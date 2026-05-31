# reel Glossary

> Informative. Definitions referenced from
> [`spec/spec.md`](spec.md) §4. Aligned with
> ADR-0025
> canonical ontology.

## A

**Abort** — The protocol verb (§6.3) that transitions a View to
state `aborted`, discards its Delta, drops the reference to its
pending-effects Block (which becomes unreachable per I-001), and
guarantees per I-002 that no Class C effect of the View has fired or
will fire.

**Active** — One of the three View states; the only non-terminal
state. A View in `active` state accepts further verbs (fork,
submit_*, commit, abort). See spec.md §5.6.1.

**Adapter** — Code that wires an external resource (filesystem,
Slack, GitHub, Postgres, ...) into reel's protocol surface. Adapters
classify operations into Class A/B/C and implement `fire` /
`assert_precondition` / `compensate`. See
[`adapter-interface.md`](adapter-interface.md).

**Aborted** — A terminal View state (post-`abort`). The View MUST
NOT accept further verbs; operations return E-008 ViewTerminal.

**Apalache** — Symbolic model checker for TLA+. Quint compiles to
TLA+ for Apalache consumption. Apalache verification of
`spec/formal/reel.qnt` is advisory R&D, not an MVP gate
(per ADR-0025 §"Implementation status").

**ACID** — Atomicity / Consistency / Isolation / Durability. The
exhaustive-axiom precedent (Haerder & Reuter 1983) reel mirrors
with its 3 invariants. reel claims to be "the ACID of agent side
effects".

**Atomicity** (commit, abort) — Derived property D-3: commit and
abort are single critical sections at v0.1; no partial state is
observable. Derived from single-threaded kernel (ADR-0008).

**Atomix** — arXiv 2602.14849 (Mohammadi et al. 2026). A runtime
for transactional tool use in LLM agents. Atomix's two-axis effect
taxonomy (reversibility × bufferability) is the protocol-layer
companion reel cites at adapter layer. Complementary to reel: Atomix
adds multi-agent frontiers; reel adds content-addressed effect
buffers.

## B

**BLAKE3** — Cryptographic hash function used to compute `Hash` from
Block content. See ADR-0004.

**Block** — A core type. An immutable content unit:
`{ hash: Hash, data: Bytes, provenance: Provenance }`. Identity is a
function of content (`hash = BLAKE3(data + provenance)`). Blocks
have no class; classes are properties of *effect descriptors*
within Block payloads. See spec.md §5.2.

**Buffer** — Informal term for `View.effects_hash`. The set of
pending Class B and Class C effects awaiting commit, materialised
as a content-addressed Block.

## C

**Capability** — A core type. A scoped attenuating authority:
`{ path_prefix: String, op_set: Set<OpKind>, ttl: u64 }`. Derived
Capabilities MUST be sub-lattices of their delegator (I-003
narrowing). Constructed only by the kernel; held only by Views.
See spec.md §5.5; Miller-Yee-Shapiro 2003.

**Class A** — Pure local effect. Operates on a View's Delta or on
transient computation. Reversed by Delta discard at abort. Does NOT
enter `View.effects_hash`. See spec.md §5.7.1.

**Class B** — Idempotent remote effect. Carries a stable version
reference (ETag, commit SHA, object version ID) for commit-time
precondition checking. Two Class B effects with the same version
reference are equivalent under idempotency. See spec.md §5.7.2.
(The 2026-05-15 morning reframe corrected a prior misreading that
"same key forbidden".)

**Class C** — Irreversible remote effect. Buffered in
`View.effects_hash` until commit; fired at commit; cannot be
undone by any further action of reel. See spec.md §5.7.3.

**Commit** — The protocol verb (§6.2) that atomically transitions a
View from `active` to `committed`, validates the delta and Class B
preconditions, fires Class B then Class C effects, moves the
pending-effects Block reference into the committed-effects log, and
returns a new Ref.

**Committed** — A terminal View state (post-`commit`). The View
MUST NOT accept further verbs.

**Compensation** — Per-adapter action to mitigate a fired effect's
consequences (post-commit). Best-effort; not protocol-guaranteed.
Saga-style (Garcia-Molina & Salem 1987). For Class B refundable
purchases or Class C cancellation emails. See
adapter-interface.md §4.

