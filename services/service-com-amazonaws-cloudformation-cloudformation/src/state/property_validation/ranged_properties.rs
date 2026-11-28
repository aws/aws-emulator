// SPDX-License-Identifier: CC0-1.0

//! The question a resource type's schema makes decidable about a NUMBER a property carries:
//! the closed range of values it admits.
//!
//! A resource type whose schema the registry holds is validated AGAINST that schema before its
//! handler is invoked, and a property the schema states a `minimum` or a `maximum` for admits
//! the numbers inside that bound and no other — so a declaration carrying one outside it is one
//! no handler of the type could ever be given. The pre-deployment stage reports it, quoting the
//! rejected number and the bound it broke at that property's own path, and the operation
//! deploys nothing: no resource of the stack is created, so a bound the OWNING service would
//! have enforced on the one call that carries the property is not reached by a create at all,
//! and a bound only the type's SCHEMA states (one the owning API types as a plain number) is
//! enforced where CloudFormation states it.
//!
//! The sentence is the SCHEMA VALIDATOR's own ([`maximum_failure_reason`],
//! [`minimum_failure_reason`]), as it is for a value outside the set its schema states
//! ([`super::enum_properties`]): it names the judged member as a pointer into the resource's
//! property document, the number the declaration carried and the bound that number is held to,
//! which together are what a template author has to act on.
//!
//! Where the schema states the bound on a member NESTED inside a structured property, that
//! member is what is judged — a schema's `minimum`/`maximum` belongs to the property it is
//! declared on wherever that property sits in the declaration — so a row names the CHAIN of
//! members from the resource's property document down to the judged value
//! ([`RangedProperty::property`]) rather than one property name, which is the same reading the
//! value-set and spelling questions of this stage are in.
//!
//! What counts as a NUMBER here is what counts as one for the type question beside it
//! ([`super::typed_properties`]): a number written as one, and the TEXT of a number, which is
//! how a value that travelled through a template parameter reaches such a property. A value
//! that is neither — a structure, a list, an expression this service left unevaluated, a
//! dynamic reference nothing has resolved yet — is admitted, because it is no number this
//! stage can hold to a bound either way.
//!
//! The catalogue is the same kind as the rest of that stage's: one row per (resource type,
//! property) whose schema-stated bound this emulator knows, so a property no row covers is
//! left to the service that owns the resource rather than guessed at.

use ::serde_json::Value;

use super::{member_path, PropertyFailure, StoredResource};

/// One property whose admitted numbers its type's schema states as a closed range.
#[derive(Debug, Clone, Copy)]
pub struct RangedProperty {
    /// The resource type the property belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The chain of members leading from the resource's property document down to the property
    /// the schema states the bound on, each spelled as a template spells it: one name for a
    /// property of the resource itself, and the members stepped through for one nested inside a
    /// structured property.
    pub property: &'static [&'static str],
    /// The smallest number the schema admits, where it states one.
    pub minimum: Option<i64>,
    /// The largest number the schema admits, where it states one.
    pub maximum: Option<i64>,
}

impl RangedProperty {
    /// Why this row refuses `judged`, or `None` for a number inside the bound.
    ///
    /// The MAXIMUM is asked first and the two are exclusive: a closed range is broken at one
    /// end, and the sentence names the end it was broken at.
    fn broken_bound(&self, resource_type: &str, rejected: &str, judged: f64) -> Option<String> {
        let reported = super::reported_value(resource_type, rejected);
        if let Some(maximum) = self.maximum {
            if judged > maximum as f64 {
                return Some(maximum_failure_reason(self.property, reported, maximum));
            }
        }
        if let Some(minimum) = self.minimum {
            if judged < minimum as f64 {
                return Some(minimum_failure_reason(self.property, reported, minimum));
            }
        }
        None
    }
}

/// The largest `Priority` the `AWS::Batch::JobQueue` schema admits.
///
/// Source: the `Maximum` that type's entry in the "AWS resource and property types reference"
/// of the CloudFormation User Guide publishes for `Priority`, which is the `maximum` the type's
/// registry schema states. AWS Batch's own API types the member it is passed to
/// (`CreateJobQueue$priority`, `JobQueueDetail$priority`) as a plain required `Integer` with no
/// range at all, so this ceiling is CloudFormation's to enforce: a declaration past it never
/// reaches that service, and nothing there would refuse it.
pub const BATCH_JOB_QUEUE_MAX_PRIORITY: i64 = 1_000;

