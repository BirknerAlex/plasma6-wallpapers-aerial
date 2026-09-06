use std::path::Path;
use std::sync::Arc;

use once_cell::sync::OnceCell;

use crate::cache::CacheState;
use crate::manifest::{fetch_manifest_or_fallback, AerialAsset};
use crate::runtime::RUNTIME;

fn path_to_file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn default_cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aerial-wallpaper")
        .join("videos")
}

const DEFAULT_MAX_CACHE_BYTES: i64 = 4 * 1024 * 1024 * 1024;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);
        /// Base class providing the QML-facing list model behaviour.
        #[qobject]
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        /// QString from cxx-qt-lib
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qvariant.h");
        /// QVariant from cxx-qt-lib
        type QVariant = cxx_qt_lib::QVariant;

        include!("cxx-qt-lib/qmodelindex.h");
        /// QModelIndex from cxx-qt-lib
        type QModelIndex = cxx_qt_lib::QModelIndex;

        include!("cxx-qt-lib/qhash.h");
        /// QHash<i32, QByteArray> from cxx-qt-lib, used for QAbstractListModel::roleNames
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;

        include!("cxx-qt-lib/qbytearray.h");
        /// QByteArray from cxx-qt-lib
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    /// Column roles exposed by [`AerialManifest`] to QML delegates.
    #[qenum(AerialManifest)]
    enum Roles {
        Id,
        AccessibilityLabel,
        Url1080Sdr,
        Url1080Hdr,
        Url4kSdr,
        Url4kHdr,
    }

    extern "RustQt" {
        /// Async-fetched list of Apple Aerial screensaver entries, exposed as a
        /// `QAbstractListModel` so QML can bind a `Repeater`/`ListView` directly to it.
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qproperty(bool, ready)]
        type AerialManifest = super::AerialManifestRust;

        /// Emitted once the manifest has finished (re)loading.
        #[qsignal]
        #[cxx_name = "manifestLoaded"]
        fn manifest_loaded(self: Pin<&mut AerialManifest>);

        /// Starts (re)fetching the manifest from Apple, falling back to the bundled
        /// snapshot on failure. Safe to call again to refresh.
        #[qinvokable]
        fn refresh(self: Pin<&mut AerialManifest>);
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut AerialManifest>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut AerialManifest>);
    }

    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &AerialManifest) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &AerialManifest, _parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &AerialManifest, index: &QModelIndex, role: i32) -> QVariant;
    }

    impl cxx_qt::Threading for AerialManifest {}

    /// Selectable download quality, mirrors the four variants Apple publishes per clip.
    #[qenum(AerialCache)]
    enum Quality {
        Sdr1080,
        Hdr1080,
        Sdr4k,
        Hdr4k,
    }

    extern "RustQt" {
        /// Downloads and LRU-caches Aerial video files on disk, keyed by asset id.
        #[qobject]
        #[qml_element]
        #[qproperty(Quality, quality)]
        #[qproperty(i64, max_cache_bytes, cxx_name = "maxCacheBytes")]
        #[qproperty(bool, wifi_only, cxx_name = "wifiOnly")]
        #[qproperty(QString, current_id, cxx_name = "currentId")]
        #[qproperty(QString, next_id, cxx_name = "nextId")]
        type AerialCache = super::AerialCacheRust;

        /// Emitted when a background download started by `ensureDownloaded` or
        /// `prefetchNext` completes.
        #[qsignal]
        #[cxx_name = "downloadFinished"]
        fn download_finished(self: Pin<&mut AerialCache>, id: QString, path: QString);

        /// Emitted when a background download started by `ensureDownloaded`
        /// fails (e.g. the manifest lists a URL Apple has since removed --
        /// observed to be common). QML should treat this as "skip to the
        /// next playlist entry", not just show a black screen forever.
        #[qsignal]
        #[cxx_name = "downloadFailed"]
        fn download_failed(self: Pin<&mut AerialCache>, id: QString);

        /// Returns the `file://` path immediately if `id` is already cached;
        /// otherwise starts an async download and returns an empty string,
        /// later emitting `downloadFinished`. Idempotent while a download for
        /// `id` is in flight.
        #[qinvokable]
        #[cxx_name = "ensureDownloaded"]
        fn ensure_downloaded(self: Pin<&mut AerialCache>, id: QString, url: QString) -> QString;

        /// Best-effort background prefetch of the next item in the playlist.
        #[qinvokable]
        #[cxx_name = "prefetchNext"]
        fn prefetch_next(self: Pin<&mut AerialCache>, id: QString, url: QString);
    }

    impl cxx_qt::Threading for AerialCache {}
}

