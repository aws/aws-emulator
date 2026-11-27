// SPDX-License-Identifier: CC0-1.0

//! The email-archive lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` —
//! so what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the rule covers
//! (every modelled `RetentionPeriod`, several `ArchiveName` shapes, several
//! archive-count/`PageSize` combinations, each published `PageSize` bound and one past
//! it). The archive the bootstrap issue happened to name is one row among them.
//!
//! The archive's JOB plane — the search and export jobs started on an archive and the
//! listings that report them — is the second half of this file, from
//! `a_minted_archive_id_satisfies_the_constrained_archive_id_shape` on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::ops::types::RETENTION_PERIOD_VALUES;
use service_com_amazonaws_mailmanager_mailmanagersvc::state::{
    self, ExportJobRecord, ARCHIVE_JOB_COMPLETION_MILLIS, EXPORT_STATE_COMPLETED,
    EXPORT_STATE_QUEUED, MAX_ARCHIVES, SEARCH_STATE_COMPLETED, SEARCH_STATE_QUEUED,
};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the ARNs the service returns must carry it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
    /// The SAME engine the service is mounted on, so a test can place in this service's
    /// store a record only an operation that is still a scaffold would write (an archive
    /// EXPORT job: `StartArchiveExport` is unwritten, while `ListArchiveExports` reports
    /// what the store holds).
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
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
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field
            // is this service's signing name (`ses`) and whose 3rd is the region the
            // response's ARNs must use.
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
    async fn create(&self, body: Value) -> String {
        let (status, out) = self.call("CreateArchive", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body:?}: expected CreateArchive to answer 200, got {status} {out:?}"
        );
        out["ArchiveId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {body:?}: CreateArchive returned no ArchiveId: {out:?}"))
            .to_string()
    }
}

/// The message a refusal of an id this service holds no search job for carries, spelled out
/// here rather than read off the implementation so a change of wording fails this suite.
const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The message a refusal of a search job this service holds but no longer serves carries;
/// spelled out for the same reason as [`SEARCH_ID_INVALID`].
const SEARCH_NOT_FOUND: &str = "Search not found.";

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

/// `Retention` round-trips for EVERY value the model's `RetentionPeriod` enum declares,
/// and an archive created without one still reports the required member.
///
/// The catalog is `RETENTION_PERIOD_VALUES`, generated from the model's `RetentionPeriod`
/// enum (`smithy_query(query="shape",shape="RetentionPeriod")`), so the row set cannot
/// drift from the model.
#[tokio::test]
async fn create_then_get_round_trips_every_modelled_retention_period() {
    let harness = Harness::new();

    for (index, period) in RETENTION_PERIOD_VALUES.iter().enumerate() {
        let name = format!("retention-{index}");
        let archive_id = harness
            .create(json!({
                "ArchiveName": name,
                "Retention": { "RetentionPeriod": period },
                "Tags": [{ "Key": "team", "Value": "mail" }],
            }))
            .await;

        let (status, out) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {period:?}: expected GetArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out["ArchiveId"].as_str(),
            Some(archive_id.as_str()),
            "case {period:?}: GetArchive must report the created id, got {out:?}"
        );
        assert_eq!(
            out["ArchiveName"].as_str(),
            Some(name.as_str()),
            "case {period:?}: GetArchive must report the created name, got {out:?}"
        );
        assert_eq!(
            out["ArchiveState"].as_str(),
            Some("ACTIVE"),
            "case {period:?}: a freshly created archive is ACTIVE, got {out:?}"
        );
        assert_eq!(
            out["Retention"]["RetentionPeriod"].as_str(),
            Some(*period),
            "case {period:?}: GetArchive must report the stored retention, got {out:?}"
        );
        assert_eq!(
            out["ArchiveArn"].as_str(),
            Some(
                format!("arn:aws:ses:{REGION}:{ACCOUNT}:mail-manager-archive/{archive_id}")
                    .as_str()
            ),
            "case {period:?}: unexpected ArchiveArn, got {out:?}"
        );
        assert!(
            out["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
            "case {period:?}: GetArchive must report CreatedTimestamp, got {out:?}"
        );
        assert!(
            out["LastUpdatedTimestamp"]
                .as_f64()
                .is_some_and(|t| t > 0.0),
            "case {period:?}: GetArchive must report LastUpdatedTimestamp, got {out:?}"
        );
    }

    // No `Retention` supplied: the model declares no default for the input member yet
    // makes the output member required, so the record must still carry one.
    let archive_id = harness
        .create(json!({ "ArchiveName": "retention-unspecified" }))
        .await;
    let (status, out) = harness
        .call("GetArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "case retention-unspecified: expected 200, got {status} {out:?}"
    );
    let reported = out["Retention"]["RetentionPeriod"].as_str();
    assert!(
        reported.is_some_and(|value| RETENTION_PERIOD_VALUES.contains(&value)),
        "case retention-unspecified: Retention must be a modelled RetentionPeriod, got {out:?}"
    );
}

