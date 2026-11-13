// SPDX-License-Identifier: CC0-1.0

//! The email-template surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: a create records the resource under the name the request
//! names, holding the content and tags it carried VERBATIM, so the resource's own read
//! reports exactly that and nothing else; a create naming a resource the account
//! already holds is refused with the operation's `AlreadyExistsException` and leaves
//! the stored resource exactly as it was; a read of a name the account holds nothing
//! under is refused `NotFoundException`; and a create whose content the service cannot
//! use — a template carrying no subject line, the one part a send cannot supply for
//! itself — is refused `BadRequestException` and stores nothing.
//!
//! The tables below span what the rule generalises over: which parts the content
//! carries (subject alone, with the text part, with the HTML part, with both), the
//! form of a part's value (plain, empty, non-ASCII, substitution placeholders, raw
//! markup), the number of tags (none, one, several, the documented ceiling and one
//! past it), and the form of the template name (one character, punctuation, mixed
//! case, a long name).

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the body, or `Value::Null` when absent.
    fn member(&self, member: &str) -> Value {
        self.body.get(member).cloned().unwrap_or(Value::Null)
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_template(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/templates", Some(body)).await
    }

    async fn get_template(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/templates/{name}"), None)
            .await
    }
}

/// The account the emulator serves, which the duplicate-name refusal names.
fn account_id() -> &'static str {
    emulator_server::emulator_account_id()
}

/// One row of the round-trip table: the name to create under, the `TemplateContent` the
/// request carries and the tags it declares.
struct StoredCase {
    case: &'static str,
    name: &'static str,
    content: fn() -> Value,
    tags: fn() -> Vec<(&'static str, &'static str)>,
}

/// No tags at all, for a row whose point is the content.
fn no_tags() -> Vec<(&'static str, &'static str)> {
    Vec::new()
}

/// The rows span the parts the content carries, the form of each part's value and the
/// number of tags, and — independently — the form of the template name.
const STORED_CASES: &[StoredCase] = &[
    StoredCase {
        case: "subject alone",
        name: "subject-only",
        content: || json!({"Subject": "Your receipt"}),
        tags: no_tags,
    },
    StoredCase {
        case: "subject and text part",
        name: "subject_and_text",
        content: || json!({"Subject": "Your receipt", "Text": "Thanks for your order."}),
        tags: no_tags,
    },
    StoredCase {
        case: "subject and HTML part",
        name: "subject.and.html",
        content: || json!({"Subject": "Your receipt", "Html": "<h1>Thanks</h1>"}),
        tags: no_tags,
    },
    StoredCase {
        case: "all three parts",
        name: "T",
        content: || {
            json!({
                "Subject": "Your receipt",
                "Text": "Thanks for your order.",
                "Html": "<h1>Thanks</h1><p>for your order.</p>",
            })
        },
        tags: no_tags,
    },
    StoredCase {
        case: "an empty text part is a part, not an absent one",
        name: "EmptyTextPart",
        content: || json!({"Subject": "Your receipt", "Text": "", "Html": "<p>hi</p>"}),
        tags: no_tags,
    },
    StoredCase {
        case: "non-ASCII subject and parts",
        name: "non-ascii",
        content: || {
            json!({
                "Subject": "Ihre Quittung · 領収書",
                "Text": "Grüße 😀",
                "Html": "<p>Grüße 😀</p>",
            })
        },
        tags: no_tags,
    },
    StoredCase {
        case: "substitution placeholders are stored, not resolved",
        name: "placeholders",
        content: || {
            json!({
                "Subject": "Hello {{name}}",
                "Text": "{{#if vip}}VIP{{else}}guest{{/if}} {{name}}",
                "Html": "<p>Hello {{name}}</p>",
            })
        },
        tags: no_tags,
    },
    StoredCase {
        case: "markup and entities are stored byte for byte",
        name: "markup",
        content: || {
            json!({
                "Subject": "5 > 3 &amp; rising",
                "Html": "<p>a &lt; b</p><script>alert(\"x\")</script>",
            })
        },
        tags: no_tags,
    },
    StoredCase {
        case: "one tag",
        name: "one-tag",
        content: || json!({"Subject": "Your receipt"}),
        tags: || vec![("purpose", "receipts")],
    },
    StoredCase {
        case: "several tags, in the order supplied",
        name: "several-tags",
        content: || json!({"Subject": "Your receipt", "Text": "Thanks."}),
        tags: || vec![("zone", "eu"), ("purpose", "receipts"), ("owner", "")],
    },
    StoredCase {
        case: "a long name of punctuation and mixed case",
        name: "Order.Receipt_v2-2026__the-one-that-is-sixty-four-characters-long",
        content: || json!({"Subject": "Your receipt", "Text": "Thanks."}),
        tags: || vec![("purpose", "receipts")],
    },
];

