// SPDX-License-Identifier: CC0-1.0

//! Whether the KMS key an enabled `EncryptionAtRestOptions` names is a key the domain can
//! really be encrypted at rest with — the ONE statement of that rule, so every call that
//! takes the container judges it alike.
//!
//! `EncryptionAtRestOptions` is "whether the domain should encrypt data at rest, and if so,
//! the Key Management Service (KMS) key to use"
//! (`smithy_query(query="doc",shape="EncryptionAtRestOptions")`), and its `KmsKeyId` is
//! "the KMS key ID" (`…,member="KmsKeyId"`). A domain whose encryption at rest is enabled
//! is provisioned THROUGH that key, so the service ACCESSES the key before the
//! configuration is accepted: it asks KMS for the key the container names, and a key KMS
//! does not answer for, or answers for in any key state but `Enabled`, encrypts nothing —
//! so the request is refused with the operation's `ValidationException` (HTTP 400) and
//! nothing is stored.
//!
//! What the model cannot say, and this module does, is that `KmsKeyId` is a REFERENCE to
//! another service's resource rather than a free string. A KMS key is a regional resource:
//! it answers at one Region's endpoint in one account, so a key ARN naming another Region
//! names no key the domain's own endpoint can reach — which is why the Region is not a case
//! of its own here but the ordinary outcome of asking KMS at the domain's Region.
//!
//! The two refusals a caller sees are the two things that can go wrong with accessing the
//! key, and both are reported under the one sentence this service states them in — "Error
//! in Accessing KmsKeyID with details:" followed by the detail:
//!
//! * the KMS call FAILED, and the detail is what KMS answered: its own message, closed by
//!   the exception trailer an Amazon Web Services SDK appends to a service exception — the
//!   service, the status, the error code and the id of that KMS call. The message and the
//!   error code are KMS's own ([`kms_store`]), so a caller is told the same thing by
//!   whichever service it asked, and the id is minted per call because it identifies THAT
//!   attempt to access the key.
//! * the KMS call SUCCEEDED and the key it answered for is not in the one key state the
//!   domain can be encrypted with, which is this service's own statement about the key it
//!   resolved ([`not_in_enabled_state`]) rather than anything KMS refused — so it carries
//!   no exception trailer.
//!
//! The key is resolved in the KMS service's own store, reached through the SAME shared
//! engine this service's handlers reach their own store through — the store
//! `kms:CreateKey`, `kms:DisableKey`, `kms:EnableKey` and `kms:ScheduleKeyDeletion` write
//! and `kms:DescribeKey` reports, resolved the way that operation resolves it — the way
//! [`super::log_destinations`] resolves a log group and [`super::cognito_authentication`] a
//! user pool. That store belongs to the ENGINE rather than to a mounted service, so which
//! services the emulator happens to mount does not change what this check resolves.
//!
//! Nothing is refused on evidence that is not there: an engine handing out no KMS store
//! leaves the configuration to be served, as does a container that enables no encryption or
//! names no key of its own — one that names none asks for the Amazon Web Services managed
//! key of this service ([`AWS_MANAGED_KEY_ALIAS`]), and so does one that names that alias,
//! which is not a key a caller's request can misname.
//!
//! LOCKING: this takes the KMS store's guard, and a handler may ask it while holding this
//! service's own. No KMS operation reads this service's store, so the two can never be
//! taken in opposite orders, and the work done under both is a single lookup.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

use crate::ops::types::{EncryptionAtRestOptions, ValidationException};

/// The KMS API this service accesses the key through: `DescribeKey` is what reports "a key
/// state" of a KMS key, which is what this check needs of it, and an authorization refusal
/// names the action it refused.
const KEY_ACCESS_OPERATION: &str = "DescribeKey";

/// The status an Amazon Web Services SDK reports for the KMS exceptions relayed here: KMS
/// answers every client error on its protocol's client status.
const KMS_CLIENT_STATUS: u16 = 400;

/// The alias of the Amazon Web Services managed key THIS service encrypts a domain with
/// when the request names no key of its own: `aws/<service-name>` is the form of such an
/// alias ("you can recognize Amazon Web Services aliases because their names have the
/// format `aws/<service-name>`", `smithy_query(query="doc",shape="ListAliases")` in the KMS
/// model), under this service's own name — the one its endpoint and its signing name carry.
///
/// It is the ONE predefined alias a domain's encryption at rest is served by: KMS associates
/// a managed key with such an alias when it is first asked for, so this alias always names
/// an enabled key, and its key state is not one a caller moves. Another service's
/// `aws/<service-name>` alias names THAT service's managed key rather than a key this one
/// encrypts a domain with, so it is not short-circuited here — it is accessed like any other
/// reference.
///
/// It is also the key an enabled configuration that names NO key encrypts a domain under,
/// which is why [`super::encryption_grant`] reads it from here rather than spelling it
/// again.
pub(crate) const AWS_MANAGED_KEY_ALIAS: &str = "alias/aws/es";

