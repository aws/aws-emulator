Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5885802d62bfe456736d54774a25f0dc1a0cbac

# StartAssessment ignores the entity lock a running change set holds, and assesses the pre-change delivery option

_Filed: 2026-11-28T14:22:34Z_
_Operation: StartAssessment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5885802d62bfe456736d54774a25f0dc1a0cbac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds the documented entity lock in one helper, `ComAmazonawsMarketplacecatalogAwsmpseymourState::locking_change_set`, and `StartChangeSet` consults it before touching an entity. `StartAssessment` acts on the very same resource — it reads `entity.details` and evaluates the delivery option that entity currently carries — and it models the same `ResourceInUseException` (423), which it already raises for an entity whose previous ASSESSMENT is still running. It never calls `locking_change_set`, which is the only consumer check in the crate (grep shows the sole call site is start_change_set.rs:617). The consequence: while a change set that is rewriting an entity's delivery options is still PREPARING/APPLYING, an assessment can be started against that entity; it reads the pre-change details, records PASS/FAIL and an `AssessmentTargetSummary$EntityId` about a state the in-flight change set is replacing, and that stale verdict is what `ListAssessments` / `DescribeAssessment` then report.

## Evidence

The lock, `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2191-2197`:
```
    /// The id of the still-running change set that has `entity_id` locked, if
    /// any.
    ///
    /// `StartChangeSet` documents the lock: "when a ChangeSet is running, all
    /// the entities targeted by the different changes are locked until the
    /// change set has completed (either succeeded, cancelled, or failed)".
    pub fn locking_change_set(&self, entity_id: &str) -> Option<&str> {
```
Sibling that applies it, `src/ops/start_change_set.rs:617-622`:
```
                            if let Some(holder) = store.locking_change_set(entity_id) {
                                return Err(StartChangeSetErrorModeled::ResourceInUseException(
                                    crate::state::entity_locked(entity_id, holder),
                                )
                                .into());
                            }
```
The handler that skips it — `src/ops/start_assessment.rs:331-364` resolves the entity and checks only a running ASSESSMENT:
```
        let Some(entity) = store.entities.get(&input.entity_id) else {
...
        // The entity is in use for as long as an assessment of it is running,
        // so a second start against the same entity is refused with the
        // resource-in-use fault this operation models.
        if let Some(running) = store.running_assessment_of(&input.entity_id, now) {
```
and it then reads the pre-change details, `src/ops/start_assessment.rs:388-389`:
```
        let (delivery_option_id, submitted) =
            crate::state::current_delivery_option(&entity.details);
```
`grep locking_change_set src/` returns only state.rs:2160/2197/4657 and start_change_set.rs:617 — no assessment path consults it. No test pins the current answer: `tests/all/started_assessments.rs:656-674` covers only the already-being-assessed case, and `tests/all/entity_lock_in_flight.rs` exercises the lock through `StartChangeSet` alone.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace, one CreateProduct change for AmiProduct@1.0 carrying DeliveryOptions with a valid AmiSource; wait for SUCCEEDED and note the entity id.
2. POST /StartChangeSet with Catalog=AWSMarketplace and an AddDeliveryOptions change naming that entity id (the change set is PREPARING for CHANGE_SET_PREPARING = 250ms, during which locking_change_set(<entity id>) returns the holder).
3. Within that window, POST /StartAssessment with Catalog=AWSMarketplace, FrameworkId=AMISecurity, EntityId=<entity id> — it answers 200 with an AssessmentId instead of the 423 ResourceInUseException a StartChangeSet against the same locked entity answers.
4. POST /DescribeAssessment for that assessment once its run finishes — FrameworkSummary$DeliveryOptionId and the control results describe the delivery option from before the in-flight change set, not the entity the catalog is about to hold.
