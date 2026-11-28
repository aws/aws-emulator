// SPDX-License-Identifier: CC0-1.0

//! The ledger of the changes this service has accepted on a gateway.
//!
//! A change to a gateway, to a link one of its ends is, or to a routing rule of
//! such a link, applies ASYNCHRONOUSLY: the call that asks for it returns once the
//! change is accepted, and the change commits a span later. An update is the
//! record of one such accepted change — what kind it was, which resource it was
//! accepted on, when it was accepted and whether it has committed — so
//! `ListUpdates` answers from the ledger rather than from a reading of the
//! resources, which says nothing about changes that have already committed.
//!
//! The ledger is also where an update's id lives, which is what a retry of the
//! accepting call is answered with: an update mints an id of its own, so a call
//! carrying an `@idempotencyToken` records that token HERE, beside the id it
//! minted, rather than on the resource (see
//! [`AcceptedUpdates`](super::AcceptedUpdates), which answers a retry that minted
//! nothing).

use std::time::SystemTime;

use super::{ComAmazonawsRtbfabricRtbfabricState, PROVISIONING_DELAY};

/// `UpdateType` member: a change to a gateway's own configuration.
pub const UPDATE_TYPE_GATEWAY_CONFIG: &str = "GATEWAY_CONFIG_UPDATE";
/// `UpdateType` member: a change to a link's configuration.
pub const UPDATE_TYPE_LINK_CONFIG: &str = "LINK_CONFIG_UPDATE";
/// `UpdateType` member: a change to the flows a link runs.
pub const UPDATE_TYPE_LINK_FLOW: &str = "LINK_FLOW_UPDATE";
/// `UpdateType` member: a change to a routing rule of a link.
pub const UPDATE_TYPE_ROUTING_RULE: &str = "ROUTING_RULE_UPDATE";

/// `UpdateStatus` member: the change is accepted and has not committed yet.
pub const UPDATE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `UpdateStatus` member: terminal, the change has committed.
pub const UPDATE_STATUS_SUCCESSFUL: &str = "SUCCESSFUL";
/// `UpdateStatus` member: terminal, the change could not be applied.
pub const UPDATE_STATUS_FAILED: &str = "FAILED";

/// How many updates ONE gateway's ledger retains.
///
/// `GetUpdate` documents a 90-day retention window on an update, and this
/// emulator holds no record for 90 days, so this is the emulator's OWN bound
/// rather than a claim about that window: it is what stops a caller growing the
/// store by changing one gateway for ever, while sitting far above the changes
/// any workflow an emulator serves accepts. Once a gateway has accepted this many
/// further changes, the oldest record is let go of, exactly as a record past the
/// service's window is.
pub const UPDATES_RETAINED_PER_GATEWAY: usize = 100;

/// One accepted change, as `ListUpdates` reports it.
#[derive(Debug, Clone)]
pub struct Update {
    /// The id this service minted for the change, and the one the accepting call
    /// answered with.
    pub update_id: String,
    /// The gateway the change was accepted on — for a change to a link, the end
    /// the accepting call addressed, since that is the gateway a later listing is
    /// addressed to.
    pub gateway_id: String,
    /// The link the change was accepted on, when the change was to a link or to
    /// one of its routing rules; `None` for a change to the gateway itself.
    pub link_id: Option<String>,
    /// The routing rule the change was accepted on, when the change was to a
    /// rule; `None` otherwise.
    pub rule_id: Option<String>,
    /// Which kind of change this was, a member of the modelled `UpdateType`.
    pub update_type: String,
    /// Where the change has got to, a member of the modelled `UpdateStatus`.
    pub status: String,
    pub created_at: SystemTime,
    /// The instant the clock makes this change's commit due; `None` once the
    /// record has settled.
    pub settles_at: Option<SystemTime>,
    /// The `@idempotencyToken` the accepting call was presented under, folded
    /// into the digest [`idempotency_key`](super::idempotency_key) renders, and a
    /// fingerprint of that call's other parameters — so a retry is answered with
    /// this update's id, and the same token restating other parameters is
    /// refused. `None` for a change accepted under no token.
    pub token: Option<[u8; 32]>,
    /// The parameter fingerprint belonging to [`token`](Self::token).
    pub fingerprint: Option<[u8; 32]>,
}

