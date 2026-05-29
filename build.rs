// build.rs — service-worker cache version helper
//
// Tells Cargo to re-run this script whenever any file in static/ changes.
// The build will then emit a reminder warning so developers don't forget to
// bump CACHE_NAME in static/sw.js.
//
// Full automation (deriving CACHE_NAME from a content hash at build time)
// is tracked as a future improvement; this provides a loud, low-effort
// safety net in the meantime.

use std::{
    fs,
    path::Path,
};

fn main() {
    // Re-run if any static asset changes.
    println!("cargo:rerun-if-changed=static/");

    // Read the current CACHE_NAME from sw.js so we can show it in the warning.
    let sw_path = Path::new("static/sw.js");
    let cache_name = fs::read_to_string(sw_path)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|l| l.contains("CACHE_NAME") && l.contains('"'))
                .and_then(|l| {
                    let start = l.find('"')? + 1;
                    let end = l.rfind('"')?;
                    if end > start { Some(l[start..end].to_string()) } else { None }
                })
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Warn so the developer notices during a build that touches static/.
    // Silence this warning only after verifying CACHE_NAME was bumped.
    println!(
        "cargo:warning=static/ assets changed — verify CACHE_NAME in sw.js is \
         up-to-date (currently \"{cache_name}\"). Bump the version if any cached \
         asset was modified."
    );
}
