# Local additions to the Smithy model snapshot

`com-amazonaws-swf-simpleworkflowservice.json` in this directory is a snapshot of the
published SWF model and is kept EXACTLY as the upstream update wrote it: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit
there would be invisible to a reader of the crate's source and lost on the next refresh of
the snapshot.

AWS answers six of this service's shapes WIDER than that snapshot declares: a member the
recorded response carries is absent from the snapshot's member list. Such a shape is
declared in this crate's own source instead, from a small overlay model beside the
snapshot (`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no hand-written schema
and no hand-written serializer. The same mechanism is used by
`services/service-com-amazonaws-pcs-awsparallelcomputingservice`
(`smithy-model/local-additions.json` + `src/ops/types/resource_not_found_wire.rs`), by
`services/service-com-amazonaws-gameliftstreams-gameliftstreams` and by
`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`.

Each of those shapes is declared in a module named `overlay` beside a module named
`snapshot` that declares the shapes of the same generated path as the snapshot spells
them. Two modules rather than one file because `smithy_macros::smithy_model!` defines the
local `smithy!` macro `#[smithy]` expands into, and that definition covers the whole file
it appears in, so one file cannot name two models. Both module roots
(`src/ops/types/shared_types.rs` and `src/ops/types/start_workflow_execution_types.rs`)
are hand-written at a generated path and therefore left as they are by a model refresh.

## `ResourceTag.__type`

- **What AWS does**: every structure the service returns in an awsJson list carries the
  type discriminator naming its own shape, so each element of `ListTagsForResource`'s
  `tags` reports `ResourceTag`'s shape id beside the stored key and value.
- **What the snapshot declares**: `ResourceTag` with `key` (`@required`) and `value`.
- **Where it is declared**: `src/ops/types/shared_types/overlay.rs`.
- **What the member carries**: fixed text about the shape, written by the operation that
  reports the tag (`src/ops/list_tags_for_resource.rs`); never anything of the tag, the
  resource or the request, and never a discriminator the request itself carried.

## `ActivityTaskStartedEventAttributes.clientToken`, `DecisionTaskStartedEventAttributes.clientToken` and `DecisionTaskStartedEventAttributes.startedOnPreviousTaskCompletion`

- **What AWS does**: the event recording the service handing a task to a worker or a
  decider carries the opaque, service-generated handle on THAT hand-over, and a decision
  task's start also records whether it was started as a continuation of the previous
  task's completion rather than in answer to a poll.
- **What the snapshot declares**: both shapes with `identity` and `scheduledEventId` only.
- **Where they are declared**: `src/ops/types/shared_types/overlay.rs`.
- **What the members carry**: the handle minted for the stored event
  (`crate::state::task_start_client_token`), read back unchanged on every later read of
  that event, and the flag the hand-over itself sets.

## `ActivityTaskScheduledEventAttributes.immediatelyStart`

- **What AWS does**: a scheduled activity task's event records whether the service started
  the task straight away rather than leaving it in its task list for a worker to poll for.
- **What the snapshot declares**: the eleven members of the scheduling decision and no
  member for it.
- **Where it is declared**: `src/ops/types/shared_types/overlay.rs`.
- **What the member carries**: whether the schedule handed the task over at once; a task
  scheduled into a task list reports `false`.

## `WorkflowExecutionInfo.lastClosedEventId`, `.lastClosedTimestamp`, `.reopenGenerationId` and `.reopenable`

- **What AWS does**: the record every reader of a run reports carries the close the run's
  own history records — the `eventId` of the event that closed it and the instant that
  event is stamped at — and the reopening state of the run.
- **What the snapshot declares**: the nine members from `execution` to `cancelRequested`.
- **Where they are declared**: `src/ops/types/shared_types/overlay.rs`.
- **What the members carry**: the closing event this service stored
  (`crate::state::Execution::closing_event`), which a run that is still open
  has none of — the modelled default of `0` and no instant at all — and the first
  generation, with no run reopenable, because no operation this service models reopens a
  closed run.

## `WorkflowExecutionAlreadyStartedFault.workflowExecution`

- **What AWS does**: the fault refusing a start that collides with a run that is still
  open identifies the incumbent, so a caller that collided can describe, signal or
  terminate that run without a separate visibility query.
- **What the snapshot declares**: `WorkflowExecutionAlreadyStartedFault` with `message`.
- **Where it is declared**: `src/ops/types/start_workflow_execution_types/overlay.rs`. The
  error class comes from the overlay too, so the refusal is still a 400 of the same shape
  id.
- **What the member carries**: the requested `workflowId` with the `runId` this service's
  own open-run index holds for it (`src/ops/start_workflow_execution.rs`).

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen reads: the
string, `long`, `boolean`, `timestamp`, `enum` and `list` shapes with the length, default
and value traits the snapshot spells for them, and `ActivityType`, `TaskList`,
`WorkflowExecution` and `WorkflowType` as member-less structures, because codegen reads
nothing of a structure target but its type and its name — each is declared in full from
the snapshot in the `snapshot` module beside the overlay. The added scalars target the
Smithy prelude's `String`, `Boolean` and `Long`, which no model file declares.

`tests/all/workflow_execution_lifecycle/local_model_additions.rs` compares the overlay
with the snapshot declaration for declaration and pins every added member on the wire; a
refreshed snapshot that declares one of them, or that changes a target, a value set or a
constraint of anything the overlay restates, fails there. When the snapshot does declare
one, the local declaration should give way to the modelled member: drop that shape from
`local-additions.json` and let the `snapshot` module declare it again.
