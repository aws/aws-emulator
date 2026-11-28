Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-get-item-attributes-to-get-name-over-65535-rejected dynamodb.batch-get-item-expression-attribute-name-over-65535-characters-rejected dynamodb.delete-item-expression-attribute-name-value-over-65535-rejected dynamodb.transact-condition-check-name-substitute-length-bounds-rejected dynamodb.transact-delete-expression-attribute-name-over-65535-chars-rejected dynamodb.transact-get-items-attribute-name-value-length-bounds-rejected dynamodb.transact-write-put-condition-name-placeholder-over-65535-rejected dynamodb.transact-write-update-set-name-placeholder-over-65535-rejected dynamodb.create-table-tag-value-over-256-characters-rejected dynamodb.untag-resource-tag-key-length-bounds-rejected dynamodb.create-backup-table-name-length-bounds-rejected dynamodb.delete-backup-malformed-arn-rejected-at-every-length dynamodb.describe-contributor-insights-index-name-length-bounds-rejected dynamodb.describe-import-arn-outside-length-bounds-rejected-as-invalid dynamodb.execute-statement-next-token-over-32768-rejected dynamodb.list-tags-of-resource-arn-length-bounds-rejected dynamodb.delete-table-arn-over-1024-chars-rejected dynamodb.transact-write-items-update-table-name-over-1024-rejected dynamodb.create-table-lsi-projection-type-outside-enum-rejected dynamodb.delete-item-lowercase-conditional-operator-rejected dynamodb.list-backups-lowercase-backup-type-rejected dynamodb.query-key-conditions-comparison-operator-outside-enum-rejected dynamodb.scan-filter-comparison-operator-outside-enum-rejected dynamodb.update-item-attribute-updates-unknown-action-rejected dynamodb.update-table-vector-index-unknown-distance-function-rejected dynamodb.vector-index-unlisted-distance-function-rejected dynamodb.create-table-gsi-include-over-twenty-non-key-attributes-rejected dynamodb.update-table-vector-index-include-attribute-count-out-of-bounds-rejected dynamodb.batch-execute-statement-map-parameter-key-over-65535-rejected dynamodb.create-table-vector-index-twenty-include-names-rejected dynamodb.execute-statement-parameter-map-key-length-bounds-rejected dynamodb.execute-transaction-parameter-map-key-over-65535-chars-rejected dynamodb.put-item-nested-map-key-byte-limit-rejected dynamodb.scan-filter-map-key-over-65535-characters-rejected dynamodb.transact-put-condition-value-map-key-length-bound-rejected dynamodb.transact-put-map-key-over-65535-chars-rejected dynamodb.update-item-attribute-updates-name-length-rejected dynamodb.batch-get-item-with-no-read-capacity-for-any-key-rejected dynamodb.delete-item-of-large-item-on-drained-one-wcu-table-rejected dynamodb.delete-item-of-large-item-on-drained-table-rejected dynamodb.execute-transaction-on-large-item-throttled-at-one-wcu-rejected dynamodb.get-item-past-read-capacity-names-table-read-reason-rejected dynamodb.update-item-increment-on-large-item-throttled-at-one-wcu-rejected dynamodb.partiql-insert-over-one-wcu-throttled-until-on-demand-rejected dynamodb.transact-write-on-drained-one-wcu-table-rejected dynamodb.gsi-still-creating-refuses-query-and-scan dynamodb.update-table-vector-index-search-schema-range-type-rejected dynamodb.update-table-vector-index-zero-dimensions-rejected dynamodb.batch-write-item-one-table-keyed-by-name-and-arn-over-25-rejected dynamodb.update-delete-from-binary-scalar-attribute-rejected dynamodb.update-item-delete-action-subtracts-members-from-binary-set dynamodb.update-item-delete-list-operand-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-get-item-attributes-to-get-name-over-65535-rejected|run#17|AssertionError|assert got == want, (
Issue-Key: dynamodb.batch-get-item-expression-attribute-name-over-65535-characters-rejected|run#17|AssertionError|assert got == expected, (
Issue-Key: dynamodb.delete-item-expression-attribute-name-value-over-65535-rejected|run#10|AssertionError|assert got == (
Issue-Key: dynamodb.transact-condition-check-name-substitute-length-bounds-rejected|run#13|AssertionError|assert over == want_over, f"a {_LONGEST + 1}-character substitute gave {over!r}, expected {want_over!r}"
Issue-Key: dynamodb.transact-delete-expression-attribute-name-over-65535-chars-rejected|run#13|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: dynamodb.transact-get-items-attribute-name-value-length-bounds-rejected|run#17|AssertionError|assert (code, message) == ("ValidationException", expected_over), (
Issue-Key: dynamodb.transact-write-put-condition-name-placeholder-over-65535-rejected|run#14|AssertionError|assert message == want_message, (
Issue-Key: dynamodb.transact-write-update-set-name-placeholder-over-65535-rejected|run#12|AssertionError|assert message == want_message, (
Issue-Key: dynamodb.create-table-tag-value-over-256-characters-rejected|run#17|AssertionError|assert got == expected, f"257-character tag value: expected {expected!r}, got {got!r}"
Issue-Key: dynamodb.untag-resource-tag-key-length-bounds-rejected|run#11|AssertionError|assert empty == want_empty, f"untagging an empty key answered {empty!r}, expected {want_empty!r}"
Issue-Key: dynamodb.create-backup-table-name-length-bounds-rejected|run#9|AssertionError|assert got == (
Issue-Key: dynamodb.delete-backup-malformed-arn-rejected-at-every-length|run#19|AssertionError|assert answers == want, f"malformed BackupArn answers by length were {answers!r}, expected {want!r}"
Issue-Key: dynamodb.describe-contributor-insights-index-name-length-bounds-rejected|run#13|AssertionError|assert got == (
Issue-Key: dynamodb.describe-import-arn-outside-length-bounds-rejected-as-invalid|run#10|AssertionError|assert got == _INVALID, f"a 1025-character import ARN was answered {got!r}"
Issue-Key: dynamodb.execute-statement-next-token-over-32768-rejected|run#17|AssertionError|assert got[0] == want[0] and got == want, (
Issue-Key: dynamodb.list-tags-of-resource-arn-length-bounds-rejected|run#9|AssertionError|assert got == (
Issue-Key: dynamodb.delete-table-arn-over-1024-chars-rejected|run#17|AssertionError|assert message.endswith(
Issue-Key: dynamodb.transact-write-items-update-table-name-over-1024-rejected|run#10|AssertionError|assert got[0] == want[0] and got == want, (
Issue-Key: dynamodb.create-table-lsi-projection-type-outside-enum-rejected|run#6|AssertionError|assert got == (
Issue-Key: dynamodb.delete-item-lowercase-conditional-operator-rejected|run#10|AssertionError|assert got == (
Issue-Key: dynamodb.list-backups-lowercase-backup-type-rejected|run#22|AssertionError|assert got == expected, f"BackupType 'user': expected {expected!r}, got {got!r}"
Issue-Key: dynamodb.query-key-conditions-comparison-operator-outside-enum-rejected|run#13|AssertionError|assert got == want, f"ComparisonOperator STARTS_WITH: expected {want!r}, got {got!r}"
Issue-Key: dynamodb.scan-filter-comparison-operator-outside-enum-rejected|run#8|AssertionError|assert got == (
Issue-Key: dynamodb.update-item-attribute-updates-unknown-action-rejected|run#7|AssertionError|assert got == want, f"Action {action!r}: expected {want!r}, got {got!r}"
Issue-Key: dynamodb.update-table-vector-index-unknown-distance-function-rejected|run#12|AssertionError|assert got == (
Issue-Key: dynamodb.vector-index-unlisted-distance-function-rejected|run#8|AssertionError|assert got == want, f"an unlisted distance function gave {got!r}, expected {want!r}"
Issue-Key: dynamodb.create-table-gsi-include-over-twenty-non-key-attributes-rejected|run#19|AssertionError|assert got == want, f"21 non-key attributes gave {got!r}, expected {want!r}"
Issue-Key: dynamodb.update-table-vector-index-include-attribute-count-out-of-bounds-rejected|run#17|AssertionError|assert too_many == want_max, f"21 NonKeyAttributes: expected {want_max!r}, got {too_many!r}"
Issue-Key: dynamodb.batch-execute-statement-map-parameter-key-over-65535-rejected|run#21|AssertionError|assert err is not None, f"a {_LIMIT + 1}-character map key must fail its slot; slot was {str(slots[0])[:200]!r}"
Issue-Key: dynamodb.create-table-vector-index-twenty-include-names-rejected|_refusal#5|AssertionError|raise AssertionError(f"the call was accepted where a refusal was expected: {result!r}")
Issue-Key: dynamodb.execute-statement-parameter-map-key-length-bounds-rejected|_refusal#15|AssertionError|raise AssertionError(f"the statement was accepted but must be refused; it returned {sorted(out)!r}")
Issue-Key: dynamodb.execute-transaction-parameter-map-key-over-65535-chars-rejected|_refusal#13|AssertionError|raise AssertionError(f"ExecuteTransaction was expected to be refused but returned {answer!r}")
Issue-Key: dynamodb.put-item-nested-map-key-byte-limit-rejected|_refusal#13|AssertionError|raise AssertionError(f"the write was expected to be refused but succeeded: {answer!r}")
Issue-Key: dynamodb.scan-filter-map-key-over-65535-characters-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected the request to be refused, but it answered {result!r}")
Issue-Key: dynamodb.transact-put-condition-value-map-key-length-bound-rejected|run#17|AssertionError|assert (code, message) == (
Issue-Key: dynamodb.transact-put-map-key-over-65535-chars-rejected|_refusal#13|AssertionError|raise AssertionError(f"the transaction was accepted: {answer!r}")
Issue-Key: dynamodb.update-item-attribute-updates-name-length-rejected|_refused_with#14|AssertionError|raise AssertionError(f"UpdateItem was accepted where a refusal was expected: {resp!r}")
Issue-Key: dynamodb|PutItem|ProvisionedThroughputExceededException
Issue-Key: dynamodb.delete-item-of-large-item-on-drained-table-rejected|run#17|AssertionError|assert stored, 'the first large write was refused, so there is no large item to delete'
Issue-Key: dynamodb.partiql-insert-over-one-wcu-throttled-until-on-demand-rejected|run#15|AssertionError|assert refusal is not None, (
Issue-Key: dynamodb.transact-write-on-drained-one-wcu-table-rejected|_refusal#13|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.gsi-still-creating-refuses-query-and-scan|expect_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb|UpdateTable|ResourceInUseException
Issue-Key: dynamodb.batch-write-item-one-table-keyed-by-name-and-arn-over-25-rejected|run#12|AssertionError|assert after_control in (by_name, by_arn), (
Issue-Key: dynamodb.update-delete-from-binary-scalar-attribute-rejected|run#12|AssertionError|assert got == ("ValidationException", _SCALAR_OPERAND), (
Issue-Key: dynamodb.update-item-delete-action-subtracts-members-from-binary-set|run#15|AssertionError|assert got == want, f'DELETE with a scalar binary operand answered {got!r}, expected {want!r}'
Issue-Key: dynamodb.update-item-delete-list-operand-rejected|run#12|AssertionError|assert got == _LIST_OPERAND, f"DELETE with a list operand answered {got!r}"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb: ValidationException texts are generated from the Smithy constraint instead of DynamoDB's own messages, several length limits are not enforced at all, and provisioned-capacity metering is wrong in both directions (10 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Ten net-new root causes in DynamoDB. Open issues 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md, 20261120T225636Z-dynamodb-validationexception-envelope-diverges-member-paths-value-rendering.md and 20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md touch the same area but none of them carries any of the replay keys below.

