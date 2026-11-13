// SPDX-License-Identifier: CC0-1.0

//! `TestRenderTemplate`: the preview of the MIME message an email template of this account
//! composes over a request's replacement values, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call is a real
//! awsQuery request against the service. What is asserted is the RULE the operation follows
//! over a SET of inputs rather than one request:
//!
//! * a replacement tag stands for the value the replacement data holds under it — whatever
//!   that value is, and wherever the tag sits (the subject line, either body part, a nested
//!   path, a block's body) — and a block takes the branch the data chooses or repeats its body
//!   once per element of what it names; a value substituted into the HTML body is escaped as
//!   markup, one substituted into the subject line or the text body is not;
//! * the preview is ONE MIME message: the rendered subject line as its `Subject` header, the
//!   instant it was composed as its `Date` header, and the rendered body parts as its
//!   content — the two of them as the alternatives of a `multipart/alternative` message when
//!   the template carries both;
//! * a replacement tag the rendering REACHES and the data holds no value for is refused as the
//!   modelled `MissingRenderingAttribute`, naming that tag, while a tag in a branch the data
//!   did not take is never reached and never missing;
//! * replacement data that is not a JSON object of replacement values is refused as the
//!   modelled `InvalidRenderingParameter`, and so is a rendering past the bounds this service
//!   composes a message within;
//! * a name the account holds no template under is refused as the modelled
//!   `TemplateDoesNotExist`;
//! * the preview REPORTS: nothing about the stored template changes, so a second preview and
//!   the reader that follows it answer exactly what the first one did.

use crate::harness::{account, call, element, emulator, field, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// One part of a rendered message: its content type, the transfer encoding it is declared
/// under, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    content_type: String,
    transfer_encoding: String,
    body: String,
}

impl Part {
    fn new(content_type: &str, transfer_encoding: &str, body: &str) -> Self {
        Self {
            content_type: content_type.to_string(),
            transfer_encoding: transfer_encoding.to_string(),
            body: body.to_string(),
        }
    }
}

/// A rendered message as this suite reads it: the `Subject` header and the body parts, so an
/// assertion is about the message's content rather than about the boundary the composition
/// drew for it or the instant it was composed at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    subject: Option<String>,
    parts: Vec<Part>,
}

/// One `CreateTemplate` call, carrying only the parts the case names.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    subject: Option<&str>,
    text: Option<&str>,
    html: Option<&str>,
) {
    let mut params: Vec<Field> = vec![field("Template.TemplateName", name)];
    for (member, value) in [
        ("Template.SubjectPart", subject),
        ("Template.TextPart", text),
        ("Template.HtmlPart", html),
    ] {
        if let Some(value) = value {
            params.push(field(member, value));
        }
    }
    let (status, body) = call(router, "CreateTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from CreateTemplate, got {status} with {body}"
    );
}

/// One `TestRenderTemplate` call: `(status, body)`.
async fn render_raw(
    router: &MultiServiceRouter,
    name: &str,
    template_data: &str,
) -> (u16, String) {
    call(
        router,
        "TestRenderTemplate",
        &[
            field("TemplateName", name),
            field("TemplateData", template_data),
        ],
    )
    .await
}

/// `TestRenderTemplate`, as the message it rendered. Asserts the call was served.
async fn rendered(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    template_data: &str,
) -> Message {
    let (status, body) = render_raw(router, name, template_data).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from TestRenderTemplate, got {status} with {body}"
    );
    let rendered = element(&body, "RenderedTemplate")
        .map(|text| xml_text(&text))
        .unwrap_or_else(|| panic!("case {case}: expected a RenderedTemplate, got {body}"));
    read_message(case, &rendered)
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error_envelope(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|text| xml_text(&text)),
    )
}

