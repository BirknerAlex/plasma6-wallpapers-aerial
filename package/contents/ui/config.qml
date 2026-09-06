/*
    SPDX-License-Identifier: GPL-2.0-or-later
*/

import QtQuick
import QtQuick.Controls as QtControls2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.wallpaper.aerial 1.0

ColumnLayout {
    id: root

    property var configDialog
    property var wallpaperConfiguration: wallpaper.configuration

    property int cfg_Quality: 0
    property int cfg_QualityDefault: 0
    property bool cfg_Shuffle: true
    property bool cfg_ShuffleDefault: true
    property int cfg_MaxCacheMB: 4096
    property int cfg_MaxCacheMBDefault: 4096
    property bool cfg_WifiOnly: false
    property bool cfg_WifiOnlyDefault: false
    property list<string> cfg_BlacklistedIds: []
    property list<string> cfg_BlacklistedIdsDefault: []

    AerialManifest {
        id: manifest
        Component.onCompleted: manifest.refresh(root.cfg_Quality)
    }

    function isBlacklisted(id) {
        return root.cfg_BlacklistedIds.indexOf(id) !== -1;
    }

    function setBlacklisted(id, excluded) {
        const current = root.cfg_BlacklistedIds.slice();
        const index = current.indexOf(id);
        if (excluded && index === -1) {
            current.push(id);
        } else if (!excluded && index !== -1) {
            current.splice(index, 1);
        }
        root.cfg_BlacklistedIds = current;
    }

    Kirigami.FormLayout {
        id: formLayout
        Layout.fillWidth: true

        QtControls2.ComboBox {
            Kirigami.FormData.label: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Quality:")
            model: [
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "1080p SDR"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "1080p HDR"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "4K SDR"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "4K HDR"),
            ]
            currentIndex: root.cfg_Quality
            onActivated: root.cfg_Quality = currentIndex
        }

        QtControls2.CheckBox {
            Kirigami.FormData.label: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Playback:")
            text: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Shuffle videos")
            checked: root.cfg_Shuffle
            onToggled: root.cfg_Shuffle = checked
        }

        QtControls2.CheckBox {
            text: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Only download over Wi-Fi")
            checked: root.cfg_WifiOnly
            onToggled: root.cfg_WifiOnly = checked
        }

        RowLayout {
            Kirigami.FormData.label: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Cache limit:")

            QtControls2.SpinBox {
                from: 512
                to: 102400
                stepSize: 512
                value: root.cfg_MaxCacheMB
                onValueModified: root.cfg_MaxCacheMB = value
            }

            QtControls2.Label {
                text: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "MB")
            }
        }
    }

    Kirigami.Separator {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.largeSpacing
    }

    QtControls2.Label {
        Layout.topMargin: Kirigami.Units.smallSpacing
        text: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Locations")
        font.bold: true
    }

    QtControls2.ScrollView {
        Layout.fillWidth: true
        Layout.fillHeight: true
        Layout.minimumHeight: Kirigami.Units.gridUnit * 10

        ListView {
            model: manifest
            clip: true

            delegate: QtControls2.CheckBox {
                required property string id
                required property string accessibilityLabel

                width: ListView.view.width
                text: accessibilityLabel
                checked: !root.isBlacklisted(id)
                onToggled: root.setBlacklisted(id, !checked)
            }
        }
    }
}
