// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a stack operation is held to before anything is
//! provisioned — CloudFormation's "early validation".
//!
//! ## The rule this module implements
//!
//! Every operation that deploys a template runs it, and a change set is the caller that runs
//! it EARLIEST: a change set is a plan a caller reviews and then executes, and CloudFormation
//! validates that plan WHILE CREATING IT rather than leaving every problem to the
//! deployment: a validation the plan does not pass fails the change set itself, which settles
//! `FAILED` with `ExecutionStatus` `UNAVAILABLE` and a `StatusReason` naming the validations
//! that failed ([`failed_reason`]) — so nothing is offered for execution that CloudFormation
//! already knows cannot be deployed. `CreateStack` and `UpdateStack` run the same stage over
//! the resources they would create ([`named_additions`]), before anything is asked of any
//! owning service. `DisableValidation` is what turns that stage off, and an operation that
//! states it carries the template unvalidated, leaving the same problem to surface at
//! deployment time.
//!
//! The validation this module performs is the EXISTENCE check
//! ([`RESOURCE_EXISTENCE_CHECK`]): a resource the plan would CREATE under a physical name the
//! template SPELLS cannot be created if the service that owns it already holds a resource
//! under that name — the create would be refused — so the plan is not a plan that can be
//! deployed.
//!
//! ## What is asked, and of whom
//!
//! Only a plan's ADDITIONS fail the validation, and only those whose name the template
//! states: a resource whose name CloudFormation generates is unique by construction, and one
//! the plan MODIFIES or REMOVES already exists because the stack has it. The question is put
//! to the service that OWNS the type, through the same in-process read drift detection asks
//! ([`super::provisioning::Owner::actual_configuration`]), so a name is reported taken only
//! when that service really holds a resource under it; a type with no owning provisioner
//! here, and a service whose read is not served, report nothing and so fail nothing.
//!
//! Nor is a name a live stack of the account and Region MANAGES this stage's to fail:
//! CloudFormation keeps its own record of which stack holds which name, and a deployment that
//! would take one fails ON THE RESOURCE, naming that stack ([`super::physical_name`]). What
//! this stage establishes is a name held OUTSIDE CloudFormation — an object a client created,
//! one another service made of its own accord — which no record of CloudFormation's own could
//! report ([`named_additions`]).
//!
//! The names are read off the submitted template ([`named_resources`]) and asked about with
//! the store unlocked ([`taken_names`]); which of them the change set would really CREATE is
//! the plan's to say ([`failing_addition`]), so the two halves are separate and neither holds
//! this service's store while the other runs. Nothing here reads or writes that store: the
//! handler computes the plan, asks this module for the verdict, and records the status
//! itself.

use ::std::collections::BTreeMap;

use ::serde_json::Value;

use super::change_action;
use super::change_plan::StoredResourceChange;
use super::provisioning::{owner_for, ActualConfiguration};
use super::template::{self, Template};
use super::StoredResource;

/// The validation CloudFormation names for the check that a resource the plan would create
/// does not already exist, as its own `StatusReason` spells it.
pub const RESOURCE_EXISTENCE_CHECK: &str = "AWS::EarlyValidation::ResourceExistenceCheck";

/// The NAME of the validation an event about a resource whose identifier is already taken
/// carries (`OperationEvent$ValidationName`) — the detail `DescribeEvents` reports for a
/// failed existence check, which is where that `StatusReason` sends a client for "detailed
/// failure information".
///
/// It is the validation's own name, not a resource status: nothing was done to the resource,
/// since the plan was never executed.
pub const NAME_CONFLICT_VALIDATION: &str = "NAME_CONFLICT_VALIDATION";

/// `OperationEvent$ValidationStatus` for a validation that FAILED, from the model's
/// `ValidationStatus` enum (`[FAILED, SKIPPED]`).
pub const VALIDATION_FAILED: &str = "FAILED";

/// The path a validation about ONE RESOURCE of a submitted template is applied at: the
/// `Resources` entry its logical id names, which is where a resource is declared in every
/// template document ("Resources: `"Logical ID": { "Type": … }`").
///
/// `OperationEvent$ValidationPath` is "the path within the resource where the validation was
/// applied", so it is stated as a path into the TEMPLATE rather than taken from the request:
/// an event about any resource of any submitted template names the element that resource was
/// declared at. The existence check is about the resource as a whole and stops here; the
/// property validation beside it is about the resource's own property document and steps one
/// further ([`super::property_validation::properties_path`]).
pub fn resource_path(logical_resource_id: &str) -> String {
    format!("/{RESOURCES_SECTION}/{logical_resource_id}")
}

/// The template section a resource is declared in, which the path above steps through.
const RESOURCES_SECTION: &str = "Resources";

