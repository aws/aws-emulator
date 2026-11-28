// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ONE rendering rule, over every response of this service that
//! reports a rule's record: an optional member the rule holds NO value for is left OUT of
//! the JSON body — it is never rendered as `null` and never as a stand-in value such as an
//! empty list.
//!
//! Every member below is optional and carries no `@default`
//! (smithy_query(query="operation", operation="CreateRule,GetRule,UpdateRule,LockRule,
//! UnlockRule,ListRules")), and under this protocol an optional member with no value is not
//! serialized at all, so "the rule has no description / no lock / assigned no tag" is the
//! ABSENCE of the member, which is what a client testing for its presence reads.
//!
//! `CreateRule`'s `Tags` is the member the create assigns the rule's tags with and answers
//! back, so a create that assigned none has no assignment to report; the rule's tag
//! COLLECTION, and every other collection a response reads off the record, is a present
//! list however empty it is (`tests/all/reported_collection_members.rs` and
//! `tests/all/create_rule_tags_echo.rs` own those two halves). This suite owns the
//! presence of the optional members a response renders from a value the rule may simply
//! not have.
//!
//! The table spans what the rule generalises over: which operation renders the record (the
//! create, the single-rule reader, the update, the two lock transitions and the list
//! projection), which member it is (description, tag assignments, lock configuration, lock
//! state, lock end time), the rule shape (Region-level, tag-level, exclusion-tagged,
//! locked) and whether the create stated the member, stated it empty, or left it out — plus
//! one row per modelled resource type.

use serde_json::{json, Value};

use crate::retention_rule_lifecycle::{
    create_rule, get_rule, list_rules, lock_configuration, retention, router, supported_retention,
    supported_retention_max, update_rule, RESOURCE_TYPES, UNLOCK_DELAY_MIN,
};
use crate::rule_lock_transitions::unlock_rule;

/// The optional members of `CreateRuleResponse` a rule may hold no value for.
/// Source: smithy_query(query="operation", operation="CreateRule") — output members that
/// are neither required nor always populated from the stored record.
const CREATE_OPTIONAL_MEMBERS: [&str; 4] = ["Description", "Tags", "LockConfiguration", "LockState"];

/// The same members of `GetRuleResponse`, which reports a lock end time as well and has no
/// `Tags` member at all.
/// Source: smithy_query(query="operation", operation="GetRule").
const GET_OPTIONAL_MEMBERS: [&str; 4] =
    ["Description", "LockConfiguration", "LockState", "LockEndTime"];

/// The optional members of the `RuleSummary` projection `ListRules` pages.
/// Source: smithy_query(query="operation", operation="ListRules").
const SUMMARY_OPTIONAL_MEMBERS: [&str; 2] = ["Description", "LockState"];

/// Assert `body` renders `member` as present when `expected` says so, and leaves it out
/// entirely otherwise — `null` is a rendering of the member, so it fails either way.
fn assert_presence(case: &str, operation: &str, body: &Value, member: &str, expected: bool) {
    let reported = body.get(member);
    if expected {
        assert!(
            reported.is_some_and(|value| !value.is_null()),
            "case {case:?}: expected {operation} to report {member}, got {reported:?}"
        );
    } else {
        assert_eq!(
            reported, None,
            "case {case:?}: expected {operation} to omit {member} for a rule holding no \
             such value, got {reported:?}"
        );
    }
}

/// Assert no member anywhere in `body` is rendered as `null`: a value the rule does not
/// have is an absent member, at every nesting level of the response.
fn assert_no_null_members(case: &str, operation: &str, body: &Value) {
    match body {
        Value::Object(members) => {
            for (member, value) in members {
                assert!(
                    !value.is_null(),
                    "case {case:?}: {operation} rendered {member} as null; an unset optional \
                     member is omitted"
                );
                assert_no_null_members(case, operation, value);
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                assert_no_null_members(case, operation, entry);
            }
        }
        _ => {}
    }
}

/// One row: the case, the `CreateRule` body, and which optional members the created rule
/// holds a value for.
struct Row {
    case: &'static str,
    body: Value,
    description: bool,
    tags: bool,
    locked: bool,
}

impl Row {
    /// Whether the row expects `member` present in a response that renders it.
    fn expects(&self, member: &str) -> bool {
        match member {
            "Description" => self.description,
            "Tags" => self.tags,
            // A rule created with a lock configuration starts out locked, so both the
            // configuration and the lock state it put the rule in are reported; a rule that
            // has never been locked reports neither. `LockEndTime` is "only returned for
            // retention rules that have been unlocked and that are still within the unlock
            // delay period", so no freshly created rule carries one.
            "LockConfiguration" | "LockState" => self.locked,
            _ => false,
        }
    }
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "a Region-level rule stating no optional member at all",
            body: json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(7),
            }),
            description: false,
            tags: false,
            locked: false,
        },
        Row {
            case: "a Region-level rule stating an explicitly empty tag list",
            body: json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(1),
                "Tags": [],
            }),
            description: false,
            tags: false,
            locked: false,
        },
        Row {
            case: "a Region-level rule carrying a description only",
            body: json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(30),
                "Description": "images worth keeping",
            }),
            description: true,
            tags: false,
            locked: false,
        },
        Row {
            case: "a tag-level rule carrying tag assignments but no description",
            body: json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(45),
                "Tags": [{ "Key": "env", "Value": "prod" }],
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
            }),
            description: false,
            tags: true,
            locked: false,
        },
        Row {
            case: "a Region-level rule with exclusion tags, a description and tag assignments",
            body: json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(supported_retention_max(RESOURCE_TYPES[1])),
                "Description": "everything but the scratch volumes",
                "Tags": [{ "Key": "team", "Value": "platform" }],
                "ExcludeResourceTags": [{ "ResourceTagKey": "scratch", "ResourceTagValue": "1" }],
            }),
            description: true,
            tags: true,
            locked: false,
        },
        Row {
            case: "a locked Region-level rule stating no description and no tag",
            body: json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(7),
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
            }),
            description: false,
            tags: false,
            locked: true,
        },
    ];
    // One bare rule per modelled resource type, including the types the rows above do not
    // reach with an empty record.
    for resource_type in RESOURCE_TYPES {
        rows.push(Row {
            case: "a bare rule, one per modelled resource type",
            body: json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(supported_retention(resource_type, 11)),
            }),
            description: false,
            tags: false,
            locked: false,
        });
    }
    rows
}

