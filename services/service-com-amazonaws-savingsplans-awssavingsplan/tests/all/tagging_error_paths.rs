// SPDX-License-Identifier: CC0-1.0

//! The two error paths every `resourceArn`-taking operation shares: how a
//! refused member is NAMED, and how an unknown resource is REPORTED.
//!
//! Both messages are built from the request, so both are part of the contract:
//!
//! * A constraint violation is stated in the AWS envelope
//!   (`N validation error(s) detected: …`) with the member path rooted at the
//!   REQUEST OBJECT — `originalRequest.<member>` — whatever the constraint
//!   class, whatever the operation and however deep the member sits. The
//!   `@pattern` requirement is quoted as the model writes it, anchors and all.
//! * A well-formed `resourceArn` that names no plan of ours is a 404
//!   `ResourceNotFoundException` whose message names the ARN the caller sent:
//!   `Resource with resourceArn: <arn> doesn't exist`.
//!
//! The ARN pattern anchors the partition as the literal `aws`, so an `aws-cn` /
//! `aws-us-gov` / `aws-iso-*` plan ARN is not a well-formed value of the shape
//! at all: it is refused at the boundary (a 400) rather than looked for in the
//! store (a 404).
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The expression `SavingsPlanArn` declares, VERBATIM — the contract a caller is
/// told to satisfy, so the clause quotes it exactly as the model spells it.
///
/// `pub(crate)` so the sibling suites of this test target quote ONE copy of the
/// model's expression rather than each repeating it.
///
/// Source: `smithy_query(query="shape",shape="SavingsPlanArn",
/// model_path="services/service-com-amazonaws-savingsplans-awssavingsplan/\
/// smithy-model/com-amazonaws-savingsplans-awssavingsplan.json")`.
pub(crate) const SAVINGS_PLAN_ARN_PATTERN: &str = r"^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$";

/// The expression `PaginationToken` declares, from the same model query — a
/// second `@pattern` member, on a different operation, to show the rule is not
/// about `resourceArn`.
const PAGINATION_TOKEN_PATTERN: &str = r"^[A-Za-z0-9/=\+]+$";

