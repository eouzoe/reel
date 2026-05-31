//! The reel [`Kernel`] — entry point for the three protocol verbs.
//!
//! At v0.1 the kernel holds a [`Store`] handle and implements
//! [`Kernel::abort`].  `fork` lands with, `commit` validation with
//!, and `commit` drain with.
//!
//! The kernel is single-threaded per
//! ADR-0008;
//! the `&mut self` on every verb enforces mutual exclusion at the type
//! level.  The [`Store`] behind the `Arc` is itself `Send + Sync` with
//! `&self` methods, so it is shared, not owned exclusively — the kernel's
//! `&mut self` is the verb-level lock, the store is the durable backend.

use std::fmt;
use std::sync::Arc;

use reel_spec::{Capability, ReelError, Status, View};
use reel_store::Store;

/// The reel kernel: the single object that holds the verb implementations
/// and the durable [`Store`] handle.
///
/// `store` is an `Arc<dyn Store>`: dynamic dispatch on a cold path
/// (the verbs are infrequent relative to reads).  This lets the CLI pick
/// [`reel_store::RedbStore`] at runtime while tests use
/// [`reel_store::MemStore`], without threading a generic parameter through
/// every caller.
///
/// `Kernel` is `Clone` (cloning shares the same underlying store via the
/// `Arc` refcount) but **not** `Copy` / `Default` — there is no meaningful
/// kernel without a store.
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use reel_core::Kernel;
/// use reel_spec::View;
/// use reel_store::MemStore;
///
/// let mut k = Kernel::new(Arc::new(MemStore::new()));
/// let mut v = View::root(vec![]);
/// k.abort(&mut v).expect("Active View aborts cleanly");
/// assert!(v.is_aborted());
/// ```
#[derive(Clone)]
pub struct Kernel {
    /// Durable backend.  Shared (`Arc`) and interior-mutable (`Store`
    /// methods take `&self`); the kernel's `&mut self` is the verb lock.
    store: Arc<dyn Store>,
}

impl Kernel {
    /// Constructs a `Kernel` over the given [`Store`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::Arc;
    /// use reel_core::Kernel;
    /// use reel_store::MemStore;
    ///
    /// let k = Kernel::new(Arc::new(MemStore::new()));
    /// // The kernel now carries a durable backend.
    /// assert!(!k.store().was_recovered()); // a fresh MemStore was not recovered
    /// ```
    #[must_use]
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Returns the kernel's [`Store`] handle.
    ///
    /// Callers (e.g. the CLI, the commit drain) use this to read / write
    /// Blocks and Refs.  Returns `&dyn Store` so callers depend on the
    /// trait, not the concrete backend.
    #[must_use]
    pub fn store(&self) -> &dyn Store {
        &*self.store
    }

    /// Forks `parent` into a new child [`View`] — the `fork` verb.
    ///
    /// Delegates to [`View::child_of`], which performs the load-bearing
    /// work: rejects a non-`Active` parent (`E-008`), enforces **I-003
    /// capability narrowing** (`E-009` on any widening of path-prefix,
    /// op-set, or TTL), and inherits the parent namespace by **O(1)
    /// copy-on-write** clone (imbl `OrdMap` refcount bump; ADR-0034).
    ///
    /// # Errors
    ///
    /// - [`ReelError::ViewTerminal`] (`E-008`) — `parent` is not `Active`.
    /// - [`ReelError::CapabilityViolation`] (`E-009`) — `attenuated_caps`
    ///   widens `parent.caps` on any axis (path-prefix, op-set, or TTL);
    ///   per I-003.
    ///
    /// # Complexity
    ///
    /// O(1) in the parent's namespace size (`CoW` clone; ADR-0034).
    ///
    /// # Depth bound — deferred at v0.1
    ///
    /// The I-008 *convenience* bound (`depth ≤ MAX_FORK_DEPTH`) is **not**
    /// enforced here: `View` stores no `depth`, the kernel holds no view
    /// registry to walk the parent chain, and the bound is unreachable in
    /// v0.1's single-fork-per-run usage. Tracked as a documented gap (a
    /// stored `depth` field lands when nested fork becomes real). The core
    /// invariants I-001 / I-002 / I-003 are unaffected.
    ///
    /// # Cancel Safety
    ///
    /// Cancel safe — synchronous, no `await`, no `FireCapability`
    /// constructed.
    // Takes `&mut self` for kernel-verb uniformity (the ADR-0008 single-writer
    // lock shared by `commit` / `abort`); `fork` is store-independent at v0.1,
    // but a future fork may persist to the store.
    pub fn fork(
        &mut self,
        parent: &View,
        attenuated_caps: Vec<Capability>,
    ) -> Result<View, ReelError> {
        View::child_of(parent, attenuated_caps)
    }

