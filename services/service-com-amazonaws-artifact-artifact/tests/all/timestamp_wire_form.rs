// SPDX-License-Identifier: CC0-1.0

//! The wire form of every timestamp this service reports, and the terminal state a
//! compliance inquiry reaches.
//!
//! Two rules are under test, both read off the Smithy model rather than off any one
//! recorded response:
//!
//! 1. A member whose target carries `@timestampFormat("date-time")` is serialized on
//!    restJson1 as an RFC 3339 / ISO-8601 date-time STRING — never as restJson1's default
//!    epoch-seconds number. In this service that is EVERY timestamp member: all of them
//!    target `com.amazonaws.artifact#TimestampAttribute`, which the model marks
//!    `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape",shape="TimestampAttribute")`). The catalog of members
//!    below comes from `smithy_query(query="shape",shape="InquirySummary,InquiryDetail,
//!    QuerySummary,ResponseVersion,ReportSummary,ReportDetail,CustomerAgreementSummary,
//!    PutComplianceInquiryFeedbackResponse")` — the shapes of this service that own one.
//! 2. A compliance inquiry's terminal `status`/`statusMessage` are DERIVED from its
//!    queries: an inquiry whose queries all reached their terminal state without failing
//!    completes with `InquiryStatusMessage`'s "Compliance inquiry processing is complete.",
//!    the "One or more queries encountered errors during processing." value is reported
//!    only when a query failed, and an inquiry whose submitted content carried nothing to
//!    process — so it owns no query at all — ends `FAILED`. A query reports the answer it
//!    holds — its `response` and the `citations` supporting it, always together — once its
//!    processing is over, and reports neither while no source of the published catalog
//!    bears on it, which is not a query-level error.
//!
//! Rule 1 is checked end to end over HTTP across the operations below that report a
//! timestamp (so the rows differ along operation, shape, member, whether the member is
//! `@required`, and whether the instant has a sub-second part: a created inquiry's
//! `createdAt` comes from the clock, a catalog period is a whole second), and directly on
//! the CODEC this service serves, which is the only reach a shape no operation of this
//! service reports (`ResponseVersion`) has.
//! `PutComplianceInquiryFeedbackResponse$submittedAt` is
//! reported over HTTP by `PutComplianceInquiryFeedback`, and its end-to-end row lives in
//! `implemented_inquiry_operations.rs`, beside that operation's other rows.
//!
//! The date-time is rendered in the CANONICAL form of the instant it names: RFC 3339 with
//! a fractional-seconds component only where the instant has one, so an instant on the
//! second closes straight at `Z` (`2024-11-01T00:00:00Z`, as a recorded catalog entry
//! shows) while an instant off it reports the fraction it holds. That is a property of the
//! service's rendering, not of any one member, so every row below asserts it, and
//! [`a_date_time_carries_a_fraction_only_where_the_instant_has_one`] asserts it over a
//! table of instants spanning both.

use std::sync::Arc;

use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::protocol::ServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::ops::types::{
    CustomerAgreementSummary, InquiryDetail, InquirySummary, PutComplianceInquiryFeedbackOutput,
    QuerySummary, ReportDetail, ReportSummary, ResponseVersion,
};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use service_common::date_time_json::{date_time_text, DateTimeRestJson};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{assert_rendered_date_time, settled_inquiry_status};

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// `InquiryStatusMessage`'s value for an inquiry that completed with nothing failing.
const INQUIRY_COMPLETE: &str = "Compliance inquiry processing is complete.";

/// `InquiryStatusMessage`'s value for an inquiry that completed with a failed query.
const INQUIRY_COMPLETE_WITH_QUERY_ERRORS: &str = "Compliance inquiry processing is complete. \
     One or more queries encountered errors during processing.";

/// `InquiryStatusMessage`'s value for an inquiry whose processing itself failed.
const INQUIRY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED)
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, uri, request).await
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, uri, request).await
}

async fn send(
    router: &MultiServiceRouter,
    uri: &str,
    request: ::http::Request<Bytes>,
) -> Value {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "{uri} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    assert!(
        (200..300).contains(&status),
        "{uri} must succeed for this suite to say anything, got {status} with {body:?}"
    );
    body
}

/// A `CreateComplianceInquiry` body carrying one submitted query.
fn text_body(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query }, "supportMode": "AI_ONLY" })
}

/// A `CreateComplianceInquiry` body carrying an uploaded document of `content` bytes.
fn file_body(name: &str, content: &str) -> Value {
    json!({
        "name": name,
        "inquiryContent": { "fileContent": { "content": content } },
    })
}

