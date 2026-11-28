# Local additions to the Smithy model snapshot

`com-amazonaws-s3control-awss3controlservicev20180820.json` in this directory is
a snapshot of the published Amazon S3 Control model and is kept EXACTLY as
shipped: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh of the snapshot.

Where AWS answers one of this service's shapes differently from what that
snapshot declares, the shape is declared in this crate's own source instead, from
a small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of traits, no hand-written schema and no hand-written serializer. The
same mechanism is used by
`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`
(`smithy-model/local-additions.json` + `src/ops/types/image_types.rs`) and by
`services/service-com-amazonaws-codestarconnections-codestar-connections-20191201`.

## `MatchObjectSize` and `MatchObjectAge`, with every bound optional

- **What AWS does**: a Storage Lens group is read back as the document it was
  written with. A filter condition that carried only one of its two bounds reads
  back with only that bound: the member that was not sent is OMITTED rather than
  reported as its `@default`.
- **What the snapshot declares**: `MatchObjectSize$BytesGreaterThan` /
  `$BytesLessThan` and `MatchObjectAge$DaysGreaterThan` / `$DaysLessThan`, each
  targeting a value shape carrying `@default` of 0 — and a member with a
  `@default` is a member the codec always writes. 0 is a value those members'
  own documentation excludes ("the value must be a positive number, greater than
  0"; an upper bound must be "greater than the minimum object size"), so
  reporting it for a bound nobody set states a range the group does not filter
  on, and a client that reads a group, edits one clause and writes it back
  submits it.
- **Where it is declared**: `src/ops/types/storage_lens_group_conditions.rs`,
  generated from `local-additions.json`, which restates both structures — with
  their members in the snapshot's own order, against value shapes of the
  snapshot's own types (`integer` and `long`) — without that trait, so each bound
  is an `Option` the codec writes only when the stored document carries it.
  `src/ops/types/shared_types.rs` — which is rendered from the snapshot —
  therefore declares every other multi-operation shape but not these two, and
  `types/mod.rs` re-exports the local declarations like any other, so both
  resolve unchanged wherever they are named.
- **What the members carry**: exactly what the create or update request carried,
  held on the stored group record and echoed by `GetStorageLensGroup` and
  `ListStorageLensGroups`; nothing is derived here.

`tests/all/storage_lens_group_absent_bounds.rs` pins the rule on the wire, over
the documents a group can be written with. If the snapshot is ever refreshed with
those members no longer defaulted, the local declaration should give way to the
modelled shapes: delete `storage_lens_group_conditions.rs` and its overlay
entries and let `shared_types.rs` declare them again.

## What else is in the overlay

Only the shapes those member lists target: `ObjectAgeValue` (an `integer`) and
`ObjectSizeValue` (a `long`), each as the type the snapshot gives it and without
the `@default` the snapshot puts on it.

A member the WIRE carries that the snapshot does not declare AT ALL is a
different problem, and this crate already answers it its own way: the structures
that report such a member keep the snapshot's member list and append it through
the schema-and-codec macros in `src/ops/types/unmodelled_member_wire.rs`, which
is where `AccessPoint` (`src/ops/types/access_point_wire.rs`) and the outputs of
`GetAccessPoint`, `GetAccessGrant` and `ListAccessGrants` get theirs. This
overlay is for a shape the snapshot DOES declare and declares differently from
what AWS answers.
