//! The overwrite loop itself. Direct port of the Android app's
//! `WipeEngine.kt` -- same safety guarantees, same state machine, same
//! reasoning -- adapted to Rust's ownership model (no GC, so file handles
//! and buffers are owned values, not references into a JVM heap) and to
//! `std::sync` primitives instead of `java.util.concurrent`.

use crate::engine::config::{OverwritePattern, WipeConfig};
use crate::engine::random_filler::FastRandomFiller;
use crate::engine::target;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Never let the free-space calculation drive the volume below this many
/// bytes free, *regardless* of what target_fill_percent implies.
///
/// Same rationale as the Android app's identical constant:
///   1. Free space can be consumed concurrently by other processes while
///      this job runs; without a floor, a job that started with a safe
///      margin could still race itself down to zero.
///   2. Windows itself (and many running applications) can behave badly --
///      failing writes, showing confusing errors, in extreme cases
///      becoming unstable -- once free space on the system volume hits
///      genuine single-digit MB.
/// 64 MiB is comfortably more than routine OS/application housekeeping
/// needs, while being small enough not to meaningfully change the
/// *effective* max fill percentage on any realistically sized volume.
pub const MIN_FREE_SPACE_FLOOR_BYTES: u64 = 64 * 1024 * 1024;

/// Buffer size for each write() call -- large enough to amortize syscall
/// overhead, small enough to keep pause latency low and avoid a huge
/// transient allocation.
pub const WRITE_BUFFER_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WipeProgress {
    pub bytes_written_total: u64,
    pub bytes_target: u64,
    pub current_write_speed_bytes_per_sec: u64,
    pub files_created: u32,
}

impl WipeProgress {
    pub fn percent_complete(&self) -> u8 {
        if self.bytes_target == 0 {
            return 0;
        }
        let pct = (self.bytes_written_total as f64 / self.bytes_target as f64) * 100.0;
        pct.clamp(0.0, 100.0) as u8
    }

