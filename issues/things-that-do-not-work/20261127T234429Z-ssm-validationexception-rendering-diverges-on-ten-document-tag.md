Issue-Source: trace-sweep-divergence
Issue-Trace: add-tags-resource-type-outside-enum-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: add-tags-resource-type-outside-enum-rejected create-document-requires-name-grammar ssm-document-requires-version-name-pattern ssm-connection-status-target-length-bounds ssm-delete-resource-policy-resource-arn-length-bounds ssm-describe-sessions-filter-value-length-bounds ssm-document-permission-max-results-range ssm-document-permission-type-outside-enum-refused ssm-document-permission-remove-list-bound ssm-document-permission-shared-version-pattern ssm-list-documents-filters-count-bound ssm-list-documents-legacy-filter-key-outside-enum ssm-list-documents-tag-filter-value-length-bounds ssm-start-session-reason-length-bounds
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|AddTagsToResource|4xx->4xx|$.message
Issue-Key: ssm|CreateDocument|4xx->4xx|$.message
Issue-Key: ssm|GetConnectionStatus|4xx->4xx|$.message
Issue-Key: ssm|DeleteResourcePolicy|4xx->4xx|$.__type
Issue-Key: ssm|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: ssm|DescribeSessions|4xx->4xx|$.message
Issue-Key: ssm|DescribeDocumentPermission|4xx->4xx|$.message
Issue-Key: ssm|ModifyDocumentPermission|4xx->4xx|$.message
Issue-Key: ssm|ListDocuments|4xx->4xx|$.message
Issue-Related: 20261127T234429Z-ssm-run-command-and-session-refusals-filters-and-command.md
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM ValidationException rendering diverges on ten document, tag, policy and session operations

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTagsToResource

## Description
Ten net-new causes sharing one fault: the ValidationException body these operations serve is not the body AWS serves for the same request. AWS's template is "<n> validation error(s) detected: Value[ '<value>'] at '<member path>' failed to satisfy constraint: Member must <constraint>" — the fixed text is the template and the constraint sentences; the variables are the member path, the limit/pattern, and the value only where AWS quotes it. Open issue 20261127T125407Z-ssm-8-causes-input-validation-refusal-messages-quote.md describes the same family but does not own these clusters, which the replay reports unowned.

Cause 1 of 10: AddTagsToResource's resourceType refusal is the wrong sentence.
AWS: "1 validation error detected: Value at 'resourceType' failed to satisfy constraint: Member is not a valid resourceType" — no value, no enum list.
Emulator: "Value 'Bucket' at 'resourceType' failed to satisfy constraint: Member must satisfy enum value set: [Association, OpsMetadata, MaintenanceWindow, OpsItem, Parameter, PatchBaseline, ManagedInstance, Automation, CloudConnector, Document]".
Fix: the AddTagsToResource validation message.
Traces: add-tags-resource-type-outside-enum-rejected#task#2.

Cause 2 of 10: CreateDocument quotes the wrong Requires name grammar.
AWS: "Value at 'requires.1.member.name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_\\-.:/]{3,200}$" — anchored, upper bound 200, no value echoed.
Emulator: echoes the value and quotes '[a-zA-Z0-9_\\-.:/]{3,128}' (un-anchored, bound 128). The on-disk Smithy DocumentRequires$Name pattern is ^[a-zA-Z0-9_\\-.:/]{3,128}$; the recorded service accepts up to 200, so the recording is the requirement and names between 129 and 200 characters are wrongly refused.
Fix: the DocumentRequires name constraint and its message.
Traces: create-document-requires-name-grammar#task#3.

Cause 3 of 10: GetConnectionStatus reports one of two errors.
AWS: "2 validation errors detected: Value at 'target' failed to satisfy constraint: Member must have length greater than or equal to 1; Value at 'target' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_:-]{1,400}$".
Emulator: emits only the length clause and echoes the value.
Fix: the validation walk (evaluate and report both constraints).
Traces: ssm-connection-status-target-length-bounds#task#2.

