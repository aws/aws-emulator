Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-policy-tagging
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-policy-tagging resiliencehubv2-policy-validation-and-conflict-errors resiliencehubv2-system-association-and-delete-conflict resiliencehubv2-service-function-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|CreatePolicy|2xx->2xx|$.policy.createdAt
Issue-Key: resiliencehubv2|CreatePolicy|2xx->2xx|$.policy.updatedAt
Issue-Key: resiliencehubv2|GetPolicy|2xx->2xx|$.policy.createdAt
Issue-Key: resiliencehubv2|GetPolicy|2xx->2xx|$.policy.updatedAt
Issue-Key: resiliencehubv2|UpdateService|2xx->2xx|$.service.createdAt
Issue-Key: resiliencehubv2|UpdateService|2xx->2xx|$.service.dependencyDiscovery
Issue-Key: resiliencehubv2|UpdateService|2xx->2xx|$.service.rerunAssessment
Issue-Key: resiliencehubv2|UpdateService|2xx->2xx|$.service.tags
Issue-Key: resiliencehubv2|UpdateService|2xx->2xx|$.service.updatedAt
Issue-Key: resiliencehubv2|GetPolicy|2xx->2xx|$.policy.tags.team
Issue-Key: resiliencehubv2|ListServiceFunctions|2xx->2xx|$.serviceFunctions[*].createdAt
Issue-Key: resiliencehubv2|ListServiceFunctions|2xx->2xx|$.serviceFunctions[*].criticality
Issue-Key: resiliencehubv2|ListServiceFunctions|2xx->2xx|$.serviceFunctions[*].updatedAt
Issue-Key: resiliencehubv2|ListServiceFunctions|2xx->2xx|$.serviceFunctions[*]
Issue-Related: 20261104T144946Z-resiliencehubv2-policy-lifecycle-responses-diverge-from-aws-empty-tags.md
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# resiliencehubv2 read projections diverge from AWS: dropped members, misserialized timestamps and state that never changes

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Wire replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout found SIX distinct net-new causes in what resiliencehubv2's SUCCESS bodies project for policies, services and service functions. All six must be closed before this issue is submitted. Cause 2 is the same defect the open issue "Policy lifecycle responses diverge from AWS: empty tags dropped, multiAz not reflected, deleted policy still served 200" (20261104T144946Z-resiliencehubv2-policy-lifecycle-responses-diverge-from-aws-empty-tags.md) describes, but that issue's replay key covers only the resiliencehubv2-policy-lifecycle fixture; the two steps named in Cause 2 are in a DIFFERENT fixture and are re-replayed by nothing, which is why they are filed here. Causes 4, 5 and 6 are read-path divergences whose proximate root is an operation that is still a scaffold stub; each names the stub issue that owns the 501 itself, and what is filed here is that the READ projection must reflect the mutation once the stub is implemented.

