//! Core overwrite engine -- storage-agnostic aside from the Win32 calls in
//! `target.rs`, no UI dependencies. Mirrors the Android app's `engine/`
//! package: same module boundaries, same responsibilities per file.

pub mod cleaner;
pub mod config;
pub mod random_filler;
pub mod target;
pub mod wipe_engine;

pub use config::{OverwritePattern, WipeConfig, WipeTargetInfo};
pub use wipe_engine::{WipeControl, WipeProgress, WipeState};
