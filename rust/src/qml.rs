use std::path::Path;
use std::sync::Arc;

use once_cell::sync::OnceCell;

use crate::cache::CacheState;
use crate::manifest::{fetch_assets, filter_live_assets, time_of_day_matches, AerialAsset};
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

/// Default on-disk budget for cached videos, mirroring `MaxCacheMB`'s default
/// in `contents/config/main.xml`.
///
/// A 4K HDR Aerial clip is a few hundred megabytes, so the old 4 GB default
/// held only about ten of them against a ~140-clip catalog: with shuffle on,
/// nearly every transition evicted something and re-downloaded it. 16 GB
/// holds a useful working set at the top quality tier.
const DEFAULT_MAX_CACHE_BYTES: i64 = 16 * 1024 * 1024 * 1024;

fn default_thumbnail_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aerial-wallpaper")
        .join("thumbnails")
}

/// Budget for the config dialog's preview images. Apple's stills are 900x580
/// PNGs and the whole tvOS 26 catalog is ~140 of them, so this holds the lot
/// with room to spare; it exists only so the directory can't grow without
/// bound across years of catalog churn. Not user-configurable: `maxCacheBytes`
/// is about the multi-gigabyte video cache, and conflating the two would let
/// a tight video budget evict thumbnails on every scroll.
const MAX_THUMBNAIL_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// How long the wallpaper waits between pulling *new* videos from Apple.
///
/// A 4K HDR clip is a few hundred megabytes; left unthrottled against a
/// ~140-clip catalog, an all-day shuffle downloads continuously. One new clip
/// a day fills the cache steadily in the background while everything else
/// plays from disk. Exposed as `downloadIntervalSecs` so QML can lower it (0
/// disables the limit entirely).
const DEFAULT_DOWNLOAD_INTERVAL_SECS: i64 = 24 * 60 * 60;

fn interval_nanos(seconds: i64) -> u64 {
    (seconds.max(0) as u64).saturating_mul(crate::cache::NANOS_PER_SECOND)
}

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

        include!("cxx-qt-lib/qstringlist.h");
        /// QStringList from cxx-qt-lib
        type QStringList = cxx_qt_lib::QStringList;
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
        TimeOfDay,
        PreviewImage,
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
        /// snapshot on failure, then drops entries whose video URL at `quality`
        /// (the QML `Quality` enum ordinal) turns out to be dead. Safe to call
        /// again to refresh, e.g. after the user changes the quality setting.
        #[qinvokable]
        fn refresh(self: Pin<&mut AerialManifest>, quality: i32);

        /// Whether an entry tagged `timeOfDay` belongs in a playlist filtered
        /// with `mode` (the QML `TimeOfDay` config enum: 0=all, 1=day,
        /// 2=night, 3=match the clock) at local clock hour `hour`.
        ///
        /// The hour is passed in rather than read here so the rule stays a
        /// pure, unit-tested function, and so QML gets the same answer for
        /// the wallpaper's own idea of "now" (see `main.qml`).
        #[qinvokable]
        #[cxx_name = "matchesTimeOfDay"]
        fn matches_time_of_day(
            self: &AerialManifest,
            time_of_day: &QString,
            mode: i32,
            hour: i32,
        ) -> bool;
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
        /// Minimum seconds between downloads of videos that aren't cached
        /// yet; 0 downloads whenever playback asks. Thumbnails are exempt.
        #[qproperty(i64, download_interval_secs, cxx_name = "downloadIntervalSecs")]
        /// Whether a new video may be fetched right now. QML binds to this to
        /// keep the playlist on cached entries while the limit is in force;
        /// refreshed by `refreshDownloadGate` and after every download.
        #[qproperty(bool, downloads_allowed, cxx_name = "downloadsAllowed")]
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

        /// Ids currently on disk, for building a playlist that plays without
        /// downloading anything (see `downloadsAllowed`).
        #[qinvokable]
        #[cxx_name = "cachedIds"]
        fn cached_ids(self: &AerialCache) -> QStringList;

        /// Recomputes `downloadsAllowed` against the clock. Cheap; QML calls
        /// it from the same timer that watches the time of day.
        #[qinvokable]
        #[cxx_name = "refreshDownloadGate"]
        fn refresh_download_gate(self: Pin<&mut AerialCache>);

        /// Emitted when a preview image requested via `ensureThumbnail` has
        /// been downloaded.
        #[qsignal]
        #[cxx_name = "thumbnailFinished"]
        fn thumbnail_finished(self: Pin<&mut AerialCache>, id: QString, path: QString);

        /// Preview-image counterpart of `ensureDownloaded`: returns the
        /// `file://` path immediately if the thumbnail for `id` is cached,
        /// otherwise starts an async download and returns an empty string,
        /// later emitting `thumbnailFinished`.
        ///
        /// These cannot be handed to a QML `Image` as plain https URLs:
        /// Apple's CDN is signed by a root Qt's network stack doesn't trust
        /// (see `http.rs`), so they have to come through this crate's client
        /// and be shown from disk. A failed download is silent -- a config
        /// dialog row without a picture is not worth a warning.
        #[qinvokable]
        #[cxx_name = "ensureThumbnail"]
        fn ensure_thumbnail(self: Pin<&mut AerialCache>, id: QString, url: QString) -> QString;
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
    pub fn refresh(self: core::pin::Pin<&mut Self>, quality: i32) {
        use cxx_qt::{CxxQtType, Threading};

        let qt_thread = self.qt_thread();
        RUNTIME.spawn(async move {
            let assets = fetch_assets(&crate::http::CLIENT).await;
            let assets = filter_live_assets(&crate::http::CLIENT, assets, quality).await;
            let _ = qt_thread.queue(move |mut manifest: core::pin::Pin<&mut Self>| {
                // Safety: begin/end reset model bracket a full replacement of
                // the backing Vec, which is exactly what they're for.
                unsafe {
                    manifest.as_mut().begin_reset_model();
                    manifest.as_mut().rust_mut().assets = assets;
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
        roles.insert(qobject::Roles::Url4kSdr.repr, QByteArray::from("url4kSDR"));
        roles.insert(qobject::Roles::Url4kHdr.repr, QByteArray::from("url4kHDR"));
        roles.insert(
            qobject::Roles::TimeOfDay.repr,
            QByteArray::from("timeOfDay"),
        );
        roles.insert(
            qobject::Roles::PreviewImage.repr,
            QByteArray::from("previewImage"),
        );
        roles
    }

    pub fn matches_time_of_day(
        &self,
        time_of_day: &cxx_qt_lib::QString,
        mode: i32,
        hour: i32,
    ) -> bool {
        time_of_day_matches(&time_of_day.to_string(), mode, hour)
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
            qobject::Roles::TimeOfDay => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.time_of_day))
            }
            qobject::Roles::PreviewImage => {
                cxx_qt_lib::QVariant::from(&QString::from(&asset.preview_image))
            }
            _ => cxx_qt_lib::QVariant::default(),
        }
    }
}

