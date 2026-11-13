Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__wafv2.appsync-waf-blocked-request-returns-403-waf-forbidden
Issue-Key: appsync__wafv2.appsync-waf-blocked-request-returns-403-waf-forbidden|_post_text#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: 4dd18d60a1467d656609e574bdc9c5a1ead23bf9

# appsync (with wafv2): the GraphQL API's published uris.GRAPHQL names a real-AWS appsync-api.<region>.amazonaws.com host the emulator does not serve, so no WAF-blocked GraphQL request can be issued (1 cause)

> _Note: this issue was identified against the code base at commit `4dd18d60a1467d656609e574bdc9c5a1ead23bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the endpoint AppSync publishes for a GraphQL API names an AWS production host instead of the address the emulator is actually reached on, so every data-plane leg of a WAF-protected AppSync API is unreachable.

AWS: GraphqlApi.uris is 'The URIs' of the API (Smithy appsync: GraphqlApi member `uris`: MapOfStringToString; sibling member `dns`: MapOfStringToString, 'The DNS records for the API'), and its GRAPHQL entry is the address a client POSTs queries to. In a recorded run the client reaches the API on exactly the host it was handed by CreateGraphqlApi/GetGraphqlApi; with a WebACL associated (AssociateWebACL on the API's ARN, confirmed by GetWebACLForResource) a request matching a Block rule is refused at that same edge with HTTP 403 and a GraphQL envelope carrying exactly one error with errorType 'WAFForbiddenException' and message '403 Forbidden' (fixed text; no `data` key), while the identical request without the matching header still reaches the resolver with HTTP 200.

Emulator: CreateGraphqlApi publishes uris.GRAPHQL as 'https://<api-id>.appsync-api.<region>.amazonaws.com/graphql' — a host the emulator does not serve. The harness refuses the published value outright: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The api id, region and local port in that line are per-run values; the real-AWS host suffix is the divergence. Because the very first (pre-WAF) POST never leaves the harness, the emulator is never asked for the 403 refusal at all, so the WAF edge behaviour (403 + WAFForbiddenException/'403 Forbidden', no data envelope, no resolver marker leak) is also unverified.

Why: a client that honours the published uris.GRAPHQL value sends its GraphQL queries to real AWS (or nowhere). No request can be issued against the emulated API, so neither the resolver round trip nor WAF enforcement at the AppSync edge can be exercised by any customer of the emulator.

Fix: the appsync GraphqlApi `uris`/`dns` serializer and the state that mints the API's endpoint must publish the emulator's own host and port for the API's resource path (the same address the control plane is reached on, or a per-resource listener on an own host); the WAF data-plane evaluation for an associated WebACL then has to run on that listener.

Related already-open issues describe the same serializer for other checks and do not carry this check's replay key: 20261110T153046Z-appsync-logs-appsync-the-graphql-api-s-published-uris-endpoint.md, 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md, 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md, 20261109T150433Z-appsync-4-net-new-causes-apicache-creating-state.md. None of them covers the wafv2-associated data-plane leg graded here.

Traces: appsync__wafv2.appsync-waf-blocked-request-returns-403-waf-forbidden.

## Evidence
Expected: uris['GRAPHQL'] is an address on one of the emulator's own hosts; POSTing {"query":"query { ping }"} with the API key returns HTTP 200 and data.ping == the resolver marker; after AssociateWebACL of a REGIONAL WebACL whose rule Blocks SingleHeader x-fidelity-block EXACTLY 'yes', the same POST with that header returns HTTP 403 with body {"errors":[{"errorType":"WAFForbiddenException","message":"403 Forbidden"}]} and no 'data' key; removing the header returns HTTP 200 again while GetWebACLForResource still reports the WebACL attached.

Actual: the check never reaches an HTTP call — the published endpoint is rejected at context.py _published_target: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://fqbyvglbrtekehhalcv4qm2r4m.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:39193/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The api id 'fqbyvglbrtekehhalcv4qm2r4m', the region and the port 39193 are per-run values; the defect is the 'appsync-api.<region>.amazonaws.com' host being announced at all.

## Steps to Reproduce
Replay appsync__wafv2.appsync-waf-blocked-request-returns-403-waf-forbidden: CreateGraphqlApi (API_KEY) + StartSchemaCreation for a trivial `Query { ping }` schema, CreateDataSource type NONE, CreateResolver (APPSYNC_JS) returning a marker, CreateApiKey, then read uris['GRAPHQL'] from CreateGraphqlApi and POST {"query":"query { ping }"} to it with header x-api-key. The published URI's host is an appsync-api.<region>.amazonaws.com name the emulator does not serve, so the POST is refused before it is sent; with an emulator-served endpoint the check then creates a REGIONAL WebACL blocking header x-fidelity-block=='yes', AssociateWebACL on the API ARN, and asserts the 403/WAFForbiddenException refusal and the header-less 200 control.
