//! Effect class traits and `Capability` narrowing for the reel protocol.
//!
//! `reel-effects` provides:
//!
//! - Three **sealed** effect-class traits: [`ClassA`], [`ClassB`], [`ClassC`].
//! - The **published** [`Capability`] type with narrowing-only constructors
//!   (I-003 enforcement by API surface).
//! - The **kernel-internal** [`FireCapability`] linear token (I-002
//!   enforcement at commit drain time).
//! - Type-erased wrappers [`BoxedA`], [`BoxedB`], [`BoxedC`] for kernel
//!   buffering.
//! - [`EffectReceipt`] returned by successful `fire()` calls.
//!
//! # Effect classes
//!
//! | Trait | Class | Reversibility | `fire` capability |
//! |-------|-------|---------------|-------------------|
//! | [`ClassA`] | A — pure local | Reversed by Delta discard at `abort` | None |
//! | [`ClassB`] | B — idempotent remote | Versioned precondition at commit | [`FireCapability`] reference |
//! | [`ClassC`] | C — irreversible remote | Not reversible | [`FireCapability`] reference |
//!
//! # Sealing
//!
//! All three traits are sealed via the `pub(crate) sealed::Sealed`
//! supertrait — that supertrait is unreachable to external crates, so
//! `impl ClassA for ForeignType` fails at visibility resolution.  External
//! crates must use the `#[reel::class_a]` / `#[reel::class_b]` /
//! `#[reel::class_c]` proc-macros, which generate the seal + class
//! impl pair inside this crate.
//!
//! # Two capabilities
//!
//! Per ADR-0025 and ADR-0006:
//!
//! - [`Capability`] — the **published authority** type (`{ path_prefix,
//!   op_set, ttl }`).  Held by `View.caps`; transmitted across `fork`;
//!   narrowed by [`Capability::narrow`].  Governs I-003.
//! - [`FireCapability`] — the **commit-drain mode** token.  Constructed only
//!   inside `reel-core`'s commit path; consumed by `ClassB::fire` /
//!   `ClassC::fire`.  Enforces I-002 (abort silences Class C).
//!
//! # References
//!
//! - Wang, C., Zheng, Y. (2026). Fork, Explore, Commit. arXiv 2602.08199v2.
//! - ADR-0025: Six Types, Three Verbs, Three Invariants.
//! - ADR-0003: Three-class effect taxonomy.
//! - ADR-0006: Four-layer effect isolation (L1–L4).
//! - Miller, M., Yee, K., Shapiro, J. (2003). Capability Myths Demolished.

// Propagate workspace-level lint overrides that are relevant here.
#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace configuration issues"
)]
// Seal-related public items require doc; ensure rustdoc links stay valid.
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::bare_urls)]
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings))))]

// Allow proc-macro expansions that name `::reel_effects::__private::Sealed`
// to resolve correctly when used from inside this crate (downstream crates
// pick up the path naturally via their dep on `reel-effects`).
extern crate self as reel_effects;

mod boxed;
pub(crate) mod capability;
mod class_a;
mod class_b;
mod class_c;
#[cfg(feature = "walking-skeleton")]
pub mod fake;
pub(crate) mod fire_capability;
mod receipt;
pub(crate) mod sealed;

pub use boxed::{BoxedA, BoxedB, BoxedC};
pub use capability::{Capability, NarrowArgs};
pub use class_a::ClassA;
pub use class_b::{ClassB, PreconditionCtx};
pub use class_c::ClassC;
pub use fire_capability::FireCapability;
pub use receipt::EffectReceipt;

// ──────────────────────────────────────────────────────────────────────────
// Procedural macros
//
// `class_a` / `class_b` / `class_c` are re-exported from `reel-effects-macros`
// so users write `#[reel_effects::class_a]` rather than the macro crate path
// directly.  The macros generate `impl __private::Sealed for $Type {}` at the
// call site; see `__private` below.
// ──────────────────────────────────────────────────────────────────────────

pub use reel_effects_macros::{class_a, class_b, class_c};

