//! Ordered mutations from a base Block — the reel `Delta` type.
//!
//! A [`Delta`] records a sequence of [`Op`] operations anchored at a
//! base Block identified by [`crate::Hash`].
//! Views carry one Delta layer above their inherited namespace.
//!
//! See `spec/spec.md` §5.4.
//!
//! # Complexity
//!
//! - `fork`: the child receives an empty `Delta` in O(1) (no copy of
//!   the parent's namespace).

use crate::Hash;
use serde::{Deserialize, Serialize};

/// Ordered mutations from a base [`crate::Block`].
///
/// A `Delta` anchors mutations at `base_hash` (the Block from which the
/// parent's effective namespace is computed) and stores an ordered list
/// of [`Op`] values.
///
/// A [`crate::View`] carries one `Delta` layer above its inherited
/// namespace.
/// `fork` assigns the child an empty `Delta` with the parent's effective
/// hash as `base_hash`.
/// `commit` applies the `Delta` to the parent.
/// `abort` discards it.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{Delta, Hash, Op};
///
/// let base = Hash::from_bytes([0u8; 32]);
/// let target = Hash::from_bytes(*blake3::hash(b"file-content").as_bytes());
///
/// let delta = Delta {
///     base_hash: base,
///     ops: vec![Op::Put {
///         name: "README.md".into(),
///         hash: target,
///     }],
/// };
/// assert_eq!(delta.ops.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// The Block from which this Delta starts.
    pub base_hash: Hash,

    /// Mutations applied in insertion order.
    pub ops: Vec<Op>,
}

/// A single mutation operation within a [`Delta`].
///
/// The protocol defines `Put` and `Remove` as the minimum required set;
/// per-adapter op kinds may grow beyond this minimum.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{Hash, Op};
///
/// let h = Hash::from_bytes(*blake3::hash(b"content").as_bytes());
/// let put = Op::Put { name: "file.txt".into(), hash: h };
/// let remove = Op::Remove { name: "old.txt".into() };
///
/// assert!(matches!(put, Op::Put { .. }));
/// assert!(matches!(remove, Op::Remove { .. }));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// Inserts or replaces a name → hash mapping in the namespace.
    Put {
        /// The namespace entry name being set.
        name: String,
        /// The Block hash the entry is set to.
        hash: Hash,
    },

    /// Removes a name from the namespace.
    Remove {
        /// The namespace entry name being removed.
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn delta_serde_round_trip() -> Result<(), Box<dyn Error>> {
        let base = Hash::from_bytes([0u8; 32]);
        let h = Hash::from_bytes(*blake3::hash(b"data").as_bytes());
        let delta = Delta {
            base_hash: base,
            ops: vec![
                Op::Put { name: "a.txt".into(), hash: h },
                Op::Remove { name: "b.txt".into() },
            ],
        };
        let json = serde_json::to_string(&delta)?;
        let d2: Delta = serde_json::from_str(&json)?;
        assert_eq!(delta, d2);
        Ok(())
    }

    #[test]
    fn op_serde_tag_kind() -> Result<(), Box<dyn Error>> {
        let h = Hash::from_bytes([1u8; 32]);
        let put = Op::Put { name: "f".into(), hash: h };
        let json = serde_json::to_string(&put)?;
        // The JSON tag must use the word "put".
        assert!(json.contains("\"kind\":\"put\""), "tag absent: {json}");
        Ok(())
    }

    #[test]
    fn empty_delta() {
        let delta = Delta { base_hash: Hash::from_bytes([0u8; 32]), ops: vec![] };
        assert!(delta.ops.is_empty());
    }
}
