Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id
Issue-Key: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id|run#34|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# events__lambda__stepfunctions: a Task state with Resource arn:aws:states:::lambda:invoke is never dispatched - the execution fails States.Runtime with an emulator-authored cause, so the EventBridge-envelope-as-execution-input contract is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
ONE cause in this issue; it must be closed before submitting.

Related open issue: 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describes the same dispatch gap for pure-sfn checks, but the replay suite does NOT report this check as OWNED by it (its replay set does not carry this check id, and this check is in the events__lambda__stepfunctions group), so it is filed here. Closing that issue without re-replaying this check leaves this flow failing.

Cause 1 of 1 - the optimized Lambda integration 'arn:aws:states:::lambda:invoke' is not an integration the interpreter will call, so every Task state using it fails at runtime instead of invoking the function.
AWS: a Task state whose Resource is arn:aws:states:::lambda:invoke with Parameters {FunctionName, Payload} invokes the named function and sets the state's result to the Invoke response envelope (Payload/StatusCode/ExecutedVersion), so a state machine built from Task states over Lambda runs to SUCCEEDED and DescribeExecution reports status SUCCEEDED with the output the last state produced (Smithy: StartExecution input stateMachineArn required + input + name, output executionArn/startDate; DescribeExecution output status one of RUNNING|SUCCEEDED|FAILED|TIMED_OUT|ABORTED|PENDING_REDRIVE plus output/error/cause - a run that fails carries error/cause, a run that succeeds carries neither). When the function itself raises, AWS reports the function's error name (e.g. the handler's exception type, or Lambda.Unhandled) in error with the function's JSON error payload in cause; States.Runtime is reserved for an interpreter-level fault such as an unresolvable path, never for 'this integration is not supported'. The check's downstream requirement is recorded behavior: a rule target with no Input hands the state machine EventBridge's FULL envelope, so the job id the poller derives from event["id"] is the PutEvents event id, not the id inside detail.
Emulator: the execution ends status FAILED with error 'States.Runtime' and cause "state 'SubmitJob': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run fo..." - an emulator-authored sentence that names the emulator itself and that AWS never produces, under an error name AWS does not use for this situation. Nothing is invoked, so the check's real assertion (final["status"] == "SUCCEEDED" and the echoed job id equal to the PutEvents event id rather than the detail's own id) can never be evaluated.
Why: the Lambda Task state is the single most common Step Functions building block. Any client workflow that calls a function - job pollers, sagas, orchestration of EventBridge-delivered work - fails with a runtime error that points at no request defect, and the error/cause pair a client catches (Retry/Catch on 'Lambda.*' or on the function's own error name) never matches, so error handling written against AWS behaves differently here. It also hides whether the EventBridge target hands the envelope or the detail to the execution.
Fix: the Step Functions interpreter's Task-state service-integration dispatch (the sfn service crate, services/service-com-amazonaws-sfn-awsstepfunctions) must route arn:aws:states:::lambda:invoke (and its .sync/.waitForTaskToken variants) to the lambda service's Invoke path, set the state result to the Invoke envelope, and map a function error onto the AWS error/cause naming rather than refusing the integration with States.Runtime and emulator-authored prose.
Traces: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id.

## Evidence
Suite report, service row events__lambda__stepfunctions 1/4 (25%), 3 failed. This check is reported with verdict 'mismatch' and detail:
  File "check_events__lambda__stepfunctions_refarch_sfn_eventbridge_envelope_id_becomes_the_job_id.py", line 133, in run
    assert final["status"] == "SUCCEEDED", (
  AssertionError: the delivered execution should run the poller to completion; got 'FAILED' error='States.Runtime' cause="state 'SubmitJob': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run fo"
It is NOT marked ALREADY FILED and NOT marked OWNED by any open issue.

Expected (AWS): DescribeExecution status SUCCEEDED, with no error/cause, and the poller's output id equal to the PutEvents envelope id. Actual: status FAILED, error 'States.Runtime', cause = the emulator-authored sentence above. The fixed part of a diverging AWS failure here would be the function's own error name in error and the function's JSON error payload in cause; the state name ('SubmitJob') and the Resource ARN in the emulator's message are request-derived values, but the sentence itself is not AWS wording.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. iam CreateRole for a lambda.amazonaws.com execution role and for a states.amazonaws.com role allowing lambda:InvokeFunction on *.
3. lambda CreateFunction twice: a 'submit' handler returning {"id": event["id"], "status": "SUCCEEDED"} and a 'check' handler echoing status/id.
4. sfn CreateStateMachine with a poller definition whose Task states use Resource 'arn:aws:states:::lambda:invoke' with Parameters {"FunctionName": <submit arn>, "Payload.$": "$"}.
5. events PutRule on a unique source + PutTargets with the state machine as target and no Input; events PutEvents on that source with Detail {"id": "sim2real-detail-id-not-the-envelope-id", "job": "nightly"}.
6. Poll sfn ListExecutions/DescribeExecution for the delivered execution. Expect status SUCCEEDED and the resulting job id equal to the PutEvents event id. Observe status FAILED with error 'States.Runtime' and the cause naming 'arn:aws:states:::lambda:invoke' as an integration the emulator does not call.
