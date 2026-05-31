//! Throwaway walking-skeleton kernel.
//!
//! Implements the minimum [`SkeletonKernel`] needed to exercise the
//! `fork → submit_* → commit / abort` lifecycle end-to-end against the
//! [`reel_effects::fake`] adapter and a [`reel_store::MemStore`].  This
//! module is gated behind the `walking-skeleton` Cargo feature and is
//! **not** considered part of the v0.1 protocol surface.
//!
//! # Why a separate type
//!
//! The production [`crate::Kernel`] is `#[derive(Clone, Copy, Debug,
//! Default)]` ().  Holding a [`Store`] handle and a per-View
//! effects buffer is incompatible with `Copy`, so a separate
//! `SkeletonKernel` lets ship without disturbing's tests or
//! waiting for the proper / / implementations to land.
//! Real `fork`/`commit` will be re-derived in.. with full
//! Verus/Bolero coverage; THIS module is throwaway scaffolding for
//! spec calibration.
//!
//! # Spec calibration findings (Round 0)
//!
//!
//! enumerated gaps surfaced while writing this skeleton.  In short:
//!
//! - The spec specifies `View.effects_hash: Option<Hash>` but offers no
//!   normative schema for the pending-effects Block payload, leaving
//!   the in-memory `Box<dyn ClassB::fire>` future representation
//!   without a clear round-trip path. The skeleton keeps a sidecar
//!   map (`ViewId → PendingEffects`) and uses the effects Block only
//!   as a content-addressed digest of the descriptor list.
//! - `Kernel::commit` is required to return a `Ref` per
//!   `spec/spec.md` §6.2, but `Ref` carries a `version_constraint`
//!   that has no spec-level definition for the commit-produced Ref —
//!   the skeleton sets it to `None`.
//! - The spec demands that `commit` "merge `v.delta` into the parent"
//!   but neither §5.4 nor §6.2 defines parent-merging when the parent
//!   is the root namespace (no parent View). The skeleton treats root
//!   commits as direct namespace updates.

use std::collections::HashMap;
use std::sync::Arc;

use ciborium::ser::into_writer as cbor_into_writer;
use reel_effects::{
    BoxedA, BoxedB, BoxedC, ClassB, ClassC, FireCapability,
    fake::{FakeFsWrite, FakeSlackPost, FakeSlackUpdate},
};
use reel_spec::{
    Block, Capability, EffectDescriptor, Hash, Op, ReelError, Status, View, ViewId,
};
use reel_store::{MemStore, Namespace, Store, StoreError};

/// Pending non-Class-A effects for a single View.
///
/// Class A effects live in `view.delta.ops`; B+C effects live in this
/// sidecar (the spec calibration report explains why the spec's
/// `effects_hash` cannot itself carry the in-memory `Box<dyn Fire>`).
#[derive(Debug, Default)]
struct PendingEffects {
    class_b: Vec<BoxedB>,
    class_c: Vec<BoxedC>,
    /// Descriptors used to compute `effects_hash` deterministically.
    /// Built incrementally as `submit_b` / `submit_c` are called.
    descriptors: Vec<EffectDescriptor>,
    /// Counter for the next `effect_id` assigned within this view.
    next_effect_id: u64,
}

/// Walking-skeleton kernel — `fork` / `submit_a` / `submit_b` /
/// `submit_c` / `commit` / `abort` over a [`MemStore`] and the
/// [`reel_effects::fake`] adapter.
///
/// One `SkeletonKernel` per process (single-threaded per ADR-0008).
/// Owns the [`Store`] and the per-View effects buffer.
#[derive(Debug)]
pub struct SkeletonKernel {
    store: Arc<MemStore>,
    /// Sidecar buffer indexed by `View.id`. Entry created on first
    /// `submit_*` for that view, cleared on `commit` / `abort`.
    buffers: HashMap<ViewId, PendingEffects>,
    /// Namespace mirroring `store.list_refs()` at the kernel level so
    /// the skeleton can call `store.lookup_by_hash(h, &namespace)`
    /// without an extra round-trip.
    namespace: Namespace,
}

impl SkeletonKernel {
    /// Construct a fresh `SkeletonKernel` backed by the given
    /// in-memory store.
    #[must_use]
    pub fn new(store: Arc<MemStore>) -> Self {
        Self { store, buffers: HashMap::new(), namespace: Namespace::default() }
    }

