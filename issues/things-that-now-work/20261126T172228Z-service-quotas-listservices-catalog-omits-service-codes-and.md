Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-services
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-services
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListServices|2xx->2xx|$.Services[*]
Issue-Commit: bf5aedd86bc1c51d8b010649bf7e2337ac48d430

# service-quotas ListServices catalog omits service codes and shifts the Services list (PARTIAL CLOSE: every code this report names is registered and resolves, and every registered `(ServiceCode, ServiceName)` pair it QUOTES is now served; the positions that still diverge are inventory entries whose registered display name no source in this checkout states)

> _Note: this issue was identified against the code base at commit `bf5aedd86bc1c51d8b010649bf7e2337ac48d430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
PARTIAL CLOSE (added by the fixer). All ten service codes this report names as absent
(`migrationhubstrategy`, `networkinsights`, `observabilityadmin`, `opensearch`,
`pca-connector-ad`, `pca-connector-scep`, `qbusiness`, `rolesanywhere`,
`route53-recovery-control-config`, `route53-recovery-readiness`) are registered in the
inventory, are listed in `ServiceCode` order and resolve their quota listings, so no code
this report names is undiscoverable any more. The six registered display names it QUOTES
were served as a "tidied" product name or as the code echoed back, and are corrected to
the registered string: `migrationhubstrategy` -> 'Migration Hub Strategy Recommendations',
`networkinsights` -> 'Network Insights', `observabilityadmin` -> 'Amazon CloudWatch
Observability Admin', `opensearch` -> 'OpenSearch Application' (the `es` code keeps
'Amazon OpenSearch Service'), `qbusiness` -> 'Amazon Q', `rolesanywhere` -> 'IAM Roles
Anywhere'. Replay confirms those positions no longer diverge. What still diverges are
inventory entries whose registered `ServiceName` neither this report, nor the checkout's
other service-quotas reports, nor the Smithy model states — the model enumerates no
catalog, so the registered string is the only authority and the corpus withholds it; no
rule derives it either (the registrations drop a prefix, keep a prefix, drop a suffix or
name a plane, with no pattern over the catalog). Read the `Issue-Key` above as the key of
the ORIGINAL sweep, not as a key that now matches in full.

LOCKFILE NOTE (added by the fixer, so the reason for the one dependency-resolution line in
this change is stated in the change itself). This fix needs no dependency: its only edits
outside the service-quotas crate are to `Cargo.lock`, where the third-party `pymath` crate's
`malachite-bigint` is held at the 0.12.0 line the rest of the tree already locks (the
workspace's own `python-guest-substrate` requires exactly that version). The 0.15.0 line a
fresh resolution picks declares `rust-version = 1.100.0`, which this toolchain (rustc 1.99.1)
does not meet, so with 0.15.0 locked `cargo check --locked --workspace` refuses to run at
all: "rustc 1.99.1 is not supported by the following packages: malachite-base@0.15.0 requires
rustc 1.100.0 …". Holding the one line is what lets the workspace compile here; no crate is
added, and no other version is moved.

Cause 1 of 1: ListServices returns the wrong service catalog.

AWS: Smithy ListServices -> ListServicesResponse.Services: list<ServiceInfo>, ServiceInfo = {ServiceCode (@length 1-63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$), ServiceName}; the operation is @paginated (inputToken/outputToken NextToken, items Services, pageSize MaxResults @range 1-100). The model fixes the shape but not the catalog contents, so the membership and ordering of the list are recorded behavior only: AWS returns one ServiceInfo per Service Quotas-integrated service, ordered by ServiceCode, so a client reading index N or scanning for a code finds that code. Status is 200 on both sides and no error is involved (modelled errors on this op are AccessDeniedException 403, IllegalArgumentException 400, InvalidPaginationTokenException 400, ServiceException 500, TooManyRequestsException 429), so there is no status, error code or error message to quote — the divergence is entirely in the body.

Emulator: HTTP 200 with a Services list that is missing a large block of service codes, so every element from the first gap onward is shifted and 79 list positions diverge. Codes absent from the emulator's catalog include migrationhubstrategy ('Migration Hub Strategy Recommendations'), networkinsights ('Network Insights'), observabilityadmin ('Amazon CloudWatch Observability Admin'), opensearch ('OpenSearch Application'), pca-connector-ad, pca-connector-scep, qbusiness ('Amazon Q'), rolesanywhere ('IAM Roles Anywhere'), route53-recovery-control-config and route53-recovery-readiness; at those positions the emulator instead serves codes that belong later in the ordering (marketplaceagreement, mediastore, memorydb, migrationhuborchestrator, neptune, neptune-graph, omics, private-networks, profile, proton). The general fault is that the seeded catalog is a strict, incomplete subset of the recorded one, re-indexed by the omissions, not a permutation of equal content.

Why: a client that enumerates ListServices to discover which services expose quotas (a console-style picker, a sweep that then calls ListServiceQuotas/ListAWSDefaultServiceQuotas per code, a compliance inventory) silently loses every omitted service — the call succeeds with 200 and a plausible list, so the omission reads as 'this service has no quotas' rather than as an error. Pagination makes it worse: with MaxResults set, the shifted list hands a client a different page boundary than AWS for the same NextToken sequence.

Fix: the service-quotas ListServices handler's seeded service catalog (the ServiceInfo table it serves and the order it serves it in) — the response shape, pagination wiring and 200 status are correct.

No open issue covers ListServices: the existing service-quotas issues are all against ListAWSDefaultServiceQuotas per service code.

Traces: fixture service-quotas-catalog-list-services, step task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture service-quotas-catalog-list-services step task#1, operation ListServices, verdict=mismatch, expected_status 200, actual_status 200, 79 diverging keys ($.Services[40], [48], [50], [79], [80], [106], [127], [135], [142], [169], [170], [173], [175], [181], [184], [186], [187], [190], [191], [193]... through [309]).

Examples from the replay detail:
- $.Services[175]: expected {'ServiceCode': 'migrationhubstrategy', 'ServiceName': 'Migration Hub Strategy Recommendations'}, got {'ServiceCode': 'marketplaceagreement', 'ServiceName': 'AWS Marketplace Agreement Service'}
- $.Services[181]: expected {'ServiceCode': 'networkinsights', 'ServiceName': 'Network Insights'}, got {'ServiceCode': 'mediastore', 'ServiceName': 'AWS Elemental MediaStore'}
- $.Services[184]: expected {'ServiceCode': 'observabilityadmin', 'ServiceName': 'Amazon CloudWatch Observability Admin'}, got {'ServiceCode': 'memorydb', 'ServiceName': 'Amazon MemoryDB'}
- $.Services[186]: expected {'ServiceCode': 'opensearch', 'ServiceName': 'OpenSearch Application'}, got {'ServiceCode': 'migrationhuborchestrator', 'ServiceName': 'Migration Hub Orchestrator'}
- $.Services[190]/[191]: expected pca-connector-ad / pca-connector-scep, got neptune / neptune-graph
- $.Services[200]: expected qbusiness ('Amazon Q'), got omics ('AWS HealthOmics')
- $.Services[214]-[216]: expected rolesanywhere, route53-recovery-control-config, route53-recovery-readiness, got private-networks, profile, proton

The uniform pattern (expected code E at index i, actual a code that sorts earlier) shows the emulator's list is the recorded list with entries removed, shifting all later indices.

Model used: smithy_query operation=ListServices and shape=ServiceInfo on services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call service-quotas ListServices (no NextToken, default MaxResults) and collect every ServiceInfo, paginating to the end.
3. Diff the set of ServiceCode values against the recorded response: codes such as migrationhubstrategy, networkinsights, observabilityadmin, opensearch, pca-connector-ad, pca-connector-scep, qbusiness, rolesanywhere, route53-recovery-control-config and route53-recovery-readiness are absent, and every index after the first omission names a different service than the recording.
4. Equivalently, re-replay fixture service-quotas-catalog-list-services, step task#1.
