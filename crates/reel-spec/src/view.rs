//! Workspace closure — the reel `View` type.
//!
//! A [`View`] is the central workspace abstraction in reel.
//! It corresponds to a Snapshot Isolation transaction (Berenson et al.
//! 1995): reads from a fixed snapshot, writes deferred to commit.
//!
//! The distinctive structural commitment of reel is that the pending
//! effect buffer is a content-addressed Block referenced by
//! `effects_hash: Option<Hash>`.
//! This means pending effects are immutable, addressable, and shareable.
//!
//! See `spec/spec.md` §5.6.
//!
//! # Invariants
//!
//! - I-002: If `abort` is invoked on a View, no Class C effect in its
//!   `effects_hash` must have fired or will fire.
//! - I-003: Capability narrowing — derived `View.caps` are a sub-lattice
//!   of the delegator's `caps` (enforced at [`View::child_of`]).
//! - I-003-perf: `fork` is O(1) (namespace inherited by reference, COW).
//! - I-008: `View.depth ≤ MAX_FORK_DEPTH` (fork depth bound; convenience).
//!
//! # Complexity
//!
//! - `fork`: O(1) — namespace is reference-counted, not copied.
//! - `abort`: O(1) — drops the `effects_hash` reference.
//!
//! # References
//!
//! - Berenson, H., et al. (1995). A Critique of ANSI SQL Isolation Levels.
//!   SIGMOD. (View ≈ Snapshot Isolation transaction.)
//! - Wang, C., Zheng, Y. (2026). Fork, Explore, Commit. arXiv 2602.08199v2.
//! - Miller, M., Yee, K., Shapiro, J. (2003). Capability Myths Demolished.
//!   (I-003 narrowing lattice realised in [`View::child_of`].)

use crate::{Capability, Delta, Hash, Namespace, ReelError};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Maximum nesting depth of forked Views (convenience bound I-008).
///
/// Matches `spec/spec.md` §12 default for `MAX_FORK_DEPTH`.
/// Used by [`View::depth`] callers to enforce I-008 via [`ReelError::ForkDepthExceeded`].
pub const MAX_FORK_DEPTH: u32 = 64;

/// A workspace closure — reel's core transaction primitive.
///
/// A `View` encapsulates:
///
/// - `namespace`: the set of [`crate::Ref`]s visible from inside the View,
///   each resolved to a Block hash.
/// - `delta`: the View's pending mutations to its namespace.
/// - `caps`: the View's authority set.
/// - `effects_hash`: an optional content-addressed reference to the
///   pending Class B and C effects Block.
/// - `status`: the View's lifecycle state (`active`, `committed`, or
///   `aborted`).
/// - `parent`: the identifier of the parent View, or `None` for a root View.
///
/// The key structural insight: `effects_hash: Option<Hash>` makes pending
/// effects a content-addressed Block, which is reel's distinctive
/// contribution (see `spec/spec.md` §5.6 and ADR-0025 §"Positive").
///
/// # Construction
///
/// Use [`View::root`] for a top-level View, [`View::child_of`] for a
/// forked child View.  Direct struct construction is permitted in tests
/// and serde paths where field values are already trusted.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{View, Status};
///
/// let root = View::root(vec![]);
/// assert!(root.is_active());
/// assert!(root.parent.is_none());
/// assert_eq!(root.status, Status::Active);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Unique identifier for this View (`UUIDv7`).
    pub id: ViewId,

    /// Map from namespace name to the Block hash currently bound there.
    ///
    /// This is the effective namespace visible inside the View.
    ///
    /// The type is a copy-on-write [`Namespace`] handle:
    /// `clone()` is `O(1)` (refcount bump on the underlying B+tree root),
    /// realising I-003-perf at the data-structure layer. See ADR-0034.
    pub namespace: Namespace,

    /// Pending mutations to the namespace (one layer per View).
    pub delta: Delta,

    /// The View's authority set (Capabilities constraining what this
    /// View may do, per I-003).
    pub caps: Vec<Capability>,

    /// Hash of the pending-effects Block (Class B + C effects awaiting
    /// commit), or `None` if no effects have been buffered.
    ///
    /// This is the "Effect Buffer = Block" structural commitment.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effects_hash: Option<Hash>,

    /// Lifecycle state of this View.
    pub status: Status,

    /// Identifier of the parent View.
    ///
    /// `None` for a root View (directly forked from the kernel's root
    /// namespace, not from another View).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<ViewId>,
}

impl View {
    /// Constructs a fresh root View with the given Capability set.
    ///
    /// A root View has:
    /// - a fresh `UUIDv7` id;
    /// - an empty `namespace`;
    /// - an empty `delta` (anchored at the zero `Hash`);
    /// - the caller-supplied `caps`;
    /// - `effects_hash = None`;
    /// - `status = Active`;
    /// - `parent = None`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Capability, OpKind, View};
    /// use std::collections::BTreeSet;
    ///
    /// let cap = Capability {
    ///     path_prefix: "/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read]),
    ///     ttl: 0,
    /// };
    /// let v = View::root(vec![cap]);
    /// assert!(v.is_active());
    /// assert!(v.parent.is_none());
    /// assert_eq!(v.caps.len(), 1);
    /// ```
    #[must_use]
    pub fn root(caps: Vec<Capability>) -> Self {
        Self {
            id: ViewId::new_v7(),
            namespace: Namespace::new(),
            delta: Delta { base_hash: Hash::from_bytes([0u8; 32]), ops: vec![] },
            caps,
            effects_hash: None,
            status: Status::Active,
            parent: None,
        }
    }

