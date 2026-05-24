use eframe::egui::{self, Color32, RichText, ScrollArea};
use std::{
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{
    config::{Args, LauncherConfig},
    settings::AppSettings,
    telemetry::TelemetryHub,
    util::lan_addresses,
};

// ── LED colour scheme (mirrors static/style.css) ─────────────────────────────

/// Total number of shift LEDs — identical to the web frontend.
const NUM_LEDS: usize = 14;

/// Base RGB for LED at `index` (0-based).
/// Groups:  0-3 green | 4-7 yellow | 8-10 red | 11-13 purple
fn led_rgb(i: usize) -> (u8, u8, u8) {
    match i {
        0..=3 => (0x26, 0xf0, 0x6e),  // #26f06e
        4..=7 => (0xf3, 0xdf, 0x4e),  // #f3df4e
        8..=10 => (0xff, 0x36, 0x58), // #ff3658
        _ => (0xd8, 0x46, 0xff),      // #d846ff
    }
}

fn led_active(i: usize) -> Color32 {
    let (r, g, b) = led_rgb(i);
    Color32::from_rgb(r, g, b)
}

/// Inactive LED: #dce4e8 @ ~20 % opacity
fn led_dim() -> Color32 {
    Color32::from_rgba_unmultiplied(0xdc, 0xe4, 0xe8, 50)
}

/// Flash-ON: CSS `brightness(2.05)` — clamp each channel
fn led_flash_on(i: usize) -> Color32 {
    let (r, g, b) = led_rgb(i);
    let b2 = |v: u8| ((v as u32 * 205 / 100).min(255)) as u8;
    Color32::from_rgb(b2(r), b2(g), b2(b))
}

/// Flash-OFF: almost invisible (opacity ~6 %)
fn led_flash_off() -> Color32 {
    Color32::from_rgba_unmultiplied(0xdc, 0xe4, 0xe8, 15)
}

// ── Shift-LED state ───────────────────────────────────────────────────────────

struct LedState {
    /// How many LEDs are lit (0..=NUM_LEDS).
    active: usize,
    /// True once RPM ≥ shiftNowRpm (with hysteresis).
    flash: bool,
    /// Current flash phase: true = bright, alternates every 80 ms.
    flash_on: bool,
    /// Forza is actively sending packets.
    race_on: bool,
}

fn clampf(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

fn fv(v: &serde_json::Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .unwrap_or(0.0)
}

/// Compute LED state from the latest telemetry packet.
/// `time` is the egui clock in seconds (used for the 80 ms flash period).
/// `flash_active` / `flash_gear` are persistent state held in `ForzaTachoApp`.
fn compute_led_state(
    telemetry: Option<&serde_json::Value>,
    time: f64,
    flash_active: &mut bool,
    flash_gear: &mut i64,
) -> LedState {
    let off = LedState {
        active: 0,
        flash: false,
        flash_on: false,
        race_on: false,
    };

    let Some(tel) = telemetry else { return off };
    if !tel["raceOn"].as_bool().unwrap_or(false) {
        *flash_active = false;
        return off;
    }

    let engine = &tel["engine"];
    let rpm = fv(&engine["rpm"]);
    let idle = fv(&engine["idleRpm"]).max(0.0);
    let shift_now = fv(&engine["shiftNowRpm"]);
    let redline = fv(&engine["redlineRpm"]);
    let gear = tel["controls"]["gear"].as_i64().unwrap_or(0);

    // Reset flash when the driver changes gear (mirrors app.js shiftFlashState)
    if gear != *flash_gear {
        *flash_active = false;
        *flash_gear = gear;
    }

    let release_gap = clampf(shift_now * 0.025, 180.0, 350.0);
    if !*flash_active && shift_now > idle + 1000.0 && rpm >= shift_now {
        *flash_active = true;
    }
    if *flash_active && rpm < shift_now - release_gap {
        *flash_active = false;
    }

    // LEDs fill up to 97 % of shiftNowRpm (brief "all on, not blinking" state)
    let led_full = if shift_now > idle + 1000.0 {
        shift_now * 0.97
    } else {
        0.0
    };
    let led_ratio = if led_full > 0.0 {
        clampf(rpm / led_full, 0.0, 1.0)
    } else if redline > 0.0 {
        clampf(rpm / redline, 0.0, 1.0)
    } else {
        0.0
    };

    let active = (led_ratio * NUM_LEDS as f64).round() as usize;
    // 80 ms flash period: Math.floor(now_ms / 80) % 2
    let flash_on = *flash_active && ((time * 1000.0 / 80.0) as i64 % 2 == 0);

    LedState {
        active,
        flash: *flash_active,
        flash_on,
        race_on: true,
    }
}

// ── LED painter ───────────────────────────────────────────────────────────────

fn paint_leds(painter: &egui::Painter, rect: egui::Rect, state: &LedState) {
    let padding = 6.0_f32;
    let gap = 4.0_f32;
    let avail_w = rect.width() - 2.0 * padding;
    let avail_h = rect.height() - 2.0 * padding;

    // Radius: fit NUM_LEDS circles with gaps into available width, but also
    // respect available height so the overlay can be resized short.
    let r = ((avail_w - (NUM_LEDS as f32 - 1.0) * gap) / (NUM_LEDS as f32 * 2.0))
        .min(avail_h / 2.0)
        .max(3.0);

    let diameter = r * 2.0;
    let total_w = NUM_LEDS as f32 * diameter + (NUM_LEDS as f32 - 1.0) * gap;
    let start_x = rect.center().x - total_w / 2.0 + r;
    let cy = rect.center().y;

    for i in 0..NUM_LEDS {
        let cx = start_x + i as f32 * (diameter + gap);
        let center = egui::pos2(cx, cy);

        let bright = if state.flash {
            state.flash_on
        } else {
            i < state.active
        };

        // Single soft glow — capped to half the inter-LED gap so it never
        // bleeds into a neighbouring LED (centers are 2r+gap apart, boundary at r+gap/2).
        if bright {
            let (rr, gg, bb) = led_rgb(i);
            painter.circle_filled(
                center,
                r + gap * 0.35, // stays well inside r+gap/2 boundary
                Color32::from_rgba_unmultiplied(rr, gg, bb, 65),
            );
        }

        let color = if state.flash {
            if state.flash_on {
                led_flash_on(i)
            } else {
                led_flash_off()
            }
        } else if i < state.active {
            led_active(i)
        } else {
            led_dim()
        };

        painter.circle_filled(center, r, color);
    }
}

// ── Overlay viewport UI ───────────────────────────────────────────────────────

#[allow(deprecated)]
fn overlay_ui(ctx: &egui::Context, state: &LedState, close_signal: &Arc<AtomicBool>) {
    // Repaint fast during racing so the LEDs track 60 Hz telemetry.
    ctx.request_repaint_after(if state.race_on {
        if state.flash {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(16)
        }
    } else {
        Duration::from_millis(500)
    });

    // Transparent panel — the OS window itself is transparent via ViewportBuilder.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let rect = ui.max_rect();

            // Dark pill background
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(8),
                Color32::from_rgba_unmultiplied(10, 10, 14, 215),
            );

            // Full-area interaction: drag + right-click menu
            let resp = ui.interact(
                rect,
                egui::Id::new("overlay_drag"),
                egui::Sense::click_and_drag(),
            );

            // Hand off dragging to the OS — no manual position tracking needed.
            if resp.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            resp.context_menu(|ui| {
                ui.label(RichText::new("Shift-LED Overlay").strong().small());
                ui.separator();
                if ui.button("x  Close overlay").clicked() {
                    close_signal.store(true, Ordering::Relaxed);
                    ui.close();
                }
            });

            // Draw LEDs on top
            paint_leds(ui.painter(), rect, state);
        });
}

