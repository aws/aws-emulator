Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.create-delivery-field-delimiter-past-5-characters-rejected logs.delivery-of-application-logs-to-an-xray-destination-rejected logs.create-lookup-table-query-id-past-256-characters-rejected logs.create-lookup-table-with-both-body-and-query-id-rejected logs.associate-kms-key-id-over-256-chars-rejected logs.associate-kms-key-overlong-resource-identifier-rejected logs.describe-field-indexes-identifier-over-2048-rejected logs.describe-log-groups-name-pattern-past-512-chars-matches-nothing logs.put-metric-filter-dimension-key-over-255-chars-rejected logs.put-metric-filter-field-selection-criteria-over-limit-rejected logs.untag-resource-fifty-one-tag-keys-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.create-delivery-field-delimiter-past-5-characters-rejected|refused#19|AssertionError|raise AssertionError(f"a fieldDelimiter of {delimiter!r} was accepted: {made!r}")
Issue-Key: logs.delivery-of-application-logs-to-an-xray-destination-rejected|_refusal#21|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {got!r}")
Issue-Key: cloudwatch-logs|CreateLookupTable|InvalidParameterException
Issue-Key: logs.create-lookup-table-with-both-body-and-query-id-rejected|run#14|AssertionError|assert both == ("InvalidParameterException", _ONE_SOURCE), f"CSV plus a query id was answered {both!r}"
Issue-Key: cloudwatch-logs|AssociateKmsKey|InvalidParameterException
Issue-Key: logs.associate-kms-key-overlong-resource-identifier-rejected|run#9|AssertionError|assert got == expected, f"a {length}-character resourceIdentifier was refused with {got!r}"
Issue-Key: logs.describe-field-indexes-identifier-over-2048-rejected|run#20|AssertionError|assert long_name == (
Issue-Key: cloudwatch-logs|DescribeLogGroups|InvalidParameterException
Issue-Key: logs.put-metric-filter-dimension-key-over-255-chars-rejected|_refusal#9|AssertionError|raise AssertionError(f"the call was accepted, answering {answer!r}")
Issue-Key: logs.put-metric-filter-field-selection-criteria-over-limit-rejected|run#24|AssertionError|assert "fieldSelectionCriteria" not in plain, (
Issue-Key: logs.untag-resource-fifty-one-tag-keys-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs: 10 net-new causes where a documented constraint is not enforced — CreateDelivery, CreateLookupTable, AssociateKmsKey, DescribeFieldIndexes, DescribeLogGroups, PutMetricFilter and UntagResource accept input AWS refuses (or refuse input AWS accepts)

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
Cause 1 of 10 — `CreateDelivery.fieldDelimiter` length is not enforced.
AWS: the delimiter may be at most 5 characters; a longer value is refused with a 400 validation error naming `fieldDelimiter`. Emulator: `';;;;;'` (5) and longer values are accepted and stored on the delivery — the response echoes `'fieldDelimiter': ';;;;;'`.
Traces: logs.create-delivery-field-delimiter-past-5-characters-rejected.

Cause 2 of 10 — a delivery of application logs to an `XRAY` destination is accepted.
AWS: the delivery-destination type must be compatible with the source's log type; APPLICATION_LOGS to an XRAY destination is refused with a 400 validation error. Emulator: HTTP 200 and the delivery is created with `'deliveryDestinationType': 'XRAY'`.
Traces: logs.delivery-of-application-logs-to-an-xray-destination-rejected.

Cause 3 of 10 — `CreateLookupTable.queryId` length is not checked before the value is resolved.
AWS: a `queryId` over 256 characters is refused as a validation error on that member. Emulator: it is resolved first and refused with `InvalidParameterException: Query with id [<id>] is not a completed or cancelled query.` — the wrong code and a message about query state rather than about the length bound.
Traces: logs.create-lookup-table-query-id-past-256-characters-rejected.

Cause 4 of 10 — the mutually-exclusive `tableBody`/`queryId` refusal is re-worded (and uses a different code than AWS).
Emulator: `('ValidationException', 'tableBody and queryId are mutually exclusive parameters.')` — the sentence is the emulator's own. Recorded behavior only for AWS's exact wording; the requirement is that the refusal is the one AWS gives for this operation, not a locally invented sentence.
Traces: logs.create-lookup-table-with-both-body-and-query-id-rejected.

Cause 5 of 10 — `AssociateKmsKey.kmsKeyId` length is not checked before resolution.
AWS: a `kmsKeyId` over 256 characters is refused as a validation error on that member. Emulator: `InvalidParameterException: The specified KMS Key Id could not be found.` — it tries to resolve the over-long value instead of rejecting it.
Traces: logs.associate-kms-key-id-over-256-chars-rejected.

