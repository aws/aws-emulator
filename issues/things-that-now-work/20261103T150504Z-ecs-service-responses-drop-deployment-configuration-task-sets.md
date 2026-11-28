Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsdep-traj-create-deployment-configuration-refusals
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsdep-traj-create-deployment-configuration-refusals ecs-ecsdep-traj-update-circuit-breaker-partial-struct ecs-ecsdep-traj-redeploy-same-revision-vs-force-new-deployment ecs-ecsdep-traj-service-deployment-read-mistakes ecs-ecscm-traj-create-service-launch-mistakes ecs-ntraf-service-binding-accepted-once-a-listener-forwards
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.alarms
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.deploymentCircuitBreaker.enable
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.deploymentCircuitBreaker.rollback
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentConfiguration.maximumPercent
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deploymentConfiguration.minimumHealthyPercent
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.maximumPercent
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deploymentConfiguration.minimumHealthyPercent
Issue-Key: ecs|ListServiceDeployments|2xx->2xx|$.serviceDeployments[*].serviceDeploymentArn
Issue-Key: ecs|ListServiceDeployments|2xx->2xx|$.serviceDeployments[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].launchType
Issue-Key: ecs|CreateService|2xx->2xx|$.service.platformVersion
Issue-Key: ecs|CreateService|2xx->2xx|$.service.taskSets[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].platformVersion
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].taskSets[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].version
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS service responses drop deployment configuration, task sets and deployment identity

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Nine net-new causes, one fault: what a client declares on CreateService/UpdateService is not what ECS stores or echoes back. Deployment configuration is replaced with defaults, task sets are omitted, and deployment identity is a counter rather than AWS's generated id. No open issue carries these replay keys.

Cause 1 of 9: deploymentConfiguration.alarms is dropped and deploymentCircuitBreaker.enable is forced false. AWS: DescribeServices echoes alarms = {alarmNames: <the names from the request>, enable: true, rollback: true} and deploymentCircuitBreaker.enable as declared. Smithy: DeploymentConfiguration.alarms -> DeploymentAlarms with @required alarmNames/enable/rollback. Emulator: alarms is absent and deploymentCircuitBreaker.enable is false. Why: a client cannot confirm its rollback guardrails were accepted, and believes they are off (or on) incorrectly. Fix: the service state and CreateService/UpdateService's deploymentConfiguration handling. Traces: ecs-ecsdep-traj-create-deployment-configuration-refusals#task#11.

Cause 2 of 9: deploymentCircuitBreaker.rollback is inverted/not persisted. AWS: DescribeServices returns rollback as declared (false here). Emulator: returns true. Why: the client thinks failed deployments will roll back when they will not (or the reverse). Fix: the service state's circuit-breaker fields. Traces: ecs-ecsdep-traj-update-circuit-breaker-partial-struct#task#7.

Cause 3 of 9: UpdateService discards maximumPercent/minimumHealthyPercent and answers with the defaults. AWS: the UpdateService response echoes the declared values (150 / 50 in the recording). Smithy: both are optional BoxedInteger members of DeploymentConfiguration; the REPLICA default is 200/100. Emulator: 200/100 — the service-wide defaults, i.e. the request's values were dropped. Why: the deployment batch size the client configured is silently not in effect. Fix: UpdateService's deploymentConfiguration merge. Traces: ecs-ecsdep-traj-update-circuit-breaker-partial-struct#task#11.

Cause 4 of 9: the same two members are also defaults on the following DescribeServices, confirming the loss is in state rather than in the UpdateService response serializer. AWS: 150 / 50. Emulator: 200 / 100. Fix: the service state. Traces: ecs-ecsdep-traj-update-circuit-breaker-partial-struct#task#12.

Cause 5 of 9: serviceDeploymentArn's trailing id is a zero-padded counter. AWS: the last ARN segment is a generated 21-22 character mixed-case base62 id (recorded behavior only — the model types serviceDeploymentArn as String). Emulator: '0000000000000000001'. Why: the shape is observable and a client that validates or round-trips the id (or distinguishes deployments by it) sees a format no AWS region produces; it is also guessable/enumerable. Fix: the service-deployment state's id generation. Traces: ecs-ecsdep-traj-redeploy-same-revision-vs-force-new-deployment#task#6 (cluster of 2 steps, also ecs-ecsdep-traj-service-deployment-read-mistakes).

