import QtQuick
import QtQuick.Window
import QtQuick.Controls
import org.kde.plasma.wallpaper.aerial 1.0

ApplicationWindow {
    id: root
    visible: false
    width: 200
    height: 200

    AerialManifest {
        id: manifest

        onManifestLoaded: {
            console.log("[bridge-test] manifest ready:", manifest.ready);
            console.log("[bridge-test] entry count:", listView.count);
            if (listView.count > 0) {
                console.log("[bridge-test] first entry id:", listView.itemAtIndex(0) ? listView.itemAtIndex(0).entryId : "(no delegate yet)");
            }
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

    function checkCache() {
        // Exercise the cache invokables without downloading a multi-GB video:
        // an unreachable URL still proves the QML<->Rust call path and the
        // "returns empty string, emits signal later" contract.
        var firstMiss = cache.ensureDownloaded("smoke-test-id", "https://127.0.0.1:1/does-not-exist.mov");
        console.log("[bridge-test] ensureDownloaded immediate result (expect empty):", JSON.stringify(firstMiss));
        Qt.quit();
    }

    Component.onCompleted: {
        console.log("[bridge-test] starting manifest refresh");
        manifest.refresh();
    }
}
