# Contributing

## Architecture

- `rust/` -- the `aerial_core` crate, built as a `cdylib`:
  - `manifest.rs` -- fetches/parses `https://sylvan.apple.com/Aerials/2x/entries.json`,
    falling back to a bundled snapshot on failure.
  - `cache.rs` -- download/LRU-cache/eviction logic, independent of Qt (unit
    tested with `wiremock` + `tempfile`).
  - `qml.rs` -- the `#[cxx_qt::bridge]` exposing `AerialManifest` (a
    `QAbstractListModel`) and `AerialCache` as QML types.
- `plugin/` -- a small, hand-written `QQmlEngineExtensionPlugin` built directly
  by CMake. See "Known issue" below for why this exists instead of using
  cxx-qt-build's own generated dynamic plugin entry point.
- `package/` -- the KPackage (`Plasma/Wallpaper`) itself: `metadata.json`,
  `contents/config/main.xml` (KConfigXT), `contents/ui/main.qml`,
  `contents/ui/config.qml`.
- `tests/qml_bridge_test/` -- a standalone Qt Quick app that loads the QML
  module in isolation, to validate the Rust<->QML bridge without any
  Plasma-specific plumbing.
- `CMakeLists.txt` -- wires the Rust crate (via Corrosion/cxx-qt-cmake), the
  plugin, and the KPackage (`plasma_install_package`) together.
- `PKGBUILD` -- AUR packaging.

## Known issue: why there's a hand-written plugin shim

`cxx-qt-build` 0.10.0 generates its own dynamic QML plugin entry point
(`qt_plugin_instance`), but compiles it into the same static archive as the
rest of the Rust crate. Rust's default `cdylib` link flags on Linux
(`--exclude-libs=ALL`) demote every symbol contributed by a linked static
archive to local ELF binding -- including that entry point -- so Qt can never
`dlsym()` it, even though the symbol is present in the binary. This is
reproducible with KDAB's own `qml_minimal_plugin` example, not something
specific to this project. Neither `--export-dynamic`/
`--export-dynamic-symbol` nor post-hoc `objcopy --globalize-symbol` can undo
it once `--exclude-libs=ALL` has been applied.

The actual QML type registration for `AerialManifest`/`AerialCache` is
unaffected by this -- it happens via static initializers that run
automatically as soon as `libaerial_core.so` is loaded by anything. So
`plugin/aerial_plugin.{h,cpp}` provides a minimal, plain-CMake/AUTOMOC-built
`QQmlEngineExtensionPlugin` that simply links against `aerial_core-shared`,
sidestepping the export-visibility problem entirely. See
`cmake/AerialQmlPlugin.cmake` for the full explanation and wiring.

## Known issue: Apple's TLS chain and stale catalog