/// A MIME message read as the [`Message`] this suite asserts on.
///
/// The line endings a MIME message is written with are normalised away first — an XML body
/// carries a carriage return as itself or as the `&#xD;` reference, and neither is what an
/// assertion about the message's content is about.
fn read_message(case: &str, message: &str) -> Message {
    let message = message.replace("&#xD;", "").replace('\r', "");
    let (headers, body) = split_once_blank_line(case, &message);
    let subject = header(&headers, "Subject");
    let content_type = header(&headers, "Content-Type").unwrap_or_default();
    assert_eq!(
        header(&headers, "MIME-Version").as_deref(),
        Some("1.0"),
        "case {case}: expected a MIME-Version header, got {message:?}"
    );
    // Every rendered message carries the origination date of the message it previews, in the
    // date-and-time form RFC 5322 lays out: a weekday, the day, the month's name, a 4-digit
    // year, a time of day and the UTC offset. The instant itself is the one the preview was
    // composed at, so only its FORM is asserted.
    let date = header(&headers, "Date").unwrap_or_default();
    let date_fields: Vec<&str> = date.split_whitespace().collect();
    assert_eq!(
        date_fields.len(),
        6,
        "case {case}: expected an RFC 5322 Date header, got {date:?}"
    );
    assert!(
        date_fields
            .first()
            .is_some_and(|weekday| weekday.len() == 4 && weekday.ends_with(',')),
        "case {case}: expected the Date header to open with a weekday, got {date:?}"
    );
    assert!(
        date_fields
            .get(3)
            .is_some_and(|year| year.len() == 4 && year.chars().all(|digit| digit.is_ascii_digit())),
        "case {case}: expected the Date header to carry a 4-digit year, got {date:?}"
    );
    assert!(
        date_fields
            .get(4)
            .is_some_and(|time| time.split(':').count() == 3),
        "case {case}: expected the Date header to carry a time of day, got {date:?}"
    );
    assert!(
        date_fields.get(5).is_some_and(|zone| zone.starts_with('+')
            || zone.starts_with('-')
            || zone.chars().all(|letter| letter.is_ascii_uppercase())),
        "case {case}: expected the Date header to carry a zone, got {date:?}"
    );
    let parts = match boundary_of(&content_type) {
        None => vec![Part::new(
            &content_type,
            &header(&headers, "Content-Transfer-Encoding").unwrap_or_default(),
            &body,
        )],
        Some(boundary) => {
            assert!(
                content_type.starts_with("multipart/alternative;"),
                "case {case}: expected a multipart/alternative message, got {content_type:?}"
            );
            assert!(
                body.contains(&format!("--{boundary}--")),
                "case {case}: expected the multipart message to be closed, got {body:?}"
            );
            let mut parts = Vec::new();
            for chunk in body.split(&format!("--{boundary}")) {
                let chunk = chunk.trim_start_matches('\n');
                // The preamble before the first delimiter and the closing `--` after the
                // last one are not parts.
                if chunk.is_empty() || chunk.starts_with("--") {
                    continue;
                }
                let (part_headers, part_body) = split_once_blank_line(case, chunk);
                parts.push(Part::new(
                    &header(&part_headers, "Content-Type").unwrap_or_default(),
                    &header(&part_headers, "Content-Transfer-Encoding").unwrap_or_default(),
                    part_body.trim_end_matches('\n'),
                ));
            }
            parts
        }
    };
    Message { subject, parts }
}

/// A message or a part split into its header block and its body, at the blank line between
/// them.
fn split_once_blank_line(case: &str, text: &str) -> (String, String) {
    let Some((headers, body)) = text.split_once("\n\n") else {
        panic!("case {case}: expected a header block and a body, got {text:?}");
    };
    (headers.to_string(), body.to_string())
}

/// The value of the header `name` in a header block.
fn header(headers: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}: ");
    headers
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .map(str::to_string)
}

/// The boundary a `multipart` content type declares, or `None` for a single-part message.
fn boundary_of(content_type: &str) -> Option<String> {
    let after = content_type.split("boundary=\"").nth(1)?;
    after.split('"').next().map(str::to_string)
}

/// One success case: a template, the replacement values, and the message the rendering must
/// compose.
struct RenderCase {
    case: &'static str,
    subject: &'static str,
    text: Option<&'static str>,
    html: Option<&'static str>,
    data: &'static str,
    expected_subject: &'static str,
    expected_parts: Vec<Part>,
}

