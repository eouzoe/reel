//! `StoreError` — storage-layer error type.
//!
//! `StoreError` wraps the errors that originate inside `reel-store`:
//! `redb` engine errors, CBOR codec errors, raw I/O failures, and
//! protocol-level [`reel_spec::ReelError`]s surfaced from the I-001
//! reachability gate.
//!
//! The protocol-level [`reel_spec::ReelError`] variants relevant to this
//! crate are [`ReelError::BlockNotFound`](reel_spec::ReelError::BlockNotFound)
//! (E-001), [`ReelError::ReachabilityViolation`](reel_spec::ReelError::ReachabilityViolation)
//! (E-010), and [`ReelError::Storage`](reel_spec::ReelError::Storage) (E-007).
//! Callers can match on [`StoreError::Protocol`] and inspect the inner
//! [`reel_spec::ReelError`] to route them.

use std::io;

use reel_spec::ReelError;
use thiserror::Error;

/// Error type for storage operations in `reel-store`.
///
/// Three categories of error are produced:
///
/// - [`StoreError::Engine`] / [`StoreError::Io`] / [`StoreError::Codec`] —
///   infrastructure failures (redb, I/O, CBOR codec). Map onto
///   `ReelError::Storage` (E-007) when surfaced to the kernel.
/// - [`StoreError::Protocol`] — a protocol-level [`ReelError`] (E-001 /
///   E-010 / E-009) that the kernel must propagate verbatim.
///
/// # Examples
///
/// ```rust
/// use reel_store::StoreError;
/// use reel_spec::{Hash, ReelError};
///
/// let h = Hash::from_bytes([0u8; 32]);
/// let e: StoreError = ReelError::ReachabilityViolation(h).into();
/// assert!(matches!(e, StoreError::Protocol(ReelError::ReachabilityViolation(_))));
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A protocol-level [`ReelError`] from the I-001 / I-003 enforcement
    /// path (typically `BlockNotFound`, `ReachabilityViolation`, or
    /// `CapabilityViolation`).
    #[error(transparent)]
    Protocol(#[from] ReelError),

    /// An underlying `redb` engine error.
    ///
    /// Carried as a [`String`] so callers do not need to take a
    /// transitive dependency on the redb error hierarchy.
    #[error("redb engine: {0}")]
    Engine(String),

    /// A raw I/O error (e.g. unable to create the store directory).
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// A CBOR encoding / decoding error.
    ///
    /// Stored as a string to keep `StoreError` cheap to construct and
    /// avoid bleeding `ciborium` types into the public error type.
    #[error("cbor: {0}")]
    Codec(String),

    /// A storage configuration error (e.g. `create_if_missing: false`
    /// but the file does not exist).
    #[error("config: {0}")]
    Config(String),
}

impl StoreError {
    /// Helper: lift the protocol-level "Block not found" error onto a
    /// [`reel_spec::Hash`] without taking the [`ReelError`] import at
    /// the call site.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_store::StoreError;
    /// use reel_spec::{Hash, ReelError};
    ///
    /// let h = Hash::from_bytes([0u8; 32]);
    /// let e = StoreError::block_not_found(h);
    /// assert!(matches!(e, StoreError::Protocol(ReelError::BlockNotFound(_))));
    /// ```
    #[must_use]
    pub const fn block_not_found(hash: reel_spec::Hash) -> Self {
        Self::Protocol(ReelError::BlockNotFound(hash))
    }

    /// Helper: lift the protocol-level "reachability violation" error
    /// (E-010) onto a [`reel_spec::Hash`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_store::StoreError;
    /// use reel_spec::{Hash, ReelError};
    ///
    /// let h = Hash::from_bytes([0u8; 32]);
    /// let e = StoreError::reachability_violation(h);
    /// assert!(matches!(
    ///     e,
    ///     StoreError::Protocol(ReelError::ReachabilityViolation(_))
    /// ));
    /// ```
    #[must_use]
    pub const fn reachability_violation(hash: reel_spec::Hash) -> Self {
        Self::Protocol(ReelError::ReachabilityViolation(hash))
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
    use reel_spec::Hash;

    #[test]
    fn from_reel_error() {
        let h = Hash::from_bytes([1u8; 32]);
        let inner = ReelError::ReachabilityViolation(h);
        let outer: StoreError = inner.into();
        assert!(matches!(outer, StoreError::Protocol(_)));
    }

    #[test]
    fn helpers_construct_protocol_variants() {
        let h = Hash::from_bytes([2u8; 32]);

        let nf = StoreError::block_not_found(h);
        let nf_msg = nf.to_string();
        assert!(nf_msg.contains("E-001"), "msg: {nf_msg}");

        let rv = StoreError::reachability_violation(h);
        let rv_msg = rv.to_string();
        assert!(rv_msg.contains("E-010"), "msg: {rv_msg}");
    }

    #[test]
    fn engine_error_displays() {
        let e = StoreError::Engine("table not found".to_owned());
        let s = e.to_string();
        assert!(s.contains("redb engine"), "msg: {s}");
        assert!(s.contains("table not found"), "msg: {s}");
    }

    #[test]
    fn codec_error_displays() {
        let e = StoreError::Codec("unexpected EOF".to_owned());
        let s = e.to_string();
        assert!(s.contains("cbor"), "msg: {s}");
    }
}
