Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload
Issue-Key: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload|run#17|AssertionError|assert (rejected.get("error"), rejected.get("cause")) == ("States.Runtime", expected_cause), (
Issue-Commit: 70fd4cb3c22ab43bc53c62d82ccb3cfc2863a4f8

# lambda__stepfunctions: the States.Runtime cause for an unusable Payload parameter uses emulator wording instead of AWS's "The value for the field 'Payload' must be a STRING" (1 cause)

> _Note: this issue was identified against the code base at commit `70fd4cb3c22ab43bc53c62d82ccb3cfc2863a4f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a bare JSON scalar execution input resolved into the optimized Lambda integration's `Payload.$` now correctly fails the Task with States.Runtime, but the bracketed diagnostic inside the cause is not AWS's message. Failing check: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload.

AWS: StartExecution accepts any valid JSON text as input, including a bare number. The optimized `arn:aws:states:::lambda:invoke` integration requires the resolved `Payload` field to be a string/object/array (Smithy lambda#Invoke: Payload targets Blob and is the @httpPayload), so resolving `Payload.$: "$"` against a bare-scalar input fails the Task before the invocation is scheduled, and the execution ends FAILED with error `States.Runtime` and cause `An error occurred while executing the state '<StateName>' (entered at the event id #<n>). The Parameters '<resolved Parameters JSON>' could not be used to start the Task: [The value for the field 'Payload' must be a STRING]`. Fixed text: the sentence template and the bracketed detail `[The value for the field 'Payload' must be a STRING]`, which names the offending field. Variable parts from the request/state: the state name, the entered-at event id, and the resolved Parameters JSON (compact separators). Beyond the message, AWS schedules no invocation — the history contains no LambdaFunctionScheduled event. The message itself is recorded behavior only; it is not in the model.

Emulator: everything up to the bracket matches, then it substitutes a renderer-specific detail: `... could not be used to start the Task: [Cannot deserialize the 'Payload' of the invocation from Number value 42]`. The bracketed text names the JSON type and the literal value rather than stating the field's required type, so it does not match AWS's wording for this error class.

Why: the cause is the only machine-readable description of the rejection (the error is the generic States.Runtime), so a client or operator that matches on AWS's `[The value for the field 'Payload' must be a STRING]` — in a Catch-side classifier, a log-based alarm pattern or a parity assertion — does not recognise the failure and treats a well-understood parameter-type rejection as an unknown runtime fault.

Fix: the Step Functions ASL interpreter's Parameters-resolution error renderer for the optimized Lambda integration (the component that builds the `could not be used to start the Task: [...]` cause) must emit AWS's field-type diagnostic naming the field and its required type instead of a deserializer message quoting the input value. Context: the earlier accept-and-succeed behavior was filed and resolved as 20261105T081644Z-lambda-stepfunctions-a-raising-lambda-handler-never-fails.md (its Cause 3); the Task now fails as AWS does, so only the cause wording remains divergent.

Traces: lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload (the array-input positive control and the no-LambdaFunctionScheduled assertion in the same check are unaffected).

## Evidence
Check lambda__stepfunctions.execution-input-array-reaches-lambda-unchanged-bare-number-rejected-at-payload, line 69:
AssertionError: expected States.Runtime with the exact Payload-type cause naming the resolved Parameters; got 'States.Runtime': 'An error occurred while executing the state \'InvokeProcessingLambda\' (entered at the event id #2). The Parameters \'{"FunctionName":"arn:aws:lambda:us-west-2:000000000000:function:sim2real-scn-fn-<id>","Payload":42}\' could not be used to start the Task: [Cannot deserialize the \'Payload\' of the invocation from Number value 42]'

Expected (AWS) bracketed detail: [The value for the field 'Payload' must be a STRING]
Actual (emulator): [Cannot deserialize the 'Payload' of the invocation from Number value 42]

The function ARN, state name and event id in the sentence are request/state values and already match; the divergence is the bracketed diagnostic's text.

## Steps to Reproduce
1. Create a Lambda function and a STANDARD state machine with one Task: Resource `arn:aws:states:::lambda:invoke`, Parameters {"FunctionName": <fn arn>, "Payload.$": "$"}, no Retry.
2. StartExecution with input `[3, "two", {"k": null}]` — it SUCCEEDS and the array is echoed (control).
3. StartExecution with input `42` and poll DescribeExecution to FAILED.
4. Read `error`/`cause`: error is States.Runtime (correct) but the cause ends `[Cannot deserialize the 'Payload' of the invocation from Number value 42]` instead of `[The value for the field 'Payload' must be a STRING]`.
