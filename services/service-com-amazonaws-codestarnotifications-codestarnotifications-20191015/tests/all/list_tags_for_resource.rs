// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` and `UntagResource` over the wire: what the tag listing of a
//! notification rule reports, and what removing keys from one leaves behind.
//!
//! The RULES:
//!
//! * `ListTagsForResource` REPORTS the tag map the rule the `Arn` names holds — the same
//!   map `TagResource` left there and `DescribeNotificationRule` reports — as an empty map
//!   when the rule holds no tags, and it changes nothing a later caller sees;
//! * `UntagResource` removes exactly the keys the request names from that rule: a key the
//!   rule carries leaves it, a key it does not carry is nothing to remove, and every other
//!   key stays;
//! * both address the rule by ARN, so a value outside the notification-rule ARN form is
//!   HTTP 403 `AccessDeniedException` and a well-formed ARN this account and region holds
//!   no rule for is HTTP 404 `ResourceNotFoundException` over the whole submitted ARN —
//!   the answers every other ARN-addressed operation of this service gives.
//!
//! Each suite asserts a rule over a TABLE whose rows differ in what it generalises over
//! (how many tags the rule holds, the shape of the key and the value, how many keys are
//! removed, whether they are held, which field of the ARN form a value breaks, the
//! region), rather than pinning one recorded request.
//!
//! The harness (`Emulator`, `error_code`, `message`, `string`, `tags_of`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;

use crate::notification_rule_lifecycle::{error_code, message, string, tags_of, Emulator};

/// A well-formed `CreateNotificationRule` body, tagged with `tags`.
fn create_body(name: &str, tags: &[(&str, &str)]) -> Value {
    let mut body = json!({
        "Name": name,
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:ListTagsRepo",
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

/// A pair list as the comparable map a listing, a tagging answer and a describe are all
/// read into.
fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One value as a URI component: every byte outside the unreserved set travels as the
/// `%XX` of that byte, which is how an ARN's `:` and `/` stay inside ONE path segment
/// (`UntagResource` binds `Arn` to `@httpLabel` and `TagKeys` to `@httpQuery`).
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The `UntagResource` request line for `arn` and `keys`.
///
/// `pub(crate)`: every suite of this test target that drives `UntagResource` builds the
/// request line through this ONE reading of the `@httpLabel` / `@httpQuery` encoding.
pub(crate) fn untag_path(arn: &str, keys: &[&str]) -> String {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect();
    if query.is_empty() {
        format!("/untagResource/{}", encoded(arn))
    } else {
        format!("/untagResource/{}?{}", encoded(arn), query.join("&"))
    }
}

/// The 403 text: fixed either side of the ARN exactly as submitted.
fn not_authorized(arn: &str) -> String {
    format!(
        "You don't have sufficient permissions to perform operations on the following \
         resource: {arn}. For more information, see the notifications feature \
         documentation in the user guide for the Developer Tools console."
    )
}

/// The 404 text the TAGGING paths state: the whole submitted ARN, which is how they
/// address the resource.
fn no_resource(arn: &str) -> String {
    format!(
        "The following resource ARN was not found: {arn}. Make sure that the resource ARN \
         is correct and then try again."
    )
}

/// RULE: the listing reports the tag map the rule holds — the map the create and the
/// tagging calls left on it, and the map a describe reports — as an empty map for a rule
/// that holds none.
///
/// The rows differ in the tags the rule is created with, the tags later merged in, and so
/// in the count and the shape of what the listing must report: no tags at all, one, several,
/// a key already held taking a new value, an empty value (`TagValue` is length 0..256) and
/// a key carrying the characters `TagKey`'s pattern allows.
#[tokio::test]
async fn the_listing_reports_the_tags_the_rule_holds() {
    // (case, tags at create, tags tagged on afterwards, the map the listing must report)
    type Row<'a> = (
        &'a str,
        &'a [(&'a str, &'a str)],
        &'a [(&'a str, &'a str)],
        &'a [(&'a str, &'a str)],
    );
    let rows: &[Row] = &[
        ("a rule with no tags at all", &[], &[], &[]),
        (
            "a rule tagged at create only",
            &[("team", "dev")],
            &[],
            &[("team", "dev")],
        ),
        (
            "a rule tagged after create only",
            &[],
            &[("env", "prod")],
            &[("env", "prod")],
        ),
        (
            "several tags from both paths",
            &[("team", "dev")],
            &[("env", "prod"), ("owner", "platform")],
            &[("env", "prod"), ("owner", "platform"), ("team", "dev")],
        ),
        (
            "a key the rule already held, re-tagged",
            &[("team", "dev")],
            &[("team", "platform")],
            &[("team", "platform")],
        ),
        (
            "a tag with an empty value",
            &[],
            &[("marker", "")],
            &[("marker", "")],
        ),
        (
            "a key carrying the characters TagKey's pattern allows",
            &[("cost.center/eu=1", "finance@example")],
            &[("unit name", "platform team")],
            &[
                ("cost.center/eu=1", "finance@example"),
                ("unit name", "platform team"),
            ],
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, initial, added, expected)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("listed-{index}"), initial),
            )
            .await;
        let arn = string(&created, "Arn");

        if !added.is_empty() {
            emulator
                .ok(
                    case,
                    "/tagResource",
                    &json!({
                        "Arn": arn,
                        "Tags": Value::Object(
                            added
                                .iter()
                                .map(|(key, value)| ((*key).to_string(), json!(value)))
                                .collect(),
                        ),
                    }),
                )
                .await;
        }

        let want = tag_map(expected);
        let listed = emulator
            .ok(case, "/listTagsForResource", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&listed),
            want,
            "case {case:?}: expected the listing to report {want:?}, got {listed}"
        );
        assert!(
            listed.get("Tags").is_some(),
            "case {case:?}: Tags is an empty map rather than an absent member, got {listed}"
        );

        // The listing and the describe report ONE map: both read the same stored rule.
        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&listed),
            tags_of(&described),
            "case {case:?}: the describe reports {described}, the listing {listed}"
        );
    }
}