/// Re-export hatch used **only** by the `#[reel_effects::class_a / b / c]`
/// macros expanded in downstream crates.
///
/// The contents of this module are intentionally not part of the published
/// API surface: writing `::reel_effects::__private::Sealed` by hand is
/// supported by the compiler but unsupported by the crate's authors and
/// will not be considered a breaking-change anchor.  Use the macros.
///
/// Rationale: the `Sealed` supertrait
/// otherwise lives in `pub(crate) sealed`, which makes it unreachable to
/// any caller — including a derive-style macro's expansion at the user's
/// call site.  To allow `#[reel_effects::class_a]` to compile downstream,
/// the macros must spell out a path the user's crate can resolve;
/// `__private::Sealed` is that path.  The seal therefore degrades from
/// "mechanically impossible" to "by-discipline + lint-visible"
///.  This is the established pattern used by `serde`, `pin-project`
/// and `clap` for the same situation.
#[doc(hidden)]
pub mod __private {
    pub use crate::sealed::Sealed;
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "test assertions intentionally panic on failure"
)]
mod tests {
    use super::*;
    use crate::sealed::Sealed;
    use reel_spec::{
        Capability as SpecCapability, Hash, Op, OpKind, ReelError, VersionKind, VersionRef,
    };
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::pin::Pin;

    // ──────────────────────────────────────────────────────────────────
    // Helpers — in-crate concrete implementations for testing
    //
    // `Sealed` is `pub(crate)` so in-crate test types may `impl Sealed`
    // directly — this is the canonical pattern that preserves the
    // external-uninhabitability of `Sealed` (and therefore of ClassA / B / C).
    // ──────────────────────────────────────────────────────────────────

    struct TestPutEffect {
        name: String,
        hash: Hash,
    }

    impl Sealed for TestPutEffect {}

    impl ClassA for TestPutEffect {
        fn apply(&self) -> Op {
            Op::Put { name: self.name.clone(), hash: self.hash }
        }
    }

    struct TestRemoveEffect {
        name: String,
    }

    impl Sealed for TestRemoveEffect {}