    /// Constructs a child View forked from `parent`, with optionally
    /// attenuated capabilities.
    ///
    /// The child View inherits `parent.namespace` by reference-counted
    /// borrow (per `spec/spec.md` §5.4): cloning the [`Namespace`] handle
    /// is an `O(1)` refcount bump on the underlying B+tree root, never
    /// a deep copy. This satisfies I-003-perf per ADR-0034.
    ///
    /// The child has:
    /// - a fresh `UUIDv7` id;
    /// - `parent.namespace` cloned;
    /// - an empty `delta` anchored at the zero `Hash`;
    /// - the caller-supplied `attenuated_caps`;
    /// - `effects_hash = None`;
    /// - `status = Active`;
    /// - `parent = Some(parent.id)`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] (E-008) if `parent` is not
    /// in the `Active` state.
    ///
    /// Returns [`ReelError::CapabilityViolation`] (E-009) if
    /// `attenuated_caps` is not a sub-lattice of `parent.caps` per
    /// I-003 (every child cap must be covered by some parent cap with
    /// at least the same `op_set`, a `path_prefix` that is a prefix of
    /// the child's, and a `ttl` that is at least the child's).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Capability, OpKind, View};
    /// use std::collections::BTreeSet;
    ///
    /// let parent_cap = Capability {
    ///     path_prefix: "ws/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
    ///     ttl: 3600,
    /// };
    /// let parent = View::root(vec![parent_cap]);
    ///
    /// // Narrowing: read-only over a sub-prefix.
    /// let child_cap = Capability {
    ///     path_prefix: "ws/docs/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read]),
    ///     ttl: 1800,
    /// };
    /// let child = View::child_of(&parent, vec![child_cap])
    ///     .expect("narrowing is permitted");
    /// assert_eq!(child.parent, Some(parent.id));
    /// ```
    pub fn child_of(parent: &Self, attenuated_caps: Vec<Capability>) -> Result<Self, ReelError> {
        if !parent.is_active() {
            return Err(ReelError::ViewTerminal(parent.status));
        }
        validate_narrowing(&parent.caps, &attenuated_caps)?;
        Ok(Self {
            id: ViewId::new_v7(),
            namespace: parent.namespace.clone(),
            delta: Delta { base_hash: Hash::from_bytes([0u8; 32]), ops: vec![] },
            caps: attenuated_caps,
            effects_hash: None,
            status: Status::Active,
            parent: Some(parent.id),
        })
    }

    /// Returns `true` iff this View is in the `Active` (non-terminal) state.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::View;
    ///
    /// let v = View::root(vec![]);
    /// assert!(v.is_active());
    /// ```
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self.status, Status::Active)
    }

    /// Returns `true` iff this View is in the `Committed` state.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Status, View};
    ///
    /// let mut v = View::root(vec![]);
    /// v.status = Status::Committed;
    /// assert!(v.is_committed());
    /// ```
    #[must_use]
    pub const fn is_committed(&self) -> bool {
        matches!(self.status, Status::Committed)
    }

    /// Returns `true` iff this View is in the `Aborted` state.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Status, View};
    ///
    /// let mut v = View::root(vec![]);
    /// v.status = Status::Aborted;
    /// assert!(v.is_aborted());
    /// ```
    #[must_use]
    pub const fn is_aborted(&self) -> bool {
        matches!(self.status, Status::Aborted)
    }

    /// Returns `true` iff this View is in a terminal state
    /// (`Committed` or `Aborted`).
    ///
    /// Operations on a terminal View MUST be rejected with E-008
    /// (`ViewTerminal`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Status, View};
    ///
    /// let mut v = View::root(vec![]);
    /// v.status = Status::Aborted;
    /// assert!(v.is_terminal());
    /// ```
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self.status, Status::Committed | Status::Aborted)
    }

    /// Atomically transitions this View to a new `Status`.
    ///
    /// Only `Active → Committed` and `Active → Aborted` are legal.
    /// All other transitions return [`ReelError::ViewTerminal`] (E-008).
    /// In particular:
    ///
    /// - Transitioning from a terminal state is rejected with the
    ///   current `status` recorded in the error.
    /// - Transitioning `Active → Active` is also rejected (no-op
    ///   transitions are not part of the surface).
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ViewTerminal`] when the transition is not
    /// `Active → Committed` or `Active → Aborted`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Status, View};
    ///
    /// let mut v = View::root(vec![]);
    /// v.transition_to(Status::Committed).expect("Active → Committed");
    /// assert!(v.is_committed());
    /// ```
    pub const fn transition_to(&mut self, new_status: Status) -> Result<(), ReelError> {
        match (self.status, new_status) {
            (Status::Active, Status::Committed | Status::Aborted) => {
                self.status = new_status;
                Ok(())
            }
            _ => Err(ReelError::ViewTerminal(self.status)),
        }
    }

    /// Computes this View's depth in the parent chain.
    ///
    /// Returns `0` for a root View (`parent == None`).  Otherwise walks
    /// `parent` references through the supplied `lookup` closure, which
    /// maps a [`ViewId`] to the corresponding [`View`] (or returns
    /// [`ReelError::ViewTerminal`] / [`ReelError::Storage`] if the
    /// parent is unknown to the caller).
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::ForkDepthExceeded`] if walking the parent
    /// chain reaches a depth greater than [`MAX_FORK_DEPTH`] without
    /// terminating (defends I-008 against malformed chains).
    ///
    /// Propagates any error returned by `lookup` (typically
    /// [`ReelError::Storage`] when the parent has been GC'd or the
    /// caller's lookup table is incomplete).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{View, ViewId};
    ///
    /// let root = View::root(vec![]);
    /// // A root View has depth 0 even when the lookup is empty.
    /// let d = root.depth(|_id: ViewId| panic!("lookup must not be called for root"))
    ///     .expect("root depth");
    /// assert_eq!(d, 0);
    /// ```
    pub fn depth<F>(&self, mut lookup: F) -> Result<u32, ReelError>
    where
        F: FnMut(ViewId) -> Result<Self, ReelError>,
    {
        let mut depth: u32 = 0;
        let mut current_parent = self.parent;
        while let Some(parent_id) = current_parent {
            depth = depth.checked_add(1).ok_or(ReelError::ForkDepthExceeded(u32::MAX))?;
            if depth > MAX_FORK_DEPTH {
                return Err(ReelError::ForkDepthExceeded(depth));
            }
            let parent = lookup(parent_id)?;
            current_parent = parent.parent;
        }
        Ok(depth)
    }
}

