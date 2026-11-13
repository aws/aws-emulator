Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-create-update-delete-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ListPortals|2xx->2xx|$.items[*]
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway ListPortals summaries publish a host:port portal domain name instead of a DNS host

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPortals

## Description
The list path for portals leaks the emulator's own listener address into the portal summary, so no listed summary matches the recorded AWS portal representation. Two clusters, one root cause, both reported with empty stub_path and empty owned_by.

Cause 1 of 2: ListPortals on a single portal returns a summary whose endpointConfiguration.portalDefaultDomainName is an authority-plus-path value.
AWS: Smithy (apigatewayv2) ListPortals — http GET /v2/portals, code 200; output member Items (jsonName 'items', target __listOfPortalSummary). PortalSummary.EndpointConfiguration (jsonName 'endpointConfiguration', target EndpointConfigurationResponse, @required) carries PortalDefaultDomainName, documented as the domain name API Gateway generates and manages for the portal. It is a DNS host name only: dot-separated host labels, no scheme, no ':port' and no '/path'. The recorded response asserts that member by shape rather than by literal value, because the host itself is per-account.
Emulator: serializes it as '<host>:<port>/_aws/portal/<portalId>' — an authority plus a path — so the whole expected summary fails to match (detail: `$.items[0]: missing`, with the actual item carrying `endpointConfiguration.portalDefaultDomainName: '127.0.0.1:34257/_aws/portal/z1hvlts8lq'`). The particular host, port and portal id are per-run values and are not the requirement; the requirement is the bare-hostname form.
Why: a client reads this member to build 'https://<portalDefaultDomainName>' or to point a CNAME / Route 53 alias at the portal. A value with a port and a path is not a DNS name at all, so DNS wiring is impossible and any client-side hostname validation rejects the listed portal; a list-then-describe reconciliation also cannot match the listed summary against AWS's shape.
Fix: the portal summary serializer used by ListPortals — take the portal's domain name from the same portal representation builder that mints it for CreatePortal/GetPortal and emit host labels only, keeping the emulator's listener port and front-door path out of the member.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#3.

Cause 2 of 2: the same defect across a multi-portal list, where every listed entry diverges.
AWS: same ListPortals / PortalSummary binding as above; with two portals created, both items must carry hostname-only portalDefaultDomainName values.
Emulator: both entries carry the '<host>:<port>/_aws/portal/<portalId>' form, so items[0] AND items[1] fail to match (detail: `$.items[0]: missing ... $.items[1]: missing`). This confirms the fault is in the per-item representation and not in list ordering or pagination.
Why: as above, multiplied — a client enumerating portals gets no usable portal address for any of them.
Fix: same component; fixing the shared portal representation must be reflected on the list path for every item.
Traces: apigatewayv2-portal-multiple-isolation#task#3.

Related open issue, which does NOT carry these replay keys: 20261103T032504Z-apigateway-portal-responses-return-a-host-port-url.md records the identical malformed member on CreatePortal/GetPortal/UpdatePortal only (Issue-Key lines name just those three operations), so the ListPortals keys above are unowned and are filed here; a fixer should expect one shared root cause with that issue.

## Evidence
Replay of simparity-wire-aws-v1. Clusters with empty stub_path and empty owned_by:

1) divergence_key `service=apigateway|ListPortals|status=200->200|keys=$.items[0]`, fixture apigatewayv2-portal-create-update-delete-lifecycle step task#3, verdict mismatch, 200/200, 1 affected step. Expected item has `endpointConfiguration.portalDefaultDomainName: '<shaped-not-asserted>'`; actual item has `'127.0.0.1:34257/_aws/portal/z1hvlts8lq'`. Every other member of the item (portalArn, portalId, portalContent, publishStatus DISABLED, includedPortalProductArns, preview, tags) matches.

2) divergence_key `service=apigateway|ListPortals|status=200->200|keys=$.items[0],$.items[1]`, fixture apigatewayv2-portal-multiple-isolation step task#3, verdict mismatch, 200/200, 1 affected step. Both listed summaries carry the same authority-plus-path value (recorded actual `'127.0.0.1:42821/_aws/portal/z1hvlts8lq'` for the first entry).

Hosts, ports and generated portal ids in the recorded actuals are per-run; the requirement is that portalDefaultDomainName be a bare DNS host name with no port and no path.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay apigatewayv2-portal-create-update-delete-lifecycle#task#3: GET /v2/portals after creating one portal. Inspect items[0].endpointConfiguration.portalDefaultDomainName — it is '<host>:<port>/_aws/portal/<portalId>' instead of a bare DNS host name, so the listed summary does not match the recorded AWS summary.
3. Replay apigatewayv2-portal-multiple-isolation#task#3: GET /v2/portals with two portals present — items[0] and items[1] both carry the same malformed member.
4. Expected on both: a hostname-only portalDefaultDomainName (dot-separated labels, no port, no path).
