// ── App settings ──────────────────────────────────────────────────────────────
// Shared by index.html (app.js) and analytics.html (analytics.js).
// Load settings.js BEFORE the page-specific script.

/** Current unit system: "metric" (default) | "imperial" */
let unitSystem = "metric";

/** Sound played in the browser on shift. "none" = off. */
let shiftSoundWeb = "blip";

/** Sound played on the backend/server device on shift. "none" = off. */
let shiftSoundBackend = "none";

/** All valid sound names (must match backend SOUND_NAMES). */
const SOUND_NAMES = ["none", "blip", "click", "beep", "chord", "buzz"];

/** Unit-aware conversion and formatting helpers. */
const conv = {
  // Raw conversions
  speed:  (kmh) => unitSystem === "imperial" ? kmh  * 0.621371  : kmh,
  torque: (nm)  => unitSystem === "imperial" ? nm   * 0.737562  : nm,
  temp:   (c)   => unitSystem === "imperial" ? c    * 9 / 5 + 32 : c,

  // Unit label strings
  speedLabel:  () => unitSystem === "imperial" ? "mph"   : "km/h",
  torqueLabel: () => unitSystem === "imperial" ? "lb-ft" : "Nm",
  tempLabel:   () => unitSystem === "imperial" ? "°F"    : "°C",

  // One-shot formatted strings (number + unit)
  fmtSpeed:  (kmh, digits = 0) => `${conv.speed(kmh).toFixed(digits)} ${conv.speedLabel()}`,
  fmtTorque: (nm,  digits = 0) => `${conv.torque(nm).toFixed(digits)} ${conv.torqueLabel()}`,
  fmtTemp:   (c,   digits = 1) => `${conv.temp(c).toFixed(digits)} ${conv.tempLabel()}`,
};

/** Fetches all settings from the server and updates all globals. */
async function loadUnitSettings() {
  try {
    const res  = await fetch("/api/settings", { cache: "no-store" });
    const data = await res.json();
    if (data.unitSystem === "imperial" || data.unitSystem === "metric") {
      unitSystem = data.unitSystem;
    }
    if (SOUND_NAMES.includes(data.shiftSoundWeb))     shiftSoundWeb     = data.shiftSoundWeb;
    if (SOUND_NAMES.includes(data.shiftSoundBackend)) shiftSoundBackend = data.shiftSoundBackend;
  } catch (_) { /* keep defaults */ }
}

/** Persists a new unit system and updates `unitSystem`. */
async function saveUnitSettings(system) {
  unitSystem = system === "imperial" ? "imperial" : "metric";
  try {
    await fetch("/api/settings", {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify({ unitSystem }),
    });
  } catch (_) {}
}

/** Persists the two shift-sound selections and updates globals. */
async function saveShiftSoundSettings(web, backend) {
  if (SOUND_NAMES.includes(web))     shiftSoundWeb     = web;
  if (SOUND_NAMES.includes(backend)) shiftSoundBackend = backend;
  try {
    await fetch("/api/settings", {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify({ shiftSoundWeb, shiftSoundBackend }),
    });
  } catch (_) {}
}

/** Updates all elements with class `unitToggleBtn` to reflect the current unit system. */
function syncUnitToggleBtns() {
  const label = unitSystem === "imperial" ? "mph" : "km/h";
  document.querySelectorAll(".unitToggleBtn").forEach((btn) => {
    btn.textContent = label;
  });
}
