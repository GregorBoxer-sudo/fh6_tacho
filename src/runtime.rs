use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, Uri, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::StreamExt;
use rust_embed::RustEmbed;
use serde_json::{Value, json};
use std::{fs, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::UdpSocket, time};
use tokio_stream::wrappers::BroadcastStream;

/// All files from `static/` are embedded at compile time.
/// The binary is fully self-contained — no `static/` folder needed at runtime.
#[derive(RustEmbed)]
#[folder = "static/"]
struct StaticAssets;

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if path == "map.html" {
        return Response::builder()
            .status(StatusCode::PERMANENT_REDIRECT)
            .header(header::LOCATION, "/analytics.html")
            .body(Body::empty())
            .unwrap();
    }
    match StaticAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(content.data))
                .unwrap()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

use crate::util::{get_f64, now_seconds};
use crate::{
    analytics::{
        TelemetryRecorder, car_browser, car_power_curve, list_sessions, session_detail,
        session_track,
    },
    config::Args,
    logging::PacketInspector,
    packet::parse_forza_dash,
    shift::{PowerCurveStore, enrich_shift_data},
    telemetry::{AppState, TelemetryHub},
};

pub(crate) async fn udp_loop(
    hub: Arc<TelemetryHub>,
    args: Args,
    power_curves: Arc<PowerCurveStore>,
    recorder: Arc<TelemetryRecorder>,
) -> Result<()> {
    let addr = format!("{}:{}", args.udp_host, args.udp_port);
    let sock = UdpSocket::bind(&addr)
        .await
        .with_context(|| format!("binding UDP {addr}"))?;
    let inspector = if args.inspect {
        Some(PacketInspector::new(&args.inspect_dir, args.inspect_every)?)
    } else {
        None
    };
    println!("UDP telemetry listening on {addr}");
    let mut buf = vec![0u8; 2048];
    let mut position_fallback = PositionFallback::new();
    loop {
        let (len, _) = sock.recv_from(&mut buf).await?;
        if len >= 232 {
            let mut decoded = parse_forza_dash(&buf[..len]);
            enrich_shift_data(&mut decoded, Some(&power_curves));
            position_fallback.apply(&mut decoded);
            recorder.record(&decoded);
            if let Some(inspector) = &inspector {
                inspector.inspect(&buf[..len], &decoded);
            }
            hub.publish(&decoded);
        } else if let Some(inspector) = &inspector {
            inspector.inspect(
                &buf[..len],
                &json!({ "receivedAt": now_seconds(), "packetBytes": len, "tooShort": true }),
            );
        }
    }
}

struct PositionFallback {
    x: f64,
    z: f64,
    last_at: Option<f64>,
    initialized: bool,
}

impl PositionFallback {
    fn new() -> Self {
        Self {
            x: 0.0,
            z: 0.0,
            last_at: None,
            initialized: false,
        }
    }

    fn apply(&mut self, payload: &mut Value) {
        let at = get_f64(payload, &["receivedAt"]);
        if !at.is_finite() || at <= 0.0 {
            return;
        }

        let raw_x = get_f64(payload, &["position", "x"]);
        let raw_z = get_f64(payload, &["position", "z"]);
        let raw_y = get_f64(payload, &["position", "y"]);
        let speed_ms = get_f64(payload, &["speed", "ms"]).max(0.0);
        let yaw = get_f64(payload, &["motion", "yaw"]);
        let vel_x = get_f64(payload, &["motion", "velocityX"]);
        let vel_z = get_f64(payload, &["motion", "velocityZ"]);

        let dt = self
            .last_at
            .map(|prev| (at - prev).clamp(0.0, 0.35))
            .unwrap_or(0.0);
        self.last_at = Some(at);

        let raw_is_nonzero = raw_x.abs() > f64::EPSILON || raw_z.abs() > f64::EPSILON;
        let raw_is_valid =
            raw_x.is_finite() && raw_z.is_finite() && (raw_is_nonzero || speed_ms < 0.8);

        if raw_is_valid {
            self.x = raw_x;
            self.z = raw_z;
            self.initialized = true;
            payload["position"] = json!({
                "x": self.x,
                "y": raw_y,
                "z": self.z,
                "source": "raw"
            });
            return;
        }

        if !self.initialized {
            self.x = 0.0;
            self.z = 0.0;
            self.initialized = true;
        }

        if dt > 0.0 {
            let (dx, dz) = if speed_ms > 0.5 && yaw.is_finite() {
                // Use heading + speed when available.
                (yaw.sin() * speed_ms * dt, yaw.cos() * speed_ms * dt)
            } else if yaw.is_finite() {
                // Fallback to rotating local velocity into world space by yaw.
                (
                    (yaw.cos() * vel_x + yaw.sin() * vel_z) * dt,
                    (-yaw.sin() * vel_x + yaw.cos() * vel_z) * dt,
                )
            } else {
                (0.0, vel_z * dt)
            };
            self.x += dx;
            self.z += dz;
        }

        payload["position"] = json!({
            "x": self.x,
            "y": raw_y,
            "z": self.z,
            "source": "integrated"
        });
    }
}

