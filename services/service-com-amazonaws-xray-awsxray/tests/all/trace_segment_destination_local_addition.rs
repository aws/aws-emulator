// SPDX-License-Identifier: CC0-1.0

//! The destination value set this crate declares LOCALLY, checked against the
//! published snapshot it restates and against the wire it produces.
//!
//! The rule asserted here: the values an enumerated member admits and reports
//! are the values this service's own model declares for that type — the
//! published snapshot plus the overlay beside it
//! (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`),
//! which restates what the snapshot declares EXACTLY and adds only what the
//! service admits beyond it. So every value of the set is a destination
//! `UpdateTraceSegmentDestination` configures and `GetTraceSegmentDestination`
//! reports back, a value outside the set is refused with the whole set stated,
//! and nothing of the restatement — a member, a bound, a target, a value — may
//! drift from the snapshot unnoticed.
//!
//! The rows span what the rule generalises over on both halves:
//!
//! * the model: WHICH shape the overlay restates (an operation input, the two
//!   answers that report a destination, the enumerated type itself, the status
//!   type, the nested configuration, and the string its one member targets) and
//!   WHAT it restates of it (member targets and traits, enum values, a length
//!   bound);
//! * the wire: WHICH admitted value is configured (every value the set carries,
//!   each reached from the account default), and how a value outside the set is
//!   spelled (a name no destination carries, an admitted value miscased, the
//!   empty string).
//!
//! The one thing this cannot compare is the traits codegen does not read: the
//! overlay restates what the generated types are built from — targets, types,
//! enum values and the `smithy.api` traits the validating walk enforces — so
//! prose documentation is dropped from both sides before they are compared.
//!
//! When a refreshed snapshot declares the local value itself, this suite fails
//! and the local declaration should give way to the modelled shape — which is
//! the point: the overlay exists only for what the snapshot does not carry.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use service_com_amazonaws_xray_awsxray::ops::types::ADMITTED_TRACE_SEGMENT_DESTINATIONS;
use service_com_amazonaws_xray_awsxray::state::{
    DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_STATUS_ACTIVE, DESTINATION_STATUS_PENDING,
};

use super::xray_wire::{Answer, Xray};

/// The published model snapshot this crate is generated from, embedded at
/// compile time so the local declaration is checked against the model rather
/// than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-xray-awsxray.json"
));

/// The overlay model the local declarations are generated from, embedded for the
/// same reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the overlay restates with a value of its own, and that value: the
/// enumerated destination type, which the snapshot declares without the
/// composite destination.
const RESTATED_WITH_LOCAL_VALUES: &[(&str, &str)] =
    &[("com.amazonaws.xray#TraceSegmentDestination", "Composite")];

/// Every shape a local type is declared from: the switch's input and answer and
/// the read's answer, each carrying a destination member.
const DECLARED_FROM_OVERLAY: &[&str] = &[
    "com.amazonaws.xray#UpdateTraceSegmentDestinationRequest",
    "com.amazonaws.xray#UpdateTraceSegmentDestinationResult",
    "com.amazonaws.xray#GetTraceSegmentDestinationResult",
];

/// How long a switch is waited FOR before a case gives up: generous on purpose,
/// because what the wait below is bounded by is the CONDITION, not the clock.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often the reported destination is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

// =============================================================================
// 1. The overlay restates the snapshot exactly, but for the value it adds
// =============================================================================

/// The `shapes` object of one model document.
fn model_shapes(document: &str) -> Value {
    serde_json::from_str::<Value>(document)
        .expect("the model is JSON")
        .get("shapes")
        .expect("the model carries a shapes object")
        .clone()
}

