Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-create-event-bus-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-create-event-bus-validation-errors eventbridgev2-event-source-and-subscriber-reference-missing-bus
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|$.Message
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|$.__type
Issue-Key: eventbridgev2|CreateEventBus|4xx->2xx|status
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|$.Message
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|$.__type
Issue-Key: eventbridgev2|CreateEventSource|4xx->4xx|status
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# eventbridgev2 creates no longer refuse a ClientToken-less create, and validate request parameters only after resolving the event bus

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
TWO causes, both on service token eventbridgev2 (sdkId EventBridgeV2, AWSEventsV2 v2025-05-15, rpcv2Cbor; model services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json). They share one fault: the resource-creating operations of this service no longer answer the ClientToken refusal the corpus records for them, and where a refusal is still produced it is produced only AFTER the request's named resource has been resolved, so a request-parameter fault is reported as a resource fault. Both causes must be closed before this issue is submitted. The other four clusters in this replay report a non-empty owned_by (20261113T174325Z-eventbridgev2-subscriber-error-paths-answer-the-wrong-status.md, 20261120T064841Z-eventbridgev2-updatesubscriber-skips-target-parameter-and-cross-account.md) and are not re-filed here.

Cause 1 of 2: CreateEventBus answers HTTP 200 and mints an event bus for a create the recording shows AWS refuses with HTTP 400 InvalidInputException / 'ClientToken is required' (fixture eventbridgev2-create-event-bus-validation-errors, step task#1).

AWS: recorded behavior only for the refusal itself. Smithy (com.amazonaws.eventbridgev2#CreateEventBus) marks only Name @required (EventBusName, @length 1..256, @pattern ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$); ClientToken is modeled @idempotencyToken with @length 1..64 and @pattern ^[\u0021-\u007E]+$, and the operation's 400-class errors are LimitExceededException and the service-common InvalidInputException ('A request parameter is missing or not valid.'). The recorded response for this step of the validation-errors fixture is HTTP 400 with __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException' and Message 'ClientToken is required' — so for this request shape the service DOES treat the client token as a parameter fault of the InvalidInputException class. The message is wholly fixed text: 'ClientToken is required' interpolates nothing from the request or from state, and the member name 'ClientToken' in it is the modeled member name, not a request value. Hypothesis (neither the model nor this recording states it): the differentiator between this step and the step behind open issue 20261121T184930Z-eventbridgev2-createeventbus-still-refuses-a-create-that-omits.md — which records a 200 for a create that carries no token — is a ClientToken that is PRESENT but cannot satisfy @length min 1 / @pattern ^[\u0021-\u007E]+$ (empty or blank), i.e. a stated-but-unusable token is the parameter fault while a wholly omitted one is a legal non-idempotent create. A fixer must reconcile the two recordings rather than reinstate or remove the rule unconditionally; both fixture steps have to pass together.

Emulator: the step is accepted. HTTP 200 is served with a full created-bus body and no error at all — $.__type is absent (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException') and $.Message is absent (expected 'ClientToken is required') — so a create AWS refuses leaves a real event bus in the store, under the name the request asked for. The handler now consults the client token only when it is Some(token), purely to replay an earlier create, and imposes no requirement of its own.

Why: a client that relies on the service to reject a malformed/absent idempotency token gets a silently created bus instead of a 400. It cannot distinguish 'my token was bad' from 'my create was idempotently honoured', and every retry of the same bad request mints or collides with another bus (the next attempt then meets ResourceAlreadyExistsException/409 instead of the 400 it should have had the first time). Any test or control loop asserting the refusal passes vacuously, and the leaked bus becomes the parent of event sources and subscribers that should never have existed.

Fix: the CreateEventBus handler's pre-store request validation in services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs, together with the shared client-token helper in src/state.rs that the sibling creates reuse. The refusal must be decided from the request alone and raised before the store is locked, so a refused create mints nothing; the existing Some(token) idempotent-replay / IdempotentParameterMismatchException path is unaffected.

Traces: eventbridgev2-create-event-bus-validation-errors#task#1.

Cause 2 of 2: CreateEventSource answers HTTP 404 ResourceNotFoundException for a request that is BOTH missing its ClientToken and names a non-existent event bus, where the recording shows HTTP 400 InvalidInputException / 'ClientToken is required' (fixture eventbridgev2-event-source-and-subscriber-reference-missing-bus, step task#2).

AWS: recorded behavior only for the ordering. Smithy (com.amazonaws.eventbridgev2#CreateEventSource) requires Name, EventBusArn and Configuration, models ClientToken as @idempotencyToken (@length 1..64, @pattern ^[\u0021-\u007E]+$), and models BOTH InvalidInputException (@httpError 400) and ResourceNotFoundException (@httpError 404). The recorded response for this step is HTTP 400 with __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException' and Message 'ClientToken is required' even though the EventBusArn in the request names a bus that does not exist — so the service answers the request-parameter fault FIRST and never reaches the lookup of the resource the request names. The required behavior is therefore an ordering rule: parameter-level refusals of this error class precede resolution of the named event bus, and the 400 is the answer whenever both faults are present. Message text as above: fixed text, no interpolation.

Emulator: HTTP 404 with __type 'ResourceNotFoundException' and Message 'Event bus not found: <bus name from the request>' — the handler locks the store and resolves input.event_bus_arn (create_event_source.rs, the `let Some(bus) = state.bus(&input.event_bus_arn) else { ... event_bus_not_found(...) }` arm) before any client-token check, and the client-token check no longer exists at all. Both the status/code and the message diverge: the emulator's message interpolates the requested bus name, while the required answer for this request carries the fixed 'ClientToken is required' instead.

Why: a client gets the wrong diagnosis and the wrong retry class. A 404 ResourceNotFoundException tells it to go create or wait for the bus, when the real defect is in its own request parameters; conversely, code that probes whether a bus exists by attempting a create can be told 'not found' for a bus that does exist (or 'found' semantics withheld) depending on which fault the handler happens to reach first. Error-class-driven retry and alarm logic keyed on 400 vs 404 takes the wrong branch.

Fix: the CreateEventSource handler's validation ordering in services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_source.rs — move the request-only refusals (the client-token rule restored in Cause 1, alongside the existing event_source_configuration_fault check that is already answered before the store is touched) ahead of the store lock and the bus lookup, so a request that is invalid in itself is refused for that and never resolves the bus. Keep the ResourceNotFoundException path for requests whose parameters are valid. The same ordering applies to the sibling CreateSubscriber step (task#3) of this fixture, whose divergence is owned by 20261113T174325Z-eventbridgev2-subscriber-error-paths-answer-the-wrong-status.md and is not re-filed here.

Traces: eventbridgev2-event-source-and-subscriber-reference-missing-bus#task#2.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting): 17 fixtures replayed, 200 steps — 194 passed, 6 mismatched, 0 blocked, 0 errored, score 97.0%, 6 divergence clusters. Four clusters carry a non-empty owned_by and are not repeated; the two below are the clusters with owned_by empty and stub_path empty.

[mismatch] Cause 1 — eventbridgev2 CreateEventBus, fixture eventbridgev2-create-event-bus-validation-errors, step task#1 (cluster of 1):
  divergence_key: service=eventbridgev2|CreateEventBus|status=400->200|keys=$.Message,$.__type,status
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got None)
  $.Message: missing (expected 'ClientToken is required', got None)
  Expected: HTTP 400, __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', Message 'ClientToken is required' (entirely fixed text; the only variable-looking token in it, 'ClientToken', is the modeled member name), and NO event bus created.
  Actual: HTTP 200 carrying a created-bus body, no __type and no Message, and a bus left in the store. The generated ARN id, name suffix and timestamps in that body are per-run values and are not themselves requirements.

[mismatch] Cause 2 — eventbridgev2 CreateEventSource, fixture eventbridgev2-event-source-and-subscriber-reference-missing-bus, step task#2 (cluster of 1):
  divergence_key: service=eventbridgev2|CreateEventSource|status=400->404|keys=$.Message,$.__type,status
  status: expected 400, got 404
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ResourceNotFoundException')
  $.Message: value_mismatch (expected 'ClientToken is required', got 'Event bus not found: sim2real-probe-ebv2-missing')
  Expected: HTTP 400, __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', Message 'ClientToken is required' — the parameter fault answered before the named bus is resolved.
  Actual: HTTP 404, __type 'ResourceNotFoundException', Message 'Event bus not found: <name>' where <name> is interpolated from the request's EventBusArn (the recorded run's 'sim2real-probe-ebv2-missing' is that run's value, not part of the requirement).

Code carrying the behavior in the CURRENT checkout (orientation only): services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs:234-246 — the client token is consulted only as `if let Some(token) = input.client_token.as_deref()` for idempotent replay, with the comment that a create stating no token 'mints a bus of its own', i.e. no refusal remains; services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_source.rs:224-235 — the store is locked and `state.bus(&input.event_bus_arn)` resolved (returning crate::state::event_bus_not_found) before any token handling at lines 262-269.

Model references (same model file): CreateEventBusRequest required = {Name}; CreateEventSourceRequest required = {Name, EventBusArn, Configuration}; ClientToken @idempotencyToken, @length 1..64, @pattern ^[\u0021-\u007E]+$ on both; CreateEventBus errors include InvalidInputException/400 and LimitExceededException/400; CreateEventSource errors include InvalidInputException/400 and ResourceNotFoundException/404.

Related open issue, named because it looks similar and does NOT cover these steps: 20261121T184930Z-eventbridgev2-createeventbus-still-refuses-a-create-that-omits.md reports the OPPOSITE direction on a different fixture step (eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4, a 400 where a 200 was recorded). That step now passes; the two steps named here diverge, and replay reported owned_by empty for both.

## Steps to Reproduce
1. git_clone this repo, then replay_wire_corpus (trace set simparity-wire-aws-v1), which builds and boots the emulator from the checkout.
2. Cause 1 — fixture eventbridgev2-create-event-bus-validation-errors, step task#1: send that fixture's recorded rpcv2Cbor AWSEventsV2.CreateEventBus request (the one whose recorded answer is the ClientToken refusal). Observe HTTP 200 with a created-bus body where HTTP 400 / __type 'com.amazon.nexus.v20250515.exceptions#InvalidInputException' / Message 'ClientToken is required' was recorded, and observe that a bus is now present (ListEventBuses / DescribeEventBus for that name) where AWS created none.
3. Cause 2 — fixture eventbridgev2-event-source-and-subscriber-reference-missing-bus, step task#2: send the recorded AWSEventsV2.CreateEventSource request, whose EventBusArn names a bus that was never created and which carries no usable ClientToken. Observe HTTP 404 / __type 'ResourceNotFoundException' / Message 'Event bus not found: <requested bus name>' where HTTP 400 / 'com.amazon.nexus.v20250515.exceptions#InvalidInputException' / 'ClientToken is required' was recorded — i.e. the bus lookup is reached before the parameter check.
4. Cross-check the ordering independently: repeat the same CreateEventSource request WITH a valid ClientToken and confirm the 404 ResourceNotFoundException is then the correct answer; the 400 must win only while a parameter fault is present.
5. Cross-check the reconciliation before submitting: re-replay BOTH eventbridgev2-create-event-bus-validation-errors#task#1 and eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4 (the 200-expecting create in open issue 20261121T184930Z) and confirm they pass together — a blanket reinstatement of 'ClientToken is required' breaks the second, a blanket removal breaks the first.
6. After the fix, re-replay eventbridgev2-create-event-bus-validation-errors#task#1 and eventbridgev2-event-source-and-subscriber-reference-missing-bus#task#2 and confirm both answer the recorded status, __type and Message.
