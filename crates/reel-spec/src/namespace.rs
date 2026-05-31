//! CoW namespace handle — the forest-GC borrow at the data-structure layer.
//!
//! A [`Namespace`] maps a [`RefName`] to the [`crate::Hash`] of the
//! [`crate::Block`] currently bound there, inside a [`crate::View`]. It is
//! the *handle* through which a child View borrows its parent's namespace
//! snapshot without copying: `child.namespace.clone()` is a refcount bump on
//! the underlying B+tree root, **independent of the namespace size**.
//!
//! This is the implementation-layer realisation of the forest-GC theorem
//! (cf. `reel-kernel-architecture-2026-05-27.md` §1): cheap sharing without
//! GC requires *borrow* (`lifetime(borrower) ⊆ lifetime(owner)`), never
//! co-ownership. `imbl::OrdMap`'s structurally-shared B+tree gives reel
//! exactly this: cloning shares the spine; writes path-copy the affected
//! nodes only.
//!
//! See ADR-0034
//! for the decision record, alternatives considered, and flip conditions.
//!
//! # Why `imbl::OrdMap` (not `HashMap`)
//!
//! Sorted iteration is required for deterministic serialisation: future
//! Snapshot hashing (cf. `spec/spec.md` §5.6) builds on the same
//! BLAKE3-of-canonical-form pattern that backs [`crate::Block`]. `OrdMap`'s
//! B+tree iterates keys in sorted order natively; `HashMap` would require
//! an explicit sort step at every hash boundary, defeating the small
//! lookup-time win.
//!
//! # Why `SmolStr` for [`RefName`]
//!
//! Most reel namespace keys are short (single-segment names like
//! `"main"`, `"docs"`, or short paths like `"ws/a/b"`). `SmolStr`'s
//! 22-byte inline buffer eliminates the heap allocation in the common
//! case while preserving `O(1)` clone via the standard small-string
//! discriminator pattern.
//!
//! # Complexity
//!
//! | Operation | Complexity | Notes |
//! |---|---|---|
//! | [`Namespace::clone`] | `O(1)` | Arc refcount bump on the B+tree root. |
//! | [`Namespace::get`] | `O(log n)` | B+tree descent, ~`log₃₂ n` levels. |
//! | [`Namespace::insert`] | `O(log n)` | Path-copy on touched spine nodes only. |
//! | [`Namespace::remove`] | `O(log n)` | Path-copy + rebalance. |
//! | Sorted iteration | `O(n)` | In-order traversal; native. |
//!
//! # Invariants
//!
//! - **I-003-perf** (`spec/spec.md` §7.3): `View::child_of` is `O(1)` in
//!   the parent payload's size. `Namespace::clone` discharges this.
//!
//! # Examples
//!
//! ```rust
//! use reel_spec::{Hash, Namespace, RefName};
//!
//! // Fresh namespaces are empty.
//! let mut parent = Namespace::new();
//! assert!(parent.is_empty());
//!
//! // Insert a binding.
//! let h1 = Hash::from_bytes([1u8; 32]);
//! parent.insert(RefName::new("main"), h1);
//! assert_eq!(parent.get(&RefName::new("main")), Some(&h1));
//!
//! // Forking is a refcount bump — the child sees the parent's bindings
//! // immediately, without a deep copy.
//! let child = parent.clone();
//! assert_eq!(child.get(&RefName::new("main")), Some(&h1));
//!
//! // Writes path-copy: the child diverges without mutating the parent.
//! let mut child = child;
//! let h2 = Hash::from_bytes([2u8; 32]);
//! child.insert(RefName::new("draft"), h2);
//! assert_eq!(parent.get(&RefName::new("draft")), None);
//! assert_eq!(child.get(&RefName::new("draft")), Some(&h2));
//! ```
//!
//! # References
//!
//! - ADR-0034: CoW namespace representation.
//! - ADR-0025: Six Types, Three Verbs, Three Invariants.
//! - ADR-0032: Nesting boundary — Isolated / Autonomous (per-fork
//!   borrow chain).
//! - Bagwell, P. (2001). *Ideal Hash Trees*. EPFL.
//! - Stucki, N., Rompf, T., Ureche, V., Bagwell, P. (2015).
//!   *RRB-Vector*. ICFP 2015.
//! - `imbl` crate documentation: <https://docs.rs/imbl/7.0.0/imbl/>

use crate::Hash;
use smol_str::SmolStr;

/// A namespace key — the canonical name of a [`crate::Ref`] inside a View.
///
/// Implemented as [`smol_str::SmolStr`]: 22-byte inline buffer + `O(1)`
/// clone. Most agent ref names are short (e.g. `"main"`, `"draft"`,
/// `"ws/docs/readme"`) and fit inline without a heap allocation.
///
/// Use [`SmolStr::new`] / [`SmolStr::from`] to construct.
pub type RefName = SmolStr;

