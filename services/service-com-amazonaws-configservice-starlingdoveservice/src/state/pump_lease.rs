// SPDX-License-Identifier: CC0-1.0

//! The CLAIM an in-process pump holds while it runs the work one store owes.
//!
//! A pump is the in-process task that performs what the clock has made due without a
//! further call from the caller: the recording-and-evaluation passes of a running
//! configuration recorder (`super::recording`). One store runs at most one such pump,
//! which is what the claim here is for.
//!
//! The claim is a LEASE rather than a flag, because a recorder that is recording keeps
//! recording: the work it owes goes on being owed whatever became of the task that was
//! running it. A pump whose task panicked, was dropped with the runtime that spawned it,
//! or is wedged in a call that never answered stops renewing its lease, and the next call
//! that makes work due takes the lease over and starts a fresh pump — so a lost pump
//! cannot be what stops a store's passes for good. [`PumpClaim`] gives the lease up as
//! soon as its pump ends, however it ended.

use std::time::{Duration, SystemTime};

use super::ComAmazonawsConfigserviceStarlingdoveserviceState;

/// How long one pump's lease stands without that pump reaching its next piece of work.
///
/// A pump renews the lease as it takes each one, so a lease this old belongs to a pump
/// that is no longer working. Many of the sub-second spans this service's owed work falls
/// due at, so a pump that is merely slow is never displaced, and short enough that the
/// next call which makes work due takes over rather than waiting on it.
pub const PUMP_LEASE_HOLDS_FOR: Duration = Duration::from_secs(15);

/// The lease on one kind of pump, held by at most one pump at a time.
#[derive(Debug, Clone, Copy, Default)]
pub struct PumpLease {
    /// The lease as it stands: which pump holds it and until when, or `None` while no
    /// pump holds it.
    held: Option<HeldLease>,
    /// How many leases this store has issued, which is where the next pump's id comes
    /// from: an id names ONE pump, so a pump that has been taken over cannot give up or
    /// renew the lease its successor holds.
    issued: u64,
}

/// One pump's hold on a [`PumpLease`].
#[derive(Debug, Clone, Copy)]
struct HeldLease {
    id: u64,
    renew_by: SystemTime,
}

impl PumpLease {
    /// The instant a lease taken or renewed at `now` stands until.
    ///
    /// Computed with a CHECKED add and a lapsed fallback, so a clock near the end of what
    /// an instant can represent leaves the lease free to be taken rather than panicking
    /// inside the primitive that exists to survive a pump's failure.
    fn renew_by(now: SystemTime) -> Option<SystemTime> {
        now.checked_add(PUMP_LEASE_HOLDS_FOR)
    }

    /// Take the lease for a new pump, answering the id that pump renews and gives it up
    /// by — or `None` while a pump that is still renewing it holds it.
    pub fn claim(&mut self, now: SystemTime) -> Option<u64> {
        if self.held.is_some_and(|held| now < held.renew_by) {
            return None;
        }
        self.issued = self.issued.wrapping_add(1);
        let id = self.issued;
        self.held = Self::renew_by(now).map(|renew_by| HeldLease { id, renew_by });
        Some(id)
    }

    /// Renew the lease the pump `id` holds, answering `false` when it no longer holds it
    /// — which is a pump another has taken over, and whose own loop therefore ends.
    pub fn renew(&mut self, id: u64, now: SystemTime) -> bool {
        if self.held.is_none_or(|held| held.id != id) {
            return false;
        }
        self.held = Self::renew_by(now).map(|renew_by| HeldLease { id, renew_by });
        true
    }

    /// Give up the lease the pump `id` holds, leaving a lease another pump has since
    /// taken over where it is.
    pub fn release(&mut self, id: u64) {
        if self.held.is_some_and(|held| held.id == id) {
            self.held = None;
        }
    }
}

/// This store's state, as a pump reaches it.
type Store = ComAmazonawsConfigserviceStarlingdoveserviceState;

