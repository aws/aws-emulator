# Local additions to the Smithy model snapshot

`com-amazonaws-codestarconnections-codestar-connections-20191201.json` in this
directory is a snapshot of the published CodeConnections (CodeStar connections)
model and is kept EXACTLY as shipped: it is a third-party artefact consumed at
build time by `#[smithy_macros::smithy]`, so an edit there would be invisible to
a reader of the crate's source and lost on the next refresh.

AWS answers one of this service's reads with a member set NARROWER than that
snapshot declares: a member the snapshot marks `@required` is absent from the
recorded body. That shape is declared in this crate's own source instead, from a
small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of constraint traits, no hand-written schema or serializer. The same
mechanism is used by `services/service-com-amazonaws-applicationsignals-applicationsignals`
(`smithy-model/local-additions.json` + `src/ops/types/recurrence_rule_types.rs`)
and by `services/service-com-amazonaws-arczonalshift-percdataplane`.

## `GetResourceSyncStatusOutput.LatestSync` on a resource that has never synced

- **What AWS does**: `GetResourceSyncStatus` for a `(SyncType, ResourceName)`
  pair the account has never synced is answered HTTP 200 with NONE of
  `DesiredState`, `LatestSync` and `LatestSuccessfulSync` present. It is not a
  refusal: a client polling for sync status before — or without — configuring
  the resource has to be able to tell "never synced" from "bad request", so
  neither `ResourceNotFoundException` nor `InvalidInputException` is the answer
  to "no sync state", and no revision, commit sha or event list is synthesised
  for a sync nothing ran.
- **What the snapshot declares**: `GetResourceSyncStatusOutput` with
  `DesiredState` (target `Revision`) and `LatestSuccessfulSync` (target
  `ResourceSyncAttempt`) optional, and `LatestSync` (target
  `ResourceSyncAttempt`) marked `@required`.
- **Where it is declared**: `src/ops/types/get_resource_sync_status_types/overlay.rs`,
  generated from `local-additions.json`, which restates the shape with that
  member optional and against the snapshot's own member targets. What the
  members CARRY is the one `crate::state::StoredResourceSyncState` row the
  service holds for the pair.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen
reads: `Revision` and `ResourceSyncAttempt` are reached ONLY as structure
members, where codegen takes the member list from the member's own Rust type —
generated from the snapshot, in
`src/ops/types/get_resource_sync_status_types/snapshot.rs` — so the overlay
declares them as the structures they are and restates no member of either.

The two models are named from two submodules of
`src/ops/types/get_resource_sync_status_types.rs` rather than from one file
because `smithy_macros::smithy_model!` defines the local `smithy!` macro that
`#[smithy]` expands into, and that definition covers the whole file it appears
in — nested modules included.

`tests/all/resource_sync_status.rs` pins both halves on the wire: the member set
of a never-synced read and of a recorded one, each over a set of inputs, and the
overlay against the snapshot shape by shape. If the snapshot is ever refreshed
without that `@required` trait, the local declaration should give way to the
modelled shape.
