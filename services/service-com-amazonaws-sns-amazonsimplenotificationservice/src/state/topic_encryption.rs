// SPDX-License-Identifier: CC0-1.0

//! The customer managed key a TOPIC is encrypted with, and whether a message may be sealed
//! under it right now — the step every operation that writes a message to a topic takes
//! before it accepts one.
//!
//! The rule: a topic carrying [`KMS_MASTER_KEY_ID_ATTRIBUTE`] has every message published to
//! it encrypted under that key, and Amazon SNS performs that encryption by asking KMS for a
//! data key as the message is published. So the KEY's own condition is in force over the
//! publish: a key KMS cannot use for a cryptographic operation at that moment seals nothing,
//! and the publish is refused rather than answered with a message id — a client is never told
//! a message is enveloped under a key that could not have encrypted it. The key's condition
//! is read when the message is published rather than when the topic was configured, so
//! disabling the key stops the topic from taking traffic and enabling it again lets the topic
//! take traffic once more, with nothing written to the topic in between.
//!
//! The key is resolved in the KMS service's OWN store, through KMS's own resolution of a
//! `KeyId` — so a key id, a key ARN, an alias and an alias ARN all name the key their own
//! service says they name, and the two services cannot disagree about a key's state.
//!
//! Amazon SNS does not word these refusals itself: the call that failed is its own call to
//! KMS, so the message it answers with is KMS's own sentence followed by the metadata block an
//! AWS error carries — the service that refused, the status, the error code and the id of the
//! refused call ([`relayed`]). Which modelled error each refusal is answered in is the
//! publishing operation's own mapping, since each models the `KMS*` family itself.
//!
//! Only what the KEY ITSELF says about a cryptographic use is decided here, and only where a
//! key record says it: the state the key is in, and whether the authorities in force let this
//! publish have a data key generated under it. An identifier that resolves to no key record
//! attributes no condition to a key and so refuses nothing.
//!
//! The PERMISSION is the second half: Amazon SNS generates the data key on the publishing
//! caller's behalf, so the use is one that reaches KMS THROUGH this service, judged for the
//! principal that signed the publish by the documents in force on it — the key's own policy
//! and that principal's identity-based policies, which is KMS's OWN decision
//! ([`kms_authorization::authorize_use`]) over the key record KMS's own handlers wrote. A key
//! whose policy admits the publisher nothing seals no message, and the publish is refused
//! rather than answered with a message id. A key Amazon Web Services MANAGES is judged on its
//! state alone: AWS writes that key's policy for the service that owns it.
//!
//! When no KMS service is mounted beside this one, nothing answers for the key and the publish
//! proceeds: a condition nobody attributed to the key never refuses a publish.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

use super::KMS_MASTER_KEY_ID_ATTRIBUTE;

/// The key the topic's attribute set encrypts its messages under, or `None` for a topic on no
/// customer managed key.
///
/// An empty value is no key: an empty `KmsMasterKeyId` is how a caller takes a topic off its
/// key, so nothing downstream reads the empty string as a key identifier.
pub fn configured_master_key(attributes: &HashMap<String, String>) -> Option<&str> {
    attributes
        .get(KMS_MASTER_KEY_ID_ATTRIBUTE)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Why the topic's key cannot seal this message, in the KMS error class the publishing
/// operation answers it in. The payload is the message, as KMS answered it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// The key is disabled — KMS's `DisabledException`.
    Disabled(String),
    /// The key is in a state no cryptographic operation can use — KMS's
    /// `KMSInvalidStateException`.
    InvalidState(String),
    /// The authorities in force do not let this publish have a data key generated under the
    /// key — KMS's `AccessDeniedException`.
    AccessDenied(String),
}

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status KMS answers each of these refusals with.
const KMS_REFUSAL_STATUS: u16 = 400;

/// KMS's own error code for a key that is disabled.
const KMS_DISABLED_CODE: &str = "DisabledException";

/// KMS's own error code for a key whose state admits no cryptographic operation.
const KMS_INVALID_STATE_CODE: &str = "KMSInvalidStateException";

