Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.add-tags-key-outside-tag-charset-rejected ssm.add-tags-value-outside-character-set-rejected ssm.add-tags-to-parameter-key-length-bounds-rejected ssm.add-tags-to-parameter-value-over-256-rejected ssm.remove-tags-key-outside-tag-charset-rejected ssm.remove-tags-key-over-128-chars-rejected-and-batch-kept ssm.describe-parameters-filter-key-outside-pattern-rejected ssm.describe-parameters-filter-value-length-bounds-rejected ssm.describe-parameters-filter-value-over-1024-chars-rejected ssm.put-parameter-allowed-pattern-over-1024-chars-rejected ssm.put-parameter-description-over-1024-chars-rejected ssm.put-parameter-key-id-outside-pattern-rejected ssm.put-inventory-fractional-second-capture-time-rejected ssm.update-association-status-message-length-bounds-rejected ssm.resource-data-sync-bare-kms-key-id-rejected ssm.add-tags-to-parameter-tag-list-bounds-rejected ssm.put-inventory-context-key-over-64-characters-rejected ssm.compliance-items-filter-type-outside-enum-rejected ssm.remove-just-added-tag-keys-restores-prior-tag-set ssm.describe-parameters-legacy-filter-value-count-bounds-rejected ssm.put-parameter-data-type-over-128-chars-rejected ssm.describe-association-executions-rejects-unissued-next-token ssm.describe-automation-executions-unissued-next-token-rejected ssm.describe-automation-step-executions-rejects-unissued-next-token ssm.describe-maintenance-window-executions-rejects-unissued-next-token ssm.describe-maintenance-windows-unissued-next-token-rejected ssm.list-documents-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.add-tags-key-outside-tag-charset-rejected|run#8|AssertionError|assert got == expected, f"a '#' in a tag key was refused with {got!r}, expected {expected!r}"
Issue-Key: ssm.add-tags-value-outside-character-set-rejected|run#10|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: ssm.add-tags-to-parameter-key-length-bounds-rejected|run#12|AssertionError|assert message == (
Issue-Key: ssm.add-tags-to-parameter-value-over-256-rejected|run#11|AssertionError|assert message == (
Issue-Key: ssm.remove-tags-key-outside-tag-charset-rejected|run#9|AssertionError|assert got == expected, f"a '!' in a tag key to remove gave {got!r}, expected {expected!r}"
Issue-Key: ssm.remove-tags-key-over-128-chars-rejected-and-batch-kept|run#10|AssertionError|assert message == (
Issue-Key: ssm.describe-parameters-filter-key-outside-pattern-rejected|run#8|AssertionError|assert message == expected, f"filter key {key!r} gave message {message!r}"
Issue-Key: ssm.describe-parameters-filter-value-length-bounds-rejected|run#11|AssertionError|assert message == expected, (
Issue-Key: ssm.describe-parameters-filter-value-over-1024-chars-rejected|run#12|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: ssm.put-parameter-allowed-pattern-over-1024-chars-rejected|run#22|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: ssm.put-parameter-description-over-1024-chars-rejected|run#15|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: ssm.put-parameter-key-id-outside-pattern-rejected|run#14|AssertionError|assert (code, message) == (
Issue-Key: ssm.put-inventory-fractional-second-capture-time-rejected|run#18|AssertionError|assert message == expected, f"refusal message was {message!r}"
Issue-Key: ssm.update-association-status-message-length-bounds-rejected|run#19|AssertionError|assert got == want, (
Issue-Key: ssm.resource-data-sync-bare-kms-key-id-rejected|run#27|AssertionError|assert message.startswith(head) and message.endswith(tail), (
Issue-Key: ssm.add-tags-to-parameter-tag-list-bounds-rejected|run#18|AssertionError|assert (code, framed) == ("ValidationException", True), (
Issue-Key: ssm.put-inventory-context-key-over-64-characters-rejected|run#17|AssertionError|code == "UnsupportedInventoryItemContextException"
Issue-Key: ssm|ListComplianceItems|ValidationException
Issue-Key: ssm|DescribeParameters|InvalidFilterValue
Issue-Key: ssm.describe-parameters-legacy-filter-value-count-bounds-rejected|_rejected_with#14|AssertionError|raise AssertionError(f"expected a refusal, but the call answered {outcome!r}")
Issue-Key: ssm.put-parameter-data-type-over-128-chars-rejected|_refusal#19|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: ssm.describe-association-executions-rejects-unissued-next-token|run#14|AssertionError|assert got == ("InvalidNextToken", ""), (
Issue-Key: ssm.describe-automation-executions-unissued-next-token-rejected|run#16|AssertionError|assert got == ("InvalidNextToken", ""), (
Issue-Key: ssm.describe-automation-step-executions-rejects-unissued-next-token|run#13|AssertionError|assert got == ("InvalidNextToken", ""), (
Issue-Key: ssm.describe-maintenance-window-executions-rejects-unissued-next-token|run#9|AssertionError|assert got == ("ValidationException", ""), (
Issue-Key: ssm.describe-maintenance-windows-unissued-next-token-rejected|run#14|AssertionError|assert got == ("ValidationException", ""), f"unissued NextToken answered {got!r}"
Issue-Key: ssm.list-documents-unissued-next-token-rejected|run#14|AssertionError|assert (code, message) == ("InvalidNextToken", ""), (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 8 causes — input-validation refusal messages quote the refused value AWS omits, tag/filter/data-type bounds are mis-coded or unenforced, and an unissued NextToken carries a message

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTagsToResource

