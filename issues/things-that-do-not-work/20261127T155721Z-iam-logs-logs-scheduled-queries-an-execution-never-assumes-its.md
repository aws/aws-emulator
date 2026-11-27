Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__logs.scheduled-query-resolves-execution-role-created-after-the-query iam__logs.update-scheduled-query-execution-role-swap-runs-under-new-role
Issue-Key: iam__logs.scheduled-query-resolves-execution-role-created-after-the-query|_role_use#33|AssertionError|raise AssertionError(f"IAM reported no use of the execution role within {budget_s}s")
Issue-Key: iam__logs.update-scheduled-query-execution-role-swap-runs-under-new-role|_first_complete#33|AssertionError|raise AssertionError(
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs scheduled queries: an execution never assumes its execution role (IAM records no use), and executions are recorded as having run before the query was enabled

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScheduledQuery

## Description
Cause 1 of 2 — a scheduled query's execution role is never assumed, so IAM records no use of it.
AWS: each scheduled-query execution is performed under the query's `executionRoleArn`; the role is resolved at execution time (so a role created after the query is picked up), and `GetRole` subsequently reports a `RoleLastUsed` for it. Recorded behavior only (the execution-role contract is documented; the Smithy model only carries `executionRoleArn` on the scheduled-query shape).
Emulator: no use of the execution role is ever reported, for either the original or the later-created role.
Traces: iam__logs.scheduled-query-resolves-execution-role-created-after-the-query.

Cause 2 of 2 — executions are timestamped before the enable that should have started them.
AWS: after a scheduled query is enabled (or its execution role swapped and re-enabled), the next `Complete` execution has a start time at or after the enabling call, so a client can attribute an execution to a given configuration.
Emulator: the executions observed after enabling are dated *before* it — the check reports `saw (status, seconds after enabling) [('Complete', -5), ('Complete', -7)]`, i.e. two Complete executions 5 s and 7 s before the enable, and no execution after it. The negative offsets show the execution history is back-dated (or pre-seeded) rather than produced by the schedule.
Traces: iam__logs.update-scheduled-query-execution-role-swap-runs-under-new-role.

Why: a client cannot tell which configuration produced a result. Cause 1 means the query is not actually running under the identity the client granted, so permission scoping on scheduled queries is unenforced and un-auditable; cause 2 makes 'did my change take effect?' unanswerable, because every observation window already contains executions from before the change.

Fix: the scheduled-query executor — resolve and assume `executionRoleArn` per execution (recording the role use), and stamp each execution with its real start time so that an execution never predates the configuration that enabled it.

## Evidence
Cause 1: "IAM reported no use of the execution role within -10.418s" (the budget was already spent; no RoleLastUsed was ever observed).
Cause 2: "no Complete execution after enabling in -0.08s; saw (status, seconds after enabling) [('Complete', -5), ('Complete', -7)]".

## Steps to Reproduce
1. Create a scheduled query naming an executionRoleArn, create that role afterwards, let the schedule fire, then GetRole and read RoleLastUsed — never set.
2. Update the scheduled query to a second execution role and enable it; list its executions with their start times — the Complete executions are dated before the enabling call and none occurs after it.
