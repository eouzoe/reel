//! Immutable, content-addressed Block.
//!
//! A [`Block`] is reel's primitive storage unit. **Identity is a pure
//! function of content**: `hash = BLAKE3(data)`. Two Blocks with identical
//! `data` have identical [`struct@Hash`] and are therefore the *same* Block
//! — the property content-addressing and forest-GC deduplication depend on.
//!
//! ## Why provenance is not here
//!
//! A Block's lineage — who/when/how it came to be — is deliberately **not**
//! part of its identity and **not** a field of `Block`. Because identical
//! content deduplicates to one Block, a single physical Block can be
//! introduced by many different events, so it cannot own one lineage record.
//! Lineage is a *relationship*, recorded on the commit that introduced the
//! content (reel's analogue of a Nix derivation / Git commit), not on the
//! content unit. See [`crate::Provenance`] and `spec/spec.md` §5.2.
//!
//! Putting provenance in the hash (as a prior iteration did) gives every
//! Block a creation-time-dependent identity, which silently defeats dedup
//! and breaks determinism — see the regression guard
//! [`tests::identical_content_is_identical_block`].
//!
//! ## Zero-copy payload
//!
//! `data` is [`bytes::Bytes`]: cloning a Block is an `O(1)` reference-count
//! bump (not a memcpy) and slices share the allocation, so sharing a Block
//! across Views and the store — the common operation under forest-GC
//! borrowing — is free.
//!
//! See `spec/spec.md` §5.2, ADR-0025, and ADR-0004 (BLAKE3-256).
//!
//! # Invariants
//!
//! - **I-001**: [`struct@Hash`] identifies content; no code path grants
//!   access to a Block solely on knowing its hash (`spec/invariants.yaml`).
//!
//! # References
//!
//! - Wang, C., Zheng, Y. (2026). Fork, Explore, Commit. arXiv 2602.08199v2.
//! - IPFS Merkle-DAG: <https://github.com/ipfs/specs/blob/main/MERKLE_DAG.md>
//! - Dolstra, E. (2006). The Purely Functional Software Deployment Model.
//!   (Nix: content-address = *what it is*; derivation = *how it was made*.)
//! - ADR-0004: BLAKE3-256 as Block hash. ADR-0025: six types.

use crate::{Hash, ReelError};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Maximum payload size for a [`Block`] (16 MiB per `spec/spec.md` §12).
///
/// [`Block::new`] enforces this at construction time, returning
/// [`ReelError::Storage`] when `data.len() > MAX_BLOCK_SIZE`.
pub const MAX_BLOCK_SIZE: usize = 16 * 1024 * 1024;

/// An immutable, content-addressed unit of reel's data model.
///
/// Identity is a pure function of content: `hash = BLAKE3(data)`. Two Blocks
/// with the same `data` are the same Block. Blocks have no class and no
/// provenance — classes are properties of effect *descriptors* within a
/// payload (`spec/spec.md` §5.2), and lineage is recorded on the introducing
/// commit (see the module docs).
///
/// `data` is [`bytes::Bytes`], so [`Clone`] is an `O(1)` refcount bump.
///
/// # Construction
///
/// Use [`Block::new`]; it computes the hash and enforces [`MAX_BLOCK_SIZE`].
/// Direct struct construction is only for tests / serde paths where the hash
/// is already known-good.
///
/// # Examples
///
/// ```rust
/// use reel_spec::Block;
///
/// let block = Block::new(b"hello reel".to_vec()).expect("within 16 MiB");
/// assert_eq!(block.hash().as_bytes().len(), 32);
///
/// // Content addressing: identical bytes ⇒ identical Block.
/// let again = Block::new(b"hello reel".to_vec()).expect("within 16 MiB");
/// assert_eq!(block.hash(), again.hash());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// BLAKE3-256 digest of `data` (per ADR-0004). Block identity.
    pub hash: Hash,

    /// Opaque, zero-copy-shareable payload; up to [`MAX_BLOCK_SIZE`].
    pub data: Bytes,
}

impl Block {
    /// Construct a Block from `data`, computing `hash = BLAKE3(data)`.
    ///
    /// Accepts anything convertible into [`bytes::Bytes`] (`Vec<u8>`,
    /// `&'static [u8]`, `Bytes`, …). The conversion is move/zero-copy for
    /// `Vec<u8>` and `Bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] when `data.len() > MAX_BLOCK_SIZE`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Block;
    ///
    /// let block = Block::new(b"agent output".to_vec()).expect("small payload");
    /// assert_eq!(block.hash().as_bytes().len(), 32);
    /// ```
    pub fn new(data: impl Into<Bytes>) -> Result<Self, ReelError> {
        let data = data.into();
        if data.len() > MAX_BLOCK_SIZE {
            return Err(ReelError::Storage(format!(
                "Block payload exceeds MAX_BLOCK_SIZE ({} > {} bytes)",
                data.len(),
                MAX_BLOCK_SIZE
            )));
        }
        let hash = content_hash(&data);
        Ok(Self { hash, data })
    }

    /// The cached BLAKE3-256 digest of this Block's content (`O(1)`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Block;
    ///
    /// let block = Block::new(b"reel".to_vec()).expect("small payload");
    /// assert_eq!(block.hash(), block.hash(), "hash accessor is idempotent");
    /// ```
    #[must_use]
    pub const fn hash(&self) -> Hash {
        self.hash
    }
}

