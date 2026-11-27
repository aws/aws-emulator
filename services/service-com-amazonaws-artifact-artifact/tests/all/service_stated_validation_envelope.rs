// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a field-validation refusal it decides ITSELF, over HTTP.
//!
//! RULE: every refusal this service states for a value a request carried — the ones a
//! handler decides, not the ones the boundary validator rejects against the model's own
//! constraints — is answered in the ONE AWS validation envelope: HTTP 400
//! `ValidationException`, `reason` `fieldValidationFailed`, one `fieldList` entry per
//! refused member stating `Value at '<member>' failed to satisfy constraint:
//! <requirement>`, and a `message` that is the envelope over exactly those clauses
//! (`N validation error(s) detected: …`). So a client that parses one of this service's
//! validation refusals parses all of them, and the two halves of one refusal — the message
//! and the field list — cannot disagree.
//!
//! `validation_exception_shape.rs` and `validation_message_rendering.rs` pin the same
//! envelope for a value the MODEL's constraints reject at the boundary; this suite pins it
//! for the refusals the handlers decide from the request and from persisted state, which
//! the boundary cannot see.
//!
//! Every case goes through the router — routing, the front door, deserialization, the
//! handler, the persisted state and serialization — so a refusal worded anywhere but in
//! the one envelope cannot pass.
//!
//! The rows differ along what the rule generalises over: WHICH requirement was failed (a
//! per-resource count ceiling, the reserved tag-key namespace, a support mode the
//! submitted content cannot be processed under, a query identifier the targeted inquiry
//! does not own), the member that carried the value (`tags`, `tagKeys`, `supportMode`,
//! `queryIdentifiers`, `queryIdentifier`), the operation and its method (POST, PUT and
//! DELETE), where the request carried the value (body, query string), and — for the count
//! ceiling — by how much the write would pass it and whether the keys it names are already
//! carried.

use std::collections::BTreeMap;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap and the request sender are the crate's test harness': one emulator
// over one shared engine, reached the way a client reaches it.
use super::harness::{call, emulator, get, Answer};

/// How many tags one resource holds at most, from `TagsMap`'s modelled `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`). Spelled out here rather than imported
/// from the crate, so a wrong constant in `src/state.rs` cannot make this suite pass.
const TAG_CEILING: usize = 50;

/// The tag-key namespace AWS keeps for the tags it applies itself — the AWS-wide tagging
/// rule, also spelled out here rather than imported.
const RESERVED_PREFIX: &str = "aws:";

/// `ValidationExceptionReason`'s value for a member of the request whose value failed
/// validation (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// `FeedbackRating`'s value a submission below rates with
/// (`smithy_query(query="shape",shape="FeedbackRating")`).
const RATING: &str = "THUMBS_DOWN";

/// The requirement a write that would take a resource past [`TAG_CEILING`] failed: the
/// modelled map length ceiling, stated the way a length ceiling is stated.
fn tag_ceiling_requirement() -> String {
    format!("Member must have length less than or equal to {TAG_CEILING}")
}

/// The requirement a key in the reserved namespace failed.
fn reserved_key_requirement() -> String {
    format!("Member must not contain a tag key beginning with {RESERVED_PREFIX}")
}

/// The requirement a query identifier naming no query of the targeted inquiry failed.
const UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT: &str =
    "every query identifier must name a query of the compliance inquiry";

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is
/// ONE path segment, so the `/` inside an ARN is percent-encoded.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One query-string value, percent-encoded: a tag key may carry `:`, `/`, `=`, `+` and
/// whitespace, each of which means something else in a query string.
fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// `count` distinct tag pairs whose keys begin with `prefix`.
fn tag_run(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}{index:02}"), format!("value{index:02}")))
        .collect()
}

/// Tag pairs as the `TagsMap` a request body carries.
fn tags_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    call(router, "POST", uri, Bytes::from(body.to_string())).await
}

/// The body `CreateComplianceInquiry` takes for a text submission carrying `tags`.
fn create_body(name: &str, tags: &[(String, String)]) -> Value {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    body
}

/// Create one inquiry carrying `tags` and answer its id and the ARN the service minted —
/// the resource every tag write below addresses.
async fn created_inquiry(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let answer = post(
        router,
        "/v1/compliance-inquiry/create",
        &create_body(name, tags),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "case {name:?}: creating the inquiry must answer HTTP 202, got {} with {:?}",
        answer.status, answer.body
    );
    let summary = &answer.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {name:?}: the create must report an id and an ARN, got {:?}",
        answer.body
    );
    (id, arn)
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &[(String, String)]) -> Answer {
    post(
        router,
        &format!("/tags/{}", label(resource_arn)),
        &json!({ "tags": tags_object(tags) }),
    )
    .await
}

