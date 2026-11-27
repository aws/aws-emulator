// SPDX-License-Identifier: CC0-1.0

//! The window an ACCEPTED payment manager deletion is under way in: the manager stays
//! readable in `DELETING` until the removal the clock owes it comes due, and only then
//! is the record gone.
//!
//! `DeletePaymentManager` answers 202 carrying `status` (`PaymentManagerStatus`) and
//! "initiates the deletion process asynchronously"
//! (`smithy_query doc shape=DeletePaymentManager`), so the call ACCEPTS the deletion, it
//! does not carry it out. What a client observes in that window is the point — a manager
//! still present and reporting `DELETING`, a change asked of it refused as the conflict
//! it is, and a read answering the modelled 404 only once the removal is due — because
//! that is how a client tells "busy, retry later" from "gone, stop".
//!
//! The removal is owed by the CLOCK: it was due at that instant whether or not anybody
//! called, so every operation that reads the payments plane materialises it first
//! ([`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_payment_managers`])
//! and nothing ever sleeps for it. This is the same shape the agent runtime, the policy
//! engine and the consent portal families already record a deletion in; it lives here
//! rather than in any one `ops/` file so the delete, the guards and every read of a
//! payment manager cannot drift apart.

use super::*;

use crate::ops::types::DeletePaymentManagerOutput;

/// How long an accepted payment manager deletion stays under way — the span the manager
/// reports `DELETING` for before the clock owes its removal.
///
/// AWS documents no duration for it, so the span is chosen: the one the payments family
/// already gives a clock-owed transition ([`PAYMENT_MANAGER_SETTLE_DELAY`]), long enough
/// that a caller acting straight after the delete still observes the `DELETING` state,
/// short enough to keep a client script quick. Stored as an instant on the record and
/// never slept on.
pub const PAYMENT_MANAGER_REMOVAL_DELAY: Duration = PAYMENT_MANAGER_SETTLE_DELAY;

/// How this service words the refusal of a `DeletePaymentManager` for a manager that
/// still holds payment connectors: what the manager still contains, counted off this
/// service's own state, and the fact that it cannot be deleted like that.
///
/// "All payment connectors associated with the payment manager must be deleted before
/// the payment manager can be deleted" (`smithy_query doc shape=DeletePaymentManager`),
/// so a connector is never silently cascaded away with the manager that holds it. The
/// text around the count is fixed; the count is the number of connectors the manager
/// still holds. Worded as the sibling refusal of a parent that still holds children
/// ([`policy_engine_has_policies`]), so one service does not report the same precondition
/// in two sentences.
pub fn payment_manager_has_connectors(payment_connectors: usize) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Payment manager still contains {payment_connectors} payment connector and cannot \
             be deleted"
        )),
    }
}

/// The modelled conflict for a write asked of a payment manager whose own deletion has
/// been accepted: the manager is going away, so what the write would change goes with
/// it.
///
/// The sentence is fixed and states what the manager is doing; its variable parts are
/// the id the request's path carried and the status as the enum spells it — the same
/// token every response reports a status in. Worded as its sibling
/// [`payment_connector_status_conflict`] words a write refused for the status of the
/// resource it addressed, minus that sentence's retry clause: a deletion does not
/// settle back into a status the write could be retried under.
pub fn payment_manager_deleting_conflict(payment_manager_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Payment manager {payment_manager_id} is in {PAYMENT_MANAGER_STATUS_DELETING} \
             status and cannot be modified."
        )),
    }
}

impl PaymentManagerRecord {
    /// Whether this manager's own deletion has been accepted and is still under way.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the clock has already made this manager's removal due at `now`, which is what
    /// [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_payment_managers`]
    /// materialises.
    ///
    /// Read on its own by a caller that must not reclaim anything — the anonymous
    /// authorization page a connector publishes — so that page reports a manager the clock
    /// has already removed as gone without writing to the store.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|at| now >= at)
    }

    /// Accept this manager's deletion at `now`: it reports `DELETING` from this instant
    /// on ([`Self::status_at`]) and the clock owes its removal
    /// [`PAYMENT_MANAGER_REMOVAL_DELAY`] later.
    ///
    /// Idempotent, as `DeletePaymentManager` is (`@idempotent`): a repeat while the
    /// deletion is still under way leaves the record and the instant its removal is owed
    /// alone rather than pushing that removal further out.
    pub fn accept_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.removal_due_at = Some(now + PAYMENT_MANAGER_REMOVAL_DELAY);
        self.updated_at = now;
    }

    /// This payment manager as `DeletePaymentManager`'s response envelope: the id the
    /// request addressed it by and the status it reports now the deletion is accepted,
    /// read off the same record every other response renders from.
    pub fn delete_output(&self, now: SystemTime) -> DeletePaymentManagerOutput {
        DeletePaymentManagerOutput {
            payment_manager_id: Some(self.payment_manager_id.clone()),
            status: self.status_at(now).to_string(),
        }
    }
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Remove every payment manager whose deletion the clock has already made due, with
    /// the connectors held on it and the idempotency records naming it.
    ///
    /// Called by every operation that reads the payments plane, before it reads: the
    /// removal was owed at this instant whether or not anybody called, so a caller that
    /// polls the manager sees `DELETING` and then the modelled 404 without anything
    /// sleeping for it.
    pub fn expire_deleted_payment_managers(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .payment_managers
            .iter()
            .filter(|(_, record)| record.removal_due(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            // The connectors and the per-manager client tokens are held ON the record,
            // so they go with it; the account-wide manager tokens are not.
            self.payment_managers.remove(&id);
            self.forget_payment_manager_client_tokens_for(&id);
        }
    }

    /// Drop the idempotency records naming a payment manager that no longer exists: such
    /// a record can never replay anything again, and nothing else would ever reclaim it.
    fn forget_payment_manager_client_tokens_for(&mut self, payment_manager_id: &str) {
        self.payment_manager_idempotency
            .retain(|_, id| id != payment_manager_id);
        let live = &self.payment_manager_idempotency;
        self.payment_manager_idempotency_order
            .retain(|token| live.contains_key(token));
    }
}
