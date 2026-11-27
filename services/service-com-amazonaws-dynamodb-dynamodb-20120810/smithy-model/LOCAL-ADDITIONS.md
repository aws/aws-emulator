# Local additions to the Smithy model snapshot

`com-amazonaws-dynamodb-dynamodb-20120810.json` in this directory is a snapshot of the
published DynamoDB model and is kept EXACTLY as shipped: it is a third-party artefact
consumed at build time by `#[smithy_macros::smithy]`, so an edit there would be invisible
to a reader of the crate's source and lost on the next refresh.

AWS answers some operations with a member that snapshot does not declare. It is declared
in this crate's own source instead, from a small overlay model beside the snapshot
(`local-additions.json`), so the declaration is generated the same way every modelled
shape is — one member list, one set of constraint traits, no hand-written schema or
serializer. The same mechanism is used by
`services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` + `src/ops/types/workspace_properties_types.rs`).

## `TableThroughputModeSummary` on every table description

- **What AWS does**: every table description a response carries reports the table's
  read/write throughput mode in the member `TableThroughputModeSummary` — an object
  carrying `TableThroughputMode` (the mode in force, `PROVISIONED` or `PAY_PER_REQUEST`)
  and, for a table that has been switched to on-demand, the
  `LastUpdateToPayPerRequestDateTime` of that switch. It is carried by `CreateTable`,
  `DescribeTable`, `UpdateTable` and `DeleteTable` alike, beside `BillingModeSummary`,
  which a table that has never been on-demand does not carry at all.
- **What the snapshot declares**: `TableDescription` with the members
  `AttributeDefinitions` … `VectorIndexes`, none of them
  `TableThroughputModeSummary`, and no shape of that name at all.
- **Where it is declared**: `src/ops/types/table_description_types.rs`, which holds the one
  `TableDescription` type of this crate — the shape every table operation answers with. It
  is generated from `local-additions.json`, which restates the members the snapshot
  declares — with their own targets, so every length, pattern and enum value is captured
  exactly as the snapshot spells it — and adds `TableThroughputModeSummary`. Its mode is
  the `BillingMode` enum the snapshot declares and its instant the `Date` timestamp, since
  they are the mode and the switch instant the table already stores
  (`crate::state::Table::describe`), so the two summaries cannot disagree about one table.
- **What else is in the overlay**: only the shapes those members target, and only as much
  of each as codegen reads — the enums carry their values, the strings their length and
  pattern, the timestamps their type, and a member targeting a structure declared from the
  snapshot carries that structure's id alone.

The suites `tests/all/table_throughput_mode_and_warm_capacity.rs` and
`tests/all/table_description_members.rs` pin the member on the wire, over every operation
that reports a table. If the snapshot is ever refreshed with `TableThroughputModeSummary`,
the local declaration should give way to the modelled shape — and the shape should go back
to `shared_types.rs`, where a comment records why it left.
