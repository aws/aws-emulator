// SPDX-License-Identifier: CC0-1.0

//! Updating a stored email template, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and every stored template is read back through `GetEmailTemplate`.
//!
//! The rule these cases pin: an update replaces the stored resource's content with the
//! content the request carries, WHOLE — a part the request omits is absent from the
//! updated resource rather than inherited from what stood there before — and leaves
//! everything the request does not carry (the name it is keyed by, the tags attached to
//! it) exactly as it was; an update naming a resource the account holds nothing under is
//! refused with the operation's `NotFoundException` and creates nothing; and an update
//! whose content the service cannot use — content carrying no subject line, or content
//! larger than the documented maximum — is refused `BadRequestException` and leaves the
//! stored resource exactly as it was.
//!
//! The tables below span what the rule generalises over: which parts the content
//! carries before and after the update (subject alone, with the text part, with the HTML
//! part, with both), the form of a part's value (plain, empty, non-ASCII, substitution
//! placeholders, raw markup), whether the resource carries tags, the form of the
//! resource's name, the number of updates applied in a row, and the size of the content
//! at the documented bound and one byte past it.

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

    async fn update_template(&self, name: &str, content: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/templates/{name}"),
            Some(json!({"TemplateContent": content})),
        )
        .await
    }

    async fn get_template(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/templates/{name}"), None)
            .await
    }
}

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

/// One row of the replacement table: the name the template is keyed by, the tags the
/// create attached, the content it was created with and the content the update carries.
struct UpdateCase {
    case: &'static str,
    name: &'static str,
    tags: fn() -> Vec<(&'static str, &'static str)>,
    created: fn() -> Value,
    updated: fn() -> Value,
}

/// No tags at all, for a row whose point is the content.
fn no_tags() -> Vec<(&'static str, &'static str)> {
    Vec::new()
}

/// The rows span which parts the content carries before and after the update, the form
/// of each part's value, the form of the template name and whether the template is
/// tagged.
const UPDATE_CASES: &[UpdateCase] = &[
    UpdateCase {
        case: "the subject line alone changes",
        name: "subject-only",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt"}),
        updated: || json!({"Subject": "Your invoice"}),
    },
    UpdateCase {
        case: "a text part is added to a subject-only template",
        name: "gains_text",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt"}),
        updated: || json!({"Subject": "Your receipt", "Text": "Thanks for your order."}),
    },
    UpdateCase {
        case: "an HTML part is added to a subject-only template",
        name: "gains.html",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt"}),
        updated: || json!({"Subject": "Your receipt", "Html": "<h1>Thanks</h1>"}),
    },
    UpdateCase {
        case: "both bodies are added at once",
        name: "T",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt"}),
        updated: || {
            json!({
                "Subject": "Your receipt",
                "Text": "Thanks for your order.",
                "Html": "<h1>Thanks</h1><p>for your order.</p>",
            })
        },
    },
    UpdateCase {
        case: "the text part is dropped, not inherited",
        name: "drops-text",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt", "Text": "Thanks for your order."}),
        updated: || json!({"Subject": "Your receipt"}),
    },
    UpdateCase {
        case: "the HTML part is dropped while the text part stays",
        name: "drops-html",
        tags: no_tags,
        created: || {
            json!({
                "Subject": "Your receipt",
                "Text": "Thanks for your order.",
                "Html": "<h1>Thanks</h1>",
            })
        },
        updated: || json!({"Subject": "Your receipt", "Text": "Thanks for your order."}),
    },
    UpdateCase {
        case: "the text part is emptied, which is a part and not an absent one",
        name: "EmptiesText",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt", "Text": "Thanks for your order."}),
        updated: || json!({"Subject": "Your receipt", "Text": ""}),
    },
    UpdateCase {
        case: "non-ASCII replaces ASCII",
        name: "non-ascii",
        tags: no_tags,
        created: || json!({"Subject": "Your receipt", "Text": "Thanks"}),
        updated: || {
            json!({
                "Subject": "Ihre Quittung · 領収書",
                "Text": "Grüße 😀",
                "Html": "<p>Grüße 😀</p>",
            })
        },
    },
    UpdateCase {
        case: "substitution placeholders are stored, not resolved",
        name: "placeholders",
        tags: no_tags,
        created: || json!({"Subject": "Hello"}),
        updated: || {
            json!({
                "Subject": "Hello {{name}}",
                "Text": "{{#if vip}}VIP{{else}}guest{{/if}} {{name}}",
                "Html": "<p>Hello {{name}}</p>",
            })
        },
    },
    UpdateCase {
        case: "markup and entities are stored byte for byte",
        name: "markup",
        tags: no_tags,
        created: || json!({"Subject": "plain"}),
        updated: || {
            json!({
                "Subject": "5 > 3 &amp; rising",
                "Html": "<p>a &lt; b</p><script>alert(\"x\")</script>",
            })
        },
    },
    UpdateCase {
        case: "a tagged template keeps its one tag",
        name: "one-tag",
        tags: || vec![("purpose", "receipts")],
        created: || json!({"Subject": "Your receipt"}),
        updated: || json!({"Subject": "Your invoice", "Text": "Please pay."}),
    },
    UpdateCase {
        case: "a tagged template keeps every tag, in the order supplied",
        name: "Order.Receipt_v2-2026__the-one-that-is-sixty-four-characters-long",
        tags: || vec![("zone", "eu"), ("purpose", "receipts"), ("owner", "")],
        created: || json!({"Subject": "Your receipt", "Text": "Thanks."}),
        updated: || json!({"Subject": "Your invoice"}),
    },
];