/// The traits one declaration states that codegen reads, as a map — empty where
/// it states none of them.
///
/// What is kept is the `smithy.api` namespace: the `@enumValue`, `@required`,
/// `@length` and naming traits the validating walk and the codec read. What is
/// dropped is prose documentation, which nothing reads off the overlay.
fn read_traits(declaration: &Value) -> Map<String, Value> {
    declaration["traits"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(trait_id, _)| trait_id.starts_with("smithy.api#"))
        .filter(|(trait_id, _)| trait_id.as_str() != "smithy.api#documentation")
        .map(|(trait_id, value)| (trait_id.clone(), value.clone()))
        .collect()
}

/// The declaration of one member, as codegen reads it: its target and the traits
/// above.
fn as_read(declaration: &Value) -> Value {
    match declaration {
        Value::Null => Value::Null,
        _ => {
            let mut read = Map::new();
            read.insert("target".to_string(), declaration["target"].clone());
            read.insert(
                "traits".to_string(),
                Value::Object(read_traits(declaration)),
            );
            Value::Object(read)
        }
    }
}

/// The traits one declaration states, `Null` where it states none but
/// documentation.
fn traits_of(declaration: &Value) -> Value {
    match Value::Object(read_traits(declaration)) {
        Value::Object(traits) if traits.is_empty() => Value::Null,
        other => other,
    }
}

/// The local value the overlay adds to `shape_id`, or `None` for a shape it
/// restates without adding one.
fn local_value_of(shape_id: &str) -> Option<&'static str> {
    RESTATED_WITH_LOCAL_VALUES
        .iter()
        .find(|(shape, _)| *shape == shape_id)
        .map(|(_, value)| *value)
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_value_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        let case = shape_id.as_str();
        let modelled = &snapshot[case];
        assert!(
            !modelled.is_null(),
            "case {case}: the overlay restates a shape the snapshot does not declare"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "case {case}: expected the type the snapshot gives it, got {}",
            declared["type"]
        );
        // The traits of the SHAPE are where a string's length bound lives, and
        // they are the bounds the validating walk enforces for every member
        // that targets this shape — so they must be the snapshot's own.
        assert_eq!(
            traits_of(declared),
            traits_of(modelled),
            "case {case}: expected the traits the snapshot states for it, got {}",
            declared["traits"]
        );

        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();
        for (member, restated_member) in &declared_members {
            if Some(member.as_str()) == local_value_of(case) {
                continue;
            }
            assert_eq!(
                as_read(restated_member),
                as_read(&modelled_members[member]),
                "case {case}: member {member} must be restated as the snapshot declares it, \
                 got {restated_member}"
            );
        }

        // Nothing the snapshot declares may be dropped: a member left out would
        // be dropped from the wire, and an enum value left out would be refused
        // where the service admits it.
        let dropped: Vec<&String> = modelled_members
            .keys()
            .filter(|member| !declared_members.contains_key(*member))
            .collect();
        assert!(
            dropped.is_empty(),
            "case {case}: every modelled member must be restated, dropped {dropped:?}"
        );
        let added: Vec<&str> = declared_members
            .keys()
            .filter(|member| !modelled_members.contains_key(*member))
            .map(String::as_str)
            .collect();
        assert_eq!(
            added,
            local_value_of(case).map(|local| vec![local]).unwrap_or_default(),
            "case {case}: expected only the local value the overlay adds, added {added:?}"
        );
    }

    // Every shape a local type is declared from is restated by the overlay, and
    // the snapshot still declares none of the local values: when it does, the
    // local declaration should give way to the modelled one.
    for shape_id in DECLARED_FROM_OVERLAY {
        assert!(
            restated.contains_key(*shape_id),
            "case {shape_id}: the overlay must restate the shape its local type is declared \
             from"
        );
    }
    for (shape_id, local) in RESTATED_WITH_LOCAL_VALUES {
        assert!(
            restated.contains_key(*shape_id),
            "case {shape_id}: the overlay must restate the shape it adds {local} to"
        );
        let modelled_members = snapshot[*shape_id]["members"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !modelled_members.contains_key(*local),
            "case {shape_id}: the snapshot now declares {local}, so the local declaration \
             should give way to the modelled one"
        );
    }
}

