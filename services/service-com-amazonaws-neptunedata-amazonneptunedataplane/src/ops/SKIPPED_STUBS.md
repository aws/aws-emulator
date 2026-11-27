# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: DeleteSparqlStatistics — unsupported shape construct: operation 'DeleteSparqlStatistics' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetSparqlStatistics — unsupported shape construct: operation 'GetSparqlStatistics' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetStorageInfo — unsupported shape construct: operation 'GetStorageInfo' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: ListIndexRebuilds — unsupported shape construct: operation 'ListIndexRebuilds' has a Unit/absent input, which the generator does not emit yet

Hand-authored since: `GetEngineStatus` was declined here for the same reason (a
Unit/absent input) and is now hand-written in `ops/get_engine_status.rs`, which
declares its own input type and schema. `GetPropertygraphStatistics` and
`DeletePropertygraphStatistics` were declined for that reason too and are now
hand-written in `ops/get_propertygraph_statistics.rs` and
`ops/delete_propertygraph_statistics.rs`. Use any of them as the pattern for the
four above.

`GetStorageInfo` and `ListIndexRebuilds` arrived with the storage endpoints a
later model refresh added, and were declined for that same reason. The three
index-rebuild operations that refresh added WITH a typed input —
`StartIndexRebuild`, `GetIndexRebuildStatus` and `CancelIndexRebuild` — are
implemented in their own files and answer from the rebuild registry in
`src/state/index_rebuild.rs`, which is what a hand-authored `ListIndexRebuilds`
would list.
