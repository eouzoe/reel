# reel Protocol Specification

## Abstract

reel is a protocol for the boundary between an AI agent's speculative
computation and the outside world. It exposes three verbs (`fork`,
`commit`, `abort`) over six core types (`Hash`, `Block`, `Ref`,
`Delta`, `Capability`, `View`) and a three-class classification of
side effects (Class A / B / C). Its three core invariants — I-001
Reachability, I-002 Abort silences Class C, I-003 Capability narrowing
— exhaustively partition the correctness concerns of agent
speculative execution. Any property that does not reduce to a
property over this ontology is out of scope for reel.

## Status of This Document

- **Schema version**: 2026-Q3 (v0.1 pre-release).
- **Document status**: Draft.
- **Canonical ontology**: ADR-0025.
- **Supersedes**: none.
- **Superseded by**: none.

<a id="s-1"></a>
## 1. Introduction

This section is informative.

An AI agent reasons speculatively. Some of its tentative actions
modify the outside world: a Slack message sent, a row inserted, an
email dispatched. Once such an action crosses the boundary into the
world, no decision the agent later makes — including the decision to
abandon that line of reasoning — can take it back.

reel exists to give the agent a guarantee: **an action the system has
decided to discard must never have crossed the boundary**. This is
captured formally as **I-002** (§7.1).

The protocol expresses this guarantee through:

- Three verbs (§6): `fork` opens speculation; `commit` makes it real;
  `abort` discards it.
- Six core types (§5): immutable content (`Block`), names (`Ref`),
  mutations (`Delta`), scoped authority (`Capability`), workspace
  closures (`View`), and the identity constructor (`Hash`).
- Three side-effect classes (§5.7) that distinguish reversibility:
  Class A (pure local), Class B (idempotent remote), Class C
  (irreversible remote).

**Intellectual lineage** (cited per ADR-0005):

- Wang & Zheng arXiv 2602.08199 — OS-level fork/commit/abort verbs.
- Mohammadi et al. arXiv 2602.14849 (Atomix) — transactional tool
  use; effect taxonomy.
- Berenson et al. SIGMOD 1995 — snapshot isolation as the formal
  reading of `View`.
- Miller-Yee-Shapiro 2003 — object-capability model as the formal
  reading of `Capability`.
- Garcia-Molina & Salem SIGMOD 1987 — Saga + pivot transactions; the
  ancestral classification of irreversible operations.
- Haerder & Reuter ACM CS 1983 — ACID; the exhaustiveness rhetoric
  this document inherits.

**What is novel** to reel: the structural choice to make the pending
effect buffer a content-addressed `Block`. This is reel's distinctive
contribution; the rest is composition.

