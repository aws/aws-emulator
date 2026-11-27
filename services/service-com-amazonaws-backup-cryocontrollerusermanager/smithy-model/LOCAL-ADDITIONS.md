# Local additions to the Smithy model snapshot

`com-amazonaws-backup-cryocontrollerusermanager.json` in this directory is a
snapshot of the published Backup model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, and it
is rewritten wholesale by the next model refresh, so an edit there would be lost.
The files rendered from it under `src/ops/types/` are rewritten by that refresh
too — which is why the member below is declared from a model file of this crate's
own rather than by editing a rendered one.

AWS answers a read of a report plan with one member that snapshot does not
declare. It is declared in this crate's own source instead, from a small overlay
model beside the snapshot (`local-additions.json`), so the declaration is
generated the same way every modelled shape is — one member list, one set of
constraint traits, no hand-written schema or serializer. The same mechanism is
used by `services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`) and by
`services/service-com-amazonaws-redshift-redshiftserviceversion20121201`.

## `ReportTiming` on every report plan Backup reports

- **What AWS does**: a read of a report plan reports the TIMING of the plan's
  report — how many hours apart two runs of it are (`ReportFrequencyHours`) and
  the hour of the day a run starts at (`ReportStartHour`) — in the member
  `ReportTiming` of the `ReportPlan` body.
- **What the snapshot declares**: `ReportPlan` with the nine members
  `ReportPlanArn`, `ReportPlanName`, `ReportPlanDescription`, `ReportSetting`,
  `ReportDeliveryChannel`, `DeploymentStatus`, `CreationTime`,
  `LastAttemptedExecutionTime` and `LastSuccessfulExecutionTime`, none of them
  `ReportTiming`, and no shape of that name at all.
- **Where it is declared**: `src/ops/types/report_plan_wire.rs`, which holds the
  ONE `ReportPlan` type of this crate — the body `DescribeReportPlan` answers
  with and the entries `ListReportPlans` pages. It is generated from
  `local-additions.json`, which restates the nine members the snapshot declares —
  with their own targets, so every length and pattern constraint is captured
  exactly as the snapshot spells it — and adds `ReportTiming`, a structure of two
  integers. `ReportPlan` is therefore NOT declared in the rendered
  `src/ops/types/shared_types.rs`, which says so where the shape would sit.
- **What else is in the overlay**: only the shapes those members target, and only
  as much of each as codegen reads — the strings their length and pattern, the
  timestamps and the integers their type, and the two structures the plan nests
  (`ReportSetting`, `ReportDeliveryChannel`) their type alone, since their own
  rendered types carry their member lists.
- **The value**: no create of a plan states a timing (`CreateReportPlan` carries
  `ReportPlanName`, `ReportPlanDescription`, `ReportDeliveryChannel`,
  `ReportSetting`, `ReportPlanTags` and `IdempotencyToken`), so a stored plan
  carries this service's own schedule (`crate::state::stored_report_timing`) and
  every read reports it from the plan.

`tests/all/report_plan_local_members.rs` pins both halves: the member on the wire,
over a set of plans and through every operation that reports one; and the overlay
against the snapshot, shape by shape (type, member targets and constraint traits,
documentation aside). If the snapshot is ever refreshed with `ReportTiming`, with
a member the overlay does not restate, or with a changed target or constraint,
that suite fails — and the local declaration should then give way to the modelled
shape.