Cause 1 of 10: where AWS answers a length/ARN violation with its own sentence, the emulator emits the generic Smithy constraint text.
AWS: DynamoDB has bespoke messages for these classes, and they do NOT use the `N validation error(s) detected:` envelope: an over-long expression-attribute name or AttributesToGet entry is `Attribute name is too large, must be less than 65536 bytes for key <placeholder>` (the placeholder, e.g. `#wide`, comes from the request); a malformed BackupArn is `Invalid Backup ARN` at every length, including below the modeled minimum and above the maximum; an over-long tag value is `The Tag Value provided is invalid, Value: <value>`; an empty tag key on UntagResource is `The Tag Key provided is invalid, Key: <key>`. Status 400, code ValidationException.
Emulator: every one of these answers the generic constraint text, e.g. `1 validation error detected: Value at 'RequestItems.<table>.member.ExpressionAttributeNames.#wide' failed to satisfy constraint: Member must have length less than or equal to 65535`, and for the ARN cases it only falls back to the modeled length bound outside [37,1024] while using `Invalid Backup ARN` inside it. The same substitution happens on CreateBackup TableName, DeleteTable/ListTagsOfResource/DescribeImport ARNs, DescribeContributorInsights IndexName, ExecuteStatement NextToken and TransactWriteItems TableName.
Why: clients and operators match on DynamoDB's documented sentences; the generic text exposes internal member paths and conveys a different rule (a length bound rather than "this ARN is not a backup ARN" or "this tag value is invalid").
Traces: dynamodb.batch-get-item-attributes-to-get-name-over-65535-rejected, dynamodb.batch-get-item-expression-attribute-name-over-65535-characters-rejected, dynamodb.delete-item-expression-attribute-name-value-over-65535-rejected, dynamodb.transact-condition-check-name-substitute-length-bounds-rejected, dynamodb.transact-delete-expression-attribute-name-over-65535-chars-rejected, dynamodb.transact-get-items-attribute-name-value-length-bounds-rejected, dynamodb.transact-write-put-condition-name-placeholder-over-65535-rejected, dynamodb.transact-write-update-set-name-placeholder-over-65535-rejected, dynamodb.create-table-tag-value-over-256-characters-rejected, dynamodb.untag-resource-tag-key-length-bounds-rejected, dynamodb.create-backup-table-name-length-bounds-rejected, dynamodb.delete-backup-malformed-arn-rejected-at-every-length, dynamodb.describe-contributor-insights-index-name-length-bounds-rejected, dynamodb.describe-import-arn-outside-length-bounds-rejected-as-invalid, dynamodb.execute-statement-next-token-over-32768-rejected, dynamodb.list-tags-of-resource-arn-length-bounds-rejected, dynamodb.delete-table-arn-over-1024-chars-rejected, dynamodb.transact-write-items-update-table-name-over-1024-rejected.

