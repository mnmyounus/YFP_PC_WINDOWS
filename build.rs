// Runs at compile time to embed the .exe icon and Windows application
// manifest directly into the binary (via the `embed-resource`-style
// winres linkage below), so there's no separate .ico or .manifest file
// the release .exe depends on at runtime -- everything needed ships
// inside the single .exe, matching the "portable single-file app" goal.

fn main() {
    // Register our one custom cfg name up front so newer rustc's
    // unexpected_cfgs lint (stable since 1.80, warn-by-default) doesn't
    // flag it as unrecognized -- this directive is simply ignored by
    // older toolchains that predate --check-cfg, so it's safe on the
    // 1.78 toolchain this project pins as well as on anything newer.
    println!("cargo:rustc-check-cfg=cfg(yfp_has_window_icon)");

    // main.rs's include_bytes! of assets/icon_256.rgba AND tray.rs's
    // include_bytes! of assets/icon_32.rgba are both gated on this single
    // cfg flag, so it's only set when BOTH files are present -- setting it
    // based on just one existing would let the other's include_bytes! fail
    // to compile (a missing file at a hardcoded include_bytes! path is a
    // hard compile error, not a runtime-recoverable condition).
    let icon_256_path = std::path::Path::new("assets/icon_256.rgba");
    let icon_32_path = std::path::Path::new("assets/icon_32.rgba");
    if icon_256_path.exists() && icon_32_path.exists() {
        println!("cargo:rustc-cfg=yfp_has_window_icon");
    }
    // Re-run this build script if either RGBA asset appears/changes, so a
    // developer who runs scripts/generate-icon.ps1 after their first
    // `cargo build` gets the icon picked up on the next build without
    // needing a `cargo clean`.
    println!("cargo:rerun-if-changed=assets/icon_256.rgba");
    println!("cargo:rerun-if-changed=assets/icon_32.rgba");

    // Only meaningful when actually cross/native-compiling for Windows;
    // guards against breaking `cargo check`/tests on a non-Windows dev
    // machine, since resource embedding is a Windows-PE-specific step.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let mut res = winresource::WindowsResource::new();

    // icon.ico is generated (not committed as a binary) from the source
    // assets/icon.svg -- see scripts/generate-icon.ps1 for local dev, and
    // the CI workflow's "Generate icon.ico" step for release builds. A
    // fresh clone won't have it yet, so this degrades to an unadorned
    // default executable icon rather than failing the build outright,
    // which would otherwise block a contributor's very first `cargo build`
    // on a step that has nothing to do with the code they're changing.
    if std::path::Path::new("assets/icon.ico").exists() {
        res.set_icon("assets/icon.ico");
    } else {
        println!("cargo:warning=assets/icon.ico not found -- building with default icon. Run scripts/generate-icon.ps1 to generate it.");
    }

    res.set_manifest_file("assets/app.manifest");

    // Embed version/product metadata directly into the .exe's file
    // properties (visible in Windows Explorer's Properties -> Details tab),
    // so a downloaded YFP.exe identifies itself accurately to anyone who
    // right-clicks it before running it -- reasonable due diligence for a
    // security-adjacent tool that a user is trusting to write to their disk.
    res.set("ProductName", "YFP (Your Files Protector)");
    res.set("FileDescription", "YFP - overwrites free disk space to prevent deleted file recovery");
    res.set("LegalCopyright", "MNM Younus");
    res.set("OriginalFilename", "YFP.exe");

    if let Err(e) = res.compile() {
        eprintln!("cargo:warning=Failed to embed Windows resources: {e}");
    }
}
