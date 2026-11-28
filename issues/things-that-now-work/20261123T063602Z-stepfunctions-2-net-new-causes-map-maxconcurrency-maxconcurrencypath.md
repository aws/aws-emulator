Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime stepfunctions.stop-execution-aborted-event-details-with-and-without-cause stepfunctions.wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime|_assert_states_runtime#7|AssertionError|assert desc["status"] == "FAILED", f"{label}: expected FAILED, got {desc['status']}: {desc!r}"
Issue-Key: stepfunctions.stop-execution-aborted-event-details-with-and-without-cause|run#20|AssertionError|assert types == ["ExecutionStarted", "WaitStateEntered", "ExecutionAborted"], (
Issue-Key: stepfunctions.wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts|run#18|AssertionError|assert types == ["ExecutionStarted", "WaitStateEntered", "ExecutionAborted"], (
Issue-Commit: 44d4304dbd56e0f97705f387ff3fc6793140eb84

# stepfunctions: 2 net-new causes — Map MaxConcurrency/MaxConcurrencyPath out-of-range values are not validated at runtime, and StopExecution inside a Wait appends a WaitStateAborted event AWS never records

> _Note: this issue was identified against the code base at commit `44d4304dbd56e0f97705f387ff3fc6793140eb84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Two net-new root causes in the Step Functions interpreter, both reached through StartExecution/DescribeExecution/GetExecutionHistory on plain STANDARD state machines. Related already-filed issue to be aware of: 20261123T034326Z-cloudformation-stepfunctions-stopexecution-inside-a-wait-state-appends.md describes the same WaitStateAborted defect but is keyed to the cloudformation__stepfunctions check only, so it does not cover the two stepfunctions checks named under Cause 2 here.

Cause 1 of 2: a Map state whose MaxConcurrency (literal) or MaxConcurrencyPath (resolved) value falls outside the valid integer range is accepted and the execution SUCCEEDS, instead of failing the execution with States.Runtime. Failing check id: stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime.
AWS:      recorded behavior only (the ASL Map fields MaxConcurrency/MaxConcurrencyPath are not members of the Smithy API model; Smithy com.amazonaws.sfn#DescribeExecutionOutput only models the generic `status` (ExecutionStatus), `error` (SensitiveError, length 0-256) and `cause` (SensitiveCause, length 0-32768) members that carry the outcome). The recording shows the interpreter parses these fields as a Java long at state-entry time and, when the value is not a valid non-negative integer, ends the execution FAILED with error "States.Runtime" and a cause naming the state, the entry event id, the field and the offending value. The cause is a template: fixed text `An error occurred while executing the state '<stateName>' (entered at the event id #<id>). ` followed by, per class — a literal that cannot be parsed as an integer: `The MaxConcurrency field cannot be parsed as an integer value: <value>`; a resolved path value that is negative: `MaxConcurrency cannot be negative but was: <value>`; a resolved path value that is not a valid integer: `The MaxConcurrencyPath field refers to value "<value>" which is not a valid integer: <path>`. The state name, the entry event id, the value and the path are taken from the definition/request and the running execution; only the quoted fixed text is invariant. The failure is mode-independent: ItemProcessor ProcessorConfig Mode INLINE and Mode DISTRIBUTED behave identically. A MaxConcurrencyPath resolving to a small positive integer succeeds (positive control, which the emulator also passes).
Emulator: DescribeExecution reports status "SUCCEEDED" with output `[1]` and no error/cause for a Map state carrying MaxConcurrency = 9223372036854775808 (one above Java long max) in INLINE mode — the out-of-range value is silently ignored and the Map runs. Because the check stops at the first assertion, the DISTRIBUTED-literal, negative-MaxConcurrencyPath and MaxConcurrencyPath-overflow arms are unverified but share the same missing range check.
Why:      a client relying on AWS's behavior sees a definition defect turn into a silent success. A pipeline that depends on an invalid concurrency value failing fast (so a bad deployment is caught by the FAILED execution and its States.Runtime error) instead gets a SUCCEEDED execution whose concurrency was chosen by the emulator, and any catcher/retrier keyed on States.Runtime never fires.
Fix:      the Map state's entry/validation path in the Step Functions interpreter — the component that resolves MaxConcurrency and MaxConcurrencyPath for both INLINE and DISTRIBUTED ItemProcessor modes must range-check the resolved value (integer-parseable, non-negative) and fail the execution with error States.Runtime and the cause template above, rather than defaulting or clamping.
Traces:   stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime (listed under the suite report's ### stepfunctions heading).

Cause 2 of 2: GetExecutionHistory on an execution that StopExecution ended while it was suspended inside a top-level Wait state contains an extra per-state WaitStateAborted event. Failing check ids: stepfunctions.stop-execution-aborted-event-details-with-and-without-cause, stepfunctions.wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts.
AWS:      recorded behavior only. The recordings show the history is truncated at the interrupted state and is exactly ExecutionStarted, WaitStateEntered, ExecutionAborted, in that order: the Wait is entered and never left (no WaitStateExited), no per-state abort event is written, and the Wait's Next transition never fires (no SucceedStateEntered). The only abort record is the execution-level ExecutionAborted event, whose executionAbortedEventDetails carry the caller's error and cause when StopExecution supplied them and are empty ({}) when it did not. Smithy com.amazonaws.sfn#HistoryEventType does declare the value WaitStateAborted, so the value exists in the model — the divergence is that AWS does not emit it on this path.
Emulator: GetExecutionHistory (paginated, reverseOrder=false) returns the event types ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted'] — a WaitStateAborted inserted between WaitStateEntered and ExecutionAborted. This happens both for a Wait with a literal Seconds and for a Wait whose SecondsPath resolves to a numeric string, and both for a stop that supplies error/cause and one that does not. The DescribeExecution side matches AWS in both checks (ABORTED, caller's error/cause echoed, no output).
Why:      a client that reads execution history gets a state-level event AWS never recorded, and every event id after WaitStateEntered is shifted. Audit/export pipelines, per-state timing or step metrics derived from Entered/Exited pairs, logic asking whether a state was left, history-length comparisons, fixed-index reads and previousEventId chain arithmetic all disagree with AWS for every aborted-inside-Wait execution.
Fix:      the interpreter's StopExecution path and the execution-history writer it drives — aborting a running execution must stop the interpreter at the entered state without appending a per-state WaitStateAborted event, leaving ExecutionAborted as the only abort record.
Traces:   stepfunctions.stop-execution-aborted-event-details-with-and-without-cause, stepfunctions.wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts (both listed under the suite report's ### stepfunctions heading).

## Evidence
Cause 1 — check stepfunctions.map-maxconcurrency-runtime-overflow-and-negative-path-fail-with-states-runtime [mismatch]

Definition under test (ASL): {"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","MaxConcurrency":9223372036854775808,"ItemProcessor":{"ProcessorConfig":{"Mode":"INLINE"},"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":2,"End":true}}},"End":true}}} — the MaxConcurrency literal is Java long max + 1. Input {"items": [1]}.

