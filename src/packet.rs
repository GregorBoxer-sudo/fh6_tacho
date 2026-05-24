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
    let dash = detect_dash_offset(packet);
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
    let position_x = f32_at(packet, dash, 0.0) as f64;
    let position_y = f32_at(packet, dash + 4, 0.0) as f64;
    let position_z = f32_at(packet, dash + 8, 0.0) as f64;
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
        "position": {
            "x": position_x,
            "y": position_y,
            "z": position_z
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
        // These are normalised slip-angle values (same scale as tireCombinedSlip),
        // NOT radians. Never apply .to_degrees() here.
        "tireSlipAngleDeg": {
            "fl": f32_at(packet, 164, 0.0) as f64, "fr": f32_at(packet, 168, 0.0) as f64,
            "rl": f32_at(packet, 172, 0.0) as f64, "rr": f32_at(packet, 176, 0.0) as f64
        },
        "tireSlipRatio": {
            "fl": f32_at(packet, 84, 0.0), "fr": f32_at(packet, 88, 0.0),
            "rl": f32_at(packet, 92, 0.0), "rr": f32_at(packet, 96, 0.0)
        },
        "tireCombinedSlip": {
            "fl": f32_at(packet, 180, 0.0), "fr": f32_at(packet, 184, 0.0),
            "rl": f32_at(packet, 188, 0.0), "rr": f32_at(packet, 192, 0.0)
        },
        // Forza sends tyre temperatures in Fahrenheit; convert to Celsius on read.
        "tireTempC": {
            "fl": fahrenheit_to_celsius(f32_at(packet, dash + 24, 0.0)),
            "fr": fahrenheit_to_celsius(f32_at(packet, dash + 28, 0.0)),
            "rl": fahrenheit_to_celsius(f32_at(packet, dash + 32, 0.0)),
            "rr": fahrenheit_to_celsius(f32_at(packet, dash + 36, 0.0))
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

fn detect_dash_offset(packet: &[u8]) -> usize {
    if packet.len() < DASH_OFFSET_STANDARD + 77 {
        return DASH_OFFSET_STANDARD;
    }
    if packet.len() < DASH_OFFSET_EXTENDED + 77 {
        return DASH_OFFSET_STANDARD;
    }

    // Prefer a data-driven choice between both known offsets. This is more
    // robust than relying solely on the optional type field at byte 232.
    let score = |dash: usize| -> i32 {
        let speed_ms = f32_at(packet, dash + 12, 0.0) as f64;
        let power_w = f32_at(packet, dash + 16, 0.0) as f64;
        let lap_current = f32_at(packet, dash + 60, 0.0) as f64;
        let gear = u8_at(packet, dash + 75, 0);
        let px = f32_at(packet, dash, 0.0) as f64;
        let py = f32_at(packet, dash + 4, 0.0) as f64;
        let pz = f32_at(packet, dash + 8, 0.0) as f64;
        let mut s = 0;
        if speed_ms.is_finite() {
            if (0.0..=220.0).contains(&speed_ms) {
                s += 4;
            } else if (-20.0..=260.0).contains(&speed_ms) {
                s += 2;
            } else {
                s -= 4;
            }
        }
        if power_w.is_finite() && power_w.abs() <= 2_500_000.0 {
            s += 1;
        } else {
            s -= 2;
        }
        if lap_current.is_finite() && (0.0..=50_000.0).contains(&lap_current) {
            s += 1;
        }
        if gear <= 12 || gear == 255 {
            s += 2;
        } else {
            s -= 2;
        }
        if px.is_finite() && py.is_finite() && pz.is_finite() {
            if px.abs() <= 200_000.0 && py.abs() <= 50_000.0 && pz.abs() <= 200_000.0 {
                s += 2;
            } else {
                s -= 2;
            }
        }
        s
    };

    let standard_score = score(DASH_OFFSET_STANDARD);
    let extended_score = score(DASH_OFFSET_EXTENDED);
    if extended_score > standard_score {
        return DASH_OFFSET_EXTENDED;
    }
    if standard_score > extended_score {
        return DASH_OFFSET_STANDARD;
    }

    // Tie-breaker: keep the old heuristic.
    if packet.len() >= EXTENDED_PACKET_LEN && i32_at(packet, EXTENDED_TYPE_OFFSET, 0) != 0 {
        DASH_OFFSET_EXTENDED
    } else {
        DASH_OFFSET_STANDARD
    }
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

fn fahrenheit_to_celsius(f: f32) -> f64 {
    ((f - 32.0) * 5.0 / 9.0) as f64
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid Forza standard-format packet (232 + 77 bytes).
    /// Only the fields explicitly set carry meaningful values; everything else is zero.
    fn make_standard_packet(
        race_on: bool,
        max_rpm: f32,
        idle_rpm: f32,
        rpm: f32,
        ordinal: i32,
        cylinders: i32,
        speed_ms: f32,
        power_w: f32,
        gear: u8,
        accel: u8,
        brake: u8,
    ) -> Vec<u8> {
        let mut p = vec![0u8; 312];
        // Sled block (starts at byte 0)
        p[0..4].copy_from_slice(&(if race_on { 1i32 } else { 0i32 }).to_le_bytes());
        p[8..12].copy_from_slice(&max_rpm.to_le_bytes());
        p[12..16].copy_from_slice(&idle_rpm.to_le_bytes());
        p[16..20].copy_from_slice(&rpm.to_le_bytes());
        p[212..216].copy_from_slice(&ordinal.to_le_bytes());
        p[228..232].copy_from_slice(&cylinders.to_le_bytes());
        // Dash block starts at DASH_OFFSET_STANDARD = 232
        p[244..248].copy_from_slice(&speed_ms.to_le_bytes());  // +12
        p[248..252].copy_from_slice(&power_w.to_le_bytes());   // +16
        // gear at 232+75=307, accel at 232+71=303, brake at 232+72=304
        p[303] = accel;
        p[304] = brake;
        p[307] = gear;
        p
    }

    #[test]
    fn parse_standard_packet_basic_fields() {
        let p = make_standard_packet(
            true, 8000.0, 750.0, 6500.0,
            42, 4, 50.0, 200_000.0, 3, 255, 0,
        );
        let v = parse_forza_dash(&p);

        assert_eq!(v["raceOn"], serde_json::json!(true));

        let rpm = v["engine"]["rpm"].as_f64().unwrap();
        assert!((rpm - 6500.0).abs() < 1.0, "rpm={rpm}");

        let max_rpm = v["engine"]["maxRpm"].as_f64().unwrap();
        assert!((max_rpm - 8000.0).abs() < 1.0, "max_rpm={max_rpm}");

        let kmh = v["speed"]["kmh"].as_f64().unwrap();
        assert!((kmh - 50.0 * 3.6).abs() < 0.1, "kmh={kmh}");

        let gear = v["controls"]["gear"].as_u64().unwrap();
        assert_eq!(gear, 3);

        let accel = v["controls"]["accel"].as_f64().unwrap();
        assert!((accel - 1.0).abs() < 0.01, "accel={accel}");
    }

    #[test]
    fn parse_too_short_packet_returns_default() {
        // f32_at / i32_at must not panic on short slices; they return the default.
        let p = vec![0u8; 20];
        // Offset 17 needs bytes 17..21, but the slice is only 20 bytes → default
        let rpm = f32_at(&p, 17, -1.0);
        assert_eq!(rpm, -1.0);
        // Offset 16 fits exactly (bytes 16..20) → reads the zero bytes as 0.0
        let zero = f32_at(&p, 16, -1.0);
        assert_eq!(zero, 0.0);
    }

    #[test]
    fn f32_at_returns_default_for_nan_and_inf() {
        let mut p = vec![0u8; 8];
        // Write f32::NAN at offset 0
        p[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(f32_at(&p, 0, 99.0), 99.0);
        // Write f32::INFINITY at offset 4
        p[4..8].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert_eq!(f32_at(&p, 4, 99.0), 99.0);
    }

    #[test]
    fn parse_race_off_sets_flag_false() {
        let p = make_standard_packet(
            false, 8000.0, 750.0, 0.0,
            1, 4, 0.0, 0.0, 0, 0, 0,
        );
        let v = parse_forza_dash(&p);
        assert_eq!(v["raceOn"], serde_json::json!(false));
    }

    #[test]
    fn extended_format_detected_by_length_and_type_field() {
        // If the packet is >= 324 bytes and byte 232 is non-zero, it uses the
        // extended dash offset (244 instead of 232). Constructing a minimal
        // extended packet and verifying the speed field is read from +12 of 244.
        let mut p = vec![0u8; 324];
        // Signal extended format: non-zero i32 at offset 232
        p[232..236].copy_from_slice(&1i32.to_le_bytes());
        // speed_ms at extended dash offset 244 + 12 = 256
        p[256..260].copy_from_slice(&30.0f32.to_le_bytes());
        let v = parse_forza_dash(&p);
        let kmh = v["speed"]["kmh"].as_f64().unwrap();
        assert!((kmh - 30.0 * 3.6).abs() < 0.1, "kmh={kmh}");
    }
}
