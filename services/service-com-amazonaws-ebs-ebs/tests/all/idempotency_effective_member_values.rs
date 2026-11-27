// SPDX-License-Identifier: CC0-1.0

//! `ClientToken` idempotency is scoped to the request that TAKES EFFECT, not to the
//! members the caller happened to spell out.
//!
//! `StartSnapshotRequest$ClientToken` documents "The subsequent retries with the same
//! client token return the result from the original successful request and they have no
//! additional effect", and the modelled `ConflictException` is for "the same client token
//! as a previous, but non-identical request"
//! (`smithy_query(query="doc",shape="StartSnapshotRequest",member="ClientToken",model_path="services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json")`).
//! So for a member the model documents a value for when it is ABSENT — `Timeout`, whose
//! prose is "If no value is specified, the timeout defaults to 60 minutes" — omitting it
//! and naming that value ask for the SAME snapshot, and only a value that differs from the
//! one taking effect is a different request. (A member with no documented default is the
//! other half of the rule and `mutually_exclusive_members.rs` owns it: there absence is
//! itself the value that takes effect, so presence still decides.)
//!
//! The rules under test, each over a TABLE of inputs:
//!
//! 1. Two calls under one token whose EFFECTIVE timeout is the same are one request: the
//!    second replays the first answer and mints nothing, however each call spells that
//!    timeout. Two whose effective timeout differs are the modelled `ConflictException`.
//! 2. A `Timeout` outside the modelled range never reaches the token at all: it is the
//!    constraint refusal, and the token it carried is still free afterwards.

use serde_json::json;
use service_com_amazonaws_ebs_ebs::state::{service_state, DEFAULT_SNAPSHOT_TIMEOUT_MINUTES};

use crate::snapshot_block_loop::{member, router_with_engine, start_snapshot, Reply};

/// The bounds of `StartSnapshotRequest$Timeout`'s modelled `range`, in minutes
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#Timeout",model_path="services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json")`
/// reports `range` 10..4320), so a row can name a legal value that is NOT the default and
/// the refused values just outside them.
const MIN_TIMEOUT_MINUTES: i32 = 10;
const MAX_TIMEOUT_MINUTES: i32 = 4320;

/// One idempotency row: the two requests sent under the same `ClientToken`, and whether
/// they ask for the same snapshot (a replay) or different ones (the modelled conflict).
struct TokenCase<'a> {
    case: &'a str,
    first: serde_json::Value,
    second: serde_json::Value,
    replays: bool,
    /// The timeout, in minutes, the snapshot the FIRST call minted takes effect with.
    effective_minutes: i32,
}

/// The number of snapshots this service's store holds, and the timeout the one named by
/// `snapshot_id` was stored with.
fn stored(engine: &engine::InMemoryEngine, case: &str, snapshot_id: &str) -> (usize, i32) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    let snapshot = store
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("case {case:?}: {snapshot_id} must be stored"));
    (store.snapshots.len(), snapshot.timeout_minutes)
}

