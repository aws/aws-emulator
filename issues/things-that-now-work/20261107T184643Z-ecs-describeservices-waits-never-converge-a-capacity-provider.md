Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Step: task#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|2xx->4xx|$
Issue-Key: ecs|DescribeServices|2xx->4xx|$.failures
Issue-Key: ecs|DescribeServices|2xx->4xx|$.services
Issue-Key: ecs|DescribeServices|2xx->4xx|status
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# ECS DescribeServices waits never converge: a capacity-provider service is refused 400, and a circuit-breaker deployment never reaches a terminal rolloutState

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Two net-new root causes sharing one fault: a client that polls DescribeServices to wait for a service to reach the state it asked for never gets there against the emulator. In one case the read itself is refused with HTTP 400 so the wait can never succeed; in the other the read answers 200 forever with a deployment that never leaves IN_PROGRESS. BOTH causes must be closed before this issue is submitted.

Cause 1 of 2: DescribeServices on a service in a capacity-provider cluster is refused with HTTP 400 where AWS answers 200 with the service, so the wait step is retried until its 180s poll cap. Fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step task#16.
AWS: Smithy (com.amazonaws.ecs) DescribeServices is a read-only operation whose input is cluster + services (StringList, required) + include, and whose output is DescribeServicesResponse {services: Services, failures: Failures}. Its modelled errors are AccessDeniedException (403), ClientException, ClusterNotFoundException, InvalidParameterException (all 400) and ServerException (500). For an existing cluster, AWS answers HTTP 200: every identifier that resolves is returned in `services` as a full Service element (clusterArn, capacityProviderStrategy, desiredCount, deploymentConfiguration, deploymentController, deployments[] with capacityProviderStrategy / rolloutState / stabilityStatus / status / taskDefinition, currentServiceDeployment, currentServiceRevisions), and every identifier that does not resolve is reported inside the SAME 200 response as a Failure (Smithy: Failure {arn, reason, detail}; reason 'MISSING'), not as an error status. ClusterNotFoundException is reserved for a cluster that genuinely does not exist ("The specified cluster wasn't found") and ServiceNotFoundException for a service that does not ("The specified service wasn't found"); neither applies to a cluster/service the caller has just created. A client is therefore entitled to poll DescribeServices for a target state and to receive 200 on every poll.
Emulator: the step is refused with HTTP 400 and a body carrying neither `services` nor `failures` — the replay reports "status: expected 200, got 400" with diverging keys $, $.services, $.failures, status — so the poll's success condition can never be evaluated and the step is retried until the replay's 180s poll cap ("$: poll capped at 180s"). Hypothesis: the same cluster/service lookup that fails elsewhere in this trajectory fails here — the neighbouring RunTask step (task#13) of this fixture shows the emulator answering ClusterNotFoundException 'Cluster not found.' for this cluster — so the describe is being turned into a 400 instead of a 200 whose `failures` list names the unresolved identifier.
Why: a read-only describe that answers 400 breaks every waiter built on it (the CLI `ecs wait services-stable`, CDK/CodePipeline rollout gates, any poll on services[0].deployments[0].rolloutState). The client cannot distinguish "this service is not there" (which AWS expresses as a 200 with reason MISSING and is a normal, retryable poll result) from "your request was bad" (a 400), so it aborts a deployment that AWS would have reported as progressing, and it can never read back the capacityProviderStrategy / deployments it is waiting on.
Fix: the ECS DescribeServices handler and its cluster/service resolution in the ecs service crate — resolve `cluster` and each entry of `services` by short name or full ARN, answer 200 with the Service element for each resolved identifier and a Failure {arn, reason 'MISSING'} for each unresolved one, and raise ClusterNotFoundException only when the named cluster itself is absent. Open issue 20261106T135659Z-ecs-ec2-autoscaling-capacity-providers-are-invisible-after.md describes the upstream capacity-provider invisibility in this same fixture (its causes cover DescribeCapacityProviders, CreateCluster, RunTask and UpdateService) but carries no replay key for this DescribeServices step, so a fix to it will not re-run or close this one.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#16 (DescribeServices).

Cause 2 of 2: a service whose deployment circuit breaker is enabled never produces a terminal deployment state, so a DescribeServices wait step answers 200 indefinitely and the whole fixture budget is consumed. Fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15.
AWS: Smithy Deployment.rolloutState -> DeploymentRolloutState, an enum constrained to COMPLETED | FAILED | IN_PROGRESS, with Deployment.rolloutStateReason, Deployment.status (PRIMARY | ACTIVE | INACTIVE) and the running/pending/desired counts alongside it. The model states the contract: a deployment begins IN_PROGRESS, transitions to COMPLETED when the service reaches a steady state, and transitions to FAILED when the service fails to reach a steady state and the circuit breaker is turned on (DeploymentCircuitBreaker.enable / .rollback, both required booleans; with rollback on, the service is rolled back to the last deployment that completed successfully). The requirement is that a deployment leaves IN_PROGRESS for a terminal rolloutState within a bounded time — FAILED with a rolloutStateReason attributing the failure to the circuit breaker — and, with rollback enabled, the PRIMARY deployment returns to the last completed revision with runningCount converging on desiredCount; the parallel ServiceDeployment record reaches a terminal status/statusReason the same way, whether the rollout ended by circuit-breaker rollback or by StopServiceDeployment.
Emulator: the step is a DescribeServices wait with no status assertion of its own (the replay records "status: expected , got None" and actual_status null — no response was ever accepted as final). The emulator keeps answering, but never with the deployment state the step waits for, so the step is retried until the fixture's entire budget is spent ("$: fixture budget 960s spent"); every later step of the trajectory, including its StopServiceDeployment comparison arm, is abandoned unreplayed. The exposed deployment stays non-terminal: it never becomes FAILED with a circuit-breaker rolloutStateReason and no rollback to the last completed revision is ever visible.
Why: a client deploying behind the deployment circuit breaker relies on ECS marking an unlaunchable rollout FAILED and restoring the last good revision, and on DescribeServices reporting that within a bounded time. Against the emulator such waiters hang until their own timeout and then report a false verdict, and automation can never tell "the circuit breaker rolled this back" from "an operator stopped the deployment".
Fix: the ECS service-deployment state machine in the ecs service crate — the progression driven by CreateService/UpdateService must evaluate the circuit breaker against failing task launches, drive the deployment to the terminal rolloutState FAILED with a rolloutStateReason naming the circuit breaker, make the prior completed revision PRIMARY again when rollback is enabled, and converge runningCount on desiredCount; the DescribeServices deployments projection must surface those fields so a poll can terminate. NOTE for the fixer: open issue 20261107T170944Z-ecs-circuit-breaker-deployment-never-reaches-a-terminal.md already carries this exact fixture/step as its replay key and describes this same cause; the replay nevertheless reported this cluster as unowned (owned_by empty), so it is carried here to keep the cluster covered — closing it closes that issue too, and no separate fix is needed. Behavioral issues 20261106T115645Z-ecs-3-net-new-causes-a-cluster-scoped.md (cause 2) and 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md (cause 3) describe the same gap but carry no wire replay key.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15 (DescribeServices).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, /__admin/health 200, data listener accepting). 96 divergence clusters reported; these are the two whose owned_by and stub_path are both empty. Both are service ecs, operation DescribeServices.

