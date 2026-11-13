// SPDX-License-Identifier: CC0-1.0

//! `CreateTemplate`, and the reader that proves what it recorded, end to end over
//! the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. What is asserted is the RULE the operation
//! follows over a SET of inputs rather than one recorded request:
//!
//! * a create records the template under the name the request names, and every
//!   later reader reports the parts that were sent, byte for byte, whatever those
//!   parts are — text, HTML, placeholder markup, non-ASCII text, XML
//!   metacharacters — while a part the request did not send stays ABSENT;
//! * a name the account already holds is a naming conflict: the create is refused
//!   with the modelled `AlreadyExists` and the stored template is left alone;
//! * a template that names no subject line is refused with HTTP 400
//!   `InvalidParameterValue` and records nothing;
//! * reading a name the account holds no template under is refused with the
//!   modelled `TemplateDoesNotExist`;
//! * a create past the account's template ceiling is refused with the modelled
//!   `LimitExceeded`;
//! * the creation instant the listing reports is the instant the create was
//!   served.

use crate::harness::{call, element, elements, emulator, emulator_over, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    recorded_template, service_state, MAX_TEMPLATES,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One template as a request carries it. A `None` part is a member the request
/// does not send at all, which is a different request from one sending an empty
/// string.
#[derive(Debug, Clone)]
struct TemplateRow {
    case: &'static str,
    name: String,
    subject: Option<String>,
    text: Option<String>,
    html: Option<String>,
}

/// The parts a reader reports for one template: the name, and each part it
/// carries (`None` when the member is absent from the response).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTemplate {
    name: Option<String>,
    subject: Option<String>,
    text: Option<String>,
    html: Option<String>,
}

impl TemplateRow {
    /// A row naming every part, so a case only has to say what makes it
    /// different.
    fn new(case: &'static str, name: &str, subject: &str, text: &str, html: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            subject: Some(subject.to_string()),
            text: Some(text.to_string()),
            html: Some(html.to_string()),
        }
    }

    /// What a reader must report for this row once it is stored.
    fn as_reported(&self) -> ReportedTemplate {
        ReportedTemplate {
            name: Some(self.name.clone()),
            subject: self.subject.clone(),
            text: self.text.clone(),
            html: self.html.clone(),
        }
    }
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope, with
/// the entities the XML codec escapes read back.
fn error_envelope(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|text| xml_text(&text)),
    )
}

/// One `CreateTemplate` call: `(status, body)`. Only the parts the row names are
/// sent.
async fn create_template_raw(router: &MultiServiceRouter, row: &TemplateRow) -> (u16, String) {
    let mut params: Vec<Field> = vec![field("Template.TemplateName", row.name.clone())];
    for (member, value) in [
        ("Template.SubjectPart", &row.subject),
        ("Template.TextPart", &row.text),
        ("Template.HtmlPart", &row.html),
    ] {
        if let Some(value) = value {
            params.push(field(member, value.clone()));
        }
    }
    call(router, "CreateTemplate", &params).await
}

/// `CreateTemplate`, asserting it was accepted.
async fn create_template(router: &MultiServiceRouter, row: &TemplateRow) {
    let (status, body) = create_template_raw(router, row).await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from CreateTemplate, got {status} with {body}",
        row.case
    );
    assert!(
        body.contains("<CreateTemplateResponse"),
        "case {}: expected this operation's awsQuery envelope, got {body}",
        row.case
    );
}

/// One `GetTemplate` call: `(status, body)`.
async fn get_template_raw(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "GetTemplate", &[field("TemplateName", name)]).await
}

/// `GetTemplate`, as the parts it reports. Asserts the call succeeded and carried
/// a `Template`.
async fn get_template(router: &MultiServiceRouter, case: &str, name: &str) -> ReportedTemplate {
    let (status, body) = get_template_raw(router, name).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetTemplate({name:?}), got {status} with {body}"
    );
    let template = element(&body, "Template").unwrap_or_else(|| {
        panic!("case {case}: GetTemplate({name:?}) must report a Template, got {body}")
    });
    let part = |tag: &str| element(&template, tag).map(|text| xml_text(&text));
    ReportedTemplate {
        name: part("TemplateName"),
        subject: part("SubjectPart"),
        text: part("TextPart"),
        html: part("HtmlPart"),
    }
}

