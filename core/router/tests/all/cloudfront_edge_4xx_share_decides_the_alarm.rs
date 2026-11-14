// SPDX-License-Identifier: CC0-1.0

//! What the SHARE of a distribution's viewer requests answered 4xx does to an alarm over that
//! distribution's `AWS/CloudFront` `4xxErrorRate` — through the WHOLE mounted service set, the
//! one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a metric alarm over a distribution's `4xxErrorRate` is decided by the
//! share of the period's viewer requests the edge answered 4xx, graded against the alarm's own
//! threshold by the comparison it names: a share GREATER than the threshold moves it to ALARM
//! and publishes its SNS alarm action to the subscribed queue, while a share at or below the
//! threshold — the bound `GreaterThanThreshold` excludes included — leaves it `OK` with nothing
//! published.** The share is the one the edge really counted, so the same traffic read back
//! through `GetMetricStatistics` and the state the alarm reaches agree.
//!
//! The sibling [`super::cloudfront_edge_metrics_drive_the_alarm_chain`] suite pins the FIRING
//! leg of that chain and the untouched control beside it — a distribution with no traffic at
//! all — and this suite reuses its wiring to span the THRESHOLD itself: a distribution that
//! really served traffic, and whose alarm must still not fire. Both ends of the rule have to
//! be pinned together, because an emulator that fires on any traffic and one that fires on
//! none are both indistinguishable from the real thing on the firing row alone.
//!
//! The rows differ along what the rule generalises over: the share of the burst answered 4xx —
//! none of it, a share below the threshold, a share exactly AT it, a share just past it and a
//! burst answered 4xx throughout — each row with a distribution, an alarm, a topic and a queue
//! of its own, so what fired and what did not is read where a client reads it rather than
//! inferred from a shared queue.

use std::time::{Duration, Instant};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudfront_edge_metrics_drive_the_alarm_chain::{
    alarm_state, deployed, emulator, given_alarm, given_distribution, given_origin_access_control,
    given_origin_bucket, now_secs, reported_rate, view, ALARM_THRESHOLD, CHAIN_DEADLINE,
    LOOK_AGAIN, ORIGIN_OBJECT_BODY, REGION,
};
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    drain, given_topic_with_subscriber, Grant,
};

/// The two alarm states a row reads: the one a breached threshold moves an alarm to, and the
/// one an unbreached threshold leaves it in under `TreatMissingData: notBreaching`.
const ALARM_STATE: &str = "ALARM";
const OK_STATE: &str = "OK";

/// How many seconds a minute holds, which is the width of the period the alarms grade.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// The latest second of a minute a burst may start in. CloudWatch anchors the windows an alarm
/// grades at multiples of its period, so a burst split across two of them is graded as two
/// shares rather than as the one share it is; starting it before this second leaves the whole
/// burst — milliseconds of in-process requests — inside one window.
const LATEST_BURST_SECOND: f64 = 50.0;

/// How long the wait for that point in the minute may take before the burst is driven anyway:
/// the alignment is a precaution, not the property under test, so it never fails a row.
const ALIGNMENT_DEADLINE: Duration = Duration::from_secs(30);

/// How close a share read back out of CloudWatch has to be to the share the burst really had.
/// Both sides are computed in floating point out of the same integers, so the tolerance only
/// has to absorb that.
const SHARE_TOLERANCE: f64 = 1e-9;

/// One row: how many of the burst's requests ask for a key the origin does not hold (answered
/// 4xx), how many ask for the one it does (answered 200), and whether the share that leaves
/// breaches the alarm's threshold.
struct Row {
    case: &'static str,
    missing: usize,
    present: usize,
    breaches: bool,
}

impl Row {
    /// The share of this row's burst answered 4xx, as the percentage the series carries.
    #[allow(clippy::cast_precision_loss)]
    fn share(&self) -> f64 {
        100.0 * self.missing as f64 / (self.missing + self.present) as f64
    }
}

/// What one row's own wiring is addressed by: the distribution the traffic goes to, the alarm
/// watching it, and the queue its alarm action reaches.
struct Wiring {
    distribution: String,
    domain: String,
    alarm: String,
    queue: String,
}

