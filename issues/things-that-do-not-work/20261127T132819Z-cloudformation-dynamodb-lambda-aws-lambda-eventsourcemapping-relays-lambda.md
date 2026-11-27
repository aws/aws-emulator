Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.stream-mapping-factor-ceiling-rolls-back-without-attaching
Issue-Key: cloudformation__dynamodb__lambda.stream-mapping-factor-ceiling-rolls-back-without-attaching|_assert_ceiling_reason#10|AssertionError|assert _UUID.fullmatch(token) and reason == _REASON_PREFIX + token + _REASON_SUFFIX, (
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# cloudformation__dynamodb__lambda: AWS::Lambda::EventSourceMapping relays Lambda's API validation error instead of CloudFormation's model-validation failure for ParallelizationFactor above 10

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a property value outside the resource schema's declared range is sent to the service and the service's refusal is relayed, instead of being caught by CloudFormation's own model validation.
AWS: the AWS::Lambda::EventSourceMapping schema declares `ParallelizationFactor` with maximum 10. A template with 11 fails in CloudFormation before any Lambda call, and the resource status reason is `Resource handler returned message: "Model validation failed (#/ParallelizationFactor: failed validation constraint for keyword [maximum])" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. The fixed text is `Model validation failed (#/<PropertyName>: failed validation constraint for keyword [<keyword>])`; the property pointer, the keyword and the request token are the variable parts.
Emulator: CloudFormation calls CreateEventSourceMapping anyway and relays the Lambda service error: `Resource handler returned message: "Invalid request provided: 1 validation error detected: Value '11' at 'parallelizationFactor' failed to satisfy constraint: Member must have value less than or equal to 10 (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`.
Why: the failure reason a client reads back from DescribeStackEvents is the wrong text and attributes the refusal to the Lambda API rather than to the template property, so tooling that matches CloudFormation's `Model validation failed (#/...)` form to point at the offending template property cannot.
Fix: the AWS::Lambda::EventSourceMapping resource handler's property validation — enforce the resource schema's range constraints before calling the service and emit the model-validation message.
Traces: cloudformation__dynamodb__lambda.stream-mapping-factor-ceiling-rolls-back-without-attaching.

## Evidence
Expected: `Resource handler returned message: "Model validation failed (#/ParallelizationFactor: failed validation constraint for keyword [maximum])" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`.
Actual: `Resource handler returned message: "Invalid request provided: 1 validation error detected: Value '11' at 'parallelizationFactor' failed to satisfy constraint: Member must have value less than or equal to 10 (Service: Lambda, Status Code: 400, Request ID: ...) (SDK Attempt Count: 1)" (RequestToken: ..., HandlerErrorCode: InvalidRequest)`.

## Steps to Reproduce
CreateStack with an AWS::Lambda::EventSourceMapping on a DynamoDB stream whose ParallelizationFactor is 11, then read the StreamHook resource's status reason from DescribeStackEvents.