Cause 2 of 10: enum-violation ValidationException texts carry the wrong prefix, the wrong enum member order, or lose the value and path.
AWS: an out-of-enum member is refused with `Value '<value>' at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [<members in the model's declared order>]` — without a `1 validation error detected: ` prefix for these data-plane members — and the member list is rendered in the model's order (e.g. `[ALL, AWS_BACKUP, USER, SYSTEM]` for BackupTypeFilter, `[ADD, DELETE, PUT]` for AttributeAction, `[DOT_PRODUCT, COSINE, EUCLIDEAN]` for the vector distance function).
Emulator: it prefixes `1 validation error detected: ` on Query KeyConditions / Scan ScanFilter ComparisonOperator, LSI ProjectionType and the vector DistanceFunction; it re-orders the enum members (`[USER, SYSTEM, AWS_BACKUP, ALL]`, `[ADD, PUT, DELETE]`, `[COSINE, DOT_PRODUCT, EUCLIDEAN]`); and for a lowercase DeleteItem ConditionalOperator it drops the value and path entirely, answering `1 validation error detected: Failed to satisfy constraint: Member must satisfy enum value set: [ALL, OR]`.
Why: the error message is a requirement; a client that string-matches the documented text, or an operator comparing the allowed list, reads something AWS never emits, and the ConditionalOperator case does not even name which member was wrong.
Traces: dynamodb.create-table-lsi-projection-type-outside-enum-rejected, dynamodb.delete-item-lowercase-conditional-operator-rejected, dynamodb.list-backups-lowercase-backup-type-rejected, dynamodb.query-key-conditions-comparison-operator-outside-enum-rejected, dynamodb.scan-filter-comparison-operator-outside-enum-rejected, dynamodb.update-item-attribute-updates-unknown-action-rejected, dynamodb.update-table-vector-index-unknown-distance-function-rejected, dynamodb.vector-index-unlisted-distance-function-rejected.

