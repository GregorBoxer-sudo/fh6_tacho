# forza-tacho

A real-time tachometer overlay for Forza Horizon 6 — with a shift light that actually learns your car.

![Dashboard](docs/dashboard.png)

---

## What is this?

forza-tacho runs in the background while you play Forza.  
It shows you a **shift indicator** that tells you exactly when to upshift for maximum power — not just "shift at 80% RPM", but the real optimal point for each car, learned from how you drive it.

You get:
- A **shift light overlay** you can drag anywhere on screen
- A **dashboard** in your browser with RPM, speed, gear, G-forces, lap times and more
- The dashboard works on **any device on the same network** — open it on a tablet or old phone, prop it up next to your monitor, or mount it behind your wheel as a second display. It also installs as a **web app (PWA)** so it looks and feels like a native app with no browser chrome in the way.

---

## Quick start — just want to play?

### 1. Download and open

Go to the **[Releases](../../releases)** page and download the right file for your system:

- **Windows:** `forza-tacho.exe` — double-click it. A small window opens, that's the launcher.
- **Linux:** `forza-tacho-linux-x86_64` — make it executable (`chmod +x`) and run it.  
  Works on any x86-64 desktop distro: Ubuntu, Fedora, Arch, CachyOS, Bazzite, Nobara, and others.
- **macOS:** `forza-tacho-macos` — same as Linux. One file that runs natively on both Intel Macs and Apple Silicon (M1/M2/M3) — no separate downloads.

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
> You can run it on any machine on the same network — a spare laptop, a Raspberry Pi, whatever.  
> Point Forza's Data Out at that machine's IP, and open the dashboard from anywhere on the network.

---

### 3. Open the dashboard

Click "Open in browser" in the launcher, or type the URL shown there into any browser on your network.

> **Second screen / tablet:**  
> The dashboard runs in the browser, so you can open it on a completely separate device.  
> A tablet propped up next to your monitor works great — or clamp one behind your steering wheel for a proper sim-racing cockpit feel.  
> On mobile you can install it as an app: tap *Share → Add to Home Screen* (iOS) or the install prompt in Chrome (Android), and it opens fullscreen without any browser UI.

---

### 4. Optional: shift LED overlay

In the launcher, toggle "Show overlay".  
A small row of coloured dots appears on screen — drag it wherever you want.  
It sits on top of the game and shows the shift indicator in real time.

Right-click the overlay to close it.

---

## How the shift light works

The indicator goes through several stages as you drive more.

**Stage 1 — First drive, no data yet**  
Uses a conservative estimate: warns slightly early rather than letting you hit the limiter.

**Stage 2 — Learning your rev limit**  
After a few laps the app has seen your car's actual RPM ceiling and refines the warning accordingly.

**Stage 3 — Rev limiter detection**  
If you hold full throttle at the limiter, the app detects the characteristic RPM oscillation and locks in the exact limit.

**Stages 4–7 — Power curve and optimal shift point**  
On full-throttle runs the app builds a power curve for your car (per car, per tune).  
Once it has enough data, it calculates the RPM where shifting to the next gear actually gives *more* power than staying — and that's when the light fires.  
Lead time is compensated dynamically so the light fires early enough for you to react.

All data is saved per car, so the second time you drive the same vehicle everything is already there.

---

## Command-line options

These are all optional — the defaults work out of the box.

```
forza-tacho [OPTIONS]

Options:
  --http-host <HOST>           Address to bind the web server [default: 0.0.0.0]
  --http-port <PORT>           Web server port [default: 8765]
  --udp-host <HOST>            Address for Forza UDP packets [default: 0.0.0.0]
  --udp-port <PORT>            UDP port [default: 5300]
  --demo                       Run a simulated car (no Forza needed)
  --no-gui                     Terminal-only mode, no window
  --inspect                    Log raw UDP packets to disk
  --inspect-dir <DIR>          Directory for packet logs [default: logs]
  --inspect-every <N>          Log every Nth packet [default: 30]
  --shift-cache-log            Write shift decisions to a JSONL log
  --shift-cache-log-dir <DIR>  Directory for shift logs [default: logs]
  --limiter-log                Print limiter-detection events to stdout
  --limiter-debug              Verbose per-packet limiter debug log
  -h, --help                   Print help
```

---

## Data storage

Everything is written next to the executable:

```
forza-tacho(.exe)
data/
  power_curves.json     <- learned shift points, one entry per car
  drive_sessions/       <- one .jsonl file per session
logs/                   <- only created with --inspect or --shift-cache-log
```

Deleting `data/` resets all learned shift data.

---

## Overlay on KDE Wayland

On KDE Wayland the compositor ignores the app-level "always on top" flag by default.  
Fix it with a window rule:

1. **System Settings → Window Management → Window Rules**
2. **+ Add New** — Window class: `forza-tacho`
3. Add property: **Keep above** → Force → Yes
4. Add property: **Virtual Desktop** → Force → All Desktops
5. **Apply** — takes effect immediately, no restart needed

---

## Performance

Forza streams telemetry at 60 packets/second (~324 bytes each).

| Metric | Value |
|---|---|
| Processing time per packet | **< 1 ms** |
| Resident memory | **~10–15 MB** |
| CPU during active play | **< 1%** |
| External dependencies at runtime | **Zero** — single binary |

