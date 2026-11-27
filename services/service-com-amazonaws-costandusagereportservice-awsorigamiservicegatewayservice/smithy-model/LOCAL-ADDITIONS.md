# Local additions to the Smithy model snapshot

`com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice.json`
in this directory is a snapshot of the published Cost and Usage Report model and
is kept EXACTLY as shipped: it is a third-party artefact consumed at build time
by `#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers one of this service's operations with a member that snapshot does not
declare. It is declared in this crate's own source instead, from a small overlay
model beside the snapshot (`local-additions.json`), so the declaration is
generated the same way every modelled shape is — one member list, one set of
constraint traits, no hand-written schema or serializer. The same mechanism is
used by `services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`).

## `ReportCreationDate` on every report definition this service reports

- **What AWS does**: every report definition a `DescribeReportDefinitions`
  response carries states the date the definition was created, in the member
  `ReportCreationDate`, beside the members the request supplied and the other two
  the service populates itself (`BillingViewArn` and `ReportStatus`). The member
  is on every recorded read-back of a definition; its VALUE is the run's, so what
  is required of it is that it is there and stated in the form this service states
  a report timestamp in.
- **What the snapshot declares**: `ReportDefinition` with the thirteen members
  `ReportName`, `TimeUnit`, `Format`, `Compression`, `AdditionalSchemaElements`,
  `S3Bucket`, `S3Prefix`, `S3Region`, `AdditionalArtifacts`,
  `RefreshClosedReports`, `ReportVersioning`, `BillingViewArn` and
  `ReportStatus`, none of them `ReportCreationDate`, and no shape of that name at
  all.
- **Where it is declared**: `src/ops/types/report_definition_types.rs`, which
  holds the one `ReportDefinition` type of this crate — the input shape of
  `PutReportDefinition` and `ModifyReportDefinition`, and the element of the
  `ReportDefinitionList` that `DescribeReportDefinitions` answers. It is
  generated from `local-additions.json`, which restates the members the snapshot
  declares — with their own targets, so every length, pattern and enum constraint
  is captured exactly as the snapshot spells it — and adds
  `ReportCreationDate`. An open `string`, because nothing publishes a value set
  or a constraint for it; the value itself comes from the stored creation instant
  (`crate::state::StoredReport::wire_definition` through
  `crate::state::report_timestamp`), never from the request.
- **What else is in the overlay**: only the shapes those members target, and only
  as much of each as codegen reads — the enums carry their values, the strings
  their length and pattern, the booleans and lists their type. `ReportStatus` is
  restated for its type alone: codegen resolves a structure member through the
  Rust field's type, which is the modelled shape in `src/ops/types/shared_types.rs`.

`tests/all/report_definition_local_members.rs` pins both halves: the member on
the wire, over a set of definitions driven through the create and the update path
and read back; and the overlay against the snapshot, shape by shape (type,
member targets, enum values and constraint traits, documentation aside). If the
snapshot is ever refreshed with `ReportCreationDate`, with a member the overlay
does not restate, or with a changed target, value set or constraint, that suite
fails — and the local declaration should then give way to the modelled shape.
