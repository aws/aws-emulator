Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#55|AssertionError|assert (det["DetectionStatus"], det.get("StackDriftStatus")) == ("DETECTION_COMPLETE", "IN_SYNC"), (
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# cloudformation__elbv2: a freshly deployed ELBv2 stack detects DRIFTED (every resource counted as drifted) instead of IN_SYNC, so the out-of-band drift contract cannot be read

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
One net-new root cause, grounded in 1 failing behavioral check the replay report lists under `### cloudformation__elbv2` and does not mark ALREADY FILED or OWNED.

Related issues that do NOT carry this check's replay key, and so do not cover it: 20261029T171940Z-cloudformation-drift-reporting-returns-live-resource-attributes-and.md (open — drift never records a status at all, the opposite symptom), 20261106T115216Z-cloudformation-drift-detection-reports-an-existing-out-of.md (open — a MODIFIED DynamoDB table reported DELETED), 20261109T172142Z-cloudformation-dynamodb-5-net-new-causes-fresh-stacks.md (open — the same false-DRIFTED symptom for DynamoDB resources, a different service and a different resource-handler comparison) and 20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md (fixed — the CREATE-side priority collision, which this check now gets past: the colliding stack rolls back, CreateRule refuses the occupied priority with PriorityInUse, and the control stack's ALB serves all three routed responses before the drift phase is reached).

Cause 1 of 1: DetectStackDrift on a stack that has just been deployed and never touched outside CloudFormation reports StackDriftStatus DRIFTED.

AWS: Smithy com.amazonaws.cloudformation#DetectStackDrift starts an asynchronous detection and returns a StackDriftDetectionId; DescribeStackDriftDetectionStatus then reports DetectionStatus and, once DETECTION_COMPLETE, StackDriftStatus plus DriftedStackResourceCount. The model does not state the verdict, so recorded behavior only: for a stack whose resources have only ever been created and updated by CloudFormation, detection completes with DetectionStatus 'DETECTION_COMPLETE', StackDriftStatus 'IN_SYNC' and DriftedStackResourceCount 0 — a resource matches its template-declared properties until something changes it outside the stack. Only after an out-of-band change (here ModifyLoadBalancerAttributes setting idle_timeout.timeout_seconds away from the template's value) does the same call report DRIFTED, with exactly the load balancer's logical id MODIFIED, ResourceType AWS::ElasticLoadBalancingV2::LoadBalancer, a PropertyDifference under /LoadBalancerAttributes whose DifferenceType is NOT_EQUAL with the template value as ExpectedValue and the live value as ActualValue, and ActualProperties carrying the live attribute.

Emulator: detection completes but inverts the baseline — DescribeStackDriftDetectionStatus answers {'DetectionStatus': 'DETECTION_COMPLETE', 'StackDriftStatus': 'DRIFTED', 'DriftedStackResourceCount': 10} for the freshly deployed, untouched stack, counting every resource in it as drifted. Because the baseline is already DRIFTED, the subsequent out-of-band-change phase of the contract is unreadable: a DRIFTED verdict after the change is indistinguishable from the DRIFTED verdict before it, and the per-resource expectation (exactly the ALB MODIFIED, with an idle-timeout PropertyDifference) can never be isolated. The drifted count being the stack's whole resource count is the shape of the defect; the number itself is a per-template value.

Why: drift detection exists to answer 'has anything changed outside my stack?'. A client that gates a deploy, a compliance report or a remediation run on IN_SYNC gets a permanent false positive: every stack looks drifted from the moment it is created, so the signal carries no information and either blocks every pipeline or trains operators to ignore it — and a real out-of-band change is then invisible against the noise.

