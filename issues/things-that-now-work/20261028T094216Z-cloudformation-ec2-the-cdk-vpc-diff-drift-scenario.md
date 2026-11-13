Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift
Issue-Key: cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift|_delete_stack_and_wait#16|Timeout|time.sleep(10.0)
Issue-Commit: 66edebc70f4e641d0e6ce34332328f72c099e422

# cloudformation + ec2: the CDK VPC diff/drift scenario never produces a terminal result — deploy and DetectStackDrift/DescribeStackResourceDrifts hang until the check is killed

> _Note: this issue was identified against the code base at commit `66edebc70f4e641d0e6ce34332328f72c099e422`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
One net-new root cause of the cloudformation+ec2 pair, grounded in one failing behavioral check.

Cause 1 of 1 — the CDK-deployed VPC stack scenario that exercises template-diff vs resource-drift never settles: the check produces no terminal outcome and is killed by the harness at 60s, with no response for the deploy/drift sequence.

AWS: recorded behavior only (there is no Smithy constraint that governs settling time). The scenario is: `cdk bootstrap` a toolkit stack, `cdk deploy` an app declaring a VPC with max_azs=2 and a single PUBLIC subnet group (no NAT gateway), then (a) `cdk diff --fail --no-change-set` exits 0 and prints `Number of stacks with differences: 0` on the unmodified app; (b) after ec2:ModifySubnetAttribute flips MapPublicIpOnLaunch to False on one public subnet out of band, the same `cdk diff` still exits 0 — it compares the local synthesized template with the template CloudFormation holds, and neither changed; (c) DetectStackDrift followed by DescribeStackResourceDrifts reports StackDriftStatus `DRIFTED`, exactly one entry for the mutated subnet with StackResourceDriftStatus `MODIFIED` and exactly one PropertyDifferences entry with PropertyPath `/MapPublicIpOnLaunch`, ExpectedValue true, ActualValue false and DifferenceType `NOT_EQUAL`; after the attribute is restored the same subnet reports `IN_SYNC`; (d) a changed local tag value makes `cdk diff --fail` exit non-zero, naming the new value, without mutating the template CloudFormation holds. Every leg returns within its own budget (bootstrap 300s, deploy 480s, each diff 120s, each drift detection 300s), which is why the suite can replay it at all.

Emulator: the check yields no verdict whatsoever — the only output is `Check timed out after 60.0s`. Nothing in the sequence (the bootstrap stack, the VPC/subnet stack create, the GetTemplate the diff issues, or the DetectStackDrift / DescribeStackResourceDrifts poll) ever reaches a terminal response inside the suite's budget, so none of the four arms above is ever evaluated. Hypothesis: a stack carrying EC2 networking resources is left non-terminal (never reaching CREATE_COMPLETE / CREATE_FAILED) and/or a drift detection id never leaves DETECTION_IN_PROGRESS, so every poll loop in the scenario spins forever rather than being refused or answered. The neighbouring failures on this same CDK VPC app are refusals, not hangs — the sibling check cloudformation__ec2.cdk-destroy-removes-every-resource-the-stack-created is already filed against UNIMPLEMENTED `DescribeInternetGateways` (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_internet_gateways.rs) and cloudformation__ec2.cdk-vpc-from-lookup-... against UNIMPLEMENTED `DescribeNatGateways` — so those scaffold stubs do not explain this one: a NotImplemented refusal fails fast, it does not hang.