impl Default for qobject::Quality {
    fn default() -> Self {
        qobject::Quality::Sdr1080
    }
}

/// Backing Rust state for the [`qobject::AerialManifest`] QObject.
#[derive(Default)]
pub struct AerialManifestRust {
    assets: Vec<AerialAsset>,
    ready: bool,
}

impl qobject::AerialManifest {
    pub fn refresh(self: core::pin::Pin<&mut Self>) {
        use cxx_qt::{CxxQtType, Threading};

        let qt_thread = self.qt_thread();
        RUNTIME.spawn(async move {
            let data = fetch_manifest_or_fallback(&crate::http::CLIENT).await;
            let _ = qt_thread.queue(move |mut manifest: core::pin::Pin<&mut Self>| {
                // Safety: begin/end reset model bracket a full replacement of
                // the backing Vec, which is exactly what they're for.
                unsafe {
                    manifest.as_mut().begin_reset_model();
                    manifest.as_mut().rust_mut().assets = data.assets;
                    manifest.as_mut().end_reset_model();
                }
                manifest.as_mut().set_ready(true);
                manifest.as_mut().manifest_loaded();
            });
        });
    }

    pub fn role_names(&self) -> cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray> {
        use cxx_qt_lib::{QByteArray, QHash};

        let mut roles = QHash::default();
        roles.insert(qobject::Roles::Id.repr, QByteArray::from("id"));
        roles.insert(
            qobject::Roles::AccessibilityLabel.repr,
            QByteArray::from("accessibilityLabel"),
        );
        roles.insert(
            qobject::Roles::Url1080Sdr.repr,
            QByteArray::from("url1080SDR"),
        );
        roles.insert(
            qobject::Roles::Url1080Hdr.repr,
            QByteArray::from("url1080HDR"),
        );
        roles.insert(
            qobject::Roles::Url4kSdr.repr,
            QByteArray::from("url4kSDR"),
        );
        roles.insert(
            qobject::Roles::Url4kHdr.repr,
            QByteArray::from("url4kHDR"),
        );
        roles
    }

    pub fn row_count(&self, _parent: &cxx_qt_lib::QModelIndex) -> i32 {
        self.assets.len() as i32
    }

    pub fn data(&self, index: &cxx_qt_lib::QModelIndex, role: i32) -> cxx_qt_lib::QVariant {
        use cxx_qt_lib::QString;

        let Some(asset) = self.assets.get(index.row() as usize) else {
            return cxx_qt_lib::QVariant::default();
        };
        let role = qobject::Roles { repr: role };
        match role {
            qobject::Roles::Id => cxx_qt_lib::QVariant::from(&QString::from(&asset.id)),
            qobject::Roles::AccessibilityLabel => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.accessibility_label))
            }
            qobject::Roles::Url1080Sdr => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.url_1080_sdr))
            }
            qobject::Roles::Url1080Hdr => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.url_1080_hdr))
            }
            qobject::Roles::Url4kSdr => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.url_4k_sdr))
            }
            qobject::Roles::Url4kHdr => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.url_4k_hdr))
            }
            _ => cxx_qt_lib::QVariant::default(),
        }
    }
}

type SyncPathIndex = Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;

/// Backing Rust state for the [`qobject::AerialCache`] QObject.
pub struct AerialCacheRust {
    quality: qobject::Quality,
    max_cache_bytes: i64,
    wifi_only: bool,
    current_id: cxx_qt_lib::QString,
    next_id: cxx_qt_lib::QString,
    state: OnceCell<Arc<CacheState>>,
    // Mirrors CacheState's cache-hit index using a plain std::sync::Mutex
    // (never held across an .await) so `ensureDownloaded` -- a Q_INVOKABLE
    // called directly on the Qt GUI thread -- can check for a cache hit
    // without ever blocking on the tokio runtime. A previous version called
    // `RUNTIME.block_on(...)` here, which blocks the *entire* plasmashell
    // process (single Qt event loop) for as long as the async cache's
    // internal lock is held elsewhere (e.g. by a slow file deletion during
    // eviction) -- this froze the whole shell (panels included) in testing.
    sync_cached_paths: SyncPathIndex,
}

