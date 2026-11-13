Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes
Issue-Key: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes|run#22|AssertionError|assert _exported(first).get("SIM2REAL_PAYLOAD") == payload_v1, (
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# codebuild__s3: a build pinned to an S3 source object version SUCCEEDS without the source archive — the buildspec reads no file out of it

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
1 net-new root cause in the CodeBuild S3-source download. Related open issues cover neighbouring causes but carry not this replay key: 20261109T181128Z-codebuild-s3-buildspec-commands-run-without-a-shell.md, 20261025T014438Z-codebuild-s3-builds-fault-at-provisioning-so-no.md, 20261029T204615Z-codebuild-s3-builds-fault-at-provisioning-so-declared.md (there the builds FAULT in PROVISIONING; here the build reports SUCCEEDED).

Cause 1 of 1: the S3 source ZIP is not downloaded and unzipped into the build's source directory, yet the build succeeds.
AWS: for an S3 source (source.type=S3, location=<bucket>/<key>), CodeBuild downloads the object — the exact object VERSION when StartBuild carries sourceVersion — unzips it into the primary source root, and the buildspec's commands run with those files present; `export SIM2REAL_PAYLOAD="$(cat payload.txt)"` therefore exports the contents of payload.txt from the archive, and that value is reported in Build.exportedEnvironmentVariables for a name listed under env/exported-variables. Smithy: StartBuild input sourceVersion; Build members sourceVersion, exportedEnvironmentVariables (ExportedEnvironmentVariable{name,value}), buildStatus. The model does not describe the download, so the archive-extraction behavior is recorded AWS behavior.
Emulator: the build reports buildStatus SUCCEEDED and the exported variable IS present, but its value is the empty string — `cat payload.txt` found no such file, so nothing from the ZIP reached the container's working directory. All exported: {'SIM2REAL_PAYLOAD': ''}.
Why: a client gets a green build that silently produced nothing from its own source: every buildspec step that reads a file out of the repository/archive sees an empty tree, and because the build is SUCCEEDED rather than FAILED there is no failure phase or context to diagnose. It also makes source pinning unobservable — a RetryBuild that must reuse the original object version and a fresh unpinned build that must see the overwritten bytes produce the same empty result.
Fix: the S3 source acquisition step of the build runner in the codebuild service (fetch the object — the pinned VersionId when sourceVersion is given — unzip it into the primary source root before the buildspec phases run, and fail the build when that download/extract cannot be done).
Traces: codebuild__s3.codebuild-retry-build-reuses-the-pinned-source-version-after-the-object-changes.

## Evidence
Cause 1: AssertionError: the container must have read version 1's payload 'OFX6Q8GQYG' out of the unzipped source; it exported '' (all exported: {'SIM2REAL_PAYLOAD': ''}) — while first['buildStatus'] was SUCCEEDED and first['sourceVersion'] matched the pinned S3 VersionId.

## Steps to Reproduce
Cause 1: enable versioning on a bucket; upload a ZIP containing payload.txt with a known token and keep its VersionId; CreateProject with source {type: S3, location: <bucket>/<key>} and a buildspec whose build phase runs `export SIM2REAL_PAYLOAD="$(cat payload.txt)"` with SIM2REAL_PAYLOAD under env/exported-variables; StartBuild with sourceVersion=<VersionId>; poll BatchGetBuilds to terminal and read exportedEnvironmentVariables.