Cause 1 — divergence cluster service=ecs|DescribeServices|status=200->400|keys=$,$.failures,$.services,status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
  step_key:   task#16
  expected_status: 200; actual_status: 400
  detail: "status: expected 200, got 400" and "$: poll capped at 180s"
  Expected (AWS): 200 with failures: [] and services: [ the full Service element for the polled service — capacityProviderStrategy over the fixture's capacity provider, clusterArn, desiredCount 1, deploymentConfiguration {bakeTimeInMinutes, deploymentCircuitBreaker, maximumPercent, minimumHealthyPercent, strategy 'ROLLING'}, deploymentController {type 'ECS'}, currentServiceDeployment, currentServiceRevisions[], deployments[] with capacityProviderStrategy, rolloutState 'IN_PROGRESS', stabilityStatus 'STABILIZING', status 'PRIMARY', taskDefinition ].
  Actual (emulator): 400 with no services and no failures member at all; the poll is retried to its cap and the step fails.
  (The recorded ARNs, service/cluster/provider names, revision ids and createdAt timestamps are per-run values and are not requirements; the requirement is 200 carrying services/failures.)
  Sibling evidence for the hypothesis, from the same fixture and already owned by other issues: setup#8 CreateCluster 200->400, setup#9 DescribeClusters reports the cluster with reason 'MISSING', task#13 RunTask answers __type 'ClusterNotFoundException' / message 'Cluster not found.' where AWS answers InvalidParameterException.

Cause 2 — divergence cluster service=ecs|DescribeServices|status=0->None|keys=$,status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
  step_key:   task#15
  expected_status: 0 (wait step — no status asserted); actual_status: null
  detail: "status: expected , got None" and "$: fixture budget 960s spent"
  Expected (AWS): the poll terminates because the circuit-breaker deployment leaves IN_PROGRESS for a terminal rolloutState — FAILED with a rolloutStateReason attributing the failure to the deployment circuit breaker — and, with rollback enabled, the PRIMARY deployment returns to the last completed revision with runningCount == desiredCount. deployments[].rolloutState is constrained by the model to COMPLETED | FAILED | IN_PROGRESS.
  Actual (emulator): the awaited state never appears; no response is ever accepted as final and the fixture's whole budget is consumed, so this step and everything after it in the trajectory never complete.
  (The 180s poll cap and the 960s fixture budget are this corpus's limits, not requirements; the defects are the 400 on a read-only describe and the deployment that never becomes terminal.)

## Steps to Reproduce
Cause 1 — fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step task#16:
1. Build and boot the emulator from this checkout and replay that fixture from trace set simparity-wire-aws-v1.
2. By hand: create a launch template and a zero-capacity Auto Scaling group, ECS CreateCapacityProvider over that group, CreateCluster associating the provider with a defaultCapacityProviderStrategy, RegisterTaskDefinition, CreateService with a capacityProviderStrategy over the provider, then UpdateService to repair the strategy.
3. Poll DescribeServices with that cluster and service (short name and/or full ARN) for the service to show the repaired capacityProviderStrategy and its PRIMARY deployment.
   -> The emulator answers HTTP 400 with no services/failures, so the poll can never succeed and is retried to its cap. AWS answers 200 with the Service element, and reports any identifier it cannot resolve as a Failure {arn, reason 'MISSING'} inside that same 200 response.

Cause 2 — fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15:
1. Replay that fixture and observe step task#15.
2. By hand: CreateCluster; RegisterTaskDefinition for a FARGATE/awsvpc task definition with a launchable image (revision 1); CreateService desiredCount=1, launchType FARGATE, deploymentController ECS, deploymentConfiguration.deploymentCircuitBreaker {enable: true, rollback: true}; wait for a steady state.
3. RegisterTaskDefinition a second revision whose tasks cannot start (unpullable image) and UpdateService taskDefinition=<revision 2>.
4. Poll DescribeServices for the new deployment to reach rolloutState FAILED with a rolloutStateReason naming the deployment circuit breaker, then for the PRIMARY deployment to point back at revision 1 with runningCount == desiredCount.
   -> Against the emulator the poll never satisfies either condition: it keeps answering 200 with a non-terminal deployment until the caller's budget is exhausted (here the fixture's 960s), and the fixture's remaining steps (including its StopServiceDeployment comparison arm) never run.
5. Contrast arm: ListServiceDeployments / DescribeServiceDeployments for the same service -> the service deployment record never reaches a terminal status/statusReason either, so a circuit-breaker rollback cannot be told apart from a StopServiceDeployment'd rollout.
