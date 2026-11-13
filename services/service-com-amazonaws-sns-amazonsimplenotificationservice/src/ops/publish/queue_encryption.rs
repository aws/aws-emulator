// SPDX-License-Identifier: CC0-1.0

//! The customer managed key a SUBSCRIBED QUEUE encrypts its messages with, and whether this
//! service may have a data key generated under it — which is what handing one notification to
//! such a queue needs ([`super::Queues::hand_over`]).
//!
//! The rule: a queue carrying `KmsMasterKeyId` has every message written to it encrypted
//! under that key, and the queue's own service performs that encryption by calling KMS for
//! the WRITER — "you must have the `kms:GenerateDataKey` permission". A notification is
//! written by this service as its own service principal ([`super::SNS_SERVICE_PRINCIPAL`])
//! rather than as the queue's owner, so the KEY's own policy is a second document in force
//! over the delivery, beside the queue's own access policy ([`super::resource_policy_allows`]):
//! a key policy that admits this service's principal nothing is a write that does not happen
//! however freely the queue's policy grants the send. The notification is then WITHHELD — the
//! queue receives nothing, while the publishing call still answers with a message id — and a
//! client that adds the grant sees the next notification arrive.
//!
//! The decision is KMS's OWN ([`state::topic_encryption::withheld_by`], over
//! `kms_authorization::authorize_use`) over the key record the KMS service's own handlers
//! wrote, so `PutKeyPolicy` moves it with nothing else changed and the two services cannot
//! disagree about what a key policy says. It is the same question the TOPIC's own key is judged
//! by, asked for this service's principal acting for the queue.
//!
//! Only the PERMISSION is decided here. Everything else about the key — that it exists, that
//! its state admits a cryptographic operation at all, that a data key can be generated under
//! its spec — is decided by the queue's own `SendMessage`, which refuses the hand-over with
//! the KMS error it relays; that refusal arrives on the ordinary path a refused hand-over
//! takes ([`super::answered_refusal`]), so nothing about it is judged twice.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

use crate::state;

use super::{QueueLocation, SNS_SERVICE_PRINCIPAL};

/// The KMS store, as its own service's state struct — the one `CreateKey` and `PutKeyPolicy`
/// write.
pub(super) type KmsState =
    ::service_com_amazonaws_kms_trentservice::ComAmazonawsKmsTrentserviceState;

/// The KMS store in the SAME shared engine, which is where the key a subscribed queue is
/// encrypted with lives. `None` means the engine carries no such store — a deployment with no
/// KMS to ask, which is evidence of nothing.
pub(super) fn kms_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<KmsState>> {
    kms_store::service_state(engine)
}

/// The status Amazon SQS answers a send it could not have the queue's key used for with: the
/// `Kms*` family it models that refusal in is modelled at HTTP 400.
const KMS_REFUSAL_STATUS: u16 = 400;

/// The error code Amazon SQS answers a send whose writer holds no permission for the KMS
/// operation under: "`KmsAccessDenied` (HTTP 400) — The caller doesn't have the required KMS
/// access" (`SendMessage`).
const KMS_ACCESS_DENIED_CODE: &str = "KmsAccessDenied";

/// Why a notification is not handed to a queue: its encryption key admits this service
/// nothing that would let the message be sealed under it.
///
/// Carries KMS's own sentence for the refusal, which is what the delivery-status record of
/// the attempt reports as the queue service's answer.
#[derive(Debug, Clone)]
pub(super) struct KeyWithheld {
    /// KMS's refusal sentence: who asked, for what, on which key, and which document
    /// withheld it.
    message: String,
}

impl KeyWithheld {
    /// What the queue's service is reported to have answered for this delivery: the status
    /// its `Kms*` refusals are modelled at, and the error document the refusal is stated in.
    pub(super) fn answered(&self) -> state::delivery_status::Answered {
        state::delivery_status::Answered {
            status_code: Some(KMS_REFUSAL_STATUS),
            provider_response: Some(super::error_document(
                KMS_ACCESS_DENIED_CODE,
                Some(&self.message),
            )),
        }
    }
}

/// Why the key `master_key_id` names — the one the queue at `queue` encrypts its messages
/// with — withholds a delivery to it, or `None` when it lets this service seal the
/// notification under it.
///
/// The use is one the queue's own service makes on the writer's behalf rather than one this
/// account asked KMS for directly, so it is judged as arriving THROUGH that service
/// ([`kms_authorization::RequestedUse::through_service`]) — the queue service's regional
/// endpoint, which is what a key policy scoped by `kms:ViaService` is written against. The
/// principal it is judged FOR is this service's own, acting for the queue: such a principal
/// holds no identity-based policies, so the KEY's own policy is the only authority over the
/// use, and a key carrying only the policy `CreateKey` writes for the account root admits it
/// nothing until a statement names it.
///
/// `None` wherever nothing of the kind can be shown, so a notification is never withheld on a
/// guess: an engine carrying no KMS store, an identifier naming no key that Region holds, a
/// key Amazon Web Services manages (whose policy AWS writes for the service that owns it),
/// and a key no cryptographic operation can use right now at all — the last is a different
/// failure from a permission the key withholds, and the queue's own send is what reports it.
///
/// Call it with no store of this service locked: the KMS guard is taken here and released
/// before it answers.
pub(super) fn withheld_by_queue_key(
    kms: Option<&::engine::ServiceState<KmsState>>,
    queue: &QueueLocation,
    master_key_id: &str,
) -> Option<KeyWithheld> {
    let master_key_id = master_key_id.trim();
    if master_key_id.is_empty() {
        return None;
    }
    let Some(kms) = kms else {
        // Visible to an operator: a lookup nothing answered lets the delivery through, so
        // without this line it would be indistinguishable from a key whose policy KMS said
        // admits this service.
        ::tracing::warn!(
            "no KMS service is mounted beside Amazon SNS; an encrypted queue's key is accepted"
        );
        return None;
    };
    let caller =
        kms_authorization::Caller::service_principal(SNS_SERVICE_PRINCIPAL, &queue.queue_arn);
    let via_service =
        state::topic_encryption::regional_endpoint(super::SQS_ARN_NAMESPACE, &queue.region);
    let store = kms.lock();
    let kms_store::KeyRef::Found(key) = store.resolve(&queue.region, master_key_id) else {
        return None;
    };
    // The crate's ONE permission question about sealing a message under a customer managed key,
    // asked here for this service's own principal acting for the queue: which operations a seal
    // performs, and the keys it judges none of, are held in one place, so the topic's own key
    // and a subscribed queue's cannot drift apart.
    state::topic_encryption::withheld_by(&caller, &store, &queue.region, key, &via_service)
        .map(|message| KeyWithheld { message })
}
