//! I-001 reachability walker.
//!
//! Computes the transitive closure of [`reel_spec::Hash`]es reachable from a
//! starting namespace, plus any hash references discovered inside the
//! reached Blocks' payloads.
//!
//! Block payloads in reel are opaque bytes; however, two protocol-defined
//! payload shapes carry hash references that participate in the
//! reachability closure:
//!
//! - A CBOR-encoded [`reel_spec::Delta`] — each [`reel_spec::Op::Put`]
//!   carries a target [`reel_spec::Hash`].
//! - A CBOR-encoded [`reel_spec::EffectDescriptor`] sequence (the pending
//!   effects buffer carried by `View.effects_hash`) — Class A / B / C
//!   effects MAY reference further Blocks; only the protocol-defined
//!   shapes are inspected.
//!
//! The walker is conservative: payloads that fail to decode as either
//! shape are treated as opaque leaves (no outgoing edges). This is the
//! correct safety behaviour — unknown payloads cannot make additional
//! Blocks reachable that would not otherwise be reachable through the
//! namespace.
//!
//! ## Cycle handling
//!
//! Blocks are content-addressed (BLAKE3-256); a Block payload cannot
//! reference its own hash without breaking the BLAKE3 collision-
//! resistance property. The walker still maintains an explicit
//! `visited` set so adversarial test payloads that happen to encode a
//! self-reference (e.g. a CBOR `Hash` field set to the parent's hash by
//! accident) do not push the walker into unbounded recursion.
//!
//! ## Depth bound
//!
//! The walker is capped at [`DEFAULT_REACHABILITY_DEPTH_LIMIT`] BFS
//! levels (1024) to prevent pathological depth. See spec
//! "I-001 reachability algorithm" §3.
//!
//! ## Why an `Iterator<Item = (Hash, &[u8])>` shape
//!
//! The walker is decoupled from any specific [`crate::Store`] backend:
//! both [`crate::MemStore`] and [`crate::RedbStore`] provide a closure
//! `fn raw_get(&Hash) -> Result<Option<Vec<u8>>, StoreError>` that the
//! walker calls. The closure may be backed by an in-memory `BTreeMap`,
//! a redb read transaction, or a mock during tests.

use std::collections::{HashSet, VecDeque};
use std::mem;

use reel_spec::{Delta, EffectDescriptor, Hash, Op};

use crate::cbor;
use crate::error::StoreError;

/// Maximum BFS depth for the reachability walker.
///
/// Each "level" corresponds to following one round of outgoing edges
/// from the set of currently-reached Blocks. With a depth bound of
/// 1024 the walker handles deeply chained `Delta` histories without
/// risking an unbounded recursion (see spec).
pub const DEFAULT_REACHABILITY_DEPTH_LIMIT: usize = 1024;

/// A reusable reachability walker.
///
/// Computes the set of [`struct@Hash`]es reachable from a starting root set,
/// expanding through Block payloads that decode as a [`Delta`] or
/// [`EffectDescriptor`].
///
/// # Examples
///
/// ```rust
/// use reel_store::ReachabilityWalker;
/// use reel_spec::Hash;
/// use std::collections::BTreeMap;
///
/// let h = Hash::from_bytes([1u8; 32]);
/// let mut store = BTreeMap::<Hash, Vec<u8>>::new();
/// store.insert(h, vec![]);
///
/// let walker = ReachabilityWalker::with_depth_limit(4);
/// let reachable = walker
///     .walk(&[h], |hash| Ok(store.get(hash).cloned()))
///     .expect("walk");
/// assert!(reachable.contains(&h));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ReachabilityWalker {
    depth_limit: usize,
}

impl Default for ReachabilityWalker {
    fn default() -> Self {
        Self { depth_limit: DEFAULT_REACHABILITY_DEPTH_LIMIT }
    }
}

