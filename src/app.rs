//! The application's UI and top-level state machine. Uses egui's
//! immediate-mode model: `App::ui()` runs every frame, redrawing based on
//! current state -- there's no separate "view" layer to keep in sync by
//! hand, which sidesteps an entire category of binding-mismatch bugs the
//! Android app's XML-layout-plus-Kotlin approach has to guard against.
//!
//! Threading model: the write loop (`engine::wipe_engine::run`) blocks for
//! the job's full duration, so it always runs on a dedicated background
//! thread, never on the UI thread. That background thread communicates
//! back via an `mpsc::channel` of `UiUpdate` messages rather than a shared
//! mutex -- so the UI thread (which redraws on every frame, several times
//! a second) never risks blocking on a lock the writer thread might be
//! holding mid-syscall. `WipeControl`'s pause/cancel flags are the one
//! thing that *does* cross threads via shared atomics (see wipe_engine.rs)
//! since those are simple one-way signals, not state the UI reads back.

use crate::engine::{
    self, target, OverwritePattern, WipeConfig, WipeControl, WipeProgress, WipeState,
    WipeTargetInfo,
};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

const GITHUB_RELEASES_URL: &str = "https://github.com/mnmyounus/yfp-windows/releases";
const DEVELOPER_CONTACT_EMAIL: &str = "mnmyounus@proton.me";

/// Messages the background worker thread sends back to the UI thread.
enum UiUpdate {
    Progress(WipeProgress),
    State(WipeState),
}

/// Which screen/section is currently showing. Mirrors the Android app's
/// visibility-toggling between "target selection" and "progress" groups,
/// but expressed as an explicit enum here rather than several independent
/// boolean visibility flags, which is easier to reason about exhaustively
/// (the compiler flags a missing match arm if a new screen is ever added).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    TargetSelection,
    Progress,
}

pub struct YfpApp {
    drives: Vec<WipeTargetInfo>,
    selected_drive_index: usize,
    custom_folder: Option<PathBuf>,
    pattern: OverwritePattern,

    screen: Screen,
    state: WipeState,
    progress: Option<WipeProgress>,

    control: Option<WipeControl>,
    update_rx: Option<Receiver<UiUpdate>>,
    /// Some(...) while a folder-picker dialog is open on its own worker
    /// thread; None otherwise. Kept separate from update_rx (which carries
    /// wipe-job progress/state) since these are semantically distinct
    /// message streams with different lifetimes -- a picker's channel
    /// exists for one dialog interaction, not for a whole wipe job's
    /// duration.
    folder_picker_rx: Option<Receiver<Option<PathBuf>>>,

    show_start_confirm: bool,
    show_delete_confirm: bool,
    last_error_toast: Option<String>,
}