Cause 3 of 10: a list-length violation renders the offending collection as a count instead of its members.
AWS: `Value '[p00, p01, ..., p20]' at '<path>' failed to satisfy constraint: Member must have length less than or equal to 20` — the rejected value is rendered as the bracketed, comma-separated member list taken from the request.
Emulator: it renders `Value '21 items' at '<path>' ...` — a count, not the value.
Why: the message no longer shows which attributes were supplied, so an author of a 20-plus projection list cannot see what to trim.
Traces: dynamodb.create-table-gsi-include-over-twenty-non-key-attributes-rejected, dynamodb.update-table-vector-index-include-attribute-count-out-of-bounds-rejected.

Cause 4 of 10: several name/key length limits are not enforced at all — the request succeeds.
AWS: a map key or attribute name over 65535 bytes (in an item, a nested map, an expression-attribute-value map, a PartiQL parameter map, a ScanFilter map, an AttributeUpdates name or a TransactWriteItems condition value) is refused with ValidationException, HTTP 400; a vector index with 20 INCLUDE non-key attribute names is likewise refused.
Emulator: the call is accepted and returns 200 — PutItem with an over-long nested map key stores the item, ExecuteStatement/BatchExecuteStatement/ExecuteTransaction with an over-long parameter map key return normally, Scan with an over-long ScanFilter map key answers `[]`, UpdateItem with an over-long AttributeUpdates name succeeds, TransactWriteItems with an over-long map key commits, and a TransactWriteItems condition-value map key over the limit comes back as `TransactionCanceledException` / `Transaction cancelled, please refer cancellation reasons for specific reasons [None, ConditionalCheckFailed]` rather than a ValidationException.
Why: data AWS refuses is written and stored, so an application developed against the emulator fails only in production; the TransactWriteItems case additionally reports a condition failure that did not happen.
Traces: dynamodb.batch-execute-statement-map-parameter-key-over-65535-rejected, dynamodb.create-table-vector-index-twenty-include-names-rejected, dynamodb.execute-statement-parameter-map-key-length-bounds-rejected, dynamodb.execute-transaction-parameter-map-key-over-65535-chars-rejected, dynamodb.put-item-nested-map-key-byte-limit-rejected, dynamodb.scan-filter-map-key-over-65535-characters-rejected, dynamodb.transact-put-condition-value-map-key-length-bound-rejected, dynamodb.transact-put-map-key-over-65535-chars-rejected, dynamodb.update-item-attribute-updates-name-length-rejected.

