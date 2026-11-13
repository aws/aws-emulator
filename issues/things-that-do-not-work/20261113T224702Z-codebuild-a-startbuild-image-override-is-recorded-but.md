Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only|run#14|AssertionError|assert _exported(done_overridden) == {"SIM2REAL_IMAGE": _OVERRIDE_IMAGE, "SIM2REAL_OS": "amzn"}, (
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# CodeBuild: a StartBuild image override is recorded but never actually used to run the build — CODEBUILD_BUILD_IMAGE is unset and exported variables come back empty on a SUCCEEDED build

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One net-new root cause (Cause 1 of 1), grounded in failing check codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only.

Cause 1 of 1: StartBuild's imageOverride/computeTypeOverride are echoed back on the Build record, and the build settles SUCCEEDED, but nothing runs the buildspec inside the overridden build image: the agent-provided environment variable CODEBUILD_BUILD_IMAGE is not set in the command environment, and the image's root filesystem (here /etc/os-release) is not present, so every value the buildspec computes from them is empty. BatchGetBuilds then returns the declared exported variables with empty-string values on a build it reports as SUCCEEDED.

AWS: Smithy: StartBuild.imageOverride (NonEmptyString) overrides ProjectEnvironment.image for that build only; ProjectEnvironment.image is documented as "The image tag or image digest that identifies the Docker image to use for this build project" — the build's commands execute inside that image. CodeBuild sets the agent environment variable CODEBUILD_BUILD_IMAGE to the image the build is running in, and the buildspec's `env/exported-variables` names are returned in Build.exportedEnvironmentVariables as ExportedEnvironmentVariable{name,value} where `value` is documented as "The value assigned to the exported environment variable" — i.e. the value the build's own shell assigned during a phase between `install` and the end of `post_build`. A SUCCEEDED build whose buildspec exported a non-empty value must report that non-empty value. Beyond the model, the recorded behavior is: with imageOverride=an Amazon Linux CodeBuild image, the build's `$CODEBUILD_BUILD_IMAGE` equals that override and `. /etc/os-release && echo $ID` yields `amzn`; with the project's Ubuntu-based standard image it yields `ubuntu`. That the override, not the project image, is the one the commands see is the whole contract of a per-build override.

Emulator: BatchGetBuilds reports buildStatus SUCCEEDED and environment.image/computeType/timeoutInMinutes equal to the overrides (those assertions pass), but exportedEnvironmentVariables is [{name: SIM2REAL_IMAGE, value: ""}, {name: SIM2REAL_OS, value: ""}] — the names are carried through from the buildspec while both values are empty strings. The check fails with: "AssertionError: the overridden build's buildspec reported {'SIM2REAL_IMAGE': '', 'SIM2REAL_OS': ''}; it must run in 'aws/codebuild/amazonlinux-x86_64-standard:5.0', an Amazon Linux image". So the override is stored as metadata only; it does not select an execution environment, and the exported-variable values are synthesized as empty rather than read back from the commands that ran.

Why: a client uses imageOverride precisely to run one build on a different image (a different distro, toolchain or runtime) without mutating the project, and reads the result through CODEBUILD_BUILD_IMAGE and exported variables — exactly how CodePipeline consumes a build's output variables into later stages. Against the emulator the build reports SUCCEEDED while the override had no effect and every exported value is empty, so a pipeline or script that branches on an exported value silently takes the empty-string branch, and image-matrix builds all appear to pass while none of them ran on the requested image. A green build that did not do the work is worse than a refusal.

Fix: the StartBuild handler's override resolution must feed the build executor, not just the Build record: the per-build effective environment (image, computeType) has to be the one the command runtime is instantiated from, that runtime must export CODEBUILD_BUILD_IMAGE (and expose the chosen image's filesystem) into the command environment, and the exported-variables collector must read the variables' values out of the finished command environment and serialize them into Build.exportedEnvironmentVariables, rather than emitting the declared names with empty values.

