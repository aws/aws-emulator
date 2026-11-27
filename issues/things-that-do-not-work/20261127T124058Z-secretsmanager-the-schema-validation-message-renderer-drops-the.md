Issue-Source: trace-sweep-divergence
Issue-Trace: list-secrets-filter-and-filter-value-count-bounds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: list-secrets-filter-and-filter-value-count-bounds untag-resource-key-length-and-missing-secret-refusals secretsmanager-remove-regions-from-replication-list-and-region-length-limits secretsmanager-put-secret-value-rotation-token-character-set secretsmanager-put-secret-value-version-stage-over-256-chars-rejected secretsmanager-create-secret-kms-key-id-over-2048-chars-rejected secretsmanager-create-secret-replica-region-code-pattern secretsmanager-create-secret-replica-region-list-bounds
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|ListSecrets|4xx->4xx|$.message
Issue-Key: secretsmanager|UntagResource|4xx->4xx|$.message
Issue-Key: secretsmanager|RemoveRegionsFromReplication|4xx->4xx|$.message
Issue-Key: secretsmanager|PutSecretValue|4xx->4xx|$.message
Issue-Key: secretsmanager|CreateSecret|4xx->4xx|$.message
Issue-Commit: 34735640b265050797646a7031263995ba2fdc83

# secretsmanager: the schema-validation message renderer drops the offending value, mis-elides it and strips regex anchors

