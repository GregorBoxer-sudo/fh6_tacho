const $ = (id) => document.getElementById(id);
const DETAIL_MAP_COLOR_KEY = "forzaAnalyticsMapColorMode";
const DETAIL_MAP_COLOR_MODES = ["plain", "speed", "drift", "slip", "gLat", "gTotal"];

let sessions        = [];
let cars            = [];
let selectedSession = "";
let selectedCar     = "";
let currentCar      = null;   // car object for tab switching
let carPowerPoints  = [];     // preloaded curve data
let activeCarChart  = "shifts"; // "shifts" | "power"
let detailTrack     = [];
let detailTrackSrc  = "";
let detailSamples   = [];
let detailSummary   = null;   // cached for re-render on unit toggle
let mapColorMode    = "plain";
let mapCalibration  = { points: [], worldFlipX: false, worldFlipZ: false, mapImage: "map.jpg" };
let mapImage        = null;
let mapImageLoaded  = false;
let mapImageProcessed = null;
let mapDebugMode    = false;
let chartFilters    = { speed: true, rpm: true, accel: true, brake: true, gLat: false, gLong: false, drift: false };
let chartHoverIndex = -1;
let mapPendingWorld = null;
let mapLivePosition = null;
let mapLiveSource   = null;
let detailMapTransformCache = null;
let detailProjectedTrack = [];
let detailProjectionDirty = true;
let detailMapDrawQueued = false;
let replayIndex = 0;
let replayPlaying = false;
let replayRaf = 0;
let replayLastFrame = 0;
let replayAccumulator = 0; // accumulated session-time ms for timestamp-based replay
let chartView       = { startFrac: 0, endFrac: 1 };   // visible window [0,1] fractions
let chartDrag       = { active: false, moved: false, startX: 0, startStart: 0, spanFrac: 1 };
let detailMapView   = {
  scale: 1,
  tx: 0,
  ty: 0,
  initialized: false,
  pointer: {
    down: false,
    moved: false,
    startX: 0,
    startY: 0,
    startTx: 0,
    startTy: 0,
  },
};

function num(value, digits = 0) {
  const n = Number(value);
  return Number.isFinite(n) ? n.toFixed(digits) : (0).toFixed(digits);
}

function setDetailMapHint(text) {
  $("detailMapHint").textContent = text || "";
}

function invalidateDetailProjection() {
  detailMapTransformCache = null;
  detailProjectedTrack = [];
  detailProjectionDirty = true;
}

function scheduleDetailMapDraw() {
  if (detailMapDrawQueued) return;
  detailMapDrawQueued = true;
  requestAnimationFrame(() => {
    detailMapDrawQueued = false;
    drawDetailMap(detailTrack);
  });
}

function stopReplay() {
  replayPlaying = false;
  replayLastFrame = 0;
  replayAccumulator = 0;
  if (replayRaf) cancelAnimationFrame(replayRaf);
  replayRaf = 0;
  const btn = $("replayPlayBtn");
  if (btn) btn.textContent = "Play";
}

function updateReplayUi() {
  const slider = $("replaySlider");
  const label = $("replayLabel");
  if (!slider || !label) return;
  const max = Math.max(0, detailSamples.length - 1);
  replayIndex = clamp(replayIndex, 0, max);
  slider.max = String(max);
  slider.value = String(replayIndex);
  const sample = detailSamples[replayIndex] || {};
  label.textContent = timeText(sample.t || 0);
}

function setReplayIndex(index) {
  replayIndex = clamp(Math.round(index), 0, Math.max(0, detailSamples.length - 1));
  if (replayPlaying) ensureReplayVisible();
  updateReplayUi();
  drawSessionChart(detailSamples);
  scheduleDetailMapDraw();
}

// Timestamp-based replay: advances by real session-time elapsed × speed factor.
// Uses sample.t (seconds since session start) so playback is frame-rate independent
// and true-to-life at 1× speed regardless of how many samples there are.
function replayStep(now) {
  if (!replayPlaying) return;
  if (!replayLastFrame) {
    replayLastFrame = now;
    replayRaf = requestAnimationFrame(replayStep);
    return;
  }
  const speed = Number($("replaySpeed")?.value) || 1;
  // Cap frame delta at 100 ms so an invisible tab doesn't jump huge distances on resume.
  const wallElapsed = Math.min(100, now - replayLastFrame);
  replayLastFrame = now;
  // Accumulate session-time milliseconds.
  replayAccumulator += wallElapsed * speed;

  let idx = replayIndex;
  while (idx < detailSamples.length - 1) {
    const curT  = (detailSamples[idx]?.t     ?? 0) * 1000;
    const nextT = (detailSamples[idx + 1]?.t ?? 0) * 1000;
    const gap   = Math.max(1, nextT - curT);
    if (replayAccumulator < gap) break;
    replayAccumulator -= gap;
    idx++;
  }
  if (idx !== replayIndex) setReplayIndex(idx);
  if (replayIndex >= detailSamples.length - 1) { stopReplay(); return; }
  replayRaf = requestAnimationFrame(replayStep);
}

function toggleReplay() {
  if (detailSamples.length < 2) return;
  replayPlaying = !replayPlaying;
  $("replayPlayBtn").textContent = replayPlaying ? "Pause" : "Play";
  if (replayPlaying) {
    if (replayIndex >= detailSamples.length - 1) setReplayIndex(0);
    replayRaf = requestAnimationFrame(replayStep);
  } else {
    stopReplay();
  }
}

