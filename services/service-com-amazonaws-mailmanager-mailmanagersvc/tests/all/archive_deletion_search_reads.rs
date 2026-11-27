// SPDX-License-Identifier: CC0-1.0

//! What the archive-search reads answer once the archive a search ran against has been
//! deleted, over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so what it
//! pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! RULE: deleting an archive takes its search jobs out of reach, so a read that takes a
//! `SearchId` refuses the id as an unacceptable input member — HTTP 400,
//! `ValidationException`, one fixed text that echoes no id — however far the job had got
//! before the deletion, while a listing scoped on the archive itself reports the ARCHIVE as
//! no longer active. The table spans what that generalises over: how far the job had got
//! when its archive was deleted (submitted and still on its way, and already reported
//! `COMPLETED`), the shape of the search it was (filtered and unfiltered, several
//! `MaxResults`), and both reads a `SearchId` reaches.
//!
//! `archive_search_reachability.rs` pins the same wording for an id that was never minted;
//! `archive_lifecycle.rs` pins the archive resource's own lifecycle. Nothing here waits for a
//! fixed time: where a job's completion matters, the CONDITION is polled for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The message a read of a `SearchId` that resolves to no search job carries, spelled out
/// here rather than read off the implementation so a change of wording fails this suite.
const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The message an archive-scoped listing carries once its archive is no longer active;
/// spelled out for the same reason as [`SEARCH_ID_INVALID`].
const ARCHIVE_NOT_ACTIVE: &str = "Archive is not active.";

/// The `SearchState` a job reports once its completion has fallen due.
const SEARCH_STATE_COMPLETED: &str = "COMPLETED";

/// How long this suite waits for a job's completion to fall due before giving up.
/// Generously past the service's own completion span, since what is waited for is a
/// CONDITION (the job reports `COMPLETED`) and not a duration.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(60);

/// How often it looks again while waiting for that condition.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The two instants the searches below are asked over. Any pair inside `Timestamp`'s range
/// does; these are epoch seconds a day apart.
const RANGE_FROM: f64 = 1_761_000_000.0;
/// See [`RANGE_FROM`].
const RANGE_TO: f64 = 1_761_086_400.0;

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential below
        // is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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
    async fn create(&self, case: &str, name: &str) -> String {
        let (status, out) = self.call("CreateArchive", json!({ "ArchiveName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateArchive must answer 200, got {status} {out:?}"
        );
        out["ArchiveId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateArchive returned no ArchiveId: {out:?}"))
            .to_string()
    }

    /// Submit a search on `archive_id` and return its `SearchId`.
    async fn start_search(
        &self,
        case: &str,
        archive_id: &str,
        max_results: i64,
        filters: Option<Value>,
    ) -> String {
        let mut body = json!({
            "ArchiveId": archive_id,
            "FromTimestamp": RANGE_FROM,
            "ToTimestamp": RANGE_TO,
            "MaxResults": max_results,
        });
        if let Some(filters) = filters {
            body["Filters"] = filters;
        }
        let (status, out) = self.call("StartArchiveSearch", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartArchiveSearch must answer 200, got {status} {out:?}"
        );
        out["SearchId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: StartArchiveSearch returned no SearchId: {out:?}")
            })
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

/// The `Message` of a failure body, whichever casing the protocol put it under.
fn error_message(body: &Value) -> Option<&str> {
    body["Message"].as_str().or_else(|| body["message"].as_str())
}

/// An `ArchiveFilters` holding one `CONTAINS` condition over one string email attribute.
///
/// The attribute and the operator are values of the model's `ArchiveStringEmailAttribute`
/// and `ArchiveStringOperator` enums
/// (`smithy_query(query="shape",shape="ArchiveStringEmailAttribute,ArchiveStringOperator")`).
fn subject_filter(value: &str) -> Value {
    json!({
        "Include": [{
            "StringExpression": {
                "Evaluate": { "Attribute": "SUBJECT" },
                "Operator": "CONTAINS",
                "Values": [value],
            }
        }]
    })
}

