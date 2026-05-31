# adapter-fs — reel Class A filesystem adapter

The reference Class A adapter. Maps filesystem operations to
`Delta::Op` entries on a `View` per
ADR-0025
§5.7.1.

Class A ops are pure, local, and reversed by Delta discard at
`abort` — they never enter `View.effects_hash`.

## Surface

- `FsPut { name, hash }` — `#[reel_effects::class_a]` effect; emits
  `Op::Put { name, hash }`.
- `FsRemove { name }` — `#[reel_effects::class_a]` effect; emits
  `Op::Remove { name }`.
- `FsAdapter` — host-side facade. Builds `FsPut` / `FsRemove`
  effects, classifies ops, and (at commit drain) materialises an
  applied `Delta` onto the real filesystem under a Capability-narrowed
  scope.

## I-003 enforcement

`FsAdapter::apply_delta` rejects any op whose target falls outside
`Capability.path_prefix`, or whose op-kind is absent from
`Capability.op_set`, with `E-009 CapabilityViolation`.

## I-001 / spec links

- `spec/spec.md` §5.7.1 — Class A semantics.
- `spec/adapter-interface.md` §3.1 — Class A classification rules.
- `spec/conformance.md` §1.2 `t_002_fork_creates_isolated_view`.

## Status

v0.1 (2026-05-22). MVP-targeted; matches the Class A row of
`spec/adapter-interface.md` §2 (`fire` is host-driven for Class A,
so this crate exposes Op-producing effects + a delta materialiser
rather than the `async fn fire` of Class B/C adapters).
