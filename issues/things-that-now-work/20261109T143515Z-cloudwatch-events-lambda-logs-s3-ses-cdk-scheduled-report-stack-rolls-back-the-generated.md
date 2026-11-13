Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-workflow
Issue-Key: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-workflow|_deploy_cfn#8|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CDK scheduled-report stack rolls back: the generated Lambda Permission StatementId exceeds Lambda's 100-character limit

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: CloudFormation hands Lambda a generated statement id longer than the modelled maximum, so the stack cannot deploy.
AWS: Smithy (lambda) AddPermission.StatementId -> com.amazonaws.lambda#StatementId, @length min 1 max 100, @pattern ^([a-zA-Z0-9-_]+)$. Real AWS deploys this CDK app to CREATE_COMPLETE, so the statement id CloudFormation generates for an AWS::Lambda::Permission always fits within 100 characters.
Emulator: the generated id concatenates the stack name, the full CDK logical id and a 12-character suffix (110+ characters) and Lambda refuses it, leaving the stack in ROLLBACK_COMPLETE with reason 'Resource handler returned message: "1 validation error detected: Value '<generated id>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The validation sentence and constraint clause are fixed text; the id, request id and token are per-run values.
Why: any CDK app that grants EventBridge (or API Gateway) permission to invoke a Lambda function is undeployable, so the scheduled-report workflow never exists.
Fix: the AWS::Lambda::Permission handler's physical-id generation in the CloudFormation crate (shorten the generated statement id to the modelled 100-character maximum).
Traces: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-workflow.

## Evidence
Expected: CREATE_COMPLETE. Actual: 'AssertionError: stack <name> ended in ROLLBACK_COMPLETE: ScheduleAllowEventRule...=Resource handler returned message: "1 validation error detected: Value '<110-char id>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, ...)"'.

## Steps to Reproduce
Replay cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-workflow: synthesize and deploy the CDK stack containing an AWS::Lambda::Permission for events.amazonaws.com.
