use anyhow::Result;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    sync::Mutex,
    time::Duration,
};

use crate::shift::{learned_forward_gear, power_curve_key};
use crate::util::*;

pub(crate) struct ShiftCacheLogger {
    file: Mutex<File>,
    last_shift_decisions: Mutex<HashMap<String, Value>>,
    last_cache_events: Mutex<HashMap<String, Value>>,
}

impl ShiftCacheLogger {
    pub(crate) fn new(log_dir: &Path, keep_sessions: usize) -> Result<Self> {
        fs::create_dir_all(log_dir)?;
        prune_logs(log_dir, "shift-cache-", keep_sessions.max(1));
        let path = log_dir.join(format!("shift-cache-{}.jsonl", timestamp_name()));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        println!("Shift cache log: {}", path.display());
        Ok(Self {
            file: Mutex::new(file),
            last_shift_decisions: Mutex::new(HashMap::new()),
            last_cache_events: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn log(&self, event: &str, payload: &Value, fields: Map<String, Value>) {
        let mut entry = Map::new();
        entry.insert("at".to_string(), json!(now_seconds()));
        entry.insert("event".to_string(), json!(event));
        entry.insert("car".to_string(), json!(power_curve_key(payload)));
        entry.insert("gear".to_string(), json!(learned_forward_gear(payload)));
        entry.insert(
            "rpm".to_string(),
            json!((get_f64(payload, &["engine", "rpm"]) * 10.0).round() / 10.0),
        );
        entry.extend(fields);
        let line = serde_json::to_string(&Value::Object(entry)).unwrap();
        let mut file = self.file.lock().unwrap();
        let _ = writeln!(file, "{line}");
    }

    pub(crate) fn log_shift_decision(&self, payload: &Value, fields: Map<String, Value>) {
        let key = format!(
            "{}:{}",
            power_curve_key(payload),
            learned_forward_gear(payload)
        );
        let signature = json!([
            fields.get("source").cloned().unwrap_or(Value::Null),
            round_to(get_curve_f64(&fields, "shiftNowRpm"), 50.0),
            round_to(get_curve_f64(&fields, "learnedShiftRpm"), 50.0),
            round_to(get_curve_f64(&fields, "safetyShiftRpm"), 50.0),
        ]);
        let mut last = self.last_shift_decisions.lock().unwrap();
        if last.get(&key) == Some(&signature) {
            return;
        }
        last.insert(key, signature);
        drop(last);
        self.log("shift_decision", payload, fields);
    }

    pub(crate) fn log_cache_event(&self, event: &str, payload: &Value, fields: Map<String, Value>) {
        if matches!(event, "cache_hit_validated" | "cache_pending_validation") {
            let key = format!(
                "{event}:{}:{}",
                power_curve_key(payload),
                learned_forward_gear(payload)
            );
            let signature = json!([
                round_to(get_curve_f64(&fields, "shiftRpm"), 50.0),
                round_to(get_curve_f64(&fields, "warningRpm"), 50.0),
            ]);
            let mut last = self.last_cache_events.lock().unwrap();
            if last.get(&key) == Some(&signature) {
                return;
            }
            last.insert(key, signature);
        }
        self.log(event, payload, fields);
    }
}

pub(crate) struct PacketInspector {
    sample_every: u64,
    console_every: Duration,
    state: Mutex<PacketInspectorState>,
    file: Mutex<File>,
}

struct PacketInspectorState {
    packet_count: u64,
    previous: Option<Vec<u8>>,
    last_console_at: std::time::Instant,
}

impl PacketInspector {
    pub(crate) fn new(log_dir: &Path, sample_every: u64) -> Result<Self> {
        fs::create_dir_all(log_dir)?;
        let path = log_dir.join(format!("forza-packets-{}.jsonl", timestamp_name()));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        println!("Packet inspector log: {}", path.display());
        Ok(Self {
            sample_every: sample_every.max(1),
            console_every: Duration::from_secs(1),
            state: Mutex::new(PacketInspectorState {
                packet_count: 0,
                previous: None,
                last_console_at: std::time::Instant::now(),
            }),
            file: Mutex::new(file),
        })
    }

    pub(crate) fn inspect(&self, packet: &[u8], decoded: &Value) {
        let floats = scan_floats(packet);
        let ints = scan_ints(packet);
        let mut state = self.state.lock().unwrap();
        state.packet_count += 1;
        let packet_count = state.packet_count;
        let changed = changed_bytes(state.previous.as_deref(), packet);
        if packet_count.is_multiple_of(self.sample_every) {
            let entry = json!({
                "receivedAt": now_seconds(),
                "packetNumber": packet_count,
                "packetBytes": packet.len(),
                "rawHex": hex_string(packet),
                "decodedByCurrentMap": decoded,
                "changedByteOffsets": changed.iter().take(300).copied().collect::<Vec<_>>(),
                "float32Candidates": floats,
                "int32Candidates": ints,
            });
            if let Ok(line) = serde_json::to_string(&entry) {
                let _ = writeln!(self.file.lock().unwrap(), "{line}");
            }
        }
        if state.last_console_at.elapsed() >= self.console_every {
            let changed_preview = changed
                .iter()
                .take(40)
                .map(|offset| offset.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let float_preview = floats
                .as_array()
                .into_iter()
                .flatten()
                .take(18)
                .filter_map(|item| {
                    let pair = item.as_array()?;
                    Some(format!("{}:{}", pair.first()?, pair.get(1)?))
                })
                .collect::<Vec<_>>()
                .join(" ");
            let int_preview = ints
                .as_array()
                .into_iter()
                .flatten()
                .take(18)
                .filter_map(|item| {
                    let pair = item.as_array()?;
                    Some(format!("{}:{}", pair.first()?, pair.get(1)?))
                })
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "[inspect] packets={} size={} changed={}",
                packet_count,
                packet.len(),
                if changed_preview.is_empty() {
                    "-"
                } else {
                    &changed_preview
                }
            );
            println!(
                "[inspect] f32 {}",
                if float_preview.is_empty() {
                    "-"
                } else {
                    &float_preview
                }
            );
            println!(
                "[inspect] i32 {}",
                if int_preview.is_empty() {
                    "-"
                } else {
                    &int_preview
                }
            );
            state.last_console_at = std::time::Instant::now();
        }
        state.previous = Some(packet.to_vec());
    }
}
