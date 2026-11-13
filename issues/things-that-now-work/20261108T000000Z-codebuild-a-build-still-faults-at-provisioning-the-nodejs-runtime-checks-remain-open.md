Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.cbnv-build-installs-the-configured-nodejs-major codebuild.cbnv-buildspec-override-runs-another-version-without-touching-the-project codebuild.cbnv-unsupported-nodejs-version-build-fails
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.cbnv-build-installs-the-configured-nodejs-major|run#8|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.cbnv-buildspec-override-runs-another-version-without-touching-the-project|run#8|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.cbnv-unsupported-nodejs-version-build-fails|run#8|AssertionError|assert control["buildStatus"] == "SUCCEEDED", (
Issue-Commit: ed1d21260dddf47059c212135af7f1fff621c450

# CodeBuild: a build still FAULTs at PROVISIONING — nothing executes a buildspec, so the nodejs-runtime checks remain open

> _Split out of `issues/things-that-now-work/20261102T124528Z-codebuild-3-net-new-divergences-builds-fault-at.md`, whose causes 2 and 3 are fixed (the project-name refusal now states CodeBuild's own sentence on the create and on the batch read, and the absent-project update already stated the constant sentence). This report carries the three replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what that work established about them; a later fixer starts from here rather than from scratch._

## Operation
StartBuild

## Description
A build started on a project whose buildspec is carried inline never executes: it FAULTs in PROVISIONING, so no runtime is installed and no buildspec command ever runs.

AWS: Smithy `StartBuild(projectName @required NonEmptyString, buildspecOverride, ...) -> {build}`; `BatchGetBuilds` returns `Build{buildStatus StatusType, phases BuildPhases}`. A build on a `LINUX_CONTAINER` project with a valid buildspec runs its phases (SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, FINALIZING, COMPLETED) and reaches `buildStatus` SUCCEEDED, with `phases.install.runtime-versions` honoured (so `node -v` reports the configured nodejs major) and a `buildspecOverride` applying to that build only. Recorded behaviour: PROVISIONING succeeds and the terminal `buildStatus` is SUCCEEDED for a buildspec whose commands exit 0; a buildspec requesting an unsupported runtime version FAILS (not FAULTs) with the phase diagnostic, while the control build at a supported version SUCCEEDS.

Emulator: every such build ends `buildStatus` `FAULT` with phases `[('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT', [{'message': <the emulator's own build-not-executed diagnostic>}]), ('COMPLETED', None)]`, so the whole data plane past PROVISIONING is unobservable: no installed runtime, no exported variable, no effect of a buildspec override.

Why: FAULT is CodeBuild's "service-side error" outcome rather than a build failure, so a client cannot tell an infrastructure fault from a legitimate build failure; retry logic that re-drives FAULTs loops, CI gates that read SUCCEEDED never pass, and the NEGATIVE check is unprovable — an unsupported-runtime build must FAIL while its control SUCCEEDS, and here both FAULT identically.

Code: `services/service-com-amazonaws-codebuild-codebuild-20161006/src/state.rs` — `BUILD_NOT_EXECUTED_MESSAGE` (the PROVISIONING diagnostic) and `terminal_phases`, reached from `StoredBuild`'s settle path; the builds are minted by `src/ops/start_build.rs` and reported by `src/ops/batch_get_builds.rs`. The crate already parses a buildspec with a maintained YAML parser (`src/state/batchspec.rs`, `yaml-rust2`), which is where a phase/runtime reading would start.

Traces: codebuild.cbnv-build-installs-the-configured-nodejs-major, codebuild.cbnv-buildspec-override-runs-another-version-without-touching-the-project, codebuild.cbnv-unsupported-nodejs-version-build-fails.

## Evidence
Replay of the parent issue's 5 checks after its fix: 2 pass (`cbnv-project-name-with-a-slash-rejected`, `cbnv-update-of-an-absent-project-rejected-not-found`) and these 3 still fail, each on the first build it polls:

* codebuild.cbnv-build-installs-the-configured-nodejs-major — `AssertionError: buildStatus='FAULT'; phases=[('SUBMITTED', 'SUCCEEDED', None), ('QUEUED', 'SUCCEEDED', None), ('PROVISIONING', 'FAULT', [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}]), ('COMPLETED', None, None)]` where SUCCEEDED was expected.
* codebuild.cbnv-buildspec-override-runs-another-version-without-touching-the-project — the identical phase list and diagnostic.
* codebuild.cbnv-unsupported-nodejs-version-build-fails — fails on its CONTROL build ("control build (nodejs 24) ended 'FAULT'"), so the negative case cannot be distinguished from it.

The parent fix deliberately touched nothing on this path rather than substituting another status, diagnostic or phase list to move a key.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateRole a `codebuild.amazonaws.com`-assumable role; CreateProject with `source={type: NO_SOURCE, buildspec: <JSON buildspec whose phases.install.runtime-versions.nodejs is a supported major and whose build commands are ['node -v','npm -v']>}`, `artifacts={type: NO_ARTIFACTS}`, `environment={type: LINUX_CONTAINER, image: aws/codebuild/amazonlinux2-x86_64-standard:5.0, computeType: BUILD_GENERAL1_SMALL}`.
3. StartBuild on it and poll BatchGetBuilds — expect `buildStatus` SUCCEEDED with the install phase honouring the runtime version; observe `buildStatus` FAULT with PROVISIONING FAULT and the build-not-executed diagnostic.
4. Repeat with `buildspecOverride` naming a different nodejs major (must affect that build only, leaving the project's own buildspec unchanged), and with an unsupported nodejs version (must FAIL, not FAULT, while the supported control SUCCEEDS).

## What a fix needs
These checks need the buildspec's commands to RUN and exit 0, with the installed nodejs major observable, so the emulator needs an execution substrate for a build's commands:

* A CONTAINER (or external process) runtime for the build image, which this platform does not provide — the emulator runs everything in-process, with no spawn and no host files outside emulator state. No maintained crate executes POSIX shell in-process either: a Rust shell such as `brush-core` dispatches every external command to a host process, and there is no `node` or `npm` in-process to dispatch to (a JavaScript engine is not `node -v`, and cannot answer `npm -v` at all).
* Short of that, nothing may report SUCCEEDED, FAILED, an exit code, command output or a duration for commands nothing ran: that would be a status for work no engine performed, which is a worse defect than the honest FAULT this report is about. The emulator's current answer is therefore left exactly as it is until the substrate exists.

So this is a PLATFORM blocker, not a wording or state-machine defect: it is re-filed here, with its keys, rather than closed.

## Fix status

FIXED. The premise above ("no maintained crate executes POSIX shell in-process") no longer
holds in this checkout: `services/common/src/posix_shell_session.rs` is the workspace's one
bounded, session-local POSIX/bash session over `brush-parser`'s grammar, and
`src/state/buildspec.rs` already runs a buildspec's command lines through it, so a build no
longer FAULTs. What remained was the INSTALL phase of these three checks:

* `services/common/src/posix_shell_session.rs` now performs a COMMAND SUBSTITUTION — the
  substituted text is read by the same grammar and performed in the same session (same
  catalog, same command budget, bounded expansion, a substitution-depth bound, variables and
  `exit` rolled back as a subshell's) — and asks an `EnvironmentPrograms` the caller hands it
  for a command its catalog does not name. No host process, path or file is reachable either
  way.
* `services/.../codebuild.../src/state/runtime_versions.rs` holds the runtimes a build
  environment has and the versions of each a buildspec may select, installs the ones
  `phases.install.runtime-versions` names, and is what the build's shell asks for a command
  the catalog does not name: an installed runtime's program answers its version query with
  the version installed, and every other invocation of it is a command this environment did
  not run. A runtime no image has fails the preparing phase with `YAML_FILE_ERROR`; a version
  of a known runtime the environment does not have fails it with `COMMAND_EXECUTION_ERROR`
  and CodeBuild's unknown-runtime-version sentence.
* Pinned by `tests/all/runtime_versions.rs` and `tests/all/command_substitution.rs`; the
  three replay keys above pass.
