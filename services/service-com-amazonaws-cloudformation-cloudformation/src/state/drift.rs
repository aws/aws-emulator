// SPDX-License-Identifier: CC0-1.0

//! Detecting whether a stack's resources have DRIFTED from the configuration its template
//! declared.
//!
//! ## The rule this module implements
//!
//! `DetectStackDrift` "compares the actual configuration of the resource with its expected
//! template configuration" and "only resource properties explicitly defined in the stack
//! template are checked for drift"; a stack "is considered to have drifted if one or more
//! of its resources differ from their expected template configurations".
//!
//! So the comparison has two sides, and each comes from a different place:
//!
//! * the EXPECTED configuration is [`StoredResource::properties`] — the resource's declared
//!   `Properties` with the template's intrinsic functions evaluated, which is exactly the
//!   request the owning service's create was given;
//! * the ACTUAL configuration is what the OWNING SERVICE reports for the resource now
//!   ([`Provisioner::actual_configuration`](super::provisioner::Provisioner::actual_configuration)),
//!   read through the same in-process, signed callback the create went out on.
//!
//! Nothing here reports a resource as checked that was not read from its owning service:
//! a resource of a type this emulator provisions nothing for, or whose owning service does
//! not serve the read, is `NOT_CHECKED` — "any resources that do not currently support
//! drift detection have a status of NOT_CHECKED".
//!
//! ## What each status means here
//!
//! * `DELETED` — the owning service answers that it no longer holds the resource.
//! * `MODIFIED` — a property the template declares differs from the value that service
//!   reports under the same name.
//! * `IN_SYNC` — the resource is there and no declared property it reports differs.
//! * `NOT_CHECKED` — nothing was asked of any service for it (declared-only resource, a
//!   type with no provisioner, a type whose configuration no read here reports, or an owning
//!   service whose read is still a scaffold stub).
//!   It is the outcome of a check that never happened, so it is never REPORTED in a
//!   `StackResourceDrift` ("CloudFormation does not currently return this value"): a
//!   whole-stack detection holds no record for such a resource, and a request for that ONE
//!   resource's drift is refused instead (`crate::ops::detect_stack_resource_drift`).
//! * `UNKNOWN` — the read was refused for another reason, so the check could not run; the
//!   detection as a whole is then `DETECTION_FAILED`, with that refusal as its reason.
//!
//! Only the properties BOTH sides name are compared: a service whose read spells the same
//! configuration under other member names reports nothing comparable, and a resource of
//! such a type is reported as existing rather than as modified on a name mismatch.
//!
//! One declared property is left out of the comparison: the one the declaration states the
//! resource's own IDENTITY in ([`identity_property`]). The actual-state read is addressed BY
//! that identity, so a read that answered resolved it to the resource it reported, and the
//! form the owning service renders it back in says nothing about the resource's
//! configuration; a resource the identity no longer resolves to is `DELETED`.
//!
//! ## What a MODIFIED resource differs IN
//!
//! A resource that differs carries the difference itself, not just the verdict: a
//! `PropertyDifference` per LEAF the two sides disagree about, at "the fully-qualified path
//! to the resource property" — the declared property, then the members and list indices
//! inside it — with both values and which kind of difference it is. So the comparison walks
//! INTO a declared property the owning service reports: a value the two sides spell
//! differently is `NOT_EQUAL`, one the reported document no longer carries is `REMOVE` ("the
//! property has been removed from the current resource configuration"), and a list element
//! only the reported document carries is `ADD` ("a value has been added to a resource
//! property that's an array or list data type").
//!
//! The two rules are the same rule at two depths, and the difference between them is what
//! each side's absence means. A TOP-LEVEL property the service's read does not report at all
//! is no evidence of anything — a service reports the configuration it models, not every
//! property a template may declare. INSIDE a value that service does report, what it reports
//! is that value in full, so a member missing from it really is missing from the resource's
//! configuration.

use serde_json::Value;

use super::provisioning::{owner_for, ActualConfiguration};
use super::{resource_drift_status, StoredResource, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_UNKNOWN};

/// The error code an owning service answers when the read this module needs is still one
/// of this emulator's scaffold stubs. Such a resource cannot be checked for drift here,
/// which is what `NOT_CHECKED` states.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The model's `DifferenceType`: which kind of difference one property difference is.
pub mod difference_type {
    /// "A value has been added to a resource property that's an array or list data type."
    pub const ADD: &str = "ADD";
    /// "The property has been removed from the current resource configuration."
    pub const REMOVE: &str = "REMOVE";
    /// "The current property value differs from its expected value (as defined in the
    /// stack template and any values specified as template parameters)."
    pub const NOT_EQUAL: &str = "NOT_EQUAL";
}

