Issue-Source: trace-sweep-divergence
Issue-Trace: describe-job-execution-execution-number-bounds
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: describe-job-execution-execution-number-bounds iot-cancel-job-execution-status-details-limit
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeJobExecution|2xx->2xx|$.execution.forceCanceled
Issue-Key: iot|DescribeJobExecution|2xx->2xx|$.execution.status
Issue-Key: iot|DescribeJobExecution|2xx->2xx|$.execution.statusDetails
Issue-Key: iot|DescribeJobExecution|2xx->2xx|$.execution.versionNumber
Issue-Key: iot|DescribeJobExecution|4xx->4xx|$.message
Issue-Key: iot|DescribeJobExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeJobExecution|4xx->4xx|status
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot DescribeJobExecution diverges: a cancel never lands, statusDetails is omitted, and a bad executionNumber 404s

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobExecution

## Description
Three independent net-new root causes on the iot jobs control plane's DescribeJobExecution read. Each cause must be closed before submitting. Related but NOT covering these: open issue 20261125T045434Z-iot-canceljobexecution-stub.md records that CancelJobExecution is an unimplemented scaffold stub, but it carries no replay key for the DescribeJobExecution steps below and says nothing about the state the read must report, the executionNumber refusal, or the statusDetails shape; the replay reports all three clusters as unowned.

Cause 1 of 3: a canceled job execution is still reported as QUEUED — the cancel leaves no trace in the execution record.
AWS: Smithy CancelJobExecution (PUT /things/{thingName}/jobs/{jobId}/cancel) takes force (@httpQuery, default false), expectedVersion and statusDetails (DetailsMap) and returns an empty body; DescribeJobExecution (GET /things/{thingName}/jobs/{jobId}) returns a JobExecution whose members include status (JobExecutionStatus: IN_PROGRESS, QUEUED, FAILED, SUCCEEDED, TIMED_OUT, CANCELED, REJECTED), forceCanceled ("true if the job execution was canceled with the optional force parameter set to true"), statusDetails (JobExecutionStatusDetails {detailsMap}) and versionNumber ("incremented each time they are updated"). Recorded behavior: after a successful CancelJobExecution carrying statusDetails, the read reports status CANCELED, forceCanceled false (the request did not set force), statusDetails.detailsMap holding exactly the name/value pairs the cancel supplied, and versionNumber advanced by one (1 -> 2).
Emulator: the read keeps reporting status QUEUED, omits forceCanceled, omits statusDetails and leaves versionNumber at 1; polling the status 30 times never observes CANCELED.
Why: a fleet-management client that cancels a job execution and then waits for CANCELED never converges, and the cancel's statusDetails (its audit reason) are lost — the device is still handed a queued job the operator believes was canceled.
Fix: CancelJobExecution's write over the shared job-execution record — set status=CANCELED, set forceCanceled from the force query parameter, merge the request's statusDetails into detailsMap and bump versionNumber — and DescribeJobExecution's projection of those members (services/service-com-amazonaws-iot-awsiotservice/src/ops/cancel_job_execution.rs and describe_job_execution.rs over service_common::iot_job_executions).
Traces: describe-job-execution-execution-number-bounds#task#5 (cluster of 2: also iot-cancel-job-execution-status-details-limit#task#7).

Cause 2 of 3: an invalid executionNumber is answered as a missing execution.
AWS: DescribeJobExecution binds executionNumber as @httpQuery to ExecutionNumber (long) and models BOTH InvalidRequestException (httpError 400) and ResourceNotFoundException (404). Recorded behavior: an executionNumber outside the legal range is a request-validation failure, refused HTTP 400 with the message "Execution number is invalid." — four words of fixed text with no variable part; the 404 is reserved for a thing/job pair that has no execution at all.
Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException and the message "Job execution for thing <thingName> cannot be found within job <jobId>." — the not-found class and sentence for a request the real service rejects as malformed.
Why: a client distinguishes "I asked for a nonsense execution number" (fix the request, do not retry) from "this thing has no execution of this job" (a legitimate, retryable state). The emulator collapses the two, so the caller retries a request that can never succeed.
Fix: validate executionNumber's bounds in DescribeJobExecution before the execution lookup and raise InvalidRequestException with the sentence above (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_job_execution.rs).
Traces: describe-job-execution-execution-number-bounds#task#6.

