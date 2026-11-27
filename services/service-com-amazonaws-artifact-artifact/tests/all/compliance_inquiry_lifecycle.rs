// SPDX-License-Identifier: CC0-1.0

//! The AWS Artifact compliance-inquiry surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones this surface states:
//!
//! * `CreateComplianceInquiry` CREATES a resource: it mints an `InquiryId`, persists the
//!   submission, and `GetComplianceInquiryMetadata` reports the same record back — so the
//!   detail a caller reads can never disagree with the summary the create returned,
//!   whatever form the submission took;
//! * the inquiry is created NON-TERMINAL (HTTP 202, `status` `PROCESSING`) and its
//!   terminal status is owed by the CLOCK, not by any call: a read before the due instant
//!   still reports `PROCESSING`, a read after it reports a terminal status, and every
//!   later read reports that same record;
//! * `clientToken` is an `@idempotencyToken`: the same token names the inquiry its first
//!   call created, a different token names a different inquiry, and the same token on
//!   different content is refused rather than honoured;
//! * an inquiry id the store does not hold is refused with the front door's 401
//!   `UnauthorizedException`, not with the modelled not-found: the stored inquiry set IS
//!   the account's entitlement, so the absence of an inquiry the caller may not read is
//!   never confirmed.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `InquiryStatus` declares, spelled out here rather than imported from the
/// crate so a status outside the enum fails this suite
/// (`smithy_query(query="shape",shape="InquiryStatus")`).
const INQUIRY_STATUSES: &[&str] = &["PROCESSING", "HUMAN_REVIEW", "COMPLETED", "FAILED"];

/// The values `InquiryStatusMessage` declares — it is an ENUM, so a `statusMessage` AWS
/// never emits fails this suite
/// (`smithy_query(query="shape",shape="InquiryStatusMessage")`).
const INQUIRY_STATUS_MESSAGES: &[&str] = &[
    "Compliance inquiry processing is complete.",
    "Malware was detected on the file. Provide a new file and try again.",
    "Compliance inquiry processing is in-progress.",
    "An internal error occurred while processing the inquiry. Try again at a later time.",
    "Human review is in progress.",
    "Compliance inquiry processing is complete. One or more queries encountered errors during processing.",
];

/// `InquiryStatusMessage`'s value a created inquiry carries while it is `PROCESSING`.
const MESSAGE_IN_PROGRESS: &str = "Compliance inquiry processing is in-progress.";

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// A SigV4 credential scope for `region` naming this service — what routes the request.
/// The region it carries is the caller's; it is not what an inquiry ARN names.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The region the suite signs for unless a case names another.
const REGION: &str = "eu-west-1";

/// The Region every ARN this service mints names, whatever Region a request was signed
/// for: this service is reached through one partitional endpoint, so every resource it
/// reports is homed in that one Region (`crate::state::SERVICE_REGION`).
const HOME_REGION: &str = "us-east-1";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob. Two distinct
/// payloads, so a case can change the content without changing anything else.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";
/// See [`FILE_CONTENT`].
const OTHER_FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZTI=";

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

    /// The `complianceInquirySummary` a create returned.
    fn summary(&self) -> &Value {
        &self.body["complianceInquirySummary"]
    }

    /// The `complianceInquiryDetail` a metadata read returned.
    fn detail(&self) -> &Value {
        &self.body["complianceInquiryDetail"]
    }

    /// The inquiry id in whichever projection this answer carries.
    fn inquiry_id(&self) -> String {
        for projection in [self.summary(), self.detail()] {
            if let Some(id) = projection["id"].as_str() {
                return id.to_string();
            }
        }
        panic!("the answer carries no inquiry id: {:?}", self.body)
    }
}

async fn send(router: &MultiServiceRouter, request: ::http::Request<Bytes>) -> Answer {
    let uri = request.uri().to_string();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/v1/compliance-inquiry/create")
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", signed_for(region))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, request).await
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    create_in(router, REGION, body).await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"
        ))
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, request).await
}

/// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`, checked here rather than through
/// the crate so a wrong minting cannot make this test pass.
fn matches_inquiry_id_pattern(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("compliance-inquiry-") else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A `tags` object for a create body, or `Value::Null` for an untagged submission.
fn tags_object(tags: &[(&str, &str)]) -> Value {
    if tags.is_empty() {
        return Value::Null;
    }
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(
    name: &str,
    query: &str,
    support_mode: Option<&str>,
    tags: &[(&str, &str)],
) -> Value {
    body(name, json!({ "query": query }), support_mode, tags)
}

/// A `CreateComplianceInquiry` body for an uploaded document.
fn file_body(
    name: &str,
    content: &str,
    sections: &[&str],
    support_mode: Option<&str>,
    tags: &[(&str, &str)],
) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    body(name, json!({ "fileContent": file }), support_mode, tags)
}

fn body(
    name: &str,
    inquiry_content: Value,
    support_mode: Option<&str>,
    tags: &[(&str, &str)],
) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    let tags = tags_object(tags);
    if !tags.is_null() {
        body["tags"] = tags;
    }
    body
}

/// One submission, and what the inquiry it creates must report.
struct Case {
    label: &'static str,
    body: Value,
    /// `InputSource`: `TEXT` for a submitted query, `FILE` for an uploaded document.
    input_source: &'static str,
    /// The `supportMode` the detail must report — "Defaults to AI_ONLY if not specified".
    support_mode: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// The `InquiryStatus` the clock owes this inquiry, and the message that goes with
    /// it. A submission's units of work become its queries, so a submission that carries
    /// any reaches `COMPLETED` once the service's own model has processed them — the one
    /// support mode the model admits (`AI_ONLY`) is an AI review. A submission carrying
    /// nothing to process — an upload with no bytes — owns no query, so its processing
    /// failed.
    terminal_status: &'static str,
    /// See [`Case::terminal_status`].
    terminal_message: &'static str,
}

