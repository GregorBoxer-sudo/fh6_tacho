mod analytics;
mod audio;
mod config;
mod gui;
mod logging;
mod packet;
mod runtime;
mod settings;
mod shift;
mod telemetry;
mod util;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;

use analytics::TelemetryRecorder;
use config::{Args, LauncherConfig};
use logging::ShiftCacheLogger;
use runtime::{demo_loop, run_http, udp_loop};
use settings::load_settings;
use shift::PowerCurveStore;
use telemetry::TelemetryHub;
use util::lan_addresses;

// ─── Headless detection ──────────────────────────────────────────────────────

/// Returns `true` when there is no display available to render a window.
/// On Linux we check for both X11 ($DISPLAY) and Wayland ($WAYLAND_DISPLAY).
/// On Windows/macOS a display is always assumed to be present.
fn is_headless() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

// ─── main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let mut args = Args::parse();

    // Use the directory that contains the executable as the data root so the
    // app works correctly regardless of the working directory it is launched from.
    let root = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let launcher_config_path = root.join("data/launcher_config.json");
    let launcher_config = LauncherConfig::load(&launcher_config_path);
    if launcher_config.local_only {
        args.http_host = "127.0.0.1".to_string();
        args.udp_host = "127.0.0.1".to_string();
    }

    let logger = if args.shift_cache_log {
        Some(Arc::new(ShiftCacheLogger::new(
            &args.shift_cache_log_dir,
            args.shift_cache_log_keep,
        )?))
    } else {
        None
    };

    let power_curves = Arc::new(PowerCurveStore::new(
        root.join("data/power_curves.json"),
        logger,
        args.limiter_log,
        args.limiter_debug,
    ));
    let hub = Arc::new(TelemetryHub::new());
    let recorder = Arc::new(TelemetryRecorder::new(root.join("data/drive_sessions"))?);

    // Shared settings — loaded once; updated in-place by the HTTP handler.
    let app_settings = Arc::new(Mutex::new(load_settings(&root.join("data/settings.json"))));

    let use_gui = !args.no_gui && !is_headless();

    if use_gui {
        run_with_gui(
            args,
            root,
            launcher_config,
            launcher_config_path,
            hub,
            power_curves,
            recorder,
            app_settings,
        )
    } else {
        run_terminal(args, root, hub, power_curves, recorder, app_settings)
    }
}

// ─── GUI mode ────────────────────────────────────────────────────────────────

/// Starts the Tokio runtime in a background thread, then opens the egui window
/// on the main thread (required by most window managers).
fn run_with_gui(
    args: Args,
    root: std::path::PathBuf,
    launcher_config: LauncherConfig,
    launcher_config_path: std::path::PathBuf,
    hub: Arc<TelemetryHub>,
    power_curves: Arc<PowerCurveStore>,
    recorder: Arc<TelemetryRecorder>,
    app_settings: Arc<Mutex<settings::AppSettings>>,
) -> Result<()> {
    // Clone handles for the background thread.
    let hub_bg = hub.clone();
    let args_bg = args.clone();
    let root_bg = root.clone();
    let power_curves_bg = power_curves.clone();
    let recorder_bg = recorder.clone();
    let settings_bg = app_settings.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build Tokio runtime");

        rt.block_on(async move {
            if args_bg.demo {
                tokio::spawn(demo_loop(
                    hub_bg.clone(),
                    power_curves_bg.clone(),
                    recorder_bg.clone(),
                ));
                eprintln!("demo telemetry running at 60 Hz");
            } else {
                tokio::spawn(udp_loop(
                    hub_bg.clone(),
                    args_bg.clone(),
                    power_curves_bg.clone(),
                    recorder_bg.clone(),
                ));
            }

            eprintln!(
                "web dashboard available at {}:{}",
                args_bg.http_host, args_bg.http_port
            );
            for address in lan_addresses() {
                eprintln!("  http://{}:{}", address, args_bg.http_port);
            }

            if let Err(e) = run_http(root_bg.join("data"), hub_bg, &args_bg, settings_bg)
                .await
                .context("HTTP server")
            {
                eprintln!("server error: {e:#}");
            }
        });
    });

    // egui/eframe must run on the main thread.
    gui::run(hub, &args, launcher_config, launcher_config_path, app_settings)
}

// ─── Terminal mode ───────────────────────────────────────────────────────────

/// Classic terminal-only mode: Tokio runs on the main thread, no window opened.
fn run_terminal(
    args: Args,
    root: std::path::PathBuf,
    hub: Arc<TelemetryHub>,
    power_curves: Arc<PowerCurveStore>,
    recorder: Arc<TelemetryRecorder>,
    app_settings: Arc<Mutex<settings::AppSettings>>,
) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build Tokio runtime")
        .block_on(async move {
            if args.demo {
                tokio::spawn(demo_loop(
                    hub.clone(),
                    power_curves.clone(),
                    recorder.clone(),
                ));
                println!("demo telemetry running at 60 Hz");
            } else {
                tokio::spawn(udp_loop(
                    hub.clone(),
                    args.clone(),
                    power_curves.clone(),
                    recorder.clone(),
                ));
            }

            println!(
                "web dashboard available at {}:{}",
                args.http_host, args.http_port
            );
            for address in lan_addresses() {
                println!("  http://{}:{}", address, args.http_port);
            }

            run_http(root.join("data"), hub, &args, app_settings)
                .await
                .context("HTTP server")
        })
}
