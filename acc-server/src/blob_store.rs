/// Shared in-memory blob store with per-entry TTL, background expiry sweep,
/// and AccFS storage cleanup.
///
/// # Design
///
/// Every stored blob carries an `expires_at` instant computed from the
/// caller-supplied `ttl_secs` at insert time.  A `BlobStore` is an
/// `Arc`-wrapped struct so it can be cheaply cloned into both the Axum
/// extension and the background sweep task.
///
/// ## TTL semantics (per SPEC.md)
///
/// | Condition                              | Behaviour                       |
/// |----------------------------------------|---------------------------------|
/// | `ttl_secs` omitted                     | Use `BLOB_DEFAULT_TTL_SECS`     |
/// | `ttl_secs` > `BLOB_MAX_TTL_SECS`       | Clamped to `BLOB_MAX_TTL_SECS`  |
/// | `ttl_secs == 0`                        | Blob is **immediately** expired |
/// |                                        | and not retrievable after insert|
/// | Blob already past its TTL on insert    | Treated identically to zero-TTL:|
/// |                                        | the entry is accepted then      |
/// |                                        | immediately expired             |
///
/// ## Storage cleanup
///
/// When a `BlobEntry` is constructed with a non-empty `storage_path`, the
/// sweep task will call `tokio::fs::remove_file` on that path when it evicts
/// the entry.  The path is relative to the AccFS root stored inside
/// `BlobStore`; the sweep resolves the absolute path before deletion.
///
/// ## Background sweep
///
/// `BlobStore::spawn_sweep` starts a single `tokio::spawn`-ed loop that wakes
/// every `SWEEP_INTERVAL_SECS` seconds, locks the map for writing, removes
/// every entry whose `expires_at` is ≤ `Instant::now()`, and then fires
/// filesystem cleanup for each evicted entry that carried a storage path.
///
/// The sweep loop holds the write lock only long enough to drain expired
/// entries into a local `Vec`; file I/O happens after the lock is released so
/// reads are never blocked by slow disk operations.
///
/// ## Concurrency
///
/// `BlobMap` is protected by a `tokio::sync::RwLock`.  Multiple concurrent
/// `GET /api/bus/blobs/:id` requests proceed in parallel.  `POST` and sweep
/// both take an exclusive write lock; they cannot overlap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

// ── TTL constants (re-exported so routes/blobs.rs can reference them) ─────────

/// Default TTL applied when the caller omits `ttl_secs` on upload.
/// 24 hours — conservative enough for transient collaboration data.
pub const BLOB_DEFAULT_TTL_SECS: u64 = 86_400;

/// Hard cap; any caller-supplied value above this is silently clamped.
/// 7 days — matches the bus SPEC.md BLOB_MAX_TTL_SECS definition.
pub const BLOB_MAX_TTL_SECS: u64 = 604_800;

/// How often the background sweep wakes to evict expired entries.
pub const SWEEP_INTERVAL_SECS: u64 = 60;

// ── BlobEntry ─────────────────────────────────────────────────────────────────

/// A single stored blob.
#[derive(Debug)]
pub struct BlobEntry {
    /// Validated MIME type string.
    pub mime: String,
    /// Raw (already base64-decoded) payload bytes.
    pub data: Vec<u8>,
    /// Instant at which this entry becomes eligible for eviction.
    pub expires_at: Instant,
    /// Optional AccFS-relative path to the backing file.
    ///
    /// When non-empty the sweep task will attempt to delete
    /// `<fs_root>/<storage_path>` when evicting this entry.
    pub storage_path: String,
}

