Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-policy-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-policy-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|CreatePolicy|2xx->2xx|$.policy.tags
Issue-Key: resiliencehubv2|GetPolicy|2xx->2xx|$.policy.tags
Issue-Key: resiliencehubv2|GetPolicy|2xx->2xx|$.policy.multiAz
Issue-Key: resiliencehubv2|GetPolicy|4xx->2xx|$.message
Issue-Key: resiliencehubv2|GetPolicy|4xx->2xx|$.resourceId
Issue-Key: resiliencehubv2|GetPolicy|4xx->2xx|$.resourceType
Issue-Key: resiliencehubv2|GetPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehubv2|GetPolicy|4xx->2xx|status
Issue-Commit: ed43c7c5e13b0f1212f82cf25816ea1e565f9817

# Policy lifecycle responses diverge from AWS: empty tags dropped, multiAz not reflected, deleted policy still served 200

> _Note: this issue was identified against the code base at commit `ed43c7c5e13b0f1212f82cf25816ea1e565f9817`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
The resiliencehubv2 `Policy` lifecycle (CreatePolicy -> GetPolicy -> UpdatePolicy -> GetPolicy -> DeletePolicy -> GetPolicy) answers bodies that do not match the shape AWS returns for the same sequence. FOUR distinct causes are bundled here; all four must be closed before this issue is submitted.

