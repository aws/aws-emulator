Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-get-execution-preview-id-length-bounds
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-get-execution-preview-id-length-bounds ssm-get-execution-preview-id-pattern ssm-get-parameters-by-path-filter-key-length ssm-parameters-by-path-filter-key-pattern ssm-instance-information-legacy-filter-value-length-bound ssm-instance-property-filter-value-length-bounds ssm-list-nodes-summary-aggregator-type-name-enum ssm-list-nodes-filter-count-bounds ssm-remove-document-tags-unicode-and-space-keys ssm-resume-session-id-length-bound ssm-update-document-default-version-number-grammar ssm-patch-group-deregister-length-bounds
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetExecutionPreview|4xx->4xx|$.message
Issue-Key: ssm|GetParametersByPath|4xx->4xx|$.message
Issue-Key: ssm|DescribeInstanceInformation|4xx->4xx|$.message
Issue-Key: ssm|DescribeInstanceProperties|4xx->4xx|$.message
Issue-Key: ssm|ListNodesSummary|4xx->4xx|$.message
Issue-Key: ssm|ListNodes|4xx->4xx|$.Message
Issue-Key: ssm|RemoveTagsFromResource|4xx->4xx|$.message
Issue-Key: ssm|ResumeSession|4xx->4xx|$.message
Issue-Key: ssm|UpdateDocumentDefaultVersion|4xx->4xx|$.message
Issue-Key: ssm|DeregisterPatchBaselineForPatchGroup|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM ValidationException rendering diverges on ten node, parameter-path and preview operations

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionPreview

## Description
Ten more net-new causes of the same shared fault as the companion rendering issue, on a different set of operations: AWS's "<n> validation error(s) detected: Value[ '<value>'] at '<path>' failed to satisfy constraint: Member must <constraint>" body is re-ordered, re-pathed, truncated, or carries a value AWS omits. Filed separately because a bundle carries at most ten causes.

Cause 1 of 10: GetExecutionPreview inverts the error order and echoes the id.
AWS: "2 validation errors detected: Value at 'executionPreviewId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}; Value at 'executionPreviewId' failed to satisfy constraint: Member must have length less than or equal to 36" — pattern first, no value.
Emulator: length first and the value quoted in both clauses.
Fix: the validation walk order plus the message renderer.
Traces: ssm-get-execution-preview-id-length-bounds#task#4.

Cause 2 of 10: GetParametersByPath inverts the order and re-brackets the pattern.
AWS: "2 validation errors detected: Value '' at 'parameterFilters.1.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: tag:.+|Name|Type|KeyId|Path|Label|Tier|DataType; Value '' at ... Member must have length greater than or equal to 1".
Emulator: length clause first and the pattern wrapped in an extra group '(tag:.+|...|DataType)'.
Fix: as cause 1 (order) and the pattern rendering.
Traces: ssm-get-parameters-by-path-filter-key-length#task#5.

Cause 3 of 10: DescribeInstanceInformation reports a legacy filter value set at the wrong level.
AWS: "1 validation error detected: Value at 'instanceInformationFilterList.1.member.valueSet' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 100000, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^.{1,100000}$]".
Emulator: reports '...valueSet.1.member' with a single constraint.
Fix: the validation error renderer for list members.
Traces: ssm-instance-information-legacy-filter-value-length-bound#task#1.

Cause 4 of 10: DescribeInstanceProperties does the same for instancePropertyFilterList.
AWS: aggregated constraint set against '...valueSet'.
Emulator: single constraint against '...valueSet.1.member'.
Fix: as cause 3.
Traces: ssm-instance-property-filter-value-length-bounds#task#2.

Cause 5 of 10: ListNodesSummary's aggregator typeName enum set is missing Extension.
AWS: "Member must satisfy enum value set: [Instance, Extension]".
Emulator: "[Instance]" — so an Extension aggregator is refused as unknown and the message misstates the set. Related open issue 20261123T021656Z-ssm-listnodessummary-validationexception-message-lists-only-8-of-aws.md covers the NodeAttributeName list, not this typeName set.
Fix: the aggregator typeName enum used by validation plus the message.
Traces: ssm-list-nodes-summary-aggregator-type-name-enum#task#1.

Cause 6 of 10: ListNodes adds a Message AWS does not send.
AWS: refuses the request with 400 and no Message member.
Emulator: adds Message 'ResourceDataSync with name <name> synctype SyncFromSource does not exists'.
Why: the body carries a field AWS omits (and a sentence AWS never writes).
Fix: the ListNodes error body.
Traces: ssm-list-nodes-filter-count-bounds#task#1.

Cause 7 of 10: RemoveTagsFromResource echoes the submitted tag-key list.
AWS: "1 validation error detected: Value at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$]".
Emulator: inserts "Value '[<key>, <key>]'".
Fix: the validation message renderer for list members.
Traces: ssm-remove-document-tags-unicode-and-space-keys#task#2.

Cause 8 of 10: ResumeSession echoes the sessionId.
AWS: "1 validation error detected: Value at 'sessionId' failed to satisfy constraint: Member must have length less than or equal to 96".
Emulator: quotes the submitted session id.
Fix: the validation message renderer.
Traces: ssm-resume-session-id-length-bound#task#2.

Cause 9 of 10: UpdateDocumentDefaultVersion echoes the documentVersion.
AWS: "1 validation error detected: Value at 'documentVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[1-9][0-9]*$)".
Emulator: inserts "Value '01' at 'documentVersion'".
Fix: the validation message renderer.
Traces: ssm-update-document-default-version-number-grammar#task#6.

Cause 10 of 10: DeregisterPatchBaselineForPatchGroup truncates the echoed patchGroup.
AWS: quotes the whole submitted patchGroup in "1 validation error detected: Value '<patchGroup>' at 'patchGroup' failed to satisfy constraint: Member must have length less than or equal to 256".
Emulator: quotes a prefix plus '... [257 bytes]'.
Fix: the validation message renderer (no truncation marker).
Traces: ssm-patch-group-deregister-length-bounds#task#3.

## Evidence
Replay of simparity-wire-aws-v1; all causes are $.message/$.Message mismatches at 400->400 unless stated.
Cause 1 ssm-get-execution-preview-id-length-bounds#task#4: clause order inverted plus value echo.
Cause 2 ssm-get-parameters-by-path-filter-key-length#task#5: order inverted, pattern wrapped in an extra group.
Cause 3 ssm-instance-information-legacy-filter-value-length-bound#task#1: path '...valueSet' vs '...valueSet.1.member'; aggregated constraint list absent.
Cause 4 ssm-instance-property-filter-value-length-bounds#task#2: same divergence.
Cause 5 ssm-list-nodes-summary-aggregator-type-name-enum#task#1: enum set '[Instance, Extension]' vs '[Instance]'.
Cause 6 ssm-list-nodes-filter-count-bounds#task#1: $.Message unexpected_present (expected None).
Cause 7 ssm-remove-document-tags-unicode-and-space-keys#task#2: "Value '[équipe, cost#center]'" echoed.
Cause 8 ssm-resume-session-id-length-bound#task#2: session id echoed.
Cause 9 ssm-update-document-default-version-number-grammar#task#6: "Value '01'" echoed.
Cause 10 ssm-patch-group-deregister-length-bounds#task#3: echo truncated with '... [257 bytes]'.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Replay each listed fixture and diff the 400 body at the named step against the recorded AWS body.
3. Representative: fixture ssm-get-execution-preview-id-length-bounds step task#4 (GetExecutionPreview with a 37-character id).
