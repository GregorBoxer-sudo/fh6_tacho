# forza-tacho

A self-contained telemetry dashboard for Forza Horizon 6.  
Open a browser, point Forza at your PC's IP, and get a real-time tachometer with a shift indicator that **learns the optimal shift point for every car you drive**.

## Features

- **Adaptive shift light** — not just "shift at X% of max RPM". The app learns each car's actual power curve at full throttle and calculates the RPM where shifting to the next gear gives more power.
- **Rev limiter detection** — recognises the characteristic oscillation at the limiter and uses it to refine the shift point automatically.
- **Session recorder** — every drive is saved as a JSONL file with speed, RPM, G-forces, lap times and more.
- **Analytics dashboard** — browse past sessions and view per-car power curves in the browser.
- **Single binary** — the web frontend is embedded at compile time. Nothing to install or copy alongside the executable.

## Quick start

### 1. Enable Data Out in Forza

Go to **Settings → HUD and Gameplay → Data Out** and set:

| Setting | Value |
|---|---|
| Data Out | **On** |
| Data Out IP Address | your PC's local IP (e.g. `192.168.1.42`) |
| Data Out IP Port | **5300** |
| Data Out Packet Format | **Car Dash** |

### 2. Run the app

**Windows** — download `forza-tacho.exe` from the releases page and double-click it.  
A console window will print the URL to open.

**Linux / macOS**
```
cargo run --release
```

### 3. Open the dashboard

The console will print something like:
```
Web dashboard listening on 0.0.0.0:8765
Open: http://127.0.0.1:8765
Open: http://192.168.1.42:8765
```

Open that URL in a browser on any device on the same network.

## Command-line options

```
forza-tacho [OPTIONS]

Options:
  --http-host <HOST>      Address to bind the web server to [default: 0.0.0.0]
  --http-port <PORT>      Web server port [default: 8765]
  --udp-host <HOST>       Address to listen for Forza UDP packets [default: 0.0.0.0]
  --udp-port <PORT>       UDP port to listen on [default: 5300]
  --demo                  Run a simulated car instead of waiting for Forza
  --inspect               Log raw UDP packets to disk for debugging
  --inspect-dir <DIR>     Directory for packet logs [default: logs]
  --inspect-every <N>     Log every Nth packet [default: 30]
  --shift-cache-log       Write shift decisions to a JSONL log file
  --shift-cache-log-dir   Directory for shift logs [default: logs]
  --limiter-log           Print limiter-bounce detection events to stdout
  -h, --help              Print help
```

## Building from source

You need [Rust](https://rustup.rs/) installed.

```bash
# debug build (static files served live from disk — good for frontend work)
cargo run

# optimised release build
cargo build --release
```

### Cross-compiling for Windows (from Linux)

```bash
rustup target add x86_64-pc-windows-gnu
sudo dnf install mingw64-gcc        # Fedora / RHEL
# sudo apt install gcc-mingw-w64    # Debian / Ubuntu

cargo build --release --target x86_64-pc-windows-gnu
# output: target/x86_64-pc-windows-gnu/release/forza-tacho.exe
```

The `.exe` is fully self-contained. No extra files needed.

## Data storage

All data is written next to the executable:

```
forza-tacho(.exe)
data/
  power_curves.json     ← learned shift points, one entry per car
  drive_sessions/       ← one .jsonl file per session
logs/                   ← optional, only with --inspect or --shift-cache-log
```

The `data/` folder is created automatically on first run. Deleting it resets all learned data.

## How the shift learning works

On every full-throttle run the app samples power and RPM into 100-RPM buckets. Once enough buckets are filled it finds the RPM where the power gain from upshifting outweighs the loss — taking the actual gear-drop ratio (learned from your own upshifts) into account. The result is stored per car and updated continuously as you drive.

The rev limiter is detected by recognising the oscillation pattern it creates: alternating small rises and drops around a fixed RPM ceiling. Once confirmed, the observed limit is used to tighten the safety shift point.

## License

MIT — see [LICENSE](LICENSE).
