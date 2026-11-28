Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility|run#15|AssertionError|assert awsvpc_compat == {"EC2", "FARGATE", "MANAGED_INSTANCES"}, (
Issue-Commit: 51580daa099a6a502e147b76b77645918dba2576

# ECS task-definition compatibilities can never derive MANAGED_INSTANCES: an awsvpc revision with no requiresCompatibilities reports only [EC2, FARGATE]

> _Note: this issue was identified against the code base at commit `51580daa099a6a502e147b76b77645918dba2576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
1 cause.

Cause 1 of 1: the derived `TaskDefinition$compatibilities` list can no longer contain `MANAGED_INSTANCES` for ANY revision, so an `awsvpc` revision that pins no `requiresCompatibilities` is reported as compatible with only two launch types instead of three. Failing check: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility.

AWS: `TaskDefinition$compatibilities` is DERIVED by the service from the stored revision's own parameters, not echoed from the request. Smithy: `com.amazonaws.ecs#TaskDefinition$compatibilities` -> `com.amazonaws.ecs#CompatibilityList` -> `com.amazonaws.ecs#Compatibility`, whose value set is [EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES]; the member documents only "Amazon ECS validates the task definition parameters with those supported by the launch type", so the derivation RULE itself is recorded behavior only. The recorded behavior for this check (RegisterTaskDefinition + DescribeTaskDefinition, HTTP 200 both) is: a revision registered with NO `requiresCompatibilities`, NO `networkMode` and task-level cpu/memory derives exactly {EC2, EXTERNAL} (bridge networking rules FARGATE and managed instances out), while the IDENTICAL definition registered with `networkMode=awsvpc` derives exactly {EC2, FARGATE, MANAGED_INSTANCES} — awsvpc networking admits both Fargate and ECS Managed Instances capacity and rules EXTERNAL out. MANAGED_INSTANCES is therefore a value the derived list genuinely carries for an unconstrained awsvpc revision; it is not merely a function of `networkMode == awsvpc` either (cf. the fixed issue below, whose awsvpc revision pinned `requiresCompatibilities=["FARGATE"]` and derived only {EC2, FARGATE}).

Emulator: the same two calls return HTTP 200 with `compatibilities` = ['EC2', 'FARGATE'] for the awsvpc revision — the MANAGED_INSTANCES entry is absent, and the list is one element short rather than mis-ordered. Suite text: `AssertionError: an awsvpc definition must derive EC2-, FARGATE- and MANAGED_INSTANCES-compatible, got ['EC2', 'FARGATE']`. The check's earlier assertions pass (the omitted `networkMode` stays omitted on read, and the plain revision derives exactly {EC2, EXTERNAL}), which isolates the defect to the MANAGED_INSTANCES member of the derivation for awsvpc revisions. The cause is structural, not conditional: the candidate set the projection filters, `COMPATIBILITY_ORDER`, has been reduced to a 3-element array ["EXTERNAL", "EC2", "FARGATE"], so no revision of any shape can ever report MANAGED_INSTANCES.

Related, but not covering this check: the FIXED issue 20261025T062345Z-ecs-describetaskdefinition-registertaskdefinition-over-reports-compatibilities-managed-instances.md asked for MANAGED_INSTANCES to be removed from a Fargate-PINNED awsvpc revision, and the open issues 20261120T233305Z-ecs-servicediscovery-ecs-describetaskdefinition-reports-managed-instances-in-the-derived.md, 20261122T202428Z-ecs-deregister-and-delete-task-definition-responses-render.md and 20261124T015542Z-ecs-2-net-new-causes-a-draining-ec2.md ask for it to be removed from BRIDGE-mode revisions. Those fixes over-corrected by deleting the launch type from the candidate set instead of classifying it per revision, which is the defect reported here; the open wire issue 20261120T005727Z-ecs-task-definition-records-render-wrong-compatibilities-volumes.md asks for MANAGED_INSTANCES to be ADDED on its own awsvpc fixtures but carries no key for this check.

Why: `compatibilities` is what a client (console, CDK, deployment tooling) reads to decide which launch types a revision may be placed with. A client that offers or auto-selects a launch type from this list concludes an unconstrained awsvpc revision cannot run on ECS Managed Instances capacity and refuses to deploy it, or silently picks a different launch type than AWS would allow. Because the same derived list is reused as the launch-type gate for placement, the emulator additionally REFUSES RunTask/CreateService with `launchType: MANAGED_INSTANCES` (or a managed-instances capacity provider) for revisions real ECS admits, so the under-report changes behavior and not just the rendered payload.

