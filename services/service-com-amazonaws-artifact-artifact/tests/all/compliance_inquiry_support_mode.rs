// SPDX-License-Identifier: CC0-1.0

//! `CreateComplianceInquiry`'s `supportMode`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is one rule:
//!
//! * `InquirySupportMode` admits exactly ONE value — `AI_ONLY`
//!   (`smithy_query(query="shape",shape="InquirySupportMode")`), which is also the value
//!   the member's own documentation makes the default: "Support mode for inquiry
//!   processing. Only supported for file upload mode. Defaults to AI_ONLY if not
//!   specified."
//!   (`smithy_query(query="doc",shape="CreateComplianceInquiryRequest",member="supportMode")`).
//!   So every accepted submission is processed under that mode and every read of the
//!   inquiry reports it, whether the request named it or left it out, whatever form the
//!   submitted content took;
//! * and the mode a read reports agrees with the `ReviewType` the inquiry's queries carry:
//!   the one mode the model admits "provides AI-generated responses", so every query is
//!   `AI`-reviewed and no read advertises a review nothing performed;
//! * a value OUTSIDE that enum is refused before an inquiry is created. This service is
//!   reached through a gateway that binds the request body against the modelled shape, so
//!   such a request is answered with that gateway's 400 `BadRequestException` (the refusal
//!   itself is pinned per member in `front_door_errors.rs`), nothing is persisted, and the
//!   `clientToken` the refused call carried is not spent.
//!
//! The rows differ along what the rule generalises over: the form of the content (a
//! submitted query versus an uploaded document), whether the member is named or left out,
//! the size of the query text (one character, a sentence, and the longest
//! `LongStringAttribute` admits), how many sections an upload names, whether the upload
//! carries any bytes, and whether the request also carries tags and a `clientToken`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const UPLOAD: &str = "cXVlc3Rpb25uYWlyZQ==";

/// `LongStringAttribute`'s published maximum, which `InquiryContent$query` targets
/// (`smithy_query(query="shape",shape="LongStringAttribute")` — `@length(min: 1, max:
/// 2048)`): a row submits a query of exactly this length, so the rule is asserted at the
/// bound as well as below it.
const QUERY_TEXT_CEILING: usize = 2048;

/// The one value `InquirySupportMode` admits, and the member's documented default
/// (`smithy_query(query="shape",shape="InquirySupportMode")`).
const SUPPORT_MODE: &str = "AI_ONLY";

/// `ReviewType`'s value for a response the service's own model reviews
/// (`smithy_query(query="shape",shape="ReviewType")`).
const REVIEW_AI: &str = "AI";

/// The front door's refusal of a body it cannot bind to the modelled shape: the status and
/// the wire error code a restJson1 client resolves the shape from. Values outside a
/// modelled enum are among the bodies it cannot bind, so this is what a `supportMode`
/// outside `InquirySupportMode` is answered with.
const INVALID_BODY: (u16, &str) = (400, "BadRequestException");

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The id of the inquiry whichever projection this answer carries names.
    fn inquiry_id(&self) -> String {
        for projection in ["complianceInquirySummary", "complianceInquiryDetail"] {
            if let Some(id) = self.body[projection]["id"].as_str() {
                return id.to_string();
            }
        }
        panic!("expected an answer carrying an inquiry id, got {:?}", self.body)
    }

    /// The `queries` a listing reported.
    fn queries(&self) -> &[Value] {
        self.body["queries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `complianceInquiries` a listing reported.
    fn inquiries(&self) -> &[Value] {
        self.body["complianceInquiries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `supportMode` a metadata read reported.
    fn detail_support_mode(&self) -> Option<&str> {
        self.body["complianceInquiryDetail"]["supportMode"].as_str()
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let uri = format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}");
    send(router, "GET", &uri, Bytes::new()).await
}

async fn list_queries(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let uri = format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}");
    send(router, "GET", &uri, Bytes::new()).await
}

async fn list_inquiries(router: &MultiServiceRouter) -> Answer {
    send(router, "GET", "/v1/compliance-inquiry/list", Bytes::new()).await
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(name: &str, query: &str, support_mode: Option<&str>) -> Value {
    body(name, json!({ "query": query }), support_mode)
}

/// A `CreateComplianceInquiry` body for an uploaded document of `content` bytes.
fn file_body(name: &str, content: &str, sections: &[&str], support_mode: Option<&str>) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    body(name, json!({ "fileContent": file }), support_mode)
}

fn body(name: &str, inquiry_content: Value, support_mode: Option<&str>) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    body
}

/// One submission the service accepts.
struct Case {
    label: &'static str,
    body: Value,
}

/// The submissions this suite covers: every one of them is accepted, and every one of them
/// reports the one mode the model admits.
fn cases() -> Vec<Case> {
    let longest_query = "a".repeat(QUERY_TEXT_CEILING);
    let mut tagged = text_body(
        "Subprocessors",
        "Which subprocessors handle customer content?",
        Some(SUPPORT_MODE),
    );
    tagged["tags"] = json!({ "team": "legal", "env": "prod" });
    tagged["clientToken"] = json!("token-tagged-submission");

    vec![
        // A submitted query, with the member named and left out, over the sizes the query
        // shape admits.
        Case {
            label: "a submitted query naming the mode",
            body: text_body(
                "Data residency",
                "Where is customer data stored?",
                Some(SUPPORT_MODE),
            ),
            },
        Case {
            label: "a one-character query naming the mode",
            body: text_body("Shortest", "?", Some(SUPPORT_MODE)),
        },
        Case {
            label: "the longest query the shape admits, naming the mode",
            body: text_body("Longest", &longest_query, Some(SUPPORT_MODE)),
        },
        Case {
            label: "a query naming the mode, with tags and a clientToken",
            body: tagged,
        },
        Case {
            label: "a submitted query naming no supportMode",
            body: text_body("Retention", "How long are audit logs retained?", None),
        },
        Case {
            label: "the longest query the shape admits, naming no supportMode",
            body: text_body("Longest", &longest_query, None),
        },
        // And an uploaded document — the form the member's documentation names — however
        // many sections it names, and even when it carries no bytes.
        Case {
            label: "an upload naming no section, naming the mode",
            body: file_body("Vendor questionnaire", UPLOAD, &[], Some(SUPPORT_MODE)),
        },
        Case {
            label: "an upload naming two sections, naming the mode",
            body: file_body(
                "Annual audit workbook",
                UPLOAD,
                &["Section A", "Section B"],
                Some(SUPPORT_MODE),
            ),
        },
        Case {
            label: "an upload carrying no bytes, naming the mode",
            body: file_body("Empty workbook", "", &["Section A"], Some(SUPPORT_MODE)),
        },
        Case {
            label: "an upload naming one section, naming the mode",
            body: file_body("Customer DDQ", UPLOAD, &["Sheet 1"], Some(SUPPORT_MODE)),
        },
        Case {
            label: "an upload naming no supportMode",
            body: file_body("Security addendum", UPLOAD, &[], None),
        },
    ]
}

#[tokio::test]
async fn every_accepted_submission_reports_the_one_support_mode_the_model_admits() {
    for case in cases() {
        let label = case.label;
        // One emulator per case, so what one case created cannot decide what another
        // observes.
        let router = emulator();
        let created = create(&router, &case.body).await;

        assert_eq!(
            created.status, 202,
            "case {label:?}: a submission the model admits must be accepted \
             with HTTP 202, got {} with {:?}",
            created.status, created.body
        );
        let id = created.inquiry_id();

        let read = get_metadata(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the created inquiry must read back, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.detail_support_mode(),
            Some(SUPPORT_MODE),
            "case {label:?}: expected the detail to report supportMode \
             {SUPPORT_MODE:?}, got {:?}",
            read.body["complianceInquiryDetail"]["supportMode"]
        );

        // The reported mode and the review its queries carry are one decision: the mode
        // the model admits is an AI review, so that is what every query reports.
        let listed = list_queries(&router, &id).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: the inquiry's queries must list, got {} with {:?}",
            listed.status, listed.body
        );
        for query in listed.queries() {
            assert_eq!(
                query["reviewType"].as_str(),
                Some(REVIEW_AI),
                "case {label:?}: a query of an inquiry reported under {SUPPORT_MODE:?} \
                 must carry reviewType {REVIEW_AI:?}, got {:?}",
                query["reviewType"]
            );
        }
    }
}

