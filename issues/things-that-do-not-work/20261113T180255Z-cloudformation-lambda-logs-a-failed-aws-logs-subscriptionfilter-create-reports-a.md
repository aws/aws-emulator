Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create
Issue-Key: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create|run#34|AssertionError|assert match is not None, f"Filter's reason is not a resource-handler message: {reasons[0]!r}"
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# A failed AWS::Logs::SubscriptionFilter create reports a bare service message: the resource-handler wrapper drops the "Invalid request provided: <Type>." prefix and the "(Service: …, Status Code: …, Request ID: …)" suffix

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: when a stack resource of type AWS::Logs::SubscriptionFilter fails to create because the destination Lambda grants logs.amazonaws.com no invoke permission, the emulator does fail the resource and roll the stack back, but the CREATE_FAILED event's ResourceStatusReason carries only the raw CloudWatch Logs message. The CloudFormation resource-handler envelope AWS emits around a downstream service error is incomplete: the "Invalid request provided: <ResourceType>. " prefix and the trailing "(Service: <SdkId>, Status Code: <http status>, Request ID: <id>)" clause are both missing. Failing check: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): PutSubscriptionFilter(logGroupName, filterName, filterPattern, destinationArn required) models InvalidParameterException with httpError 400; CloudWatch Logs validates execute access on a Lambda destinationArn at PutSubscriptionFilter time and refuses with InvalidParameterException / "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function." CloudFormation's resource handler wraps that refusal, and DescribeStackEvents reports the Filter resource's CREATE_FAILED ResourceStatusReason in its fixed template:
  Resource handler returned message: "Invalid request provided: <ResourceType>. <service message> (Service: <SdkId>, Status Code: <status>, Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: <code>)
Fixed text: `Resource handler returned message: "`, `Invalid request provided: `, `. `, ` (Service: `, `, Status Code: `, `, Request ID: `, `)" (RequestToken: `, `, HandlerErrorCode: `, `)`. Variable parts taken from the request and the failing call: the resource type (here AWS::Logs::SubscriptionFilter), the wrapped service message, the SDK id (CloudWatchLogs), the status (400), the request id and the request token (both per-run), and the handler error code (InvalidRequest for a 400 InvalidParameterException). Per-run ids/tokens are not part of the requirement; the prefix, the Service/Status Code/Request ID clause and the handler error code are.

Emulator: the single CREATE_FAILED event for the Filter resource reads
  Resource handler returned message: "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)
so the inner quoted string is the bare Logs message: no "Invalid request provided: AWS::Logs::SubscriptionFilter. " prefix and no "(Service: CloudWatchLogs, Status Code: 400, Request ID: …)" clause. The stack status (ROLLBACK_COMPLETE), the single CREATE_FAILED event for Filter and HandlerErrorCode=InvalidRequest all match AWS; only the wrapped message diverges.

Why: a client that parses a CREATE_FAILED reason to recover the originating service, HTTP status and request id — which is how deploy tooling, CI log scrapers and support escalations attribute a stack failure to a downstream API call — gets no match at all and cannot tell which service refused the create or with what status. The resource type is also absent, so the reason does not name the resource class that failed.

Fix: the CloudFormation resource-handler error wrapper — the component that formats a failed resource provider's downstream error into ResourceStatusReason (it must interpolate the resource type and the originating service's sdk id, HTTP status and request id around the service message) in the cloudformation crate's stack-event/resource-provider path, not the Logs validation itself, which already produces the correct inner message.

Traces: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create (the check's positive control — the permissioned filter creating and actually delivering — passed, so only the refusal's reason text is at issue). No open issue covers the resource-handler reason envelope; the subscription-filter issues already on file (e.g. 20261110T153046Z-cloudwatch-lambda-logs-a-cloudformation-deployed-subscription-filter.md) are about delivery, not this message.

## Evidence
Expected (AWS): the Filter resource's one CREATE_FAILED event has ResourceStatusReason matching
  ^Resource handler returned message: "(?P<message>.*) \(Service: (?P<service>[A-Za-z0-9]+), Status Code: (?P<status>\d+), Request ID: [^)]*\)(?: \(SDK Attempt Count: \d+\))?" \(RequestToken: [^,]*, HandlerErrorCode: (?P<handler>[A-Za-z]+)\)$
with message = "Invalid request provided: AWS::Logs::SubscriptionFilter. Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.", service = "CloudWatchLogs", status = "400", handler = "InvalidRequest".

Actual (emulator):
  AssertionError: Filter's reason is not a resource-handler message: 'Resource handler returned message: "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function." (RequestToken: 0766c8e8-6878-38c2-83ec-cec370cf876c, HandlerErrorCode: InvalidRequest)'
(the RequestToken is per-run and not part of the requirement; the missing 'Invalid request provided: AWS::Logs::SubscriptionFilter. ' prefix and the missing '(Service: CloudWatchLogs, Status Code: 400, Request ID: <id>)' clause are.)

## Steps to Reproduce
1. cloudformation.CreateStack with Capabilities=[CAPABILITY_IAM] and a template containing: an AWS::Logs::LogGroup (LogGroupName=<group>, RetentionInDays=1); an AWS::IAM::Role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole; an AWS::Lambda::Function (python3.13, inline ZipFile handler) using that role; and an AWS::Logs::SubscriptionFilter (LogGroupName={Ref SrcGroup}, FilterPattern="MARKER", DestinationArn={Fn::GetAtt [Function, Arn]}) with DependsOn the function and NO AWS::Lambda::Permission granting logs.amazonaws.com lambda:InvokeFunction.
2. Poll cloudformation.DescribeStacks until the stack settles. Observed and expected: ROLLBACK_COMPLETE.
3. cloudformation.DescribeStackEvents and take the single event with LogicalResourceId="Filter" and ResourceStatus="CREATE_FAILED".
4. Read its ResourceStatusReason. Expected: 'Resource handler returned message: "Invalid request provided: AWS::Logs::SubscriptionFilter. Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function. (Service: CloudWatchLogs, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Observed: the inner message is the bare Logs sentence, with no resource-type prefix and no Service/Status Code/Request ID clause.
