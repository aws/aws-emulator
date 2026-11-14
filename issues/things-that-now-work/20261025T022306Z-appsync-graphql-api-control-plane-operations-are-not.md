Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.access-denied-graphql-api
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.access-denied-graphql-api|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: a577dca75911a15e8d14eba87791fcef54f4bfc0

# AppSync GraphQL-API control-plane operations are not gated by identity-based IAM: a zero-policy principal's GetGraphqlApi/UpdateGraphqlApi/DeleteGraphqlApi/Get+PutGraphqlApiEnvironmentVariables succeed with 200 instead of AccessDeniedException/403

> _Note: this issue was identified against the code base at commit `a577dca75911a15e8d14eba87791fcef54f4bfc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGraphqlApi

## Description
1 cause in this issue; it must be closed before submitting.

=== Cause 1 of 1: no identity-based authorization decision is applied to the AppSync GraphQL-API control plane, so an unauthorized principal is served instead of denied ===

AWS: Smithy (com.amazonaws.appsync#AWSDeepdishControlPlaneService) models AccessDeniedException with errorKind=client and httpError=403 on every operation under test: GetGraphqlApi (GET /v1/apis/{apiId}), UpdateGraphqlApi (POST /v1/apis/{apiId}), DeleteGraphqlApi (DELETE /v1/apis/{apiId}), GetGraphqlApiEnvironmentVariables (GET /v1/apis/{apiId}/environmentVariables) and PutGraphqlApiEnvironmentVariables (PUT /v1/apis/{apiId}/environmentVariables) — each carries the error set {AccessDeniedException 403, BadRequestException 400, NotFoundException 404, UnauthorizedException 401, InternalFailureException 500} (+ ConcurrentModificationException 409 on the mutating three). A SigV4-signed request whose IAM principal has no identity-based policy granting the corresponding appsync:<Action> is refused by implicit default deny with AccessDeniedException / HTTP 403 BEFORE the resource is read or mutated, and the resource and its environment variables are left untouched. The denial message is rule-derived from the request (principal ARN, action, resource ARN), of the documented implicit-deny form: "User: <principal-arn> is not authorized to perform: appsync:<Action> on resource: <resource-arn> because no identity-based policy allows the appsync:<Action> action" — that derivation rule is the requirement, not any one recorded string. The resource in the ARN position is the api ARN for the three api operations and the /v1/apis/<apiId>/environmentVariables resource form for the two environment-variable operations.

Emulator: with credentials of a freshly created IAM user that has ZERO attached policies, the very first denied-principal call, GetGraphqlApi(apiId=<api>), is ACCEPTED: HTTP 200 with a full graphqlApi body (apiId, arn, uris, dns, visibility, apiType, owner, introspectionConfig, ...). No AccessDeniedException is produced, so the check fails at the first probe with "expected the call to be rejected, but it succeeded". The remaining four probes (UpdateGraphqlApi, DeleteGraphqlApi, GetGraphqlApiEnvironmentVariables, PutGraphqlApiEnvironmentVariables) are not reached, but they are served by the same un-gated dispatch path and the mutating ones would additionally mutate state an unauthorized caller must never be able to touch. The positive control in the same check (the same five operations with the runner's own permissioned credentials) succeeds, so the divergence is specifically that the emulator makes no authorization decision about the CALLER — it is not a blanket stub.

Why: this is an overly-permissive security boundary. Any client, test, or CI gate that verifies least privilege against the emulator — "a role without appsync:UpdateGraphqlApi cannot change my API", "an unprivileged principal cannot read or overwrite environment variables" — passes against the emulator while the same call is refused by real AWS, so IAM-policy regressions are invisible. Conversely, code written to catch appsync.exceptions.AccessDeniedException and fall back (assume a role, re-sign, surface a permissions error to the user) never takes that branch, and a destructive DeleteGraphqlApi / PutGraphqlApiEnvironmentVariables from a principal that AWS would have refused actually takes effect, diverging the emulator's state from what AWS would hold.

Note: the open issue 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md describes a missing IAM default-deny gate in its Cause 4, but that cause is grounded only in appsync.access-denied-sandbox-eval (EvaluateCode / EvaluateMappingTemplate, where the scaffold NotImplemented preempts any gate) and does not carry this check's replay key or these five GraphQL-API operations, which ARE implemented and answer 200. It does not cover this divergence.

Fix: the request authorization layer in front of AppSync operation dispatch — resolve the SigV4 principal, evaluate identity-based policy, and refuse with the modeled AccessDeniedException (HTTP 403) and the implicit-deny message built from principal/action/resource before the GetGraphqlApi/UpdateGraphqlApi/DeleteGraphqlApi/GetGraphqlApiEnvironmentVariables/PutGraphqlApiEnvironmentVariables handlers run, so no state is read or written on a denied call.

Traces: appsync.access-denied-graphql-api.

## Evidence
Cause 1 — expected: each of the five calls, made with an access key of an IAM user created with no attached policies, is refused with error code `AccessDeniedException` / HTTP 403 and a message of the implicit-deny form `User: <user-arn> is not authorized to perform: appsync:<Action> on resource: <resource> because no identity-based policy allows the appsync:<Action> action`; afterwards the api still exists and its environmentVariables still hold only what the authorized caller set (PROBE=1, no FOO).

Actual (appsync.access-denied-graphql-api, check_appsync_access_denied_graphql_api.py line 101 -> _error line 28): `AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'graphqlApi': {'name': 'sim2real-behav-appsync-api-...', 'apiId': '...', 'authenticationType': 'API_KEY', 'arn': 'arn:aws:appsync:us-west-2:<acct>:apis/...', 'uris': {...}, 'tags': {}, 'xrayEnabled': False, 'dns': {...}, 'visibility': 'GLOBAL', 'apiType': 'GRAPHQL', 'owner': '<acct>', 'introspectionConfig': 'ENABLED', 'queryDepthLimit': 0, 'resolverCountLimit': 0}}` — i.e. HTTP 200 with the full api body for the unauthorized principal instead of a 403. The identical five calls with the runner's permissioned credentials (the check's positive control, including PutGraphqlApiEnvironmentVariables taking effect) all succeeded, so the operations are implemented and only the caller-authorization decision is absent.

Smithy citation used: GetGraphqlApi / UpdateGraphqlApi / DeleteGraphqlApi / GetGraphqlApiEnvironmentVariables / PutGraphqlApiEnvironmentVariables each list `AccessDeniedException` (errorKind client, httpError 403) in their error sets.

## Steps to Reproduce
1. With permissioned credentials: appsync CreateGraphqlApi(name=<unique>, authenticationType="API_KEY") -> note apiId and arn.
2. Confirm the positive control: GetGraphqlApi(apiId), UpdateGraphqlApi(apiId, name, authenticationType="API_KEY"), PutGraphqlApiEnvironmentVariables(apiId, environmentVariables={"PROBE":"1"}), GetGraphqlApiEnvironmentVariables(apiId) all succeed and PROBE=1 is readable.
3. iam CreateUser(<unique>) with NO policies attached, then iam CreateAccessKey for that user; build an appsync client from that key.
4. With the zero-policy client call, in turn: GetGraphqlApi(apiId=<api>), UpdateGraphqlApi(apiId=<api>, name=<same>, authenticationType="API_KEY"), DeleteGraphqlApi(apiId=<api>), GetGraphqlApiEnvironmentVariables(apiId=<api>), PutGraphqlApiEnvironmentVariables(apiId=<api>, environmentVariables={"FOO":"bar"}).
5. Observe: the first call returns HTTP 200 with the graphqlApi body instead of AccessDeniedException/403. Expected every one of the five to be refused with AccessDeniedException/403 carrying the implicit-deny message for its action and resource, and the api plus its environment variables (PROBE=1, no FOO) to be unchanged afterwards.