// ── Help / legend content ─────────────────────────────────────────────────────

fn help_ui(ui: &mut egui::Ui) {
    ui.add_space(4.0);

    // ── Main displays ─────────────────────────────────────────────────────
    ui.label(RichText::new("Main displays").strong().small());
    ui.add_space(3.0);

    egui::Grid::new("help_main")
        .num_columns(2)
        .spacing([14.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, field: &str, desc: &str| {
                ui.label(RichText::new(field).small().strong().monospace());
                ui.label(RichText::new(desc).small().weak());
                ui.end_row();
            };
            row(ui, "RPM", "Engine revolutions per minute.");
            row(ui, "Speed", "Vehicle speed in km/h.");
            row(ui, "Gear", "Current gear. 0 = neutral, negative = reverse.");
            row(
                ui,
                "Power",
                "Estimated wheel power at the current RPM (hp).",
            );
            row(ui, "Torque", "Engine torque at the current RPM (Nm).");
            row(ui, "Boost", "Turbo / supercharger boost pressure.");
            row(
                ui,
                "Lap / Best",
                "Lap number, current lap time, and fastest lap this session.",
            );
            row(ui, "Pos", "Race position — only in structured events.");
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── Control strip ─────────────────────────────────────────────────────
    ui.label(RichText::new("Control strip (right side)").strong().small());
    ui.add_space(3.0);

    egui::Grid::new("help_controls")
        .num_columns(2)
        .spacing([14.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, full: &str, desc: &str| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).small().strong().monospace());
                    ui.label(RichText::new(full).small().weak());
                });
                ui.label(RichText::new(desc).small().weak());
                ui.end_row();
            };
            row(ui, "GAS", "(Throttle)", "Accelerator pedal, 0–100 %.");
            row(ui, "BRK", "(Brake)", "Brake pedal, 0–100 %.");
            row(
                ui,
                "STR",
                "(Steering)",
                "Wheel position: -1.0 = full left, +1.0 = full right.",
            );
            row(
                ui,
                "DLT",
                "(Lap delta)",
                "Time gap to your best lap. Green = faster, red = slower.",
            );
            row(
                ui,
                "PROG",
                "(Lap progress)",
                "How far through the current lap, as a percentage.",
            );
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── Data strip ────────────────────────────────────────────────────────
    ui.label(RichText::new("Data strip").strong().small());
    ui.add_space(3.0);

    egui::Grid::new("help_data")
        .num_columns(2)
        .spacing([14.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, full: &str, desc: &str| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).small().strong().monospace());
                    ui.label(RichText::new(full).small().weak());
                });
                ui.label(RichText::new(desc).small().weak());
                ui.end_row();
            };
            row(ui, "LAT", "(Lateral G)", "Left/right cornering load in G.");
            row(
                ui,
                "LON",
                "(Longitudinal G)",
                "Acceleration / braking load in G.",
            );
            row(ui, "DRV", "(Drivetrain)", "FWD, RWD, or AWD.");
            row(
                ui,
                "FUEL",
                "(Fuel level)",
                "Percentage (<=100 %) or litres when > 1.2.",
            );
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── G-force meter & drift ─────────────────────────────────────────────
    ui.label(
        RichText::new("G-force meter and drift display")
            .strong()
            .small(),
    );
    ui.add_space(3.0);

    let notes: &[(&str, &str)] = &[
        (
            "G-meter dot",
            "Moves in two dimensions: left/right = lateral load, up/down = longitudinal load.",
        ),
        (
            "Np suffix (e.g. 1.23p)",
            "Peak value recorded since you started driving this session.",
        ),
        (
            "Drift angle",
            "Estimated side-slip angle in degrees. The sliding bar visualises the live angle.",
        ),
        ("Drift Np", "Peak drift angle seen this session."),
    ];
    for (title, body) in notes {
        ui.label(RichText::new(*title).small().strong());
        ui.label(RichText::new(*body).small().weak());
        ui.add_space(2.0);
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── Tyre corners ──────────────────────────────────────────────────────
    ui.label(
        RichText::new("Tyre corners  FL / FR / RL / RR")
            .strong()
            .small(),
    );
    ui.add_space(3.0);
    ui.label(RichText::new(
        "FL = Front Left, FR = Front Right, RL = Rear Left, RR = Rear Right.\n\
         Temperature box — tyre surface °C. Colour: blue (cold) → green (optimal) → red/white (overheating).\n\
         Slip LED strip (5 dots, shown at screen corners) — how hard that tyre is working. \
         All 5 lit = significant wheelspin or lockup."
    ).small().weak());

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── Warning indicators ────────────────────────────────────────────────
    ui.label(RichText::new("Warning indicators").strong().small());
    ui.add_space(3.0);

    egui::Grid::new("help_warnings")
        .num_columns(2)
        .spacing([14.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, full: &str, desc: &str| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).small().strong().monospace());
                    ui.label(RichText::new(full).small().weak());
                });
                ui.label(RichText::new(desc).small().weak());
                ui.end_row();
            };
            row(
                ui,
                "US",
                "(Understeer)",
                "Front tyres losing grip more than rears while steering — car pushes wide.",
            );
            row(
                ui,
                "OS",
                "(Oversteer)",
                "Rear tyres losing grip more than fronts while steering — rear steps out.",
            );
            row(
                ui,
                "B/T",
                "(Brake/Throttle)",
                "Both brake and throttle pressed at the same time (both > 8 %).",
            );
            row(
                ui,
                "TMP",
                "(Temperature)",
                "At least one tyre has exceeded 110 °C (grip noticeably degrades above this).",
            );
            row(
                ui,
                "ABS",
                "(ABS active)",
                "Anti-lock braking system is intervening under hard braking.",
            );
            row(
                ui,
                "LOCK",
                "(Wheel lock)",
                "A front wheel has locked up under hard braking.",
            );
            row(ui, "CL", "(Clutch)", "Clutch pedal is pressed (> 8 %).");
            row(ui, "HB", "(Handbrake)", "Handbrake is on (> 8 %).");
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── Shift indicator stages ────────────────────────────────────────────
    ui.label(
        RichText::new("Shift indicator — learning stages")
            .strong()
            .small(),
    );
    ui.add_space(3.0);

    let stages: &[(&str, &str)] = &[
        (
            "Stage 1 — No data yet",
            "Uses a conservative estimate: shifts at ~94 % of Forza's reported max RPM, \
          leaning early to avoid hitting the limiter.",
        ),
        (
            "Stage 2 — Rev limit observed",
            "After a few laps the app locks in the actual RPM ceiling it has seen and \
          tightens the warning accordingly.",
        ),
        (
            "Stage 3 — Limiter bounce detected",
            "If you hold full throttle at the limiter the characteristic RPM oscillation \
          is detected and the exact limit is confirmed.",
        ),
        (
            "Stage 4-5 — Power curve + gear ratios",
            "Full-throttle runs build a per-car power curve (100 RPM buckets). \
          Gear drop ratios are measured from your actual upshifts.",
        ),
        (
            "Stage 6 — Optimal shift point",
            "The app finds the RPM where power after an upshift exceeds current power \
          (>0.5 % threshold). That becomes the shift target.",
        ),
        (
            "Stage 7 — Dynamic warning",
            "The light fires early: warning_rpm = shift_rpm - clamp(rpm_rate x 0.20s, 100, 800). \
          Lead time scales with how fast RPM is climbing in the current gear.",
        ),
    ];

    for (title, body) in stages {
        ui.label(RichText::new(*title).small().strong());
        ui.label(RichText::new(*body).small().weak());
        ui.add_space(3.0);
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // ── Shift LED colours ─────────────────────────────────────────────────
    ui.label(RichText::new("Shift LED colours").strong().small());
    ui.add_space(3.0);

    egui::Grid::new("help_leds")
        .num_columns(2)
        .spacing([14.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let swatch = |ui: &mut egui::Ui, color: Color32, label: &str, desc: &str| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 6.0, color);
                    ui.label(RichText::new(label).small().strong());
                });
                ui.label(RichText::new(desc).small().weak());
                ui.end_row();
            };
            swatch(
                ui,
                Color32::from_rgb(0x26, 0xf0, 0x6e),
                "Green  (1–4)",
                "RPM building — shift point not yet near.",
            );
            swatch(
                ui,
                Color32::from_rgb(0xf3, 0xdf, 0x4e),
                "Yellow (5–8)",
                "Approaching the shift point.",
            );
            swatch(
                ui,
                Color32::from_rgb(0xff, 0x36, 0x58),
                "Red    (9–11)",
                "Getting close — prepare to shift.",
            );
            swatch(
                ui,
                Color32::from_rgb(0xd8, 0x46, 0xff),
                "Purple (12–14) + flash",
                "Shift NOW. All LEDs flash rapidly until you upshift.",
            );
        });
}