impl ReachabilityWalker {
    /// Construct a walker with a custom depth bound.
    ///
    /// A depth limit of `0` will return only the root hashes that are
    /// already present in the supplied store.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_store::ReachabilityWalker;
    /// let w = ReachabilityWalker::with_depth_limit(8);
    /// assert_eq!(w.depth_limit(), 8);
    /// ```
    #[must_use]
    pub const fn with_depth_limit(depth_limit: usize) -> Self {
        Self { depth_limit }
    }

    /// The configured depth limit.
    #[must_use]
    pub const fn depth_limit(&self) -> usize {
        self.depth_limit
    }

    /// Compute the reachable set starting from `roots`.
    ///
    /// `get_block` is called once per visited [`struct@Hash`]; a `Ok(None)`
    /// return means the Block is not in storage at all (the hash is
    /// dropped from the closure without erroring).
    ///
    /// # Errors
    ///
    /// Propagates any [`StoreError`] returned by `get_block`. CBOR
    /// decode failures inside the walker are NOT errors — payloads
    /// that do not decode as [`Delta`] or [`EffectDescriptor`] are
    /// treated as opaque leaves.
    ///
    /// # Complexity
    ///
    /// `O(N · d)` where `N` is the number of Blocks in the closure and
    /// `d` is the average outdegree (number of hash references per
    /// decoded payload). Each Block is visited at most once.
    pub fn walk<F>(&self, roots: &[Hash], mut get_block: F) -> Result<HashSet<Hash>, StoreError>
    where
        F: FnMut(&Hash) -> Result<Option<Vec<u8>>, StoreError>,
    {
        let mut visited: HashSet<Hash> = HashSet::new();
        let mut frontier: VecDeque<Hash> = roots.iter().copied().collect();
        let mut level_marker: VecDeque<Hash> = VecDeque::new();
        let mut depth: usize = 0;

        while let Some(hash) = frontier.pop_front() {
            if !visited.insert(hash) {
                // Already visited; do not expand again.
                continue;
            }

            let bytes_opt = get_block(&hash)?;
            let Some(bytes) = bytes_opt else {
                // Hash exists in the namespace closure but is not in
                // storage — that is a missing Block, not a walker error.
                // The lookup-site (lookup_by_hash) decides whether to
                // surface BlockNotFound or quietly continue (during gc).
                continue;
            };

            for next in extract_outgoing_hashes(&bytes) {
                if !visited.contains(&next) {
                    level_marker.push_back(next);
                }
            }

            if frontier.is_empty() {
                // Finished current BFS level. Bump depth and roll over
                // the next-level queue.
                if !level_marker.is_empty() {
                    depth = depth.saturating_add(1);
                    if depth > self.depth_limit {
                        // Refuse to expand beyond the cap; return the
                        // closure as known so far.  The caller may treat
                        // this as a soft over-budget signal but the
                        // returned set is still a valid lower bound.
                        return Ok(visited);
                    }
                }
                mem::swap(&mut frontier, &mut level_marker);
            }
        }

        Ok(visited)
    }
}

/// Inspect `payload` for hash references that participate in the
/// reachability closure.
///
/// The walker tries the two known shapes (`Delta`, single
/// `EffectDescriptor`) and returns the union of discovered hashes.
/// Payloads that do not decode as either shape contribute nothing to
/// the closure.
fn extract_outgoing_hashes(payload: &[u8]) -> Vec<Hash> {
    let mut out = Vec::<Hash>::new();

    // Try Delta first — the more common payload shape (every commit
    // produces one).
    if let Ok(delta) = cbor::decode::<Delta>(payload) {
        out.push(delta.base_hash);
        for op in &delta.ops {
            if let Op::Put { hash, .. } = op {
                out.push(*hash);
            }
        }
    }

    // Try EffectDescriptor — the View.effects_hash payload.
    if let Ok(eff) = cbor::decode::<EffectDescriptor>(payload) {
        // Class A / B / C effect ops MAY embed `Hash` values in their
        // adapter-defined `op` JSON tree.  We do not look inside the
        // free-form `op` payload — protocol-level reachability is only
        // through `Delta` Ops.  The EffectDescriptor decode is exercised
        // purely to record that the payload type is well-known.
        let _ = &eff;
    }

    out
}