    /// Borrow the kernel's namespace (mostly for skeleton diagnostics).
    #[must_use]
    pub const fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// Borrow the underlying store.
    #[must_use]
    pub fn store(&self) -> &MemStore {
        &self.store
    }

    /// Bind a Ref `name → hash` outside any View — used by the example
    /// to seed an initial `main` ref before the first fork.  Skipping
    /// I-001 enforcement for the namespace mirror; the underlying
    /// store still validates via `put_ref`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] (wrapping store I/O errors) or
    /// [`ReelError::BlockNotFound`] when the Block has not been
    /// pre-written via [`Self::write_block`].
    pub fn seed_ref(&mut self, name: &str, hash: Hash) -> Result<(), ReelError> {
        self.store
            .put_ref(name, &hash, None)
            .map_err(|e| ReelError::Storage(format!("seed_ref({name}): {e}")))?;
        self.namespace.insert(name.into(), hash);
        Ok(())
    }

    /// Persist a [`Block`] in the store.  The skeleton example pre-writes
    /// file-content Blocks before `submit_a` because Class A `apply()`
    /// only produces an [`reel_spec::Op::Put`] referencing the hash.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] wrapping any store I/O failure.
    pub fn write_block(&self, block: &Block) -> Result<(), ReelError> {
        self.store
            .write_block(block)
            .map_err(|e| ReelError::Storage(format!("write_block({:?}): {e}", block.hash())))
    }

    /// Fork a child View from `parent`.
    ///
    /// **Walking-skeleton minimum**: replicates `View::child_of` semantics.
    /// The real implementation will own the parent-id → View
    /// resolution map and emit `E-005 ForkDepthExceeded` when the depth
    /// budget is exhausted.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) when `parent` is not
    /// in `Active`, or [`ReelError::CapabilityViolation`] (E-009) when
    /// `attenuated_caps` amplifies parent authority.
    pub fn fork(
        &mut self,
        parent: &View,
        attenuated_caps: Vec<Capability>,
    ) -> Result<View, ReelError> {
        View::child_of(parent, attenuated_caps)
    }

