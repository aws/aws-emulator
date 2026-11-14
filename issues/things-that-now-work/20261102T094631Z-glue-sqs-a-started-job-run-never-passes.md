Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive
Issue-Key: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive|run#10|AssertionError|assert observed_running, "never observed the job in RUNNING state"
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# glue__sqs: a started job run never passes through RUNNING — it is minted and immediately FAILED with an emulator-authored ErrorMessage

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: StartJobRun records a run that never enters the RUNNING state and ends FAILED without executing the job, so a client polling GetJobRun for progress observes no running state at all.

AWS: StartJobRun returns a JobRunId whose GetJobRun / GetJobRuns state progresses STARTING -> RUNNING -> SUCCEEDED (or FAILED with the script's own ErrorMessage); a poller watching for RUNNING sees it before the terminal state, and ExecutionTime reflects real work. Smithy (glue StartJobRun / GetJobRun -> JobRun.JobRunState enum includes RUNNING, SUCCEEDED, FAILED; JobRun.ErrorMessage) models the states and the member but not the execution — the progression is recorded behavior only.

Emulator: the run goes from creation straight to JobRunState 'FAILED' with ExecutionTime 0 and ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.', within half a second of StartedOn. The check never observes RUNNING ('never observed the job in RUNNING state').

Why: clients that poll a job run for completion, drive downstream work off the SUCCEEDED transition, or assert on a failure reason get an immediate synthetic failure whose message is the emulator's own prose rather than anything AWS or the script would produce; every job-driven pipeline reads as broken.

Fix: the Glue job-run state machine in StartJobRun/GetJobRun — a started run must occupy RUNNING and reach a terminal state consistent with the job, and a failure's ErrorMessage must be the job's, not the emulator's narration.

Traces: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive. Related open issue that does NOT carry this replay key: 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md.

## Evidence
Expected: GetJobRun reports JobRunState 'RUNNING' at least once, then a terminal state.
Actual: 'AssertionError: never observed the job in RUNNING state'; sibling recordings show the terminal run as {'JobRunState': 'FAILED', 'ExecutionTime': 0, 'ErrorMessage': 'This emulator records job runs but executes no job script, so the job run ended without running.'}.

## Steps to Reproduce
1. CreateJob with a script and a role. 2. StartJobRun. 3. Poll GetJobRun (or `aws glue get-job-run`) for JobRunState and watch for RUNNING before the terminal state. Replay check glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive.
