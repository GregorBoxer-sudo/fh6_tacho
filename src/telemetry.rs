use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) hub: Arc<TelemetryHub>,
    pub(crate) data_dir: PathBuf,
}

pub(crate) struct TelemetryHub {
    pub(crate) tx: broadcast::Sender<String>,
    packet_count: Mutex<u64>,
    last_packet_at: Mutex<Option<std::time::Instant>>,
}

impl TelemetryHub {
    pub(crate) fn new() -> Self {
        let (tx, _) = broadcast::channel(32);
        Self {
            tx,
            packet_count: Mutex::new(0),
            last_packet_at: Mutex::new(None),
        }
    }

    pub(crate) fn publish(&self, payload: &Value) {
        let message = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
        *self.packet_count.lock().unwrap() += 1;
        *self.last_packet_at.lock().unwrap() = Some(std::time::Instant::now());
        let _ = self.tx.send(message);
    }

    pub(crate) fn status(&self) -> Value {
        let packets = *self.packet_count.lock().unwrap();
        let age_ms = self
            .last_packet_at
            .lock()
            .unwrap()
            .map(|at| at.elapsed().as_millis() as u64);
        json!({ "packets": packets, "ageMs": age_ms })
    }
}
