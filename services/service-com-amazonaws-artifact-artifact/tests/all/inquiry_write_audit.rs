// SPDX-License-Identifier: CC0-1.0

//! Every inquiry mutator and the term-grant mint leave one attributable record.
//!
//! The rule under test, over a TABLE rather than one recorded request: a call on an
//! operation of this service that WRITES state a later caller can observe leaves EXACTLY ONE
//! structured record on the audit target, naming the operation, the caller (the access key id
//! and the credential-scope region the request presented), what the call addressed and the
//! outcome. A state-changing call that changed nothing is still attributable, so the
//! refusal, idempotent-replay and ignored-reuse paths report their outcome too; and no
//! record carries caller content
//! or a credential — not the inquiry's `@sensitive` name, content or feedback comment, and not
//! the download token or presigned URL a mint hands back.
//!
//! The rows differ along what the rule generalises over: which of the four writes made the
//! call (`CreateComplianceInquiry`, `PutComplianceInquiryFeedback`, `ExportComplianceInquiry`,
//! `GetTermForReport`), which path through that write the request took (a fresh write, an
//! idempotent replay, a request ignored for the token it reused, each refusal the handler
//! states), the form of the submitted content, how
//! many reason codes a submission carried, whether the resource the call addressed exists at
//! all, and which region the request was signed for.
//!
//! `resource_tag_write_audit.rs` states the same rule for the two tag writes and
//! `account_settings.rs` for the account-wide one; this suite covers the remaining four, so
//! every write of this service is pinned. The responses themselves are the subject of
//! `compliance_inquiry_feedback.rs`, `compliance_inquiry_export.rs` and
//! `term_grant_receipt.rs`: this suite asserts a status only to show which path a row took.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::audit_capture::audit_capture;

/// The access key id every call in this suite signs with: no other suite uses it, so this
/// suite's records can be told apart from a sibling's in the process-wide capture, which is
/// what lets each row demand exactly one record.
const AUDIT_AKID: &str = "AKIDAUDITINQUIRYWRITES";

/// The region the fixtures are created in, and the one most rows are signed for.
const REGION: &str = "eu-west-1";

/// A second region, which two rows sign for — so a record's `region` has to come from the
/// request's credential scope rather than from a constant or from the resource.
const OTHER_REGION: &str = "ap-southeast-2";

/// The endpoint host every request is addressed to.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// A well-formed `InquiryId` (`^compliance-inquiry-[a-zA-Z0-9]{16}$`, from
/// `smithy_query(query="operation",operation="ExportComplianceInquiry")`) this account holds
/// no inquiry for.
const ABSENT_INQUIRY: &str = "compliance-inquiry-0000000000000000";

/// A well-formed `ReportId` (`^report-[a-zA-Z0-9]{16}$`, from
/// `smithy_query(query="operation",operation="GetTermForReport")`) the catalog does not hold.
const ABSENT_REPORT: &str = "report-0000000000000000";

/// The report a term is accepted for, and the version it publishes. A row the issue did not
/// cite: `report-9pQ2mT8xLbV4sZ1c` ("SOC 2 Type 2 Report") requires the explicit acceptance
/// of its term, which is what a grant is the receipt for.
const GATED_REPORT: &str = "report-9pQ2mT8xLbV4sZ1c";

/// A version `GATED_REPORT` publishes.
const GATED_VERSION: i64 = 2;

/// A version no report publishes, past every version the catalog carries — still inside
/// `VersionAttribute`'s `@range(min: 1)`, so the request reaches the handler.
const UNPUBLISHED_VERSION: i64 = 9999;

/// `InquirySupportMode`'s one value. Source:
/// `smithy_query(query="shape",shape="InquirySupportMode")`.
const SUPPORT_MODE: &str = "AI_ONLY";

/// `FeedbackRating`'s values. Source: `smithy_query(query="shape",shape="FeedbackRating")`.
const RATINGS: [&str; 2] = ["THUMBS_UP", "THUMBS_DOWN"];

