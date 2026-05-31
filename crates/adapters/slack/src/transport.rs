//! HTTP transport abstraction for the Slack Web API.
//!
//! The adapter does not depend on a particular HTTP client. Implementors
//! of [`SlackTransport`] perform the actual network call; the adapter
//! invokes the transport at commit time. A [`MockTransport`] used by the
//! crate's tests captures sends in a `Vec` and never opens a socket,
//! satisfying the discipline that **no unit test ever performs real
//! network I/O** (`spec/adapter-interface.md` § 5).
//!
//! A real HTTP transport (`reqwest`-based or `slack-morphism`-based)
//! will land behind a cargo feature in a follow-up task; the trait is
//! shaped so adding it is a leaf change.

use crate::SlackOp;
use crate::error::SlackError;
use async_trait::async_trait;
use std::fmt;
use std::sync::{Mutex, PoisonError};

/// A captured transport invocation — the message id allocated by the
/// remote system on success, plus the raw response excerpt for
/// observability (never the request body, per spec.md § 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportRecord {
    /// The Slack-assigned message timestamp (`ts`), opaque to the
    /// adapter.
    pub message_ts: String,
    /// Adapter-defined diagnostic JSON (HTTP status, channel id,
    /// response code). Never includes the message text or the bot
    /// token.
    pub diagnostic: serde_json::Value,
}

/// Async HTTP transport for Slack Web API calls.
///
/// # Cancel Safety
///
/// Implementors should treat futures as **not cancel-safe**: dropping
/// the future mid-flight may leave the remote system in an arbitrary
/// state. The kernel commit drain coordinates compensation per the
/// adapter contract (`spec/adapter-interface.md` § 2).
#[async_trait]
pub trait SlackTransport: fmt::Debug + Send + Sync + 'static {
    /// Performs the side-effecting call described by `op`.
    ///
    /// On success returns a [`TransportRecord`] for downstream
    /// observability. On any failure returns a [`SlackError`].
    ///
    /// # Errors
    ///
    /// Returns a [`SlackError`] variant matching the transport-level
    /// failure mode; the adapter maps it to a protocol-surface error
    /// at the boundary.
    async fn send(&self, op: &SlackOp) -> Result<TransportRecord, SlackError>;
}

/// Test-only in-memory transport that records sends and replays
/// scripted responses.
///
/// Construct with [`MockTransport::new`] (default: every send
/// succeeds with a fixed `ts`). Use [`MockTransport::with_responses`]
/// to script alternating success and failure outcomes.
///
/// `MockTransport` lives in the crate's library surface — not behind
/// `#[cfg(test)]` — so the forthcoming `examples/dryrun/send_slack.py`
/// demo and external conformance harness can re-use it. It never
/// performs network I/O.
pub struct MockTransport {
    sends: Mutex<Vec<SlackOp>>,
    responses: Mutex<Vec<Result<TransportRecord, SlackError>>>,
    default_record: TransportRecord,
}

impl fmt::Debug for MockTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockTransport").finish_non_exhaustive()
    }
}

impl MockTransport {
    /// Constructs a transport that returns a fixed success record on
    /// every send.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sends: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
            default_record: TransportRecord {
                message_ts: "1700000000.000100".into(),
                diagnostic: serde_json::json!({"http_status": 200, "ok": true}),
            },
        }
    }

    /// Scripts a queue of responses; subsequent `send` calls dequeue
    /// from the front. When the queue is exhausted, falls back to the
    /// default response.
    #[must_use]
    pub fn with_responses(self, mut responses: Vec<Result<TransportRecord, SlackError>>) -> Self {
        responses.reverse();
        Self { responses: Mutex::new(responses), ..self }
    }

    /// Returns the operations captured so far in send order.
    #[must_use]
    pub fn captured(&self) -> Vec<SlackOp> {
        let guard = self.sends.lock().unwrap_or_else(PoisonError::into_inner);
        guard.clone()
    }

    /// Number of operations captured so far.
    #[must_use]
    pub fn send_count(&self) -> usize {
        self.captured().len()
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SlackTransport for MockTransport {
    async fn send(&self, op: &SlackOp) -> Result<TransportRecord, SlackError> {
        {
            let mut guard = self.sends.lock().unwrap_or_else(PoisonError::into_inner);
            guard.push(op.clone());
        }
        let mut responses = self.responses.lock().unwrap_or_else(PoisonError::into_inner);
        responses.pop().unwrap_or_else(|| Ok(self.default_record.clone()))
    }
}