Expected (recorded AWS): DescribeExecution status FAILED, error "States.Runtime", cause "An error occurred while executing the state 'M' (entered at the event id #2). The MaxConcurrency field cannot be parsed as an integer value: 9223372036854775808".
Actual (emulator): status "SUCCEEDED", output "[1]", no error and no cause member.

Failure text:
  File "check_stepfunctions_map_maxconcurrency_runtime_overflow_and_negative_path_fail_with_states_runtime.py", line 96, in _assert_states_runtime
    assert desc["status"] == "FAILED"
  AssertionError: literal overflow, INLINE: expected FAILED, got SUCCEEDED

The check's remaining arms (same definition with ProcessorConfig Mode DISTRIBUTED/ExecutionType STANDARD; MaxConcurrencyPath "$.mc" with input mc = -5 expecting cause "...MaxConcurrency cannot be negative but was: -5"; MaxConcurrencyPath "$.mc" with mc = 9223372036854775808 under DISTRIBUTED expecting cause '...The MaxConcurrencyPath field refers to value "9223372036854775808" which is not a valid integer: $.mc') were not reached. The positive control (MaxConcurrencyPath "$.mc" with mc = 2 over three items) SUCCEEDED, as AWS does. Execution arns, names, timestamps and request ids in the run are per-run values and are not part of the requirement; the state name 'M', the entry event id #2, the field name and the offending value are the template's variable parts, filled from the definition/request.

Cause 2 — checks stepfunctions.stop-execution-aborted-event-details-with-and-without-cause [mismatch] and stepfunctions.wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts [mismatch]