#[tokio::test]
async fn an_optional_member_the_rule_holds_no_value_for_is_omitted_from_every_response() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let resource_type = row.body["ResourceType"]
            .as_str()
            .expect("every row states a resource type")
            .to_string();

        let (status, created) = create_rule(&router, row.body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
        );
        assert_no_null_members(case, "CreateRule", &created);
        for member in CREATE_OPTIONAL_MEMBERS {
            assert_presence(case, "CreateRule", &created, member, row.expects(member));
        }

        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier"))
            .to_string();

        // ROUND TRIP: the reader renders the SAME record, so a member the create omitted is
        // a member the rule holds no value for, not one the create forgot.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        assert_no_null_members(case, "GetRule", &fetched);
        for member in GET_OPTIONAL_MEMBERS {
            assert_presence(case, "GetRule", &fetched, member, row.expects(member));
        }
        // `GetRuleResponse` models no `Tags` member, so the reader renders none whatever the
        // rule was assigned.
        assert_eq!(
            fetched.get("Tags"),
            None,
            "case {case:?}: GetRule models no Tags member, got {:?}",
            fetched.get("Tags")
        );

        // The list projection renders the same record through a smaller shape.
        let (status, page) = list_rules(&router, json!({ "ResourceType": resource_type })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListRules, got {status} / {page:?}"
        );
        assert_no_null_members(case, "ListRules", &page);
        let summary = page["Rules"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListRules reported no Rules page"))
            .iter()
            .find(|rule| rule["Identifier"].as_str() == Some(identifier.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the created rule is not in the page"))
            .clone();
        for member in SUMMARY_OPTIONAL_MEMBERS {
            assert_presence(case, "ListRules", &summary, member, row.expects(member));
        }
    }
}

#[tokio::test]
async fn update_rule_omits_a_description_the_rule_still_does_not_have() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        json!({
            "ResourceType": RESOURCE_TYPES[0],
            "RetentionPeriod": retention(7),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "fixture CreateRule returned {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .expect("a minted Identifier")
        .to_string();

    // An update that does not touch the description leaves the rule without one, so the
    // member is still absent from the response the update renders.
    let (status, updated) =
        update_rule(&router, &identifier, json!({ "RetentionPeriod": retention(14) })).await;
    assert_eq!(
        status, 200,
        "an update that names no description returned {status} / {updated:?}"
    );
    assert_no_null_members("an update that names no description", "UpdateRule", &updated);
    for member in ["Description", "LockState", "LockEndTime"] {
        assert_presence(
            "an update that names no description",
            "UpdateRule",
            &updated,
            member,
            false,
        );
    }

    // Once the rule holds a description, the same response renders it.
    let (status, described) = update_rule(
        &router,
        &identifier,
        json!({ "Description": "a description this rule did not have" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update that names a description returned {status} / {described:?}"
    );
    assert_presence(
        "an update that names a description",
        "UpdateRule",
        &described,
        "Description",
        true,
    );
}

#[tokio::test]
async fn lock_end_time_is_rendered_only_once_an_unlock_delay_is_running() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        json!({
            "ResourceType": RESOURCE_TYPES[1],
            "RetentionPeriod": retention(supported_retention_max(RESOURCE_TYPES[1])),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "fixture CreateRule returned {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .expect("a minted Identifier")
        .to_string();

    // A locked rule is inside no unlock delay, so it holds no lock end time to render.
    let (status, fetched) = get_rule(&router, &identifier).await;
    assert_eq!(status, 200, "GetRule returned {status} / {fetched:?}");
    assert_presence("a locked rule", "GetRule", &fetched, "LockEndTime", false);

    // Unlocking starts the delay, which gives the rule an end time — the one value these
    // responses render only while it exists.
    let (status, unlocked) = unlock_rule(&router, &identifier).await;
    assert_eq!(status, 200, "UnlockRule returned {status} / {unlocked:?}");
    assert_no_null_members("a rule inside its unlock delay", "UnlockRule", &unlocked);
    assert_presence(
        "a rule inside its unlock delay",
        "UnlockRule",
        &unlocked,
        "LockEndTime",
        true,
    );

    let (status, fetched) = get_rule(&router, &identifier).await;
    assert_eq!(status, 200, "GetRule returned {status} / {fetched:?}");
    assert_presence(
        "a rule inside its unlock delay",
        "GetRule",
        &fetched,
        "LockEndTime",
        true,
    );
}
