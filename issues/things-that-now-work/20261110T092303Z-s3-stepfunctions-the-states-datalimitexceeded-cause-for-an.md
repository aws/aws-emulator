Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected
Issue-Key: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected|run#16|AssertionError|assert bad.get("cause") == expected_cause, f"expected cause {expected_cause!r}, got {bad.get('cause')!r}"
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# s3__stepfunctions: the States.DataLimitExceeded cause for an oversize aws-sdk task result is emulator-authored instead of AWS's "The state/task '<resource>' returned a result with a size exceeding the maximum number of bytes service limit."

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution (Task state with Resource arn:aws:states:::aws-sdk:s3:getObject) / GetExecutionHistory

## Description
Cause 1 of 1: the failure cause Step Functions reports when an optimized/aws-sdk Task state produces a result larger than the 262144-byte state-payload limit does not use AWS's wording.
AWS: recorded behavior only — the States.DataLimitExceeded name and its cause text are runtime values and are not in the Smithy model. For an aws-sdk integration task whose result exceeds the limit, the execution ends FAILED with error "States.DataLimitExceeded" and the fixed cause "The state/task '<task resource>' returned a result with a size exceeding the maximum number of bytes service limit." — the only variable part is the task resource identifier ('aws-sdk:s3' for arn:aws:states:::aws-sdk:s3:getObject), taken from the state's Resource. The execution produces no output.
Emulator: the error name is correct but the cause is an emulator-authored sentence that names the state and a byte count instead: "the 'Task' state '<state name>' produced a document of at least <N> bytes, more than the 262144 a state's output may carry" (the state name and the byte count are per-run values; the whole sentence is not AWS's text).
Why: a client that matches on the documented cause string — to distinguish an oversize-result failure from the other States.DataLimitExceeded sources, or to extract the offending task resource from the cause — does not recognise the failure; the same cause also lands in the execution history's ExecutionFailed / TaskStateExited events and in any Catch/Retry handler that inspects $.Cause, so error-handling branches keyed on AWS's text never fire.
Fix: the state-machine interpreter's data-limit check — raise States.DataLimitExceeded with AWS's fixed cause text, interpolating the task's resource identifier rather than the state name and an observed size.
Traces: s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected.

## Evidence
Expected cause: "The state/task 'aws-sdk:s3' returned a result with a size exceeding the maximum number of bytes service limit."
Actual cause: "the 'Task' state 'Get' produced a document of at least 307235 bytes, more than the 262144 a state's output may carry"
(error name is States.DataLimitExceeded in both; the status FAILED and the absence of output match. 'Get' and 307235 are this run's state name and body size, not part of AWS's template.)

## Steps to Reproduce
Replay the behavioral suite and re-run s3__stepfunctions.sfn-aws-sdk-s3-getobject-result-over-256kib-rejected. By hand: put an object larger than 262144 bytes in a bucket; create a state machine with a single Task state whose Resource is arn:aws:states:::aws-sdk:s3:getObject, Parameters {Bucket, Key.$} and ResultSelector {"content.$": "$.Body"}; start an execution naming that key, wait for the terminal status and read DescribeExecution's error/cause (and the ExecutionFailed event in GetExecutionHistory).
