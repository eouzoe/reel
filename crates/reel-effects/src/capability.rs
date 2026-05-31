//! Published `Capability` type with narrowing-only constructors.
//!
//! This module houses the **narrowing wrapper** around the protocol-surface
//! [`reel_spec::Capability`] type.  It enforces I-003 (Capability narrowing)
//! at the API level: every constructor either receives the root Capability from
//! the kernel or derives a strict sub-lattice from an existing one.
//!
//! # I-003 enforcement
//!
//! Per `spec/spec.md` § 5.5 and
//! ADR-0025:
//!
//! - Sub-Capabilities MUST have a more-specific (longer) or equal `path_prefix`.
//! - Sub-Capabilities MUST have a strict subset of the delegator's `op_set`.
//! - Sub-Capabilities MUST have `ttl ≤` the delegator's `ttl`.
//!
//! [`Capability::narrow`] enforces all three rules and returns
//! `Err(ReelError::CapabilityViolation)` (E-009) on any violation.
//!
//! # Root Capabilities
//!
//! Root Capabilities are constructed by the kernel only.  There is no public
//! `new()` or `from_*` constructor in `reel-effects` — doing so would allow
//! arbitrary code to mint root authority, violating I-003.
//!
//! See
//! effect-class-rules § 4
//! and
//! 03-effect-isolation § 3
//! for the full design.

use reel_spec::{Capability as SpecCapability, OpKind, ReelError};
use std::collections::BTreeSet;

/// Arguments for [`Capability::narrow`].
///
/// All fields are optional; omitting a field keeps the parent's value.
/// Any field that *widens* the authority produces E-009.
#[derive(Debug, Clone)]
pub struct NarrowArgs {
    /// Override `path_prefix`.  MUST be ≥ (more specific than or equal to)
    /// the parent prefix, i.e. `new_prefix.starts_with(parent_prefix)`.
    pub path_prefix: Option<String>,

    /// Override `op_set`.  MUST be a subset of the parent's `op_set`.
    pub op_set: Option<BTreeSet<OpKind>>,

    /// Override `ttl`.  MUST be `≤` the parent's `ttl`
    /// (`0` means unbounded; narrowing from unbounded to bounded is allowed;
    ///  narrowing from bounded to a larger value is not).
    pub ttl: Option<u64>,
}

/// A narrowable scoped-authority wrapper.
///
/// `Capability` wraps the protocol-surface [`reel_spec::Capability`] and
/// exposes **only** the [`narrow`](Capability::narrow) constructor for
/// deriving sub-Capabilities.  No public `new()` or `from_*` exists; root
/// Capabilities arrive from the kernel through crate-internal paths.
///
/// # I-003
///
/// All narrowing checks are performed eagerly in [`narrow`](Capability::narrow).
/// An `Err(E-009 CapabilityViolation)` is returned on any amplification attempt.
///
/// # Examples
///
/// ```rust,ignore
/// // `Capability::from_kernel` is pub(crate); root Capabilities are
/// // constructed only by the kernel.  The example below shows the kernel's
/// // internal usage.  For runnable tests see the unit-test suite in `lib.rs`.
/// use reel_effects::{Capability, NarrowArgs};
/// use reel_spec::OpKind;
/// use std::collections::BTreeSet;
///
/// // root: Capability  (received from kernel via `fork`)
/// let sub = root.narrow(NarrowArgs {
///     path_prefix: Some("workspace/docs/".into()),
///     op_set: Some(BTreeSet::from([OpKind::Read])),
///     ttl: Some(600),
/// }).expect("valid narrowing");
///
/// assert_eq!(sub.inner().path_prefix, "workspace/docs/");
/// assert_eq!(sub.inner().op_set, BTreeSet::from([OpKind::Read]));
/// assert_eq!(sub.inner().ttl, 600);
/// ```
#[derive(Debug, Clone)]
pub struct Capability {
    inner: SpecCapability,
}

impl Capability {
    /// Kernel-only constructor.  Wraps a raw [`reel_spec::Capability`].
    ///
    /// `pub(crate)` — only `reel-core` (or test helpers within this crate)
    /// may construct root Capabilities.  External code derives Capabilities
    /// exclusively via [`narrow`](Self::narrow).
    // Suppressed: called from doc-tests, unit tests inside this crate, and
    // eventually from reel-core once the kernel type lands.
    #[allow(dead_code, reason = "used in doc-tests + reel-core kernel")]
    pub(crate) const fn from_kernel(cap: SpecCapability) -> Self {
        Self { inner: cap }
    }