pub(crate) async fn demo_loop(
    hub: Arc<TelemetryHub>,
    power_curves: Arc<PowerCurveStore>,
    recorder: Arc<TelemetryRecorder>,
) {
    let mut interval = time::interval(Duration::from_millis(1000 / 60));
    let start = std::time::Instant::now();
    loop {
        interval.tick().await;
        let t = start.elapsed().as_secs_f64();
        let speed = (110.0 + (t * 0.8).sin() * 95.0).max(0.0);
        let rpm_ratio = 0.25 + (t * 1.7).sin().abs() * 0.75;
        let rpm = 900.0 + rpm_ratio * 7200.0;
        let gear = ((speed / 45.0) as i64 + 1).clamp(1, 7);
        let map_t = t * 0.18;
        let pos_x = map_t.cos() * 840.0;
        let pos_z = map_t.sin() * 520.0;
        let mut payload = json!({
            "receivedAt": now_seconds(), "packetBytes": 324, "raceOn": true, "timestampMs": (t * 1000.0) as u64,
            "position": { "x": pos_x, "y": 0.0, "z": pos_z },
            "speed": { "ms": speed / 3.6, "kmh": speed, "mph": speed / 1.60934 },
            "engine": { "rpm": rpm, "maxRpm": 8500.0, "idleRpm": 900.0, "rpmRatio": rpm / 8500.0,
                "powerHp": 420.0 + (t * 2.0).sin() * 80.0, "powerKw": 315.0, "torqueNm": 620.0 + (t * 2.2).sin() * 120.0 },
            "motion": { "accelX": (t * 1.25).sin() * 9.80665 * 1.15, "accelY": (t * 0.9).sin() * 9.80665 * 0.75,
                "accelZ": 9.80665 + (t * 2.1).sin() * 1.4, "velocityX": (t * 0.8).sin() * 4.5, "velocityY": 0.0,
                "velocityZ": speed / 3.6, "yaw": (t * 0.45).sin() * 0.35, "pitch": 0.0, "roll": (t * 1.1).sin() * 0.08,
                "driftAngleDeg": (t * 0.85).sin() * 28.0, "gLat": (t * 1.25).sin() * 1.15,
                "gLong": (t * 0.9).sin() * 0.75, "gVert": 1.0 + (t * 2.1).sin() * 0.14 },
            "car": { "ordinal": 999999, "class": "S1", "classId": 4, "performanceIndex": 900, "drivetrain": "AWD", "drivetrainId": 2, "cylinders": 6 },
            "tireSlipAngleDeg": { "fl": (t * 3.1).sin() * 8.0, "fr": (t * 3.0).sin() * 8.0, "rl": (t * 3.2).sin() * 12.0, "rr": (t * 3.3).sin() * 12.0 },
            "tireSlipRatio": { "fl": 0.05 + (t * 1.6).sin().max(0.0) * 0.4, "fr": 0.05 + (t * 1.5 + 0.3).sin().max(0.0) * 0.4,
                "rl": 0.08 + (t * 1.8 + 0.8).sin().max(0.0) * 3.5, "rr": 0.08 + (t * 1.7 + 1.1).sin().max(0.0) * 3.5 },
            "tireCombinedSlip": { "fl": 0.2 + (t * 1.7).sin().max(0.0) * 0.65, "fr": 0.18 + (t * 1.6 + 0.4).sin().max(0.0) * 0.65,
                "rl": 0.31 + (t * 1.9 + 1.0).sin().max(0.0) * 0.9, "rr": 0.29 + (t * 1.8 + 1.4).sin().max(0.0) * 0.9 },
            "tireTempC": { "fl": 83, "fr": 84, "rl": 88, "rr": 89 },
            "boost": ((t * 1.4).sin() * 1.2).max(0.0), "fuel": 0.72,
            "lap": { "best": 0, "last": 0, "current": t, "raceTime": t, "number": 1 },
            "controls": { "accel": (t * 0.9).sin().max(0.0), "brake": (-(t * 0.7).sin()).max(0.0) * 0.7,
                "clutch": 0, "handbrake": 0, "gear": gear, "steer": (t * 1.3).sin() * 0.65 }
        });
        enrich_shift_data(&mut payload, Some(&power_curves));
        recorder.record(&payload);
        hub.publish(&payload);
    }
}

