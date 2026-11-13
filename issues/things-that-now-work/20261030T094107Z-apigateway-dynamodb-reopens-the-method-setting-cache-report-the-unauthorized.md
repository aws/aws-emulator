Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies
Issue-Key: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies|_wait_for_status#13|Timeout|time.sleep(3)
Issue-Commit: db763654427c553d8b06d764576848aa87eee804

# REOPENS the method-setting cache report: the unauthorized Cache-Control strategy check still reaches no verdict — the stage's AWS-type DynamoDB integration is never performed, so no cached entry ever exists

> _Note: this issue was identified against the code base at commit `db763654427c553d8b06d764576848aa87eee804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
REOPENS 20261023T210711Z-apigateway-dynamodb-all-three-aws-integration-method-setting.md: its fix left the call refused.

That report's Resolution names this exact check (`…methodsetting-unauthorized-cache-control-strategies`) as fixed by the new stage response cache in `src/service/invoke.rs`. It still fails, and it still fails the same way the report described: nothing ever comes back 200 from the deployed stage, so the check burns its whole budget and is reported as `Check timed out after 60.0s`. Its sibling trace `…methodsetting-cache-ttl-expiry-and-flush-eviction` now passes, so the cache machinery itself landed; what did not land is the integration the cached response is supposed to come FROM. The open report 20261030T120000Z-apigateway-aws-service-integration-to-another-services-action-is-not-performed.md describes that missing capability but carries only the `apigw-aws-integration-mapping-template-writes-ddb` trace and replay key — this check's key is carried by no open issue, which is why it is filed here.

Cause 1 of 1: an `AWS`-type (non-proxy) integration naming another service's action is not performed at a deployed stage, so the unauthorized-Cache-Control method-setting surface behind it is unreachable.
Failing check id: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies

AWS: a deployed REST stage whose `GET` method carries `Integration$type = AWS`, `uri = arn:aws:apigateway:{region}:dynamodb:action/UpdateItem`, `credentials` = an integration role and an `application/json` entry in `Integration$requestTemplates` evaluates that template, calls DynamoDB `UpdateItem`, and answers with the integration response the mapped template produces (200 here, body `{"n": …, "rid": …}`). With the stage's cache cluster up (Smithy: `Stage$cacheClusterEnabled`, `Stage$cacheClusterSize` -> enum `CacheClusterSize`, `Stage$cacheClusterStatus` -> enum `CacheClusterStatus`, reaching `AVAILABLE`) and the method's settings enabling caching (Smithy: `MethodSetting$cachingEnabled`, `MethodSetting$cacheTtlInSeconds`), AWS serves the held body on later requests with the same cache key. An unauthorized `Cache-Control: max-age=0` is then handled per the method's settings — Smithy: `MethodSetting$requireAuthorizationForCacheControl` (Boolean, "Specifies whether authorization is required for a cache invalidation request") and `MethodSetting$unauthorizedCacheControlHeaderStrategy` -> enum `UnauthorizedCacheControlHeaderStrategy` with values `FAIL_WITH_403`, `SUCCEED_WITH_RESPONSE_HEADER`, `SUCCEED_WITHOUT_RESPONSE_HEADER`. Recorded behavior only for the per-case wire detail, which the recording shows as: `FAIL_WITH_403` -> HTTP 403 carrying header `x-amzn-ErrorType: MissingAuthenticationTokenException` and NO `Warning` header, with the integration not invoked; `SUCCEED_WITH_RESPONSE_HEADER` -> HTTP 200 serving the cached body plus header `Warning: 199 Cache-control headers were ignored because the caller was unauthorized.` (this is fixed text in its entirety — the only variable part of the header line is the `199` warn-code prefix AWS always emits for it, nothing is taken from the request); `SUCCEED_WITHOUT_RESPONSE_HEADER` -> HTTP 200 serving the cached body with no `Warning` header. With `requireAuthorizationForCacheControl` patched to `false`, the same header is honoured: the integration runs exactly once, its fresh body is returned, and the immediately following plain GET is a hit on that refreshed entry. GetStage must also read back the server-side defaults the recording shows for a caching-enabled method setting: `requireAuthorizationForCacheControl = true` and `unauthorizedCacheControlHeaderStrategy = SUCCEED_WITH_RESPONSE_HEADER`.

Emulator: the check reaches no verdict at all — the replay detail is exactly `Error: Check timed out after 60.0s`. It dies in its FIRST wait, `_wait_for_status(url, 200, timeout_s=180)`, which polls the deployed stage's invoke URL for the backend's first 200 before any caching is in play. Per the open companion report, that request is answered with the api's `API_CONFIGURATION_ERROR` gateway response, `500 {"message":"Internal server error"}`, because `perform_integration` serves `MOCK` from the records, passes an `HTTP_PROXY` through, and refuses every other integration type rather than performing it — no mapping template is evaluated and no DynamoDB `UpdateItem` is issued. Consequently the counter item is never written, there is never a cached body to serve, and none of the three `unauthorizedCacheControlHeaderStrategy` branches, the `Warning: 199 Cache-control headers were ignored because the caller was unauthorized.` header, the `403` + `x-amzn-ErrorType: MissingAuthenticationTokenException` refusal, nor the `requireAuthorizationForCacheControl=false` invalidation path is ever exercised.

Why: a client that wires API Gateway to another AWS service with no compute in between — the whole point of a non-proxy `AWS` integration — gets `500 {"message":"Internal server error"}` from a stage AWS answers 200 on, and its backend is never called. Every cache-control semantic layered on top (403 vs 200+Warning vs silent, and authorized invalidation) is silently unobservable rather than wrong, so a client cannot distinguish "caching behaved" from "nothing ran", and a caller that is supposed to be refused 403 is instead refused 500 with the wrong error type.

Fix: the `execute-api` data-plane request path of the apigateway crate — integration dispatch for `Integration$type = AWS` must perform the service action the `uri` names (evaluating `requestTemplates` and mapping the reply through the integration response) so a response other than `API_CONFIGURATION_ERROR` exists; the already-landed stage response cache must then hold THAT response and apply the method's `requireAuthorizationForCacheControl` / `unauthorizedCacheControlHeaderStrategy` to it, and `GetStage` must read back the two server-side defaults for a caching-enabled method setting.

Traces: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies (file and line citations for the refusing branch are in the companion report 20261030T120000Z-apigateway-aws-service-integration-to-another-services-action-is-not-performed.md, which names `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`).

## Evidence
Replay report, failing check `apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies`, verdict `mismatch`, detail verbatim:

    Error: Check timed out after 60.0s

No HTTP status line and no body from the deployed stage is ever recorded, so the check never reaches its first assertion (`assert status == 200, f"backend never answered 200 before caching: {status} {body!r}"`).

Expected (AWS):
- GET {invoke-url}/count on the deployed stage -> 200, body `{"n": <counter>, "rid": <request id>}`, and the DynamoDB counter item incremented by the `UpdateItem` the AWS integration performs.
- After the 0.5 GB cache cluster reaches `cacheClusterStatus = AVAILABLE` and the method setting enables caching, GetStage reads back `requireAuthorizationForCacheControl = true` and `unauthorizedCacheControlHeaderStrategy = SUCCEED_WITH_RESPONSE_HEADER`.
- `Cache-Control: max-age=0` from an unauthorized caller, per strategy: `FAIL_WITH_403` -> 403 with `x-amzn-ErrorType: MissingAuthenticationTokenException`, no `Warning`, backend not invoked; `SUCCEED_WITH_RESPONSE_HEADER` -> 200 with the cached body and `Warning: 199 Cache-control headers were ignored because the caller was unauthorized.`; `SUCCEED_WITHOUT_RESPONSE_HEADER` -> 200 with the cached body and no `Warning`.
- With `requireAuthorizationForCacheControl = false`: the same request invokes the backend exactly once, returns the new counter, and the next plain GET is a hit on that refreshed value.

Actual (emulator): the stage never answers 200. Per the open companion report for the same integration shape, it answers the api's `API_CONFIGURATION_ERROR` gateway response `500 {"message":"Internal server error"}` and performs no DynamoDB call, so the check polls to its budget and is killed at 60 s with no verdict on any of the above.

The fixed report 20261023T210711Z-apigateway-dynamodb-all-three-aws-integration-method-setting.md lists this check in its `Issue-Trace-Ids` and claims it resolved in its Resolution section; the check still fails with the identical detail.

## Steps to Reproduce
1. Create a DynamoDB table with key schema `pk` (S), `BillingMode=PAY_PER_REQUEST`, and wait for ACTIVE.
2. Create an IAM role trusted by `apigateway.amazonaws.com` with `dynamodb:UpdateItem` on that table.
3. Create a REST API with resource `/count` and a `GET` method (`authorizationType=NONE`) whose integration is `type=AWS`, `integrationHttpMethod=POST`, `uri=arn:aws:apigateway:{region}:dynamodb:action/UpdateItem`, `credentials=<role arn>`, `passthroughBehavior=NEVER`, and an `application/json` request template performing `ADD n :one` with `ReturnValues=UPDATED_NEW`; add a 200 method response and an integration response whose template maps `$input.path('$.Attributes.n.N')` to `n`.
4. CreateDeployment, then CreateStage with `cacheClusterEnabled=true`, `cacheClusterSize=0.5`.
5. GET `{invoke-url}/count`: the stage answers `500 {"message":"Internal server error"}` instead of `200 {"n":1,…}`, and the table's counter item is never written — so the check hangs in its first readiness wait and is killed at the 60 s cap before any cache-control strategy is probed.
6. Or replay the check id in trace_ids.
