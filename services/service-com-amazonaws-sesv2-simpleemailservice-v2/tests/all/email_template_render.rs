// SPDX-License-Identifier: CC0-1.0

//! `TestRenderEmailTemplate`: the preview of the MIME message a SAVED email template
//! renders for one set of rendering data, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised. The document the preview answers with is read back with `mailparse`,
//! the crate this project reads an RFC 5322 / MIME message with, so the assertions are
//! about the MESSAGE rather than about one spelling of it.
//!
//! The rule these cases pin: a template saved in the account is written in a template
//! language, and a render is that language evaluated against the rendering data — every
//! expression replaced by the value the data states for the attribute it names, every
//! conditional block replaced by the branch its test selects, every iteration block
//! repeated over the values the data lists, and every comment dropped — assembled into
//! one MIME document whose parts are the parts the template carries. The model draws
//! that line itself: `Template$TemplateContent` notes that Amazon SES "supports only
//! simple substitions when you send email using the `SendEmail` or `SendBulkEmail`
//! operations and you provide the full template content in the request", so the
//! restriction to plain substitution belongs to content supplied INLINE in a send, and a
//! saved template — the only kind this operation renders — carries the whole language.
//! Data that states no value for an attribute an expression names renders no message at
//! all: it is refused, which is what makes a preview worth asking for.
//!
//! The tables below span what the rule generalises over: which parts the content carries
//! (subject alone, subject with one body part, subject with both), the construct a part
//! writes (expression, dotted path, conditional, inverted conditional, iteration, nested
//! blocks, comment, no reference at all), the form of a value (text, number, boolean,
//! markup, non-ASCII, a value carrying a line break), and the ways a render is refused
//! (an attribute the data omits, data that is no set of key-value pairs, text the
//! language does not accept, a template the account does not hold).

use std::sync::Arc;

use mailparse::{MailHeaderMap, ParsedMail};
use serde_json::{json, Value};
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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The MIME document the preview answered with, or an empty string when it answered
    /// no `RenderedTemplate` at all.
    fn rendered(&self) -> String {
        self.body
            .get("RenderedTemplate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

    /// Save a template under `name` with `content`.
    async fn create_template(&self, name: &str, content: Value) -> Reply {
        self.call(
            "POST",
            "/v2/email/templates",
            Some(json!({"TemplateName": name, "TemplateContent": content})),
        )
        .await
    }

    async fn get_template(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/templates/{name}"), None)
            .await
    }

    async fn delete_template(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/templates/{name}"), None)
            .await
    }

    /// Render the saved template `name` against `data`.
    async fn render(&self, name: &str, data: &str) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/templates/{name}/render"),
            Some(json!({"TemplateData": data})),
        )
        .await
    }
}

/// The name the cases save their template under, where the name is not what is under
/// test.
const TEMPLATE: &str = "receipt";

/// The body part of `document` whose media type is `media_type`, decoded.
///
/// A document with both body parts is a `multipart/alternative` whose subparts are those
/// parts; one with a single part declares that part's media type at the top level. Both
/// are read the same way here, so a case asserts the PART it means whichever shape the
/// document has.
fn part_of(document: &ParsedMail, media_type: &str) -> Option<String> {
    if document.subparts.is_empty() {
        return (document.ctype.mimetype == media_type)
            .then(|| document.get_body().expect("decode the body"));
    }
    document
        .subparts
        .iter()
        .find(|part| part.ctype.mimetype == media_type)
        .map(|part| part.get_body().expect("decode the body"))
}

/// One row of the render table: the content to save, the data to render it against and
/// the message that has to come back.
struct RenderCase {
    case: &'static str,
    /// The `TemplateContent` the template is saved with.
    content: fn() -> Value,
    /// The `TemplateData` the render carries.
    data: &'static str,
    /// The subject line the document has to carry.
    subject: &'static str,
    /// The text part the document has to carry, if any.
    text: Option<&'static str>,
    /// The HTML part the document has to carry, if any.
    html: Option<&'static str>,
}

