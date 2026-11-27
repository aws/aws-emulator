Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftserverless-data-api-batch-sql-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftserverless-data-api-batch-sql-round-trip
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.SubStatements[*].ResultSize
Issue-Commit: 955f6c0c0ddc3c6d3572e0c42ed8336820cafac5

# redshift-data DescribeStatement under-reports ResultSize for a batch sub-statement result set

> _Note: this issue was identified against the code base at commit `955f6c0c0ddc3c6d3572e0c42ed8336820cafac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStatement

## Description
Cause 1 of 1: `DescribeStatement` reports a too-small `ResultSize` for a FINISHED statement that returned a result set, observed on the third sub-statement of a `BatchExecuteStatement` submission ($.SubStatements[2].ResultSize).

AWS: `DescribeStatementResponse.ResultSize` and `SubStatementData.ResultSize` are modelled as `Long` with `default: 0` and documented as "The size in bytes of the returned results. A -1 indicates the value is null." (Smithy: com.amazonaws.redshiftdata#DescribeStatementResponse$ResultSize, com.amazonaws.redshiftdata#SubStatementData$ResultSize). The model fixes the member's MEANING (bytes of the returned result) but not its accounting, so the requirement here is recorded behavior only: for the batch's third statement — the same SQL, run against the same table contents, reaching the same FINISHED status with the same `ResultRows`/`HasResultSet` as the emulator reports — the recorded Data API response reports `ResultSize` 47.

Emulator: the same step answers HTTP 200 with the same statement record but `$.SubStatements[2].ResultSize` = 31, i.e. the reported byte size of an identical result set is smaller than the service's. Every other member of that sub-statement record matched, so this is purely the size accounting, not a different result. The emulator derives the number from its own rule, stated in `result_size()`: the length of each column NAME plus the text of each row cell, plus exactly one separator byte per gap between adjacent cells (`cells - 1`). That rule is an emulator invention that happens to disagree with the service's for this result shape; because the shortfall comes from per-cell/per-line framing, it grows with the number of cells, so every result set wider or taller than this one diverges too.

Why: `ResultSize` is the only size a Data API client has before it fetches rows. Clients use it to decide whether to page `GetStatementResult`/`GetStatementResultV2`, to pre-size buffers, to bill or log result volume, and to assert "result is non-empty but small". Against the emulator every such client gets a number that is systematically low for a given result set, so size-based paging or buffer sizing that is correct against AWS is wrong here, and any test that pins an expected `ResultSize` (as this fixture does) fails for a result the emulator otherwise produced correctly.

Hypothesis (not stated by the model or derivable from the recording alone): the service's accounting includes framing the emulator's does not — e.g. a separator after every cell of a line plus a line terminator per line (including the column-name line), or per-value quoting of text cells — rather than a single byte only between two adjacent cells.

Fix: the result-size accounting in the redshift-data state, `state/result_size.rs::result_size()` (the single place the module's own doc comment says the rule is stated and must be refined), which feeds `Execution.result_size` -> `Statement::result_size()` -> the `ResultSize` member of both `DescribeStatementOutput` and each `SubStatements` entry built by `ops/describe_statement.rs::sub_statement_data`. The fix belongs there and not in the handler: the handler only copies the stored number, and the same number is reported for a single `ExecuteStatement`, for `ListStatements` records and for the CSV form of `GetStatementResultV2`, so correcting the rule in one place fixes all readers at once.

No existing open redshift-data issue covers this: the open issues cover `ExecuteStatement` WithEvent/EventBridge delivery, `SessionId` connection isolation, LOCK TABLE contention, and `DescribeTable.TableName` qualification — none of them touches `ResultSize` accounting.

Traces: redshiftserverless-data-api-batch-sql-round-trip#task#3 (DescribeStatement on the batch submission; $.SubStatements[2].ResultSize).

## Evidence
Replay of fixture `redshiftserverless-data-api-batch-sql-round-trip`, step `task#3`, service `redshift-data`, operation `DescribeStatement`.

verdict: mismatch — expected_status 200, actual_status 200 (status and envelope agree; one body member diverges).
detail: `$.SubStatements[2].ResultSize: value_mismatch (expected 47, got 31)`
divergence_key: `service=redshift-data|DescribeStatement|status=200->200|keys=$.SubStatements[2].ResultSize`

Expected (recorded AWS response): SubStatements[2].ResultSize = 47 bytes for that statement's result set.
Actual (emulator): SubStatements[2].ResultSize = 31 bytes for the same result set — a 16-byte shortfall, with all other members of that sub-statement record (Status, ResultRows, HasResultSet, QueryString, Id) matching.

Emulator accounting that produces the low value, from services/service-com-amazonaws-redshiftdata-redshiftdata/src/state/result_size.rs:53-72 — sum of column-name lengths + sum of `value_text(field).len()` over all row cells + `cells - 1` separator bytes. Consumed at src/state.rs:2069 (`result_size: bytes` for a rows-producing execution), stored at src/state.rs:931, read back at src/state.rs:1017-1023, and serialized at src/ops/describe_statement.rs:312 (`result_size: sub_statement.result_size()`).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay wire fixture `redshiftserverless-data-api-batch-sql-round-trip` (trace set simparity-wire-aws-v1) and let it run through step `task#3`.
   Equivalently, by hand against the redshift-data endpoint:
   a. BatchExecuteStatement on the serverless workgroup/database the fixture creates, submitting the fixture's three statements where the third returns a result set.
   b. Poll DescribeStatement with the returned batch Id until Status is FINISHED.
3. Inspect `$.SubStatements[2].ResultSize` in the DescribeStatement response.
4. Observed: 31. Expected (recorded AWS): 47 — the byte size AWS reports for that same result set. Any result set with more cells diverges by more, since the gap comes from per-cell/per-line framing the emulator does not count.