/// Every `(name, creation second)` the listing reports, over as many pages as it
/// takes.
async fn listed(router: &MultiServiceRouter) -> Vec<(String, i64)> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for _ in 0..1_000 {
        let mut params: Vec<Field> = vec![field("MaxItems", "100")];
        if let Some(token) = &token {
            params.push(field("NextToken", token.clone()));
        }
        let (status, body) = call(router, "ListTemplates", &params).await;
        assert_eq!(
            status, 200,
            "ListTemplates must answer 200, got {status} with {body}"
        );
        let page = element(&body, "TemplatesMetadata")
            .map(|list| elements(&list, "member"))
            .unwrap_or_default();
        for entry in &page {
            let name = element(entry, "Name")
                .unwrap_or_else(|| panic!("a listed entry carries Name, got {entry}"));
            let stamp = element(entry, "CreatedTimestamp")
                .unwrap_or_else(|| panic!("a listed entry carries CreatedTimestamp, got {entry}"));
            let parsed = ::aws_smithy_types::DateTime::from_str(
                &stamp,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .unwrap_or_else(|error| {
                panic!("case {name:?}: CreatedTimestamp {stamp:?} must be a date-time: {error}")
            });
            found.push((name, parsed.secs()));
        }
        match element(&body, "NextToken") {
            None => return found,
            Some(next) => token = Some(next),
        }
    }
    panic!("ListTemplates never ran out of pages");
}

/// Just the names the listing reports.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    listed(router)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The content classes a template part can carry: each row differs in the kind of
/// text the parts hold, or in which parts are sent at all.
fn content_rows() -> Vec<TemplateRow> {
    let long_body = "x".repeat(4_096);
    vec![
        TemplateRow::new(
            "every part, plain text",
            "plain",
            "Your receipt",
            "Thanks for your order.",
            "<h1>Thanks for your order.</h1>",
        ),
        // "either an HTML part or a text-only part" (`Template`): each of the two
        // bodies on its own, and a template carrying neither.
        TemplateRow {
            case: "the text part only",
            name: "text-only".to_string(),
            subject: Some("Text only".to_string()),
            text: Some("No HTML here.".to_string()),
            html: None,
        },
        TemplateRow {
            case: "the HTML part only",
            name: "html_only".to_string(),
            subject: Some("HTML only".to_string()),
            text: None,
            html: Some("<p>No text part.</p>".to_string()),
        },
        TemplateRow {
            case: "the subject line alone",
            name: "SubjectOnly9".to_string(),
            subject: Some("Subject only".to_string()),
            text: None,
            html: None,
        },
        TemplateRow::new(
            "placeholder markup, which a send renders and a create stores as sent",
            "with-placeholders",
            "Hello {{name}}",
            "{{#if member}}Welcome back, {{name}}.{{else}}Welcome, {{name}}.{{/if}}",
            "<p>{{greeting}}</p>",
        ),
        TemplateRow::new(
            "non-ASCII text in every part",
            "unicode-parts",
            "Grüße, 你好 — приве́т 🎉",
            "Übergrößenträger… 日本語のテキスト 🚀",
            "<p>Ünïcödé 😀 — 中文</p>",
        ),
        TemplateRow::new(
            "XML metacharacters, which travel escaped and must come back unchanged",
            "metacharacters",
            "5 < 6 & \"quoted\" 'single'",
            "a > b && c < d",
            "<p class=\"a&b\">x &amp; y</p>",
        ),
        TemplateRow::new(
            "whitespace-only bodies, which are content and not absence",
            "whitespace-bodies",
            " ",
            "\n\t ",
            " ",
        ),
        TemplateRow::new(
            "bodies of a few kilobytes",
            "long-bodies",
            "A long one",
            &long_body,
            &format!("<p>{long_body}</p>"),
        ),
        // The name itself is caller input: the classes a template name can be
        // drawn from, including one that differs from another row only by case.
        TemplateRow::new(
            "a name of digits",
            "0123456789",
            "Digits",
            "Digits.",
            "<p>Digits.</p>",
        ),
        TemplateRow::new(
            "a name with dashes and underscores",
            "a-name_with-both",
            "Mixed punctuation",
            "Mixed.",
            "<p>Mixed.</p>",
        ),
        TemplateRow::new(
            "a name differing from another only by case",
            "PLAIN",
            "Upper case name",
            "Upper.",
            "<p>Upper.</p>",
        ),
        TemplateRow::new(
            "a 64-character name",
            &"n".repeat(64),
            "A long name",
            "Long name.",
            "<p>Long name.</p>",
        ),
    ]
}

