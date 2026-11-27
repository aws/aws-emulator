// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every compliance-inquiry-targeting operation resolves its target by,
//! asserted over all four of them at once.
//!
//! A request naming a `complianceInquiryId` is resolved in one place, and what a caller
//! sees never depends on which operation asked:
//!
//! 1. the stored inquiry set IS the account's entitlement, so a well-formed `InquiryId`
//!    the account holds no inquiry for names no inquiry it may reach and is answered with
//!    the front door's 401 `UnauthorizedException` — the identical refusal, body included,
//!    whichever operation asked and whatever else the request carried;
//! 2. the inquiry the account DOES hold is served with the transition the clock already
//!    owed it applied, so the record every one of the four operations resolves past the
//!    due instant is the same one — and the instant it reports as last modified is the
//!    instant the transition fell DUE, not the instant that reader happened to call.
//!
//! The rows span what the rule generalises over: which of the four operations drives the
//! resolution (the identifier is bound to the query string on two of them and to the
//! request body on the other two, and two of them go on to record against the inquiry),
//! the form of the submitted content (a typed query, an uploaded document, an upload
//! carrying no bytes at all), whether the submission named a support mode (`AI_ONLY` is the
//! one the model admits) and therefore the terminal status the materialised record reports
//! (`COMPLETED`, `FAILED`), and — for the refusal — the alphabet and case
//! of the 16-character suffix the identifier pattern admits.
//!
//! Every case goes through the router, so each one exercises the same path a client takes:
//! routing, the front door, deserialization, the handler, the persisted state and
//! serialization. `ExportComplianceInquiry` is read through the document its own answer
//! announces, which the handler renders off the record it resolved, so what that operation
//! resolved is observable in its own answer rather than in a later read.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, follow_announced_url, get, Answer};

/// The front door's authorization refusal: the status, the wire error code a restJson1
/// client resolves the shape from, and the gateway's own constant text, which names
/// neither the member nor the resource.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio without
/// its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// [`PAST_THE_PROCESSING_SPAN`] in seconds — the upper bound on how far past an inquiry's
/// creation the transition it owed can have fallen due, since the driving call is made
/// only after this much has elapsed.
const SLEPT_SECONDS: f64 = 0.9;

/// `InquiryStatusMessage`'s values, spelled out rather than imported from the crate so a
/// changed constant fails this suite instead of redefining it
/// (`smithy_query(query="shape",shape="InquiryStatusMessage")`).
const INQUIRY_COMPLETE: &str = "Compliance inquiry processing is complete.";
/// See [`INQUIRY_COMPLETE`].
const INQUIRY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// `QueryStatusMessage`'s value, from the same model
/// (`smithy_query(query="shape",shape="QueryStatusMessage")`).
const QUERY_COMPLETE: &str = "Query processing is complete.";

/// One rating `FeedbackRating` admits
/// (`smithy_query(query="shape",shape="FeedbackRating")`).
const RATING: &str = "THUMBS_UP";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const UPLOAD: &str = "cXVlc3Rpb25uYWlyZQ==";

/// An upload carrying NO bytes: `InquiryFileContent$content` is `@required` and the model
/// bounds its length in no other way, so this is a well-formed submission with nothing to
/// process.
const EMPTY_UPLOAD: &str = "";

/// Well-formed inquiry ids the account holds no inquiry for — targets judged against this
/// service's STATE rather than against the identifier syntax. Spelled out from
/// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`
/// (`smithy_query(query="shape",shape="InquiryId")`) rather than taken from the crate, so a
/// wrong predicate in `src/state.rs` cannot make this suite pass. The rows differ along
/// what the 16-character suffix generalises over: its alphabet and its case.
const UNHELD_INQUIRY_IDS: &[(&str, &str)] = &[
    ("all digits", "compliance-inquiry-0000000000000000"),
    ("all lowercase", "compliance-inquiry-abcdefghijklmnop"),
    ("all uppercase", "compliance-inquiry-ABCDEFGHIJKLMNOP"),
    ("mixed case and digits", "compliance-inquiry-AbCdEf0123456789"),
    ("leading and trailing digits", "compliance-inquiry-0aBcDeFgHiJkLmN9"),
];

