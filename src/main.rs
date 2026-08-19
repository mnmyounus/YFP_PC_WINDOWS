// YFP (Your Files Protector) by MNM Younus -- Windows desktop entry point.
//
// windows_subsystem = "windows" suppresses the console window that would
// otherwise flash open behind the GUI on launch (the default "console"
// subsystem is meant for CLI tools, not GUI apps) -- this has zero effect
// on functionality, purely a "don't show an empty black window behind the
// real UI" fix.
#![windows_subsystem = "windows"]

mod app;
mod engine;
mod tray;
mod util;

fn main() -> eframe::Result<()> {
    let icon = load_window_icon();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([560.0, 640.0])
        .with_min_inner_size([440.0, 500.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        // persist_window lets egui remember window position/size across
        // launches via a small local config file -- this is the only
        // thing ever written outside the user-selected wipe target, and
        // it contains nothing but window geometry, matching "zero
        // telemetry" (there is no user data, usage stats, or identifiers
        // in it).
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "YFP - Your Files Protector",
        native_options,
        Box::new(|cc| {
            // System tray setup happens here, once, at startup -- see
            // tray.rs for why minimize-to-tray matters for this app
            // specifically (keeping a long wipe controllable without
            // requiring the main window to stay visible, the desktop
            // analog of the Android app's foreground-service notification).
            tray::init_tray();
            Ok(Box::new(app::YfpApp::new(cc)))
        }),
    )
}

/// Loads the embedded icon for use as the egui window/taskbar icon (distinct
/// from the .exe file icon build.rs embeds via the .ico resource -- that
/// one is what Explorer shows for the file itself; this one is what the
/// running window's titlebar/taskbar entry shows, and egui wants it as raw
/// RGBA pixels rather than a .ico resource reference).
///
/// Uses `include_bytes!` (a compile-time embed, baked directly into the
/// binary's data section) rather than reading the file at runtime --
/// runtime reads from a path under the source tree would only work when
/// launched via `cargo run` from a checkout, and would silently fail (or
/// worse, read a stale unrelated file) once the .exe is distributed
/// standalone to someone who doesn't have the source tree at all.
///
/// assets/icon_256.rgba is generated from assets/icon.svg (see
/// scripts/generate-icon.ps1 for local dev, and the CI workflow's
/// "Generate icon.ico" step for release builds, which produces this
/// alongside the .ico). If it hasn't been generated yet -- e.g. a very
/// first `cargo build` on a fresh clone before running the icon script --
/// this file legitimately won't exist, so we check for that at compile
/// time via a build-script-set cfg flag rather than letting a missing
/// asset hard-fail the whole build over something cosmetic.
fn load_window_icon() -> Option<egui::IconData> {
    const SIZE: u32 = 256;

    #[cfg(yfp_has_window_icon)]
    {
        let bytes: &[u8] = include_bytes!("../assets/icon_256.rgba");
        if bytes.len() == (SIZE * SIZE * 4) as usize {
            return Some(egui::IconData {
                rgba: bytes.to_vec(),
                width: SIZE,
                height: SIZE,
            });
        }
    }

    None
}
