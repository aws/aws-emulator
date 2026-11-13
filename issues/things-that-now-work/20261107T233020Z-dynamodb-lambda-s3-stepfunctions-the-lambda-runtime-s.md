Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status dynamodb__lambda__s3__stepfunctions.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record dynamodb__lambda__s3__stepfunctions.tenant-workflow-input-without-tenant-id-fails-at-first-task
Issue-Key: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status|run#4|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: dynamodb__lambda__s3__stepfunctions.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record|run#7|AssertionError|assert first["status"] == "SUCCEEDED", (
Issue-Key: dynamodb__lambda__s3__stepfunctions.tenant-workflow-input-without-tenant-id-fails-at-first-task|run#5|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# dynamodb__lambda__s3__stepfunctions: the Lambda runtime's bundled botocore knows no 's3' client, so every tenant-provisioning workflow fails with UnknownServiceError

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, 3 failing checks.

Cause 1 of 1: a Lambda handler that calls boto3.client("s3") dies with UnknownServiceError, so the Step Functions task that invokes it fails and the whole provisioning workflow ends FAILED.
  AWS: recorded behavior only for the runtime's SDK inventory (no Smithy model covers what boto3 ships). The recording shows the handler's s3.put_object(...) succeeding inside the function and the four-task execution ending SUCCEEDED with both S3 objects written and the DynamoDB record PROVISIONED.
  Emulator: the execution ends status='FAILED' with error 'UnknownServiceError' and the cause body {"errorMessage":"Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, sesv2", "errorType":"UnknownServiceError", ...} raised from the runtime's own bundled botocore shim (/var/task/botocore/client.py, _raise_refusal). The bundled catalog is a fixed nine-service list that omits s3; the valid-service list is emulator-authored text.
  Why: no Lambda function can touch S3 — the most common integration there is. Every workflow whose task writes or reads an object fails, and a client sees an error class (UnknownServiceError) that AWS's runtime never raises for a real service name, so it cannot even be handled as a service error. The three checks here all fail on their happy-path control, so their own subjects (PROVISIONED record shape, ExecutionAlreadyExists on a re-run, first-task failure without tenant_id) are never reached.
  Note: sibling issues already exist for other missing clients in the same catalog (20261107T212447Z-lambda-sqs-stepfunctions-2-net-new-causes-the-lambda-runtime-s.md for 'stepfunctions', 20261107T212447Z-cloudwatch-lambda-the-lambda-python-runtime-s-bundled-botocore-knows.md for 'cloudwatch', 20261107T212447Z-lambda-sesv2-2-net-new-causes-the-lambda-runtime-s.md for 'sesv2'); none carries these checks or the 's3' gap. Open issues 20261102T111517Z-dynamodb-lambda-s3-stepfunctions-a-task-state-invoking.md and 20261106T213332Z-dynamodb-lambda-s3-stepfunctions-the-state-machine-interpreter.md describe earlier failure points of these checks (States.Runtime task error, a 2-second interpretation budget) and not this one.
  Fix: the Lambda runtime's bundled SDK/service-model inventory in the lambda service crate (services/service-com-amazonaws-lambda-awsgirapiservice, the in-process python runtime's botocore data catalog and its endpoint resolution) must include s3 and resolve it to the emulator.
  Traces: dynamodb__lambda__s3__stepfunctions.tenant-provisioning-workflow-writes-objects-url-and-provisioned-status, dynamodb__lambda__s3__stepfunctions.tenant-setup-rerun-execution-name-rejected-and-put-item-replaces-record, dynamodb__lambda__s3__stepfunctions.tenant-workflow-input-without-tenant-id-fails-at-first-task.

## Evidence
AssertionError: the workflow for basalt-tenant ended 'FAILED': 'UnknownServiceError' '{"errorMessage":"Unknown service: \'s3\'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, sesv2","errorType":"UnknownServiceError","requestId":"...","stackTrace":["  File \"/var/task/index.py\", line 41, in handler\n    s3.put_object(\n","  File \"/var/task/botocore/client.py\", line 113, in method ...","  File \"/var/task/botocore/client.py\", line 168, in _raise_refusal\n    raise UnknownServiceError(\n"]}'
AssertionError: the first setup run ended 'FAILED': 'UnknownServiceError' (same body)
AssertionError: the control execution ended 'FAILED': 'UnknownServiceError'

Expected: status 'SUCCEEDED' with the handler's s3.put_object having written the objects. The tenant id and request id are per-run values; the requirement is that boto3.client("s3") constructs and its calls reach the emulator's S3.

## Steps to Reproduce
1. Create a Lambda function (python runtime) whose handler does s3 = boto3.client("s3"); s3.put_object(Bucket=..., Key=..., Body=...).
2. Create a state machine with a Task state invoking that function and StartExecution.
3. DescribeExecution: the emulator reports FAILED with error UnknownServiceError and the "Unknown service: 's3'" body; AWS reports SUCCEEDED and the object exists.
