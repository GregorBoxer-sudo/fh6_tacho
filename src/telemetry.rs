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
    pub(crate) debug_mode: bool,
}

pub(crate) struct TelemetryHub {
    pub(crate) tx: broadcast::Sender<String>,
    packet_count: Mutex<u64>,
    last_packet_at: Mutex<Option<std::time::Instant>>,
    /// Most recent telemetry packet — used by the GUI overlay to read shift data.
    last_value: Mutex<Option<Value>>,
}

impl TelemetryHub {
    pub(crate) fn new() -> Self {
        let (tx, _) = broadcast::channel(32);
        Self {
            tx,
            packet_count: Mutex::new(0),
            last_packet_at: Mutex::new(None),
            last_value: Mutex::new(None),
        }
    }

    pub(crate) fn publish(&self, payload: &Value) {
        let message = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
        *self.packet_count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        *self.last_packet_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
        *self.last_value.lock().unwrap_or_else(|e| e.into_inner()) = Some(payload.clone());
        let _ = self.tx.send(message);
    }

    /// Returns the most recently received telemetry packet, or `None` if no
    /// packet has been received yet.  Cheap clone of a small JSON value.
    pub(crate) fn latest(&self) -> Option<Value> {
        self.last_value.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn status(&self) -> Value {
        let packets = *self.packet_count.lock().unwrap_or_else(|e| e.into_inner());
        let age_ms = self
            .last_packet_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|at| at.elapsed().as_millis() as u64);
        json!({ "packets": packets, "ageMs": age_ms })
    }
}