#[tokio::test]
async fn a_created_template_is_reported_part_for_part_by_the_reader() {
    let router = emulator();
    let rows = content_rows();
    for row in &rows {
        create_template(&router, row).await;
        let reported = get_template(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: the reader must report the parts the create sent, and only those",
            row.case
        );
    }

    // Every created template is in the account's listing, exactly once, and the
    // listing holds nothing else.
    let mut expected: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    expected.sort();
    let names = listed_names(&router).await;
    assert_eq!(
        names, expected,
        "case the whole table: expected every created template once, in name order, got {names:?}"
    );

    // Re-reading changes nothing a later caller sees: the same parts come back,
    // and the listing is unchanged.
    for row in &rows {
        let reread = get_template(&router, row.case, &row.name).await;
        assert_eq!(
            reread,
            row.as_reported(),
            "case {}: a second read must report the same parts",
            row.case
        );
    }
    let after_rereads = listed_names(&router).await;
    assert_eq!(
        after_rereads, expected,
        "case after re-reading every template: the store must be untouched, got {after_rereads:?}"
    );
}

#[tokio::test]
async fn a_name_the_account_already_holds_is_refused_and_leaves_the_stored_template_alone() {
    let router = emulator();
    let stored = TemplateRow::new(
        "the first create",
        "already-there",
        "The stored subject",
        "The stored text.",
        "<p>The stored text.</p>",
    );
    create_template(&router, &stored).await;

    // Each row is a second create of the SAME name, differing in what it would
    // have written: the refusal cannot depend on the content.
    let rows: Vec<TemplateRow> = vec![
        TemplateRow {
            case: "the same content again",
            ..stored.clone()
        },
        TemplateRow {
            case: "different content in every part",
            name: stored.name.clone(),
            subject: Some("Another subject".to_string()),
            text: Some("Another text.".to_string()),
            html: Some("<p>Another text.</p>".to_string()),
        },
        TemplateRow {
            case: "a different subject only",
            name: stored.name.clone(),
            subject: Some("Only the subject differs".to_string()),
            text: stored.text.clone(),
            html: stored.html.clone(),
        },
        TemplateRow {
            case: "fewer parts than the stored template",
            name: stored.name.clone(),
            subject: Some("Subject only".to_string()),
            text: None,
            html: None,
        },
    ];
    for row in &rows {
        let (status, body) = create_template_raw(&router, row).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 for a name already in use, got {status} with {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("AlreadyExists"),
            "case {}: expected the modelled AlreadyExists, got {body}",
            row.case
        );
        assert_eq!(
            message.as_deref(),
            Some("Duplicate Template Name."),
            "case {}: expected the duplicate-name message, got {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Name").as_deref(),
            Some(row.name.as_str()),
            "case {}: the refusal must name the template, got {body}",
            row.case
        );

        // The refusal wrote nothing: the stored template still reports the parts
        // the FIRST create sent.
        let reported = get_template(&router, row.case, &stored.name).await;
        assert_eq!(
            reported,
            stored.as_reported(),
            "case {}: a refused create must leave the stored template alone",
            row.case
        );
    }
    let names = listed_names(&router).await;
    assert_eq!(
        names,
        vec![stored.name.clone()],
        "case after {} refusals: the account must hold only the first template, got {names:?}",
        rows.len()
    );
}

