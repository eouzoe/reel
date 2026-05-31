//! Persistent and in-memory storage for reel `Block`s and `Ref`s.
//!
//! `reel-store` provides two interchangeable storage backends behind the
//! [`Store`] trait:
//!
//! - [`MemStore`] — an in-memory backend backed by [`std::collections::BTreeMap`].
//!   Used by the walking-skeleton tests and the in-process
//!   `reel-conformance-runner` to avoid touching the filesystem.
//! - [`RedbStore`] — a persistent backend backed by [`redb`] 4.x with a
//!   four-table layout per ADR-0017
//!   and ADR-0018.
//!
//! Both backends enforce **I-001 Reachability** at `lookup_by_hash`:
//! a Block is only returned when its `Hash` is reachable through the
//! transitive closure of the supplied namespace plus the
//! Block-payload → Block-payload edges discovered along the way.
//! See `spec/invariants.yaml` (I-001) and
//! `spec/conformance/i_001_reachability.json`.
//!
//! # ADR-0025 ontology
//!
//! Per ADR-0025:
//!
//! - Block identity is `Hash` (BLAKE3-256), not the pre-ADR-0024 `BlockId`.
//! - `Ref` identity is `name`; there is no separate `RefId` UUID.
//! - Blocks have no protocol-surface parent; the pre-ADR-0025
//!   `parent_idx` table is removed.
//! - Class B effects carry a [`VersionRef`](reel_spec::VersionRef), not an
//!   `IdempotencyKey`.
//!
//! # Examples
//!
//! ```rust
//! use reel_store::{MemStore, Namespace, Store};
//! use reel_spec::{Block};
//!
//! let store = MemStore::new();
//! let block = Block::new(b"hello reel".to_vec())
//!     .expect("payload fits");
//! let hash = block.hash();
//! store.write_block(&block).expect("write");
//!
//! let mut ns = Namespace::default();
//! ns.insert("main".into(), hash);
//! store.put_ref("main", &hash, None).expect("put_ref");
//!
//! let fetched = store.lookup_by_hash(&hash, &ns).expect("reachable");
//! assert_eq!(fetched.hash(), hash);
//! ```
//!
//! # References
//!
//! - ADR-0017 — On-disk storage layout (four redb tables + CBOR encoding).
//! - ADR-0018 — Storage engine choice (redb 2.x; workspace now pins `redb = "4"`).
//! - ADR-0025 — Six types, three verbs, three invariants.
//! - `spec/spec.md` §5 — Block / Ref / Hash type definitions.
//! - `spec/conformance/i_001_reachability.json` — I-001 reachability vector.

// Pre-existing workspace issues: workspace-level lint group priorities + the
// transitive redb/moka multi-version graph trip `cargo` warnings that are
// outside this crate's authority.
#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace configuration; outside reel-store scope"
)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::invalid_codeblock_attributes)]
#![warn(rustdoc::bare_urls)]
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings))))]
#![doc(test(attr(deny(dead_code))))]

mod cbor;
mod error;
mod mem;
mod reachability;
mod redb_store;
mod store_trait;

pub use error::StoreError;
pub use mem::MemStore;
pub use reachability::{DEFAULT_REACHABILITY_DEPTH_LIMIT, ReachabilityWalker};
pub use redb_store::{Config, GcStats, RedbStore, SyncMode};
pub use store_trait::{Namespace, Store};

// Workspace-level `unused_crate_dependencies` lint accounting:
//   - `serde` is referenced through `serde::{Serialize, Deserialize}` derives
//     on types that flow through the crate, but the lint needs a visible
//     non-test `use` to count.
//   - `proptest` is a `[dev-dependencies]`; squelched here for the
//     non-test build only.
use serde as _;

#[cfg(test)]
use proptest as _;