/// RULE: the listing is a REPORT — reading it leaves the rule, and what every later caller
/// sees, exactly as it was.
///
/// The rows differ in how many tags the rule holds, so nothing about the invariant is keyed
/// to one map.
#[tokio::test]
async fn listing_the_tags_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    for (case, tags) in [
        ("an untagged rule", &[][..]),
        ("one tag", &[("team", "dev")][..]),
        (
            "several tags",
            &[("team", "dev"), ("env", "prod"), ("release", "1")][..],
        ),
    ] {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("stable-{}", tags.len()), tags),
            )
            .await;
        let arn = string(&created, "Arn");
        let before = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;

        let mut seen = Vec::new();
        for _ in 0..3 {
            let listed = emulator
                .ok(case, "/listTagsForResource", &json!({ "Arn": arn }))
                .await;
            seen.push(tags_of(&listed));
        }
        let want = tag_map(tags);
        for (round, reported) in seen.iter().enumerate() {
            assert_eq!(
                *reported, want,
                "case {case:?}: listing {round} must report {want:?}, got {reported:?}"
            );
        }

        let after = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            after, before,
            "case {case:?}: the rule must be unchanged by a listing, got {after} after {before}"
        );
    }
}

/// RULE: `UntagResource` removes exactly the keys the request names and leaves every other
/// key — so the map the listing then reports is what is left, and a repeated call reaches
/// the same state.
///
/// The rows differ in how many keys are removed, whether the rule holds them, and whether
/// anything is left: one of several, several at once, every key, a key the rule never held,
/// a key named twice in one call, and a key of a shape `TagKey`'s pattern allows.
#[tokio::test]
async fn untagging_removes_only_the_named_keys() {
    // (case, the tags the rule holds, the keys removed, the map expected afterwards)
    type Row<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str], &'a [(&'a str, &'a str)]);
    let rows: &[Row] = &[
        (
            "one key of several",
            &[("team", "dev"), ("env", "prod")],
            &["team"],
            &[("env", "prod")],
        ),
        (
            "several keys at once",
            &[("team", "dev"), ("env", "prod"), ("release", "1")],
            &["team", "release"],
            &[("env", "prod")],
        ),
        (
            "every key the rule holds",
            &[("team", "dev"), ("env", "prod")],
            &["team", "env"],
            &[],
        ),
        (
            "a key the rule never held",
            &[("team", "dev")],
            &["absent"],
            &[("team", "dev")],
        ),
        (
            "a held key beside one the rule never held",
            &[("team", "dev"), ("env", "prod")],
            &["env", "absent"],
            &[("team", "dev")],
        ),
        (
            "the same key named twice in one call",
            &[("team", "dev"), ("env", "prod")],
            &["team", "team"],
            &[("env", "prod")],
        ),
        (
            "a key carrying the characters TagKey's pattern allows",
            &[("cost.center/eu=1", "finance@example"), ("team", "dev")],
            &["cost.center/eu=1"],
            &[("team", "dev")],
        ),
        (
            "an untagged rule",
            &[],
            &["team"],
            &[],
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, held, removed, expected)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &create_body(&format!("untagged-{index}"), held),
            )
            .await;
        let arn = string(&created, "Arn");

        let want = tag_map(expected);
        // The same request twice: removal is by key, so the second call reaches the state
        // the first one left.
        for round in 0..2 {
            let (status, body) = emulator
                .post(&untag_path(&arn, removed), &json!({}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: untag round {round} expected 200, got {status} with {body}"
            );

            let listed = emulator
                .ok(case, "/listTagsForResource", &json!({ "Arn": arn }))
                .await;
            assert_eq!(
                tags_of(&listed),
                want,
                "case {case:?}: after untag round {round} expected {want:?}, got {listed}"
            );
            let described = emulator
                .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
                .await;
            assert_eq!(
                tags_of(&described),
                want,
                "case {case:?}: the describe must report {want:?}, got {described}"
            );
        }
    }
}

