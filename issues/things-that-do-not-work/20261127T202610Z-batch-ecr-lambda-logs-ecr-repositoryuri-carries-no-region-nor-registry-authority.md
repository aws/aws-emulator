Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected
Issue-Key: batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected|run#18|AssertionError|assert other['repositoryArn'] != first['repositoryArn'] and (
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# ecr: repositoryUri carries no region (nor registry) authority, so same-named repositories in two regions collapse to one identity after a name collision is refused

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Cause 1 of 1: ECR's `repositoryUri` is minted from the emulator's own listening socket plus the repository name, with no registry-id and no region component. Because the per-region registry authority is missing from the value, a repository created under name N in region A and a repository created under the SAME name N in region B serialize to the IDENTICAL `repositoryUri`, so the two region-scoped repositories are indistinguishable by the only field clients use to address an image. Failing check: batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected.

AWS: Smithy (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921) `Repository$repositoryUri` — "The URI for the repository. You can use this URI for container image `push` and `pull` operations." AWS composes it as `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>`. FIXED parts of that template: the label order and the literal labels `dkr` and `ecr` in the positions `<registryId>.dkr.ecr.<region>.`, no port, and a path component equal exactly to `Repository$repositoryName`. VARIABLE parts, taken from state and the request: `registryId` (the 12-digit account owning the registry), the region of the call, and the requested repositoryName. The consequence that this check asserts is structural, not cosmetic: the ECR registry — and therefore the collision namespace — is scoped per account PER REGION, so the same name is free in another region and the two repositories must differ in BOTH `repositoryArn` and `repositoryUri`. The refusal half of this check is already correct: the duplicate create in the home region is refused `RepositoryAlreadyExistsException` with the AWS message "The repository with name '<repositoryName>' already exists in the registry with id '<registryId>'" (fixed text as quoted; `<repositoryName>` from the request and `<registryId>` from the calling account are the variable parts), and the first repository's URI, imageTagMutability and (empty) image list are left unchanged.

Emulator: HTTP 200 on both creates, no error-code or message divergence. The second-region CreateRepository returns a `repositoryUri` of the form `<emulator-host>:<emulator-port>/<repositoryName>` — recorded run `127.0.0.1:36959/sim2real-refarch-batch-repo-78f9f131640f` — which is byte-identical to the `repositoryUri` the home-region repository of the same name already carries (the host:port and the generated name are per-run values; what is required is that the authority carry the registryId and the region so the two values cannot coincide). `repositoryArn` does name the region, so the ARN differs; only the URI collapses, which is what the check's `arn != arn AND uri != uri` identity assertion trips on.

Why: a client that does what the field documents — `docker login`/`push`/`pull`, or deriving an ECS task-definition `image`, a Lambda `ImageUri`, a Batch job-definition image or a buildspec push target from the API's own answer — addresses the emulator's control-plane socket rather than a registry, and in a multi-region estate pushes and pulls for two different repositories resolve to the same target: an image intended for region B is read back for region A. Any consumer that parses the authority to recover the registry id or the region (cross-account/cross-region guards, repository-policy conditions, IaC assertions that an image belongs to this account and region) can recover neither.

Fix: the ECR Repository serializer / the component that computes `repositoryUri` from state (registry id, region, repository name) must emit the documented registry authority, including the region label, and the authority it announces must be one the emulator actually answers on — both halves together, not one traded for the other. The same serializer is shared by CreateRepository and DescribeRepositories.

Already-open issue that does NOT carry this check's replay key and therefore does not cover it: 20261124T024116Z-ecr-repositoryuri-is-served-as-the-emulator-s.md records the bare-host:port form of the same field for check ecr.ecsdeploy-repository-config-round-trips; 20261126T025732Z-codebuild-ecr-ecs-elbv2-logs-createrepository-announces-repositoryuri.md records the reachability half. A serializer fix that adds the registryId and region labels should close this one with them, but this check's cross-region identity assertion is the one to re-replay here.

Traces: batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected (assertion at check_batch__ecr__lambda__logs_refarch_batch_repository_name_collision_rejected.py line 74).

## Evidence
Suite: 273/299 passed (91.3%). Service heading `batch__ecr__lambda__logs` scored 13/15 (87%), 2 failed; one is owned by an open issue, this one is not.

[mismatch] batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected
  check_batch__ecr__lambda__logs_refarch_batch_repository_name_collision_rejected.py line 74:
    assert other['repositoryArn'] != first['repositoryArn'] and (
        other['repositoryUri'] != first['repositoryUri'])
  AssertionError: the us-east-1 repository shares its identity with the us-west-2 one (arn='arn:aws:ecr:us-east-1:000000000000:repository/sim2real-refarch-batch-repo-78f9f131640f', uri='127.0.0.1:36959/sim2real-refarch-batch-repo-78f9f131640f') — two regions must not collapse to the one repository the home-region call was rejected for

Expected (AWS): HTTP 200 and `$.repository.repositoryUri` == `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>`, so the value for region B differs from the value for region A in the region label even when registryId and repositoryName are identical; `$.repository.repositoryArn` likewise names region B.
Actual (emulator): HTTP 200 and `$.repository.repositoryUri` == `<emulator-host>:<emulator-port>/<repositoryName>` for BOTH regions — identical strings, no registryId label, no `dkr`/`ecr` labels, no region label, and a port present.

No status, error code or error message divergence is involved: the earlier refusal assertions in the same check passed (`RepositoryAlreadyExistsException` with AWS's message text), as did the post-refusal invariants (one repository under the name, unchanged repositoryUri, imageTagMutability still IMMUTABLE, no images). The account id, region pair, generated repository name and host port of the recorded run are per-run values filling the template and are not themselves requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Point an AWS client at the emulator's ECR endpoint in region A (e.g. us-west-2) and call CreateRepository with a fresh repositoryName N and imageTagMutability='IMMUTABLE'. Record `$.repository.repositoryUri` and `$.repository.repositoryArn`.
3. Call CreateRepository again in region A with the same N.
   -> Both AWS and the emulator refuse with RepositoryAlreadyExistsException and AWS's message; this half is correct.
4. Point a second client at the emulator's ECR endpoint in a DIFFERENT region B (e.g. us-east-1) and call CreateRepository with the same N.
   -> AWS: 200; the new repository's repositoryUri is `<registryId>.dkr.ecr.<regionB>.<dns-suffix>/N`, which differs from the region-A value, and its ARN names region B.
   -> Emulator: 200, but repositoryUri is byte-identical to the region-A value (`<host>:<port>/N`), so the two region-scoped repositories share the one addressable identity.
5. Re-replay check batch__ecr__lambda__logs.refarch-batch-repository-name-collision-rejected to confirm the fix.
