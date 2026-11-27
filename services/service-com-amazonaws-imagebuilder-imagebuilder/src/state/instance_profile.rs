// SPDX-License-Identifier: CC0-1.0

//! Whether the instance profile a configuration names is one this account holds.
//!
//! `CreateInfrastructureConfiguration`'s `instanceProfileName` is "the instance profile to
//! associate with the instance used to customize your Amazon EC2 AMI" — an IAM resource of
//! the caller's own account. Its model says only what such a NAME looks like (required,
//! `@length(1..256)`, `@pattern ^[\w+=,.@-]+$`), so a well-formed name that no profile
//! exists under satisfies the model and whether it resolves is a question for IAM: Image
//! Builder resolves it when the configuration is created, and refuses the create — writing
//! nothing — when this account holds no such profile.
//!
//! IAM is reached the way any emulated service reaches another: the ONE in-process IAM
//! client this tree builds that hop with ([`service_common::iam_client::iam_client`]), over
//! the operation callback, so this crate names no other service crate and no request leaves
//! the process.
//!
//! ## What grounds a refusal
//!
//! The two ways the lookup fails to find a profile are different claims, and only one of
//! them is about the value the request carried:
//!
//! * IAM ANSWERED that this account holds no profile of that name (`NoSuchEntity`) — a fact
//!   about `instanceProfileName`, so the create is refused as that parameter's value, in the
//!   one sentence this service refuses a parameter's value in
//!   ([`missing_instance_profile_message`]);
//! * nothing answered the lookup at all — this emulator was assembled with no IAM to ask —
//!   so nothing about the profile is established and the create proceeds rather than
//!   refusing a value no answer placed.

use ::aws_sdk_iam::error::SdkError;
use ::aws_sdk_iam::operation::get_instance_profile::GetInstanceProfileError;
use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;

/// The member an instance profile name arrives in, as the API addresses it — the parameter a
/// refusal of such a value names.
pub const INSTANCE_PROFILE_NAME_MEMBER: &str = "instanceProfileName";

/// Why a create or an update naming an instance profile this account holds none of is
/// refused.
///
/// It is stated in the ONE sentence this service refuses a parameter's value in
/// (`service_common::validation_message::parameter_value_mismatch`) — the sentence a value a
/// modelled constraint, a parse or a page token placed is refused in — over the requirement
/// that an instance profile of that name exists, so the wording does not drift between the
/// things that establish a value is not admitted. The member the value arrived in is the only
/// variable part — nothing of the name the request carried, and no id, ARN or timestamp,
/// appears in it, and the value itself is WITHHELD.
#[must_use]
pub fn missing_instance_profile_message(member: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        member,
        "The provided instance profile does not exist. Please specify a different instance \
         profile and try again.",
    )
}

/// What a lookup of the instance profile a request named established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceProfileLookup {
    /// IAM holds an instance profile of that name in this account.
    Held,
    /// IAM answered that this account holds no instance profile of that name.
    Absent,
    /// Nothing established whether the profile exists: no IAM answered the lookup.
    Unestablished,
}

/// What IAM answers about the instance profile `name` of this account, asked in the Region
/// this request was signed for.
pub async fn instance_profile_named(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    name: &str,
) -> InstanceProfileLookup {
    let region = super::request_region(ctx.parts());
    let answer = ::service_common::iam_client::iam_client(ctx, &region)
        .get_instance_profile()
        .instance_profile_name(name)
        .send()
        .await;
    match answer {
        Ok(_) => InstanceProfileLookup::Held,
        // IAM itself says this account holds no such profile: the refusal is grounded in
        // its answer.
        Err(error) if profile_is_absent(&error) => InstanceProfileLookup::Absent,
        // Any other answer is this emulator failing to ask IAM rather than IAM reporting an
        // unresolvable profile, so it refuses nothing — and it is worth a warning, because a
        // deployment with no IAM to ask is one where this resolution does not happen. The
        // caller's own value is rendered in its escaped `Debug` form, so a name carrying a
        // newline cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                instance_profile_name = ?name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an instance profile could not be resolved against the emulator's IAM"
            );
            InstanceProfileLookup::Unestablished
        }
    }
}

/// Whether an IAM `GetInstanceProfile` failure is IAM reporting that the account holds no
/// such profile (`NoSuchEntity`), as opposed to the call not reaching IAM at all.
fn profile_is_absent(error: &SdkError<GetInstanceProfileError, HttpResponse>) -> bool {
    error
        .as_service_error()
        .is_some_and(GetInstanceProfileError::is_no_such_entity_exception)
}