    /// Returns a read-only reference to the underlying protocol-surface
    /// [`reel_spec::Capability`].
    #[must_use = "inner() provides the raw spec Capability for inspection"]
    pub const fn inner(&self) -> &SpecCapability {
        &self.inner
    }

    /// Derives a sub-Capability that is a strict sub-lattice of `self`.
    ///
    /// Each [`NarrowArgs`] field that is `Some` replaces the parent's value;
    /// `None` keeps the parent's value.  Any field that would *widen* authority
    /// returns `Err(E-009 CapabilityViolation)`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::CapabilityViolation`] (E-009) if:
    ///
    /// - `args.path_prefix` is `Some(p)` and `p` does not start with
    ///   `self.inner.path_prefix` (would broaden namespace access).
    /// - `args.op_set` is `Some(s)` and `s` is not a subset of
    ///   `self.inner.op_set` (would add unauthorised operations).
    /// - `args.ttl` is `Some(t)` and the effective TTL would exceed
    ///   `self.inner.ttl` (parent TTL=0 means unbounded; any narrowing from
    ///   unbounded is valid; narrowing from bounded `t_parent` with
    ///   `t > t_parent` is forbidden).
    ///
    /// # Examples
    ///
    /// Happy path:
    ///
    /// ```rust,ignore
    /// // `from_kernel` is pub(crate).  Runnable test: `lib.rs::tests::narrow_happy_path`.
    /// use reel_effects::{Capability, NarrowArgs};
    /// use reel_spec::OpKind;
    /// use std::collections::BTreeSet;
    ///
    /// // root: Capability  (received from kernel via `fork`)
    /// let child = root.narrow(NarrowArgs {
    ///     path_prefix: Some("/docs/".into()),
    ///     op_set: Some(BTreeSet::from([OpKind::Read])),
    ///     ttl: Some(900),
    /// }).expect("valid narrowing");
    ///
    /// assert_eq!(child.inner().path_prefix, "/docs/");
    /// assert!(child.inner().op_set.contains(&OpKind::Read));
    /// assert_eq!(child.inner().ttl, 900);
    /// ```
    ///
    /// Reject path — `op_set` amplification:
    ///
    /// ```rust,ignore
    /// // `from_kernel` is pub(crate).  Runnable test: `lib.rs::tests::narrow_rejects_op_amplification`.
    /// use reel_effects::{Capability, NarrowArgs};
    /// use reel_spec::{OpKind, ReelError};
    /// use std::collections::BTreeSet;
    ///
    /// // parent: Capability  (received from kernel via `fork`)
    /// let err = parent.narrow(NarrowArgs {
    ///     path_prefix: None,
    ///     op_set: Some(BTreeSet::from([OpKind::Read, OpKind::Write])),
    ///     ttl: None,
    /// });
    /// assert!(matches!(err, Err(ReelError::CapabilityViolation(_))));
    /// ```
    pub fn narrow(&self, args: NarrowArgs) -> Result<Self, ReelError> {
        // --- path_prefix check ---
        let new_prefix = match args.path_prefix {
            Some(p) => {
                if !p.starts_with(self.inner.path_prefix.as_str()) {
                    return Err(ReelError::CapabilityViolation(format!(
                        "I-003 violation: requested path_prefix '{p}' does not start with \
                         parent prefix '{}'",
                        self.inner.path_prefix
                    )));
                }
                p
            }
            None => self.inner.path_prefix.clone(),
        };

        // --- op_set check ---
        let new_ops = match args.op_set {
            Some(ops) => {
                if !ops.is_subset(&self.inner.op_set) {
                    let extra: Vec<_> = ops.difference(&self.inner.op_set).collect();
                    return Err(ReelError::CapabilityViolation(format!(
                        "I-003 violation: requested op_set contains operations \
                         not present in parent: {extra:?}"
                    )));
                }
                ops
            }
            None => self.inner.op_set.clone(),
        };

        // --- ttl check ---
        // parent ttl=0 means unbounded; any requested ttl ≤ unbounded.
        // parent ttl>0: new ttl must be ≤ parent.
        let new_ttl = match args.ttl {
            Some(t) => {
                if self.inner.ttl != 0 && t > self.inner.ttl {
                    return Err(ReelError::CapabilityViolation(format!(
                        "I-003 violation: requested ttl {} exceeds parent ttl {}",
                        t, self.inner.ttl
                    )));
                }
                t
            }
            None => self.inner.ttl,
        };

        Ok(Self {
            inner: SpecCapability { path_prefix: new_prefix, op_set: new_ops, ttl: new_ttl },
        })
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "test assertions intentionally panic on failure")]
mod tests {
    use super::*;
    use reel_spec::OpKind;
    use std::collections::BTreeSet;

