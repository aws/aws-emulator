# Local additions to the Smithy model snapshot

`com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json` in this
directory is the upstream Well-Architected Tool model, kept EXACTLY as the model
update wrote it: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers the workload reads with one member that snapshot does not declare. It
is declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-artifact-artifact`
(`smithy-model/local-additions.json` + `src/ops/types/report_catalog_types.rs`).

## `Workload$FTRSupported`

- **What AWS does**: `GetWorkload`, `UpdateWorkload` and the workload a milestone
  froze all carry `FTRSupported` — `false` for a workload that is not supported
  for a Foundational Technical Review, emitted rather than omitted.
- **What the snapshot declares**: `com.amazonaws.wellarchitected#Workload` with
  twenty-eight members, none of them `FTRSupported`, and no shape of that name.
- **Where it is declared**: `src/ops/types/workload_projection_types.rs`, whose
  `Workload` is the single type every workload read of this crate projects a
  stored workload record through. It is generated from `local-additions.json`,
  which restates the modelled members — with their own targets, so every length,
  pattern, range and enum constraint is captured exactly as the snapshot spells
  it — and adds the one the snapshot lacks, as a `boolean`.
- **What else is in the overlay**: only the shapes those members target, and
  only as much of each as codegen reads — the enums carry their values, the
  scalars their type and their constraint traits, the lists and maps their
  member, key and value.
- **How it reaches callers**: `src/ops/types/mod.rs` re-exports this `Workload`
  explicitly, so it takes precedence over the glob that also re-exports the
  snapshot's shape of the same name from `shared_types`.

`tests/all/workload_ftr_support.rs` pins the member on the wire for every read
that projects a workload. If the snapshot is ever refreshed with `FTRSupported`,
or with a member the overlay does not restate, that suite and
`tests/all/workload_projection_members.rs` fail — and the local declaration
should then give way to the modelled shape.
