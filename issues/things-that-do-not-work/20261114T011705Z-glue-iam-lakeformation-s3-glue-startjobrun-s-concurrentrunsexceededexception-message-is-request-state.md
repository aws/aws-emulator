Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run
Issue-Key: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run|run#16|AssertionError|assert got == _REFUSED, f"a second start under MaxConcurrentRuns 1 returned {got!r}, expected {_REFUSED!r}"
Issue-Commit: 7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69

# Glue StartJobRun's ConcurrentRunsExceededException message is request/state-derived ("Job <name> already has 1 of 1 concurrent runs in flight") instead of AWS's fixed "Concurrent runs exceeded."

> _Note: this issue was identified against the code base at commit `7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One net-new root cause in the Glue job-run admission path.

Cause 1 of 1: the MaxConcurrentRuns refusal now fires with the right code but an emulator-authored message.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) — StartJobRun models ConcurrentRunsExceededException (errorKind client, HTTP 400, doc "Too many jobs are being run concurrently."), and ExecutionProperty.MaxConcurrentRuns is documented "The maximum number of concurrent runs allowed for the job. The default is 1. An error is returned when this threshold is reached." Recorded behavior: when a StartJobRun would take the job past its effective MaxConcurrentRuns (the default 1 when ExecutionProperty is absent), counted over that job's runs that are not in a terminal state (SUCCEEDED/FAILED/STOPPED/TIMEOUT/ERROR), the call is refused HTTP 400 with Error.Code "ConcurrentRunsExceededException" and Error.Message exactly

    Concurrent runs exceeded.

That message is ENTIRELY fixed text for this error class. It contains no variable part at all: no job name, no current run count and no configured cap. The only thing the request and the job's state govern is WHEN the refusal fires, not what it says.

Emulator: the admission check now fires and the error code is correct, but the message is composed from the request and the emulator's state:

    Job sim2real-gluecc-f5b5871d67fa already has 1 of 1 concurrent runs in flight

(the job name and the two counts in that recording are filler for the emulator's template, not part of any requirement — the point is that AWS's sentence interpolates nothing).

Why: Glue clients, the AWS SDK samples, Glue triggers/workflows and orchestration retry loops branch on this refusal, and tooling that logs or asserts on the message text (and any consumer that pattern-matches the sentence to decide "back off and retry" rather than "this is a different 400") sees a sentence AWS never emits. Because the emulator's text embeds the job name and a run count, it is also unstable across runs, so a client or test that pins the documented wording cannot match it at all.

Fix: the StartJobRun handler's concurrency-admission step in the glue service — when the non-terminal run count for the named job is already at the job's effective ExecutionProperty.MaxConcurrentRuns, return ConcurrentRunsExceededException with the constant message "Concurrent runs exceeded." and interpolate nothing from the request or the stored job.

Relation to what is already filed: open issue 20261109T072702Z-glue-iam-lakeformation-s3-glue-job-run-surface-2-net-new-causes.md describes this same check failing because StartJobRun did NOT enforce MaxConcurrentRuns at all (its recorded failure was "the call succeeded where AWS refuses it"). The refusal now happens, so that cause's symptom is gone and the replay no longer matches that issue's key; what remains is the message wording, which that issue states as the requirement but which is not yet met. This is filed as the residual cause, not as a restatement of the un-enforced cap.

Traces: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run

## Evidence
Check glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run [mismatch].

Setup that passed: CreateJob (pythonshell, MaxCapacity 0.0625, DefaultArguments {"--hold_s": "180"}, no ExecutionProperty) -> GetJob reports ExecutionProperty == {"MaxConcurrentRuns": 1}; a warm-up run reached SUCCEEDED; one holding run was started and is non-terminal.

Request: glue:StartJobRun(JobName=<job>) while that one run is active and the cap is 1.

Expected (AWS): HTTP 400, (Error.Code, Error.Message) == ("ConcurrentRunsExceededException", "Concurrent runs exceeded.") — the message is fixed text with no interpolated values.

Actual (emulator): ("ConcurrentRunsExceededException", "Job sim2real-gluecc-f5b5871d67fa already has 1 of 1 concurrent runs in flight") — correct code, message built from the job name plus the in-flight/configured counts.

Replay output:
  File "check_glue__iam__lakeformation__s3_glue_update_job_raising_max_concurrent_runs_to_two_refuses_only_the_third_run.py", line 58, in run
    assert got == _REFUSED, f"a second start under MaxConcurrentRuns 1 returned {got!r}, expected {_REFUSED!r}"
  AssertionError: a second start under MaxConcurrentRuns 1 returned ('ConcurrentRunsExceededException', 'Job sim2real-gluecc-f5b5871d67fa already has 1 of 1 concurrent runs in flight'), expected ('ConcurrentRunsExceededException', 'Concurrent runs exceeded.')

The job name and counts in the observed message are that recording's values filling the emulator's template; the requirement is the constant AWS sentence, whatever the job and cap are.

Smithy: StartJobRun errors include ConcurrentRunsExceededException (client, httpError 400, defaulted); ExecutionProperty.MaxConcurrentRuns "The default is 1. An error is returned when this threshold is reached."

## Steps to Reproduce
1. Create an S3 bucket and upload a pythonshell script that sleeps for a required argument (e.g. --hold_s), so a started run stays non-terminal.
2. iam:CreateRole a role Glue can assume, then glue:CreateJob with Command={Name: pythonshell, PythonVersion: "3.9", ScriptLocation: s3://<bucket>/scripts/ps.py}, MaxCapacity=0.0625, DefaultArguments={"--hold_s": "180"} and NO ExecutionProperty.
3. glue:GetJob -> ExecutionProperty is {"MaxConcurrentRuns": 1}.
4. glue:StartJobRun(JobName=<job>) and wait until that run is non-terminal.
5. glue:StartJobRun(JobName=<job>) again. Read Error.Code and Error.Message. EXPECT ("ConcurrentRunsExceededException", "Concurrent runs exceeded."); OBSERVED the correct code with the message "Job <job name> already has 1 of 1 concurrent runs in flight".
6. glue:UpdateJob with JobUpdate.ExecutionProperty={"MaxConcurrentRuns": 2}; the next start must be admitted, and only the third start is refused — with that same constant message.
