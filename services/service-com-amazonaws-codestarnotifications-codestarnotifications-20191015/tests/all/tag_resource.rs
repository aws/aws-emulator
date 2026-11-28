// SPDX-License-Identifier: CC0-1.0

//! `TagResource` over the wire: what tagging a notification rule does, and what it
//! answers for a rule that is not there.
//!
//! The RULE: tagging MERGES the supplied map into the rule the ARN names — a key the rule
//! already carries takes the new value, keys the request does not name are left alone —
//! and answers with the resulting map, which is the same map `DescribeNotificationRule`
//! then reports. An ARN no rule of this account and region holds is refused with 404
//! `ResourceNotFoundException` over the whole submitted ARN, whether the rule never
//! existed or was deleted.
//!
//! A further RULE, asserted here over both tagging paths: a call that leaves the rule
//! holding a different tag map MODIFIED the rule, so it moves the instant the rule reports
//! as the one it was most recently updated at, while a call that leaves the map as it was
//! moves nothing — and no report of the rule moves it at all.
//!
//! Each suite asserts that over a TABLE whose rows differ in what the rule generalises
//! over — how many keys are supplied, whether they are new or already held, the shape of
//! the key and the value — rather than pinning one recorded request.
//!
//! The harness (`Emulator`, `error_code`, `message`, `string`, `tags_of`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;

use crate::list_tags_for_resource::untag_path;
use crate::notification_rule_lifecycle::{error_code, message, string, tags_of, Emulator};

/// A well-formed `CreateNotificationRule` body.
fn create_body(name: &str, tags: &[(&str, &str)]) -> Value {
    let mut body = json!({
        "Name": name,
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:TagRepo",
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-comments-on-commits"],
        "Targets": [],
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// The tags of a `TagResource` answer or a describe, as a comparable map.
fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the merge table: the case, the tags the rule is created with, the tags the
/// call supplies, and the map expected afterwards.
type MergeRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
);

/// RULE: the supplied tags are merged into the rule and the resulting map is what the
/// call answers with — and what a later describe reports.
///
/// The rows differ in the tags the rule starts with, the tags supplied, and what the
/// merge therefore produces: a first tag on an untagged rule, several at once, a key the
/// rule already holds (the new value wins), a key with an empty value (`TagValue` is len
/// 0..256), and a call that supplies a key the rule already has beside a new one.
#[tokio::test]
async fn tagging_merges_the_supplied_tags_and_reports_the_result() {
    // (case, tags at create, tags supplied, the map expected afterwards)
    let rows: &[MergeRow] = &[
        (
            "a first tag on an untagged rule",
            &[],
            &[("team", "dev")],
            &[("team", "dev")],
        ),
        (
            "several tags at once",
            &[],
            &[("team", "dev"), ("env", "prod"), ("owner", "platform")],
            &[("env", "prod"), ("owner", "platform"), ("team", "dev")],
        ),
        (
            "a new key beside the ones the rule holds",
            &[("team", "dev")],
            &[("env", "prod")],
            &[("env", "prod"), ("team", "dev")],
        ),
        (
            "a key the rule already holds takes the new value",
            &[("team", "dev")],
            &[("team", "platform")],
            &[("team", "platform")],
        ),
        (
            "one key replaced and one added in the same call",
            &[("team", "dev"), ("env", "staging")],
            &[("env", "prod"), ("release", "1")],
            &[("env", "prod"), ("release", "1"), ("team", "dev")],
        ),
        (
            "a tag with an empty value",
            &[],
            &[("marker", "")],
            &[("marker", "")],
        ),
        (
            "a key carrying the characters TagKey's pattern allows",
            &[],
            &[("cost.center/eu=1", "finance@example")],
            &[("cost.center/eu=1", "finance@example")],
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, initial, supplied, expected)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("tagged-{index}"), initial),
            )
            .await;
        let arn = string(&created, "Arn");

        let tagged = emulator
            .ok(
                case,
                "/tagResource",
                &json!({
                    "Arn": arn,
                    "Tags": Value::Object(
                        supplied
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), json!(value)))
                            .collect(),
                    ),
                }),
            )
            .await;
        let answered = tags_of(&tagged);
        let want = tag_map(expected);
        assert_eq!(
            answered, want,
            "case {case:?}: expected the resulting Tags {want:?}, got {tagged}"
        );

        // Round trip: the rule itself now holds exactly that map.
        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&described),
            want,
            "case {case:?}: the describe must report the merged map, got {described}"
        );
    }
}

