Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected
Issue-Key: ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected|run#6|AssertionError|assert (code, message) == (
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# SSM GetInventory: the InvalidAggregatorException message for an unsupported count aggregator is emulator-authored prose

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInventory

## Description
Cause 1 of 1: GetInventory refuses an unsupported count aggregator with the right code but the wrong message.
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm) models GetInventory with Aggregators (InventoryAggregatorList, length 1..10) and the client error InvalidAggregatorException (HTTP 400). `AWS:InstanceInformation.PlatformName` is accepted as a count aggregator, while its sibling `AWS:InstanceInformation.InstanceStatus` is refused with code `InvalidAggregatorException` and the message "Attribute <AttributeName> of type <TypeName> does not support count aggregator." — fixed text whose two variable parts are the attribute name and the inventory type name taken from the request's aggregator Expression (here InstanceStatus and AWS:InstanceInformation).
Emulator: the code is correct but the message is the generic "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported." — it names neither the attribute nor the type.
Why: an agent that counts managed nodes by aggregation has to learn from the message which attribute is unsupported so it can fall back to filtering; the emulator's text identifies nothing, and clients that match AWS's wording diverge.
Fix: the GetInventory handler's aggregator validation — build the InvalidAggregatorException message from the rejected aggregator's attribute and type name.
Traces: ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected.

## Evidence
Replayed behavioral suite, service heading `### ec2__ssm`:
- ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected — AssertionError: ('InvalidAggregatorException', "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported."). The check first confirms GetInventory(Aggregators=[{'Expression': 'AWS:InstanceInformation.PlatformName'}]) succeeds, then asserts the refusal of Aggregators=[{'Expression': 'AWS:InstanceInformation.InstanceStatus'}] equals ('InvalidAggregatorException', 'Attribute InstanceStatus of type AWS:InstanceInformation does not support count aggregator.').

Smithy: GetInventory errors include InvalidAggregatorException (client, httpError 400); Aggregators is an InventoryAggregatorList of length 1..10.

## Steps to Reproduce
Re-replay ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected. Minimal repro: ssm.get_inventory(Aggregators=[{'Expression': 'AWS:InstanceInformation.PlatformName'}]) must succeed; then ssm.get_inventory(Aggregators=[{'Expression': 'AWS:InstanceInformation.InstanceStatus'}]) must raise InvalidAggregatorException with the message 'Attribute InstanceStatus of type AWS:InstanceInformation does not support count aggregator.' — compare the returned Message.
