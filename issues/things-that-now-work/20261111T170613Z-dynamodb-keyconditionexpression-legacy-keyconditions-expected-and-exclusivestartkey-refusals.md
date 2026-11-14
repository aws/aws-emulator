Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.lsi-query-sort-key-operand-type-mismatch-rejected dynamodb.query-value-first-key-condition-operand-message-states-operator-as-written dynamodb.query-key-condition-undefined-value-vs-bad-operand-precedence dynamodb.query-key-condition-syntax-error-and-attribute-type-function-rejected dynamodb.legacy-keyconditions-disallowed-sort-key-operator-rejected dynamodb.legacy-expected-scalar-comparator-matrix-put-item dynamodb.legacy-expected-scalar-comparator-matrix-update-item dynamodb.legacy-expected-scalar-comparator-matrix-delete-item dynamodb.query-gsi-four-range-key-schema-skipping-two-attributes-rejected dynamodb.gsi-query-exclusive-start-key-missing-an-index-key-attribute-rejected dynamodb.scan-exclusive-start-key-naming-an-attribute-outside-the-key-schema-rejected dynamodb.query-limit-at-partition-count-keeps-lastevaluatedkey-scan-limit-above-count-drops-it dynamodb.delete-item-key-duplicate-set-member-byte-order-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.lsi-query-sort-key-operand-type-mismatch-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Key: dynamodb.query-value-first-key-condition-operand-message-states-operator-as-written|run#9|AssertionError|assert message == expected, f"{value_first_kce!r}: expected message {expected!r}, got {message!r}"
Issue-Key: dynamodb.query-key-condition-undefined-value-vs-bad-operand-precedence|run#14|AssertionError|assert message == _BAD_OPERAND, f"order B (operand first): expected {_BAD_OPERAND!r}, got {message!r}"
Issue-Key: dynamodb.query-key-condition-syntax-error-and-attribute-type-function-rejected|run#14|AssertionError|assert message2 == expected_stray, f"expected message {expected_stray!r}, got {message2!r}"
Issue-Key: dynamodb.legacy-keyconditions-disallowed-sort-key-operator-rejected|run#10|AssertionError|assert message == want, f'{op}: expected AWS exact message, got {message!r}'
Issue-Key: dynamodb.legacy-expected-scalar-comparator-matrix-put-item|run#6|AssertionError|assert succeeded, f"{case_name}: expected the write to land, got {error_code!r}"
Issue-Key: dynamodb.legacy-expected-scalar-comparator-matrix-update-item|run#6|AssertionError|assert succeeded, f"{case_name}: expected the write to land, got {error_code!r}"
Issue-Key: dynamodb.legacy-expected-scalar-comparator-matrix-delete-item|run#6|AssertionError|assert succeeded, f"{case_name}: expected the delete to land, got {error_code!r}"
Issue-Key: dynamodb.query-gsi-four-range-key-schema-skipping-two-attributes-rejected|run#15|AssertionError|assert message == expected, f"skipping gsk2 and gsk3: expected {expected!r}, got {message!r}"
Issue-Key: dynamodb.gsi-query-exclusive-start-key-missing-an-index-key-attribute-rejected|run#17|AssertionError|assert message == "The provided starting key is invalid", (
Issue-Key: dynamodb.scan-exclusive-start-key-naming-an-attribute-outside-the-key-schema-rejected|run#17|AssertionError|assert message == "The provided starting key is invalid", (
Issue-Key: dynamodb.query-limit-at-partition-count-keeps-lastevaluatedkey-scan-limit-above-count-drops-it|run#12|AssertionError|assert 'LastEvaluatedKey' in first, "Limit exactly equal to the partition's item count must still return a LastEvaluatedKey"
Issue-Key: dynamodb.delete-item-key-duplicate-set-member-byte-order-rejected|expect_error#14|AssertionError|assert got_message == message, f"unexpected message {got_message!r}"
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: KeyConditionExpression, legacy KeyConditions/Expected and ExclusiveStartKey refusals diverge from AWS, and Limit drops LastEvaluatedKey (10 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
Ten net-new root causes in the Query/Scan key-condition path: the legacy parameters, the key-condition diagnostics, the exclusive-start-key validation and the pagination contract. Smithy models none of the message text; each refusal is a 400 ValidationException whose message is the contract, and the paging contract is Query/Scan's LastEvaluatedKey member. In every quoted message the attribute names, operator and operand type come from the request; the sentence around them is AWS's fixed text.

Cause 1 of 10 — a sort-key operand whose type does not match the schema type is diagnosed as a parameter error instead of a key-condition error. AWS: 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N'. Emulator: 'One or more parameter values were invalid: Condition parameter type does not match schema type'. Seen on an LSI query. Fix: the key-condition operand type-check diagnostic. Traces: lsi-query-sort-key-operand-type-mismatch-rejected.

Cause 2 of 10 — a value-first key condition (':v >= sk') is rejected as a syntax error. AWS accepts the operand order and, when the operand type is wrong, names the operator AS WRITTEN: 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: >=, operand type: BOOL'. Emulator: 'Invalid KeyConditionExpression: Syntax error; token: ">=", near: ":v >= sk"'. Fix: the key-condition grammar's operand ordering. Traces: query-value-first-key-condition-operand-message-states-operator-as-written.

Cause 3 of 10 — defect precedence between a bad operand type and an undefined expression-attribute value. AWS reports the operand-type defect first; the emulator reports 'An expression attribute value used in expression is not defined; attribute value: :undef'. Fix: ordering in the key-condition validator. Traces: query-key-condition-undefined-value-vs-bad-operand-precedence.

Cause 4 of 10 — the key-condition syntax-error diagnostic names the wrong token. AWS: 'Invalid KeyConditionExpression: Syntax error; token: "=", near: "= = :p"'; emulator: '... token: ":p", near: "= :p"'. Fix: the key-condition parser's error-position reporting. Traces: query-key-condition-syntax-error-and-attribute-type-function-rejected.

Cause 5 of 10 — the legacy KeyConditions disallowed-operator refusal is emulator-authored. Emulator: 'Unsupported operator on KeyConditions: NE'; AWS words it its own way. Fix: the legacy KeyConditions validation message. Traces: legacy-keyconditions-disallowed-sort-key-operator-rejected.

Cause 6 of 10 — the legacy Expected parameter with a scalar comparator is refused where AWS performs the write. AWS: Expected with a satisfied EQ comparator lets the PutItem/UpdateItem/DeleteItem land; the emulator answers ValidationException for the eq_value_comparator_true case on all three operations. Why: the whole legacy conditional-write surface is unusable. Fix: the legacy Expected evaluation in the item-write path. Traces: legacy-expected-scalar-comparator-matrix-put-item, legacy-expected-scalar-comparator-matrix-update-item, legacy-expected-scalar-comparator-matrix-delete-item.

Cause 7 of 10 — a multi-RANGE-key GSI query that skips attributes gets the generic 'Query key condition not supported' instead of AWS's specific sentence 'RANGE key attributes <a>, <b> must have equality conditions specified in the query because a condition is present on key attribute <c>' (attribute names from the request). Fix: the multi-attribute index key-condition validator. Traces: query-gsi-four-range-key-schema-skipping-two-attributes-rejected.

Cause 8 of 10 — ExclusiveStartKey validation collapses every defect into 'The provided starting key is invalid: The provided key element does not match the schema'. AWS words the GSI case (a missing index key attribute) and the base-table case (an attribute outside the key schema) distinctly, naming the attribute. Fix: the ExclusiveStartKey validator's messages. Traces: gsi-query-exclusive-start-key-missing-an-index-key-attribute-rejected, scan-exclusive-start-key-naming-an-attribute-outside-the-key-schema-rejected.

Cause 9 of 10 — pagination: a Query whose Limit exactly equals the partition's item count must still return a LastEvaluatedKey (and a Scan whose Limit exceeds Count must not). The emulator omits LastEvaluatedKey at the boundary. Why: a client paging with Limit stops one page early and silently loses items. Fix: LastEvaluatedKey computation in the Query/Scan read path. Traces: query-limit-at-partition-count-keeps-lastevaluatedkey-scan-limit-above-count-drops-it.

Cause 10 of 10 — a key holding a set with duplicate members / out-of-byte-order members gets the generic 'The provided key element does not match the schema' instead of AWS's duplicate-collection refusal ('One or more parameter values were invalid: Input collection [...] contains duplicates.'). Fix: key attribute-value validation on DeleteItem/GetItem. Traces: delete-item-key-duplicate-set-member-byte-order-rejected.

## Evidence
Expected vs actual, verbatim from the replay:
- lsi-query-sort-key-operand-type-mismatch-rejected: the refusal message is 'One or more parameter values were invalid: Condition parameter type does not match schema type', expected 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N'.
- query-value-first-key-condition-operand-message-states-operator-as-written: 'pk = :p AND :v >= sk': expected 'Invalid KeyConditionExpression: Incorrect operand type for operator or function; operator or function: >=, operand type: BOOL', got 'Invalid KeyConditionExpression: Syntax error; token: ">=", near: ":v >= sk"'.
- query-key-condition-undefined-value-vs-bad-operand-precedence: order B (operand first): expected the operand-type message, got 'Invalid KeyConditionExpression: An expression attribute value used in expression is not defined; attribute value: :undef'.
- query-key-condition-syntax-error-and-attribute-type-function-rejected: expected 'Invalid KeyConditionExpression: Syntax error; token: "=", near: "= = :p"', got '... token: ":p", near: "= :p"'.
- legacy-keyconditions-disallowed-sort-key-operator-rejected: NE: expected AWS exact message, got 'Unsupported operator on KeyConditions: NE'.
- legacy-expected-scalar-comparator-matrix-{put,update,delete}-item: 'eq_value_comparator_true: expected the write/delete to land, got 'ValidationException''.
- query-gsi-four-range-key-schema-skipping-two-attributes-rejected: expected 'RANGE key attributes gsk2, gsk3 must have equality conditions specified in the query because a condition is present on key attribute gsk4', got 'Query key condition not supported'.
- gsi-query-exclusive-start-key-missing-an-index-key-attribute-rejected / scan-exclusive-start-key-naming-an-attribute-outside-the-key-schema-rejected: "expected AWS's exact ValidationException message, got 'The provided starting key is invalid: The provided key element does not match the schema'".
- query-limit-at-partition-count-keeps-lastevaluatedkey-scan-limit-above-count-drops-it: "Limit exactly equal to the partition's item count must still return a LastEvaluatedKey".
- delete-item-key-duplicate-set-member-byte-order-rejected: unexpected message 'The provided key element does not match the schema'.
(Attribute names such as gsk2/gsk3/gsk4 and :undef are this run's request values.)

## Steps to Reproduce
1. Create a table with a HASH+RANGE key (S sort key), an LSI, and a GSI whose KeySchema has one HASH and four RANGE attributes; seed a handful of items in one partition.
2. Query the LSI with begins_with(sk, :n) where :n is an N — read the message.
3. Query with 'pk = :p AND :v >= sk' (:v a BOOL), with the operand-first/undefined-value combination, and with 'pk = = :p' — read code, message and reported token.
4. Query with legacy KeyConditions using NE on the sort key.
5. PutItem/UpdateItem/DeleteItem with a legacy Expected entry whose EQ comparator is satisfied — expect the write to land.
6. Query the four-RANGE GSI with a condition on gsk4 but none on gsk2/gsk3.
7. Query the GSI with an ExclusiveStartKey missing an index key attribute; Scan with an ExclusiveStartKey naming a non-key attribute.
8. Query one partition with Limit equal to its item count — expect LastEvaluatedKey present; Scan with Limit above Count — expect it absent.
9. DeleteItem/GetItem with a key whose SS holds duplicate members.