#[test]
fn the_value_catalogue_is_the_overlays_own_set() {
    // The catalogue the handler admits a destination from is written out beside
    // the type it belongs to, so it is held to the overlay's own enum values
    // rather than to whatever the service happens to accept.
    let overlay = model_shapes(OVERLAY);
    let (enum_shape, _) = RESTATED_WITH_LOCAL_VALUES[0];
    let mut declared: Vec<String> = overlay[enum_shape]["members"]
        .as_object()
        .expect("the overlay declares the destination type's members")
        .values()
        .map(|member| {
            member["traits"]["smithy.api#enumValue"]
                .as_str()
                .expect("every enum member declares its value")
                .to_string()
        })
        .collect();
    declared.sort_unstable();
    let catalogue: Vec<String> = ADMITTED_TRACE_SEGMENT_DESTINATIONS
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        catalogue, declared,
        "case \"the destination catalogue\": expected the overlay's value set {declared:?}, \
         got {catalogue:?}"
    );
    assert!(
        ADMITTED_TRACE_SEGMENT_DESTINATIONS.contains(&DEFAULT_TRACE_SEGMENT_DESTINATION),
        "case \"the account default\": {DEFAULT_TRACE_SEGMENT_DESTINATION:?} must be one of \
         {ADMITTED_TRACE_SEGMENT_DESTINATIONS:?}"
    );
}

// =============================================================================
// 2. The set on the wire: every admitted value configured and reported back
// =============================================================================

/// `(Destination, Status)`: the pair both operations answer with.
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

/// Read the reported destination until the switch the clock owes it is in
/// effect: the condition is waited FOR rather than for a duration, so the
/// deadline is generous and the poll looks again until the status is terminal.
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

#[tokio::test]
async fn every_value_of_the_declared_set_round_trips_through_both_operations() {
    for destination in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        let case = format!("the declared value {destination}");
        // CloudWatch Logs is mounted on the same engine and the spans delivery
        // granted, which is what a switch to the destination this service
        // delivers to needs; the other values need neither and are unaffected.
        let xray = Xray::with_cloud_watch_logs();
        xray.grant_spans_delivery(&case).await;

        if *destination == DEFAULT_TRACE_SEGMENT_DESTINATION {
            // The account starts on its default, so there is no switch to apply:
            // it is reported in effect from the first read.
            let read = get_destination(&xray).await;
            assert_eq!(
                (read.status, destination_and_status(&read)),
                (200, (Some(*destination), Some(DESTINATION_STATUS_ACTIVE))),
                "case {case:?}: the default must be in effect, got {read:?}"
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
async fn a_value_outside_the_declared_set_is_refused_stating_every_value_in_it() {
    // Rows: how a value outside the set is spelled. The set a refusal states
    // belongs to the TYPE, so nothing about the request moves it.
    let spellings = [
        ("a name no destination carries", "NotARealDestination"),
        ("an admitted value miscased", "composite"),
        ("the empty string", ""),
    ];

    let xray = Xray::new();
    for (case, value) in spellings {
        let refused = update_destination(&xray, value).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        // The clause lists the whole declared set — read off the service's own
        // message, so a value missing from it fails the case.
        let mut listed = stated_values(refused.message().unwrap_or_default());
        let mut declared: Vec<String> = ADMITTED_TRACE_SEGMENT_DESTINATIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        listed.sort_unstable();
        declared.sort_unstable();
        assert_eq!(
            listed, declared,
            "case {case:?}: the clause must list every declared destination, got {listed:?}"
        );
    }
}

/// The values a requirement clause states its enum value set over; empty for a
/// message stating no set.
fn stated_values(message: &str) -> Vec<String> {
    let Some((_, set)) = message.split_once("enum value set: [") else {
        return Vec::new();
    };
    let Some((set, _)) = set.split_once(']') else {
        return Vec::new();
    };
    set.split(", ").map(str::to_string).collect()
}
