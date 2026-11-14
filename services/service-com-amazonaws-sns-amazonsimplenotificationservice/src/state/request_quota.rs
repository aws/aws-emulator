// SPDX-License-Identifier: CC0-1.0

//! The REQUEST RATE one account's calls to an operation are admitted at, and what Amazon SNS
//! answers the calls past it with.
//!
//! Amazon SNS publishes a request quota per operation, per account and per Region, and the
//! ones it marks non-adjustable are enforced whatever the account asks for: a burst above
//! such a quota is not served slowly, it is REFUSED, and a client's retry and backoff path
//! exists for exactly that answer. So the rate is part of the operation's contract, not of
//! its capacity.
//!
//! Three pieces:
//!
//! 1. **which operations are metered, and at what rate** — [`PUBLISHED_REQUEST_RATES`], the
//!    quotas the SNS Developer Guide publishes as hard, per-second, per-account and
//!    per-Region limits. An operation absent from it is not metered here, because no rate for
//!    it is published to meter it at; a rate is never invented for one.
//! 2. **what each account has left** — [`RequestRates`], one
//!    [`::service_common::token_bucket::Bucket`] per metered operation and Region, which is
//!    the tree's one reading of a published rate. Nothing here sleeps: the capacity is
//!    computed from the time since the last admitted call, so a caller that paused finds its
//!    budget again.
//! 3. **the refusal** — [`THROTTLE_CODE`], [`THROTTLE_STATUS`] and [`THROTTLE_MESSAGE`]: the
//!    throttling answer of this service's wire protocol. A refused call performs nothing, so
//!    it leaves every record of the account exactly as it was and a later call inside the
//!    quota reads back what it read before.
//!
//! The gate runs in front of the operations rather than inside each of them
//! ([`crate::service`]), because a quota is a property of the ENDPOINT a request reached and
//! not of any one handler's work.

use std::collections::HashMap;
use std::time::Instant;

use ::service_common::token_bucket::{Bucket, Limits};

/// The error code Amazon SNS refuses a call past a published request quota with, as its
/// awsQuery error document spells it. Recorded evidence: a burst above the quota of an SMS
/// sandbox read is answered with this code at [`THROTTLE_STATUS`], the throttling error class
/// a generated client retries on — the modelled `ThrottledException` is the same class as
/// SNS's own shape for it.
pub const THROTTLE_CODE: &str = "Throttling";

/// The HTTP status that refusal carries, from the same recorded evidence as
/// [`THROTTLE_CODE`].
pub const THROTTLE_STATUS: u16 = 400;

/// The sentence that refusal states: that the caller's request rate was above the quota, and
/// nothing about the account. Fixed text — the quota it exceeded is published rather than
/// stated per call.
pub const THROTTLE_MESSAGE: &str = "Rate exceeded";

/// The requests per second Amazon SNS admits per account and Region, for each operation whose
/// quota it publishes as a HARD (non-adjustable) one: `GetSMSSandboxAccountStatus`, the SMS
/// sandbox account-level read, at ten requests per second.
///
/// The operations whose published quotas are adjustable, or published only as a far higher
/// regional rate, are deliberately absent: metering one of those here would refuse a call AWS
/// serves. An entry is added only with the rate the quota table publishes for that operation.
pub const PUBLISHED_REQUEST_RATES: &[(&str, f64)] = &[("GetSMSSandboxAccountStatus", 10.0)];

/// The most buckets one account's record keeps at once.
///
/// A bucket is keyed by the Region the request addressed, which the caller chooses, so the map
/// is bounded like every other store of this service: at the ceiling, the buckets that have
/// refilled to their whole capacity are dropped first, which changes no decision — a caller
/// with its whole burst left is in exactly the position a caller with no bucket is.
const MAX_METERED_BUDGETS: usize = 256;

/// The per-second rate `operation` is metered at, or `None` when no hard quota is published
/// for it — which is every operation [`PUBLISHED_REQUEST_RATES`] does not name.
#[must_use]
pub fn published_rate(operation: &str) -> Option<f64> {
    PUBLISHED_REQUEST_RATES
        .iter()
        .find(|(name, _)| *name == operation)
        .map(|(_, rate)| *rate)
}

/// The limits a published per-second rate admits requests under: the rate itself, and one
/// second's worth of it as the burst — the quota is published per second, so that is what a
/// caller may spend at once before the rate is what governs.
fn limits(rate: f64) -> Limits {
    Limits {
        rate,
        burst: rate,
    }
}

