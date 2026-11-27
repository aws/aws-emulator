// SPDX-License-Identifier: CC0-1.0

//! `GetArchiveSearchResults` over the wire: the rows a completed archive search reports.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so what it
//! pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The rule under test is that a completed search reports the archive's mail its range and
//! its filters admit, newest first, capped at the job's `MaxResults`. The table spans what
//! that generalises over: each bound of the range and one instant past each of them, every
//! `ArchiveStringEmailAttribute` the model declares, both `ArchiveBooleanOperator` values,
//! each shape of `ArchiveFilters`, several caps, and two archives whose mail must never
//! cross over.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::ops::types::{
    Envelope, Row, ARCHIVE_STRING_EMAIL_ATTRIBUTE_VALUES,
};
use service_com_amazonaws_mailmanager_mailmanagersvc::state::{self, ArchivedMessageRecord};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// How long the suites below wait for a job's completion to fall due before giving up.
/// Generously past the service's own completion span, since what is waited for is a
/// CONDITION (the job reports `COMPLETED`) and not a duration.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(60);

/// How often they look again while waiting for that condition.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `SearchState` a job reports once its completion has fallen due.
const SEARCH_STATE_COMPLETED: &str = "COMPLETED";

/// The message a search whose completion is not yet due is refused with, spelled out here
/// rather than read off the implementation so a change of wording fails this suite.
const SEARCH_NOT_COMPLETED: &str = "Search is not completed yet.";

/// The message a `SearchId` that resolves to no search job is refused with; spelled out for
/// the same reason as [`SEARCH_NOT_COMPLETED`].
const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The lower bound of the range every search below is asked over, in epoch MILLISECONDS.
const RANGE_FROM_MILLIS: i64 = 1_761_000_000_000;
/// The upper bound, a day later; see [`RANGE_FROM_MILLIS`].
const RANGE_TO_MILLIS: i64 = 1_761_086_400_000;

/// The instant the two messages that share one millisecond were received.
const SHARED_INSTANT_MILLIS: i64 = RANGE_FROM_MILLIS + 3_600_000;

