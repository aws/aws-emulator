// SPDX-License-Identifier: CC0-1.0

//! The neptunedata INDEX REBUILD endpoints, end to end over HTTP: `StartIndexRebuild`,
//! `GetIndexRebuildStatus` and `CancelIndexRebuild`.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The suite shares the engine, the router and the request helpers of its
//! parent module, `tests/all/neptune_data_plane.rs`.
//!
//! The rule under test: the three endpoints compute every answer from the ONE rebuild record
//! the start wrote — the scope, the concurrency (capped at the writer instance's vCPU count)
//! and the write-buffer size the request settled, and the status, percentage and instants
//! the record's stored instants make due. So a read reports the job the start admitted, a
//! retry carrying the same `clientToken` reaches that same job, a second rebuild while one
//! is in flight is refused, and a cancel marks the record without removing it, which is what
//! makes it idempotent.
//!
//! The tables below span what the rule generalises over: every `Scope`, every
//! `WriteBufferSize`, an absent member against a carried one, a concurrency below, at and
//! far above what the writer instance affords, how far into its spans a rebuild is, and
//! which shapes of id name no rebuild at all.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_neptunedata_amazonneptunedataplane::state::{
    ComAmazonawsNeptunedataAmazonneptunedataplaneState, IndexRebuildRecord, RebuildCancellation,
};
use smithy_server_runtime::MultiServiceRouter;

use super::{call, emulator, Answer, Emulator, ANNOUNCED_MEDIA_TYPE};

/// The route the index rebuilds are reached at. Source: smithy_query(query="digest") —
/// `StartIndexRebuild` is `POST /storage/indexes/rebuilds`, `GetIndexRebuildStatus` is
/// `GET /storage/indexes/rebuilds/{rebuildId}` and `CancelIndexRebuild` is
/// `POST /storage/indexes/rebuilds/{rebuildId}/cancel`.
const REBUILDS_PATH: &str = "/storage/indexes/rebuilds";

/// The `RebuildStatus` values a rebuild of this engine passes through, spelled out here
/// rather than imported so a wrong spelling in `src/state/index_rebuild.rs` fails this test.
/// Source: smithy_query(query="shape", shape="RebuildStatus").
const PREPARING: &str = "PREPARING";
const REBUILDING: &str = "REBUILDING";
const COMPLETED: &str = "COMPLETED";
const CANCELLING: &str = "CANCELLING";
const CANCELLED: &str = "CANCELLED";

/// Every value `Scope` and `WriteBufferSize` declare, and the default each member documents
/// ("`ALL` … is the default if not specified", "`LOW` … This is the default"), spelled out
/// here for the same reason. Sources: smithy_query(query="shape", shape="Scope") and
/// smithy_query(query="shape", shape="WriteBufferSize").
const SCOPES: [&str; 3] = ["ALL", "QUADS", "DICTIONARIES"];
const WRITE_BUFFER_SIZES: [&str; 3] = ["LOW", "MEDIUM", "HIGH"];
const DEFAULT_SCOPE: &str = "ALL";
const DEFAULT_WRITE_BUFFER_SIZE: &str = "LOW";

/// `StartIndexRebuildInput$concurrency`'s documented default: "If not specified, defaults
/// to 1".
const DEFAULT_CONCURRENCY: i64 = 1;

/// The spans this engine's rebuild passes its stages in, in seconds: preparing, then
/// rebuilding, and — once a cancel is accepted — cancelling. The seeded records below are
/// placed around them, so a wrong span in `src/state/index_rebuild.rs` fails this test.
const PREPARATION_SECS: u64 = 3;
const CANCELLATION_SECS: u64 = 3;

/// `StartIndexRebuild` with the members it takes in the query string, and an optional JSON
/// body (which is where `clientToken` is carried).
async fn start_rebuild(router: &MultiServiceRouter, query: &str, body: Option<Value>) -> Answer {
    call(router, "POST", &format!("{REBUILDS_PATH}{query}"), body).await
}

async fn rebuild_status(router: &MultiServiceRouter, rebuild_id: &str) -> Answer {
    call(router, "GET", &format!("{REBUILDS_PATH}/{rebuild_id}"), None).await
}

