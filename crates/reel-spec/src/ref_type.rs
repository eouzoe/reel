//! Mutable named pointer — the reel `Ref` type.
//!
//! A [`Ref`] maps a stable UTF-8 `name` (the identity) to a [`crate::Hash`]
//! and carries an optional [`crate::VersionRef`] for Class B effect commit
//! gating (see `spec/spec.md` §5.7.2).
//!
//! Per ADR-0025 §5.3 (and the reconciliation note in
//! the design notes §1), the **name** is
//! the identity of a `Ref`; the pre-ADR-0024 `RefId` `UUIDv7` type is
//! removed.  ADR-0011 is superseded by ADR-0025 — retained only for
//! historical comparison of `UUIDv7` vs ULID vs Snowflake vs sequential
//! `u64`.
//!
//! See `spec/spec.md` §5.3 and `spec/schema/2026-Q3/ref.schema.json`.
//!
//! # Invariants
//!
//! - A `Ref` MUST be mutated only through a successful `commit` verb
//!   (spec §6.2).
//! - `name` is a non-empty UTF-8 string of at most [`MAX_REF_NAME_BYTES`]
//!   bytes — enforced at construction time by [`Ref::new`] and
//!   [`Ref::with_version_constraint`].
//!
//! # References
//!
//! - ADR-0025
//! - ADR-0011 (superseded)
//! - Schema: `spec/schema/2026-Q3/ref.schema.json`

use crate::{Hash, ReelError, VersionRef};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Upper bound on the UTF-8 byte length of a [`Ref::name`].
///
/// Matches `maxLength: 4096` in `spec/schema/2026-Q3/ref.schema.json`.
/// The lower bound is one byte (non-empty names only).
pub const MAX_REF_NAME_BYTES: usize = 4096;

/// A mutable named pointer to a [`crate::Block`].
///
/// `Ref` maps a stable UTF-8 `name` (the identity) to the `hash` of the
/// Block currently referenced.
/// An optional [`VersionRef`] carries the precondition used for Class B
/// effect commits at commit time (see `spec/spec.md` §5.7.2).
///
/// `name` serves as the identity of a `Ref`; there is no UUID identity
/// field (the pre-ADR-0025 `RefId` type is removed — see ADR-0011 history).
///
/// `Ref` implements [`Ord`] by `name` to support ordered iteration of the
/// namespace map; the `hash` and `version_constraint` are not part of the
/// ordering key.
///
/// # Construction
///
/// Prefer [`Ref::new`] and [`Ref::with_version_constraint`] over direct
/// struct construction — they validate the name against
/// [`MAX_REF_NAME_BYTES`] and reject empty names.
///
/// # Examples
///
/// ```rust
/// use reel_spec::{Hash, Ref};
///
/// let hash = Hash::from_bytes([0u8; 32]);
/// let r = Ref::new("workspace/main", hash).expect("valid name");
/// assert_eq!(r.name(), "workspace/main");
/// assert_eq!(r.hash(), &hash);
/// assert!(r.version_constraint().is_none());
/// ```
///
/// # Note on [`std::hash::Hash`]
///
/// `Ref` intentionally does **not** derive [`std::hash::Hash`]:
/// the accessor [`Ref::hash`] returning `&Hash` would collide with the
/// trait method `std::hash::Hash::hash`, triggering
/// `clippy::same_name_method`. `name` is the identity of a `Ref`
/// (ADR-0025 §5.3); downstream code that needs to hash by identity
/// should hash the result of [`Ref::name`] directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    /// Stable UTF-8 identifier; the identity of this `Ref`.
    ///
    /// Path syntax is reel-defined; minimum length 1 byte, maximum
    /// [`MAX_REF_NAME_BYTES`] (4096) per
    /// `spec/schema/2026-Q3/ref.schema.json`.
    pub name: String,

    /// The [`crate::Block`] currently referenced by this `Ref`.
    pub hash: Hash,

    /// Optional precondition for Class B commit gating.
    ///
    /// `None` means this `Ref` has no remote version semantics.
    /// `Some(vr)` is checked at `commit` time; if the precondition fails,
    /// `commit` returns [`crate::ReelError::CommitConflict`] (E-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version_constraint: Option<VersionRef>,
}

