Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-fargate-linux-parameters-unsupported-fields
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-fargate-linux-parameters-unsupported-fields ecs-register-task-definition-container-cpu-exceeds-task-cpu-rejected ecs-register-task-definition-container-memory-exceeds-task-memory-rejected ecs-register-task-definition-cpu-below-minimum-rejected ecs-register-task-definition-invalid-volume-name-rejected ecs-register-task-definition-rejects-port-mapping-name-starting-with-hyphen
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes
Issue-Key: ecs|DescribeTaskDefinition|2xx->2xx|$.taskDefinition.requiresAttributes
Issue-Key: ecs|RegisterTaskDefinition|2xx->2xx|$.taskDefinition.containerDefinitions[*].essential
Issue-Related: 20261120T005727Z-ecs-task-definition-records-render-wrong-compatibilities-volumes.md
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ECS RegisterTaskDefinition renders task-definition records with wrong compatibilities, essential, requiresAttributes and volumes

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
Seven net-new causes in the task-definition record ECS renders from RegisterTaskDefinition and reads back from DescribeTaskDefinition. Each is a separate work item; close all seven before submitting.

Cause 1 of 7: RegisterTaskDefinition omits taskDefinition.requiresAttributes entirely.
AWS: ECS derives the attributes a task definition needs from its container definitions and returns them in the registered record — here a single entry {'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.22'} (Smithy: TaskDefinition.requiresAttributes; the derived capability set is recorded behavior).
Emulator: the member is absent from the response.
Why: clients and the ECS agent use requiresAttributes to decide which container instances can run a revision; an empty/absent set makes every revision look universally placeable and hides the Docker-API floor the definition actually requires.
Fix: the requiresAttributes derivation in the RegisterTaskDefinition handler and the TaskDefinition serializer.
Traces: ecs-fargate-linux-parameters-unsupported-fields#task#7.

Cause 2 of 7: DescribeTaskDefinition reads the same revision back without requiresAttributes.
AWS: the describe of the registered revision returns the identical requiresAttributes list the register returned.
Emulator: the member is absent on the read path too, so it is not merely a response-shaping slip on register — it is never stored.
Fix: the stored task-definition record plus the DescribeTaskDefinition serializer.
Traces: ecs-fargate-linux-parameters-unsupported-fields#task#8.

Cause 3 of 7: the derived requiresAttributes entry names the wrong Docker remote-API version, and compatibilities carry the wrong member.
AWS: this revision's derived attribute is 'com.amazonaws.ecs.capability.docker-remote-api.1.22' and its compatibilities include MANAGED_INSTANCES (Smithy: Compatibility enum [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES]).
Emulator: derives 'com.amazonaws.ecs.capability.docker-remote-api.1.18' and renders compatibilities[1] as 'FARGATE' where MANAGED_INSTANCES is expected.
Why: both members gate placement; a revision that advertises a lower Docker-API floor and a missing compatibility is placed against the wrong candidate set.
Note: open issue 20261124T210114Z-ecs-task-definition-compatibilities-can-never-derive-managed.md describes the MANAGED_INSTANCES derivation gap for awsvpc revisions; this cluster is not marked owned by it and can be closed together with it if the root cause is shared.
Traces: ecs-fargate-linux-parameters-unsupported-fields#task#1.

Cause 4 of 7: container definitions read back without the defaulted essential member, and compatibilities drop MANAGED_INSTANCES.
AWS: 'If this parameter is omitted, a container is assumed to be essential' (Smithy: ContainerDefinition.essential doc), and the registered record reports the resolved value essential=true for each such container; compatibilities[2] is 'MANAGED_INSTANCES'.
Emulator: containerDefinitions[*].essential is absent from the rendered record, and compatibilities[2] is absent.
Why: a client reading a revision back cannot tell which containers are essential, so stop-propagation behavior and 'at least one essential container' validation cannot be reasoned about from the API.
Fix: default resolution for ContainerDefinition.essential at registration time and the containerDefinitions/compatibilities rendering.
Traces: ecs-register-task-definition-container-cpu-exceeds-task-cpu-rejected#task#3, ecs-register-task-definition-container-memory-exceeds-task-memory-rejected#task#3 (both containers), ecs-register-task-definition-cpu-below-minimum-rejected#task#3.