(a) stop-execution-aborted-event-details-with-and-without-cause: definition {"StartAt":"Park","States":{"Park":{"Type":"Wait","Seconds":3600,"End":true}}}. Two executions started; both polled until WaitStateEntered; one stopped with error "Sim2Real.OperatorHalt" and cause "stopped by the sim2real behavioral suite", the other stopped with neither. DescribeExecution matched AWS for both (ABORTED; error/cause echoed or absent; stopDate equal to StopExecution's answer; a repeated StopExecution idempotent).
  Expected history event types: ['ExecutionStarted', 'WaitStateEntered', 'ExecutionAborted']
  Actual:                       ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted']
  Failure text: File "check_stepfunctions_stop_execution_aborted_event_details_with_and_without_cause.py", line 156, in run / assert types == ["ExecutionStarted", "WaitStateEntered", "ExecutionAborted"] / AssertionError: history of the with cause stop is ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted']

(b) wait-secondspath-numeric-string-51-years-accepted-then-stop-aborts: definition {"StartAt":"Hold","States":{"Hold":{"Type":"Wait","SecondsPath":"$.seconds","Next":"After"},"After":{"Type":"Succeed"}}}, input {"seconds": "1609437600"} (a numeric STRING, ~51 years). The emulator correctly accepted the numeric-string SecondsPath, parked RUNNING in the Wait with history exactly ['ExecutionStarted','WaitStateEntered'], and after StopExecution(error="OperatorAbort", cause="stopped before the 51-year wait could elapse") reported ABORTED with the caller's error/cause and no output.
  Expected history event types: ['ExecutionStarted', 'WaitStateEntered', 'ExecutionAborted']
  Actual:                       ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted']
  Failure text: File "check_stepfunctions_wait_secondspath_numeric_string_51_years_accepted_then_stop_aborts.py", line 91, in run / AssertionError: the aborted run's history must be exactly this triple, with WaitStateEntered as the last-but-one event; got ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted']

The divergence in both is solely the presence of the WaitStateAborted event (and the event ids it shifts); arns, timestamps and event ids are per-run values. Model evidence: com.amazonaws.sfn#HistoryEventType includes WaitStateEntered, WaitStateExited and WaitStateAborted.

## Steps to Reproduce
Cause 1 (Map MaxConcurrency range validation):
1. CreateRole with a trust policy for states.amazonaws.com; CreateStateMachine (type STANDARD) with definition {"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","MaxConcurrency":9223372036854775808,"ItemProcessor":{"ProcessorConfig":{"Mode":"INLINE"},"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":2,"End":true}}},"End":true}}}.
2. StartExecution with input {"items":[1]} and poll DescribeExecution to terminal.
   Expected: status FAILED, error "States.Runtime", cause "An error occurred while executing the state 'M' (entered at the event id #2). The MaxConcurrency field cannot be parsed as an integer value: 9223372036854775808".
   Actual: status SUCCEEDED, output "[1]", no error/cause.
3. Repeat step 1-2 with ProcessorConfig {"Mode":"DISTRIBUTED","ExecutionType":"STANDARD"} — AWS fails identically (mode-independent).
4. Replace MaxConcurrency with MaxConcurrencyPath "$.mc" and run with input {"items":[1],"mc":-5} (INLINE): expected FAILED / States.Runtime / cause "...(entered at the event id #2). MaxConcurrency cannot be negative but was: -5".
5. Same definition with Mode DISTRIBUTED and input {"items":[1],"mc":9223372036854775808}: expected FAILED / States.Runtime / cause '...(entered at the event id #2). The MaxConcurrencyPath field refers to value "9223372036854775808" which is not a valid integer: $.mc'.
6. Control: MaxConcurrencyPath "$.mc" with input {"items":[1,2,3],"mc":2} must SUCCEED (it does).

Cause 2 (spurious WaitStateAborted):
1. CreateStateMachine (STANDARD) with definition {"StartAt":"Park","States":{"Park":{"Type":"Wait","Seconds":3600,"End":true}}}.
2. StartExecution; poll GetExecutionHistory until the event types contain "WaitStateEntered".
3. StopExecution(executionArn, error="Sim2Real.OperatorHalt", cause="stopped by the sim2real behavioral suite"). Repeat on a second execution with no error/cause.
4. Poll DescribeExecution until terminal: ABORTED, caller's error/cause present (or both absent), no output — this matches AWS.
5. Read the full paginated GetExecutionHistory event-type sequence for both executions.
   Expected: ['ExecutionStarted', 'WaitStateEntered', 'ExecutionAborted'] with executionAbortedEventDetails = {"error":...,"cause":...} on the stop that supplied them and {} on the bare stop; no WaitStateExited, no per-state abort event.
   Actual: ['ExecutionStarted', 'WaitStateEntered', 'WaitStateAborted', 'ExecutionAborted'].
6. Repeat with a Wait driven by SecondsPath resolving to a numeric string ({"StartAt":"Hold","States":{"Hold":{"Type":"Wait","SecondsPath":"$.seconds","Next":"After"},"After":{"Type":"Succeed"}}}, input {"seconds":"1609437600"}) — the extra WaitStateAborted appears there too.
