use hardy_bpa::Bytes;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::oneshot::{Receiver, Sender};

use hardy_async::sync::Mutex;

use crate::frame::Frame;

#[derive(Default)]
pub struct PendingAcks {
    inner: Mutex<HashMap<u64, Sender<()>>>,
    next_bundle_id: AtomicU64,
}

pub enum AckError {
    SenderNotFound,
    ReceiverDropped,
}

impl PendingAcks {
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
            next_bundle_id: AtomicU64::new(1),
        }
    }

    pub fn add(&self, bundle: Bytes) -> (Frame, Receiver<()>) {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let bundle_id = self.next_bundle_id.fetch_add(1, Ordering::Relaxed);
        self.inner.lock().insert(bundle_id, ack_tx);
        (
            Frame::Bundle {
                id: bundle_id,
                payload: bundle.to_vec(),
            },
            ack_rx,
        )
    }

    fn remove(&self, bundle_id: u64) -> Option<Sender<()>> {
        self.inner.lock().remove(&bundle_id)
    }

    pub fn discard_ack(&self, bundle_id: Option<u64>) {
        if let Some(bundle_id) = bundle_id {
            self.remove(bundle_id);
        }
    }

    pub fn ack_bundle(&self, bundle_id: u64) -> Result<(), AckError> {
        self.remove(bundle_id)
            .ok_or(AckError::SenderNotFound)?
            .send(())
            .map_err(|_| AckError::ReceiverDropped)?;
        Ok(())
    }
}