/// The inquiry submissions this suite covers. The rows differ along everything the rules
/// generalise over: the form of the content (a query versus an uploaded document), how
/// many file sections it names, whether the upload carries any bytes at all, whether
/// `supportMode` is given and which value, and how many tags come with it.
fn cases() -> Vec<Case> {
    const COMPLETED: &str = "Compliance inquiry processing is complete.";
    const INQUIRY_INTERNAL_ERROR: &str =
        "An internal error occurred while processing the inquiry. Try again at a later time.";
    vec![
        Case {
            label: "a query, no supportMode, no tags",
            body: text_body("Data residency", "Where is customer data stored?", None, &[]),
            input_source: "TEXT",
            support_mode: "AI_ONLY",
            tags: &[],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "a query, AI_ONLY, one tag",
            body: text_body(
                "Encryption at rest",
                "Is customer data encrypted at rest?",
                Some("AI_ONLY"),
                &[("team", "compliance")],
            ),
            input_source: "TEXT",
            support_mode: "AI_ONLY",
            tags: &[("team", "compliance")],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "a query, AI_ONLY, three tags",
            body: text_body(
                "Subprocessors",
                "Which subprocessors handle customer content?",
                Some("AI_ONLY"),
                &[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")],
            ),
            input_source: "TEXT",
            support_mode: "AI_ONLY",
            tags: &[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, no sections",
            body: file_body("Vendor questionnaire", FILE_CONTENT, &[], None, &[]),
            input_source: "FILE",
            support_mode: "AI_ONLY",
            tags: &[],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, two sections, AI_ONLY",
            body: file_body(
                "Annual audit workbook",
                FILE_CONTENT,
                &["Section A", "Section B"],
                Some("AI_ONLY"),
                &[("env", "staging")],
            ),
            input_source: "FILE",
            support_mode: "AI_ONLY",
            tags: &[("env", "staging")],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, one section, other content",
            body: file_body(
                "Customer DDQ",
                OTHER_FILE_CONTENT,
                &["Sheet 1"],
                Some("AI_ONLY"),
                &[],
            ),
            input_source: "FILE",
            support_mode: "AI_ONLY",
            tags: &[],
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an upload carrying no bytes, so nothing to process",
            body: file_body("Empty workbook", "", &["Section A"], None, &[]),
            input_source: "FILE",
            support_mode: "AI_ONLY",
            tags: &[],
            terminal_status: "FAILED",
            terminal_message: INQUIRY_INTERNAL_ERROR,
        },
    ]
}

/// The tags an answer reports, as comparable pairs. The member is present whatever the
/// tag set: an untagged resource reports the empty map.
fn reported_tags(body: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = body["tags"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// The tags a case submitted, as comparable pairs.
fn submitted_tags(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    pairs.sort();
    pairs
}

#[tokio::test]
async fn every_submission_is_created_and_read_back_identically() {
    let router = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in cases() {
        let label = case.label;
        let created = create(&router, &case.body).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: CreateComplianceInquiry must answer HTTP 202, got {} with {:?}",
            created.status, created.body
        );

        let summary = created.summary().clone();
        let id = created.inquiry_id();
        assert!(
            matches_inquiry_id_pattern(&id),
            "case {label:?}: the minted id must satisfy InquiryId's \
             ^compliance-inquiry-[a-zA-Z0-9]{{16}}$, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {label:?}: every create must mint a distinct id, got {id:?} twice"
        );
        assert_eq!(
            summary["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:artifact:{HOME_REGION}:{}:compliance-inquiry/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {label:?}: the arn must name this service's Region, account and inquiry, \
             got {:?}",
            summary["arn"]
        );
        assert_eq!(
            summary["inputSource"].as_str(),
            Some(case.input_source),
            "case {label:?}: expected inputSource {:?}, got {:?}",
            case.input_source,
            summary["inputSource"]
        );
        assert_eq!(
            summary["status"].as_str(),
            Some("PROCESSING"),
            "case {label:?}: a created inquiry is accepted for processing, not terminal; \
             got {:?}",
            summary["status"]
        );
        assert_eq!(
            summary["statusMessage"].as_str(),
            Some(MESSAGE_IN_PROGRESS),
            "case {label:?}: expected the in-progress statusMessage, got {:?}",
            summary["statusMessage"]
        );
        assert_eq!(
            reported_tags(&created.body),
            submitted_tags(case.tags),
            "case {label:?}: the create must report the submitted tags, got {:?}",
            created.body["tags"]
        );

        let read = get_metadata(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetComplianceInquiryMetadata must find the created inquiry, \
             got {} with {:?}",
            read.status, read.body
        );
        let detail = read.detail().clone();
        for member in ["arn", "name", "id", "inputSource", "createdAt"] {
            assert_eq!(
                detail[member], summary[member],
                "case {label:?}: the detail's {member} must be the summary's, got {:?} \
                 against {:?}",
                detail[member], summary[member]
            );
        }
        assert_eq!(
            detail["supportMode"].as_str(),
            Some(case.support_mode),
            "case {label:?}: expected supportMode {:?}, got {:?}",
            case.support_mode,
            detail["supportMode"]
        );
        assert_eq!(
            reported_tags(&read.body),
            submitted_tags(case.tags),
            "case {label:?}: the metadata read must report the submitted tags, got {:?}",
            read.body["tags"]
        );
        let status = detail["status"].as_str().unwrap_or_default();
        let message = detail["statusMessage"].as_str().unwrap_or_default();
        assert!(
            INQUIRY_STATUSES.contains(&status),
            "case {label:?}: status must be one of InquiryStatus's values, got {status:?}"
        );
        assert!(
            INQUIRY_STATUS_MESSAGES.contains(&message),
            "case {label:?}: statusMessage must be one of InquiryStatusMessage's values, \
             got {message:?}"
        );
        // Read within the processing span or after it, the pair is one of exactly two.
        let legal = [
            ("PROCESSING", MESSAGE_IN_PROGRESS),
            (case.terminal_status, case.terminal_message),
        ];
        assert!(
            legal.contains(&(status, message)),
            "case {label:?}: expected one of {legal:?}, got {:?}",
            (status, message)
        );
        assert!(
            !detail["updatedAt"].is_null(),
            "case {label:?}: the detail must report when the resource was last modified, \
             got {:?}",
            detail["updatedAt"]
        );
    }
}

#[tokio::test]
async fn an_inquiry_reaches_its_terminal_status_on_the_clock_and_then_stops_changing() {
    for case in cases() {
        let label = case.label;
        let router = emulator();
        let created = create(&router, &case.body).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: expected HTTP 202, got {} with {:?}",
            created.status, created.body
        );
        let id = created.inquiry_id();

        // Before the due instant: still the non-terminal status the create reported, and
        // the resource has not been modified since it was created.
        let pending = get_metadata(&router, &id).await;
        let pending_detail = pending.detail().clone();
        assert_eq!(
            (
                pending_detail["status"].as_str(),
                pending_detail["statusMessage"].as_str()
            ),
            (Some("PROCESSING"), Some(MESSAGE_IN_PROGRESS)),
            "case {label:?}: a read inside the processing span must still report \
             PROCESSING, got {pending_detail:?}"
        );
        assert_eq!(
            pending_detail["updatedAt"], pending_detail["createdAt"],
            "case {label:?}: nothing has changed yet, so updatedAt is createdAt; got {:?} \
             against {:?}",
            pending_detail["updatedAt"], pending_detail["createdAt"]
        );

        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let settled = get_metadata(&router, &id).await;
        let settled_detail = settled.detail().clone();
        assert_eq!(
            (
                settled_detail["status"].as_str(),
                settled_detail["statusMessage"].as_str()
            ),
            (Some(case.terminal_status), Some(case.terminal_message)),
            "case {label:?}: past the processing span the inquiry must be terminal, got \
             {settled_detail:?}"
        );
        assert_ne!(
            settled_detail["updatedAt"], settled_detail["createdAt"],
            "case {label:?}: the transition modified the resource, so updatedAt must have \
             moved off createdAt; got {:?}",
            settled_detail["updatedAt"]
        );

        // A read must not change what a later reader sees: the terminal record is now
        // fixed, `updatedAt` included.
        let again = get_metadata(&router, &id).await;
        assert_eq!(
            again.detail(),
            &settled_detail,
            "case {label:?}: two reads of a terminal inquiry must report the same record, \
             got {:?} against {settled_detail:?}",
            again.detail()
        );
    }
}

#[tokio::test]
async fn an_inquiry_arn_names_this_service_s_own_region() {
    let router = emulator();
    // The rule generalises over the Region a caller signs for, so the rows are scopes:
    // this service's own Region, two others, and one whose region field is not a region
    // at all — none of which shapes the ARN.
    let cases = [HOME_REGION, "eu-west-1", "ap-southeast-3", "not_a_region"];
    for signed in cases {
        let body = text_body("Region", "Which region serves this?", None, &[]);
        let created = create_in(&router, signed, &body).await;
        assert_eq!(
            created.status, 202,
            "case {signed:?}: expected HTTP 202, got {} with {:?}",
            created.status, created.body
        );
        let id = created.inquiry_id();
        assert_eq!(
            created.summary()["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:artifact:{HOME_REGION}:{}:compliance-inquiry/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {signed:?}: expected an ARN in {HOME_REGION:?}, got {:?}",
            created.summary()["arn"]
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_names_the_inquiry_its_first_call_created() {
    let router = emulator();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let mut first = case.body.clone();
        first["clientToken"] = json!(format!("token-{index}"));
        let created = create(&router, &first).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: expected HTTP 202, got {} with {:?}",
            created.status, created.body
        );
        let id = created.inquiry_id();

        // The same token on the same submission is a RETRY: the same inquiry, not a
        // second one.
        let replay = create(&router, &first).await;
        assert_eq!(
            replay.status, 202,
            "case {label:?}: a replay must succeed, got {} with {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.inquiry_id(),
            id,
            "case {label:?}: a replayed clientToken must answer with the first inquiry, \
             got {:?}",
            replay.summary()
        );
        assert_eq!(
            replay.summary()["createdAt"],
            created.summary()["createdAt"],
            "case {label:?}: a replay reports the first call's createdAt, got {:?}",
            replay.summary()["createdAt"]
        );

        // A DIFFERENT token on the same submission is a new submission.
        let mut other_token = first.clone();
        other_token["clientToken"] = json!(format!("token-{index}-distinct"));
        let other = create(&router, &other_token).await;
        assert_eq!(
            other.status, 202,
            "case {label:?}: expected HTTP 202, got {} with {:?}",
            other.status, other.body
        );
        assert_ne!(
            other.inquiry_id(),
            id,
            "case {label:?}: a different clientToken must create a different inquiry"
        );
    }
}

#[tokio::test]
async fn a_client_token_reused_on_different_content_is_refused() {
    let router = emulator();
    let token = "token-reused";
    let original = {
        let mut body = text_body(
            "Original",
            "Is customer data encrypted at rest?",
            Some("AI_ONLY"),
            &[("team", "compliance")],
        );
        body["clientToken"] = json!(token);
        body
    };
    let created = create(&router, &original).await;
    assert_eq!(
        created.status, 202,
        "the first call must succeed, got {} with {:?}",
        created.status, created.body
    );
    let id = created.inquiry_id();

    // Each row changes exactly ONE thing about the submission the token was spent on —
    // every member the created inquiry is built from, and the content's whole form.
    let mut differs: Vec<(&str, Value)> = Vec::new();
    for (label, mutate) in [
        ("the title", 0usize),
        ("the query", 1),
        ("the content form and the sections it names", 2),
        ("a tag value", 3),
        ("an added tag", 4),
        ("the content form", 5),
    ] {
        let mut body = original.clone();
        match mutate {
            0 => body["name"] = json!("Renamed"),
            1 => body["inquiryContent"] = json!({ "query": "Is customer data encrypted in transit?" }),
            2 => {
                // `supportMode` cannot differ between two submissions: the model admits
                // one value and it is also the default, so a request naming it and one
                // leaving it out are processed under the same mode. The row differs on the
                // sections the uploaded content names instead.
                body["inquiryContent"] = json!({
                    "fileContent": { "content": FILE_CONTENT, "fileSections": ["Section A"] }
                });
            }
            3 => body["tags"] = json!({ "team": "legal" }),
            4 => body["tags"] = json!({ "team": "compliance", "env": "prod" }),
            _ => {
                body["inquiryContent"] = json!({ "fileContent": { "content": FILE_CONTENT } });
            }
        }
        differs.push((label, body));
    }

    for (label, body) in differs {
        let answer = create(&router, &body).await;
        assert_eq!(
            answer.status, 409,
            "case {label:?}: a clientToken reused on different content must be refused \
             with HTTP 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ConflictException",
            "case {label:?}: expected the modelled ConflictException, got {:?}",
            answer.body
        );
    }

    // The refusals created nothing and changed nothing: the first inquiry still reads
    // back, and the token still replays it.
    let read = get_metadata(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "the first inquiry must survive the refusals, got {} with {:?}",
        read.status, read.body
    );
    let replay = create(&router, &original).await;
    assert_eq!(
        (replay.status, replay.inquiry_id()),
        (202, id.clone()),
        "the token must still replay the first inquiry, got {:?}",
        replay.body
    );
}

#[tokio::test]
async fn an_inquiry_id_the_store_does_not_hold_is_refused_as_unauthorized() {
    let router = emulator();
    // A control, so the refusals below are about the id and not about the operation.
    let created = create(
        &router,
        &text_body("Control", "Where is customer data stored?", None, &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the control create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let known = created.inquiry_id();

    // Well-formed ids (`InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`) that this
    // account holds no inquiry for. The rows differ along what the 16-character suffix
    // generalises over — digits, lowercase, uppercase, mixed — plus one that differs from
    // the control's id in its last character only, so a refusal cannot come from matching
    // a prefix instead of the whole id.
    let mut unknown: Vec<(String, String)> = vec![
        (
            "all digits".to_string(),
            "compliance-inquiry-0000000000000000".to_string(),
        ),
        (
            "all lowercase".to_string(),
            "compliance-inquiry-ffffffffffffffff".to_string(),
        ),
        (
            "all uppercase".to_string(),
            "compliance-inquiry-ZZZZZZZZZZZZZZZZ".to_string(),
        ),
        (
            "mixed case and digits".to_string(),
            "compliance-inquiry-AbCdEf0123456789".to_string(),
        ),
    ];
    unknown.push((
        "one character off the control's id".to_string(),
        {
            let mut neighbour = known.clone();
            let last = neighbour.pop().expect("a minted id is not empty");
            neighbour.push(if last == 'z' { 'y' } else { 'z' });
            neighbour
        },
    ));

    for (case, id) in &unknown {
        assert!(
            matches_inquiry_id_pattern(id),
            "case {case:?}: the row must satisfy InquiryId's pattern, got {id:?}"
        );
        assert_ne!(
            id, &known,
            "case {case:?}: the unknown ids must not be the control's"
        );
        let answer = get_metadata(&router, id).await;
        assert_eq!(
            answer.status, 401,
            "case {case:?} {id:?}: an inquiry the account does not hold must answer HTTP \
             401, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "UnauthorizedException",
            "case {case:?} {id:?}: expected UnauthorizedException, got {:?}",
            answer.body
        );
        // The refusal says nothing about the resource: naming it (or its type) is what
        // would confirm the absence AWS declines to confirm.
        assert!(
            answer.body["resourceId"].is_null() && answer.body["resourceType"].is_null(),
            "case {case:?} {id:?}: an authorization refusal must disclose no resource, got \
             {:?}",
            answer.body
        );
        assert!(
            answer.detail().is_null(),
            "case {case:?} {id:?}: a refused read must carry no inquiry detail, got {:?}",
            answer.body
        );
    }

    let control = get_metadata(&router, &known).await;
    assert_eq!(
        control.status, 200,
        "the control inquiry must still be found, got {} with {:?}",
        control.status, control.body
    );
    assert_eq!(
        control.detail()["id"].as_str(),
        Some(known.as_str()),
        "the control inquiry must report its own id, got {:?}",
        control.body
    );
}