impl Default for AerialCacheRust {
    fn default() -> Self {
        Self {
            quality: qobject::Quality::default(),
            max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
            wifi_only: false,
            current_id: cxx_qt_lib::QString::default(),
            next_id: cxx_qt_lib::QString::default(),
            state: OnceCell::new(),
            sync_cached_paths: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl qobject::AerialCache {
    fn cache_state(&self) -> Arc<CacheState> {
        use cxx_qt::CxxQtType;

        self.state
            .get_or_init(|| {
                let state = Arc::new(CacheState::new(default_cache_dir(), DEFAULT_MAX_CACHE_BYTES as u64));
                // Prime the synchronous lookup index with whatever is
                // already on disk from a previous session, in the
                // background -- never blocks the caller.
                let scan_state = state.clone();
                let sync_index = self.rust().sync_cached_paths.clone();
                RUNTIME.spawn(async move {
                    if scan_state.load_existing().await.is_ok() {
                        let cached = scan_state.all_cached().await;
                        let mut index = sync_index.lock().unwrap();
                        for (id, path) in cached {
                            index.insert(id, path_to_file_url(&path));
                        }
                    }
                });
                state
            })
            .clone()
    }

    pub fn ensure_downloaded(
        self: core::pin::Pin<&mut Self>,
        id: cxx_qt_lib::QString,
        url: cxx_qt_lib::QString,
    ) -> cxx_qt_lib::QString {
        use cxx_qt::{CxxQtType, Threading};

        let id_str = id.to_string();

        // Fast, non-blocking cache-hit check -- safe to call from the Qt GUI
        // thread since this plain mutex is only ever held for a few
        // in-memory HashMap operations, never across an .await.
        if let Some(path_url) = self.rust().sync_cached_paths.lock().unwrap().get(&id_str).cloned() {
            return cxx_qt_lib::QString::from(&path_url);
        }

        let state = self.cache_state();
        let max_bytes = (*self.max_cache_bytes()).max(0) as u64;
        let current = self.current_id().to_string();
        let next = self.next_id().to_string();
        let sync_index = self.rust().sync_cached_paths.clone();
        let qt_thread = self.qt_thread();
        let url_str = url.to_string();

        RUNTIME.spawn(async move {
            state.set_max_bytes(max_bytes).await;
            let protected = [current.as_str(), next.as_str()];
            match state.ensure_downloaded(&id_str, &url_str, &protected).await {
                Ok(outcome) => {
                    let path_url = path_to_file_url(outcome.path());
                    sync_index.lock().unwrap().insert(id_str.clone(), path_url.clone());
                    let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                        cache.as_mut().download_finished(
                            cxx_qt_lib::QString::from(&id_str),
                            cxx_qt_lib::QString::from(&path_url),
                        );
                    });
                }
                Err(err) => {
                    tracing::warn!("download failed for {id_str}: {err}");
                    let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                        cache.as_mut().download_failed(cxx_qt_lib::QString::from(&id_str));
                    });
                }
            }
        });

        cxx_qt_lib::QString::default()
    }

    pub fn prefetch_next(
        self: core::pin::Pin<&mut Self>,
        id: cxx_qt_lib::QString,
        url: cxx_qt_lib::QString,
    ) {
        use cxx_qt::CxxQtType;

        let id_str = id.to_string();
        if self.rust().sync_cached_paths.lock().unwrap().contains_key(&id_str) {
            return;
        }

        let state = self.cache_state();
        let current = self.current_id().to_string();
        let next = self.next_id().to_string();
        let sync_index = self.rust().sync_cached_paths.clone();
        let url_str = url.to_string();
        RUNTIME.spawn(async move {
            let protected = [current.as_str(), next.as_str()];
            state.prefetch(&id_str, &url_str, &protected).await;
            if let Some(path) = state.cached_path(&id_str).await {
                sync_index
                    .lock()
                    .unwrap()
                    .insert(id_str, path_to_file_url(&path));
            }
        });
    }
}