/// An `ArchiveId` the service never minted is a `ResourceNotFoundException` (404) on
/// `GetArchive` — the operation that models it — for ids of every shape, and never a
/// hollow 200.
#[tokio::test]
async fn get_archive_refuses_an_unknown_id() {
    let harness = Harness::new();
    // One real archive exists, so the refusal is about the id and not about emptiness.
    harness
        .create(json!({ "ArchiveName": "the-only-archive" }))
        .await;

    // `ArchiveIdString` is len 1..66: the single character, a plausible-looking id, a
    // 66-character id at the modelled maximum, and the id of a never-created name.
    let cases = [
        "x",
        "does-not-exist",
        &"0".repeat(66),
        "00000000000000000000000000000000",
    ];
    for case in cases {
        let (status, out) = harness.call("GetArchive", json!({ "ArchiveId": case })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetArchive to answer 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// An `ArchiveName` already in use is a `ConflictException` (409), whatever the name's
/// shape; a repeat with the SAME `ClientToken` is a retry and returns the original id.
#[tokio::test]
async fn archive_names_are_unique_and_client_tokens_are_idempotent() {
    let harness = Harness::new();

    // `ArchiveNameString` is len 1..64 with pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$`: the shortest name the pattern admits,
    // one using both inner punctuation characters, a digits-only name, and a name at the
    // modelled 64-character maximum.
    let names = [
        "ab".to_string(),
        "demo_archive-01".to_string(),
        "2024".to_string(),
        format!("a{}z", "x".repeat(62)),
    ];

    for (index, name) in names.iter().enumerate() {
        let token = format!("tok-{index}");
        let first = harness
            .create(json!({ "ArchiveName": name, "ClientToken": token }))
            .await;

        // Same token: a retry, not a second archive.
        let repeat = harness
            .create(json!({ "ArchiveName": name, "ClientToken": token }))
            .await;
        assert_eq!(
            repeat, first,
            "case {name:?}: a retry with ClientToken {token:?} must return the original id"
        );

        // Fresh token, same name: a duplicate.
        let (status, out) = harness
            .call(
                "CreateArchive",
                json!({ "ArchiveName": name, "ClientToken": format!("fresh-{index}") }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected a duplicate name to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {name:?}: unexpected error code, got {out:?}"
        );
    }

    // Exactly one archive per name survived: no retry created a duplicate.
    let (status, out) = harness.call("ListArchives", json!({})).await;
    assert_eq!(status, 200, "ListArchives must answer 200, got {out:?}");
    let listed = out["Archives"].as_array().expect("Archives is a list").len();
    assert_eq!(
        listed,
        names.len(),
        "expected exactly one archive per name ({}), got {listed}: {out:?}",
        names.len()
    );

    // A token is a retry of ONE request: presented with a different request it is not a
    // retry, and must not be answered with the first request's archive. Rows vary which
    // member differs.
    let differing: [(&str, Value); 4] = [
        (
            "different name",
            json!({ "ArchiveName": "token-reuse-other", "ClientToken": "tok-0" }),
        ),
        (
            "different retention",
            json!({ "ArchiveName": "ab", "ClientToken": "tok-0", "Retention": { "RetentionPeriod": "ONE_YEAR" } }),
        ),
        (
            "different kms key",
            json!({
                "ArchiveName": "ab",
                "ClientToken": "tok-0",
                "KmsKeyArn": "arn:aws:kms:eu-west-1:000000000000:key/abcd",
            }),
        ),
        (
            "different tags",
            json!({
                "ArchiveName": "ab",
                "ClientToken": "tok-0",
                "Tags": [{ "Key": "team", "Value": "mail" }],
            }),
        ),
    ];
    for (case, body) in differing {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a ClientToken for a different request must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // ...and none of those refusals created anything.
    let (_, after) = harness.call("ListArchives", json!({})).await;
    assert_eq!(
        after["Archives"].as_array().map(Vec::len),
        Some(names.len()),
        "a refused token reuse must not create an archive, got {after:?}"
    );
}

/// `ListArchives` reports exactly the archives created so far, honours `PageSize` and
/// round-trips `NextToken`: every archive appears exactly once across the pages, and the
/// final page carries no `NextToken`.
#[tokio::test]
async fn list_archives_pages_every_archive_exactly_once() {
    // Rows vary the two things the rule generalises over: how many archives exist and how
    // large a page is — including no archives at all, a page size of 1, a page that
    // divides the set exactly, one that leaves a remainder, and a page larger than the set.
    let cases: [(usize, Option<i32>); 7] = [
        (0, None),
        (0, Some(5)),
        (1, Some(1)),
        (3, Some(2)),
        (4, Some(2)),
        (5, Some(1)),
        (7, Some(50)),
    ];

    for (total, page_size) in cases {
        let harness = Harness::new();
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            created.push(
                harness
                    .create(json!({ "ArchiveName": format!("page-{index:02}") }))
                    .await,
            );
        }
        created.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let (status, out) = harness.call("ListArchives", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected ListArchives to answer 200, got {status} {out:?}",
                (total, page_size, &body)
            );
            let items = out["Archives"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: Archives must be a list, got {out:?}", (total, page_size)))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "case {:?}: a page must not exceed PageSize, got {} items: {out:?}",
                    (total, page_size),
                    items.len()
                );
            }
            for item in &items {
                let id = item["ArchiveId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {:?}: a listed archive must carry ArchiveId, got {item:?}", (total, page_size)))
                    .to_string();
                assert!(
                    item["ArchiveName"].as_str().is_some(),
                    "case {:?}: a listed archive must carry ArchiveName, got {item:?}",
                    (total, page_size)
                );
                assert_eq!(
                    item["ArchiveState"].as_str(),
                    Some("ACTIVE"),
                    "case {:?}: a listed archive must carry its state, got {item:?}",
                    (total, page_size)
                );
                assert!(
                    item["LastUpdatedTimestamp"].as_f64().is_some(),
                    "case {:?}: a listed archive must carry LastUpdatedTimestamp, got {item:?}",
                    (total, page_size)
                );
                seen.push(id);
            }

            pages += 1;
            token = out
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            match &token {
                // A page that reports more to come must have been a FULL page, otherwise
                // the token is reporting work the page could have carried.
                Some(_) => assert_eq!(
                    Some(items.len()),
                    page_size.map(|size| size as usize),
                    "case {:?}: a non-final page must be full, got {} items: {out:?}",
                    (total, page_size),
                    items.len()
                ),
                None => break,
            }
            assert!(
                pages <= total + 1,
                "case {:?}: paging did not terminate after {pages} pages",
                (total, page_size)
            );
        }

        assert_eq!(
            seen, created,
            "case {:?}: paging must yield every archive exactly once, in id order ({pages} pages)",
            (total, page_size)
        );
    }
}

/// `PageSize` is modelled `range(1..50)` and `NextToken` `length(1..2048)`: each published
/// bound is served and a value past it is refused with `ValidationException` (400).
#[tokio::test]
async fn list_archives_enforces_the_modelled_pagination_bounds() {
    let harness = Harness::new();
    for index in 0..3 {
        harness
            .create(json!({ "ArchiveName": format!("bound-{index}") }))
            .await;
    }

    // Published bounds (`smithy_query(query="operation",operation="ListArchives")`).
    for accepted in [1, 2, 49, 50] {
        let (status, out) = harness
            .call("ListArchives", json!({ "PageSize": accepted }))
            .await;
        assert_eq!(
            status, 200,
            "case PageSize={accepted}: a value inside the modelled range must be served, got {status} {out:?}"
        );
    }

    // One past each bound, an out-of-range token length, and a well-formed token that
    // names no archive this service minted.
    let refused: [(&str, Value); 6] = [
        ("PageSize=0", json!({ "PageSize": 0 })),
        ("PageSize=51", json!({ "PageSize": 51 })),
        ("PageSize=-1", json!({ "PageSize": -1 })),
        ("NextToken=\"\"", json!({ "NextToken": "" })),
        ("NextToken=unknown", json!({ "NextToken": "zzzzzzzzzzzzzzzz" })),
        (
            "NextToken=other-run",
            json!({ "NextToken": "00000000000000000000000000000000" }),
        ),
    ];
    for (case, body) in refused {
        let (status, out) = harness.call("ListArchives", body).await;
        assert_eq!(
            status, 400,
            "case {case}: a value outside the modelled bound must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case}: unexpected error code, got {out:?}"
        );
    }
}

/// `UpdateArchive` mutates the stored record and bumps `LastUpdatedTimestamp`; the next
/// `GetArchive` and `ListArchives` observe the new values. Rows vary which members the
/// update names.
#[tokio::test]
async fn update_archive_is_observed_by_later_reads() {
    // (case, what the update sets, expected name, expected retention)
    let cases: [(&str, Value, Option<&str>, &str); 4] = [
        (
            "rename only",
            json!({ "ArchiveName": "renamed-1" }),
            Some("renamed-1"),
            "SIX_MONTHS",
        ),
        (
            "retention only",
            json!({ "Retention": { "RetentionPeriod": "TEN_YEARS" } }),
            None,
            "TEN_YEARS",
        ),
        (
            "both members",
            json!({ "ArchiveName": "renamed-3", "Retention": { "RetentionPeriod": "PERMANENT" } }),
            Some("renamed-3"),
            "PERMANENT",
        ),
        (
            "no member",
            json!({}),
            None,
            "SIX_MONTHS",
        ),
    ];

    for (index, (case, patch, expected_name, expected_retention)) in cases.into_iter().enumerate() {
        let harness = Harness::new();
        let original_name = format!("update-{index}");
        let archive_id = harness
            .create(json!({
                "ArchiveName": original_name,
                "Retention": { "RetentionPeriod": "SIX_MONTHS" },
            }))
            .await;

        let (_, before) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        let created_at = before["CreatedTimestamp"].as_f64().expect("CreatedTimestamp");
        let updated_before = before["LastUpdatedTimestamp"]
            .as_f64()
            .expect("LastUpdatedTimestamp");

        let mut request = patch.clone();
        request["ArchiveId"] = json!(archive_id);
        let (status, out) = harness.call("UpdateArchive", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: UpdateArchive has an empty output body, got {out:?}"
        );

        let (status, after) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetArchive to answer 200, got {status} {after:?}"
        );
        assert_eq!(
            after["ArchiveName"].as_str(),
            Some(expected_name.unwrap_or(original_name.as_str())),
            "case {case:?}: unexpected ArchiveName after the update, got {after:?}"
        );
        assert_eq!(
            after["Retention"]["RetentionPeriod"].as_str(),
            Some(expected_retention),
            "case {case:?}: unexpected Retention after the update, got {after:?}"
        );
        let updated_after = after["LastUpdatedTimestamp"]
            .as_f64()
            .expect("LastUpdatedTimestamp");
        assert!(
            updated_after > updated_before,
            "case {case:?}: LastUpdatedTimestamp must advance, got {updated_before} then {updated_after}"
        );
        assert_eq!(
            after["CreatedTimestamp"].as_f64(),
            Some(created_at),
            "case {case:?}: CreatedTimestamp must not move, got {after:?}"
        );

        // ListArchives reads the same record.
        let (_, listed) = harness.call("ListArchives", json!({})).await;
        let item = listed["Archives"]
            .as_array()
            .and_then(|items| items.first().cloned())
            .unwrap_or_else(|| panic!("case {case:?}: ListArchives reported nothing: {listed:?}"));
        assert_eq!(
            item["ArchiveName"].as_str(),
            Some(expected_name.unwrap_or(original_name.as_str())),
            "case {case:?}: ListArchives must observe the update, got {item:?}"
        );
    }

    // A rename onto a name another archive already holds conflicts, exactly as a create
    // under that name would.
    let harness = Harness::new();
    let taken = harness.create(json!({ "ArchiveName": "taken" })).await;
    let other = harness.create(json!({ "ArchiveName": "other" })).await;
    let (status, out) = harness
        .call(
            "UpdateArchive",
            json!({ "ArchiveId": other, "ArchiveName": "taken" }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case rename-onto-taken: expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case rename-onto-taken: unexpected error code, got {out:?}"
    );
    // ...and the archive that holds the name is untouched.
    let (_, held) = harness
        .call("GetArchive", json!({ "ArchiveId": taken }))
        .await;
    assert_eq!(
        held["ArchiveName"].as_str(),
        Some("taken"),
        "case rename-onto-taken: the holder of the name must be unchanged, got {held:?}"
    );
}

/// An unknown `ArchiveId` is refused by `UpdateArchive` with the `ResourceNotFoundException`
/// it models, and by `DeleteArchive` — which models no such error — with the modelled
/// `ValidationException`.
#[tokio::test]
async fn mutating_an_unknown_archive_is_refused_with_a_modelled_error() {
    let harness = Harness::new();
    harness.create(json!({ "ArchiveName": "present" })).await;

    for case in ["missing", "00000000000000000000000000000000"] {
        let (status, out) = harness
            .call(
                "UpdateArchive",
                json!({ "ArchiveId": case, "ArchiveName": "whatever" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UpdateArchive to answer 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {out:?}"
        );

        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DeleteArchive to answer 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// `DeleteArchive` MARKS the archive `PENDING_DELETION` rather than erasing it: the
/// archive still resolves through `GetArchive` and still appears in `ListArchives` with
/// that state, and no further modification is accepted in that state.
#[tokio::test]
async fn delete_archive_marks_pending_deletion_and_blocks_further_modification() {
    let harness = Harness::new();

    // Rows: the archive deleted straight after create, and one that was renamed first —
    // the state transition does not depend on the record's history.
    for (index, rename) in [false, true].into_iter().enumerate() {
        let case = if rename { "renamed then deleted" } else { "deleted" };
        let name = format!("delete-{index}");
        let archive_id = harness.create(json!({ "ArchiveName": name })).await;
        if rename {
            let (status, out) = harness
                .call(
                    "UpdateArchive",
                    json!({ "ArchiveId": archive_id, "ArchiveName": format!("{name}-renamed") }),
                )
                .await;
            assert_eq!(status, 200, "case {case:?}: rename failed: {status} {out:?}");
        }

        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteArchive to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: DeleteArchive has an empty output body, got {out:?}"
        );

        // Still readable, now PENDING_DELETION.
        let (status, after) = harness
            .call("GetArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pending-deletion archive must still resolve, got {status} {after:?}"
        );
        assert_eq!(
            after["ArchiveState"].as_str(),
            Some("PENDING_DELETION"),
            "case {case:?}: unexpected ArchiveState after delete, got {after:?}"
        );

        // Still listed, with that state.
        let (_, listed) = harness.call("ListArchives", json!({})).await;
        let entry = listed["Archives"]
            .as_array()
            .expect("Archives is a list")
            .iter()
            .find(|item| item["ArchiveId"].as_str() == Some(archive_id.as_str()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a pending-deletion archive must still be listed: {listed:?}")
            });
        assert_eq!(
            entry["ArchiveState"].as_str(),
            Some("PENDING_DELETION"),
            "case {case:?}: ListArchives must report the new state, got {entry:?}"
        );

        // "No further modifications can be made in this state."
        let (status, out) = harness
            .call(
                "UpdateArchive",
                json!({ "ArchiveId": archive_id, "ArchiveName": format!("{name}-again") }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected UpdateArchive on a pending-deletion archive to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );

        // And deleting it again is a conflict, not a second success.
        let (status, out) = harness
            .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a repeat DeleteArchive to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// The name a pending-deletion archive holds is still taken: it is marked, not erased.
#[tokio::test]
async fn a_pending_deletion_archive_still_holds_its_name() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "held-name" })).await;
    let (status, _) = harness
        .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status}");

    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "held-name" }))
        .await;
    assert_eq!(
        status, 409,
        "case reuse-pending-name: expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case reuse-pending-name: unexpected error code, got {out:?}"
    );
}