/// The tag set `ListTagsForResource` reports for a resource, in a comparable order.
async fn listed_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let answer = get(router, &format!("/tags/{}", label(resource_arn))).await;
    assert_eq!(
        answer.status, 200,
        "ListTagsForResource must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The `fieldList` entries of a refusal, as (name, clause) pairs.
fn field_list(answer: &Answer) -> Vec<(String, String)> {
    answer.body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `message` the refusal carries, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The whole rule, asserted over one answer: the modelled refusal, the required `reason`,
/// the clause stated against `member` and `requirement`, and a message that is the
/// envelope over exactly the clauses the field list carries.
fn assert_stated_in_the_envelope(case: &str, answer: &Answer, member: &str, requirement: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a value the service refuses must be answered with HTTP 400, got {} \
         with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );

    let clause = format!("Value at '{member}' failed to satisfy constraint: {requirement}");
    assert_eq!(
        field_list(answer),
        vec![(member.to_string(), clause.clone())],
        "case {case:?}: expected one fieldList entry naming {member:?} with the clause \
         {clause:?}, got {:?}",
        answer.body
    );
    let expected = format!("1 validation error detected: {clause}");
    assert_eq!(
        message(answer),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        message(answer)
    );
    // The generic halves of the rule, independent of any one row's wording: the message is
    // the envelope over the clauses the list carries, and no clause quotes the value it
    // refused.
    for (name, stated) in field_list(answer) {
        assert!(
            message(answer).contains(stated.as_str()),
            "case {case:?}: the message must state the clause the entry for {name:?} \
             carries ({stated:?}), got {:?}",
            message(answer)
        );
    }
    assert!(
        !message(answer).contains("Value '"),
        "case {case:?}: the clause must not quote the rejected value, got {:?}",
        message(answer)
    );
}

/// One refused request, and the clause its refusal must state.
struct Row {
    /// What this row is a case of.
    case: String,
    method: &'static str,
    uri: String,
    /// The request body, for a row whose operation carries one.
    body: Bytes,
    /// The member the clause names.
    member: &'static str,
    /// The requirement the clause states.
    requirement: String,
}

/// Every refusal this service decides itself, with the state each one needs persisted
/// first: resources holding none, one, half and exactly a ceiling's worth of tags, and an
/// inquiry owning one query.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let a_reserved_key = format!("{RESERVED_PREFIX}cloudformation:stack-name");

    // A resource filled to exactly the published ceiling: the bound itself is served, which
    // is what makes the rows past it about the bound and not about the member.
    let at_the_ceiling = tag_run("key", TAG_CEILING);
    let (_, full) = created_inquiry(router, "at the tag ceiling", &[]).await;
    let filled = tag(router, &full, &at_the_ceiling).await;
    assert_eq!(
        filled.status, 204,
        "filling a resource to the published ceiling must be served, got {} with {:?}",
        filled.status, filled.body
    );

    // A resource holding one tag, and one holding half the ceiling: a write past the
    // ceiling is about the resource's whole tag set, not about one request's size.
    let (_, one_tag) = created_inquiry(
        router,
        "holding one tag",
        &[("team".to_string(), "legal".to_string())],
    )
    .await;
    let half = tag_run("key", TAG_CEILING / 2);
    let (_, half_full) = created_inquiry(router, "holding half the ceiling", &half).await;

    // An inquiry owning exactly one query — its text submission — so every identifier but
    // the first names a query it does not own. Waited out to the end of its processing,
    // because an export of an inquiry still being processed is refused on the state of the
    // resource before any member of the request is read.
    let (inquiry, _) = created_inquiry(router, "one query of its own", &[]).await;
    crate::harness::settled_inquiry_status(router, "one query of its own", &inquiry).await;

    vec![
        Row {
            case: "a write one past the per-resource tag ceiling".to_string(),
            method: "POST",
            uri: format!("/tags/{}", label(&full)),
            body: Bytes::from(json!({ "tags": { "one-key-too-many": "refused" } }).to_string()),
            member: "tags",
            requirement: tag_ceiling_requirement(),
        },
        Row {
            case: "a write a whole ceiling past it".to_string(),
            method: "POST",
            uri: format!("/tags/{}", label(&full)),
            body: Bytes::from(
                json!({ "tags": tags_object(&tag_run("extra", TAG_CEILING)) }).to_string(),
            ),
            member: "tags",
            requirement: tag_ceiling_requirement(),
        },
        Row {
            case: "a write past it on a resource holding one tag".to_string(),
            method: "POST",
            uri: format!("/tags/{}", label(&one_tag)),
            body: Bytes::from(
                json!({ "tags": tags_object(&tag_run("extra", TAG_CEILING)) }).to_string(),
            ),
            member: "tags",
            requirement: tag_ceiling_requirement(),
        },
        Row {
            // Ten of the keys this row names are already carried, so they take the
            // resource no further: the write is still past the ceiling on the rest.
            case: "a write past it naming keys the resource already carries".to_string(),
            method: "POST",
            uri: format!("/tags/{}", label(&half_full)),
            body: Bytes::from(
                json!({
                    "tags": tags_object(
                        &half[half.len() - 10..]
                            .iter()
                            .cloned()
                            .chain(tag_run("extra", TAG_CEILING - 15))
                            .collect::<Vec<_>>(),
                    )
                })
                .to_string(),
            ),
            member: "tags",
            requirement: tag_ceiling_requirement(),
        },
        Row {
            case: "a tag write naming a key in the reserved namespace".to_string(),
            method: "POST",
            uri: format!("/tags/{}", label(&one_tag)),
            body: Bytes::from(
                json!({
                    "tags": tags_object(&[(a_reserved_key.clone(), "mine".to_string())])
                })
                .to_string(),
            ),
            member: "tags",
            requirement: reserved_key_requirement(),
        },
        Row {
            case: "a create naming a key in the reserved namespace".to_string(),
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Bytes::from(
                create_body(
                    "a reserved key on the create",
                    &[(a_reserved_key.clone(), "mine".to_string())],
                )
                .to_string(),
            ),
            member: "tags",
            requirement: reserved_key_requirement(),
        },
        Row {
            case: "a removal naming a key in the reserved namespace".to_string(),
            method: "DELETE",
            uri: format!(
                "/tags/{}?tagKeys={}",
                label(&one_tag),
                query_value(&a_reserved_key)
            ),
            body: Bytes::new(),
            member: "tagKeys",
            requirement: reserved_key_requirement(),
        },
        Row {
            case: "an export naming a query the inquiry does not own".to_string(),
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Bytes::from(
                json!({ "complianceInquiryId": inquiry, "queryIdentifiers": [7] }).to_string(),
            ),
            member: "queryIdentifiers",
            requirement: UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT.to_string(),
        },
        Row {
            case: "a feedback submission naming a query the inquiry does not own".to_string(),
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Bytes::from(
                json!({
                    "complianceInquiryId": inquiry,
                    "rating": RATING,
                    "responseRevisionId": 1,
                    "queryIdentifier": 7,
                })
                .to_string(),
            ),
            member: "queryIdentifier",
            requirement: UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT.to_string(),
        },
    ]
}

