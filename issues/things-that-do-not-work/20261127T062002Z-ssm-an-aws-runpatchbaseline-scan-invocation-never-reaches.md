Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.patch-group-state-server-computed-from-baseline
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.patch-group-state-server-computed-from-baseline|_scan_patches#9|TimeoutError|raise TimeoutError("AWS-RunPatchBaseline scan: the run budget ended first; not graded")
Issue-Commit: 6db8a8f1efcc2e7e74b5d619ee1408e650a718be

# ssm: an AWS-RunPatchBaseline Scan invocation never reaches a terminal CommandInvocationStatus, so no patch-compliance report is ever produced for a patch group

> _Note: this issue was identified against the code base at commit `6db8a8f1efcc2e7e74b5d619ee1408e650a718be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommandInvocation

## Description
Cause 1 of 1: a Run Command invocation of the AWS-RunPatchBaseline document (Operation=Scan) against an SSM-managed node never settles, so the Patch Manager read that depends on it (DescribePatchGroupState) is never reachable.

AWS: SendCommand with DocumentName="AWS-RunPatchBaseline" and Parameters={"Operation":["Scan"]} returns 200 with a Command, and GetCommandInvocation for that CommandId/InstanceId pair then advances the invocation through non-terminal states to a TERMINAL one within the normal run window. Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): GetCommandInvocation.Status targets the CommandInvocationStatus enum, and the output also carries StatusDetails, ResponseCode, ExecutionStartDateTime, ExecutionElapsedTime, ExecutionEndDateTime and StandardOutputContent — members that only have a value once the invocation has actually run. A scan terminating Success is what writes that node's instance patch-compliance report, and DescribePatchGroupState (modeled: "Returns high-level aggregated patch compliance state information for a patch group", required member Instances = "The number of managed nodes in the patch group") then counts the node whose "Patch Group" tag names the group, so Instances is 1 for a group with one scanned node and 0 for a group name that was never registered or tagged. Error wording is not at issue here: the recorded flow succeeds, there is no refusal to word.

Emulator: the invocation never reaches a terminal CommandInvocationStatus. The check sends the command, then polls GetCommandInvocation every 2 s against a budget of up to 480 s; the poll loop exits through its `else` branch, i.e. the deadline passed with the last observed Status still non-terminal (never any of Success, Failed, Cancelled, TimedOut), and the check consumes the whole remaining run budget and raises TimeoutError("AWS-RunPatchBaseline scan: the run budget ended first; not graded") from _scan_patches. Because the scan never settles, the DescribePatchGroupState assertions after it (Instances == 1 for the tagged group, Instances == 0 for an unregistered control group) are never exercised at all. Note that DescribePatchGroupState itself IS implemented and counts from crate::state::instance_patch_states; the gap is that nothing ever writes a report into that store for a SendCommand-driven AWS-RunPatchBaseline scan.

Why: a client that follows the documented Patch Manager flow — SendCommand AWS-RunPatchBaseline, then poll GetCommandInvocation (or use the SDK/CLI command-executed waiter) until the invocation is terminal — never returns. It hangs until its own timeout instead of seeing a terminal status, and every downstream patch read (DescribePatchGroupState, DescribeInstancePatchStates, DescribeInstancePatchStatesForPatchGroup) keeps reporting an empty group, so patch compliance can never be observed for a node that was scanned.

Fix: the Run Command invocation lifecycle for the AWS-RunPatchBaseline document — SendCommand's dispatch of that document to a managed node and the settle step GetCommandInvocation reads, plus the patch-compliance state that a finished scan owes (the instance_patch_states / patch_runs state DescribePatchGroupState already counts from). The handlers are send_command and get_command_invocation in the ssm service's ops module; file and line citations are under Traces.

Related open issues that do NOT cover this one (named per instruction, not as duplicates): 20261120T152132Z-ec2-ssm-a-patch-manager-scan-of-a.md is filed under service ec2__ssm and is about a scan that DOES settle but inventories nothing installed; 20261115T003554Z-cloudformation-ec2-ssm-ssm-run-command-on-a-cloudformation-provisioned-ec2.md and 20261116T054200Z-cloudwatch-ec2-ssm-ssm-run-command-invocations-on-nat-gateway-managed.md are filed under other service headings and describe invocations that reach Status 'Failed' with empty output, not invocations that never reach a terminal status. None of them carries this check's replay key, and the suite report did not mark this check OWNED or ALREADY FILED.

Traces: ssm.patch-group-state-server-computed-from-baseline. Component citations: services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_patch_group_state.rs:214-223 (the read that counts reports from crate::state::instance_patch_states and the settle calls it makes), and the SendCommand / GetCommandInvocation handlers under services/service-com-amazonaws-ssm-amazonssm/src/ops/.

## Evidence
Failing check (suite report, service heading `### ssm`):

  #### [error] ssm.patch-group-state-server-computed-from-baseline
  Error: File "check_ssm_patch_group_state_server_computed_from_baseline.py", line 53, in run
    _scan_patches(ctx, ssm, instance_id)
  File "check_ssm_patch_group_state_server_computed_from_baseline.py", line 89, in _scan_patches
    raise TimeoutError("AWS-RunPatchBaseline scan: the run budget ended first; not graded")
  TimeoutError: AWS-RunPatchBaseline scan: the run budget ended first; not graded

