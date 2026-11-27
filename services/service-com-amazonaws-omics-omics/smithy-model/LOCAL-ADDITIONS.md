# Local additions to the Smithy model snapshot

`com-amazonaws-omics-omics.json` in this directory is a snapshot of the published
Amazon Web Services HealthOmics model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, and it
is rewritten wholesale by the next refresh, so an edit there would be invisible to
a reader of the crate's source and lost on that refresh.

AWS answers one of this service's shapes with a member WIDER than that snapshot
declares: a member the recorded response carries is absent from the snapshot's
member list. Such a shape is declared in this crate's own source instead, from a
small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of constraint traits, no hand-written schema and no hand-written
serializer. The same mechanism is used by
`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`
(`smithy-model/local-additions.json` + `src/ops/types/image_types.rs`), by
`services/service-com-amazonaws-backup-cryocontrollerusermanager` and by
`services/service-com-amazonaws-redshift-redshiftserviceversion20121201`.

## `FileInformation.md5`

- **What AWS does**: every read of a read set's files reports, beside each file's
  size and part layout, the MD5 digest of that file as the service ingested it.
  `GetReadSetMetadata` reports `files.source1`, `files.source2` and `files.index`
  through `FileInformation`, and `GetReferenceMetadata` reports `files.source` and
  `files.index` through the same shape. A client that uploads a file and later
  reads the read set back uses the digest to confirm that what it reads is what it
  uploaded.
- **What the snapshot declares**: `FileInformation` with `totalParts`, `partSize`,
  `contentLength` and `s3Access`, and no member for the file's digest.
- **Where it is declared**: `src/ops/types/read_set_file_information.rs`, generated
  from `local-additions.json`, which restates `FileInformation` with that member (a
  string) after the four the snapshot declares, each against the snapshot's own
  target and range, so every constraint is captured exactly as the snapshot spells
  it. `src/ops/types/shared_types.rs` — which is rendered from the snapshot —
  therefore declares every other multi-operation shape but not this one, and
  `types/mod.rs` re-exports the local declaration like any other, so
  `FileInformation` resolves unchanged wherever it is named.
- **What the member carries**: the digest of the bytes the file was ingested with
  (`crate::state::content_md5`, `crate::state::ReadSetSource::md5`), so a read
  reports the digest of what this emulator actually stored for that file.

## What else is in the overlay

Only the shapes that member list targets, and only as much of each as codegen
reads: `Md5` as the string the snapshot spells it as, with its length and pattern;
the two ranges the snapshot puts on the integer members, against the prelude
`Integer` and `Long` they target; and `ReadSetS3Access` — reached only as a nested
structure, where codegen takes the member list from the nested shape's own Rust
type, generated from the snapshot in `src/ops/types/shared_types.rs` — declared as
the structure it is with no member of it restated.

`tests/all/read_set_upload_parts.rs` pins the member on the wire, over a set of
uploaded files. If the snapshot is ever refreshed WITH `md5` on
`FileInformation`, the local declaration should give way to the modelled shape:
delete `read_set_file_information.rs` and its overlay entry and let
`shared_types.rs` declare the shape again.
