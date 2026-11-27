Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__sns__sqs.deploy-subscription-protocol-change-under-replace-deny-policy-rejected
Issue-Key: cloudformation__iam__lambda__sns__sqs.deploy-subscription-protocol-change-under-replace-deny-policy-rejected|run#31|AssertionError|assert summary == [("Link", "Modify", "True")], f"protocol plan: got {summary!r}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# Changing AWS::SNS::Subscription Protocol is classified Modify instead of a replacement, so a stack policy denying Update:Replace is never consulted

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One cause.

Cause 1 of 1: AWS::SNS::Subscription's create-only `Protocol` property is classified as an in-place modification, so the change set reports Replacement=False and a stack policy that denies `Update:Replace` on the resource does not block the update.

AWS: `Protocol` is create-only on AWS::SNS::Subscription (SNS has no API to change a subscription's protocol — Smithy: com.amazonaws.sns has Subscribe/Unsubscribe and SetSubscriptionAttributes, and SetSubscriptionAttributes' attributeName does not include Protocol). CloudFormation therefore classifies a Protocol change as `Action=Modify, Replacement=True` with the change detail's `RequiresRecreation=Always`, and a stack policy statement with `Effect: Deny, Action: Update:Replace` on that logical id makes the update fail with the stack-policy denial instead of replacing the subscription.

Emulator: the change set lists the subscription as `('Link', 'Modify', 'False')` — Replacement=False — so the resource is treated as updatable in place and the Deny-on-Replace statement is bypassed entirely.

Why: a client that protects a resource with a Deny on `Update:Replace` is silently unprotected: the operation it asked CloudFormation to refuse proceeds, and the endpoint/protocol of a live subscription changes without the plan ever disclosing a replacement.

Fix: the AWS::SNS::Subscription resource schema's create-only property set and the change-set classifier that derives Replacement/RequiresRecreation from it, then the stack-policy evaluation that keys off `Update:Replace`.

The open issue 20261109T185037Z-cloudformation-sns-sqs-6-net-new-causes-custom-resource-responseurl-points.md reports the same misclassification for the subscription's `Endpoint` property under the cloudformation__sns__sqs heading; it does not carry this check's replay key, so this is filed separately.

Traces: cloudformation__iam__lambda__sns__sqs.deploy-subscription-protocol-change-under-replace-deny-policy-rejected.

## Evidence
Expected: the change set lists the subscription with Action=Modify and Replacement=True (change detail RequiresRecreation=Always), and executing it under a stack policy denying `Update:Replace` fails the update with the stack-policy denial.
Actual: `AssertionError: protocol plan: got [('Link', 'Modify', 'False')]` — (LogicalResourceId, Action, Replacement) with Replacement 'False'.

## Steps to Reproduce
1. Deploy a stack with an AWS::SNS::Subscription (Protocol: sqs) and a stack policy that denies `Update:Replace` on that logical id.
2. Create a change set that changes only the subscription's Protocol.
3. DescribeChangeSet: the row reads Action=Modify, Replacement=False instead of Replacement=True, and executing it is not blocked by the stack policy.
Replay: cloudformation__iam__lambda__sns__sqs.deploy-subscription-protocol-change-under-replace-deny-policy-rejected.
