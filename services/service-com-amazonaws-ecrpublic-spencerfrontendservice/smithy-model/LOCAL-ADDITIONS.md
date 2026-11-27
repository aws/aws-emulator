# Local additions to the Smithy model snapshot

`com-amazonaws-ecrpublic-spencerfrontendservice.json` in this directory is a
snapshot of the published ECR Public model and is kept EXACTLY as shipped: it is
a third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so
an edit there would be invisible to a reader of the crate's source and lost on
the next refresh.

AWS answers some operations with members that snapshot does not declare. Each is
declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`).

## `ImageDetail$repositoryType`

- **What AWS does**: every `ImageDetail` entry of a `DescribeImages` response
  carries `repositoryType`, the registry tier of the public repository the image
  sits in — recorded in the `simparity-wire-aws-v1` wire fixtures
  `api-repository-deletion-and-non-empty-error-paths` (step `task#10`) and
  `ecr-public-image-push-idempotent-replay-and-layer-reupload` (step `task#10`),
  where a repository created through `CreateRepository` in the account's default
  public registry reports `FREE`. The value is a property of the repository, not
  of the manifest or of the request, so it is the same on every entry of one
  repository's page.
- **What the snapshot declares**: `com.amazonaws.ecrpublic#ImageDetail` with
  eight members, none of them `repositoryType`, and no `RepositoryType` shape at
  all.
- **Where it is declared**: `src/ops/types/image_detail_types.rs`, whose
  `ImageDetail` is the single type `DescribeImages` pages its entries through.
  It is generated from `local-additions.json`, which restates the eight modelled
  members — with their own targets, so every length and pattern constraint is
  captured exactly as the snapshot spells it — and adds the ninth. Which tier
  this registry reports is `crate::state::DEFAULT_REPOSITORY_TYPE`.
- **What else is in the overlay**: only the shapes those nine members target,
  and only as much of each as codegen reads — the scalars their type and
  constraint traits, the list its member target, the added enum its value. The
  member is output-only, so the enum states the one value AWS was recorded
  reporting rather than a published value set.

`tests/all/image_detail_repository_type.rs` pins every half of this: the member
on the wire, over every way a caller can select entries; the snapshot's own
member list, which the local declaration must cover in full and which must still
not declare `repositoryType`; each shape the overlay restates, compared
declaration for declaration against the snapshot (type, members, enum values and
constraint traits, documentation aside); and the value catalogue
`REPOSITORY_TYPE_VALUES` against the overlay's own enum. If the snapshot is ever
refreshed with `repositoryType`, with a member the overlay does not restate, or
with a changed target, value set or constraint, that suite fails — and the local
declaration should then give way to the modelled shape.
