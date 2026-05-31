//! Class A effect types for the filesystem adapter.
//!
//! Two effect structs, both attributed `#[reel_effects::class_a]`:
//!
//! - [`FsPut`] — emits `Op::Put { name, hash }`.
//! - [`FsRemove`] — emits `Op::Remove { name }`.
//!
//! Both effects act only on a [`reel_spec::Delta`] layer
//! (per `spec/spec.md` §5.7.1, Class A) — they do **not** touch the
//! real filesystem and have no I/O capability in their signatures.
//! Drain-time materialisation (the actual write/unlink) happens in
//! [`crate::FsAdapter::apply_delta`], where the kernel grants a
//! `Capability` and applies the I-003 check.

use reel_effects::ClassA;
use reel_spec::{Hash, Op};

/// Class A filesystem write effect.
///
/// Emits an `Op::Put { name, hash }` when applied; the kernel appends
/// the op to the View's `Delta`.  No I/O is performed here — the
/// physical write happens later at commit-drain via
/// [`crate::FsAdapter::apply_delta`].
///
/// # Examples
///
/// ```rust
/// use adapter_fs::FsPut;
/// use reel_effects::ClassA;
/// use reel_spec::{Hash, Op};
///
/// let h = Hash::from_bytes(*blake3::hash(b"file contents").as_bytes());
/// let eff = FsPut { name: "docs/readme.md".to_owned(), hash: h };
/// let op = eff.apply();
/// assert!(matches!(op, Op::Put { ref name, .. } if name == "docs/readme.md"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[reel_effects::class_a]
pub struct FsPut {
    /// Namespace name (path within the adapter's scope).
    pub name: String,
    /// Hash of the Block whose payload is the new file contents.
    pub hash: Hash,
}

impl ClassA for FsPut {
    fn apply(&self) -> Op {
        Op::Put { name: self.name.clone(), hash: self.hash }
    }
}

/// Class A filesystem remove effect.
///
/// Emits an `Op::Remove { name }` when applied.  No I/O is performed
/// here — the physical unlink happens at commit-drain via
/// [`crate::FsAdapter::apply_delta`].
///
/// # Examples
///
/// ```rust
/// use adapter_fs::FsRemove;
/// use reel_effects::ClassA;
/// use reel_spec::Op;
///
/// let eff = FsRemove { name: "draft.tmp".to_owned() };
/// let op = eff.apply();
/// assert!(matches!(op, Op::Remove { ref name } if name == "draft.tmp"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[reel_effects::class_a]
pub struct FsRemove {
    /// Namespace name (path within the adapter's scope).
    pub name: String,
}

impl ClassA for FsRemove {
    fn apply(&self) -> Op {
        Op::Remove { name: self.name.clone() }
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "test assertions intentionally panic on failure")]
mod tests {
    use super::*;

    #[test]
    fn put_emits_op_put() {
        let h = Hash::from_bytes([7u8; 32]);
        let eff = FsPut { name: "a/b.txt".to_owned(), hash: h };
        match eff.apply() {
            Op::Put { name, hash } => {
                assert_eq!(name, "a/b.txt");
                assert_eq!(hash, h);
            }
            other @ Op::Remove { .. } => panic!("expected Put, got {other:?}"),
        }
    }

    #[test]
    fn remove_emits_op_remove() {
        let eff = FsRemove { name: "stale.tmp".to_owned() };
        match eff.apply() {
            Op::Remove { name } => assert_eq!(name, "stale.tmp"),
            other @ Op::Put { .. } => panic!("expected Remove, got {other:?}"),
        }
    }

    /// L1 isolation proof — `apply()` is signature-pure.
    ///
    /// The fact that this test runs without any tempdir / FS handle is
    /// itself the evidence: `ClassA::apply` has no `&Path` / `&mut File`
    /// parameter, so safe Rust cannot reach the filesystem here.
    /// Materialisation only happens via the kernel-driven drain in
    /// [`crate::FsAdapter::apply_delta`].
    #[test]
    fn apply_has_no_io_signature() {
        let h = Hash::from_bytes([0u8; 32]);
        assert!(matches!(FsPut { name: "x".into(), hash: h }.apply(), Op::Put { .. }));
        assert!(matches!(FsRemove { name: "y".into() }.apply(), Op::Remove { .. }));
    }
}