/// The rows span the construct a part writes and the form of the value that goes into
/// it. The language's constructs are the ones the model's own template content is
/// written in (`smithy_query(query="doc",shape="Template",member="TemplateContent")`):
/// expressions, conditional blocks, iteration blocks and comments.
const RENDER_CASES: &[RenderCase] = &[
    RenderCase {
        case: "an expression in every part",
        content: || {
            json!({
                "Subject": "Your receipt, {{name}}",
                "Text": "Thanks {{name}}, you paid {{amount}}.",
                "Html": "<p>Thanks {{name}}, you paid {{amount}}.</p>",
            })
        },
        data: r#"{"name":"Ada","amount":"12.50"}"#,
        subject: "Your receipt, Ada",
        text: Some("Thanks Ada, you paid 12.50."),
        html: Some("<p>Thanks Ada, you paid 12.50.</p>"),
    },
    RenderCase {
        case: "the same attribute named twice",
        content: || json!({"Subject": "{{name}} and {{name}}", "Text": "body"}),
        data: r#"{"name":"Ada"}"#,
        subject: "Ada and Ada",
        text: Some("body"),
        html: None,
    },
    RenderCase {
        case: "whitespace inside the braces",
        content: || json!({"Subject": "Hi {{ name }}", "Text": "body"}),
        data: r#"{"name":"Ada"}"#,
        subject: "Hi Ada",
        text: Some("body"),
        html: None,
    },
    RenderCase {
        case: "a dotted path into the data",
        content: || json!({"Subject": "Hi {{customer.first}} {{customer.last}}", "Text": "body"}),
        data: r#"{"customer":{"first":"Ada","last":"Lovelace"}}"#,
        subject: "Hi Ada Lovelace",
        text: Some("body"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test the data states true",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}VIP{{else}}guest{{/if}}"}),
        data: r#"{"vip":true}"#,
        subject: "s",
        text: Some("VIP"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test the data states false",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}VIP{{else}}guest{{/if}}"}),
        data: r#"{"vip":false}"#,
        subject: "s",
        text: Some("guest"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test the data states as text",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}VIP{{else}}guest{{/if}}"}),
        data: r#"{"vip":"yes"}"#,
        subject: "s",
        text: Some("VIP"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test the data states as empty text",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}VIP{{else}}guest{{/if}}"}),
        data: r#"{"vip":""}"#,
        subject: "s",
        text: Some("guest"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test the data omits, which is a test for presence",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}VIP{{else}}guest{{/if}}"}),
        data: "{}",
        subject: "s",
        text: Some("guest"),
        html: None,
    },
    RenderCase {
        case: "a conditional with no else branch",
        content: || json!({"Subject": "s", "Text": "a{{#if vip}} VIP{{/if}}"}),
        data: r#"{"vip":false}"#,
        subject: "s",
        text: Some("a"),
        html: None,
    },
    RenderCase {
        case: "an inverted conditional",
        content: || json!({"Subject": "s", "Text": "{{#unless vip}}guest{{/unless}}"}),
        data: r#"{"vip":false}"#,
        subject: "s",
        text: Some("guest"),
        html: None,
    },
    RenderCase {
        case: "an iteration over the values the data lists",
        content: || json!({"Subject": "s", "Text": "{{#each items}}{{this}};{{/each}}"}),
        data: r#"{"items":["a","b","c"]}"#,
        subject: "s",
        text: Some("a;b;c;"),
        html: None,
    },
    RenderCase {
        case: "an iteration over an empty list",
        content: || json!({"Subject": "s", "Text": "[{{#each items}}{{this}}{{/each}}]"}),
        data: r#"{"items":[]}"#,
        subject: "s",
        text: Some("[]"),
        html: None,
    },
    RenderCase {
        case: "an iteration whose items are objects",
        content: || {
            json!({
                "Subject": "s",
                "Text": "{{#each lines}}{{name}} x{{count}}\n{{/each}}",
            })
        },
        data: r#"{"lines":[{"name":"pen","count":2},{"name":"ink","count":1}]}"#,
        subject: "s",
        text: Some("pen x2\nink x1\n"),
        html: None,
    },
    RenderCase {
        case: "a conditional nested inside an iteration",
        content: || {
            json!({
                "Subject": "s",
                "Text": "{{#each orders}}{{id}} {{#if paid}}paid{{else}}due{{/if}};{{/each}}",
            })
        },
        data: r#"{"orders":[{"id":"A","paid":true},{"id":"B","paid":false}]}"#,
        subject: "s",
        text: Some("A paid;B due;"),
        html: None,
    },
    RenderCase {
        case: "a conditional whose test is a subexpression the language defines",
        content: || json!({"Subject": "s", "Text": "{{#if (eq name \"Ada\")}}known{{else}}new{{/if}}"}),
        data: r#"{"name":"Ada"}"#,
        subject: "s",
        text: Some("known"),
        html: None,
    },
    RenderCase {
        case: "a comment, which renders as nothing",
        content: || json!({"Subject": "Hi{{! not sent }} there", "Text": "body"}),
        data: "{}",
        subject: "Hi there",
        text: Some("body"),
        html: None,
    },
    RenderCase {
        case: "no reference at all, so the data states nothing",
        content: || json!({"Subject": "Your receipt", "Text": "Thanks.", "Html": "<p>Thanks.</p>"}),
        data: "{}",
        subject: "Your receipt",
        text: Some("Thanks."),
        html: Some("<p>Thanks.</p>"),
    },
    RenderCase {
        case: "no reference at all and no data at all, which is the empty set of values",
        content: || json!({"Subject": "Your receipt", "Text": "Thanks."}),
        data: "",
        subject: "Your receipt",
        text: Some("Thanks."),
        html: None,
    },
    RenderCase {
        case: "a number, a fractional number and a boolean",
        content: || json!({"Subject": "s", "Text": "{{a}}/{{b}}/{{c}}"}),
        data: r#"{"a":42,"b":4.5,"c":false}"#,
        subject: "s",
        text: Some("42/4.5/false"),
        html: None,
    },
    RenderCase {
        case: "a value carrying markup: escaped in the HTML part, as written elsewhere",
        content: || {
            json!({
                "Subject": "Hi {{name}}",
                "Text": "Hi {{name}}",
                "Html": "<p>{{name}}</p><p>{{{name}}}</p>",
            })
        },
        data: r#"{"name":"<b>Ada</b> & Grace"}"#,
        subject: "Hi <b>Ada</b> & Grace",
        text: Some("Hi <b>Ada</b> & Grace"),
        html: Some("<p>&lt;b&gt;Ada&lt;/b&gt; &amp; Grace</p><p><b>Ada</b> & Grace</p>"),
    },
    RenderCase {
        case: "non-ASCII template text and non-ASCII values",
        content: || {
            json!({
                "Subject": "Ihre Quittung, {{name}} · 領収書",
                "Text": "Grüße {{name}} 😀",
                "Html": "<p>Grüße {{name}} 😀</p>",
            })
        },
        data: r#"{"name":"Ada Lovelace"}"#,
        subject: "Ihre Quittung, Ada Lovelace · 領収書",
        text: Some("Grüße Ada Lovelace 😀"),
        html: Some("<p>Grüße Ada Lovelace 😀</p>"),
    },
    RenderCase {
        case: "an empty body part is a part, rendered empty",
        content: || json!({"Subject": "Hi {{name}}", "Text": ""}),
        data: r#"{"name":"Ada"}"#,
        subject: "Hi Ada",
        text: Some(""),
        html: None,
    },
    RenderCase {
        case: "a value that itself reads like an expression, evaluated once",
        content: || json!({"Subject": "Hi {{name}}", "Text": "body"}),
        data: r#"{"name":"{{name}}"}"#,
        subject: "Hi {{name}}",
        text: Some("body"),
        html: None,
    },
];

