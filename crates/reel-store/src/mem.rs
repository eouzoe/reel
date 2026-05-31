//! In-memory backend — [`MemStore`].
//!
//! `MemStore` is a fully-functional [`crate::Store`] implementation
//! backed by [`std::collections::BTreeMap`]s under a
//! [`std::sync::RwLock`]. It is used by:
//!
//! - The walking-skeleton tests (no filesystem dependency).
//! - The `reel-conformance-runner` (per-test ephemeral store).
//! - Property tests where a redb-backed store would dominate runtime.
//!
//! Both `MemStore` and [`crate::RedbStore`] satisfy [`crate::Store`]
//! and provide identical semantics for I-001 reachability and Ref
//! lifecycle.

use std::collections::BTreeMap;
use std::sync::RwLock;

use reel_spec::{Block, Hash, Ref, VersionRef};

use crate::cbor;
use crate::error::StoreError;
use crate::reachability::{ReachabilityWalker, namespace_roots};
use crate::store_trait::{Namespace, Store};

/// In-memory storage backend.
///
/// `MemStore` is `Send + Sync` and cheaply cloneable via shared
/// references — wrap it in `Arc<MemStore>` for sharing across threads.
///
/// All operations are constant-time relative to map size (`BTreeMap`'s
/// `O(log n)` cost). `lookup_by_hash`'s walker is `O(N · d)` where
/// `N` is the size of the namespace closure.
///
/// # Examples
///
/// ```rust
/// use reel_store::{MemStore, Namespace, Store};
/// use reel_spec::{Block};
///
/// let store = MemStore::new();
/// let block = Block::new(b"hello".to_vec()).expect("ok");
/// store.write_block(&block).expect("write");
///
/// let mut ns = Namespace::default();
/// ns.insert("k".into(), block.hash());
/// store.put_ref("k", &block.hash(), None).expect("put_ref");
///
/// assert!(store.lookup_by_hash(&block.hash(), &ns).is_ok());
/// ```
#[derive(Debug, Default)]
pub struct MemStore {
    blocks: RwLock<BTreeMap<Hash, Vec<u8>>>,
    refs: RwLock<BTreeMap<String, Ref>>,
}

impl MemStore {
    /// Construct an empty `MemStore`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_store::MemStore;
    /// let s = MemStore::new();
    /// drop(s);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of Blocks currently stored.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_store::MemStore;
    /// let s = MemStore::new();
    /// assert_eq!(s.block_count(), 0);
    /// ```
    pub fn block_count(&self) -> usize {
        self.blocks.read().map(|g| g.len()).unwrap_or_default()
    }

    /// Number of Refs currently stored.
    pub fn ref_count(&self) -> usize {
        self.refs.read().map(|g| g.len()).unwrap_or_default()
    }

    fn raw_get_payload(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StoreError> {
        let guard = self
            .blocks
            .read()
            .map_err(|e| StoreError::Engine(format!("MemStore blocks read lock poisoned: {e}")))?;
        Ok(guard.get(hash).cloned())
    }
}

impl Store for MemStore {
    fn write_block(&self, block: &Block) -> Result<(), StoreError> {
        let encoded = cbor::encode(block)?;
        let hash = block.hash();
        {
            let mut guard = self.blocks.write().map_err(|e| {
                StoreError::Engine(format!("MemStore blocks write lock poisoned: {e}"))
            })?;
            // Deduplicate: identical content is a no-op.
            guard.entry(hash).or_insert(encoded);
        }
        Ok(())
    }

    fn lookup_by_hash(&self, hash: &Hash, root: &Namespace) -> Result<Block, StoreError> {
        // Step 1: presence in storage (release the lock before walking).
        let encoded = {
            let blocks_guard = self.blocks.read().map_err(|e| {
                StoreError::Engine(format!("MemStore blocks read lock poisoned: {e}"))
            })?;
            match blocks_guard.get(hash) {
                Some(bytes) => bytes.clone(),
                None => return Err(StoreError::block_not_found(*hash)),
            }
        };

        // Step 2: I-001 reachability gate.
        let walker = ReachabilityWalker::default();
        let roots = namespace_roots(root);
        let closure = walker.walk(&roots, |h| self.raw_get_payload(h))?;
        if !closure.contains(hash) {
            return Err(StoreError::reachability_violation(*hash));
        }

        // Step 3: decode the Block envelope.
        let block: Block = cbor::decode(&encoded)?;
        Ok(block)
    }