impl YfpApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let drives = target::list_drives();
        Self {
            drives,
            selected_drive_index: 0,
            custom_folder: None,
            pattern: OverwritePattern::PseudoRandom,
            screen: Screen::TargetSelection,
            state: WipeState::Idle,
            progress: None,
            control: None,
            update_rx: None,
            folder_picker_rx: None,
            show_start_confirm: false,
            show_delete_confirm: false,
            last_error_toast: None,
        }
    }

    /// Opens the native folder-picker dialog on its own dedicated thread
    /// rather than calling `rfd::FileDialog::pick_folder()`'s blocking sync
    /// API directly from inside `App::ui` (which runs on the winit event
    /// loop thread). rfd's own issue tracker documents sync dialogs
    /// freezing when called directly from a winit-driven event loop on
    /// some platforms/versions -- running the picker on a plain
    /// std::thread and receiving the result over a channel sidesteps that
    /// entirely, at the cost of one extra frame of latency before the
    /// result appears (negligible for a one-off folder pick).
    fn spawn_folder_picker(&mut self) {
        if self.folder_picker_rx.is_some() {
            return; // a picker is already open; ignore a duplicate click
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("yfp-folder-picker".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new().pick_folder();
                let _ = tx.send(picked);
            })
            .expect("failed to spawn folder picker thread");
        self.folder_picker_rx = Some(rx);
    }

    fn selected_target_root(&self) -> Option<(String, PathBuf)> {
        if let Some(folder) = &self.custom_folder {
            let label = folder.display().to_string();
            return Some((label, folder.clone()));
        }
        self.drives
            .get(self.selected_drive_index)
            .map(|d| (d.label.clone(), d.root_path.clone()))
    }

    fn selected_target_free_bytes(&self) -> u64 {
        if let Some(folder) = &self.custom_folder {
            return target::free_space_bytes(folder);
        }
        self.drives
            .get(self.selected_drive_index)
            .map(|d| d.free_bytes)
            .unwrap_or(0)
    }

    fn drain_updates(&mut self) {
        let Some(rx) = &self.update_rx else { return };
        // Drain everything currently queued rather than just one message
        // per frame -- a fast-writing drive can produce progress updates
        // faster than 60fps, and we only care about the *latest* state for
        // rendering, so catching up in one pass avoids the UI visibly
        // lagging behind the real write progress.
        let mut latest_progress = None;
        let mut latest_state = None;
        while let Ok(update) = rx.try_recv() {
            match update {
                UiUpdate::Progress(p) => latest_progress = Some(p),
                UiUpdate::State(s) => latest_state = Some(s),
            }
        }
        if let Some(p) = latest_progress {
            self.progress = Some(p);
        }
        if let Some(s) = latest_state {
            self.apply_state(s);
        }
    }

    /// Polls the folder-picker worker thread's channel (see
    /// spawn_folder_picker) for a completed pick. A picker dialog only
    /// ever sends one message before its thread exits, so unlike
    /// drain_updates there's no "drain everything, keep only the latest"
    /// concern here -- the first (only) message is the result.
    fn drain_folder_picker(&mut self) {
        let Some(rx) = &self.folder_picker_rx else { return };
        match rx.try_recv() {
            Ok(picked) => {
                if let Some(folder) = picked {
                    self.custom_folder = Some(folder);
                }
                self.folder_picker_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Dialog still open; nothing to do this frame.
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Worker thread ended without sending (shouldn't normally
                // happen given spawn_folder_picker always sends before
                // returning, but treat it as "picker closed, no
                // selection" rather than leaving folder_picker_rx stuck
                // Some forever, which would permanently disable the
                // button via picker_busy).
                self.folder_picker_rx = None;
            }
        }
    }

    fn apply_state(&mut self, state: WipeState) {
        match &state {
            WipeState::Completed { .. } | WipeState::Cancelled(_) | WipeState::Failed { .. } => {
                self.screen = Screen::Progress; // stay on progress screen so
                // the user can see the result and still reach Delete,
                // rather than snapping back to target selection under them.
            }
            WipeState::Failed { message, .. } => {
                self.last_error_toast = Some(message.clone());
            }
            _ => {}
        }
        self.state = state;
    }

    fn start_wipe(&mut self) {
        let Some((label, root)) = self.selected_target_root() else {
            return;
        };

        let free_bytes = self.selected_target_free_bytes();
        let target_info = WipeTargetInfo {
            label,
            root_path: root.clone(),
            total_bytes: 0, // not needed by the engine itself, only free_bytes is used live
            free_bytes,
            is_removable: false,
        };

        let config = WipeConfig {
            target: target_info,
            pattern: self.pattern,
            target_fill_percent: WipeConfig::DEFAULT_FILL_PERCENT,
            chunk_size_bytes: WipeConfig::DEFAULT_CHUNK_MB * 1024 * 1024,
        };

        let control = WipeControl::new();
        let (tx, rx): (Sender<UiUpdate>, Receiver<UiUpdate>) = std::sync::mpsc::channel();

        let resume_from_bytes = engine::cleaner::sum_existing_dummy_file_bytes(&root);

        let control_for_thread = control.clone();
        let tx_progress: Sender<UiUpdate> = tx.clone();
        let tx_state: Sender<UiUpdate> = tx;

        std::thread::Builder::new()
            .name("yfp-wipe-worker".into())
            .spawn(move || {
                engine::wipe_engine::run(
                    &config,
                    &control_for_thread,
                    resume_from_bytes,
                    move |progress| {
                        let _ = tx_progress.send(UiUpdate::Progress(progress));
                    },
                    move |state| {
                        let _ = tx_state.send(UiUpdate::State(state));
                    },
                );
            })
            .expect("failed to spawn wipe worker thread");

        self.control = Some(control);
        self.update_rx = Some(rx);
        self.screen = Screen::Progress;
        self.state = WipeState::Running(WipeProgress {
            bytes_written_total: resume_from_bytes,
            bytes_target: 0,
            current_write_speed_bytes_per_sec: 0,
            files_created: 0,
        });
    }

    fn cancel_and_delete(&mut self) {
        if let Some(control) = &self.control {
            control.cancel();
        }
        // Purge runs on a short-lived background thread too (not the UI
        // thread) since deleting a handful of multi-GB files can itself
        // take a moment on a slow drive, and we don't want the window to
        // freeze while that happens. The engine's write loop, once it
        // observes cancellation, closes and flushes its own current file
        // before returning -- so by the time this purge thread actually
        // runs, that file is no longer open for write on the OS side.
        let Some((_, root)) = self.selected_target_root() else {
            self.reset_to_idle();
            return;
        };
        let (tx, rx): (Sender<UiUpdate>, Receiver<UiUpdate>) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("yfp-cleanup-worker".into())
            .spawn(move || {
                // Give the writer thread a brief moment to observe the
                // cancel flag and close its file handle before we try to
                // delete files in the same directory -- this is a small,
                // deliberate grace period, not a substitute for the
                // writer's own flush-before-close (which still happens
                // regardless of timing); it just reduces the odds of a
                // transient "file in use" error on the very file the
                // writer was mid-write on when cancel was requested.
                std::thread::sleep(std::time::Duration::from_millis(150));
                let _ = engine::cleaner::purge_all(&root);
                let _ = tx.send(UiUpdate::State(WipeState::Idle));
            })
            .expect("failed to spawn cleanup worker thread");
        self.update_rx = Some(rx);
    }

    fn reset_to_idle(&mut self) {
        self.control = None;
        self.update_rx = None;
        self.state = WipeState::Idle;
        self.progress = None;
        self.screen = Screen::TargetSelection;
        self.drives = target::list_drives(); // refresh free-space figures
        self.custom_folder = None;
    }

    fn is_terminal_state(&self) -> bool {
        matches!(
            self.state,
            WipeState::Completed { .. } | WipeState::Cancelled(_) | WipeState::Failed { .. }
        )
    }
}

