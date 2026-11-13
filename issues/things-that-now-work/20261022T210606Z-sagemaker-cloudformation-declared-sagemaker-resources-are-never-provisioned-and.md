Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-stack-positive s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-role-positive sagemaker.deploy-notebook-instance-cfn-invalid-role-rejected sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds sagemaker.deploy-notebook-instance-cfn-presigned-url-works sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role s3__sagemaker.deploy-cfn-sagemaker-model-s3-artifact-reference-positive s3__sagemaker.deploy-cfn-sagemaker-notebook-instance-role-s3-access-positive sagemaker.deploy-notebook-instance-cfn-behaves
Issue-Key: s3__sagemaker|DescribeModel|ValidationException
Issue-Key: sagemaker|DescribeNotebookInstance|ValidationException
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role|_deploy_cfn#6|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: s3__sagemaker.deploy-cfn-sagemaker-model-s3-artifact-reference-positive|_deploy_cfn#11|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: s3__sagemaker.deploy-cfn-sagemaker-notebook-instance-role-s3-access-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Related: 20261022T120642Z-sagemaker-behavioral-checks-blocked-by-neighbouring-service-gaps.md
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# CloudFormation-declared SageMaker resources are never provisioned and stack Outputs carry no OutputValue, so every CFN-deployed SageMaker scenario fails (3 causes)

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNotebookInstance

## Description
Three independent defects on the CloudFormation-deploy path into SageMaker. Each is its own root cause and its own work item; this issue bundles THREE causes and all three must be closed before submitting. Note the direct APIs work: CreateModel/DescribeModel and CreateNotebookInstance/DescribeNotebookInstance succeed when called directly in other checks, so the gap is the CFN resource provider, not those handlers.

Cause 1 of 3: a stack declaring AWS::SageMaker::Model reaches CREATE_COMPLETE but no model exists in SageMaker.
AWS: recorded behavior only - after the stack completes, DescribeModel on the model the template declares returns the model (its PrimaryContainer and ModelDataUrl as templated). Smithy: DescribeModel's only modeled error is a client 400; a missing model is rejected 400 ValidationException.
Emulator: the stack reports success, then DescribeModel on the declared model is rejected 400 ValidationException ("Could not find model ..."), so the resource was never created.
Why: an IaC-deployed inference stack silently contains nothing - the caller believes the model exists and every downstream EndpointConfig/Endpoint reference fails for a reason the stack never reported.
Fix: the CloudFormation resource provider for AWS::SageMaker::Model (create the model in SageMaker state during stack create/update and fail the stack if it cannot).
Traces: s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-stack-positive.

Cause 2 of 3: a stack declaring AWS::SageMaker::NotebookInstance reaches CREATE_COMPLETE but no notebook instance exists.
AWS: recorded behavior only - once the stack completes, DescribeNotebookInstance on the declared instance returns it (eventually NotebookInstanceStatus InService) and CreatePresignedNotebookInstanceUrl can be minted against it. A notebook instance that does not exist is rejected 400 ValidationException.
Emulator: the stack reports success, then DescribeNotebookInstance is rejected 400 ValidationException "RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>" - nothing was provisioned.
Why: every CFN-deployed notebook scenario (role wiring, presigned URL, invalid-role rejection) is unobservable, and the stack's own status lies about what it created.
Fix: the CloudFormation resource provider for AWS::SageMaker::NotebookInstance (provision into SageMaker state, propagate RoleArn/LifecycleConfigName, and surface create failures as a stack failure/rollback).
Traces: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive, s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-role-positive, sagemaker.deploy-notebook-instance-cfn-invalid-role-rejected, sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds, sagemaker.deploy-notebook-instance-cfn-presigned-url-works, sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible.

Cause 3 of 3: DescribeStacks returns Outputs entries with no OutputValue.
AWS: recorded behavior only - for a CREATE_COMPLETE stack whose template declares Outputs, each Output in DescribeStacks carries OutputKey AND the resolved OutputValue (here the bucket name, the role ARN, the notebook-instance name).
Emulator: the Outputs list is present but each entry lacks the OutputValue key, so the consuming code raises KeyError: 'OutputValue' and no stack-produced identifier can be read.
Why: nothing deployed by template can be wired to a subsequent API call; the stack is indistinguishable from an empty one.
Fix: stack-output resolution/serialization in the cloudformation service (resolve !Ref/!GetAtt to physical ids and emit OutputValue). Related open issues that report the same serializer gap for other services - this one is grounded in the SageMaker deploy checks and is listed here because the report did not mark these checks as owned: 20261022T202249Z-autoscaling-cloudformation-ec2-cloudformation-describestacks-returns-stack-outputs-entries-without-outputvalue.md, 20261022T043627Z-fis-describestacks-omits-the-outputs-member-for-a-stack.md.
Traces: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-with-s3-role, s3__sagemaker.deploy-cfn-sagemaker-model-s3-artifact-reference-positive, s3__sagemaker.deploy-cfn-sagemaker-notebook-instance-role-s3-access-positive, sagemaker.deploy-notebook-instance-cfn-behaves.

## Evidence
Cause 1: "botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the DescribeModel operation: Could not find model \"arn:aws:sagemaker:us-west-2:000000000000:model/sim2real-xsvc-model-...\"" after the stack declaring AWS::SageMaker::Model completed.
Cause 2: "botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the DescribeNotebookInstance operation: RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:us-west-2:000000000000:notebook-instance/sim2real-..." in six checks, each after its stack reported completion.
Cause 3: "KeyError: 'OutputValue'" raised while reading DescribeStacks Outputs in four checks.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::SageMaker::Model (ExecutionRoleArn + PrimaryContainer.ModelDataUrl) and an Output for its name; wait for CREATE_COMPLETE.
2. DescribeModel(ModelName=<declared name>) -> expect the model; observe 400 ValidationException "Could not find model ...".
3. CreateStack with a template declaring AWS::SageMaker::NotebookInstance (InstanceType ml.t3.medium, RoleArn from an AWS::IAM::Role in the same stack); wait for CREATE_COMPLETE, then DescribeNotebookInstance(NotebookInstanceName=<declared name>) -> expect the instance; observe 400 ValidationException RecordNotFound.
4. DescribeStacks(StackName=<either stack>) and read Stacks[0].Outputs -> every entry is missing OutputValue.