/// One property the two sides of the comparison disagree about, as the modelled
/// `PropertyDifference` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyDiff {
    /// "The fully-qualified path to the resource property": the declared property, and the
    /// members and list indices inside it, each segment after a `/`.
    pub property_path: String,
    /// "The expected property value of the resource property, as defined in the stack
    /// template" — empty for a difference the expected configuration has no value in
    /// ([`difference_type::ADD`]).
    pub expected_value: String,
    /// "The actual property value of the resource property" — empty for a difference the
    /// reported configuration has no value in ([`difference_type::REMOVE`]).
    pub actual_value: String,
    /// One of [`difference_type`].
    pub difference_type: String,
}

impl PropertyDiff {
    /// A leaf the two sides spell differently.
    fn not_equal(path: &str, expected: &Value, actual: &Value) -> Self {
        Self {
            property_path: path.to_string(),
            expected_value: value_text(expected),
            actual_value: value_text(actual),
            difference_type: difference_type::NOT_EQUAL.to_string(),
        }
    }

    /// A value the reported configuration no longer carries.
    fn removed(path: &str, expected: &Value) -> Self {
        Self {
            property_path: path.to_string(),
            expected_value: value_text(expected),
            actual_value: String::new(),
            difference_type: difference_type::REMOVE.to_string(),
        }
    }

    /// A list element only the reported configuration carries.
    fn added(path: &str, actual: &Value) -> Self {
        Self {
            property_path: path.to_string(),
            expected_value: String::new(),
            actual_value: value_text(actual),
            difference_type: difference_type::ADD.to_string(),
        }
    }
}

/// The two sides of one resource's comparison, as the check really read them.
///
/// Recorded for every resource a check COMPARED, so the drift report of that resource is
/// the comparison that was carried out rather than one recomputed on a read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceDriftDetail {
    /// "A JSON structure that contains the expected property values of the stack resource,
    /// as defined in the stack template and any values specified as template parameters" —
    /// absent for a resource that declared no properties, and for one the owning service no
    /// longer holds ("for resources whose StackResourceDriftStatus is DELETED, this
    /// structure will not be present").
    pub expected_properties: Option<String>,
    /// "A JSON structure that contains the actual property values of the stack resource" —
    /// absent for a resource no configuration was read for, for the same reasons.
    pub actual_properties: Option<String>,
    /// "A collection of the resource properties whose actual values differ from their
    /// expected values", which is what makes the resource `MODIFIED`.
    pub differences: Vec<PropertyDiff>,
    /// `StackResourceDrift$DriftStatusReason`: "the reason for the drift status", for a
    /// resource whose check could not be run.
    pub reason: Option<String>,
}

/// One resource's drift outcome, as the detection computed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDrift {
    pub logical_id: String,
    /// One of [`resource_drift_status`].
    pub status: String,
    /// What the comparison read and found, for a resource it really compared. `None` for
    /// one no check was asked of any service for — nothing was compared, so there is no
    /// comparison to report.
    pub detail: Option<ResourceDriftDetail>,
}

/// What one drift detection found across the resources it covered.
#[derive(Debug, Clone, Default)]
pub struct Detected {
    /// One entry per resource the detection covered, in the order they were checked.
    pub results: Vec<ResourceDrift>,
    /// The reason the detection could not be carried out for at least one resource — an
    /// owning service's own refusal. `None` when every covered resource was either checked
    /// or is one no check is supported for.
    pub failure: Option<String>,
}

impl Detected {
    /// The stack-level `StackDriftStatus` these per-resource outcomes add up to.
    ///
    /// `DRIFTED` when one or more resources differ from their expected configuration ("a
    /// stack is considered to have drifted if one or more of its resources have drifted"),
    /// `UNKNOWN` when drift detection could not be run for a resource of the stack, and
    /// `IN_SYNC` otherwise — including for a stack none of whose resources support drift
    /// detection, which has no resource differing from its template.
    pub fn stack_drift_status(&self) -> &'static str {
        if self.drifted_count() > 0 {
            return DRIFT_STATUS_DRIFTED;
        }
        if self
            .results
            .iter()
            .any(|result| result.status == resource_drift_status::UNKNOWN)
        {
            return DRIFT_STATUS_UNKNOWN;
        }
        DRIFT_STATUS_IN_SYNC
    }

    /// How many resources differ from their expected configuration —
    /// `DriftedStackResourceCount`, which "will be 0 for stacks whose drift status is
    /// IN_SYNC".
    pub fn drifted_count(&self) -> i32 {
        let drifted = self
            .results
            .iter()
            .filter(|result| is_drifted(&result.status))
            .count();
        i32::try_from(drifted).unwrap_or(i32::MAX)
    }
}

