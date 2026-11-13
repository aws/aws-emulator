// SPDX-License-Identifier: CC0-1.0

//! Bringing resources that ALREADY EXIST under a stack's management.
//!
//! ## The rule this module implements
//!
//! Two members of `CreateChangeSet` ask for a resource to be ADOPTED rather than created:
//! `ChangeSetType=IMPORT`, whose `ResourcesToImport` names each resource and the identifier
//! it already exists under, and `ImportExistingResources`, which asks CloudFormation to
//! adopt a resource the plan would otherwise CREATE whenever one already exists under the
//! name the template states ("CloudFormation imports it into the stack instead of creating
//! it"). Either way the resource is reported with the `Import` action and the physical id it
//! already carries ([`import_change`], [`adopt_existing`]), and executing the change set
//! records it as a resource the stack now manages WITHOUT creating anything
//! ([`adopted_resources`]) — nothing is asked of the owning service, because the resource
//! it would be asked for is already there.
//!
//! A name is adoptable only while NO stack manages it: a resource another stack owns is that
//! stack's, so adopting it would take over a resource CloudFormation is already managing
//! elsewhere, and the change set fails instead ([`super::physical_name`]).
//!
//! An adopted resource keeps the IDENTITY it already has: the identifier the plan reports is
//! the resource's physical id and the name every value the record reports is read off
//! ([`adopt_identifier`]), so a resource whose declaration names it nowhere is adopted under
//! the identifier it exists with rather than under the name CloudFormation would have
//! generated for a resource it was never asked to create.
//!
//! An IMPORT also holds the template to the one rule CloudFormation states for it: every
//! imported resource must carry a `DeletionPolicy`, so that what happens to a resource the
//! stack did not create is stated before the stack takes it over
//! ([`validate_deletion_policies`]).

use ::std::collections::BTreeMap;

use super::change_plan::StoredResourceChange;
use super::early_validation::NamedResource;
use super::template::Template;
use super::{change_action, StoredResource};

/// The resource attribute CloudFormation requires on every imported resource:
/// "each resource to import must have a `DeletionPolicy` attribute in your template".
const DELETION_POLICY: &str = "DeletionPolicy";

/// CloudFormation's refusal for an IMPORT whose template gives an imported resource no
/// `DeletionPolicy`.
///
/// The fixed text is CloudFormation's own; the variable part is the list of logical ids
/// that lack the attribute, in the order `ResourcesToImport` names them.
pub fn deletion_policy_required(logical_ids: &[String]) -> String {
    format!(
        "The following resources to import [{}] must have DeletionPolicy attribute \
         specified in the template.",
        logical_ids.join(", ")
    )
}

/// CloudFormation's refusal for an IMPORT that names no resource to import at all: there is
/// nothing for the change set to adopt, so it is no plan.
///
/// This service's own wording for the condition, in the form its other input refusals take
/// (beside `ResourcesToImport can only be specified when ChangeSetType is IMPORT`, which
/// `CreateChangeSet` already answers the converse request with): there is no recorded
/// sentence for it, and its one job is to name the member the request is wrong about.
pub const RESOURCES_TO_IMPORT_REQUIRED: &str =
    "ResourcesToImport must be specified when ChangeSetType is IMPORT";

/// The refusal for an entry naming a logical id the template does not declare: the
/// identifier would be adopted as a resource nothing describes.
///
/// This service's own wording, as [`RESOURCES_TO_IMPORT_REQUIRED`] is, with the logical id
/// the request named as its one variable part.
pub fn not_in_the_template(logical_resource_id: &str) -> String {
    format!(
        "Resource with logical id {logical_resource_id} is not in the template and cannot be \
         imported"
    )
}

/// `Err(message)` unless `template` declares every resource `resources` names, each with a
/// `DeletionPolicy`.
///
/// The two conditions are reported in the order CloudFormation establishes them: a logical id
/// the template does not declare at all cannot be checked for the attribute.
pub fn validate_deletion_policies(
    template: &Template,
    resources: &[crate::ops::types::ResourceToImport],
) -> Result<(), String> {
    let mut without_the_policy = Vec::new();
    for imported in resources {
        let declared = template
            .resources
            .iter()
            .find(|declared| declared.logical_id == imported.logical_resource_id)
            .ok_or_else(|| not_in_the_template(&imported.logical_resource_id))?;
        if declared.declaration.get(DELETION_POLICY).is_none() {
            without_the_policy.push(imported.logical_resource_id.clone());
        }
    }
    if !without_the_policy.is_empty() {
        return Err(deletion_policy_required(&without_the_policy));
    }
    Ok(())
}

