Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-create-service-rejects-daemon-scheduling-on-fargate
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-create-service-rejects-daemon-scheduling-on-fargate ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate ecs-create-service-rejects-platform-version-on-ec2-launch-type ecs-service-binpack-placement-field-must-be-cpu-or-memory ecs-service-awsvpc-subnet-count-limit
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|4xx->2xx|$.__type
Issue-Key: ecs|CreateService|4xx->2xx|$.message
Issue-Key: ecs|CreateService|4xx->2xx|status
Issue-Key: ecs|CreateService|2xx->4xx|$.service
Issue-Key: ecs|CreateService|2xx->4xx|status
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].currentServiceRevisions[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].events[*].message
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].schedulingStrategy
Issue-Key: ecs|CreateService|4xx->4xx|$.message
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].placementStrategy[*]
Issue-Key: ecs|CreateService|2xx->2xx|$.service.availabilityZoneRebalancing
Issue-Key: ecs|CreateService|2xx->2xx|$.service.placementStrategy[*].field
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].placementStrategy[*].field
Issue-Related: 20261103T150504Z-ecs-service-responses-drop-deployment-configuration-task-sets.md
Issue-Related: 20261122T202428Z-ecs-service-task-and-daemon-reads-return-wrong.md
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ECS CreateService skips launch-type parameter validation and renders service records with wrong defaults

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Eight independent net-new causes in the ECS CreateService request path and in the Service record it stores and reads back. Each is a separate work item; close all eight before submitting.

Cause 1 of 8: CreateService accepts service parameters that are incompatible with the requested launch type / scheduling strategy and returns 200 instead of refusing the request.
AWS: CreateService validates the combination of launchType, schedulingStrategy, platformVersion, placementConstraints and placementStrategy before creating anything and refuses with HTTP 400 InvalidParameterException (Smithy: CreateService errors include InvalidParameterException, httpError 400 defaulted; schedulingStrategy -> SchedulingStrategy enum [REPLICA, DAEMON]; launchType -> LaunchType; placementStrategy -> PlacementStrategies; platformVersion is a String only meaningful for FARGATE). The recorded refusals carry the fixed message text for their class, with no variable parts: schedulingStrategy=DAEMON with launchType=FARGATE -> 'The daemon scheduling strategy does not support the FARGATE launch type. Change the launch type and try again'; placementConstraints/placementStrategy with launchType=FARGATE -> 'Placement constraints are not supported with FARGATE launch type.'; platformVersion with launchType=EC2 -> refused; a binpack placementStrategy whose field is neither cpu nor memory -> refused (Smithy PlacementStrategy.field doc: 'For the binpack placement strategy, valid values are cpu and memory').
Emulator: returns HTTP 200 with a created service and no __type/message at all for every one of these four requests.
Why: a client that relies on ECS to reject an impossible service definition instead gets a service it cannot run, and the error handling it is written around (InvalidParameterException with the launch-type explanation) never fires. The wrongly-created service then poisons the rest of the caller's workflow (see causes 2, 3, 6 and the teardown DeleteCluster failure filed separately).
Fix: request validation in the CreateService handler (launch-type / scheduling-strategy / placement compatibility checks) before any state mutation.
Traces: ecs-create-service-rejects-daemon-scheduling-on-fargate#task#4, ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#4, ecs-create-service-rejects-platform-version-on-ec2-launch-type#task#3, ecs-service-binpack-placement-field-must-be-cpu-or-memory#task#3.

Cause 2 of 8: where the emulator does refuse a CreateService request, it refuses it for the wrong reason, with an idempotency message instead of the validation message.
AWS: the refusal for placement constraints on FARGATE is InvalidParameterException with the message 'Placement constraints are not supported with FARGATE launch type.' (fixed text, no variable parts). The same cluster covers the awsvpc subnet-count refusal, which likewise names the violated rule rather than idempotency.
Emulator: answers 400 with message 'Creation of service was not idempotent.' — a name-reuse message produced because an earlier invalid create (cause 1) already stored a service under that name.
Why: the message is the only machine-visible discriminator here (both are InvalidParameterException), so a client cannot tell 'your request is invalid' from 'you re-created with different parameters', and retry logic keyed on the idempotency message loops.
Fix: order of checks in the CreateService handler — run parameter validation before the client-token/name idempotency comparison, and emit the validation message.
Traces: ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#5, ecs-service-awsvpc-subnet-count-limit#task#4.

