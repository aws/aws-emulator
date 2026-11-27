Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.cancel-jobs-refuses-malformed-entries-beside-accepted-job batch.cancel-jobs-reports-malformed-id-in-errors-beside-cancelled-job batch.terminate-jobs-refuses-malformed-entry-beside-accepted-jobs
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.cancel-jobs-refuses-malformed-entries-beside-accepted-job|run#28|AssertionError|assert mixed["successful"] == [target], (
Issue-Key: batch.cancel-jobs-reports-malformed-id-in-errors-beside-cancelled-job|run#22|AssertionError|assert sorted(resp["successful"]) == sorted([job_id, unknown_id]), (
Issue-Key: batch.terminate-jobs-refuses-malformed-entry-beside-accepted-jobs|run#27|AssertionError|assert sorted(mixed["successful"]) == sorted([target, unknown]), (
Issue-Commit: dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e

# batch: CancelJobs and TerminateJobs do no per-entry job-identifier validation, so a malformed id is reported in successful instead of a ValidationException errors entry (2 causes)

> _Note: this issue was identified against the code base at commit `dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJobs

## Description
Two net-new root causes grounded in failing behavioral checks the suite listed under `### batch`, none of them marked ALREADY FILED or OWNED by an open issue. Both are the same seam seen on two operations: the bulk job operations CancelJobs and TerminateJobs push every string supplied in `jobs` into `successful` and always serialize `errors` as an empty list, so an identifier AWS refuses per entry is reported to the caller as an accepted cancellation/termination. A fixer must close both causes before submitting.

Related open issue that does NOT carry these checks' replay keys: '20261127T090730Z-batch-canceljobs-and-terminatejobs-never-validate-job-identifiers.md' — it is a simparity-wire-aws-v1 issue whose Issue-Keys are wire keys (batch|CancelJobs|2xx->2xx|$.errors[*] etc.) and whose Issue-Trace-Ids name the wire fixture only; the three behavioral checks below are not covered by it and are the replay units for this filing. The same fault is now also observable end-to-end from the job plane: the behavioral checks additionally show that the refusal must happen beside a real RUNNABLE job that IS cancelled/terminated in the same call.

=== Cause 1 of 2: CancelJobs lists a malformed job identifier as successful and emits no errors entry ===
AWS: Smithy com.amazonaws.batch#CancelJobs binds POST /v1/canceljobs, success 200, input {jobs: StringList (required), reason: String (required)}, output {successful: StringList (required), errors: CancelJobsErrorDetailList (required)}; modeled errors are only ClientException (400) and ServerException (500) — a malformed entry is NOT a whole-request failure. CancelJobsErrorDetail requires {job, code, message} and its `code` documentation enumerates 'ValidationException - A job identifier in the request is malformed or isn't valid.'; the operation doc states the result is reported for each job individually, that 200 is returned even when some jobs encountered errors, and that jobs that can't be found are treated as successfully processed (so an unknown but well-formed id IS successful). The identifier character class and the message wording are recorded behavior only (the model places no @pattern/@length on StringList): HTTP 200 whose body carries, for each identifier that is not 1..128 characters of [a-zA-Z_0-9-], an errors item {job: <the identifier exactly as supplied>, code: 'ValidationException', message: 'Provided name <that identifier> is not supported pattern[a-zA-Z_0-9-]{1,128}'}, and that identifier absent from `successful`, while the well-formed identifiers named in the same request stay in `successful`. In that message the fixed text is 'Provided name ' … ' is not supported pattern[a-zA-Z_0-9-]{1,128}' (no space before 'pattern'); the only variable part is the identifier echoed from the request's `jobs` entry — a recorded probe string is one filling of that template, not part of it.
Emulator: HTTP 200 with `errors` == [] and EVERY supplied string echoed into `successful`. With jobs = [an id containing spaces, a real RUNNABLE job id, a 129-character id], the emulator answered successful = all three ids and errors = [] (the check asserted successful == [the real job id] only). With jobs = [a real job id, an unknown well-formed UUID, an id carrying characters outside the class], it answered 3 ids in `successful` including the malformed one and errors = [] (expected 2 successful ids and one errors entry). No ValidationException detail is ever produced on this path.
Why: a client that submits a mistyped, truncated or user-supplied job identifier is told the cancellation was accepted. Code that iterates the documented `errors` list to surface bad input finds nothing to report, so the malformed identifier is silently dropped and the job the caller meant to cancel keeps running — exactly the failure mode the per-entry errors contract exists to prevent. The partial-failure shape of a mixed bulk call (some entries successful, some refused) never occurs, and the ValidationException branch of CancelJobsErrorDetail is dead.
Fix: per-entry input validation in the CancelJobs handler of the batch service crate (the loop over `input.jobs` that unconditionally pushes each id onto `successful` and returns an empty `errors` vector): validate each entry's length (1..=128) and charset ([a-zA-Z_0-9-]) BEFORE addressing a job, and emit a CancelJobsErrorDetail with code 'ValidationException' and AWS's message for each failing entry while leaving the request's well-formed entries (including unknown-but-well-formed ids) in `successful`. The check belongs in one shared helper used by both bulk handlers so the two operations cannot diverge.
Traces: batch.cancel-jobs-refuses-malformed-entries-beside-accepted-job, batch.cancel-jobs-reports-malformed-id-in-errors-beside-cancelled-job.

