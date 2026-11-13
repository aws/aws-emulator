Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive|run#21|AssertionError|assert final_status == "UPDATE_COMPLETE", final_status
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# s3__sagemaker: 2 net-new causes — a notebook instance's OnStart lifecycle script never runs (no S3 side effect), and a stack update that replaces AWS::SageMaker::Model's ModelDataUrl rolls back

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotebookInstance

## Description
TWO net-new behavioral root causes where SageMaker meets S3 through CloudFormation. Neither failing check is marked ALREADY FILED or OWNED by the replay. Both must be closed before this issue is submitted.

Cause 1 of 2: a notebook instance created with a NotebookInstanceLifecycleConfig never executes its OnStart script, so the script's S3 side effect never appears.
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateNotebookInstanceLifecycleConfig takes OnCreate/OnStart (NotebookInstanceLifecycleConfigList, @length 0..1) whose Content is a base64 shell script, and DescribeNotebookInstance echoes NotebookInstanceLifecycleConfigName. AWS runs the OnStart script server-side as the instance starts, under the instance's ExecutionRole (RoleArn), before/while the instance reaches NotebookInstanceStatus='InService'; anything the script does with the role's permissions actually happens — an `aws s3 cp` in the script puts the object into the bucket, and a subsequent read of it from the script writes the read-back object too. Recorded behavior only for the timing; the requirement is that the hook's effects are observable at the destination (S3) for a notebook that reached InService with a role granting s3:GetObject/PutObject/ListBucket on that bucket.
Emulator: the lifecycle config and the instance are created and the instance reports InService, but no object the OnStart script writes ever appears in the bucket — both checks poll the destination (GetObject on the marker/write/verify keys) until the 60 s harness cap and never see it, so the hook's effect, and with it the 'the notebook's role really can use this bucket' contract, is unobservable.
Why: lifecycle configs exist to bootstrap a notebook (install packages, stage data, write a readiness marker). A client can register one and see InService, but nothing it does happens, so provisioning automation silently no-ops and any test that proves the execution role is usable from the running resource can never pass. The resolved issue 20261029T222441Z-sagemaker-performs-no-job-notebook-hook-cluster-node.md recorded this hook as platform-blocked for the OnCreate/CloudWatch-Logs observable (its cause 7); these two checks observe the OnStart script's S3 side effect instead and are not carried by it, and a resolved issue does not cover them.
Fix: the notebook-instance lifecycle-config execution path behind CreateNotebookInstance/StartNotebookInstance in the sagemaker crate — the OnStart content must be executed (at minimum its AWS-API effects performed under the instance's RoleArn) before the instance is reported InService.
Traces: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive, s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role.

Cause 2 of 2: updating a stack so that AWS::SageMaker::Model's ModelDataUrl changes ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
AWS: PrimaryContainer.ModelDataUrl is an immutable property of AWS::SageMaker::Model, so changing it is a REPLACEMENT update: CloudFormation creates a new model from the new S3 artifact, swaps the stack's Ref/Outputs to it, deletes the old physical model, and the stack reaches UPDATE_COMPLETE. Afterwards DescribeModel on the new name reports the new ModelDataUrl and DescribeModel on the old name is refused with ValidationException 'Could not find model "<old model name>".' (fixed text 'Could not find model "' ... '".' with the old model's name interpolated). The creating path works in the emulator: the same template at the first key reached CREATE_COMPLETE and DescribeModel reported the expected s3://<bucket>/<key>.
Emulator: the UpdateStack is accepted but the stack settles in UPDATE_ROLLBACK_COMPLETE, so the replacement never happens: the stack keeps the old model, the new ModelDataUrl is never published in Outputs, and neither the new-model read nor the old-model-gone refusal is reachable.
Why: re-pointing a model at a newly trained artifact is the normal way a SageMaker stack ships a new model version. A client that does it gets a rolled-back stack and keeps serving the previous artifact with no error of its own to act on; every model-promotion pipeline built on CloudFormation is blocked.
Fix: the CloudFormation provisioner for AWS::SageMaker::Model — the update path must detect the immutable-property change, create the replacement model, repoint the stack's Ref/Outputs, and delete the old model instead of failing the resource update. The open issue 20261111T111216Z-cloudformation-sagemaker-aws-sagemaker-model-physical-resource-id-is-the.md concerns that resource's PhysicalResourceId/tag retention on an in-place update and does not carry this check.
Traces: s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive.