/// The member at `pointer`, or `Value::Null` when the body carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Assert one `TimestampAttribute` member's wire form — the harness's one rule, an RFC
/// 3339 UTC string in the canonical rendering of the instant it names — and, beyond it,
/// that the member names a real instant.
///
/// Parsed rather than matched against a literal: the instant itself is
/// environment-specific (a created inquiry's `createdAt` is the clock's), so what is under
/// test is the JSON type, the lexical form and the precision, not the value.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let parsed = assert_rendered_date_time(case, member, value);
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {value:?}"
    );
}

#[tokio::test]
async fn every_reported_date_time_member_is_an_rfc3339_string() {
    let router = emulator();

    // The compliance-inquiry projections, read BEFORE the processing span is over (the
    // inquiry is still `PROCESSING`, so `updatedAt` is its creation instant) and again
    // after it, where the clock-owed transition moved `updatedAt` on. Both instants carry
    // a sub-second part, which the catalog's whole-second periods do not.
    let created = post(
        &router,
        "/v1/compliance-inquiry/create",
        &text_body("Data residency", "Where is customer data stored?"),
    )
    .await;
    let inquiry_id = at(&created, "/complianceInquirySummary/id")
        .as_str()
        .expect("the create reports the minted inquiry id")
        .to_string();
    let pending = get(
        &router,
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
    )
    .await;
    let listed_inquiries = get(&router, "/v1/compliance-inquiry/list").await;
    let pending_queries = get(
        &router,
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}"),
    )
    .await;

    // Waited out by POLLING for the condition — the handlers store the due instant rather
    // than sleeping, so the test looks again until the inquiry has left its non-terminal
    // status instead of resting for a fixed span.
    settled_inquiry_status(&router, "every reported date-time member", &inquiry_id).await;

    let settled = get(
        &router,
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
    )
    .await;
    let settled_queries = get(
        &router,
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}"),
    )
    .await;

    // The report catalog and the account's customer-agreements: whole-second instants,
    // optional members, and both the list and the single-record projection.
    let reports = get(&router, "/v1/report/list").await;
    let report_id = at(&reports, "/reports/0/id")
        .as_str()
        .expect("the catalog publishes at least one report")
        .to_string();
    let report_version = at(&reports, "/reports/0/version")
        .as_i64()
        .expect("a catalog entry reports its version");
    let latest_metadata = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report_id}"),
    )
    .await;
    let pinned_metadata = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report_id}&reportVersion={report_version}"),
    )
    .await;
    let versions = get(
        &router,
        &format!("/v1/report/listVersions?reportId={report_id}"),
    )
    .await;
    let agreements = get(&router, "/v1/customer-agreement/list").await;

    // (case, body, member pointer, present?) — one row per TimestampAttribute member each
    // implemented operation reports. A `@required` member must be there; an optional one is
    // only checked when the record carries it.
    let required: Vec<(&str, &Value, &str)> = vec![
        (
            "CreateComplianceInquiry/InquirySummary",
            &created,
            "/complianceInquirySummary/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, still processing",
            &pending,
            "/complianceInquiryDetail/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, still processing",
            &pending,
            "/complianceInquiryDetail/updatedAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, terminal",
            &settled,
            "/complianceInquiryDetail/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, terminal",
            &settled,
            "/complianceInquiryDetail/updatedAt",
        ),
        (
            "ListComplianceInquiries/InquirySummary",
            &listed_inquiries,
            "/complianceInquiries/0/createdAt",
        ),
        (
            "ListComplianceInquiryQueries/QuerySummary, still processing",
            &pending_queries,
            "/queries/0/createdAt",
        ),
        (
            "ListComplianceInquiryQueries/QuerySummary, terminal",
            &settled_queries,
            "/queries/0/createdAt",
        ),
    ];
    let optional: Vec<(&str, &Value, &str)> = vec![
        ("ListReports/ReportSummary", &reports, "/reports/0/periodStart"),
        ("ListReports/ReportSummary", &reports, "/reports/0/periodEnd"),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/periodStart",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/periodEnd",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/createdAt",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/lastModifiedAt",
        ),
        (
            "GetReportMetadata/ReportDetail, pinned version",
            &pinned_metadata,
            "/reportDetails/periodStart",
        ),
        (
            "GetReportMetadata/ReportDetail, pinned version",
            &pinned_metadata,
            "/reportDetails/createdAt",
        ),
        (
            "ListReportVersions/ReportSummary",
            &versions,
            "/reports/0/periodStart",
        ),
        (
            "ListReportVersions/ReportSummary",
            &versions,
            "/reports/0/periodEnd",
        ),
        (
            "ListCustomerAgreements/CustomerAgreementSummary",
            &agreements,
            "/customerAgreements/0/effectiveStart",
        ),
    ];

    for (case, body, pointer) in required {
        let value = at(body, pointer);
        assert!(
            !value.is_null(),
            "case {case:?}: {pointer} is @required, so the answer must carry it, got {body:?}"
        );
        assert_date_time(case, pointer, &value);
    }
    for (case, body, pointer) in optional {
        let value = at(body, pointer);
        if value.is_null() {
            continue;
        }
        assert_date_time(case, pointer, &value);
    }

    // A deleted report carries `deletedAt`; the catalog holds none, so the member is
    // absent rather than an epoch-zero instant.
    assert!(
        at(&latest_metadata, "/reportDetails/deletedAt").is_null(),
        "a report in the catalog is not deleted, so it reports no deletedAt, got {:?}",
        at(&latest_metadata, "/reportDetails/deletedAt")
    );
}