type SyncPathIndex = Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;

/// Fast-path lookup for the Qt GUI thread: the cached `file://` path for
/// `id`, but only if that file is still on disk.
///
/// LRU eviction can delete a file while this index still names it, and
/// handing that path back means a video that never loads or a thumbnail that
/// stays blank and is never re-fetched. A stale entry is dropped here so the
/// caller re-downloads instead. The `is_file` check is a single stat on a
/// local cache directory -- unlike the async cache's locks, that is safe to
/// do on the GUI thread.
fn live_cached_path(index: &SyncPathIndex, id: &str) -> Option<String> {
    let path_url = index.lock().unwrap().get(id).cloned()?;
    if path_url
        .strip_prefix("file://")
        .is_some_and(|path| Path::new(path).is_file())
    {
        return Some(path_url);
    }

    let mut index = index.lock().unwrap();
    if index.get(id) == Some(&path_url) {
        index.remove(id);
    }
    None
}

/// Drops ids that LRU eviction removed from a synchronous lookup index.
fn forget_evicted(index: &SyncPathIndex, evicted: &[String]) {
    if evicted.is_empty() {
        return;
    }
    let mut index = index.lock().unwrap();
    for id in evicted {
        index.remove(id);
    }
}

/// Backing Rust state for the [`qobject::AerialCache`] QObject.
pub struct AerialCacheRust {
    quality: qobject::Quality,
    max_cache_bytes: i64,
    download_interval_secs: i64,
    downloads_allowed: bool,
    wifi_only: bool,
    current_id: cxx_qt_lib::QString,
    next_id: cxx_qt_lib::QString,
    state: OnceCell<Arc<CacheState>>,
    thumbnail_state: OnceCell<Arc<CacheState>>,
    // Mirrors CacheState's cache-hit index using a plain std::sync::Mutex
    // (never held across an .await) so `ensureDownloaded` -- a Q_INVOKABLE
    // called directly on the Qt GUI thread -- can check for a cache hit
    // without ever blocking on the tokio runtime. A previous version called
    // `RUNTIME.block_on(...)` here, which blocks the *entire* plasmashell
    // process (single Qt event loop) for as long as the async cache's
    // internal lock is held elsewhere (e.g. by a slow file deletion during
    // eviction) -- this froze the whole shell (panels included) in testing.
    sync_cached_paths: SyncPathIndex,
    sync_thumbnail_paths: SyncPathIndex,
}

