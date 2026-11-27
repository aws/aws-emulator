Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.command-execution-lifecycle-in-sandbox codebuild.command-execution-lifecycle-reaches-terminal-state codebuild.e2e-sandbox-command-execution-after-stop-rejected codebuild.sandbox-lifecycle-active-to-terminated codebuild.sandbox-lifecycle-transitions
Issue-Key: codebuild.command-execution-lifecycle-in-sandbox|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.command-execution-lifecycle-reaches-terminal-state|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.e2e-sandbox-command-execution-after-stop-rejected|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.sandbox-lifecycle-active-to-terminated|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.sandbox-lifecycle-transitions|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018

# CodeBuild: a sandbox command execution never runs the command, so it never reaches SUCCEEDED or FAILED and carries no exit code or output — the one capability left is a CONTAINED in-process shell

> _Split out of `issues/things-that-now-work/20261025T062553Z-codebuild-3-of-the-6-divergence-causes-remain-open.md`, whose other two causes are fixed (`StartBuildBatch`'s batch now settles into a terminal status the suite can act on, and CloudFormation now provisions `AWS::CodeBuild::Fleet` through CodeBuild's own `CreateFleet`). This report carries the five replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what the two earlier fixes established about them; a later fixer starts from here rather than from scratch._

## RESOLVED

The capability the report below calls blocked EXISTS in the tree, so everything it states
about the emulator (the `FAULT` settle, the "no exit code and no output", the
`mark_wont_fix(platform_blocker)` it recommends) is HISTORY and no longer describes this
workspace. A command submitted to a sandbox whose session is live is now run in that
session and the execution reports what the run answered:

* `services/common/src/posix_shell_session.rs` — the workspace's one bounded, CONTAINED
  in-process shell session. It parses the command with the shell project's grammar half
  only and performs a fixed catalog of session-local utilities over an in-memory tree: it
  starts no process, resolves no command name against the host's `PATH`, opens no socket
  and reads no host file or host environment, failing closed with the status a shell
  answers for a command it cannot run for every command and form it does not perform.
* `services/service-com-amazonaws-codebuild-codebuild-20161006/src/state/sandbox_shell.rs`
  — ONE such session per sandbox, holding that sandbox's environment variables.
* `ComAmazonawsCodebuildCodebuild20161006State::run_due_command_executions` (`src/state.rs`)
  — runs the commands of a sandbox in submit order when the clock brings their turn round,
  and `StoredCommandExecution::record_run` writes the status, `exitCode`,
  `standardOutputContent` and `standardErrContent` the run answered. A command whose
  session had already ended still never ran, and ends where that session did.

All five `Issue-Key` replay units pass. The rule is pinned by
`tests/all/sandbox_command_run_reporting.rs` (what every reader of the execution reports,
the run owed once, and the run reaching no host program or host file), beside
`tests/all/sandbox_command_runs.rs` (what each command form answers) and
`tests/all/command_execution_lifecycle.rs` (the submission itself).

## Operation
StartCommandExecution

## Description
A command execution started in a `RUNNING` sandbox never reaches a terminal status that
reflects a process, because nothing runs the command.

AWS: a command run in a `RUNNING` sandbox reaches a terminal status that reflects the
process it ran — `echo <marker>` leaves the execution `SUCCEEDED` with `exitCode` '0' and
the marker in `standardOutputContent`; `exit 7` leaves it `FAILED` with `exitCode` '7'.

Emulator: `StoredCommandExecution::settle`
(`services/service-com-amazonaws-codebuild-codebuild-20161006/src/state.rs`) ends every
execution in `COMMAND_EXECUTION_STATUS_FAULT` carrying `COMMAND_NOT_EXECUTED_MESSAGE`, with
no exit code and no output, so the five checks spin on `BatchGetCommandExecutions` until
their budget is gone and never reach their own negative assertions.

Why it is BLOCKED, and on what: the capability needed is RUNNING a caller-supplied shell
command line and reporting its real exit code and output. Served faithfully that needs the
build image's shell and filesystem — a runtime this platform does not provide ("Containers
and external processes are not available to a generated emulator yet",
`vendor/engine/src/lib.rs:13-15`). What was checked and ruled out:

* `brush-core` 0.5.0 (`cargo info brush-core`: MIT, rust-version 1.88.0, dependencies
  include `command-fds`, `nix`, `uzers`, `check_elevation`) is the only maintained
  in-process POSIX/bash shell on the registry, and it is a FULL shell: it resolves and
  executes external commands as host processes against the host filesystem. That breaks
  containment (no process spawn and no host filesystem on request data) and is unfaithful
  anyway — the host is not the CodeBuild image — and it exposes no seam that disables
  external command resolution.
* Writing an evaluator for a shell subset instead is the forbidden hand-written language
  subset: a subset sized to `echo` and `exit` answers every other command wrongly while
  reporting success.

So the honest exit for this cause remains `mark_wont_fix(platform_blocker)` naming
`vendor/engine/src/lib.rs:13-15`, UNLESS a contained in-process shell engine appears — a
shell crate with a seam for external command resolution, or a WASI runtime plus a shell
image the tree may ship. A fixer that finds one implements it ONCE, in a shared place under
`services/`, and the execution's own status, exit code and `standardOutputContent` then all
follow from it. Do not re-word the refusal: answering the call with a different non-success
status, or with a 501, is not a fix.

What is already in place around it, so a later fixer does not redo it: `StartSandbox` /
`BatchGetSandboxes` carry the sandbox through its own lifecycle, and
`StartCommandExecution`'s refusal for a sandbox whose session is not live states
CodeBuild's own sentence ("Command execution cannot be started if sandbox status is
STOPPED", with the status read off the stored record) — so the negative half of the sandbox
checks is in place and waiting only for the positive control above.

## Evidence
Replay of the parent issue's 7 units after the fleet fix: 2 passed
(`codebuild.e2e-build-batch-retry-and-cleanup-flow`,
`codebuild.deploy-fleet-list-reflects-fleet`), 5 failed, and the 5 keys above are still
open with `other_causes` empty — the fleet fix caused none of them. Each failure is
`Check timed out after 60.0s`: the execution poll waits on `{'SUCCEEDED','FAILED'}` and the
emulator settles every execution to `FAULT`.

## Steps to Reproduce
1. Boot the emulator and replay the check ids in Issue-Trace-Ids.
2. `CreateProject` on a NO_SOURCE project, `StartSandbox`, poll `BatchGetSandboxes` until
   `RUNNING`, then `StartCommandExecution(sandboxId=..., command='echo marker',
   type='SHELL')` and poll `BatchGetCommandExecutions` — observe `FAULT` with no `exitCode`
   and no `standardOutputContent` instead of `SUCCEEDED` / '0' / the marker.
3. Repeat with `command='exit 7'` (expect `FAILED` / '7'). The `StopSandbox` refusal that
   follows is already served with AWS's own sentence.
