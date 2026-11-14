Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config bedrock-agentcore-control.gateway-target-lifecycle-transitions bedrock-agentcore-control.gateway-target-lifecycle-ready
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config|run#16|AssertionError|assert settled["status"] != "READY", (
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|ValidationException
Issue-Key: bedrock-agentcore-control.gateway-target-lifecycle-ready|run#21|AssertionError|assert mutate_code == "ResourceNotFoundException", (
Issue-Commit: eadb2d0c274461491404935b82a39edede7a2374

# bedrock-agentcore-control: 3 net-new causes — VPC-mode CreateBrowser accepts a nonexistent subnet and reaches READY; UpdateGatewayTarget demands credentialProviderConfigurations and misclassifies a deleted target

> _Note: this issue was identified against the code base at commit `eadb2d0c274461491404935b82a39edede7a2374`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGatewayTarget

## Description
Three net-new root causes in the bedrock-agentcore-control emulator, all found by replaying the behavioral suite. None is covered by an open issue: the nearest neighbours are 20261108T153618Z-bedrock-agentcore-control-creategatewaytarget-settles-every-mcp-mcpserver-target-ready-with.md (CreateGatewayTarget readiness/statusReasons, not Update) and 20261110T043203Z-bedrock-agentcore-control-3-net-new-causes-in.md (built-in browser inventory, dataset-version message, UpdateGateway gatewayUrl) — neither touches VPC validation for browsers or UpdateGatewayTarget's input/lookup handling.

Cause 1 of 3: CreateBrowser accepts a VPC-mode browser whose vpcConfig names a subnet that does not exist, and the browser converges to READY.
AWS: Smithy — CreateBrowser.networkConfiguration is @required (BrowserNetworkConfiguration), whose vpcConfig member is documented "required when the network mode is set to VPC"; VpcConfig.subnets and VpcConfig.securityGroups are @required lists (length 1..16). CreateBrowser models ValidationException (HTTP 400) and BrowserStatus is the enum [CREATING, CREATE_FAILED, READY, DELETING, DELETE_FAILED, DELETED], and GetBrowser carries failureReason for exactly this case. On AWS a VPC-mode browser whose subnet id is well-formed but is not a subnet of the account is either refused up front with ValidationException or accepted and settled in the terminal failure state CREATE_FAILED with a failureReason naming the unresolvable subnet — it never becomes usable, because no ENI can be placed. Recorded behavior: the check derives the absent subnet id from a real one (so it matches ^subnet-[0-9a-zA-Z]{8,17}$), first proves via ec2 DescribeSubnets that it does not exist, and accepts EITHER refusal path.
Emulator: CreateBrowser accepts the request, and GetBrowser polls to status=READY with failureReason absent (null). The emulator never resolves the declared subnets/security groups against EC2 at all, so the network binding it echoes back is pure request echo.
Why: a client that provisions a VPC-mode browser with a wrong, stale or typo'd subnet id is told the browser is READY. Every downstream call then targets a browser with no real network placement, and the error surfaces far from its cause — exactly the misconfiguration the ValidationException/CREATE_FAILED contract exists to catch. Infrastructure tests that assert a bad subnet is rejected pass against the emulator and fail against AWS.
Fix: CreateBrowser's validation/handler for networkMode=VPC must resolve each vpcConfig.subnets and vpcConfig.securityGroups entry against the account's EC2 state, and either refuse with ValidationException (400) naming the unresolvable subnet or settle the browser in CREATE_FAILED with a failureReason, instead of settling READY from the request echo. The same resolution belongs in the state transition that moves a browser out of CREATING.
Traces: bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config (assertion: "a browser bound to the nonexistent subnet ... reached READY: failureReason=None"). The same check's first negative — networkMode=VPC with vpcConfig omitted — already passes, so only the unresolvable-id path is in scope.

Cause 2 of 3: UpdateGatewayTarget refuses any request that omits credentialProviderConfigurations with ValidationException "Credential provider configurations is not defined".
AWS: Smithy — UpdateGatewayTargetRequest members are gatewayIdentifier (@required, httpLabel), targetId (@required, httpLabel), targetConfiguration (@required), and the OPTIONAL members name, description, credentialProviderConfigurations (list, length 1..1), metadataConfiguration, privateEndpoint, certificateConfigurations. credentialProviderConfigurations is @required only on the RESPONSE shape (UpdateGatewayTargetResponse), not on the input; CreateGatewayTarget's input likewise leaves it optional. So AWS accepts an update that names only the target's configuration (HTTP 202) and keeps the target's existing/service-defaulted credential provider configuration, which is then echoed in the required response member.
Emulator: the call is refused 400 ValidationException with the message "Credential provider configurations is not defined" — the emulator treats an input member the model marks optional as mandatory, and conflates the required OUTPUT member with the input. Fixed text of the emulator message is the whole sentence; it has no variable part, and no AWS message for this request exists because AWS does not refuse the request.
Why: the documented update flow is unusable. A client that created a target without credentialProviderConfigurations (which the emulator itself accepts) can never update it — not its endpoint, not its name — so the target's configured state is frozen after create, and every generated/SDK-shaped update that omits the optional member is rejected.
Fix: UpdateGatewayTarget's input validation must stop requiring credentialProviderConfigurations; the handler must carry the stored target's credential provider configuration forward when the member is absent and return it in the required response member.
Traces: bedrock-agentcore-control.gateway-target-lifecycle-transitions (botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the UpdateGatewayTarget operation: Credential provider configurations is not defined).