=== Cause 2 of 2: TerminateJobs lists a malformed job identifier as successful and emits no errors entry ===
AWS: Smithy com.amazonaws.batch#TerminateJobs binds POST /v1/terminatejobs, success 200, input {jobs (required), reason (required)}, output {successful (required), errors: TerminateJobsErrorDetailList (required)}; modeled errors only ClientException (400) / ServerException (500). TerminateJobsErrorDetail requires {job, code, message} and documents the same 'ValidationException - A job identifier in the request is malformed or isn't valid.' code; the operation reports per-job results and treats jobs that can't be found as successfully processed. Recorded behavior only for the identifier rule: HTTP 200 with one errors entry {job: <the identifier as supplied>, code: 'ValidationException', message: 'Provided name <that identifier> is not supported pattern[a-zA-Z_0-9-]{1,128}'} per malformed identifier (same fixed text, same single variable part), that identifier absent from `successful`, and the real job plus any unknown well-formed id still in `successful` and actually ended.
Emulator: HTTP 200 with `errors` == [] and the malformed identifier echoed into `successful`. With jobs = [a dotted id (characters outside the class), a real RUNNABLE job id, an unknown well-formed id], the emulator answered successful = all three ids (the dotted id included) and errors = [], where AWS answers successful = [the real id, the unknown well-formed id] and errors = [the one ValidationException entry keyed by the dotted id].
Why: the bulk terminate path reports success for an identifier AWS refuses, so a client believes a RUNNING job was terminated when nothing was addressed; a partially-bad bulk terminate is indistinguishable from a fully accepted one, and the ValidationException branch of TerminateJobsErrorDetail is unreachable.
Fix: the same per-entry validation seam in the TerminateJobs handler (its loop over `input.jobs` likewise hard-codes an empty `errors` vector), sharing the single identifier-shape helper with CancelJobs.
Traces: batch.terminate-jobs-refuses-malformed-entry-beside-accepted-jobs.

## Evidence
From the simparity-behavioral-aws-v1 replay against the emulator built from this checkout: 271/299 passed (90.6%), 26 mismatched, 2 errored, bootstrap_signal false; service `batch` 241/254 (95%), 13 failed. The three failures below were the only `### batch` failures marked neither ALREADY FILED nor OWNED.

Cause 1 — batch.cancel-jobs-refuses-malformed-entries-beside-accepted-job
  Request: CancelJobs jobs=['<id with spaces>', '<real RUNNABLE job id>', '<129-character id>'], reason=<probe reason>.
  Expected (AWS): 200 with successful=['<real job id>'] and errors=[{job:'<id with spaces>',code:'ValidationException',message:'Provided name <id with spaces> is not supported pattern[a-zA-Z_0-9-]{1,128}'},{job:'<129-char id>',code:'ValidationException',message:'Provided name <129-char id> is not supported pattern[a-zA-Z_0-9-]{1,128}'}].
  Actual: AssertionError: only the real job may be accepted; successful=['sim2real cancel probe', '7ac7a43e-be69-4266-abd0-f5b23db48ebc', 'sim2real-zzzz…(129 chars)'] — i.e. 200 with all three ids in successful and errors=[].