/// RULE: a replacement tag stands for the value the replacement data holds under it, a block
/// takes the branch the data chooses or repeats its body over what it names, and the preview
/// is the one MIME message the rendered parts compose.
///
/// The rows differ along what that rule generalises over: which parts the template carries,
/// the TYPE of the replacement value (text, a number, a boolean, a nested object, a list), the
/// FORM of the tag (a bare tag, a nested path, a conditional, a repetition), and content that
/// is not ASCII or carries markup characters.
#[tokio::test]
async fn a_tag_renders_the_value_the_data_holds_and_the_parts_compose_one_message() {
    let cases = vec![
        RenderCase {
            case: "a text part only",
            subject: "Receipt for {{name}}",
            text: Some("Hello {{name}}, your order shipped."),
            html: None,
            data: r#"{"name":"Jane"}"#,
            expected_subject: "Receipt for Jane",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "Hello Jane, your order shipped.",
            )],
        },
        RenderCase {
            case: "an HTML part only",
            subject: "Receipt for {{name}}",
            text: None,
            html: Some("<p>Hello {{name}}</p>"),
            data: r#"{"name":"Jane"}"#,
            expected_subject: "Receipt for Jane",
            expected_parts: vec![Part::new(
                "text/html; charset=UTF-8",
                "7bit",
                "<p>Hello Jane</p>",
            )],
        },
        RenderCase {
            case: "both body parts",
            subject: "Order {{order.id}}",
            text: Some("Order {{order.id}} for {{name}}"),
            html: Some("<p>Order {{order.id}}</p>"),
            data: r#"{"name":"Jane","order":{"id":"A-1"}}"#,
            expected_subject: "Order A-1",
            expected_parts: vec![
                Part::new("text/plain; charset=UTF-8", "7bit", "Order A-1 for Jane"),
                Part::new("text/html; charset=UTF-8", "7bit", "<p>Order A-1</p>"),
            ],
        },
        RenderCase {
            case: "a number and a boolean value",
            subject: "{{count}} items",
            text: Some("count={{count}} express={{express}}"),
            html: None,
            data: r#"{"count":3,"express":true}"#,
            expected_subject: "3 items",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "count=3 express=true",
            )],
        },
        RenderCase {
            case: "a conditional whose test the data passes",
            subject: "Your plan",
            text: Some("{{#if premium}}gold{{else}}silver{{/if}}"),
            html: None,
            data: r#"{"premium":true}"#,
            expected_subject: "Your plan",
            expected_parts: vec![Part::new("text/plain; charset=UTF-8", "7bit", "gold")],
        },
        RenderCase {
            case: "a conditional whose test the data fails",
            subject: "Your plan",
            text: Some("{{#if premium}}gold{{else}}silver{{/if}}"),
            html: None,
            data: r#"{"premium":false}"#,
            expected_subject: "Your plan",
            expected_parts: vec![Part::new("text/plain; charset=UTF-8", "7bit", "silver")],
        },
        RenderCase {
            case: "a tag in the branch the data did not take",
            subject: "Your plan",
            text: Some("{{#if premium}}{{discount}} off{{else}}full price{{/if}}"),
            html: None,
            data: r#"{"premium":false}"#,
            expected_subject: "Your plan",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "full price",
            )],
        },
        RenderCase {
            case: "a conditional whose test the data does not carry",
            subject: "Your plan",
            text: Some("{{#if premium}}gold{{else}}silver{{/if}}"),
            html: None,
            data: "{}",
            expected_subject: "Your plan",
            expected_parts: vec![Part::new("text/plain; charset=UTF-8", "7bit", "silver")],
        },
        RenderCase {
            case: "a conditional chaining a further test",
            subject: "Your plan",
            text: Some("{{#if premium}}gold{{else if trial}}trial{{else}}silver{{/if}}"),
            html: None,
            data: r#"{"premium":false,"trial":true}"#,
            expected_subject: "Your plan",
            expected_parts: vec![Part::new("text/plain; charset=UTF-8", "7bit", "trial")],
        },
        RenderCase {
            case: "a repetition over a list",
            subject: "Your order",
            text: Some("{{#each items}}[{{this}}]{{/each}}"),
            html: None,
            data: r#"{"items":["mug","pen","hat"]}"#,
            expected_subject: "Your order",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "[mug][pen][hat]",
            )],
        },
        RenderCase {
            case: "a repetition over a list of objects",
            subject: "Your order",
            text: Some("{{#each items}}{{this.name}}={{this.price}};{{/each}}"),
            html: None,
            data: r#"{"items":[{"name":"mug","price":9},{"name":"pen","price":2}]}"#,
            expected_subject: "Your order",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "mug=9;pen=2;",
            )],
        },
        RenderCase {
            case: "no replacement tag at all",
            subject: "A fixed subject",
            text: Some("A fixed body."),
            html: None,
            data: "{}",
            expected_subject: "A fixed subject",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "A fixed body.",
            )],
        },
        RenderCase {
            case: "a value carrying markup characters",
            subject: "Hello {{name}}",
            text: Some("Hello {{name}}"),
            html: None,
            data: r#"{"name":"A & B <c> \"d\""}"#,
            expected_subject: "Hello A & B <c> \"d\"",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "7bit",
                "Hello A & B <c> \"d\"",
            )],
        },
        RenderCase {
            // The HTML body IS markup, so a value substituted into it is escaped, while the
            // same value in the subject line and the text body is carried as the data holds
            // it; the unescaped spelling `{{{name}}}` asks for it as written.
            case: "a value carrying markup characters in the HTML part",
            subject: "Hello {{name}}",
            text: Some("Hello {{name}}"),
            html: Some("<p>{{name}}</p><pre>{{{name}}}</pre>"),
            data: r#"{"name":"A & B <c>"}"#,
            expected_subject: "Hello A & B <c>",
            expected_parts: vec![
                Part::new("text/plain; charset=UTF-8", "7bit", "Hello A & B <c>"),
                Part::new(
                    "text/html; charset=UTF-8",
                    "7bit",
                    "<p>A &amp; B &lt;c&gt;</p><pre>A & B <c></pre>",
                ),
            ],
        },
        RenderCase {
            case: "a value that is not ASCII",
            subject: "{{subject}}",
            text: Some("{{greeting}}"),
            html: None,
            data: r#"{"subject":"領収書","greeting":"こんにちは"}"#,
            expected_subject: "領収書",
            expected_parts: vec![Part::new(
                "text/plain; charset=UTF-8",
                "8bit",
                "こんにちは",
            )],
        },
    ];

    let router = emulator();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("render-{index}");
        create_template(
            &router,
            case.case,
            &name,
            Some(case.subject),
            case.text,
            case.html,
        )
        .await;
        let message = rendered(&router, case.case, &name, case.data).await;
        assert_eq!(
            message,
            Message {
                subject: Some(case.expected_subject.to_string()),
                parts: case.expected_parts.clone(),
            },
            "case {}: expected the rendered message above, got {message:?}",
            case.case
        );
    }
}