/// One clause of the AWS validation envelope: a value that was refused, the
/// member it sat at, and the requirement it failed.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The whole message for a request with exactly one violation.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The requirement a `@pattern` member states.
fn must_match(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// A `resourceArn` the model's pattern refuses is refused at the boundary with
/// the member path rooted at the request object, on every operation that takes
/// one — and the clause quotes the model's own expression.
#[tokio::test]
async fn a_refused_resource_arn_is_named_from_the_request_object_inwards() {
    let emulator = Emulator::new();

    // (case, resourceArn) — one row per way an ARN can miss the pattern: not an
    // ARN at all, absent partition/account/region shape, a partition other than
    // the literal `aws` the pattern pins, and each UUID field the pattern
    // constrains (`[1-5]` version, `[89ab]` variant).
    let refused: [(&str, &str); 11] = [
        ("not an ARN at all", "not-an-arn"),
        ("empty", ""),
        (
            "no arn prefix",
            "savingsplan/11111111-1111-1111-8111-111111111111",
        ),
        (
            "an upper-case partition",
            "arn:AWS:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        ),
        (
            "the China partition",
            "arn:aws-cn:savingsplans::000000000000:savingsplan/33333333-3333-3333-a333-333333333333",
        ),
        (
            "the GovCloud partition",
            "arn:aws-us-gov:savingsplans::123456789012:savingsplan/44444444-4444-4444-b444-444444444444",
        ),
        (
            "an isolated partition, with a region",
            "arn:aws-iso-b:savingsplans:eu-west-1:999999999999:savingsplan/55555555-5555-5555-8555-555555555555",
        ),
        (
            "an account id that is not 12 digits",
            "arn:aws:savingsplans::0:savingsplan/11111111-1111-1111-8111-111111111111",
        ),
        (
            "a resource id that is not a UUID",
            "arn:aws:savingsplans::000000000000:savingsplan/not-a-uuid",
        ),
        (
            "a UUID version outside 1-5",
            "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-6111-8111-111111111111",
        ),
        (
            "a UUID variant outside 8-b",
            "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-c111-111111111111",
        ),
    ];

    for (case, arn) in refused {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let mut request = json!({ "resourceArn": arn });
            // Each operation's other required members, so the only violation is
            // the one the row is about.
            if operation == "TagResource" {
                request["tags"] = json!({ "k": "v" });
            }
            if operation == "UntagResource" {
                request["tagKeys"] = json!(["k"]);
            }
            let answer = emulator.call(operation, request).await;
            let expected = one_error(&clause(
                arn,
                "originalRequest.resourceArn",
                &must_match(SAVINGS_PLAN_ARN_PATTERN),
            ));
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code, "ValidationException",
                "case {case:?} on {operation}: expected ValidationException, got {:?}",
                answer.code
            );
            assert_eq!(
                text(&answer.body, "message"),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// The request-rooted member path is a property of this service's validator,
/// not of one member or one constraint class: an absent required member, a
/// numeric bound, a second `@pattern` member and a member nested inside a list
/// are all named the same way.
#[tokio::test]
async fn every_constraint_class_names_its_member_from_the_request_object() {
    let emulator = Emulator::new();

    // (case, operation, request, the whole expected message)
    let rows: [(&str, &str, Value, String); 5] = [
        (
            "an absent required member",
            "ListTagsForResource",
            json!({}),
            one_error(
                "Value null at 'originalRequest.resourceArn' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "an absent required member of another operation",
            "TagResource",
            json!({ "tags": { "k": "v" } }),
            one_error(
                "Value null at 'originalRequest.resourceArn' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a numeric bound above the maximum",
            "DescribeSavingsPlans",
            json!({ "maxResults": 1001 }),
            one_error(&clause(
                "1001",
                "originalRequest.maxResults",
                "Member must have value less than or equal to 1000",
            )),
        ),
        (
            "another pattern member",
            "DescribeSavingsPlans",
            json!({ "nextToken": "not a token!" }),
            one_error(&clause(
                "not a token!",
                "originalRequest.nextToken",
                &must_match(PAGINATION_TOKEN_PATTERN),
            )),
        ),
        (
            "a member nested inside a list",
            "DescribeSavingsPlans",
            json!({
                "savingsPlanArns": [
                    "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
                    "not-an-arn",
                ],
            }),
            one_error(&clause(
                "not-an-arn",
                "originalRequest.savingsPlanArns.2.member",
                &must_match(SAVINGS_PLAN_ARN_PATTERN),
            )),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let answer = emulator.call(operation, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} on {operation}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} on {operation}: expected ValidationException, got {:?}",
            answer.code
        );
        assert_eq!(
            text(&answer.body, "message"),
            expected,
            "case {case:?} on {operation}: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// A request that breaks two rules at once is stated in the plural envelope,
/// with EVERY clause rooted at the request object.
#[tokio::test]
async fn several_violations_are_each_rooted_at_the_request_object() {
    let emulator = Emulator::new();
    let answer = emulator
        .call(
            "DescribeSavingsPlans",
            json!({ "maxResults": 0, "nextToken": "not a token!" }),
        )
        .await;
    let message = text(&answer.body, "message");

    assert_eq!(
        answer.status, 400,
        "two violations: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        message.starts_with("2 validation errors detected: "),
        "two violations: expected the plural envelope, got {message:?}"
    );
    for expected in [
        clause(
            "0",
            "originalRequest.maxResults",
            "Member must have value greater than or equal to 1",
        ),
        clause(
            "not a token!",
            "originalRequest.nextToken",
            &must_match(PAGINATION_TOKEN_PATTERN),
        ),
    ] {
        assert!(
            message.contains(&expected),
            "two violations: expected a clause {expected:?}, got {message:?}"
        );
    }
}

/// A well-formed Savings Plan ARN that names no plan of ours is a 404 naming
/// the ARN the caller sent — for every operation that takes a `resourceArn` and
/// reads the store, and whatever the fields the pattern leaves open say.
#[tokio::test]
async fn an_unknown_plan_is_reported_with_the_arn_the_caller_sent() {
    let emulator = Emulator::new();

    // (case, resourceArn) — the fields `SavingsPlanArn` leaves a caller free to
    // vary, spanned: the region alternative `([a-z]{2}-[a-z]+-\d{1}|)`, the
    // account id, and the `[1-5]` version and `[89ab]` variant of the uuid.
    let unknown: [(&str, &str); 5] = [
        (
            "no region",
            "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        ),
        (
            "a region",
            "arn:aws:savingsplans:us-east-1:000000000000:savingsplan/22222222-2222-2222-9222-222222222222",
        ),
        (
            "another account id",
            "arn:aws:savingsplans::123456789012:savingsplan/33333333-3333-3333-a333-333333333333",
        ),
        (
            "the last uuid version and variant the pattern admits",
            "arn:aws:savingsplans::999999999999:savingsplan/44444444-4444-5444-b444-444444444444",
        ),
        (
            "a long region name",
            "arn:aws:savingsplans:ap-southeast-1:000000000000:savingsplan/55555555-5555-1555-8555-555555555555",
        ),
    ];

    for (case, arn) in unknown {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let mut request = json!({ "resourceArn": arn });
            if operation == "TagResource" {
                request["tags"] = json!({ "k": "v" });
            }
            if operation == "UntagResource" {
                request["tagKeys"] = json!(["k"]);
            }
            let answer = emulator.call(operation, request).await;
            let expected = format!("Resource with resourceArn: {arn} doesn't exist");
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code, "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {:?}",
                answer.code
            );
            assert_eq!(
                text(&answer.body, "message"),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// The control for the two error paths above: a plan that DOES exist is still
/// tagged and read back through its own ARN, so neither the pattern refusal nor
/// the not-found template swallowed the success path.
#[tokio::test]
async fn an_existing_plans_arn_still_round_trips_through_the_tagging_operations() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

    let tagged = emulator
        .call("TagResource", json!({ "resourceArn": arn, "tags": { "env": "test" } }))
        .await;
    assert_eq!(
        tagged.status, 200,
        "tagging {arn}: expected 200, got {} {:?}",
        tagged.status, tagged.body
    );

    let listed = emulator
        .call("ListTagsForResource", json!({ "resourceArn": arn }))
        .await;
    assert_eq!(
        listed.status, 200,
        "listing tags of {arn}: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.get("tags"),
        &json!({ "env": "test" }),
        "listing tags of {arn}: expected the tag written, got {:?}",
        listed.body
    );
}

/// `UntagResource` removes exactly the keys it names and nothing else, and what
/// it removed is gone from both readers.
///
/// The keys a request names are what the resource must END UP without, so a key
/// the plan never carried is not an error and a repeated request is a no-op.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, tagKeys, the whole tag set expected afterwards) — each row starts
    // from the same four stored tags, so the rows differ only in which keys
    // they name: none, one, several, one that is not there, a repeat, all.
    let stored = json!({ "a": "1", "b": "2", "c": "3", "d": "4" });
    let rows: [(&str, Value, Value); 6] = [
        ("no keys at all", json!([]), stored.clone()),
        (
            "one key",
            json!(["b"]),
            json!({ "a": "1", "c": "3", "d": "4" }),
        ),
        ("several keys", json!(["a", "d"]), json!({ "b": "2", "c": "3" })),
        (
            "a key the plan does not carry",
            json!(["never-set"]),
            stored.clone(),
        ),
        (
            "the same key twice",
            json!(["c", "c"]),
            json!({ "a": "1", "b": "2", "d": "4" }),
        ),
        ("every key", json!(["a", "b", "c", "d"]), json!({})),
    ];

    for (case, tag_keys, expected) in rows {
        // A fresh plan per row, so each row untags the same starting set.
        let savings_plan_id = emulator
            .purchase(json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": "0.1",
                "tags": stored,
            }))
            .await;
        let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

        let answer = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": tag_keys }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let listed = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await;
        assert_eq!(
            listed.get("tags"),
            &expected,
            "case {case:?}: ListTagsForResource must report {expected:?}, got {:?}",
            listed.body
        );
        assert_eq!(
            emulator.plan(&savings_plan_id).await.get("tags"),
            Some(&expected),
            "case {case:?}: DescribeSavingsPlans must report {expected:?}"
        );

        // Repeating the same removal changes nothing more.
        let again = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": tag_keys }),
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {case:?} repeated: expected 200, got {} {:?}",
            again.status, again.body
        );
        let listed_again = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await;
        assert_eq!(
            listed_again.get("tags"),
            &expected,
            "case {case:?} repeated: expected {expected:?}, got {:?}",
            listed_again.body
        );
    }
}
