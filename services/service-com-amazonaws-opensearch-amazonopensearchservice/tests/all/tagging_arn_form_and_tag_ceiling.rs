// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `ARN` the three tagging operations take, and for the sentence
//! the tag ceiling is stated with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`] and
//! the three tagging readers and writers from [`super::resource_tags`]; this suite adds no
//! second copy of either.
//!
//! The rules under test, each over a SET of inputs rather than one recorded call:
//!
//! * `ARN` is `@length(min: 20, max: 2048)` with pattern `.*`
//!   (`smithy_query(query="operation",operation="AddTags")`), so the request boundary
//!   enforces only the length and the SERVICE reads the value. It tags a domain, a data
//!   source and an application, each addressed by an ARN of its own namespace, so a value
//!   that cannot be read as one of those names nothing in any account: it is refused for its
//!   FORM with `ValidationException` (HTTP 400) and the service's fixed `Invalid ARN format.`
//!   — one wording for the whole class, over a table of ways a value fails to be such an ARN;
//! * that refusal is DISTINCT from the one for an ARN that IS one of this service's and
//!   names no resource the account holds, which keeps its own fixed sentence — so a caller
//!   can tell "this is not an ARN of this service" from "this account holds no such
//!   resource";
//! * both are stated by all three operations identically, so a reader and the two writers of
//!   one tag set cannot disagree about a value;
//! * the tag ceiling is stated with the service's fixed sentence, which carries no variable
//!   part — no ARN, no count, no limit — whichever call runs into it: `AddTags` on a resource
//!   that already carries tags, and the `TagList` a create carries.

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    MALFORMED_TAGGING_ARN, TAG_QUOTA, TAG_QUOTA_EXCEEDED,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};
use super::resource_tags::{
    add_tags, domain_arn, error_message, list_tags, stored_tags, tag, wire_tags,
};

/// `RemoveTags`: `POST /2021-01-01/tags-removal`, which this suite drives beside the two
/// calls [`super::resource_tags`] publishes.
async fn remove_tags(emulator: &Emulator, arn: &str, tag_keys: &[&str]) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/tags-removal"),
            Some(json!({ "ARN": arn, "TagKeys": tag_keys })),
        )
        .await
}

/// What each of the three tagging operations answers `arn` with — so one row asserts the
/// whole family rather than one operation.
async fn tagging_answers(emulator: &Emulator, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "AddTags",
            add_tags(emulator, arn, wire_tags(&[tag("env", "prod")])).await,
        ),
        ("RemoveTags", remove_tags(emulator, arn, &["env"]).await),
        ("ListTags", list_tags(emulator, arn).await),
    ]
}

