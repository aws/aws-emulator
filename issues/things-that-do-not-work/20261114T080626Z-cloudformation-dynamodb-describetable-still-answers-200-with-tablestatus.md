Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-resource-existence-dynamodb-table
Issue-Key: cloudformation__dynamodb.cfn-resource-existence-dynamodb-table|_error#32|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__dynamodb: DescribeTable still answers 200 with TableStatus DELETING after the owning stack reached DELETE_COMPLETE, instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Cause 1 of 1: a table deleted with its stack never becomes absent to DynamoDB.
AWS:      Smithy (dynamodb): DescribeTable's modeled errors include ResourceNotFoundException (client, HTTP 400). Once the table is gone, DescribeTable raises it with the message AWS words as `Requested resource not found: Table: <table-name> not found` — the fixed text is `Requested resource not found: Table: ` and ` not found`, with the table name taken from the request. CloudFormation only reports DELETE_COMPLETE for the stack once its AWS::DynamoDB::Table has actually been deleted, so a client that waits for DELETE_COMPLETE and then describes the table gets the not-found error, never a DELETING snapshot.
Emulator: after DeleteStack and the stack reaching DELETE_COMPLETE, DescribeTable(TableName=<physical id>) still SUCCEEDS, returning a full TableDescription whose TableStatus is `DELETING`. The table row is left in a non-terminal deleting state and is never removed, so the stack's DELETE_COMPLETE is premature with respect to the resource it owned.
Why:      a client that tears a stack down and then verifies the table is gone — a teardown test, a re-create of the same table name, a cost/leak audit — is told the table still exists. A re-create with the same name can collide with the lingering row, and `waiter table_not_exists` never settles, so automated teardown hangs or reports a leak.
Fix:      the AWS::DynamoDB::Table resource handler's delete path and dynamodb's table state: DeleteTable must take the table through DELETING to actual removal before CloudFormation marks the resource DELETE_COMPLETE, and DescribeTable must then answer ResourceNotFoundException with AWS's message for the requested table name.
Traces:   cloudformation__dynamodb.cfn-resource-existence-dynamodb-table.

## Evidence
Expected, after the stack reaches DELETE_COMPLETE:
  DescribeTable(TableName=<physical id>) -> ResourceNotFoundException (HTTP 400)
  message == "Requested resource not found: Table: <table-name> not found"
Actual:
  File "check_cloudformation__dynamodb_cfn_resource_existence_dynamodb_table.py", line 137, in run
  code, message = _error(lambda: ddb.describe_table(TableName=table_name))
  AssertionError: expected the call to be rejected, but it succeeded: {'Table': {..., 'TableName': '<generated>', 'TableStatus': 'DELETING', 'GlobalSecondaryIndexes': [{'IndexName': 'GsiIndex', 'IndexStatus': 'ACTIVE', ...}], 'StreamSpecification': {'StreamEnabled': True, 'StreamViewType': 'NEW_AND_OLD_IMAGES'}, ...}}

The diverging parts are the HTTP status/outcome (200 with a body instead of a 400 ResourceNotFoundException) and the lingering TableStatus 'DELETING'. The table name, ARN, TableId and timestamps in the recording are per-run values and fill the message template; they are not part of it.

## Steps to Reproduce
1. CreateStack with one AWS::DynamoDB::Table (BillingMode PAY_PER_REQUEST, KeySchema pk HASH, one GSI `GsiIndex` on gsipk, StreamSpecification NEW_AND_OLD_IMAGES) and Outputs for its Arn and StreamArn.
2. Wait for CREATE_COMPLETE; note the table's PhysicalResourceId.
3. DeleteStack and wait for DELETE_COMPLETE.
4. Call dynamodb DescribeTable(TableName=<physical id>).
5. Observe a 200 TableDescription with TableStatus 'DELETING' instead of ResourceNotFoundException "Requested resource not found: Table: <name> not found".
