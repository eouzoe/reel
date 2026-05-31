//! Fake effect implementations for the walking-skeleton.
//!
//! `Sealed` is `pub(crate)` so external crates cannot `impl ClassA / B / C`
//! before (`#[reel::class_a]` proc-macro) lands.  The walking-skeleton
//! example needs concrete Class A / B / C effect values to exercise the
//! `fork → commit → abort` lifecycle end-to-end against a fake adapter.
//! This module supplies three throwaway types that satisfy the trait bounds
//! by living inside the `reel-effects` crate.
//!
//! Per's calibration mandate, these types are intentionally minimal
//! — they exist to surface spec gaps when the walking-skeleton tries to
//! honour the spec, not to model real production adapters.  Real adapters
//! land in (fs Class A)) on top of the
//! `#[reel::class_*]` macros from.
//!
//! # Feature gate
//!
//! Behind the `walking-skeleton` feature.  Default builds do not compile
//! this module.

use crate::class_a::ClassA;
use crate::class_b::{ClassB, PreconditionCtx};
use crate::class_c::ClassC;
use crate::fire_capability::FireCapability;
use crate::receipt::EffectReceipt;
use crate::sealed::Sealed;
use reel_spec::{Hash, Op, ReelError, VersionRef};
use std::future::{Future, ready};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// Tag for an event recorded by the [`FakeAdapterLog`].
///
/// Used by the walking-skeleton example to assert I-002 on the abort path:
/// the log MUST NOT contain a `ClassCFired` entry corresponding to an
/// aborted View.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeAdapterEvent {
    /// A Class A `apply` was called; payload = Delta Op produced.
    ClassAApplied {
        /// Human-readable label, e.g. `fs.write("/tmp/reel-demo/a.txt")`.
        label: String,
    },
    /// A Class B `fire` completed against the fake adapter.
    ClassBFired {
        /// Human-readable label, e.g. `slack.update_message(VersionRef::Opaque("v1"))`.
        label: String,
        /// [`VersionRef`] the Class B effect carried (used for precondition).
        version_ref: VersionRef,
    },
    /// A Class C `fire` completed against the fake adapter.
    ///
    /// I-002 invariant: the abort-path example MUST NOT produce this event
    /// for an aborted View.
    ClassCFired {
        /// Human-readable label, e.g. `slack.post("deploy done")`.
        label: String,
    },
}

/// Shared observability sink for the walking-skeleton fake adapter.
///
/// Wrapped in `Arc<Mutex<…>>` so the example main + every fired effect can
/// append events from the same logical adapter.
#[derive(Clone, Debug, Default)]
pub struct FakeAdapterLog {
    inner: Arc<Mutex<Vec<FakeAdapterEvent>>>,
}

impl FakeAdapterLog {
    /// Construct a fresh, empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an event; on poisoned mutex, recover the inner data and append
    /// anyway — the walking-skeleton example must never crash on a
    /// poisoning event (which only happens on a panicking effect, an
    /// observable test failure in its own right).
    pub fn append(&self, ev: FakeAdapterEvent) {
        match self.inner.lock() {
            Ok(mut guard) => guard.push(ev),
            Err(poisoned) => poisoned.into_inner().push(ev),
        }
    }