// ── Main app ──────────────────────────────────────────────────────────────────

pub(crate) struct ForzaTachoApp {
    hub: Arc<TelemetryHub>,
    http_port: u16,
    udp_port: u16,
    demo_mode: bool,
    local_only: bool,
    local_only_active: bool,
    config_path: PathBuf,
    settings_notice: String,
    lan_addresses: Vec<Ipv4Addr>,
    // Overlay
    show_overlay: bool,
    overlay_close: Arc<AtomicBool>,
    shift_flash_active: bool,
    shift_flash_gear: i64,
    // Shared app settings (updated by HTTP handler on save)
    settings: Arc<Mutex<AppSettings>>,
}

impl ForzaTachoApp {
    pub(crate) fn new(
        hub: Arc<TelemetryHub>,
        args: &Args,
        launcher_config: LauncherConfig,
        config_path: PathBuf,
        settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        Self {
            hub,
            http_port: args.http_port,
            udp_port: args.udp_port,
            demo_mode: args.demo,
            local_only: launcher_config.local_only,
            local_only_active: args.http_host == "127.0.0.1" && args.udp_host == "127.0.0.1",
            config_path,
            settings_notice: String::new(),
            lan_addresses: lan_addresses(),
            show_overlay: false,
            overlay_close: Arc::new(AtomicBool::new(false)),
            shift_flash_active: false,
            shift_flash_gear: 0,
            settings,
        }
    }
}

