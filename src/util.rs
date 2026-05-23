use serde_json::{Map, Value, json};
use std::{
    fs,
    net::{Ipv4Addr, UdpSocket as StdUdpSocket},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::config::POWER_CURVE_RPM_BUCKET;
use crate::packet::{f32_at, i32_at};

pub(crate) fn object_remove_key(curve: &mut Map<String, Value>, field: &str, key: &str) {
    if let Some(object) = curve.get_mut(field).and_then(Value::as_object_mut) {
        object.remove(key);
    }
}

pub(crate) fn ensure_object<'a>(
    object: &'a mut Map<String, Value>,
    field: &str,
) -> &'a mut Map<String, Value> {
    if !object.get(field).is_some_and(Value::is_object) {
        object.insert(field.to_string(), json!({}));
    }
    object.get_mut(field).unwrap().as_object_mut().unwrap()
}

pub(crate) fn ensure_path_object<'a>(
    value: &'a mut Value,
    path: &[&str],
) -> &'a mut Map<String, Value> {
    let mut current = value;
    for part in path {
        if !current.is_object() {
            *current = json!({});
        }
        let object = current.as_object_mut().unwrap();
        if !object.get(*part).is_some_and(Value::is_object) {
            object.insert((*part).to_string(), json!({}));
        }
        current = object.get_mut(*part).unwrap();
    }
    current.as_object_mut().unwrap()
}

pub(crate) fn get_f64(value: &Value, path: &[&str]) -> f64 {
    let mut current = value;
    for part in path {
        current = match current.get(*part) {
            Some(value) => value,
            None => return 0.0,
        };
    }
    current
        .as_f64()
        .or_else(|| current.as_i64().map(|n| n as f64))
        .unwrap_or(0.0)
}

pub(crate) fn get_child_f64(value: &Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

pub(crate) fn get_child_i64(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
        .unwrap_or(0)
}

pub(crate) fn get_curve_f64(curve: &Map<String, Value>, key: &str) -> f64 {
    curve
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

pub(crate) fn get_curve_i64(curve: &Map<String, Value>, key: &str) -> i64 {
    curve
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
        .unwrap_or(0)
}

pub(crate) fn map_fields(fields: &[(&str, f64)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect()
}

pub(crate) fn bucket_key(rpm: f64) -> String {
    ((rpm / POWER_CURVE_RPM_BUCKET).round() * POWER_CURVE_RPM_BUCKET)
        .round()
        .to_string()
}

pub(crate) fn round_to(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

pub(crate) fn clamp(value: f64, low: f64, high: f64) -> f64 {
    value.max(low).min(high)
}

pub(crate) fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub(crate) fn timestamp_name() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    secs.to_string()
}

pub(crate) fn prune_logs(log_dir: &Path, prefix: &str, keep: usize) {
    let mut logs: Vec<_> = fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    logs.sort_by_key(|item| std::cmp::Reverse(item.0));
    for (_, path) in logs.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn scan_floats(packet: &[u8]) -> Value {
    let mut values = Vec::new();
    if packet.len() < 4 {
        return json!(values);
    }
    for offset in (0..=packet.len() - 4).step_by(4) {
        let value = f32_at(packet, offset, 0.0);
        if value.abs() < 1_000_000.0 && value.abs() > 0.000001 {
            values.push(json!([offset, (value * 100000.0).round() / 100000.0]));
        }
    }
    json!(values)
}

pub(crate) fn scan_ints(packet: &[u8]) -> Value {
    let mut values = Vec::new();
    if packet.len() < 4 {
        return json!(values);
    }
    for offset in (0..=packet.len() - 4).step_by(4) {
        let value = i32_at(packet, offset, 0);
        if value.abs() < 10_000_000 && value != 0 {
            values.push(json!([offset, value]));
        }
    }
    json!(values)
}

pub(crate) fn changed_bytes(previous: Option<&[u8]>, packet: &[u8]) -> Vec<usize> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    let length = previous.len().min(packet.len());
    let mut changed = (0..length)
        .filter(|index| previous[*index] != packet[*index])
        .collect::<Vec<_>>();
    if packet.len() != previous.len() {
        changed.extend(length..packet.len().max(previous.len()));
    }
    changed
}

pub(crate) fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn lan_addresses() -> Vec<Ipv4Addr> {
    let mut addresses = vec![Ipv4Addr::LOCALHOST];
    if let Ok(sock) = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        && sock.connect(("8.8.8.8", 80)).is_ok()
        && let Ok(addr) = sock.local_addr()
        && let std::net::IpAddr::V4(ip) = addr.ip()
        && !addresses.contains(&ip)
    {
        addresses.push(ip);
    }
    addresses
}