/// `millis` as the epoch SECONDS an `awsJson1_0` `Timestamp` travels as.
fn seconds(millis: i64) -> f64 {
    millis as f64 / 1000.0
}

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
    /// The SAME engine the service is mounted on, so a test can place in this service's
    /// store the archived mail only an email ARRIVING in an archive would write: no
    /// operation this crate serves archives a message, so this is what makes the
    /// assertions below a test of a PROJECTION of state rather than of a constant empty
    /// list.
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential below
        // is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an archive and return its `ArchiveId`, failing the test on any refusal.
    async fn create(&self, name: &str) -> String {
        let (status, out) = self.call("CreateArchive", json!({ "ArchiveName": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateArchive to answer 200, got {status} {out:?}"
        );
        out["ArchiveId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateArchive returned no ArchiveId: {out:?}"))
            .to_string()
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// One message to archive: the metadata a `Row` reports, flattened so a table can spell it
/// out.
struct Message {
    id: &'static str,
    received_at_millis: i64,
    to: &'static str,
    from: &'static str,
    cc: &'static str,
    subject: &'static str,
    envelope_from: &'static str,
    envelope_to: &'static [&'static str],
    has_attachments: bool,
}

/// The mail archived into the FIRST archive: two inside the range at its bounds, two
/// sharing one millisecond in the middle, and one just outside each bound.
const ARCHIVE_MAIL: &[Message] = &[
    Message {
        id: "am-at-the-lower-bound",
        received_at_millis: RANGE_FROM_MILLIS,
        to: "ops@example.com",
        from: "alice@Example.com",
        cc: "audit@example.com",
        subject: "Quarterly report",
        envelope_from: "alice@example.com",
        envelope_to: &["ops@example.com"],
        has_attachments: true,
    },
    Message {
        id: "am-first-of-the-shared-instant",
        received_at_millis: SHARED_INSTANT_MILLIS,
        to: "team@example.net",
        from: "bob@example.net",
        cc: "",
        subject: "Lunch plans",
        envelope_from: "bounce@example.net",
        envelope_to: &["team@example.net", "cc-list@example.net"],
        has_attachments: false,
    },
    Message {
        id: "am-second-of-the-shared-instant",
        received_at_millis: SHARED_INSTANT_MILLIS,
        to: "team@example.net",
        from: "erin@example.net",
        cc: "",
        subject: "Lunch plans follow-up",
        envelope_from: "noreply@example.net",
        envelope_to: &["team@example.net"],
        has_attachments: true,
    },
    Message {
        id: "am-at-the-upper-bound",
        received_at_millis: RANGE_TO_MILLIS,
        to: "ops@example.com",
        from: "carol@example.org",
        cc: "legal@example.org",
        subject: "Invoice 4521",
        envelope_from: "carol@example.org",
        envelope_to: &["ops@example.com"],
        has_attachments: false,
    },
    Message {
        id: "am-one-millisecond-before-the-range",
        received_at_millis: RANGE_FROM_MILLIS - 1,
        to: "ops@example.com",
        from: "alice@example.com",
        cc: "",
        subject: "Invoice 4520",
        envelope_from: "alice@example.com",
        envelope_to: &["ops@example.com"],
        has_attachments: true,
    },
    Message {
        id: "am-one-millisecond-after-the-range",
        received_at_millis: RANGE_TO_MILLIS + 1,
        to: "ops@example.com",
        from: "carol@example.org",
        cc: "legal@example.org",
        subject: "Invoice 4522",
        envelope_from: "carol@example.org",
        envelope_to: &["ops@example.com"],
        has_attachments: false,
    },
];

/// The mail archived into the SECOND archive: one message whose every attribute also
/// appears in the first archive's mail, so a search that reported the wrong archive's mail
/// would be caught.
const OTHER_ARCHIVE_MAIL: &[Message] = &[Message {
    id: "bm-in-the-other-archive",
    received_at_millis: RANGE_FROM_MILLIS + 10,
    to: "ops@example.com",
    from: "alice@Example.com",
    cc: "legal@example.org",
    subject: "Invoice 4521",
    envelope_from: "bounce@example.net",
    envelope_to: &["cc-list@example.net"],
    has_attachments: true,
}];

/// Archive `mail` into `archive_id` in this service's store.
fn archive_mail(harness: &Harness, archive_id: &str, mail: &[Message]) {
    let store = state::service_state(&*harness.engine).expect("this service's store");
    let mut store = store.lock();
    for message in mail {
        store.archived_messages.insert(
            message.id.to_string(),
            ArchivedMessageRecord {
                archived_message_id: message.id.to_string(),
                archive_id: archive_id.to_string(),
                received_at_millis: message.received_at_millis,
                metadata: Row {
                    to: Some(message.to.to_string()),
                    from: Some(message.from.to_string()),
                    cc: Some(message.cc.to_string()),
                    subject: Some(message.subject.to_string()),
                    has_attachments: Some(message.has_attachments),
                    envelope: Some(Envelope {
                        helo: Some("mail.example.net".to_string()),
                        from: Some(message.envelope_from.to_string()),
                        to: Some(
                            message
                                .envelope_to
                                .iter()
                                .map(|address| (*address).to_string())
                                .collect(),
                        ),
                    }),
                    ..Row::default()
                },
            },
        );
    }
}

/// A `StartArchiveSearch` request body.
fn search_request(
    archive_id: &str,
    from_millis: i64,
    to_millis: i64,
    max_results: i64,
    filters: Option<Value>,
) -> Value {
    let mut body = json!({
        "ArchiveId": archive_id,
        "FromTimestamp": seconds(from_millis),
        "ToTimestamp": seconds(to_millis),
        "MaxResults": max_results,
    });
    if let Some(filters) = filters {
        body["Filters"] = filters;
    }
    body
}

/// An `ArchiveFilters` whose `Include`/`Unless` list holds `conditions`.
fn filters(include: Vec<Value>, unless: Vec<Value>) -> Value {
    let mut built = json!({});
    if !include.is_empty() {
        built["Include"] = Value::Array(include);
    }
    if !unless.is_empty() {
        built["Unless"] = Value::Array(unless);
    }
    built
}

/// A `CONTAINS` condition over one `ArchiveStringEmailAttribute`.
fn contains(attribute: &str, value: &str) -> Value {
    json!({
        "StringExpression": {
            "Evaluate": { "Attribute": attribute },
            "Operator": "CONTAINS",
            "Values": [value],
        }
    })
}

/// A boolean condition over `HAS_ATTACHMENTS`, the one `ArchiveBooleanEmailAttribute`.
fn has_attachments(operator: &str) -> Value {
    json!({
        "BooleanExpression": {
            "Evaluate": { "Attribute": "HAS_ATTACHMENTS" },
            "Operator": operator,
        }
    })
}

/// Start a search and return its `SearchId`, failing the test on any refusal.
async fn start_search(harness: &Harness, case: &str, request: Value) -> String {
    let (status, out) = harness.call("StartArchiveSearch", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartArchiveSearch must answer 200, got {status} {out:?}"
    );
    out["SearchId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: StartArchiveSearch returned no SearchId: {out:?}"))
        .to_string()
}