/// RULE: untagging is addressed at ONE rule, so it leaves every other rule's tags alone —
/// and a key removed can be tagged back on.
#[tokio::test]
async fn untagging_one_rule_leaves_every_other_rule_alone() {
    let emulator = Emulator::new();

    let mut arns = Vec::new();
    for index in 0..3 {
        let created = emulator
            .ok(
                "neighbour",
                "/createNotificationRule",
                &create_body(
                    &format!("untag-neighbour-{index}"),
                    &[("team", "dev"), ("env", "prod")],
                ),
            )
            .await;
        arns.push(string(&created, "Arn"));
    }

    let (status, body) = emulator
        .post(&untag_path(&arns[1], &["team"]), &json!({}))
        .await;
    assert_eq!(status, 200, "the untag must succeed, got {status} with {body}");

    for (index, arn) in arns.iter().enumerate() {
        let listed = emulator
            .ok("neighbour", "/listTagsForResource", &json!({ "Arn": arn }))
            .await;
        let expected = if index == 1 {
            tag_map(&[("env", "prod")])
        } else {
            tag_map(&[("env", "prod"), ("team", "dev")])
        };
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {index}: expected {expected:?}, got {listed}"
        );
    }

    // A removed key is tagged back on, and the listing reports it again.
    emulator
        .ok(
            "re-tag",
            "/tagResource",
            &json!({ "Arn": arns[1], "Tags": { "team": "platform" } }),
        )
        .await;
    let listed = emulator
        .ok("re-tag", "/listTagsForResource", &json!({ "Arn": arns[1] }))
        .await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "prod"), ("team", "platform")]),
        "a removed key is tagged back on, got {listed}"
    );
}

/// RULE: a rule is addressed through the endpoint of the region that holds it — so its own
/// region lists and untags it, and another region reports it absent.
#[tokio::test]
async fn a_rule_is_listed_and_untagged_through_its_own_region() {
    let emulator = Emulator::new();

    for (case, home, other) in [
        ("created in eu-west-1", "eu-west-1", "us-east-1"),
        ("created in ap-south-1", "ap-south-1", "us-west-2"),
    ] {
        let (status, created) = emulator
            .post_in(
                home,
                "/createNotificationRule",
                &create_body("regional-tags", &[("team", "dev"), ("env", "prod")]),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with {created}"
        );
        let arn = string(&created, "Arn");

        let (status, listed) = emulator
            .post_in(home, "/listTagsForResource", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the home region lists it, got {status} with {listed}"
        );
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("env", "prod"), ("team", "dev")]),
            "case {case:?}: got {listed}"
        );

        for (operation, path, body) in [
            ("ListTagsForResource", "/listTagsForResource".to_string(), json!({ "Arn": arn })),
            ("UntagResource", untag_path(&arn, &["team"]), json!({})),
        ] {
            let (status, refused) = emulator.post_in(other, &path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} from {other} reports it absent, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: {operation}: got {refused}"
            );
            let stated = message(&refused);
            let expected = no_resource(&arn);
            assert_eq!(
                stated, expected,
                "case {case:?}: {operation}: expected {expected:?}, got {stated:?}"
            );
        }

        // Neither refused call touched the rule the other region holds.
        let (status, listed) = emulator
            .post_in(home, "/listTagsForResource", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the home region still lists it, got {status} with {listed}"
        );
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("env", "prod"), ("team", "dev")]),
            "case {case:?}: a refused call must change nothing, got {listed}"
        );
    }
}

/// RULE: a rule holds a bounded number of tags, so a request naming more keys than that
/// names keys no rule can hold and is refused with the `LimitExceededException` the model
/// declares for the class — and a refused call removes nothing.
///
/// The rows are the bound itself, one under it and two beyond it, taken from the constant
/// rather than from a literal.
#[tokio::test]
async fn untagging_more_keys_than_a_rule_holds_is_refused() {
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
                &create_body(&format!("untag-bound-{count}"), &[("kept", "yes")]),
            )
            .await;
        let arn = string(&created, "Arn");

        let keys: Vec<String> = (0..count).map(|index| format!("key-{index}")).collect();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let (status, body) = emulator
            .post(&untag_path(&arn, &borrowed), &json!({}))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {count} keys must be accepted, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {count} keys must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceededException",
                "case {case:?}: got {body}"
            );
        }

        // Either way the key the rule really holds is still there: none of the keys above
        // names it.
        let listed = emulator
            .ok(case, "/listTagsForResource", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("kept", "yes")]),
            "case {case:?}: expected the rule's own tag to remain, got {listed}"
        );
    }
}

