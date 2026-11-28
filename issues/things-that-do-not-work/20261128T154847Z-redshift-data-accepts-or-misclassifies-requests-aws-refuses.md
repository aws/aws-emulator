Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftdata-list-statements-unknown-target-filters
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftdata-list-statements-unknown-target-filters redshiftdata-unknown-session-id-submission-rejection
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|ListStatements|4xx->2xx|$.Message
Issue-Key: redshift-data|ListStatements|4xx->2xx|$.__type
Issue-Key: redshift-data|ListStatements|4xx->2xx|status
Issue-Key: redshift-data|BatchExecuteStatement|4xx->4xx|$.Message
Issue-Key: redshift-data|BatchExecuteStatement|4xx->4xx|$.__type
Issue-Key: redshift-data|BatchExecuteStatement|4xx->4xx|status
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|$.Message
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|$.__type
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|status
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# redshift-data accepts or misclassifies requests AWS refuses with a 400 ValidationException: ListStatements target filters and an unknown SessionId

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStatements

## Description
Three distinct causes on the redshift-data request-validation surface. Each cause below is separate work; all three must be closed before this issue is submitted. The shared fault: a request AWS refuses up front with ValidationException/HTTP 400 is instead served successfully, or refused with a different error class, status and message.

Cause 1 of 3: ListStatements naming BOTH a provisioned cluster and a serverless workgroup is served instead of refused.
AWS: ListStatements rejects a request that carries both ClusterIdentifier and WorkgroupName. Smithy (com.amazonaws.redshiftdata): ListStatementsRequest.WorkgroupName documentation — "When providing WorkgroupName, then ClusterIdentifier can't be specified"; ListStatements models ValidationException with httpError 400. The recorded response is HTTP 400, __type ValidationException, Message "Only one of the following can be specified: ClusterIdentifier, WorkgroupName." — that message is entirely FIXED text (it names the two mutually exclusive member names from the model, not any request value); there is no variable part.
Emulator: HTTP 200 with a normal ListStatementsResponse body — no __type and no Message at all. The mutual-exclusion check does not exist, so the emulator silently picks one filter (or ignores both) and answers a statement list.
Why: a client that sets both filters by mistake (a common wiring bug when switching a tool from provisioned Redshift to Redshift Serverless) is told nothing is wrong and then reads a statement list scoped to the wrong target, so it reports statuses for statements it never submitted. Negative tests that assert the ValidationException never fire.
Fix: ListStatements input validation in the redshift-data operation handler — reject ClusterIdentifier and WorkgroupName supplied together with ValidationException/400 and the fixed message above, before any state lookup.
Traces: redshiftdata-list-statements-unknown-target-filters#task#3.

Cause 2 of 3: BatchExecuteStatement naming a SessionId that was never issued is refused with the wrong error class, status and message.
AWS: ValidationException, HTTP 400, Message "Session with Id: <SessionId> is invalid." — fixed text "Session with Id: " + the request's SessionId value + " is invalid." (the recorded 5e551000-7a1d-4c0d-9e11-35a96f731d3e is the request's SessionId filling that template, not part of it). Smithy: BatchExecuteStatement models both ValidationException (httpError 400) and ResourceNotFoundException (httpError 404); AWS treats a SessionId that resolves to no session as a malformed input value, not a missing resource, so the 400 class is the one that is used.
Emulator: HTTP 404, __type ResourceNotFoundException, Message "Session not found." — wrong status, wrong error code, and a message of its own wording that neither names the member nor echoes the offending SessionId.
Why: a client that branches on the error class takes the wrong path — 404/ResourceNotFoundException reads as "the workgroup or statement is gone, re-resolve the target" and is commonly retried, whereas a 400 ValidationException is a non-retryable bad-input signal telling it to stop reusing that SessionId. Anything matching AWS's "Session with Id: … is invalid." text never matches, and the SessionId is not reported back so the failure cannot be attributed to a specific session.
Fix: the SessionId resolution step shared by the ExecuteStatement/BatchExecuteStatement handlers in the redshift-data service — an unresolvable SessionId must raise ValidationException/400 with the templated message above.
Traces: redshiftdata-unknown-session-id-submission-rejection#task#1.