impl BlobEntry {
    /// Returns `true` when the entry is past its TTL.
    #[inline]
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

// ── Internal map type ─────────────────────────────────────────────────────────

type BlobMap = RwLock<HashMap<String, BlobEntry>>;

// ── BlobStore ─────────────────────────────────────────────────────────────────

/// Thread-safe, TTL-aware blob store.
///
/// Clone is O(1) — both clones share the same underlying map.
#[derive(Clone, Debug)]
pub struct BlobStore {
    inner: Arc<BlobStoreInner>,
}

#[derive(Debug)]
struct BlobStoreInner {
    map:     BlobMap,
    fs_root: String,
}

impl BlobStore {
    /// Create a new, empty store backed by AccFS at `fs_root`.
    pub fn new(fs_root: impl Into<String>) -> Self {
        BlobStore {
            inner: Arc::new(BlobStoreInner {
                map:     RwLock::new(HashMap::new()),
                fs_root: fs_root.into(),
            }),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Insert a blob, computing `expires_at` from `ttl_secs`.
    ///
    /// Rules applied (matching SPEC.md):
    /// - `None` → `BLOB_DEFAULT_TTL_SECS`
    /// - `Some(t)` where `t > BLOB_MAX_TTL_SECS` → clamped to `BLOB_MAX_TTL_SECS`
    /// - `Some(0)` → expires immediately (entry stored, immediately evictable)
    ///
    /// Already-expired entries (e.g. zero TTL) are stored and will be swept
    /// out on the next tick; `get` checks `is_expired` so they are never
    /// returned to callers.
    pub async fn insert(
        &self,
        id:           impl Into<String>,
        mime:         impl Into<String>,
        data:         Vec<u8>,
        ttl_secs:     Option<u64>,
        storage_path: impl Into<String>,
    ) {
        let ttl = Self::resolve_ttl(ttl_secs);
        let expires_at = Instant::now() + Duration::from_secs(ttl);
        let entry = BlobEntry {
            mime:         mime.into(),
            data,
            expires_at,
            storage_path: storage_path.into(),
        };
        self.inner.map.write().await.insert(id.into(), entry);
    }

    /// Retrieve a blob by id.
    ///
    /// Returns `None` if the id is unknown **or** if the entry has expired.
    /// Expired entries are left in the map for the sweep to clean up so that
    /// the read path never performs I/O under the lock.
    pub async fn get(&self, id: &str) -> Option<BlobRef> {
        let guard = self.inner.map.read().await;
        let entry = guard.get(id)?;
        if entry.is_expired() {
            return None;
        }
        // Copy out just what the HTTP handler needs — avoids holding the lock
        // while building an Axum response.
        Some(BlobRef {
            mime: entry.mime.clone(),
            data: entry.data.clone(),
        })
    }

    /// Returns `true` when an entry with the given `id` exists **and** has not
    /// yet expired.  Useful for tests that want to confirm liveness without
    /// cloning the payload.
    pub async fn contains(&self, id: &str) -> bool {
        let guard = self.inner.map.read().await;
        guard.get(id).map(|e| !e.is_expired()).unwrap_or(false)
    }

    /// Number of entries currently in the map (including expired, pre-sweep).
    /// Intended for tests and metrics; do not rely on this count for
    /// business logic.
    pub async fn len(&self) -> usize {
        self.inner.map.read().await.len()
    }

    /// Manually evict a single entry by id, triggering storage cleanup if the
    /// entry carried a `storage_path`.  Returns `true` if an entry was removed.
    ///
    /// This is exposed primarily for testing; the HTTP DELETE endpoint (if one
    /// is added later) would also call this.
    pub async fn remove(&self, id: &str) -> bool {
        let evicted = self.inner.map.write().await.remove(id);
        if let Some(entry) = evicted {
            self.cleanup_storage(&entry).await;
            true
        } else {
            false
        }
    }

    // ── Background sweep ──────────────────────────────────────────────────────

    /// Spawn the background expiry sweep loop.
    ///
    /// The loop fires every [`SWEEP_INTERVAL_SECS`] seconds.  It:
    /// 1. Acquires a write lock, drains all entries where `is_expired()`.
    /// 2. Releases the write lock immediately.
    /// 3. Calls `tokio::fs::remove_file` for every evicted entry that carried
    ///    a `storage_path`, resolving the absolute path against `fs_root`.
    ///
    /// The spawned task holds a clone of `self` (Arc clone, cheap) and runs
    /// until the process exits.  There is no shutdown channel — the task is
    /// naturally cancelled when the Tokio runtime shuts down.
    pub fn spawn_sweep(self) {
        tokio::spawn(async move {
            let interval = Duration::from_secs(SWEEP_INTERVAL_SECS);
            loop {
                tokio::time::sleep(interval).await;
                self.sweep_once().await;
            }
        });
    }

    /// Run a single expiry sweep.  Exposed as `pub` so unit tests can trigger
    /// it deterministically without waiting for the background interval.
    pub async fn sweep_once(&self) {
        // Phase 1 — collect expired entries under the write lock (fast, no I/O).
        let expired: Vec<BlobEntry> = {
            let mut map = self.inner.map.write().await;
            let keys: Vec<String> = map
                .iter()
                .filter(|(_, e)| e.is_expired())
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter().filter_map(|k| map.remove(&k)).collect()
        };

        if expired.is_empty() {
            return;
        }

        tracing::debug!(
            count = expired.len(),
            "blob_store: evicting {} expired blob(s)",
            expired.len()
        );

        // Phase 2 — delete backing files outside the lock (may be slow).
        for entry in &expired {
            self.cleanup_storage(entry).await;
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Resolve a caller-supplied TTL to the clamped, validated value.
    fn resolve_ttl(ttl_secs: Option<u64>) -> u64 {
        match ttl_secs {
            None    => BLOB_DEFAULT_TTL_SECS,
            Some(t) => t.min(BLOB_MAX_TTL_SECS),
        }
    }

    /// Delete the backing file for `entry` if it carried a non-empty
    /// `storage_path`.  Errors (including "not found") are logged at debug
    /// level and swallowed — a missing file should never abort the sweep.
    async fn cleanup_storage(&self, entry: &BlobEntry) {
        if entry.storage_path.is_empty() {
            return;
        }

        let abs: PathBuf = Path::new(&self.inner.fs_root).join(&entry.storage_path);

        match tokio::fs::remove_file(&abs).await {
            Ok(()) => {
                tracing::debug!(
                    path = %abs.display(),
                    "blob_store: deleted backing file"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File was already gone — nothing to do.
                tracing::debug!(
                    path = %abs.display(),
                    "blob_store: backing file already absent"
                );
            }
            Err(e) => {
                tracing::warn!(
                    path = %abs.display(),
                    error = %e,
                    "blob_store: failed to delete backing file"
                );
            }
        }
    }
}

// ── BlobRef ───────────────────────────────────────────────────────────────────

/// A cheaply-copyable view of a live blob, returned by [`BlobStore::get`].
///
/// Separating this from `BlobEntry` means the HTTP handler receives owned
/// data without holding any lock.
#[derive(Debug)]
pub struct BlobRef {
    pub mime: String,
    pub data: Vec<u8>,
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn store_with_tmp() -> (BlobStore, TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(tmp.path().to_string_lossy());
        (store, tmp)
    }

    async fn insert_default(store: &BlobStore, id: &str) {
        store.insert(id, "text/plain", b"hello".to_vec(), None, "").await;
    }

    // ── resolve_ttl ───────────────────────────────────────────────────────────

    #[test]
    fn ttl_none_applies_default() {
        assert_eq!(BlobStore::resolve_ttl(None), BLOB_DEFAULT_TTL_SECS);
    }

    #[test]
    fn ttl_within_max_is_unchanged() {
        assert_eq!(BlobStore::resolve_ttl(Some(3600)), 3600);
    }

    #[test]
    fn ttl_above_max_is_clamped() {
        let over = BLOB_MAX_TTL_SECS + 1;
        assert_eq!(BlobStore::resolve_ttl(Some(over)), BLOB_MAX_TTL_SECS);
    }

    #[test]
    fn ttl_exactly_max_is_unchanged() {
        assert_eq!(BlobStore::resolve_ttl(Some(BLOB_MAX_TTL_SECS)), BLOB_MAX_TTL_SECS);
    }

    #[test]
    fn ttl_zero_resolves_to_zero() {
        // Zero is a valid caller-supplied value meaning "immediate expiry".
        assert_eq!(BlobStore::resolve_ttl(Some(0)), 0);
    }

    // ── insert and get (non-expired) ──────────────────────────────────────────

    #[tokio::test]
    async fn insert_and_retrieve_live_blob() {
        let (store, _tmp) = store_with_tmp();
        store.insert("a", "image/png", vec![1, 2, 3], Some(3600), "").await;
        let b = store.get("a").await.expect("should find live blob");
        assert_eq!(b.mime, "image/png");
        assert_eq!(b.data, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn get_unknown_id_returns_none() {
        let (store, _tmp) = store_with_tmp();
        assert!(store.get("does-not-exist").await.is_none());
    }

    #[tokio::test]
    async fn contains_live_blob_returns_true() {
        let (store, _tmp) = store_with_tmp();
        insert_default(&store, "x").await;
        assert!(store.contains("x").await);
    }

    #[tokio::test]
    async fn contains_absent_id_returns_false() {
        let (store, _tmp) = store_with_tmp();
        assert!(!store.contains("ghost").await);
    }

    #[tokio::test]
    async fn len_reflects_insertions() {
        let (store, _tmp) = store_with_tmp();
        assert_eq!(store.len().await, 0);
        insert_default(&store, "a").await;
        insert_default(&store, "b").await;
        assert_eq!(store.len().await, 2);
    }

    // ── zero-TTL: already-expired on insert ───────────────────────────────────

    #[tokio::test]
    async fn zero_ttl_blob_not_retrievable() {
        // A blob inserted with TTL=0 sets expires_at = now, making it
        // immediately past its deadline.  get() must return None.
        let (store, _tmp) = store_with_tmp();
        store.insert("zero", "text/plain", b"gone".to_vec(), Some(0), "").await;
        assert!(
            store.get("zero").await.is_none(),
            "zero-TTL blob must not be retrievable"
        );
    }

    #[tokio::test]
    async fn zero_ttl_blob_occupies_map_before_sweep() {
        // The entry IS in the map (awaiting sweep) even though get() returns None.
        let (store, _tmp) = store_with_tmp();
        store.insert("z", "text/plain", b"x".to_vec(), Some(0), "").await;
        assert_eq!(
            store.len().await, 1,
            "zero-TTL entry should sit in the map until the sweep removes it"
        );
    }

    #[tokio::test]
    async fn zero_ttl_blob_removed_by_sweep() {
        let (store, _tmp) = store_with_tmp();
        store.insert("z", "text/plain", b"x".to_vec(), Some(0), "").await;
        store.sweep_once().await;
        assert_eq!(store.len().await, 0);
    }

    // ── sweep evicts only expired entries ─────────────────────────────────────

    #[tokio::test]
    async fn sweep_removes_expired_leaves_live() {
        let (store, _tmp) = store_with_tmp();

        // Long-lived blob
        store.insert("live", "text/plain", b"alive".to_vec(), Some(9999), "").await;
        // Zero-TTL blob — expires immediately
        store.insert("dead", "text/plain", b"bye".to_vec(), Some(0), "").await;

        store.sweep_once().await;

        assert!(store.contains("live").await, "live blob must survive sweep");
        assert!(!store.contains("dead").await, "expired blob must be removed");
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn sweep_on_empty_store_is_a_noop() {
        let (store, _tmp) = store_with_tmp();
        store.sweep_once().await; // must not panic
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn sweep_on_all_live_blobs_removes_nothing() {
        let (store, _tmp) = store_with_tmp();
        insert_default(&store, "a").await;
        insert_default(&store, "b").await;
        store.sweep_once().await;
        assert_eq!(store.len().await, 2);
    }

    // ── manual remove ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn remove_existing_entry_returns_true() {
        let (store, _tmp) = store_with_tmp();
        insert_default(&store, "del").await;
        assert!(store.remove("del").await);
        assert!(!store.contains("del").await);
    }

    #[tokio::test]
    async fn remove_absent_entry_returns_false() {
        let (store, _tmp) = store_with_tmp();
        assert!(!store.remove("nope").await);
    }

    // ── storage cleanup ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn sweep_deletes_backing_file_on_expiry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(tmp.path().to_string_lossy());

        // Write a backing file under the fs_root.
        let rel_path = "blobs/test-sweep-file.bin";
        let abs_path = tmp.path().join(rel_path);
        tokio::fs::create_dir_all(abs_path.parent().unwrap()).await.unwrap();
        tokio::fs::write(&abs_path, b"payload").await.unwrap();

        // Insert with zero TTL so the entry expires immediately.
        store.insert("file-blob", "application/octet-stream", b"p".to_vec(),
                     Some(0), rel_path).await;

        assert!(abs_path.exists(), "backing file must exist before sweep");

        store.sweep_once().await;

        assert!(
            !abs_path.exists(),
            "backing file must be deleted after expiry sweep"
        );
    }

    #[tokio::test]
    async fn sweep_tolerates_already_absent_backing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(tmp.path().to_string_lossy());

        // Insert with a storage_path that does NOT exist — sweep must not panic.
        store.insert("ghost-file", "application/octet-stream", b"x".to_vec(),
                     Some(0), "blobs/does-not-exist.bin").await;

        store.sweep_once().await; // must succeed without panic
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn remove_deletes_backing_file_immediately() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::new(tmp.path().to_string_lossy());

        let rel_path = "blobs/manual-remove.bin";
        let abs_path = tmp.path().join(rel_path);
        tokio::fs::create_dir_all(abs_path.parent().unwrap()).await.unwrap();
        tokio::fs::write(&abs_path, b"data").await.unwrap();

        store.insert("m", "application/octet-stream", b"d".to_vec(),
                     Some(9999), rel_path).await;

        store.remove("m").await;

        assert!(!abs_path.exists(), "backing file must be deleted on manual remove");
    }

    #[tokio::test]
    async fn blob_without_storage_path_sweeps_cleanly() {
        // No-path blob: sweep should evict the entry without any filesystem ops.
        let (store, _tmp) = store_with_tmp();
        store.insert("inline", "text/plain", b"hi".to_vec(), Some(0), "").await;
        store.sweep_once().await;
        assert_eq!(store.len().await, 0);
    }

    // ── TTL clamping: max boundary ────────────────────────────────────────────

    #[tokio::test]
    async fn oversized_ttl_blob_not_expired_immediately() {
        // A blob submitted with ttl_secs=u64::MAX must be clamped to
        // BLOB_MAX_TTL_SECS — it should still be alive right after insertion.
        let (store, _tmp) = store_with_tmp();
        store.insert("big", "text/plain", b"hi".to_vec(), Some(u64::MAX), "").await;
        assert!(
            store.contains("big").await,
            "oversized-TTL blob must be alive immediately after insertion"
        );
    }

    // ── Arc clone semantics: two handles share the same map ──────────────────

    #[tokio::test]
    async fn cloned_store_shares_state() {
        let (store, _tmp) = store_with_tmp();
        let store2 = store.clone();

        store.insert("shared", "text/plain", b"x".to_vec(), None, "").await;

        assert!(
            store2.contains("shared").await,
            "clone must see entries inserted via original handle"
        );
    }

    // ── Concurrent reads during sweep ────────────────────────────────────────

    #[tokio::test]
    async fn concurrent_reads_while_sweep_runs() {
        // Spawn many read tasks, trigger a sweep concurrently, and ensure
        // no panics and no stale results leak through.
        let (store, _tmp) = store_with_tmp();

        // Populate: 50 long-lived + 50 zero-TTL
        for i in 0u32..50 {
            store.insert(format!("live-{i}"),  "text/plain", vec![i as u8], Some(9999), "").await;
            store.insert(format!("dead-{i}"),  "text/plain", vec![i as u8], Some(0), "").await;
        }

        // Fan out 100 concurrent read tasks.
        let mut handles = Vec::new();
        for i in 0u32..100 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                // Accessing a zero-TTL key must return None (not panic).
                let _ = s.get(&format!("dead-{}", i % 50)).await;
                // Accessing a live key must return Some.
                let b = s.get(&format!("live-{}", i % 50)).await;
                assert!(b.is_some(), "live blob must be readable under concurrent sweep");
            }));
        }

        // Sweep runs concurrently with the reads above.
        let s = store.clone();
        let sweep_handle = tokio::spawn(async move { s.sweep_once().await });

        // Await everything.
        for h in handles { h.await.expect("read task panicked"); }
        sweep_handle.await.expect("sweep panicked");

        // After sweep: all zero-TTL entries gone, all live entries still present.
        for i in 0u32..50 {
            assert!(!store.contains(&format!("dead-{i}")).await);
            assert!(store.contains(&format!("live-{i}")).await);
        }
    }

    // ── Entry is_expired helper ───────────────────────────────────────────────

    #[test]
    fn blob_entry_is_expired_when_past_deadline() {
        let entry = BlobEntry {
            mime:         "text/plain".to_string(),
            data:         vec![],
            expires_at:   Instant::now() - Duration::from_secs(1),
            storage_path: String::new(),
        };
        assert!(entry.is_expired());
    }

    #[test]
    fn blob_entry_is_not_expired_when_before_deadline() {
        let entry = BlobEntry {
            mime:         "text/plain".to_string(),
            data:         vec![],
            expires_at:   Instant::now() + Duration::from_secs(3600),
            storage_path: String::new(),
        };
        assert!(!entry.is_expired());
    }
}