function configureReplayControls(enabled) {
  const controls = $("replayControls");
  if (!controls) return;
  controls.hidden = !enabled;
  stopReplay();
  replayIndex = 0;
  updateReplayUi();
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function quantile(sorted, q) {
  if (!sorted.length) return 0;
  const pos = clamp(q, 0, 1) * (sorted.length - 1);
  const lo = Math.floor(pos);
  const hi = Math.min(sorted.length - 1, lo + 1);
  const t = pos - lo;
  return sorted[lo] * (1 - t) + sorted[hi] * t;
}

function timeText(seconds) {
  const s = Math.max(0, Number(seconds) || 0);
  const m = Math.floor(s / 60);
  return `${m}:${(s - m * 60).toFixed(1).padStart(4, "0")}`;
}

async function loadJson(path) {
  const res = await fetch(path, { cache: "no-store" });
  return res.json();
}

async function refresh() {
  const [sessionData, carData, calibrationData] = await Promise.all([
    loadJson("/api/analytics/sessions"),
    loadJson("/api/analytics/cars"),
    loadJson("/api/map/calibration"),
  ]);
  sessions = sessionData.sessions || [];
  cars     = carData.cars         || [];
  mapDebugMode = !!calibrationData.debugMode;
  mapCalibration = {
    mapImage: typeof calibrationData.mapImage === "string" ? calibrationData.mapImage : "map.jpg",
    worldFlipX: !!calibrationData.worldFlipX,
    worldFlipZ: !!calibrationData.worldFlipZ,
    points: Array.isArray(calibrationData.points)
      ? calibrationData.points.filter((p) =>
          Number.isFinite(p?.world?.x) &&
          Number.isFinite(p?.world?.z) &&
          Number.isFinite(p?.pixel?.x) &&
          Number.isFinite(p?.pixel?.y)
        )
      : [],
  };
  invalidateDetailProjection();
  if (!mapDebugMode) {
    mapPendingWorld = null;
  }
  updateCalibrationDerivedState();
  renderPendingPoint();
  renderPointList();
  toggleCalibrationPane();
  refreshAxisButtons();
  if (mapDebugMode) {
    startMapLiveStream();
  } else {
    stopMapLiveStream();
  }
  ensureDetailMapImage();
  renderOverview();
  renderSessions();
  renderCars();
  if (!selectedSession && !selectedCar && sessions[0]) {
    selectSession(sessions[0].id);
  }
}

// ── Overview ────────────────────────────────────────────────────────────────

function renderOverview() {
  $("sessionCount").textContent = sessions.length;
  $("carCount").textContent     = cars.length;
  $("topSpeed").textContent     = conv.fmtSpeed(Math.max(0, ...sessions.map((s) => s.maxSpeed || 0)));
  $("topG").textContent         = `${num(Math.max(0, ...sessions.map((s) => s.maxAbsG  || 0)), 2)}g`;
  $("topPureLatG").textContent  = `${num(Math.max(0, ...sessions.map((s) => s.maxPureLatG || 0)), 2)}g`;
}

// ── Session List ────────────────────────────────────────────────────────────

function renderSessions() {
  $("sessionList").innerHTML = sessions.map((s) => {
    const raceTag = s.isRace ? `<span class="raceTag">Race</span>` : "";
    const lapNote = s.isRace && (s.lapTimes || []).length
      ? ` · ${s.lapTimes.length} laps` : "";
    const posNote = s.isRace && s.finishPosition > 0
      ? ` · P${s.finishPosition}` : "";
    return `
    <button class="item ${s.id === selectedSession ? "active" : ""}" data-session="${s.id}">
      <strong>${raceTag}${s.carKey || "Unknown"} · ${timeText(s.duration)}</strong>
      <small>${new Date((s.startedAt || 0) * 1000).toLocaleString()}</small>
      <em>${conv.fmtSpeed(s.maxSpeed)} · ${num(s.maxRpm)} rpm · ${num(s.maxAbsG, 2)}g · ${s.shiftCount || 0} shifts${lapNote}${posNote}</em>
    </button>
  `}).join("");
  document.querySelectorAll("[data-session]").forEach((btn) =>
    btn.addEventListener("click", () => selectSession(btn.dataset.session))
  );
}

// ── Car List ────────────────────────────────────────────────────────────────

function renderCars() {
  $("carList").innerHTML = cars.map((car) => {
    const shifts = Object.entries(car.shiftTargets || {})
      .sort(([a], [b]) => Number(a) - Number(b))
      .map(([g, rpm]) => `G${g}: ${Math.round(rpm)}`)
      .join("  ");
    return `
      <button class="item ${car.key === selectedCar ? "active" : ""}" data-car="${car.key}">
        <strong>${car.key} · ${car.class || "-"} ${car.pi || ""}</strong>
        <small>${car.drivetrain || "-"} · ${car.cylinders || 0} cyl. · ${car.sessions || 0} sessions</small>
        <em>${conv.fmtSpeed(car.maxSpeed)} · ${num(car.maxPower)} hp · ${num(car.observedRpm)} rpm</em>
        <em>${shifts || "no shift points yet"}</em>
      </button>
    `;
  }).join("");
  document.querySelectorAll("[data-car]").forEach((btn) =>
    btn.addEventListener("click", () => selectCar(btn.dataset.car))
  );
}

// ── Selection ───────────────────────────────────────────────────────────────

async function selectSession(id) {
  selectedSession = id;
  selectedCar     = "";
  currentCar      = null;
  renderSessions();
  renderCars();
  $("chartTabs").style.display = "none";
  $("chartFilters").hidden = false;
  $("detailMapSection").style.display = "";
  resetChartView();
  configureReplayControls(true);
  const [detail, trackData] = await Promise.all([
    loadJson(`/api/analytics/sessions/${encodeURIComponent(id)}`),
    loadJson(`/api/analytics/sessions/${encodeURIComponent(id)}/track`),
  ]);
  detailTrack    = Array.isArray(trackData.points) ? trackData.points : [];
  detailTrackSrc = typeof trackData.trackSource === "string" ? trackData.trackSource : "";
  invalidateDetailProjection();
  renderDetail(detail.summary || {}, detail.samples || []);
  scheduleDetailMapDraw();
}

async function selectCar(key) {
  selectedCar     = key;
  selectedSession = "";
  detailTrack     = [];
  detailTrackSrc  = "";
  invalidateDetailProjection();
  renderSessions();
  renderCars();

  currentCar     = cars.find((c) => c.key === key) || null;
  activeCarChart = "shifts";
  setActiveTab("shifts");
  $("chartTabs").style.display = "";
  $("chartFilters").hidden = true;
  $("detailMapSection").style.display = "none";
  const rd = $("raceDetail"); if (rd) rd.hidden = true;
  const csvB = $("csvDownloadBtn"); if (csvB) csvB.hidden = true;
  configureReplayControls(false);

  // Show detail immediately, curve loads in parallel
  if (currentCar) renderCarDetail(currentCar);

  const curveData = await loadJson(`/api/analytics/cars/${encodeURIComponent(key)}/powercurve`);
  carPowerPoints  = curveData.points || [];
  // If the power tab is already active, draw immediately
  if (activeCarChart === "power") drawCarPowerCurve(carPowerPoints);
}

// ── Tab Control (car context only) ──────────────────────────────────────────

function setActiveTab(chart) {
  activeCarChart = chart;
  document.querySelectorAll(".chartTab").forEach((btn) =>
    btn.classList.toggle("active", btn.dataset.chart === chart)
  );
}

document.querySelectorAll(".chartTab").forEach((btn) => {
  btn.addEventListener("click", () => {
    setActiveTab(btn.dataset.chart);
    if (activeCarChart === "shifts" && currentCar) drawCarChart(currentCar);
    if (activeCarChart === "power")                drawCarPowerCurve(carPowerPoints);
  });
});

// ── Session Detail ───────────────────────────────────────────────────────────

function renderDetail(summary, samples) {
  detailSamples = Array.isArray(samples) ? samples : [];
  detailSummary = summary || {};
  replayIndex = 0;
  resetChartView();

  const isRace = !!summary.isRace;
  const titleTag = isRace ? `<span class="raceTag">Race</span> ` : "";
  $("detailTitle").innerHTML = `${titleTag}${summary.carKey || "Session"} · ${timeText(summary.duration)}`;

  const stats = [];
  // Race-specific stats come first when this is a race session
  if (isRace) {
    stats.push(["Finish Pos.", summary.finishPosition > 0 ? `P${summary.finishPosition}` : "–"]);
    stats.push(["Best Lap",    summary.bestLap ? timeText(summary.bestLap) : "–"]);
    stats.push(["Laps",        (summary.lapTimes || []).length > 0
      ? String((summary.lapTimes || []).length) : "–"]);
  }
  stats.push(["Top Speed", conv.fmtSpeed(summary.maxSpeed)]);
  stats.push(["Max RPM",   `${num(summary.maxRpm)} rpm`]);
  stats.push(["Power",     `${num(summary.maxPower)} hp`]);
  stats.push(["Torque",    conv.fmtTorque(summary.maxTorque)]);
  stats.push(["Max G",     `${num(summary.maxAbsG, 2)}g`]);
  stats.push(["Pure Lat G",`${num(summary.maxPureLatG, 2)}g`]);
  stats.push(["Max Drift", `${num(summary.maxDrift, 1)}°`]);
  stats.push(["Shifts",    `${summary.shiftCount || 0}`]);

  $("detailStats").innerHTML = stats.map(([l, v]) =>
    `<div class="stat"><span>${l}</span><strong>${v}</strong></div>`
  ).join("");
  $("lapInfo").innerHTML = `
    <span>Class ${summary.class || "-"}</span>
    <span>${summary.drivetrain || "-"}</span>
    <span>${summary.cylinders || 0} cyl.</span>
    <span>Ø Throttle ${num((summary.avgThrottle || 0) * 100)}%</span>
    <span>Ø Brake ${num((summary.avgBrake || 0) * 100)}%</span>
    ${!isRace ? `<span>Best Lap ${summary.bestLap ? timeText(summary.bestLap) : "--"}</span>` : ""}
    <span>Track Src ${detailTrackSrc || "-"}</span>
  `;
  renderRaceDetail(summary);
  // CSV download button
  const csvBtn = $("csvDownloadBtn");
  if (csvBtn) {
    const safeId = encodeURIComponent(summary.id || selectedSession);
    csvBtn.href     = `/api/analytics/sessions/${safeId}/csv`;
    csvBtn.download = `${(summary.carKey || "session").replace(/[:/]/g, "_")}_${safeId}.csv`;
    csvBtn.hidden   = false;
  }
  updateReplayUi();
  drawSessionChart(detailSamples);
}

// ── Race Detail (lap times + race stats) ─────────────────────────────────────

function renderRaceDetail(summary) {
  const el = $("raceDetail");
  if (!el) return;
  const laps = summary?.lapTimes || [];
  if (!summary?.isRace || !laps.length) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  const best = Math.min(...laps);
  const rows = laps.map((t, i) => {
    const delta  = t - best;
    const isBest = delta < 0.001;
    const sign   = delta >= 0 ? "+" : "";
    const dText  = isBest ? "BEST" : `${sign}${delta.toFixed(3)}s`;
    const dClass = isBest ? "lapBestMark" : (delta > 0 ? "lapSlower" : "");
    return `<div class="lapRow${isBest ? " lapBest" : ""}">
      <span class="lapNum">Lap ${i + 1}</span>
      <span class="lapTime">${timeText(t)}</span>
      <span class="lapDelta ${dClass}">${dText}</span>
    </div>`;
  }).join("");

  const posLine = summary.finishPosition > 0
    ? `<span>Finish: <strong>P${summary.finishPosition}</strong></span>  ` : "";
  const lapsLine = `<span>${laps.length} completed lap${laps.length !== 1 ? "s" : ""}</span>`;

  el.innerHTML = `
    <div class="raceDetailHead">${posLine}${lapsLine}</div>
    <div class="lapGrid">${rows}</div>
  `;
}

// ── Car Detail ───────────────────────────────────────────────────────────────

function renderCarDetail(car) {
  $("detailTitle").textContent = `${car.key} · ${car.class || "-"} ${car.pi ? "PI " + car.pi : ""}`;
  const stats = [
    ["Top Speed", conv.fmtSpeed(car.maxSpeed)],
    ["Max Power", `${num(car.maxPower)} hp`],
    ["Obs. RPM",  `${num(car.observedRpm)} rpm`],
    ["Sessions",  `${car.sessions || 0}`],
    ["Drivetrain", car.drivetrain || "-"],
    ["Cylinders",  `${car.cylinders || 0}`],
    ["Max Gear",   `${car.maxGear || "-"}`],
    ["PI",        `${car.pi || "-"}`],
  ];
  $("detailStats").innerHTML = stats.map(([l, v]) =>
    `<div class="stat"><span>${l}</span><strong>${v}</strong></div>`
  ).join("");

  const shifts = Object.entries(car.shiftTargets || {}).sort(([a], [b]) => Number(a) - Number(b));
  const standards = car.standardShiftTargets || {};
  const drops  = Object.entries(car.dropRatios   || {}).sort(([a], [b]) => Number(a) - Number(b));
  const comparisons = shifts.length
    ? shifts.map(([g, rpm]) => {
        const learned = Number(rpm) || 0;
        const standard = Number(standards[g]) || 0;
        const delta = learned && standard ? learned - standard : 0;
        const sign = delta > 0 ? "+" : "";
        return `<span>G${g}: learned ${Math.round(learned)} / std ${Math.round(standard)} rpm (${sign}${Math.round(delta)})</span>`;
      })
    : Object.entries(standards)
        .sort(([a], [b]) => Number(a) - Number(b))
        .map(([g, rpm]) => `<span>G${g}: std ${Math.round(rpm)} rpm</span>`);
  $("lapInfo").innerHTML = [
    `<span>Class ${car.class || "-"}</span>`,
    `<span>${car.drivetrain || "-"}</span>`,
    ...comparisons,
    ...drops.map(([g, r]) => {
      // Robustly extract a numeric ratio from different possible shapes:
      // - number (e.g. 0.895)
      // - string ("0.895")
      // - object { ratio: number, samples: N }
      // - object with a single numeric value
      let ratio = 0;
      if (r == null) {
        ratio = 0;
      } else if (typeof r === 'number') {
        ratio = r;
      } else if (typeof r === 'string') {
        ratio = Number(r) || 0;
      } else if (typeof r === 'object') {
        if (Number.isFinite(Number(r.ratio))) {
          ratio = Number(r.ratio);
        } else {
          // find first finite numeric property value
          for (const k of Object.keys(r)) {
            const v = Number(r[k]);
            if (Number.isFinite(v)) { ratio = v; break; }
          }
        }
      }
      return `<span>Drop ${g}: ${num(ratio, 3)}</span>`;
    }),
  ].join("");

  drawCarChart(car);
}

// ── Legend helper ────────────────────────────────────────────────────────────

function legendItem(ctx, x, y, color, label) {
  ctx.fillStyle = color;
  ctx.fillRect(x, y - 9, 11, 11);
  ctx.fillStyle = "#c0d4dc";
  ctx.font      = "11px sans-serif";
  ctx.textAlign = "left";
  ctx.fillText(label, x + 15, y);
}

function prepareSessionChartCanvas() {
  const canvas = $("sessionChart");
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width * dpr));
  const height = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h };
}

