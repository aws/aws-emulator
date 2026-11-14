// SPDX-License-Identifier: CC0-1.0

//! The IAM instance profile the launch templates of an Auto Scaling group name, resolved
//! against the account's own IAM store.
//!
//! A launch template's `IamInstanceProfile` is a REFERENCE to another service's resource:
//! Amazon EC2 stores whatever well-formed value `CreateLaunchTemplate` was given, without
//! resolving it, and the reference is resolved when the template is CONSUMED. Amazon EC2
//! Auto Scaling is one of the consumers — a group launches its instances under that
//! profile — so a group whose launch template names an instance profile the account does
//! not hold cannot launch anything, and the request that would create or update it is
//! refused instead of stored.
//!
//! This module is the one place that resolution happens, for every launch template a
//! request names: the group's own `LaunchTemplate`, the launch template of its
//! `MixedInstancesPolicy`, and the per-override templates that policy may carry. The
//! profiles live in IAM's own store, reached through IAM's own accessor, so one instance
//! profile is never resolved by two services differently.
//!
//! LOCKING. The launch-template versions are read under Amazon EC2's store lock
//! ([`super::ec2_instances::launch_template_data`]), which is released before IAM's is
//! taken for the one pass that resolves the references; nothing awaits while either is
//! held. Callers in `src/ops/` resolve BEFORE taking this service's own lock, so no two
//! stores are ever held at once; neither Amazon EC2 nor IAM reaches this service, so no
//! cycle exists.

use ::service_com_amazonaws_ec2_amazonec2::ops::types::LaunchTemplateIamInstanceProfileSpecificationRequest;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use crate::ops::types::{LaunchTemplateSpecification, MixedInstancesPolicy};

/// The parameter a refusal names when the template supplied the profile's ARN, and the
/// sentence that refusal ends with. Both are Amazon EC2's own wording for a launch
/// parameter naming an instance profile that does not resolve, which is the reference being
/// resolved here.
const ARN_PARAMETER: (&str, &str) = ("iamInstanceProfile.arn", "Invalid IAM Instance Profile ARN");

/// The same two for a template that supplied the profile's NAME instead.
const NAME_PARAMETER: (&str, &str) = (
    "iamInstanceProfile.name",
    "Invalid IAM Instance Profile name",
);

/// One `IamInstanceProfile` reference a launch template carries.
#[derive(Debug, Clone)]
struct ProfileReference {
    /// The launch parameter the value was supplied under, as the refusal names it.
    parameter: &'static str,
    /// The sentence the refusal for that parameter ends with.
    detail: &'static str,
    /// The value exactly as the template carries it, which the refusal quotes back.
    value: String,
    /// The instance profile name the value denotes, or `None` when the value denotes no
    /// instance profile of this account at all — which is itself unresolvable.
    name: Option<String>,
}

impl ProfileReference {
    /// The reference `profile` carries, or `None` when it carries neither an ARN nor a
    /// name: a launch template may omit the member entirely, and the empty string names
    /// nothing either.
    fn of(profile: &LaunchTemplateIamInstanceProfileSpecificationRequest) -> Option<Self> {
        if let Some(arn) = profile.arn.as_deref().filter(|arn| !arn.is_empty()) {
            let (parameter, detail) = ARN_PARAMETER;
            return Some(Self {
                parameter,
                detail,
                value: arn.to_string(),
                // An ARN is read through the tree's one IAM-ARN reading, so a value that
                // is ARN-shaped but names another vendor, another IAM entity type, another
                // account or a Region an IAM ARN does not carry denotes no profile rather
                // than an unrelated name.
                name: ::service_common::iam_role::instance_profile_name(arn)
                    .map(::std::string::ToString::to_string),
            });
        }
        let name = profile.name.as_deref().filter(|name| !name.is_empty())?;
        let (parameter, detail) = NAME_PARAMETER;
        Some(Self {
            parameter,
            detail,
            value: name.to_string(),
            name: Some(name.to_string()),
        })
    }

    /// What AWS answers a request whose launch template carries this unresolvable
    /// reference: the launch template is not one the group can be built from, and the
    /// offending launch parameter and the value as supplied are named so the caller can fix
    /// the template.
    fn refusal(&self) -> String {
        format!(
            "You must use a valid fully-formed launch template. Value ({}) for parameter {} is \
             invalid. {}",
            self.value, self.parameter, self.detail
        )
    }
}

/// The refusal for a launch source whose launch template names an IAM instance profile this
/// account does not hold; `None` when every reference its templates carry resolves.
///
/// A template the account holds no version of contributes no reference: there is nothing to
/// resolve, which is the same position a launch source naming no template at all is in. A
/// reference that resolves to nothing is refused with HTTP 400 `ValidationError` — the code
/// Amazon EC2 Auto Scaling answers a request it cannot satisfy with — and the caller's
/// request is stored no more than any other refused one.
///
/// `None` also covers an engine carrying no IAM store at all: a refusal has to be grounded
/// in what the account's IAM state answered, never assumed.
pub fn unresolvable_profile_error<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let references: Vec<ProfileReference> =
        super::ec2_instances::specifications(launch_template, mixed_instances_policy)
            .into_iter()
            .filter_map(|specification| {
                super::ec2_instances::launch_template_data(engine, specification)
            })
            .filter_map(|data| data.iam_instance_profile)
            .filter_map(|profile| ProfileReference::of(&profile))
            .collect();
    if references.is_empty() {
        return None;
    }
    let state = iam::service_state(engine)?;
    let store = state.lock();
    references
        .into_iter()
        .find(|reference| {
            // IAM entity names "are not distinguished by case", so the profile is looked up
            // under the key IAM itself holds it by rather than under the casing the
            // template was written in.
            !reference.name.as_deref().is_some_and(|name| {
                store
                    .instance_profiles
                    .contains_key(&iam::name_key(name))
            })
        })
        .map(|reference| crate::scaffolded::Scaffolded::validation_error(reference.refusal()))
}
