Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set|run#12|AssertionError|assert returned and unprocessed, (
Issue-Key: dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected|_error_code#16|AssertionError|raise AssertionError(f'real AWS refuses this replay, but it SUCCEEDED and returned {result!r}')
Issue-Key: dynamodb|UpdateItem|ValidationException
Issue-Commit: ea226a61fd4fc055bb456ff32cf72ea13892c65d

# DynamoDB: BatchGetItem never defers past the 16 MB response cap, a TransactWriteItems token replay with different Return* options is accepted, and the ADD-operand ValidationException is re-worded (3 causes)

> _Note: this issue was identified against the code base at commit `ea226a61fd4fc055bb456ff32cf72ea13892c65d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetItem

## Description
THREE net-new root causes, all in service `dynamodb`.

Cause 1 of 3: BatchGetItem has no response-size cap, so it never produces a partial result or a non-empty UnprocessedKeys.
AWS: Smithy BatchGetItem docs — "A single operation can retrieve up to 16 MB of data, which can contain as many as 100 items. BatchGetItem returns a partial result if the response size limit is exceeded [...] If a partial result is returned, the operation returns a value for UnprocessedKeys", and the worked example "if you ask to retrieve 100 items, but each individual item is 300 KB in size, the system returns 52 items (so as not to exceed the 16 MB limit). It also returns an appropriate UnprocessedKeys value". Smithy: BatchGetItemOutput.UnprocessedKeys targets BatchGetRequestMap (@length min 1 max 100) and its doc says the deferred keys come back "in the same form as RequestItems, so the value can be provided directly to a subsequent BatchGetItem operation"; whole items are deferred, never truncated, and a deferred key is still present in storage (deferral is about the response budget, not existence). The call still succeeds with HTTP 200.
Emulator: a single BatchGetItem for 60 keys whose items are ~380 KB each (about 22 MB of item data) answers 200 with ALL 60 items in Responses and an EMPTY UnprocessedKeys map — the 16 MB ceiling is not applied at all. The handler doc-comment in services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/batch_get_item.rs states the design explicitly: "Every key this emulator accepted is read, so UnprocessedKeys is the empty map AWS answers with rather than an absent member."
Why: every client that batch-reads items of non-trivial size relies on the drain loop (re-submit UnprocessedKeys until it is empty). Against this emulator that loop never executes, so the code path is never exercised and the single response can exceed what any real client/SDK ever receives from AWS; a caller validated here ships an application that silently loses the keys AWS would have deferred.
Fix: the BatchGetItem handler and its read accounting (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/batch_get_item.rs) — accumulate the serialized response size while reading, stop before 16 MB on a whole-item boundary, and return the keys not read under UnprocessedKeys[<table>] in RequestItems form (Keys plus the entry's ConsistentRead/ProjectionExpression), leaving the deferred items untouched in storage.
Traces: dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set.

Cause 2 of 3: the TransactWriteItems ClientRequestToken idempotency fingerprint ignores ReturnConsumedCapacity and ReturnItemCollectionMetrics, so a replay that changes them is served instead of refused.
AWS: Smithy — TransactWriteItems.ClientRequestToken is @idempotencyToken (@length 1..36) and the operation models IdempotentParameterMismatchException (client error, HTTP 400), documented as "DynamoDB rejected the request because you retried a request with a different payload but with an idempotent token that was already used." The request parameters covered by that payload fingerprint include the Return* options: re-sending the same ClientRequestToken and the same TransactItems but with ReturnConsumedCapacity changed from 'TOTAL' to 'NONE', or with ReturnItemCollectionMetrics='SIZE' added, is a different request and is refused 400 IdempotentParameterMismatchException with the message "Specified idempotent token was used with different request parameters within the idempotency window" (fixed text, no request-derived parts), and the transaction's effect is NOT re-applied.
Emulator: token idempotency itself works (the first use is billed as a write and applies the ADD; the byte-identical replay is billed as a read and does not re-apply), but the replay with ReturnConsumedCapacity='NONE' is ACCEPTED: HTTP 200 with an empty body (content-length 2) and no error. The fingerprint is computed over TransactItems only, so the Return* members are invisible to it.
Why: a client that varies capacity/metrics reporting between a call and its retry — a normal thing to do while debugging or when a wrapper injects ReturnConsumedCapacity — gets a silently accepted second request against this emulator, and no IdempotentParameterMismatchException handling path is ever exercised; on real AWS the same code is refused 400.
Fix: the TransactWriteItems handler's client-request-token ledger (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/transact_write_items.rs) — include ReturnConsumedCapacity and ReturnItemCollectionMetrics in the stored request fingerprint, and answer 400 IdempotentParameterMismatchException with AWS's fixed message when a live token is re-used with a different fingerprint, applying nothing.
Traces: dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected.

Cause 3 of 3: UpdateItem's ADD-with-a-String-operand refusal carries emulator-authored wording instead of AWS's operand-type message.
AWS: an UpdateExpression 'ADD n :one' whose :one is a String is refused HTTP 400 ValidationException (Smithy: UpdateItem does not model a separate error for this; the recorded refusal is ValidationException) with the message "1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: ADD, operand type: STRING, typeSet: ALLOWED_FOR_ADD_OPERAND". Fixed text: the '1 validation error detected: ' envelope, 'Invalid UpdateExpression: Incorrect operand type for operator or function; operator: ', ', operand type: ', ', typeSet: '. Variable parts taken from the request: the operator ('ADD'), the offending operand's type name ('STRING'), and the typeSet name for that operator ('ALLOWED_FOR_ADD_OPERAND'). The call must not be retried (400) and must write nothing.
Emulator: the status (400) and the code (ValidationException) are right and nothing is written, but the message is the invented sentence "An operand in the update expression has an incorrect data type" — it names neither the operator, nor the operand type, nor the typeSet, and omits the envelope.
Why: the message is the only machine-readable statement of WHICH operand of WHICH operator was wrong; clients, SDK test suites and operators match it exactly or anchored, and all of those fail here. Nothing identifies the offending expression element.
Why this is net-new: open issue 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md (wire corpus) covers eight other DynamoDB ValidationException wordings — the ProjectionExpression size envelope, the two expression/non-expression conflicts, three Coral value renderings, the UpdateTable member list and an enum-set ordering — and does NOT cover the UpdateExpression operand-type family or this check's replay key.
Fix: the UpdateItem UpdateExpression validation / its ValidationException message builder (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_item.rs) — emit AWS's 'Incorrect operand type for operator or function' template, interpolating the operator, the operand's type and the operator's typeSet name.
Traces: dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate (the check's earlier arms — retry counts, amz-sdk-request/amz-sdk-invocation-id stamping, per-attempt x-amzn-RequestId, token'd vs token-less effect counts — all PASSED; it fails only on this message).

## Evidence
Replay of suite simparity-behavioral-aws-v1 (367/948 passed).

Cause 1 — dynamodb.batch-get-item-unprocessed-keys-are-in-storage-and-drain-to-the-exact-set [mismatch]:
  File "check_dynamodb_batch_get_item_unprocessed_keys_are_in_storage_and_drain_to_the_exact_set.py", line 45, in run
    assert returned and unprocessed, (
  AssertionError: 60 items of 389120 bytes cannot fit one BatchGetItem response, so the call must come back partial: it returned 60 items and named 0 unprocessed keys
Source corroboration: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/batch_get_item.rs, module doc lines 20-24: "Every key this emulator accepted is read, so `UnprocessedKeys` is the empty map AWS answers with rather than an absent member."
Model: smithy_query(doc, shape=BatchGetItem) -> "A single operation can retrieve up to 16 MB of data ... returns a partial result if the response size limit is exceeded ... the operation returns a value for UnprocessedKeys"; smithy_query(doc, shape=BatchGetItemOutput, member=UnprocessedKeys) -> "in the same form as RequestItems, so the value can be provided directly to a subsequent BatchGetItem operation".

Cause 2 — dynamodb.conc-ddb-optimistic-token-fingerprint-covers-return-options-rejected [mismatch]:
  File "check_dynamodb_conc_ddb_optimistic_token_fingerprint_covers_return_options_rejected.py", line 76, in run
    code, message = _error_code(_tx(ReturnConsumedCapacity='NONE'))
  AssertionError: real AWS refuses this replay, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}, 'RetryAttempts': 0}}
Expected: 400 IdempotentParameterMismatchException, message "Specified idempotent token was used with different request parameters within the idempotency window", counter unchanged at n=1. The in-check positive controls passed first: the initial use was billed WriteCapacityUnits>0 and applied the ADD, and the byte-identical replay was billed ReadCapacityUnits>0 with no WriteCapacityUnits and did not re-apply — so only the fingerprint's coverage of the Return* members is wrong.
Model: smithy_query(operation=TransactWriteItems) -> ClientRequestToken {idempotencyToken: true, length 1..36}; input members ReturnConsumedCapacity, ReturnItemCollectionMetrics; errors include IdempotentParameterMismatchException (client, httpError 400). smithy_query(shape=IdempotentParameterMismatchException) -> "DynamoDB rejected the request because you retried a request with a different payload but with an idempotent token that was already used."

Cause 3 — dynamodb.sdk-retry-replays-the-wire-and-only-the-token-stops-the-duplicate [mismatch]:
  File "check_dynamodb_sdk_retry_replays_the_wire_and_only_the_token_stops_the_duplicate.py", line 301, in run
    assert message == add_message, (
  AssertionError: ADD with a string operand raised the message 'An operand in the update expression has an incorrect data type', expected '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: ADD, operand type: STRING, typeSet: ALLOWED_FOR_ADD_OPERAND'
The code (ValidationException), the status (400), the single wire delivery (a 400 is not retried) and the absence of any written item all matched; only the message text diverges.

## Steps to Reproduce
1. Boot the emulator and replay the three check ids in trace_ids.
2. Cause 1: create a PAY_PER_REQUEST table keyed on pk (S). PutItem 60 items, each ~380 KB ({pk, pad}). Call BatchGetItem with all 60 keys and ConsistentRead=true in one RequestItems entry -> expect a partial result: fewer than 60 items in Responses, the remaining keys under UnprocessedKeys[<table>].Keys (disjoint from the returned set, untruncated payloads, each still readable by GetItem), and re-submitting UnprocessedKeys until it is empty yielding exactly the 60 keys once. Observe 200 with all 60 items and an empty UnprocessedKeys.
3. Cause 2: on a table keyed on pk, PutItem {pk:'ctr', n:0}. Call TransactWriteItems with ClientRequestToken=T, TransactItems=[Update ADD n :one], ReturnConsumedCapacity='TOTAL' -> applies, billed as a write. Repeat the identical call -> accepted, billed as a read, n stays 1. Now repeat with the same T and the same TransactItems but ReturnConsumedCapacity='NONE' -> expect 400 IdempotentParameterMismatchException with the message above and n unchanged; observe 200 with an empty body. Same with ReturnConsumedCapacity='TOTAL' plus ReturnItemCollectionMetrics='SIZE'.
4. Cause 3: UpdateItem on any table with UpdateExpression='ADD n :one' and ExpressionAttributeValues={':one': {'S': 'not-a-number'}} -> expect 400 ValidationException with message '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: ADD, operand type: STRING, typeSet: ALLOWED_FOR_ADD_OPERAND'; observe 'An operand in the update expression has an incorrect data type'.