Cause 4 of 10: DeleteResourcePolicy answers an over-long ARN with ResourceNotFoundException.
AWS: 400, __type ValidationException, message 'Invalid arn: <arn>'.
Emulator: __type ResourceNotFoundException with a different body.
Why: a malformed-input fault reads as a missing policy, so the caller retries against a resource that exists.
Fix: DeleteResourcePolicy ARN validation before lookup.
Traces: ssm-delete-resource-policy-resource-arn-length-bounds#task#2.

Cause 5 of 10: DeleteResourcePolicy's length refusal body diverges.
AWS: the standard template quoting the full submitted ARN against the modeled bound.
Emulator: a different message for the same request.
Fix: the validation message renderer for resourceArn.
Traces: ssm-delete-resource-policy-resource-arn-length-bounds#task#3.

Cause 6 of 10: DescribeSessions reports one of two errors.
AWS: "2 validation errors detected: Value at 'filters.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 400; Value at 'filters.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{1,400}$".
Emulator: length clause only.
Fix: as cause 3.
Traces: ssm-describe-sessions-filter-value-length-bounds#task#2.

Cause 7 of 10: DescribeDocumentPermission echoes the maxResults value.
AWS: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: inserts "Value '0' at 'maxResults'".
Fix: the validation message renderer for ranged members.
Traces: ssm-document-permission-max-results-range#task#4.

Cause 8 of 10: ModifyDocumentPermission echoes the whole account-id list.
AWS: "1 validation error detected: Value at 'accountIdsToRemove' failed to satisfy constraint: Member must have length less than or equal to 20".
Emulator: quotes the entire submitted list inside the message.
Fix: the validation message renderer for list members.
Traces: ssm-document-permission-remove-list-bound#task#4.

Cause 9 of 10: ListDocuments echoes the whole filters list.
AWS: "1 validation error detected: Value at 'filters' failed to satisfy constraint: Member must have length less than or equal to 6".
Emulator: quotes a Java-style rendering of every DocumentKeyValuesFilter submitted.
Fix: as cause 8.
Traces: ssm-list-documents-filters-count-bound#task#4.

Cause 10 of 10: StartSession quotes the reason pattern un-anchored.
AWS: "2 validation errors detected: Value at 'reason' failed to satisfy constraint: Member must have length less than or equal to 256; Value at 'reason' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{1,256}$".
Emulator: quotes '.{1,256}' without anchors.
Fix: the validation message renderer (emit patterns as modeled).
Traces: ssm-start-session-reason-length-bounds#task#2.

## Evidence
Replay of simparity-wire-aws-v1; all causes are $.message/$.__type mismatches at 400->400.
Cause 1 add-tags-resource-type-outside-enum-rejected#task#2: expected "...Member is not a valid resourceType", got the enum-value-set sentence with "Value 'Bucket'".
Cause 2 create-document-requires-name-grammar#task#3: expected '^[a-zA-Z0-9_\\-.:/]{3,200}$' with no value, got '[a-zA-Z0-9_\\-.:/]{3,128}' with the value echoed.
Cause 3 ssm-connection-status-target-length-bounds#task#2: expected 2 errors, got 1.
Cause 4 ssm-delete-resource-policy-resource-arn-length-bounds#task#2: $.__type expected 'ValidationException' got 'ResourceNotFoundException'; message expected 'Invalid arn: <arn>'.
Cause 5 #task#3: $.message diverges for the same refusal.
Cause 6 ssm-describe-sessions-filter-value-length-bounds#task#2: expected 2 errors, got 1.
Cause 7 ssm-document-permission-max-results-range#task#4: "Value '0'" echoed.
Cause 8 ssm-document-permission-remove-list-bound#task#4: the 21-element account list echoed.
Cause 9 ssm-list-documents-filters-count-bound#task#4: the filters list echoed.
Cause 10 ssm-start-session-reason-length-bounds#task#2: pattern quoted as '.{1,256}'.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Replay each listed fixture and diff the 400 body at the named step against the recorded AWS body.
3. Representative: fixture add-tags-resource-type-outside-enum-rejected step task#2 (AddTagsToResource with ResourceType 'Bucket').