/// RULE: tagging is addressed at the rule, so it changes no OTHER rule — and a second
/// call adds to what the first one left.
#[tokio::test]
async fn tagging_one_rule_leaves_every_other_rule_alone() {
    let emulator = Emulator::new();

    let mut arns = Vec::new();
    for index in 0..3 {
        let created = emulator
            .ok(
                "neighbour",
                "/createNotificationRule",
                &create_body(&format!("neighbour-{index}"), &[("kept", "yes")]),
            )
            .await;
        arns.push(string(&created, "Arn"));
    }

    emulator
        .ok(
            "first call",
            "/tagResource",
            &json!({ "Arn": arns[1], "Tags": { "step": "one" } }),
        )
        .await;
    let second = emulator
        .ok(
            "second call",
            "/tagResource",
            &json!({ "Arn": arns[1], "Tags": { "step": "two", "extra": "yes" } }),
        )
        .await;
    assert_eq!(
        tags_of(&second),
        tag_map(&[("extra", "yes"), ("kept", "yes"), ("step", "two")]),
        "a second call adds to what the first left, got {second}"
    );

    for (index, arn) in arns.iter().enumerate() {
        let described = emulator
            .ok("neighbour", "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        let expected = if index == 1 {
            tag_map(&[("extra", "yes"), ("kept", "yes"), ("step", "two")])
        } else {
            tag_map(&[("kept", "yes")])
        };
        assert_eq!(
            tags_of(&described),
            expected,
            "case {index}: expected {expected:?}, got {described}"
        );
    }
}