    fn raw_lookup_by_hash(&self, hash: &Hash) -> Result<Block, StoreError> {
        let encoded = {
            let guard = self.blocks.read().map_err(|e| {
                StoreError::Engine(format!("MemStore blocks read lock poisoned: {e}"))
            })?;
            match guard.get(hash) {
                Some(bytes) => bytes.clone(),
                None => return Err(StoreError::block_not_found(*hash)),
            }
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
        // I-001 enforcement: the named Block MUST already exist in storage.
        let present = {
            let blocks_guard = self.blocks.read().map_err(|e| {
                StoreError::Engine(format!("MemStore blocks read lock poisoned: {e}"))
            })?;
            blocks_guard.contains_key(hash)
        };
        if !present {
            return Err(StoreError::block_not_found(*hash));
        }

        let r = match version_constraint {
            Some(vr) => Ref::with_version_constraint(name, *hash, vr)?,
            None => Ref::new(name, *hash)?,
        };
        {
            let mut refs_guard = self.refs.write().map_err(|e| {
                StoreError::Engine(format!("MemStore refs write lock poisoned: {e}"))
            })?;
            refs_guard.insert(name.to_owned(), r);
        }
        Ok(())
    }

    fn get_ref(&self, name: &str) -> Result<Option<Ref>, StoreError> {
        let guard = self
            .refs
            .read()
            .map_err(|e| StoreError::Engine(format!("MemStore refs read lock poisoned: {e}")))?;
        Ok(guard.get(name).cloned())
    }

    fn remove_ref(&self, name: &str) -> Result<bool, StoreError> {
        let mut guard = self
            .refs
            .write()
            .map_err(|e| StoreError::Engine(format!("MemStore refs write lock poisoned: {e}")))?;
        Ok(guard.remove(name).is_some())
    }

    fn list_refs(&self) -> Result<Vec<Ref>, StoreError> {
        let guard = self
            .refs
            .read()
            .map_err(|e| StoreError::Engine(format!("MemStore refs read lock poisoned: {e}")))?;
        Ok(guard.values().cloned().collect())
    }

    fn was_recovered(&self) -> bool {
        // Memory backend has no persistence; recovery is meaningless.
        false
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

    fn small_block(payload: &[u8]) -> Block {
        Block::new(payload.to_vec()).expect("small payload")
    }

    #[test]
    fn write_then_raw_lookup_round_trips() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b = small_block(b"hello");
        s.write_block(&b)?;
        let got = s.raw_lookup_by_hash(&b.hash())?;
        assert_eq!(got.hash(), b.hash());
        assert_eq!(got.data, b.data);
        Ok(())
    }

    #[test]
    fn write_dedupes_identical_block() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b = small_block(b"x");
        s.write_block(&b)?;
        s.write_block(&b)?;
        assert_eq!(s.block_count(), 1, "identical Block must dedupe");
        Ok(())
    }

    #[test]
    fn lookup_unreachable_block_returns_e010() -> Result<(), StoreError> {
        // Write a Block but never name it; lookup_by_hash with empty
        // namespace must return E-010 ReachabilityViolation.
        let s = MemStore::new();
        let b = small_block(b"orphan");
        s.write_block(&b)?;

        let ns = Namespace::default();
        let err = s.lookup_by_hash(&b.hash(), &ns).expect_err("must reject unreachable");
        if let StoreError::Protocol(ReelError::ReachabilityViolation(h)) = &err {
            assert_eq!(*h, b.hash(), "E-010 carries the offending hash");
        } else {
            panic!("expected E-010, got {err:?}");
        }
        Ok(())
    }

