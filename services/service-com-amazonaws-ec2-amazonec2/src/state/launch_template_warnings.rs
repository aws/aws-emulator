// SPDX-License-Identifier: CC0-1.0

//! The validation WARNINGS a launch template's parameters earn.
//!
//! "If the launch template contains parameters or parameter combinations that are not valid,
//! an error code and an error message are returned for each issue that's found"
//! (`CreateLaunchTemplateResult$Warning`, and the same sentence on
//! `CreateLaunchTemplateVersionResult$Warning`): a launch template is a set of launch
//! PARAMETERS, not a launch, so a parameter or a combination no launch could use is STORED
//! and reported back as a finding instead of being refused — the launch that tries to use it
//! is what refuses it.
//!
//! An instance type the published catalog does not hold is one such parameter: the type an
//! instance launched from the template would be placed as is resolved when a launch is made,
//! so a write that names a type this service publishes no record of stores the parameters and
//! reports the type as a finding instead of refusing the write.
//!
//! Both creates answer from here, so the two cannot report one template's parameters
//! differently, and the findings sit beside the other readings of a version's parameters
//! this store makes (`super::launch_template_security_groups`).

use crate::ops::types::{RequestLaunchTemplateData, ValidationError, ValidationWarning};

use super::{
    iam_held_instance_profile, instance_type_is_modelled, nonexistent_instance_types_message,
    INVALID_INSTANCE_TYPE, INVALID_PARAMETER_COMBINATION,
};

/// The error code a reference to an IAM instance profile the account does not hold carries:
/// EC2's own code for a profile that is not there.
pub const VALIDATION_WARNING_IAM_INSTANCE_PROFILE_NOT_FOUND: &str =
    "InvalidIamInstanceProfile.NotFound";

/// The finding a template that declares a network interface but does not carry its security
/// groups inside it earns.
///
/// The sentence is the rule the parameters broke, as `SecurityGroupIds` states it — "if you
/// specify a network interface, you must specify any security groups as part of the network
/// interface instead of using this parameter"
/// (`RequestLaunchTemplateData$SecurityGroupIds`, and the same on `$SecurityGroups`).
pub const SECURITY_GROUPS_BELONG_TO_THE_NETWORK_INTERFACE: &str =
    "If you specify a network interface, you must specify all security groups as part of the \
     network interface.";

/// The findings one launch template's parameters earn, or `None` when they earn none.
///
/// The ONE reading of them, called by `CreateLaunchTemplate` and
/// `CreateLaunchTemplateVersion` alike. A template with nothing to report carries no
/// `Warning` element at all rather than an empty one: the member is there "if the launch
/// template contains parameters or parameter combinations that are not valid".
///
/// `engine` is the shared engine, because one of the findings is about a resource ANOTHER
/// component owns: whether the account holds the IAM instance profile the template names is
/// IAM's own record to answer.
pub fn launch_template_warning<Eng: Send + Sync + 'static>(
    engine: &Eng,
    data: &RequestLaunchTemplateData,
) -> Option<ValidationWarning> {
    let mut errors: Vec<ValidationError> = Vec::new();
    if let Some(instance_type) = unpublished_instance_type(data) {
        errors.push(ValidationError {
            // EC2's own code for an instance type that does not exist
            // ([`INVALID_INSTANCE_TYPE`]), in the sentence it words every answer about
            // supplied types the catalog does not hold with
            // ([`nonexistent_instance_types_message`]) — so a client reads one code and one
            // wording whether it asked the catalog about the type or named it as a launch
            // parameter a template keeps.
            code: Some(INVALID_INSTANCE_TYPE.to_string()),
            message: Some(nonexistent_instance_types_message(&[instance_type])),
        });
    }
    if !security_groups_are_part_of_the_network_interface(data) {
        errors.push(ValidationError {
            // "The error code that indicates why the parameter or parameter combination is
            // not valid" (`ValidationError$Code`): for two parameters that are each valid
            // but cannot be used together, the code this service already names once
            // ([`INVALID_PARAMETER_COMBINATION`]) — a finding states the same code a
            // refusal of the same condition would.
            code: Some(INVALID_PARAMETER_COMBINATION.to_string()),
            message: Some(SECURITY_GROUPS_BELONG_TO_THE_NETWORK_INTERFACE.to_string()),
        });
    }
    if let Some(message) = absent_instance_profile_reference(engine, data) {
        errors.push(ValidationError {
            code: Some(VALIDATION_WARNING_IAM_INSTANCE_PROFILE_NOT_FOUND.to_string()),
            message: Some(message),
        });
    }
    (!errors.is_empty()).then_some(ValidationWarning {
        errors: Some(errors),
    })
}