impl Update {
    /// Materialise the commit the clock has already made due, dated at the
    /// instant it was owed rather than now, so the record does not depend on when
    /// it was next read.
    pub fn settle(&mut self, now: SystemTime) {
        if self.settles_at.is_some_and(|due| due <= now) {
            self.status = UPDATE_STATUS_SUCCESSFUL.to_string();
            self.settles_at = None;
        }
    }

    /// Whether the stated filters select this update. A filter the caller left
    /// out selects every update; one it stated selects the updates whose record
    /// names exactly that value, so a change to a gateway itself is not one of a
    /// link's changes.
    pub fn selected_by(
        &self,
        link_id: Option<&str>,
        rule_id: Option<&str>,
        update_type: Option<&str>,
        status: Option<&str>,
    ) -> bool {
        link_id.is_none_or(|id| self.link_id.as_deref() == Some(id))
            && rule_id.is_none_or(|id| self.rule_id.as_deref() == Some(id))
            && update_type.is_none_or(|kind| self.update_type == kind)
            && status.is_none_or(|status| self.status == status)
    }
}

/// What the resource a change was accepted on was, and under which token, as the
/// accepting handler states it.
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// The link the change was accepted on, for a change to a link or to one of
    /// its routing rules.
    pub link_id: Option<String>,
    /// The routing rule the change was accepted on.
    pub rule_id: Option<String>,
    /// The `@idempotencyToken` record, as [`Update::token`] holds it.
    pub token: Option<[u8; 32]>,
    /// The parameter fingerprint belonging to that token.
    pub fingerprint: Option<[u8; 32]>,
}

/// How a call presenting an `@idempotencyToken` to an operation that MINTS an
/// update id is answered.
///
/// The three answers are the ones every `@idempotencyToken` of this service is
/// answered with ([`Retry`](super::Retry)); this one carries the id the first call
/// minted, because that is what a retry has to report rather than a second update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRetry {
    /// The ledger retains no record of this token: the call is a request of its
    /// own, to be applied.
    Fresh,
    /// The same token carrying the same parameters: the change it asks for has
    /// already been accepted, so the caller is answered with the update it
    /// started rather than a second one.
    Replay(String),
    /// The same token carrying DIFFERENT parameters: refused, rather than
    /// answering for parameters the caller did not send.
    Mismatch,
}

/// The digest an update's `@idempotencyToken` record is keyed by: the operation
/// the token was presented to, the resource it was presented FOR, and the token
/// itself.
///
/// The resource travels in the key because a token is presented with a request,
/// and the same token addressed to another link, rule or gateway is another
/// request: it is applied there rather than answered from a change accepted
/// somewhere else. Hashing all three keeps the retained key a fixed 32 bytes
/// however long a `clientToken` the caller chose. The wording a mismatch is
/// refused with is this service's one
/// ([`token_mismatch_message`](super::token_mismatch_message)), shared with the
/// creates.
pub fn update_token_key(operation: &str, resource_id: &str, client_token: &str) -> [u8; 32] {
    super::idempotent_creates::digest(&[
        operation.as_bytes(),
        resource_id.as_bytes(),
        client_token.as_bytes(),
    ])
}

// What a call presenting the same `clientToken` with other parameters is refused
// with is stated once for this whole service, beside the replay decision the
// creates take (`super::token_mismatch_message`): an update's retry is refused in
// exactly those words.

impl ComAmazonawsRtbfabricRtbfabricState {
    /// A fresh `updateId`, matching the modelled `^update-[a-z0-9]{1,57}$`.
    pub fn mint_update_id(&mut self) -> String {
        format!("update-{}", self.next_suffix())
    }