## Evidence
Cause 1 — s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive [mismatch] and s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role [mismatch]:
  Error: Check timed out after 60.0s
Both checks deploy a stack (S3 bucket + IAM role trusting sagemaker.amazonaws.com with s3:GetObject/PutObject/ListBucket on that bucket + AWS::SageMaker::NotebookInstanceLifecycleConfig whose OnStart script does `aws s3 cp` + AWS::SageMaker::NotebookInstance using both), then poll the destination:
  _poll_until(_probe, lambda b: b is not None, timeout_s=180.0, description="marker object s3://<bucket>/<key> written by notebook OnStart script")
  _poll_until(lambda: _read(write_key), ..., description="OnStart PutObject to s3://<bucket>/notebook-role-probe/write.txt via the notebook role")
The stack reaches CREATE_COMPLETE and DescribeNotebookInstance reports InService; the polled objects never appear, so the checks are cut at the harness cap. The bucket, key and payload values are per-run; the requirement is that the OnStart script's effects exist at the destination.

Cause 2 — s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive [mismatch]:
  assert final_status == "UPDATE_COMPLETE", final_status
  AssertionError: UPDATE_ROLLBACK_COMPLETE
The create half passed: the model stack reached CREATE_COMPLETE and DescribeModel reported PrimaryContainer.ModelDataUrl == s3://<bucket>/model-artifacts/v1/model.tar.gz. The update changes only the ModelDataKey parameter to model-artifacts/v2/model.tar.gz (the object was uploaded first). The check then expected a new physical model name in Outputs, DescribeModel reporting the v2 URL, and DescribeModel on the old name refused with ValidationException 'Could not find model "<old name>".' The key names and model names are per-run values.

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateNotebookInstanceLifecycleConfig (NotebookInstanceLifecycleConfigName required; OnCreate/OnStart @length 0..1; error ResourceLimitExceeded); DescribeNotebookInstance (output NotebookInstanceStatus, RoleArn @pattern ^arn:aws[a-z\-]*:iam::\d{12}:role/..., NotebookInstanceLifecycleConfigName).

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run the three checks in trace_ids. Manually:

Cause 1: CreateStack with an S3 bucket, an IAM role trusting sagemaker.amazonaws.com and granting s3:ListBucket/GetObject/PutObject on that bucket, an AWS::SageMaker::NotebookInstanceLifecycleConfig whose OnStart Content is base64 of `echo marker | aws s3 cp - s3://<bucket>/<key>`, and an AWS::SageMaker::NotebookInstance (ml.t3.medium) using that role and lifecycle config. Wait for CREATE_COMPLETE and DescribeNotebookInstance Status=InService, then GetObject s3://<bucket>/<key>: AWS serves the marker written by the hook; the emulator never creates the object.

Cause 2: deploy a bucket+role stack; upload two artifacts (…/v1/model.tar.gz and …/v2/model.tar.gz); deploy a second stack with one AWS::SageMaker::Model whose PrimaryContainer.ModelDataUrl is s3://<bucket>/<ModelDataKey parameter> pointing at v1 and confirm DescribeModel reports it. Then UpdateStack with ModelDataKey=…/v2/model.tar.gz and poll DescribeStacks: expect UPDATE_COMPLETE, a NEW model name in Outputs, DescribeModel on it reporting the v2 URL, and DescribeModel on the old name refused 'Could not find model "<old name>".' The emulator settles in UPDATE_ROLLBACK_COMPLETE.
