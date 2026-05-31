//! `adapter-fs` — Class A filesystem adapter for the reel protocol.
//!
//! This crate wires local filesystem operations into reel's
//! [`reel_spec::Delta`] layer per the Class A semantics of
//! ADR-0025
//! and `spec/spec.md` §5.7.1: reversed by Delta discard at `abort`,
//! never entering `View.effects_hash`.
//!
//! # Surface
//!
//! - [`FsPut`] / [`FsRemove`] — `#[reel_effects::class_a]` effect
//!   structs that emit `Op::Put` / `Op::Remove` on
//!   [`reel_effects::ClassA::apply`].
//! - [`FsAdapter`] — host-side facade.  Constructs the two effect
//!   types, classifies ops, and materialises an applied
//!   [`reel_spec::Delta`] onto the real filesystem under a narrowed
//!   [`reel_spec::Capability`] (I-003 enforcement).
//! - [`ApplyOutcome`] — per-op result from drain-time materialisation
//!   (`Changed` / `Unchanged`).
//! - [`EffectClass`] — adapter-level effect-class enum (per
//!   `spec/adapter-interface.md` §3); `FsAdapter::classify` always
//!   returns [`EffectClass::A`].
//! - [`FsAdapterError`] — adapter-local errors, mapped onto
//!   [`reel_spec::ReelError`] codes E-007 / E-009.
//!
//! # Invariants
//!
//! - **I-003** (Capability narrowing) — every materialised op is
//!   checked against the supplied capability's `path_prefix` and
//!   `op_set`.  Violations return `E-009` (see
//!   [`FsAdapterError::CapabilityViolation`]).
//! - **Class A purity** — `FsPut::apply` and `FsRemove::apply` have
//!   no I/O capability in their signatures; the L1 sealed-trait
//!   discipline (per [`reel_effects`]) makes a network or filesystem
//!   call unreachable from these methods in safe Rust.
//!
//! # Idempotency
//!
//! `FsAdapter::apply_delta` is content-idempotent: writing a file
//! whose on-disk contents already hash to the same value is a no-op,
//! and removing a missing file is a no-op.  This permits safe retry
//! and is the property exercised by the per-class conformance test
//! `t_002_fork_creates_isolated_view` (see `spec/conformance.md`
//! §1.2).
//!
//! # Examples
//!
//! ```rust,no_run
//! use adapter_fs::{ApplyOutcome, FsAdapter};
//! use reel_effects::ClassA;
//! use reel_spec::{Block, Capability, Delta, Hash, OpKind};
//! use std::collections::{BTreeMap, BTreeSet};
//!
//! let tmp = tempfile::tempdir().expect("tempdir");
//! let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
//!
//! let block = Block::new(b"hello reel".to_vec())
//!     .expect("small payload");
//! let mut blocks = BTreeMap::new();
//! blocks.insert(block.hash(), block.clone());
//!
//! let put = adapter.put("ws/greeting.txt".into(), block.hash());
//! let delta = Delta {
//!     base_hash: Hash::from_bytes([0u8; 32]),
//!     ops: vec![put.apply()],
//! };
//!
//! let cap = Capability {
//!     path_prefix: "ws/".into(),
//!     op_set: BTreeSet::from([OpKind::Write]),
//!     ttl: 600,
//! };
//! let outcomes = adapter.apply_delta(&delta, &cap, &blocks).expect("apply");
//! assert_eq!(outcomes, vec![ApplyOutcome::Changed]);
//! ```
//!
//! # References
//!
//! - `spec/spec.md` §5.7.1 — Class A semantics.
//! - `spec/adapter-interface.md` §3.1 — Class A classification.
//! - `spec/conformance.md` §1.2 — `t_002_fork_creates_isolated_view`.
//! - ADR-0003 — three-class effect taxonomy.
//! - ADR-0006 — four-layer effect isolation.
//! - ADR-0025 — six types, three verbs, three invariants.

#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace configuration issues outside adapter-fs scope"
)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::invalid_codeblock_attributes)]
#![warn(rustdoc::bare_urls)]
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings))))]

mod adapter;
mod effect;
mod error;

pub use adapter::{ApplyOutcome, EffectClass, FsAdapter};
pub use effect::{FsPut, FsRemove};
pub use error::FsAdapterError;

// Workspace `unused_crate_dependencies` lint: `serde` is pulled in for
// downstream derive support on adapter-defined effect payloads, but no
// non-test type in this crate references it directly.  Acknowledge by
// importing the crate root as `_` (see `reel-store/src/lib.rs` precedent).
use serde as _;
