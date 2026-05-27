# Changelog

All notable changes are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## [0.4.7] — 2026-05-27

### Bug fixes

- **Single-lap race analytics** — races that consist of only one lap (Forza lap number never
  increments past 0) now correctly show `totalLaps = 1` and the lap time in the lap-times grid
  instead of appearing empty.
- **Power curve contamination at high RPM** — the learning algorithm no longer records samples
  while RPM is falling at high values (downshift over-rev, limiter bounce recovery).  A
  confirmed rising run of at least 5 % of `maxRpm` is required before high-RPM buckets are
  written.  Additionally, samples are skipped when the limiter-bounce counter is active
  (`bounceCount > 0`).
- **Weak car / uphill limit learning** — the `is_high_rpm_continuation` path in max-RPM
  detection now requires full throttle (option C), only fires in the lower half of observed
  gears (the driver passes through these first and hits the real limiter there), and is
  suppressed after RPM has been stable for 2 s at full throttle without bounce detection
  (plateau detector, option E).  Together these prevent a weak car plateauing on a hill from
  recording a falsely low `maxObservedRpm`.
- **Touch pinch-to-zoom on analytics chart** — two-finger pinch now zooms the session chart
  on mobile / tablet; single-finger drag-to-pan and mouse-wheel zoom remain unchanged.
  `touch-action: none` prevents the browser from intercepting touch events for page scroll.

---

## [0.4.0] — 2026-05-24

### Analytics — new features
- **Race session support** — sessions where `raceOn` was true for the majority of samples are
  automatically flagged as race sessions.  The summary now includes `isRace`, `finishPosition`,
  `totalLaps`, and a `lapTimes[]` array with the duration of each completed lap.
- **Analytics session list** — race sessions show a green **RACE** badge; finish position and
  lap count appear in the sub-line.
- **Race detail panel** — new lap-times grid below the track map showing every completed lap,
  its time, and its delta vs the session best (fastest lap highlighted green).
- **Chart zoom / pan** — mouse-wheel zooms in/out around the cursor; drag left/right to pan
  while zoomed in; a thin indicator bar shows the current viewport position.
- **G-force & drift series** — |G-Lat|, |G-Long|, and Drift° are now toggleable chart series
  (orange / purple / teal, off by default).
- **Real-time timestamp-based replay** — replay advances by `sample.t` delta × speed factor,
  so 1× speed is true real-time regardless of sample rate or monitor frame rate.
- **Replay auto-scroll** — when playing back a zoomed-in view, the chart window follows the
  replay cursor automatically.
- **CSV export** — new endpoint `GET /api/analytics/sessions/{id}/csv`; a ⬇ CSV button
  appears in the detail pane header for every session.  All 21 columns: time, speed, RPM,
  gear, inputs, G-forces, drift, slip, power, torque, boost, position, race state, lap data.
- **Higher chart resolution** — session detail now returns up to 3 600 samples (was 360),
  giving enough data to make zoom useful.
- **1-hour session cap** — sessions are hard-split after 60 min to prevent runaway files.

### Analytics — bug fixes
- Tyre temperatures were previously stored and displayed in °F; converted correctly to °C
  (`(raw − 32) × 5/9`).
- `tireSlipAngleDeg` fields were incorrectly run through `.to_degrees()` — these are
  normalised values (~±2 scale), not radians.  The conversion has been removed.
- TMP tyre-warning threshold corrected to **110 °C** (was 105; the old threshold was
  accidentally calibrated against the raw °F value).

### Shift sound
- **Audio shift cue** — synthesised shift beep plays at the shift-now point, on the backend
  device (via `rodio` / ALSA/PulseAudio), the browser (Web Audio API), or both simultaneously.
- **5 built-in sounds**: `blip` (falling sawtooth chirp), `click` (square burst), `beep`
  (sine), `chord` (major triad A4+C5+E5), `buzz` (low sawtooth).  All sounds are generated
  as PCM samples at runtime — no audio files bundled.
- **Native GUI selector** — "Shift sound" ComboBox under Settings controls the backend sound;
  selecting a new option plays a preview immediately.  ▶ Test button fires on demand.
- **Web dashboard selector** — ⚙ button in the status bar opens a panel with a "Shift sound"
  dropdown controlling the browser sound; selecting plays a preview.
- Settings persisted to `data/settings.json` (`shiftSoundWeb` / `shiftSoundBackend`).
- Backend sound fires autonomously from the GUI's shift-flash detector — no HTTP round-trip.
  When the GUI is not running (headless mode) only the web sound is available.

### Unit system
- **Metric / Imperial toggle** — speed (km/h ↔ mph), torque (Nm ↔ lb-ft), tyre temp (°C ↔ °F).
  Persisted in `data/settings.json` (`unitSystem`).  Toggle button in the status bar of both
  the dashboard and analytics page; both pages share the same stored setting.

### Dashboard / overlay
- Full English translation — all German labels and tooltips replaced with English equivalents.
- Comprehensive **help / legend** panel describing every abbreviation on the dashboard:
  speed, RPM, G-force meter, drift indicator, tyre corners (FL/FR/RL/RR), control-strip
  fields (GAS/BRK/STR/DLT/PROG), data-strip fields (LAT/LON/DRV/FUEL), warning LEDs
  (US/OS/B-T/TMP/ABS/LOCK/CL/HB), and shift LED colours.
- Shift LED glow rings no longer bleed into adjacent LEDs (ring radius capped to `r + gap×0.35`).
- Power / torque curve chart added to the car browser in the analytics UI.
- Map saturation reduced (`saturate(0.55) contrast(0.90)`) for a cleaner track overlay.

### Performance
- **Blocking I/O decoupled from the async hot path** — `TelemetryRecorder::record()` now
  merely clones the telemetry `Value` and sends it over a bounded `std::sync::mpsc` channel
  (128 slots).  All file I/O (open, write, flush, close) happens on a dedicated background
  thread.  The Tokio executor thread that handles UDP packets is never blocked by a syscall.
- `BufWriter<File>` wraps all session-file writes to batch `writeln!` syscalls.

### Internal
- Removed `Mutex` from `TelemetryRecorder` — state is now owned exclusively by the worker thread.
- `src/util.rs` — `lock_recover` helper added for Mutex poison recovery.
- Packet parser: `detect_dash_offset()` scoring algorithm handles both 232-byte and 244-byte
  (extended FH5/FH6) packet formats.

---

## [0.3.0] — prior release

- egui/eframe GUI with overlay shift LED viewport (always-on-top, borderless, draggable).
- Web dashboard (`/`) with live telemetry via Server-Sent Events.
- Analytics page (`/analytics`) with session list, car browser, shift-point bar chart,
  session track map with affine calibration, replay controls, hover tooltip.
- Learned shift-point engine: per-gear optimal RPM with gear-drop ratio tracking.
- `--demo` flag for offline testing without a console.
- `--inspect` / `--inspect-dir` for raw packet logging.

---

## [0.2.0] — initial public release

- UDP listener for Forza Horizon Data Out (standard 232-byte and extended 324-byte packets).
- Terminal-mode tachometer.
- Basic shift LED output.

---

## [0.1.0] — bootstrap

- Project skeleton.
