Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event
Issue-Key: logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does|_final_event#8|AssertionError|raise AssertionError(
Issue-Key: logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event|_final_event#8|AssertionError|raise AssertionError(
Issue-Key: logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event|_final_event#8|AssertionError|raise AssertionError(
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# logs__pinpoint-sms-voice-v2: a configuration set's CloudWatch Logs event destination never receives any message event, so no SendTextMessage outcome is observable

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTextMessage

## Description
One root cause, grounded in 3 failing checks of this service heading.

Cause 1 of 1: SendTextMessage accepts a send and mints a MessageId, but the configuration set's CreateEventDestination CloudWatchLogsDestination never has a single event record written to its log group — not an interim event and not the final one — so the message's outcome is unobservable.
Failing checks: logs__pinpoint-sms-voice-v2.dry-run-text-message-logs-no-event-while-a-real-send-does, logs__pinpoint-sms-voice-v2.text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event, logs__pinpoint-sms-voice-v2.text-to-success-simulator-number-logs-a-final-successful-event.

AWS: Smithy model (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/smithy-model/com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2.json). `CreateEventDestination` takes a required `ConfigurationSetName`, a required `EventDestinationName`, a required `MatchingEventTypes` (EventTypeList, 1..52) and a `CloudWatchLogsDestination` whose `IamRoleArn` and `LogGroupArn` are both required — "Contains the destination configuration to use when publishing message sending events"; `IamRoleArn` is "an Identity and Access Management role that is able to write event data to an Amazon CloudWatch destination" and `LogGroupArn` names "the Amazon CloudWatch log group that you want to record events in". The `EventType` enum includes `ALL`, `TEXT_SENT`, `TEXT_PENDING`, `TEXT_SUCCESSFUL`, `TEXT_DELIVERED`, `TEXT_BLOCKED` and the rest of the TEXT_* family, i.e. the service's contract is that matching events for a send are published to that destination. `SendTextMessage` returns a `MessageId` and, per https://docs.aws.amazon.com/sms-voice/latest/APIReference/API_SendTextMessage.html, `DryRun` (PrimitiveBoolean, default false) validates the request without sending. Against real AWS the recordings show: a send to the AWS-published success simulator number ends with a record carrying `isFinal: true` and `eventType: "TEXT_SUCCESSFUL"`; a send to the failure simulator number ends with `isFinal: true`, `eventType: "TEXT_BLOCKED"`, `messageStatus: "BLOCKED"`, `messageStatusDescription: "Phone has blocked SMS"` (that description is AWS's fixed text for that status) and echoes the request's `destinationPhoneNumber` and `Context` map back as `context`; and a `DryRun=true` send is answered with a MessageId of its own but logs no event at all. Each record is a JSON document naming the send's `messageId`, so it is retrievable with FilterLogEvents and the filter pattern `{ $.messageId = "<id>" }`. The variable parts of a record (messageId, destinationPhoneNumber, context, timestamps) come from the request and the emulator's state; the fixed parts are the eventType/messageStatus/messageStatusDescription vocabulary and the `isFinal` marker.

Emulator: SendTextMessage returns 200 with a non-empty MessageId (that part is right), but the log group named by the event destination stays empty forever. All three checks poll FilterLogEvents on `{ $.messageId = "<id>" }` until a record with `isFinal: true` appears and time out with the empty event list: `AssertionError: no final event for the message within 40s; event types logged: []`. Nothing diverges in the SendTextMessage response itself; the divergence is that no event is ever published to the CloudWatch Logs destination, for a successful send, for a blocked send, or at all. The DryRun check cannot even establish its control leg: it needs the real send's final event in order to measure the latency that sizes its absence window, so the "DryRun logs nothing" assertion is never reached.

Why: a client relying on AWS's behaviour has no way to learn what happened to a message. SendTextMessage is asynchronous — the MessageId alone says only that the request was accepted — and the event destination is the service's only delivery-outcome channel. An application that waits for TEXT_SUCCESSFUL/TEXT_DELIVERED before marking a notification sent waits forever; one that watches for TEXT_BLOCKED/TEXT_INVALID to suppress a bad number never sees a failure and keeps retrying; a dashboard or alarm built on a metric filter over that log group reads zero for every send. CreateEventDestination looks accepted, so the misconfiguration is invisible: the resource exists and the group is silent.

