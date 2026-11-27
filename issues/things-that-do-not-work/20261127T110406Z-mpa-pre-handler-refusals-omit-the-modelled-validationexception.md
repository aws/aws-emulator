Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-cancel-session-arn-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-cancel-session-arn-validation
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|CancelSession|4xx->4xx|$.Message
Issue-Commit: 558d2bea4a2bc55ff66bbf3c15524a570b71a39f

# MPA pre-handler refusals omit the modelled ValidationException Message member: CancelSession's malformed SessionArn 400 carries a lowercase "message" instead

> _Note: this issue was identified against the code base at commit `558d2bea4a2bc55ff66bbf3c15524a570b71a39f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelSession

## Description
One cause.

Cause 1 of 1: a CancelSession request whose `SessionArn` is not a well-formed MPA session ARN is refused on the right status with the right error code, but the response body carries NO `Message` member, so a client reading the refusal's message gets nothing.

AWS: Smithy — `CancelSession` (PUT /sessions/{SessionArn}) takes `SessionArn` as a @required @httpLabel whose target `com.amazonaws.mpa#SessionArn` is `@length(20..2048)` and `@pattern("^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$")`, and declares `ValidationException` with `@httpError(400)`. `ValidationException` has exactly one member, `Message` (target `String`, **@required**, `@length(max: 1000)`, no `jsonName`), so on this service's `restJson1` protocol the member goes on the wire under the name `Message` — capital M. A value that is not a well-formed MPA ARN is therefore answered with HTTP 400, error code `ValidationException`, and a body whose `Message` carries the one fixed sentence this service words that class with. The recorded response fixes that sentence as the entirely fixed text `Invalid Resource Arn` — no part of it is taken from the request, the rejected value is not echoed, and neither the member name nor the model's regex appears in it (the emulator's own crate records the same rule: `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:4308-4326`, `INVALID_ARN_MESSAGE` / `invalid_arn()`).

Emulator: the replayed step gets HTTP 400 — the expected status — but the body has no `Message` key at all: the comparison reports `$.Message: missing (expected 'Invalid Resource Arn', got None)`. The wording itself is not lost; it is published under the WRONG member name. The service does reword this class: `src/service.rs:201-245` (`override_refusal`) matches a `Refusal::Invalid` whose violations include an ARN-pattern violation (`crate::state::is_malformed_arn_violation`, `src/state.rs:4381-4390`) and hands back `Refused { code: "ValidationException", message: INVALID_ARN_MESSAGE }`. But a refusal raised at the framework's constraint boundary — which is where a member carrying an ARN `@pattern` is refused, before any handler runs — is rendered by the shared pre-dispatch JSON envelope, and that envelope writes the message under a lowercase literal key: `vendor/smithy-server-runtime/src/pre_dispatch.rs:464` emits `{"__type":"<type>","message":"<text>"}`. So the emulator answers `{"__type":"ValidationException","message":"Invalid Resource Arn"}` where AWS answers a `ValidationException` document whose required `Message` member carries that sentence. Nothing in the service's own path is wrong about the status, the error code or the text — only the member name the text is published under. This is the rendering every pre-handler refusal of this service passes through, not something specific to `CancelSession`: any ARN-valued member with an `@pattern`, and any absent @required member or out-of-bound value, is answered the same way.

Why: a client cannot read the refusal. An AWS SDK deserializes this error against the modelled `ValidationException` shape, whose single member is the @required `Message`; a body that has no `Message` leaves that member unset, so `e.message()` is empty/None and the SDK (or a strict deserializer) either surfaces a refusal with no explanation or fails to deserialize a required member. A caller that branches on, logs, or shows the validation text — the normal way a 400 from an ARN-valued label is handled — sees nothing, and the malformed-ARN class becomes indistinguishable from any other 400 on the same operation. Handler-raised errors of the SAME service are unaffected (they are serialized through the modelled error schema and so do carry `Message`), so the service is internally inconsistent: the same `ValidationException`, with the same status and code, reaches the client under two different member names depending on whether a handler or the constraint boundary raised it.

Fix: the refusal SERIALIZER for this service's pre-handler refusals. The `override_refusal` seam (`src/service.rs:201-245`) only chooses a code and a message; the body is built by the shared JSON pre-dispatch envelope, which hardcodes the key `message` and cannot know that this model names the member `Message`. The refusal must be emitted through the modelled `ValidationException` shape — the same path a handler-returned one takes, so the member name comes from the model — either by rendering a chosen refusal through the service's protocol/error schema, or, for the ARN class specifically, by screening the ARN-valued member in the operation's own validation and returning the modelled `ValidationException(crate::state::invalid_arn())` on the operation's error channel, exactly as the `ResourceArn` family already does (`src/ops/list_tags_for_resource.rs`, `src/ops/get_resource_policy.rs`). Status (400) and error code (`ValidationException`) must stay as they are; only the member the text is written under changes. The same correction fixes every other framework-boundary refusal of this service, which all render through that one envelope.

