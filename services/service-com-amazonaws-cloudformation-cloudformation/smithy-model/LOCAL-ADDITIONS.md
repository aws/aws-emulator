# Local additions to the Smithy model snapshot

`com-amazonaws-cloudformation-cloudformation.json` in this directory is a snapshot
of the published CloudFormation model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh.

AWS answers two of this service's reads with one element MORE than that snapshot
declares. Each such shape is declared in this crate's own source instead, from a
small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of constraint traits, no hand-written schema or serializer. The same
mechanism is used by `services/service-com-amazonaws-workspaces-workspacesservice`
and `services/service-com-amazonaws-applicationsignals-applicationsignals`
(`smithy-model/local-additions.json` + a types file at a generated path).

## `DescribeResourceScanOutput.ResourceTypeSummaries`

- **What AWS does**: a read of a resource scan carries a per-resource-type
  breakdown of the rows the scan listed — one summary per type, with the
  `ResourceType` and how many resources of it (`ResourcesScanned`) the scan
  accounts for. The element is present whatever the scan's status, empty while
  the scan has no per-type result to report, so a client iterating the breakdown
  sees the same shape throughout the scan's life.
- **What the snapshot declares**: `DescribeResourceScanOutput` without that
  member, and neither `ResourceTypeSummaries` nor `ResourceTypeSummary`.
- **Where it is declared**: `src/ops/types/describe_resource_scan_types.rs`,
  whose output shape and summary element are generated from
  `local-additions.json`. What the element CARRIES is computed from the same rows
  `ResourceTypes` takes the distinct types of and `ResourcesScanned` totals
  (`crate::state::resource_scans`), so the three cannot disagree.

## `DescribeTypeOutput.SchemaHandlerPermissions`

- **What AWS does**: a read of a registry extension carries the permissions the
  handlers declared in the described version's schema require — the union of
  them, and an empty element rather than no element at all for a record whose
  schema declares none.
- **What the snapshot declares**: `DescribeTypeOutput` without that member, and
  no shape of that name.
- **Where it is declared**: `src/ops/types/describe_type_types.rs`, whose output
  shape is generated from `local-additions.json`. What the element carries is read
  out of the schema the registry holds for the described version
  (`crate::state::resource_schema::schema_handler_permissions`), so the
  permissions and the `Schema` they come from cannot disagree.

## What else is in the overlay

Only the shapes those two output structures target, and only as much of each as
codegen reads: the strings with their length and pattern, the enums with their
values, the lists with their element target and their length. `ScanFilter`,
`LoggingConfig` and `RequiredActivatedType` are reached only as a member or list
element, where codegen takes the member list from the element's own Rust type —
generated from the snapshot, in `src/ops/types/shared_types.rs` and
`src/ops/types/describe_type_types.rs` — so the overlay declares them as the
structures they are and restates no member of either.

Both files are at paths the scaffold generator also writes, and both carry no
`@generated` marker: that marker is what tells the next model refresh a file is
rendered from the snapshot and may be overwritten, so a hand-written declaration
at such a path must not carry it. Each file names the overlay on the ONE
attribute that needs it (`model = "smithy-model/local-additions.json"`), so every
other shape in it is still generated from the snapshot and follows the next
refresh.

`tests/all/registry_and_scan_read_members.rs` pins both elements on the wire. If
the snapshot is ever refreshed with these members declared, the local
declarations should give way to the modelled shapes.
