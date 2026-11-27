Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-push-via-layer-upload
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-push-via-layer-upload api-image-scanning-and-findings api-immutable-tag-conflict-and-image-deletion api-lifecycle-policy-preview-and-apply api-registry-policy-and-scanning-configuration api-repository-lifecycle-with-policy-and-tags ecr-duplicate-push-and-tag-based-storage-class-update ecr-image-and-repo-not-found-errors ecr-image-lookup-errors ecr-image-push-and-inspect ecr-immutable-tag-conflict ecr-layer-upload-lifecycle-and-errors ecr-multi-image-isolation-in-shared-repository ecr-push-multi-layer-image ecr-put-image-missing-layers-error ecr-put-image-missing-layers-error-then-recover ecr-repository-create-verify-delete ecr-repository-explicit-mutable-tags-defaults-read-back-tag-change-remove-last-and-not-found ecr-repository-immutable-aes256-scan-off-own-account-policy-and-lifecycle-round-trip ecr-repository-lifecycle-and-errors ecr-repository-pagination-and-scoping-conflict
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|CreateRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr|DeleteRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr|DescribeRepositories|2xx->2xx|$.repositories[*].repositoryUri
Issue-Commit: e1287cf409e72f27d41b5362b1ef9b2f278addc7

# ecr: repositoryUri is published on a synthetic .localhost registry host by every Repository-shaped response

