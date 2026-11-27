Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigatewayv2.payload-1-0-path-through-a-custom-domain-keeps-the-api-mapping
Issue-Key: acm__apigatewayv2.payload-1-0-path-through-a-custom-domain-keeps-the-api-mapping|run#7|AssertionError|assert event.get("path") == "/v1/shop/orders/9", (
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__apigatewayv2: a payload-format-1.0 Lambda proxy event served through a custom domain drops the API mapping key from `path`

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApiMapping

## Description
Cause 1 of 1: the API mapping key is stripped from the payload-1.0 event's `path` instead of being preserved.

AWS: an HTTP API reached through an apigatewayv2 custom domain with an ApiMapping whose ApiMappingKey is a multi-segment prefix serves requests at https://<domain>/<mapping-key>/<route-path>. The mapping key is used to select the API, but the request path delivered to a Lambda proxy integration with PayloadFormatVersion 1.0 is the FULL request path as the client sent it, mapping key included: `event["path"]` is "/<mapping-key>/<route-path>". (Smithy: apigatewayv2 ApiMapping.ApiMappingKey selects the API for a custom domain; the Lambda proxy payload shape itself is not in the model, so: recorded behavior only — the recording shows `path` carrying the mapping key.)

Emulator: the mapping key prefix is removed before the payload-1.0 event is built, so the integration receives only the route-relative path. For a mapping key of two segments and a request to /<seg1>/<seg2>/orders/9, `event["path"]` is "/orders/9" instead of "/<seg1>/<seg2>/orders/9".

Why: a function behind a custom domain cannot reconstruct the URL the client used. Anything that rebuilds absolute URLs from the event (redirects, Location headers, HATEOAS links, pagination cursors), any framework that re-routes on event.path, and any logging/metric dimension keyed on the request path all see a path AWS never sends — and a function ported from AWS either routes to the wrong handler or emits links that 404.

Fix: the apigatewayv2 custom-domain request router and its payload-format-1.0 event builder for Lambda proxy integrations — the mapping key must be consumed for API selection only, and `path` must be serialized from the original request target.

Traces: acm__apigatewayv2.payload-1-0-path-through-a-custom-domain-keeps-the-api-mapping

## Evidence
Expected: the payload-1.0 event's `path` equals the full request path including the API mapping key, i.e. "/<mapping-key>/orders/9" (recorded as '/v1/shop/orders/9' for a mapping key of 'v1/shop').
Actual: AssertionError: path must carry the API mapping v1/shop, got '/orders/9'
The request was served (status and body were otherwise fine) — only the `path` member of the integration event diverges, so the mapping key is being consumed and discarded rather than passed through. 'v1/shop' and 'orders/9' are this recording's values filling the template; the requirement is that whatever mapping key and route path the request used both appear in `path`.

## Steps to Reproduce
1. acm RequestCertificate for a domain and get it to ISSUED; apigatewayv2 CreateDomainName with that certificate.
2. apigatewayv2 CreateApi (HTTP API) with a Lambda proxy integration whose PayloadFormatVersion is "1.0", a route for /orders/{id}, and a stage.
3. apigatewayv2 CreateApiMapping with ApiMappingKey="v1/shop" binding that API/stage to the custom domain.
4. GET https://<custom-domain>/v1/shop/orders/9 and read the event the Lambda received.
5. Observe event["path"] == "/orders/9"; AWS delivers "/v1/shop/orders/9".
