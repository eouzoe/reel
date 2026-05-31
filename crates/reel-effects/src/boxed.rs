//! Type-erased effect wrappers for kernel buffering.
//!
//! The kernel's effect buffer stores heterogeneous effect types in a single
//! `Vec`.  Each wrapper (`BoxedA`, `BoxedB`, `BoxedC`) erases the concrete
//! effect type while preserving the class distinction at the wrapper level —
//! `BoxedA` and `BoxedC` are distinct types with no implicit conversion.
//!
//! Design follows
//! 03-effect-isolation § 6:
//! type erasure through a private supertrait (`ClassAErased`, etc.) keeps the
//! public API clean while allowing the kernel to call `apply()` / `fire()`
//! through a `Box<dyn …>`.
//!
//! # Cross-class impossibility
//!
//! `BoxedA::new(c)` where `c: ClassC` is a compile-time error: the `T:
//! ClassA` bound in `BoxedA::new` cannot be satisfied by a `ClassC` type.
//! The sealed-trait + orphan rules make this impossible by construction.

use crate::{
    class_a::ClassA,
    class_b::{ClassB, PreconditionCtx},
    class_c::ClassC,
    fire_capability::FireCapability,
    receipt::EffectReceipt,
};
use reel_spec::{Op, ReelError, VersionRef};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

// ────────────────────────────────────────────────────────────────────────────
// Class A
// ────────────────────────────────────────────────────────────────────────────

/// Private erasure trait for [`ClassA`] implementors.
trait ClassAErased: Send + 'static {
    fn apply_erased(&self) -> Op;
}

impl<T: ClassA> ClassAErased for T {
    fn apply_erased(&self) -> Op {
        self.apply()
    }
}

/// Type-erased Class A effect for kernel buffering.
///
/// # Examples
///
/// `ClassA` is sealed via the `pub(crate)` `Sealed` supertrait, so this
/// example is `ignore`'d at the rustdoc level — external crates must
/// declare effect types via the `#[reel::class_a]` proc-macro.
/// Runnable proof of the same shape lives in
/// `lib.rs::tests::boxed_a_apply`.
///
/// ```rust,ignore
/// use reel_effects::{BoxedA, ClassA};
/// use reel_spec::{Hash, Op};
///
/// #[reel::class_a]
/// struct AppendEntry { name: String, hash: Hash }
///
/// impl ClassA for AppendEntry {
///     fn apply(&self) -> Op {
///         Op::Put { name: self.name.clone(), hash: self.hash }
///     }
/// }
///
/// let boxed = BoxedA::new(AppendEntry {
///     name: "file.txt".into(),
///     hash: Hash::from_bytes([1u8; 32]),
/// });
/// let op = boxed.apply();
/// assert!(matches!(op, Op::Put { .. }));
/// ```
#[must_use = "BoxedA must be applied; unused effects are a logic error"]
pub struct BoxedA(Box<dyn ClassAErased>);

impl BoxedA {
    /// Wrap a [`ClassA`] effect for kernel buffering.
    pub fn new<E: ClassA>(effect: E) -> Self {
        Self(Box::new(effect))
    }

    /// Apply the erased effect, producing the [`Op`] for the View's delta.
    #[must_use = "the returned Op must be appended to the delta"]
    pub fn apply(&self) -> Op {
        self.0.apply_erased()
    }
}

impl fmt::Debug for BoxedA {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoxedA { .. }")
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Class B
// ────────────────────────────────────────────────────────────────────────────

/// Private erasure trait for [`ClassB`] implementors.
///
/// Uses manual `Pin<Box<dyn Future>>` return types because `async fn` would
/// generate futures that require `Send`, but `FireCapability` is intentionally
/// `!Sync` (hence `&FireCapability` is `!Send`).  The kernel commit path is
/// single-threaded by design (ADR-0008).
trait ClassBErased: Send + 'static {
    fn version_ref_erased(&self) -> VersionRef;

    fn assert_precondition_erased<'a>(
        &'a self,
        ctx: &'a PreconditionCtx,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReelError>> + 'a>>;

    fn fire_erased<'a>(
        self: Box<Self>,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>;
}