/// The bounds the `AWS::Batch::SchedulingPolicy` schema admits a `FairsharePolicy`'s
/// `ComputeReservation` in.
///
/// Source: the same property reference, whose bound is the one AWS Batch publishes for the
/// member the property is passed to — `FairsharePolicy$computeReservation`, "a value used to
/// reserve some of the available maximum vCPU for share identifiers that aren't already used":
/// "The minimum value is 0 and the maximum value is 99"
/// (`smithy_query(query="doc", shape="FairsharePolicy", member="computeReservation")` of the
/// `batch` model, which that service holds itself to in
/// `::service_com_amazonaws_batch_awsbatchv20160810::state::MIN_COMPUTE_RESERVATION`).
pub const BATCH_MIN_COMPUTE_RESERVATION: i64 = 0;
/// See [`BATCH_MIN_COMPUTE_RESERVATION`].
pub const BATCH_MAX_COMPUTE_RESERVATION: i64 = 99;

/// The bounds the same schema admits that policy's `ShareDecaySeconds` in.
///
/// Source: as above, for `FairsharePolicy$shareDecaySeconds`, "the amount of time (in seconds)
/// to use to calculate a fair-share percentage for each share identifier in use": "A value of
/// zero (0) indicates the default minimum time window (600 seconds). The maximum supported
/// value is 604800 (1 week)."
pub const BATCH_MIN_SHARE_DECAY_SECONDS: i64 = 0;
/// See [`BATCH_MIN_SHARE_DECAY_SECONDS`].
pub const BATCH_MAX_SHARE_DECAY_SECONDS: i64 = 604_800;

/// The floor the `AWS::ApiGateway::UsagePlan` schema admits a `QuotaSettings`' `Limit` and
/// `Offset` in.
///
/// Source: the bound API Gateway publishes for the members the properties are passed to —
/// `QuotaSettings$limit`, "the target maximum number of requests that can be made in a given
/// time period", and `QuotaSettings$offset`, "the number of requests subtracted from the given
/// limit in the initial time period" — which that type's entry in the "AWS resource and property
/// types reference" publishes as the `Minimum` of each, and which the type's registry schema
/// carries as their `minimum`. Neither member carries a ceiling: the apigateway model types both
/// as a plain `Integer` with no range
/// (`smithy_query(query="shape", shape="QuotaSettings")`), and the reference publishes no
/// `Maximum` for either.
pub const APIGATEWAY_MIN_QUOTA_REQUESTS: i64 = 0;

/// The properties this stage knows a schema-stated numeric bound of.
///
/// Source for each row: the bound the resource type's REGISTRY SCHEMA states for the property,
/// as its entry in the "AWS resource and property types reference" publishes it. That is the
/// bound the schema validation holds a declaration to — a number outside it never reaches the
/// owning service at all — and it is stated by the TYPE even where the owning API types the
/// member as a plain number, which is why this question cannot be left to the service that
/// creates the resource.
pub const RANGED_PROPERTIES: &[RangedProperty] = &[
    // The `Priority` of an `AWS::Batch::JobQueue`, "the priority of the job queue … a higher
    // value for priority indicates a higher priority": the schema states the ceiling
    // ([`BATCH_JOB_QUEUE_MAX_PRIORITY`]) and no floor.
    RangedProperty {
        resource_type: crate::state::template::BATCH_JOB_QUEUE,
        property: &["Priority"],
        minimum: None,
        maximum: Some(BATCH_JOB_QUEUE_MAX_PRIORITY),
    },
    // The two bounded members of an `AWS::Batch::SchedulingPolicy`'s `FairsharePolicy`, each
    // nested one level down inside that structured property.
    RangedProperty {
        resource_type: crate::state::template::BATCH_SCHEDULING_POLICY,
        property: &["FairsharePolicy", "ComputeReservation"],
        minimum: Some(BATCH_MIN_COMPUTE_RESERVATION),
        maximum: Some(BATCH_MAX_COMPUTE_RESERVATION),
    },
    RangedProperty {
        resource_type: crate::state::template::BATCH_SCHEDULING_POLICY,
        property: &["FairsharePolicy", "ShareDecaySeconds"],
        minimum: Some(BATCH_MIN_SHARE_DECAY_SECONDS),
        maximum: Some(BATCH_MAX_SHARE_DECAY_SECONDS),
    },
    // The two bounded members of an `AWS::ApiGateway::UsagePlan`'s `Quota`, each nested one
    // level down inside that structured property: a request count is a count, so neither the
    // limit a quota allows nor the offset subtracted from it can be negative
    // ([`APIGATEWAY_MIN_QUOTA_REQUESTS`]), and a declaration past that floor names no quota API
    // Gateway could meter a plan with.
    RangedProperty {
        resource_type: crate::state::template::APIGATEWAY_USAGE_PLAN,
        property: &["Quota", "Limit"],
        minimum: Some(APIGATEWAY_MIN_QUOTA_REQUESTS),
        maximum: None,
    },
    RangedProperty {
        resource_type: crate::state::template::APIGATEWAY_USAGE_PLAN,
        property: &["Quota", "Offset"],
        minimum: Some(APIGATEWAY_MIN_QUOTA_REQUESTS),
        maximum: None,
    },
];

