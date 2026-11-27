Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sns.scheduler-sns-target-without-input-publishes-the-default-payload scheduler__sns.scheduler-sns-topic-policy-deny-withholds-a-permitted-schedule-role scheduler__sns.scheduler-universal-sns-tag-resource-target-denied-without-that-action
Issue-Key: scheduler__sns.scheduler-sns-target-without-input-publishes-the-default-payload|run#17|KeyError|received.setdefault(json.loads(message["Body"])["id"], message)
Issue-Key: scheduler__sns.scheduler-sns-topic-policy-deny-withholds-a-permitted-schedule-role|run#24|AssertionError|raise AssertionError(
Issue-Key: scheduler__sns.scheduler-universal-sns-tag-resource-target-denied-without-that-action|run#24|AssertionError|assert code == "AuthorizationError", (
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# scheduler__sns: an SNS target with no Input publishes a payload that is not Scheduler's default event, the topic policy's Deny is not evaluated, and a universal sns:tagResource denial is dead-lettered as InternalFailure

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Three net-new causes on EventBridge Scheduler's SNS target path.

Cause 1 of 3: a schedule whose Target carries no Input publishes a body that is not Scheduler's default event.
AWS: when Target.Input is absent, each occurrence publishes Scheduler's default event — a JSON object with exactly the members version, id, detail-type, source, account, time, region, resources, detail. The fixed parts are version = "0", detail-type = "Scheduled Event", source = "aws.scheduler" and detail = "{}"; the variable parts come from the schedule and the occurrence: resources = [the schedule's own ARN], account and region = the schedule's account and Region, id = a fresh UUID per occurrence (the same id on a redelivery of one occurrence, a different id per occurrence), time = the nominal fire instant formatted "%Y-%m-%dT%H:%M:%SZ". GetSchedule continues to report no Input.
Emulator: the published body has no "id" member at all (the check raises KeyError 'id' reading it), so the default event is not what is published.
Why: every consumer of a no-Input schedule — the documented way to get a Scheduled Event — receives a payload it cannot parse, and cannot deduplicate redeliveries by event id or read the fire time.
Fix: the occurrence-delivery path in the scheduler crate — the default-event builder used when Target.Input is absent.
Traces: scheduler__sns.scheduler-sns-target-without-input-publishes-the-default-payload

Cause 2 of 3: an SNS TOPIC POLICY's explicit Deny on sns:Publish is not evaluated for a schedule's execution role, so the occurrence neither delivers nor dead-letters.
AWS: the topic's resource policy is evaluated alongside the role's identity policy. A role that holds sns:Publish in its identity policy but is denied by the topic policy cannot publish: the occurrence fails and, with a Target.DeadLetterConfig.Arn set and MaximumRetryAttempts 0, Scheduler writes a dead-letter event to that queue carrying MessageAttribute ERROR_CODE = "AuthorizationError". Removing only the Deny statement makes the identical schedule deliver.
Emulator: within 240 s the barred schedule dead-lettered 0 events (while the twin role's schedule delivered), so the topic policy is not consulted and no failure is recorded at all.
Why: a client cannot observe or diagnose a topic-policy denial — no delivery, no dead letter, no error. (The open issue 20261111T160016Z-scheduler-sns-an-sns-target-occurrence-whose-target.md covers the IDENTITY-policy variant — a Target.RoleArn lacking sns:Publish; this cause is the topic's own resource policy and is not covered by it.)
Fix: the SNS-target invoker in the scheduler crate — evaluate the topic resource policy and emit the dead-letter event on denial.
Traces: scheduler__sns.scheduler-sns-topic-policy-deny-withholds-a-permitted-schedule-role

Cause 3 of 3: a universal sns:tagResource target whose execution role may only publish is dead-lettered with the wrong ERROR_CODE.
AWS: the universal target's AWS-SDK call is made under the execution role, so a role without sns:TagResource is refused by SNS and the dead-letter event's ERROR_CODE MessageAttribute is SNS's own error code for an authorization failure, "AuthorizationError". The topic must not end up carrying the tag.
Emulator: dead-letters the occurrence with ERROR_CODE = "InternalFailure" (the attribute set served is ERROR_CODE, ERROR_MESSAGE, EXECUTION_ID, EXHAUSTED_RETRY_CONDITION, RETRY_ATTEMPTS, SCHEDULED_TIME, SCHEDULE_ARN, TARGET_ARN).
Why: an authorization failure is reported as a service fault, so a client retries or escalates instead of fixing the role's policy, and alarms keyed on InternalFailure fire for a permissions problem.
Fix: the universal-target invoker's error mapping — relay the target service's error code rather than defaulting to InternalFailure.
Traces: scheduler__sns.scheduler-universal-sns-tag-resource-target-denied-without-that-action

## Evidence
Cause 1 — scheduler__sns.scheduler-sns-target-without-input-publishes-the-default-payload:
  expected a published body with keys {version, id, detail-type, source, account, time, region, resources, detail}, version "0", detail-type "Scheduled Event", source "aws.scheduler", detail "{}", resources [<schedule ARN>], id a UUID
  got      a body with no "id" member — json.loads(body)["id"] raises KeyError: 'id'

Cause 2 — scheduler__sns.scheduler-sns-topic-policy-deny-withholds-a-permitted-schedule-role:
  topic policy: Allow sns:Publish to the account, plus an explicit Deny on sns:Publish for the barred role
  expected the barred schedule to dead-letter one event with MessageAttribute ERROR_CODE = "AuthorizationError", and its payload never to reach the subscriber
  got      "within 240s the twin role delivered 4 message(s) and the barred schedule dead-lettered 0 event(s)"

Cause 3 — scheduler__sns.scheduler-universal-sns-tag-resource-target-denied-without-that-action:
  Target.Arn = arn:aws:scheduler:::aws-sdk:sns:tagResource, role granted sns:Publish only
  expected dead-letter MessageAttribute ERROR_CODE = "AuthorizationError"
  got      ERROR_CODE = "InternalFailure"

## Steps to Reproduce
1. Boot the emulator. Create an SNS topic, subscribe an SQS inbox queue to it, and create a second SQS queue as the dead-letter queue.
2. Cause 1: create an execution role granting sns:Publish on the topic and sqs:SendMessage on the DLQ. CreateSchedule with ScheduleExpression rate(1 minute), Target{Arn=<topic ARN>, RoleArn=<role>, DeadLetterConfig.Arn=<dlq ARN>, RetryPolicy.MaximumRetryAttempts=0} and NO Input. Receive from the inbox queue and parse the body — expect the nine default-event members, observe no "id".
3. Cause 2: create two roles both granting sns:Publish on the topic; set the topic Policy to Allow the account plus an explicit Deny on sns:Publish for the second role's ARN. Create one schedule per role, both with the DLQ and MaximumRetryAttempts 0. Expect the denied schedule to dead-letter with ERROR_CODE "AuthorizationError"; observe 0 dead-letter events.
4. Cause 3: create a role granting only sns:Publish (plus sqs:SendMessage on the DLQ). CreateSchedule with Target.Arn = "arn:aws:scheduler:::aws-sdk:sns:tagResource" and Input = {"ResourceArn": "<topic ARN>", "Tags": [{"Key": "k", "Value": "v"}]}. Read the dead-letter event's ERROR_CODE — expect "AuthorizationError", observe "InternalFailure".
