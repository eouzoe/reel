//! Protocol-type definitions for the reel agent-side-effect protocol.
//!
//! `reel-spec` defines the six core types from
//! ADR-0025:
//! [`struct@Hash`], [`Block`], [`Ref`], [`Delta`], [`Capability`], and [`View`].
//! It also provides [`EffectDescriptor`] (the three-class effect
//! classification over Block payloads) and [`ReelError`] (the nine-variant
//! error catalogue from `spec/errors.yaml`).
//!
//! This crate is intentionally dependency-light so every downstream crate
//! — `reel-core`, `reel-effects`, `reel-store`, `reel-cli`, //! and all adapters — can import from here without pulling in the kernel
//! or storage layers.
//!
//! # Protocol overview
//!
//! reel expresses three verbs (`fork` / `commit` / `abort`) over six
//! core types and a three-class effect taxonomy.
//! The canonical ontology is described in ADR-0025 and the normative spec
//! is in `spec/spec.md`.
//!
//! ## Six core types
//!
//! | Type | Role |
//! |------|------|
//! | [`struct@Hash`] | BLAKE3-256 digest; Block identity constructor (per ADR-0004). |
//! | [`Block`] | Immutable content unit. Identity = `Hash(content)`. |
//! | [`Ref`] | Mutable named pointer. Updated atomically by `commit`. |
//! | [`Delta`] | Ordered mutations from a base Block. |
//! | [`Capability`] | Scoped attenuating authority (I-003 narrowing). |
//! | [`View`] | Workspace closure; the reel "transaction". |
//!
//! ## Effect classes
//!
//! See [`EffectDescriptor`] for Class A (pure local), Class B (idempotent
//! remote), and Class C (irreversible remote) semantics.
//!
//! ## Errors
//!
//! See [`ReelError`] for all nine protocol error codes (E-001 through
//! E-010, with E-004 reserved).
//!
//! # Examples
//!
//! ```rust
//! use reel_spec::Block;
//!
//! // Identity is a pure function of content: hash = BLAKE3(data).
//! let block = Block::new(b"hello reel".to_vec())
//!     .expect("payload is within MAX_BLOCK_SIZE");
//!
//! assert_eq!(block.hash().as_bytes().len(), 32);
//! ```
//!
//! # References
//!
//! - Wang, C., Zheng, Y. (2026). Fork, Explore, Commit.
//!   arXiv 2602.08199v2. <https://arxiv.org/abs/2602.08199>
//! - ADR-0025: Six Types, Three Verbs, Three Invariants.
//!   the design notes
//! - ADR-0004: BLAKE3-256 as Block hash.
//!   the design notes
//! - Berenson, H., et al. (1995). A Critique of ANSI SQL Isolation Levels.
//!   SIGMOD. (View ≈ Snapshot Isolation transaction.)
//! - Miller, M., Yee, K., Shapiro, J. (2003). Capability Myths Demolished.
//!   (Capability narrowing lattice, I-003.)

// Pre-existing workspace issues: multiple dep versions and lint group priority
// conflicts in root Cargo.toml are not reel-spec defects.
#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace configuration issues outside reel-spec scope"
)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::invalid_codeblock_attributes)]
#![warn(rustdoc::bare_urls)]
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings))))]
#![doc(test(attr(deny(dead_code))))]

pub mod block;
pub mod capability;
pub mod delta;
pub mod effect;
pub mod error;
pub mod hash;
pub mod namespace;
pub mod provenance;
pub mod ref_type;
pub mod view;

pub use block::{Block, MAX_BLOCK_SIZE};
pub use capability::{Capability, OpKind};
pub use delta::{Delta, Op};
pub use effect::{EffectDescriptor, VersionKind, VersionRef};
pub use error::ReelError;
pub use hash::Hash;
pub use namespace::{Namespace, RefName};
pub use provenance::Provenance;
pub use ref_type::Ref;
pub use view::{Status, View, ViewId};