Related but distinct open issues, named for the fixer and NOT covering this: 20261113T085141Z-codebuild-a-well-formed-buildspec-build-fails-in.md covers a well-formed buildspec build that FAILS in BUILD and exports nothing (here the build SUCCEEDS and exports empty-valued names), and 20261113T190942Z-codebuild-secretsmanager-...md covers an unresolved SECRETS_MANAGER env reference. Neither is about the per-build image override failing to select the execution environment.

Traces: codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only.

## Evidence
Check: codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only [mismatch]

Setup: CreateProject with source {type: NO_SOURCE, buildspec: exports SIM2REAL_IMAGE=$CODEBUILD_BUILD_IMAGE and SIM2REAL_OS=$(. /etc/os-release && echo $ID)}, environment {type: LINUX_CONTAINER, image: aws/codebuild/standard:7.0, computeType: BUILD_GENERAL1_SMALL}, timeoutInMinutes 60. Then StartBuild with imageOverride=aws/codebuild/amazonlinux-x86_64-standard:5.0, computeTypeOverride=BUILD_GENERAL1_MEDIUM, timeoutInMinutesOverride=5.

Expected (AWS): BatchGetBuilds on the overridden build -> buildStatus SUCCEEDED, environment.image = aws/codebuild/amazonlinux-x86_64-standard:5.0, exportedEnvironmentVariables = [{name: SIM2REAL_IMAGE, value: "aws/codebuild/amazonlinux-x86_64-standard:5.0"}, {name: SIM2REAL_OS, value: "amzn"}] (the values are whatever the override image yields; the fixed requirement is that they are the running image's id and that image's os-release ID, not empty).

Actual (emulator): buildStatus SUCCEEDED and environment echoes the overrides, but exportedEnvironmentVariables = [{name: SIM2REAL_IMAGE, value: ""}, {name: SIM2REAL_OS, value: ""}].

Failure text from the replay:
  File "check_codebuild_start_build_image_and_compute_overrides_apply_to_that_build_only.py", line 179, in run
    assert _exported(done_overridden) == {"SIM2REAL_IMAGE": _OVERRIDE_IMAGE, "SIM2REAL_OS": "amzn"}
  AssertionError: the overridden build's buildspec reported {'SIM2REAL_IMAGE': '', 'SIM2REAL_OS': ''}; it must run in 'aws/codebuild/amazonlinux-x86_64-standard:5.0', an Amazon Linux image

No HTTP status or error code diverges here: every call returns 200 and the divergence is in the response body's exportedEnvironmentVariables values on a build reported SUCCEEDED.

## Steps to Reproduce
1. CreateRole for a CodeBuild service role trusting codebuild.amazonaws.com, with logs:CreateLogGroup/CreateLogStream/PutLogEvents.
2. CreateProject: source {type: NO_SOURCE, buildspec: version 0.2 declaring env.exported-variables [SIM2REAL_IMAGE, SIM2REAL_OS] and a build phase running `export SIM2REAL_IMAGE="$CODEBUILD_BUILD_IMAGE"` then `export SIM2REAL_OS="$(. /etc/os-release && echo $ID)"`}, artifacts {type: NO_ARTIFACTS}, cache {type: NO_CACHE}, environment {type: LINUX_CONTAINER, image: aws/codebuild/standard:7.0, computeType: BUILD_GENERAL1_SMALL}, serviceRole the role above, timeoutInMinutes 60, logsConfig both DISABLED.
3. StartBuild projectName=<name> imageOverride=aws/codebuild/amazonlinux-x86_64-standard:5.0 computeTypeOverride=BUILD_GENERAL1_MEDIUM timeoutInMinutesOverride=5.
4. StartBuild projectName=<name> with no overrides.
5. Poll BatchGetBuilds on each id until buildComplete is true.
6. Read exportedEnvironmentVariables on the overridden build: the emulator returns both names with value "" even though buildStatus is SUCCEEDED; AWS returns the override image id and `amzn`. Reading the plain build shows the same empty-value shape where AWS returns the project image id and `ubuntu`.
7. BatchGetProjects confirms the project itself is unchanged (this part matches AWS).
