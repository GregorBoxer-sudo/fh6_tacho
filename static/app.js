const $ = (id) => document.getElementById(id);

const els = {
  screen: document.querySelector(".screen"),
  status: $("statusDot"),
  connection: $("connection"),
  latency: $("latency"),
  speed: $("speed"),
  speedUnit: $("speedUnit"),
  gear: $("gear"),
  driftAngle: $("driftAngle"),
  driftPeak: $("driftPeak"),
  driftMarker: $("driftMarker"),
  rpm: $("rpm"),
  rpmValue: $("rpmValue"),
  rpmMax: $("rpmMax"),
  rpmFill: $("rpmFill"),
  drivetrain: $("drivetrain"),
  accel: $("accel"),
  accelBar: $("accelBar"),
  brake: $("brake"),
  brakeBar: $("brakeBar"),
  steer: $("steer"),
  steerMarker: $("steerMarker"),
  power: $("power"),
  torque: $("torque"),
  boost: $("boost"),
  lapNumber: $("lapNumber"),
  position: $("position"),
  lapTime: $("lapTime"),
  bestLap: $("bestLap"),
  lapDelta: $("lapDelta"),
  lapProgress: $("lapProgress"),
  progressBar: $("progressBar"),
  gDot: $("gDot"),
  gForce: $("gForce"),
  gPeak: $("gPeak"),
  gLat: $("gLat"),
  gLong: $("gLong"),
  fuel: $("fuel"),
  understeerWarn: $("understeerWarn"),
  oversteerWarn: $("oversteerWarn"),
  tempWarn: $("tempWarn"),
  overlapWarn: $("overlapWarn"),
  absWarn: $("absWarn"),
  brakeLockWarn: $("brakeLockWarn"),
  clutchWarn: $("clutchWarn"),
  handbrakeWarn: $("handbrakeWarn"),
  tires: {
    fl: $("tireFL"),
    fr: $("tireFR"),
    rl: $("tireRL"),
    rr: $("tireRR"),
  },
  slipLeds: {
    fl: Array.from(document.querySelectorAll(".slipFL i")),
    fr: Array.from(document.querySelectorAll(".slipFR i")),
    rl: Array.from(document.querySelectorAll(".slipRL i")),
    rr: Array.from(document.querySelectorAll(".slipRR i")),
  },
  shiftLeds: Array.from(document.querySelectorAll(".shift .led")),
};

let latest = null;
let lastRender = 0;
let lastMessageAt = 0;
let connected = false;
let lastGearText = "0";
let learnedMaxRpm = 0;
let rpmWasReset = false;
let peakG = 0;
let peakDrift = 0;
let gLongBaseline = null;
let learnedLapDistance = 0;
let lastLapNumber = 0;
let lastLapDistance = 0;
let lapStartDistance = 0;
let pauseStartAt = 0;
let zeroStartAt = 0;
let shiftFlashActive = false;
let shiftFlashGear = 0;

const PEAK_G_DECAY = 0.995;
const PEAK_DRIFT_DECAY = 0.998;
const DEFAULT_ENGINE_LIMIT_RATIO_OF_TACHO_MAX = 0.895;
const REDLINE_RATIO_OF_ENGINE_LIMIT = 1 - 500 / (9000 - 750);
const SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT = 0.995;
const LOW_GEAR_SAFETY_SHIFT_TARGET_RATIOS = {
  1: 0.98,
  2: 0.985,
  3: 0.99,
};
const SHIFT_WARNING_LEAD_SECONDS = 0.20;
const LOW_GEAR_SHIFT_WARNING_LEAD_SECONDS = {
  1: 0.22,
  2: 0.22,
  3: 0.20,
};
const SHIFT_WARNING_FALLBACK_GAP_RATIO = 0.012;
const SHIFT_WARNING_MIN_GAP_RPM = 100;
const SHIFT_WARNING_MAX_FALLBACK_GAP_RPM = 220;
const SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM = 800;
const SAFETY_SHIFT_WARNING_FALLBACK_BAND_RATIO = 0.065;
const SAFETY_SHIFT_WARNING_MAX_BAND_RATIO = 0.18;
const SHIFT_WARNING_MIN_RPM_RATE = 350;
const SHIFT_WARNING_MAX_RPM_RATE = 14000;

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function int(value) {
  return Math.round(Number.isFinite(value) ? value : 0).toString();
}

