# reel Conformance Specification

> What it means for an implementation to claim "this is reel".
> Companion to [spec.md](spec.md). All numbered sections reference
> spec.md unless noted otherwise.

## Status

- Version: v0.1 (2026-05-15).
- Canonical authority: ADR-0025.
- Test vectors: `spec/conformance/*.json` (per-invariant).

---

## 1. Mandatory tests

An implementation MUST pass every test in this section to claim
conformance. Tests are run by `cargo nextest run --workspace
--features=conformance`.

### 1.1 The six core types are exposed

Conformance test `t_001_types_exposed`:

- `Hash`, `Block`, `Ref`, `Delta`, `Capability`, `View` are all
  accessible from the implementation's public surface.
- `Block` is immutable (no mutator on Block bytes after construction).
- `Hash` is 32 bytes and produced by BLAKE3-256.
- `View.effects_hash` is of type `Option<Hash>` and refers to a Block
  whose payload is an effect sequence.

### 1.2 The three verbs have the specified semantics

`t_002_fork_creates_isolated_view`:

```
let parent = kernel.open(path);
let child = parent.fork();
child.delta.put("a", b"new");
assert!(parent.namespace.get("a") != Some(b"new")); // isolated
```

`t_003_commit_merges_atomically`:

```
let v = parent.fork();
let eff_id = v.submit_b(idempotency_key, ...);
let ref_or_err = v.commit();
// either: parent has the new Block AND eff fired
// or:    parent unchanged AND eff did not fire
```

`t_004_abort_discards_class_c`:

```
let v = parent.fork();
v.submit_c(class_c_effect);
v.abort();
// External system MUST NOT have observed the class-c effect.
assert!(external_observer.never_saw(class_c_effect));
```

### 1.3 The three core invariants hold

`t_005_i001_reachability`:

```
let h = some_block.hash();
// drop all refs that reach the block
drop_namespace();
gc.run();
let result = store.lookup_by_hash(h);
assert_eq!(result, Err(E-010 ReachabilityViolation));
```

`t_006_i002_abort_silences_class_c`:

```
for _ in 0..10000 {
    let v = parent.fork();
    v.submit_c(class_c_effect);
    if random_bool() {
        v.commit();
    } else {
        v.abort();
        assert!(external_observer.never_saw(class_c_effect));
    }
}
```

`t_007_i003_capability_narrowing`:

```
let cap = Capability::root_authority();
let narrowed = cap.derive(narrowing_args);
assert!(narrowed.is_sub_lattice_of(&cap));
let amplification_attempt = cap.derive(amplifying_args);
assert!(amplification_attempt.is_err()); // structurally rejected
```

### 1.4 Effect class discipline

`t_008_class_a_no_io`:

- Class A effects can be applied with no network or remote storage
  available.

`t_009_class_b_precondition_at_commit`:

- A Class B effect with stale version_constraint at commit time
  yields E-002 CommitConflict; parent state unchanged.

`t_010_class_c_only_fires_at_commit`:

- Static or instrumentation-based check: `class_c.fire` is reachable
  only from the kernel's commit path; not from abort, fork, or any
  external invocation.

### 1.5 Effect Buffer = Block

`t_011_effects_hash_is_block`:

- `view.effects_hash`, when `Some(h)`, refers to a Block in the
  store whose payload deserializes as `EffectSequence`.

- Hashing the effects payload yields the same `h`.

- `view.abort()` drops the reference; after GC, `store.get(h)`
  returns E-001 BlockNotFound.

### 1.6 Snapshot semantics

`t_012_snapshot_restore_class_a`:

- Save snapshot. Modify Class A state. Restore. State matches save.

`t_013_snapshot_restore_class_b_version_check`:

- Save snapshot with Class B effect carrying version_ref X.
- Externally change resource to version Y.
- Restore: implementation MUST raise explicit warning (not silent
  proceed).

`t_014_snapshot_restore_class_c_pending`:

- Save snapshot with pending Class C effect.
- Restore. Class C effect is pending; user decides commit/abort.

---

## 2. Optional conformance levels

### Level 1 — MVP

- Sections 1.1 through 1.6 pass.
- Adapters: `fs` (Class A) and at least one of `{slack, github, ...}`
  for Class B or C.

### Level 2 — Multi-adapter

- Level 1 + Class B (versioned) adapter + Class C (irreversible)
  adapter.
- Conformance test suite extended per-adapter.

### Level 3 — Production-grade

- Level 2 + crash-recovery: kernel crash mid-commit recovers to a
  consistent state (either all of commit, or none).
- Level 2 + concurrent-fork: multiple `fork`s in flight maintain
  Φ₄ (atomicity) and Φ₅ (sibling isolation) — see spec.md §7.2.
- Level 2 + Snapshot import/export of `.reel` archives is
  byte-deterministic.

### Level 4 — Formal verification (post-MVP advisory)

- Apalache inductive proof of inv_i002 over `spec/formal/reel.qnt`
  passes.
- Verus refinement annotation on the Rust kernel proves the impl
  refines the formal model.

Levels 1–3 are required for an implementation to claim
"reel-conformant". Level 4 is reserved for implementations that wish
to claim "reel-verified".

---

## 3. Non-conformance examples (what's NOT reel)

An implementation is NOT reel if any of:

- It fires a Class C effect after `abort` is called on the owning
  View (breaks I-002).
- It permits a derived Capability to expand its delegator's authority
  in any dimension (breaks I-003).
- It permits access to a Block whose hash is known but is not
  reachable from any current Ref (breaks I-001).
- It does not expose `View.effects_hash` as a `Block`-typed reference
  (breaks the structural commitment that pending effects are
  content-addressed).
- It does not distinguish Class A/B/C at the effect type level
  (collapses the protocol's central distinction).

---

## 4. Test artefact format

Each conformance vector lives at `spec/conformance/<topic>.json` and
follows the schema in `spec/schema/2026-Q3/conformance-vector.json`.

A vector defines:

```json
{
  "id": "t_006_i002_abort_silences_class_c",
  "invariant": "I-002",
  "setup": [...],
  "actions": [...],
  "expected": {
    "external_observations": [],
    "view_status": "aborted",
    "errors": []
  }
}
```

Conformance vectors are language-neutral. A Rust implementation reads
them via `crates/reel-conformance-runner/`; future implementations in
other languages MUST consume the same vector format.

---

## 5. Reporting conformance

An implementation publishes a conformance report at
`docs/conformance-report.md` with:

- Implementation version + commit hash.
- Level claimed (1 / 2 / 3 / 4).
- Per-test pass/fail.
- Adapter list.
- Date of last test run.

The reel project MAY maintain a public registry of conformant
implementations once the protocol has been published.
