/*
    SPDX-License-Identifier: GPL-2.0-or-later
*/

import QtQuick
import QtMultimedia
import org.kde.plasma.plasmoid
import org.kde.plasma.wallpaper.aerial 1.0

WallpaperItem {
    id: root

    // Mirrors the TimeOfDay enum in contents/config/main.xml, and
    // `time_filter` in manifest.rs.
    readonly property int timeOfDayMatchClock: 3

    property var playlist: []
    property int playlistIndex: -1
    // Index (0/1) of the MediaPlayer/VideoOutput pair currently visible.
    property int activePlayer: 0
    // Local clock hour the time-of-day filter is currently built around. Kept
    // as state (rather than read inline) so the "match the clock" mode can
    // notice when the hour has moved on and rebuild the playlist.
    property int currentHour: new Date().getHours()

    // Mirrors `quality_url` in manifest.rs, including its fallback: Apple's
    // catalog doesn't guarantee every tier for every clip, and playing one at
    // an unrequested resolution beats an entry that can never load.
    function urlForQuality(entry) {
        if (!entry) {
            return "";
        }
        let preferred;
        switch (root.configuration.Quality) {
        case 1: preferred = entry.url1080HDR; break;
        case 2: preferred = entry.url4kSDR; break;
        case 3: preferred = entry.url4kHDR; break;
        default: preferred = entry.url1080SDR; break;
        }
        if (preferred) {
            return preferred;
        }
        const tiers = [entry.url1080SDR, entry.url1080HDR, entry.url4kSDR, entry.url4kHDR];
        for (const url of tiers) {
            if (url) {
                return url;
            }
        }
        return "";
    }

    function currentEntry() {
        if (root.playlistIndex < 0 || root.playlistIndex >= root.playlist.length) {
            return null;
        }
        return root.playlist[root.playlistIndex];
    }

    function nextEntry() {
        if (root.playlist.length === 0) {
            return null;
        }
        return root.playlist[(root.playlistIndex + 1) % root.playlist.length];
    }

    function playSource(source) {
        const inactive = root.activePlayer === 0 ? playerB : playerA;
        inactive.source = source;
        inactive.play();
        root.activePlayer = root.activePlayer === 0 ? 1 : 0;
        root.loading = false;
    }

    function startCurrent() {
        const entry = root.currentEntry();
        if (!entry) {
            return;
        }
        cache.currentId = entry.id;
        const next = root.nextEntry();
        cache.nextId = next ? next.id : "";

        const immediatePath = cache.ensureDownloaded(entry.id, root.urlForQuality(entry));
        if (immediatePath.length > 0) {
            root.playSource(immediatePath);
        }
        // Otherwise wait for AerialCache.downloadFinished (see below).

        if (next) {
            cache.prefetchNext(next.id, root.urlForQuality(next));
        }
    }

    function advance() {
        if (root.playlist.length === 0) {
            return;
        }
        root.playlistIndex = (root.playlistIndex + 1) % root.playlist.length;
        root.startCurrent();
    }

    function shuffled(entries) {
        const copy = entries.slice();
        for (let i = copy.length - 1; i > 0; --i) {
            const j = Math.floor(Math.random() * (i + 1));
            const tmp = copy[i];
            copy[i] = copy[j];
            copy[j] = tmp;
        }
        return copy;
    }

    function matchesTimeOfDay(timeOfDay) {
        // The rule itself lives in Rust (manifest.rs `time_of_day_matches`),
        // where it's unit tested; this only supplies the clock.
        return manifest.matchesTimeOfDay(timeOfDay, root.configuration.TimeOfDay, root.currentHour);
    }

    function rebuildPlaylist() {
        const previousEntry = root.currentEntry();
        const previousId = previousEntry ? previousEntry.id : "";
        const blacklist = root.configuration.BlacklistedIds || [];
        let entries = [];
        let unfiltered = [];
        for (let i = 0; i < entryCollector.count; ++i) {
            const obj = entryCollector.objectAt(i);
            if (!obj || blacklist.indexOf(obj.entryId) !== -1) {
                continue;
            }
            const entry = {
                id: obj.entryId,
                accessibilityLabel: obj.entryLabel,
                timeOfDay: obj.entryTimeOfDay,
                url1080SDR: obj.entryUrl1080Sdr,
                url1080HDR: obj.entryUrl1080Hdr,
                url4kSDR: obj.entryUrl4kSdr,
                url4kHDR: obj.entryUrl4kHdr,
            };
            unfiltered.push(entry);
            if (root.matchesTimeOfDay(entry.timeOfDay)) {
                entries.push(entry);
            }
        }
        // Never let the filter blank the wallpaper: a catalog with nothing
        // tagged for the selected time of day falls back to the whole thing.
        if (entries.length === 0) {
            entries = unfiltered;
        }

        // While the daily download limit is in force, play only what's
        // already on disk -- otherwise every transition would ask for a video
        // the cache is not allowed to fetch and just skip onwards. With an
        // empty cache there is nothing to fall back to, so the full list
        // stands and the day's one download goes ahead.
        if (!cache.downloadsAllowed) {
            const cached = cache.cachedIds();
            const playable = entries.filter(entry => cached.indexOf(entry.id) !== -1);
            if (playable.length > 0) {
                entries = playable;
            }
        }

        root.playlist = root.configuration.Shuffle ? root.shuffled(entries) : entries;

        if (root.playlist.length === 0) {
            return;
        }
        for (let i = 0; i < root.playlist.length; ++i) {
            if (root.playlist[i].id === previousId) {
                root.playlistIndex = i;
                return;
            }
        }

        // The prior entry was filtered out (or there was no valid prior
        // index). Select an eligible entry and start it even when the old
        // numeric index would still fit in the rebuilt playlist.
        root.playlistIndex = 0;
        root.startCurrent();
    }

    // In "match the clock" mode the right half of the catalog changes as the
    // day goes on, so re-check periodically and rebuild when the hour rolls
    // over. Ten minutes is far finer than the day/night boundaries it's
    // watching for, and costs nothing between rebuilds.
    Timer {
        interval: 10 * 60 * 1000
        repeat: true
        running: true
        onTriggered: {
            // Also re-checks whether the daily download window has reopened;
            // the property change rebuilds the playlist on its own.
            cache.refreshDownloadGate();

            if (root.configuration.TimeOfDay !== root.timeOfDayMatchClock) {
                return;
            }
            const hour = new Date().getHours();
            if (hour === root.currentHour) {
                return;
            }
            root.currentHour = hour;
            root.rebuildPlaylist();
        }
    }

    AerialManifest {
        id: manifest
        onManifestLoaded: root.rebuildPlaylist()
    }

    // Re-validates the manifest whenever the preferred quality changes, since
    // a URL that's live at one quality tier can be dead at another.
    Connections {
        target: root.configuration
        function onQualityChanged() {
            manifest.refresh(root.configuration.Quality);
        }
        // The manifest itself doesn't change with the time-of-day filter --
        // only which of its entries are eligible -- so rebuild the playlist
        // in place instead of refetching.
        function onTimeOfDayChanged() {
            root.currentHour = new Date().getHours();
            root.rebuildPlaylist();
        }
    }

    // Extracts each manifest row into a plain object once, so the playlist can
    // be built/shuffled with ordinary JS array logic instead of re-querying
    // the QAbstractListModel by index every time.
    Instantiator {
        id: entryCollector
        model: manifest
        delegate: QtObject {
            readonly property string entryId: model.id
            readonly property string entryLabel: model.accessibilityLabel
            readonly property string entryTimeOfDay: model.timeOfDay
            readonly property string entryUrl1080Sdr: model.url1080SDR
            readonly property string entryUrl1080Hdr: model.url1080HDR
            readonly property string entryUrl4kSdr: model.url4kSDR
            readonly property string entryUrl4kHdr: model.url4kHDR
        }
    }

    AerialCache {
        id: cache

        // Rebuild whenever the daily window opens or closes: closing drops
        // the playlist to cached clips, opening restores the full catalog so
        // the next transition can fetch one new video.
        onDownloadsAllowedChanged: root.rebuildPlaylist()
        quality: {
            switch (root.configuration.Quality) {
            case 1: return AerialCache.Hdr1080;
            case 2: return AerialCache.Sdr4k;
            case 3: return AerialCache.Hdr4k;
            default: return AerialCache.Sdr1080;
            }
        }
        maxCacheBytes: root.configuration.MaxCacheMB * 1024 * 1024
        wifiOnly: root.configuration.WifiOnly

        onDownloadFinished: (id, path) => {
            const entry = root.currentEntry();
            if (entry && entry.id === id) {
                root.playSource(path);
            }
        }

        onDownloadFailed: (id) => {
            // Apple's manifest lists more entries than currently have live
            // video files -- a stale/dead URL is common, not exceptional.
            // Skip forward rather than leaving the wallpaper black forever.
            const entry = root.currentEntry();
            if (entry && entry.id === id) {
                root.advance();
            }
        }
    }

    Component.onCompleted: {
        root.loading = true; // cleared once the first frame starts playing
        manifest.refresh(root.configuration.Quality);
    }

    Rectangle {
        anchors.fill: parent
        color: "black"
        visible: root.loading
    }

    VideoOutput {
        id: videoOutputA
        anchors.fill: parent
        fillMode: VideoOutput.PreserveAspectCrop
        opacity: root.activePlayer === 0 ? 1 : 0

        Behavior on opacity {
            NumberAnimation { duration: 800; easing.type: Easing.InOutQuad }
        }
    }

    VideoOutput {
        id: videoOutputB
        anchors.fill: parent
        fillMode: VideoOutput.PreserveAspectCrop
        opacity: root.activePlayer === 1 ? 1 : 0

        Behavior on opacity {
            NumberAnimation { duration: 800; easing.type: Easing.InOutQuad }
        }
    }

    // No AudioOutput is attached to either player, so playback is silent by
    // construction (Aerial videos have no meaningful soundtrack to begin with).
    MediaPlayer {
        id: playerA
        videoOutput: videoOutputA
        onMediaStatusChanged: {
            if (mediaStatus === MediaPlayer.EndOfMedia && root.activePlayer === 0) {
                root.advance();
            }
        }
    }

    MediaPlayer {
        id: playerB
        videoOutput: videoOutputB
        onMediaStatusChanged: {
            if (mediaStatus === MediaPlayer.EndOfMedia && root.activePlayer === 1) {
                root.advance();
            }
        }
    }
}
