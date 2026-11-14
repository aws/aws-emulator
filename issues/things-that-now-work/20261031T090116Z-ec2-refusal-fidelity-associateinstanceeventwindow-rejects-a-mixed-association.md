Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-associate-instance-event-window ec2.op-contract-delete-route-server-endpoint ec2.route-server-endpoint-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-associate-instance-event-window|run#12|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-delete-route-server-endpoint|run#29|AssertionError|assert dead_msg == expected_msg, f"expected message {expected_msg!r}, got {dead_msg!r}"
Issue-Key: ec2.route-server-endpoint-lifecycle-transitions|run#28|AssertionError|assert err.get("Message") == f"Route Server Endpoint {ep_id} is in an invalid state.", (
Issue-Commit: 435579c3f4f417d2ee68a6d86543c2b3ca676fd8

# EC2 refusal fidelity: AssociateInstanceEventWindow rejects a mixed association target with the wrong error code, and the route-server-endpoint IncorrectState message is emulator-authored (2 causes)

> _Note: this issue was identified against the code base at commit `435579c3f4f417d2ee68a6d86543c2b3ca676fd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateInstanceEventWindow, DeleteRouteServerEndpoint

## Description
Two net-new root causes in the EC2 emulator's refusal surface. Both operations are implemented and answer the right HTTP status, but one picks the wrong error code and the other writes its own message text where AWS has fixed wording. Neither is covered by an existing issue: the replay report marked neither check ALREADY FILED nor OWNED. The open issues 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md and 20261029T215142Z-ec2-refusal-messages-reword-aws-text-across-eight.md describe the same *class* of defect on other operation families, and neither carries these two replay keys, so this issue is their work item.

Cause 1 of 2 — AssociateInstanceEventWindow answers InvalidParameterCombination for a mutually exclusive association target (failing check: ec2.op-contract-associate-instance-event-window).
AWS: AssociateInstanceEventWindow takes one AssociationTarget (InstanceEventWindowAssociationRequest) whose InstanceIds / InstanceTags / DedicatedHostIds are mutually exclusive — "Only one type of target (instance ID, instance tag, or Dedicated Host ID) can be associated with an event window" (Smithy: ec2#AssociateInstanceEventWindow input AssociationTarget -> InstanceEventWindowAssociationRequest; the operation models no errors, as all EC2 query-protocol errors are unmodeled, so the error identity is recorded behavior only). The recorded rejection of a request that names two kinds at once is HTTP 400 with error code InvalidParameterValue and message "Target Associations must be composed entirely of Instance IDs, Instance Tags, or Dedicated Host IDs." — that message is fixed text with no variable parts: AWS names the three admissible target kinds, not the ids the request supplied. AWS classifies this as an invalid *value* of one structure member, not an invalid *combination* of independent request parameters.
Emulator: the same request is refused with error code InvalidParameterCombination. The replayed check asserts the code first, so only the code is proven to diverge here; the message text was never compared.
Why: a client that branches on the error code — retry/repair logic that treats InvalidParameterValue as "fix this field" and InvalidParameterCombination as "drop one of these parameters", and test suites that assert the documented code — takes the wrong branch, and an error-code allow-list built against AWS never matches.
Fix: the refusal classification in this operation's validation — the target-kind check that AssociateInstanceEventWindow performs before touching state must raise the InvalidParameterValue class (and keep AWS's message text) for both the one-request case and the case where the window already holds a different target kind; today both paths produce the InvalidParameterCombination class.
Traces: ec2.op-contract-associate-instance-event-window. Handler: services/service-com-amazonaws-ec2-amazonec2/src/ops/associate_instance_event_window.rs, lines 165-197 (`require_one_event_window_target_kind(...)` at lines 168-169 and `Scaffolded::invalid_parameter_combination(ONE_TARGET_TYPE_MESSAGE)` at lines 194-196).

Cause 2 of 2 — REOPENS 20261023T102725Z-ec2-deleterouteserverendpoint-stub.md: its fix left the call refused with emulator-authored wording. A repeat DeleteRouteServerEndpoint on a terminal endpoint is refused with the right class but the wrong message (failing checks: ec2.op-contract-delete-route-server-endpoint, ec2.route-server-endpoint-lifecycle-transitions).
AWS: DeleteRouteServerEndpoint names one endpoint by RouteServerEndpointId (Smithy: ec2#DeleteRouteServerEndpoint input RouteServerEndpointId required, output RouteServerEndpoint; no modeled errors — recorded behavior only). A delete aimed at an id that still resolves but has reached its terminal state is refused HTTP 400 IncorrectState (distinct from the InvalidRouteServerEndpointId.NotFound an id that never existed gets), with message "Route Server Endpoint <RouteServerEndpointId> is in an invalid state." The fixed text is "Route Server Endpoint " ... " is in an invalid state."; the only variable part is the endpoint id taken from the request. AWS does NOT name the state the resource is in.
Emulator: the error code is right (IncorrectState) but the message is emulator-authored: "route-server-endpoint '<id>' is in the 'deleted' state." — it lower-cases and hyphenates the resource type, quotes the id, and adds the current state, which AWS's template has no slot for.
Why: callers and operators match on the message (log scrapers, assertions, support tooling, and SDK retry policies that inspect message text) and get no match; the extra state token also leaks an internal state name as if it were part of AWS's contract.
Fix: the message template behind this refusal — the shared incorrect-state refusal helper this operation calls — must emit AWS's wording for route server endpoints, with the request's endpoint id interpolated and no state token. Keep the code (IncorrectState) and the status (400) as they are, and keep the NotFound-vs-IncorrectState split, which the replay confirms is already right.
Traces: ec2.op-contract-delete-route-server-endpoint, ec2.route-server-endpoint-lifecycle-transitions. Handler: services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_route_server_endpoint.rs, lines 166-181 (`state::incorrect_state_of(state::RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT, &input.route_server_endpoint_id, &current_state)`).

## Evidence
Cause 1 — ec2.op-contract-associate-instance-event-window [mismatch], replay detail:
  File "check_ec2_op_contract_associate_instance_event_window.py", line 48, in run
    assert code == "InvalidParameterValue", f"unexpected error code: {code}"
  AssertionError: unexpected error code: InvalidParameterCombination
The request replayed: associate_instance_event_window(InstanceEventWindowId=<window>, AssociationTarget={"InstanceIds": [<instance>], "InstanceTags": [{"Key": ..., "Value": ...}]}).
Expected (AWS): HTTP 400, code InvalidParameterValue, message "Target Associations must be composed entirely of Instance IDs, Instance Tags, or Dedicated Host IDs." (all fixed text).
Actual (emulator): code InvalidParameterCombination.

Cause 2 — ec2.op-contract-delete-route-server-endpoint [mismatch], replay detail:
  File "check_ec2_op_contract_delete_route_server_endpoint.py", line 103, in run
    assert dead_msg == expected_msg
  AssertionError: expected message 'Route Server Endpoint rse-0000000000000002e is in an invalid state.', got "route-server-endpoint 'rse-0000000000000002e' is in the 'deleted' state."
Same divergence from ec2.route-server-endpoint-lifecycle-transitions [mismatch]:
  File "check_ec2_route_server_endpoint_lifecycle_transitions.py", line 106, in run
    assert err.get("Message") == f"Route Server Endpoint {ep_id} is in an invalid state."
  AssertionError: unexpected message: "route-server-endpoint 'rse-0000000000000002e' is in the 'deleted' state."
Both checks assert err["Code"] == "IncorrectState" BEFORE the message, and both got past that assertion, so the code and status are right and the message text is the whole divergence. (The endpoint id shown, rse-0000000000000002e, is this run's generated id — it is the message's variable part, not part of the template.)

## Steps to Reproduce
Cause 1 (ec2.op-contract-associate-instance-event-window):
1. ec2 RunInstances to get an instance id; ec2 CreateInstanceEventWindow with a weekly TimeRange to get an InstanceEventWindowId.
2. ec2 AssociateInstanceEventWindow(InstanceEventWindowId=<window>, AssociationTarget={InstanceIds:[<instance>]}) — succeeds, association reflected.
3. ec2 AssociateInstanceEventWindow(InstanceEventWindowId=<window>, AssociationTarget={InstanceIds:[<instance>], InstanceTags:[{Key:'k',Value:'v'}]}).
4. Observe the error: emulator answers code InvalidParameterCombination; AWS answers 400 InvalidParameterValue with message "Target Associations must be composed entirely of Instance IDs, Instance Tags, or Dedicated Host IDs."

Cause 2 (ec2.op-contract-delete-route-server-endpoint, ec2.route-server-endpoint-lifecycle-transitions):
1. ec2 CreateVpc + CreateSubnet; ec2 CreateRouteServer(AmazonSideAsn=...) and poll DescribeRouteServers until State == available.
2. ec2 AssociateRouteServer(RouteServerId, VpcId) and poll GetRouteServerAssociations until the association State == associated.
3. ec2 CreateRouteServerEndpoint(RouteServerId, SubnetId) and poll DescribeRouteServerEndpoints until State == available.
4. ec2 DeleteRouteServerEndpoint(RouteServerEndpointId) and poll DescribeRouteServerEndpoints until State == deleted.
5. ec2 DeleteRouteServerEndpoint(RouteServerEndpointId) again on that terminal id.
6. Observe the error message: emulator answers "route-server-endpoint '<id>' is in the 'deleted' state."; AWS answers "Route Server Endpoint <id> is in an invalid state." (code IncorrectState, HTTP 400, in both).
