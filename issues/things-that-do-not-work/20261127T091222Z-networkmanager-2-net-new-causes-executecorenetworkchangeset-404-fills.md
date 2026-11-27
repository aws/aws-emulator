Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.execute-change-set-unknown-policy-version-not-found networkmanager.execute-change-set-while-core-network-updating-rejected
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.execute-change-set-unknown-policy-version-not-found|run#17|AssertionError|assert resource_id == "Policy version not found", (
Issue-Key: networkmanager.execute-change-set-while-core-network-updating-rejected|run#20|AssertionError|assert error["Message"] == "Core network state is invalid.", (
Issue-Commit: 0ad1d763d225c91fcbd1777be6c7eddb77b756de

# networkmanager: 2 net-new causes — ExecuteCoreNetworkChangeSet 404 fills ResourceId with <core-network-id>/<version> instead of "Policy version not found", and a busy core network is refused as a change-set-state error instead of "Core network state is invalid."

> _Note: this issue was identified against the code base at commit `0ad1d763d225c91fcbd1777be6c7eddb77b756de`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteCoreNetworkChangeSet

## Description
Two distinct root causes in the networkmanager ExecuteCoreNetworkChangeSet refusal path. Both are in the same handler and both concern the CONTENT of a modeled refusal (the status codes and error codes are already right); a fixer must close BOTH before submitting.

Cause 1 of 2: the ResourceNotFoundException raised for a policy version the core network does not hold carries the wrong ResourceId.
AWS: Smithy: ExecuteCoreNetworkChangeSet models ResourceNotFoundException (HTTP 404); ResourceNotFoundException.Message, .ResourceId and .ResourceType are all @required (ServerSideString). The model does not state what fills ResourceId, so: recorded behavior only — for this refusal class AWS answers HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "Resource not found." and ResourceId with the FIXED diagnostic text "Policy version not found". The whole of that ResourceId value is fixed text for this error class: it contains NO value taken from the request (no core network id, no policy version number) and no per-run value, so it is identical on every call of this class.
Emulator: status 404, code ResourceNotFoundException and message "Resource not found." all match, but ResourceId is built from the request as the composite "<CoreNetworkId>/<PolicyVersionId>" — one replay named ResourceId 'core-network-00000000000000002/41' where AWS names 'Policy version not found'. The handler passes crate::state::policy_version_resource_id(core_network_id, policy_version_id) as the ResourceId for the policy-version 404 (see Traces for the citation).
Why: ResourceId is a @required member clients read to tell WHICH lookup failed — a client (or an operator reading logs) that keys on the documented "Policy version not found" diagnostic sees an opaque composite id instead, so it cannot distinguish "this core network has no such policy version" from any other 404, and code that matches that member exactly never matches against the emulator.
Fix: the ExecuteCoreNetworkChangeSet handler's not-found branch for an unknown policy version — the ResourceNotFoundException it constructs must carry the fixed ResourceId text for this class rather than the request-derived composite resource id.
Traces: networkmanager.execute-change-set-unknown-policy-version-not-found. Handler: services/service-com-amazonaws-networkmanager-networkmanager/src/ops/execute_core_network_change_set.rs lines 173-187 (the policy_versions.get(...).ok_or_else(...) branch).