// ── Rounded-rect path helper (Canvas roundRect fallback) ────────────────────

function roundRect(ctx, x, y, w, h, r) {
  if (typeof ctx.roundRect === "function") {
    ctx.beginPath();
    ctx.roundRect(x, y, w, h, r);
  } else {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + w - r, y);
    ctx.quadraticCurveTo(x + w, y, x + w, y + r);
    ctx.lineTo(x + w, y + h - r);
    ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
    ctx.lineTo(x + r, y + h);
    ctx.quadraticCurveTo(x, y + h, x, y + h - r);
    ctx.lineTo(x, y + r);
    ctx.quadraticCurveTo(x, y, x + r, y);
    ctx.closePath();
  }
}

// ── Chart zoom/pan helpers ───────────────────────────────────────────────────

function resetChartView() {
  chartView = { startFrac: 0, endFrac: 1 };
}

// Returns { start, end } index range of the currently visible window.
function getChartBounds(samples) {
  if (samples.length < 2) return { start: 0, end: Math.max(0, samples.length - 1) };
  const n = samples.length;
  const start = clamp(Math.floor(chartView.startFrac * (n - 1)), 0, n - 2);
  const end   = clamp(Math.ceil(chartView.endFrac   * (n - 1)), start + 1, n - 1);
  return { start, end };
}

// Auto-scrolls the chart window to keep replayIndex visible (called during playback).
function ensureReplayVisible() {
  if (!detailSamples.length) return;
  const n = detailSamples.length;
  if (n < 2) return;
  const span = chartView.endFrac - chartView.startFrac;
  if (span >= 0.999) return; // fully zoomed out — nothing to scroll
  const idxFrac = replayIndex / (n - 1);
  const margin  = span * 0.06;
  // Only scroll when the cursor approaches or passes the edges
  if (idxFrac >= chartView.startFrac + margin && idxFrac <= chartView.endFrac - margin) return;
  const half = span * 0.5;
  chartView.startFrac = clamp(idxFrac - half, 0, 1 - span);
  chartView.endFrac   = chartView.startFrac + span;
}

// ── Session Progress Chart ───────────────────────────────────────────────────

// fn:       transform raw sample value before plotting/display
// fixedMax: use this as the Y-axis ceiling instead of computing from data
const CHART_SERIES = [
  { key: "speed", color: "#55e8ff", label: "Speed",    fn: (v) => v },
  { key: "rpm",   color: "#f3df4e", label: "RPM",      fn: (v) => v },
  { key: "accel", color: "#26f06e", label: "Throttle", fn: (v) => v,              fixedMax: 1  },
  { key: "brake", color: "#ff3658", label: "Brake",    fn: (v) => v,              fixedMax: 1  },
  { key: "gLat",  color: "#ff9a34", label: "|G-Lat|",  fn: (v) => Math.abs(v),   fixedMax: 2  },
  { key: "gLong", color: "#d84dff", label: "|G-Long|", fn: (v) => Math.abs(v),   fixedMax: 2  },
  { key: "drift", color: "#4ff0d8", label: "Drift°",   fn: (v) => Math.abs(v),   fixedMax: 45 },
];

function seriesMax(samples, series) {
  if (series.fixedMax !== undefined) return series.fixedMax;
  if (!samples.length) return 1;
  const fn = series.fn || ((v) => v);
  return Math.max(1, ...samples.map((s) => fn(s[series.key] ?? 0)));
}

function seriesValueText(series, sample) {
  const fn  = series.fn || ((v) => v);
  const raw = fn(sample[series.key] ?? 0);
  switch (series.key) {
    // raw value here is already in km/h (fn = identity for speed); apply conv for display
    case "speed": return `${num(conv.speed(raw))} ${conv.speedLabel()}`;
    case "rpm":   return `${num(raw)} rpm`;
    case "gLat":
    case "gLong": return `${num(raw, 2)}g`;
    case "drift": return `${num(raw, 1)}°`;
    default:      return `${num(raw * 100)}%`;
  }
}

// Returns a CHART_SERIES entry with unit conversion baked into fn so the Y-axis
// scales in the display unit (e.g. mph instead of km/h when imperial).
function resolveSeriesForDisplay(s) {
  if (s.key === "speed") {
    return {
      ...s,
      fn:       (v) => conv.speed(s.fn ? s.fn(v) : v),
      fixedMax: undefined,  // recompute from data in display units
    };
  }
  return s;
}

function sampleY(val, max, h) {
  return h - Math.max(0, Math.min(1, val / max)) * (h - 28) - 8;
}