/// What each metered operation has left to spend, per Region — the account's own request
/// budgets.
#[derive(Debug, Default)]
pub struct RequestRates {
    /// One budget per (Region, operation) a metered call has arrived for. At most
    /// [`MAX_METERED_BUDGETS`].
    budgets: HashMap<(String, String), Bucket>,
}

impl RequestRates {
    /// Admit one call to `operation` in `region` at `now` and spend its capacity, or report
    /// that the published quota does not admit it.
    ///
    /// `rate` is that operation's published rate ([`published_rate`]), passed in so the
    /// budget is metered at the quota in force on the call rather than at one frozen when the
    /// budget was first taken.
    pub fn admits(&mut self, operation: &str, region: &str, rate: f64, now: Instant) -> bool {
        let limits = limits(rate);
        let key = (region.to_string(), operation.to_string());
        if let Some(bucket) = self.budgets.get_mut(&key) {
            return bucket.admits(limits, now);
        }
        if self.budgets.len() >= MAX_METERED_BUDGETS {
            // Only the budgets still being spent are worth keeping; a full one decides
            // nothing a fresh one would not.
            self.budgets
                .retain(|_, bucket| bucket.capacity_at(limits, now) < limits.burst);
        }
        let mut bucket = Bucket::full(limits, now);
        let admitted = bucket.admits(limits, now);
        if self.budgets.len() < MAX_METERED_BUDGETS {
            self.budgets.insert(key, bucket);
        }
        admitted
    }
}

/// Whether the call to `operation` that `region` reached is past this account's published
/// quota for it, and so refused with [`THROTTLE_CODE`].
///
/// `false` for every operation no hard quota is published for, and for an engine carrying no
/// store of this service: a budget that cannot be read refuses nothing, because refusing a
/// call AWS serves is the worse answer.
///
/// Call it with this service's own store NOT locked: the store is taken here.
#[must_use]
pub fn is_throttled<Eng: Send + Sync + 'static>(
    engine: &Eng,
    operation: &str,
    region: &str,
) -> bool {
    let Some(rate) = published_rate(operation) else {
        return false;
    };
    let Some(store) = super::service_state(engine) else {
        return false;
    };
    let mut store = store.lock();
    !store
        .request_rates
        .admits(operation, region, rate, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// RULE: a metered operation admits one second's worth of its published rate at once and
    /// then what the rate refilled, per Region and per operation.
    #[test]
    fn a_published_rate_admits_its_second_and_then_refills() {
        let start = Instant::now();
        let mut rates = RequestRates::default();
        let operation = "GetSMSSandboxAccountStatus";
        let rate = published_rate(operation).expect("the operation carries a published rate");

        // How many calls arriving at one instant are admitted: one second's worth of the
        // published rate, and the next one is refused.
        let mut admitted: u32 = 0;
        for _ in 0..100 {
            if !rates.admits(operation, "us-east-1", rate, start) {
                break;
            }
            admitted += 1;
        }
        assert_eq!(
            f64::from(admitted),
            rate,
            "case \"calls arriving at one instant\": expected one second of the published rate, \
             got {admitted}"
        );
        // Another Region is another budget: the quota is published per Region.
        assert!(
            rates.admits(operation, "eu-west-1", rate, start),
            "case \"the first call in another Region\": expected it admitted"
        );
        // And the rate refills: one call's capacity returns after one call's worth of time.
        let refilled = start + Duration::from_secs_f64(1.0 / rate);
        assert!(
            rates.admits(operation, "us-east-1", rate, refilled),
            "case \"a call once the rate refilled one\": expected it admitted"
        );
    }

    /// RULE: only the operations with a published hard quota are metered at all.
    #[test]
    fn an_operation_with_no_published_quota_is_not_metered() {
        for operation in ["GetSMSAttributes", "ListTopics", "Publish", "Subscribe"] {
            let rate = published_rate(operation);
            assert!(
                rate.is_none(),
                "case {operation:?}: expected no published hard quota, got {rate:?}"
            );
        }
        for (operation, rate) in PUBLISHED_REQUEST_RATES {
            assert!(
                *rate > 0.0,
                "case {operation:?}: a published rate must admit something, got {rate}"
            );
        }
    }
}