Cause 2 of 2: executing a READY_TO_EXECUTE version while the core network is mid-deployment is refused with the change-set-state message and no Fields, instead of the core-network-state message with a coreNetworkId field.
AWS: Smithy: ExecuteCoreNetworkChangeSet models ValidationException (HTTP 400) with @required Message plus optional Reason and Fields (ValidationExceptionFieldList of ValidationExceptionField{Name, Message}). The model does not state the precedence of the state checks, so: recorded behavior only — when a previous execution is still deploying (the core network has left AVAILABLE and is UPDATING), AWS refuses the next ExecuteCoreNetworkChangeSet with HTTP 400, Error.Code "ValidationException", Error.Message "Core network state is invalid." and Fields exactly [{"Name": "coreNetworkId", "Message": "Core network state is invalid."}]. Both the message and the field entry are entirely fixed text for this class — the Name is the input member name "coreNetworkId", and no id, version number or state name from the request or state appears. AWS thus validates the CORE NETWORK's state before (and in preference to) the addressed change set's state.
Emulator: the handler has no core-network-state check at all; it only compares the addressed change set's state to READY_TO_EXECUTE. Because the first execution flips the sibling version's change set out of READY_TO_EXECUTE, the second call falls into that branch and is refused HTTP 400 ValidationException with message 'Change set state for policy is invalid.', Reason "Other" and Fields absent (None). Replay: "executing version 1 during a deployment was refused with message 'Change set state for policy is invalid.'".
Why: a client that serialises deployments by catching the busy-core-network refusal (matching Message "Core network state is invalid." or reading Fields[0].Name == "coreNetworkId" to learn which input was at fault) gets a different message and an absent Fields list, so it misclassifies a transient "core network is busy deploying, retry later" condition as a permanent "this change set can never be executed" condition and gives up instead of retrying.
Fix: the ExecuteCoreNetworkChangeSet handler's validation ordering — check the core network's deployment state first and raise the core-network-state ValidationException (fixed message plus the single coreNetworkId field entry) before falling through to the change-set-state ValidationException.
Traces: networkmanager.execute-change-set-while-core-network-updating-rejected. Handler: services/service-com-amazonaws-networkmanager-networkmanager/src/ops/execute_core_network_change_set.rs lines 199-210 (the single change_set_state != CHANGE_SET_READY_TO_EXECUTE branch).

Neither cause is covered by an open issue: the only other open networkmanager issue is 20261024T052551Z-networkmanager-listcorenetworks-stub.md (ListCoreNetworks scaffold stub), which concerns a different operation.

## Evidence
Cause 1 — networkmanager.execute-change-set-unknown-policy-version-not-found:
  Call: ExecuteCoreNetworkChangeSet(CoreNetworkId=<existing core network>, PolicyVersionId=<a version it never held>)
  Expected (AWS): HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "Resource not found.", ResourceId "Policy version not found" (fixed text for this class).
  Actual (emulator): HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "Resource not found." (all correct), but ResourceId = "<CoreNetworkId>/<PolicyVersionId>".
  Replay assertion: AssertionError: the refusal named ResourceId 'core-network-00000000000000002/41', expected 'Policy version not found'

Cause 2 — networkmanager.execute-change-set-while-core-network-updating-rejected:
  Setup: two policy versions both READY_TO_EXECUTE; ExecuteCoreNetworkChangeSet is called on the second (deployment starts, core network leaves AVAILABLE), then on the first.
  Expected (AWS): HTTP 400, Error.Code "ValidationException", Error.Message "Core network state is invalid.", Fields == [{"Name": "coreNetworkId", "Message": "Core network state is invalid."}].
  Actual (emulator): HTTP 400, Error.Code "ValidationException" (both correct), Error.Message "Change set state for policy is invalid.", Reason "Other", Fields absent.
  Replay assertion: AssertionError: executing version 1 during a deployment was refused with message 'Change set state for policy is invalid.'

## Steps to Reproduce
Cause 1:
1. CreateGlobalNetwork; wait until DescribeGlobalNetworks reports State AVAILABLE.
2. CreateCoreNetwork in that global network; wait until GetCoreNetwork reports State AVAILABLE.
3. PutCoreNetworkPolicy with a valid 2021.12 document; wait until GetCoreNetworkPolicy reports ChangeSetState READY_TO_EXECUTE for the returned version V.
4. Call ExecuteCoreNetworkChangeSet(CoreNetworkId=<cn>, PolicyVersionId=V+40) and inspect the ClientError response: ResponseMetadata.HTTPStatusCode, Error.Code, Error.Message and the top-level ResourceId member.
5. Observe ResourceId is "<cn-id>/<V+40>" instead of "Policy version not found".

Cause 2:
1. CreateGlobalNetwork and CreateCoreNetwork as above, waiting for AVAILABLE.
2. PutCoreNetworkPolicy twice (second document adds a segment); wait until BOTH versions report ChangeSetState READY_TO_EXECUTE.
3. ExecuteCoreNetworkChangeSet on the SECOND version (this starts a deployment and moves the core network out of AVAILABLE).
4. Immediately call ExecuteCoreNetworkChangeSet on the FIRST version and inspect the ClientError: Error.Message and the top-level Fields member.
5. Observe Message "Change set state for policy is invalid." with no Fields, instead of "Core network state is invalid." with Fields [{"Name": "coreNetworkId", "Message": "Core network state is invalid."}].
