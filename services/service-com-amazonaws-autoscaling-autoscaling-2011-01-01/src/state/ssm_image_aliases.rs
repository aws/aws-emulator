// SPDX-License-Identifier: CC0-1.0

//! WHAT THE `ImageId` OF A GROUP'S LAUNCH SOURCE RESOLVES TO when it names a Systems Manager
//! parameter instead of an AMI.
//!
//! "Alternatively, you can specify a Systems Manager parameter, using one of the following
//! formats. The Systems Manager parameter WILL RESOLVE TO AN AMI ID ON LAUNCH"
//! (`RequestLaunchTemplateData$ImageId`): Amazon EC2 stores such a value unresolved, so this
//! service — the CONSUMER of the launch template — is what resolves it when it launches from
//! the version. The form the value carries is Amazon EC2's own reading of that member
//! ([`ec2::ssm_image_alias_parameter`], the one reading of it in this emulator) and the
//! parameter behind it is the resource of the service that OWNS parameters, read from THAT
//! service's own store — the one `ssm:PutParameter` writes and `ssm:GetParameter` reports —
//! rather than from a second copy of it here. So a parameter a client creates resolves, one it
//! deletes stops resolving, and the version selector a reference carries selects the same
//! version a read of it would report.
//!
//! A reference that store produces no value for is a launch Amazon EC2 would not carry out:
//! the group places nothing and records the invalid-ALIAS sentence Amazon EC2 states for it
//! ([`invalid_aliases_launch_failure`]).
//!
//! An engine carrying no Parameter Store at all shows NOTHING about the reference, so such a
//! launch is left exactly as it was — the rule every other gate over another service's store
//! in this tree keeps.
//!
//! LOCK ORDER: the published catalog is projected from Amazon EC2's own image state, so it is
//! read BEFORE Parameter Store's guard is taken and no two of those stores are ever held at
//! once. A caller reaches this with no guard of any store held.

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use ::service_com_amazonaws_ssm_amazonssm::state as ssm;

/// What Parameter Store holds for one `resolve:ssm:` reference a launch source carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasedLaunchImage {
    /// The value the parameter the reference names holds, which is the AMI the launch runs.
    Value(String),
    /// Parameter Store produces no value for the reference: nothing of that name at this
    /// endpoint, or a selector the parameter does not carry.
    Unresolved,
}

/// What the launch source's `image_id` resolves to, or `None` when it names an AMI directly —
/// and `None` as well on an engine that carries no Parameter Store to ask.
pub fn aliased_launch_image<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    image_id: &str,
) -> Option<AliasedLaunchImage> {
    let reference = ec2::ssm_image_alias_parameter(image_id)?;
    let state = ssm::service_state(engine)?;
    // Which parameter, which account and Region, and which version the reference names: the
    // reading of the service that owns parameters, so every documented form of the value — a
    // name, a name with a version or a label, and the parameter ARN in each of those shapes —
    // names here what it names there. A reference that service can read nothing out of, and
    // one addressing another endpoint, name no parameter it holds.
    let Some(query) = ssm::parse_parameter_query(reference) else {
        return Some(AliasedLaunchImage::Unresolved);
    };
    if query.selector == Some("") || !query.names_this_endpoint(region) {
        return Some(AliasedLaunchImage::Unresolved);
    }
    // The parameters AWS itself publishes resolve beside the account's own, the way every read
    // of that service resolves one — projected before its guard is taken, because projecting it
    // reads Amazon EC2's store.
    let published = ssm::public_parameters::published(engine, region, query.name);
    let store = state.lock();
    let value = store
        .parameter(region, query.name)
        .or(published.as_ref())
        .and_then(|record| record.select(query.selector))
        .map(|version| version.value.clone())
        .filter(|value| !value.is_empty());
    Some(match value {
        Some(value) => AliasedLaunchImage::Value(value),
        None => AliasedLaunchImage::Unresolved,
    })
}

/// What a capacity change records when the launch source's image reference resolves to
/// nothing: Amazon EC2's own sentence for an ALIAS it could not resolve, naming the reference
/// the launch template carried, followed by the sentence every launch failure of this crate
/// ends in ([`super::LAUNCH_FAILED_SENTENCE`]).
#[must_use]
pub fn invalid_aliases_launch_failure(reference: &str) -> String {
    format!(
        "{}{}. {}",
        ec2::INVALID_SSM_ALIASES_PREFIX,
        ec2::echoed_resource_id(reference),
        super::LAUNCH_FAILED_SENTENCE
    )
}
