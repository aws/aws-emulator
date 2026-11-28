// SPDX-License-Identifier: CC0-1.0

//! Which destinations the trace segment destination type admits, end to end over
//! the wire.
//!
//! The rule asserted here: the values an enumerated type admits are the values
//! the SERVICE admits — so every value of `TraceSegmentDestination` is a
//! destination `UpdateTraceSegmentDestination` configures and
//! `GetTraceSegmentDestination` reports back, each going through the one
//! destination state machine (`PENDING` until the switch is in effect, then
//! `ACTIVE`, and a request naming the destination already in effect refused),
//! and a value OUTSIDE the type is refused in AWS's validation envelope whose
//! requirement clause states the WHOLE admitted set.
//!
//! The rows differ along what the rule generalises over: which admitted value is
//! configured (every value the type publishes, as both the source and the target
//! of a switch), whether the value reached the service as the first switch or
//! after another, and — for the refusals — how a value outside the type is
//! spelled (a name no destination carries, an admitted value miscased, an
//! admitted value padded, the empty string, a value carrying the stated set's own
//! text).
//!
//! The admitted set is read from this service's own statement of the type
//! ([`ADMITTED_TRACE_SEGMENT_DESTINATIONS`], the values of
//! `com.amazonaws.xray#TraceSegmentDestination` as this crate declares it — see
//! `smithy-model/LOCAL-ADDITIONS.md`), so a value added to or removed from the
//! type is covered here without editing a row; only the ORDER the refusal states
//! the set in is written out, because an order cannot be derived from the set.

use std::time::{Duration, Instant};

use serde_json::json;

use service_com_amazonaws_xray_awsxray::ops::types::ADMITTED_TRACE_SEGMENT_DESTINATIONS;
use service_com_amazonaws_xray_awsxray::state::destination_switch::destination_already_set_message;
use service_com_amazonaws_xray_awsxray::state::{
    CLOUD_WATCH_LOGS_DESTINATION, DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_STATUS_ACTIVE,
    DESTINATION_STATUS_PENDING,
};

use super::xray_wire::{Answer, Xray};

/// How long a switch is waited FOR before a case gives up: generous on purpose,
/// because what each wait below is bounded by is the CONDITION, not the clock.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often the reported destination is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The dual destination `TraceSegmentDestination` admits beside `XRay` and
/// `CloudWatchLogs`, named here because it is the member the emulator did not
/// admit; the cases below take the whole set from
/// [`ADMITTED_TRACE_SEGMENT_DESTINATIONS`] rather than from this constant.
const COMPOSITE_DESTINATION: &str = "Composite";

/// The order this service states the admitted set in when it refuses a value
/// outside the type: the composite destination first, which is neither the
/// sorted order nor the order the type is declared in.
const STATED_VALUE_SET: &str = "[Composite, CloudWatchLogs, XRay]";

/// `(Destination, Status)`: the pair both operations here answer with.
fn destination_and_status(answer: &Answer) -> (Option<&str>, Option<&str>) {
    (
        answer.get("Destination").as_str(),
        answer.get("Status").as_str(),
    )
}

async fn get_destination(xray: &Xray) -> Answer {
    xray.call("/GetTraceSegmentDestination", &json!({})).await
}

async fn update_destination(xray: &Xray, destination: &str) -> Answer {
    xray.call(
        "/UpdateTraceSegmentDestination",
        &json!({ "Destination": destination }),
    )
    .await
}

/// One emulator with CloudWatch Logs mounted beside X-Ray on ONE engine and the
/// spans delivery granted, which is what a switch to the destination this
/// service delivers to needs.
async fn emulator(case: &str) -> Xray {
    let xray = Xray::with_cloud_watch_logs();
    xray.grant_spans_delivery(case).await;
    xray
}

/// Read the reported destination until the switch the clock owes it is in
/// effect, and answer what was read then.
///
/// The move to `ACTIVE` falls due at a fixed instant after the call that asked
/// for it, so the condition is waited FOR rather than for a duration: the
/// deadline is generous and the poll looks again until the status is the
/// terminal one.
async fn settled(xray: &Xray, case: &str, destination: &str) -> Answer {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = get_destination(xray).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the destination must succeed, got {read:?}"
        );
        if destination_and_status(&read) == (Some(destination), Some(DESTINATION_STATUS_ACTIVE)) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the switch to {destination:?} never took effect, last read {read:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

// =============================================================================
// 1. Every value the type admits is a destination the service configures
// =============================================================================

#[tokio::test]
async fn every_admitted_destination_is_configured_and_reported_back() {
    for destination in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        let case = format!("switch to {destination}");
        let xray = emulator(&case).await;

        // The account starts on its default, so a value that IS the default has
        // no switch to apply and is reported as in effect from the start; every
        // other value is switched to and reported `PENDING` until the clock owes
        // it `ACTIVE`.
        if *destination == DEFAULT_TRACE_SEGMENT_DESTINATION {
            let read = get_destination(&xray).await;
            assert_eq!(
                (read.status, destination_and_status(&read)),
                (200, (Some(*destination), Some(DESTINATION_STATUS_ACTIVE))),
                "case {case:?}: the default destination must be in effect, got {read:?}"
            );
            continue;
        }

        let switched = update_destination(&xray, destination).await;
        assert_eq!(
            (switched.status, destination_and_status(&switched)),
            (200, (Some(*destination), Some(DESTINATION_STATUS_PENDING))),
            "case {case:?}: expected 200 {destination}/{DESTINATION_STATUS_PENDING}, \
             got {switched:?}"
        );

        let read = settled(&xray, &case, destination).await;
        assert_eq!(
            destination_and_status(&read),
            (Some(*destination), Some(DESTINATION_STATUS_ACTIVE)),
            "case {case:?}: the configured destination must be reported back, got {read:?}"
        );
    }
}

