use super::*;
use hardy_bpa::async_trait;
use hardy_bpa::bpa::BpaRegistration;
use hardy_bpa::services::{Application, ApplicationSink, StatusNotify};
use hardy_bpv7::eid::{Eid, Service};
use hardy_proto::client::RemoteBpa;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    NoResponse = 1,
    Error = 2,
}

#[derive(Debug)]
struct Response {
    source: Eid,
    payload: hardy_bpa::Bytes,
}

struct EchoClient {
    sink: hardy_async::sync::spin::Mutex<Option<Arc<dyn ApplicationSink>>>,
    response_tx: hardy_async::sync::spin::Mutex<Option<tokio::sync::oneshot::Sender<Response>>>,
}

#[async_trait]
impl Application for EchoClient {
    async fn on_register(&self, _source: &Eid, sink: Box<dyn ApplicationSink>) {
        *self.sink.lock() = Some(Arc::from(sink));
    }

    async fn on_unregister(&self) {
        self.sink.lock().take();
    }

    async fn on_receive(
        &self,
        source: Eid,
        _expiry: time::OffsetDateTime,
        _ack_requested: bool,
        payload: hardy_bpa::Bytes,
    ) {
        if let Some(tx) = self.response_tx.lock().take() {
            let _ = tx.send(Response { source, payload });
        }
    }

    async fn on_status_notify(
        &self,
        _bundle_id: &hardy_bpv7::bundle::Id,
        _from: &Eid,
        _kind: StatusNotify,
        _reason: hardy_bpv7::status_report::ReasonCode,
        _timestamp: Option<time::OffsetDateTime>,
    ) {
    }
}

/// Send one payload via BPA gRPC and wait for the echoed response.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
pub struct Command {
    /// BPA gRPC address
    #[arg(long, default_value = "http://[::1]:50051")]
    bpa: String,

    /// Destination EID (typically remote echo service, e.g. ipn:2.7)
    destination: Eid,

    /// Payload to send
    #[arg(long, default_value = "hello from hardy")]
    payload: String,

    /// Source IPN service number to register for the return traffic
    #[arg(long, default_value_t = 4242)]
    source_service: u32,

    /// Bundle lifetime
    #[arg(long, default_value = "60s")]
    lifetime: humantime::Duration,

    /// Max time to wait for the echo response
    #[arg(long, default_value = "15s")]
    timeout: humantime::Duration,
}

async fn exec_async(args: &Command) -> anyhow::Result<ExitCode> {
    let remote_bpa = RemoteBpa::new(args.bpa.clone());
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    let app = Arc::new(EchoClient {
        sink: hardy_async::sync::spin::Mutex::new(None),
        response_tx: hardy_async::sync::spin::Mutex::new(Some(response_tx)),
    });

    let source = remote_bpa
        .register_application(Some(Service::Ipn(args.source_service)), app.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to register application via gRPC: {e}"))?;

    let sink = app
        .sink
        .lock()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Application sink unavailable after registration"))?;

    let bundle_id = sink
        .send(
            args.destination.clone(),
            args.payload.clone().into_bytes().into(),
            *args.lifetime,
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send bundle: {e}"))?;

    println!(
        "sent bundle {} from {} to {}",
        bundle_id, source, args.destination
    );

    let outcome = tokio::time::timeout(*args.timeout, response_rx).await;
    sink.unregister().await;

    match outcome {
        Ok(Ok(response)) => {
            let payload = String::from_utf8_lossy(&response.payload);
            println!("received echo from {}: {}", response.source, payload);
            Ok(ExitCode::Success)
        }
        Ok(Err(_)) => {
            eprintln!("application response channel closed");
            Ok(ExitCode::Error)
        }
        Err(_) => {
            eprintln!(
                "timed out waiting for echo response after {}",
                humantime::format_duration(*args.timeout)
            );
            Ok(ExitCode::NoResponse)
        }
    }
}

pub fn exec(args: Command) -> ! {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to build tokio runtime: {e}");
            std::process::exit(ExitCode::Error as i32);
        }
    };

    match runtime.block_on(exec_async(&args)) {
        Ok(exit_code) => std::process::exit(exit_code as i32),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(ExitCode::Error as i32);
        }
    }
}
