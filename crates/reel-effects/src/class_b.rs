//! Class B — idempotent remote effect trait.
//!
//! Class B effects act on external resources and carry a **stable version
//! reference** ([`reel_spec::VersionRef`]: `ETag`, commit SHA, object version ID) for the
//! commit-time precondition check.  Idempotency-under-stable-identifier means
//! applying the same Class B effect twice with the same `VersionRef` has the
//! same observable consequence as applying it once.
//!
//! # Protocol flow (per `spec/spec.md` § 5.7.2)
//!
//! 1. `version_ref()` — returns the stable identifier submitted with the
//!    effect (used to identify it in the buffer).
//! 2. `assert_precondition(ctx)` — called at commit time; returns
//!    `Err(E-002)` if the version reference no longer matches the remote.
//! 3. `fire(cap)` — performs the idempotent remote operation; returns an
//!    [`EffectReceipt`] with the updated version reference.
//!
//! # Enforcement
//!
//! [`ClassB::fire`] takes a `&FireCapability` reference — the kernel-internal
//! commit-drain token.  The token is constructed only inside `reel-core`'s
//! commit path (L2 isolation).  On the `abort` path no token is ever minted,
//! so `fire()` is mechanically unreachable there (I-002).
//!
//! # Note on async bounds
//!
//! The trait methods are declared as returning `Pin<Box<dyn Future<...>>>` so
//! that `FireCapability` (which is intentionally `!Sync`, hence `!Send` when
//! borrowed) does not force `Send` bounds on the generated futures.  The
//! kernel's commit path is single-threaded per ADR-0008.
//!
//! # Sealing
//!
//! The trait is sealed via [`crate::sealed::Sealed`].
//!
//! # References
//!
//! - ADR-0003 (Class B idempotency-under-stable-identifier semantics).
//! - ADR-0025 `VersionRef` rename from `IdempotencyKey`.
//! - `spec/spec.md` § 5.7.2.

use crate::{fire_capability::FireCapability, receipt::EffectReceipt, sealed::Sealed};
use reel_spec::{ReelError, VersionRef};
use std::future::Future;
use std::pin::Pin;

/// Context passed to [`ClassB::assert_precondition`].
///
/// Currently opaque; future versions may carry token context, deadline, etc.
/// Kept as a concrete struct rather than a trait object to avoid object-safety
/// concerns.
#[derive(Debug)]
#[non_exhaustive]
pub struct PreconditionCtx {
    /// Remaining drain-budget in milliseconds (0 = no budget tracking).
    pub drain_budget_ms: u64,
}

/// Class B — idempotent remote effect.
///
/// Implementations MUST be safely re-invocable with the same `VersionRef`:
/// if the remote system has already applied the effect, a second call
/// MUST produce the same observable result (or a no-op).
///
/// # Examples
///
/// See the integration-test helper in the crate's `tests/` module.
pub trait ClassB: Sealed + Send + 'static {
    /// Returns the stable version reference for this effect.
    ///
    /// Used by the kernel to identify the effect in the buffer and to supply
    /// the precondition context at commit time.
    fn version_ref(&self) -> VersionRef;

    /// Verifies the version reference is still valid at commit time.
    ///
    /// Called by the kernel before `fire()`.  Returns:
    /// - `Ok(())` — precondition holds; proceed to fire.
    /// - `Err(E-002 CommitConflict)` — version reference no longer matches;
    ///   the commit MUST fail.
    ///
    /// # Cancel Safety
    ///
    /// Implementations SHOULD be cancel-safe.  If the precondition check is
    /// dropped before completion the kernel will treat the result as unknown
    /// and MUST NOT fire the effect.
    fn assert_precondition<'a>(
        &'a self,
        ctx: &'a PreconditionCtx,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReelError>> + 'a>>;

    /// Fires the idempotent remote operation.
    ///
    /// Called by the kernel after `assert_precondition` succeeds.
    ///
    /// # Cancel Safety
    ///
    /// Cancel-safe at the protocol level: the idempotency semantics permit
    /// safe re-invocation via a fresh `FireCapability` after recovery.
    /// Implementations MUST be idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::EffectDrainTimeout`] on timeout (E-006) or
    /// [`ReelError::CommitConflict`] (E-002) if the remote rejects the
    /// version reference during the fire phase.
    fn fire<'a>(
        self,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a;
}
