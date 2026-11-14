Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__kms__secretsmanager.ecsm-kms-grant-unlocks-secret-until-revoked ec2__iam__kms__secretsmanager.ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place ec2__iam__kms__secretsmanager.ecsm-secret-under-key-pending-deletion-fails-decryption-until-reenabled ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped ec2__kms.sep-kms-key-scope-ebs-volume-under-a-disabled-key-never-becomes-available-rejected ec2__lambda__s3.lambda-log-group-outlives-delete-function ec2__s3__sqs__ssm.s3pointer-oversized-inline-payload-is-rejected-and-never-enqueued-while-the-pointer-form-is-accepted ec2__lambda__s3__sqs.send-message-to-absent-queue-rejected-with-live-control ec2__emr__iam.cluster-in-nonexistent-subnet-rejected ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-kms-grant-unlocks-secret-until-revoked|_refusal#8|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place|_refusal#8|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-secret-under-key-pending-deletion-fails-decryption-until-reenabled|_refusal#9|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped|run#12|AssertionError|assert (code, message) == ("ResourceNotFoundException", expected), (
Issue-Key: ec2__kms.sep-kms-key-scope-ebs-volume-under-a-disabled-key-never-becomes-available-rejected|run#19|AssertionError|assert fate_a == "InvalidVolume.NotFound", (
Issue-Key: ec2__lambda__s3.lambda-log-group-outlives-delete-function|run#5|KeyError|assert created["LoggingConfig"]["LogGroup"] == group, (
Issue-Key: ec2__s3__sqs__ssm.s3pointer-oversized-inline-payload-is-rejected-and-never-enqueued-while-the-pointer-form-is-accepted|run#18|AssertionError|assert code == ("InvalidParameterValue", too_long), (
Issue-Key: ec2__lambda__s3__sqs.send-message-to-absent-queue-rejected-with-live-control|run#8|AssertionError|assert message == "The specified queue does not exist.", (
Issue-Key: ec2__emr__iam|ListInstances|NotImplemented
Issue-Key: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group|run#11|AssertionError|assert (attrs.get("Ec2SubnetId"), attrs.get("Ec2AvailabilityZone"), attrs.get("IamInstanceProfile")) \
Issue-Key: ec2__ecr__ecs|UploadLayerPart|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Neighbouring-service fidelity gaps reached through EC2-led checks: Secrets Manager ignores KMS key state and grants, EBS ignores a disabled key, Lambda omits LoggingConfig, SQS misclassifies an oversized body, and EMR/ECR stubs block setup (9 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Nine root causes, all grounded in failing checks whose check-id leading token is ec2 (cross-service scenarios). All nine must be closed before submitting.

Cause 1 of 9: Secrets Manager GetSecretValue ignores the state of, and the grants on, the secret's KMS key.
AWS: GetSecretValue decrypts through kms:Decrypt on the secret's KmsKeyId, so the call FAILS when the key is disabled or pending deletion, and succeeds only while a covering grant exists (Smithy com-amazonaws-secretsmanager-secretsmanager: GetSecretValue errors include DecryptionFailure, InvalidRequestException, ResourceNotFoundException).
Emulator: three scenarios that AWS refuses return HTTP 200 with the full SecretString ('grant-route', 'rekeyed-value', 'survives').
Why: a client cannot model the key-state kill switch that is the whole point of a customer-managed key, and secret-access revocation is untestable.
Fix: Secrets Manager GetSecretValue must consult KMS key state/grants (secretsmanager state + its KMS integration).
Traces: ec2__iam__kms__secretsmanager.ecsm-kms-grant-unlocks-secret-until-revoked; ...ecsm-rekey-to-aws-managed-key-unlocks-current-version-in-place; ...ecsm-secret-under-key-pending-deletion-fails-decryption-until-reenabled.

Cause 2 of 9: Secrets Manager's ResourceNotFoundException message leaks the requested name.
AWS: documents the exact text "Secrets Manager can't find the specified secret." for a missing secret; the code is ResourceNotFoundException.
Emulator: same code, message "Secrets Manager can't find the specified secret: sim2real-ecsm-missing-44e9fd160cbc".
Why: the documented message is the rule here, and a client that matches it (or an auditor checking that names are not echoed) diverges.
Fix: the not-found path of the GetSecretValue handler.
Traces: ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped.

Cause 3 of 9: an EBS volume requested under a DISABLED KMS key still becomes available.
AWS: the volume cannot be created under a disabled key — it is withdrawn, and DescribeVolumes answers InvalidVolume.NotFound.
Emulator: DescribeVolumes reports the volume in state 'available'.
Why: encryption-key scoping has no effect, so a client cannot rely on key state to gate storage creation.
Fix: KMS key-state evaluation in the EC2 CreateVolume handler / volume state machine.
Traces: ec2__kms.sep-kms-key-scope-ebs-volume-under-a-disabled-key-never-becomes-available-rejected.

Cause 4 of 9: Lambda function responses omit LoggingConfig.
AWS: CreateFunction/GetFunctionConfiguration return LoggingConfig (LogFormat, LogGroup, ...) for every function, which is how a client learns the log group a function writes to (Smithy com-amazonaws-lambda-awsgirapiservice: FunctionConfiguration.LoggingConfig).
Emulator: the member is absent — KeyError: 'LoggingConfig'.
Why: a client cannot discover or assert the function's log group, so log-retention/teardown logic breaks.
Fix: the Lambda function-configuration serializer / function state.
Traces: ec2__lambda__s3.lambda-log-group-outlives-delete-function.

Cause 5 of 9: SQS SendMessage refuses an oversized body with the wrong error code.
AWS: a body one byte over the queue's 262144-byte maximum is refused with InvalidParameterValue and the message 'One or more parameters are invalid. Reason: Message must be shorter than 262144 bytes.' (Smithy com-amazonaws-sqs-amazonsqs: SendMessage.MessageBody with the queue's MaximumMessageSize attribute; InvalidParameterValue is the modeled refusal).
Emulator: ValidationException with 'The message body and its attributes must be at most 262144 bytes together; got 262145 bytes (0 of them in attributes).'
Why: ValidationException is not in SQS's error vocabulary for this case, so no client error handler matches and the size boundary cannot be branched on.
Fix: SendMessage size validation/error construction in the SQS handler.
Traces: ec2__s3__sqs__ssm.s3pointer-oversized-inline-payload-is-rejected-and-never-enqueued-while-the-pointer-form-is-accepted.

