Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-method-outside-role-policy-call-rejected apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-tools-call-follows-stage-deployment apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-tools-call-returns-stage-response apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-unfiltered-method-call-rejected
Issue-Key: apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-method-outside-role-policy-call-rejected|run#36|AssertionError|assert names == sorted([open_tool, guarded_tool]), f"tools/list advertised {names!r}"
Issue-Key: apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-tools-call-follows-stage-deployment|_served_text#34|Timeout|time.sleep(min(2 ** attempt, 15))
Issue-Key: apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-tools-call-returns-stage-response|run#28|AssertionError|assert names == [tool], f"tools/list advertised {names!r}, expected exactly {[tool]!r}"
Issue-Key: apigateway__bedrock-agentcore__bedrock-agentcore-control.gateway-rest-stage-target-unfiltered-method-call-rejected|_served_text#37|Timeout|time.sleep(min(2 ** attempt, 15))
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# apigateway__bedrock-agentcore__bedrock-agentcore-control: a gateway target over an API Gateway REST stage advertises no tools, so tools/list is empty and every tools/call is unknown

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
Cause 1 of 1: an AgentCore gateway target whose configuration names an API Gateway REST API + stage contributes no tools to the gateway's MCP endpoint.

AWS: recorded behavior only (the bedrock-agentcore-control model carries the REST-API target configuration but does not state the tool projection). The recording shows that once the target is created and the REST API stage is deployed, the gateway's MCP endpoint answers tools/list with exactly one tool per IAM-authorized method of the stage, named `<targetName>___<operationName>` (the recorded run expected exactly ['inventory___listInventory'], where `inventory` is the target name from the request and `listInventory` the method's operationName), and a SigV4-signed tools/call on that name reaches the stage's integration and returns its response. A method outside the gateway role's execute-api:Invoke policy is refused with the execute-api denial sentence 'User: <principal> is not authorized to perform: execute-api:Invoke on resource: <resource> because no identity-based policy allows the execute-api:Invoke action' (fixed text; principal and resource come from the signed request and the stage ARN).

Emulator: tools/list answers {"tools": []} for such a gateway — 'tools/list advertised []' — so no tool name exists to call; the two checks that poll for the tool to appear after the stage deployment never see one and are killed at the 60s budget.

Why: a client that adds a REST-API stage target to a gateway and then discovers tools over MCP sees an empty tool list, so the whole REST-stage target surface (tool discovery, invocation, and the execute-api authorization refusal) is unreachable.

Fix: the gateway MCP endpoint's tools/list projection for apiGatewayRestApi/stage targets, and the tools/call dispatch that signs and forwards to the stage, in the AgentCore gateway data-plane handler (the same component that projects Lambda/OpenAPI targets).

Traces: the four checks below; two assert on the empty tools/list directly, two time out polling for the tool.

## Evidence
Expected: tools/list lists exactly ['inventory___listInventory'] (one entry per IAM-authorized stage method, named <target>___<operationName>); tools/call on that name returns the stage's response; a method outside the role policy is refused with the execute-api:Invoke denial message.
Actual: `AssertionError: tools/list advertised []` and `AssertionError: tools/list advertised [], expected exactly ['inventory___listInventory']`; the two call-path checks end with `Error: Check timed out after 60.0s` while waiting for any tool to be advertised.

## Steps to Reproduce
1. Create a REST API with an IAM-authorized method backed by a MOCK integration and deploy it to a stage.
2. CreateGateway (MCP) and CreateGatewayTarget whose targetConfiguration names that REST API and stage, with GATEWAY_IAM_ROLE credentials.
3. Wait for the target to reach a ready state, then send a SigV4-signed JSON-RPC 'tools/list' to the gateway's MCP endpoint.
4. Observe {"tools": []} instead of one tool per authorized method; a subsequent 'tools/call' on <target>___<operationName> is unknown.
