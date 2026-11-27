Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.deploy-gsi-contributor-insights-specification-enables-index-insights cloudformation__dynamodb.deploy-stream-resource-policy-attaches-to-the-stream
Issue-Key: cloudformation__dynamodb.deploy-gsi-contributor-insights-specification-enables-index-insights|run#11|AssertionError|assert status == "ENABLED", f"the index's ContributorInsightsStatus is {status!r}, expected ENABLED"
Issue-Key: cloudformation__dynamodb.deploy-stream-resource-policy-attaches-to-the-stream|run#7|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# cloudformation__dynamodb: AWS::DynamoDB::Table ignores a GSI ContributorInsightsSpecification and refuses StreamSpecification.ResourcePolicy outright (2 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the AWS::DynamoDB::Table CloudFormation resource handler.

Cause 1 of 2: `GlobalSecondaryIndexes[].ContributorInsightsSpecification` is accepted and then not applied.
AWS: a GSI whose `ContributorInsightsSpecification.Enabled` is true has contributor insights enabled on that index; `DescribeContributorInsights` for the table/index reports `ContributorInsightsStatus: ENABLED` (or ENABLING) once the stack reaches CREATE_COMPLETE.
Emulator: the stack succeeds but the index's `ContributorInsightsStatus` stays `DISABLED` — the property is parsed and dropped.
Why: a template that turns on contributor insights deploys green while the feature is off, so the monitoring the stack promises does not exist and nothing tells the operator.
Fix: the AWS::DynamoDB::Table resource handler's GSI create path — it must call UpdateContributorInsights for each index whose specification enables it.
Traces: cloudformation__dynamodb.deploy-gsi-contributor-insights-specification-enables-index-insights.

Cause 2 of 2: `StreamSpecification.ResourcePolicy` makes the whole stack roll back with an emulator-authored handler message.
AWS: a table whose `StreamSpecification` carries a `ResourcePolicy` is created with that policy attached to the stream; the stack reaches CREATE_COMPLETE and the policy is readable on the stream resource.
Emulator: the Table resource fails and the stack ends ROLLBACK_COMPLETE with `Resource handler returned message: "this emulator does not apply the StreamSpecification.ResourcePolicy property of AWS::DynamoDB::Table" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. The fixed text `this emulator does not apply the StreamSpecification.ResourcePolicy property of AWS::DynamoDB::Table` is not an AWS message at all; AWS raises no error here.
Why: a valid template cannot be deployed, and the failure message names the emulator rather than any property defect, so there is no corrective action a template author can take.
Fix: the AWS::DynamoDB::Table resource handler — implement StreamSpecification.ResourcePolicy (PutResourcePolicy against the stream ARN) instead of failing the resource.
Traces: cloudformation__dynamodb.deploy-stream-resource-policy-attaches-to-the-stream.

## Evidence
Cause 1: expected the GSI's ContributorInsightsStatus to be ENABLED; actual `the index's ContributorInsightsStatus is 'DISABLED'`.
Cause 2: expected CREATE_COMPLETE with the stream resource policy attached; actual `stack ended 'ROLLBACK_COMPLETE': [('Table', 'Resource handler returned message: "this emulator does not apply the StreamSpecification.ResourcePolicy property of AWS::DynamoDB::Table" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')]`.

## Steps to Reproduce
1. CreateStack with an AWS::DynamoDB::Table whose GlobalSecondaryIndexes[0].ContributorInsightsSpecification.Enabled is true; after CREATE_COMPLETE call DescribeContributorInsights for the table and index.
2. CreateStack with an AWS::DynamoDB::Table whose StreamSpecification carries a ResourcePolicy — observe ROLLBACK_COMPLETE.
