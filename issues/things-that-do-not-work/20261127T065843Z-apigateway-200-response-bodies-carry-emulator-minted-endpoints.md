Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-rest-api-import-export-and-put
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-rest-api-import-export-and-put apigateway-rest-api-import-export-put apigw-envelope-api-keys-position-walk
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetExport|2xx->2xx|$.servers[*].url
Issue-Key: apigateway|GetApiKeys|2xx->2xx|$.position
Issue-Commit: 5616c0fe372bb644c36eb1b1314ae3023677a93c

# apigateway 200 response bodies carry emulator-minted endpoints and page tokens (2 causes)

> _Note: this issue was identified against the code base at commit `5616c0fe372bb644c36eb1b1314ae3023677a93c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
TWO net-new causes on the `apigateway` (REST, BackplaneControlService) wire surface, one shared fault: the HTTP status is right and the body is well-formed, but a body field is filled with a value the emulator minted itself instead of the value AWS reports there — one an endpoint the emulator does not answer on, one a pagination token of the wrong shape. BOTH causes must be closed before this issue is submitted.

Cause 1 of 2 — GetExport's exported API document advertises an execute-api host the emulator does not serve (fixture apigateway-rest-api-import-export-and-put, step task#5; the cluster also covers apigateway-rest-api-import-export-put#task#5).
AWS: Smithy apigateway GetExport — GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, success code 200, output ExportResponse with `body` bound as @httpPayload (blob) plus contentType/contentDisposition headers; errors BadRequestException 400, ConflictException 409, NotFoundException 404, LimitExceededException/TooManyRequestsException 429, UnauthorizedException 401. The model describes `body` only as the exported definition and says nothing about the document's contents, so for the `servers` entry this is RECORDED BEHAVIOR ONLY: the recording shows the exported OAS3 document's servers[0].url is the endpoint at which that stage is actually served, with the base-path server variable appended — `https://<authority that serves this API>/{basePath}`. The fixed parts are the `https://` scheme and the trailing `/{basePath}` variable segment; the authority is taken from where the API is served (one recorded run rendered `https://2usztwj3ww.execute-api.us-west-2.amazonaws.com/{basePath}`, whose api id and region are per-run request/state values and are NOT the requirement). No error code or message is involved; status is 200 on both sides.
Emulator: answers 200 with servers[0].url expanded from a fixed `https://{restApiId}.execute-api.{region}.amazonaws.com/{basePath}` template — a public AWS DNS name this emulator is not reachable on. The replay verdict for `$.servers[0].url` is `host_not_own`: the emulator announced an address other than the one it is reached on (hence a host verdict rather than a plain value mismatch — the two strings are textually alike).
Why: the export is the artifact clients feed to code generators, spec importers, mock servers and API clients. Every client, re-import or generated SDK built from the emulator's export points at the real AWS execute-api domain, so invoke traffic leaves the emulator entirely (or fails to resolve) instead of reaching the emulated stage, and the export -> import round trip never yields a working endpoint.
Fix: the GetExport handler's OpenAPI/Swagger document builder — the `servers[]` (OAS3) / `host`+`basePath` (Swagger 2) renderer — must derive the advertised authority from the endpoint the emulator actually serves this API on (the request's Host header / the configured data listener and its execute-api route) instead of expanding a hard-coded `execute-api.<region>.amazonaws.com` template.
Traces: apigateway-rest-api-import-export-and-put#task#5, apigateway-rest-api-import-export-put#task#5 (GetExport, 200 vs 200, key $.servers[0].url, verdict host_not_own).

