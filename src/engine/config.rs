//! Job configuration types. This module is the Rust equivalent of the
//! Android app's `WipeConfig.kt` -- same design, same rationale, ported to
//! the platform's idioms (no Parcelable needed here since this process
//! never hands config to another process/service; it all lives in one
//! app's memory for the run's duration).

use std::path::PathBuf;

/// Overwrite pattern used to fill dummy files.
///
/// See the Android app's identical enum for the full rationale (this
/// project intentionally keeps the same two options and the same
/// reasoning, since the underlying threat model -- defeating common
/// undelete tools by overwriting the logical free space a deleted file's
/// data used to occupy -- is identical across platforms).
///
/// Neither pattern is a forensic-grade erasure claim (e.g. DoD 5220.22-M).
/// On flash storage (many modern laptops/SSDs), wear-leveling means the
/// physical NAND cells backing a given logical sector aren't deterministic
/// -- a limitation shared by every consumer free-space-wipe tool on any
/// platform, not something specific to this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePattern {
    ZeroFill,
    PseudoRandom,
}

/// A single target drive/volume the engine can write into.
#[derive(Debug, Clone)]
pub struct WipeTargetInfo {
    /// Human-readable label, e.g. "Local Disk (C:)" or "USB Drive (E:)".
    pub label: String,
    /// Root path to write dummy files under, e.g. `C:\` or a user-picked
    /// folder. Unlike the Android app, Windows has no Storage-Access-
    /// Framework distinction between "internal" and "external, needs a
    /// grant" -- any path the current user can already write to (which
    /// the OS enforces via normal NTFS permissions) is writable the same
    /// way, so there is exactly one target-resolution path here instead
    /// of the Android app's two (APP_INTERNAL vs SAF_TREE).
    pub root_path: PathBuf,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_removable: bool,
}

#[derive(Debug, Clone)]
pub struct WipeConfig {
    pub target: WipeTargetInfo,
    pub pattern: OverwritePattern,
    /// 1..=95. Spec calls for up to 95%; kept as a config value (not a
    /// hardcoded constant) for the same reason as the Android app: a
    /// single place to reason about the safety margin, and room for a
    /// future settings control.
    pub target_fill_percent: u8,
    /// Size of each dummy chunk file, in bytes. 1-4 GiB per the original
    /// spec -- a few large files sustain much higher write throughput than
    /// millions of small ones, avoiding filesystem I/O overhead per file.
    pub chunk_size_bytes: u64,
}

impl WipeConfig {
    pub const MIN_FILL_PERCENT: u8 = 50;
    pub const MAX_FILL_PERCENT: u8 = 95;
    pub const DEFAULT_FILL_PERCENT: u8 = 90;

    pub const MIN_CHUNK_MB: u64 = 1024; // 1 GiB
    pub const MAX_CHUNK_MB: u64 = 4096; // 4 GiB
    pub const DEFAULT_CHUNK_MB: u64 = 2048; // 2 GiB -- same balance rationale
    // as the Android app: few large files (avoids I/O overhead), but not
    // one single multi-GB file held open so long that a Cancel mid-chunk
    // has to discard a huge amount of already-written work.

    pub const DUMMY_FILE_PREFIX: &'static str = "yfp_dummy_";
    pub const DUMMY_FILE_SUFFIX: &'static str = ".bin";
}
