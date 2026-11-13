Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-alias-management-v2
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-alias-management-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|CreateAlias|4xx->4xx|status
Issue-Key: payment-cryptography|GetAlias|4xx->4xx|status
Issue-Commit: 1747967889e85246e725647f8dca9265cfad2964

# payment-cryptography alias refusals answer the shape's @httpError status instead of the service's HTTP 400

> _Note: this issue was identified against the code base at commit `1747967889e85246e725647f8dca9265cfad2964`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
Payment Cryptography's control plane is an awsJson1_0 (JSON-RPC) service: it answers client-side refusals with HTTP 400 and conveys the error CLASS in the response body's `__type`, not in the status line. The emulator instead derives the status from each error shape's @httpError trait, so the alias-registry refusals come back with the shape's status (409 for ConflictException, 404 for ResourceNotFoundException) rather than 400. Both bodies already match the recordings; only the status line diverges. Two causes, both of which must be closed before this issue is submitted.

Cause 1 of 2: CreateAlias refusing an alias name that is already bound answers HTTP 409 instead of 400.
AWS: Smithy — service PaymentCryptographyControlPlane (version 2021-09-14), protocols [awsJson1_0], sigv4Name payment-cryptography. CreateAlias.AliasName is @required (@length 7..256, @pattern ^alias/[a-zA-Z0-9/_-]+$) and the operation models ConflictException (errorKind client, @httpError 409). The recorded wire response for a duplicate alias name is HTTP 400 carrying the class in the body (__type com.amazonaws.paymentcryptography.exception#ConflictException); for awsJson1_0 the status line belongs to the protocol, not to the shape's @httpError value, and the recording is authoritative. Error code requirement: ConflictException; message requirement (recorded behavior only, and already filed separately for its wording — see Note): fixed text `Alias `, then the AliasName exactly as supplied in the request, then the fixed text ` already exists.` The alias name is the only variable part, taken from the request; no recorded alias name is itself part of the requirement.
Emulator: answers HTTP 409 (expected 400) with an otherwise-matching ConflictException body — divergence_key service=payment-cryptography|CreateAlias|status=400->409|keys=status, detail "status: expected 400, got 409".
Why: SDKs, CLIs and generic HTTP middleware classify responses from the status line as well as the body. A 409 from a JSON-RPC AWS endpoint is read as an HTTP-layer conflict rather than a modeled service error, so retry/error-classification logic keyed on "400 + __type" never surfaces ConflictException to the caller; a client that distinguishes an alias-name collision from other failures mis-reports it, and proxies may discard the body before it is parsed.
Fix: the error-to-HTTP-status mapping in the payment-cryptography control-plane error serializer (the component that turns a modeled error shape into a wire response): emit this service's wire status (400) for client-error shapes and let `__type` carry the class, instead of the shape's @httpError value. The CreateAlias handler's choice of error class is correct and must not change.
Traces: payment-cryptography-alias-management-v2#task#7.

Cause 2 of 2: GetAlias refusing an alias name that names no alias answers HTTP 404 instead of 400.
AWS: Smithy — GetAlias.AliasName @required (@length 7..256, same @pattern), readonly; its modeled errors include ResourceNotFoundException (errorKind client, @httpError 404; also a service-common error). The recorded wire response for a well-formed alias name that resolves to nothing is HTTP 400 with __type com.amazonaws.paymentcryptography.exception#ResourceNotFoundException. Error code requirement: ResourceNotFoundException. Status requirement: 400, the same protocol-level status this service uses for every client refusal.
Emulator: answers HTTP 404 (expected 400) with an otherwise-matching ResourceNotFoundException body — divergence_key service=payment-cryptography|GetAlias|status=400->404|keys=status, detail "status: expected 400, got 404".
Why: as cause 1 — a 404 from a JSON-RPC endpoint is what clients treat as "no such endpoint/route" rather than a modeled service error, so the caller can see a transport failure instead of ResourceNotFoundException and never learns the alias simply does not exist.
Fix: the same status mapping as cause 1, reached through a second error class. Because one class is fixed by a per-shape override and the other by the general rule, a fix must be confirmed on BOTH recorded steps rather than on one.
Traces: payment-cryptography-alias-management-v2#task#9.

Note on already-filed, related issues (neither covers these two steps; the replay reports no owner for either):
- 20261031T105700Z-payment-cryptography-not-found-refusals-answer-http-404.md files the same mapping defect, but its replay keys are GetKey (payment-cryptography-validation-and-notfound-errors-v2#task#1) and DeleteKey (#task#5) only; the two alias steps here are not in its pointer, and its scope is ResourceNotFoundException, so the ConflictException status is not covered by it either.
- 20261031T180548Z-payment-cryptography-control-plane-duplicate-alias-and-already.md files the CreateAlias duplicate-alias MESSAGE wording (behavioral). It explicitly notes the error code is already correct and says nothing about the status line, so it does not cover this status divergence.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 73 steps, 51 passed, 4 divergence clusters). Two clusters, verdict=mismatch, body fully matching and ONLY the HTTP status diverging:
- {service: payment-cryptography, operation: CreateAlias, fixture_id: payment-cryptography-alias-management-v2, step_key: task#7, expected_status: 400, actual_status: 409, detail: "status: expected 400, got 409", divergence_key: service=payment-cryptography|CreateAlias|status=400->409|keys=status, owned_by: [], stub_path: ""}
- {service: payment-cryptography, operation: GetAlias, fixture_id: payment-cryptography-alias-management-v2, step_key: task#9, expected_status: 400, actual_status: 404, detail: "status: expected 400, got 404", divergence_key: service=payment-cryptography|GetAlias|status=400->404|keys=status, owned_by: [], stub_path: ""}

Expected vs actual, per step:
- task#7 CreateAlias (duplicate alias name): expected HTTP 400 with body __type com.amazonaws.paymentcryptography.exception#ConflictException; actual HTTP 409 with the same class in the body.
- task#9 GetAlias (alias name that names no alias): expected HTTP 400 with body __type com.amazonaws.paymentcryptography.exception#ResourceNotFoundException; actual HTTP 404 with the same class in the body.

Smithy (services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/smithy-model/com-amazonaws-paymentcryptography-paymentcryptographycontrolplane.json): service_info protocols [awsJson1_0], sdkId "Payment Cryptography", arnNamespace payment-cryptography, common_errors include ResourceNotFoundException and ValidationException; CreateAlias errors include ConflictException (client, httpError 409) and ValidationException (client, 400); GetAlias errors include ResourceNotFoundException (client, httpError 404). The @httpError values are exactly the statuses the emulator emits, which is the signature of the mapping defect: for awsJson1_0 the recorded service answers 400 regardless.

Corroboration from the same corpus: the recorded ResourceNotFoundException refusals on GetKey and DeleteKey (payment-cryptography-validation-and-notfound-errors-v2#task#1 / #task#5) are likewise HTTP 400 — i.e. every recorded client refusal on this service is 400, so the rule is service-wide and not per-shape.

## Steps to Reproduce
1. Replay fixture payment-cryptography-alias-management-v2 against the emulator.
2. Step task#7 calls CreateAlias with an AliasName that is already bound in the registry. Observed: HTTP 409 with a ConflictException body. Expected: HTTP 400 with the same ConflictException class carried in the body's __type.
3. Step task#9 calls GetAlias with a well-formed AliasName (matching ^alias/[a-zA-Z0-9/_-]+$, length 7..256) that names no alias. Observed: HTTP 404 with a ResourceNotFoundException body. Expected: HTTP 400 with the same class in __type.
4. Both steps must be re-replayed after the fix: they exercise two different error shapes (@httpError 409 and 404) through the same status-mapping path, so passing one does not demonstrate the other.
5. Cross-check: payment-cryptography-validation-and-notfound-errors-v2#task#1 and #task#5 show the same 400 expectation for ResourceNotFoundException on GetKey/DeleteKey.
