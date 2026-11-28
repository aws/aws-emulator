Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-export-import-reimport-roundtrip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-export-import-reimport-roundtrip apigatewayv2-import-export-reimport-api-round-trip apigatewayv2-product-rest-endpoint-page-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ExportApi|2xx->2xx|$.servers[*].url
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.statusException.exception
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.statusException.message
Issue-Commit: 1a552fe8708f5d6fbecf5907a9407a2fce831c57

# apigateway exported API document and portal-product-page statusException carry values no client can use

> _Note: this issue was identified against the code base at commit `1a552fe8708f5d6fbecf5907a9407a2fce831c57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportApi

## Description
TWO net-new causes on the `apigateway` surface, one shared fault: a 200 response body is well-formed and the status agrees, but a body field is filled from something other than what AWS reports there — one with an endpoint the emulator does not answer on, one with an unrelated internal lookup error. BOTH causes must be closed before this issue is submitted.

Cause 1 of 2 — ExportApi's exported OpenAPI document advertises an execute-api host the emulator does not serve (fixture apigatewayv2-export-import-reimport-roundtrip, step task#3; cluster also covers apigatewayv2-import-export-reimport-api-round-trip#task#4).
AWS: Smithy apigatewayv2 ExportApi — GET /v2/apis/{ApiId}/exports/{Specification}, success code 200, output is the single `ExportedApi` blob bound as @httpPayload (members: ApiId @httpLabel required, Specification @httpLabel required, OutputType required, ExportVersion/IncludeExtensions/StageName @httpQuery; errors BadRequestException 400, NotFoundException 404, TooManyRequestsException 429). The model describes the payload only as "an exported definition of an API in a particular output format" and says nothing about the document's contents, so for the `servers` entry this is RECORDED BEHAVIOR ONLY: the recording shows the exported OAS3 document's `servers[0].url` is the endpoint at which that API is actually served, with the base-path server variable appended — recorded as `https://<host serving the API>/{basePath}` (one run: `https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com/{basePath}`; the api id and region are values from the request and state, the `https://` scheme and the `/{basePath}` variable segment are the fixed parts). The requirement is that the advertised endpoint is one the serving host answers on — the concrete host string of any single run is not itself the requirement.
Emulator: answers 200 with `servers[0].url` rendered from a hard-coded `https://{apiId}.execute-api.{region}.amazonaws.com/{basePath}` template, i.e. a public AWS DNS name that this emulator is NOT reachable on. The replay verdict for `$.servers[0].url` is `host_not_own`: the emulator announced an address other than the one it is reached on. No error code and no error message are involved; only this field diverges.
Why: the export is the artifact clients feed to code generators, spec importers, mock servers and API clients. Every client, re-import or generated SDK built from the emulator's export points at the real AWS execute-api domain, so invoke traffic leaves the emulator entirely (or fails to resolve) instead of reaching the emulated API, and the round trip export -> import never yields a working endpoint.
Fix: the ExportApi handler's OpenAPI-document builder — the `servers[]` renderer — must derive the advertised authority from the endpoint the emulator actually serves this API on (the request's Host / the configured data listener and its execute-api route), instead of expanding a fixed `execute-api.<region>.amazonaws.com` template.
Traces: apigatewayv2-export-import-reimport-roundtrip#task#3, apigatewayv2-import-export-reimport-api-round-trip#task#4 (ExportApi, 200 vs 200, key $.servers[0].url, verdict host_not_own).

