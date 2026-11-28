Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 750e814e0eab442e82cf95e9450e7ae6388ca522

# The "id held under another resource kind" rule only excludes change sets, so an ARN naming a held ASSESSMENT under an entity type is served a policy

_Filed: 2026-11-28T09:15:21Z_
_Operation: GetResourcePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `750e814e0eab442e82cf95e9450e7ae6388ca522`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`policy_key_for` is the single decision every resource-policy operation of this crate makes about whether a resource ARN identifies an ENTITY resource. Its own doc comment states the rule it implements: absence from the catalog's records is NOT evidence that an ARN names no resource, but an id the catalog holds "under a DIFFERENT resource kind or entity type" IS, because the ARN then contradicts the record.

The code implements that rule for only ONE of the two other resource kinds this crate has. It excludes an id held in `self.change_sets` and it excludes an id held in `self.entities` under another entity type — but it never consults `self.assessments`, although `CatalogResource::Assessment` is a kind this crate mints ARNs for (`assessment_arn`), parses (`parse_resource_arn`), stores tags for (`resource_tags_write`), reports ARNs for (`stored_resource_arn`) and explicitly declares to carry NO resource-based policy (`CatalogResource::policy_key` returns `None` for `Assessment`).

Consequence: a resource ARN whose resource type is an entity type but whose id is an id this catalog holds as an ASSESSMENT (e.g. `arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/AmiProduct/assessment-000000000001`) passes `policy_key_for`, so

* `GetResourcePolicy` answers 200 with `Policy` = `crate::state::EMPTY_RESOURCE_POLICY` (`{}`) for a resource the crate says carries no resource-based policy at all, and
* `DeleteResourcePolicy` answers 200 for the same ARN,

while the byte-for-byte equivalent ARN built over a held CHANGE SET id is refused 404 `ResourceNotFoundException` — the behaviour `tests/all/delete_resource_policy_existence.rs` pins. `PutResourcePolicy` disagrees with its own two siblings on the same ARN: it additionally calls `store.holds_resource(&resource)`, which answers `false` for `Entity { .. }` whose id is only held as an assessment, so the attach is refused 404 while the report and the clear are served.

The three resource-policy operations therefore give three different answers for one ARN, and the one rule they all go through implements half of what its own comment states.

## Evidence

The rule, stated by the helper's own doc comment — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2246-2253:

```
    /// A policy is recorded per resource in [`Self::resource_policies`] rather
    /// than on this catalog's own record of the resource, and the records here
    /// are only the resources this emulator's own change sets created — an
    /// account's catalog holds entities no change set of this run created — so
    /// absence from them is not evidence that an ARN names no resource. What IS
    /// such evidence: an id this catalog holds a record of under a DIFFERENT
    /// resource kind or entity type, which the ARN then contradicts.
    pub fn policy_key_for(&self, resource: &CatalogResource) -> Option<String> {
```

The code under it, which consults only `change_sets` — state.rs:2254-2268:

```
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
        if !names_an_entity {
            return None;
        }
        resource.policy_key()
```

The crate knows a third kind, and says that kind carries no policy — src/state/arns.rs:224-257:

```
pub enum CatalogResource {
    /// A catalog entity: its `EntityType` and its `ResourceId`.
    Entity { entity_type: String, id: String },
    /// A change set, by its `ResourceId`.
    ChangeSet { id: String },
    /// An assessment, by its `ResourceId`.
    Assessment { id: String },
}
...
    /// A resource of a kind the resource-policy operations are not documented
    /// for carries no resource-based policy at all, so it has no such key:
    /// `None`.
    pub fn policy_key(&self) -> Option<String> {
        match self {
            Self::Entity { entity_type, id } => Some(format!("{entity_type}/{id}")),
            Self::ChangeSet { .. } | Self::Assessment { .. } => None,
        }
    }
```

The sibling lookups in the same `impl` all know the assessment store — state.rs:2221-2229:

```
    pub fn holds_resource(&self, resource: &CatalogResource) -> bool {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .is_some_and(|held| &held.entity_type == entity_type),
            CatalogResource::ChangeSet { id } => self.change_sets.contains_key(id),
            CatalogResource::Assessment { id } => self.assessments.contains_key(id),
        }
    }
```

The served path, which turns the `Some(policy_key)` into a 200 — src/ops/get_resource_policy.rs:188-209:

```
        let Some(policy_key) = store.policy_key_for(&resource) else {
            return Err(GetResourcePolicyErrorModeled::ResourceNotFoundException(
                crate::state::policy_resource_not_found(&resource),
            )
            .into());
        };
        ...
        let policy = store
            .resource_policies
            .get(&policy_key)
            .cloned()
            .unwrap_or_else(|| crate::state::EMPTY_RESOURCE_POLICY.to_string());
        Ok(GetResourcePolicyOutput { policy: Some(policy) })
```

and the same for `DeleteResourcePolicy` — src/ops/delete_resource_policy.rs:185-197.

The disagreeing sibling — src/ops/put_resource_policy.rs:203-215:

```
        let Some(policy_key) = store.policy_key_for(&resource) else { ... };

        if !store.holds_resource(&resource) {
            return Err(PutResourcePolicyErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(&resource),
            )
            .into());
        }
```

Test coverage that pins the CHANGE SET half of the rule and has no assessment row — tests/all/delete_resource_policy_existence.rs:334-369:

```
    let refused_rows: Vec<(&str, String, String)> = vec![
        (
            "the held entity's id under another entity type",
            catalog_arn("SaaSProduct", &held_entity_id),
            entity_missing(&held_entity_id),
        ),
        (
            "the held entity's id as a change set",
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, &held_entity_id),
            entity_missing(&held_entity_id),
        ),
        (
            "the held change set itself",
            change_set_arn.clone(),
            entity_missing(&held_change_set_id),
        ),
```

and tests/all/resource_policy_reads.rs:9-16, which states the rule naming only the change-set kind: "is refused with the modelled `ResourceNotFoundException` (HTTP 404) for an ARN that identifies no entity resource of this catalog (including one naming its other resource kind, a change set — see `resource_policy_entity_only.rs`)".

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and one `AddDeliveryOptions`/`CreateProduct` change against an `AmiProduct` that submits a `DeliveryOptions` entry, then wait CHANGE_SET_PREPARING + CHANGE_SET_APPLYING so the change set reaches SUCCEEDED and `apply_changes` records an assessment (`record_assessment`).
2. POST /ListAssessments with {"Catalog":"AWSMarketplace"} and read `AssessmentSummaryList[0].AssessmentId` — an `assessment-…` id the store now holds — and `AssessmentArn`.
3. GET /GetResourcePolicy?resourceArn=arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/AmiProduct/<that assessment id> — the id is held, but as an ASSESSMENT, under an ARN that calls it an AmiProduct.
4. Observed: 200 with {"Policy":"{}"}. For the same construction over a held CHANGE SET id (`.../AmiProduct/<change set id>`) the service answers 404 ResourceNotFoundException "Requested entity '<id>' does not exist.", which is what tests/all/delete_resource_policy_existence.rs asserts.
5. DELETE /DeleteResourcePolicy?resourceArn=<the same assessment-under-AmiProduct ARN> — observed 200 with {}, again against 404 for the change-set form.
6. POST /PutResourcePolicy with that same ResourceArn and a valid policy — observed 404 ResourceNotFoundException, so the three resource-policy operations answer the same ARN three different ways.