#[tokio::test]
async fn the_preview_is_the_saved_template_evaluated_against_the_rendering_data() {
    for row in RENDER_CASES {
        let case = row.case;
        let ses = Ses::new();
        let created = ses.create_template(TEMPLATE, (row.content)()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, row.data).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (200, String::new()),
            "case {case:?}: expected the render to be answered with 200, got {} {:?}",
            reply.status,
            reply.body
        );
        let document = reply.rendered();
        let parsed = mailparse::parse_mail(document.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: expected a MIME document, got {error} for {document:?}"));

        assert_eq!(
            parsed.headers.get_first_value("Subject").as_deref(),
            Some(row.subject),
            "case {case:?}: expected the rendered subject line, got {:?} in {document:?}",
            parsed.headers.get_first_value("Subject")
        );
        assert_eq!(
            part_of(&parsed, "text/plain").as_deref(),
            row.text,
            "case {case:?}: expected the rendered text part, got {:?} in {document:?}",
            part_of(&parsed, "text/plain")
        );
        assert_eq!(
            part_of(&parsed, "text/html").as_deref(),
            row.html,
            "case {case:?}: expected the rendered HTML part, got {:?} in {document:?}",
            part_of(&parsed, "text/html")
        );
        // Nothing of the template's own language is left in the message — except where
        // the row's own expected message carries those braces, which is a VALUE that
        // reads like an expression and is written out as the value it is.
        let expects_braces = row.subject.contains("{{")
            || row.text.is_some_and(|part| part.contains("{{"))
            || row.html.is_some_and(|part| part.contains("{{"));
        assert!(
            expects_braces || !document.contains("{{"),
            "case {case:?}: expected every reference to be evaluated, got {document:?}"
        );
    }
}

