Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: globalaccelerator.custom-routing-listener-delete-rejects-associated-endpoint-group
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator.custom-routing-listener-delete-rejects-associated-endpoint-group|run#16|AssertionError|assert code == "AssociatedEndpointGroupFoundException", (
Issue-Commit: 1e75e4fa76b037c3133a2c84a8255f6a8754d328

# DeleteCustomRoutingListener answers 501 NotImplemented instead of AssociatedEndpointGroupFoundException / deleting the listener

> _Note: this issue was identified against the code base at commit `1e75e4fa76b037c3133a2c84a8255f6a8754d328`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomRoutingListener

## Description
Cause 1 of 1: DeleteCustomRoutingListener is an unimplemented scaffold stub, so both its refusal path and its success path diverge. Failing check ids: globalaccelerator.custom-routing-listener-delete-rejects-associated-endpoint-group.

AWS: Smithy model (services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json) declares DeleteCustomRoutingListener with input ListenerArn (GenericString, @length 0..255, required), output Unit, and modeled errors AssociatedEndpointGroupFoundException (HTTP 400), ListenerNotFoundException (HTTP 404), InvalidArgumentException (HTTP 400), InternalServiceErrorException (HTTP 500). The model documents AssociatedEndpointGroupFoundException as "The listener that you specified has an endpoint group associated with it. You must remove all dependent resources from a listener before you can delete it." So: when the named custom-routing listener still has at least one custom routing endpoint group associated with it, AWS refuses the delete with HTTP 400, error code AssociatedEndpointGroupFoundException, and leaves the listener and its endpoint group untouched (a following DescribeCustomRoutingListener and ListCustomRoutingEndpointGroups still return them). Once the endpoint group is deleted, the same DeleteCustomRoutingListener call succeeds with an empty 200 body, after which DescribeCustomRoutingListener on that ARN fails with HTTP 404 ListenerNotFoundException, while the parent accelerator still answers DescribeCustomRoutingAccelerator.

Error message wording (recorded behavior, as the check asserts it): for the refusal the message is "Cannot delete listener <ListenerArn> because an associated endpoint group was found." — the fixed text is "Cannot delete listener " ... " because an associated endpoint group was found." and the only variable part is the listener ARN taken verbatim from the request's ListenerArn. For the post-delete describe the message is "Listener <ListenerArn> not found." — fixed text "Listener " ... " not found.", variable part the requested ListenerArn. No ARN, account id, region or generated id from any single run is part of the template.

Emulator: the handler is the generated scaffold. services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/delete_custom_routing_listener.rs line 174 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteCustomRoutingListener"))`, which serializes as HTTP 501 with error code `NotImplemented`. The replay shows the call made with a listener that still owns an endpoint group coming back as code 'NotImplemented' where 'AssociatedEndpointGroupFoundException' was expected. Because the call never succeeds, the whole positive control after it is unreachable too: the listener can never be deleted, so the subsequent ListenerNotFoundException-on-describe behavior is also not exercisable. Note the surrounding custom-routing operations DO work in this emulator — CreateCustomRoutingAccelerator, CreateCustomRoutingListener, CreateCustomRoutingEndpointGroup, DescribeCustomRoutingAccelerator, DescribeCustomRoutingAcceleratorAttributes, DescribeCustomRoutingListener, DescribeCustomRoutingEndpointGroup and ListCustomRoutingEndpointGroups all answered correctly before the failing call — so this is the one missing link in the custom-routing lifecycle.

Why: a client cannot tear down a custom routing accelerator at all. 501/NotImplemented is not a modeled error for this operation, so an SDK surfaces it as an unmodeled/unknown service error rather than the typed AssociatedEndpointGroupFoundException clients catch to drive "delete children first" cleanup. Teardown code and CloudFormation-style stack deletion loops see a non-retryable unknown failure, leak the listener and accelerator, and the dependency-ordering contract (children before parent) can never be validated. Conversely, nothing can confirm a refused delete is side-effect free, because no delete ever runs.

Fix: implement the handler in services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/delete_custom_routing_listener.rs (the only file for this operation) and remove the SCAFFOLDED marker at its top. It needs: a state lookup of the listener by ARN (404 ListenerNotFoundException when absent), an association check against the custom routing endpoint groups held for that listener in crate::state (400 AssociatedEndpointGroupFoundException when non-empty, with no mutation), and removal of the listener from the parent accelerator's listener set on success, returning the empty Unit output with status 200. Error construction goes through DeleteCustomRoutingListenerErrorModeled::{AssociatedEndpointGroupFoundException, ListenerNotFoundException}.into().

Traces: globalaccelerator.custom-routing-listener-delete-rejects-associated-endpoint-group — assertion at check_globalaccelerator_custom_routing_listener_delete_rejects_associated_endpoint_group.py line 141. Stub site: services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/delete_custom_routing_listener.rs lines 3-6 (SCAFFOLDED marker) and 174 (the not_implemented return).

## Evidence
Expected (AWS / modeled): DeleteCustomRoutingListener on a custom routing listener that still has an associated custom routing endpoint group -> HTTP 400, Error.Code = "AssociatedEndpointGroupFoundException", Error.Message = "Cannot delete listener <ListenerArn> because an associated endpoint group was found." (fixed text with the request's ListenerArn substituted); listener and endpoint group unchanged afterwards. After the endpoint group is deleted, the same call -> 200 empty body; DescribeCustomRoutingListener on that ARN -> HTTP 404 "ListenerNotFoundException", message "Listener <ListenerArn> not found."; DescribeCustomRoutingAccelerator on the parent still succeeds.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented". Replay output: AssertionError: deleting a listener with an associated endpoint group returned 'NotImplemented', expected 'AssociatedEndpointGroupFoundException' (check_globalaccelerator_custom_routing_listener_delete_rejects_associated_endpoint_group.py line 141). 'NotImplemented' is not among the operation's modeled errors. Handler source: delete_custom_routing_listener.rs:174 `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteCustomRoutingListener"))`.

## Steps to Reproduce
1. CreateCustomRoutingAccelerator(Name=<unique>, Enabled=false, IdempotencyToken=<uuid>) -> capture AcceleratorArn.
2. CreateCustomRoutingListener(AcceleratorArn=<arn>, PortRanges=[{FromPort:1024, ToPort:65535}], IdempotencyToken=<uuid>) -> capture ListenerArn.
3. CreateCustomRoutingEndpointGroup(ListenerArn=<listener>, EndpointGroupRegion=<region>, DestinationConfigurations=[{FromPort:80, ToPort:80, Protocols:["TCP"]}], IdempotencyToken=<uuid>) -> capture EndpointGroupArn.
4. Call DeleteCustomRoutingListener(ListenerArn=<listener>). Observe HTTP 501 / NotImplemented instead of HTTP 400 / AssociatedEndpointGroupFoundException with message "Cannot delete listener <listener> because an associated endpoint group was found."
5. Then DeleteCustomRoutingEndpointGroup(EndpointGroupArn=<group>) and retry DeleteCustomRoutingListener(ListenerArn=<listener>): it still answers 501 instead of succeeding, and DescribeCustomRoutingListener(ListenerArn=<listener>) therefore never reaches the 404 ListenerNotFoundException state.

Replay: run_behavioral_suite and re-run check globalaccelerator.custom-routing-listener-delete-rejects-associated-endpoint-group.
