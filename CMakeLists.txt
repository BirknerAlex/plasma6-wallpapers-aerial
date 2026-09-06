cmake_minimum_required(VERSION 3.24)

project(plasma6-wallpapers-aerial)

# Rust always links against the non-debug Windows runtime on *-msvc targets;
# irrelevant on Linux, kept for parity with upstream cxx-qt CMake examples.
if(CMAKE_CXX_COMPILER_ID STREQUAL "MSVC")
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
endif()

set(CMAKE_AUTOMOC ON)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(ECM REQUIRED NO_MODULE)
list(APPEND CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

find_package(Qt6 REQUIRED COMPONENTS Core Qml Quick Multimedia Network)
find_package(KF6Package REQUIRED)
find_package(Plasma REQUIRED)

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

# CXX-Qt (via Corrosion) creates a CMake target with the same name as the crate.
cxx_qt_import_crate(
    MANIFEST_PATH ${CMAKE_CURRENT_SOURCE_DIR}/rust/Cargo.toml
    CRATES aerial_core
    LOCKED
    QT_MODULES Qt::Core Qt::Qml Qt::Network
)

# Produces a real qmldir + plugin .so pair that any Qt application (including
# a pre-built plasmashell) can discover at runtime via the QML import path --
# no static linking into plasmashell required. See cmake/AerialQmlPlugin.cmake
# for why this doesn't just use cxx_qt_import_qml_module directly.
include(${CMAKE_SOURCE_DIR}/cmake/AerialQmlPlugin.cmake)
aerial_add_qml_plugin()

# Install the generated qmldir + plugin into Qt's global QML import path so
# `import org.kde.plasma.wallpaper.aerial` resolves for any QML engine,
# notably plasmashell's. Queried at configure time rather than hardcoded,
# since it varies by distro (e.g. /usr/lib/qt6/qml on Arch).
if(NOT DEFINED QT_INSTALL_QML)
    get_target_property(QMAKE_EXECUTABLE Qt6::qmake IMPORTED_LOCATION)
    execute_process(
        COMMAND ${QMAKE_EXECUTABLE} -query QT_INSTALL_QML
        OUTPUT_VARIABLE QT_INSTALL_QML
        OUTPUT_STRIP_TRAILING_WHITESPACE
    )
endif()
message(STATUS "Installing Aerial QML module into: ${QT_INSTALL_QML}")

install(
    DIRECTORY "${AERIAL_QML_OUTPUT_DIR}/org"
    DESTINATION "${QT_INSTALL_QML}"
    USE_SOURCE_PERMISSIONS
)

plasma_install_package(package io.github.birkneralex.aerial wallpapers wallpaper)

option(BUILD_BRIDGE_TEST "Build the standalone Rust<->QML bridge smoke test" ON)
if(BUILD_BRIDGE_TEST)
    add_subdirectory(tests/qml_bridge_test)
endif()
