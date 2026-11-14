// SPDX-License-Identifier: CC0-1.0

//! Whether the KMS key a write's `encryptionConfiguration` names is a key this
//! service could encrypt investigation data with — the one half of the rule that
//! cannot be decided from the request alone, because it is a fact only KMS holds.
//!
//! `encryptionConfiguration` is "Use this structure if you want to use a customer
//! managed KMS key to encrypt your investigation data"
//! (`smithy_query(query="shape",shape="EncryptionConfiguration")`), so the key the
//! member names is the key the group's data is encrypted WITH: a key that does not
//! exist encrypts nothing, and storing it would leave every later read reporting a
//! configuration nothing stands behind. The key is therefore RESOLVED before anything
//! is persisted, exactly as the group's `roleArn` is
//! ([`super::role_arn_rejection`]), and a key KMS reports it cannot encrypt with is
//! refused with the writing operation's modelled `ValidationException`
//! ([`KMS_KEY_NOT_USABLE`]) while the stored configuration is left as it was.
//!
//! The two members AGREEING is a different question, decided from the request alone:
//! [`super::encryption_configuration_rejection`].
//!
//! KMS is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client
//! driven over the in-process callback's own HTTP client and credentials, so nothing
//! here opens a socket, resolves a name or reaches anything outside this process. The
//! endpoint URL below is ADDRESSING for that transport, which routes the call back into
//! this emulator's own KMS service; its only variable part is the Region the request was
//! signed for — a KMS key is regional, so the key a value names is the one in that
//! Region — and that Region is already narrowed to what an ARN field admits
//! ([`super::request_region`]), so no caller-supplied text becomes a destination. The
//! caller's own `Authorization` header is never forwarded: the credentials are the
//! emulator's own.
//!
//! An answer that is not KMS reporting on the key — an emulator assembled with no KMS
//! beside this service, a fault on KMS's side — establishes NOTHING about it, so it
//! refuses nothing: a refusal has to be grounded in what KMS actually answered, which is
//! the same standard the role resolution holds itself to.

use crate::ops::types::EncryptionConfiguration;

/// The sentence a write naming a KMS key this service could not encrypt with is refused
/// with.
///
/// No recording of this service covers an unusable key, so the sentence is this
/// service's own vocabulary for the error class — the resolution of a caller-supplied
/// resource the group needs failed — as its recorded sibling words it
/// ([`super::ROLE_NOT_ASSUMABLE`]): it states both halves of what the caller has to fix,
/// the resource has to exist and it has to be usable by this service, and carries nothing
/// the request sent.
pub const KMS_KEY_NOT_USABLE: &str = "The InvestigationGroup could not use the KMS key \
     specified for encryption. Please validate that the key exists and is configured \
     correctly.";

/// The error codes that are KMS reporting the value names no key this account can
/// encrypt with here: one it holds no key for in this Region (`NotFoundException`), one
/// it cannot read as a key identifier at all (`InvalidArnException`), and one whose ARN
/// names another account, which KMS answers as an authorization failure
/// (`AccessDeniedException`). Every other answer says nothing about the key.
const KEY_IS_NOT_USABLE_CODES: [&str; 3] = [
    "NotFoundException",
    "InvalidArnException",
    "AccessDeniedException",
];

/// Why the key `configuration` names is not one this service could encrypt investigation
/// data with, or `None` when it is usable — and when nothing KMS answered shows it is
/// not (see the module docs).
///
/// A configuration naming no key names nothing to resolve: the Amazon Web Services owned
/// key is KMS's own and no `kmsKeyId` identifies it
/// (`super::default_encryption_configuration`).
///
/// ONE function for every operation that stores the member — the create and the update —
/// so the two cannot judge the same key differently. Awaited with NO store guard held.
pub async fn encryption_key_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let key_id = configuration?.kms_key_id.as_deref()?;
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_id)
        .send()
        .await
    {
        Ok(answer) => {
            // An answer carrying no metadata states nothing about the key.
            let metadata = answer.key_metadata()?;
            // Enabled is the one state a key's data keys can be used in: "specifies
            // whether the KMS key is enabled. When KeyState is Enabled this value is
            // true, otherwise it is false" (`KeyMetadata$Enabled`), so a disabled key, one
            // pending deletion and one still awaiting key material are each a key that
            // cannot encrypt.
            let enabled = matches!(
                metadata.key_state(),
                Some(::aws_sdk_kms::types::KeyState::Enabled)
            );
            (!enabled).then(|| KMS_KEY_NOT_USABLE.to_string())
        }
        Err(error) => {
            let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error);
            if code.is_some_and(|code| KEY_IS_NOT_USABLE_CODES.contains(&code)) {
                return Some(KMS_KEY_NOT_USABLE.to_string());
            }
            // Not KMS reporting on the key, so nothing is established about it and
            // nothing is refused. Worth a debug line, because an emulator with no KMS to
            // ask is one where this resolution does not run; the caller's own value is
            // rendered in its escaped `Debug` form, so a key identifier carrying a
            // newline cannot forge a log line.
            ::tracing::debug!(
                kms_key = ?key_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not report on the key this encryption configuration names, so the key \
                 is not judged"
            );
            None
        }
    }
}

/// A KMS client over the in-process callback into this same emulator, addressed to
/// `region`'s endpoint.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — each would need a sleep implementation this client is not given —
/// and the credentials the callback hands over are used directly rather than cached.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
