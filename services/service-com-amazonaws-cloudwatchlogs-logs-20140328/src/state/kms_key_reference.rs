// SPDX-License-Identifier: CC0-1.0

//! Whether the KMS key a request names is a key CloudWatch Logs could encrypt a resource's
//! data with at all: the FORM of the identifier, and what KMS answers about the key it names.
//!
//! `AssociateKmsKey` states the gate and the error class it answers with: "If you attempt to
//! associate a KMS key with a log group but the KMS key does not exist or the KMS key is
//! disabled, you receive an InvalidParameterException error", and `CreateLogGroup` states the
//! same for the key a group is created under. It also states what the member may carry — "The
//! Amazon Resource Name (ARN) of the KMS key to use when encrypting log data" — so an
//! identifier that is not a key's ARN names no key this member takes, and the two conditions
//! the gate refuses are the two that sentence names: there is no such key
//! ([`KEY_NOT_FOUND_MESSAGE`]), or KMS reports it in a state it cannot be used in
//! ([`KEY_DISABLED_MESSAGE`]).
//!
//! This is a different question from the one [`super::kms_key_grant`] asks: that one is about
//! the key's own key POLICY — whether the key may be used for THIS resource — and it can only
//! be asked of a key KMS reports. This module asks whether there is such a key.
//!
//! KMS is reached the way the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or leaves
//! the host, and the only caller-supplied value that reaches the client's authority is the
//! signing Region, narrowed first ([`service_common::arn::region_label`]).
//!
//! An answer that is not KMS reporting on the key — an emulator with no KMS mounted beside
//! this service, a fault on KMS's side — establishes NOTHING about it, so it refuses nothing:
//! a refusal has to be grounded in what KMS answered.

use super::callback_client::callback_client;

/// The ARN vendor of a KMS key.
const KMS_VENDOR: &str = "kms";

/// The ARN resource type of a KEY, which is what tells a key's ARN from an ALIAS's: an alias
/// is a re-targetable name for a key rather than a key, so it is not what this member takes.
const KEY_RESOURCE_TYPE: &str = "key/";

/// The error codes KMS answers for an identifier it reports no key for: one it holds no key
/// under, and one it cannot read as a key identifier at all. Either is KMS stating that there
/// is no such key, which is the condition this gate refuses.
const NO_SUCH_KEY_CODES: [&str; 2] = ["NotFoundException", "InvalidArnException"];

/// Whether `value` is the ARN of a KMS KEY — the form `kmsKeyId` documents ("The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data").
///
/// The partition, Region and account are not matched: a key's ARN is one whichever of those it
/// names, and which key it is, is KMS's answer rather than a reading taken here.
#[must_use]
pub fn is_key_arn(value: &str) -> bool {
    super::arn_service_and_resource(value)
        .is_some_and(|(service, resource)| service == KMS_VENDOR && {
            // `key/<key-id>`: the resource type, then an id. An ARN carrying the type and no
            // id names no key.
            resource
                .strip_prefix(KEY_RESOURCE_TYPE)
                .is_some_and(|key_id| !key_id.is_empty())
        })
}

/// The refusal CloudWatch Logs answers for a `kmsKeyId` that names no key it can find: a value
/// that is not a key's ARN at all, an alias (which is a name for a key rather than a key), and
/// a well-formed key ARN KMS holds no key under. Fixed text for the class, carrying nothing of
/// the request.
pub const KEY_NOT_FOUND_MESSAGE: &str = "The specified KMS Key Id could not be found.";

/// The refusal it answers for a key KMS does report, in a state it cannot encrypt with: "if you
/// attempt to associate a KMS key with a log group but [...] the KMS key is disabled, you
/// receive an InvalidParameterException error". A key is in that state exactly when KMS reports
/// it as not enabled — "specifies whether the KMS key is enabled. When KeyState is Enabled this
/// value is true, otherwise it is false" (`KeyMetadata$Enabled`).
pub const KEY_DISABLED_MESSAGE: &str = "The specified KMS key is marked as disabled.";

