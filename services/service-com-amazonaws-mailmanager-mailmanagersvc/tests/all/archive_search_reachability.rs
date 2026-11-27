// SPDX-License-Identifier: CC0-1.0

//! How the archive-search reads word the refusal of a `SearchId` that resolves to nothing,
//! over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so what it
//! pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! RULE: an operation that cannot resolve the search a `SearchId` names refuses the request
//! with a validation failure (HTTP 400, `ValidationException`) carrying the fixed text of the
//! condition it met and no id at all — the input MEMBER is unacceptable when this service
//! holds no search of that id, and the SEARCH is not there when it holds one whose archive no
//! longer serves job operations — whichever of the two reads resolved it. The table below is
//! that class: both reads against both ways a `SearchId` resolves to nothing (one never
//! minted, at both ends of the modelled 1..64 length and inside it; and a real job whose
//! archive no longer serves job operations).
//!
//! The minted `ArchiveId`'s own form, and the search lifecycle on it, are pinned by
//! `archive_lifecycle.rs`; the rows a completed search reports by `archive_search_results.rs`.
//! Nothing here waits for a clock: a search that cannot be resolved is refused whatever state
//! it was in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The message a refusal of an id this service holds no search job for carries, spelled out
/// here rather than read off the implementation so a change of wording fails this suite.
const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The message a refusal of a search job this service holds but no longer serves carries;
/// spelled out for the same reason as [`SEARCH_ID_INVALID`].
const SEARCH_NOT_FOUND: &str = "Search not found.";

/// The two instants the search below is asked over. Any pair inside `Timestamp`'s range does;
/// these are epoch seconds a day apart.
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
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
}

/// An id that resolves to no search job is refused with the same status, the same error code
/// and the fixed text of its condition — for both reads that take a `SearchId`, and for both
/// ways an id resolves to nothing.
#[tokio::test]
async fn an_unresolvable_search_id_is_refused_with_its_conditions_message() {
    let harness = Harness::new();

    // A real job, of an archive that is then deleted — the archive going away is what takes
    // the job out of reach, whatever state the job itself was in.
    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "search-unreachable" }))
        .await;
    assert_eq!(status, 200, "CreateArchive must answer 200, got {status} {out:?}");
    let archive_id = out["ArchiveId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateArchive returned no ArchiveId: {out:?}"))
        .to_string();

    let (status, out) = harness
        .call(
            "StartArchiveSearch",
            json!({
                "ArchiveId": archive_id,
                "FromTimestamp": RANGE_FROM,
                "ToTimestamp": RANGE_TO,
                "MaxResults": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "StartArchiveSearch must accept the ArchiveId CreateArchive returned, got {status} \
         {out:?}"
    );
    let job_of_a_deleted_archive = out["SearchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartArchiveSearch returned no SearchId: {out:?}"))
        .to_string();

    let (status, out) = harness
        .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status} {out:?}");

    // (case, the id presented, the message that condition carries) — ids inside `SearchId`'s
    // modelled 1..64 length (`smithy_query(query="shape",shape="SearchId")`) that resolve to
    // nothing: the shortest, a plausible minted-looking value, the longest, and the id of a
    // job whose archive no longer serves job operations.
    let cases: [(&str, String, &str); 4] = [
        (
            "the shortest id the length admits",
            "s".to_string(),
            SEARCH_ID_INVALID,
        ),
        (
            "a plausible minted id",
            "0123456789abcdef".repeat(2),
            SEARCH_ID_INVALID,
        ),
        (
            "the longest id the length admits",
            "s".repeat(64),
            SEARCH_ID_INVALID,
        ),
        (
            "a job of a deleted archive",
            job_of_a_deleted_archive,
            SEARCH_NOT_FOUND,
        ),
    ];

    for (case, search_id, expected) in &cases {
        for operation in ["GetArchiveSearch", "GetArchiveSearchResults"] {
            let (status, out) = harness
                .call(operation, json!({ "SearchId": search_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): an unresolvable SearchId must be refused, got \
                 {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?} ({operation}): unexpected error code, got {out:?}"
            );
            assert_eq!(
                error_message(&out),
                Some(*expected),
                "case {case:?} ({operation}): expected message {expected:?}, got {out:?}"
            );
        }
    }
}
