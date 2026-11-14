Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.deploy-cfn-dynamodb-getatt-stream-arn-follows-stream-view-change
Issue-Key: cloudformation__dynamodb.deploy-cfn-dynamodb-getatt-stream-arn-follows-stream-view-change|_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# CloudFormation Fn::GetAtt StreamArn output carries no OutputValue after a StreamViewType change

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause in the cloudformation__dynamodb group.

Cause 1 of 1: a stack Output built from `Fn::GetAtt <Table>.StreamArn` has no `OutputValue`.
AWS: `DescribeStacks` returns the Output with the table's current stream ARN, and the value follows a StreamViewType change (a new stream ARN is minted and the output is updated).
Emulator: the Outputs entry exists without an `OutputValue` member, so reading it raises `KeyError: 'OutputValue'` and the stream ARN is never observable.
Why: a template that exports a table's stream ARN (the normal way to wire an event source mapping) hands the consumer nothing, so the downstream resource can never be configured. (The open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md covers sibling checks in this group but not this replay key.)
Fix: the cloudformation Output resolution for Fn::GetAtt on AWS::DynamoDB::Table StreamArn, and the DescribeStacks serializer that must always emit OutputValue.
Traces: cloudformation__dynamodb.deploy-cfn-dynamodb-getatt-stream-arn-follows-stream-view-change

## Evidence
[error] cloudformation__dynamodb.deploy-cfn-dynamodb-getatt-stream-arn-follows-stream-view-change — KeyError: 'OutputValue' raised while reading the stack Output that should carry the table's stream ARN.

## Steps to Reproduce
Boot the emulator and replay cloudformation__dynamodb.deploy-cfn-dynamodb-getatt-stream-arn-follows-stream-view-change: it deploys a table whose stack Output is Fn::GetAtt Table.StreamArn, changes StreamViewType, and re-reads the Output.
