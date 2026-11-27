// SPDX-License-Identifier: CC0-1.0

//! Two request members a service documents as impossible to specify TOGETHER are judged
//! on whether the request SPECIFIED each of them, never on the value either one carries —
//! and the same presence-vs-absence distinction decides whether two requests under one
//! `ClientToken` are the same request.
//!
//! `StartSnapshotRequest$Encrypted` is the pair this service models: "You can't specify
//! Encrypted and ParentSnapshotId in the same request. If you specify both parameters,
//! the request fails with `ValidationException`"
//! (`smithy_query(query="doc",shape="StartSnapshotRequest",member="Encrypted",model_path="services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json")`,
//! repeated on `ParentSnapshotId`). A boolean member is specified when the request
//! carried it, `false` included, so `false` beside a parent is "both parameters".
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production
//! router uses, over the harness `snapshot_block_loop` owns, and asserts a RULE over a
//! TABLE of inputs:
//!
//! 1. A request specifying both members is refused with the modelled
//!    `ValidationException` and the service's fixed sentence for the pair, whatever value
//!    `Encrypted` carries and whatever else the request carried — and the refused call
//!    stores no snapshot.
//! 2. Either member ALONE is accepted and persisted, `Encrypted: false` included, so the
//!    refusal is about the combination rather than about one member or one value.
//! 3. To `ClientToken` idempotency a member the request specified and the same member
//!    omitted are DIFFERENT requests — the modelled `ConflictException` — while a true
//!    repeat replays the first answer. (A member the model documents a value for when it
//!    is absent is the exception, and `idempotency_effective_member_values.rs` owns that
//!    rule: there, omitting it and spelling that value out are one request.)

use serde_json::json;
use service_com_amazonaws_ebs_ebs::state::service_state;

use crate::changed_blocks_between_snapshots::sealed_snapshot;
use crate::sealed_status_and_fixed_refusal_text::PARENT_AND_ENCRYPT_REFUSAL;
use crate::snapshot_block_loop::{member, router_with_engine, start_snapshot};

/// A parent id shaped like one of this service's (`^snap-[0-9a-f]+$`) that no call minted,
/// for the row showing the pair is refused before the parent is ever resolved.
const UNHELD_PARENT_ID: &str = "snap-0123456789abcdef0";

/// A KMS key ARN of the modelled form, for the row that carries a third encryption member
/// beside the two that conflict.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// How many snapshots this service's store holds.
fn stored_snapshot_count(engine: &engine::InMemoryEngine, case: &str) -> usize {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    store.snapshots.len()
}

/// The stored snapshot's encryption, as `(encrypted, sse type)`.
fn stored_encryption(
    engine: &engine::InMemoryEngine,
    case: &str,
    snapshot_id: &str,
) -> (bool, Option<String>) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let store = state.lock();
    let snapshot = store
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("case {case:?}: {snapshot_id} must be stored"));
    (
        snapshot.encrypted,
        snapshot
            .sse_type
            .map(|sse_type| sse_type.as_wire().to_string()),
    )
}

/// RULE 1: specifying both `Encrypted` and `ParentSnapshotId` is the documented
/// `ValidationException`, whichever value `Encrypted` carries — the members conflict by
/// being specified — and nothing is minted for a refused request.
#[tokio::test]
async fn specifying_both_members_is_refused_whatever_encrypted_carries() {
    let (router, engine) = router_with_engine();
    let parent = sealed_snapshot(&router, "the parent fixture", 1, None, &[]).await;
    let before = stored_snapshot_count(&engine, "the parent fixture");

    // Rows differ along the value `Encrypted` carries, whether the parent is one the
    // service holds, the volume size, and which other optional members the request
    // carries beside the two that conflict.
    let cases: &[(&str, serde_json::Value)] = &[
        (
            "Encrypted false with a stored parent",
            json!({ "VolumeSize": 1, "Encrypted": false, "ParentSnapshotId": parent.clone() }),
        ),
        (
            "Encrypted true with a stored parent",
            json!({ "VolumeSize": 1, "Encrypted": true, "ParentSnapshotId": parent.clone() }),
        ),
        (
            "Encrypted false with a parent the service does not hold",
            json!({ "VolumeSize": 64, "Encrypted": false, "ParentSnapshotId": UNHELD_PARENT_ID }),
        ),
        (
            "Encrypted false with a description, tags and a timeout",
            json!({
                "VolumeSize": 4,
                "Encrypted": false,
                "ParentSnapshotId": parent.clone(),
                "Description": "a child that asked for no encryption",
                "Tags": [{ "Key": "k", "Value": "v" }],
                "Timeout": 30,
            }),
        ),
        (
            "Encrypted false with a client token and a KMS key",
            json!({
                "VolumeSize": 2,
                "Encrypted": false,
                "ParentSnapshotId": parent.clone(),
                "ClientToken": "token-false-and-parent",
                "KmsKeyArn": KMS_KEY_ARN,
            }),
        ),
    ];

    for (case, request) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: both members specified must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the parameter-value reason, got {}",
            reply.json()
        );
        let message = member(&reply, "Message");
        assert_eq!(
            message, PARENT_AND_ENCRYPT_REFUSAL,
            "case {case:?}: the refusal's sentence is the service's fixed text, got {message:?}"
        );
        let after = stored_snapshot_count(&engine, case);
        assert_eq!(
            after, before,
            "case {case:?}: a refused request must mint no snapshot, got {after} stored where \
             {before} were"
        );
    }
}