Cause 3 of 3: statusDetails is omitted entirely where AWS serializes the member with an empty detailsMap.
AWS: JobExecution.statusDetails targets JobExecutionStatusDetails, a structure whose single member is detailsMap. Recorded behavior: for an execution that carries no status details, the response still contains execution.statusDetails as an object, with detailsMap absent/null inside it.
Emulator: execution.statusDetails is missing from the response document altogether.
Why: a client that reads execution['statusDetails']['detailsMap'] (as the SDK shapes allow once statusDetails is present) raises on a missing key against the emulator but not against AWS, so the structure the SDK models is not the structure served.
Fix: DescribeJobExecution's JobExecution serializer — emit the statusDetails structure whenever AWS does rather than eliding it when detailsMap is empty (services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_job_execution.rs and the shared JobExecution projection in crate::state).
Traces: iot-cancel-job-execution-status-details-limit#task#5.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. Thing/job names below are one run's values filling the templates.

Cause 1 — describe-job-execution-execution-number-bounds#task#5, DescribeJobExecution, status 200 both sides:
  $.execution.status: expected 'CANCELED', got 'QUEUED' (and: poll not in ['CANCELED'] after 30 attempts)
  $.execution.forceCanceled: expected False, got None (member absent)
  $.execution.statusDetails: expected {'detailsMap': {'reason': 'retired'}}, got None (member absent)
  $.execution.versionNumber: expected 2, got 1
  Cluster of 2: iot-cancel-job-execution-status-details-limit#task#7. ('reason'/'retired' is the cancel request's own statusDetails entry, not a fixed value.)

Cause 2 — describe-job-execution-execution-number-bounds#task#6, DescribeJobExecution:
  status: expected 400, got 404
  $.message: expected 'Execution number is invalid.', got 'Job execution for thing sim2real-probe-iot-exnum-dev-3769f5c7 cannot be found within job sim2real-probe-iot-exnum-job-8b6ccbb6.'
  $headers.x-amzn-errortype: got 'ResourceNotFoundException' where the recording's error class is the 400 InvalidRequestException one.
  AWS's message is wholly fixed text with no variable part; the emulator's interpolates the request's thingName and jobId into a sentence AWS does not emit here.

Cause 3 — iot-cancel-job-execution-status-details-limit#task#5, DescribeJobExecution, status 200 both sides:
  $.execution.statusDetails: expected {'detailsMap': None} (the member present, detailsMap empty), got None (the member absent).

Model citations: DescribeJobExecution GET /things/{thingName}/jobs/{jobId}, jobId/thingName @httpLabel @required, executionNumber @httpQuery (long); output execution -> JobExecution; errors InvalidRequestException(400), ResourceNotFoundException(404), ThrottlingException(400), ServiceUnavailableException(503). CancelJobExecution PUT /things/{thingName}/jobs/{jobId}/cancel, force @httpQuery default false, expectedVersion, statusDetails (DetailsMap), empty output; errors InvalidRequestException(400), InvalidStateTransitionException(409), VersionConflictException(409), ResourceNotFoundException(404). JobExecution members: jobId, status (JobExecutionStatus), forceCanceled (Forced, boolean), statusDetails (JobExecutionStatusDetails), thingArn, queuedAt, startedAt, lastUpdatedAt, executionNumber, versionNumber (default 0), approximateSecondsBeforeTimedOut.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. CreateThing(<thing>); CreateJob(<job>, targets=[thing arn], document=...) and confirm DescribeJobExecution reports status QUEUED, versionNumber 1.
3. CancelJobExecution(thingName=<thing>, jobId=<job>, statusDetails={'reason': 'retired'}), then GET /things/<thing>/jobs/<job> — expect status 'CANCELED', forceCanceled false, statusDetails.detailsMap == {'reason': 'retired'}, versionNumber 2; the emulator still reports QUEUED/versionNumber 1 with neither member present (Cause 1).
4. GET /things/<thing>/jobs/<job>?executionNumber=<out-of-range value> — expect 400 InvalidRequestException "Execution number is invalid."; the emulator answers 404 ResourceNotFoundException naming the thing and job (Cause 2).
5. GET /things/<thing>/jobs/<job> for an execution that never carried status details — the response must contain execution.statusDetails as an object; the emulator omits it (Cause 3).
6. Re-replay describe-job-execution-execution-number-bounds#task#5, #task#6 and iot-cancel-job-execution-status-details-limit#task#5 to confirm the fix.
