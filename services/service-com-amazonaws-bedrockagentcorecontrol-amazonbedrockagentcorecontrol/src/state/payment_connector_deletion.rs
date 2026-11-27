// SPDX-License-Identifier: CC0-1.0

//! The window an ACCEPTED payment connector deletion is under way in: the connector
//! stays readable in `DELETING` until the removal the clock owes it comes due, and only
//! then is the record gone.
//!
//! `DeletePaymentConnector` answers 202 carrying `status`, "The current status of the
//! payment connector, set to DELETING when deletion is initiated"
//! (`smithy_query doc shape=DeletePaymentConnectorResponse member=status`), so the call
//! ACCEPTS the deletion, it does not carry it out. What a client observes in that window
//! is the point — a connector still present and reporting `DELETING`, a change asked of
//! it refused as the conflict it is, and a read answering the modelled 404 only once the
//! removal is due — because that is how a client tells "busy, retry later" from "gone,
//! stop".
//!
//! The removal is owed by the CLOCK: it was due at that instant whether or not anybody
//! called, so every operation that reads the payments plane materialises it first
//! ([`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_payment_connectors`])
//! and nothing ever sleeps for it. The same shape the manager that holds the connectors
//! records its own accepted deletion in ([`super::payment_manager_deletion`]); it lives
//! here rather than in any one `ops/` file so the delete, the guards and every read of a
//! payment connector cannot drift apart.

use super::*;

use crate::ops::types::DeletePaymentConnectorOutput;

/// How long an accepted payment connector deletion stays under way — the span the
/// connector reports `DELETING` for before the clock owes its removal.
///
/// AWS documents no duration for it, so the span is chosen: the one the payments family
/// already gives a clock-owed transition ([`PAYMENT_CONNECTOR_SETTLE_DELAY`]), long
/// enough that a caller acting straight after the delete still observes the `DELETING`
/// state, short enough to keep a client script quick. Stored as an instant on the record
/// and never slept on.
pub const PAYMENT_CONNECTOR_REMOVAL_DELAY: Duration = PAYMENT_CONNECTOR_SETTLE_DELAY;

/// The modelled conflict for a write asked of a payment connector whose own deletion has
/// been accepted: the connector is going away, so what the write would change goes with
/// it.
///
/// The sentence is fixed and states what the connector is doing; its variable parts are
/// the id the request's path carried and the status as the enum spells it — the same
/// token every response reports a status in. Worded as its sibling
/// [`payment_manager_deleting_conflict`] words the same refusal for the manager that
/// holds the connector, so one plane does not report a resource going away in two
/// sentences.
pub fn payment_connector_deleting_conflict(payment_connector_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Payment connector {payment_connector_id} is in {PAYMENT_CONNECTOR_STATUS_DELETING} \
             status and cannot be modified."
        )),
    }
}

impl PaymentConnectorRecord {
    /// Whether this connector's own deletion has been accepted and is still under way.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has already made this connector's removal due at `now`, which
    /// is what [`PaymentConnectorStore::expire_deleted`] materialises.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|at| now >= at)
    }

    /// Accept this connector's deletion at `now`: it reports `DELETING` from this instant
    /// on ([`Self::status_at`]) and the clock owes its removal
    /// [`PAYMENT_CONNECTOR_REMOVAL_DELAY`] later.
    ///
    /// Idempotent, as `DeletePaymentConnector` is (`@idempotent`): a repeat while the
    /// deletion is still under way leaves the record and the instant its removal is owed
    /// alone rather than pushing that removal further out.
    pub fn accept_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.removal_due_at = Some(now + PAYMENT_CONNECTOR_REMOVAL_DELAY);
        self.updated_at = now;
    }

    /// This payment connector as `DeletePaymentConnector`'s response envelope: the id the
    /// request addressed it by and the status it reports now the deletion is accepted,
    /// read off the same record every other response renders from.
    pub fn delete_output(&self, now: SystemTime) -> DeletePaymentConnectorOutput {
        DeletePaymentConnectorOutput {
            payment_connector_id: Some(self.payment_connector_id.clone()),
            status: self.status_at(now).to_string(),
        }
    }
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Materialise every payment connector removal the clock has already made due, on
    /// every payment manager this account holds.
    ///
    /// Called by every operation that reads a payment connector, before it reads: the
    /// removal was owed at this instant whether or not anybody called, so a caller that
    /// polls the connector sees `DELETING` and then the modelled 404 without anything
    /// sleeping for it.
    pub fn expire_deleted_payment_connectors(&mut self, now: SystemTime) {
        for manager in self.payment_managers.values_mut() {
            manager.payment_connectors.expire_deleted(now);
        }
    }
}
