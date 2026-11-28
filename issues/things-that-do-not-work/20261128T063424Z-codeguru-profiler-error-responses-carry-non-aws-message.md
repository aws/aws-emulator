Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-frame-metrics-on-deleted-group-not-found
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-frame-metrics-on-deleted-group-not-found codeguruprofiler-recommendations-missing-group-not-found codeguruprofiler-frame-metrics-request-validation
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|BatchGetFrameMetricData|4xx->4xx|$.message
Issue-Key: codeguru-profiler|GetRecommendations|4xx->4xx|$.message
Issue-Commit: ed3ed06301643834f5ccb593614cfd988c817634

# codeguru-profiler error responses carry non-AWS message text for missing profiling groups and frame-metric time-range validation

> _Note: this issue was identified against the code base at commit `ed3ed06301643834f5ccb593614cfd988c817634`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetFrameMetricData

## Description
Three replayed clusters diverge on the `message` field of CodeGuru Profiler error bodies. The HTTP status and the `__type`/error code match AWS in every case; only the human-readable message the service is contractually required to emit is the emulator's own invented wording. Both error shapes declare `message` as a REQUIRED member (Smithy: ResourceNotFoundException {message: String, required} httpError 404, doc "The resource specified in the request does not exist."; ValidationException {message: String, required} httpError 400, doc "The parameter is not valid."), so the message is part of the modelled response, not decoration.

Cause 1 of 3: BatchGetFrameMetricData on a profiling group that does not exist (here, one that was deleted earlier in the fixture) returns the right status and error shape but the wrong message.
AWS: 404 ResourceNotFoundException whose message is the fixed template `Profiling group for <accountId>/<profilingGroupName> does not exist!` - fixed text is "Profiling group for ", the "/" separator and " does not exist!"; the variable parts are the caller's AWS account id and the `profilingGroupName` taken from the request URI label (Smithy: BatchGetFrameMetricData POST /profilingGroups/{profilingGroupName}/frames/-/metrics, errors include ResourceNotFoundException @httpError 404; exact message template is recorded behavior only - the model documents the condition, not the string).
Emulator: 404 with message `CodeGuru Profiler can\u2019t complete the operation because <profilingGroupName> was not found.` - a different template entirely: it omits the account-scoped resource identity and uses prose AWS does not use for this error.
Why: clients and test suites that match on the not-found message (log scrapers, retry/branch logic, assertions in customer CI) see text that never appears in the real service, and the account-qualified resource identity AWS includes - which is what tells a caller the group is missing in THIS account rather than misspelled - is absent.
Fix: the ResourceNotFoundException construction used by the CodeGuru Profiler profiling-group lookup (the shared not-found error helper in the service's handler/error module), so every operation resolving a profiling group renders the account-qualified AWS template.
Traces: codeguruprofiler-frame-metrics-on-deleted-group-not-found#task#3 (and task#4 in the same cluster).

Cause 2 of 3: GetRecommendations on a profiling group that was never created shows the same wrong not-found message, confirming the defect is in the shared error helper rather than one handler.
AWS: 404 ResourceNotFoundException with the same `Profiling group for <accountId>/<profilingGroupName> does not exist!` template, variable parts again the caller's account id and the request's `profilingGroupName` label (Smithy: GetRecommendations GET /internal/profilingGroups/{profilingGroupName}/recommendations, errors include ResourceNotFoundException @httpError 404; message template is recorded behavior only).
Emulator: 404 with message `CodeGuru Profiler can\u2019t complete the operation because <profilingGroupName> was not found.`
Why: same as cause 1 - the not-found message is wrong for a second operation, so any client-side handling keyed on AWS's wording fails across the whole service surface, not just one API.
Fix: same shared ResourceNotFoundException helper; GetRecommendations' handler must not need its own copy once the helper is corrected.
Traces: codeguruprofiler-recommendations-missing-group-not-found#task#3.

Cause 3 of 3: BatchGetFrameMetricData's time-range validation rejects the request with the correct status and error shape but a message that states a different (and weaker) rule than AWS's.
AWS: `startTime`, `endTime` and `period` are three independent optional query parameters (Smithy: BatchGetFrameMetricDataRequest.startTime @httpQuery "startTime" timestamp date-time, .endTime @httpQuery "endTime" timestamp date-time, .period @httpQuery "period" @length 1..64 - none required), and the service requires EXACTLY TWO of them to be supplied, deriving the third. Supplying all three (as the fixture does) is a 400 ValidationException whose message is the fixed template ``Please specify exactly two of `startTime` (got `<startTime>`), `endTime` (got `<endTime>`) and `period` (got `<period>`).`` - the fixed text is "Please specify exactly two of", the backquoted parameter names in that order, the parenthesised "(got `...`)" for each, and the trailing period; the variable parts are the three values as they arrived on the query string (date-time timestamps for startTime/endTime, the ISO-8601 duration for period). Requirement is recorded behavior only for the exact string; the "exactly two" rule is what the recorded response states.
Emulator: 400 ValidationException with message `specify 1 or 2 of startTime, endTime and period, not all 3` - different fixed text, no parameter values echoed back, and it asserts a looser rule (1 or 2 accepted) than AWS's "exactly two", so a request carrying only one of the three is described as valid when AWS rejects it.
Why: a client that supplies one parameter is told by the emulator's own message that this is legal and (per the stated rule) is not rejected, so code written against the emulator breaks on AWS; and callers that surface the service message to users lose the echoed-back offending values AWS provides for diagnosis.
Fix: BatchGetFrameMetricData's input validation - enforce the exactly-two-of-three predicate and render the AWS ValidationException template with the request's own startTime/endTime/period values substituted.
Traces: codeguruprofiler-frame-metrics-request-validation#task#4 (and task#5 in the same cluster).

Not covered by existing open issues: 20261127T103329Z-codeguruprofiler-batchgetframemetricdata-computes-aggregatedrelativetotaltime-against-the-wrong-denominator concerns BatchGetFrameMetricData success-path arithmetic, and 20261022T213013Z-codeguru-profiler-postagentprofile-accepts-an-invalid-agent-profile-with-204 concerns PostAgentProfile; neither carries these error-message replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (13 fixtures, 86 steps, 80 passed, 6 mismatched):

1) fixture codeguruprofiler-frame-metrics-on-deleted-group-not-found step task#3 (affected also task#4), BatchGetFrameMetricData, verdict mismatch, expected_status 404 / actual_status 404:
   $.message: value_mismatch
   expected: 'Profiling group for 000000000000/sim2real-probe-cgp-fm-2320dbf5 does not exist!'
   actual:   'CodeGuru Profiler can\u2019t complete the operation because sim2real-probe-cgp-fm-2320dbf5 was not found.'
   (000000000000 is the recorded account id and sim2real-probe-cgp-fm-2320dbf5 the requested group name - both are per-run values filling the template, not requirements.)

