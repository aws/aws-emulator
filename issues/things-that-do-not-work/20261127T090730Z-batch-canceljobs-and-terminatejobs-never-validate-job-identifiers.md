Issue-Source: trace-sweep-divergence
Issue-Trace: batch-cancel-jobs-refuses-malformed-ids-per-entry
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-cancel-jobs-refuses-malformed-ids-per-entry
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|CancelJobs|2xx->2xx|$.errors[*]
Issue-Key: batch|CancelJobs|2xx->2xx|$.successful[*]
Issue-Key: batch|TerminateJobs|2xx->2xx|$.errors[*]
Issue-Key: batch|TerminateJobs|2xx->2xx|$.successful[*]
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# batch: CancelJobs and TerminateJobs never validate job identifiers per entry, so a malformed id is reported successful instead of a ValidationException errors entry (4 causes)

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJobs

## Description
FOUR net-new divergence clusters of the simparity-wire-aws-v1 replay, all in AWS Batch, all one shared fault: the bulk job operations CancelJobs and TerminateJobs do no per-entry validation of the job identifiers in `jobs`. AWS checks each identifier's SHAPE before it tries to address a job: an identifier that does not match the Batch job-identifier character class is reported as a per-entry `errors` item with code `ValidationException`, and is NOT listed in `successful`. The emulator pushes EVERY supplied string into `successful` and always serializes `errors` as an empty list, so a malformed identifier is reported to the caller as an accepted cancellation/termination. Each cause below is one divergence cluster; a fixer must close all four before submitting. No open issue carries these replay keys. Related open batch issues that do NOT cover this (they concern other operations and other refusal paths): '20261102T113649Z-batch-job-api-refusal-contracts-diverge-two-wrong.md' (DescribeJobs/ListJobs/SubmitJob/RegisterJobDefinition refusals) and '20261024T120647Z-batch-overly-permissive-control-and-service-job-planes.md' (quota-share / service-job planes).

=== Cause 1 of 4: CancelJobs reports malformed identifiers as successful (whitespace and '#' in the id) ===
AWS: Smithy com.amazonaws.batch#CancelJobs binds POST /v1/canceljobs, success 200, input {jobs: StringList (required), reason: String (required)}, output {successful: StringList (required), errors: CancelJobsErrorDetailList (required)}; CancelJobsErrorDetail requires {job, code, message} and its `code` doc enumerates exactly 'ValidationException - A job identifier in the request is malformed or isn't valid.', ClientException, ThrottlingException, ServerException, AccessDenied. The operation doc states 'Batch reports the result for each job individually in the response... The response returns an HTTP status code of 200 even when some jobs encountered errors, so check the errors list' and 'Jobs that can't be found are treated as successfully processed' — i.e. NOT-FOUND is a success, but MALFORMED is an errors entry. The identifier character class itself is recorded behavior only; the recording shows HTTP 200 whose body carries, for each malformed identifier, an errors item {job: <the identifier as supplied>, code: 'ValidationException', message: 'Provided name <the identifier as supplied> is not supported pattern[a-zA-Z_0-9-]{1,128}'}, and that identifier absent from `successful`. In that message the fixed text is 'Provided name ' ... ' is not supported pattern[a-zA-Z_0-9-]{1,128}' (note: no space before 'pattern'); the only variable part is the identifier echoed from the request's `jobs` entry.
Emulator: HTTP 200 with `errors` empty and every identifier echoed into `successful` — the two malformed ids in the request appear at $.successful[0] and $.successful[1] and no $.errors[0]/$.errors[1] is produced.
Why: a client that submits a mistyped or user-supplied job identifier is told the cancellation was accepted. Code that iterates the documented `errors` list to surface bad input finds nothing to report, so malformed identifiers are silently dropped and the job the caller meant to cancel keeps running — the exact failure mode the per-entry errors contract exists to prevent.
Fix: per-entry input validation in the CancelJobs handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/cancel_jobs.rs, handle() lines 191-215, where `errors: Vec::new()` is hard-coded and every id is unconditionally pushed to `successful`): validate each `jobs` entry against the job-identifier character class and emit a CancelJobsErrorDetail with code 'ValidationException' and AWS's message instead of listing it as successful.
Traces: batch-cancel-jobs-refuses-malformed-ids-per-entry#task#1.

