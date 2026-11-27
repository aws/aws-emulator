# Local additions to the Smithy model snapshot

`com-amazonaws-datasync-fmrsservice.json` in this directory is a snapshot of the
published DataSync model and is kept EXACTLY as shipped: it is a third-party
artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit there
would be invisible to a reader of the crate's source and lost on the next
refresh.

AWS answers some operations with members that snapshot does not declare. Each is
declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`).

## `Options$PreserveNfsAcls`

- **What AWS does**: every task and task execution DataSync describes reports
  `PreserveNfsAcls` alongside the other metadata options of `Options` —
  recorded in the `simparity-wire-aws-v1` wire fixtures, where the
  `DescribeTaskExecution` of a transfer between two Amazon S3 locations carries
  it with the value `NONE`, the value every other metadata option that run could
  not carry is reported with.
- **What the snapshot declares**: `com.amazonaws.datasync#Options` with sixteen
  members, none of them `PreserveNfsAcls`, and no `PreserveNfsAcls` shape at all.
- **Where it is declared**: `src/ops/types/options_types.rs`, whose `Options` is
  the single type every operation of this crate parses, persists and reports
  those options through (`CreateTask`, `UpdateTask`, `StartTaskExecution`,
  `UpdateTaskExecution`, `DescribeTask`, `DescribeTaskExecution`). It is
  generated from `local-additions.json`, which restates the sixteen modelled
  members — with their own targets, so every enum value set and range is captured
  exactly as the snapshot spells it — and adds the seventeenth. An open `string`,
  because nothing publishes the value set: whatever a request carries is what
  this service stores and reports back.
- **What else is in the overlay**: only the shapes those seventeen members
  target, and only as much of each as codegen reads — the enums carry their
  values (that is what validation enforces), the scalars their type and range.

`tests/all/transfer_task_lifecycle.rs` pins both halves: the member on the wire,
through the executions and tasks this service reports (the `Options` cases), and
the overlay against the snapshot it restates — compared declaration for
declaration (type, members, enum values and constraint traits, documentation
aside) in
`the_options_overlay_restates_the_snapshot_but_for_the_member_it_adds`. If the
snapshot is ever refreshed with `PreserveNfsAcls`, with a member the overlay does
not restate, or with a changed target, value set or constraint, that test fails —
and the local declaration should then give way to the modelled shape.

## One declaration per shape, after a snapshot refresh

`src/ops/types/shared_types.rs` is rendered from the snapshot, so it declares no
`Options`: a member list carrying `PreserveNfsAcls` does not validate against the
model that module loads, and two declarations of one shape make the name the
`types` registry re-exports ambiguous. A refresh that re-renders that module with
an `Options` of its own has to have it removed again, leaving
`src/ops/types/options_types.rs` as this crate's single declaration of the shape.
