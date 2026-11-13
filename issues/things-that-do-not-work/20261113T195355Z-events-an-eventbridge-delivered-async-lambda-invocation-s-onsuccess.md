Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope|run#28|AssertionError|assert rec["requestContext"]["functionArn"] == f"{fn_arn}:$LATEST", (
Issue-Commit: dcecf1852a9f86e80d5d906dc10b7044742585db

# An EventBridge-delivered async Lambda invocation's OnSuccess destination record carries the unqualified functionArn instead of the invoked version's qualified ARN (…:$LATEST)

> _Note: this issue was identified against the code base at commit `dcecf1852a9f86e80d5d906dc10b7044742585db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the asynchronous invocation record the emulator writes to a function's OnSuccess destination reports `requestContext.functionArn` without the version qualifier, so the ARN ends at the function name instead of naming the version that actually ran. Failing check: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope.

AWS: an unqualified function ARN resolves to the $LATEST version (Smithy, lambda InvocationRequest.Qualifier: "Specify a version or alias to invoke a published version of the function" — absent, the invoke targets $LATEST). The async invocation record is a runtime envelope and is not modeled in Smithy, so this is recorded/documented behavior only: AWS's invocation record carries `requestContext.functionArn` as the QUALIFIED ARN of the version that ran, i.e. `arn:aws:<partition-service>:<region>:<account>:function:<function-name>:$LATEST` for a target invoked through its unqualified ARN, alongside requestContext.condition ("Success") and approximateInvokeCount. The fixed part of that value is the function's ARN; the variable parts are the region, account, function name taken from the request and the resolved qualifier taken from the emulator's state ($LATEST when no version/alias was named, otherwise the version number or alias).

Emulator: the record's `requestContext.functionArn` is the bare unqualified ARN — its last colon-separated segment is the function NAME (recorded: qualifier read back as 'sim2real-behav-fn-<id>') rather than '$LATEST'. The rest of the record (condition Success, approximateInvokeCount 1, requestPayload, responsePayload) is reached only after this assertion, so the record is otherwise produced; only the ARN's qualifier is missing. No status or error code diverges here — the delivery itself is a success.

Why: a consumer of destination records keys, filters or routes on requestContext.functionArn to tell which VERSION of a function produced a result — that is the whole reason AWS qualifies it. With the qualifier dropped, a consumer cannot distinguish $LATEST from a published version or an alias, an exact-match comparison against the qualified ARN a client builds (`<fn_arn>:$LATEST`) never matches, and any per-version aggregation over destination records collapses to one bucket.

Fix: the serializer that builds the asynchronous invocation record's requestContext in the Lambda async-invoke destination path — it must stamp the RESOLVED version qualifier onto functionArn (the version/alias the invoke was routed to, defaulting to $LATEST), not echo the unqualified ARN it was called with. The open issue 20261111T230358Z-events-5-net-new-causes-a-lambda-target.md covers a Lambda target's handler ERROR producing no OnFailure record under this same heading; it does not cover the content of a delivered OnSuccess record and so does not cover this.

Traces: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope (assertion at the functionArn comparison; the record was delivered and graded up to that point).

## Evidence
Expected (AWS): the OnSuccess destination record for an EventBridge-delivered async invoke of an unqualified function ARN carries
  requestContext.functionArn == "<function-arn>:$LATEST"
(the qualifier segment is the literal $LATEST; region/account/function-name come from the request).

Actual (emulator): the qualifier segment is the function name itself — the ARN is unqualified. Check assertion:
  assert rec["requestContext"]["functionArn"] == f"{fn_arn}:$LATEST"
  AssertionError: the destination record names a different function or qualifier than the target's unqualified ARN resolves to (qualifier 'sim2real-behav-fn-1f822f9bc0d0')

The record is otherwise delivered: requestContext.condition and approximateInvokeCount are only asserted before this line and passed, so the divergence is the missing ':$LATEST' qualifier, not a missing record. HTTP status and error code: none — this is a successful delivery whose payload content diverges.

## Steps to Reproduce
1. Create a queue and a Python function whose handler returns what it saw; grant the role sqs:SendMessage on the queue.
2. lambda PutFunctionEventInvokeConfig(FunctionName=<fn>, MaximumRetryAttempts=0, DestinationConfig={"OnSuccess":{"Destination":<queue-arn>}}).
3. Create a custom event bus and a rule matching {"source":["<source>"]}; lambda AddPermission(Principal=events.amazonaws.com, Action=lambda:InvokeFunction, SourceArn=<rule-arn>); events PutTargets with the function's UNQUALIFIED ARN as the target.
4. events PutEvents one entry on that bus with the matching source.
5. Receive the OnSuccess destination record from the queue and read requestContext.functionArn.
Observed: the value is the unqualified function ARN (last segment = function name). Expected: the same ARN suffixed with ':$LATEST'.
