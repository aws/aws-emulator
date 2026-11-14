Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed
Issue-Key: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed|_probe_denied#61|AssertionError|raise AssertionError(
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__dynamodb__iam__lambda__s3: the SAM Policies shorthand expands over-permissively — dynamodb:DeleteTable succeeds under a DynamoDBCrudPolicy-derived role

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTable

## Description
Cause 1 of 1: the role the SAM transform synthesizes from a `Policies:` shorthand grants more than the shorthand's documented action set, and the extra action is not denied at call time.
AWS:      recorded behavior only for the expansion itself (the SAM policy templates are not in a Smithy model), but the consequence is modeled: dynamodb DeleteTable is an authorized control-plane call, and a principal whose policy does not allow `dynamodb:DeleteTable` on the table is refused — IAM authorization answers AccessDeniedException (HTTP 400 for the DynamoDB JSON protocol) with AWS's message shape `User: <role-session-arn> is not authorized to perform: dynamodb:DeleteTable on resource: <table-arn> because no identity-based policy allows the dynamodb:DeleteTable action`, whose fixed text is `User: `, ` is not authorized to perform: `, ` on resource: ` and ` because no identity-based policy allows the ... action`, with the principal ARN, the action and the table ARN taken from the request and the caller's identity. The CRUD shorthand covers item and query/scan operations plus DescribeTable; it does not cover deleting the table, so the probe must be refused.
Emulator: the probe invoked under the synthesized role SUCCEEDS: DeleteTable returns 200 with a TableDescription (TableStatus 'DELETING'). Either the expansion emits a wildcard/over-broad action set, or the DeleteTable handler performs no identity-based authorization for the assumed role.
Why:      a client relying on SAM's shorthand to bound a function's blast radius is given a function that can destroy the table it was only meant to read and write. A negative test that proves least privilege passes vacuously, and the emulator cannot be used to validate any policy-scoping change.
Fix:      the SAM transform's `Policies:` shorthand expansion (the policy document it synthesizes for the generated AWS::IAM::Role) and dynamodb's DeleteTable authorization path, which must evaluate the caller's identity-based policy and refuse with AccessDeniedException when the action is not allowed.
Traces:   cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed.

## Evidence
Expected: the probe `dynamodb.delete_table` executed under the SAM-synthesized role is refused with AccessDeniedException naming dynamodb:DeleteTable and the table ARN.
Actual:
  File "check_cloudformation__dynamodb__iam__lambda__s3_sam_policies_shorthand_expansion_deny_probed.py", line 404, in _probe_denied
  AssertionError: dynamodb.delete_table SUCCEEDED under the synthesized role; the expanded policy is over-permissive. Probe returned {'ok': True, 'result': {'TableDescription': {'TableName': '<generated>', 'TableStatus': 'DELETING', 'BillingModeSummary': {'BillingMode': 'PAY_PER_REQUEST', ...}, 'StreamSpecification': {'StreamEnabled': True, 'StreamViewType': 'NEW_AND_OLD_IMAGES'}, ...}}}

The diverging part is the outcome: a 200 TableDescription instead of an AccessDeniedException refusal. The table name, ARN, TableId and timestamps are per-run values.

## Steps to Reproduce
1. Deploy a SAM template (Transform: AWS::Serverless-2016-10-31) whose AWS::Serverless::Function uses the `Policies:` shorthand for CRUD access to a DynamoDB table (and the corresponding S3 shorthand), letting the transform synthesize the execution role.
2. Wait for CREATE_COMPLETE.
3. From inside the function (using its execution role), call dynamodb DeleteTable on the table named by the shorthand.
4. Observe a 200 TableDescription (TableStatus 'DELETING') instead of an AccessDeniedException refusing dynamodb:DeleteTable.