impl Ref {
    /// Constructs a `Ref` with no Class B precondition.
    ///
    /// `version_constraint` is set to `None`.
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] when `name` is empty or longer than
    /// [`MAX_REF_NAME_BYTES`] (4096) bytes (matches the constraints in
    /// `spec/schema/2026-Q3/ref.schema.json`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Hash, Ref};
    ///
    /// let hash = Hash::from_bytes([0u8; 32]);
    /// let r = Ref::new("workspace/main", hash).expect("valid name");
    /// assert_eq!(r.name(), "workspace/main");
    /// assert!(r.version_constraint().is_none());
    /// ```
    pub fn new(name: impl Into<String>, hash: Hash) -> Result<Self, ReelError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self { name, hash, version_constraint: None })
    }

    /// Constructs a `Ref` carrying a Class B precondition.
    ///
    /// The `version_ref` is recorded for the commit-time precondition
    /// check (see `spec/spec.md` §5.7.2).
    ///
    /// # Errors
    ///
    /// Returns [`ReelError::Storage`] when `name` is empty or longer than
    /// [`MAX_REF_NAME_BYTES`] bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Hash, Ref, VersionKind, VersionRef};
    ///
    /// let hash = Hash::from_bytes([1u8; 32]);
    /// let vr = VersionRef { kind: VersionKind::Etag, value: "\"abc\"".into() };
    /// let r = Ref::with_version_constraint("api/posts", hash, vr.clone())
    ///     .expect("valid name");
    /// assert_eq!(r.version_constraint(), Some(&vr));
    /// ```
    pub fn with_version_constraint(
        name: impl Into<String>,
        hash: Hash,
        version_ref: VersionRef,
    ) -> Result<Self, ReelError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self { name, hash, version_constraint: Some(version_ref) })
    }

    /// Returns the `Ref`'s name (its identity).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Hash, Ref};
    ///
    /// let r = Ref::new("main", Hash::from_bytes([0u8; 32])).expect("ok");
    /// assert_eq!(r.name(), "main");
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the [`struct@Hash`] of the [`crate::Block`] currently referenced.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Hash, Ref};
    ///
    /// let h = Hash::from_bytes([7u8; 32]);
    /// let r = Ref::new("main", h).expect("ok");
    /// assert_eq!(r.hash(), &h);
    /// ```
    #[must_use]
    pub const fn hash(&self) -> &Hash {
        &self.hash
    }

    /// Returns the optional Class B `VersionRef` precondition.
    ///
    /// Returns `None` for `Ref`s without remote version semantics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::{Hash, Ref};
    ///
    /// let r = Ref::new("main", Hash::from_bytes([0u8; 32])).expect("ok");
    /// assert!(r.version_constraint().is_none());
    /// ```
    #[must_use]
    pub const fn version_constraint(&self) -> Option<&VersionRef> {
        self.version_constraint.as_ref()
    }
}

/// Validates a candidate [`Ref`] name against the schema.
///
/// Enforces the `minLength: 1` / `maxLength: 4096` constraints from
/// `spec/schema/2026-Q3/ref.schema.json`.
///
/// # Errors
///
/// Returns [`ReelError::Storage`] when `name` is empty or its UTF-8 byte
/// length exceeds [`MAX_REF_NAME_BYTES`].
fn validate_name(name: &str) -> Result<(), ReelError> {
    if name.is_empty() {
        return Err(ReelError::Storage(
            "Ref name must be non-empty (schema minLength: 1)".to_owned(),
        ));
    }
    if name.len() > MAX_REF_NAME_BYTES {
        return Err(ReelError::Storage(format!(
            "Ref name exceeds MAX_REF_NAME_BYTES ({} > {} bytes)",
            name.len(),
            MAX_REF_NAME_BYTES
        )));
    }
    Ok(())
}

// `Ord` / `PartialOrd` keyed on `name`.
//
// Per ADR-0025 §5.3, `name` is the identity of a `Ref`.  Ordering by
// `name` supports ordered iteration of the namespace map and a stable
// log layout in storage layers.  The `hash` and `version_constraint`
// are not part of the ordering key.

impl Ord for Ref {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl PartialOrd for Ref {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
    use crate::{VersionKind, VersionRef};
    use std::error::Error;

    // ── Helpers ────────────────────────────────────────────────────────────

    fn fixed_hash() -> Hash {
        Hash::from_bytes([0xAB; 32])
    }

