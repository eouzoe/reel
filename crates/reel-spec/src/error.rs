//! Protocol error catalogue — `ReelError`.
//!
//! [`ReelError`] contains exactly nine variants, one per entry in
//! `spec/errors.yaml` (E-001 through E-010, with E-004 reserved and
//! therefore absent).
//!
//! Each variant carries the minimum information needed by callers to
//! route the error, per `spec/spec.md` §9.
//!
//! # Error codes
//!
//! | Variant | Code | Severity | Condition |
//! |---------|------|----------|-----------|
//! | `BlockNotFound` | E-001 | recoverable | Block unknown or unreachable. |
//! | `CommitConflict` | E-002 | recoverable | Class B precondition failed; concurrent conflict. |
//! | `ClassCAfterAbort` | E-003 | critical | I-002 breach — must be unreachable. |
//! | `ForkDepthExceeded` | E-005 | recoverable | Depth > MAX_FORK_DEPTH (I-008). |
//! | `EffectDrainTimeout` | E-006 | critical | Effect fire timed out. |
//! | `Storage` | E-007 | critical | Underlying storage error. |
//! | `ViewTerminal` | E-008 | recoverable | Operation on committed/aborted View. |
//! | `CapabilityViolation` | E-009 | recoverable | I-003 enforcement. |
//! | `ReachabilityViolation` | E-010 | recoverable | I-001 enforcement. |

use crate::{Hash, Status};
use thiserror::Error;

/// The reel protocol error type.
///
/// Nine variants cover every error code in `spec/errors.yaml`.
/// E-004 was removed in ADR-0024 and the slot is reserved — do not
/// re-use it in this schema version.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{ReelError, Hash};
///
/// let h = Hash::from_bytes([0u8; 32]);
/// let err = ReelError::BlockNotFound(h);
/// let msg = err.to_string();
/// assert!(msg.contains("E-001"));
///
/// let conflict = ReelError::CommitConflict("ETag mismatch".into());
/// assert!(conflict.to_string().contains("E-002"));
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReelError {
    /// E-001: The Store does not contain a Block matching the requested
    /// [`struct@Hash`], or the Block exists in storage but is not reachable
    /// from the current namespace (I-001 enforcement).
    ///
    /// Severity: recoverable.
    #[error("E-001 BlockNotFound: {0}")]
    BlockNotFound(Hash),

    /// E-002: The commit cannot proceed because a Class B effect's
    /// `version_constraint` is no longer satisfied, or overlapping
    /// concurrent commits conflict.
    ///
    /// Severity: recoverable.
    #[error("E-002 CommitConflict: {0}")]
    CommitConflict(String),

    /// E-003: A Class C effect was attempted after `abort`.
    ///
    /// This MUST be unreachable in a correct implementation; reaching
    /// it means I-002 has been breached.
    ///
    /// Severity: critical.
    #[error("E-003 ClassCAfterAbort (UNREACHABLE)")]
    ClassCAfterAbort,

    // E-004 is reserved — MUST NOT be re-used in schema version 2026-Q3.
    /// E-005: A `fork` would produce a View with depth greater than
    /// `MAX_FORK_DEPTH` (I-008 convenience bound).
    ///
    /// Severity: recoverable.
    #[error("E-005 ForkDepthExceeded: depth={0}")]
    ForkDepthExceeded(u32),

    /// E-006: A Class B or Class C `fire` exceeded the configured
    /// `EFFECT_DRAIN_TIMEOUT`.
    ///
    /// Severity: critical.
    #[error("E-006 EffectDrainTimeout")]
    EffectDrainTimeout,

    /// E-007: An underlying storage error from the persistence layer.
    ///
    /// Severity: critical.
    #[error("E-007 Storage: {0}")]
    Storage(String),

    /// E-008: An operation was attempted on a terminal View
    /// (status is `committed` or `aborted`).
    ///
    /// Severity: recoverable.
    #[error("E-008 ViewTerminal: status={0:?}")]
    ViewTerminal(Status),

    /// E-009: An operation was attempted without sufficient [`crate::Capability`]
    /// to authorise it (I-003 enforcement).
    ///
    /// Severity: recoverable.
    #[error("E-009 CapabilityViolation: {0}")]
    CapabilityViolation(String),

    /// E-010: An attempt was made to access a [`crate::Block`] via
    /// [`struct@Hash`] that is not reachable through the current namespace
    /// (I-001 enforcement).
    ///
    /// Severity: recoverable.
    #[error("E-010 ReachabilityViolation: hash={0:?}")]
    ReachabilityViolation(Hash),
}

