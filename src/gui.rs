use eframe::egui::{self, Color32, RichText, ScrollArea};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{config::Args, telemetry::TelemetryHub, util::lan_addresses};

// ── LED colour scheme (mirrors static/style.css) ─────────────────────────────

/// Total number of shift LEDs — identical to the web frontend.
const NUM_LEDS: usize = 14;

/// Base RGB for LED at `index` (0-based).
/// Groups:  0-3 green | 4-7 yellow | 8-10 red | 11-13 purple
fn led_rgb(i: usize) -> (u8, u8, u8) {
    match i {
        0..=3  => (0x26, 0xf0, 0x6e), // #26f06e
        4..=7  => (0xf3, 0xdf, 0x4e), // #f3df4e
        8..=10 => (0xff, 0x36, 0x58), // #ff3658
        _      => (0xd8, 0x46, 0xff), // #d846ff
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
    let off = LedState { active: 0, flash: false, flash_on: false, race_on: false };

    let Some(tel) = telemetry else { return off };
    if !tel["raceOn"].as_bool().unwrap_or(false) {
        *flash_active = false;
        return off;
    }

    let engine     = &tel["engine"];
    let rpm        = fv(&engine["rpm"]);
    let idle       = fv(&engine["idleRpm"]).max(0.0);
    let shift_now  = fv(&engine["shiftNowRpm"]);
    let redline    = fv(&engine["redlineRpm"]);
    let gear       = tel["controls"]["gear"].as_i64().unwrap_or(0);

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
    let led_full = if shift_now > idle + 1000.0 { shift_now * 0.97 } else { 0.0 };
    let led_ratio = if led_full > 0.0 {
        clampf(rpm / led_full, 0.0, 1.0)
    } else if redline > 0.0 {
        clampf(rpm / redline, 0.0, 1.0)
    } else {
        0.0
    };

    let active   = (led_ratio * NUM_LEDS as f64).round() as usize;
    // 80 ms flash period: Math.floor(now_ms / 80) % 2
    let flash_on = *flash_active && ((time * 1000.0 / 80.0) as i64 % 2 == 0);

    LedState { active, flash: *flash_active, flash_on, race_on: true }
}

// ── LED painter ───────────────────────────────────────────────────────────────

fn paint_leds(painter: &egui::Painter, rect: egui::Rect, state: &LedState) {
    let padding = 6.0_f32;
    let gap     = 4.0_f32;
    let avail_w = rect.width()  - 2.0 * padding;
    let avail_h = rect.height() - 2.0 * padding;

    // Radius: fit NUM_LEDS circles with gaps into available width, but also
    // respect available height so the overlay can be resized short.
    let r = ((avail_w - (NUM_LEDS as f32 - 1.0) * gap) / (NUM_LEDS as f32 * 2.0))
        .min(avail_h / 2.0)
        .max(3.0);

    let diameter = r * 2.0;
    let total_w  = NUM_LEDS as f32 * diameter + (NUM_LEDS as f32 - 1.0) * gap;
    let start_x  = rect.center().x - total_w / 2.0 + r;
    let cy       = rect.center().y;

    for i in 0..NUM_LEDS {
        let cx     = start_x + i as f32 * (diameter + gap);
        let center = egui::pos2(cx, cy);

        let bright = if state.flash { state.flash_on } else { i < state.active };

        // Glow rings behind active LEDs
        if bright {
            let (rr, gg, bb) = led_rgb(i);
            painter.circle_filled(
                center, r + 7.0,
                Color32::from_rgba_unmultiplied(rr, gg, bb, 18),
            );
            painter.circle_filled(
                center, r + 3.5,
                Color32::from_rgba_unmultiplied(rr, gg, bb, 40),
            );
        }

        let color = if state.flash {
            if state.flash_on { led_flash_on(i) } else { led_flash_off() }
        } else if i < state.active {
            led_active(i)
        } else {
            led_dim()
        };

        painter.circle_filled(center, r, color);
    }
}

// ── Overlay viewport UI ───────────────────────────────────────────────────────

fn overlay_ui(
    ctx:          &egui::Context,
    state:        &LedState,
    close_signal: &Arc<AtomicBool>,
) {
    // Repaint fast during racing so the LEDs track 60 Hz telemetry.
    ctx.request_repaint_after(if state.race_on {
        if state.flash { Duration::from_millis(40) } else { Duration::from_millis(16) }
    } else {
        Duration::from_millis(500)
    });

    // Transparent panel — the OS window itself is transparent via ViewportBuilder.
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let rect = ui.max_rect();

            // Dark pill background
            ui.painter().rect_filled(
                rect,
                egui::Rounding::same(8.0),
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
                if ui.button("✕  Overlay schließen").clicked() {
                    close_signal.store(true, Ordering::Relaxed);
                    ui.close_menu();
                }
            });

            // Draw LEDs on top
            paint_leds(ui.painter(), rect, state);
        });
}

