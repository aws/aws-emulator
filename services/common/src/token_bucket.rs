// SPDX-License-Identifier: CC0-1.0

//! The ONE token bucket an emulated service meters a caller-driven request rate with.
//!
//! Several AWS services publish a request rate as a PAIR — a steady-state rate and a burst —
//! and document the pair as the token bucket that admits requests: API Gateway's usage-plan
//! `ThrottleSettings` ("`rateLimit` … the API request steady-state rate limit", "`burstLimit`
//! … the API request burst limit", "provided by the token bucket algorithm") and API Gateway
//! v2's per-route `RouteSettings` (`ThrottlingRateLimit` / `ThrottlingBurstLimit`) are the
//! same reading of the same algorithm. Two services asked whether a request is admitted must
//! not answer differently, and a second hand-written copy of the arithmetic is also a second
//! chance to get the refill or the cap wrong, so the bucket lives here once.
//!
//! What stays with the service is everything ABOUT ITS OWN MODEL: which members declare the
//! pair, what an absent or non-positive member means for that resource, and what the thing
//! being metered is keyed by. This module owns only the bucket: how much capacity is left,
//! how it refills, and whether one request may be admitted.
//!
//! Nothing here sleeps or waits: the capacity is computed from how much time has passed
//! since the last admitted request, so a caller that was idle finds its whole burst again
//! and a caller that was not finds what the rate refilled.
//!
//! Two services still carry their own copy of this arithmetic, each pinned by its own
//! suites and each to be MIGRATED onto this module in the change that owns that service
//! rather than copied again:
//! `service-com-amazonaws-apigateway-backplanecontrolservice/src/state.rs` (`ThrottleLimits`
//! + `ThrottleBucket`, metering a usage plan's `ThrottleSettings`) and
//! `service-com-amazonaws-xray-awsxray/src/state.rs` (`RateLimiter`, whose `burst` /
//! `per_second` / `last_refill` are the same bucket over a per-account request budget).

/// The request rate one metered thing is admitted at, as the bucket reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The requests per second the bucket refills at.
    pub rate: f64,
    /// The most requests admissible at once — the bucket's capacity.
    pub burst: f64,
}

/// The capacity one metered thing has left, and the instant that was last computed at.
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    tokens: f64,
    at: ::std::time::Instant,
}

/// The capacity one request takes out of a bucket.
const ONE_REQUEST: f64 = 1.0;

impl Bucket {
    /// A bucket holding `limits.burst` requests as of `now`: a caller that has sent nothing
    /// may burst up to the capacity.
    #[must_use]
    pub fn full(limits: Limits, now: ::std::time::Instant) -> Self {
        Self {
            tokens: limits.burst,
            at: now,
        }
    }

    /// Admit one request and record it, or report that `limits` admit none at `now`.
    ///
    /// The refill is the time since the last request times the rate, capped at the burst:
    /// the bucket never holds more than the capacity however long the caller has been idle,
    /// which is what makes the steady-state rate the rate. The limits are passed per call
    /// rather than held, so limits a resource is UPDATED to take effect on the next request
    /// instead of on a bucket frozen at the old ones.
    pub fn admits(&mut self, limits: Limits, now: ::std::time::Instant) -> bool {
        self.admits_cost(limits, now, ONE_REQUEST)
    }

    /// Admit one request that takes `cost` of the capacity and record it, or report that
    /// `limits` do not admit it at `now`.
    ///
    /// The same bucket as [`Self::admits`] — same refill, same cap — for a rate published
    /// over something other than whole requests: the records or the BYTES one call carries,
    /// where one request spends as much capacity as it brings. `cost` is the caller's: a
    /// cost larger than the burst can never be admitted, so a caller whose metered thing
    /// may exceed the capacity in one request decides what such a request spends before it
    /// asks here.
    pub fn admits_cost(&mut self, limits: Limits, now: ::std::time::Instant, cost: f64) -> bool {
        if self.capacity_at(limits, now) < cost {
            return false;
        }
        self.charge(cost);
        true
    }

