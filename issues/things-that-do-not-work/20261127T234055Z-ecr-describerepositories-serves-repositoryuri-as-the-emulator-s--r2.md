Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.ecracc-describe-unknown-repository-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.ecracc-describe-unknown-repository-rejected|run#9|AssertionError|assert host.startswith(f"{found[0]['registryId']}.dkr.ecr.{ctx.region}."), (
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ecr: DescribeRepositories serves repositoryUri as the emulator's bare host:port, so the authority carries no <registryId>.dkr.ecr.<region> registry labels

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Cause 1 of 1: the `repositoryUri` member of the ECR `Repository` shape is serialized as the emulator's own listening socket address followed by the repository name, instead of an ECR registry DNS authority. The divergence is reached before the check's negative-class assertions (duplicate-name non-deduplication, the RepositoryNotFoundException message), so those are not yet gradeable. Failing check: ecr.ecracc-describe-unknown-repository-rejected.

AWS: Smithy (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921) `Repository$repositoryUri` — "The URI for the repository. You can use this URI for container image `push` and `pull` operations." AWS composes that value as `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>`. The FIXED part of the template is the label ordering with the literal labels `dkr` and `ecr` in the positions `<registryId>.dkr.ecr.<region>.`, no port component, and a path component exactly equal to `Repository$repositoryName`. The VARIABLE parts come from state and the request: `registryId` is the same body's `Repository$registryId` (the 12-digit account owning the registry), `region` is the region of the call, `repositoryName` is the name requested. No status or error-code divergence is involved: the call is answered HTTP 200 and only the body value is wrong. The structural rule — not any one run's account, region, name or suffix — is the requirement; the check accepts any served DNS suffix.

Emulator: DescribeRepositories answers HTTP 200 with `$.repositories[0].repositoryUri` equal to the emulator's bare socket plus the name (recorded run: `127.0.0.1:42257/sim2real-ecracc-repo-12e24b50a05a`). There is no registry-id label, no `dkr` label, no `ecr` label and no region label, and an explicit port is present, so `repositoryUri.split('/',1)[0]` does not start with `<registryId>.dkr.ecr.<region>.`.

Why: a client that uses the field as documented cannot. `docker login`/`push`/`pull` against the value addresses the emulator's control-plane socket rather than a registry, and any consumer that derives an ECS task-definition `image`, a Lambda `ImageUri`, a buildspec push target or an EKS pod image from it — or that parses the authority to recover the registry id and region for cross-account/cross-region checks, resource-policy conditions or IaC assertions that an image belongs to this account — gets a value from which none of that is recoverable. Because the authority is the per-run socket, artifacts derived from the API's own answer are also not reproducible across runs.

Fix: the shared ECR `Repository` serializer / the component that computes `repositoryUri` from state (registry id, region, repository name) must emit a registry DNS authority of the documented shape, and the authority it announces must be one the emulator actually answers on — both halves together (publish the registry-DNS form AND serve that host), not one traded for the other.

Related already-filed issues that do NOT carry this check's replay key: open 20261124T024116Z-ecr-repositoryuri-is-served-as-the-emulator-s.md and 20261127T202610Z-batch-ecr-lambda-logs-ecr-repositoryuri-carries-no-region-nor-registry-authority.md record the same field for other checks; resolved 20261125T075709Z-ecr-describerepositories-serves-repositoryuri-as-the-emulator-s.md and 20261126T143924Z-ecr-repositoryuri-is-published-on-a-synthetic-localhost.md claimed this field, and the bare host:port form is being served again. A resolved issue does not cover this failure.

Traces: ecr.ecracc-describe-unknown-repository-rejected (assertion at check_ecr_ecracc_describe_unknown_repository_rejected.py line 28).

## Evidence
Suite: service `ecr` scored 217/226 (96%), 9 failed; 8 of those are owned by open issues, this one is not.

[mismatch] ecr.ecracc-describe-unknown-repository-rejected
  check_ecr_ecracc_describe_unknown_repository_rejected.py line 28:
    assert host.startswith(f"{found[0]['registryId']}.dkr.ecr.{ctx.region}.")
  AssertionError: the URI host must start with registry id and region; '127.0.0.1:42257/sim2real-ecracc-repo-12e24b50a05a'

Expected (AWS): HTTP 200 and `$.repositories[0].repositoryUri` == `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>` — first authority label is the 12-digit registry id (equal to `registryId` in the same body and to the calling account), then `dkr`, `ecr`, the call's region and at least one suffix label; no port; path == repositoryName.
Actual (emulator): HTTP 200 and `$.repositories[0].repositoryUri` == `<emulator-host>:<emulator-port>/<repositoryName>` — a bare socket address with none of the required labels and with a port.

The host, port, account, region and generated repository name of the recorded run are per-run values filling the template and are not themselves requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Point an AWS client at the emulator's ECR endpoint and call CreateRepository with a valid repositoryName.
3. Call DescribeRepositories with repositoryNames=[that name] and split `$.repositories[0].repositoryUri` on the first '/'.
   -> AWS: the authority is `<registryId>.dkr.ecr.<region>.<dns-suffix>` with no port; its first label equals `registryId` in the same body; the path is the repository name.
   -> Emulator: the authority is the emulator's bare `127.0.0.1:<port>`.
4. Re-replay check ecr.ecracc-describe-unknown-repository-rejected to confirm the fix (and that the check's later duplicate-name and RepositoryNotFoundException-message assertions then pass).
