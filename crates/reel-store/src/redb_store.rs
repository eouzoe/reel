//! Persistent backend — [`RedbStore`].
//!
//! `RedbStore` wraps a [`redb`] 4.x database file with a four-table
//! layout per ADR-0017:
//!
//! - `blocks`: `Hash` (32-byte BLAKE3) → CBOR-encoded [`Block`].
//! - `refs`: `name` (UTF-8 string) → CBOR-encoded [`reel_spec::Ref`].
//! - `meta`: short metadata strings (`schema_version`, recovery flag).
//!
//! The pre-ADR-0024 `parent_idx` table is dropped because Blocks have
//! no protocol-level parent under ADR-0025. ADR-0017's table list is
//! superseded on that single point.
//!
//! All values use ciborium-flavoured CBOR (RFC 8949) per ADR-0017.
//!
//! # I-001 enforcement
//!
//! Identical to [`crate::MemStore`]:
//!
//! - `lookup_by_hash` first checks storage presence (E-001 if absent)
//!   then runs the [`crate::ReachabilityWalker`] over the supplied
//!   [`crate::Namespace`] (E-010 if unreachable).
//! - `put_ref` rejects dangling hashes with E-001
//!   (`reel_spec::ReelError::BlockNotFound`).
//! - `gc` deletes every Block not reachable from the supplied
//!   namespace.
//!
//! # Caching
//!
//! `RedbStore` keeps a [`moka::sync::Cache`] of the most-recently-read
//! Block payloads to avoid the redb open-table round-trip on every
//! lookup. The cache is invalidated by `put_ref` (which may alter the
//! reachability closure) and by `gc` (which deletes Blocks).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use moka::sync::Cache;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use reel_spec::{Block, Hash, Ref, VersionRef};

use crate::cbor;
use crate::error::StoreError;
use crate::reachability::{ReachabilityWalker, namespace_roots};
use crate::store_trait::{Namespace, Store};

const BLOCKS: TableDefinition<'static, &'static [u8], &'static [u8]> =
    TableDefinition::new("reel_blocks_v1");
const REFS: TableDefinition<'static, &'static str, &'static [u8]> =
    TableDefinition::new("reel_refs_v1");
const META: TableDefinition<'static, &'static str, &'static [u8]> =
    TableDefinition::new("reel_meta_v1");

const META_SCHEMA_VERSION: &str = "schema_version";
const SCHEMA_VERSION_V1: &[u8] = b"reel-store/v1";

/// Synchronisation mode for the underlying redb instance.
///
/// At reel v0.1 redb's default durability semantics already match the
/// kernel's requirements; the enum is preserved so the config surface
/// is stable as `Config` grows.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub enum SyncMode {
    /// Default: redb's normal durability (one fsync per commit).
    #[default]
    Default,
    /// Periodic durability — for benchmarking and DST only.
    ///
    /// Not currently distinguished from [`SyncMode::Default`] at the
    /// API surface; reserved for future use without breaking callers.
    Periodic,
}

/// Per-store configuration.
///
/// `Config` is `#[non_exhaustive]`; construct via [`Config::default`] and
/// mutate fields in place.
///
/// # Examples
///
/// ```rust
/// use reel_store::Config;
/// let mut c = Config::default();
/// c.create_if_missing = true;
/// c.block_cache_entries = 8_192;
/// assert!(c.create_if_missing);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    /// Create the store file if it does not exist.
    pub create_if_missing: bool,
    /// Maximum number of Block payloads to keep in the in-memory cache.
    pub block_cache_entries: u64,
    /// fsync policy.
    pub sync_mode: SyncMode,
}

impl Default for Config {
    fn default() -> Self {
        Self { create_if_missing: true, block_cache_entries: 1024, sync_mode: SyncMode::Default }
    }
}

/// Summary of a `gc` pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcStats {
    /// Blocks retained because they were reachable from the supplied namespace.
    pub kept: u64,
    /// Blocks deleted because they were unreachable.
    pub deleted: u64,
}

