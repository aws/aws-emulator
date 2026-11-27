Issue-Source: trace-sweep-divergence
Issue-Trace: fis-stop-experiment-id-constraint-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-stop-experiment-id-constraint-errors
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|StopExperiment|4xx->4xx|$.__type
Issue-Key: fis|StopExperiment|4xx->4xx|$.message
Issue-Key: fis|StopExperiment|4xx->4xx|status
Issue-Commit: 0f328bb6ae93185eda2a76c9c7fc6646652b63ca

# fis: StopExperiment rejects a whitespace-bearing experiment id as ValidationException instead of returning ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `0f328bb6ae93185eda2a76c9c7fc6646652b63ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopExperiment

## Description
1 cause.

Cause 1 of 1: StopExperiment applies the ExperimentId @pattern as a pre-dispatch validation gate, so an id that contains whitespace is answered with a 400 ValidationException where AWS performs the lookup and answers 404 ResourceNotFoundException (fixture fis-stop-experiment-id-constraint-errors, step task#3).
AWS: Smithy: StopExperiment DELETE /experiments/{id}, input StopExperimentRequest.id -> ExperimentId, @httpLabel, @required, @length(min 0, max 64), @pattern "^[\\S]+$"; modeled errors ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior shows AWS does NOT turn the ^[\\S]+$ pattern on this httpLabel into a request-rejection for StopExperiment: the recorded response for an id containing an embedded space is HTTP 404 with __type "ResourceNotFoundException" and message "Experiment with ID <id> not found." — fixed text "Experiment with ID " and " not found.", with <id> the id taken verbatim from the request path (in the recording, the whitespace-bearing id the step sent). Hypothesis: the pattern is enforced only where the label is used to mint a resource, while the stop path resolves the decoded label against experiment state and reports a miss. Other steps of the same fixture (the length-bound cases) pass, so the emulator's ValidationException shape itself is right — only this precedence is wrong.
Emulator: returns HTTP 400 with __type "ValidationException" and message "1 validation error detected: Value '<id>' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\S]+$", i.e. it rejects on the pattern before any state lookup. Expected status 404, actual 400; both __type and message diverge.
Why: a client stopping an experiment distinguishes "this id does not exist (or is already gone)" from "this request is malformed". ResourceNotFoundException is the retryable/idempotent-cleanup path — teardown code catches it and proceeds — while ValidationException is a non-retryable programming error that aborts the run. Against the emulator any id the client builds by concatenation or reads from a file with a stray space surfaces as a hard client-side failure instead of a benign not-found, so idempotent stop/cleanup logic that works on AWS breaks here; conversely a test asserting the not-found message never sees it.
Fix: the StopExperiment handler's input validation — do not reject the {id} httpLabel on the ^[\\S]+$ pattern ahead of resolution; keep the @length(max 64) check, resolve the decoded id against experiment state, and on a miss return ResourceNotFoundException with HTTP 404 and the message template "Experiment with ID " + the request's id + " not found.".
Traces: fis-stop-experiment-id-constraint-errors#task#3.

Note: no open fis issue covers StopExperiment error precedence; 20261127T111958Z-fis-kinesis-2-net-new-causes-createexperimenttemplate-accepts.md concerns CreateExperimentTemplate parameter ranges and is unrelated.

## Evidence
fis-stop-experiment-id-constraint-errors#task#3 (StopExperiment, DELETE /experiments/{id} with an id containing an embedded space):
  expected: status 404, $.__type = "ResourceNotFoundException", $.message = "Experiment with ID sim2real-probe-fis-f3fcefd4 x not found."
  actual:   status 400, $.__type = "ValidationException", $.message = "1 validation error detected: Value 'sim2real-probe-fis-f3fcefd4 x' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\S]+$"
Message breakdown for the required 404: fixed text is "Experiment with ID " and " not found."; the id between them is the value from the request path (the recording's "sim2real-probe-fis-f3fcefd4 x" is one run's generated probe id and is not part of the template). Smithy confirms both errors are modeled on StopExperiment (ResourceNotFoundException httpError 404, ValidationException httpError 400), so the divergence is which one wins, not whether the emulator knows the shape.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Replay fixture fis-stop-experiment-id-constraint-errors; steps task#1/task#2 (length-bound ids) pass.
3. At step task#3, call StopExperiment (DELETE /experiments/{id}) with an id that is <=64 characters but contains an embedded space and that matches no existing experiment, e.g. "<some-id> x".
4. Emulator answers 400 ValidationException citing the ^[\\S]+$ pattern; AWS answers 404 ResourceNotFoundException with "Experiment with ID <id> not found.".
