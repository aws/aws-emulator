# Local additions to the Smithy model snapshot

`com-amazonaws-codebuild-codebuild-20161006.json` in this directory is a snapshot
of the published CodeBuild model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh.

AWS answers some operations with members that snapshot does not declare. Each is
declared in this crate's own source instead, from a small overlay model beside
the snapshot (`local-additions.json`), so the declaration is generated the same
way every modelled shape is — one member list, one set of constraint traits, no
hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-arczonalshift-percdataplane`
(`smithy-model/local-additions.json` + `src/ops/types/zonal_shift_types.rs`) and
by `services/service-com-amazonaws-workspaces-workspacesservice`.

All three members below are declared in `src/ops/types/recorded_members_types.rs`,
generated from `local-additions.json`. That overlay restates the members the
snapshot declares — with their own targets, so every length, enum and required
constraint is captured exactly as the snapshot spells it — and adds the ones AWS
carries. The shapes it declares beyond those three are only the shapes those
members target, and only as much of each as codegen reads: the enums carry their
values, the strings their length, and a structure a member targets is declared as
a structure (its own members are generated from the snapshot, beside the other
modelled shapes).

## `type` on the artifacts of a build record

- **What AWS does**: a build and a batch build report the destination they ran
  with in `artifacts`, whose `type` is that destination's type in the token form
  CodeBuild puts on the wire for an artifact it REPORTS — the configured value
  lowercased (`NO_ARTIFACTS` -> `no_artifacts`).
- **What the snapshot declares**: `BuildArtifacts` with `location`, `sha256sum`,
  `md5sum`, `overrideArtifactName`, `encryptionDisabled`, `artifactIdentifier`
  and `bucketOwnerAccess`, and no `type`. (`ProjectArtifacts`, the CONFIGURED
  destination, does declare one, as the modelled `ArtifactsType` enum.)
- **The value**: derived from the configured destination type
  (`crate::state::wire_artifacts_type`), never written out, so a destination type
  this service has not seen before reports in the same form. An open `string` in
  the overlay, because the token form is published nowhere as a value set.

## `customEntrypoint` on a provisioned build's environment

- **What AWS does**: CodeBuild starts a build container with its own entrypoint —
  the agent that runs the buildspec — rather than the entrypoint of the image, so
  a build that got an environment reports `environment.customEntrypoint`. A build
  whose provisioning never completed never had one and reports none, and neither
  does a project's own configuration.
- **What the snapshot declares**: `ProjectEnvironment` with `type`, `image`,
  `computeType`, `computeConfiguration`, `fleet`, `environmentVariables`,
  `privilegedMode`, `certificate`, `registryCredential`,
  `imagePullCredentialsType`, `dockerServer` and `hostKernel` — and no
  `customEntrypoint`, on this or any other shape.
- **The value**: written when the build's environment is provisioned
  (`crate::state::build_queue`) and cleared for a build that has not been
  provisioned yet (`crate::ops::retry_build`).

## `webhookDeletedStatus` on the delete of a project

- **What AWS does**: a project is connected to its repository's events by the
  webhook record CodeBuild keeps against it, and deleting the project
  disconnects it, so the recorded 200 body of `DeleteProject` states the
  disposition of that webhook.
- **What the snapshot declares**: `DeleteProjectOutput` with no members at all.
- **The value**: computed from the project's webhook state inside the same
  critical section as the removal (`crate::state::webhook_deleted_status`).

`tests/all/local_model_additions.rs` pins both halves: the members on the wire,
over a set of inputs each rule generalises over, and the overlay against the
snapshot, shape by shape (type, traits, members, enum values and constraint
traits, documentation aside). If the snapshot is ever refreshed with one of these
members, with a member the overlay does not restate, or with a changed target,
value set or constraint, that suite fails — and the local declaration should then
give way to the modelled shape.
