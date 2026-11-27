// SPDX-License-Identifier: CC0-1.0

//! The RESOLUTION an Amazon RDS event's date is published and reported at: whole
//! milliseconds.
//!
//! RULE: the instant a lifecycle change happened is carried as RFC 3339 UTC text whose
//! fractional-second field is exactly three digits wide — `2018-10-06T12:26:13.882Z` — in the
//! `detail.Date` of the envelope the change is published as, and the `Event$Date` the control
//! plane reports for the same record names that same instant. Nothing finer than a
//! millisecond is observable of an event, whatever resolution the clock the change was timed
//! against offers.
//!
//! Why a client cares: the shape of every "order a change, then wait for its event" flow is
//! to note when the ordering call was answered and then poll for an event dated at or after
//! that moment, which means PARSING `detail.Date`. A field whose width follows whatever
//! sub-second digits the host clock happened to produce is a field a client's fixed date-time
//! format cannot read — a reader built for the three digits every Amazon RDS event carries
//! rejects a nine-digit one, and discards the event it was waiting for. The same instant is
//! also read back from the control plane, so the two readings of ONE record must agree.
//!
//! The table spans what the rule generalises over: the three DB instance changes this service
//! answers before performing them, each published under its own event identifier and matched
//! by a rule of its own, plus the creation of the instance itself — four records, each timed
//! against a different reading of the clock. EventBridge and Amazon SQS are mounted beside
//! this service on ONE engine exactly as the assembled router mounts them, so every date
//! asserted here is read off a message a queue really handed over; every wait is a poll on the
//! arrival under a generous deadline.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_bus_events::{
    collect, emulator, wait_for_status, DB_INSTANCE_EVENT, EVENT_SOURCE,
};
use crate::db_instance_event_pattern_precision::{create_available, rule_to_its_own_queue};
use crate::db_instances::{call, expect_ok, subtrees, text};

/// The DB instance every change below is made to.
const INSTANCE: &str = "emu-dated-db";

/// Digits in the fractional-second field of an Amazon RDS event date.
///
/// Source: the `Date` of a published Amazon RDS event detail names its instant with a
/// three-digit fraction of a second (`"Date": "2018-10-06T12:26:13.882Z"`). Stated here
/// rather than imported from the service crate, so a wrong width in the implementation
/// cannot make this suite pass.
const FRACTION_DIGITS: usize = 3;

/// One change this suite drives, and the event identifier it is published under.
struct Dated {
    case: &'static str,
    /// The operation that orders it, or `None` for the creation of the instance itself,
    /// which [`create_available`] performs.
    action: Option<&'static str>,
    /// `detail.EventID`, which this row's rule pattern pins.
    ///
    /// Source: the per-source-type tables of "Amazon RDS event categories and event
    /// messages" (Amazon RDS User Guide) — the DB instance rows for the creation, the
    /// restart, the stop and the start of an instance.
    event_id: &'static str,
    /// `detail.Message` of the same row, which the control-plane read narrows by.
    message: &'static str,
    /// The name of this row's rule and of the queue it targets.
    slug: &'static str,
    /// The status the instance reports once the change has happened, so the next row's
    /// change is accepted.
    settled_status: &'static str,
}

/// The four records, in an order each of which the next one is accepted from.
const DATED: &[Dated] = &[
    Dated {
        case: "the creation of the instance",
        action: None,
        event_id: "RDS-EVENT-0005",
        message: "DB instance created",
        slug: "emu-dated-created",
        settled_status: "available",
    },
    Dated {
        case: "the restart a reboot's momentary outage ends with",
        action: Some("RebootDBInstance"),
        event_id: "RDS-EVENT-0006",
        message: "DB instance restarted",
        slug: "emu-dated-restarted",
        settled_status: "available",
    },
    Dated {
        case: "the stop that leaves the instance stopped",
        action: Some("StopDBInstance"),
        event_id: "RDS-EVENT-0087",
        message: "DB instance stopped",
        slug: "emu-dated-stopped",
        settled_status: "stopped",
    },
    Dated {
        case: "the start that brings it back into service",
        action: Some("StartDBInstance"),
        event_id: "RDS-EVENT-0088",
        message: "DB instance started",
        slug: "emu-dated-started",
        settled_status: "available",
    },
];

/// The pattern that matches this row's change for the instance at `arn` exactly.
fn pattern_of(row: &Dated, arn: &str) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [DB_INSTANCE_EVENT],
        "detail": { "EventID": [row.event_id], "SourceArn": [arn] },
    })
}

