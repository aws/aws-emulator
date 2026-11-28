Issue-Source: trace-sweep-divergence
Issue-Trace: iot-thing-registration-task-not-found
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-thing-registration-task-not-found iot-stop-thing-registration-task-id-length-bound
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|ListThingRegistrationTaskReports|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|ListThingRegistrationTaskReports|4xx->4xx|status
Issue-Key: iot|StopThingRegistrationTask|4xx->4xx|$.message
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot thing-registration-task refusals answer the wrong status and drop the rejected taskId

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListThingRegistrationTaskReports

## Description
Two independent net-new root causes in the iot thing-registration-task (bulk provisioning) plane's refusal paths. Each cause must be closed before submitting. Related but NOT covering these: open issue 20261127T130644Z-iot-stopthingregistrationtask-stub.md records StopThingRegistrationTask as an unimplemented scaffold stub and says nothing about either refusal; open issue 20261127T130644Z-iot-7-net-new-causes-listcommandexecutions-three-validationexception.md (cause 4) records the same value-dropping constraint formatter for a different member (ListThings.maxResults). The replay reports both clusters below as unowned.

Cause 1 of 2: ListThingRegistrationTaskReports answers 400 InvalidRequestException for a taskId that names no task, where AWS answers 404.
AWS: Smithy ListThingRegistrationTaskReports (GET /thing-registration-tasks/{taskId}/reports) binds taskId as @httpLabel @required (TaskId @length 0..40) and reportType as @httpQuery @required; the on-disk model lists only InvalidRequestException (400), ThrottlingException (400), UnauthorizedException (401) and InternalFailureException (500) — ResourceNotFoundException is NOT modeled on this operation, so this is recorded behavior only: a well-formed taskId that names no registration task is refused with HTTP 404 (the recording pins the status and the not-found error class rather than the exact header string).
Emulator: HTTP 400 with x-amzn-errortype "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/" — a request-validation refusal for a request that is well formed and merely names an absent resource.
Why: 400 tells a client its request is malformed and must not be retried; 404 tells it the task is absent (it may still be starting, or the id is wrong). A bulk-provisioning poller that waits for a task's reports treats the emulator's 400 as a permanent client bug and aborts, and HTTP-status-based retry policies take the wrong branch.
Fix: ListThingRegistrationTaskReports' task lookup must distinguish an absent task from an invalid request and raise the not-found refusal with status 404 (services/service-com-amazonaws-iot-awsiotservice/src/ops/list_thing_registration_task_reports.rs plus the thing-registration-task store it reads).
Traces: iot-thing-registration-task-not-found#task#3.

Cause 2 of 2: StopThingRegistrationTask's @length violation message omits the rejected taskId.
AWS: Smithy StopThingRegistrationTask (PUT /thing-registration-tasks/{taskId}/cancel) binds taskId as @httpLabel @required to TaskId with @length(min 0, max 40) and models InvalidRequestException (400). Recorded behavior: a taskId longer than the maximum is refused 400 with "1 validation error detected: Value '<taskId>' at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40". Fixed text: "1 validation error detected: Value '", "' at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40"; the only variable part is the rejected taskId copied from the request path (40 is the modeled maximum).
Emulator: same status and class, but the message is "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40" — the "'<value>' " segment is missing, so the sentence reads "Value at 'taskId'".
Why: the operator loses the one fact the message exists to carry (which value was rejected), and exact-match comparisons against AWS fail. Note the echo decision is per member (a resolved iot issue records CreateRoleAlias.credentialDurationSeconds as a member that must NOT echo its value), so this cannot be fixed by flipping the shared formatter globally.
Fix: StopThingRegistrationTask's input validation path and the shared constraint-violation message builder's @length branch (services/service-com-amazonaws-iot-awsiotservice/src/ops/stop_thing_registration_task.rs plus the shared validation-message builder).
Traces: iot-stop-thing-registration-task-id-length-bound#task#2 (cluster of 2: also iot-thing-registration-task-not-found#task#5).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. The probe task ids below are one run's values filling the templates.

Cause 1 — iot-thing-registration-task-not-found#task#3, ListThingRegistrationTaskReports:
  status: expected 404, got 400
  $headers.x-amzn-errortype: the recording's not-found error class vs actual 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/'
  (the fixture pins the status and the error class shape, not the exact message, so the status/class pair is the requirement.)

Cause 2 — iot-stop-thing-registration-task-id-length-bound#task#2, StopThingRegistrationTask, status 400 both sides:
  expected $.message "1 validation error detected: Value 'sim2real-probe-stop-3f8f1c49c94d7111b4fe5' at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40"
  actual   $.message "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40"
  Cluster of 2: iot-thing-registration-task-not-found#task#5. Fixed text as described in the cause; the only variable part is the over-long taskId from the request path.

Model citations: ListThingRegistrationTaskReports GET /thing-registration-tasks/{taskId}/reports, taskId @httpLabel @required (TaskId @length 0..40), reportType @httpQuery @required, @paginated(nextToken/resourceLinks/maxResults); errors InvalidRequestException(400), ThrottlingException(400), UnauthorizedException(401), InternalFailureException(500) — no ResourceNotFoundException is modeled, hence 'recorded behavior only' for the 404. StopThingRegistrationTask PUT /thing-registration-tasks/{taskId}/cancel, taskId @httpLabel @required (@length 0..40); errors InvalidRequestException(400), ResourceNotFoundException(404), ThrottlingException(400), UnauthorizedException(401), InternalFailureException(500).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. GET /thing-registration-tasks/<a well-formed taskId that was never started>/reports?reportType=ERRORS — expect HTTP 404 (not-found class); the emulator answers 400 InvalidRequestException (Cause 1).
3. PUT /thing-registration-tasks/<a taskId longer than 40 characters>/cancel — the InvalidRequestException message must quote the rejected value: "1 validation error detected: Value '<taskId>' at 'taskId' failed to satisfy constraint: Member must have length less than or equal to 40"; the emulator omits the quoted value (Cause 2).
4. Re-replay iot-thing-registration-task-not-found#task#3 and iot-stop-thing-registration-task-id-length-bound#task#2 to confirm the fix.
