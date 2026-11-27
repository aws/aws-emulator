# Local additions to the Smithy model snapshot

`com-amazonaws-codeconnections-codeconnections-20231201.json` in this directory is a
snapshot of the published CodeConnections model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit
there would be invisible to a reader of the crate's source and lost on the next refresh.

AWS answers some operations in ways that snapshot does not declare. Each is declared in
this crate's own source instead, from the small overlay model `local-additions.json`
beside the snapshot, so the declaration is generated the same way every modelled shape
is — one member list, one set of constraint traits, no hand-written schema or serializer.
The same mechanism is used by `services/service-com-amazonaws-datasync-fmrsservice`
(`smithy-model/local-additions.json` + `src/ops/types/options_types.rs`) and by
`services/service-com-amazonaws-applicationsignals-applicationsignals`.

## `Connection$OwnershipType`

- **What AWS does**: every connection read answers with `OwnershipType` beside the
  connection's other members — the member that says whether the account reading the
  connection owns it or had it shared in. Recorded in the wire fixtures, where a
  connection created in the calling account reads back with `Owned`.
- **What the snapshot declares**: `com.amazonaws.codeconnections#Connection` with
  `ConnectionName`, `ConnectionArn`, `ProviderType`, `OwnerAccountId`, `ConnectionStatus`,
  `HostArn` and `ProviderEndpoint`, and no `OwnershipType` shape at all.
- **Where it is declared**: `src/ops/types/connection_types.rs`, whose `Connection` is the
  single type every reader of a connection reports through (`GetConnection`,
  `ListConnections`, both rendering `state::StoredConnection::to_modeled`). It is generated
  from `local-additions.json`, which restates the members the snapshot declares — against
  their own targets, so every value set, length and pattern is captured exactly as the
  snapshot spells it — and adds the one AWS answers with. An open `string`, because nothing
  publishes the value set.

## `GetResourceSyncStatusOutput$LatestSync`

- **What AWS does**: `GetResourceSyncStatus` for a `(SyncType, ResourceName)` pair that has
  never been synced SUCCEEDS and answers a payload with none of the sync-state members set
  — no `DesiredState`, no `LatestSync`, no `LatestSuccessfulSync`. Recorded in the wire
  fixtures (`codeconnections.get-resource-sync-status-not-found`).
- **What the snapshot declares**: `LatestSync` `@required`, which predicts a payload that
  always carries a sync attempt. The requirement is a requirement on a resource that HAS
  been synced.
- **Where it is declared**: `src/ops/types/get_resource_sync_status_output_types.rs`,
  generated from `local-additions.json`, which restates the three members against their own
  targets and leaves that one optional. Only a sync the service performed records a state,
  and nothing in this emulator fetches a remote repository's commits, so the members are
  rendered from the stored row and absent when nothing is stored.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen reads: the enums
carry their values (that is what validation enforces), the scalars their type, length and
pattern. A nested structure a restated member targets is named with its type alone — its
own declaration stays in the generated module the snapshot renders.

`tests/all/connection_reads.rs` pins both halves: the members on the wire, through the
connections this service reports and the sync status it answers, and the overlay against
the snapshot it restates — compared declaration for declaration (type, members, enum
values and constraint traits, documentation aside) in
`the_local_additions_overlay_restates_the_snapshot_but_for_what_aws_answers`. If the
snapshot is ever refreshed with `OwnershipType`, without that `@required`, with a member the
overlay does not restate, or with a changed target, value set or constraint, that test fails
— and the local declaration should then give way to the modelled shape.