=== Cause 1 of 6: Timestamp members are serialized in a form AWS does not emit ===
AWS: this service is restJson1 and Policy$createdAt / Policy$updatedAt, Service$createdAt / Service$updatedAt and ServiceFunction$createdAt / ServiceFunction$updatedAt all target the prelude smithy.api#Timestamp with NO @timestampFormat override, so the wire form is restJson1's default epoch-seconds number. The recorded responses assert the WIRE SHAPE of these members but not their value (the corpus marks them '<shaped-not-asserted>', i.e. the clock reading is explicitly not a requirement), and the emulator's serialization fails that shape assertion. Hypothesis: AWS emits epoch-seconds with at most millisecond resolution (no more than three fractional digits), so a full-precision float64 is not a value AWS's serializer can produce.
Emulator: emits a raw float64 epoch-seconds with sub-millisecond fractional digits (seven fractional digits in every observed body) for every one of those members, across CreatePolicy, GetPolicy, UpdateService and ListServiceFunctions.
Why: a client that round-trips, formats, compares or hashes these timestamps sees a precision AWS never produces; strict deserializers and recorded-response fixtures keyed on AWS's timestamp wire form reject the body. The defect is the serialization FORM only - the clock value itself is a per-run value and is not part of the requirement.
Fix: the shared timestamp projection used when building the Policy, Service and ServiceFunction views (the service's serializer/state projection, not any single handler) - emit the epoch-seconds form AWS emits rather than the raw float.
Traces: resiliencehubv2-policy-tagging#task#1 (CreatePolicy), resiliencehubv2-policy-tagging#task#7 (GetPolicy), resiliencehubv2-policy-validation-and-conflict-errors#task#3 (CreatePolicy) and #task#7 (GetPolicy), resiliencehubv2-service-function-lifecycle#task#5 (ListServiceFunctions), resiliencehubv2-system-association-and-delete-conflict#task#10 (UpdateService).

=== Cause 2 of 6: Policy$tags is omitted entirely instead of being returned as an empty object ===
AWS: Smithy Policy$tags targets TagMap (@length 1..50, @sensitive). Recorded behavior: for a policy created with no tags, AWS still returns the `tags` member, present and EMPTY ({}), in both CreatePolicy and GetPolicy bodies.
Emulator: omits the member from the response body altogether (the key is absent / None) on both CreatePolicy and GetPolicy.
Why: a client that reads resp["policy"]["tags"] and iterates it - the normal shape against AWS - hits a missing key or a null and has to special-case the emulator; tag-reconciling code (IaC, taggers, compliance sweeps) cannot distinguish "no tags" from "tags not reported".
Fix: the policy view projection shared by the CreatePolicy and GetPolicy handlers - always emit the tags member, empty when the policy carries no tags.
Traces: resiliencehubv2-policy-validation-and-conflict-errors#task#3 (CreatePolicy) and resiliencehubv2-policy-validation-and-conflict-errors#task#7 (GetPolicy).

=== Cause 3 of 6: UpdateService's Service projection drops dependencyDiscovery, rerunAssessment and tags ===
AWS: Smithy UpdateService (POST /v2/update-service) returns `service` -> Service (required). Service$dependencyDiscovery targets DependencyDiscoveryConfig { status: DependencyDiscoveryStatus required enum ENABLED|INITIALIZING|DISABLED, updatedAt: Timestamp, eligibleResourceCount: Integer @range min 0, message: String @length 0..1024 }; Service$rerunAssessment targets Boolean; Service$tags targets TagMap. Recorded behavior: AWS populates all three on the UpdateService response of a service that has not enabled dependency discovery and carries no tags - dependencyDiscovery = { "status": "DISABLED", "message": "Enable dependency discovery to display dependencies.", "updatedAt": <timestamp> }, rerunAssessment = false, tags = {}. The message text "Enable dependency discovery to display dependencies." is fixed text for the DISABLED status (it is the documented initialization-state status message); the updatedAt value is a per-run clock reading and only its presence and wire form are required.
Emulator: omits all three members from the UpdateService body (dependencyDiscovery, rerunAssessment and tags are absent).
Why: a client reading a service's dependency-discovery state or its rerun flag straight off the UpdateService response - as against AWS - gets nothing and cannot tell DISABLED from unknown; the required-by-default boolean and the tag map have to be re-fetched or defaulted by the caller. Because the same Service shape is returned by CreateService/GetService, this is a projection gap, not an UpdateService-only quirk.
Fix: the shared Service view projection (and the UpdateService handler that builds it) - always populate dependencyDiscovery with the service's current status plus the fixed DISABLED status message, rerunAssessment, and tags.
Traces: resiliencehubv2-system-association-and-delete-conflict#task#10.

=== Cause 4 of 6: GetPolicy still returns the pre-mutation tag value after the policy's tags are changed ===
AWS: Smithy TagResource (POST /v2/tags/{resourceArn}, tags -> TagMap required) adds/overwrites tags on the named resource and UpdatePolicy likewise persists a changed tag map; a subsequent GetPolicy must return the NEW value for an overwritten tag key.
Emulator: after the fixture changes a policy tag, GetPolicy still returns the value the policy was CREATED with for that key, so the read never reflects the mutation. The proximate root is that the tag-mutating operations on this path are unimplemented scaffold stubs that answer HTTP 501 and persist nothing: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/tag_resource.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and returns Scaffolded::not_implemented("TagResource") (line 156), and UpdatePolicy is likewise a stub (owned by 20261110T225437Z-resiliencehubv2-updatepolicy-stub.md).
Why: a client that tags a policy and reads it back sees its own write silently lost - the GetPolicy body reports stale tags with a 200, which is worse than an error because nothing signals the loss.
Fix: TagResource's handler (and UpdatePolicy's) must write into the policy's tag map in the service state store, and the policy view projection must read tags from that store so GetPolicy reflects the change. Implementing the stub alone is not enough if the read projection keeps serving the creation-time map.
Traces: resiliencehubv2-policy-tagging#task#7.