=== Cause 1 of 4: CreatePolicy omits `policy.tags` for an untagged policy (fixture resiliencehubv2-policy-lifecycle, step task#1) ===
AWS: recorded behavior only. The Smithy model (Policy$tags -> TagMap, @length min 1 max 50, @sensitive, map<TagKey,TagValue>) states no rule for the untagged case; the recorded CreatePolicy 200 response carries `policy.tags` present and EMPTY (`{}`) when the request supplies no `tags`. The member is always present in the projection; its value is the (possibly empty) tag map built from the request.
Emulator: `policy.tags` is absent from the 200 body — replay reports `$.policy.tags: missing (expected {}, got None)`. The projection suppresses the member whenever the stored tag map is empty (`tag_map_view` returns `None` for an empty map, and `PolicyRecord::to_policy` passes that straight through).
Why: a client that reads `policy.tags` to enumerate or diff a policy's tags gets a null/absent member instead of an empty map, so tag-aware code must special-case the emulator (null-check, or treat "no tags" as "unknown"), and round-tripping create -> tag -> create drops the member entirely.
Fix: the Policy response projection / state view — the policy record's `to_policy` projection and the shared tag-map view it calls must emit the tag map for every policy, empty map included, rather than eliding it when empty.
Traces: resiliencehubv2-policy-lifecycle#task#1 (CreatePolicy, status 200 vs 200, key $.policy.tags).

=== Cause 2 of 4: GetPolicy omits `policy.tags` for an untagged policy (fixture resiliencehubv2-policy-lifecycle, step task#2) ===
AWS: recorded behavior only, as in Cause 1 — Smithy GetPolicy -> GetPolicyResponse$policy (Policy, required), Policy$tags (TagMap). The recorded GetPolicy 200 read-back of a policy created without tags carries `policy.tags` present and empty (`{}`).
Emulator: `$.policy.tags: missing (expected {}, got None)` on the read-back path as well, i.e. the elision is in the shared projection both readers answer from, not in one handler.
Why: the read path and the create path are consistent with each other but both differ from AWS, so a client cannot recover the member by re-reading the policy.
Fix: same component as Cause 1 — the shared Policy projection in the policy state module, so CreatePolicy and GetPolicy both gain the member.
Traces: resiliencehubv2-policy-lifecycle#task#2 (GetPolicy, status 200 vs 200, key $.policy.tags).

=== Cause 3 of 4: GetPolicy does not reflect the multi-AZ targets set on the policy (fixture resiliencehubv2-policy-lifecycle, step task#4) ===
AWS: Smithy UpdatePolicy (POST /v2/update-policy) takes `multiAz` (MultiAzTargets: rtoInMinutes Integer @range min 0, rpoInMinutes Integer @range min 0, disasterRecoveryApproach MultiAzDisasterRecoveryApproach) and returns the updated `policy`; Policy$multiAz is documented "The multi-AZ disaster recovery targets defined in the policy" and GetPolicy is @readonly, projecting the stored policy. After the policy's multi-AZ targets are set, the subsequent GetPolicy 200 body carries `policy.multiAz` with the configured disasterRecoveryApproach / rpoInMinutes / rtoInMinutes (the recording shows an approach value plus both minute targets), plus `policy.tags` as in Causes 1-2.
Emulator: the GetPolicy 200 body has `$.policy.multiAz: missing` (expected an object with disasterRecoveryApproach, rpoInMinutes and rtoInMinutes) and `$.policy.tags: missing`. The preceding update in the fixture never reaches the store, because UpdatePolicy is still an unimplemented scaffold that answers 501 (ops/update_policy.rs returns `Scaffolded::not_implemented("UpdatePolicy")`), so the stored record keeps the target blocks it was created with and GetPolicy faithfully projects a policy that AWS would have changed. GetPolicy's own projection does read `multi_az`, so the gap is the missing write path.
Why: a client that configures resilience targets and then reads the policy back sees the targets silently absent and cannot distinguish "no targets configured" from "my update was lost"; any logic keyed on disasterRecoveryApproach/RPO/RTO takes the wrong branch.
Fix: the UpdatePolicy handler and the policy state record it writes — UpdatePolicy must merge the caller's target blocks (availabilitySlo, multiAz, multiRegion, dataRecovery, description, sharingEnabled) into the stored policy record and bump updatedAt, so the @readonly GetPolicy projection reads them back. The separately filed UpdatePolicy scaffold-stub issue covers only that the operation answers 501; this divergence is about the GetPolicy body a client observes afterwards, and must be re-replayed on this step.
Traces: resiliencehubv2-policy-lifecycle#task#4 (GetPolicy, status 200 vs 200, keys $.policy.multiAz, $.policy.tags).

=== Cause 4 of 4: GetPolicy serves a deleted policy with 200 instead of a 404 ResourceNotFoundException (fixture resiliencehubv2-policy-lifecycle, step task#7) ===
AWS: Smithy GetPolicy declares ResourceNotFoundException (@error client, httpError 404, members `message` (String, required), `resourceId` (String, "The identifier of the resource that was not found"), `resourceType` (String, "The type of the resource that was not found")). Once the policy has been deleted, GetPolicy on its ARN answers HTTP 404 with the awsJson/restJson error-type header `x-amzn-errortype` naming ResourceNotFoundException, and a body whose `message` is the fixed text "Policy not found: " followed by the policy's resource identifier, `resourceId` set to that same identifier, and `resourceType` set to the fixed text "Policy". In the recorded message the fixed part is "Policy not found: "; the variable part is the identifier taken from the request's `policyArn` — the ARN's resource segment, `policy/<uuid>`, not the full ARN — and the recording's particular uuid is just that run's value.
Emulator: HTTP 200 with the still-stored policy body. Replay reports `status: expected 404, got 200`, `$.message: missing (expected 'Policy not found: <resource-id>')`, `$.resourceId: missing`, `$.resourceType: missing (expected 'Policy')`, `$headers.x-amzn-errortype: missing`. Two things are wrong: (a) the delete never removes the record, because DeletePolicy is an unimplemented scaffold answering 501 (ops/delete_policy.rs returns `Scaffolded::not_implemented("DeletePolicy")`), so the policy remains readable forever; and (b) when GetPolicy's not-found branch IS taken it words the error differently from AWS — it builds `message` as "No policy found for " + the full ARN and sets `resourceId` to the full ARN, where AWS uses "Policy not found: " + the ARN's resource segment and the same segment for `resourceId`.
Why: deletion is unobservable. A client that deletes a policy and polls GetPolicy to confirm removal never converges, retry/cleanup loops never terminate, and error handling keyed on a 404 / ResourceNotFoundException (or on the x-amzn-errortype header) never fires — the SDK hands back a success model instead of raising the modeled exception. When a 404 does occur, message-matching clients still see the wrong wording and an ARN where AWS puts the resource id.
Fix: the DeletePolicy handler plus the policy state store (remove the record and prune the name and clientToken indexes that point at it, returning `policyArn`), and the GetPolicy handler's not-found branch (ResourceNotFoundException message/resourceId wording). The separately filed DeletePolicy scaffold-stub issue covers only the 501 on the delete call itself; this divergence is the GetPolicy-after-delete contract and must be re-replayed on this step.
Traces: resiliencehubv2-policy-lifecycle#task#7 (GetPolicy, status 404 vs 200, keys $.message, $.resourceId, $.resourceType, $headers.x-amzn-errortype, status).

Related open issue, not a duplicate of any cause above: "resiliencehubv2 ValidationException message uses legacy validation wording instead of the Smithy constraint-violation text" (20261029T214205Z-...) owns the CreateSystem ValidationException wording divergence in the resiliencehubv2-system-validation-and-conflict-errors fixture.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false):

1) fixture resiliencehubv2-policy-lifecycle step task#1 — CreatePolicy, verdict mismatch, expected_status 200, actual_status 200
   detail: $.policy.tags: missing (expected {}, got None)

2) fixture resiliencehubv2-policy-lifecycle step task#2 — GetPolicy, verdict mismatch, expected_status 200, actual_status 200
   detail: $.policy.tags: missing (expected {}, got None)

3) fixture resiliencehubv2-policy-lifecycle step task#4 — GetPolicy, verdict mismatch, expected_status 200, actual_status 200
   detail: $.policy.multiAz: missing (expected {'disasterRecoveryApproach': 'WARM_STANDBY', 'rpoInMinutes': 5, 'rtoInMinutes': 15}, got None)
           $.policy.tags: missing (expected {}, got None)

