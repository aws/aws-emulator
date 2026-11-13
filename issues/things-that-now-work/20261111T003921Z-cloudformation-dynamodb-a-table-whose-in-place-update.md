Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found|_assert_table#24|AssertionError|assert got == want, f"DescribeTable {name}: {got!r}, expected {want!r}"
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# cloudformation__dynamodb: a table whose in-place update the stack already reported UPDATE_COMPLETE stays TableStatus UPDATING forever instead of returning to ACTIVE

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
One net-new root cause (Cause 1 of 1), grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: after a no-interruption (in-place) update of an AWS::DynamoDB::Table, DescribeTable keeps reporting TableStatus 'UPDATING' although the new configuration is already in effect and the stack has reached UPDATE_COMPLETE.
AWS: Smithy (dynamodb) DescribeTable -> TableDescription.TableStatus is an enum whose values are CREATING, UPDATING, DELETING, ACTIVE, INACCESSIBLE_ENCRYPTION_CREDENTIALS, ARCHIVING, ARCHIVED, REPLICATION_NOT_AUTHORIZED. An UpdateTable that changes ProvisionedThroughput moves the table through UPDATING and back to ACTIVE once the new throughput is applied; CloudFormation does not report UPDATE_COMPLETE for the table's logical id until the table has settled. The recording reads back TableName/TableArn/KeySchema/AttributeDefinitions unchanged, ReadCapacityUnits=2 (the request's new value) and TableStatus ACTIVE.
Emulator: the same read returns everything else exactly as AWS does - including the new Read capacity 2 - but TableStatus 'UPDATING', and it never leaves that state.
Why: every DynamoDB client waits for ACTIVE before using a freshly changed table: the SDK's table_exists/table_updated waiters, the CLI's `dynamodb wait`, CDK and Terraform convergence loops. A table parked in UPDATING makes all of them hang or time out on a table that is actually ready, and it also makes a subsequent UpdateTable look illegal (a table 'being updated' is normally refused), so the divergence blocks the next step too.
Fix: the dynamodb table state machine behind UpdateTable - the status must return to ACTIVE when the applied change lands (and the AWS::DynamoDB::Table in-place update path must not report UPDATE_COMPLETE before the owning service's table is ACTIVE).
Traces: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found.

## Evidence
Replay output (behavioral suite, section ### cloudformation__dynamodb):
  [mismatch] cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found
  AssertionError: DescribeTable <table>: {'TableName': <table>, 'TableArn': 'arn:aws:dynamodb:<region>:<account>:table/<table>', 'TableStatus': 'UPDATING', 'KeySchema': [{'AttributeName': 'pk', 'KeyType': 'HASH'}], 'AttributeDefinitions': [{'AttributeName': 'pk', 'AttributeType': 'S'}], 'Read': 2, 'Write': 1}, expected {... 'TableStatus': 'ACTIVE' ...}

The only diverging member is TableStatus: expected 'ACTIVE', actual 'UPDATING'. Table name, ARN, key schema, attribute definitions and the updated Read/Write capacity all match. The table name/ARN in the message are per-run values from the request; the status value is the requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and dynamodb are both bootstrapped and serving).
2. CreateStack with one AWS::DynamoDB::Table (PROVISIONED billing, ReadCapacityUnits 1, WriteCapacityUnits 1, hash key 'pk'). Wait for CREATE_COMPLETE; dynamodb DescribeTable reports TableStatus ACTIVE.
3. UpdateStack with the same template except ReadCapacityUnits: 2 (a no-interruption property). Wait for UPDATE_COMPLETE.
4. Call dynamodb DescribeTable on the same table. Expect TableStatus 'ACTIVE' with ReadCapacityUnits 2. Observe TableStatus 'UPDATING' that never changes, with ReadCapacityUnits already 2.
5. Equivalently: re-replay cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found.