/// Which of the four operations drives the target resolution in a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driver {
    /// `GetComplianceInquiryMetadata` — reports the inquiry's own detail.
    Metadata,
    /// `ListComplianceInquiryQueries` — reports the inquiry's queries.
    Queries,
    /// `ExportComplianceInquiry` — renders a document off the record it resolved.
    Export,
    /// `PutComplianceInquiryFeedback` — records against the record it resolved.
    Feedback,
}

/// Every operation that resolves ONE inquiry by `complianceInquiryId`.
const DRIVERS: [Driver; 4] = [
    Driver::Metadata,
    Driver::Queries,
    Driver::Export,
    Driver::Feedback,
];

impl Driver {
    /// The modelled operation this driver calls.
    fn operation(self) -> &'static str {
        match self {
            Self::Metadata => "GetComplianceInquiryMetadata",
            Self::Queries => "ListComplianceInquiryQueries",
            Self::Export => "ExportComplianceInquiry",
            Self::Feedback => "PutComplianceInquiryFeedback",
        }
    }

    /// One request on this operation targeting `id`, at the operation's modelled URI.
    async fn drive(self, router: &MultiServiceRouter, id: &str) -> Answer {
        match self {
            Self::Metadata => {
                get(
                    router,
                    &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
                )
                .await
            }
            Self::Queries => {
                get(
                    router,
                    &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
                )
                .await
            }
            Self::Export => {
                call(
                    router,
                    "POST",
                    "/v1/compliance-inquiry/export",
                    json_body(&json!({ "complianceInquiryId": id })),
                )
                .await
            }
            Self::Feedback => {
                call(
                    router,
                    "PUT",
                    "/v1/compliance-inquiry/putFeedback",
                    json_body(&json!({ "complianceInquiryId": id, "rating": RATING })),
                )
                .await
            }
        }
    }
}

/// One submission, and what the record every driver resolves past the due instant must
/// report.
struct Submission {
    label: &'static str,
    body: Value,
    /// The `InquiryStatus`/`InquiryStatusMessage` the inquiry rests in.
    terminal: (&'static str, &'static str),
    /// How many queries the inquiry reports: one unit of work per section an upload
    /// names, the document as a whole under its title when it names none, the one query a
    /// text submission carries — and none at all for a submission with nothing to process.
    reported_queries: usize,
    /// The `QueryStatus`/`QueryStatusMessage` each query it reports rests in.
    query_terminal: (&'static str, &'static str),
}

/// The submissions this suite drives every operation against.
fn submissions() -> Vec<Submission> {
    let completed = ("COMPLETED", QUERY_COMPLETE);
    vec![
        Submission {
            label: "a typed query, no support mode named",
            body: json!({
                "name": "Data residency",
                "inquiryContent": { "query": "Where is customer data stored?" },
            }),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported_queries: 1,
            query_terminal: completed,
        },
        Submission {
            label: "an uploaded document under AI_ONLY",
            body: json!({
                "name": "Vendor questionnaire",
                "inquiryContent": { "fileContent": { "content": UPLOAD } },
                "supportMode": "AI_ONLY",
            }),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported_queries: 1,
            query_terminal: completed,
        },
        Submission {
            label: "an uploaded document naming one section",
            body: json!({
                "name": "Controls walkthrough",
                "inquiryContent": {
                    "fileContent": { "content": UPLOAD, "fileSections": ["Controls 1-20"] },
                },
                "supportMode": "AI_ONLY",
            }),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported_queries: 1,
            query_terminal: completed,
        },
        Submission {
            label: "an upload carrying no bytes, so nothing to process",
            body: json!({
                "name": "Empty workbook",
                "inquiryContent": { "fileContent": { "content": EMPTY_UPLOAD } },
            }),
            terminal: ("FAILED", INQUIRY_INTERNAL_ERROR),
            // Nothing was processed, so the inquiry owns no unit of work to report.
            reported_queries: 0,
            query_terminal: completed,
        },
    ]
}

/// A JSON body as restJson1 carries it.
fn json_body(body: &Value) -> Bytes {
    Bytes::from(body.to_string())
}