/// RULE: a replacement tag the rendering reaches and the data holds no value for is refused as
/// the modelled `MissingRenderingAttribute`, naming that tag.
///
/// The rows differ along WHERE the unresolved tag sits — the subject line, a body part, a
/// nested path, the body of a block the data did take — since the rule is about the tag being
/// reached, not about which part carries it.
#[tokio::test]
async fn a_tag_the_data_holds_no_value_for_is_refused_as_a_missing_rendering_attribute() {
    // (case, subject, text, replacement data, the attribute the refusal names)
    let cases = [
        (
            "a tag in the text part",
            "A fixed subject",
            "Hello {{name}}",
            "{}",
            "name",
        ),
        (
            "a tag in the subject line",
            "Hello {{name}}",
            "A fixed body.",
            "{}",
            "name",
        ),
        (
            "one tag of several",
            "A fixed subject",
            "{{greeting}}, {{name}}",
            r#"{"greeting":"Hello"}"#,
            "name",
        ),
        (
            "a tag in the branch the data took",
            "A fixed subject",
            "{{#if premium}}{{discount}} off{{else}}full price{{/if}}",
            r#"{"premium":true}"#,
            "discount",
        ),
        (
            "a tag inside a repetition",
            "A fixed subject",
            "{{#each items}}{{this.name}}{{/each}}",
            r#"{"items":[{"price":9}]}"#,
            "this.name",
        ),
    ];

    let router = emulator();
    for (index, (case, subject, text, data, attribute)) in cases.iter().enumerate() {
        let name = format!("missing-{index}");
        create_template(&router, case, &name, Some(subject), Some(text), None).await;
        let (status, body) = render_raw(&router, &name, data).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("MissingRenderingAttribute"),
            "case {case}: expected the modelled code, got {code:?} with {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(
                format!("Attribute '{attribute}' is not present in the rendering data.")
                    .as_str()
            ),
            "case {case}: expected the refusal to name {attribute:?}, got {message:?}"
        );
        assert_eq!(
            element(&body, "TemplateName").as_deref(),
            Some(name.as_str()),
            "case {case}: expected the refusal to name the template, got {body}"
        );
    }
}