/// Compute a Block's content address: `BLAKE3(data)`.
///
/// Pure and total — identity depends on content alone, nothing else.
#[must_use]
pub fn content_hash(data: &[u8]) -> Hash {
    Hash::from_bytes(*blake3::hash(data).as_bytes())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: expect()/panic on infallible-by-construction fixtures is acceptable"
)]
mod tests {
    use super::*;
    use ciborium::ser::into_writer as cbor_enc;
    use std::error::Error;

    fn small_block() -> Block {
        Block::new(b"test block data".to_vec()).expect("small payload")
    }

    // ── content-addressed identity ─────────────────────────────────────────

    #[test]
    fn block_new_computes_content_hash() {
        let data = b"hello reel".to_vec();
        let block = Block::new(data.clone()).expect("construction succeeds");
        assert_eq!(block.hash, content_hash(&data), "Block::new must store BLAKE3(data)");
    }

    /// Regression guard for the provenance-in-hash defect (F1): identical
    /// content MUST produce an identical Block, with nothing time-dependent
    /// in the identity. This is the property forest-GC dedup relies on.
    #[test]
    fn identical_content_is_identical_block() {
        let a = Block::new(b"identical logical content".to_vec()).expect("ok");
        let b = Block::new(b"identical logical content".to_vec()).expect("ok");
        assert_eq!(a.hash(), b.hash(), "identical content ⇒ identical hash (dedup foundation)");
        assert_eq!(a, b, "identical content ⇒ structurally equal Block");
    }

    #[test]
    fn different_data_different_hash() {
        let a = Block::new(b"payload A".to_vec()).expect("ok");
        let b = Block::new(b"payload B".to_vec()).expect("ok");
        assert_ne!(a.hash(), b.hash(), "distinct data ⇒ distinct hash");
    }

    // ── size enforcement ───────────────────────────────────────────────────

    #[test]
    fn block_new_rejects_oversized() {
        let big = vec![0u8; MAX_BLOCK_SIZE + 1];
        let err = Block::new(big).expect_err("must reject oversized");
        match err {
            ReelError::Storage(msg) => {
                assert!(msg.contains("MAX_BLOCK_SIZE"), "should mention MAX_BLOCK_SIZE: {msg}");
            }
            other @ (ReelError::BlockNotFound(_)
            | ReelError::CommitConflict(_)
            | ReelError::ClassCAfterAbort
            | ReelError::ForkDepthExceeded(_)
            | ReelError::EffectDrainTimeout
            | ReelError::ViewTerminal(_)
            | ReelError::CapabilityViolation(_)
            | ReelError::ReachabilityViolation(_)) => panic!("expected Storage, got {other:?}"),
        }
    }

    #[test]
    fn block_new_accepts_max_size() {
        Block::new(vec![0u8; MAX_BLOCK_SIZE]).expect("exact MAX_BLOCK_SIZE is allowed");
    }

    #[test]
    fn max_block_size_value() {
        assert_eq!(MAX_BLOCK_SIZE, 16 * 1024 * 1024, "MAX_BLOCK_SIZE must be exactly 16 MiB");
    }

    // ── hash accessor ──────────────────────────────────────────────────────

    #[test]
    fn hash_accessor_idempotent() {
        let b = small_block();
        assert_eq!(b.hash(), b.hash(), "hash() idempotent");
        assert_eq!(b.hash(), b.hash, "hash() matches the stored field");
    }

    // ── serde ──────────────────────────────────────────────────────────────
    //
    // NOTE: full Block CBOR *read* is still blocked by `Hash`'s hex_serde
    //. The write
    // path is exercised here; JSON round-trips fully.

    #[test]
    fn block_cbor_serialises_without_error() -> Result<(), Box<dyn Error>> {
        let mut buf = Vec::new();
        cbor_enc(&small_block(), &mut buf)?;
        assert!(!buf.is_empty(), "CBOR output must be non-empty");
        Ok(())
    }

    #[test]
    fn block_json_round_trip() -> Result<(), Box<dyn Error>> {
        let b = small_block();
        let json = serde_json::to_string(&b)?;
        let b2: Block = serde_json::from_str(&json)?;
        assert_eq!(b.hash, b2.hash, "JSON round-trip preserves hash");
        assert_eq!(b.data, b2.data, "JSON round-trip preserves data");
        Ok(())
    }

    // ── property: hash == BLAKE3(data) for any payload ──────────────────────

    mod property {
        use super::*;
        use proptest::collection::vec as pvec;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn hash_equals_blake3_data(data in pvec(any::<u8>(), 0..=4096usize)) {
                let block = Block::new(data.clone()).expect("within MAX_BLOCK_SIZE");
                prop_assert_eq!(block.hash(), content_hash(&data));
            }

            // Determinism + dedup: same bytes ⇒ same hash, always.
            #[test]
            fn same_bytes_same_hash(data in pvec(any::<u8>(), 0..=4096usize)) {
                let a = Block::new(data.clone()).expect("ok");
                let b = Block::new(data).expect("ok");
                prop_assert_eq!(a.hash(), b.hash());
            }
        }
    }
}
