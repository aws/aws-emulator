// SPDX-License-Identifier: CC0-1.0

//! Whether the customer managed key a request names may be used for the resource it names.
//!
//! `CreateLogGroup` documents the gate: "If you attempt to associate a KMS key with the log
//! group but the KMS key does not exist or the KMS key is disabled, you receive an
//! InvalidParameterException error", and `AssociateKmsKey` adds that the association is what
//! makes CloudWatch Logs encrypt that resource's data with the key. Using a key is a KMS
//! operation, so what decides whether CloudWatch Logs MAY use it is the key's own KEY POLICY,
//! read for the service principal CloudWatch Logs calls KMS as.
//!
//! The scoping matters because it is how such a key is normally written: a key policy grants
//! CloudWatch Logs its encrypt operations only under
//! [`LOG_RESOURCE_ENCRYPTION_CONTEXT_KEY`], the encryption-context key CloudWatch Logs passes
//! the encrypted resource's ARN in, so one key can be confined to one log group. A key
//! confined that way is not usable for any OTHER resource, and the service says so on the
//! call that would associate it rather than storing an association whose key could never
//! encrypt anything.
//!
//! KMS is reached the way the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or leaves
//! the host, and the only caller-supplied value that reaches the client's authority is the
//! signing Region, narrowed first ([`service_common::arn::region_label`]).
//!
//! An answer that is not KMS stating the key's policy — an emulator with no KMS mounted
//! beside this service, a key this account does not hold, any other failure to ask —
//! establishes NOTHING about the key, so it refuses nothing: a refusal has to be grounded in
//! the policy KMS answered with.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::callback_client::callback_client;

/// The service principal CloudWatch Logs calls KMS as, in both spellings a key policy may be
/// written for ([`super::SERVICE_PRINCIPAL`]): a policy granting either names this service, so
/// both are offered to the evaluation.
fn service_principals(region: &str) -> [String; 2] {
    [
        super::SERVICE_PRINCIPAL.to_string(),
        super::regional_service_principal(region),
    ]
}

/// The encryption-context key CloudWatch Logs passes the encrypted resource's ARN in, which
/// is what a key policy scopes a grant to one log group with.
const LOG_RESOURCE_ENCRYPTION_CONTEXT_KEY: &str = "kms:EncryptionContext:aws:logs:arn";

/// The condition key naming the service making the KMS request on the caller's behalf.
const VIA_SERVICE_KEY: &str = "kms:ViaService";

/// The condition key naming the account whose credentials the KMS request was made under.
const CALLER_ACCOUNT_KEY: &str = "kms:CallerAccount";

/// The KMS operations an association needs the key for: CloudWatch Logs encrypts the data it
/// ingests under the key and decrypts it to serve it back, so a key policy that grants the
/// service none of them cannot be used for the resource.
const ENCRYPTION_ACTIONS: &[&str] = &[
    "kms:Encrypt",
    "kms:Decrypt",
    "kms:ReEncryptFrom",
    "kms:ReEncryptTo",
    "kms:GenerateDataKey",
    "kms:DescribeKey",
];

/// The refusal CloudWatch Logs answers when the key a request names cannot be used for the
/// resource it names.
///
/// The service draws no distinction between a key that is not there and a key whose policy
/// does not cover this resource — both leave it unable to encrypt — so one sentence states
/// both possibilities, with the ARN of the resource the key was asked to encrypt as its only
/// variable part.
#[must_use]
pub fn key_not_usable_message(resource_arn: &str) -> String {
    format!(
        "The specified KMS key does not exist or is not allowed to be used with Arn \
         '{resource_arn}'"
    )
}

/// `Some(message)` when KMS's own key policy says the key `kms_key_id` names may NOT be used
/// to encrypt `resource_arn`, and `None` when it may be — or when KMS did not answer with a
/// policy at all, which establishes nothing (see the module docs).
///
/// `resource_arn` is the ARN of the resource whose data the key would encrypt — a log group's
/// `logGroupArn`, or the `query-result` ARN of the account's query results — which is the
/// value CloudWatch Logs passes KMS in [`LOG_RESOURCE_ENCRYPTION_CONTEXT_KEY`] and therefore
/// the value a scoped key policy is written against.
pub async fn refusal_for(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: &str,
    resource_arn: &str,
) -> Option<String> {
    if kms_key_id.trim().is_empty() {
        return None;
    }
    let policy = key_policy(ctx, region, kms_key_id).await?;
    (!policy_permits(&policy, region, kms_key_id, resource_arn))
        .then(|| key_not_usable_message(resource_arn))
}

/// Whether `policy` lets CloudWatch Logs use the key for `resource_arn`.
///
/// Allowed when the policy grants the service principal ANY of [`ENCRYPTION_ACTIONS`] under
/// the condition context this use carries: a policy scoped to another resource's ARN grants
/// none of them here, and one that states nothing about this principal grants nothing at all.
/// A key policy's `Resource` element is `*` — "in a key policy, the value of the Resource
/// element is `*`, which means this KMS key" (KMS Developer Guide, *Key policies*) — so the
/// key's own identifier is what the request names as its resource.
fn policy_permits(policy: &str, region: &str, kms_key_id: &str, resource_arn: &str) -> bool {
    let principals = service_principals(region);
    let context = [
        (
            LOG_RESOURCE_ENCRYPTION_CONTEXT_KEY.to_string(),
            resource_arn.to_string(),
        ),
        (
            VIA_SERVICE_KEY.to_string(),
            super::regional_service_principal(region),
        ),
        (
            CALLER_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let published = [
        LOG_RESOURCE_ENCRYPTION_CONTEXT_KEY,
        VIA_SERVICE_KEY,
        CALLER_ACCOUNT_KEY,
    ];
    ENCRYPTION_ACTIONS.iter().any(|action| {
        let request = iam::Request::new(action, kms_key_id, &principals)
            .with_context(&context)
            .with_published_keys(&published);
        iam::resource_decision(policy, &request) == Decision::Allowed
    })
}

/// The key policy KMS holds for `kms_key_id`, or `None` when KMS did not answer with one.
async fn key_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: &str,
) -> Option<String> {
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_kms, ctx.callback(), "kms", region);
    match client
        .get_key_policy()
        .key_id(kms_key_id)
        .send()
        .await
    {
        Ok(answer) => answer.policy().map(str::to_string),
        // Not KMS reporting a policy, so nothing is established about the key and nothing is
        // refused. Worth a debug line, because a deployment with no KMS to ask is one where
        // this gate does not run; the caller's own value is rendered in its escaped `Debug`
        // form, so a key identifier carrying a newline cannot forge a log line.
        Err(error) => {
            ::tracing::debug!(
                kms_key = ?kms_key_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not state a key policy for the key this request names, so the key is \
                 not judged against one"
            );
            None
        }
    }
}
