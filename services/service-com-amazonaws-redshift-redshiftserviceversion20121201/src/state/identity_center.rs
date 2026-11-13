// SPDX-License-Identifier: CC0-1.0

//! The IAM Identity Center dependency an application registration has: the instance
//! the managed application is created in, and the refusal the creation is denied with.
//!
//! `CreateRedshiftIdcApplication` and `CreateQev2IdcApplication` each "create a new
//! managed application" in the IAM Identity Center instance their `IdcInstanceArn`
//! names, and each reports that application back
//! (`RedshiftIdcApplication$IdcManagedApplicationArn`,
//! `Qev2IdcApplication$IdcManagedApplicationArn`) — so the create performs
//! `sso:CreateApplication` on the caller's behalf, in that instance. An IAM Identity
//! Center application is a resource OF its instance, so an instance the account does
//! not hold has no application namespace to create one in: the dependent call is
//! denied, and Amazon Redshift surfaces that denial on the
//! `DependentServiceAccessDeniedFault` channel both operations model ("A dependent
//! service denied access for the integration", HTTP 403), registering nothing.
//!
//! WHICH instances the account holds is the owning service's own rule, asked here
//! through its read-only reading of it
//! ([`::service_com_amazonaws_ssoadmin_swbexternalservice::state::ComAmazonawsSsoadminSwbexternalserviceState::holds_instance`]),
//! so nothing about that rule is restated in this crate and a caller that creates (or
//! deletes) an instance changes the next answer. Asking is a READ: it enables no
//! instance and decides nothing for the IAM Identity Center service.
//!
//! Nothing is refused on an assumption: an emulator with no IAM Identity Center to ask
//! answers nothing about the account's instances, so it refuses nothing — the same
//! reading [`super::serverless`] takes of a peer that never answered.

/// The IAM Identity Center action an application registration performs on the caller's
/// behalf, as IAM names it.
pub const SSO_CREATE_APPLICATION: &str = "sso:CreateApplication";

/// The partition an IAM Identity Center resource is named under when the value the
/// request named is no ARN to read one off. Every ARN this emulator emits carries it.
const DEFAULT_PARTITION: &str = "aws";

/// The segment an IAM Identity Center INSTANCE ARN carries before its instance id
/// (`arn:aws:sso:::instance/ssoins-…`).
const INSTANCE_SEGMENT: &str = ":instance/";

/// The IAM principal every request to this emulator speaks for — the one the account
/// holds and `sts:GetCallerIdentity` reports — which is the principal an authorization
/// refusal names.
///
/// Read from the shared definition, so the principal this service is refused in the
/// name of is the same one every other service of this emulator names, and the same one
/// a client that reads its own identity is given.
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The IAM resource an `sso:CreateApplication` is evaluated against: the APPLICATION
/// namespace of one IAM Identity Center instance,
/// `arn:<partition>:sso::*:application/<instance id>/*`.
///
/// An IAM Identity Center application is a resource of its instance, so the instance's
/// id is part of the resource's name; the account and the application id are the IAM
/// wildcards the authorization is evaluated over, since the application has no id until
/// the call that would create it succeeds. The instance id is read off the ARN the
/// request named — an instance ARN carries it after its `:instance/` segment — so
/// nothing here is derived from a recording.
pub fn identity_center_application_resource(instance_arn: &str) -> String {
    let partition = ::service_common::arn::parse(instance_arn)
        .map(|arn| arn.partition)
        .unwrap_or(DEFAULT_PARTITION);
    let instance = instance_arn
        .split_once(INSTANCE_SEGMENT)
        .map(|(_, instance)| instance)
        .unwrap_or(instance_arn);
    format!("arn:{partition}:sso::*:application/{instance}/*")
}

/// Whether the account's IAM Identity Center ANSWERED that `instance_arn` names no
/// instance of this account.
///
/// `false` — refuse nothing — when nothing is KNOWN about the account's instances: an
/// emulator the IAM Identity Center service is not mounted on cannot show the instance
/// to be absent.
pub fn identity_center_instance_is_absent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_arn: &str,
) -> bool {
    use ::service_com_amazonaws_ssoadmin_swbexternalservice::state as identity_center;

    let Some(state) = identity_center::service_state(engine) else {
        return false;
    };
    let held = state.lock().holds_instance(instance_arn);
    !held
}

/// The message an application registration is denied with when the
/// `sso:CreateApplication` it performs on the caller's behalf is denied, or `None` when
/// nothing denies it.
///
/// The one condition this service can SHOW: the instance the request names is not one
/// the account's IAM Identity Center holds, so there is no application namespace to
/// create the managed application in. The sentence is the standard IAM
/// authorization-failure one, from the single shared statement of it
/// ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]) rather
/// than a copy, so two services that reach the same decision word it the same way: the
/// principal that called, the action performed on its behalf, and the application
/// namespace of the instance the request named.
pub fn sso_create_application_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_arn: &str,
) -> Option<String> {
    if !identity_center_instance_is_absent(engine, instance_arn) {
        return None;
    }
    Some(
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            &caller_principal(),
            SSO_CREATE_APPLICATION,
            &identity_center_application_resource(instance_arn),
        ),
    )
}