async fn status(State(state): State<AppState>) -> Json<Value> {
    let mut status = state.hub.status();
    if let Some(object) = status.as_object_mut() {
        object.insert("debugMode".to_string(), json!(state.debug_mode));
    }
    Json(status)
}

async fn events(State(state): State<AppState>) -> impl IntoResponse {
    let stream = BroadcastStream::new(state.hub.tx.subscribe()).filter_map(|msg| async move {
        msg.ok().map(|data| {
            Ok::<Event, std::convert::Infallible>(Event::default().event("telemetry").data(data))
        })
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keepalive"),
    )
}

async fn api_sessions(State(state): State<AppState>) -> Json<Value> {
    Json(list_sessions(&state.data_dir.join("drive_sessions")))
}

async fn api_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<Value> {
    Json(session_detail(&state.data_dir.join("drive_sessions"), &id))
}

async fn api_session_track(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<Value> {
    Json(session_track(
        &state.data_dir.join("drive_sessions"),
        &id,
        12000,
    ))
}

async fn api_cars(State(state): State<AppState>) -> Json<Value> {
    Json(car_browser(
        &state.data_dir.join("power_curves.json"),
        &state.data_dir.join("drive_sessions"),
    ))
}

async fn api_car_powercurve(
    State(state): State<AppState>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Json<Value> {
    Json(car_power_curve(
        &state.data_dir.join("power_curves.json"),
        &key,
    ))
}

fn default_calibration() -> Value {
    StaticAssets::get("map_calibration.default.json")
        .and_then(|asset| serde_json::from_slice::<Value>(&asset.data).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| {
            json!({
                "version": 1,
                "mapImage": "map.jpg",
                "worldFlipX": false,
                "worldFlipZ": false,
                "points": []
            })
        })
}

fn map_calibration_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("map_calibration.json")
}

async fn api_map_calibration(State(state): State<AppState>) -> Json<Value> {
    let path = map_calibration_path(&state.data_dir);
    let mut calibration = fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(default_calibration);
    if let Some(object) = calibration.as_object_mut() {
        object.insert("debugMode".to_string(), json!(state.debug_mode));
    }
    Json(calibration)
}

async fn api_save_map_calibration(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    if !state.debug_mode {
        return Json(json!({ "ok": false, "error": "debug_mode_required" }));
    }
    let Some(input) = payload.as_object() else {
        return Json(json!({ "ok": false, "error": "invalid_payload" }));
    };
    let mut points = Vec::new();
    for point in input
        .get("points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(256)
    {
        let world_x = point
            .pointer("/world/x")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let world_z = point
            .pointer("/world/z")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let pixel_x = point
            .pointer("/pixel/x")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let pixel_y = point
            .pointer("/pixel/y")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        if !(world_x.is_finite()
            && world_z.is_finite()
            && pixel_x.is_finite()
            && pixel_y.is_finite())
        {
            continue;
        }
        points.push(json!({
            "world": { "x": world_x, "z": world_z },
            "pixel": { "x": pixel_x, "y": pixel_y },
            "label": point.get("label").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    let map_image = input
        .get("mapImage")
        .and_then(Value::as_str)
        .unwrap_or("map.jpg");
    let world_flip_x = input
        .get("worldFlipX")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let world_flip_z = input
        .get("worldFlipZ")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let normalized = json!({
        "version": 1,
        "mapImage": map_image,
        "worldFlipX": world_flip_x,
        "worldFlipZ": world_flip_z,
        "points": points
    });
    let path = map_calibration_path(&state.data_dir);
    match serde_json::to_string_pretty(&normalized)
        .ok()
        .and_then(|raw| fs::write(path, raw).ok())
    {
        Some(()) => Json(json!({ "ok": true, "points": points.len() })),
        None => Json(json!({ "ok": false, "error": "write_failed" })),
    }
}

pub(crate) async fn run_http(data_dir: PathBuf, hub: Arc<TelemetryHub>, args: &Args) -> Result<()> {
    let state = AppState {
        hub,
        data_dir,
        debug_mode: args.debug,
    };
    let app = Router::new()
        .route("/events", get(events))
        .route("/api/status", get(status))
        .route("/api/analytics/sessions", get(api_sessions))
        .route("/api/analytics/sessions/{id}", get(api_session))
        .route("/api/analytics/sessions/{id}/track", get(api_session_track))
        .route("/api/analytics/cars", get(api_cars))
        .route(
            "/api/analytics/cars/{key}/powercurve",
            get(api_car_powercurve),
        )
        .route(
            "/api/map/calibration",
            get(api_map_calibration).post(api_save_map_calibration),
        )
        .fallback(static_handler)
        .with_state(state);
    let addr: SocketAddr = format!("{}:{}", args.http_host, args.http_port)
        .parse()
        .with_context(|| "invalid HTTP address")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
