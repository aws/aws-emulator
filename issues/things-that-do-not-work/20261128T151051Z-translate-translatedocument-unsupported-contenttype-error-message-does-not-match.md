Issue-Source: trace-sweep-divergence
Issue-Trace: translate-document-unsupported-content-type-rejected
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-document-unsupported-content-type-rejected
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|TranslateDocument|4xx->4xx|$.Message
Issue-Commit: 68c3ace149bc175fa18851d7845c2a02e20f9350

# TranslateDocument unsupported ContentType error message does not match AWS wording

> _Note: this issue was identified against the code base at commit `68c3ace149bc175fa18851d7845c2a02e20f9350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TranslateDocument

## Description
One net-new cause in Amazon Translate's TranslateDocument rejection path: the emulator rejects an unsupported Document.ContentType with the correct HTTP status and error type, but writes its own diagnostic sentence into the error's Message member instead of the sentence AWS returns for that error class.

Cause 1 of 1: TranslateDocument rejects an unsupported Document.ContentType with a non-AWS error message.
AWS: Smithy: TranslateDocument input Document is @required; Document.ContentType is @required, targets ContentType (@length 0..256, @pattern ^[-\w.]+\/[-\w.+]+$) and is documented as accepting exactly text/plain, text/html and application/vnd.openxmlformats-officedocument.wordprocessingml.document. A content type outside that set is a client error modelled by InvalidRequestException (errorKind client, httpError 400, Message member targets String @length 0..10000). The message text itself is recorded behavior only: AWS answers HTTP 400 with the fixed sentence 'The provided ContentType is not supported.' — the whole sentence is fixed text, it has NO variable parts (AWS does not interpolate the rejected content type or enumerate the accepted ones).
Emulator: answers HTTP 400 with the same error type but the Message 'Unsupported content type: <rejectedContentType>. Supported content types are text/plain, text/html and application/vnd.openxmlformats-officedocument.wordprocessingml.document.' — i.e. an emulator-authored sentence that interpolates the rejected content type (observed as 'application/json') and appends the supported-type list.
Why: the error message is part of the API contract. A client that surfaces, logs, asserts on or pattern-matches the message of an unsupported-content-type rejection sees text AWS never produces, so message-based assertions and user-facing error reporting written against AWS fail against the emulator even though the status and error shape are right.
Fix: the TranslateDocument handler's ContentType validation in the translate service — it must emit the fixed AWS sentence for this rejection rather than constructing its own diagnostic string.
Traces: translate-document-unsupported-content-type-rejected#task#3.

Related but NOT covering this: the open issue 20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md covers translate 200-response body values (terminology ARN version segment and the TranslateText '[mt:...]' prefix); it does not cover this error-path message.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Cause 1 — fixture translate-document-unsupported-content-type-rejected, step task#3, operation TranslateDocument, verdict mismatch. Expected status 400, actual status 400 (status and error type agree). Body divergence at $.Message: expected 'The provided ContentType is not supported.', got 'Unsupported content type: application/json. Supported content types are text/plain, text/html and application/vnd.openxmlformats-officedocument.wordprocessingml.document.'

Fixed vs variable: AWS's message is entirely fixed text ('The provided ContentType is not supported.'). In the emulator's message, 'application/json' is the ContentType taken from the request (a per-fixture value, not part of any requirement) and the remainder is emulator-authored fixed text that AWS does not emit at all.

Model basis: com.amazonaws.translate#Document$ContentType is @required with @pattern ^[-\w.]+\/[-\w.+]+$ and documents the three accepted media types; com.amazonaws.translate#InvalidRequestException is a client error with httpError 400 and a Message member.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture translate-document-unsupported-content-type-rejected, step task#3 — call TranslateDocument with a Document whose ContentType is outside {text/plain, text/html, application/vnd.openxmlformats-officedocument.wordprocessingml.document} (the fixture uses application/json).
4. Observe the HTTP 400 error body: the Message must be exactly 'The provided ContentType is not supported.' with no interpolation of the rejected content type and no appended list of supported types.