## Description
Eight net-new causes in SSM's request-validation and refusal-shaping layer. Every one is a client-visible status/code/message divergence.

Cause 1 of 8: ValidationException messages quote the refused value where AWS names only the member.
AWS: SSM renders a constraint failure as `1 validation error detected: Value at '<member path>' failed to satisfy constraint: <constraint>` — the member path is present, the offending value is NOT. Smithy: Tag.Key @length 1..128 @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$; Tag.Value @length 0..256 same pattern; ParameterStringFilter.Key @length 1..132 @pattern ^(tag:.+|Name|Type|KeyId|Path|Label|Tier|DataType)$; ParameterStringFilter.Values @length 1..50.
Emulator: emits `1 validation error detected: Value '<the request's value>' at '<member>' failed to satisfy constraint: ...` — e.g. `Value 'cost#centre' at 'tags.2.member.key'`, `Value '[team, team!lead]' at 'tagKeys'`, `Value 'tag:' at 'parameterFilters.1.member.key'`, `Value '2026-11-27T12:20:33.250Z' at 'items.1.member.captureTime'`. Fixed text of the requirement: `1 validation error detected: Value at '`, the member path, `' failed to satisfy constraint: ` and the constraint clause; the only variable parts are the member path and the constraint text, both derived from the model — never the submitted value.
Why: clients that match or log AWS's refusal text (and the suite's own graders) see a different message for every value, and secrets/PII in a refused member are echoed back.
Fix: the shared validation-message builder used by the SSM op handlers (services/service-com-amazonaws-ssm-amazonssm/src/…, the validation helper that formats constraint failures), not each op.
Traces: ssm.add-tags-key-outside-tag-charset-rejected, ssm.add-tags-value-outside-character-set-rejected, ssm.add-tags-to-parameter-key-length-bounds-rejected, ssm.add-tags-to-parameter-value-over-256-rejected, ssm.remove-tags-key-outside-tag-charset-rejected, ssm.remove-tags-key-over-128-chars-rejected-and-batch-kept, ssm.describe-parameters-filter-key-outside-pattern-rejected, ssm.describe-parameters-filter-value-length-bounds-rejected, ssm.describe-parameters-filter-value-over-1024-chars-rejected, ssm.put-parameter-allowed-pattern-over-1024-chars-rejected, ssm.put-parameter-description-over-1024-chars-rejected, ssm.put-parameter-key-id-outside-pattern-rejected, ssm.put-inventory-fractional-second-capture-time-rejected, ssm.update-association-status-message-length-bounds-rejected, ssm.resource-data-sync-bare-kms-key-id-rejected.

