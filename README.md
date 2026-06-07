# reel

reel is a protocol for the boundary between an AI agent's speculative work and
the systems it does not own.

An agent reasons by attempting things. Some attempts cross that boundary: a
message is posted, a row is written, a payment is taken. Once an action has
crossed, no later decision — including the decision to abandon the line of
reasoning that produced it — can retract it. reel exists to make the boundary
transactional, so that an action the system decides to discard is never
performed in the first place.

## What it is

reel defines six content-addressed types — `Hash`, `Block`, `Ref`, `Delta`,
`Capability`, `View` — three verbs — `fork`, `commit`, `abort` — and three
invariants: reachability, the silencing of discarded irreversible effects, and
capability narrowing.

The load-bearing decision is that a `View`'s pending side-effects are themselves
a content-addressed `Block`. `commit` moves that block into the committed log
and drains it; `abort` drops the reference, after which the block is unreachable
and is collected. No path between the submission of an irreversible effect and
`commit` can fire it, because the capability needed to fire one is constructed
only inside `commit`'s drain. `abort` constructs none, so on the discard path
the effect is, by construction, unable to run.

The verbs, the snapshot-isolation reading of a `View`, the capability model and
the effect taxonomy are each taken from existing work — Gray (1981), Wang &
Zheng (2026), Berenson et al. (1995), Miller, Yee & Shapiro (2003),
Garcia-Molina & Salem (1987). reel's contribution is their composition under a
single content-addressed representation.

## Why it is more than a single tool

State, side-effects and capabilities are represented as the same kind of
content-addressed block. One transactional kernel therefore provides rollback,
audit and sharing for an entire workspace, rather than each being implemented
again for each feature. A system that routes work across many external
resources can rebuild its state-and-effect layer on this one kernel instead of
reasoning about transactionality resource by resource.


## What is built

- **The specification** (`spec/`): the six types, three verbs and three
  invariants; the effect classes; the conformance criteria; the adapter
  interface.
- **A Rust kernel** (`crates/`):
  - `reel-spec` — the protocol types. The namespace is a persistent,
    copy-on-write map, so a fork shares it in constant time.
  - `reel-store` — a content-addressed block-and-ref store, with an in-memory
    backend and a redb-backed persistent one, and a reachability walker for
    collection.
  - `reel-effects` — the sealed effect-class traits and the linear
    fire-capability.
  - `reel-core` — the kernel. `fork` (constant-time copy-on-write inheritance
    with capability narrowing) and `abort` are implemented; `commit` follows.
  - `adapters/`, `reel-cli` — the filesystem and remote adapters and the
    command-line surface, in progress.

The workspace builds on stable Rust and passes its test suite, which includes
property-based tests over the kernel's invariants.

## What is next

- `commit` — precondition validation and the ordered drain of buffered effects.
- The command-line tool — a dry-run loop that runs a process, displays its
  pending effects, and either commits or discards them.
- The effect-interception layer — capturing a process's outbound effects
  through a replaceable backend, so that classifying a new service is the only
  work a new integration requires.

## Building

```
cargo build --workspace
cargo test  --workspace
```

Rust 1.95 (stable), edition 2024.

## Status

Pre-release. The specification is settled; the kernel is under construction
along the path described above. Interfaces may change.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