/// The `Tags` list a request sends for `pairs`.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The create body for `name`, `content` and `tags`.
fn create_body(name: &str, content: Value, tags: &[(&str, &str)]) -> Value {
    let mut body = Map::new();
    body.insert("TemplateName".to_string(), json!(name));
    body.insert("TemplateContent".to_string(), content);
    if !tags.is_empty() {
        body.insert("Tags".to_string(), tag_list(tags));
    }
    Value::Object(body)
}

#[tokio::test]
async fn a_created_email_template_round_trips_through_its_own_read() {
    for row in STORED_CASES {
        let case = row.case;
        let ses = Ses::new();
        let content = (row.content)();
        let tags = (row.tags)();
        let created = ses
            .create_template(create_body(row.name, content.clone(), &tags))
            .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected the create to be accepted with 200, got {} {:?}",
            created.status,
            created.body
        );
        // `CreateEmailTemplateResponse` carries no members.
        assert!(
            created.body.as_object().is_none_or(Map::is_empty),
            "case {case:?}: expected an empty create response body, got {:?}",
            created.body
        );

        let read = ses.get_template(row.name).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the stored template to be readable with 200, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.member("TemplateName"),
            json!(row.name),
            "case {case:?}: expected the read to echo the name it was created under, got {:?}",
            read.member("TemplateName")
        );
        assert_eq!(
            read.member("TemplateContent"),
            content,
            "case {case:?}: expected the content to be reported exactly as supplied, got {:?}",
            read.member("TemplateContent")
        );
        // A template that exists reports its tag set as the list it is: the tags it was
        // created with, and an EMPTY list when it was created with none.
        let expected_tags = tag_list(&tags);
        assert_eq!(
            read.member("Tags"),
            expected_tags,
            "case {case:?}: expected the tags to be reported exactly as supplied, got {:?}",
            read.member("Tags")
        );
    }
}

#[tokio::test]
async fn a_template_name_the_account_already_holds_is_refused_and_the_stored_one_stands() {
    // (case, the content the second create carries): the refusal is decided by the
    // name alone, whatever content follows it.
    let second_contents: Vec<(&str, Value)> = vec![
        ("the same content again", json!({"Subject": "first"})),
        ("different content", json!({"Subject": "second", "Text": "other"})),
        (
            "content with the other parts",
            json!({"Subject": "second", "Html": "<p>other</p>"}),
        ),
    ];

    for row in STORED_CASES {
        for (second_case, second_content) in &second_contents {
            let case = format!("{} / {second_case}", row.case);
            let ses = Ses::new();
            let content = (row.content)();
            let tags = (row.tags)();
            let created = ses
                .create_template(create_body(row.name, content.clone(), &tags))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the first create to be accepted, got {} {:?}",
                created.status, created.body
            );

            let again = ses
                .create_template(create_body(row.name, second_content.clone(), &[]))
                .await;
            assert_eq!(
                again.status, 400,
                "case {case:?}: expected the duplicate create to be refused with 400, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code(),
                "AlreadyExistsException",
                "case {case:?}: expected AlreadyExistsException, got {:?}",
                again.error_code()
            );
            assert_eq!(
                again.message(),
                format!(
                    "Template {} already exists for account id {}",
                    row.name,
                    account_id()
                ),
                "case {case:?}: expected the refusal to name the template and the account, got {:?}",
                again.message()
            );

            // The refused create changed nothing: the template the first create stored
            // is still exactly what a read reports.
            let read = ses.get_template(row.name).await;
            assert_eq!(
                read.member("TemplateContent"),
                content,
                "case {case:?}: expected the stored content to survive the refused create, got {:?}",
                read.member("TemplateContent")
            );
        }
    }
}

#[tokio::test]
async fn reading_a_template_the_account_does_not_hold_is_refused() {
    // (case, the name stored first — if any, the name read back).
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("nothing stored at all", None, "receipts"),
        ("another template stored", Some("receipts"), "invoices"),
        (
            "a name differing only in case",
            Some("receipts"),
            "Receipts",
        ),
        (
            "a name differing only in punctuation",
            Some("receipts.v2"),
            "receipts-v2",
        ),
        ("a one-character name", Some("receipts"), "r"),
    ];

    for (case, stored, requested) in cases {
        let ses = Ses::new();
        if let Some(stored) = stored {
            let created = ses
                .create_template(create_body(stored, json!({"Subject": "s"}), &[]))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
                created.status, created.body
            );
        }

        let read = ses.get_template(requested).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the read to be refused with 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            read.error_code()
        );
        assert_eq!(
            read.message(),
            format!("Template {requested} does not exist."),
            "case {case:?}: expected the refusal to name the template read, got {:?}",
            read.message()
        );
    }
}

