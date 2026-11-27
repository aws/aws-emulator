Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-notebook-run-identifier-validation
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-notebook-run-identifier-validation
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|StopNotebookRun|4xx->4xx|$.message
Issue-Key: datazone|StartNotebookRun|4xx->4xx|$.message
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# datazone notebook-run identifier ValidationException message interpolates the rejected value

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopNotebookRun

## Description
Two net-new causes sharing one fault: the emulator builds DataZone's pattern-constraint ValidationException message with the offending value spliced in, while DataZone's notebook-run identifier validation emits the value-free form of the message. Status (400) and error class (ValidationException) already match in both causes; only $.message diverges.

Cause 1 of 2: StopNotebookRun 'identifier' pattern failure.
  AWS: Smithy - StopNotebookRun (PUT /v2/domains/{domainIdentifier}/notebook-runs/{identifier}/stop) input member identifier targets NotebookRunId with @pattern ^[a-zA-Z0-9_-]{1,36}$ and is required; modeled error ValidationException, httpError 400. The recorded response is HTTP 400 ValidationException with message "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}". The fixed text of this message class is "1 validation error detected: Value at '" + <member name> + "' failed to satisfy constraint: Member must satisfy regular expression pattern: " + <the member's pattern, without the ^...$ anchors>; the only variable parts are the member name taken from the request binding and the pattern taken from the model. Notably AWS does NOT echo the rejected value here (recorded behavior only - the model marks neither NotebookRunId nor NotebookId @sensitive, but the live service suppresses the value for these identifiers).
  Emulator: returns 400 ValidationException with "1 validation error detected: Value 'run.with.dots' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}" - i.e. it inserts "Value '<request value>' at" where AWS writes "Value at". ('run.with.dots' is just this recording's request value; the defect is the inserted value clause, not that literal.)
  Why: a client or test that matches DataZone's validation message - or an operator reading logs - sees a message AWS never produces; worse, the emulator echoes caller-supplied identifier text back into an error body that AWS deliberately keeps value-free, so anything that treats the message as a stable contract (or that must not reflect request content) diverges.
  Fix: the shared input-validation/error-message formatter used by the datazone notebook-run operations (the pattern-constraint branch that renders the ValidationException message), not the per-operation handler logic.
  Traces: datazone-notebook-run-identifier-validation#task#1 (cluster also covers task#2 and task#3, the same message on other malformed identifier values).

Cause 2 of 2: StartNotebookRun 'notebookIdentifier' pattern failure.
  AWS: Smithy - StartNotebookRun (POST /v2/domains/{domainIdentifier}/notebook-runs) input member notebookIdentifier targets NotebookId with @pattern ^[a-zA-Z0-9_-]{1,36}$ and is required; modeled error ValidationException, httpError 400. Recorded response: HTTP 400 ValidationException, message "1 validation error detected: Value at 'notebookIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}" - same template as Cause 1 with the member name 'notebookIdentifier', again with no value echoed.
  Emulator: returns 400 ValidationException with "1 validation error detected: Value 'notebook.with.dots' at 'notebookIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}".
  Why: same as Cause 1 - the message text for a body member's pattern violation does not match AWS, so message-based assertions and error surfacing break.
  Fix: same formatter; the fix must cover body members (StartNotebookRun.notebookIdentifier) as well as httpLabel members (StopNotebookRun.identifier).
  Traces: datazone-notebook-run-identifier-validation#task#4 (cluster also covers task#5).

## Evidence
Cause 1 - fixture datazone-notebook-run-identifier-validation step task#1 (cluster: task#1, task#2, task#3), StopNotebookRun: expected 400, actual 400; $.message value_mismatch - expected "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}", actual "1 validation error detected: Value 'run.with.dots' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}".
Cause 2 - same fixture step task#4 (cluster: task#4, task#5), StartNotebookRun: expected 400, actual 400; $.message value_mismatch - expected "1 validation error detected: Value at 'notebookIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,36}", actual "...Value 'notebook.with.dots' at 'notebookIdentifier'...".
Fixed vs variable: fixed text = "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; variable = member name (from the request's bound member) and pattern (from the model). The rejected value is NOT part of the message.

## Steps to Reproduce
1. Replay fixture datazone-notebook-run-identifier-validation against the emulator (replay_wire_corpus).
2. Step task#1: call StopNotebookRun with a notebook-run identifier that violates ^[a-zA-Z0-9_-]{1,36}$ (e.g. one containing dots). Response is 400 ValidationException, but $.message contains a "Value '<rejected value>' at" clause AWS omits.
3. Step task#4: call StartNotebookRun with a notebookIdentifier violating the same pattern; the 400 ValidationException message again interpolates the rejected value.