impl eframe::App for ForzaTachoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Handle close request sent from inside the overlay's context menu.
        if self.overlay_close.load(Ordering::Relaxed) {
            self.show_overlay = false;
            self.overlay_close.store(false, Ordering::Relaxed);
        }

        // Snapshot fields needed inside closures (avoids partial-borrow issues).
        let status = self.hub.status();
        let age_ms = status["ageMs"].as_u64();
        let packets = status["packets"].as_u64().unwrap_or(0);
        let is_live = age_ms.map(|ms| ms < 2000).unwrap_or(false);

        // Always compute LED/shift state so the audio trigger fires even when
        // the overlay window is not shown.
        {
            let latest = self.hub.latest();
            let time = ctx.input(|i| i.time);
            let prev_flash = self.shift_flash_active;
            let led = compute_led_state(
                latest.as_ref(),
                time,
                &mut self.shift_flash_active,
                &mut self.shift_flash_gear,
            );
            // false → true edge: fire the backend shift sound (if configured).
            if !prev_flash && led.flash {
                if let Ok(s) = self.settings.lock() {
                    crate::audio::play_shift_sound(&s.shift_sound_backend);
                }
            }
        }

        // Repaint rate: fast when live and any feature needs it.
        let needs_fast = is_live && {
            self.show_overlay
                || self
                    .settings
                    .lock()
                    .map(|s| s.shift_sound_backend != "none")
                    .unwrap_or(false)
        };
        ctx.request_repaint_after(Duration::from_millis(if needs_fast { 16 } else { 500 }));

        let demo_mode = self.demo_mode;
        let http_port = self.http_port;
        let udp_port = self.udp_port;
        let local_only_active = self.local_only_active;
        let lan_addresses = if local_only_active {
            vec![Ipv4Addr::LOCALHOST]
        } else {
            self.lan_addresses.clone()
        };

        let primary_ip: String = lan_addresses
            .iter()
            .find(|ip| !ip.is_loopback())
            .or_else(|| lan_addresses.first())
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());

        // Non-loopback first so the prominent "Open" button targets the LAN address.
        let mut sorted_addrs = lan_addresses.clone();
        sorted_addrs.sort_by_key(|ip| ip.is_loopback());

        let has_lan_ip = local_only_active || lan_addresses.iter().any(|ip| !ip.is_loopback());

        // ── Main window ───────────────────────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                ui.set_min_width(360.0);

                // ── Header ─────────────────────────────────────────────────
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🏎  forza-tacho").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                .small()
                                .weak(),
                        );
                    });
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                // ── Status ─────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    let (dot, label) = if demo_mode {
                        (Color32::from_rgb(255, 190, 20), "Demo mode running")
                    } else if is_live {
                        (Color32::from_rgb(60, 200, 60), "Receiving telemetry (60 Hz)")
                    } else {
                        (Color32::from_rgb(220, 55, 55), "Waiting for Forza packets...")
                    };
                    ui.label(RichText::new("●").color(dot).size(16.0));
                    ui.label(label);
                });
                if packets > 0 {
                    ui.label(
                        RichText::new(format!("   {} packets received", packets))
                            .small().weak(),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 1 · Set up Forza ───────────────────────────────────────
                ui.label(RichText::new("1.  Set up Forza").strong());
                ui.add_space(3.0);
                ui.label(
                    RichText::new("Settings → HUD and Gameplay → Data Out:")
                        .small().weak(),
                );
                ui.add_space(6.0);

                egui::Grid::new("forza_settings")
                    .num_columns(2)
                    .spacing([20.0, 5.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Data Out");
                        ui.label(
                            RichText::new("ON")
                                .color(Color32::from_rgb(60, 200, 60))
                                .strong(),
                        );
                        ui.end_row();

                        ui.label("IP Address");
                        ui.horizontal(|ui| {
                            ui.code(&primary_ip);
                            if ui.add(egui::Button::new("copy").small())
                                .on_hover_text("Copy to clipboard")
                                .clicked()
                            {
                                ui.ctx().copy_text(primary_ip.clone());
                            }
                        });
                        ui.end_row();

                        ui.label("Port");
                        ui.code(udp_port.to_string());
                        ui.end_row();
                    });

                if !has_lan_ip {
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new(
                            "⚠  No LAN IP detected — Forza and this app must run on the same PC.",
                        )
                        .color(Color32::from_rgb(255, 190, 20))
                        .small(),
                    );
                }
                if local_only_active {
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new(
                            "Local-only mode active — dashboard and UDP listener are bound to 127.0.0.1.",
                        )
                        .color(Color32::from_rgb(255, 190, 20))
                        .small(),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 2 · Open Dashboard ─────────────────────────────────────
                ui.label(RichText::new("2.  Open Dashboard").strong());
                ui.add_space(6.0);

                for (i, addr) in sorted_addrs.iter().enumerate() {
                    let url = format!("http://{}:{}", addr, http_port);
                    ui.horizontal(|ui| {
                        ui.code(&url);
                        let lbl = if i == 0 { "Open in browser" } else { "open" };
                        if ui.button(lbl).on_hover_text(&url).clicked() {
                            let u = url.clone();
                            std::thread::spawn(move || { let _ = open::that(u); });
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 3 · Shift-LED Overlay ──────────────────────────────────
                ui.label(RichText::new("3.  Shift-LED Overlay").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.show_overlay, "Show overlay");
                    ui.label(
                        RichText::new("always on top · draggable · resizable")
                            .small().weak(),
                    );
                });
                if self.show_overlay {
                    ui.label(
                        RichText::new(
                            "   Overlay running — right-click the overlay to close it.",
                        )
                        .small()
                        .color(Color32::from_rgb(60, 200, 60)),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Help / Legend ──────────────────────────────────────────
                egui::CollapsingHeader::new("?  Help & field legend")
                    .default_open(false)
                    .show(ui, |ui| {
                        help_ui(ui);
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Settings (collapsible) ─────────────────────────────────
                egui::CollapsingHeader::new("Settings")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        egui::Grid::new("settings_grid")
                            .num_columns(2)
                            .spacing([20.0, 5.0])
                            .show(ui, |ui| {
                                ui.label("HTTP port (dashboard)");
                                ui.code(http_port.to_string());
                                ui.end_row();
                                ui.label("UDP port (Forza Data Out)");
                                ui.code(udp_port.to_string());
                                ui.end_row();
                            });
                        ui.add_space(4.0);
                        let before = self.local_only;
                        ui.checkbox(
                            &mut self.local_only,
                            "Local-only mode (applies after restart)",
                        )
                        .on_hover_text(
                            "Binds HTTP and UDP to 127.0.0.1 so the dashboard is not exposed on the LAN.",
                        );
                        if self.local_only != before {
                            let config = LauncherConfig {
                                local_only: self.local_only,
                            };
                            self.settings_notice = match config.save(&self.config_path) {
                                Ok(()) => {
                                    if self.local_only {
                                        "Local-only saved. Restart forza-tacho to bind only to 127.0.0.1."
                                            .to_string()
                                    } else {
                                        "LAN mode saved. Restart forza-tacho to listen on the network again."
                                            .to_string()
                                    }
                                }
                                Err(e) => format!("Could not save launcher settings: {e}"),
                            };
                        }
                        if !self.settings_notice.is_empty() {
                            ui.label(
                                RichText::new(&self.settings_notice)
                                    .small()
                                    .color(Color32::from_rgb(255, 190, 20)),
                            );
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Change ports at startup with --http-port / --udp-port.",
                            )
                            .small().weak(),
                        );
                        if demo_mode {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Demo mode active (--demo)")
                                    .color(Color32::from_rgb(255, 190, 20)).small(),
                            );
                        }

                        // ── Shift sound (this device) ──────────────────────
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(RichText::new("Shift sound").strong().small());
                        ui.add_space(4.0);

                        // Read current backend sound without holding the lock across UI.
                        let mut snd_backend = {
                            let s = self.settings.lock().unwrap_or_else(|e| e.into_inner());
                            s.shift_sound_backend.clone()
                        };
                        let old_backend = snd_backend.clone();
                        let sounds = crate::settings::SOUND_NAMES;
                        let settings_file = self.config_path.with_file_name("settings.json");

                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("snd_backend")
                                .selected_text(&snd_backend)
                                .width(100.0)
                                .show_ui(ui, |ui| {
                                    for &name in sounds {
                                        ui.selectable_value(&mut snd_backend, name.to_string(), name);
                                    }
                                });
                            if ui.small_button("▶ Test").clicked() {
                                crate::audio::play_shift_sound(&snd_backend);
                            }
                        });

                        if snd_backend != old_backend {
                            let mut s = self.settings.lock().unwrap_or_else(|e| e.into_inner());
                            s.shift_sound_backend = snd_backend.clone();
                            let _ = crate::settings::save_settings(&settings_file, &s);
                            crate::audio::play_shift_sound(&snd_backend);
                        }

                        ui.add_space(3.0);
                        ui.label(
                            RichText::new("Plays on this machine. Set web sound in the browser dashboard.")
                                .small().weak(),
                        );

                        // ── KDE Wayland tip ────────────────────────────────
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("KDE Wayland — keep overlay always on top")
                                .strong().small(),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(
                                "On KDE Wayland the compositor ignores the app-level \
                                always-on-top flag. Fix it with a window rule:",
                            )
                            .small().weak(),
                        );
                        ui.add_space(6.0);
                        egui::Grid::new("kde_tip")
                            .num_columns(2)
                            .spacing([6.0, 4.0])
                            .show(ui, |ui| {
                                let step = |ui: &mut egui::Ui, n: &str, text: &str| {
                                    ui.label(RichText::new(n).small().strong());
                                    ui.label(RichText::new(text).small());
                                    ui.end_row();
                                };
                                step(ui, "1.", "System Settings > Window Management > Window Rules");
                                step(ui, "2.", "+ Add New — Window class: forza-tacho");
                                step(ui, "3.", "Keep above: Force, Yes");
                                step(ui, "4.", "Virtual Desktop: Force, All Desktops");
                                step(ui, "5.", "Apply — takes effect immediately, no restart needed");
                            });
                    });

                ui.add_space(8.0);
            });
        });

        // ── Overlay viewport (separate OS window) ─────────────────────────────
        if self.show_overlay {
            // Re-read the current telemetry for a fresh render (compute_led_state
            // was already called above for audio; calling it again is safe — the
            // state machine is idempotent on the same packet).
            let latest = self.hub.latest();
            let time = ctx.input(|i| i.time);
            let led_state = compute_led_state(
                latest.as_ref(),
                time,
                &mut self.shift_flash_active,
                &mut self.shift_flash_gear,
            );
            let close_sig = self.overlay_close.clone();

            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("shift_leds_overlay"),
                egui::ViewportBuilder::default()
                    .with_title("forza-tacho – Shift LEDs")
                    .with_decorations(false)
                    .with_always_on_top()
                    .with_transparent(true)
                    .with_resizable(true)
                    .with_inner_size([320.0, 48.0])
                    .with_min_inner_size([120.0, 24.0]),
                move |ctx, class| {
                    if class == egui::ViewportClass::EmbeddedWindow {
                        // Platform doesn't support separate windows — skip silently.
                        return;
                    }
                    overlay_ui(ctx, &led_state, &close_sig);
                },
            );
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub(crate) fn run(
    hub: Arc<TelemetryHub>,
    args: &Args,
    launcher_config: LauncherConfig,
    launcher_config_path: PathBuf,
    settings: Arc<Mutex<AppSettings>>,
) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("forza-tacho")
            .with_inner_size([440.0, 570.0])
            .with_min_inner_size([380.0, 460.0]),
        ..Default::default()
    };
    let args = args.clone();
    eframe::run_native(
        "forza-tacho",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(ForzaTachoApp::new(
                hub,
                &args,
                launcher_config,
                launcher_config_path,
                settings,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}