/// `GetArchiveSearchResults` on `search_id`, looked at again until the job's completion has
/// fallen due or [`COMPLETION_DEADLINE`] passes. The CONDITION is waited for, never a fixed
/// time.
async fn results_once_completed(harness: &Harness, case: &str, search_id: &str) -> Value {
    let deadline = Instant::now() + COMPLETION_DEADLINE;
    loop {
        let (status, read) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetArchiveSearch must answer 200, got {status} {read:?}"
        );
        if read["Status"]["State"].as_str() == Some(SEARCH_STATE_COMPLETED) {
            let (status, out) = harness
                .call("GetArchiveSearchResults", json!({ "SearchId": search_id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetArchiveSearchResults must answer 200 for a completed \
                 search, got {status} {out:?}"
            );
            return out;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the search never reported {SEARCH_STATE_COMPLETED}, last read \
             {read:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `ArchivedMessageId` of every row of a results body, in the order reported.
fn reported_ids(case: &str, out: &Value) -> Vec<String> {
    out["Rows"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Rows must be a list, got {out:?}"))
        .iter()
        .map(|row| {
            row["ArchivedMessageId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a row must carry its id, got {row:?}"))
                .to_string()
        })
        .collect()
}

/// One row of the search table: `(case, the archive to search, the range's two bounds in
/// epoch milliseconds, the result cap, the filters, the ids expected in the order reported)`.
type Case<'a> = (
    &'static str,
    &'a str,
    i64,
    i64,
    i64,
    Option<Value>,
    Vec<&'static str>,
);

/// A completed search reports the archive's mail its range and its filters admit, newest
/// first, capped at the job's `MaxResults` — and never another archive's mail.
///
/// Each row of the table is one search: `(case, which archive, range, cap, filters,
/// the ids expected in order)`. The string-attribute rows cover every value the model's
/// `ArchiveStringEmailAttribute` declares, which the assertion after the table checks
/// against `ARCHIVE_STRING_EMAIL_ATTRIBUTE_VALUES` (generated from that enum).
#[tokio::test]
async fn completed_search_results_report_the_mail_the_range_and_filters_admit() {
    let harness = Harness::new();
    let archive_id = harness.create("search-results").await;
    let other_archive_id = harness.create("search-results-other").await;
    archive_mail(&harness, &archive_id, ARCHIVE_MAIL);
    archive_mail(&harness, &other_archive_id, OTHER_ARCHIVE_MAIL);

    // The whole range, newest first: the two bound messages are in, the two a millisecond
    // outside are out, and the pair sharing one instant is ordered by id.
    let whole_range = vec![
        "am-at-the-upper-bound",
        "am-first-of-the-shared-instant",
        "am-second-of-the-shared-instant",
        "am-at-the-lower-bound",
    ];

    let cases: Vec<Case> = vec![
        (
            "no filters over the whole range",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            None,
            whole_range.clone(),
        ),
        (
            "a range of one millisecond holds both messages received in it",
            &archive_id,
            SHARED_INSTANT_MILLIS,
            SHARED_INSTANT_MILLIS,
            1000,
            None,
            vec![
                "am-first-of-the-shared-instant",
                "am-second-of-the-shared-instant",
            ],
        ),
        (
            "a range a second inside each bound drops the bound messages",
            &archive_id,
            RANGE_FROM_MILLIS + 1_000,
            RANGE_TO_MILLIS - 1_000,
            1000,
            None,
            vec![
                "am-first-of-the-shared-instant",
                "am-second-of-the-shared-instant",
            ],
        ),
        (
            "a range past both bounds still holds only this archive's mail",
            &archive_id,
            RANGE_FROM_MILLIS - 10_000,
            RANGE_TO_MILLIS + 10_000,
            1000,
            None,
            vec![
                "am-one-millisecond-after-the-range",
                "am-at-the-upper-bound",
                "am-first-of-the-shared-instant",
                "am-second-of-the-shared-instant",
                "am-at-the-lower-bound",
                "am-one-millisecond-before-the-range",
            ],
        ),
        (
            "the smallest cap reports nothing",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            0,
            None,
            vec![],
        ),
        (
            "a cap inside the modelled range keeps the newest",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            2,
            None,
            vec!["am-at-the-upper-bound", "am-first-of-the-shared-instant"],
        ),
        (
            "an include on TO",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("TO", "ops@")], vec![])),
            vec!["am-at-the-upper-bound", "am-at-the-lower-bound"],
        ),
        (
            "an include on FROM, matched without regard to case",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("FROM", "ALICE@EXAMPLE.COM")], vec![])),
            vec!["am-at-the-lower-bound"],
        ),
        (
            "an include on CC",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("CC", "legal@")], vec![])),
            vec!["am-at-the-upper-bound"],
        ),
        (
            "an include on SUBJECT",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("SUBJECT", "invoice")], vec![])),
            vec!["am-at-the-upper-bound"],
        ),
        (
            "an include on ENVELOPE_FROM reads the envelope, not the From header",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("ENVELOPE_FROM", "bounce@")], vec![])),
            vec!["am-first-of-the-shared-instant"],
        ),
        (
            "an include on ENVELOPE_TO evaluates every envelope recipient",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("ENVELOPE_TO", "cc-list@")], vec![])),
            vec!["am-first-of-the-shared-instant"],
        ),
        (
            "an include on HAS_ATTACHMENTS IS_TRUE",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![has_attachments("IS_TRUE")], vec![])),
            vec!["am-second-of-the-shared-instant", "am-at-the-lower-bound"],
        ),
        (
            "an include on HAS_ATTACHMENTS IS_FALSE",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![has_attachments("IS_FALSE")], vec![])),
            vec!["am-at-the-upper-bound", "am-first-of-the-shared-instant"],
        ),
        (
            "two includes must both hold",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(
                vec![contains("TO", "ops@"), contains("SUBJECT", "invoice")],
                vec![],
            )),
            vec!["am-at-the-upper-bound"],
        ),
        (
            "an unless excludes what it matches",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![], vec![contains("SUBJECT", "lunch")])),
            vec!["am-at-the-upper-bound", "am-at-the-lower-bound"],
        ),
        (
            "an unless over a boolean attribute",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![], vec![has_attachments("IS_TRUE")])),
            vec!["am-at-the-upper-bound", "am-first-of-the-shared-instant"],
        ),
        (
            "an unless narrows what the includes admitted",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(
                vec![contains("TO", "ops@")],
                vec![contains("SUBJECT", "invoice")],
            )),
            vec!["am-at-the-lower-bound"],
        ),
        (
            "an include no message satisfies reports nothing",
            &archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(
                vec![contains("SUBJECT", "no message carries this")],
                vec![],
            )),
            vec![],
        ),
        (
            "the other archive reports only its own mail",
            &other_archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            None,
            vec!["bm-in-the-other-archive"],
        ),
        (
            "the other archive's filters are evaluated against its own mail",
            &other_archive_id,
            RANGE_FROM_MILLIS,
            RANGE_TO_MILLIS,
            1000,
            Some(filters(vec![contains("SUBJECT", "lunch")], vec![])),
            vec![],
        ),
    ];

    // Every attribute the model declares is covered by a row above.
    let covered: std::collections::BTreeSet<&str> = cases
        .iter()
        .filter_map(|(_, _, _, _, _, filters, _)| filters.as_ref())
        .flat_map(|filters| {
            ["Include", "Unless"]
                .into_iter()
                .filter_map(|list| filters.get(list))
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(|condition| {
                    condition["StringExpression"]["Evaluate"]["Attribute"].as_str()
                })
                .collect::<Vec<&str>>()
        })
        .collect();
    assert_eq!(
        covered.into_iter().collect::<Vec<&str>>(),
        ARCHIVE_STRING_EMAIL_ATTRIBUTE_VALUES.to_vec(),
        "every ArchiveStringEmailAttribute the model declares must be one of the rows"
    );

    // The searches are SUBMITTED first and read afterwards: each completes on its own
    // stored instant, so one wait covers every job.
    let mut started: Vec<(&str, String, Vec<&str>)> = Vec::new();
    for (case, archive, from_millis, to_millis, max_results, filters, expected) in cases {
        let request = search_request(archive, from_millis, to_millis, max_results, filters);
        let search_id = start_search(&harness, case, request).await;
        started.push((case, search_id, expected));
    }

    let unique: std::collections::BTreeSet<&String> =
        started.iter().map(|(_, search_id, _)| search_id).collect();
    assert_eq!(
        unique.len(),
        started.len(),
        "each submitted search must be tracked under its own SearchId, got {started:?}"
    );

    for (case, search_id, expected) in &started {
        let out = results_once_completed(&harness, case, search_id).await;
        assert_eq!(
            reported_ids(case, &out),
            expected.iter().map(|id| (*id).to_string()).collect::<Vec<String>>(),
            "case {case:?}: unexpected rows, got {out:?}"
        );
    }
}

