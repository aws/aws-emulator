# Local additions to the Smithy model snapshot

`com-amazonaws-taxsettings-taxsettings.json` in this directory is a snapshot of
the published Tax Settings model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh of the snapshot.

AWS answers two of this service's shapes with FEWER members than that snapshot
requires: the snapshot marks `errorCode` `@required`, and the recorded responses
of those two classes carry no `errorCode` at all. Such a shape is declared in
this crate's own source instead, from a small overlay model beside the snapshot
(`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no
hand-written schema and no hand-written serializer. The same mechanism is used by
`services/service-com-amazonaws-pcs-awsparallelcomputingservice`
(`smithy-model/local-additions.json` + `src/ops/types/resource_not_found_wire.rs`),
by `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921` and
by `services/service-com-amazonaws-artifact-artifact`.

## `ResourceNotFoundException.errorCode` and `InternalServerException.errorCode`

- **What AWS does**: `errorCode` on this service's exceptions is a
  SERVICE-DEFINED code — the wire already carries the exception's own shape name
  in `x-amzn-errortype` / `__type` — and this service publishes no code for a
  missing resource: the recorded 404 of `GetTaxRegistration` for an account with
  no stored registration carries `message` alone, with `errorCode` absent from
  the body. The same holds for the 500 class, for which this service publishes no
  code either.
- **What the snapshot declares**: both shapes with `message` and `errorCode`,
  each `@required`, so a generated declaration would make `errorCode` a bare
  `String` that is ALWAYS written — putting an empty `errorCode` on the wire
  where AWS writes none.
- **Where it is declared**: `src/ops/types/exception_wire.rs`, generated from
  `local-additions.json`, which restates both shapes with `message` `@required`
  and `errorCode` optional, each against the snapshot's own target, so every
  length and pattern constraint and the `@sensitive` marking of the message are
  captured exactly as the snapshot spells them. The error class and the HTTP
  status come from the overlay too, so the refusals are still a 404 and a 500 of
  the same shape ids. `src/ops/types/shared_types.rs` — which is rendered from
  the snapshot — therefore declares every other multi-operation shape but not
  these two, and `types/mod.rs` re-exports the local declarations like any other
  shape, so both names resolve unchanged wherever they are named.
- **What the member carries**: nothing, in both classes — see
  `crate::state::no_tax_registration` and `crate::state::state_unavailable`. The
  classes this service DOES publish a code for (`ValidationException`,
  `ConflictException`) are declared from the snapshot and carry it.

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen
reads: `ErrorMessage` and `ErrorCode` with the length, pattern and sensitivity
traits the snapshot spells for them.

`tests/all/missing_resource_errors.rs` and `tests/all/fidelity_rules.rs` pin the
absence of the member on the wire over every operation that answers the
not-found class. If the snapshot is ever refreshed with `errorCode` optional on
these shapes, the local declarations should give way to the modelled ones: delete
`exception_wire.rs` and its overlay entries and let `shared_types.rs` declare the
shapes again.