`sylvan.apple.com` (the Aerial CDN) is signed by "Apple Root CA", a
long-standing Apple root that predates most public CA programs and was never
submitted to Mozilla's/most Linux distros' root stores (it's mainly used for
Apple's internal services). Neither the system trust store nor rustls's
bundled `webpki-roots` trust it, so plain TLS verification fails with
`UnknownIssuer` even though the connection is genuinely to Apple. `rust/http.rs`
pins the specific intermediate CA that signs this endpoint (captured directly
from Apple's own TLS handshake, `rust/assets/apple-server-authentication-ca.pem`)
as an additional trusted root for this client only -- this keeps verification
strict (a MITM'd/spoofed cert still fails) rather than disabling verification.

Separately, Apple's `entries.json` catalog lists more entries than currently
have live video files -- in testing, only 1 of 13 listed assets had a working
URL. This isn't something we can fix (it's Apple's content catalog drifting
out of sync with the manifest), so `AerialCache` emits `downloadFailed` for
any dead URL and `main.qml` skips to the next playlist entry rather than
getting stuck on a black screen.

## Known issue: HDR video playback can break panel screen assignment

Observed on a multi-monitor system: playing a 4K HDR (Dolby Vision) Aerial
video triggered KWin/Qt to renegotiate the video surface format, which in
turn caused a panel's `screen` property to reset to `-1` (unassigned to any
screen) -- making it invisible until fixed. This is a Wayland/KWin-level
display-mode-switch issue, not something in this plugin's own code, but it's
this plugin's HDR video playback that triggers it. Confirmed to be a screen
assignment issue, not a frozen/blocked process: plasmashell's D-Bus interface
stayed fully responsive throughout using `qdbus6 org.kde.plasmashell
/PlasmaShell org.kde.PlasmaShell.immutable` to probe latency.

If a panel disappears, recover it without restarting plasmashell:

```sh
qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript '
var p = panels()[0];
print("current screen: " + p.screen);
p.screen = 0;  // set to whichever screen index the panel belongs on
'
```

The default `Quality: SDR1080` setting avoids HDR entirely and sidesteps
this; it only reproduces when a config is explicitly switched to an HDR
quality tier.

## Building

Requires: `extra-cmake-modules`, `cmake`, `rust`, `corrosion`, Qt6
(`qt6-base`, `qt6-declarative`, `qt6-multimedia`).

```sh
cmake -B build -S .
cmake --build build
```

This builds the Rust crate, the QML plugin, the standalone bridge test, and
stages the KPackage for `plasma_install_package`.

## Testing

Follow these steps in order -- each validates a different layer, so a failure
narrows down where to look.

### 1. Standalone Rust<->QML bridge test

Before touching Plasma at all, verify the Rust<->QML bridge in isolation:

```sh
cmake --build build --target aerial_qml_bridge_test
QML2_IMPORT_PATH=build/qml_modules ./build/tests/qml_bridge_test/aerial_qml_bridge_test
```

It fetches the Aerial manifest (falling back to the bundled snapshot if
offline), logs the entry count and first entry id, exercises
`AerialCache.ensureDownloaded`, and exits 0. If it hangs for 15 seconds and
exits with status 2, the manifest never loaded -- check network access and/or
that `rust/assets/entries.fallback.json` is a valid fallback.

### 2. Install the KPackage

```sh
kpackagetool6 --type Plasma/Wallpaper -i package/
```

(Re-installing after changes: `kpackagetool6 --type Plasma/Wallpaper -u package/`.)

Also install the QML plugin somewhere Qt can find it -- either
`sudo cmake --install build` with a normal `/usr` prefix, or
`sudo cp -r build/qml_modules/org "$(qmake6 -query QT_INSTALL_QML)"`.
Both write into a system path, hence `sudo`.

Then, in System Settings -> Appearance -> Wallpaper, select "Aerial" as the
wallpaper type on a single monitor first. Verify:
- The config dialog shows quality/shuffle/cache/Wi-Fi-only/location controls.
- Video starts playing within a few seconds (first video needs to download).
- Crossfade between videos happens without a black flash.

### 3. Multi-monitor test

Repeat with two or more monitors at different resolutions and scale factors.
Verify each screen's video fills its own geometry correctly
(`PreserveAspectCrop`, no letterboxing/stretching) and that monitors don't
block on each other's downloads.

### 4. Battery / GPU decode sanity check

While a video is playing, confirm hardware decode is engaged (not a software
fallback burning CPU):

```sh
intel_gpu_top      # Intel
# or
radeontop          # AMD
```

You should see video decode engine usage, and `top`/`htop` should show low
CPU usage from `plasmashell`.

## AUR packaging

```sh
makepkg -si
```

For a clean-room verification before submitting to the AUR, build in a clean
chroot (e.g. `extra-x86_64-build` from `devtools`) rather than relying on your
own machine's package set.

Note: `PKGBUILD`'s `source=` array points at a tagged git release
(`v$pkgver`); push a matching tag before `makepkg` can fetch it.

## CI/CD

- `.github/workflows/ci.yml` runs `cargo fmt`/`clippy`/`test` plus a full
  CMake build and the headless bridge test on every push/PR to `main`.
- Releases are cut with [release-please](https://github.com/googleapis/release-please-action)
  from conventional commits (`release-please-config.json` /
  `.release-please-manifest.json`), which keeps `rust/Cargo.toml`,
  `package/metadata.json`, and `PKGBUILD`'s `pkgver` in sync via `extra-files`.
- `.github/workflows/release-build.yml` builds the Arch package (x86_64 only
  for now) when the GitHub Release for a release-please tag is published,
  attaches it to the GitHub Release, and publishes it to the maintainer's
  [Silo](https://github.com/BirknerAlex/silo) instance.
- Releases created with the default `GITHUB_TOKEN` don't trigger other
  workflows' `release: published` event (GitHub's anti-recursion rule), which
  would silently stop `release-build.yml` from ever running. Set a repo
  secret `RELEASE_PLEASE_TOKEN` -- a PAT or GitHub App token with `contents:
  write` + `pull-requests: write` on this repo -- for release-please to use
  instead; `release-please.yml` falls back to `GITHUB_TOKEN` if it's unset,
  which still opens release PRs but won't chain into the build.