Cause 2 of 2 — GetApiKeys returns a 64-character hex digest as the pagination `position` instead of a token of the shape AWS emits (fixture apigw-envelope-api-keys-position-walk, step task#3).
AWS: Smithy apigateway GetApiKeys — GET /apikeys, success code 200, output ApiKeys{warnings, items (jsonName "item"), position}; @paginated inputToken=position, outputToken=position, items=items, pageSize=limit; errors BadRequestException 400, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401. The model documents `position` only as "The current pagination position in the paged result set", so this is RECORDED BEHAVIOR ONLY: the recording shape-asserts the field, and the recorded token is a short opaque API Gateway position of the same shape the service emits for a collection page (an api-key-identifier-shaped lowercase alphanumeric token, ~10 characters), which the client echoes back in the `position` query parameter to fetch the next page. The requirement is the token's shape — a position of the recorded form — not any one run's literal token value.
Emulator: answers 200 with `$.position` = a 64-character lowercase hex SHA-256 digest (this run: 'a739a368ef208730f608cc38266255f9f4250dccbc9490da6faf1d18155b569a'), reported `value_mismatch` against the shape-asserted recording. Status, error code and error message are not involved.
Why: `position` is a public, client-visible continuation token: SDK paginators, CLI `--starting-token`/`NextToken` plumbing and stored cursors carry it and sometimes size- or format-validate it. A client written against AWS that persists or validates the cursor, or that asserts the cursor looks like an API Gateway position, behaves differently against the emulator, and any test pinning cursor shape fails even though the walk itself advances.
Fix: the GetApiKeys handler's pagination-token minter / response serializer for the ApiKeys collection — the `position` emitter — must mint and accept a token of the shape AWS emits for an api-key page (an api-key-identifier-shaped opaque token derived from the last element of the page) rather than serializing a SHA-256 digest of the cursor state.
Traces: apigw-envelope-api-keys-position-walk#task#3 (GetApiKeys, 200 vs 200, key $.position, verdict mismatch).

Related but NOT covering: open issue 20261125T231452Z-apigateway-exported-api-document-and-portal-product-page.md covers the apigatewayv2 ExportApi `servers[].url` cluster (keys service=apigateway|ExportApi|...), not this REST GetExport cluster; the resolved issue 20261110T030315Z-apigateway-api-gateway-getexport-advertises-an-execute-api-host.md covered an earlier form of cause 1 and, being resolved, covers nothing now. Open issues about `apiEndpoint` on CreateApi/ReimportApi (20261125T063702Z, 20261126T153221Z) concern that member, not the exported document. The replay reports both clusters below with stub_path "" and owned_by [].

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true (health 200, data listener accepts), 206 fixtures replayed, 2023 steps (2004 passed, 19 mismatched, 0 errored, 0 blocked), score 99.1%, 10 divergence clusters, bootstrap_signal false. Both clusters below carry stub_path "" and owned_by [] — nothing already owns them.

Cause 1 of 2 — fixture apigateway-rest-api-import-export-and-put, step task#5, service apigateway, operation GetExport, verdict mismatch:
  divergence_key: service=apigateway|GetExport|status=200->200|keys=$.servers[0].url
  status: expected 200, got 200 (status agrees; no error involved)
  $.servers[0].url: host_not_own
    Expected: the exported document advertises the endpoint this API/stage is actually served on — scheme + an authority the emulator answers on + '/{basePath}' (recorded run rendered 'https://2usztwj3ww.execute-api.us-west-2.amazonaws.com/{basePath}'; the api id and region there are per-run request/state values).
    Actual:   'https://2usztwj3ww.execute-api.us-west-2.amazonaws.com/{basePath}' — textually the recorded AWS host, but an address this emulator is not reached on, hence the host_not_own verdict rather than a value mismatch.
  Cluster members (affected_count 2): apigateway-rest-api-import-export-and-put#task#5, apigateway-rest-api-import-export-put#task#5.
  Smithy: GetExport GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, code 200, output ExportResponse{contentType @httpHeader, contentDisposition @httpHeader, body @httpPayload blob}.

Cause 2 of 2 — fixture apigw-envelope-api-keys-position-walk, step task#3, service apigateway, operation GetApiKeys, verdict mismatch:
  divergence_key: service=apigateway|GetApiKeys|status=200->200|keys=$.position
  status: expected 200, got 200 (status agrees)
  $.position: value_mismatch
    Expected: '<shaped-not-asserted>' — a position token of the recorded shape (short opaque api-key-identifier-shaped lowercase alphanumeric token the client echoes back as ?position=).
    Actual:   'a739a368ef208730f608cc38266255f9f4250dccbc9490da6faf1d18155b569a' — a 64-char lowercase hex SHA-256 digest (this run's value; the digest FORM, not this literal, is the defect).
  Cluster members (affected_count 1): apigw-envelope-api-keys-position-walk#task#3.
  Smithy: GetApiKeys GET /apikeys, code 200, output ApiKeys{warnings, items jsonName 'item', position}; @paginated inputToken=position/outputToken=position/items=items/pageSize=limit; position doc 'The current pagination position in the paged result set.'

Per-run values (hosts, ports, generated rest-api ids and api-key ids, timestamps, request ids and this run's digest) are not asserted and are not requirements. The asserted requirements are that the exported document advertises an endpoint the emulator answers on, and that `position` is a token of the shape AWS emits there.

## Steps to Reproduce
Cause 1 of 2 (GetExport, apigateway):
1. git_clone(), then replay_wire_corpus() and look at the cluster service=apigateway|GetExport|status=200->200|keys=$.servers[0].url (fixture apigateway-rest-api-import-export-and-put, step task#5; also apigateway-rest-api-import-export-put#task#5).
2. By hand: ImportRestApi (or CreateRestApi + resources/methods), CreateDeployment onto a stage, then GET /restapis/{restApiId}/stages/{stageName}/exports/oas30?extensions=integrations with Accept: application/json.
3. Parse the returned document and read servers[0].url. Expect an authority the emulator answers on (the host the request arrived on / the listener serving this API's execute-api traffic) followed by '/{basePath}'. Actual: 'https://{restApiId}.execute-api.{region}.amazonaws.com/{basePath}' — resolve or dial that authority and it does not reach the emulator.

Cause 2 of 2 (GetApiKeys, apigateway):
1. Replay fixture apigw-envelope-api-keys-position-walk and stop at step task#3.
2. By hand: CreateApiKey several times, then GET /apikeys?limit=1 and read the response's `position`.
3. Expect a short opaque API Gateway position token of the recorded shape (api-key-identifier-shaped, ~10 lowercase alphanumeric characters). Actual: a 64-character lowercase hex SHA-256 digest, e.g. 'a739a368ef208730f608cc38266255f9f4250dccbc9490da6faf1d18155b569a'.
4. Pass that value back as GET /apikeys?limit=1&position=<token> to confirm the walk still advances — only the token's shape diverges.
