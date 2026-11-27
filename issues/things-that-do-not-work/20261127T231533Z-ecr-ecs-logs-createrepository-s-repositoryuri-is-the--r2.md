Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull
Issue-Key: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task|_pull_through_cache_repo#4|AssertionError|assert ".dkr.ecr." in uri, uri
Issue-Key: ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull|_pull_through_cache_repo#3|AssertionError|assert ".dkr.ecr." in uri, uri
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# ecr__ecs__logs: CreateRepository's repositoryUri is the emulator's bare host:port, so a pull-through-cache image URI is not an ECR registry URI

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
One root cause, grounded in 2 failing checks of this service heading.

Cause 1 of 1: the `repository.repositoryUri` that ECR returns carries no ECR registry authority — it is the emulator's own serving `host:port`.
Failing checks: ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task, ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull.

AWS: Smithy model (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json), shape `Repository`, member `repositoryUri`: "The URI for the repository. You can use this URI for container image push and pull operations." Amazon ECR publishes that URI as the account's registry DNS name followed by the repository name — `<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>` — where `<registryId>` is the 12-digit account that owns the registry (CreateRepository's `registryId` input, defaulting to the caller's account) and `<region>` is the Region the call was made in. For a pull-through cache repository the repository name is `<ecrRepositoryPrefix>/<upstream image path>`, so the private URI is `<registryId>.dkr.ecr.<region>.amazonaws.com/<prefix>/docker/library/busybox`. The fixed part of the authority is the literal `.dkr.ecr.` label and the registry suffix; the variable parts are the registry id, the Region and the repository name, all taken from the request and the emulator's own account/Region configuration. CreateRepository returns HTTP 200 here — this is not a refusal, the success body carries the wrong value.

Emulator: CreateRepository answers 200, but `repository.repositoryUri` is the address the emulator answers registry requests on. The checks assert `".dkr.ecr." in uri` on the value they got back and fail with the URI verbatim: `127.0.0.1:41157/sim2real-xsvc-ptc-217e29806001/docker/library/busybox:latest` (first check) and `127.0.0.1:34815/sim2real-xsvc-ptc-c3920507b80f/docker/library/busybox:latest` (second). The host and port differ per run and are not themselves the requirement; what is invariant across both runs is that the authority is a bare `host:port` with no `<registryId>.dkr.ecr.<region>` registry identity and no account or Region in it. create_repository.rs's own module doc states this intent ("the URI it is pushed and pulled at, which is the address this emulator answers the reading caller's registry requests at (crate::state::registry_address)"), so the divergence is by construction rather than an accident of one response.

Why: a client cannot tell which registry, account or Region a repository belongs to from its published URI, and two same-named repositories in different Regions or accounts collapse onto one identity. Everything downstream that parses the URI breaks: an ECS task definition `image` built from `repositoryUri` is not a resolvable ECR reference, `docker login`/`GetAuthorizationToken` cannot be matched to a registry, and any consumer that derives the registry id or Region by splitting the authority on `.dkr.ecr.` gets nothing. Both failing checks stop at the repository-creation step and never get to exercise what they are about (a pull-through-cache pull by an ECS task, and the execution-role permission differential), so two whole behaviours go unverified.

Fix: the repositoryUri minting shared by every Repository-shaped response — `crate::state::registry_address` / the Repository projection in services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs, consumed by src/ops/create_repository.rs and by DescribeRepositories and the other Repository-returning handlers. It must compose the registry authority from the owning registry id and the request's Region rather than from the emulator's listening socket, so the same value is read back by every later operation.

Related already-open issues, named here because none of them covers this service heading's checks: 20261124T024116Z-ecr-repositoryuri-is-served-as-the-emulator-s.md and 20261127T202610Z-batch-ecr-lambda-logs-ecr-repositoryuri-carries-no-region-nor-registry-authority.md report the same shape of defect under the `ecr` and `batch__ecr__lambda__logs` headings. 20261126T031516Z-ecr-ecs-logs-a-fargate-task-whose-image.md is open against these same two checks for a later step (the pull-through-cache task never reaches RUNNING); the replay now fails earlier, at the repositoryUri assertion, so that issue's step is not even reached. Several resolved issues (20261121T160051Z-ecr-ecs-logs-a-pull-through-cache-repository.md, 20261125T054057Z-ecr-ecs-logs-createrepository-s-repositoryuri-is-the.md, 20261120T083150Z-ecr-ecs-logs-createrepository-returns-a-repositoryuri-that.md) claimed this exact value; the replay shows the bare host:port form is back, and a resolved issue does not cover a live failure.

Traces: the two failing checks above; both abort in their `_pull_through_cache_repo` helper on `assert ".dkr.ecr." in uri` immediately after `ecr.create_repository(...)["repository"]["repositoryUri"]`.

## Evidence
Expected (AWS): CreateRepository 200 with repository.repositoryUri = "<registryId>.dkr.ecr.<region>.amazonaws.com/<repositoryName>" — for these checks "<account>.dkr.ecr.<region>.amazonaws.com/sim2real-xsvc-ptc-<suffix>/docker/library/busybox", so appending ":latest" yields a pullable ECR image reference containing the literal ".dkr.ecr." label.

Actual (emulator): CreateRepository 200 with repository.repositoryUri = the emulator's bare serving authority and no registry identity. Replay output:
  ecr-pull-through-cache-image-pulled-by-ecs-task — AssertionError: 127.0.0.1:41157/sim2real-xsvc-ptc-217e29806001/docker/library/busybox:latest (at `assert ".dkr.ecr." in uri`)
  ecs-exec-role-without-ecr-auth-cannot-pull — AssertionError: 127.0.0.1:34815/sim2real-xsvc-ptc-c3920507b80f/docker/library/busybox:latest (at `assert ".dkr.ecr." in uri`)
The host/port values are per-run and not part of the requirement; the invariant divergence is the absence of the "<registryId>.dkr.ecr.<region>" authority. No error code or status diverges — the body value does.

## Steps to Reproduce
1. CreatePullThroughCacheRule with ecrRepositoryPrefix=<prefix>, upstreamRegistryUrl="public.ecr.aws".
2. CreateRepository with repositoryName="<prefix>/docker/library/busybox".
3. Read repository.repositoryUri from the 200 response (DescribeRepositories on the same name shows the same value).
4. Observe the authority is the emulator's host:port, e.g. "127.0.0.1:41157/<prefix>/docker/library/busybox", with no ".dkr.ecr." label, no 12-digit registry id and no Region; AWS returns "<registryId>.dkr.ecr.<region>.amazonaws.com/<prefix>/docker/library/busybox".
5. Re-replay checks ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task and ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull to confirm the fix.