/// Whether a resource-level drift status is one that makes the resource — and so its
/// stack — drifted.
pub fn is_drifted(status: &str) -> bool {
    status == resource_drift_status::MODIFIED || status == resource_drift_status::DELETED
}

/// Run one drift detection over `resources`, asking each resource's owning service what
/// that resource IS now.
///
/// The calls are made with no store lock held (the caller is what releases it), one
/// resource at a time in the order the stack holds them, so a detection costs one read per
/// covered resource and no more.
pub async fn detect(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
) -> Detected {
    let mut detected = Detected::default();
    for resource in resources {
        let (status, detail) = match owner_for(&resource.resource_type) {
            // Nothing was created for this resource in any service, so there is no actual
            // configuration to compare: the type does not support drift detection here.
            Some(provisioner) if resource.materialised => {
                match provisioner.actual_configuration(ctx, region, resource).await {
                    // Nothing was asked of any service for this resource, because no read of
                    // its type's configuration is served here: no comparison was carried
                    // out, which is what `NOT_CHECKED` states — and is NOT the resource
                    // having been deleted, which only that service's own answer establishes.
                    Ok(ActualConfiguration::Unchecked) => {
                        (resource_drift_status::NOT_CHECKED.to_string(), None)
                    }
                    // "For resources whose StackResourceDriftStatus is DELETED", neither
                    // property structure "will be present": there is no configuration left
                    // to report, and none was compared.
                    Ok(ActualConfiguration::Gone) => (
                        resource_drift_status::DELETED.to_string(),
                        Some(ResourceDriftDetail::default()),
                    ),
                    Ok(ActualConfiguration::Reported(actual)) => {
                        let differences = resource_differences(resource, &actual);
                        let status = if differences.is_empty() {
                            resource_drift_status::IN_SYNC
                        } else {
                            resource_drift_status::MODIFIED
                        };
                        (
                            status.to_string(),
                            Some(ResourceDriftDetail {
                                expected_properties: properties_text(&resource.properties),
                                actual_properties: properties_text(&actual),
                                differences,
                                reason: None,
                            }),
                        )
                    }
                    // The owning service does not serve the read, so this resource's
                    // configuration cannot be checked here.
                    Err(refusal) if refusal.code == NOT_IMPLEMENTED => {
                        (resource_drift_status::NOT_CHECKED.to_string(), None)
                    }
                    Err(refusal) => {
                        // "DETECTION_FAILED: The stack drift detection operation has
                        // failed for at least one resource in the stack."
                        let reason = format!(
                            "Failed to detect drift on resource {}: {refusal}",
                            resource.logical_id
                        );
                        detected.failure.get_or_insert_with(|| reason.clone());
                        (
                            resource_drift_status::UNKNOWN.to_string(),
                            Some(ResourceDriftDetail {
                                // The expected configuration is known — it is the
                                // template's — and nothing was read back, so "the reason
                                // for the drift status" is the refusal that stopped it.
                                expected_properties: properties_text(&resource.properties),
                                actual_properties: None,
                                differences: Vec::new(),
                                reason: Some(reason),
                            }),
                        )
                    }
                }
            }
            _ => (resource_drift_status::NOT_CHECKED.to_string(), None),
        };
        detected.results.push(ResourceDrift {
            logical_id: resource.logical_id.clone(),
            status,
            detail,
        });
    }
    detected
}

/// One side of the comparison as the modelled `Properties` member carries it: "a JSON
/// structure that contains the … property values of the stack resource".
///
/// A resource that declared no properties has no such structure, which the shape spells as
/// an absent member rather than as an empty one.
fn properties_text(value: &Value) -> Option<String> {
    (!value.is_null()).then(|| value.to_string())
}

/// One value as the single text a `PropertyValue` carries it in: a scalar's own spelling,
/// and a composite's compact JSON — the text the create would have sent it as.
fn value_text(value: &Value) -> String {
    scalar_text(value).unwrap_or_else(|| value.to_string())
}

