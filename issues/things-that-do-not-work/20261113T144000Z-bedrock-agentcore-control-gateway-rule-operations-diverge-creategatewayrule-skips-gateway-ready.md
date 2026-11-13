Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-gateway-rule-routing-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-gateway-rule-routing-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGatewayRule|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|CreateGatewayRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateGatewayRule|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|GetGatewayRule|4xx->4xx|$.message
Issue-Commit: 8519584554abb240da771f66a4fe7c47c54f612d

# Gateway rule operations diverge: CreateGatewayRule skips gateway READY-state gating and GetGatewayRule not-found message is generic

> _Note: this issue was identified against the code base at commit `8519584554abb240da771f66a4fe7c47c54f612d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayRule

## Description
The emulator's gateway-rule surface (/gateways/{gatewayIdentifier}/rules) diverges from AWS in TWO independent ways. BOTH must be fixed and re-replayed before this issue is closed.

--- Cause 1 of 2: CreateGatewayRule is not gated on the gateway's lifecycle state ---
AWS: Smithy: CreateGatewayRule, POST /gateways/{gatewayIdentifier}/rules, http code 202, errors include ValidationException (httpError 400) and ResourceNotFoundException (404). A gateway created by CreateGateway starts in a non-terminal lifecycle state (CREATING) and only later becomes READY; AWS refuses rule creation against a gateway that is not READY with HTTP 400, x-amzn-errortype: ValidationException. Recorded behavior: the message's fixed text is "Gateway " + <gatewayIdentifier> + " is not in READY state. Current status: " + <the gateway's current lifecycle status, title-cased>; the gateway identifier and the status word are variable parts taken from the request path and the emulator's own gateway state (in the recording they were the probe's gateway id and "Creating"). This matches the state-refusal rule the same model already applies to the sibling gateway-target operations.
Emulator: answers HTTP 202 and creates the rule, with no error body at all ($.message missing, $headers.x-amzn-errortype missing), while the gateway is still CREATING.
Why: a client that relies on AWS's ordering contract — create the gateway, wait for READY, then attach rules — gets no signal that it raced the gateway's provisioning. On AWS the call fails and the client retries after READY; on the emulator the rule is silently accepted against a gateway that does not yet exist in a routable form, so the emulator reports a routing configuration AWS would never have produced, and the client's error-handling and wait-for-READY path is never exercised.
Fix: the CreateGatewayRule handler's pre-validation — resolve the path gateway and refuse with ValidationException/400 unless its status is READY, before any rule is persisted. The refusal belongs with the gateway-state check, not in the rule serializer.
Traces: bedrock-agentcore-control-gateway-rule-routing-workflow#task#2 (expected 400 ValidationException, got 202).

--- Cause 2 of 2: GetGatewayRule not-found message does not name the missing rule ---
AWS: Smithy: GetGatewayRule, GET /gateways/{gatewayIdentifier}/rules/{ruleId}, ruleId is a required @httpLabel constrained to a 36-char UUID (pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$); modeled error ResourceNotFoundException (httpError 404). Recorded behavior: for an unknown rule the status and error code are correct (404 / ResourceNotFoundException) but the message is "GatewayRule " + <ruleId> + " not found" — "GatewayRule " and " not found" are the fixed text, and the UUID in the middle is the variable part taken verbatim from the request's ruleId path label (the recording happened to use the all-zero UUID; that value is not part of the requirement).
Emulator: returns 404 ResourceNotFoundException with the generic message "Failed to retrieve rule because it doesn't exist. Retry the request with a different resource identifier." — it names neither the resource type as AWS spells it nor the requested ruleId.
Why: the error message is part of the API contract. Clients and operators that log or parse the message to identify WHICH rule was missing (common when a request fans out over several rule ids) get a message with no identifier in it, so the failure cannot be attributed to a specific rule, and any assertion or log-scrape keyed on AWS's wording fails.
Fix: the GetGatewayRule handler's not-found path — emit AWS's message template for the ResourceNotFoundException it already returns, interpolating the request's ruleId. Status and error code are already right; only the message text is wrong.
Traces: bedrock-agentcore-control-gateway-rule-routing-workflow#task#3 (404/404, $.message value_mismatch).

No open issue owns either cause: the replay report marks both clusters with an empty owned_by, and no existing open issue mentions GatewayRule or the gateway-rule routing fixture.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 296 steps, 160 passed, score 54.1%).

Cause 1 — fixture bedrock-agentcore-control-gateway-rule-routing-workflow, step task#2, operation CreateGatewayRule:
  expected_status 400, actual_status 202
  status: expected 400, got 202
  $.message: missing (expected 'Gateway sim2real-probe-gwrule-a479c794-tfszrh9vsn is not in READY state. Current status: Creating', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)
  (the gateway id in that expected message is the probe run's generated id — the requirement is the template, not that literal)

Cause 2 — fixture bedrock-agentcore-control-gateway-rule-routing-workflow, step task#3, operation GetGatewayRule:
  expected_status 404, actual_status 404
  $.message: value_mismatch (expected 'GatewayRule 00000000-0000-0000-0000-000000000000 not found', got "Failed to retrieve rule because it doesn't exist. Retry the request with a different resource identifier.")

Smithy model consulted: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json — CreateGatewayRule (http 202 POST /gateways/{gatewayIdentifier}/rules; errors ValidationException/400, ResourceNotFoundException/404, ConflictException/409) and GetGatewayRule (http 200 GET /gateways/{gatewayIdentifier}/rules/{ruleId}; errors ResourceNotFoundException/404, ValidationException/400).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture bedrock-agentcore-control-gateway-rule-routing-workflow and inspect step task#2. The fixture's setup creates a gateway (which the emulator leaves in CREATING) and then calls CreateGatewayRule on it: POST /gateways/<gatewayIdentifier>/rules with a priority, actions and description. Expect HTTP 400 with x-amzn-errortype: ValidationException and message 'Gateway <gatewayIdentifier> is not in READY state. Current status: <status>'; observe HTTP 202 with a created rule and no error body.
4. Cause 2: in the same fixture, step task#3 calls GET /gateways/<gatewayIdentifier>/rules/00000000-0000-0000-0000-000000000000 for a rule that does not exist. Expect HTTP 404 ResourceNotFoundException with message 'GatewayRule <ruleId> not found'; observe HTTP 404 ResourceNotFoundException with the generic message "Failed to retrieve rule because it doesn't exist. Retry the request with a different resource identifier."
5. Re-replay BOTH steps (task#2 and task#3) of bedrock-agentcore-control-gateway-rule-routing-workflow to confirm a fix; closing only one leaves the other diverging.