Cause 6 of 9: SQS's nonexistent-queue message is not AWS's documented text.
AWS: documents 'The specified queue does not exist.' for a queue URL that was never created.
Emulator: 'The queue referred to by https://<host>/000000000000/<name> does not exist. Ensure that the QueueUrl is correct and that the queue has not been deleted.' (the host/port shown is environment-specific and not itself a requirement).
Why: the documented message is the rule; a client matching it diverges.
Fix: the nonexistent-queue refusal in the SQS SendMessage handler.
Traces: ec2__lambda__s3__sqs.send-message-to-absent-queue-rejected-with-live-control.

Cause 7 of 9: EMR ListInstances answers 501 NotImplemented.
AWS: elasticmapreduce:ListInstances returns the cluster's Instances (Smithy com-amazonaws-emr-elasticmapreduce: ListInstances -> ListInstancesOutput.Instances; errors InternalServerException, InvalidRequestException).
Emulator: NotImplemented ('operation ListInstances is scaffolded but not implemented in this emulator'), aborting the check before any EMR assertion.
Why: no client can enumerate the instances of a cluster it created.
Fix: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_instances.rs.
Traces: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected.

Cause 8 of 9: EMR DescribeCluster's Ec2InstanceAttributes omits Ec2AvailabilityZone.
AWS: Ec2InstanceAttributes carries Ec2SubnetId, Ec2AvailabilityZone and IamInstanceProfile (Smithy com-amazonaws-emr-elasticmapreduce: Ec2InstanceAttributes.Ec2AvailabilityZone).
Emulator: the structure returns only Ec2SubnetId and IamInstanceProfile.
Why: a client cannot learn which AZ its cluster landed in, so placement assertions and AZ-aware scheduling fail.
Fix: the DescribeCluster serializer / cluster state in EMR.
Traces: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group.