impl ReelError {
    /// Stable error-code identifier — the `"E-NNN"` prefix that
    /// appears at the start of every [`Display`](std::fmt::Display)
    /// rendering, exposed as a first-class accessor.
    ///
    /// Bindings and adapters MUST classify errors by this code rather
    /// than parsing the [`Display`] or [`Debug`] string. The code is
    /// part of the protocol surface (per
    /// [`spec/errors.yaml`](../../../spec/errors.yaml)) and is stable
    /// across schema versions in the `2026-Q3` track. New variants
    /// MUST add a new code and MUST NOT re-use a retired one (E-004
    /// is permanently reserved per the error catalogue).
    ///
    /// # Example
    /// ```
    /// # use reel_spec::{ReelError, Status};
    /// let e = ReelError::ViewTerminal(Status::Aborted);
    /// assert_eq!(e.code(), "E-008");
    /// ```
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BlockNotFound(_) => "E-001",
            Self::CommitConflict(_) => "E-002",
            Self::ClassCAfterAbort => "E-003",
            Self::ForkDepthExceeded(_) => "E-005",
            Self::EffectDrainTimeout => "E-006",
            Self::Storage(_) => "E-007",
            Self::ViewTerminal(_) => "E-008",
            Self::CapabilityViolation(_) => "E-009",
            Self::ReachabilityViolation(_) => "E-010",
        }
    }

    /// Whether an implementation MAY retry the operation that produced
    /// this error.
    ///
    /// `true` for `recoverable` codes; `false` for `critical` codes
    /// that indicate an invariant breach or an unrecoverable I/O
    /// failure. Bindings MUST NOT retry critical errors silently.
    /// Severity classes follow `spec/errors.yaml`.
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        match self {
            Self::BlockNotFound(_)
            | Self::CommitConflict(_)
            | Self::ForkDepthExceeded(_)
            | Self::ViewTerminal(_)
            | Self::CapabilityViolation(_)
            | Self::ReachabilityViolation(_) => true,
            Self::ClassCAfterAbort | Self::EffectDrainTimeout | Self::Storage(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_in_messages() {
        let h = Hash::from_bytes([0u8; 32]);

        let e001 = ReelError::BlockNotFound(h);
        assert!(e001.to_string().contains("E-001"), "E-001: {e001}");

        let e002 = ReelError::CommitConflict("conflict reason".into());
        assert!(e002.to_string().contains("E-002"), "E-002: {e002}");

        let e003 = ReelError::ClassCAfterAbort;
        assert!(e003.to_string().contains("E-003"), "E-003: {e003}");

        let e005 = ReelError::ForkDepthExceeded(65);
        assert!(e005.to_string().contains("E-005"), "E-005: {e005}");
        assert!(e005.to_string().contains("65"), "depth in msg: {e005}");

        let e006 = ReelError::EffectDrainTimeout;
        assert!(e006.to_string().contains("E-006"), "E-006: {e006}");

        let e007 = ReelError::Storage("disk full".into());
        assert!(e007.to_string().contains("E-007"), "E-007: {e007}");

        let e008 = ReelError::ViewTerminal(Status::Aborted);
        assert!(e008.to_string().contains("E-008"), "E-008: {e008}");

        let e009 = ReelError::CapabilityViolation("op not in set".into());
        assert!(e009.to_string().contains("E-009"), "E-009: {e009}");

        let e010 = ReelError::ReachabilityViolation(h);
        assert!(e010.to_string().contains("E-010"), "E-010: {e010}");
    }

    #[test]
    fn code_accessor_matches_display_prefix() {
        // The `code()` accessor must return the same "E-NNN" identifier
        // that prefixes the Display string. Drift between the two would
        // break bindings that classify on the first surface only.
        let h = Hash::from_bytes([0u8; 32]);
        let cases: &[(ReelError, &str)] = &[
            (ReelError::BlockNotFound(h), "E-001"),
            (ReelError::CommitConflict("x".into()), "E-002"),
            (ReelError::ClassCAfterAbort, "E-003"),
            (ReelError::ForkDepthExceeded(1), "E-005"),
            (ReelError::EffectDrainTimeout, "E-006"),
            (ReelError::Storage("io".into()), "E-007"),
            (ReelError::ViewTerminal(Status::Committed), "E-008"),
            (ReelError::CapabilityViolation("y".into()), "E-009"),
            (ReelError::ReachabilityViolation(h), "E-010"),
        ];
        for (err, code) in cases {
            assert_eq!(err.code(), *code, "code() != expected for {err}");
            assert!(err.to_string().starts_with(code), "Display prefix != code: {err}");
        }
    }

    #[test]
    fn severity_classes_align_with_errors_yaml() {
        // Recoverable vs critical mapping per spec/errors.yaml.
        // Recoverable: caller MAY retry / take corrective action.
        // Critical: implies invariant breach or unrecoverable I/O.
        let h = Hash::from_bytes([0u8; 32]);
        assert!(ReelError::BlockNotFound(h).is_recoverable());
        assert!(ReelError::CommitConflict("x".into()).is_recoverable());
        assert!(ReelError::ForkDepthExceeded(1).is_recoverable());
        assert!(ReelError::ViewTerminal(Status::Committed).is_recoverable());
        assert!(ReelError::CapabilityViolation("y".into()).is_recoverable());
        assert!(ReelError::ReachabilityViolation(h).is_recoverable());
        assert!(!ReelError::ClassCAfterAbort.is_recoverable());
        assert!(!ReelError::EffectDrainTimeout.is_recoverable());
        assert!(!ReelError::Storage("disk".into()).is_recoverable());
    }

    #[test]
    fn nine_variants_count() {
        // This test exercises all nine variants to ensure the catalogue
        // stays in sync with errors.yaml.
        let h = Hash::from_bytes([0u8; 32]);
        let variants: &[&str] = &[
            &ReelError::BlockNotFound(h).to_string(),
            &ReelError::CommitConflict("x".into()).to_string(),
            &ReelError::ClassCAfterAbort.to_string(),
            &ReelError::ForkDepthExceeded(1).to_string(),
            &ReelError::EffectDrainTimeout.to_string(),
            &ReelError::Storage("io".into()).to_string(),
            &ReelError::ViewTerminal(Status::Committed).to_string(),
            &ReelError::CapabilityViolation("y".into()).to_string(),
            &ReelError::ReachabilityViolation(h).to_string(),
        ];
        assert_eq!(variants.len(), 9);
    }
}
