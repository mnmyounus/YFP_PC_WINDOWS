//! Deletes every dummy file this app has previously written to a target
//! directory. Used both by the explicit "Cancel & Delete" action (purge
//! everything, don't leave partial multi-GB files sitting around) and by
//! `wipe_engine::run`'s own resume-from-restart calculation.
//!
//! Direct port of the Android app's `DummyFileCleaner.kt`: deliberately
//! re-lists from disk (rather than only deleting files the *current*
//! run's in-memory state knows about), so dummy files left over from a
//! previous run that crashed or was force-closed still get cleaned up
//! correctly.

use crate::engine::config::WipeConfig;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default)]
pub struct CleanupResult {
    pub files_deleted: u32,
    pub files_failed_to_delete: u32,
}

fn is_dummy_file_name(name: &str) -> bool {
    name.starts_with(WipeConfig::DUMMY_FILE_PREFIX) && name.ends_with(WipeConfig::DUMMY_FILE_SUFFIX)
}

/// Lists every existing YFP dummy file directly under `dir` (not
/// recursive -- the engine only ever writes flat into the target root,
/// never into subfolders, so a non-recursive scan is both correct and
/// avoids the (small but real) risk of a recursive scan wandering into
/// unrelated folders on a drive root and taking a long time on a large
/// volume with many unrelated files).
fn list_dummy_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            let name_matches = entry
                .file_name()
                .to_str()
                .map(is_dummy_file_name)
                .unwrap_or(false);
            is_file && name_matches
        })
        .map(|entry| entry.path())
        .collect()
}

/// Sum of bytes currently sitting in existing dummy files at `dir` -- used
/// by wipe_engine::run's `resume_from_bytes` parameter so a job resumed
/// after an app restart continues topping up toward the same target
/// instead of restarting the whole percentage calculation from zero.
pub fn sum_existing_dummy_file_bytes(dir: &Path) -> u64 {
    list_dummy_files(dir)
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

pub fn purge_all(dir: &Path) -> CleanupResult {
    let mut result = CleanupResult::default();
    for path in list_dummy_files(dir) {
        match fs::remove_file(&path) {
            Ok(()) => result.files_deleted += 1,
            Err(_) => result.files_failed_to_delete += 1,
        }
    }
    result
}
