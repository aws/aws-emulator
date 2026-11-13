Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails|run#16|AssertionError|assert expected in reasons, (
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails|run#17|AssertionError|assert expected in reasons, (
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# cloudformation__s3__sqs: an AWS::Include transform failure is never recorded as its own stack event reason — the transform error is folded into the rollback reason

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
1 net-new root cause in CloudFormation's transform (macro) failure reporting. Related open issue 20261109T185037Z-cloudformation-s3-sqs-aws-serverless-application-with-a-location-is-not.md covers a neighbouring transform cause but carries neither of these replay keys.

Cause 1 of 1: a failed Fn::Transform AWS::Include never produces a stack event whose ResourceStatusReason is the transform's own error.
AWS: when a template's Fn::Transform / AWS::Include cannot be processed, CloudFormation records the transform's error as a stack-level event reason on the AWS::CloudFormation::Stack resource, verbatim and on its own — e.g. "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON" for an object that is not parseable YAML/JSON, and "Transform AWS::Include failed with: S3 key [<key>] does not exist." for a Location whose key is absent (fixed text per failure class; the bracketed S3 key is taken from the request's Location). The rollback it then triggers is a SEPARATE event whose reason is "Rollback requested by user."; the stack still ends ROLLBACK_COMPLETE and owns no resources. Smithy: DescribeStackEvents returns StackEvent{ResourceType, ResourceStatus, ResourceStatusReason}; the model states no reason text, so the wording above is recorded AWS behavior.
Emulator: the stack does reach ROLLBACK_COMPLETE and provisions no queue, but DescribeStackEvents carries only two stack-level reasons — 'User Initiated' and one concatenated sentence that appends the rollback clause to the transform error: "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON. Rollback requested by user." (and likewise "... S3 key [snippets/absent.yaml] does not exist. Rollback requested by user."). No event carries the transform error alone.
Why: a client or CI job that reads DescribeStackEvents and matches/extracts the transform diagnostic (the documented way to find out WHY a macro-expanded deploy failed) finds no event equal to it; equality and prefix/suffix matching on AWS's sentence both fail, and the two distinct causes CloudFormation keeps apart — the transform's failure and the rollback it triggered — are merged into one record.
Why: the operator loses the ability to tell the transform diagnostic apart from the rollback notice.
Fix: the stack-event writer on the transform-failure path of the CreateStack handler in the cloudformation service — emit the transform error as its own stack-level event reason, then the rollback reason as a separate event.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails, cloudformation__s3__sqs.deploy-cloudformation-3-include-transform-missing-s3-key-fails.

## Evidence
Cause 1: expected the stack-level reason "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON"; the stack's reasons were ['User Initiated', "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON. Rollback requested by user."]. Second check: expected 'Transform AWS::Include failed with: S3 key [snippets/absent.yaml] does not exist.'; reasons were ['User Initiated', 'Transform AWS::Include failed with: S3 key [snippets/absent.yaml] does not exist. Rollback requested by user.'].

## Steps to Reproduce
Cause 1: put an object that is not valid YAML/JSON (or omit the key entirely) in a bucket; CreateStack with CAPABILITY_AUTO_EXPAND on a template whose AWS::SQS::Queue Properties carry Fn::Transform {Name: AWS::Include, Parameters: {Location: s3://<bucket>/<key>}}; wait for ROLLBACK_COMPLETE, then read DescribeStackEvents and collect every ResourceStatusReason whose ResourceType is AWS::CloudFormation::Stack.
