Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-catalog-get-inventory-schema
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-catalog-get-inventory-schema ssm-delete-inventory-schema-delete-option-enum ssm-delete-inventory-type-name-length-bound ssm-get-inventory-aggregator-count-bounds ssm-get-ops-summary-filter-key-length-bounds ssm-get-ops-summary-filter-type-enum ssm-list-compliance-items-filter-key-length-bounds ssm-put-inventory-capture-time-pattern
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetInventorySchema|2xx->2xx|$.Schemas[*]
Issue-Key: ssm|DeleteInventory|4xx->4xx|$.Message
Issue-Key: ssm|DeleteInventory|4xx->4xx|$.message
Issue-Key: ssm|GetInventory|4xx->4xx|$.Message
Issue-Key: ssm|GetInventory|4xx->4xx|$.__type
Issue-Key: ssm|GetInventory|4xx->4xx|$.message
Issue-Key: ssm|GetOpsSummary|4xx->4xx|$.Message
Issue-Key: ssm|GetOpsSummary|2xx->2xx|$.Entities[*].Data.AWS:OpsItem.Content[*].AccountId
Issue-Key: ssm|GetOpsSummary|2xx->2xx|$.Entities[*].Data.AWS:OpsItem.Content[*].Description
Issue-Key: ssm|GetOpsSummary|2xx->2xx|$.Entities[*].Data.AWS:OpsItem.Content[*].Notifications
Issue-Key: ssm|GetOpsSummary|2xx->2xx|$.Entities[*].Data.AWS:OpsItem.Content[*].OperationalData
Issue-Key: ssm|GetOpsSummary|2xx->2xx|$.Entities[*].Data.AWS:OpsItem.Content[*].RelatedItems
Issue-Key: ssm|ListComplianceItems|4xx->4xx|$.__type
Issue-Key: ssm|ListComplianceItems|4xx->4xx|$.message
Issue-Key: ssm|PutInventory|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Inventory and compliance reads and refusals diverge: wrong codes, wrong messages, invented schemas

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInventory

## Description
Ten net-new causes across Inventory, Compliance and OpsSummary: the seeded inventory schema catalog, the refusal codes/messages of the inventory entry points, and one read projection.

Cause 1 of 10: the seeded inventory schema catalog is not AWS's.
AWS: GetInventorySchema returns the AWS-published type list in AWS's order, each with its real attribute set (e.g. AWS:File with Name, Size, Description, FileVersion, InstalledDate, ModificationTime, LastAccessTime, ProductName, InstalledDir, ProductLanguage, CompanyName, ProductVersion).
Emulator: serves a different set of types in different positions (e.g. AWS:ComplianceSummary where AWS:File is expected) and so publishes attributes AWS does not for the index the client reads.
Why: a client that discovers inventory attributes from the schema builds queries against attributes that do not exist.
Fix: the seeded inventory schema reference data.
Traces: ssm-catalog-get-inventory-schema#task#1.

Cause 2 of 10: DeleteInventory re-words the invalid-type-name refusal.
AWS: 400 with Message "TypeName <value> is not a valid type" — fixed text with the submitted type name.
Emulator: emits a multi-sentence message about case sensitivity and a Custom: regular expression.
Fix: the DeleteInventory type-name validation message.
Traces: ssm-delete-inventory-schema-delete-option-enum#task#3.

Cause 3 of 10: DeleteInventory orders two validation errors the other way round.
AWS: "2 validation errors detected: Value '' at 'typeName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(AWS|Custom):.*$; Value '' at 'typeName' failed to satisfy constraint: Member must have length greater than or equal to 1" — pattern clause first.
Emulator: emits the length clause first.
Why: the refusal body differs byte-for-byte from AWS's for the same request.
Fix: the validation walk's constraint order (pattern before length).
Traces: ssm-delete-inventory-type-name-length-bound#task#3.

Cause 4 of 10: GetInventory's count-aggregator refusal is the wrong sentence.
AWS: 400 with Message 'One and only one top level count aggregator is supported.'
Emulator: 'Attribute Name of type AWS:Application does not support count aggregator.'
Fix: the GetInventory aggregator validation.
Traces: ssm-get-inventory-aggregator-count-bounds#task#2.

Cause 5 of 10: GetInventory uses the wrong error code for mixed aggregators.
AWS: 400, __type InvalidInventoryGroupException, Message "Groups, Filters, SubAggregators, AttributeNames and Expressions can't be used together in multiple Aggregators."
Emulator: 400, __type InvalidAggregatorException, Message "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported."
Why: a client branching on the error code takes the wrong branch.
Fix: GetInventory aggregator validation (code and message).
Traces: ssm-get-inventory-aggregator-count-bounds#task#3.

