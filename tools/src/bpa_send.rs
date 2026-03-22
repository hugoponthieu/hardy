use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use hardy_bpa::{
    async_trait,
    bpa::BpaRegistration,
    services::{Application, ApplicationSink, SendOptions, StatusNotify},
};
use hardy_bpv7::{
    bundle::Id,
    eid::{Eid, Service},
    status_report::ReasonCode,
};
use hardy_proto::client::RemoteBpa;
use tokio::sync::{Notify, oneshot};
use tracing_subscriber::fmt;

#[derive(Parser, Debug)]
#[command(author, version, about = "Send one payload into a BPA over gRPC")]
struct Cli {
    /// BPA gRPC endpoint, e.g. http://[::1]:50061
    #[arg(long)]
    grpc: String,

    /// Destination EID, e.g. ipn:2.7
    #[arg(long)]
    destination: Eid,

    /// Local IPN service number to register before sending (0 = auto-assign)
    #[arg(long, default_value_t = 0)]
    source_service: u32,

    /// Payload to send as UTF-8 text
    #[arg(long)]
    payload: String,

    /// Bundle lifetime, e.g. 30s, 5m
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    lifetime: Duration,

    /// Timeout for BPA gRPC registration and initial sink setup
    #[arg(long, default_value = "5s", value_parser = parse_duration)]
    register_timeout: Duration,

    /// Wait for the first payload received back on the registered source endpoint
    #[arg(long, value_parser = parse_duration)]
    wait: Option<Duration>,

    /// Request a delivery acknowledgment
    #[arg(long, default_value_t = false)]
    request_ack: bool,
}

fn parse_duration(input: &str) -> Result<Duration, String> {
    humantime::parse_duration(input).map_err(|err| err.to_string())
}

struct ReceivedPayload {
    source: Eid,
    payload: Vec<u8>,
    ack_requested: bool,
}

struct App {
    sink: tokio::sync::Mutex<Option<Arc<dyn ApplicationSink>>>,
    sink_ready: Notify,
    first_receive_tx: Mutex<Option<oneshot::Sender<ReceivedPayload>>>,
}

impl App {
    fn new(first_receive_tx: oneshot::Sender<ReceivedPayload>) -> Self {
        Self {
            sink: tokio::sync::Mutex::new(None),
            sink_ready: Notify::new(),
            first_receive_tx: Mutex::new(Some(first_receive_tx)),
        }
    }

    async fn sink(&self) -> Arc<dyn ApplicationSink> {
        loop {
            if let Some(sink) = self.sink.lock().await.clone() {
                return sink;
            }
            self.sink_ready.notified().await;
        }
    }
}

#[async_trait]
impl Application for App {
    async fn on_register(&self, source: &Eid, sink: Box<dyn ApplicationSink>) {
        let mut slot = self.sink.lock().await;
        *slot = Some(Arc::from(sink));
        self.sink_ready.notify_waiters();
        println!("registered source endpoint: {source}");
    }

    async fn on_unregister(&self) {
        println!("application unregistered");
    }

    async fn on_receive(
        &self,
        source: Eid,
        _expiry: time::OffsetDateTime,
        ack_requested: bool,
        payload: hardy_bpa::Bytes,
    ) {
        if let Some(tx) = self
            .first_receive_tx
            .lock()
            .expect("first_receive_tx poisoned")
            .take()
        {
            let _ = tx.send(ReceivedPayload {
                source,
                payload: payload.to_vec(),
                ack_requested,
            });
        }
    }

    async fn on_status_notify(
        &self,
        bundle_id: &Id,
        from: &Eid,
        kind: StatusNotify,
        reason: ReasonCode,
        timestamp: Option<time::OffsetDateTime>,
    ) {
        let timestamp = timestamp
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string());
        println!(
            "status: bundle_id={bundle_id} from={from} kind={kind:?} reason={reason:?} time={timestamp}"
        );
    }
}

fn render_payload(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn describe_source_service(service_number: u32) -> String {
    if service_number == 0 {
        "auto-assigned source service".to_string()
    } else {
        format!("source service {service_number}")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt().with_max_level(tracing::Level::INFO).init();

    let cli = Cli::parse();
    let remote_bpa = RemoteBpa::new(cli.grpc.clone());

    dbg!(cli.wait);
    let (reply_tx, reply_rx) = oneshot::channel();
    let app = Arc::new(App::new(reply_tx));

    println!(
        "connecting to BPA gRPC at {} and registering {}",
        cli.grpc,
        describe_source_service(cli.source_service)
    );
    let source_eid = tokio::time::timeout(
        cli.register_timeout,
        remote_bpa.register_application(Some(Service::Ipn(cli.source_service)), app.clone()),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "timed out registering application after {:?}",
            cli.register_timeout
        )
    })?
    .with_context(|| {
        if cli.source_service == 0 {
            format!(
                "failed to register auto-assigned application on {}",
                cli.grpc
            )
        } else {
            format!(
                "failed to register application on {} with source service {}. \
retry with --source-service 0 for an auto-assigned service, or pick a different explicit value",
                cli.grpc, cli.source_service
            )
        }
    })?;

    println!("waiting for application sink");
    let sink = tokio::time::timeout(cli.register_timeout, app.sink())
        .await
        .map_err(|_| {
            anyhow!(
                "timed out waiting for application sink after {:?}",
                cli.register_timeout
            )
        })?;
    println!("application sink ready");
    let bundle_id = sink
        .send(
            cli.destination.clone(),
            cli.payload.clone().into_bytes().into(),
            cli.lifetime,
            Some(SendOptions {
                request_ack: cli.request_ack,
                notify_delivery: cli.request_ack,
                ..SendOptions::default()
            }),
        )
        .await
        .context("failed to send payload")?;

    println!("sent bundle_id: {bundle_id}");
    println!("from: {source_eid}");
    println!("to: {}", cli.destination);
    println!("payload: {}", cli.payload);

    let result = if let Some(wait_for) = cli.wait {
        match tokio::time::timeout(wait_for, reply_rx).await {
            Ok(Ok(reply)) => {
                println!(
                    "received reply from {} ack_requested={} payload={}",
                    reply.source,
                    reply.ack_requested,
                    render_payload(&reply.payload)
                );
                Ok(())
            }
            Ok(Err(_)) => Err(anyhow!("reply channel closed before a payload arrived")),
            Err(_) => Err(anyhow!("timed out waiting for reply after {:?}", wait_for)),
        }
    } else {
        Ok(())
    };

    sink.unregister().await;
    result
}