/// One row of the document-shape table: the parts the template carries and the media
/// types the document has to declare, in order.
struct ShapeCase {
    case: &'static str,
    content: fn() -> Value,
    /// The media type of the document itself.
    top_level: &'static str,
    /// The media types of its subparts, in the order the document writes them.
    subparts: &'static [&'static str],
}

/// The rows span which body parts a template carries, since the parts of the message are
/// the parts of the template.
const SHAPE_CASES: &[ShapeCase] = &[
    ShapeCase {
        case: "a subject line and both body parts",
        content: || json!({"Subject": "s", "Text": "plain", "Html": "<p>html</p>"}),
        top_level: "multipart/alternative",
        subparts: &["text/plain", "text/html"],
    },
    ShapeCase {
        case: "a subject line and the text part alone",
        content: || json!({"Subject": "s", "Text": "plain"}),
        top_level: "text/plain",
        subparts: &[],
    },
    ShapeCase {
        case: "a subject line and the HTML part alone",
        content: || json!({"Subject": "s", "Html": "<p>html</p>"}),
        top_level: "text/html",
        subparts: &[],
    },
    ShapeCase {
        case: "a subject line alone, which renders a message with no body",
        content: || json!({"Subject": "s"}),
        top_level: "text/plain",
        subparts: &[],
    },
];

#[tokio::test]
async fn the_document_carries_the_parts_the_template_carries() {
    for row in SHAPE_CASES {
        let case = row.case;
        let ses = Ses::new();
        let created = ses.create_template(TEMPLATE, (row.content)()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, "{}").await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the render to be answered with 200, got {} {:?}",
            reply.status, reply.body
        );
        let document = reply.rendered();
        let parsed = mailparse::parse_mail(document.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: expected a MIME document, got {error} for {document:?}"));

        assert_eq!(
            parsed.ctype.mimetype, row.top_level,
            "case {case:?}: expected the document's own media type, got {:?} in {document:?}",
            parsed.ctype.mimetype
        );
        let subparts: Vec<String> = parsed
            .subparts
            .iter()
            .map(|part| part.ctype.mimetype.clone())
            .collect();
        assert_eq!(
            subparts, row.subparts,
            "case {case:?}: expected the document's parts in order, got {subparts:?} in {document:?}"
        );
        assert_eq!(
            parsed.headers.get_first_value("MIME-Version").as_deref(),
            Some("1.0"),
            "case {case:?}: expected the document to declare its MIME version, got {document:?}"
        );
        // Every part states the character set its text is in, whichever shape the
        // document has.
        let declared: Vec<&ParsedMail> = if parsed.subparts.is_empty() {
            vec![&parsed]
        } else {
            parsed.subparts.iter().collect()
        };
        for part in declared {
            assert_eq!(
                part.ctype.params.get("charset").map(String::as_str),
                Some("UTF-8"),
                "case {case:?}: expected every part to declare UTF-8, got {:?} in {document:?}",
                part.ctype.params.get("charset")
            );
        }
    }
}

/// One row of the refused-attribute table: the content to save, the data to render it
/// against and the attribute the refusal has to name.
struct MissingCase {
    case: &'static str,
    content: fn() -> Value,
    data: &'static str,
    /// The attribute's name as the template wrote it.
    attribute: &'static str,
}

