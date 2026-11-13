Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#12|AssertionError|assert broken_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# A CloudFormation stack whose ELBv2 listener rules collide on priority reaches CREATE_COMPLETE instead of rolling back (PriorityInUse never raised)

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'cloudformation__elbv2'. The already-open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md describes a related rollback symptom for the 'cloudformation' token and does not carry this replay key.

Cause 1 of 1: a duplicate listener-rule priority is accepted, so the resource never fails and the stack never rolls back.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateRule(ListenerArn required, Conditions required, Priority required @range 1..50000, Actions required) models PriorityInUseException with awsQuery error code 'PriorityInUse' and HTTP 400 — creating a second rule on the same listener at a priority already in use is refused. CloudFormation propagates that resource failure: the stack goes CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE. Recorded behavior only for the terminal status string, which the check asserts as 'ROLLBACK_COMPLETE'.
Emulator: the stack declaring two AWS::ElasticLoadBalancingV2::ListenerRule resources at the same Priority ends in status 'CREATE_COMPLETE'.
Why: a template with a genuine configuration conflict is reported as a successful deployment, so a client's CI gate passes on a stack whose routing rules are not what the template describes, and nothing signals the PriorityInUse conflict.
Fix: the ELBv2 CreateRule handler's priority-uniqueness validation (it must raise PriorityInUse/400) and the CloudFormation resource-failure -> rollback state machine that must propagate it.
Traces: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack.

## Evidence
cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack — AssertionError: a stack whose rule collides on priority must end ROLLBACK_COMPLETE, got 'CREATE_COMPLETE' (check line 204).
Smithy citation: CreateRule errors include PriorityInUseException, awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}.

## Steps to Reproduce
1. Boot the emulator.
2. Deploy a CloudFormation template with an ALB, an HTTP listener, and two AWS::ElasticLoadBalancingV2::ListenerRule resources on that listener both using Priority: 10.
3. Poll describe_stacks for the terminal status: it is CREATE_COMPLETE; AWS reaches ROLLBACK_COMPLETE because the second CreateRule is refused with PriorityInUse (HTTP 400).
4. Re-replay cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack.