impl eframe::App for YfpApp {
    // NOTE ON API VERSION: eframe 0.36's App trait method is `ui`, taking
    // `&mut egui::Ui` directly -- not the older `update(&mut self, ctx:
    // &egui::Context, ...)` shape from earlier egui/eframe generations.
    // This was verified against the current docs.rs listing for eframe
    // rather than assumed, since dependency versions and their APIs shift
    // over time. `ui.ctx()` recovers the underlying `&egui::Context` for
    // the two calls below (request_repaint_after, and the confirm-dialog
    // egui::Window::show calls) that specifically need a Context rather
    // than a Ui.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_updates();
        self.drain_folder_picker();

        // Handle any tray icon menu click since the last frame. See
        // tray.rs's module doc for what's verified vs. still a known gap
        // here: Exit needs no window-handle API (just a process exit) so
        // it's fully wired; ShowWindow is detected correctly but the
        // actual window-restore call is the one deliberately-unfinished
        // piece (see tray.rs's doc comment for why).
        match crate::tray::poll_tray_events() {
            Some(crate::tray::TrayCommand::Exit) => {
                // If a wipe is actively running or paused, signal a clean
                // cancel first -- same WipeControl::cancel() the in-app
                // Cancel button uses -- so the engine flushes and closes
                // its current dummy file properly instead of a bare
                // process::exit killing the writer thread mid-write with
                // no chance to flush. This is a best-effort, bounded wait:
                // if the writer thread doesn't finish unwinding within the
                // timeout (e.g. a very slow drive mid-flush), this still
                // exits rather than hanging the whole app on a tray click
                // meant to feel immediate. Either way, any file left
                // partially written is not data-loss-unsafe: the resume
                // logic in start_wipe (sum_existing_dummy_file_bytes)
                // treats a leftover partial file as already-progressed
                // bytes on next launch, and cancel_and_delete's purge can
                // clean it up regardless of how the process ended.
                if let Some(control) = &self.control {
                    control.cancel();
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                std::process::exit(0);
            }
            Some(crate::tray::TrayCommand::ShowWindow) => {
                // TODO: actually restore/focus the window here once the
                // eframe/winit window-handle API is confirmed against a
                // real build -- see tray.rs's doc comment. Bringing the
                // egui context to the front of the next repaint is a
                // no-op today, so for now this at least doesn't silently
                // eat the click; a future build can complete this branch.
                eprintln!("Tray: Show YFP requested (window-restore not yet wired -- see tray.rs)");
            }
            None => {}
        }

        let ctx = ui.ctx().clone();

        // Keep redrawing while a job is active so progress numbers animate
        // smoothly instead of only updating when the OS happens to send an
        // input event -- egui is immediate-mode and otherwise only repaints
        // on user interaction. Also keep polling while a folder-picker
        // thread is outstanding, so its result (see drain_folder_picker)
        // is picked up promptly once the user closes the native dialog,
        // rather than waiting for some unrelated input to trigger the
        // next frame. And keep a slow idle poll running at all other
        // times too (once/second is enough) specifically so a tray-icon
        // Exit/Show click is noticed within about a second even when the
        // app is doing nothing else -- without this, the tray click above
        // would only be detected the next time some unrelated input (e.g.
        // mouse movement over the window) happened to trigger a repaint,
        // which could be a long and confusing delay for something meant
        // to feel instantaneous.
        if matches!(self.state, WipeState::Running(_) | WipeState::Paused(_))
            || self.folder_picker_rx.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }

        // Verified against the current docs.rs/eframe usage example:
        // CentralPanel::default().show(ui, |ui| {...}) is the correct call
        // when already inside App::ui's Ui parameter (not .show_inside,
        // which does something different -- confirmed by cross-checking
        // the official example rather than assumed).
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(12.0);
            ui.heading("YFP");
            ui.label("Your Files Protector — 100% offline · No ads · No tracking");
            ui.add_space(16.0);

            match self.screen {
                Screen::TargetSelection => self.draw_target_selection(ui),
                Screen::Progress => self.draw_progress(ui),
            }

            ui.add_space(24.0);
            ui.separator();
            ui.add_space(8.0);
            self.draw_utility_links(ui);
        });

        self.draw_confirm_dialogs(&ctx);
    }
}

impl YfpApp {
    fn draw_target_selection(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Select target drive or folder").strong());
        ui.add_space(6.0);

        for (i, drive) in self.drives.iter().enumerate() {
            let selected = self.custom_folder.is_none() && self.selected_drive_index == i;
            let text = format!(
                "{} — {} free of {}",
                drive.label,
                target::format_bytes(drive.free_bytes),
                target::format_bytes(drive.total_bytes)
            );
            if ui.selectable_label(selected, text).clicked() {
                self.selected_drive_index = i;
                self.custom_folder = None;
            }
        }

        ui.add_space(8.0);
        let custom_label = match &self.custom_folder {
            Some(p) => format!("Custom folder: {}", p.display()),
            None => "Choose a custom folder instead…".to_string(),
        };
        let picker_busy = self.folder_picker_rx.is_some();
        if ui
            .add_enabled(!picker_busy, egui::Button::new(custom_label))
            .clicked()
        {
            self.spawn_folder_picker();
        }

        ui.add_space(20.0);
        ui.label(egui::RichText::new("Overwrite pattern").strong());
        ui.radio_value(
            &mut self.pattern,
            OverwritePattern::PseudoRandom,
            "Pseudo-Random — non-repeating pattern, slightly slower, thorough",
        );
        ui.radio_value(
            &mut self.pattern,
            OverwritePattern::ZeroFill,
            "Zero-Fill — all-zero pattern, fastest",
        );

        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(
                "YFP overwrites free space to prevent common recovery tools from restoring \
                 deleted files. It does not make forensic-grade claims (e.g. DoD 5220.22-M) — \
                 on flash storage (SSDs), wear-leveling means no consumer tool can guarantee \
                 every physical cell is touched.",
            )
            .small()
            .weak(),
        );

