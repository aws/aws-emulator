Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-layer-upload-and-push-workflow
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-layer-upload-and-push-workflow api-multi-repository-isolation api-repository-creation-catalog-lifecycle api-repository-policy-management api-repository-uri-arn-alias-scoped-regionless api-resource-tagging-workflow api-repository-deletion-and-non-empty-error-paths ecr-public-describe-repositories-max-results-pagination api-registry-level-operations-and-authorization
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|CreateRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr-public|DeleteRepository|2xx->2xx|$.repository.repositoryUri
Issue-Key: ecr-public|DescribeRepositories|2xx->2xx|$.repositories[*].repositoryUri
Issue-Key: ecr-public|DescribeRegistries|2xx->2xx|$.registries[*].registryUri
Issue-Commit: c3e5fb924e64874a3c7124e31a5bf7fe6194984f

# ECR Public publishes registry and repository URIs on an unserved public.ecr.<loopback-zone> authority (4 ops, 5 causes)

> _Note: this issue was identified against the code base at commit `c3e5fb924e64874a3c7124e31a5bf7fe6194984f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Wire replay of simparity-wire-aws-v1 against this checkout: 16 fixtures / 180 steps, 148 passed, 32 mismatched (score 82.2%), emulator_up=true, bootstrap_signal=false, divergence_count=7. ecr-public IS bootstrapped (service-com-amazonaws-ecrpublic-spencerfrontendservice is in the bootstrapped set), so these are fidelity defects, not a bootstrap gap. Two of the seven clusters (UploadLayerPart) are owned_by 20261127T122205Z-ecr-public-uploadlayerpart-error-bodies-diverge-unenforced-region.md and are skipped here. The remaining FIVE clusters are net-new (no stub_path, no owned_by) and are bundled below. They share ONE root fault, but each is a separate response surface with its own replay key, so a fixer must re-replay and close ALL FIVE before submitting.

SHARED ROOT FAULT: every URI this service publishes is composed as `public.ecr.<announced zone>` by state/registry_address.rs (`published_gallery_authority` -> `gallery_host_in_zone(announced_gallery_zone(ctx))`, with `registry_uri`/`repository_uri` appending `<alias>` and `<alias>/<name>`). The zone it writes those fixed labels over is the RFC 6761 loopback zone (`LOOPBACK_ZONE`, i.e. `localhost`) and the port the emulator actually answers on is deliberately dropped, so the published authority is `public.ecr.localhost` — a name that is neither AWS's public-gallery authority nor any address this emulator is reached on. The replay classifies every one of these values `host_not_own`: the service announces an address other than the endpoint it is serving the request from (the run's data listener is :37271). The specific host, port and registry alias are per-environment values and are NOT the requirement; the requirement is that the value a client is handed for `push`/`pull` names an address that actually serves this registry.

=== Cause 1 of 5: CreateRepository response `$.repository.repositoryUri` ===
AWS: Smithy `Repository$repositoryUri` -> `Url`, documented only as "The URI for the repository. You can use this URI for container image push and pull operations" (CreateRepository output = `repository: Repository`, `catalogData`), so the value's structure is recorded behavior only. The recording shows a 200 whose repositoryUri is `public.ecr.<gallery zone>/<registry alias>/<repository name>`: the fixed labels `public` and `ecr` lead the authority, the rest of the authority is the zone that actually SERVES the registry the caller is talking to, no port, and the path is `<alias>/<name>` with the repository name taken from the request. No error code is involved. The fixed text is the leading `public.ecr.` labels and the `<alias>/<name>` path shape; the variable parts are the serving zone, the registry alias and the requested repository name.
Emulator: 200 with the right path shape but an authority it does not serve — `public.ecr.localhost/<alias>/<name>` where the recording has `public.ecr.<serving zone>/<alias>/<name>` (replay detail: `$.repository.repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-...', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-...')`). Dialling the published name does not reach the emulator: the loopback-zone label is not the endpoint the request arrived on and the serving port is stripped.
Why: repositoryUri is the one value a client feeds to `docker push`/`docker pull` and to registry tooling. A client that uses the address the API handed it gets a connection to nothing (or, where `*.localhost` resolves, to port 443 on loopback where no listener answers), so the whole image push/pull plane is unusable even though every control-plane call succeeds.
Fix: the URI publication helper in the ecr-public state (`published_gallery_authority` / `announced_gallery_zone` in services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/state/registry_address.rs), which CreateRepository's serializer reads — it must write the `public.ecr.` labels over an authority this process really answers on (the endpoint the request was served from), not over the loopback zone with the port discarded.
Traces: api-image-layer-upload-and-push-workflow#setup#1 (representative, 8 steps in cluster: api-image-layer-upload-and-push-workflow#setup#1, api-multi-repository-isolation#setup#1, api-multi-repository-isolation#setup#2, api-repository-creation-catalog-lifecycle#setup#1, api-repository-policy-management#setup#1, api-repository-uri-arn-alias-scoped-regionless#task#2, api-repository-uri-arn-alias-scoped-regionless#task#4, api-resource-tagging-workflow#setup#1).

=== Cause 2 of 5: DeleteRepository response `$.repository.repositoryUri` ===
AWS: DeleteRepository output = `repository: Repository` (same `Repository$repositoryUri` member and documentation; modelled errors RepositoryNotFoundException / RepositoryNotEmptyException are not involved — the recorded call is a 200). The deleted repository is reported back with the same published URI form as on creation: `public.ecr.<serving zone>/<alias>/<name>`.
Emulator: 200 with `public.ecr.localhost/<alias>/<name>` — same unserved authority (`host_not_own`).
Why: a client that reconciles what it just removed (cache invalidation, local image-reference cleanup, audit records) compares the URI it was given at create time with the one in the delete response; both are addresses that cannot be reached, so neither identifies a real registry endpoint.
Fix: same publication helper, read by DeleteRepository's response serializer.
Traces: api-image-layer-upload-and-push-workflow#teardown#17 (representative, 8 steps: api-image-layer-upload-and-push-workflow#teardown#17, api-multi-repository-isolation#task#12, api-multi-repository-isolation#teardown#15, api-repository-creation-catalog-lifecycle#teardown#8, api-repository-deletion-and-non-empty-error-paths#task#14, api-repository-deletion-and-non-empty-error-paths#task#23, api-repository-policy-management#teardown#11, api-resource-tagging-workflow#teardown#11).

=== Cause 3 of 5: DescribeRepositories multi-entry listing `$.repositories[*].repositoryUri` ===
AWS: DescribeRepositories output = `repositories: RepositoryList` (+ `nextToken`), each entry a `Repository` carrying the same published `repositoryUri`. Every entry of a multi-repository page carries an authority that serves that registry.
Emulator: every entry carries `public.ecr.localhost/<alias>/<name>` (`host_not_own` on both `$.repositories[0].repositoryUri` and `$.repositories[1].repositoryUri`), so the defect is per-entry and not confined to the single-entry case.
Why: a client enumerating a registry's repositories to pick one to pull from has no reachable address for any of them.
Fix: same publication helper; it is read per response entry by the DescribeRepositories serializer.
Observation (NOT the flagged divergence, and not filed as a cause): in this cluster the recording's entries are ordered frontend-repo then backend-repo while the emulator answers backend-repo then frontend-repo, so the two listings are positionally transposed. The recording does not assert repositoryName positionally, so the replay flags only the URIs; a fixer should be aware the pairing above is positional.
Traces: api-multi-repository-isolation#task#11 (representative, 2 steps: api-multi-repository-isolation#task#11, ecr-public-describe-repositories-max-results-pagination#task#5).

=== Cause 4 of 5: DescribeRepositories single-entry / paginated page `$.repositories[0].repositoryUri` ===
AWS: as Cause 3; this cluster covers the single-entry and max-results-paginated reads (`paginated`: inputToken nextToken / items repositories / pageSize maxResults), where the only diverging member is the published URI.
Emulator: `public.ecr.localhost/<alias>/<name>` instead of the serving-zone authority (`host_not_own`); everything else on the page — repositoryName, registryId, repositoryArn, createdAt, the pagination token — passes.
Why: the unreachable address is served on the ordinary read path a client uses to resolve a repository it already knows the name of, so no caller can obtain a dialable address by any route.
Fix: same publication helper.
Traces: api-multi-repository-isolation#task#14 (representative, 5 steps: api-multi-repository-isolation#task#14, api-repository-creation-catalog-lifecycle#task#2, api-repository-deletion-and-non-empty-error-paths#task#12, ecr-public-describe-repositories-max-results-pagination#task#3, ecr-public-describe-repositories-max-results-pagination#task#4).

=== Cause 5 of 5: DescribeRegistries response `$.registries[0].registryUri` ===
AWS: Smithy `Registry$registryUri` -> `Url`, required, documented as "The URI of a public registry. The URI contains a universal prefix and the registry alias" (DescribeRegistries output = `registries: RegistryList`, required). So the value is the universal `public.ecr.` prefix over the authority serving that registry, scoped by the registry's own alias: `public.ecr.<serving zone>/<alias>`, no repository path, no port. The recording does not pin the exact value (expected `<shaped-not-asserted>`), it pins the shape, and the replay's own-host check is part of that shape.
Emulator: `public.ecr.localhost/n7v9k4n5` — the universal prefix and the alias segment are right, the authority is one the emulator does not answer on (`host_not_own`).
Why: the registry URI is the base a client composes every image reference on (`<registryUri>/<repo>:<tag>`); a registry-level address that resolves nowhere makes every reference derived from it undialable, and tooling that validates the registry endpoint before pushing fails at the registry level rather than per repository.
Fix: same publication helper, read by the DescribeRegistries serializer (`registry_uri(published_gallery_authority(ctx), alias)`).
Traces: api-registry-level-operations-and-authorization#task#3 (representative, 2 steps: api-registry-level-operations-and-authorization#task#3, api-repository-uri-arn-alias-scoped-regionless#task#1).

Related already-open issue (does NOT cover these): 20261122T162711Z-ecr-public-iam-ecr-public-publishes-repositoryuri-as-the-bare-serving.md is a BEHAVIORAL issue reporting that repositoryUri was published as the bare serving `host:port` with no `public.ecr.` labels at all. The labels are now present, so that issue's symptom has moved: the authority is now `public.ecr.localhost` with the serving port dropped. It carries no wire replay key for any of the five clusters above, so this issue is filed separately and both must be re-replayed.

## Evidence
Replay summary (simparity-wire-aws-v1): {"kind":"summary","emulator_up":true,"bootstrap_signal":false,"fixtures_replayed":16,"steps_total":180,"steps_passed":148,"steps_mismatched":32,"steps_errored":0,"steps_blocked":0,"not_implemented":0,"score_pct":82.2,"divergence_count":7}. emulator_detail: "/__admin/health answered 200 and the :37271 data listener accepts".

Cause 1 — divergence_key "service=ecr-public|CreateRepository|status=200->200|keys=$.repository.repositoryUri", verdict mismatch, expected_status 200, actual_status 200, stub_path "", owned_by []. Representative api-image-layer-upload-and-push-workflow#setup#1, affected_count 8. detail:
  $.repository.repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-ecr-public-push-repo-02c1a31d', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-ecr-public-push-repo-02c1a31d')

Cause 2 — divergence_key "service=ecr-public|DeleteRepository|status=200->200|keys=$.repository.repositoryUri", verdict mismatch, 200->200, stub_path "", owned_by []. Representative api-image-layer-upload-and-push-workflow#teardown#17, affected_count 8. detail:
  $.repository.repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-ecr-public-push-repo-02c1a31d', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-ecr-public-push-repo-02c1a31d')

Cause 3 — divergence_key "service=ecr-public|DescribeRepositories|status=200->200|keys=$.repositories[0].repositoryUri,$.repositories[1].repositoryUri", verdict mismatch, 200->200, stub_path "", owned_by []. Representative api-multi-repository-isolation#task#11, affected_count 2. detail:
  $.repositories[0].repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-ecr-public-frontend-repo-27ed4f43', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-ecr-public-backend-repo-27ed4f43')
  $.repositories[1].repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-ecr-public-backend-repo-27ed4f43', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-ecr-public-frontend-repo-27ed4f43')
(the frontend/backend transposition shows the comparison is positional; only the URI authority is flagged)

Cause 4 — divergence_key "service=ecr-public|DescribeRepositories|status=200->200|keys=$.repositories[0].repositoryUri", verdict mismatch, 200->200, stub_path "", owned_by []. Representative api-multi-repository-isolation#task#14, affected_count 5. detail:
  $.repositories[0].repositoryUri: host_not_own (expected 'public.ecr.aws/a6g2m9v2/sim2real-probe-ecr-public-backend-repo-27ed4f43', got 'public.ecr.localhost/n7v9k4n5/sim2real-probe-ecr-public-backend-repo-27ed4f43')

Cause 5 — divergence_key "service=ecr-public|DescribeRegistries|status=200->200|keys=$.registries[0].registryUri", verdict mismatch, 200->200, stub_path "", owned_by []. Representative api-registry-level-operations-and-authorization#task#3, affected_count 2. detail:
  $.registries[0].registryUri: host_not_own (expected '<shaped-not-asserted>', got 'public.ecr.localhost/n7v9k4n5')

The hosts, ports, registry aliases (a6g2m9v2 / n7v9k4n5) and repository-name suffixes above are per-run environment values and are not the requirement. The requirement is the one thing every line reports: the published authority is not an address this emulator answers on.

Model corroboration (smithy_query on services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json):
- Repository.repositoryUri -> Url, doc "The URI for the repository. You can use this URI for container image push and pull operations". CreateRepository output {repository: Repository, catalogData}; DeleteRepository output {repository: Repository}; DescribeRepositories output {repositories: RepositoryList, nextToken}, paginated(items=repositories, pageSize=maxResults).
- Registry.registryUri -> Url, REQUIRED, doc "The URI of a public registry. The URI contains a universal prefix and the registry alias". DescribeRegistries output {registries: RegistryList (required), nextToken}.
- No modelled error participates: all five clusters are 200->200 body divergences.

Code corroboration: services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/state/registry_address.rs — GALLERY_HOST_LABELS = ["public","ecr"] (line 76), gallery_host_in_zone (line 91), published_gallery_authority = gallery_host_in_zone(announced_gallery_zone(ctx)) (line 104), announced_gallery_zone returning service_common::data_plane_addressing::LOOPBACK_ZONE whenever the answered endpoint's zone names an AWS gallery zone (lines 134-142), registry_uri (line 265) and repository_uri (line 278) composing `<authority>/<alias>` and `<authority>/<alias>/<name>`; the module docs (lines 43-47) state the serving port is deliberately omitted.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus). 32 of 180 steps mismatch across 7 clusters; 5 of those clusters (25 steps) are the ecr-public URI-publication clusters below, the other 2 are the already-owned UploadLayerPart clusters.
2. Cause 1 — replay api-image-layer-upload-and-push-workflow, step setup#1: CreateRepository(repositoryName='<name>') against the ecr-public endpoint. Read $.repository.repositoryUri: it must be `public.ecr.<zone the registry is served in>/<registry alias>/<name>` and dialling that authority must reach the registry this emulator is serving. The emulator answers `public.ecr.localhost/<alias>/<name>` — loopback zone, serving port discarded, nothing listening there.
3. Cause 2 — replay api-image-layer-upload-and-push-workflow, step teardown#17: DeleteRepository(repositoryName='<name>') on an empty repository; $.repository.repositoryUri carries the same unserved authority.
4. Cause 3 — replay api-multi-repository-isolation, step task#11: DescribeRepositories returning two repositories; every $.repositories[i].repositoryUri carries the unserved authority (and note the entries come back in the opposite order from the recording).
5. Cause 4 — replay api-multi-repository-isolation, step task#14 (and ecr-public-describe-repositories-max-results-pagination#task#3 / #task#4): DescribeRepositories for a single repository / one maxResults page; $.repositories[0].repositoryUri carries the unserved authority while every other member and the nextToken pass.
6. Cause 5 — replay api-registry-level-operations-and-authorization, step task#3: DescribeRegistries; $.registries[0].registryUri must be the universal `public.ecr.` prefix over a serving authority plus the registry alias. The emulator answers `public.ecr.localhost/<alias>`.
7. Verification: make the published authority an address the emulator actually answers on for the endpoint that served the request (keeping the fixed `public.ecr.` labels, the `<alias>` / `<alias>/<name>` path composition and the account/alias/ARN members unchanged), then re-replay the step in Issue-Trace/Issue-Step and all four steps in Issue-Trace-Ids and confirm all 25 previously mismatched steps pass.
8. Regression guard: a container reference written on the newly published authority must still be read back as naming this gallery (state/gallery_pull parse path), and references written by hand on AWS's own `public.ecr.aws/<alias>/<name>` must keep resolving, so publishing and reading do not disagree.
