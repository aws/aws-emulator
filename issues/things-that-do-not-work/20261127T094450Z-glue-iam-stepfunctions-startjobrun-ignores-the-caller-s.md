Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__stepfunctions.start-job-run-task-refused-above-caller-usage-profile-timeout
Issue-Key: glue__iam__stepfunctions.start-job-run-task-refused-above-caller-usage-profile-timeout|run#38|AssertionError|assert got == want, f"a Timeout one minute above the profile maximum gave {got!r}, expected {want!r}"
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# glue__iam__stepfunctions: StartJobRun ignores the caller's Glue usage profile, so a Timeout above the profile maximum starts a run instead of failing the Step Functions task with Glue.InvalidInputException

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: the usage-profile limits on a job run's Timeout are never enforced on StartJobRun.
AWS: when the calling identity is tagged with a Glue usage profile (`glue:UsageProfile`), Glue holds every StartJobRun to that profile's JobConfiguration limits. With a profile whose timeout configuration is DefaultValue 5 / MinValue 1 / MaxValue 10, a StartJobRun with Timeout 11 is refused with `InvalidInputException` and the message `Provided timeout is not within the range [<min>,<max>] in <profile-name> profile.` — the fixed text is `Provided timeout is not within the range [` ... `] in ` ... ` profile.`, with the bracketed bounds taken from the profile's MinValue/MaxValue and the name taken from the profile the caller is bound to. No job run is created. Reached through a Step Functions `arn:aws:states:::glue:startJobRun` Task, that refusal fails the execution: status FAILED, error `Glue.InvalidInputException`, cause the Glue message followed by the SDK's request-scoped ` (Service: ...)` suffix. A Timeout at the maximum (10) is accepted and the run completes with JobRun.Timeout 10 (that positive control passes today). Smithy: StartJobRun.Timeout has range min 1 only — the per-profile maximum is a service rule, not a model constraint — and InvalidInputException is modeled on StartJobRun. Reference: https://docs.aws.amazon.com/glue/latest/dg/start-usage-profiles-jobs.html
Emulator: the over-limit Timeout is accepted. The Step Functions execution ends `SUCCEEDED` with no error and an empty cause, and the job run is started (the check's later GetJobRuns comparison would see a run AWS never creates).
Why: a client that relies on usage profiles to cap job-run cost/duration gets no enforcement at all: runs that production refuses are admitted, the Glue.InvalidInputException branch of the state machine is never taken, and an orchestration whose error handling is built on that task failure silently takes the success path.
Fix: validation in the Glue StartJobRun handler — resolve the caller's usage profile and reject a Timeout outside its JobConfiguration timeout MinValue/MaxValue with InvalidInputException before any run is created; the Step Functions `glue:startJobRun` task integration then surfaces it as the `Glue.InvalidInputException` task failure it already surfaces for other Glue refusals.
Traces: glue__iam__stepfunctions.start-job-run-task-refused-above-caller-usage-profile-timeout.

## Evidence
`AssertionError: a Timeout one minute above the profile maximum gave ('SUCCEEDED', None, ''), expected ('FAILED', 'Glue.InvalidInputException', 'Provided timeout is not within the range [1,10] in <profile-name> profile.')` — the tuple is (execution status, execution error, cause with the SDK ' (Service: ...)' suffix stripped). The bounds [1,10] are the profile's MinValue/MaxValue and the profile name is the one the execution role is tagged with; the surrounding sentence is AWS's fixed text. The in-range control (Timeout 10) ends SUCCEEDED with JobRun.Timeout 10 in the same check, so the task integration itself works — only the profile enforcement is absent.

## Steps to Reproduce
1. s3:CreateBucket and put a trivial pythonshell script at s3://<bucket>/job.py; iam:CreateRole trusting glue.amazonaws.com with s3:GetObject on that object.
2. glue:CreateUsageProfile(Name=<profile>, Configuration={JobConfiguration:{timeout:{DefaultValue:'5', MinValue:'1', MaxValue:'10'}}}).
3. glue:CreateJob(Name=<job>, Role=<job role>, GlueVersion='3.0', MaxCapacity=0.0625, Timeout=5, Command={Name:'pythonshell', PythonVersion:'3.9', ScriptLocation:'s3://<bucket>/job.py'}).
4. iam:CreateRole trusting states.amazonaws.com, allowed glue:StartJobRun/GetJobRun on the job and tagged glue:UsageProfile=<profile>.
5. states:CreateStateMachine with one Task {Resource:'arn:aws:states:::glue:startJobRun', Parameters:{JobName:<job>, 'Timeout.$':'$.timeout'}}.
6. StartExecution with timeout=10 — SUCCEEDED (control). StartExecution with timeout=11 — the emulator reports SUCCEEDED with no error and starts a run; AWS reports FAILED / Glue.InvalidInputException and starts none.