Expected (real AWS):
- SendCommand(InstanceIds=[<node>], DocumentName="AWS-RunPatchBaseline", Parameters={"Operation":["Scan"]}) -> 200 with Command.CommandId.
- GetCommandInvocation(CommandId=<that id>, InstanceId=<node>) -> 200 whose Status reaches a TERMINAL CommandInvocationStatus (Success for a healthy scan) well inside the 480 s poll window, with ExecutionStartDateTime/ExecutionEndDateTime set.
- DescribePatchGroupState(PatchGroup=<group the node is tagged into>) -> 200 with Instances == 1.
- DescribePatchGroupState(PatchGroup=<never-registered group name>) -> 200 with Instances == 0.

Actual (emulator):
- GetCommandInvocation is polled every 2 s for the full window and the last observed Status is still non-terminal (the loop never breaks on Success/Failed/Cancelled/TimedOut; it falls through to the timeout branch), so the invocation never settles.
- The two DescribePatchGroupState reads that follow are never reached, so the Instances==1 / Instances==0 pair is unverifiable.

The invariant part of this evidence is the shape, not the values: the group name, baseline id, instance id and the 480 s budget are per-run values. The requirement is that an AWS-RunPatchBaseline Scan invocation reaches a terminal CommandInvocationStatus and leaves a patch-compliance report that DescribePatchGroupState counts.

## Steps to Reproduce
1. ssm.CreatePatchBaseline(Name=<unique>, OperatingSystem="AMAZON_LINUX_2", ApprovalRules={"PatchRules":[{"PatchFilterGroup":{"PatchFilters":[{"Key":"PRODUCT","Values":["*"]}]},"ApproveAfterDays":7}]}) and keep the BaselineId.
2. ssm.RegisterPatchBaselineForPatchGroup(BaselineId=<id>, PatchGroup=<group>).
3. Launch an SSM-managed EC2 node (instance profile with AmazonSSMManagedInstanceCore) tagged {"Key":"Patch Group","Value":<group>}.
4. ssm.SendCommand(InstanceIds=[<instance-id>], DocumentName="AWS-RunPatchBaseline", Parameters={"Operation":["Scan"]}) and take Command.CommandId.
5. Poll ssm.GetCommandInvocation(CommandId=<command-id>, InstanceId=<instance-id>) every 2 s. On AWS the Status reaches Success; on the emulator it never reaches any terminal CommandInvocationStatus and the poll runs out its budget.
6. ssm.DescribePatchGroupState(PatchGroup=<group>) -> expect Instances == 1; and ssm.DescribePatchGroupState(PatchGroup=<a group name never registered or tagged>) -> expect Instances == 0. Step 5 never completes, so step 6 never reflects the scan.

Replay: run the behavioral suite and re-run check ssm.patch-group-state-server-computed-from-baseline.
