use std::sync::Arc;

use crate::config;
use cspcl_bindings::{
    Cspcl as CspclInner, Error as CspclError, Interface as CspInterface, InterfaceName,
    ReceivedBundle,
    asynchronous::{Cspcl, Receiver, Sender},
};
use tracing::debug;

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
    cspcl: Arc<Cspcl>,
    sender: Arc<Sender>,
    receiver: Arc<Receiver>,
}

impl Transport {
    pub fn new(config: &config::Config) -> Result<Self, Error> {
        let interface = match config.interface {
            config::Interface::Loopback => {
                CspInterface::Loopback(InterfaceName::new(&config.interface_name))
            }
            config::Interface::Can => CspInterface::Can(InterfaceName::new(&config.interface_name)),
        };
        let cspcl_inner = CspclInner::new(config.local_addr, config.port, interface)?;
        let cspcl = Arc::new(Cspcl::from_sync(cspcl_inner));
        let (sender, receiver) = cspcl.split();
        let sender = Arc::new(sender);
        let receiver = Arc::new(receiver);

        Ok(Self {
            cspcl,
            sender,
            receiver,
        })
    }

    pub async fn send_bundle(
        &self,
        payload: impl Into<Vec<u8>>,
        addr: u8,
        port: u8,
    ) -> Result<(), Error> {
        debug!("Try sending bundle to: {}:{}", addr, port);
        self.sender
            .send_bundle(&payload.into(), addr, port)
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
        self.cspcl.shutdown().await.map_err(Error::Init)
    }
}
