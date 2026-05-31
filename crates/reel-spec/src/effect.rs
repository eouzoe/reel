//! Effect classification and the `VersionRef` type.
//!
//! An effect in reel is a Block whose payload satisfies an effect schema.
//! The classification is over Block *payloads*, not over Blocks themselves.
//!
//! [`EffectDescriptor`] enumerates the three classes:
//!
//! - Class A — pure local, reversible. Applied directly to the Delta.
//! - Class B — idempotent remote. Carries a [`VersionRef`] precondition.
//! - Class C — irreversible remote. Fires only at `commit` and MUST NOT
//!   fire after `abort` (I-002).
//!
//! [`VersionRef`] is used by Class B effects as a stable remote-system
//! identifier (`ETag`, commit SHA, object version ID).
//! It replaces the pre-ADR-0025 `EffectKey` / `IdempotencyKey` concept
//! (see `00a-adr-0025-reconciliation.md` §1).
//!
//! See `spec/spec.md` §5.7 and ADR-0025.
//!
//! # Invariants
//!
//! - I-002: A Class C effect in a View's `effects_hash` MUST NOT fire
//!   after `abort` on that View.
//!
//! # References
//!
//! - Mohammadi, B., et al. (2026). Atomix. arXiv 2602.14849.
//!   (Two-axis effect taxonomy; reel is a 1-D projection.)
//! - Garcia-Molina, H., Salem, K. (1987). Sagas. SIGMOD.

use serde::{Deserialize, Serialize};

/// Version reference for Class B idempotent effects.
///
/// A `VersionRef` carries a stable remote-system identifier used for the
/// commit-time precondition check (see `spec/spec.md` §5.7.2).
///
/// This type replaces the pre-ADR-0025 `EffectKey` / `IdempotencyKey`
/// concept: two Class B effects with the same `VersionRef` are *equivalent
/// under idempotency*, not forbidden.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{VersionRef, VersionKind};
///
/// let vr = VersionRef {
///     kind: VersionKind::Etag,
///     value: "\"abc123\"".into(),
/// };
/// assert_eq!(vr.kind, VersionKind::Etag);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionRef {
    /// The kind of version reference.
    pub kind: VersionKind,

    /// The stable identifier value (`ETag`, commit SHA, object version ID,
    /// or opaque string).
    pub value: String,
}

/// The category of a [`VersionRef`].
///
/// Matches the `kind` enum in `spec/schema/2026-Q3/ref.schema.json`
/// `version_constraint`.
///
/// # Examples
///
/// ```rust
/// use reel_spec::VersionKind;
///
/// let k = VersionKind::CommitSha;
/// let json = serde_json::to_string(&k).expect("serialise");
/// assert_eq!(json, "\"commit_sha\"");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    /// HTTP `ETag` value (RFC 7232), e.g. `"\"abc123\""`.
    Etag,
    /// A git-style commit SHA (e.g. `"a1b2c3d4…"`).
    CommitSha,
    /// An S3 or similar object-store version ID.
    ObjectVersionId,
    /// Any other stable external version identifier.
    Opaque,
}

/// Discriminated union of Class A, B, and C effect descriptors.
///
/// An effect descriptor captures the information needed by the kernel
/// to buffer, validate, and fire an effect at commit time.
///
/// | Class | Bufferability | Reversibility |
/// |-------|---------------|---------------|
/// | A | directly in View delta | reversed by Delta discard at abort |
/// | B | in `View.effects_hash` | reversible via `version_ref` precondition |
/// | C | in `View.effects_hash` | **irreversible** — MUST NOT fire after abort |
///
/// This is a one-dimensional projection of Atomix's two-axis taxonomy
/// (reversibility × bufferability); at the protocol layer all effects
/// must be bufferable.
///
/// # Examples
///
/// ```rust
/// use reel_spec::EffectDescriptor;
///
/// let class_a = EffectDescriptor::A {
///     effect_id: 1,
///     op: serde_json::json!({"action": "write", "path": "/tmp/x"}),
/// };
/// let class_c = EffectDescriptor::C {
///     effect_id: 2,
///     op: serde_json::json!({"action": "send_slack", "text": "deploy done"}),
/// };
/// assert!(matches!(class_a, EffectDescriptor::A { .. }));
/// assert!(matches!(class_c, EffectDescriptor::C { .. }));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "UPPERCASE")]
pub enum EffectDescriptor {
    /// Class A — pure local, reversible.
    ///
    /// Applies directly to the View's delta layer.
    /// Does NOT enter `View.effects_hash`.
    /// Reversed by Delta discard at `abort`.
    A {
        /// Stable effect identifier within a single commit batch.
        effect_id: u64,
        /// Adapter-defined operation payload.
        op: serde_json::Value,
    },

