Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.gateway-rule-duplicate-priority-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.gateway-rule-duplicate-priority-rejected|_error#13|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Commit: 55602a0ca6efec06d210bcb5a14eb249d752ec62

# CreateGatewayRule accepts a priority already held by a sibling rule instead of refusing with ValidationException

> _Note: this issue was identified against the code base at commit `55602a0ca6efec06d210bcb5a14eb249d752ec62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayRule

## Description
Cause 1 of 1: CreateGatewayRule does not enforce the uniqueness of a rule's `priority` within one gateway, so two rules can be created at the same priority.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json): CreateGatewayRule, POST /gateways/{gatewayIdentifier}/rules, success http code 202; `priority` is a required integer (GatewayRulePriority, @range min 1 max 1000000); modeled errors include ValidationException (httpError 400), ConflictException (409) and ResourceNotFoundException (404). Rule priority orders evaluation within a gateway, so AWS treats a priority already occupied by a sibling rule on the SAME gateway as an invalid request: the second CreateGatewayRule is refused with HTTP 400, error code ValidationException, and the message "Priority <priority> conflicts with an existing rule". In that message "Priority " and " conflicts with an existing rule" are the fixed text and <priority> is the variable part taken verbatim from the request's `priority` member (the replayed check used 500; that number fills the template and is not part of it). The refusal has no effect: the gateway keeps exactly one rule at that priority, the pre-existing one, and a subsequent rule at a DISTINCT priority is still accepted (202) and settles ACTIVE. Beyond the model's error list the recorded behavior is the check's assertion only.

Emulator: the duplicate call succeeds. It answers HTTP 202 with a freshly minted ruleId, echoing the same `priority` (500) and `status: CREATING`, and persists a second rule, so the gateway ends up with two rules at the identical priority. No ValidationException and no ConflictException is ever produced for a colliding priority.

Why: priority is the gateway's rule-evaluation key. A client that relies on AWS rejecting a colliding priority uses the 400/ValidationException as its collision detector — it catches the error, picks the next free priority and retries, or surfaces the conflict to the operator. Against the emulator the duplicate is silently accepted, so (a) the client's conflict-handling branch is never exercised, (b) ListGatewayRules returns two rules at one priority, a configuration AWS can never produce, and (c) which of the two colliding rules wins routing becomes non-deterministic emulator-specific behavior that no client can reason about.

Fix: the CreateGatewayRule handler's pre-persist validation — before minting a ruleId, scan the target gateway's existing rules for one already holding the requested `priority` and, if one exists, refuse with ValidationException / HTTP 400 and AWS's message template, persisting nothing. This belongs with the handler's input/state validation (next to the gateway-state gating), not in the response serializer.

Traces: behavioral check bedrock-agentcore-control.gateway-rule-duplicate-priority-rejected (expected 400 ValidationException "Priority 500 conflicts with an existing rule", got 202 with a second rule created). Handler file: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_gateway_rule.rs.

Not covered by any open issue: the only open gateway-rule issue, 20261113T144000Z-bedrock-agentcore-control-gateway-rule-operations-diverge-creategatewayrule-skips-gateway-ready.md, covers two different causes (CreateGatewayRule not gated on the gateway being READY, and GetGatewayRule's generic not-found message) and says nothing about priority uniqueness; the suite report left this cluster unowned.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (55/141 checks passed).

Check bedrock-agentcore-control.gateway-rule-duplicate-priority-rejected, verdict mismatch:
  The check creates an IAM role, a gateway (polled to READY), a configuration bundle, then CreateGatewayRule with priority=500 (polled to ACTIVE), then repeats CreateGatewayRule on the SAME gateway with the SAME priority=500.
  Expected: ClientError with Error.Code == "ValidationException" and Error.Message == "Priority 500 conflicts with an existing rule" (HTTP 400), followed by ListGatewayRules showing exactly one rule at priority 500 (the first one), and a rule at priority 600 accepted.
  Actual: the duplicate call returned HTTP 202:
    AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'ruleId': '62f7cd14-e2b3-4cca-816b-93087de0403d', 'gatewayArn': 'arn:aws:bedrock-agentcore:...:gateway/...', 'priority': 500, 'conditions': [...], 'actions': [...], 'createdAt': ..., 'status': 'CREATING'}
  (the ruleId, gateway arn, account, region and timestamp above are per-run values; the requirement is the 400/ValidationException refusal and its message template, not those literals)

Smithy consulted: CreateGatewayRule — http {method: POST, uri: /gateways/{gatewayIdentifier}/rules, code: 202}; input member priority required, @range 1..1000000; errors ValidationException/400, ConflictException/409, ResourceNotFoundException/404, AccessDeniedException/403, ServiceQuotaExceededException/402, ThrottlingException/429, InternalServerException/500.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Create an IAM role trusted by bedrock-agentcore.amazonaws.com, then CreateGateway (name, roleArn, authorizerType=AWS_IAM) and poll GetGateway until status READY.
3. CreateConfigurationBundle to obtain a bundleArn/versionId for the rule's action.
4. CreateGatewayRule on that gateway with priority=500, one matchPrincipals condition and one configurationBundle staticOverride action; poll GetGatewayRule until status ACTIVE.
5. Call CreateGatewayRule again on the SAME gatewayIdentifier with the SAME priority=500. Expect HTTP 400, x-amzn-errortype: ValidationException, message 'Priority 500 conflicts with an existing rule'; observe HTTP 202 and a second rule created at priority 500.
6. ListGatewayRules for that gateway: expect exactly one rule at priority 500 (the rule from step 4); observe two.
7. Re-replay behavioral check bedrock-agentcore-control.gateway-rule-duplicate-priority-rejected to confirm the fix — it also asserts the positive control, that a rule at priority 600 is still accepted and settles ACTIVE.
