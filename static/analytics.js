const $ = (id) => document.getElementById(id);

let sessions        = [];
let cars            = [];
let selectedSession = "";
let selectedCar     = "";
let currentCar      = null;   // car object for tab switching
let carPowerPoints  = [];     // preloaded curve data
let activeCarChart  = "shifts"; // "shifts" | "power"

function num(value, digits = 0) {
  const n = Number(value);
  return Number.isFinite(n) ? n.toFixed(digits) : (0).toFixed(digits);
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
  const [sessionData, carData] = await Promise.all([
    loadJson("/api/analytics/sessions"),
    loadJson("/api/analytics/cars"),
  ]);
  sessions = sessionData.sessions || [];
  cars     = carData.cars         || [];
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
  $("topSpeed").textContent     = `${num(Math.max(0, ...sessions.map((s) => s.maxSpeed || 0)))} km/h`;
  $("topG").textContent         = `${num(Math.max(0, ...sessions.map((s) => s.maxAbsG  || 0)), 2)}g`;
}

// ── Session List ────────────────────────────────────────────────────────────

function renderSessions() {
  $("sessionList").innerHTML = sessions.map((s) => `
    <button class="item ${s.id === selectedSession ? "active" : ""}" data-session="${s.id}">
      <strong>${s.carKey || "Unknown"} · ${timeText(s.duration)}</strong>
      <small>${new Date((s.startedAt || 0) * 1000).toLocaleString()}</small>
      <em>${num(s.maxSpeed)} km/h · ${num(s.maxRpm)} rpm · ${num(s.maxAbsG, 2)}g · ${s.shiftCount || 0} shifts</em>
    </button>
  `).join("");
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
        <em>${num(car.maxSpeed)} km/h · ${num(car.maxPower)} hp · ${num(car.observedRpm)} rpm</em>
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
  const detail = await loadJson(`/api/analytics/sessions/${encodeURIComponent(id)}`);
  renderDetail(detail.summary || {}, detail.samples || []);
}

async function selectCar(key) {
  selectedCar     = key;
  selectedSession = "";
  renderSessions();
  renderCars();

  currentCar     = cars.find((c) => c.key === key) || null;
  activeCarChart = "shifts";
  setActiveTab("shifts");
  $("chartTabs").style.display = "";

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
  $("detailTitle").textContent = `${summary.carKey || "Session"} · ${timeText(summary.duration)}`;
  const stats = [
    ["Top Speed", `${num(summary.maxSpeed)} km/h`],
    ["Max RPM",   `${num(summary.maxRpm)} rpm`],
    ["Power",     `${num(summary.maxPower)} hp`],
    ["Torque",    `${num(summary.maxTorque)} Nm`],
    ["Boost",     `${num(summary.maxBoost, 2)}`],
    ["Max G",     `${num(summary.maxAbsG, 2)}g`],
    ["Drift",     `${num(summary.maxDrift)} deg`],
    ["Shifts",    `${summary.shiftCount || 0}`],
  ];
  $("detailStats").innerHTML = stats.map(([l, v]) =>
    `<div class="stat"><span>${l}</span><strong>${v}</strong></div>`
  ).join("");
  $("lapInfo").innerHTML = `
    <span>Class ${summary.class || "-"}</span>
    <span>${summary.drivetrain || "-"}</span>
    <span>${summary.cylinders || 0} cylinders</span>
    <span>Ø Throttle ${num((summary.avgThrottle || 0) * 100)}%</span>
    <span>Ø Brake ${num((summary.avgBrake || 0) * 100)}%</span>
    <span>Best Lap ${summary.bestLap ? timeText(summary.bestLap) : "--"}</span>
  `;
  drawSessionChart(samples);
}

// ── Car Detail ───────────────────────────────────────────────────────────────