/// Validates that `child_caps` is a sub-lattice of `parent_caps` per I-003.
///
/// For every `c` in `child_caps`, there MUST exist a `p` in `parent_caps`
/// such that:
///
/// - `c.op_set ⊆ p.op_set` (sub-set);
/// - `c.path_prefix` starts with `p.path_prefix` (sub-prefix; equal counts);
/// - `c.ttl ≤ p.ttl`, with `0` treated as unbounded.  When the parent
///   has `ttl = 0` (unbounded), any child `ttl` is permitted.  When the
///   parent has a positive `ttl`, the child's `ttl` must be positive and
///   at most the parent's.
///
/// An empty `child_caps` is always valid (no authority claimed).
/// An empty `parent_caps` rejects any non-empty `child_caps`.
fn validate_narrowing(
    parent_caps: &[Capability],
    child_caps: &[Capability],
) -> Result<(), ReelError> {
    for child in child_caps {
        if parent_caps.iter().any(|parent| covers(parent, child)) {
            continue;
        }
        // No parent covers; build a diagnostic naming each widening
        // axis against the closest parent (fewest violations). This is
        // load-bearing for caller debuggability — see G-04 calibration
        // closure: bindings classify by E-009, but a human needs the
        // axis to fix the call site.
        let diag = diagnose_widening(parent_caps, child);
        return Err(ReelError::CapabilityViolation(diag));
    }
    Ok(())
}

/// Returns `true` iff `parent` covers `child` per the I-003 narrowing rules.
fn covers(parent: &Capability, child: &Capability) -> bool {
    if !child.path_prefix.starts_with(&parent.path_prefix) {
        return false;
    }
    if !child.op_set.is_subset(&parent.op_set) {
        return false;
    }
    // ttl = 0 means unbounded; otherwise positive seconds.
    match (parent.ttl, child.ttl) {
        (0, _) => true,  // unbounded parent covers anything
        (_, 0) => false, // bounded parent cannot cover unbounded child
        (p_ttl, c_ttl) => c_ttl <= p_ttl,
    }
}

/// Builds a per-axis diagnostic when no parent covers `child`.
///
/// Strategy: find the parent with the most matching axes (path prefix
/// being the canonical "intent" axis) and report exactly which axes
/// fail, naming the offending values. Falls back to a generic "no
/// parent" message when `parent_caps` is empty.
fn diagnose_widening(parent_caps: &[Capability], child: &Capability) -> String {
    let child_fmt = format!(
        "{{path_prefix={:?}, op_set={:?}, ttl={}}}",
        child.path_prefix, child.op_set, child.ttl
    );
    let Some(closest) = closest_parent(parent_caps, child) else {
        return format!(
            "I-003 violation: parent capability set is empty; \
             child {child_fmt} cannot be derived"
        );
    };
    let mut failures: Vec<String> = Vec::new();
    if !child.path_prefix.starts_with(&closest.path_prefix) {
        failures.push(format!(
            "path_prefix {:?} is not under parent prefix {:?}",
            child.path_prefix, closest.path_prefix
        ));
    }
    if !child.op_set.is_subset(&closest.op_set) {
        let extra: Vec<_> = child.op_set.difference(&closest.op_set).collect();
        failures.push(format!(
            "op_set adds {extra:?} not present in parent op_set {:?}",
            closest.op_set
        ));
    }
    match (closest.ttl, child.ttl) {
        (0, _) | (_, _) if closest.ttl != 0 && child.ttl == 0 => {
            failures.push(format!("ttl=0 (unbounded) exceeds parent ttl={}", closest.ttl));
        }
        (p_ttl, c_ttl) if p_ttl != 0 && c_ttl > p_ttl => {
            failures.push(format!("ttl={c_ttl} exceeds parent ttl={p_ttl}"));
        }
        _ => {}
    }
    format!(
        "I-003 violation: child capability {child_fmt} widens parent \
         {{path_prefix={:?}, op_set={:?}, ttl={}}}: {}",
        closest.path_prefix,
        closest.op_set,
        closest.ttl,
        if failures.is_empty() {
            "(no axis named; check derivation)".to_string()
        } else {
            failures.join("; ")
        }
    )
}

