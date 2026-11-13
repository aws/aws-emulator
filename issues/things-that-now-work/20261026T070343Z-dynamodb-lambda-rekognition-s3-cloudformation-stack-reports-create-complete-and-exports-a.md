Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle
Issue-Key: dynamodb|DeleteItem|ResourceNotFoundException
Issue-Commit: 6de2108b200ed90d33fb4f24d0237ce550e05e72

# CloudFormation stack reports CREATE_COMPLETE and exports a TableName, but the AWS::DynamoDB::Table it names does not exist (DeleteItem/GetItem answer ResourceNotFoundException)

> _Note: this issue was identified against the code base at commit `6de2108b200ed90d33fb4f24d0237ce550e05e72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

Cause 1 of 1: a stack that declares an AWS::DynamoDB::Table reaches CREATE_COMPLETE and its Outputs carry a non-empty TableName, yet no DynamoDB table of that name exists, so the very next data-plane call against it is refused as a missing resource.

AWS: CloudFormation only reports CREATE_COMPLETE for a stack once every declared resource has been created, and a stack Output built from `Ref`/`Fn::GetAtt` on an AWS::DynamoDB::Table resolves to the physical name of a table that is then addressable on the DynamoDB data plane. DynamoDB's ResourceNotFoundException is modelled (Smithy: com.amazonaws.dynamodb#ResourceNotFoundException, client error, HTTP 400, awsQueryCompatible JSON 1.0 with __type com.amazonaws.dynamodb.v20120810#ResourceNotFoundException) and its documentation states the contract exactly: "The operation tried to access a nonexistent table or index. The resource might not be specified correctly, or its status might not be ACTIVE." So on real AWS, DeleteItem against the table named by a CREATE_COMPLETE stack's Output succeeds (DeleteItem is idempotent — a key that is not present is still a 200, not an error; only a missing TABLE is ResourceNotFoundException).

Emulator: the deploy phase of the check passes — the stack reaches CREATE_COMPLETE, DescribeStacks returns Outputs whose BucketName/TableName/FunctionName are all non-empty, GetBucketLocation agrees with the region, and the bucket really does carry the three suffix-filtered S3 notification configurations the template declares. The very next call, `DeleteItem(TableName=<the stack's TableName output>, Key={"image_name": {"S": "alpha.jpg"}})`, is refused: `ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteItem operation: Requested resource not found`. So the emulator either never provisioned the AWS::DynamoDB::Table resource at all while still advancing the stack to CREATE_COMPLETE, or it provisioned the table under a physical name different from the one `Ref` resolved into the Output. Note that the emulator's message text is also not AWS's: AWS words this error "Requested resource not found" only for Streams; for the DynamoDB control/data plane the message names the table, e.g. "Requested resource not found: Table: <TableName> not found" — the fixed text is "Requested resource not found: Table: " and " not found", and the variable part is the TableName taken from the request.

Why: a client that deploys a table through CloudFormation and then reads the stack Output to address it — the standard pattern, and what every CDK/SAM app does — gets a stack it is told is complete and an identifier that does not resolve. Every subsequent PutItem/GetItem/DeleteItem against it fails, so no application deployed this way can run, and the failure is attributed to the wrong service (DynamoDB answers "no such table" for a table CloudFormation said it created).

Fix: CloudFormation's AWS::DynamoDB::Table resource provider and the stack's resource/physical-id bookkeeping — the resource must actually be created in DynamoDB state before the stack advances to CREATE_COMPLETE, and the physical resource id that `Ref` resolves into Outputs must be the name DynamoDB registered. Secondarily, DynamoDB's not-found serializer should emit AWS's message template naming the table rather than the bare "Requested resource not found". Related open issues describe adjacent CloudFormation gaps but neither covers this resource type or the Output/physical-id mismatch: 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md and 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md.

Traces: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle (fails in phase 2/6 pre_invoke, immediately after phase 1/6 deploy passed).

## Evidence
Expected (real AWS): after the stack reaches CREATE_COMPLETE and DescribeStacks reports Outputs.TableName=<T>, `dynamodb:DeleteItem(TableName=<T>, Key={"image_name":{"S":"alpha.jpg"}})` returns HTTP 200 with an empty body (DeleteItem on an absent item is a success, not an error).

Actual (emulator): the same call is refused — botocore raises `ResourceNotFoundException`, surfaced by the check as `[phase 2/6 pre_invoke] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteItem operation: Requested resource not found`.

Corroborating detail from the same run: phase 1/6 deploy passed all of its assertions — the stack status was CREATE_COMPLETE, Outputs BucketName/TableName/FunctionName were all non-empty, and `GetBucketNotificationConfiguration` on the stack's bucket reported exactly the three declared suffix filters [.jpeg, .jpg, .png]. So the stack was accepted and partially materialised; only the table is missing.

Message-shape note: AWS's message for this error class on the DynamoDB data plane is the fixed text "Requested resource not found: Table: " + <TableName from the request> + " not found"; the emulator emits only the fixed prefix "Requested resource not found" with no table name.

## Steps to Reproduce
1. Create a stack from a template that declares an AWS::S3::Bucket, an AWS::DynamoDB::Table (hash key `image_name`, type S), an AWS::Lambda::Function and an S3 notification configuration, and that exports BucketName, TableName and FunctionName as Outputs.
2. Poll DescribeStacks until StackStatus == CREATE_COMPLETE.
3. Read Outputs.TableName from DescribeStacks — observe it is non-empty.
4. Call dynamodb:DeleteItem(TableName=<that value>, Key={"image_name": {"S": "alpha.jpg"}}).
5. Observe ResourceNotFoundException instead of a 200. Also try dynamodb:DescribeTable(TableName=<that value>) and dynamodb:ListTables to confirm no table of that name exists, and compare against DescribeStackResources for the logical table resource to see which physical id the stack recorded.
