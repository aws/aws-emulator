Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-account-bootstrap
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-account-bootstrap
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|InitializeService|4xx->2xx|$headers.content-length
Issue-Related: 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md
Issue-Commit: 992eebbb1a1c5db8bb6bfb43357b4e2de2d0ccf2

# mgn InitializeService always answers 204 and never serializes the modeled ValidationException error response

> _Note: this issue was identified against the code base at commit `992eebbb1a1c5db8bb6bfb43357b4e2de2d0ccf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitializeService

## Description
Single cause (Cause 1 of 1): the mgn InitializeService handler has only a success path, so a request that AWS rejects with a client error is answered with the operation's 204 success code and an entirely empty response envelope (no status, no error body, no x-amzn-errortype, no Content-Length).

NOTE ON EXISTING ISSUES: open issue 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md describes the same operation and the replay report does NOT mark this cluster as owned by it (owned_by is empty), because the current cluster key covers an additional diverging key ($headers.content-length) beyond the keys that issue recorded. This issue is therefore filed as net-new and supersedes/extends that one; the missing Content-Length on an empty 204 body is also filed separately for the tag operations as 20261104T100723Z-mgn-empty-body-204-tag-responses-omit-the.md, but that issue covers TagResource/UntagResource only, not the InitializeService error path.

AWS: Smithy (model services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json, service com.amazonaws.mgn#ApplicationMigrationService) models InitializeService with http {method: POST, uri: /InitializeService, code: 204} and errors AccessDeniedException (httpError 403) and ValidationException (httpError 400). com.amazonaws.mgn#ValidationException is a client error with members message, code (both LargeBoundedString, @length 0..65536), reason (ValidationExceptionReason, values unknownOperation|cannotParse|fieldValidationFailed|other) and fieldList (ValidationExceptionFieldList). When account initialization cannot complete, AWS answers HTTP 400 with response header x-amzn-errortype: ValidationException, a Content-Length matching the serialized body, and an awsJson-shaped body carrying __type com.amazon.mgn#ValidationException plus reason and message. In the recorded response the message is "Failed to create SLR or instance profiles" and reason is "OTHER": for this error class the requirement is the 400 status, the ValidationException error code/errortype, the __type discriminator and a reason drawn from ValidationExceptionReason, with the message text stating the initialization step that failed (service-linked role / instance profile creation). The specific sentence is recorded behavior only - its fixed part is the "Failed to create SLR or instance profiles" phrasing AWS uses for the SLR/instance-profile initialization failure; it carries no request- or state-derived values in this case, so no per-run value should be templated out of it.

Emulator: the request is answered HTTP 204 with an empty body, no x-amzn-errortype header and no Content-Length. Replay detail verbatim: "status: expected 400, got 204"; "$: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})"; "$headers.content-length: missing (expected '123', got None)" (the 123 is one recording's byte count, not a requirement - the requirement is that a non-empty error body is accompanied by its Content-Length); "$headers.x-amzn-errortype: missing (expected 'ValidationException', got None)". The handler unconditionally returns the modeled 204 success code and has no route to the modeled error shapes.

Why: a client bootstrapping an account through InitializeService is told initialization succeeded when the service-linked role / instance profiles were not created. SDKs decide success purely from the 2xx status and the absence of x-amzn-errortype, so no ValidationException is thrown, reason/message are never surfaced, no retry or remediation runs, and every subsequent mgn call proceeds against an uninitialized account - a silent success instead of a surfaced client error. The absent Content-Length additionally breaks clients and proxies that frame the response body from that header.

Fix: the mgn InitializeService handler plus its error serializer. The handler must be able to conclude in failure based on the initialization state it actually attempts to establish, rather than always returning the 204 success code; the shared mgn error serializer must emit HTTP 400, the x-amzn-errortype: ValidationException header, a Content-Length matching the serialized payload, and a body with __type com.amazon.mgn#ValidationException and the reason member populated from ValidationExceptionReason (plus message/code/fieldList when known).

Traces: fixture mgn-account-bootstrap, step task#1 (sole member of the cluster; affected_count 1).

## Evidence
Fixture mgn-account-bootstrap, step task#1, operation InitializeService, service mgn, verdict mismatch.

Expected (recorded AWS response):
  HTTP 400
  x-amzn-errortype: ValidationException
  content-length: <length of serialized error body> (recording showed 123)
  body: {"__type": "com.amazon.mgn#ValidationException", "reason": "OTHER", "message": "Failed to create SLR or instance profiles"}

Actual (emulator):
  HTTP 204
  no x-amzn-errortype header
  no content-length header
  empty body ({})

Replay detail verbatim:
  status: expected 400, got 204
  $: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})
  $headers.content-length: missing (expected '123', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Model support: InitializeService http {POST /InitializeService, code 204}; errors AccessDeniedException (403), ValidationException (400); ValidationException members message/code/reason/fieldList with reason -> ValidationExceptionReason [unknownOperation, cannotParse, fieldValidationFailed, other].

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or replay fixture mgn-account-bootstrap alone.
3. Observe step task#1: POST /InitializeService against the mgn endpoint.
4. The emulator answers HTTP 204, empty body, no x-amzn-errortype and no Content-Length; the recorded AWS response is HTTP 400 with x-amzn-errortype: ValidationException, a Content-Length, and a com.amazon.mgn#ValidationException body (reason OTHER).
5. After the fix, re-replay mgn-account-bootstrap#task#1 and confirm the 400 / ValidationException response with the errortype and Content-Length headers.

## Resolution (Cause 1 of 1 declined; the rule it is about is implemented and now pinned on the wire)

The handler defect this report describes no longer exists: `InitializeService` establishes the
account's service-linked role and the two server instance profiles in the IAM service that owns
them, records the account as initialized only once IAM reports all three, and answers an attempt
IAM refuses with the modelled `ValidationException` — HTTP 400, `x-amzn-errortype:
ValidationException`, a body carrying the `__type` discriminator, the modelled `reason`, and the
sentence naming WHICH of those resources could not be created, framed with that body's own
`content-length`. That was implemented under the related report
`20261104T132908Z-mgn-initializeservice-always-answers-204-success-and-never.md`, after the commit
this one was filed against.

The cause is DECLINED because the recorded 400 is a fact about the RECORDED account's IAM (its
service-linked role and instance profiles could not be created there), while this emulator's IAM
holds no such obstruction: the same call establishes them and concludes with the 204 the model
states for the success. Refusing a request whose work actually succeeded would be a refusal fitted
to one recording. Every deterministic condition that does make the establishment fail is served as
the modelled refusal: the account at IAM's role quota (`Failed to create SLR or instance
profiles`, the recorded sentence), at its instance-profile quota (`Failed to create instance
profiles`), and an account holding the profiles but unable to hold the role (`Failed to create
SLR`). The replay key `mgn|InitializeService|4xx->2xx|$headers.content-length` therefore remains
unmatched by design.

What this change added: `tests/all/initialization_refusal_on_the_wire.rs`, which serves the two
services on a loopback port and reads the raw bytes, pinning status, `x-amzn-errortype`, the error
body and a `content-length` equal to the body that arrived (never the recording's byte count) over
a table of the three obstruction kinds plus the unobstructed success.