    /// Record that `gateway_id` accepted a change of kind `update_type` at `now`,
    /// and answer with the id this service minted for it.
    ///
    /// The change is IN_PROGRESS, and the clock owes it its commit one
    /// [`PROVISIONING_DELAY`] later — the same span every other asynchronous
    /// change of this service is owed — so a caller that lists straight after the
    /// accepting call still sees the change in flight.
    ///
    /// `retained` is the MAXIMUM number of records this gateway's ledger may hold
    /// afterwards, which the accepting call states at its own growth site
    /// ([`UPDATES_RETAINED_PER_GATEWAY`]): this appends one record and then lets go
    /// of that gateway's oldest until the ledger is within it, so the store cannot
    /// grow past `retained` records per gateway however many changes are accepted.
    pub fn record_update(
        &mut self,
        gateway_id: &str,
        update_type: &str,
        change: Change,
        retained: usize,
        now: SystemTime,
    ) -> String {
        let update_id = self.mint_update_id();
        self.updates.push(Update {
            update_id: update_id.clone(),
            gateway_id: gateway_id.to_string(),
            link_id: change.link_id,
            rule_id: change.rule_id,
            update_type: update_type.to_string(),
            status: UPDATE_STATUS_IN_PROGRESS.to_string(),
            created_at: now,
            settles_at: Some(now + PROVISIONING_DELAY),
            token: change.token,
            fingerprint: change.fingerprint,
        });
        self.forget_oldest_updates(gateway_id, retained);
        update_id
    }

    /// The update a call presenting the `@idempotencyToken` digest `token`
    /// started, if the ledger still retains it: what a retry of that call is
    /// answered from.
    pub fn update_for_token(&self, token: &[u8; 32]) -> Option<&Update> {
        self.updates
            .iter()
            .find(|update| update.token.as_ref() == Some(token))
    }

    /// How the call presenting `key` with the parameters `fingerprint` digests is
    /// to be answered. A call carrying no token at all (`None`) is a request of
    /// its own: the SDK generates a token for a caller that states none, so
    /// nothing here is answered from a record that was never written.
    pub fn classify_update(
        &self,
        key: Option<&[u8; 32]>,
        fingerprint: &[u8; 32],
    ) -> UpdateRetry {
        let Some(key) = key else {
            return UpdateRetry::Fresh;
        };
        match self.update_for_token(key) {
            None => UpdateRetry::Fresh,
            Some(update) if update.fingerprint.as_ref() == Some(fingerprint) => {
                UpdateRetry::Replay(update.update_id.clone())
            }
            Some(_) => UpdateRetry::Mismatch,
        }
    }

    /// The updates `gateway_id` has accepted, oldest first — which is the order
    /// their ids were minted in, so it is also the order
    /// [`ListUpdates`](crate::ops::list_updates) pages by.
    pub fn updates_of_gateway(&self, gateway_id: &str) -> Vec<&Update> {
        self.updates
            .iter()
            .filter(|update| update.gateway_id == gateway_id)
            .collect()
    }

    /// Let go of `gateway_id`'s oldest updates until its ledger holds at most
    /// `retained` of them.
    fn forget_oldest_updates(&mut self, gateway_id: &str, retained: usize) {
        let held = self
            .updates
            .iter()
            .filter(|update| update.gateway_id == gateway_id)
            .count();
        let mut excess = held.saturating_sub(retained);
        if excess == 0 {
            return;
        }
        // `retain` visits in order and the ledger is appended to, so the records
        // let go of are that gateway's oldest.
        self.updates.retain(|update| {
            if excess > 0 && update.gateway_id == gateway_id {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Let go of every update a gateway that is gone accepted: an update is a
    /// fact about a resource of this store, so it is released with the gateway it
    /// was accepted on, exactly as that gateway's links and rules are.
    pub fn release_gateway_updates(&mut self, gateway_id: &str) {
        self.updates
            .retain(|update| update.gateway_id != gateway_id);
    }
}