Fix: CloudFormation's drift-detection comparison and the state it reads — the per-resource comparison that builds StackResourceDrift must compare the resource's live properties against the template-declared properties of the resources CloudFormation actually provisioned (defaulted and read-only properties must not count as differences), and the stack-level roll-up must report IN_SYNC with DriftedStackResourceCount 0 when no resource differs. For the ELBv2 resource types in this stack the comparison must be able to read live LoadBalancer attributes, so this cause also depends on CloudFormation provisioning AWS::ElasticLoadBalancingV2::* through the ELBv2 API rather than synthesising identifiers for them.

Traces: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack.

## Evidence
Replay report, service cloudformation__elbv2:
  [mismatch] cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
  check_cloudformation__elbv2_deploy_elbv2_2_duplicate_rule_priority_rejected_rolls_back_stack.py:306
  assert (det["DetectionStatus"], det.get("StackDriftStatus")) == ("DETECTION_COMPLETE", "IN_SYNC")
  AssertionError: a freshly deployed stack must detect IN_SYNC, got
    {'DetectionStatus': 'DETECTION_COMPLETE', 'StackDriftStatus': 'DRIFTED', 'DriftedStackResourceCount': 10}

Expected (AWS), for the control stack that has only ever been provisioned by CloudFormation:
  DetectStackDrift -> StackDriftDetectionId; DescribeStackDriftDetectionStatus polled to a non-DETECTION_IN_PROGRESS value yields
  DetectionStatus 'DETECTION_COMPLETE', StackDriftStatus 'IN_SYNC', DriftedStackResourceCount 0.
Actual (emulator):
  DetectionStatus 'DETECTION_COMPLETE', StackDriftStatus 'DRIFTED', DriftedStackResourceCount equal to the stack's whole resource count (10 for this template).

The assertions before line 306 passed, so the failure is isolated to the drift baseline: the duplicate-priority stack rolled back, CreateRule at the occupied priority was refused with PriorityInUse carrying the message CloudFormation reported, the rejected CreateRule left the listener's rule priorities unchanged, and the control stack's ALB served 200 for the FirstRule path, 200 'other' for the SecondRule path and 404 'no-match' for an unrouted path.

The second half of the same contract, which this baseline makes unreadable, is:
  after ModifyLoadBalancerAttributes changes idle_timeout.timeout_seconds outside the stack, detection must report DRIFTED with exactly the ALB's logical id MODIFIED, ResourceType 'AWS::ElasticLoadBalancingV2::LoadBalancer', one PropertyDifference under /LoadBalancerAttributes with DifferenceType 'NOT_EQUAL' (ExpectedValue = the template's idle timeout, ActualValue = the live one), and ActualProperties.LoadBalancerAttributes showing the live idle timeout.

## Steps to Reproduce
1. Boot the emulator.
2. create_stack with a self-contained ELBv2 template: VPC + IGW + two public subnets + security group, an internet-facing application load balancer declaring LoadBalancerAttributes with an explicit idle_timeout.timeout_seconds, an HTTP:80 listener with a fixed-response default (404 'no-match'), and two AWS::ElasticLoadBalancingV2::ListenerRule resources at distinct priorities with fixed-response actions. Wait for CREATE_COMPLETE. Make no change of any kind outside CloudFormation.
3. Call detect_stack_drift(StackName=<stack>) and poll describe_stack_drift_detection_status(StackDriftDetectionId=...) until DetectionStatus leaves DETECTION_IN_PROGRESS.
4. Observe StackDriftStatus 'DRIFTED' with DriftedStackResourceCount equal to the number of resources in the stack; AWS reports 'IN_SYNC' with DriftedStackResourceCount 0 for an untouched stack.
5. Then make one out-of-band change — modify_load_balancer_attributes setting idle_timeout.timeout_seconds to a different value than the template declares — and repeat step 3: AWS now reports DRIFTED, and describe_stack_resource_drifts shows exactly the load balancer MODIFIED with a NOT_EQUAL PropertyDifference under /LoadBalancerAttributes. Confirm the emulator can distinguish this from step 4.
6. Re-replay cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack.
