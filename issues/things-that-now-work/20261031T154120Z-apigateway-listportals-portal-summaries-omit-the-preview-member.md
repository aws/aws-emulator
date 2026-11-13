Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-create-update-delete-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ListPortals|2xx->2xx|$.items[*]
Issue-Commit: 418fa7a6b82984f3652fde3052f2e7dd1ee98ae8

# ListPortals portal summaries omit the preview member

> _Note: this issue was identified against the code base at commit `418fa7a6b82984f3652fde3052f2e7dd1ee98ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPortals

## Description
The portal summaries ListPortals returns are missing the `preview` member that the recorded AWS summaries always carry. The open issue `20261029T220031Z-apigateway-portal-lifecycle-and-representation-preview-hostedzoneid-publishstatus.md` reports the identical omission on GetPortal, UpdatePortal and CreatePortal and owns those replay keys; the collection projection used by ListPortals is a separate code path with its own replay keys, so it is filed here. Both causes below must be closed.

Cause 1 of 2: ListPortals omits `preview` from the single returned portal summary.
AWS: Smithy: ListPortals http {method GET, uri /v2/portals, code 200}, output ListPortalsResponse.Items -> __listOfPortalSummary; PortalSummary declares member `Preview` (jsonName `preview`, target Preview). Recorded behavior: every portal summary carries `preview`, an empty object `{}` when no preview has been requested — the member is always present, not conditionally emitted.
Emulator: the item is returned with every other member populated but `preview` is absent from the JSON object.
Why: a client that iterates ListPortals and reads `item.preview` to decide whether a preview endpoint exists (and to surface preview generation errors) gets null/undefined instead of an empty structure and faults or mis-reports "no preview information" where AWS hands it a well-formed empty one. Because ListPortals is the discovery call, this breaks the list-then-inspect path even though GetPortal is fixed.
Fix: the ListPortals response projection / the PortalSummary serializer — default `preview` to the empty structure exactly as the GetPortal projection must.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#3.

Cause 2 of 2: ListPortals omits `preview` from every summary when several portals exist.
AWS: as cause 1 — each element of `items` carries `preview: {}`.
Emulator: both returned summaries (`items[0]` and `items[1]`) lack `preview`.
Why: as cause 1, and it confirms the omission is in the per-element projection rather than a single-item special case.
Fix: as cause 1.
Traces: apigatewayv2-portal-multiple-isolation#task#3.

Note: both clusters also report `endpointConfiguration.portalDefaultDomainName` as a loopback host:port URL. That is the address the emulator is actually reachable on and is NOT part of this issue.

## Evidence
Replay verdict `mismatch`. Cause 1 detail: `$.items[0]: missing (expected {... 'preview': {}, 'publishStatus': 'DISABLED', ...}, got {... 'publishStatus': 'DISABLED', ...})` — the expected element carries `preview: {}` and the actual element has no `preview` key at all; every other asserted member (portalArn, portalId, portalContent, authorization, includedPortalProductArns, publishStatus, tags) matches. Cause 2 detail: the same omission reported for both `$.items[0]` and `$.items[1]`. Status 200 expected and 200 returned in both clusters.

## Steps to Reproduce
Replay the corpus. Cause 1: create a portal, then GET /v2/portals (fixture apigatewayv2-portal-create-update-delete-lifecycle step task#3) — the single element of `items` has no `preview` key. Cause 2: create two portals and GET /v2/portals (apigatewayv2-portal-multiple-isolation#task#3) — neither element carries `preview`.

## Resolution
Both causes are resolved: every element of a `ListPortals` page carries `preview` (the empty structure until a preview has been built), because the operation projects each paged record through the one portal projection every portal reader shares. Pinned by `services/service-com-amazonaws-apigatewayv2-apigatewayv2/tests/all/portal_summary_projection.rs`, over collections of one to five portals with no, some and every preview built, at every page size.

The replay key `apigateway|ListPortals|2xx->2xx|$.items[*]` is a WHOLE-ELEMENT key and still diverges, on one member only: `endpointConfiguration.portalDefaultDomainName`, the loopback address the emulator is reachable on (the Note above excludes it from this issue). That member is owned by `issues/things-that-do-not-work/20261103T032504Z-apigateway-portal-responses-return-a-host-port-url.md`, which carries the same member's keys for `CreatePortal`, `GetPortal` and `UpdatePortal`; the element cannot diverge in any other member, because the list element and the `GetPortal` body are one projection of one record. So read this key as resolved for `preview` and still open for that member under the issue that owns it.