    fn root_cap() -> Capability {
        Capability::from_kernel(SpecCapability {
            path_prefix: "ws/".into(),
            op_set: BTreeSet::from([
                OpKind::Read,
                OpKind::Write,
                OpKind::Delete,
                OpKind::FireB,
                OpKind::FireC,
            ]),
            ttl: 3600,
        })
    }

    #[test]
    fn narrow_prefix_valid() {
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs { path_prefix: Some("ws/docs/".into()), op_set: None, ttl: None })
            .unwrap_or_else(|e| panic!("valid prefix narrowing failed: {e}"));
        assert_eq!(sub.inner().path_prefix, "ws/docs/");
    }

    #[test]
    fn narrow_op_set_valid() {
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs {
                path_prefix: None,
                op_set: Some(BTreeSet::from([OpKind::Read])),
                ttl: None,
            })
            .unwrap_or_else(|e| panic!("valid op_set narrowing failed: {e}"));
        assert_eq!(sub.inner().op_set, BTreeSet::from([OpKind::Read]));
    }

    #[test]
    fn narrow_ttl_valid() {
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs { path_prefix: None, op_set: None, ttl: Some(600) })
            .unwrap_or_else(|e| panic!("valid ttl narrowing failed: {e}"));
        assert_eq!(sub.inner().ttl, 600);
    }

    #[test]
    fn narrow_all_fields() {
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs {
                path_prefix: Some("ws/docs/".into()),
                op_set: Some(BTreeSet::from([OpKind::Read, OpKind::Write])),
                ttl: Some(1800),
            })
            .unwrap_or_else(|e| panic!("all-field narrowing failed: {e}"));
        assert_eq!(sub.inner().path_prefix, "ws/docs/");
        assert_eq!(sub.inner().op_set, BTreeSet::from([OpKind::Read, OpKind::Write]));
        assert_eq!(sub.inner().ttl, 1800);
    }

    // --- reject paths (I-003 violations) ---

    #[test]
    fn reject_prefix_amplification() {
        let parent = Capability::from_kernel(SpecCapability {
            path_prefix: "ws/docs/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 60,
        });
        let err = parent.narrow(NarrowArgs {
            path_prefix: Some("ws/".into()), // broader prefix
            op_set: None,
            ttl: None,
        });
        assert!(
            matches!(err, Err(ReelError::CapabilityViolation(_))),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn reject_op_set_amplification() {
        let parent = Capability::from_kernel(SpecCapability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 60,
        });
        let err = parent.narrow(NarrowArgs {
            path_prefix: None,
            op_set: Some(BTreeSet::from([OpKind::Read, OpKind::Write])),
            ttl: None,
        });
        assert!(
            matches!(err, Err(ReelError::CapabilityViolation(_))),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn reject_ttl_amplification() {
        let parent = Capability::from_kernel(SpecCapability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 60,
        });
        let err = parent.narrow(NarrowArgs {
            path_prefix: None,
            op_set: None,
            ttl: Some(120), // exceeds parent 60
        });
        assert!(
            matches!(err, Err(ReelError::CapabilityViolation(_))),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn unbounded_ttl_allows_any_narrowing() {
        let parent = Capability::from_kernel(SpecCapability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 0, // unbounded
        });
        let sub = parent
            .narrow(NarrowArgs { path_prefix: None, op_set: None, ttl: Some(999_999) })
            .unwrap_or_else(|e| panic!("unbounded-parent ttl narrowing failed: {e}"));
        assert_eq!(sub.inner().ttl, 999_999);
    }

    #[test]
    fn narrow_keeps_same_prefix_is_valid() {
        // Same prefix (not strictly longer) is OK per I-003 spec wording
        // "more specific (longer) prefix or the same prefix".
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs { path_prefix: Some("ws/".into()), op_set: None, ttl: None })
            .unwrap_or_else(|e| panic!("same-prefix narrowing failed: {e}"));
        assert_eq!(sub.inner().path_prefix, "ws/");
    }

    #[test]
    fn empty_op_set_is_valid_narrowing() {
        // An empty op_set is a valid sub-set of any op_set.
        let root = root_cap();
        let sub = root
            .narrow(NarrowArgs { path_prefix: None, op_set: Some(BTreeSet::new()), ttl: None })
            .unwrap_or_else(|e| panic!("empty op_set narrowing failed: {e}"));
        assert!(sub.inner().op_set.is_empty());
    }
}
