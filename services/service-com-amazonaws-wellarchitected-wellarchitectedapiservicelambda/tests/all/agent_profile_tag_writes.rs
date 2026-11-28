// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the VALUES a tag write may record on a resource of this
//! service, across the two tag shapes a resource's tags are reported through.
//!
//! The rules pinned here, each over a TABLE of values rather than one recorded request:
//!
//! 1. `TagResource` records a pair only when the tag shape the RESOURCE's own reads
//!    project its tags through can carry it. An agent profile reports its tags as modelled
//!    `Tag` structures, whose `value` is `@required` `@length(min: 1, max: 256)` over the
//!    character class `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`
//!    (`smithy_query(query="shape", shape="Tag")`), so a value only a `TagMap`'s
//!    `TagValue` (`@length(max: 256)`, no minimum, no pattern) admits is refused
//!    `400 ValidationException` naming the map entry to fix, and nothing is recorded —
//!    while every value both shapes admit round-trips through `GetAgentProfile`,
//!    `ListAgentProfiles` and `ListTagsForResource`.
//! 2. The rule belongs to the RESOURCE, not to the operation: a resource whose tags are
//!    reported ONLY as a `TagMap` still records every value that map admits, including the
//!    ones an agent profile refuses.
//! 3. The two doors onto one agent profile agree: a value `TagResource` refuses for a
//!    profile is a value `CreateAgentProfile` refuses in its `tags` list, and a value one
//!    records is a value the other records.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is `super::workload_harness`'s; the profiles, the tag write
//! and the tag reads are the wire helpers of the suites that own those operations, because
//! a resource is minted, tagged and read by exactly one operation each.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::agent_goals::{create_profile, get_profile, profile_arn_of, profile_body, tags_by_key};
use super::agent_profile_listing::list_profiles;
use super::resource_tagging::tag;
use super::resource_tags::{create_tagged, list_tags, reported_tags, KINDS};
use super::workload_harness::{Answer, Client};

/// The length bounds the two tag shapes state for a value: `Tag$value` is
/// `@length(min: 1, max: 256)` and `TagValue` is `@length(max: 256)`
/// (`smithy_query(query="shape", shape="Tag,TagValue")`), so the maximum is shared and
/// only the minimum is the structure's own.
const MAX_TAG_VALUE: usize = 256;

/// One row of the value table: what it is called, the key it is written under, the value
/// itself, and which of this service's two tag shapes can carry that value.
struct ValueRow {
    case: &'static str,
    /// The `TagKey` this row writes under — distinct per row, so one resource accumulates
    /// the rows it admits and a refusal shows up as an ABSENT key rather than as an
    /// overwrite.
    key: &'static str,
    value: String,
    /// Whether the modelled `Tag` structure can carry the value (see [`MAX_TAG_VALUE`] and
    /// the pattern this module's header quotes).
    carried_by_tag_structure: bool,
    /// Whether a `TagMap` can carry it: `TagValue` bounds the length alone, so every value
    /// up to the shared maximum qualifies.
    carried_by_tag_map: bool,
}