impl<T: ClassB> ClassBErased for T {
    fn version_ref_erased(&self) -> VersionRef {
        self.version_ref()
    }

    fn assert_precondition_erased<'a>(
        &'a self,
        ctx: &'a PreconditionCtx,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReelError>> + 'a>> {
        self.assert_precondition(ctx)
    }

    fn fire_erased<'a>(
        self: Box<Self>,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>> {
        (*self).fire(cap)
    }
}

/// Type-erased Class B effect for kernel buffering.
///
/// Carries the version reference and async precondition check / fire
/// methods through a trait object.
#[must_use = "BoxedB must be fired or dropped; unused effects are a logic error"]
pub struct BoxedB(Box<dyn ClassBErased>);

impl BoxedB {
    /// Wrap a [`ClassB`] effect for kernel buffering.
    pub fn new<E: ClassB>(effect: E) -> Self {
        Self(Box::new(effect))
    }

    /// Returns the stable version reference for this effect.
    #[must_use = "version_ref is needed for precondition checks at commit"]
    pub fn version_ref(&self) -> VersionRef {
        self.0.version_ref_erased()
    }

    /// Verifies the version reference at commit time.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::CommitConflict`] (E-002) if the version reference
    /// no longer matches the remote system.
    #[must_use = "the precondition future must be awaited before calling fire()"]
    pub fn assert_precondition<'a>(
        &'a self,
        ctx: &'a PreconditionCtx,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReelError>> + 'a>> {
        self.0.assert_precondition_erased(ctx)
    }

    /// Fires the idempotent remote operation.
    ///
    /// Consumes the `BoxedB`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::EffectDrainTimeout`] (E-006) on timeout or
    /// [`ReelError::CommitConflict`] (E-002) if the remote rejects the
    /// version reference.
    ///
    /// # Cancel Safety
    ///
    /// Delegates to the concrete [`ClassB::fire`] implementation.
    #[must_use = "the fire future must be awaited to execute the effect"]
    pub fn fire<'a>(
        self,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a,
    {
        self.0.fire_erased(cap)
    }
}

impl fmt::Debug for BoxedB {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoxedB { .. }")
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Class C
// ────────────────────────────────────────────────────────────────────────────

/// Private erasure trait for [`ClassC`] implementors.
///
/// Same rationale as `ClassBErased`: manual `Pin<Box<dyn Future>>` to avoid
/// `Send` bound on futures that capture `&FireCapability`.
trait ClassCErased: Send + 'static {
    fn describe_erased(&self) -> serde_json::Value;

    fn fire_erased<'a>(
        self: Box<Self>,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>;
}

impl<T: ClassC> ClassCErased for T {
    fn describe_erased(&self) -> serde_json::Value {
        self.describe()
    }

    fn fire_erased<'a>(
        self: Box<Self>,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>> {
        (*self).fire(cap)
    }
}

/// Type-erased Class C effect for kernel buffering.
///
/// Carries the irreversibility description and async fire method
/// through a trait object.
#[must_use = "BoxedC must be fired or dropped; unused effects are a logic error"]
pub struct BoxedC(Box<dyn ClassCErased>);

impl BoxedC {
    /// Wrap a [`ClassC`] effect for kernel buffering.
    pub fn new<E: ClassC>(effect: E) -> Self {
        Self(Box::new(effect))
    }

    /// Returns the human-readable description for the `reel preview` UI.
    #[must_use = "describe() output is required for the commit UI confirmation"]
    pub fn describe(&self) -> serde_json::Value {
        self.0.describe_erased()
    }

    /// Fires the irreversible remote operation.
    ///
    /// Consumes the `BoxedC`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::EffectDrainTimeout`] (E-006) on timeout, or
    /// [`ReelError::Storage`] (E-007) for underlying adapter errors.
    ///
    /// # Cancel Safety
    ///
    /// **Not cancel-safe.**  Delegates to the concrete [`ClassC::fire`]
    /// implementation.
    #[must_use = "the fire future must be awaited to execute the effect"]
    pub fn fire<'a>(
        self,
        cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a,
    {
        self.0.fire_erased(cap)
    }
}

impl fmt::Debug for BoxedC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoxedC { .. }")
    }
}
