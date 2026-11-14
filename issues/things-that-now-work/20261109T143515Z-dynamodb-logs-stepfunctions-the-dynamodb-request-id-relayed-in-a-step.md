Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__logs__stepfunctions.xsvc-sfn-optimized-ddb-put-item-conditional-check-rejected
Issue-Key: dynamodb__logs__stepfunctions.xsvc-sfn-optimized-ddb-put-item-conditional-check-rejected|run#15|AssertionError|assert masked == (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# The DynamoDB Request ID relayed in a Step Functions failure cause is a lowercase UUID instead of AWS's uppercase alphanumeric request id

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Cause 1 of 1: the request-id token inside the relayed DynamoDB error sentence does not have AWS's shape.
AWS: an optimized dynamodb:putItem task whose condition fails ends the execution FAILED with error 'DynamoDB.ConditionalCheckFailedException' and cause 'The conditional request failed (Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ConditionalCheckFailedException; Request ID: <id>; Proxy: null)', where the AWS DynamoDB request id is an uppercase alphanumeric token (matching [0-9A-Z]+) — that is the form the recording masks on.
Emulator: the fixed text and the error code are right, but the request id is a lowercase, hyphenated UUID ('a3522451-f366-4f3b-93ec-c372fadca8ca'), so the relayed cause does not have AWS's request-id format. The divergence is the token's format, not the particular value.
Why: a client that parses or masks the relayed DynamoDB message (as operators and log pipelines do) does not recognise the request id, and support workflows keyed on the DynamoDB request-id format break.
Fix: the DynamoDB request-id generator in the dynamodb crate (emit AWS's uppercase alphanumeric request id), which Step Functions relays verbatim into the execution's cause.
Traces: dynamodb__logs__stepfunctions.xsvc-sfn-optimized-ddb-put-item-conditional-check-rejected.

## Evidence
Expected cause (request id masked by the recording's [0-9A-Z]+ rule): 'The conditional request failed (Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ConditionalCheckFailedException; Request ID: <id>; Proxy: null)'. Actual: the same sentence with 'Request ID: a3522451-f366-4f3b-93ec-c372fadca8ca;', a lowercase UUID the AWS request-id form does not permit.

## Steps to Reproduce
Replay dynamodb__logs__stepfunctions.xsvc-sfn-optimized-ddb-put-item-conditional-check-rejected: start the state machine twice with the same id so the second optimized PutItem fails its condition, then read the execution's cause.
