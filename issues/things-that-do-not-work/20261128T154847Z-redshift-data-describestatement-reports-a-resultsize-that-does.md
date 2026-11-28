Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftdata-serverless-workgroup-sql-round-trip
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftdata-serverless-workgroup-sql-round-trip
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.ResultSize
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# redshift-data DescribeStatement reports a ResultSize that does not match the bytes of the statement's result set

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStatement

## Description
Cause 1 of 1: after a SELECT finishes on a serverless workgroup, DescribeStatement reports the wrong ResultSize for the statement's result set.
AWS: DescribeStatement.ResultSize is "The size in bytes of the returned results. A -1 indicates the value is null." (Smithy: com.amazonaws.redshiftdata#DescribeStatementResponse$ResultSize, target Long, default 0, documentation quoted). For a given statement over a given result set the value is a deterministic function of the rows and columns returned — the same query over the same data yields the same byte count — so it is a modeled output value, not a per-run artifact like CreatedAt or RedshiftPid. The recorded run of this statement reports 46 bytes.
Emulator: for the same statement in the same fixture it reports 54 — a different byte count, so its result-size accounting uses a different (larger) measure of the result than Redshift does. Hypothesis: the emulator is sizing its own serialized result representation (e.g. including column metadata, separators, or stringified rather than native field widths) instead of the result bytes Redshift accounts for.
Why: ResultSize is what clients use to decide whether to page a result, to budget memory before calling GetStatementResult, and to report/charge for data scanned. A client that sizes a buffer or enforces a byte budget from ResultSize gets a figure that does not correspond to what the result read actually yields, and any assertion or regression check on the reported size is wrong by a fixed offset. Because the value is inflated, size-based thresholds trip earlier than on AWS.
Fix: the ResultSize computation in the redshift-data statement-completion bookkeeping (the statement state the DescribeStatement handler reads) — it must measure the result-set bytes the same way Redshift accounts for them, and still report -1 where the value is null rather than a substitute.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#12.

Note: no open issue carries this step's replay key; the replay reports it unowned. The nearest open redshift-data statement-metadata issues (20261125T005547Z-… on DescribeStatement.Duration and 20261121T075506Z-… on a batch sub-statement's RedshiftQueryId) are about different DescribeStatement members and do not cover ResultSize.

## Evidence
Replay of trace set simparity-wire-aws-v1: fixture redshiftdata-serverless-workgroup-sql-round-trip, step task#12, operation DescribeStatement, verdict mismatch. Status matched (200 expected, 200 actual); body diverged on exactly one path: $.ResultSize value_mismatch — expected 46, got 54. Divergence key: service=redshift-data|DescribeStatement|status=200->200|keys=$.ResultSize.

Smithy grounding (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): DescribeStatementResponse$ResultSize -> Long, default 0; documentation "The size in bytes of the returned results. A -1 indicates the value is null."

## Steps to Reproduce
1. Boot the emulator from this checkout and point the redshift-data client at its data listener.
2. Create/select a redshift-serverless workgroup and database, then ExecuteStatement a SELECT that returns a small, fixed result set (the fixture's round-trip query).
3. Poll DescribeStatement(Id=<statement id>) until Status=FINISHED and read ResultSize alongside ResultRows/HasResultSet.
4. Compare ResultSize against the bytes of the result set that GetStatementResult actually returns for the same statement: the emulator reports a larger figure (54 where the recorded AWS run reports 46 for this statement).
5. Re-replay fixture redshiftdata-serverless-workgroup-sql-round-trip, step task#12, to confirm.