Cause 3 of 8: the valid CreateService that follows a refused one is itself refused with 400.
AWS: after the invalid request is rejected, nothing exists under that service name, so the corrected CreateService (same cluster, same serviceName, valid parameters) succeeds with HTTP 200 and the full Service record.
Emulator: answers HTTP 400 and no `service` member, because the invalid request of cause 1 already created a service with that name.
Why: the normal 'fix the request and retry' path AWS clients and CDK/CloudFormation providers follow is unavailable — the caller is wedged and must pick a new service name.
Fix: same root as cause 1 (no state should be written for a refused CreateService); confirm by replaying this step after cause 1 is fixed.
Traces: ecs-create-service-rejects-daemon-scheduling-on-fargate#task#5, ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#6, ecs-create-service-rejects-platform-version-on-ec2-launch-type#task#4.

Cause 4 of 8: DescribeServices reports the service with the refused request's scheduling strategy and with the DAEMON deployment-configuration defaults.
AWS: the described service is the one the valid create made — schedulingStrategy 'REPLICA' with the REPLICA rolling-update defaults deploymentConfiguration.maximumPercent 200 and minimumHealthyPercent 100 (Smithy DeploymentConfiguration.maximumPercent doc: 'The default maximumPercent value for a service using the REPLICA service scheduler is 200%').
Emulator: reports schedulingStrategy 'DAEMON' with maximumPercent 100 and minimumHealthyPercent 0 — the daemon defaults, carried over from the service the refused request created.
Why: a client reading back the service it just created sees a different scheduler and different deployment batch sizes than it asked for, so rollout maths (batch size, minimum healthy capacity) are wrong for every later deployment.
Fix: the DescribeServices read path plus the stored Service record written by CreateService (scheduling strategy and the strategy-dependent deploymentConfiguration defaults).
Traces: ecs-create-service-rejects-daemon-scheduling-on-fargate#task#6.

Cause 5 of 8: a FARGATE service created with no placementStrategy reads back with a placement strategy the emulator invented.
AWS: placementStrategy is echoed as submitted; a FARGATE service created without one has no placementStrategy entries in DescribeServices (Smithy: CreateServiceRequest.placementStrategy -> PlacementStrategies; placement strategies are not supported with the FARGATE launch type at all, see cause 1).
Emulator: DescribeServices returns an extra entry {'type': 'spread', 'field': 'attribute:ecs.availability-zone'}.
Why: every config-diffing client (CloudFormation drift detection, CDK re-apply, Terraform plan) sees a permanent difference between the submitted and reported service definition and reports drift forever.
Fix: the service serializer / state write for placementStrategy — store and echo only what the request carried, no default injection.
Traces: ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#7.

Cause 6 of 8: the service revision and deployment identity reported for a service do not match the ones minted for the create the caller actually made.
AWS: each service revision gets its own identity under .../service-revision/<cluster>/<service>/<revision-number> (the recording shows a large unique numeric revision number, e.g. a 19-digit value) and the deployment reported in service events is the deployment id of that create; DescribeServices' currentServiceRevisions and events[].message agree with the ids returned by the CreateService that produced them.
Emulator: reports a zero-padded counter ('0000000000000000001') as the revision number and names deployment 'ecs-svc/0000000000000000001' in '(service <name>) (deployment <deployment-id>) deployment completed.', i.e. the revision/deployment created by the refused request of cause 1 rather than by the caller's successful create — the fixture's captured revision/deployment ids never bind.
Why: a client that correlates a deployment or service revision it was handed at create time against a later read never finds it, so deployment tracking and service-revision pinning break.
Fix: service-revision and deployment identity allocation in the CreateService/deployment state, and the currentServiceRevisions / events rendering in DescribeServices. Depends on cause 1 (no revision should be minted for a refused create).
Traces: ecs-create-service-rejects-platform-version-on-ec2-launch-type#task#5.

