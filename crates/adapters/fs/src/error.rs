//! Error catalogue for the filesystem adapter.
//!
//! [`FsAdapterError`] wraps the protocol-surface [`reel_spec::ReelError`]
//! catalogue with adapter-local detail.  Per `spec/adapter-interface.md`
//! §7 the adapter is responsible for translating its native errors into
//! the corresponding reel error class — variant mapping:
//!
//! | Variant | Maps to |
//! |---------|---------|
//! | [`FsAdapterError::CapabilityViolation`] | `E-009 CapabilityViolation` (I-003) |
//! | [`FsAdapterError::Io`] | `E-007 Storage` |
//! | [`FsAdapterError::NamespaceEscape`] | `E-009 CapabilityViolation` (defence-in-depth path-traversal check) |
//!
//! All variants are recoverable except `Io`, which is treated as
//! critical per `spec/errors.yaml` E-007.

use reel_spec::ReelError;
use std::io;
use thiserror::Error;

/// Errors emitted by [`crate::FsAdapter`] operations.
///
/// # Mapping
///
/// Each variant has a designated [`ReelError`] counterpart on the
/// protocol surface — see the table in the crate-level docs.
///
/// # Examples
///
/// ```rust
/// use adapter_fs::FsAdapterError;
/// use reel_spec::ReelError;
///
/// let err = FsAdapterError::CapabilityViolation(
///     "path 'outside/' escapes prefix 'ws/'".into(),
/// );
/// let reel: ReelError = err.into();
/// assert!(matches!(reel, ReelError::CapabilityViolation(_)));
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsAdapterError {
    /// I-003 violation: the requested op falls outside the Capability's
    /// `path_prefix` or `op_set`.
    ///
    /// Maps to `E-009 CapabilityViolation`.
    #[error("E-009 CapabilityViolation: {0}")]
    CapabilityViolation(String),

    /// Defence-in-depth: the requested op's `name` contains `..` or
    /// otherwise tries to escape the adapter's `root` directory after
    /// path canonicalisation.  Reported as `E-009` so callers can route
    /// it identically to Capability checks.
    ///
    /// Maps to `E-009 CapabilityViolation`.
    #[error("E-009 NamespaceEscape: {0}")]
    NamespaceEscape(String),

    /// Underlying filesystem I/O failed during drain materialisation.
    ///
    /// Maps to `E-007 Storage`.
    #[error("E-007 Storage: {0}")]
    Io(String),
}

impl From<FsAdapterError> for ReelError {
    fn from(err: FsAdapterError) -> Self {
        match err {
            FsAdapterError::CapabilityViolation(msg) | FsAdapterError::NamespaceEscape(msg) => {
                Self::CapabilityViolation(msg)
            }
            FsAdapterError::Io(msg) => Self::Storage(msg),
        }
    }
}

impl From<io::Error> for FsAdapterError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "test assertions intentionally panic on failure")]
mod tests {
    use super::*;

    #[test]
    fn cap_violation_maps_to_e009() {
        let err = FsAdapterError::CapabilityViolation("outside prefix".into());
        let reel: ReelError = err.into();
        assert!(matches!(reel, ReelError::CapabilityViolation(_)), "expected E-009");
    }

    #[test]
    fn namespace_escape_maps_to_e009() {
        let err = FsAdapterError::NamespaceEscape("../etc/passwd".into());
        let reel: ReelError = err.into();
        assert!(matches!(reel, ReelError::CapabilityViolation(_)), "expected E-009");
    }

    #[test]
    fn io_maps_to_e007() {
        let err = FsAdapterError::Io("disk full".into());
        let reel: ReelError = err.into();
        assert!(matches!(reel, ReelError::Storage(_)), "expected E-007");
    }

    #[test]
    fn display_contains_code() {
        assert!(FsAdapterError::CapabilityViolation("x".into()).to_string().contains("E-009"));
        assert!(FsAdapterError::NamespaceEscape("x".into()).to_string().contains("E-009"));
        assert!(FsAdapterError::Io("x".into()).to_string().contains("E-007"));
    }
}
