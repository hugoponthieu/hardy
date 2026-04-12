use crate::config;
use cspcl_bindings::async_api::{AsyncCspcl, AsyncReceiver, AsyncSender};
use cspcl_bindings::{
    Error as CspclError, Interface as CspInterface, InterfaceName, ReceivedBundle,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transport init failed: {0}")]
    Init(#[from] CspclError),
    #[error("transport send failed: {0}")]
    Send(#[source] CspclError),
    #[error("transport receive failed: {0}")]
    Recv(#[source] CspclError),
}

pub enum ReceiveResult {
    Bundle(ReceivedBundle),
    Timeout,
}

#[derive(Clone)]
pub struct Transport {
    runtime: AsyncCspcl,
    sender: AsyncSender,
    receiver: AsyncReceiver,
}

impl Transport {
    pub fn new(config: &config::Config) -> Result<Self, Error> {
        let interface = match config.interface {
            config::Interface::Loopback => {
                CspInterface::Loopback(InterfaceName::new(&config.interface_name))
            }
            config::Interface::Can => CspInterface::Can(InterfaceName::new(&config.interface_name)),
        };

        let runtime = AsyncCspcl::from_sync(cspcl_bindings::Cspcl::new(
            config.local_addr,
            config.port,
            interface,
        )?);
        let (sender, receiver) = runtime.split();
        Ok(Self {
            runtime,
            sender,
            receiver,
        })
    }

    pub async fn send_bundle(&self, payload: &[u8], addr: u8, port: u8) -> Result<(), Error> {
        self.sender
            .send_bundle(payload, addr, port)
            .await
            .map_err(Error::Send)
    }

    pub async fn recv_bundle(&self, timeout_ms: u32) -> Result<ReceiveResult, Error> {
        match self.receiver.recv_bundle(timeout_ms).await {
            Ok(bundle) => Ok(ReceiveResult::Bundle(bundle)),
            Err(err)
                if err.code() == cspcl_bindings::cspcl_sys::cspcl_error_t_CSPCL_ERR_TIMEOUT =>
            {
                Ok(ReceiveResult::Timeout)
            }
            Err(err) => Err(Error::Recv(err)),
        }
    }

    pub async fn shutdown(&self) -> Result<(), Error> {
        self.runtime.shutdown().await.map_err(Error::Init)
    }
}