    fn fixed_version_ref() -> VersionRef {
        VersionRef { kind: VersionKind::Etag, value: "\"abc123\"".into() }
    }

    // ── Ref::new — name validation ─────────────────────────────────────────

    #[test]
    fn new_accepts_simple_name() -> Result<(), ReelError> {
        let r = Ref::new("main", fixed_hash())?;
        assert_eq!(r.name(), "main");
        assert_eq!(r.hash(), &fixed_hash());
        assert!(r.version_constraint().is_none());
        Ok(())
    }

    #[test]
    fn new_accepts_path_style_name() -> Result<(), ReelError> {
        let r = Ref::new("workspace/feature/branch", fixed_hash())?;
        assert_eq!(r.name(), "workspace/feature/branch");
        Ok(())
    }

    #[test]
    fn new_accepts_utf8_multibyte() -> Result<(), ReelError> {
        // Greek + CJK + emoji — multi-byte UTF-8 is valid per the schema
        // ("UTF-8 string" with no character-class restriction).
        let r = Ref::new("αβγ/测试/🎬", fixed_hash())?;
        assert_eq!(r.name(), "αβγ/测试/🎬");
        Ok(())
    }

    #[test]
    fn new_rejects_empty_name() {
        let err = Ref::new("", fixed_hash()).expect_err("empty must be rejected");
        match err {
            ReelError::Storage(msg) => {
                assert!(msg.contains("non-empty"), "msg must mention non-empty, got: {msg}");
            }
            // ReelError is #[non_exhaustive]; match remaining known variants
            // explicitly to avoid `clippy::wildcard_enum_match_arm` while
            // still flagging unexpected types.
            other @ (ReelError::BlockNotFound(_)
            | ReelError::CommitConflict(_)
            | ReelError::ClassCAfterAbort
            | ReelError::ForkDepthExceeded(_)
            | ReelError::EffectDrainTimeout
            | ReelError::ViewTerminal(_)
            | ReelError::CapabilityViolation(_)
            | ReelError::ReachabilityViolation(_)) => {
                panic!("expected ReelError::Storage, got {other:?}");
            }
        }
    }

    #[test]
    fn new_accepts_name_at_max_length() -> Result<(), ReelError> {
        // ASCII => 1 byte per char => exactly MAX_REF_NAME_BYTES bytes.
        let name = "a".repeat(MAX_REF_NAME_BYTES);
        assert_eq!(name.len(), MAX_REF_NAME_BYTES);
        let r = Ref::new(name, fixed_hash())?;
        assert_eq!(r.name().len(), MAX_REF_NAME_BYTES);
        Ok(())
    }

    #[test]
    fn new_rejects_name_over_max_length() {
        let name = "a".repeat(MAX_REF_NAME_BYTES + 1);
        let err = Ref::new(name, fixed_hash()).expect_err("oversize must be rejected");
        match err {
            ReelError::Storage(msg) => {
                assert!(
                    msg.contains("MAX_REF_NAME_BYTES"),
                    "msg must mention MAX_REF_NAME_BYTES, got: {msg}"
                );
            }
            // Same explicit-variant pattern as new_rejects_empty_name above.
            other @ (ReelError::BlockNotFound(_)
            | ReelError::CommitConflict(_)
            | ReelError::ClassCAfterAbort
            | ReelError::ForkDepthExceeded(_)
            | ReelError::EffectDrainTimeout
            | ReelError::ViewTerminal(_)
            | ReelError::CapabilityViolation(_)
            | ReelError::ReachabilityViolation(_)) => {
                panic!("expected ReelError::Storage, got {other:?}");
            }
        }
    }

    #[test]
    fn new_max_length_uses_byte_count_not_char_count() -> Result<(), ReelError> {
        // 4-byte UTF-8 character (emoji); MAX_REF_NAME_BYTES / 4 of them
        // fit exactly inside the byte budget.
        let glyph = "🎬"; // 4 bytes
        assert_eq!(glyph.len(), 4);
        let name = glyph.repeat(MAX_REF_NAME_BYTES / 4);
        assert_eq!(name.len(), MAX_REF_NAME_BYTES);
        Ref::new(name, fixed_hash())?;

        // One extra emoji pushes us 4 bytes over the limit.
        let oversize = glyph.repeat((MAX_REF_NAME_BYTES / 4) + 1);
        assert!(oversize.len() > MAX_REF_NAME_BYTES);
        let err = Ref::new(oversize, fixed_hash()).expect_err("oversize must reject");
        assert!(matches!(err, ReelError::Storage(_)));
        Ok(())
    }