async fn cancel_rebuild(router: &MultiServiceRouter, rebuild_id: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("{REBUILDS_PATH}/{rebuild_id}/cancel"),
        None,
    )
    .await
}

/// The concurrency ceiling this engine caps a request at — the writer instance's vCPU count
/// — read back from the engine itself by asking for more than any instance has, because the
/// instance is this process and the count is the parallelism its host affords it.
async fn concurrency_ceiling() -> i64 {
    let Emulator { router, .. } = emulator();
    let answer = start_rebuild(&router, &format!("?concurrency={}", i32::MAX), None).await;
    assert_eq!(
        answer.status, 200,
        "the ceiling probe must be admitted, got {} with body {:?}",
        answer.status, answer.body
    );
    let ceiling = answer.body["concurrency"].as_i64().unwrap_or_default();
    assert!(
        ceiling >= 1,
        "the writer instance has at least one vCPU, got {ceiling:?} in {:?}",
        answer.body
    );
    ceiling
}

/// Assert a refusal is the modelled error `code`, carrying the four members every refusal of
/// this plane carries, and that its text names `names`.
fn assert_rebuild_refusal(case: &str, answer: &Answer, status: u16, code: &str, names: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status}, got {} with body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().rsplit('#').next(),
        Some(code),
        "case {case}: expected the modelled {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["code"].as_str(),
        Some(code),
        "case {case}: code carries the exception class, got {:?}",
        answer.body["code"]
    );
    let detail = answer.body["detailedMessage"].as_str().unwrap_or_default();
    assert!(
        detail.contains(names),
        "case {case}: the message must name {names:?}, got {detail:?}"
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(detail),
        "case {case}: the refusal's text goes out under `message` too, got {:?}",
        answer.body["message"]
    );
    assert!(
        !answer.body["requestId"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case {case}: the error must name its request, got {:?}",
        answer.body["requestId"]
    );
}

/// One start: which of the three members the request carried, and what the answer must then
/// report. The rows differ along everything the rule generalises over — every scope, every
/// write-buffer size, an absent member against a carried one, and a concurrency below, at
/// and far above what the writer instance affords.
#[derive(Debug, Clone, Copy)]
struct RebuildCase {
    case: &'static str,
    scope: Option<&'static str>,
    write_buffer_size: Option<&'static str>,
    concurrency: Option<i64>,
}

/// The query string a case asks for, with only the members it carries.
fn rebuild_query(case: &RebuildCase) -> String {
    let mut asked: Vec<String> = Vec::new();
    if let Some(scope) = case.scope {
        asked.push(format!("scope={scope}"));
    }
    if let Some(size) = case.write_buffer_size {
        asked.push(format!("writeBufferSize={size}"));
    }
    if let Some(concurrency) = case.concurrency {
        asked.push(format!("concurrency={concurrency}"));
    }
    if asked.is_empty() {
        String::new()
    } else {
        format!("?{}", asked.join("&"))
    }
}

/// Put one rebuild in the registry, admitted `started_secs_ago` seconds ago and — when
/// `cancel_secs_ago` says so — with a cancel accepted that many seconds ago, the way the two
/// endpoints that write the registry would have left it.
fn seed_rebuild(
    engine: &engine::InMemoryEngine,
    rebuild_id: &str,
    started_secs_ago: u64,
    cancel_secs_ago: Option<u64>,
) {
    let now = SystemTime::now();
    let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let mut store = state.lock();
    let mut record = IndexRebuildRecord::admitted(
        rebuild_id,
        "QUADS",
        1,
        "MEDIUM",
        None,
        now - Duration::from_secs(started_secs_ago),
    );
    record.cancel = cancel_secs_ago.map(|secs| {
        let requested_at = now - Duration::from_secs(secs);
        RebuildCancellation {
            requested_at,
            cancelled_at: requested_at + Duration::from_secs(CANCELLATION_SECS),
        }
    });
    store.index_rebuilds.admit(record, now);
}