#[tokio::test]
async fn a_support_mode_outside_the_modelled_enum_creates_nothing_and_spends_no_client_token() {
    let token = "token-refused-support-mode";

    // The rows differ along the shape of the value that is not in the enum — a value of
    // another name, the one value in the wrong case, and the empty string — and along the
    // form of the submitted content.
    let rows: [(&str, Value); 4] = [
        (
            "a mode of another name",
            text_body(
                "Subprocessors",
                "Which subprocessors handle customer content?",
                Some("FULL_SUPPORT"),
            ),
        ),
        (
            "the admitted mode in the wrong case",
            text_body("Data residency", "Where is customer data stored?", Some("ai_only")),
        ),
        ("an empty mode", text_body("Retention", "How long are audit logs retained?", Some(""))),
        (
            "a mode of another name on an upload",
            file_body("Annual audit workbook", UPLOAD, &["Section A"], Some("BOGUS_MODE")),
        ),
    ];

    for (label, body) in rows {
        let router = emulator();
        let mut refused_body = body;
        refused_body["clientToken"] = json!(token);

        let refused = create(&router, &refused_body).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            INVALID_BODY,
            "case {label:?}: a supportMode outside the modelled enum must be refused by \
             the front door, got {:?}",
            refused.body
        );

        let listed = list_inquiries(&router).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListComplianceInquiries must answer, got {} with {:?}",
            listed.status, listed.body
        );
        assert!(
            listed.inquiries().is_empty(),
            "case {label:?}: a refused create must persist no inquiry, got {:?}",
            listed.body
        );

        // The refused call spent nothing: the same token on a submission the model admits
        // creates the inquiry, and repeating it replays that one.
        let mut accepted_body = text_body(
            "Subprocessors",
            "Which subprocessors handle customer content?",
            Some(SUPPORT_MODE),
        );
        accepted_body["clientToken"] = json!(token);
        let accepted = create(&router, &accepted_body).await;
        assert_eq!(
            accepted.status, 202,
            "case {label:?}: the token must still be available to a submission the model \
             admits, got {} with {:?}",
            accepted.status, accepted.body
        );
        let id = accepted.inquiry_id();
        let replay = create(&router, &accepted_body).await;
        assert_eq!(
            (replay.status, replay.inquiry_id()),
            (202, id.clone()),
            "case {label:?}: the token must replay the inquiry it created, got {:?}",
            replay.body
        );

        let read = get_metadata(&router, &id).await;
        assert_eq!(
            read.detail_support_mode(),
            Some(SUPPORT_MODE),
            "case {label:?}: the created inquiry must report the mode it was created \
             under, got {:?}",
            read.body["complianceInquiryDetail"]["supportMode"]
        );
    }
}
