Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fsx__lambda.function-creates-snapshot-and-backup-of-openzfs-file-system fsx__lambda.function-without-create-snapshot-permission-denied
Issue-Key: fsx__lambda.function-creates-snapshot-and-backup-of-openzfs-file-system|run#26|AssertionError|assert "FunctionError" not in resp, f"the function failed: {out!r}"
Issue-Key: fsx__lambda.function-without-create-snapshot-permission-denied|_ask#21|AssertionError|assert "FunctionError" not in resp, f"the function failed: {out!r}"
Issue-Commit: 51355cfc1cfbb5e6f009c7f9575dfb9e65707db6

# fsx__lambda: the in-sandbox Lambda AWS SDK has no 'fsx' client, so every FSx call from a function faults UnknownServiceError before reaching FSx

> _Note: this issue was identified against the code base at commit `51355cfc1cfbb5e6f009c7f9575dfb9e65707db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshot

## Description
Cause 1 of 1: the AWS SDK the emulator provides inside the Python Lambda execution environment carries a fixed registry of service names that does not include `fsx`, so `boto3.client("fsx")` (or the first call made through it) raises `UnknownServiceError` inside the handler. Both fsx__lambda checks die on the function's very first FSx call, before FSx is ever asked anything. Failing checks: fsx__lambda.function-creates-snapshot-and-backup-of-openzfs-file-system, fsx__lambda.function-without-create-snapshot-permission-denied.

AWS: on AWS, a Lambda function's bundled SDK can construct a client for any AWS service the SDK ships, FSx included, and the call is then signed and sent to the FSx endpoint, where the usual FSx contract applies. Smithy (service-com-amazonaws-fsx-awssimbaapiservice-v20180301): `CreateSnapshot` takes required `Name` (SnapshotName, @length 1-203, pattern ^[a-zA-Z0-9_:.-]{1,203}$) and required `VolumeId` (@length 23, pattern ^(fsvol-[0-9a-f]{17,})$) plus optional `Tags`, returns `Snapshot`, and models `BadRequest`, `VolumeNotFound`, `ServiceLimitExceeded`, `InternalServerError` (all 400 except the 500). `CreateBackup` takes `FileSystemId` (pattern ^(fs-[0-9a-f]{8,})$) / `VolumeId` / `Tags`, returns `Backup`, and models `FileSystemNotFound`, `BackupInProgress`, `IncompatibleParameterError`, `UnsupportedOperation`, `BadRequest`, `ServiceLimitExceeded`, `VolumeNotFound`. Recorded behavior of the checks: the granted function gets back a snapshot id matching `(fs)?volsnap-[0-9a-f]{8,}` and a backup id matching `backup-[0-9a-f]{8,}`; a function whose execution role lacks fsx:CreateSnapshot is refused `AccessDeniedException` with the sigv4 authorization sentence whose fixed text is "User: <assumed-role ARN> is not authorized to perform: fsx:CreateSnapshot on resource: <snapshot ARN> because no identity-based policy allows the fsx:CreateSnapshot action" — the variable parts are the caller's assumed-role ARN (arn:aws:sts::<account>:assumed-role/<role>/<function>) and the resource ARN (arn:aws:fsx:<region>:<account>:snapshot/<volume-id>/*), both taken from the request and the emulator's state, not from any one recording; and the granted function asking for a volume no file system carries is refused `BadRequest` with the fixed message "Unable to create a snapshot because the volume was not found".

Emulator: the handler never reaches FSx. The invoke returns with `FunctionError` set and a payload of `{"errorType": "UnknownServiceError", "errorMessage": "Unknown service: 'fsx'. Valid service names are: sqs, sns, sts, iam, events, ... transfer, batch, ecr, ecs"}`, raised from the in-sandbox client's own `_raise_refusal` at the first `fsx.create_snapshot(...)`/`fsx.create_backup(...)` line of index.py. `fsx` is simply absent from that valid-service list, while the FSx front door itself answers the harness's own out-of-band fsx client (3 of 4 direct fsx checks pass).

Why: any client code that does the normal thing — a Lambda function administering FSx (snapshotting a volume, taking a user-initiated backup, reacting to a file-system event) — cannot run at all on the emulator. It fails with an SDK-construction error that AWS never produces, instead of either succeeding or being refused by FSx's own modeled errors. The failure mode also masks IAM behavior: the denial path (AccessDeniedException for a role without fsx:CreateSnapshot, and its later allow after put_role_policy) can never be exercised, so a user testing least-privilege FSx policies gets a false signal.

Fix: the Lambda execution environment's bundled AWS SDK shim — the service registry/allowlist its in-sandbox `client.py` consults before dispatching (the code that raises `UnknownServiceError` in `_raise_refusal`). `fsx` must resolve there and its calls must be dispatched to the emulator's FSx front door so FSx's own handlers (CreateSnapshot / CreateBackup) and the IAM authorization check decide the outcome. Note this is NOT the already-filed 'runtime provides no boto3 at all' family (20261105T081139Z-lambda-rds-secretsmanager-..., 20261105T094842Z-cloudformation-lambda-s3-sqs-..., 20261105T112044Z-dynamodb-lambda-s3-...): an SDK is now present and importable; its service registry is just missing fsx.

Traces: fsx__lambda.function-creates-snapshot-and-backup-of-openzfs-file-system (assert "FunctionError" not in resp, run.py line 461) and fsx__lambda.function-without-create-snapshot-permission-denied (_ask, line 113).

## Evidence
Expected (AWS): `boto3.client("fsx")` inside a python3.12 Lambda constructs an FSx client; `create_snapshot(Name=..., VolumeId=<fsvol-...>, Tags=[...])` returns `Snapshot.SnapshotId` of shape `(fs)?volsnap-[0-9a-f]{8,}` and `create_backup(FileSystemId=<fs-...>, Tags=[...])` returns `Backup.BackupId` of shape `backup-[0-9a-f]{8,}`; with no fsx:CreateSnapshot grant on the execution role the call is refused `AccessDeniedException` "User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: fsx:CreateSnapshot on resource: arn:aws:fsx:<region>:<account>:snapshot/<volume-id>/* because no identity-based policy allows the fsx:CreateSnapshot action"; after the grant, an absent volume is refused `BadRequest` "Unable to create a snapshot because the volume was not found".

Actual (emulator): Invoke returns with `FunctionError` set and payload
{'errorMessage': "Unknown service: 'fsx'. Valid service names are: sqs, sns, sts, iam, events, eventbridge, pipes, scheduler, dynamodb, logs, cloudwatchlogs, cloudwatch, apigateway, apigatewaymanagementapi, secretsmanager, kms, ssm, opensearch, es, stepfunctions, sfn, ses, sesv2, s3, xray, rds, cloudformation, ec2, rds-data, appconfigdata, appconfig, config, configservice, codedeploy, codepipeline, cloudfront, rekognition, lambda, glue, bedrock-agent-runtime, bedrock-agent, opensearchserverless, s3vectors, kinesis, cognito-idp, cognito-identity-provider, iot, iot-data, iot-data-plane, transfer, batch, ecr, ecs", 'errorType': 'UnknownServiceError', 'stackTrace': ['File "index.py", line 9, in handler\n  snap = fsx.create_snapshot(...)', 'File "client.py", line 739, in _call\n  self._raise_refusal(refused, answered, operation_name)', 'File "client.py", line 821, in _raise_refusal\n  raise UnknownServiceError(']}
The same payload (differing only in requestId and the handler line number) is returned for both checks, so neither FSx nor IAM is ever consulted. No status code or FSx error code is observable at all: the refusal is produced client-side inside the sandbox.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com; for the happy-path leg attach an inline policy allowing fsx:CreateSnapshot, fsx:CreateBackup, fsx:TagResource on *.
2. CreateFunction (Runtime python3.12, Handler index.handler) with index.py containing:
     import boto3
     fsx = boto3.client("fsx")
     def handler(event, context):
         snap = fsx.create_snapshot(Name=event["snapshot_name"], VolumeId=event["volume_id"])
         return {"snapshot_id": snap["Snapshot"]["SnapshotId"]}
3. CreateFileSystem FileSystemType=OPENZFS (StorageCapacity 64, SSD, SINGLE_AZ_1, ThroughputCapacity 64); wait for Lifecycle AVAILABLE and read OpenZFSConfiguration.RootVolumeId.
4. Invoke the function with {"volume_id": <root volume id>, "snapshot_name": "<name>"}.
5. Observe the response carries FunctionError and the payload is UnknownServiceError "Unknown service: 'fsx'" instead of a snapshot id (or, for the role without the grant, instead of AccessDeniedException).
