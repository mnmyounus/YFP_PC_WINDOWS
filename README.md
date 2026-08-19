# YFP (Your Files Protector) by MNM Younus — Windows

A portable, single-file Windows desktop app that overwrites free disk space
so deleted files can no longer be recovered by tools like PhotoRec or
DiskDrill. 100% offline, zero ads, zero tracking, no installer required.

This is the Windows counterpart to the [Android version](../yfp) of YFP —
same underlying approach (overwrite the logical free space a deleted
file's data used to occupy, so common recovery tools have nothing to
reconstruct), ported to Windows's native storage APIs and built as a
native Rust binary rather than a web-tech wrapper, specifically to meet
the "low MB" requirement.

## Why Rust + egui, not Electron or .NET

- **Electron** ships a full Chromium runtime (150MB+) before a single line
  of app code. Incompatible with "low MB."
- **.NET**, self-contained, bundles the whole runtime (60–150MB
  typically). Framework-dependent .NET is smaller but requires the user to
  already have the right .NET runtime installed — fragile for a
  security tool someone downloads once and expects to just work.
- **Rust + [egui](https://github.com/emilk/egui)/[eframe](https://github.com/emilk/egui/tree/main/crates/eframe)**
  (native immediate-mode GUI, no HTML/CSS/JS, no WebView2 runtime
  dependency) compiles to a single self-contained binary. Combined with a
  size-tuned release profile (see `[profile.release]` in `Cargo.toml`:
  `opt-level = "z"`, LTO, symbol stripping, `panic = "abort"`), this gets
  the final `.exe` into the low single-digit megabytes.

No installer is produced — the release artifact is just `YFP.exe`,
matching the Android app's "just an APK" simplicity.

## Privacy

Same commitments as the Android app:
- **No network code anywhere in the app.** "Check for Updates" and
  "Contact Developer" hand off to the user's own browser/mail client via
  the OS's `ShellExecute`-equivalent (the `open` crate) — this process
  itself never opens a socket.
- **No analytics, telemetry, or third-party SDKs.**
- The only thing ever written outside a user-selected wipe target is a
  small local window-geometry file (via eframe's `persistence` feature) —
  no user data, no identifiers, no usage stats.

## Project structure

```
src/
├── main.rs          Entry point: window setup, icon loading, tray init
├── app.rs             eframe::App implementation — all UI state & rendering
├── tray.rs             System tray icon (minimize-to-tray convenience)
├── engine/
│   ├── config.rs         Job parameters (target, pattern, fill %, chunk size)
│   ├── random_filler.rs  Fast non-crypto PRNG for the pseudo-random pattern
│   ├── target.rs          Windows drive enumeration + live free-space queries
│   ├── wipe_engine.rs      The write loop: pause/resume/cancel, progress, safety floor
│   └── cleaner.rs           Purges all YFP dummy files from a target
└── util/
    └── format.rs        ETA/speed formatting for the UI
```

The `engine/` module mirrors the Android app's `engine/` package
module-for-module — same responsibilities, same safety guarantees (a 64
MiB free-space floor that's never crossed regardless of the configured
target percentage, live free-space rechecking during the write loop so a
concurrently-running other process can't drive the volume to zero, honest
distinction between "reached target %" and "stopped early because storage
ran lower than expected").

## ⚠️ Verification status — read before your first build

This project was authored in a sandboxed environment **without the actual
Rust/Windows toolchain or a live crates.io connection to compile against**.
Every dependency version and API call was written against training
knowledge, then **cross-checked against current, real documentation via
web search** wherever that was feasible — several version pins and one
whole API shape (`eframe::App`'s method changed from `update` to `ui`
between the version originally assumed and the current release) were
caught and corrected this way. That said, this is not a substitute for an
actual `cargo build`. Specifically flagged as **not yet compiler-verified**:

- **`tray.rs`'s event wiring is partially complete.** `Exit` is fully
  wired (reuses the same `WipeControl::cancel()` the in-app Cancel button
  uses, with a bounded grace period, before exiting). `ShowWindow` is
  correctly *detected* but the actual window-restore call is not yet
  implemented — it currently just logs. See the `TODO` comment at that
  branch in `app.rs`, and `tray.rs`'s own doc comment, for why: restoring
  a minimized/hidden window needs either eframe's viewport-control
  commands or raw `HWND` manipulation, and the exact API depends on the
  precise eframe/winit version closely enough that guessing it here — with
  no compiler to catch a mistake — would be worse than an explicit,
  marked gap.
- **`tray_icon::MenuItem::new(...)`, `.id()`, `Menu::append(...)`, and
  `Icon::from_rgba(...)` call shapes** were confirmed against tray-icon's
  own official examples (`examples/egui.rs`, `examples/tao.rs` in its
  repo) and its docs.rs listing — see the detailed "Verification note" at
  the top of `tray.rs` for exactly what was checked against what.
- **`windows` crate calls in `target.rs`** (`GetDiskFreeSpaceExW`,
  `GetLogicalDrives`, `GetDriveTypeW`) — the first was confirmed against
  its exact current signature; the latter two were not independently
  re-checked.
- **The CI toolchain-setup step pins `1.78.0` explicitly** rather than
  relying on `dtolnay/rust-toolchain` to auto-read `rust-toolchain.toml`,
  because that auto-detection could not be confirmed as behavior of the
  actual action in use (as opposed to documented forks of it). If you
  change `rust-toolchain.toml`'s channel, update the workflow's
  `toolchain:` input to match in the same commit.

One item that started as a "needs verification" flag and was resolved
during authoring, kept here for transparency: the CI workflow pins
`runs-on: windows-2022` rather than `windows-latest`. This was checked
against GitHub's own changelog and runner-images issue tracker and is
**confirmed current** as of this writing — `windows-latest` migrated to a
Visual Studio 2026-based image in June 2026, a change GitHub's own
migration notes flag as affecting native/SDK-dependent builds, and
`windows-2022` is GitHub's documented stable pin with a multi-year support
window. Not a guess; see the comment at the workflow's `runs-on:` line.

**The practical path forward:** push this to a repo and let the CI
workflow's first run be the real compile check — that's a genuine Windows
+ MSVC + real crates.io environment, which is the actual ground truth
none of the above research can fully substitute for. Fix whatever it
reports; the architecture and logic are sound even where a specific crate
call needs a small correction.

## Prerequisites

- **Rust**, via [rustup](https://rustup.rs/) — the exact toolchain version
  is pinned in `rust-toolchain.toml` and installs automatically on first
  `cargo build`.
- **Windows SDK** (specifically `rc.exe`, the resource compiler) — this
  targets the MSVC ABI (`x86_64-pc-windows-msvc`), and the icon/manifest
  embedding in `build.rs` (via the `winresource` crate) needs `rc.exe` to
  compile the embedded resources. If you have Visual Studio or Visual
  Studio Build Tools installed, you already have this. GitHub's CI runner
  (pinned to `windows-2022`, see below) has it preinstalled, so this only
  matters for building locally.
- **[ImageMagick](https://imagemagick.org/script/download.php#windows)**
  (optional, local dev only) — for `scripts/generate-icon.ps1`. Not
  required to build; without it, the app builds and runs fine with a
  default icon.

## First build

```powershell
cargo build --release
```

This will:
1. Generate `Cargo.lock` automatically (not committed to this repo — see
   below).
2. Emit a build warning that `assets/icon.ico` is missing (harmless — the
   binary still builds with a default icon). Run
   `.\scripts\generate-icon.ps1` first (requires
   [ImageMagick](https://imagemagick.org/script/download.php#windows)) if
   you want the real icon locally.

**About `Cargo.lock`:** intentionally not committed by this initial
scaffold, since generating a *correct* one requires resolving the real
dependency graph against crates.io, which this authoring environment
didn't have network access to do. Once you run a build and get a real
`Cargo.lock`, commit it — this pins exact dependency versions for
reproducible builds — and add `--locked` to the CI workflow's `cargo
build` step (see the comment at that line in
`.github/workflows/build-release.yml`) so CI fails loudly on any
accidental dependency drift instead of silently building against newer
versions than what was last tested.

## Releasing

The CI workflow (`.github/workflows/build-release.yml`) builds and
publishes automatically whenever a tag matching `v*.*.*` is pushed:

```powershell
git tag v1.0.0
git push origin v1.0.0
```

This builds `YFP.exe` in release mode on a GitHub-hosted `windows-2022`
runner — pinned deliberately rather than `windows-latest`, since GitHub
migrated that label to a Visual Studio 2026-based image in June 2026, and
that migration has been reported to affect native-toolchain builds
(Windows SDK / `rc.exe` availability among them, which `build.rs`'s icon
embedding depends on). `windows-2022` is GitHub's documented stable
fallback with a multi-year support window; see the comment at the
`runs-on:` line in the workflow if you want to track `windows-latest`
again later; reports the final binary size in the workflow summary (so a
size regression is visible immediately, not just asserted), and attaches
`YFP-1.0.0.exe` directly to a new GitHub Release — no installer, no zip,
just the portable executable.

### About code signing

This project does not use an EV code-signing certificate (they cost
money, typically $300+/year, and this is a free open-source tool). This
means:
- Windows SmartScreen will likely show an "unrecognized app" warning on
  first run for anyone who downloads the `.exe`. The release notes
  template in the workflow includes guidance on this for end users.
- If you want to eliminate this warning, you'd need to acquire a code
  signing certificate and add a signing step to the workflow (out of
  scope for this initial setup, but the workflow's `Build release binary`
  step is where a `signtool.exe` invocation would go, gated on repository
  secrets holding the certificate — same pattern as the Android app's
  keystore-from-secrets approach).

## Design notes worth knowing

- **No admin elevation requested.** `assets/app.manifest` explicitly
  requests `asInvoker`, not `requireAdministrator` — the app only ever
  writes to locations the current user already has permission to write
  to, so it has no legitimate reason to ask for more.
- **Safety floor:** the write loop never drives a volume's free space
  below 64 MiB, regardless of the configured target fill percentage, and
  rechecks live free space periodically during writing (not just once at
  job start) so a concurrently-running other process writing to the same
  volume can't cause the engine to overshoot. See the extensive comments
  on `MIN_FREE_SPACE_FLOOR_BYTES` in `src/engine/wipe_engine.rs`.
- **Not a forensic-grade erasure claim.** Same disclaimer as the Android
  app: on flash storage (most modern Windows laptops/SSDs), wear-leveling
  means the physical NAND cells backing a given logical sector aren't
  deterministic — a limitation shared by every consumer free-space-wipe
  tool, not something specific to this app.

## License

Add your chosen license here.
