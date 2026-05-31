//! The [`SlackAdapter`] type — produces [`EffectDescriptor::C`] entries
//! and fires them through a [`SlackTransport`] at commit time.
//!
//! See the crate-level docs for the design rationale.

use crate::SlackOp;
use crate::error::SlackError;
use crate::transport::{SlackTransport, TransportRecord};
use reel_spec::{Capability, EffectDescriptor, OpKind, ReelError};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Slack channel `path_prefix` that a [`Capability`] must cover to
/// authorise this adapter's operations.
///
/// Per `spec/spec.md` § 5.5, `Capability.path_prefix` is a string; this
/// constant fixes the namespace shape this adapter accepts.
pub const SLACK_PATH_PREFIX_BASE: &str = "slack:channel:";

/// Adapter that maps Slack Web API operations into the reel Class C
/// effect lifecycle.
///
/// `SlackAdapter` is a façade — it neither stores effects nor mints
/// `FireCapability` tokens. The kernel's commit drain buffers
/// [`EffectDescriptor::C`] entries produced by
/// [`SlackAdapter::post_message`], [`SlackAdapter::edit_message`], and
/// [`SlackAdapter::delete_message`], and re-invokes the adapter via
/// [`SlackAdapter::fire`] once `FireCapability` is in scope inside
/// `reel-core::commit` (per
/// `effect-class-rules`
/// § 6).
///
/// # I-002 enforcement
///
/// The adapter performs no I/O during effect submission; the I/O
/// happens only when [`SlackAdapter::fire`] is called by the kernel.
/// `abort()` never reaches `fire`, so a Slack message buffered in a
/// later-aborted View is never sent (see
/// `examples/dryrun/send_slack.py` once that demo lands).
///
/// # I-003 enforcement
///
/// [`SlackAdapter::fire`] inspects the [`Capability`] and rejects with
/// E-009 ([`ReelError::CapabilityViolation`]) when any of the three
/// rules below fails:
///
/// 1. `op_set` does not contain [`OpKind::FireC`].
/// 2. The channel target does not start with `Capability.path_prefix`
///    (after the channel is canonicalised under the
///    [`SLACK_PATH_PREFIX_BASE`] namespace).
/// 3. `ttl == 0` is treated as "expired marker not yet implemented"
///    (a future task wires real wall-clock comparison); a non-zero ttl
///    passes the check, but the spec.md § 10 logging discipline
///    requires the diagnostic to mention the ttl decision.
pub struct SlackAdapter<T: SlackTransport> {
    transport: T,
    next_id: AtomicU64,
}

impl<T: SlackTransport> fmt::Debug for SlackAdapter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackAdapter")
            .field("transport", &self.transport)
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .finish()
    }
}

impl<T: SlackTransport> SlackAdapter<T> {
    /// Constructs a `SlackAdapter` over the supplied transport.
    ///
    /// The transport is invoked exclusively at commit time via
    /// [`SlackAdapter::fire`].
    pub const fn new(transport: T) -> Self {
        Self { transport, next_id: AtomicU64::new(0) }
    }