Cause 2 of 8: a tag list past 1000 entries is refused with TooManyTagsError and an empty message.
AWS: Smithy TagList @length 0..1000, so the over-length list is a modelled shape violation: ValidationException, HTTP 400, message `1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 1000`.
Emulator: code `TooManyTagsError` with an empty message.
Why: a client branching on ValidationException (or reading the message to learn the 1000 limit) sees an unmodelled code and no diagnosis.
Fix: AddTagsToResource's input validation (ops/add_tags_to_resource.rs) must apply the TagList length bound through the shared validator.
Traces: ssm.add-tags-to-parameter-tag-list-bounds-rejected.

Cause 3 of 8: PutInventory refuses an over-long Context key with the wrong error class.
AWS: the Context key is a length-bounded map key; a key past the bound is a ValidationException naming the member.
Emulator: `UnsupportedInventoryItemContextException` with `The Context attribute that you specified for the InventoryItem isn't allowed for this inventory type. You can only use the Context attribute with inventory types like AWS:ComplianceItem.` — a type-support refusal used for a length violation.
Why: the client is told its inventory TYPE is wrong when its key length is wrong, so the request is never corrected.
Fix: PutInventory's validation (ops/put_inventory.rs) must check the Context key bound before the type-support gate.
Traces: ssm.put-inventory-context-key-over-64-characters-rejected.

Cause 4 of 8: ListComplianceItems refuses the mixed-case filter Type 'Equal' that AWS accepts.
AWS: ComplianceQueryOperatorType enum is [EQUAL, NOT_EQUAL, BEGIN_WITH, LESS_THAN, GREATER_THAN]; recorded behavior shows AWS also accepts the mixed-case alias 'Equal' and answers normally (empty ComplianceItems for a node with no data), and for a genuinely unknown value refuses ValidationException with `1 validation error detected: Value '<value>' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [<accepted values>]`.
Emulator: refuses 'Equal' with ValidationException, so the call raises where the recording returned 200.
Why: callers using the documented CLI/console casing are refused outright.
Fix: ListComplianceItems filter-type parsing (ops/list_compliance_items.rs).
Traces: ssm.compliance-items-filter-type-outside-enum-rejected.

Cause 5 of 8: DescribeParameters refuses a `tag:<key>` ParameterFilter that carries no Values.
AWS: Smithy ParameterStringFilter.Values is OPTIONAL (@length 1..50 when present); a filter of only `Key: tag:<key>` selects every parameter carrying that tag key and the call answers 200.
Emulator: `InvalidFilterValue` — `Filter key 'tag:release' carries no value to search for`.
Why: the documented 'has this tag key' search is unusable; a tag read-back loop fails with a hard client error.
Fix: DescribeParameters filter handling (ops/describe_parameters.rs) must treat an absent Values as a key-presence predicate.
Traces: ssm.remove-just-added-tag-keys-restores-prior-tag-set.

Cause 6 of 8: the legacy Filters value-count bound is not enforced on DescribeParameters.
AWS: an over-count legacy `Filters[].Values` list is refused with ValidationException naming the member and its count bound.
Emulator: the call succeeds and returns the matching parameter list (HTTP 200).
Why: an over-count filter silently returns results, so a client never learns it exceeded the bound and pages a different result set than AWS would serve.
Fix: DescribeParameters legacy-filter validation (ops/describe_parameters.rs).
Traces: ssm.describe-parameters-legacy-filter-value-count-bounds-rejected.