#[tokio::test]
async fn every_refusal_this_service_states_itself_is_worded_in_the_one_envelope() {
    let router = emulator();

    for row in rows(&router).await {
        let answer = call(&router, row.method, &row.uri, row.body.clone()).await;
        assert_stated_in_the_envelope(&row.case, &answer, row.member, &row.requirement);
    }
}

#[tokio::test]
async fn the_tag_ceiling_is_served_at_its_bound_and_refused_past_it_without_writing() {
    let router = emulator();

    // The published bound: one key short of it, then exactly at it, are both served.
    let (_, arn) = created_inquiry(&router, "walking up to the ceiling", &[]).await;
    let one_short = tag_run("key", TAG_CEILING - 1);
    for (case, written) in [
        ("one tag short of the ceiling", &one_short),
        ("exactly at the ceiling", &tag_run("key", TAG_CEILING)),
    ] {
        let answer = tag(&router, &arn, written).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: a tag set within the published bound must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
    let at_the_ceiling: BTreeMap<String, String> = tag_run("key", TAG_CEILING).into_iter().collect();
    assert_eq!(
        listed_tags(&router, &arn).await,
        at_the_ceiling,
        "a resource at the published bound must report every tag written to it"
    );

    // Past it, the refusal is the envelope — and it persists nothing, so the resource
    // still reports the tag set it had.
    let answer = tag(
        &router,
        &arn,
        &[("one-key-too-many".to_string(), "refused".to_string())],
    )
    .await;
    assert_stated_in_the_envelope(
        "one key past the ceiling",
        &answer,
        "tags",
        &tag_ceiling_requirement(),
    );
    assert_eq!(
        listed_tags(&router, &arn).await,
        at_the_ceiling,
        "case \"one key past the ceiling\": a refused write must persist nothing"
    );

    // A key the resource ALREADY carries is still served at the bound: the ceiling is on
    // how many tags the resource holds, not on how many a request names.
    let answer = tag(
        &router,
        &arn,
        &[("key00".to_string(), "rewritten".to_string())],
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case \"overwriting a carried key at the ceiling\": expected HTTP 204, got {} with \
         {:?}",
        answer.status, answer.body
    );
    let mut rewritten = at_the_ceiling.clone();
    rewritten.insert("key00".to_string(), "rewritten".to_string());
    assert_eq!(
        listed_tags(&router, &arn).await,
        rewritten,
        "case \"overwriting a carried key at the ceiling\": the write must be persisted"
    );
}
