# Standalone Rust<->QML bridge smoke test for aerial_core, independent of the
# Plasma KPackage. Verifies AerialManifest/AerialCache are usable from QML
# before any Plasma-specific plumbing is involved.
#
# Usable two ways:
#  - As a subdirectory of the top-level CMakeLists.txt (which already imports
#    the aerial_core crate and builds the QML plugin).
#  - Standalone: `cmake -S tests/qml_bridge_test -B build` on its own, in
#    which case it bootstraps CxxQt/Corrosion and the plugin itself.

set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Qt6 REQUIRED COMPONENTS Core Gui Qml Quick QuickControls2 Network)

if(NOT TARGET aerial_core)
    find_package(CxxQt QUIET)
    if(NOT CxxQt_FOUND)
        include(FetchContent)
        FetchContent_Declare(
            CxxQt
            GIT_REPOSITORY https://github.com/kdab/cxx-qt-cmake.git
            GIT_TAG 0.10.0
        )
        FetchContent_MakeAvailable(CxxQt)
    endif()

    cxx_qt_import_crate(
        MANIFEST_PATH ${CMAKE_CURRENT_SOURCE_DIR}/../../rust/Cargo.toml
        CRATES aerial_core
        LOCKED
        QT_MODULES Qt::Core Qt::Qml Qt::Network
    )
endif()

include(${CMAKE_CURRENT_SOURCE_DIR}/../../cmake/AerialQmlPlugin.cmake)
aerial_add_qml_plugin()

add_executable(aerial_qml_bridge_test main.cpp qml/qml.qrc)
target_link_libraries(aerial_qml_bridge_test PRIVATE
    Qt::Core
    Qt::Gui
    Qt::Qml
    Qt::Quick
    Qt::QuickControls2
)
add_dependencies(aerial_qml_bridge_test aerial_wallpaper_qmlplugin aerial_wallpaper_qmltypes)