/// Picks the parent capability whose `path_prefix` best matches the
/// child's (longest common prefix). Used to attribute violations to a
/// specific parent intent.
fn closest_parent<'a>(parent_caps: &'a [Capability], child: &Capability) -> Option<&'a Capability> {
    parent_caps.iter().max_by_key(|p| {
        // Score by (prefix-prefixes-child? then length of parent prefix)
        // so the most specific covering prefix wins; otherwise the
        // longest common prefix.
        let prefix_match = if child.path_prefix.starts_with(&p.path_prefix) {
            p.path_prefix.len() + 1_000_000
        } else {
            common_prefix_len(&p.path_prefix, &child.path_prefix)
        };
        prefix_match
    })
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Lifecycle state of a [`View`].
///
/// A View has exactly one state at any point.
/// `Active` is the only non-terminal state; `Committed` and `Aborted`
/// are terminal.
/// Operations on terminal Views MUST return `E-008 ViewTerminal`.
///
/// Canonical names from `spec/spec.md` §5.6.1 and the conformance
/// vectors in `spec/conformance/`.
///
/// # Examples
///
/// ```rust
/// use reel_spec::Status;
///
/// let s = Status::Active;
/// assert!(!s.is_terminal());
/// assert!(Status::Committed.is_terminal());
/// assert!(Status::Aborted.is_terminal());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The View accepts further verbs (`fork`, `submit_*`, `commit`,
    /// `abort`). This is the only non-terminal state.
    Active,

    /// Terminal: the View's effects have been flushed and its Delta
    /// merged into the parent. No further verbs are accepted.
    Committed,

    /// Terminal: the View's pending state was discarded. No further
    /// verbs are accepted.
    Aborted,
}

impl Status {
    /// Returns `true` iff this is a terminal state (`Committed` or
    /// `Aborted`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Status;
    /// assert!(!Status::Active.is_terminal());
    /// assert!(Status::Committed.is_terminal());
    /// assert!(Status::Aborted.is_terminal());
    /// ```
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Aborted)
    }
}

/// A unique identifier for a [`View`] (`UUIDv7`).
///
/// `ViewId` wraps a [`uuid::Uuid`] generated with `UUIDv7` for monotonic,
/// sortable uniqueness.
///
/// # Examples
///
/// ```rust
/// use reel_spec::ViewId;
///
/// let id = ViewId::new_v7();
/// let s = id.to_string();
/// assert_eq!(s.len(), 36); // standard UUID format
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewId(pub Uuid);

impl ViewId {
    /// Creates a new `ViewId` using `UUIDv7`.
    ///
    /// `UUIDv7` provides monotonic, timestamp-prefixed uniqueness suitable
    /// for use as a database key.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::ViewId;
    ///
    /// let id1 = ViewId::new_v7();
    /// let id2 = ViewId::new_v7();
    /// assert_ne!(id1, id2);
    /// ```
    #[must_use]
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are acceptable for test failures"
)]
mod tests {
    use super::*;
    use crate::{Capability, OpKind, RefName};
    use ciborium::ser::into_writer as cbor_encode;
    use std::collections::{BTreeSet, HashMap};
    use std::error::Error;

    // ── Helpers ────────────────────────────────────────────────────────────

    fn root_view() -> View {
        View::root(vec![])
    }

    fn cap(prefix: &str, ops: &[OpKind], ttl: u64) -> Capability {
        Capability { path_prefix: prefix.to_owned(), op_set: ops.iter().copied().collect(), ttl }
    }

    fn parent_root_cap() -> Capability {
        cap("/", &[OpKind::Read, OpKind::Write, OpKind::Delete], 3600)
    }

    // ── Existing surface (kept green) ────────────────────────────────

    #[test]
    fn active_view_not_terminal() {
        let v = root_view();
        assert!(v.is_active());
        assert!(!v.is_terminal());
    }

    #[test]
    fn aborted_view_is_terminal() {
        let mut v = root_view();
        v.status = Status::Aborted;
        assert!(!v.is_active());
        assert!(v.is_terminal());
    }

    #[test]
    fn committed_view_is_terminal() {
        let mut v = root_view();
        v.status = Status::Committed;
        assert!(v.is_terminal());
    }

    #[test]
    fn view_serde_effects_absent_when_none() -> Result<(), Box<dyn Error>> {
        let v = root_view();
        let json = serde_json::to_string(&v)?;
        assert!(!json.contains("effects_hash"), "effects_hash should be absent when None: {json}");
        Ok(())
    }

    #[test]
    fn view_serde_round_trip_with_effects() -> Result<(), Box<dyn Error>> {
        let mut v = root_view();
        v.effects_hash = Some(Hash::from_bytes(*blake3::hash(b"effects").as_bytes()));
        let json = serde_json::to_string(&v)?;
        let v2: View = serde_json::from_str(&json)?;
        assert_eq!(v.effects_hash, v2.effects_hash);
        Ok(())
    }

    #[test]
    fn status_serde_snake_case() -> Result<(), Box<dyn Error>> {
        assert_eq!(serde_json::to_string(&Status::Active)?, "\"active\"");
        assert_eq!(serde_json::to_string(&Status::Committed)?, "\"committed\"");
        assert_eq!(serde_json::to_string(&Status::Aborted)?, "\"aborted\"");
        Ok(())
    }

    #[test]
    fn view_id_uniqueness() {
        let a = ViewId::new_v7();
        let b = ViewId::new_v7();
        assert_ne!(a, b);
    }

    // ── View::root ─────────────────────────────────────────────────────────

    #[test]
    fn root_initial_state() {
        let v = View::root(vec![]);
        assert!(v.is_active());
        assert!(v.parent.is_none());
        assert!(v.namespace.is_empty());
        assert!(v.delta.ops.is_empty());
        assert_eq!(v.delta.base_hash, Hash::from_bytes([0u8; 32]));
        assert!(v.caps.is_empty());
        assert!(v.effects_hash.is_none());
    }