/// The values every rule below is asserted over. They differ along what the rules
/// generalise over — the LENGTH of the value (at each published bound and one past it) and
/// the CHARACTER CLASSES it is drawn from (letters, separators, digits, the punctuation the
/// class names, and characters outside it) — rather than along one recorded example.
fn value_rows() -> Vec<ValueRow> {
    let mut rows = vec![
        ValueRow {
            case: "a plain value",
            key: "plain",
            value: "production".to_string(),
            carried_by_tag_structure: true,
            carried_by_tag_map: true,
        },
        ValueRow {
            case: "one character, Tag$value's length minimum",
            key: "at-the-minimum",
            value: "x".to_string(),
            carried_by_tag_structure: true,
            carried_by_tag_map: true,
        },
        ValueRow {
            // `\p{L}` is not ASCII-only.
            case: "non-ASCII letters",
            key: "letters",
            value: "Ωμέγα-значение".to_string(),
            carried_by_tag_structure: true,
            carried_by_tag_map: true,
        },
        ValueRow {
            // `\p{N}` digits and the `\p{Z}` space separator.
            case: "digits and separators",
            key: "digits",
            value: "release 2026 01".to_string(),
            carried_by_tag_structure: true,
            carried_by_tag_map: true,
        },
        ValueRow {
            // Every punctuation character the class names, and nothing else.
            case: "every punctuation character the class admits",
            key: "punctuation",
            value: "_.:/=+-@".to_string(),
            carried_by_tag_structure: true,
            carried_by_tag_map: true,
        },
        ValueRow {
            case: "the empty value, one short of Tag$value's length minimum",
            key: "empty",
            value: String::new(),
            carried_by_tag_structure: false,
            carried_by_tag_map: true,
        },
        ValueRow {
            case: "a comma, which the class does not admit",
            key: "comma",
            value: "cost,centre".to_string(),
            carried_by_tag_structure: false,
            carried_by_tag_map: true,
        },
        ValueRow {
            case: "a percent sign, which the class does not admit",
            key: "percent",
            value: "100%".to_string(),
            carried_by_tag_structure: false,
            carried_by_tag_map: true,
        },
        ValueRow {
            // A line feed is whitespace but NOT one of `\p{Z}`'s separators.
            case: "a line feed, which is not one of the class's separators",
            key: "line-feed",
            value: "two\nlines".to_string(),
            carried_by_tag_structure: false,
            carried_by_tag_map: true,
        },
        ValueRow {
            case: "a quotation mark, which the class does not admit",
            key: "quoted",
            value: "\"quoted\"".to_string(),
            carried_by_tag_structure: false,
            carried_by_tag_map: true,
        },
    ];
    // The shared length maximum, and one character past it: past the bound NEITHER shape
    // carries the value, so every kind of resource refuses that row.
    rows.push(ValueRow {
        case: "a value at the length maximum both shapes state",
        key: "at-the-maximum",
        value: "v".repeat(MAX_TAG_VALUE),
        carried_by_tag_structure: true,
        carried_by_tag_map: true,
    });
    rows.push(ValueRow {
        case: "a value one character past the length maximum both shapes state",
        key: "past-the-maximum",
        value: "v".repeat(MAX_TAG_VALUE + 1),
        carried_by_tag_structure: false,
        carried_by_tag_map: false,
    });
    rows
}

/// The one-pair tag map a row is written as.
fn requested(row: &ValueRow) -> BTreeMap<String, String> {
    [(row.key.to_string(), row.value.clone())]
        .into_iter()
        .collect()
}

/// The message an answer carries, under either casing the error envelopes use.
fn message_of(answer: &Answer) -> &str {
    answer
        .body
        .get("Message")
        .or_else(|| answer.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Assert `answer` is the refusal a value the target shape cannot carry earns.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a value the resource's own tag shape cannot carry must be refused \
         400 ValidationException, got {answer:?}"
    );
}

/// The tags an agent profile projection reports, as a map.
fn profile_tags(projection: Option<&Value>) -> BTreeMap<String, String> {
    tags_by_key(projection).into_iter().collect()
}

/// Rule 1: what an agent profile's own tag shape can carry is what `TagResource` records
/// on it, and every read of the profile reports exactly that.
#[tokio::test]
async fn a_tag_write_records_only_values_the_resources_own_tag_shape_can_carry() {
    let client = Client::new();
    let arn = profile_arn_of(&client, "tag-value-rules", 1).await;

    // The rows the profile admitted so far: a refusal must leave this set untouched, which
    // is what the read after every row asserts.
    let mut recorded: BTreeMap<String, String> = BTreeMap::new();

    for row in value_rows() {
        let case = row.case;
        let answer = tag(&client, &arn, &requested(&row)).await;
        if row.carried_by_tag_structure {
            assert_eq!(
                answer.status, 204,
                "case {case:?}: a value the Tag structure carries must be recorded, got \
                 {answer:?}"
            );
            recorded.insert(row.key.to_string(), row.value.clone());
        } else {
            assert_refused(&answer, case);
            assert!(
                message_of(&answer).contains(&format!("tags.{}", row.key)),
                "case {case:?}: the refusal must name the map entry to fix, got {answer:?}"
            );
        }

        let read = get_profile(&client, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAgentProfile 200, got {read:?}"
        );
        assert_eq!(
            profile_tags(read.body.get("tags")),
            recorded,
            "case {case:?}: GetAgentProfile must report the admitted tags and nothing else, \
             got {read:?}"
        );
    }

    // Every value the profile admitted is reported by each read that projects them: the
    // single-profile read above and the `TagMap` plane the tag read answers through.
    // `AgentProfileSummary` declares no `tags` member
    // (`smithy_query(query="shape", shape="AgentProfileSummary")`), so the summary a
    // listing projects carries none at all.
    let listed = list_profiles(&client, "").await;
    let summary = listed
        .body
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("arn").and_then(Value::as_str) == Some(arn.as_str()))
        })
        .unwrap_or_else(|| panic!("expected the tagged profile in {listed:?}"))
        .clone();
    assert_eq!(
        summary.get("tags"),
        None,
        "a listed profile carries no tags member, got {listed:?}"
    );
    let read_back = list_tags(&client, &arn).await;
    assert_eq!(
        reported_tags(&read_back, "the admitted rows"),
        recorded,
        "ListTagsForResource must report the tags the profile carries, got {read_back:?}"
    );

    // Nothing the profile refused reached the store: the stored tag map is exactly the
    // admitted set, read through the state the handlers share.
    assert_eq!(
        client
            .store()
            .lock()
            .resource_tags(&arn)
            .cloned()
            .unwrap_or_default(),
        recorded,
        "the stored tag map must hold the admitted rows alone"
    );
}