    // ── Ref::with_version_constraint ───────────────────────────────────────

    #[test]
    fn with_version_constraint_stores_version_ref() -> Result<(), ReelError> {
        let vr = fixed_version_ref();
        let r = Ref::with_version_constraint("api/items", fixed_hash(), vr.clone())?;
        assert_eq!(r.name(), "api/items");
        assert_eq!(r.version_constraint(), Some(&vr));
        Ok(())
    }

    #[test]
    fn with_version_constraint_validates_name() {
        let vr = fixed_version_ref();
        let err = Ref::with_version_constraint("", fixed_hash(), vr)
            .expect_err("empty name must be rejected here too");
        assert!(matches!(err, ReelError::Storage(_)));
    }

    // ── Accessors ──────────────────────────────────────────────────────────

    /// Tripwire: asserts that the accessor methods return references to the
    /// underlying fields rather than computed values. The body looks like a
    /// tautology because the current accessor implementations are
    /// `&self.<field>`; that is the invariant. If a future edit changes an
    /// accessor to derive its value (clone, transform, default-on-missing),
    /// this test starts comparing two distinct addresses and fails — forcing
    /// the change to be acknowledged in code review.
    #[test]
    fn accessors_match_fields() -> Result<(), ReelError> {
        let vr = fixed_version_ref();
        let r = Ref::with_version_constraint("k", fixed_hash(), vr.clone())?;
        assert_eq!(r.name(), &r.name);
        assert_eq!(r.hash(), &r.hash);
        assert_eq!(r.version_constraint(), Some(&vr));
        Ok(())
    }

    // ── Ord — sorted by name ───────────────────────────────────────────────

    #[test]
    fn ord_sorts_by_name_ascending() -> Result<(), ReelError> {
        // Ten Refs with names "00".."09".  Shuffle and sort; expect alpha
        // order regardless of the differing hashes.
        let mut refs: Vec<Ref> = (0..10)
            .map(|i| Ref::new(format!("{i:02}"), Hash::from_bytes([i; 32])))
            .collect::<Result<_, _>>()?;
        refs.reverse(); // worst-case ordering
        refs.sort();
        for (i, r) in refs.iter().enumerate() {
            assert_eq!(r.name, format!("{i:02}"), "sort by name");
        }
        Ok(())
    }

    #[test]
    fn ord_ignores_hash_and_version_constraint() -> Result<(), ReelError> {
        // Same name, different hash + version_constraint => Ordering::Equal.
        let h1 = Hash::from_bytes([1u8; 32]);
        let h2 = Hash::from_bytes([2u8; 32]);
        let a = Ref::new("same", h1)?;
        let b = Ref::with_version_constraint("same", h2, fixed_version_ref())?;
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(b.cmp(&a), Ordering::Equal);
        Ok(())
    }

    #[test]
    fn ord_total_ordering_holds() -> Result<(), ReelError> {
        let a = Ref::new("alpha", fixed_hash())?;
        let b = Ref::new("beta", fixed_hash())?;
        let c = Ref::new("gamma", fixed_hash())?;
        assert!(a < b);
        assert!(b < c);
        assert!(a < c, "transitivity");
        Ok(())
    }

    // ── serde JSON round-trip ──────────────────────────────────────────────

    #[test]
    fn serde_json_round_trip_no_version() -> Result<(), Box<dyn Error>> {
        let r = Ref::new("workspace/main", fixed_hash())?;
        let json = serde_json::to_string(&r)?;
        // `version_constraint: None` MUST be skipped per skip_serializing_if.
        assert!(
            !json.contains("version_constraint"),
            "version_constraint absent when None, got: {json}"
        );
        let r2: Ref = serde_json::from_str(&json)?;
        assert_eq!(r, r2);
        Ok(())
    }

    #[test]
    fn serde_json_round_trip_with_version() -> Result<(), Box<dyn Error>> {
        let r = Ref::with_version_constraint(
            "workspace/feature",
            Hash::from_bytes([2u8; 32]),
            fixed_version_ref(),
        )?;
        let json = serde_json::to_string(&r)?;
        assert!(json.contains("version_constraint"), "field must be present");
        let r2: Ref = serde_json::from_str(&json)?;
        assert_eq!(r, r2, "JSON round-trip preserves all three fields");
        assert_eq!(r.name(), r2.name());
        assert_eq!(r.hash(), r2.hash());
        assert_eq!(r.version_constraint(), r2.version_constraint());
        Ok(())
    }

