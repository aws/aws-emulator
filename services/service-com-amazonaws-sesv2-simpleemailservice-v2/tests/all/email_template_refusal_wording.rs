// SPDX-License-Identifier: CC0-1.0

//! How the email-template plane WORDS the three refusals a client branches on, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered, through the crate's shared JSON wire harness
//! [`crate::json_wire`] — so routing, the constraint walk, the handler and serialization
//! are all exercised.
//!
//! The rules these cases pin, one per suite below:
//!
//! * a create refused because the account already holds the name states the bare fact
//!   that the name is taken when the create ATTACHES TAGS, and names the template and
//!   the account only when it attaches none — one condition, two sentences, selected by
//!   the create rather than by the store;
//! * a write of a named template whose content exceeds the service's documented
//!   whole-template size quota is refused `BadRequestException` naming the template and
//!   the quota in the unit it is documented in, and stores nothing, so the name is still
//!   free afterwards;
//! * a member refused for exceeding its modelled `@length` MAXIMUM is reported by naming
//!   the member and the maximum, withholding the submitted value, while a member refused
//!   for falling short of a minimum keeps the value the framework quotes (the minimum
//!   side of that line is pinned in the boundary-validation suite beside this one).
//!
//! The tables span what each rule generalises over: the number of tags a losing create
//! attaches (none, one, several, the documented tag ceiling) and the form of the template
//! name; the operation that writes the content (create, update), how far past the quota
//! it is and the name it writes under; and, for the length maximum, which member carries
//! the value (`TemplateData`, 262144, and `PolicyName`, 64 — both maxima read from
//! `smithy_query(query="operation",operation="TestRenderEmailTemplate")` and
//! `(…,operation="CreateEmailIdentityPolicy")`), with each maximum exercised at the bound
//! itself and one byte past it.

use serde_json::{json, Map, Value};

use crate::json_wire::{Reply, Ses};

/// `CreateEmailTemplate` with the request body the case supplies.
async fn create_template(ses: &Ses, body: Value) -> Reply {
    ses.call("POST", "/v2/email/templates", Some(body)).await
}

/// `UpdateEmailTemplate`, replacing the content of the template `name`.
async fn update_template(ses: &Ses, name: &str, content: Value) -> Reply {
    ses.call(
        "PUT",
        &format!("/v2/email/templates/{name}"),
        Some(json!({"TemplateContent": content})),
    )
    .await
}

/// `GetEmailTemplate` for the template `name`.
async fn get_template(ses: &Ses, name: &str) -> Reply {
    ses.call("GET", &format!("/v2/email/templates/{name}"), None)
        .await
}

/// `TestRenderEmailTemplate`: render the saved template `name` against the rendering data
/// `data`.
async fn render(ses: &Ses, name: &str, data: &str) -> Reply {
    ses.call(
        "POST",
        &format!("/v2/email/templates/{name}/render"),
        Some(json!({"TemplateData": data})),
    )
    .await
}

/// `CreateEmailIdentityPolicy`, whose `PolicyName` carries a modelled length maximum.
async fn create_identity_policy(ses: &Ses, policy_name: &str) -> Reply {
    ses.call(
        "POST",
        &format!("/v2/email/identities/example.com/policies/{policy_name}"),
        Some(json!({"Policy": "{\"Version\":\"2012-10-17\"}"})),
    )
    .await
}

/// A member of an answered body, or `Value::Null` when absent.
fn member(reply: &Reply, member: &str) -> Value {
    reply.body.get(member).cloned().unwrap_or(Value::Null)
}

/// The MIME document a render answered with, or an empty string when it answered no
/// `RenderedTemplate` at all.
fn rendered(reply: &Reply) -> String {
    member(reply, "RenderedTemplate")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The account the emulator serves, which the untagged duplicate-name refusal names.
fn account_id() -> &'static str {
    emulator_server::emulator_account_id()
}

/// The `Tags` list a request sends for `count` tags, or no `Tags` member at all when the
/// create attaches none.
fn tags_of(count: usize) -> Option<Value> {
    (count > 0).then(|| {
        Value::Array(
            (0..count)
                .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
                .collect(),
        )
    })
}

/// The create body for `name` and `content`, attaching `tag_count` tags.
fn create_body(name: &str, content: Value, tag_count: usize) -> Value {
    let mut body = Map::new();
    body.insert("TemplateName".to_string(), json!(name));
    body.insert("TemplateContent".to_string(), content);
    if let Some(tags) = tags_of(tag_count) {
        body.insert("Tags".to_string(), tags);
    }
    Value::Object(body)
}