Cause 2 of 2 — GetProductRestEndpointPage's statusException reports an internal API-lookup failure instead of the page's own failure (fixture apigatewayv2-product-rest-endpoint-page-lifecycle, steps task#7 and task#9).
AWS: Smithy apigatewayv2 GetProductRestEndpointPage — GET /v2/portalproducts/{PortalProductId}/productrestendpointpages/{ProductRestEndpointPageId}, success code 200; output members include `status` (enum Status = AVAILABLE | IN_PROGRESS | FAILED), `restEndpointIdentifier`, and `statusException` (structure StatusException, documented "Represents a StatusException" / member doc "The status exception information", with `exception` -> __stringMin1Max256 "The exception" and `message` -> __stringMin1Max2048 "The error message"). For the CONTENT of those two members this is RECORDED BEHAVIOR ONLY — the fixture shape-asserts them rather than pinning literal text — and the recording shows a 200 whose statusException describes why THIS product REST endpoint page failed, i.e. the failed validation of the page's own `restEndpointIdentifier`: `exception` names the exception class of that page-level failure and `message` is its sentence. The emulator's values do not match that recorded shape.
Emulator: answers 200 but fills the page's statusException with the apigateway control-plane API-lookup error verbatim — `$.statusException.exception` = "NotFoundException" and `$.statusException.message` = "Invalid API identifier specified 000000000000:mp67t4dbd4". In that message the fixed text is "Invalid API identifier specified " and the variable part is "<accountId>:<apiId>" taken from the emulator's account and the request/state (`000000000000` and `mp67t4dbd4` are this run's values, not part of the requirement). Both members are reported `value_mismatch` against the shape-asserted recording, on both reads of the page.
Why: statusException is the only thing a client has to explain a non-AVAILABLE page. Instead of the page's own validation failure it receives a generic wire error code plus a sentence about a REST *API identifier* that the caller never supplied, prefixed with the emulator's own account id. A consumer that surfaces statusException.message to a user, or branches on exception to retry/repair the page's restEndpointIdentifier, is told the wrong thing about the wrong object, and the leaked account id is emulator internal state.
Hypothesis: the page's status evaluation calls the shared api-lookup helper and, on failure, copies that helper's error code and message straight into StatusException rather than mapping it to the page-level exception AWS reports.
Fix: the portal-product REST-endpoint-page state/handler — where the page's FAILED status is computed from its stored `restEndpointIdentifier` — must build StatusException from the page-level validation failure (exception class and sentence describing the unresolvable endpoint reference for that page) rather than propagating the internal apigateway "Invalid API identifier specified <accountId>:<apiId>" NotFoundException into the response body.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#7 and #task#9 (GetProductRestEndpointPage, 200 vs 200, keys $.statusException.exception, $.statusException.message).

Related but NOT covering: the RESOLVED issue 20261124T190644Z-apigateway-export-and-portal-product-page-responses-report.md covered the earlier form of both paths (ExportApi emitting a loopback `host:port`+`/_aws/execute-api/<id>` path URL, and GetProductRestEndpointPage reporting status AVAILABLE with no statusException at all). Both of those earlier symptoms are gone — the status now agrees and the export no longer path-styles the api id — but the fix swung the export to a public AWS host the emulator does not answer on, and populated statusException with the wrong error. That issue is resolved and therefore covers neither cause; the replay reports both clusters with empty owned_by and empty stub_path. Open issue 20261125T063702Z-apigateway-createapi-and-reimportapi-publish-apiendpoint-as-a.md covers the `apiEndpoint` MEMBER on CreateApi/ReimportApi and explicitly excludes this ExportApi cluster. The same host_not_own fault in the REST GetExport export document was reported in the resolved 20261110T030315Z-apigateway-api-gateway-getexport-advertises-an-execute-api-host.md.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, 1057 steps (932 passed, 124 mismatched, 1 errored), 55 divergence clusters, score 88.2%. Both clusters below carry stub_path "" and owned_by [].

Cause 1 of 2 — fixture apigatewayv2-export-import-reimport-roundtrip, step task#3, service apigateway, operation ExportApi, verdict mismatch:
  divergence_key: service=apigateway|ExportApi|status=200->200|keys=$.servers[0].url
  status: expected 200, got 200 (status agrees; no error involved)
  $.servers[0].url: host_not_own
    Expected: the exported document advertises the endpoint the API is actually served on, scheme+authority+'/{basePath}' (recorded run: 'https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com/{basePath}' — api id and region are per-run request/state values).
    Actual:   'https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com/{basePath}' — textually the recorded AWS host, but a host this emulator does not answer on, hence the host_not_own verdict rather than a value mismatch.
  Cluster members: apigatewayv2-export-import-reimport-roundtrip#task#3, apigatewayv2-import-export-reimport-api-round-trip#task#4 (affected_count 2).

Cause 2 of 2 — fixture apigatewayv2-product-rest-endpoint-page-lifecycle, step task#7, service apigateway, operation GetProductRestEndpointPage, verdict mismatch:
  divergence_key: service=apigateway|GetProductRestEndpointPage|status=200->200|keys=$.statusException.exception,$.statusException.message
  status: expected 200, got 200 (status agrees)
  $.statusException.exception: value_mismatch (expected: shape-asserted exception class of the page's own failure; got 'NotFoundException')
  $.statusException.message:   value_mismatch (expected: shape-asserted sentence describing the page's failed restEndpointIdentifier validation; got 'Invalid API identifier specified 000000000000:mp67t4dbd4' — fixed text 'Invalid API identifier specified ', variable part '<accountId>:<apiId>')
  Smithy (apigatewayv2): Status enum = AVAILABLE | IN_PROGRESS | FAILED; StatusException{exception @length 1..256, message @length 1..2048}; GetProductRestEndpointPage GET /v2/portalproducts/{PortalProductId}/productrestendpointpages/{ProductRestEndpointPageId}, code 200.
  Cluster members: apigatewayv2-product-rest-endpoint-page-lifecycle#task#7 and #task#9 (affected_count 2).

Per-run values (hosts, ports, generated api ids, the 000000000000 account id, timestamps and request ids) are not asserted and are not requirements; the asserted requirements are that the exported endpoint is one the emulator answers on, and that statusException describes the page's own failure.

## Steps to Reproduce
Cause 1 of 2 (ExportApi, apigateway):
1. git_clone(), then replay_wire_corpus() and look at the cluster service=apigateway|ExportApi|status=200->200|keys=$.servers[0].url.
2. By hand: create/import an HTTP API, then GET /v2/apis/{apiId}/exports/OAS30?outputType=JSON.
3. Parse the returned document and read servers[0].url. Expect an authority the emulator answers on (the host the request arrived on / the listener that serves this API's execute-api traffic) followed by '/{basePath}'. Actual: 'https://{apiId}.execute-api.{region}.amazonaws.com/{basePath}' — resolve or dial that authority and it does not reach the emulator.
4. Fixtures: apigatewayv2-export-import-reimport-roundtrip step task#3, apigatewayv2-import-export-reimport-api-round-trip step task#4.

Cause 2 of 2 (GetProductRestEndpointPage, apigateway):
1. Replay fixture apigatewayv2-product-rest-endpoint-page-lifecycle and stop at step task#7 (and task#9).
2. By hand: create a portal product, then CreateProductRestEndpointPage with a restEndpointIdentifier that does not resolve, then GET /v2/portalproducts/{portalProductId}/productrestendpointpages/{productRestEndpointPageId}.
3. Read the 200 response's statusException. Expect exception/message describing the page's own failed endpoint-reference validation. Actual: exception 'NotFoundException' and message 'Invalid API identifier specified <accountId>:<apiId>' — the internal apigateway api-lookup error, with the emulator's account id interpolated.