/// Look again at `search_id` until the job reports `COMPLETED` or [`COMPLETION_DEADLINE`]
/// passes. The CONDITION is waited for, never a fixed time.
async fn poll_until_completed(harness: &Harness, case: &str, search_id: &str) {
    let deadline = Instant::now() + COMPLETION_DEADLINE;
    loop {
        let (status, out) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetArchiveSearch must answer 200 while the archive is alive, got \
             {status} {out:?}"
        );
        if out["Status"]["State"].as_str() == Some(SEARCH_STATE_COMPLETED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the search never reported {SEARCH_STATE_COMPLETED}, last read \
             {out:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Deleting an archive makes every read of its searches refuse the `SearchId` as an
/// unacceptable input member — the same status, code and fixed text however far the job had
/// got, for both reads — while the listings scoped on that archive report the archive as not
/// active, and another archive's searches keep serving.
#[tokio::test]
async fn deleting_an_archive_makes_reads_of_its_searches_refuse_the_search_id() {
    let harness = Harness::new();

    // A bystander archive whose own search must be untouched by any other archive's
    // deletion.
    let bystander = harness.create("bystander", "deletion-bystander").await;
    let bystander_search = harness.start_search("bystander", &bystander, 10, None).await;

    // (case, whether the job is let reach COMPLETED before the deletion, MaxResults, filters)
    // — `MaxResults` rows are inside `SearchMaxResults`' modelled 0..1000 range
    // (`smithy_query(query="shape",shape="SearchMaxResults")`).
    let cases: [(&str, bool, i64, Option<Value>); 4] = [
        ("submitted, unfiltered, deleted at once", false, 10, None),
        (
            "submitted, filtered, deleted at once",
            false,
            1,
            Some(subject_filter("invoice")),
        ),
        ("completed, unfiltered", true, 1000, None),
        (
            "completed, filtered",
            true,
            500,
            Some(subject_filter("receipt")),
        ),
    ];

    for (index, (case, await_completion, max_results, filters)) in cases.into_iter().enumerate() {
        let archive_id = harness
            .create(case, &format!("deletion-unreachable-{index}"))
            .await;
        let search_id = harness
            .start_search(case, &archive_id, max_results, filters)
            .await;

        // Positive control: while the archive is alive the search reads serve.
        if await_completion {
            poll_until_completed(&harness, case, &search_id).await;
            let (status, out) = harness
                .call("GetArchiveSearchResults", json!({ "SearchId": search_id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a completed search of a live archive must report its results, \
                 got {status} {out:?}"
            );
        }
        let (status, out) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetArchiveSearch must serve while the archive is alive, got \
             {status} {out:?}"
        );
        assert_eq!(
            out["ArchiveId"].as_str(),
            Some(archive_id.as_str()),
            "case {case:?}: GetArchiveSearch must echo the archive searched, got {out:?}"
        );

        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteArchive must answer 200, got {status} {out:?}"
        );

        // The search is now unreachable through either read, in one fixed wording that
        // carries neither the SearchId nor the ArchiveId.
        for operation in ["GetArchiveSearch", "GetArchiveSearchResults"] {
            let (status, out) = harness
                .call(operation, json!({ "SearchId": search_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): a search of a deleted archive must be refused, \
                 got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?} ({operation}): unexpected error code, got {out:?}"
            );
            assert_eq!(
                error_message(&out),
                Some(SEARCH_ID_INVALID),
                "case {case:?} ({operation}): unexpected message, got {out:?}"
            );
            let rendered = out.to_string();
            assert!(
                !rendered.contains(search_id.as_str()) && !rendered.contains(archive_id.as_str()),
                "case {case:?} ({operation}): the refusal must echo no id, got {out:?}"
            );
        }

        // The listings scoped on the archive report the ARCHIVE's condition instead, which
        // this fix leaves as it was.
        for operation in ["ListArchiveSearches", "ListArchiveExports"] {
            let (status, out) = harness
                .call(operation, json!({ "ArchiveId": archive_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): an archive that is not active serves no listing, \
                 got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?} ({operation}): unexpected error code, got {out:?}"
            );
            assert_eq!(
                error_message(&out),
                Some(ARCHIVE_NOT_ACTIVE),
                "case {case:?} ({operation}): unexpected message, got {out:?}"
            );
        }

        // One archive's deletion reaches only its own jobs.
        let (status, out) = harness
            .call("GetArchiveSearch", json!({ "SearchId": bystander_search }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: another archive's search must keep serving, got {status} {out:?}"
        );
        assert_eq!(
            out["ArchiveId"].as_str(),
            Some(bystander.as_str()),
            "case {case:?}: the bystander search must still report its own archive, got {out:?}"
        );
    }
}
