//! Adapter operation enum.
//!
//! [`SlackOp`] enumerates the three Slack Web API verbs this adapter
//! exposes. Each variant carries the parameters needed to construct
//! both the in-buffer [`reel_spec::EffectDescriptor::C`] payload and
//! the eventual remote API call.

use serde::{Deserialize, Serialize};

/// The three Class C operations exposed by the Slack adapter.
///
/// Each variant is shaped to match a Slack Web API method:
///
/// - [`SlackOp::PostMessage`] ↔ `chat.postMessage`
/// - [`SlackOp::EditMessage`] ↔ `chat.update`
/// - [`SlackOp::DeleteMessage`] ↔ `chat.delete`
///
/// Editing and deletion are treated as **separate** Class C effects —
/// they are not "undo" of [`SlackOp::PostMessage`]. The protocol does
/// not allow Class C compensation across `abort` (`spec/spec.md`
/// § 5.7.3, I-002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SlackOp {
    /// Send a new chat message.
    PostMessage {
        /// Slack channel (e.g. `"#general"`).
        channel: String,
        /// Message body text.
        text: String,
    },

    /// Update an existing chat message.
    EditMessage {
        /// Slack channel.
        channel: String,
        /// The `ts` (timestamp) identifier of the message being edited.
        ts: String,
        /// New message body text.
        text: String,
    },

    /// Delete an existing chat message.
    DeleteMessage {
        /// Slack channel.
        channel: String,
        /// The `ts` (timestamp) identifier of the message being deleted.
        ts: String,
    },
}

impl SlackOp {
    /// Returns the channel string this operation targets.
    #[must_use]
    pub fn channel(&self) -> &str {
        match self {
            Self::PostMessage { channel, .. }
            | Self::EditMessage { channel, .. }
            | Self::DeleteMessage { channel, .. } => channel,
        }
    }

    /// Returns the canonical action verb (`"post_message"`, etc.).
    #[must_use]
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::PostMessage { .. } => "post_message",
            Self::EditMessage { .. } => "edit_message",
            Self::DeleteMessage { .. } => "delete_message",
        }
    }
}