Related open issues: none of the open `mpa` issues covers this. `20261123T025202Z-mpa-listtagsforresource-maintainability` is about the duplicated ordered `ResourceArn` screen in five HANDLERS (and explicitly leaves every status, code and message unchanged); `20261125T082728Z-mpa-deleteidentitysource-error-response--r2` and `20261127T094606Z-mpa-startactiveapprovalteamdeletion-error-response` concern handler-raised message text, not the member name a boundary refusal is published under.

Traces: mpa-cancel-session-arn-validation#task#1 and #task#2 (one cluster, 2 affected steps, verdict mismatch, divergence key `service=mpa|CancelSession|status=400->400|keys=$.Message`).

## Evidence
Replay cluster (trace set simparity-wire-aws-v1):
  fixture_id: mpa-cancel-session-arn-validation
  step_key:   task#1
  service:    mpa   operation: CancelSession   verdict: mismatch
  affected:   ["mpa-cancel-session-arn-validation#task#1", "mpa-cancel-session-arn-validation#task#2"] (affected_count 2)
  expected_status 400 -> actual_status 400
  detail: "$.Message: missing (expected 'Invalid Resource Arn', got None)"

Expected (AWS / recorded):
  HTTP/1.1 400
  x-amzn-errortype: ValidationException
  {"__type":"ValidationException","Message":"Invalid Resource Arn"}
  — `Message` is the modelled member name (ValidationException$Message, @required,
    @length(max:1000), no jsonName). Fixed text: the whole sentence "Invalid Resource Arn";
    variable parts: none (the rejected ARN is NOT echoed, the member name and the @pattern
    are NOT named).

Actual (emulator):
  HTTP/1.1 400
  {"__type":"ValidationException","message":"Invalid Resource Arn"}
  — the text is right, the status is right, the error code is right; the member name is
    lowercase `message`, so the modelled required `Message` member is absent.

Code evidence:
  - services/service-com-amazonaws-mpa-awsfluffycoreservice/src/service.rs:230-240 — the ARN
    class is reworded to `Refused { code: "ValidationException", message:
    crate::state::INVALID_ARN_MESSAGE }`, so the sentence is correct at the seam.
  - services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:4317 —
    `INVALID_ARN_MESSAGE = "Invalid Resource Arn"`.
  - services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:4385-4390 —
    `is_malformed_arn_violation` matches `ViolationKind::Pattern` for an `^arn:` pattern,
    which `SessionArn`'s @pattern is.
  - vendor/smithy-server-runtime/src/pre_dispatch.rs:464 — the JSON envelope that renders a
    chosen refusal: `format!(r#"{{"__type":"{type_name}","message":"{message}"}}"#)` — the
    key is the hardcoded lowercase `message`, never the model's member name.
  - services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/cancel_session.rs:17-20 —
    the handler's own docs state this class is answered with `crate::state::INVALID_ARN_MESSAGE`
    as a `ValidationException` 400, i.e. the service intends the modelled shape.
  - Contrast, same service, same error shape, handler-raised and therefore serialized through
    the modelled schema (member `Message`): src/ops/list_tags_for_resource.rs:165-176 returning
    `ValidationException(crate::state::invalid_arn())`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `mpa-cancel-session-arn-validation` (steps task#1 and task#2).
2. Equivalently, by hand: send `PUT /sessions/<value>` to the mpa endpoint with a SigV4-signed request, where `<value>` is a value that is not a well-formed MPA session ARN (anything failing `^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$`, e.g. `not-an-arn` or an `arn:aws:mpa:...:approval-team/team-1` that names the wrong resource kind, percent-encoding the slashes in the label).
3. Observe the response: status 400 and `__type` `ValidationException` are correct, but the body is `{"__type":"ValidationException","message":"Invalid Resource Arn"}` — parse it as JSON and `$.Message` is absent.
4. Compare with a handler-raised `ValidationException` of the same service — e.g. `GET /resource-policies/<malformed-ResourceArn>` style reads through `src/ops/list_tags_for_resource.rs` — whose body does carry `Message`. The same modelled error reaches the client under two different member names.
5. Re-run the fixture after the fix: both steps must return 400 with `$.Message == "Invalid Resource Arn"`.
