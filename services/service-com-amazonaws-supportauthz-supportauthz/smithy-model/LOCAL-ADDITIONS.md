# Local additions to the Smithy model snapshot

`com-amazonaws-supportauthz-supportauthz.json` in this directory is the upstream
AWS SupportAuthZ model, kept EXACTLY as the model update wrote it: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh.

Two of its shapes cannot be rendered from that snapshot alone, so they are
declared from the small overlay model `local-additions.json` beside it. The same
mechanism is used by `services/service-com-amazonaws-artifact-artifact`
(`smithy-model/local-additions.json` + `src/ops/types/report_catalog_types.rs`).

## `ActionSet$allActions` and `ResourceSet$allResourcesInRegion`

- **What the snapshot declares**: `ActionSet` and `ResourceSet`, each a union
  whose "all" member targets the Smithy PRELUDE shape `smithy.api#Unit` — the
  structure with no members, which on the wire is the empty object
  (`{"allActions": {}}`).
- **Why that is not enough**: the Smithy JSON AST does not serialize prelude
  shapes, so `smithy.api#Unit` has no definition in the snapshot. Codegen
  classifies a member from its target's definition and `smithy.api#Unit` is not
  one of the prelude ids it short-circuits, so it can type neither union — which
  is the reason `src/ops/SKIPPED_STUBS.md` records for the four support-permit
  operations being hand-authored.
- **What the overlay carries**: the prelude's own definition of
  `smithy.api#Unit`, plus the two unions and the list and string shapes their
  other member targets, restated with their own targets so every length and
  pattern constraint is captured exactly as the snapshot spells it. Nothing is
  invented and nothing is widened: no member, no target and no constraint differs
  from the snapshot.
- **Where it is read**: `src/ops/types/permit_shapes.rs`, which declares the two
  unions with `model = "smithy-model/local-additions.json"` on the attribute and
  every other shape from the snapshot itself.

`tests/all/local_model_overlay.rs` pins both halves: the overlay's restated
shapes against the snapshot's own definitions — so a refreshed snapshot that
changes a member, a target or a constraint fails rather than drifting — and the
two union forms on the wire, through `CreateSupportPermit` and the reads. If a
snapshot is ever refreshed with `smithy.api#Unit` in it, the overlay should give
way to the snapshot alone.
