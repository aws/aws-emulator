Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event
Issue-Key: pinpoint-sms-voice-v2|SendTextMessage|ValidationException
Issue-Commit: 7edee2d691bdf0e85063cccc4792b3c085a7344a

# SendTextMessage refuses every send that omits the optional OriginationIdentity (ValidationException instead of a 200 MessageId)

> _Note: this issue was identified against the code base at commit `7edee2d691bdf0e85063cccc4792b3c085a7344a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTextMessage

## Description
This issue carries 1 root cause (Cause 1 of 1). The suite lists the three grounding checks under the heading `logs__pinpoint-sms-voice-v2` (a cross-service check pairing CloudWatch Logs with End User Messaging SMS/Voice); the diverging operation and the defective handler both belong to pinpoint-sms-voice-v2, so the issue is filed against that service.

Cause 1 of 1: SendTextMessage treats the OPTIONAL input member OriginationIdentity as required and refuses the call.

AWS: Smithy (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/smithy-model/com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2.json) models SendTextMessageRequest with exactly one @required member, DestinationPhoneNumber (PhoneNumber, len 1..20, pattern ^\+?[1-9][0-9]{1,18}$), plus DryRun (PrimitiveBoolean, default false). OriginationIdentity (TextMessageOriginationIdentity, len 1..256, pattern ^[A-Za-z0-9_:/\+-]+$) is NOT @required, and its documentation describes only what it may name (PhoneNumber, PhoneNumberId, PhoneNumberArn, RcsAgentId, RcsAgentArn, SenderId, SenderIdArn, PoolId, PoolArn) — it never states that omitting it is invalid. Recorded behavior: a SendTextMessage carrying only DestinationPhoneNumber, MessageBody, MessageType and ConfigurationSetName (no OriginationIdentity) is ACCEPTED — HTTP 200 with a MessageId in the body — and the service resolves the account's own origination identity itself; the configuration set's event destination then logs the delivery events for that MessageId. All three grounding checks depend on that accepted send, both with DryRun=false and with DryRun=true.

Emulator: the request is refused with HTTP 400, error code ValidationException, Message "OriginationIdentity is required: this emulator has no account default origination identity to fall back to", Reason "MISSING_PARAMETER" and Fields [{Name: "OriginationIdentity", Message: "must name a leased phone number"}]. The whole body is emulator-authored: AWS does not refuse this request at all, so no status, code or message is correct here; and even as a field-validation refusal the text diverges from this service's template (fixed text "The following parameter is null, empty or invalid: " followed by a bracketed, single-quoted list of the offending members' lowerCamel WIRE names — 'originationIdentity', not 'OriginationIdentity' — with each Fields entry's Message naming the constraint the member must satisfy). The refusal is unconditional whenever the member is absent, so the entire text data plane is unreachable without an explicit origination identity.

Why: every client that sends to a destination using the account's default origination identity — the shape of call AWS accepts and of all three recorded checks, including sends to AWS's published success/failure simulator numbers — gets a non-retryable 400 instead of a MessageId. Nothing downstream can run: no MessageId is minted, no message is recorded, and no event is produced for a configuration set's CloudWatch Logs event destination, so delivery-event flows (TEXT_SUCCESSFUL / undelivered final events) and the DryRun-logs-nothing contract cannot be exercised at all. A caller cannot work around it by sending a value, because the emulator additionally requires the identity to resolve to a phone number it leases.

Fix: the origination-identity resolution arm of this operation's handler. An absent OriginationIdentity must fall back to an account default origination identity the service state provides (minting/holding one if the slice has none) and the send must proceed to the normal accepted path that mints a MessageId and records the send — not short-circuit into ValidationException. Only an origination identity that is PRESENT and unresolvable should be refused, and that refusal must use this service's modeled error rendering.

Traces: behavioral checks logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does, logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event, logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event. Code: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/send_text_message.rs:217-236 (the `let Some(origination_identity) = input.origination_identity else { ... ValidationException ... }` branch), with the follow-on leased-number check at :237-249. Related but NOT covering: open issue 20261110T085439Z-pinpoint-sms-voice-v2-error-responses-are-emulator.md describes emulator-authored error bodies for CreateConfigurationSet/DeleteConfigurationSet/PutOptedOutNumber; it does not cover SendTextMessage refusing a request AWS accepts. Note: once the send is accepted, these checks go on to assert the configuration set's event destination writes the delivery events to CloudWatch Logs; that stage was never reached in this replay, so it is not filed here.

## Evidence
Expected (recorded AWS, all three checks): SendTextMessage(DestinationPhoneNumber=<AWS simulator number in E.164>, MessageBody=<text>, MessageType="TRANSACTIONAL", ConfigurationSetName=<the check's configuration set>, DryRun=false|true) with NO OriginationIdentity -> HTTP 200, body {"MessageId": "<id>"}.

Actual (emulator): HTTP 400, awsJson1_0 error ValidationException —
  Message: "OriginationIdentity is required: this emulator has no account default origination identity to fall back to"
  Reason:  "MISSING_PARAMETER"
  Fields:  [{"Name": "OriginationIdentity", "Message": "must name a leased phone number"}]
botocore surfaces it as: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the SendTextMessage operation: OriginationIdentity is required: this emulator has no account default origination identity to fall back to

The three grounding checks fail at exactly this call, before any delivery-event assertion:
  - logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does (fails on the DryRun=false control send)
  - logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event
  - logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event

Model evidence: smithy_query(query="operation", operation="SendTextMessage") reports input {DestinationPhoneNumber: required true; OriginationIdentity: no required flag; DryRun: default false} — OriginationIdentity is optional, so no ValidationException is owed for its absence.

Code evidence: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/send_text_message.rs:220 returns ValidationException whenever input.origination_identity is None, with the comment "A send needs an origination identity: this slice leases phone numbers... Pools and sender ids are not held by this emulator" — i.e. the refusal is an emulator-state limitation, not the modeled contract.

## Steps to Reproduce
1. Boot the emulator and point a pinpoint-sms-voice-v2 (sms-voice) client at it.
2. sms.create_configuration_set(ConfigurationSetName="evsink") -> 200.
3. sms.send_text_message(DestinationPhoneNumber="+46790645100", MessageBody="Your test code is 8765", MessageType="TRANSACTIONAL", ConfigurationSetName="evsink", DryRun=False)  # no OriginationIdentity, exactly as AWS permits.
4. Observe HTTP 400 ValidationException with Message "OriginationIdentity is required: this emulator has no account default origination identity to fall back to" instead of HTTP 200 with a MessageId.
5. Repeat step 3 with DryRun=True: the same refusal, so the DryRun path is unreachable too.

Re-replay to confirm a fix: logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does, logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event, logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event.