Fix: the SendTextMessage send pipeline's event publication in services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/send_text_message.rs together with the configuration-set/event-destination state it must consult (the stored EventDestination records created by src/ops/create_event_destination.rs). A send must resolve its ConfigurationSetName's event destinations, select those whose MatchingEventTypes cover the event being emitted (including `ALL`/`TEXT_ALL`), and write one JSON record per event to the CloudWatchLogsDestination's LogGroupArn — with the destination-number-dependent terminal event (TEXT_SUCCESSFUL for the success simulator, TEXT_BLOCKED/messageStatus BLOCKED/"Phone has blocked SMS" for the failure simulator), `isFinal` set on the terminal record, and the request's Context echoed as `context`. A `DryRun=true` send must mint a MessageId and publish nothing.

No already-open issue covers this: the open pinpoint-sms-voice-v2 issues concern read-after-delete/disable state, DestinationPhoneNumber refusal bodies, tag mutations, DeleteProtectConfiguration and MessageId minting — none of them the event-destination delivery path.

Traces: the three failing checks above; each fails in its `_final_event` helper after polling FilterLogEvents for a record with `isFinal: true`.

## Evidence
Expected (AWS): after CreateEventDestination(ConfigurationSetName=<set>, EventDestinationName="all-to-logs", MatchingEventTypes=["ALL"], CloudWatchLogsDestination={IamRoleArn, LogGroupArn}) and a SendTextMessage to that configuration set, FilterLogEvents on the destination log group with filterPattern `{ $.messageId = "<MessageId>" }` returns at least one JSON record, the last of which has isFinal=true and:
  - success simulator destination: eventType="TEXT_SUCCESSFUL"
  - failure simulator destination: eventType="TEXT_BLOCKED", messageStatus="BLOCKED", messageStatusDescription="Phone has blocked SMS", destinationPhoneNumber echoing the request, context echoing the request's Context map
  - DryRun=true send: a MessageId distinct from the real send's, and NO record at all for it

Actual (emulator): SendTextMessage returns 200 with a MessageId, and the destination log group never receives any record. All three checks time out polling:
  dry-run-text-message-logs-no-event-while-a-real-send-does — AssertionError: no final event for the message within 40s; event types logged: []
  text-to-failure-simulator-number-is-accepted-then-logs-an-undelivered-final-event — AssertionError: no final event for the message within 40s; event types logged: []
  text-to-success-simulator-number-logs-a-final-successful-event — AssertionError: no final event for the message within 40s; event types logged: []
The empty `[]` event-type list is the divergence: zero events published, not a wrong event. No status code or error code diverges; the 40 s figure is the run's remaining budget, not a requirement.

## Steps to Reproduce
1. logs.CreateLogGroup(logGroupName="/sim2real/sms-events/<name>").
2. pinpoint-sms-voice-v2.CreateConfigurationSet(ConfigurationSetName=<name>); iam.CreateRole trusting sms-voice.amazonaws.com and granting logs:CreateLogStream / logs:DescribeLogStreams / logs:PutLogEvents on the group.
3. pinpoint-sms-voice-v2.CreateEventDestination(ConfigurationSetName=<name>, EventDestinationName="all-to-logs", MatchingEventTypes=["ALL"], CloudWatchLogsDestination={IamRoleArn=<role arn>, LogGroupArn=<group arn>}).
4. pinpoint-sms-voice-v2.SendTextMessage(DestinationPhoneNumber=<AWS success simulator number>, MessageBody=..., MessageType="TRANSACTIONAL", ConfigurationSetName=<name>) and keep the returned MessageId.
5. Poll logs.FilterLogEvents(logGroupName=<group>, filterPattern='{ $.messageId = "<MessageId>" }'). Observe it returns no events at all, so no record with isFinal=true and eventType="TEXT_SUCCESSFUL" ever appears. Repeat with the failure simulator number (expect TEXT_BLOCKED/BLOCKED/"Phone has blocked SMS") and with DryRun=true (expect a MessageId and no records).
6. Re-replay the three checks listed in trace_ids to confirm the fix.