/// The instant the codec rows below report, on the second, and its date-time rendering.
const CODEC_INSTANT: i64 = 1_730_419_200;

/// One shape serialized by the codec this service serves, as JSON.
fn codec_body(case: &str, protocol: &DateTimeRestJson, value: &dyn SerializableStruct, schema: &Schema) -> Value {
    let answered = protocol
        .serialize_output(value, schema, "TimestampWireForm", 200)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the codec must serialize this shape ({error})")
        });
    let bytes = answered
        .body()
        .bytes()
        .expect("the codec answers an in-memory body")
        .to_vec();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the codec must answer JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

/// The CODEC this service serves writes a timestamp member as a date-time STRING, whichever
/// shape carries it: `service_common::date_time_json::DateTimeRestJson`, which
/// `src/service.rs` hands the dispatch sequence through `Service::override_protocol`,
/// supplies the `date-time` form the model declares and the generated member schemas drop.
///
/// Driven on the codec itself, which is the only reach `ResponseVersion` has — the one shape
/// here no operation of this service reports, an edited response being the only thing that
/// carries one — while the rows above assert the same rule end to end over HTTP. The rows
/// differ along what the rule generalises over: the shape, the member, whether the member is
/// `@required` or optional-and-set, and whether the instant falls on the second.
#[test]
fn the_served_codec_writes_every_timestamp_member_as_a_date_time_string() {
    let protocol = DateTimeRestJson::new();
    let on_second = DateTime::from_secs(CODEC_INSTANT);
    let off_second = DateTime::from_secs_f64(CODEC_INSTANT as f64 + 0.25);

    // Source: `smithy_query(query="shape",shape="InquirySummary,InquiryDetail,QuerySummary,
    // ResponseVersion,ReportSummary,ReportDetail,CustomerAgreementSummary,
    // PutComplianceInquiryFeedbackResponse")` — every member of this service targeting
    // `TimestampAttribute`.
    let inquiry_summary = InquirySummary {
        arn: "arn:aws:artifact:::compliance-inquiry/compliance-inquiry-0000000000000001"
            .to_string(),
        name: "Data residency".to_string(),
        id: "compliance-inquiry-0000000000000001".to_string(),
        status: "PROCESSING".to_string(),
        status_message: "Compliance inquiry processing is in-progress.".to_string(),
        input_source: "TEXT".to_string(),
        created_at: off_second,
    };
    let inquiry_detail = InquiryDetail {
        arn: inquiry_summary.arn.clone(),
        name: inquiry_summary.name.clone(),
        id: inquiry_summary.id.clone(),
        status: inquiry_summary.status.clone(),
        status_message: inquiry_summary.status_message.clone(),
        input_source: inquiry_summary.input_source.clone(),
        created_at: off_second,
        updated_at: Some(on_second),
        support_mode: Some("AI_ONLY".to_string()),
    };
    let query_summary = QuerySummary {
        query_identifier: 1,
        query: "Where is customer data stored?".to_string(),
        response: None,
        review_type: None,
        citations: None,
        status: "PROCESSING".to_string(),
        status_message: "Query processing is in-progress.".to_string(),
        created_at: off_second,
        updated_response_versions: None,
    };
    let response_version = ResponseVersion {
        response_text: "The edited response.".to_string(),
        timestamp: on_second,
    };
    let report_summary = ReportSummary {
        id: Some("report-0000000000000001".to_string()),
        version: Some(1),
        period_start: Some(on_second),
        period_end: Some(on_second),
        ..ReportSummary::default()
    };
    let report_detail = ReportDetail {
        id: report_summary.id.clone(),
        version: Some(1),
        period_start: Some(on_second),
        period_end: Some(on_second),
        created_at: Some(on_second),
        last_modified_at: Some(off_second),
        deleted_at: Some(on_second),
        ..ReportDetail::default()
    };
    let agreement = CustomerAgreementSummary {
        id: Some("customer-agreement-0000000000000001".to_string()),
        effective_start: Some(on_second),
        effective_end: Some(on_second),
        ..CustomerAgreementSummary::default()
    };
    let feedback = PutComplianceInquiryFeedbackOutput {
        submitted_at: off_second,
    };

    let rows: Vec<(&str, Value, &[&str])> = vec![
        (
            "InquirySummary",
            codec_body(
                "InquirySummary",
                &protocol,
                &inquiry_summary,
                &InquirySummary::SCHEMA,
            ),
            &["createdAt"],
        ),
        (
            "InquiryDetail",
            codec_body(
                "InquiryDetail",
                &protocol,
                &inquiry_detail,
                &InquiryDetail::SCHEMA,
            ),
            &["createdAt", "updatedAt"],
        ),
        (
            "QuerySummary",
            codec_body(
                "QuerySummary",
                &protocol,
                &query_summary,
                &QuerySummary::SCHEMA,
            ),
            &["createdAt"],
        ),
        (
            "ResponseVersion",
            codec_body(
                "ResponseVersion",
                &protocol,
                &response_version,
                &ResponseVersion::SCHEMA,
            ),
            &["timestamp"],
        ),
        (
            "ReportSummary",
            codec_body(
                "ReportSummary",
                &protocol,
                &report_summary,
                &ReportSummary::SCHEMA,
            ),
            &["periodStart", "periodEnd"],
        ),
        (
            "ReportDetail",
            codec_body(
                "ReportDetail",
                &protocol,
                &report_detail,
                &ReportDetail::SCHEMA,
            ),
            &[
                "periodStart",
                "periodEnd",
                "createdAt",
                "lastModifiedAt",
                "deletedAt",
            ],
        ),
        (
            "CustomerAgreementSummary",
            codec_body(
                "CustomerAgreementSummary",
                &protocol,
                &agreement,
                &CustomerAgreementSummary::SCHEMA,
            ),
            &["effectiveStart", "effectiveEnd"],
        ),
        (
            "PutComplianceInquiryFeedbackResponse",
            codec_body(
                "PutComplianceInquiryFeedbackResponse",
                &protocol,
                &feedback,
                &PutComplianceInquiryFeedbackOutput::SCHEMA,
            ),
            &["submittedAt"],
        ),
    ];

    for (shape, body, members) in rows {
        for member in members {
            let case = format!("{shape}${member}");
            let reported = at(&body, &format!("/{member}"));
            assert_rendered_date_time(&case, member, &reported);
        }
    }

    // A member that is not a timestamp keeps the type the protocol gives it, so the codec
    // rewrites nothing the model did not declare a date-time.
    let untouched = codec_body(
        "ReportSummary",
        &protocol,
        &report_summary,
        &ReportSummary::SCHEMA,
    );
    assert!(
        at(&untouched, "/version").is_i64(),
        "case \"ReportSummary$version\": a long member stays a JSON number, got {:?}",
        at(&untouched, "/version")
    );
    assert!(
        at(&untouched, "/id").is_string(),
        "case \"ReportSummary$id\": a string member stays a JSON string, got {:?}",
        at(&untouched, "/id")
    );
}