/// The plan an IMPORT change set carries: one `Import` change per resource the request names,
/// in that order, each reporting the physical id the resource already exists under.
///
/// The identifier reported is the one the NAMING pipeline computes for the declaration
/// (`named`, from [`super::early_validation::named_resources`]) — the form the owning service
/// identifies the resource by, which is what every other change reports — and the value the
/// request's own `ResourceIdentifier` carries where the declaration states its name nowhere.
pub fn import_changes(
    resources: &[crate::ops::types::ResourceToImport],
    named: &[NamedResource],
) -> Vec<StoredResourceChange> {
    resources
        .iter()
        .map(|imported| {
            let physical_resource_id = named
                .iter()
                .find(|candidate| candidate.logical_id == imported.logical_resource_id)
                .map(|candidate| candidate.physical_resource_id.clone())
                .or_else(|| {
                    // A declaration that states its name nowhere is identified by what the
                    // REQUEST carries, read in key order so one request reads one way.
                    let mut identifier: Vec<(&String, &String)> =
                        imported.resource_identifier.iter().collect();
                    identifier.sort();
                    identifier
                        .first()
                        .map(|(_, value)| (*value).clone())
                });
            import_change(
                &imported.logical_resource_id,
                &imported.resource_type,
                physical_resource_id,
            )
        })
        .collect()
}

/// The resources an IMPORT names, as the candidates the existence check asks the owning
/// services about: the type and the identifier each `Import` change reports.
///
/// A resource to import must BE there — the plan reports the stack as managing a resource
/// that already exists — so the identifier the request named is read back from the service
/// that owns the type, exactly as a name the TEMPLATE states is
/// ([`super::early_validation::taken_names`]), rather than inferred from anything.
pub fn import_candidates(changes: &[StoredResourceChange]) -> Vec<NamedResource> {
    changes
        .iter()
        .filter(|change| change.action == change_action::IMPORT)
        .filter_map(|change| {
            Some(NamedResource {
                logical_id: change.logical_resource_id.clone(),
                resource_type: change.resource_type.clone(),
                physical_resource_id: change.physical_resource_id.clone()?,
            })
        })
        .collect()
}

/// One `Import` change: the resource the change set adopts, under the identifier it already/// exists with.
///
/// `ResourceChange$PhysicalResourceId` is "the resource's physical ID (resource name)", which
/// an adopted resource HAS — unlike an addition, which has none because it does not exist
/// yet. No `Replacement`, `Scope` or `Details` comes with it: nothing about the resource is
/// changed by adopting it.
pub fn import_change(
    logical_resource_id: &str,
    resource_type: &str,
    physical_resource_id: Option<String>,
) -> StoredResourceChange {
    StoredResourceChange {
        action: change_action::IMPORT.to_string(),
        logical_resource_id: logical_resource_id.to_string(),
        resource_type: resource_type.to_string(),
        physical_resource_id,
        scope: Vec::new(),
        replacement: None,
        details: Vec::new(),
        policy_action: None,
        resource_drift_status: None,
    }
}

/// Reclassify as `Import` every ADDITION of `changes` whose resource already exists under the
/// name the template states (`taken`, from [`super::early_validation::taken_names`]).
///
/// This is what `ImportExistingResources` asks for: "if you have resources that exist outside
/// of CloudFormation, you can import them into your stack as part of the change set" — the
/// plan is otherwise unchanged, so a resource nothing holds a name for is still created.
pub fn adopt_existing(changes: &mut [StoredResourceChange], taken: &[NamedResource]) {
    for change in changes
        .iter_mut()
        .filter(|change| change.action == change_action::ADD)
    {
        let Some(existing) = taken
            .iter()
            .find(|candidate| candidate.logical_id == change.logical_resource_id)
        else {
            continue;
        };
        change.action = change_action::IMPORT.to_string();
        change.physical_resource_id = Some(existing.physical_resource_id.clone());
    }
}

