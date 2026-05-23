use serde_json::{Value, json};

use crate::util::{clamp, now_seconds};

// Forza data out packet layout.
// The "sled" block (physics, engine, controls) always starts at byte 0.
// The "dash" block (speed, power, temps, lap) sits at a variable offset
// depending on which game / format version sent the packet.
const DASH_OFFSET_STANDARD: usize = 232; // FH4 / FM7 format
const DASH_OFFSET_EXTENDED: usize = 244; // FH5 / FM8 format (12-byte type header prepended)
const EXTENDED_PACKET_LEN: usize = 324;  // minimum length for the extended format
const EXTENDED_TYPE_OFFSET: usize = 232; // non-zero i32 here signals the extended format

pub(crate) fn parse_forza_dash(packet: &[u8]) -> Value {
    let dash = if packet.len() >= EXTENDED_PACKET_LEN && i32_at(packet, EXTENDED_TYPE_OFFSET, 0) != 0 {
        DASH_OFFSET_EXTENDED
    } else {
        DASH_OFFSET_STANDARD
    };
    let speed_ms = f32_at(packet, dash + 12, 0.0) as f64;
    let rpm = f32_at(packet, 16, 0.0) as f64;
    let max_rpm = (f32_at(packet, 8, 1.0) as f64).max(1.0);
    let idle_rpm = f32_at(packet, 12, 0.0) as f64;
    let power_w = f32_at(packet, dash + 16, 0.0) as f64;
    let torque_nm = f32_at(packet, dash + 20, 0.0) as f64;
    let car_class = i32_at(packet, 216, -1);
    let drivetrain = i32_at(packet, 224, -1);
    let race_on = i32_at(packet, 0, 0) != 0;
    let velocity_x = f32_at(packet, 32, 0.0) as f64;
    let velocity_y = f32_at(packet, 36, 0.0) as f64;
    let velocity_z = f32_at(packet, 40, 0.0) as f64;
    let yaw = f32_at(packet, 56, 0.0) as f64;
    let drift_angle = velocity_x.atan2(velocity_z).to_degrees();
    json!({
        "receivedAt": now_seconds(),
        "packetBytes": packet.len(),
        "raceOn": race_on,
        "timestampMs": u32_at(packet, 4, 0),
        "speed": { "ms": speed_ms, "kmh": speed_ms * 3.6, "mph": speed_ms * 2.23694 },
        "engine": {
            "rpm": rpm, "maxRpm": max_rpm, "idleRpm": idle_rpm,
            "rpmRatio": clamp(rpm / max_rpm, 0.0, 1.2),
            "powerHp": power_w * 0.00134102,
            "powerKw": power_w / 1000.0,
            "torqueNm": torque_nm
        },
        "motion": {
            "accelX": f32_at(packet, 20, 0.0), "accelY": f32_at(packet, 24, 0.0), "accelZ": f32_at(packet, 28, 0.0),
            "velocityX": velocity_x, "velocityY": velocity_y, "velocityZ": velocity_z, "yaw": yaw,
            "pitch": f32_at(packet, 60, 0.0), "roll": f32_at(packet, 64, 0.0),
            "driftAngleDeg": if speed_ms > 2.2 { drift_angle } else { 0.0 },
            "gLat": f32_at(packet, 20, 0.0) as f64 / 9.80665,
            "gLong": f32_at(packet, 28, 0.0) as f64 / 9.80665,
            "gVert": f32_at(packet, 24, 0.0) as f64 / 9.80665
        },
        "car": {
            "ordinal": i32_at(packet, 212, -1),
            "class": car_class_name(car_class),
            "classId": car_class,
            "performanceIndex": i32_at(packet, 220, 0),
            "drivetrain": drivetrain_name(drivetrain),
            "drivetrainId": drivetrain,
            "cylinders": i32_at(packet, 228, 0),
            "typeId": if dash == DASH_OFFSET_EXTENDED { i32_at(packet, EXTENDED_TYPE_OFFSET, 0) } else { 0 }
        },
        "tireSlipAngleDeg": {
            "fl": (f32_at(packet, 164, 0.0) as f64).to_degrees(), "fr": (f32_at(packet, 168, 0.0) as f64).to_degrees(),
            "rl": (f32_at(packet, 172, 0.0) as f64).to_degrees(), "rr": (f32_at(packet, 176, 0.0) as f64).to_degrees()
        },
        "tireSlipRatio": {
            "fl": f32_at(packet, 84, 0.0), "fr": f32_at(packet, 88, 0.0),
            "rl": f32_at(packet, 92, 0.0), "rr": f32_at(packet, 96, 0.0)
        },
        "tireCombinedSlip": {
            "fl": f32_at(packet, 180, 0.0), "fr": f32_at(packet, 184, 0.0),
            "rl": f32_at(packet, 188, 0.0), "rr": f32_at(packet, 192, 0.0)
        },
        "tireTempC": {
            "fl": f32_at(packet, dash + 24, 0.0), "fr": f32_at(packet, dash + 28, 0.0),
            "rl": f32_at(packet, dash + 32, 0.0), "rr": f32_at(packet, dash + 36, 0.0)
        },
        "boost": f32_at(packet, dash + 40, 0.0),
        "fuel": f32_at(packet, dash + 44, 0.0),
        "lap": {
            "distance": f32_at(packet, dash + 48, 0.0),
            "best": f32_at(packet, dash + 52, 0.0),
            "last": f32_at(packet, dash + 56, 0.0),
            "current": f32_at(packet, dash + 60, 0.0),
            "raceTime": f32_at(packet, dash + 64, 0.0),
            "number": u16_at(packet, dash + 68, 0),
            "position": u8_at(packet, dash + 70, 0)
        },
        "controls": {
            "accel": u8_at(packet, dash + 71, 0) as f64 / 255.0,
            "brake": u8_at(packet, dash + 72, 0) as f64 / 255.0,
            "clutch": u8_at(packet, dash + 73, 0) as f64 / 255.0,
            "handbrake": u8_at(packet, dash + 74, 0) as f64 / 255.0,
            "gear": u8_at(packet, dash + 75, 0),
            "steer": s8_at(packet, dash + 76, 0) as f64 / 127.0
        }
    })
}