Cause 7 of 8: PutParameter accepts a DataType longer than its bound.
AWS: DataType is length-bounded (128); an over-length value is refused ValidationException naming 'dataType'.
Emulator: returns 200 with `{Version: 1, Tier: 'Standard'}` and stores the parameter.
Why: a parameter is created with a DataType AWS would never store, so later reads diverge.
Fix: PutParameter input validation (ops/put_parameter.rs).
Traces: ssm.put-parameter-data-type-over-128-chars-rejected.

Cause 8 of 8: an unissued NextToken is refused with an emulator-authored message (and the wrong code on the maintenance-window reads).
AWS: for a token the service never issued, SSM answers code `InvalidNextToken` with an EMPTY message (HTTP 400); the code alone is the contract.
Emulator: `InvalidNextToken` with message `The NextToken value is not valid`, and for DescribeMaintenanceWindowExecutions / DescribeMaintenanceWindows the code is `ValidationException` with that same message.
Why: a client that classifies by code mis-handles the maintenance-window reads as a shape violation, and message-matching clients see text AWS does not send.
Fix: the shared pagination-token decoder and its error mapping used by the SSM list/describe handlers.
Traces: ssm.describe-association-executions-rejects-unissued-next-token, ssm.describe-automation-executions-unissued-next-token-rejected, ssm.describe-automation-step-executions-rejects-unissued-next-token, ssm.describe-maintenance-window-executions-rejects-unissued-next-token, ssm.describe-maintenance-windows-unissued-next-token-rejected, ssm.list-documents-unissued-next-token-rejected.

## Evidence
Cause 1: expected ('ValidationException', "1 validation error detected: Value at 'tags.2.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"), got the same with "Value 'cost#centre' at ...". Same shape for 'tagKeys', 'parameterFilters.1.member.key', 'items.1.member.captureTime', 'allowedPattern', 'description', 'keyId', 'associationStatus.message', 's3Destination.aWSKMSKeyARN'.
Cause 2: expected ('ValidationException', "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 1000"); got ('TooManyTagsError', '').
Cause 3: got ('UnsupportedInventoryItemContextException', "The Context attribute that you specified for the InventoryItem isn't allowed for this inventory type...") for a 64-character Context key.
Cause 4: ListComplianceItems(Filters=[{Type:'Equal'}]) raised ValidationException "1 validation error detected: Value 'Equal' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [GREATER_THAN, LESS_THAN, NOT_EQUAL, EQUAL, BEGIN_WITH]"; the recording answers with an empty ComplianceItems list.
Cause 5: DescribeParameters(ParameterFilters=[{'Key':'tag:release'}]) raised InvalidFilterValue "Filter key 'tag:release' carries no value to search for".
Cause 6: the over-count legacy filter answered 200 with one Parameter record.
Cause 7: PutParameter with a 129-character DataType answered 200 {'Version': 1, 'Tier': 'Standard'}.
Cause 8: expected ('InvalidNextToken', ''); got ('InvalidNextToken', 'The NextToken value is not valid') and ('ValidationException', 'The NextToken value is not valid') for the two maintenance-window reads.

## Steps to Reproduce
1. AddTagsToResource on a parameter with a tag key containing '#', a 0-char key, a 257-char value, and a 1001-entry tag list; RemoveTagsFromResource with a '!' key and a 129-char key.
2. DescribeParameters with ParameterFilters Key 'tag:' , a 0-char and a 1025-char filter value, and with ParameterFilters=[{'Key':'tag:release'}] (no Values); also an over-count legacy Filters value list.
3. PutParameter with a 1025-char AllowedPattern, a 1025-char Description, KeyId 'alias/x.', and a 129-char DataType.
4. PutInventory with a 64-char Context key on a Custom: type and with a fractional-second captureTime.
5. ListComplianceItems with Filters[0].Type='Equal'.
6. Call each of DescribeAssociationExecutions, DescribeAutomationExecutions, DescribeAutomationStepExecutions, DescribeMaintenanceWindowExecutions, DescribeMaintenanceWindows, ListDocuments with a NextToken the service never issued.
Compare code and message against AWS in each case.