/// The archive store is BOUNDED: a create past the per-account quota is refused with the
/// modelled `ServiceQuotaExceededException`, and the refusal does not consume or corrupt
/// the archives already stored.
#[tokio::test]
async fn create_archive_refuses_a_create_past_the_archive_quota() {
    let harness = Harness::new();

    // Fill the store exactly to the quota the service declares.
    for index in 0..MAX_ARCHIVES {
        harness
            .create(json!({ "ArchiveName": format!("quota-{index:04}") }))
            .await;
    }

    // Rows: a plain create, and one carrying a ClientToken that was never used — neither
    // may be served past the quota.
    let cases: [(&str, Value); 2] = [
        ("plain", json!({ "ArchiveName": "quota-overflow" })),
        (
            "with a fresh ClientToken",
            json!({ "ArchiveName": "quota-overflow-2", "ClientToken": "never-seen" }),
        ),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a create past the quota must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // The refusals stored nothing, and a retry of an existing archive still works.
    let (_, listed) = harness
        .call("ListArchives", json!({ "PageSize": 50 }))
        .await;
    assert_eq!(
        listed["Archives"].as_array().map(Vec::len),
        Some(50),
        "the first page must still be a full page of stored archives, got {listed:?}"
    );

    // Freeing a slot (the 30-day purge is the only eviction, so delete alone does not)
    // still leaves the quota reached, which is the honest answer.
    let (status, _) = harness
        .call("DeleteArchive", json!({ "ArchiveId": first_archive_id(&harness).await }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status}");
    let (status, out) = harness
        .call("CreateArchive", json!({ "ArchiveName": "quota-after-delete" }))
        .await;
    assert_eq!(
        status, 402,
        "a pending-deletion archive still occupies its slot, got {status} {out:?}"
    );
}

/// The `ArchiveId` of the first archive in `ListArchives` order.
async fn first_archive_id(harness: &Harness) -> String {
    let (_, listed) = harness.call("ListArchives", json!({ "PageSize": 1 })).await;
    listed["Archives"][0]["ArchiveId"]
        .as_str()
        .expect("ListArchives must report at least one archive")
        .to_string()
}

/// `ArchiveName`'s modelled constraints are refused before any handler runs, so a create
/// that would be stored under an invalid name never happens.
#[tokio::test]
async fn create_archive_refuses_names_outside_the_modelled_constraints() {
    let harness = Harness::new();

    // `ArchiveNameString`: len 1..64, pattern `^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$` —
    // one past the length bound, a leading separator, a trailing separator, and a
    // character the pattern excludes.
    let cases: [(&str, Value); 5] = [
        ("65 characters", json!({ "ArchiveName": "a".repeat(65) })),
        ("empty", json!({ "ArchiveName": "" })),
        ("leading dash", json!({ "ArchiveName": "-leading" })),
        ("trailing underscore", json!({ "ArchiveName": "trailing_" })),
        ("excluded character", json!({ "ArchiveName": "has space" })),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateArchive", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ArchiveName outside the model must be refused, got {status} {out:?}"
        );
    }

    // A missing required member is refused too.
    let (status, out) = harness.call("CreateArchive", json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no ArchiveName\": the required member must be enforced, got {status} {out:?}"
    );
}

// ---------------------------------------------------------------------------------------
// The archive's JOB plane: the search and export jobs an archive carries.
// ---------------------------------------------------------------------------------------

/// How long the search suites below wait for a job's completion to fall due before giving
/// up. Generously past [`ARCHIVE_JOB_COMPLETION_MILLIS`], since what is being waited for is
/// a CONDITION (the job reports `COMPLETED`) and not a duration: a loaded host may take
/// much longer than the span to get round to the next call.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(60);

/// How often the search suites look again while waiting for that condition.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The two instants the modelled search range is asked over. Any pair inside
/// `Timestamp`'s range does; these are epoch seconds a day apart.
const RANGE_FROM: f64 = 1_761_000_000.0;
/// See [`RANGE_FROM`].
const RANGE_TO: f64 = 1_761_086_400.0;

/// Whether `candidate` satisfies the lexical form the model's `ArchiveId` declares:
/// `@length 3..66` and `@pattern ^a-[\w]{1,64}$`
/// (`smithy_query(query="shape",shape="ArchiveId")`). `\w` is `[A-Za-z0-9_]`.
///
/// Spelled out rather than compiled: this crate pulls in no regex engine, and the shape is
/// a literal prefix and one character class.
fn is_modelled_archive_id(candidate: &str) -> bool {
    let Some(body) = candidate.strip_prefix("a-") else {
        return false;
    };
    (3..=66).contains(&candidate.len())
        && (1..=64).contains(&body.len())
        && body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// A `StartArchiveSearch` request body over [`RANGE_FROM`]..[`RANGE_TO`].
fn search_request(archive_id: &str, max_results: i64, filters: Option<Value>) -> Value {
    let mut body = json!({
        "ArchiveId": archive_id,
        "FromTimestamp": RANGE_FROM,
        "ToTimestamp": RANGE_TO,
        "MaxResults": max_results,
    });
    if let Some(filters) = filters {
        body["Filters"] = filters;
    }
    body
}

/// The `State` of a `SearchStatus`/`ExportStatus` in a response body.
fn job_state(status: &Value) -> &str {
    status["State"].as_str().unwrap_or("<no State>")
}

/// Start a search on `archive_id` and return its `SearchId`, failing the test on a refusal.
async fn start_search(harness: &Harness, archive_id: &str) -> String {
    let (status, out) = harness
        .call("StartArchiveSearch", search_request(archive_id, 10, None))
        .await;
    assert_eq!(
        status, 200,
        "StartArchiveSearch on {archive_id} must answer 200, got {status} {out:?}"
    );
    out["SearchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartArchiveSearch returned no SearchId: {out:?}"))
        .to_string()
}

/// `GetArchiveSearch` on `search_id`, looked at again until its `State` is `COMPLETED` or
/// [`COMPLETION_DEADLINE`] passes. The CONDITION is waited for, never a fixed time.
async fn poll_until_completed(harness: &Harness, search_id: &str) -> Value {
    let deadline = Instant::now() + COMPLETION_DEADLINE;
    loop {
        let (status, out) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 200,
            "GetArchiveSearch on {search_id} must answer 200, got {status} {out:?}"
        );
        if job_state(&out["Status"]) == SEARCH_STATE_COMPLETED {
            return out;
        }
        assert!(
            Instant::now() < deadline,
            "case {search_id:?}: the search never reported {SEARCH_STATE_COMPLETED}, last read \
             {out:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Place archive export jobs in this service's store, each due to complete at the given
/// instant.
///
/// `StartArchiveExport` is still a scaffold, so this is the only way to hold an export job
/// in the store — and it is what makes the `ListArchiveExports` assertions below a test of
/// a PROJECTION of state rather than of a constant empty list.
fn place_export_jobs(harness: &Harness, archive_id: &str, jobs: &[(&str, i64)]) {
    let store = state::service_state(&*harness.engine).expect("this service's store");
    let mut store = store.lock();
    for (export_id, completes_at_millis) in jobs {
        store.archive_exports.insert(
            (*export_id).to_string(),
            ExportJobRecord {
                export_id: (*export_id).to_string(),
                archive_id: archive_id.to_string(),
                submitted_at_millis: completes_at_millis
                    .saturating_sub(ARCHIVE_JOB_COMPLETION_MILLIS),
                completes_at_millis: *completes_at_millis,
            },
        );
    }
}

/// The `NextToken` the first one-job page of `archive_id`'s export listing carries.
async fn first_export_page_token(harness: &Harness, archive_id: &str) -> String {
    let (status, page) = harness
        .call(
            "ListArchiveExports",
            json!({ "ArchiveId": archive_id, "PageSize": 1 }),
        )
        .await;
    assert_eq!(status, 200, "a first page must answer 200, got {page:?}");
    page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of two jobs must carry a NextToken: {page:?}"))
        .to_string()
}

/// The `ArchiveId` this service mints satisfies the CONSTRAINED `ArchiveId` shape, so every
/// operation that takes the identifier back accepts the value `CreateArchive` handed out.
///
/// The rule covers the whole archive-scoped surface, not one operation: the row set is
/// several freshly minted ids, each fed to every operation this crate serves that binds
/// either `ArchiveId` shape.
#[tokio::test]
async fn a_minted_archive_id_satisfies_the_constrained_archive_id_shape() {
    let harness = Harness::new();

    let mut minted: Vec<String> = Vec::new();
    for name in ["job-plane-a", "job-plane-b", "job-plane-c"] {
        let archive_id = harness.create(json!({ "ArchiveName": name })).await;
        assert!(
            is_modelled_archive_id(&archive_id),
            "case {name:?}: the minted ArchiveId must satisfy the modelled length and \
             pattern, got {archive_id:?}"
        );

        // Every operation that takes an `ArchiveId` must accept it, including the three
        // that bind the CONSTRAINED shape.
        for operation in ["GetArchive", "ListArchiveExports", "ListArchiveSearches"] {
            let (status, out) = harness
                .call(operation, json!({ "ArchiveId": archive_id }))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?} ({operation}): the minted ArchiveId must be accepted, got \
                 {status} {out:?}"
            );
        }
        let (status, out) = harness
            .call("StartArchiveSearch", search_request(&archive_id, 10, None))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?} (StartArchiveSearch): the minted ArchiveId must be accepted, got \
             {status} {out:?}"
        );

        minted.push(archive_id);
    }

    let unique: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        unique.len(),
        minted.len(),
        "two archives must never share an id, got {minted:?}"
    );
}

