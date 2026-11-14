// SPDX-License-Identifier: CC0-1.0

//! What a REPORTING operation answers with while the clock still owes a change
//! it has already scheduled.
//!
//! Every transient status this service reports — a cluster `creating`, a
//! snapshot `creating` or `deleting`, a replication group `modifying`, a user
//! group `modifying` — is held for a span the emulator chose
//! ([`CLUSTER_TRANSITION`]), stored as the due instant on the record and
//! materialised by whichever later call reads it. The call that STARTS the work
//! answers with the transient status, and a MUTATION arriving inside the span is
//! still refused by the state gate that goes with it, because neither of those
//! waits: they are answered from the record as it stands.
//!
//! A REPORT is different. A client polls a status to learn when the work it
//! started has finished, on an interval sized for the real service (tens of
//! seconds), so a report that answers "still transient" because the span has a
//! few hundred milliseconds left costs that client its whole interval — the
//! emulator settles in half a second and the client discovers it a poll later.
//! [`await_settled`] closes that gap: a reporting operation whose store holds a
//! transition already scheduled to fall due within [`SETTLE_WAIT_CAP`] answers
//! once it HAS fallen due, so the state it reports is the settled one and the
//! first poll a client issues after its call converges. Nothing is reported
//! early: the wait ends at the due instant the record already carried, and the
//! change is then materialised by the operation's own settling pass, as the
//! change the clock owed at the instant the answer was computed.

use std::time::{Duration, SystemTime};

use super::*;

/// The longest a reporting operation waits for the changes the clock has
/// already scheduled.
///
/// One transition span: every span this service starts is
/// [`CLUSTER_TRANSITION`] long ([`SNAPSHOT_TRANSITION`],
/// [`SERVERLESS_CACHE_TRANSITION`], [`USER_DELETION_TRANSITION`],
/// [`UPDATE_ACTION_TRANSITION`] and [`RESERVATION_TRANSITION`] are the same
/// span), so one span is all it can take for the changes outstanding when a
/// report arrives to fall due. It is a CAP and not a sleep: a report with
/// nothing outstanding waits not at all, and one that waits stops at the due
/// instant.
pub const SETTLE_WAIT_CAP: Duration = CLUSTER_TRANSITION;

/// Wait until every change the clock has already scheduled within
/// [`SETTLE_WAIT_CAP`] has fallen due, so the caller's settling pass can
/// materialise it.
///
/// Holds no lock while it waits — the store is taken for the one read of the
/// next due instant and handed straight back — so nothing else this emulator
/// serves is held up by a report that is waiting.
pub async fn await_settled(
    state: &::engine::ServiceState<ComAmazonawsElasticacheAmazonelasticachev9State>,
) {
    let give_up_at = ::tokio::time::Instant::now() + SETTLE_WAIT_CAP;
    loop {
        let now = SystemTime::now();
        // A change already due is materialised by the caller's own settling
        // pass; what is waited for here is one that is not due YET.
        let Some(due) = state.lock().next_scheduled_change(now) else {
            return;
        };
        let Ok(wait) = due.duration_since(now) else {
            return;
        };
        let until = ::tokio::time::Instant::now() + wait;
        if until > give_up_at {
            return;
        }
        ::tokio::time::sleep_until(until).await;
    }
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// Whether the clock owes an RBAC record — a user or a user group — a change at `now`:
    /// a status transition it has reached, or a removal a delete made due.
    ///
    /// Read before settling them, so a caller that settles on a hot path (a RESP session,
    /// which is judged against the membership of the moment) walks the maps only when there
    /// is something to carry through.
    pub fn rbac_change_due(&self, now: SystemTime) -> bool {
        let user_due = self.users.values().any(|user| {
            user.pending_status
                .as_ref()
                .is_some_and(|pending| now >= pending.due)
                || user.pending_removal.is_some_and(|due| now >= due)
        });
        user_due
            || self.user_groups.values().any(|group| {
                group
                    .pending_status
                    .as_ref()
                    .is_some_and(|pending| now >= pending.due)
                    || group.pending_removal.is_some_and(|due| now >= due)
            })
    }

    /// The earliest instant after `now` at which the clock owes this store a
    /// change, or `None` when it owes nothing that has not already fallen due.
    ///
    /// Every record that carries a due instant is read here — the status
    /// transitions and the removals a delete began — so a report cannot answer
    /// a poll from a resource it forgot to wait for. The ones left out are the
    /// changes no span of this service's own owes: a reservation's RETIREMENT
    /// (due at the end of the term its offering sells, years out) and the event
    /// log's expiry (due at the end of the retention the service publishes).
    pub fn next_scheduled_change(&self, now: SystemTime) -> Option<SystemTime> {
        let clusters = self
            .cache_clusters
            .values()
            .filter_map(|cluster| cluster.pending_status.as_ref().map(|pending| pending.due));
        let groups = self
            .replication_groups
            .values()
            .filter_map(|group| group.pending_status.as_ref().map(|pending| pending.due));
        let global_groups = self
            .global_replication_groups
            .values()
            .filter_map(|group| group.pending_status.as_ref().map(|pending| pending.due));
        let snapshots = self.snapshots.values().flat_map(|snapshot| {
            [
                snapshot.pending_status.as_ref().map(|pending| pending.due),
                snapshot.pending_removal,
            ]
        });
        let caches = self.serverless_caches.values().flat_map(|cache| {
            [
                cache.pending_status.as_ref().map(|pending| pending.due),
                cache.pending_deletion,
            ]
        });
        let cache_snapshots = self
            .serverless_cache_snapshots
            .values()
            .filter_map(|snapshot| snapshot.pending_status.as_ref().map(|pending| pending.due));
        let users = self.users.values().flat_map(|user| {
            [
                user.pending_status.as_ref().map(|pending| pending.due),
                user.pending_removal,
            ]
        });
        let user_groups = self.user_groups.values().flat_map(|group| {
            [
                group.pending_status.as_ref().map(|pending| pending.due),
                group.pending_removal,
            ]
        });
        let actions = self
            .update_actions
            .values()
            .filter_map(|action| action.pending_status.as_ref().map(|pending| pending.due));
        let reservations = self
            .reserved_cache_nodes
            .values()
            .filter_map(|node| node.pending_state.as_ref().map(|pending| pending.due));
        clusters
            .chain(groups)
            .chain(global_groups)
            .chain(cache_snapshots)
            .chain(actions)
            .chain(reservations)
            .map(Some)
            .chain(snapshots)
            .chain(caches)
            .chain(users)
            .chain(user_groups)
            .flatten()
            .filter(|due| *due > now)
            .min()
    }
}