#[tokio::test]
async fn a_rebuild_is_admitted_with_what_the_request_settled_and_read_back_from_the_record() {
    let ceiling = concurrency_ceiling().await;
    let cases = [
        RebuildCase {
            case: "nothing carried: every documented default",
            scope: None,
            write_buffer_size: None,
            concurrency: None,
        },
        RebuildCase {
            case: "every member carried",
            scope: Some(SCOPES[1]),
            write_buffer_size: Some(WRITE_BUFFER_SIZES[2]),
            concurrency: Some(1),
        },
        RebuildCase {
            case: "the dictionaries scope, with the middle buffer size",
            scope: Some(SCOPES[2]),
            write_buffer_size: Some(WRITE_BUFFER_SIZES[1]),
            concurrency: Some(2),
        },
        RebuildCase {
            case: "the whole store, with the smallest buffer",
            scope: Some(SCOPES[0]),
            write_buffer_size: Some(WRITE_BUFFER_SIZES[0]),
            concurrency: None,
        },
        RebuildCase {
            case: "a scope with no buffer size",
            scope: Some(SCOPES[1]),
            write_buffer_size: None,
            concurrency: Some(ceiling),
        },
        RebuildCase {
            case: "a concurrency beyond what any writer instance affords",
            scope: None,
            write_buffer_size: Some(WRITE_BUFFER_SIZES[2]),
            concurrency: Some(i64::from(i32::MAX)),
        },
    ];

    for case in &cases {
        // One emulator per row: the storage layer rebuilds one job at a time, so each row
        // is the first rebuild of its own engine.
        let Emulator { router, .. } = emulator();
        let answer = start_rebuild(&router, &rebuild_query(case), None).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the start must be admitted, got {} with body {:?}",
            case.case, answer.status, answer.body
        );

        let expected_scope = case.scope.unwrap_or(DEFAULT_SCOPE);
        let expected_size = case.write_buffer_size.unwrap_or(DEFAULT_WRITE_BUFFER_SIZE);
        // "If the value exceeds the available vCPUs, it is capped at the maximum instead of
        // returning an InvalidParameterException. The effective (capped) value is returned
        // in the response."
        let expected_concurrency = case.concurrency.unwrap_or(DEFAULT_CONCURRENCY).min(ceiling);

        let rebuild_id = answer.body["rebuildId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !rebuild_id.is_empty(),
            "case {:?}: the answer must name the rebuild it started, got {:?}",
            case.case,
            answer.body
        );
        assert_eq!(
            (
                answer.body["status"].as_str(),
                answer.body["scope"].as_str(),
                answer.body["writeBufferSize"].as_str(),
                answer.body["concurrency"].as_i64(),
            ),
            (
                Some(PREPARING),
                Some(expected_scope),
                Some(expected_size),
                Some(expected_concurrency),
            ),
            "case {:?}: expected a rebuild preparing over {expected_scope} with \
             {expected_size} and concurrency {expected_concurrency}, got {:?}",
            case.case,
            answer.body
        );

        // Read the job back through the implemented reader: every member is the record's.
        let read = rebuild_status(&router, &rebuild_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the rebuild must be reported, got {} with body {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            (
                read.body["status"].as_str(),
                read.body["scope"].as_str(),
                read.body["writeBufferSize"].as_str(),
                read.body["concurrency"].as_i64(),
                read.body["progressPercentage"].as_i64(),
            ),
            (
                Some(PREPARING),
                Some(expected_scope),
                Some(expected_size),
                Some(expected_concurrency),
                Some(0),
            ),
            "case {:?}: the read must report the record the start wrote, got {:?}",
            case.case,
            read.body
        );
        assert!(
            read.body["startTime"].is_string(),
            "case {:?}: the read must report when the rebuild started, got {:?}",
            case.case,
            read.body["startTime"]
        );
        assert!(
            read.body["estimatedEndTime"].is_string(),
            "case {:?}: a rebuild still running carries its estimate, got {:?}",
            case.case,
            read.body["estimatedEndTime"]
        );
        assert!(
            read.body["endTime"].is_null() && read.body["failureReason"].is_null(),
            "case {:?}: a rebuild that has not ended and has not failed carries neither \
             member, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.content_type, ANNOUNCED_MEDIA_TYPE,
            "case {:?}: expected the announced media type, got {:?}",
            case.case, read.content_type
        );
    }
}

