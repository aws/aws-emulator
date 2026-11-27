Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.projection-list-index-out-of-range-null-element-and-nested-list dynamodb.projection-nested-map-path-returns-leaf-under-parent dynamodb.get-item-query-unaliased-dotted-name-is-a-nested-path dynamodb.legacy-attributes-to-get-empty-item-and-whole-nested-values dynamodb.batch-execute-statement-update-stored-type-mismatch-is-a-slot-error-rejected dynamodb.bulk-put-items-client-token-replay-returns-one-operation dynamodb.execute-statement-vector-index-qualified-select-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.projection-list-index-out-of-range-null-element-and-nested-list|run#11|AssertionError|assert got == want, f"ProjectionExpression {expr!r} returned {got!r}, expected {want!r}"
Issue-Key: dynamodb.projection-nested-map-path-returns-leaf-under-parent|run#12|KeyError|missing = _get(
Issue-Key: dynamodb.get-item-query-unaliased-dotted-name-is-a-nested-path|run#11|KeyError|assert unaliased["Item"] == {}, (
Issue-Key: dynamodb.legacy-attributes-to-get-empty-item-and-whole-nested-values|run#11|AssertionError|assert "Item" in none_matching, f"expected an Item key even though nothing matched: {none_matching!r}"
Issue-Key: dynamodb.batch-execute-statement-update-stored-type-mismatch-is-a-slot-error-rejected|run#12|AssertionError|assert responses == [
Issue-Key: dynamodb.bulk-put-items-client-token-replay-returns-one-operation|run#27|AssertionError|assert message == (
Issue-Key: dynamodb.execute-statement-vector-index-qualified-select-rejected|expect_error#13|AssertionError|assert got_message == message, f"unexpected message {got_message!r}"
Issue-Commit: 25f334ef49f0fe78ce039b48068cddfc6b5b1616

# dynamodb: 4 net-new causes — GetItem/BatchGetItem drop the Item member when a projection selects nothing, BatchExecuteStatement error slots omit TableName, and two refusal messages are re-worded

> _Note: this issue was identified against the code base at commit `25f334ef49f0fe78ce039b48068cddfc6b5b1616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
Four independent root causes in the DynamoDB emulator, found by replaying the behavioral suite. All four are in the `dynamodb` service; each must be closed before this issue is submitted. None is covered by an open issue: the closest open items are 20261119T104646Z-dynamodb-9-net-new-causes-vector-index-projection.md (vector-index projection/capacity, not the PartiQL FROM-clause refusal of Cause 4) and 20261111T170613Z-dynamodb-filter-projection-update-expression-validation-accepts-malformed.md (expression validation, not the empty-projection response shape of Cause 1); neither carries these replay keys.

=== Cause 1 of 4: GetItem / BatchGetItem / Query drop the `Item` member entirely when the projection selects none of an existing item's attributes. Failing checks: dynamodb.projection-list-index-out-of-range-null-element-and-nested-list, dynamodb.projection-nested-map-path-returns-leaf-under-parent, dynamodb.get-item-query-unaliased-dotted-name-is-a-nested-path, dynamodb.legacy-attributes-to-get-empty-item-and-whole-nested-values ===

AWS: Smithy: GetItemOutput.Item targets AttributeMap, documented as "A map of attribute names to AttributeValue objects, as specified by ProjectionExpression". The member is absent ONLY when no item matched the Key. When the Key DID match a stored item but the projection (ProjectionExpression or legacy AttributesToGet) resolves to no attribute of it, AWS answers HTTP 200 with the member present and empty — body `{"Item":{}}`. The same rule holds for the paths the checks exercise: an out-of-range list index (`mylist[99]`), a nested map path naming a key the map does not hold (`profile.not_a_key`), an unaliased dotted name read as a nested path (`Safety.Warning` against a top-level attribute literally spelled `Safety.Warning`), and legacy `AttributesToGet` naming only attributes the item lacks. For BatchGetItem the per-table `Responses` list carries an empty-map entry for that key rather than dropping it, and for Query the matched item appears in `Items` as `{}` with `Count` 1.

Emulator: answers HTTP 200 with a body of exactly `{}` (observed `content-length: 2`, content-type application/x-amz-json-1.0) — no `Item` member at all. Replay evidence: `ProjectionExpression 'mylist[99]' returned None, expected {}`; `KeyError: 'Item'` on `#p.#nope` and on the unaliased `Safety.Warning` projection; `AssertionError: expected an Item key even though nothing matched: {'ResponseMetadata': ...}` for `AttributesToGet=['phone','address']`.

Why: a client cannot distinguish "the item exists but nothing was projected" from "there is no such item". Idiomatic SDK code that reads `response['Item']` (Python KeyError, Java/JS undefined dereference) crashes on a legal read, and a read-then-write flow concludes the item is absent and re-creates or overwrites a live item.