/// What that event reports about the resource: which identifier of which type is already
/// there — CloudFormation's own sentence for a resource that cannot be created because it
/// exists, whose variable parts are the declared type and the identifier the declaration
/// asked for.
///
/// The same sentence is what a resource handler answers at EXECUTION time for the same
/// collision (`super::provisioner`'s own `AlreadyExists` wording): the two want to be one
/// helper, which the file-length rule keeps out of reach for now — collapse them here when
/// that file is split.
pub fn already_exists(resource_type: &str, identifier: &str) -> String {
    format!("Resource of type '{resource_type}' with identifier '{identifier}' already exists.")
}

/// The validation RESULT a failed existence check records about `conflicting`: which
/// validation failed, that it FAILED, what is wrong with the resource and where the
/// validation was applied.
///
/// The one place that shape is stated, for every operation the stage runs on
/// (`CreateChangeSet`, `CreateStack`, `UpdateStack`): the result a client reads through
/// `DescribeEvents` is the same whichever call put the question.
pub fn name_conflict_failure(conflicting: &NamedResource) -> super::StoredValidationFailure {
    super::StoredValidationFailure {
        name: NAME_CONFLICT_VALIDATION.to_string(),
        status: VALIDATION_FAILED.to_string(),
        reason: already_exists(
            &conflicting.resource_type,
            &conflicting.physical_resource_id,
        ),
        path: resource_path(&conflicting.logical_id),
    }
}

/// The converse sentence, for a resource that is NOT there: what a resource handler reports
/// for an identifier the service that owns the type does not hold, whose variable parts are
/// the declared type and the identifier that was looked for.
///
/// Read by the IMPORT change sets `CreateChangeSet` plans
/// ([`super::resource_import`]): a resource to import that the owning service does not hold
/// is no resource to adopt, so the plan cannot be carried out.
pub fn not_found(resource_type: &str, identifier: &str) -> String {
    format!("Resource of type '{resource_type}' with identifier '{identifier}' was not found.")
}

/// Whether a resource of `resource_type` can be READ BACK from the service that owns it at
/// all — the question [`taken_names`] puts, answered without putting it.
///
/// A type with no owning provisioner here is one nothing can be established about: it is
/// neither reported as taken nor as missing, because no read was served either way.
pub fn read_back(resource_type: &str) -> bool {
    owner_for(resource_type).is_some()
}

/// The `StatusReason` a change set early validation failed carries: which validations failed,
/// and where the detail of the failure is reported.
///
/// The sentence is CloudFormation's own and the variable part is the list of validations, in
/// the order they were evaluated.
pub fn failed_reason(validations: &[&str]) -> String {
    format!(
        "The following hook(s)/validation failed: [{}]. To troubleshoot Early Validation \
         errors, use the DescribeEvents API for detailed failure information.",
        validations.join(", ")
    )
}

/// One resource whose physical name the template SPELLS: the resource the existence check is
/// about, with the identifier its owning service would know it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedResource {
    pub logical_id: String,
    pub resource_type: String,
    /// The physical resource id the declaration gives it: the name the template states, in
    /// the form the owning service identifies the resource by (which for some types is an
    /// ARN built from that name).
    pub physical_resource_id: String,
}

/// The resources `template` declares whose physical name it STATES, in the order the
/// template reader walked the `Resources` section, each carrying the identifier its create
/// would produce.
///
/// The identifiers are computed by the ONE naming pipeline a deployment uses
/// ([`template::provision`]), so the resource this check asks about is the resource the
/// execution would create, down to the id form its owning service uses. `stack` and
/// `parameters` are that evaluation's scope, since a stated name may be built from a
/// parameter or a pseudo parameter.
///
/// A declaration that names the resource NOWHERE is not among them — CloudFormation
/// generates that name, and a generated name carries a unique suffix, so nothing can already
/// hold it.
pub fn named_resources(
    template: &Template,
    stack: template::StackScope<'_>,
    parameters: &BTreeMap<String, String>,
) -> Vec<NamedResource> {
    let no_resources = BTreeMap::new();
    let no_imports = BTreeMap::new();
    // A stated name is built from the template, its parameters and the stack's own identity;
    // a cross-stack read is no part of that, so neither table carries anything here.
    let no_stack_outputs = super::stack_outputs::Values::new();
    let parameter_types = super::ssm_parameters::parameter_types_in_force(template);
    let scope = template::Scope {
        stack,
        parameters,
        parameter_types: &parameter_types,
        resources: &no_resources,
        mappings: &template.mappings,
        conditions: &template.conditions,
        imports: &no_imports,
        stack_outputs: &no_stack_outputs,
    };
    // ONE budget for the whole template, as a deployment evaluates a whole template out of
    // one: the work this check can be asked for is bounded.
    let mut budget = template::Budget::new();
    template::provision(template, &scope, &mut budget)
        .into_iter()
        .filter(|provisioned| states_its_name(template, provisioned))
        .map(|provisioned| NamedResource {
            logical_id: provisioned.logical_id,
            resource_type: provisioned.resource_type,
            physical_resource_id: provisioned.physical_resource_id,
        })
        .collect()
}

