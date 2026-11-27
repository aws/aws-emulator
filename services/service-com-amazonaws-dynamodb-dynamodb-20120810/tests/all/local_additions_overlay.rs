// SPDX-License-Identifier: CC0-1.0

//! The OVERLAY model this crate declares its local shapes from, checked against the
//! published snapshot it restates.
//!
//! RULE under test: `smithy-model/local-additions.json` restates what the snapshot declares
//! EXACTLY — the same type, the same constraint traits, the same member targets, the same
//! enum values — and adds only the members AWS answers with (or accepts) that the snapshot
//! declares nowhere (see `smithy-model/LOCAL-ADDITIONS.md`). Those restatements are what the
//! framework's validating walk enforces for the shapes declared from them, so a bound
//! dropped, mistyped or renamed here would silently loosen or tighten what the service
//! admits, with no wire test obliged to notice.
//!
//! The rows span what the rule generalises over: WHICH shape the overlay restates (an
//! operation input, a description, a summary, a capacity report), and WHAT it restates of it
//! (a shape whose members a local type is declared from, and a shape restated only so those
//! members can target it, whose type and constraint traits are all codegen reads).
//!
//! The one thing this cannot compare is the traits codegen does not read: the overlay
//! restates what the generated types are built from — targets, types, enum values and the
//! `smithy.api` traits the validating walk enforces — so the prose documentation and the
//! endpoint-rules annotations, which are read from the snapshot's own service and operation
//! shapes and from nothing the overlay declares, are dropped from both sides before they are
//! compared.
//!
//! When a refreshed snapshot declares one of the local members itself, this suite fails and
//! the local declaration should give way to the modelled shape — which is the point: the
//! overlay exists only for what the snapshot does not carry.

use serde_json::{Map, Value};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json"
));

/// The overlay model the local declarations are generated from, embedded for the same
/// reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shapes a local type is declared from, each with the member it adds to the snapshot's
/// own: the throughput mode a request states and a description reports, the write figure a
/// vector capacity carries under a second name, the recovery window reported beside the
/// modelled period, and the destination kind the export readers report.
const RESTATED_WITH_LOCAL_MEMBERS: &[(&str, &str)] = &[
    (
        "com.amazonaws.dynamodb#CreateTableInput",
        "TableThroughputMode",
    ),
    (
        "com.amazonaws.dynamodb#UpdateTableInput",
        "TableThroughputMode",
    ),
    (
        "com.amazonaws.dynamodb#TableDescription",
        "TableThroughputModeSummary",
    ),
    (
        "com.amazonaws.dynamodb#VectorCapacity",
        "VectorWriteRequestUnits",
    ),
    (
        "com.amazonaws.dynamodb#PointInTimeRecoveryDescription",
        "PointInTimeRecoveryWindowDays",
    ),
    (
        "com.amazonaws.dynamodb#ExportDescription",
        "DestinationType",
    ),
    ("com.amazonaws.dynamodb#ExportSummary", "DestinationType"),
];

/// The shapes the overlay ADDS outright, which the snapshot declares nowhere: the summary
/// object the local table member carries, and the type the local export member is read as.
const LOCAL_SHAPES: &[&str] = &[
    "com.amazonaws.dynamodb#TableThroughputModeSummary",
    "com.amazonaws.dynamodb#ExportDestinationType",
];

/// The `shapes` object of one model document.
fn model_shapes(document: &str) -> Value {
    serde_json::from_str::<Value>(document)
        .expect("the model is JSON")
        .get("shapes")
        .expect("the model carries a shapes object")
        .clone()
}

/// The traits one declaration states that codegen reads, as a map — empty where it states
/// none of them.
///
/// What is kept is the `smithy.api` namespace: the `@required`, `@default`, `@length`,
/// `@range`, `@pattern` and naming traits the validating walk and the codec read. What is
/// dropped is documentation (prose) and the endpoint-rules annotations
/// (`smithy.rules#contextParam` and its siblings), which are resolved from the snapshot's own
/// service and operation shapes — the overlay declares neither, so nothing reads them from
/// it.
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