/// The instant `text` names, read with the same reader an SDK reads a date-time member with.
fn instant_of(case: &str, text: &str) -> f64 {
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {text:?} must be an RFC 3339 instant, got {error}")
        })
        .as_secs_f64()
}

/// Assert `dated` is RFC 3339 UTC text whose fractional-second field is exactly
/// [`FRACTION_DIGITS`] digits wide — the form a client's own date-time reader is built for.
fn assert_millisecond_precision(case: &str, dated: &str) {
    let instant = dated.strip_suffix('Z').unwrap_or_else(|| {
        panic!("case {case:?}: expected a UTC instant ending in Z, got {dated:?}")
    });
    let (whole, fraction) = instant.split_once('.').unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a fractional-second field of {FRACTION_DIGITS} digits, \
             got {dated:?}"
        )
    });
    assert_eq!(
        fraction.len(),
        FRACTION_DIGITS,
        "case {case:?}: expected a fractional-second field of {FRACTION_DIGITS} digits, got \
         {dated:?}"
    );
    assert!(
        fraction.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: expected the fraction to be digits, got {dated:?}"
    );
    assert!(
        whole.contains('T') && whole.len() == "2026-01-01T00:00:00".len(),
        "case {case:?}: expected a whole RFC 3339 calendar instant, got {dated:?}"
    );
}

/// The `Date` the control plane reports for the one record of `row`'s message on
/// [`INSTANCE`], or a panic naming what the log held instead.
async fn reported_date(router: &MultiServiceRouter, row: &Dated) -> String {
    let body = expect_ok(
        row.case,
        call(
            router,
            "DescribeEvents",
            &[
                ("SourceType", "db-instance"),
                ("SourceIdentifier", INSTANCE),
            ],
        )
        .await,
    );
    let rows: Vec<String> = subtrees(&body, "Event")
        .into_iter()
        .filter(|event| text(event, "Message") == row.message)
        .map(|event| text(&event, "Date"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "case {:?}: the log reports exactly the one change that happened, got {body}",
        row.case
    );
    rows.into_iter().next().unwrap_or_default()
}

/// RULE: every published event date carries a three-digit fraction of a second, and names the
/// same instant the control plane reports for that record.
#[tokio::test]
async fn every_published_event_is_dated_at_millisecond_precision() {
    let router = emulator();
    // The rules are in place before the instance exists, so the creation of the instance is
    // published onto a bus that is already matching — the row a consumer written before the
    // resource receives.
    let arn = format!(
        "arn:aws:rds:{}:{}:db:{INSTANCE}",
        crate::db_instances::REGION,
        crate::db_instance_bus_events::ACCOUNT
    );
    let mut queues = Vec::new();
    for row in DATED {
        queues.push(rule_to_its_own_queue(&router, row.slug, &pattern_of(row, &arn)).await);
    }

    let reported_arn = create_available(&router, INSTANCE).await;
    assert_eq!(
        reported_arn, arn,
        "the instance's own ARN is the one the patterns above pin, got {reported_arn:?}"
    );

    for (row, queue_url) in DATED.iter().zip(&queues) {
        if let Some(action) = row.action {
            expect_ok(
                row.case,
                call(&router, action, &[("DBInstanceIdentifier", INSTANCE)]).await,
            );
        }
        let delivered = collect(&router, queue_url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {:?}: the change's own rule receives it exactly once, got {delivered:?}",
            row.case
        );
        let event: Value = serde_json::from_str(&delivered[0]).unwrap_or_else(|error| {
            panic!("case {:?}: {} must be JSON: {error}", row.case, delivered[0])
        });
        assert_eq!(
            event["detail"]["EventID"],
            json!(row.event_id),
            "case {:?}: expected {}, got {event}",
            row.case,
            row.event_id
        );
        let dated = event["detail"]["Date"].as_str().unwrap_or_default();
        assert_millisecond_precision(row.case, dated);

        // The instance has settled on the status the change leaves it in, so the control
        // plane holds the same record — and reports the same instant for it.
        wait_for_status(&router, INSTANCE, row.settled_status).await;
        let reported = reported_date(&router, row).await;
        assert_eq!(
            instant_of(row.case, &reported),
            instant_of(row.case, dated),
            "case {:?}: expected the reported date to name the published instant ({dated:?}), \
             got {reported:?}",
            row.case
        );
    }
}