Fix: the projection application / response serialization for GetItem (and the per-key entries of BatchGetItem and the per-item entries of Query/Scan) in the DynamoDB data-plane handler: once a Key has matched a stored item, the projected attribute map must be serialized even when it is empty, instead of the member being omitted on an empty map.

Traces: dynamodb.projection-list-index-out-of-range-null-element-and-nested-list (case `mylist[99]`), dynamodb.projection-nested-map-path-returns-leaf-under-parent (case `#p.#nope`), dynamodb.get-item-query-unaliased-dotted-name-is-a-nested-path (unaliased `Safety.Warning` on GetItem and Query), dynamodb.legacy-attributes-to-get-empty-item-and-whole-nested-values (GetItem and BatchGetItem with legacy AttributesToGet).

=== Cause 2 of 4: BatchExecuteStatement omits TableName from a slot that carries an Error. Failing check: dynamodb.batch-execute-statement-update-stored-type-mismatch-is-a-slot-error-rejected ===

AWS: Smithy: BatchStatementResponse has members Error (BatchStatementError), TableName (documented "The table name associated with a failed PartiQL batch statement", length 3-255, pattern ^[a-zA-Z0-9_.-]+$) and Item. AWS populates BOTH Error and TableName on a slot that failed, so each failing slot of a PartiQL batch names the table its statement addressed. HTTP status is 200: a per-slot failure is reported inside the batch response, not as an operation error.

Emulator: returns the error slot with Error only and no TableName. Recorded `Responses`: `[{'Error': {'Code': 'ValidationError', 'Message': 'An operand in the update expression has an incorrect data type'}}, {'TableName': '<table>'}]`. The error Code (`ValidationError`) and Message (fixed text `An operand in the update expression has an incorrect data type`, no variable parts) are correct, and the sibling slot does carry TableName — only the failing slot's TableName is missing.

Why: a client that correlates per-slot failures back to tables — to retry only the affected table, or to report which table rejected which statement — finds no table name on precisely the slots that failed, and must re-parse the submitted statement text to recover it.

Fix: the BatchExecuteStatement response serializer / per-slot result construction in the PartiQL batch handler: set TableName on every slot, including the ones carrying an Error.

Traces: dynamodb.batch-execute-statement-update-stored-type-mismatch-is-a-slot-error-rejected (both arms: the stored-type mismatch `SET s = s + 1` and the path-through-a-scalar `SET m.x.y = 1`).

=== Cause 3 of 4: StartBulkPutItems' IdempotentParameterMismatchException message drops the word "already". Failing check: dynamodb.bulk-put-items-client-token-replay-returns-one-operation ===

AWS: Smithy: StartBulkPutItems models IdempotentParameterMismatchException (client error, HTTP 400) and ClientToken is @idempotencyToken. The exception is documented as "DynamoDB rejected the request because you retried a request with a different payload but with an idempotent token that was already used." Reusing a client token with different request parameters (here MaxWriteRequestUnits doubled) is refused 400 IdempotentParameterMismatchException with the message `Specified idempotent token was already used with different request parameters within the idempotency window` — that string is entirely fixed text; it has no parts taken from the request or from state.

Emulator: refuses with the right status and code (400, IdempotentParameterMismatchException) but the message `Specified idempotent token was used with different request parameters within the idempotency window` — the fixed token "already" is missing.

Why: clients, CLI output and tests that match AWS's documented message for this refusal class do not match, so a caller cannot tell a token-reuse refusal from a differently worded validation refusal by message.

Fix: the StartBulkPutItems handler's client-token idempotency check, where the mismatch message is authored.

Traces: dynamodb.bulk-put-items-client-token-replay-returns-one-operation (the token-reuse-with-a-different-MaxWriteRequestUnits arm; the identical-token replay arm passed).

=== Cause 4 of 4: ExecuteStatement on a FROM clause qualified with a table's vector index reports the index as nonexistent. Failing check: dynamodb.execute-statement-vector-index-qualified-select-rejected ===