/// The rows span where the expression that names the attribute sits and what kind of
/// reference it is.
const MISSING_CASES: &[MissingCase] = &[
    MissingCase {
        case: "an expression in the subject line",
        content: || json!({"Subject": "Hi {{name}}", "Text": "body"}),
        data: "{}",
        attribute: "name",
    },
    MissingCase {
        case: "an expression in the text part",
        content: || json!({"Subject": "s", "Text": "Hi {{name}}"}),
        data: r#"{"other":"Ada"}"#,
        attribute: "name",
    },
    MissingCase {
        case: "an expression in the HTML part",
        content: || json!({"Subject": "s", "Text": "body", "Html": "<p>{{name}}</p>"}),
        data: "{}",
        attribute: "name",
    },
    MissingCase {
        case: "the unescaped spelling of an expression",
        content: || json!({"Subject": "s", "Html": "<p>{{{name}}}</p>"}),
        data: "{}",
        attribute: "name",
    },
    MissingCase {
        case: "a dotted path whose root the data states",
        content: || json!({"Subject": "Hi {{customer.first}}", "Text": "body"}),
        data: r#"{"customer":{"last":"Lovelace"}}"#,
        attribute: "customer.first",
    },
    MissingCase {
        case: "a dotted path the data states nothing under at all",
        content: || json!({"Subject": "Hi {{customer.first}}", "Text": "body"}),
        data: "{}",
        attribute: "customer.first",
    },
    MissingCase {
        case: "an iteration over an attribute the data omits",
        content: || json!({"Subject": "s", "Text": "{{#each items}}{{this}}{{/each}}"}),
        data: "{}",
        attribute: "items",
    },
    MissingCase {
        case: "an expression inside the branch a conditional selects",
        content: || json!({"Subject": "s", "Text": "{{#if vip}}Hi {{name}}{{/if}}"}),
        data: r#"{"vip":true}"#,
        attribute: "name",
    },
    MissingCase {
        case: "no data at all, which states no value for anything",
        content: || json!({"Subject": "Hi {{name}}", "Text": "body"}),
        data: "",
        attribute: "name",
    },
];

