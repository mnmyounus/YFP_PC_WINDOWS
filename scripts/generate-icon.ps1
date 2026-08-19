# Regenerates assets/icon.ico from assets/icon.svg for local development.
#
# Requires ImageMagick (https://imagemagick.org/script/download.php#windows)
# on PATH as `magick`. CI does not use this script -- the release workflow
# runs the equivalent commands itself (see .github/workflows/build-release.yml,
# "Generate icon.ico" step) since GitHub's windows-latest runners already
# have ImageMagick preinstalled. This script exists purely so a local
# `cargo build` picks up a real icon instead of build.rs's fallback warning.

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$svgPath = Join-Path $repoRoot "assets\icon.svg"
$icoPath = Join-Path $repoRoot "assets\icon.ico"
$rgba256Path = Join-Path $repoRoot "assets\icon_256.rgba"
$rgba32Path = Join-Path $repoRoot "assets\icon_32.rgba"

if (-not (Get-Command magick -ErrorAction SilentlyContinue)) {
    Write-Error "ImageMagick ('magick' command) not found on PATH. Install it from https://imagemagick.org/script/download.php#windows, or Windows will just use a default icon -- the app still builds and runs fine without this."
    exit 1
}

Write-Host "Generating $icoPath from $svgPath ..."

# A multi-resolution .ico embeds several sizes in one file so Windows can
# pick the sharpest one for each context (taskbar, Alt-Tab, Explorer
# thumbnail, shortcut properties, etc.) instead of upscaling a single
# low-res source.
& magick $svgPath -background none -define icon:auto-resize=256,128,64,48,32,16 $icoPath

if ($LASTEXITCODE -ne 0) {
    Write-Error "ImageMagick failed to generate the icon."
    exit 1
}

Write-Host "Generating $rgba256Path (raw RGBA8, 256x256) for the in-app window icon ..."

# main.rs embeds this directly (via include_bytes!, gated on the
# yfp_has_window_icon cfg build.rs sets when this file is present) for the
# egui window/taskbar icon -- a separate asset from icon.ico because egui
# wants raw pixels, not a Windows .ico resource reference.
& magick $svgPath -background none -resize 256x256 -depth 8 "RGBA:$rgba256Path"

if ($LASTEXITCODE -ne 0) {
    Write-Error "ImageMagick failed to generate the 256x256 RGBA window icon."
    exit 1
}

Write-Host "Generating $rgba32Path (raw RGBA8, 32x32) for the system tray icon ..."

# tray.rs embeds this one, at tray-icon size directly -- tray_icon::Icon
# wants pre-sized pixel data rather than resizing internally.
& magick $svgPath -background none -resize 32x32 -depth 8 "RGBA:$rgba32Path"

if ($LASTEXITCODE -ne 0) {
    Write-Error "ImageMagick failed to generate the 32x32 RGBA tray icon."
    exit 1
}

Write-Host "Done: $icoPath"
Write-Host "Done: $rgba256Path"
Write-Host "Done: $rgba32Path"