#[tokio::test]
async fn an_update_replaces_the_stored_content_whole_and_keeps_everything_else() {
    for row in UPDATE_CASES {
        let case = row.case;
        let ses = Ses::new();
        let tags = (row.tags)();
        let created_content = (row.created)();
        let updated_content = (row.updated)();

        let created = ses
            .create_template(create_body(row.name, created_content, &tags))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the seeding create to be accepted with 200, got {} {:?}",
            created.status, created.body
        );

        let updated = ses
            .update_template(row.name, updated_content.clone())
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected the update to be accepted with 200, got {} {:?}",
            updated.status, updated.body
        );
        // `UpdateEmailTemplateResponse` carries no members.
        assert!(
            updated.body.as_object().is_none_or(Map::is_empty),
            "case {case:?}: expected an empty update response body, got {:?}",
            updated.body
        );

        let read = ses.get_template(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the updated template to be readable with 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("TemplateName"),
            json!(row.name),
            "case {case:?}: expected the read to echo the name the template is keyed by, got {:?}",
            read.member("TemplateName")
        );
        assert_eq!(
            read.member("TemplateContent"),
            updated_content,
            "case {case:?}: expected the content the update carried and nothing else, got {:?}",
            read.member("TemplateContent")
        );
        // The tag set the create supplied, as the list it is — empty where the create
        // supplied none; an update changes the content alone.
        let expected_tags = tag_list(&tags);
        assert_eq!(
            read.member("Tags"),
            expected_tags,
            "case {case:?}: expected the tags to survive the update untouched, got {:?}",
            read.member("Tags")
        );
    }
}

#[tokio::test]
async fn the_last_update_is_what_a_read_reports() {
    // The rule holds for any number of updates in a row, and applying the same content
    // twice changes nothing: each row is the sequence of contents sent after the create.
    let sequences: Vec<(&str, Vec<Value>)> = vec![
        ("one update", vec![json!({"Subject": "one"})]),
        (
            "two updates",
            vec![
                json!({"Subject": "one", "Text": "first"}),
                json!({"Subject": "two", "Html": "<p>second</p>"}),
            ],
        ),
        (
            "the same content twice",
            vec![
                json!({"Subject": "same", "Text": "same"}),
                json!({"Subject": "same", "Text": "same"}),
            ],
        ),
        (
            "five updates, the parts coming and going",
            vec![
                json!({"Subject": "a"}),
                json!({"Subject": "b", "Text": "t"}),
                json!({"Subject": "c", "Html": "<p>h</p>"}),
                json!({"Subject": "d", "Text": "t", "Html": "<p>h</p>"}),
                json!({"Subject": "e"}),
            ],
        ),
    ];

    for (case, sequence) in sequences {
        let ses = Ses::new();
        let name = "sequenced";
        let created = ses
            .create_template(create_body(name, json!({"Subject": "created"}), &[]))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
            created.status, created.body
        );

        for content in &sequence {
            let updated = ses.update_template(name, content.clone()).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected each update to be accepted, got {} {:?}",
                updated.status, updated.body
            );
        }

        let last = sequence.last().expect("a sequence has a last content");
        let read = ses.get_template(name).await;
        assert_eq!(
            read.member("TemplateContent"),
            *last,
            "case {case:?}: expected the content of the last update, got {:?}",
            read.member("TemplateContent")
        );
    }
}