#[tokio::test]
async fn data_that_states_no_value_for_an_attribute_the_template_names_is_refused() {
    for row in MISSING_CASES {
        let case = row.case;
        let ses = Ses::new();
        let created = ses.create_template(TEMPLATE, (row.content)()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, row.data).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected the render to be refused, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "Attribute '{}' is not present in the rendering data.",
                row.attribute
            ),
            "case {case:?}: expected the refusal to name the attribute, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn rendering_data_that_is_no_set_of_key_value_pairs_is_refused() {
    // The model: `TemplateData` "is a set of key-value pairs", so a JSON value that is no
    // object states no values for anything. The rows span what else text can be.
    for (case, data) in [
        ("text that is no JSON at all", "not json"),
        ("a JSON array", r#"["name","Ada"]"#),
        ("a JSON string", r#""Ada""#),
        ("a JSON number", "42"),
        ("a JSON null", "null"),
        ("a JSON boolean", "true"),
        ("an object that never closes", r#"{"name":"Ada""#),
    ] {
        let ses = Ses::new();
        let created = ses
            .create_template(TEMPLATE, json!({"Subject": "Hi {{name}}", "Text": "body"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, data).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected the render to be refused, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            "The template data is not a valid JSON object",
            "case {case:?}: expected the refusal to name the data, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_saved_template_the_language_does_not_accept_renders_nothing() {
    // The rows span the ways text can fail to be a template: a reference that never
    // closes, one that names nothing, a block that never closes (in both spellings of a
    // block tag), a closing tag with nothing to close, a call to a helper the language
    // does not define, and nesting past what the engine's own parse accepts — the last
    // one being how a template too deep to parse is reported rather than how deeply
    // anything here recurses.
    let cases: Vec<(&str, String)> = vec![
        ("a reference that never closes", "Hi {{name".to_string()),
        ("a reference that names nothing", "Hi {{}}".to_string()),
        ("a block that never closes", "{{#if vip}}VIP".to_string()),
        (
            "a block that never closes, written to omit whitespace",
            "{{~#if vip~}}VIP".to_string(),
        ),
        ("a closing tag with no block", "VIP{{/if}}".to_string()),
        (
            "a helper the language does not define",
            "{{formatDate when}}".to_string(),
        ),
    ];
    for (case, part) in cases {
        let ses = Ses::new();
        let created = ses
            .create_template(TEMPLATE, json!({"Subject": "s", "Text": part}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, "{}").await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected the render to be refused, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            reply
                .message()
                .starts_with(&format!("Template {TEMPLATE} is not a valid template:")),
            "case {case:?}: expected the refusal to name the template, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_render_of_a_template_the_account_does_not_hold_is_refused() {
    let ses = Ses::new();
    let created = ses
        .create_template(TEMPLATE, json!({"Subject": "s", "Text": "body"}))
        .await;
    assert_eq!(
        created.status, 200,
        "expected the template to be saved, got {} {:?}",
        created.status, created.body
    );
    let deleted = ses.delete_template("gone").await;
    assert_eq!(
        deleted.status, 404,
        "expected a name the account never held to be unknown to the delete too, got {} {:?}",
        deleted.status, deleted.body
    );

    // The rows span how a name can fail to address a saved template: one never saved, one
    // that differs from a saved one only in case (a template name is the key it was saved
    // under), and one whose template was saved and then deleted.
    let removed = ses.delete_template(TEMPLATE).await;
    assert_eq!(
        removed.status, 200,
        "expected the saved template to be deletable, got {} {:?}",
        removed.status, removed.body
    );
    for (case, name) in [
        ("a name the account never held", "unknown"),
        ("a name differing only in case", "Receipt"),
        ("a name whose template was deleted", TEMPLATE),
    ] {
        let reply = ses.render(name, "{}").await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected the render to be refused, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("Template {name} does not exist."),
            "case {case:?}: expected the refusal to name the template, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_preview_changes_nothing_a_later_read_or_render_sees() {
    // A preview reports; it does not write. The stored template still carries its own
    // language verbatim afterwards, and a second render with other data answers that
    // other message rather than the first one again.
    let ses = Ses::new();
    let content = json!({
        "Subject": "Hi {{name}}",
        "Text": "{{#if vip}}VIP{{else}}guest{{/if}} {{name}}",
    });
    let created = ses.create_template(TEMPLATE, content.clone()).await;
    assert_eq!(
        created.status, 200,
        "expected the template to be saved, got {} {:?}",
        created.status, created.body
    );

    for (case, data, subject, text) in [
        ("the first render", r#"{"name":"Ada","vip":true}"#, "Hi Ada", "VIP Ada"),
        (
            "a second render with other data",
            r#"{"name":"Grace","vip":false}"#,
            "Hi Grace",
            "guest Grace",
        ),
        (
            "the first data again, which answers the same message again",
            r#"{"name":"Ada","vip":true}"#,
            "Hi Ada",
            "VIP Ada",
        ),
    ] {
        let reply = ses.render(TEMPLATE, data).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the render to be answered with 200, got {} {:?}",
            reply.status, reply.body
        );
        let document = reply.rendered();
        let parsed = mailparse::parse_mail(document.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: expected a MIME document, got {error}"));
        assert_eq!(
            parsed.headers.get_first_value("Subject").as_deref(),
            Some(subject),
            "case {case:?}: expected this data's subject line, got {document:?}"
        );
        assert_eq!(
            part_of(&parsed, "text/plain").as_deref(),
            Some(text),
            "case {case:?}: expected this data's text part, got {document:?}"
        );

        let read = ses.get_template(TEMPLATE).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the template to still be readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body.get("TemplateContent"),
            Some(&content),
            "case {case:?}: expected the stored content to be unchanged, got {:?}",
            read.body.get("TemplateContent")
        );
    }
}

#[tokio::test]
async fn a_render_past_what_this_emulator_renders_produces_no_message() {
    // These are the EMULATOR's own ceilings and the one construct it does not evaluate,
    // not SES refusals, so each is answered on the 501 channel that says "this emulator
    // does not render that" rather than as a modelled refusal of a valid request. The
    // rows span them: the bytes one part produces; how deeply a template nests blocks,
    // which the evaluation recurses through — spelled plainly, spelled to omit
    // whitespace, and with each level carrying a comment or a string literal that reads
    // like a closing tag, since the nesting is counted off the engine's own parse tree
    // rather than guessed from the text; how many block bodies a part evaluates over the
    // data it is given, which is the cost a block whose body writes nothing still has;
    // and a partial or a decorator, the constructs that expand a template inside a
    // template.
    let value = "x".repeat(100 * 1024);
    let wide: Vec<Value> = (0..200).map(|index| json!(index)).collect();
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "a part that renders past the byte ceiling",
            json!({"Subject": "{{blob}}{{blob}}{{blob}}{{blob}}{{blob}}{{blob}}", "Text": "b"}),
            json!({"blob": value}).to_string(),
        ),
        (
            "a template that nests blocks past the nesting ceiling",
            json!({
                "Subject": "s",
                "Text": format!("{}deep{}", "{{#if flag}}".repeat(65), "{{/if}}".repeat(65)),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "the same nesting written to omit whitespace",
            json!({
                "Subject": "s",
                "Text": format!("{}deep{}", "{{~#if flag~}}".repeat(65), "{{~/if~}}".repeat(65)),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "the same nesting with a comment that reads like a closing tag at each level",
            json!({
                "Subject": "s",
                "Text": format!(
                    "{}deep{}",
                    "{{#if flag}}{{!-- {{/if}} --}}".repeat(65),
                    "{{/if}}".repeat(65)
                ),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "the same nesting with a string literal that reads like a tag's close",
            json!({
                "Subject": "s",
                "Text": format!(
                    "{}deep{}",
                    "{{#if (eq flag \"}}\")}}".repeat(65),
                    "{{/if}}".repeat(65)
                ),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "a template that writes more tags than are parsed",
            json!({
                "Subject": "s",
                "Text": "{{flag}} ".repeat(1_025),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "a template whose nested blocks are more tags than are parsed",
            json!({
                "Subject": "s",
                "Text": format!(
                    "{}deep{}",
                    "{{#if flag}}".repeat(600),
                    "{{/if}}".repeat(600)
                ),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "a template carrying more parentheses than are parsed",
            json!({
                "Subject": "s",
                "Text": format!("{{{{eq {}flag{}}}}}", "(eq ".repeat(300), ")".repeat(300)),
            }),
            json!({"flag": true}).to_string(),
        ),
        (
            "a body of many expressions inside nested iterations",
            json!({
                "Subject": "s",
                "Text": format!(
                    "{{{{#each items}}}}{{{{#each @root.items}}}}{}{{{{/each}}}}{{{{/each}}}}",
                    "{{empty}}".repeat(400)
                ),
            }),
            json!({"items": (0..700).map(|index| json!(index)).collect::<Vec<Value>>(), "empty": ""})
                .to_string(),
        ),
        (
            "blocks that would evaluate more bodies than the work ceiling",
            json!({
                "Subject": "s",
                "Text": "{{#each items}}{{#each @root.items}}{{#each @root.items}}{{/each}}{{/each}}{{/each}}",
            }),
            json!({"items": wide}).to_string(),
        ),
        (
            "the same blocks written to omit whitespace",
            json!({
                "Subject": "s",
                "Text": "{{~#each items~}}{{~#each @root.items~}}{{~#each @root.items~}}{{~/each~}}{{~/each~}}{{~/each~}}",
            }),
            json!({"items": wide}).to_string(),
        ),
        (
            "the same blocks with a comment that reads like a closing tag at each level",
            json!({
                "Subject": "s",
                "Text": "{{#each items}}{{!-- {{/each}} --}}{{#each @root.items}}{{!-- {{/each}} --}}{{#each @root.items}}{{/each}}{{/each}}{{/each}}",
            }),
            json!({"items": wide}).to_string(),
        ),
        (
            "a template that includes a partial",
            json!({"Subject": "s", "Text": "Hi {{> greeting}}"}),
            "{}".to_string(),
        ),
        (
            "a template that defines an inline partial and includes it",
            json!({
                "Subject": "s",
                "Text": "{{#*inline \"greeting\"}}Hi{{/inline}}{{> greeting}}",
            }),
            "{}".to_string(),
        ),
        (
            "a template whose partials include each other",
            json!({
                "Subject": "s",
                "Text": "{{#*inline \"a\"}}{{> b}}{{/inline}}{{#*inline \"b\"}}{{> a}}{{/inline}}{{> a}}",
            }),
            "{}".to_string(),
        ),
    ];

    for (case, content, data) in cases {
        let ses = Ses::new();
        let created = ses.create_template(TEMPLATE, content).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, &data).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (501, "NotImplemented"),
            "case {case:?}: expected the render to be answered as beyond this emulator, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

#[tokio::test]
async fn templates_within_the_ceilings_are_evaluated() {
    // The other side of those ceilings, so neither is asserted by a check that would pass
    // against a handler that refused everything: the same shapes, sized to stay inside
    // the bounds, render the messages they describe. Three iteration blocks nested over
    // fifty values is 375000 block bodies, under the million the work ceiling allows;
    // conditional blocks evaluate their body at most once, so nesting them multiplies
    // nothing however many attributes the data states.
    let narrow: Vec<Value> = (0..50).map(|index| json!(index)).collect();
    let attributes: Value = Value::Object(
        (0..20)
            .map(|index| (format!("attribute{index}"), json!(format!("value{index}"))))
            .collect(),
    );
    let cases: Vec<(&str, Value, String, &str)> = vec![
        (
            "iteration blocks nested inside the work ceiling",
            json!({
                "Subject": "s",
                "Text": "{{#each items}}{{#each @root.items}}{{#each @root.items}}{{/each}}{{/each}}{{/each}}ok",
            }),
            json!({"items": narrow}).to_string(),
            "ok",
        ),
        (
            "a body of many expressions, evaluated once",
            json!({
                "Subject": "s",
                "Text": format!("{}ok", "{{empty}}".repeat(500)),
            }),
            json!({"empty": ""}).to_string(),
            "ok",
        ),
        (
            "five conditional blocks deep over data stating twenty attributes",
            json!({
                "Subject": "s",
                "Text": format!(
                    "{}ok{}",
                    "{{#if attribute0}}".repeat(5),
                    "{{/if}}".repeat(5)
                ),
            }),
            attributes.to_string(),
            "ok",
        ),
    ];

    for (case, content, data, text) in cases {
        let ses = Ses::new();
        let created = ses.create_template(TEMPLATE, content).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let reply = ses.render(TEMPLATE, &data).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the render to be answered with 200, got {} {:?}",
            reply.status, reply.body
        );
        let document = reply.rendered();
        let parsed = mailparse::parse_mail(document.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: expected a MIME document, got {error}"));
        assert_eq!(
            part_of(&parsed, "text/plain").as_deref(),
            Some(text),
            "case {case:?}: expected the text part the template describes, got {document:?}"
        );
    }
}

#[tokio::test]
async fn a_rendered_subject_line_cannot_add_a_line_to_the_document() {
    // A header field is one line of US-ASCII text (RFC 5322 section 2.2) of at most 998
    // octets (section 2.1.1), and the value substituted into a subject line is a
    // caller's: text that is not that goes in as RFC 2047 encoded words, which decode
    // back to exactly what was rendered and carry no line of their own. The rows span
    // what such a value can be, including one too long for any line.
    let long_ascii = "Ada ".repeat(400);
    let long_unicode = "Grüße ".repeat(200);
    let cases: Vec<(&str, String)> = vec![
        (
            "a value carrying a line break and a header of its own",
            "Ada\r\nBcc: attacker@example.net".to_string(),
        ),
        ("a value carrying a bare newline", "Ada\nGrace".to_string()),
        ("a non-ASCII value", "Grüße 😀".to_string()),
        ("a value longer than a line may be", long_ascii),
        (
            "a non-ASCII value longer than a line may be",
            long_unicode,
        ),
    ];
    for (case, value) in cases {
        let ses = Ses::new();
        let created = ses
            .create_template(TEMPLATE, json!({"Subject": "Hi {{name}}", "Text": "body"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be saved, got {} {:?}",
            created.status, created.body
        );

        let data = json!({"name": value}).to_string();
        let reply = ses.render(TEMPLATE, &data).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the render to be answered with 200, got {} {:?}",
            reply.status, reply.body
        );
        let document = reply.rendered();
        let parsed = mailparse::parse_mail(document.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: expected a MIME document, got {error}"));
        assert_eq!(
            parsed.headers.get_first_value("Subject").as_deref(),
            Some(format!("Hi {value}").as_str()),
            "case {case:?}: expected the whole value to decode back into the subject, got {:?}",
            parsed.headers.get_first_value("Subject")
        );
        // The value named a header; the document carries no such header, because the
        // value went in as encoded words rather than as lines.
        assert_eq!(
            parsed.headers.get_first_value("Bcc"),
            None,
            "case {case:?}: expected no header the value named, got {document:?}"
        );
        let headers: Vec<String> = parsed
            .headers
            .iter()
            .map(|header| header.get_key())
            .collect();
        assert_eq!(
            headers,
            vec![
                "Subject".to_string(),
                "MIME-Version".to_string(),
                "Content-Type".to_string(),
                "Content-Transfer-Encoding".to_string(),
            ],
            "case {case:?}: expected the document's own header block, got {headers:?} in {document:?}"
        );
        // Every line of the document is one a message may carry.
        let longest = document
            .split("\r\n")
            .map(|line| line.len())
            .max()
            .unwrap_or_default();
        assert!(
            longest <= 998,
            "case {case:?}: expected every line to be at most 998 octets, got one of {longest} in {document:?}"
        );
    }
}