Disk writes happen only when the power curve is updated (~every 0.3 s during learning, idle otherwise). Saves are atomic (write to `.tmp`, then rename) so a crash never corrupts stored data.

---

## Releases

Releases are built automatically by GitHub Actions when a version tag is pushed.  
Windows, Linux (x86-64), and macOS (universal) binaries are attached to every release.

To publish a new release:

```bash
git tag v0.2.0
git push origin v0.2.0
```

The workflow (`.github/workflows/release.yml`) builds both targets in parallel and creates the GitHub release with auto-generated notes.

---

## Building from source

You need [Rust](https://rustup.rs/) installed.

```bash
# debug build
cargo run

# optimised release build
cargo build --release
```

### Linux system dependencies

The GUI requires a few system libraries. On Debian/Ubuntu:

```bash
sudo apt install libxkbcommon-dev libwayland-dev libegl-dev libgl1-mesa-dev
```

On Fedora these are typically already present on a desktop install.

### macOS universal binary

The CI builds two separate binaries — one for Intel (`x86_64-apple-darwin`) and one for Apple Silicon (`aarch64-apple-darwin`) — then merges them into a single file using `lipo`. The result runs natively on both without Rosetta.

### Cross-compiling for Windows (from Linux)

CI uses a native Windows runner, but local cross-compilation also works:

```bash
rustup target add x86_64-pc-windows-gnu
sudo dnf install mingw64-gcc        # Fedora
# sudo apt install gcc-mingw-w64    # Debian / Ubuntu

cargo build --release --target x86_64-pc-windows-gnu
# -> target/x86_64-pc-windows-gnu/release/forza-tacho.exe
```

The `.exe` is fully self-contained — no extra files needed alongside it.

---

## Technical deep-dive: the shift algorithm

<details>
<summary>Click to expand</summary>

### Stage 1 — Safety fallback

Before any real data exists, the engine limit is estimated at **94 % of Forza's reported `maxRpm`**. The shift warning fires at this limit minus a lead gap proportional to the usable RPM band. Gears 1–3 get tighter safety ratios (98–99 %) and slightly longer lead times because RPM builds fastest there.

### Stage 2 — Observed rev limit

Every full-throttle pass in a forward gear updates `maxObservedRpm`. This is capped at **97 % of `maxRpm`** to prevent a single RPM spike from collapsing the safety margin. The value is persisted between sessions.

### Stage 3 — Limiter bounce detection

When holding full throttle at the limiter, Forza's engine simulation oscillates RPM in a characteristic pattern. The app detects this by counting *direction reversals*:

1. Track the local peak while RPM rises.
2. Once RPM drops ≥ 30 RPM from that peak (but < 400 RPM — larger drops are upshifts), record a reversal.
3. After **3 reversals within 1 second**, the limiter is confirmed.

If the confirmed limiter is more than 1.5 % *below* the stored value, all shift points for that car are recalculated.

### Stage 4 — Power curve learning

On every full-throttle pass (throttle ≥ 99 %, brake ≤ 2 %) the app samples power output and groups it into **100-RPM buckets**, keeping the *maximum* seen per bucket. At least **6 filled buckets** are needed before the learned shift algorithm activates.

The curve is saved atomically every 20 samples.

### Stage 5 — Gear drop ratio

Forza doesn't expose gear ratios directly, so the app measures them from your actual shifts.  
When an upshift is detected (gear increases by exactly 1 within 0.8 s):

```
drop_ratio = rpm_after / rpm_before
```

Ratios outside 0.35–0.92 are discarded. Valid ratios are averaged with equal weight; a running weighted mean lets later, more accurate samples refine earlier ones.

### Stage 6 — Optimal shift point

Once both the power curve (≥ 6 buckets) and at least one gear drop ratio are available:

1. Find the **power peak RPM**.
2. For every RPM above the peak, simulate an upshift: `rpm_after = current × drop_ratio`.
3. Look up `power_after` via linear interpolation.
4. The first RPM where `power_after ≥ current_power × 1.005` is the shift point.

The 0.5 % threshold prevents premature shifts on flat power curves (EVs, turbo plateau, noisy data). If no such point exists, the safety limit is used.

### Stage 7 — Dynamic shift warning

The indicator fires *early* to compensate for reaction time:

```
warning_rpm = shift_rpm − clamp(rpm_rate × 0.20s, 100, 800)
```

`rpm_rate` is a smoothed measurement of how fast RPM is currently climbing, tracked per gear. If the rate is outside a plausible range (e.g. you're cruising), a fallback gap of 1.2 % of shift RPM is used instead.

### Cache validation

Cached shift points are validated live: if measured power deviates more than 15 % from the stored curve, the entry is invalidated and recomputed. Once deviation stays below 5 %, the entry is marked *validated*.

Car identity is keyed on `ordinal:performanceIndex`, so the same car at different PI ratings gets separate curves.

</details>

---

## AI

Parts of this project were developed with AI assistance — particularly the shift-learning algorithm, limiter-bounce detection, and test suite. All generated code was reviewed and integrated by the author.

- [Claude](https://claude.ai) (Anthropic) — via Claude Code
- [Copilot / Codex](https://github.com/features/copilot) (OpenAI)

---

## License

MIT — see [LICENSE](LICENSE).
