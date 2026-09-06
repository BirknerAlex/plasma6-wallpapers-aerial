use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

/// Shared multi-threaded tokio runtime used by the QML-facing QObjects to run
/// networking/caching work off the Qt event loop thread. `Runtime::spawn` is
/// safe to call from any thread, including the Qt main thread, without
/// blocking it. Do NOT call `RUNTIME.block_on(...)` from a Q_INVOKABLE:
/// plasmashell is single-process/single-GUI-thread, so blocking there stalls
/// every panel and the desktop, not just this wallpaper -- see the
/// `sync_cached_paths` mechanism in `qml.rs` for the pattern used instead.
///
/// Worker threads are deliberately capped and de-prioritized (`nice`): a
/// wallpaper downloading a multi-hundred-MB 4K video does real CPU work
/// (TLS decryption, disk writes) that, left at normal scheduling priority on
/// a fully loaded system, competes with -- and can visibly starve -- the Qt
/// GUI thread that draws every panel and the desktop. This was observed
/// directly: the whole shell appeared frozen for the entire duration of a
/// video download and recovered the instant it finished. Since a wallpaper
/// download is not latency-sensitive (nothing user-visible is waiting on
/// it), it's correct for it to always lose CPU contention against the rest
/// of the desktop.
pub static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(4);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name("aerial-core-worker")
        .on_thread_start(|| {
            // SAFETY: nice(2) only adjusts the calling thread's own
            // scheduling priority; it has no side effects on other threads
            // or memory. A positive value lowers priority (0 is default).
            unsafe {
                libc::nice(10);
            }
        })
        .enable_all()
        .build()
        .expect("failed to create tokio runtime for aerial-core")
});