/// Persistent storage backend.
///
/// `RedbStore` opens (or creates) a single redb file. Multiple
/// `RedbStore` handles to the **same** file from the **same** process
/// share the underlying [`redb::Database`] via [`std::sync::Arc`]
/// semantics; cross-process access is not supported at v0.1 — redb's
/// lock file enforces single-process opening.
///
/// # Examples
///
/// ```rust
/// use reel_store::{Config, RedbStore, Store};
/// use reel_spec::{Block};
///
/// let tmp = tempfile::tempdir().expect("tmp");
/// let store_path = tmp.path().join("reel.redb");
/// let store = RedbStore::open(&store_path, &Config::default()).expect("open");
///
/// let block = Block::new(b"x".to_vec()).expect("ok");
/// store.write_block(&block).expect("write");
/// let got = store.raw_lookup_by_hash(&block.hash()).expect("read");
/// assert_eq!(got.hash(), block.hash());
/// ```
#[derive(Debug)]
pub struct RedbStore {
    db: Database,
    cache: Cache<Hash, Vec<u8>>,
    path: PathBuf,
    was_recovered: AtomicBool,
    // Used to single-thread `gc` so two threads cannot delete the
    // same Block from under each other; lookup / write are serialised
    // by redb's transactional model.
    gc_lock: Mutex<()>,
}

impl RedbStore {
    /// Open (or create) a redb store at `path`.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Engine`] for any redb-layer failure.
    /// - [`StoreError::Config`] when `config.create_if_missing` is
    ///   `false` and the file does not exist.
    /// - [`StoreError::Io`] when the parent directory cannot be created.
    pub fn open(path: impl AsRef<Path>, config: &Config) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }

        let exists = path.exists();
        if !exists && !config.create_if_missing {
            return Err(StoreError::Config(format!(
                "store at {} does not exist and create_if_missing=false",
                path.display()
            )));
        }

        let db = Database::create(&path).map_err(|e| StoreError::Engine(e.to_string()))?;

        // Ensure tables exist by opening them once inside a write tx.
        // redb creates tables lazily on first open_table.
        let had_prior_schema = {
            let tx = db.begin_write().map_err(|e| StoreError::Engine(e.to_string()))?;
            let had_prior_schema = {
                drop(tx.open_table(BLOCKS).map_err(|e| StoreError::Engine(e.to_string()))?);
                drop(tx.open_table(REFS).map_err(|e| StoreError::Engine(e.to_string()))?);
                let mut meta_t =
                    tx.open_table(META).map_err(|e| StoreError::Engine(e.to_string()))?;

                // Detect "was_recovered" heuristically: a fresh file has no
                // schema_version row; pre-existing files do.
                let key: &str = META_SCHEMA_VERSION;
                let was = meta_t.get(key).map_err(|e| StoreError::Engine(e.to_string()))?.is_some();
                if !was {
                    meta_t
                        .insert(key, SCHEMA_VERSION_V1)
                        .map_err(|e| StoreError::Engine(e.to_string()))?;
                }
                was
            };
            tx.commit().map_err(|e| StoreError::Engine(e.to_string()))?;
            had_prior_schema
        };
        let was_recovered = exists && had_prior_schema;

        let cache = Cache::new(config.block_cache_entries);

        Ok(Self {
            db,
            cache,
            path,
            was_recovered: AtomicBool::new(was_recovered),
            gc_lock: Mutex::new(()),
        })
    }

    /// Path to the underlying redb file (for diagnostics).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Garbage-collect Blocks that are not reachable from `root`.
    ///
    /// Deletes every Block whose `Hash` is not in the transitive
    /// closure of `root` (computed by [`ReachabilityWalker`]). Returns
    /// a [`GcStats`] reporting kept and deleted counts.
    ///
    /// The cache is invalidated on every successful `gc`.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Engine`] for redb failures.
    /// - [`StoreError::Codec`] when an existing Block payload fails to
    ///   decode (suggests on-disk corruption).
    pub fn gc(&self, root: &Namespace) -> Result<GcStats, StoreError> {
        // Single-thread GC so two callers cannot race the visited-set
        // computation with each other.
        let _lock = self
            .gc_lock
            .lock()
            .map_err(|e| StoreError::Engine(format!("gc lock poisoned: {e}")))?;

        let walker = ReachabilityWalker::default();
        let roots = namespace_roots(root);
        // The walker reads block payloads via a fresh read tx.
        let reachable = walker.walk(&roots, |h| self.raw_get_payload(h))?;

        let mut deleted: u64 = 0;
        let mut kept: u64 = 0;

        let tx = self.db.begin_write().map_err(|e| StoreError::Engine(e.to_string()))?;
        {
            let mut blocks_t =
                tx.open_table(BLOCKS).map_err(|e| StoreError::Engine(e.to_string()))?;

            // Collect first to avoid concurrent mutation of the iterator.
            let mut to_delete: Vec<[u8; 32]> = Vec::new();
            for kv in blocks_t.iter().map_err(|e| StoreError::Engine(e.to_string()))? {
                let (k, _v) = kv.map_err(|e| StoreError::Engine(e.to_string()))?;
                let hash_bytes: &[u8] = k.value();
                let arr: [u8; 32] = hash_bytes
                    .try_into()
                    .map_err(|_| StoreError::Engine("blocks key not 32 bytes".to_owned()))?;
                let hash = Hash::from_bytes(arr);
                if reachable.contains(&hash) {
                    kept = kept.saturating_add(1);
                } else {
                    to_delete.push(arr);
                }
            }

            for key in &to_delete {
                let key_slice: &[u8] = key.as_slice();
                drop(blocks_t.remove(key_slice).map_err(|e| StoreError::Engine(e.to_string()))?);
                deleted = deleted.saturating_add(1);
            }
        }
        tx.commit().map_err(|e| StoreError::Engine(e.to_string()))?;

        // GC may have deleted Blocks present in the cache.
        self.cache.invalidate_all();

        Ok(GcStats { kept, deleted })
    }

    fn raw_get_payload(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StoreError> {
        if let Some(cached) = self.cache.get(hash) {
            return Ok(Some(cached));
        }
        let tx = self.db.begin_read().map_err(|e| StoreError::Engine(e.to_string()))?;
        let t = tx.open_table(BLOCKS).map_err(|e| StoreError::Engine(e.to_string()))?;
        let key: &[u8] = hash.as_bytes();
        let g = t.get(key).map_err(|e| StoreError::Engine(e.to_string()))?;
        g.map_or(Ok(None), |v| {
            let bytes = v.value().to_vec();
            self.cache.insert(*hash, bytes.clone());
            Ok(Some(bytes))
        })
    }

    fn block_exists(&self, hash: &Hash) -> Result<bool, StoreError> {
        if self.cache.get(hash).is_some() {
            return Ok(true);
        }
        let tx = self.db.begin_read().map_err(|e| StoreError::Engine(e.to_string()))?;
        let t = tx.open_table(BLOCKS).map_err(|e| StoreError::Engine(e.to_string()))?;
        let key: &[u8] = hash.as_bytes();
        let g = t.get(key).map_err(|e| StoreError::Engine(e.to_string()))?;
        Ok(g.is_some())
    }
}