function drawSessionChart(samples) {
  const { ctx, w, h } = prepareSessionChartCanvas();
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);
  if (!samples.length) return;

  const { start, end } = getChartBounds(samples);
  const slice = samples.slice(start, end + 1);
  if (slice.length < 2) return;

  const activeSeries = CHART_SERIES.filter((s) => chartFilters[s.key]).map(resolveSeriesForDisplay);

  // Draw series lines on the visible slice
  let legendX = 12;
  for (const s of activeSeries) {
    drawSeries(ctx, slice, s, w, h, seriesMax(slice, s));
    legendItem(ctx, legendX, 18, s.color, s.label);
    legendX += 78;
  }

  // Replay cursor — only when inside the visible window
  if (slice.length > 1 && replayIndex >= start && replayIndex <= end) {
    const localIdx = replayIndex - start;
    const x = (localIdx / (slice.length - 1)) * w;
    ctx.strokeStyle = "#ff3ea5";
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, h);
    ctx.stroke();
    ctx.fillStyle = "#ff3ea5";
    ctx.beginPath();
    ctx.arc(x, 14, 4, 0, Math.PI * 2);
    ctx.fill();
  }

  // Hover tooltip + crosshair
  if (chartHoverIndex >= start && chartHoverIndex <= end && slice.length > 1) {
    const sample   = samples[chartHoverIndex];
    const localIdx = chartHoverIndex - start;
    const hx       = (localIdx / (slice.length - 1)) * w;

    // Crosshair line
    ctx.strokeStyle = "rgba(255,255,255,0.14)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(hx, 22);
    ctx.lineTo(hx, h);
    ctx.stroke();

    // Dots on each visible series at the hover position
    for (const sr of activeSeries) {
      const fn  = sr.fn || ((v) => v);
      const max = seriesMax(slice, sr);
      const yd  = sampleY(fn(sample[sr.key] ?? 0), max, h);
      ctx.fillStyle = sr.color;
      ctx.beginPath();
      ctx.arc(hx, yd, 3.5, 0, Math.PI * 2);
      ctx.fill();
    }

    // Tooltip box
    const lines = [{ label: "t", value: timeText(sample.t || 0), color: "#c0d4dc" }];
    for (const sr of activeSeries) {
      lines.push({ label: sr.label, value: seriesValueText(sr, sample), color: sr.color });
    }
    if (sample.gear !== undefined) lines.push({ label: "Gear", value: String(sample.gear || 0), color: "#9aaeb8" });

    const lh  = 16;
    const pad = 8;
    const bw  = 150;
    const bh  = lines.length * lh + pad * 2;
    let bx = hx + 12;
    if (bx + bw > w - 4) bx = hx - bw - 12;
    const by = Math.max(22, Math.min(h - bh - 4, 36));

    ctx.fillStyle = "rgba(4, 7, 11, 0.92)";
    ctx.strokeStyle = "rgba(192, 212, 220, 0.16)";
    ctx.lineWidth = 1;
    roundRect(ctx, bx, by, bw, bh, 4);
    ctx.fill();
    ctx.stroke();

    ctx.font = "11px sans-serif";
    lines.forEach(({ label, value, color }, i) => {
      const ty = by + pad + (i + 1) * lh - 1;
      ctx.fillStyle = "rgba(150, 178, 188, 0.6)";
      ctx.textAlign = "left";
      ctx.fillText(label, bx + pad, ty);
      ctx.fillStyle = color;
      ctx.textAlign = "right";
      ctx.fillText(value, bx + bw - pad, ty);
    });
    ctx.textAlign = "left";
  }

  // Zoom scroll indicator bar at the bottom (only visible when zoomed in)
  const span = chartView.endFrac - chartView.startFrac;
  if (span < 0.999) {
    const barH = 3;
    const barY = h - barH;
    ctx.fillStyle = "rgba(255,255,255,0.07)";
    ctx.fillRect(0, barY, w, barH);
    ctx.fillStyle = "rgba(85, 232, 255, 0.45)";
    ctx.fillRect(chartView.startFrac * w, barY, span * w, barH);
  }
}

// Smooth line through the visible slice using midpoint quadratic Bézier curves.
// series: full CHART_SERIES entry (has .key, .color, .fn)
function drawSeries(ctx, samples, series, w, h, max) {
  if (samples.length < 2) return;
  const fn  = series.fn || ((v) => v);
  const pts = samples.map((s, i) => ({
    x: (i / (samples.length - 1)) * w,
    y: sampleY(fn(s[series.key] ?? 0), max, h),
  }));
  ctx.beginPath();
  ctx.strokeStyle = series.color;
  ctx.lineWidth = series.key === "rpm" ? 1.3 : 1.8;
  ctx.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length - 1; i++) {
    const mx = (pts[i].x + pts[i + 1].x) * 0.5;
    const my = (pts[i].y + pts[i + 1].y) * 0.5;
    ctx.quadraticCurveTo(pts[i].x, pts[i].y, mx, my);
  }
  ctx.lineTo(pts[pts.length - 1].x, pts[pts.length - 1].y);
  ctx.stroke();
}

// ── Car: Shift Points Bar Chart ──────────────────────────────────────────────

function drawCarChart(car) {
  const { ctx, w, h } = prepareSessionChartCanvas();
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);

  const shifts = Object.entries(car.shiftTargets || {})
    .map(([g, rpm]) => ({ gear: Number(g), rpm }))
    .filter((s) => s.rpm > 0)
    .sort((a, b) => a.gear - b.gear);
  const standardMap = car.standardShiftTargets || {};
  const standardShifts = Object.entries(standardMap)
    .map(([g, rpm]) => ({ gear: Number(g), rpm: Number(rpm) || 0 }))
    .filter((s) => s.rpm > 0)
    .sort((a, b) => a.gear - b.gear);

  if (!shifts.length && !standardShifts.length) {
    ctx.fillStyle = "#6b8a99"; ctx.font = "13px sans-serif"; ctx.textAlign = "center";
    ctx.fillText("No shift points recorded yet", w / 2, h / 2);
    ctx.textAlign = "left";
    return;
  }

  const chartShifts = shifts.length ? shifts : standardShifts;
  const maxRpm = Math.max(...chartShifts.map((s) => s.rpm), ...standardShifts.map((s) => s.rpm)) * 1.18;
  const pad    = { left: 56, right: 16, top: 36, bottom: 36 };
  const cW = w - pad.left - pad.right;
  const cH = h - pad.top  - pad.bottom;
  const slotW = cW / chartShifts.length;
  const barW  = Math.min(72, slotW * 0.55);

  for (let i = 0; i <= 5; i++) {
    const y = pad.top + cH - (i / 5) * cH;
    ctx.strokeStyle = "#1a2529"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(pad.left + cW, y); ctx.stroke();
    const v = Math.round(maxRpm * i / 5);
    ctx.fillStyle = "#6b8a99"; ctx.font = "10px sans-serif"; ctx.textAlign = "right";
    ctx.fillText(v >= 1000 ? `${(v / 1000).toFixed(1)}k` : v, pad.left - 6, y + 4);
  }

  chartShifts.forEach((s, i) => {
    const cx   = pad.left + slotW * i + slotW / 2;
    const standard = Number(standardMap[String(s.gear)]) || 0;
    if (standard > 0) {
      const refY = pad.top + cH - (standard / maxRpm) * cH;
      ctx.strokeStyle = "#c0d4dc99";
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(cx - barW * 0.65, refY);
      ctx.lineTo(cx + barW * 0.65, refY);
      ctx.stroke();
    }
    const barH = (s.rpm / maxRpm) * cH;
    const x    = cx - barW / 2;
    const y    = pad.top + cH - barH;
    const grad = ctx.createLinearGradient(0, y, 0, y + barH);
    grad.addColorStop(0, shifts.length ? "#55e8ff" : "#7f8b93");
    grad.addColorStop(1, shifts.length ? "#0d4a58" : "#303a40");
    ctx.fillStyle = grad;
    ctx.fillRect(x, y, barW, barH);
    ctx.fillStyle = "#e0f8ff"; ctx.font = "bold 11px sans-serif"; ctx.textAlign = "center";
    ctx.fillText(Math.round(s.rpm), cx, y - 7);
    ctx.fillStyle = "#9aaeb8"; ctx.font = "12px sans-serif";
    ctx.fillText(`G${s.gear}`, cx, pad.top + cH + 22);
  });

  ctx.fillStyle = "#9aaeb8"; ctx.font = "11px sans-serif"; ctx.textAlign = "left";
  ctx.fillText("Shift Points — learned bars, standard reference ticks", pad.left, 20);

  const obsRpm = Number(car.observedRpm);
  if (obsRpm > 0 && obsRpm <= maxRpm) {
    const refY = pad.top + cH - (obsRpm / maxRpm) * cH;
    ctx.strokeStyle = "#f3df4e55"; ctx.lineWidth = 1; ctx.setLineDash([4, 4]);
    ctx.beginPath(); ctx.moveTo(pad.left, refY); ctx.lineTo(pad.left + cW, refY); ctx.stroke();
    ctx.setLineDash([]);
    ctx.fillStyle = "#f3df4e99"; ctx.font = "10px sans-serif"; ctx.textAlign = "right";
    ctx.fillText(`max ${Math.round(obsRpm)} rpm`, pad.left + cW, refY - 4);
  }
  ctx.textAlign = "left";
}

// ── Car: Power / Torque Curve ────────────────────────────────────────────────

// Smooths a data series with a moving average (window ±radius)
function smooth(pts, key, radius = 2) {
  return pts.map((p, i) => {
    const lo  = Math.max(0, i - radius);
    const hi  = Math.min(pts.length - 1, i + radius);
    const avg = pts.slice(lo, hi + 1).reduce((s, q) => s + (q[key] || 0), 0) / (hi - lo + 1);
    return { ...p, [key]: avg };
  });
}

