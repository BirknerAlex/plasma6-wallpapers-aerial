use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("invalid cache id")]
    InvalidId,
    #[error("network request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    size: u64,
    last_played: u64,
}

/// Result of asking the cache to ensure a video is available locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// Already on disk, here is the path.
    Cached(PathBuf),
    /// Was not cached; has now been downloaded to this path.
    Downloaded {
        path: PathBuf,
        evicted_ids: Vec<String>,
    },
}

impl EnsureOutcome {
    pub fn path(&self) -> &Path {
        match self {
            EnsureOutcome::Cached(path) | EnsureOutcome::Downloaded { path, .. } => path,
        }
    }

    pub fn evicted_ids(&self) -> &[String] {
        match self {
            EnsureOutcome::Cached(_) => &[],
            EnsureOutcome::Downloaded { evicted_ids, .. } => evicted_ids,
        }
    }
}

/// Core download/cache/eviction logic, independent of Qt so it can be unit tested
/// with a plain tokio runtime and a mock HTTP server.
/// One second in the units [`now_ts`] deals in.
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

pub struct CacheState {
    dir: PathBuf,
    /// When a download last claimed the throttle window (see
    /// [`CacheState::claim_download_slot`]), in [`now_ts`] units. Zero means
    /// "never", so the first download into a fresh cache is always allowed.
    ///
    /// Seeded from the newest cached file's mtime by `load_existing`, so the
    /// limit survives a plasmashell restart or a reboot instead of resetting
    /// to "you may download now" every session.
    download_window: AsyncMutex<u64>,
    /// Guards `load_existing` so it runs exactly once, however many callers
    /// need the index (or the download window) to be populated first.
    loaded: tokio::sync::OnceCell<()>,
    /// File extension for cached files, without the dot. Also what
    /// `load_existing` uses to tell this cache's own files apart from
    /// anything else in the directory. The same download/LRU machinery backs
    /// both the video cache (`mov`) and the config dialog's thumbnail cache
    /// (`png`).
    extension: String,
    max_bytes: AsyncMutex<u64>,
    entries: AsyncMutex<HashMap<String, Entry>>,
    locks: AsyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

/// Nanoseconds since the epoch, used purely as a monotonically-increasing
/// ordering key for LRU eviction (not wall-clock precision-sensitive).
fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Extension used by the video cache, and the default for [`CacheState`].
const VIDEO_EXTENSION: &str = "mov";

impl CacheState {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self::with_extension(dir, max_bytes, VIDEO_EXTENSION)
    }

    /// Same cache, for files that aren't videos (see [`CacheState::extension`]).
    pub fn with_extension(
        dir: impl Into<PathBuf>,
        max_bytes: u64,
        extension: impl Into<String>,
    ) -> Self {
        let dir = dir.into();
        Self {
            dir,
            download_window: AsyncMutex::new(0),
            loaded: tokio::sync::OnceCell::new(),
            extension: extension.into(),
            max_bytes: AsyncMutex::new(max_bytes),
            entries: AsyncMutex::new(HashMap::new()),
            locks: AsyncMutex::new(HashMap::new()),
        }
    }

    fn file_name_for(&self, id: &str) -> Result<String, CacheError> {
        let mut components = Path::new(id).components();
        if id.contains(['/', '\\'])
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(CacheError::InvalidId);
        }