Cause 1 — batch.cancel-jobs-reports-malformed-id-in-errors-beside-cancelled-job
  Request: CancelJobs jobs=['<real job id>', '<unknown well-formed UUID>', '<malformed id>'], reason='bulk cancel of queued work'.
  Expected (AWS): 200 with successful = the real job id + the unknown well-formed id (2 ids) and exactly one errors entry, keyed by the malformed id, code 'ValidationException', message per the template above.
  Actual: AssertionError: successful: expected the queued job and the unknown well-formed id (2 ids), got 3 id(s); malformed id listed: True; errors [].

Cause 2 — batch.terminate-jobs-refuses-malformed-entry-beside-accepted-jobs
  Request: TerminateJobs jobs=['<dotted id>', '<real RUNNABLE job id>', '<unknown well-formed id>'], reason=<probe reason>.
  Expected (AWS): 200 with successful = [the real job id, the unknown well-formed id] and errors=[{job:'<dotted id>',code:'ValidationException',message:'Provided name <dotted id> is not supported pattern[a-zA-Z_0-9-]{1,128}'}].
  Actual: AssertionError: the real job and the unknown well-formed id must both be accepted; successful=['sim2real.terminate.probe', '00faa413-9f03-43b3-85cb-f3ac15337072', 'sim2real-never-submitted-842c349ffcc9'] — 200 with the dotted id in successful and errors=[].

The identifier strings and job UUIDs above are this run's probe/generated values; the requirement is the verdict (malformed entries absent from successful, one ValidationException errors entry per malformed entry) and the message template 'Provided name ' + the identifier echoed from the request + ' is not supported pattern[a-zA-Z_0-9-]{1,128}'.

Model facts used: smithy_query(model_path='services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json', query='operation', operation='CancelJobs,TerminateJobs') — POST /v1/canceljobs and POST /v1/terminatejobs, success code 200, errors ClientException(400)/ServerException(500), outputs require both `successful` and `errors` (CancelJobsErrorDetailList / TerminateJobsErrorDetailList).

## Steps to Reproduce
Preconditions (both causes): an IAM service role Batch can assume, an UNMANAGED compute environment polled to VALID, an ENABLED job queue over it polled to VALID, a container job definition, and one or two jobs submitted and polled to RUNNABLE (the environment registers no capacity, so they park there).

Cause 1 (CancelJobs):
1. aws batch cancel-jobs --reason probe --jobs 'sim2real cancel probe' <real RUNNABLE job id> 'sim2real-<120 z characters>'  (jobs = [id with a space, the real job, a 129-character id]).
   Emulator: 200 with successful = all three ids and errors = []. AWS: 200 with successful = [the real job id] and one ValidationException errors entry per malformed id, message 'Provided name <that id> is not supported pattern[a-zA-Z_0-9-]{1,128}'.
2. aws batch cancel-jobs --reason 'bulk cancel of queued work' --jobs <real job id> <an unknown well-formed UUID> '<an id containing characters outside [a-zA-Z_0-9-]>'.
   Emulator: 200 with three ids in successful, errors = []. AWS: 200 with the real id and the unknown well-formed id in successful (unknown ids are treated as successfully processed) and exactly one ValidationException errors entry for the malformed id.
3. Re-replay behavioral checks batch.cancel-jobs-refuses-malformed-entries-beside-accepted-job and batch.cancel-jobs-reports-malformed-id-in-errors-beside-cancelled-job.

Cause 2 (TerminateJobs):
4. aws batch terminate-jobs --reason probe --jobs 'sim2real.terminate.probe' <real RUNNABLE job id> <an unknown well-formed id>.
   Emulator: 200 with all three ids in successful and errors = []. AWS: 200 with successful = [the real id, the unknown well-formed id] and errors = [{job: 'sim2real.terminate.probe', code: 'ValidationException', message: 'Provided name sim2real.terminate.probe is not supported pattern[a-zA-Z_0-9-]{1,128}'}].
5. Confirm a call naming only well-formed ids still answers successful = those ids with errors = [].
6. Re-replay behavioral check batch.terminate-jobs-refuses-malformed-entry-beside-accepted-jobs.
