Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-launch-and-exec-client-refusals
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-launch-and-exec-client-refusals ecs-run-task-request-refusals ecs-run-task-started-by-accepts-long-and-mixed-charset-values ecs-service-binpack-placement-field-must-be-cpu-or-memory ecs-task-calls-on-inactive-and-missing-clusters
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|4xx->4xx|$.message
Issue-Key: ecs|ExecuteCommand|4xx->4xx|$.message
Issue-Key: ecs|RunTask|4xx->4xx|$.__type
Issue-Key: ecs|RunTask|2xx->2xx|$.tasks[*].taskComputeId
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].attachments[*].details[*]
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].taskComputeId
Issue-Key: ecs|DescribeTasks|2xx->2xx|$.tasks[*].version
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|StopTask|4xx->4xx|$.__type
Issue-Key: ecs|StopTask|4xx->4xx|$.message
Issue-Related: 20261122T202428Z-ecs-service-task-and-daemon-reads-return-wrong.md
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ECS task, exec and cluster calls answer with the wrong refusal and omit task fields

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Seven net-new causes across the ECS task launch, exec and cluster-delete paths. Each is a separate work item; close all seven before submitting.

Cause 1 of 7: RunTask answers every malformed request with a capacity message instead of the validation message for the violated rule.
AWS: RunTask validates the request before attempting placement and refuses with the message naming the offending member — for an override that references a device no container declares, 'Override for inferenceAccelerator named <device-name> is not referenced in any containers' (fixed text, with the device name taken from the request's overrides). The cluster covers four such refusals in the RunTask request-validation family (Smithy: RunTask errors include InvalidParameterException and ClientException, both httpError 400 defaulted; RunTaskRequest.overrides -> TaskOverride).
Emulator: answers 400 with message 'No Container Instances were found in your cluster.' for all of them — a placement/capacity message, i.e. placement is attempted before (or instead of) request validation.
Why: the message is the only discriminator between 'your request is wrong' and 'the cluster has no capacity'. A client sees a retryable capacity condition where AWS reported a permanent request defect, so it retries forever instead of fixing the override.
Fix: ordering and message selection in the RunTask handler — validate overrides/request members and emit their messages before the candidate-capacity check.
Traces: ecs-launch-and-exec-client-refusals#task#6, ecs-run-task-request-refusals#task#5, ecs-run-task-request-refusals#task#7, ecs-run-task-request-refusals#task#9.

Cause 2 of 7: ExecuteCommand's non-interactive refusal uses the documentation sentence instead of the API message.
AWS: ExecuteCommand with interactive=false is refused 400 with the message 'Interactive is the only mode supported currently.' (fixed text, no variable parts; Smithy: ExecuteCommandRequest.interactive is a required Boolean, errors include InvalidParameterException).
Emulator: answers 400 with 'Amazon ECS only supports initiating interactive sessions, so you must specify true for this value.' — the model's prose for the member, not the refusal message.
Why: clients and tests that match on the refusal text (the AWS CLI surfaces it verbatim) do not recognize the condition.
Fix: the message constant in the ExecuteCommand validation path.
Traces: ecs-launch-and-exec-client-refusals#task#7.

Cause 3 of 7: RunTask with a task role ECS cannot assume is refused with the wrong error type and message.
AWS: refuses 400 ClientException with the message "ECS was unable to assume the role '<role-arn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role." — fixed text with the role ARN taken from the task definition's taskRoleArn/executionRoleArn (Smithy: ClientException is a modeled RunTask error, httpError 400).
Emulator: answers 400 InvalidParameterException with message 'No Container Instances were found in your cluster.'
Why: the error type is what clients branch on; a role-trust misconfiguration is reported as a transient capacity problem, so it is retried rather than surfaced to the operator.
Fix: role-assumption validation in the RunTask handler, raising ClientException with the role-assume message.
Traces: ecs-run-task-request-refusals#task#4.

Cause 4 of 7: the RunTask response omits tasks[].taskComputeId.
AWS: the recorded RunTask response carries a taskComputeId on each launched task (recorded behavior only — the member is not present in this repo's on-disk ECS Smithy model, so there is nothing modeled to cite).
Emulator: the member is absent.
Why: a client that keys a launched task to its compute allocation has no id to key on.
Fix: the Task serializer / task state used by RunTask.
Traces: ecs-run-task-started-by-accepts-long-and-mixed-charset-values#task#6, #task#8.

Cause 5 of 7: DescribeTasks omits taskComputeId and the container image digest, names the wrong attachment detail, and over-counts the task version.
AWS: DescribeTasks reports taskComputeId, containers[].imageDigest (the sha256 digest of the image actually pulled), and an awsvpc ENI attachment whose details include a 'privateDnsName' entry; the task's version counter advances once per observed state transition (3 for this task in the recording). (Smithy: Task.containers -> Container.imageDigest, Task.attachments -> Attachment.details, Task.version; taskComputeId is recorded behavior only.)
Emulator: omits taskComputeId and imageDigest, emits 'privateIPv4Address' where the 'privateDnsName' detail belongs, and reports version 6.
Why: image-digest pinning and ENI DNS resolution are both unavailable to a client, and a version that advances faster than AWS's breaks optimistic-concurrency / change-detection loops built on Task.version.
Fix: the DescribeTasks serializer, the ENI attachment detail construction, and the task version accounting in the task state machine.
Note: open issue 20261122T202428Z-ecs-service-task-and-daemon-reads-return-wrong.md describes the missing imageDigest on another DescribeTasks step; this cluster is not marked owned by it and can be closed together with it if the root cause is shared.
Traces: ecs-run-task-started-by-accepts-long-and-mixed-charset-values#task#7, #task#9.

Cause 6 of 7: StopTask on an INACTIVE cluster reports the cluster state instead of the missing task.
AWS: refuses 400 InvalidParameterException with the message 'The referenced task was not found' — the task reference is resolved and reported before the cluster's INACTIVE state (Smithy: StopTask errors include InvalidParameterException and ClusterNotFoundException, both httpError 400 defaulted).
Emulator: answers 400 ClusterNotFoundException with 'The referenced cluster was inactive.'
Why: the error type differs, so a caller cleaning up tasks after a cluster delete takes the cluster-missing branch (recreate/abort) instead of the benign task-not-found branch.
Fix: check ordering in the StopTask handler — resolve the task reference first and emit its not-found refusal.
Traces: ecs-task-calls-on-inactive-and-missing-clusters#task#7.

Cause 7 of 7: DeleteCluster refuses the teardown of a cluster AWS deletes successfully.
AWS: the teardown DeleteCluster for this fixture returns HTTP 200 with the deleted Cluster record (Smithy: DeleteCluster, idempotent, output cluster -> Cluster; ClusterContainsServicesException is modeled only for a cluster that still holds services).
Emulator: answers HTTP 400 — the cluster still holds the extra service created by the CreateService validation gap, so the contains-services guard fires.
Why: a caller's cleanup path fails and leaks a cluster, and in a provisioning tool the stack delete fails outright.
Fix: the DeleteCluster handler's contains-services guard is correct only once the cluster's service inventory is; this is a cascade of the CreateService validation gap filed in the companion CreateService bundle (fixture ecs-create-service-rejects-daemon-scheduling-on-fargate#task#4). Re-replay this teardown step after that is fixed.
Traces: ecs-service-binpack-placement-field-must-be-cpu-or-memory#teardown#8.

## Evidence
Cause 1 — $.message expected 'Override for inferenceAccelerator named device1 is not referenced in any containers', got 'No Container Instances were found in your cluster.' (HTTP 400 both sides; 4 steps in the cluster).
Cause 2 — $.message expected 'Interactive is the only mode supported currently.', got 'Amazon ECS only supports initiating interactive sessions, so you must specify true for this value.' (HTTP 400 both sides).
Cause 3 — $.__type expected 'ClientException', got 'InvalidParameterException'; $.message expected "ECS was unable to assume the role '<role-arn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role.", got 'No Container Instances were found in your cluster.'
Cause 4 — $.tasks[0].taskComputeId expected present, got None (HTTP 200 both sides).
Cause 5 — $.tasks[0].taskComputeId expected present, got None; $.tasks[0].containers[0].imageDigest expected a 'sha256:...' digest, got None; $.tasks[0].attachments[0].details[3] expected {'name': 'privateDnsName', ...}, got {'name': 'privateIPv4Address', ...}; $.tasks[0].version expected 3, got 6.
Cause 6 — $.__type expected 'InvalidParameterException', got 'ClusterNotFoundException'; $.message expected 'The referenced task was not found', got 'The referenced cluster was inactive.'
Cause 7 — status expected 200, got 400 on the teardown DeleteCluster.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and re-run these fixtures/steps:
1. ecs-launch-and-exec-client-refusals#task#6 (and ecs-run-task-request-refusals#task#5/#7/#9) — RunTask with an overrides entry referencing an inferenceAccelerator no container declares; compare the refusal message.
2. ecs-launch-and-exec-client-refusals#task#7 — ExecuteCommand with interactive=false.
3. ecs-run-task-request-refusals#task#4 — RunTask with a task role that cannot be assumed; compare __type and message.
4. ecs-run-task-started-by-accepts-long-and-mixed-charset-values#task#6 — RunTask and inspect tasks[0].taskComputeId.
5. ecs-run-task-started-by-accepts-long-and-mixed-charset-values#task#7 — DescribeTasks on that task; inspect taskComputeId, containers[0].imageDigest, attachments[0].details and version.
6. ecs-task-calls-on-inactive-and-missing-clusters#task#7 — StopTask naming an unknown task on an INACTIVE cluster.
7. ecs-service-binpack-placement-field-must-be-cpu-or-memory#teardown#8 — run the fixture to teardown and DeleteCluster.
