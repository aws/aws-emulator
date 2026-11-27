Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-eventbridge-rule-to-sns-topic-invokes-subscribed-lambda
Issue-Key: events__lambda__sns.cdk-deploy-eventbridge-rule-to-sns-topic-invokes-subscribed-lambda|_deploy_cfn#8|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CDK/CloudFormation: the stack deploying an EventBridge rule to an SNS topic with a subscribed Lambda rolls back

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the deployment fails instead of reaching CREATE_COMPLETE.
AWS: a CDK-synthesized stack declaring `AWS::SNS::Topic`, `AWS::SNS::TopicPolicy`, an `AWS::Events::Rule` with that topic as a target, an `AWS::Lambda::Function` and `AWS::SNS::Subscription` + `AWS::Lambda::Permission` deploys to CREATE_COMPLETE; PutEvents matching the rule then publishes to the topic and the subscribed Lambda is invoked.
Emulator: the stack ends `ROLLBACK_COMPLETE`, so no part of the rule -> SNS -> Lambda path can be exercised. One or more of the declared resource providers fails during create.
Why: a client cannot deploy the standard EventBridge-to-SNS fan-out via CDK/CloudFormation at all; the stack is rolled back and the generated topology never exists.
Fix: the failing resource provider(s) in the CloudFormation stack-create path for this template (the AWS::Events::Rule SNS-target, AWS::SNS::TopicPolicy / Subscription and AWS::Lambda::Permission providers) — the stack must reach CREATE_COMPLETE, and any genuine resource failure must surface a status reason naming the resource rather than a bare rollback.
Traces: events__lambda__sns.cdk-deploy-eventbridge-rule-to-sns-topic-invokes-subscribed-lambda.

## Evidence
`AssertionError: Stack sim2real-xsvc-stk-1a7750a30e62 ended in ROLLBACK_COMPLETE` (stack name is per-run). Expected CREATE_COMPLETE followed by a PutEvents-triggered Lambda invocation through the SNS subscription.

## Steps to Reproduce
1. CreateStack with the CDK-synthesized template declaring AWS::SNS::Topic + TopicPolicy, AWS::Events::Rule targeting the topic, AWS::Lambda::Function, AWS::SNS::Subscription and AWS::Lambda::Permission.
2. Poll DescribeStacks until a terminal status; observe ROLLBACK_COMPLETE, and read DescribeStackEvents for the failing resource.
3. Replay events__lambda__sns.cdk-deploy-eventbridge-rule-to-sns-topic-invokes-subscribed-lambda.