/// `StartArchiveSearch` records a search job and hands back a `SearchId`, and
/// `GetArchiveSearch` reads the SAME request back: the archive, the range, the result cap
/// and the filters, whatever shape they had.
///
/// Rows span what the rule generalises over: the result cap at both ends of
/// `SearchMaxResults`'s modelled `range 0..1000` and inside it, and every shape of
/// `ArchiveFilters` the model admits (absent, an include condition, an exclude condition,
/// both at once).
#[tokio::test]
async fn start_archive_search_records_a_job_every_read_reports_back() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "search-round-trip" })).await;

    let include = json!({
        "Include": [{
            "StringExpression": {
                "Evaluate": { "Attribute": "SUBJECT" },
                "Operator": "CONTAINS",
                "Values": ["invoice"],
            }
        }]
    });
    let unless = json!({
        "Unless": [{
            "BooleanExpression": {
                "Evaluate": { "Attribute": "HAS_ATTACHMENTS" },
                "Operator": "IS_TRUE",
            }
        }]
    });
    let both = json!({ "Include": include["Include"], "Unless": unless["Unless"] });

    // (case, MaxResults, Filters) — the modelled range's two bounds, a value inside it,
    // and each filter shape.
    let cases: [(&str, i64, Option<Value>); 5] = [
        ("no filters, smallest cap", 0, None),
        ("no filters, largest cap", 1000, None),
        ("an include condition", 25, Some(include)),
        ("an exclude condition", 1, Some(unless)),
        ("both filter lists", 500, Some(both)),
    ];

    let mut search_ids: Vec<String> = Vec::new();
    for (case, max_results, filters) in cases {
        let request = search_request(&archive_id, max_results, filters.clone());
        let (status, out) = harness.call("StartArchiveSearch", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartArchiveSearch must answer 200, got {status} {out:?}"
        );
        let search_id = out["SearchId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no SearchId in {out:?}"))
            .to_string();
        assert!(
            (1..=64).contains(&search_id.len()),
            "case {case:?}: SearchId must satisfy the modelled 1..64 length, got {search_id:?}"
        );

        let (status, read) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetArchiveSearch must answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read["ArchiveId"].as_str(),
            Some(archive_id.as_str()),
            "case {case:?}: the job must report the archive it was started on, got {read:?}"
        );
        assert_eq!(
            read["MaxResults"].as_i64(),
            Some(max_results),
            "case {case:?}: MaxResults must round-trip, got {read:?}"
        );
        assert_eq!(
            read["FromTimestamp"].as_f64(),
            Some(RANGE_FROM),
            "case {case:?}: FromTimestamp must round-trip, got {read:?}"
        );
        assert_eq!(
            read["ToTimestamp"].as_f64(),
            Some(RANGE_TO),
            "case {case:?}: ToTimestamp must round-trip, got {read:?}"
        );
        match filters {
            None => assert!(
                read.get("Filters").is_none_or(Value::is_null),
                "case {case:?}: a job started without filters must report none, got {read:?}"
            ),
            Some(filters) => assert_eq!(
                read["Filters"], filters,
                "case {case:?}: Filters must round-trip, got {read:?}"
            ),
        }

        // The status is an invariant at every instant: a job reports a completion instant
        // exactly when it reports itself completed.
        let state = job_state(&read["Status"]).to_string();
        assert!(
            [SEARCH_STATE_QUEUED, SEARCH_STATE_COMPLETED].contains(&state.as_str()),
            "case {case:?}: a submitted job is queued or completed, got {read:?}"
        );
        assert!(
            read["Status"]["SubmissionTimestamp"]
                .as_f64()
                .is_some_and(|instant| instant > 0.0),
            "case {case:?}: the job must report when it was submitted, got {read:?}"
        );
        assert_eq!(
            read["Status"]["CompletionTimestamp"].as_f64().is_some(),
            state == SEARCH_STATE_COMPLETED,
            "case {case:?}: a CompletionTimestamp is reported exactly when the job is \
             {SEARCH_STATE_COMPLETED}, got {read:?}"
        );

        search_ids.push(search_id);
    }

    // Every job started above is accounted for by the listing, each with its own status.
    let (status, listed) = harness
        .call("ListArchiveSearches", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "ListArchiveSearches must answer 200, got {status} {listed:?}"
    );
    let reported: Vec<&str> = listed["Searches"]
        .as_array()
        .unwrap_or_else(|| panic!("Searches must be a list, got {listed:?}"))
        .iter()
        .map(|summary| {
            assert!(
                summary["Status"]["SubmissionTimestamp"].as_f64().is_some(),
                "every listed search must carry its status, got {summary:?}"
            );
            summary["SearchId"].as_str().unwrap_or("<no SearchId>")
        })
        .collect();
    for search_id in &search_ids {
        assert!(
            reported.contains(&search_id.as_str()),
            "case {search_id:?}: the listing must account for every started search, got \
             {reported:?}"
        );
    }
    assert_eq!(
        reported.len(),
        search_ids.len(),
        "the listing must report exactly the started searches, got {reported:?}"
    );
}

