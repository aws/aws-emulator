Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# StartChangeSet never consults running_assessment_of, so a change set is accepted against an entity this crate's own rule holds to be in use

_Filed: 2026-11-28T00:39:44Z_
_Operation: StartChangeSet | Class: validation_

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate defines two kinds of "this entity is in use" and calls them siblings of each other: an entity targeted by a running change set (`locking_change_set` / `entity_locked`) and an entity an assessment is still running over (`running_assessment_of` / `assessment_in_progress`). `StartChangeSet` consults only the first. A change set whose change names an entity whose assessment run is still in flight is therefore started, and its changes are applied to that entity when the run ends — the entity's details, tags, visibility, name and revision are rewritten while the crate holds it to be in use — and `StartChangeSet`'s modelled `ResourceInUseException` (HTTP 423) is never raised for that half of the state, although the handler already raises exactly that error for the other half two lines of code earlier.

The helper needed is already in the crate and is already documented as a property of the ENTITY rather than of one operation, so only the call is missing. No comment, test or doc in the crate states that a change set may proceed against an entity under assessment.

## Evidence

The crate states the rule as a property of the entity — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:986-999` (doc of `ASSESSMENT_RUN`): "It is non-zero so that a caller reading straight after the start observes the run it started — and so that the entity being assessed is in use for as long as that run lasts ([`assessment_in_progress`])."

And again at `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2713-2718` (doc of the helper): "An assessment is a run over one entity, so the entity is in use for as long as that run lasts: this is what a second start against the same entity is refused on ([`assessment_in_progress`])." + `pub fn running_assessment_of(&self, entity_id: &str, now: SystemTime) -> Option<&str>`.

And it names the two refusals siblings — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:4666-4677`: "/// The modelled `ResourceInUseException` payload for an entity an assessment is still running over … so while that run lasts the entity is in use: a second start against the same entity is refused, stated the way the sibling lock of this service is ([`entity_locked`])".

The handler asks only the change-set half — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:617-622`:

```
                            if let Some(holder) = store.locking_change_set(entity_id) {
                                return Err(StartChangeSetErrorModeled::ResourceInUseException(
                                    crate::state::entity_locked(entity_id, holder),
                                )
                                .into());
                            }
```

and `start_change_set.rs:664-667` closes the subject with "// The lock is judged where the entity is resolved, above" — `running_assessment_of` is called nowhere in `src/ops/start_change_set.rs` (its only caller is `src/ops/start_assessment.rs:358`).

Test coverage: `tests/all/started_assessments.rs:653-710` (`an_entity_already_being_assessed_is_in_use`) pins the in-use rule only for a second StartAssessment; `tests/all/entity_lock_in_flight.rs` and `tests/all/change_pair_uniqueness.rs:502-570` pin the change-set lock only against other change sets. Nothing pins a change set started against an entity under assessment.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and {ChangeType: "CreateProduct", Entity: {Type: "AmiProduct@1.0"}, Details: {"ProductTitle":"p","DeliveryOptions":[{"Details":{"AmiDeliveryOptionDetails":{"AmiSource":{"AmiId":"ami-1","AccessRoleArn":"arn:aws:iam::000000000000:role/r","UserName":"ec2-user"},"UsageInstructions":"run it"}}}]} and wait 500 ms for SUCCEEDED, noting the created entity id
2. POST /StartAssessment with Catalog=AWSMarketplace, FrameworkId=AMISecurity, EntityId=<that id> — the assessment now runs for crate::state::ASSESSMENT_RUN (3 s), during which state.rs:995-997 says the entity is in use
3. Within those 3 s, POST /StartAssessment again for the same entity: refused 423 ResourceInUseException (the crate's rule in force)
4. Within those same 3 s, POST /StartChangeSet with {ChangeType: "UpdateInformation", Entity: {Type: "AmiProduct@1.0", Identifier: "<that id>"}, Details: {"ProductTitle":"renamed"}}
5. Observed: 200 with a new ChangeSetId, and the change is applied to the entity while its assessment run is still in flight; the modelled ResourceInUseException is never raised, contradicting the crate's own statement that the entity is in use for the length of that run
