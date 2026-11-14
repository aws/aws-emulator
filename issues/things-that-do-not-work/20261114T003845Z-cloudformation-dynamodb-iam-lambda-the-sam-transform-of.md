Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda.sam-connector-function-table-grants-read-policy
Issue-Key: cloudformation|DescribeStackResource|ValidationError
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# cloudformation__dynamodb__iam__lambda: the SAM transform of AWS::Serverless::Connector synthesizes no IAM policy resource, so <ConnectorId>Policy does not exist in the deployed stack

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResource

## Description
Cause 1 of 1: a stack containing an AWS::Serverless::Connector deploys, but the transform never emits the connector's generated IAM policy resource, so the stack has no resource at the connector's generated logical id.
AWS: the SAM transform expands an AWS::Serverless::Connector into concrete resources, including an AWS::IAM::ManagedPolicy/Policy whose logical id is the connector's logical id with the suffix `Policy` (for a connector named `Connector`, the resource `ConnectorPolicy`). That resource is part of the transformed template, so DescribeStackResource/DescribeStackResources on the deployed stack return it with its physical id and type, and the source function's role gains the declared read permissions on the destination table. Documented in the SAM developer guide, 'AWS::Serverless::Connector'; the cloudformation Smithy model states only the refusal shape — DescribeStackResource raises ValidationError when the LogicalResourceId is not a resource of the stack — so the expansion rule itself is documented behavior, not model text.
Emulator: the stack reaches CREATE_COMPLETE but DescribeStackResource for the connector's generated policy is refused with 400 ValidationError and the message `Resource <LogicalResourceId> does not exist for stack <stack ARN>` — the fixed text `Resource ` / ` does not exist for stack ` with the requested logical id and the stack's own ARN filled in. The refusal itself is AWS's correct wording for a missing resource; the defect is that the resource is missing, i.e. the transform emitted no policy for the connector.
Why: a client deploying a SAM template that relies on a connector to grant its function read access to a table gets a stack that looks complete but contains none of the generated permissions, and cannot discover or audit the grant through DescribeStackResource(s) — the function is then denied at runtime for a permission the template declared.
Fix: the CloudFormation SAM transform's AWS::Serverless::Connector expansion — it must synthesize and register the `<ConnectorId>Policy` IAM resource (and attach it to the source's role) in the transformed template so the stack's resource list contains it.
Traces: cloudformation__dynamodb__iam__lambda.sam-connector-function-table-grants-read-policy.

The open issue 20261109T163411Z-cloudformation-dynamodb-iam-lambda-the-sam-transform-accepts-an-aws-serverless-connector.md covers the connector's *invalid-permission refusal*; it does not cover the missing generated policy resource on the valid path.

## Evidence
cloudformation__dynamodb__iam__lambda.sam-connector-function-table-grants-read-policy — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the DescribeStackResource operation: Resource ConnectorPolicy does not exist for stack arn:aws:cloudformation:<region>:<account>:stack/<stack-name>/<stack-id>.
Expected per AWS: DescribeStackResource(LogicalResourceId="ConnectorPolicy") returns the transform-generated IAM policy resource for the AWS::Serverless::Connector, with ResourceStatus CREATE_COMPLETE.

## Steps to Reproduce
1. CreateStack with Transform: AWS::Serverless-2016-10-31, a AWS::Serverless::Function, a AWS::Serverless::SimpleTable (or AWS::DynamoDB::Table) and an AWS::Serverless::Connector named `Connector` with Permissions: [Read] from the function to the table.
2. Wait for CREATE_COMPLETE.
3. Call DescribeStackResource(StackName=<stack>, LogicalResourceId="ConnectorPolicy"): AWS returns the generated policy resource; the emulator raises ValidationError "Resource ConnectorPolicy does not exist for stack <arn>".
