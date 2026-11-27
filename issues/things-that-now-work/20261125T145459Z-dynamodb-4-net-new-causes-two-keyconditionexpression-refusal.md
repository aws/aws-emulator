Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.key-condition-illegal-sort-key-operator-rejected dynamodb.query-key-condition-undefined-value-vs-bad-operand-precedence dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes dynamodb.update-remove-through-missing-nested-parent-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.key-condition-illegal-sort-key-operator-rejected|run#25|AssertionError|assert message == expected, (
Issue-Key: dynamodb.query-key-condition-undefined-value-vs-bad-operand-precedence|run#14|AssertionError|assert message == _BAD_OPERAND, f"order B (operand first): expected {_BAD_OPERAND!r}, got {message!r}"
Issue-Key: dynamodb|UpdateItem|ValidationException
Issue-Commit: 0970c67cd532b68a1462dc1d98bdb119ad45d0ac

# dynamodb: 4 net-new causes — two KeyConditionExpression refusal divergences, an over-strict UpdateItem size ceiling, and a REMOVE of a nested path on an absent item refused instead of being a no-op

> _Note: this issue was identified against the code base at commit `0970c67cd532b68a1462dc1d98bdb119ad45d0ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Four distinct root causes in the DynamoDB data plane. Smithy (dynamodb 2012-08-10) models neither ValidationException nor any message text for Query/UpdateItem (the ops' modeled errors are ConditionalCheckFailed/ResourceNotFound/Throughput/Transaction classes), so each refusal's 400 ValidationException code and its message wording below are recorded behavior only; the fixed text is the requirement and the variable parts come from the request.

Cause 1 of 4: Query refuses a BETWEEN whose bounds are of different types with its own sentence instead of AWS's operand-type sentence.
AWS: a KeyConditionExpression using BETWEEN on an S sort key with a non-S bound is refused 400 ValidationException with the operand-type message: "Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: BETWEEN, operand type: BOOL" — fixed text "Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: " + the operator from the expression + ", operand type: " + the offending AttributeValue's type tag from the request. Recorded behavior only (no model text).
Emulator: answers a different sentence that leaks the two operands: "Invalid KeyConditionExpression: The BETWEEN operator requires same data type for lower and upper bounds; lower bound operand: AttributeValue: {BOOL:}, upper bound operand: AttributeValue: {S:z}".
Why: a client matching on AWS's operand-type message (or parsing the operator/operand-type fields out of it) does not recognize the refusal, and the message exposes operand values AWS does not put there.
Fix: the Query handler's KeyConditionExpression validation/message builder.
Traces: dynamodb.key-condition-illegal-sort-key-operator-rejected.

Cause 2 of 4: Query reports the wrong one of two coexisting KeyConditionExpression defects — validation order.
AWS: when a key condition contains both a begins_with whose operand is of the wrong type (N against an S sort key) and an undefined expression attribute value, the operand-type defect is reported: "Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N", independent of the order the two conditions appear in the expression. Recorded behavior only.
Emulator: reports the undefined-value defect instead — "Invalid KeyConditionExpression: An expression attribute value used in expression is not defined; attribute value: :undef" — i.e. it resolves placeholders before it type-checks operands, so the precedence inverts for the operand-first ordering.
Why: a client fixing the error it is told about fixes the wrong defect and gets a second round-trip; refusal text is not stable against AWS for the same request.
Fix: the ordering of the key-condition validation passes in the Query handler's expression validation (type-check operands before/ahead of the undefined-placeholder check, as AWS does).
Traces: dynamodb.query-key-condition-undefined-value-vs-bad-operand-precedence.

Cause 3 of 4: a standalone UpdateItem is refused "Item size to update has exceeded the maximum allowed size" exactly AT the boundary AWS accepts.
AWS: for a bare "SET v = :v" on an item with no v, the accepted length of the string value is 409600 - len("v") - max(22, bytes of every other attribute in the item: partition key name+value, sort key name+value and any untouched attribute, none excluded) — the boundary value is accepted and only one byte more is refused 400 ValidationException "Item size to update has exceeded the maximum allowed size" (documented quota: ServiceQuotas.html#limits-items; message is recorded behavior only). The rule is identical in every region.
Emulator: refuses the write AT that boundary: the UpdateItem at the computed ceiling raises ValidationException "Item size to update has exceeded the maximum allowed size" instead of succeeding, so its ceiling is stricter than AWS's (it charges bytes AWS does not, or applies the 22-byte floor differently).
Why: writes AWS accepts are rejected; any client sizing an attribute against the documented 400 KB item limit loses usable capacity and sees a false ValidationException.
Fix: the UpdateItem handler's item-size accounting (the pre-existing-attribute byte sum and the flat-floor term) that gates the too-large refusal.
Traces: dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes.

Cause 4 of 4: UpdateItem REMOVE of a nested path is refused against a key that does not exist, where AWS makes it a no-op.
AWS: "REMOVE absent.child" against a key with no stored item is accepted — there is nothing to validate the document path against and REMOVE never upserts, so the call succeeds and the item still does not exist. The document-path refusal 400 ValidationException "The document path provided in the update expression is invalid for update" applies only when the item EXISTS and its parent attribute is absent.
Emulator: the call against the nonexistent key is refused with 400 ValidationException "The document path provided in the update expression is invalid for update" (the error escapes the check uncaught at the no-op step), i.e. the path is validated as though the item existed.
Why: idempotent cleanup code that removes an optional nested field without first reading the item fails against the emulator while it succeeds on AWS.
Fix: the UpdateItem handler's update-expression path validation / REMOVE application for the missing-item case.
Traces: dynamodb.update-remove-through-missing-nested-parent-rejected.

Note: the open issues 20261117T172826Z-dynamodb-expression-evaluation-10-net-new-causes-transaction.md (expression evaluation) and 20261118T031500Z-dynamodb-a-secondary-index-table-never-reaches-active-and-updateitem-clause-overhead.md (UpdateItem clause overhead) touch neighbouring surfaces but do not carry any of these four checks' replay keys.

## Evidence
Cause 1 — expected: 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: BETWEEN, operand type: BOOL'; actual: 'Invalid KeyConditionExpression: The BETWEEN operator requires same data type for lower and upper bounds; lower bound operand: AttributeValue: {BOOL:}, upper bound operand: AttributeValue: {S:z}' (code ValidationException in both).

Cause 2 — expected (operand-first ordering): 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N'; actual: 'Invalid KeyConditionExpression: An expression attribute value used in expression is not defined; attribute value: :undef'.

Cause 3 — expected: UpdateItem "SET v = :v" with a value of length 409600 - 1 - max(22, pre-existing attribute bytes) succeeds and reads back at that length; actual: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the UpdateItem operation: Item size to update has exceeded the maximum allowed size — raised at the accept side of the boundary (probed with a 1-byte key, a 22-byte and a 23-byte pre-existing footprint, 100- and 1024-byte key values, a 100-byte key name and a 100-byte sort key, in both us-west-2 and us-east-1).

Cause 4 — expected: UpdateItem(Key={pk:{S:'nowhere'}}, UpdateExpression='REMOVE absent.child') on a key with no item succeeds and creates nothing; actual: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the UpdateItem operation: The document path provided in the update expression is invalid for update.

## Steps to Reproduce
Cause 1: create a table with an S partition key and S sort key; Query with KeyConditionExpression 'pk = :p AND sk BETWEEN :lo AND :hi' where :lo is {"BOOL": false} and :hi is {"S": "z"}; read Error.Message.
Cause 2: Query the same table with a key condition containing begins_with(sk, :n) where :n is an N value AND a second comparison using an expression attribute value that is not defined (:undef), with the bad operand appearing first; read Error.Message.
Cause 3: create a PAY_PER_REQUEST table with partition key k (S); PutItem {k:'0'}; UpdateItem Key={k:'0'}, UpdateExpression='SET v = :v' with :v a string of length 409600 - 1 - 22; observe the ValidationException where AWS accepts it (repeat with an extra 19- and 20-byte attribute, a 100/1024-byte key value, a 100-byte key name, and a 100-byte sort key).
Cause 4: create a table with partition key pk (S); PutItem {pk:'doc', keep:'k', box:{M:{hits:1}}}; UpdateItem Key={pk:'nowhere'} UpdateExpression='REMOVE absent.child'; observe the ValidationException where AWS returns a successful no-op.
