// Standalone smoke test for the Rust<->QML bridge (aerial-core), independent
// of the Plasma wallpaper KPackage. Loads main.qml, which instantiates
// AerialManifest, fetches the Apple manifest (falling back to the bundled
// snapshot if offline), and exits non-zero if nothing loads within a timeout.
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlEngine>
#include <QTimer>
#include <QUrl>

int
main(int argc, char* argv[])
{
    QGuiApplication app(argc, argv);

    QQmlApplicationEngine engine;
    QObject::connect(&engine, &QQmlEngine::warnings, &app, [](const QList<QQmlError>& warnings) {
        for (const auto& warning : warnings) {
            qWarning() << warning.toString();
        }
    });
    QObject::connect(
        &engine,
        &QQmlApplicationEngine::objectCreationFailed,
        &app,
        []() { QCoreApplication::exit(1); },
        Qt::QueuedConnection);

    // Fail the test if the manifest never loads (e.g. no network and a
    // broken bundled fallback).
    QTimer::singleShot(15000, &app, [&app]() {
        qCritical("Timed out waiting for AerialManifest to load");
        app.exit(2);
    });

    engine.load(QUrl(QStringLiteral("qrc:/qt/qml/tests/qml_bridge_test/main.qml")));
    if (engine.rootObjects().isEmpty()) {
        return 1;
    }

    return app.exec();
}