/// The resources an execution ADOPTS, as records of resources the stack already has: one per
/// `Import` change, carrying the declaration's own configuration and the identifier the
/// change reports, marked as existing in the service that owns it.
///
/// This is what an import IS: from the moment the change set is executed the stack MANAGES a
/// resource that already exists. The deployment therefore reconciles against a resource the
/// stack HAS ([`super::provisioning::reconcile`]) rather than one it must create — so nothing
/// is asked of the owning service about it, and importing a resource does not modify it — and
/// the stack goes on recording it, so its own deletion disposes of it under the
/// `DeletionPolicy` the template states.
pub fn adopted_resources(
    changes: &[StoredResourceChange],
    declared: &[StoredResource],
) -> Vec<StoredResource> {
    changes
        .iter()
        .filter(|change| change.action == change_action::IMPORT)
        .filter_map(|change| {
            let declaration = declared
                .iter()
                .find(|candidate| candidate.logical_id == change.logical_resource_id)?;
            Some(StoredResource {
                physical_resource_id: change
                    .physical_resource_id
                    .clone()
                    .unwrap_or_else(|| declaration.physical_resource_id.clone()),
                materialised: true,
                ..declaration.clone()
            })
        })
        .collect()
}

/// The identifier each resource a plan ADOPTS already exists under, keyed by logical id: one
/// entry per `Import` change that reports one.
///
/// What the EXECUTION of that plan has to take over ([`adopt_identifier`]): the resources are
/// adopted rather than created, so the identity in force for each of them is the one it
/// already has rather than the one the naming rules would compute for a resource
/// CloudFormation was never asked to create.
pub fn adopted_identifiers(changes: &[StoredResourceChange]) -> BTreeMap<String, String> {
    changes
        .iter()
        .filter(|change| change.action == change_action::IMPORT)
        .filter_map(|change| {
            Some((
                change.logical_resource_id.clone(),
                change.physical_resource_id.clone()?,
            ))
        })
        .collect()
}

/// The suffix of the attributes that report a resource's own NAME — the `<Type>Name` its type
/// publishes as a return value and the uniform `Name` the naming convention gives every
/// resource.
const NAME_ATTRIBUTE: &str = "Name";

/// Record `resource` as the resource that already EXISTS under `identifier`, answering
/// whether that changed the identity in force.
///
/// Adopting a resource takes over the identity it already HAS: the identifier the plan
/// reports becomes the resource's physical id ([`super::template::adopt_identity`]), so
/// `Ref`, the stack's outputs and the deployment that follows all address the resource that is
/// really there rather than the name CloudFormation would have generated for a resource it was
/// never asked to create.
///
/// A type's published return values are functions of the resource's NAME
/// ([`super::template::provision`]), so the name is what the computed identifier and the
/// adopted one differ in: the values the record read off the computed name are read off the
/// adopted resource's name instead ([`renamed_to`]), which is what keeps the ARN and the name
/// the stack reports for the resource addressing that same resource. An identifier of another
/// form than the one the type computes says nothing about what the resource is called, so
/// those values are left as the declaration computed them and only the identity is adopted.
pub fn adopt_identifier(resource: &mut StoredResource, identifier: &str) -> bool {
    if resource.physical_resource_id == identifier {
        return false;
    }
    if let Some((computed, adopted)) = renamed_to(resource, identifier) {
        for value in resource.attributes.values_mut() {
            if let Some(renamed) = renamed(value, &computed, &adopted) {
                *value = renamed;
            }
        }
    }
    super::template::adopt_identity(resource, identifier.to_string());
    true
}

/// `value` with the name `computed` replaced by `adopted`, or `None` where it carries that
/// name nowhere.
///
/// The LAST occurrence is the one replaced, for the reason [`renamed_to`] reads the last one:
/// a value these conventions build from a resource's name carries it as the value's TAIL (the
/// resource segment of an ARN, the last path segment of a url, the name itself), while the
/// text before it may contain the same string — the service namespace of the ARN of a queue
/// called `sqs`, say — and that text names the service rather than the resource.
fn renamed(value: &str, computed: &str, adopted: &str) -> Option<String> {
    let at = value.rfind(computed)?;
    Some(format!(
        "{}{adopted}{}",
        &value[..at],
        &value[at + computed.len()..]
    ))
}