Cause 9 of 9: ECR UploadLayerPart answers 501 NotImplemented, so no image can be pushed.
AWS: ecr:UploadLayerPart accepts a layer-part blob and returns 200 with registryId/repositoryName/uploadId/lastByteReceived (Smithy com-amazonaws-ecr-amazonec2containerregistry-v20150921).
Emulator: NotImplemented ('operation UploadLayerPart is scaffolded but not implemented in this emulator').
Why: nothing can be pushed to ECR, so every pull-failure/retag scenario is unreachable.
Fix: services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/upload_layer_part.rs. Open issue 20261022T120642Z-sagemaker-behavioral-checks-blocked-by-neighbouring-service-gaps.md mentions this stub for the sagemaker-led checks; it does not carry this check.
Traces: ec2__ecr__ecs.ecsnrt-fargate-task-on-unpushed-tag-fails-pull-until-retagged.

## Evidence
Cause 1 — AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ARN': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-ecsm-grant-...', 'SecretString': 'grant-route', ... 'HTTPStatusCode': 200} (and the same shape for the rekey and pending-deletion checks).
Cause 2 — AssertionError: with a wildcard grant a missing name is ResourceNotFoundException "Secrets Manager can't find the specified secret.", got 'ResourceNotFoundException': "Secrets Manager can't find the specified secret: sim2real-ecsm-missing-44e9fd160cbc".
Cause 3 — AssertionError: a volume requested under the disabled key A must be withdrawn (DescribeVolumes -> InvalidVolume.NotFound), got 'available'.
Cause 4 — KeyError: 'LoggingConfig'.
Cause 5 — AssertionError: ... must be refused with InvalidParameterValue 'One or more parameters are invalid. Reason: Message must be shorter than 262144 bytes.'; SQS returned ('ValidationException', 'The message body and its attributes must be at most 262144 bytes together; got 262145 bytes (0 of them in attributes).').
Cause 6 — AssertionError: sending to a never-created queue read 'The queue referred to by https://127.0.0.1:42743/000000000000/sim2real-scn-absent-... does not exist. Ensure that the QueueUrl is correct and that the queue has not been deleted.', expected 'The specified queue does not exist.'
Cause 7 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListInstances operation: operation ListInstances is scaffolded but not implemented in this emulator.
Cause 8 — AssertionError: Ec2InstanceAttributes should resolve to (subnet, 'us-west-2a', profile), got {'Ec2SubnetId': '...', 'IamInstanceProfile': '...'}.
Cause 9 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UploadLayerPart operation: operation UploadLayerPart is scaffolded but not implemented in this emulator.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) create a secret under a customer KMS key, kms:DisableKey (or ScheduleKeyDeletion), then secretsmanager:GetSecretValue. (2) secretsmanager:GetSecretValue on a name that does not exist. (3) ec2:CreateVolume Encrypted=True KmsKeyId=<disabled key> then ec2:DescribeVolumes. (4) lambda:CreateFunction then lambda:GetFunctionConfiguration and read LoggingConfig. (5) sqs:SendMessage with a 262145-byte body. (6) sqs:SendMessage to a never-created queue URL. (7) emr:RunJobFlow then emr:ListInstances. (8) emr:DescribeCluster and read Cluster.Ec2InstanceAttributes.Ec2AvailabilityZone. (9) ecr:InitiateLayerUpload then ecr:UploadLayerPart.
