Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.update-item-if-not-exists-on-number-types-the-fallback-only-when-chosen dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict dynamodb.bulk-put-items-client-token-replay-returns-one-operation dynamodb.transact-get-items-table-name-outside-length-bounds-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.update-item-if-not-exists-on-number-types-the-fallback-only-when-chosen|run#13|AssertionError|assert refused == ("ValidationException", _BAD_OPERAND), (
Issue-Key: dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict|_error_code_and_message#31|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: dynamodb.bulk-put-items-client-token-replay-returns-one-operation|run#30|KeyError|named = sorted(d["TableName"] for d in described["TableDetails"])
Issue-Key: dynamodb.transact-get-items-table-name-outside-length-bounds-rejected|run#18|AssertionError|assert got == want, f"a 1025-character TableName at position 2 gave {got!r}, expected {want!r}"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb: if_not_exists arithmetic message, an MRSC global table accepting UpdateTimeToLive, DescribeBulkOperation omitting TableDetails, and the TransactItems index in constraint paths (4 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Four further net-new root causes in DynamoDB, distinct from the validation/capacity causes filed alongside this issue.

Cause 1 of 4: arithmetic on a non-numeric `if_not_exists` fallback answers the generic operand-type text instead of DynamoDB's update-expression message.
AWS: adding to a value that resolves to a String is refused with ValidationException and the message `An operand in the update expression has an incorrect data type` — a fixed sentence with no interpolated value.
Emulator: it answers `1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator or function: +, operand type: S` — the Smithy-style constraint form, naming the operator and the AttributeValue tag.
Why: the message is a requirement; a client matching AWS's sentence does not recognise the refusal, and the text implies a different rule (an expression-grammar fault rather than a data-type fault in the resolved operand).
Fix: the UpdateItem expression evaluator's operand-type check.
Traces: dynamodb.update-item-if-not-exists-on-number-types-the-fallback-only-when-chosen.

Cause 2 of 4: a multi-Region strong-consistency (MRSC) global table accepts UpdateTimeToLive.
AWS: an MRSC global table refuses the call with a ClientError (HTTP 400) — TTL is not configurable while the table is in an MRSC replication group.
Emulator: UpdateTimeToLive returns 200 with `{'TimeToLiveSpecification': {'Enabled': True, 'AttributeName': 'ttl'}}`.
Why: a configuration AWS forbids is applied, so the emulator reports a TTL that cannot exist on that table and a client never learns the setting was invalid.
Fix: the UpdateTimeToLive handler's replication-group guard.
Traces: dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict.

Cause 3 of 4: DescribeBulkOperation's BulkOperationDescription omits TableDetails.
AWS: the operation description returned by StartBulkPutItems and read back by DescribeBulkOperation carries the per-table detail list, so a caller can confirm which tables the operation writes to; alongside it the ClientToken and MaxWriteRequestUnits it was started with.
Emulator: the response has no `TableDetails` member at all — the caller gets a KeyError reading it — so the tables of a running bulk operation cannot be read back.
Why: a client cannot verify what a bulk operation it replayed by client token is actually writing to, which is exactly the check the idempotent-replay contract needs.
Fix: the DescribeBulkOperation (and StartBulkPutItems) response serializer — populate BulkOperationDescription.TableDetails from the operation's state.
Traces: dynamodb.bulk-put-items-client-token-replay-returns-one-operation.

Cause 4 of 4: the TransactItems index in a constraint-violation member path does not match AWS's.
AWS: for an over-long TableName inside TransactGetItems the member path AWS reports is `TransactItems.1.member.Get.TableName` for the offending entry.
Emulator: it reports `TransactItems.2.member.Get.TableName` — the index it writes into the path is one greater than AWS's for the same request, so the path points at the wrong entry.
Why: the member path is the only thing telling a caller which transaction entry is bad; an off-by-one sends them to the wrong item.
Fix: the ValidationException member-path builder for list members under TransactItems.
Traces: dynamodb.transact-get-items-table-name-outside-length-bounds-rejected.

## Evidence
Cause 1: expected `('ValidationException', 'An operand in the update expression has an incorrect data type')`; got `('ValidationException', '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator or function: +, operand type: S')`.
Cause 2: `expected a ClientError, but the call succeeded and returned {'TimeToLiveSpecification': {'Enabled': True, 'AttributeName': 'ttl'}, ... 'HTTPStatusCode': 200}`.
Cause 3: `KeyError: 'TableDetails'` reading `describe_bulk_operation(...)['BulkOperationDescription']['TableDetails']`.
Cause 4: got `"1 validation error detected: Value at 'TransactItems.2.member.Get.TableName' failed to satisfy constraint: Member must have length less than or equal to 1024"`, expected the same text with `TransactItems.1.member.Get.TableName`.

## Steps to Reproduce
1. UpdateItem with `SET n = if_not_exists(missing, :string) + :one` where the fallback is a String.
2. Create an MRSC global table and call UpdateTimeToLive on it.
3. StartBulkPutItems with a client token, then DescribeBulkOperation on the returned BulkOperationArn and read BulkOperationDescription.TableDetails.
4. TransactGetItems whose entry carries a 1025-character TableName; read the member path in the ValidationException message.