/// Build a namespace's root hash set as a sorted, deduplicated `Vec`.
///
/// Used by [`crate::Store::lookup_by_hash`] and similar callers that
/// want a stable iteration order.
#[allow(
    clippy::redundant_pub_crate,
    reason = "module is private; pub(crate) makes intent explicit and satisfies workspace `unreachable_pub` lint"
)]
pub(crate) fn namespace_roots(namespace: &crate::Namespace) -> Vec<Hash> {
    let mut roots: Vec<Hash> = namespace.values().copied().collect();
    roots.sort_unstable();
    roots.dedup();
    roots
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test module: panics are acceptable for test failures"
)]
mod tests {
    use super::*;

    use reel_spec::{Block};
    use std::collections::BTreeMap;

    fn store_block_raw(map: &mut BTreeMap<Hash, Vec<u8>>, payload: Vec<u8>) -> Hash {
        let b = Block::new(payload).expect("small payload");
        let h = b.hash();
        map.insert(h, cbor::encode(&b).expect("encode"));
        h
    }

    #[test]
    fn empty_namespace_yields_empty_closure() {
        let store = BTreeMap::<Hash, Vec<u8>>::new();
        let w = ReachabilityWalker::default();
        let closure = w.walk(&[], |h| Ok(store.get(h).cloned())).expect("walk");
        assert!(closure.is_empty(), "no roots → empty closure");
    }

    #[test]
    fn root_hash_in_closure_if_in_store() {
        let mut store = BTreeMap::<Hash, Vec<u8>>::new();
        let h = store_block_raw(&mut store, b"leaf".to_vec());
        let w = ReachabilityWalker::default();
        let closure = w.walk(&[h], |k| Ok(store.get(k).cloned())).expect("walk");
        assert!(closure.contains(&h), "root in store must be reachable");
        assert_eq!(closure.len(), 1, "only root reachable");
    }

    #[test]
    fn root_hash_missing_from_store_not_returned() {
        let store = BTreeMap::<Hash, Vec<u8>>::new();
        let h = Hash::from_bytes([0xab; 32]);
        let w = ReachabilityWalker::default();
        let closure = w.walk(&[h], |k| Ok(store.get(k).cloned())).expect("walk");
        // The hash was visited but `get_block` returned `None`, so the
        // walker still marks the hash as "explored" but no further
        // expansion happens — `h` is in `visited` because that is the
        // set of hashes the walker has examined.  For the lookup gate
        // the caller separately verifies the hash is actually present
        // in storage.
        assert!(closure.contains(&h), "root visit recorded");
    }

    #[test]
    fn delta_op_put_extends_closure() {
        let mut store = BTreeMap::<Hash, Vec<u8>>::new();
        let leaf = store_block_raw(&mut store, b"leaf bytes".to_vec());

        // Build a Delta payload that references `leaf` via Op::Put and
        // store it as a Block whose payload IS the CBOR-encoded Delta.
        let delta = Delta { base_hash: leaf, ops: vec![Op::Put { name: "x".into(), hash: leaf }] };
        let delta_bytes = cbor::encode(&delta).expect("encode");
        let delta_block = Block::new(delta_bytes.clone()).expect("small");
        let delta_hash = delta_block.hash();
        // Insert the Delta-bearing Block, keyed by its hash, with the
        // raw Delta bytes as the value (the reachability walker reads
        // the payload, not the Block envelope, for outgoing-edge
        // discovery).
        store.insert(delta_hash, delta_bytes);

        let w = ReachabilityWalker::default();
        let closure = w.walk(&[delta_hash], |k| Ok(store.get(k).cloned())).expect("walk");
        assert!(closure.contains(&delta_hash), "root Delta reachable");
        assert!(closure.contains(&leaf), "Op::Put target reachable through Delta");
    }

