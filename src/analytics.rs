use anyhow::Result;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, SyncSender},
};

use crate::config::{
    INITIAL_ENGINE_LIMIT_RATIO_OF_TACHO_MAX, SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT,
};
use crate::shift::power_curve_key;
use crate::util::{get_f64, now_seconds};

const SAMPLE_INTERVAL_SECONDS: f64 = 0.05;
const SESSION_GAP_SECONDS: f64 = 30.0;
const MAX_SESSION_DURATION_SECONDS: f64 = 3600.0; // hard cap — never log more than 1 h
const MAX_SESSIONS: usize = 50;
const MAX_CARS: usize = 50;
/// Depth of the channel between the async hot path and the recorder worker.
/// 128 slots × 20 Hz ≈ 6 s of head-room — in practice the worker drains in µs.
const RECORDER_CHANNEL_CAP: usize = 128;

/// Handle to the background recording worker.
///
/// The async hot path (UDP receive loop, demo loop) calls [`TelemetryRecorder::record`]
/// which merely clones the telemetry `Value` and sends it over a bounded channel.
/// The actual file I/O — opening/closing JSONL session files, writing samples —
/// happens on a dedicated `std::thread` so the Tokio executor is never blocked.
pub(crate) struct TelemetryRecorder {
    sender: SyncSender<Value>,
    /// Kept alive so the worker thread lives as long as the recorder.
    _worker: std::thread::JoinHandle<()>,
}

/// All mutable recording state lives exclusively on the worker thread.
struct RecorderState {
    file: Option<BufWriter<File>>,
    session_id: String,
    session_started_at: f64,
    last_sample_at: f64,
    last_seen_at: f64,
    car_key: String,
    last_race_on: Option<bool>,
}

impl TelemetryRecorder {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir: PathBuf = dir.into();
        fs::create_dir_all(&dir)?;
        let (sender, receiver) = mpsc::sync_channel::<Value>(RECORDER_CHANNEL_CAP);
        let worker = std::thread::spawn(move || {
            let mut state = RecorderState {
                file: None,
                session_id: String::new(),
                session_started_at: 0.0,
                last_sample_at: 0.0,
                last_seen_at: 0.0,
                car_key: String::new(),
                last_race_on: None,
            };
            // Drive the recording loop until the sender side is dropped.
            while let Ok(payload) = receiver.recv() {
                state.record_inner(&payload, &dir);
            }
            // Flush any in-flight buffered writes before the thread exits.
            if let Some(mut f) = state.file.take() {
                let _ = f.flush();
            }
        });
        Ok(Self { sender, _worker: worker })
    }

    /// Non-blocking: clones the telemetry value and sends it to the worker thread.
    /// Drops the sample silently if the channel is full (should never happen at 20 Hz).
    pub(crate) fn record(&self, payload: &Value) {
        let _ = self.sender.try_send(payload.clone());
    }
}