/// Whether the ACTUAL configuration differs from the EXPECTED one, over the properties the
/// template declared — which is exactly whether the comparison found any difference.
pub fn differs(expected: &Value, actual: &Value) -> bool {
    !differences(expected, actual).is_empty()
}

/// `CreateChangeSet$DeploymentMode`'s only value: "REVERT_DRIFT — Creates a drift-aware
/// change set that brings actual resource states in line with template definitions."
pub const DEPLOYMENT_MODE_REVERT_DRIFT: &str = "REVERT_DRIFT";

/// Whether a change set's `DeploymentMode` asks for the resources' ACTUAL state to be
/// compared against the template — and, when it is executed, brought back to it.
pub fn reverts_drift(deployment_mode: Option<&str>) -> bool {
    deployment_mode == Some(DEPLOYMENT_MODE_REVERT_DRIFT)
}

/// One resource the service that owns it reports in a configuration the stack did NOT
/// record for it — what makes the resource, and so its stack, drifted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftedResource {
    pub logical_id: String,
    pub resource_type: String,
    pub physical_resource_id: String,
    /// The configuration that service reports the resource in, as it reported it.
    pub actual: Value,
    /// Every property it disagrees with the recorded configuration about
    /// ([`differences`]), each carrying both values.
    pub differences: Vec<PropertyDiff>,
}

/// Those of `resources` whose owning service reports a configuration the stack did not
/// record, in the order the stack holds them.
///
/// The same question [`detect`] asks, for the callers that need the ACTUAL configuration
/// itself rather than a drift report to store: a change set whose `DeploymentMode` compares
/// the template against the resource's actual state, and the execution of such a change set,
/// which has to put the recorded configuration back where that state differs.
///
/// The calls are made with no store lock held (the caller is what releases it), one resource
/// at a time, so this costs one read per resource and no more. A resource no service here
/// owns, one nothing was created for, and a read a service does not serve establish nothing:
/// such a resource is not reported as drifted.
pub async fn drifted(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
) -> Vec<DriftedResource> {
    let mut drifted = Vec::new();
    for resource in resources {
        let Some(provisioner) = owner_for(&resource.resource_type) else {
            continue;
        };
        if !resource.materialised {
            continue;
        }
        let Ok(ActualConfiguration::Reported(actual)) =
            provisioner.actual_configuration(ctx, region, resource).await
        else {
            continue;
        };
        let differences = resource_differences(resource, &actual);
        if differences.is_empty() {
            continue;
        }
        drifted.push(DriftedResource {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: resource.physical_resource_id.clone(),
            actual,
            differences,
        });
    }
    drifted
}

/// Every property the ACTUAL configuration disagrees with the EXPECTED one about, for ONE
/// resource its owning service reported — the comparison a drift check carries out.
///
/// The same walk as [`differences`], with the resource's own IDENTITY left out of it
/// ([`identity_property`]), because the read that reported the resource was ADDRESSED by
/// that identity.
pub fn resource_differences(resource: &StoredResource, actual: &Value) -> Vec<PropertyDiff> {
    differences_beside(
        &resource.properties,
        actual,
        identity_property(resource).as_deref(),
    )
}

/// The declared property that holds the resource's own IDENTITY — the identifier the
/// actual-state read was ADDRESSED by — or `None` for a resource whose declaration names it
/// nowhere, and for one whose owning service answers for it under an identifier IT minted.
///
/// A resource is read back from the service that owns it by its physical id
/// ([`super::provisioner`]), so a read that ANSWERED resolved that very identifier to the
/// resource it reported: the property the declaration states that identifier in names the
/// resource that answered, whichever of the forms it resolves the service renders the
/// identifier back in. Comparing it would read the identifier's FORM as a renamed resource,
/// and a resource the identifier no longer resolves to is reported `DELETED` by the read
/// itself rather than as a name that changed.
///
/// WHICH property a declaration names the resource in is
/// [`super::template::name_property`], the one list of those properties this crate holds.
fn identity_property(resource: &StoredResource) -> Option<String> {
    let property =
        super::template::name_property(&resource.resource_type, &resource.properties)?;
    let declared = resource.properties.get(&property)?;
    (scalar_text(declared)? == resource.physical_resource_id).then_some(property)
}

