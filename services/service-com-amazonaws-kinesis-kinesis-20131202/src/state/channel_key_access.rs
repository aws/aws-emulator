// SPDX-License-Identifier: CC0-1.0

//! Whether a channel's service execution role may USE the KMS key the create names.
//!
//! `CreateChannel` takes a key ("The identifier of the customer managed Amazon Web Services
//! KMS key", `ChannelEncryptionConfiguration$KeyId`) and the role the channel delivers
//! under ("The ARN of the IAM role that the channel assumes",
//! `CreateChannelInput$ServiceExecutionRoleARN`), and it models the whole KMS error family
//! beside `ValidationException` and `AccessDeniedException` — because the two are checked
//! TOGETHER, synchronously, before anything is provisioned: a channel whose role cannot use
//! its key could never deliver a record, so the service refuses the create rather than
//! minting a channel that fails later with no signal.
//!
//! The decision is KMS's own: a channel's delivery wraps records under a data key it asks
//! KMS for and unwraps them to deliver, so the role needs `kms:GenerateDataKey` and
//! `kms:Decrypt` on that key, and both halves of an AWS access decision have to grant them —
//! the KEY POLICY, which is the resource-based document KMS holds for the key, and, where
//! that policy delegates to the account rather than naming the role, the role's own
//! identity-based documents. Each half is evaluated by the tree's one evaluation of an IAM
//! document ([`::service_common::iam_authorization`]) over documents collected from the
//! service that HOLDS them: KMS states the key policy, IAM simulates the role's
//! ([`::service_common::iam_role::role_withholds_action`]).
//!
//! An answer that is not one of those services STATING a document establishes nothing, so it
//! refuses nothing: no KMS mounted beside this service, a key KMS will not state a policy
//! for, a role IAM holds nothing under. A refusal here is always grounded in a document that
//! was read.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The KMS operations a channel's delivery needs the key for: it asks KMS for a data key to
/// wrap the records it delivers and decrypts to read them back.
const DELIVERY_ACTIONS: &[&str] = &["kms:GenerateDataKey", "kms:Decrypt"];

/// The condition key naming the service making the KMS request on the role's behalf.
const VIA_SERVICE_KEY: &str = "kms:ViaService";

/// The condition key naming the account whose credentials the KMS request was made under.
const CALLER_ACCOUNT_KEY: &str = "kms:CallerAccount";

/// How the create refuses a key its service execution role may not use: FIXED text — it
/// names the member at fault and the two permissions the role needs, and nothing of the
/// request or the store.
pub const ROLE_NOT_AUTHORIZED_FOR_KEY_MESSAGE: &str =
    "The service execution role is not authorized to access the KMS key specified in \
     EncryptionConfiguration.KeyId. Verify that the role's IAM policy and KMS key policy \
     grants kms:Decrypt and kms:GenerateDataKey permissions.";

/// `true` — refuse the create — when the documents in force do NOT let the role `role_arn`
/// use the key `key_id` for a channel's delivery, and `false` when they do or when nothing
/// could be read to decide it (see the module docs).
pub async fn role_cannot_use_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
    role_arn: &str,
) -> bool {
    // The ARN KMS reports for the key is what both halves are evaluated against: a key
    // policy's statements are about the key itself, and the role's documents name it by ARN.
    // A key KMS says nothing about is judged against nothing.
    let key_arn = match super::key_state(ctx, region, key_id).await {
        super::KeyState::Usable(arn) | super::KeyState::Disabled(arn) => arn,
        super::KeyState::NoSuchKey | super::KeyState::Unknown => return false,
    };
    let Some(policy) = super::key_policy(ctx, region, key_id).await else {
        return false;
    };
    for action in DELIVERY_ACTIONS {
        if action_is_withheld(ctx, region, &policy, &key_arn, role_arn, action).await {
            return true;
        }
    }
    false
}

/// Whether `action` on the key is withheld from the role by the documents in force.
///
/// KMS's own evaluation order, for one action:
///
/// * a key policy statement that DENIES the role — or the account it belongs to — refuses,
///   whatever any allow says;
/// * a key policy that names the role itself in an `Allow` grants the action on its own
///   ("a key policy that allows a principal gives it permission without an IAM policy");
/// * a key policy that allows the ACCOUNT delegates the decision to that account's
///   identity-based policies, so the role's own documents decide — asked of IAM, which
///   holds them;
/// * a key policy that grants the role nothing at all withholds the action.
async fn action_is_withheld(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    policy: &str,
    key_arn: &str,
    role_arn: &str,
    action: &str,
) -> bool {
    // The condition context this use carries, so a statement conditioned on the service
    // making the call or on the account it is made under can be decided at all.
    let context = [
        (VIA_SERVICE_KEY.to_string(), via_service(region)),
        (
            CALLER_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let published = [VIA_SERVICE_KEY, CALLER_ACCOUNT_KEY];
    let role = [role_arn.to_string()];
    let account = iam::account_principals(role_arn);

    let named = iam::resource_decision(
        policy,
        &iam::Request::new(action, key_arn, &role)
            .with_context(&context)
            .with_published_keys(&published),
    );
    let delegated = iam::resource_decision(
        policy,
        &iam::Request::new(action, key_arn, &account)
            .with_context(&context)
            .with_published_keys(&published),
    );
    if named == Decision::Denied || delegated == Decision::Denied {
        return true;
    }
    if named == Decision::Allowed {
        return false;
    }
    if delegated == Decision::Allowed {
        return ::service_common::iam_role::role_withholds_action(
            ctx, region, role_arn, action, key_arn,
        )
        .await;
    }
    true
}

/// The service principal this service calls KMS as, in the Regional spelling a key policy
/// scoped to one Region's channels names.
fn via_service(region: &str) -> String {
    format!("{}.{region}.amazonaws.com", super::SERVICE_ARN_NAMESPACE)
}