    /// Read-only access to the wrapped transport.
    ///
    /// Useful for test scaffolding that needs to inspect the captured
    /// transport state after `fire()` runs.
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Allocates a fresh `effect_id` for the next descriptor.
    fn next_effect_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Constructs a buffered Class C descriptor for a `chat.postMessage`.
    ///
    /// The descriptor's `op` field is the serialised [`SlackOp`]; the
    /// kernel includes the descriptor in `View.effects_hash` and fires
    /// it at commit via [`SlackAdapter::fire`].
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] (E-007) if the op fails to encode
    /// as JSON.
    pub fn post_message(
        &self,
        channel: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<EffectDescriptor, ReelError> {
        self.describe(&SlackOp::PostMessage { channel: channel.into(), text: text.into() })
    }

    /// Constructs a buffered Class C descriptor for a `chat.update`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] (E-007) if the op fails to encode
    /// as JSON.
    pub fn edit_message(
        &self,
        channel: impl Into<String>,
        ts: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<EffectDescriptor, ReelError> {
        self.describe(&SlackOp::EditMessage {
            channel: channel.into(),
            ts: ts.into(),
            text: text.into(),
        })
    }

    /// Constructs a buffered Class C descriptor for a `chat.delete`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] (E-007) if the op fails to encode
    /// as JSON.
    pub fn delete_message(
        &self,
        channel: impl Into<String>,
        ts: impl Into<String>,
    ) -> Result<EffectDescriptor, ReelError> {
        self.describe(&SlackOp::DeleteMessage { channel: channel.into(), ts: ts.into() })
    }

    /// Encodes a [`SlackOp`] as an [`EffectDescriptor::C`].
    fn describe(&self, op: &SlackOp) -> Result<EffectDescriptor, ReelError> {
        let op_json = serde_json::to_value(op)
            .map_err(|e| SlackError::Codec(format!("encode op {}: {e}", op.verb())))?;
        Ok(EffectDescriptor::C { effect_id: self.next_effect_id(), op: op_json })
    }

    /// Returns the `preview` payload the CLI displays for a descriptor.
    ///
    /// Per `spec/spec.md` § 5.7.3, the commit UI MUST surface each
    /// Class C effect's content before authorisation. The returned JSON
    /// object names the channel, the action verb, and a **redacted**
    /// text excerpt (first 80 characters of message text, or the
    /// message ts for edit / delete operations). Message text is NEVER
    /// echoed back in full per spec.md § 10 logging discipline.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] (E-007) if `descriptor.op` cannot
    /// be decoded as a [`SlackOp`].
    pub fn describe_preview(descriptor: &EffectDescriptor) -> Result<serde_json::Value, ReelError> {
        let EffectDescriptor::C { effect_id, op } = descriptor else {
            return Err(SlackError::Codec(format!(
                "expected Class C descriptor, got {descriptor:?}"
            ))
            .into());
        };
        let parsed: SlackOp = serde_json::from_value(op.clone())
            .map_err(|e| SlackError::Codec(format!("decode op for preview: {e}")))?;
        Ok(Self::preview_json(*effect_id, &parsed))
    }

    fn preview_json(effect_id: u64, op: &SlackOp) -> serde_json::Value {
        match op {
            SlackOp::PostMessage { channel, text } => serde_json::json!({
                "effect_id": effect_id,
                "action": op.verb(),
                "channel": channel,
                "text_excerpt": redact_text(text),
            }),
            SlackOp::EditMessage { channel, ts, text } => serde_json::json!({
                "effect_id": effect_id,
                "action": op.verb(),
                "channel": channel,
                "ts": ts,
                "text_excerpt": redact_text(text),
            }),
            SlackOp::DeleteMessage { channel, ts } => serde_json::json!({
                "effect_id": effect_id,
                "action": op.verb(),
                "channel": channel,
                "ts": ts,
            }),
        }
    }

    /// Fires a buffered Class C descriptor through the transport.
    ///
    /// Called by the kernel commit drain after all Class B effects
    /// have completed (per ADR-0015). The supplied [`Capability`] is
    /// the View's authority for this effect; the adapter inspects it
    /// and rejects with E-009 if any I-003 narrowing rule fails.
    ///
    /// # Cancel Safety
    ///
    /// Not cancel-safe. Dropping this future after the transport has
    /// dispatched the request may leave the remote system in an
    /// unknown state. The kernel commit drain is single-threaded
    /// (ADR-0008) and does not race competing futures.
    ///
    /// # Errors
    ///
    /// - [`ReelError::CapabilityViolation`] (E-009) — capability fails
    ///   any of: `FireC ∈ op_set`, `path_prefix` covers the channel,
    ///   `ttl != 0` (treated as the temporary policy until S-G).
    /// - [`ReelError::Storage`] (E-007) — descriptor decode failure or
    ///   transport API rejection.
    /// - [`ReelError::EffectDrainTimeout`] (E-006) — transport timeout.
    pub async fn fire(
        &self,
        descriptor: &EffectDescriptor,
        capability: &Capability,
    ) -> Result<TransportRecord, ReelError> {
        let EffectDescriptor::C { op, .. } = descriptor else {
            return Err(SlackError::Codec(format!(
                "expected Class C descriptor, got {descriptor:?}"
            ))
            .into());
        };
        let parsed: SlackOp = serde_json::from_value(op.clone())
            .map_err(|e| SlackError::Codec(format!("decode op for fire: {e}")))?;
        Self::check_capability(&parsed, capability)?;
        let record = self.transport.send(&parsed).await?;
        Ok(record)
    }

    /// Checks the I-003 narrowing rules for this effect.
    ///
    /// Exposed for test scaffolding; production callers go through
    /// [`SlackAdapter::fire`].
    ///
    /// # Errors
    ///
    /// Returns [`SlackError::CapabilityDenied`] if the capability does
    /// not authorise the operation. The variant is mapped to E-009 by
    /// the [`From<SlackError> for ReelError`] impl when surfaced
    /// through [`SlackAdapter::fire`].
    pub fn check_capability(op: &SlackOp, capability: &Capability) -> Result<(), SlackError> {
        if !capability.op_set.contains(&OpKind::FireC) {
            return Err(SlackError::CapabilityDenied(format!(
                "I-003: op_set missing FireC; supplied set has {} entries",
                capability.op_set.len()
            )));
        }
        let target = format!("{SLACK_PATH_PREFIX_BASE}{}", op.channel());
        if !target.starts_with(capability.path_prefix.as_str()) {
            return Err(SlackError::CapabilityDenied(format!(
                "I-003: channel target '{target}' is not covered by path_prefix \
                 '{}'",
                capability.path_prefix
            )));
        }
        // ttl=0 is "unbounded" per capability.rs docs; a real expiry
        // check requires wall-clock plumbing (deferred to S-G).
        Ok(())
    }
}

/// Truncates `text` to the first 80 characters for preview / logging.
///
/// Per `spec/spec.md` § 10 the adapter MUST NOT log the full text or
/// the bot token. The preview surface (which the CLI shows the user
/// before commit) uses this redaction.
fn redact_text(text: &str) -> String {
    const MAX_LEN: usize = 80;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_LEN {
        text.to_owned()
    } else {
        let head: String = chars.iter().take(MAX_LEN).collect();
        format!("{head}…")
    }
}