#[tokio::test]
async fn a_rebuild_reports_the_stage_its_stored_instants_make_due() {
    let Emulator { router, engine } = emulator();

    // One row per stage, each a rebuild admitted far enough in the past for the stage to be
    // due: preparing, two points inside the rebuild span, finished, long finished, a
    // cancellation in flight and one that has completed. The row's id is its case name.
    let rows: [(&str, u64, Option<u64>, &str); 7] = [
        ("preparing", 0, None, PREPARING),
        ("rebuilding-early", PREPARATION_SECS + 1, None, REBUILDING),
        ("rebuilding-later", PREPARATION_SECS + 2, None, REBUILDING),
        ("finished", PREPARATION_SECS + 4, None, COMPLETED),
        ("long-finished", 600, None, COMPLETED),
        ("cancelling", PREPARATION_SECS + 1, Some(1), CANCELLING),
        ("cancelled", PREPARATION_SECS + 5, Some(4), CANCELLED),
    ];
    for (case, started_secs_ago, cancel_secs_ago, _) in rows {
        seed_rebuild(&engine, case, started_secs_ago, cancel_secs_ago);
    }

    let mut progress_while_rebuilding: Vec<(u64, i64)> = Vec::new();
    for (case, started_secs_ago, _, expected_status) in rows {
        let answer = rebuild_status(&router, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: the seeded rebuild must be reported, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["status"].as_str(),
            Some(expected_status),
            "case {case}: expected {expected_status}, got {:?}",
            answer.body
        );

        let progress = answer.body["progressPercentage"]
            .as_i64()
            .unwrap_or_default();
        let ended = answer.body["endTime"].is_string();
        let estimated = answer.body["estimatedEndTime"].is_string();
        match expected_status {
            PREPARING => {
                assert_eq!(
                    (progress, ended, estimated),
                    (0, false, true),
                    "case {case}: nothing is rebuilt yet, nothing has ended and the \
                     estimate stands, got {:?}",
                    answer.body
                );
            }
            REBUILDING => {
                assert!(
                    (0..=99).contains(&progress) && !ended && estimated,
                    "case {case}: a rebuild in progress reports a share short of whole, no \
                     end and an estimate, got {:?}",
                    answer.body
                );
                progress_while_rebuilding.push((started_secs_ago, progress));
            }
            COMPLETED => {
                assert_eq!(
                    (progress, ended, estimated),
                    (100, true, false),
                    "case {case}: a finished rebuild is wholly done and carries the instant \
                     it ended instead of an estimate, got {:?}",
                    answer.body
                );
            }
            CANCELLING => {
                assert!(
                    (0..=99).contains(&progress) && !ended && estimated,
                    "case {case}: a cancellation in flight has not ended yet, got {:?}",
                    answer.body
                );
            }
            CANCELLED => {
                assert!(
                    (0..=99).contains(&progress) && ended && !estimated,
                    "case {case}: a cancelled rebuild stopped short of whole and carries \
                     the instant it ended, got {:?}",
                    answer.body
                );
            }
            other => panic!("case {case}: unexpected status {other}"),
        }

        // The answer is a projection of the stored record, so reading twice reports the same
        // stage and never less progress than the read before it.
        let again = rebuild_status(&router, case).await;
        assert_eq!(
            again.body["status"].as_str(),
            Some(expected_status),
            "case {case}: a second read reports the same stage, got {:?}",
            again.body
        );
        assert!(
            again.body["progressPercentage"]
                .as_i64()
                .unwrap_or_default()
                >= progress,
            "case {case}: progress cannot go backwards, got {:?} after {progress}",
            again.body["progressPercentage"]
        );
    }

    // The further into the rebuild span a job is, the more of it is done: the percentage is
    // derived from the record's own instants, not from a counter.
    progress_while_rebuilding.sort();
    for pair in progress_while_rebuilding.windows(2) {
        let [(earlier_secs, earlier), (later_secs, later)] = [pair[0], pair[1]];
        assert!(
            later >= earlier,
            "a rebuild admitted {later_secs}s ago is further on than one admitted \
             {earlier_secs}s ago: got {later} and {earlier}"
        );
    }
}