/// RULE 1: under one `ClientToken`, two calls asking for the same EFFECTIVE timeout are
/// one request — the second replays the first answer, whichever of them spelled the
/// documented default out — and two asking for different ones conflict.
#[tokio::test]
async fn two_calls_asking_for_the_same_effective_timeout_are_one_request() {
    let default = DEFAULT_SNAPSHOT_TIMEOUT_MINUTES;
    // Rows differ along which call spells the timeout out, the value named (the default,
    // each modelled bound, and values either side of the default), and what else the
    // request carries beside it.
    let cases = [
        TokenCase {
            case: "no timeout then the documented default",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Timeout": default }),
            replays: true,
            effective_minutes: default,
        },
        TokenCase {
            case: "the documented default then no timeout",
            first: json!({ "VolumeSize": 1, "Timeout": default }),
            second: json!({ "VolumeSize": 1 }),
            replays: true,
            effective_minutes: default,
        },
        TokenCase {
            case: "the documented default twice",
            first: json!({ "VolumeSize": 1, "Timeout": default }),
            second: json!({ "VolumeSize": 1, "Timeout": default }),
            replays: true,
            effective_minutes: default,
        },
        TokenCase {
            case: "no timeout twice",
            first: json!({ "VolumeSize": 64 }),
            second: json!({ "VolumeSize": 64 }),
            replays: true,
            effective_minutes: default,
        },
        TokenCase {
            case: "the default spelled out beside every other member, then omitted",
            first: json!({
                "VolumeSize": 2,
                "Description": "the default spelled out",
                "Tags": [{ "Key": "k", "Value": "v" }],
                "Encrypted": true,
                "Timeout": default,
            }),
            second: json!({
                "VolumeSize": 2,
                "Description": "the default spelled out",
                "Tags": [{ "Key": "k", "Value": "v" }],
                "Encrypted": true,
            }),
            replays: true,
            effective_minutes: default,
        },
        TokenCase {
            case: "no timeout then the modelled minimum",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Timeout": MIN_TIMEOUT_MINUTES }),
            replays: false,
            effective_minutes: default,
        },
        TokenCase {
            case: "no timeout then the modelled maximum",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Timeout": MAX_TIMEOUT_MINUTES }),
            replays: false,
            effective_minutes: default,
        },
        TokenCase {
            case: "no timeout then one minute short of the default",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Timeout": default - 1 }),
            replays: false,
            effective_minutes: default,
        },
        TokenCase {
            case: "no timeout then one minute past the default",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Timeout": default + 1 }),
            replays: false,
            effective_minutes: default,
        },
        TokenCase {
            case: "the modelled maximum then the default spelled out",
            first: json!({ "VolumeSize": 1, "Timeout": MAX_TIMEOUT_MINUTES }),
            second: json!({ "VolumeSize": 1, "Timeout": default }),
            replays: false,
            effective_minutes: MAX_TIMEOUT_MINUTES,
        },
        TokenCase {
            case: "the modelled minimum then no timeout",
            first: json!({ "VolumeSize": 1, "Timeout": MIN_TIMEOUT_MINUTES }),
            second: json!({ "VolumeSize": 1 }),
            replays: false,
            effective_minutes: MIN_TIMEOUT_MINUTES,
        },
    ];

    for TokenCase {
        case,
        first,
        second,
        replays,
        effective_minutes,
    } in &cases
    {
        // A router of its own per row, so one row's token can never answer another's.
        let (router, engine) = router_with_engine();
        // `ClientToken`'s pattern `^[\S]+$` admits no whitespace, so the row's name is
        // joined into one token rather than carried verbatim.
        let token = json!(format!("token-{}", case.replace(' ', "-")));
        let mut first = first.clone();
        first["ClientToken"] = token.clone();
        let mut second = second.clone();
        second["ClientToken"] = token;

        let original = start_snapshot(&router, &first).await;
        assert_eq!(
            original.status, 201,
            "case {case:?}: the first call must answer 201, got {} with body {}",
            original.status,
            String::from_utf8_lossy(&original.body)
        );
        let original_id = member(&original, "SnapshotId");

        // The timeout the record takes effect with is the value the second call is judged
        // against, so it is read back from the store before the retry is sent.
        let (_, stored_minutes) = stored(&engine, case, &original_id);
        assert_eq!(
            stored_minutes, *effective_minutes,
            "case {case:?}: the stored snapshot must take effect with {effective_minutes} \
             minutes, got {stored_minutes}"
        );

        let repeat = start_snapshot(&router, &second).await;
        if *replays {
            assert_eq!(
                repeat.status, 201,
                "case {case:?}: the same effective request must replay 201, got {} with body {}",
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert_eq!(
                repeat.json(),
                original.json(),
                "case {case:?}: the replay must answer the original snapshot, got {}",
                repeat.json()
            );
        } else {
            assert_eq!(
                repeat.status, 409,
                "case {case:?}: a different effective request must answer 409, got {} with body {}",
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert!(
                repeat.error_code().ends_with("ConflictException"),
                "case {case:?}: expected the ConflictException error code, got {:?}",
                repeat.error_code()
            );
        }

        // Either way the second call minted nothing: a replay answers the stored snapshot
        // and a conflict creates none.
        let (count, minutes_after) = stored(&engine, case, &original_id);
        assert_eq!(
            count, 1,
            "case {case:?}: the second call must mint no snapshot, got {count} stored beside \
             {original_id}"
        );
        assert_eq!(
            minutes_after, *effective_minutes,
            "case {case:?}: the second call must not change the stored timeout, got \
             {minutes_after}"
        );
    }
}

/// RULE 2: a `Timeout` outside the modelled range is the constraint refusal, so no token
/// row is ever written for it and the token is still free for a legal request.
#[tokio::test]
async fn a_timeout_outside_the_modelled_range_leaves_the_token_free() {
    // One past each published bound of the modelled range, plus the sentinel no legal
    // request can carry.
    let refused: &[i32] = &[MIN_TIMEOUT_MINUTES - 1, MAX_TIMEOUT_MINUTES + 1, 0];

    for timeout in refused {
        let case = format!("Timeout {timeout}");
        let (router, engine) = router_with_engine();
        let token = format!("token-timeout-{timeout}");

        let refusal: Reply = start_snapshot(
            &router,
            &json!({ "VolumeSize": 1, "Timeout": timeout, "ClientToken": token.clone() }),
        )
        .await;
        assert_eq!(
            refusal.status, 400,
            "case {case:?}: a Timeout outside the modelled range must answer 400, got {} with \
             body {}",
            refusal.status,
            String::from_utf8_lossy(&refusal.body)
        );
        assert!(
            refusal.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            refusal.error_code()
        );

        // The refused call stored neither a snapshot nor a token row, so the same token
        // still mints.
        let accepted = start_snapshot(
            &router,
            &json!({ "VolumeSize": 1, "ClientToken": token.clone() }),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: the token must still be free, got {} with body {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        let snapshot_id = member(&accepted, "SnapshotId");
        let (count, minutes) = stored(&engine, &case, &snapshot_id);
        assert_eq!(
            count, 1,
            "case {case:?}: only the accepted call may have minted, got {count} stored"
        );
        assert_eq!(
            minutes, DEFAULT_SNAPSHOT_TIMEOUT_MINUTES,
            "case {case:?}: the accepted call takes the documented default, got {minutes}"
        );
    }
}
