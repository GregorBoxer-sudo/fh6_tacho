use clap::Parser;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const DEFAULT_ENGINE_LIMIT_RATIO_OF_TACHO_MAX: f64 = 0.895;
/// Initial estimate for cars with no drive data yet.
/// More conservative than MAX_OBSERVED_LIMIT_RATIO (0.97) so the very first
/// drive doesn't immediately trigger a shift warning right at the limiter.
/// Better to warn a little early than to land in the rev limiter.
pub(crate) const INITIAL_ENGINE_LIMIT_RATIO_OF_TACHO_MAX: f64 = 0.94;
/// Cap on how high the observed limit is allowed to climb as a fraction of maxRpm.
/// Prevents an RPM spike from pushing observedLimit all the way to maxRpm and
/// collapsing the safety margin between the shift warning and Forza's maxRpm.
pub(crate) const MAX_OBSERVED_LIMIT_RATIO_OF_TACHO_MAX: f64 = 0.97;
pub(crate) const REDLINE_RATIO_OF_ENGINE_LIMIT: f64 = 1.0 - 500.0 / (9000.0 - 750.0);
pub(crate) const SAFETY_SHIFT_TARGET_RATIO_OF_ENGINE_LIMIT: f64 = 0.995;
pub(crate) const SHIFT_WARNING_LEAD_SECONDS: f64 = 0.20;
pub(crate) const SHIFT_WARNING_FALLBACK_GAP_RATIO: f64 = 0.012;
pub(crate) const SHIFT_WARNING_MIN_GAP_RPM: f64 = 100.0;
pub(crate) const SHIFT_WARNING_MAX_FALLBACK_GAP_RPM: f64 = 220.0;
pub(crate) const SHIFT_WARNING_MAX_DYNAMIC_GAP_RPM: f64 = 800.0;
pub(crate) const SAFETY_SHIFT_WARNING_FALLBACK_BAND_RATIO: f64 = 0.065;
pub(crate) const SAFETY_SHIFT_WARNING_MAX_BAND_RATIO: f64 = 0.18;
pub(crate) const SHIFT_WARNING_MIN_RPM_RATE: f64 = 350.0;
pub(crate) const SHIFT_WARNING_MAX_RPM_RATE: f64 = 14000.0;
pub(crate) const SHIFT_RPM_RATE_CAPTURE_SECONDS: f64 = 0.35;
pub(crate) const SHIFT_RPM_RATE_MIN_GAIN: f64 = 5.0;
pub(crate) const SHIFT_RPM_RATE_SMOOTHING: f64 = 0.25;
/// Minimum throttle for full-throttle learning (accel is u8/255, so 0.99 ≈ 252/255).
/// Lower values would let part-throttle data pollute the power curve.
pub(crate) const FULL_THROTTLE_MIN: f64 = 0.99;
/// Maximum brake input tolerated during full-throttle learning (overlap tolerance).
pub(crate) const FULL_THROTTLE_MAX_BRAKE: f64 = 0.02;
pub(crate) const POWER_CURVE_RPM_BUCKET: f64 = 100.0;
pub(crate) const POWER_CURVE_MIN_BUCKETS: usize = 6;
/// Minimum power gain required after an upshift to trigger a shift point.
/// 1.005 = next gear must deliver at least 0.5 % more power.
/// Prevents premature shifts on flat power curves (EVs, turbo plateau, noisy data).
pub(crate) const SHIFT_POWER_GAIN_RATIO: f64 = 1.005;
/// Bump when the shift strategy changes to force a recalculation of all cached
/// shift points.  Stored per car-curve, not at the file level.
pub(crate) const SHIFT_CACHE_STRATEGY_VERSION: i64 = 6;
/// Schema version for the on-disk power_curves.json file.
/// Governs structural changes to the JSON layout (added/removed top-level keys,
/// renamed fields) — distinct from SHIFT_CACHE_STRATEGY_VERSION which triggers
/// algorithm recompute on a per-curve basis.  Increment this when the file
/// structure changes; migrate_schema() handles the transition.
pub(crate) const SHIFT_CACHE_SCHEMA_VERSION: u64 = 1;
pub(crate) const MAX_PLAUSIBLE_LEARNED_GEAR: i64 = 10;
pub(crate) const SHIFT_DROP_MIN_RATIO: f64 = 0.35;
pub(crate) const SHIFT_DROP_MAX_RATIO: f64 = 0.92;
pub(crate) const SHIFT_DROP_CAPTURE_SECONDS: f64 = 0.8;
pub(crate) const CACHE_REVALIDATE_AFTER_SECONDS: f64 = 60.0;
pub(crate) const MAX_RPM_SIGNATURE_TOLERANCE: f64 = 0.01;
pub(crate) const IDLE_RPM_SIGNATURE_TOLERANCE: f64 = 0.25;
pub(crate) const POWER_SIGNATURE_TOLERANCE: f64 = 0.25;
pub(crate) const SHIFT_CACHE_VALID_POWER_TOLERANCE: f64 = 0.05;
pub(crate) const SHIFT_CACHE_INVALID_POWER_TOLERANCE: f64 = 0.15;
/// Minimum reversal amplitude as a fraction of the engine's maxRpm.
/// Calibrated so a typical 8 000 RPM car keeps the historic 30 RPM floor
/// (30 / 8000 = 0.00375), while a low-revving 5 000 RPM engine uses ~19 RPM,
/// letting it accumulate the required reversals even though its limiter
/// oscillation is tighter than the old fixed 30 RPM threshold.
pub(crate) const LIMITER_BOUNCE_MIN_AMPLITUDE_RATIO: f64 = 0.00375; // ≈ 30 RPM at 8 000 RPM
/// Maximum swing per half-oscillation, as a fraction of maxRpm.
/// Anything larger is more likely a gear change than a limiter bounce.
/// At 8 000 RPM this equals the historic 400 RPM (400 / 8000 = 0.05); at
/// 12 000 RPM it allows 600 RPM, matching hard-cut limiters on high-rev engines.
pub(crate) const LIMITER_BOUNCE_MAX_AMPLITUDE_RATIO: f64 = 0.05; // ≈ 400 RPM at 8 000 RPM
/// Time window in seconds within which direction reversals are counted.
pub(crate) const LIMITER_BOUNCE_WINDOW: f64 = 1.0;
/// Number of direction reversals required to confirm the limiter.
pub(crate) const LIMITER_BOUNCE_MIN_COUNT: i64 = 3;
/// Tolerance for downward correction of maxObservedRpm: only correct when the detected
/// limiter sits more than this fraction below the stored value.
pub(crate) const LIMITER_CORRECT_TOLERANCE: f64 = 0.015;
pub(crate) const LIMIT_LEARN_MIN_RPM_GAIN: f64 = 40.0;
pub(crate) const LIMIT_LEARN_MAX_SAMPLE_AGE: f64 = 0.5;
pub(crate) const LIMIT_LEARN_HIGH_RPM_RATIO: f64 = 0.88;
/// Lower RPM bound for power-curve and limit learning, as a fraction of Forza's maxRpm.
/// Avoids a hardcoded absolute value that would cut off low-revving cars (e.g. diesels,
/// EVs, tractors) — 20 % of a 2 500 RPM car is 500 RPM vs the old hardcoded 1 500.
pub(crate) const POWER_LEARN_MIN_RPM_RATIO: f64 = 0.20;
/// How long (seconds) RPM must be stable at full throttle in the high-RPM zone before
/// the continuation path is considered a terrain/speed plateau and is suppressed.
/// Prevents weak cars on hills from recording a falsely low maxObservedRpm.
pub(crate) const LIMIT_LEARN_PLATEAU_SECONDS: f64 = 2.0;
/// EMA blend weight for each new gear-drop ratio sample.
/// 0.25 → new = 0.75 × old + 0.25 × latest.  Converges in ~4 clean shifts while
/// letting old outlier measurements decay naturally (unlike a running average that
/// weights every historical sample equally forever).
pub(crate) const GEAR_DROP_EMA_RATE: f64 = 0.25;
/// Minimum number of accumulated gear-drop samples before the stored ratio is
/// trusted by compute_power_shift_rpm().  Until this threshold is met, the
/// function returns None so the safety shift path is used.
///
/// Set to 2 so a single outlier first shift (clutch slip, mid-corner lift,
/// hill crest) cannot immediately skew the shift point for that gear.  The
/// EMA blend (GEAR_DROP_EMA_RATE = 0.25) keeps converging toward the true
/// ratio regardless; this gate simply delays arming until a second data point
/// confirms the first is plausible.
pub(crate) const SHIFT_DROP_MIN_SAMPLES: i64 = 2;
/// Future-use tolerance for checking whether the EMA ratio has converged
/// enough to be trusted.  Currently unused in the gate logic (the samples
/// count is sufficient); reserved here so the concept has a named constant
/// and a defined home for future variance-based checks.
///
/// Interpretation: two ratios are "consistent" if they agree within ±6 %
/// of the larger one (e.g. 0.65 → acceptable range 0.611 – 0.689).
#[allow(dead_code)] // not yet wired in; reserved for future variance-based consistency gate
pub(crate) const SHIFT_DROP_CONSISTENCY_TOLERANCE: f64 = 0.06;
pub(crate) const SHIFT_CACHE_VALIDATION_RPM_WINDOW: f64 = 300.0;
/// Minimum number of distinct ascending runs through a power-curve bucket before
/// bidirectional EMA re-learning kicks in.  A "run" is one full-throttle pass
/// through the bucket from below (RPM enters the bucket rising from a lower bucket).
///
/// Run-counting is independent of acceleration: a fast car with only 1 sample per
/// pass and a slow car with 20 samples per pass both need the same number of actual
/// runs before the bucket is considered established.
pub(crate) const POWER_BUCKET_RELEARN_MIN_RUNS: i64 = 3;
/// A new reading must differ from the stored value by more than this fraction
/// in either direction to trigger an EMA update (0.05 = 5 %).
/// Keeps small sensor noise from moving established buckets while still
/// reacting to genuine changes and contaminated spikes (typically 20–50 % off).
pub(crate) const POWER_BUCKET_RELEARN_TOLERANCE: f64 = 0.05;
/// Per-sample EMA blend weight — applied symmetrically for both upward and
/// downward corrections once a bucket is established.
/// 0.12 → new = 0.88 × old + 0.12 × current each qualifying sample.
/// A sustained real change (10 qualifying samples) moves the value ~72 % of the way;
/// a single outlier only shifts it by 12 % and self-corrects on the next pass.
pub(crate) const POWER_BUCKET_RELEARN_RATE: f64 = 0.12;