/// `FeedbackReasonCode`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackReasonCode")`.
const REASON_CODES: [&str; 3] = ["OTHER", "PARTIAL_RESPONSE", "IRRELEVANT_RESPONSE"];

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// The marker every `@sensitive` value this suite submits carries — the inquiry's name, the
/// query text inside its content and the feedback comment. No record may contain it: a record
/// names the resource a call addressed, never the caller's content.
const SENSITIVE_MARKER: &str = "sensitive-no-record-may-carry";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope for `region` naming this service, presenting [`AUDIT_AKID`].
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AUDIT_AKID}/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// Dispatch one request through the router, signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", ENDPOINT)
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// A `@sensitive` value distinguished by `n`, carrying [`SENSITIVE_MARKER`].
fn sensitive(n: usize) -> String {
    format!("{SENSITIVE_MARKER}-{n}")
}

/// The body of a text submission: a title and a single query, both `@sensitive`.
fn text_submission(n: usize) -> Value {
    json!({
        "name": sensitive(n),
        "inquiryContent": { "query": sensitive(n + 100) }
    })
}

/// Whether `record` carries `pair` as a WHOLE field: the renderer writes one space before
/// every `name=value`, so the pair has to start at such a space and end at the next one or at
/// the end of the line — which is what keeps `query_identifier=1` from being satisfied by
/// `query_identifier=11`, or `report_version=2` by `report_version=22`.
fn carries(record: &str, pair: &str) -> bool {
    let field = format!(" {pair}");
    record.match_indices(&field).any(|(at, matched)| {
        record[at + matched.len()..]
            .chars()
            .next()
            .is_none_or(|next| next == ' ')
    })
}

/// Which value the record's resource id must be read from, since a write that MINTS a
/// resource cannot have its id spelled out in the table.
enum Names {
    /// The record names nothing beyond the fields the row spells out.
    Nothing,
    /// The record must name the inquiry id the answer reports under
    /// `complianceInquirySummary.id` — a create's id is minted by the call.
    CreatedInquiry,
    /// The record must name the export id the announced document URL carries, and must carry
    /// neither that URL nor the download token in it.
    MintedExport,
    /// The record must carry neither the announced URL nor the `termToken` it hands back.
    MintedTermGrant,
}

/// One row: a request, and the record it must leave.
struct Case {
    /// What this row exercises — named in every assertion.
    case: &'static str,
    /// The operation the record must name.
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The region the request is signed for.
    region: &'static str,
    /// The status the call must answer — which path through the handler the row took.
    status: u16,
    /// The `outcome` the record must report.
    outcome: &'static str,
    /// Every further whole `field=value` pair the record must carry.
    fields: Vec<String>,
    names: Names,
}

/// Create one inquiry as a fixture and answer its id, draining nothing: the records the
/// fixtures leave are cleared once, before the table runs.
async fn fixture_inquiry(router: &MultiServiceRouter, n: usize) -> String {
    let created = call(
        router,
        REGION,
        "POST",
        "/v1/compliance-inquiry/create",
        Some(&text_submission(n)),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the fixture create must answer HTTP 202, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "the fixture create must report an id, got {:?}",
        created.body
    );
    id
}

/// The export id an announced export URL names: its path is
/// `…/<inquiry>/export/<exportId>/<object>`.
fn export_id_in(url: &str) -> String {
    let path = url.split('?').next().unwrap_or_default();
    let mut segments = path.split("/export/");
    segments.next();
    segments
        .next()
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_string()
}