// ── Main app ──────────────────────────────────────────────────────────────────

pub(crate) struct ForzaTachoApp {
    hub:          Arc<TelemetryHub>,
    http_port:    u16,
    udp_port:     u16,
    demo_mode:    bool,
    lan_addresses: Vec<Ipv4Addr>,
    // Overlay
    show_overlay:       bool,
    overlay_close:      Arc<AtomicBool>,
    shift_flash_active: bool,
    shift_flash_gear:   i64,
}

impl ForzaTachoApp {
    pub(crate) fn new(hub: Arc<TelemetryHub>, args: &Args) -> Self {
        Self {
            hub,
            http_port:    args.http_port,
            udp_port:     args.udp_port,
            demo_mode:    args.demo,
            lan_addresses: lan_addresses(),
            show_overlay:       false,
            overlay_close:      Arc::new(AtomicBool::new(false)),
            shift_flash_active: false,
            shift_flash_gear:   0,
        }
    }
}

impl eframe::App for ForzaTachoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(500));

        // Handle close request sent from inside the overlay's context menu.
        if self.overlay_close.load(Ordering::Relaxed) {
            self.show_overlay = false;
            self.overlay_close.store(false, Ordering::Relaxed);
        }

        // Snapshot fields needed inside closures (avoids partial-borrow issues).
        let status  = self.hub.status();
        let age_ms  = status["ageMs"].as_u64();
        let packets = status["packets"].as_u64().unwrap_or(0);
        let is_live = age_ms.map(|ms| ms < 2000).unwrap_or(false);

        let demo_mode     = self.demo_mode;
        let http_port     = self.http_port;
        let udp_port      = self.udp_port;
        let lan_addresses = self.lan_addresses.clone();

        let primary_ip: String = lan_addresses
            .iter()
            .find(|ip| !ip.is_loopback())
            .or_else(|| lan_addresses.first())
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());

        // Non-loopback first so the prominent "Open" button targets the LAN address.
        let mut sorted_addrs = lan_addresses.clone();
        sorted_addrs.sort_by_key(|ip| ip.is_loopback());

        let has_lan_ip = lan_addresses.iter().any(|ip| !ip.is_loopback());

        // ── Main window ───────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
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
                        (Color32::from_rgb(255, 190, 20), "Demo-Modus läuft")
                    } else if is_live {
                        (Color32::from_rgb(60, 200, 60), "Empfange Telemetrie (60 Hz)")
                    } else {
                        (Color32::from_rgb(220, 55, 55), "Warte auf Forza-Pakete…")
                    };
                    ui.label(RichText::new("●").color(dot).size(16.0));
                    ui.label(label);
                });
                if packets > 0 {
                    ui.label(
                        RichText::new(format!("   {} Pakete empfangen", packets))
                            .small().weak(),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 1 · Forza einrichten ───────────────────────────────────
                ui.label(RichText::new("1.  Forza einrichten").strong());
                ui.add_space(3.0);
                ui.label(
                    RichText::new("Einstellungen → HUD und Gameplay → Data Out:")
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
                            RichText::new("EIN")
                                .color(Color32::from_rgb(60, 200, 60))
                                .strong(),
                        );
                        ui.end_row();

                        ui.label("IP-Adresse");
                        ui.horizontal(|ui| {
                            ui.code(&primary_ip);
                            if ui.add(egui::Button::new("copy").small())
                                .on_hover_text("In Zwischenablage kopieren")
                                .clicked()
                            {
                                ui.output_mut(|o| o.copied_text = primary_ip.clone());
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
                            "⚠  Keine LAN-IP — Forza und App müssen auf demselben PC laufen.",
                        )
                        .color(Color32::from_rgb(255, 190, 20))
                        .small(),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // ── 2 · Dashboard öffnen ───────────────────────────────────
                ui.label(RichText::new("2.  Dashboard öffnen").strong());
                ui.add_space(6.0);

                for (i, addr) in sorted_addrs.iter().enumerate() {
                    let url = format!("http://{}:{}", addr, http_port);
                    ui.horizontal(|ui| {
                        ui.code(&url);
                        let lbl = if i == 0 { "Im Browser öffnen" } else { "öffnen" };
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
                    ui.toggle_value(&mut self.show_overlay, "Overlay anzeigen");
                    ui.label(
                        RichText::new("immer im Vordergrund · verschiebbar · skalierbar")
                            .small().weak(),
                    );
                });
                if self.show_overlay {
                    ui.label(
                        RichText::new(
                            "   Overlay läuft — Rechtsklick auf das Overlay zum Schließen.",
                        )
                        .small()
                        .color(Color32::from_rgb(60, 200, 60)),
                    );
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Einstellungen (aufklappbar) ────────────────────────────
                egui::CollapsingHeader::new("⚙  Einstellungen")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        egui::Grid::new("settings_grid")
                            .num_columns(2)
                            .spacing([20.0, 5.0])
                            .show(ui, |ui| {
                                ui.label("HTTP-Port (Dashboard)");
                                ui.code(http_port.to_string());
                                ui.end_row();
                                ui.label("UDP-Port (Forza Data Out)");
                                ui.code(udp_port.to_string());
                                ui.end_row();
                            });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Ports beim Start über --http-port / --udp-port ändern.",
                            )
                            .small().weak(),
                        );
                        if demo_mode {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Demo-Modus aktiv (--demo)")
                                    .color(Color32::from_rgb(255, 190, 20)).small(),
                            );
                        }

                        // ── KDE Wayland Hinweis ────────────────────────────
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("KDE Wayland – Overlay immer im Vordergrund")
                                .strong().small(),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(
                                "Auf KDE Wayland ignoriert der Compositor das App-seitige \
                                'Always on top'-Flag. Loesung via Fensterregeln:",
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
                                step(ui, "1.", "Systemeinstellungen > Fenster-Verwaltung > Fensterregeln");
                                step(ui, "2.", "+ Neu > Fensterklasse: forza-tacho");
                                step(ui, "3.", "Anordnung: Erzwingen, Immer im Vordergrund");
                                step(ui, "4.", "Arbeitsflaeeche: Erzwingen, Alle Arbeitsflaechen");
                                step(ui, "5.", "Uebernehmen — gilt sofort, kein Neustart noetig");
                            });
                    });

                ui.add_space(8.0);
            });
        });

        // ── Overlay viewport (separate OS window) ─────────────────────────────
        if self.show_overlay {
            let latest     = self.hub.latest();
            let time       = ctx.input(|i| i.time);
            let led_state  = compute_led_state(
                latest.as_ref(),
                time,
                &mut self.shift_flash_active,
                &mut self.shift_flash_gear,
            );
            let close_sig  = self.overlay_close.clone();

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
                    if class == egui::ViewportClass::Embedded {
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

pub(crate) fn run(hub: Arc<TelemetryHub>, args: &Args) -> anyhow::Result<()> {
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
        Box::new(move |_cc| Ok(Box::new(ForzaTachoApp::new(hub, &args)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI-Fehler: {e}"))
}