/// The `message` an error body carries, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// Create one inquiry and answer its id together with the instant it reports as created
/// at — the instant the clock-owed transition is due a fixed span after.
async fn held_inquiry(router: &MultiServiceRouter, case: &str, body: &Value) -> (String, String) {
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        json_body(body),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the control create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let summary = &created.body["complianceInquirySummary"];
    let id = summary["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create reports the minted inquiry id, got {:?}",
                created.body
            )
        })
        .to_string();
    let created_at = summary["createdAt"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create reports the inquiry's createdAt, got {:?}",
                created.body
            )
        })
        .to_string();
    (id, created_at)
}

/// One timestamp the service rendered, as the instant it names.
fn instant(case: &str, member: &str, value: &Value) -> DateTime {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be a date-time string, got {value:?}")
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
    })
}

/// How many seconds separate two instants the service rendered.
fn seconds_between(case: &str, earlier: &DateTime, later: &DateTime) -> f64 {
    let gap = later.as_secs_f64() - earlier.as_secs_f64();
    assert!(
        gap >= 0.0,
        "case {case:?}: the later instant must not precede the earlier one, got {gap} seconds"
    );
    gap
}

/// Assert `answer` is the front door's authorization refusal: the status, the wire error
/// code and the gateway's constant message.
fn assert_refused(case: &str, answer: &Answer) {
    let (status, code, text) = UNAUTHORIZED;
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
        message(answer),
        text,
        "case {case:?}: expected message {text:?}, got {:?}",
        answer.body
    );
}

/// Assert the inquiry-level pair `reported` carries is the one the submission's record
/// rests in once the clock-owed transition has been applied.
fn assert_inquiry_pair(case: &str, expected: (&str, &str), reported: &Value) {
    assert_eq!(
        (
            reported["status"].as_str(),
            reported["statusMessage"].as_str()
        ),
        (Some(expected.0), Some(expected.1)),
        "case {case:?}: expected the materialised pair {expected:?}, got {reported:?}"
    );
}

/// Assert the inquiry reports the queries its submission owes it, each resting in the
/// pair the materialised record owes them.
///
/// The COUNT is asserted first: an answer reporting no query at all would otherwise satisfy
/// the loop below without reporting anything.
fn assert_queries(
    case: &str,
    expected: (&str, &str),
    expected_count: usize,
    queries: &[Value],
) {
    assert_eq!(
        queries.len(),
        expected_count,
        "case {case:?}: expected {expected_count} reported quer(y|ies), got {queries:?}"
    );
    for query in queries {
        assert_eq!(
            (query["status"].as_str(), query["statusMessage"].as_str()),
            (Some(expected.0), Some(expected.1)),
            "case {case:?}: expected the materialised query pair {expected:?}, got {query:?}"
        );
    }
}

/// The JSON document an export's announced URL serves, read exactly as a client reads it.
async fn exported_document(router: &MultiServiceRouter, case: &str, answer: &Answer) -> Value {
    let url = answer.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the export must announce a document URL, got {:?}",
                answer.body
            )
        });
    let served = follow_announced_url(router, case, url).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: the announced document must be served, got {}",
        served.status
    );
    serde_json::from_slice(&served.bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the exported document must be JSON ({error}), got {:?}",
            String::from_utf8_lossy(&served.bytes)
        )
    })
}