Cause 5 of 10: a provisioned table has no initial/burst write capacity, so the very first PutItem is throttled.
AWS: a freshly created provisioned table starts with capacity available (and accumulated burst credit), so a single PutItem of a large item against a 1-WCU table succeeds; throttling only begins once the sustained rate exceeds the provisioning.
Emulator: the setup PutItem is refused with `ProvisionedThroughputExceededException` / `The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API.` after the SDK's retries are exhausted, so six checks cannot even seed their item.
Why: writing a single item to a newly provisioned table fails. Any workload that provisions small and writes occasionally is unusable, and the throttling behaviour that was to be measured afterwards cannot be reached.
Traces: dynamodb.batch-get-item-with-no-read-capacity-for-any-key-rejected, dynamodb.delete-item-of-large-item-on-drained-one-wcu-table-rejected, dynamodb.delete-item-of-large-item-on-drained-table-rejected, dynamodb.execute-transaction-on-large-item-throttled-at-one-wcu-rejected, dynamodb.get-item-past-read-capacity-names-table-read-reason-rejected, dynamodb.update-item-increment-on-large-item-throttled-at-one-wcu-rejected.

Cause 6 of 10: the PartiQL and TransactWriteItems write paths are not metered against provisioned capacity at all.
AWS: an INSERT through ExecuteStatement and a write through TransactWriteItems consume the table's write capacity like any other write; sustained large writes on a 1-WCU provisioned table are refused with ProvisionedThroughputExceededException until the table is switched to on-demand.
Emulator: twelve back-to-back ~341 KB PartiQL INSERTs on a 1-WCU table were all accepted, and TransactWriteItems on a drained 1-WCU table returned 200.
Why: together with cause 5 the capacity model is inconsistent — the item-API path throttles writes AWS accepts while the PartiQL/transaction paths accept writes AWS throttles — so no capacity behaviour can be relied on.
Traces: dynamodb.partiql-insert-over-one-wcu-throttled-until-on-demand-rejected, dynamodb.transact-write-on-drained-one-wcu-table-rejected.

