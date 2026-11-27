Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.deploy-import-source-specification-loads-the-s3-items cloudformation__dynamodb__s3.deploy-import-source-change-replaces-the-table-with-the-new-prefix-items cloudformation__dynamodb__s3.deploy-import-source-is-write-only-absent-from-the-read-table-model cloudformation__dynamodb__s3.deploy-named-table-import-source-change-rejected-and-items-kept
Issue-Key: cloudformation__dynamodb__s3.deploy-import-source-specification-loads-the-s3-items|run#12|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation__dynamodb__s3.deploy-import-source-change-replaces-the-table-with-the-new-prefix-items|run#15|AssertionError|assert built["StackStatus"] == "CREATE_COMPLETE", f"the import stack ended {built['StackStatus']!r}"
Issue-Key: cloudformation__dynamodb__s3.deploy-import-source-is-write-only-absent-from-the-read-table-model|run#17|AssertionError|assert built["StackStatus"] == "CREATE_COMPLETE", f"the import stack ended {built['StackStatus']!r}"
Issue-Key: cloudformation__dynamodb__s3.deploy-named-table-import-source-change-rejected-and-items-kept|run#18|AssertionError|assert built["StackStatus"] == "CREATE_COMPLETE", f"the import stack ended {built['StackStatus']!r}"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# cloudformation__dynamodb__s3: AWS::DynamoDB::Table rolls the stack back on ImportSourceSpecification with an emulator-authored handler message

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::DynamoDB::Table resource handler refuses `ImportSourceSpecification` instead of importing the S3 data.
AWS: a table whose `ImportSourceSpecification` names an S3 bucket/prefix, input format and compression is created by ImportTable; the stack reaches CREATE_COMPLETE and the table holds the items from that prefix. The property is write-only — it does not appear in the read model of the table resource — and changing it on a named table is a replacement that CloudFormation refuses for a custom-named resource (the existing items survive), while changing it on an unnamed table replaces the table with the items of the new prefix.
Emulator: the Table resource fails immediately and every such stack ends `ROLLBACK_COMPLETE`, with `Resource handler returned message: "this emulator does not apply the ImportSourceSpecification property of AWS::DynamoDB::Table" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. That sentence is emulator-authored; AWS raises no error for this property. Because the create never succeeds, the write-only-property, replacement and refusal behaviours above are all unobservable too.
Why: a valid, widely used template (seed a table from S3 at deploy time) cannot be deployed at all, and the failure reason names the emulator rather than any defect the author could fix.
Fix: the AWS::DynamoDB::Table resource handler's create path — implement ImportSourceSpecification by driving ImportTable, keep the property out of the read model, and classify a change to it as a replacement.
Traces: cloudformation__dynamodb__s3.deploy-import-source-specification-loads-the-s3-items, cloudformation__dynamodb__s3.deploy-import-source-change-replaces-the-table-with-the-new-prefix-items, cloudformation__dynamodb__s3.deploy-import-source-is-write-only-absent-from-the-read-table-model, cloudformation__dynamodb__s3.deploy-named-table-import-source-change-rejected-and-items-kept.

## Evidence
Expected: CREATE_COMPLETE with the S3 items loaded into the table. Actual: `stack ended 'ROLLBACK_COMPLETE': [('Table', 'Resource handler returned message: "this emulator does not apply the ImportSourceSpecification property of AWS::DynamoDB::Table" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')]`, and `the import stack ended 'ROLLBACK_COMPLETE'` for the three follow-on checks.

## Steps to Reproduce
Upload DynamoDB-JSON items to an S3 prefix, then CreateStack with an AWS::DynamoDB::Table carrying ImportSourceSpecification for that bucket/prefix. Observe the Table resource fails and the stack rolls back.