    #[test]
    fn root_carries_caps() {
        let c = cap("/", &[OpKind::Read], 0);
        let v = View::root(vec![c.clone()]);
        assert_eq!(v.caps, vec![c]);
    }

    #[test]
    fn root_ids_are_unique() {
        let a = View::root(vec![]);
        let b = View::root(vec![]);
        assert_ne!(a.id, b.id, "fresh roots must have distinct ids");
    }

    // ── View::child_of ─────────────────────────────────────────────────────

    // ── ADR-0032 boundary interaction (CoW borrow chain) ───────────────────
    //
    // §6 requires an explicit test that an Isolated-boundary fork
    // (the default per ADR-0032) does not leak the child's namespace
    // mutations into the parent under abort. The new `Namespace` type's
    // structural-sharing semantics give this property at the data-structure
    // layer; this test names the contract.

    #[test]
    fn isolated_boundary_fork_write_abort_leaves_parent_untouched() {
        let mut parent = View::root(vec![parent_root_cap()]);
        let h_parent = Hash::from_bytes(*blake3::hash(b"parent").as_bytes());
        parent.namespace.insert("shared".into(), h_parent);
        let parent_namespace_before = parent.namespace.clone();

        // Fork an Isolated child (ADR-0032 default boundary).
        let mut child =
            View::child_of(&parent, vec![]).expect("empty caps narrowing always permitted");

        // Child writes — both a fresh key and a shadow of `shared`.
        let h_child = Hash::from_bytes(*blake3::hash(b"child").as_bytes());
        child.namespace.insert("k1".into(), h_child);
        child.namespace.insert("shared".into(), h_child);

        // Sanity: child sees its own writes.
        assert_eq!(child.namespace.get(&RefName::new("k1")), Some(&h_child));
        assert_eq!(child.namespace.get(&RefName::new("shared")), Some(&h_child));

        // Abort the child (ADR-0032 Isolated: discard delta + effects;
        // namespace borrow released).
        child.transition_to(Status::Aborted).expect("Active → Aborted");

        // The parent's namespace must be byte-equal to its snapshot.
        // This is what makes "fork freely + abort cheap" sound under CoW.
        assert_eq!(
            parent.namespace, parent_namespace_before,
            "parent namespace leaked from aborted Isolated child"
        );
        assert_eq!(parent.namespace.get(&RefName::new("k1")), None);
        assert_eq!(parent.namespace.get(&RefName::new("shared")), Some(&h_parent));
    }

    #[test]
    fn child_of_inherits_namespace_and_links_parent() {
        let mut parent = View::root(vec![parent_root_cap()]);
        let h = Hash::from_bytes(*blake3::hash(b"v").as_bytes());
        parent.namespace.insert("k".into(), h);

        let child_cap = cap("/", &[OpKind::Read], 60);
        let child = View::child_of(&parent, vec![child_cap]).expect("narrowing");
        assert_eq!(child.parent, Some(parent.id));
        assert_eq!(child.namespace.get("k"), Some(&h), "namespace inherited");
        assert!(child.delta.ops.is_empty(), "child delta is empty");
        assert!(child.effects_hash.is_none(), "child effects_hash is empty");
        assert!(child.is_active());
    }

    #[test]
    fn child_of_rejects_terminal_parent() {
        let mut parent = View::root(vec![parent_root_cap()]);
        parent.status = Status::Aborted;
        let err = View::child_of(&parent, vec![]).expect_err("terminal parent must be rejected");
        match err {
            ReelError::ViewTerminal(s) => assert_eq!(s, Status::Aborted),
            other @ (ReelError::BlockNotFound(_)
            | ReelError::CommitConflict(_)
            | ReelError::ClassCAfterAbort
            | ReelError::ForkDepthExceeded(_)
            | ReelError::EffectDrainTimeout
            | ReelError::Storage(_)
            | ReelError::CapabilityViolation(_)
            | ReelError::ReachabilityViolation(_)) => {
                panic!("expected ViewTerminal, got {other:?}");
            }
        }
    }