function drawCarPowerCurve(pts) {
  const { ctx, w, h } = prepareSessionChartCanvas();
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);

  let valid = pts.filter((p) => p.power > 0 || p.torque > 0);
  if (valid.length < 3) {
    ctx.fillStyle = "#6b8a99"; ctx.font = "13px sans-serif"; ctx.textAlign = "center";
    ctx.fillText("No data yet — drive the car to record the power curve", w / 2, h / 2);
    ctx.textAlign = "left";
    return;
  }

  // Smooth the curve (only when enough points available)
  if (valid.length >= 5) {
    valid = smooth(smooth(valid, "power",  2), "torque", 2);
  }

  const minRpm    = valid[0].rpm;
  const maxRpm    = valid[valid.length - 1].rpm;
  const rpmRange  = Math.max(1, maxRpm - minRpm);
  const maxPower  = Math.max(1, ...valid.map((p) => p.power))  * 1.12;
  const maxTorque = Math.max(1, ...valid.map((p) => p.torque)) * 1.12;

  const pad = { left: 62, right: 68, top: 36, bottom: 40 };
  const cW  = w - pad.left - pad.right;
  const cH  = h - pad.top  - pad.bottom;

  // Grid + Y axes
  for (let i = 0; i <= 4; i++) {
    const y = pad.top + cH - (i / 4) * cH;
    ctx.strokeStyle = "#1a2529"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(pad.left + cW, y); ctx.stroke();
    ctx.font = "10px sans-serif";
    ctx.textAlign = "right"; ctx.fillStyle = "#55e8ffaa";
    ctx.fillText(`${Math.round(maxPower  * i / 4)} hp`, pad.left - 5, y + 4);
    ctx.textAlign = "left";  ctx.fillStyle = "#ff9a34aa";
    ctx.fillText(`${Math.round(maxTorque * i / 4)} Nm`, pad.left + cW + 5, y + 4);
  }

  // X axis RPM values
  const rpmSteps = Math.min(8, valid.length);
  for (let i = 0; i <= rpmSteps; i++) {
    const x = pad.left + (i / rpmSteps) * cW;
    const r = Math.round((minRpm + rpmRange * i / rpmSteps) / 100) * 100;
    ctx.fillStyle = "#6b8a99"; ctx.font = "10px sans-serif"; ctx.textAlign = "center";
    ctx.fillText(r, x, pad.top + cH + 18);
    // Small tick mark
    ctx.strokeStyle = "#1a2529"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(x, pad.top + cH); ctx.lineTo(x, pad.top + cH + 5); ctx.stroke();
  }

  // Fill areas (semi-transparent) beneath the curves
  fillArea(ctx, valid, pad, cW, cH, minRpm, rpmRange, maxPower,  "power",  "#55e8ff18");
  fillArea(ctx, valid, pad, cW, cH, minRpm, rpmRange, maxTorque, "torque", "#ff9a3418");

  // Curve lines
  drawPowerLine(ctx, valid, pad, cW, cH, minRpm, rpmRange, maxPower,  "power",  "#55e8ff", 2.2);
  drawPowerLine(ctx, valid, pad, cW, cH, minRpm, rpmRange, maxTorque, "torque", "#ff9a34", 2.2);

  // Peak markers
  const peakP = valid.reduce((a, b) => b.power  > a.power  ? b : a);
  const peakT = valid.reduce((a, b) => b.torque > a.torque ? b : a);
  markPeak(ctx, peakP, "power",  maxPower,  "#55e8ff", pad, cW, cH, minRpm, rpmRange, "hp");
  markPeak(ctx, peakT, "torque", maxTorque, "#ff9a34", pad, cW, cH, minRpm, rpmRange, "Nm");

  // Legend + axis labels
  ctx.textAlign = "left";
  legendItem(ctx, pad.left,       20, "#55e8ff", "Power (hp)");
  legendItem(ctx, pad.left + 125, 20, "#ff9a34", "Torque (Nm)");
  ctx.fillStyle = "#6b8a99"; ctx.font = "10px sans-serif"; ctx.textAlign = "center";
  ctx.fillText("RPM", pad.left + cW / 2, pad.top + cH + 32);
  ctx.textAlign = "left";
}

function fillArea(ctx, pts, pad, cW, cH, minRpm, rpmRange, maxVal, key, color) {
  ctx.beginPath();
  ctx.fillStyle = color;
  pts.forEach((p, i) => {
    const x = pad.left + ((p.rpm - minRpm) / rpmRange) * cW;
    const y = pad.top  + cH - (p[key] / maxVal) * cH;
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  const lastX = pad.left + ((pts[pts.length - 1].rpm - minRpm) / rpmRange) * cW;
  ctx.lineTo(lastX, pad.top + cH);
  ctx.lineTo(pad.left, pad.top + cH);
  ctx.closePath();
  ctx.fill();
}

function drawPowerLine(ctx, pts, pad, cW, cH, minRpm, rpmRange, maxVal, key, color, lw) {
  ctx.beginPath();
  ctx.strokeStyle = color;
  ctx.lineWidth   = lw;
  pts.forEach((p, i) => {
    const x = pad.left + ((p.rpm - minRpm) / rpmRange) * cW;
    const y = pad.top  + cH - (p[key] / maxVal) * cH;
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();
}

function markPeak(ctx, pt, key, maxVal, color, pad, cW, cH, minRpm, rpmRange, unit) {
  const x = pad.left + ((pt.rpm - minRpm) / rpmRange) * cW;
  const y = pad.top  + cH - (pt[key] / maxVal) * cH;
  // Dot
  ctx.beginPath();
  ctx.arc(x, y, 4, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  // Label text
  const label = `${Math.round(pt[key])} ${unit} @ ${Math.round(pt.rpm)} rpm`;
  ctx.font      = "bold 10px sans-serif";
  ctx.fillStyle = color;
  const lx = Math.min(x + 6, pad.left + cW - ctx.measureText(label).width - 2);
  ctx.fillText(label, lx, y - 7);
}

// ── Session Map (Analytics Detail) ───────────────────────────────────────────

function toggleCalibrationPane() {
  $("detailCalibrationPane").hidden = !mapDebugMode;
}

function refreshAxisButtons() {
  $("detailFlipXBtn").classList.toggle("active", !!mapCalibration.worldFlipX);
  $("detailFlipZBtn").classList.toggle("active", !!mapCalibration.worldFlipZ);
}

function renderPendingPoint() {
  if (!mapPendingWorld) {
    $("detailPendingPoint").textContent = "none";
    return;
  }
  $("detailPendingPoint").textContent = `W(${num(mapPendingWorld.x, 2)}, ${num(mapPendingWorld.z, 2)})`;
}

function renderCalibrationInfo(transform = null) {
  if (transform === null) {
    ensureDetailProjection();
    transform = detailMapTransformCache;
  }
  const info = $("detailCalibrationInfo");
  if (!mapDebugMode) {
    info.innerHTML = "<strong>Debug mode off</strong><span>Start app with --debug to calibrate.</span>";
    return;
  }
  if (!transform) {
    info.innerHTML = "<strong>Calibration</strong><span>Add at least 3 points to compute transform.</span>";
    return;
  }
  info.innerHTML = `
    <strong>Calibration</strong>
    <span>Points: ${transform.points}</span>
    <span>Model: ${transform.kind || "unknown"}</span>
    <span>RMS error: ${num(transform.error, 2)} px</span>
  `;
}

function nearestTrackDistanceWorld(wx, wz) {
  if (!Array.isArray(detailTrack) || !detailTrack.length) return Infinity;
  let best = Infinity;
  for (const p of detailTrack) {
    const dx = Number(p.x) - wx;
    const dz = Number(p.z) - wz;
    if (!Number.isFinite(dx) || !Number.isFinite(dz)) continue;
    const d = Math.hypot(dx, dz);
    if (d < best) best = d;
  }
  return best;
}

function nearestTrackDistancePixel(px, py, projectedTrack) {
  if (!Array.isArray(projectedTrack) || !projectedTrack.length) return Infinity;
  let best = Infinity;
  for (const p of projectedTrack) {
    const d = Math.hypot(p.x - px, p.y - py);
    if (d < best) best = d;
  }
  return best;
}

function renderPointList() {
  const list = $("detailPointList");
  if (!mapDebugMode) {
    list.innerHTML = "";
    return;
  }
  ensureDetailProjection();
  const transform = detailMapTransformCache;
  const projectedTrack = detailProjectedTrack;
  list.innerHTML = (mapCalibration.points || []).map((p, i) => {
    const worldD = nearestTrackDistanceWorld(p.world.x, p.world.z);
    const proj = transform ? projectDetailPoint(p.world, transform) : null;
    const pixelD = proj ? nearestTrackDistancePixel(proj.x, proj.y, projectedTrack) : Infinity;
    const worldText = Number.isFinite(worldD) ? `world Δ ${num(worldD, 1)}m` : "world Δ --";
    const pixelText = Number.isFinite(pixelD) ? `px Δ ${num(pixelD, 1)}` : "px Δ --";
    return `
      <div class="pointRow">
        <span>#${i + 1} W(${num(p.world.x, 2)}, ${num(p.world.z, 2)}) -> P(${num(p.pixel.x, 1)}, ${num(p.pixel.y, 1)}) | ${worldText} | ${pixelText}</span>
        <button type="button" data-detail-del="${i}">delete</button>
      </div>
    `;
  }).join("");
  document.querySelectorAll("[data-detail-del]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const index = Number(btn.dataset.detailDel);
      if (!Number.isInteger(index) || index < 0) return;
      mapCalibration.points.splice(index, 1);
      updateCalibrationDerivedState();
      scheduleDetailMapDraw();
    });
  });
}

function updateCalibrationDerivedState() {
  invalidateDetailProjection();
  renderCalibrationInfo();
  renderPointList();
}

function setLiveMapPosition(payload) {
  const x = Number(payload?.position?.x);
  const z = Number(payload?.position?.z);
  const speed = Number(payload?.speed?.ms);
  const source = typeof payload?.position?.source === "string" ? payload.position.source : "raw";
  if (!Number.isFinite(x) || !Number.isFinite(z)) return;
  mapLivePosition = { x, z, speed: Number.isFinite(speed) ? speed : 0, source };
  $("detailLivePosition").textContent = `W(${num(x, 2)}, ${num(z, 2)}) · ${source}`;
}

function startMapLiveStream() {
  if (mapLiveSource) return;
  mapLiveSource = new EventSource("/events");
  mapLiveSource.addEventListener("telemetry", (event) => {
    try {
      const payload = JSON.parse(event.data);
      setLiveMapPosition(payload);
    } catch (_) {
      // ignore malformed frames
    }
  });
}

function stopMapLiveStream() {
  if (!mapLiveSource) return;
  mapLiveSource.close();
  mapLiveSource = null;
}

async function saveDetailCalibration() {
  if ((mapCalibration.points || []).length < 3) {
    setDetailMapHint("Calibration needs at least 3 points.");
    return;
  }
  const payload = {
    version: 1,
    mapImage: mapCalibration.mapImage || "map.jpg",
    worldFlipX: !!mapCalibration.worldFlipX,
    worldFlipZ: !!mapCalibration.worldFlipZ,
    points: mapCalibration.points || [],
  };
  const res = await fetch("/api/map/calibration", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await res.json();
  if (!data.ok) {
    setDetailMapHint(`Save failed: ${data.error || "unknown"}`);
    return;
  }
  setDetailMapHint(`Calibration saved (${data.points || 0} points)`);
}

function ensureDetailMapImage() {
  const src = `/${mapCalibration.mapImage || "map.jpg"}`;
  if (mapImage && mapImage.src.endsWith(src)) return;
  mapImageLoaded = false;
  mapImageProcessed = null;
  mapImage = new Image();
  mapImage.onload = () => {
    mapImageLoaded = true;
    detailMapView.initialized = false;
    // Pre-render reduced-saturation map once to keep pan/zoom responsive.
    const processed = document.createElement("canvas");
    processed.width = mapImage.width;
    processed.height = mapImage.height;
    const pctx = processed.getContext("2d");
    pctx.filter = "saturate(0.55) contrast(0.90)";
    pctx.drawImage(mapImage, 0, 0);
    pctx.filter = "none";
    mapImageProcessed = processed;
    invalidateDetailProjection();
    scheduleDetailMapDraw();
  };
  mapImage.src = src;
}

function detailMapCanvasPosition(event, canvas) {
  const rect = canvas.getBoundingClientRect();
  const sx = event.clientX - rect.left;
  const sy = event.clientY - rect.top;
  return { sx, sy };
}

function ensureDetailMapCanvasSize() {
  const canvas = $("detailMapCanvas");
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width * dpr));
  const height = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width === width && canvas.height === height) return;
  canvas.width = width;
  canvas.height = height;
}

