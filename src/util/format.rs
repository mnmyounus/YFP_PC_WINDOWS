//! Small formatting helpers used only by the UI layer. Byte-count
//! formatting lives in `engine::target::format_bytes` instead of here,
//! since it's conceptually a storage-domain concern the engine itself
//! also wants (e.g. for log/error messages) -- this module is reserved
//! for formatting that's purely presentational and has no engine-side use.

/// Formats a duration in seconds as `H:MM:SS` or `M:SS`, matching the
/// Android app's `formatEta` helper.
pub fn format_eta(total_seconds: u64) -> String {
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_speed(bytes_per_sec: u64) -> String {
    let mbps = bytes_per_sec as f64 / (1024.0 * 1024.0);
    format!("{mbps:.1} MB/s")
}