/// Wait until the clock is early enough in its own minute that the burst cannot straddle a
/// window boundary ([`LATEST_BURST_SECOND`]). Best effort: it looks again until the condition
/// holds or [`ALIGNMENT_DEADLINE`] has passed, and asserts nothing.
async fn within_one_window() {
    let started = Instant::now();
    while now_secs() % SECONDS_PER_MINUTE > LATEST_BURST_SECOND
        && started.elapsed() < ALIGNMENT_DEADLINE
    {
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Drive one row's burst against its own distribution, asserting what each viewer request is
/// answered with.
async fn burst(viewers: &MultiServiceRouter, row: &Row, wiring: &Wiring, missing_key: &str, present_key: &str) {
    let case = row.case;
    for request in 0..row.missing {
        let (status, _served) = view(viewers, &wiring.domain, &format!("/{missing_key}")).await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: request {request} for a missing key must answer 4xx, got {status}"
        );
    }
    for request in 0..row.present {
        let (status, served) = view(viewers, &wiring.domain, &format!("/{present_key}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: request {request} for the present key must answer 200, got {status}"
        );
        assert_eq!(
            served, ORIGIN_OBJECT_BODY,
            "case {case:?}: request {request} must be served the origin object's own bytes"
        );
    }
}

#[tokio::test]
async fn only_a_4xx_share_past_the_threshold_fires_the_alarm_to_its_queue() {
    // The rows span the share of the burst answered 4xx, around the alarm's threshold: the
    // bound itself is a row, because `GreaterThanThreshold` excludes it.
    let rows: [Row; 5] = [
        Row {
            case: "no request answered 4xx",
            missing: 0,
            present: 3,
            breaches: false,
        },
        Row {
            case: "a share below the threshold",
            missing: 1,
            present: 9,
            breaches: false,
        },
        Row {
            case: "a share exactly at the threshold",
            missing: 1,
            present: 4,
            breaches: false,
        },
        Row {
            case: "a share just past the threshold",
            missing: 1,
            present: 3,
            breaches: true,
        },
        Row {
            case: "every request answered 4xx",
            missing: 2,
            present: 0,
            breaches: true,
        },
    ];

    let engine = aws_emulator_router::new_engine();
    // The setup plane carries a credential scope without a computed signature; the viewer plane
    // is the authenticating one the binary serves, and a viewer presents no credential.
    let router = emulator(&engine, false);
    let viewers = emulator(&engine, true);

    // One origin for every row: what differs between rows is the traffic, not the origin.
    let bucket = "edge-4xx-share-origin";
    let present_key = "present.txt";
    let missing_key = "absent.txt";
    given_origin_bucket(&router, "the shared origin", bucket, present_key).await;
    let control = given_origin_access_control(&router, "the shared origin", "edge-4xx-share").await;

    // Each row's own wiring: a distribution, a topic with a subscribed queue, and an alarm over
    // that distribution's 4xx error rate publishing to that topic.
    let mut wirings: Vec<Wiring> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let (distribution, domain) = given_distribution(
            &router,
            row.case,
            &format!("edge-4xx-share-{index}"),
            bucket,
            &control,
        )
        .await;
        let grant = if index % 2 == 0 {
            Grant::Unscoped
        } else {
            Grant::ScopedToTopic
        };
        let (topic, queue) = given_topic_with_subscriber(
            &router,
            row.case,
            REGION,
            &format!("edge-4xx-share-{index}"),
            grant,
        )
        .await;
        let alarm = format!("edge-4xx-share-alarm-{index}");
        given_alarm(&router, row.case, &alarm, &distribution, &topic).await;
        wirings.push(Wiring {
            distribution,
            domain,
            alarm,
            queue,
        });
    }
    for (row, wiring) in rows.iter().zip(&wirings) {
        deployed(&router, row.case, &wiring.distribution).await;
    }

    // The bursts, all of them inside one graded window so each row's share is one share.
    within_one_window().await;
    let at = now_secs();
    for (row, wiring) in rows.iter().zip(&wirings) {
        burst(&viewers, row, wiring, missing_key, present_key).await;
    }

    // What the edge counted for each row is that row's own share, and whether it breaches the
    // threshold is what the rows differ by.
    for (row, wiring) in rows.iter().zip(&wirings) {
        let case = row.case;
        let expected = row.share();
        let reported = reported_rate(&router, case, &wiring.distribution, at).await;
        assert!(
            (reported - expected).abs() < SHARE_TOLERANCE,
            "case {case:?}: expected the edge to report a 4xx rate of {expected}, got {reported}"
        );
        assert_eq!(
            reported > ALARM_THRESHOLD,
            row.breaches,
            "case {case:?}: a share of {reported} against the threshold {ALARM_THRESHOLD} must \
             {} it",
            if row.breaches { "breach" } else { "not breach" }
        );
    }

    // The chain, with no call forcing any of it: every breaching row's action reaches its own
    // queue, and no row whose share does not breach the threshold ever leaves OK — which is
    // checked on every look, not only at the end.
    let started = Instant::now();
    let mut delivered: Vec<Vec<Value>> = rows.iter().map(|_| Vec::new()).collect();
    let mut quiet: Vec<String> = rows.iter().map(|_| String::new()).collect();
    loop {
        for (index, row) in rows.iter().enumerate() {
            if row.breaches {
                if delivered[index].is_empty() {
                    let arrived = drain(&router, row.case, REGION, &wirings[index].queue).await;
                    delivered[index].extend(arrived);
                }
            } else {
                let state = alarm_state(&router, row.case, &wirings[index].alarm).await;
                assert_ne!(
                    state, ALARM_STATE,
                    "case {:?}: a 4xx share of {} must never move the alarm to {ALARM_STATE}",
                    row.case,
                    row.share()
                );
                quiet[index] = state;
            }
        }
        let fired = rows
            .iter()
            .enumerate()
            .all(|(index, row)| !row.breaches || !delivered[index].is_empty());
        let settled = rows
            .iter()
            .enumerate()
            .all(|(index, row)| row.breaches || quiet[index] == OK_STATE);
        if fired && settled {
            break;
        }
        assert!(
            started.elapsed() < CHAIN_DEADLINE,
            "every breaching row must deliver and every other row must report {OK_STATE}: \
             delivered {delivered:?}, states {quiet:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        if row.breaches {
            assert_eq!(
                delivered[index].len(),
                1,
                "case {case:?}: the alarm's action must reach its queue exactly once, got {:?}",
                delivered[index]
            );
            let notification = &delivered[index][0];
            assert_eq!(
                notification["AlarmName"].as_str(),
                Some(wirings[index].alarm.as_str()),
                "case {case:?}: the notification must name the alarm, got {notification}"
            );
            assert_eq!(
                notification["NewStateValue"].as_str(),
                Some(ALARM_STATE),
                "case {case:?}: the notification must carry the state moved to, got \
                 {notification}"
            );
        } else {
            assert_eq!(
                quiet[index], OK_STATE,
                "case {case:?}: a share of {} must leave the alarm {OK_STATE}, got {:?}",
                row.share(),
                quiet[index]
            );
            let withheld = drain(&router, case, REGION, &wirings[index].queue).await;
            assert!(
                withheld.is_empty(),
                "case {case:?}: nothing may be delivered for an alarm that never fired, got \
                 {withheld:?}"
            );
        }
    }
}