/// An inquiry's terminal `status`/`statusMessage` are derived from its queries, so the
/// modelled clean-completion value is what a successfully processed inquiry reports.
///
/// The rows differ along what the rule generalises over: the form of the submitted
/// content (a single submitted query versus an uploaded document), whether the content
/// carries anything to process at all, and the submitted text.
#[tokio::test]
async fn an_inquiry_completes_cleanly_when_no_query_of_it_failed() {
    let cases: Vec<(&str, Value, &str, &str, usize)> = vec![
        (
            "one submitted query",
            text_body("Data residency", "Where is customer data stored?"),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "one submitted query, other text",
            text_body("Subprocessors", "Which subprocessors handle customer content?"),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "an uploaded document processed as one unit",
            file_body("Vendor questionnaire", FILE_CONTENT),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "an upload carrying no bytes, so nothing to process",
            file_body("Empty questionnaire", ""),
            "FAILED",
            INQUIRY_INTERNAL_ERROR,
            0,
        ),
    ];

    for (case, body, terminal_status, terminal_message, query_count) in cases {
        let router = emulator();
        let created = post(&router, "/v1/compliance-inquiry/create", &body).await;
        let id = at(&created, "/complianceInquirySummary/id")
            .as_str()
            .expect("the create reports the minted inquiry id")
            .to_string();

        // The same poll as above: wait for the inquiry to have left its non-terminal
        // status, not for a fixed span.
        settled_inquiry_status(&router, case, &id).await;

        let detail = get(
            &router,
            &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        )
        .await;
        assert_eq!(
            (
                at(&detail, "/complianceInquiryDetail/status"),
                at(&detail, "/complianceInquiryDetail/statusMessage"),
            ),
            (json!(terminal_status), json!(terminal_message)),
            "case {case:?}: expected the terminal pair {terminal_status:?} / \
             {terminal_message:?}, got {detail:?}"
        );
        assert_ne!(
            at(&detail, "/complianceInquiryDetail/statusMessage"),
            json!(INQUIRY_COMPLETE_WITH_QUERY_ERRORS),
            "case {case:?}: no query of this inquiry failed, so the query-error value must \
             not be reported, got {detail:?}"
        );

        let queries = get(
            &router,
            &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
        )
        .await;
        let reported = at(&queries, "/queries");
        let reported = reported.as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            query_count,
            "case {case:?}: expected {query_count} queries, got {queries:?}"
        );
        for (position, query) in reported.iter().enumerate() {
            assert_eq!(
                (query["status"].clone(), query["statusMessage"].clone()),
                (
                    json!("COMPLETED"),
                    json!("Query processing is complete.")
                ),
                "case {case:?}: query {position} is reviewed by the service's own model, so \
                 its processing is over without error, got {query:?}"
            );
            assert_eq!(
                query["response"].is_null(),
                query["citations"].is_null(),
                "case {case:?}: query {position} reports a response and the citations \
                 supporting it together, got {query:?}"
            );
        }
    }
}