/// A submitted search is not a finished one: it completes when the clock says so, and both
/// the job read and the listing then report the same completion.
#[tokio::test]
async fn a_submitted_search_completes_on_the_clock_and_reports_both_timestamps() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "search-completion" })).await;
    let search_id = start_search(&harness, &archive_id).await;

    let completed = poll_until_completed(&harness, &search_id).await;
    let submitted_at = completed["Status"]["SubmissionTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("no SubmissionTimestamp in {completed:?}"));
    let completed_at = completed["Status"]["CompletionTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("no CompletionTimestamp in {completed:?}"));
    // The span is read off the two instants the service itself reports, not off the test's
    // own clock: a completion that fell due at submission time would mean no caller could
    // ever observe the job running.
    let span_seconds = (ARCHIVE_JOB_COMPLETION_MILLIS as f64) / 1000.0;
    assert!(
        completed_at - submitted_at >= span_seconds,
        "case {search_id:?}: completion must fall due a whole span after submission, got \
         {submitted_at} then {completed_at}"
    );

    // Reading a finished job again reports the same instants: nothing moved because it was
    // read.
    let again = poll_until_completed(&harness, &search_id).await;
    assert_eq!(
        again["Status"], completed["Status"],
        "case {search_id:?}: a completed job's status must not move between reads, got \
         {again:?}"
    );

    let (status, listed) = harness
        .call("ListArchiveSearches", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "ListArchiveSearches must answer 200, got {status} {listed:?}"
    );
    let summary = &listed["Searches"][0];
    assert_eq!(
        summary["SearchId"].as_str(),
        Some(search_id.as_str()),
        "the listing must report the started search, got {listed:?}"
    );
    assert_eq!(
        job_state(&summary["Status"]),
        SEARCH_STATE_COMPLETED,
        "the listing must report the completion the job read reports, got {listed:?}"
    );
    assert_eq!(
        summary["Status"], completed["Status"],
        "the listing and the job read must report ONE status, got {listed:?}"
    );
}

/// `ListArchiveExports` projects the export jobs the store holds for the archive named:
/// zero of them is an empty list and a success, and each job carries the state its
/// completion instant gives it.
#[tokio::test]
async fn list_archive_exports_projects_the_export_jobs_of_its_archive() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "export-listing" })).await;
    let other_archive = harness.create(json!({ "ArchiveName": "export-listing-other" })).await;

    // An archive on which no export has been started.
    let (status, out) = harness
        .call("ListArchiveExports", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "a listing of zero export jobs is a success, got {status} {out:?}"
    );
    assert_eq!(
        out["Exports"],
        json!([]),
        "Exports must be present and empty, got {out:?}"
    );
    assert!(
        out.get("NextToken").is_none_or(Value::is_null),
        "an exhausted listing carries no NextToken, got {out:?}"
    );

    // Rows: one job whose completion already fell due and two that are still due, so both
    // modelled states are covered without waiting for either.
    let now = state::now_millis();
    let jobs: [(&str, i64); 3] = [
        ("ex-0001", now - ARCHIVE_JOB_COMPLETION_MILLIS),
        ("ex-0002", now + 60_000),
        ("ex-0003", now + 60_000),
    ];
    place_export_jobs(&harness, &archive_id, &jobs);
    // A job of ANOTHER archive must not be reported by this archive's listing.
    place_export_jobs(&harness, &other_archive, &[("ex-9999", now + 60_000)]);

    let (status, out) = harness
        .call("ListArchiveExports", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(
        status, 200,
        "ListArchiveExports must answer 200, got {status} {out:?}"
    );
    let reported = out["Exports"]
        .as_array()
        .unwrap_or_else(|| panic!("Exports must be a list, got {out:?}"));
    assert_eq!(
        reported.len(),
        jobs.len(),
        "the listing must report exactly this archive's jobs, got {out:?}"
    );
    for (export_id, completes_at_millis) in jobs {
        let summary = reported
            .iter()
            .find(|summary| summary["ExportId"].as_str() == Some(export_id))
            .unwrap_or_else(|| panic!("case {export_id:?}: not reported, got {out:?}"));
        let expected_state = if completes_at_millis <= now {
            EXPORT_STATE_COMPLETED
        } else {
            EXPORT_STATE_QUEUED
        };
        assert_eq!(
            job_state(&summary["Status"]),
            expected_state,
            "case {export_id:?}: expected state {expected_state}, got {summary:?}"
        );
        assert_eq!(
            summary["Status"]["CompletionTimestamp"].as_f64().is_some(),
            expected_state == EXPORT_STATE_COMPLETED,
            "case {export_id:?}: a CompletionTimestamp is reported exactly when the job is \
             {EXPORT_STATE_COMPLETED}, got {summary:?}"
        );
    }

    // Paging walks the same jobs exactly once, whatever the page size.
    for page_size in [1_usize, 2, 3, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "ArchiveId": archive_id, "PageSize": page_size });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, page) = harness.call("ListArchiveExports", request).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: a page must answer 200, got {status} {page:?}"
            );
            let items = page["Exports"]
                .as_array()
                .unwrap_or_else(|| panic!("case {page_size:?}: no Exports in {page:?}"));
            assert!(
                items.len() <= page_size,
                "case {page_size:?}: a page must not exceed the size asked for, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|item| item["ExportId"].as_str().unwrap_or_default().to_string()),
            );
            next_token = page["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                "ex-0001".to_string(),
                "ex-0002".to_string(),
                "ex-0003".to_string()
            ],
            "case {page_size:?}: paging must walk every job exactly once, got {seen:?}"
        );
    }
}