4) fixture resiliencehubv2-policy-lifecycle step task#7 — GetPolicy, verdict mismatch, expected_status 404, actual_status 200
   detail: status: expected 404, got 200
           $.message: missing (expected 'Policy not found: policy/<uuid>', got None)
           $.resourceId: missing (expected 'policy/<uuid>', got None)
           $.resourceType: missing (expected 'Policy', got None)
           $headers.x-amzn-errortype: missing (expected a shaped ResourceNotFoundException error type, got None)
   (the uuid is a per-run value; the requirement is the fixed text plus the request's resource id)

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json):
- Policy members: policyArn (required), name (required), description, availabilitySlo, multiAz (MultiAzTargets), multiRegion, dataRecovery, sharingEnabled, organizationId, kmsKeyId, tags (TagMap @length 1..50 @sensitive), associatedServiceCount, createdAt, updatedAt.
- MultiAzTargets: rtoInMinutes (Integer @range min 0), rpoInMinutes (Integer @range min 0), disasterRecoveryApproach (MultiAzDisasterRecoveryApproach enum).
- GetPolicy: GET /v2/get-policy, @readonly, errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400.
- ResourceNotFoundException: @error client, httpError 404, members message (required), resourceId, resourceType.
- UpdatePolicy: POST /v2/update-policy, input includes multiAz; DeletePolicy: POST /v2/delete-policy, output policyArn.

Emulator code evidence:
- src/state.rs tag_map_view(): returns None when the stored tag map is empty, so the member is elided.
- src/state/policies.rs PolicyRecord::to_policy(): tags: tag_map_view(&self.tags) — the single projection both CreatePolicy and GetPolicy answer from.
- src/ops/update_policy.rs handle(): Err(Scaffolded::not_implemented("UpdatePolicy")).
- src/ops/delete_policy.rs handle(): Err(Scaffolded::not_implemented("DeletePolicy")).
- src/ops/get_policy.rs handle(): not-found branch builds message "No policy found for {policyArn}" and resource_id = the full policyArn.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire fixture corpus simparity-wire-aws-v1 (replay_wire_corpus) and look at the resiliencehubv2-policy-lifecycle fixture.
3. Cause 1 — step task#1: POST /v2/create-policy with a valid `name` and no `tags`; the 200 body's `policy` has no `tags` member (AWS returns `"tags": {}`).
4. Cause 2 — step task#2: GET /v2/get-policy?policyArn=<the created ARN>; the 200 body's `policy` again has no `tags` member.
5. Cause 3 — steps task#3/task#4: POST /v2/update-policy for that ARN setting `multiAz` (disasterRecoveryApproach + rpoInMinutes + rtoInMinutes), then GET /v2/get-policy?policyArn=<ARN>; the update answers 501 and the following 200 GetPolicy body has no `policy.multiAz`.
6. Cause 4 — steps task#6/task#7: POST /v2/delete-policy for that ARN, then GET /v2/get-policy?policyArn=<ARN>; the delete answers 501 and the GetPolicy returns 200 with the policy still present, instead of 404 ResourceNotFoundException carrying x-amzn-errortype plus message "Policy not found: <resource-id>", resourceId <resource-id>, resourceType "Policy".