    pub fn estimated_seconds_remaining(&self) -> Option<u64> {
        if self.current_write_speed_bytes_per_sec == 0 {
            return None;
        }
        let remaining = self.bytes_target.saturating_sub(self.bytes_written_total);
        Some(remaining / self.current_write_speed_bytes_per_sec)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WipeState {
    Idle,
    Running(WipeProgress),
    Paused(WipeProgress),
    /// `hit_storage_limit` distinguishes two genuinely different outcomes,
    /// same distinction as the Android app's `WipeState.Completed`:
    ///   - false: the configured target percentage was reached normally.
    ///   - true: writing stopped early because live free space dropped
    ///     to/below the safety floor, or a write hit a disk-full error --
    ///     i.e. the volume had less headroom than the pre-flight
    ///     calculation assumed (most often because something else was
    ///     writing to the same volume concurrently).
    Completed {
        progress: WipeProgress,
        hit_storage_limit: bool,
    },
    Cancelled(WipeProgress),
    Failed {
        message: String,
        progress: Option<WipeProgress>,
    },
}

/// Shared control flags, cheap to clone (Arc-wrapped) so the UI thread can
/// signal pause/resume/cancel into the background worker thread without
/// needing a channel round-trip for what's fundamentally just "flip a
/// flag and possibly wake a condvar."
#[derive(Clone)]
pub struct WipeControl {
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    pause_condvar: Arc<(Mutex<()>, Condvar)>,
}

impl WipeControl {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            pause_condvar: Arc::new((Mutex::new(()), Condvar::new())),
        }
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        let (lock, cvar) = &*self.pause_condvar;
        let _guard = lock.lock().unwrap();
        cvar.notify_all();
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // Wake a possibly-paused loop so it can observe cancellation and
        // unwind instead of blocking forever waiting for a resume.
        let (lock, cvar) = &*self.pause_condvar;
        let _guard = lock.lock().unwrap();
        cvar.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl Default for WipeControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Blocks the calling (background worker) thread while paused is true,
/// waking immediately on resume() or cancel(). Emits an on_state callback
/// exactly once per pause (not on every wakeup check), so the UI doesn't
/// flicker between Running/Paused.
fn wait_while_paused(
    control: &WipeControl,
    current_progress: WipeProgress,
    on_state: &dyn Fn(WipeState),
    last_emitted_paused: &mut bool,
) {
    let (lock, cvar) = &*control.pause_condvar;
    let mut guard = lock.lock().unwrap();

    if control.paused.load(Ordering::SeqCst) && !control.is_cancelled() {
        if !*last_emitted_paused {
            on_state(WipeState::Paused(current_progress));
            *last_emitted_paused = true;
        }
        while control.paused.load(Ordering::SeqCst) && !control.is_cancelled() {
            guard = cvar.wait(guard).unwrap();
        }
        drop(guard);
        if !control.is_cancelled() {
            *last_emitted_paused = false;
            on_state(WipeState::Running(current_progress));
        }
    }
}

/// Runs the overwrite job on the calling thread until the target fill
/// percentage is reached, cancellation is requested, or an unrecoverable
/// error occurs. Meant to be invoked from a dedicated background thread
/// the caller spawns -- this function blocks for the job's full duration.
///
/// `resume_from_bytes` lets a caller resume a job across an app restart:
/// if the process was closed mid-wipe and relaunched, the caller can pass
/// in the sum of bytes already sitting in existing dummy files on disk
/// (see cleaner::sum_existing_dummy_file_bytes) so this run continues
/// topping up toward the same target instead of restarting the whole
/// percentage calculation from zero.
pub fn run(
    config: &WipeConfig,
    control: &WipeControl,
    resume_from_bytes: u64,
    on_progress: impl Fn(WipeProgress),
    on_state: impl Fn(WipeState),
) {
    let mut total_written = resume_from_bytes;

    let initial_free = target::free_space_bytes(&config.target.root_path);
    if initial_free == 0 {
        on_state(WipeState::Failed {
            message: "Could not read free space on the selected drive. It may have been disconnected.".to_string(),
            progress: None,
        });
        return;
    }

    // "Available to fill" = current free space, minus what we already hold
    // in progress (resume_from_bytes already occupies disk, it's not
    // additional headroom), minus the safety floor.
    let fillable_now = initial_free.saturating_sub(MIN_FREE_SPACE_FLOOR_BYTES);
    let target_bytes = (((fillable_now + resume_from_bytes) as f64)
        * (config.target_fill_percent as f64 / 100.0)) as u64;
    let target_bytes = target_bytes.max(resume_from_bytes); // never target *less* than what's already written

    let mut files_created_count: u32 = 0;
    let mut progress = WipeProgress {
        bytes_written_total: total_written,
        bytes_target: target_bytes,
        current_write_speed_bytes_per_sec: 0,
        files_created: files_created_count,
    };
    on_state(WipeState::Running(progress));

    let mut random_filler = FastRandomFiller::new();
    let zero_buffer = vec![0u8; WRITE_BUFFER_BYTES];
    let mut random_buffer = vec![0u8; WRITE_BUFFER_BYTES];

    let mut last_emitted_paused = false;

    'outer: while total_written < target_bytes && !control.is_cancelled() {
        wait_while_paused(control, progress, &on_state, &mut last_emitted_paused);
        if control.is_cancelled() {
            break;
        }

        let remaining_for_target = target_bytes - total_written;
        let chunk_target = remaining_for_target.min(config.chunk_size_bytes);
        if chunk_target == 0 {
            break;
        }

        let file_name = format!(
            "{}{}_{}{}",
            WipeConfig::DUMMY_FILE_PREFIX,
            files_created_count,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            WipeConfig::DUMMY_FILE_SUFFIX
        );
        let file_path: PathBuf = config.target.root_path.join(&file_name);

        let file = match OpenOptions::new().write(true).create_new(true).open(&file_path) {
            Ok(f) => f,
            Err(e) => {
                progress.bytes_written_total = total_written;
                on_state(WipeState::Failed {
                    message: format!("Failed to create dummy file: {e}"),
                    progress: Some(progress),
                });
                return;
            }
        };
        files_created_count += 1;

        let mut writer = io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
        let mut chunk_written: u64 = 0;
        let mut speed_window_start = std::time::Instant::now();
        let mut speed_window_bytes: u64 = 0;
        let mut current_speed: u64 = 0;

        while chunk_written < chunk_target && !control.is_cancelled() {
            wait_while_paused(control, progress, &on_state, &mut last_emitted_paused);
            if control.is_cancelled() {
                break;
            }

            // Re-check real free space periodically (not on every single
            // buffer write, to avoid a syscall per 4 MiB) so a volume
            // that's genuinely almost out of room -- independent of our
            // target math above, e.g. another process is simultaneously
            // consuming space -- doesn't drive us below the safety floor
            // or into a disk-full error loop.
            let live_free = target::free_space_bytes(&config.target.root_path);
            if live_free <= MIN_FREE_SPACE_FLOOR_BYTES {
                let _ = writer.flush(); // best-effort only -- discarding this file either way
                drop(writer);
                // If the partial file can't be removed for some reason,
                // that's not fatal to reporting accurate progress -- the
                // cleaner's purge-all pass will pick it up later same as
                // any other leftover dummy file.
                let _ = std::fs::remove_file(&file_path);
                progress.bytes_written_total = total_written; // don't count a file we just deleted
                progress.current_write_speed_bytes_per_sec = current_speed;
                progress.files_created = files_created_count;
                on_state(WipeState::Completed {
                    progress,
                    hit_storage_limit: true,
                });
                on_progress(progress);
                return;
            }

            let remaining_in_chunk = chunk_target - chunk_written;
            let write_size = remaining_in_chunk.min(WRITE_BUFFER_BYTES as u64) as usize;

            let buffer: &[u8] = match config.pattern {
                OverwritePattern::ZeroFill => &zero_buffer[..write_size],
                OverwritePattern::PseudoRandom => {
                    random_filler.fill(&mut random_buffer);
                    &random_buffer[..write_size]
                }
            };

            match writer.write_all(buffer) {
                Ok(()) => {}
                Err(e) => {
                    // Most commonly disk-full (ERROR_DISK_FULL) racing
                    // ahead of our free-space check above (another process
                    // wrote a large file in the same instant). Treat this
                    // exactly like hitting the floor: stop cleanly and
                    // report what was actually achieved, rather than
                    // surfacing a raw I/O error to the user.
                    //
                    // The flush()+remove_file() below discard this whole
                    // chunk file, including any bytes from earlier
                    // successful write_all() calls in this same chunk --
                    // so bytes_written_total reports only `total_written`
                    // (completed *prior* chunks, which are real, separate,
                    // still-on-disk files), not `total_written +
                    // chunk_written`. Counting chunk_written here would
                    // claim bytes as "written" in the same state update
                    // that deletes the file holding them.
                    let _ = writer.flush(); // best-effort only -- discarding this file either way
                    drop(writer);
                    let _ = std::fs::remove_file(&file_path);
                    progress.bytes_written_total = total_written;
                    progress.current_write_speed_bytes_per_sec = current_speed;
                    progress.files_created = files_created_count;
                    on_state(WipeState::Failed {
                        message: format!("Write error (likely disk full): {e}"),
                        progress: Some(progress),
                    });
                    on_progress(progress);
                    return;
                }
            }

            chunk_written += write_size as u64;
            speed_window_bytes += write_size as u64;

            let elapsed = speed_window_start.elapsed();
            if elapsed.as_millis() >= 500 {
                // refresh speed twice/sec -- smooth-looking MB/s readout
                // without recomputing on every 4MB buffer
                current_speed = (speed_window_bytes as f64 / elapsed.as_secs_f64()) as u64;
                speed_window_start = std::time::Instant::now();
                speed_window_bytes = 0;
            }

            progress.bytes_written_total = total_written + chunk_written;
            progress.current_write_speed_bytes_per_sec = current_speed;
            progress.files_created = files_created_count;
            on_progress(progress);
        }

        if let Err(e) = writer.flush() {
            // A flush failure here means we can't actually confirm this
            // chunk's buffered bytes made it to disk -- silently
            // proceeding to open the *next* chunk file would report false
            // progress on data that might not be durable. Treat this the
            // same as a write error: stop and report honestly rather than
            // continuing on an unverified assumption.
            drop(writer);
            progress.bytes_written_total = total_written; // don't count the unconfirmed chunk
            progress.files_created = files_created_count;
            on_state(WipeState::Failed {
                message: format!("Failed to flush data to disk: {e}"),
                progress: Some(progress),
            });
            on_progress(progress);
            return;
        }
        drop(writer);

        total_written += chunk_written;

        if control.is_cancelled() {
            break 'outer;
        }
    }

    progress.bytes_written_total = total_written;
    progress.files_created = files_created_count;

    if control.is_cancelled() {
        on_state(WipeState::Cancelled(progress));
    } else {
        on_state(WipeState::Completed {
            progress,
            hit_storage_limit: false,
        });
    }
}