/// Some content a create or an update can carry, where the content is not what is under
/// test.
fn content() -> Value {
    json!({"Subject": "Your receipt", "Text": "Thanks for your order."})
}

/// The template names the tables vary over: a one-character name, a name of punctuation
/// and mixed case, and a long one.
const NAMES: [&str; 3] = [
    "r",
    "Order.Receipt_v2-2026",
    "the-one-that-is-sixty-four-characters-long-and-then-some-more-x",
];

/// The tag counts a losing create attaches, spanning none, one, several and the
/// documented ceiling ("A resource can have as many as 50 tags", the model's own `Tag`
/// documentation).
const TAG_COUNTS: [usize; 4] = [0, 1, 3, 50];

/// The refusal AWS words for a duplicate-name create that attached `tag_count` tags under
/// `name`.
fn duplicate_refusal(name: &str, tag_count: usize) -> String {
    if tag_count == 0 {
        format!(
            "Template {name} already exists for account id {}",
            account_id()
        )
    } else {
        "Template already exists".to_string()
    }
}

#[tokio::test]
async fn a_duplicate_name_refusal_names_the_template_only_when_the_create_attaches_no_tags() {
    for name in NAMES {
        for tag_count in TAG_COUNTS {
            let case = format!("{name:?} with {tag_count} tags");
            let ses = Ses::new();
            let first = create_template(&ses, create_body(name, content(), 1)).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: expected the first create to be accepted, got {} {:?}",
                first.status, first.body
            );

            let again = create_template(&ses, create_body(name, content(), tag_count)).await;
            assert_eq!(
                (again.status, again.error_code().as_str()),
                (400, "AlreadyExistsException"),
                "case {case:?}: expected 400 AlreadyExistsException, got {} {:?}",
                again.status,
                again.body
            );
            assert_eq!(
                again.message(),
                duplicate_refusal(name, tag_count),
                "case {case:?}: expected the sentence the path AWS takes words, got {:?}",
                again.message()
            );
            if tag_count > 0 {
                // The tagged path discloses nothing about the account that holds the
                // name; the equality above is what pins the sentence itself.
                assert!(
                    !again.message().contains(account_id()),
                    "case {case:?}: expected the account not to be named, got {:?}",
                    again.message()
                );
            }
        }
    }
}

#[tokio::test]
async fn concurrent_tagged_creates_leave_one_winner_and_refuse_the_rest_without_naming_it() {
    // The race: several creates of ONE fresh name, each attaching tags, in flight
    // together. Exactly one may win, and every loser is refused for THIS name already
    // existing — with the sentence the tagged path words.
    let name = "tplrace";
    let ses = Ses::new();
    let (a, b, c, d, e) = tokio::join!(
        create_template(
            &ses,
            create_body(name, json!({"Subject": "subj-0", "Text": "body-0"}), 1)
        ),
        create_template(
            &ses,
            create_body(name, json!({"Subject": "subj-1", "Text": "body-1"}), 1)
        ),
        create_template(
            &ses,
            create_body(name, json!({"Subject": "subj-2", "Text": "body-2"}), 1)
        ),
        create_template(
            &ses,
            create_body(name, json!({"Subject": "subj-3", "Text": "body-3"}), 1)
        ),
        create_template(
            &ses,
            create_body(name, json!({"Subject": "subj-4", "Text": "body-4"}), 1)
        ),
    );
    let replies = [a, b, c, d, e];

    let winners = replies.iter().filter(|reply| reply.status == 200).count();
    assert_eq!(
        winners,
        1,
        "expected exactly one winner out of {}, got {winners} (statuses {:?})",
        replies.len(),
        replies
            .iter()
            .map(|reply| reply.status)
            .collect::<Vec<u16>>()
    );
    for (index, reply) in replies.iter().enumerate().filter(|(_, r)| r.status != 200) {
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "loser {index}: expected 400 AlreadyExistsException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            "Template already exists",
            "loser {index}: expected the tagged path's sentence, got {:?}",
            reply.message()
        );
    }

    // One winner, so one stored template — and the untagged duplicate that follows the
    // settled name is refused with the OTHER sentence.
    let settled = create_template(&ses, create_body(name, content(), 0)).await;
    assert_eq!(
        settled.message(),
        duplicate_refusal(name, 0),
        "expected the settled untagged duplicate to name the template and the account, got {:?}",
        settled.message()
    );
}