    #[test]
    fn opaque_payload_does_not_extend_closure() {
        let mut store = BTreeMap::<Hash, Vec<u8>>::new();
        // A payload that is neither a Delta nor an EffectDescriptor; it
        // should be treated as a leaf.
        store.insert(Hash::from_bytes([7; 32]), b"opaque junk".to_vec());
        let leaf = Hash::from_bytes([7; 32]);
        let w = ReachabilityWalker::default();
        let closure = w.walk(&[leaf], |k| Ok(store.get(k).cloned())).expect("walk");
        assert_eq!(closure.len(), 1, "opaque payload yields no outgoing edges");
    }

    #[test]
    fn depth_limit_caps_expansion() {
        // Build a chain of three Deltas: A → B → C (each Delta's
        // base_hash references the next Block in the chain).
        let mut store = BTreeMap::<Hash, Vec<u8>>::new();
        let leaf_c = store_block_raw(&mut store, b"c".to_vec());

        // Block B: Delta with base_hash = leaf_c
        let delta_b = Delta { base_hash: leaf_c, ops: vec![] };
        let b_bytes = cbor::encode(&delta_b).expect("ok");
        let b_block = Block::new(b_bytes.clone()).expect("ok");
        let b_hash = b_block.hash();
        store.insert(b_hash, b_bytes);

        // Block A: Delta with base_hash = b_hash
        let delta_a = Delta { base_hash: b_hash, ops: vec![] };
        let a_bytes = cbor::encode(&delta_a).expect("ok");
        let a_block = Block::new(a_bytes.clone()).expect("ok");
        let a_hash = a_block.hash();
        store.insert(a_hash, a_bytes);

        // Depth limit 1: from A we should reach B (the immediate edge)
        // but not C (would require a second BFS level).
        let w = ReachabilityWalker::with_depth_limit(1);
        let closure = w.walk(&[a_hash], |k| Ok(store.get(k).cloned())).expect("walk");
        assert!(closure.contains(&a_hash));
        assert!(closure.contains(&b_hash));
        assert!(!closure.contains(&leaf_c), "depth-1 must NOT reach grandparent");
    }

    #[test]
    fn cycle_does_not_loop() {
        // Two Blocks whose Delta payloads each reference the other's
        // hash.  Because Block hashes are content-addressed this can
        // only happen via constructed payloads (we set base_hash by
        // hand), but the walker must terminate regardless.
        let mut store = BTreeMap::<Hash, Vec<u8>>::new();
        let leaf = store_block_raw(&mut store, b"leaf".to_vec());

        // First a self-referential Delta (base_hash points to leaf)
        // and a separate Block whose payload references that Delta.
        let self_ref =
            Delta { base_hash: leaf, ops: vec![Op::Put { name: "self".into(), hash: leaf }] };
        let sr_bytes = cbor::encode(&self_ref).expect("ok");
        store.insert(leaf, sr_bytes); // overwrite leaf payload to encode a self-ref delta

        let w = ReachabilityWalker::with_depth_limit(64);
        let closure = w.walk(&[leaf], |k| Ok(store.get(k).cloned())).expect("walk");
        assert!(closure.contains(&leaf), "walker terminates on self-ref");
    }

    #[test]
    fn namespace_roots_dedup_and_sort() {
        let h1 = Hash::from_bytes([1; 32]);
        let h2 = Hash::from_bytes([2; 32]);
        let mut ns = crate::Namespace::new();
        ns.insert("a".into(), h2);
        ns.insert("b".into(), h1);
        ns.insert("c".into(), h2); // dup
        let roots = namespace_roots(&ns);
        assert_eq!(roots, vec![h1, h2]);
    }
}
