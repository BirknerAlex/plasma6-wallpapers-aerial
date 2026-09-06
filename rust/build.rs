use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    // QML type registration for AerialManifest/AerialCache happens via static
    // initializers baked into this cdylib -- they run automatically whenever
    // the library is loaded, independent of PluginType. The actual QML
    // *plugin* wrapper (the thing Qt dlsym()s to load the module) is instead
    // hand-written in plugin/ and built directly by CMake; see
    // cmake/AerialQmlPlugin.cmake for why.
    CxxQtBuilder::new_qml_module(QmlModule::new("org.kde.plasma.wallpaper.aerial"))
        .qt_module("Network")
        .file("src/qml.rs")
        .build();
}