        Ok(format!("{id}.{}", self.extension))
    }

    /// Rebuilds the entry index from whatever files already exist in the cache
    /// directory (e.g. left over from a previous session).
    pub async fn load_existing(&self) -> Result<(), CacheError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let mut read_dir = tokio::fs::read_dir(&self.dir).await?;
        let mut newest: u64 = 0;
        let mut entries = self.entries.lock().await;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some(self.extension.as_str()) {
                continue;
            }
            let meta = entry.metadata().await?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or_else(now_ts);
            newest = newest.max(mtime);
            entries.insert(
                stem.to_string(),
                Entry {
                    path,
                    size: meta.len(),
                    last_played: mtime,
                },
            );
        }
        drop(entries);

        // Only ever move the window forward: a download in this session has
        // already claimed a later slot than anything sitting on disk.
        let mut window = self.download_window.lock().await;
        *window = (*window).max(newest);
        Ok(())
    }

    /// Runs `load_existing` once, if it hasn't run yet.
    ///
    /// Every caller that consults the download window has to await this
    /// first: the window is seeded from the files already on disk, and a
    /// claim made before that seeding would hand out a fresh download on
    /// every plasmashell restart.
    pub async fn ensure_loaded(&self) {
        self.loaded
            .get_or_init(|| async {
                if let Err(err) = self.load_existing().await {
                    tracing::warn!("could not scan the cache directory: {err}");
                }
            })
            .await;
    }

    /// Atomically claims the right to download something new, if at least
    /// `interval_ns` has passed since the last claim. Returns the previous
    /// window value on success, for [`CacheState::restore_download_slot`].
    ///
    /// Claiming up front -- rather than checking, downloading, then recording
    /// -- is what keeps the limit honest: `ensureDownloaded` and
    /// `prefetchNext` fire back to back for the current and next clip, and a
    /// check-then-act gate would let both of them through.
    pub async fn claim_download_slot(&self, interval_ns: u64) -> Option<u64> {
        let mut window = self.download_window.lock().await;
        let previous = *window;
        if interval_ns > 0 && previous > 0 && now_ts().saturating_sub(previous) < interval_ns {
            return None;
        }
        *window = now_ts();
        Some(previous)
    }

    /// Hands a claim back after a failed download, so a dead URL doesn't cost
    /// the whole window.
    pub async fn restore_download_slot(&self, previous: u64) {
        let mut window = self.download_window.lock().await;
        // Another claim may have succeeded meanwhile; never rewind past it.
        if now_ts().saturating_sub(*window) < NANOS_PER_SECOND {
            *window = previous;
        }
    }

    /// Nanoseconds until a new download is allowed; 0 when one is allowed now.
    pub async fn download_allowed_in(&self, interval_ns: u64) -> u64 {
        let window = *self.download_window.lock().await;
        if interval_ns == 0 || window == 0 {
            return 0;
        }
        interval_ns.saturating_sub(now_ts().saturating_sub(window))
    }

    pub async fn set_max_bytes(&self, max_bytes: u64) {
        *self.max_bytes.lock().await = max_bytes;
    }

    /// Synchronous-feeling cache lookup for callers (e.g. the QML bridge) that
    /// want to check for a cache hit without starting a download.
    pub async fn cached_path(&self, id: &str) -> Option<PathBuf> {
        let entries = self.entries.lock().await;
        entries.get(id).map(|e| e.path.clone())
    }

    /// Snapshot of every currently cached (id, path) pair, e.g. to prime a
    /// synchronous lookup index for callers that must never block (like a
    /// Q_INVOKABLE running on the Qt GUI thread).
    pub async fn all_cached(&self) -> Vec<(String, PathBuf)> {
        self.entries
            .lock()
            .await
            .iter()
            .map(|(id, e)| (id.clone(), e.path.clone()))
            .collect()
    }

    /// Updates the "last played" timestamp for LRU purposes.
    pub async fn touch(&self, id: &str) {
        let mut entries = self.entries.lock().await;
        if let Some(e) = entries.get_mut(id) {
            e.last_played = now_ts();
        }
    }

    async fn lock_for(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Ensures `id` is downloaded, downloading from `url` if necessary.
    /// Concurrent calls for the same `id` serialize on a per-id lock and the
    /// second (and later) callers observe the cache hit left behind by the
    /// first, so the file is never downloaded twice.
    pub async fn ensure_downloaded(
        &self,
        id: &str,
        url: &str,
        protected: &[&str],
    ) -> Result<EnsureOutcome, CacheError> {
        // Validate before cache lookup and lock creation so untrusted IDs can
        // never become path components or persistent in-memory keys.
        self.file_name_for(id)?;

        if let Some(path) = self.cached_path(id).await {
            self.touch(id).await;
            return Ok(EnsureOutcome::Cached(path));
        }

        let per_id_lock = self.lock_for(id).await;
        let _guard = per_id_lock.lock().await;

        // Re-check: another task may have finished the download while we waited.
        if let Some(path) = self.cached_path(id).await {
            self.touch(id).await;
            return Ok(EnsureOutcome::Cached(path));
        }

        let path = self.download(id, url).await?;
        let evicted_ids = self.evict_lru(protected).await;
        Ok(EnsureOutcome::Downloaded { path, evicted_ids })
    }

    /// Best-effort background prefetch; identical dedup guarantees as
    /// `ensure_downloaded`. The download result itself is ignored, but the
    /// ids LRU eviction removed are returned: a prefetch can evict just as an
    /// explicit download can, and a caller keeping its own index has to hear
    /// about it either way.
    pub async fn prefetch(&self, id: &str, url: &str, protected: &[&str]) -> Vec<String> {
        if self.cached_path(id).await.is_some() {
            return Vec::new();
        }
        match self.ensure_downloaded(id, url, protected).await {
            Ok(outcome) => outcome.evicted_ids().to_vec(),
            Err(_) => Vec::new(),
        }
    }

    async fn download(&self, id: &str, url: &str) -> Result<PathBuf, CacheError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let final_path = self.dir.join(self.file_name_for(id)?);
        let tmp_path = self.dir.join(format!("{id}.part"));

        let response = crate::http::CLIENT
            .get(url)
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?).await?;
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp_path, &final_path).await?;

        let size = tokio::fs::metadata(&final_path).await?.len();
        let mut entries = self.entries.lock().await;
        entries.insert(
            id.to_string(),
            Entry {
                path: final_path.clone(),
                size,
                last_played: now_ts(),
            },
        );
        Ok(final_path)
    }

    /// Evicts least-recently-played entries until total size is under budget,
    /// never touching an id present in `protected`.
    pub async fn evict_lru(&self, protected: &[&str]) -> Vec<String> {
        let max_bytes = *self.max_bytes.lock().await;
        let mut entries = self.entries.lock().await;

        let mut total: u64 = entries.values().map(|e| e.size).sum();
        if total <= max_bytes {
            return Vec::new();
        }

        let mut candidates: Vec<(String, u64, u64)> = entries
            .iter()
            .filter(|(id, _)| !protected.contains(&id.as_str()))
            .map(|(id, e)| (id.clone(), e.last_played, e.size))
            .collect();
        candidates.sort_by_key(|(_, last_played, _)| *last_played);

        let mut evicted_ids = Vec::new();
        for (id, _, size) in candidates {
            if total <= max_bytes {
                break;
            }
            if let Some(entry) = entries.remove(&id) {
                let _ = std::fs::remove_file(&entry.path);
                total -= size;
                evicted_ids.push(id);
            }
        }

        evicted_ids
    }

    #[cfg(test)]
    pub async fn cached_ids(&self) -> Vec<String> {
        self.entries.lock().await.keys().cloned().collect()
    }

    #[cfg(test)]
    pub async fn total_bytes(&self) -> u64 {
        self.entries.lock().await.values().map(|e| e.size).sum()
    }
}

