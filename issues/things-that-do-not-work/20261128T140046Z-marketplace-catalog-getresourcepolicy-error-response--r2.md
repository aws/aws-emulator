Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6b12489e1ebb5e6d9c766b33956d5bab581b07ea

# Resource-policy reads accept an ARN whose id the catalog holds as an ASSESSMENT, while the same ARN naming a change-set id is refused 404

_Filed: 2026-11-28T14:00:46Z_
_Operation: GetResourcePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `6b12489e1ebb5e6d9c766b33956d5bab581b07ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMarketplacecatalogAwsmpseymourState::policy_key_for` is the one place the resource-policy surface decides whether a resource ARN identifies an ENTITY of this catalog. Its own doc states the rule as: an id the catalog holds a record of under a DIFFERENT resource KIND (or another entity type) contradicts an ARN that spells it as an entity, and is therefore refused as absent. The code under that doc implements only two of the three kinds the crate models: it rejects an id held in `self.change_sets`, and an id held in `self.entities` under another type — but it never looks at `self.assessments`, even though `CatalogResource` has an `Assessment` variant and every sibling store accessor (`holds_resource`, `stored_resource_arn`, `resource_tags`, `resource_tags_write`) handles that kind.

Consequence: for an ARN of this catalog's own form whose type segment is an entity type and whose id is an assessment this catalog holds (e.g. `arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/AmiProduct/assessment-000000000001`), `policy_key_for` answers `Some(key)`, so `GetResourcePolicy` answers 200 with the empty policy document and `DeleteResourcePolicy` answers 200 — whereas the exactly analogous ARN carrying a held CHANGE-SET id (`.../AmiProduct/cs-000000000001`) is refused 404 `ResourceNotFoundException` by both. One kind of record the catalog holds contradicts the ARN, the other silently does not. `PutResourcePolicy`, which additionally calls `holds_resource`, refuses the assessment-id ARN with 404, so the three operations of one surface disagree about the same ARN.

The fix is to apply the rule the doc states to every resource kind the store holds (an id held in `self.assessments` is no entity either), which also makes the policy surface agree with `holds_resource`.

## Evidence

The rule, stated in the crate's own doc on the helper — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2250-2252`:
```
    /// absence from them is not evidence that an ARN names no resource. What IS
    /// such evidence: an id this catalog holds a record of under a DIFFERENT
    /// resource kind or entity type, which the ARN then contradicts.
```
The code immediately under it, which checks only change sets — `state.rs:2253-2264`:
```
    pub fn policy_key_for(&self, resource: &CatalogResource) -> Option<String> {
        let CatalogResource::Entity { entity_type, id } = resource else {
            return None;
        };
        let names_an_entity = is_entity_type(entity_type)
            // An id this catalog holds as a CHANGE SET is not an entity of any
            // type, whatever type the ARN carries.
            && !self.change_sets.contains_key(id)
            && self
                .entities
                .get(id)
                .is_none_or(|held| &held.entity_type == entity_type);
```
The sibling accessor in the SAME impl that does treat an assessment as a kind the catalog holds — `state.rs:2221-2229`:
```
    pub fn holds_resource(&self, resource: &CatalogResource) -> bool {
        match resource {
            ...
            CatalogResource::ChangeSet { id } => self.change_sets.contains_key(id),
            CatalogResource::Assessment { id } => self.assessments.contains_key(id),
        }
    }
```
And `CatalogResource` itself names three kinds — `src/state/arns.rs:225-231`: `Entity { entity_type, id }`, `ChangeSet { id }`, `Assessment { id }`.

The handlers that consume the helper: `src/ops/get_resource_policy.rs:188-205` (`let Some(policy_key) = store.policy_key_for(&resource) else { ... ResourceNotFoundException ... }` then `unwrap_or_else(|| crate::state::EMPTY_RESOURCE_POLICY.to_string())`) and `src/ops/delete_resource_policy.rs:185-195`.

Test coverage: `tests/all/delete_resource_policy_existence.rs:334-370` enumerates exactly the contradicting-kind rows — `("the held entity's id as a change set", ...)`, `("the held change set itself", ...)`, `("the held change set's id as an entity", ...)` — each asserted `404 ResourceNotFoundException`; there is NO row for an id held as an assessment (grep for `assessment` over `tests/all/resource_policy_*.rs` and `delete_resource_policy_existence.rs` returns nothing), so no test pins today's 200. `tests/all/resource_policy_entity_only.rs:13` states the intended rule in the wider form: "a resource ARN of this catalog naming a resource of any OTHER kind".

## Steps to Reproduce

1. Start the emulator with this service registered.
2. POST /StartChangeSet with Catalog=AWSMarketplace and ChangeSet=[{ChangeType:"CreateProduct", Entity:{Type:"AmiProduct@1.0"}, Details:"{\"ProductTitle\":\"p\",\"DeliveryOptions\":[{\"AmiSource\":{\"AmiId\":\"ami-1\"}}]}"}] and wait for it to reach SUCCEEDED (CHANGE_SET_PREPARING + CHANGE_SET_APPLYING).
3. POST /ListAssessments with Catalog=AWSMarketplace and read an AssessmentId (form `assessment-…`) the applied change produced — or POST /StartAssessment and use the AssessmentId it returns.
4. GET /GetResourcePolicy?resourceArn=arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/AmiProduct/<that assessment id> — observe 200 with Policy "{}" (expected: 404 ResourceNotFoundException naming that id, as the change-set case gives).
5. DELETE /DeleteResourcePolicy?resourceArn=<the same ARN> — observe 200 (expected: 404).
6. For contrast, repeat both calls with the change set's id in place of the assessment id (arn:…:AWSMarketplace/AmiProduct/cs-000000000001) — both answer 404 ResourceNotFoundException.
7. For contrast within the same surface, POST /PutResourcePolicy with the assessment-id ARN — it answers 404, disagreeing with the 200 the read and the delete gave for that ARN.