/// KMS's own error code for a use no document in force permits.
const KMS_ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The KMS operations sealing ONE message under a customer managed key performs: the data key
/// the message is encrypted under is generated, and a data key already in use is decrypted to
/// seal another message under it — which is why the permissions AWS documents for a publisher
/// to an encrypted topic, and for a queue subscribed to one, are `kms:GenerateDataKey*` and
/// `kms:Decrypt`.
///
/// Spelled without KMS's `kms:` namespace, which is the form
/// [`kms_authorization::RequestedUse`] takes the operation in.
pub const SEALING_OPERATIONS: &[&str] = &["GenerateDataKey", "Decrypt"];

/// The regional endpoint name of this service, which is the value of KMS's `kms:ViaService`
/// condition key for a use of a key that reaches KMS through Amazon SNS — what a key policy
/// written for an encrypted topic is scoped by.
fn via_service(region: &str) -> String {
    regional_endpoint(super::authorization::ARN_NAMESPACE, region)
}

/// The regional endpoint name of the service `namespace` names, which is the value KMS's
/// `kms:ViaService` condition key takes for a use that reaches it through that service.
///
/// One spelling for the whole crate, so the topic's own key and a subscribed queue's key cannot
/// be judged under two different forms of the same name.
pub fn regional_endpoint(namespace: &str, region: &str) -> String {
    format!("{namespace}.{region}.amazonaws.com")
}

/// A refusal KMS answered, as the publishing caller sees it: this service does not word these
/// refusals itself, it relays the call that failed — so the message is KMS's own sentence
/// followed by the metadata block an AWS error carries.
///
/// `code` is KMS's own error code for the class, which is not the `KMS*` code this service
/// answers with: the first names the shape KMS returned, the second the shape SNS relays it
/// as. The request id identifies the ONE refused call, so it is minted per refusal.
fn relayed(message: &str, code: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_REFUSAL_STATUS}; \
         Error Code: {code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// Whether the key `master_key_id` names — the one the topic encrypts its messages with — can
/// seal a message being published right now, judged at `region`'s endpoint for the principal
/// that signed the publish.
///
/// `master_key_id` is the topic's `KmsMasterKeyId` in any form KMS accepts: a key id, a key
/// ARN, an alias or an alias ARN.
///
/// `Ok(())` for a key KMS can use and lets this caller have a data key generated under, and
/// wherever nothing of the kind can be shown, so a publish is never refused on a guess: an
/// empty identifier (no key), an engine carrying no KMS store, and an identifier that resolves
/// to no key record this endpoint holds — which says nothing about a key, and whose resolution
/// is a concern of its own. See the module docs for the rule.
///
/// Call it with no store of this service locked: the KMS guard is taken here and released
/// before it answers, and the caller's own documents are read out of the IAM and STS stores
/// before that guard is taken at all.
pub fn authorize_topic_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    master_key_id: &str,
) -> Result<(), KeyRefusal> {
    let master_key_id = master_key_id.trim();
    if master_key_id.is_empty() {
        return Ok(());
    }
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the publish through, so
        // without this line it would be indistinguishable from a key KMS said it can use.
        ::tracing::warn!(
            master_key_id,
            "no KMS service is mounted beside Amazon SNS; an encrypted topic's key is accepted"
        );
        return Ok(());
    };
    // Who the publish is judged for, resolved before the KMS guard is taken: the stores that
    // answer it are other services' own.
    let caller = kms_authorization::caller(engine, ctx);
    let via_service = via_service(region);
    let found = {
        let mut store = kms.lock();
        // "Amazon Web Services services use DescribeKey to create Amazon Web Services managed
        // keys from a predefined Amazon Web Services alias with no key ID": a topic on such an
        // alias is on the Amazon Web Services managed key it names, and the first use of it is
        // what associates the two — so the key whose condition governs the publish exists from
        // this publish on.
        if !matches!(
            store.resolve(region, master_key_id),
            kms_store::KeyRef::Found(_)
        ) {
            if let Some(alias) = kms_store::alias_named_by(region, master_key_id) {
                store.associate_predefined_aws_alias(region, &alias);
            }
        }
        // An identifier that resolves to no key record attributes no condition to a key, so it
        // refuses nothing here: an identifier KMS cannot parse, one naming a key this endpoint
        // does not hold, and one whose account segment names no account are all a question
        // about the IDENTIFIER rather than about the key's state.
        match store.resolve(region, master_key_id) {
            kms_store::KeyRef::Found(key) => Some((
                KeyCondition::of(key),
                withheld_by(&caller, &store, region, key, &via_service),
            )),
            kms_store::KeyRef::NotFound
            | kms_store::KeyRef::MalformedArn
            | kms_store::KeyRef::UnauthorizedAccount => None,
        }
    };
    let Some((key, withheld)) = found else {
        return Ok(());
    };

    // A key that is merely DISABLED is its own refusal class, told apart from every other
    // state no cryptographic operation can use: a disabled key is one a client enables again,
    // and the two are modelled separately for exactly that reason.
    if key.state == kms_store::KEY_STATE_DISABLED {
        return Err(KeyRefusal::Disabled(relayed(
            &key.state_message,
            KMS_DISABLED_CODE,
        )));
    }
    if !key.usable {
        return Err(KeyRefusal::InvalidState(relayed(
            &key.state_message,
            KMS_INVALID_STATE_CODE,
        )));
    }
    if let Some(message) = withheld {
        return Err(KeyRefusal::AccessDenied(relayed(
            &message,
            KMS_ACCESS_DENIED_CODE,
        )));
    }
    Ok(())
}