#[tokio::test]
async fn a_cancel_marks_the_stored_rebuild_and_a_second_cancel_answers_the_same() {
    let Emulator { router, engine } = emulator();

    let started = start_rebuild(
        &router,
        &format!("?scope={}&writeBufferSize={}", SCOPES[1], WRITE_BUFFER_SIZES[1]),
        None,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the start must be admitted, got {} with body {:?}",
        started.status, started.body
    );
    let rebuild_id = started.body["rebuildId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Both cancels answer the same thing: the first accepts the cancellation, the second
    // finds one already in flight and does not restart it.
    for case in ["first cancel", "second cancel"] {
        let answer = cancel_rebuild(&router, &rebuild_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: the cancel must be accepted, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                answer.body["status"].as_str(),
                answer.body["concurrency"].as_i64(),
                answer.body["writeBufferSize"].as_str(),
            ),
            (
                Some(CANCELLING),
                started.body["concurrency"].as_i64(),
                Some(WRITE_BUFFER_SIZES[1]),
            ),
            "case {case}: the cancel reports the rebuild it is stopping, got {:?}",
            answer.body
        );
    }

    // The record was marked, not removed: the reader still reports the id its submitter was
    // given, now as a cancellation in flight.
    let read = rebuild_status(&router, &rebuild_id).await;
    assert_eq!(
        (read.status, read.body["status"].as_str()),
        (200, Some(CANCELLING)),
        "the cancelled rebuild is still reported: got {} with body {:?}",
        read.status,
        read.body
    );

    // A rebuild that has reached a terminal status has nothing left to stop, whichever
    // terminal status it reached.
    seed_rebuild(&engine, "finished", PREPARATION_SECS + 4, None);
    seed_rebuild(
        &engine,
        "already-cancelled",
        PREPARATION_SECS + 5,
        Some(CANCELLATION_SECS + 1),
    );
    for (case, expected_status) in [("finished", COMPLETED), ("already-cancelled", CANCELLED)] {
        let answer = cancel_rebuild(&router, case).await;
        assert_rebuild_refusal(case, &answer, 400, "InvalidParameterException", case);
        let detail = answer.body["detailedMessage"].as_str().unwrap_or_default();
        assert!(
            detail.contains(expected_status),
            "case {case}: the refusal must name the status the rebuild is in, got {detail:?}"
        );
    }
}

