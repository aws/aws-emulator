// SPDX-License-Identifier: CC0-1.0

//! Whether this service can ASSUME the service execution role a channel's create names.
//!
//! A channel "reads records from the specified stream and writes them to" its destination
//! under the role the create hands it ("The ARN of the IAM role that the channel assumes",
//! `CreateChannelInput$ServiceExecutionRoleARN`), so that role has to let THIS service
//! principal assume it before anything is provisioned: a channel whose role this service
//! cannot take on could never read a record or write one, so the create is refused with the
//! modelled `ValidationException` rather than a channel being minted that delivers nothing.
//!
//! The question is the tree's shared reading of a caller-supplied execution role
//! ([`::service_common::iam_role::role_is_unassumable`]) — which role of this account the
//! ARN names, the in-process `GetRole`, and the trust-policy reading
//! ([`::service_common::iam_trust_policy`]) — so this module states only WHICH principal
//! has to be admitted and HOW this service words the refusal. A refusal is therefore always
//! grounded in what IAM ANSWERED: this account's IAM holds no role under the name the ARN
//! carries, or the trust policy of the role it does hold does not admit this principal with
//! the assume. Nothing that is not such an answer refuses anything — a value naming no role
//! of this account, an answer this emulator could not read, an emulator with no IAM mounted
//! at all.
//!
//! Whether that role may then USE the KMS key the same create names is the other half of
//! what `CreateChannel` judges synchronously ([`super::channel_key_access`]); this half is
//! asked first, because a role this service cannot assume at all is one no key policy can
//! say anything useful about.

/// The service principal a channel's service execution role has to trust: this service is
/// what assumes it, in the global spelling a trust policy names a service by.
pub const CHANNEL_SERVICE_PRINCIPAL: &str = "kinesis.amazonaws.com";

/// How the create refuses a service execution role this service cannot assume: FIXED text —
/// it names what to verify about the role and nothing of the request or the store, so the
/// refusal never echoes the role ARN back.
pub const UNASSUMABLE_SERVICE_EXECUTION_ROLE_MESSAGE: &str =
    "Unable to assume the service execution role. Verify that the role exists and the trust \
     policy allows Kinesis service principal and sts:AssumeRole permissions.";

/// `true` — refuse the create — when IAM ANSWERED that `role_arn` is a role this service
/// cannot assume, and `false` when it is assumable or when nothing could be read to decide
/// it (see the module docs).
pub async fn service_execution_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, CHANNEL_SERVICE_PRINCIPAL)
        .await
}
