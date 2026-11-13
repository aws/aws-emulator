Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.iam-role-trusts-sns-service
Issue-Key: cloudwatch__sns.iam-role-trusts-sns-service|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch__sns: SetTopicAttributes accepts an SQSSuccessFeedbackRoleArn whose role does not trust sns.amazonaws.com instead of refusing InvalidParameter (1 cause)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
Cause 1 of 1: the delivery-status feedback role's trust policy is not validated.
AWS: recorded behavior only (the trust-policy requirement is not expressible in the SNS Smithy model, which types the attribute value as a plain string). SetTopicAttributes with AttributeName=`SQSSuccessFeedbackRoleArn` validates that the named IAM role's trust policy allows `sns.amazonaws.com` to assume it. A role that trusts only the account root is refused HTTP 400 `InvalidParameter` with the message `Invalid parameter: SQSSuccessFeedbackRoleArn: <roleArn> is not a valid role to allow SNS to write to Cloudwatch Logs` — the fixed text is everything but `<roleArn>`, which is the role ARN from the request — and the attribute does NOT land on the topic (GetTopicAttributes afterwards has no SQSSuccessFeedbackRoleArn key). A role trusting sns.amazonaws.com is accepted, and only then does a publish write a delivery-status record to the vended CloudWatch Logs group.
Emulator: the call returns HTTP 200 (RequestId present, content-type text/xml) and the untrusted role is stored as the topic's SQSSuccessFeedbackRoleArn, so no refusal exists.
Why: the misconfiguration SNS exists to catch at configuration time (a feedback role SNS cannot assume) is accepted, so the topic reads back as correctly configured while delivery-status logging can never work; a client written against the InvalidParameter refusal never sees it, and the trusted/untrusted distinction becomes unobservable.
Fix: SetTopicAttributes' validation for the *FeedbackRoleArn attributes — resolve the named role and check its trust policy for sns.amazonaws.com before storing the attribute.
Traces: cloudwatch__sns.iam-role-trusts-sns-service.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__sns`), check iam-role-trusts-sns-service:

  File "check_cloudwatch__sns_iam_role_trusts_sns_service.py", line 56, in run
    error = _error(lambda: sns.set_topic_attributes(TopicArn=untrusted['topic'], AttributeName='SQSSuccessFeedbackRoleArn', AttributeValue=untrusted_role))
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'RequestId': '...', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '255', ...}, 'RetryAttempts': 0}}

Expected: error Code 'InvalidParameter' with Message 'Invalid parameter: SQSSuccessFeedbackRoleArn: <untrusted role ARN> is not a valid role to allow SNS to write to Cloudwatch Logs', and GetTopicAttributes with no SQSSuccessFeedbackRoleArn key afterwards. Actual: HTTP 200 and the attribute is stored.

## Steps to Reproduce
1. iam.create_role with an AssumeRolePolicyDocument whose Principal is {'AWS': 'arn:aws:iam::<account>:root'} (i.e. it does NOT trust sns.amazonaws.com), and attach a policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents.
2. sns.create_topic and wire it to an SQS subscription and a CloudWatch Logs group.
3. sns.set_topic_attributes(TopicArn=<topic>, AttributeName='SQSSuccessFeedbackRoleArn', AttributeValue=<that role ARN>).
4. Expect 400 InvalidParameter with the message above; observe HTTP 200.
5. sns.get_topic_attributes(TopicArn=<topic>) — expect no SQSSuccessFeedbackRoleArn key; observe the untrusted role ARN stored.
6. For contrast, repeat 1-5 with a role whose Principal is {'Service': 'sns.amazonaws.com'}: that one must be accepted.
