Issue-Source: trace-sweep-divergence
Issue-Trace: acc-gateway-update-and-target-errors
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-gateway-update-and-target-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|status
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# UpdateGatewayTarget resolves the path target before validating the body, so an omitted credentialProviderConfigurations answers 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGatewayTarget

## Description
Cause 1 of 1: `UpdateGatewayTarget` decides the request in the wrong order. It resolves the resource the URI path addresses (the gateway, then the target under it) BEFORE it judges the request body, so a request that both omits `credentialProviderConfigurations` and names a target the store cannot resolve is answered as a missing resource instead of as an invalid request. Fixture acc-gateway-update-and-target-errors step task#7 (PUT /gateways/{gatewayIdentifier}/targets/{targetId}/) is exactly that request.

AWS: recorded behavior only for the precedence; the error itself is modeled. Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json): `UpdateGatewayTarget` http {method PUT, uri /gateways/{gatewayIdentifier}/targets/{targetId}/, code 202}; input members `gatewayIdentifier`/`targetId`/`targetConfiguration` are @required while `credentialProviderConfigurations` (CredentialProviderConfigurations, @length min 1 max 1) is optional on the input and @required on `UpdateGatewayTargetResponse`; modeled errors include ValidationException (httpError 400, members message+reason @required) and ResourceNotFoundException (httpError 404). The recording shows AWS running the body check first and answering HTTP 400 with message `Credential provider configurations is not defined` — the WHOLE sentence is fixed text for this error class, with no part taken from the request or from account state (no member name, id or limit is interpolated). AWS never reaches the target lookup for this request, so no not-found error is emitted.

Emulator: answers HTTP 404 with header `x-amzn-errortype: ResourceNotFoundException` and message `Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier.` — i.e. a different status class (404 vs 400), a different error code (ResourceNotFoundException vs ValidationException) and a different message than the recorded refusal. The ordering is deliberate in the handler, which states it in a comment: "The resource the PATH addresses is resolved before the body is judged: a request naming a gateway or a target that is not there answers the modelled 404 whatever else the body leaves out" — the gateway/target lookup and its 404 run first, and the `credentialProviderConfigurations` presence check only runs after both resolve.

Why: a client branches on the error code. `ValidationException` tells it to fix the request it sent; `ResourceNotFoundException` tells it the target is gone, so an SDK retry/reconcile loop (CDK- or Terraform-style) recreates a target that in fact still exists, or a 404-triggered create path mints a duplicate, and error handling keyed on AWS's 400/ValidationException wording never matches. The 400-vs-404 split also changes which responses are treated as client-input faults by generic middleware.

Fix: the operation's validation ordering in its handler — services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_gateway_target.rs. Run the body-shape validation (the required-on-output `credentialProviderConfigurations`, and any other body-only check) BEFORE the path-resource resolution, so a request whose body cannot be rendered into the modeled response is refused with the modeled 400 ValidationException and its fixed sentence, and only a body-valid request proceeds to the gateway/target lookup that can answer 404.

Note: open issue 20261101T065227Z-bedrock-agentcore-control-updategatewaytarget-refuses-a-request-that-omits-the-optional.md (behavioral corpus) covers the SAME member from the other side — it asks that an omitted `credentialProviderConfigurations` be defaulted rather than refused for a target that EXISTS. It does not carry this fixture step's replay key and does not cover this precedence defect; this recorded wire step shows AWS does emit the 400 `Credential provider configurations is not defined` refusal for this request, so whichever way that issue is resolved, the body verdict must be reached before the target lookup.

Traces: acc-gateway-update-and-target-errors#task#7 (handler precedence at update_gateway_target.rs lines 218-269).

## Evidence
Fixture acc-gateway-update-and-target-errors, step task#7, operation UpdateGatewayTarget (verdict: mismatch).

Expected (recorded AWS response):
  HTTP 400
  $.message = "Credential provider configurations is not defined"   (entire sentence is fixed text for this error class; no request- or state-derived part)
  x-amzn-errortype: <not asserted by the fixture>  (modeled as ValidationException, httpError 400)

Actual (emulator):
  HTTP 404
  $.message = "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier."
  $headers.x-amzn-errortype = "ResourceNotFoundException"

Replay detail, verbatim:
  status: expected 400, got 404
  $.message: value_mismatch (expected 'Credential provider configurations is not defined', got "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier.")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  divergence_key: service=bedrock-agentcore-control|UpdateGatewayTarget|status=400->404|keys=$.message,$headers.x-amzn-errortype,status

Model evidence (smithy_query query="operation" operation="UpdateGatewayTarget"):
  http: {"code": 202, "method": "PUT", "uri": "/gateways/{gatewayIdentifier}/targets/{targetId}/"}
  input.credentialProviderConfigurations -> {"target":"CredentialProviderConfigurations","length":{"min":1,"max":1},"type":"list"}  (NOT required)
  output.credentialProviderConfigurations -> required
  errors include {"shape":"ValidationException","httpError":400} and {"shape":"ResourceNotFoundException","httpError":404}

Code evidence (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_gateway_target.rs):
  lines 218-252: gateway lookup -> ResourceNotFoundException(resource_not_found(RESOURCE_GATEWAY)); target lookup -> ResourceNotFoundException(resource_not_found(RESOURCE_GATEWAY_TARGET)) — both BEFORE any body check.
  lines 254-269: only then the `credential_provider_configurations` presence check -> ValidationException(member_not_defined(CREDENTIAL_PROVIDER_CONFIGURATIONS_PROSE)).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a bedrock-agentcore-control client at its data listener.
2. Replay fixture acc-gateway-update-and-target-errors and stop at step task#7.
3. Equivalently, by hand: send PUT /gateways/{gatewayIdentifier}/targets/{targetId}/ for a targetId the emulator cannot resolve under that gateway, with a body carrying `targetConfiguration` (and optionally `name`/`description`) and NO `credentialProviderConfigurations`.
4. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier."
5. Expected: HTTP 400 ValidationException with message "Credential provider configurations is not defined" — the body verdict is reached before the path target is resolved.
