// SPDX-License-Identifier: CC0-1.0

//! The AWS Artifact FRONT DOOR, end to end over HTTP.
//!
//! AWS Artifact is not reached directly: it is served through a gateway that runs
//! before the service and answers in shapes this service's model does not declare. The
//! rule this suite pins, over the whole class of requests it covers rather than any one
//! of them:
//!
//! 1. **The targeted resource is authorized first.** A request that names a resource
//!    the gateway cannot authorize — an identifier that is not a well-formed id of its
//!    type, so it names nothing, or one the account's own catalog or inquiry set does not
//!    hold — is refused with HTTP 401 `UnauthorizedException`, ahead of the modelled
//!    member constraints and ahead of the service's own not-found. A target it CAN
//!    authorize is passed through to the operation, whatever the operation then answers.
//! 2. **The request body is validated next.** A body that does not bind to the
//!    modelled shape — a value outside an enum's set or a member's constraint, or a
//!    member the shape requires and the body omits — is refused with HTTP 400
//!    `BadRequestException`.
//! 3. **A value the URI carries is not the gateway's concern.** A `@httpQuery` or
//!    `@httpLabel` value outside its modelled constraint is refused by the SERVICE, as
//!    its modelled 400 `ValidationException` naming the member and the constraint.
//!
//! Each refusal carries the gateway's own constant text (`Unauthorized`, `Invalid
//! request body`): it names neither member nor constraint, because the gateway knows
//! only that it refused the call.
//!
//! Every assertion goes through the router, so each case exercises the same path a
//! client takes: routing, the front door, deserialization, validation and the handler.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The front door's authorization refusal.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");
/// The front door's request-body refusal.
const INVALID_BODY: (u16, &str, &str) = (400, "BadRequestException", "Invalid request body");

/// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`, satisfied: a well-formed id, so
/// the front door's PRE-DISPATCH checks have a target they can parse and go on to judge
/// the body. It names no stored inquiry, which is why the rows below that use it are
/// body-refusal rows: whether the account holds the inquiry is a state question, answered
/// after the body is validated. Spelled out here rather than taken from the crate so a
/// wrong predicate in `src/state.rs` cannot make this suite pass.
const WELL_FORMED_INQUIRY: &str = "compliance-inquiry-0123456789abcdef";

/// Well-formed inquiry ids the account holds no inquiry for — the second class of
/// unauthorizable target, judged against this service's state rather than against the id
/// syntax. The rows differ along what the 16-character suffix generalises over.
const UNHELD_INQUIRY_IDS: &[(&str, &str)] = &[
    ("all digits", "compliance-inquiry-0000000000000000"),
    ("all lowercase", "compliance-inquiry-zzzzzzzzzzzzzzzz"),
    ("all uppercase", "compliance-inquiry-ZZZZZZZZZZZZZZZZ"),
    ("mixed case and digits", "compliance-inquiry-AbCdEf0123456789"),
];

/// Identifiers that satisfy no resource-id syntax of this service, so they name nothing
/// the gateway could authorize. The rows differ along what the pattern generalises
/// over: the prefix, the suffix length, and the suffix alphabet.
const MALFORMED_IDS: &[(&str, &str)] = &[
    ("free text", "not-a-valid-id"),
    ("no prefix", "0123456789abcdef"),
    ("suffix too short", "abc"),
    ("suffix too long", "0123456789abcdef0"),
    ("suffix outside the alphabet", "0123456789abcde_"),
    ("empty", ""),
];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the body, and the error-shape header
/// restJson1 resolves an error shape from.
struct Answer {
    status: u16,
    errortype_header: Option<String>,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error body's `message`, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request through the router.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype_header = response
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
    Answer {
        status,
        errortype_header,
        body,
    }
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    call(router, "GET", uri, None).await
}

/// Assert that `answer` is the front-door refusal `expected` — status, wire error code,
/// the `x-amzn-errortype` header a restJson1 client resolves the shape from, and the
/// gateway's constant message.
fn assert_refused(case: &str, answer: &Answer, expected: (u16, &str, &str)) {
    let (status, code, message) = expected;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.errortype_header.as_deref(),
        Some(code),
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype_header
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        answer.body
    );
}