    /// Submit a Class A effect.  Walking-skeleton path: apply the effect
    /// to the View's delta and observe the resulting `Op`.
    ///
    /// Class A does NOT enter the kernel's pending-effects buffer (per
    /// `spec/spec.md` §5.7.1 and `effect-class-rules.md` §1).
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) when `view` is not
    /// `Active`.
    pub fn submit_a(&mut self, view: &mut View, effect: FakeFsWrite) -> Result<(), ReelError> {
        if !view.is_active() {
            return Err(ReelError::ViewTerminal(view.status));
        }
        let boxed = BoxedA::new(effect);
        view.delta.ops.push(boxed.apply());
        Ok(())
    }

    /// Submit a Class B effect.  Walking-skeleton path: append to the
    /// sidecar buffer and re-hash the descriptor list.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) when `view` is not
    /// `Active`.
    pub fn submit_b(&mut self, view: &mut View, effect: FakeSlackUpdate) -> Result<(), ReelError> {
        if !view.is_active() {
            return Err(ReelError::ViewTerminal(view.status));
        }
        let pending = self.buffers.entry(view.id).or_default();
        let effect_id = pending.next_effect_id;
        pending.next_effect_id = pending.next_effect_id.saturating_add(1);
        let version_ref = effect.version_ref();
        pending.descriptors.push(EffectDescriptor::B {
            effect_id,
            version_ref,
            op: serde_json::json!({"adapter": "fake-slack", "op": "update"}),
        });
        pending.class_b.push(BoxedB::new(effect));
        let new_hash = compute_effects_hash(&pending.descriptors)?;
        let descriptor_block = build_effects_block(&pending.descriptors)?;
        // I-001: write the effects descriptor Block so that `lookup_by_hash`
        // can resolve it when the namespace later names it.  The skeleton
        // does NOT actually put the effects-hash into the namespace
        // (commit moves it to the committed-effects log per spec §5.6);
        // until then it remains reachable via the in-memory View handle.
        self.store
            .write_block(&descriptor_block)
            .map_err(|e| ReelError::Storage(format!("submit_b effects-block: {e}")))?;
        view.effects_hash = Some(new_hash);
        Ok(())
    }

    /// Submit a Class C effect.  Walking-skeleton path: append to the
    /// sidecar buffer and re-hash the descriptor list.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) when `view` is not
    /// `Active`.
    pub fn submit_c(&mut self, view: &mut View, effect: FakeSlackPost) -> Result<(), ReelError> {
        if !view.is_active() {
            return Err(ReelError::ViewTerminal(view.status));
        }
        let pending = self.buffers.entry(view.id).or_default();
        let effect_id = pending.next_effect_id;
        pending.next_effect_id = pending.next_effect_id.saturating_add(1);
        pending.descriptors.push(EffectDescriptor::C { effect_id, op: effect.describe() });
        pending.class_c.push(BoxedC::new(effect));
        let new_hash = compute_effects_hash(&pending.descriptors)?;
        let descriptor_block = build_effects_block(&pending.descriptors)?;
        self.store
            .write_block(&descriptor_block)
            .map_err(|e| ReelError::Storage(format!("submit_c effects-block: {e}")))?;
        view.effects_hash = Some(new_hash);
        Ok(())
    }

    /// Commit `view`: drain pending effects in (Class A → Class B →
    /// Class C) order, atomically move the namespace ref `name` to the
    /// freshly-built Delta Block's hash.
    ///
    /// **Walking-skeleton minimum** — the production `commit` ( +
    ///) will own:
    ///   - Class B `assert_precondition` checks before any `fire()`.
    ///   - Multi-Ref atomic commit per spec §6.2.
    ///   - `commit` cancel-safety / drain timeout handling.
    ///
    /// `name` is the namespace Ref that will be updated to point to the
    /// committed Delta Block.  Per the spec calibration report, the
    /// production `commit` will derive `name` from the View's parent
    /// chain; the skeleton requires the caller to pass it explicitly.
    ///
    /// # Errors
    ///
    /// - [`ReelError::ViewTerminal`] (E-008) when `view` is not `Active`.
    /// - [`ReelError::Storage`] (E-007) on any store I/O failure.
    #[allow(
        clippy::future_not_send,
        reason = "FireCapability is `!Send` by design (see fire_capability.rs); the commit future therefore captures a non-Send borrow.  Per ADR-0008 the kernel is single-threaded so this is acceptable.  Production commit will inherit the same constraint."
    )]
    pub async fn commit(&mut self, view: &mut View, name: &str) -> Result<Hash, ReelError> {
        if !view.is_active() {
            return Err(ReelError::ViewTerminal(view.status));
        }

        // --- Phase 1 — Class B precondition checks (per spec §6.2 v1) ---
        let pending = self.buffers.remove(&view.id).unwrap_or_default();
        // PreconditionCtx is #[non_exhaustive]; the walking-skeleton has no
        // budget tracking so we skip the precondition phase if the kernel
        // cannot construct a context.  Per spec calibration report #5
        // the production kernel will need a `PreconditionCtx::new()` ctor.
        // For the skeleton: per spec §6.2, "the implementation MUST verify
        // the version reference is still valid"; FakeSlackUpdate always
        // returns Ok so the precondition phase is a no-op.  We DO call
        // `fire` below, which is the load-bearing action for the demo.
        for _ in &pending.class_b {
            // No-op precondition phase (skeleton calibration: gap #5).
        }

        // --- Phase 2 — atomic drain ---
        // Per `effect-class-rules.md` §6 + ADR-0015, Class B fire before
        // Class C; the walking-skeleton honours that order.
        let cap = FireCapability::new_for_skeleton();

        // Class A materialisation: encode the Delta as a Block and write
        // it so the resulting Ref is I-001-reachable.
        let delta_block = build_delta_block(&view.delta)?;
        let delta_hash = delta_block.hash();
        self.store
            .write_block(&delta_block)
            .map_err(|e| ReelError::Storage(format!("commit delta: {e}")))?;

        // Class B drain.
        for b in pending.class_b {
            let _receipt = b.fire(&cap).await?;
        }

        // Class C drain — fired LAST per ADR-0015.  After this point the
        // outside world has observed the effect; reel cannot undo it.
        for c in pending.class_c {
            let _receipt = c.fire(&cap).await?;
        }

        // --- Phase 3 — atomic ref move ---
        // Place the Delta Block hash into the namespace under `name`.
        self.store
            .put_ref(name, &delta_hash, None)
            .map_err(|e| ReelError::Storage(format!("commit put_ref({name}): {e}")))?;
        self.namespace.insert(name.into(), delta_hash);

        // **Calibration workaround for finding #6** (reachability walker
        // cannot decode Block envelopes as Delta payloads).  Pin each
        // `Op::Put.hash` into the namespace under a synthetic
        // `__delta-put/<name>` Ref so the I-001 walker can reach it from
        // root.  Production will instead change the walker (or the
        // store) so Block-envelope payloads are unwrapped before edge
        // extraction.
        for op in &view.delta.ops {
            if let Op::Put { name: put_name, hash } = op {
                // TODO: replace `__delta-put/<name>` pinning with a
                // proper reachability walker that unwraps Block-envelope
                // payloads. Per `mid-task-scope-protocol.md` category D,
                // this allocation stays in (orthogonal to the
                // namespace-shape fix).
                let pinned = format!("__delta-put/{put_name}");
                self.store
                    .put_ref(&pinned, hash, None)
                    .map_err(|e| ReelError::Storage(format!("commit pin {pinned}: {e}")))?;
                self.namespace.insert(pinned.into(), *hash);
            }
        }

        // Move View → Committed.
        view.effects_hash = None;
        view.transition_to(Status::Committed)?;

        Ok(delta_hash)
    }

    /// Abort `view`: synchronously drop its sidecar buffer and call the
    /// production [`Kernel::abort`] for the View-level transition.
    ///
    /// **I-002**: no `FireCapability` is ever constructed on this path.
    /// The Class C effects in the sidecar buffer are dropped without
    /// being fired.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) when `view` is not
    /// `Active`.
    pub fn abort(&mut self, view: &mut View) -> Result<(), ReelError> {
        // Drop the per-View sidecar **before** the View transition so
        // there is no path to re-enter and fire (matches's
        // ordering proof for `effects_hash`).
        let _pending = self.buffers.remove(&view.id);
        crate::Kernel::new(self.store.clone()).abort(view)
    }

    /// Returns the count of pending Class B + Class C effects for `view_id`
    /// in this kernel's sidecar buffer.  Used by the example to assert the
    /// drain happened.
    #[must_use]
    pub fn pending_count(&self, view_id: ViewId) -> usize {
        self.buffers.get(&view_id).map_or(0, |p| p.class_b.len().saturating_add(p.class_c.len()))
    }

    /// Returns true iff `hash` is I-001-reachable from the current namespace.
    /// The walking-skeleton example uses this to print `reachable: true`
    /// for each freshly-committed Block hash (proves I-001 holds for the
    /// commit path).
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] when the underlying store reports
    /// an I/O failure other than `BlockNotFound` / `ReachabilityViolation`.
    pub fn reachable(&self, hash: &Hash) -> Result<bool, ReelError> {
        match self.store.lookup_by_hash(hash, &self.namespace) {
            Ok(_) => Ok(true),
            Err(StoreError::Protocol(
                ReelError::BlockNotFound(_) | ReelError::ReachabilityViolation(_),
            )) => Ok(false),
            Err(e) => Err(ReelError::Storage(format!("reachable check: {e}"))),
        }
    }
}