**Conformance** — The property that an implementation correctly
realises the reel protocol per spec.md + invariants.yaml. See
[`conformance.md`](conformance.md) for mandatory tests.

**Content-addressed** — Identified by the hash of content rather
than by a separate name. Blocks are content-addressed via `Hash`;
the pending-effects buffer is content-addressed via
`View.effects_hash`. Inherited from IPFS Merkle-DAG.

## D

**Delta** — A core type. Ordered mutations from a base Block:
`{ base_hash: Hash, ops: Vec<Op> }`. A View carries one Delta layer
above its inherited namespace. See spec.md §5.4.

**Derived property (D-N)** — A structural consequence of the 6 core
types + 3 verbs that holds without being an axiom. D-1 (Class
immutability), D-2 (B+C exit only via commit), D-3 (Atomicity), D-4
(Sibling isolation). See spec.md §7.2.

**Drift** — One of the three reel product narratives: environment
time machine. Audience: every developer. Pitch: `reel save` then
break things; `reel restore` to revert. See README.md.

**Dryrun** — One of the three reel product narratives: agent
sandbox. Audience: developers writing agents. Most likely first
commercial win. Pitch: test agents against production data, zero
side effects.

**Duvet** — AWS labs requirement-tracking tool; tracks every
RFC-2119 keyword in `spec.md` to its Citations and Tests in source
code.

## E

**Effect** — A Block whose payload satisfies an effect schema.
Classified as Class A, B, or C (§5.7). Not a separate core type:
Effect is a *classification over Block payloads*. See spec.md §5.7.

**Effects Hash** — `View.effects_hash`, the optional Hash pointing
to the pending-effects Block. `None` means no pending effects.
`Some(h)` means the Block at `h` is the current pending buffer.

## F