/// The declaration of one member or one list/map slot, as codegen reads it: its target and
/// the traits above.
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

/// The traits one declaration states, `Null` where it states none but documentation.
fn traits_of(declaration: &Value) -> Value {
    match Value::Object(read_traits(declaration)) {
        Value::Object(traits) if traits.is_empty() => Value::Null,
        other => other,
    }
}

/// The local member the overlay adds to `shape_id`, or `None` for a shape it restates only
/// so another one can target it.
fn local_member_of(shape_id: &str) -> Option<&'static str> {
    RESTATED_WITH_LOCAL_MEMBERS
        .iter()
        .find(|(shape, _)| *shape == shape_id)
        .map(|(_, member)| *member)
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        let case = shape_id.as_str();
        if LOCAL_SHAPES.contains(&case) {
            assert!(
                snapshot[case].is_null(),
                "case {case}: the snapshot now declares this shape, so the local declaration \
                 should give way to the modelled one"
            );
            continue;
        }
        let modelled = &snapshot[case];
        assert!(
            !modelled.is_null(),
            "case {case}: the overlay restates a shape the snapshot no longer declares"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "case {case}: expected the type the snapshot gives it, got {}",
            declared["type"]
        );
        // The traits of the SHAPE are where a string's length and pattern, a number's range
        // and a list's length live, and they are the bounds the validating walk enforces for
        // every member that targets this shape — so they must be the snapshot's own.
        assert_eq!(
            traits_of(declared),
            traits_of(modelled),
            "case {case}: expected the traits the snapshot states for it, got {}",
            declared["traits"]
        );
        // A list's element and a map's key and value are read by codegen too.
        for read in ["member", "key", "value"] {
            assert_eq!(
                as_read(&declared[read]),
                as_read(&modelled[read]),
                "case {case}: expected the {read} the snapshot declares, got {}",
                declared[read]
            );
        }

        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();
        for (member, restated_member) in &declared_members {
            if Some(member.as_str()) == local_member_of(case) {
                continue;
            }
            assert_eq!(
                as_read(restated_member),
                as_read(&modelled_members[member]),
                "case {case}: member {member} must be restated as the snapshot declares it, \
                 got {restated_member}"
            );
        }

        match local_member_of(case) {
            // A shape a local type is declared from restates EVERY member the snapshot
            // declares — a member left out would be dropped from the wire — and adds the one
            // local member and no other.
            Some(local) => {
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
                    vec![local],
                    "case {case}: expected the one local member, added {added:?}"
                );
            }
            // A shape restated only so the members above can target it is restated as far as
            // codegen reads it — its type and its own traits, which the two assertions above
            // compared — and declares no member of its own beside the ones the snapshot
            // declares, so nothing of it can drift unnoticed.
            None => {
                let added: Vec<&str> = declared_members
                    .keys()
                    .filter(|member| !modelled_members.contains_key(*member))
                    .map(String::as_str)
                    .collect();
                assert!(
                    added.is_empty(),
                    "case {case}: a shape restated to be targeted may add no member, added \
                     {added:?}"
                );
            }
        }
    }

    // Every shape a local type is declared from is restated by the overlay, and the snapshot
    // still declares none of the local members: when it does, the local declaration should
    // give way to the modelled member.
    for (shape_id, local) in RESTATED_WITH_LOCAL_MEMBERS {
        assert!(
            restated.contains_key(*shape_id),
            "case {shape_id}: the overlay must restate the shape its local type is declared \
             from"
        );
        let modelled_members = snapshot[*shape_id]["members"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !modelled_members.contains_key(*local),
            "case {shape_id}: the snapshot now declares {local}, so the local declaration \
             should give way to the modelled member"
        );
    }
}
