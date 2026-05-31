//! Scoped attenuating authority — the reel `Capability` type.
//!
//! A [`Capability`] grants the holder authority to perform operations
//! within a namespace prefix.
//! Capabilities are monotone-narrowing: derived Capabilities can only
//! attenuate (sub-set `op_set`, sub-prefix `path_prefix`, ≤ TTL).
//! Amplification is forbidden by I-003.
//!
//! See `spec/spec.md` §5.5 and
//! ADR-0025.
//!
//! # Invariants
//!
//! - I-003: Capability narrowing — derived Capabilities are sub-lattices
//!   of their delegator (see `spec/invariants.yaml`).
//!
//! # References
//!
//! - Miller, M., Yee, K., Shapiro, J. (2003). Capability Myths Demolished.
//! - IETF draft-pidlisnyi-aps-00. Agent Passport System. (Monotone-narrowing lattice.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Scoped attenuating authority over a namespace prefix.
///
/// `Capability` is first-class state in reel: Capabilities live in
/// [`crate::View::caps`], are passed across `fork` (optionally
/// attenuated), constructed only by the kernel, and held only by Views.
///
/// Per I-003, derived Capabilities MUST be sub-lattices of their
/// delegator.
/// Amplification (wider `op_set`, broader `path_prefix`, larger `ttl`)
/// is forbidden.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{Capability, OpKind};
/// use std::collections::BTreeSet;
///
/// let cap = Capability {
///     path_prefix: "workspace/".into(),
///     op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
///     ttl: 3600,
/// };
///
/// // A sub-capability that only allows reads.
/// let sub = Capability {
///     path_prefix: "workspace/docs/".into(),
///     op_set: BTreeSet::from([OpKind::Read]),
///     ttl: 1800,
/// };
///
/// // I-003: sub's op_set is a subset of cap's op_set.
/// assert!(sub.op_set.is_subset(&cap.op_set));
/// assert!(sub.ttl <= cap.ttl);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// Namespace prefix this Capability covers.
    ///
    /// Sub-Capabilities MUST have a more specific (longer) prefix or
    /// the same prefix as the delegator (per I-003).
    pub path_prefix: String,

    /// The set of operation kinds permitted by this Capability.
    ///
    /// Sub-Capabilities MUST have a strict subset of the delegator's
    /// `op_set` (per I-003).
    pub op_set: BTreeSet<OpKind>,

    /// Time-to-live in seconds.
    ///
    /// `0` means unbounded.
    /// Sub-Capabilities MUST have `ttl ≤` the delegator's `ttl`
    /// (per I-003).
    pub ttl: u64,
}

impl Capability {
    /// Returns `true` iff `self` *covers* `other` — i.e. `other` is a
    /// legal I-003 narrowing of `self`. `other` is covered when, on every
    /// axis, it claims no more authority than `self`:
    ///
    /// - **prefix**: `other.path_prefix` starts with `self.path_prefix`
    ///   (equal counts — a sub-prefix is more specific, never broader);
    /// - **ops**: `other.op_set` is a subset of `self.op_set`;
    /// - **ttl**: `other.ttl` lasts no longer than `self.ttl`, where `0`
    ///   means unbounded (an unbounded `self` covers any `other`; a
    ///   bounded `self` never covers an unbounded `other`).
    ///
    /// `covers` is reflexive (`c.covers(&c)`) and transitive — the two
    /// properties a delegation chain relies on (I-003, `spec/spec.md` §7.1).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Capability, OpKind};
    /// use std::collections::BTreeSet;
    ///
    /// let parent = Capability {
    ///     path_prefix: "ws/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
    ///     ttl: 3600,
    /// };
    /// let child = Capability {
    ///     path_prefix: "ws/docs/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read]),
    ///     ttl: 600,
    /// };
    /// assert!(parent.covers(&child));
    /// assert!(!child.covers(&parent)); // narrowing is one-way
    /// ```
    #[must_use]
    pub fn covers(&self, other: &Self) -> bool {
        other.path_prefix.starts_with(&self.path_prefix)
            && other.op_set.is_subset(&self.op_set)
            && match (self.ttl, other.ttl) {
                (0, _) => true,  // unbounded self covers any other
                (_, 0) => false, // bounded self cannot cover unbounded other
                (s, o) => o <= s,
            }
    }

