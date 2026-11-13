// SPDX-License-Identifier: CC0-1.0

//! ONE set of email templates behind the account's email APIs, paged by ONE cursor.
//!
//! The rule under test: **an email template belongs to the ACCOUNT, not to the API that
//! recorded it, and the two APIs page ONE list of those templates.** Amazon SES publishes
//! two APIs over one template collection — the classic `awsQuery` surface
//! (`CreateTemplate`, `GetTemplate`, `UpdateTemplate`, `ListTemplates`, `DeleteTemplate`)
//! and the v2 REST surface (`CreateEmailTemplate`, `GetEmailTemplate`,
//! `UpdateEmailTemplate`, `ListEmailTemplates`, `DeleteEmailTemplate`) — so a template
//! recorded through either one is read and listed by BOTH, a second create of its name on
//! the other surface is the naming conflict it is, content replaced through one surface is
//! reported by the other in that API's own spelling of it, a delete served on either
//! surface removes the template from both, and a continuation token either surface's
//! template listing issued names a position in that one list and continues it on either
//! surface.
//!
//! The rows differ along what the rule generalises over: which surface RECORDED the
//! template, which surface REPLACED its content and which parts that content carries, which
//! surface READ it back, which surface DELETED it, the page size a walk asks for, and which
//! surface each page of that walk is asked of. Every expected value is computed from what
//! the row declares.
//!
//! This suite needs the two services over ONE engine, with the other members of the `ses`
//! signing-name family mounted beside them — the same assembly `core/router/src/lib.rs`
//! performs — because the hand-off being asserted exists only when one engine is shared,
//! and the claim that resolves each request to one service exists only when the peers are
//! there to compete for it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, elements, form_encode, members_of, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which API a call is made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API.
    V1,
    /// The v2 REST API.
    V2,
}

impl Surface {
    /// The other one, which is the surface a row's hand-off is asserted through.
    fn other(self) -> Self {
        match self {
            Self::V1 => Self::V2,
            Self::V2 => Self::V1,
        }
    }

    /// Both of them, for an assertion that must hold on either API.
    fn both() -> [Self; 2] {
        [Self::V1, Self::V2]
    }
}

/// The content a row writes: the subject line, the text part and the HTML part, as the two
/// models both carry them (`Template$SubjectPart`/`TextPart`/`HtmlPart` on the classic
/// surface, `EmailTemplateContent$Subject`/`Text`/`Html` on the v2 one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Content {
    subject: &'static str,
    text: Option<&'static str>,
    html: Option<&'static str>,
}

/// One row of the shared-template table.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The template's name, which both APIs address it by.
    template: &'static str,
    /// The surface the create is made on.
    created_on: Surface,
    /// The content the create carries.
    created_with: Content,
    /// The surface the content-replacing update is made on, and the content it writes.
    replaced: Option<(Surface, Content)>,
    /// The surface the delete is served on.
    deleted_on: Surface,
}

/// The rows: every combination of recording surface and deleting surface, each with a
/// content replacement on one surface or none, and content spanning the parts a template
/// carries.
fn rows() -> Vec<Row> {
    let subject_only = Content {
        subject: "Your receipt",
        text: None,
        html: None,
    };
    let with_text = Content {
        subject: "Your receipt",
        text: Some("Thanks for your order."),
        html: None,
    };
    let with_html = Content {
        subject: "Your invoice",
        text: None,
        html: Some("<p>Thanks</p>"),
    };
    let every_part = Content {
        subject: "Your receipt {{name}}",
        text: Some("Thanks, {{name}}."),
        html: Some("<h1>Thanks</h1><p>{{name}}</p>"),
    };

    vec![
        Row {
            case: "recorded with the classic API, never replaced",
            template: "v1-plain",
            created_on: Surface::V1,
            created_with: with_text,
            replaced: None,
            deleted_on: Surface::V2,
        },
        Row {
            case: "recorded with the v2 API, never replaced",
            template: "v2-plain",
            created_on: Surface::V2,
            created_with: with_text,
            replaced: None,
            deleted_on: Surface::V1,
        },
        Row {
            case: "recorded with the classic API, replaced with the v2 API",
            template: "v1-replaced-on-v2",
            created_on: Surface::V1,
            created_with: subject_only,
            replaced: Some((Surface::V2, every_part)),
            deleted_on: Surface::V1,
        },
        Row {
            case: "recorded with the v2 API, replaced with the classic API",
            template: "v2-replaced-on-v1",
            created_on: Surface::V2,
            created_with: every_part,
            replaced: Some((Surface::V1, with_html)),
            deleted_on: Surface::V2,
        },
        Row {
            case: "a replacement that CLEARS the part the create carried",
            template: "v2-cleared-on-v1",
            created_on: Surface::V2,
            created_with: every_part,
            replaced: Some((Surface::V1, subject_only)),
            deleted_on: Surface::V1,
        },
        Row {
            case: "replaced on the surface that recorded it",
            template: "v1-replaced-on-v1",
            created_on: Surface::V1,
            created_with: subject_only,
            replaced: Some((Surface::V1, with_html)),
            deleted_on: Surface::V2,
        },
    ]
}

