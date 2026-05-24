//! Persistent user preferences stored as JSON in the data directory.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// Valid shift-sound names (also accepted by the audio module).
pub(crate) const SOUND_NAMES: &[&str] = &["none", "blip", "click", "beep", "chord", "buzz"];

pub(crate) fn is_valid_sound(s: &str) -> bool {
    SOUND_NAMES.contains(&s)
}

/// Application-wide user preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSettings {
    /// Display unit system: `"metric"` (SI, default) or `"imperial"` (mph / lb-ft / °F).
    #[serde(default = "default_metric")]
    pub unit_system: String,

    /// Shift sound played in the browser ("none" = disabled).
    #[serde(default = "default_sound_web")]
    pub shift_sound_web: String,

    /// Shift sound played on the backend/server device ("none" = disabled).
    #[serde(default = "default_sound_backend")]
    pub shift_sound_backend: String,
}

fn default_metric() -> String {
    "metric".to_string()
}
fn default_sound_web() -> String {
    "blip".to_string()
}
fn default_sound_backend() -> String {
    "none".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            unit_system: default_metric(),
            shift_sound_web: default_sound_web(),
            shift_sound_backend: default_sound_backend(),
        }
    }
}

/// Reads settings from `path`; returns defaults silently on any error.
pub(crate) fn load_settings(path: &Path) -> AppSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Serialises `settings` to `path` as pretty-printed JSON.
pub(crate) fn save_settings(path: &Path, settings: &AppSettings) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(path, json)?;
    Ok(())
}
