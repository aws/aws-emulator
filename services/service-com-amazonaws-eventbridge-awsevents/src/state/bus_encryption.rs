// SPDX-License-Identifier: CC0-1.0

//! The customer managed key an EVENT BUS is configured with, and whether that key's own
//! policy lets this service use it — the check every writer of a bus's
//! `KmsKeyIdentifier` makes before the bus is stored as encrypted under it.
//!
//! "If you do not specify a customer managed key identifier, EventBridge uses an Amazon
//! Web Services owned key to encrypt events on the event bus"
//! (`smithy_query(query="doc",shape="CreateEventBusRequest",member="KmsKeyIdentifier")`),
//! so a bus that DOES name one has every event on it encrypted and decrypted under that
//! key — by EventBridge itself, as its own service principal
//! ([`super::SERVICE_PRINCIPAL`]) acting for that bus. Such a principal is no identity of
//! the account and holds no identity-based policies, so the KEY's own policy is the whole
//! authority over the use: a key whose policy does not admit this service admits it
//! nothing, whatever the configuring caller may do.
//!
//! That is why the configuration is refused rather than stored. A bus stored on a key
//! EventBridge cannot use would be reported as encrypted under a key that can encrypt
//! nothing, and the failure would surface later — on each publication — or never, instead
//! of at the call that asked for it.
//!
//! The decision is KMS's OWN ([`kms_authorization::authorize_use`]) over the key record
//! that service's handlers wrote, so `PutKeyPolicy` moves it with nothing else changed and
//! the two services cannot disagree about what a key policy says. It is the same reading
//! of a key policy a DELIVERY to an encrypted target takes
//! ([`super::target_encryption`]); what differs is the key, the resource it is used for
//! and the moment the question is asked.
//!
//! The emulator refuses only what it can SHOW, so each of these is evidence of nothing and
//! lets the configuration through: an engine carrying no KMS store beside this service, an
//! identifier the KMS store holds no key for — which may name a key of another account or
//! Region — and a key the account does not manage itself, whose policy AWS manages for the
//! service that owns it.

use ::std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

use super::*;

/// The encryption-context entry EventBridge binds an event bus's cryptographic operations
/// to: the ARN of the bus the data belongs to. A key policy statement or a grant
/// conditioned on it is claimed for that one bus and for no other, which is the scoping an
/// account writes a bus key with.
pub const EVENT_BUS_CONTEXT_ENTRY: &str = "aws:events:event-bus:arn";

/// The KMS operation EventBridge performs under an event bus's customer managed key, and
/// the one a refusal names: encrypting what is published onto the bus means having a data
/// key generated under that key ("GenerateDataKey returns a unique symmetric data key for
/// use outside of KMS", which is how a service encrypts a payload under a customer managed
/// key).
const BUS_KEY_OPERATION: &str = "GenerateDataKey";

/// The sentence EventBridge refuses a configuration whose key withholds the use with: the
/// call it could not make, and then KMS's own reason for a denial about a key it can report
/// nothing further about — the resource may not exist in this Region, no resource-based
/// policy may allow the use, or one may deny it, which is the one clause that covers every
/// way a key policy can withhold a service principal's use of a key (the spelling
/// `service_com_amazonaws_kms_trentservice::state::unauthorized_resource_message` words for
/// a request made to KMS directly).
fn denied_message(key_arn: &str) -> String {
    let action = format!("{}:{BUS_KEY_OPERATION}", kms_store::ARN_SERVICE);
    format!(
        "Access to call {action} is denied. Reason: {SERVICE_PRINCIPAL} is not authorized to \
         perform: {action} on {key_arn} because the resource does not exist in this Region, no \
         resource-based policies allow access, or a resource-based policy explicitly denies \
         access"
    )
}

/// Why the key `identifier` names does not let this service encrypt the bus `bus_arn`
/// under it, as KMS words that refusal — or `None` when it does, and for every case the
/// module docs record as evidence of nothing.
///
/// The use is one this service makes for its own resource rather than one the configuring
/// caller asked KMS for, so it is judged FOR this service's own principal acting for the
/// bus ([`kms_authorization::Caller::service_principal`], which is what gives a statement
/// scoped by `aws:SourceArn` or `aws:SourceAccount` its values) and as arriving THROUGH
/// this service ([`kms_authorization::RequestedUse::through_service`], which is what a
/// statement scoped by `kms:ViaService` is written against).
///
/// Call it with NO store of this service locked: the KMS guard is taken here and released
/// before it answers.
pub fn bus_key_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    identifier: &str,
    bus_arn: &str,
) -> Option<String> {
    let Some(kms) = super::target_encryption::kms_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the configuration
        // through, so without this line it would be indistinguishable from a key whose
        // policy KMS said admits this service.
        ::tracing::warn!(
            "no KMS service is mounted beside EventBridge; an event bus's customer managed key \
             is accepted"
        );
        return None;
    };
    let caller = kms_authorization::Caller::service_principal(SERVICE_PRINCIPAL, bus_arn);
    let via_service = super::target_encryption::via_service(ARN_NAMESPACE, region);
    let context = HashMap::from([(
        EVENT_BUS_CONTEXT_ENTRY.to_string(),
        bus_arn.to_string(),
    )]);
    let store = kms.lock();
    let kms_store::KeyRef::Found(key) = store.resolve(region, identifier) else {
        return None;
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    // The key's own refusal is not relayed as KMS worded it: the decision is what KMS
    // answers, and the SENTENCE is this service's own ([`denied_message`]), because the
    // caller asked EventBridge for a bus rather than asking KMS for a data key.
    kms_authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        kms_authorization::RequestedUse::through_service(BUS_KEY_OPERATION, &via_service),
        Some(&context),
        None,
    )
    .err()
    .map(|_| denied_message(&key.arn))
}
