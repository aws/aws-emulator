Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-gateway-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-gateway-error-paths bedrock-agentcore-control-oauth2-credential-provider-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->4xx|status
Issue-Key: bedrock-agentcore-control|UpdateOauth2CredentialProvider|2xx->2xx|$.callbackUrl
Issue-Key: bedrock-agentcore-control|UpdateOauth2CredentialProvider|2xx->2xx|$.clientSecretJsonKey
Issue-Related: 20261102T055559Z-bedrock-agentcore-control-gateway-state-refusals-on-gateway-target-operations-are.md
Issue-Commit: 6d305b4d5bd3d6326cb7439fced5e9504df7c5e5

# bedrock-agentcore-control: 2 net-new causes — CreateGatewayTarget misses the gateway existence check; UpdateOauth2CredentialProvider response representation

> _Note: this issue was identified against the code base at commit `6d305b4d5bd3d6326cb7439fced5e9504df7c5e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
Two independent net-new divergences on service bedrock-agentcore-control, both of which are the emulator answering a well-formed request with a response AWS would not produce: a gateway-target create that refuses the request with a fabricated validation error instead of reporting the missing gateway, and an OAuth2 credential-provider update whose response body carries the wrong member set. 2 causes, both on service bedrock-agentcore-control.

Cause 1 of 2: CreateGatewayTarget never resolves the gateway named by the path, and refuses the request with a fabricated `name: is required` validation error instead of 404 ResourceNotFoundException.
AWS: Smithy CreateGatewayTarget binds POST /gateways/{gatewayIdentifier}/targets/ with success code 202. Its input declares `gatewayIdentifier` as @httpLabel @required (GatewayIdentifier, pattern ^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$) and `targetConfiguration` as @required; `name` (TargetName, pattern ^([0-9a-zA-Z][-]?){1,100}$) is NOT @required, so omitting it is a legal request. The operation models ResourceNotFoundException (httpError 404) and ValidationException (httpError 400). AWS resolves the gateway from the path label BEFORE validating the body, and when no gateway with that identifier exists it answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and the message "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier." — that message is entirely fixed text for this error class, with no variable part: AWS does NOT interpolate the requested gateway identifier into it.
Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and the message `name: is required`. The gateway identifier in the path is never looked up at all, and the refusal is attributed to an input member the model does not require.
Why: a client that targets a gateway which was deleted or never created cannot tell "the gateway is gone" from "my request is malformed" — it catches ValidationException-style errors as non-retryable programming bugs and never takes the recreate-the-gateway path that ResourceNotFoundException signals, and generic retry/classification layers treat 400 differently from 404. Worse, the fabricated required-check means a legitimate CreateGatewayTarget that relies on `name` being optional (AWS derives it) is rejected against an existing gateway too, so the operation is unreachable for that shape of request.
Fix: the CreateGatewayTarget handler and its input validation — resolve the path's `gatewayIdentifier` against gateway state and raise ResourceNotFoundException (404) with AWS's fixed message before any body validation runs, and remove the `name` required-check, which is not in the model. Note the open issue 20261108T153618Z-bedrock-agentcore-control-creategatewaytarget-settles-every-mcp-mcpserver-target-ready-with.md concerns the target's post-create READY settling on the SUCCESS path and does not cover this error path.
Traces: bedrock-agentcore-control-gateway-error-paths#task#1.

Cause 2 of 2: UpdateOauth2CredentialProvider's response omits `callbackUrl` and invents `clientSecretJsonKey`.
AWS: Smithy UpdateOauth2CredentialProvider (POST /identities/UpdateOauth2CredentialProvider, success code 200) declares in its output both `callbackUrl` (String) and `clientSecretJsonKey` (SecretJsonKeyType, @length 1..128) as OPTIONAL members. The recording shows AWS populating `callbackUrl` with the provider's OAuth2 redirect URL on the update response, and OMITTING `clientSecretJsonKey` entirely when no secret JSON key was configured for that provider (recorded behavior only for the omission rule — the model states only that the member is optional). The callback URL's value is derived from the provider (region/vault/provider name), not a constant.
Emulator: answers 200 but with `callbackUrl` absent from the body, and with `clientSecretJsonKey` present carrying the synthesized value `clientSecret` where AWS sends no member at all.
Why: a client that rotates an OAuth2 provider configuration and uses the update response directly to rebuild its authorization redirect reads a null callback URL and cannot complete the 3-legged OAuth flow without a second call; and a client that round-trips the update response into a later update sends back a `clientSecretJsonKey` the user never configured, silently changing which JSON key of the secret the credential is read from.
Fix: the UpdateOauth2CredentialProvider output serializer and the credential-provider state record behind it — persist and emit the provider's callback URL, and leave `clientSecretJsonKey` unset unless the request supplied one. The open issue 20261110T043203Z-bedrock-agentcore-control-oauth2-credential-provider-surface-diverges-representation-update-delete.md reports the same two serializer defects on the Get and Create responses and reports that Update's configuration write is discarded, but it does NOT cover the Update response's own representation — this cluster is unowned by it, so the Update serializer must be fixed as well.
Traces: bedrock-agentcore-control-oauth2-credential-provider-lifecycle#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (296 steps, 167 passed, 125 mismatched, score 56.4%); these are the two divergence clusters with no owning open issue and no blocking stub.

Cause 1 — CreateGatewayTarget, fixture bedrock-agentcore-control-gateway-error-paths step task#1:
  status: expected 404, got 400
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'ValidationException'
  $.message: expected "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier." (entirely fixed text), got 'name: is required'
  Model citations: CreateGatewayTarget http {method: POST, uri: /gateways/{gatewayIdentifier}/targets/, code: 202}; input.gatewayIdentifier @httpLabel @required; input.targetConfiguration @required; input.name NOT required; errors include ResourceNotFoundException (httpError 404) and ValidationException (httpError 400).

Cause 2 — UpdateOauth2CredentialProvider, fixture bedrock-agentcore-control-oauth2-credential-provider-lifecycle step task#5, expected 200 / got 200:
  $.callbackUrl: missing (expected the provider's OAuth2 callback URL, got None)
  $.clientSecretJsonKey: type_mismatch (expected None — member absent, got 'clientSecret')
  Model citations: UpdateOauth2CredentialProvider http {uri: /identities/UpdateOauth2CredentialProvider, method: POST, code: 200}; output.callbackUrl (String, optional); output.clientSecretJsonKey (SecretJsonKeyType, @length 1..128, optional).

## Steps to Reproduce
Cause 1:
1. POST /gateways/<a gatewayIdentifier matching ^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$ that was never created>/targets/ with a valid targetConfiguration and no `name` member.
2. Observe HTTP 400 ValidationException 'name: is required'. AWS answers HTTP 404 x-amzn-errortype: ResourceNotFoundException with "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier."
3. Replay: fixture bedrock-agentcore-control-gateway-error-paths step task#1.

Cause 2:
1. CreateOauth2CredentialProvider with a Google oauth2ProviderConfigInput and no secret JSON key.
2. UpdateOauth2CredentialProvider on that name with a changed oauth2ProviderConfigInput.
3. Inspect the update response: `callbackUrl` is absent (AWS populates it) and `clientSecretJsonKey` is present as 'clientSecret' (AWS omits the member).
4. Replay: fixture bedrock-agentcore-control-oauth2-credential-provider-lifecycle step task#5.
