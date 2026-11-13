Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-w22n-traj-inventory-aggregator-count
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-w22n-traj-inventory-aggregator-count ssm-w22n-traj-inventory-filter-mistakes ssm-patch-baseline-and-compliance-workflow
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetInventory|4xx->4xx|$.Message
Issue-Key: ssm|GetInventory|4xx->4xx|$.__type
Issue-Key: ssm|GetInventory|4xx->2xx|$.Message
Issue-Key: ssm|GetInventory|4xx->2xx|$.__type
Issue-Key: ssm|GetInventory|4xx->2xx|status
Issue-Key: ssm|GetInventory|2xx->2xx|$.Entities[*]
Issue-Key: ssm|DescribePatchBaselines|2xx->2xx|$.BaselineIdentities[*]
Issue-Commit: d6956c38e66386622390b3c589013c23dfd2f689

# ssm inventory and patch-baseline reads: GetInventory misclassifies validation errors and read responses drop members

> _Note: this issue was identified against the code base at commit `d6956c38e66386622390b3c589013c23dfd2f689`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInventory

## Description
Five net-new causes on ssm read operations. Four are in GetInventory, which collapses three distinct modeled validation failures into a single InvalidAggregatorException with one generic message and returns an empty result set where AWS returns a result entity; the fifth is in DescribePatchBaselines, which drops a modeled response member. THIS ISSUE HAS 5 CAUSES — all five must be fixed and re-replayed before it is closed.

Cause 1 of 5: GetInventory aggregator-attribute rejection carries a generic message instead of the attribute-specific one.
AWS: Smithy (com.amazonaws.ssm) GetInventory.Aggregators -> InventoryAggregatorList (@length 1..10) of InventoryAggregator, whose Expression member is the "inventory type and attribute name for aggregation". When the aggregator expression names an attribute of an inventory type that cannot be counted, AWS answers HTTP 400 with __type InvalidAggregatorException and a message that identifies the offending attribute and type. Message template (fixed text, from the recording): "Attribute <AttributeName> of type <TypeName> does not support count aggregator." The only variable parts are <AttributeName> and <TypeName>, both taken from the request's aggregator Expression; everything else is fixed. The recorded example is InstanceStatus / AWS:InstanceInformation, which fills the template and is not part of it.
Emulator: HTTP 400 and __type InvalidAggregatorException (correct class), but the message is the shape's own doc string, "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported." — it names neither the attribute nor the type.
Why: a client cannot tell WHICH aggregator in a list of up to ten was rejected. Tooling and operators that parse or surface the attribute name from this message get nothing actionable, and a retry loop that strips the named attribute cannot know what to strip.
Fix: GetInventory's input validation / error construction in the ssm handler — the aggregator check must build the message from the request's Expression rather than emitting a constant.
Traces: ssm-w22n-traj-inventory-aggregator-count#task#1 (representative), ssm-w22n-traj-inventory-aggregator-count#task#2, ssm-w22n-traj-inventory-filter-mistakes#task#3, ssm-w22n-traj-inventory-filter-mistakes#task#5.

