//! Class C — irreversible remote effect trait.
//!
//! Class C effects act on external resources and **cannot be undone** by any
//! further action of reel.  Once observable, they persist independently of
//! reel's state.
//!
//! - Buffered in `View.effects_hash` until `commit`.
//! - Fire after all Class B effects at commit (per ADR-0015).
//! - MUST NOT fire on abort (I-002).
//!
//! # I-002 enforcement
//!
//! [`ClassC::fire`] takes a `&FireCapability` reference — the kernel-internal
//! commit-drain token.  The token is constructed **only** inside `reel-core`'s
//! commit path (L2 isolation per
//! effect-class-rules § 6).
//! Because `abort()` never calls `FireCapability::new()`, `ClassC::fire` is
//! mechanically unreachable on the abort path.
//!
//! # Note on async bounds
//!
//! `fire` returns `Pin<Box<dyn Future<...>>>` so that `FireCapability`
//! (intentionally `!Sync`) does not force `Send` bounds on futures.
//! The kernel commit path is single-threaded (ADR-0008).
//!
//! # Sealing
//!
//! The trait is sealed via [`crate::sealed::Sealed`].
//!
//! # References
//!
//! - ADR-0003 (Class C non-reversibility).
//! - ADR-0025 (canonical ontology, I-002 statement).
//! - `spec/spec.md` § 5.7.3.
//! - Garcia-Molina & Salem, "Sagas", SIGMOD 1987 (saga-compensation model;
//!   Class C is the "irreversible" leg of a saga that reel buffers until commit).

use crate::{fire_capability::FireCapability, receipt::EffectReceipt, sealed::Sealed};
use reel_spec::ReelError;
use std::future::Future;
use std::pin::Pin;

/// Class C — irreversible remote effect.
///
/// Implementations MUST NOT have side effects in `Drop`.  The `Drop` impl
/// (if any) MUST be pure cleanup only (e.g. de-allocating buffers).  Any
/// side-effect in `Drop` would bypass the commit-gate `FireCapability` check
/// and violate I-002.
///
/// # Examples
///
/// See the integration-test helper in the crate's `tests/` module.
pub trait ClassC: Sealed + Send + 'static {
    /// Produces a human-readable / UI-displayable description of this effect.
    ///
    /// The `reel preview` UI MUST display this value to the user before the
    /// commit is authorised (`spec/spec.md` § 5.7.3: "The `commit` UI MUST
    /// display each Class C effect's full content and an irreversibility
    /// warning before the user authorises the commit.").
    fn describe(&self) -> serde_json::Value;

    /// Fires the irreversible remote operation.
    ///
    /// Called by the kernel during the commit drain phase, after all Class B
    /// effects have completed (per ADR-0015).
    ///
    /// # Cancel Safety
    ///
    /// **Not cancel-safe.**  If the future is dropped mid-flight, the effect
    /// may have already reached the remote system.  The caller (kernel commit
    /// path) MUST coordinate compensation per the adapter's contract.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::EffectDrainTimeout`] (E-006) on timeout, or
    /// [`ReelError::Storage`] (E-007) for underlying adapter errors.
    fn fire<'a>(
        self,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a;
}