/// `Some(message)` when the `kmsKeyId` a request names is not a key this service could encrypt
/// with — it names no key KMS can find ([`KEY_NOT_FOUND_MESSAGE`]), or KMS reports the key in a
/// state it cannot be used in ([`KEY_DISABLED_MESSAGE`]) — and `None` when it is usable, or when
/// KMS did not report on it at all, which establishes nothing (see the module docs).
pub async fn refusal_for(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: &str,
) -> Option<String> {
    // Only the key's own ARN names a key here, so no other identifier form is even taken to
    // KMS: it names no key this member accepts.
    if !is_key_arn(kms_key_id) {
        return Some(KEY_NOT_FOUND_MESSAGE.to_string());
    }
    let narrowed = ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_kms, ctx.callback(), "kms", narrowed);
    match client.describe_key().key_id(kms_key_id).send().await {
        Ok(answer) => {
            // An answer carrying no metadata states nothing about the key.
            let metadata = answer.key_metadata()?;
            (!is_enabled(metadata)).then(|| KEY_DISABLED_MESSAGE.to_string())
        }
        Err(error) => {
            let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error);
            if code.is_some_and(|code| NO_SUCH_KEY_CODES.contains(&code)) {
                return Some(KEY_NOT_FOUND_MESSAGE.to_string());
            }
            // Not KMS reporting on the key, so nothing is established about it and nothing is
            // refused. Worth a debug line, because a deployment with no KMS to ask is one
            // where this gate does not run; the caller's own value is rendered in its escaped
            // `Debug` form, so a key identifier carrying a newline cannot forge a log line.
            ::tracing::debug!(
                kms_key = ?kms_key_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not report on the key this request names, so the key is not judged"
            );
            None
        }
    }
}

/// Whether the key KMS reported is ENABLED, the one state its data keys can be used in: every
/// other state — disabled, pending deletion, pending import — is a key that cannot encrypt
/// (`KeyMetadata$Enabled`, quoted on [`KEY_DISABLED_MESSAGE`]).
fn is_enabled(metadata: &::aws_sdk_kms::types::KeyMetadata) -> bool {
    matches!(
        metadata.key_state(),
        Some(::aws_sdk_kms::types::KeyState::Enabled)
    )
}

#[cfg(test)]
mod tests {
    use super::is_key_arn;

    /// RULE: `kmsKeyId` takes the ARN of a KMS KEY and nothing else — not an alias (a
    /// re-targetable name), not a bare identifier, not another vendor's ARN.
    #[test]
    fn only_a_kms_key_arn_names_a_key() {
        for (case, value, expected) in [
            (
                "a key ARN",
                "arn:aws:kms:eu-west-2:000000000000:key/4b2f1a60-1f2a-4f26-8a3a-1f4d9f0b61c2",
                true,
            ),
            (
                "a key ARN in another partition",
                "arn:aws-us-gov:kms:us-gov-west-1:000000000000:key/abc",
                true,
            ),
            (
                "an alias ARN",
                "arn:aws:kms:eu-west-2:000000000000:alias/log-data",
                false,
            ),
            ("an alias name", "alias/log-data", false),
            ("a bare key id", "4b2f1a60-1f2a-4f26-8a3a-1f4d9f0b61c2", false),
            ("a value that is no ARN", "not-an-arn", false),
            (
                "another vendor's ARN",
                "arn:aws:logs:eu-west-2:000000000000:log-group:g",
                false,
            ),
            (
                "a key ARN naming no key",
                "arn:aws:kms:eu-west-2:000000000000:key/",
                false,
            ),
            ("the empty value", "", false),
        ] {
            let actual = is_key_arn(value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected is_key_arn({value:?}) == {expected}, got {actual}"
            );
        }
    }
}
