Issue-Source: trace-sweep-divergence
Issue-Trace: athena-named-query-crud-and-batch-retrieval
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-named-query-crud-and-batch-retrieval
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetNamedQuery|2xx->2xx|$.NamedQuery.Name
Issue-Key: athena|GetNamedQuery|2xx->2xx|$.NamedQuery.QueryString
Issue-Key: athena|GetNamedQuery|4xx->2xx|$.AthenaErrorCode
Issue-Key: athena|GetNamedQuery|4xx->2xx|$.ErrorCode
Issue-Key: athena|GetNamedQuery|4xx->2xx|$.Message
Issue-Key: athena|GetNamedQuery|4xx->2xx|$.__type
Issue-Key: athena|GetNamedQuery|4xx->2xx|status
Issue-Commit: a4d278f1279dca95bd1b1693fbff7a33c3289259

# Athena named query mutations are not applied: UpdateNamedQuery and DeleteNamedQuery leave stored state

> _Note: this issue was identified against the code base at commit `a4d278f1279dca95bd1b1693fbff7a33c3289259`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNamedQuery

## Description
Two causes, both the same root fault: the Athena named-query store accepts mutating calls but never applies them, so a read after the mutation still serves the pre-mutation entity. Both are net-new; no open issue carries either replay key (a search for 'named query' in the open issues returns nothing).

Cause 1 of 2: UpdateNamedQuery does not update the stored named query (GetNamedQuery, fixture athena-named-query-crud-and-batch-retrieval, step task#6).
AWS: Smithy: UpdateNamedQuery input members NamedQueryId (required, @length 1-128, pattern ^\S+$), Name (required, @length 1-128), QueryString (required, @length 1-262144), Description (@length 0-1024); output is empty; errors InvalidRequestException (400) / InternalServerException (500). The operation is @idempotent and its contract is to replace the named query's Name, QueryString and Description. A subsequent GetNamedQuery must return NamedQuery with the NEW Name and QueryString (Smithy: NamedQuery.Name, NamedQuery.QueryString, both required). The recording confirms this: after an UpdateNamedQuery that sets a new name and a new query string, GetNamedQuery answers 200 with the updated values.
Emulator: GetNamedQuery answers 200 but with the values the named query was CREATED with: $.NamedQuery.Name is the pre-update name (expected the updated name, e.g. '...-updated-<suffix>', got the original) and $.NamedQuery.QueryString is the pre-update SQL (expected 'SELECT 100', got 'SELECT 1'). The recorded suffix/ids are per-run values that fill the template; the requirement is that Name and QueryString equal whatever the preceding UpdateNamedQuery sent.
Why: a client that renames or edits a saved query, then reads it back (or lists it) to confirm, sees the old definition and concludes the edit failed; a client that re-runs the saved query executes the stale SQL. Any workflow that round-trips UpdateNamedQuery -> GetNamedQuery/BatchGetNamedQuery cannot make progress.
Fix: the UpdateNamedQuery handler / the named-query state store — the call must write Name, QueryString and Description back onto the stored record keyed by NamedQueryId rather than returning an empty success without mutating state.
Traces: athena-named-query-crud-and-batch-retrieval#task#6.

Cause 2 of 2: a deleted named query is still readable — GetNamedQuery answers 200 instead of 400 InvalidRequestException / NAMED_QUERY_NOT_FOUND (GetNamedQuery, fixture athena-named-query-crud-and-batch-retrieval, step task#10).
AWS: Smithy: DeleteNamedQuery (@idempotent, input NamedQueryId required) removes the named query; GetNamedQuery for an id that does not exist is modeled to fail with InvalidRequestException (errorKind client, httpError 400), whose members are AthenaErrorCode (@length 1-256) and Message. The recorded AWS response for a GetNamedQuery after DeleteNamedQuery is HTTP 400 with __type 'InvalidRequestException', AthenaErrorCode 'NAMED_QUERY_NOT_FOUND', ErrorCode 'NAMED_QUERY_NOT_FOUND' and Message 'NamedQuery: <namedQueryId> does not exist'. In that message the fixed text is 'NamedQuery: ' and ' does not exist'; the only variable part is the NamedQueryId taken from the request (the recording's '235e684a-f032-4619-a3f7-e51b06dd173a' is one run's id, not part of the template).
Emulator: the same call answers HTTP 200 with a NamedQuery body: status expected 400, got 200, and $.__type, $.AthenaErrorCode, $.ErrorCode and $.Message are all absent. The delete left the record in place.
Why: delete is not observable. A client cannot tell a successful delete from a no-op, cleanup/idempotency logic that probes with GetNamedQuery sees the query as still present and loops or reports a leaked resource, and error handling for the NAMED_QUERY_NOT_FOUND case is never exercised.
Fix: the DeleteNamedQuery handler / the named-query state store must remove the record, and the GetNamedQuery handler must raise InvalidRequestException (HTTP 400) with AthenaErrorCode/ErrorCode 'NAMED_QUERY_NOT_FOUND' and the message above when the id is unknown — the error serializer must emit AthenaErrorCode AND ErrorCode alongside __type, as the recorded body does.
Traces: athena-named-query-crud-and-batch-retrieval#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 67.0%), divergence clusters 33 and 34 of 42, both verdict=mismatch, service=athena, neither owned_by any open issue nor blocked by a stub.

Cause 1 — fixture athena-named-query-crud-and-batch-retrieval, step task#6, GetNamedQuery, status 200 -> 200:
  $.NamedQuery.Name: value_mismatch (expected 'sim2real-probe-athena-named-query-one-updated-00eaff9b', got 'sim2real-probe-athena-named-query-one-00eaff9b')
  $.NamedQuery.QueryString: value_mismatch (expected 'SELECT 100', got 'SELECT 1')

Cause 2 — fixture athena-named-query-crud-and-batch-retrieval, step task#10, GetNamedQuery, expected status 400, actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidRequestException', got None)
  $.AthenaErrorCode: missing (expected 'NAMED_QUERY_NOT_FOUND', got None)
  $.ErrorCode: missing (expected 'NAMED_QUERY_NOT_FOUND', got None)
  $.Message: missing (expected 'NamedQuery: 235e684a-f032-4619-a3f7-e51b06dd173a does not exist', got None)

Model evidence: smithy_query(operation="GetNamedQuery,UpdateNamedQuery,DeleteNamedQuery") and smithy_query(shape="InvalidRequestException,NamedQuery") on services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json.

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. Replay fixture athena-named-query-crud-and-batch-retrieval.
   Cause 1: CreateNamedQuery a query (Name 'q-one', QueryString 'SELECT 1', Database/WorkGroup from the fixture), then UpdateNamedQuery with the same NamedQueryId, a new Name ('q-one-updated') and QueryString 'SELECT 100', then GetNamedQuery with that NamedQueryId (step task#6). Observe NamedQuery.Name and NamedQuery.QueryString still hold the created values instead of the updated ones.
   Cause 2: DeleteNamedQuery that NamedQueryId, then GetNamedQuery with it again (step task#10). Observe HTTP 200 with a NamedQuery body instead of HTTP 400 InvalidRequestException with AthenaErrorCode/ErrorCode 'NAMED_QUERY_NOT_FOUND' and Message 'NamedQuery: <id> does not exist'.