/// The services sharing the SigV4 signing name `ses`, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These rows exercise routing and handler behaviour, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `ses` — the signal the
/// router resolves a routing key from. The signature itself is never checked.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ses/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answered request: its status and its body, as text and as JSON.
struct Answer {
    status: u16,
    text: String,
    json: Value,
    /// The `x-amzn-errortype` header a restJson1 refusal names its shape in.
    error_type: String,
}

impl Answer {
    /// The modelled error code the answer names, whichever protocol carried it: the
    /// classic surface's `<Code>` element, or the v2 surface's error-type header or
    /// `__type` member.
    fn error_code(&self) -> String {
        if let Some(code) = opt_element(&self.text, "Code") {
            return code;
        }
        let raw = if self.error_type.is_empty() {
            self.json
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        } else {
            self.error_type.as_str()
        };
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }
}

/// Send one request, refusing at the door the two answers the ROUTER itself produces: a
/// request that reached nobody, or one several services claimed.
async fn send(
    router: &MultiServiceRouter,
    case: &str,
    leg: &str,
    request: http::Request<Bytes>,
) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(response.body()).to_string();
    for refusal in ROUTER_REFUSALS {
        assert!(
            !text.contains(refusal) && !error_type.contains(refusal),
            "case {case:?}: {leg} must be served by exactly one service, got HTTP {status}: \
             {text}"
        );
    }
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer {
        status,
        text,
        json,
        error_type,
    }
}