/// RULE: replacement data that is not a JSON object of replacement values — and a rendering
/// whose work is past the bounds this service composes a message within — is refused as the
/// modelled `InvalidRenderingParameter`.
///
/// The rows span the forms caller-supplied data takes: text that is not JSON at all, JSON that
/// is not an object, well-formed objects whose rendering is past the evaluation bound — both
/// the shape that MULTIPLIES (a repetition inside a repetition) and the shape that merely ADDS
/// UP (repetitions side by side, writing nothing at all, so the output ceiling never sees
/// them) — and the templates this service does not render at all: nesting past the bound as
/// plain blocks and as an `{{else if}}` chain, more tags than one part may write, more
/// parentheses than one part may carry, a close that closes nothing, a block left open, and a
/// comment never terminated.
#[tokio::test]
async fn data_that_is_no_object_of_replacement_values_is_refused_as_an_invalid_parameter() {
    // A list long enough that the elements a rendering over it would evaluate are past the
    // evaluation bound this service renders within (`state::template_rendering`).
    let wide: String = format!(
        r#"{{"items":[{}]}}"#,
        (0..3_000)
            .map(|index| format!("\"{index}\""))
            .collect::<Vec<String>>()
            .join(",")
    );
    // Repetitions that only ADD UP: four hundred blocks side by side, each with an empty body,
    // so the rendering writes no byte at all and nothing but the evaluation bound can refuse
    // it.
    let side_by_side: String = "{{#each items}}{{/each}}".repeat(400);
    // Templates nesting blocks deeper than this service renders one, in the two spellings the
    // engine's own tree nests: the plain block, and the `{{else if}}` chain it nests inside a
    // block's inverse.
    let deeply_nested: String = format!(
        "{}{}",
        "{{#if flag}}".repeat(128),
        "{{/if}}".repeat(128)
    );
    let deeply_chained: String = format!(
        "{{{{#if flag}}}}a{}{{{{/if}}}}",
        "{{else if flag}}b".repeat(96)
    );
    // More tags than one part may write, and more parentheses than one part may carry: both
    // are counted over the text before the engine parses it, since a parse is what they bound.
    let too_many_tags: String = "{{flag}}".repeat(2_000);
    let too_many_parentheses: String = "(".repeat(300);
    // Openers whose closes are not closes at all: one written inside a comment and one inside
    // a quoted parameter, neither of which closes the block the engine has open.
    let commented_closes: String = "{{#if flag}}{{!-- {{/if}} --}}".repeat(32);
    let quoted_closes: String = "{{#if flag}}{{log \"{{/if}}\"}}".repeat(32);
    let unterminated_comment: String = format!(
        "{{{{!-- never closed {}{}",
        "{{#if flag}}".repeat(8),
        "{{/if}}".repeat(8)
    );
    // (case, the template's text part, the replacement data, the sentence the refusal states)
    let unrenderable = "Template rendering data is invalid";
    let too_large = "The replacement values ask for a message larger than this service \
                     sends.";
    let unrenderable_template =
        "The template could not be rendered with the replacement values provided.";
    let cases: Vec<(&str, &str, String, &str)> = vec![
        ("text that is not JSON", "Hello", "not json".to_string(), unrenderable),
        ("no data at all", "Hello", String::new(), unrenderable),
        ("a truncated object", "Hello", r#"{"name":"#.to_string(), unrenderable),
        ("a JSON array", "Hello", "[1,2]".to_string(), unrenderable),
        ("a JSON string", "Hello", "\"Jane\"".to_string(), unrenderable),
        ("a JSON number", "Hello", "42".to_string(), unrenderable),
        ("a JSON null", "Hello", "null".to_string(), unrenderable),
        (
            "repetitions that multiply past the bound",
            "{{#each items}}{{#each ../items}}.{{/each}}{{/each}}",
            wide.clone(),
            too_large,
        ),
        (
            "repetitions that add up past the bound while writing nothing",
            side_by_side.as_str(),
            wide,
            too_large,
        ),
        (
            "blocks nested deeper than this service renders",
            deeply_nested.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "a chain of `else if` deeper than this service renders",
            deeply_chained.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "more tags than one part may write",
            too_many_tags.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "openers whose closes sit inside a comment",
            commented_closes.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "openers whose closes sit inside a quoted parameter",
            quoted_closes.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "a close that closes nothing",
            "{{/if}}",
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "a block left open",
            "{{#if flag}}never closed",
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "more parentheses than one part may carry",
            too_many_parentheses.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
        (
            "a comment the template never terminates",
            unterminated_comment.as_str(),
            r#"{"flag":true}"#.to_string(),
            unrenderable_template,
        ),
    ];

    let router = emulator();
    for (index, (case, text, data, expected)) in cases.iter().enumerate() {
        let name = format!("invalid-{index}");
        create_template(&router, case, &name, Some("A fixed subject"), Some(text), None).await;
        let (status, body) = render_raw(&router, &name, data).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidRenderingParameter"),
            "case {case}: expected the modelled code, got {code:?} with {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(*expected),
            "case {case}: expected the refusal above, got {message:?}"
        );
    }
}

/// RULE: the evaluation bound admits the renderings a caller really asks for — a repetition
/// over a list renders every element of it — and refuses only the work past the bound (the
/// rows of the suite above).
#[tokio::test]
async fn a_repetition_within_the_step_bound_renders_every_element() {
    let router = emulator();
    create_template(
        &router,
        "a wide repetition",
        "wide",
        Some("Your order"),
        Some("{{#each items}}[{{this}}]{{/each}}"),
        None,
    )
    .await;
    // A list of a thousand elements: well past anything a recorded request carries, and well
    // within the bound, so the whole repetition is rendered.
    let count = 1_000;
    let data = format!(
        r#"{{"items":[{}]}}"#,
        (0..count)
            .map(|index| format!("\"{index}\""))
            .collect::<Vec<String>>()
            .join(",")
    );
    let message = rendered(&router, "a wide repetition", "wide", &data).await;
    let body = message
        .parts
        .first()
        .map(|part| part.body.clone())
        .unwrap_or_default();
    for index in [0, count / 2, count - 1] {
        assert!(
            body.contains(&format!("[{index}]")),
            "case a wide repetition: expected element {index} rendered, got {} bytes",
            body.len()
        );
    }
    assert_eq!(
        body.matches('[').count(),
        count,
        "case a wide repetition: expected {count} elements rendered, got {}",
        body.matches('[').count()
    );
}

/// RULE: a name the account holds no template under is refused as the modelled
/// `TemplateDoesNotExist`, in the words every operation of this service that resolves a
/// template by name reports its absence with.
#[tokio::test]
async fn a_name_the_account_holds_no_template_under_is_refused() {
    let router = emulator();
    create_template(
        &router,
        "a held template",
        "held",
        Some("A fixed subject"),
        Some("A fixed body."),
        None,
    )
    .await;
    // Names spanning what a caller can send: another template's name is not one of them, so
    // each of these names nothing — including the name of a template that was deleted.
    for case in ["absent", "Held", "held ", "領収書"] {
        let (status, body) = render_raw(&router, case, "{}").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("TemplateDoesNotExist"),
            "case {case:?}: expected the modelled code, got {code:?} with {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Unable to find <{case}> for <{}>", account()).as_str()),
            "case {case:?}: expected the refusal to name the template, got {message:?}"
        );
    }
}