/// Whether the declaration this resource came from STATES its name, through the one reading
/// of a declaration's name property this crate holds ([`template::name_property`]).
fn states_its_name(template: &Template, provisioned: &StoredResource) -> bool {
    template
        .resources
        .iter()
        .find(|declared| declared.logical_id == provisioned.logical_id)
        .is_some_and(|declared| {
            template::name_property(&declared.resource_type, &declared.properties).is_some()
        })
}

/// The resources a DEPLOYMENT would CREATE whose physical name its template STATES: what
/// the existence check asks the owning services about for a stack operation
/// (`CreateStack`, `UpdateStack`) that deploys `stage_body`, as [`named_resources`] is for
/// the plan a change set carries.
///
/// `stage_body` is the PROCESSED stage of the template the operation deploys — the document
/// CloudFormation really provisions from — and `None` (a stack with no template, or a stage
/// this service could not compute) has no declaration to ask about.
///
/// `already_managed` are the logical ids the stack manages BEFORE this deployment: such a
/// resource is not one the deployment would create — it exists because the stack has it — so
/// the name it holds is no collision. A create manages none of them yet and passes none. The
/// exclusion is by LOGICAL ID, which is what CloudFormation identifies a declaration by: a
/// declaration the stack already manages that this deployment RENAMES is a replacement, and
/// the create half of that replacement is left to the owning service to refuse — this stage
/// asks about the names the operation ADDS.
///
/// `managed_names` is CloudFormation's own record of which name each live stack of the
/// account and Region holds ([`super::physical_name`]). A candidate whose name is in it is
/// NOT one this stage is about: CloudFormation knows which stack manages that name, and a
/// create that would take it fails ON THE RESOURCE, naming that stack. What this stage
/// establishes is a name held OUTSIDE CloudFormation — an object a client created, one
/// another service made of its own accord — which no record of CloudFormation's own reports.
pub fn named_additions(
    stage_body: Option<&str>,
    stack: template::StackScope<'_>,
    parameters: &BTreeMap<String, String>,
    already_managed: &[String],
    managed_names: &[super::physical_name::ManagedName],
) -> Vec<NamedResource> {
    let Some(declared) = stage_body.and_then(super::declared_template) else {
        return Vec::new();
    };
    named_resources(&declared, stack, parameters)
        .into_iter()
        .filter(|candidate| {
            !already_managed
                .iter()
                .any(|logical_id| logical_id == &candidate.logical_id)
        })
        .filter(|candidate| {
            !managed_names.iter().any(|held| {
                held.resource_type == candidate.resource_type
                    && held.physical_resource_id == candidate.physical_resource_id
            })
        })
        .collect()
}

