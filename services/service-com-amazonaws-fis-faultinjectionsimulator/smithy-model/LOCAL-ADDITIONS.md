# Local additions to the Smithy model snapshot

`com-amazonaws-fis-faultinjectionsimulator.json` in this directory is a snapshot of the
published AWS FIS model and is kept EXACTLY as shipped: it is a third-party artefact
consumed at build time by `#[smithy_macros::smithy]`, so an edit there would be invisible
to a reader of the crate's source and lost on the next refresh of the snapshot.

AWS answers one of this service's shapes with a member WIDER than that snapshot declares: a
member the recorded response carries is absent from the snapshot's member list. Such a shape
is declared in this crate's own source instead, from a small overlay model beside the
snapshot (`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no hand-written schema
and no hand-written serializer. The same mechanism is used by
`services/service-com-amazonaws-pcs-awsparallelcomputingservice`
(`smithy-model/local-additions.json` + `src/ops/types/resource_not_found_wire.rs`) and by
`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`.

## `SafetyLever.accountId`

- **What AWS does**: a safety lever is reported with the Amazon Web Services account it
  belongs to beside its id, its ARN and its state. `GetSafetyLever` and
  `UpdateSafetyLeverState` both report the lever through that one shape, so both carry it.
- **What the snapshot declares**: `SafetyLever` with `id`, `arn` and `state`, and no member
  for the account.
- **Where it is declared**: `src/ops/types/safety_lever_wire.rs`, generated from
  `local-additions.json`, which restates the shape with that member beside the three the
  snapshot declares, each against the snapshot's own target, so every length and pattern
  constraint is captured exactly as the snapshot spells it.
  `src/ops/types/shared_types.rs` — which is rendered from the snapshot — therefore declares
  every other multi-operation shape but not this one, and `types/mod.rs` re-exports the
  local declaration like any other, so `SafetyLever` resolves unchanged wherever it is
  named.
- **What the member carries**: the account this emulator serves
  (`crate::state::SafetyLeverRecord::to_safety_lever`), which is the account whose lever
  these operations report — FIS gives an account exactly one lever.

## What else is in the overlay

Only the shapes that member list targets, and only as much of each as codegen reads:
`SafetyLeverId` and `ResourceArn` with the length and pattern traits the snapshot spells for
them, and `SafetyLeverState` with its two members and their targets. The added member
targets the Smithy prelude's `String`, which no model file declares.

`tests/all/safety_lever_report_matches_the_model.rs` pins the member on the wire over both
operations that report a lever, in several Regions and in every state a lever can be in. If
the snapshot is ever refreshed WITH `accountId` on `SafetyLever`, the local declaration
should give way to the modelled shape: delete `safety_lever_wire.rs` and its overlay entry
and let `shared_types.rs` declare the shape again.