#[tokio::test]
async fn a_value_that_is_no_arn_of_this_service_is_refused_for_its_form() {
    let emulator = Emulator::new();
    let live = domain_arn(&emulator, "arn-form-target").await;
    add_tags(&emulator, &live, wire_tags(&[tag("env", "prod")])).await;

    // Each row is at least the 20 characters the member's `@length` admits, so the modelled
    // constraint passes and the service itself is what refuses the value. The rows differ
    // along the ways a value fails to be an ARN of a resource this service tags: it carries
    // no ARN fields at all, too few of them, the wrong literal, an empty partition or
    // service, another vendor's namespace, a resource field with no id after its type, a
    // resource type this service tags nothing of, an account field that is no account id,
    // and a Region field that is no Region label.
    let malformed: Vec<(&str, String)> = vec![
        ("no fields at all", "x".repeat(20)),
        ("too few fields", "arn:aws:es:us-east-1:1".to_string()),
        (
            "not the arn literal",
            "urn:aws:es:us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "an empty partition",
            "arn::es:us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "an empty service",
            "arn:aws::us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "another vendor's namespace",
            "arn:aws:s3:us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "a bucket of another service",
            "arn:aws:s3:::a-bucket-of-another-service".to_string(),
        ),
        (
            "a resource type with no id after it",
            "arn:aws:es:us-east-1:123456789012:domain".to_string(),
        ),
        (
            "an empty resource id",
            "arn:aws:es:us-east-1:123456789012:domain/".to_string(),
        ),
        (
            "a resource type this service tags nothing of",
            "arn:aws:es:us-east-1:123456789012:vpc-endpoint/aos-1234".to_string(),
        ),
        (
            "an account field that is no account id",
            "arn:aws:es:us-east-1:not-an-account:domain/d".to_string(),
        ),
        (
            "a Region field that is no Region label",
            "arn:aws:es:us_east_1:123456789012:domain/d".to_string(),
        ),
        (
            "no Region field at all",
            "arn:aws:es::123456789012:domain/d".to_string(),
        ),
    ];

    for (what, value) in &malformed {
        for (operation, answer) in tagging_answers(&emulator, value).await {
            let case = format!("{operation}({what}: {value})");
            let code = error_code(&answer);
            let message = error_message(&answer);
            assert_eq!(
                (answer.status, code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {} / {code}",
                answer.status
            );
            assert_eq!(
                message, MALFORMED_TAGGING_ARN,
                "case {case}: expected the service's malformed-ARN sentence, got {message:?}"
            );
            assert!(
                !message.contains(value.as_str()),
                "case {case}: the sentence has no variable part, got {message:?}"
            );
        }
    }

    // The distinction the rule rests on: a value that IS an ARN of one of this service's
    // resources and names none the account holds keeps its own sentence, so the two
    // conditions are told apart. The rows are every taggable resource type, in this Region
    // and another.
    let absent: Vec<(&str, String)> = vec![
        (
            "a domain that was never created",
            live.replace("arn-form-target", "never-created-domain"),
        ),
        (
            "the same domain name in another Region",
            live.replace(":us-east-1:", ":eu-west-1:"),
        ),
        (
            "a data source",
            live.replace("domain/arn-form-target", "datasource/some-source"),
        ),
        (
            "an application",
            live.replace("domain/arn-form-target", "application/some-app"),
        ),
    ];
    for (what, value) in &absent {
        for (operation, answer) in tagging_answers(&emulator, value).await {
            let case = format!("{operation}({what}: {value})");
            let code = error_code(&answer);
            let message = error_message(&answer);
            assert_eq!(
                (answer.status, code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {} / {code}",
                answer.status
            );
            assert_ne!(
                message, MALFORMED_TAGGING_ARN,
                "case {case}: a well-formed ARN of this service naming nothing is not a \
                 format error, got {message:?}"
            );
        }
    }

    // Not one of the refusals above wrote anything: the resource that does exist still
    // carries exactly the tag it was given.
    let read = stored_tags(&emulator, &live, "arn-form-target").await;
    assert_eq!(
        read,
        vec![tag("env", "prod")],
        "case arn-form-target: a refused call must write nothing, got {read:?}"
    );
}

#[tokio::test]
async fn the_tag_ceiling_is_stated_with_one_fixed_sentence() {
    let emulator = Emulator::new();
    let arn = domain_arn(&emulator, "tag-ceiling-target").await;

    // Rows differ in how far past the ceiling the call reaches and in how the tags arrive:
    // one call carrying more than the ceiling at once, and a call that would carry a
    // resource already at the ceiling past it.
    let at_ceiling: Vec<(String, String)> = (0..TAG_QUOTA)
        .map(|index| tag(&format!("key-{index}"), "v"))
        .collect();
    let filled = add_tags(&emulator, &arn, wire_tags(&at_ceiling)).await;
    assert_eq!(
        filled.status, 200,
        "case AddTags(at the ceiling): expected 200, got {} with {:?}",
        filled.status, filled.body
    );

    let mut past: Vec<(String, String)> = at_ceiling.clone();
    past.push(tag("one-too-many", "v"));
    let mut far_past: Vec<(String, String)> = (0..TAG_QUOTA * 3)
        .map(|index| tag(&format!("far-{index}"), "v"))
        .collect();
    far_past.push(tag("and-another", "v"));

    for (what, tags) in [
        ("one tag past the ceiling", vec![tag("one-too-many", "v")]),
        ("a call carrying the whole set and one more", past),
        ("a call carrying far more than the ceiling", far_past),
    ] {
        let case = format!("AddTags({what})");
        let answer = add_tags(&emulator, &arn, wire_tags(&tags)).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409 past the ceiling, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "LimitExceededException",
            "case {case}: expected LimitExceededException, got {:?}",
            answer.body
        );
        let message = error_message(&answer);
        assert_eq!(
            message, TAG_QUOTA_EXCEEDED,
            "case {case}: expected the service's fixed sentence, got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()) && !message.contains(&TAG_QUOTA.to_string()),
            "case {case}: the sentence carries neither the ARN nor a count, got {message:?}"
        );
    }

    // The same sentence states the same ceiling on the `TagList` a create carries, whichever
    // create it is: a domain reports it through the error that operation models, an
    // application through the one its own model declares.
    let past_ceiling = wire_tags(&past_ceiling_tags());
    let domain = emulator
        .create(json!({ "DomainName": "tag-ceiling-create", "TagList": past_ceiling.clone() }))
        .await;
    assert_eq!(
        domain.status, 409,
        "case CreateDomain(past the ceiling): expected 409, got {} with {:?}",
        domain.status, domain.body
    );
    assert_eq!(
        error_message(&domain),
        TAG_QUOTA_EXCEEDED,
        "case CreateDomain(past the ceiling): expected the service's fixed sentence, got {:?}",
        domain.body
    );

    let application = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(json!({ "name": "tag-ceiling-app", "tagList": past_ceiling })),
        )
        .await;
    assert_eq!(
        application.status, 400,
        "case CreateApplication(past the ceiling): expected 400, got {} with {:?}",
        application.status, application.body
    );
    assert_eq!(
        error_message(&application),
        TAG_QUOTA_EXCEEDED,
        "case CreateApplication(past the ceiling): expected the service's fixed sentence, got \
         {:?}",
        application.body
    );
}

/// One tag more than a resource may carry, for the two create paths above.
fn past_ceiling_tags() -> Vec<(String, String)> {
    (0..=TAG_QUOTA)
        .map(|index| tag(&format!("create-{index}"), "v"))
        .collect()
}
