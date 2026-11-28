Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# StartAssessment never consults the entity lock a running change set holds, so a locked entity id is answered 404 (or assessed outright) instead of the in-use refusal

_Filed: 2026-11-28T00:39:44Z_
_Operation: StartAssessment | Class: error-response_

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other path of this crate that acts on an entity id asks `crate::state::locking_change_set` first, because a running change set locks the ids it targets — including the id of an entity it is still CREATING, which the catalog has not written yet. `StartAssessment` skips that helper entirely: it goes straight to `store.entities.get(&input.entity_id)` and so produces the one answer its sibling's comment says such an id must NOT get ("does not exist"):

* an id a running change set reserved for an entity it is creating is reported ABSENT (404 `ResourceNotFoundException`, message "Requested entity '<id>' does not exist."), even though `ListChangeSets` is at that moment reporting that id in the change set's `EntityIdList`;
* an existing entity that a running change set is changing is assessed anyway, with no refusal at all, though the crate holds that entity locked for the whole run.

`StartAssessment` models `ResourceInUseException` (HTTP 423) and already raises it through `crate::state::assessment_in_progress` for the other half of the same "this entity is in use" state, so the refusal and the helper both exist; only the call is missing. Nothing in the crate justifies the asymmetry: the lock test file scopes itself to `StartChangeSet` and the assessment tests only ever assess entities whose creating change set has already succeeded, so this path is untested rather than deliberately pinned.

## Evidence

The sibling applies the check and states the rule it enforces — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:602-622`:

```
// "when a ChangeSet is running, all the entities
// targeted by the different changes are locked until
// the change set has completed … If you try to start
// a change set containing a change against an entity
// that is already locked, you will receive a
// ResourceInUseException error."
//
// The lock is held on the entity ID, and a running
// change set holds the ids of the entities it is
// still CREATING as well as the ids of the ones it
// found: a change set writes its entities into the
// catalog only once it succeeds. So the lock is
// consulted BEFORE the catalog is read — an id a
// change set in flight holds names an entity being
// worked on, not a missing one.
if let Some(holder) = store.locking_change_set(entity_id) {
    return Err(StartChangeSetErrorModeled::ResourceInUseException(
        crate::state::entity_locked(entity_id, holder),
    )
    .into());
}
```

The helper is crate-wide, not StartChangeSet-specific — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2191-2197`: "/// The id of the still-running change set that has `entity_id` locked, if any. … `StartChangeSet` documents the lock: \"when a ChangeSet is running, all the entities targeted by the different changes are locked until the change set has completed (either succeeded, cancelled, or failed)\"" followed by `pub fn locking_change_set(&self, entity_id: &str) -> Option<&str>`.

This operation skips it — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_assessment.rs:331-336`:

```
        let Some(entity) = store.entities.get(&input.entity_id) else {
            return Err(StartAssessmentErrorModeled::ResourceNotFoundException(
                crate::state::entity_not_found(&input.entity_id),
            )
            .into());
        };
```

yet the very same handler raises the modelled in-use fault for the other in-use case, 20 lines later (`start_assessment.rs:355-364`): "// The entity is in use for as long as an assessment of it is running, // so a second start against the same entity is refused with the // resource-in-use fault this operation models." + `store.running_assessment_of(&input.entity_id, now)`.

Test coverage: `tests/all/entity_lock_in_flight.rs:3-16` exercises the lock only through `StartChangeSet` ("Which refusal `StartChangeSet` answers for an entity id a change set still RUNNING holds"); `tests/all/started_assessments.rs:653-654` pins only "An entity being assessed is in use: a second start against it is refused". No test covers StartAssessment against a change-set-locked id.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and one change {ChangeType: "CreateProduct", Entity: {Type: "AmiProduct@1.0"}, Details: {"ProductTitle":"p"}} — no Entity.Identifier, so the call reserves a new entity id
2. Within the PREPARING+APPLYING window (crate::state::CHANGE_SET_PREPARING + CHANGE_SET_APPLYING = 500 ms) POST /ListChangeSets and read the reserved id out of the change set's EntityIdList, e.g. ent-000000000001
3. Still inside that window, POST /StartAssessment with Catalog=AWSMarketplace, FrameworkId=AMISecurity, EntityId=ent-000000000001
4. Observed: 404 ResourceNotFoundException "Requested entity 'ent-000000000001' does not exist." — the answer start_change_set.rs:613-616 says an id a change set in flight holds must not get; expected the crate's own lock refusal, 423 ResourceInUseException naming the holding change set (crate::state::entity_locked)
5. Second case: let the change set succeed, then start a change set with {ChangeType: "AddDeliveryOptions", Entity: {Type: "AmiProduct@1.0", Identifier: "<entity id>"}} and, while it is still PREPARING, POST /StartAssessment for that same entity id — the assessment is started with no in-use refusal although the entity is locked for the whole run