/// Every row carries the metadata the message was archived with, at the instant it was
/// received.
#[tokio::test]
async fn every_reported_row_carries_the_metadata_of_its_message() {
    let harness = Harness::new();
    let archive_id = harness.create("search-results-rows").await;
    archive_mail(&harness, &archive_id, ARCHIVE_MAIL);

    let case = "the whole range";
    let search_id = start_search(
        &harness,
        case,
        search_request(&archive_id, RANGE_FROM_MILLIS, RANGE_TO_MILLIS, 1000, None),
    )
    .await;
    let out = results_once_completed(&harness, case, &search_id).await;
    let rows = out["Rows"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Rows must be a list, got {out:?}"));

    for row in rows {
        let id = row["ArchivedMessageId"].as_str().unwrap_or("<no id>");
        let archived = ARCHIVE_MAIL
            .iter()
            .find(|message| message.id == id)
            .unwrap_or_else(|| panic!("case {id:?}: a reported row must be an archived message"));
        assert_eq!(
            row["ReceivedTimestamp"].as_f64(),
            Some(seconds(archived.received_at_millis)),
            "case {id:?}: the row must report when the message was received, got {row:?}"
        );
        assert_eq!(
            row["To"].as_str(),
            Some(archived.to),
            "case {id:?}: the row must report the To header, got {row:?}"
        );
        assert_eq!(
            row["From"].as_str(),
            Some(archived.from),
            "case {id:?}: the row must report the From header, got {row:?}"
        );
        assert_eq!(
            row["Subject"].as_str(),
            Some(archived.subject),
            "case {id:?}: the row must report the Subject header, got {row:?}"
        );
        assert_eq!(
            row["HasAttachments"].as_bool(),
            Some(archived.has_attachments),
            "case {id:?}: the row must report the attachment flag, got {row:?}"
        );
        assert_eq!(
            row["Envelope"]["From"].as_str(),
            Some(archived.envelope_from),
            "case {id:?}: the row must report the SMTP envelope, got {row:?}"
        );
        assert_eq!(
            row["Envelope"]["To"].as_array().map(|to| to.len()),
            Some(archived.envelope_to.len()),
            "case {id:?}: the row must report every envelope recipient, got {row:?}"
        );
    }

    // Reading the results again reports the same rows: nothing moved because they were
    // read.
    let again = results_once_completed(&harness, case, &search_id).await;
    assert_eq!(
        again["Rows"], out["Rows"],
        "case {case:?}: a completed search's rows must not move between reads, got {again:?}"
    );
}

/// The results of a search whose completion is not yet due are refused: the operation
/// reports the results of a COMPLETED job, so one still on its way conflicts with the state
/// it is in. Once the completion falls due the same call is served.
#[tokio::test]
async fn the_results_of_a_search_that_has_not_completed_are_refused() {
    let harness = Harness::new();
    let archive_id = harness.create("search-results-queued").await;
    archive_mail(&harness, &archive_id, ARCHIVE_MAIL);

    let case = "a search read straight after it was submitted";
    let search_id = start_search(
        &harness,
        case,
        search_request(&archive_id, RANGE_FROM_MILLIS, RANGE_TO_MILLIS, 1000, None),
    )
    .await;

    let (status, out) = harness
        .call("GetArchiveSearchResults", json!({ "SearchId": search_id }))
        .await;
    assert_eq!(
        status, 409,
        "case {case:?}: a search that has not completed has no results to report, got \
         {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case {case:?}: unexpected error code, got {out:?}"
    );
    assert_eq!(
        out["message"].as_str().or_else(|| out["Message"].as_str()),
        Some(SEARCH_NOT_COMPLETED),
        "case {case:?}: unexpected message, got {out:?}"
    );

    // The same call, once the completion has fallen due, is served.
    let served = results_once_completed(&harness, case, &search_id).await;
    assert_eq!(
        reported_ids(case, &served).len(),
        4,
        "case {case:?}: the completed search must report the mail inside its range, got \
         {served:?}"
    );
}

/// A `SearchId` that resolves to no search job this store serves is refused the way
/// `GetArchiveSearch` refuses it: the modelled `ValidationException`, whatever made the id
/// unresolvable.
#[tokio::test]
async fn search_results_refuse_a_search_id_that_resolves_to_no_job() {
    let harness = Harness::new();
    let archive_id = harness.create("search-results-refusals").await;
    archive_mail(&harness, &archive_id, ARCHIVE_MAIL);
    let completed_search = start_search(
        &harness,
        "a job of an archive about to be deleted",
        search_request(&archive_id, RANGE_FROM_MILLIS, RANGE_TO_MILLIS, 1000, None),
    )
    .await;

    // Ids inside the modelled 1..64 length that this store never minted: a plain word, a
    // value of another shape's form, and one of the greatest admitted length.
    let never_minted = ["s", "not-a-search-id", &"a".repeat(64)];
    for search_id in never_minted {
        let (status, out) = harness
            .call("GetArchiveSearchResults", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {search_id:?}: an id that names no job is refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {search_id:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["message"].as_str().or_else(|| out["Message"].as_str()),
            Some(SEARCH_ID_INVALID),
            "case {search_id:?}: unexpected message, got {out:?}"
        );
    }

    // An id past the modelled length never reaches the handler at all.
    let (status, out) = harness
        .call(
            "GetArchiveSearchResults",
            json!({ "SearchId": "s".repeat(65) }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"past the modelled length\": the constraint must be enforced, got {status} \
         {out:?}"
    );

    // A job whose archive is marked for deletion is no longer reachable, exactly as it is
    // not through `GetArchiveSearch`.
    let (status, out) = harness
        .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "DeleteArchive must answer 200, got {status} {out:?}"
    );
    let (status, out) = harness
        .call(
            "GetArchiveSearchResults",
            json!({ "SearchId": completed_search }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"an archive marked for deletion\": its jobs serve no results, got {status} \
         {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"an archive marked for deletion\": unexpected error code, got {out:?}"
    );
    assert_eq!(
        out["message"].as_str().or_else(|| out["Message"].as_str()),
        Some(SEARCH_ID_INVALID),
        "case \"an archive marked for deletion\": unexpected message, got {out:?}"
    );
}