Cause 7 of 8: the CreateService response reports availabilityZoneRebalancing ENABLED for a service that did not request it, and case-folds the placement-strategy field.
AWS: availabilityZoneRebalancing is an enum [ENABLED, DISABLED] (Smithy: AvailabilityZoneRebalancing); the recorded create of this EC2/binpack service reports 'DISABLED'. PlacementStrategy.field is a plain String with no enum or pattern (Smithy: only PlacementStrategy.type is an enum of random/spread/binpack), so it is echoed with exactly the spelling submitted — 'MEMORY' here.
Emulator: returns availabilityZoneRebalancing 'ENABLED' and placementStrategy[0].field 'memory'.
Why: the caller's own service definition does not round-trip: AZ rebalancing is reported on for a service that has it off (changing the caller's model of rebalancing behavior), and the lower-cased field makes every re-apply/drift comparison differ.
Fix: the CreateService response serializer / stored Service record — default availabilityZoneRebalancing per the created service instead of unconditionally ENABLED, and store PlacementStrategy.field verbatim.
Note: open issue 20261122T202428Z-ecs-service-task-and-daemon-reads-return-wrong.md describes the same availabilityZoneRebalancing defect on UpdateService/DescribeServices, and open issue 20261127T105620Z-ecs-4-net-new-causes-ec2-container-instances.md describes the placement-field case-folding from the behavioral suite; this replay cluster is not marked owned by either, so it is filed here and can be closed together with them if the root cause is shared.
Traces: ecs-service-binpack-placement-field-must-be-cpu-or-memory#task#4.

Cause 8 of 8: the same two field defects as cause 7 are also present on the DescribeServices read path.
AWS: DescribeServices reports availabilityZoneRebalancing 'DISABLED' and placementStrategy[0].field 'MEMORY' for this service.
Emulator: reports 'ENABLED' and 'memory'.
Why: the wrong values are not just a response-shaping slip on create — they are what the stored service reads back as, so every subsequent reader sees them.
Fix: the DescribeServices serializer and the shared Service state these fields are read from.
Traces: ecs-service-binpack-placement-field-must-be-cpu-or-memory#task#5.

## Evidence
Cause 1 — expected HTTP 400 with $.__type 'InvalidParameterException' and $.message 'The daemon scheduling strategy does not support the FARGATE launch type. Change the launch type and try again'; got HTTP 200 with no __type and no message (cluster of 4 steps: daemon-on-FARGATE, placement-strategy/constraints-on-FARGATE, platformVersion-on-EC2, binpack field not cpu/memory).
Cause 2 — expected $.message 'Placement constraints are not supported with FARGATE launch type.'; got 'Creation of service was not idempotent.' (both HTTP 400).
Cause 3 — expected HTTP 200 with the full $.service record; got HTTP 400 with no $.service.
Cause 4 — $.services[0].schedulingStrategy expected 'REPLICA', got 'DAEMON'; $.services[0].deploymentConfiguration.maximumPercent expected 200, got 100; minimumHealthyPercent expected 100, got 0.
Cause 5 — $.services[0].placementStrategy[0] expected absent, got {'type': 'spread', 'field': 'attribute:ecs.availability-zone'}.
Cause 6 — $.services[0].currentServiceRevisions[0].arn ends in the emulator's '0000000000000000001' instead of the revision id minted for the caller's create; $.services[0].events[1].message got '(service <service-name>) (deployment ecs-svc/0000000000000000001) deployment completed.' where the deployment id must be the one returned by that service's own create.
Cause 7 — $.service.availabilityZoneRebalancing expected 'DISABLED', got 'ENABLED'; $.service.placementStrategy[0].field expected 'MEMORY', got 'memory'.
Cause 8 — $.services[0].availabilityZoneRebalancing expected 'DISABLED', got 'ENABLED'; $.services[0].placementStrategy[0].field expected 'MEMORY', got 'memory'.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and re-run these fixtures/steps:
1. ecs-create-service-rejects-daemon-scheduling-on-fargate#task#4 — CreateService with schedulingStrategy=DAEMON and launchType=FARGATE; expect 400 InvalidParameterException.
2. ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#5 — CreateService with placementConstraints and launchType=FARGATE; check the refusal message.
3. ecs-create-service-rejects-daemon-scheduling-on-fargate#task#5 — re-issue the corrected CreateService under the same serviceName; expect 200.
4. ecs-create-service-rejects-daemon-scheduling-on-fargate#task#6 — DescribeServices and compare schedulingStrategy and deploymentConfiguration.
5. ecs-create-service-rejects-placement-strategy-and-constraints-on-fargate#task#7 — DescribeServices and compare placementStrategy.
6. ecs-create-service-rejects-platform-version-on-ec2-launch-type#task#5 — DescribeServices and compare currentServiceRevisions[0].arn and events[].message against the ids the create returned.
7/8. ecs-service-binpack-placement-field-must-be-cpu-or-memory#task#4 and #task#5 — CreateService with placementStrategy [{type: binpack, field: MEMORY}] on EC2, then DescribeServices; compare availabilityZoneRebalancing and placementStrategy[0].field.
