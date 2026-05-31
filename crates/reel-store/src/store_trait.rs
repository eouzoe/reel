//! The unified [`Store`] trait + [`Namespace`] re-export.
//!
//! Both [`crate::MemStore`] and [`crate::RedbStore`] implement [`Store`].
//! The trait fixes the canonical method signatures that the kernel
//! (`reel-core`) and the conformance runner depend on, decoupled from
//! the backend choice.

use reel_spec::{Block, Hash, Ref, VersionRef};

use crate::error::StoreError;

/// The kernel's view of the active namespace.
///
/// Re-exports [`reel_spec::Namespace`] (an `imbl::OrdMap<RefName, Hash>`,
/// per ADR-0034) so the kernel and the storage layer share one canonical
/// CoW namespace type — `O(1)` clone, ordered iteration, structural
/// sharing across forks. A namespace is owned by the kernel; the store
/// only consults it as an input to [`Store::lookup_by_hash`] and
/// [`crate::RedbStore::gc`].
///
/// # Examples
///
/// ```rust
/// use reel_store::Namespace;
/// use reel_spec::Hash;
///
/// let mut ns = Namespace::default();
/// ns.insert("main".into(), Hash::from_bytes([0u8; 32]));
/// assert_eq!(ns.len(), 1);
/// ```
pub type Namespace = reel_spec::Namespace;

/// Unified storage trait for `MemStore` and `RedbStore`.
///
/// `Store` is `Send + Sync` so it can be wrapped in `Arc` and shared
/// across threads. Each implementation provides its own internal
/// synchronisation (redb's transactional model, or a `parking_lot`
/// `RwLock` for the in-memory backend).
///
/// All `lookup_by_hash` calls enforce **I-001 Reachability** against
/// the supplied namespace. The
/// [`Store::raw_lookup_by_hash`](Store::raw_lookup_by_hash) method
/// bypasses the gate and is reserved for GC / snapshot machinery.
///
/// # Examples
///
/// ```rust
/// use reel_store::{MemStore, Namespace, Store};
/// use reel_spec::{Block};
///
/// let s = MemStore::new();
/// let b = Block::new(b"x".to_vec()).expect("small");
/// s.write_block(&b).expect("write");
/// let mut ns = Namespace::default();
/// ns.insert("k".into(), b.hash());
/// s.put_ref("k", &b.hash(), None).expect("put_ref");
/// let got = s.lookup_by_hash(&b.hash(), &ns).expect("reachable");
/// assert_eq!(got.hash(), b.hash());
/// ```
pub trait Store: Send + Sync {
    /// Write `block` to storage, indexed by `block.hash`.
    ///
    /// Identical content (same `Hash`) is deduplicated — a no-op write
    /// returns `Ok(())` without error.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Engine`] / [`StoreError::Io`] /
    /// [`StoreError::Codec`] on storage-layer failure.
    fn write_block(&self, block: &Block) -> Result<(), StoreError>;

    /// Look up a [`Block`] by [`struct@Hash`], with the I-001 reachability gate.
    ///
    /// The lookup walks `root` (plus the Block-payload → Block-payload
    /// edges encoded in CBOR-`Delta` payloads) and returns the Block
    /// iff `hash` is in the resulting closure.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Protocol`] wrapping
    ///   [`reel_spec::ReelError::ReachabilityViolation`] (E-010) if the
    ///   Block exists in storage but is not reachable from `root`.
    /// - [`StoreError::Protocol`] wrapping
    ///   [`reel_spec::ReelError::BlockNotFound`] (E-001) if the Block
    ///   is not in storage at all.
    /// - Storage-layer errors as for [`Store::write_block`].
    fn lookup_by_hash(&self, hash: &Hash, root: &Namespace) -> Result<Block, StoreError>;

    /// Look up a [`Block`] by [`struct@Hash`] without the reachability check.
    ///
    /// This is the GC / snapshot back-door: it MUST NOT be exposed to
    /// adapters or the CLI. The default impl returns
    /// [`StoreError::Config`] so backends that do not wish to support
    /// raw access can opt out; concrete backends in this crate
    /// implement it.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Protocol`] wrapping
    ///   [`reel_spec::ReelError::BlockNotFound`] (E-001) when the Block
    ///   is absent.
    /// - Storage-layer errors as for [`Store::write_block`].
    fn raw_lookup_by_hash(&self, hash: &Hash) -> Result<Block, StoreError>;

    /// Insert (or replace) a [`Ref`] keyed by `name`.
    ///
    /// Enforces I-001 at write time: `hash` MUST already be present in
    /// storage. A mismatch returns
    /// [`reel_spec::ReelError::BlockNotFound`] (E-001) — the spec uses
    /// this rather than a separate `DanglingRef` error code (E-001
    /// already covers "the named Block isn't there").
    ///
    /// # Errors
    ///
    /// - [`StoreError::Protocol`] with `BlockNotFound` (E-001) when
    ///   `hash` does not correspond to a stored Block.
    /// - [`StoreError::Protocol`] propagating
    ///   [`reel_spec::ReelError::Storage`] (E-007) when `name` is
    ///   empty or exceeds [`reel_spec::ref_type::MAX_REF_NAME_BYTES`].
    /// - Storage-layer errors as for [`Store::write_block`].
    fn put_ref(
        &self,
        name: &str,
        hash: &Hash,
        version_constraint: Option<VersionRef>,
    ) -> Result<(), StoreError>;

    /// Fetch a [`Ref`] by `name`.
    ///
    /// Returns `Ok(None)` if no Ref with that name is stored.
    ///
    /// # Errors
    ///
    /// Storage-layer errors as for [`Store::write_block`].
    fn get_ref(&self, name: &str) -> Result<Option<Ref>, StoreError>;

    /// Remove a [`Ref`] by `name`.
    ///
    /// Returns `Ok(true)` if a Ref was removed; `Ok(false)` when no
    /// Ref with `name` existed.
    ///
    /// # Errors
    ///
    /// Storage-layer errors as for [`Store::write_block`].
    fn remove_ref(&self, name: &str) -> Result<bool, StoreError>;

    /// Enumerate all stored [`Ref`]s in `name`-sorted order.
    ///
    /// The collection is materialised into a `Vec` rather than
    /// returned as `impl Iterator` so that the backend can release its
    /// read transaction before yielding.
    ///
    /// # Errors
    ///
    /// Storage-layer errors as for [`Store::write_block`].
    fn list_refs(&self) -> Result<Vec<Ref>, StoreError>;

    /// True iff the store reports it was recovered (redb's WAL replay)
    /// on the last `open`.
    fn was_recovered(&self) -> bool;
}
