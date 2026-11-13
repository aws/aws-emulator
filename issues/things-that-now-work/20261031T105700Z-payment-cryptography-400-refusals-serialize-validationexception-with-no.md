Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-validation-and-notfound-errors-v2
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-validation-and-notfound-errors-v2 payment-cryptography-key-replication-regions-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|GetAlias|4xx->4xx|$.Message
Issue-Key: payment-cryptography|ListKeys|4xx->4xx|$.Message
Issue-Key: payment-cryptography|CreateAlias|4xx->4xx|$.Message
Issue-Key: payment-cryptography|RemoveKeyReplicationRegions|4xx->4xx|$.Message
Issue-Commit: 2b1389a4a6c4df71c4118b776bf7e768de987e7c

# payment-cryptography 400 refusals serialize ValidationException with no Message member

> _Note: this issue was identified against the code base at commit `2b1389a4a6c4df71c4118b776bf7e768de987e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAlias

## Description
The emulator detects input-constraint violations and answers the right status and error code, but serializes ValidationException with an EMPTY body as far as `Message` is concerned, so the caller is told only that something was invalid. AWS always fills in the standard constraint-violation sentence. Four causes, one per operation whose refusal was exercised.

Cause 1 of 4: GetAlias with a syntactically invalid alias name.
AWS: Smithy — GetAlias.AliasName is @required, @length(min 7, max 256), @pattern "^alias/[a-zA-Z0-9/_-]+$"; GetAlias's modeled errors include ValidationException (errorKind client, httpError 400, single member `Message`). Recorded response: HTTP 400, __type com.amazonaws.paymentcryptography.exception#ValidationException, and Message carrying AWS's standard constraint sentence. Its FIXED text is "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; its VARIABLE parts are the error count, the rejected value taken from the request, the lowerCamelCase wire name of the offending member (`aliasName`), and the member's @pattern with its anchors stripped (`alias/[a-zA-Z0-9/_-]+`).
Emulator: HTTP 400 with the correct __type, but `$.Message` is absent entirely (expected the sentence above, got None).
Why: SDK error objects and CLI output surface the message as the whole explanation of the refusal. With no Message, a client is told a request was invalid but not which member, which value, or which constraint — so retry/repair logic and operator diagnostics that parse the message get nothing.
Fix: the shared ValidationException construction path for the payment-cryptography control plane — the validator that rejects a member must render AWS's constraint sentence into `Message` from the member name, the submitted value and the violated constraint, instead of emitting a Message-less error.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#2.

Cause 2 of 4: ListKeys with MaxResults above the modeled ceiling.
AWS: Smithy — ListKeys.MaxResults targets MaxResults with @range(min 1, max 100). Recorded response: HTTP 400 ValidationException whose Message has fixed text "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must have value less than or equal to <max>", with the rejected value from the request, the wire member name `maxResults`, and the ceiling (100) from the @range trait.
Emulator: HTTP 400, correct error code, `$.Message` missing.
Why/Fix: as cause 1, for the @range branch of the validator.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#3.

Cause 3 of 4: CreateAlias with a syntactically invalid alias name.
AWS: Smithy — CreateAlias.AliasName is @required, @length(7,256), @pattern "^alias/[a-zA-Z0-9/_-]+$"; ValidationException httpError 400. Recorded response: HTTP 400 with the same regular-expression-pattern sentence as cause 1, naming `aliasName` and the anchor-stripped pattern.
Emulator: HTTP 400, correct error code, `$.Message` missing.
Why/Fix: as cause 1 — the same validator serves the create path.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#4.

Cause 4 of 4: RemoveKeyReplicationRegions with a malformed Region in ReplicationRegions.
AWS: Smithy — RemoveKeyReplicationRegions.ReplicationRegions is @required and targets Regions, a list of Region (@pattern "^[a-z]{2}-[a-z]{1,16}-[0-9]+$"); ValidationException httpError 400. Recorded response: HTTP 400 whose Message uses the nested-member form of the sentence — fixed text "<n> validation error detected: Value '[<value>]' at '<memberName>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: <pattern>]" — with the rejected list rendered in brackets, the wire member name `replicationRegions`, and the element pattern with anchors stripped.
Emulator: HTTP 400, correct error code, `$.Message` missing.
Why/Fix: as cause 1, for the list-element branch of the validator (the nested "Member must satisfy constraint: [...]" wrapper is required when the violation is on a list element rather than the member itself).
Traces: payment-cryptography-key-replication-regions-v2#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1. Four divergence clusters, verdict=mismatch, expected_status 400 and actual_status 400 in every case — only the body diverges:
- GetAlias (payment-cryptography-validation-and-notfound-errors-v2#task#2): "$.Message: missing (expected \"1 validation error detected: Value 'not-a-valid-alias-name' at 'aliasName' failed to satisfy constraint: Member must satisfy regular expression pattern: alias/[a-zA-Z0-9/_-]+\", got None)".
- ListKeys (#task#3): "$.Message: missing (expected \"1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100\", got None)".
- CreateAlias (#task#4): same regular-expression-pattern sentence for 'aliasName', $.Message missing.
- RemoveKeyReplicationRegions (payment-cryptography-key-replication-regions-v2#task#5): "$.Message: missing (expected \"1 validation error detected: Value '[not-a-region]' at 'replicationRegions' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: [a-z]{2}-[a-z]{1,16}-[0-9]+]\", got None)".
The quoted values (not-a-valid-alias-name, 1000, not-a-region) are the fixtures' request inputs and fill the template; the sentence structure, the member names and the constraint text are the requirement.
Smithy: ValidationException (client, httpError 400) has exactly one member, Message; AliasName @pattern ^alias/[a-zA-Z0-9/_-]+$; ListKeys.MaxResults @range 1..100; Regions element Region @pattern ^[a-z]{2}-[a-z]{1,16}-[0-9]+$.

## Steps to Reproduce
1. Replay fixture payment-cryptography-validation-and-notfound-errors-v2. Step task#2 calls GetAlias with AliasName="not-a-valid-alias-name"; the emulator answers 400 ValidationException with no Message member.
2. Step task#3 calls ListKeys with MaxResults=1000 (> the modeled max of 100): 400, no Message.
3. Step task#4 calls CreateAlias with the same invalid alias name: 400, no Message.
4. Replay payment-cryptography-key-replication-regions-v2 step task#5, RemoveKeyReplicationRegions with ReplicationRegions=["not-a-region"]: 400, no Message.
5. In each case compare against the recorded response, which carries the "<n> validation error detected: ..." sentence in Message.