    /// Snapshot the events for assertion.
    #[must_use]
    pub fn snapshot(&self) -> Vec<FakeAdapterEvent> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Returns true iff the log contains any `ClassCFired` event.
    ///
    /// The walking-skeleton-abort example asserts this returns `false`
    /// after an aborted View — that is the runtime witness of I-002.
    #[must_use]
    pub fn has_class_c_fired(&self) -> bool {
        self.snapshot().iter().any(|e| matches!(e, FakeAdapterEvent::ClassCFired { .. }))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// FakeFsWrite — Class A (pure local)
// ─────────────────────────────────────────────────────────────────────────

/// Walking-skeleton Class A effect: "write a file".
///
/// Purely local, recorded in the View's Delta.  The actual filesystem write
/// happens at commit time by example code (NOT by the effect — Class A may
/// only act on the Delta; per `effect-class-rules.md` §1 the file write is
/// the adapter's commit-time materialisation of the Delta).
///
/// The walking-skeleton example records the *intent* via a `Op::Put` whose
/// `name` is the file path and `hash` is BLAKE3 of the file contents.  At
/// commit time the example walks the Delta and materialises the files.
#[derive(Debug)]
pub struct FakeFsWrite {
    path: PathBuf,
    content_hash: Hash,
    log: FakeAdapterLog,
}

impl FakeFsWrite {
    /// Construct a fake fs-write effect.
    ///
    /// The walking-skeleton hashes the file content separately and passes
    /// the digest here.  Storage will write the content Block before
    /// `commit` so that I-001 reachability holds after the Ref move.
    #[must_use]
    pub const fn new(path: PathBuf, content_hash: Hash, log: FakeAdapterLog) -> Self {
        Self { path, content_hash, log }
    }
}

impl Sealed for FakeFsWrite {}

impl ClassA for FakeFsWrite {
    fn apply(&self) -> Op {
        self.log.append(FakeAdapterEvent::ClassAApplied {
            label: format!("fs.write({})", self.path.display()),
        });
        Op::Put { name: self.path.display().to_string(), hash: self.content_hash }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// FakeSlackUpdate — Class B (idempotent remote, version_ref'd)
// ─────────────────────────────────────────────────────────────────────────

/// Walking-skeleton Class B effect: "update Slack message".
///
/// Idempotent under stable identifier (the `VersionRef`).  Fires at commit;
/// fails at precondition check if the version reference no longer matches
/// (the walking-skeleton always permits the precondition for simplicity).
#[derive(Debug)]
pub struct FakeSlackUpdate {
    label: String,
    version_ref: VersionRef,
    log: FakeAdapterLog,
}

impl FakeSlackUpdate {
    /// Construct a fake Slack-update Class B effect.
    #[must_use]
    pub const fn new(label: String, version_ref: VersionRef, log: FakeAdapterLog) -> Self {
        Self { label, version_ref, log }
    }
}

impl Sealed for FakeSlackUpdate {}

impl ClassB for FakeSlackUpdate {
    fn version_ref(&self) -> VersionRef {
        self.version_ref.clone()
    }

    fn assert_precondition<'a>(
        &'a self,
        _ctx: &'a PreconditionCtx,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReelError>> + 'a>> {
        // Walking-skeleton always says "precondition holds".  Real Slack
        // adapter would check the message ETag against api.slack.com.
        Box::pin(ready(Ok(())))
    }

    fn fire<'a>(
        self,
        _cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a,
    {
        let label = self.label;
        let vr = self.version_ref;
        let log = self.log;
        Box::pin(async move {
            log.append(FakeAdapterEvent::ClassBFired {
                label: format!("slack.update({label})"),
                version_ref: vr.clone(),
            });
            Ok(EffectReceipt {
                effect_id: 0,
                version_ref_after: Some(vr),
                adapter_diagnostic: serde_json::json!({"fake": true, "class": "B"}),
            })
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// FakeSlackPost — Class C (irreversible remote)
// ─────────────────────────────────────────────────────────────────────────

/// Walking-skeleton Class C effect: "post Slack message" — irreversible
/// once observable.  MUST NOT fire on the abort path (I-002 witness).
#[derive(Debug)]
pub struct FakeSlackPost {
    label: String,
    log: FakeAdapterLog,
}

impl FakeSlackPost {
    /// Construct a fake Slack-post Class C effect.
    #[must_use]
    pub const fn new(label: String, log: FakeAdapterLog) -> Self {
        Self { label, log }
    }
}

impl Sealed for FakeSlackPost {}

impl ClassC for FakeSlackPost {
    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "class": "C",
            "irreversible": true,
            "label": self.label,
        })
    }

    fn fire<'a>(
        self,
        _cap: &'a FireCapability,
    ) -> Pin<Box<dyn Future<Output = Result<EffectReceipt, ReelError>> + 'a>>
    where
        Self: 'a,
    {
        let label = self.label;
        let log = self.log;
        Box::pin(async move {
            log.append(FakeAdapterEvent::ClassCFired { label: format!("slack.post({label})") });
            Ok(EffectReceipt {
                effect_id: 0,
                version_ref_after: None,
                adapter_diagnostic: serde_json::json!({"fake": true, "class": "C"}),
            })
        })
    }
}