/// The whole-template size quota, in bytes and in the unit the refusal states it in: SES
/// documents a maximum size of 500 KB for an email template, counted over the subject
/// line and both body parts.
const QUOTA_BYTES: usize = 500 * 1024;

/// Content of exactly `size` bytes across the subject line and the text part.
fn content_of(size: usize) -> Value {
    let subject = "Your receipt";
    json!({"Subject": subject, "Text": "x".repeat(size - subject.len())})
}

#[tokio::test]
async fn content_past_the_size_quota_is_refused_naming_the_template_and_the_quota() {
    // The rows span how far past the quota the content is — the first byte past it, well
    // past it, and twice the quota — and the operation that writes it.
    for name in NAMES {
        for size in [QUOTA_BYTES + 1, 600_000, QUOTA_BYTES * 2] {
            let expected = format!(
                "Template {name} exceeds maximum allowed size of {} KB.",
                QUOTA_BYTES / 1024
            );

            // The create: refused, and the name is left holding nothing.
            let case = format!("create of {name:?} with {size} bytes");
            let ses = Ses::new();
            let created = create_template(&ses, create_body(name, content_of(size), 0)).await;
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {} {:?}",
                created.status,
                created.body
            );
            assert_eq!(
                created.message(),
                expected,
                "case {case:?}: expected the refusal to name the template and the quota, got {:?}",
                created.message()
            );
            let read = get_template(&ses, name).await;
            assert_eq!(
                (read.status, read.error_code().as_str()),
                (404, "NotFoundException"),
                "case {case:?}: expected the refused name to hold nothing, got {} {:?}",
                read.status,
                read.body
            );
            assert_eq!(
                read.message(),
                format!("Template {name} does not exist."),
                "case {case:?}: expected the read to report the name as absent, got {:?}",
                read.message()
            );

            // The update of a template that DOES exist: the same sentence, and the stored
            // content stands.
            let case = format!("update of {name:?} with {size} bytes");
            let ses = Ses::new();
            let stored = content();
            let created = create_template(&ses, create_body(name, stored.clone(), 0)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
                created.status, created.body
            );
            let updated = update_template(&ses, name, content_of(size)).await;
            assert_eq!(
                (updated.status, updated.error_code().as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {} {:?}",
                updated.status,
                updated.body
            );
            assert_eq!(
                updated.message(),
                expected,
                "case {case:?}: expected the refusal to name the template and the quota, got {:?}",
                updated.message()
            );
            let read = get_template(&ses, name).await;
            assert_eq!(
                member(&read, "TemplateContent"),
                stored,
                "case {case:?}: expected the stored content to survive the refusal, got {:?}",
                member(&read, "TemplateContent")
            );
        }
    }
}

