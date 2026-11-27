Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns.deploy-sns-topic-declared-topic-arn-is-ignored-and-getatt-serves-the-minted-arn cloudformation__sns.deploy-topic-policy-id-is-service-assigned-even-when-the-template-sets-it cloudformation__sns.deploy-sns-topic-inline-policy-adds-no-system-tags-and-stripping-them-rejected cloudformation__sns.deploy-topic-inline-policy-tags-property-rejected cloudformation__sns.deploy-topic-system-tag-edits-rejected-while-a-template-tag-untags
Issue-Key: cloudformation__sns.deploy-sns-topic-declared-topic-arn-is-ignored-and-getatt-serves-the-minted-arn|run#13|AssertionError|assert status == "CREATE_COMPLETE", f"stack ended {status!r}, expected CREATE_COMPLETE"
Issue-Key: cloudformation__sns.deploy-topic-policy-id-is-service-assigned-even-when-the-template-sets-it|run#21|AssertionError|assert status == "CREATE_COMPLETE", f"Id in template: expected CREATE_COMPLETE, got {status!r}"
Issue-Key: cloudformation__sns.deploy-sns-topic-inline-policy-adds-no-system-tags-and-stripping-them-rejected|run#15|AssertionError|assert physical == outside_arn, f"the inline policy's physical id is {physical!r}, expected its topic ARN"
Issue-Key: cloudformation__sns.deploy-topic-inline-policy-tags-property-rejected|run#18|AssertionError|assert status == "ROLLBACK_COMPLETE", f"tagged inline policy: expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__sns.deploy-topic-system-tag-edits-rejected-while-a-template-tag-untags|run#15|AssertionError|assert got == system_refusal, f"untagging aws:cloudformation:stack-name gave {got!r}"
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + sns: 4 net-new causes — declared read-only properties roll stacks back, TopicInlinePolicy's physical id is not the topic ARN, an unsupported Tags property is accepted, and UntagResource's system-tag refusal names the wrong parameter

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Four net-new causes in CloudFormation's SNS surface.

Cause 1 of 4: a template that supplies a value for a READ-ONLY property rolls the stack back instead of ignoring it.
AWS: TopicArn on AWS::SNS::Topic and Id on AWS::SNS::TopicPolicy are read-only/service-assigned; a template that sets them is accepted, the declared value is discarded and the stack reaches CREATE_COMPLETE with Fn::GetAtt resolving to the minted value (recorded behaviour only).
Emulator: both stacks settle ROLLBACK_COMPLETE.
Why: templates emitted by CDK/SAM or round-tripped out of a stack read commonly carry read-only members; against the emulator such a deploy fails outright, and a caller cannot tell a genuine template error from a harmless one.
Fix: the input projection of the AWS::SNS::Topic and AWS::SNS::TopicPolicy handlers — strip read-only members before validation.

Cause 2 of 4: the PhysicalResourceId (and Ref) of AWS::SNS::TopicInlinePolicy is a generated CloudFormation id instead of the topic ARN.
AWS: the resource's physical id is the ARN of the topic the policy is attached to, so DescribeStackResource and Ref both yield `arn:<partition>:sns:<region>:<account>:<topic-name>`.
Emulator: the physical id is `<stack-name>-<LogicalId>-<generated suffix>`, which names no SNS resource.
Why: Ref on the inline policy is how a template passes the topic on to another resource; an id that is not the topic ARN makes every such reference invalid. A related open issue, 20261109T223435Z-cloudformation-sns-6-net-new-causes-topicinlinepolicy-ref.md, carries the Ref check only; this physical-id check is not covered by it.
Fix: the AWS::SNS::TopicInlinePolicy handler's create/read path — return the topic ARN as the physical resource id.

Cause 3 of 4: a `Tags` property on AWS::SNS::TopicInlinePolicy is accepted.
AWS: the resource schema does not declare Tags, so CloudFormation fails the resource in its own property validation and the stack settles ROLLBACK_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE.
Why: a template real CloudFormation refuses is deployed, so the caller believes an unsupported property took effect.
Fix: CloudFormation's pre-provision property validation for the AWS::SNS::TopicInlinePolicy schema.

Cause 4 of 4: the refusal SNS returns for an attempt to mutate a CloudFormation system tag names the wrong request parameter.
AWS: both TagResource and UntagResource refuse a request touching an `aws:` system tag with the SAME InvalidParameterException — Smithy (com.amazonaws.sns) models InvalidParameterException with awsQueryError code `InvalidParameter` and HTTP 400 — and the message `Invalid parameter: Tags Reason: Caller is an end user and not allowed to mutate system tags`. The fixed text is `Invalid parameter: Tags Reason: Caller is an end user and not allowed to mutate system tags`; nothing in it varies with the request, and the parameter named is `Tags` for the untag path too.
Emulator: UntagResource answers ('InvalidParameter', 'Invalid parameter: TagKeys Reason: Caller is an end user and not allowed to mutate system tags') — the right code and shape, but the parameter name substituted from the untag input shape.
Why: the message is a requirement; a client or test that matches SNS's refusal text (or that reports which parameter was invalid) sees a parameter name AWS does not use here, so the refusal is misattributed.
Fix: the system-tag guard in the UntagResource handler — emit the `Tags` parameter name both paths share.

## Evidence
Cause 1: "stack ended 'ROLLBACK_COMPLETE', expected CREATE_COMPLETE" (declared TopicArn); "Id in template: expected CREATE_COMPLETE, got 'ROLLBACK_COMPLETE'".
Cause 2: "the inline policy's physical id is 'sim2real-xsvc-tipol-stack-<id>-OutsideInline-<suffix>', expected its topic ARN".
Cause 3: "tagged inline policy: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'".
Cause 4: expected ('InvalidParameter', 'Invalid parameter: Tags Reason: Caller is an end user and not allowed to mutate system tags'); got ('InvalidParameter', 'Invalid parameter: TagKeys Reason: Caller is an end user and not allowed to mutate system tags').

## Steps to Reproduce
Cause 1: CreateStack with an AWS::SNS::Topic whose Properties set TopicArn, and separately an AWS::SNS::TopicPolicy whose Properties set Id — expect CREATE_COMPLETE with the declared value ignored.
Cause 2: deploy an AWS::SNS::TopicInlinePolicy and compare DescribeStackResource's PhysicalResourceId (and Ref) with the topic ARN.
Cause 3: CreateStack with an AWS::SNS::TopicInlinePolicy that declares Tags — expect ROLLBACK_COMPLETE.
Cause 4: on a stack-managed topic, call UntagResource(ResourceArn=<topic>, TagKeys=['aws:cloudformation:stack-name']) and read the Error Code and Message.
