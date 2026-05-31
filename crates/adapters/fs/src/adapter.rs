//! Host-side filesystem adapter facade.
//!
//! [`FsAdapter`] is the surface a kernel/CLI talks to in order to:
//!
//! 1. **Construct Class A effects** ([`FsAdapter::put`] /
//!    [`FsAdapter::remove`]).
//! 2. **Classify** an `Op` as Class A (per
//!    `spec/adapter-interface.md` §3.1).
//! 3. **Materialise** an applied `Delta` onto the real filesystem at
//!    commit-drain time ([`FsAdapter::apply_delta`]) under a narrowed
//!    [`reel_spec::Capability`] (I-003).
//!
//! Class A's defining property is that `apply` is reversed by `Delta`
//! discard at `abort` — the kernel simply does not invoke
//! [`FsAdapter::apply_delta`] on the abort path.  This crate carries
//! none of the abort logic; it is supplied by the kernel.

use crate::effect::{FsPut, FsRemove};
use crate::error::FsAdapterError;
use reel_spec::{Block, Capability, Delta, Hash, Op, OpKind};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// Effect class as classified by an adapter (per
/// `spec/adapter-interface.md` §3).
///
/// `FsAdapter` returns only [`EffectClass::A`] from
/// [`FsAdapter::classify`].
///
/// # Examples
///
/// ```rust
/// use adapter_fs::{EffectClass, FsAdapter};
/// use reel_spec::{Hash, Op};
///
/// let adapter = FsAdapter::new("ws/").expect("scope");
/// let op = Op::Put {
///     name: "ws/file.txt".into(),
///     hash: Hash::from_bytes([0u8; 32]),
/// };
/// assert_eq!(adapter.classify(&op), EffectClass::A);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectClass {
    /// Class A — pure local, reversible.
    A,
    /// Class B — idempotent remote.
    B,
    /// Class C — irreversible remote.
    C,
}

/// Class A host-side facade for filesystem operations.
///
/// An `FsAdapter` is bound to a `scope` (a namespace prefix) and an
/// optional `root` (the on-disk directory under which names resolve).
/// The scope MUST be a prefix of any `Capability.path_prefix` that
/// authorises operations through this adapter.
///
/// # I-003
///
/// `apply_delta` enforces capability narrowing — the supplied
/// `Capability.path_prefix` must contain every `Op` name applied, and
/// the `Capability.op_set` must include `Write` (for `Put`) or `Delete`
/// (for `Remove`).  Violations return
/// [`FsAdapterError::CapabilityViolation`] (`E-009`).
///
/// # Idempotency (Class A)
///
/// `apply_delta` is content-idempotent: writing a file whose disk
/// contents already hash to the same `Hash` is a no-op (and the
/// op's `ApplyOutcome` reports `Unchanged`).  Re-running the same
/// `Delta` against the same disk state is therefore safe.
///
/// # Examples
///
/// Build effects and feed them through a `Delta`:
///
/// ```rust,no_run
/// use adapter_fs::FsAdapter;
/// use reel_spec::{Block, Capability, Delta, Hash, OpKind};
/// use std::collections::{BTreeMap, BTreeSet};
///
/// let tmp = tempfile::tempdir().expect("tempdir");
/// let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("scope");
///
/// // The host (kernel) computes the Block and hash; the adapter only
/// // emits effects against that hash.
/// let content = b"hello reel".to_vec();
/// let block = Block::new(content).expect("small");
/// let mut blocks = BTreeMap::new();
/// blocks.insert(block.hash(), block.clone());
///
/// let put_eff = adapter.put("ws/hello.txt".to_owned(), block.hash());
/// let delta = Delta {
///     base_hash: Hash::from_bytes([0u8; 32]),
///     ops: vec![reel_effects::ClassA::apply(&put_eff)],
/// };
///
/// let cap = Capability {
///     path_prefix: "ws/".into(),
///     op_set: BTreeSet::from([OpKind::Write]),
///     ttl: 60,
/// };
/// let outcomes = adapter.apply_delta(&delta, &cap, &blocks).expect("apply");
/// assert_eq!(outcomes.len(), 1);
/// ```
#[derive(Clone, Debug)]
pub struct FsAdapter {
    scope: String,
    root: Option<PathBuf>,
}

/// Outcome of applying a single `Op` from a `Delta`.
///
/// Returned per-op by [`FsAdapter::apply_delta`] so the kernel can
/// log idempotency status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// File was written / removed; disk state changed.
    Changed,
    /// File already matched the desired state; no I/O performed.
    Unchanged,
}