#[tokio::test]
async fn a_template_that_names_no_subject_line_is_refused_and_records_nothing() {
    let router = emulator();
    // Each row omits the subject line, or sends it empty, while differing in the
    // bodies it carries: the refusal is about the subject and nothing else.
    let rows: Vec<TemplateRow> = vec![
        TemplateRow {
            case: "no subject member at all",
            name: "absent-subject".to_string(),
            subject: None,
            text: Some("Body with no subject.".to_string()),
            html: Some("<p>Body with no subject.</p>".to_string()),
        },
        TemplateRow {
            case: "an empty subject",
            name: "empty-subject".to_string(),
            subject: Some(String::new()),
            text: Some("Body with an empty subject.".to_string()),
            html: None,
        },
        TemplateRow {
            case: "an empty subject and no bodies",
            name: "empty-subject-no-bodies".to_string(),
            subject: Some(String::new()),
            text: None,
            html: None,
        },
        TemplateRow {
            case: "no subject and no bodies",
            name: "nothing-but-a-name".to_string(),
            subject: None,
            text: None,
            html: None,
        },
        TemplateRow {
            case: "no subject, with non-ASCII bodies",
            name: "unicode-without-subject".to_string(),
            subject: None,
            text: Some("Körper ohne Betreff 🚫".to_string()),
            html: Some("<p>日本語の本文</p>".to_string()),
        },
    ];
    for row in &rows {
        let (status, body) = create_template_raw(&router, row).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 for a template with no subject line, got {status} \
             with {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {}: expected InvalidParameterValue, got {body}",
            row.case
        );
        assert_eq!(
            message.as_deref(),
            Some("The subject must be specified."),
            "case {}: expected the missing-subject message, got {body}",
            row.case
        );

        // Nothing was recorded: the reader refuses the name the create carried.
        let (status, body) = get_template_raw(&router, &row.name).await;
        assert_eq!(
            status, 400,
            "case {}: a refused create must record nothing, got {status} with {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("TemplateDoesNotExist"),
            "case {}: expected TemplateDoesNotExist for a name nothing was stored under, \
             got {body}",
            row.case
        );
    }
    let names = listed_names(&router).await;
    assert!(
        names.is_empty(),
        "case after {} refusals: the account must hold no template, got {names:?}",
        rows.len()
    );

    // The same name succeeds as soon as the request carries a subject line, so
    // the refusals above left no half-written record behind.
    let repaired = TemplateRow::new(
        "the same name, with a subject line",
        "absent-subject",
        "Now it has a subject",
        "Body with no subject.",
        "<p>Body with no subject.</p>",
    );
    create_template(&router, &repaired).await;
    let reported = get_template(&router, repaired.case, &repaired.name).await;
    assert_eq!(
        reported,
        repaired.as_reported(),
        "case {}: the create must be recorded once the subject line is supplied",
        repaired.case
    );
}

#[tokio::test]
async fn reading_a_name_the_account_holds_no_template_under_is_refused() {
    let router = emulator();
    let stored = TemplateRow::new(
        "the stored template",
        "stored-one",
        "Stored",
        "Stored.",
        "<p>Stored.</p>",
    );
    create_template(&router, &stored).await;

    // Each row is a name the account holds no template under: never created, or
    // differing from the stored one by case, by whitespace, or by a suffix.
    let rows: &[(&str, &str)] = &[
        ("a name nothing was ever created under", "no-such-template"),
        ("the stored name in upper case", "STORED-ONE"),
        ("the stored name with a suffix", "stored-one-2"),
        ("the stored name with trailing space", "stored-one "),
        ("a name carrying XML metacharacters", "<Message>&\"'"),
        ("a non-ASCII name", "nie-istnieje-🚫"),
    ];
    for (case, name) in rows {
        let (status, body) = get_template_raw(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case} ({name:?}): expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("TemplateDoesNotExist"),
            "case {case} ({name:?}): expected the modelled TemplateDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Template {name} does not exist.").as_str()),
            "case {case} ({name:?}): the message must name the template, got {body}"
        );
        assert_eq!(
            element(&body, "TemplateName")
                .map(|text| xml_text(&text))
                .as_deref(),
            Some(*name),
            "case {case} ({name:?}): the refusal must carry the name that was asked for, \
             got {body}"
        );
        // A rejected value is quoted back into the error envelope, so one carrying
        // XML metacharacters must travel escaped rather than breaking the envelope
        // it is rendered into.
        assert!(
            !body.contains(*name) || !name.contains(['<', '>', '&']),
            "case {case} ({name:?}): an XML metacharacter must be escaped in the envelope, \
             got {body}"
        );
    }

    // A refused read leaves the one stored template readable and listed.
    let reported = get_template(&router, "after the refusals", &stored.name).await;
    assert_eq!(
        reported,
        stored.as_reported(),
        "case after {} refused reads: the stored template must be unchanged",
        rows.len()
    );
    let names = listed_names(&router).await;
    assert_eq!(
        names,
        vec![stored.name.clone()],
        "case after {} refused reads: the listing must be unchanged, got {names:?}",
        rows.len()
    );
}

