Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__events__s3.put-events-is-logged-as-a-data-event-only-for-the-bus-a-selector-names
Issue-Key: cloudtrail|PutEventSelectors|InvalidEventSelectorsException
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudtrail__events__s3: PutEventSelectors refuses an EventBridge event-bus data-event selector — "The resources.type field value is not valid."

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventSelectors

## Description
Cause 1 of 1: an advanced/data event selector whose resources.type names the EventBridge event-bus resource type is rejected, so PutEvents can never be logged as a data event for the bus a selector names.

AWS: CloudTrail accepts AWS::Events::EventBus as a data-event resource type: a selector with DataResources [{Type: 'AWS::Events::EventBus', Values: [<bus arn>]}] (or the equivalent advanced selector field resources.type) is stored, GetEventSelectors reads it back, and events:PutEvents on that bus is then logged as a data event while PutEvents on other buses is not. Recorded behavior for the accepted type list; Smithy (cloudtrail): PutEventSelectors models InvalidEventSelectorsException for a selector it rejects — the defect is that a VALID type is placed in that class.
Emulator: the call is refused with HTTP 400 `InvalidEventSelectorsException: The resources.type field value is not valid.` (the message's fixed text is AWS's wording for a genuinely unsupported type; the emulator applies it to a supported one), so the trail can never be configured and the data-event behavior cannot be exercised at all.
Why: a client cannot turn on EventBridge data-event logging; anything that audits per-bus PutEvents activity (compliance trails, selector-scoped detection) cannot be built, and the refusal misleads the caller into thinking its type string is wrong.
Fix: the allowed resources.type / DataResources.Type catalogue used by the cloudtrail PutEventSelectors handler's validation (it must include AWS::Events::EventBus), and the data-event recording path for events PutEvents.
Traces: cloudtrail__events__s3.put-events-is-logged-as-a-data-event-only-for-the-bus-a-selector-names.

## Evidence
Expected: PutEventSelectors accepts the AWS::Events::EventBus data resource and GetEventSelectors reads it back; PutEvents on the named bus is then logged as a data event.
Actual: `botocore.errorfactory.InvalidEventSelectorsException: An error occurred (InvalidEventSelectorsException) when calling the PutEventSelectors operation: The resources.type field value is not valid.`

## Steps to Reproduce
1. CreateTrail with an S3 bucket destination.
2. PutEventSelectors with EventSelectors [{ReadWriteType: All, DataResources: [{Type: 'AWS::Events::EventBus', Values: ['arn:aws:events:<region>:<acct>:event-bus/<bus>']}]}].
3. Observe the InvalidEventSelectorsException refusal.