/// One classic-API call: `awsQuery` under the `ses` credential scope, `POST /` with the
/// operation in the `Action` form field.
async fn v1(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={SES_V1_API_VERSION}");
    for (name, value) in fields {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://email.{REGION}.amazonaws.com/"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    send(router, case, &format!("SES v1 {action}"), request).await
}

/// One v2-API call: `restJson1` under the `ses` credential scope, in the `/v2/email/…`
/// path space.
async fn v2(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    send(router, case, &format!("SESv2 {method} {path}"), request).await
}

/// Assert a call succeeded, quoting the whole answer when it did not.
fn succeeded(case: &str, leg: &str, answered: &Answer) {
    assert_eq!(
        answered.status, 200,
        "case {case:?}: {leg} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
}

/// The form fields the classic API's `Template` shape occupies for `content`.
fn v1_template_fields(name: &str, content: Content) -> Vec<(&'static str, String)> {
    let mut fields: Vec<(&'static str, String)> = vec![
        ("Template.TemplateName", name.to_string()),
        ("Template.SubjectPart", content.subject.to_string()),
    ];
    if let Some(text) = content.text {
        fields.push(("Template.TextPart", text.to_string()));
    }
    if let Some(html) = content.html {
        fields.push(("Template.HtmlPart", html.to_string()));
    }
    fields
}

/// The v2 API's `EmailTemplateContent` for `content`.
fn v2_content(content: Content) -> Value {
    let mut members = serde_json::Map::new();
    members.insert("Subject".to_string(), json!(content.subject));
    if let Some(text) = content.text {
        members.insert("Text".to_string(), json!(text));
    }
    if let Some(html) = content.html {
        members.insert("Html".to_string(), json!(html));
    }
    Value::Object(members)
}

/// Record `content` under `name`, on `surface`.
async fn create(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    name: &str,
    content: Content,
) -> Answer {
    match surface {
        Surface::V1 => v1(router, case, "CreateTemplate", &v1_template_fields(name, content)).await,
        Surface::V2 => {
            v2(
                router,
                case,
                "POST",
                "/v2/email/templates",
                Some(json!({"TemplateName": name, "TemplateContent": v2_content(content)})),
            )
            .await
        }
    }
}

/// Replace the content of `name` with `content`, on `surface`.
async fn replace(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    name: &str,
    content: Content,
) -> Answer {
    match surface {
        Surface::V1 => v1(router, case, "UpdateTemplate", &v1_template_fields(name, content)).await,
        Surface::V2 => {
            v2(
                router,
                case,
                "PUT",
                &format!("/v2/email/templates/{name}"),
                Some(json!({"TemplateContent": v2_content(content)})),
            )
            .await
        }
    }
}

/// Delete `name` on `surface`.
async fn delete(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    name: &str,
) -> Answer {
    match surface {
        Surface::V1 => {
            v1(
                router,
                case,
                "DeleteTemplate",
                &[("TemplateName", name.to_string())],
            )
            .await
        }
        Surface::V2 => {
            v2(
                router,
                case,
                "DELETE",
                &format!("/v2/email/templates/{name}"),
                None,
            )
            .await
        }
    }
}

/// Read `name` on `surface`, as `(the answer, the content it reports)`. The content is
/// `None` when the read was refused.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    name: &str,
) -> (Answer, Option<(String, Option<String>, Option<String>)>) {
    match surface {
        Surface::V1 => {
            let answered = v1(
                router,
                case,
                "GetTemplate",
                &[("TemplateName", name.to_string())],
            )
            .await;
            let content = block_of(&answered.text, "Template").map(|block| {
                (
                    elements(block, "SubjectPart")
                        .into_iter()
                        .next()
                        .unwrap_or_default(),
                    elements(block, "TextPart").into_iter().next(),
                    elements(block, "HtmlPart").into_iter().next(),
                )
            });
            (answered, content)
        }
        Surface::V2 => {
            let answered = v2(
                router,
                case,
                "GET",
                &format!("/v2/email/templates/{name}"),
                None,
            )
            .await;
            let content = answered.json.get("TemplateContent").map(|content| {
                let member = |name: &str| {
                    content
                        .get(name)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                (
                    member("Subject").unwrap_or_default(),
                    member("Text"),
                    member("Html"),
                )
            });
            (answered, content)
        }
    }
}

/// The content a read must report for `content`: the subject, the text part and the HTML
/// part, with an absent part reported as absent.
fn expected_content(content: Content) -> (String, Option<String>, Option<String>) {
    (
        content.subject.to_string(),
        content.text.map(str::to_string),
        content.html.map(str::to_string),
    )
}

/// One page of the account's template listing, asked of `surface`: `(the names it
/// reported, the NextToken it carried)`.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    match surface {
        Surface::V1 => {
            let mut fields: Vec<(&str, String)> = vec![("MaxItems", page_size.to_string())];
            if let Some(token) = token {
                fields.push(("NextToken", token.to_string()));
            }
            let answered = v1(router, case, "ListTemplates", &fields).await;
            succeeded(case, "SES v1 ListTemplates", &answered);
            let block = block_of(&answered.text, "TemplatesMetadata")
                .map(str::to_string)
                .unwrap_or_default();
            let names = members_of(&block)
                .into_iter()
                .filter_map(|member| elements(member, "Name").into_iter().next())
                .collect();
            (names, opt_element(&answered.text, "NextToken"))
        }
        Surface::V2 => {
            let mut query = format!("PageSize={page_size}");
            if let Some(token) = token {
                // The v2 listing binds `NextToken` to `@httpQuery` and the token is
                // base64, whose alphabet includes `+`, `/` and `=` — characters a query
                // string gives its own meaning to, so the value is escaped the way an SDK
                // escapes it.
                query.push_str(&format!("&NextToken={}", form_encode(token)));
            }
            let answered = v2(
                router,
                case,
                "GET",
                &format!("/v2/email/templates?{query}"),
                None,
            )
            .await;
            succeeded(case, "SESv2 ListEmailTemplates", &answered);
            let names = answered
                .json
                .get("TemplatesMetadata")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("TemplateName")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            let token = answered
                .json
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|token| !token.is_empty());
            (names, token)
        }
    }
}

/// Every template name the listing reports, asked of `first` for the first page and of the
/// surface `switch` answers for each page after it — so a walk can hand every cursor to
/// the OTHER API.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    mut surface: Surface,
    switch: impl Fn(Surface) -> Surface,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count a row here can ask for, so a walk that never
    // terminates fails instead of looping forever.
    for round in 0..1_000 {
        let (names, next) = page(router, case, surface, page_size, token.as_deref()).await;
        assert!(
            names.len() <= page_size,
            "case {case:?} page {round}: expected at most {page_size} names, got {names:?}"
        );
        seen.extend(names);
        match next {
            None => return seen,
            Some(next) => {
                token = Some(next);
                surface = switch(surface);
            }
        }
    }
    panic!("case {case:?}: the walk never ran out of pages");
}

