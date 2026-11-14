// SPDX-License-Identifier: CC0-1.0

//! RESTARTING a broker, and the user changes a restart applies.
//!
//! RULE: a broker accepts a restart only while it is RUNNING; the restart puts it in
//! `REBOOT_IN_PROGRESS` until the clock makes its return to `RUNNING` due, and completing
//! it is what applies the user changes staged on the broker since the last one — a
//! staged addition or modification becomes the user's settled state, and a staged
//! deletion drops the user.
//!
//! `RebootBroker` is documented "Reboots a broker. Note: This API is asynchronous", so
//! the broker it restarts is left in a non-terminal state and the move back to `RUNNING`
//! is owed by the CLOCK, not by any later call: the due instant is stored
//! ([`Broker::restart_completes_at`]) and nothing sleeps for it. The first operation to
//! resolve the broker after that instant materialises what the restart owed
//! ([`Broker::apply_due_changes`], reached through
//! [`super::ComAmazonawsMqMqState::settled_broker`]), so the answer a caller gets is the
//! one it would have got had nobody called at all.
//!
//! A user change is pending until the restart applies it, which is what
//! `UserPendingChanges$PendingChange` reports ("Required. The type of change pending for
//! the ActiveMQ user"): Amazon MQ's user changes take effect on the broker's next
//! reboot, so until then the user carries its [`ChangeType`](super::CHANGE_TYPE_CREATE)
//! and the broker still reports it.

use super::*;

/// `BrokerState` while a broker is restarting. From `BrokerState`'s modelled values
/// (`BROKER_STATE_VALUES`).
pub const BROKER_STATE_REBOOT_IN_PROGRESS: &str = "REBOOT_IN_PROGRESS";

/// How long after a restart is asked for the broker stops reporting
/// `REBOOT_IN_PROGRESS` and reports `RUNNING` again.
///
/// A real reboot takes minutes. The span this emulator gives one is short enough to
/// test and long enough that a caller acting straight after the call still observes the
/// transient state — and the refusals that go with it — rather than a broker that was
/// never anything but RUNNING. Stored as the instant it comes due on the broker
/// ([`Broker::restart_completes_at`]); nothing sleeps for it.
pub const BROKER_RESTART: Duration = Duration::from_secs(3);

impl Broker {
    /// Whether this broker is in the middle of a restart as of `now`.
    pub fn restarting(&self, now: SystemTime) -> bool {
        self.restart_completes_at.is_some_and(|due| now < due)
    }
    /// Start a restart at `now`: the broker reports `REBOOT_IN_PROGRESS` until the span
    /// comes due, and what it owes its users is applied when it does.
    pub fn restart(&mut self, now: SystemTime) {
        self.restart_completes_at = Some(now + BROKER_RESTART);
    }

    /// Apply what the clock has already made due to this broker: a restart whose span
    /// has passed, and with it every user change that restart was to apply.
    ///
    /// Does nothing when no restart is in flight or the running one is not due yet, so
    /// calling it on every resolve of a broker costs one comparison and changes nothing
    /// a caller had not already been owed.
    pub fn apply_due_changes(&mut self, now: SystemTime) {
        if !self.restart_completes_at.is_some_and(|due| now >= due) {
            return;
        }
        self.restart_completes_at = None;
        // A user the broker was to drop is gone once the restart has applied it; every
        // other staged change becomes that user's settled state, so nothing is pending
        // any more.
        self.users
            .retain(|_, user| user.pending_change.as_deref() != Some(CHANGE_TYPE_DELETE));
        for user in self.users.values_mut() {
            user.pending_change = None;
        }
    }
}

/// How Amazon MQ refuses a restart of a broker that is not in a state it can be
/// restarted from.
///
/// The sentence is fixed; the broker's own NAME and the `BrokerState` it is in are its
/// only variable parts, both read from the stored broker.
pub fn broker_not_restartable_message(broker_name: &str, broker_state: &str) -> String {
    format!("Broker [{broker_name}] can't be restarted while in state [{broker_state}]")
}