/// Best-effort check of whether the primary network connection is metered
/// (e.g. mobile hotspot), via the NetworkManager D-Bus API. Returns `false`
/// (assume unmetered) if NetworkManager is unreachable, so a Wi-Fi-only
/// setting degrades to "allow" rather than blocking downloads forever on
/// systems without NetworkManager.
pub async fn is_network_metered() -> bool {
    match is_network_metered_inner().await {
        Ok(metered) => metered,
        Err(err) => {
            tracing::debug!("could not query NetworkManager metered state: {err}");
            false
        }
    }
}

async fn is_network_metered_inner() -> zbus::Result<bool> {
    let connection = zbus::Connection::system().await?;

    // NM.Metered enum: 0=unknown, 1=yes, 2=no, 3=guess-yes, 4=guess-no.
    let metered_value: u32 = connection
        .call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager", "Metered"),
        )
        .await?
        .body()
        .deserialize::<zbus::zvariant::Value>()?
        .try_into()?;

    Ok(metered_value == 1 || metered_value == 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn non_video_cache_uses_its_own_extension_and_survives_a_restart() {
        // The config dialog's thumbnail cache shares this machinery with a
        // different extension; a restart must find its files again (and
        // ignore anything else sitting in the directory).
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/preview.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 512]))
            .mount(&server)
            .await;

        let cache = CacheState::with_extension(dir.path(), 1024 * 1024, "png");
        let outcome = cache
            .ensure_downloaded("shot", &format!("{}/preview.png", server.uri()), &[])
            .await
            .expect("download must succeed");
        assert_eq!(outcome.path().extension().unwrap(), "png");

        tokio::fs::write(dir.path().join("stray.mov"), b"not ours")
            .await
            .unwrap();

        let restarted = CacheState::with_extension(dir.path(), 1024 * 1024, "png");
        restarted.load_existing().await.unwrap();
        assert_eq!(restarted.cached_ids().await, vec!["shot".to_string()]);
    }

    #[tokio::test]
    async fn invalid_cache_ids_are_rejected_before_creating_paths() {
        let parent = tempdir().unwrap();
        let cache_dir = parent.path().join("cache");
        let cache = CacheState::new(&cache_dir, 1024 * 1024);

        for id in [
            "",
            ".",
            "..",
            "../escape",
            "/absolute",
            "nested/name",
            "nested\\name",
        ] {
            let result = cache.ensure_downloaded(id, "not a url", &[]).await;
            assert!(matches!(result, Err(CacheError::InvalidId)), "id: {id:?}");
        }

        assert!(!cache_dir.exists());
        assert!(!parent.path().join("escape.mov").exists());
        assert!(!parent.path().join("escape.part").exists());
    }

    #[tokio::test]
    async fn cache_miss_then_hit() {
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/video.mov"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 1024]))
            .expect(1)
            .mount(&server)
            .await;

        let cache = CacheState::new(dir.path(), 10 * 1024 * 1024);
        let url = format!("{}/video.mov", server.uri());

        let first = cache.ensure_downloaded("abc", &url, &[]).await.unwrap();
        assert!(matches!(first, EnsureOutcome::Downloaded { .. }));

        let second = cache.ensure_downloaded("abc", &url, &[]).await.unwrap();
        assert!(matches!(second, EnsureOutcome::Cached(_)));

        // wiremock's `.expect(1)` (checked on drop) asserts the file was only
        // fetched once across both calls.
    }

    #[tokio::test]
    async fn concurrent_calls_download_once() {
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/video.mov"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 1024]))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Arc::new(CacheState::new(dir.path(), 10 * 1024 * 1024));
        let url = format!("{}/video.mov", server.uri());

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let url = url.clone();
            handles.push(tokio::spawn(async move {
                cache.ensure_downloaded("dup", &url, &[]).await.unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn eviction_removes_least_recently_played_but_protects_current() {
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        for name in ["a.mov", "b.mov", "c.mov"] {
            Mock::given(method("GET"))
                .and(path(format!("/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 1000]))
                .mount(&server)
                .await;
        }

        // Budget only large enough for ~2 files.
        let cache = CacheState::new(dir.path(), 2200);

        cache
            .ensure_downloaded("a", &format!("{}/a.mov", server.uri()), &[])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cache
            .ensure_downloaded("b", &format!("{}/b.mov", server.uri()), &[])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        // "a" is oldest and would normally be evicted, but it's protected
        // (e.g. currently playing), so "b" should be evicted instead once "c"
        // pushes the cache over budget.
        cache
            .ensure_downloaded("c", &format!("{}/c.mov", server.uri()), &["a"])
            .await
            .unwrap();

        let ids = cache.cached_ids().await;
        assert!(
            ids.contains(&"a".to_string()),
            "protected id must survive eviction"
        );
        assert!(
            ids.contains(&"c".to_string()),
            "most recent download must survive"
        );
        assert!(cache.total_bytes().await <= 2200 || ids.len() <= 2);
    }

    #[tokio::test]
    async fn download_slot_admits_one_claim_per_window() {
        let dir = tempdir().unwrap();
        let cache = CacheState::new(dir.path(), 1024);
        let day = 24 * 60 * 60 * NANOS_PER_SECOND;

        // First ever claim always wins -- a cold cache must be able to fetch.
        let first = cache
            .claim_download_slot(day)
            .await
            .expect("the first claim must succeed");
        assert_eq!(first, 0);
        assert!(cache.download_allowed_in(day).await > 0);

        // The current clip took the slot; the prefetch of the next one must
        // not also get it.
        assert!(cache.claim_download_slot(day).await.is_none());

        // A failed download hands the window back.
        cache.restore_download_slot(first).await;
        assert_eq!(cache.download_allowed_in(day).await, 0);
        assert!(cache.claim_download_slot(day).await.is_some());

        // An interval of zero is "no limit".
        assert!(cache.claim_download_slot(0).await.is_some());
        assert_eq!(cache.download_allowed_in(0).await, 0);
    }

    #[tokio::test]
    async fn ensure_loaded_scans_once_and_seeds_the_window_before_any_claim() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("old.mov"), vec![0u8; 128])
            .await
            .unwrap();

        let cache = CacheState::new(dir.path(), 1024 * 1024);
        // Two concurrent callers, one scan, and the window seeded either way.
        tokio::join!(cache.ensure_loaded(), cache.ensure_loaded());
        assert_eq!(cache.cached_ids().await, vec!["old".to_string()]);

        let day = 24 * 60 * 60 * NANOS_PER_SECOND;
        assert!(
            cache.claim_download_slot(day).await.is_none(),
            "a file written moments ago must already hold this window"
        );
    }

    #[tokio::test]
    async fn download_window_survives_a_restart() {
        // Otherwise every plasmashell restart would hand out a fresh daily
        // download, which is most of the way back to no limit at all.
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 512]))
            .mount(&server)
            .await;

        let cache = CacheState::new(dir.path(), 1024 * 1024);
        cache
            .ensure_downloaded("a", &format!("{}/a.mov", server.uri()), &[])
            .await
            .unwrap();

        let day = 24 * 60 * 60 * NANOS_PER_SECOND;
        let restarted = CacheState::new(dir.path(), 1024 * 1024);
        restarted.load_existing().await.unwrap();
        assert!(
            restarted.download_allowed_in(day).await > 0,
            "the file just written must count as this window's download"
        );
        assert!(restarted.claim_download_slot(day).await.is_none());

        // A long-idle cache is allowed to fetch again.
        assert!(restarted.claim_download_slot(1).await.is_some());
    }

    #[tokio::test]
    async fn prefetch_reports_ids_removed_by_eviction() {
        // A prefetch evicts exactly like an explicit download does, and the
        // QML bridge's lookup index has to hear about it either way.
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 1000]))
            .mount(&server)
            .await;

        let cache = CacheState::new(dir.path(), 1500);
        cache
            .ensure_downloaded("old", &format!("{}/old.mov", server.uri()), &[])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let evicted = cache
            .prefetch("new", &format!("{}/new.mov", server.uri()), &[])
            .await;
        assert_eq!(evicted, vec!["old".to_string()]);

        // Already cached: nothing downloaded, nothing evicted.
        assert!(cache
            .prefetch("new", &format!("{}/new.mov", server.uri()), &[])
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn download_reports_ids_removed_by_eviction() {
        let dir = tempdir().unwrap();
        let server = MockServer::start().await;
        for name in ["old.mov", "new.mov"] {
            Mock::given(method("GET"))
                .and(path(format!("/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8; 1000]))
                .mount(&server)
                .await;
        }

        let cache = CacheState::new(dir.path(), 1500);
        cache
            .ensure_downloaded("old", &format!("{}/old.mov", server.uri()), &[])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let outcome = cache
            .ensure_downloaded("new", &format!("{}/new.mov", server.uri()), &[])
            .await
            .unwrap();

        assert_eq!(outcome.evicted_ids(), &["old".to_string()]);
        assert!(!dir.path().join("old.mov").exists());
    }
}