/// `(the name the record's values were computed from, the name `identifier` carries in its
/// place)`, or `None` where the record reports no name the computed identifier is built from,
/// or where `identifier` is not that same function of a name.
///
/// The two identifiers are built from their names the same way, so the text around the name is
/// the same in both: that is what reads the adopted resource's name off the identifier without
/// a second idea of how any type's identifier is spelled.
///
/// The LAST occurrence of the name is the one read: a name may also occur in the fixed text an
/// identifier is built from (a queue called `sqs`, whose url is
/// `https://sqs.<region>.amazonaws.com/<account>/sqs`), and the name an identifier of these
/// forms carries is its trailing segment.
fn renamed_to(resource: &StoredResource, identifier: &str) -> Option<(String, String)> {
    let computed = resource.physical_resource_id.clone();
    let name = resource
        .attributes
        .iter()
        .filter(|(attribute, _)| attribute.ends_with(NAME_ATTRIBUTE))
        .map(|(_, value)| value.clone())
        .find(|value| !value.is_empty() && computed.contains(value.as_str()))?;
    let at = computed.rfind(&name)?;
    let adopted = identifier
        .strip_prefix(&computed[..at])?
        .strip_suffix(&computed[at + name.len()..])?;
    (!adopted.is_empty()).then(|| (name, adopted.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_of(body: &str) -> Template {
        super::super::declared_template(body).expect("a well-formed template")
    }

    fn imported(logical_id: &str, name: &str) -> crate::ops::types::ResourceToImport {
        crate::ops::types::ResourceToImport {
            resource_type: "AWS::SSM::Parameter".to_string(),
            logical_resource_id: logical_id.to_string(),
            resource_identifier: [("Name".to_string(), name.to_string())]
                .into_iter()
                .collect(),
        }
    }

    /// RULE: an IMPORT is refused unless every resource it names is declared with a
    /// `DeletionPolicy`, and the refusal lists the logical ids that lack one.
    #[test]
    fn an_import_needs_a_deletion_policy_on_every_imported_resource() {
        let body = r#"{"Resources":{
            "Kept":{"Type":"AWS::SSM::Parameter","DeletionPolicy":"Retain",
                    "Properties":{"Name":"/kept","Type":"String","Value":"1"}},
            "Also":{"Type":"AWS::SSM::Parameter","DeletionPolicy":"Delete",
                    "Properties":{"Name":"/also","Type":"String","Value":"1"}},
            "Bare":{"Type":"AWS::SSM::Parameter",
                    "Properties":{"Name":"/bare","Type":"String","Value":"1"}},
            "Other":{"Type":"AWS::SSM::Parameter",
                     "Properties":{"Name":"/other","Type":"String","Value":"1"}}}}"#;
        let template = template_of(body);
        type Row = (
            &'static str,
            Vec<crate::ops::types::ResourceToImport>,
            Option<&'static str>,
        );
        let rows: Vec<Row> = vec![
            ("one resource that carries Retain", vec![imported("Kept", "/kept")], None),
            (
                "two of them, one carrying Delete",
                vec![imported("Kept", "/kept"), imported("Also", "/also")],
                None,
            ),
            (
                "one resource that carries none",
                vec![imported("Bare", "/bare")],
                Some(
                    "The following resources to import [Bare] must have DeletionPolicy \
                     attribute specified in the template.",
                ),
            ),
            (
                "two that carry none, in the order the request names them",
                vec![imported("Other", "/other"), imported("Bare", "/bare")],
                Some(
                    "The following resources to import [Other, Bare] must have \
                     DeletionPolicy attribute specified in the template.",
                ),
            ),
            (
                "a logical id the template does not declare",
                vec![imported("Absent", "/absent")],
                Some(
                    "Resource with logical id Absent is not in the template and cannot be \
                     imported",
                ),
            ),
        ];
        for (case, resources, expected) in &rows {
            let refusal = validate_deletion_policies(&template, resources).err();
            assert_eq!(
                refusal.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }

    /// RULE: an addition whose name already exists is reclassified as an `Import` carrying
    /// that name; every other change is left exactly as the comparison computed it.
    #[test]
    fn an_existing_name_turns_an_addition_into_an_import() {
        let change = |action: &str, logical_id: &str| StoredResourceChange {
            action: action.to_string(),
            logical_resource_id: logical_id.to_string(),
            resource_type: "AWS::SSM::Parameter".to_string(),
            physical_resource_id: None,
            scope: Vec::new(),
            replacement: None,
            details: Vec::new(),
            policy_action: None,
            resource_drift_status: None,
        };
        let taken = vec![NamedResource {
            logical_id: "Adopted".to_string(),
            resource_type: "AWS::SSM::Parameter".to_string(),
            physical_resource_id: "/held".to_string(),
        }];
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "an addition of a name that exists",
                change_action::ADD,
                "Adopted",
                Some("/held"),
            ),
            ("an addition of a name nothing holds", change_action::ADD, "Fresh", None),
            ("a modification of the adopted resource", change_action::MODIFY, "Adopted", None),
        ];
        for (case, action, logical_id, adopted) in rows {
            let mut changes = vec![change(action, logical_id)];
            adopt_existing(&mut changes, &taken);
            let expected_action = if adopted.is_some() {
                change_action::IMPORT
            } else {
                *action
            };
            assert_eq!(
                (changes[0].action.as_str(), changes[0].physical_resource_id.as_deref()),
                (expected_action, *adopted),
                "case {case:?}: expected {expected_action:?}/{adopted:?}, got {:?}",
                changes[0]
            );
        }
    }

    /// RULE: a resource is adopted under the identifier it ALREADY exists under — that
    /// identifier is its physical id, and every value the record read off the name
    /// CloudFormation computed is read off the adopted resource's name instead.
    #[test]
    fn an_adopted_resource_takes_over_the_identity_it_already_has() {
        /// `(case, type, the computed identifier, the attributes computed with it, the
        /// identifier the resource is adopted under, the identity in force afterwards, the
        /// attributes in force afterwards)`.
        type Row = (
            &'static str,
            &'static str,
            &'static str,
            Vec<(&'static str, &'static str)>,
            &'static str,
            bool,
            Vec<(&'static str, &'static str)>,
        );
        let rows: Vec<Row> = vec![
            (
                // The published return values of this type: "Ref returns the queue URL";
                // Fn::GetAtt Arn, QueueName, QueueUrl.
                "a queue url, whose name is the last segment",
                "AWS::SQS::Queue",
                "https://sqs.us-east-1.amazonaws.com/000000000000/stack-Queue-ABC",
                vec![
                    ("Arn", "arn:aws:sqs:us-east-1:000000000000:stack-Queue-ABC"),
                    ("QueueName", "stack-Queue-ABC"),
                    (
                        "QueueUrl",
                        "https://sqs.us-east-1.amazonaws.com/000000000000/stack-Queue-ABC",
                    ),
                ],
                "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                true,
                vec![
                    ("Arn", "arn:aws:sqs:us-east-1:000000000000:adopted"),
                    ("QueueName", "adopted"),
                    (
                        "QueueUrl",
                        "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                    ),
                ],
            ),
            (
                // "Ref returns the topic ARN"; Fn::GetAtt TopicArn, TopicName.
                "a topic ARN, whose name is its last ARN segment",
                "AWS::SNS::Topic",
                "arn:aws:sns:us-east-1:000000000000:stack-Topic-ABC",
                vec![
                    ("TopicArn", "arn:aws:sns:us-east-1:000000000000:stack-Topic-ABC"),
                    ("TopicName", "stack-Topic-ABC"),
                ],
                "arn:aws:sns:us-east-1:000000000000:adopted",
                true,
                vec![
                    ("TopicArn", "arn:aws:sns:us-east-1:000000000000:adopted"),
                    ("TopicName", "adopted"),
                ],
            ),
            (
                "an identifier that IS the resource's name",
                "AWS::SSM::Parameter",
                "/computed",
                vec![("Name", "/computed")],
                "/adopted",
                true,
                vec![("Name", "/adopted")],
            ),
            (
                "the identifier the record already carries",
                "AWS::SQS::Queue",
                "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                vec![("QueueName", "adopted")],
                "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                false,
                vec![("QueueName", "adopted")],
            ),
            (
                "an identifier of another form than the one the type computes",
                "AWS::SQS::Queue",
                "https://sqs.us-east-1.amazonaws.com/000000000000/stack-Queue-ABC",
                vec![("QueueName", "stack-Queue-ABC")],
                "a-handle-of-another-form",
                true,
                // Nothing says what that resource is called, so the name-derived value is
                // left exactly as the declaration computed it.
                vec![("QueueName", "stack-Queue-ABC")],
            ),
            (
                // The name occurs in the fixed text of the url too, so the TRAILING segment
                // is the one the identifier carries.
                "a name that also occurs in the identifier's fixed text",
                "AWS::SQS::Queue",
                "https://sqs.us-east-1.amazonaws.com/000000000000/sqs",
                vec![
                    ("Arn", "arn:aws:sqs:us-east-1:000000000000:sqs"),
                    ("QueueName", "sqs"),
                    (
                        "QueueUrl",
                        "https://sqs.us-east-1.amazonaws.com/000000000000/sqs",
                    ),
                ],
                "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                true,
                vec![
                    ("Arn", "arn:aws:sqs:us-east-1:000000000000:adopted"),
                    ("QueueName", "adopted"),
                    (
                        "QueueUrl",
                        "https://sqs.us-east-1.amazonaws.com/000000000000/adopted",
                    ),
                ],
            ),
        ];
        for (case, resource_type, computed, attributes, identifier, changes, expected) in &rows {
            let mut resource = StoredResource {
                logical_id: "Adopted".to_string(),
                resource_type: (*resource_type).to_string(),
                physical_resource_id: (*computed).to_string(),
                attributes: attributes
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                metadata: None,
                properties: ::serde_json::Value::Null,
                materialised: true,
                deletion_policy: None,
                update_replace_policy: None,
            };
            let adopted = adopt_identifier(&mut resource, identifier);
            let reported: Vec<(String, String)> = expected
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            let in_force: Vec<(String, String)> = reported
                .iter()
                .map(|(name, _)| {
                    (
                        name.clone(),
                        resource.attributes.get(name).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                (adopted, resource.physical_resource_id.as_str(), in_force),
                (*changes, *identifier, reported.clone()),
                "case {case:?}: expected {changes:?}/{identifier:?}/{reported:?}, got \
                 {resource:?}"
            );
        }
    }

    /// RULE: the identifiers an execution has to take over are those of the plan's `Import`
    /// changes, keyed by the logical id each one names.
    #[test]
    fn the_adopted_identifiers_are_the_plans_imports() {
        let change = |action: &str, logical_id: &str, physical: Option<&str>| {
            StoredResourceChange {
                action: action.to_string(),
                logical_resource_id: logical_id.to_string(),
                resource_type: "AWS::SQS::Queue".to_string(),
                physical_resource_id: physical.map(str::to_string),
                scope: Vec::new(),
                replacement: None,
                details: Vec::new(),
                policy_action: None,
                resource_drift_status: None,
            }
        };
        /// `(case, the plan's changes, the identifiers its imports report)`.
        type PlanRow = (&'static str, Vec<StoredResourceChange>, Vec<(&'static str, &'static str)>);
        let rows: Vec<PlanRow> = vec![
            (
                "two imports of one plan",
                vec![
                    change(change_action::IMPORT, "One", Some("one")),
                    change(change_action::IMPORT, "Two", Some("two")),
                ],
                vec![("One", "one"), ("Two", "two")],
            ),
            (
                "an addition and a modification beside an import",
                vec![
                    change(change_action::ADD, "Added", None),
                    change(change_action::IMPORT, "One", Some("one")),
                    change(change_action::MODIFY, "Changed", Some("changed")),
                ],
                vec![("One", "one")],
            ),
            (
                "an import whose identifier the plan does not report",
                vec![change(change_action::IMPORT, "One", None)],
                Vec::new(),
            ),
            ("a plan that adopts nothing", Vec::new(), Vec::new()),
        ];
        for (case, changes, expected) in &rows {
            let adopted = adopted_identifiers(changes);
            let reported: BTreeMap<String, String> = expected
                .iter()
                .map(|(logical_id, identifier)| {
                    ((*logical_id).to_string(), (*identifier).to_string())
                })
                .collect();
            assert_eq!(
                adopted, reported,
                "case {case:?}: expected {reported:?}, got {adopted:?}"
            );
        }
    }
}
