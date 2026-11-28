Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|$.CreationTime
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|$.EventBusArn
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|$.Name
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|$.State
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|$.StorageConfiguration
Issue-Key: eventbridgev2|CreateEventBus|2xx->4xx|status
Issue-Commit: d9773982178bffc206ea9531196a8841a020d5e3

# eventbridgev2 CreateEventBus still refuses a create that omits ClientToken (400 InvalidInputException instead of minting the bus)

> _Note: this issue was identified against the code base at commit `d9773982178bffc206ea9531196a8841a020d5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
One cause, on service token eventbridgev2 (sdkId EventBridgeV2, AWSEventsV2 v2025-05-15, rpcv2Cbor; model services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json). This is the ONLY net-new (unowned, non-stub-blocked) divergence cluster in this replay of simparity-wire-aws-v1; every other cluster is already carried by an open issue or by a stub issue filed automatically.

Cause 1 of 1: CreateEventBus answers HTTP 400 instead of creating the event bus when the request carries no ClientToken (fixture eventbridgev2-subscriber-delivers-published-event-to-sqs, step task#4).

AWS: Smithy (com.amazonaws.eventbridgev2#CreateEventBus) marks exactly ONE input member @required — Name (EventBusName, @length 1..256, @pattern ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$). ClientToken is modeled only as @idempotencyToken (ClientToken, @length 1..64, @pattern ^[\u0021-\u007E]+$) and is NOT @required; the operation doc says "Retries carrying the same ClientToken are idempotent" and the member doc says "Unique, case-sensitive identifier that ensures the idempotency of the request. The SDK generates one automatically when the field is omitted" — so omitting it yields a legal, merely non-idempotent create, not a validation failure. StorageConfiguration is optional (StorageConfiguration.RetentionPeriodInDays @range 1..365). A valid create therefore answers HTTP 200 with EventBusArn, Name, State 'CREATING' (provisioning is asynchronous: BusState CREATING -> ACTIVE), StorageConfiguration (StorageConfigurationOutput = RetentionPeriodInDays + RetentionWindowStartTime) carrying the bus's effective retention period, and CreationTime. None of the operation's modeled errors covers "the caller omitted an optional idempotency token": its 400-class errors are LimitExceededException ("The request would exceed a service quota for the account") and the service-common InvalidInputException ("A request parameter is missing or not valid."), and an absent optional member is neither.

Emulator: the step is refused with HTTP 400 and no success body at all — EventBusArn, Name, State, StorageConfiguration and CreationTime are all absent — so no bus is minted. The refusal is the service's own invented requirement: the handler returns the modeled InvalidInputException whose whole Message is the fixed text "ClientToken is required" whenever input.client_token is None. That text is entirely fixed (it interpolates nothing from the request or from state) and the same rule is applied by all three creates (CreateEventBus, CreateEventSource, CreateSubscriber). Hypothesis (the model does not state it and the recording shows only the 200): this fixture's create names only Name — its expected StorageConfiguration.RetentionPeriodInDays of 1 is the service default rather than a requested value — so with no ClientToken on the wire it is refused, while every other fixture in the corpus passes a token (e.g. 'sim2real-probe-tok-...') and creates its bus successfully. The only other 400 path in this handler is LimitExceededException, bounded at 100 buses per region / 1000 per process (state.rs MAX_EVENT_BUSES, MAX_EVENT_BUSES_PER_EMULATOR) — orders of magnitude above this corpus — and it is not reachable here.

REGRESSION / INCOMPLETE FIX, which is why this is filed again: the RESOLVED issue 20261116T144630Z-eventbridgev2-createeventbus-refuses-a-create-that-omits-clienttoken.md reported exactly this fixture step against the validation front door (service/validation.rs). That front door no longer carries the rule — it now documents "This door adds no requirement of its own: it restates the model's refusals in the modelled shape, and refuses nothing the model accepts" — but the requirement was MOVED into the operation handler and the shared state module instead of removed, so the wire behavior is unchanged and the step still diverges. A resolved issue covers no failure, so the cause is net-new work.

Why: a client that calls CreateEventBus without supplying its own idempotency token — every caller using a raw HTTP/CBOR request, a hand-rolled client, or any SDK path that does not auto-fill the token — cannot create an event bus at all against the emulator, and is told a parameter AWS treats as optional is missing. Because the bus is the root of this service's resource tree the refusal cascades: no event source, no subscriber, no PutEvents and no subscriber delivery can be exercised for that bus (in this fixture it is what leaves the subscribed SQS queue empty at task#9/task#10).

Fix: the CreateEventBus handler's pre-store validation, not the request walk — the missing-ClientToken refusal in services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs and the shared CLIENT_TOKEN_REQUIRED_MESSAGE / client_token_required() helper in src/state.rs that the two sibling creates reuse. An omitted ClientToken must reach the normal create path and perform a plain non-idempotent create (the token-replay / IdempotentParameterMismatchException path already applies only when a token IS present, and bus_by_client_token is only consulted for Some(token)). The model's own constraints must keep being enforced in the modeled InvalidInputException shape exactly as they are today.

Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#4 (the refused create). The downstream delivery failures of the same fixture (task#9 GetQueueAttributes, task#10 ReceiveMessage) are owned by open sqs issues and are not re-filed here.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting): 17 fixtures replayed, 200 steps — 144 passed, 47 mismatched, 9 blocked, 0 errored, score 72.0%, 17 divergence clusters. 16 of those clusters report a non-empty owned_by (open eventbridgev2/sqs issues) and are not repeated; this is the one cluster with owned_by empty and stub_path empty.

[mismatch] eventbridgev2 CreateEventBus, fixture eventbridgev2-subscriber-delivers-published-event-to-sqs, step task#4 (cluster of 1):
  divergence_key: service=eventbridgev2|CreateEventBus|status=200->400|keys=$.CreationTime,$.EventBusArn,$.Name,$.State,$.StorageConfiguration,status
  status: expected 200, got 400
  $.Name: missing (expected the requested bus name, got None)
  $.EventBusArn: missing (expected arn:aws:events:<region>:<account>:event-busv2/<name>/<25-char id>, got None)
  $.State: missing (expected 'CREATING', got None)
  $.StorageConfiguration: missing (expected {'RetentionPeriodInDays': 1, 'RetentionWindowStartTime': <shaped-not-asserted>}, got None)
  $.CreationTime: missing (expected <shaped-not-asserted>, got None)

Expected: HTTP 200 with the bus minted — EventBusArn, Name, State 'CREATING', StorageConfiguration carrying the effective RetentionPeriodInDays (1 here, the service default for a create naming no StorageConfiguration) plus a RetentionWindowStartTime, and a CreationTime. The generated name segment, the 25-char ARN id and the timestamps are per-run values and are not requirements.

Actual: HTTP 400 with no success body. The refusal served is the modeled InvalidInputException (__type 'InvalidInputException', @httpError 400) whose Message is the wholly fixed, emulator-authored text 'ClientToken is required' — a requirement the Smithy model does not express, since ClientToken is @idempotencyToken and not @required. AWS has no refusal of this error class for an omitted optional idempotency token at all, so the correct answer carries no error code or message.

Code carrying the invented rule in the CURRENT checkout (orientation only): services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_bus.rs:212-217 (`if input.client_token.is_none() { return Err(... crate::state::client_token_required()) }`, ahead of the store lock) and src/state.rs:138-160 (CLIENT_TOKEN_REQUIRED_MESSAGE = "ClientToken is required" and client_token_required(), documented there as "the requirement is the SERVICE's" even though the model leaves the member optional). The replay-match path immediately below it (create_event_bus.rs:243-259) already handles Some(token) only, so removing the refusal leaves a correct non-idempotent create. src/service/validation.rs:40-42 confirms the front door no longer imposes the rule that the previously resolved issue targeted.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): CreateEventBusRequest required members = {Name}; ClientToken @idempotencyToken, @length 1..64, @pattern ^[\u0021-\u007E]+$, doc "The SDK generates one automatically when the field is omitted"; operation doc "Creates an event bus. Provisioning is asynchronous: the bus is returned in the CREATING state and transitions to ACTIVE when ready (see the EventBusActive waiter). Retries carrying the same ClientToken are idempotent."; errors = ConcurrentModificationException/409, IdempotentParameterMismatchException/409, InvalidStateException/409, LimitExceededException/400, ResourceAlreadyExistsException/409, AccessDeniedException/403, InternalException/500, InvalidInputException/400, ThrottlingException/429; output = EventBusArn, Name, Description, EncryptionConfiguration, StorageConfiguration (StorageConfigurationOutput), State (BusState enum incl. CREATING), StateReason, CreationTime.

## Steps to Reproduce
1. git_clone this repo, then replay_wire_corpus (trace set simparity-wire-aws-v1), which builds and boots the emulator from the checkout.
2. Let fixture eventbridgev2-subscriber-delivers-published-event-to-sqs run its setup steps (SQS queue creation, queue policy, IAM role) — they answer as recorded.
3. Step task#4: send the rpcv2Cbor request for AWSEventsV2.CreateEventBus with ONLY Name set to a valid bus name (no ClientToken, no StorageConfiguration).
4. Observe HTTP 400 with __type 'InvalidInputException' and Message 'ClientToken is required', and no bus created, where AWS answers HTTP 200 with EventBusArn, Name, State 'CREATING', StorageConfiguration (RetentionPeriodInDays defaulted to 1 plus RetentionWindowStartTime) and CreationTime.
5. Repeat the identical request WITH a ClientToken and observe it succeed — confirming the refusal is driven solely by the omitted optional token.
6. After the fix, re-replay fixture eventbridgev2-subscriber-delivers-published-event-to-sqs step task#4 and confirm the 200 and the full response body; the fixture's later steps (task#9 GetQueueAttributes, task#10 ReceiveMessage) depend on this bus existing, though their own divergences are owned by separate open sqs issues.