impl Store for RedbStore {
    fn write_block(&self, block: &Block) -> Result<(), StoreError> {
        let encoded = cbor::encode(block)?;
        let tx = self.db.begin_write().map_err(|e| StoreError::Engine(e.to_string()))?;
        {
            let mut t = tx.open_table(BLOCKS).map_err(|e| StoreError::Engine(e.to_string()))?;
            let key: &[u8] = block.hash.as_bytes();
            // Dedupe: skip the insert if the key already exists.  redb's
            // insert overwrites, so we honour content-addressed dedupe
            // by checking presence first.  The cost is one extra B-tree
            // lookup per write; acceptable at v0.1.
            if t.get(key).map_err(|e| StoreError::Engine(e.to_string()))?.is_none() {
                let value: &[u8] = encoded.as_slice();
                t.insert(key, value).map_err(|e| StoreError::Engine(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| StoreError::Engine(e.to_string()))?;
        // Populate the read cache so the next lookup is hot.
        self.cache.insert(block.hash, encoded);
        Ok(())
    }

    fn lookup_by_hash(&self, hash: &Hash, root: &Namespace) -> Result<Block, StoreError> {
        // Step 1: presence in storage.
        let Some(encoded) = self.raw_get_payload(hash)? else {
            return Err(StoreError::block_not_found(*hash));
        };
        // Step 2: I-001 reachability gate.
        let walker = ReachabilityWalker::default();
        let roots = namespace_roots(root);
        let closure = walker.walk(&roots, |h| self.raw_get_payload(h))?;
        if !closure.contains(hash) {
            return Err(StoreError::reachability_violation(*hash));
        }
        // Step 3: decode envelope.
        let block: Block = cbor::decode(&encoded)?;
        Ok(block)
    }

    fn raw_lookup_by_hash(&self, hash: &Hash) -> Result<Block, StoreError> {
        let Some(encoded) = self.raw_get_payload(hash)? else {
            return Err(StoreError::block_not_found(*hash));
        };
        let block: Block = cbor::decode(&encoded)?;
        Ok(block)
    }

    fn put_ref(
        &self,
        name: &str,
        hash: &Hash,
        version_constraint: Option<VersionRef>,
    ) -> Result<(), StoreError> {
        if !self.block_exists(hash)? {
            return Err(StoreError::block_not_found(*hash));
        }

        let r = match version_constraint {
            Some(vr) => Ref::with_version_constraint(name, *hash, vr)?,
            None => Ref::new(name, *hash)?,
        };
        let encoded = cbor::encode(&r)?;
        let tx = self.db.begin_write().map_err(|e| StoreError::Engine(e.to_string()))?;
        {
            let mut t = tx.open_table(REFS).map_err(|e| StoreError::Engine(e.to_string()))?;
            let key: &str = name;
            let value: &[u8] = encoded.as_slice();
            t.insert(key, value).map_err(|e| StoreError::Engine(e.to_string()))?;
        }
        tx.commit().map_err(|e| StoreError::Engine(e.to_string()))?;
        // put_ref may have made previously-unreachable Blocks reachable
        // or vice versa; invalidate the cache for safety.  (write_block
        // populates the cache; that's safe — we only cache raw bytes.)
        // We DON'T need to drop the block cache because raw payloads
        // are content-addressed; reachability is recomputed on every
        // lookup.
        let _ = name;
        Ok(())
    }

    fn get_ref(&self, name: &str) -> Result<Option<Ref>, StoreError> {
        let tx = self.db.begin_read().map_err(|e| StoreError::Engine(e.to_string()))?;
        let t = tx.open_table(REFS).map_err(|e| StoreError::Engine(e.to_string()))?;
        let key: &str = name;
        match t.get(key).map_err(|e| StoreError::Engine(e.to_string()))? {
            None => Ok(None),
            Some(v) => {
                let bytes = v.value().to_vec();
                let r: Ref = cbor::decode(&bytes)?;
                Ok(Some(r))
            }
        }
    }

    fn remove_ref(&self, name: &str) -> Result<bool, StoreError> {
        let tx = self.db.begin_write().map_err(|e| StoreError::Engine(e.to_string()))?;
        let removed = {
            let mut t = tx.open_table(REFS).map_err(|e| StoreError::Engine(e.to_string()))?;
            let key: &str = name;
            t.remove(key).map_err(|e| StoreError::Engine(e.to_string()))?.is_some()
        };
        tx.commit().map_err(|e| StoreError::Engine(e.to_string()))?;
        Ok(removed)
    }

    fn list_refs(&self) -> Result<Vec<Ref>, StoreError> {
        let tx = self.db.begin_read().map_err(|e| StoreError::Engine(e.to_string()))?;
        let t = tx.open_table(REFS).map_err(|e| StoreError::Engine(e.to_string()))?;
        let mut out: Vec<Ref> = Vec::new();
        // redb iterates keys in sorted order by default.
        for kv in t.iter().map_err(|e| StoreError::Engine(e.to_string()))? {
            let (_k, v) = kv.map_err(|e| StoreError::Engine(e.to_string()))?;
            let bytes = v.value().to_vec();
            let r: Ref = cbor::decode(&bytes)?;
            out.push(r);
        }
        // Sort by Ref::name to be defensive against unexpected ordering.
        out.sort();
        Ok(out)
    }

    fn was_recovered(&self) -> bool {
        self.was_recovered.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test module: panics are acceptable for test failures"
)]
mod tests {
    use super::*;
    use reel_spec::{ReelError};
    use tempfile::tempdir;

    fn fresh_store() -> (tempfile::TempDir, RedbStore) {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("reel.redb");
        let store = RedbStore::open(&path, &Config::default()).expect("open fresh");
        (dir, store)
    }

    fn block(payload: &[u8]) -> Block {
        Block::new(payload.to_vec()).expect("small")
    }

    #[test]
    fn open_creates_file() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("reel.redb");
        assert!(!path.exists(), "file does not exist yet");
        let _s = RedbStore::open(&path, &Config::default()).expect("open");
        assert!(path.exists(), "open(create_if_missing=true) creates the file");
    }

    #[test]
    fn open_refuses_missing_when_create_disabled() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("absent.redb");
        let cfg = Config { create_if_missing: false, ..Config::default() };
        let err = RedbStore::open(&path, &cfg).expect_err("must refuse");
        assert!(matches!(err, StoreError::Config(_)), "got {err:?}");
    }

    #[test]
    fn write_then_raw_lookup_round_trip() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"hello redb");
        s.write_block(&b)?;
        let got = s.raw_lookup_by_hash(&b.hash())?;
        assert_eq!(got.hash(), b.hash());
        assert_eq!(got.data, b.data);
        Ok(())
    }

    #[test]
    fn write_dedupes_identical_block() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"x");
        s.write_block(&b)?;
        s.write_block(&b)?;
        // No direct count API on RedbStore for blocks; verify dedupe
        // indirectly by listing through gc.
        let mut ns = Namespace::default();
        ns.insert("k".into(), b.hash());
        s.put_ref("k", &b.hash(), None)?;
        let stats = s.gc(&ns)?;
        assert_eq!(stats.kept, 1, "exactly one block kept");
        assert_eq!(stats.deleted, 0);
        Ok(())
    }

    #[test]
    fn lookup_unreachable_block_returns_e010() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"orphan");
        s.write_block(&b)?;
        let ns = Namespace::default();
        let err = s.lookup_by_hash(&b.hash(), &ns).expect_err("must reject");
        assert!(
            matches!(err, StoreError::Protocol(ReelError::ReachabilityViolation(_))),
            "expected E-010, got {err:?}"
        );
        Ok(())
    }

    #[test]
    fn lookup_missing_block_returns_e001() {
        let (_dir, s) = fresh_store();
        let h = Hash::from_bytes([0xff; 32]);
        let mut ns = Namespace::default();
        ns.insert("k".into(), h);
        let err = s.lookup_by_hash(&h, &ns).expect_err("must reject");
        assert!(
            matches!(err, StoreError::Protocol(ReelError::BlockNotFound(_))),
            "expected E-001, got {err:?}"
        );
    }

    #[test]
    fn lookup_reachable_block_succeeds() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"reachable");
        s.write_block(&b)?;
        s.put_ref("main", &b.hash(), None)?;
        let mut ns = Namespace::default();
        ns.insert("main".into(), b.hash());
        let got = s.lookup_by_hash(&b.hash(), &ns)?;
        assert_eq!(got.hash(), b.hash());
        Ok(())
    }

    #[test]
    fn put_ref_rejects_dangling_hash() {
        let (_dir, s) = fresh_store();
        let h = Hash::from_bytes([0xab; 32]); // never written
        let err = s.put_ref("dangling", &h, None).expect_err("dangling rejected");
        assert!(matches!(err, StoreError::Protocol(ReelError::BlockNotFound(_))), "got {err:?}");
    }

    #[test]
    fn put_ref_replaces_existing_ref() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b1 = block(b"one");
        let b2 = block(b"two");
        s.write_block(&b1)?;
        s.write_block(&b2)?;
        s.put_ref("main", &b1.hash(), None)?;
        s.put_ref("main", &b2.hash(), None)?;
        let got = s.get_ref("main")?.expect("present");
        assert_eq!(got.hash, b2.hash());
        Ok(())
    }

    #[test]
    fn remove_ref_reports_existence() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"x");
        s.write_block(&b)?;
        s.put_ref("main", &b.hash(), None)?;
        assert!(s.remove_ref("main")?);
        assert!(!s.remove_ref("main")?);
        Ok(())
    }

    #[test]
    fn list_refs_returns_name_sorted() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b = block(b"x");
        s.write_block(&b)?;
        for name in ["zeta", "alpha", "mu", "beta"] {
            s.put_ref(name, &b.hash(), None)?;
        }
        let listed = s.list_refs()?;
        let names: Vec<&str> = listed.iter().map(reel_spec::Ref::name).collect();
        assert_eq!(names, vec!["alpha", "beta", "mu", "zeta"]);
        Ok(())
    }

    #[test]
    fn gc_keeps_reachable_deletes_orphaned() -> Result<(), StoreError> {
        let (_dir, s) = fresh_store();
        let b_keep = block(b"keep");
        let b_drop = block(b"drop");
        s.write_block(&b_keep)?;
        s.write_block(&b_drop)?;
        s.put_ref("main", &b_keep.hash(), None)?;
        let mut ns = Namespace::default();
        ns.insert("main".into(), b_keep.hash());

        let stats = s.gc(&ns)?;
        assert_eq!(stats.kept, 1);
        assert_eq!(stats.deleted, 1);

        // After gc, raw_lookup of the dropped block must return E-001.
        let err = s.raw_lookup_by_hash(&b_drop.hash()).expect_err("dropped block gone");
        assert!(matches!(err, StoreError::Protocol(ReelError::BlockNotFound(_))), "got {err:?}");
        // And the kept block still resolves through the namespace.
        let got = s.lookup_by_hash(&b_keep.hash(), &ns)?;
        assert_eq!(got.hash(), b_keep.hash());
        Ok(())
    }

    #[test]
    fn persistence_across_open() -> Result<(), StoreError> {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("persisted.redb");
        let b = block(b"persisted");

        {
            let s = RedbStore::open(&path, &Config::default())?;
            s.write_block(&b)?;
            s.put_ref("main", &b.hash(), None)?;
        }

        // Drop the first handle and re-open.  Data must survive.
        let s2 = RedbStore::open(&path, &Config::default())?;
        assert!(s2.was_recovered(), "reopen of existing file reports recovered=true");
        let got = s2.get_ref("main")?.expect("ref persisted");
        assert_eq!(got.hash, b.hash());
        let blk = s2.raw_lookup_by_hash(&b.hash())?;
        assert_eq!(blk.hash(), b.hash());
        Ok(())
    }

    #[test]
    fn was_recovered_false_on_fresh_open() {
        let (_dir, s) = fresh_store();
        assert!(!s.was_recovered(), "fresh open is not a recovery");
    }

    #[test]
    fn i_001_reachability_conformance_drop_ref_then_lookup() -> Result<(), StoreError> {
        // Mirrors spec/conformance/i_001_reachability.json: write a block,
        // create a ref to it, remove the ref, gc, then lookup_by_hash =>
        // E-010 ReachabilityViolation.
        let (_dir, s) = fresh_store();
        let b = block(b"hello");
        s.write_block(&b)?;
        s.put_ref("/probe", &b.hash(), None)?;

        // Reachable while ref exists.
        let mut ns = Namespace::default();
        ns.insert("/probe".into(), b.hash());
        assert!(s.lookup_by_hash(&b.hash(), &ns).is_ok());

        // Remove the ref + gc; the Block is now unreachable + deleted.
        s.remove_ref("/probe")?;
        let empty_ns = Namespace::default();
        let stats = s.gc(&empty_ns)?;
        assert_eq!(stats.deleted, 1, "the unreferenced block must be deleted");

        let err =
            s.lookup_by_hash(&b.hash(), &empty_ns).expect_err("dropped + gc'd block must error");
        // Post-gc the block is physically absent → E-001; before gc it
        // would have been E-010 (storage-present but unreachable).
        assert!(
            matches!(
                err,
                StoreError::Protocol(
                    ReelError::BlockNotFound(_) | ReelError::ReachabilityViolation(_)
                )
            ),
            "expected E-001 or E-010, got {err:?}"
        );
        Ok(())
    }
}