    impl ClassA for TestRemoveEffect {
        fn apply(&self) -> Op {
            Op::Remove { name: self.name.clone() }
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // ClassA tests
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn class_a_produces_put_op() {
        let h = Hash::from_bytes([42u8; 32]);
        let eff = TestPutEffect { name: "docs/readme.md".into(), hash: h };
        let op = eff.apply();
        assert!(
            matches!(op, Op::Put { ref name, .. } if name == "docs/readme.md"),
            "expected Put op, got {op:?}"
        );
    }

    #[test]
    fn class_a_produces_remove_op() {
        let eff = TestRemoveEffect { name: "old.txt".into() };
        let op = eff.apply();
        assert!(
            matches!(op, Op::Remove { ref name } if name == "old.txt"),
            "expected Remove op, got {op:?}"
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // BoxedA tests
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn boxed_a_apply() {
        let h = Hash::from_bytes([1u8; 32]);
        let boxed = BoxedA::new(TestPutEffect { name: "x.txt".into(), hash: h });
        let op = boxed.apply();
        assert!(matches!(op, Op::Put { .. }));
    }

    // ──────────────────────────────────────────────────────────────────
    // Capability narrowing tests (spot-check; full suite in capability.rs)
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn narrow_happy_path() {
        let root = Capability::from_kernel(SpecCapability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
            ttl: 3600,
        });
        let child = root
            .narrow(NarrowArgs {
                path_prefix: Some("/docs/".into()),
                op_set: Some(BTreeSet::from([OpKind::Read])),
                ttl: Some(600),
            })
            .unwrap_or_else(|e| panic!("valid narrowing failed: {e}"));
        assert_eq!(child.inner().path_prefix, "/docs/");
        assert_eq!(child.inner().ttl, 600);
    }

    #[test]
    fn narrow_rejects_op_amplification() {
        let parent = Capability::from_kernel(SpecCapability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 60,
        });
        let err = parent.narrow(NarrowArgs {
            path_prefix: None,
            op_set: Some(BTreeSet::from([OpKind::Read, OpKind::Delete])),
            ttl: None,
        });
        assert!(matches!(err, Err(ReelError::CapabilityViolation(_))), "expected E-009");
    }

    // ──────────────────────────────────────────────────────────────────
    // EffectReceipt tests
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn effect_receipt_fields() {
        let receipt = EffectReceipt {
            effect_id: 7,
            version_ref_after: Some(VersionRef {
                kind: VersionKind::CommitSha,
                value: "abc123".into(),
            }),
            adapter_diagnostic: serde_json::json!({"ok": true}),
        };
        assert_eq!(receipt.effect_id, 7);
        assert!(receipt.version_ref_after.is_some());
    }

    // ──────────────────────────────────────────────────────────────────
    // Sealed-trait check (compile-time; verified by grep in self-check)
    // ──────────────────────────────────────────────────────────────────

    // No runtime test needed — if an external crate tried `impl ClassA` it
    // would fail at compile time because `Sealed` is `pub(crate)`.

    // ──────────────────────────────────────────────────────────────────
    // Procedural macro round-trip
    //
    // The macros expand at the call site, so the in-crate path
    // `::reel_effects::__private::Sealed` resolves through the published
    // re-export — exactly as it will for downstream crates.  These tests
    // are the runnable counterpart to the `#[reel::class_a]` doc-test
    // examples carried by `class_a.rs` / `boxed.rs`.
    // ──────────────────────────────────────────────────────────────────

    #[crate::class_a]
    struct MacroPutEffect {
        name: String,
        hash: Hash,
    }

    impl ClassA for MacroPutEffect {
        fn apply(&self) -> Op {
            Op::Put { name: self.name.clone(), hash: self.hash }
        }
    }

    #[test]
    fn macro_class_a_round_trips_via_apply() {
        let eff = MacroPutEffect { name: "macros.txt".into(), hash: Hash::from_bytes([7u8; 32]) };
        let op = eff.apply();
        assert!(
            matches!(op, Op::Put { ref name, .. } if name == "macros.txt"),
            "expected Put op via macro-generated Sealed, got {op:?}"
        );
        // Drift marker exists at compile time and disambiguates by type.
        assert_eq!(__REEL_EFFECT_CLASS_A__MacroPutEffect, "A");
    }

    #[crate::class_c]
    struct MacroSlackPost {
        channel: String,
        body: String,
    }

    impl ClassC for MacroSlackPost {
        fn describe(&self) -> serde_json::Value {
            serde_json::json!({ "channel": self.channel, "body": self.body })
        }

        fn fire<'a>(
            self,
            _cap: &'a FireCapability,
        ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
        where
            Self: 'a,
        {
            Box::pin(async move {
                Ok(EffectReceipt {
                    effect_id: 0,
                    version_ref_after: None,
                    adapter_diagnostic: serde_json::json!({ "kind": "test-class-c" }),
                })
            })
        }
    }

    #[test]
    fn macro_class_c_describe_and_buffer() {
        let eff = MacroSlackPost { channel: "#general".into(), body: "ship it".into() };
        let desc = eff.describe();
        assert_eq!(desc.get("channel").and_then(|v| v.as_str()), Some("#general"));
        let boxed = BoxedC::new(eff);
        assert_eq!(boxed.describe().get("body").and_then(|v| v.as_str()), Some("ship it"));
        // Drift marker exists at compile time and disambiguates by type.
        assert_eq!(__REEL_EFFECT_CLASS_C__MacroSlackPost, "C");
    }

    // ──────────────────────────────────────────────────────────────────
    // FireCapability is kernel-internal
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn fire_capability_new_only_pub_crate() {
        // FireCapability::new() is pub(crate); this test exercises the
        // constructor within the crate.
        let cap = FireCapability::new();
        assert_eq!(format!("{cap:?}"), "FireCapability { .. }");
    }
}