function pct(value) {
  return `${Math.round(clamp(value || 0, 0, 1) * 100)}%`;
}

function fuelText(value) {
  const fuel = Number(value);
  if (!Number.isFinite(fuel) || fuel < 0) return "--%";
  return fuel <= 1.2 ? `${Math.round(clamp(fuel, 0, 1) * 100)}%` : fuel.toFixed(1);
}

function lapTime(value, allowZero = false) {
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds < 0 || (!allowZero && seconds === 0)) {
    return "--:--.---";
  }
  const minutes = Math.floor(seconds / 60);
  const rest = seconds - minutes * 60;
  return `${minutes.toString().padStart(2, "0")}:${rest.toFixed(3).padStart(6, "0")}`;
}

function displayLapNumber(data) {
  // Forza delivers lap.number 0-based (Lap 1 = 0, Lap 2 = 1, ...)
  const rawLap = Number(data.lap?.number) ?? -1;
  if (rawLap >= 0 && data.raceOn) return int(rawLap + 1);
  const currentTime = Number(data.lap?.current) || 0;
  const distance = Number(data.lap?.distance) || 0;
  const moving = (data.speed?.kmh || 0) > 1 || distance > 1;
  return data.raceOn && currentTime > 0 && moving ? "1" : "--";
}

function hasRaceData(data) {
  const lap = data.lap || {};
  return Boolean(
    data.raceOn
    && (
      (Number(lap.position) || 0) > 0
      || (Number(lap.number) || 0) > 0
      || (Number(lap.current) || 0) > 0
      || Math.abs(Number(lap.distance) || 0) > 1
    )
  );
}

function signedSeconds(value) {
  if (!Number.isFinite(value)) return "--";
  const sign = value > 0 ? "+" : value < 0 ? "-" : "+";
  return `${sign}${Math.abs(value).toFixed(2)}`;
}

function signedDeg(value) {
  return `${Math.abs(value).toFixed(0)} deg`;
}

function gearLabel(gear) {
  if (gear === 0) {
    lastGearText = "R";
    return lastGearText;
  }
  if (gear >= 1 && gear <= 10) {
    lastGearText = String(gear);
    return lastGearText;
  }
  return lastGearText;
}

function safetyShiftTargetRatio(gear) {
  return LOW_GEAR_SAFETY_SHIFT_TARGET_RATIOS[gear] || SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT;
}

function shiftWarningLeadSeconds(gear) {
  return LOW_GEAR_SHIFT_WARNING_LEAD_SECONDS[gear] || SHIFT_WARNING_LEAD_SECONDS;
}

function fallbackShiftWarningGap(shiftRpm) {
  return clamp(
    shiftRpm * SHIFT_WARNING_FALLBACK_GAP_RATIO,
    SHIFT_WARNING_MIN_GAP_RPM,
    SHIFT_WARNING_MAX_FALLBACK_GAP_RPM,
  );
}

function dynamicShiftWarningRpm(shiftRpm, rpmRate, leadSeconds) {
  if (shiftRpm <= 0) return 0;
  let gap = fallbackShiftWarningGap(shiftRpm);
  if (rpmRate >= SHIFT_WARNING_MIN_RPM_RATE && rpmRate <= SHIFT_WARNING_MAX_RPM_RATE) {
    gap = clamp(
      rpmRate * leadSeconds,
      SHIFT_WARNING_MIN_GAP_RPM,
      SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM,
    );
  }
  return Math.max(0, shiftRpm - gap);
}

