Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.lambda-target-non-json-input-rejected-while-sqs-target-delivers-it lambda__scheduler__sqs.schedule-without-input-invokes-lambda-with-default-event
Issue-Key: lambda__scheduler__sqs.lambda-target-non-json-input-rejected-while-sqs-target-delivers-it|_rejection#30|AssertionError|raise AssertionError(f"expected CreateSchedule to be refused, it returned {result!r}")
Issue-Key: lambda__scheduler__sqs.schedule-without-input-invokes-lambda-with-default-event|run#42|AssertionError|assert set(event) == {"version", "id", "detail-type", "source", "account", "time", "region",
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# lambda__scheduler__sqs: a non-JSON Input for a Lambda target is accepted, and a schedule with no Input invokes the function with an empty event instead of Scheduler's default event

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Two net-new causes on EventBridge Scheduler's Lambda target path.

Cause 1 of 2: Target.Input is not parsed as JSON when the target is a Lambda function, so plain text is accepted.
AWS: for a Lambda target the Input must be a JSON document, and CreateSchedule refuses a non-JSON Input with 400 ValidationException and the message "JSON syntax error in input for the target: [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]" — the fixed text is the whole sentence and bracketed detail; the line/column are the position in the submitted Input at which parsing failed. Nothing is stored, so the schedule group lists only its other schedules. The same Input wrapped as JSON (e.g. {"note": "<text>"}) is accepted and read back verbatim, and the SAME raw text IS legal for an SQS target, which delivers it unchanged.
Emulator: returns 200 with a ScheduleArn for the Lambda target carrying plain text and stores the schedule.
Why: a schedule is persisted whose payload Lambda cannot be invoked with; the client learns nothing at configure time, and the create-time distinction between a Lambda target (JSON required) and an SQS target (raw text allowed) is lost.
Fix: the target-specific Input validation in the scheduler crate's CreateSchedule/UpdateSchedule handler.
Traces: lambda__scheduler__sqs.lambda-target-non-json-input-rejected-while-sqs-target-delivers-it

Cause 2 of 2: a schedule whose Target carries no Input invokes the Lambda function with an empty event instead of Scheduler's default event.
AWS: when Target.Input is absent, the occurrence invokes the function with Scheduler's default event — a JSON object whose members are exactly version, id, detail-type, source, account, time, region, resources, detail. Its fixed parts are version = "0", detail-type = "Scheduled Event", source = "aws.scheduler" and detail = "{}"; its variable parts come from the schedule and the occurrence (resources = [the schedule's own ARN]; account and region = the schedule's account and Region; id = a fresh UUID per occurrence; time = the nominal fire instant).
Emulator: the function receives an event with no members at all — the handler that relays its event reported the key set as [].
Why: a function written against the documented Scheduled Event shape cannot read the schedule ARN, the fire time or the event id, so every no-Input Lambda schedule is unusable.
Fix: the occurrence-delivery path in the scheduler crate — the default-event builder used when Target.Input is absent.
Traces: lambda__scheduler__sqs.schedule-without-input-invokes-lambda-with-default-event

## Evidence
Cause 1 — lambda__scheduler__sqs.lambda-target-non-json-input-rejected-while-sqs-target-delivers-it:
  Target{Arn=<lambda function ARN>, RoleArn=<role granting lambda:InvokeFunction>, Input=<plain text, not JSON>}
  expected ValidationException "JSON syntax error in input for the target: [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>"}
  (control: the same plain text as an SQS target's Input is accepted and delivered verbatim)

Cause 2 — lambda__scheduler__sqs.schedule-without-input-invokes-lambda-with-default-event:
  expected the invoked event's key set to be {version, id, detail-type, source, account, time, region, resources, detail}
  got      "default event keys were []"

## Steps to Reproduce
1. Boot the emulator. Create a Lambda function that relays the event it is given to an SQS queue, an SQS queue, and two scheduler.amazonaws.com roles (one granting lambda:InvokeFunction on the function, one granting sqs:SendMessage on the queue). Create a schedule group.
2. Cause 1: CreateSchedule with Target{Arn=<function ARN>, RoleArn=<lambda role>, Input="some plain text"} — expect ValidationException "JSON syntax error in input for the target: …", observe 200. For contrast, the same text as an SQS target's Input is accepted and delivered unchanged.
3. Cause 2: CreateSchedule with Target{Arn=<function ARN>, RoleArn=<lambda role>} and NO Input, ScheduleExpression at(<now+1m>). Read the relayed event from the queue — expect the nine default-event members, observe an empty object.