/// RULE: an ARN of this service's own form that names no rule of this account and region
/// is refused with 404 `ResourceNotFoundException`, stating the WHOLE submitted ARN — and
/// that is a different sentence from the one the describe path states, which names the
/// rule by the id its ARN carries.
///
/// The rows differ in why the rule is absent (deleted, never created, created in another
/// region) and in the form of the rule id, so the message cannot be keyed to one ARN.
#[tokio::test]
async fn tagging_an_absent_rule_is_a_404_over_the_submitted_arn() {
    let emulator = Emulator::new();

    // A rule that was created and then deleted.
    let created = emulator
        .ok("deleted", "/createNotificationRule", &create_body("to-delete", &[]))
        .await;
    let deleted_arn = string(&created, "Arn");
    emulator
        .ok(
            "deleted",
            "/deleteNotificationRule",
            &json!({ "Arn": deleted_arn }),
        )
        .await;

    // A rule that lives in another region, addressed through this one.
    let (status, elsewhere) = emulator
        .post_in(
            "eu-west-1",
            "/createNotificationRule",
            &create_body("elsewhere", &[]),
        )
        .await;
    assert_eq!(
        status, 200,
        "the other region's create must succeed, got {status} with {elsewhere}"
    );
    let other_region_arn = string(&elsewhere, "Arn");

    let mut rows: Vec<(String, String)> = vec![
        ("a rule that was deleted".to_string(), deleted_arn.clone()),
        (
            "a rule of another region".to_string(),
            other_region_arn.clone(),
        ),
    ];
    for id in [
        "11111111-1111-4111-8111-111111111111",
        "r1",
        "a-notification-rule-id-of-some-length",
    ] {
        rows.push((
            format!("a rule that never existed ({id})"),
            format!("arn:aws:codestar-notifications:us-east-1:000000000000:notificationrule/{id}"),
        ));
    }

    for (case, arn) in &rows {
        let (status, body) = emulator
            .post("/tagResource", &json!({ "Arn": arn, "Tags": { "late": "tag" } }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
        let stated = message(&body);
        let expected = format!(
            "The following resource ARN was not found: {arn}. Make sure that the resource ARN \
             is correct and then try again."
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The rule that does exist elsewhere was not tagged by any of the above.
    let described = emulator
        .post_in(
            "eu-west-1",
            "/describeNotificationRule",
            &json!({ "Arn": other_region_arn }),
        )
        .await;
    assert!(
        described
            .1
            .get("Tags")
            .and_then(Value::as_object)
            .is_some_and(serde_json::Map::is_empty),
        "a refused tagging must not have tagged the rule, got {:?}",
        described.1
    );
}

/// The refusal text the service states for submitted keys carrying the reserved prefix:
/// one fixed sentence, whose ONLY variable part is the bracketed list of those keys. The
/// list here is written out literally rather than taken from the service, so the wording
/// is asserted and not merely echoed.
fn reserved_message(keys: &[&str]) -> String {
    format!(
        "One or more of the following tag keys is reserved: [{}]. Tag keys cannot begin \
         with the characters \"aws:\". For more information, see the AWS CodeStar \
         Notifications API Reference.",
        keys.join(", ")
    )
}

/// One row of the reserved-key table: the case, the tags the call supplies, and the keys
/// the refusal must list.
type ReservedRow<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);

/// RULE: a tag key that begins with the reserved prefix `aws:` is refused with HTTP 400
/// `ValidationException` stating the service's one text for the class, naming EVERY
/// submitted key that carries the prefix, and a refused call tags nothing.
///
/// The rows differ in how many submitted keys carry the prefix, how many ordinary keys
/// travel beside them, what follows the prefix, and the casing of the prefix — the
/// dimensions the refusal generalises over; the expected list is the submitted keys as
/// submitted, in key order.
#[tokio::test]
async fn a_tag_key_carrying_the_reserved_prefix_is_refused_in_the_services_words() {
    // (case, tags supplied, the keys the refusal must list)
    let rows: &[ReservedRow] = &[
        (
            "one reserved key",
            &[("aws:reserved", "nope")],
            &["aws:reserved"],
        ),
        (
            "a reserved key naming an AWS-managed namespace",
            &[("aws:cloudformation:stack-name", "stack")],
            &["aws:cloudformation:stack-name"],
        ),
        (
            "a reserved key beside an ordinary one",
            &[("aws:owner", "x"), ("team", "dev")],
            &["aws:owner"],
        ),
        (
            "two reserved keys, listed in key order",
            &[("aws:second", "2"), ("aws:first", "1")],
            &["aws:first", "aws:second"],
        ),
        (
            "every submitted key reserved, beside two ordinary ones",
            &[
                ("aws:c", "3"),
                ("aws:a", "1"),
                ("env", "prod"),
                ("aws:b", "2"),
                ("owner", "platform"),
            ],
            &["aws:a", "aws:b", "aws:c"],
        ),
        (
            "the prefix spelled in upper case",
            &[("AWS:Owner", "x")],
            &["AWS:Owner"],
        ),
        (
            "the prefix with nothing after it",
            &[("aws:", "x")],
            &["aws:"],
        ),
    ];

    let emulator = Emulator::new();
    let created = emulator
        .ok("reserved", "/createNotificationRule", &create_body("reserved", &[]))
        .await;
    let arn = string(&created, "Arn");

    for (case, supplied, listed) in rows {
        let (status, body) = emulator
            .post(
                "/tagResource",
                &json!({
                    "Arn": arn,
                    "Tags": Value::Object(
                        supplied
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), json!(value)))
                            .collect(),
                    ),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: got {body}"
        );
        let stated = message(&body);
        let expected = reserved_message(listed);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // A refused call tags NOTHING: not the reserved key, and not the ordinary keys that
    // travelled beside it.
    let described = emulator
        .ok("reserved", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        tags_of(&described),
        BTreeMap::new(),
        "a refused tagging must have stored nothing, got {described}"
    );
}

/// RULE: only the four characters `aws:` are reserved, so a key that begins with the
/// letters `aws` without the colon — or carries the prefix anywhere but at the start — is
/// an ordinary key: it is accepted, stored, and read back through
/// `ListTagsForResource`.
///
/// The rows differ in where the letters sit in the key and in their casing. Each key
/// stays inside `TagKey` (`@length(min: 1, max: 128)`, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
#[tokio::test]
async fn a_tag_key_that_only_resembles_the_reserved_prefix_is_accepted() {
    let keys = [
        "aws",
        "awsome",
        "AWSteam",
        "awsome-team",
        "aws-owner",
        "aws.account",
        "aws/team",
        "team-aws",
        "lawsuit",
        "my-aws-thing",
        "team:aws:owner",
    ];

    let emulator = Emulator::new();
    let created = emulator
        .ok("accepted", "/createNotificationRule", &create_body("accepted", &[]))
        .await;
    let arn = string(&created, "Arn");

    let mut expected = BTreeMap::new();
    for key in keys {
        let tagged = emulator
            .ok(
                key,
                "/tagResource",
                &json!({ "Arn": arn, "Tags": { key: "ok" } }),
            )
            .await;
        expected.insert(key.to_string(), "ok".to_string());
        assert_eq!(
            tags_of(&tagged),
            expected,
            "case {key:?}: a key outside the reserved prefix is accepted, got {tagged}"
        );
    }

    // Round trip: the rule holds every accepted key, as the tag listing reports it.
    let listed = emulator
        .ok("accepted", "/listTagsForResource", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        tags_of(&listed),
        expected,
        "the listing must report every accepted key, got {listed}"
    );
}

/// RULE: the reserved-prefix test and its wording belong to the tag map, not to one
/// operation, so the CREATE path refuses the same keys with the same sentence and accepts
/// the same ordinary ones.
#[tokio::test]
async fn the_create_path_states_the_same_reserved_key_refusal() {
    let emulator = Emulator::new();

    for (index, (case, supplied, listed)) in [
        (
            "one reserved key",
            &[("aws:created-by", "cfn")][..],
            &["aws:created-by"][..],
        ),
        (
            "two reserved keys beside an ordinary one",
            &[("aws:b", "2"), ("team", "dev"), ("aws:a", "1")][..],
            &["aws:a", "aws:b"][..],
        ),
    ]
    .iter()
    .enumerate()
    {
        let (status, body) = emulator
            .post(
                "/createNotificationRule",
                &create_body(&format!("create-reserved-{index}"), supplied),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: got {body}"
        );
        let stated = message(&body);
        let expected = reserved_message(listed);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The same create, with keys that only resemble the prefix, carries its tags.
    let created = emulator
        .ok(
            "create accepted",
            "/createNotificationRule",
            &create_body("create-accepted", &[("awsome", "yes"), ("AWSteam", "yes")]),
        )
        .await;
    let arn = string(&created, "Arn");
    let listed = emulator
        .ok("create accepted", "/listTagsForResource", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("AWSteam", "yes"), ("awsome", "yes")]),
        "the created rule must hold the accepted keys, got {listed}"
    );
}

/// RULE: a rule holds a bounded number of tags, so a caller cannot grow one without
/// limit; the bound is this service's and the refusal is the `LimitExceededException` the
/// model declares for the class.
///
/// The rows are the bound itself, one under it and one over it, taken from the constant
/// rather than from a literal.
#[tokio::test]
async fn a_rule_holds_a_bounded_number_of_tags() {
    let emulator = Emulator::new();
    let max = codestar::state::MAX_TAGS_PER_RULE;

    for (case, count, accepted) in [
        ("one under the bound", max - 1, true),
        ("exactly the bound", max, true),
        ("one over the bound", max + 1, false),
        ("far over the bound", max * 4, false),
    ] {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("bounded-{count}"), &[]),
            )
            .await;
        let arn = string(&created, "Arn");
        let tags: serde_json::Map<String, Value> = (0..count)
            .map(|index| (format!("key-{index}"), json!("v")))
            .collect();

        let (status, body) = emulator
            .post(
                "/tagResource",
                &json!({ "Arn": arn, "Tags": Value::Object(tags) }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {count} tags must be accepted, got {status} with {body}"
            );
            assert_eq!(
                tags_of(&body).len(),
                count,
                "case {case:?}: expected {count} tags, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {count} tags must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceededException",
                "case {case:?}: got {body}"
            );
            let described = emulator
                .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
                .await;
            assert!(
                tags_of(&described).is_empty(),
                "case {case:?}: a refused call must store nothing, got {described}"
            );
        }
    }
}