/// RULE: a well-formed rule ARN this account and region holds no rule for is HTTP 404
/// `ResourceNotFoundException` over the WHOLE submitted ARN — the sentence every tagging
/// path of this service states.
///
/// The rows differ in why the rule is absent (deleted, never created) and in the form of
/// the rule id, so the answer cannot be keyed to one value.
#[tokio::test]
async fn an_absent_rule_is_a_404_over_the_submitted_arn() {
    let emulator = Emulator::new();

    let created = emulator
        .ok(
            "deleted",
            "/createNotificationRule",
            &create_body("tags-to-delete", &[("team", "dev")]),
        )
        .await;
    let deleted_arn = string(&created, "Arn");
    emulator
        .ok(
            "deleted",
            "/deleteNotificationRule",
            &json!({ "Arn": deleted_arn }),
        )
        .await;

    let mut rows: Vec<(String, String)> =
        vec![("a rule that was deleted".to_string(), deleted_arn.clone())];
    for id in [
        "11111111-1111-4111-8111-111111111111",
        "r1",
        "a-notification-rule-id-of-some-length",
        "abcdef0123456789",
    ] {
        rows.push((
            format!("a rule that never existed ({id})"),
            format!("arn:aws:codestar-notifications:us-east-1:000000000000:notificationrule/{id}"),
        ));
    }

    for (case, arn) in &rows {
        for (operation, path, body) in [
            (
                "ListTagsForResource",
                "/listTagsForResource".to_string(),
                json!({ "Arn": arn }),
            ),
            ("UntagResource", untag_path(arn, &["team"]), json!({})),
        ] {
            let (status, refused) = emulator.post(&path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} expected 404, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: {operation}: got {refused}"
            );
            let stated = message(&refused);
            let expected = no_resource(arn);
            assert_eq!(
                stated, expected,
                "case {case:?}: {operation}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: a value outside the notification-rule ARN form names no resource of this service,
/// so nothing can authorize the caller for it: HTTP 403 `AccessDeniedException` over the
/// fixed text either side of the value exactly as submitted — the same answer every other
/// ARN-addressed operation of this service gives, and distinct from the 404 above.
///
/// The rows differ in WHICH field of the form the value breaks (the resource type, the
/// service namespace, the partition, the region, the account, the resource prefix, no ARN
/// at all).
#[tokio::test]
async fn an_arn_outside_the_rule_form_is_not_authorized() {
    let emulator = Emulator::new();

    // A rule that really exists, so a refusal cannot be mistaken for absence.
    let created = emulator
        .ok(
            "seed",
            "/createNotificationRule",
            &create_body("tags-authz-seed", &[("team", "dev")]),
        )
        .await;
    let arn = string(&created, "Arn");
    let id = arn
        .rsplit_once('/')
        .map(|(_, id)| id.to_string())
        .expect("a rule ARN carries its id");

    let malformed = [
        (
            "the resource-type segment is not notificationrule",
            format!("arn:aws:codestar-notifications:us-east-1:000000000000:wrongsegment/{id}"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:codepipeline:us-east-1:000000000000:notificationrule/{id}"),
        ),
        (
            "a partition that is not an aws one",
            format!("arn:notaws:codestar-notifications:us-east-1:000000000000:notificationrule/{id}"),
        ),
        (
            "an empty region",
            format!("arn:aws:codestar-notifications::000000000000:notificationrule/{id}"),
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:codestar-notifications:us-east-1:12345:notificationrule/{id}"),
        ),
        (
            "no resource prefix at all",
            format!("arn:aws:codestar-notifications:us-east-1:000000000000:{id}"),
        ),
        ("not an ARN at all", "not-an-arn".to_string()),
    ];

    for (form, value) in &malformed {
        for (operation, path, body) in [
            (
                "ListTagsForResource",
                "/listTagsForResource".to_string(),
                json!({ "Arn": value }),
            ),
            ("UntagResource", untag_path(value, &["team"]), json!({})),
        ] {
            let case = format!("{operation} with {form}");
            let (status, refused) = emulator.post(&path, &body).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "AccessDeniedException",
                "case {case:?}: got {refused}"
            );
            let stated = message(&refused);
            let expected = not_authorized(value);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // None of the refused calls touched the rule that exists.
    let listed = emulator
        .ok("seed", "/listTagsForResource", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("team", "dev")]),
        "a refused call must not have changed the rule's tags, got {listed}"
    );
}