    /// Aborts a [`View`], discharging I-002 by dropping its pending-effects
    /// reference.
    ///
    /// The mechanism is structural rather than active:
    ///
    /// 1. The pending-effects [`reel_spec::Block`] referenced by
    ///    `view.effects_hash` is dropped (set to `None`).  No code path
    ///    from `abort` reaches the commit drain that would fire its Class
    ///    B or Class C payload, and the Block becomes unreachable per
    ///    I-001 — storage garbage-collects it on the next sweep.
    /// 2. The pending Class A mutations in `view.delta.ops` are cleared so
    ///    callers observe an empty delta after the call.
    /// 3. The View transitions `Active → Aborted` via
    ///    [`View::transition_to`]; further verbs against this View are
    ///    rejected with `E-008 ViewTerminal`.
    ///
    /// **I-002 holds at the type level**: `abort` never constructs the
    /// per-commit fire token, so no Class B or Class C effect of `view`
    /// can have fired during `view`'s lifetime, and none can fire
    /// thereafter (the terminal-state check on `submit_*` blocks new
    /// submissions; the dropped `effects_hash` blocks any retroactive
    /// drain).  See `spec/invariants.yaml` `id: I-002` and
    /// the design notes §6.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (`E-008`) when `view` is not in
    /// [`Status::Active`].  Aborting an already-terminal View is a caller
    /// bug; the View is left untouched.
    ///
    /// # Cancel Safety
    ///
    /// Cancel safe — synchronous, no `await` points, no `FireCapability`
    /// constructed..
    ///
    /// # Complexity
    ///
    /// O(1) — three field-level writes regardless of `view.delta.ops.len()`
    /// (the `Vec::clear` retains capacity; payload Blocks are discarded by
    /// `Option<Hash>` drop, not by traversal).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::Arc;
    /// use reel_core::Kernel;
    /// use reel_spec::{Hash, View};
    /// use reel_store::MemStore;
    ///
    /// let mut v = View::root(vec![]);
    /// v.effects_hash = Some(Hash::from_bytes(*blake3::hash(b"pending").as_bytes()));
    ///
    /// let mut k = Kernel::new(Arc::new(MemStore::new()));
    /// k.abort(&mut v).expect("Active View aborts cleanly");
    ///
    /// assert!(v.is_aborted());
    /// assert!(v.effects_hash.is_none());
    /// assert!(v.delta.ops.is_empty());
    /// ```
    pub fn abort(&mut self, view: &mut View) -> Result<(), ReelError> {
        if !view.is_active() {
            return Err(ReelError::ViewTerminal(view.status));
        }
        view.effects_hash = None;
        view.delta.ops.clear();
        view.transition_to(Status::Aborted)
    }
}

impl fmt::Debug for Kernel {
    /// `Store` is a trait object with no `Debug` bound, so the kernel is
    /// rendered opaquely.  (Restores the `Debug` impl that the `derive`
    /// provided before's `Arc<dyn Store>` field made it impossible.)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics / unwraps are acceptable for test failures"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use reel_spec::{Block, Capability, Hash, Op, OpKind, Status, View};
    use reel_store::MemStore;
    use std::collections::BTreeSet;

    /// Standard kernel-under-test: a fresh in-memory store.
    fn kernel() -> Kernel {
        Kernel::new(Arc::new(MemStore::new()))
    }

