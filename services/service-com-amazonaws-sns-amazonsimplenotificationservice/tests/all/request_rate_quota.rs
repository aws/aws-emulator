// SPDX-License-Identifier: CC0-1.0

//! The published REQUEST RATE an operation's calls are admitted at, driven over the wire
//! through the router the emulator serves in production — so the refusal is observed where a
//! client observes it, in the answer to the call.
//!
//! The RULE asserted here: where Amazon SNS publishes a hard (non-adjustable) per-second
//! request quota for an operation, that account's calls to it in one Region are admitted up to
//! that rate and the excess is REFUSED with the throttling answer of this service's wire
//! protocol — HTTP 400, code `Throttling`, the sentence `Rate exceeded` — while the calls
//! inside the quota are served normally. A refused call performs nothing: it leaves every
//! record of the account exactly as it was, and once the rate has refilled the same call is
//! served again. The quota is per account, per Region and per operation, so another Region's
//! budget is its own and an operation with no published hard quota is not metered at all.
//!
//! The rows differ along what the rule generalises over: how many calls the burst carries
//! (inside the quota, and far above it), WHICH operation is called (the metered one and
//! unmetered siblings), the REGION the call addresses, and whether the call arrives before or
//! after the rate refilled. The recorded case — a burst far above the quota of the SMS
//! sandbox account-level read — is one row.

use std::time::{Duration, Instant};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::request_quota::{
    published_rate, PUBLISHED_REQUEST_RATES, THROTTLE_CODE, THROTTLE_MESSAGE, THROTTLE_STATUS,
};

use super::topics_subscriptions_publish::{Answer, Sns};

/// The operation this suite meters: the one [`PUBLISHED_REQUEST_RATES`] carries a published
/// hard quota for.
const METERED: &str = "GetSMSSandboxAccountStatus";

/// Operations of this service that no hard quota is published for, which must never be
/// refused for their rate: the sibling account-level reads of the same endpoint.
const UNMETERED: &[&str] = &[
    "GetSMSAttributes",
    "ListSMSSandboxPhoneNumbers",
    "ListTopics",
    "ListSubscriptions",
];

/// How long a poll for the refilled rate is given before the suite gives up: far longer than
/// the fraction of a second one call's capacity takes to come back, so a loaded host cannot
/// decide the outcome.
const REFILL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The calls one burst carries: far above the published quota, as the recorded burst is, so
/// no refill over the time the burst takes could admit them all.
const BURST: usize = 150;

/// Whether `answer` is the throttling refusal, asserted whole: the status, the code and the
/// sentence together, since a client branches on all three.
fn assert_throttled(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, THROTTLE_STATUS,
        "case {case:?}: expected HTTP {THROTTLE_STATUS}, got {} / {}",
        answer.status, answer.body
    );
    let code = answer.code();
    assert_eq!(
        code, THROTTLE_CODE,
        "case {case:?}: expected the {THROTTLE_CODE} code, got {code:?} in {}",
        answer.body
    );
    let message = answer.element("Message");
    assert_eq!(
        message.as_deref(),
        Some(THROTTLE_MESSAGE),
        "case {case:?}: expected the throttling sentence, got {message:?}"
    );
    assert!(
        answer.request_id_header.is_some(),
        "case {case:?}: a refusal of this service carries a request id, got {}",
        answer.body
    );
}

/// The membership an answered call to the metered read reports, asserted well formed: the
/// calls inside the quota are served normally while the ones past it are refused.
fn assert_served(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} / {}",
        answer.status, answer.body
    );
    let reported = answer.element("IsInSandbox");
    assert!(
        matches!(reported.as_deref(), Some("true" | "false")),
        "case {case:?}: expected a boolean IsInSandbox, got {reported:?} in {}",
        answer.body
    );
}

