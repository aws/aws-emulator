Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-gateway-response-template-substitutes-context-and-stage-variables apigateway.apigw-gateway-response-headers-map-request-parameters-and-stage-variables apigateway.mock-request-template-context-identity-user-agent-selects-status apigateway.mapping-template-context-identity-renders-the-signed-caller apigateway.mock-request-template-routes-on-context-request-id-client-supplied-or-minted apigateway.request-template-context-authorizer-reads-the-lambda-authorizer-context apigateway.apigw-method-context-query-parameter-rendering-by-occurrence-count apigateway.response-template-foreach-past-1000-iterations-rejected apigateway.response-template-foreach-cap-applies-to-each-loop apigateway.request-template-foreach-folds-a-body-array-into-the-mock-status apigateway.request-template-foreach-stops-after-one-thousand-iterations apigateway.mock-request-template-escape-javascript-escapes-quotes-backslash-and-slash apigateway.mock-request-template-parse-json-routes-status-and-unmapped-content-type-rejected apigateway.response-template-parse-json-reads-nested-fields-of-a-header-document apigateway.request-template-base64-decoded-pass-header-mismatch-rejected-with-403 apigateway.response-template-input-path-reads-nested-members-of-the-integration-body apigateway.integration-response-template-input-json-reserializes-backend-json apigateway.request-template-input-path-on-truncated-json-rejected-400
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-gateway-response-template-substitutes-context-and-stage-variables|run#12|AssertionError|assert got == want, f"expected {want!r}, got {got!r}"
Issue-Key: apigateway.apigw-gateway-response-headers-map-request-parameters-and-stage-variables|run#11|AssertionError|assert got == want, f"expected gateway-response headers {want!r}, got {got!r}"
Issue-Key: apigateway.mock-request-template-context-identity-user-agent-selects-status|run#9|AssertionError|assert (status, json.loads(text)) == (200, {"seen": "other"}), (
Issue-Key: apigateway.mapping-template-context-identity-renders-the-signed-caller|_until_served#19|Timeout|time.sleep(2)
Issue-Key: apigateway.mock-request-template-routes-on-context-request-id-client-supplied-or-minted|run#20|AssertionError|assert status == 200, (
Issue-Key: apigateway.request-template-context-authorizer-reads-the-lambda-authorizer-context|run#29|AssertionError|assert got == (202, "gold-context"), f"the gold caller's context rendered {got!r}"
Issue-Key: apigateway.apigw-method-context-query-parameter-rendering-by-occurrence-count|_poll#15|TimeoutError|raise TimeoutError(f"run budget ended before GET {url} answered 200; last={last!r}")
Issue-Key: apigateway.response-template-foreach-past-1000-iterations-rejected|run#7|AssertionError|assert (status, body) == (200, '{"passes": 1000}'), (
Issue-Key: apigateway.response-template-foreach-cap-applies-to-each-loop|run#12|AssertionError|assert answer["status"] == 200, (
Issue-Key: apigateway.request-template-foreach-folds-a-body-array-into-the-mock-status|run#15|Timeout|time.sleep(2)
Issue-Key: apigateway.request-template-foreach-stops-after-one-thousand-iterations|run#15|Timeout|time.sleep(2)
Issue-Key: apigateway.mock-request-template-escape-javascript-escapes-quotes-backslash-and-slash|run#16|Timeout|time.sleep(2.0)
Issue-Key: apigateway.mock-request-template-parse-json-routes-status-and-unmapped-content-type-rejected|_first_live#16|Timeout|time.sleep(2.0)
Issue-Key: apigateway.response-template-parse-json-reads-nested-fields-of-a-header-document|_until_served#18|Timeout|time.sleep(2.0)
Issue-Key: apigateway.request-template-base64-decoded-pass-header-mismatch-rejected-with-403|run#22|AssertionError|assert unencoded == (403, REFUSAL), (
Issue-Key: apigateway.response-template-input-path-reads-nested-members-of-the-integration-body|run#21|Timeout|time.sleep(2)
Issue-Key: apigateway.integration-response-template-input-json-reserializes-backend-json|run#12|JSONDecodeError|view = json.loads(text)
Issue-Key: apigateway.request-template-input-path-on-truncated-json-rejected-400|run#12|AssertionError|assert (status, head.get("x-amzn-errortype"), text) == (
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway: 10 net-new VTL mapping-template causes — $context/$stageVariables are not substituted, #foreach and the $util/$input helpers are unsupported, and a template failure answers 500 instead of 400

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
The emulator's VTL mapping-template engine implements only a small subset of the Velocity surface API Gateway supports. Ten independent causes, all observed through a deployed REST stage (MOCK, AWS_PROXY and AWS integrations). Where a template uses an unsupported construct, the stage answers 500 `{"message":"Internal server error"}` instead of the template's result, or passes the `$...` reference through as literal text.