impl Default for AerialCacheRust {
    fn default() -> Self {
        Self {
            quality: qobject::Quality::default(),
            max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
            download_interval_secs: DEFAULT_DOWNLOAD_INTERVAL_SECS,
            downloads_allowed: true,
            wifi_only: false,
            current_id: cxx_qt_lib::QString::default(),
            next_id: cxx_qt_lib::QString::default(),
            state: OnceCell::new(),
            thumbnail_state: OnceCell::new(),
            sync_cached_paths: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            sync_thumbnail_paths: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl qobject::AerialCache {
    fn cache_state(&self) -> Arc<CacheState> {
        use cxx_qt::CxxQtType;

        self.state
            .get_or_init(|| {
                let state = Arc::new(CacheState::new(
                    default_cache_dir(),
                    DEFAULT_MAX_CACHE_BYTES as u64,
                ));
                // Prime the synchronous lookup index with whatever is
                // already on disk from a previous session, in the
                // background -- never blocks the caller.
                let scan_state = state.clone();
                let sync_index = self.rust().sync_cached_paths.clone();
                RUNTIME.spawn(async move {
                    scan_state.ensure_loaded().await;
                    let cached = scan_state.all_cached().await;
                    let mut index = sync_index.lock().unwrap();
                    for (id, path) in cached {
                        index.insert(id, path_to_file_url(&path));
                    }
                });
                state
            })
            .clone()
    }

    /// Thumbnail twin of `cache_state`, with its own directory, extension and
    /// budget so preview images and videos never evict each other.
    fn thumbnail_cache_state(&self) -> Arc<CacheState> {
        use cxx_qt::CxxQtType;

        self.thumbnail_state
            .get_or_init(|| {
                let state = Arc::new(CacheState::with_extension(
                    default_thumbnail_dir(),
                    MAX_THUMBNAIL_CACHE_BYTES,
                    "png",
                ));
                let scan_state = state.clone();
                let sync_index = self.rust().sync_thumbnail_paths.clone();
                RUNTIME.spawn(async move {
                    scan_state.ensure_loaded().await;
                    let cached = scan_state.all_cached().await;
                    let mut index = sync_index.lock().unwrap();
                    for (id, path) in cached {
                        index.insert(id, path_to_file_url(&path));
                    }
                });
                state
            })
            .clone()
    }

    pub fn cached_ids(&self) -> cxx_qt_lib::QStringList {
        use cxx_qt::CxxQtType;

        let mut list = cxx_qt_lib::QList::<cxx_qt_lib::QString>::default();
        for id in self.rust().sync_cached_paths.lock().unwrap().keys() {
            list.append(cxx_qt_lib::QString::from(id));
        }
        cxx_qt_lib::QStringList::from(&list)
    }

    pub fn refresh_download_gate(self: core::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;

        let state = self.cache_state();
        let interval = interval_nanos(*self.download_interval_secs());
        let qt_thread = self.qt_thread();
        RUNTIME.spawn(async move {
            state.ensure_loaded().await;
            let allowed = state.download_allowed_in(interval).await == 0;
            let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                if *cache.downloads_allowed() != allowed {
                    cache.as_mut().set_downloads_allowed(allowed);
                }
            });
        });
    }

    pub fn ensure_thumbnail(
        self: core::pin::Pin<&mut Self>,
        id: cxx_qt_lib::QString,
        url: cxx_qt_lib::QString,
    ) -> cxx_qt_lib::QString {
        use cxx_qt::{CxxQtType, Threading};

        let id_str = id.to_string();
        let url_str = url.to_string();
        if url_str.is_empty() {
            return cxx_qt_lib::QString::default();
        }

        // Same non-blocking cache-hit check as `ensure_downloaded`: this runs
        // on the Qt GUI thread, once per config-dialog row.
        if let Some(path_url) = live_cached_path(&self.rust().sync_thumbnail_paths, &id_str) {
            return cxx_qt_lib::QString::from(&path_url);
        }

        let state = self.thumbnail_cache_state();
        let sync_index = self.rust().sync_thumbnail_paths.clone();
        let qt_thread = self.qt_thread();

        RUNTIME.spawn(async move {
            match state.ensure_downloaded(&id_str, &url_str, &[]).await {
                Ok(outcome) => {
                    let path_url = path_to_file_url(outcome.path());
                    forget_evicted(&sync_index, outcome.evicted_ids());
                    sync_index
                        .lock()
                        .unwrap()
                        .insert(id_str.clone(), path_url.clone());
                    let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                        cache.as_mut().thumbnail_finished(
                            cxx_qt_lib::QString::from(&id_str),
                            cxx_qt_lib::QString::from(&path_url),
                        );
                    });
                }
                Err(err) => {
                    // No signal: the row simply stays without a picture.
                    tracing::debug!("thumbnail download failed for {id_str}: {err}");
                }
            }
        });