/// The instance type this template names that the published catalog does not hold, or `None`
/// when it names one the catalog holds or names none at all.
///
/// "The instance type" (`RequestLaunchTemplateData$InstanceType`) is the type an instance
/// launched from the template would be placed as, and which types EXIST is the catalog's own
/// answer ([`instance_type_is_modelled`], the one reading of it every operation of this
/// service that resolves a type takes) rather than a list kept here. A template is a set of
/// launch parameters, so a type the catalog does not hold is a finding on the write and a
/// refusal on the launch that tries to use it.
///
/// A value with no text names no type: ec2Query carries an omitted member and an empty one
/// identically, which is a template that says nothing about an instance type rather than one
/// naming a type that does not exist.
fn unpublished_instance_type(data: &RequestLaunchTemplateData) -> Option<String> {
    let instance_type = data
        .instance_type
        .as_deref()
        .filter(|instance_type| !instance_type.is_empty())?;
    (!instance_type_is_modelled(instance_type)).then(|| instance_type.to_string())
}

/// Whether this template's security groups are where a template that declares a network
/// interface has to put them.
///
/// A template that declares NO network interface is free to name its groups on its own
/// members, so there is nothing to report for it. One that DOES declare an interface may not
/// name them there: "if you specify a network interface, you must specify any security
/// groups as part of the network interface instead of using this parameter"
/// (`RequestLaunchTemplateData$SecurityGroupIds`, and the same on `$SecurityGroups`) — the
/// two members are then launch parameters no launch can use together, which is the finding.
///
/// Naming NO group at all is not one: a version that names none in the place that applies is
/// a version whose instance is given its VPC's default group
/// ([`super::launch_template_security_groups`]), so there is nothing contradictory about it.
///
/// Public because the CONSUMERS of a launch template ask the same question of the version a
/// resource of theirs would launch from: THE ONE reading of these members against
/// `$NetworkInterfaces` in the tree, so no service decides on its own which of two spellings
/// of the same parameters a launch can use.
#[must_use]
pub fn security_groups_are_part_of_the_network_interface(
    data: &RequestLaunchTemplateData,
) -> bool {
    let declared = data.network_interfaces.as_deref().unwrap_or_default();
    if declared.is_empty() {
        return true;
    }
    // A reference with no text names no group, the same reading
    // [`super::launch_template_security_groups`] takes of the members it reads.
    let named_outside = [
        data.security_group_ids.as_deref().unwrap_or_default(),
        data.security_groups.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .flatten()
    .any(|group| !group.is_empty());
    !named_outside
}

/// The sentence a template whose `IamInstanceProfile` the account holds no profile for
/// earns, or `None` when the reference names a profile the account has.
///
/// "The name or Amazon Resource Name (ARN) of an IAM instance profile"
/// (`RequestLaunchTemplateData$IamInstanceProfile`) is a reference to a resource IAM owns, so
/// whether the account HAS it is IAM's own record to answer and it is asked of that store
/// ([`iam_held_instance_profile`]) rather than guessed here — the same reading every EC2
/// operation that takes a profile reference makes. The template is stored either way: a
/// template is a set of launch parameters, so a reference that resolves to nothing is a
/// finding on the create and a refusal on the launch that uses it.
///
/// The sentence names the MEMBER the request spelled the profile on and quotes the value it
/// sent, so a caller can fix the template from the message alone. A reference with NO text
/// names no profile at all, which is a template that says nothing about one rather than one
/// that names a profile the account does not hold.
fn absent_instance_profile_reference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    data: &RequestLaunchTemplateData,
) -> Option<String> {
    let specification = data.iam_instance_profile.as_ref()?;
    // ec2Query carries an omitted member and an empty one identically, and an empty value
    // names no profile either way, so both read as "not given" — the same reading
    // [`super::resolve_iam_instance_profile`] takes of the two spellings.
    let arn = specification.arn.as_deref().filter(|arn| !arn.is_empty());
    let name = specification.name.as_deref().filter(|name| !name.is_empty());
    match (arn, name) {
        // An ARN names the profile IAM holds under exactly that ARN: the name is its last
        // segment, so a profile IAM holds under another path is not this reference's.
        (Some(arn), _) => ::service_common::iam_role::instance_profile_name(arn)
            .and_then(|name| iam_held_instance_profile(engine, name))
            .filter(|held| held.arn.as_deref() == Some(arn))
            .is_none()
            .then(|| absent_instance_profile_sentence("Arn", arn)),
        (None, Some(name)) => iam_held_instance_profile(engine, name)
            .is_none()
            .then(|| absent_instance_profile_sentence("Name", name)),
        (None, None) => None,
    }
}

/// The sentence above, worded once: the member the profile was named on and the value the
/// request sent on it.
fn absent_instance_profile_sentence(member: &str, reference: &str) -> String {
    format!("The IamInstanceProfile.{member} '{reference}' does not exist.")
}