=== Cause 5 of 6: ListServiceFunctions still reports the pre-update criticality after a service function is updated ===
AWS: Smithy UpdateServiceFunction (POST /v2/update-function) takes `criticality` -> ServiceFunctionCriticality (enum PRIMARY|SUPPLEMENTAL) and changes only the named fields; ServiceFunction$criticality is required, so a subsequent ListServiceFunctions must report the NEW criticality for that function.
Emulator: ListServiceFunctions still reports criticality PRIMARY for a function that was updated to SUPPLEMENTAL. The proximate root is that UpdateServiceFunction is an unimplemented scaffold stub answering 501 and persisting nothing (owned by 20261110T225437Z-resiliencehubv2-updateservicefunction-stub.md); what is filed here is the read-path requirement that ListServiceFunctions project criticality from the mutated state once that handler exists.
Why: a client that narrows a service's functions by criticality acts on stale classification and a 200 response that looks authoritative.
Fix: UpdateServiceFunction's handler must persist criticality (and the other named fields) into the service-function state, and the ListServiceFunctions projection must read it from there.
Traces: resiliencehubv2-service-function-lifecycle#task#5.

=== Cause 6 of 6: ListServiceFunctions still lists a service function after it is deleted ===
AWS: Smithy DeleteServiceFunction (POST /v2/delete-function, serviceArn + serviceFunctionId required) deletes the function and returns its serviceFunctionId; a subsequent ListServiceFunctions for that serviceArn must not include it, and returns an empty serviceFunctions list when it was the only one. ListServiceFunctions declares serviceFunctions as required, so the empty list is still present.
Emulator: ListServiceFunctions after the delete still returns the full service-function entry (serviceArn, serviceFunctionId, name, description, criticality, resourceCount, source, createdAt, updatedAt) where AWS returns no entry at all. The proximate root is that DeleteServiceFunction is an unimplemented scaffold stub: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/delete_service_function.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and returns Scaffolded::not_implemented("DeleteServiceFunction") (line 162), so nothing is removed from state.
Why: a client that deletes a function and re-lists still sees it, so delete-then-verify loops never converge and resource counts stay wrong; against AWS the entry is gone.
Fix: DeleteServiceFunction's handler must remove the function from the service's function state (and the list projection must read live state), so ListServiceFunctions stops serving it.
Traces: resiliencehubv2-service-function-lifecycle#task#10.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 124 steps, 29 passed, 59 mismatched, score 23.4%). Every cluster below reported stub_path="" and owned_by=[], so none is covered by an auto-filed stub issue's replay key or by an existing issue's.

1) Cause 1 (and the timestamp half of Causes 4/5) - fixture resiliencehubv2-policy-tagging, step task#1, operation CreatePolicy, verdict mismatch, expected 200 / actual 200:
   $.policy.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1794382612.8291433)
   $.policy.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794382612.8291433)
   The same misserialization appears at resiliencehubv2-policy-validation-and-conflict-errors#task#3 and #task#7 ($.policy.createdAt / $.policy.updatedAt = 1794382690.5241601), at resiliencehubv2-service-function-lifecycle#task#5 ($.serviceFunctions[0].createdAt / .updatedAt = 1794382688.8228703) and at resiliencehubv2-system-association-and-delete-conflict#task#10 ($.service.createdAt = 1794382656.2138329, $.service.updatedAt = 1794382656.2395241). The numeric epoch readings are per-run clock values - the corpus asserts only the member's wire shape ('<shaped-not-asserted>'), and it is that shape the emulator's value fails.

2) Cause 2 - fixture resiliencehubv2-policy-validation-and-conflict-errors, step task#3 (CreatePolicy) and step task#7 (GetPolicy), verdict mismatch, expected 200 / actual 200:
   $.policy.tags: missing (expected {}, got None)

3) Cause 3 - fixture resiliencehubv2-system-association-and-delete-conflict, step task#10, operation UpdateService, verdict mismatch, expected 200 / actual 200:
   $.service.dependencyDiscovery: missing (expected {'status': 'DISABLED', 'message': 'Enable dependency discovery to display dependencies.', 'updatedAt': '<shaped-not-asserted>'}, got None)
   $.service.rerunAssessment: missing (expected False, got None)
   $.service.tags: missing (expected {}, got None)

