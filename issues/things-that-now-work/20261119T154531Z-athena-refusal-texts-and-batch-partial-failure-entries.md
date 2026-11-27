Issue-Source: trace-sweep-divergence
Issue-Trace: athena-capacity-reservation-validation-and-lookup
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-capacity-reservation-validation-and-lookup athena-service-catalog-metadata-and-workgroup-conflicts athena-prepared-statement-batch-partial-result-and-delete-not-found
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|PutCapacityAssignmentConfiguration|4xx->4xx|$.Message
Issue-Key: athena|GetResourceDashboard|4xx->4xx|$.Message
Issue-Key: athena|BatchGetPreparedStatement|2xx->2xx|$.UnprocessedPreparedStatementNames[*].ErrorCode
Issue-Key: athena|BatchGetPreparedStatement|2xx->2xx|$.UnprocessedPreparedStatementNames[*].ErrorMessage
Issue-Commit: f97a2791479cf3e78acbdef1e7d516a9feec11d8

# Athena refusal texts and batch partial-failure entries are emulator-authored on three operations

> _Note: this issue was identified against the code base at commit `f97a2791479cf3e78acbdef1e7d516a9feec11d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCapacityAssignmentConfiguration

## Description
Three net-new root causes from the wire replay of simparity-wire-aws-v1. In each, the emulator reaches the right HTTP status and the right error shape but reports a different FAILURE CONDITION than AWS does — a different validation order, a different failure class, or a different per-item error code — so the text and the code a client matches on are emulator-authored. All three clusters carry an empty stub_path and an empty owned_by. Each cause must be closed separately; there are 3 of them.

Related open issues that do NOT cover these causes: 20261111T123309Z-athena-capacity-reservation-read-and-lifecycle-paths-diverge.md and 20261111T163757Z-athena-capacity-reservation-lifecycle-refusals-diverge-delete-state.md (capacity-reservation read/lifecycle refusals; neither covers PutCapacityAssignmentConfiguration's workgroup-resolution order), and 20261117T053359Z-athena-7-net-new-causes-the-built-in.md (behavioral; its Cause 4 names the same BatchGetPreparedStatement ErrorCode defect but carries no wire replay key, and it does not cover the accompanying ErrorMessage divergence filed as Cause 3 here).

Cause 1 of 3: PutCapacityAssignmentConfiguration resolves the capacity reservation before the assigned workgroups, so an unresolvable assignment is reported as a missing reservation.
AWS: Smithy PutCapacityAssignmentConfiguration(CapacityReservationName @required @length 1..128 @pattern ^[a-zA-Z0-9._-]+$, CapacityAssignments @required) -> empty output, idempotent, errors InternalServerException (500) and InvalidRequestException (400). When the request's CapacityAssignments name workgroups that do not exist, the refusal is HTTP 400, InvalidRequestException with the message 'One or more WorkGroups are not found.' — entirely fixed text, with nothing interpolated from the request or state; AWS resolves the assignment's workgroup names first and reports that condition even when the named reservation is also absent (recorded behavior for the ordering).
Emulator: HTTP 400 and InvalidRequestException (both correct) but the message is the reservation-not-found template, 'Reservation with name <CapacityReservationName> not found.' — fixed text 'Reservation with name ' + ' not found.' around the name copied from the request.
Why: the two conditions require opposite remedies. 'One or more WorkGroups are not found.' tells a client to create or correct the workgroups it is assigning; the reservation-not-found text tells it the reservation is missing and should be created first. A client (or operator runbook) that branches on the refusal text takes the wrong corrective action, and a reconciler that treats reservation-not-found as 'nothing to assign yet' silently skips a request AWS rejects for a different and still-unfixed reason.
Fix: the PutCapacityAssignmentConfiguration handler's validation order in the athena service crate — resolve every workgroup named in CapacityAssignments and emit AWS's fixed workgroup message before the reservation lookup's not-found refusal.
Traces: athena-capacity-reservation-validation-and-lookup#task#6.

Cause 2 of 3: GetResourceDashboard reports a malformed ResourceARN as not found.
AWS: Smithy GetResourceDashboard(ResourceARN @required @length 1..1011) -> {Url @required}, errors InternalServerException (500), InvalidRequestException (400), ResourceNotFoundException (400). A ResourceARN that is not a complete, well-formed ARN is refused with HTTP 400 and the message 'ResourceARN is not complete or invalid' — entirely fixed text, with no part taken from the request and no terminating period (recorded behavior only; the model places no text constraint on ErrorMessage). This is an input-shape failure, distinct from a well-formed ARN naming a resource that does not exist.
Emulator: HTTP 400 and the same error shape, but the message is 'ResourceARN is not found' — it skips the ARN-syntax check and reports the lookup miss instead.
Why: a client cannot tell a malformed ARN (fix the request) from an absent resource (create it or stop polling). An ARN assembled with a missing region, account or resource segment is reported as a resource that does not exist, so the client keeps retrying a request that can never succeed.
Fix: the GetResourceDashboard handler's input validation in the athena service crate — validate the ARN's completeness and emit AWS's fixed syntax-failure message before any resource lookup.
Traces: athena-service-catalog-metadata-and-workgroup-conflicts#task#5.

Cause 3 of 3: BatchGetPreparedStatement's unprocessed entry carries an emulator-authored ErrorCode and ErrorMessage.
AWS: Smithy BatchGetPreparedStatement(PreparedStatementNames @required, WorkGroup @required) -> {PreparedStatements, UnprocessedPreparedStatementNames}; UnprocessedPreparedStatementName{StatementName, ErrorCode @length 1..256, ErrorMessage}. A name that does not exist in the workgroup is not raised as an exception — the call returns HTTP 200 and routes the name into UnprocessedPreparedStatementNames with ErrorCode 'STATEMENTNAME_NOT_FOUND' (fixed string) and ErrorMessage 'PreparedStatement <StatementName> was not found' — fixed text 'PreparedStatement ' + ' was not found' around the statement name copied from the request, with no terminating period (recorded behavior for both strings; note the member's own documentation lists 'STATEMENT_NOT_FOUND' among possible codes, but the recorded service response uses 'STATEMENTNAME_NOT_FOUND', so the recording governs).
Emulator: status 200 and the routing are correct, but the entry reports ErrorCode 'STATEMENT_NOT_FOUND' and the generic ErrorMessage 'A prepared statement with the name provided could not be found.' — a code that differs from AWS's by one token and a message that names no statement at all.
Why: in a partial-success batch the per-item ErrorCode is the only machine-readable discriminator, and the per-item ErrorMessage is the only place the failing name is reported. A client matching AWS's code sees no match and buckets the entry as an unknown failure class rather than a missing statement, and a client that surfaces the message to an operator loses which of the submitted names failed.
Fix: the per-item error classification and message construction in the BatchGetPreparedStatement handler in the athena service crate.
Traces: athena-prepared-statement-batch-partial-result-and-delete-not-found#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (42 fixtures, 433 steps, 372 passed, score 85.9%). All three clusters report stub_path '' and owned_by [].

Cause 1 — PutCapacityAssignmentConfiguration, athena-capacity-reservation-validation-and-lookup#task#6, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'One or more WorkGroups are not found.', got 'Reservation with name <CapacityReservationName> not found.')
  (the recorded run's name was sim2real-probe-athena-nonexistent-cap-res-f9a40983 — a per-run value, not part of the requirement; AWS's expected message interpolates nothing at all)

Cause 2 — GetResourceDashboard, athena-service-catalog-metadata-and-workgroup-conflicts#task#5, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'ResourceARN is not complete or invalid', got 'ResourceARN is not found')
  (both strings are wholly fixed text)

Cause 3 — BatchGetPreparedStatement, athena-prepared-statement-batch-partial-result-and-delete-not-found#task#4, expected 200 / actual 200:
  $.UnprocessedPreparedStatementNames[0].ErrorCode: value_mismatch (expected 'STATEMENTNAME_NOT_FOUND', got 'STATEMENT_NOT_FOUND')
  $.UnprocessedPreparedStatementNames[0].ErrorMessage: value_mismatch (expected 'PreparedStatement <StatementName> was not found', got 'A prepared statement with the name provided could not be found.')
  (the recorded run's StatementName was sim2real_probe_athena_ps_missing_45953105 — the variable part is the name from the request; 'PreparedStatement ' and ' was not found' are the fixed text)

Smithy consulted (services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json): PutCapacityAssignmentConfiguration (CapacityReservationName/CapacityAssignments @required, errors InvalidRequestException 400 + InternalServerException 500); GetResourceDashboard (ResourceARN @required @length 1..1011, errors InvalidRequestException/ResourceNotFoundException 400 + InternalServerException 500); BatchGetPreparedStatement -> {PreparedStatements, UnprocessedPreparedStatementNames} and UnprocessedPreparedStatementName{StatementName, ErrorCode @length 1..256, ErrorMessage} whose doc lists INVALID_INPUT / STATEMENT_NOT_FOUND / UNAUTHORIZED.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture athena-capacity-reservation-validation-and-lookup to step task#6 — call PutCapacityAssignmentConfiguration with a CapacityReservationName that does not exist and CapacityAssignments naming workgroups that do not exist either. The message is 'Reservation with name <name> not found.' instead of AWS's 'One or more WorkGroups are not found.'.
3. Cause 2: replay fixture athena-service-catalog-metadata-and-workgroup-conflicts to step task#5 — call GetResourceDashboard with an incomplete/malformed ResourceARN. The message is 'ResourceARN is not found' instead of 'ResourceARN is not complete or invalid'.
4. Cause 3: replay fixture athena-prepared-statement-batch-partial-result-and-delete-not-found to step task#4 — create a workgroup and one prepared statement in it, then call BatchGetPreparedStatement(PreparedStatementNames=[<real>, <missing>], WorkGroup=<wg>) and read UnprocessedPreparedStatementNames[0]: ErrorCode 'STATEMENT_NOT_FOUND' and the generic ErrorMessage instead of 'STATEMENTNAME_NOT_FOUND' and 'PreparedStatement <name> was not found'.
5. Re-replay all three fixture#step bindings after fixing; all three must pass before closing.