    #[test]
    fn serde_json_round_trip_each_version_kind() -> Result<(), Box<dyn Error>> {
        // Every VersionKind survives a JSON round-trip on a Ref.
        for kind in [
            VersionKind::Etag,
            VersionKind::CommitSha,
            VersionKind::ObjectVersionId,
            VersionKind::Opaque,
        ] {
            let vr = VersionRef { kind, value: "v1".into() };
            let r = Ref::with_version_constraint("k", fixed_hash(), vr.clone())?;
            let json = serde_json::to_string(&r)?;
            let r2: Ref = serde_json::from_str(&json)?;
            assert_eq!(r.version_constraint(), Some(&vr));
            assert_eq!(r, r2, "round-trip for VersionKind::{kind:?}");
        }
        Ok(())
    }

    // ── No UUID identity ───────────────────────────────────────────────────

    #[test]
    fn no_uuid_in_json_representation() -> Result<(), Box<dyn Error>> {
        // Per ADR-0025: `Ref` has no UUID identity field.  Confirm the
        // serialised form has no `id` / `uuid` keys.
        let r = Ref::new("main", fixed_hash())?;
        let json = serde_json::to_string(&r)?;
        let lower = json.to_lowercase();
        assert!(!lower.contains("\"id\""), "no `id` field expected: {json}");
        assert!(!lower.contains("uuid"), "no `uuid` substring expected: {json}");
        Ok(())
    }

    #[test]
    fn json_keys_match_schema() -> Result<(), Box<dyn Error>> {
        // `spec/schema/2026-Q3/ref.schema.json` declares exactly
        // `name`, `hash`, and `version_constraint`.  Verify the JSON
        // surface matches.
        let r = Ref::with_version_constraint("k", fixed_hash(), fixed_version_ref())?;
        let val: serde_json::Value = serde_json::to_value(&r)?;
        let obj = val.as_object().expect("Ref serialises as a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["hash", "name", "version_constraint"]);
        Ok(())
    }

    // ── Property test: JSON round-trip preserves all three fields ──────────
    //
    // For any (name, hash bytes, optional version_ref) generated within
    // schema bounds, `serde_json::to_string` followed by `from_str` must
    // return a `Ref` equal to the input.

    mod property {
        use super::*;
        use proptest::array::uniform32;
        use proptest::option::of as opt_of;
        use proptest::prelude::*;

        fn version_kind_strategy() -> impl Strategy<Value = VersionKind> {
            prop_oneof![
                Just(VersionKind::Etag),
                Just(VersionKind::CommitSha),
                Just(VersionKind::ObjectVersionId),
                Just(VersionKind::Opaque),
            ]
        }

        fn version_ref_strategy() -> impl Strategy<Value = VersionRef> {
            (version_kind_strategy(), "[A-Za-z0-9/\"._-]{1,64}")
                .prop_map(|(kind, value)| VersionRef { kind, value })
        }

        proptest! {
            #[test]
            fn json_round_trip_preserves_fields(
                // 1..=256 valid UTF-8 chars from a printable alphabet;
                // schema permits up to 4096 bytes but 256 keeps the
                // property test fast.
                name in "[A-Za-z0-9_/-]{1,256}",
                bytes in uniform32(any::<u8>()),
                vr_opt in opt_of(version_ref_strategy()),
            ) {
                let hash = Hash::from_bytes(bytes);
                let original = vr_opt.as_ref().map_or_else(
                    || Ref::new(name.as_str(), hash),
                    |vr| Ref::with_version_constraint(name.as_str(), hash, vr.clone()),
                )
                .expect("generated name is within bounds");

                let json = serde_json::to_string(&original)
                    .expect("Ref serialises to JSON");
                let recovered: Ref = serde_json::from_str(&json)
                    .expect("Ref deserialises from JSON");

                prop_assert_eq!(&original.name, &recovered.name);
                prop_assert_eq!(&original.hash, &recovered.hash);
                prop_assert_eq!(&original.version_constraint, &recovered.version_constraint);
                prop_assert_eq!(original, recovered);
            }
        }
    }
}