/// A CoW snapshot of a View's namespace.
///
/// `Namespace` is the type used by [`crate::View`]`::namespace` (see
/// `spec/spec.md` §5.6). Cloning a `Namespace` is `O(1)` — a refcount
/// bump on the underlying B+tree root — making `View::child_of` `O(1)`
/// regardless of how many bindings the parent View carries.
///
/// `imbl::OrdMap` provides the persistent (functional) B+tree:
/// modifications path-copy only the touched spine, leaving the
/// pre-existing root reachable from prior clones. This realises the
/// forest-GC borrow pattern (`reel-kernel-architecture-2026-05-27.md`
/// §1) at the data-structure layer.
///
/// See the module-level documentation for the full rationale and
/// complexity table.
pub type Namespace = imbl::OrdMap<RefName, Hash>;

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are acceptable for test failures"
)]
mod tests {
    use super::*;

    fn h(byte: u8) -> Hash {
        Hash::from_bytes([byte; 32])
    }

    #[test]
    fn empty_namespace_is_empty() {
        let ns = Namespace::new();
        assert!(ns.is_empty());
        assert_eq!(ns.len(), 0);
    }

    #[test]
    fn insert_then_get() {
        let mut ns = Namespace::new();
        ns.insert(RefName::new("k"), h(1));
        assert_eq!(ns.get(&RefName::new("k")), Some(&h(1)));
        assert_eq!(ns.len(), 1);
    }

    #[test]
    fn clone_preserves_bindings() {
        let mut parent = Namespace::new();
        parent.insert(RefName::new("a"), h(1));
        parent.insert(RefName::new("b"), h(2));

        let child = parent.clone();
        assert_eq!(child.get(&RefName::new("a")), Some(&h(1)));
        assert_eq!(child.get(&RefName::new("b")), Some(&h(2)));
        assert_eq!(child.len(), 2);
    }

    #[test]
    fn child_writes_do_not_mutate_parent() {
        // The forest-GC borrow property at the unit level:
        // child borrows parent's snapshot; child's writes path-copy
        // only the child's view, never the parent.
        let mut parent = Namespace::new();
        parent.insert(RefName::new("shared"), h(1));

        let mut child = parent.clone();
        child.insert(RefName::new("only-in-child"), h(2));

        // Parent unchanged.
        assert_eq!(parent.get(&RefName::new("shared")), Some(&h(1)));
        assert_eq!(parent.get(&RefName::new("only-in-child")), None);
        assert_eq!(parent.len(), 1);

        // Child sees both.
        assert_eq!(child.get(&RefName::new("shared")), Some(&h(1)));
        assert_eq!(child.get(&RefName::new("only-in-child")), Some(&h(2)));
        assert_eq!(child.len(), 2);
    }

    #[test]
    fn iteration_is_sorted() {
        // Insert in reverse-alphabetical order; iteration must come
        // out alphabetical (required for deterministic Snapshot hashing
        // — see ADR-0034 driver 3).
        let mut ns = Namespace::new();
        ns.insert(RefName::new("c"), h(3));
        ns.insert(RefName::new("a"), h(1));
        ns.insert(RefName::new("b"), h(2));

        let keys: Vec<&str> = ns.keys().map(SmolStr::as_str).collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn refname_short_string_inline() {
        // SmolStr's inline buffer is 22 bytes (per smol_str docs).
        // A short ref name should not heap-allocate. We can't observe
        // the allocation directly without an allocator hook, but we
        // can confirm the round-trip preserves identity.
        let n = RefName::new("ws/docs/readme.md");
        assert_eq!(n.as_str(), "ws/docs/readme.md");
        assert_eq!(n.len(), 17);
    }

    #[test]
    fn serde_json_round_trip_preserves_order() {
        // Drives ADR-0034 backward-compat clause: JSON serialisation
        // matches BTreeMap's ordered-object form.
        let mut ns = Namespace::new();
        ns.insert(RefName::new("alpha"), h(1));
        ns.insert(RefName::new("beta"), h(2));

        let j = serde_json::to_string(&ns).expect("serialise");
        // Sorted key order in the JSON object.
        let alpha_pos = j.find("alpha").expect("alpha present");
        let beta_pos = j.find("beta").expect("beta present");
        assert!(alpha_pos < beta_pos, "JSON keys must be sorted: {j}");

        let ns2: Namespace = serde_json::from_str(&j).expect("deserialise");
        assert_eq!(ns2.len(), 2);
        assert_eq!(ns2.get(&RefName::new("alpha")), Some(&h(1)));
        assert_eq!(ns2.get(&RefName::new("beta")), Some(&h(2)));
    }
}
