# Local additions to the Smithy model snapshot

`com-amazonaws-ecr-amazonec2containerregistry-v20150921.json` in this directory
is a snapshot of the published Amazon ECR model and is kept EXACTLY as shipped:
it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh of the snapshot.

AWS answers one of this service's shapes with a member WIDER than that snapshot
declares: a member the recorded response carries is absent from the snapshot's
member list. Such a shape is declared in this crate's own source instead, from a
small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of constraint traits, no hand-written schema and no hand-written
serializer. The same mechanism is used by
`services/service-com-amazonaws-applicationsignals-applicationsignals`
(`smithy-model/local-additions.json` +
`src/ops/types/discovered_service_types.rs`), by
`services/service-com-amazonaws-arczonalshift-percdataplane` and by
`services/service-com-amazonaws-redshift-redshiftserviceversion20121201`.

## `Image.imageManifestSize`

- **What AWS does**: every answer that carries an image manifest advertises that
  manifest's byte length beside it. `BatchGetImage` reports
  `images[].imageManifestSize` and `PutImage` reports it on its own `image`,
  carrying the length of the manifest document the answer served. A client that
  streams or verifies a manifest allocates and checks against the advertised
  length instead of trusting an unbounded read.
- **What the snapshot declares**: `Image` with `registryId`, `repositoryName`,
  `imageId`, `imageManifest` and `imageManifestMediaType`, and no member for the
  manifest's size.
- **Where it is declared**: `src/ops/types/image_types.rs`, generated from
  `local-additions.json`, which restates `Image` with that member (a `long`)
  beside the five the snapshot declares, each against the snapshot's own target,
  so every length and pattern constraint is captured exactly as the snapshot
  spells it. `src/ops/types/shared_types.rs` — which is rendered from the
  snapshot — therefore declares every other multi-operation shape but not this
  one, and `types/mod.rs` re-exports the local declaration like any other, so
  `Image` resolves unchanged wherever it is named.
- **What the member carries**: the stored manifest's own byte length
  (`crate::state::manifest_document_size`,
  `crate::state::ImageRecord::manifest_size`), so an answer that serves the
  manifest in another schema advertises the length of the document it actually
  served.

## What else is in the overlay

Only the shapes that member list targets, and only as much of each as codegen
reads: the strings with their length and pattern traits as the snapshot spells
them, and `ImageIdentifier` — reached only as a nested structure, where codegen
takes the member list from the nested shape's own Rust type, generated from the
snapshot in `src/ops/types/shared_types.rs` — declared as the structure it is
with no member of it restated.

`tests/all/image_metadata.rs` and `tests/all/manifest_media_type_negotiation.rs`
pin the member on the wire, over a set of manifests. If the snapshot is ever
refreshed WITH `imageManifestSize` on `Image`, the local declaration should give
way to the modelled shape: delete `image_types.rs` and its overlay entry and let
`shared_types.rs` declare the shape again.