    /// Class B — idempotent remote.
    ///
    /// Buffered in `View.effects_hash` until commit.
    /// At commit, the implementation MUST verify `version_ref` is still
    /// valid; on failure returns `E-002 CommitConflict`.
    B {
        /// Stable effect identifier within a single commit batch.
        effect_id: u64,
        /// Stable remote-system identifier for the idempotency precondition.
        version_ref: VersionRef,
        /// Adapter-defined operation payload.
        op: serde_json::Value,
    },

    /// Class C — irreversible remote.
    ///
    /// Buffered in `View.effects_hash` until commit.
    /// Fires at commit, after all Class B effects (per ADR-0015).
    /// MUST NOT fire after `abort` (I-002).
    C {
        /// Stable effect identifier within a single commit batch.
        effect_id: u64,
        /// Adapter-defined operation payload.
        op: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn version_ref_serde_round_trip() -> Result<(), Box<dyn Error>> {
        let vr = VersionRef { kind: VersionKind::Etag, value: "\"w/abc\"".into() };
        let json = serde_json::to_string(&vr)?;
        let vr2: VersionRef = serde_json::from_str(&json)?;
        assert_eq!(vr, vr2);
        Ok(())
    }

    #[test]
    fn version_kind_snake_case() -> Result<(), Box<dyn Error>> {
        let j_obj = serde_json::to_string(&VersionKind::ObjectVersionId)?;
        assert_eq!(j_obj, "\"object_version_id\"");
        let j_sha = serde_json::to_string(&VersionKind::CommitSha)?;
        assert_eq!(j_sha, "\"commit_sha\"");
        Ok(())
    }

    #[test]
    fn effect_descriptor_a_serde() -> Result<(), Box<dyn Error>> {
        let e = EffectDescriptor::A { effect_id: 42, op: serde_json::json!({"path": "/tmp/x"}) };
        let json = serde_json::to_string(&e)?;
        assert!(json.contains("\"class\":\"A\""), "tag absent: {json}");
        let e2: EffectDescriptor = serde_json::from_str(&json)?;
        assert_eq!(e, e2);
        Ok(())
    }

    #[test]
    fn effect_descriptor_b_serde() -> Result<(), Box<dyn Error>> {
        let e = EffectDescriptor::B {
            effect_id: 1,
            version_ref: VersionRef { kind: VersionKind::Etag, value: "\"v1\"".into() },
            op: serde_json::json!({"key": "val"}),
        };
        let json = serde_json::to_string(&e)?;
        assert!(json.contains("\"class\":\"B\""), "tag absent: {json}");
        let e2: EffectDescriptor = serde_json::from_str(&json)?;
        assert_eq!(e, e2);
        Ok(())
    }

    #[test]
    fn effect_descriptor_c_serde() -> Result<(), Box<dyn Error>> {
        let e = EffectDescriptor::C { effect_id: 99, op: serde_json::json!({"slack": "msg"}) };
        let json = serde_json::to_string(&e)?;
        assert!(json.contains("\"class\":\"C\""), "tag absent: {json}");
        let e2: EffectDescriptor = serde_json::from_str(&json)?;
        assert_eq!(e, e2);
        Ok(())
    }
}