/// The two instants a rule reports, as `DescribeNotificationRule` states them:
/// `(CreatedTimestamp, LastModifiedTimestamp)`. Both travel as epoch seconds, so one
/// call's instant compares with another's as a number.
async fn timestamps(emulator: &Emulator, case: &str, arn: &str) -> (f64, f64) {
    let described = emulator
        .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    let read = |member: &str| {
        described.get(member).and_then(Value::as_f64).unwrap_or_else(|| {
            panic!("case {case:?}: the describe must report {member}, got {described}")
        })
    };
    (read("CreatedTimestamp"), read("LastModifiedTimestamp"))
}

/// Tag the rule and return the status and body of the call.
async fn tag(emulator: &Emulator, arn: &str, tags: &[(&str, &str)]) -> (u16, Value) {
    emulator
        .post(
            "/tagResource",
            &json!({
                "Arn": arn,
                "Tags": Value::Object(
                    tags.iter()
                        .map(|(key, value)| ((*key).to_string(), json!(value)))
                        .collect(),
                ),
            }),
        )
        .await
}

/// One row of the tagging-bump table: the case, the tags the rule is created with, the
/// tags the call supplies, the map the rule holds afterwards, and whether that map
/// differs from the one it held — which is whether the call modified the rule.
type BumpRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    bool,
);

