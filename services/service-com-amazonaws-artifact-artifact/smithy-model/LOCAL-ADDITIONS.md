# Local additions to the Smithy model snapshot

`com-amazonaws-artifact-artifact.json` in this directory is the upstream AWS
Artifact model, kept EXACTLY as the model update wrote it: it is a third-party
artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit there
would be invisible to a reader of the crate's source and lost on the next
refresh.

AWS answers the report reads with one member that snapshot does not declare. It
is declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`).

## `ReportSummary$visibility` and `ReportDetail$visibility`

- **What AWS does**: every entry `ListReports` and `ListReportVersions` project,
  and the `reportDetails` of `GetReportMetadata`, carry `visibility` — `PUBLIC`
  for the reports AWS publishes in the Artifact catalog.
- **What the snapshot declares**: `com.amazonaws.artifact#ReportSummary` with
  fifteen members and `com.amazonaws.artifact#ReportDetail` with twenty, neither
  of them `visibility`, and no `Visibility` shape at all.
- **Where it is declared**: `src/ops/types/report_catalog_types.rs`, whose
  `ReportSummary` and `ReportDetail` are the single types every report read of
  this crate projects a stored catalog record through. They are generated from
  `local-additions.json`, which restates the modelled members — with their own
  targets, so every length, pattern, range and enum constraint is captured
  exactly as the snapshot spells it — and adds the one the snapshot lacks. An
  open `string`, because nothing publishes the value set: the value reported is
  the one the stored catalog record holds.
- **What else is in the overlay**: only the shapes those members target, and
  only as much of each as codegen reads — the enums carry their values (that is
  what validation enforces), the scalars their type and their constraint traits.

`tests/all/report_catalog_local_members.rs` pins both halves: the member on the
wire for every report read that projects it, and the snapshot's own member list,
which the local declaration must cover in full. If the snapshot is ever
refreshed with `visibility`, or with a member the overlay does not restate, that
suite fails — and the local declaration should then give way to the modelled
shape.
