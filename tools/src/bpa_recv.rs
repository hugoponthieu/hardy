use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use hardy_bpa::{
    async_trait,
    bpa::BpaRegistration,
    services::{Application, ApplicationSink, StatusNotify},
};
use hardy_bpv7::{
    bundle::Id,
    eid::{Eid, Service},
    status_report::ReasonCode,
};
use hardy_proto::client::RemoteBpa;
use tokio::sync::Notify;
use tracing_subscriber::fmt;

#[derive(Parser, Debug)]
#[command(author, version, about = "Register an application on a BPA over gRPC and print received payloads")]
struct Cli {
    /// BPA gRPC endpoint, e.g. http://[::1]:50062
    #[arg(long)]
    grpc: String,

    /// Local IPN service number to register
    #[arg(long)]
    service: u32,

    /// Timeout for BPA gRPC registration and initial sink setup
    #[arg(long, default_value = "5s", value_parser = parse_duration)]
    register_timeout: std::time::Duration,
}

fn parse_duration(input: &str) -> Result<std::time::Duration, String> {
    humantime::parse_duration(input).map_err(|err| err.to_string())
}

struct App {
    sink: tokio::sync::Mutex<Option<Arc<dyn ApplicationSink>>>,
    sink_ready: Notify,
    registered_source: Mutex<Option<Eid>>,
}

impl App {
    fn new() -> Self {
        Self {
            sink: tokio::sync::Mutex::new(None),
            sink_ready: Notify::new(),
            registered_source: Mutex::new(None),
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
        *self.sink.lock().await = Some(Arc::from(sink));
        *self
            .registered_source
            .lock()
            .expect("registered_source poisoned") = Some(source.clone());
        self.sink_ready.notify_waiters();
        println!("registered receiver endpoint: {source}");
    }

    async fn on_unregister(&self) {
        println!("receiver application unregistered");
    }

    async fn on_receive(
        &self,
        source: Eid,
        _expiry: time::OffsetDateTime,
        ack_requested: bool,
        payload: hardy_bpa::Bytes,
    ) {
        let payload = match std::str::from_utf8(payload.as_ref()) {
            Ok(text) => text.to_string(),
            Err(_) => payload
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(""),
        };

        println!(
            "received payload from {source} ack_requested={ack_requested} payload={payload}"
        );
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

#[tokio::main]
async fn main() -> Result<()> {
    fmt().with_max_level(tracing::Level::INFO).init();

    let cli = Cli::parse();
    let remote_bpa = RemoteBpa::new(cli.grpc.clone());
    let app = Arc::new(App::new());

    println!(
        "connecting to BPA gRPC at {} and registering receiver on service {}",
        cli.grpc, cli.service
    );

    tokio::time::timeout(
        cli.register_timeout,
        remote_bpa.register_application(Some(Service::Ipn(cli.service)), app.clone()),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "timed out registering receiver after {:?}",
            cli.register_timeout
        )
    })?
    .with_context(|| format!("failed to register receiver service {} on {}", cli.service, cli.grpc))?;

    let sink = tokio::time::timeout(cli.register_timeout, app.sink())
        .await
        .map_err(|_| {
            anyhow!(
                "timed out waiting for receiver sink after {:?}",
                cli.register_timeout
            )
        })?;

    println!("receiver ready, waiting for bundles; press Ctrl+C to stop");
    tokio::signal::ctrl_c()
        .await
        .context("failed to wait for Ctrl+C")?;

    sink.unregister().await;
    Ok(())
}