function safetyShiftWarningRpm(shiftRpm, rpmRate, leadSeconds, idleRpm) {
  if (shiftRpm <= 0) return 0;
  const usableBand = Math.max(1000, shiftRpm - Math.max(0, idleRpm || 0));
  const maxGap = clamp(
    usableBand * SAFETY_SHIFT_WARNING_MAX_BAND_RATIO,
    SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM,
    usableBand * 0.35,
  );
  // Fallback band gap as a floor for the dynamic gap:
  // At slow rev build-up (small rpm_rate), clamp(rate*lead, 100, max)
  // would collapse to only 100 RPM — dangerously close to the real limiter.
  // The fallback band gap is proportional to the usable RPM band and therefore
  // always a sensible minimum distance.
  const fallbackGap = clamp(
    usableBand * SAFETY_SHIFT_WARNING_FALLBACK_BAND_RATIO,
    SHIFT_WARNING_MIN_GAP_RPM,
    maxGap,
  );
  let gap = fallbackGap;
  if (rpmRate >= SHIFT_WARNING_MIN_RPM_RATE && rpmRate <= SHIFT_WARNING_MAX_RPM_RATE) {
    gap = clamp(rpmRate * leadSeconds, fallbackGap, maxGap);
  }
  return Math.max(0, shiftRpm - gap);
}

function setConnection(live) {
  if (live === connected) return;
  connected = live;
  els.status.classList.toggle("live", live);
  els.connection.textContent = live ? "live" : "no data";
}

function updateTire(el, temp) {
  el.querySelector("strong").textContent = conv.fmtTemp(temp, 0);
}

function updateSlipLeds(leds, combinedSlip) {
  const value = clamp(Math.abs(combinedSlip || 0) / 1.2, 0, 1);
  const active = Math.max(1, Math.round(value * leds.length));
  leds.forEach((led, index) => {
    led.classList.toggle("active", index >= leds.length - active);
  });
}

function updateWarnings(data) {
  const frontSlip = Math.max(Math.abs(data.tireCombinedSlip.fl || 0), Math.abs(data.tireCombinedSlip.fr || 0));
  const rearSlip = Math.max(Math.abs(data.tireCombinedSlip.rl || 0), Math.abs(data.tireCombinedSlip.rr || 0));
  const hottestTire = Math.max(data.tireTempC.fl || 0, data.tireTempC.fr || 0, data.tireTempC.rl || 0, data.tireTempC.rr || 0);
  const understeer = frontSlip > 0.62 && frontSlip > rearSlip + 0.18 && Math.abs(data.controls.steer || 0) > 0.25;
  const oversteer = rearSlip > 0.68 && rearSlip > frontSlip + 0.2 && Math.abs(data.controls.steer || 0) > 0.18;
  const brake = data.controls.brake || 0;
  const throttle = data.controls.accel || 0;
  const brakeLock = brake > 0.58 && frontSlip > 0.72;
  const absActive = brake > 0.42 && frontSlip > 0.42;
  const overlap = brake > 0.08 && throttle > 0.08;

  els.understeerWarn.classList.toggle("active", understeer);
  els.oversteerWarn.classList.toggle("active", oversteer);
  els.tempWarn.classList.toggle("active", hottestTire >= 110); // °C — grip degrades above ~110 °C
  els.overlapWarn.classList.toggle("active", overlap);
  els.absWarn.classList.toggle("active", absActive);
  els.brakeLockWarn.classList.toggle("active", brakeLock);
  els.clutchWarn.classList.toggle("active", (data.controls.clutch || 0) > 0.08);
  els.handbrakeWarn.classList.toggle("active", (data.controls.handbrake || 0) > 0.08);
}

function resetLearnedRpm() {
  learnedMaxRpm = 0;
  rpmWasReset = true;
  els.rpmFill.style.width = "0%";
  els.shiftLeds.forEach((led) => {
    led.classList.remove("active");
    led.classList.remove("shiftNow");
  });
}

