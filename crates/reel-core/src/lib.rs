//! reel kernel — `fork` / `commit` / `abort` over the [`reel_spec`] types.
//!
//! `reel-core` realises the three protocol verbs defined in
//! ADR-0025
//! and `spec/spec.md` §6 on top of the value types exported by
//! [`reel_spec`].  At v0.1 the kernel is single-threaded
//! (ADR-0008);
//! every public operation takes `&mut self` on [`Kernel`].
//!
//! # Invariants
//!
//! - **I-001** Reachability — discharged by storage; `abort` makes the
//!   pending-effects [`reel_spec::Block`] unreachable so it is collected.
//! - **I-002** Abort silences Class C — discharged by [`Kernel::abort`]
//!   never constructing a `FireCapability`; the irreversible-effect
//!   payload is dropped before the drain phase that would have fired it.
//! - **I-003** Capability narrowing — discharged by
//!   [`reel_spec::View::child_of`] (out of scope here).
//!
//! See `spec/invariants.yaml` for the normative statement of each
//! invariant.
//!
//! # Status
//!
//! introduces [`Kernel::abort`]. (`fork`),
//!/ (`commit`), and (storage) are pending; this crate
//! grows one verb at a time.

// Pre-existing workspace issues: multiple dep versions and lint group priority
// conflicts in root Cargo.toml are not reel-core defects.
#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace configuration issues outside reel-core scope"
)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::invalid_codeblock_attributes)]
#![warn(rustdoc::bare_urls)]
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings))))]
#![doc(test(attr(deny(dead_code))))]

pub mod kernel;
#[cfg(feature = "walking-skeleton")]
pub mod skeleton;

pub use kernel::Kernel;
#[cfg(feature = "walking-skeleton")]
pub use skeleton::SkeletonKernel;
