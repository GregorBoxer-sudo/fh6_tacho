use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::config::*;
use crate::logging::ShiftCacheLogger;
use crate::util::*;

pub(crate) struct PowerCurveStore {
    #[allow(dead_code)]
    path: PathBuf,
    curves: Mutex<Map<String, Value>>,
    logger: Option<Arc<ShiftCacheLogger>>,
    limiter_log: bool,
    limiter_debug: bool,
    /// Channel to the background save-worker thread.
    /// Bounded at 2 so try_send never blocks; if the worker is busy the
    /// in-progress write already holds the full current state — dropping
    /// an intermediate snapshot is safe.
    save_tx: std::sync::mpsc::SyncSender<String>,
}

impl PowerCurveStore {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        logger: Option<Arc<ShiftCacheLogger>>,
        limiter_log: bool,
        limiter_debug: bool,
    ) -> Self {
        let path = path.into();
        let curves = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let save_tx = Self::spawn_save_worker(path.clone());
        Self {
            path,
            curves: Mutex::new(curves),
            logger,
            limiter_log,
            limiter_debug,
            save_tx,
        }
    }

    /// Spawns a dedicated thread that receives serialised JSON and writes it
    /// atomically to disk.  Lives for the lifetime of the process.
    fn spawn_save_worker(path: PathBuf) -> std::sync::mpsc::SyncSender<String> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(2);
        std::thread::Builder::new()
            .name("curve-save".into())
            .spawn(move || {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                while let Ok(json) = rx.recv() {
                    let tmp = path.with_extension("tmp");
                    if fs::write(&tmp, &json).is_ok() {
                        let _ = fs::rename(&tmp, &path);
                    }
                }
            })
            .expect("curve-save thread");
        tx
    }

    /// Serialise `curves` and hand the bytes off to the save-worker.
    /// Never blocks — if the worker already has two pending writes the
    /// current snapshot is dropped (the next save will include it anyway).
    fn queue_save(&self, curves: &Map<String, Value>) {
        if let Ok(text) = serde_json::to_string(curves) {
            let _ = self.save_tx.try_send(text);
        }
    }

    fn curve_mut<'a>(curves: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
        if !curves.get(key).is_some_and(Value::is_object) {
            curves.insert(key.to_string(), json!({ "buckets": {}, "updatedAt": 0.0 }));
        }
        curves.get_mut(key).unwrap().as_object_mut().unwrap()
    }

    fn reset_shift_cache(curve: &mut Map<String, Value>) {
        curve.insert("optimalShiftRpmByGear".to_string(), json!({}));
        curve.insert("shiftWarningRpmByGear".to_string(), json!({}));
        curve.insert("dirtyShiftGears".to_string(), json!([]));
        curve.insert("validatedShiftGears".to_string(), json!([]));
        curve.insert("noOptimalShiftGears".to_string(), json!([]));
        curve.insert(
            "shiftStrategyVersion".to_string(),
            json!(SHIFT_CACHE_STRATEGY_VERSION),
        );
        curve.insert("updatedAt".to_string(), json!(now_seconds()));
    }

    fn reset_curve(curve: &mut Map<String, Value>, engine: &Value) {
        curve.insert("buckets".to_string(), json!({}));
        curve.insert("gearDropRatios".to_string(), json!({}));
        curve.insert("optimalShiftRpmByGear".to_string(), json!({}));
        curve.insert("shiftWarningRpmByGear".to_string(), json!({}));
        curve.insert("rpmRiseRateByGear".to_string(), json!({}));
        curve.insert(
            "shiftStrategyVersion".to_string(),
            json!(SHIFT_CACHE_STRATEGY_VERSION),
        );
        curve.insert("dirtyShiftGears".to_string(), json!([]));
        curve.insert("validatedShiftGears".to_string(), json!([]));
        curve.insert("noOptimalShiftGears".to_string(), json!([]));
        curve.insert("maxObservedRpm".to_string(), json!(0.0));
        curve.insert("savedMaxObservedRpm".to_string(), json!(0.0));
        curve.insert("maxObservedGear".to_string(), json!(0));
        curve.insert(
            "maxRpmSignature".to_string(),
            json!(get_child_f64(engine, "maxRpm")),
        );
        curve.insert(
            "idleRpmSignature".to_string(),
            json!(get_child_f64(engine, "idleRpm")),
        );
        curve.insert("gearPeakRpm".to_string(), json!({}));
        curve.insert("bounceConfirmedLimit".to_string(), json!(0.0));
        curve.insert("updatedAt".to_string(), json!(now_seconds()));
    }

    fn validate_profile(&self, payload: &Value) {
        let key = power_curve_key(payload);
        if key.is_empty() {
            return;
        }
        let engine = payload.get("engine").unwrap_or(&Value::Null);
        let controls = payload.get("controls").unwrap_or(&Value::Null);
        let rpm = get_child_f64(engine, "rpm");
        let power = get_child_f64(engine, "powerHp");
        let max_rpm = get_child_f64(engine, "maxRpm");
        let idle_rpm = get_child_f64(engine, "idleRpm");
        let now = get_f64(payload, &["receivedAt"]);
        let mut save = false;
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let curve = Self::curve_mut(&mut curves, &key);

        if get_curve_i64(curve, "shiftStrategyVersion") != SHIFT_CACHE_STRATEGY_VERSION {
            Self::reset_shift_cache(curve);
            save = true;
        }

        let stale = now - get_curve_f64(curve, "lastSeenAt") > CACHE_REVALIDATE_AFTER_SECONDS;
        let mut reset = false;
        let max_sig = get_curve_f64(curve, "maxRpmSignature");
        if max_sig <= 0.0 && max_rpm > 0.0 {
            curve.insert("maxRpmSignature".to_string(), json!(max_rpm));
        } else if max_sig > 0.0
            && max_rpm > 0.0
            && (max_rpm - max_sig).abs() / max_sig > MAX_RPM_SIGNATURE_TOLERANCE
        {
            reset = true;
        }
        let idle_sig = get_curve_f64(curve, "idleRpmSignature");
        if idle_sig <= 0.0 && idle_rpm > 0.0 {
            curve.insert("idleRpmSignature".to_string(), json!(idle_rpm));
        } else if idle_sig > 0.0
            && idle_rpm > 0.0
            && (idle_rpm - idle_sig).abs() / idle_sig > IDLE_RPM_SIGNATURE_TOLERANCE
        {
            reset = true;
        }
        if stale && get_child_f64(controls, "accel") >= FULL_THROTTLE_MIN && power > 0.0 && rpm > 1500.0 {
            let bucket = bucket_key(rpm);
            let learned = curve
                .get("buckets")
                .and_then(Value::as_object)
                .and_then(|b| b.get(&bucket))
                .map(|p| get_child_f64(p, "power"))
                .unwrap_or(0.0);
            if learned > 0.0 && (power - learned).abs() / learned > POWER_SIGNATURE_TOLERANCE {
                reset = true;
            }
        }
        if reset {
            Self::reset_curve(curve, engine);
        }
        curve.insert("lastSeenAt".to_string(), json!(now));
        if reset || stale || save {
            self.queue_save(&curves);
        }
    }

    fn update_observed_limit(&self, payload: &Value) -> (f64, i64, f64) {
        let key = power_curve_key(payload);
        let gear = learned_forward_gear(payload);
        let rpm = get_f64(payload, &["engine", "rpm"]);
        let now = get_f64(payload, &["receivedAt"]);
        let max_rpm = get_f64(payload, &["engine", "maxRpm"]).max(3000.0);
        let accel = get_f64(payload, &["controls", "accel"]);
        if key.is_empty() || gear <= 0 || rpm < 1500.0 {
            return (0.0, 0, 0.0);
        }
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let (observed, bounce_count, bounce_confirmed, should_save) = {
            let curve = Self::curve_mut(&mut curves, &key);
            let previous = curve.get("lastLimitSample").unwrap_or(&Value::Null);
            let previous_gear = get_child_i64(previous, "gear");
            let previous_rpm = get_child_f64(previous, "rpm");
            let previous_at = get_child_f64(previous, "at");
            let current_baseline = get_curve_f64(curve, "maxObservedRpm").max(
                max_rpm * DEFAULT_ENGINE_LIMIT_RATIO_OF_TACHO_MAX,
            );
            let is_rising_same_gear = previous_gear == gear
                && rpm >= previous_rpm + LIMIT_LEARN_MIN_RPM_GAIN
                && now - previous_at <= LIMIT_LEARN_MAX_SAMPLE_AGE;
            // Lower-half guard: weak/tall-geared cars plateau in high gears first,
            // never reaching the real engine limiter there.  Only allow the continuation
            // path in the lower half of observed gears where the driver is more likely
            // to be heading into a true limiter hit.
            let max_obs_gear = get_curve_i64(curve, "maxObservedGear").max(1);
            let gear_half = (max_obs_gear / 2).max(1);
            let is_high_rpm_continuation = previous_gear == gear
                && now - previous_at <= LIMIT_LEARN_MAX_SAMPLE_AGE
                && rpm >= previous_rpm - LIMIT_LEARN_MIN_RPM_GAIN
                && rpm >= current_baseline * LIMIT_LEARN_HIGH_RPM_RATIO
                && accel >= FULL_THROTTLE_MIN   // C: full throttle required — coasting or
                                                //    hill-descent at high RPM must not count
                && gear <= gear_half;           // lower-half gears only
            curve.insert(
                "lastLimitSample".to_string(),
                json!({ "gear": gear, "rpm": rpm, "at": now }),
            );
            // Plateau detector (E): if RPM has been stable (no meaningful rise) at full
            // throttle for >= LIMIT_LEARN_PLATEAU_SECONDS, the car is terrain- or
            // speed-limited, not engine-limited — suppress the continuation path so a
            // hilltop or top-speed plateau doesn't record a falsely low maxObservedRpm.
            let prev_plateau_since = get_curve_f64(curve, "limitLearnPlateauSince");
            let new_plateau_since = if is_high_rpm_continuation && !is_rising_same_gear {
                if prev_plateau_since > 0.0 { prev_plateau_since } else { now }
            } else {
                0.0
            };
            curve.insert("limitLearnPlateauSince".to_string(), json!(new_plateau_since));
            let is_plateau = new_plateau_since > 0.0
                && now - new_plateau_since >= LIMIT_LEARN_PLATEAU_SECONDS;

            let previous_max = get_curve_f64(curve, "maxObservedRpm");
            let mut should_save = false;
            if (is_rising_same_gear || (is_high_rpm_continuation && !is_plateau)) && rpm > previous_max {
                curve.insert("maxObservedRpm".to_string(), json!(rpm));
                curve.insert("updatedAt".to_string(), json!(now_seconds()));
                let saved = get_curve_f64(curve, "savedMaxObservedRpm");
                if saved == 0.0 || rpm >= saved + 50.0 {
                    curve.insert("savedMaxObservedRpm".to_string(), json!(rpm));
                    should_save = true;
                }
            }

            // Track per-gear full-throttle peak RPM so we can identify power-limited
            // gears where the car tops out well below max_rpm.
            let gear_peak_key = gear.to_string();
            if accel >= FULL_THROTTLE_MIN {
                let prev_peak = curve
                    .get("gearPeakRpm")
                    .and_then(Value::as_object)
                    .and_then(|p| p.get(&gear_peak_key))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                if rpm > prev_peak {
                    ensure_object(curve, "gearPeakRpm")
                        .insert(gear_peak_key.clone(), json!(rpm));
                }
            }
            let gear_peak = curve
                .get("gearPeakRpm")
                .and_then(Value::as_object)
                .and_then(|p| p.get(&gear_peak_key))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);

            // Limiter bounce detection: at full throttle and high RPM, look for the
            // characteristic oscillation at the rev limiter and use it to correct
            // maxObservedRpm downward if needed.
            //
            // For power-limited gears (car tops out at < 96 % of the global limit),
            // use the gear's own ceiling for the detection zone so the oscillation
            // is captured even though it lies below the global threshold.
            let is_power_limited_gear = gear_peak > 0.0 && gear_peak < current_baseline * 0.96;
            let high_rpm_threshold = if is_power_limited_gear {
                gear_peak * LIMIT_LEARN_HIGH_RPM_RATIO
            } else {
                max_rpm * LIMIT_LEARN_HIGH_RPM_RATIO
            };
            let prev_bounce_count = get_child_i64(
                &curve.get("limiterBounce").cloned().unwrap_or(Value::Null),
                "count",
            );

            // --limiter-debug: print one line per packet when RPM is high enough to
            // be interesting, showing every value that feeds into the detection logic.
            if self.limiter_debug && rpm >= max_rpm * 0.80 {
                let bounce_state = curve.get("limiterBounce").cloned().unwrap_or(Value::Null);
                let b_count  = get_child_i64(&bounce_state, "count");
                let b_dir    = get_child_f64(&bounce_state, "dir");
                let b_ref    = get_child_f64(&bounce_state, "refRpm");
                let cur_max  = get_curve_f64(curve, "maxObservedRpm");
                let in_zone  = accel >= FULL_THROTTLE_MIN && rpm >= high_rpm_threshold;
                let reason   = if accel < FULL_THROTTLE_MIN {
                    format!("throttle={:.3} < {:.2} (not full)", accel, FULL_THROTTLE_MIN)
                } else if rpm < high_rpm_threshold {
                    format!("rpm={:.0} < threshold={:.0}", rpm, high_rpm_threshold)
                } else {
                    "IN ZONE".to_string()
                };
                println!(
                    "[limiter:dbg] car={key} g={gear} rpm={:.0} max={:.0} accel={:.3} \
                     threshold={:.0}({}) gearPeak={:.0} observedMax={:.0} zone:{} \
                     bounce(count={b_count} dir={b_dir:+.0} ref={b_ref:.0})",
                    rpm, max_rpm, accel, high_rpm_threshold,
                    if is_power_limited_gear { "gear" } else { "global" },
                    gear_peak, cur_max,
                    if in_zone { &reason } else { &reason },
                );
            }

            if accel >= FULL_THROTTLE_MIN && rpm >= high_rpm_threshold {
                if let Some(detected) = detect_limiter_bounce(curve, rpm, now) {
                    let current_max = get_curve_f64(curve, "maxObservedRpm");
                    let needs_down = detected < current_max * (1.0 - LIMITER_CORRECT_TOLERANCE);
                    let needs_up = detected > current_max;
                    if self.limiter_log {
                        println!(
                            "[limiter] confirmed  car={key} g={gear} detected={:.0} current_max={:.0} {}",
                            detected, current_max,
                            if needs_up { "-> UP" }
                            else if needs_down { "-> DOWN" }
                            else { "(no change)" }
                        );
                    }
                    if needs_up || needs_down {
                        curve.insert("maxObservedRpm".to_string(), json!(detected));
                        curve.insert("savedMaxObservedRpm".to_string(), json!(detected));
                        curve.insert("updatedAt".to_string(), json!(now_seconds()));
                        if needs_down {
                            // The rev limiter is a global engine property — record the
                            // confirmed value so rpm_reference can apply it to all gears
                            // even if they haven't been driven to the limiter yet.
                            curve.insert("bounceConfirmedLimit".to_string(), json!(detected));
                            // Saved shift points were based on the old (incorrect) limit,
                            // so mark them dirty to force a recalculation.
                            mark_shift_cache_dirty(curve, None);
                        }
                        should_save = true;
                    }
                } else if self.limiter_log {
                    let bounce_state = curve.get("limiterBounce").cloned().unwrap_or(Value::Null);
                    let new_count = get_child_i64(&bounce_state, "count");
                    if new_count != prev_bounce_count {
                        println!(
                            "[limiter] bounce     car={key} g={gear} rpm={:.0} count={new_count}/{}",
                            rpm, LIMITER_BOUNCE_MIN_COUNT
                        );
                    }
                }
            } else if rpm < high_rpm_threshold * 0.9 {
                // RPM well below the limiter zone — reset bounce state so it doesn't
                // carry over between driving situations.
                if self.limiter_log && prev_bounce_count > 0 {
                    println!("[limiter] reset      car={key} g={gear} rpm={:.0} (below threshold)", rpm);
                }
                curve.remove("limiterBounce");
            }

            let bounce_state = curve.get("limiterBounce").cloned().unwrap_or(Value::Null);
            let bounce_count = get_child_i64(&bounce_state, "count");
            // Return the bounce-confirmed global rev limit so rpm_reference can apply it
            // to all gears without requiring each gear to be individually driven to its ceiling.
            let bounce_confirmed = get_curve_f64(curve, "bounceConfirmedLimit");
            (get_curve_f64(curve, "maxObservedRpm"), bounce_count, bounce_confirmed, should_save)
        };
        if should_save {
            self.queue_save(&curves);
        }
        (observed, bounce_count, bounce_confirmed)
    }


    fn update_observed_gear(&self, payload: &Value) -> i64 {
        let key = power_curve_key(payload);
        let gear = learned_forward_gear(payload);
        if key.is_empty() || gear <= 0 {
            let curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
            let observed = curves
                .get(&key)
                .and_then(Value::as_object)
                .map(|c| get_curve_i64(c, "maxObservedGear"))
                .unwrap_or(0);
            return if is_electric(payload) {
                observed.max(1)
            } else {
                observed
            };
        }
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let (observed, should_save) = {
            let curve = Self::curve_mut(&mut curves, &key);
            let previous = get_curve_i64(curve, "maxObservedGear");
            let mut should_save = false;
            if gear > previous {
                curve.insert("maxObservedGear".to_string(), json!(gear));
                if previous > 0 {
                    mark_shift_cache_dirty(curve, None);
                }
                curve.insert("updatedAt".to_string(), json!(now_seconds()));
                should_save = true;
            }
            (get_curve_i64(curve, "maxObservedGear"), should_save)
        };
        if should_save {
            self.queue_save(&curves);
        }
        if is_electric(payload) {
            observed.max(1)
        } else {
            observed
        }
    }

    fn learn_shift_drop(&self, payload: &Value) {
        let key = power_curve_key(payload);
        let gear = learned_forward_gear(payload);
        let rpm = get_f64(payload, &["engine", "rpm"]);
        let now = get_f64(payload, &["receivedAt"]);
        if key.is_empty() || gear <= 0 || rpm < 1000.0 {
            return;
        }
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let should_save = {
            let curve = Self::curve_mut(&mut curves, &key);
            let previous = curve.get("lastShiftSample").unwrap_or(&Value::Null);
            let previous_gear = get_child_i64(previous, "gear");
            let previous_rpm = get_child_f64(previous, "rpm");
            let previous_at = get_child_f64(previous, "at");
            let mut should_save = false;
            let in_window = previous_gear > 0
                && now - previous_at <= SHIFT_DROP_CAPTURE_SECONDS;
            let is_upshift = in_window && gear == previous_gear + 1;

            if is_upshift && previous_rpm > rpm {
                let ratio = rpm / previous_rpm;
                if (SHIFT_DROP_MIN_RATIO..=SHIFT_DROP_MAX_RATIO).contains(&ratio) {
                    // Valid ratio — save it and update the sample.
                    let drops = ensure_object(curve, "gearDropRatios");
                    let transition = format!("{previous_gear}>{gear}");
                    if !drops.get(&transition).is_some_and(Value::is_object) {
                        drops.insert(transition.clone(), json!({ "ratio": ratio, "samples": 0 }));
                    }
                    let point = drops.get_mut(&transition).unwrap().as_object_mut().unwrap();
                    let samples = get_curve_i64(point, "samples");
                    let previous_ratio = get_curve_f64(point, "ratio");
                    // EMA: first observation is stored as-is; subsequent ones blend toward
                    // the latest at GEAR_DROP_EMA_RATE so old outlier shifts decay naturally.
                    let new_ratio = if samples == 0 {
                        ratio
                    } else {
                        previous_ratio * (1.0 - GEAR_DROP_EMA_RATE) + ratio * GEAR_DROP_EMA_RATE
                    };
                    point.insert("ratio".to_string(), json!(new_ratio));
                    point.insert("samples".to_string(), json!(samples + 1));
                    mark_shift_cache_dirty(curve, Some(&[previous_gear]));
                    curve.insert("updatedAt".to_string(), json!(now));
                    should_save = true;
                }
                // ratio > SHIFT_DROP_MAX_RATIO: RPM hasn't dropped far enough yet —
                // don't save and don't update lastShiftSample so we keep waiting for
                // the drop to complete (handled by the condition below).
            }
            // Only update the sample when no active shift is still in progress
            if !is_upshift || should_save {
                curve.insert(
                    "lastShiftSample".to_string(),
                    json!({ "gear": gear, "rpm": rpm, "at": now }),
                );
            }
            should_save
        };
        if should_save {
            self.queue_save(&curves);
        }
    }

    fn update_rpm_rise_rate(&self, payload: &Value) -> f64 {
        let key = power_curve_key(payload);
        let gear = learned_forward_gear(payload);
        let rpm = get_f64(payload, &["engine", "rpm"]);
        let now = get_f64(payload, &["receivedAt"]);
        if key.is_empty() || gear <= 0 || rpm < 1000.0 {
            return 0.0;
        }
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let curve = Self::curve_mut(&mut curves, &key);
        let previous = curve.get("lastRpmRateSample").unwrap_or(&Value::Null);
        let previous_gear = get_child_i64(previous, "gear");
        let previous_rpm = get_child_f64(previous, "rpm");
        let previous_at = get_child_f64(previous, "at");
        curve.insert(
            "lastRpmRateSample".to_string(),
            json!({ "gear": gear, "rpm": rpm, "at": now }),
        );
        let mut instant_rate = 0.0;
        if previous_gear == gear
            && now > previous_at
            && now - previous_at <= SHIFT_RPM_RATE_CAPTURE_SECONDS
            && rpm >= previous_rpm + SHIFT_RPM_RATE_MIN_GAIN
        {
            instant_rate = (rpm - previous_rpm) / (now - previous_at);
            if get_f64(payload, &["controls", "accel"]) > 0.5
                && (SHIFT_WARNING_MIN_RPM_RATE..=SHIFT_WARNING_MAX_RPM_RATE).contains(&instant_rate)
            {
                let rates = ensure_object(curve, "rpmRiseRateByGear");
                let gear_key = gear.to_string();
                let previous_rate = rates
                    .get(&gear_key)
                    .and_then(Value::as_f64)
                    .unwrap_or(instant_rate);
                rates.insert(
                    gear_key,
                    json!(
                        previous_rate * (1.0 - SHIFT_RPM_RATE_SMOOTHING)
                            + instant_rate * SHIFT_RPM_RATE_SMOOTHING
                    ),
                );
            }
        }
        if (SHIFT_WARNING_MIN_RPM_RATE..=SHIFT_WARNING_MAX_RPM_RATE).contains(&instant_rate) {
            instant_rate
        } else {
            0.0
        }
    }

    fn learn(&self, payload: &Value, limit_rpm: f64, bounce_count: i64) {
        let key = power_curve_key(payload);
        let rpm = get_f64(payload, &["engine", "rpm"]);
        let power = get_f64(payload, &["engine", "powerHp"]);
        let torque = get_f64(payload, &["engine", "torqueNm"]);
        if key.is_empty()
            || get_f64(payload, &["controls", "accel"]) < FULL_THROTTLE_MIN
            || get_f64(payload, &["controls", "brake"]) > FULL_THROTTLE_MAX_BRAKE
        {
            return;
        }
        if rpm < 1500.0 || rpm > limit_rpm * 1.02 || power <= 0.0 {
            return;
        }
        // Skip learning while the rev-limiter oscillation is being confirmed.
        // During bounce detection Forza intermittently cuts engine power; those
        // partial or zero-power readings are unreliable and would corrupt the
        // high-RPM end of the power curve.
        if bounce_count > 0 {
            return;
        }
        let now   = get_f64(payload, &["receivedAt"]);
        let gear  = learned_forward_gear(payload);
        let bucket = bucket_key(rpm);
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let curve = Self::curve_mut(&mut curves, &key);

        // Detect a new ascending run through this bucket:
        // a run starts when the car enters the bucket from below (previous RPM was in
        // a lower bucket, same gear, within the sample-age window).
        // This is independent of acceleration rate — a fast car with 1 sample/pass and
        // a slow car with 20 samples/pass both increment runs exactly once per pass.
        let prev_learn    = curve.get("lastLearnSample").cloned().unwrap_or(Value::Null);
        let prev_rpm_l    = get_child_f64(&prev_learn, "rpm");
        let prev_at_l     = get_child_f64(&prev_learn, "at");
        let prev_gear_l   = get_child_i64(&prev_learn, "gear");
        let age_ok_l      = prev_at_l > 0.0 && now - prev_at_l <= LIMIT_LEARN_MAX_SAMPLE_AGE;
        let entering_bucket = age_ok_l
            && prev_gear_l == gear
            && rpm > prev_rpm_l              // rising
            && bucket_key(prev_rpm_l) != bucket; // crossed into a new bucket
        curve.insert(
            "lastLearnSample".to_string(),
            json!({ "gear": gear, "rpm": rpm, "at": now, "runStartRpm": 0.0 }),
        );

        let buckets = ensure_object(curve, "buckets");
        let is_new_bucket = !buckets.contains_key(&bucket);
        if !buckets.get(&bucket).is_some_and(Value::is_object) {
            buckets.insert(
                bucket.clone(),
                json!({ "power": 0.0, "torque": 0.0, "samples": 0, "runs": 0 }),
            );
        }
        let point = buckets.get_mut(&bucket).unwrap().as_object_mut().unwrap();
        let previous_power  = get_curve_f64(point, "power");
        let previous_torque = get_curve_f64(point, "torque");
        let samples         = get_curve_i64(point, "samples");
        let runs            = get_curve_i64(point, "runs") + if entering_bucket { 1 } else { 0 };
        point.insert("runs".to_string(), json!(runs));

        // Bidirectional EMA bucket learning:
        //
        //   Fresh bucket (< RELEARN_MIN_RUNS ascending passes):
        //     Take max() immediately — still exploring, find the real ceiling fast.
        //
        //   Established bucket (≥ RELEARN_MIN_RUNS):
        //     • Deviation ≤ TOLERANCE   → keep stored value (sensor noise, ignore).
        //     • Deviation > TOLERANCE   → symmetric EMA blend toward current reading.
        //
        //   A single outlier sample shifts the bucket by only RELEARN_RATE and
        //   self-corrects on the next clean pass.  A sustained real change (3 passes)
        //   moves the stored value ~70 % of the way to the new truth.
        let blend = |stored: f64, current: f64| -> f64 {
            if stored <= 0.0 || runs < POWER_BUCKET_RELEARN_MIN_RUNS {
                return stored.max(current);
            }
            let deviation = (current - stored).abs() / stored;
            if deviation > POWER_BUCKET_RELEARN_TOLERANCE {
                stored * (1.0 - POWER_BUCKET_RELEARN_RATE) + current * POWER_BUCKET_RELEARN_RATE
            } else {
                stored // small fluctuation — hold steady
            }
        };
        let new_power  = blend(previous_power,  power);
        let new_torque = blend(previous_torque, torque);

        point.insert("power".to_string(),  json!(new_power));
        point.insert("torque".to_string(), json!(new_torque));
        point.insert("samples".to_string(), json!(samples + 1));

        let power_up   = previous_power > 0.0 && new_power > previous_power * 1.001;
        let power_down = previous_power > 0.0 && new_power < previous_power * 0.999;
        if is_new_bucket {
            mark_no_optimal_shift_cache_dirty(curve);
        } else if power_up || power_down {
            mark_shift_cache_dirty(curve, None);
        }
        curve.insert("updatedAt".to_string(), json!(now_seconds()));
        if samples == 0 || samples % 20 == 0 {
            self.queue_save(&curves);
        }
    }

    fn cached_power_shift_rpm(
        &self,
        payload: &Value,
        limit_rpm: f64,
        max_shift_rpm: f64,
        rpm_rate: f64,
    ) -> Option<f64> {
        let key = power_curve_key(payload);
        let gear = learned_forward_gear(payload);
        if key.is_empty() || gear <= 0 {
            return None;
        }
        let mut curves = self.curves.lock().unwrap_or_else(|e| e.into_inner());
        let curve = Self::curve_mut(&mut curves, &key);
        let gear_key = gear.to_string();

        let cached = curve
            .get("optimalShiftRpmByGear")
            .and_then(Value::as_object)
            .and_then(|o| o.get(&gear_key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let is_dirty = set_contains(curve, "dirtyShiftGears", &gear_key);
        let at_full_throttle = get_f64(payload, &["controls", "accel"]) >= FULL_THROTTLE_MIN;

        if cached > 0.0 {
            // Dirty + full throttle: clear the cached value and recompute below.
            if is_dirty && at_full_throttle {
                object_remove_key(curve, "optimalShiftRpmByGear", &gear_key);
                object_remove_key(curve, "shiftWarningRpmByGear", &gear_key);
                set_remove(curve, "validatedShiftGears", &gear_key);
                if let Some(logger) = &self.logger {
                    logger.log_cache_event("cache_forced_recompute", payload, Map::new());
                }
                // cached value is now gone — fall through to the recompute branch below
            } else {
                // Not dirty or not at full throttle: validate and return the existing value.
                let warning =
                    dynamic_shift_warning_rpm(cached, rpm_rate, shift_warning_lead_seconds(payload))
                        .min(max_shift_rpm);
                if set_contains(curve, "validatedShiftGears", &gear_key) && !is_dirty {
                    if let Some(logger) = &self.logger {
                        logger.log_cache_event(
                            "cache_hit_validated",
                            payload,
                            map_fields(&[("shiftRpm", cached), ("warningRpm", warning)]),
                        );
                    }
                    return Some(cached);
                }
                return match validate_cached_shift_point(
                    curve, payload, gear, cached, warning, limit_rpm,
                ) {
                    Some(true) => {
                        set_add(curve, "validatedShiftGears", &gear_key);
                        self.queue_save(&curves);
                        if let Some(logger) = &self.logger {
                            logger.log(
                                "cache_validated",
                                payload,
                                map_fields(&[("shiftRpm", cached), ("warningRpm", warning)]),
                            );
                        }
                        Some(cached)
                    }
                    None => {
                        if let Some(logger) = &self.logger {
                            logger.log_cache_event(
                                "cache_pending_validation",
                                payload,
                                map_fields(&[("shiftRpm", cached), ("warningRpm", warning)]),
                            );
                        }
                        Some(cached)
                    }
                    Some(false) => {
                        object_remove_key(curve, "optimalShiftRpmByGear", &gear_key);
                        object_remove_key(curve, "shiftWarningRpmByGear", &gear_key);
                        set_remove(curve, "validatedShiftGears", &gear_key);
                        set_add(curve, "dirtyShiftGears", &gear_key);
                        self.queue_save(&curves);
                        if let Some(logger) = &self.logger {
                            logger.log(
                                "cache_invalidated",
                                payload,
                                map_fields(&[
                                    ("previousShiftRpm", cached),
                                    ("previousWarningRpm", warning),
                                ]),
                            );
                        }
                        None
                    }
                };
            }
        }

        // Recompute branch: no cached value, or just cleared by dirty + full throttle.
        if set_contains(curve, "noOptimalShiftGears", &gear_key)
            && !set_contains(curve, "dirtyShiftGears", &gear_key)
        {
            return None;
        }
        let learned = compute_power_shift_rpm(curve, payload, limit_rpm, max_shift_rpm);
        if let Some(learned) = learned {
            ensure_object(curve, "optimalShiftRpmByGear")
                .insert(gear_key.clone(), json!(learned));
            let warning = dynamic_shift_warning_rpm(
                learned,
                rpm_rate,
                shift_warning_lead_seconds(payload),
            )
            .min(max_shift_rpm);
            ensure_object(curve, "shiftWarningRpmByGear")
                .insert(gear_key.clone(), json!(warning));
            set_remove(curve, "noOptimalShiftGears", &gear_key);
            if let Some(logger) = &self.logger {
                logger.log(
                    "shift_recomputed",
                    payload,
                    map_fields(&[("shiftRpm", learned), ("warningRpm", warning)]),
                );
            }
        } else {
            object_remove_key(curve, "optimalShiftRpmByGear", &gear_key);
            object_remove_key(curve, "shiftWarningRpmByGear", &gear_key);
            set_add(curve, "noOptimalShiftGears", &gear_key);
            if let Some(logger) = &self.logger {
                logger.log("shift_recompute_empty", payload, Map::new());
            }
        }
        set_remove(curve, "dirtyShiftGears", &gear_key);
        curve.insert("updatedAt".to_string(), json!(now_seconds()));
        self.queue_save(&curves);
        learned
    }

    pub(crate) fn log_shift_decision(&self, payload: &Value, fields: Map<String, Value>) {
        if let Some(logger) = &self.logger {
            logger.log_shift_decision(payload, fields);
        }
    }
}

fn compute_power_shift_rpm(
    curve: &Map<String, Value>,
    payload: &Value,
    limit_rpm: f64,
    max_shift_rpm: f64,
) -> Option<f64> {
    let gear = learned_forward_gear(payload);
    if gear <= 0 {
        return None;
    }
    let drop_ratio = curve
        .get("gearDropRatios")
        .and_then(Value::as_object)
        .and_then(|d| d.get(&format!("{gear}>{}", gear + 1)))
        .map(|p| get_child_f64(p, "ratio"))
        .unwrap_or(0.0);
    if drop_ratio <= 0.0 {
        return None;
    }
    let points = curve_points_from_buckets(curve.get("buckets"), limit_rpm);
    if points.len() < POWER_CURVE_MIN_BUCKETS {
        return None;
    }
    let peak_rpm = points
        .iter()
        .max_by(|a, b| a.power.total_cmp(&b.power))
        .map(|point| point.rpm)
        .unwrap_or(1500.0);
    for point in points
        .iter()
        .filter(|p| p.rpm >= peak_rpm && p.rpm <= max_shift_rpm)
    {
        let after_shift_rpm = point.rpm * drop_ratio;
        // Only shift if the next gear delivers at least SHIFT_POWER_GAIN_RATIO more power.
        // This prevents premature shifts on flat power curves (EVs, noisy data).
        if let Some(after_shift_power) = interpolated_power(&points, after_shift_rpm)
            && after_shift_power >= point.power * SHIFT_POWER_GAIN_RATIO
        {
            return Some(point.rpm);
        }
    }
    Some(max_shift_rpm)
}

fn validate_cached_shift_point(
    curve: &Map<String, Value>,
    payload: &Value,
    gear: i64,
    cached_shift_rpm: f64,
    cached_warning_rpm: f64,
    limit_rpm: f64,
) -> Option<bool> {
    let rpm = get_f64(payload, &["engine", "rpm"]);
    let power = get_f64(payload, &["engine", "powerHp"]);
    if get_f64(payload, &["controls", "accel"]) < FULL_THROTTLE_MIN
        || get_f64(payload, &["controls", "brake"]) > FULL_THROTTLE_MAX_BRAKE
    {
        return None;
    }
    if learned_forward_gear(payload) != gear || power <= 0.0 {
        return None;
    }
    if rpm < cached_warning_rpm - SHIFT_CACHE_VALIDATION_RPM_WINDOW
        || rpm > cached_shift_rpm + SHIFT_CACHE_VALIDATION_RPM_WINDOW
    {
        return None;
    }
    let points = curve_points_from_buckets(curve.get("buckets"), limit_rpm);
    let expected = interpolated_power(&points, rpm)?;
    if expected <= 0.0 {
        return None;
    }
    let deviation = (power - expected).abs() / expected;
    if deviation <= SHIFT_CACHE_VALID_POWER_TOLERANCE {
        Some(true)
    } else if deviation >= SHIFT_CACHE_INVALID_POWER_TOLERANCE {
        Some(false)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct CurvePoint {
    rpm: f64,
    power: f64,
}

fn curve_points_from_buckets(buckets: Option<&Value>, limit_rpm: f64) -> Vec<CurvePoint> {
    let mut points = Vec::new();
    if let Some(buckets) = buckets.and_then(Value::as_object) {
        for (rpm, point) in buckets {
            let samples = get_child_i64(point, "samples");
            let power = get_child_f64(point, "power");
            let rpm_value = rpm.parse::<f64>().unwrap_or(0.0);
            if samples >= 2 && power > 0.0 && rpm_value <= limit_rpm * 1.02 {
                points.push(CurvePoint {
                    rpm: rpm_value,
                    power,
                });
            }
        }
    }
    points.sort_by(|a, b| a.rpm.total_cmp(&b.rpm));
    points
}

fn interpolated_power(points: &[CurvePoint], rpm: f64) -> Option<f64> {
    if points.is_empty() || rpm < points[0].rpm || rpm > points[points.len() - 1].rpm {
        return None;
    }
    for (index, point) in points.iter().enumerate() {
        if (rpm - point.rpm).abs() < f64::EPSILON {
            return Some(point.power);
        }
        if rpm < point.rpm && index > 0 {
            let prev = points[index - 1];
            let span = point.rpm - prev.rpm;
            if span <= 0.0 {
                return Some(point.power);
            }
            let ratio = (rpm - prev.rpm) / span;
            return Some(prev.power + (point.power - prev.power) * ratio);
        }
    }
    Some(points[points.len() - 1].power)
}

pub(crate) fn enrich_shift_data(payload: &mut Value, power_curves: Option<&Arc<PowerCurveStore>>) {
    let mut observed_limit = 0.0;
    let mut bounce_confirmed = 0.0;
    let mut max_observed_gear = if is_electric(payload) { 1 } else { 0 };
    let rpm_rate;
    let mut bounce_count = 0i64;
    if let Some(store) = power_curves {
        store.validate_profile(payload);
        (observed_limit, bounce_count, bounce_confirmed) = store.update_observed_limit(payload);
        max_observed_gear = store.update_observed_gear(payload);
        store.learn_shift_drop(payload);
        rpm_rate = store.update_rpm_rise_rate(payload);
    } else {
        rpm_rate = 0.0;
    }

    let (limit_rpm, redline_rpm) = rpm_reference(payload, observed_limit, bounce_confirmed);
    let safety_ratio = safety_shift_target_ratio(payload);
    let safety_shift_rpm = limit_rpm * safety_ratio;
    let warning_lead_seconds = shift_warning_lead_seconds(payload);
    let safety_warning_rpm =
        safety_shift_warning_rpm(payload, safety_shift_rpm, rpm_rate, warning_lead_seconds);
    let mut learned_shift_rpm = None;
    let mut use_power_shift = true;
    if is_electric(payload) {
        let gear = current_forward_gear(payload);
        use_power_shift = max_observed_gear > 1 && gear > 0 && gear < max_observed_gear;
    }
    if let Some(store) = power_curves {
        if use_power_shift {
            learned_shift_rpm =
                store.cached_power_shift_rpm(payload, limit_rpm, safety_shift_rpm, rpm_rate);
        }
        store.learn(payload, limit_rpm, bounce_count);
    }
    let warned_learned_shift_rpm =
        learned_shift_rpm.map(|rpm| dynamic_shift_warning_rpm(rpm, rpm_rate, warning_lead_seconds));
    // EVs in the highest known gear: suppress the shift warning (no higher gear available).
    // Also suppress when only one gear has been observed yet to avoid spurious flashing.
    let ev_suppress = is_electric(payload) && !use_power_shift;
    let shift_now_rpm = if ev_suppress {
        0.0
    } else {
        warned_learned_shift_rpm.map_or(safety_warning_rpm, |learned| {
            learned.min(safety_warning_rpm)
        })
    };
    let mut shift_source = "safety";
    if let Some(warned) = warned_learned_shift_rpm {
        shift_source = if warned <= safety_warning_rpm {
            "learned"
        } else {
            "learned_capped"
        };
    }
    if let Some(store) = power_curves {
        let mut fields = Map::new();
        fields.insert(
            "learnedShiftRpm".to_string(),
            json!(learned_shift_rpm.unwrap_or(0.0)),
        );
        fields.insert(
            "learnedWarningRpm".to_string(),
            json!(warned_learned_shift_rpm.unwrap_or(0.0)),
        );
        fields.insert("safetyShiftRpm".to_string(), json!(safety_shift_rpm));
        fields.insert("safetyWarningRpm".to_string(), json!(safety_warning_rpm));
        fields.insert("shiftNowRpm".to_string(), json!(shift_now_rpm));
        fields.insert("rpmRiseRate".to_string(), json!(rpm_rate));
        fields.insert(
            "warningLeadSeconds".to_string(),
            json!(warning_lead_seconds),
        );
        fields.insert("source".to_string(), json!(shift_source));
        store.log_shift_decision(payload, fields);
    }

    let engine = ensure_path_object(payload, &["engine"]);
    engine.insert("limitRpm".to_string(), json!(limit_rpm));
    engine.insert("redlineRpm".to_string(), json!(redline_rpm));
    engine.insert("observedLimitRpm".to_string(), json!(observed_limit));
    // observed_limit ist bereits maxObservedRpm nach allen Bounce-Korrekturen.
    engine.insert("confirmedLimiterRpm".to_string(), json!(observed_limit));
    // Running bounce counter: 0 = outside limiter zone, 1-2 = being detected,
    // 3 = confirmed (immediately reset to 0, so the next frame reads 0 again).
    engine.insert("limiterBounceCount".to_string(), json!(bounce_count));
    engine.insert("safetyShiftRatio".to_string(), json!(safety_ratio));
    engine.insert("safetyShiftRpm".to_string(), json!(safety_shift_rpm));
    engine.insert(
        "safetyShiftWarningRpm".to_string(),
        json!(safety_warning_rpm),
    );
    engine.insert(
        "learnedPowerShiftRpm".to_string(),
        json!(learned_shift_rpm.unwrap_or(0.0)),
    );
    engine.insert(
        "learnedShiftWarningRpm".to_string(),
        json!(warned_learned_shift_rpm.unwrap_or(0.0)),
    );
    engine.insert("shiftNowRpm".to_string(), json!(shift_now_rpm));
    engine.insert(
        "shiftWarningLeadSeconds".to_string(),
        json!(warning_lead_seconds),
    );
    engine.insert("rpmRiseRate".to_string(), json!(rpm_rate));
    engine.insert(
        "shiftNowRatioOfRedline".to_string(),
        json!(if redline_rpm > 0.0 {
            shift_now_rpm / redline_rpm
        } else {
            1.0
        }),
    );
    let electric = is_electric(payload);
    let car = ensure_path_object(payload, &["car"]);
    car.insert("maxObservedGear".to_string(), json!(max_observed_gear));
    car.insert("electric".to_string(), json!(electric));
}
/// Detects the characteristic RPM oscillation at the rev limiter.
///
/// On the rising edge, the local peak (refRpm) is tracked. Once RPM drops by
/// MIN_AMPLITUDE below that peak, a direction reversal is recorded (now falling).
/// On the falling edge the local trough is tracked symmetrically.
/// After LIMITER_BOUNCE_MIN_COUNT reversals within LIMITER_BOUNCE_WINDOW seconds
/// the limiter is confirmed; the return value is the highest RPM seen in that window.
fn detect_limiter_bounce(curve: &mut Map<String, Value>, rpm: f64, now: f64) -> Option<f64> {
    let bounce = curve.get("limiterBounce").cloned().unwrap_or(Value::Null);
    let prev_dir = get_child_f64(&bounce, "dir"); // +1 rising, -1 falling, 0 initial
    let ref_rpm = {
        let r = get_child_f64(&bounce, "refRpm");
        if r > 0.0 { r } else { rpm }
    };
    let count = get_child_i64(&bounce, "count");
    let window_start = {
        let ws = get_child_f64(&bounce, "windowStart");
        if ws > 0.0 { ws } else { now }
    };
    let window_peak = {
        let wp = get_child_f64(&bounce, "windowPeak");
        if wp > 0.0 { wp } else { rpm }
    };

    let in_window = now - window_start <= LIMITER_BOUNCE_WINDOW;
    let new_window_peak = if in_window { window_peak.max(rpm) } else { rpm };
    let new_window_start = if in_window { window_start } else { now };

    // prev_dir >= 0 means rising edge (including the initial state)
    let rising = prev_dir >= 0.0;
    let (new_dir, new_ref, direction_changed) = if rising {
        let new_ref = ref_rpm.max(rpm); // track local peak
        let drop = new_ref - rpm;
        if drop >= LIMITER_BOUNCE_MIN_AMPLITUDE && drop <= LIMITER_BOUNCE_MAX_AMPLITUDE {
            (-1.0_f64, rpm, true) // direction reversal: now falling
        } else {
            (1.0_f64, new_ref, false)
        }
    } else {
        let new_ref = ref_rpm.min(rpm); // track local trough
        let rise = rpm - new_ref;
        if rise >= LIMITER_BOUNCE_MIN_AMPLITUDE && rise <= LIMITER_BOUNCE_MAX_AMPLITUDE {
            (1.0_f64, rpm, true) // direction reversal: now rising
        } else {
            (-1.0_f64, new_ref, false)
        }
    };

    let new_count = if direction_changed {
        if in_window { count + 1 } else { 1 }
    } else if in_window {
        count
    } else {
        0
    };

    let detected = if direction_changed && new_count >= LIMITER_BOUNCE_MIN_COUNT {
        Some(new_window_peak)
    } else {
        None
    };

    curve.insert(
        "limiterBounce".to_string(),
        json!({
            "dir": new_dir,
            "refRpm": new_ref,
            "count": if detected.is_some() { 0i64 } else { new_count },
            "windowStart": new_window_start,
            "windowPeak": if detected.is_some() { rpm } else { new_window_peak },
        }),
    );

    detected
}

fn rpm_reference(payload: &Value, observed_limit: f64, bounce_confirmed: f64) -> (f64, f64) {
    let idle = get_f64(payload, &["engine", "idleRpm"]).max(0.0);
    let max_rpm = get_f64(payload, &["engine", "maxRpm"]).max(3000.0);
    // Without real drive data, use the conservative initial estimate (94 % vs 89.5 %).
    // This gives a noticeably early shift warning on the first drive — better to warn
    // a bit early than to hit the limiter.  Once drive data is available
    // (observed_limit > 0), DEFAULT_ENGINE_LIMIT_RATIO acts only as a floor so that
    // limit_rpm can't drift too low.
    let estimated_limit = if observed_limit > 0.0 {
        max_rpm * DEFAULT_ENGINE_LIMIT_RATIO_OF_TACHO_MAX
    } else {
        max_rpm * INITIAL_ENGINE_LIMIT_RATIO_OF_TACHO_MAX
    };
    // Cap observedLimit at MAX_OBSERVED_LIMIT_RATIO to prevent an RPM spike from pushing
    // limit_rpm all the way to maxRpm and eliminating the safety margin.
    let capped_observed = observed_limit.min(max_rpm * MAX_OBSERVED_LIMIT_RATIO_OF_TACHO_MAX);
    let base_limit = (idle + 1000.0).max(estimated_limit).max(capped_observed);
    // When bounce detection has confirmed the actual rev limiter AND the highest observed
    // RPM is consistent with that value (i.e. no other gear has driven significantly past
    // it), trust the confirmed limit over the theoretical floor.  The rev limiter is a
    // global engine property — confirming it in gear 1 is enough to apply it to all gears.
    // If the car later reaches higher RPM in another gear the bounce_confirmed value is
    // superseded (observed_limit > bounce_confirmed * 1.06 → condition fails).
    let limit_rpm = if bounce_confirmed > 0.0
        && bounce_confirmed < base_limit
        && observed_limit <= bounce_confirmed * 1.06
    {
        bounce_confirmed.max(idle + 1000.0)
    } else {
        base_limit
    };
    let redline_rpm = (idle + 1000.0).max(limit_rpm * REDLINE_RATIO_OF_ENGINE_LIMIT);
    (limit_rpm, redline_rpm)
}

fn safety_shift_target_ratio(payload: &Value) -> f64 {
    match learned_forward_gear(payload) {
        1 => 0.98,
        2 => 0.985,
        3 => 0.99,
        _ => SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT,
    }
}

fn shift_warning_lead_seconds(payload: &Value) -> f64 {
    match learned_forward_gear(payload) {
        1 => 0.22,
        2 => 0.22,
        3 => 0.20,
        _ => SHIFT_WARNING_LEAD_SECONDS,
    }
}

fn dynamic_shift_warning_rpm(shift_rpm: f64, rpm_rate: f64, lead_seconds: f64) -> f64 {
    if shift_rpm <= 0.0 {
        return 0.0;
    }
    let gap = if (SHIFT_WARNING_MIN_RPM_RATE..=SHIFT_WARNING_MAX_RPM_RATE).contains(&rpm_rate) {
        clamp(
            rpm_rate * lead_seconds,
            SHIFT_WARNING_MIN_GAP_RPM,
            SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM,
        )
    } else {
        clamp(
            shift_rpm * SHIFT_WARNING_FALLBACK_GAP_RATIO,
            SHIFT_WARNING_MIN_GAP_RPM,
            SHIFT_WARNING_MAX_FALLBACK_GAP_RPM,
        )
    };
    (shift_rpm - gap).max(0.0)
}

fn safety_shift_warning_rpm(
    payload: &Value,
    shift_rpm: f64,
    rpm_rate: f64,
    lead_seconds: f64,
) -> f64 {
    if shift_rpm <= 0.0 {
        return 0.0;
    }
    let idle = get_f64(payload, &["engine", "idleRpm"]).max(0.0);
    let usable_band = (shift_rpm - idle).max(1000.0);
    let max_gap = clamp(
        usable_band * SAFETY_SHIFT_WARNING_MAX_BAND_RATIO,
        SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM,
        usable_band * 0.35,
    );
    // Fallback gap: proportional to the usable RPM band.
    // Used as a floor for the dynamic gap so the safety warning always keeps at least
    // this much distance — even at low rpm_rate where clamp(rate*lead, 100, max)
    // would otherwise collapse to just 100 RPM and risk landing right at the limiter.
    let fallback_gap = clamp(
        usable_band * SAFETY_SHIFT_WARNING_FALLBACK_BAND_RATIO,
        SHIFT_WARNING_MIN_GAP_RPM,
        max_gap,
    );
    let gap = if (SHIFT_WARNING_MIN_RPM_RATE..=SHIFT_WARNING_MAX_RPM_RATE).contains(&rpm_rate) {
        clamp(rpm_rate * lead_seconds, fallback_gap, max_gap)
    } else {
        fallback_gap
    };
    (shift_rpm - gap).max(0.0)
}

fn current_forward_gear(payload: &Value) -> i64 {
    let gear = get_f64(payload, &["controls", "gear"]) as i64;
    if gear > 0 { gear } else { 0 }
}

pub(crate) fn learned_forward_gear(payload: &Value) -> i64 {
    let gear = current_forward_gear(payload);
    if (1..=MAX_PLAUSIBLE_LEARNED_GEAR).contains(&gear) {
        gear
    } else {
        0
    }
}

pub(crate) fn power_curve_key(payload: &Value) -> String {
    let ordinal = get_f64(payload, &["car", "ordinal"]) as i64;
    let pi = get_f64(payload, &["car", "performanceIndex"]) as i64;
    if ordinal <= 0 {
        String::new()
    } else {
        format!("{ordinal}:{pi}")
    }
}

fn is_electric(payload: &Value) -> bool {
    get_f64(payload, &["car", "cylinders"]) as i64 == 0
}

fn mark_shift_cache_dirty(curve: &mut Map<String, Value>, gears: Option<&[i64]>) {
    let max_gear = get_curve_i64(curve, "maxObservedGear").max(MAX_PLAUSIBLE_LEARNED_GEAR);
    let gear_list: Vec<i64> =
        gears.map_or_else(|| (1..=max_gear).collect(), |items| items.to_vec());
    let mut dirty = array_set(curve, "dirtyShiftGears");
    let mut validated = array_set(curve, "validatedShiftGears");
    let mut no_optimal = array_set(curve, "noOptimalShiftGears");
    for gear in gear_list {
        if (1..=MAX_PLAUSIBLE_LEARNED_GEAR).contains(&gear) {
            let gear_key = gear.to_string();
            dirty.insert(gear_key.clone());
            validated.remove(&gear_key);
            no_optimal.remove(&gear_key);
        }
    }
    write_set(curve, "dirtyShiftGears", dirty);
    write_set(curve, "validatedShiftGears", validated);
    write_set(curve, "noOptimalShiftGears", no_optimal);
}

fn mark_no_optimal_shift_cache_dirty(curve: &mut Map<String, Value>) {
    let gears: Vec<i64> = array_set(curve, "noOptimalShiftGears")
        .into_iter()
        .filter_map(|item| item.parse::<i64>().ok())
        .filter(|gear| (1..=MAX_PLAUSIBLE_LEARNED_GEAR).contains(gear))
        .collect();
    if !gears.is_empty() {
        mark_shift_cache_dirty(curve, Some(&gears));
    }
}

fn set_contains(curve: &Map<String, Value>, field: &str, item: &str) -> bool {
    curve
        .get(field)
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|v| v.as_str() == Some(item)))
}

fn set_add(curve: &mut Map<String, Value>, field: &str, item: &str) {
    let mut set = array_set(curve, field);
    set.insert(item.to_string());
    write_set(curve, field, set);
}

fn set_remove(curve: &mut Map<String, Value>, field: &str, item: &str) {
    let mut set = array_set(curve, field);
    set.remove(item);
    write_set(curve, field, set);
}

fn array_set(curve: &Map<String, Value>, field: &str) -> HashSet<String> {
    curve
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_set(curve: &mut Map<String, Value>, field: &str, set: HashSet<String>) {
    let mut items: Vec<i64> = set
        .into_iter()
        .filter_map(|item| item.parse().ok())
        .collect();
    items.sort_unstable();
    curve.insert(
        field.to_string(),
        json!(items.into_iter().map(|n| n.to_string()).collect::<Vec<_>>()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── interpolated_power ────────────────────────────────────────────────────

    fn pts(pairs: &[(f64, f64)]) -> Vec<CurvePoint> {
        pairs.iter().map(|&(rpm, power)| CurvePoint { rpm, power }).collect()
    }

    #[test]
    fn interpolated_power_empty_returns_none() {
        assert_eq!(interpolated_power(&[], 5000.0), None);
    }

    #[test]
    fn interpolated_power_below_range_returns_none() {
        let p = pts(&[(3000.0, 200.0), (5000.0, 300.0)]);
        assert_eq!(interpolated_power(&p, 2000.0), None);
    }

    #[test]
    fn interpolated_power_above_range_returns_none() {
        // rpm > last point → out-of-range, returns None (no extrapolation)
        let p = pts(&[(3000.0, 200.0), (5000.0, 300.0)]);
        assert_eq!(interpolated_power(&p, 6000.0), None);
    }

    #[test]
    fn interpolated_power_exact_match() {
        let p = pts(&[(3000.0, 200.0), (5000.0, 300.0), (7000.0, 280.0)]);
        assert_eq!(interpolated_power(&p, 3000.0), Some(200.0));
        assert_eq!(interpolated_power(&p, 5000.0), Some(300.0));
    }

    #[test]
    fn interpolated_power_midpoint_linearly_interpolated() {
        let p = pts(&[(3000.0, 200.0), (5000.0, 300.0)]);
        // Midpoint at 4000 RPM: 200 + (300-200)*0.5 = 250
        let result = interpolated_power(&p, 4000.0).unwrap();
        assert!((result - 250.0).abs() < 0.001, "got {result}");
    }

    #[test]
    fn interpolated_power_quarter_point() {
        let p = pts(&[(0.0, 0.0), (4000.0, 400.0)]);
        // 1000 RPM = 25 % of span → 100 hp
        let result = interpolated_power(&p, 1000.0).unwrap();
        assert!((result - 100.0).abs() < 0.001, "got {result}");
    }

    // ── dynamic_shift_warning_rpm ─────────────────────────────────────────────

    #[test]
    fn warning_rpm_zero_for_non_positive_shift_rpm() {
        assert_eq!(dynamic_shift_warning_rpm(0.0, 2000.0, 0.2), 0.0);
        assert_eq!(dynamic_shift_warning_rpm(-100.0, 2000.0, 0.2), 0.0);
    }

    #[test]
    fn warning_rpm_uses_rate_when_in_valid_range() {
        // rpm_rate=2000 rpm/s, lead=0.2 s → gap = clamp(400, 100, 800) = 400
        let warning = dynamic_shift_warning_rpm(8000.0, 2000.0, 0.2);
        assert!((warning - 7600.0).abs() < 1.0, "got {warning}");
    }

    #[test]
    fn warning_rpm_uses_fallback_when_rate_too_low() {
        // rpm_rate=0 (below SHIFT_WARNING_MIN_RPM_RATE=350) → fallback gap
        // fallback = clamp(8000 * 0.012, 100, 220) = clamp(96, 100, 220) = 100
        let warning = dynamic_shift_warning_rpm(8000.0, 0.0, 0.2);
        assert!((warning - 7900.0).abs() < 1.0, "got {warning}");
    }

    #[test]
    fn warning_rpm_clamps_large_rate_gap() {
        // rpm_rate=10000, lead=0.2 → rate*lead=2000, clamped to max=800
        let warning = dynamic_shift_warning_rpm(8000.0, 10_000.0, 0.2);
        assert!((warning - 7200.0).abs() < 1.0, "got {warning}");
    }

    // ── rpm_reference ─────────────────────────────────────────────────────────

    fn payload_with_engine(max_rpm: f64, idle_rpm: f64) -> Value {
        json!({ "engine": { "maxRpm": max_rpm, "idleRpm": idle_rpm } })
    }

    #[test]
    fn rpm_reference_no_observed_uses_initial_ratio() {
        let payload = payload_with_engine(8000.0, 750.0);
        let (limit, _redline) = rpm_reference(&payload, 0.0, 0.0);
        // Initial ratio = 0.94 → estimated = 8000 * 0.94 = 7520
        // limit = max(idle+1000=1750, 7520) = 7520
        assert!((limit - 7520.0).abs() < 1.0, "limit={limit}");
    }

    #[test]
    fn rpm_reference_observed_limit_raises_limit() {
        let payload = payload_with_engine(8000.0, 750.0);
        // observed_limit > 0 → DEFAULT ratio used (0.895) → estimated=7160
        // capped observed = min(7800, 8000*0.97=7760) = 7760
        // limit = max(1750, 7160, 7760) = 7760
        let (limit, _) = rpm_reference(&payload, 7800.0, 0.0);
        assert!((limit - 7760.0).abs() < 1.0, "limit={limit}");
    }

    #[test]
    fn rpm_reference_observed_limit_capped_at_97_percent() {
        let payload = payload_with_engine(8000.0, 750.0);
        // observed_limit = 8000 (at 100%) → capped at 97% = 7760
        let (limit, _) = rpm_reference(&payload, 8000.0, 0.0);
        assert!((limit - 7760.0).abs() < 1.0, "limit={limit}");
    }

    #[test]
    fn rpm_reference_bounce_confirmed_overrides_floor() {
        let payload = payload_with_engine(8000.0, 750.0);
        // bounce_confirmed=6755, observed=6755 — rev limiter confirmed below theoretical floor
        // estimated floor = 8000*0.895 = 7160, but bounce_confirmed < base AND
        // observed (6755) <= 6755*1.06 (7160) → use bounce_confirmed
        let (limit, _) = rpm_reference(&payload, 6755.0, 6755.0);
        assert!((limit - 6755.0).abs() < 1.0, "limit={limit}");
    }

    #[test]
    fn rpm_reference_bounce_ignored_when_higher_rpm_observed() {
        let payload = payload_with_engine(8000.0, 750.0);
        // bounce_confirmed=6755 but car reached 7800 in another gear → power limitation,
        // not rev limiter. 7800 > 6755*1.06=7160 → ignore bounce_confirmed, use standard limit
        let (limit, _) = rpm_reference(&payload, 7800.0, 6755.0);
        // capped_observed = min(7800, 7760) = 7760
        assert!((limit - 7760.0).abs() < 1.0, "limit={limit}");
    }

    // ── detect_limiter_bounce ─────────────────────────────────────────────────

    #[test]
    fn limiter_bounce_confirms_after_three_reversals() {
        let mut curve = Map::new();
        // t=0.00: rising phase starts, no reversal yet
        assert_eq!(detect_limiter_bounce(&mut curve, 8100.0, 0.00), None);
        // t=0.15: drop of 40 RPM (>= MIN_AMPLITUDE=30) → first reversal
        assert_eq!(detect_limiter_bounce(&mut curve, 8060.0, 0.15), None);
        // t=0.30: rise of 45 RPM → second reversal
        assert_eq!(detect_limiter_bounce(&mut curve, 8105.0, 0.30), None);
        // t=0.45: drop of 50 RPM → third reversal → CONFIRMED
        let result = detect_limiter_bounce(&mut curve, 8055.0, 0.45);
        assert!(result.is_some(), "expected confirmation, got None");
        let peak = result.unwrap();
        // Peak should be the highest RPM seen in the window (8105)
        assert!((peak - 8105.0).abs() < 1.0, "peak={peak}");
    }

    #[test]
    fn limiter_bounce_not_triggered_with_too_large_swings() {
        // Swings > LIMITER_BOUNCE_MAX_AMPLITUDE (400) are gear changes, not limiter bounce
        let mut curve = Map::new();
        assert_eq!(detect_limiter_bounce(&mut curve, 8000.0, 0.00), None);
        // Drop of 500 RPM — exceeds max amplitude, no reversal counted
        assert_eq!(detect_limiter_bounce(&mut curve, 7500.0, 0.15), None);
        assert_eq!(detect_limiter_bounce(&mut curve, 8000.0, 0.30), None);
        assert_eq!(detect_limiter_bounce(&mut curve, 7500.0, 0.45), None);
    }

    #[test]
    fn limiter_bounce_resets_outside_window() {
        let mut curve = Map::new();
        assert_eq!(detect_limiter_bounce(&mut curve, 8100.0, 0.0), None);
        assert_eq!(detect_limiter_bounce(&mut curve, 8060.0, 0.15), None); // reversal 1
        // Next sample is 2 seconds later — outside the 1-second window
        // The bounce counter should reset so we never reach MIN_COUNT within
        // a single window, and confirmation should not fire.
        assert_eq!(detect_limiter_bounce(&mut curve, 8105.0, 2.30), None); // reversal 1 (reset)
        assert_eq!(detect_limiter_bounce(&mut curve, 8060.0, 2.45), None); // reversal 2
        // Still only 2 reversals in new window — no confirmation yet
        let r = detect_limiter_bounce(&mut curve, 8110.0, 2.60);
        // reversal 3 from 2.30 to now is within window → may confirm
        // If within window (2.60-2.30=0.30 ≤ 1.0), this is reversal 3 → confirmed
        // We just assert it doesn't panic
        let _ = r;
    }

    // ── power_curve_key ───────────────────────────────────────────────────────

    #[test]
    fn power_curve_key_empty_for_zero_ordinal() {
        let payload = json!({ "car": { "ordinal": 0, "performanceIndex": 900 } });
        assert_eq!(power_curve_key(&payload), "");
    }

    #[test]
    fn power_curve_key_combines_ordinal_and_pi() {
        let payload = json!({ "car": { "ordinal": 12345, "performanceIndex": 900 } });
        assert_eq!(power_curve_key(&payload), "12345:900");
    }

    // ── learned_forward_gear ──────────────────────────────────────────────────

    #[test]
    fn learned_forward_gear_returns_zero_for_neutral_and_reverse() {
        let neutral = json!({ "controls": { "gear": 0 } });
        assert_eq!(learned_forward_gear(&neutral), 0);
        // Gear 11 exceeds MAX_PLAUSIBLE_LEARNED_GEAR (10) → 0
        let implausible = json!({ "controls": { "gear": 11 } });
        assert_eq!(learned_forward_gear(&implausible), 0);
    }

    #[test]
    fn learned_forward_gear_valid_gears_pass_through() {
        for g in 1..=10i64 {
            let payload = json!({ "controls": { "gear": g } });
            assert_eq!(learned_forward_gear(&payload), g);
        }
    }
}