function rpmScale(engine) {
  const rpm = Math.max(0, engine.rpm || 0);
  if (rpm > learnedMaxRpm) {
    learnedMaxRpm = rpm;
    rpmWasReset = false;
  }

  const idle = Math.max(0, engine.idleRpm || 0);
  const telemetryMax = Math.max(0, engine.maxRpm || 0);
  const fallback = Math.max(3000, idle * 2.5, rpm);
  const maxRpm = telemetryMax >= 3000 ? telemetryMax : Math.max(learnedMaxRpm, fallback);
  const telemetryLimit = Math.max(0, engine.limitRpm || 0);
  const telemetryRedline = Math.max(0, engine.redlineRpm || 0);
  const limitRpm = telemetryLimit > idle + 1000 ? telemetryLimit : Math.max(idle + 1000, maxRpm * DEFAULT_ENGINE_LIMIT_RATIO_OF_TACHO_MAX);
  const redlineRpm = telemetryRedline > idle + 1000 ? telemetryRedline : Math.max(idle + 1000, limitRpm * REDLINE_RATIO_OF_ENGINE_LIMIT);
  const telemetryShiftNow = Number(engine.shiftNowRpm);
  const gear = Number(latest?.controls?.gear) || 0;
  const rpmRate = Number(engine.rpmRiseRate) || 0;
  const fallbackShiftTargetRpm = limitRpm * safetyShiftTargetRatio(gear);
  const shiftNowRpm = Number.isFinite(telemetryShiftNow) && telemetryShiftNow > idle + 1000
    ? telemetryShiftNow
    : safetyShiftWarningRpm(fallbackShiftTargetRpm, rpmRate, shiftWarningLeadSeconds(gear), idle);
  return {
    maxRpm,
    limitRpm,
    redlineRpm,
    shiftNowRpm,
    ratio: maxRpm > 0 ? clamp(rpm / maxRpm, 0, 1) : 0,
    redlineRatio: redlineRpm > 0 ? clamp(rpm / redlineRpm, 0, 1.2) : 0,
  };
}

function shiftFlashState(currentRpm, shiftNowRpm, gear) {
  if (!Number.isFinite(currentRpm) || !Number.isFinite(shiftNowRpm) || shiftNowRpm <= 0 || gear <= 0) {
    shiftFlashActive = false;
    shiftFlashGear = gear;
    return false;
  }
  if (gear !== shiftFlashGear) {
    shiftFlashActive = false;
    shiftFlashGear = gear;
  }
  const releaseGap = clamp(shiftNowRpm * 0.025, 180, 350);
  if (!shiftFlashActive && currentRpm >= shiftNowRpm) {
    shiftFlashActive = true;
  } else if (shiftFlashActive && currentRpm < shiftNowRpm - releaseGap) {
    shiftFlashActive = false;
  }
  return shiftFlashActive;
}

function updateGMeter(motion, speed) {
  const kmh = speed?.kmh || 0;
  const isStationary = kmh < 2;

  const rawLat  = motion?.gLat  || 0;
  const rawLong = motion?.gLong || 0;

  // Update the longitudinal baseline to cancel Forza's static gravity/slope offset.
  // While stationary: converge quickly (~15 % per frame at 60 Hz ≈ ~1 s to settle).
  // While moving: only correct slowly when the value is already close to baseline,
  // so genuine acceleration isn't silently zeroed out.
  if (gLongBaseline === null) {
    gLongBaseline = rawLong;
  } else if (isStationary) {
    gLongBaseline = gLongBaseline * 0.85 + rawLong * 0.15;
  } else if (Math.abs(rawLong - gLongBaseline) < 0.08) {
    gLongBaseline = gLongBaseline * 0.995 + rawLong * 0.005;
  }

  // Below 2 km/h the car is stationary — clamp both axes to zero so gravity
  // noise and Forza's static bias never show as phantom G-force.
  const lat  = isStationary ? 0 : clamp(rawLat,               -2.5, 2.5);
  const long = isStationary ? 0 : clamp(rawLong - gLongBaseline, -2.5, 2.5);
  const total = Math.hypot(lat, long);
  peakG = Math.max(total, peakG * PEAK_G_DECAY);

  els.gDot.style.left = `${50 - lat * 19.2}%`;
  els.gDot.style.top = `${50 + long * 19.2}%`;
  els.gForce.textContent = `${total.toFixed(2)}g`;
  els.gPeak.textContent = `${peakG.toFixed(2)}p`;
  els.gLat.textContent = lat.toFixed(2);
  els.gLong.textContent = long.toFixed(2);
}

