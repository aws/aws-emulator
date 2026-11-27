Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3.cdk-bucket-deployment-copies-sources-and-removes-them-on-delete cloudformation__lambda__s3.cdk-bucket-deployment-into-a-missing-bucket-fails-and-rolls-back cloudformation__lambda__s3.cfn-lambda-layer-version-tags-property-rejected cloudformation__lambda__s3.deploy-s3-code-location-is-write-only-and-update-from-deleted-object-rejected
Issue-Key: cloudformation__lambda__s3.cdk-bucket-deployment-copies-sources-and-removes-them-on-delete|_deployed#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3.cdk-bucket-deployment-into-a-missing-bucket-fails-and-rolls-back|_deployed#11|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3.cfn-lambda-layer-version-tags-property-rejected|run#23|AssertionError|assert flagged == want, f"validation flagged {flagged!r}, expected {want!r}"
Issue-Key: cloudformation__lambda__s3.deploy-s3-code-location-is-write-only-and-update-from-deleted-object-rejected|run#19|AssertionError|assert (status, code) == ("IN_SYNC", {}), f"drift read gave {status!r} with Code {code!r}; expected IN_SYNC and {{}}"
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + lambda + s3: 3 net-new causes — CDK bucket deployments never get a custom-resource response and leave ROLLBACK_FAILED, a Tags property on LayerVersion is not validated, and the S3 code location leaks into the drift read

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new causes.

Cause 1 of 3: a CDK BucketDeployment (a Lambda-backed Custom::CDKBucketDeployment) never completes, and the rollback that follows cannot delete it.
AWS: the deployment custom resource runs, copies the sources into the bucket and PUTs its response, so `cdk deploy` exits 0 and the stack reaches CREATE_COMPLETE; when the target bucket does not exist the resource answers FAILED and the stack rolls back cleanly to ROLLBACK_COMPLETE with the resource deleted.
Emulator: the resource fails with `CloudFormation did not receive a response from your Custom Resource. Please check your logs for requestId [<id>]. If you are using the Python cfn-response module, you may need to update your Lambda function code so that CloudFormation can attach the updated version.` and the stack then settles ROLLBACK_FAILED with `The following resource(s) failed to delete: [DeployCustomResource218AF6A4].`; `cdk deploy` exits 1. Two distinct defects show here: the custom-resource response never reaches CloudFormation, and the rollback's delete of a never-completed custom resource fails instead of succeeding.
Why: BucketDeployment is how almost every CDK app ships static assets. The deploy fails, and because the rollback also fails the stack is left in ROLLBACK_FAILED, which blocks every later operation until it is manually continued. The negative case is just as bad: a genuinely failing deployment cannot even roll back.
Fix: the custom-resource response ingestion (the ResponseURL the provisioner hands the function and the PUT it accepts) and the rollback delete path for a resource that never reported a physical id. The open issue 20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md records the same symptom for the cloudformation__s3 suite; it does not carry these two checks.

Cause 2 of 3: a `Tags` property on AWS::Lambda::LayerVersion is accepted.
AWS: the resource schema does not declare Tags, so CloudFormation's own property validation fails the resource and DescribeEvents carries one VALIDATION_ERROR event with ValidationPath `/Resources/<LogicalId>/Properties/Tags`.
Emulator: no validation failure is flagged at all.
Why: the template deploys a layer version the real service would refuse, and a caller reading DescribeEvents to find the offending property gets nothing.
Fix: CloudFormation's pre-provision property validation for the AWS::Lambda::LayerVersion schema (same stage as the cloudformation__lambda validation gap, but this check is listed under this service).

Cause 3 of 3: the write-only S3 code location shows up in the drift read.
AWS: Code.S3Bucket/S3Key are write-only, so DetectStackResourceDrift reports IN_SYNC with an EMPTY Code block in both property sets, even after the S3 object behind the code has been deleted.
Emulator: the drift read is IN_SYNC but reports Code as 'absent' rather than `{}` — the member is projected into the comparison instead of being omitted.
Why: a drift consumer that reads the Code block to decide whether the deployed code matches the template gets a value AWS never reports.
Fix: the write-only member projection in the AWS::Lambda::Function drift path.

## Evidence
Cause 1: "stack 'sim2real-cdkbd-<id>' is 'ROLLBACK_FAILED', expected 'CREATE_COMPLETE'; failed events [('DeployCustomResource218AF6A4','CREATE_FAILED','CloudFormation did not receive a response from your Custom Resource. ...'), ('<stack>','ROLLBACK_FAILED','The following resource(s) failed to delete: [DeployCustomResource218AF6A4]. ')]; `cdk deploy` exited 1"; and the same pair for the missing-bucket case where ROLLBACK_COMPLETE was expected.
Cause 2: "validation flagged [], expected [('Shared', '/Resources/Shared/Properties/Tags')]".
Cause 3: "drift read gave 'IN_SYNC' with Code 'absent'; expected IN_SYNC and {}".

## Steps to Reproduce
Cause 1: `cdk deploy` an app whose stack contains a BucketDeployment into an existing bucket (expect exit 0 / CREATE_COMPLETE), and one into a bucket that does not exist (expect ROLLBACK_COMPLETE).
Cause 2: CreateStack with an AWS::Lambda::LayerVersion whose Properties include Tags, then DescribeEvents and look for a VALIDATION_ERROR on /Resources/<LogicalId>/Properties/Tags.
Cause 3: deploy a function whose Code is an S3 location, delete the S3 object, then DetectStackResourceDrift and inspect the Code block of the property sets.