Cause 1 of 10: GATEWAY RESPONSE responseTemplates do not substitute $context.* or $stageVariables.*.
AWS: recorded behavior only (the model's PutGatewayResponse.responseTemplates is MapOfStringToString, with no modelled template semantics). A gateway response template is rendered with the same VTL context as a mapping template, so `$context.stage`, `$context.path`, `$context.apiId` and `$stageVariables.<name>` are replaced with the stage name, the request path, the API id and the stage variable's value; `$context.error.*` already renders.
Emulator: the rendered body contains the references verbatim — `{'stage': '$context.stage', 'tier': '$stageVariables.tier', 'path': '$context.path', 'api': '$context.apiId', 'type': 'INVALID_API_KEY'}` — only the `$context.error.*` member rendered.
Why: a client reading a customized gateway-response body gets literal `$context.*` text where AWS puts the stage/API/path, so error payloads are unusable.
Fix: the gateway-response rendering path — it must use the same VTL context binding as integration request/response templates.
Traces: apigateway.apigw-gateway-response-template-substitutes-context-and-stage-variables

Cause 2 of 10: GATEWAY RESPONSE responseParameters are not mapped into response headers.
AWS: recorded behavior only. A gateway response's responseParameters (`gatewayresponse.header.<name>` sourced from `method.request.querystring.<q>`, `method.request.header.<h>`, `stageVariables.<v>` or a `'literal'`) are emitted as response headers on the gateway response.
Emulator: only the static literal header is emitted; every header sourced from a request parameter or a stage variable is absent — got `{'access-control-allow-origin': None, 'x-query': None, 'x-tier': None, 'x-static': 'fixed'}` where all four must carry values.
Why: CORS and diagnostic headers configured on gateway responses never reach the client, so a browser request that trips a gateway response fails its CORS check.
Fix: the gateway-response serializer — apply responseParameters mapping before writing headers.
Traces: apigateway.apigw-gateway-response-headers-map-request-parameters-and-stage-variables

Cause 3 of 10: $context.identity.* is not bound.
AWS: recorded behavior only. In a mapping template `$context.identity.userAgent`, `.sourceIp`, `.caller`, `.user`, `.userArn` and `.accountId` render the values API Gateway derived from the request and its SigV4 credentials, so a template can branch on them.
Emulator: a template that reads `$context.identity.userAgent` makes the stage answer 500 `{"message":"Internal server error"}` even on the default branch, and the signed-caller identity rendering check reaches no verdict inside its budget.
Why: any template that logs or authorizes on the caller identity turns every request into a 500.
Fix: the VTL context builder — populate the `identity` map of $context.
Traces: apigateway.mock-request-template-context-identity-user-agent-selects-status, apigateway.mapping-template-context-identity-renders-the-signed-caller

Cause 4 of 10: $context.requestId is not bound.
AWS: recorded behavior only. `$context.requestId` renders the request id API Gateway assigned — the client-supplied `x-amzn-RequestId`/trace value when one is sent, otherwise a freshly minted lowercase UUID. (The id value itself is per-request and is not part of the requirement; that it is a bound, lowercase-UUID-shaped value is.)
Emulator: with no client-supplied id the template renders nothing and the stage answers 500; the template's branch never sees a value.
Fix: the VTL context builder — bind requestId.
Traces: apigateway.mock-request-template-routes-on-context-request-id-client-supplied-or-minted

Cause 5 of 10: $context.authorizer.* is not populated from a Lambda authorizer's context.
AWS: recorded behavior only. The key/value pairs a TOKEN/REQUEST authorizer returns in its policy's `context` are exposed to the integration request template as `$context.authorizer.<key>`.
Emulator: the template takes its no-match branch — the stage answered `(200, 'context-unmatched')` for a caller whose authorizer returned the matching context — so `$context.authorizer.*` is empty.
Why: an integration that passes authorizer claims (tenant, tier, principal) to the backend silently sends nothing.
Fix: the VTL context builder — carry the authorizer context forward from the authorizer result into $context.authorizer.
Traces: apigateway.request-template-context-authorizer-reads-the-lambda-authorizer-context

Cause 6 of 10: $input.params() rendering of a repeated query parameter fails.
AWS: recorded behavior only. For `?tag=first&tag=second&tag=third`, `$input.params('tag')` renders the first occurrence and `$input.params().querystring` / `$method.request.multivaluequerystring` expose all occurrences, so a template can render by occurrence count.
Emulator: the stage never answers 200 — the last answer was 500 `{"message":"Internal server error"}` for `GET .../render?tag=first&tag=second&tag=third`.
Fix: the VTL `$input.params` implementation (multi-valued query parameters).
Traces: apigateway.apigw-method-context-query-parameter-rendering-by-occurrence-count

Cause 7 of 10: #foreach is unsupported in request and response templates, including its 1000-iteration cap.
AWS: recorded behavior only. `#foreach` over a JSON array from `$input.path(...)` is evaluated in both integration request and integration response templates; a loop is capped at 1000 iterations per loop (two sequential 1000-pass loops both run and the response is 200; a loop asked to run past 1000 passes is refused rather than answered 200 with a truncated body).
Emulator: a response template with a single 1000-pass loop, and one with two 1000-pass loops, both answer 500 `{"message":"Internal server error"}` instead of 200; the two request-template #foreach checks reach no verdict inside their budget.
Fix: the VTL interpreter — implement #foreach and its per-loop iteration cap.
Traces: apigateway.response-template-foreach-past-1000-iterations-rejected, apigateway.response-template-foreach-cap-applies-to-each-loop, apigateway.request-template-foreach-folds-a-body-array-into-the-mock-status, apigateway.request-template-foreach-stops-after-one-thousand-iterations

Cause 8 of 10: the $util helpers ($util.escapeJavaScript, $util.parseJson, $util.base64Decode) are unsupported.
AWS: recorded behavior only. `$util.escapeJavaScript(s)` escapes quotes, backslash and `/`; `$util.parseJson(s)` parses a JSON string (from the body or from a header document) into a navigable object; `$util.base64Decode(s)` decodes base64 text.
Emulator: the escapeJavaScript and parseJson-routing checks reach no verdict inside their budget, the header-document parseJson check likewise, and the base64Decode template answers 500 `{"message":"Internal server error"}` where the expected result was the template's own 403 `{"message": "pass token refused"}`.
Fix: the VTL `$util` function library.
Traces: apigateway.mock-request-template-escape-javascript-escapes-quotes-backslash-and-slash, apigateway.mock-request-template-parse-json-routes-status-and-unmapped-content-type-rejected, apigateway.response-template-parse-json-reads-nested-fields-of-a-header-document, apigateway.request-template-base64-decoded-pass-header-mismatch-rejected-with-403

Cause 9 of 10: $input.path / $input.json over the integration body do not read nested members or re-serialize correctly.
AWS: recorded behavior only. In an integration RESPONSE template `$input.path('$.a.b')` navigates the backend's JSON body, and `$input.json('$')` re-serializes the backend JSON as valid JSON.
Emulator: the nested-member response-template check reaches no verdict inside its budget, and the `$input.json` re-serialization check fails parsing the served body — `json.decoder.JSONDecodeError: Expecting value: line 1 column 137 (char 136)`, i.e. the emulator emitted a body that is not valid JSON.
Fix: the VTL `$input.path`/`$input.json` implementation for the integration-response direction.
Traces: apigateway.response-template-input-path-reads-nested-members-of-the-integration-body, apigateway.integration-response-template-input-json-reserializes-backend-json

Cause 10 of 10: a template failure on a malformed body is reported as 500 InternalServerErrorException instead of 400 BadRequestException.
AWS: recorded behavior only; BadRequestException is @httpError 400 in the model. `$input.path(...)` over a truncated/malformed JSON request body is refused with HTTP 400, `x-amzn-ErrorType: BadRequestException` and the fixed body text `Could not process payload` — a client error, not a server error.
Emulator: the stage answers HTTP 500 with `x-amzn-ErrorType: InternalServerErrorException` and `{"message":"Internal server error"}`.
Why: the caller's own malformed payload is reported as the service's fault, so SDK/HTTP clients retry a request that can never succeed instead of failing fast.
Fix: the mapping-template execution error path — classify a template evaluation failure caused by the request payload as BadRequestException/400 with AWS's message.
Traces: apigateway.request-template-input-path-on-truncated-json-rejected-400

## Evidence
Cause 1 — expected {'stage': 'live', 'tier': 'gold', 'path': '/live/keyed', 'api': '<api id>', 'type': 'INVALID_API_KEY'}; got {'stage': '$context.stage', 'tier': '$stageVariables.tier', 'path': '$context.path', 'api': '$context.apiId', 'type': 'INVALID_API_KEY'}.
Cause 2 — expected {'access-control-allow-origin': 'https://app.example.com', 'x-query': 'abc', 'x-tier': 'gold', 'x-static': 'fixed'}; got {'access-control-allow-origin': None, 'x-query': None, 'x-tier': None, 'x-static': 'fixed'}.
Cause 3 — 'control: an unrelated User-Agent must take the default branch; got 500 {"message":"Internal server error"}'.
Cause 4 — 'with no client id the template must see a minted lowercase UUID (200); got 500, rendered None'.
Cause 5 — "the gold caller's context rendered (200, 'context-unmatched')".
Cause 6 — TimeoutError: GET .../render?tag=first&tag=second&tag=third never answered 200; last=(500, '{"message":"Internal server error"}').
Cause 7 — 'a 1000-pass loop answered 500 {"message":"Internal server error"}, expected 200 and 1000 passes'; 'two 1000-pass loops answered 500 ..., expected 200'.
Cause 8 — 'expected the unencoded token to be refused with 403 {"message": "pass token refused"}, got (500, {"message":"Internal server error"})'; three further checks reached no verdict in budget.
Cause 9 — json.decoder.JSONDecodeError: Expecting value: line 1 column 137 (char 136) while parsing the served body.
Cause 10 — '$input.path over a truncated JSON body must be refused with 400 BadRequestException \'Could not process payload\'; got 500 \'InternalServerErrorException\' {"message":"Internal server error"}'.

## Steps to Reproduce
1. CreateRestApi (REGIONAL); add a resource with a MOCK integration whose requestTemplates/responseTemplates use, in turn: $context.identity.userAgent, $context.requestId, $context.authorizer.<key>, $input.params('tag'), #foreach over $input.path('$.items'), $util.escapeJavaScript/$util.parseJson/$util.base64Decode, $input.path('$.a.b') and $input.json('$').
2. PutGatewayResponse for INVALID_API_KEY with responseTemplates referencing $context.stage/$context.path/$context.apiId/$stageVariables.tier and responseParameters mapping method.request.querystring.*, method.request.header.* and stageVariables.* into gatewayresponse.header.*.
3. CreateDeployment + CreateStage (with stage variables), then invoke each path through the stage and compare the rendered body/headers and status.
4. For cause 10, POST a truncated JSON body to a method whose request template calls $input.path('$...').