    #[test]
    fn lookup_missing_block_returns_e001() {
        let s = MemStore::new();
        let h = Hash::from_bytes([0xff; 32]);
        let mut ns = Namespace::default();
        ns.insert("k".into(), h);

        let err = s.lookup_by_hash(&h, &ns).expect_err("must reject missing");
        if let StoreError::Protocol(ReelError::BlockNotFound(got)) = &err {
            assert_eq!(*got, h, "E-001 carries the offending hash");
        } else {
            panic!("expected E-001, got {err:?}");
        }
    }

    #[test]
    fn lookup_reachable_block_succeeds() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b = small_block(b"reachable");
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
        let s = MemStore::new();
        let h = Hash::from_bytes([0x42; 32]); // never written
        let err = s.put_ref("dangling", &h, None).expect_err("dangling hash rejected");
        if let StoreError::Protocol(ReelError::BlockNotFound(got)) = &err {
            assert_eq!(*got, h, "E-001 carries the dangling hash");
        } else {
            panic!("expected BlockNotFound, got {err:?}");
        }
    }

    #[test]
    fn put_ref_replaces_existing_ref() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b1 = small_block(b"one");
        let b2 = small_block(b"two");
        s.write_block(&b1)?;
        s.write_block(&b2)?;

        s.put_ref("main", &b1.hash(), None)?;
        s.put_ref("main", &b2.hash(), None)?;

        let got = s.get_ref("main")?.expect("ref present");
        assert_eq!(got.hash, b2.hash(), "later put_ref must replace");
        Ok(())
    }

    #[test]
    fn get_ref_absent_returns_none() -> Result<(), StoreError> {
        let s = MemStore::new();
        let got = s.get_ref("nothing")?;
        assert!(got.is_none());
        Ok(())
    }

    #[test]
    fn remove_ref_reports_existence() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b = small_block(b"x");
        s.write_block(&b)?;
        s.put_ref("main", &b.hash(), None)?;
        assert!(s.remove_ref("main")?, "first remove must report true");
        assert!(!s.remove_ref("main")?, "second remove must report false");
        Ok(())
    }

    #[test]
    fn list_refs_returns_name_sorted() -> Result<(), StoreError> {
        let s = MemStore::new();
        let b = small_block(b"x");
        s.write_block(&b)?;
        // Insert in deliberately unsorted order.
        for name in ["zeta", "alpha", "mu", "beta"] {
            s.put_ref(name, &b.hash(), None)?;
        }
        let listed = s.list_refs()?;
        let names: Vec<&str> = listed.iter().map(reel_spec::Ref::name).collect();
        assert_eq!(names, vec!["alpha", "beta", "mu", "zeta"]);
        Ok(())
    }

    #[test]
    fn was_recovered_false_for_memory_backend() {
        let s = MemStore::new();
        assert!(!s.was_recovered());
    }

    #[test]
    fn i_001_reachability_conformance_drop_ref_then_lookup() -> Result<(), StoreError> {
        // Mirrors spec/conformance/i_001_reachability.json:
        //   init -> put_block -> create_ref -> remove_ref -> lookup_by_hash ⇒ E-010.
        let s = MemStore::new();
        let b = small_block(b"hello"); // matches data_base64 "aGVsbG8="
        s.write_block(&b)?;

        s.put_ref("/probe", &b.hash(), None)?;
        // Reachable while ref exists.
        let mut ns = Namespace::default();
        ns.insert("/probe".into(), b.hash());
        assert!(s.lookup_by_hash(&b.hash(), &ns).is_ok());

        // Remove the ref; the kernel removes /probe from its namespace.
        s.remove_ref("/probe")?;
        let empty_ns = Namespace::default();
        let err =
            s.lookup_by_hash(&b.hash(), &empty_ns).expect_err("dropped ref must trigger E-010");
        assert!(
            matches!(err, StoreError::Protocol(ReelError::ReachabilityViolation(_))),
            "expected E-010, got {err:?}"
        );
        Ok(())
    }
}
