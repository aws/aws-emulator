// SPDX-License-Identifier: CC0-1.0

//! A retry carrying the `ClientToken` of a successful create answers with the RESULT that
//! create returned, not with a fresh rendering of the snapshot as it stands now.
//!
//! `StartSnapshotRequest$ClientToken` documents "The subsequent retries with the same
//! client token return the result from the original successful request and they have no
//! additional effect"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshotRequest",member="ClientToken",model_path="services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json")`),
//! and the operation itself documents what any such result reports: "The new snapshot
//! enters the pending state after the request completes"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshot",model_path="services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json")`).
//!
//! The rules under test, each over a TABLE of inputs:
//!
//! 1. Whatever happened to the snapshot between the two calls — it was sealed, a declared
//!    count finalised it to `error`, its `Timeout` fell due, a block was written into it —
//!    the retry answers the original body byte for byte, and the retry changes nothing
//!    itself.
//! 2. A retry of a request the store never accepted is not a replay: a token is free until
//!    a call under it succeeded, so the first call under a free token creates.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ebs_ebs::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, elapse, member, put_block, router_with_engine,
    start_snapshot, Reply, BLOCK_SIZE,
};

/// What happens to the snapshot between the original create and the retry.
#[derive(Clone, Copy, Debug)]
enum Interlude {
    /// Nothing at all: the snapshot is still being assembled.
    Nothing,
    /// One block written, and no completion following it.
    BlockWritten,
    /// One block written and a completion declaring that one block: the snapshot is sealed.
    Sealed,
    /// A completion declaring a count nothing wrote, which finalises the snapshot to
    /// `error`.
    CompletionMismatch,
    /// The snapshot's whole `Timeout` window passing with nothing written, which cancels it.
    TimeoutElapsed,
}

/// One row: the create request (the `ClientToken` is added per row), what happens to the
/// snapshot before the retry, and what the STORE then holds for it.
struct ReplayCase {
    case: &'static str,
    request: fn() -> Value,
    interlude: Interlude,
    /// The status the stored record carries once the retry has run.
    live_status: &'static str,
    /// Whether the store already carries `live_status` BEFORE the retry. A `Timeout` that
    /// has fallen due is owed rather than applied, so the request that observes it is what
    /// materialises it.
    materialised_before_the_retry: bool,
    /// The blocks the stored record holds once the retry has run.
    blocks_held: usize,
}

/// The stored status of `snapshot_id`, the blocks it holds, and how many snapshots the
/// store holds in all.
fn stored(
    engine: &engine::InMemoryEngine,
    case: &str,
    snapshot_id: &str,
) -> (&'static str, usize, usize) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    let snapshot = store
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("case {case:?}: {snapshot_id} must be stored"));
    (
        snapshot.status.as_wire(),
        snapshot.blocks.len(),
        store.snapshots.len(),
    )
}

/// The `Timeout`, in minutes, the stored snapshot takes effect with — the window the
/// `TimeoutElapsed` row has to wait out, read from the record rather than assumed.
fn stored_timeout_minutes(engine: &engine::InMemoryEngine, case: &str, snapshot_id: &str) -> u64 {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    let snapshot = store
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("case {case:?}: {snapshot_id} must be stored"));
    u64::try_from(snapshot.timeout_minutes).expect("a positive timeout")
}