/// A body `CreateComplianceInquiry` accepts, with `overrides` merged over it. The base
/// satisfies every modelled constraint, so a row changes exactly one thing.
fn create_body(overrides: Value) -> Value {
    let mut body = json!({
        "name": "Does the SOC 2 report cover eu-west-1?",
        "inquiryContent": { "query": "Does the SOC 2 report cover eu-west-1?" },
        "supportMode": "AI_ONLY"
    });
    let Value::Object(fields) = overrides else {
        return body;
    };
    for (member, value) in fields {
        if value.is_null() {
            body.as_object_mut().expect("an object").remove(&member);
        } else {
            body[member] = value;
        }
    }
    body
}

/// The id of a report the catalog publishes, read from `ListReports` rather than
/// hard-coded: the catalog is service-owned, and this suite only needs SOME report the
/// account is entitled to.
async fn a_published_report(router: &MultiServiceRouter) -> String {
    let listed = get(router, "/v1/report/list").await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["reports"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_report_target_the_front_door_cannot_authorize_is_refused_with_401() {
    let router = emulator();

    // Two classes of unauthorizable target, both refused the same way: an id that is
    // not a well-formed `ReportId` names no report at all, and a well-formed one the
    // catalog does not publish names no report THIS account may read.
    let mut targets: Vec<(String, String)> = MALFORMED_IDS
        .iter()
        .map(|(case, id)| (format!("malformed id, {case}"), (*id).to_string()))
        .collect();
    for unseeded in ["report-0000000000000000", "report-zzzzzzzzzzzzzzzz"] {
        targets.push((format!("unpublished id {unseeded}"), unseeded.to_string()));
    }

    for (label, id) in targets {
        // Every operation whose request targets a report, at its modelled URI.
        for (operation, uri) in [
            (
                "GetReportMetadata",
                format!("/v1/report/getMetadata?reportId={id}"),
            ),
            (
                "ListReportVersions",
                format!("/v1/report/listVersions?reportId={id}"),
            ),
            (
                "GetTermForReport",
                format!("/v1/report/getTermForReport?reportId={id}"),
            ),
            (
                "GetReport",
                format!("/v1/report/get?reportId={id}&termToken=never-issued"),
            ),
        ] {
            let answer = get(&router, &uri).await;
            assert_refused(&format!("{operation} {label}"), &answer, UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn a_compliance_inquiry_target_the_front_door_cannot_authorize_is_refused_with_401() {
    let router = emulator();

    for (case, id) in MALFORMED_IDS {
        // The identifier is bound to the query string on two operations and to the
        // request body on the other two; the gateway authorizes the target either way,
        // and does so BEFORE it validates that body.
        let answers = [
            (
                "GetComplianceInquiryMetadata",
                get(
                    &router,
                    &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
                )
                .await,
            ),
            (
                "ListComplianceInquiryQueries",
                get(
                    &router,
                    &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
                )
                .await,
            ),
            (
                "ExportComplianceInquiry",
                call(
                    &router,
                    "POST",
                    "/v1/compliance-inquiry/export",
                    Some(json!({ "complianceInquiryId": id })),
                )
                .await,
            ),
            (
                "PutComplianceInquiryFeedback",
                call(
                    &router,
                    "PUT",
                    "/v1/compliance-inquiry/putFeedback",
                    Some(json!({ "complianceInquiryId": id, "rating": "THUMBS_UP" })),
                )
                .await,
            ),
        ];
        for (operation, answer) in answers {
            assert_refused(&format!("{operation} malformed id, {case}"), &answer, UNAUTHORIZED);
        }
    }

    // A report id is not an inquiry id: the kind of resource the operation targets is
    // what decides whether an identifier names anything.
    let crossed = get(
        &router,
        "/v1/compliance-inquiry/getMetadata?complianceInquiryId=report-cloudsecalliance",
    )
    .await;
    assert_refused("a report id as an inquiry id", &crossed, UNAUTHORIZED);

    // The other class of unauthorizable target: a well-formed id the account holds no
    // inquiry for. That judgement needs this service's state, so the HANDLER makes it and
    // answers the same refusal — exactly as an unpublished `reportId` is answered above.
    // All four inquiry operations are implemented and each makes it; the rows for the
    // other three live in `implemented_inquiry_operations.rs`, beside the evidence that
    // those same operations serve an inquiry the account does hold.
    for (case, id) in UNHELD_INQUIRY_IDS {
        let answer = get(
            &router,
            &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        )
        .await;
        assert_refused(
            &format!("GetComplianceInquiryMetadata unheld id, {case}"),
            &answer,
            UNAUTHORIZED,
        );
    }
}

#[tokio::test]
async fn a_well_formed_target_reaches_the_operation() {
    let router = emulator();

    // A target the gateway authorizes is passed through, and the operation answers for
    // itself. For an inquiry that means an inquiry this account HOLDS, so one is created
    // first: `GetComplianceInquiryMetadata` and `ListComplianceInquiryQueries` then serve
    // it — what each answers beyond "not refused by the front door" is pinned by their own
    // suites, which this one deliberately does not duplicate.
    let created = call(
        &router,
        "POST",
        "/v1/compliance-inquiry/create",
        Some(create_body(json!({}))),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the control create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let held = created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (operation, answer) in [
        (
            "GetComplianceInquiryMetadata",
            get(
                &router,
                &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={held}"),
            )
            .await,
        ),
        (
            "ListComplianceInquiryQueries",
            get(
                &router,
                &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={held}"),
            )
            .await,
        ),
    ] {
        assert_ne!(
            answer.status,
            UNAUTHORIZED.0,
            "case {operation}: an inquiry the account holds must not be refused, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.status,
            INVALID_BODY.0,
            "case {operation}: a request with no body must not be refused as one, got {:?}",
            answer.body
        );
    }

    // And the read of the held inquiry is SERVED, which is what says the refusal above is
    // a judgement on the target rather than a blanket denial of the operation.
    let served = get(
        &router,
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={held}"),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case {held:?}: a held inquiry must be served, got {} with {:?}",
        served.status, served.body
    );

    // And a report the catalog publishes is served, which is what says the front door
    // judges the target rather than refusing the class of request.
    let published = a_published_report(&router).await;
    let served = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={published}"),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case {published:?}: a published report must be served, got {} with {:?}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_request_body_the_front_door_cannot_bind_is_refused_with_400_bad_request() {
    let router = emulator();

    // One row per way a body can fail to bind, across every operation that carries
    // one. Each names the member and the modelled rule it breaks; the value is always
    // the only thing wrong with the request.
    let cases: Vec<(String, &str, &str, Value)> = vec![
        // `supportMode`: InquirySupportMode, enum AI_ONLY.
        (
            "CreateComplianceInquiry supportMode outside its set".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "supportMode": "BOGUS_MODE" })),
        ),
        (
            "CreateComplianceInquiry supportMode in the wrong case".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "supportMode": "ai_only" })),
        ),
        (
            "CreateComplianceInquiry supportMode empty".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "supportMode": "" })),
        ),
        // `clientToken`: IdempotentClientToken, length 1..128, pattern ^[\w\-]+$.
        (
            "CreateComplianceInquiry clientToken below its length".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "clientToken": "" })),
        ),
        (
            "CreateComplianceInquiry clientToken past its length".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "clientToken": "t".repeat(129) })),
        ),
        (
            "CreateComplianceInquiry clientToken outside its pattern".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "clientToken": "has a space" })),
        ),
        // `inquiryContent` has no row: it targets a UNION, and the framework's
        // validating walk does not descend into a union's member, so no violation on
        // `inquiryContent.query` is produced for the front door (or for the service)
        // to answer. Its ABSENCE is a row below, because that one the codec reports.
        // A member the shape REQUIRES, absent from the body.
        (
            "CreateComplianceInquiry without the required name".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "name": null })),
        ),
        (
            "CreateComplianceInquiry without the required inquiryContent".to_string(),
            "POST",
            "/v1/compliance-inquiry/create",
            create_body(json!({ "inquiryContent": null })),
        ),
        // `rating`: FeedbackRating, enum THUMBS_UP | THUMBS_DOWN. The inquiry id is
        // well formed, so the target is authorized and the BODY is what is refused.
        (
            "PutComplianceInquiryFeedback rating outside its set".to_string(),
            "PUT",
            "/v1/compliance-inquiry/putFeedback",
            json!({ "complianceInquiryId": WELL_FORMED_INQUIRY, "rating": "MAYBE" }),
        ),
        // `comment`: FeedbackCommentAttribute, length 1..1000, pattern
        // ^[a-zA-Z0-9\s.,;:!?'\-()/@#&+=]+$.
        (
            "PutComplianceInquiryFeedback comment outside its pattern".to_string(),
            "PUT",
            "/v1/compliance-inquiry/putFeedback",
            json!({
                "complianceInquiryId": WELL_FORMED_INQUIRY,
                "rating": "THUMBS_DOWN",
                "comment": "angle <brackets> are not in the set"
            }),
        ),
        (
            "PutComplianceInquiryFeedback comment past its length".to_string(),
            "PUT",
            "/v1/compliance-inquiry/putFeedback",
            json!({
                "complianceInquiryId": WELL_FORMED_INQUIRY,
                "rating": "THUMBS_DOWN",
                "comment": "c".repeat(1001)
            }),
        ),
        // `notificationSubscriptionStatus`: NotificationSubscriptionStatus, enum
        // SUBSCRIBED | NOT_SUBSCRIBED.
        (
            "PutAccountSettings notificationSubscriptionStatus outside its set".to_string(),
            "PUT",
            "/v1/account-settings/put",
            json!({ "notificationSubscriptionStatus": "MAYBE_SUBSCRIBED" }),
        ),
        // `tags`: TagsMap, length ..50 entries, each key 1..128 of
        // ^[a-zA-Z0-9\s_.:/=+\-@]*$.
        (
            "TagResource tags past the map's entry count".to_string(),
            "POST",
            "/tags/tagged-thing",
            {
                let mut tags = serde_json::Map::new();
                for entry in 0..51 {
                    tags.insert(format!("key{entry}"), Value::String("value".to_string()));
                }
                json!({ "tags": tags })
            },
        ),
        (
            "TagResource a tag value outside its pattern".to_string(),
            "POST",
            "/tags/tagged-thing",
            json!({ "tags": { "owner": "not<allowed>" } }),
        ),
    ];

    for (case, method, uri, body) in cases {
        let answer = call(&router, method, uri, Some(body)).await;
        assert_refused(&case, &answer, INVALID_BODY);
    }
}