Cause 6 of 10: GetInventory's aggregator-count refusal renders the offending value differently.
AWS: quotes the full aggregator list in the standard validation template ("1 validation error detected: Value '[InventoryAggregator(...), ...]' at 'aggregators' failed to satisfy constraint: ...").
Emulator: renders a different body for the same refusal.
Fix: the validation message renderer for list-valued members.
Traces: ssm-get-inventory-aggregator-count-bounds#task#4.

Cause 7 of 10: GetOpsSummary's unknown-filter refusal is the wrong sentence.
AWS: 400 with Message "Attribute name '<name>' is not defined for type '<TypeName>'." — fixed text with the submitted attribute name and the resolved type.
Emulator: "The filter name isn't valid. Verify that you entered the correct name and try again."
Fix: the GetOpsSummary filter validation message.
Traces: ssm-get-ops-summary-filter-key-length-bounds#task#2.

Cause 8 of 10: GetOpsSummary's AWS:OpsItem content omits five members.
AWS: each AWS:OpsItem content row carries AccountId, Description, Notifications, OperationalData and RelatedItems (empty string when unset).
Emulator: all five absent.
Why: an OpsCenter summary consumer reads null for fields AWS always emits.
Fix: the GetOpsSummary entity/content projection.
Traces: ssm-get-ops-summary-filter-type-enum#task#2.

Cause 9 of 10: ListComplianceItems answers a shape violation with InvalidFilter.
AWS: 400, __type ValidationException, message 'A validation exception occurred, please re-check input values.'
Emulator: 400, __type InvalidFilter, message "The filter name isn't valid. Verify that you entered the correct name and try again."
Fix: ListComplianceItems filter validation (code and message).
Traces: ssm-list-compliance-items-filter-key-length-bounds#task#3.

Cause 10 of 10: PutInventory reports one of two captureTime errors and un-anchors the pattern.
AWS: "2 validation errors detected: Value '<value>' at 'items.1.member.captureTime' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(20)[0-9][0-9]-(0[1-9]|1[012])-([12][0-9]|3[01]|0[1-9])(T)(2[0-3]|[0-1][0-9])(:[0-5][0-9])(:[0-5][0-9])(Z)$; Value '<value>' at 'items.1.member.captureTime' failed to satisfy constraint: Member must have length less than or equal to 20".
Emulator: one error only, and quotes the pattern without its ^...$ anchors.
Fix: PutInventory input validation and the pattern rendering.
Traces: ssm-put-inventory-capture-time-pattern#task#2.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-catalog-get-inventory-schema#task#1: $.Schemas[3] expected the AWS:File schema, got AWS:ComplianceSummary; $.Schemas[4], [7], [8], [13] likewise diverge.
Cause 2 ssm-delete-inventory-schema-delete-option-enum#task#3: $.Message expected 'TypeName Custom:<name> is not a valid type'.
Cause 3 ssm-delete-inventory-type-name-length-bound#task#3: clause order inverted versus AWS.
Cause 4 ssm-get-inventory-aggregator-count-bounds#task#2: $.Message expected 'One and only one top level count aggregator is supported.'
Cause 5 #task#3: $.__type expected 'InvalidInventoryGroupException' got 'InvalidAggregatorException'.
Cause 6 #task#4: $.message rendering of the aggregator list diverges.
Cause 7 ssm-get-ops-summary-filter-key-length-bounds#task#2: $.Message expected "Attribute name '<name>' is not defined for type 'AWS:EC2InstanceInformation'."
Cause 8 ssm-get-ops-summary-filter-type-enum#task#2: $.Entities[0].Data.AWS:OpsItem.Content[0].AccountId / Description / Notifications / OperationalData / RelatedItems missing.
Cause 9 ssm-list-compliance-items-filter-key-length-bounds#task#3: $.__type expected 'ValidationException' got 'InvalidFilter'.
Cause 10 ssm-put-inventory-capture-time-pattern#task#2: expected 2 validation errors with the anchored pattern, got 1 with an un-anchored pattern.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Cause 1: fixture ssm-catalog-get-inventory-schema step task#1.
3. Causes 2-3: fixtures ssm-delete-inventory-schema-delete-option-enum step task#3 and ssm-delete-inventory-type-name-length-bound step task#3.
4. Causes 4-6: fixture ssm-get-inventory-aggregator-count-bounds steps task#2, task#3, task#4.
5. Causes 7-8: fixtures ssm-get-ops-summary-filter-key-length-bounds step task#2 and ssm-get-ops-summary-filter-type-enum step task#2.
6. Cause 9: fixture ssm-list-compliance-items-filter-key-length-bounds step task#3.
7. Cause 10: fixture ssm-put-inventory-capture-time-pattern step task#2.
