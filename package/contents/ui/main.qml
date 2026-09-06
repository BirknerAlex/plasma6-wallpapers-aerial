/*
    SPDX-License-Identifier: GPL-2.0-or-later
*/

import QtQuick
import QtMultimedia
import org.kde.plasma.plasmoid
import org.kde.plasma.wallpaper.aerial 1.0

WallpaperItem {
    id: root

    property var playlist: []
    property int playlistIndex: -1
    // Index (0/1) of the MediaPlayer/VideoOutput pair currently visible.
    property int activePlayer: 0

    function urlForQuality(entry) {
        if (!entry) {
            return "";
        }
        switch (root.configuration.Quality) {
        case 1: return entry.url1080HDR;
        case 2: return entry.url4kSDR;
        case 3: return entry.url4kHDR;
        default: return entry.url1080SDR;
        }
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

    function rebuildPlaylist() {
        const blacklist = root.configuration.BlacklistedIds || [];
        let entries = [];
        for (let i = 0; i < entryCollector.count; ++i) {
            const obj = entryCollector.objectAt(i);
            if (!obj || blacklist.indexOf(obj.entryId) !== -1) {
                continue;
            }
            entries.push({
                id: obj.entryId,
                accessibilityLabel: obj.entryLabel,
                url1080SDR: obj.entryUrl1080Sdr,
                url1080HDR: obj.entryUrl1080Hdr,
                url4kSDR: obj.entryUrl4kSdr,
                url4kHDR: obj.entryUrl4kHdr,
            });
        }
        root.playlist = root.configuration.Shuffle ? root.shuffled(entries) : entries;

        if (root.playlistIndex < 0 && root.playlist.length > 0) {
            root.playlistIndex = 0;
            root.startCurrent();
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
            readonly property string entryUrl1080Sdr: model.url1080SDR
            readonly property string entryUrl1080Hdr: model.url1080HDR
            readonly property string entryUrl4kSdr: model.url4kSDR
            readonly property string entryUrl4kHdr: model.url4kHDR
        }
    }

    AerialCache {
        id: cache
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