/// RULE: a call that leaves a rule holding a different tag map MODIFIED that rule, so the
/// instant it reports as the one it was most recently updated at moves to that write
/// (`LastModifiedTimestamp` is "the date and time the notification rule was most recently
/// updated"); a call that leaves the map exactly as it was modified nothing, so that
/// instant stays where it was. `CreatedTimestamp` never moves either way.
///
/// The rows differ in what the merge does to the stored map — a first key, several at
/// once, a key beside the ones already held, a held key taking a new value, a key with an
/// empty value — and in whether it changes the map at all: re-supplying the map the rule
/// already holds, or a subset of it with the values it already carries, changes nothing.
#[tokio::test]
async fn tagging_that_changes_the_map_moves_the_rules_last_modified_instant() {
    // (case, tags at create, tags supplied, the map held afterwards, the map changed)
    let rows: &[BumpRow] = &[
        (
            "a first tag on an untagged rule",
            &[],
            &[("team", "dev")],
            &[("team", "dev")],
            true,
        ),
        (
            "several tags at once",
            &[],
            &[("team", "dev"), ("env", "prod"), ("owner", "platform")],
            &[("env", "prod"), ("owner", "platform"), ("team", "dev")],
            true,
        ),
        (
            "a new key beside the ones the rule holds",
            &[("team", "dev")],
            &[("env", "prod")],
            &[("env", "prod"), ("team", "dev")],
            true,
        ),
        (
            "a key the rule already holds taking a new value",
            &[("team", "dev")],
            &[("team", "platform")],
            &[("team", "platform")],
            true,
        ),
        (
            "a tag with an empty value",
            &[("team", "dev")],
            &[("marker", "")],
            &[("marker", ""), ("team", "dev")],
            true,
        ),
        (
            "the map the rule already holds, re-supplied",
            &[("team", "dev"), ("env", "prod")],
            &[("team", "dev"), ("env", "prod")],
            &[("env", "prod"), ("team", "dev")],
            false,
        ),
        (
            "a subset of the map, with the values it already carries",
            &[("team", "dev"), ("env", "prod")],
            &[("env", "prod")],
            &[("env", "prod"), ("team", "dev")],
            false,
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, initial, supplied, held, changed)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("bumped-{index}"), initial),
            )
            .await;
        let arn = string(&created, "Arn");
        let (created_before, modified_before) = timestamps(&emulator, case, &arn).await;

        let (status, body) = tag(&emulator, &arn, supplied).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the tagging must succeed, got {status} with {body}"
        );

        let want = tag_map(held);
        let (created_after, modified_after) = timestamps(&emulator, case, &arn).await;
        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&described),
            want,
            "case {case:?}: expected the rule to hold {want:?}, got {described}"
        );

        if *changed {
            assert!(
                modified_after > modified_before,
                "case {case:?}: a changed tag map moves LastModifiedTimestamp, \
                 got {modified_before} then {modified_after}"
            );
        } else {
            assert_eq!(
                modified_after, modified_before,
                "case {case:?}: an unchanged tag map leaves LastModifiedTimestamp alone, \
                 got {modified_before} then {modified_after}"
            );
        }
        assert_eq!(
            created_after, created_before,
            "case {case:?}: CreatedTimestamp never moves, got {created_before} then {created_after}"
        );
    }
}