=== Cause 2 of 4: CancelJobs reports an over-length identifier and a dotted identifier as successful ===
AWS: same model facts as cause 1 (CancelJobs / CancelJobsErrorDetail.code 'ValidationException - A job identifier in the request is malformed or isn't valid.'). The pattern in the message bounds LENGTH as well as charset ({1,128}), so an identifier longer than 128 characters is malformed even when every character is in the class, and so is an identifier containing '.'; recorded behavior: HTTP 200 with one ValidationException errors entry per malformed identifier, message templated as in cause 1, and those identifiers absent from `successful` while the well-formed identifiers in the same request remain successful.
Emulator: HTTP 200 with `errors` empty; the over-length identifier and the dotted identifier are echoed into `successful` ($.successful[1], $.successful[2]) and no errors entries are produced.
Why: the length and charset bound of a Batch job identifier is unobservable through the emulator, so a client generating identifiers (or forwarding them from another system) cannot discover that AWS will reject them; the mixed request's partial-failure semantics — some entries successful, some in errors — never occur.
Fix: same validation seam as cause 1 — the per-entry check must cover length (1..=128) as well as charset, and must leave the request's well-formed entries in `successful`.
Traces: batch-cancel-jobs-refuses-malformed-ids-per-entry#task#2.

=== Cause 3 of 4: TerminateJobs reports a whitespace-bearing malformed identifier as successful ===
AWS: Smithy com.amazonaws.batch#TerminateJobs binds POST /v1/terminatejobs, success 200, input {jobs (required), reason (required)}, output {successful (required), errors: TerminateJobsErrorDetailList (required)}; TerminateJobsErrorDetail requires {job, code, message} and its `code` doc enumerates 'ValidationException - A job identifier in the request is malformed or isn't valid.' alongside ClientException/ThrottlingException/ServerException/AccessDenied. Operation doc: results are reported per job, 200 is returned even when some jobs encountered errors, and jobs that can't be found are treated as successfully processed. Recorded behavior for a malformed identifier: HTTP 200 with errors entry {job: <identifier>, code: 'ValidationException', message: 'Provided name <identifier> is not supported pattern[a-zA-Z_0-9-]{1,128}'} and the identifier absent from `successful`.
Emulator: HTTP 200 with `errors` empty and the malformed identifier echoed into `successful` ($.successful[0]); no $.errors[0].
Why: the bulk terminate path reports success for an identifier AWS refuses, so a client believes a RUNNING job was terminated when nothing was addressed — and the ValidationException branch of TerminateJobsErrorDetail is dead code.
Fix: per-entry input validation in the TerminateJobs handler (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/terminate_jobs.rs, handle() lines 185-210, which hard-codes `errors: Vec::new()` and pushes every id to `successful`). The identifier check belongs in one shared helper used by both bulk handlers so the two operations cannot diverge.
Traces: batch-cancel-jobs-refuses-malformed-ids-per-entry#task#3.

=== Cause 4 of 4: TerminateJobs reports a '#'-bearing malformed identifier as successful ===
AWS: same model facts as cause 3; recorded behavior is HTTP 200 carrying one ValidationException errors entry for the identifier containing characters outside [a-zA-Z_0-9-], with the message template of cause 3, and that identifier absent from `successful`.
Emulator: HTTP 200 with `errors` empty and the identifier echoed into `successful` ($.successful[1]); no $.errors[0].
Why: as cause 3 — the per-entry refusal AWS makes for a malformed identifier never happens, so a partially-bad bulk terminate is indistinguishable from a fully accepted one.
Fix: same shared per-entry validation seam as cause 3.
Traces: batch-cancel-jobs-refuses-malformed-ids-per-entry#task#4.

## Evidence
From the simparity-wire-aws-v1 replay against the emulator built from this checkout (trace_set_id simparity-wire-aws-v1; 202 steps, 188 passed, 14 mismatched, score 93.1%; emulator_up true, bootstrap_signal false). The four clusters below are the only ones with no `stub_path` and no `owned_by`.

Cause 1 — fixture batch-cancel-jobs-refuses-malformed-ids-per-entry, step task#1, operation CancelJobs, verdict mismatch, expected 200 / actual 200:
  $.successful[0]: extra (expected None, got 'sim2real probe job')
  $.successful[1]: extra (expected None, got 'sim2real#probe#job')
  $.errors[0]: missing (expected {'job': 'sim2real probe job', 'code': 'ValidationException', 'message': 'Provided name sim2real probe job is not supported pattern[a-zA-Z_0-9-]{1,128}'}, got None)
  $.errors[1]: missing (expected {'job': 'sim2real#probe#job', 'code': 'ValidationException', 'message': 'Provided name sim2real#probe#job is not supported pattern[a-zA-Z_0-9-]{1,128}'}, got None)

Cause 2 — same fixture, step task#2, operation CancelJobs, expected 200 / actual 200:
  $.successful[1] and $.successful[2]: extra (an identifier of 128+ characters, and 'sim2real.probe.job')
  $.errors[0]/$.errors[1]: missing — each expected {'job': <that identifier>, 'code': 'ValidationException', 'message': 'Provided name <that identifier> is not supported pattern[a-zA-Z_0-9-]{1,128}'}

Cause 3 — same fixture, step task#3, operation TerminateJobs, expected 200 / actual 200:
  $.successful[0]: extra (expected None, got 'sim2real probe job')
  $.errors[0]: missing (expected {'job': 'sim2real probe job', 'code': 'ValidationException', 'message': 'Provided name sim2real probe job is not supported pattern[a-zA-Z_0-9-]{1,128}'}, got None)

Cause 4 — same fixture, step task#4, operation TerminateJobs, expected 200 / actual 200:
  $.successful[1]: extra (expected None, got 'sim2real#probe#job')
  $.errors[0]: missing (expected {'job': 'sim2real#probe#job', 'code': 'ValidationException', 'message': 'Provided name sim2real#probe#job is not supported pattern[a-zA-Z_0-9-]{1,128}'}, got None)

The recorded identifier strings above are the fixtures' probe values; the requirement is the template ('Provided name ' + the identifier echoed from the request + ' is not supported pattern[a-zA-Z_0-9-]{1,128}'), with the identifier taken from the request, not any one recording's value.

Code: services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/cancel_jobs.rs lines 191-215 and src/ops/terminate_jobs.rs lines 185-210 both loop over `input.jobs`, call state.cancel_job/terminate_job for each, unconditionally push the id onto `successful`, and return `errors: Vec::new()` with the comment 'Every id this service could address was processed' — there is no identifier-shape check anywhere on either path. The only refusal either handler makes is the whole-request MAX_BULK_JOBS bound (ClientException 400).

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json', query='operation', operation='CancelJobs,TerminateJobs') and query='shape', shape='CancelJobsErrorDetail,TerminateJobsErrorDetail' — POST /v1/canceljobs and POST /v1/terminatejobs, success 200, errors ClientException(400)/ServerException(500); outputs require both `successful` and `errors`; the error-detail structures require job/code/message and document 'ValidationException - A job identifier in the request is malformed or isn't valid.' The identifier pattern and the message wording are recorded behavior only (the model places no @pattern/@length on StringList).

## Steps to Reproduce
1. git_clone, then replay_wire_corpus and read the batch clusters with empty stub_path/owned_by.
2. Cause 1: POST /v1/canceljobs with {"jobs": ["sim2real probe job", "sim2real#probe#job"], "reason": "probe"} (aws batch cancel-jobs --jobs 'sim2real probe job' 'sim2real#probe#job' --reason probe) -> emulator answers 200 with {"successful": [both ids], "errors": []} instead of 200 with errors=[{job, code 'ValidationException', message 'Provided name <id> is not supported pattern[a-zA-Z_0-9-]{1,128}'} per id] and those ids absent from successful.
3. Cause 2: POST /v1/canceljobs with a jobs list mixing one well-formed id, one id longer than 128 characters and 'sim2real.probe.job' -> all three come back in successful with errors=[]; AWS keeps only the well-formed id in successful and reports the other two as ValidationException errors entries.
4. Cause 3: POST /v1/terminatejobs with {"jobs": ["sim2real probe job"], "reason": "probe"} -> 200 with successful=['sim2real probe job'], errors=[] instead of a ValidationException errors entry.
5. Cause 4: POST /v1/terminatejobs with a jobs list containing 'sim2real#probe#job' -> that id appears in successful with errors=[] instead of a ValidationException errors entry.
6. Re-replay fixture batch-cancel-jobs-refuses-malformed-ids-per-entry (steps task#1..task#4) to confirm all four causes.