        ui.add_space(20.0);
        let has_target = self.selected_target_root().is_some();
        if ui
            .add_enabled(has_target, egui::Button::new("Start Wipe").min_size(egui::vec2(200.0, 36.0)))
            .clicked()
        {
            self.show_start_confirm = true;
        }
    }

    fn draw_progress(&mut self, ui: &mut egui::Ui) {
        let status_text = match &self.state {
            WipeState::Idle => "Ready".to_string(),
            WipeState::Running(_) => "Overwriting free space…".to_string(),
            WipeState::Paused(_) => "Paused".to_string(),
            WipeState::Completed {
                hit_storage_limit, ..
            } => {
                if *hit_storage_limit {
                    "Wipe stopped — storage ran lower than expected. Files written so far are \
                     safe to delete below."
                        .to_string()
                } else {
                    "Wipe complete".to_string()
                }
            }
            WipeState::Cancelled(_) => "Cancelled".to_string(),
            WipeState::Failed { message, .. } => format!("Failed: {message}"),
        };
        ui.label(egui::RichText::new(status_text).strong());
        ui.add_space(12.0);

        if let Some(progress) = self.progress {
            let pct = progress.percent_complete();
            ui.add(egui::ProgressBar::new(pct as f32 / 100.0).text(format!("{pct}%")));
            ui.add_space(8.0);
            ui.label(format!(
                "{} / {} written",
                target::format_bytes(progress.bytes_written_total),
                target::format_bytes(progress.bytes_target)
            ));

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("SPEED").small().weak());
                    ui.label(
                        egui::RichText::new(crate::util::format::format_speed(
                            progress.current_write_speed_bytes_per_sec,
                        ))
                        .strong(),
                    );
                });
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("TIME REMAINING").small().weak());
                    let eta_text = match progress.estimated_seconds_remaining() {
                        Some(s) => crate::util::format::format_eta(s),
                        None => "Calculating…".to_string(),
                    };
                    ui.label(egui::RichText::new(eta_text).strong());
                });
            });
        }

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            match &self.state {
                WipeState::Running(_) => {
                    if ui.button("Pause").clicked() {
                        if let Some(control) = &self.control {
                            control.pause();
                        }
                    }
                }
                WipeState::Paused(_) => {
                    if ui.button("Resume").clicked() {
                        if let Some(control) = &self.control {
                            control.resume();
                        }
                    }
                }
                _ => {}
            }

            if matches!(self.state, WipeState::Running(_) | WipeState::Paused(_))
                || self.is_terminal_state()
            {
                if ui
                    .button(egui::RichText::new("Cancel && Delete Dummy Files").color(egui::Color32::from_rgb(192, 57, 43)))
                    .clicked()
                {
                    self.show_delete_confirm = true;
                }
            }
        });

        if self.is_terminal_state() && self.update_rx.is_some() {
            // A terminal state's channel receiver may still be attached if
            // the user hasn't triggered a cleanup/reset action yet -- no
            // special handling needed here, just noting this is
            // intentional rather than a leak: reset_to_idle() (called from
            // the delete-confirm flow) is what tears it down.
        }
    }

    fn draw_utility_links(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Check for Updates").clicked() {
                let _ = open::that(GITHUB_RELEASES_URL);
            }
            if ui.button("Contact Developer").clicked() {
                let _ = open::that(format!(
                    "mailto:{DEVELOPER_CONTACT_EMAIL}?subject=YFP%20Support%20Request"
                ));
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new("YFP by MNM Younus").small().weak());
    }

    fn draw_confirm_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_start_confirm {
            egui::Window::new("Start overwrite?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    let target_label = self
                        .selected_target_root()
                        .map(|(label, _)| label)
                        .unwrap_or_default();
                    ui.label(format!(
                        "YFP will fill up to {}% of free space on {} with dummy files, then \
                         delete them. This can take a while depending on drive speed and size. \
                         You can pause or cancel at any time.",
                        WipeConfig::DEFAULT_FILL_PERCENT,
                        target_label
                    ));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Start").clicked() {
                            self.show_start_confirm = false;
                            self.start_wipe();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_start_confirm = false;
                        }
                    });
                });
        }

        if self.show_delete_confirm {
            egui::Window::new("Delete dummy files?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(
                        "This will stop the current wipe (if running) and immediately delete \
                         all dummy files YFP has created, freeing the space back up.",
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            self.show_delete_confirm = false;
                            self.cancel_and_delete();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_delete_confirm = false;
                        }
                    });
                });
        }

        if let Some(err) = self.last_error_toast.clone() {
            egui::Window::new("Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(err);
                    if ui.button("OK").clicked() {
                        self.last_error_toast = None;
                    }
                });
        }
    }
}