#[tokio::test]
async fn a_value_the_uri_carries_keeps_the_services_own_validation_error() {
    let router = emulator();
    let published = a_published_report(&router).await;

    // Each row breaks a modelled constraint on a member bound to the URI, which the
    // gateway does not validate: the SERVICE refuses it, as its modelled
    // `ValidationException`, and the message names the member.
    let cases = [
        (
            "ListReports maxResults below its range",
            "GET".to_string(),
            "/v1/report/list?maxResults=0".to_string(),
            "maxResults",
        ),
        (
            "ListReports maxResults past its range",
            "GET".to_string(),
            "/v1/report/list?maxResults=301".to_string(),
            "maxResults",
        ),
        (
            "ListReportVersions nextToken past its length",
            "GET".to_string(),
            format!(
                "/v1/report/listVersions?reportId={published}&nextToken={}",
                "t".repeat(2049)
            ),
            "nextToken",
        ),
        (
            "GetReport termToken past its length",
            "GET".to_string(),
            format!(
                "/v1/report/get?reportId={published}&termToken={}",
                "t".repeat(257)
            ),
            "termToken",
        ),
        (
            "UntagResource tagKeys below the list's length",
            "DELETE".to_string(),
            "/tags/tagged-thing?tagKeys=".to_string(),
            "tagKeys",
        ),
    ];

    for (case, method, uri, member) in cases {
        let answer = call(&router, &method, &uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: a URI-bound value is the service's to refuse, got {:?}",
            answer.body
        );
        assert!(
            answer.message().contains(member),
            "case {case:?}: the message must name {member}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_body_that_binds_reaches_the_operation() {
    let router = emulator();

    // Every value the model admits for `supportMode` — `AI_ONLY` is the whole enum — plus
    // the member left out entirely ("Defaults to AI_ONLY if not specified"): the front door
    // passes each one, so none of them is answered as a refusal of the request. The row
    // that submits an upload is there because the front door's job is to BIND the member
    // whatever form the content takes.
    let rows = [
        ("AI_ONLY", create_body(json!({ "supportMode": "AI_ONLY" }))),
        (
            "AI_ONLY on an upload",
            create_body(json!({
                "supportMode": "AI_ONLY",
                // Base64 of an uploaded document's bytes, as restJson1 carries a blob.
                "inquiryContent": { "fileContent": { "content": "cXVlc3Rpb25uYWlyZQ==" } },
            })),
        ),
        ("absent", create_body(json!({ "supportMode": null }))),
    ];
    for (case, body) in rows {
        let answer = call(&router, "POST", "/v1/compliance-inquiry/create", Some(body)).await;
        assert_ne!(
            answer.status,
            INVALID_BODY.0,
            "case supportMode {case}: a body the model admits must not be refused, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.status,
            UNAUTHORIZED.0,
            "case supportMode {case}: a request naming no resource has no target to \
             refuse, got {:?}",
            answer.body
        );
    }
}
