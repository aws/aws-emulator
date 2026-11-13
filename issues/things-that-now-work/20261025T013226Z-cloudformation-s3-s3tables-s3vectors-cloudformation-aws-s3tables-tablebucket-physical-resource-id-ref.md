Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services
Issue-Key: s3tables|GetTableBucket|ValidationException
Issue-Commit: 705dc5da304dfc9ebeeba3d270950b691cc99b27

# CloudFormation AWS::S3Tables::TableBucket physical resource id / Ref is the bare bucket name, not the table bucket ARN, so S3 Tables cannot resolve it

> _Note: this issue was identified against the code base at commit `705dc5da304dfc9ebeeba3d270950b691cc99b27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

=== Cause 1 of 1: the PhysicalResourceId CloudFormation reports for AWS::S3Tables::TableBucket is the declared bucket NAME, but S3 Tables identifies a table bucket only by ARN ===

AWS: every CloudFormation resource type's physical resource id is the identifier shape the OWNING service accepts, and the identifier shape differs per bucket variant. For AWS::S3::Bucket it is the bare bucket name (S3 APIs take `Bucket=<name>`); for AWS::S3Vectors::VectorBucket and AWS::S3Tables::TableBucket it is the bucket ARN, because those services' read operations are keyed by ARN. `Ref` of the logical resource returns that same value, so a stack Output built from `{"Ref": "Tables"}` carries the table bucket ARN and can be passed straight to S3 Tables. Recorded behavior confirms the constraint from the receiving side: S3 Tables' GetTableBucket `tableBucketARN` member only accepts `^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$`, i.e. an ARN ending in `:bucket/<name>`; anything else is refused with HTTP 400 `ValidationException`. (The S3 Tables Smithy model is not on disk in this checkout — the pattern above is quoted verbatim from the emulator's own refusal, and matches AWS. The per-variant identifier rule itself is CloudFormation resource-type documentation, not Smithy.)

Emulator: the stack reaches CREATE_COMPLETE and its resource rows/Outputs agree with each other, the regular bucket row resolves in S3 by name, and the vector bucket row resolves in S3 Vectors by ARN — but the AWS::S3Tables::TableBucket row's PhysicalResourceId is the bare declared name (e.g. `sim2real-cfns3-ids-<token>-t`). Passing that row to S3 Tables GetTableBucket is therefore refused: `An error occurred (ValidationException) when calling the GetTableBucket operation: 1 validation error detected: Value 'sim2real-cfns3-ids-87d10c03b0d0-t' at 'tableBucketARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$` (that name value is one run's example; the defect is the identifier SHAPE, not the token). The later assertion that the ARN rows end in `:bucket/<name>` is never reached.

Why: a client that reads a table bucket out of a stack — via ListStackResources/DescribeStackResources PhysicalResourceId, or via a `Ref` stack Output — gets a value S3 Tables structurally cannot accept, so every CFN-driven S3 Tables workflow (get/update the bucket, create namespaces or tables under it, attach a policy) fails at the first call with ValidationException rather than operating on the bucket the stack just declared. Because the emulator's own rows and Outputs agree with each other, the stack looks healthy and the breakage only appears one hop downstream. It also makes the variants indistinguishable: a consumer cannot tell a name-keyed bucket from an ARN-keyed one from the stack data alone.

Fix: the CloudFormation provisioning path for the AWS::S3Tables::TableBucket resource type — the physical-id derivation in its resource handler (and the `Ref` resolution that copies it into stack Outputs) must emit the table bucket ARN (`arn:aws:s3tables:<region>:<account>:bucket/<TableBucketName>`), ideally the ARN returned by the create call it routes through, instead of the declared TableBucketName. Keep AWS::S3::Bucket on the bare name and leave AWS::S3Vectors::VectorBucket (already ARN-shaped) as is. Related but NOT the same cause: open issue 20261024T043150Z-cloudformation-step-functions-lambda-resources-physical-resource-id.md reports the same name-instead-of-ARN shape for Step Functions/Lambda resource types, and 20261023T014718Z-cloudformation-s3-s3tables-s3vectors-cloudformation-does-not-propagate-an-s3-vectors-resource.md covers rollback propagation in this same check group; neither carries this check or this resource type.

Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services.

## Evidence
Expected: for a stack declaring AWS::S3::Bucket + AWS::S3Vectors::VectorBucket + AWS::S3Tables::TableBucket, each resource row's PhysicalResourceId (and the matching `Ref` Output) is the identifier its owning service accepts — a bare name for S3, an ARN ending in `:bucket/<VectorBucketName>` for S3 Vectors, and an ARN ending in `:bucket/<TableBucketName>` for S3 Tables — and S3 Tables GetTableBucket(tableBucketARN=<that row>) returns name=<TableBucketName>, arn=<that row>.

Actual: the S3 (name) and S3 Vectors (ARN) rows resolve, but the S3Tables row is the bare declared name, and GetTableBucket on it is refused: HTTP 400, error code `ValidationException`, message (example) "1 validation error detected: Value 'sim2real-cfns3-ids-87d10c03b0d0-t' at 'tableBucketARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$". The check errors at that call, before its `endswith(":bucket/<name>")` assertion on the ARN rows.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. CreateStack with a template whose Resources are: Plain = AWS::S3::Bucket; Vectors = AWS::S3Vectors::VectorBucket with VectorBucketName=<tok>-v; Tables = AWS::S3Tables::TableBucket with TableBucketName=<tok>-t; and Outputs {Plain,Vectors,Tables} each {"Value": {"Ref": <logical id>}}.
3. Poll DescribeStacks until terminal — it reaches CREATE_COMPLETE.
4. ListStackResources (paginated) and DescribeStacks Outputs: note the Tables PhysicalResourceId/OutputValue is the bare name `<tok>-t`, not an ARN ending in `:bucket/<tok>-t`.
5. Call s3tables GetTableBucket(tableBucketARN=<that Tables row value>) — observe HTTP 400 ValidationException on tableBucketARN instead of a bucket whose name is `<tok>-t` and whose arn equals the row.
6. For contrast, s3 GetBucketLocation(Bucket=<Plain row>) and s3vectors GetVectorBucket(vectorBucketArn=<Vectors row>) both succeed in the same run.