/// RULE 2: either member ALONE is accepted and persisted — `Encrypted: false` is a
/// perfectly good request on its own, and so is a parent without `Encrypted` — so the
/// refusal is about the two being specified together.
#[tokio::test]
async fn either_member_alone_is_accepted_and_persisted() {
    let (router, engine) = router_with_engine();
    let parent = sealed_snapshot(&router, "the parent fixture", 1, None, &[]).await;

    // Rows differ along which of the two members the request carries and, where it is
    // `Encrypted`, the value it carries; the expected encryption is what the accepted
    // request asked for.
    let cases: &[(&str, serde_json::Value, bool, Option<&str>)] = &[
        (
            "Encrypted false alone",
            json!({ "VolumeSize": 1, "Encrypted": false }),
            false,
            None,
        ),
        (
            "Encrypted true alone",
            json!({ "VolumeSize": 1, "Encrypted": true }),
            true,
            Some("sse-ebs"),
        ),
        (
            "a parent alone",
            json!({ "VolumeSize": 1, "ParentSnapshotId": parent.clone() }),
            false,
            None,
        ),
        (
            "neither member",
            json!({ "VolumeSize": 1 }),
            false,
            None,
        ),
        (
            "Encrypted false beside a description",
            json!({ "VolumeSize": 2, "Encrypted": false, "Description": "no encryption asked for" }),
            false,
            None,
        ),
    ];

    for (case, request, encrypted, sse_type) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: one member alone must be accepted 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let snapshot_id = member(&reply, "SnapshotId");
        assert!(
            snapshot_id.starts_with("snap-"),
            "case {case:?}: expected a minted snapshot id, got {snapshot_id:?}"
        );
        let reported = reply.json().get("SseType").cloned();
        let expected_reported = sse_type.map(|value| json!(value));
        assert_eq!(
            reported, expected_reported,
            "case {case:?}: expected SseType {expected_reported:?}, got {reported:?}"
        );
        let stored = stored_encryption(&engine, case, &snapshot_id);
        let expected_stored = (*encrypted, sse_type.map(str::to_string));
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected the stored encryption {expected_stored:?}, got {stored:?}"
        );
    }
}

/// One idempotency row: the two requests sent under the SAME `ClientToken`, and whether
/// the second one is the same request as the first (a replay) or a different one (the
/// modelled conflict).
struct TokenCase<'a> {
    case: &'a str,
    first: serde_json::Value,
    second: serde_json::Value,
    replays: bool,
}

/// RULE 3: `ClientToken` scopes idempotency to the request, and a request that SPECIFIED
/// an optional member the model documents no absent-value default for is not the request
/// that omitted it: the second call conflicts (HTTP 409) exactly when it differs from the
/// first, that member's presence included, and replays the first answer when it does not.
#[tokio::test]
async fn a_specified_member_and_an_omitted_one_are_different_requests() {
    // Rows differ along which optional member the two requests disagree about, which way
    // round the disagreement is, and whether they disagree at all.
    let cases = [
        TokenCase {
            case: "Encrypted false then no Encrypted",
            first: json!({ "VolumeSize": 1, "Encrypted": false }),
            second: json!({ "VolumeSize": 1 }),
            replays: false,
        },
        TokenCase {
            case: "no Encrypted then Encrypted false",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1, "Encrypted": false }),
            replays: false,
        },
        TokenCase {
            case: "Encrypted false then Encrypted true",
            first: json!({ "VolumeSize": 1, "Encrypted": false }),
            second: json!({ "VolumeSize": 1, "Encrypted": true }),
            replays: false,
        },
        TokenCase {
            case: "a description then no description",
            first: json!({ "VolumeSize": 1, "Description": "described" }),
            second: json!({ "VolumeSize": 1 }),
            replays: false,
        },
        TokenCase {
            case: "a timeout then no timeout",
            first: json!({ "VolumeSize": 1, "Timeout": 30 }),
            second: json!({ "VolumeSize": 1 }),
            replays: false,
        },
        TokenCase {
            case: "Encrypted false twice",
            first: json!({ "VolumeSize": 1, "Encrypted": false }),
            second: json!({ "VolumeSize": 1, "Encrypted": false }),
            replays: true,
        },
        TokenCase {
            case: "no Encrypted twice",
            first: json!({ "VolumeSize": 1 }),
            second: json!({ "VolumeSize": 1 }),
            replays: true,
        },
    ];

    for TokenCase {
        case,
        first,
        second,
        replays,
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

        let repeat = start_snapshot(&router, &second).await;
        if *replays {
            assert_eq!(
                repeat.status, 201,
                "case {case:?}: the same request under the same token must replay 201, got {} \
                 with body {}",
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert_eq!(
                repeat.json(),
                original.json(),
                "case {case:?}: the replay must answer the original snapshot"
            );
            let stored = stored_snapshot_count(&engine, case);
            assert_eq!(
                stored, 1,
                "case {case:?}: a replay must mint no second snapshot, got {stored} stored"
            );
        } else {
            assert_eq!(
                repeat.status, 409,
                "case {case:?}: a different request under the same token must answer 409, got {} \
                 with body {}",
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert!(
                repeat.error_code().ends_with("ConflictException"),
                "case {case:?}: expected the ConflictException error code, got {:?}",
                repeat.error_code()
            );
            let stored = stored_snapshot_count(&engine, case);
            assert_eq!(
                stored, 1,
                "case {case:?}: the conflicting call must mint nothing, got {stored} stored \
                 beside {original_id}"
            );
        }
    }
}