Cause 7 of 10: a global secondary index that is still CREATING serves Query and Scan.
AWS: querying or scanning a GSI whose IndexStatus is CREATING (backfilling) is refused — the index cannot be read until it is ACTIVE.
Emulator: the Query/Scan succeeds and returns rows (`{'Count': 50, 'ScannedCount': 50}`, HTTP 200).
Why: an application reads a half-built index and gets partial results it believes are complete, and the refusal that tells a client to wait never arrives.
Traces: dynamodb.gsi-still-creating-refuses-query-and-scan.

Cause 8 of 10: a vector index never leaves CREATING, so the table cannot be updated again.
AWS: a vector index created by CreateTable/UpdateTable transitions CREATING → ACTIVE; a subsequent UpdateTable that creates another vector index with an invalid SearchSchema key type or zero Dimensions is refused with ValidationException.
Emulator: the follow-up UpdateTable is refused with `ResourceInUseException` / `Attempt to change a resource which is still in use: Index creation is in resource allocation phase. Retry deletion during backfilling phase or when the index is active. Table: <table> Index: <index>` — the index stays in the allocation phase indefinitely, so the validation under test is unreachable.
Why: a table with a vector index is permanently frozen against further UpdateTable calls, and the validation error AWS raises is replaced by a resource-state error.
Traces: dynamodb.update-table-vector-index-search-schema-range-type-rejected, dynamodb.update-table-vector-index-zero-dimensions-rejected.

Cause 9 of 10: BatchWriteItem accepts one table addressed twice — once by name and once by ARN — and applies both entries.
AWS: `RequestItems` is keyed by table; naming the same table by name and by ARN in one request is not two tables, so the request is refused (and the 25-item ceiling applies to the single table), leaving the table holding only one entry's keys.
Emulator: both entries are applied — the table ends up holding all 13 ARN-keyed and all 12 name-keyed items, 25 in total.
Why: a request AWS refuses writes twice as much data as the caller asked for, and the per-request item ceiling is effectively doubled.
Traces: dynamodb.batch-write-item-one-table-keyed-by-name-and-arn-over-25-rejected.

Cause 10 of 10: the UpdateExpression DELETE action reports the ADD action's allowed type set and upper-cases the operand type name.
AWS: a DELETE clause with an operand that is not a set is refused with `1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: DELETE, operand type: <Type>, typeSet: ALLOWED_FOR_DELETE_OPERAND` — the type name uses the AttributeValue spelling (`Binary`, `List`) and the type set names the DELETE operand set.
Emulator: it answers `operand type: BINARY, typeSet: ALLOWED_FOR_ADD_OPERAND` (and `operand type: LIST, typeSet: ALLOWED_FOR_ADD_OPERAND`) — the operand type is upper-cased and the type set is always the ADD one, whatever the operator.
Why: the message names the wrong operator's rule, so a caller debugging a DELETE clause is pointed at ADD's allowed types.
Traces: dynamodb.update-delete-from-binary-scalar-attribute-rejected, dynamodb.update-item-delete-action-subtracts-members-from-binary-set, dynamodb.update-item-delete-list-operand-rejected.

Fix for all ten: the DynamoDB request-validation component (causes 1–4, 10 — the ValidationException message builder and the per-operation length checks), the capacity-metering component (causes 5, 6), the table/index status machine (causes 7, 8) and the BatchWriteItem RequestItems key normalisation (cause 9).