/// An archive-scoped job operation refuses an `ArchiveId` that names no archive the same
/// way whichever operation was called, and refuses one outside the modelled shape before
/// any handler runs.
#[tokio::test]
async fn the_job_operations_refuse_an_archive_id_that_names_no_archive() {
    let harness = Harness::new();

    // Rows: values inside `ArchiveId`'s modelled shape that this service never minted —
    // the shortest the 3..66 length admits, a plausible minted-looking one, and the
    // longest.
    let unknown: [(&str, String); 3] = [
        ("the shortest id the length admits", "a-x".to_string()),
        (
            "a plausible minted id",
            format!("a-{}", "0123456789abcdef".repeat(2)),
        ),
        ("the longest id the length admits", format!("a-{}", "w".repeat(64))),
    ];
    for (case, archive_id) in &unknown {
        for operation in ["ListArchiveExports", "ListArchiveSearches"] {
            let (status, out) = harness
                .call(operation, json!({ "ArchiveId": archive_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): an unknown archive is refused as a request \
                 fault, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?} ({operation}): unexpected error code, got {out:?}"
            );
            assert_eq!(
                out["Message"].as_str(),
                Some(state::ARCHIVE_NOT_FOUND),
                "case {case:?} ({operation}): unexpected message, got {out:?}"
            );
        }
        let (status, out) = harness
            .call("StartArchiveSearch", search_request(archive_id, 10, None))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} (StartArchiveSearch): an unknown archive is refused, got {status} \
             {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?} (StartArchiveSearch): unexpected error code, got {out:?}"
        );
    }

    // Rows: values OUTSIDE the modelled shape — no `a-` prefix, a character the pattern
    // excludes, and one past the length. These are refused by the boundary validator,
    // which is what makes the minted form above a requirement.
    let malformed: [(&str, String); 4] = [
        ("no prefix", "0123456789abcdef".repeat(2)),
        ("a dash in the body", "a-not-a-word-body".to_string()),
        ("too short", "a-".to_string()),
        ("one past the length", format!("a-{}", "w".repeat(65))),
    ];
    for (case, archive_id) in &malformed {
        for operation in ["ListArchiveExports", "ListArchiveSearches"] {
            let (status, out) = harness
                .call(operation, json!({ "ArchiveId": archive_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): an ArchiveId outside the model must be refused, \
                 got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?} ({operation}): unexpected error code, got {out:?}"
            );
        }
    }
}

