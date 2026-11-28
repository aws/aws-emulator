Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-terminate-running-job-stops-container-before-completion-marker
Issue-Key: batch__logs.batch-terminate-running-job-stops-container-before-completion-marker|<lambda>#29|Timeout|resp = _safe(lambda kw=dict(kwargs): logs.get_log_events(**kw))
Issue-Commit: 783077cd783724858ba947c044602be1843996e1

# batch__logs: TerminateJob of a RUNNING job is unobservable — the scenario reaches no verdict inside the runner budget because a placed job's container never executes and never reaches RUNNING (1 cause)

> _Note: this issue was identified against the code base at commit `783077cd783724858ba947c044602be1843996e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateJob

## Description
One net-new root cause in the AWS Batch job data plane, grounded in the one batch__logs check the suite left unowned.

Related open issue that does NOT cover this check any more: 20261119T003525Z-batch-logs-a-placed-job-s-container-never.md lists this check id among its trace ids, but its recorded replay key for it is an AssertionError at run#15 (`assert running["status"] == "RUNNING"`). On this replay the check no longer reaches that assertion at all — it is killed by the runner at its budget before any status/log observation succeeds — so the suite reports it as unowned and it is filed here. Also related but not covering it: 20261119T120645Z-batch-logs-queued-jobs-are-never-dispatched-a.md, 20261104T041055Z-batch-logs-array-job-submission-is-unimplemented-and.md.

Cause 1 of 1: a submitted Batch job never passes through an observable RUNNING window and its container's stdout is never delivered to the awslogs stream, so TerminateJob of a running job cannot be exercised and the scenario reaches no verdict.
AWS: Smithy (com.amazonaws.batch): JobStatus enumerates SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED; AttemptDetail.startedAt is "the Unix timestamp (in milliseconds) for when the attempt was started (when the attempt transitioned from the STARTING state to the RUNNING state)" and AttemptDetail.stoppedAt the transition "from the RUNNING state to a terminal state, such as SUCCEEDED or FAILED"; AttemptContainerDetail.logStreamName is "The name of the CloudWatch Logs log stream that's associated with the container... Each container attempt receives a log stream name when they reach the RUNNING status"; AttemptContainerDetail.exitCode: "The exit code for the job attempt. A non-zero exit code is considered failed." TerminateJob(jobId required, reason required) is POST /v1/terminatejob returning 200 with an empty body, errors ClientException (400) / ServerException (500). On AWS a job submitted to a VALID queue on a MANAGED FARGATE compute environment is placed, its image is pulled, the container runs and each stdout line is delivered as one event to the awslogs group/stream named by container.logStreamName; DescribeJobs reports status RUNNING for the whole duration of that execution; TerminateJob on it stops the container, the job becomes FAILED, JobDetail.statusReason is the caller-supplied reason verbatim, and startedAt/stoppedAt bracket an interval shorter than the command the container never finished.
Emulator: the check never reaches a verdict. It is killed by the runner with "Error: Check timed out after 60.0s" while still blocked on its first data-plane observations — the control job's DONE line in the awslogs stream and then a DescribeJobs read reporting the victim job as RUNNING. Neither the per-line stdout delivery nor an observable RUNNING window ever materialises, so TerminateJob's documented effects (status FAILED, statusReason echoed verbatim, startedAt/stoppedAt bracketing a truncated attempt) are all unreachable. The job's own deadlines in the check are generous (job 1200 s, logs 420 s); nothing is observed at all.
Why: a client cannot observe, poll on, or cancel work on this emulator. Progress reporting keyed on RUNNING never advances, cancellation (TerminateJob) can only ever be aimed at a job that is not running, and any consumer that reads the job's output from the stream named by container.logStreamName blocks forever or silently processes an empty result.
Fix: the Batch job data-plane runner — the component that dispatches a RUNNABLE job onto its compute environment's backing ECS cluster, runs the container image, holds the job in RUNNING until the container exits, and the awslogs log-driver delivery path that writes the container's stdout lines into the configured log group/stream; plus TerminateJob's handler, which must stop the running container and record the caller's reason in JobDetail.statusReason rather than only writing a status.
Traces: batch__logs.batch-terminate-running-job-stops-container-before-completion-marker.

## Evidence
Expected (AWS): after SubmitJob of a container running ["sh","-c","echo START; sleep <n> && echo DONE"] with an awslogs logConfiguration, (a) the control job's log stream holds its START and DONE lines, (b) DescribeJobs on the long-sleeping job returns status 'RUNNING' with container.logStreamName set and its START line readable from that stream, (c) TerminateJob(jobId, reason) returns 200, (d) DescribeJobs then reports status 'FAILED' with statusReason equal to the supplied reason string verbatim, no DONE line ever appears in the stream, and startedAt/stoppedAt are both present with (stoppedAt-startedAt)/1000 < the sleep length.

Actual (emulator): no verdict. The check is terminated by the runner: "Error: Check timed out after 60.0s" while waiting on the container's log output and on the first RUNNING read; none of (a)-(d) is ever observed. The 60 s figure is the runner's budget and the sleep lengths/ids in the check are per-run values — the requirement is the observable RUNNING window, the per-line awslogs delivery, the FAILED terminal status and the verbatim statusReason.

## Steps to Reproduce
1. CreateComputeEnvironment (type MANAGED, FARGATE) and CreateJobQueue bound to it; poll until the queue is VALID.
2. CreateLogGroup, and create a task execution role allowing ecr:GetAuthorizationToken/BatchGetImage/GetDownloadUrlForLayer/BatchCheckLayerAvailability and logs:CreateLogStream/PutLogEvents.
3. RegisterJobDefinition (type container, FARGATE) with image public.ecr.aws/docker/library/busybox:1.36, command ["sh","-c","echo START; sleep 240 && echo DONE"] and logConfiguration logDriver=awslogs pointing at that log group.
4. SubmitJob twice: once with a containerOverrides command that sleeps 3 s (control), once unmodified (victim).
5. Poll DescribeJobs on the control job to SUCCEEDED, then GetLogEvents/FilterLogEvents on its container.logStreamName — no START/DONE event is ever delivered.
6. Poll DescribeJobs on the victim job — it is never observed with status RUNNING carrying its own logStreamName, so TerminateJob(jobId=victim, reason="<caller reason>") can never be issued against a running job, and the FAILED/statusReason/startedAt-stoppedAt assertions are unreachable.