/// RULE: the preview REPORTS — it renders the template the account holds and changes nothing,
/// so a second preview answers what the first did and the reader that follows reports the
/// template exactly as it was recorded, tags and all.
#[tokio::test]
async fn a_preview_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let subject = "Order {{order.id}}";
    let text = "Hello {{name}}";
    let html = "<p>Hello {{name}}</p>";
    create_template(
        &router,
        "round trip",
        "receipts",
        Some(subject),
        Some(text),
        Some(html),
    )
    .await;

    let data = r#"{"name":"Jane","order":{"id":"A-1"}}"#;
    let first = rendered(&router, "first preview", "receipts", data).await;
    let again = rendered(&router, "second preview", "receipts", data).await;
    assert_eq!(
        first, again,
        "case repeated preview: expected the same message, got {first:?} then {again:?}"
    );

    // The stored template is the one that was recorded: a preview rendered it, it did not
    // rewrite it.
    let (status, body) = call(&router, "GetTemplate", &[field("TemplateName", "receipts")]).await;
    assert_eq!(
        status, 200,
        "case stored template: expected 200 from GetTemplate, got {status} with {body}"
    );
    for (member, expected) in [
        ("SubjectPart", subject),
        ("TextPart", text),
        ("HtmlPart", html),
    ] {
        assert_eq!(
            element(&body, member).map(|text| xml_text(&text)).as_deref(),
            Some(expected),
            "case stored {member}: expected the recorded part, got {body}"
        );
    }

    // And the preview a send-shaped set of replacement values renders is still the message
    // those parts compose, which is what the first assertion above read.
    assert_eq!(
        first.subject.as_deref(),
        Some("Order A-1"),
        "case rendered subject: expected the rendered subject line, got {:?}",
        first.subject
    );
}