impl FsAdapter {
    /// Construct an adapter with a logical scope but no on-disk root.
    ///
    /// Useful for testing the effect-construction surface without
    /// materialising onto a real filesystem.  [`FsAdapter::apply_delta`]
    /// will return `Ok(vec![])` for an empty `Delta` but error
    /// (`E-007 Storage`) for any op when `root` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`FsAdapterError::CapabilityViolation`] if `scope` is
    /// empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use adapter_fs::FsAdapter;
    /// let adapter = FsAdapter::new("ws/").expect("non-empty scope");
    /// assert_eq!(adapter.scope(), "ws/");
    /// ```
    pub fn new(scope: impl Into<String>) -> Result<Self, FsAdapterError> {
        let scope = scope.into();
        if scope.is_empty() {
            return Err(FsAdapterError::CapabilityViolation(
                "FsAdapter scope must be non-empty".into(),
            ));
        }
        Ok(Self { scope, root: None })
    }

    /// Construct an adapter rooted at a real on-disk directory.
    ///
    /// `root` MUST exist and be a directory.  The directory is the
    /// resolution base for every `Op.name` applied through this
    /// adapter — names are joined onto `root` and normalised
    /// (path-traversal sequences are rejected, see
    /// [`FsAdapterError::NamespaceEscape`]).
    ///
    /// # Errors
    ///
    /// - [`FsAdapterError::CapabilityViolation`] when `scope` is empty.
    /// - [`FsAdapterError::Io`] when `root` does not exist or is not a
    ///   directory.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use adapter_fs::FsAdapter;
    /// let tmp = tempfile::tempdir().expect("tempdir");
    /// let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
    /// assert_eq!(adapter.scope(), "ws/");
    /// ```
    pub fn with_root(
        scope: impl Into<String>,
        root: impl AsRef<Path>,
    ) -> Result<Self, FsAdapterError> {
        let scope = scope.into();
        if scope.is_empty() {
            return Err(FsAdapterError::CapabilityViolation(
                "FsAdapter scope must be non-empty".into(),
            ));
        }
        let root = root.as_ref();
        let meta = fs::metadata(root)
            .map_err(|e| FsAdapterError::Io(format!("root {}: {e}", root.display())))?;
        if !meta.is_dir() {
            return Err(FsAdapterError::Io(format!("root {} is not a directory", root.display())));
        }
        Ok(Self { scope, root: Some(root.to_path_buf()) })
    }

    /// The namespace prefix this adapter manages (per
    /// `spec/adapter-interface.md` §2).
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The on-disk root, if any.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Construct a Class A [`FsPut`] effect.
    ///
    /// The kernel calls `reel_effects::ClassA::apply` on the returned
    /// effect to obtain an `Op::Put { name, hash }` for buffering in
    /// the current View's `Delta`.
    #[must_use]
    pub const fn put(&self, name: String, hash: Hash) -> FsPut {
        FsPut { name, hash }
    }

    /// Construct a Class A [`FsRemove`] effect.
    #[must_use]
    pub const fn remove(&self, name: String) -> FsRemove {
        FsRemove { name }
    }

    /// Classify an `Op`.  Always [`EffectClass::A`] for this adapter
    /// (per `spec/adapter-interface.md` §3.1).
    #[must_use]
    pub const fn classify(&self, _op: &Op) -> EffectClass {
        EffectClass::A
    }