/// Build a [`Block`] whose payload is the CBOR encoding of the given Delta.
///
/// I-001 reachability walker (in `reel_store::reachability`) understands
/// this shape and follows `base_hash` + `Op::Put` edges through it.
fn build_delta_block(delta: &reel_spec::Delta) -> Result<Block, ReelError> {
    let mut cbor = Vec::new();
    cbor_into_writer(delta, &mut cbor)
        .map_err(|e| ReelError::Storage(format!("CBOR encode delta: {e}")))?;
    Block::new(cbor)
}

/// Build a [`Block`] whose payload is the CBOR encoding of the effects
/// descriptor list.  This is the content-addressed representation of
/// `View.effects_hash`.
fn build_effects_block(descriptors: &[EffectDescriptor]) -> Result<Block, ReelError> {
    let mut cbor = Vec::new();
    cbor_into_writer(descriptors, &mut cbor)
        .map_err(|e| ReelError::Storage(format!("CBOR encode effects: {e}")))?;
    Block::new(cbor)
}

/// Compute `View.effects_hash = hash(Block(cbor(descriptors)))` —
/// content-addressed across the descriptor list.
fn compute_effects_hash(descriptors: &[EffectDescriptor]) -> Result<Hash, ReelError> {
    let block = build_effects_block(descriptors)?;
    Ok(block.hash())
}
