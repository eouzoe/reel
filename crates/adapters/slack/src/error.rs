//! Adapter-local error type and conversion to [`reel_spec::ReelError`].
//!
//! `SlackError` records adapter-specific failure cases and maps them to
//! the protocol-surface [`ReelError`] catalogue from `spec/errors.yaml`
//! per `spec/adapter-interface.md` § 7.
//!
//! | Adapter signal                  | Reel error                              |
//! |---------------------------------|-----------------------------------------|
//! | Capability does not authorise   | [`ReelError::CapabilityViolation`] E-009 |
//! | Slack API timeout               | [`ReelError::EffectDrainTimeout`] E-006 |
//! | Slack API rejected the request  | [`ReelError::Storage`] E-007            |
//! | Local serialisation failure     | [`ReelError::Storage`] E-007            |

use reel_spec::ReelError;
use thiserror::Error;

/// Adapter-local errors raised inside [`crate::SlackAdapter`].
///
/// Mapped to [`reel_spec::ReelError`] at the adapter boundary so callers
/// (the kernel commit drain) see only protocol-surface errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SlackError {
    /// The [`reel_spec::Capability`] supplied at `fire` does not authorise
    /// the requested operation (missing `FireC` in `op_set`, channel
    /// outside `path_prefix`, or expired TTL marker).
    #[error("capability does not authorise this slack operation: {0}")]
    CapabilityDenied(String),

    /// Remote Slack API timed out while the adapter awaited a response.
    #[error("slack api timeout")]
    Timeout,

    /// Remote Slack API rejected the request (non-2xx status, `ok=false`,
    /// or transport-level failure that is not a timeout).
    #[error("slack api rejected request: {0}")]
    Api(String),

    /// Local encoding or response decoding failure.
    #[error("slack payload codec error: {0}")]
    Codec(String),
}

impl From<SlackError> for ReelError {
    fn from(value: SlackError) -> Self {
        match value {
            SlackError::CapabilityDenied(msg) => Self::CapabilityViolation(msg),
            SlackError::Timeout => Self::EffectDrainTimeout,
            SlackError::Api(msg) | SlackError::Codec(msg) => Self::Storage(msg),
        }
    }
}
