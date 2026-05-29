# forza-tacho

A real-time tachometer overlay for Forza Horizon 6 — with a shift light that actually learns your car.

![Dashboard](docs/dashboard.png)

![Analytics](docs/analysis.png)

## Contents

- [What is this?](#what-is-this)
- [Quick start](#quick-start--just-want-to-play)
- [Dashboard field reference](#dashboard-field-reference)
- [How the shift light works](#how-the-shift-light-works)
- [Shift sound](#shift-sound)
- [Unit system](#unit-system)
- [Analytics](#analytics)
- [Session Map + Calibration](#session-map--calibration)
- [Command-line options](#command-line-options)
- [Data storage](#data-storage)
- [Building from source](#building-from-source)
- [Technical deep-dive](#technical-deep-dive-the-shift-algorithm)
- [Releases](#releases)

---

## What is this?

forza-tacho runs in the background while you play Forza.  
It shows you a **shift indicator** that tells you exactly when to upshift for maximum power — not just "shift at 80% RPM", but the real optimal point for each car, learned from how you drive it.

You get:
- A **shift light overlay** you can drag anywhere on screen
- A **dashboard** in your browser with RPM, speed, gear, G-forces, lap times and more
- An **analytics page** with session history, race results, lap times, track maps, charts, and CSV export
- A **shift sound cue** that plays when the light fires — on the backend device, in the browser, or both
- The dashboard works on **any device on the same network** — open it on a tablet or phone, prop it up next to your monitor, or mount it behind your wheel. It installs as a **PWA** so it opens fullscreen with no browser chrome.

---

## Quick start — just want to play?

### 1. Download and open

Go to the **[Releases](../../releases)** page and download the right file for your system:

- **Windows:** `forza-tacho.exe` — double-click it. A small launcher window opens.
- **Linux:** `forza-tacho-linux-x86_64` — `chmod +x` and run it.  
  Works on Ubuntu, Fedora, Arch, CachyOS, Bazzite, Nobara, and others.
- **macOS:** `forza-tacho-macos` — one universal binary for Intel and Apple Silicon (M1/M2/M3).

---

### 2. Set up Forza

In Forza Horizon 6, go to:

**Settings → HUD and Gameplay → Data Out**

| Setting | Value |
|---|---|
| Data Out | **On** |
| Data Out IP Address | the IP shown in the forza-tacho window |
| Data Out IP Port | **5300** |

The IP address is shown right in the launcher window — there's a copy button next to it.

> **forza-tacho doesn't have to run on the same PC as Forza.**  
> Run it on any machine on the same network and point Forza's Data Out at that machine's IP.

> **Privacy mode:** Enable **Local-only mode** in the launcher settings to bind HTTP and UDP to `127.0.0.1` — the dashboard will only be reachable from the same PC.

---

### 3. Open the dashboard

Click "Open in browser" in the launcher, or type the URL into any browser on your network.

> **Second screen / tablet:**  
> Open the dashboard on a separate device. A tablet next to your monitor works great.  
> On mobile: *Share → Add to Home Screen* (iOS) or the install prompt in Chrome (Android) to get a fullscreen app with no browser UI.

---

### 4. Optional: shift LED overlay

In the launcher, toggle **Show overlay**.  
A row of coloured dots appears — drag it wherever you want, it stays on top of the game.  
Right-click the overlay to close it.

---

## Dashboard field reference

<details>
<summary>Open dashboard field legend</summary>

### Main displays

| Field | What it means |
|---|---|
| **RPM** | Engine revolutions per minute. |
| **Speed** | Vehicle speed in km/h or mph (see [Unit system](#unit-system)). |
| **Gear** | Current gear. 0 = neutral, negative = reverse. |
| **Power** | Estimated wheel power at the current RPM (hp). |
| **Torque** | Engine torque at the current RPM (Nm or lb-ft). |
| **Boost** | Turbo / supercharger boost pressure. |
| **Lap / Time / Best** | Lap number, elapsed lap time, and fastest completed lap this session. |
| **Pos** | Race position — only populated in structured events. |

### Control strip (right side)

| Label | Full name | What it shows |
|---|---|---|
| **GAS** | Throttle | Accelerator pedal, 0–100 %. |
| **BRK** | Brake | Brake pedal, 0–100 %. |
| **STR** | Steering | Wheel position, −1.0 (full left) to +1.0 (full right). |
| **DLT** | Lap delta | Time gap to your best lap. Green = faster, red = slower. |
| **PROG** | Lap progress | How far through the current lap, as a percentage. |

### Data strip

| Label | Full name | What it shows |
|---|---|---|
| **LAT** | Lateral G | Left/right cornering load in G. |
| **LON** | Longitudinal G | Acceleration / braking load in G. |
| **DRV** | Drivetrain | FWD, RWD, or AWD. |
| **FUEL** | Fuel level | Percentage (≤ 100 %) or litres when > 1.2. |

### G-force meter

The dot moves in two dimensions — left/right for lateral, up/down for longitudinal.  
The `p` suffix (e.g. `1.23p`) is the **peak** value since you started driving.

### Drift display

| Element | What it shows |
|---|---|
| Angle (deg) | Estimated side-slip angle in degrees. |
| Sliding bar | Live slip angle as a needle. |
| `Np` number | Peak drift angle this session. |

### Tyre corners (FL / FR / RL / RR)

**FL** = Front Left, **FR** = Front Right, **RL** = Rear Left, **RR** = Rear Right.

- **Temperature box** — tyre surface °C (or °F). Colour: blue (cold) → green (optimal) → red/white (overheating).
- **Slip LED strip** (5 dots per corner, at screen edges) — combined slip load. All lit = significant wheelspin or lockup.

### Warning indicators

| Label | Full name | When it lights |
|---|---|---|
| **US** | Understeer | Fronts losing grip more than rears while steering — car pushes wide. |
| **OS** | Oversteer | Rears losing grip more than fronts while steering — rear steps out. |
| **B/T** | Brake/Throttle overlap | Both pedals pressed > 8 % simultaneously. |
| **TMP** | Tyre temperature | Any tyre above 110 °C — grip noticeably degrades. |
| **ABS** | ABS active | Anti-lock system intervening under hard braking. |
| **LOCK** | Wheel lock | A front wheel locked up under braking. |
| **CL** | Clutch | Clutch pedal pressed > 8 %. |
| **HB** | Handbrake | Handbrake on > 8 %. |

### Shift LED colours

| Colour | Meaning |
|---|---|
| 🟢 Green (1–4) | RPM building — shift not yet near. |
| 🟡 Yellow (5–8) | Approaching the shift point. |
| 🔴 Red (9–11) | Getting close — prepare to shift. |
| 🟣 Purple (12–14) + flash | **Shift NOW.** Flashes until you upshift. |

</details>

---

## How the shift light works

<details>
<summary>Open shift-light overview</summary>

The indicator goes through several stages as you drive more.

**Stage 1 — First drive, no data yet**  
Conservative estimate: warns early rather than letting you hit the limiter.

**Stage 2 — Learning your rev limit**  
After a few laps the app locks in the car's actual RPM ceiling.

**Stage 3 — Rev limiter detection**  
Holding full throttle at the limiter produces a characteristic RPM oscillation — the app detects it and confirms the exact limit.

**Stages 4–7 — Power curve and optimal shift point**  
Full-throttle runs build a per-car power curve. The app finds the RPM where shifting gives *more* power than staying, and that's when the light fires.  
Lead time is compensated dynamically so you have time to react.

All data is saved per car. The second time you drive the same vehicle everything is already learned.

</details>

---

## Shift sound

A sound cue fires whenever the shift indicator activates.

**5 built-in sounds:** `blip` (falling chirp), `click` (square burst), `beep` (sine tone), `chord` (major triad), `buzz` (low sawtooth). All synthesised at runtime — no audio files.

**Two independent selectors:**

| Where | What it controls | How to access |
|---|---|---|
| Native app window | Sound played on **this machine's speaker** | Settings section → Shift sound dropdown |
| Browser dashboard | Sound played **in the browser** | ⚙ button in the status bar |

Selecting a sound previews it immediately. The native app also has a **▶ Test** button.  
Settings are saved to `data/settings.json` and survive restarts.

> **Linux note:** audio requires the ALSA bridge for PipeWire/PulseAudio.  
> Install `pipewire-alsa` (Fedora/Arch) or `pulseaudio-alsa` (Ubuntu/Debian) if no sound plays.  
> Error details are printed to the terminal if audio initialisation fails.

---

## Unit system

Toggle between **metric** (km/h · Nm · °C) and **imperial** (mph · lb-ft · °F) from:

- The **km/h / mph** button in the status bar of either the dashboard or the analytics page
- The setting is shared — changing it in one page applies everywhere

Stored in `data/settings.json` as `unitSystem`.

---

## Analytics

<details>
<summary>Open analytics feature list</summary>

Open the analytics page at `http://<host>:8765/analytics.html`.

### Sessions

- Every drive is automatically recorded as a session.
- **Race sessions** are detected automatically (majority of samples have `raceOn=true`). Race sessions show a **RACE** badge and include finish position, lap count, and a full lap-time grid with delta vs session best.
- Session stats: top speed, max G, pure lateral G, distance, duration.

### Charts

- **Toggleable series:** Speed, RPM, Throttle, Brake, |G-Lat|, |G-Long|, Drift°.
- **Zoom:** mouse-wheel zooms in/out around the cursor.
- **Pan:** click and drag left/right while zoomed in. A thin bar shows the current viewport position.
- **Replay:** timestamp-based playback at 0.5×/1×/2×/4× speed. 1× is true real-time. The chart window follows the replay cursor automatically.
- Hover over the chart for a per-sample tooltip.

### Track map

- Session path drawn from recorded world positions, coloured by: Speed, Drift, Slip, G Lat, G Total, or plain.
- Pan and zoom with mouse/wheel, or use the +/−/Fit buttons.
- Calibration tools available in `--debug` mode.

### CSV export

Every session has a **⬇ CSV** button in the detail pane. Downloads a full 21-column CSV:  
`time, speed, rpm, gear, throttle, brake, steer, g_lat, g_long, drift, slip, power, torque, boost, pos_x, pos_z, race_on, lap_num, lap_current_s, lap_best_s, lap_pos`

Also available via API: `GET /api/analytics/sessions/{id}/csv`

### Car browser

- Lists every car you have driven with top speed, best G, and recorded sessions.
- Per-car power curve chart and learned shift-point bar chart.

</details>

---

## Session Map + Calibration

<details>
<summary>Open map and calibration workflow</summary>

Map features are integrated into the analytics page (`/analytics.html`).

- Select a session → the track is drawn below the charts.
- Coloring modes: Plain, Speed, Drift, Slip, G Lat, G Total.
- Pan + zoom: mouse-wheel, drag, or +/−/Fit buttons.
- Start the app with `--debug` to enable calibration tools:
  - Capture your current world position while driving.
  - Click the matching point on the map image.
  - Repeat, then save to `data/map_calibration.json`.
  - `Flip X` / `Flip Z` toggles for mirrored-axis tracks.

</details>

---

## Command-line options

<details>
<summary>Open CLI reference</summary>

All options are optional — the defaults work out of the box.

```
forza-tacho [OPTIONS]

Options:
  --http-host <HOST>           Address for the web server       [default: 0.0.0.0]
  --http-port <PORT>           Web server port                  [default: 8765]
  --udp-host <HOST>            Address for Forza UDP packets    [default: 0.0.0.0]
  --udp-port <PORT>            UDP port                         [default: 5300]
  --demo                       Run a simulated car (no Forza needed)
  --debug                      Enable analytics map calibration tools
  --no-gui                     Terminal-only mode, no window
  --inspect                    Log raw UDP packets to disk
  --inspect-dir <DIR>          Directory for packet logs        [default: logs]
  --inspect-every <N>          Log every Nth packet             [default: 30]
  --shift-cache-log            Write shift decisions to JSONL
  --shift-cache-log-dir <DIR>  Directory for shift logs         [default: logs]
  --limiter-log                Print limiter-detection events to stdout
  --limiter-debug              Verbose per-packet limiter debug
  -h, --help                   Print help
```

</details>

---

## Data storage

<details>
<summary>Open data layout</summary>

Everything is written next to the executable:

```
forza-tacho(.exe)
data/
  power_curves.json       ← learned shift points, one entry per car
  drive_sessions/         ← one .jsonl file per session
  settings.json           ← unit system, shift sound preferences
  map_calibration.json    ← world→image calibration for the track map
  launcher_config.json    ← launcher prefs (local-only mode)
logs/                     ← only created with --inspect or --shift-cache-log
```

Deleting `data/` resets all learned shift data and settings.  
If `map_calibration.json` is missing, the embedded default calibration is used.

</details>

---

## Security / Network

forza-tacho is designed for **trusted home networks only**.

- The HTTP server and UDP listener bind to `0.0.0.0` by default, making the dashboard reachable by **any device on the same network**. Data exposed includes live position, track line, lap times, and cars driven.
- There is **no authentication** on the HTTP API, including the `/settings` and `/preview-sound` endpoints. The `map.calibration` POST is the only mutation and requires `--debug`.
- On untrusted networks (public Wi-Fi, LAN parties), enable **Local-only mode** in the launcher settings — or pass `--local-only` on the command line — to bind to `127.0.0.1` so the dashboard is only reachable from the same PC.

---

## Overlay on KDE Wayland

<details>
<summary>Open KDE Wayland setup</summary>

On KDE Wayland the compositor ignores the app-level "always on top" flag.  
Fix it with a window rule:

1. **System Settings → Window Management → Window Rules**
2. **+ Add New** — Window class: `forza-tacho`
3. Add: **Keep above** → Force → Yes
4. Add: **Virtual Desktop** → Force → All Desktops
5. **Apply** — takes effect immediately.

</details>

---

## Performance

<details>
<summary>Open performance notes</summary>

Forza streams telemetry at 60 packets/second (~324 bytes each).

| Metric | Value |
|---|---|
| Processing time per packet | **< 1 ms** |
| Resident memory | **~10–15 MB** |
| CPU during active play | **< 1 %** |
| Runtime dependencies | **Zero** — single self-contained binary |

File I/O is fully decoupled from the hot path via a bounded background channel — the UDP receive thread never blocks on a syscall. Session writes are batched with `BufWriter`.

The SSE broadcast channel has a capacity of **32 frames**. A very slow client or many simultaneous dashboard windows may see dropped telemetry frames, which is acceptable for a tachometer display.

</details>

---

## Building from source

<details>
<summary>Open build instructions</summary>

You need [Rust](https://rustup.rs/) installed.

```bash
# debug build / run
cargo run

# optimised release build
cargo build --release
```

### Linux system dependencies

The GUI and audio require a few system libraries. On Fedora:

```bash
sudo dnf install alsa-lib-devel pipewire-alsa
```

On Debian/Ubuntu:

```bash
sudo apt install libxkbcommon-dev libwayland-dev libegl-dev libgl1-mesa-dev \
                 libasound2-dev pulseaudio-alsa
```

`alsa-lib-devel` / `libasound2-dev` is required for the shift sound cue (rodio/CPAL audio backend). On modern desktops the ALSA bridge (`pipewire-alsa` or `pulseaudio-alsa`) routes audio to PipeWire/PulseAudio — install it if no sound plays.

### macOS universal binary

The CI builds two binaries — Intel (`x86_64`) and Apple Silicon (`aarch64`) — then merges them with `lipo`.

### Cross-compiling for Windows (from Linux)

```bash
rustup target add x86_64-pc-windows-gnu
sudo dnf install mingw64-gcc        # Fedora
# sudo apt install gcc-mingw-w64    # Debian / Ubuntu

cargo build --release --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/forza-tacho.exe
```

The `.exe` is fully self-contained — no extra files needed.

</details>

---

## Technical deep-dive: the shift algorithm

<details>
<summary>Click to expand</summary>

### Stage 1 — Safety fallback

Before any real data exists, the engine limit is estimated at **94 % of Forza's reported `maxRpm`**. The shift warning fires at this limit minus a lead gap proportional to the usable RPM band. Gears 1–3 get tighter safety ratios (98–99 %) and slightly longer lead times.

### Stage 2 — Observed rev limit

Every full-throttle pass updates `maxObservedRpm`. Capped at **97 % of `maxRpm`** to prevent a single spike from collapsing the safety margin.

### Stage 3 — Limiter bounce detection

Holding full throttle at the limiter produces characteristic RPM oscillation. Detection:

1. Track the local RPM peak.
2. Once RPM drops ≥ 30 RPM from that peak (but < 400 RPM — larger drops are upshifts), record a reversal.
3. After **3 reversals within 1 second**, the limiter is confirmed.

### Stage 4 — Power curve learning

On full-throttle passes (throttle ≥ 99 %, brake ≤ 2 %) power output is sampled into **100-RPM buckets**, keeping the maximum per bucket. At least **6 filled buckets** are needed before the learned shift algorithm activates.

### Stage 5 — Gear drop ratio

Gear ratios are measured from actual upshifts:

```
drop_ratio = rpm_after / rpm_before
```

Ratios outside 0.35–0.92 are discarded. Valid ratios are averaged; later samples refine earlier ones.

### Stage 6 — Optimal shift point

With power curve and gear ratio available:

1. Find the **power peak RPM**.
2. For every RPM above the peak, simulate an upshift: `rpm_after = current × drop_ratio`.
3. Look up `power_after` by linear interpolation.
4. First RPM where `power_after ≥ current_power × 1.005` is the shift point.

The 0.5 % threshold prevents premature shifts on flat curves. If no such point exists, the safety limit is used.

### Stage 7 — Dynamic shift warning

```
warning_rpm = shift_rpm − clamp(rpm_rate × 0.20 s, 100, 800)
```

`rpm_rate` is smoothed per gear. A fallback gap of 1.2 % of shift RPM applies when rate is out of range.

### Cache validation

Cached shift points are validated live: deviation > 15 % from the stored curve invalidates the entry; < 5 % marks it *validated*.

Car identity is keyed on `ordinal:performanceIndex` — same car at different PI ratings gets separate curves.

</details>

---

## Releases

Releases are published on GitHub Releases. See [CHANGELOG.md](CHANGELOG.md) for what changed.

<details>
<summary>Open release security details</summary>

Release binaries are gated on:

- `cargo test`
- frontend syntax checks (`node --check`)
- SHA-256 checksum generation (`SHA256SUMS.txt`) for all uploaded binaries

The workflow also runs `cargo audit` and emits a GitHub warning if dependency advisories are found or the audit tool cannot be installed. The audit is intentionally non-blocking for packaging because upstream RustSec advisories can affect transitive GUI dependencies without an immediate available fix; check the workflow log before publishing security-sensitive releases.

Windows binaries are Authenticode-signed when these repository secrets are configured:

- `WINDOWS_CERT_PFX_BASE64` — base64-encoded `.pfx` certificate
- `WINDOWS_CERT_PASSWORD` — certificate password

Without those secrets, the workflow publishes unsigned binaries plus checksums.

</details>

---

## AI

Parts of this project were developed with AI assistance.

- [Claude Code](https://claude.ai/code) (Anthropic)

---

## License

MIT — see [LICENSE](LICENSE).
