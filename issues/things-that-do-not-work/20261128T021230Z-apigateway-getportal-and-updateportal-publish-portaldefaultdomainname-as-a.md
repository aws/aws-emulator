Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-create-update-delete-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetPortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigateway GetPortal and UpdatePortal publish portalDefaultDomainName as a host:port literal rather than a domain name

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPortal

## Description
2 causes. The portal read and update views publish endpointConfiguration.portalDefaultDomainName as an address-and-port literal instead of the DNS domain name AWS returns. The requirement here is the SHAPE of the value — a host name with no port — not any particular host: the specific address and port differ in every environment and are not part of the contract.

This is the same fault class as the already-open issues 20261119T122508Z-apigateway-createportal-announces-a-portal-default-domain-the.md (CreatePortal), 20261113T182955Z-apigateway-listportals-summaries-publish-a-host-port-portal.md (ListPortals) and 20261126T210550Z-apigatewayv2-portal-data-plane-the-portaldefaultdomainname-announced-for.md (the data plane). Neither of those carries the replay key for the GetPortal or UpdatePortal response path, so those two views are net-new here and are filed with their own keys.

Cause 1 of 2: GetPortal.
AWS: recorded behavior only — the Smithy model types endpointConfiguration.portalDefaultDomainName as a plain string with no pattern, so the shape is not model-stated. The recording shows GetPortal answering 200 with a portalDefaultDomainName that is a DNS domain name: a dotted host label sequence, carrying NO ':<port>' suffix, which a client can resolve and place in a URL or a CNAME.
Emulator: answers 200 with portalDefaultDomainName set to an IP-literal-plus-port string of the form '<address>:<port>'.
Why: a client that builds the portal URL from this member, or that publishes it as a DNS alias target, produces an unusable value — an authority with a port cannot be a CNAME target, and the emulator's own data plane is announced on an address no name resolves to.
Fix: the GetPortal response serializer / the portal state's endpoint-configuration projection in the apigatewayv2 portal handler — it must render a domain-name-shaped value and keep the port out of this member.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#2, apigatewayv2-portal-create-update-delete-lifecycle#task#5, apigatewayv2-portal-multiple-isolation#task#4, apigatewayv2-portal-multiple-isolation#task#5.

Cause 2 of 2: UpdatePortal.
AWS: recorded behavior only, as above — the 200 from UpdatePortal echoes the same domain-name-shaped portalDefaultDomainName.
Emulator: echoes the same '<address>:<port>' literal.
Why: the update response is what a client caches after a mutation, so the unusable value propagates even for clients that never call GetPortal.
Fix: the UpdatePortal response serializer in the apigatewayv2 portal handler, sharing the projection fixed for cause 1.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#4.

## Evidence
Cause 1 — GetPortal 200: $.endpointConfiguration.portalDefaultDomainName expected a domain-name-shaped value (shape assertion, no literal required), got a '<address>:<port>' authority literal (recorded instance '127.0.0.1:40103'); 4 affected steps across apigatewayv2-portal-create-update-delete-lifecycle and apigatewayv2-portal-multiple-isolation. Cause 2 — UpdatePortal 200: same member, same divergence.

## Steps to Reproduce
1. Replay apigatewayv2-portal-create-update-delete-lifecycle step task#2 (GetPortal) and inspect $.endpointConfiguration.portalDefaultDomainName: it is an '<address>:<port>' literal, not a domain name.
2. Replay the same fixture step task#4 (UpdatePortal) and inspect the same member.