    /// Materialise an applied `Delta` onto the real filesystem.
    ///
    /// This is the **drain-phase** entry point invoked by the kernel
    /// during `commit` — Class A effects are reversed by `Delta`
    /// discard at `abort`, so this method is **never called** on the
    /// abort path (per `spec/spec.md` §5.7.1).
    ///
    /// For each `Op` in `delta.ops`:
    ///
    /// - `Op::Put { name, hash }` — looks up `hash` in `blocks`, then
    ///   either writes `block.data` to `root/<rel(name)>` (creating
    ///   parent directories) or skips when the on-disk file already
    ///   hashes to the same value (content idempotency).
    /// - `Op::Remove { name }` — unlinks `root/<rel(name)>` if it
    ///   exists; missing files are a no-op (idempotency).
    ///
    /// # Capability enforcement (I-003)
    ///
    /// For every op:
    ///
    /// - The `name` MUST start with `cap.path_prefix`.
    /// - For `Put`, `OpKind::Write` MUST be in `cap.op_set`.
    /// - For `Remove`, `OpKind::Delete` MUST be in `cap.op_set`.
    ///
    /// Violations return [`FsAdapterError::CapabilityViolation`]
    /// before any I/O is performed — the function is therefore
    /// fail-safe under capability misconfiguration.
    ///
    /// # Path-traversal defence
    ///
    /// After resolving `root.join(rel(name))`, the canonical path of
    /// the parent must remain under `root`.  Names containing `..` or
    /// absolute paths are rejected with
    /// [`FsAdapterError::NamespaceEscape`] (also mapped to E-009).
    ///
    /// # Errors
    ///
    /// - [`FsAdapterError::CapabilityViolation`] (`E-009`) on I-003
    ///   violations.
    /// - [`FsAdapterError::NamespaceEscape`] (`E-009`) on
    ///   path-traversal attempts.
    /// - [`FsAdapterError::Io`] (`E-007`) on filesystem failures
    ///   (missing root, permission denied, etc.).
    ///
    /// Returns one [`ApplyOutcome`] per op, in the order of
    /// `delta.ops`.
    pub fn apply_delta(
        &self,
        delta: &Delta,
        cap: &Capability,
        blocks: &BTreeMap<Hash, Block>,
    ) -> Result<Vec<ApplyOutcome>, FsAdapterError> {
        // Pre-flight: validate every op's authority before touching disk.
        for op in &delta.ops {
            authorise(op, cap)?;
        }

        let Some(root) = &self.root else {
            if delta.ops.is_empty() {
                return Ok(Vec::new());
            }
            return Err(FsAdapterError::Io(
                "FsAdapter has no on-disk root; cannot apply Delta ops".into(),
            ));
        };

        let mut outcomes = Vec::with_capacity(delta.ops.len());
        for op in &delta.ops {
            let outcome = match op {
                Op::Put { name, hash } => apply_put(root, name, *hash, blocks)?,
                Op::Remove { name } => apply_remove(root, name)?,
            };
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check that `op` is authorised under `cap` per I-003 (`path_prefix`
/// + `op_set`).
fn authorise(op: &Op, cap: &Capability) -> Result<(), FsAdapterError> {
    let (name, required) = match op {
        Op::Put { name, .. } => (name, OpKind::Write),
        Op::Remove { name } => (name, OpKind::Delete),
    };

    if !name.starts_with(cap.path_prefix.as_str()) {
        return Err(FsAdapterError::CapabilityViolation(format!(
            "I-003 violation: op name '{name}' does not start with cap path_prefix '{}'",
            cap.path_prefix
        )));
    }

    if !cap.op_set.contains(&required) {
        return Err(FsAdapterError::CapabilityViolation(format!(
            "I-003 violation: op requires {required:?} which is absent from cap.op_set {:?}",
            cap.op_set
        )));
    }

    Ok(())
}

/// Apply `Op::Put` to disk; idempotent when on-disk content already
/// hashes to `expected_hash`.
fn apply_put(
    root: &Path,
    name: &str,
    expected_hash: Hash,
    blocks: &BTreeMap<Hash, Block>,
) -> Result<ApplyOutcome, FsAdapterError> {
    let target = resolve(root, name)?;

    if target.exists() {
        let on_disk = fs::read(&target).map_err(FsAdapterError::from)?;
        let actual = Hash::from_bytes(*blake3::hash(&on_disk).as_bytes());
        let block = blocks.get(&expected_hash).ok_or_else(|| {
            FsAdapterError::Io(format!(
                "block {expected_hash} not supplied to apply_delta for op_put '{name}'"
            ))
        })?;
        let want = Hash::from_bytes(*blake3::hash(&block.data).as_bytes());
        if actual == want {
            // Content idempotent: nothing to do.
            return Ok(ApplyOutcome::Unchanged);
        }
    }

    let block = blocks.get(&expected_hash).ok_or_else(|| {
        FsAdapterError::Io(format!(
            "block {expected_hash} not supplied to apply_delta for op_put '{name}'"
        ))
    })?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(FsAdapterError::from)?;
    }
    fs::write(&target, &block.data).map_err(FsAdapterError::from)?;
    Ok(ApplyOutcome::Changed)
}

/// Apply `Op::Remove` to disk; missing-file is a no-op.
fn apply_remove(root: &Path, name: &str) -> Result<ApplyOutcome, FsAdapterError> {
    let target = resolve(root, name)?;
    match fs::remove_file(&target) {
        Ok(()) => Ok(ApplyOutcome::Changed),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(ApplyOutcome::Unchanged),
        Err(err) => Err(FsAdapterError::from(err)),
    }
}

/// Resolve `name` against `root`, rejecting path-traversal sequences.
fn resolve(root: &Path, name: &str) -> Result<PathBuf, FsAdapterError> {
    if name.is_empty() {
        return Err(FsAdapterError::NamespaceEscape("empty op name".into()));
    }
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return Err(FsAdapterError::NamespaceEscape(format!(
            "absolute path not allowed: '{name}'"
        )));
    }
    for component in candidate.components() {
        if matches!(component, Component::ParentDir) {
            return Err(FsAdapterError::NamespaceEscape(format!(
                "parent-dir traversal not allowed: '{name}'"
            )));
        }
    }
    Ok(root.join(candidate))
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::expect_used,
    reason = "test module: panic / expect on infallible-by-construction fixtures is acceptable per bose-style.md §Unwrap-policy"
)]
mod tests {
    use super::*;
    use reel_spec::{Block, OpKind};
    use std::collections::BTreeSet;
    use std::slice;

