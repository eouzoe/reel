//! Effect receipt — the result returned by a successful `fire()` call.
//!
//! An [`EffectReceipt`] is produced by [`crate::ClassB::fire`] and
//! [`crate::ClassC::fire`] and records:
//!
//! - The stable effect identifier (`effect_id`).
//! - The version reference **after** firing (`version_ref_after`) — for Class B
//!   this is the new authoritative version returned by the remote system;
//!   for Class C it is `None`.
//! - An adapter-defined diagnostic payload (`adapter_diagnostic`) — arbitrary
//!   JSON for logging and observability.
//!
//! See `spec/adapter-interface.md` § 7 (Error model) and
//! 03-effect-isolation § 6.

use reel_spec::VersionRef;

/// Result of a successfully fired effect.
///
/// Returned by [`crate::ClassB::fire`] and [`crate::ClassC::fire`] on success.
///
/// # Examples
///
/// ```rust
/// use reel_effects::EffectReceipt;
/// use reel_spec::{VersionRef, VersionKind};
///
/// // Class B receipt — carries the updated version reference.
/// let receipt = EffectReceipt {
///     effect_id: 42,
///     version_ref_after: Some(VersionRef {
///         kind: VersionKind::Etag,
///         value: "\"new-etag\"".into(),
///     }),
///     adapter_diagnostic: serde_json::json!({"status": 200}),
/// };
/// assert_eq!(receipt.effect_id, 42);
/// assert!(receipt.version_ref_after.is_some());
/// ```
#[derive(Debug, Clone)]
pub struct EffectReceipt {
    /// The stable identifier for this effect within the commit batch.
    pub effect_id: u64,

    /// For Class B: the version reference after firing (returned by the remote
    /// system after the idempotent operation).
    /// For Class C: `None` (irreversible; no meaningful version reference).
    pub version_ref_after: Option<VersionRef>,

    /// Adapter-defined diagnostic JSON (e.g. HTTP status, Slack message id,
    /// response body excerpt).  Used for logging and observability only.
    pub adapter_diagnostic: serde_json::Value,
}