Cause 2 of 5: an invalid inventory TYPE NAME is reported as InvalidAggregatorException instead of InvalidTypeNameException.
AWS: GetInventory models InvalidTypeNameException (client error, HTTP 400, awsQueryError code InvalidTypeName, doc "The parameter type name isn't valid.") as a DISTINCT error from InvalidAggregatorException. Inventory type names are constrained by InventoryItemTypeName (@length 1..100, @pattern ^(AWS|Custom):.*$). When the type name named in the request is not a real inventory type, AWS answers HTTP 400 with __type InvalidTypeNameException and the message template "Inventory type name [<TypeName>] is invalid. Type name is case sensitive, such as AWS:APpLICation is invalid; Type name must start with either AWS: or Custom:;  Custom inventory type name must satisfy regular expression pattern: ^(Custom:)([a-zA-Z0-9]{3,93})$" — all fixed text except <TypeName>, which is echoed from the request (the recording's AWS:Foo is just the value that filled it). Note the literal double space before "Custom inventory type name".
Emulator: HTTP 400 with __type InvalidAggregatorException and the generic message "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported." — wrong error class AND wrong message.
Why: clients branch on the error code. Code that catches InvalidTypeNameException to correct a mistyped or unsupported inventory type never catches it, so a recoverable type-name mistake surfaces as an unrelated aggregator fault and the per-type retry path is dead. The explanatory text telling the caller the AWS:/Custom: rule is also lost.
Fix: GetInventory's validation in the ssm handler must validate the inventory type name as its own check, ahead of (and separately from) the aggregator check, and raise InvalidTypeNameException with the templated message.
Traces: ssm-w22n-traj-inventory-aggregator-count#task#3, ssm-w22n-traj-inventory-filter-mistakes#task#4.

Cause 3 of 5: an unsupported GetInventory ResultAttributes type name is not validated at all — the request succeeds with HTTP 200.
AWS: GetInventory.ResultAttributes -> ResultAttributeList (@length 1..1) of ResultAttribute, whose required TypeName member documents "Valid value: AWS:InstanceInformation. Default Value: AWS:InstanceInformation." GetInventory models InvalidResultAttributeException (client error, HTTP 400, awsQueryError code InvalidResultAttribute, doc "The specified inventory item result attribute isn't valid."). For a result type name outside the supported set, AWS answers HTTP 400 with __type InvalidResultAttributeException and the message template "Result type name <TypeName> is not supported. Supported type names are: AWS:InstanceInformation, AWS:PatchSummary. Default result type name is AWS:InstanceInformation." — the only variable part is <TypeName> echoed from the request (recorded as AWS:WindowsUpdate); the supported-name list and the default-name sentence are fixed text.
Emulator: HTTP 200 with a normal (empty) result body. No validation of ResultAttributes[].TypeName happens at all, and __type and Message are absent.
Why: this is a silent wrong-answer, the worst failure mode here. A caller that asks for an unsupported result attribute is told its query SUCCEEDED and returned nothing, so it concludes the account has no inventory of that type instead of learning its request was invalid. Error-path tests assert a 400 and get a 200.
Fix: GetInventory's input validation in the ssm handler — add the ResultAttributes type-name allow-list check and raise InvalidResultAttributeException.
Traces: ssm-w22n-traj-inventory-aggregator-count#task#4, ssm-w22n-traj-inventory-filter-mistakes#task#6.

Cause 4 of 5: a valid GetInventory query that matches no nodes returns an empty Entities list instead of a result entity for the queried type.
AWS: recorded behavior only — the Smithy model constrains the shapes (GetInventoryResult.Entities -> InventoryResultEntityList of InventoryResultEntity{Id, Data: InventoryResultItemMap}) but says nothing about the empty-result projection. The recording shows that for a valid query AWS returns ONE result entity whose Data map carries an entry keyed by the queried inventory type name with an empty Content list — i.e. Entities[0] = {"Data": {"<TypeName>": {"Content": []}}} — rather than an empty Entities list. The type name key comes from the request; the empty Content list is the fixed shape of a no-match answer.
Emulator: Entities is empty, so Entities[0] is absent (reported as missing, got None) while the status is 200 in both.
Why: a client that indexes Entities[0] and reads Data[typeName].Content to enumerate inventory rows raises an index error against the emulator instead of iterating an empty row set. "Query succeeded, this type has no rows" and "query returned no entities" are different answers, and only the first is AWS's.
Fix: GetInventory's response projection / serializer in the ssm handler — emit the per-type result entity with an empty Content list when the query is valid and matches nothing.
Traces: ssm-w22n-traj-inventory-aggregator-count#task#5.

Cause 5 of 5: DescribePatchBaselines omits BaselineDescription from every returned baseline identity.
AWS: Smithy DescribePatchBaselines returns DescribePatchBaselinesResult.BaselineIdentities -> PatchBaselineIdentityList of PatchBaselineIdentity, which models BaselineDescription (target BaselineDescription, @length 1..1024, doc "The description of the patch baseline.") alongside BaselineId, BaselineName, OperatingSystem and DefaultBaseline. A baseline that has a description reports it in this listing, and the value is whatever description that baseline currently carries in state (set at CreatePatchBaseline and replaced by UpdatePatchBaseline).
Emulator: HTTP 200 and the entry is present, but it carries only BaselineId, BaselineName, OperatingSystem and DefaultBaseline — the BaselineDescription member is dropped, even for a baseline whose description was just written by an earlier step in the same fixture. (The recorded value 'updated probe baseline' is the fixture's own string, not a requirement.)
Why: the description is the only human-readable field distinguishing baselines that share an operating system, so console-like listings and any client that reconciles a baseline's description through the list API see it as unset and can conclude a description write was lost — or overwrite it with empty on a read-modify-write.
Fix: the DescribePatchBaselines response projection in the ssm handler — the PatchBaselineIdentity projection must carry the description from the baseline's stored state. The write path is evidently storing it (the create/update steps pass), so this is a read-side projection gap, not a state gap.
Traces: ssm-patch-baseline-and-compliance-workflow#task#4.

No existing open issue covers any of these five. The nearest open ssm/inventory issues are 20261103T215115Z-ssm-getinventoryschema-stub.md (a different operation, GetInventorySchema, unimplemented), 20261109T022325Z-cloudformation-ec2-ssm-aws-gathersoftwareinventory-never-produces-an.md and 20261108T004524Z-ec2-ssm-an-instance-whose-profile-role-gains.md (both about inventory DATA never being produced for a real EC2 instance via the agent/ping path, not about GetInventory request validation, error classification or response projection). None of them carries the replay keys above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). Per cause:

Cause 1 — GetInventory, fixture ssm-w22n-traj-inventory-aggregator-count step task#1 (+3 more affected steps). Status 400 expected, 400 actual (status agrees). $.Message value_mismatch:
  expected: 'Attribute InstanceStatus of type AWS:InstanceInformation does not support count aggregator.'
  actual:   "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported."
Fixed text of the expected message: "Attribute " / " of type " / " does not support count aggregator."; variables: the attribute name and the inventory type name from the request's aggregator Expression.

Cause 2 — GetInventory, fixture ssm-w22n-traj-inventory-aggregator-count step task#3 (+1 more). Status 400 expected, 400 actual.
  $.__type value_mismatch: expected 'InvalidTypeNameException', got 'InvalidAggregatorException'.
  $.Message value_mismatch: expected 'Inventory type name [AWS:Foo] is invalid. Type name is case sensitive, such as AWS:APpLICation is invalid; Type name must start with either AWS: or Custom:;  Custom inventory type name must satisfy regular expression pattern: ^(Custom:)([a-zA-Z0-9]{3,93})$', got "The specified aggregator isn't valid for the group type. Verify that the aggregator you provided is supported."
Only the bracketed type name is variable (echoed from the request); the rest, including the double space before "Custom inventory type name", is fixed.

Cause 3 — GetInventory, fixture ssm-w22n-traj-inventory-aggregator-count step task#4 (+1 more).
  status: expected 400, got 200.
  $.__type: missing — expected 'InvalidResultAttributeException', got None.
  $.Message: missing — expected 'Result type name AWS:WindowsUpdate is not supported. Supported type names are: AWS:InstanceInformation, AWS:PatchSummary. Default result type name is AWS:InstanceInformation.', got None.
Only the result type name is variable; the supported-names list and default-name sentence are fixed text.

Cause 4 — GetInventory, fixture ssm-w22n-traj-inventory-aggregator-count step task#5. Status 200 expected, 200 actual.
  $.Entities[0]: missing — expected {'Data': {'AWS:InstanceInformation': {'Content': []}}}, got None (Entities came back empty).

Cause 5 — DescribePatchBaselines, fixture ssm-patch-baseline-and-compliance-workflow step task#4. Status 200 expected, 200 actual.
  $.BaselineIdentities[0]: expected {'BaselineDescription': 'updated probe baseline', 'BaselineId': ..., 'BaselineName': ..., 'DefaultBaseline': False, 'OperatingSystem': 'AMAZON_LINUX_2'}, got the same entry WITHOUT BaselineDescription. The id and name in the recording are generated per run and are not requirements; the presence of the BaselineDescription member is.

Model evidence used (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json):
- GetInventory errors include InvalidAggregatorException, InvalidTypeNameException, InvalidResultAttributeException, InvalidFilter, InvalidInventoryGroupException, InvalidNextToken — all client, HTTP 400 — so the three faults above are three distinct modeled classes, not one.
- ResultAttribute.TypeName required, @length 1..100, @pattern ^(AWS|Custom):.*$, doc "Valid value: AWS:InstanceInformation. Default Value: AWS:InstanceInformation."
- InventoryAggregator.Expression doc "The inventory type and attribute name for aggregation."
- PatchBaselineIdentity.BaselineDescription target BaselineDescription, @length 1..1024, doc "The description of the patch baseline."
- The emulator's generic aggregator message is verbatim the InvalidAggregatorException shape doc, which is why it appears for all three faults.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus against trace set simparity-wire-aws-v1), or drive the steps directly against the ssm endpoint:

Cause 1: POST AmazonSSM.GetInventory with an Aggregators entry whose Expression names a non-countable attribute of an inventory type (recorded: count on InstanceStatus of AWS:InstanceInformation). Expect 400 / InvalidAggregatorException with a message naming that attribute and type; observe the constant "The specified aggregator isn't valid for the group type..." message. Fixture ssm-w22n-traj-inventory-aggregator-count step task#1 (also task#2; ssm-w22n-traj-inventory-filter-mistakes task#3 and task#5).

Cause 2: POST AmazonSSM.GetInventory naming an inventory type that does not exist (recorded: AWS:Foo). Expect 400 / InvalidTypeNameException with the "Inventory type name [...] is invalid..." message; observe 400 / InvalidAggregatorException with the generic aggregator message. Fixture ssm-w22n-traj-inventory-aggregator-count step task#3 (also ssm-w22n-traj-inventory-filter-mistakes task#4).

Cause 3: POST AmazonSSM.GetInventory with ResultAttributes=[{TypeName: <unsupported type>}] (recorded: AWS:WindowsUpdate). Expect 400 / InvalidResultAttributeException with the "Result type name ... is not supported..." message; observe HTTP 200 with an ordinary empty result. Fixture ssm-w22n-traj-inventory-aggregator-count step task#4 (also ssm-w22n-traj-inventory-filter-mistakes task#6).

Cause 4: POST AmazonSSM.GetInventory with a VALID query that matches no managed nodes. Expect 200 with Entities[0].Data[<TypeName>].Content == []; observe 200 with Entities empty. Fixture ssm-w22n-traj-inventory-aggregator-count step task#5.

Cause 5: CreatePatchBaseline, then UpdatePatchBaseline giving the baseline a description, then DescribePatchBaselines. Expect the matching BaselineIdentities entry to carry BaselineDescription equal to the stored description; observe the member absent. Fixture ssm-patch-baseline-and-compliance-workflow step task#4.

All five steps are in the same replayed corpus, so a single replay after the fix re-exercises every cause (the issue's trace_ids list names each one's fixture#step).
