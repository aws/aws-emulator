// SPDX-License-Identifier: CC0-1.0

//! What the archive-search reads SAY when the `SearchId` they were handed names no search
//! they can serve, over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so what it
//! pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! RULE: a read that takes a `SearchId` refuses a request it cannot serve with HTTP 400 and
//! the modelled `ValidationException` — neither read models a not-found error
//! (`smithy_query(query="operation",operation="GetArchiveSearch,GetArchiveSearchResults")`
//! lists only `AccessDeniedException`, `ThrottlingException`, `ValidationException` and, for
//! the results read, `ConflictException`) — and the MESSAGE states which condition it met, as
//! fixed text with no variable part: a search the service has a record of but can no longer
//! serve, because the archive it searched no longer serves job operations, is a search that
//! is NOT FOUND, while an id the service has no search for at all is an id it will NOT TAKE.
//!
//! The table spans what that generalises over: the condition met (a job whose archive was
//! deleted while the job was still on its way; a job whose archive was deleted after the job
//! had already reported `COMPLETED`; and ids this service never minted, at both ends of
//! `SearchId`'s modelled 1..64 length, inside it, and one character off a live id), crossed
//! with both reads that take a `SearchId`. The last test pins the SCOPE of the not-found
//! wording: the address-list import-job read, which resolves a different resource, keeps its
//! own.
//!
//! `archive_search_reachability.rs` and `archive_deletion_search_reads.rs` pin the same
//! refusals from the reachability side; this suite is about the TEXT. Nothing here waits for a
//! fixed time: where a job's completion matters, the CONDITION is polled for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The whole message a search read answers a `SearchId` whose search it holds but can no
/// longer serve with, spelled out here rather than read off the implementation so a change of
/// wording fails this suite.
const SEARCH_NOT_FOUND: &str = "Search not found.";

/// The whole message a search read answers a `SearchId` it holds no search for with; spelled
/// out for the same reason as [`SEARCH_NOT_FOUND`].
const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The whole message the address-list import-job read answers an unresolvable `JobId` with:
/// a different resource, and its own wording, which this suite must leave alone.
const IMPORT_JOB_ID_INVALID: &str = "Invalid JobId";

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