#[tokio::test]
async fn a_create_past_the_accounts_template_ceiling_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    // Filled to exactly the ceiling through the same state API the handlers use:
    // the ceiling is 10,000 templates, and making every one of them over the wire
    // would say nothing the store does not already decide.
    for index in 0..MAX_TEMPLATES {
        let recorded = store
            .lock()
            .put_template(recorded_template(
                &format!("seeded-{index:05}"),
                Some(format!("Subject {index}")),
                Some(format!("Text {index}")),
                None,
                UNIX_EPOCH + Duration::from_secs(1_767_225_600 + index as u64),
            ))
            .is_some();
        assert!(
            recorded,
            "case seeded template {index}: a template inside the account's limit must be recorded"
        );
    }
    let router = emulator_over(engine);

    // A create of a NEW name is refused at the ceiling, whatever it carries.
    let rows = [
        TemplateRow::new(
            "one past the ceiling",
            "one-too-many",
            "Over the limit",
            "Over.",
            "<p>Over.</p>",
        ),
        TemplateRow {
            case: "one past the ceiling, subject only",
            name: "one-too-many-2".to_string(),
            subject: Some("Over the limit".to_string()),
            text: None,
            html: None,
        },
    ];
    for row in &rows {
        let (status, body) = create_template_raw(&router, row).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 at the account's template ceiling, got {status} \
             with {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("LimitExceeded"),
            "case {}: expected the modelled LimitExceeded, got {body}",
            row.case
        );
        assert_eq!(
            message.as_deref(),
            Some(
                format!(
                    "Template limit exceeded: this account may hold {MAX_TEMPLATES} email \
                     templates."
                )
                .as_str()
            ),
            "case {}: the message must name the account's limit, got {body}",
            row.case
        );

        // The refused create recorded nothing, so the store is still at — not
        // past — the ceiling.
        let (status, body) = get_template_raw(&router, &row.name).await;
        assert_eq!(
            status, 400,
            "case {}: a refused create must record nothing, got {status} with {body}",
            row.case
        );
        assert_eq!(
            store.lock().templates.len(),
            MAX_TEMPLATES,
            "case {}: the store must hold exactly the ceiling",
            row.case
        );
    }

    // A template already at the ceiling is still readable, so the refusal is
    // about growing the store and nothing else.
    let reported = get_template(&router, "a seeded template", "seeded-00000").await;
    assert_eq!(
        reported,
        ReportedTemplate {
            name: Some("seeded-00000".to_string()),
            subject: Some("Subject 0".to_string()),
            text: Some("Text 0".to_string()),
            html: None,
        },
        "case a seeded template: the reader must report what the store holds"
    );

    // Both refusals at once: a create of a name already held, at the ceiling.
    // The naming conflict is answered first, because a create that could only
    // ever replace an existing template adds nothing to the store and so is
    // never the one that would grow it past the limit.
    let duplicate_at_the_ceiling = TemplateRow::new(
        "a name already held, at the ceiling",
        "seeded-00000",
        "Over the limit and already there",
        "Over.",
        "<p>Over.</p>",
    );
    let (status, body) = create_template_raw(&router, &duplicate_at_the_ceiling).await;
    assert_eq!(
        status, 400,
        "case {}: expected HTTP 400, got {status} with {body}",
        duplicate_at_the_ceiling.case
    );
    let (code, message) = error_envelope(&body);
    assert_eq!(
        code.as_deref(),
        Some("AlreadyExists"),
        "case {}: the naming conflict is answered before the account's limit, got {body}",
        duplicate_at_the_ceiling.case
    );
    assert_eq!(
        message.as_deref(),
        Some("Duplicate Template Name."),
        "case {}: expected the duplicate-name message, got {body}",
        duplicate_at_the_ceiling.case
    );
    assert_eq!(
        reported,
        get_template(&router, duplicate_at_the_ceiling.case, "seeded-00000").await,
        "case {}: the refused create must leave the stored template alone",
        duplicate_at_the_ceiling.case
    );
}

#[tokio::test]
async fn the_creation_instant_the_listing_reports_is_when_the_create_was_served() {
    let router = emulator();
    let before = SystemTime::now();
    // Created in name order, so the order the listing reports is also the order
    // they were created in and the instants must not go backwards.
    let rows: Vec<TemplateRow> = ["stamp-a", "stamp-b", "stamp-c"]
        .iter()
        .map(|name| TemplateRow::new("creation instant", name, "Subject", "Text.", "<p>Text.</p>"))
        .collect();
    for row in &rows {
        create_template(&router, row).await;
    }
    let after = SystemTime::now();

    let floor = before
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs() as i64;
    let ceiling = after
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs() as i64;
    let listing = listed(&router).await;
    assert_eq!(
        listing.len(),
        rows.len(),
        "case the whole table: expected one entry per created template, got {listing:?}"
    );
    let mut previous = floor;
    for (name, created) in &listing {
        assert!(
            (floor..=ceiling).contains(created),
            "case {name:?}: expected a creation instant inside [{floor}, {ceiling}], got {created}"
        );
        assert!(
            *created >= previous,
            "case {name:?}: creation instants must not go backwards, got {created} after \
             {previous}"
        );
        previous = *created;
    }
}