    #[test]
    fn child_of_rejects_committed_parent() {
        let mut parent = View::root(vec![parent_root_cap()]);
        parent.status = Status::Committed;
        let err = View::child_of(&parent, vec![]).expect_err("committed parent rejected");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Committed)));
    }

    #[test]
    fn child_of_rejects_amplification_op_set() {
        // Parent: read only.  Child: read + write → amplification.
        let parent = View::root(vec![cap("/", &[OpKind::Read], 3600)]);
        let amplified = cap("/", &[OpKind::Read, OpKind::Write], 3600);
        let err = View::child_of(&parent, vec![amplified])
            .expect_err("op_set amplification must be rejected");
        assert!(matches!(err, ReelError::CapabilityViolation(_)));
    }

    #[test]
    fn child_of_rejects_amplification_prefix() {
        // Parent: ws/docs.  Child: ws → broader prefix → amplification.
        let parent = View::root(vec![cap("ws/docs/", &[OpKind::Read], 3600)]);
        let amplified = cap("ws/", &[OpKind::Read], 3600);
        let err = View::child_of(&parent, vec![amplified])
            .expect_err("prefix amplification must be rejected");
        assert!(matches!(err, ReelError::CapabilityViolation(_)));
    }

    #[test]
    fn child_of_rejects_amplification_ttl() {
        // Parent: ttl 60.  Child: ttl 3600 → amplification.
        let parent = View::root(vec![cap("/", &[OpKind::Read], 60)]);
        let amplified = cap("/", &[OpKind::Read], 3600);
        let err = View::child_of(&parent, vec![amplified])
            .expect_err("ttl amplification must be rejected");
        assert!(matches!(err, ReelError::CapabilityViolation(_)));
    }

    #[test]
    fn child_of_rejects_unbounded_when_parent_bounded() {
        // Parent: ttl 60 (bounded).  Child: ttl 0 (unbounded) → amplification.
        let parent = View::root(vec![cap("/", &[OpKind::Read], 60)]);
        let amplified = cap("/", &[OpKind::Read], 0);
        let err = View::child_of(&parent, vec![amplified])
            .expect_err("unbounded child against bounded parent must be rejected");
        assert!(matches!(err, ReelError::CapabilityViolation(_)));
    }

    #[test]
    fn child_of_accepts_unbounded_parent() {
        // Parent ttl=0 covers any child ttl.
        let parent = View::root(vec![cap("/", &[OpKind::Read], 0)]);
        let bounded = cap("/", &[OpKind::Read], 60);
        View::child_of(&parent, vec![bounded]).expect("unbounded parent covers bounded child");
        let unbounded = cap("/", &[OpKind::Read], 0);
        View::child_of(&parent, vec![unbounded]).expect("unbounded parent covers unbounded child");
    }

    #[test]
    fn child_of_accepts_narrowing() {
        let parent_cap = cap("ws/", &[OpKind::Read, OpKind::Write, OpKind::Delete], 3600);
        let parent = View::root(vec![parent_cap]);
        // Narrow op_set, narrow prefix, narrow ttl.
        let narrow = cap("ws/docs/", &[OpKind::Read], 1800);
        View::child_of(&parent, vec![narrow]).expect("narrowing accepted");
    }

    #[test]
    fn child_of_accepts_equal_prefix() {
        let parent = View::root(vec![cap("ws/", &[OpKind::Read], 60)]);
        let same = cap("ws/", &[OpKind::Read], 30);
        View::child_of(&parent, vec![same]).expect("equal prefix counts as sub-prefix");
    }

    #[test]
    fn child_of_accepts_empty_caps() {
        // Renouncing authority: child claims nothing.
        let parent = View::root(vec![cap("/", &[OpKind::Read], 60)]);
        View::child_of(&parent, vec![]).expect("empty child caps always valid");
    }

    #[test]
    fn child_of_rejects_nonempty_against_empty_parent() {
        // Parent has no caps; child claims authority → amplification.
        let parent = View::root(vec![]);
        let any_cap = cap("/", &[OpKind::Read], 60);
        let err = View::child_of(&parent, vec![any_cap])
            .expect_err("child cap without parent cap is amplification");
        assert!(matches!(err, ReelError::CapabilityViolation(_)));
    }

    // ── Status accessors ───────────────────────────────────────────────────

    #[test]
    fn is_active_committed_aborted_partition() {
        let mut v = root_view();
        assert!(v.is_active());
        assert!(!v.is_committed());
        assert!(!v.is_aborted());

        v.status = Status::Committed;
        assert!(!v.is_active());
        assert!(v.is_committed());
        assert!(!v.is_aborted());

        v.status = Status::Aborted;
        assert!(!v.is_active());
        assert!(!v.is_committed());
        assert!(v.is_aborted());
    }

    // ── View::transition_to ────────────────────────────────────────────────

    #[test]
    fn transition_active_to_committed() {
        let mut v = root_view();
        v.transition_to(Status::Committed).expect("Active → Committed");
        assert!(v.is_committed());
    }

    #[test]
    fn transition_active_to_aborted() {
        let mut v = root_view();
        v.transition_to(Status::Aborted).expect("Active → Aborted");
        assert!(v.is_aborted());
    }

    #[test]
    fn transition_active_to_active_rejected() {
        // No-op transitions are not part of the surface.
        let mut v = root_view();
        let err = v.transition_to(Status::Active).expect_err("Active → Active is not legal");
        assert!(matches!(err, ReelError::ViewTerminal(Status::Active)));
    }

    #[test]
    fn transition_committed_is_one_way() {
        let mut v = root_view();
        v.transition_to(Status::Committed).expect("ok");
        for target in [Status::Active, Status::Committed, Status::Aborted] {
            let err = v
                .transition_to(target)
                .expect_err("committed View is terminal; no further transitions");
            assert!(matches!(err, ReelError::ViewTerminal(Status::Committed)));
        }
        assert!(v.is_committed(), "status unchanged after rejected transition");
    }

    #[test]
    fn transition_aborted_is_one_way() {
        let mut v = root_view();
        v.transition_to(Status::Aborted).expect("ok");
        for target in [Status::Active, Status::Committed, Status::Aborted] {
            let err = v
                .transition_to(target)
                .expect_err("aborted View is terminal; no further transitions");
            assert!(matches!(err, ReelError::ViewTerminal(Status::Aborted)));
        }
        assert!(v.is_aborted(), "status unchanged after rejected transition");
    }

    // ── View::depth ────────────────────────────────────────────────────────

    #[test]
    fn root_depth_is_zero() {
        let v = root_view();
        let d = v
            .depth(|_id: ViewId| panic!("lookup must not fire for a root View"))
            .expect("root depth is computable without lookup");
        assert_eq!(d, 0);
    }

    #[test]
    fn linear_chain_depth() {
        // Build a chain root → c1 → c2 → c3 and verify each depth.
        let root = View::root(vec![parent_root_cap()]);
        let child_cap = || cap("/", &[OpKind::Read], 60);
        let c1 = View::child_of(&root, vec![child_cap()]).expect("ok");
        let c2 = View::child_of(&c1, vec![child_cap()]).expect("ok");
        let c3 = View::child_of(&c2, vec![child_cap()]).expect("ok");

        let mut table: HashMap<ViewId, View> = HashMap::new();
        table.insert(root.id, root.clone());
        table.insert(c1.id, c1.clone());
        table.insert(c2.id, c2.clone());

        let lookup = |id: ViewId| -> Result<View, ReelError> {
            table.get(&id).cloned().ok_or_else(|| ReelError::Storage(format!("missing View {id}")))
        };

        assert_eq!(root.depth(lookup).expect("root"), 0);
        assert_eq!(c1.depth(lookup).expect("c1"), 1);
        assert_eq!(c2.depth(lookup).expect("c2"), 2);
        assert_eq!(c3.depth(lookup).expect("c3"), 3);
    }

    #[test]
    fn depth_propagates_lookup_failure() {
        let root = View::root(vec![parent_root_cap()]);
        let c1 = View::child_of(&root, vec![cap("/", &[OpKind::Read], 60)]).expect("ok");
        // Empty lookup → Storage error.
        let err = c1
            .depth(|_id| Err(ReelError::Storage("missing".to_owned())))
            .expect_err("missing parent must propagate as Storage error");
        assert!(matches!(err, ReelError::Storage(_)));
    }

    #[test]
    fn depth_rejects_cycle_beyond_max() {
        // Build a self-cycle by hand: a View whose parent is itself.
        // depth() walks parent.parent.parent... — since lookup always
        // returns the same View with parent=Some(self), the loop must
        // bail at MAX_FORK_DEPTH + 1.
        let mut v = View::root(vec![]);
        v.parent = Some(v.id);
        let cycle = v.clone();
        let lookup = |_id: ViewId| -> Result<View, ReelError> { Ok(cycle.clone()) };
        let err = v.depth(lookup).expect_err("cycle must trip MAX_FORK_DEPTH");
        match err {
            ReelError::ForkDepthExceeded(d) => {
                assert!(d > MAX_FORK_DEPTH, "depth {d} must exceed MAX_FORK_DEPTH");
            }
            other @ (ReelError::BlockNotFound(_)
            | ReelError::CommitConflict(_)
            | ReelError::ClassCAfterAbort
            | ReelError::EffectDrainTimeout
            | ReelError::Storage(_)
            | ReelError::ViewTerminal(_)
            | ReelError::CapabilityViolation(_)
            | ReelError::ReachabilityViolation(_)) => {
                panic!("expected ForkDepthExceeded, got {other:?}");
            }
        }
    }

    // ── effects_hash semantics ─────────────────────────────────────────────

    #[test]
    fn effects_hash_some_then_none_round_trip() -> Result<(), Box<dyn Error>> {
        let mut v = root_view();
        let h = Hash::from_bytes(*blake3::hash(b"buf").as_bytes());
        v.effects_hash = Some(h);
        let j = serde_json::to_string(&v)?;
        assert!(j.contains("effects_hash"), "Some(h) must serialise: {j}");
        let r: View = serde_json::from_str(&j)?;
        assert_eq!(r.effects_hash, Some(h));

        let mut v2 = r;
        v2.effects_hash = None;
        let j2 = serde_json::to_string(&v2)?;
        assert!(!j2.contains("effects_hash"), "None must be skipped: {j2}");
        let r2: View = serde_json::from_str(&j2)?;
        assert_eq!(r2.effects_hash, None);
        Ok(())
    }

    // ── CBOR serialisation (write path) ───────────────────────────────────
    //
    // NOTE: CBOR full round-trip is blocked by a known-era limitation
    // in `Hash::hex_serde::deserialize` (borrowing `&str` from `ciborium`
    // text strings).  The Block module documented the same gap and tests
    // CBOR write-only.  We mirror that here.

    #[test]
    fn view_cbor_serialises_without_error() -> Result<(), Box<dyn Error>> {
        let v = root_view();
        let mut buf = Vec::new();
        cbor_encode(&v, &mut buf)?;
        assert!(!buf.is_empty(), "CBOR output must be non-empty");
        Ok(())
    }

    #[test]
    fn view_cbor_serialises_with_effects_hash() -> Result<(), Box<dyn Error>> {
        let mut v = root_view();
        v.effects_hash = Some(Hash::from_bytes(*blake3::hash(b"effects").as_bytes()));
        let mut buf = Vec::new();
        cbor_encode(&v, &mut buf)?;
        assert!(!buf.is_empty(), "CBOR output must be non-empty");
        Ok(())
    }

    // ── Schema cross-check: JSON keys match view.schema.json ──────────────

    #[test]
    fn json_keys_match_schema_active_view_no_effects() -> Result<(), Box<dyn Error>> {
        // view.schema.json `required: ["id", "namespace", "delta", "caps", "status"]`
        // — `effects_hash` and `parent` are optional.  A root Active view
        // with no effects must serialise the 5 required keys exactly.
        let v = root_view();
        let val: serde_json::Value = serde_json::to_value(&v)?;
        let obj = val.as_object().expect("View serialises as object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["caps", "delta", "id", "namespace", "status"]);
        Ok(())
    }

    #[test]
    fn json_status_enum_values_match_schema() -> Result<(), Box<dyn Error>> {
        // schema enum: ["active", "committed", "aborted"]
        let v = root_view();
        let j = serde_json::to_string(&v)?;
        assert!(j.contains("\"status\":\"active\""), "active: {j}");
        let mut v2 = v.clone();
        v2.transition_to(Status::Committed)?;
        let j2 = serde_json::to_string(&v2)?;
        assert!(j2.contains("\"status\":\"committed\""), "committed: {j2}");
        let mut v3 = v;
        v3.transition_to(Status::Aborted)?;
        let j3 = serde_json::to_string(&v3)?;
        assert!(j3.contains("\"status\":\"aborted\""), "aborted: {j3}");
        Ok(())
    }

    // ── Property tests ─────────────────────────────────────────────────────
    //
    // spec done_when: "Property tests: fork-then-abort leaves parent
    // unchanged; status transitions are one-way."

    mod property {
        use super::*;
        use proptest::array::uniform32;
        use proptest::collection::{btree_map as p_btree, vec as p_vec};
        use proptest::prelude::*;

        fn hash_strategy() -> impl Strategy<Value = Hash> {
            uniform32(any::<u8>()).prop_map(Hash::from_bytes)
        }

        fn name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_/]{0,64}".prop_map(|s| s)
        }

        fn namespace_strategy() -> impl Strategy<Value = Namespace> {
            // Generate via BTreeMap then convert; `imbl::OrdMap` collects
            // from any `IntoIterator<Item = (K, V)>` so the conversion is
            // O(n log n) and cheap at the proptest scale (0..=8 entries).
            p_btree(name_strategy(), hash_strategy(), 0..=8)
                .prop_map(|m| m.into_iter().map(|(k, v)| (crate::RefName::from(k), v)).collect())
        }

        fn op_kind_strategy() -> impl Strategy<Value = OpKind> {
            prop_oneof![
                Just(OpKind::Read),
                Just(OpKind::Write),
                Just(OpKind::Delete),
                Just(OpKind::FireB),
                Just(OpKind::FireC),
            ]
        }

        fn op_set_strategy() -> impl Strategy<Value = BTreeSet<OpKind>> {
            p_vec(op_kind_strategy(), 0..=5).prop_map(|v| v.into_iter().collect::<BTreeSet<_>>())
        }

        fn parent_cap_strategy() -> impl Strategy<Value = Capability> {
            // Parent capabilities use the root prefix so any child prefix is
            // trivially covered.  ttl=0 is unbounded so any child ttl is
            // covered.  This focuses the property on the abort invariant
            // rather than on lattice plumbing (covered by unit tests).
            op_set_strategy().prop_map(|op_set| Capability {
                path_prefix: "/".into(),
                op_set,
                ttl: 0,
            })
        }

        proptest! {
            // Property 1 — fork-then-abort leaves the parent unchanged.
            //
            // Construct a parent View with a randomised namespace and one
            // permissive root capability.  Fork a child, abort the child.
            // The parent's id / namespace / delta / caps / effects_hash /
            // status / parent fields must be byte-for-byte unchanged.
            #[test]
            fn fork_then_abort_preserves_parent(
                ns in namespace_strategy(),
                parent_cap in parent_cap_strategy(),
            ) {
                let mut parent = View::root(vec![parent_cap]);
                parent.namespace = ns;
                let parent_before = parent.clone();

                // Child inherits whatever sub-set of parent caps the
                // narrowing rules permit; an empty child cap set is
                // always valid.
                let mut child = View::child_of(&parent, vec![])
                    .expect("empty caps always permitted");
                child.transition_to(Status::Aborted)
                    .expect("Active → Aborted");

                prop_assert_eq!(&parent, &parent_before, "parent unchanged by child abort");
            }

            // Property 3 (ADR-0034) — child namespace writes never propagate
            // to the parent (the forest-GC borrow property at the View level).
            //
            // Pick a namespace, fork, mutate the child via the public API
            // (`namespace.insert`), then assert the parent's bindings are
            // bit-equal to the snapshot taken at fork time. This is what
            // makes "fork freely + speculate" sound under the new CoW
            // namespace.
            #[test]
            fn child_namespace_writes_do_not_propagate_to_parent(
                ns in namespace_strategy(),
                parent_cap in parent_cap_strategy(),
                injected_key in "[a-z][a-z0-9]{0,32}",
                injected_byte in any::<u8>(),
            ) {
                let mut parent = View::root(vec![parent_cap]);
                parent.namespace = ns;
                let parent_snapshot = parent.namespace.clone();

                let mut child = View::child_of(&parent, vec![])
                    .expect("empty caps always permitted");

                // Child writes — both an inject of a fresh key and an
                // overwrite of an existing key (if one is present).
                let injected_hash = Hash::from_bytes([injected_byte; 32]);
                child.namespace.insert(injected_key.clone().into(), injected_hash);
                if let Some((k, _v)) = parent_snapshot.iter().next() {
                    child.namespace.insert(k.clone(), injected_hash);
                }

                // Parent's namespace is byte-for-byte unchanged.
                prop_assert_eq!(
                    &parent.namespace,
                    &parent_snapshot,
                    "child writes leaked into parent namespace"
                );
            }

            // Property 2 — status transitions are one-way.
            //
            // From any terminal state, no transition is legal.  Active is
            // the only entry; once we leave it, we never return.
            #[test]
            fn status_transitions_are_one_way(
                go_aborted in any::<bool>(),
            ) {
                let mut v = View::root(vec![]);
                let terminal = if go_aborted { Status::Aborted } else { Status::Committed };
                v.transition_to(terminal).expect("Active → terminal");
                prop_assert_eq!(v.status, terminal);

                for target in [Status::Active, Status::Committed, Status::Aborted] {
                    let err = v.transition_to(target);
                    prop_assert!(err.is_err(), "transition from {:?} to {:?} must fail", terminal, target);
                    prop_assert_eq!(v.status, terminal, "status must be unchanged after rejected transition");
                }
            }
        }
    }
}