function renderCarDetail(car) {
  $("detailTitle").textContent = `${car.key} · ${car.class || "-"} ${car.pi ? "PI " + car.pi : ""}`;
  const stats = [
    ["Top Speed", `${num(car.maxSpeed)} km/h`],
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
  const drops  = Object.entries(car.dropRatios   || {}).sort(([a], [b]) => Number(a) - Number(b));
  $("lapInfo").innerHTML = [
    `<span>Class ${car.class || "-"}</span>`,
    `<span>${car.drivetrain || "-"}</span>`,
    ...shifts.map(([g, rpm]) => `<span>Gear ${g} → ${Math.round(rpm)} rpm</span>`),
    ...drops .map(([g, r])   => `<span>Drop G${g}: ${num(r, 3)}</span>`),
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

// ── Session Progress Chart ───────────────────────────────────────────────────

function drawSessionChart(samples) {
  const canvas = $("sessionChart");
  const ctx = canvas.getContext("2d");
  const w = canvas.width, h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);
  if (!samples.length) return;

  drawSeries(ctx, samples, "speed", "#55e8ff", w, h, Math.max(1, ...samples.map((s) => s.speed || 0)));
  drawSeries(ctx, samples, "rpm",   "#f3df4e", w, h, Math.max(1, ...samples.map((s) => s.rpm   || 0)));
  drawSeries(ctx, samples, "brake", "#ff3658", w, h, 1);
  drawSeries(ctx, samples, "accel", "#26f06e", w, h, 1);

  legendItem(ctx,  12, 18, "#55e8ff", "Speed");
  legendItem(ctx,  82, 18, "#f3df4e", "RPM");
  legendItem(ctx, 142, 18, "#26f06e", "Throttle");
  legendItem(ctx, 197, 18, "#ff3658", "Brake");
}

function drawSeries(ctx, samples, key, color, w, h, max) {
  ctx.beginPath();
  ctx.strokeStyle = color;
  ctx.lineWidth   = key === "rpm" ? 1.3 : 1.8;
  samples.forEach((s, i) => {
    const x = samples.length > 1 ? (i / (samples.length - 1)) * w : 0;
    const y = h - Math.max(0, Math.min(1, (s[key] || 0) / max)) * (h - 28) - 8;
    if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
  });
  ctx.stroke();
}

// ── Car: Shift Points Bar Chart ──────────────────────────────────────────────

function drawCarChart(car) {
  const canvas = $("sessionChart");
  const ctx = canvas.getContext("2d");
  const w = canvas.width, h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#070a0b";
  ctx.fillRect(0, 0, w, h);

  const shifts = Object.entries(car.shiftTargets || {})
    .map(([g, rpm]) => ({ gear: Number(g), rpm }))
    .filter((s) => s.rpm > 0)
    .sort((a, b) => a.gear - b.gear);

  if (!shifts.length) {
    ctx.fillStyle = "#6b8a99"; ctx.font = "13px sans-serif"; ctx.textAlign = "center";
    ctx.fillText("No shift points recorded yet", w / 2, h / 2);
    ctx.textAlign = "left";
    return;
  }

  const maxRpm = Math.max(...shifts.map((s) => s.rpm)) * 1.18;
  const pad    = { left: 56, right: 16, top: 36, bottom: 36 };
  const cW = w - pad.left - pad.right;
  const cH = h - pad.top  - pad.bottom;
  const slotW = cW / shifts.length;
  const barW  = Math.min(72, slotW * 0.55);

  for (let i = 0; i <= 5; i++) {
    const y = pad.top + cH - (i / 5) * cH;
    ctx.strokeStyle = "#1a2529"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(pad.left + cW, y); ctx.stroke();
    const v = Math.round(maxRpm * i / 5);
    ctx.fillStyle = "#6b8a99"; ctx.font = "10px sans-serif"; ctx.textAlign = "right";
    ctx.fillText(v >= 1000 ? `${(v / 1000).toFixed(1)}k` : v, pad.left - 6, y + 4);
  }

  shifts.forEach((s, i) => {
    const cx   = pad.left + slotW * i + slotW / 2;
    const barH = (s.rpm / maxRpm) * cH;
    const x    = cx - barW / 2;
    const y    = pad.top + cH - barH;
    const grad = ctx.createLinearGradient(0, y, 0, y + barH);
    grad.addColorStop(0, "#55e8ff"); grad.addColorStop(1, "#0d4a58");
    ctx.fillStyle = grad;
    ctx.fillRect(x, y, barW, barH);
    ctx.fillStyle = "#e0f8ff"; ctx.font = "bold 11px sans-serif"; ctx.textAlign = "center";
    ctx.fillText(Math.round(s.rpm), cx, y - 7);
    ctx.fillStyle = "#9aaeb8"; ctx.font = "12px sans-serif";
    ctx.fillText(`G${s.gear}`, cx, pad.top + cH + 22);
  });

  ctx.fillStyle = "#9aaeb8"; ctx.font = "11px sans-serif"; ctx.textAlign = "left";
  ctx.fillText("Optimal Shift Points — RPM per Gear", pad.left, 20);

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
  const canvas = $("sessionChart");
  const ctx = canvas.getContext("2d");
  const w = canvas.width, h = canvas.height;
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

// ── Init ─────────────────────────────────────────────────────────────────────


$("refreshBtn").addEventListener("click", refresh);
refresh();
