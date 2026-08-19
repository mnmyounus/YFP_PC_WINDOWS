//! System tray icon integration.
//!
//! This is the desktop analog of the Android app's foreground service +
//! persistent notification: the goal is the same -- a long-running wipe
//! job should stay running and stay controllable even if the user isn't
//! looking at the main window -- but the *mechanism* is necessarily
//! different, since Windows has no equivalent of Android's "foreground
//! service" concept. A normal Windows process just keeps running as long
//! as its window exists (minimized or not), so the tray icon's actual job
//! here is narrower than Android's: it's a convenience for getting back to
//! a minimized window, not something the wipe job's continued execution
//! depends on.
//!
//! ## Verification note
//! The event-dispatch pattern below (poll `MenuEvent::receiver()` once per
//! frame, compare `event.id` against a stored `MenuItem::id()`) is copied
//! from tray-icon's own official examples, not guessed:
//!   - `examples/egui.rs` in the tray-icon repo polls
//!     `TrayIconEvent::receiver().try_recv()` from inside its eframe
//!     `App` impl's update method (that example's own method is named
//!     `update`, likely targeting an eframe version older than the 0.36
//!     this project pins, where the method is `App::ui` -- see main.rs's
//!     `App::ui` for this project's actual current call site). The
//!     naming difference doesn't matter for what's being verified here:
//!     what matters is that polling `try_recv()` once per egui frame,
//!     from inside whichever per-frame method eframe calls, is the
//!     maintainer-demonstrated integration path for eframe+tray-icon.
//!   - `examples/tao.rs` in the tray-icon repo shows the actual comparison
//!     idiom: `if event.id == quit_i.id() { ... }` -- confirming
//!     `MenuEvent` has a public `.id` field and `MenuItem` has an `.id()`
//!     method, which is what `poll_tray_events` below relies on.
//!
//! ## What's still NOT wired up
//! Actually showing/restoring a minimized or hidden window from outside
//! eframe's own event loop requires either eframe's viewport-control
//! commands or raw `HWND` manipulation via
//! `windows::Win32::UI::WindowsAndMessaging::ShowWindow` against a window
//! handle obtained through `eframe::Frame`/`egui::Context`'s window-handle
//! API. A real user's account of doing this (egui GitHub discussion
//! #737) describes it as a multi-day undertaking even with a working
//! compiler in hand, and the exact API surface needed
//! (`cc.window_handle()`, `egui::ViewportCommand::Visible`, or similar)
//! depends on the precise eframe/winit version in play closely enough
//! that guessing it here -- with no compiler to catch a mistake -- would
//! be worse than leaving it as an explicit, marked gap. `poll_tray_events`
//! below correctly detects and returns which command the user invoked;
//! `app.rs` currently only acts on `TrayCommand::Exit` (which needs no
//! window-handle API, just a process exit) and logs `ShowWindow` without
//! yet acting on it. Wiring the actual show/restore call is the one
//! concrete follow-up this module still needs, ideally done against a
//! real build rather than guessed further here.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// What the user asked for via the tray icon's context menu. Returned by
/// `poll_tray_events` so `app.rs` can act on it without this module
/// needing to know anything about egui/eframe itself -- keeps the
/// crate-version-sensitive tray-icon glue isolated to this one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    Exit,
}

struct TrayState {
    _icon: TrayIcon, // held only to keep the icon alive; see TRAY_STATE doc below
    show_item_id: MenuId,
    exit_item_id: MenuId,
}

/// Kept alive for the process lifetime via a thread-local static --
/// tray-icon's documented pattern is that the TrayIcon must not be
/// dropped for the icon to remain visible, and it does not implement
/// Send, so it can't simply live inside YfpApp's state (which eframe may
/// touch from contexts tray-icon doesn't guarantee are the same thread
/// that created it). A thread-local matches tray-icon's own examples for
/// this exact situation. The menu item ids are stored alongside the icon
/// itself (rather than as separate statics) since they're only ever
/// needed together, at poll time.
thread_local! {
    static TRAY_STATE: std::cell::RefCell<Option<TrayState>> = const { std::cell::RefCell::new(None) };
}

pub fn init_tray() {
    let icon = load_tray_icon();

    let menu = Menu::new();
    let show_item = MenuItem::new("Show YFP", true, None);
    let exit_item = MenuItem::new("Exit", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&exit_item);

    // Menu::append (confirmed via docs.rs: "Add a menu item to the end of
    // this menu", distinct from the batch append_items(&[...]) tao.rs's
    // example uses) takes its argument by reference, so show_item/
    // exit_item are NOT consumed here -- unlike tao.rs's example, where
    // append_items does take ownership of the slice's contents. That
    // means, unlike a first draft of this comment claimed, capturing the
    // ids below doesn't strictly have to happen before the append() calls
    // above -- it's written in this order simply because it reads
    // naturally, not because the borrow checker requires it.
    let show_item_id = show_item.id().clone();
    let exit_item_id = exit_item.id().clone();

    let builder = TrayIconBuilder::new()
        .with_tooltip("YFP - Your Files Protector")
        .with_menu(Box::new(menu));

    let builder = match icon {
        Some(icon) => builder.with_icon(icon),
        None => builder,
    };

    match builder.build() {
        Ok(tray) => {
            TRAY_STATE.with(|cell| {
                *cell.borrow_mut() = Some(TrayState {
                    _icon: tray,
                    show_item_id,
                    exit_item_id,
                })
            });
        }
        Err(e) => {
            // Non-fatal: a tray icon failing to create (e.g. explorer.exe
            // not yet ready at very early startup) shouldn't prevent the
            // main window from opening -- the app is still fully usable
            // without it, just without the minimize-to-tray convenience.
            eprintln!("Failed to create tray icon: {e}");
        }
    }
}

/// Call once per frame (see app.rs's `App::ui`) to check whether the user
/// clicked a tray menu item since the last poll. Non-blocking: returns
/// immediately with `None` if nothing happened, matching tray-icon's own
/// `examples/egui.rs` pattern of polling `try_recv()` from inside the
/// eframe update loop rather than running a separate dedicated event loop.
pub fn poll_tray_events() -> Option<TrayCommand> {
    let event = MenuEvent::receiver().try_recv().ok()?;

    TRAY_STATE.with(|cell| {
        let state = cell.borrow();
        let state = state.as_ref()?;
        if event.id == state.show_item_id {
            Some(TrayCommand::ShowWindow)
        } else if event.id == state.exit_item_id {
            Some(TrayCommand::Exit)
        } else {
            None
        }
    })
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    const SIZE: u32 = 32; // tray icons render small; no need to decode the full 256px asset

    #[cfg(yfp_has_window_icon)]
    {
        // Reuses the same generated-icon pipeline as the window icon (see
        // main.rs's load_window_icon) rather than a separate asset source
        // -- tray_icon::Icon::from_rgba wants raw RGBA8 + dimensions, same
        // shape of input. The build pipeline (generate-icon.ps1 / CI)
        // produces this pre-sized 32x32 variant alongside the 256x256 one,
        // since tray-icon's API takes pre-sized pixel data directly rather
        // than resizing internally.
        let bytes: &[u8] = include_bytes!("../assets/icon_32.rgba");
        if bytes.len() == (SIZE * SIZE * 4) as usize {
            return tray_icon::Icon::from_rgba(bytes.to_vec(), SIZE, SIZE).ok();
        }
    }

    None
}