function fitDetailMapView() {
  const canvas = $("detailMapCanvas");
  if (!mapImageLoaded || !mapImage || !canvas.clientWidth || !canvas.clientHeight) return;
  const scale = Math.min(canvas.clientWidth / mapImage.width, canvas.clientHeight / mapImage.height) * 0.96;
  detailMapView.scale = Math.max(0.05, scale);
  detailMapView.tx = (canvas.clientWidth - mapImage.width * detailMapView.scale) * 0.5;
  detailMapView.ty = (canvas.clientHeight - mapImage.height * detailMapView.scale) * 0.5;
  detailMapView.initialized = true;
}

function detailScreenToImage(x, y) {
  return {
    x: (x - detailMapView.tx) / detailMapView.scale,
    y: (y - detailMapView.ty) / detailMapView.scale,
  };
}

function normalizedWorldForDetail(point) {
  const fx = mapCalibration.worldFlipX ? -1 : 1;
  const fz = mapCalibration.worldFlipZ ? -1 : 1;
  return { x: Number(point.x) * fx, z: Number(point.z) * fz };
}

function solveLinearSystem(matrix, rhs) {
  const n = rhs.length;
  const a = matrix.map((row) => row.slice());
  const b = rhs.slice();
  for (let col = 0; col < n; col++) {
    let pivot = col;
    let best = Math.abs(a[col][col]);
    for (let row = col + 1; row < n; row++) {
      const score = Math.abs(a[row][col]);
      if (score > best) {
        best = score;
        pivot = row;
      }
    }
    if (best < 1e-10) return null;
    if (pivot !== col) {
      [a[col], a[pivot]] = [a[pivot], a[col]];
      [b[col], b[pivot]] = [b[pivot], b[col]];
    }
    const inv = 1 / a[col][col];
    for (let j = col; j < n; j++) a[col][j] *= inv;
    b[col] *= inv;
    for (let row = 0; row < n; row++) {
      if (row === col) continue;
      const factor = a[row][col];
      if (Math.abs(factor) < 1e-12) continue;
      for (let j = col; j < n; j++) a[row][j] -= factor * a[col][j];
      b[row] -= factor * b[col];
    }
  }
  return b;
}

function scoreDetailTransform(points, transform) {
  let sqErr = 0;
  for (const p of points) {
    const pr = projectDetailPoint(p.world, transform);
    const ex = p.pixel.x - pr.x;
    const ey = p.pixel.y - pr.y;
    sqErr += ex * ex + ey * ey;
  }
  return Math.sqrt(sqErr / Math.max(1, points.length));
}

function computeDetailMapTransform(points) {
  const valid = (points || []).filter((p) =>
    Number.isFinite(p?.world?.x) && Number.isFinite(p?.world?.z) &&
    Number.isFinite(p?.pixel?.x) && Number.isFinite(p?.pixel?.y)
  );
  if (valid.length < 3) return null;

  const ata = Array.from({ length: 6 }, () => Array(6).fill(0));
  const atb = Array(6).fill(0);
  for (const p of valid) {
    const world = normalizedWorldForDetail(p.world);
    const wx = world.x;
    const wz = world.z;
    const px = p.pixel.x;
    const py = p.pixel.y;
    const rowX = [wx, wz, 1, 0, 0, 0];
    const rowY = [0, 0, 0, wx, wz, 1];
    for (let i = 0; i < 6; i++) {
      for (let j = 0; j < 6; j++) {
        ata[i][j] += rowX[i] * rowX[j] + rowY[i] * rowY[j];
      }
      atb[i] += rowX[i] * px + rowY[i] * py;
    }
  }
  const sol = solveLinearSystem(ata, atb);
  if (!sol) return null;
  const transform = { m00: sol[0], m01: sol[1], tx: sol[2], m10: sol[3], m11: sol[4], ty: sol[5] };
  return {
    ...transform,
    error: scoreDetailTransform(valid, transform),
    points: valid.length,
    kind: "affine",
  };
}

function projectDetailPoint(point, transform) {
  const world = normalizedWorldForDetail(point);
  return {
    x: transform.m00 * world.x + transform.m01 * world.z + transform.tx,
    y: transform.m10 * world.x + transform.m11 * world.z + transform.ty,
    speed: Number(point.speed) || 0,
    drift: Number(point.drift) || 0,
    slip: Number(point.slip) || 0,
    gLat: Number(point.gLat) || 0,
    gLong: Number(point.gLong) || 0,
    gTotal: Number(point.gTotal) || 0,
  };
}

function projectReplaySample(sample, transform) {
  const position = sample?.position || {};
  const world = {
    x: Number(position.x),
    z: Number(position.z),
    speed: Number(sample?.speed) || 0,
    drift: Number(sample?.drift) || 0,
    slip: Number(sample?.slip) || 0,
    gLat: Number(sample?.gLat) || 0,
    gLong: Number(sample?.gLong) || 0,
    gTotal: Math.hypot(Number(sample?.gLat) || 0, Number(sample?.gLong) || 0),
  };
  if (!Number.isFinite(world.x) || !Number.isFinite(world.z)) return null;
  const point = projectDetailPoint(world, transform);
  return Number.isFinite(point.x) && Number.isFinite(point.y) ? point : null;
}

function ensureDetailProjection() {
  if (!detailProjectionDirty) return;
  const transform = computeDetailMapTransform(mapCalibration.points);
  detailMapTransformCache = transform;
  detailProjectedTrack = transform
    ? (detailTrack || [])
      .map((point) => projectDetailPoint(point, transform))
      .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y))
    : [];
  detailProjectionDirty = false;
}