Cause 3 of 3: ExecuteStatement naming a SessionId that was never issued is refused the same wrong way.
AWS: identical to Cause 2 — ValidationException, HTTP 400, "Session with Id: <SessionId> is invalid." Smithy: ExecuteStatement models ValidationException httpError 400 and ResourceNotFoundException httpError 404.
Emulator: HTTP 404, ResourceNotFoundException, "Session not found."
Why: same as Cause 2 — the single-statement submission path misroutes every unknown-session failure into a retryable not-found.
Fix: same resolution step as Cause 2, reached from the ExecuteStatement handler; verify both entry points, since the replay shows each operation independently.
Traces: redshiftdata-unknown-session-id-submission-rejection#task#2.

Related but NOT covering these: open issue 20261127T214412Z-redshift-data-refusals-and-pagination-7-net-new, whose Cause 2 is a session that was opened and has since CLOSED (AWS: ValidationException "Session is not available"). Causes 2 and 3 here are a SessionId that never existed, which AWS refuses with the different "Session with Id: … is invalid." message; the replay reports these steps as not owned by any open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (89 steps, 85 passed, 4 mismatched).

Cause 1 — fixture redshiftdata-list-statements-unknown-target-filters, step task#3, ListStatements: status expected 400, got 200; $.__type missing (expected 'ValidationException', got None); $.Message missing (expected 'Only one of the following can be specified: ClusterIdentifier, WorkgroupName.', got None).

Cause 2 — fixture redshiftdata-unknown-session-id-submission-rejection, step task#1, BatchExecuteStatement: status expected 400, got 404; $.__type value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException'); $.Message value_mismatch (expected 'Session with Id: 5e551000-7a1d-4c0d-9e11-35a96f731d3e is invalid.', got 'Session not found.').

Cause 3 — fixture redshiftdata-unknown-session-id-submission-rejection, step task#2, ExecuteStatement: status expected 400, got 404; $.__type value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException'); $.Message value_mismatch (expected 'Session with Id: 5e551000-7a1d-4c0d-9e11-35a96f731d3e is invalid.', got 'Session not found.').

Smithy grounding (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ListStatements errors = [InternalServerException 500, ResourceNotFoundException 404, ValidationException 400]; ListStatementsRequest.WorkgroupName doc states ClusterIdentifier can't be specified alongside it. ExecuteStatement/BatchExecuteStatement errors both include ValidationException (400) and ResourceNotFoundException (404); their SessionId member is a UUID-patterned optional input.

## Steps to Reproduce
1. Boot the emulator from this checkout and point the redshift-data client at its data listener.
2. Cause 1: call ListStatements with BOTH ClusterIdentifier=<any cluster id matching ^[a-z][a-z0-9]*(-[a-z0-9]+)*$> and WorkgroupName=<any 3-128 char workgroup name>. Observe HTTP 200 and a statement list; AWS answers 400 ValidationException 'Only one of the following can be specified: ClusterIdentifier, WorkgroupName.'
3. Cause 2: call BatchExecuteStatement(Sqls=['select 1'], WorkgroupName=<workgroup>, Database='dev', SessionId=<a syntactically valid UUID never returned by any ExecuteStatement/BatchExecuteStatement call>). Read the HTTP status, x-amzn-errortype/__type and Message. Observe 404/ResourceNotFoundException/'Session not found.'; AWS answers 400/ValidationException/'Session with Id: <that SessionId> is invalid.'
4. Cause 3: repeat step 3 with ExecuteStatement(Sql='select 1', ...) and the same never-issued SessionId.
5. Re-replay fixtures redshiftdata-list-statements-unknown-target-filters (task#3) and redshiftdata-unknown-session-id-submission-rejection (task#1, task#2) to confirm.
