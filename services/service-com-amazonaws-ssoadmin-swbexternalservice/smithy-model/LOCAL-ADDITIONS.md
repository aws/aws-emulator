# Local additions to the Smithy model snapshot

`com-amazonaws-ssoadmin-swbexternalservice.json` in this directory is the
UPSTREAM model of this service and is kept EXACTLY as the model update wrote
it: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers three of this service's reads with a member set WIDER than that
snapshot declares. Those three shapes are declared from a small overlay model
beside the snapshot (`local-additions.json`), so the declaration is generated
the same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-gameliftstreams-gameliftstreams`
(`smithy-model/local-additions.json` +
`src/ops/types/get_stream_group_types/overlay.rs`) and by
`services/service-com-amazonaws-dynamodb-dynamodb-20120810`.

## `InstanceMetadata.State` on every served instance view

- **What AWS does**: every read that reports an Identity Center instance view —
  each `ListInstances` entry, and the `Instance` envelope below — carries a
  `State` structure whose single `Name` is that instance's own lifecycle value,
  beside the `Status` member the snapshot declares.
- **What the snapshot declares**: `InstanceMetadata` with `InstanceArn`,
  `IdentityStoreId`, `IdentityStoreArn`, `OwnerAccountId`, `Name`,
  `CreatedDate`, `Status`, `StatusReason`, `PrimaryRegion` and `Regions`, and
  no member for that structure.
- **Where it is declared**: `src/ops/types/list_instances_types/overlay.rs`,
  generated from `local-additions.json`, which restates the shape with that one
  member against the snapshot's own member targets. What `State` carries is read
  from the one field the instance's status lives in
  (`crate::state::Instance::state`), so the two views of one instance's
  lifecycle cannot disagree.

## `DescribeInstanceResponse.Instance`, the instance envelope

- **What AWS does**: `DescribeInstance` answers with the instance VIEW above as
  an `Instance` member, beside the flat members it reports.
- **What the snapshot declares**: `DescribeInstanceResponse` with those flat
  members only (`InstanceArn` … `PermissionSetsEnabled`), and no envelope.
- **Where it is declared**: `src/ops/types/describe_instance_types/overlay.rs`,
  generated from `local-additions.json`. Both views are projected from the one
  instance record the handler read, so they cannot disagree.

## `DescribeApplicationProviderResponse.ApplicationProvider`

- **What AWS does**: `DescribeApplicationProvider` answers with the whole
  application-provider catalog entry — the `ApplicationProvider` structure
  `ListApplicationProviders` reports — beside the flat members it reports.
- **What the snapshot declares**: `DescribeApplicationProviderResponse` with
  `ApplicationProviderArn`, `FederationProtocol`, `DisplayData` and
  `ResourceServerConfig` only.
- **Where it is declared**:
  `src/ops/types/describe_application_provider_types/overlay.rs`, generated from
  `local-additions.json`, with the modelled members keeping the model's own
  order and the local one stated last. Both the flat members and the entry are
  projected from the one catalog record the handler read.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen
reads. `InstanceState` is the structure the local member carries and the one
shape the overlay ADDS: the snapshot declares it nowhere. `RegionMetadata`,
`EncryptionConfigurationDetails`, `NetworkConfigurationInfo`, `DisplayData`,
`ResourceServerConfig` and `ApplicationProvider` are reached ONLY as structure
members, where codegen takes the member list from the member's own Rust type —
generated from the snapshot, in `types/shared_types.rs`,
`types/describe_instance_types/snapshot.rs` and
`types/list_application_providers_types.rs` — so the overlay declares them as
the structures they are and restates no member of any of them. Every string,
enum, timestamp and list a restated member targets is restated with the
snapshot's own constraint traits, because those are the bounds the framework's
validating walk enforces.

The two models are named from two submodules of each of
`src/ops/types/describe_instance_types.rs`,
`src/ops/types/list_instances_types.rs` and
`src/ops/types/describe_application_provider_types.rs` rather than from one file
each, because `smithy_macros::smithy_model!` defines the local `smithy!` macro
that `#[smithy]` expands into, and that definition covers the whole file it
appears in — nested modules included.

Those three files are therefore HAND-WRITTEN at generated paths: they carry no
`@generated` marker, so a model refresh neither overwrites nor deletes them, and
their `snapshot` halves have to be kept in agreement with a refreshed snapshot
by hand. Everything else under `src/ops/types/` is still rendered from the
model.

`tests/all/listed_instance_state_beside_the_modelled_members.rs` pins both
halves: the overlay against the snapshot shape by shape, and the member set a
served instance view carries against the overlay it is generated from. If the
snapshot is ever refreshed with these members declared, that suite fails and the
local declarations should give way to the modelled shapes.
