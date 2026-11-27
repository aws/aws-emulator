Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-rotation-schedule-expression-grammar-rejected
Issue-Step: teardown#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-rotation-schedule-expression-grammar-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups
Issue-Commit: 34735640b265050797646a7031263995ba2fdc83

# logs: the rotation Lambda's log group is never created, so DescribeLogGroups returns an empty list

> _Note: this issue was identified against the code base at commit `34735640b265050797646a7031263995ba2fdc83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
1 net-new cause.

AWS: recorded behavior only — the Smithy model for CloudWatch Logs DescribeLogGroups says nothing about who creates a log group. The recording shows that after a Lambda function has been created and used as a Secrets Manager rotation function, the function's log group (the `/aws/lambda/<function-name>` group) EXISTS, so DescribeLogGroups filtered to that prefix answers HTTP 200 with exactly one entry in $.logGroups. The replay step polls for that one entry.
Emulator: HTTP 200 with $.logGroups == [] — still empty after 12 poll attempts, i.e. the group is never created at all, not merely created late.
Why: a client that discovers a function's log group by DescribeLogGroups (to tail it, to set a retention policy, to attach a subscription filter, or to delete it on teardown) finds nothing and concludes the function never ran or never existed. The follow-on teardown DeleteLogGroup in the same fixture has nothing to delete. Any workflow that reads a Lambda's logs through the Logs API cannot be exercised against the emulator.
Fix: the component that provisions a Lambda function's log group — Lambda function creation/invocation must register the `/aws/lambda/<function-name>` group in the CloudWatch Logs state so the Logs read path (DescribeLogGroups) serves it.
Traces: secretsmanager-rotation-schedule-expression-grammar-rejected#teardown#13.

Related open issues: 20261024T150703Z-lambda-logs-secretsmanager-a-secrets-manager-rotation-never (the rotation Lambda is never executed) and 20261124T202401Z-lambda-logs-secretsmanager-a-rotation-whose-testsecret-step--r2. Neither carries this replay key and neither is about the log group's existence in the Logs API, so this cause is filed here. The sibling DeleteLogGroup divergence in the same fixture (teardown#14) is already owned by 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md and is NOT re-filed.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up at the admin health endpoint and the data listener; 45/484 steps mismatched).

Divergence cluster, service=logs, operation=DescribeLogGroups, verdict=mismatch, status 200 -> 200:
  $.logGroups: poll: 0 item(s), not 1 after 12 attempt(s) (expected None, got [])

fixture_id: secretsmanager-rotation-schedule-expression-grammar-rejected
step_key: teardown#13

The step is a poll with 12 attempts, so this is not a timing flake in the runner: the emulator returned an empty $.logGroups on every attempt. The emulator serves both the Lambda and the CloudWatch Logs surfaces in this run (the preceding steps of the fixture created the rotation function successfully), so the missing log group is the emulator's own state, not a runner or network failure. The very next step (teardown#14, DeleteLogGroup) then answers 400 where the recording has 200 — consistent with the group not existing — and that step is already owned by an open issue.

## Steps to Reproduce
1. git clone the repo; boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 corpus.
3. Replay fixture secretsmanager-rotation-schedule-expression-grammar-rejected through its teardown. Its earlier steps create a Lambda function and wire it as a Secrets Manager rotation function.
4. At step teardown#13, call CloudWatch Logs DescribeLogGroups with the logGroupNamePrefix for that function's `/aws/lambda/<function-name>` group.
5. Expected: 200 with exactly one entry in $.logGroups. Actual: 200 with $.logGroups == [], unchanged across all 12 poll attempts.
