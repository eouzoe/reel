//! Kernel-internal `FireCapability` linear token.
//!
//! `FireCapability` is the **commit-drain mode** token: it is constructed
//! only inside `reel-core`'s commit path and consumed by
//! [`crate::ClassB::fire`] / [`crate::ClassC::fire`].
//!
//! This is distinct from the published [`reel_spec::Capability`] type
//! (`{ path_prefix, op_set, ttl }`) which encodes *authority* and governs
//! I-003 narrowing.  See the task spec § "Two capabilities" and
//! ADR-0006 § "Updated by
//! ADR-0025".
//!
//! # I-002 enforcement
//!
//! `FireCapability` is `!Clone + !Copy + !Send`.  The kernel mints exactly
//! one token per effect inside the commit drain loop; once consumed by
//! `fire()` the token is gone.  Because `abort()` never calls
//! `FireCapability::new()`, there is no token on the abort path and
//! `ClassC::fire` is mechanically unreachable there.
//!
//! See
//! effect-class-rules § 2
//! (seL4 capability model) and § 6 (L2 isolation) for the full argument.

use core::fmt;
use core::marker::{PhantomData, PhantomPinned};

/// Linear commit-drain token.
///
/// Constructed only by `reel-core`'s commit path via `FireCapability::new`.
/// Passed by reference to [`crate::ClassB::fire`] and [`crate::ClassC::fire`].
///
/// Properties enforced by the type:
///
/// - `!Clone` / `!Copy` — cannot be duplicated; each `fire()` call receives
///   a fresh token.
/// - `!Send` — confines the token to the commit task's thread.
/// - Private constructor — only `pub(crate)` access prevents forgery from
///   outside this crate.
///
/// # Examples
///
/// ```text
/// // FireCapability is intentionally not constructable in doc-tests.
/// // The kernel creates one via FireCapability::new() (pub(crate)) inside
/// // the commit drain.  User code never constructs a FireCapability.
/// ```
#[must_use = "FireCapability must be passed to fire(); unused tokens indicate a logic error"]
pub struct FireCapability {
    /// `!Send` — confines the token to the commit task's thread.
    _not_send: PhantomData<*const ()>,
    /// `!Clone` / `!Copy` — `PhantomPinned` is `!Unpin + !Clone`.
    _linear: PhantomData<PhantomPinned>,
    /// Private field; prevents external construction.
    _seal: (),
}

impl FireCapability {
    /// Mints a fresh `FireCapability`.
    ///
    /// Only callable from `reel-core` (visibility = `pub(crate)`).
    /// Must be called exactly once per effect in the commit drain loop.
    // Suppressed: `reel-core` is not in scope here; the function will be used
    // once the commit pipeline lands.
    #[allow(dead_code, reason = "used by reel-core commit drain")]
    pub(crate) const fn new() -> Self {
        Self { _not_send: PhantomData, _linear: PhantomData, _seal: () }
    }

    /// Walking-skeleton constructor.  Identical to [`Self::new`] but
    /// `pub` so the throwaway `reel_core::skeleton::SkeletonKernel` can mint
    /// the token from its commit drain.  When / land the real
    /// pipeline they will use the `pub(crate)` constructor and this
    /// feature-gated wrapper can be removed.
    ///
    ///
    /// the constructor visibility gap this resolves.
    #[cfg(feature = "walking-skeleton")]
    pub const fn new_for_skeleton() -> Self {
        Self::new()
    }
}

// Manual Debug — never reveals internal state.
impl fmt::Debug for FireCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FireCapability { .. }")
    }
}
