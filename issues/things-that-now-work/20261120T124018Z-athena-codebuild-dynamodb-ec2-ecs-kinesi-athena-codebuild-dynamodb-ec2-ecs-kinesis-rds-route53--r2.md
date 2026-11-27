Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys
Issue-Key: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Commit: 0139b649892491ec475f7b8e31445a257f779000

# athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm: a CDK AwsCustomResource (Custom::AWS) never delivers its response, so CloudFormation fails the stack with "did not receive a response from your Custom Resource" and the 30-stack estate cannot deploy

> _Note: this issue was identified against the code base at commit `0139b649892491ec475f7b8e31445a257f779000`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a Lambda-backed CDK custom resource (Custom::AWS, the AwsCustomResource provider CDK synthesizes for data seeding) is never completed — CloudFormation never receives its response, so the resource's create fails and the whole estate deploy is rolled back.
AWS: when a stack contains a Custom::AWS resource, CloudFormation invokes the provider function with the custom-resource request (RequestType Create, ResponseURL, StackId, RequestId, LogicalResourceId); the provider PUTs its SUCCESS/FAILED response document to the presigned ResponseURL and CloudFormation then moves that resource to CREATE_COMPLETE. A stack whose provider does answer reaches CREATE_COMPLETE and `cdk deploy --all` exits 0. Recorded behavior only (the custom-resource response protocol is not in the Smithy model; CloudFormation models only the resource's status and the stack's rollback).
Emulator: `cdk deploy --all` exits 1 and the failing resource is reported as 'sim2real-ds-<id>-TagTable/PopulateData/Resource/Default (Custom::AWS PopulateData54FDD7A7) — CloudFormation did not receive a response from your Custom Resource. Please check your logs for requestId [<uuid>]' (fixed text; the logical path, resource type and request id are the stack's own values), i.e. DeploymentError 'Resource updates failed'. Either the provider function is never invoked or its response to the presigned ResponseURL is not accepted/correlated, so the resource never leaves its in-progress state and the deploy of the 30-stack, two-Region estate never completes.
Why: every CDK construct that seeds or reads data through AwsCustomResource (and anything built on custom_resources.AwsCustomResource/Provider) wedges or fails the deploy, so no CDK app that uses one can be exercised against the emulator at all.
Fix: the custom-resource execution path of the stack executor — invoke the provider function with the request document and accept/correlate the response PUT to the presigned ResponseURL, then advance the resource's status.
Related open issues of the same shape under OTHER service tokens, which do NOT cover this check: 20261117T115022Z-cloudformation-ecr-iam-logs-s3-ssm-sts-2.md, 20261117T115047Z-cloudformation-ecr-iam-logs-s3-ssm-sts-2.md, 20261105T204906Z-cloudformation-dynamodb-lambda-a-custom-resources-provider-custom.md.
Traces: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys.

## Evidence
Expected: `cdk deploy --all` exits 0 and every stack of the estate reaches CREATE_COMPLETE.
Actual: AssertionError: `cdk deploy --all` exited 1, expected 0; output tail: '❌ sim2real-ds-<id>-TagTable failed to deploy ‣ DeploymentError: Resource updates failed: sim2real-ds-<id>-TagTable/PopulateData/Resource/Default (Custom::AWS PopulateData54FDD7A7) CloudFormation did not receive a response from your Custom Resource. Please check your logs for requestId [<uuid>].' (the remaining output is CDK's own minHealthyPercent/circuitBreaker annotations, which are advisory warnings, not failures.)

## Steps to Reproduce
Synthesize a CDK stack containing a DynamoDB table plus an AwsCustomResource (Custom::AWS) that writes seed items on Create, and run `cdk deploy --all` (the suite's cdk_storage_estate app, 30 stacks across us-east-1/us-west-2). Watch the TagTable stack's PopulateData resource: CloudFormation should invoke the provider function and accept its response to the presigned ResponseURL, taking the resource to CREATE_COMPLETE. Replay the check id in trace_ids.

## Fix note (what was closed, and what this report does not demonstrate)
Closed: a provider function whose MODULE BODY imported a node standard-library module this runtime did not supply — `console` among them, which is what a bundled provider builds its logger over (`new Console({ stdout: process.stdout })`), beside the `child_process` import its SDK installer uses — never loaded, so its handler was never entered, nothing was PUT to the request's `ResponseURL`, and CloudFormation failed the resource with the no-response sentence above. The node runtime now supplies that library (`console` and the modules beside it), with only the CALL that needs an absent host capability refused, and the console it installs carries the whole surface a program calls on it. Pinned by `services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/node_standard_library_modules.rs` and, at this report's own level, by `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/standard_library_provider_custom_resource.rs`.

Not demonstrated: the 30-stack estate check itself still exits 1 on replay, unchanged in shape. The check's captured output tail excludes the failure sentence (the window begins inside the CDK CLI's own stack trace and ends in CDK's ECS annotations), so the remaining divergence names neither a resource nor a reason and could not be attributed from the replay.