#[tokio::test]
async fn content_carrying_no_subject_line_is_refused_and_stores_nothing() {
    // (case, the TemplateContent the request carries): a subject the request omits and
    // one sent as an empty string are the same absence, whatever other parts follow.
    let cases: Vec<(&str, Value)> = vec![
        ("no parts at all", json!({})),
        ("a text part only", json!({"Text": "Thanks."})),
        ("an HTML part only", json!({"Html": "<p>Thanks.</p>"})),
        (
            "both bodies but no subject",
            json!({"Text": "Thanks.", "Html": "<p>Thanks.</p>"}),
        ),
        ("an empty subject", json!({"Subject": ""})),
        (
            "an empty subject beside a body",
            json!({"Subject": "", "Text": "Thanks."}),
        ),
    ];

    for (case, content) in cases {
        let ses = Ses::new();
        let name = "no-subject";
        let refused = ses.create_template(create_body(name, content, &[])).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the create to be refused with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            "The subject must be specified.",
            "case {case:?}: expected the refusal to name the missing subject, got {:?}",
            refused.message()
        );

        // Nothing was stored, so the name is still free and its read is a 404.
        let read = ses.get_template(name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the refused name to hold nothing, got {} {:?}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_template_may_be_as_large_as_the_documented_maximum_and_no_larger() {
    // SES documents a maximum size of 500 KB for an email template, counted over the
    // subject line and both body parts, so the cases are the bound itself, a size well
    // below it and the first size past it.
    let ceiling = 500 * 1024usize;
    let subject = "Your receipt";
    // (case, how many bytes the parts hold in total)
    let sizes = [subject.len(), ceiling / 2, ceiling, ceiling + 1];
    for size in sizes {
        let case = format!("{size} bytes of content");
        let ses = Ses::new();
        let name = "sized";
        // The subject is required, so the remaining budget goes to the text part.
        let text = "x".repeat(size - subject.len());
        let created = ses
            .create_template(create_body(
                name,
                json!({"Subject": subject, "Text": text}),
                &[],
            ))
            .await;

        if size <= ceiling {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be accepted, got {} {:?}",
                created.status, created.body
            );
            let read = ses.get_template(name).await;
            assert_eq!(
                read.member("TemplateContent")
                    .get("Text")
                    .and_then(Value::as_str)
                    .map(str::len),
                Some(size - subject.len()),
                "case {case:?}: expected the whole text part to be stored, got {:?} bytes",
                read.member("TemplateContent")
                    .get("Text")
                    .and_then(Value::as_str)
                    .map(str::len)
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected the create to be refused with 400, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                created.error_code()
            );
            assert_eq!(
                created.message(),
                format!(
                    "Template {name} exceeds maximum allowed size of {} KB.",
                    ceiling / 1024
                ),
                "case {case:?}: expected the refusal to name the template and the quota, got {:?}",
                created.message()
            );
            let read = ses.get_template(name).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the refused name to hold nothing, got {} {:?}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn a_template_may_carry_the_documented_number_of_tags_and_no_more() {
    // "A resource can have as many as 50 tags" (the model's own `Tag` documentation),
    // so the cases are the bound itself, the counts below it and the first count past
    // it.
    let ceiling = 50usize;
    for count in [0usize, 1, ceiling - 1, ceiling, ceiling + 1] {
        let case = format!("{count} tags");
        let ses = Ses::new();
        let name = "tagged";
        let pairs: Vec<(String, String)> = (0..count)
            .map(|index| (format!("key-{index}"), format!("value-{index}")))
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let created = ses
            .create_template(create_body(
                name,
                json!({"Subject": "Your receipt"}),
                &borrowed,
            ))
            .await;

        if count <= ceiling {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be accepted, got {} {:?}",
                created.status, created.body
            );
            let read = ses.get_template(name).await;
            // Every tag supplied, as a list — an empty one for a template created with
            // no tags at all.
            let expected = tag_list(&borrowed);
            assert_eq!(
                read.member("Tags"),
                expected,
                "case {case:?}: expected every tag to be reported as supplied, got {:?}",
                read.member("Tags")
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected the create to be refused with 400, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                created.error_code()
            );
            assert_eq!(
                created.message(),
                format!("A resource may have no more than {ceiling} tags, but {count} were provided."),
                "case {case:?}: expected the refusal to state the ceiling and the count, got {:?}",
                created.message()
            );
            let read = ses.get_template(name).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the refused name to hold nothing, got {} {:?}",
                read.status, read.body
            );
        }
    }
}