#[tokio::test]
async fn one_rebuild_runs_at_a_time_and_a_retried_token_reaches_the_first() {
    let Emulator { router, .. } = emulator();

    let token = "a-client-token-for-one-rebuild";
    let first = start_rebuild(&router, "", Some(serde_json::json!({ "clientToken": token }))).await;
    assert_eq!(
        first.status, 200,
        "the first start must be admitted, got {} with body {:?}",
        first.status, first.body
    );
    let rebuild_id = first.body["rebuildId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The same token is the same request: it reaches the rebuild the first call started
    // rather than a second one.
    let retried =
        start_rebuild(&router, "", Some(serde_json::json!({ "clientToken": token }))).await;
    assert_eq!(
        (retried.status, retried.body["rebuildId"].as_str()),
        (200, Some(rebuild_id.as_str())),
        "a retry carrying the same clientToken must answer the first rebuild, got {} with \
         body {:?}",
        retried.status,
        retried.body
    );

    // A token that differs only in the whitespace around it is a DIFFERENT token — an
    // idempotency token is an opaque identifier — so it is a second job over the data the
    // first is already reorganising, exactly as a start carrying no token is.
    for (case, body) in [
        ("no token", None),
        (
            "another token",
            Some(serde_json::json!({ "clientToken": "a-different-client-token" })),
        ),
        (
            "the same token with surrounding whitespace",
            Some(serde_json::json!({ "clientToken": format!("  {token} ") })),
        ),
    ] {
        let answer = start_rebuild(&router, "", body).await;
        assert_rebuild_refusal(
            case,
            &answer,
            500,
            "ConcurrentModificationException",
            &rebuild_id,
        );
    }

    // A rebuild that has finished is not in flight, so the next one is admitted.
    let Emulator {
        router: later,
        engine,
    } = emulator();
    seed_rebuild(&engine, "finished", PREPARATION_SECS + 4, None);
    let after = start_rebuild(&later, "", None).await;
    assert_eq!(
        (after.status, after.body["status"].as_str()),
        (200, Some(PREPARING)),
        "a start after the held rebuild finished must be admitted, got {} with body {:?}",
        after.status,
        after.body
    );
}

#[tokio::test]
async fn a_rebuild_request_this_engine_cannot_serve_is_refused_before_it_is_admitted() {
    let Emulator { router, .. } = emulator();

    // `scope` and `writeBufferSize` are modelled enums, so a value outside the set is
    // refused by the request walk before the handler runs, and the refusal names the member
    // and the values it takes: the wrong case, a near miss, a blank value and a value of the
    // OTHER member's set.
    for (member, value) in [
        ("scope", "all"),
        ("scope", "QUAD"),
        ("scope", ""),
        ("scope", "LOW"),
        ("writeBufferSize", "low"),
        ("writeBufferSize", "HUGE"),
        ("writeBufferSize", ""),
        ("writeBufferSize", "ALL"),
    ] {
        let case = format!("{member}={value}");
        let answer = start_rebuild(&router, &format!("?{member}={value}"), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: a value the member does not take must be refused, got {} with \
             body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().rsplit('#').next(),
            Some("ValidationException"),
            "case {case}: expected the request walk's refusal, got {:?}",
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        let accepted = if member == "scope" {
            SCOPES
        } else {
            WRITE_BUFFER_SIZES
        };
        assert!(
            message.contains(member),
            "case {case}: the refusal must name the member, got {message:?}"
        );
        for expected in accepted {
            assert!(
                message.contains(expected),
                "case {case}: the refusal must list the values the member accepts, got \
                 {message:?}"
            );
        }
    }

    // There is no number of indexes below one to rebuild, which is the lower bound the model
    // publishes for the member — so that value is refused the same way.
    for concurrency in [0, -1, i64::from(i32::MIN)] {
        let case = format!("concurrency={concurrency}");
        let answer = start_rebuild(&router, &format!("?concurrency={concurrency}"), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: a concurrency below the published minimum must be refused, got {} \
             with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().rsplit('#').next(),
            Some("ValidationException"),
            "case {case}: expected the request walk's refusal, got {:?}",
            answer.body
        );
    }

    // Every refusal above left the registry empty, so a well-formed start is still admitted.
    let admitted = start_rebuild(&router, "", None).await;
    assert_eq!(
        admitted.status, 200,
        "a refused start must leave no rebuild behind, got {} with body {:?}",
        admitted.status, admitted.body
    );
}

#[tokio::test]
async fn an_unknown_rebuild_id_is_refused_by_both_endpoints() {
    let Emulator { router, engine } = emulator();

    // Ids that name no rebuild this engine holds, each with the text its refusal has to
    // name: a well-formed id nobody minted, a value that is not UUID-shaped, one far longer
    // than any id (which the message carries bounded), and the id of a rebuild the engine
    // DOES hold with whitespace around it, because an id is an opaque identifier and not one
    // the engine trims to fit.
    seed_rebuild(&engine, "held-rebuild", 0, None);
    let long_id = "r".repeat(200);
    for (id, named) in [
        (
            "7f1c1a1e-0000-4000-8000-00000000beef",
            "7f1c1a1e-0000-4000-8000-00000000beef",
        ),
        ("not-a-rebuild-id", "not-a-rebuild-id"),
        (long_id.as_str(), "rrrrrrrrrrrrrrrr"),
        ("%20held-rebuild%20", "held-rebuild"),
    ] {
        for (method, answer) in [
            ("GET", rebuild_status(&router, id).await),
            ("POST", cancel_rebuild(&router, id).await),
        ] {
            let case = format!("{method} {id}");
            assert_rebuild_refusal(&case, &answer, 400, "InvalidParameterException", named);
        }
    }

    // Refusing a lookup changed nothing: the rebuild the engine really holds is still
    // reported under the id it was admitted with.
    let held = rebuild_status(&router, "held-rebuild").await;
    assert_eq!(
        (held.status, held.body["status"].as_str()),
        (200, Some(PREPARING)),
        "the held rebuild must still be reported, got {} with body {:?}",
        held.status,
        held.body
    );
}