/// Every property the ACTUAL configuration disagrees with the EXPECTED one about, over the
/// properties the template declared.
///
/// Only a declared property the actual document also carries under the same name is
/// compared — "only resource properties explicitly defined in the stack template are
/// checked for drift", and a property the owning service's read does not report is not
/// evidence of anything. Each compared property is then walked to its LEAVES, so what is
/// reported is the fully-qualified path of every value the two sides disagree about rather
/// than the whole property.
pub fn differences(expected: &Value, actual: &Value) -> Vec<PropertyDiff> {
    differences_beside(expected, actual, None)
}

/// [`differences`] over every declared property BESIDE `identity`, which is the one property
/// the comparison does not establish anything about ([`identity_property`]).
fn differences_beside(
    expected: &Value,
    actual: &Value,
    identity: Option<&str>,
) -> Vec<PropertyDiff> {
    let mut found = Vec::new();
    let (Some(declared), Some(reported)) = (expected.as_object(), actual.as_object()) else {
        return found;
    };
    for (property, declared) in declared {
        if Some(property.as_str()) == identity {
            continue;
        }
        if let Some(reported) = reported.get(property) {
            compare(&format!("/{property}"), declared, reported, 0, &mut found);
        }
    }
    found
}

/// The deepest a compared property is descended into, past which the two values are
/// compared whole.
///
/// The declared side of the comparison comes from a caller-supplied template body, so the
/// nesting is the REQUESTER's choice and a recursive walk over it is bounded by a constant
/// rather than by the input: exhausting the native stack aborts the process instead of
/// unwinding. This is the nesting a template body is already held to
/// ([`crate::state::template::MAX_TEMPLATE_DEPTH`]), so no document a stored template can
/// carry reaches it and the bound changes nothing a client sees.
const MAX_COMPARISON_DEPTH: usize = super::template::MAX_TEMPLATE_DEPTH;

/// Walk one declared value against the one the owning service reports, appending a
/// difference per leaf the two disagree about.
///
/// A scalar is compared on its TEXT, because a configuration a template writes as a number
/// or a boolean is carried as the string an API member takes (`VisibilityTimeout: 30`
/// against `"30"`). Two documents are compared member by member and element by element, so
/// the path names the value that changed; what one side carries and the other does not is a
/// removal or an addition. The descent stops at [`MAX_COMPARISON_DEPTH`], past which the
/// two values are compared as the documents they are.
fn compare(
    path: &str,
    expected: &Value,
    actual: &Value,
    depth: usize,
    found: &mut Vec<PropertyDiff>,
) {
    // A composite the service reports as a STRING is the document the create sent it as, so
    // the reported text is parsed and walked as that document (see [`same_value`]).
    let parsed = reported_document(expected, actual);
    let actual = parsed.as_ref().unwrap_or(actual);
    if depth >= MAX_COMPARISON_DEPTH {
        if !same_value(expected, actual) {
            found.push(PropertyDiff::not_equal(path, expected, actual));
        }
        return;
    }
    match (expected, actual) {
        (Value::Object(declared), Value::Object(reported)) => {
            for (member, declared) in declared {
                let path = format!("{path}/{member}");
                match reported.get(member) {
                    Some(reported) => compare(&path, declared, reported, depth + 1, found),
                    None => found.push(PropertyDiff::removed(&path, declared)),
                }
            }
        }
        (Value::Array(declared), Value::Array(reported)) => {
            for (index, element) in declared.iter().enumerate() {
                let path = format!("{path}/{index}");
                match reported.get(index) {
                    Some(reported) => compare(&path, element, reported, depth + 1, found),
                    None => found.push(PropertyDiff::removed(&path, element)),
                }
            }
            for (index, element) in reported.iter().enumerate().skip(declared.len()) {
                found.push(PropertyDiff::added(&format!("{path}/{index}"), element));
            }
        }
        _ => {
            if !same_value(expected, actual) {
                found.push(PropertyDiff::not_equal(path, expected, actual));
            }
        }
    }
}

/// The document a reported STRING spells, when the declared value is a composite: `None`
/// when the declared value is a scalar, or when the reported text is not a document at all.
fn reported_document(expected: &Value, actual: &Value) -> Option<Value> {
    if scalar_text(expected).is_some() {
        return None;
    }
    ::serde_json::from_str::<Value>(actual.as_str()?).ok()
}

