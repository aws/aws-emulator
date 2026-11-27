# Local additions to the Smithy model snapshot

`com-amazonaws-pcs-awsparallelcomputingservice.json` in this directory is a
snapshot of the published PCS model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh of the snapshot.

AWS answers one of this service's shapes with a member WIDER than that snapshot
declares: a member the recorded response carries is absent from the snapshot's
member list. Such a shape is declared in this crate's own source instead, from a
small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of constraint traits, no hand-written schema and no hand-written
serializer. The same mechanism is used by
`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`
(`smithy-model/local-additions.json` + `src/ops/types/image_types.rs`), by
`services/service-com-amazonaws-artifact-artifact` and by
`services/service-com-amazonaws-redshift-redshiftserviceversion20121201`.

## `ResourceNotFoundException.resourceArn`

- **What AWS does**: a not-found refusal of a request that addressed its subject
  by ARN — the key the three tagging operations take — reports that ARN back,
  exactly as the request supplied it. It is the one value of such a request the
  refusal carries: a caller tagging many resources in a loop reads it to
  attribute the failure to the call that made it.
- **What the snapshot declares**: `ResourceNotFoundException` with `message`,
  `resourceId` and `resourceType`, each `@required`, and no member for the ARN.
- **Where it is declared**: `src/ops/types/resource_not_found_wire.rs`, generated
  from `local-additions.json`, which restates the shape with that member (an
  `Arn`) beside the three the snapshot declares, each against the snapshot's own
  target and with its own `@required` marking, so every length and pattern
  constraint is captured exactly as the snapshot spells it. The error class and
  the HTTP status come from the overlay too, so the refusal is still a 404 of
  the same shape id. `src/ops/types/shared_types.rs` — which is rendered from the
  snapshot — therefore declares every other multi-operation shape but not this
  one, and `types/mod.rs` re-exports the local declaration like any other, so
  `ResourceNotFoundException` resolves unchanged wherever it is named.
- **What the member carries**: the ARN the refused request addressed the resource
  by (`crate::state::arn_not_found`). The identifier-keyed refusal
  (`crate::state::not_found`) addressed no ARN, so it reports none.

## What else is in the overlay

Only the shape that member list targets, and only as much of it as codegen
reads: `Arn` with the length and pattern traits the snapshot spells for it. The
three modelled members target the Smithy prelude's `String`, which no model file
declares.

`tests/all/refusal_wording.rs` pins the member on the wire over every
ARN-addressed operation, and asserts the identifier-keyed refusal carries none.
If the snapshot is ever refreshed WITH `resourceArn` on
`ResourceNotFoundException`, the local declaration should give way to the
modelled shape: delete `resource_not_found_wire.rs` and its overlay entry and let
`shared_types.rs` declare the shape again.
