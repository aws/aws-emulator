Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.deleting-a-space-removes-its-profiles-seeded-grants-and-dashboards
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|ListAccessGrants|ResourceNotFoundException
Issue-Commit: 9c5c108e968319a9f7cfd2267f751f496cc6a487

# cloudwatchomni ListAccessGrants treats the spaceId filter as an existence precondition: 404 ResourceNotFoundException instead of an empty page after the space is deleted

> _Note: this issue was identified against the code base at commit `9c5c108e968319a9f7cfd2267f751f496cc6a487`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccessGrants

## Description
One net-new root cause.

Cause 1 of 1: ListAccessGrants rejects a request whose `spaceId` filter names a space that is no longer in the store, instead of answering 200 with an empty `items` list. Failing check: cloudwatchomni.deleting-a-space-removes-its-profiles-seeded-grants-and-dashboards.

AWS: `spaceId` is a FILTER on the account/Region grant listing, not a path or parent resource. Smithy (com-amazonaws-cloudwatchomni-cloudwatchomnifrontend): ListAccessGrants is @readonly and @paginated(inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: items); input member `spaceId` (SpaceId, pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$) is documented "Filter by space ID." and the operation doc says "A grant is returned only when it matches every filter supplied. With no filters, returns the grants for the current account and Region." Output `items` (AccessGrantSummaryList) is @required, so a listing that matches nothing is an empty list, not an error. Recorded behavior confirms it: after DeleteSpace the recording's ListAccessGrants(spaceId=<deleted space>) returns 200 with `items: []` (the check asserts `left == []`), and the subsequent per-resource reads are the calls AWS refuses — not the listing. ResourceNotFoundException (HTTP 404) is modeled for this operation, but for a resource the request addresses, not for a filter value that simply matches no grant.

Emulator: the same call is refused with HTTP 404 ResourceNotFoundException, message `Space '<spaceId>' does not exist.` (fixed text `Space '` + the request's spaceId + `' does not exist.`, with resourceType="Space", resourceId=<spaceId>). The pre-delete listing of the same space succeeds and returns the two seeded grants, so the refusal is purely the post-delete existence check on the filter value. The same shape applies to the `domainId` filter branch (Hypothesis: a domainId filter naming a deleted/unknown domain is refused the same way — the handler runs the identical store-membership check and emits `Domain '<domainId>' does not exist.`; only the spaceId branch is exercised by the failing check).

Why: a client that deletes a space and then enumerates grants to confirm the cascade — or any client that simply holds a stale spaceId filter — gets an exception where AWS gives an empty page. Pagination helpers (botocore's list_access_grants paginator) raise ResourceNotFoundException mid-iteration instead of yielding an empty page, so cleanup and reconciliation loops abort rather than observing "no grants left". It also makes the listing unusable as the post-delete audit AWS supports.

Fix: the ListAccessGrants handler's parent-existence precondition (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/list_access_grants.rs) — the store-membership checks on `input.space_id` and `input.domain_id` must not turn a filter into a 404; filters should narrow the result set and yield 200 with `items: []` when nothing matches. Note the open issue "Implement cloudwatchomni ListAccessGrants: unimplemented scaffold stub" (20261111T022337Z-cloudwatchomni-listaccessgrants-stub.md) no longer describes this handler — it is implemented and answering; this is a fidelity defect in that implementation, not a stub. The later assertions of the same check (reading the deleted space, its access profile and its dashboard must be refused with AccessDeniedException "Authorization failed") were never reached and are not claimed here; authorization enforcement is tracked by 20261109T172919Z-cloudwatchomni-authorization-not-enforced-accessdeniedexception-never-returned.md.

Traces: cloudwatchomni.deleting-a-space-removes-its-profiles-seeded-grants-and-dashboards.

## Evidence
Check: cloudwatchomni.deleting-a-space-removes-its-profiles-seeded-grants-and-dashboards (verdict: error).

Sequence: CreateDomain(identityProviders=[IAM]) -> CreateSpace(domainId, dataAccessRoleArn) -> CreateAccessProfile(spaceId, name) -> CreateOmniDashboard(spaceId, name, body) -> ListAccessGrants(spaceId) [2 seeded grants, passes] -> DeleteSpace(spaceId) -> ListAccessGrants(spaceId).

Expected (AWS, recorded): the final ListAccessGrants(spaceId=<deleted space>) returns HTTP 200 with items: [] (the check asserts `left == []`).

Actual (emulator): HTTP 404
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListAccessGrants operation: Space '421b175e-b1b6-46a2-9779-f4296a536115' does not exist.

Message anatomy: fixed text is "Space '" ... "' does not exist."; the UUID is the spaceId taken from the request (the value shown is one run's generated id and is not part of the requirement). The whole response is the divergence — AWS returns no error here at all.

Handler source: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/list_access_grants.rs lines 228-240 (spaceId membership check) and lines 215-227 (same for domainId).

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudwatchomni client at it.
2. domain = create_domain(name=<unique>, identityProviders=["IAM"]) ; take domainId.
3. Create an IAM role with a cloudwatch.amazonaws.com trust policy; space = create_space(name=<unique>, domainId=domainId, dataAccessRoleArn=<role arn>) ; take spaceId.
4. create_access_profile(spaceId=spaceId, name=<unique>) and create_omni_dashboard(spaceId=spaceId, name=<unique>, body='{"panels": []}').
5. Paginate list_access_grants(spaceId=spaceId) — two seeded grants are returned (passes).
6. delete_space(spaceId=spaceId).
7. Paginate list_access_grants(spaceId=spaceId) again.
   Observed: ResourceNotFoundException 404, "Space '<spaceId>' does not exist."
   Expected: HTTP 200 with items: [] (and nextToken absent).
