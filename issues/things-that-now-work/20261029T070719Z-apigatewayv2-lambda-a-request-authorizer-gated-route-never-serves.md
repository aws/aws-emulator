Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value|_poll_http#21|TimeoutError|raise TimeoutError(f"{description} ({url}): the run budget ended first; not graded")
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# apigatewayv2, lambda: an HTTP-API route gated by a REQUEST authorizer never serves, so the authorizer result cache cannot be keyed at all (1 cause)

> _Carried over from `20261029T070719Z-apigatewayv2-3-net-new-root-causes-the-10.md`, whose other two causes (the 10 MiB payload gate and the absent `HostedZoneId`) are implemented. This cause is NOT fixed and keeps that bundle's third replay key, so a later scan does not lose it. The code base can have drifted since the commit above._

## Operation
Invoke (execute-api HTTP data plane, CUSTOM/REQUEST authorizer + AWS_PROXY integration)

## Description
Cause 1 of 1: an HTTP-API route protected by a `REQUEST` (Lambda) authorizer never serves.

AWS:      For an HTTP API route protected by a `REQUEST` authorizer with an `IdentitySource` and `AuthorizerResultTtlInSeconds` greater than zero, API Gateway invokes the authorizer function, then caches the decision keyed on the concatenated identity-source VALUES. Two requests inside the TTL carrying the same identity-source value invoke the authorizer exactly once; a request carrying a different identity-source value is a cache miss and invokes it again. Every allowed request is answered by the backend integration with its own status (200 here). Recorded behavior only for the cache-keying rule — Smithy models `Authorizer$IdentitySource` and `Authorizer$AuthorizerResultTtlInSeconds` but not the keying semantics; the requirement is the recorded one-invocation-per-distinct-identity-source-value count and the 200 the backend returns.

Emulator: the authorizer-protected route never answers 200. Past the identity-source precondition (which IS applied — an absent source is `401` and the function is not invoked), the data plane answers `500 {"message":"Internal Server Error"}`, because deciding the route needs the authorizer function's own answer: `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs::authorization_refusal`. Polling the route for a served status exhausts the run budget.

Why:      A client behind a `REQUEST` authorizer gets a 5xx rather than 200 or 401/403, so authorization cannot be observed at all, and nothing that depends on the authorizer cache (invocation counts, per-identity isolation, TTL behaviour) can be relied on.

Blocker:  Two Lambda deployment packages have to RUN for this check to be served — the authorizer's and the route's backend — and this emulator has no in-process runtime for one: `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` answers HTTP 501 for `RequestResponse`, and the recorded functions are `python3.12` inline zips, for which no supported in-process engine exists in this project. An ALLOW decision or a backend body invented here would be indistinguishable from the function's own, so the data plane refuses instead. This cause therefore stays open behind the Lambda runtime work: `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`, `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`, and, for the dispatch that maps a function's answer back onto the HTTP response, `20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`.

Fix:      The execute-api data-plane authorizer pipeline: invoke the `REQUEST` authorizer, cache its decision keyed on the identity-source values for `AuthorizerResultTtlInSeconds`, then dispatch to the route's integration.

Traces:   apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value. The open issue `20261025T174820Z-apigatewayv2-an-http-api-route-gated-by-a.md` concerns authorizer-cache TTL expiry and does not carry this check's replay key.

## Evidence
Expected: GET of the authorizer-protected route returns 200 from the backend, with the authorizer invoked once per distinct identity-source value within the TTL.
Actual: no 200 is ever returned — `TimeoutError: the authorizer-protected route serving its backend (https://127.0.0.1:<port>/_aws/execute-api/<apiId>/$default/guarded): the run budget ended first; not graded` (check_apigatewayv2_request_authorizer_cache_is_keyed_per_identity_source_value.py line 139). The host, port and api id there are per-run values; the invariant is that no served status is ever returned.

## Steps to Reproduce
Create an HTTP API with a `REQUEST` Lambda authorizer (`IdentitySource` set, `AuthorizerResultTtlInSeconds` > 0) on a route, deploy a stage, then GET the guarded route twice with the same identity-source value and once with a different value. Replay check apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value.