AWS: recorded behavior only — the Smithy model carries no message text for this refusal (ExecuteStatement's modeled errors do not name ValidationException; it arrives as a service-common error). The recording shows that for a table whose vector index exists and which DescribeTable reports with IndexStatus ACTIVE, `SELECT * FROM "<table>"."<vector-index>"` is refused 400 ValidationException with the message `Scan operation not supported on this index type` — fixed text naming the index TYPE as the unsupported thing, with no variable parts. The nonexistent-index message is a different refusal and is not what AWS serves here.

Emulator: refuses with 400 ValidationException but the message `The table does not have the specified index`, i.e. the refusal for an index that is not on the table, even though the vector index was created in the same run and read back ACTIVE.

Why: the client is told its index does not exist and will try to (re-)create an index that is already ACTIVE, instead of learning that a PartiQL scan of a vector index is unsupported and that it must use SearchVectors or query the base table.

Fix: the ExecuteStatement/PartiQL FROM-clause index resolution in the PartiQL handler: a named vector index must RESOLVE against the table's vector indexes and then be refused as an unsupported scan target, rather than falling through the secondary-index lookup into the unknown-index refusal.

Traces: dynamodb.execute-statement-vector-index-qualified-select-rejected (the base-table positive control in the same check passed).

## Evidence
Cause 1 — expected vs actual, GetItem on an existing item whose projection selects nothing:
  expected: HTTP 200, body {"Item":{}}
  actual:   HTTP 200, body {} (content-length 2, content-type application/x-amz-json-1.0); no Item member
  replay lines: "ProjectionExpression 'mylist[99]' returned None, expected {}"; "KeyError: 'Item'" (ProjectionExpression '#p.#nope'); "KeyError: 'Item'" (ProjectionExpression 'Safety.Warning'); "expected an Item key even though nothing matched: {'ResponseMetadata': {...'HTTPStatusCode': 200, ...'content-length': '2'...}}" (AttributesToGet=['phone','address'])

Cause 2 — expected vs actual, BatchExecuteStatement Responses:
  expected: [{'Error': {'Code': 'ValidationError', 'Message': 'An operand in the update expression has an incorrect data type'}, 'TableName': '<table>'}, {'TableName': '<table>'}]
  actual:   [{'Error': {'Code': 'ValidationError', 'Message': 'An operand in the update expression has an incorrect data type'}}, {'TableName': '<table>'}]
  (code and message correct; TableName absent on the error slot only)

Cause 3 — expected vs actual, StartBulkPutItems replayed with the same ClientToken and a different MaxWriteRequestUnits:
  expected: 400 IdempotentParameterMismatchException, message "Specified idempotent token was already used with different request parameters within the idempotency window" (entirely fixed text)
  actual:   400 IdempotentParameterMismatchException, message "Specified idempotent token was used with different request parameters within the idempotency window" ("already" missing)

Cause 4 — expected vs actual, ExecuteStatement SELECT * FROM "<table>"."<vector-index>" where the vector index is ACTIVE:
  expected: 400 ValidationException, message "Scan operation not supported on this index type"
  actual:   400 ValidationException, message "The table does not have the specified index"

## Steps to Reproduce
Cause 1: CreateTable with a single HASH key, PAY_PER_REQUEST; PutItem an item carrying e.g. {"mylist": {"L":[{"S":"zero"},{"S":"one"}]}, "profile": {"M":{"city":{"S":"sea"}}}, "Safety.Warning": {"S":"flat"}}. Then GetItem with ConsistentRead on that exact Key and (a) ProjectionExpression "mylist[99]", (b) ProjectionExpression "#p.#nope" with ExpressionAttributeNames {"#p":"profile","#nope":"not_a_key"}, (c) ProjectionExpression "Safety.Warning" (unaliased), (d) legacy AttributesToGet=["phone","address"]. Each must return 200 with Item present and equal to {}. Repeat (d) through BatchGetItem (per-table Responses must hold one [{}] entry) and (c) through Query with KeyConditionExpression on the hash key (Count 1, Items [{}]).

Cause 2: CreateTable (HASH pk, PAY_PER_REQUEST); PutItem {pk:'doc', s:'text', m:{x:1}} and {pk:'counter', n:5}; call BatchExecuteStatement with Statements = [UPDATE "<t>" SET s = s + 1 WHERE pk = 'doc', UPDATE "<t>" SET n = n + 1 WHERE pk = 'counter']. The first slot must carry both Error (ValidationError / "An operand in the update expression has an incorrect data type") AND TableName.

Cause 3: Create an S3 bucket with a staged DYNAMODB_JSON object and a table; call StartBulkPutItems with a fresh ClientToken and MaxWriteRequestUnits=1024; call it again with the SAME ClientToken but MaxWriteRequestUnits=2048. The refusal must be 400 IdempotentParameterMismatchException with the message "Specified idempotent token was already used with different request parameters within the idempotency window".

Cause 4: CreateTable with VectorIndexes=[{IndexName:'vec-idx', VectorAttribute:{AttributeName:'embedding'}, Dimensions:3, DistanceFunction:'COSINE', Projection:{ProjectionType:'ALL'}}]; poll DescribeTable until TableStatus and VectorIndexes[0].IndexStatus are both ACTIVE; call ExecuteStatement with Statement = SELECT * FROM "<table>"."vec-idx". The refusal must be 400 ValidationException with the message "Scan operation not supported on this index type".
