mod analytics;
mod config;
mod logging;
mod packet;
mod runtime;
mod shift;
mod telemetry;
mod util;

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use analytics::TelemetryRecorder;
use config::Args;
use logging::ShiftCacheLogger;
use runtime::{demo_loop, run_http, udp_loop};
use shift::PowerCurveStore;
use telemetry::TelemetryHub;
use util::lan_addresses;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Use the directory that contains the executable as the data root so the app
    // works correctly regardless of the working directory it's launched from.
    let root = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
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

    if args.demo {
        tokio::spawn(demo_loop(
            hub.clone(),
            power_curves.clone(),
            recorder.clone(),
        ));
        println!("Demo telemetry running at 60 Hz");
    } else {
        tokio::spawn(udp_loop(
            hub.clone(),
            args.clone(),
            power_curves.clone(),
            recorder.clone(),
        ));
    }

    println!(
        "Web dashboard listening on {}:{}",
        args.http_host, args.http_port
    );
    for address in lan_addresses() {
        println!("Open: http://{}:{}", address, args.http_port);
    }

    run_http(root.join("data"), hub, &args)
        .await
        .context("running HTTP server")?;
    Ok(())
}
