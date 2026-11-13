Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.deploy-table-bucket-via-cloudformation-behaves s3tables.e2e-create-s3-tables-with-compaction-lifecycle
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|CreateTable|NotFoundException
Issue-Key: s3tables.e2e-create-s3-tables-with-compaction-lifecycle|_settle#13|Timeout|rows = fetch()
Issue-Commit: f1473582b6746d116724ae43d0fc6ffbe3d6e185

# A CloudFormation-deployed AWS::S3Tables::Namespace does not exist in S3 Tables: CreateTable in it answers 404 NotFoundException and ListNamespaces never lists it

> _Note: this issue was identified against the code base at commit `f1473582b6746d116724ae43d0fc6ffbe3d6e185`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNamespace

## Description
Cause 1 of 1: CloudFormation reports an AWS::S3Tables::Namespace resource as created, but no namespace is ever created in the S3 Tables table bucket, so the deployed estate is non-functional even though the stack is CREATE_COMPLETE.

AWS: the AWS::S3Tables::Namespace resource provider calls S3 Tables CreateNamespace on the referenced table bucket (Smithy com-amazonaws-s3tables-s3tablebuckets: CreateNamespace PUT /namespaces/{tableBucketARN}, 200, required input tableBucketARN + namespace (NamespaceList, @length min 1 max 1), output echoes tableBucketARN + namespace). After the stack reaches CREATE_COMPLETE the namespace is real: ListNamespaces (GET /namespaces/{tableBucketARN}, 200, required `namespaces`) lists exactly that namespace for the bucket, and CreateTable(tableBucketARN, namespace, name, format=ICEBERG) succeeds and GetTable reads the table back with namespace == [<namespace>]. A namespace that does not exist is what NotFoundException (@httpError 404) is for; here it must not be raised, because the stack created the namespace.

Emulator: the stack reaches CREATE_COMPLETE and the bucket itself is usable, but the namespace is absent from S3 Tables state. CreateTable against the stack-declared namespace is refused `404 NotFoundException: The specified namespace does not exist.` (that message is the correct template for a genuinely missing namespace — the defect is that the namespace is missing, not the wording), and ListNamespaces on the deployed bucket keeps returning an empty `namespaces` list, so a poll for the deployed namespace never settles and the e2e lifecycle check is cut off at its budget. Hypothesis: the AWS::S3Tables::Namespace handler records a physical id without calling CreateNamespace on the bucket; the second check's stall point (ListNamespaces polling in its deploy phase) is inferred from that evidence, since the run reports only the timeout.

Why: a client that provisions an S3 Tables estate declaratively gets a green stack and a bucket with no namespaces. Every subsequent table create, Iceberg load/commit and listing against the templated namespace fails with a 404 that contradicts CloudFormation's own CREATE_COMPLETE, and tools that wait for the namespace to appear hang instead of failing fast.

Fix: the AWS::S3Tables::Namespace resource handler and the s3tables namespace state it must write — the create path has to materialize the namespace under the referenced table bucket (and the delete path remove it), so the control-plane reads (ListNamespaces, CreateTable/GetTable) see it.

Traces: s3tables.deploy-table-bucket-via-cloudformation-behaves (CreateTable → NotFoundException after CREATE_COMPLETE); s3tables.e2e-create-s3-tables-with-compaction-lifecycle (deploy phase never sees the namespace in ListNamespaces; check cut off at 60 s).

## Evidence
Expected (check deploy-table-bucket-via-cloudformation-behaves): after a stack with AWS::S3Tables::TableBucket + AWS::S3Tables::Namespace {TableBucketARN: {Fn::GetAtt: [TableBucket, TableBucketARN]}, Namespace: <ns>} reaches CREATE_COMPLETE, s3tables CreateTable(tableBucketARN=<bucket arn>, namespace=<ns>, name=<tbl>, format=ICEBERG) returns 200 with a tableARN under <bucket arn>/table/, and GetTable echoes name/namespace([<ns>])/format.
Actual: botocore.errorfactory.NotFoundException — An error occurred (NotFoundException) when calling the CreateTable operation: The specified namespace does not exist. (HTTP 404).

Expected (check e2e-create-s3-tables-with-compaction-lifecycle): after the estate stack is CREATE_COMPLETE, ListNamespaces on the exported TableBucketARN returns exactly one row whose namespace is the stack's NamespaceName parameter.
Actual: the listing stays empty, the check's settle loop polls until the run budget expires — "Check timed out after 60.0s" — so none of the Iceberg trajectory after deploy is reached.

## Steps to Reproduce
1. CreateStack with Resources {TableBucket: {Type: AWS::S3Tables::TableBucket, Properties: {TableBucketName: <name>}}, Namespace: {Type: AWS::S3Tables::Namespace, Properties: {TableBucketARN: {Fn::GetAtt: [TableBucket, TableBucketARN]}, Namespace: <ns, [0-9a-z_] only>}}} and an Output exporting the bucket ARN; wait for CREATE_COMPLETE.
2. Call s3tables ListNamespaces(tableBucketARN=<exported arn>) — the namespaces list is empty.
3. Call s3tables CreateTable(tableBucketARN=<exported arn>, namespace=<ns>, name=<tbl>, format=ICEBERG) — 404 NotFoundException "The specified namespace does not exist."
4. Replay checks s3tables.deploy-table-bucket-via-cloudformation-behaves and s3tables.e2e-create-s3-tables-with-compaction-lifecycle.
