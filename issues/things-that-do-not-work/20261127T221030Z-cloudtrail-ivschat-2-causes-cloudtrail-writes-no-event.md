Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ivschat.chat-token-and-moderation-calls-are-audited-without-their-secrets cloudtrail__ivschat.chat-token-session-over-180-minutes-is-rejected-and-audited cloudtrail__ivschat.logging-configuration-journey-is-audited-with-its-destination cloudtrail__ivschat.room-journey-is-audited-call-by-call-with-read-only-flags cloudtrail__ivschat.update-of-a-deleted-room-is-rejected-and-audited-as-not-found
Issue-Key: cloudtrail__ivschat|SendEvent|NotImplemented
Issue-Key: cloudtrail__ivschat.chat-token-session-over-180-minutes-is-rejected-and-audited|_await#12|Timeout|time.sleep(pause)
Issue-Key: cloudtrail__ivschat.logging-configuration-journey-is-audited-with-its-destination|_poll_records#25|Timeout|time.sleep(pause)
Issue-Key: cloudtrail__ivschat.room-journey-is-audited-call-by-call-with-read-only-flags|_await_records#11|Timeout|time.sleep(pause)
Issue-Key: cloudtrail__ivschat.update-of-a-deleted-room-is-rejected-and-audited-as-not-found|_until_audited#14|Timeout|time.sleep(pause)
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudtrail__ivschat: 2 causes — CloudTrail writes no event-history record for any ivschat management call, and ivschat SendEvent is an unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Two net-new root causes behind all five failing checks listed under `### cloudtrail__ivschat`.

Cause 1 of 2: no CloudTrail event-history record is ever written for Amazon IVS Chat management calls, so any audit poll for eventSource `ivschat.amazonaws.com` never completes.
AWS:      IVS Chat is a CloudTrail-integrated service: every ivschat control-plane call (CreateRoom, UpdateRoom, GetRoom, DeleteRoom, CreateChatToken, CreateLoggingConfiguration, GetLoggingConfiguration, UpdateLoggingConfiguration, ListLoggingConfigurations, DisconnectUser, DeleteMessage) is recorded in CloudTrail event history as a management event with `eventSource` = `ivschat.amazonaws.com`, the serving call's `requestID`, the `eventName` of the operation, `readOnly` true for the Get*/List* reads and false for the writes, the `requestParameters` actually sent, and — for a refused call — `errorCode`/`errorMessage` plus `responseElements` carrying the refusal's `Code`, `Message` and `Details` (e.g. `Details.reason` = `FIELD_VALIDATION_FAILED` for a ValidationException, `Details.resourceType` = `ROOM` for a room ResourceNotFoundException). Secret-bearing members are masked: CreateChatToken's `responseElements.token` is recorded as `***` and the refused call records no `token` at all. Reference: https://docs.aws.amazon.com/ivs/latest/LowLatencyUserGuide/cloudtrail.html. Smithy: CloudTrail LookupEvents is the read path (LookupAttributes EventSource/EventName; modelled errors InvalidLookupAttributesException, InvalidNextTokenException, ThrottlingException, ...).
Emulator: the ivschat calls themselves are served (CreateRoom mints a well-formed `arn:aws:ivschat:<region>:<account>:room/<12 chars>`, the 180-minute CreateChatToken is issued, the 181-minute one is refused as ValidationException, UpdateRoom on a deleted room is refused as ResourceNotFoundException), but LookupEvents returns no record for any of them. The checks' audit polls — `LookupEvents(LookupAttributes=[{EventSource: ivschat.amazonaws.com}], StartTime=since)`, retried with a 900 s delivery budget — never see a single matching `requestID`, so each check runs to the harness cap and reports `Check timed out after 60.0s` instead of ever reaching its record assertions.
Why:      every audit- and compliance-shaped client is broken against this emulator: no IVS Chat call can be correlated to an audit record, a refused call's errorCode/errorMessage is unauditable, and the redaction guarantees (token recorded as `***`, no token on the refused call) cannot be verified at all. Because LookupEvents returns an empty page rather than an error, a client polling for delivery hangs on its own retry budget rather than failing fast.
Fix:      the CloudTrail event-history ingestion path for the ivschat service — ivschat's request-handling layer must emit a management-event record (eventSource, eventName, requestID, readOnly flag, requestParameters, and on refusal errorCode/errorMessage/responseElements with Code/Message/Details) into the store LookupEvents reads, with CreateChatToken's token masked to `***`.
Traces:   cloudtrail__ivschat.chat-token-session-over-180-minutes-is-rejected-and-audited, cloudtrail__ivschat.logging-configuration-journey-is-audited-with-its-destination, cloudtrail__ivschat.room-journey-is-audited-call-by-call-with-read-only-flags, cloudtrail__ivschat.update-of-a-deleted-room-is-rejected-and-audited-as-not-found.

