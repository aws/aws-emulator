Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__eventbridgev2__events.create-event-bus-for-a-taken-name-is-rejected-and-recorded-with-its-error-code cloudtrail__eventbridgev2__events.create-event-bus-is-recorded-under-the-event-source-of-the-api-that-served-it cloudtrail__eventbridgev2__events.tag-resource-history-records-carry-the-tag-pair-and-the-bus-arn
Issue-Key: cloudtrail__eventbridgev2__events.create-event-bus-for-a-taken-name-is-rejected-and-recorded-with-its-error-code|run#26|AssertionError|raise AssertionError(
Issue-Key: cloudtrail__eventbridgev2__events.create-event-bus-is-recorded-under-the-event-source-of-the-api-that-served-it|run#20|AssertionError|raise AssertionError(
Issue-Key: cloudtrail__eventbridgev2__events.tag-resource-history-records-carry-the-tag-pair-and-the-bus-arn|run#26|AssertionError|raise AssertionError(
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudtrail__eventbridgev2__events: no CloudTrail event-history record is written for EventBridge classic or v2 management calls, so LookupEvents returns nothing for CreateEventBus/TagResource (success or refusal)

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: EventBridge management calls are never auditable through CloudTrail event history. Failing checks: cloudtrail__eventbridgev2__events.create-event-bus-for-a-taken-name-is-rejected-and-recorded-with-its-error-code, cloudtrail__eventbridgev2__events.create-event-bus-is-recorded-under-the-event-source-of-the-api-that-served-it, cloudtrail__eventbridgev2__events.tag-resource-history-records-carry-the-tag-pair-and-the-bus-arn.

AWS: every EventBridge control-plane call — the classic bus API (eventSource `events.amazonaws.com`) and the v2 bus API (eventSource `eventsv2.amazonaws.com`) — is a management event recorded in CloudTrail event history and returned by `LookupEvents`, whether it succeeded or was refused. Each record's `CloudTrailEvent` JSON carries `eventSource` naming the API that served the call, `eventName` (`CreateEventBus`, `TagResource`), `requestParameters` echoing the request (the bus `name`, the tag pair, the bus ARN), and, for a refused call, `errorCode` plus `errorMessage` with `responseElements` null. AWS reference: CloudTrail record contents (https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html). The classic API records a refused duplicate create under `errorCode` `UnknownError` (recorded behavior only), while the v2 API records it under the modelled `ResourceAlreadyExistsException`; Smithy (eventbridgev2): CreateEventBus errors include ResourceAlreadyExistsException (409). The emulator's own refusals are correct — the suite graded both of them as expected — so only the audit record is missing.

Emulator: `LookupEvents(LookupAttributes=[{EventName: CreateEventBus}], StartTime=...)` paginated to exhaustion returns ZERO records for both event sources, over the full polling budget. The suite reports `after -2s event history held [], expected four records: ["('events.amazonaws.com', 'UnknownError')", "('events.amazonaws.com', None)", "('eventsv2.amazonaws.com', 'ResourceAlreadyExistsException')", "('eventsv2.amazonaws.com', None)"]`; `event history never recorded both CreateEventBus calls in 29s; sources recorded so far: []`; and `event history held TagResource records for [] after -2s, expected both ['events.amazonaws.com', 'eventsv2.amazonaws.com']`. The empty list is the whole divergence: no record is emitted at all, so no field of the record can be graded.

Why: a client that audits EventBridge — reconciling who created or tagged a bus, or proving a refused create was attempted and under which error code — gets an empty history and concludes the calls never happened. Compliance tooling, change-attribution and any rule or alarm keyed on `AWS API Call via CloudTrail` for EventBridge silently never fires, which is worse than an error because it reads as a clean account.

Fix: the audit-emission hook of the eventbridge (classic) and eventbridgev2 crates' control-plane handlers — each served call, including one that ends in a modelled error, must hand CloudTrail a management record carrying eventSource (`events.amazonaws.com` / `eventsv2.amazonaws.com`), eventName, requestParameters, and errorCode/errorMessage with null responseElements on refusal — plus CloudTrail's LookupEvents store that must index and return them.

Related but NOT covering this: 20261108T081736Z-autoscaling-cloudtrail-... (Auto Scaling), 20261122T054242Z-cloudtrail-cognito-idp-... (cognito-idp) and 20261127T024328Z-cloudtrail-dynamodb-events-sqs-... (DynamoDB) report the same shape of gap for other services; none names the events/eventsv2 event sources.

Traces: the three failing check ids listed above.

## Evidence
Expected (AWS): LookupEvents(EventName=CreateEventBus) returns four records keyed by (eventSource, errorCode): ('events.amazonaws.com', None), ('eventsv2.amazonaws.com', None), ('events.amazonaws.com', 'UnknownError') with an errorMessage containing 'unknown error' and responseElements null, and ('eventsv2.amazonaws.com', 'ResourceAlreadyExistsException') with an errorMessage containing 'already exists' and responseElements null. LookupEvents(EventName=TagResource) returns a record for each of ['events.amazonaws.com', 'eventsv2.amazonaws.com'] carrying the tag pair and the bus ARN in requestParameters.

Actual (emulator): both lookups return an empty Events list on every page for the entire polling budget.
  - create-event-bus-for-a-taken-name...: AssertionError: after -2s event history held [], expected four records: ["('events.amazonaws.com', 'UnknownError')", "('events.amazonaws.com', None)", "('eventsv2.amazonaws.com', 'ResourceAlreadyExistsException')", "('eventsv2.amazonaws.com', None)"]
  - create-event-bus-is-recorded-under-the-event-source...: AssertionError: event history never recorded both CreateEventBus calls in 29s; sources recorded so far: []
  - tag-resource-history-records-carry-the-tag-pair-and-the-bus-arn: AssertionError: event history held TagResource records for [] after -2s, expected both ['events.amazonaws.com', 'eventsv2.amazonaws.com']

The CreateEventBus refusals themselves matched AWS in the same checks (classic: ResourceAlreadyExistsException / 'Event bus <name> already exists.'; v2: ResourceAlreadyExistsException / 'Resource already exists'), which isolates the defect to audit emission rather than to the operations.

## Steps to Reproduce
1. Create a classic bus: events.CreateEventBus(Name=<name>, Tags=[{Key:sim2real,Value:dup}]).
2. Create a v2 bus of the same name: eventbridgev2.CreateEventBus(Name=<name>, Tags={sim2real:dup}); poll DescribeEventBus until State == ACTIVE.
3. Repeat both creates; both are refused (classic ResourceAlreadyExistsException, v2 ResourceAlreadyExistsException).
4. Poll cloudtrail.LookupEvents(LookupAttributes=[{EventName: CreateEventBus}], StartTime=<2 minutes ago>) across all pages and parse each CloudTrailEvent, keying records by (eventSource, errorCode) for those whose requestParameters.name == <name>.
   -> Observed: no records at all. Expected: the four records above.
5. Likewise TagResource on both a classic bus and a v2 bus, then poll LookupEvents(EventName=TagResource): observed no records for either events.amazonaws.com or eventsv2.amazonaws.com.