> _Note: this issue was identified against the code base at commit `e1287cf409e72f27d41b5362b1ef9b2f278addc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
4 net-new causes, all one shared fault: every ECR response that carries a Repository (or a list of them) mints `repository.repositoryUri` with the authority `<registryId>.dkr.ecr.<region>.localhost`, a synthetic non-resolvable TLD that is neither AWS's registry domain nor a host the emulator's own listener answers on. The four causes are four distinct replay clusters on four different operations/response shapes, so the URI is built in (at least) four serialization paths and each must be corrected and re-replayed.

Cause 1 of 4 — CreateRepository (fixture api-image-push-via-layer-upload, step setup#1; 20 affected steps).
AWS: Smithy (com.amazonaws.ecr) Repository.repositoryUri, target Url — "The URI for the repository. You can use this URI for container image push and pull operations." On AWS the value is `<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>`: an authority that both carries the registry identity (the 12-digit owning account id from Repository.registryId, the region of the endpoint) and is a host that actually serves the registry, so the same string is usable as a `docker login`/`push`/`pull` target and as an ECS task-definition image reference. registryId is `^[0-9]{12}$`; repositoryName is the request's name. The account id, region and repository name in any one recording are values taken from the request/state — the requirement is the authority pattern plus the dialability of the host.
Emulator: CreateRepository answers HTTP 200 (status matches) with `repository.repositoryUri` = `<registryId>.dkr.ecr.<region>.localhost/<repositoryName>` — recorded divergence `$.repository.repositoryUri: host_not_own (expected '000000000000.dkr.ecr.us-west-2.amazonaws.com/<repo>', got '000000000000.dkr.ecr.us-west-2.localhost/<repo>')`. The `.localhost` suffix replaces AWS's domain and is also not one of the emulator's own serving hosts, so the published address resolves to nothing.
Why: a client that uses repositoryUri as AWS documents it cannot reach the registry at all — the host does not resolve and nothing listens on it, so no image push, pull or digest-pinned ECS image reference built from the create response can work, and the failure surfaces as an unresolvable host rather than an API error.
Fix: the ECR CreateRepository handler's repository serializer (the single registry-authority helper it should share) must derive the authority from the serving endpoint configuration while preserving `<registryId>.dkr.ecr.<region>` identity, instead of appending a synthetic `.localhost` TLD.

Cause 2 of 4 — DeleteRepository (fixture api-image-push-via-layer-upload, step teardown#14; 20 affected steps).
AWS: DeleteRepository returns the deleted Repository; its repositoryUri carries the same documented value as above (same shape member, same requirement).
Emulator: 200 with `$.repository.repositoryUri: host_not_own (expected '000000000000.dkr.ecr.us-west-2.amazonaws.com/<repo>', got '000000000000.dkr.ecr.us-west-2.localhost/<repo>')`.
Why: a client that records or logs the deleted repository's URI (for cleanup bookkeeping or to compare against a task definition's image) sees an address that is not the registry's, so delete-time reconciliation against image references cannot match.
Fix: the DeleteRepository handler's Repository serializer — same shared registry-authority helper as Cause 1.

Cause 3 of 4 — DescribeRepositories, single-repository response (fixture api-repository-lifecycle-with-policy-and-tags, step task#2; 6 affected steps).
AWS: DescribeRepositories returns `repositories[]` of Repository; each element's repositoryUri is the documented push/pull URI on the account's registry host for the endpoint's region.
Emulator: 200 with `$.repositories[0].repositoryUri: host_not_own (expected '000000000000.dkr.ecr.us-west-2.amazonaws.com/<repo>', got '000000000000.dkr.ecr.us-west-2.localhost/<repo>')`.
Why: the read-back path is how clients resolve an existing repository's push target; a wrong authority here makes a read-then-push flow fail even when the repository exists.
Fix: the DescribeRepositories handler's per-element Repository serializer.

Cause 4 of 4 — DescribeRepositories, multi-repository/pagination response (fixture ecr-repository-pagination-and-scoping-conflict, step task#4; 1 affected step).
AWS: every element of `repositories[]` independently carries the correct registry authority, whatever the page size.
Emulator: 200 with the same defect on EVERY element — `$.repositories[0].repositoryUri` and `$.repositories[1].repositoryUri` both `host_not_own` (expected `...us-west-2.amazonaws.com/<name>`, got `...us-west-2.localhost/<name>`), so the fault is in the element serializer and not a one-off of the first entry.
Why: a client enumerating repositories to build a push/pull map gets an entire map of undialable addresses.
Fix: same element serializer as Cause 3; correcting the shared helper must be verified against a multi-element page, not only a single-element one.

Related open issues, none of which carries these replay keys: 20261126T025732Z-codebuild-ecr-ecs-elbv2-logs-createrepository-announces-repositoryuri.md describes this exact `.localhost` authority but is a BEHAVIORAL issue keyed on codebuild__ecr__ecs__elbv2__logs check ids; 20261124T024116Z-ecr-repositoryuri-is-served-as-the-emulator-s.md and 20261125T054057Z-ecr-ecs-logs-createrepository-s-repositoryuri-is-the.md describe the earlier bare-host:port symptom. 20261121T083658Z-ecr-getauthorizationtoken-announces-a-proxyendpoint-that-is-not.md owns the matching proxyEndpoint divergence this run (skipped here) — it must agree with whatever authority this fix mints.

## Evidence
Replay of trace set simparity-wire-aws-v1: 567 steps, 519 passed, 48 mismatched, score 91.5%. All 48 mismatched steps are ECR repositoryUri divergences in 5 clusters; 1 cluster (GetAuthorizationToken proxyEndpoint) is owned by 20261121T083658Z-ecr-getauthorizationtoken-announces-a-proxyendpoint-that-is-not.md and is excluded. The 4 clusters below are net-new (owned_by empty, stub_path empty).

Cause 1 — service=ecr|CreateRepository|status=200->200|keys=$.repository.repositoryUri, representative api-image-push-via-layer-upload#setup#1, 20 affected steps (api-image-scanning-and-findings#setup#1, api-immutable-tag-conflict-and-image-deletion#setup#1, api-lifecycle-policy-preview-and-apply#setup#1, api-registry-policy-and-scanning-configuration#setup#3, api-repository-lifecycle-with-policy-and-tags#setup#1, ecr-duplicate-push-and-tag-based-storage-class-update#setup#1, ecr-image-and-repo-not-found-errors#setup#1, ecr-image-lookup-errors#setup#1, ecr-image-push-and-inspect#task#1, ecr-immutable-tag-conflict#setup#1, ecr-layer-upload-lifecycle-and-errors#setup#1, ecr-multi-image-isolation-in-shared-repository#setup#1, ecr-push-multi-layer-image#task#1, ecr-put-image-missing-layers-error#setup#1, ecr-put-image-missing-layers-error-then-recover#setup#1, ecr-repository-create-verify-delete#task#1, ...).
  Expected (AWS, status 200): repository.repositoryUri = '000000000000.dkr.ecr.us-west-2.amazonaws.com/sim2real-probe-ecr-ecr-repo-bab899a5'
  Actual   (emulator, status 200): '000000000000.dkr.ecr.us-west-2.localhost/sim2real-probe-ecr-ecr-repo-bab899a5'  [verdict mismatch, detail 'host_not_own']

Cause 2 — service=ecr|DeleteRepository|status=200->200|keys=$.repository.repositoryUri, representative api-image-push-via-layer-upload#teardown#14, 20 affected steps (api-image-scanning-and-findings#teardown#12, api-immutable-tag-conflict-and-image-deletion#teardown#15, api-lifecycle-policy-preview-and-apply#teardown#10, api-registry-policy-and-scanning-configuration#teardown#14, ecr-image-push-and-inspect#task#12, ecr-repository-create-verify-delete#task#4, ...).
  Expected: '000000000000.dkr.ecr.us-west-2.amazonaws.com/<repo>'  Actual: '000000000000.dkr.ecr.us-west-2.localhost/<repo>'

Cause 3 — service=ecr|DescribeRepositories|status=200->200|keys=$.repositories[0].repositoryUri, representative api-repository-lifecycle-with-policy-and-tags#task#2, 6 affected steps (ecr-repository-create-verify-delete#task#3, ecr-repository-explicit-mutable-tags-defaults-read-back-tag-change-remove-last-and-not-found#task#2, ecr-repository-immutable-aes256-scan-off-own-account-policy-and-lifecycle-round-trip#task#2 and #task#9, ecr-repository-pagination-and-scoping-conflict#task#5).
  Expected: '000000000000.dkr.ecr.us-west-2.amazonaws.com/sim2real-probe-ecr-ecr-repo-d36f360b'  Actual: '000000000000.dkr.ecr.us-west-2.localhost/sim2real-probe-ecr-ecr-repo-d36f360b'

Cause 4 — service=ecr|DescribeRepositories|status=200->200|keys=$.repositories[0].repositoryUri,$.repositories[1].repositoryUri, representative ecr-repository-pagination-and-scoping-conflict#task#4, 1 affected step.
  Expected: repositories[0] '000000000000.dkr.ecr.us-west-2.amazonaws.com/sim2real-probe-ecr-pag-44ff074a', repositories[1] '...amazonaws.com/sim2real-probe-ecr-pag-25901805'
  Actual:   repositories[0] '000000000000.dkr.ecr.us-west-2.localhost/sim2real-probe-ecr-pag-44ff074a', repositories[1] '...us-west-2.localhost/sim2real-probe-ecr-pag-25901805'

What is fixed text vs per-run value: the diverging fixed text is the authority pattern — AWS serves `<registryId>.dkr.ecr.<region>.<aws-domain>` while the emulator serves `<registryId>.dkr.ecr.<region>.localhost`. The account id (000000000000), the region (us-west-2), the generated repository names (sim2real-probe-ecr-*) and the emulator's listener port are per-run values taken from the request and the emulator's state, not requirements. No HTTP status or error code differs: both sides answer 200, so the defect is purely the value of the published address.

Smithy consulted: com.amazonaws.ecr#Repository — members registryId (pattern ^[0-9]{12}$), repositoryName, repositoryUri (target Url, doc: "The URI for the repository. You can use this URI for container image push and pull operations.").

## Steps to Reproduce
1. Boot the emulator from this checkout and note the host:port its data listener answers on.
2. Cause 1: call ecr:CreateRepository with any valid repositoryName in us-west-2 and read repository.repositoryUri from the 200 response -> emulator returns `<registryId>.dkr.ecr.us-west-2.localhost/<name>`; AWS returns `<registryId>.dkr.ecr.us-west-2.amazonaws.com/<name>`, a host that serves the registry. Try to dial the announced host: nothing resolves/listens.
3. Cause 3: call ecr:DescribeRepositories for that repository and read repositories[0].repositoryUri -> same `.localhost` authority.
4. Cause 4: create a second repository, call ecr:DescribeRepositories with a page covering both (as fixture ecr-repository-pagination-and-scoping-conflict#task#4 does) and read repositories[0] and repositories[1] -> every element carries the `.localhost` authority.
5. Cause 2: call ecr:DeleteRepository on one of them and read repository.repositoryUri from the 200 response -> same `.localhost` authority.
6. Re-replay to confirm the fix: api-image-push-via-layer-upload#setup#1 (CreateRepository), api-image-push-via-layer-upload#teardown#14 (DeleteRepository), api-repository-lifecycle-with-policy-and-tags#task#2 (DescribeRepositories, single) and ecr-repository-pagination-and-scoping-conflict#task#4 (DescribeRepositories, multi-element page) — all four clusters must pass, i.e. all 48 mismatched steps minus the GetAuthorizationToken cluster already owned elsewhere.
