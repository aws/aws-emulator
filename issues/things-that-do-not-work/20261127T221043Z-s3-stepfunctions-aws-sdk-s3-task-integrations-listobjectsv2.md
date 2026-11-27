Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.aws-sdk-s3-list-head-copy-tasks-return-service-results
Issue-Key: s3__stepfunctions.aws-sdk-s3-list-head-copy-tasks-return-service-results|run#12|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# s3__stepfunctions: aws-sdk:s3 Task integrations (listObjectsV2, headObject, copyObject) are refused at run time with States.Runtime

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Task whose Resource is an 'arn:aws:states:::aws-sdk:s3:<operation>' integration fails the execution instead of calling S3 and returning that operation's output members.
AWS: the AWS SDK integration calls the named S3 operation with the Task's Parameters and places the operation's own output members (PascalCased) in the state's result — ListObjectsV2 returns Contents/KeyCount, HeadObject returns ContentLength/ETag, CopyObject returns CopyObjectResult — and the execution SUCCEEDs.
Emulator: the execution ends FAILED with error 'States.Runtime' and cause "state 'List': 'arn:aws:states:::aws-sdk:s3:listObjectsV2' names the 'ListObjectsV2' operation of S3, whose answer this emulator does not read as that operation's own output members, so nothing was run for it" — a sentence of the emulator's own, not any AWS error.
Why: every workflow that reads or copies objects through the S3 SDK integration fails here, and the failure is reported with an error/cause no AWS client can interpret or catch by name.
Fix: the aws-sdk Task integration dispatcher in the Step Functions execution engine — bind the S3 operation's response into the state result's output members.
Traces: s3__stepfunctions.aws-sdk-s3-list-head-copy-tasks-return-service-results.

## Evidence
Expected: the three aws-sdk S3 tasks run and return their service results (execution SUCCEEDED).
Actual: "the S3 aws-sdk tasks must run, got 'FAILED' cause=\"state 'List': 'arn:aws:states:::aws-sdk:s3:listObjectsV2' names the 'ListObjectsV2' operation of S3, whose answer this emulator does not read as that operation's own output members, so nothing was run for it\"".

## Steps to Reproduce
1. Create a state machine with Tasks whose Resource is 'arn:aws:states:::aws-sdk:s3:listObjectsV2', ':headObject' and ':copyObject'.
2. Start an execution; it fails with States.Runtime instead of returning the S3 results.
3. Or replay the check id in trace_ids.
