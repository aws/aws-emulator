// SPDX-License-Identifier: CC0-1.0

//! What KMS says about the key this service was handed, and the ONE client every such
//! question is put through.
//!
//! Two kinds of operation ask: the control plane, before it stores a key on a stream or a
//! channel ("Enables or updates server-side encryption using an Amazon Web Services KMS key
//! for a specified stream", `StartStreamEncryption`), and the DATA plane, on every call that
//! wraps or unwraps a record under that key — which is why `PutRecord`, `PutRecords` and
//! `GetRecords` model the whole KMS error family (`KMSDisabledException`,
//! `KMSInvalidStateException`, `KMSNotFoundException`, `KMSAccessDeniedException`). A
//! customer who disables the CMK stops the stream's data plane, and the service says so on
//! the call rather than silently writing records nothing could decrypt.
//!
//! KMS is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over
//! the in-process callback's HTTP client and credentials, so nothing here opens a socket or
//! leaves the host, and the only caller-supplied value that reaches the client's authority is
//! the signing Region, read as a Region LABEL first ([`::service_common::arn::region_label`]).
//!
//! An answer that is NOT KMS stating the key's state — no KMS mounted beside this service,
//! a fault, a throttle, a refusal about the call — establishes nothing about the key, so it
//! refuses nothing ([`KeyState::Unknown`]): a refusal has to be grounded in what KMS
//! answered.

/// The KMS client this service puts every question about a key through. `region` is the
/// request's own signing region, which is where the stream or channel that key encrypts
/// lives.
///
/// That region is CALLER-SUPPLIED text and here it ends up in an endpoint URL's AUTHORITY,
/// so it is read as a Region LABEL rather than interpolated as given. `None` for a value
/// that would name an authority the caller chose, in which case no lookup is made at all and
/// nothing is refused on it.
pub fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_kms::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        // The endpoint is the LABEL the in-process transport routes by (and what the
        // request is signed for); the callback above dispatches it inside this process, so
        // no socket is opened and nothing outside the emulator is reached.
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_kms::Client::from_conf(config))
}

/// What KMS answered about one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyState {
    /// KMS resolved the identifier to a key that is usable, and reported it under this ARN.
    Usable(String),
    /// KMS resolved it to a key that is DISABLED, reported under this ARN. "The request was
    /// rejected because the specified customer master key (CMK) isn't enabled"
    /// (`KMSDisabledException`).
    Disabled(String),
    /// KMS reported that this account and region hold no key the identifier resolves to.
    NoSuchKey,
    /// KMS said nothing about the key itself. Not a negative, so not a refusal.
    Unknown,
}

/// The key state the `Disabled` key state is spelled with in KMS's own answer
/// (`KeyMetadata$KeyState`, whose value set is "Creating | Disabled | Enabled |
/// PendingDeletion | PendingImport | PendingReplicaDeletion | Unavailable | Updating").
const KEY_STATE_DISABLED: &str = "Disabled";

/// Ask KMS what state the key `key_id` names is in.
///
/// `DescribeKey` is the read, because it resolves every form `KeyId` takes (a key id, a key
/// or alias ARN, an alias name, and the service's own predefined alias) and reports both the
/// key's ARN — which is what a refusal names — and its state. A definite negative is only
/// KMS's own "no such key" answer with the client status it came with; see the module docs
/// for why everything else is [`KeyState::Unknown`].
pub async fn key_state(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> KeyState {
    let Some(client) = kms_client(ctx, region) else {
        return KeyState::Unknown;
    };
    let error = match client.describe_key().key_id(key_id).send().await {
        Ok(answer) => {
            let Some(metadata) = answer.key_metadata() else {
                return KeyState::Unknown;
            };
            // A key KMS reported without an ARN is still the key the request named, so the
            // identifier it was named by stands in for it.
            let arn = metadata.arn().unwrap_or(key_id).to_string();
            return if metadata.key_state().map(|state| state.as_str()) == Some(KEY_STATE_DISABLED)
            {
                KeyState::Disabled(arn)
            } else {
                KeyState::Usable(arn)
            };
        }
        Err(error) => error,
    };
    let about_the_key = error.as_service_error().is_some_and(
        ::aws_sdk_kms::operation::describe_key::DescribeKeyError::is_not_found_exception,
    );
    // A 5xx is about KMS, not about the key, whatever code travels with it.
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    if about_the_key && client_error {
        return KeyState::NoSuchKey;
    }
    // Visible to an operator: an answer nothing can be read from refuses nothing, so
    // without this line it would be indistinguishable from a key KMS reported as usable.
    ::tracing::warn!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "KMS gave no answer about this key, so nothing is refused on its state"
    );
    KeyState::Unknown
}

/// The key policy KMS holds for `key_id`, or `None` when KMS did not answer with one —
/// which establishes nothing about the key and therefore refuses nothing.
pub async fn key_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> Option<String> {
    let client = kms_client(ctx, region)?;
    match client.get_key_policy().key_id(key_id).send().await {
        Ok(answer) => answer.policy().map(str::to_string),
        Err(error) => {
            // The caller's own value is rendered in its escaped `Debug` form, so a key
            // identifier carrying a newline cannot forge a log line.
            ::tracing::debug!(
                kms_key = ?key_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not state a key policy for the key this request names, so the key \
                 is not judged against one"
            );
            None
        }
    }
}

/// How the data plane states that the stream's CMK is disabled: the sentence KMS itself
/// refuses a request on a disabled key with ("`<key ARN>` is disabled."), carried by this
/// service's modelled `KMSDisabledException`. The variable part is the ARN of the key the
/// stream is encrypted under, as KMS reported it.
#[must_use]
pub fn key_disabled_message(key_arn: &str) -> String {
    format!("{key_arn} is disabled.")
}

/// How the control plane refuses a key that is not enabled: FIXED text, the sentence AWS
/// answers `StartStreamEncryption` with for a key it will not encrypt a stream under.
pub const KEY_NOT_ENABLED_MESSAGE: &str = "Key is not enabled.";
