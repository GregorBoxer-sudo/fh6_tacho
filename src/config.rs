use clap::Parser;
use std::path::PathBuf;

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
pub(crate) const SHIFT_CACHE_STRATEGY_VERSION: i64 = 5; // bump when the shift strategy changes to force a recalculation
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
/// Minimum RPM swing (half-amplitude) needed to count as a direction reversal at the limiter.
pub(crate) const LIMITER_BOUNCE_MIN_AMPLITUDE: f64 = 30.0;
/// Maximum swing per half-oscillation — larger drops are gear changes, not limiter bounce.
pub(crate) const LIMITER_BOUNCE_MAX_AMPLITUDE: f64 = 400.0;
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
pub(crate) const SHIFT_CACHE_VALIDATION_RPM_WINDOW: f64 = 300.0;

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
}