    fn pending_hash() -> Hash {
        Hash::from_bytes(*blake3::hash(b"pending-effects-block").as_bytes())
    }

    fn put_op(name: &str) -> Op {
        Op::Put { name: name.to_owned(), hash: Hash::from_bytes([1u8; 32]) }
    }

    // ──: the kernel holds a working store ─────────────────────────

    /// Done-when: the kernel carries a durable backend and a Block
    /// written through `store()` round-trips back by hash.  This is the
    /// oracle that `Kernel::new` actually wired a real `Store`, not a stub.
    #[test]
    fn kernel_holds_working_store() {
        let k = kernel();
        let block = Block::new(b"t-007 store wiring".to_vec()).expect("block");
        let hash = block.hash();

        k.store().write_block(&block).expect("write_block");
        let got = k.store().raw_lookup_by_hash(&hash).expect("raw lookup");

        assert_eq!(got.hash(), hash, "store must round-trip the Block by content hash");
    }

    /// A fresh in-memory backend reports it was not recovered (the recovery
    /// flag is meaningful only for the persistent `RedbStore` on reopen).
    #[test]
    fn fresh_store_not_recovered() {
        assert!(!kernel().store().was_recovered());
    }

    /// `Debug` renders the kernel opaquely (there is no `Store: Debug`
    /// bound).  Pins the manual impl so a no-op mutant is caught.
    #[test]
    fn kernel_debug_names_the_type() {
        let rendered = format!("{:?}", kernel());
        assert!(rendered.contains("Kernel"), "Debug must name the type: {rendered}");
    }

    // ──: fork ─────────────────────────────────────────────────────

    /// Build a Capability from raw parts (test ergonomics).
    fn cap(prefix: &str, ops: &[OpKind], ttl: u64) -> Capability {
        Capability { path_prefix: prefix.to_owned(), op_set: ops.iter().copied().collect(), ttl }
    }

    /// fork accepts a strictly-attenuated child (sub-prefix, sub-op-set,
    /// bounded TTL under an unbounded parent).
    #[test]
    fn fork_accepts_attenuation() {
        let parent = View::root(vec![cap("/", &[OpKind::Read, OpKind::Write], 0)]);
        let mut k = kernel();
        let child = k
            .fork(&parent, vec![cap("/sub", &[OpKind::Read], 100)])
            .expect("attenuation is permitted");
        assert_eq!(child.parent, Some(parent.id), "child records its parent");
        assert!(child.is_active());
    }

    /// I-003: fork rejects a child claiming an op the parent does not grant.
    #[test]
    fn fork_rejects_op_widening() {
        let parent = View::root(vec![cap("/a", &[OpKind::Read], 0)]);
        let mut k = kernel();
        let err = k
            .fork(&parent, vec![cap("/a", &[OpKind::Read, OpKind::Write], 0)])
            .expect_err("op-set widening must be rejected");
        assert!(matches!(err, ReelError::CapabilityViolation(_)), "E-009 on widening");
    }

