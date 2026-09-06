use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Error)]
pub enum CacheError {
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
    Downloaded(PathBuf),
}

impl EnsureOutcome {
    pub fn path(&self) -> &Path {
        match self {
            EnsureOutcome::Cached(p) | EnsureOutcome::Downloaded(p) => p,
        }
    }
}

/// Core download/cache/eviction logic, independent of Qt so it can be unit tested
/// with a plain tokio runtime and a mock HTTP server.
pub struct CacheState {
    dir: PathBuf,
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

fn file_name_for(id: &str) -> String {
    format!("{id}.mov")
}

impl CacheState {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        let dir = dir.into();
        Self {
            dir,
            max_bytes: AsyncMutex::new(max_bytes),
            entries: AsyncMutex::new(HashMap::new()),
            locks: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Rebuilds the entry index from whatever files already exist in the cache
    /// directory (e.g. left over from a previous session).
    pub async fn load_existing(&self) -> Result<(), CacheError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let mut read_dir = tokio::fs::read_dir(&self.dir).await?;
        let mut entries = self.entries.lock().await;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("mov") {
                continue;
            }
            let meta = entry.metadata().await?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or_else(now_ts);
            entries.insert(
                stem.to_string(),
                Entry {
                    path,
                    size: meta.len(),
                    last_played: mtime,
                },
            );
        }
        Ok(())
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
        self.evict_lru(protected).await;
        Ok(EnsureOutcome::Downloaded(path))
    }

    /// Best-effort background prefetch; identical dedup guarantees as
    /// `ensure_downloaded`, but callers are expected to ignore the result.
    pub async fn prefetch(&self, id: &str, url: &str, protected: &[&str]) {
        if self.cached_path(id).await.is_some() {
            return;
        }
        let _ = self.ensure_downloaded(id, url, protected).await;
    }

    async fn download(&self, id: &str, url: &str) -> Result<PathBuf, CacheError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let final_path = self.dir.join(file_name_for(id));
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
    pub async fn evict_lru(&self, protected: &[&str]) {
        let max_bytes = *self.max_bytes.lock().await;
        let mut entries = self.entries.lock().await;

        let mut total: u64 = entries.values().map(|e| e.size).sum();
        if total <= max_bytes {
            return;
        }

        let mut candidates: Vec<(String, u64, u64)> = entries
            .iter()
            .filter(|(id, _)| !protected.contains(&id.as_str()))
            .map(|(id, e)| (id.clone(), e.last_played, e.size))
            .collect();
        candidates.sort_by_key(|(_, last_played, _)| *last_played);

        for (id, _, size) in candidates {
            if total <= max_bytes {
                break;
            }
            if let Some(entry) = entries.remove(&id) {
                let _ = std::fs::remove_file(&entry.path);
                total -= size;
            }
        }
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
        assert!(matches!(first, EnsureOutcome::Downloaded(_)));

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
        assert!(ids.contains(&"a".to_string()), "protected id must survive eviction");
        assert!(ids.contains(&"c".to_string()), "most recent download must survive");
        assert!(cache.total_bytes().await <= 2200 || ids.len() <= 2);
    }
}