4) Cause 4 - fixture resiliencehubv2-policy-tagging, step task#7, operation GetPolicy, verdict mismatch, expected 200 / actual 200:
   $.policy.tags.team: value_mismatch (expected 'checkout', got 'payments') - i.e. the value the policy was created with is still served after the tag was changed to the new value. ('team', 'checkout' and 'payments' are this recording's tag key/values filling the template; the requirement is that the read returns the value last written.)
   Source: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/tag_resource.rs:4 (SCAFFOLDED marker) and :156 (Scaffolded::not_implemented("TagResource")).

5) Cause 5 - fixture resiliencehubv2-service-function-lifecycle, step task#5, operation ListServiceFunctions, verdict mismatch, expected 200 / actual 200:
   $.serviceFunctions[0].criticality: value_mismatch (expected 'SUPPLEMENTAL', got 'PRIMARY')

6) Cause 6 - fixture resiliencehubv2-service-function-lifecycle, step task#10, operation ListServiceFunctions, verdict mismatch, expected 200 / actual 200:
   $.serviceFunctions[0]: extra (expected None, got {'serviceArn': 'arn:aws:resiliencehub:<region>:<account>:service/<uuid>', 'serviceFunctionId': '<uuid>', 'name': '<name>', 'description': '<description>', 'criticality': 'PRIMARY', 'resourceCount': 0, 'source': 'USER', 'createdAt': <epoch>, 'updatedAt': <epoch>}) - the deleted function is still listed. (ARNs, ids, names and timestamps in that body are per-run values; the requirement is that no entry is returned.)
   Source: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/delete_service_function.rs:4 (SCAFFOLDED marker) and :162 (Scaffolded::not_implemented("DeleteServiceFunction")).

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json, service NGRHServiceCore, sdkId resiliencehubv2, protocol restJson1, no service-wide @timestampFormat):
- Policy: policyArn/name required; tags -> TagMap @length 1..50 @sensitive; createdAt/updatedAt -> smithy.api#Timestamp (prelude, no format override).
- Service: dependencyDiscovery -> DependencyDiscoveryConfig; rerunAssessment -> Boolean; tags -> TagMap; createdAt/updatedAt -> Timestamp.
- DependencyDiscoveryConfig: status -> DependencyDiscoveryStatus (enum ENABLED|INITIALIZING|DISABLED) required; updatedAt -> Timestamp; eligibleResourceCount -> Integer @range min 0; message -> String @length 0..1024 ("A status message for dependency discovery, displayed during the initialization state").
- ServiceFunction: serviceArn/serviceFunctionId/name/criticality required; criticality -> ServiceFunctionCriticality (enum PRIMARY|SUPPLEMENTAL); createdAt/updatedAt -> Timestamp.
- UpdateService: POST /v2/update-service, output service -> Service required. UpdateServiceFunction: POST /v2/update-function, input criticality -> ServiceFunctionCriticality. DeleteServiceFunction: POST /v2/delete-function, output serviceFunctionId. ListServiceFunctions: GET /v2/list-functions, output serviceFunctions -> ServiceFunctionList required. TagResource: POST /v2/tags/{resourceArn}, tags -> TagMap required.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and inspect the seven fixture#step keys named under Traces.
3. Cause 1: POST /v2/create-policy with any valid input and read $.policy.createdAt / $.policy.updatedAt; also GET /v2/get-policy, POST /v2/update-service and GET /v2/list-functions. Every Timestamp member comes back as a full-precision float64 epoch (sub-millisecond fractional digits) rather than the epoch-seconds form AWS emits.
4. Cause 2: POST /v2/create-policy with NO tags, then GET /v2/get-policy for it. In both bodies the `tags` key is absent; AWS returns "tags": {}.
5. Cause 3: create a service, then POST /v2/update-service for it. The response `service` object has no dependencyDiscovery, no rerunAssessment and no tags; AWS returns dependencyDiscovery {"status":"DISABLED","message":"Enable dependency discovery to display dependencies.","updatedAt":<ts>}, rerunAssessment false and tags {}.
6. Cause 4: create a policy with tags {"team": "<v1>"}, change that tag to "<v2>" (POST /v2/tags/{resourceArn} - today a 501 scaffold), then GET /v2/get-policy. The body still reports team="<v1>".
7. Cause 5: create a service function with criticality PRIMARY, POST /v2/update-function setting criticality SUPPLEMENTAL (today a 501 scaffold), then GET /v2/list-functions. The entry still reports PRIMARY.
8. Cause 6: create a single service function, POST /v2/delete-function for it (today a 501 scaffold), then GET /v2/list-functions. The deleted function is still listed; AWS returns an empty serviceFunctions list.