/// `Some(failure)` for a resource carrying a number outside the bound its type's schema states
/// for a property ([`RANGED_PROPERTIES`]) — the first such property in the order the catalogue
/// states them; `None` for a type with no row, for a property the declaration does not carry,
/// for a value that is no number, and for every number the bound admits.
///
/// A row's chain of members is walked by the ONE walker of this crate's state that steps a
/// declaration's property document down a nested member path
/// ([`crate::state::template::property_at`]), so a chain that runs into a value no member can
/// be read off judges nothing — the same reading an absent member gets, since neither is a
/// number held to the bound.
pub(super) fn value_outside_its_range(resource: &StoredResource) -> Option<PropertyFailure> {
    RANGED_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .find_map(|row| {
            let declared = crate::state::template::property_at(&resource.properties, row.property);
            let (rejected, judged) = judged_number(declared?)?;
            Some(PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: row.broken_bound(&resource.resource_type, &rejected, judged)?,
                path: member_path(&resource.logical_id, row.property),
            })
        })
}

/// The number one declared member carries, with the text a refusal of it quotes: the value
/// itself where the declaration states a number, and the number its TEXT spells where the
/// declaration states text — the coercion a template's text form is carried through
/// ([`super::typed_properties`] is where that reading is spelled for the type question).
///
/// `None` for everything that is no number: a structure or a list (an expression this service
/// left unevaluated among them), text that spells no number, a text still carrying a DYNAMIC
/// REFERENCE — which this stage runs before the deployment resolves, so the value it stands for
/// is not known here — and a spelling that is no finite number at all.
fn judged_number(declared: &Value) -> Option<(String, f64)> {
    match declared {
        Value::Number(number) => Some((number.to_string(), number.as_f64()?)),
        Value::String(text) => {
            if crate::state::dynamic_references::carries_a_reference(text) {
                return None;
            }
            let text = text.trim();
            let judged: f64 = text.parse().ok()?;
            judged.is_finite().then(|| (text.to_string(), judged))
        }
        _ => None,
    }
}

/// How the schema validator reports a number above the `maximum` its property's schema states:
/// fixed text around the member that was judged, the number the declaration carried and the
/// ceiling it is held to.
///
/// Every variable part comes from what was judged — the member as a pointer into the resource's
/// property document ([`pointer`]), the bound the one its property is held to
/// ([`RANGED_PROPERTIES`]) and the number the declaration's own, as far as a failure of this
/// stage may say it ([`super::reported_value`]) — and the number is named, as it is for a value
/// outside its set or its pattern, because a rejected number is only actionable beside the
/// bound it had to be inside.
pub fn maximum_failure_reason(property: &[&str], value: &str, maximum: i64) -> String {
    format!(
        "{}: {value} is not less or equal to {maximum}",
        pointer(property)
    )
}

