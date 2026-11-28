// SPDX-License-Identifier: CC0-1.0

//! The ONE replay decision this service's creates take on an `@idempotencyToken`.
//!
//! `clientToken` is an `@idempotencyToken` on every create of this service: the
//! same token replays the first call's answer, and the same token carrying
//! DIFFERENT parameters is refused rather than silently answering for parameters
//! the caller did not send. The model declares no `IdempotentParameterMismatch`,
//! so that refusal is each operation's own `ValidationException` (400).
//!
//! A create that MINTS an id is answered from the record
//! [`ComAmazonawsRtbfabricRtbfabricState::idempotency`](super::ComAmazonawsRtbfabricRtbfabricState::idempotency)
//! holds under its token, so the decision — how the token is keyed, how a prior
//! record is looked up, how the parameter fingerprints are compared, how the
//! mismatch is worded and what bounds the records retained — is stated once here
//! rather than once per create: two creates then cannot drift apart about what a
//! replay is, and a rule fixed for one create is fixed for every create. Each
//! handler supplies only its own fingerprint rendering and its own reading of the
//! stored resource.
//!
//! A call that mints NO id keeps the record of the change on the resource that took
//! it ([`AcceptedUpdates`](super::AcceptedUpdates)) and answers a replay from that
//! resource; the refusal it words is this same one, so it is read from
//! [`token_mismatch_message`] here too.

use super::*;

/// What the first call carrying an `@idempotencyToken` minted, so a retry with
/// the same token answers with the same resource instead of a second one.
///
/// Fixed size: see [`IDEMPOTENCY_QUOTA`] for the retention rule.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The id the first call minted.
    pub resource_id: String,
    /// A digest of that call's other parameters: the documented contract is that
    /// a retry of the same token with DIFFERENT parameters fails rather than
    /// silently answering the first call's resource.
    pub fingerprint: [u8; 32],
}

/// How a create presenting a `clientToken` is to be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateRetry {
    /// The store retains no record of this token: the call is a request of its
    /// own, to be served and then retained ([`ClientToken::retain`]).
    Fresh,
    /// The same token carrying the same parameters: the id the first call minted,
    /// whose stored resource is what the handler reports. A record is let go of
    /// with the resource it points at, so a store that no longer holds that
    /// resource leaves the handler serving the call as a request of its own.
    Replay(String),
    /// The same token carrying DIFFERENT parameters: refused, with this message.
    Mismatch(String),
}

/// One create's reading of its own `clientToken`: the key the record is held
/// under, the fingerprint of the parameters presented with it, and the wording a
/// mismatch is refused in.
#[derive(Debug, Clone)]
pub struct ClientToken {
    /// The digest the record is keyed by ([`idempotency_key`]), which scopes the
    /// token to the operation it was presented to.
    key: [u8; 32],
    /// The digest of the other parameters this call presented
    /// ([`parameter_fingerprint`]).
    fingerprint: [u8; 32],
    /// The refusal a later call reusing this token with other parameters is
    /// answered with ([`token_mismatch_message`]).
    mismatch_message: String,
}

impl ClientToken {
    /// The `client_token` a create of `operation` was called with, whose other
    /// parameters render as `parameters`.
    ///
    /// `parameters` is the one thing each create states for itself: the members a
    /// retry has to repeat to be the same request, rendered by the handler that
    /// knows which those are.
    pub fn presented(operation: &str, client_token: &str, parameters: &str) -> Self {
        Self {
            key: idempotency_key(operation, client_token),
            fingerprint: parameter_fingerprint(parameters),
            mismatch_message: token_mismatch_message(operation, client_token),
        }
    }

    /// How this call is to be answered, from the records `store` retains.
    pub fn classify(&self, store: &ComAmazonawsRtbfabricRtbfabricState) -> CreateRetry {
        match store.idempotency.get(&self.key) {
            None => CreateRetry::Fresh,
            Some(prior) if prior.fingerprint == self.fingerprint => {
                CreateRetry::Replay(prior.resource_id.clone())
            }
            Some(_) => CreateRetry::Mismatch(self.mismatch_message.clone()),
        }
    }

