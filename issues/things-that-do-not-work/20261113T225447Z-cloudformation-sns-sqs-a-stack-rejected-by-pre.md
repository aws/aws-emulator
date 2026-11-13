Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.deploy-sns-subscription-missing-protocol-rejected
Issue-Key: cloudformation__sns__sqs.deploy-sns-subscription-missing-protocol-rejected|run#32|AssertionError|assert reason == _STACK_REASON, (
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# cloudformation__sns__sqs: a stack rejected by pre-deployment property validation reports the per-resource property error as the stack's CREATE_FAILED reason instead of AWS's 'Validation failed with N error(s)...' summary

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE net-new root cause of service `cloudformation__sns__sqs`, grounded in the single failing check in trace_ids. The check is not marked ALREADY FILED or OWNED in the replay report, and none of this service's open issues carries it.

=== Cause 1 of 1: the stack-level CREATE_FAILED ResourceStatusReason carries the resource-level property-validation text ===
AWS: recorded behavior (CloudFormation does not model stack-event text in Smithy). When a template fails pre-deployment property validation — here an `AWS::SNS::Subscription` with its @required `Protocol` property omitted — the stack reaches ROLLBACK_COMPLETE and the STACK's own CREATE_FAILED event carries one fixed summary sentence that counts the errors and points the caller at the detail: `Validation failed with <N> error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` The only variable part is the error count N (1 here). The per-property detail is reported SEPARATELY, as the resource's validation row (`Subscription`, `PROPERTY_VALIDATION`, `FAILED`, `FAIL`, `Required property [Protocol] not found`, `/Resources/Subscription/Properties`) — i.e. the two levels say different things on purpose, and the summary is what a caller polling DescribeStacks/StackEvents for the stack's failure reason reads first. No resource is provisioned (ListStackResources is empty and the template's topic does not exist).
Emulator: the stack's CREATE_FAILED reason is the resource-level sentence `Required property [Protocol] not found` — the summary sentence, and with it the error COUNT and the instruction to call DescribeEvents, is never produced at the stack level. (The stack does reach ROLLBACK_COMPLETE, so the validation itself happens; only the reporting level is wrong. The assertions after the reason — the validation rows, the empty resource list, the absent topic — were not reached.)
Why: a deployer (CLI, CDK, a CI gate, any `wait stack-create-complete` wrapper) reports the stack-level reason as THE failure, and clients key on that text to tell "this stack was rejected before provisioning, with N property errors to look up" from a mid-deploy resource failure. Collapsing the two levels loses the error count and makes the stack-level reason unparseable by anything written against AWS's summary, while a template with several bad properties reports only one of them.
Fix: the CloudFormation stack-provisioning path's pre-deployment property validation — record each property error as the resource's PROPERTY_VALIDATION row (as it already words it) and set the STACK's CREATE_FAILED ResourceStatusReason to the `Validation failed with <N> error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` summary with N = the number of property errors found. Related open issue that does NOT carry this check and describes a different defect (property-schema violations handed to the resource handler at all): 20261112T024755Z-cloudformation-mwaa-a-template-property-that-violates-the.md.
Traces: cloudformation__sns__sqs.deploy-sns-subscription-missing-protocol-rejected.

## Evidence
cloudformation__sns__sqs.deploy-sns-subscription-missing-protocol-rejected [mismatch]:
  File "check_cloudformation__sns__sqs_deploy_sns_subscription_missing_protocol_rejected.py", line 270, in run — assert reason == _STACK_REASON
  AssertionError: expected the stack's CREATE_FAILED reason to be 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.', got 'Required property [Protocol] not found'
The control arm (the same template WITH Protocol='sqs') reached CREATE_COMPLETE, subscribed the queue and attached the queue policy — so only the failure-reporting path diverges. The stack status assertion (ROLLBACK_COMPLETE) passed immediately before.

## Steps to Reproduce
1. CreateStack with a template containing an AWS::SNS::Topic, an AWS::SQS::Queue and an AWS::SNS::Subscription whose Properties give TopicArn and Endpoint but OMIT Protocol.
2. Wait for the terminal status — ROLLBACK_COMPLETE.
3. DescribeStackEvents and read the ResourceStatusReason of the CREATE_FAILED event whose LogicalResourceId is the STACK itself: AWS states 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.'; the emulator states 'Required property [Protocol] not found'.
4. Check that the per-resource validation row still carries ('Subscription', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Required property [Protocol] not found', '/Resources/Subscription/Properties'), that ListStackResources is empty and that the template's topic does not exist.
5. Re-replay cloudformation__sns__sqs.deploy-sns-subscription-missing-protocol-rejected.