        cxx_qt_lib::QString::default()
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
        if let Some(path_url) = live_cached_path(&self.rust().sync_cached_paths, &id_str) {
            return cxx_qt_lib::QString::from(&path_url);
        }

        let state = self.cache_state();
        let max_bytes = (*self.max_cache_bytes()).max(0) as u64;
        let interval = interval_nanos(*self.download_interval_secs());
        let current = self.current_id().to_string();
        let next = self.next_id().to_string();
        let sync_index = self.rust().sync_cached_paths.clone();
        let qt_thread = self.qt_thread();
        let url_str = url.to_string();

        RUNTIME.spawn(async move {
            state.set_max_bytes(max_bytes).await;
            state.ensure_loaded().await;

            // Claimed before the download starts, so the current clip and the
            // prefetch of the next one can't each take a slot.
            let Some(claim) = state.claim_download_slot(interval).await else {
                tracing::debug!("skipping download of {id_str}: waiting for the next window");
                let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                    cache.as_mut().set_downloads_allowed(false);
                    cache
                        .as_mut()
                        .download_failed(cxx_qt_lib::QString::from(&id_str));
                });
                return;
            };

            let protected = [current.as_str(), next.as_str()];
            match state.ensure_downloaded(&id_str, &url_str, &protected).await {
                Ok(outcome) => {
                    let path_url = path_to_file_url(outcome.path());
                    forget_evicted(&sync_index, outcome.evicted_ids());
                    sync_index
                        .lock()
                        .unwrap()
                        .insert(id_str.clone(), path_url.clone());
                    // A cache hit costs nothing, so it must not close the
                    // window; only a genuine download does.
                    let downloaded =
                        matches!(outcome, crate::cache::EnsureOutcome::Downloaded { .. });
                    if !downloaded {
                        state.restore_download_slot(claim).await;
                    }
                    let still_allowed = state.download_allowed_in(interval).await == 0;
                    let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                        cache.as_mut().set_downloads_allowed(still_allowed);
                        cache.as_mut().download_finished(
                            cxx_qt_lib::QString::from(&id_str),
                            cxx_qt_lib::QString::from(&path_url),
                        );
                    });
                }
                Err(err) => {
                    // A dead URL must not cost the whole window.
                    state.restore_download_slot(claim).await;
                    tracing::warn!("download failed for {id_str}: {err}");
                    let _ = qt_thread.queue(move |mut cache: core::pin::Pin<&mut Self>| {
                        cache
                            .as_mut()
                            .download_failed(cxx_qt_lib::QString::from(&id_str));
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
        if live_cached_path(&self.rust().sync_cached_paths, &id_str).is_some() {
            return;
        }

        let state = self.cache_state();
        let current = self.current_id().to_string();
        let next = self.next_id().to_string();
        let sync_index = self.rust().sync_cached_paths.clone();
        let interval = interval_nanos(*self.download_interval_secs());
        let url_str = url.to_string();
        RUNTIME.spawn(async move {
            state.ensure_loaded().await;
            // Prefetching is best-effort, so a closed window just skips it --
            // no signal, no restored claim.
            let Some(claim) = state.claim_download_slot(interval).await else {
                return;
            };
            let protected = [current.as_str(), next.as_str()];
            let evicted = state.prefetch(&id_str, &url_str, &protected).await;
            if state.cached_path(&id_str).await.is_none() {
                state.restore_download_slot(claim).await;
            }
            forget_evicted(&sync_index, &evicted);
            if let Some(path) = state.cached_path(&id_str).await {
                sync_index
                    .lock()
                    .unwrap()
                    .insert(id_str, path_to_file_url(&path));
            }
        });
    }
}
