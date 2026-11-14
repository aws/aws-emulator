Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next
Issue-Key: codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match|run#15|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded|run#13|AssertionError|assert build["buildStatus"] == "FAILED", (
Issue-Key: codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk|run#20|AssertionError|assert statuses == {"SUCCEEDED"}, (
Issue-Key: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes|run#20|AssertionError|assert first["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next|run#13|AssertionError|assert first["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CodeBuild/S3: builds FAULT at PROVISIONING, so no declared artifact, finally-block output or S3 cache object is ever written to the bucket

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
ONE root cause, grounded in all five failing checks of the `codebuild__s3` suite.

Cause 1 of 1: the build never executes, so every S3-coupled build effect is unobservable.
AWS: Smithy StartBuild returns a Build; StatusType enumerates SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED and BuildPhaseType enumerates SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED. A build with artifacts {type: S3, location: <bucket>} runs its buildspec, reaches buildStatus SUCCEEDED when every command exits 0, and uploads the declared artifact files to the bucket (the UPLOAD_ARTIFACTS phase) so the stored bytes match what the build produced. A buildspec command exiting non-zero yields buildStatus FAILED while a `finally` block still runs and its output is still uploaded. Concurrent builds of one project each upload their own object with no cross-talk. RetryBuild re-runs the pinned sourceVersion even after the S3 source object changed. A project with cache {type: S3, location: <bucket/prefix>} restores the cached path into the next build.
Emulator: every build walks SUBMITTED(SUCCEEDED) -> QUEUED(SUCCEEDED) -> PROVISIONING(FAULT) -> COMPLETED with buildStatus FAULT, all phase durations 0, and the PROVISIONING phase carrying the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. FAILED is unreachable, no object is ever written to the artifact bucket, no cache object is written or restored, and the pinned-source-version retry has no bytes to compare.
Why: FAULT is the status AWS reserves for an infrastructure fault, so a client sees every build as a service failure; CI logic keyed on SUCCEEDED/FAILED, on artifacts appearing in S3, on per-build artifact isolation, on source-version pinning across RetryBuild, or on cache reuse between builds, all break, and no S3-side effect of a build can be exercised against this emulator at all.
Fix: the build state machine behind the StartBuild handler and the artifact/cache transfer it drives (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the build/phase state read back by batch_get_builds.rs, and the artifact-upload and S3-cache paths that must write into the S3 service state). A deterministic simulated phase walk driven by the project's buildspec, writing the declared artifact objects, is what the suite grades — not a real container.
Traces: codebuild__s3.codebuild-build-uploads-its-declared-artifact-and-the-bytes-match, codebuild__s3.codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded, codebuild__s3.codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk, codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes, codebuild__s3.codebuild-s3-cache-carries-a-file-from-one-build-into-the-next. Open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md describes the same FAULT-at-PROVISIONING defect for the single-service `codebuild` suite and carries no replay key for any of these five cross-service checks, so they must be replayed before the fix is confirmed.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1: codebuild__s3 scored 0/5 (0%), all five mismatches with the same shape.

- codebuild-build-uploads-its-declared-artifact-and-the-bytes-match: "build did not succeed: 'FAULT' phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)] contexts=[None, None, [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}], None]".
- codebuild-buildspec-finally-block-runs-after-a-failing-command-and-its-output-is-uploaded: "expected FAILED after a command exiting 7, got 'FAULT'; phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]".
- codebuild-four-concurrent-builds-write-four-distinct-artifacts-with-no-cross-talk: "concurrent build statuses were ['FAULT']" with all four build ids showing the identical PROVISIONING(FAULT) phase list.
- codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes: "the pinned build must succeed for its bytes to be evidence; got 'FAULT'" with the same PROVISIONING context message.
- codebuild-s3-cache-carries-a-file-from-one-build-into-the-next: "the first build was 'FAULT'" with the same phase list.

Expected in every case: buildStatus SUCCEEDED (or FAILED for the exit-7 leg) with the DOWNLOAD_SOURCE/INSTALL/PRE_BUILD/BUILD/POST_BUILD/UPLOAD_ARTIFACTS/FINALIZING phases reported and the declared artifact/cache objects present in the S3 bucket. Actual: buildStatus FAULT, only SUBMITTED/QUEUED/PROVISIONING/COMPLETED, and an empty bucket.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. CreateBucket in the emulated S3, and CreateProject with source {type: NO_SOURCE, buildspec: <inline spec that writes a file and declares it under `artifacts`>}, artifacts {type: 'S3', location: <bucket>, name: <key>, packaging: 'NONE'}, and a serviceRole.
3. StartBuild, then poll BatchGetBuilds until buildComplete — observe buildStatus 'FAULT' with phases [SUBMITTED(SUCCEEDED), QUEUED(SUCCEEDED), PROVISIONING(FAULT), COMPLETED] and the context message 'this emulator does not run CodeBuild build containers, ...'.
4. GetObject/ListObjectsV2 on the artifact bucket — no object was written.
5. Repeat with a buildspec whose build command exits 7 and a `finally` block that writes output: expect buildStatus FAILED with the finally output uploaded; observe FAULT and nothing uploaded.
6. Repeat with four concurrent StartBuild calls on one project (expect four distinct artifact keys), with RetryBuild on a build pinned to an S3 sourceVersion after replacing the source object (expect the pinned bytes), and with a project configured cache {type: 'S3'} across two builds (expect the cached file in the second build).
7. Or simply replay the behavioral suite (run_behavioral_suite) and read the codebuild__s3 section: 0/5.