/// Rule 2: a resource whose tags are reported only as a `TagMap` records every value that
/// map admits — including the values an agent profile refuses.
#[tokio::test]
async fn a_resource_reported_only_as_a_tag_map_records_the_values_only_it_carries() {
    for (column, kind) in KINDS.into_iter().enumerate() {
        // The kinds whose tags no read projects through the `Tag` structure.
        if !kind.carries_tag_map_only_rows() {
            continue;
        }
        let client = Client::new();
        let arn = create_tagged(
            &client,
            kind,
            &format!("{kind:?}"),
            &format!("tv{column}"),
            &BTreeMap::new(),
        )
        .await;

        let mut recorded: BTreeMap<String, String> = BTreeMap::new();
        for row in value_rows() {
            let case = format!("{kind:?}/{}", row.case);
            let answer = tag(&client, &arn, &requested(&row)).await;
            if row.carried_by_tag_map {
                assert_eq!(
                    answer.status, 204,
                    "case {case:?}: a value a TagMap carries must be recorded on a resource \
                     reported through one, got {answer:?}"
                );
                recorded.insert(row.key.to_string(), row.value.clone());
            } else {
                assert_refused(&answer, &case);
            }

            let read_back = list_tags(&client, &arn).await;
            assert_eq!(
                reported_tags(&read_back, &case),
                recorded,
                "case {case:?}: ListTagsForResource must report the recorded tags and \
                 nothing else, got {read_back:?}"
            );
        }
    }
}

/// Rule 3: the create and the tag write are two doors onto the same resource, and they
/// admit the same values.
#[tokio::test]
async fn the_create_and_the_tag_write_agree_on_what_an_agent_profile_tag_can_hold() {
    let client = Client::new();

    for (index, row) in value_rows().into_iter().enumerate() {
        let case = row.case;
        let mut body = profile_body(&format!("tag-door-{index}"), &["SECURITY"], 1);
        body["tags"] = json!([{ "key": row.key, "value": row.value }]);
        let created = create_profile(&client, body).await;

        if !row.carried_by_tag_structure {
            assert_refused(&created, case);
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {case:?}: a value the Tag structure carries must be recorded by the \
             create, got {created:?}"
        );
        let arn = created
            .body
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expected = requested(&row);
        assert_eq!(
            profile_tags(created.body.get("tags")),
            expected,
            "case {case:?}: the create must report the pair it recorded, got {created:?}"
        );

        // The same pair, written through the OTHER door onto an untagged profile, is
        // recorded and read back the same way.
        let tagged_arn = profile_arn_of(&client, &format!("tag-door-written-{index}"), 1).await;
        let answer = tag(&client, &tagged_arn, &expected).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected TagResource 204, got {answer:?}"
        );
        for (reader, read) in [
            ("the created profile", get_profile(&client, &arn).await),
            ("the tagged profile", get_profile(&client, &tagged_arn).await),
        ] {
            assert_eq!(
                profile_tags(read.body.get("tags")),
                expected,
                "case {case:?}: {reader} must report the pair, got {read:?}"
            );
        }
    }
}