    /// Derives a narrowed Capability from `self`, or `None` if the request
    /// would amplify authority along any axis (I-003).
    ///
    /// Pure and total: no I/O, no hidden state. When the result is
    /// `Some(c)`, `self.covers(&c)` is guaranteed; when it is `None`, the
    /// request widened at least one axis and no Capability is produced —
    /// amplification has no representable outcome. This is the structural
    /// enforcement point for I-003: the only narrowing constructor on the
    /// type (`spec/spec.md` §7.1).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Capability, OpKind};
    /// use std::collections::BTreeSet;
    ///
    /// let parent = Capability {
    ///     path_prefix: "ws/".into(),
    ///     op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
    ///     ttl: 3600,
    /// };
    ///
    /// // Narrowing succeeds.
    /// let narrowed =
    ///     parent.narrow("ws/docs/".into(), BTreeSet::from([OpKind::Read]), 600);
    /// assert!(narrowed.is_some());
    ///
    /// // Amplifying the op_set is refused.
    /// let amplified = parent.narrow(
    ///     "ws/".into(),
    ///     BTreeSet::from([OpKind::Read, OpKind::Write, OpKind::Delete]),
    ///     600,
    /// );
    /// assert!(amplified.is_none());
    /// ```
    #[must_use]
    pub fn narrow(&self, path_prefix: String, op_set: BTreeSet<OpKind>, ttl: u64) -> Option<Self> {
        let candidate = Self { path_prefix, op_set, ttl };
        self.covers(&candidate).then_some(candidate)
    }
}