#[tokio::test]
async fn a_template_recorded_on_either_surface_is_the_accounts_template() {
    for row in rows() {
        let case = row.case;
        let router = router();

        // The create, on the surface the row declares.
        let created = create(&router, case, row.created_on, row.template, row.created_with).await;
        succeeded(case, "the create", &created);

        // Both APIs read the template and both list it, whichever recorded it.
        for surface in Surface::both() {
            let (answered, content) = read(&router, case, surface, row.template).await;
            succeeded(case, &format!("the read on {surface:?}"), &answered);
            assert_eq!(
                content,
                Some(expected_content(row.created_with)),
                "case {case:?}: the read on {surface:?} must report the content the create \
                 carried, got {}",
                answered.text
            );
            let listed = walk(&router, case, 2, surface, |surface| surface).await;
            assert!(
                listed.contains(&row.template.to_string()),
                "case {case:?}: the listing on {surface:?} must report the template, got \
                 {listed:?}"
            );
        }

        // A second create under the same name on the OTHER surface is the naming conflict
        // it is: one account template, one name.
        let again = create(
            &router,
            case,
            row.created_on.other(),
            row.template,
            row.created_with,
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: a second create on {:?} must be refused, got {}: {}",
            row.created_on.other(),
            again.status,
            again.text
        );
        assert!(
            again.error_code().contains("AlreadyExists"),
            "case {case:?}: a second create must name the naming conflict, got {:?} in {}",
            again.error_code(),
            again.text
        );

        // The content replacement, if the row declares one: both APIs then report the
        // content that was written, including the parts it CLEARED.
        let expected = match row.replaced {
            None => row.created_with,
            Some((surface, content)) => {
                let replaced = replace(&router, case, surface, row.template, content).await;
                succeeded(case, &format!("the replacement on {surface:?}"), &replaced);
                content
            }
        };
        for surface in Surface::both() {
            let (answered, content) = read(&router, case, surface, row.template).await;
            succeeded(case, &format!("the read on {surface:?}"), &answered);
            assert_eq!(
                content,
                Some(expected_content(expected)),
                "case {case:?}: the read on {surface:?} must report the account's content, \
                 got {}",
                answered.text
            );
        }

        // The delete, on the surface the row declares: the template is gone from BOTH.
        let deleted = delete(&router, case, row.deleted_on, row.template).await;
        succeeded(case, &format!("the delete on {:?}", row.deleted_on), &deleted);
        for surface in Surface::both() {
            let (answered, content) = read(&router, case, surface, row.template).await;
            assert!(
                answered.status >= 400 && content.is_none(),
                "case {case:?}: the read on {surface:?} must refuse the deleted template, got \
                 {}: {}",
                answered.status,
                answered.text
            );
            let listed = walk(&router, case, 2, surface, |surface| surface).await;
            assert!(
                !listed.contains(&row.template.to_string()),
                "case {case:?}: the listing on {surface:?} must omit the deleted template, \
                 got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_template_listing_cursor_issued_by_either_surface_continues_the_other() {
    // More templates than the largest page a row asks for, recorded alternately through
    // the two APIs, so the one list a walk pages holds templates of both surfaces.
    let names: Vec<String> = (0..7).map(|index| format!("template-{index:02}")).collect();
    let router = router();
    for (index, name) in names.iter().enumerate() {
        let surface = if index % 2 == 0 {
            Surface::V1
        } else {
            Surface::V2
        };
        let content = Content {
            subject: "Your receipt",
            text: Some("Thanks."),
            html: None,
        };
        let created = create(&router, "the shared walk", surface, name, content).await;
        succeeded(
            "the shared walk",
            &format!("the create of {name} on {surface:?}"),
            &created,
        );
    }
    let expected: Vec<String> = {
        let mut sorted = names.clone();
        sorted.sort();
        sorted
    };

    // Each row is a page size and the surface the FIRST page is asked of; every page after
    // it is asked of the other API, so every cursor crosses the surface that issued it.
    for page_size in [1_usize, 2, 3, names.len(), names.len() + 1] {
        for first in Surface::both() {
            let case = format!("page size {page_size} starting on {first:?}, alternating");
            let seen = walk(&router, &case, page_size, first, Surface::other).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: a cursor must continue the one list of the account's \
                 templates, visiting each exactly once in name order, got {seen:?}"
            );

            // And the same walk asked of ONE surface throughout reports the same list, so
            // crossing surfaces neither repeats nor skips an entry.
            let case = format!("page size {page_size} on {first:?} throughout");
            let same = walk(&router, &case, page_size, first, |surface| surface).await;
            assert_eq!(
                same, expected,
                "case {case:?}: expected the same list, got {same:?}"
            );
        }
    }
}