function updateDrift(data) {
  const speed = data.speed?.kmh || 0;
  const combinedSlip = data.tireCombinedSlip || {};
  const slipRatio = data.tireSlipRatio || {};
  const frontSlip = Math.max(Math.abs(combinedSlip.fl || 0), Math.abs(combinedSlip.fr || 0));
  const rearSlip = Math.max(Math.abs(combinedSlip.rl || 0), Math.abs(combinedSlip.rr || 0));
  const frontRatio = Math.max(Math.abs(slipRatio.fl || 0), Math.abs(slipRatio.fr || 0));
  const rearRatio = Math.max(Math.abs(slipRatio.rl || 0), Math.abs(slipRatio.rr || 0));
  const rearGripLoss = Math.max(rearSlip, rearRatio);
  const frontGripLoss = Math.max(frontSlip, frontRatio);
  const anyGripLoss = Math.max(rearGripLoss, frontGripLoss);
  const driftRaw = Number(data.motion?.driftAngleDeg);
  const drivetrainId = Number(data.car?.drivetrainId);
  let driftActive = anyGripLoss > 0.5;
  if (drivetrainId === 1) {
    driftActive = rearGripLoss > frontGripLoss + 0.3;
  } else if (drivetrainId === 2) {
    driftActive = anyGripLoss > 0.6;
  } else if (drivetrainId === 0) {
    driftActive = rearGripLoss > 0.5 || (data.controls?.handbrake || 0) > 0.08;
  }
  let angle = speed > 7 && Number.isFinite(driftRaw) && Math.abs(driftRaw) > 5 && driftActive ? driftRaw : 0;
  angle = clamp(angle, -90, 90);
  peakDrift = Math.max(Math.abs(angle), peakDrift * PEAK_DRIFT_DECAY);

  els.driftAngle.textContent = signedDeg(angle);
  els.driftPeak.textContent = `${peakDrift.toFixed(0)}p`;
  els.driftMarker.style.left = `${(clamp(angle, -90, 90) / 90 + 1) * 50}%`;
}

function updateLapDerived(lap) {
  const lapNumber = Number(lap.number) || 0;
  const distance = Math.max(0, Number(lap.distance) || 0);

  // New race: lapNumber switches from >0 back to 0 → reset state.
  // Without this reset, lapStartDistance would carry over from the previous race
  // and currentLapDistance would be 0 → progress=0 → delta grows monotonically from 0 to ~lap time.
  if (lastLapNumber > 0 && lapNumber === 0) {
    lastLapNumber = 0;
    lastLapDistance = 0;
    lapStartDistance = 0;
  }

  // Init: first recognisable lap after race start (lapNumber changes from 0 to >0).
  // lapNumber===0 is Forza lap 1; lapNumber===1 is lap 2, etc.
  // Lap 0 needs no explicit init (lapStartDistance=0, distance=0 → correct).
  if (lapNumber > 0 && lastLapNumber === 0) {
    lastLapNumber = lapNumber;
    lapStartDistance = distance;
  }

  if (lastLapDistance > 100 && distance + 50 < lastLapDistance) {
    const lapLength = Math.max(lastLapDistance - lapStartDistance, lastLapDistance);
    learnedLapDistance = Math.max(learnedLapDistance, lapLength);
    lapStartDistance = 0;
  } else if (lapNumber > 0 && lastLapNumber > 0 && lapNumber !== lastLapNumber) {
    const lapLength = Math.max(0, distance - lapStartDistance);
    if (lapLength > 100) {
      learnedLapDistance = Math.max(learnedLapDistance, lapLength);
    }
    lapStartDistance = distance;
  }

  lastLapNumber = lapNumber;
  lastLapDistance = distance;

  const currentLapDistance = Math.max(0, distance - lapStartDistance);
  const hasLapLength = learnedLapDistance > 100;
  const progress = hasLapLength ? clamp(currentLapDistance / learnedLapDistance, 0, 1) : 0;
  els.lapProgress.textContent = hasLapLength ? `${Math.round(progress * 100)}%` : "--%";
  els.progressBar.style.width = `${progress * 100}%`;

  const best = Number(lap.best) || 0;
  const current = Number(lap.current) || 0;
  const delta = hasLapLength && best > 0 && current >= 0 ? current - best * progress : NaN;
  els.lapDelta.textContent = signedSeconds(delta);
  els.lapDelta.classList.toggle("good", Number.isFinite(delta) && delta <= 0);
  els.lapDelta.classList.toggle("bad", Number.isFinite(delta) && delta > 0);
}