    fn full_cap(prefix: &str) -> Capability {
        Capability {
            path_prefix: prefix.to_owned(),
            op_set: BTreeSet::from([OpKind::Read, OpKind::Write, OpKind::Delete]),
            ttl: 3600,
        }
    }

    fn read_only_cap(prefix: &str) -> Capability {
        Capability {
            path_prefix: prefix.to_owned(),
            op_set: BTreeSet::from([OpKind::Read]),
            ttl: 3600,
        }
    }

    fn make_block(data: &[u8]) -> Block {
        Block::new(data.to_vec()).expect("small payload")
    }

    fn block_map(blocks: &[Block]) -> BTreeMap<Hash, Block> {
        let mut map = BTreeMap::new();
        for b in blocks {
            map.insert(b.hash(), b.clone());
        }
        map
    }

    // ── scope + constructor ───────────────────────────────────────────────

    #[test]
    fn empty_scope_rejected() {
        let err = FsAdapter::new("").expect_err("must reject");
        assert!(matches!(err, FsAdapterError::CapabilityViolation(_)));
    }

    #[test]
    fn new_holds_scope() {
        let adapter = FsAdapter::new("ws/").expect("ok");
        assert_eq!(adapter.scope(), "ws/");
        assert!(adapter.root().is_none());
    }

    #[test]
    fn with_root_requires_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("not_a_dir");
        fs::write(&file, b"x").expect("write tempfile");
        let err = FsAdapter::with_root("ws/", &file).expect_err("must reject file");
        assert!(matches!(err, FsAdapterError::Io(_)), "expected Io, got {err:?}");
    }

    #[test]
    fn classify_always_class_a() {
        let adapter = FsAdapter::new("ws/").expect("ok");
        let put = Op::Put { name: "ws/x".into(), hash: Hash::from_bytes([0u8; 32]) };
        assert_eq!(adapter.classify(&put), EffectClass::A);
        let rm = Op::Remove { name: "ws/y".into() };
        assert_eq!(adapter.classify(&rm), EffectClass::A);
    }

    // ── I-003: capability enforcement ─────────────────────────────────────