/// Write the single block the rows that need one use, asserting the write was accepted.
async fn write_one_block(router: &MultiServiceRouter, case: &str, snapshot_id: &str) {
    let data = block_bytes(7);
    let expected = checksum(&data);
    let written = put_block(router, snapshot_id, 0, data, BLOCK_SIZE, &expected).await;
    assert_eq!(
        written.status, 201,
        "case {case:?}: the block write must answer 201, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
}

/// Apply the row's interlude to the snapshot the original call minted.
async fn apply(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    snapshot_id: &str,
    interlude: Interlude,
) {
    match interlude {
        Interlude::Nothing => {}
        Interlude::BlockWritten => write_one_block(router, case, snapshot_id).await,
        Interlude::Sealed => {
            write_one_block(router, case, snapshot_id).await;
            let sealed = complete_snapshot(router, snapshot_id, 1).await;
            assert_eq!(
                sealed.status, 202,
                "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
                sealed.status,
                String::from_utf8_lossy(&sealed.body)
            );
        }
        Interlude::CompletionMismatch => {
            // Nothing was written, so a declared count of one disagrees with the content.
            let finalised = complete_snapshot(router, snapshot_id, 1).await;
            assert_eq!(
                finalised.status, 202,
                "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
                finalised.status,
                String::from_utf8_lossy(&finalised.body)
            );
        }
        Interlude::TimeoutElapsed => {
            let minutes = stored_timeout_minutes(engine, case, snapshot_id);
            elapse(
                engine,
                case,
                snapshot_id,
                Duration::from_secs(minutes * 60),
            );
        }
    }
}

/// Rows differ along what happened to the snapshot between the calls (nothing, a write, a
/// seal, a completion that finalises it to `error`, a window that cancels it) and along
/// what the create request carries (sizes, a description, tags, encryption, a `Timeout`
/// inside the modelled range and the default no request names).
const REPLAY_CASES: &[ReplayCase] = &[
    ReplayCase {
        case: "nothing happens between the calls",
        request: || json!({ "VolumeSize": 1 }),
        interlude: Interlude::Nothing,
        live_status: "pending",
        materialised_before_the_retry: true,
        blocks_held: 0,
    },
    ReplayCase {
        case: "a block is written and the snapshot is still being assembled",
        request: || json!({ "VolumeSize": 2, "Description": "a producer mid-upload" }),
        interlude: Interlude::BlockWritten,
        live_status: "pending",
        materialised_before_the_retry: true,
        blocks_held: 1,
    },
    ReplayCase {
        case: "the snapshot is sealed before the retry",
        request: || json!({ "VolumeSize": 1, "Tags": [{ "Key": "team", "Value": "backup" }] }),
        interlude: Interlude::Sealed,
        live_status: "completed",
        materialised_before_the_retry: true,
        blocks_held: 1,
    },
    ReplayCase {
        case: "a declared count finalises the snapshot to error before the retry",
        request: || json!({ "VolumeSize": 4, "Encrypted": true, "Timeout": 10 }),
        interlude: Interlude::CompletionMismatch,
        live_status: "error",
        materialised_before_the_retry: true,
        blocks_held: 0,
    },
    ReplayCase {
        case: "the Timeout window passes before the retry",
        request: || json!({ "VolumeSize": 1, "Timeout": 10 }),
        interlude: Interlude::TimeoutElapsed,
        live_status: "error",
        // Owed at the deadline, applied by the next request to observe it.
        materialised_before_the_retry: false,
        blocks_held: 0,
    },
    ReplayCase {
        case: "the default Timeout window passes after a write, before the retry",
        request: || {
            json!({
                "VolumeSize": 8,
                "Description": "left alone for the default window",
                "Tags": [{ "Key": "k", "Value": "v" }],
            })
        },
        interlude: Interlude::TimeoutElapsed,
        live_status: "error",
        materialised_before_the_retry: false,
        blocks_held: 0,
    },
];

/// RULE 1: the retry returns the result of the original successful request — the same body,
/// member for member, carrying the `pending` status that request answered with — however
/// far the snapshot's own life has moved on, and it has no additional effect.
#[tokio::test]
async fn a_retry_answers_the_body_the_original_call_returned() {
    for ReplayCase {
        case,
        request,
        interlude,
        live_status,
        materialised_before_the_retry,
        blocks_held,
    } in REPLAY_CASES
    {
        let (router, engine) = router_with_engine();
        // `ClientToken`'s pattern `^[\S]+$` admits no whitespace, so the row's name is
        // joined into one token.
        let token = format!("token-{}", case.replace(' ', "-"));
        let mut body = request();
        body["ClientToken"] = json!(token);

        let original: Reply = start_snapshot(&router, &body).await;
        assert_eq!(
            original.status, 201,
            "case {case:?}: the create must answer 201, got {} with body {}",
            original.status,
            String::from_utf8_lossy(&original.body)
        );
        assert_eq!(
            member(&original, "Status"),
            "pending",
            "case {case:?}: a create answers for a snapshot that enters the pending state, got {}",
            original.json()
        );
        let snapshot_id = member(&original, "SnapshotId");

        apply(&router, &engine, case, &snapshot_id, *interlude).await;

        // The record really did move where the row says, so the replay below is not just
        // re-reading an unchanged record.
        if *materialised_before_the_retry {
            let (status_before, _, _) = stored(&engine, case, &snapshot_id);
            assert_eq!(
                status_before, *live_status,
                "case {case:?}: the stored snapshot must be {live_status} before the retry, got \
                 {status_before}"
            );
        }

        let retry = start_snapshot(&router, &body).await;
        assert_eq!(
            retry.status, 201,
            "case {case:?}: the retry must answer 201, got {} with body {}",
            retry.status,
            String::from_utf8_lossy(&retry.body)
        );
        assert_eq!(
            retry.json(),
            original.json(),
            "case {case:?}: expected the retry to return the original body {}, got {}",
            original.json(),
            retry.json()
        );
        assert_eq!(
            member(&retry, "Status"),
            "pending",
            "case {case:?}: expected the retry to report the pending status the original call \
             answered with, got {}",
            retry.json()
        );

        // "they have no additional effect": the retry minted no snapshot, moved no status
        // and neither released nor restored a block.
        let (status_after, blocks_after, stored_count) = stored(&engine, case, &snapshot_id);
        assert_eq!(
            status_after, *live_status,
            "case {case:?}: expected the stored snapshot to stay {live_status}, got {status_after}"
        );
        assert_eq!(
            blocks_after, *blocks_held,
            "case {case:?}: expected the stored snapshot to hold {blocks_held} block(s), got \
             {blocks_after}"
        );
        assert_eq!(
            stored_count, 1,
            "case {case:?}: expected the retry to mint no snapshot, got {stored_count} stored"
        );

        // A second retry is the same request again, so it answers the same original result.
        let repeated = start_snapshot(&router, &body).await;
        assert_eq!(
            repeated.json(),
            original.json(),
            "case {case:?}: expected every retry to return the original body {}, got {}",
            original.json(),
            repeated.json()
        );
    }
}

/// RULE 2: only a call that SUCCEEDED leaves an answer to replay. A token a refused request
/// carried names no result, so the next call under it creates — and the answer that call
/// leaves is the one its own retries get, `pending` and all.
#[tokio::test]
async fn a_token_only_replays_the_answer_a_successful_call_left() {
    // Rows differ along why the first call under the token was refused: a `VolumeSize`
    // past the documented maximum ("The maximum size is 65536 GiB (64 TiB)"), a `Timeout`
    // outside the modelled `range` 10..4320, and the two members the model declares
    // mutually exclusive ("You can't specify Encrypted and ParentSnapshotId in the same
    // request").
    let refused: &[(&str, Value)] = &[
        ("a volume past the documented maximum", json!({ "VolumeSize": 65_537 })),
        ("a Timeout under the modelled minimum", json!({ "VolumeSize": 1, "Timeout": 9 })),
        (
            "Encrypted beside a ParentSnapshotId",
            json!({ "VolumeSize": 1, "Encrypted": true, "ParentSnapshotId": "snap-0123456789abcdef0" }),
        ),
    ];

    for (case, body) in refused {
        let (router, engine) = router_with_engine();
        let token = format!("token-{}", case.replace(' ', "-"));
        let mut refused_body = body.clone();
        refused_body["ClientToken"] = json!(token);

        let refusal = start_snapshot(&router, &refused_body).await;
        assert_eq!(
            refusal.status, 400,
            "case {case:?}: the refused create must answer 400, got {} with body {}",
            refusal.status,
            String::from_utf8_lossy(&refusal.body)
        );

        // The token is still free, so this call creates rather than replaying anything.
        let created = start_snapshot(
            &router,
            &json!({ "VolumeSize": 1, "ClientToken": token.clone() }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the token must still create, got {} with body {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let snapshot_id = member(&created, "SnapshotId");

        // And THIS answer is the one the token now replays, even after the snapshot it
        // named has been sealed.
        write_one_block(&router, case, &snapshot_id).await;
        let sealed = complete_snapshot(&router, &snapshot_id, 1).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        let (status_after, _, _) = stored(&engine, case, &snapshot_id);
        assert_eq!(
            status_after, "completed",
            "case {case:?}: the stored snapshot must be completed before the retry, got \
             {status_after}"
        );

        let retry = start_snapshot(
            &router,
            &json!({ "VolumeSize": 1, "ClientToken": token.clone() }),
        )
        .await;
        assert_eq!(
            retry.json(),
            created.json(),
            "case {case:?}: expected the retry to return the created body {}, got {}",
            created.json(),
            retry.json()
        );
    }
}
