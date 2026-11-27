# Local additions to the Smithy model snapshot

`com-amazonaws-gameliftstreams-gameliftstreams.json` in this directory is a
snapshot of the published Amazon GameLift Streams model and is kept EXACTLY as
shipped: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers this service's stream-group operations with three members that
snapshot does not declare. They are declared in this crate's own source instead,
from a small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of targets, no hand-written schema or serializer. The same mechanism is
used by `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201`
(`smithy-model/local-additions.json` +
`src/ops/types/get_resource_sync_status_types/overlay.rs`) and by
`services/service-com-amazonaws-workspaces-workspacesservice`.

## `DesiredCapacity`, `ActiveCapacity` and `IdleCapacity` on a stream group

- **What AWS does**: every operation that answers with a stream group —
  `CreateStreamGroup`, `UpdateStreamGroup` and `GetStreamGroup` — reports the
  capacity the group holds for ITSELF at the top level of the body, beside the
  per-location `LocationStates`: `DesiredCapacity`, the capacity its locations
  have most recently requested, `ActiveCapacity`, the capacity the service has
  provisioned across them, and `IdleCapacity`, how much of that provisioned
  capacity is not streaming. Recorded in the `simparity-wire-aws-v1` wire
  fixtures, where a `GetStreamGroup` on a group created with always-on capacity
  carries all three. A client that scales a group polls those members to decide
  when its request converged, so an absent member is a missing key rather than a
  number.
- **What the snapshot declares**: `CreateStreamGroupOutput`,
  `GetStreamGroupOutput` and `UpdateStreamGroupOutput` with the same twelve
  members, none of them a group-level capacity member; capacity is declared only
  per location, on `LocationState`.
- **Where it is declared**: `src/ops/types/create_stream_group_types/overlay.rs`,
  `src/ops/types/get_stream_group_types/overlay.rs` and
  `src/ops/types/update_stream_group_types/overlay.rs`, each generated from
  `local-additions.json`, which restates its output shape against the snapshot's
  own member targets and adds the three. What the members CARRY is the roll-up of
  the group's stored locations (`crate::state::StreamGroup::capacity_summary`) —
  the same projection `LocationStates` is reported from, so no value is invented
  for them.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen reads:
the enums carry their values (that is what validation enforces), the scalars and
lists their type. `DefaultApplication` and `LocationState` are reached ONLY as
structure members, where codegen takes the member list from the member's own Rust
type — generated from the snapshot, in `src/ops/types/shared_types.rs` — so the
overlay declares them as the structures they are and restates no member of
either. The constraint traits of the scalar targets (the ARN and description
lengths and patterns) live on the snapshot's declarations, which are what every
request is validated against on the way in.

Each model is named from its own submodule of
`src/ops/types/<operation>_types.rs` rather than from one file because
`smithy_macros::smithy_model!` defines the local `smithy!` macro that `#[smithy]`
expands into, and that definition covers the whole file it appears in — nested
modules included.

`tests/all/stream_group_capacity_summary.rs` pins the three members on the wire,
over a table of groups that differ in how many locations they hold and in which
capacity setting each is configured through, and
`tests/all/stream_group_local_members.rs` pins the overlay against the snapshot
shape by shape. If the snapshot is ever refreshed with those members, the local
declaration should give way to the modelled ones.