**Fork** — The protocol verb (§6.1) that creates a new View whose
parent is the given View. The new View inherits the parent's
namespace (COW), has an empty Delta, an empty effects_hash, an
optional attenuation of the parent's capability set, and `status =
active`. O(1) in parent payload size (I-003-perf).

## H

**Hash** — A core type (technically a derived identity constructor).
A BLAKE3-256 32-byte digest. Serves as `Block` identity.
`Hash(content) → Block` is the content-addressing function. See
spec.md §5.1.

## I

**I-001 Reachability** — Core invariant: a Block is reachable iff
it is reachable through the transitive closure of Ref→Block and
Block→Block edges from the current namespace. Knowing a Hash does
NOT grant access. See spec.md §7.1.

**I-002 Abort silences Class C** — Core invariant: an aborted
View's Class C effects MUST NOT have been fired and MUST NOT fire
later. reel's reason to exist. See spec.md §7.1.

**I-003 Capability narrowing** — Core invariant: every derived
Capability is a sub-lattice of its delegator; amplification is
forbidden. See spec.md §7.1; Miller-Yee-Shapiro 2003; APS IETF.

**Invariant** — A property that holds in every reachable protocol
state, identified by `I-NNN`. ADR-0025 defines exactly three core
invariants (I-001 / I-002 / I-003); previous numbering (I-004..
I-008, Φ₁..Φ₅) is either superseded (Φ → derived D-N) or demoted to
convenience tier (I-008 fork depth bound).

## K

**Kernel** — The reel runtime root object. Implements `fork`,
`commit`, `abort` and effect-submission methods. Single-threaded at
v0.1 (ADR-0008). See 04-commit-pipeline design.

## L

**Linear capability** — A capability that cannot be duplicated.
CHERI uses linear capabilities for memory isolation. reel's
`Capability` is *attenuating* not strictly linear; the kernel
typically uses a linear marker token inside the commit path to gate
Class C firing. See Class C discipline §8.3.

## M


**MVP** — Minimum viable product per the requirements: core
Rust kernel, CLI, adapters/fs and adapters/slack, conformance
tests. Formal verification (Verus, Apalache) is explicitly NOT in
MVP scope. See ADR-0025 §"Implementation status".

## N

**Namespace** — `View.namespace`, the map `Name → Hash` of Refs
visible from inside the View. Inherited from parent via COW; the
View's Delta overlays.

## O

**Object-capability (ocap)** — The capability-based security model
formalised by Miller-Yee-Shapiro 2003. reel's `Capability` is a
reel-specific subset (narrow + path_prefix + ttl) of the ocap model.
See Miller 2003.

## P

**Path-dependent capability** — Capabilities tracked via lifetime /
flow types per arXiv 2510.08889 (POPL'26). reel's MVP does not use
these; a future formal capability layer might.

**Phase 1 / Phase 2** — Validation / Drain phases of commit. See
spec.md §6.2.

**Provenance** — `Block.provenance`, recording the Block's origin
(kernel version, creation time, optional signing key). May be part
of the Block's hash.

## Q

**Quint** — Engineer-friendly language for state-machine
specifications; compiles to TLA+. `spec/formal/reel.qnt` is reel's
advisory formal model.

**quint-connect** — Rust crate (Informal Systems) for model-based
testing against a Quint spec. Used in
`crates/reel-conformance-runner` (post-MVP).

## R

**Reachability (I-001)** — See I-001.

**Ref** — A core type. A mutable named pointer:
`{ name: String, hash: Hash, version_constraint: Option<VersionRef> }`.
Updated atomically by commit. See spec.md §5.3.

**Refinement** — A relationship between two state machines where
one (the "concrete") simulates the other (the "abstract"). reel's
spec layer is the abstract; the Rust kernel is the concrete. See
`spec/refinement.md` (advisory).

**Replay** — One of the three reel product narratives: collaborative
agent debugging via shareable snapshots.

## S

**Saga** — A pattern (Garcia-Molina & Salem 1987) of sequencing
local transactions with compensating actions for failure recovery.
reel's Class B/C handling shares conceptual ancestry; reel's choice
to *buffer* Class C until commit is more conservative than Saga's
"fire then compensate".

**Schema version** — A frozen specification snapshot under
`spec/schema/<YYYY-Qn>/`. Current: 2026-Q3.

**Snapshot** — A verifiable reproduction descriptor for a View:
`{ namespace_config, delta_layer_bytes, effects_hash, version_refs,
capability_set }`. NOT a complete copy of the world; sufficient to
*reproduce* the View on a compatible kernel. See spec.md §5.8.

**Snapshot Isolation (SI)** — A multiversion concurrency control
model (Berenson et al. SIGMOD 1995). reel's View ≈ SI transaction:
reads from a fixed snapshot, writes deferred to commit.

**Store** — The on-disk persistence layer for Blocks and Refs.

**submit_a / submit_b / submit_c** — Kernel API methods for
submitting Class A / B / C effects. Class A applies directly to the
Delta; Class B and C buffer into the pending-effects Block.

## T

**Terminal state** — One of `committed` or `aborted`. A View in a
terminal state MUST NOT accept further verbs.

**TTL** — Time-to-live. A Capability field constraining how long
the capability is valid (0 = unbounded; positive integer = seconds
until expiry).

## V

**Verus** — SMT-based verification tool for Rust. Post-MVP; used to
prove that the Rust kernel refines the formal spec. See
`spec/refinement.md`.

**Version Ref / VersionRef** — A Class B effect's stable identifier
on a remote system: ETag, commit SHA, object version ID, or
equivalent. Used for the commit-time precondition check.

**View** — A core type. A workspace closure:
`{ namespace, delta, caps, effects_hash, status, parent }`. See
spec.md §5.6. Compare Snapshot Isolation transaction (Berenson
1995). Three states: active / committed / aborted.

## W

**Witness** — A reachable state in the formal model where an
invariant is non-vacuously checked. Each named invariant has a
witness run in `spec/formal/reel.qnt`. See
the design notes.

**Anti-witness** — A mutation of the formal model that triggers an
invariant violation. Demonstrates the invariant has discriminating
power against a specific fault class.

## ✗ Removed / superseded

- **BlockId** — replaced by `Hash`. The name `BlockId` was used in
  earlier iterations; current canon is `Hash`.
- **parent_opt** — the Block parent-hash field. Removed from the
  protocol surface; if needed by an adapter, it lives in
  `Block.provenance` or in the Delta `ops`.
- **EffectKey** / **IdempotencyCollision** — superseded by Class B
  `version_ref` semantics (precondition check, not uniqueness).
- **Φ₁ / Φ₂ / Φ₄ / Φ₅** — superseded by D-1 / D-2 / D-3 / D-4 as
  derived properties (not separate axioms).
- **Draft** — replaced by `Active` as the View's non-terminal state
  name (per the requirements + ADR-0025).