/// What `GetComplianceInquiryMetadata` reports for `id`.
async fn metadata(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = Driver::Metadata.drive(router, id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: an inquiry the account holds must be reported, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquiryDetail"].clone()
}

#[tokio::test]
async fn an_id_the_account_holds_no_inquiry_for_is_the_front_door_refusal_on_every_inquiry_operation()
{
    let router = emulator();

    // Holding one inquiry does not make another id one of the account's: the refusal is
    // keyed on the TARGET, not on the store being empty — so the same rows are driven with
    // the store empty and with an inquiry held.
    for held in [false, true] {
        if held {
            let (id, _) = held_inquiry(
                &router,
                "the inquiry the account does hold",
                &json!({
                    "name": "Subprocessors",
                    "inquiryContent": { "query": "Which subprocessors handle content?" },
                }),
            )
            .await;
            assert!(
                !UNHELD_INQUIRY_IDS.iter().any(|(_, unheld)| *unheld == id),
                "the minted id must not be one of the ids this suite calls unheld, got {id:?}"
            );
        }

        for (row, id) in UNHELD_INQUIRY_IDS {
            let mut answers = Vec::new();
            for driver in DRIVERS {
                let answer = driver.drive(&router, id).await;
                let case = format!("{} {row}, another inquiry held: {held}", driver.operation());
                assert_refused(&case, &answer);
                answers.push((case, answer));
            }

            // Every operation states the SAME refusal, body included: one rule, not four.
            let (first_case, first) = &answers[0];
            for (case, answer) in &answers[1..] {
                assert_eq!(
                    answer.body, first.body,
                    "case {case:?}: must state the same refusal as {first_case:?}, got {:?} \
                     against {:?}",
                    answer.body, first.body
                );
            }
        }
    }
}

#[tokio::test]
async fn every_inquiry_operation_resolves_its_target_with_the_clock_owed_transition_applied() {
    for submission in submissions() {
        // The gap between the inquiry's creation and the instant it reports as last
        // modified, as the first driver observed it: the span the transition fell due
        // after, which cannot depend on which operation materialised it.
        let mut due_after: Option<(&str, f64)> = None;

        for driver in DRIVERS {
            let case = format!("{} on {}", driver.operation(), submission.label);
            // One emulator per case, so the ONLY call that reaches this inquiry past its
            // due instant is the driver under test.
            let router = emulator();
            let (id, created_at) = held_inquiry(&router, &case, &submission.body).await;
            std::thread::sleep(PAST_THE_PROCESSING_SPAN);

            let answer = driver.drive(&router, &id).await;
            assert!(
                (200..300).contains(&answer.status),
                "case {case:?}: an inquiry the account holds is a target the front door \
                 authorizes, so the operation must serve it, got {} with {:?}",
                answer.status,
                answer.body
            );

            // What the driver's OWN answer says about the record it resolved.
            match driver {
                Driver::Metadata => {
                    let detail = answer.body["complianceInquiryDetail"].clone();
                    assert_inquiry_pair(&case, submission.terminal, &detail);
                }
                Driver::Queries => {
                    let queries = answer.body["queries"].as_array().cloned().unwrap_or_default();
                    assert_queries(
                        &case,
                        submission.query_terminal,
                        submission.reported_queries,
                        &queries,
                    );
                }
                Driver::Export => {
                    let document = exported_document(&router, &case, &answer).await;
                    assert_inquiry_pair(&case, submission.terminal, &document);
                    let queries = document["queries"].as_array().cloned().unwrap_or_default();
                    assert_queries(
                        &case,
                        submission.query_terminal,
                        submission.reported_queries,
                        &queries,
                    );
                }
                Driver::Feedback => {
                    // This operation reports no status of its own, so what it resolved is
                    // visible in the record it wrote against and in the projection below.
                    assert!(
                        !answer.body["submittedAt"].is_null(),
                        "case {case:?}: a recorded submission reports submittedAt, got {:?}",
                        answer.body
                    );
                }
            }

            // The record itself, read back: whichever operation resolved the target first,
            // the inquiry rests in the same materialised pair.
            let detail = metadata(&router, &case, &id).await;
            assert_inquiry_pair(&case, submission.terminal, &detail);

            // And it reports itself last modified at the instant the transition fell DUE —
            // which is before this driver called, since the driver ran only after the test
            // slept past it. A reader's own clock would land at or after that call.
            let created_at = instant(&case, "createdAt", &json!(created_at.clone()));
            let updated_at = instant(&case, "updatedAt", &detail["updatedAt"]);
            let gap = seconds_between(&case, &created_at, &updated_at);
            assert!(
                gap > 0.0 && gap < SLEPT_SECONDS,
                "case {case:?}: updatedAt must name the instant the transition fell due, \
                 which is after the creation and before this call, got {gap} seconds after \
                 createdAt"
            );
            match due_after {
                None => due_after = Some((driver.operation(), gap)),
                Some((first_operation, first_gap)) => assert!(
                    (gap - first_gap).abs() < 0.01,
                    "case {case:?}: the transition is due at one instant, so this must match \
                     what {first_operation} observed ({first_gap} seconds after createdAt), \
                     got {gap}"
                ),
            }
        }
    }
}