/// How the same validator reports a number below the `minimum` its property's schema states,
/// which is that sentence for the other end of the range.
pub fn minimum_failure_reason(property: &[&str], value: &str, minimum: i64) -> String {
    format!(
        "{}: {value} is not greater or equal to {minimum}",
        pointer(property)
    )
}

/// The judged member as the schema validator names it: the JSON pointer of the member inside
/// the RESOURCE's property document, which is the document that validator was handed.
///
/// That is the same chain of members the validation's own path names, under the template's
/// `/Resources/<LogicalId>/Properties` ([`member_path`]) — the sentence names where in the
/// document the number sits, the path where in the template.
fn pointer(property: &[&str]) -> String {
    format!("#/{}", property.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde_json::json;
    use ::std::collections::BTreeMap;

    /// One declared resource of `resource_type` carrying `properties`.
    fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: format!("{logical_id}-physical"),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// The two types whose rows the table below is about.
    const JOB_QUEUE: &str = crate::state::template::BATCH_JOB_QUEUE;
    const SCHEDULING_POLICY: &str = crate::state::template::BATCH_SCHEDULING_POLICY;

    /// Which end of a row's range a declaration broke, for the row that expects a refusal.
    #[derive(Debug, Clone, Copy)]
    enum Broke {
        Maximum,
        Minimum,
    }

    /// One row of the table: the case, the type declared, its property document, and the
    /// `(member chain, end of the range, number)` the stage must report — or `None` where
    /// every judged number is one the schema admits.
    type RangeRow<'a> = (
        &'a str,
        &'a str,
        Value,
        Option<(&'a [&'a str], Broke, &'a str)>,
    );

    /// RULE: a number outside the closed range its type's schema states for the property fails
    /// the validation, quoting that number and the bound it broke at the path of the member
    /// that carries it, and a number the range admits — each published bound included — does
    /// not.
    ///
    /// The rows span what the rule generalises over: which end of the range is broken, which
    /// property and which resource type states it (properties the report that led here did not
    /// cite among them), how deep in the declaration the member sits, the form the number takes
    /// (a number, the text of one, a fraction), by how much the bound is broken (one past it
    /// and far past it), the bounds themselves, and the values this stage judges no number of
    /// at all.
    #[test]
    fn a_number_outside_its_schemas_range_fails_the_validation() {
        let rows: &[RangeRow] = &[
            (
                "a queue whose priority is one past the ceiling the schema states",
                JOB_QUEUE,
                json!({"Priority": 1001, "ComputeEnvironmentOrder": []}),
                Some((&["Priority"], Broke::Maximum, "1001")),
            ),
            (
                "a queue whose priority is far past that ceiling",
                JOB_QUEUE,
                json!({"Priority": 1_000_000}),
                Some((&["Priority"], Broke::Maximum, "1000000")),
            ),
            (
                "a queue whose priority is the TEXT of a number past that ceiling",
                JOB_QUEUE,
                json!({"Priority": "1001"}),
                Some((&["Priority"], Broke::Maximum, "1001")),
            ),
            (
                "a queue whose priority IS the ceiling",
                JOB_QUEUE,
                json!({"Priority": 1000}),
                None,
            ),
            (
                "a queue whose priority is well inside the range",
                JOB_QUEUE,
                json!({"Priority": 7}),
                None,
            ),
            (
                // The schema states no floor for this property, so a number below zero is
                // left to the service that owns the queue.
                "a queue whose priority is negative, which the schema states no floor for",
                JOB_QUEUE,
                json!({"Priority": -5}),
                None,
            ),
            (
                "a queue whose priority is an expression this service left unevaluated",
                JOB_QUEUE,
                json!({"Priority": {"Ref": "Priority"}}),
                None,
            ),
            (
                "a queue whose priority is a dynamic reference nothing has resolved yet",
                JOB_QUEUE,
                json!({"Priority": "{{resolve:ssm:/batch/priority}}"}),
                None,
            ),
            (
                "a queue whose priority is text that spells no number",
                JOB_QUEUE,
                json!({"Priority": "the highest"}),
                None,
            ),
            (
                "a queue declaring no priority at all",
                JOB_QUEUE,
                json!({"ComputeEnvironmentOrder": []}),
                None,
            ),
            (
                "a policy reserving more compute than the schema admits",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 150}}),
                Some((
                    &["FairsharePolicy", "ComputeReservation"],
                    Broke::Maximum,
                    "150",
                )),
            ),
            (
                "a policy reserving one past that ceiling",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 100}}),
                Some((
                    &["FairsharePolicy", "ComputeReservation"],
                    Broke::Maximum,
                    "100",
                )),
            ),
            (
                "a policy reserving exactly that ceiling",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 99}}),
                None,
            ),
            (
                "a policy reserving one below the floor the schema states",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": -1}}),
                Some((
                    &["FairsharePolicy", "ComputeReservation"],
                    Broke::Minimum,
                    "-1",
                )),
            ),
            (
                "a policy reserving exactly that floor",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 0}}),
                None,
            ),
            (
                "a policy reserving a fraction below that floor",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": -0.5}}),
                Some((
                    &["FairsharePolicy", "ComputeReservation"],
                    Broke::Minimum,
                    "-0.5",
                )),
            ),
            (
                // The other bounded member of the same structure, which the report that led
                // here did not cite: one past its own ceiling.
                "a policy decaying its shares over longer than the schema admits",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ShareDecaySeconds": 604_801}}),
                Some((
                    &["FairsharePolicy", "ShareDecaySeconds"],
                    Broke::Maximum,
                    "604801",
                )),
            ),
            (
                "a policy decaying its shares over exactly that ceiling",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ShareDecaySeconds": 604_800}}),
                None,
            ),
            (
                "a policy decaying its shares over less than the floor",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ShareDecaySeconds": -60}}),
                Some((
                    &["FairsharePolicy", "ShareDecaySeconds"],
                    Broke::Minimum,
                    "-60",
                )),
            ),
            (
                // The catalogue's order decides which failure is reported when a document
                // breaks two bounds: the reservation's row comes first.
                "a policy breaking both bounds of its fair-share policy",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 150,
                                           "ShareDecaySeconds": 604_801}}),
                Some((
                    &["FairsharePolicy", "ComputeReservation"],
                    Broke::Maximum,
                    "150",
                )),
            ),
            (
                "a policy whose fair-share members are all inside their ranges",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"ComputeReservation": 50,
                                           "ShareDecaySeconds": 60,
                                           "ShareDistribution": [
                                               {"ShareIdentifier": "A*", "WeightFactor": 1.0}
                                           ]}}),
                None,
            ),
            (
                "a policy declaring no fair-share policy at all",
                SCHEDULING_POLICY,
                json!({"QuotaSharePolicy": {"IdleResourceAssignmentStrategy": "PROPORTIONAL"}}),
                None,
            ),
            (
                "a policy whose fair-share policy is an unevaluated expression",
                SCHEDULING_POLICY,
                json!({"FairsharePolicy": {"Fn::ImportValue": "Policy"}}),
                None,
            ),
            (
                "a type this stage knows no numeric bound of",
                crate::state::template::BATCH_COMPUTE_ENVIRONMENT,
                json!({"Type": "UNMANAGED", "UnmanagedvCpus": 1_000_000}),
                None,
            ),
        ];
        for (case, resource_type, properties, reported) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = super::super::failure(&resources);
            let expected = reported.map(|(property, broke, value)| {
                let row = RANGED_PROPERTIES
                    .iter()
                    .find(|row| row.resource_type == *resource_type && row.property == property)
                    .expect("the table names a property the catalogue holds a row for");
                let reason = match broke {
                    Broke::Maximum => maximum_failure_reason(
                        property,
                        value,
                        row.maximum.expect("the row states a ceiling"),
                    ),
                    Broke::Minimum => minimum_failure_reason(
                        property,
                        value,
                        row.minimum.expect("the row states a floor"),
                    ),
                };
                (
                    reason,
                    format!("/Resources/R/Properties/{}", property.join("/")),
                )
            });
            assert_eq!(
                failed
                    .as_ref()
                    .map(|failure| (failure.reason.clone(), failure.path.clone())),
                expected,
                "case {case:?}: expected the failure {expected:?}, got {failed:?}"
            );
        }
    }
}
