Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.job-parameter-substitution-and-container-override-precedence
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.job-parameter-substitution-and-container-override-precedence|run#11|AssertionError|assert default_job["status"] == "SUCCEEDED", (
Issue-Commit: e8e6e0e7228b80352ff1138147473dcb9e7f9930

# batch: job-definition `Ref::` parameter substitution is never performed, so a job whose command carries a placeholder runs the literal `Ref::<name>` text and ends FAILED instead of SUCCEEDED (1 cause)

> _Note: this issue was identified against the code base at commit `e8e6e0e7228b80352ff1138147473dcb9e7f9930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause in the batch emulator's job data plane, found by replaying the behavioral suite (simparity-behavioral-aws-v1). The container now does execute (the emulator no longer answers the old emulator-authored "does not run containers" reason), but the command it executes is not the substituted one.

Cause 1 of 1: parameter substitution placeholders in a container job definition's command are not resolved before the container is started.
AWS: Batch resolves `Ref::<name>` placeholders in a container job definition against the effective parameter map before the container runs. Smithy: RegisterJobDefinition.parameters — "Default parameter substitution placeholders to set in the job definition. Parameters are specified as a key-value pair mapping. Parameters in a SubmitJob request override any corresponding parameter defaults from the job definition."; SubmitJob.parameters — "Additional parameters passed to the job that replace parameter substitution placeholders that are set in the job definition... Parameters in a SubmitJob request override any corresponding parameter defaults from the job definition."; ContainerProperties.command maps to the Docker `Cmd`/`COMMAND` the container is started with. So for a FARGATE container job definition registered with command ["/bin/sh", "-c", "exit \"$0\"", "Ref::code"] and parameters {"code": "0"}, the command actually executed is ["/bin/sh", "-c", "exit \"$0\"", "0"], the container exits 0, and the job reaches status SUCCEEDED (Smithy: JobStatus = SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED) with statusReason the fixed AWS text `Essential container in task exited`, container.exitCode 0, DescribeJobs' JobDetail.container.command holding the RESOLVED command (no element containing the substring `Ref::`), and JobDetail.parameters echoing the effective map (the job-definition default merged with any SubmitJob override — the submit-time value winning per the model doc above). The requirement is the substitution rule and the resulting status/exit code; the parameter names and values (`code`, `0`, `7`) are this check's request values filling the template, not part of it.
Emulator: the job reaches terminal status FAILED with statusReason `Essential container in task exited` — i.e. the container was started, but with the UNSUBSTITUTED argument, so `/bin/sh -c 'exit "$0"' Ref::code` executes `exit Ref::code`, which is not a numeric argument and exits non-zero. Replay output: `AssertionError: the default parameter code=0 must produce exit 0, got 'FAILED' reason='Essential container in task exited'`. The placeholder is carried through to the container verbatim instead of being replaced by the job definition's default parameter value (and, by the same code path, instead of being replaced by a SubmitJob-supplied override).
Why: every Batch workload that parameterises its command — the documented and overwhelmingly common way to run one job definition over many inputs (array shards, per-tenant arguments, per-run file keys) — is executed with the literal `Ref::<name>` text in argv. Depending on the image's entrypoint this either fails the job (as here: a healthy workload is reported FAILED, so SDK waiters, CI pipelines, Step Functions `.sync` integrations and CloudFormation-driven runs all conclude the job errored) or, worse, silently runs the WRONG command with placeholder text where a path, key or numeric argument belonged. Clients that read DescribeJobs' JobDetail.container.command or JobDetail.parameters to learn what a job actually ran are told either nothing or the unresolved form, so the divergence is also unobservable from the API.
Fix: the SubmitJob / job-dispatch path in the batch crate that materialises a job's effective container definition — it must (a) merge the job definition's `parameters` defaults with the SubmitJob `parameters` map, SubmitJob winning on a key collision, (b) rewrite every `Ref::<name>` occurrence in the effective container command (and the other substitutable container members) with the merged value before the container is started, and (c) project the resolved command and the effective parameter map into the DescribeJobs JobDetail. containerOverrides.command must continue to replace the definition command outright, placeholders and all.
Traces: behavioral check batch.job-parameter-substitution-and-container-override-precedence (mismatch).

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1): 270/295 passed; 23 mismatched, 2 errored. Service `batch` 241/251. Every other batch-family failure in this run is marked OWNED by an already-filed issue; this is the only surviving check.

[mismatch] batch.job-parameter-substitution-and-container-override-precedence
  File "check_batch_job_parameter_substitution_and_container_override_precedence.py", line 491, in run
    assert default_job["status"] == "SUCCEEDED", (
  AssertionError: the default parameter code=0 must produce exit 0, got 'FAILED' reason='Essential container in task exited'

Setup the check performed before that assertion (all of it succeeded):
  - MANAGED/FARGATE compute environment + ENABLED job queue, both polled to VALID (maxvCpus 8)
  - RegisterJobDefinition: type=container, platformCapabilities=[FARGATE],
    containerProperties.command = ["/bin/sh", "-c", 'exit "$0"', "Ref::code"], parameters = {"code": "0"}
  - SubmitJob with no overrides -> polled DescribeJobs to a terminal status

Expected (AWS): status SUCCEEDED, statusReason 'Essential container in task exited', container.exitCode 0,
container.command containing no element with the substring 'Ref::' and ending in the substituted value,
and for the sibling submissions: parameters={"code":"7"} -> FAILED with container.exitCode 7 and
DescribeJobs echoing parameters {"code":"7"}; containerOverrides.command=["/bin/sh","-c","exit 0"] -> SUCCEEDED
with container.command equal to the override.
Actual (emulator): the first job is FAILED. The reason text itself matches AWS's fixed text, so the container
was started — the divergence is the argv it was started with: the unsubstituted literal `Ref::code`.

Model citations: Smithy RegisterJobDefinitionRequest.parameters, SubmitJobRequest.parameters (substitution/override rule quoted in the description), ContainerProperties.command (maps to the Docker Cmd/COMMAND the container runs), JobStatus enum, JobDetail.{status, statusReason, parameters, container}, ContainerDetail.{command, exitCode}.

Per-run values (job ids, ARNs, task ids, log stream names, timestamps, request ids) are not requirements. Note: open issue 20261109T161717Z-batch-3-net-new-causes-tag-untag-204 (cause 3) covers a Fargate job never reaching SUCCEEDED with the emulator-authored "does not run containers" reason — a different cause, and the emulator no longer emits that reason here; it now runs the container, with the wrong command. No open or fixed issue carries parameter substitution.

## Steps to Reproduce
1. Create a MANAGED/FARGATE compute environment and an ENABLED job queue over it; wait for both to report status VALID.
2. RegisterJobDefinition with type=container, platformCapabilities=["FARGATE"], containerProperties.command = ["/bin/sh", "-c", "exit \"$0\"", "Ref::code"] and parameters = {"code": "0"} (image/vcpu/memory are immaterial; any shell-capable image works).
3. SubmitJob against that queue and job definition with NO parameters and NO containerOverrides.
4. Poll DescribeJobs on the returned jobId until the status is SUCCEEDED or FAILED.
   AWS: SUCCEEDED, statusReason 'Essential container in task exited', container.exitCode 0, and container.command carrying the substituted last element "0" (no element contains "Ref::").
   Emulator: FAILED — the container ran `exit Ref::code` because the placeholder was never substituted.
5. Same job definition, SubmitJob with parameters={"code": "7"}: AWS reaches FAILED with container.exitCode 7 and DescribeJobs echoes parameters {"code": "7"} (the submit-time value overriding the definition default).
6. Same job definition, SubmitJob with containerOverrides={"command": ["/bin/sh", "-c", "exit 0"]}: AWS reaches SUCCEEDED and DescribeJobs reports container.command exactly equal to the override.
Replay shortcut: behavioral check batch.job-parameter-substitution-and-container-override-precedence.
