Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.glue-job-failed-event-missing-target-permission-negative
Issue-Key: athena__glue__s3.glue-job-failed-event-missing-target-permission-negative|run#23|Timeout|resp = sqs.receive_message(QueueUrl=denied_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# athena__glue__s3: a Glue job run never reaches a terminal FAILED state, so no "Glue Job State Change" event is ever emitted and the EventBridge target-permission negative reaches no verdict

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: a started Glue job run never settles into a terminal state, so the Glue Job State Change event the rule under test matches is never published and the check exhausts the harness's 60 s budget before it can compare the permitted and unpermitted SQS targets.

AWS: glue:StartJobRun starts a run of a pythonshell job; the run executes the script and settles in a terminal JobRunState (here FAILED, because the script calls sys.exit(1)), and GetJobRun reports that state. Reaching FAILED makes Glue publish an event with source "aws.glue", detail-type "Glue Job State Change" and detail {jobName, state: "FAILED"}, which EventBridge delivers to each target whose resource policy admits events.amazonaws.com and silently drops for the target that does not. Smithy: glue#StartJobRun returns StartJobRunResponse{JobRunId}; glue#GetJobRun returns JobRun whose JobRunState enum includes SUCCEEDED/FAILED/TIMEOUT/ERROR/STOPPED. The event envelope itself is recorded behavior only.
Emulator: the check runs out of its whole budget at the point where it waits for the run to become terminal ("Error: Check timed out after 60.0s"), so neither the FAILED run, the event, nor the two-target delivery contrast is observable.
Why: a client that gates downstream work on a Glue job's terminal state, or that routes Glue Job State Change events to a queue, sees nothing at all: the job neither succeeds nor fails, so retries, alarms and event-driven follow-on steps never fire, and an event-permission bug in the client's own queue policy is indistinguishable from this.
Fix: the Glue job-run executor and its run-state machine (StartJobRun/GetJobRun state transitions), plus the Glue→EventBridge "Glue Job State Change" event emission on a terminal run.
Traces: athena__glue__s3.glue-job-failed-event-missing-target-permission-negative.

Several open issues record Glue job runs not executing in other service groups (for example 20261102T114851Z-glue-s3-a-started-glue-job-run-never.md and 20261102T091623Z-glue-job-runs-never-execute-every-run-ends.md); none of them carries this check's replay key.

## Evidence
athena__glue__s3.glue-job-failed-event-missing-target-permission-negative — Error: Check timed out after 60.0s.
Expected per AWS: GetJobRun reports JobRunState "FAILED" for the run of a pythonshell job whose script exits non-zero; a rule on {source: ["aws.glue"], detail-type: ["Glue Job State Change"], detail: {jobName: [<job>], state: ["FAILED"]}} then delivers one envelope to the queue whose policy grants events.amazonaws.com sqs:SendMessage, and nothing to the queue without that policy.
Actual: the run never becomes terminal within the budget, so the assertion `state == "FAILED"` is never reached and no event is produced for either queue.

## Steps to Reproduce
1. Create a bucket and an IAM role glue.amazonaws.com can assume, and create a pythonshell Glue job whose script is `import sys\nsys.exit(1)`.
2. Create two SQS queues; give only one of them a policy allowing events.amazonaws.com sqs:SendMessage with aws:SourceArn equal to the rule ARN.
3. PutRule on {"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":[<job>],"state":["FAILED"]}} and PutTargets with both queues.
4. StartJobRun and poll GetJobRun: on AWS the run settles FAILED and the permitted queue receives one envelope; on the emulator the run never settles and nothing is delivered.
