Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative|run#22|AssertionError|assert negative_described["ProcessingJobStatus"] == "Failed", negative_described
Issue-Commit: a08b74f330a46ca29ef987efae864177fd1dcc74

# s3__sagemaker: a processing job whose execution role lacks kms:Decrypt on the CMK protecting its SSE-KMS input still reports Completed, so the KMS/IAM input-decrypt boundary is inert

> _Note: this issue was identified against the code base at commit `a08b74f330a46ca29ef987efae864177fd1dcc74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProcessingJob

## Description
Cause 1 of 1: the processing-job execution path does not evaluate the execution role's permissions on the KMS key that protects its SSE-KMS-encrypted S3 input, so a job that AWS ends Failed (having downloaded nothing and written nothing) instead runs to Completed and writes its output.

AWS: CreateProcessingJob is accepted and the job's input download is performed by the declared RoleArn. When the input object is encrypted with SSE-KMS (ServerSideEncryption=aws:kms, SSEKMSKeyId=<cmk>) and that role has no kms:Decrypt grant on the CMK, the download is refused and the job reaches a terminal ProcessingJobStatus of Failed with a FailureReason attributing the terminal state to the access-denied on the KMS key (Smithy: DescribeProcessingJob output ProcessingJobStatus (required, enum) and FailureReason (@length max 1024) / ExitMessage; the specific status value Failed and the exact FailureReason wording for a KMS-denied input download are recorded behavior only — the recording shows the job terminal as Failed with nothing written under the declared output prefix). An identical job submitted with a role that DOES hold kms:Decrypt reaches Completed and writes the declared output object — that positive control is what fixes the job-execution path as the only difference between the two.

Emulator: both jobs behave identically. The negative job — submitted with the role that has no kms:Decrypt on the CMK (RoleArn arn:aws:iam::<account>:role/<role-without-decrypt>) over the same s3://<bucket>/secure-input/ prefix whose object was PUT with ServerSideEncryption=aws:kms — is reported by DescribeProcessingJob as ProcessingJobStatus 'Completed', with ProcessingStartTime/ProcessingEndTime set, no FailureReason and no ExitMessage. Note this is NOT the old 'no container runtime' behavior: the positive control in the same check completed and its output object was readable by HeadObject, so processing jobs now execute — what is missing is the permission/decrypt verdict. The open issue 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md (Cause 2) describes the neighbouring, older symptom (processing jobs never running at all) and does not carry this check's replay key; it does not cover this cause.

Why: a client relying on AWS's behavior treats Completed as 'the container read my encrypted input and produced my output'. Here the data-protection boundary expressed by the CMK's key policy and the role's missing kms:Decrypt is silently ignored, so every test or control that asserts an unauthorized role cannot read KMS-encrypted training/processing data passes vacuously, and a job that AWS would have failed appears to have succeeded (with output written under the prefix the check expects to stay empty).

Fix: the CreateProcessingJob execution path — the worker that downloads ProcessingInputs[].S3Input from S3 under the job's RoleArn must evaluate that role against the object's SSE-KMS CMK (kms:Decrypt) as well as its S3 grants, and on refusal drive the job's state machine to the terminal Failed status with a FailureReason naming the access-denied on that key instead of completing the job and writing ProcessingOutputConfig outputs.

Traces: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative (the positive control in the same check passes; only the negative diverges).

## Evidence
Check s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative, mismatch at `assert negative_described["ProcessingJobStatus"] == "Failed"`.

Expected (AWS): DescribeProcessingJob on the job submitted with the role lacking kms:Decrypt returns ProcessingJobStatus='Failed' with a FailureReason attributing it to the KMS access denial, and ListObjectsV2 on the job's output prefix returns KeyCount=0.

Actual (emulator): DescribeProcessingJob returns ProcessingJobStatus='Completed' with no FailureReason/ExitMessage, e.g. {'ProcessingInputs': [{'InputName': 'input', 'S3Input': {'S3Uri': 's3://<bucket>/secure-input/', 'S3DataType': 'S3Prefix', 'S3InputMode': 'File', ...}}], 'ProcessingOutputConfig': {'Outputs': [{'OutputName': 'output', 'S3Output': {'S3Uri': 's3://<bucket>/secure-output-negative/', 'S3UploadMode': 'EndOfJob', ...}}]}, 'RoleArn': 'arn:aws:iam::<account>:role/<role-without-decrypt>', 'ProcessingJobStatus': 'Completed', 'ProcessingStartTime': <t0>, 'ProcessingEndTime': <t0+~0.4s>}. The role-with-decrypt twin in the same check also reached 'Completed' and its output object was found by HeadObject, so the two differ in nothing but the IAM/KMS grant the emulator never consults.

## Steps to Reproduce
1. Deploy (or create directly) a bucket, a KMS key, and two IAM roles trusting sagemaker.amazonaws.com: one with s3 access AND kms:Decrypt/kms:GenerateDataKey on the key, one with the same s3 access but NO kms permissions.
2. PutObject s3://<bucket>/secure-input/data.csv with ServerSideEncryption='aws:kms' and SSEKMSKeyId=<key id>.
3. CreateProcessingJob twice with identical AppSpecification/ProcessingInputs (S3Uri s3://<bucket>/secure-input/, S3DataType=S3Prefix, S3InputMode=File) and ProcessingOutputConfig differing only in output prefix, passing RoleArn = the role WITH kms:Decrypt for one and the role WITHOUT it for the other.
4. Poll DescribeProcessingJob on both until ProcessingJobStatus is terminal.
5. Observe: the role-without-kms:Decrypt job reports 'Completed' (expected 'Failed' with a KMS access-denied FailureReason), and ListObjectsV2 on its output prefix is expected to be empty.
