# reel-effects-macros

Procedural attribute macros (`#[reel::class_a]`, `#[reel::class_b]`,
`#[reel::class_c]`) that generate the sealed-trait impls required by
[`reel-effects`](../reel-effects).

Users should not depend on this crate directly; re-exports live in
`reel_effects::{class_a, class_b, class_c}`.

See:

- ADR-0003 (effect-class taxonomy)
- ADR-0006 (effect-isolation layers; L1 = sealed traits, L3 = manifest)
- ADR-0025 (canonical 6+3+3 ontology)
- the design notes § 5
- the design notes § 5