Cause 6 of 10 — `AssociateKmsKey.resourceIdentifier` length is not checked before resolution.
AWS: an over-long (2048-character) `resourceIdentifier` is a validation error. Emulator: `('ResourceNotFoundException', 'The specified log group does not exist.')` — a not-found answer for input that is not a legal identifier at all.
Traces: logs.associate-kms-key-overlong-resource-identifier-rejected.

Cause 7 of 10 — `DescribeFieldIndexes.logGroupIdentifiers` length is not checked before resolution.
AWS: an ARN naming a 513-character group exceeds the identifier bound and is a validation error. Emulator: `('ResourceNotFoundException', 'Log group cannot be found.')`.
Traces: logs.describe-field-indexes-identifier-over-2048-rejected.

Cause 8 of 10 — `DescribeLogGroups` refuses a long `logGroupNamePattern` that AWS accepts.
AWS: a `logGroupNamePattern` past 512 characters simply matches nothing and the call succeeds with an empty `logGroups` list. Emulator: it refuses with `InvalidParameterException: 1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy constraint: Member must have length less than or equal to 512`.
Traces: logs.describe-log-groups-name-pattern-past-512-chars-matches-nothing.

Cause 9 of 10 — `PutMetricFilter` dimension and field-selection-criteria constraints are not enforced.
AWS: a metric-transformation dimension key over 255 characters is refused with a 400 validation error, and an empty `fieldSelectionCriteria` is not stored. Emulator: the over-long dimension key is accepted (HTTP 200, empty body), and an empty criteria string is stored as `''` where AWS stores nothing.
Traces: logs.put-metric-filter-dimension-key-over-255-chars-rejected, logs.put-metric-filter-field-selection-criteria-over-limit-rejected.

Cause 10 of 10 — `UntagResource` accepts more tag keys than the limit.
AWS: at most 50 tag keys per call; a 51st is refused with a 400 validation error on `tagKeys`. Emulator: accepted, HTTP 200 with an empty body.
Traces: logs.untag-resource-fifty-one-tag-keys-rejected.

Why: each missing guard lets a client store a configuration AWS would reject, so the same code succeeds against the emulator and fails in production (causes 1, 2, 9, 10). Causes 3, 5, 6, 7 answer the wrong question — a not-found or query-state error for input that never passed validation — so a client retries or creates the 'missing' resource instead of fixing its argument. Cause 8 is the inverse: a call AWS answers successfully is refused, so pattern-based discovery breaks.

Fix: the per-operation input validation in each of these handlers (CreateDelivery, CreateLookupTable, AssociateKmsKey, DescribeFieldIndexes, DescribeLogGroups, PutMetricFilter, UntagResource) — enforce the modelled length/size bounds and the destination-type compatibility rule BEFORE resolving identifiers against state, and stop refusing the long logGroupNamePattern DescribeLogGroups accepts.

## Evidence
Cause 1: "a fieldDelimiter of ';;;;;' was accepted: {'id': ..., 'fieldDelimiter': ';;;;;', ...}".
Cause 2: "expected a refusal, the call succeeded: {'delivery': {..., 'deliveryDestinationType': 'XRAY', ...}}".
Cause 3: InvalidParameterException — "Query with id [00000000-0000-4000-8000-000000000001] is not a completed or cancelled query."
Cause 4: "CSV plus a query id was answered ('ValidationException', 'tableBody and queryId are mutually exclusive parameters.')".
Cause 5: InvalidParameterException — "The specified KMS Key Id could not be found."
Cause 6: "a 2048-character resourceIdentifier was refused with ('ResourceNotFoundException', 'The specified log group does not exist.')".
Cause 7: "an ARN naming a 513-character group answered ('ResourceNotFoundException', 'Log group cannot be found.')".
Cause 8: InvalidParameterException — "1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy constraint: Member must have length less than or equal to 512" (AWS returns an empty list).
Cause 9: "the call was accepted, answering {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}"; "an empty criteria was stored as '', expected none".
Cause 10: "the call was expected to be refused but returned {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}".

## Steps to Reproduce
1. CreateDelivery with fieldDelimiter=';;;;;' — accepted.
2. CreateDelivery from an APPLICATION_LOGS source to an XRAY delivery destination — accepted.
3. CreateLookupTable with a 257-character queryId, and with both tableBody and queryId — wrong code/message.
4. AssociateKmsKey with a 257-character kmsKeyId, and with a 2048-character resourceIdentifier — not-found instead of validation.
5. DescribeFieldIndexes with an ARN naming a 513-character group — ResourceNotFoundException.
6. DescribeLogGroups with a 513-character logGroupNamePattern — refused instead of returning an empty list.
7. PutMetricFilter with a 256-character dimension key, and with an empty fieldSelectionCriteria — accepted/stored.
8. UntagResource with 51 tag keys — accepted.
