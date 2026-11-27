Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-create-ops-item-category-pattern
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-create-ops-item-category-pattern ssm-opsitem-severity-length-bounds ssm-describe-ops-items-filter-operator-enum ssm-ops-item-get-by-arn-account-segment ssm-opsitem-arn-length-bounds ssm-ops-item-related-item-ops-item-id-grammar ssm-list-ops-item-related-items-filter-key-enum ssm-ops-item-related-items-filter-operator-enum ssm-ops-item-related-items-page-size-bounds ssm-update-opsitem-status-enum-rejected ssm-delete-ops-metadata-arn-length-bound ssm-list-ops-metadata-filter-value-length-limit
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateOpsItem|4xx->4xx|$.message
Issue-Key: ssm|DescribeOpsItems|2xx->2xx|$.OpsItemSummaries[*].OpsItemArn
Issue-Key: ssm|GetOpsItem|4xx->4xx|$.message
Issue-Key: ssm|AssociateOpsItemRelatedItem|4xx->4xx|$.message
Issue-Key: ssm|GetOpsItem|4xx->4xx|$.__type
Issue-Key: ssm|ListOpsItemRelatedItems|4xx->4xx|$.message
Issue-Key: ssm|UpdateOpsItem|4xx->4xx|$.message
Issue-Key: ssm|DeleteOpsMetadata|4xx->4xx|$.__type
Issue-Key: ssm|DeleteOpsMetadata|4xx->4xx|$.message
Issue-Key: ssm|ListOpsMetadata|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# OpsCenter and OpsMetadata refusals carry wrong codes, wrong messages and short enum sets

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpsItem

## Description
Ten net-new causes on the OpsCenter / OpsMetadata plane. The shared fault is that the refusal bodies these operations serve (code, message, quoted constraint) are not AWS's, plus one read projection gap. Related open issues 20261124T131002Z-ssm-error-messages-diverge-associateopsitemrelateditem-omits-the-response.md and 20261127T125407Z-ssm-9-causes-opsitem-refusal-messages-and-planned.md do NOT cover these clusters (none is marked owned in the replay).

Cause 1 of 10: CreateOpsItem's Category refusal echoes the value.
AWS: "1 validation error detected: Value at 'category' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!\\s*$).+" — no value quoted. Smithy: OpsItemCategory pattern ^(?!\\s*$).+$.
Emulator: inserts "Value '   ' at 'category'".
Why: the refusal body differs from AWS and echoes the rejected input AWS withholds.
Fix: the validation message renderer for this member.
Traces: ssm-create-ops-item-category-pattern#task#1.

Cause 2 of 10: DescribeOpsItems omits OpsItemArn from each summary.
AWS: OpsItemSummaries[].OpsItemArn carries arn:aws:ssm:<region>:<account>:opsitem/<opsItemId> (recorded behavior only — the on-disk OpsItemSummary shape does not model the member).
Emulator: member absent.
Why: a client that addresses an OpsItem by ARN from a listing has no ARN.
Fix: the DescribeOpsItems summary projection.
Traces: ssm-describe-ops-items-filter-operator-enum#task#3.

Cause 3 of 10: GetOpsItem quotes a truncated opsItemArn pattern and echoes the value.
AWS: "1 validation error detected: Value at 'opsItemArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\\.]{0,63}:[0-9]{12}:opsitem/(oi)-[0-9a-f]{12}".
Emulator: quotes "^arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\\.]{0,63}:[0-9]{12}:opsitem" — anchored and missing the /(oi)-[0-9a-f]{12} tail — and echoes the submitted ARN.
Why: the caller is told a grammar that accepts ARNs AWS refuses.
Fix: the opsItemArn constraint and its message rendering.
Traces: ssm-ops-item-get-by-arn-account-segment#task#2.

Cause 4 of 10: AssociateOpsItemRelatedItem echoes the opsItemId.
AWS: "1 validation error detected: Value at 'opsItemId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(oi)-[0-9a-f]{12}$" (Smithy: OpsItemId pattern ^(oi)-[0-9a-f]{12}$).
Emulator: inserts "Value 'oi-0123456789AB' at 'opsItemId'".
Fix: the validation message renderer.
Traces: ssm-ops-item-related-item-ops-item-id-grammar#task#3.

Cause 5 of 10: GetOpsItem answers a malformed ARN with OpsItemNotFoundException and an SDK-style message.
AWS: 400, __type ValidationException, message "1 validation error detected: Value at 'opsItemArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\\.]{0,63}:[0-9]{12}:opsitem/(oi)-[0-9a-f]{12}".
Emulator: __type OpsItemNotFoundException with 'OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: <uuid>; Proxy: null)' — a client-side SDK rendering AWS never puts on the wire.
Why: a shape fault reads as a missing resource, and the body carries SDK scaffolding instead of a service message.
Fix: GetOpsItem input validation before lookup, and the error message body.
Traces: ssm-opsitem-arn-length-bounds#task#3.

