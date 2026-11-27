Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__ssm__sts.ssm-automation-execute-aws-api-reaches-ec2-iam-and-sts
Issue-Key: ec2__iam__ssm__sts.ssm-automation-execute-aws-api-reaches-ec2-iam-and-sts|run#9|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# ssm Automation aws:executeAwsApi binds only five EC2 operations, so a runbook step calling ec2:DescribeVpcs ends the execution Failed

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Cause 1 of 1: the SDK the Automation runtime exposes to the `aws:executeAwsApi` action binds an arbitrary five-operation subset of Amazon EC2, so any step naming another EC2 operation fails instead of calling it.

AWS: recorded behavior only for the runtime's SDK surface. `aws:executeAwsApi` can call ANY operation of the named service — Service=ec2, Api=DescribeVpcs is a plain read — and a runbook whose steps call ec2:DescribeVpcs, an IAM read and sts:GetCallerIdentity reaches AutomationExecutionStatus `Success`, with each step's outputs selected from the real response.
Emulator: the execution ends `Failed` with FailureMessage "The aws:executeAwsApi action of step <step name> could not call ec2:DescribeVpcs: the AWS SDK this runtime supplies binds no DescribeVpcs call on Amazon EC2; the operations it binds are DescribeSecurityGroups, AuthorizeSecurityGroupIngress, AuthorizeSecurityGroupEgress, RevokeSecurityGroupIngress, RevokeSecurityGroupEgress". The failure is a property of the runtime's SDK binding, not of the request: the step name is the request's own value, the five bound operations are a fixed emulator-side list.
Why: Automation is a general-purpose cross-service orchestrator, and this limits it to five security-group calls on EC2. Every runbook that reads any other EC2 resource — the standard pattern for discovery and for remediation steps that cross EC2, IAM and STS in one execution — fails, and it fails with a runtime-authored message that names the emulator's own SDK rather than anything a client did wrong, so the author cannot fix the runbook.
Fix: the ssm Automation executor's `aws:executeAwsApi` action — dispatch the step's Service/Api pair through the emulator's full service surface instead of a hand-written per-service call list, so any bound operation of the named service is callable.
Traces: ec2__iam__ssm__sts.ssm-automation-execute-aws-api-reaches-ec2-iam-and-sts.

## Evidence
Expected (AWS): AutomationExecutionStatus == 'Success' with the ec2:DescribeVpcs step's selected outputs.

Actual (emulator), verbatim from the replay: 'AssertionError: the automation ended \'Failed\': \'The aws:executeAwsApi action of step vpcs could not call ec2:DescribeVpcs: the AWS SDK this runtime supplies binds no DescribeVpcs call on Amazon EC2; the operations it binds are DescribeSecurityGroups, AuthorizeSecurityGroupIngress, AuthorizeSecurityGroupEgress, RevokeSecurityGroupIngress, RevokeSecurityGroupEgress\''

The step name ('vpcs') is the runbook's own; the five-operation list is the emulator's.

## Steps to Reproduce
1. Create an SSM Automation document whose steps use aws:executeAwsApi: one with Service=ec2, Api=DescribeVpcs; one with an IAM read; one with Service=sts, Api=GetCallerIdentity.
2. StartAutomationExecution on it and poll GetAutomationExecution to a terminal status.
3. Expect 'Success'; observe 'Failed' with the FailureMessage above naming ec2:DescribeVpcs as unbound by the runtime's SDK.