/// Operation kinds permitted by a [`Capability`].
///
/// The set of values matches the `spec/schema/2026-Q3/capability.schema.json`
/// `op_set` enum.
///
/// # Examples
///
/// ```rust
/// use reel_spec::OpKind;
///
/// let k = OpKind::Read;
/// assert_eq!(format!("{k:?}"), "Read");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    /// Read access to Blocks and namespace entries.
    Read,
    /// Write access (put new entries in the namespace).
    Write,
    /// Delete access (remove entries from the namespace).
    Delete,
    /// Authorises firing Class B (idempotent remote) effects at commit.
    FireB,
    /// Authorises firing Class C (irreversible remote) effects at commit.
    FireC,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn capability_serde_round_trip() -> Result<(), Box<dyn Error>> {
        let cap = Capability {
            path_prefix: "ws/".into(),
            op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
            ttl: 60,
        };
        let json = serde_json::to_string(&cap)?;
        let cap2: Capability = serde_json::from_str(&json)?;
        assert_eq!(cap, cap2);
        Ok(())
    }

    #[test]
    fn op_kind_serde_snake_case() -> Result<(), Box<dyn Error>> {
        let j = serde_json::to_string(&OpKind::FireC)?;
        assert_eq!(j, "\"fire_c\"");
        Ok(())
    }

    #[test]
    fn narrowing_subset() {
        let parent = Capability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read, OpKind::Write, OpKind::Delete]),
            ttl: 3600,
        };
        let child = Capability {
            path_prefix: "/docs/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 600,
        };
        // I-003 structural check.
        assert!(child.op_set.is_subset(&parent.op_set));
        assert!(child.ttl <= parent.ttl);
    }

    // ── covers / narrow (I-003) ─────────────────────────────────────────

    fn cap(prefix: &str, ops: &[OpKind], ttl: u64) -> Capability {
        Capability { path_prefix: prefix.to_owned(), op_set: ops.iter().copied().collect(), ttl }
    }

    #[test]
    fn covers_is_reflexive() {
        let c = cap("ws/", &[OpKind::Read, OpKind::Write], 60);
        assert!(c.covers(&c), "a Capability is a valid narrowing of itself");
    }

    #[test]
    fn narrow_accepts_equal_and_strict() {
        let parent = cap("ws/", &[OpKind::Read, OpKind::Write], 3600);
        // Equal on every axis is a (degenerate) valid narrowing.
        assert!(
            parent
                .narrow("ws/".into(), BTreeSet::from([OpKind::Read, OpKind::Write]), 3600)
                .is_some()
        );
        // Strict narrowing on every axis succeeds and is covered by self.
        let n = parent.narrow("ws/docs/".into(), BTreeSet::from([OpKind::Read]), 600);
        assert!(
            matches!(&n, Some(c) if parent.covers(c)),
            "strict narrowing must succeed and be covered by self"
        );
    }

    #[test]
    fn narrow_rejects_wider_op_set() {
        let parent = cap("ws/", &[OpKind::Read], 60);
        assert!(
            parent
                .narrow("ws/".into(), BTreeSet::from([OpKind::Read, OpKind::Write]), 60)
                .is_none()
        );
    }

    #[test]
    fn narrow_rejects_broader_prefix() {
        let parent = cap("ws/docs/", &[OpKind::Read], 60);
        assert!(parent.narrow("ws/".into(), BTreeSet::from([OpKind::Read]), 60).is_none());
    }

    #[test]
    fn narrow_rejects_longer_ttl() {
        let parent = cap("ws/", &[OpKind::Read], 60);
        assert!(parent.narrow("ws/".into(), BTreeSet::from([OpKind::Read]), 120).is_none());
    }

    #[test]
    fn narrow_ttl_unbounded_semantics() {
        // Bounded parent cannot grant unbounded (0) child.
        let bounded = cap("ws/", &[OpKind::Read], 60);
        assert!(bounded.narrow("ws/".into(), BTreeSet::from([OpKind::Read]), 0).is_none());
        // Unbounded parent covers any child ttl.
        let unbounded = cap("ws/", &[OpKind::Read], 0);
        assert!(unbounded.narrow("ws/".into(), BTreeSet::from([OpKind::Read]), 999).is_some());
        assert!(unbounded.narrow("ws/".into(), BTreeSet::from([OpKind::Read]), 0).is_some());
    }

    mod property {
        use super::*;
        use proptest::collection::btree_set as p_btree_set;
        use proptest::prelude::*;

        fn op_kind() -> impl Strategy<Value = OpKind> {
            prop_oneof![
                Just(OpKind::Read),
                Just(OpKind::Write),
                Just(OpKind::Delete),
                Just(OpKind::FireB),
                Just(OpKind::FireC),
            ]
        }

        fn cap_strategy() -> impl Strategy<Value = Capability> {
            ("[a-c/]{0,6}", p_btree_set(op_kind(), 0..=5), 0u64..=4096)
                .prop_map(|(path_prefix, op_set, ttl)| Capability { path_prefix, op_set, ttl })
        }

        proptest! {
            // covers is reflexive — a Capability narrows to itself.
            #[test]
            fn covers_reflexive(c in cap_strategy()) {
                prop_assert!(c.covers(&c));
            }

            // covers is transitive — the delegation-chain property (I-003):
            // if a delegates to b and b to c, c's authority is within a's.
            #[test]
            fn covers_transitive(a in cap_strategy(), b in cap_strategy(), c in cap_strategy()) {
                if a.covers(&b) && b.covers(&c) {
                    prop_assert!(a.covers(&c));
                }
            }

            // narrow returns Some iff the request is covered; and any Some
            // result is itself covered by self — narrow never amplifies.
            #[test]
            fn narrow_iff_covered(
                parent in cap_strategy(),
                pfx in "[a-c/]{0,6}",
                ops in p_btree_set(op_kind(), 0..=5),
                ttl in 0u64..=4096,
            ) {
                let candidate = Capability { path_prefix: pfx.clone(), op_set: ops.clone(), ttl };
                let got = parent.narrow(pfx, ops, ttl);
                prop_assert_eq!(got.is_some(), parent.covers(&candidate));
                if let Some(c) = got {
                    prop_assert!(parent.covers(&c), "narrow result must be covered (no amplification)");
                }
            }
        }
    }
}