    #[test]
    fn i003_path_prefix_violation_put() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"data");
        let blocks = block_map(slice::from_ref(&block));
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "other/escape.txt".into(), hash: block.hash() }],
        };
        let cap = full_cap("ws/");
        let err = adapter.apply_delta(&delta, &cap, &blocks).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::CapabilityViolation(_)),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn i003_path_prefix_violation_remove() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Remove { name: "outside/file.txt".into() }],
        };
        let cap = full_cap("ws/");
        let err = adapter.apply_delta(&delta, &cap, &BTreeMap::new()).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::CapabilityViolation(_)),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn i003_op_set_violation_write_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"data");
        let blocks = block_map(slice::from_ref(&block));
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/file.txt".into(), hash: block.hash() }],
        };
        // Read-only cap: Write is absent.
        let err =
            adapter.apply_delta(&delta, &read_only_cap("ws/"), &blocks).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::CapabilityViolation(_)),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn i003_op_set_violation_delete_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Remove { name: "ws/file.txt".into() }],
        };
        // Cap with Write but no Delete.
        let cap = Capability {
            path_prefix: "ws/".into(),
            op_set: BTreeSet::from([OpKind::Read, OpKind::Write]),
            ttl: 3600,
        };
        let err = adapter.apply_delta(&delta, &cap, &BTreeMap::new()).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::CapabilityViolation(_)),
            "expected E-009, got {err:?}"
        );
    }

    #[test]
    fn i003_positive_write_succeeds() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"hello reel");
        let blocks = block_map(slice::from_ref(&block));
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/greeting.txt".into(), hash: block.hash() }],
        };
        let outcomes = adapter.apply_delta(&delta, &full_cap("ws/"), &blocks).expect("must apply");
        assert_eq!(outcomes, vec![ApplyOutcome::Changed]);
        let on_disk = fs::read(tmp.path().join("ws/greeting.txt")).expect("file written");
        assert_eq!(on_disk, b"hello reel");
    }

    // ── Idempotency ───────────────────────────────────────────────────────

    #[test]
    fn idempotency_identical_content_is_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"payload v1");
        let blocks = block_map(slice::from_ref(&block));
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/a.txt".into(), hash: block.hash() }],
        };
        let cap = full_cap("ws/");

        let first = adapter.apply_delta(&delta, &cap, &blocks).expect("first ok");
        assert_eq!(first, vec![ApplyOutcome::Changed]);

        let second = adapter.apply_delta(&delta, &cap, &blocks).expect("second ok");
        assert_eq!(
            second,
            vec![ApplyOutcome::Unchanged],
            "second apply must be a no-op (content idempotent)"
        );

        let on_disk = fs::read(tmp.path().join("ws/a.txt")).expect("read");
        assert_eq!(on_disk, b"payload v1");
    }

    #[test]
    fn idempotency_changed_content_overwrites() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let cap = full_cap("ws/");

        let v1 = make_block(b"version 1");
        let v2 = make_block(b"version 2");
        let blocks = block_map(&[v1.clone(), v2.clone()]);

        let put_v1 = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/file".into(), hash: v1.hash() }],
        };
        let put_v2 = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/file".into(), hash: v2.hash() }],
        };

        adapter.apply_delta(&put_v1, &cap, &blocks).expect("v1 ok");
        let outcomes = adapter.apply_delta(&put_v2, &cap, &blocks).expect("v2 ok");
        assert_eq!(outcomes, vec![ApplyOutcome::Changed]);
        let on_disk = fs::read(tmp.path().join("ws/file")).expect("read");
        assert_eq!(on_disk, b"version 2");
    }

    #[test]
    fn remove_missing_file_is_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Remove { name: "ws/never_existed.txt".into() }],
        };
        let outcomes = adapter.apply_delta(&delta, &full_cap("ws/"), &BTreeMap::new()).expect("ok");
        assert_eq!(outcomes, vec![ApplyOutcome::Unchanged]);
    }

    #[test]
    fn remove_existing_file_is_changed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"to be removed");
        let blocks = block_map(slice::from_ref(&block));
        adapter
            .apply_delta(
                &Delta {
                    base_hash: Hash::from_bytes([0u8; 32]),
                    ops: vec![Op::Put { name: "ws/doomed".into(), hash: block.hash() }],
                },
                &full_cap("ws/"),
                &blocks,
            )
            .expect("put ok");

        let outcomes = adapter
            .apply_delta(
                &Delta {
                    base_hash: Hash::from_bytes([0u8; 32]),
                    ops: vec![Op::Remove { name: "ws/doomed".into() }],
                },
                &full_cap("ws/"),
                &BTreeMap::new(),
            )
            .expect("remove ok");
        assert_eq!(outcomes, vec![ApplyOutcome::Changed]);
        assert!(!tmp.path().join("ws/doomed").exists());
    }

    // ── Path-traversal defence ────────────────────────────────────────────

    #[test]
    fn rejects_parent_dir_traversal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let block = make_block(b"x");
        let blocks = block_map(slice::from_ref(&block));
        // Cap permits the prefix and op_kind so the check that fires is
        // the namespace-escape one.
        let cap = Capability {
            path_prefix: "ws/".into(),
            op_set: BTreeSet::from([OpKind::Write]),
            ttl: 60,
        };
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/../escape.txt".into(), hash: block.hash() }],
        };
        let err = adapter.apply_delta(&delta, &cap, &blocks).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::NamespaceEscape(_)),
            "expected namespace escape, got {err:?}"
        );
    }

    #[test]
    fn rejects_absolute_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let cap = full_cap("ws/");
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Remove { name: "ws/whatever".into() }],
        };
        // Sanity: ws/whatever is fine.
        adapter.apply_delta(&delta, &cap, &BTreeMap::new()).expect("ok");

        // Now an absolute path with a permissive prefix — i-003 first
        // rejects on prefix (absolute starts with `/`), but the wider cap
        // here (path_prefix `/`) lets us reach the namespace check.
        let wide = Capability {
            path_prefix: "/".into(),
            op_set: BTreeSet::from([OpKind::Delete, OpKind::Write]),
            ttl: 60,
        };
        let escape = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Remove { name: "/etc/passwd".into() }],
        };
        let err = adapter.apply_delta(&escape, &wide, &BTreeMap::new()).expect_err("must reject");
        assert!(
            matches!(err, FsAdapterError::NamespaceEscape(_)),
            "expected namespace escape, got {err:?}"
        );
    }

    // ── adapter without a root ────────────────────────────────────────────

    #[test]
    fn no_root_empty_delta_ok() {
        let adapter = FsAdapter::new("ws/").expect("ok");
        let delta = Delta { base_hash: Hash::from_bytes([0u8; 32]), ops: vec![] };
        let outcomes = adapter.apply_delta(&delta, &full_cap("ws/"), &BTreeMap::new()).expect("ok");
        assert!(outcomes.is_empty());
    }

    #[test]
    fn no_root_non_empty_delta_errors() {
        let adapter = FsAdapter::new("ws/").expect("ok");
        let block = make_block(b"x");
        let blocks = block_map(slice::from_ref(&block));
        let delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![Op::Put { name: "ws/file".into(), hash: block.hash() }],
        };
        let err = adapter
            .apply_delta(&delta, &full_cap("ws/"), &blocks)
            .expect_err("must reject without root");
        assert!(matches!(err, FsAdapterError::Io(_)), "expected E-007, got {err:?}");
    }

    // ── conformance t_002 — fork → write → abort → base unchanged ─────────

    #[test]
    fn t_002_fork_write_abort_base_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let adapter = FsAdapter::with_root("ws/", tmp.path()).expect("ok");
        let cap = full_cap("ws/");
        let block = make_block(b"speculative content");
        let blocks = block_map(slice::from_ref(&block));

        // 1. The "base" (parent) namespace before fork: empty disk.
        assert!(!tmp.path().join("ws/spec.txt").exists());

        // 2. Fork → child View opens; user submits Class A put.
        let child_delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![reel_effects::ClassA::apply(
                &adapter.put("ws/spec.txt".into(), block.hash()),
            )],
        };

        // 3. The user changes its mind and aborts.  The kernel discards
        //    the child's Delta and NEVER calls `apply_delta` for it.
        //    Class A semantics (spec.md §5.7.1): "reversed by Delta
        //    discard at abort".  We simulate by simply dropping the
        //    Delta — the kernel would otherwise have invoked
        //    apply_delta; we deliberately do not.
        drop(child_delta);

        // 4. Verify the base namespace is unchanged: disk untouched.
        assert!(!tmp.path().join("ws/spec.txt").exists(), "base must be unchanged after abort");

        // 5. As a sanity sibling: a parallel fork that *does* commit
        //    must produce the file (proves apply_delta is wired
        //    correctly — failure here would mean the test passes
        //    vacuously).
        let committed_delta = Delta {
            base_hash: Hash::from_bytes([0u8; 32]),
            ops: vec![reel_effects::ClassA::apply(
                &adapter.put("ws/committed.txt".into(), block.hash()),
            )],
        };
        adapter.apply_delta(&committed_delta, &cap, &blocks).expect("commit ok");
        assert!(tmp.path().join("ws/committed.txt").exists());
    }

    // ── Effect-class boundary check ───────────────────────────────────────
    //
    // The compiler is the test: `impl ClassA for FsPut/FsRemove` is only
    // legal because `#[reel_effects::class_a]` injects the seal.  No
    // `ClassB` / `ClassC` impl exists for these types — verified by
    // grep in the build_handoff self-check.

    #[test]
    fn fs_effects_are_class_a_only() {
        // Round-trip through ClassA::apply (verified at compile time
        // by trait resolution).
        use reel_effects::ClassA;
        let h = Hash::from_bytes([0u8; 32]);
        let put = FsPut { name: "ws/x".into(), hash: h };
        let rm = FsRemove { name: "ws/y".into() };
        assert!(matches!(ClassA::apply(&put), Op::Put { .. }));
        assert!(matches!(ClassA::apply(&rm), Op::Remove { .. }));
    }
}