function detailMapMetricValue(point) {
  if (mapColorMode === "speed") return Math.max(0, point.speed || 0);
  if (mapColorMode === "drift") return Math.abs(point.drift || 0);
  if (mapColorMode === "slip") return Math.max(0, point.slip || 0);
  if (mapColorMode === "gLat") return Math.abs(point.gLat || 0);
  if (mapColorMode === "gTotal") return Math.max(0, point.gTotal || 0);
  return 0;
}

function detailMapMetricRange(points) {
  const values = points.map((p) => detailMapMetricValue(p)).filter(Number.isFinite).sort((a, b) => a - b);
  if (!values.length) return { min: 0, max: 1 };
  if (mapColorMode === "speed") {
    const min = quantile(values, 0.03);
    const max = quantile(values, 0.97);
    return max - min > 1e-6 ? { min, max } : { min: 0, max: Math.max(1, max) };
  }
  const max = quantile(values, 0.97);
  return max > 1e-6 ? { min: 0, max } : { min: 0, max: 1 };
}

function detailMapColor(ratio) {
  const t = clamp(ratio, 0, 1);
  const eased = Math.pow(t, 0.85);
  const hue = 240 - eased * 240;
  return `hsl(${hue.toFixed(0)} 100% 55%)`;
}

function drawDetailMap(track) {
  const canvas = $("detailMapCanvas");
  const ctx = canvas.getContext("2d");
  ensureDetailMapCanvasSize();
  const dpr = window.devicePixelRatio || 1;
  if (!detailMapView.initialized && mapImageLoaded && mapImage) {
    fitDetailMapView();
  }
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);

  if (!mapImageLoaded || !mapImage) {
    ctx.fillStyle = "#6b8a99"; ctx.font = "12px sans-serif"; ctx.textAlign = "center";
    ctx.fillText("Loading map image...", w / 2, h / 2);
    ctx.textAlign = "left";
    setDetailMapHint("loading map image...");
    return;
  }

  ensureDetailProjection();
  const transform = detailMapTransformCache;

  const scale = detailMapView.scale;
  const ox = detailMapView.tx;
  const oy = detailMapView.ty;

  ctx.save();
  ctx.translate(ox, oy);
  ctx.scale(scale, scale);
  ctx.drawImage(mapImageProcessed || mapImage, 0, 0);

  const points = mapCalibration.points || [];
  if (mapDebugMode) {
    for (let i = 0; i < points.length; i++) {
      const p = points[i];
      const x = p.pixel?.x;
      const y = p.pixel?.y;
      if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
      const r = Math.max(4 / scale, 1.4);
      ctx.strokeStyle = "#f3df4e";
      ctx.lineWidth = Math.max(1.2 / scale, 0.6);
      ctx.beginPath();
      ctx.moveTo(x - r, y);
      ctx.lineTo(x + r, y);
      ctx.moveTo(x, y - r);
      ctx.lineTo(x, y + r);
      ctx.stroke();
      ctx.fillStyle = "#f3df4e";
      ctx.font = `${Math.max(10 / scale, 4)}px sans-serif`;
      ctx.fillText(`${i + 1}`, x + r + 1, y - r - 1);
    }
  }

  const projected = transform ? detailProjectedTrack : [];

  if (transform && projected.length >= 2) {
    const width = Math.max(3.6 / scale, 1.3);
    if (mapColorMode === "plain") {
      ctx.beginPath();
      ctx.moveTo(projected[0].x, projected[0].y);
      for (let i = 1; i < projected.length; i++) ctx.lineTo(projected[i].x, projected[i].y);
      ctx.lineWidth = width;
      ctx.strokeStyle = "#d84dff";
      ctx.shadowColor = "rgba(216, 77, 255, 0.55)";
      ctx.shadowBlur = 5 / scale;
      ctx.stroke();
      ctx.shadowBlur = 0;
    } else {
      ctx.beginPath();
      ctx.moveTo(projected[0].x, projected[0].y);
      for (let i = 1; i < projected.length; i++) ctx.lineTo(projected[i].x, projected[i].y);
      ctx.lineWidth = width + (1.8 / scale);
      ctx.strokeStyle = "rgba(0, 7, 10, 0.82)";
      ctx.stroke();

      const range = detailMapMetricRange(projected);
      const buckets = 56;
      const bucketOf = (point) => {
        const ratio = (detailMapMetricValue(point) - range.min) / (range.max - range.min);
        return clamp(Math.floor(clamp(ratio, 0, 1) * (buckets - 1)), 0, buckets - 1);
      };
      const strokeBucket = (bucket) => {
        const ratio = bucket / (buckets - 1);
        ctx.lineWidth = width;
        ctx.strokeStyle = detailMapColor(ratio);
        ctx.shadowColor = "rgba(255, 255, 255, 0.18)";
        ctx.shadowBlur = 3.2 / scale;
        ctx.stroke();
      };
      let current = bucketOf(projected[1]);
      ctx.beginPath();
      ctx.moveTo(projected[0].x, projected[0].y);
      for (let i = 1; i < projected.length; i++) {
        const prev = projected[i - 1];
        const p = projected[i];
        const bucket = bucketOf(p);
        if (bucket !== current) {
          strokeBucket(current);
          ctx.beginPath();
          ctx.moveTo(prev.x, prev.y);
          current = bucket;
        }
        ctx.lineTo(p.x, p.y);
      }
      strokeBucket(current);
      ctx.shadowBlur = 0;
    }

    ctx.fillStyle = "#26f06e";
    ctx.beginPath();
    ctx.arc(projected[0].x, projected[0].y, Math.max(3 / scale, 1.2), 0, Math.PI * 2);
    ctx.fill();

    const end = projected[projected.length - 1];
    ctx.fillStyle = "#ff3658";
    ctx.beginPath();
    ctx.arc(end.x, end.y, Math.max(3 / scale, 1.2), 0, Math.PI * 2);
    ctx.fill();

    const replayPoint = projectReplaySample(detailSamples[replayIndex], transform);
    if (replayPoint) {
      const r = Math.max(6 / scale, 2.2);
      ctx.fillStyle = "#ff3ea5";
      ctx.strokeStyle = "rgba(255,255,255,0.9)";
      ctx.lineWidth = Math.max(2 / scale, 0.8);
      ctx.beginPath();
      ctx.arc(replayPoint.x, replayPoint.y, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }
  } else if (transform) {
    ctx.fillStyle = "#6b8a99";
    ctx.font = `${Math.max(11 / scale, 5)}px sans-serif`;
    ctx.fillText("No track points", 8, 16);
  }

  ctx.restore();

  if (!transform) {
    setDetailMapHint("Need at least 3 calibration points for map alignment");
  } else if (!track.length) {
    setDetailMapHint("No track points in this session");
  } else {
    const src = detailTrackSrc || "unknown";
    setDetailMapHint(`Track points: ${track.length} | Source: ${src} | Color: ${mapColorMode} | Calibration error: ${num(transform.error, 2)} px`);
  }
}

function onDetailMapClick(event) {
  if (!mapDebugMode || !mapPendingWorld) return;
  const canvas = $("detailMapCanvas");
  const { sx, sy } = detailMapCanvasPosition(event, canvas);
  const imgPt = detailScreenToImage(sx, sy);
  if (!Number.isFinite(imgPt.x) || !Number.isFinite(imgPt.y)) return;
  mapCalibration.points.push({
    world: { x: mapPendingWorld.x, z: mapPendingWorld.z },
    pixel: { x: imgPt.x, y: imgPt.y },
    label: `P${mapCalibration.points.length + 1}`,
  });
  mapPendingWorld = null;
  renderPendingPoint();
  updateCalibrationDerivedState();
  scheduleDetailMapDraw();
}

