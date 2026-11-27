# Local additions to the Smithy model snapshot

`com-amazonaws-ecs-amazonec2containerservicev20141113.json` in this directory is
a snapshot of the published Amazon ECS model and is kept EXACTLY as shipped: it
is a third-party artefact consumed at build time by `#[smithy_macros::smithy]`,
refreshed wholesale by the next model update, so an edit there would be invisible
to a reader of the crate's source and lost on that refresh. The same holds for
the files rendered from it (`src/ops/types/*_types.rs` marked `@generated`): a
declaration that has to differ from the snapshot's lives in this overlay and in a
hand-written module beside the generated ones, which the refresh neither
overwrites nor deletes.

`local-additions.json` is that overlay. Each declaration generated from it is
generated the same way every modelled shape is — one member list, one set of
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-redshift-redshiftserviceversion20121201`
(`smithy-model/local-additions.json` + `src/ops/types/cluster_types.rs`) and by
`services/service-com-amazonaws-workspaces-workspacesservice`.

Two things make a declaration differ from the snapshot's, and nothing else is in
the overlay.

## 1. Members AWS answers with that the snapshot does not declare

Recorded behavior: the `simparity-wire-aws-v1` wire fixtures of the service's own
reads and writes carry them, and the snapshot declares neither the member nor a
shape of that name.

| Shape | Members added | Where they are declared | Where the values come from |
|---|---|---|---|
| `Service` | `version`, `runStatus`, `extendDeploymentTimePeriod` | `src/ops/types/service_report_types.rs` | `crate::state`'s one service projection, so a create, an update and a read cannot disagree |
| `Deployment` | `healthyTaskCount`, `failedLaunchTaskCount`, `replacedTaskCount`, `totalFailedTasks`, `scale`, `stabilityStatus` | `src/ops/types/service_report_types.rs` | the same projection: every counter is read off the task records the deployment holds |
| `Task` | `provisionedCpu`, `provisionedMemory` | `src/ops/types/task_report_types.rs` | `TaskRecord::provisioned_size`, read off the task's own reported size |
| `TaskDefinition` | `previousStatus` | `src/ops/types/task_definition_report_types.rs` | `TaskDefinitionRecord::transition_view`, the status the revision held when the call arrived |
| `ManagedScaling` | `scalingApproach` | `src/ops/types/managed_scaling_types.rs` | `crate::state::stored_managed_scaling`, the one rule every path that stores a provider runs |

Each is an open `string`, an `integer` with the `@default 0` its neighbours
carry, or a structure the snapshot does declare (`Scale`), because nothing
publishes a value set or a shape for them: whatever the service computes is what
it reports.

## 2. A `@required` member the snapshot also gives a `@default`

`DeploymentCircuitBreaker$enable`/`$rollback`, `ThresholdConfiguration$value`,
`DeploymentAlarms$enable`/`$rollback` and
`DeploymentEarlySuccessCriteria$enable` are each `@required` AND `@default`.
Amazon ECS serves a request that omits one and fills the default in — a create
naming `enable` and nothing else is accepted and the structure reads back whole —
while the generated deserializer refuses a `@required` member whatever default it
declares. So the overlay restates those four structures with the `@default` and
WITHOUT `@required`, which is exactly the Smithy reading of a defaulted member,
and keeps `@required` on the members the snapshot gives no default
(`ThresholdConfiguration$type`, `DeploymentAlarms$alarmNames`), so a request that
omits one of those is still refused. They are declared in
`src/ops/types/deployment_configuration_types.rs`.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen reads:
the scalars their type and their `@range`, the enums their values (that is what
validation enforces), the lists their element target. A STRUCTURE target is
resolved through the Rust field's type rather than through the model, so the
overlay declares the shape without restating its members; the modelled type in
`shared_types.rs` is what that member reads and writes.

## What is NOT in the overlay

A shape whose declaration differs from the snapshot's only in a codegen
ATTRIBUTE, not in the model: `InstanceLaunchTemplate` is declared in
`src/ops/types/instance_launch_template_types.rs` from the snapshot itself, with
`skip_validation = ["monitoring"]`, for the reason that module states.

## The test that keeps this honest

`tests/all/local_model_additions.rs` pins both halves against the snapshot:
every member the snapshot declares must be restated with the same target and the
same `@default`, and `@required` exactly where the snapshot marks it required and
gives it no default; the members each shape ADDS must be exactly the recorded
ones above; and every shape the overlay restates must still carry the snapshot's
type, value set and constraints. A refreshed snapshot that declares one of those
members, adds a member the overlay does not restate, or changes a target, value
set or constraint fails there — and the local declaration should then give way to
the modelled shape.
