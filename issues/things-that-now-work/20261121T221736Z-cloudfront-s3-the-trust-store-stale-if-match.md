Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.trust-store-stale-etag-versioning
Issue-Key: cloudfront__s3.trust-store-stale-etag-versioning|run#34|AssertionError|assert message == (
Issue-Commit: 344bb9336e0011430569d037e3a195e35382835e

# cloudfront__s3: the trust-store stale-If-Match refusal carries an HTTP-generic sentence instead of AWS's PreconditionFailed message

> _Note: this issue was identified against the code base at commit `344bb9336e0011430569d037e3a195e35382835e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTrustStore

## Description
Cause 1 of 1: UpdateTrustStore and DeleteTrustStore correctly refuse a superseded (stale) If-Match ETag with HTTP 412 PreconditionFailed, but the message body is the emulator's generic conditional-request sentence rather than the message AWS sends for the CloudFront trust-store family.

AWS: Smithy (com.amazonaws.cloudfront#Cloudfront2020_05_31) models PreconditionFailed -> httpError 412 on both UpdateTrustStore (PUT /2020-05-31/trust-store/{Id}, required If-Match header) and DeleteTrustStore (DELETE /2020-05-31/trust-store/{Id}, required If-Match header); the shape's documentation is only "The precondition in one or more of the request fields evaluated to false." For the message text this is recorded behavior only: AWS answers these two trust-store mutators with the fixed sentence `Your request is missing requirements for this operation. Update your request and try again.` — the whole message is fixed text with NO variable part (no id, ETag, name or timestamp is interpolated), and the error code is `PreconditionFailed` at status 412. (The same fixed sentence is already recorded for the sibling DeleteConnectionGroup in open issue 20261029T212223Z-cloudfront-if-match-etag-handling-wrong-precondition-message.md, which records this generic-sentence fault for other CloudFront mutators but does not carry this check's replay key and so does not cover it.)

Emulator: status 412 and code PreconditionFailed are right, but the message is `The request failed because it didn't meet the preconditions in one or more request-header fields.` — an HTTP-generic sentence AWS does not use for this operation family. The replay shows this on the UpdateTrustStore leg (a stale ETag after a real bundle rotation); the check's DeleteTrustStore leg asserts the identical sentence, so the same defect is expected there.

Why: a client (or an operator-facing tool) that recognises AWS's wording to tell "your ETag is stale, re-read and retry" apart from other 412s gets unrecognised text, and any test or runbook asserting AWS's sentence fails against the emulator even though the code and status match.

Fix: the shared If-Match/ETag precondition path used by the CloudFront trust-store mutators (UpdateTrustStore and DeleteTrustStore handlers / their conditional-request helper) must raise the modelled PreconditionFailed with the trust-store family's message text instead of the generic header sentence.

Traces: cloudfront__s3.trust-store-stale-etag-versioning.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

[mismatch] cloudfront__s3.trust-store-stale-etag-versioning
  AssertionError: expected AWS's exact precondition message, got "The request failed because it didn't meet the preconditions in one or more request-header fields."

Expected (AWS): HTTP 412, Error.Code `PreconditionFailed`, Error.Message exactly `Your request is missing requirements for this operation. Update your request and try again.` (entire message is fixed text).
Actual (emulator): HTTP 412, Error.Code `PreconditionFailed`, Error.Message `The request failed because it didn't meet the preconditions in one or more request-header fields.`

The trust-store id, the bundle object keys and the ETag values in the run are per-run values and are not part of the requirement; the requirement is the status, the code and the fixed message text.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket and put two CA-bundle objects (one certificate, then two concatenated certificates).
3. CreateTrustStore with CaCertificatesBundleS3Location pointing at the one-certificate object; poll GetTrustStore until Status is `active` and keep its ETag (call it E1).
4. UpdateTrustStore(Id, IfMatch=E1, CaCertificatesBundleSource=<two-certificate object>); poll to `active` and keep the new ETag (E2 != E1).
5. UpdateTrustStore(Id, IfMatch=E1, ...) again — now stale. Observe HTTP 412 PreconditionFailed whose Message is the generic request-header sentence instead of `Your request is missing requirements for this operation. Update your request and try again.`
6. DeleteTrustStore(Id, IfMatch=<a superseded ETag>) shows the same message.
7. Re-replay the check in trace_ids to confirm a fix.