#[tokio::test]
async fn content_at_the_size_quota_is_accepted_and_round_trips() {
    // The bound itself: the largest content the quota admits is stored whole.
    let name = "at-the-quota";
    let ses = Ses::new();
    let created = create_template(&ses, create_body(name, content_of(QUOTA_BYTES), 0)).await;
    assert_eq!(
        created.status, 200,
        "expected content of exactly {QUOTA_BYTES} bytes to be accepted, got {} {:?}",
        created.status, created.message()
    );
    let read = get_template(&ses, name).await;
    assert_eq!(
        member(&read, "TemplateContent"),
        content_of(QUOTA_BYTES),
        "expected the content to round-trip byte for byte, got {} bytes of text",
        member(&read, "TemplateContent")
            .get("Text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .len()
    );
}

/// `EmailTemplateData`'s modelled maximum length
/// (`smithy_query(query="operation",operation="TestRenderEmailTemplate")`).
const TEMPLATE_DATA_MAX: usize = 262_144;

/// `PolicyName`'s modelled maximum length
/// (`smithy_query(query="operation",operation="CreateEmailIdentityPolicy")`).
const POLICY_NAME_MAX: usize = 64;

/// The clause separator the framework renders, shared by every requirement.
const SEPARATOR: &str = " failed to satisfy constraint: ";

/// The member a length row sends its value as.
#[derive(Debug, Clone, Copy)]
enum LengthMember {
    /// `TestRenderEmailTemplate$TemplateData`, sent as a set of key-value pairs.
    TemplateData,
    /// `CreateEmailIdentityPolicy$PolicyName`, sent on the request's path.
    PolicyName,
}

/// Rendering data of exactly `size` bytes: one key-value pair whose value takes the
/// remaining budget.
fn rendering_data_of(size: usize) -> String {
    let wrapper = "{\"k\": \"\"}";
    format!("{{\"k\": \"{}\"}}", "x".repeat(size - wrapper.len()))
}

/// The template a rendering-data row renders, which substitutes the pair's value into
/// both body parts.
fn substituting_template() -> Value {
    json!({
        "Subject": "limit probe",
        "Text": "value={{k}}",
        "Html": "<p>value={{k}}</p>",
    })
}

#[tokio::test]
async fn a_value_past_a_modelled_length_maximum_is_refused_without_echoing_it() {
    // (case, the member, its wire path, its modelled maximum, the value's length): each
    // maximum one byte past the bound and well past it.
    let rows: [(&str, LengthMember, &str, usize, usize); 4] = [
        (
            "rendering data one byte past the maximum",
            LengthMember::TemplateData,
            "templateData",
            TEMPLATE_DATA_MAX,
            TEMPLATE_DATA_MAX + 1,
        ),
        (
            "rendering data well past the maximum",
            LengthMember::TemplateData,
            "templateData",
            TEMPLATE_DATA_MAX,
            TEMPLATE_DATA_MAX * 2,
        ),
        (
            "a policy name one byte past the maximum",
            LengthMember::PolicyName,
            "policyName",
            POLICY_NAME_MAX,
            POLICY_NAME_MAX + 1,
        ),
        (
            "a policy name well past the maximum",
            LengthMember::PolicyName,
            "policyName",
            POLICY_NAME_MAX,
            POLICY_NAME_MAX * 4,
        ),
    ];

    for (case, member, path, maximum, length) in rows {
        let ses = Ses::new();
        let (sent, reply) = match member {
            LengthMember::TemplateData => {
                let template = "limit-probe";
                let created =
                    create_template(&ses, create_body(template, substituting_template(), 0)).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
                    created.status, created.body
                );
                let data = rendering_data_of(length);
                let reply = render(&ses, template, &data).await;
                (data, reply)
            }
            LengthMember::PolicyName => {
                let policy_name = "p".repeat(length);
                let reply = create_identity_policy(&ses, &policy_name).await;
                (policy_name, reply)
            }
        };

        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value at '{path}'{SEPARATOR}Member must have length \
                 less than or equal to {maximum}"
            ),
            "case {case:?}: expected the value-less clause naming the member and the maximum, got {:?}",
            reply.message()
        );
        assert!(
            !reply.message().contains(&sent[..32]),
            "case {case:?}: expected the submitted value to be withheld, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn rendering_data_at_the_modelled_maximum_is_accepted_and_substituted_whole() {
    // The bound itself: the largest rendering data the member admits is substituted into
    // both body parts, whole.
    let ses = Ses::new();
    let template = "limit-probe";
    let created = create_template(&ses, create_body(template, substituting_template(), 0)).await;
    assert_eq!(
        created.status, 200,
        "expected the seeding create to be accepted, got {} {:?}",
        created.status, created.body
    );

    let data = rendering_data_of(TEMPLATE_DATA_MAX);
    let reply = render(&ses, template, &data).await;
    assert_eq!(
        reply.status,
        200,
        "expected rendering data of exactly {TEMPLATE_DATA_MAX} bytes to be accepted, got {} {:?}",
        reply.status,
        reply.message()
    );
    // The value of the one pair, which both parts substitute.
    let value = serde_json::from_str::<Value>(&data)
        .expect("the rendering data is a set of key-value pairs")
        .get("k")
        .and_then(Value::as_str)
        .expect("the pair's value")
        .to_string();
    let document = rendered(&reply);
    let substituted = document.matches(value.as_str()).count();
    assert_eq!(
        substituted, 2,
        "expected the whole value in both the text and the HTML alternative, got {substituted} \
         occurrences in {} bytes of document",
        document.len()
    );
}

#[tokio::test]
async fn a_policy_name_at_the_modelled_maximum_is_not_refused_for_its_length() {
    // The other side of the maximum: a value of exactly the modelled length passes the
    // constraint walk, whatever the operation then answers about the identity itself.
    let ses = Ses::new();
    let reply = create_identity_policy(&ses, &"p".repeat(POLICY_NAME_MAX)).await;
    assert!(
        !reply.message().contains(SEPARATOR),
        "expected no constraint refusal for a name of exactly {POLICY_NAME_MAX} characters, got {} {:?}",
        reply.status,
        reply.message()
    );
}