/// The lease a RUNNING pump holds, given up when that pump ends.
///
/// Held by the pump's task for as long as it runs, so the lease is given up however the
/// pump ended — a loop that ran out of work, the cap on how much one pump does, a pass
/// that panicked, or a task dropped with the runtime that spawned it. That is what keeps
/// the work a store owes from being wedged behind a pump that is no longer running it.
///
/// Giving the lease up takes the store's guard, so a pump must hold NO guard of its own
/// when its claim is dropped: every critical section of the pump's loop is one statement
/// that releases the guard before the next await. Where that invariant could not hold, the
/// lease lapsing ([`PUMP_LEASE_HOLDS_FOR`]) is what still frees the work.
pub struct PumpClaim {
    state: ::engine::ServiceState<Store>,
    /// Which lease of the store this claim is on.
    lease: fn(&mut Store) -> &mut PumpLease,
    id: u64,
}

impl PumpClaim {
    /// Take `lease` for a new pump, or answer `None` when a pump already holds it.
    pub fn take(
        state: &::engine::ServiceState<Store>,
        lease: fn(&mut Store) -> &mut PumpLease,
        now: SystemTime,
    ) -> Option<Self> {
        let id = lease(&mut state.lock()).claim(now)?;
        Some(Self {
            state: ::engine::ServiceState::clone(state),
            lease,
            id,
        })
    }

    /// The id this pump renews and gives up its lease by.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The store this pump works on.
    pub fn store(&self) -> &::engine::ServiceState<Store> {
        &self.state
    }
}

impl Drop for PumpClaim {
    fn drop(&mut self) {
        (self.lease)(&mut self.state.lock()).release(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lease is held by ONE pump at a time, and a pump that stops renewing it loses it
    /// to the next one — whatever ended it. The rows differ along how long since the
    /// holder last renewed and whether it renewed at all.
    #[test]
    fn a_lease_is_taken_over_only_once_its_holder_stops_renewing_it() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let cases: [(&str, Duration, bool); 5] = [
            ("the instant it was taken", Duration::ZERO, false),
            ("a span inside the lease", PUMP_LEASE_HOLDS_FOR / 2, false),
            (
                "the last instant inside the lease",
                PUMP_LEASE_HOLDS_FOR - Duration::from_millis(1),
                false,
            ),
            ("the instant the lease lapses", PUMP_LEASE_HOLDS_FOR, true),
            (
                "long past the lease",
                PUMP_LEASE_HOLDS_FOR * 10,
                true,
            ),
        ];
        for (case, elapsed, taken_over) in cases {
            let mut lease = PumpLease::default();
            let held = lease
                .claim(start)
                .unwrap_or_else(|| panic!("case {case:?}: a free lease must be taken"));
            let next = lease.claim(start + elapsed);
            assert_eq!(
                next.is_some(),
                taken_over,
                "case {case:?}: expected taken_over {taken_over}, got {next:?}"
            );
            if let Some(next) = next {
                assert_ne!(
                    next, held,
                    "case {case:?}: a pump that took the lease over gets its own id"
                );
                // The pump that lost the lease can neither renew it nor give it away.
                assert!(
                    !lease.renew(held, start + elapsed),
                    "case {case:?}: a displaced pump must not renew the lease"
                );
                lease.release(held);
                assert!(
                    lease.claim(start + elapsed).is_none(),
                    "case {case:?}: a displaced pump must not give up its successor's lease"
                );
            }
        }
    }

    /// Renewing keeps the lease with the pump that holds it; giving it up hands it to the
    /// next pump at once.
    #[test]
    fn a_renewed_lease_stays_held_and_a_released_one_is_free() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut lease = PumpLease::default();
        let held = lease.claim(start).expect("a free lease is taken");
        let renewed_at = start + PUMP_LEASE_HOLDS_FOR / 2;
        assert!(
            lease.renew(held, renewed_at),
            "the holder renews its own lease"
        );
        assert!(
            lease
                .claim(renewed_at + PUMP_LEASE_HOLDS_FOR - Duration::from_millis(1))
                .is_none(),
            "a renewed lease stands for a further span from the renewal"
        );
        lease.release(held);
        assert!(
            lease.claim(renewed_at).is_some(),
            "a lease its holder gave up is taken by the next pump at once"
        );
    }
}