fn car_class_name(id: i32) -> &'static str {
    match id {
        0 => "D",
        1 => "C",
        2 => "B",
        3 => "A",
        4 => "S1",
        5 => "S2",
        6 => "X",
        _ => "-",
    }
}

fn drivetrain_name(id: i32) -> &'static str {
    match id {
        0 => "FWD",
        1 => "RWD",
        2 => "AWD",
        _ => "-",
    }
}

pub(crate) fn f32_at(packet: &[u8], offset: usize, default: f32) -> f32 {
    if packet.len() < offset + 4 {
        return default;
    }
    let value = f32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap());
    if value.is_finite() { value } else { default }
}

pub(crate) fn i32_at(packet: &[u8], offset: usize, default: i32) -> i32 {
    if packet.len() < offset + 4 {
        default
    } else {
        i32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap())
    }
}

fn u32_at(packet: &[u8], offset: usize, default: u32) -> u32 {
    if packet.len() < offset + 4 {
        default
    } else {
        u32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap())
    }
}

fn u16_at(packet: &[u8], offset: usize, default: u16) -> u16 {
    if packet.len() < offset + 2 {
        default
    } else {
        u16::from_le_bytes(packet[offset..offset + 2].try_into().unwrap())
    }
}

fn u8_at(packet: &[u8], offset: usize, default: u8) -> u8 {
    packet.get(offset).copied().unwrap_or(default)
}

fn s8_at(packet: &[u8], offset: usize, default: i8) -> i8 {
    packet
        .get(offset)
        .map(|byte| *byte as i8)
        .unwrap_or(default)
}
