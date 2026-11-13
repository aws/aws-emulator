Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-batch-security-requirements-crud
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-batch-security-requirements-crud
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|BatchGetSecurityRequirements|2xx->2xx|$.securityRequirements[*].description
Issue-Key: securityagent|ListSecurityRequirements|2xx->2xx|$.securityRequirementSummaries[name="sim2real-probe-securityagent-req-alpha-c992f49d"].description
Issue-Key: securityagent|ListSecurityRequirements|2xx->2xx|$.securityRequirementSummaries[name="sim2real-probe-securityagent-req-gamma-c992f49d"]
Issue-Related: 20261108T133019Z-securityagent-aws-managed-security-requirement-packs-return-no-requirements.md
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# Security requirement updates and deletes never change stored pack contents

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecurityRequirements

## Description
Three net-new causes in one fixture, all read-back divergences: a mutation applied earlier in the same fixture is not visible to any later read, so the requirement store still holds the pre-mutation content.

Cause 1 of 3 (fixture securityagent-batch-security-requirements-crud, step task#6, BatchGetSecurityRequirements): after a requirement's description has been updated, BatchGetSecurityRequirements still projects the original description.
AWS: Smithy: BatchUpdateSecurityRequirements (POST /BatchUpdateSecurityRequirements, 200) takes packId + securityRequirements (UpdateSecurityRequirementEntryList, @required) and returns updatedSecurityRequirementNames (@required) plus per-item errors; a name returned in updatedSecurityRequirementNames has been written. BatchGetSecurityRequirements is @readonly and returns securityRequirements (@required), whose entries carry the requirement's current description and updatedAt. So a get issued after a successful update returns the NEW description, and updatedAt moves to the update's instant. The recording shows exactly that.
Emulator: HTTP 200, but $.securityRequirements[0].description is still the pre-update text (expected 'Requirement alpha updated description', got 'Requirement alpha description'). The update is lost, not merely delayed.
Why: a client that updates a requirement and reads it back — the normal read-after-write confirmation — sees its write silently discarded while the update call reported success. Idempotency checks, drift detection and any edit-then-verify workflow conclude the service rejected a change it accepted.
Fix: the requirement store in crate::state and the BatchUpdateSecurityRequirements handler must write the mutation to the same (packId, name) records BatchGetSecurityRequirements/ListSecurityRequirements read, refreshing updatedAt.

Cause 2 of 3 (same fixture, step task#8, ListSecurityRequirements): the list projection shows both symptoms at once — the updated requirement still carries its pre-update description, and a requirement deleted earlier in the fixture is still listed.
AWS: Smithy: ListSecurityRequirements is @readonly and returns securityRequirementSummaries (@required) of SecurityRequirementSummary{packId,name,description,createdAt,updatedAt} — the pack's CURRENT contents. BatchDeleteSecurityRequirements (POST /BatchDeleteSecurityRequirements, 200) returns deletedSecurityRequirementNames (@required); a name it reports deleted is gone from the pack, so a subsequent list does not contain it. The recording shows the updated description and no entry for the deleted requirement.
Emulator: HTTP 200 with the stale description on the updated requirement, and an extra summary for the deleted requirement (reported as `extra (expected None, got {...})`).
Why: a client enumerating a pack after editing and deleting content gets a listing that matches neither the state it created nor the responses it was given — deleted requirements appear live and edits appear unapplied, so reconciliation loops delete the same name forever.
Fix: same place — the delete path must remove the (packId, name) record from the store the list handler walks, and the update path must mutate it.

Cause 3 of 3 (same fixture, step task#10, ListSecurityRequirements): after the fixture's deletes have removed every customer-created requirement from the pack, the pack still lists all three.
AWS: as above — a pack whose requirements have all been deleted lists securityRequirementSummaries as an empty list (the member is @required, so it is present and empty, not omitted).
Emulator: HTTP 200 with three summaries still present (all three reported `extra`), one per requirement created earlier in the fixture.
Why: deletion is unobservable, so a client can never confirm cleanup; teardown asserts fail and the pack cannot be emptied.
Fix: the requirement store's delete path in crate::state plus the BatchDeleteSecurityRequirements handler.

Note for the fixer: BatchUpdateSecurityRequirements and BatchDeleteSecurityRequirements are still unimplemented scaffold stubs and are separately filed as 20261108T031300Z-securityagent-batchupdatesecurityrequirements-stub.md and 20261108T031300Z-securityagent-batchdeletesecurityrequirements-stub.md. Those two issues cover the stub operations themselves; they carry no replay key for these three read-back steps (the replay reports owned_by empty and stub_path empty for all three), so this issue is what gates the read side: once the mutations are implemented they must be visible through BatchGetSecurityRequirements and ListSecurityRequirements, which is what these three steps verify.

Traces: securityagent-batch-security-requirements-crud#task#6, #task#8, #task#10.

## Evidence
Fixture securityagent-batch-security-requirements-crud, verdict mismatch, expected status 200 / actual 200 on all three steps.

task#6 BatchGetSecurityRequirements: $.securityRequirements[0].description: value_mismatch — expected 'Requirement alpha updated description', got 'Requirement alpha description'.

task#8 ListSecurityRequirements: $.securityRequirementSummaries[name="...req-alpha-..."].description: value_mismatch — expected 'Requirement alpha updated description', got 'Requirement alpha description'; $.securityRequirementSummaries[name="...req-gamma-..."]: extra — expected None, got a full summary {packId, name, description: 'Requirement gamma description', createdAt, updatedAt} for a requirement the fixture had deleted.

task#10 ListSecurityRequirements: $.securityRequirementSummaries[0], [1] and [2]: extra — expected None, got summaries for req-alpha, req-beta and req-gamma. Expected listing is empty after the deletes.

(The requirement names in the recording carry a per-run suffix and the createdAt/updatedAt values are per-run clock readings; neither is a requirement. What is a requirement is that the description returned is the last one written and that a deleted name is absent.)

Smithy: BatchUpdateSecurityRequirements output updatedSecurityRequirementNames @required; BatchDeleteSecurityRequirements output deletedSecurityRequirementNames @required; BatchGetSecurityRequirements output securityRequirements @required, @readonly; ListSecurityRequirements output securityRequirementSummaries @required, @readonly; SecurityRequirementSummary.description/updatedAt @required.

Citations: services/service-com-amazonaws-securityagent-securityagent/src/state.rs (the requirement store and its (packId, name) keying); src/ops/batch_update_security_requirements.rs and src/ops/batch_delete_security_requirements.rs (scaffold stubs); src/ops/batch_get_security_requirements.rs:195-206 and src/ops/list_security_requirements.rs (readers that project whatever the store holds).

## Steps to Reproduce
1. Boot the emulator (fresh state).
2. sa = boto3.client("securityagent", endpoint_url=<emulator>)
3. pack = sa.create_security_requirement_pack(...)  -> packId
4. sa.batch_create_security_requirements(packId=packId, securityRequirements=[alpha, beta, gamma])
5. sa.batch_update_security_requirements(packId=packId, securityRequirements=[{name: alpha, description: 'Requirement alpha updated description'}])
6. sa.batch_get_security_requirements(packId=packId, securityRequirementNames=[alpha]) — observe the ORIGINAL description (cause 1).
7. sa.batch_delete_security_requirements(packId=packId, securityRequirementNames=[gamma]); sa.list_security_requirements(packId=packId) — observe gamma still listed and alpha's description still stale (cause 2).
8. Delete the remaining names, then sa.list_security_requirements(packId=packId) — observe all three still listed instead of an empty list (cause 3).
9. Replay fixture securityagent-batch-security-requirements-crud steps task#6, task#8, task#10 to confirm.
