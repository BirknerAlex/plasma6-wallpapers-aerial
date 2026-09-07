import QtQuick
import QtQuick.Window
import QtQuick.Controls
import org.kde.plasma.wallpaper.aerial 1.0

ApplicationWindow {
    id: root
    visible: false
    width: 200
    height: 200
    property bool bridgeFailed: false

    AerialManifest {
        id: manifest

        onManifestLoaded: {
            console.log("[bridge-test] manifest ready:", manifest.ready);
            console.log("[bridge-test] entry count:", listView.count);
            if (listView.count > 0) {
                console.log("[bridge-test] first entry id:", listView.itemAtIndex(0) ? listView.itemAtIndex(0).entryId : "(no delegate yet)");
            }
            checkTimeOfDayFilter();
            checkCache();
        }
    }

    AerialCache {
        id: cache
        quality: AerialCache.Sdr1080
        wifiOnly: false
    }

    ListView {
        id: listView
        model: manifest
        anchors.fill: parent
        delegate: Item {
            id: delegateRoot
            readonly property string entryId: model.id
            width: 1
            height: 1
        }
    }

    function checkTimeOfDayFilter() {
        // 0=all, 1=day, 2=night, 3=match the clock -- see contents/config/main.xml.
        const nightAll = manifest.matchesTimeOfDay("night", 0, 12);
        const nightDay = manifest.matchesTimeOfDay("night", 1, 12);
        const nightNight = manifest.matchesTimeOfDay("night", 2, 12);
        const untaggedNight = manifest.matchesTimeOfDay("", 2, 12);
        // Mode 3 reads the hour it is given, so both halves of the day are
        // checkable without waiting for one.
        const dayAtNoon = manifest.matchesTimeOfDay("day", 3, 12);
        const dayAtTwoAm = manifest.matchesTimeOfDay("day", 3, 2);
        console.log("[bridge-test] night clip, all:", nightAll);
        console.log("[bridge-test] night clip, day filter (expect false):", nightDay);
        console.log("[bridge-test] night clip, night filter:", nightNight);
        console.log("[bridge-test] untagged clip, night filter (expect true):", untaggedNight);
        console.log("[bridge-test] day clip, match-clock at 12:00 (expect true):", dayAtNoon);
        console.log("[bridge-test] day clip, match-clock at 02:00 (expect false):", dayAtTwoAm);
        if (nightAll !== true || nightDay !== false || nightNight !== true || untaggedNight !== true
            || dayAtNoon !== true || dayAtTwoAm !== false) {
            console.error("[bridge-test] time-of-day filter returned an unexpected result");
            root.bridgeFailed = true;
        }
    }

    function checkCache() {
        // Exercise the cache invokables without downloading a multi-GB video:
        // an unreachable URL still proves the QML<->Rust call path and the
        // "returns empty string, emits signal later" contract.
        var firstMiss = cache.ensureDownloaded("smoke-test-id", "https://127.0.0.1:1/does-not-exist.mov");
        console.log("[bridge-test] ensureDownloaded immediate result (expect empty):", JSON.stringify(firstMiss));
        Qt.exit(root.bridgeFailed ? 3 : 0);
    }

    Component.onCompleted: {
        console.log("[bridge-test] starting manifest refresh");
        manifest.refresh(AerialCache.Sdr1080);
    }
}