/// The query-string value `parameter` carries in `url`, which is the token an announced URL
/// hands its one caller.
fn token_in(url: &str, parameter: &str) -> String {
    url.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{parameter}=")))
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_inquiry_write_emits_exactly_one_attributable_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing record: the
    // capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", akid = AUDIT_AKID, "probe");
    let probe = captured.drain_for(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    // The inquiry the feedback and export rows address, and a second one so the export rows
    // cannot be disturbed by the feedback rows' refusals.
    let rated = fixture_inquiry(&router, 1).await;
    let exported = fixture_inquiry(&router, 2).await;
    // The export rows report on an inquiry whose processing is OVER: an export of one that
    // is still being processed is refused, so the rows below wait for that condition. A
    // metadata read emits no audit record, so the wait leaves the capture alone.
    crate::harness::settled_inquiry_status(&router, "the exported fixture", &exported).await;

    // A token the create rows replay, and one the feedback rows replay. `IdempotentClientToken`
    // is `^[\w\-]+$`, len 1..128.
    let create_token = "audit-create-token";
    let feedback_token = "audit-feedback-token";
    let replayed_create = text_submission(3);
    let replayed_create_body = json!({
        "name": replayed_create["name"],
        "inquiryContent": replayed_create["inquiryContent"],
        "clientToken": create_token
    });
    let rated_feedback = json!({
        "complianceInquiryId": rated,
        "queryIdentifier": 1,
        "responseRevisionId": 0,
        "rating": RATINGS[0],
        "reasonCodes": [REASON_CODES[0], REASON_CODES[1]],
        "comment": sensitive(4),
        "clientToken": feedback_token
    });

    let cases = vec![
        Case {
            case: "a text submission",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(text_submission(10)),
            region: REGION,
            status: 202,
            outcome: "created",
            fields: vec!["input_source=TEXT".to_string()],
            names: Names::CreatedInquiry,
        },
        Case {
            case: "an uploaded document under the mode the model admits",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(json!({
                "name": sensitive(11),
                "inquiryContent": { "fileContent": { "content": FILE_CONTENT } },
                "supportMode": SUPPORT_MODE
            })),
            region: REGION,
            status: 202,
            outcome: "created",
            fields: vec!["input_source=FILE".to_string()],
            names: Names::CreatedInquiry,
        },
        Case {
            case: "a text submission signed for another region",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(text_submission(12)),
            region: OTHER_REGION,
            status: 202,
            outcome: "created",
            fields: vec!["input_source=TEXT".to_string()],
            names: Names::CreatedInquiry,
        },
        Case {
            case: "a create carrying an idempotency token",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(replayed_create_body.clone()),
            region: REGION,
            status: 202,
            outcome: "created",
            fields: vec!["input_source=TEXT".to_string()],
            names: Names::CreatedInquiry,
        },
        Case {
            case: "a repeat of that create, which creates nothing further",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(replayed_create_body),
            region: REGION,
            status: 202,
            outcome: "replayed",
            fields: vec!["input_source=TEXT".to_string()],
            names: Names::CreatedInquiry,
        },
        Case {
            case: "that token on different content",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(json!({
                "name": sensitive(13),
                "inquiryContent": { "query": sensitive(14) },
                "clientToken": create_token
            })),
            region: REGION,
            status: 409,
            outcome: "refused",
            fields: vec![
                "input_source=TEXT".to_string(),
                "reason=the clientToken was replayed on a different request".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "a create naming a tag key in the reserved namespace",
            operation: "CreateComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/create".to_string(),
            body: Some(json!({
                "name": sensitive(15),
                "inquiryContent": { "query": sensitive(16) },
                "tags": { "aws:created-by": "someone" }
            })),
            region: REGION,
            status: 400,
            outcome: "refused",
            fields: vec![
                "input_source=TEXT".to_string(),
                "reason=tags name a key in the reserved namespace".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "feedback on the inquiry's first query",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(rated_feedback.clone()),
            region: REGION,
            status: 200,
            outcome: "recorded",
            fields: vec![
                format!("compliance_inquiry_id={rated}"),
                "query_identifier=1".to_string(),
                "response_revision_id=0".to_string(),
                format!("rating={}", RATINGS[0]),
                "reason_code_count=2".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "a repeat of that submission, which records nothing further",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(rated_feedback),
            region: REGION,
            status: 200,
            outcome: "replayed",
            fields: vec![
                format!("compliance_inquiry_id={rated}"),
                "query_identifier=1".to_string(),
                "response_revision_id=0".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "a second submission against a response version that already carries \
                   feedback",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({
                "complianceInquiryId": rated,
                "queryIdentifier": 1,
                "responseRevisionId": 0,
                "rating": RATINGS[1],
                "reasonCodes": [REASON_CODES[2]]
            })),
            region: REGION,
            status: 409,
            outcome: "refused",
            fields: vec![
                format!("compliance_inquiry_id={rated}"),
                "query_identifier=1".to_string(),
                "response_revision_id=0".to_string(),
                "reason=the response version already carries feedback".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "that feedback token on another response version, which the model has the \
                   service ignore",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({
                "complianceInquiryId": rated,
                "queryIdentifier": 1,
                "responseRevisionId": 1,
                "rating": RATINGS[1],
                "clientToken": feedback_token
            })),
            region: REGION,
            status: 200,
            outcome: "ignored",
            fields: vec![
                format!("compliance_inquiry_id={rated}"),
                "query_identifier=1".to_string(),
                "response_revision_id=1".to_string(),
                "reason=the clientToken was recorded for another request".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "feedback naming a query the inquiry does not own",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({
                "complianceInquiryId": rated,
                "queryIdentifier": 99,
                "rating": RATINGS[0]
            })),
            region: REGION,
            status: 400,
            outcome: "refused",
            fields: vec![
                format!("compliance_inquiry_id={rated}"),
                "query_identifier=99".to_string(),
                "reason=queryIdentifier names a query the inquiry does not own".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "feedback on an inquiry id this account holds none for",
            operation: "PutComplianceInquiryFeedback",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({
                "complianceInquiryId": ABSENT_INQUIRY,
                "rating": RATINGS[0]
            })),
            region: REGION,
            status: 401,
            outcome: "refused",
            fields: vec![
                format!("compliance_inquiry_id={ABSENT_INQUIRY}"),
                "reason=complianceInquiryId names no inquiry this account holds".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "an export of every query the inquiry owns, with citations",
            operation: "ExportComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({
                "complianceInquiryId": exported,
                "includeCitations": true
            })),
            region: REGION,
            status: 200,
            outcome: "exported",
            fields: vec![
                format!("compliance_inquiry_id={exported}"),
                "include_citations=true".to_string(),
            ],
            names: Names::MintedExport,
        },
        Case {
            case: "a second export of the same inquiry, signed for another region",
            operation: "ExportComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({
                "complianceInquiryId": exported,
                "queryIdentifiers": [1],
                "includeCitations": false
            })),
            region: OTHER_REGION,
            status: 200,
            outcome: "exported",
            fields: vec![
                format!("compliance_inquiry_id={exported}"),
                "include_citations=false".to_string(),
            ],
            names: Names::MintedExport,
        },
        Case {
            case: "an export naming a query the inquiry does not own",
            operation: "ExportComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({
                "complianceInquiryId": exported,
                "queryIdentifiers": [99]
            })),
            region: REGION,
            status: 400,
            outcome: "refused",
            fields: vec![
                format!("compliance_inquiry_id={exported}"),
                "reason=queryIdentifiers name a query the inquiry does not own".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "an export of an inquiry id this account holds none for",
            operation: "ExportComplianceInquiry",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({ "complianceInquiryId": ABSENT_INQUIRY })),
            region: REGION,
            status: 401,
            outcome: "refused",
            fields: vec![
                format!("compliance_inquiry_id={ABSENT_INQUIRY}"),
                "reason=complianceInquiryId names no inquiry this account holds".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "a term accepted for a published report version",
            operation: "GetTermForReport",
            method: "GET",
            uri: format!(
                "/v1/report/getTermForReport?reportId={GATED_REPORT}&reportVersion={GATED_VERSION}"
            ),
            body: None,
            region: REGION,
            status: 200,
            outcome: "issued",
            fields: vec![
                format!("report_id={GATED_REPORT}"),
                format!("report_version={GATED_VERSION}"),
            ],
            names: Names::MintedTermGrant,
        },
        Case {
            case: "a term accepted without naming a version, signed for another region",
            operation: "GetTermForReport",
            method: "GET",
            uri: format!("/v1/report/getTermForReport?reportId={GATED_REPORT}"),
            body: None,
            region: OTHER_REGION,
            status: 200,
            outcome: "issued",
            fields: vec![format!("report_id={GATED_REPORT}")],
            names: Names::MintedTermGrant,
        },
        Case {
            case: "a term for a report the catalog does not hold",
            operation: "GetTermForReport",
            method: "GET",
            uri: format!("/v1/report/getTermForReport?reportId={ABSENT_REPORT}"),
            body: None,
            region: REGION,
            status: 401,
            outcome: "refused",
            fields: vec![
                format!("report_id={ABSENT_REPORT}"),
                "reason=reportId names no report this account holds".to_string(),
            ],
            names: Names::Nothing,
        },
        Case {
            case: "a term for a version the report never published",
            operation: "GetTermForReport",
            method: "GET",
            uri: format!(
                "/v1/report/getTermForReport?reportId={GATED_REPORT}\
                 &reportVersion={UNPUBLISHED_VERSION}"
            ),
            body: None,
            region: REGION,
            status: 404,
            outcome: "refused",
            fields: vec![
                format!("report_id={GATED_REPORT}"),
                format!("report_version={UNPUBLISHED_VERSION}"),
                "reason=the report never published the version asked for".to_string(),
            ],
            names: Names::Nothing,
        },
    ];

    // The fixtures' own records are this suite's too, so they are cleared once here: from now
    // on the capture holds exactly what the row under test left.
    let _ = captured.drain_for(AUDIT_AKID);

    for expected in cases {
        let case = expected.case;
        let answer = call(
            &router,
            expected.region,
            expected.method,
            &expected.uri,
            expected.body.as_ref(),
        )
        .await;
        assert_eq!(
            answer.status, expected.status,
            "case {case:?}: the call must answer HTTP {}, got {} with {:?}",
            expected.status, answer.status, answer.body
        );

        let records = captured.drain_for(AUDIT_AKID);
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: one state-changing call must leave exactly one audit record, got \
             {records:?}"
        );
        let record = &records[0];

        // Asserted as the WHOLE `field=value` pair ([`carries`]), so a value can satisfy the
        // check neither from the wrong field nor as the prefix of a longer value.
        let mut pairs = vec![
            format!("operation={}", expected.operation),
            format!("akid={AUDIT_AKID}"),
            format!("region={}", expected.region),
            format!("outcome={}", expected.outcome),
        ];
        pairs.extend(expected.fields.iter().cloned());

        let url = answer.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut forbidden: Vec<String> = Vec::new();
        match expected.names {
            Names::Nothing => {}
            Names::CreatedInquiry => {
                let id = answer.body["complianceInquirySummary"]["id"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    !id.is_empty(),
                    "case {case:?}: the answer must report an inquiry id, got {:?}",
                    answer.body
                );
                pairs.push(format!("compliance_inquiry_id={id}"));
            }
            Names::MintedExport => {
                assert!(
                    !url.is_empty(),
                    "case {case:?}: the answer must announce a document URL, got {:?}",
                    answer.body
                );
                let export_id = export_id_in(&url);
                assert!(
                    !export_id.is_empty(),
                    "case {case:?}: the announced URL must name the export, got {url:?}"
                );
                pairs.push(format!("export_id={export_id}"));
                // The URL and the download token it carries are the receipt handed to ONE
                // caller: a record that repeated either would hand it to every log reader.
                forbidden.push(url.clone());
                forbidden.push(token_in(&url, "exportToken"));
            }
            Names::MintedTermGrant => {
                let token = answer.body["termToken"].as_str().unwrap_or_default();
                assert!(
                    !token.is_empty(),
                    "case {case:?}: the acceptance must answer a termToken, got {:?}",
                    answer.body
                );
                forbidden.push(token.to_string());
                forbidden.push(url.clone());
            }
        }

        for pair in &pairs {
            assert!(
                carries(record, pair),
                "case {case:?}: the audit record must carry {pair:?}, got {record:?}"
            );
        }

        // No record carries caller content: the inquiry's `@sensitive` name, the query text
        // inside its content, or the feedback comment.
        assert!(
            !record.contains(SENSITIVE_MARKER),
            "case {case:?}: the audit record must carry no sensitive request value, got \
             {record:?}"
        );
        for secret in &forbidden {
            assert!(
                !secret.is_empty(),
                "case {case:?}: the answer must carry the value this row forbids in the record"
            );
            assert!(
                !record.contains(secret.as_str()),
                "case {case:?}: the audit record must carry neither the announced URL nor its \
                 token, got {record:?}"
            );
        }

        // One record is one line.
        assert!(
            !record.contains('\n') && !record.contains('\r'),
            "case {case:?}: the audit record must be one line, got {record:?}"
        );
    }
}
