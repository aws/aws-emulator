Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment
Issue-Key: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment|run#19|AssertionError|assert state == "Enabled", f"CFN-deployed ESM did not reach Enabled (state={state})"
Issue-Commit: d0b60cf4e620490598360e9e180ae29d3f253b9d

# A CloudFormation-deployed AWS::Lambda::EventSourceMapping is still in State "Creating" once the stack reports CREATE_COMPLETE and never reaches Enabled

> _Note: this issue was identified against the code base at commit `d0b60cf4e620490598360e9e180ae29d3f253b9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEventSourceMapping

## Description
Cause 1 of 1: an event source mapping created by CloudFormation (AWS::Lambda::EventSourceMapping, source = a DynamoDB table StreamArn resolved by !GetAtt, Enabled: true, StartingPosition: TRIM_HORIZON) is reported by GetEventSourceMapping with State "Creating" after the stack has already reached CREATE_COMPLETE, and does not progress to "Enabled". Failing check: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment.

AWS: Smithy (com-amazonaws-lambda-awsgirapiservice) EventSourceMappingConfiguration.State — "The state of the event source mapping. It can be one of the following: Creating, Enabling, Enabled, Disabling, Disabled, Updating, or Deleting." Creating/Enabling are transient lifecycle states that the mapping leaves on its own; for a mapping created with Enabled=true the terminal state is Enabled. CloudFormation does not signal CREATE_COMPLETE for the AWS::Lambda::EventSourceMapping resource until that mapping has been created and stabilised, so on real AWS a GetEventSourceMapping issued after the stack is CREATE_COMPLETE returns State "Enabled" (and in any case converges to Enabled shortly after, without any client action). The rest of the stack's contract is honoured by the emulator here: the stack reaches CREATE_COMPLETE, !GetAtt SourceTable.StreamArn resolves to the table's LatestStreamArn, and the mapping's EventSourceArn/FunctionArn are wired to the deployed stream and function — only the lifecycle state diverges.

Emulator: GetEventSourceMapping on the UUID the stack returns as its EsmIdOut answers 200 with State "Creating". The check polls for a state in (Enabled, Creating, Disabled) and then requires Enabled; the observed value is "Creating" — i.e. the mapping is handed out in a transient state that the emulator never advances, so Enabled is never observable. This is consistent with the already-filed reports that Kinesis/DynamoDB-stream mappings never deliver records (e.g. 20261029T180026Z-dynamodb-kinesis-lambda-event-source-mappings-are-created-for-non-existent.md, 20261110T155738Z-dynamodb-kinesis-lambda-a-kinesis-event-source-mapping.md), but those cover delivery, not the never-advancing lifecycle state, so this cause is filed separately.

Why: a client that deploys a stream pipeline with CloudFormation and then gates on the mapping's State — the standard readiness probe, including `aws lambda get-event-source-mapping` in deploy scripts, CDK/CFN post-deploy waits and operational health checks — waits forever or times out, because "Creating" is a state AWS guarantees is transient. Any assertion that a deployed mapping is Enabled (or that it is not stuck mid-create) fails against the emulator even though the stack succeeded, and a client cannot distinguish "still being created" from "will never be enabled".

Fix: the Lambda event-source-mapping state machine behind CreateEventSourceMapping/GetEventSourceMapping — a mapping created with Enabled=true must leave Creating (optionally via Enabling) and settle on Enabled, and the CloudFormation AWS::Lambda::EventSourceMapping resource provider must not report the resource CREATE_COMPLETE until the mapping it created has left the transient create state. Mappings created with Enabled=false must settle on Disabled by the same path.

Traces: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment (deployments/cfn-ddb-stream-lambda.yaml: SourceTable with StreamSpecification NEW_AND_OLD_IMAGES, StreamEsm with BatchSize 1, Enabled true).

## Evidence
Check: dynamodb__kinesis__lambda.check-cfn-ddb-stream-lambda-deployment [mismatch]

Expected (AWS): after the stack reaches CREATE_COMPLETE, GetEventSourceMapping(UUID=<EsmIdOut>) returns State "Enabled" (Creating/Enabling are transient and left without client action).

Actual (emulator): State is "Creating" and stays there. Replay output:
  File "check_dynamodb__kinesis__lambda_check_cfn_ddb_stream_lambda_deployment.py", line 75, in run
    assert state == "Enabled", f"CFN-deployed ESM did not reach Enabled (state={state})"
  AssertionError: CFN-deployed ESM did not reach Enabled (state=Creating)

Preceding assertions in the same check passed, so the divergence is isolated to the lifecycle state: the stack reached CREATE_COMPLETE, StreamArnOut equalled DescribeTable(...).Table.LatestStreamArn, and the mapping's EventSourceArn/FunctionArn matched the deployed stream and function. The literal state string "Creating" is the fixed, enumerated value at issue; the UUID, ARNs, table and stack names in the recording are per-run values, not part of the requirement.

## Steps to Reproduce
1. create_stack with a template containing: an AWS::DynamoDB::Table with StreamSpecification.StreamViewType NEW_AND_OLD_IMAGES, an AWS::Lambda::Function (python3.12), and an AWS::Lambda::EventSourceMapping with EventSourceArn: !GetAtt SourceTable.StreamArn, FunctionName: !Ref the function, StartingPosition: TRIM_HORIZON, BatchSize: 1, Enabled: true. Output !Ref of the mapping as EsmIdOut.
2. Poll describe_stacks until StackStatus is CREATE_COMPLETE.
3. Call lambda get_event_source_mapping(UUID=<EsmIdOut>) and read State.
4. Observed: "Creating", and it remains "Creating" on further polls. Expected on AWS: "Enabled".