#[derive(Parser, Debug, Clone)]
#[command(about = "Local Forza telemetry dashboard")]
pub(crate) struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    pub(crate) http_host: String,
    #[arg(long, default_value_t = 8765)]
    pub(crate) http_port: u16,
    #[arg(long, default_value = "0.0.0.0")]
    pub(crate) udp_host: String,
    #[arg(long, default_value_t = 5300)]
    pub(crate) udp_port: u16,
    #[arg(long)]
    pub(crate) demo: bool,
    #[arg(long)]
    pub(crate) inspect: bool,
    #[arg(long, default_value = "logs")]
    pub(crate) inspect_dir: PathBuf,
    #[arg(long, default_value_t = 30)]
    pub(crate) inspect_every: u64,
    #[arg(long)]
    pub(crate) shift_cache_log: bool,
    #[arg(long, default_value = "logs")]
    pub(crate) shift_cache_log_dir: PathBuf,
    #[arg(long, default_value_t = 5)]
    pub(crate) shift_cache_log_keep: usize,
    /// Log limiter-bounce detection to stdout (counters, confirmations, corrections).
    #[arg(long)]
    pub(crate) limiter_log: bool,
    /// Verbose per-packet debug log for the limiter zone (rpm, throttle, thresholds, bounce state).
    /// Prints one line per packet whenever RPM exceeds 80 % of maxRpm.
    #[arg(long)]
    pub(crate) limiter_debug: bool,
    /// Start without the graphical window (terminal-only mode).
    /// Enabled automatically when no display is detected on Linux.
    #[arg(long)]
    pub(crate) no_gui: bool,
    /// Enable debug-only tools in the web UI (e.g. map calibration mode).
    #[arg(long)]
    pub(crate) debug: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LauncherConfig {
    pub(crate) local_only: bool,
}

impl LauncherConfig {
    pub(crate) fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}