/// A burst far above the published quota is answered with a MIX: the calls inside the quota
/// are served, every other one is refused with the throttling answer, and the published rate
/// is the fewest that are served.
#[tokio::test]
async fn a_burst_above_the_published_quota_is_throttled_and_the_rest_is_served() {
    let sns = Sns::new();
    let rate = published_rate(METERED).expect("the metered operation carries a published rate");

    let mut served = 0_u32;
    let mut refused = 0_u32;
    for call in 0..BURST {
        let answer = sns.call(METERED, &[]).await;
        let case = format!("call {call} of a burst of {BURST}");
        if answer.status == 200 {
            assert_served(&case, &answer);
            served += 1;
        } else {
            assert_throttled(&case, &answer);
            refused += 1;
        }
    }
    assert!(
        refused > 0,
        "case \"a burst of {BURST} against a quota of {rate} per second\": expected the excess \
         refused, got {served} served and none refused"
    );
    assert!(
        f64::from(served) >= rate,
        "case \"a burst of {BURST} against a quota of {rate} per second\": expected at least \
         the published second of calls served, got {served}"
    );

    // The refusals performed nothing: the account's own records are exactly as they were.
    let state = sns.state();
    let state = state.lock();
    assert!(
        state.sandbox_phone_numbers.is_empty() && state.sms_settings.attributes.is_empty(),
        "case \"a throttled burst\": a refused call must change nothing, got {:?} / {:?}",
        state.sandbox_phone_numbers,
        state.sms_settings
    );
}

/// Once the rate has refilled, the same call is served again — the refusal is a rate, not a
/// state the account was left in.
#[tokio::test]
async fn a_throttled_account_is_served_again_once_the_rate_refills() {
    let sns = Sns::new();

    // Spend the budget, and keep calling until one call is refused.
    let mut refused = None;
    for call in 0..BURST {
        let answer = sns.call(METERED, &[]).await;
        if answer.status != 200 {
            assert_throttled(&format!("call {call} past the quota"), &answer);
            refused = Some(call);
            break;
        }
    }
    assert!(
        refused.is_some(),
        "case \"spending the whole published budget\": expected a refusal inside {BURST} calls"
    );

    // Then poll for the CONDITION — a call the refilled rate admits — rather than for a
    // time, until the generous deadline passes.
    let deadline = Instant::now() + REFILL_DEADLINE;
    let served = loop {
        let answer = sns.call(METERED, &[]).await;
        if answer.status == 200 {
            break answer;
        }
        assert_throttled("a call before the rate refilled", &answer);
        assert!(
            Instant::now() < deadline,
            "case \"a call once the rate refilled\": the published rate never admitted a call \
             again"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_served("a call once the rate refilled", &served);
}

/// The quota is per Region and per operation: a Region whose budget is untouched serves the
/// same call, and an operation no hard quota is published for is not metered however large
/// the burst.
#[tokio::test]
async fn the_quota_is_held_per_region_and_per_operation() {
    let sns = Sns::new();

    // Spend `us-east-1`'s budget for the metered operation.
    let mut throttled_here = false;
    for call in 0..BURST {
        let answer = sns.call_in_region("us-east-1", METERED, &[]).await;
        if answer.status != 200 {
            assert_throttled(&format!("call {call} in us-east-1"), &answer);
            throttled_here = true;
            break;
        }
    }
    assert!(
        throttled_here,
        "case \"spending one Region's budget\": expected the excess refused"
    );

    // Another Region's budget is its own.
    let elsewhere = sns.call_in_region("eu-west-1", METERED, &[]).await;
    assert_served("the first call in another Region", &elsewhere);

    // And an operation with no published hard quota is never refused for its rate, however
    // many calls arrive at once.
    for operation in UNMETERED {
        assert!(
            published_rate(operation).is_none(),
            "case {operation:?}: no hard quota is published for it, so it must not be metered"
        );
        for call in 0..BURST {
            let answer = sns.call(operation, &[]).await;
            assert_eq!(
                answer.status, 200,
                "case {operation:?} call {call} of {BURST}: an unmetered operation must not be \
                 throttled, got {} / {}",
                answer.status, answer.body
            );
        }
    }

    // Every published rate admits something: a quota of nothing would refuse every call.
    for (operation, rate) in PUBLISHED_REQUEST_RATES {
        assert!(
            *rate > 0.0,
            "case {operation:?}: expected a positive published rate, got {rate}"
        );
    }
}
