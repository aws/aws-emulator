Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline__logs.queue-read-role-credentials-put-log-events-rejected deadline__logs.queue-read-role-credentials-read-queue-log-events
Issue-Key: deadline__logs.queue-read-role-credentials-put-log-events-rejected|_poll#15|Timeout|time.sleep(2)
Issue-Key: deadline__logs.queue-read-role-credentials-read-queue-log-events|_poll#13|Timeout|time.sleep(2)
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# deadline__logs: CreateQueue never creates the queue's CloudWatch Logs log group /aws/deadline/<farmId>/<queueId>, so both queue-log checks poll it to their deadline

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
Cause 1 of 1: a queue created in the deadline emulator has no log group in CloudWatch Logs, so every cross-service path that reads or writes a queue's session logs stalls.

AWS: Deadline Cloud owns the log group a queue's sessions write to and creates it with the queue — it is named /aws/deadline/<farmId>/<queueId>, and the same name is what the service hands back as jobDetails.logGroupName (BatchGetJobEntity) and as assignedSessions[*].logConfiguration.options.logGroupName (UpdateWorkerSchedule), which this emulator already reports. Recorded behavior: shortly after CreateQueue(farmId, displayName, roleArn), DescribeLogGroups(logGroupNamePrefix='/aws/deadline/<farmId>/<queueId>') lists exactly that one group, and CreateLogStream/PutLogEvents/GetLogEvents against it succeed. The Smithy model for CreateQueue says nothing about log groups (recorded behavior only for the side effect itself).
Emulator: the group never appears. services/service-com-amazonaws-deadline-deadline/src/ops/create_queue.rs inserts the Queue into the farm's map and returns the minted queueId; it performs no CloudWatch Logs side effect at all, and nothing else in the create path does. Both checks that depend on the group begin by polling DescribeLogGroups for it with a 120 s budget, which outlives the suite's 60 s per-check limit, so each is cut off with "Check timed out after 60.0s" having asserted nothing further. Hypothesis: that never-satisfied poll is where the whole budget goes — it is the first blocking wait in both checks, and the source shows the group it waits for is never created. (The AssumeQueueRoleForRead stub reported for the third check in this group is a separate, already-filed cause; these two never reach it.)
Why: a client cannot read a job's logs at all — the documented path (resolve the queue's log group from the service, create/describe streams in it, read events) fails at the first step with ResourceNotFoundException from CloudWatch Logs, and the emulator meanwhile advertises that group name in job details and session log configuration, so the two answers contradict each other.
Fix: the CreateQueue handler's cross-service side effect (src/ops/create_queue.rs) — create /aws/deadline/<farmId>/<queueId> in the CloudWatch Logs emulator's state when the queue is created (and remove it on DeleteQueue), so the group the deadline service already names in its responses exists.

## Evidence
Replay output, service heading '### deadline__logs':
  [mismatch] deadline__logs.queue-read-role-credentials-put-log-events-rejected — Error: Check timed out after 60.0s
  [mismatch] deadline__logs.queue-read-role-credentials-read-queue-log-events — Error: Check timed out after 60.0s
Both check bodies block early on the same wait:
  _poll(lambda: [g['logGroupName'] for g in logs.describe_log_groups(logGroupNamePrefix=group)['logGroups']], lambda got: group in got, 120, "the queue's log group to exist")
with group = f"/aws/deadline/{farm_id}/{queue_id}" taken from the CreateQueue response, i.e. a 120 s wait inside a 60 s check budget; the second check then asserts names == [group].
Source: services/service-com-amazonaws-deadline-deadline/src/ops/create_queue.rs (handle, lines 167-257) only mints the queueId and inserts crate::state::Queue into farm.queues — there is no CloudWatch Logs interaction anywhere in the handler.
For contrast, the emulator does report the group's name: deadline.batch-get-job-entity-reports-per-job-why-attachments-are-not-returned reads jobDetails.logGroupName == /aws/deadline/<farmId>/<queueId>, and UpdateWorkerSchedule serves logConfiguration.options.logGroupName with the same value.

## Steps to Reproduce
1. Boot the emulator and replay the suite: the two deadline__logs checks named in trace_ids time out after 60 s.
2. By hand: dl.create_farm(displayName=...) -> farmId; iam.create_role(...) for a queue role; dl.create_queue(farmId=farmId, displayName=..., roleArn=<role arn>) -> queueId.
3. logs.describe_log_groups(logGroupNamePrefix=f'/aws/deadline/{farmId}/{queueId}') -> {'logGroups': []} on the emulator; on AWS the queue's group is listed. Polling it never converges.
4. logs.create_log_stream(logGroupName=f'/aws/deadline/{farmId}/{queueId}', logStreamName='session-...') -> ResourceNotFoundException on the emulator, succeeds on AWS.
