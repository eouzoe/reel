# reel Adapter Interface

> Trait specification for adapter implementations. An adapter wires
> an external resource into reel's protocol surface.

## Status

- Version: v0.1 (2026-05-15).
- Canonical authority: ADR-0025.
- Companion: [spec.md](spec.md), [conformance.md](conformance.md).

---

## 1. Adapter responsibilities

An **adapter** is the code that connects a specific external resource
(filesystem, Slack, GitHub, Postgres, ...) into reel's protocol
surface. The adapter:

1. **Classifies operations** as Class A, B, or C (spec.md §5.7).
2. **Constructs Effect descriptors** for Class B and Class C ops.
3. **Implements `fire`** for each effect class.
4. **Implements `assert_precondition`** for Class B (version-ref
   check at commit).
5. **Defines `compensate`** for Class B and Class C if the adapter
   chooses to support post-commit reversal (optional; Class C
   compensation is best-effort and not protocol-guaranteed).

---

## 2. Core trait

```rust
pub trait Adapter: Send + Sync + 'static {
    /// The resource scope this adapter manages (e.g., "fs:/home/user/proj").
    fn scope(&self) -> &str;

    /// Classify an operation. Adapter author decides A/B/C per
    /// spec.md §5.7 semantics.
    fn classify(&self, op: &Op) -> EffectClass;

    /// Build a Block-serializable Effect descriptor for buffering.
    fn describe(&self, op: &Op) -> EffectDescriptor;

    /// For Class B: produce the version-ref that the commit gate
    /// will check.
    fn version_ref(&self, op: &Op) -> Option<VersionRef>;

    /// For Class B: at commit, assert the version-ref is still
    /// valid. Returns Ok(()) on match, Err on mismatch.
    fn assert_precondition(
        &self,
        op: &Op,
        version: &VersionRef,
    ) -> Result<(), AdapterError>;

    /// Fire the effect. Called during commit's drain phase.
    /// Class A: applies to the Delta layer (must be called by the
    ///          kernel, not the adapter — adapters do not implement
    ///          this for Class A).
    /// Class B: performs the idempotent remote operation.
    /// Class C: performs the irreversible remote operation.
    async fn fire(
        &self,
        descriptor: &EffectDescriptor,
        cap: &Capability,
    ) -> Result<EffectReceipt, AdapterError>;

    /// Optional: compensate a fired effect. Class B: refundable
    /// compensation; Class C: best-effort cleanup (e.g., a
    /// cancellation email).
    async fn compensate(
        &self,
        receipt: &EffectReceipt,
    ) -> Result<(), AdapterError> {
        // default: no compensation
        Err(AdapterError::CompensationNotSupported)
    }
}
```

---

## 3. Effect classification rules

The adapter MUST classify each operation per these rules.

### 3.1 Class A — pure local, reversible

Operation MUST satisfy ALL of:

- No network call.
- No filesystem operation outside the View's Delta scope.
- No process spawn outside the View's process tree.
- Effect is reversed by Delta discard.

Examples: write to View-scoped path, in-memory state change, compute.

### 3.2 Class B — idempotent remote

Operation MUST satisfy:

- Carries a stable version reference (ETag, commit SHA, object
  version ID, idempotency key, ...) at submission time.
- Applying twice with the same version-ref has the same observable
  consequence as applying once.
- Precondition check is verifiable: `assert_precondition` is callable
  before `fire` and gates the fire on the assertion.

Examples: S3 PUT with ETag, Git push with parent SHA, version-tagged
HTTP PUT.

### 3.3 Class C — irreversible remote

Operation falls here if it CANNOT meet Class B's preconditions —
specifically, the external system has no way to express "do this
only if version is still X" or "if you've seen this id before, no-op".

Examples: send email, post Slack message, create Linear issue,
transfer funds.

### 3.4 Adapter-layer refinement: reversible-with-cost (Atomix axis)

An adapter MAY enrich a Class B effect with an annotation
`compensable_cost: Cost` indicating the cost of compensation (e.g.,
refund fee, rate-limit consumption). This is **adapter-layer
metadata** — the protocol surface does not distinguish "Class B
zero-cost" from "Class B with cost"; both are Class B.

This refinement aligns reel with Atomix's two-axis taxonomy
(Mohammadi et al. 2026) without altering reel's one-dimensional
protocol-layer classification.

---

## 4. Required adapters in MVP

Per the MVP scope:

| Adapter | Class | Purpose |
|---|---|---|
| `adapters/fs` | A | Filesystem (local files; Class A; reversible) |
| `adapters/slack` | C | Slack message (irreversible) — MVP needs at least one C adapter to exercise the buffer mechanism |

Post-MVP adapters listed in the post-MVP scope:

| Adapter | Class | Purpose |
|---|---|---|
| `adapters/github` | B | GitHub repo operations (with commit SHA / ETag) |

---

## 5. Adapter testing

Each adapter MUST pass:

- Per-class conformance tests (`spec/conformance/adapter_*.json`).
- The `t_002_t_003_t_004` verbs-semantics tests with its operation
  set.
- For Class B adapters: the `t_009_class_b_precondition_at_commit`
  test under fault injection.
- For Class C adapters: the `t_010_class_c_only_fires_at_commit`
  static reachability check.

---

## 6. Concurrency and isolation

At v0.1 (single-threaded kernel; ADR-0008), adapters do not need to
handle concurrent invocations of `fire`. The kernel serialises
commits. Adapter implementations MUST still be `Send + Sync` so the
kernel can move them across futures.

v0.2+ (concurrent kernel; future work) will require adapters to
implement per-resource locking or use external coordination
mechanisms. The trait may extend then.

---

## 7. Error model

Adapter errors map to reel's error codes (spec.md §9):

| Adapter signals | Reel error |
|---|---|
| Class B precondition mismatch | E-002 `CommitConflict` |
| Class B/C fire timeout | E-006 `EffectDrainTimeout` |
| Underlying storage failure | E-007 `Storage` |
| Other adapter-specific | wrapped as `AdapterError::Other(...)` then surfaced |

Adapter authors are responsible for mapping their resource's native
errors into the appropriate reel error class.

---

## 8. Authentication and authorisation

reel's `Capability` (spec.md §5.5) carries `path_prefix + op_set +
ttl`. Adapters MAY inspect the capability passed to `fire` to enforce
authorisation. The protocol does not mandate the authorisation scheme
beyond:

- The kernel MUST pass the capability used to submit the effect.
- The adapter MUST NOT fire if the capability does not cover the
  operation (op_set / path_prefix violation → E-009
  CapabilityViolation).
- Adapters MAY delegate to external auth systems (OAuth, mTLS, etc.).
  Such delegation is opaque to the protocol.

---

## 9. Reporting an adapter

An adapter publishes a manifest at `crates/<adapter>/adapter.toml`:

```toml
name        = "slack"
class       = "C"                    # or "A", "B"
scope       = "slack:*"
version     = "0.1.0"
reel-target = "v0.1"

[capabilities]
ops = ["post_message", "edit_message", "delete_message"]

[conformance]
last-tested = "2026-05-15"
level       = 1
```

The reel project's adapter registry (when published) lists
conformant adapters and their declared classes.