Cause 6 of 9: redeploying the SAME task-definition revision without forceNewDeployment creates a second service deployment. AWS: ListServiceDeployments returns one deployment for the pair of calls — a no-change update does not start a deployment. Emulator: two entries (the extra one SUCCESSFUL with its own serviceRevisionArn). Why: the client's deployment history is wrong and 'has a new deployment started?' — the normal way to detect a rollout — answers yes when AWS answers no. Fix: UpdateService's handler — only open a new deployment when the resolved revision or forceNewDeployment requires it. Traces: ecs-ecsdep-traj-redeploy-same-revision-vs-force-new-deployment#task#10.

Cause 7 of 9: DescribeServices omits launchType. AWS: services[0].launchType = 'EC2' (Smithy: Service.launchType -> LaunchType). Emulator: the member is absent. Why: a client that routes on launchType (Fargate vs EC2 behavior) cannot tell, and treats the service as having no launch type. Fix: DescribeServices's serializer / the service state's launchType. Traces: ecs-ecscm-traj-create-service-launch-mistakes#task#13.

Cause 8 of 9: CreateService for an EXTERNAL/blue-green service returns no taskSets and reports platformVersion 'LATEST'. AWS: service.taskSets carries the PRIMARY task set (taskSetArn, id, scale, loadBalancers, networkConfiguration, stabilityStatus, status 'PRIMARY', ...) and platformVersion is the RESOLVED Fargate version, '1.4.0' in the recording, not the literal 'LATEST' the request asked for. Emulator: taskSets absent and platformVersion 'LATEST'. Why: a blue/green client has no task set to address, and a client pinning or logging the resolved platform version records the request alias instead. Fix: CreateService's handler (task-set creation) and the service state's platform-version resolution. Traces: ecs-ntraf-service-binding-accepted-once-a-listener-forwards#task#18.

Cause 9 of 9: the same service read back through DescribeServices still has no taskSets, still reports 'LATEST', and its version counter is 1 where AWS reports 0. AWS: services[1].version 0 for a freshly created service. Emulator: 1 — creation already bumped the optimistic-concurrency counter. Why: a client using version for optimistic concurrency computes the wrong expected value. Fix: the service state's version accounting plus DescribeServices's serializer. Traces: ecs-ntraf-service-binding-accepted-once-a-listener-forwards#task#21.

## Evidence
Replay of trace set simparity-wire-aws-v1; all nine clusters have empty owned_by and empty stub_path, status 200 on both sides.
Cause 1 — $.services[0].deploymentConfiguration.alarms expected {alarmNames: [<name>], enable: true, rollback: true}, got absent; deploymentCircuitBreaker.enable expected true, got false.
Cause 2 — $.services[0].deploymentConfiguration.deploymentCircuitBreaker.rollback expected false, got true.
Cause 3 — $.service.deploymentConfiguration.maximumPercent expected 150, got 200; minimumHealthyPercent expected 50, got 100.
Cause 4 — same two members on $.services[0] of the subsequent DescribeServices.
Cause 5 — $.serviceDeployments[0].serviceDeploymentArn trailing segment expected a generated base62 id, got '0000000000000000001'.
Cause 6 — $.serviceDeployments[1] 'extra (expected None)' — a second SUCCESSFUL deployment for an unchanged revision.
Cause 7 — $.services[0].launchType expected 'EC2', got absent.
Cause 8 — $.service.taskSets[0] expected a PRIMARY task set, got absent; $.service.platformVersion expected '1.4.0', got 'LATEST'.
Cause 9 — $.services[1].taskSets[0] missing; $.services[1].platformVersion expected '1.4.0', got 'LATEST'; $.services[1].version expected 0, got 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecsdep-traj-create-deployment-configuration-refusals, step task#11 — create a service with deploymentConfiguration.alarms and a circuit breaker, then DescribeServices; the alarms block is gone and enable is false.
3. Replay each fixture#step in trace_ids and diff the service/serviceDeployments body against the recorded response.