/// Those of `candidates` whose identifier the owning service already holds a resource under.
///
/// The calls are made with no store lock held (the caller is what releases it), one candidate
/// at a time in the order they are given, so the check costs one read per NAMED resource and
/// no more. A read the owning service does not serve, or one it refuses, establishes nothing
/// about the name: such a candidate is not reported, because a name is reported as taken only
/// on a resource that was read back.
pub async fn taken_names(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    candidates: &[NamedResource],
) -> Vec<NamedResource> {
    let mut taken = Vec::new();
    for candidate in candidates {
        let Some(owner) = owner_for(&candidate.resource_type) else {
            continue;
        };
        // The read is by the resource's own identifier: what the create would produce for
        // this declaration is what the owning service would know it by.
        let existing = StoredResource {
            logical_id: candidate.logical_id.clone(),
            resource_type: candidate.resource_type.clone(),
            physical_resource_id: candidate.physical_resource_id.clone(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties: Value::Null,
            materialised: true,
            deletion_policy: None,
        };
        // The read is BOXED: one owning service's read is a call into that service's own
        // handler, whose future is as large as every arm of it, and this stage runs inside a
        // deploying operation that holds the whole walk's future as well. Keeping it on the
        // heap keeps the caller's own future the size of the operation rather than the size of
        // the deepest read it makes.
        let answered = ::std::boxed::Box::pin(owner.actual_configuration(ctx, region, &existing))
            .await;
        // Only a configuration that was REPORTED says the name is taken: a read the owning
        // service answered not-found on, and a type no read is served for, each establish
        // nothing about the name.
        if matches!(answered, Ok(ActualConfiguration::Reported(_))) {
            taken.push(candidate.clone());
        }
    }
    taken
}

/// The first resource `changes` would CREATE under one of the `taken` identifiers, or `None`
/// when the plan creates none of them.
///
/// The plan is walked in its own order, so the resource reported is the one the deployment
/// would have reached first. A name a resource the plan MODIFIES or REMOVES holds is not a
/// collision: that resource exists BECAUSE the stack has it.
pub fn failing_addition<'a>(
    changes: &[StoredResourceChange],
    taken: &'a [NamedResource],
) -> Option<&'a NamedResource> {
    changes
        .iter()
        .filter(|change| change.action == change_action::ADD)
        .find_map(|change| {
            taken
                .iter()
                .find(|candidate| candidate.logical_id == change.logical_resource_id)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the reason names the validations that failed, in the words CloudFormation words
    /// it in.
    #[test]
    fn the_reason_names_the_validations_that_failed() {
        let rows: &[(&str, &[&str], &str)] = &[
            (
                "the existence check alone",
                &[RESOURCE_EXISTENCE_CHECK],
                "The following hook(s)/validation failed: \
                 [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early \
                 Validation errors, use the DescribeEvents API for detailed failure \
                 information.",
            ),
            (
                "more than one, in evaluation order",
                &["AWS::EarlyValidation::One", "AWS::EarlyValidation::Two"],
                "The following hook(s)/validation failed: [AWS::EarlyValidation::One, \
                 AWS::EarlyValidation::Two]. To troubleshoot Early Validation errors, use \
                 the DescribeEvents API for detailed failure information.",
            ),
        ];
        for (case, validations, expected) in rows {
            let reason = failed_reason(validations);
            assert_eq!(
                reason, *expected,
                "case {case:?}: expected {expected:?}, got {reason:?}"
            );
        }
    }

    /// RULE: the resources checked are the ones the template NAMES, and the name is the one
    /// the declaration's own name property carries; which of them fails is the plan's to
    /// say — an addition's name collides, a modification's does not.
    #[test]
    fn the_checked_resources_are_the_ones_the_template_names() {
        let body = r#"{"Resources":{
            "Named":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"already-there"}},
            "Generated":{"Type":"AWS::S3::Bucket","Properties":{}},
            "Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"events"}},
            "Changed":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"kept"}}}}"#;
        let template = super::super::declared_template(body).expect("a well-formed template");
        let named = named_resources(
            &template,
            template::StackScope {
                region: "us-east-1",
                stack_name: "checked",
                stack_id: "arn:aws:cloudformation:us-east-1:000000000000:stack/checked/0",
                notification_arns: &[],
            },
            &BTreeMap::new(),
        );
        let expected = vec![
            NamedResource {
                logical_id: "Changed".to_string(),
                resource_type: "AWS::S3::Bucket".to_string(),
                physical_resource_id: "kept".to_string(),
            },
            NamedResource {
                logical_id: "Named".to_string(),
                resource_type: "AWS::S3::Bucket".to_string(),
                physical_resource_id: "already-there".to_string(),
            },
            // A topic is identified by its ARN, which is what the create produces from the
            // name the template states — so that is what the check asks Amazon SNS about.
            NamedResource {
                logical_id: "Topic".to_string(),
                resource_type: "AWS::SNS::Topic".to_string(),
                physical_resource_id: ::emulator_server::emulator_arn(
                    "sns",
                    "us-east-1",
                    "events",
                ),
            },
        ];
        assert_eq!(
            named, expected,
            "case \"a template naming three of its four resources\": expected {expected:?}, \
             got {named:?}"
        );

        let change = |action: &str, logical_id: &str| StoredResourceChange {
            action: action.to_string(),
            logical_resource_id: logical_id.to_string(),
            resource_type: "AWS::S3::Bucket".to_string(),
            physical_resource_id: None,
            scope: Vec::new(),
            replacement: None,
            details: Vec::new(),
            policy_action: None,
            resource_drift_status: None,
        };
        let rows: &[(&str, Vec<StoredResourceChange>, Option<&str>)] = &[
            (
                "a plan that adds a resource under a taken name",
                vec![
                    change(change_action::MODIFY, "Changed"),
                    change(change_action::ADD, "Named"),
                ],
                Some("Named"),
            ),
            (
                "a plan that only modifies and removes",
                vec![
                    change(change_action::MODIFY, "Changed"),
                    change(change_action::REMOVE, "Topic"),
                ],
                None,
            ),
            (
                "a plan that adds a resource the template does not name",
                vec![change(change_action::ADD, "Generated")],
                None,
            ),
        ];
        for (case, changes, expected) in rows {
            let failing = failing_addition(changes, &named);
            assert_eq!(
                failing.map(|resource| resource.logical_id.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}, got {failing:?}"
            );
        }
    }
}