function render(now) {
  requestAnimationFrame(render);
  if (!latest || now - lastRender < 16) return;
  lastRender = now;

  const age = lastMessageAt ? performance.now() - lastMessageAt : Infinity;
  const raceDataActive = hasRaceData(latest);
  setConnection(age < 800 && latest.raceOn);
  els.latency.textContent = `${Math.max(0, Math.round(age))} ms`;
  els.screen.classList.toggle("raceOff", !raceDataActive);

  els.speed.textContent = Math.round(conv.speed(latest.speed.kmh));
  if (els.speedUnit) els.speedUnit.textContent = conv.speedLabel();
  els.gear.textContent = gearLabel(latest.controls.gear);
  els.rpm.textContent = int(latest.engine.rpm);
  els.rpmValue.textContent = int(latest.engine.rpm);

  const rpm = rpmScale(latest.engine);
  els.rpmMax.textContent = `/ ${int(rpm.maxRpm)} rpm`;
  const rpmRatio = rpm.ratio;
  els.rpmFill.style.width = `${rpmRatio * 100}%`;
  els.rpmFill.style.filter = rpmRatio > 0.92 ? "saturate(1.7) brightness(1.2)" : "";
  // LEDs scale to 97 % of shiftNowRpm so the last LED lights up just before
  // blinking starts (brief "all on, not yet blinking" state).
  const ledFull = rpm.shiftNowRpm > 0 ? rpm.shiftNowRpm * 0.97 : 0;
  const ledRatio = ledFull > 0
    ? clamp(latest.engine.rpm / ledFull, 0, 1)
    : clamp(rpm.redlineRatio, 0, 1);
  const activeLeds = Math.round(ledRatio * els.shiftLeds.length);
  const shiftFlash = shiftFlashState(latest.engine.rpm, rpm.shiftNowRpm, latest.controls.gear);
  checkShiftAudio();
  const shiftFlashOn = shiftFlash && Math.floor(now / 80) % 2 === 0;
  els.shiftLeds.forEach((led, index) => {
    led.classList.toggle("active", index < activeLeds);
    led.classList.toggle("shiftNow", shiftFlash);
    led.classList.toggle("shiftFlashOn", shiftFlashOn);
  });

  els.drivetrain.textContent = latest.car.drivetrain;
  els.fuel.textContent = fuelText(latest.fuel);
  els.accel.textContent = pct(latest.controls.accel);
  els.accelBar.style.width = pct(latest.controls.accel);
  els.brake.textContent = pct(latest.controls.brake);
  els.brakeBar.style.width = pct(latest.controls.brake);

  const steer = clamp(latest.controls.steer || 0, -1, 1);
  els.steer.textContent = steer.toFixed(2);
  els.steerMarker.style.left = `${(steer + 1) * 50}%`;

  els.power.textContent = `${int(latest.engine.powerHp)} hp`;
  els.torque.textContent = conv.fmtTorque(latest.engine.torqueNm);
  els.boost.textContent = latest.boost.toFixed(2);
  els.lapNumber.textContent = displayLapNumber(latest);
  els.position.textContent = latest.lap.position > 0 ? int(latest.lap.position) : "--";
  els.lapTime.textContent = lapTime(latest.lap.current, true);
  els.bestLap.textContent = lapTime(latest.lap.best);
  updateLapDerived(latest.lap);
  updateGMeter(latest.motion, latest.speed);
  updateDrift(latest);

  updateTire(els.tires.fl, latest.tireTempC.fl);
  updateTire(els.tires.fr, latest.tireTempC.fr);
  updateTire(els.tires.rl, latest.tireTempC.rl);
  updateTire(els.tires.rr, latest.tireTempC.rr);
  updateSlipLeds(els.slipLeds.fl, latest.tireCombinedSlip.fl);
  updateSlipLeds(els.slipLeds.fr, latest.tireCombinedSlip.fr);
  updateSlipLeds(els.slipLeds.rl, latest.tireCombinedSlip.rl);
  updateSlipLeds(els.slipLeds.rr, latest.tireCombinedSlip.rr);
  updateWarnings(latest);
}