#[tokio::test]
async fn updating_a_template_the_account_does_not_hold_is_refused_and_creates_nothing() {
    // (case, the name stored first — if any, the name updated).
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

        let updated = ses
            .update_template(requested, json!({"Subject": "replacement"}))
            .await;
        assert_eq!(
            updated.status, 404,
            "case {case:?}: expected the update to be refused with 404, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            updated.error_code()
        );
        assert_eq!(
            updated.message(),
            format!("Template {requested} does not exist."),
            "case {case:?}: expected the refusal to name the template updated, got {:?}",
            updated.message()
        );

        // The refused update created nothing, so the name still holds nothing.
        let read = ses.get_template(requested).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the refused name to hold nothing, got {} {:?}",
            read.status, read.body
        );

        // A template that was stored under another name is untouched by the refusal.
        if let Some(stored) = stored {
            let read = ses.get_template(stored).await;
            assert_eq!(
                read.member("TemplateContent"),
                json!({"Subject": "s"}),
                "case {case:?}: expected the other template to be untouched, got {:?}",
                read.member("TemplateContent")
            );
        }
    }
}

#[tokio::test]
async fn an_update_carrying_no_subject_line_is_refused_and_the_stored_content_stands() {
    // (case, the TemplateContent the update carries): a subject the request omits and
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
        let name = "keeps-subject";
        let stored = json!({"Subject": "Your receipt", "Text": "Thanks for your order."});
        let created = ses
            .create_template(create_body(name, stored.clone(), &[("purpose", "receipts")]))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
            created.status, created.body
        );

        let refused = ses.update_template(name, content).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the update to be refused with 400, got {} {:?}",
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

        let read = ses.get_template(name).await;
        assert_eq!(
            read.member("TemplateContent"),
            stored,
            "case {case:?}: expected the stored content to survive the refused update, got {:?}",
            read.member("TemplateContent")
        );
    }
}

#[tokio::test]
async fn an_update_may_be_as_large_as_the_documented_maximum_and_no_larger() {
    // SES documents a maximum size of 500 KB for an email template, counted over the
    // subject line and both body parts, so the cases are the bound itself, a size well
    // below it and the first size past it.
    let ceiling = 500 * 1024usize;
    let subject = "Your receipt";
    for size in [subject.len(), ceiling / 2, ceiling, ceiling + 1] {
        let case = format!("{size} bytes of replacement content");
        let ses = Ses::new();
        let name = "sized";
        let stored = json!({"Subject": "Your receipt", "Text": "Thanks."});
        let created = ses
            .create_template(create_body(name, stored.clone(), &[]))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the seeding create to be accepted, got {} {:?}",
            created.status, created.body
        );

        // The subject is required, so the remaining budget goes to the text part.
        let text = "x".repeat(size - subject.len());
        let updated = ses
            .update_template(name, json!({"Subject": subject, "Text": text}))
            .await;

        let read = ses.get_template(name).await;
        if size <= ceiling {
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected the update to be accepted, got {} {:?}",
                updated.status, updated.body
            );
            let stored_len = read
                .member("TemplateContent")
                .get("Text")
                .and_then(Value::as_str)
                .map(str::len);
            assert_eq!(
                stored_len,
                Some(size - subject.len()),
                "case {case:?}: expected the whole text part to be stored, got {stored_len:?} bytes"
            );
        } else {
            assert_eq!(
                updated.status, 400,
                "case {case:?}: expected the update to be refused with 400, got {} {:?}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                updated.error_code()
            );
            assert_eq!(
                updated.message(),
                format!(
                    "Template {name} exceeds maximum allowed size of {} KB.",
                    ceiling / 1024
                ),
                "case {case:?}: expected the refusal to name the template and the quota, got {:?}",
                updated.message()
            );
            assert_eq!(
                read.member("TemplateContent"),
                stored,
                "case {case:?}: expected the stored content to survive the refused update, got {:?}",
                read.member("TemplateContent")
            );
        }
    }
}
