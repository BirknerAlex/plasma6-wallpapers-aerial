# Contributing

## Architecture

- `rust/` -- the `aerial_core` crate, built as a `cdylib`:
  - `manifest.rs` -- fetches Apple's current (tvOS 26) Aerial resource bundle,
    `https://sylvan.apple.com/Aerials/resources-atv-23J-2.tar`, extracts the
    `entries.json` it contains and parses it, falling back to a bundled
    snapshot on failure. See "Apple's catalog format" below.
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

Separately, a catalog can list entries whose video files are no longer served.
This was severe on the old tvOS 11 endpoint (see below); it still isn't
something we can fix from this side, so `manifest.rs` HEAD-checks each entry
(`filter_live_assets`), `AerialCache` emits `downloadFailed` for any URL that
dies anyway, and `main.qml` skips to the next playlist entry rather than
getting stuck on a black screen.

## Apple's catalog format (tvOS 26 resource bundle)

Apple published a flat `entries.json` only up to tvOS 11
(`https://sylvan.apple.com/Aerials/2x/entries.json`). That document is still
reachable, but the `sylvan.apple.com/Aerials/2x/Videos/...` files it points at
have almost all been removed -- in testing only 1 of 13 entries still
resolved, which is why this plugin used to lean on a community-maintained
mirror of the same schema and on aggressive liveness filtering just to find
anything playable.

Since tvOS 12, each generation ships a `resources*.tar` bundle instead. The
tvOS 26 one is `resources-atv-23J-2.tar`, requested from the fully-qualified
`itunes-assets` path an Apple TV itself uses; `RESOURCES_TAR_URLS` falls back
to the short `sylvan.apple.com/Aerials/`-rooted path for the same file, which
follows the older generations' naming pattern and is reported to work. It
contains, among other resources:

- `entries.json` -- the catalog, ~139 assets, each with versioned
  `https://sylvan.apple.com/itunes-assets/...` video URLs that are actually
  served, plus a `timeOfDay` tag (`day` x100, `sunset` x18, `night` x14,
  `sunrise` x7) that drives the time-of-day playlist filter and a
  `previewImage` still (a 900x580 PNG) shown as a thumbnail in the config
  dialog.
- `TVIdleScreenStringsBundle.bundle/...` -- localized display names, unused
  here since every entry also carries a plain `accessibilityLabel`.

The per-entry schema grew a lot of keys this crate doesn't need
(`categories`, `pointsOfInterest`, `scene`, `timeOfDay`, `previewImage`,
`shotID`, ...), and newer bundles have begun shipping extra video variants
such as `url-4K-SDR-240FPS`. `manifest.rs` therefore deserializes into a
permissive `RawAerialAsset` where everything but `id` is optional, then
normalizes: unknown keys are ignored, a missing `accessibilityLabel` falls
back to `shotID`/`localizedNameKey`/`id`, entries with no usable video URL at
all are dropped, and a requested quality tier that an entry doesn't offer
falls back to one it does (`quality_url`, mirrored by `urlForQuality` in
`main.qml`). A single new or missing key must never invalidate the whole
catalog.

The feed URLs for every tvOS/macOS generation are catalogued in
<https://gist.github.com/theothernt/57a51cade0c12c407f48a5121e0939d5>.

### Time-of-day filter

The `TimeOfDay` config entry (all / day / night / match the clock) filters the
playlist on Apple's `timeOfDay` tag. The rule lives in `manifest.rs`
(`time_of_day_matches`, exposed to QML as `AerialManifest.matchesTimeOfDay`)
so it can be unit tested; `main.qml` only supplies the clock hour and rebuilds
the playlist. Three deliberate choices:

- Twilight is grouped with the half of the day it belongs to -- `sunrise`
  counts as day, `sunset` as night -- because Apple ships only 7 sunrise and
  18 sunset clips, too few to loop on their own.
- "Match the clock" uses fixed hours (day 05:00-18:00, night otherwise), not
  real solar times: a wallpaper has no location permission and no business
  asking for one. `main.qml` re-checks every 10 minutes and rebuilds the
  playlist when the hour rolls into the other half of the day.
- An entry Apple didn't tag always matches, and a filter that would leave the
  playlist empty is ignored (`filter_by_time_of_day`, and the same guard in
  `main.qml`). The wallpaper must never end up with nothing to play.

### Cache budget and the daily download limit

A 4K HDR clip is a few hundred megabytes and the catalog is ~140 of them, so
two settings work together to keep that in hand:

- `MaxCacheMB` defaults to 16 GB (mirrored by `DEFAULT_MAX_CACHE_BYTES` in
  `qml.rs`, which applies before QML sets the property -- keep the two in
  sync, along with `contents/config/main.xml`). The old 4 GB held about ten
  4K clips, so shuffle evicted and re-downloaded almost every transition.
- `AerialCache.downloadIntervalSecs` (default 24h, 0 disables) limits how
  often a video that isn't cached yet may be fetched. `CacheState` owns the
  window: `claim_download_slot` hands out the slot atomically *before* the
  download starts, because `ensureDownloaded` and `prefetchNext` fire back to
  back and a check-then-act gate would let both through. A failed download
  restores the claim, so a dead URL doesn't cost the day.

The window is seeded from the newest cached file's mtime (`load_existing`,
behind the once-only `ensure_loaded`), so it survives a plasmashell restart
rather than granting a fresh download every session -- and `ensure_loaded`
must be awaited before any claim, or that seeding races the first download.

While the window is closed, `downloadsAllowed` is false and `main.qml` builds
the playlist from `cachedIds()` alone, so playback cycles what's on disk
instead of asking for videos the cache will refuse. An empty cache is the
exception: with nothing to fall back to, the full catalog stands and the
day's one download goes ahead. Config-dialog thumbnails are exempt from the
limit -- they're a few hundred KB and the dialog would be useless without
them.

### Config-dialog thumbnails

The locations list in `config.qml` shows Apple's `previewImage` still for each
clip. These cannot be handed to a QML `Image` as https URLs: they live on
`sylvan.apple.com`, whose chain Qt's own network stack rejects for the reason
described above (only `rust/http.rs` pins that CA). So `AerialCache` gained
`ensureThumbnail`/`thumbnailFinished`, which fetch through this crate's client
and hand QML a local `file://` path -- the same contract as
`ensureDownloaded`/`downloadFinished`, minus the failure signal, since a row
without a picture isn't worth a warning.

Thumbnails reuse `CacheState` (download dedup, LRU eviction) with a `png`
extension, their own `~/.cache/aerial-wallpaper/thumbnails` directory and a
fixed 256 MB budget, so preview images and multi-gigabyte videos can never
evict one another.

### Regenerating the bundled fallback snapshot

`rust/assets/entries.fallback.json` is a trimmed copy of that same
`entries.json` (only the fields this crate reads), embedded so the wallpaper
still has a playlist when Apple is unreachable at startup. To refresh it
after Apple ships a new bundle -- update `RESOURCES_TAR_URLS` first if the
URL changed:

```sh
curl -fsSLO https://sylvan.apple.com/itunes-assets/Aerials126/v4/c0/45/d9/c045d9d0-9606-1535-62fe-189edb4f79eb/resources-atv-23J-2.tar
tar -xOf resources-atv-23J-2.tar entries.json | python3 -c '
import collections, json, sys
src = json.load(sys.stdin)
# bundled_fallback_manifest_is_valid requires both of these columns, so catch a
# bundle that drops one here rather than as a confusing test failure later.
for field in ("timeOfDay", "previewImage"):
    missing = [a for a in src["assets"] if not str(a.get(field, "")).strip()]
    if missing:
        print(f"Refusing to replace the fallback snapshot; missing or empty {field}:", file=sys.stderr)
        for a in missing:
            print("  " + str(a.get("id", "<missing id>")), file=sys.stderr)
        sys.exit(1)

keys = ["url-1080-SDR", "url-1080-HDR", "url-4K-SDR", "url-4K-SDR-240FPS", "url-4K-HDR"]
out = collections.OrderedDict(
    version=src.get("version", 1),
    initialAssetCount=src.get("initialAssetCount", len(src["assets"])),
    assets=[
        collections.OrderedDict(
            [("id", a["id"])]
            + [(k, a.get(k, "")) for k in keys]
            + [
                ("accessibilityLabel", a.get("accessibilityLabel") or a.get("shotID") or a["id"]),
                ("timeOfDay", a.get("timeOfDay", "")),
                ("previewImage", a.get("previewImage") or a.get("previewImage-900x580", "")),
            ]
        )
        for a in src["assets"]
    ],
)
with open("rust/assets/entries.fallback.json", "w") as snapshot:
    json.dump(out, snapshot, indent=4)
    snapshot.write("\n")
'
```

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