Cause 3 of 3: UpdateGatewayTarget against a DELETED target is refused ValidationException instead of ResourceNotFoundException.
AWS: Smithy — UpdateGatewayTarget models ResourceNotFoundException (HTTP 404) alongside ValidationException (400). A target whose DeleteGatewayTarget has completed no longer exists, so a mutation on its id is answered 404 ResourceNotFoundException. Recorded behavior: the same check already observes GetGatewayTarget answering ResourceNotFoundException with the message "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier." for the same id — that is the sentence AWS words for a missing target; its text is entirely fixed, with no request-derived parts. The mutation must be refused the same way, not merely absent from reads.
Emulator: UpdateGatewayTarget on the deleted targetId answers ValidationException (400). Hypothesis: the handler runs request-shape validation (Cause 2's missing-credentialProviderConfigurations check) before it resolves the target, so an existence failure is reported as an input failure; whichever ordering it uses, the emitted code is wrong for a nonexistent target.
Why: a client cannot distinguish "my request was malformed" (do not retry) from "the target is gone" (stop, re-create). Retry/cleanup logic keyed on ResourceNotFoundException loops or aborts, and a 400 wrongly blames the caller's input for a tombstoned resource.
Fix: UpdateGatewayTarget's handler must resolve (gatewayIdentifier, targetId) against state BEFORE input-member validation and raise ResourceNotFoundException (404) with AWS's missing-target sentence when the target does not exist.
Traces: bedrock-agentcore-control.gateway-target-lifecycle-ready (assertion: "expected UpdateGatewayTarget on a deleted target to fail with ResourceNotFoundException, got 'ValidationException'").

## Evidence
Cause 1 — bedrock-agentcore-control.browser-vpc-network-mode-requires-real-vpc-config:
  Expected (AWS): CreateBrowser(networkMode=VPC, vpcConfig.subnets=[<well-formed but nonexistent subnet>]) is refused 400 ValidationException, OR accepted (202) and GetBrowser settles status=CREATE_FAILED with a non-null failureReason naming the unresolvable subnet.
  Actual (emulator): CreateBrowser returns 202; GetBrowser polls to status='READY', failureReason=None. Assertion: "AssertionError: a browser bound to the nonexistent subnet 'subnet-...' reached READY: failureReason=None". The subnet id in the message is a per-run derived value, not part of the requirement; the requirement is that an unresolvable subnet never yields READY.

Cause 2 — bedrock-agentcore-control.gateway-target-lifecycle-transitions:
  Request: UpdateGatewayTarget(gatewayIdentifier=<gw>, targetId=<id>, name=<name>, targetConfiguration={http:{passthrough:{endpoint:'https://example.com/mcp-v2', protocolType:'MCP'}}}) — credentialProviderConfigurations omitted, exactly as the preceding CreateGatewayTarget omitted it (and was accepted).
  Expected (AWS): HTTP 202 with the updated target, status progressing UPDATING/SYNCHRONIZING -> READY and targetConfiguration.http.passthrough.endpoint == 'https://example.com/mcp-v2'; credentialProviderConfigurations is optional on input (Smithy UpdateGatewayTargetRequest) and required only on output.
  Actual (emulator): HTTP 400 ValidationException — "Credential provider configurations is not defined" (botocore.errorfactory.ValidationException raised out of the call, aborting the check).

Cause 3 — bedrock-agentcore-control.gateway-target-lifecycle-ready:
  Sequence: CreateGatewayTarget -> READY -> DeleteGatewayTarget -> GetGatewayTarget correctly answers ResourceNotFoundException "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier." -> UpdateGatewayTarget on the same targetId.
  Expected (AWS): 404 ResourceNotFoundException with the same fixed sentence "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier." (no variable parts).
  Actual (emulator): 400 ValidationException. Assertion: "expected UpdateGatewayTarget on a deleted target to fail with ResourceNotFoundException, got 'ValidationException'".

## Steps to Reproduce
Cause 1:
1. Create a VPC with at least one subnet and a security group via ec2.
2. Derive a well-formed subnet id that does not exist (e.g. change the last character of a real subnet id) and confirm with ec2 DescribeSubnets that it is absent.
3. bedrock-agentcore-control CreateBrowser(name=<alnum>, networkConfiguration={networkMode:'VPC', vpcConfig:{subnets:[<absent subnet>], securityGroups:[<real sg>]}}).
4. Poll GetBrowser until status != 'CREATING'. Observe status == 'READY' and failureReason absent; AWS gives ValidationException at step 3 or terminal CREATE_FAILED with a failureReason here.

Causes 2 and 3:
1. Create an IAM role trusted by bedrock-agentcore.amazonaws.com, then CreateGateway(name=..., roleArn=..., authorizerType='AWS_IAM') and wait for status != 'CREATING'.
2. CreateGatewayTarget(gatewayIdentifier=<gatewayId>, name=<target>, targetConfiguration={http:{passthrough:{endpoint:'https://example.com/mcp-v1', protocolType:'MCP'}}}) with NO credentialProviderConfigurations; wait for status READY.
3. UpdateGatewayTarget(gatewayIdentifier=<gatewayId>, targetId=<targetId>, name=<target>, targetConfiguration={http:{passthrough:{endpoint:'https://example.com/mcp-v2', protocolType:'MCP'}}}) — again with NO credentialProviderConfigurations. Observe 400 ValidationException "Credential provider configurations is not defined" (Cause 2); AWS answers 202 and settles READY with the v2 endpoint.
4. DeleteGatewayTarget(gatewayIdentifier=<gatewayId>, targetId=<targetId>) and poll GetGatewayTarget until it answers ResourceNotFoundException.
5. Call UpdateGatewayTarget on that deleted targetId. Observe ValidationException (400) instead of ResourceNotFoundException (404) with "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier." (Cause 3).