    /// The capacity left at `now`, refilled first: the time since the last look times the
    /// rate, capped at the burst, which is what makes the steady-state rate the rate.
    ///
    /// The PEEK half of [`Self::admits_cost`], for a resource bounded by SEVERAL published
    /// rates at once (a Kinesis shard's records per second AND its write bytes per second):
    /// a request such a resource does not admit must spend none of them, so every budget is
    /// asked what it has left before any of them is charged. The limits are passed per call
    /// for the reason [`Self::admits`] gives.
    pub fn capacity_at(&mut self, limits: Limits, now: ::std::time::Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * limits.rate).min(limits.burst);
        self.at = now;
        self.tokens
    }

    /// Spend `cost` of the capacity: the CHARGE half, taken only once every budget of the
    /// resource has answered that it holds what the request brings
    /// ([`Self::capacity_at`]). Charges what it is given, so a caller that has not asked
    /// first can take the bucket below zero.
    pub fn charge(&mut self, cost: f64) {
        self.tokens -= cost;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_bucket_admits_its_burst_and_then_what_the_rate_refilled() {
        let start = Instant::now();
        let limits = Limits {
            rate: 2.0,
            burst: 2.0,
        };
        let mut bucket = Bucket::full(limits, start);
        // Rows differ along WHEN the request arrives: inside the burst, past it, and after
        // enough time for the rate to refill one request's capacity.
        let cases: [(&str, Duration, bool); 5] = [
            ("the first request of the burst", Duration::ZERO, true),
            ("the second request of the burst", Duration::ZERO, true),
            ("one past the burst", Duration::ZERO, false),
            ("still past it a moment later", Duration::from_millis(100), false),
            ("once the rate refilled a request", Duration::from_millis(600), true),
        ];
        for (case, elapsed, expected) in cases {
            let actual = bucket.admits(limits, start + elapsed);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admitted {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_bucket_never_holds_more_than_its_burst_and_one_with_none_admits_nothing() {
        let start = Instant::now();
        // However long a caller has been idle, the capacity is the burst: three requests
        // after an hour of silence, where the burst is two, is one too many.
        let limits = Limits {
            rate: 1_000.0,
            burst: 2.0,
        };
        let mut bucket = Bucket::full(limits, start);
        let idle = start + Duration::from_secs(3_600);
        let admitted = [
            bucket.admits(limits, idle),
            bucket.admits(limits, idle),
            bucket.admits(limits, idle),
        ];
        assert_eq!(
            admitted,
            [true, true, false],
            "case {:?}: expected the burst and no more, got {admitted:?}",
            "three requests after an hour of silence"
        );
        // A bucket with no capacity admits nothing, however long it is left.
        let empty = Limits {
            rate: 0.0,
            burst: 0.0,
        };
        let mut none = Bucket::full(empty, start);
        for (case, elapsed) in [
            ("the first request", Duration::ZERO),
            ("a request a minute later", Duration::from_secs(60)),
        ] {
            let actual = none.admits(empty, start + elapsed);
            assert!(
                !actual,
                "case {case:?}: a bucket with no capacity must admit nothing, got {actual}"
            );
        }
    }

    #[test]
    fn a_request_spends_the_cost_it_brings_and_one_past_the_burst_is_never_admitted() {
        let start = Instant::now();
        let limits = Limits {
            rate: 1_000.0,
            burst: 1_000.0,
        };
        let mut bucket = Bucket::full(limits, start);
        // Rows differ along the COST one request brings, against the capacity left when it
        // arrives: a cost inside the capacity, the cost that spends exactly the rest of it,
        // one past what is left, what the rate refilled a moment later, and a cost no full
        // bucket could ever hold.
        let cases: [(&str, Duration, f64, bool); 6] = [
            ("a cost well inside the burst", Duration::ZERO, 400.0, true),
            ("the cost that spends the rest", Duration::ZERO, 600.0, true),
            ("one unit past an empty bucket", Duration::ZERO, 1.0, false),
            ("what the rate refilled", Duration::from_millis(100), 100.0, true),
            ("more than the refill", Duration::from_millis(100), 101.0, false),
            ("a cost past the whole burst", Duration::from_secs(60), 1_001.0, false),
        ];
        for (case, elapsed, cost, expected) in cases {
            let actual = bucket.admits_cost(limits, start + elapsed, cost);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admitted {expected} for cost {cost}, got {actual}"
            );
        }
    }

    /// The peek-then-charge split: a resource bounded by SEVERAL published rates asks every
    /// budget what it holds before any of them is charged, so a request one of them refuses
    /// spends none.
    #[test]
    fn a_resource_bounded_by_two_rates_charges_neither_budget_when_one_refuses() {
        let start = Instant::now();
        let records = Limits {
            rate: 1_000.0,
            burst: 1_000.0,
        };
        let bytes = Limits {
            rate: 1_024.0,
            burst: 1_024.0,
        };
        let mut record_budget = Bucket::full(records, start);
        let mut byte_budget = Bucket::full(bytes, start);

        // One request past what the BYTE budget holds: it is refused, and the record budget
        // it never used keeps its whole capacity.
        let too_many_bytes = 2_048.0;
        let admitted = record_budget.capacity_at(records, start) >= 1.0
            && byte_budget.capacity_at(bytes, start) >= too_many_bytes;
        assert!(
            !admitted,
            "case a request past one of two budgets: expected it refused"
        );
        assert_eq!(
            record_budget.capacity_at(records, start),
            1_000.0,
            "case a request past one of two budgets: the budget that did not refuse it keeps \
             its capacity"
        );

        // The same request inside both budgets charges both.
        assert!(
            record_budget.capacity_at(records, start) >= 1.0
                && byte_budget.capacity_at(bytes, start) >= 512.0,
            "case a request inside both budgets: expected it admitted"
        );
        record_budget.charge(1.0);
        byte_budget.charge(512.0);
        assert_eq!(
            (
                record_budget.capacity_at(records, start),
                byte_budget.capacity_at(bytes, start)
            ),
            (999.0, 512.0),
            "case a request inside both budgets: expected both charged what it brought"
        );
    }
}