#[tokio::test]
async fn every_admitted_destination_is_switched_away_from_and_back_to() {
    // Each admitted value as the SOURCE of a switch as well as its target: the
    // value configured is what the next read reports, whichever value the account
    // was on, so no admitted value is a dead end.
    for from in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        for to in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
            if from == to {
                continue;
            }
            let case = format!("{from} -> {to}");
            let xray = emulator(&case).await;

            if *from != DEFAULT_TRACE_SEGMENT_DESTINATION {
                let seeded = update_destination(&xray, from).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: seeding {from} must succeed, got {seeded:?}"
                );
                settled(&xray, &case, from).await;
            }

            let switched = update_destination(&xray, to).await;
            assert_eq!(
                (switched.status, destination_and_status(&switched)),
                (200, (Some(*to), Some(DESTINATION_STATUS_PENDING))),
                "case {case:?}: expected 200 {to}/{DESTINATION_STATUS_PENDING}, got {switched:?}"
            );
            settled(&xray, &case, to).await;

            // And the state machine's own rule holds for it: naming the
            // destination now in effect applies no switch, so it is refused with
            // the modelled `InvalidRequestException`.
            let repeated = update_destination(&xray, to).await;
            assert_eq!(
                (
                    repeated.status,
                    repeated.error_type.as_deref(),
                    repeated.message()
                ),
                (
                    400,
                    Some("InvalidRequestException"),
                    Some(destination_already_set_message(to).as_str())
                ),
                "case {case:?}: re-requesting {to} must be refused, got {repeated:?}"
            );
        }
    }
}

// =============================================================================
// 2. A value outside the type is refused stating the WHOLE admitted set
// =============================================================================

#[tokio::test]
async fn a_destination_outside_the_type_is_refused_stating_every_admitted_value() {
    // Rows: how a value outside the type is spelled. The admitted set a refusal
    // states belongs to the TYPE, so nothing about the request moves it.
    let spellings: Vec<(&str, &str)> = vec![
        ("a name no destination carries", "NotARealDestination"),
        ("an admitted value miscased", "composite"),
        ("an admitted value padded", " Composite "),
        ("the empty string", ""),
        ("a value carrying the stated set's own text", "Composite, XRay"),
    ];

    let xray = Xray::new();
    for (label, value) in spellings {
        let refused = update_destination(&xray, value).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {label:?}: expected 400 ValidationException, got {refused:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'destination' failed to satisfy \
             constraint: Member must satisfy enum value set: {STATED_VALUE_SET}"
        );
        assert_eq!(
            refused.message(),
            Some(expected.as_str()),
            "case {label:?}: expected {expected:?}, got {refused:?}"
        );

        // The values the clause listed are exactly the ones the service admits —
        // read off its own message, so a member missing from the clause fails the
        // case whatever the literal above happens to say.
        let mut listed = stated_values(refused.message().unwrap_or_default());
        let mut admitted: Vec<String> = ADMITTED_TRACE_SEGMENT_DESTINATIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        listed.sort_unstable();
        admitted.sort_unstable();
        assert_eq!(
            listed, admitted,
            "case {label:?}: the clause must list every admitted destination, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_composite_destination_is_admitted_rather_than_refused() {
    // The one value the type publishes that the account is never on to begin
    // with and that carries no configuration of its own: a request naming it is
    // served rather than refused as a value outside the type.
    let case = "the composite destination";
    assert!(
        ADMITTED_TRACE_SEGMENT_DESTINATIONS.contains(&COMPOSITE_DESTINATION),
        "case {case:?}: {COMPOSITE_DESTINATION:?} must be one of \
         {ADMITTED_TRACE_SEGMENT_DESTINATIONS:?}"
    );

    let xray = emulator(case).await;
    let switched = update_destination(&xray, COMPOSITE_DESTINATION).await;
    assert_eq!(
        (
            switched.status,
            switched.error_type.as_deref(),
            destination_and_status(&switched)
        ),
        (
            200,
            None,
            (
                Some(COMPOSITE_DESTINATION),
                Some(DESTINATION_STATUS_PENDING)
            )
        ),
        "case {case:?}: the composite destination must be accepted, got {switched:?}"
    );
    settled(&xray, case, COMPOSITE_DESTINATION).await;

    // And it is reached from the destination this service delivers to as well as
    // from the account default, so it is a destination of the state machine
    // rather than a value only the first switch takes.
    let case = "the composite destination after CloudWatchLogs";
    let xray = emulator(case).await;
    let to_logs = update_destination(&xray, CLOUD_WATCH_LOGS_DESTINATION).await;
    assert_eq!(
        to_logs.status, 200,
        "case {case:?}: seeding {CLOUD_WATCH_LOGS_DESTINATION} must succeed, got {to_logs:?}"
    );
    settled(&xray, case, CLOUD_WATCH_LOGS_DESTINATION).await;
    let composite = update_destination(&xray, COMPOSITE_DESTINATION).await;
    assert_eq!(
        (composite.status, destination_and_status(&composite)),
        (
            200,
            (
                Some(COMPOSITE_DESTINATION),
                Some(DESTINATION_STATUS_PENDING)
            )
        ),
        "case {case:?}: the composite destination must be accepted, got {composite:?}"
    );
    settled(&xray, case, COMPOSITE_DESTINATION).await;
}

/// The values a requirement clause states its enum value set over, in the order
/// it states them; empty for a message stating no set.
fn stated_values(message: &str) -> Vec<String> {
    let Some((_, set)) = message.split_once("enum value set: [") else {
        return Vec::new();
    };
    let Some((set, _)) = set.split_once(']') else {
        return Vec::new();
    };
    set.split(", ").map(str::to_string).collect()
}
