Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.deploy-subscription-arn-tag-calls-rejected-while-its-topic-carries-stack-tags cloudformation__sns__sqs.subscription-getatt-arn-names-the-subscription cloudformation__sns__sqs.deploy-subscription-getatt-region-rejected cloudformation__sns__sqs.topic-inline-subscription-list-update-moves-delivery-in-place
Issue-Key: cloudformation__sns__sqs.deploy-subscription-arn-tag-calls-rejected-while-its-topic-carries-stack-tags|run#27|AssertionError|assert got == _UNTAGGABLE, f"ListTagsForResource on the subscription ARN gave {got!r}"
Issue-Key: cloudformation__sns__sqs.subscription-getatt-arn-names-the-subscription|run#10|AssertionError|assert out["SubArn"] == out["SubRef"], f"GetAtt Sub.Arn {out['SubArn']!r} differs from Ref {out['SubRef']!r}"
Issue-Key: sns|GetSubscriptionAttributes|InvalidParameter
Issue-Key: cloudformation__sns__sqs.topic-inline-subscription-list-update-moves-delivery-in-place|run#13|AssertionError|assert status == "UPDATE_COMPLETE", f"the Subscription-list update ended {status}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 3 causes in CloudFormation's SNS subscription surface: subscription-ARN tagging refusal wording, GetAtt Arn, and in-place Subscription-list update

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three causes.

Cause 1 of 3: SNS tagging operations on a subscription ARN are refused with the wrong message.
AWS: SNS tags are held by topics only. ListTagsForResource / TagResource / UntagResource given a subscription ARN are refused with `InvalidParameter` (Smithy: com.amazonaws.sns InvalidParameterException, awsQueryError code `InvalidParameter`, HTTP 400) and the fixed message `Invalid parameter: The given ARN is not supported by tagging operations` — the whole string is fixed text, with no part drawn from the request.
Emulator: the same three calls are refused with `InvalidParameter` but the message reads `Invalid parameter: ResourceArn Reason: An SNS tag is held by a topic, so the ARN must name an Amazon SNS topic`.
Why: the code matches, so a retry-classifier is unaffected, but a client (or an operator reading logs) matching on AWS's documented wording never matches, and the message names a parameter and a reason AWS does not emit.
Fix: the SNS ListTagsForResource/TagResource/UntagResource handlers' ARN validation message.
Traces: cloudformation__sns__sqs.deploy-subscription-arn-tag-calls-rejected-while-its-topic-carries-stack-tags.

Cause 2 of 3: AWS::SNS::Subscription's `Fn::GetAtt Arn` returns a synthesized resource-style ARN instead of the subscription ARN.
AWS: an SNS subscription ARN is its topic ARN with a subscription UUID appended, `arn:aws:sns:<region>:<account>:<topic-name>:<subscription-uuid>` (Smithy: com.amazonaws.sns Subscribe output SubscriptionArn; GetSubscriptionAttributes takes that same value). For AWS::SNS::Subscription, `Ref` and `Fn::GetAtt Arn` are both that subscription ARN, and GetSubscriptionAttributes reads it back.
Emulator: GetAtt Arn returns `arn:aws:sns:<region>:<account>:subscription/<stack>-<logical-id>-<suffix>`, which differs from the resource's Ref; passing it to GetSubscriptionAttributes is then refused with `InvalidParameter` / `Invalid parameter: SubscriptionId`.
Why: any template that reads the subscription through GetAtt Arn — and any client that feeds that output to SetSubscriptionAttributes, Unsubscribe or an IAM policy — holds an ARN that names nothing. The second check shows the follow-on: a read-back of the GetAtt value fails outright.
Fix: the AWS::SNS::Subscription resource handler's attribute projection (Arn must be the subscription ARN the Subscribe call returned).
Traces: cloudformation__sns__sqs.subscription-getatt-arn-names-the-subscription, cloudformation__sns__sqs.deploy-subscription-getatt-region-rejected.

Cause 3 of 3: updating an AWS::SNS::Topic's inline `Subscription` list rolls the stack back instead of moving delivery in place.
AWS: `Subscription` is a mutable property of AWS::SNS::Topic; an update that changes its entries subscribes the new endpoints and unsubscribes the removed ones, the stack reaches UPDATE_COMPLETE, and the topic afterwards delivers to the new endpoint only.
Emulator: the update ends `UPDATE_ROLLBACK_COMPLETE`, so the subscription list cannot be changed at all.
Why: a client cannot move a topic's inline subscriptions; the stack is left on its previous template and the intended delivery change never happens.
Fix: the AWS::SNS::Topic resource handler's update path for the Subscription property.
Traces: cloudformation__sns__sqs.topic-inline-subscription-list-update-moves-delivery-in-place.

## Evidence
Cause 1 — expected ('InvalidParameter', 'Invalid parameter: The given ARN is not supported by tagging operations'); actual `('InvalidParameter', 'Invalid parameter: ResourceArn Reason: An SNS tag is held by a topic, so the ARN must name an Amazon SNS topic')`.
Cause 2 — `AssertionError: GetAtt Sub.Arn 'arn:aws:sns:us-west-2:000000000000:subscription/sim2real-behav-sns-subarn-a096ab8708d7-Sub-58679B0FFE85' differs from Ref 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-sns-subarn-a096ab8708d7-topic:09be905e-a4dd-4dfb-9e00-58ab6ce2e43a'`; and feeding the GetAtt value to GetSubscriptionAttributes raises `An error occurred (InvalidParameter) when calling the GetSubscriptionAttributes operation: Invalid parameter: SubscriptionId`. Fixed form of a subscription ARN: `<topic-arn>:<uuid>`; the topic ARN and uuid are state values.
Cause 3 — expected UPDATE_COMPLETE; actual `AssertionError: the Subscription-list update ended UPDATE_ROLLBACK_COMPLETE`.

## Steps to Reproduce
Cause 1: deploy a stack with an AWS::SNS::Topic and an AWS::SNS::Subscription, then call ListTagsForResource/TagResource/UntagResource with the subscription's physical id.
Cause 2: add `Outputs: {LinkValue: {Value: {Fn::GetAtt: [Sub, Arn]}}}` and compare with `Ref Sub`; then call GetSubscriptionAttributes with the output value.
Cause 3: deploy an AWS::SNS::Topic with an inline `Subscription` list naming one queue, then update the template so the list names a different queue.
Replay: the four check ids in trace_ids.