/// One instant that falls ON the second, as text, and the `date-time` this service renders
/// it as.
///
/// Source for the form: every timestamp member of this service targets
/// `com.amazonaws.artifact#TimestampAttribute`, which the model marks
/// `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="TimestampAttribute")`). A `date-time` is RFC 3339,
/// whose fractional-seconds component is OPTIONAL and reports the precision the instant
/// has — so an instant on the second is rendered without one, which is the form the
/// recorded catalog entries of this service carry (`2024-11-01T00:00:00Z`).
///
/// Each row names its instant as text, which is parsed to the instant under test, so no row
/// carries an epoch arithmetic of its own. The rows differ along what the rule generalises
/// over: how wide a fraction the instant was WRITTEN with (a zero fraction is a whole second
/// written long, and renders short), the second within the minute, the date, and how far
/// from the epoch the instant lies.
const WHOLE_SECONDS: &[(&str, &str, &str)] = &[
    (
        "the Unix epoch",
        "1970-01-01T00:00:00Z",
        "1970-01-01T00:00:00Z",
    ),
    (
        "a recorded catalog period start",
        "2024-11-01T00:00:00Z",
        "2024-11-01T00:00:00Z",
    ),
    (
        "the same instant written with a zero millisecond fraction",
        "2024-11-01T00:00:00.000Z",
        "2024-11-01T00:00:00Z",
    ),
    (
        "a recorded catalog period end",
        "2025-10-31T00:00:00Z",
        "2025-10-31T00:00:00Z",
    ),
    (
        "a second inside the minute",
        "2026-03-04T05:06:07Z",
        "2026-03-04T05:06:07Z",
    ),
    (
        "the same instant written with a nine-digit zero fraction",
        "2026-03-04T05:06:07.000000000Z",
        "2026-03-04T05:06:07Z",
    ),
    (
        "the last second of a leap day",
        "2024-02-29T23:59:59Z",
        "2024-02-29T23:59:59Z",
    ),
];

/// Instants that fall OFF the second, as text: the rendering reports the precision they
/// hold, so each is asserted by round trip rather than against a fraction width this
/// service would be free to choose.
const SUB_SECOND_INSTANTS: &[(&str, &str)] = &[
    ("a millisecond part", "2026-03-04T05:06:07.120Z"),
    ("a single millisecond", "2026-03-04T05:06:07.001Z"),
    ("a microsecond part", "2026-03-04T05:06:07.123456Z"),
];