function connect() {
  const source = new EventSource("/events");
  source.addEventListener("open", () => {
    els.connection.textContent = "connected";
  });
  source.addEventListener("telemetry", (event) => {
    latest = JSON.parse(event.data);
    lastMessageAt = performance.now();
  });
  source.addEventListener("error", () => {
    setConnection(false);
  });
}

setInterval(() => {
  const now = performance.now();
  const silenceMs = lastMessageAt ? performance.now() - lastMessageAt : Infinity;
  if (!lastMessageAt || silenceMs > 1200) {
    setConnection(false);
  }

  if (latest && !latest.raceOn) {
    if (!pauseStartAt) pauseStartAt = now;
  } else {
    pauseStartAt = 0;
  }

  const zeroTelemetry = latest
    && Math.abs(latest.speed?.kmh || 0) < 1
    && Math.abs(latest.engine?.rpm || 0) < 50
    && Math.abs(latest.controls?.accel || 0) < 0.01
    && Math.abs(latest.controls?.brake || 0) < 0.01;
  if (zeroTelemetry) {
    if (!zeroStartAt) zeroStartAt = now;
  } else {
    zeroStartAt = 0;
  }

  const pausedMs = pauseStartAt ? now - pauseStartAt : 0;
  const zeroMs = zeroStartAt ? now - zeroStartAt : 0;
  if ((silenceMs > 15000 || pausedMs > 15000 || zeroMs > 15000) && !rpmWasReset) {
    resetLearnedRpm();
  }
}, 250);

// ── Web Audio shift-sound engine ──────────────────────────────────────────────

let _audioCtx = null;

/** Create/return a shared AudioContext, resuming it if suspended. */
async function getAudioCtx() {
  if (!_audioCtx) {
    try { _audioCtx = new (window.AudioContext || window.webkitAudioContext)(); } catch (_) {}
  }
  if (_audioCtx?.state === "suspended") {
    try { await _audioCtx.resume(); } catch (_) {}
  }
  return _audioCtx;
}
// Unlock on any user interaction (needed on iOS and Chrome autoplay policy).
document.addEventListener("click",   () => getAudioCtx(), { once: true });
document.addEventListener("keydown", () => getAudioCtx(), { once: true });

/**
 * Synthesise and play a named shift sound through the browser's audio output.
 * Valid names: blip | click | beep | chord | buzz.  "none" is silently ignored.
 */
async function playShiftSoundWeb(name) {
  if (!name || name === "none") return;
  const ctx = await getAudioCtx();
  if (!ctx) return;
  try {
    const sr = ctx.sampleRate;
    let buf;

    if (name === "blip") {
      // Falling-pitch sawtooth chirp: 1 400 → 800 Hz, 70 ms, exp decay
      const n = Math.floor(sr * 0.070);
      buf = ctx.createBuffer(1, n, sr);
      const d = buf.getChannelData(0);
      let phase = 0;
      for (let i = 0; i < n; i++) {
        const t = i / sr;
        phase += (1400 - 600 * (t / 0.070)) / sr;
        d[i] = (phase % 1) * 2 - 1;
        d[i] *= Math.exp(-t * 40) * 0.35;
      }

    } else if (name === "click") {
      // Sharp square burst at 900 Hz, 30 ms, very fast decay
      const n = Math.floor(sr * 0.030);
      buf = ctx.createBuffer(1, n, sr);
      const d = buf.getChannelData(0);
      let phase = 0;
      for (let i = 0; i < n; i++) {
        const t = i / sr;
        phase += 900 / sr;
        d[i] = (phase % 1 < 0.5 ? 1 : -1) * Math.exp(-t * 70) * 0.28;
      }

    } else if (name === "beep") {
      // Clean sine at 1 200 Hz, 90 ms
      const n = Math.floor(sr * 0.090);
      buf = ctx.createBuffer(1, n, sr);
      const d = buf.getChannelData(0);
      for (let i = 0; i < n; i++) {
        const t = i / sr;
        d[i] = Math.sin(2 * Math.PI * 1200 * t) * Math.exp(-t * 30) * 0.30;
      }

    } else if (name === "chord") {
      // Major triad A4+C5+E5 (440+523+659 Hz), 80 ms
      const freqs = [440, 523, 659];
      const n = Math.floor(sr * 0.080);
      buf = ctx.createBuffer(1, n, sr);
      const d = buf.getChannelData(0);
      for (let i = 0; i < n; i++) {
        const t = i / sr;
        d[i] = freqs.reduce((s, f) => s + Math.sin(2 * Math.PI * f * t), 0)
               / freqs.length * Math.exp(-t * 28) * 0.32;
      }

    } else if (name === "buzz") {
      // Low sawtooth buzz at 220 Hz, 55 ms
      const n = Math.floor(sr * 0.055);
      buf = ctx.createBuffer(1, n, sr);
      const d = buf.getChannelData(0);
      let phase = 0;
      for (let i = 0; i < n; i++) {
        const t = i / sr;
        phase += 220 / sr;
        d[i] = ((phase % 1) * 2 - 1) * Math.exp(-t * 30) * 0.30;
      }
    }

    if (buf) {
      const src = ctx.createBufferSource();
      src.buffer = buf;
      src.connect(ctx.destination);
      src.start();
    }
  } catch (_) {}
}