> _Note: this issue was identified against the code base at commit `34735640b265050797646a7031263995ba2fdc83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecrets

## Description
5 net-new causes, all in ONE component: the shared renderer that formats a Smithy constraint violation into Secrets Manager's "1 validation error detected: ..." message. The emulator reaches the right HTTP status (400) and the right error type, but the message body is not the message AWS emits, and an AWS message text is a requirement, not an incidental string. The renderer's template is:

  1 validation error detected: Value '<rendered request value>' at '<memberPath>' failed to satisfy constraint: <constraint clause>

Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: " and the constraint clause's wording ("Member must have length less than or equal to <n>", "Member must have length greater than or equal to <n>", "Member must satisfy regular expression pattern: <pattern>"). Variable parts: the value from the request, the member path from the model, the limit/pattern from the model's constraint trait. Nothing in the template comes from a single run.

Cause 1 of 5: ListSecrets — the renderer omits the "Value '<value>' at '" segment for a list member.
AWS: Smithy ListSecrets.Filters -> FiltersListType @length {min:0,max:10}. The over-length list is refused 400 with message "1 validation error detected: Value '[Filter(key=name, values=[sim2real-probe-secretsmanager-fltcount ...' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10". AWS renders a structure list in Java toString form ("[Filter(key=<k>, values=[<v>, ...])]") and elides the rendering at 63 characters, appending " ..." before the closing quote.
Emulator: "1 validation error detected: Value at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10" — the entire "Value '<...>'" segment is gone.
Why: a client (or a human reading a 400) cannot see WHICH filters it sent were rejected; SDK error-message assertions and log scrapers keyed on the AWS template do not match.
Fix: the shared validation-message renderer used by the Secrets Manager operation validators — it must render the offending value for list/structure members, not just scalars.
Traces: list-secrets-filter-and-filter-value-count-bounds#task#3 (cluster also covers that fixture's task#6).

Cause 2 of 5: UntagResource — the renderer does render the value but never elides it.
AWS: Smithy UntagResource.TagKeys -> TagKeyListType, member TagKeyType @length {min:1,max:128}. Recorded 400 message: "1 validation error detected: Value '[kkkkkkk...(63 chars) ...' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]". The elision — first 63 characters of the rendered value, then a space, then "..." — is part of the template.
Emulator: prints the full rendered list value verbatim, "Value '[kkkk...(128 chars)]' at 'tagKeys' ...", with no elision.
Why: AWS caps the echoed value, so a client that logs or compares the message gets an unbounded echo of its own input instead; for a member whose max is large this inflates the error body without bound.
Fix: the same renderer — apply AWS's 63-character elision with the trailing " ..." sentinel to the rendered value.
Traces: untag-resource-key-length-and-missing-secret-refusals#task#6.

Cause 3 of 5: RemoveRegionsFromReplication — value segment omitted for an empty list.
AWS: Smithy RemoveRegionsFromReplication.RemoveReplicaRegions -> RemoveReplicaRegionListType @length {min:1}, @required. An empty list is refused 400 with "1 validation error detected: Value '[]' at 'removeReplicaRegions' failed to satisfy constraint: Member must have length greater than or equal to 1" — the empty list still renders, as "[]".
Emulator: "1 validation error detected: Value at 'removeReplicaRegions' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: same as cause 1 — the message does not carry the rejected value, so it is not AWS's message.
Fix: same renderer; an empty collection renders as "[]" rather than suppressing the segment.
Traces: secretsmanager-remove-regions-from-replication-list-and-region-length-limits#task#3 (cluster also covers task#4 of the same fixture).

Cause 4 of 5: PutSecretValue — the pattern clause strips the regex anchors.
AWS: Smithy PutSecretValue.RotationToken -> RotationTokenType @pattern "^[a-zA-Z0-9\\-]+$". AWS echoes the model's pattern trait VERBATIM: "1 validation error detected: Value at 'rotationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]+$". (Note AWS itself omits the value here because the member is @sensitive — that part the emulator gets right.)
Emulator: "... Member must satisfy regular expression pattern: [a-zA-Z0-9\\-]+" — the leading ^ and trailing $ are dropped.
Why: the echoed pattern is what a client uses to fix its input; the unanchored form describes a DIFFERENT (strictly weaker) rule than the one being enforced, so a client that trusts it keeps sending rejected tokens.
Fix: the pattern clause of the renderer must emit the @pattern trait string unmodified.
Traces: secretsmanager-put-secret-value-rotation-token-character-set#task#3 (cluster also covers secretsmanager-put-secret-value-version-stage-over-256-chars-rejected#task#3).

Cause 5 of 5: CreateSecret — the KmsKeyId over-length refusal is not wrapped in AWS's KMS-validation prefix.
AWS: Smithy CreateSecret.KmsKeyId -> KmsKeyIdType @length {min:0,max:2048}. CreateSecret validates KmsKeyId through its KMS path, so the refusal is prefixed: "KMS validation failed - 1 validation error detected: Value '<kmsKeyId from the request>' at 'kmsKeyId' failed to satisfy constraint: Member must have length less than or equal to 2048". Fixed text: "KMS validation failed - " followed by the standard template; variable part: the submitted key id.
Emulator: 400 with a $.message that is not that string (the replay reports a value_mismatch on $.message for this cluster; the emulator's rendering drops the "KMS validation failed - " wrapper and/or the value segment, consistent with causes 1/3).
Why: a client branching on the KMS-validation prefix to distinguish a bad key reference from a bad secret name cannot tell the two apart.
Fix: CreateSecret's KmsKeyId validation must route through the KMS-validation wrapper before calling the shared renderer.
Traces: secretsmanager-create-secret-kms-key-id-over-2048-chars-rejected#task#4 (cluster also covers secretsmanager-create-secret-replica-region-code-pattern#task#1 and secretsmanager-create-secret-replica-region-list-bounds#task#1).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 90.7%, 45/484 steps mismatched). All five clusters are verdict=mismatch with the EXPECTED status already matched — only the message text diverges.

Cause 1 (ListSecrets, 400 -> 400, key $.message):
  expected: "1 validation error detected: Value '[Filter(key=name, values=[sim2real-probe-secretsmanager-fltcount ...' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10"
  actual:   "1 validation error detected: Value at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10"

Cause 2 (UntagResource, 400 -> 400, key $.message):
  expected: "... Value '[kkkk<63 chars total> ...' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"
  actual:   "... Value '[kkkk<full 128-char key>]' at 'tagKeys' failed to satisfy constraint: [same clause]"

Cause 3 (RemoveRegionsFromReplication, 400 -> 400, key $.message):
  expected: "1 validation error detected: Value '[]' at 'removeReplicaRegions' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   "1 validation error detected: Value at 'removeReplicaRegions' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 4 (PutSecretValue, 400 -> 400, key $.message):
  expected: "... Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]+$"
  actual:   "... Member must satisfy regular expression pattern: [a-zA-Z0-9\\-]+"
  Model: RotationTokenType @pattern "^[a-zA-Z0-9\\-]+$" (smithy_query shape RotationTokenType via PutSecretValue input).

Cause 5 (CreateSecret, 400 -> 400, key $.message):
  expected: "KMS validation failed - 1 validation error detected: Value 'alias/kkkk...' at 'kmsKeyId' failed to satisfy constraint: Member must have length less than or equal to 2048"
  actual:   a different $.message (value_mismatch on $.message).

Model citations (smithy_query, services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): ListSecrets.Filters @length{0,10}; UntagResource.TagKeys member TagKeyType @length{1,128}; RemoveRegionsFromReplication.RemoveReplicaRegions @required @length{min:1}; PutSecretValue.RotationToken @length{36,256} @pattern ^[a-zA-Z0-9\-]+$ @sensitive; CreateSecret.KmsKeyId @length{0,2048}.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture list-secrets-filter-and-filter-value-count-bounds, step task#3 — ListSecrets with 11 Filters entries; compare $.message against the recorded one.
4. Cause 2: replay untag-resource-key-length-and-missing-secret-refusals#task#6 — UntagResource with a 128-character TagKeys entry; check the message is elided at 63 chars with a trailing " ...".
5. Cause 3: replay secretsmanager-remove-regions-from-replication-list-and-region-length-limits#task#3 — RemoveRegionsFromReplication with RemoveReplicaRegions=[]; expect "Value '[]' at 'removeReplicaRegions'".
6. Cause 4: replay secretsmanager-put-secret-value-rotation-token-character-set#task#3 — PutSecretValue with a RotationToken containing a character outside [a-zA-Z0-9-]; expect the anchored pattern ^[a-zA-Z0-9\-]+$ in the message.
7. Cause 5: replay secretsmanager-create-secret-kms-key-id-over-2048-chars-rejected#task#4 — CreateSecret with a KmsKeyId longer than 2048 chars; expect the "KMS validation failed - " prefix.