/// The instant `text` names.
fn instant(case: &str, text: &str) -> DateTime {
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: the row's instant {text:?} must parse as a date-time ({error})")
    })
}

/// The one date-time renderer of this service, over a table of instants: a fraction is
/// reported exactly where the instant has one.
#[test]
fn a_date_time_carries_a_fraction_only_where_the_instant_has_one() {
    for (case, written, expected) in WHOLE_SECONDS {
        let rendered = date_time_text(&instant(case, written))
            .unwrap_or_else(|| panic!("case {case:?}: {written:?} must render as a date-time"));
        assert_eq!(
            &rendered, expected,
            "case {case:?}: an instant on the second is rendered without a fractional-seconds \
             component, expected {expected:?}, got {rendered:?}"
        );
    }

    for (case, written) in SUB_SECOND_INSTANTS {
        let named = instant(case, written);
        let rendered = date_time_text(&named)
            .unwrap_or_else(|| panic!("case {case:?}: {written:?} must render as a date-time"));
        assert!(
            rendered.contains('.'),
            "case {case:?}: this instant has a sub-second part, so the rendering reports one, \
             got {rendered:?}"
        );
        assert_eq!(
            instant(case, &rendered),
            named,
            "case {case:?}: the rendering must name the same instant, got {rendered:?}"
        );
    }
}

/// The same rule end to end, over every operation of this service that reports an instant
/// the stored record holds ON the second: the published catalog's reporting periods and
/// publication instants, and the account's customer-agreement effective dates. Each of
/// those is a calendar instant (see this service's period constants in `src/state.rs`), so
/// none of them is reported with a fractional-seconds component.
#[tokio::test]
async fn a_catalog_instant_is_reported_without_a_fractional_component() {
    let router = emulator();

    let reports = get(&router, "/v1/report/list").await;
    let report_id = at(&reports, "/reports/0/id")
        .as_str()
        .expect("the catalog publishes at least one report")
        .to_string();
    let versions = get(
        &router,
        &format!("/v1/report/listVersions?reportId={report_id}"),
    )
    .await;
    let metadata = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report_id}"),
    )
    .await;
    let agreements = get(&router, "/v1/customer-agreement/list").await;

    // (case, the list of records, the members of a record that name a stored instant).
    let listed: Vec<(&str, &Value, &str, &[&str])> = vec![
        (
            "ListReports/ReportSummary",
            &reports,
            "reports",
            &["periodStart", "periodEnd"],
        ),
        (
            "ListReportVersions/ReportSummary",
            &versions,
            "reports",
            &["periodStart", "periodEnd"],
        ),
        (
            "ListCustomerAgreements/CustomerAgreementSummary",
            &agreements,
            "customerAgreements",
            &["effectiveStart", "effectiveEnd"],
        ),
    ];

    let mut asserted = 0usize;
    for (case, body, list, members) in listed {
        let records = body[list].as_array().cloned().unwrap_or_default();
        assert!(
            !records.is_empty(),
            "case {case:?}: the answer must report records for this rule to say anything, got \
             {body:?}"
        );
        for (position, record) in records.iter().enumerate() {
            for member in members {
                let value = &record[*member];
                if value.is_null() {
                    continue;
                }
                let row = format!("{case} record {position}");
                assert_rendered_date_time(&row, member, value);
                let text = value.as_str().unwrap_or_default();
                assert!(
                    !text.contains('.'),
                    "case {row:?}: {member} names a stored calendar instant, so it is reported \
                     without a fractional-seconds component, got {text:?}"
                );
                asserted += 1;
            }
        }
    }

    for member in ["periodStart", "periodEnd", "createdAt", "lastModifiedAt"] {
        let value = at(&metadata, &format!("/reportDetails/{member}"));
        if value.is_null() {
            continue;
        }
        let row = format!("GetReportMetadata/ReportDetail {report_id}");
        assert_rendered_date_time(&row, member, &value);
        let text = value.as_str().unwrap_or_default().to_string();
        assert!(
            !text.contains('.'),
            "case {row:?}: {member} names a stored calendar instant, so it is reported without \
             a fractional-seconds component, got {text:?}"
        );
        asserted += 1;
    }

    assert!(
        asserted > 0,
        "the rule must be asserted over at least one reported instant, asserted {asserted}"
    );
}