2) fixture codeguruprofiler-recommendations-missing-group-not-found step task#3, GetRecommendations, verdict mismatch, expected_status 404 / actual_status 404:
   $.message: value_mismatch
   expected: 'Profiling group for 000000000000/sim2real-probe-cgp-absent-d8b4040a does not exist!'
   actual:   'CodeGuru Profiler can\u2019t complete the operation because sim2real-probe-cgp-absent-d8b4040a was not found.'

3) fixture codeguruprofiler-frame-metrics-request-validation step task#4 (affected also task#5), BatchGetFrameMetricData, verdict mismatch, expected_status 400 / actual_status 400:
   $.message: value_mismatch
   expected: 'Please specify exactly two of `startTime` (got `2026-11-28T05:32:52.107Z`), `endTime` (got `2026-11-28T06:32:52.107Z`) and `period` (got `PT1H`).'
   actual:   'specify 1 or 2 of startTime, endTime and period, not all 3'
   (The two timestamps and PT1H are the request's own query values substituted into the template.)

Status and error code agree in all three; the divergence is confined to the required `message` member, and in case 3 the emulator's message also states a different validation rule than AWS enforces.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: create a profiling group, delete it, then POST /profilingGroups/<name>/frames/-/metrics (BatchGetFrameMetricData) for it. Observe 404 ResourceNotFoundException whose message is 'CodeGuru Profiler can\u2019t complete the operation because <name> was not found.' instead of 'Profiling group for <accountId>/<name> does not exist!'. Fixture codeguruprofiler-frame-metrics-on-deleted-group-not-found, step task#3.
3. Cause 2: GET /internal/profilingGroups/<name>/recommendations?startTime=...&endTime=... (GetRecommendations) for a profiling group that was never created. Observe the same wrong not-found message. Fixture codeguruprofiler-recommendations-missing-group-not-found, step task#3.
4. Cause 3: POST /profilingGroups/<name>/frames/-/metrics with all three of startTime, endTime and period on the query string. Observe 400 ValidationException with message 'specify 1 or 2 of startTime, endTime and period, not all 3' instead of AWS's 'Please specify exactly two of `startTime` (got `<startTime>`), `endTime` (got `<endTime>`) and `period` (got `<period>`).'; then repeat with only startTime supplied and observe the emulator does not reject a request AWS rejects. Fixture codeguruprofiler-frame-metrics-request-validation, step task#4.