/// `PageSize` is enforced at both published bounds for the job listings, and a value past
/// either is refused before any handler runs.
#[tokio::test]
async fn the_job_listings_enforce_the_modelled_page_size_range() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "job-page-size" })).await;

    // `PageSize` is `range 1..50`: both bounds are served, both values one past are not.
    for operation in ["ListArchiveExports", "ListArchiveSearches"] {
        for page_size in [1, 50] {
            let (status, out) = harness
                .call(
                    operation,
                    json!({ "ArchiveId": archive_id, "PageSize": page_size }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {page_size} ({operation}): a page size the model admits must be served, \
                 got {status} {out:?}"
            );
        }
        for page_size in [0, 51] {
            let (status, out) = harness
                .call(
                    operation,
                    json!({ "ArchiveId": archive_id, "PageSize": page_size }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {page_size} ({operation}): a page size past the modelled range must be \
                 refused, got {status} {out:?}"
            );
        }
    }
}

/// A `NextToken` is the listing's OWN output: a value it never issued for THIS archive's
/// walk names no position in it and is refused.
#[tokio::test]
async fn the_job_listings_refuse_a_continuation_token_they_did_not_issue() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "job-token-scope" })).await;
    let other_archive = harness.create(json!({ "ArchiveName": "job-token-scope-other" })).await;

    // Two jobs on each archive, so each listing really issues a token for its own walk.
    let now = state::now_millis();
    place_export_jobs(
        &harness,
        &archive_id,
        &[("tok-0001", now - 1), ("tok-0002", now - 1)],
    );
    place_export_jobs(
        &harness,
        &other_archive,
        &[("tok-8001", now - 1), ("tok-8002", now - 1)],
    );

    let mine = first_export_page_token(&harness, &archive_id).await;
    let another_archives = first_export_page_token(&harness, &other_archive).await;

    // The token this walk issued is accepted, and resumes it.
    let (status, page) = harness
        .call(
            "ListArchiveExports",
            json!({ "ArchiveId": archive_id, "PageSize": 1, "NextToken": mine }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the walk's own token\": must resume the walk, got {status} {page:?}"
    );

    // Rows: a value that is not a token at all, one this store never authenticated, a
    // truncated one, and one issued for ANOTHER archive's walk.
    let cases: [(&str, String); 4] = [
        ("not a token at all", "not-a-token".to_string()),
        ("an unauthenticated cursor", hex::encode("a-x\u{1f}ex-0001")),
        ("a truncated token", mine[..mine.len() - 2].to_string()),
        ("another archive's token", another_archives),
    ];
    for (case, token) in cases {
        let (status, out) = harness
            .call(
                "ListArchiveExports",
                json!({ "ArchiveId": archive_id, "NextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token this walk never issued must be refused, got {status} \
             {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// An archive marked for deletion serves no job operations, and its jobs are no longer
/// reachable: the archive takes its search and export plane with it.
#[tokio::test]
async fn a_pending_deletion_archive_serves_no_job_operations() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "job-plane-deleted" })).await;
    let search_id = start_search(&harness, &archive_id).await;
    let now = state::now_millis();
    place_export_jobs(&harness, &archive_id, &[("del-0001", now - 1)]);

    let (status, out) = harness
        .call("DeleteArchive", json!({ "ArchiveId": archive_id }))
        .await;
    assert_eq!(status, 200, "DeleteArchive must answer 200, got {status} {out:?}");

    for operation in ["ListArchiveExports", "ListArchiveSearches"] {
        let (status, out) = harness
            .call(operation, json!({ "ArchiveId": archive_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: an archive that is not active serves no listing, got {status} \
             {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {operation:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["Message"].as_str(),
            Some(state::ARCHIVE_NOT_ACTIVE),
            "case {operation:?}: unexpected message, got {out:?}"
        );
    }

    let (status, out) = harness
        .call("StartArchiveSearch", search_request(&archive_id, 10, None))
        .await;
    assert_eq!(
        status, 400,
        "case \"StartArchiveSearch\": an archive that is not active takes no new job, got \
         {status} {out:?}"
    );

    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": search_id }))
        .await;
    assert_eq!(
        status, 400,
        "case \"GetArchiveSearch\": a job of an archive that is not active is unreachable, got \
         {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"GetArchiveSearch\": unexpected error code, got {out:?}"
    );
    assert_eq!(
        out["Message"].as_str(),
        Some(SEARCH_NOT_FOUND),
        "case \"GetArchiveSearch\": unexpected message, got {out:?}"
    );
}

/// A `SearchId` this service never minted is refused, whatever shape it has inside the
/// modelled 1..64 length.
#[tokio::test]
async fn get_archive_search_refuses_a_search_id_it_never_minted() {
    let harness = Harness::new();
    // Rows: both ends of `SearchId`'s modelled 1..64 length and a plausible minted-looking
    // value in between.
    let cases: [(&str, String); 3] = [
        ("one character", "x".to_string()),
        ("a plausible minted id", "0123456789abcdef".repeat(2)),
        ("the longest id the length admits", "s".repeat(64)),
    ];
    for (case, search_id) in cases {
        let (status, out) = harness
            .call("GetArchiveSearch", json!({ "SearchId": search_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown SearchId must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["Message"].as_str(),
            Some(SEARCH_ID_INVALID),
            "case {case:?}: unexpected message, got {out:?}"
        );
    }

    // One past the modelled length is refused before any handler runs.
    let (status, out) = harness
        .call("GetArchiveSearch", json!({ "SearchId": "s".repeat(65) }))
        .await;
    assert_eq!(
        status, 400,
        "case \"one past the length\": the modelled length must be enforced, got {status} \
         {out:?}"
    );
}

/// The search-job store is BOUNDED: a submit past the quota is refused with the modelled
/// `ServiceQuotaExceededException` and records nothing.
#[tokio::test]
async fn start_archive_search_refuses_a_submit_past_the_search_quota() {
    let harness = Harness::new();
    let archive_id = harness.create(json!({ "ArchiveName": "search-quota" })).await;

    // Filled through the store rather than through the operation: the quota is what is
    // under test, not the thousand submits it takes to reach it.
    {
        let store = state::service_state(&*harness.engine).expect("this service's store");
        let mut store = store.lock();
        let now = state::now_millis();
        for index in 0..state::MAX_ARCHIVE_SEARCHES {
            let search_id = format!("quota-{index:06}");
            store.archive_searches.insert(
                search_id.clone(),
                state::SearchJobRecord {
                    search_id,
                    archive_id: archive_id.clone(),
                    filters: None,
                    from_timestamp: aws_smithy_types::DateTime::from_millis(now),
                    to_timestamp: aws_smithy_types::DateTime::from_millis(now),
                    max_results: 1,
                    submitted_at_millis: now,
                    completes_at_millis: now,
                },
            );
        }
    }

    let (status, out) = harness
        .call("StartArchiveSearch", search_request(&archive_id, 10, None))
        .await;
    assert_eq!(
        status, 402,
        "a submit past the search quota must be refused, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ServiceQuotaExceededException",
        "unexpected error code, got {out:?}"
    );

    let (status, listed) = harness
        .call(
            "ListArchiveSearches",
            json!({ "ArchiveId": archive_id, "PageSize": 50 }),
        )
        .await;
    assert_eq!(status, 200, "the listing must still be served, got {listed:?}");
    assert_eq!(
        listed["Searches"].as_array().map(Vec::len),
        Some(50),
        "the refusal must not have disturbed the jobs already stored, got {listed:?}"
    );
}