    /// Retain this call as the one that minted `resource_id`, so a retry under the
    /// same token answers with that resource instead of minting a second one.
    pub fn retain(self, store: &mut ComAmazonawsRtbfabricRtbfabricState, resource_id: String) {
        store.idempotency.insert(
            self.key,
            IdempotentCreate {
                resource_id,
                fingerprint: self.fingerprint,
            },
        );
    }
}

/// The refusal a create is answered with while the store already holds its limit
/// of client-token records ([`IDEMPOTENCY_QUOTA`]), and `None` while it can retain
/// another.
///
/// Read by the call that would insert, BEFORE it inserts, which is what keeps the
/// records retained bounded.
pub fn client_token_quota_refusal(store: &ComAmazonawsRtbfabricRtbfabricState) -> Option<String> {
    (store.idempotency.len() >= IDEMPOTENCY_QUOTA).then(|| {
        format!("this emulator holds its limit of {IDEMPOTENCY_QUOTA} client-token records")
    })
}

/// The refusal a `clientToken` presented to `operation` with parameters other than
/// the ones it was first presented with is answered with.
///
/// One wording for every surface of this service that takes an
/// `@idempotencyToken`, whether the record of the call lives in the store's own
/// map or on the resource that took the change: the caller is told which token was
/// already spent and which operation spent it, and nothing of the parameters
/// either call carried.
pub fn token_mismatch_message(operation: &str, client_token: &str) -> String {
    format!(
        "clientToken {client_token} was already used for a {operation} request with different parameters"
    )
}

/// The digest an idempotency record is keyed by: the operation the token was
/// presented to, and the token itself. Hashing both keeps the retained key a
/// fixed 32 bytes however long a `clientToken` the caller chose — the model
/// bounds neither its length nor its content.
pub fn idempotency_key(operation: &str, client_token: &str) -> [u8; 32] {
    digest(&[operation.as_bytes(), client_token.as_bytes()])
}

/// The digest of a create's parameters, which an `@idempotencyToken` retry is
/// compared against. Fixed size, so a retained record never grows with the
/// request it came from.
pub fn parameter_fingerprint(rendering: &str) -> [u8; 32] {
    digest(&[rendering.as_bytes()])
}

/// SHA-256 over `parts`, each length-prefixed so two different splits of the
/// same bytes cannot collide.
///
/// `pub(super)` so the sibling modules that key a record or a precondition by a
/// digest reach THIS one rather than spelling a second hash of their own.
pub(super) fn digest(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A tag map rendered canonically (sorted by key), so the same tags in a
/// different iteration order fingerprint identically while two DIFFERENT maps
/// never render alike.
///
/// Each key and each value is written with its own byte length in front of it,
/// which is what [`digest`] does to the parts it hashes and for the same reason:
/// a modelled tag value carries no pattern at all and a tag key admits `=`, so
/// any separator this rendering could choose is a character the tags themselves
/// may contain. With the lengths in the rendering, where one pair ends and the
/// next begins is read off it rather than guessed, so `{"a": "b,c=d"}` and
/// `{"a": "b", "c": "d"}` are the two distinct maps they are.
///
/// A map stating no pairs renders as the empty string, the same as no tags at
/// all: a create holds the tags it was given and neither form gives it any.
///
/// A fingerprint is only ever compared with one computed by the same build in
/// the same process, so the rest of a request can be rendered with `Debug`,
/// which quotes and escapes every string it writes; a `HashMap` cannot, because
/// its iteration order varies between two calls that carry identical tags.
pub fn canonical_tags(tags: Option<&HashMap<String, String>>) -> String {
    use std::fmt::Write as _;

    let mut pairs: Vec<(&str, &str)> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort_unstable();
    let mut rendered = String::new();
    for (key, value) in &pairs {
        let _ = write!(rendered, "{}:{key}{}:{value}", key.len(), value.len());
    }
    rendered
}