## Evidence
Cause 1: got `('ValidationException', "1 validation error detected: Value at 'RequestItems.<table>.member.ExpressionAttributeNames.#wide' failed to satisfy constraint: Member must have length less than or equal to 65535")`, expected `('ValidationException', 'Attribute name is too large, must be less than 65536 bytes for key #wide')`; DeleteBackup answered by length `{36: length-bound text, 37: 'Invalid Backup ARN', 1024: 'Invalid Backup ARN', 1025: length-bound text}`, expected `Invalid Backup ARN` at all four.
Cause 2: got `"1 validation error detected: Value 'STARTS_WITH' at 'keyConditions.leg.member.comparisonOperator' failed ..."`, expected the same without the prefix; got enum set `[USER, SYSTEM, AWS_BACKUP, ALL]`, expected `[ALL, AWS_BACKUP, USER, SYSTEM]`; got `[ADD, PUT, DELETE]`, expected `[ADD, DELETE, PUT]`; got `[COSINE, DOT_PRODUCT, EUCLIDEAN]`, expected `[DOT_PRODUCT, COSINE, EUCLIDEAN]`; got `1 validation error detected: Failed to satisfy constraint: Member must satisfy enum value set: [ALL, OR]` with no value or path.
Cause 3: got `Value '21 items' at 'globalSecondaryIndexes.1.member.projection.nonKeyAttributes' ...`, expected `Value '[p00, p01, ..., p20]' at ... `.
Cause 4: `the write was expected to be refused but succeeded` / `the statement was accepted but must be refused` / `expected the request to be refused, but it answered []`; TransactWriteItems gave `TransactionCanceledException: Transaction cancelled, please refer cancellation reasons for specific reasons [None, ConditionalCheckFailed]`.
Cause 5: `botocore.errorfactory.ProvisionedThroughputExceededException: ... when calling the PutItem operation (reached max retries: 3): The level of configured provisioned throughput for the table was exceeded.` on the setup write.
Cause 6: `12 back-to-back ~341 KB INSERTs on a 1-WCU table were all accepted; provisioned write capacity is not enforced`; TransactWriteItems on a drained 1-WCU table returned HTTPStatusCode 200.
Cause 7: `expected the call to be refused, but it succeeded: {'Count': 50, 'ScannedCount': 50, ... 'HTTPStatusCode': 200}`.
Cause 8: `botocore.errorfactory.ResourceInUseException: ... when calling the UpdateTable operation: Attempt to change a resource which is still in use: Index creation is in resource allocation phase. ... Table: <table> Index: <index>`.
Cause 9: `the table holds ['arn-00' ... 'name-11']; expected exactly one entry's keys, either the 12 name-keyed ones or the 13 ARN-keyed ones, never both or a mix`.
Cause 10: got `operator: DELETE, operand type: BINARY, typeSet: ALLOWED_FOR_ADD_OPERAND`, expected `operator: DELETE, operand type: Binary, typeSet: ALLOWED_FOR_DELETE_OPERAND`.

## Steps to Reproduce
1. BatchGetItem with a 65536-character ExpressionAttributeNames key; DeleteBackup with a 36/37/1024/1025-character malformed ARN; CreateTable with a 257-character tag value; UntagResource with an empty tag key.
2. Query with KeyConditions ComparisonOperator 'STARTS_WITH'; ListBackups with BackupType 'user'; UpdateItem with AttributeUpdates Action 'REPLACE'; CreateTable with an unlisted vector DistanceFunction; DeleteItem with a lowercase ConditionalOperator.
3. CreateTable with 21 GSI INCLUDE NonKeyAttributes.
4. PutItem with a nested map key of 65536 bytes; ExecuteStatement/BatchExecuteStatement/ExecuteTransaction with an over-long parameter map key; Scan with an over-long ScanFilter map key; UpdateItem with an over-long AttributeUpdates name; TransactWriteItems with an over-long map key.
5. Create a 1-WCU provisioned table and PutItem one ~400 KB item.
6. ExecuteStatement INSERT ~341 KB twelve times on a 1-WCU table; TransactWriteItems on a drained 1-WCU table.
7. UpdateTable to add a GSI, then Query/Scan that index while IndexStatus is CREATING.
8. CreateTable with a vector index, then UpdateTable to create a second one with zero Dimensions.
9. BatchWriteItem whose RequestItems names the same table once by name and once by ARN, 25 items total.
10. UpdateItem with `DELETE attr :binary_scalar` and with `DELETE attr :list`.
