// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentence a serverless-cache-snapshot lookup that resolves to
//! nothing is refused with, at every operation that resolves one BY NAME
//! (`DescribeServerlessCacheSnapshots` and `ExportServerlessCacheSnapshot`).
//!
//! The rule under test: a `ServerlessCacheSnapshotName` the account holds no snapshot under
//! is the modelled `ServerlessCacheSnapshotNotFoundFault` (HTTP 404) whose WHOLE message is
//! fixed text for the error class — it states only that no snapshot is held under the
//! specified name, and in particular it does not interpolate the name the request sent — so
//! a client branching on, logging or surfacing that text reads the same sentence whatever it
//! asked for and through whichever operation it asked.
//!
//! The test is a table: the rows span the length, the case and the character form of the
//! name, and include a name that resolves to a serverless CACHE this account does hold (a
//! resource of another kind is still no snapshot), and each row is sent through both
//! operations — so what makes the test pass is the rule, not one recorded request. The
//! contrast rows show the refusal is the rule's and not a blanket one: a snapshot the
//! account DOES hold is served by the describe, and the export of it gets past the
//! not-found check.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader. The expected sentence
//! is written out here on purpose: a test that built it from the same constant the handlers
//! read would pass whatever that constant said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// How ElastiCache words `ServerlessCacheSnapshotNotFoundFault`: the class alone, with no
/// part of the request in it — AWS's own wording, non-standard grammar included.
const SNAPSHOT_NOT_FOUND: &str = "Serverless cache snapshot with specified name does not exists.";

/// The `awsQuery` error code the fault is reported under (`smithy_query(query="operation",
/// operation="ExportServerlessCacheSnapshot")`: `awsQueryError` code
/// `ServerlessCacheSnapshotNotFoundFault`, `httpResponseCode` 404).
const SNAPSHOT_NOT_FOUND_CODE: &str = "ServerlessCacheSnapshotNotFoundFault";

/// The serverless cache the held snapshot is taken of, and the name one row asks for a
/// snapshot under.
const HELD_CACHE: &str = "snapshot-sentence-cache";

/// The snapshot the account really holds, for the contrast rows.
const HELD_SNAPSHOT: &str = "snapshot-sentence-held";

/// A 63-character name, so the sentence is pinned for a long name as well as a short one.
const LONG_NAME: &str = "a-serverless-cache-snapshot-name-of-sixty-three-characters-long";

/// Any bucket name: the not-found check comes before the destination is resolved, so no row
/// of the table depends on the object store holding anything.
const SOME_BUCKET: &str = "elasticache-exports-not-found-sentence";

/// `DescribeServerlessCacheSnapshots` for one snapshot name.
async fn describe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCacheSnapshots",
            &params(&[("ServerlessCacheSnapshotName", name)]),
        )
        .await
}

/// `ExportServerlessCacheSnapshot` for one snapshot name.
async fn export(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "ExportServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", name),
                ("S3BucketName", SOME_BUCKET),
            ]),
        )
        .await
}

/// Create [`HELD_CACHE`], wait for the status its snapshot needs, take
/// [`HELD_SNAPSHOT`] of it and wait for that snapshot to settle too.
async fn hold_one_cache_and_snapshot(emulator: &Emulator) {
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", HELD_CACHE),
                ("Engine", state::DEFAULT_ENGINE),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case the held cache: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    // Only a settled cache can be snapshotted, so the suite waits for that CONDITION
    // rather than for a fixed time.
    poll_until(
        emulator,
        REGION,
        "the held cache",
        &format!("the serverless cache {HELD_CACHE} settling at available"),
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", HELD_CACHE)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;

    let taken = emulator
        .call(
            "CreateServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", HELD_SNAPSHOT),
                ("ServerlessCacheName", HELD_CACHE),
            ]),
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case the held snapshot: CreateServerlessCacheSnapshot must answer 200, got {} {}",
        taken.status, taken.body
    );
    poll_until(
        emulator,
        REGION,
        "the held snapshot",
        &format!("the serverless cache snapshot {HELD_SNAPSHOT} settling at available"),
        "DescribeServerlessCacheSnapshots",
        &params(&[("ServerlessCacheSnapshotName", HELD_SNAPSHOT)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// Assert one reply is the not-found fault worded as the class's own fixed sentence, with
/// no part of the request in it.
fn assert_fixed_not_found(case: &str, operation: &str, requested: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, SNAPSHOT_NOT_FOUND_CODE),
        "case {case:?} / {operation}: expected 404 {SNAPSHOT_NOT_FOUND_CODE}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        SNAPSHOT_NOT_FOUND,
        "case {case:?} / {operation}: expected the class's own sentence {SNAPSHOT_NOT_FOUND:?}, \
         got {:?}",
        reply.error_message()
    );
    assert!(
        !reply.error_message().contains(requested),
        "case {case:?} / {operation}: the sentence must not interpolate the requested name \
         {requested:?}, got {:?}",
        reply.error_message()
    );
}

#[tokio::test]
async fn a_serverless_cache_snapshot_name_that_resolves_to_nothing_states_the_class_alone() {
    let emulator = Emulator::new();
    hold_one_cache_and_snapshot(&emulator).await;

    // The axis: the length, the case and the character form of a name the account holds no
    // snapshot under — including one that names a serverless CACHE it DOES hold, since a
    // resource of another kind is still no snapshot.
    let rows: &[(&str, &str)] = &[
        ("a plain absent name", "no-such-serverless-snapshot"),
        ("the same absent name in upper case", "NO-SUCH-SERVERLESS-SNAPSHOT"),
        ("a single-character name", "z"),
        ("a 63-character name", LONG_NAME),
        ("digits and hyphens", "snapshot-2026-11-11"),
        ("the name of a serverless cache the account holds", HELD_CACHE),
    ];

    let mut sentences = ::std::collections::BTreeSet::new();
    for (case, requested) in rows {
        let described = describe(&emulator, requested).await;
        assert_fixed_not_found(case, "DescribeServerlessCacheSnapshots", requested, &described);
        sentences.insert(described.error_message().to_string());

        let exported = export(&emulator, requested).await;
        assert_fixed_not_found(case, "ExportServerlessCacheSnapshot", requested, &exported);
        sentences.insert(exported.error_message().to_string());
    }
    // Fixed text means ONE sentence over the whole table and both operations.
    assert_eq!(
        sentences.len(),
        1,
        "the fault's message is fixed text, so every row must read the same sentence, got \
         {sentences:?}"
    );

    // The contrast: the snapshot the account DOES hold is served by the describe, and the
    // export of it gets past the not-found check — so the refusals above are the rule's and
    // not a blanket one.
    let held = describe(&emulator, HELD_SNAPSHOT).await;
    assert_eq!(
        held.status, 200,
        "case the held snapshot: the describe must answer 200, got {} {}",
        held.status, held.body
    );
    assert_eq!(
        held.text("ServerlessCacheSnapshotName"),
        Some(HELD_SNAPSHOT),
        "case the held snapshot: expected the describe to report {HELD_SNAPSHOT:?}, got {:?} ({})",
        held.text("ServerlessCacheSnapshotName"),
        held.body
    );
    let exported = export(&emulator, HELD_SNAPSHOT).await;
    assert_ne!(
        (exported.status, exported.error_code()),
        (404, SNAPSHOT_NOT_FOUND_CODE),
        "case the held snapshot: the export must get past the not-found check, got {} {}",
        exported.status,
        exported.body
    );
}