/// Whether two configuration values are the same value.
///
/// A scalar is compared on its text. A COMPOSITE the service reports as a STRING is the
/// document the create sent it as (an `AWS::SQS::Queue`'s `RedrivePolicy` is declared as
/// an object and carried as the JSON text of an attribute), so the reported text is PARSED
/// and compared as the document it is — key order and spacing are the serialiser's
/// choice, not a change to the configuration — and only a text that is not JSON at all
/// falls back to comparing the text.
fn same_value(expected: &Value, actual: &Value) -> bool {
    match (scalar_text(expected), scalar_text(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        (None, Some(reported)) => match ::serde_json::from_str::<Value>(&reported) {
            Ok(document) => *expected == document,
            Err(_) => {
                // Not a document at all: the only comparison left is the text the create
                // would have sent, which is this value's own compact JSON.
                let sent = expected.to_string();
                sent == reported
            }
        },
        _ => expected == actual,
    }
}

/// One JSON scalar as its single textual spelling, or `None` for a composite.
///
/// The one spelling rule the comparison needs, shared with the provisioning side
/// ([`crate::state::scalar_text`]) so the two cannot disagree about how a scalar a
/// template wrote as a number or a boolean reaches an API member.
fn scalar_text(value: &Value) -> Option<String> {
    crate::state::scalar_text(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource whose declaration states `properties` and whose owning service answers for
    /// it under `physical_resource_id`.
    fn stored(resource_type: &str, physical_resource_id: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Probe".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: physical_resource_id.to_string(),
            attributes: ::std::collections::BTreeMap::new(),
            metadata: None,
            properties,
            materialised: true,
            deletion_policy: None,
        }
    }

    /// The property a declaration states the resource's IDENTITY in is not compared when the
    /// actual-state read was addressed BY that identity: the service resolved it to the
    /// resource it reported, so the form it renders the identifier back in is not drift. A
    /// resource the read is addressed by another identifier for has every declared property
    /// compared, the name among them.
    #[test]
    fn the_identity_the_read_was_addressed_by_is_not_compared() {
        for (case, resource, actual, expected_paths) in [
            (
                "a top-level parameter name the service reports without its separator",
                stored(
                    "AWS::SSM::Parameter",
                    "/probe-top-level",
                    serde_json::json!({
                        "Name": "/probe-top-level", "Type": "String", "Value": "declared"
                    }),
                ),
                serde_json::json!({
                    "Name": "probe-top-level", "Type": "String", "Value": "changed"
                }),
                vec!["/Value"],
            ),
            (
                "a hierarchical parameter name the service reports whole",
                stored(
                    "AWS::SSM::Parameter",
                    "/probe/nested",
                    serde_json::json!({
                        "Name": "/probe/nested", "Type": "String", "Value": "declared"
                    }),
                ),
                serde_json::json!({
                    "Name": "/probe/nested", "Type": "String", "Value": "changed"
                }),
                vec!["/Value"],
            ),
            (
                "nothing else of the resource changed",
                stored(
                    "AWS::SSM::Parameter",
                    "/probe-in-sync",
                    serde_json::json!({"Name": "/probe-in-sync", "Value": "declared"}),
                ),
                serde_json::json!({"Name": "probe-in-sync", "Value": "declared"}),
                vec![],
            ),
            (
                "a name the type spells with the resource's own noun",
                stored(
                    "AWS::S3::Bucket",
                    "probe-bucket",
                    serde_json::json!({
                        "BucketName": "probe-bucket",
                        "VersioningConfiguration": {"Status": "Enabled"}
                    }),
                ),
                serde_json::json!({
                    "BucketName": "probe-bucket",
                    "VersioningConfiguration": {"Status": "Suspended"}
                }),
                vec!["/VersioningConfiguration/Status"],
            ),
            (
                "a resource the read is addressed by an identifier the service minted",
                stored(
                    "AWS::Bedrock::KnowledgeBase",
                    "KB0123456789",
                    serde_json::json!({"Name": "declared-name", "Description": "declared"}),
                ),
                serde_json::json!({"Name": "renamed", "Description": "declared"}),
                vec!["/Name"],
            ),
            (
                "a resource whose declaration names it nowhere",
                stored(
                    "AWS::SQS::Queue",
                    "https://queue/probe",
                    serde_json::json!({"VisibilityTimeout": 30}),
                ),
                serde_json::json!({"VisibilityTimeout": "60"}),
                vec!["/VisibilityTimeout"],
            ),
        ] {
            let found = resource_differences(&resource, &actual);
            let paths: Vec<&str> = found
                .iter()
                .map(|difference| difference.property_path.as_str())
                .collect();
            assert_eq!(
                paths, expected_paths,
                "case {case:?}: expected the differences at {expected_paths:?}, got {found:?}"
            );
        }
    }

    /// A declared property the owning service reports with the same value is not drift,
    /// whichever spelling each side carries it in; one it reports differently is.
    #[test]
    fn only_a_declared_property_the_service_reports_differently_is_drift() {
        for (case, expected, actual, drifted) in [
            (
                "same text",
                serde_json::json!({"VisibilityTimeout": "30"}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "number against the text an attribute carries",
                serde_json::json!({"VisibilityTimeout": 30}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "boolean against its text",
                serde_json::json!({"FifoQueue": true}),
                serde_json::json!({"FifoQueue": "true"}),
                false,
            ),
            (
                "changed value",
                serde_json::json!({"VisibilityTimeout": 30}),
                serde_json::json!({"VisibilityTimeout": "60"}),
                true,
            ),
            (
                "property the read does not report",
                serde_json::json!({"DelaySeconds": 5}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "nested document unchanged",
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                false,
            ),
            (
                "nested document changed",
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                serde_json::json!({"VersioningConfiguration": {"Status": "Suspended"}}),
                true,
            ),
            (
                "a declared document the service reports as the JSON text it was sent as",
                serde_json::json!({"RedrivePolicy": {"maxReceiveCount": 3}}),
                serde_json::json!({"RedrivePolicy": "{\"maxReceiveCount\":3}"}),
                false,
            ),
            (
                "the same document the service re-serialised its own way",
                serde_json::json!({"RedrivePolicy": {
                    "deadLetterTargetArn": "arn:aws:sqs:us-east-1:000000000000:dlq",
                    "maxReceiveCount": 3
                }}),
                serde_json::json!({"RedrivePolicy": "{ \"maxReceiveCount\": 3, \
                    \"deadLetterTargetArn\": \"arn:aws:sqs:us-east-1:000000000000:dlq\" }"}),
                false,
            ),
            (
                "a declared document the service reports changed",
                serde_json::json!({"RedrivePolicy": {"maxReceiveCount": 3}}),
                serde_json::json!({"RedrivePolicy": "{\"maxReceiveCount\":9}"}),
                true,
            ),
            (
                "resource that declared no properties",
                Value::Null,
                serde_json::json!({"Status": "ACTIVE"}),
                false,
            ),
        ] {
            let actual_drift = differs(&expected, &actual);
            assert_eq!(
                actual_drift, drifted,
                "case {case:?}: expected drifted={drifted}, got {actual_drift}"
            );
        }
    }

    /// A difference names the fully-qualified path of the value the two sides disagree
    /// about, both values, and which kind of difference it is.
    #[test]
    fn a_difference_names_the_path_both_values_and_its_kind() {
        for (case, expected, actual, expected_differences) in [
            (
                "a scalar the service reports differently",
                serde_json::json!({"VisibilityTimeout": 30}),
                serde_json::json!({"VisibilityTimeout": "60"}),
                vec![("/VisibilityTimeout", "30", "60", difference_type::NOT_EQUAL)],
            ),
            (
                "a member inside a declared document",
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                serde_json::json!({"VersioningConfiguration": {"Status": "Suspended"}}),
                vec![(
                    "/VersioningConfiguration/Status",
                    "Enabled",
                    "Suspended",
                    difference_type::NOT_EQUAL,
                )],
            ),
            (
                "a member the reported document no longer carries",
                serde_json::json!({"RedrivePolicy": {"maxReceiveCount": 3, "queue": "dlq"}}),
                serde_json::json!({"RedrivePolicy": "{\"maxReceiveCount\":3}"}),
                vec![(
                    "/RedrivePolicy/queue",
                    "dlq",
                    "",
                    difference_type::REMOVE,
                )],
            ),
            (
                "an element only the reported list carries",
                serde_json::json!({"Subnets": ["one"]}),
                serde_json::json!({"Subnets": ["one", "two"]}),
                vec![("/Subnets/1", "", "two", difference_type::ADD)],
            ),
            (
                "an element the reported list dropped",
                serde_json::json!({"Subnets": ["one", "two"]}),
                serde_json::json!({"Subnets": ["one"]}),
                vec![("/Subnets/1", "two", "", difference_type::REMOVE)],
            ),
            (
                "an element the reported list changed",
                serde_json::json!({"Subnets": ["one", "two"]}),
                serde_json::json!({"Subnets": ["one", "three"]}),
                vec![("/Subnets/1", "two", "three", difference_type::NOT_EQUAL)],
            ),
            (
                "two properties, one of them changed",
                serde_json::json!({"DelaySeconds": 0, "VisibilityTimeout": 30}),
                serde_json::json!({"DelaySeconds": "0", "VisibilityTimeout": "45"}),
                vec![("/VisibilityTimeout", "30", "45", difference_type::NOT_EQUAL)],
            ),
            (
                "a member only the reported document carries is not a declared property",
                serde_json::json!({"Policy": {"Version": "2012-10-17"}}),
                serde_json::json!({"Policy": {"Version": "2012-10-17", "Id": "x"}}),
                vec![],
            ),
            (
                "nothing differs",
                serde_json::json!({"FifoQueue": true}),
                serde_json::json!({"FifoQueue": "true"}),
                vec![],
            ),
        ] {
            let found = differences(&expected, &actual);
            let rendered: Vec<(&str, &str, &str, &str)> = found
                .iter()
                .map(|difference| {
                    (
                        difference.property_path.as_str(),
                        difference.expected_value.as_str(),
                        difference.actual_value.as_str(),
                        difference.difference_type.as_str(),
                    )
                })
                .collect();
            assert_eq!(
                rendered, expected_differences,
                "case {case:?}: expected {expected_differences:?}, got {rendered:?}"
            );
        }
    }

    /// The descent is BOUNDED: a document nested past [`MAX_COMPARISON_DEPTH`] is compared
    /// whole at that depth rather than walked further, so the walk's cost cannot be chosen
    /// by whoever wrote the template — and the difference is still reported.
    #[test]
    fn the_descent_into_a_document_is_bounded() {
        fn nested(depth: usize, leaf: &str) -> Value {
            let mut value = Value::String(leaf.to_string());
            for _ in 0..depth {
                value = serde_json::json!({"Inner": value});
            }
            serde_json::json!({"Outer": value})
        }
        let depth = MAX_COMPARISON_DEPTH + 8;
        let found = differences(&nested(depth, "declared"), &nested(depth, "reported"));
        assert_eq!(
            found.len(),
            1,
            "expected one difference past the bound, got {found:?}"
        );
        let difference = found.first().expect("the one difference");
        let segments = difference.property_path.matches('/').count();
        assert!(
            segments <= MAX_COMPARISON_DEPTH + 1,
            "expected at most {} path segments, got {segments} in {:?}",
            MAX_COMPARISON_DEPTH + 1,
            difference.property_path
        );
        assert_eq!(
            difference.difference_type,
            difference_type::NOT_EQUAL,
            "expected the bounded comparison to report a changed value, got {difference:?}"
        );
    }

    /// The stack-level status is what the per-resource outcomes add up to.
    #[test]
    fn stack_status_follows_the_resource_outcomes() {
        for (case, statuses, expected_status, expected_count) in [
            (
                "nothing checked",
                vec![resource_drift_status::NOT_CHECKED],
                DRIFT_STATUS_IN_SYNC,
                0,
            ),
            (
                "all in sync",
                vec![resource_drift_status::IN_SYNC, resource_drift_status::IN_SYNC],
                DRIFT_STATUS_IN_SYNC,
                0,
            ),
            (
                "one modified",
                vec![resource_drift_status::IN_SYNC, resource_drift_status::MODIFIED],
                DRIFT_STATUS_DRIFTED,
                1,
            ),
            (
                "one deleted and one modified",
                vec![
                    resource_drift_status::DELETED,
                    resource_drift_status::MODIFIED,
                ],
                DRIFT_STATUS_DRIFTED,
                2,
            ),
            (
                "a read that could not run",
                vec![resource_drift_status::UNKNOWN, resource_drift_status::IN_SYNC],
                DRIFT_STATUS_UNKNOWN,
                0,
            ),
            (
                "a drifted resource outweighs one that could not be read",
                vec![
                    resource_drift_status::UNKNOWN,
                    resource_drift_status::MODIFIED,
                ],
                DRIFT_STATUS_DRIFTED,
                1,
            ),
        ] {
            let detected = Detected {
                results: statuses
                    .iter()
                    .enumerate()
                    .map(|(index, status)| ResourceDrift {
                        logical_id: format!("Resource{index}"),
                        status: (*status).to_string(),
                        detail: None,
                    })
                    .collect(),
                failure: None,
            };
            let status = detected.stack_drift_status();
            let count = detected.drifted_count();
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status}"
            );
            assert_eq!(
                count, expected_count,
                "case {case:?}: expected {expected_count} drifted, got {count}"
            );
        }
    }
}