<a id="s-2"></a>
## 2. Requirements Language

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and
"OPTIONAL" in this document are to be interpreted as described in
BCP 14 [RFC 2119](https://www.rfc-editor.org/info/rfc2119)
[RFC 8174](https://www.rfc-editor.org/info/rfc8174) when, and only
when, they appear in all capitals, as shown here.

<a id="s-3"></a>
## 3. Conformance

A reel implementation is conformant iff:

- It exposes the three verbs (§6) with semantics matching this spec.
- It implements every core type (§5) including the structural
  commitment that `View.effects_hash` is a Block-typed reference.
- It honours each View's `Boundary` (§5.6, ADR-0032): an `Isolated`
  commit merges effects up to the parent; an `Autonomous`/root commit
  drains them. I-002 holds across the Isolated subtree (§7.1).
- It preserves I-001, I-002, and I-003 (§7.1) across every
  observable trace.
- It classifies effects per §5.7 and respects per-class discipline
  (§8).
- Errors defined in §9 are signalled with the corresponding code.

Detailed conformance criteria — mechanical tests an implementation
MUST pass — are in [`conformance.md`](conformance.md). Adapters
implementing per-class behaviour MUST satisfy the trait specified in
[`adapter-interface.md`](adapter-interface.md).

<a id="s-4"></a>
## 4. Terminology

Definitions live in [`glossary.md`](glossary.md). Terms used
normatively here:

- **Block**, **Ref**, **Delta**, **Capability**, **View** — §5.
- **Hash** — BLAKE3-256 digest serving as `Block` identity.
- **Effect** — a `Block` whose payload satisfies an effect schema
  (§5.6).
- **Snapshot** — a verifiable reproduction descriptor for a `View`
  (§5.8).
- **Boundary** — a View's nesting discipline: `Isolated` (its `commit`
  merges effects up into the parent) or `Autonomous` (its `commit`
  drains effects to the world). Set at `fork` (§6.1); see
  ADR-0032.

<a id="s-5"></a>
## 5. Data Model

reel commits to six core types. Together they exhaustively describe
the persistent state space of an agent's speculative execution.

> **Note on `Hash`**: `Hash` is the identity constructor for `Block`.
> We list it among the six because Rust APIs handle it as a distinct
> type alias; conceptually it is derived. See ADR-0025.

<a id="s-5-1"></a>
### 5.1 Hash

`Hash = Blake3Digest` — 32-byte BLAKE3-256 digest. Used as `Block`
identity.

The implementation MUST use BLAKE3-256 (per
ADR-0004).
Implementations MAY substitute a hash function with ≥ 128-bit
collision resistance margin in future schema versions; doing so MUST
NOT weaken the collision-resistance guarantee.

<a id="s-5-2"></a>
### 5.2 Block

A **Block** is an immutable, content-addressed unit:
`{ hash: Hash, data: Bytes }` where:

- `hash` is the BLAKE3-256 digest of `data` — and only of `data`. Block
  identity is a pure function of content (per ADR-0004).
- `data` is the opaque, zero-copy-shareable byte payload (typed as
  `bytes::Bytes` in the reference implementation, so clone is an O(1)
  refcount bump, not a memcpy).

Two Blocks with identical `data` produce identical `hash` and are
therefore the same Block. This is the deduplication property
content-addressing depends on; it is also the determinism precondition
for the formal verification of I-002. Modifying a Block produces a
*new* Block with a new `hash`; the old Block persists unchanged.

**Lineage is not identity.** A Block's provenance — who/when/how
content came to be — is deliberately **not** a field of `Block` and
**not** part of `hash`. Because content-addressed Blocks deduplicate,
one physical Block can be introduced by many events and therefore
cannot own a single lineage record. Provenance is recorded on the
**commit** that introduced the content (reel's analogue of a Nix
derivation / Git commit-object), forming the lineage/audit graph; see
the `Provenance` type. This matches Nix (Dolstra 2006): a content-
addressed store object answers *what it is*; a derivation records
*how it was made* — two separate objects.

Blocks have no class; classes are properties of *effect descriptors*
within a Block payload (§5.6). Blocks are the data over which Views
operate.

<a id="s-5-3"></a>
### 5.3 Ref

A **Ref** is a named mutable pointer: `{ name: String, hash: Hash,
version_constraint: Option<VersionRef> }` where:

- `name` is a stable string identifier (UTF-8; reel-defined path
  syntax).
- `hash` is the `Block` currently referenced.
- `version_constraint` is an OPTIONAL precondition for Class B effect
  commits (§5.7.2): an `ETag`, commit SHA, object version ID, or
  similar external version reference.

A Ref MUST be mutated only through a successful `commit` (§6.2). The
mapping `name → hash` updates atomically; the name does not change,
but the hash it resolves to may.

<a id="s-5-4"></a>
### 5.4 Delta

A **Delta** records ordered mutations from a base Block: `{ base_hash:
Hash, ops: Vec<Op> }`. Each `Op` is an adapter-defined record (e.g.,
`Put(name, hash)`, `Remove(name)`).

A View carries one Delta layer above its inherited namespace. `fork`
gives the child an empty Delta with the parent's effective state as
`base_hash`. `commit` applies the Delta to the parent. `abort`
discards it.

Inheritance from the parent is structural (the child's Delta layer
overlays the parent's effective namespace), not by value, so `fork`
remains O(1) in the size of the parent's content (per I-003-perf
in [`invariants.yaml`](invariants.yaml), §convenience; performance
bound).

<a id="s-5-5"></a>
### 5.5 Capability

A **Capability** is a scoped attenuating authority: `{ path_prefix:
String, op_set: Set<OpKind>, ttl: u64 }` where:

- `path_prefix` is the namespace prefix this Capability covers.
- `op_set` is the set of operation kinds permitted (e.g.,
  `{Read, Write}`).
- `ttl` is the time-to-live in seconds (0 = unbounded).

A Capability MUST be **monotone-narrowing**: derived Capabilities can
only attenuate (sub-set, sub-prefix, ≤ TTL). Amplification is
forbidden by I-003 (§7.1). This corresponds to the object-capability
model's *attenuation* pattern (Miller-Yee-Shapiro 2003) and the APS
IETF draft's monotone-narrowing lattice.

Capabilities are first-class state: they live in `View.caps`. They
are passed across `fork` (the child inherits, optionally attenuated),
constructed only by the kernel, and held only by Views.

<a id="s-5-6"></a>
### 5.6 View

A **View** is a workspace closure:

```
View = {
    namespace:    Map<Name, Hash>,
    delta:        Delta,
    caps:         Vec<Capability>,
    effects_hash: Option<Hash>,
    status:       Status,
    parent:       Option<ViewId>,
    boundary:     Boundary,
}
```

where:

- `namespace` is the set of Refs visible from inside the View, each
  resolved to a Block hash.
- `delta` is the View's pending mutations to its namespace.
- `caps` is the View's authority set.
- `effects_hash` is `Some(h)` where `h` is the hash of a Block whose
  payload is the sequence of pending Class B and Class C effects for
  this View — or `None` if no effects have been buffered.
- `status` is one of `active`, `committed`, `aborted` (§5.6.1).
- `parent` is the parent View's id, or `None` for a root View.
  `parent` is provenance metadata: it records the View this one was
  forked from, so audit and visualisation tooling can reconstruct the
  fork tree. It is NOT part of any reachability or invariant
  predicate; traversing `parent` chains is not a normative operation
  and an implementation MAY drop `parent` from a terminal View
  without breaking conformance.
- `boundary` is `Isolated` or `Autonomous` (per
  ADR-0032),
  fixed at `fork`. It determines whether this View's `commit` **merges
  its effects up** into the parent's buffer (`Isolated`) or **drains
  them to the world** (`Autonomous`). A root View is always
  `Autonomous` (it is the outermost scope = "the world"); forked
  children default to `Isolated`.

Conformant implementations MUST expose `effects_hash` read-only on
the public View surface (e.g. as a method or property returning the
hex form of the hash, or `None`). The `Some(h) → None` transition on
`abort` is the host-observable structural witness for I-002 and
callers MUST be able to observe it independently of any specific
adapter log.

**Effect Buffer = Block**: the choice to make the pending effects a
content-addressed Block is reel's distinctive structural commitment.
It means:

- Pending effects are immutable (modifying the buffer produces a new
  Block with a new hash; the View's `effects_hash` updates to the new
  hash).
- Pending effects are addressable, hash-verifiable, and shareable
  (the View can be diffed, audited, transmitted by hash).
- `commit` reduces to: atomically move the pending-effects Block from
  the pending log to the committed log, then drain it.
- `abort` reduces to: drop the reference to the pending-effects Block
  (it becomes unreachable per I-001 and is garbage-collected).

This structural choice subsumes the supporting properties of ADR-0024
(Φ₁/Φ₂/Φ₄/Φ₅) as derived properties of the type structure (§7.2).

<a id="s-5-6-1"></a>
#### 5.6.1 View states

A View is in exactly one of:

- `active`: accepts further verbs (`fork`, `submit_*`, `commit`,
  `abort`).
- `committed`: terminal; the View's effects have been flushed and its
  Delta merged into the parent.
- `aborted`: terminal; the View's pending state was discarded.

Operations on a terminal View MUST be rejected with `E-008`
(`ViewTerminal`).

The single non-terminal state has been called both "active" (per
REEL_CORE_REQUIREMENTS) and "Draft" (in prior iterations). The
canonical name in this spec is **active**. Implementations MAY use a
locally consistent translation; conformance vectors use `active`.

<a id="s-5-7"></a>
### 5.7 Effect Classes

An **Effect** is a Block whose payload describes a side effect to be
performed at `commit` time. The classification:

The three classes are not new. They are Jim Gray's 1981 action
taxonomy (*The Transaction Concept: Virtues and Limitations*, VLDB
1981) carried to the agent-to-world boundary:

| Gray 1981 | reel | Gray's defining property (verbatim) |
|---|---|---|
| *unprotected* | Class A | "the action need not be undone or redone if the transaction must be aborted" |
| *protected* | Class B | "the action can and must be undone or redone if the transaction must be aborted" |
| *real* | Class C | "once done, the action cannot be undone" |

Gray's prescription for *real* actions is reel's Class C mechanism
almost verbatim: "A real action which cannot be undone must be
deferred until transaction commit … deferred by keeping a redo log of
deferred operations." reel's `View.effects_hash` (§5.6) is precisely
that deferred-operation log. What reel adds to Gray is not the
classification — it is the decision to make the deferred-effect log a
*content-addressed Block*, so that deferral, discard, and audit reduce
to reachability over the Block graph (I-001) rather than to bespoke
log bookkeeping.

#### 5.7.1 Class A — pure local, reversible

Class A effects act only on the View's `delta` layer or on transient
computation. They have no observable consequence outside the system.
They apply directly to the Delta and are reversed by Delta discard at
`abort`. Class A effects do NOT enter `View.effects_hash`.

Examples: local file edit, RAM-only state, in-process compute.

#### 5.7.2 Class B — idempotent remote

Class B effects act on external resources. They MUST carry an
identifier that is **stable per effect** (an ETag, commit SHA, object
version ID, or other version reference). The protocol guarantees:

- At `commit` time, the implementation MUST verify the version
  reference is still valid via a precondition check.
- If the precondition fails, the implementation MUST return an
  explicit error (`E-002` `CommitConflict`) — never silently proceed.
- If the precondition holds, the effect fires and the version
  reference becomes the new authoritative version.

This is the *idempotency-under-stable-identifier* semantics of Class
B: applying the same Class B effect with the same identifier twice
has the same observable consequence as applying it once.

Note: this is **not** "no duplicate keys forbidden". Two Class B
effects with the same key are equivalent under idempotency; both
attempting to apply means the second is a no-op or matches a prior
result. The 2026-05-15 morning iteration (ADR-0024) corrected the
prior misreading of this as a uniqueness constraint.

Examples: S3 PUT with ETag, Git push with commit SHA, version-tagged
HTTP API.

#### 5.7.3 Class C — irreversible remote

Class C effects act on external resources and CANNOT be undone by any
further action of reel. Once observable, they persist independent of
reel's state.

- They are buffered in `View.effects_hash` until `commit`.
- At `commit`, they fire after all Class B effects have completed (per
  ADR-0015).
- At `abort`, they MUST NOT fire and the buffer is discarded.
- The `commit` UI MUST display each Class C effect's full content and
  an irreversibility warning before the user authorises the commit.

Examples: Slack message, email send, Linear issue create, financial
transfer.

#### 5.7.4 Adapter-layer refinement

The classification above is **a one-dimensional projection** of
Atomix's (Mohammadi et al. 2026) two-axis matrix
(reversibility × bufferability). At the protocol layer, all effects
MUST be bufferable (the protocol's commitment). The
"reversible-with-cost" case (refundable charge, rate-limited API)
that Atomix identifies separately is collapsed by reel into Class B
with an optional adapter-level `compensable_cost` annotation; this is
not part of the protocol surface.

The class boundary between Class A and {B, C} is the boundary of the
system. The boundary between Class B and Class C is the boundary of
recoverability under reel's own action.

<a id="s-5-8"></a>
### 5.8 Snapshot

A **Snapshot** is a verifiable reproduction descriptor for a View:

```
Snapshot = {
    namespace_config,
    delta_layer_bytes,
    effects_hash:  Option<Hash>,
    version_refs:  Map<Path, VersionRef>,
    capability_set: Vec<Capability>,
}
```

A Snapshot is NOT a complete copy of the world; it is a description
sufficient to *reproduce* a View on a compatible kernel. Restore
semantics by class:

- **Class A**: exact restoration from `delta_layer_bytes`.
- **Class B**: assert `version_refs` still match the remote system;
  fail with explicit error if not.
- **Class C**: restore as pending; the user decides commit-or-abort.

A View is to a Snapshot as a database transaction is to its read-set
+ write-set + isolation context: the Snapshot captures everything
needed to relocate the View deterministically. This corresponds to
Berenson et al. 1995 Snapshot Isolation transactions.

<a id="s-6"></a>
## 6. Operations

reel exposes three verbs. They are the only operations that
transition Views between states. There is no `merge`, no `rebase`, no
`cherry-pick`: any compound operation reduces to a sequence of
fork/commit/abort.

<a id="op-fork"></a>
### 6.1 fork

`fork(parent: View, boundary: Boundary) → View`

Creates a new View whose parent is `parent`. The new View:

- Inherits `parent.namespace` by reference (COW; per
  ADR-0002 and
  Wang & Zheng 2026 — O(1) in payload size).
- Has empty `delta`.
- Inherits `parent.caps`, optionally attenuated by the caller (I-003).
- Has `effects_hash = None`.
- Has `status = active`.
- Has `boundary` as supplied by the caller (`Isolated` — the default —
  buffers effects up to the parent on commit; `Autonomous` drains to
  the world on commit). Per
  ADR-0032.

The implementation MUST:

- Reject `fork` if `parent.status != active`, returning `E-008`.
- Complete `fork` in O(1) relative to parent payload size (I-003
  performance bound).
- Reject with `E-005` (`ForkDepthExceeded`) if the resulting depth
  would exceed `MAX_FORK_DEPTH` (default 64).

<a id="op-commit"></a>
### 6.2 commit

`commit(v: View) → Result<Ref, Error>`

Atomically transitions `v` from `active` to `committed`. The
implementation MUST execute, in order:

1. **Validation phase**:
   - Verify `v.status == active`. If not, return `E-008`.
   - For each Class B effect in `v.effects_hash`: assert its
     `version_constraint` is still satisfied on the remote system.
     If not, return `E-002` (`CommitConflict`). On validation
     failure, `v.status` and all parent state MUST be unchanged.

2. **Drain phase** (atomic — see §7.1 I-002 + §7.2 D-3). The treatment
   of buffered effects depends on `v.boundary` (per
   ADR-0032):
   - Merge `v.delta` into the parent's `delta` (or into the global
     namespace, if `v` is a root View).
   - Update each affected `Ref` in the parent namespace:
     `parent.namespace[ref_name] := new_committed_hash`. This is a
     **replace**, not a structural merge — the previous binding is
     overwritten in a single atomic operation. Concurrent-fork merge
     semantics (multiple sibling Views committing against the same
     parent ref) are out of scope at v0.1 and will be addressed by a
     future ADR; the v0.1 contract is last-writer-wins under
     single-agent serial commits.
   - Mint a new `Ref` for the resulting Block.
   - **Effect handling, by boundary:**
     - If `v.boundary == Autonomous` (or `v` is a root View): fire each
       Class B effect under its idempotency identifier; then fire each
       Class C effect, in submission order, after all Class B effects
       have completed (per
       ADR-0015); move
       the pending-effects Block reference from the pending log to the
       committed-effects log.
     - If `v.boundary == Isolated`: **fire nothing.** Append `v`'s
       pending-effects Block to the parent's `effects_hash` buffer —
       the parent's buffer becomes a new content-addressed Block whose
       hash supersedes the parent's previous `effects_hash`. The
       effects stay buffered until an `Autonomous`/root ancestor
       commits and drains them. This is what carries I-002 across the
       subtree: an `abort` of any ancestor before that drain silences
       these effects (§6.3, §7.1).
   - Empty `v.effects_hash` (set to `None`).
   - Set `v.status = committed`.

These steps form a single atomic operation: either all complete or
none. (See §7.2 D-3 atomicity.)

`commit` is **not** cancel-safe at v0.1; if the returned future is
dropped mid-drain, the state will not be observable as `committed`
until the drain completes. Implementations MAY retry under transient
errors (Class B idempotency guarantees safety; Class C requires
adapter-defined retry policy).

<a id="op-abort"></a>
### 6.3 abort

`abort(v: View) → ()`

The implementation MUST first verify `v.status == active`. If `v` is
already in a terminal state (`committed` or `aborted`), the
implementation MUST return `E-008` (`ViewTerminal`) and leave `v`
unchanged. A correct implementation never silently re-aborts.

Otherwise the implementation atomically transitions `v` from
`active` to `aborted`:

1. Discard `v.delta`.
2. Drop the reference from `v.effects_hash` (the pending-effects
   Block becomes unreachable per I-001 and is garbage-collected).
3. Set `v.status = aborted`.
4. **No Class B effect fires.** **No Class C effect fires.**

`abort` is cancel-safe by virtue of being synchronous.

Policy for *active* descendants of an aborted View (cascade-abort,
orphan, or reject-further-commits) remains an implementation choice;
the protocol does not mandate one. However, for **`Isolated`
descendants whose effects have already merged up** into `v`'s buffer
(§6.2), `abort(v)` MUST silence them: dropping `v.effects_hash` makes
the entire merged-in Isolated-subtree buffer unreachable (I-001) and
therefore unfired. Under any chosen policy for active descendants,
I-002 (§7.1) MUST continue to hold for the original View and for every
`Isolated` descendant that committed into it (per
ADR-0032).

<a id="s-7"></a>
## 7. Invariants

reel commits to **three core invariants**. These exhaustively
partition the protocol's correctness concerns: any property that does
not reduce to one of these is out of protocol scope.

Each invariant has a machine-readable entry in
[`invariants.yaml`](invariants.yaml) and a formal Quint statement in
[`spec/formal/reel.qnt`](formal/reel.qnt) (advisory layer; see §3
conformance).

<a id="s-7-1"></a>
### 7.1 Core invariants

<a id="i-001"></a>
**I-001 — Reachability**.
A `Block` is *reachable* iff it is reachable through the transitive
closure of `Ref → Block` (namespace) and `Block → Block` (parent
hash, in Delta or schema-defined edges) edges starting from the
current root namespace. The implementation MUST guarantee:

- No code path grants access to a Block solely on knowing its
  `Hash`.
- Garbage collection of unreachable Blocks is statically decidable
  from the reachability closure.

This dissolves the "I know a hash so I can resurrect a dead Block"
class of attacks and makes storage GC tractable. Inherited from Git
(reachability defines what is preserved) and IPFS
(content-addressing without ambient discoverability).

<a id="i-002"></a>
**I-002 — Abort silences Class C effects**.
Let `v` be a View. Let `e` be a Class C effect buffered in
`v.effects_hash`. If `abort(v)` is invoked, then `e.fire` MUST NOT
have been invoked at any point in `v`'s lifetime, and MUST NOT be
invoked thereafter.

**Scope (per ADR-0032).**
The buffered set over which I-002 quantifies depends on `v.boundary`:
for an `Autonomous` View it is `v`'s own buffer; for a View with
committed `Isolated` descendants it is `v`'s buffer **plus** every
Isolated descendant's effects that merged up into it (§6.2). Because
the merge is by content-addressed Block reference, `abort(v)` dropping
`v.effects_hash` renders the entire Isolated-subtree buffer unreachable
(I-001) in a single operation — the single-View mechanism composes to
the subtree with no extra machinery. The v0.1 formal proof
(`spec/formal/reel.qnt`) covers the single-View case only; the
Isolated-subtree extension is deferred per ADR-0030.

This is the protocol's *reason to exist*. An implementation that
breaches I-002 once is not a degraded reel; it is not reel.

The mechanism: `View.effects_hash` is a `Block`-typed reference;
`commit` moves it to the committed-effects log atomically; `abort`
drops it; no other code path can fire the buffered effects.

<a id="i-003"></a>
**I-003 — Capability narrowing**.
For any delegation chain of `Capability` values: each delegate's
authority is a sub-lattice of its delegator's authority. Capabilities
only attenuate (sub-set `op_set`, sub-prefix `path_prefix`, ≤ TTL);
amplification is forbidden.

**Enforcement layer**. I-003 is enforced at `fork` time by structural
typing: the protocol's only Capability-derivation primitive
(`fork(parent, child_caps)` and its delegated form) MUST reject any
`child_caps` element that widens `parent.caps` along any axis,
returning `E-009` (`CapabilityViolation`). No primitive in the
protocol surface constructs an attenuated Capability that fails to
preserve the sub-lattice; amplification has no representable
operation. Runtime authority enforcement at individual write /
submission sites (whether a given `submit_a/b/c` is authorised by
the active capability set) is **NOT** part of the protocol surface
at v0.1 — it is an adapter / host responsibility. A host that wants
runtime authority enforcement (e.g. a VFS that intercepts writes and
consults the View's capability set) MAY implement it, but the
protocol does not require it; the protocol's guarantee is that
authority handed to a child is provably less than the parent's, not
that every operation the child performs is re-checked.

Inherited from Miller-Yee-Shapiro 2003 (object-capability model) and
the APS IETF draft (monotone-narrowing lattice across seven
dimensions). Required for compositional security reasoning.

<a id="s-7-2"></a>
### 7.2 Derived properties

The following properties are NOT independent invariants — they are
structural consequences of the six core types + three verbs.
Implementations get them for free from the type design.

- **D-1 Class immutability**. A `Block` is immutable. A Block whose
  payload is an Effect cannot change class. Consequence of Block
  immutability.

- **D-2 Class B+C exit only via commit**. `View.effects_hash` is
  modified only by `submit_b`/`submit_c` (buffering) and `commit`
  (flushing); `abort` drops it. The protocol surface has no other
  verb that touches `effects_hash`. Consequence of verb enumeration.

- **D-3 Atomicity of commit and abort**. `commit` and `abort` are
  single-Ref-update operations at the impl level; no partial state is
  observable. At v0.1 (single-threaded kernel, per
  ADR-0008),
  this is trivially true. v0.2+ multi-threaded requires an additional
  proof.

- **D-4 Sibling isolation**. Two `View` records are value records;
  mutating one does not mutate another. Consequence of record value
  semantics.

The 2026-05-15 morning iteration (ADR-0024) numbered these as Φ₁,
Φ₂, Φ₄, Φ₅. ADR-0025 demoted them to derived properties: the type
structure now carries them.

<a id="s-7-3"></a>
### 7.3 Convenience bounds

These are engineering bounds, not protocol invariants. They are
satisfied at impl level, not at spec level.

- **I-003-perf (fork O(1))**: fork completes in time independent of
  the parent payload's size. Inherited from Wang & Zheng 2026
  (BranchFS sub-350μs).
- **I-008 (depth bound)**: `View.depth ≤ MAX_FORK_DEPTH` (default
  64). Stack-overflow protection.

<a id="s-8"></a>
## 8. Effect Class Discipline

This section is normative; architectural realisation is in
`03-effect-isolation`.

### 8.1 Class A discipline

A Class A effect MUST NOT perform any I/O outside the View's Delta
layer or transient computation. Class A's signature carries no
externally visible side-effect channel; combined with safe-Rust
constraints, this keeps Class A inside the boundary.

### 8.2 Class B discipline

A Class B effect MUST carry a stable identifier and a precondition
that the implementation can verify at commit. Two Class B effects
with the same identifier are equivalent under idempotency. The
implementation MAY rely on this when retrying a mid-commit failure.

### 8.3 Class C discipline

A Class C effect's `fire` MUST be invoked at most once per
`(view, effect)` pair, and only during the `commit` of its owning
View. The implementation MUST enforce this mechanically; the typical
realisation is a linear `Capability` constructed only inside
`commit`'s drain phase, but other mechanisms (per-effect counter,
kernel-private constructor) are acceptable. The construction site
MUST NOT be reachable from `abort` or from any code path outside
`commit`.

<a id="s-9"></a>
## 9. Errors

Defined in [`errors.yaml`](errors.yaml).

- **E-001 `BlockNotFound`** — Store has no Block matching the id.
- **E-002 `CommitConflict`** — Class B `version_constraint`
  precondition failed at commit; or concurrent commits modified
  overlapping state.
- **E-003 `ClassCAfterAbort`** — internal: a Class C effect was
  attempted after abort. MUST NOT be reachable; reaching this state
  is a breach of I-002 and a critical bug.
- **E-005 `ForkDepthExceeded`** — fork would exceed `MAX_FORK_DEPTH`.
- **E-006 `EffectDrainTimeout`** — Class B or Class C `fire`
  exceeded the configured timeout.
- **E-007 `Storage`** — underlying storage error.
- **E-008 `ViewTerminal`** — operation attempted on a terminal View.
- **E-009 `CapabilityViolation`** — operation attempted without
  sufficient Capability (I-003 enforcement).
- **E-010 `ReachabilityViolation`** — attempt to access a Block not
  reachable through current namespace (I-001 enforcement).

(E-004 was removed in the 2026-05-15 morning reframe; see ADR-0024 +
ADR-0025.)

### 9.1 Programmatic error classification

Bindings and adapters MUST classify errors by the stable `E-NNN`
code, not by parsing the [`Display`] or [`Debug`] rendering of the
error type. The reference Rust implementation exposes this as a
first-class accessor on `ReelError`:

```rust
fn code(&self) -> &'static str;       // returns e.g. "E-008"
fn is_recoverable(&self) -> bool;     // false for E-003/006/007
```

Non-Rust bindings MUST surface the same identifier as a typed
exception class (preferred — e.g. `ReelViewTerminal` for E-008) or
as a structured field on a generic error type. Bindings MUST NOT
rely on substring matching against the human-readable error message;
that surface is for humans, not for programs.

### 9.2 Error catalogue stability

Each `E-NNN` code is stable within a schema-version track. New
variants MUST claim a new code; retired codes MUST NOT be re-used
in the same track. E-004 is permanently reserved in the 2026-Q3
schema track.

<a id="s-10"></a>
## 10. Security Considerations

- The implementation MUST verify Block identity via BLAKE3-256
  (ADR-0004). Substitution requires schema-version bump.
- Capabilities MUST be unforgeable and constructed only by the kernel.
- The Class C firing path (typically a linear `Capability` minted in
  commit) MUST NOT be reachable from `abort` or any non-commit path.
  Violation breaks I-002.
- The implementation MUST NOT log payload, idempotency-key, or
  capability contents; such material MAY contain sensitive data.
- Block content is opaque; the implementation MUST treat Block bytes
  as data, not as code.

<a id="s-11"></a>
## 11. Privacy Considerations

v0.1 of reel does not handle user-identifiable data directly; all
such data lives in `Block` payloads, which the protocol treats as
opaque bytes. Privacy guarantees are the responsibility of the
consuming application.

GDPR right-to-be-forgotten is in tension with Block immutability. The
intended resolution is **cryptographic erasure**: Block bytes are
stored encrypted under a per-Block key; deleting the key renders the
Block permanently unreadable. Not implemented in v0.1; the
architecture must not preclude it.

<a id="s-12"></a>
## 12. Versioning and Compatibility

The protocol's normative content is frozen per schema version
(`spec/schema/<YYYY-Qn>/`). Current: **2026-Q3**.

A new schema version **MAY**:

- Add convenience bounds.
- Add error codes (E-NNN with NNN > 10).
- Add adapter-layer effect refinements.

A new schema version **MUST NOT**:

- Renumber an existing core invariant.
- Reverse the semantics of an existing MUST requirement.
- Remove or weaken I-001, I-002, or I-003.

Configuration constants (informative; MAY be tuned per deployment):

| Constant | Default |
|---|---|
| `MAX_BLOCK_SIZE` | 16 MiB |
| `MAX_FORK_DEPTH` | 64 |
| `EFFECT_DRAIN_TIMEOUT` | 60 s |

Tuning these constants is NOT a schema-version change.

## References

### Normative inheritance
- Gray, J. (1981). The Transaction Concept: Virtues and Limitations. VLDB 1981: 144–154. (Origin of the unprotected/protected/real action taxonomy = reel Class A/B/C, §5.7; and of "defer real actions until commit" = reel's `effects_hash` mechanism, §5.6.)
- Wang, C., Zheng, Y. (2026). Fork, Explore, Commit. arXiv 2602.08199v2.
- Berenson, H., et al. (1995). A Critique of ANSI SQL Isolation Levels. SIGMOD.
- Miller, M., Yee, K., Shapiro, J. (2003). Capability Myths Demolished.
- Garcia-Molina, H., Salem, K. (1987). Sagas. SIGMOD.
- Haerder, T., Reuter, A. (1983). Principles of Transaction-Oriented Database Recovery. ACM CS.
- BCP 14, [RFC 2119](https://www.rfc-editor.org/info/rfc2119) +
  [RFC 8174](https://www.rfc-editor.org/info/rfc8174).
- [RFC 9562](https://www.rfc-editor.org/info/rfc9562) — UUIDs.

### Comparative positioning
- Mohammadi, B., et al. (2026). Atomix. arXiv 2602.14849.
- Li, Y., et al. (2025). AgentGit. arXiv 2511.00628.
- IPFS Merkle-DAG: github.com/ipfs/specs/blob/main/MERKLE_DAG.md
- Sanjuan, H., et al. (2020). Merkle-CRDTs. Protocol Labs Research.

### Formal methodology (advisory layer)
- Lamport, L. (2015). Who Builds a House Without Drawing Blueprints? CACM April.
- Hawblitzel, C., et al. (2015). IronFleet. SOSP.
- Boruch-Gruszecki, A., et al. (2024). Capture Tracking. OOPSLA.
- BLAKE3 — github.com/BLAKE3-team/BLAKE3
- Verus — github.com/verus-lang/verus
- Quint — quint-lang.org