impl RecorderState {
    fn record_inner(&mut self, payload: &Value, dir: &Path) {
        let now = get_f64(payload, &["receivedAt"]);
        let car_key = power_curve_key(payload);
        if car_key.is_empty() || now <= 0.0 {
            return;
        }

        let race_on = payload
            .get("raceOn")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Race ended — flush and close so subsequent data starts a fresh session.
        if self.last_race_on == Some(true) && !race_on {
            if let Some(mut f) = self.file.take() {
                let _ = f.flush();
            }
        }

        let race_started = self.last_race_on == Some(false) && race_on;
        self.last_race_on = Some(race_on);

        let needs_new_session = self.file.is_none()
            || self.car_key != car_key
            || now - self.last_seen_at > SESSION_GAP_SECONDS
            || race_started
            || (self.session_started_at > 0.0
                && now - self.session_started_at >= MAX_SESSION_DURATION_SECONDS);
        if needs_new_session {
            trim_old_sessions(dir);
            let session_id = format!("{}-{}", timestamp_id(now), car_key.replace(':', "-"));
            let path = dir.join(format!("{session_id}.jsonl"));
            match OpenOptions::new().create(true).append(true).open(path) {
                Ok(file) => {
                    self.file = Some(BufWriter::new(file));
                    self.session_id = session_id;
                    self.session_started_at = now;
                    self.last_sample_at = 0.0;
                    self.car_key = car_key.clone();
                }
                Err(_) => return,
            }
        }
        self.last_seen_at = now;
        if now - self.last_sample_at < SAMPLE_INTERVAL_SECONDS {
            return;
        }
        self.last_sample_at = now;
        let sample = compact_sample(payload, &self.session_id, self.session_started_at);
        if let Some(file) = self.file.as_mut()
            && let Ok(line) = serde_json::to_string(&sample)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

pub(crate) fn list_sessions(dir: &Path) -> Value {
    let mut sessions = read_session_summaries(dir);
    sessions.sort_by(|a, b| {
        get_f64(b, &["startedAt"])
            .partial_cmp(&get_f64(a, &["startedAt"]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sessions.truncate(MAX_SESSIONS);
    json!({ "sessions": sessions })
}

pub(crate) fn session_detail(dir: &Path, id: &str) -> Value {
    let Some(path) = safe_session_path(dir, id) else {
        return json!({ "error": "not_found" });
    };
    let samples = read_samples(&path);
    let summary = summarize_samples(id, &samples);
    let series = downsample_series(&samples, 3600);
    json!({ "summary": summary, "samples": series })
}

pub(crate) fn session_track(dir: &Path, id: &str, max_points: usize) -> Value {
    let Some(path) = safe_session_path(dir, id) else {
        return json!({ "error": "not_found" });
    };
    let samples = read_samples(&path);
    let summary = summarize_samples(id, &samples);
    let mut raw_points = Vec::new();
    let mut fallback_points = Vec::new();
    for sample in &samples {
        let x = get_f64(sample, &["position", "x"]);
        let z = get_f64(sample, &["position", "z"]);
        if !x.is_finite() || !z.is_finite() {
            continue;
        }
        if x.abs() < f64::EPSILON && z.abs() < f64::EPSILON {
            continue;
        }
        let source = sample
            .pointer("/position/source")
            .and_then(Value::as_str)
            .unwrap_or("");
        let g_lat = get_f64(sample, &["gLat"]);
        let g_long = get_f64(sample, &["gLong"]);
        let point = json!({
            "x": x,
            "z": z,
            "t": get_f64(sample, &["t"]),
            "speed": get_f64(sample, &["speed"]),
            "drift": get_f64(sample, &["drift"]),
            "slip": get_f64(sample, &["slip"]),
            "gLat": g_lat,
            "gLong": g_long,
            "gTotal": (g_lat * g_lat + g_long * g_long).sqrt(),
            "raceOn": sample.get("raceOn").and_then(Value::as_bool).unwrap_or(false),
            "source": source,
        });
        if source == "raw" {
            raw_points.push(point);
        } else {
            fallback_points.push(point);
        }
    }
    let using_raw = !raw_points.is_empty();
    let mut points = if using_raw {
        raw_points
    } else {
        fallback_points
    };
    if max_points > 0 && points.len() > max_points {
        points = downsample_series(&points, max_points);
    }
    json!({
        "summary": summary,
        "points": points,
        "trackSource": if using_raw { "raw" } else { "fallback" }
    })
}

/// Reads power curve buckets straight from power_curves.json —
/// learned at 60 Hz with 100-RPM bucket resolution.
pub(crate) fn car_power_curve(power_curves_path: &Path, car_key: &str) -> Value {
    let curves = fs::read_to_string(power_curves_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let Some(curve) = curves.get(car_key).and_then(Value::as_object) else {
        return json!({ "points": [] });
    };
    let Some(buckets) = curve.get("buckets").and_then(Value::as_object) else {
        return json!({ "points": [] });
    };

    let mut points: Vec<Value> = buckets
        .iter()
        .filter_map(|(rpm_str, val)| {
            let rpm = rpm_str.parse::<f64>().ok()?;
            let obj = val.as_object()?;
            let power = obj.get("power").and_then(Value::as_f64).unwrap_or(0.0);
            let torque = obj.get("torque").and_then(Value::as_f64).unwrap_or(0.0);
            if power <= 0.0 && torque <= 0.0 {
                return None;
            }
            Some(json!({ "rpm": rpm, "power": power, "torque": torque }))
        })
        .collect();

    points.sort_by(|a, b| {
        get_f64(a, &["rpm"])
            .partial_cmp(&get_f64(b, &["rpm"]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    json!({ "points": points })
}

pub(crate) fn car_browser(power_curves_path: &Path, sessions_dir: &Path) -> Value {
    let curves = fs::read_to_string(power_curves_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let session_stats = car_stats_from_sessions(sessions_dir);
    let mut cars = Vec::new();
    for (key, curve_value) in curves {
        let curve = curve_value.as_object().cloned().unwrap_or_default();
        let stats = session_stats.get(&key);
        let max_rpm = number_field(&curve, "maxRpmSignature");
        let max_gear = number_field(&curve, "maxObservedGear");
        cars.push(json!({
            "key": key,
            "ordinal": key.split(':').next().unwrap_or(""),
            "pi": key.split(':').nth(1).unwrap_or(""),
            "class": stats.and_then(|s| s.class_name.clone()).unwrap_or_else(|| "-".to_string()),
            "drivetrain": stats.and_then(|s| s.drivetrain.clone()).unwrap_or_else(|| "-".to_string()),
            "cylinders": stats.and_then(|s| s.cylinders).unwrap_or(0),
            "sessions": stats.map(|s| s.sessions).unwrap_or(0),
            "maxSpeed": stats.map(|s| s.max_speed).unwrap_or(0.0),
            "maxPower": stats.map(|s| s.max_power).unwrap_or(0.0),
            "maxRpm": max_rpm,
            "observedRpm": number_field(&curve, "savedMaxObservedRpm"),
            "maxGear": max_gear,
            "shiftTargets": curve.get("optimalShiftRpmByGear").cloned().unwrap_or_else(|| json!({})),
            "standardShiftTargets": standard_shift_targets(max_rpm, max_gear),
            "dropRatios": curve.get("gearDropRatios").cloned().unwrap_or_else(|| json!({})),
            "lastSeenAt": number_field(&curve, "lastSeenAt"),
        }));
    }
    cars.sort_by(|a, b| {
        get_f64(b, &["lastSeenAt"])
            .partial_cmp(&get_f64(a, &["lastSeenAt"]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cars.truncate(MAX_CARS);
    json!({ "cars": cars })
}

fn standard_shift_targets(max_rpm: f64, max_gear: f64) -> Value {
    let gear_count = max_gear.round() as i64;
    if max_rpm <= 0.0 || gear_count <= 0 {
        return json!({});
    }
    let standard_rpm = max_rpm
        * INITIAL_ENGINE_LIMIT_RATIO_OF_TACHO_MAX
        * SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT;
    let mut targets = Map::new();
    for gear in 1..=gear_count {
        targets.insert(gear.to_string(), json!(standard_rpm));
    }
    Value::Object(targets)
}

fn compact_sample(payload: &Value, session_id: &str, started_at: f64) -> Value {
    let at = get_f64(payload, &["receivedAt"]);
    let slip_fl = get_f64(payload, &["tireCombinedSlip", "fl"]).abs();
    let slip_fr = get_f64(payload, &["tireCombinedSlip", "fr"]).abs();
    let slip_rl = get_f64(payload, &["tireCombinedSlip", "rl"]).abs();
    let slip_rr = get_f64(payload, &["tireCombinedSlip", "rr"]).abs();
    let slip_max = slip_fl.max(slip_fr).max(slip_rl).max(slip_rr);
    json!({
        "session": session_id,
        "at": at,
        "t": (at - started_at).max(0.0),
        "car": {
            "key": power_curve_key(payload),
            "ordinal": get_f64(payload, &["car", "ordinal"]) as i64,
            "pi": get_f64(payload, &["car", "performanceIndex"]) as i64,
            "class": payload.pointer("/car/class").and_then(Value::as_str).unwrap_or("-"),
            "drivetrain": payload.pointer("/car/drivetrain").and_then(Value::as_str).unwrap_or("-"),
            "cylinders": get_f64(payload, &["car", "cylinders"]) as i64,
            "maxObservedGear": get_f64(payload, &["car", "maxObservedGear"]) as i64,
        },
        "raceOn": payload.get("raceOn").and_then(Value::as_bool).unwrap_or(false),
        "speed": get_f64(payload, &["speed", "kmh"]),
        "rpm": get_f64(payload, &["engine", "rpm"]),
        "power": get_f64(payload, &["engine", "powerHp"]),
        "torque": get_f64(payload, &["engine", "torqueNm"]),
        "boost": get_f64(payload, &["boost"]),
        "gear": get_f64(payload, &["controls", "gear"]) as i64,
        "accel": get_f64(payload, &["controls", "accel"]),
        "brake": get_f64(payload, &["controls", "brake"]),
        "steer": get_f64(payload, &["controls", "steer"]),
        "gLat": get_f64(payload, &["motion", "gLat"]),
        "gLong": get_f64(payload, &["motion", "gLong"]),
        "drift": get_f64(payload, &["motion", "driftAngleDeg"]),
        "slip": slip_max,
        "position": {
            "x": get_f64(payload, &["position", "x"]),
            "y": get_f64(payload, &["position", "y"]),
            "z": get_f64(payload, &["position", "z"]),
        },
        "shiftNow": get_f64(payload, &["engine", "shiftNowRpm"]),
        "lap": {
            "number": get_f64(payload, &["lap", "number"]) as i64,
            "current": get_f64(payload, &["lap", "current"]),
            "best": get_f64(payload, &["lap", "best"]),
            "position": get_f64(payload, &["lap", "position"]) as i64,
        }
    })
}

/// Deletes the oldest session files once the total exceeds MAX_SESSIONS.
fn trim_old_sessions(dir: &Path) {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|e| e.path())
        .collect();
    if files.len() < MAX_SESSIONS {
        return;
    }
    // File names start with a timestamp, so lexicographic sort is enough.
    files.sort();
    let to_delete = files.len().saturating_sub(MAX_SESSIONS - 1);
    for path in files.iter().take(to_delete) {
        let _ = fs::remove_file(path);
    }
}

fn read_session_summaries(dir: &Path) -> Vec<Value> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let id = entry.path().file_stem()?.to_string_lossy().to_string();
            let samples = read_samples(&entry.path());
            Some(summarize_samples(&id, &samples))
        })
        .collect()
}

fn read_samples(path: &Path) -> Vec<Value> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect()
}

fn summarize_samples(id: &str, samples: &[Value]) -> Value {
    let mut summary = Summary {
        id: id.to_string(),
        ..Default::default()
    };
    for sample in samples {
        summary.ingest(sample);
    }
    summary.into_json(samples.len())
}

#[derive(Default)]
struct Summary {
    id: String,
    started_at: f64,
    ended_at: f64,
    car_key: String,
    class_name: String,
    drivetrain: String,
    cylinders: i64,
    max_speed: f64,
    max_rpm: f64,
    max_power: f64,
    max_torque: f64,
    max_boost: f64,
    max_abs_g: f64,
    max_lat_g: f64,
    max_pure_lat_g: f64,
    max_drift: f64,
    throttle_sum: f64,
    brake_sum: f64,
    moving_samples: usize,
    shift_count: usize,
    last_gear: i64,
    best_lap: f64,
    // Race / lap tracking
    total_samples: usize,
    race_on_samples: usize,
    finish_position: i64,
    max_lap_number: i64,
    lap_times: Vec<f64>,    // completed lap durations (s)
    lap_number_init: bool,  // have we seen the first lap number?
    last_lap_number: i64,   // last seen lap.number value
    last_lap_current: f64,  // lap.current from the previous sample
}

impl Summary {
    fn ingest(&mut self, sample: &Value) {
        let at = get_f64(sample, &["at"]);
        if self.started_at == 0.0 {
            self.started_at = at;
            self.car_key = sample
                .pointer("/car/key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.class_name = sample
                .pointer("/car/class")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            self.drivetrain = sample
                .pointer("/car/drivetrain")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            self.cylinders = get_f64(sample, &["car", "cylinders"]) as i64;
        }
        self.ended_at = at;
        self.max_speed = self.max_speed.max(get_f64(sample, &["speed"]));
        self.max_rpm = self.max_rpm.max(get_f64(sample, &["rpm"]));
        self.max_power = self.max_power.max(get_f64(sample, &["power"]));
        self.max_torque = self.max_torque.max(get_f64(sample, &["torque"]));
        self.max_boost = self.max_boost.max(get_f64(sample, &["boost"]));
        self.max_abs_g = self.max_abs_g.max(
            get_f64(sample, &["gLat"])
                .abs()
                .max(get_f64(sample, &["gLong"]).abs()),
        );
        let lat_g = get_f64(sample, &["gLat"]).abs();
        self.max_lat_g = self.max_lat_g.max(lat_g);
        let long_g = get_f64(sample, &["gLong"]).abs();
        let accel = get_f64(sample, &["accel"]);
        let brake = get_f64(sample, &["brake"]);
        let speed = get_f64(sample, &["speed"]);
        if speed > 20.0 && long_g <= 0.25 && accel <= 0.2 && brake <= 0.2 {
            self.max_pure_lat_g = self.max_pure_lat_g.max(lat_g);
        }
        self.max_drift = self.max_drift.max(get_f64(sample, &["drift"]).abs());
        if get_f64(sample, &["speed"]) > 5.0 {
            self.throttle_sum += get_f64(sample, &["accel"]);
            self.brake_sum += get_f64(sample, &["brake"]);
            self.moving_samples += 1;
        }
        let gear = get_f64(sample, &["gear"]) as i64;
        if self.last_gear > 0 && gear > self.last_gear {
            self.shift_count += 1;
        }
        if gear > 0 {
            self.last_gear = gear;
        }
        let best_lap = get_f64(sample, &["lap", "best"]);
        if best_lap > 0.0 && (self.best_lap == 0.0 || best_lap < self.best_lap) {
            self.best_lap = best_lap;
        }

        // ── Race / lap tracking ─────────────────────────────────────────────
        self.total_samples += 1;
        if sample.get("raceOn").and_then(Value::as_bool).unwrap_or(false) {
            self.race_on_samples += 1;
        }
        let lap_number  = get_f64(sample, &["lap", "number"]) as i64;
        let lap_current = get_f64(sample, &["lap", "current"]);
        let lap_pos     = get_f64(sample, &["lap", "position"]) as i64;

        if !self.lap_number_init {
            self.last_lap_number = lap_number;
            self.lap_number_init = true;
        } else if lap_number > self.last_lap_number && self.last_lap_current > 0.5 {
            // A lap just completed — the previous sample's `lap.current` is its duration.
            self.lap_times.push(self.last_lap_current);
            self.last_lap_number = lap_number;
        }
        if lap_number > self.max_lap_number {
            self.max_lap_number = lap_number;
        }
        if lap_pos > 0 {
            self.finish_position = lap_pos;
        }
        self.last_lap_current = lap_current;
    }

    fn into_json(self, samples: usize) -> Value {
        let moving = self.moving_samples.max(1) as f64;
        // A session counts as a "race" when the majority of samples had raceOn=true.
        let is_race = self.total_samples > 0
            && self.race_on_samples * 2 >= self.total_samples;
        // totalLaps = highest lap number seen + 1 (Forza numbers laps 0-based).
        let total_laps = if self.max_lap_number > 0 {
            self.max_lap_number + 1
        } else {
            0
        };
        json!({
            "id": self.id,
            "startedAt": self.started_at,
            "endedAt": self.ended_at,
            "duration": (self.ended_at - self.started_at).max(0.0),
            "samples": samples,
            "carKey": self.car_key,
            "class": self.class_name,
            "drivetrain": self.drivetrain,
            "cylinders": self.cylinders,
            "maxSpeed": self.max_speed,
            "maxRpm": self.max_rpm,
            "maxPower": self.max_power,
            "maxTorque": self.max_torque,
            "maxBoost": self.max_boost,
            "maxAbsG": self.max_abs_g,
            "maxLatG": self.max_lat_g,
            "maxPureLatG": self.max_pure_lat_g,
            "maxDrift": self.max_drift,
            "avgThrottle": self.throttle_sum / moving,
            "avgBrake": self.brake_sum / moving,
            "shiftCount": self.shift_count,
            "bestLap": self.best_lap,
            // Race data
            "isRace": is_race,
            "finishPosition": self.finish_position,
            "totalLaps": total_laps,
            "lapTimes": self.lap_times,
        })
    }
}

fn downsample_series(samples: &[Value], max_points: usize) -> Vec<Value> {
    if samples.len() <= max_points {
        return samples.to_vec();
    }
    let step = samples.len() as f64 / max_points as f64;
    let mut out = Vec::with_capacity(max_points);
    let mut index = 0.0;
    while (index as usize) < samples.len() && out.len() < max_points {
        out.push(samples[index as usize].clone());
        index += step;
    }
    out
}

#[derive(Default)]
struct CarStats {
    sessions: usize,
    class_name: Option<String>,
    drivetrain: Option<String>,
    cylinders: Option<i64>,
    max_speed: f64,
    max_power: f64,
}

fn car_stats_from_sessions(dir: &Path) -> HashMap<String, CarStats> {
    let mut stats = HashMap::new();
    for summary in read_session_summaries(dir) {
        let key = summary
            .get("carKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key.is_empty() {
            continue;
        }
        let entry = stats.entry(key).or_insert_with(CarStats::default);
        entry.sessions += 1;
        entry.class_name = summary
            .get("class")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        entry.drivetrain = summary
            .get("drivetrain")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        entry.cylinders = Some(get_f64(&summary, &["cylinders"]) as i64);
        entry.max_speed = entry.max_speed.max(get_f64(&summary, &["maxSpeed"]));
        entry.max_power = entry.max_power.max(get_f64(&summary, &["maxPower"]));
    }
    stats
}

/// Returns a CSV string for the full (non-downsampled) sample series of a session,
/// or `None` if the session does not exist.
pub(crate) fn session_csv(dir: &Path, id: &str) -> Option<String> {
    let path = safe_session_path(dir, id)?;
    let samples = read_samples(&path);
    if samples.is_empty() {
        return None;
    }
    // Pre-allocate roughly 120 bytes per row.
    let mut out = String::with_capacity(256 + samples.len() * 120);
    out.push_str(
        "t,speed_kmh,rpm,gear,accel,brake,steer,\
         g_lat,g_long,drift_deg,slip,\
         power_hp,torque_nm,boost,\
         pos_x,pos_z,\
         race_on,lap_num,lap_current_s,lap_best_s,lap_pos\n",
    );
    for s in &samples {
        let race_on = if s.get("raceOn").and_then(Value::as_bool).unwrap_or(false) {
            1u8
        } else {
            0u8
        };
        out.push_str(&format!(
            "{:.3},{:.1},{:.0},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{:.1},{:.4},{:.2},{:.2},{},{},{:.3},{:.3},{}\n",
            get_f64(s, &["t"]),
            get_f64(s, &["speed"]),
            get_f64(s, &["rpm"]),
            get_f64(s, &["gear"]) as i64,
            get_f64(s, &["accel"]),
            get_f64(s, &["brake"]),
            get_f64(s, &["steer"]),
            get_f64(s, &["gLat"]),
            get_f64(s, &["gLong"]),
            get_f64(s, &["drift"]),
            get_f64(s, &["slip"]),
            get_f64(s, &["power"]),
            get_f64(s, &["torque"]),
            get_f64(s, &["boost"]),
            get_f64(s, &["position", "x"]),
            get_f64(s, &["position", "z"]),
            race_on,
            get_f64(s, &["lap", "number"]) as i64,
            get_f64(s, &["lap", "current"]),
            get_f64(s, &["lap", "best"]),
            get_f64(s, &["lap", "position"]) as i64,
        ));
    }
    Some(out)
}

fn safe_session_path(dir: &Path, id: &str) -> Option<PathBuf> {
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    let path = dir.join(format!("{id}.jsonl"));
    path.exists().then_some(path)
}

fn number_field(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

fn timestamp_id(at: f64) -> String {
    format!("{}", at.max(now_seconds()) as u64)
}