Cause 2 of 2: ivschat SendEvent is a scaffolded stub that answers NotImplemented.
AWS:      Smithy: SendEvent (POST /SendEvent, success 200) takes required `roomIdentifier` (RoomIdentifier @length 1..128, @pattern ^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:room/[a-zA-Z0-9-]+$) and required `eventName` (@length 1..100), plus optional `attributes` map, and returns `id` (ID @length 12..12, @pattern ^[a-zA-Z0-9]+$). Its modelled errors are AccessDeniedException (403), PendingVerification (403), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400) — `NotImplemented` is not among them, and a well-formed SendEvent against a live room must return 200 with a 12-character `id`.
Emulator: every SendEvent call is refused with Error.Code `NotImplemented` and the emulator-authored message `operation SendEvent is scaffolded but not implemented in this emulator`, so the operation is unreachable.
Why:      a client cannot send a custom room event at all, and the refusal uses an error code the SDK's modelled error set for SendEvent does not contain, so retry/branch logic written against AWS's modelled errors cannot classify it. In the audit scenario it also means the moderation leg of the trail (SendEvent beside DisconnectUser/DeleteMessage) can never be exercised.
Fix:      the ivschat SendEvent handler — implement the operation against room state: validate `roomIdentifier` and `eventName` against their modelled constraints (ValidationException), answer ResourceNotFoundException for a room that does not exist, and otherwise return 200 with a freshly minted 12-character alphanumeric `id`.
Traces:   cloudtrail__ivschat.chat-token-and-moderation-calls-are-audited-without-their-secrets.

No open issue covers either cause. The open ivschat issues concern 204 tag-response framing (20261030T164443Z-...), CloudFormation resource registration (20261109T223435Z-...), the chat-messaging WebSocket endpoint (20261118T013251Z-...) and CreateChatToken's token encoding (20261124T110004Z-...). The open per-service CloudTrail audit gaps (lightsail 20261127T194606Z-..., ssm 20261127T125407Z-..., cognito-idp 20261122T054242Z-..., eventbridgev2 20261127T111942Z-...) each name their own service and none names ivschat.

## Evidence
Cause 1 — four checks hang on the CloudTrail audit poll and report `Check timed out after 60.0s`:
  cloudtrail__ivschat.chat-token-session-over-180-minutes-is-rejected-and-audited — reaches `_await(ct, since, {issued_rid, refused_rid})`, which repeats LookupEvents(LookupAttributes=[{EventSource: 'ivschat.amazonaws.com'}], StartTime=since) with a 900 s budget; 0 of 2 CreateChatToken records ever resolve. The preceding ivschat legs all succeed (180-minute token issued; 181 minutes refused ('ValidationException', 'CreateChatToken.sessionDurationInMinutes is required to have a maximum value of 180.0, found 181.0')), proving the hang is the audit read, not the ivschat call.
  cloudtrail__ivschat.room-journey-is-audited-call-by-call-with-read-only-flags — same poll shape, no records.
  cloudtrail__ivschat.logging-configuration-journey-is-audited-with-its-destination — same poll shape; never reaches its responseElements/state assertions.
  cloudtrail__ivschat.update-of-a-deleted-room-is-rejected-and-audited-as-not-found — UpdateRoom/DeleteRoom/refusal legs all behave (('ResourceNotFoundException', 'Resource: <room-arn> not found')), then `_until_audited(ct, since, {live_rid, refused_rid})` never resolves either requestID.
  Expected per record: eventSource 'ivschat.amazonaws.com', eventName of the call, readOnly false for writes, requestParameters as sent, errorCode/responseElements.{Code,Message,Details} on the refused call, responseElements.token == '***' on the issued token.
  Actual: LookupEvents returns no event whose CloudTrailEvent.requestID matches any of the calls' RequestIds.

Cause 2 — cloudtrail__ivschat.chat-token-and-moderation-calls-are-audited-without-their-secrets:
  Expected: SendEvent -> HTTP 200 with a 12-character alphanumeric `id` (modelled errors: AccessDeniedException, PendingVerification, ResourceNotFoundException, ThrottlingException, ValidationException).
  Actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendEvent operation: operation SendEvent is scaffolded but not implemented in this emulator

## Steps to Reproduce
Cause 1:
1. ivschat.CreateRoom(name=<unique>, tags={...}) -> note the returned room ARN and the call's RequestId.
2. ivschat.UpdateRoom(identifier=<arn>, maximumMessageRatePerSecond=7) and ivschat.CreateChatToken(roomIdentifier=<arn>, userId='audit-member', sessionDurationInMinutes=180) -> note each RequestId.
3. cloudtrail.LookupEvents(LookupAttributes=[{AttributeKey:'EventSource', AttributeValue:'ivschat.amazonaws.com'}], StartTime=<a few minutes ago>), paginated.
   Expected: one management-event record per call above, matched by CloudTrailEvent.requestID, carrying eventName, readOnly, requestParameters, and for refusals errorCode/errorMessage/responseElements; CreateChatToken's responseElements.token == '***'.
   Actual: no matching events are ever returned, so a delivery poll never terminates.
4. Repeat with a refused call (CreateChatToken sessionDurationInMinutes=181, or UpdateRoom on a deleted room) -> its record is likewise absent.

Cause 2:
5. ivschat.CreateRoom(...) then ivschat.SendEvent(roomIdentifier=<arn>, eventName='app.moderation', attributes={'k':'v'}).
   Expected: HTTP 200 with `id` of exactly 12 alphanumeric characters.
   Actual: ClientError (NotImplemented) 'operation SendEvent is scaffolded but not implemented in this emulator'.