/** Preview a sound on the backend device by calling the server. */
async function previewBackendSound(name) {
  if (!name || name === "none") return;
  try {
    await fetch("/api/shift-sound/preview", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sound: name }),
    });
  } catch (_) {}
}

// ── Settings panel ────────────────────────────────────────────────────────────

function buildSoundSelect(id, currentValue, onChange) {
  const sel = $(`snd_${id}`);
  if (!sel) return;
  // Populate options
  sel.innerHTML = "";
  const labels = { none: "None", blip: "Blip", click: "Click", beep: "Beep", chord: "Chord", buzz: "Buzz" };
  for (const name of SOUND_NAMES) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = labels[name] || name;
    if (name === currentValue) opt.selected = true;
    sel.appendChild(opt);
  }
  sel.onchange = () => onChange(sel.value);
}

function refreshSoundSelects() {
  buildSoundSelect("web", shiftSoundWeb, async (val) => {
    shiftSoundWeb = val;
    await saveShiftSoundSettings(shiftSoundWeb, shiftSoundBackend);
    await playShiftSoundWeb(val);   // preview in this browser
  });
}

// Open / close
$("settingsBtn")?.addEventListener("click", (e) => {
  e.stopPropagation();
  const panel = $("settingsPanel");
  if (!panel) return;
  const open = panel.hasAttribute("hidden");
  if (open) {
    panel.removeAttribute("hidden");
    refreshSoundSelects();
  } else {
    panel.setAttribute("hidden", "");
  }
});

document.addEventListener("click", (e) => {
  const panel = $("settingsPanel");
  if (!panel || panel.hasAttribute("hidden")) return;
  if (!panel.contains(e.target) && e.target.id !== "settingsBtn") {
    panel.setAttribute("hidden", "");
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") $("settingsPanel")?.setAttribute("hidden", "");
});

// ── Shift trigger: fire sound on false → true edge ────────────────────────────

let _prevShiftFlash = false;

function checkShiftAudio() {
  const nowFlash = shiftFlashActive;
  if (!_prevShiftFlash && nowFlash) {
    if (shiftSoundWeb !== "none")     playShiftSoundWeb(shiftSoundWeb);
    // Backend fires autonomously from the Rust GUI; no HTTP call needed here.
  }
  _prevShiftFlash = nowFlash;
}

// ── Init ──────────────────────────────────────────────────────────────────────

// Load persisted unit preference; toggle button label stays in sync via syncUnitToggleBtns().
loadUnitSettings().then(syncUnitToggleBtns);
$("unitToggle")?.addEventListener("click", async () => {
  await saveUnitSettings(unitSystem === "metric" ? "imperial" : "metric");
  syncUnitToggleBtns();
  // The render() loop picks up the conv.* changes automatically on the next frame.
});

connect();
requestAnimationFrame(render);

if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {});
  });
}

// PWA navigation: open internal links in the same window (fix for iOS standalone mode)
document.addEventListener("click", (e) => {
  const a = e.target.closest("a[href]");
  if (!a) return;
  try {
    const url = new URL(a.href, location.origin);
    if (url.origin !== location.origin) return;
    e.preventDefault();
    location.href = a.href;
  } catch (_) {}
});