/// Both reads a `SearchId` reaches, which must answer one id identically.
const SEARCH_READS: [&str; 2] = ["GetArchiveSearch", "GetArchiveSearchResults"];

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
    async fn create_archive(&self, case: &str, name: &str) -> String {
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
    async fn start_search(&self, case: &str, archive_id: &str, max_results: i64) -> String {
        let (status, out) = self
            .call(
                "StartArchiveSearch",
                json!({
                    "ArchiveId": archive_id,
                    "FromTimestamp": RANGE_FROM,
                    "ToTimestamp": RANGE_TO,
                    "MaxResults": max_results,
                }),
            )
            .await;
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

    /// Delete `archive_id`, failing the test on any refusal.
    async fn delete_archive(&self, case: &str, archive_id: &str) {
        let (status, out) = self
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteArchive must answer 200, got {status} {out:?}"
        );
    }

    /// Look again at `search_id` until the job reports `COMPLETED` or
    /// [`COMPLETION_DEADLINE`] passes. The CONDITION is waited for, never a fixed time.
    async fn poll_until_completed(&self, case: &str, search_id: &str) {
        let deadline = Instant::now() + COMPLETION_DEADLINE;
        loop {
            let (status, out) = self
                .call("GetArchiveSearch", json!({ "SearchId": search_id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetArchiveSearch must serve while the archive is alive, got \
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

/// Assert one read's refusal: HTTP 400, the `ValidationException` code, and `expected` as the
/// WHOLE message.
fn assert_refusal(case: &str, operation: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?} ({operation}): a SearchId that names no servable search must be refused \
         with 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} ({operation}): unexpected error code, got {body:?}"
    );
    assert_eq!(
        error_message(body),
        Some(expected),
        "case {case:?} ({operation}): expected message {expected:?}, got {body:?}"
    );
}

/// A search this service still has a record of, whose archive has gone, is reported as a
/// search that is not found — however far the job had got when the archive was deleted, and
/// through either read.
#[tokio::test]
async fn a_search_whose_archive_is_gone_is_reported_as_not_found() {
    let harness = Harness::new();

    // A job whose archive is deleted while the job is still on its way.
    let in_flight_case = "deleted while on its way";
    let in_flight_archive = harness
        .create_archive(in_flight_case, "not-found-in-flight")
        .await;
    let in_flight_search = harness
        .start_search(in_flight_case, &in_flight_archive, 10)
        .await;
    harness
        .delete_archive(in_flight_case, &in_flight_archive)
        .await;

    // A job that had already reported COMPLETED when its archive was deleted.
    let completed_case = "deleted after completing";
    let completed_archive = harness
        .create_archive(completed_case, "not-found-completed")
        .await;
    let completed_search = harness
        .start_search(completed_case, &completed_archive, 1000)
        .await;
    harness
        .poll_until_completed(completed_case, &completed_search)
        .await;
    harness
        .delete_archive(completed_case, &completed_archive)
        .await;

    let cases: [(&str, &String, &String); 2] = [
        (in_flight_case, &in_flight_search, &in_flight_archive),
        (completed_case, &completed_search, &completed_archive),
    ];

    for (case, search_id, archive_id) in cases {
        for operation in SEARCH_READS {
            let (status, out) = harness
                .call(operation, json!({ "SearchId": search_id }))
                .await;
            assert_refusal(case, operation, status, &out, SEARCH_NOT_FOUND);
            // The sentence is wholly fixed: neither the id asked about nor the archive's.
            let rendered = out.to_string();
            assert!(
                !rendered.contains(search_id.as_str()) && !rendered.contains(archive_id.as_str()),
                "case {case:?} ({operation}): the refusal must echo no id, got {out:?}"
            );
        }
    }

    // A live search beside the refusals still reads back, so the wording above is about the
    // searches whose archives went away and not about a store that stopped serving.
    let live_archive = harness.create_archive("a live search", "not-found-live").await;
    let live_search = harness.start_search("a live search", &live_archive, 10).await;
    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": &live_search }))
        .await;
    assert_eq!(
        (status, out["ArchiveId"].as_str()),
        (200, Some(live_archive.as_str())),
        "case \"a live search\": the search of a live archive must still report it, got {status} \
         {out:?}"
    );
}

/// An id this service holds no search for is refused as an unacceptable `SearchId` instead —
/// the contrast that makes the sentence above carry information.
#[tokio::test]
async fn an_id_naming_no_search_at_all_is_refused_as_an_invalid_search_id() {
    let harness = Harness::new();

    // A live job, so the rows below are refused beside a store that does serve searches.
    let archive_id = harness.create_archive("a live search", "invalid-id-live").await;
    let live_search = harness.start_search("a live search", &archive_id, 10).await;

    // The live id with its last character changed: a near miss, so the refusal is about the
    // whole id rather than a prefix of it.
    let mut near_miss = live_search.clone();
    let last = near_miss.pop().unwrap_or('0');
    near_miss.push(if last == 'z' { 'y' } else { 'z' });
    assert_ne!(
        near_miss, live_search,
        "the near-miss row must differ from the live id"
    );

    // Rows inside `SearchId`'s modelled 1..64 length
    // (`smithy_query(query="shape",shape="SearchId")`) that this store never minted.
    let cases: [(&str, String); 4] = [
        ("the shortest id the length admits", "s".to_string()),
        ("a plausible minted id", "0123456789abcdef".repeat(2)),
        ("the longest id the length admits", "s".repeat(64)),
        ("a near miss of a live id", near_miss),
    ];

    for (case, search_id) in &cases {
        for operation in SEARCH_READS {
            let (status, out) = harness
                .call(operation, json!({ "SearchId": search_id }))
                .await;
            assert_refusal(case, operation, status, &out, SEARCH_ID_INVALID);
            assert!(
                !error_message(&out)
                    .unwrap_or_default()
                    .contains(search_id.as_str()),
                "case {case:?} ({operation}): the refusal must echo no id, got {out:?}"
            );
        }
    }

    // The id that does name a search is unaffected by the refusals.
    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": &live_search }))
        .await;
    assert_eq!(
        (status, out["ArchiveId"].as_str()),
        (200, Some(archive_id.as_str())),
        "case \"the live id after the refusals\": the live search must still report its archive, \
         got {status} {out:?}"
    );
}

/// SCOPE: the not-found sentence belongs to the SEARCH the id names, so the read of another
/// resource that refuses its own identifier with a validation failure — the address-list
/// import job — keeps its own wording.
#[tokio::test]
async fn the_import_job_read_keeps_its_own_refusal_wording() {
    let harness = Harness::new();

    // Rows inside `JobId`'s modelled 1..255 length and `^[a-zA-Z0-9-]+$` pattern
    // (`smithy_query(query="shape",shape="JobId")`) that name no stored import job.
    let cases: [(&str, String); 3] = [
        ("the shortest id the length admits", "j".to_string()),
        ("a hyphenated id the pattern admits", "no-such-job-0001".to_string()),
        ("the longest id the length admits", "j".repeat(255)),
    ];
    for (case, job_id) in &cases {
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": job_id }))
            .await;
        assert_refusal(case, "GetAddressListImportJob", status, &out, IMPORT_JOB_ID_INVALID);
    }
}