/// One row of the untagging-bump table: the case, the tags the rule holds, the keys the
/// call names, the map left afterwards, and whether the call removed anything.
type UntagBumpRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
    bool,
);

/// RULE (the same one, through the removing path): a call that leaves a rule holding a
/// different tag map moves the instant the rule reports as the one it was most recently
/// updated at; a call that removes nothing leaves that instant alone.
///
/// The rows differ in how many of the named keys the rule holds — one of several, every
/// one of them, a key named twice, a held key beside one the rule never held — and in
/// whether anything is removed at all: a key the rule never held, and any key of a rule
/// that holds none, remove nothing.
#[tokio::test]
async fn untagging_that_removes_a_key_moves_the_rules_last_modified_instant() {
    // (case, tags held, keys named, the map left afterwards, something was removed)
    let rows: &[UntagBumpRow] = &[
        (
            "one key of several",
            &[("team", "dev"), ("env", "prod")],
            &["team"],
            &[("env", "prod")],
            true,
        ),
        (
            "every key the rule holds",
            &[("team", "dev"), ("env", "prod")],
            &["team", "env"],
            &[],
            true,
        ),
        (
            "the same key named twice in one call",
            &[("team", "dev"), ("env", "prod")],
            &["team", "team"],
            &[("env", "prod")],
            true,
        ),
        (
            "a held key beside one the rule never held",
            &[("team", "dev"), ("env", "prod")],
            &["env", "absent"],
            &[("team", "dev")],
            true,
        ),
        (
            "a key the rule never held",
            &[("team", "dev")],
            &["absent"],
            &[("team", "dev")],
            false,
        ),
        (
            "two keys the rule never held",
            &[("team", "dev")],
            &["absent", "also-absent"],
            &[("team", "dev")],
            false,
        ),
        (
            "any key, on a rule that holds none",
            &[],
            &["team"],
            &[],
            false,
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, held, keys, left, removed)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("untag-bumped-{index}"), held),
            )
            .await;
        let arn = string(&created, "Arn");
        let (created_before, modified_before) = timestamps(&emulator, case, &arn).await;

        let (status, body) = emulator.post(&untag_path(&arn, keys), &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the untagging must succeed, got {status} with {body}"
        );

        let want = tag_map(left);
        let (created_after, modified_after) = timestamps(&emulator, case, &arn).await;
        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&described),
            want,
            "case {case:?}: expected the rule to hold {want:?}, got {described}"
        );

        if *removed {
            assert!(
                modified_after > modified_before,
                "case {case:?}: a removed key moves LastModifiedTimestamp, \
                 got {modified_before} then {modified_after}"
            );
        } else {
            assert_eq!(
                modified_after, modified_before,
                "case {case:?}: removing nothing leaves LastModifiedTimestamp alone, \
                 got {modified_before} then {modified_after}"
            );
        }
        assert_eq!(
            created_after, created_before,
            "case {case:?}: CreatedTimestamp never moves, got {created_before} then {created_after}"
        );
    }
}