/// Why the KMS key `options` names cannot encrypt a domain in `region` at rest, or `None`
/// when the configuration may be served.
///
/// `region` is the Region the request was signed for, which is the Region the domain is
/// created in and so the endpoint its key is accessed at: a domain and the key its data is
/// encrypted under answer in the same Region.
pub fn kms_key_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    options: Option<&EncryptionAtRestOptions>,
) -> Option<ValidationException> {
    let options = options?;
    // "Specifies whether the domain should encrypt data at rest": a container that
    // enables no encryption uses no key, so the value it carries encrypts nothing and
    // names nothing to access.
    if !options.enabled.unwrap_or(false) {
        return None;
    }
    let requested = options.kms_key_id.as_deref().filter(|id| !id.is_empty())?;
    // The Amazon Web Services managed key of THIS service ([`AWS_MANAGED_KEY_ALIAS`]) is
    // the key a domain encrypts with when no key is named, and naming that alias asks for
    // the same key: there is nothing about it for this check to refuse. WHICH alias an
    // identifier names is read with KMS's own helper, so the two crates cannot drift on the
    // spellings of an alias, and only at the domain's own Region: an alias ARN naming
    // another Region names no alias here, and falls through to the access below.
    if kms_store::alias_named_by(region, requested).as_deref() == Some(AWS_MANAGED_KEY_ALIAS) {
        return None;
    }
    let store = kms_store::service_state(engine)?;
    let keys = store.lock();
    let detail = match keys.resolve(region, requested) {
        // The key KMS answered for, in the key state the domain can be encrypted with:
        // the configuration the request asks for is one the service can provision.
        kms_store::KeyRef::Found(key)
            if key.reported_key_state() == kms_store::KEY_STATE_ENABLED =>
        {
            return None;
        }
        // The key EXISTS and is in another key state — disabled, awaiting deletion,
        // awaiting key material. KMS refused nothing, so what the caller is told is this
        // service's own statement about the key it accessed, named by the ARN KMS answered
        // for it.
        kms_store::KeyRef::Found(key) => not_in_enabled_state(&key.arn),
        // KMS answered for no key, and the caller is told what it answered: a reference
        // this account and Region holds no key for — a key ARN of another Region or
        // another account among them, and a value that is no key id at all.
        kms_store::KeyRef::NotFound => kms_failure(
            "NotFoundException",
            &kms_store::key_not_found_message(region, requested),
        ),
        // A value that is not a well-formed KMS key or alias ARN.
        kms_store::KeyRef::MalformedArn => kms_failure(
            "InvalidArnException",
            &kms_store::invalid_arn_message(requested),
        ),
        // An ARN whose account-id segment is no account id: KMS refuses the access for
        // want of permission before it looks anything up, and reports nothing about the
        // resource.
        kms_store::KeyRef::UnauthorizedAccount => kms_failure(
            "AccessDeniedException",
            &kms_store::unauthorized_account_message(KEY_ACCESS_OPERATION),
        ),
    };
    Some(ValidationException {
        message: Some(format!("Error in Accessing KmsKeyID with details:{detail}")),
    })
}

/// The detail for a key KMS answered for whose key state is not the one a domain's
/// encryption at rest requires: the key, by the ARN KMS named it, and the state it is not
/// in — the requirement rather than the state it happens to be in, so every state outside
/// it is stated the same way.
fn not_in_enabled_state(arn: &str) -> String {
    format!(
        "KmsKeyID:{arn} is not in {} State",
        kms_store::KEY_STATE_ENABLED
    )
}

/// The detail for a KMS call that FAILED: KMS's own message, closed by the trailer an
/// Amazon Web Services SDK appends to a service exception — the service it came from, the
/// status and error code it carried, the id of that call, and the proxy it went through
/// (none, in process).
///
/// The id is minted per call ([`::service_common::query_error_envelope::new_request_id`]):
/// it identifies THIS attempt to access the key, so it is never derived from the request
/// and never stored.
fn kms_failure(error_code: &str, message: &str) -> String {
    format!(
        "{message} (Service: AWSKMS; Status Code: {KMS_CLIENT_STATUS}; Error Code: \
         {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}
