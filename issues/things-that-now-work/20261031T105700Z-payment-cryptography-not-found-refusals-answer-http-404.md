Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-validation-and-notfound-errors-v2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-validation-and-notfound-errors-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|GetKey|4xx->4xx|status
Issue-Key: payment-cryptography|DeleteKey|4xx->4xx|status
Issue-Commit: 2b1389a4a6c4df71c4118b776bf7e768de987e7c

# payment-cryptography not-found refusals answer HTTP 404 instead of the service's 400

> _Note: this issue was identified against the code base at commit `2b1389a4a6c4df71c4118b776bf7e768de987e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKey

## Description
Payment Cryptography is an awsJson1_0 service that answers client-side refusals with HTTP 400 and carries the error class in the body's `__type`. The emulator instead maps ResourceNotFoundException onto the status from the shape's @httpError trait, so a missing key is refused with HTTP 404. Two causes.

Cause 1 of 2: GetKey for a well-formed key identifier that names no key.
AWS: Smithy — GetKey.KeyIdentifier is @required, targets KeyArnOrKeyAliasType, and GetKey's modeled errors include ResourceNotFoundException. The shape declares @httpError 404, but the RECORDED wire response for this refusal is HTTP 400 with the error class carried in the body (__type com.amazonaws.paymentcryptography.exception#ResourceNotFoundException); for awsJson1_0 the status line is the protocol's, not the shape's, and the recording is authoritative. The body the emulator already produces matched the recording — only the status diverges.
Emulator: answers HTTP 404 (expected 400) with an otherwise-correct ResourceNotFoundException body.
Why: SDK and CLI retry/error classification is driven by the status line as well as the body. A 404 from a JSON-RPC AWS endpoint is what clients treat as "no such endpoint/route" rather than a modeled service error, so generic HTTP middleware, proxies and SDK response handlers can fail before the body is parsed — the caller never sees ResourceNotFoundException at all.
Fix: the error-to-HTTP-status mapping for the payment-cryptography control-plane error serializer: client-error shapes must be emitted with this service's wire status (400) rather than the shape's @httpError value, with the class conveyed by `__type`.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#1.

Cause 2 of 2: DeleteKey for a well-formed key identifier that names no key.
AWS: Smithy — DeleteKey.KeyIdentifier @required; ResourceNotFoundException is among its errors (service-common). Recorded wire response: HTTP 400 carrying ResourceNotFoundException in __type; body again matched.
Emulator: answers HTTP 404 (expected 400).
Why/Fix: as cause 1, through the same mapping — this is the same defect reached by a second operation, so a fix to the mapping must be confirmed on both.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#5.

Note: the same 400-for-client-errors rule is what the recordings show for ConflictException on this service too (see the separate alias-registry issue), so the mapping must not special-case only not-found.

## Evidence
Replay of trace set simparity-wire-aws-v1. Two divergence clusters, verdict=mismatch, body fully matching and ONLY the status diverging:
- divergence_key service=payment-cryptography|GetKey|status=400->404|keys=status, fixture payment-cryptography-validation-and-notfound-errors-v2, step task#1, detail "status: expected 400, got 404".
- divergence_key service=payment-cryptography|DeleteKey|status=400->404|keys=status, same fixture, step task#5, detail "status: expected 400, got 404".
Corroborating recordings from the same service: payment-cryptography-alias-management-v2#task#9 expects HTTP 400 with __type com.amazonaws.paymentcryptography.exception#ResourceNotFoundException, and #task#7 expects HTTP 400 with __type com.amazonaws.paymentcryptography.exception#ConflictException — i.e. this service reports client errors as 400 regardless of the shapes' @httpError traits (ResourceNotFoundException 404, ConflictException 409).
Smithy: protocols [awsJson1_0]; common_errors include ResourceNotFoundException (client, httpError 404) and ValidationException (client, httpError 400).

## Steps to Reproduce
1. Replay fixture payment-cryptography-validation-and-notfound-errors-v2.
2. Step task#1 calls GetKey with a well-formed KeyIdentifier that names no existing key: the emulator responds HTTP 404; the recorded AWS response is HTTP 400 with __type ...#ResourceNotFoundException.
3. Step task#5 calls DeleteKey with the same kind of identifier: HTTP 404 instead of 400.
4. Confirm the expected mapping against payment-cryptography-alias-management-v2#task#9, where the recorded ResourceNotFoundException is likewise an HTTP 400.
