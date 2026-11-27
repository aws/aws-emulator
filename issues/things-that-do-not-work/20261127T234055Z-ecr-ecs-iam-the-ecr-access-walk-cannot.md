Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle
Issue-Key: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# ecr__ecs__iam: the ECR access walk cannot resolve a repository — DescribeRepositories' repositoryUri is the emulator's bare host:port, addressing no registry in this account and region

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Cause 1 of 1: the final step of the frozen six-operation ECR-access walk reads `repositoryUri` off DescribeRepositories and must recover from it that the repository lives in this account and region; the emulator serves the field as its own bare listening socket plus the repository name, so the walk's `solve` phase cannot resolve the repository identity at all. Failing check: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle.

AWS: Smithy (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921) `Repository$repositoryUri` — "The URI for the repository. You can use this URI for container image `push` and `pull` operations." AWS composes it as `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>`. FIXED in that template: the label ordering with literal `dkr` and `ecr` labels in the positions `<registryId>.dkr.ecr.<region>.`, no port, and a path component equal exactly to `Repository$repositoryName`. VARIABLE from state and the request: `registryId` (the 12-digit account owning the registry, equal to the same body's `registryId`), the region of the call, and the requested repository name. The call is answered HTTP 200 in both cases — there is no status or error-code divergence, only a wrong body value.

Emulator: `aws ecr describe-repositories --repository-names <repo> --query repositories[0].repositoryUri` answers HTTP 200 with `127.0.0.1:41181/sim2real-ecracc-e2e-16fbef7bfafe-repository-9e3dc32730ab` — no registry-id label, no `dkr`/`ecr` labels, no region label, and an explicit port — so the walk's assertion that the resolved URI addresses that repository in this account and region fails and phase 2/4 (`solve`) aborts.

Why: the whole point of this read-only diagnostic walk is the one a human or tool performs on real AWS — join an ECS task definition's container `image` to the ECR repository it names, then to the task/execution role's permissions on that repository. With a bare socket authority there is nothing to join on: the registry id and region are unrecoverable, the value does not match any `image` an ECS task definition would legitimately carry, and no `docker login`/`pull` against it reaches a registry.

Fix: the shared ECR `Repository` serializer / the component that computes `repositoryUri` from state (registry id, region, repository name) must emit a registry DNS authority of the documented shape, and must announce an authority the emulator actually answers on — publishing the registry-DNS form and serving that host together, not one traded for the other.

Related already-filed issues that do NOT carry this check's replay key: resolved 20261121T160051Z-ecr-ecs-iam-describerepositories-reports-a-repositoryuri-that--r2.md claimed exactly this field for this walk and the bare host:port form is being served again (a resolved issue does not cover this failure); open 20261124T024116Z-ecr-repositoryuri-is-served-as-the-emulator-s.md records the same serializer under service `ecr`.

Traces: ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle (phase 2/4 `solve`, check_ecr__ecs__iam_e2e_ecracc_ecs_services_ecr_access_check_lifecycle.py line 156).

## Evidence
Suite: service `ecr__ecs__iam` scored 11/12 (92%), 1 failed — this one, unowned.

[mismatch] ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle
  AssertionError: [phase 2/4 solve] the resolved repository URI '127.0.0.1:41181/sim2real-ecracc-e2e-16fbef7bfafe-repository-9e3dc32730ab' must address 'sim2real-ecracc-e2e-16fbef7bfafe-repository-9e3dc32730ab' in this account and region

Expected (AWS): HTTP 200 and `repositories[0].repositoryUri` == `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repositoryName>`, whose authority yields the registry id (== the calling account) and the region, and whose path is the repository name.
Actual (emulator): HTTP 200 and `repositories[0].repositoryUri` == `<emulator-host>:<emulator-port>/<repositoryName>`, from which neither the account nor the region can be recovered.

The recorded host, port, account, region and generated repository name are per-run values filling the template, not requirements. The preceding walk steps (ecs ListServices / DescribeServices / DescribeTaskDefinition, iam ListAttachedRolePolicies / ListRolePolicies) were issued in the frozen order without divergence; the walk aborts on the ECR step.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Deploy the ecracc Fargate-service estate the check deploys (an ECR repository plus an ECS service whose task definition's image names it).
3. Run the frozen read-only walk: ecs ListServices -> DescribeServices -> DescribeTaskDefinition -> iam ListAttachedRolePolicies -> ListRolePolicies -> ecr DescribeRepositories --repository-names <repo>.
4. Inspect `repositories[0].repositoryUri` from step 3's last call.
   -> AWS: `<registryId>.dkr.ecr.<region>.<dns-suffix>/<repo>`; the authority identifies the account and region, and matches the task definition's container image authority.
   -> Emulator: `127.0.0.1:<port>/<repo>`; it addresses no registry and identifies no account or region.
5. Re-replay check ecr__ecs__iam.e2e-ecracc-ecs-services-ecr-access-check-lifecycle to confirm the fix.
