/*
    SPDX-License-Identifier: GPL-2.0-or-later
*/

import QtQuick
import QtQuick.Window
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
    property int cfg_TimeOfDay: 0
    property int cfg_TimeOfDayDefault: 0
    property bool cfg_Shuffle: true
    property bool cfg_ShuffleDefault: true
    property int cfg_MaxCacheMB: 16384
    property int cfg_MaxCacheMBDefault: 16384
    property bool cfg_WifiOnly: false
    property bool cfg_WifiOnlyDefault: false
    property list<string> cfg_BlacklistedIds: []
    property list<string> cfg_BlacklistedIdsDefault: []

    AerialManifest {
        id: manifest
        Component.onCompleted: manifest.refresh(root.cfg_Quality)
    }

    // Preview images live on Apple's CDN, whose TLS chain Qt's own network
    // stack doesn't trust (see rust/src/http.rs), so an Image can't just point
    // at the https URL -- AerialCache fetches them with the pinned client and
    // hands back a local file:// path.
    AerialCache {
        id: thumbnails
    }

    // Apple's raw timeOfDay tags, as something translatable to show next to a
    // location. An unrecognized tag is shown as-is rather than hidden.
    function timeOfDayLabel(tag) {
        switch (tag) {
        case "day": return i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "day");
        case "night": return i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "night");
        case "sunrise": return i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "sunrise");
        case "sunset": return i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "sunset");
        default: return tag;
        }
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

        QtControls2.ComboBox {
            Kirigami.FormData.label: i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Time of day:")
            model: [
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "All videos"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Day (incl. sunrise)"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Night (incl. sunset)"),
                i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Match the current time"),
            ]
            currentIndex: root.cfg_TimeOfDay
            onActivated: root.cfg_TimeOfDay = currentIndex
        }

        QtControls2.Label {
            Layout.fillWidth: true
            visible: root.cfg_TimeOfDay !== 0
            font: Kirigami.Theme.smallFont
            wrapMode: Text.WordWrap
            text: root.cfg_TimeOfDay === 3
                ? i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Daytime footage from 05:00 to 18:00, night and sunset footage otherwise. Videos Apple did not tag are always played.")
                : i18nd("plasma_wallpaper_io.github.birkneralex.aerial", "Videos Apple did not tag are always played.")
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

            spacing: Kirigami.Units.smallSpacing

            delegate: RowLayout {
                id: locationRow

                required property string id
                required property string accessibilityLabel
                required property string previewImage
                required property string timeOfDay

                width: ListView.view.width
                spacing: Kirigami.Units.largeSpacing

                QtControls2.CheckBox {
                    checked: !root.isBlacklisted(locationRow.id)
                    onToggled: root.setBlacklisted(locationRow.id, !checked)
                    // The label is its own item below, so give the box itself
                    // an accessible name rather than leaving it unnamed.
                    Accessible.name: locationRow.accessibilityLabel
                }

                Rectangle {
                    // Placeholder behind the still, so rows don't jump around
                    // while thumbnails trickle in (or never arrive).
                    implicitWidth: Kirigami.Units.gridUnit * 6
                    implicitHeight: Math.round(implicitWidth * 580 / 900)
                    radius: Kirigami.Units.smallSpacing
                    color: Kirigami.Theme.alternateBackgroundColor

                    Image {
                        id: preview
                        anchors.fill: parent
                        fillMode: Image.PreserveAspectCrop
                        asynchronous: true
                        clip: true
                        // Apple's stills are 900x580; decode them at the size
                        // actually shown instead of keeping ~140 full-size
                        // pixmaps around.
                        sourceSize.width: Math.round(width * Screen.devicePixelRatio)

                        // Cached thumbnails come back immediately; the rest
                        // arrive via thumbnailFinished below.
                        Component.onCompleted: {
                            source = thumbnails.ensureThumbnail(locationRow.id, locationRow.previewImage);
                        }

                        Connections {
                            target: thumbnails
                            function onThumbnailFinished(id, path) {
                                if (id === locationRow.id) {
                                    preview.source = path;
                                }
                            }
                        }
                    }
                }

                QtControls2.Label {
                    Layout.fillWidth: true
                    text: locationRow.accessibilityLabel
                    elide: Text.ElideRight
                }

                QtControls2.Label {
                    visible: locationRow.timeOfDay.length > 0
                    text: root.timeOfDayLabel(locationRow.timeOfDay)
                    color: Kirigami.Theme.disabledTextColor
                    font: Kirigami.Theme.smallFont
                }
            }
        }
    }
}