Why: a client gets neither a success nor an error. Every wait-for-stable and wait-for-drift loop a deploy tool runs (the CDK CLI, `aws cloudformation wait`, Terraform's CFN provider) blocks indefinitely instead of being told the stack failed, so the caller cannot distinguish "still working" from "will never finish" and has no error code to branch on. Drift detection — the one API that is supposed to see an out-of-band resource mutation — is unusable, so operators lose the ability to tell a template change from resource drift.

Fix: the cloudformation crate's stack-settling path for stacks containing EC2 networking resource types (AWS::EC2::VPC / Subnet / InternetGateway / RouteTable), which must drive every resource to a terminal status and surface a terminal stack status even when a dependency is unavailable; and the drift-detection handlers (DetectStackDrift / DescribeStackDriftDetectionStatus / DescribeStackResourceDrifts), which must settle to DETECTION_COMPLETE and emit per-resource PropertyDifferences rather than never completing.

Traces: cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift

## Evidence
Cause 1 — cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift [mismatch]:

  Error: Check timed out after 60.0s

No status code, no error code and no assertion message were produced: the check never got far enough to assert anything. Its own per-leg budgets are far larger than the 60s it consumed (_BOOTSTRAP_TIMEOUT_S=300.0, _DEPLOY_TIMEOUT_S=480.0, _DIFF_TIMEOUT_S=120.0, _DRIFT_TIMEOUT_S=300.0, _TEARDOWN_DEADLINE_S=480.0), so the emulator did not answer any leg promptly enough for even the first arm to be reached.

Expected (from the check's own assertions, none of which ran):
  - arm 1: `cdk diff --fail --no-change-set` rc == 0 and output contains 'Number of stacks with differences: 0'
  - arm 2: after ec2.modify_subnet_attribute(SubnetId=<public subnet>, MapPublicIpOnLaunch={'Value': False}) and DescribeSubnets confirming the flip, the same diff still rc == 0 with the same marker
  - arm 3: DetectStackDrift -> StackDriftStatus == 'DRIFTED'; DescribeStackResourceDrifts -> exactly 1 entry with PhysicalResourceId == <subnet id>, StackResourceDriftStatus == 'MODIFIED', exactly one PropertyDifferences entry with PropertyPath == '/MapPublicIpOnLaunch', str(ExpectedValue).lower() == 'true', str(ActualValue).lower() == 'false', DifferenceType == 'NOT_EQUAL'; after restoring the attribute, StackResourceDriftStatus == 'IN_SYNC'
  - arm 4: with only a local tag value changed, `cdk diff --fail` rc != 0 and the output names the new value; GetTemplate for the deployed stack is byte-identical before and after (a diff is a read and must not mutate the stack)

Actual: no response at all — the emulator never reached a terminal state for the scenario and the harness killed the check.

Note on attribution: the two sibling checks that deploy the same CDK VPC app fail with fast refusals already filed for you (UNIMPLEMENTED `CreateInternetGateway` / `DescribeInternetGateways` / `DescribeNatGateways` in services/service-com-amazonaws-ec2-amazonec2/src/ops/), which is a different symptom class: a NotImplemented error terminates immediately. This check's symptom is the absence of any terminal response, so it is recorded separately.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift.
2. Narrower repro without the CDK CLI: CreateStack with a template declaring AWS::EC2::VPC plus two AWS::EC2::Subnet (MapPublicIpOnLaunch: true, one per AZ from DescribeAvailabilityZones) and an AWS::EC2::InternetGateway with its attachment. Then poll DescribeStacks. Expected: the stack reaches a TERMINAL StackStatus (CREATE_COMPLETE, or CREATE_FAILED/ROLLBACK_COMPLETE with a ResourceStatusReason) within the deploy budget. Observed: no terminal status is reached.
3. Once the stack is terminal: ec2.ModifySubnetAttribute(SubnetId=<one public subnet>, MapPublicIpOnLaunch={'Value': False}) and confirm via DescribeSubnets that MapPublicIpOnLaunch is false.
4. Call DetectStackDrift(StackName=<stack>), then poll DescribeStackDriftDetectionStatus(StackDriftDetectionId=<id>). Expected: DetectionStatus settles to DETECTION_COMPLETE with StackDriftStatus 'DRIFTED' within the drift budget. Observed: the poll never settles.
5. Call DescribeStackResourceDrifts(StackName=<stack>). Expected: exactly one entry for the mutated subnet with StackResourceDriftStatus 'MODIFIED' and one PropertyDifferences entry {PropertyPath: '/MapPublicIpOnLaunch', ExpectedValue: true, ActualValue: false, DifferenceType: 'NOT_EQUAL'}.
6. Restore MapPublicIpOnLaunch to true, re-run DetectStackDrift and DescribeStackResourceDrifts. Expected: the same subnet reports 'IN_SYNC' (a target that reports MODIFIED here is reporting drift unconditionally).
7. Call GetTemplate before and after a read-only diff/change-set-free comparison. Expected: byte-identical templates — a diff must not mutate the stack.