/// RULE: the instant belongs to the RULE, not to one operation — so every modifying call
/// moves it forward, whichever path writes, and a call that only REPORTS the rule leaves
/// it (and everything else a later caller sees) alone.
///
/// The sequence mixes the tagging paths with the siblings that already moved it, so no
/// step's bump is keyed to one operation; the reads between them must not move it.
#[tokio::test]
async fn every_modifying_call_moves_the_instant_and_no_read_does() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "sequence",
            "/createNotificationRule",
            &create_body("sequence", &[("team", "dev")]),
        )
        .await;
    let arn = string(&created, "Arn");

    // (what the step is called, the request line, the body) — one modifying call each,
    // alternating between the tagging paths and the siblings.
    let steps: Vec<(&str, String, Value)> = vec![
        (
            "tagResource adds a key",
            "/tagResource".to_string(),
            json!({ "Arn": arn, "Tags": { "env": "prod" } }),
        ),
        (
            "subscribe attaches a target",
            "/subscribe".to_string(),
            json!({
                "Arn": arn,
                "Target": {
                    "TargetType": "SNS",
                    "TargetAddress": "arn:aws:sns:us-east-1:123456789012:sequence-topic",
                },
            }),
        ),
        (
            "untagResource removes a key",
            untag_path(&arn, &["env"]),
            json!({}),
        ),
        (
            "updateNotificationRule changes the status",
            "/updateNotificationRule".to_string(),
            json!({ "Arn": arn, "Status": "DISABLED" }),
        ),
        (
            "tagResource replaces a value",
            "/tagResource".to_string(),
            json!({ "Arn": arn, "Tags": { "team": "platform" } }),
        ),
        (
            "untagResource removes the last key",
            untag_path(&arn, &["team"]),
            json!({}),
        ),
    ];

    let (created_at, mut previous) = timestamps(&emulator, "sequence", &arn).await;
    for (case, path, body) in &steps {
        let (status, answered) = emulator.post(path, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must succeed, got {status} with {answered}"
        );
        let (created_now, modified) = timestamps(&emulator, case, &arn).await;
        assert!(
            modified > previous,
            "case {case:?}: the instant must move forward, got {previous} then {modified}"
        );
        assert_eq!(
            created_now, created_at,
            "case {case:?}: CreatedTimestamp never moves, got {created_at} then {created_now}"
        );
        previous = modified;

        // A REPORT of the rule moves nothing a later caller sees.
        for (report, report_path) in [
            ("describeNotificationRule", "/describeNotificationRule"),
            ("listTagsForResource", "/listTagsForResource"),
        ] {
            emulator
                .ok(report, report_path, &json!({ "Arn": arn }))
                .await;
        }
        let (_, after_reads) = timestamps(&emulator, case, &arn).await;
        assert_eq!(
            after_reads, previous,
            "case {case:?}: reading the rule must not move the instant, \
             got {previous} then {after_reads}"
        );
    }
}
