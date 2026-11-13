Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq
Issue-Key: ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq|_agent_view#7|Timeout|return None
Issue-Commit: b3586cbe985cc2412e94e4f6c5b37df3b09bc671

# ec2__s3__sqs__ssm: the SQS-race worker scenario never reaches a verdict — a launched instance never runs its SSM job, so a poisoned job's 'no result object, message lands in the DLQ' contract is unobservable

> _Note: this issue was identified against the code base at commit `b3586cbe985cc2412e94e4f6c5b37df3b09bc671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause (Cause 1 of 1).

Already filed and NOT attributed to this failure by the replay report: 20261111T040849Z-ec2-s3-sqs-ssm-a-launched-ec2-instance-never-comes-online-in.md (open) carries the five sibling checks of this same family (s3pointer-guest..., sqsfail-guest-job-failure..., sqsjob-poisoned-job..., sqsjob-worker-drains-queue..., sqsrace-two-workers-one-queue...), all of which the report marks OWNED by it; this sixth check of the identical shape is marked neither OWNED nor ALREADY FILED, so it is filed here. A fixer should close it with that issue.

Cause 1 of 1: an instance-hosted SQS worker never executes, so neither the negative (no partial result object) nor the redrive (message reaches the dead-letter queue) half of the poisoned-job contract can be observed. Failing check: ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq.
AWS: recorded behavior only — EC2/SSM/S3/SQS model none of this end-to-end wiring. The behavior the check grades: an instance launched from the AL2023 AMI named by the SSM public parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 with the AmazonSSMManagedInstanceCore role comes online as an SSM-managed instance, so SendCommand reaches it and the command settles in a terminal status (Success/Failed/Cancelled/TimedOut); the worker it runs receives the queued job, and for a poisoned job writes NO result object to S3 and does not delete the message, so after the queue's maxReceiveCount is exhausted the message is redriven to the configured dead-letter queue.
Emulator: the check is cut off at its 600 s budget (`Error: Check timed out after 600.0s`) without reaching a verdict. Hypothesis (consistent with the five sibling checks' filed finding): the launched instance never becomes SSM-managed and the command never settles, so nothing the worker would do — writing or withholding a result object, deleting or abandoning the message, the redrive to the DLQ — ever happens.
Why: a guest workload driven through SSM over SQS/S3 is the ordinary 'EC2 worker' shape; with the instance never coming online the whole scenario silently produces nothing, and the negative guarantees it is supposed to prove (no partial result object, poison messages ending up on the DLQ rather than being consumed) cannot be relied on at all.
Fix: the EC2 instance lifecycle + SSM agent/command execution path in the ec2 and ssm service crates — a RunInstances instance with an SSM-capable instance profile must become visible as an SSM-managed instance and its SendCommand invocations must actually execute and settle, after which the worker's S3 writes and SQS receive/delete behavior (and the queue's redrive to its DLQ) follow.
Traces: the check above.

## Evidence
Expected (AWS): the launched instance becomes SSM-managed, SendCommand settles in a terminal status, the poisoned job leaves NO result object under the output prefix (HeadObject 404), the message is not deleted, and after maxReceiveCount receives it appears on the dead-letter queue.
Actual (emulator):
  [mismatch] ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq
  Error: Check timed out after 600.0s
The five sibling checks in the same service (same launch + SSM pattern) fail identically and are already filed as 20261111T040849Z-ec2-s3-sqs-ssm-a-launched-ec2-instance-never-comes-online-in.md. Generated VPC/subnet/bucket/queue/instance names and ids are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create a VPC 10.82.0.0/16 with subnet 10.82.10.0/24; resolve the AMI from ssm.get_parameter(Name='/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64'); create an instance profile with arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore and run a t3.micro instance with it.
2. Poll ssm.describe_instance_information until the instance is listed (it never is) — then ssm.send_command to install/run a worker that polls an SQS queue (with a redrive policy to a DLQ) and writes one result object per job to S3.
3. Send a poisoned job (one whose input object does not exist) and wait: expect no result object under the output prefix, the message not deleted, and after maxReceiveCount receives the message on the DLQ. Observed: the command never settles, nothing is written and nothing is redriven; the check is cut off at 600 s.
Replay the check id in trace_ids to confirm a fix.