Cause 6 of 10: ListOpsItemRelatedItems echoes the filter key.
AWS: "1 validation error detected: Value at 'filters.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [ResourceType, AssociationId, ResourceUri]".
Emulator: inserts "Value 'ResourceArn' at 'filters.1.member.key'".
Fix: the validation message renderer.
Traces: ssm-list-ops-item-related-items-filter-key-enum#task#6.

Cause 7 of 10: UpdateOpsItem's status enum set is short three members and echoes the value.
AWS: the accepted set includes Unapproved, ApprovedInRetrospect and RejectedInRetrospect alongside the twenty values the on-disk OpsItemStatus enum lists, and the message omits the submitted value.
Emulator: lists only the twenty modeled values and echoes "Value 'Reopened'".
Why: an OpsItem status AWS accepts (e.g. Unapproved) is refused, and the message misstates the legal set.
Fix: the OpsItemStatus enum used by validation plus the message renderer.
Traces: ssm-update-opsitem-status-enum-rejected#task#2.

Cause 8 of 10: DeleteOpsMetadata answers a bad ARN with the wrong code.
AWS: 400, __type OpsMetadataInvalidArgumentException, message 'Invalid OpsMetadataArn. You must specify correct Region in the Amazon Resource Name (ARN).'
Emulator: __type OpsMetadataNotFoundException, 'OpsMetadata object not found. Verify the Amazon Resource Name (ARN) and try again.'
Fix: DeleteOpsMetadata ARN validation before lookup.
Traces: ssm-delete-ops-metadata-arn-length-bound#task#1.

Cause 9 of 10: DeleteOpsMetadata truncates the echoed ARN in a length refusal.
AWS: quotes the whole submitted opsMetadataArn in "1 validation error detected: Value '<arn>' at 'opsMetadataArn' failed to satisfy constraint: Member must have length less than or equal to 1011".
Emulator: quotes a prefix plus '... [1012 bytes]'.
Fix: the validation message renderer (no truncation marker).
Traces: ssm-delete-ops-metadata-arn-length-bound#task#2.

Cause 10 of 10: ListOpsMetadata reports a list-member constraint at the wrong level and truncates the value.
AWS: "1 validation error detected: Value '[<value>]' at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1024, Member must have length greater than or equal to 1]".
Emulator: reports 'filters.1.member.values.1.member' with a single constraint and a truncated value ('... [1025 bytes]').
Fix: the validation error renderer for list members.
Traces: ssm-list-ops-metadata-filter-value-length-limit#task#3.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-create-ops-item-category-pattern#task#1: $.message echoes "Value '   '" where AWS writes "Value at 'category'".
Cause 2 ssm-describe-ops-items-filter-operator-enum#task#3: $.OpsItemSummaries[0].OpsItemArn missing (expected 'arn:aws:ssm:us-west-2:000000000000:opsitem/oi-<id>').
Cause 3 ssm-ops-item-get-by-arn-account-segment#task#2: pattern quoted without the /(oi)-[0-9a-f]{12} tail and anchored, plus value echo.
Cause 4 ssm-ops-item-related-item-ops-item-id-grammar#task#3: value echo only.
Cause 5 ssm-opsitem-arn-length-bounds#task#3: $.__type expected 'ValidationException' got 'OpsItemNotFoundException'; message is an SDK-rendered string with Service/Status Code/Request ID/Proxy fields.
Cause 6 ssm-list-ops-item-related-items-filter-key-enum#task#6: value echo.
Cause 7 ssm-update-opsitem-status-enum-rejected#task#2: enum set lacks Unapproved, ApprovedInRetrospect, RejectedInRetrospect; value 'Reopened' echoed.
Cause 8 ssm-delete-ops-metadata-arn-length-bound#task#1: $.__type expected 'OpsMetadataInvalidArgumentException' got 'OpsMetadataNotFoundException'.
Cause 9 ssm-delete-ops-metadata-arn-length-bound#task#2: echo truncated with '... [1012 bytes]'.
Cause 10 ssm-list-ops-metadata-filter-value-length-limit#task#3: path and constraint set diverge, echo truncated with '... [1025 bytes]'.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Cause 1: fixture ssm-create-ops-item-category-pattern step task#1.
3. Cause 2: fixture ssm-describe-ops-items-filter-operator-enum step task#3.
4. Causes 3, 5: fixtures ssm-ops-item-get-by-arn-account-segment step task#2 and ssm-opsitem-arn-length-bounds step task#3.
5. Cause 4: fixture ssm-ops-item-related-item-ops-item-id-grammar step task#3.
6. Cause 6: fixture ssm-list-ops-item-related-items-filter-key-enum step task#6.
7. Cause 7: fixture ssm-update-opsitem-status-enum-rejected step task#2.
8. Causes 8, 9: fixture ssm-delete-ops-metadata-arn-length-bound steps task#1, task#2.
9. Cause 10: fixture ssm-list-ops-metadata-filter-value-length-limit step task#3.
