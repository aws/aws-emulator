Issue-Source: trace-sweep-divergence
Issue-Trace: athena-data-catalog-create-conflict-and-delete-not-found-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-data-catalog-create-conflict-and-delete-not-found-errors athena-named-query-crud-and-batch-retrieval athena-named-query-mutation-not-found-errors-and-pagination
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|DeleteDataCatalog|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|DeleteDataCatalog|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetNamedQuery|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetNamedQuery|4xx->4xx|$.ErrorCode
Issue-Key: athena|UpdateNamedQuery|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|UpdateNamedQuery|4xx->4xx|$.ErrorCode
Issue-Key: athena|DeleteNamedQuery|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|DeleteNamedQuery|4xx->4xx|$.ErrorCode
Issue-Commit: f97a2791479cf3e78acbdef1e7d516a9feec11d8

# Athena resource-not-found refusals report AthenaErrorCode INVALID_INPUT instead of the resource-specific not-found code

> _Note: this issue was identified against the code base at commit `f97a2791479cf3e78acbdef1e7d516a9feec11d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataCatalog

## Description
Four net-new root causes from the wire replay of simparity-wire-aws-v1: four Athena operations refuse a well-formed reference to a resource that does not exist with the right HTTP status, the right error shape and the right message, but classify it inside InvalidRequestException with AthenaErrorCode INVALID_INPUT rather than the resource-specific not-found code AWS uses. Each cause must be closed separately; there are 4 of them.

Related open issues that do NOT cover these causes: 20261117T053359Z-athena-7-net-new-causes-the-built-in.md (behavioral; its Cause 3 is the same INVALID_INPUT misclassification in the per-item UnprocessedNamedQueryIds entry of BatchGetNamedQuery — a different operation and a different response member from the singular GetNamedQuery/UpdateNamedQuery/DeleteNamedQuery refusals here, and its Cause 2 concerns the reserved AwsDataCatalog delete message, not the not-found code), and 20261113T230556Z-athena-named-query-mutations-are-not-applied-updatenamedquery.md (named-query mutations not being applied to state, not their refusal classification).

AWS (shared by all four causes): InvalidRequestException is modeled as {AthenaErrorCode: ErrorCode @length 1..256, Message: ErrorMessage} with httpError 400, and is the only client error these four operations model besides InternalServerException (500). Athena discriminates the many conditions it folds into InvalidRequestException through the AthenaErrorCode member (botocore surfaces it as both AthenaErrorCode and ErrorCode): a reference that is syntactically valid but names a resource that does not exist is reported with a resource-specific not-found code, NOT with INVALID_INPUT, which AWS reserves for a request the caller must correct (malformed, out of range, mutually exclusive members). The codes are fixed strings; the accompanying Message, which the emulator already gets right in all four steps, carries the request-derived name or id. Recorded behavior for the code strings; the model constrains the member only to be a string.

Cause 1 of 4: DeleteDataCatalog on a catalog name that does not exist.
AWS: Smithy DeleteDataCatalog(Name @required @length 1..256, DeleteCatalogOnly default false), errors InternalServerException (500) and InvalidRequestException (400). An absent catalog name is refused with HTTP 400, InvalidRequestException and AthenaErrorCode/ErrorCode 'DATACATALOG_NOT_FOUND'.
Emulator: same status and message, AthenaErrorCode/ErrorCode 'INVALID_INPUT'.
Fix: the not-found refusal path of the DeleteDataCatalog handler in the athena service crate — set the resource-specific code when the catalog lookup misses.
Traces: athena-data-catalog-create-conflict-and-delete-not-found-errors#task#4.

Cause 2 of 4: GetNamedQuery on a NamedQueryId that does not exist.
AWS: Smithy GetNamedQuery(NamedQueryId @required @length 1..128 @pattern ^\S+$) -> {NamedQuery}, errors InternalServerException (500) and InvalidRequestException (400). An unknown but well-formed id is refused with HTTP 400, InvalidRequestException and AthenaErrorCode/ErrorCode 'NAMED_QUERY_NOT_FOUND'.
Emulator: same status and message, AthenaErrorCode/ErrorCode 'INVALID_INPUT'.
Fix: the not-found refusal path of the GetNamedQuery handler.
Traces: athena-named-query-crud-and-batch-retrieval#task#10.

Cause 3 of 4: UpdateNamedQuery on a NamedQueryId that does not exist.
AWS: Smithy UpdateNamedQuery(NamedQueryId @required, Name @required, QueryString @required @length 1..262144, Description), idempotent, errors InternalServerException (500) and InvalidRequestException (400). An unknown id is refused with HTTP 400, InvalidRequestException and AthenaErrorCode/ErrorCode 'NAMED_QUERY_NOT_FOUND'.
Emulator: same status and message, AthenaErrorCode/ErrorCode 'INVALID_INPUT'.
Fix: the not-found refusal path of the UpdateNamedQuery handler.
Traces: athena-named-query-mutation-not-found-errors-and-pagination#task#6.

Cause 4 of 4: DeleteNamedQuery on a NamedQueryId that does not exist.
AWS: Smithy DeleteNamedQuery(NamedQueryId @required, idempotencyToken), idempotent, errors InternalServerException (500) and InvalidRequestException (400). An unknown id is refused with HTTP 400, InvalidRequestException and AthenaErrorCode/ErrorCode 'NAMED_QUERY_NOT_FOUND'.
Emulator: same status and message, AthenaErrorCode/ErrorCode 'INVALID_INPUT'.
Fix: the not-found refusal path of the DeleteNamedQuery handler.
Traces: athena-named-query-mutation-not-found-errors-and-pagination#task#7.

Why (shared): AthenaErrorCode is the only machine-readable discriminator a client has, because AWS gives every one of these conditions the same HTTP 400 and the same exception shape. A client that branches on it — create-if-absent reconcilers, teardown loops that treat not-found as 'already gone', IaC drift detection — cannot distinguish 'this resource does not exist' from 'your request is malformed' against the emulator. The practical effect is that a missing catalog or saved query is reported as a caller bug, so an idempotent delete retries or aborts instead of converging, and a create-if-absent path treats the probe failure as unrecoverable.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (42 fixtures, 433 steps, 372 passed, score 85.9%). All four clusters report stub_path '' and owned_by [], i.e. no filed issue carries their replay key. In each, expected and actual status are both 400 and the Message and error shape matched; the only diverging keys are:

Cause 1 — DeleteDataCatalog, athena-data-catalog-create-conflict-and-delete-not-found-errors#task#4:
  $.AthenaErrorCode: value_mismatch (expected 'DATACATALOG_NOT_FOUND', got 'INVALID_INPUT')
  $.ErrorCode: value_mismatch (expected 'DATACATALOG_NOT_FOUND', got 'INVALID_INPUT')

Cause 2 — GetNamedQuery, athena-named-query-crud-and-batch-retrieval#task#10:
  $.AthenaErrorCode: value_mismatch (expected 'NAMED_QUERY_NOT_FOUND', got 'INVALID_INPUT')
  $.ErrorCode: value_mismatch (expected 'NAMED_QUERY_NOT_FOUND', got 'INVALID_INPUT')

Cause 3 — UpdateNamedQuery, athena-named-query-mutation-not-found-errors-and-pagination#task#6: identical two-key divergence (expected 'NAMED_QUERY_NOT_FOUND', got 'INVALID_INPUT').

Cause 4 — DeleteNamedQuery, athena-named-query-mutation-not-found-errors-and-pagination#task#7: identical two-key divergence (expected 'NAMED_QUERY_NOT_FOUND', got 'INVALID_INPUT').

Smithy consulted (services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json): DeleteDataCatalog, GetNamedQuery, UpdateNamedQuery and DeleteNamedQuery each model only InvalidRequestException (client, httpError 400) and InternalServerException (server, 500); InvalidRequestException{AthenaErrorCode: ErrorCode @length 1..256, Message: ErrorMessage}.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture athena-data-catalog-create-conflict-and-delete-not-found-errors to step task#4 — call DeleteDataCatalog with a catalog name that was never created and read the error's AthenaErrorCode: 'INVALID_INPUT' instead of 'DATACATALOG_NOT_FOUND'.
3. Cause 2: replay fixture athena-named-query-crud-and-batch-retrieval to step task#10 — call GetNamedQuery with a well-formed NamedQueryId that does not exist: AthenaErrorCode 'INVALID_INPUT' instead of 'NAMED_QUERY_NOT_FOUND'.
4. Causes 3 and 4: replay fixture athena-named-query-mutation-not-found-errors-and-pagination to steps task#6 and task#7 — call UpdateNamedQuery and then DeleteNamedQuery with a NamedQueryId that does not exist: both report AthenaErrorCode 'INVALID_INPUT' instead of 'NAMED_QUERY_NOT_FOUND'.
5. Re-replay all four fixture#step bindings after fixing; all four must pass before closing.