function bindDetailMapInteraction() {
  const canvas = $("detailMapCanvas");
  const wrap = $("detailMapWrap");

  const onWheelZoom = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (!mapImageLoaded || !mapImage) return;
    if (!detailMapView.initialized) fitDetailMapView();
    const { sx, sy } = detailMapCanvasPosition(event, canvas);
    const before = detailScreenToImage(sx, sy);
    const delta = Math.max(-120, Math.min(120, event.deltaY));
    const zoom = Math.exp(-delta * 0.0015);
    detailMapView.scale = Math.max(0.05, Math.min(30, detailMapView.scale * zoom));
    detailMapView.tx = sx - before.x * detailMapView.scale;
    detailMapView.ty = sy - before.y * detailMapView.scale;
    scheduleDetailMapDraw();
  };

  wrap.addEventListener("wheel", onWheelZoom, { passive: false });

  canvas.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    detailMapView.pointer.down = true;
    detailMapView.pointer.moved = false;
    detailMapView.pointer.startX = event.clientX;
    detailMapView.pointer.startY = event.clientY;
    detailMapView.pointer.startTx = detailMapView.tx;
    detailMapView.pointer.startTy = detailMapView.ty;
    canvas.setPointerCapture(event.pointerId);
    canvas.classList.add("dragging");
  });

  canvas.addEventListener("pointermove", (event) => {
    if (!detailMapView.pointer.down) return;
    const dx = event.clientX - detailMapView.pointer.startX;
    const dy = event.clientY - detailMapView.pointer.startY;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) {
      detailMapView.pointer.moved = true;
    }
    detailMapView.tx = detailMapView.pointer.startTx + dx;
    detailMapView.ty = detailMapView.pointer.startTy + dy;
    scheduleDetailMapDraw();
  });

  canvas.addEventListener("pointerup", (event) => {
    const wasMoved = detailMapView.pointer.moved;
    detailMapView.pointer.down = false;
    canvas.classList.remove("dragging");
    if (!wasMoved) onDetailMapClick(event);
  });

  canvas.addEventListener("pointercancel", () => {
    detailMapView.pointer.down = false;
    canvas.classList.remove("dragging");
  });
}

// ── Init ─────────────────────────────────────────────────────────────────────


// ── Chart interaction (hover, drag-to-pan, wheel zoom) ───────────────────────

// Wheel zoom: scroll up/down to zoom in/out around the cursor position.
$("sessionChart").addEventListener("wheel", (event) => {
  if (!detailSamples.length || selectedCar) return;
  event.preventDefault();
  const canvas = $("sessionChart");
  const rect   = canvas.getBoundingClientRect();
  const xFrac  = clamp(event.clientX - rect.left, 0, rect.width) / rect.width;

  const span   = chartView.endFrac - chartView.startFrac;
  const center = chartView.startFrac + xFrac * span;
  const delta  = Math.max(-300, Math.min(300, event.deltaY));
  const factor = Math.exp(delta * 0.0012);
  const newSpan = clamp(span * factor, 0.02, 1);

  chartView.startFrac = clamp(center - xFrac * newSpan, 0, 1 - newSpan);
  chartView.endFrac   = chartView.startFrac + newSpan;
  drawSessionChart(detailSamples);
}, { passive: false });

// Drag-to-pan: hold + drag left/right when zoomed in.
$("sessionChart").addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || selectedCar) return;
  const span = chartView.endFrac - chartView.startFrac;
  chartDrag.active     = true;
  chartDrag.moved      = false;
  chartDrag.startX     = event.clientX;
  chartDrag.startStart = chartView.startFrac;
  chartDrag.spanFrac   = span;
  $("sessionChart").setPointerCapture(event.pointerId);
});

$("sessionChart").addEventListener("pointermove", (event) => {
  const canvas = $("sessionChart");
  const rect   = canvas.getBoundingClientRect();

  // Pan while dragging (only when actually zoomed in)
  if (chartDrag.active && chartDrag.spanFrac < 0.999) {
    const dx = event.clientX - chartDrag.startX;
    if (Math.abs(dx) > 3) chartDrag.moved = true;
    const dFrac = -(dx / rect.width) * chartDrag.spanFrac;
    chartView.startFrac = clamp(chartDrag.startStart + dFrac, 0, 1 - chartDrag.spanFrac);
    chartView.endFrac   = chartView.startFrac + chartDrag.spanFrac;
    drawSessionChart(detailSamples);
    return; // skip hover update during drag
  }

  // Hover crosshair
  if (!detailSamples.length || selectedCar) { chartHoverIndex = -1; return; }
  const x = clamp(event.clientX - rect.left, 0, rect.width);
  const { start, end } = getChartBounds(detailSamples);
  const visLen  = end - start + 1;
  const localIdx = clamp(Math.round((x / rect.width) * (visLen - 1)), 0, visLen - 1);
  const next = start + localIdx;
  if (next === chartHoverIndex) return;
  chartHoverIndex = next;
  drawSessionChart(detailSamples);
});

$("sessionChart").addEventListener("pointerup", () => {
  chartDrag.active = false;
});

$("sessionChart").addEventListener("mouseleave", () => {
  chartDrag.active = false;
  if (chartHoverIndex < 0) return;
  chartHoverIndex = -1;
  if (detailSamples.length) drawSessionChart(detailSamples);
});

// ── Chart series filters ─────────────────────────────────────────────────────

["speed", "rpm", "accel", "brake", "gLat", "gLong", "drift"].forEach((key) => {
  const cb = $(`filter_${key}`);
  if (!cb) return;
  cb.addEventListener("change", () => {
    chartFilters[key] = cb.checked;
    drawSessionChart(detailSamples);
  });
});

// ── Misc ──────────────────────────────────────────────────────────────────────

$("refreshBtn").addEventListener("click", refresh);
const savedMapColor = localStorage.getItem(DETAIL_MAP_COLOR_KEY);
if (DETAIL_MAP_COLOR_MODES.includes(savedMapColor)) {
  mapColorMode = savedMapColor;
}
$("detailMapColorMode").value = mapColorMode;
$("detailMapColorMode").addEventListener("change", (event) => {
  const value = event.target.value;
  if (!DETAIL_MAP_COLOR_MODES.includes(value)) return;
  mapColorMode = value;
  localStorage.setItem(DETAIL_MAP_COLOR_KEY, value);
  scheduleDetailMapDraw();
});
bindDetailMapInteraction();
$("detailMapZoomInBtn").addEventListener("click", () => {
  if (!detailMapView.initialized) fitDetailMapView();
  detailMapView.scale = Math.min(30, detailMapView.scale * 1.2);
  scheduleDetailMapDraw();
});
$("detailMapZoomOutBtn").addEventListener("click", () => {
  if (!detailMapView.initialized) fitDetailMapView();
  detailMapView.scale = Math.max(0.05, detailMapView.scale * 0.82);
  scheduleDetailMapDraw();
});
$("detailMapFitBtn").addEventListener("click", () => {
  fitDetailMapView();
  scheduleDetailMapDraw();
});
$("detailCaptureBtn").addEventListener("click", () => {
  if (!mapDebugMode) return;
  if (!mapLivePosition) {
    setDetailMapHint("No live telemetry yet");
    return;
  }
  if (mapLivePosition.source !== "raw") {
    setDetailMapHint("Calibration capture is blocked until RAW world coordinates are available.");
    return;
  }
  const isNearZero = Math.abs(mapLivePosition.x) + Math.abs(mapLivePosition.z) < 0.6;
  const isStandingStill = mapLivePosition.speed < 0.5;
  if (isNearZero && isStandingStill) {
    setDetailMapHint("Drive a few meters first so map position is initialized.");
    return;
  }
  mapPendingWorld = { x: mapLivePosition.x, z: mapLivePosition.z };
  renderPendingPoint();
  setDetailMapHint("Point captured. Click on the map image to place it.");
});
$("detailClearCalibrationBtn").addEventListener("click", () => {
  mapCalibration.points = [];
  mapPendingWorld = null;
  renderPendingPoint();
  updateCalibrationDerivedState();
  scheduleDetailMapDraw();
});
$("detailFlipXBtn").addEventListener("click", () => {
  mapCalibration.worldFlipX = !mapCalibration.worldFlipX;
  refreshAxisButtons();
  updateCalibrationDerivedState();
  scheduleDetailMapDraw();
});
$("detailFlipZBtn").addEventListener("click", () => {
  mapCalibration.worldFlipZ = !mapCalibration.worldFlipZ;
  refreshAxisButtons();
  updateCalibrationDerivedState();
  scheduleDetailMapDraw();
});
$("detailSaveCalibrationBtn").addEventListener("click", saveDetailCalibration);
$("replayPlayBtn").addEventListener("click", toggleReplay);
$("replaySlider").addEventListener("input", (event) => {
  stopReplay();
  setReplayIndex(Number(event.target.value) || 0);
});
$("replaySpeed").addEventListener("change", () => {
  replayAccumulator = 0;
});
window.addEventListener("resize", () => {
  detailMapView.initialized = false;
  if (selectedCar && currentCar) {
    if (activeCarChart === "power") drawCarPowerCurve(carPowerPoints);
    else drawCarChart(currentCar);
  } else if (selectedSession) {
    drawSessionChart(detailSamples);
  }
  scheduleDetailMapDraw();
});
window.addEventListener("beforeunload", () => {
  stopReplay();
  stopMapLiveStream();
});
toggleCalibrationPane();
renderPendingPoint();
updateCalibrationDerivedState();

$("unitToggle")?.addEventListener("click", async () => {
  await saveUnitSettings(unitSystem === "metric" ? "imperial" : "metric");
  syncUnitToggleBtns();
  renderOverview();
  renderSessions();
  renderCars();
  if (selectedSession && detailSummary) renderDetail(detailSummary, detailSamples);
  else if (selectedCar && currentCar) renderCarDetail(currentCar);
});

loadUnitSettings().then(() => { syncUnitToggleBtns(); refresh(); });