Cause 5 of 7: the registered revision number and taskDefinitionArn are one higher than AWS's, because an earlier invalid RegisterTaskDefinition was accepted instead of refused.
AWS: an invalid registration is refused (400, no revision consumed), so the next valid registration of that family is revision 1 and its ARN ends ':1'.
Emulator: returns revision 2 and an ARN ending ':2' — the refused request already consumed revision 1.
Why: callers that pin or compare a family revision (deployments, service task-definition references, CI promotion) address a revision AWS does not have, and the per-family revision sequence is permanently offset.
Fix: the RegisterTaskDefinition handler must validate before allocating a revision; depends on the registration validation gap tracked by open issue 20261120T081747Z-ecs-registertaskdefinition-accepts-a-healthy-container-dependency-with.md, which owns the refusal clusters themselves.
Traces: ecs-register-task-definition-container-cpu-exceeds-task-cpu-rejected#task#3, ecs-register-task-definition-container-memory-exceeds-task-memory-rejected#task#3, ecs-register-task-definition-cpu-below-minimum-rejected#task#3, ecs-register-task-definition-invalid-volume-name-rejected#task#3, ecs-register-task-definition-rejects-port-mapping-name-starting-with-hyphen#task#3.

Cause 6 of 7: a volume declared with no host configuration loses its empty host object.
AWS: the registered record reports volumes[].host as an empty object ({}) for a host volume declared without a sourcePath (Smithy: Volume.host -> HostVolumeProperties, whose sourcePath is optional).
Emulator: volumes[0].host is absent.
Why: a client round-tripping a task definition (drift comparison, re-register) sees the volume change shape, and cannot distinguish a host volume from a volume with no backing at all.
Fix: the Volume/HostVolumeProperties serializer in the task-definition rendering path.
Traces: ecs-register-task-definition-invalid-volume-name-rejected#task#3.

Cause 7 of 7: compatibilities are rendered in the wrong order/content for an EC2-only revision.
AWS: this revision reports compatibilities[0] 'EC2' with no second entry.
Emulator: reports compatibilities[0] 'EXTERNAL' and an extra compatibilities[1] 'EC2'.
Why: compatibilities is read positionally by generated clients and by humans comparing definitions; advertising EXTERNAL first claims a launch mode the revision was not registered for.
Fix: the compatibilities derivation/ordering in the task-definition serializer.
Traces: ecs-register-task-definition-rejects-port-mapping-name-starting-with-hyphen#task#3.

## Evidence
Cause 1 — $.taskDefinition.requiresAttributes expected [{'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.22'}], got None (RegisterTaskDefinition, HTTP 200 both sides).
Cause 2 — same diff on DescribeTaskDefinition: $.taskDefinition.requiresAttributes expected [{'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.22'}], got None.
Cause 3 — $.taskDefinition.requiresAttributes[0] expected {'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.22'}, got {'name': 'com.amazonaws.ecs.capability.docker-remote-api.1.18'}; $.taskDefinition.compatibilities[1] expected 'MANAGED_INSTANCES', got 'FARGATE'.
Cause 4 — $.taskDefinition.containerDefinitions[0].essential expected True, got None (and containerDefinitions[1].essential likewise in the memory fixture); $.taskDefinition.compatibilities[2] expected 'MANAGED_INSTANCES', got None.
Cause 5 — $.taskDefinition.revision expected 1, got 2; $.taskDefinition.taskDefinitionArn expected an ARN ending ':1' for the family, got the same ARN ending ':2'.
Cause 6 — $.taskDefinition.volumes[0].host expected {}, got None.
Cause 7 — $.taskDefinition.compatibilities[0] expected 'EC2', got 'EXTERNAL'; $.taskDefinition.compatibilities[1] expected absent, got 'EC2'.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and re-run these fixtures/steps:
1. ecs-fargate-linux-parameters-unsupported-fields#task#7 then #task#8 — RegisterTaskDefinition, then DescribeTaskDefinition on the returned revision; compare taskDefinition.requiresAttributes.
2. ecs-fargate-linux-parameters-unsupported-fields#task#1 — RegisterTaskDefinition for the Fargate revision; compare requiresAttributes[0] and compatibilities.
3. ecs-register-task-definition-container-cpu-exceeds-task-cpu-rejected#task#3, ecs-register-task-definition-container-memory-exceeds-task-memory-rejected#task#3, ecs-register-task-definition-cpu-below-minimum-rejected#task#3 — register the valid revision after the refused one; compare containerDefinitions[*].essential, compatibilities, revision and taskDefinitionArn.
4. ecs-register-task-definition-invalid-volume-name-rejected#task#3 — compare volumes[0].host.
5. ecs-register-task-definition-rejects-port-mapping-name-starting-with-hyphen#task#3 — compare compatibilities ordering.