    /// fork rejects a terminal parent (E-008).
    #[test]
    fn fork_rejects_terminal_parent() {
        let mut parent = View::root(vec![]);
        parent.transition_to(Status::Committed).expect("Active → Committed");
        let mut k = kernel();
        let err = k.fork(&parent, vec![]).expect_err("terminal parent must reject fork");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Committed)));
    }

    /// fork inherits the parent namespace by O(1) `CoW` clone (ADR-0034), and
    /// the child is isolated: mutating its delta does not touch the parent.
    #[test]
    fn fork_inherits_namespace_independently() {
        let mut parent = View::root(vec![]);
        parent.namespace.insert("main".into(), Hash::from_bytes([7u8; 32]));
        let mut k = kernel();
        let mut child = k.fork(&parent, vec![]).expect("fork ok");
        assert_eq!(child.namespace, parent.namespace, "child inherits parent namespace");
        child.delta.ops.push(put_op("child-only.txt"));
        assert!(parent.delta.ops.is_empty(), "parent delta unaffected by child");
    }

    /// Strategy: a (deduplicated, via the `BTreeSet` in `cap`) subset of the
    /// five `OpKind`s.
    fn any_ops() -> impl Strategy<Value = Vec<OpKind>> {
        prop::collection::vec(
            prop_oneof![
                Just(OpKind::Read),
                Just(OpKind::Write),
                Just(OpKind::Delete),
                Just(OpKind::FireB),
                Just(OpKind::FireC),
            ],
            0..5usize,
        )
    }

    proptest! {
        /// I-003 at the kernel surface: `fork` accepts iff the child cap is
        /// covered by the parent per the spec rule — sub-prefix ∧ op-set ⊆ ∧
        /// TTL rule (unbounded parent covers all; bounded parent cannot cover
        /// an unbounded child; else child ≤ parent). The oracle re-encodes
        /// the I-003 rule independently of `validate_narrowing`, so impl
        /// drift — or a mutant that drops a narrowing axis — is caught.
        #[test]
        fn fork_narrowing_matches_i003_rule(
            p_ops in any_ops(),
            c_ops in any_ops(),
            p_ttl in 0u64..50,
            c_ttl in 0u64..50,
            deeper in any::<bool>(),
        ) {
            let p_prefix = "/a";
            // "/a/b" extends "/a" (covered prefix); "/x" does not.
            let c_prefix = if deeper { "/a/b" } else { "/x" };

            let parent = View::root(vec![cap(p_prefix, &p_ops, p_ttl)]);
            let mut k = kernel();
            let result = k.fork(&parent, vec![cap(c_prefix, &c_ops, c_ttl)]);

            // Independent oracle = the I-003 `covers` rule (spec §5.5 / §7.1).
            let prefix_ok = c_prefix.starts_with(p_prefix);
            let ops_ok = c_ops.iter().all(|o| p_ops.contains(o));
            let ttl_ok = match (p_ttl, c_ttl) {
                (0, _) => true,
                (_, 0) => false,
                (p, c) => c <= p,
            };
            let expected_ok = prefix_ok && ops_ok && ttl_ok;

            prop_assert_eq!(
                result.is_ok(), expected_ok,
                "fork narrowing must equal I-003 covers(parent, child)"
            );
        }
    }

    // ── done_when: structural post-conditions ───────────────────────────

    /// I-002 mechanism — after abort, the pending-effects Block reference is
    /// dropped.  Storage GC then reclaims the Block per I-001.
    #[test]
    fn abort_drops_effects_hash() {
        let mut v = View::root(vec![]);
        v.effects_hash = Some(pending_hash());

        let mut k = kernel();
        k.abort(&mut v).expect("Active → Aborted");

        assert!(v.effects_hash.is_none(), "pending-effects reference must be dropped");
        assert!(v.is_aborted(), "View must reach Aborted state");
    }

    /// Done-when: `view.delta` (the Class A buffer) is cleared on abort.
    #[test]
    fn abort_clears_delta_ops() {
        let mut v = View::root(vec![]);
        v.delta.ops.push(put_op("a.txt"));
        v.delta.ops.push(put_op("b.txt"));
        assert_eq!(v.delta.ops.len(), 2);

        let mut k = kernel();
        k.abort(&mut v).expect("Active → Aborted");

        assert!(v.delta.ops.is_empty(), "delta ops must be cleared on abort");
    }

    /// Done-when: abort is synchronous (no async, no await).  Verified by
    /// the fact that we can call it from a non-async test without `block_on`.
    #[test]
    fn abort_is_synchronous() {
        let mut v = View::root(vec![]);
        let mut k = kernel();
        // If `abort` were `async`, this call would not type-check (the
        // returned future would be unused; clippy::let_underscore_must_use
        // is deny in the workspace lints).
        k.abort(&mut v).expect("ok");
        assert!(v.is_aborted());
    }

    /// Aborting a terminal View is a caller bug; return E-008 and leave the
    /// View untouched.
    #[test]
    fn abort_rejects_committed_view() {
        let mut v = View::root(vec![]);
        v.transition_to(Status::Committed).expect("Active → Committed");
        let snapshot = v.clone();

        let mut k = kernel();
        let err = k.abort(&mut v).expect_err("committed View must reject abort");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Committed)));
        assert_eq!(v, snapshot, "View must be unchanged after rejected abort");
    }

    #[test]
    fn abort_rejects_already_aborted_view() {
        let mut v = View::root(vec![]);
        v.transition_to(Status::Aborted).expect("Active → Aborted");
        let snapshot = v.clone();

        let mut k = kernel();
        let err = k.abort(&mut v).expect_err("aborted View must reject re-abort");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Aborted)));
        assert_eq!(v, snapshot, "View must be unchanged after rejected re-abort");
    }

    // ── I-002 invariant proof — Class C path ───────────────────────────

    /// I-002 positive witness — every reachable code path from `abort`
    /// drops `effects_hash` BEFORE any drain could occur.  We model the
    /// pending Class C payload as the hash that would have been drained,
    /// and assert the kernel never observes it again.
    ///
    /// (When lands and `commit::drain` becomes real, it will gate on
    /// `view.effects_hash.is_some()`; this test pins that the gate evaluates
    /// to `false` after abort.)
    #[test]
    fn i_002_no_class_c_reachable_after_abort() {
        let pending = pending_hash();
        let mut v = View::root(vec![]);
        v.effects_hash = Some(pending);

        // Before abort the pending-effects Block is reachable from the View.
        assert_eq!(v.effects_hash, Some(pending), "precondition: effects buffered");

        let mut k = kernel();
        k.abort(&mut v).expect("ok");

        // After abort the View no longer references the Block; the drain
        // gate will read `None` and skip all Class B/C fires.
        assert!(v.effects_hash.is_none(), "I-002: no path to drain Class C");
        assert!(v.is_aborted(), "I-002: drain gate also requires Active");
    }

    /// I-002 negative witness — once aborted, no further verb may re-enter
    /// the View and re-attach an effects-hash.  `transition_to` already
    /// rejects Aborted → anything; this exercises the kernel-level
    /// guarantee that there is no abort-then-resubmit path.
    #[test]
    fn i_002_no_resubmit_after_abort() {
        let mut v = View::root(vec![]);
        v.effects_hash = Some(pending_hash());

        let mut k = kernel();
        k.abort(&mut v).expect("ok");

        // A second abort fails — the State Machine never returns to Active.
        let err = k.abort(&mut v).expect_err("Aborted → Aborted is rejected");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Aborted)));

        // Direct status manipulation is the only way to leave Aborted; that
        // is the View invariant, not the kernel's concern.  Confirm the
        // kernel still refuses on any non-Active input.
        v.status = Status::Active;
        v.effects_hash = Some(pending_hash());
        k.abort(&mut v).expect("Active again → Aborted");
        assert!(v.is_aborted());
        assert!(v.effects_hash.is_none());
    }

    // ── Property: abort normal form ────────────────────────────────────

    /// After a successful abort, `v.delta` / `v.effects_hash` / `v.status`
    /// are independent of whatever was buffered before — the post-state is
    /// determined by abort alone.
    #[test]
    fn abort_post_state_is_normal_form() {
        let mut a = View::root(vec![]);
        a.delta.ops.push(put_op("only-in-a"));
        a.effects_hash = Some(pending_hash());

        let mut b = View::root(vec![Capability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 0,
        }]);
        b.delta.ops.push(put_op("only-in-b-1"));
        b.delta.ops.push(put_op("only-in-b-2"));
        b.effects_hash = Some(Hash::from_bytes([0xAB; 32]));

        let mut k = kernel();
        k.abort(&mut a).expect("ok");
        k.abort(&mut b).expect("ok");

        // The triple (status, delta.ops empty, effects_hash) is the
        // normal form abort guarantees.  `caps`, `namespace`, `id`,
        // `parent`, `delta.base_hash` are intentionally NOT cleared
        // (abort is about pending state, not identity / inheritance).
        for v in [&a, &b] {
            assert!(v.is_aborted());
            assert!(v.delta.ops.is_empty());
            assert!(v.effects_hash.is_none());
        }
    }
}
