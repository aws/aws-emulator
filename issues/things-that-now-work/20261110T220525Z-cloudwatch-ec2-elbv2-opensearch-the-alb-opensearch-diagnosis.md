Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config
Issue-Key: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config|phase#8|_PhaseError|raise _PhaseError(
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# cloudwatch__ec2__elbv2__opensearch: the ALB/OpenSearch diagnosis lifecycle dies in its deploy phase because DescribeStacks Outputs entries carry no OutputValue

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: DescribeStacks returns Outputs entries without the OutputValue member, so the deploy phase of a five-phase lifecycle cannot read the stack's published values and the whole scenario is abandoned at phase 1/5.
AWS: Smithy (cloudformation) DescribeStacks -> Stacks[].Outputs[] of shape Output {OutputKey, OutputValue, Description, ExportName}. Every output a template declares with a `Value:` expression is reported with both OutputKey and the resolved OutputValue once the stack is CREATE_COMPLETE. The resolved value is state/request derived (a target group ARN, a domain endpoint, a VPC id); its presence on every declared output is the requirement.
Emulator: the deploy phase raises a bare KeyError on 'OutputValue'. Replay: _PhaseError "[phase 1/5 deploy] KeyError: 'OutputValue'".
Why: the lifecycle's later phases (setup, solve, cleanup, destroy) all consume the deployed stack's outputs — target group ARNs to register stale addresses into, the OpenSearch domain's subnet, the alarm configuration to inspect. An output with no value makes a CREATE_COMPLETE stack unusable, and the failure reaches the client as a KeyError instead of as anything the API reported.
Fix: the CloudFormation stack-output component — the Outputs resolver and the DescribeStacks serializer must emit Output.OutputValue for every declared output of a stack in a terminal CREATE_COMPLETE/UPDATE_COMPLETE state.
Related open filing that does NOT carry this replay key: 20261109T153103Z-cloudformation-ec2-elbv2-2-net-new-causes-a.md reports the same missing-value defect under the cloudformation__ec2__elbv2 heading; the suite did not mark this check OWNED by it, so it survives as a live failure under this heading.
Traces: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[error] cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config
  File "contextlib.py", line 158, in __exit__ -> self.gen.throw(value)
  File "check_...e2e_diagnose_alb_opensearch_stale_targets_with_alarm_config.py", line 295, in phase -> raise _PhaseError(...)
  _PhaseError: [phase 1/5 deploy] KeyError: 'OutputValue'

The check deploys deployments/alb-opensearch-stale-targets/template.json and then reads its outputs to drive the setup/solve phases.
Expected (AWS): Stacks[0]['Outputs'] entries each carry 'OutputKey' and 'OutputValue'.
Actual (emulator): 'OutputValue' is absent; indexing it raises KeyError: 'OutputValue' and the lifecycle aborts at phase 1 of 5.

Smithy consulted (model services/service-com-amazonaws-cloudformation-cloudformation): DescribeStacks output Stacks[].Outputs[] -> Output {OutputKey, OutputValue, Description, ExportName}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Deploy a template that declares `Outputs` with `Value:` expressions (e.g. two target group ARNs, a VPC id, an OpenSearch domain endpoint, an alarm name).
3. Wait for CREATE_COMPLETE, then cloudformation.describe_stacks(StackName=<stack id>) and read Stacks[0]['Outputs'].
4. Expected: each entry carries 'OutputValue'. Actual: the member is missing, so a consumer raises KeyError: 'OutputValue'.
5. Re-replay the id in trace_ids to confirm the fix.
