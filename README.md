# Aerial Wallpaper for Plasma 6

A native Plasma 6 wallpaper plugin that plays Apple TV's "Aerial" screensaver
videos — aerial drone footage of cities and landscapes around the world — as
your desktop wallpaper. Videos are streamed from Apple, cached locally, and
crossfade into one another automatically.

## Features

- Plays the same Aerial video catalog used by Apple TV, from Apple's current
  (tvOS 26) resource bundle — around 140 clips.
- Automatically downloads and caches videos on disk so playback is smooth
  after the first watch.
- Choice of quality: SDR 1080p, HDR 1080p, SDR 4K, or HDR 4K.
- Optional Wi-Fi-only downloading, to avoid burning mobile/metered data.
- Configurable cache size limit (16 GB by default, enough for roughly forty
  4K clips), with automatic eviction of old videos.
- Fetches at most one new video per day, so a 140-clip catalog fills the
  cache gradually in the background instead of downloading constantly.
- Shuffle playback, or exclude specific locations you don't want to see —
  the picker shows a preview thumbnail for each one.
- Filter by time of day: all videos, daytime only, night only, or matched
  to your desktop's clock so it turns dark in the evening with you.
- Works correctly across multiple monitors with different resolutions and
  scale factors.

## Installing

### Arch Linux (pacman repo)

Packages are published for every release to the maintainer's
[Silo](https://github.com/BirknerAlex/silo) instance, **for x86_64 only**
(aarch64 users: build from source or the AUR instead, see below). Add it as a
pacman repository by appending this to `/etc/pacman.conf`:

```ini
[birkneralex]
SigLevel = PackageOptional DatabaseRequired
Server = https://silo.tyrola.dev/birkneralex/stable/pacman/$arch
```

Import the repo's signing key (once), **checking the fingerprint it prints
against the one below before trusting it** — this is what stops a
compromised/spoofed key from being silently accepted:

```sh
curl -fsS https://silo.tyrola.dev/pacman-signing-key | pacman-key --add -
```

Expected fingerprint: `5924 6862 5BFB B22D 7367  C5A0 C792 084B 5084 672E`

If (and only if) it matches, trust it:

```sh
pacman-key --lsign-key 592468625BFBB22D7367C5A0C792084B5084672E
```

Then install as usual (a full `-Syu` avoids Arch's unsupported
["partial upgrade"](https://wiki.archlinux.org/title/System_maintenance#Partial_upgrades_are_unsupported)
state):

```sh
sudo pacman -Syu plasma6-wallpapers-aerial
```

### Arch Linux (AUR)

```sh
makepkg -si
```

Or, once available on the AUR, install with your favorite AUR helper:

```sh
paru -S plasma6-wallpapers-aerial
```

### Other distributions

Prebuilt packages aren't published yet for other distributions. You'll need
`extra-cmake-modules`, `cmake`, `rust`, `corrosion`, and Qt6
(`qt6-base`, `qt6-declarative`, `qt6-multimedia`), then:

```sh
cmake -B build -S .
cmake --build build
sudo cmake --install build
```

## Setting it as your wallpaper

1. Open **System Settings → Appearance → Wallpaper**.
2. Change the wallpaper type dropdown to **Aerial**.
3. The first video will start downloading automatically — playback begins as
   soon as it's ready.

## Configuration

Click the wallpaper's settings (the gear/pencil icon in the wallpaper picker)
to configure:

- **Quality** — SDR 1080p (default), HDR 1080p, SDR 4K, or HDR 4K. Higher
  quality tiers use more bandwidth and disk space; see the HDR note below
  before switching away from the default.
- **Shuffle** — play videos in random order instead of catalog order.
- **Cache size** — maximum disk space to use for downloaded videos (default
  40 GB); the oldest, least-recently-played videos are evicted first once the
  limit is reached.
- **Wi-Fi only** — skip downloading new videos unless connected to Wi-Fi.
- **Excluded locations** — hide specific Aerial locations from playback.

## Troubleshooting

### A video fails to play / gets skipped

Apple's video catalog sometimes lists locations whose video files are no
longer actually hosted. When that happens, the plugin automatically skips to
the next video in the playlist rather than getting stuck on a black screen.
This is expected and isn't something a plugin update can fix — it depends on
Apple's own catalog.

### A panel/taskbar disappears after switching to an HDR quality

On some multi-monitor Wayland setups, playing 4K HDR (Dolby Vision) video can
cause KWin to reset which screen a panel is assigned to, making it invisible.
This is a Wayland/KWin display-mode-switching issue triggered by HDR
playback, not a crash — Plasma stays fully responsive.

Staying on the default **SDR 1080p** quality avoids this entirely. If it
happens and you don't want to switch quality tiers, you can reassign the
panel without restarting Plasma:

```sh
qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript '
var p = panels()[0];
print("current screen: " + p.screen);
p.screen = 0;  // set to whichever screen index the panel belongs on
'
```

## Contributing

See the source layout, build instructions, and testing checklist in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

GPL-2.0-or-later
