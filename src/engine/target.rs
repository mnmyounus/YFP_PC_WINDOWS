//! Enumerates Windows drives/volumes and reads live free-space, mirroring
//! the Android app's `StorageInfo.kt`. Windows has a much simpler storage
//! model than Android's Scoped Storage -- there's no SAF grant dance, just
//! drive letters and NTFS permissions the OS already enforces -- so this
//! module is correspondingly smaller than its Android counterpart.

use crate::engine::config::WipeTargetInfo;
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, DRIVE_REMOVABLE,
};

/// Converts a Rust string to a null-terminated UTF-16 buffer, the format
/// every Win32 W-suffixed (wide/UTF-16) API expects.
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lists every drive letter currently mounted (A: through Z:), skipping
/// any that can't be queried (e.g. an empty optical drive) rather than
/// failing the whole enumeration for one bad entry.
pub fn list_drives() -> Vec<WipeTargetInfo> {
    let mut drives = Vec::new();

    // GetLogicalDrives returns a bitmask: bit 0 = A:, bit 1 = B:, etc.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return drives;
    }

    for i in 0..26u32 {
        if (mask & (1 << i)) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");

        if let Some(info) = query_drive(&root) {
            drives.push(info);
        }
    }

    drives
}

fn query_drive(root: &str) -> Option<WipeTargetInfo> {
    let wide_root = to_wide_null(root);
    let pcwstr = PCWSTR(wide_root.as_ptr());

    let drive_type = unsafe { GetDriveTypeW(pcwstr) };
    let is_removable = drive_type == DRIVE_REMOVABLE;

    let mut free_bytes_available = 0u64;
    let mut total_bytes = 0u64;
    let mut total_free_bytes = 0u64;

    let ok = unsafe {
        GetDiskFreeSpaceExW(
            pcwstr,
            Some(&mut free_bytes_available),
            Some(&mut total_bytes),
            Some(&mut total_free_bytes),
        )
    };

    // A drive letter can be *mounted* (shows up in GetLogicalDrives) but
    // not *ready* (e.g. an empty CD/DVD drive, a disconnected network
    // share) -- GetDiskFreeSpaceExW fails cleanly in that case, and we
    // skip it rather than showing the user a bogus 0-byte entry they
    // might pick and immediately fail against.
    if ok.is_err() || total_bytes == 0 {
        return None;
    }

    let label = format!(
        "{} ({})",
        if is_removable { "Removable Drive" } else { "Local Disk" },
        root.trim_end_matches('\\')
    );

    Some(WipeTargetInfo {
        label,
        root_path: PathBuf::from(root),
        total_bytes,
        free_bytes: free_bytes_available,
        is_removable,
    })
}

/// Live free-space query for a specific path (used by the engine during a
/// running wipe to re-check headroom periodically, not just once at job
/// start -- see wipe_engine.rs for why this matters: it's what keeps the
/// engine from driving the volume to zero free space if something else on
/// the system is writing to the same drive concurrently).
pub fn free_space_bytes(path: &std::path::Path) -> u64 {
    // GetDiskFreeSpaceExW wants a root-ish path; querying with the actual
    // target folder (not just the drive root) is fine and correct -- on
    // NTFS, free space is a property of the volume, and the API resolves
    // whatever path segment it's given up to its containing volume.
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return 0,
    };
    let wide = to_wide_null(path_str);
    let pcwstr = PCWSTR(wide.as_ptr());

    let mut free_bytes_available = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(pcwstr, Some(&mut free_bytes_available), None, None) };

    if ok.is_ok() {
        free_bytes_available
    } else {
        0
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit_index = 0;
    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }
    format!("{:.1} {}", value, UNITS[unit_index])
}
