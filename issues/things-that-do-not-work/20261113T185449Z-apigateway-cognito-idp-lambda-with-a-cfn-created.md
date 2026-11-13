Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway__cognito-idp__lambda: with a CFN-created CloudWatch role on AWS::ApiGateway::Account, API Gateway never delivers execution logs to API-Gateway-Execution-Logs_<api-id>/<stage>, so the trust-is-assumable check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccount

## Description
Cause 1 of 1: a CloudFormation-created IAM role whose AssumeRolePolicyDocument trusts apigateway.amazonaws.com and which is installed as the account-level CloudWatch role (AWS::ApiGateway::Account -> CloudWatchRoleArn, i.e. apigateway UpdateAccount on /cloudwatchRoleArn) is never actually assumed for log delivery: invoking the deployed stage produces no execution-log events, so nothing ever appears in the log group API-Gateway-Execution-Logs_<api-id>/<stage>.

AWS: Smithy: apigateway UpdateAccount/GetAccount expose `cloudwatchRoleArn` on the Account resource — one value per account per region, shared by every REST API in it. Recorded behavior for the consequence: once that role is set and trusts apigateway.amazonaws.com (with AmazonAPIGatewayPushToCloudWatchLogs attached), API Gateway assumes it and writes execution logs for an invoked stage into the log group named API-Gateway-Execution-Logs_<rest-api-id>/<stage-name>, creating the group and at least one log stream carrying events. The group name is a fixed template with the api id and stage name substituted; delivery can lag the account update by a short propagation window, which is why the recording re-invokes while polling. The check's other half — that GetRole returns exactly one Allow statement whose Principal.Service is apigateway.amazonaws.com — is what proves the role itself is correct, so the only thing left is the delivery.

Emulator: the check never completes: replay reports `Check timed out after 60.0s`. Its awaited state is `_invoke_and_check_logs()` — invoke the stage, then DescribeLogStreams + GetLogEvents on API-Gateway-Execution-Logs_<api-id>/test — under a 240 s poll, so the harness cap is reached inside that poll. Hypothesis: the execute-api data plane emits no execution logs at all when the account cloudwatchRoleArn is set, so the log group is never created (DescribeLogStreams raises ResourceNotFoundException, which the check reads as "not yet") and every poll cycle returns false until the cap.

Why: execution logs are the only way a client sees what the gateway did with a request — authorizer decisions, mapping-template evaluation, integration status. A client that wires the account CloudWatch role (the documented one-time setup) and then reads the execution log group gets an absent group indistinguishable from "logging not enabled yet", so the diagnosis path it depends on silently yields nothing, and any gate that waits for the first log event hangs.

Fix: the execute-api data plane's execution-logging emitter plus the account-level cloudwatchRoleArn plumbing — when a REST API's stage is invoked and the account has a cloudwatchRoleArn, the gateway must create API-Gateway-Execution-Logs_<api-id>/<stage> and append execution-log events to a stream in it; the CloudFormation AWS::ApiGateway::Account resource must set the same account value that apigateway UpdateAccount /cloudwatchRoleArn sets.

Traces: apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive. Related open issues cover different causes and do not carry this check's replay key: 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md is about ACCESS logs on a stage's accessLogSettings, and 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md is about proxy integrations answering 500.

## Evidence
Replay output for apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive:

  #### [mismatch] apigateway__cognito-idp__lambda.cfn-apigateway-role-trusts-apigateway-service-positive
  Error: Check timed out after 60.0s

The check's sequence and its awaited state:
  - deploy a stack declaring AWS::IAM::Role (trust Principal.Service = apigateway.amazonaws.com, ManagedPolicyArns = [arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs]), AWS::ApiGateway::Account{CloudWatchRoleArn: Fn::GetAtt CwRole.Arn}, and a REST API with a MOCK /ping method deployed to stage "test";
  - iam GetRole -> AssumeRolePolicyDocument must hold exactly one Allow statement with Principal.Service == "apigateway.amazonaws.com";
  - GET <stage>/ping until 200 (90 s poll);
  - then, under a 240 s poll at 10 s intervals: re-invoke /ping and check logs DescribeLogStreams(logGroupName="API-Gateway-Execution-Logs_<api-id>/test", orderBy=LastEventTime, descending=true, limit=5) + GetLogEvents on each stream for a non-empty events list.

Expected: the log group exists and at least one stream carries events.
Actual: the awaited state is never reached — the check is killed at 60.0s with no log group/stream/events observed. The check's own reader treats a ResourceNotFoundException on DescribeLogStreams as "not yet", so an absent group polls silently to the deadline.

(The api id, role name and generated api name are per-run values; the requirement is the log-group name template API-Gateway-Execution-Logs_<rest-api-id>/<stage-name> carrying execution-log events after an invoke.)

## Steps to Reproduce
1. CreateStack with: AWS::IAM::Role whose AssumeRolePolicyDocument allows sts:AssumeRole to Principal.Service apigateway.amazonaws.com and whose ManagedPolicyArns include arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs; AWS::ApiGateway::Account with CloudWatchRoleArn = Fn::GetAtt[CwRole, Arn]; a REST API with a MOCK GET /ping deployed to stage "test". Read RoleName and ApiId from the Outputs.
2. iam GetRole(RoleName) — the trust document must carry exactly one Allow statement with Principal.Service == "apigateway.amazonaws.com" (this part is satisfied today).
3. GET https://<api-id>.execute-api.<region>.../test/ping until 200.
4. logs DescribeLogStreams(logGroupName="API-Gateway-Execution-Logs_<api-id>/test") and GetLogEvents on the newest stream, re-invoking /ping between polls.
   Expected: the group exists and at least one stream carries execution-log events.
   Observed: nothing ever appears; the check is killed by the 60 s harness cap while polling.
5. Also confirm apigateway GetAccount reports the stack's role in cloudwatchRoleArn, and that UpdateAccount /cloudwatchRoleArn restores the prior value on cleanup.