Fix: the task-definition projection's `compatibilities` derivation in the ECS service state — `MANAGED_INSTANCES` must be restored to the candidate launch types and given its own per-revision predicate alongside the FARGATE and EXTERNAL arms (admitted for an awsvpc revision, excluded for bridge/host/none and for a revision whose own parameters rule it out), rather than being deleted from the candidate set so that every revision excludes it. The launch-type validation helper that shares the function then stops refusing the launch type too. The reporting ORDER must keep the position recorded AWS responses carry, so a positional reader reads the same launch type.

Traces: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility. Code: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs — `COMPATIBILITY_ORDER` at line 1510 (`pub const COMPATIBILITY_ORDER: [&str; 3] = ["EXTERNAL", "EC2", LAUNCH_TYPE_FARGATE];`) and its doc comment at lines 1495-1509 asserting "a revision reports nothing about it"; `LAUNCH_TYPE_MANAGED_INSTANCES` at line 1572; `TaskDefinitionRecord::compatibilities` (which filters that array) and the placement gate that shares it; reached from services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs and .../src/ops/describe_task_definition.rs via the record's view projection.

## Evidence
Check: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility (verdict: mismatch).

Request A (RegisterTaskDefinition): family=<unique>, cpu="256", memory="512", tags=[{key:"sim2real",value:"ecs-fargate-netmode"}], containerDefinitions=[{name:"app", image:"public.ecr.aws/docker/library/nginx:1.27", essential:true}] — no networkMode, no requiresCompatibilities.
Request B (RegisterTaskDefinition): identical, plus networkMode="awsvpc".
Then DescribeTaskDefinition on each returned taskDefinitionArn.

Expected (recorded real AWS): HTTP 200 for both. Revision A: `networkMode` absent on read and `compatibilities` == {EC2, EXTERNAL}. Revision B (awsvpc): `compatibilities` == {EC2, FARGATE, MANAGED_INSTANCES}.

Actual (emulator): HTTP 200 for both. Revision A matches ({EC2, EXTERNAL}, networkMode absent — these assertions pass). Revision B returns `compatibilities` == ['EC2', 'FARGATE']; the MANAGED_INSTANCES element is absent entirely, so the list is one element SHORT (a fix that only re-orders the list will not close this).

Suite failure text:
  File "run.py", line 456, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_ecs_fargate_task_definition_with_network_mode_omitted_derives_no_fargate_compatibility.py", line 45, in run
    assert awsvpc_compat == {"EC2", "FARGATE", "MANAGED_INSTANCES"}, (
  AssertionError: an awsvpc definition must derive EC2-, FARGATE- and MANAGED_INSTANCES-compatible, got ['EC2', 'FARGATE']

The family names, the generated container name and the image tag above are the recorded run's values filling the template; the requirement is the CONTENTS of the derived list for an awsvpc revision that pins no requiresCompatibilities, not those literals.

Static confirmation that no revision can satisfy it: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs line 1510 declares the candidate set as `[&str; 3] = ["EXTERNAL", "EC2", LAUNCH_TYPE_FARGATE]`, with MANAGED_INSTANCES excluded by construction (doc comment lines 1504-1506: "a revision reports nothing about it").

## Steps to Reproduce
1. Boot the emulator and point an ecs client at it.
2. RegisterTaskDefinition with family=<F1>, cpu="256", memory="512", containerDefinitions=[{name:"app", image:"public.ecr.aws/docker/library/nginx:1.27", essential:true}] and NO networkMode and NO requiresCompatibilities.
3. RegisterTaskDefinition the identical definition under family=<F2> but with networkMode="awsvpc".
4. DescribeTaskDefinition on both returned taskDefinitionArns and read taskDefinition.compatibilities.
5. Observe <F1> -> {EC2, EXTERNAL} (correct) and <F2> -> ['EC2','FARGATE']; real AWS derives {EC2, FARGATE, MANAGED_INSTANCES} for the awsvpc revision.
6. Secondary effect: RunTask/CreateService naming launchType="MANAGED_INSTANCES" (or a managed-instances capacity provider) against <F2> is refused by the emulator because the launch-type gate reads the same derived list.
7. Re-replay check ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility to confirm the fix — and re-replay the bridge-mode checks named in the related issues to confirm MANAGED_INSTANCES is classified per revision rather than reinstated unconditionally.