/// KMS's own sentence for why the authorities in force withhold the data key a message needs
/// from `caller`, or `None` when they permit it.
///
/// The decision is KMS's OWN over the key record KMS's own handlers wrote, so `PutKeyPolicy`
/// moves it with nothing else changed and the two services cannot disagree about what a key
/// policy says. It is taken for the use as it really reaches KMS: through the service
/// `via_service` names, on the behalf of whoever `caller` is, under the operations sealing one
/// message performs ([`SEALING_OPERATIONS`]) — a refusal of either withholds the message.
///
/// `None` for a key Amazon Web Services MANAGES: AWS writes that key's policy for the service
/// that owns it, so nothing of this account's documents is what admits a use of it. `None` too
/// for a key no cryptographic operation can use right now at all — that is a different failure
/// from a permission the key withholds, and the key's own state is what reports it.
///
/// The ONE permission question about sealing a message under a customer managed key, so the
/// topic's own key ([`authorize_topic_key`]) and a subscribed queue's
/// ([`crate::ops::publish::queue_encryption`]) cannot drift on which operations a seal performs
/// or on the keys it does not judge.
pub fn withheld_by(
    caller: &kms_authorization::Caller,
    store: &kms_store::ComAmazonawsKmsTrentserviceState,
    region: &str,
    key: &kms_store::Cmk,
    via_service: &str,
) -> Option<String> {
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER || !key.is_usable() {
        return None;
    }
    SEALING_OPERATIONS.iter().find_map(|operation| {
        kms_authorization::authorize_use(
            caller,
            store,
            region,
            key,
            kms_authorization::RequestedUse::through_service(operation, via_service),
            // A sealed message carries no encryption context, so a grant constrained to one
            // does not permit this use: the authority it carries is not claimed rather than
            // assumed.
            None,
            None,
        )
        .err()
    })
}

/// What the key itself says about sealing a message under it, taken out of the KMS store so
/// the refusal is worded with its lock released.
#[derive(Debug, Clone)]
struct KeyCondition {
    /// The `KeyState` a reader of the key reports, which is the state the refusal is decided
    /// on — so the class the caller is refused in and the sentence it carries are the same
    /// state.
    state: String,
    /// How KMS words a refusal caused by that key state.
    state_message: String,
    /// Whether the key can be used in a cryptographic operation at all right now.
    usable: bool,
}

impl KeyCondition {
    fn of(key: &kms_store::Cmk) -> Self {
        Self {
            state: key.reported_key_state().to_string(),
            state_message: kms_store::invalid_state_message(key),
            usable: key.is_usable(),
        }
    }
}
