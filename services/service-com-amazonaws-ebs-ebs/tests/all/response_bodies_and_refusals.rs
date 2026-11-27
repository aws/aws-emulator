// SPDX-License-Identifier: CC0-1.0

//! What the EBS direct APIs put in a response BODY: which members a success carries, and
//! how a refusal is worded.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production
//! router uses, over the harness `snapshot_block_loop` owns, and asserts a RULE over a
//! table of inputs rather than one recorded request:
//!
//! 1. An optional response member the stored snapshot does not have is ABSENT, never a
//!    placeholder (`SseType`, `Tags`).
//! 2. `CompleteSnapshot` applies the seal: its 202 acknowledges the status the snapshot
//!    was in when the request arrived, no further block is accepted into it, and every
//!    later call reads `completed` back.
//! 3. A `blockToken` is judged against the expression the SERVICE enforces, and the
//!    refusal reports that expression in the service's own modelled `ValidationException`.
//! 4. A checksum algorithm the service does not support is refused through its own
//!    modelled `ValidationException`, carrying both `Message` and `Reason`.
//! 5. A `ClientToken` conflict carries the service's fixed message and echoes nothing.
//! 6. A `VolumeSize` the service will not accept is refused with its request-derived
//!    template, naming only the rejected value.
//! 7. Every "that snapshot is not stored" refusal uses ONE template, in every operation
//!    that resolves a snapshot id.
//! 8. Every HEADER-bound member whose modelled constraints this service takes off the
//!    request boundary is judged by the operation that owns it, so a value outside the
//!    member's modelled form is refused with the service's OWN modelled
//!    `ValidationException` — carrying both the `Message` and the `Reason` a client reads.
//! 9. The same shape answers EVERY member whose constraints an operation of this service
//!    judges itself, whichever operation owns it and whichever constraint the value
//!    breaks: one envelope for the whole class, never the framework's synthetic one.

use smithy_server_runtime::MultiServiceRouter;

use crate::aggregate_checksum_of_a_completion::linear_aggregate;
use crate::declared_block_inputs::{assert_validation, invalid_volume_size_message};
use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, complete_snapshot_with_algorithm, get_block,
    list_blocks, listed_blocks, member, put_block, put_block_with_algorithm,
    put_block_with_progress, router, send, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// The expression the service enforces on a `blockToken`, which is also the one its
/// refusal reports. Mirrors `crate::state::BLOCK_TOKEN_PATTERN`; a test that quoted a
/// different expression would assert nothing about the refusal a caller reads.
const BLOCK_TOKEN_PATTERN: &str = "^[A-Za-z0-9+/=_-]+$";

/// The `ValidationExceptionReason` this service reports for a value outside the form its
/// member's constraints allow — a rejected parameter value
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationExceptionReason")`).
/// Distinct from `INVALID_BLOCK_TOKEN`, which answers a WELL-FORMED token that names no
/// block.
///
/// `pub(crate)` with the clause and envelope helpers below: every suite asserting a
/// constraint report of this service states it the same way or asserts nothing.
pub(crate) const CONSTRAINT_REFUSAL_REASON: &str = "INVALID_PARAMETER_VALUE";

/// The only checksum algorithm the modelled `ChecksumAlgorithm` enum declares
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ChecksumAlgorithm")`).
pub(crate) const SUPPORTED_ALGORITHM: &str = "SHA256";

/// The widths of the hex suffix of a snapshot id the service accepts, mirroring
/// `crate::state::SNAPSHOT_ID_HEX_WIDTHS`.
const SNAPSHOT_ID_HEX_WIDTHS: [usize; 2] = [8, 17];

/// A snapshot id a request may name that no `StartSnapshot` ever minted, WELL-FORMED so
/// the refusal is about the snapshot and not about the id's shape: `snap-` plus 8 or 17
/// lowercase hex digits, the widths `crate::state::SNAPSHOT_ID_HEX_WIDTHS` names. Each
/// row's width is asserted in the test, so a miscounted literal fails loudly instead of
/// quietly testing the other rule.
const UNSTORED_IDS: &[&str] = &[
    "snap-deadbeef",
    "snap-0123456789abcdef0",
    "snap-00000000000000001",
];

/// RULE 1: an optional member of the response is populated only when the stored snapshot
/// HAS it; otherwise it is absent from the body. `SseType` and `Tags` are optional with
/// no `@default`, so an unencrypted snapshot reports no SSE type and an untagged one
/// reports no tag list — a placeholder (`"none"`, `[]`) would make a presence-based
/// client read "the service reported this" where the service reported nothing.
#[tokio::test]
async fn optional_response_members_are_absent_when_the_snapshot_has_none() {
    let router = router();

    // Rows differ along both members the rule covers: whether the snapshot is encrypted
    // (and how), and how many tags it carries.
    let cases: &[(&str, serde_json::Value, Option<&str>, usize)] = &[
        (
            "unencrypted and untagged",
            serde_json::json!({ "VolumeSize": 1 }),
            None,
            0,
        ),
        (
            "unencrypted with one tag",
            serde_json::json!({
                "VolumeSize": 2,
                "Tags": [{ "Key": "only", "Value": "one" }],
            }),
            None,
            1,
        ),
        (
            "unencrypted with two tags",
            serde_json::json!({
                "VolumeSize": 3,
                "Tags": [
                    { "Key": "a", "Value": "1" },
                    { "Key": "b", "Value": "2" },
                ],
            }),
            None,
            2,
        ),
        (
            "EBS-managed encryption, untagged",
            serde_json::json!({ "VolumeSize": 4, "Encrypted": true }),
            Some("sse-ebs"),
            0,
        ),
        (
            "KMS encryption with a tag",
            serde_json::json!({
                "VolumeSize": 5,
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "Tags": [{ "Key": "kms", "Value": "yes" }],
            }),
            Some("sse-kms"),
            1,
        ),
    ];

    for (case, request, expected_sse, tag_count) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json();

        match expected_sse {
            Some(sse) => assert_eq!(
                body.get("SseType").and_then(serde_json::Value::as_str),
                Some(*sse),
                "case {case:?}: an encrypted snapshot must report its SseType, got {body}"
            ),
            None => assert!(
                body.get("SseType").is_none(),
                "case {case:?}: a snapshot with no SSE type must omit SseType, got {body}"
            ),
        }

        let reported_tags = body.get("Tags").and_then(serde_json::Value::as_array);
        if *tag_count == 0 {
            assert!(
                body.get("Tags").is_none(),
                "case {case:?}: an untagged snapshot must omit Tags, got {body}"
            );
        } else {
            assert_eq!(
                reported_tags.map(Vec::len),
                Some(*tag_count),
                "case {case:?}: the stored tags must be reported, got {body}"
            );
        }
    }
}

/// RULE 2: `CompleteSnapshot` APPLIES the seal and acknowledges the snapshot it sealed.
/// Its 202 reports the status the record was in when the request arrived, the snapshot
/// accepts no further block from that instant, everything written into it is still
/// served, and every later call reads the sealed `completed` back.
#[tokio::test]
async fn completing_a_snapshot_seals_it_and_acknowledges_the_status_it_sealed() {
    let router = router();

    // Rows differ along how many blocks were written before the seal, including none.
    let cases: &[(&str, &[(i32, u8)])] = &[
        ("no blocks written", &[]),
        ("one block written", &[(0, 21)]),
        ("three blocks written", &[(0, 22), (1, 23), (7, 24)]),
    ];

    for (case, blocks) in cases {
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = member(&started, "SnapshotId");
        assert!(
            !snapshot_id.is_empty(),
            "case {case:?}: StartSnapshot must report a SnapshotId, got {}",
            started.json()
        );
        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &snapshot_id, *block_index, data, BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: PutSnapshotBlock at {block_index} must answer 201, got {} with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        let count = i32::try_from(blocks.len()).expect("a small block count");
        let accepted = complete_snapshot(&router, &snapshot_id, count).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        assert_eq!(
            member(&accepted, "Status"),
            "pending",
            "case {case:?}: the acknowledgement must report the status the seal was applied to, got {}",
            accepted.json()
        );

        // Sealed by that call: "You cannot write new blocks to a snapshot after it has
        // been completed".
        let data = block_bytes(99);
        let refused = put_block(
            &router,
            &snapshot_id,
            1_000,
            data.clone(),
            BLOCK_SIZE,
            &checksum(&data),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a write to a sealed snapshot must be refused, got {} with body {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            refused.error_code()
        );

        // Reading the status back reports the sealed value: the seal was applied by the
        // accepted call, not left outstanding.
        let read_back = complete_snapshot(&router, &snapshot_id, count).await;
        let observed = member(&read_back, "Status");
        assert_eq!(
            observed, "completed",
            "case {case:?}: a later call must read the sealed status back, observed {observed:?}"
        );

        // A completed snapshot still serves everything that was written into it.
        let listed = listed_blocks(&list_blocks(&router, &snapshot_id, "").await);
        let indexes: Vec<i32> = listed.iter().map(|(index, _)| *index).collect();
        let expected_indexes: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            indexes, expected_indexes,
            "case {case:?}: a completed snapshot must still list the blocks written, got {indexes:?}"
        );
    }
}

/// RULE 3: a `blockToken` is judged against the expression the SERVICE enforces —
/// `^[A-Za-z0-9+/=_-]+$`, the URL-safe base64 alphabet — and a value outside it is
/// refused with the framework's constraint report naming the member and THAT expression,
/// carried by this service's OWN modelled `ValidationException`: `Message` plus the
/// `Reason` a client branches on, as every other refusal of this operation is.
/// A value inside it is never a constraint violation: it goes on to be resolved, and
/// fails (or succeeds) on whether it names the block.
#[tokio::test]
async fn block_token_is_judged_against_the_pattern_the_service_enforces() {
    let router = router();
    let snapshot_id = write_snapshot(&router, "block-token", 1, &[(0, 31)]).await;
    let real_token = listed_blocks(&list_blocks(&router, &snapshot_id, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the snapshot lists its block");

    // Rows OUTSIDE the enforced expression, differing along which character or bound
    // puts them there. The requirement each clause must state is the third column.
    let over_long = "A".repeat(257);
    let refused: &[(&str, String, String)] = &[
        (
            "empty",
            String::new(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            "a space",
            "tok en".to_string(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            "a character outside the alphabet",
            "token!".to_string(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            // The modelled `BlockToken` `@length` is 0..256.
            "one character past the modelled maximum length",
            over_long,
            "Member must have length less than or equal to 256".to_string(),
        ),
    ];

    for (case, token, requirement) in refused {
        let reply = get_block(&router, &snapshot_id, 0, token).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token outside the enforced expression must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        // The report travels in the service's OWN modelled shape, so it arrives as
        // `Message` beside the `Reason` a client branches on. The synthetic envelope the
        // boundary would have used carries neither: its only member is a lowercase
        // `message`.
        assert_eq!(
            member(&reply, "Reason"),
            CONSTRAINT_REFUSAL_REASON,
            "case {case:?}: the refusal must carry the ValidationExceptionReason a client branches on, got {}",
            reply.json()
        );
        assert!(
            member(&reply, "message").is_empty(),
            "case {case:?}: the refusal must not travel in the synthetic envelope, got {}",
            reply.json()
        );
        let message = member(&reply, "Message");
        assert!(
            message.starts_with("1 validation error detected: Value '"),
            "case {case:?}: the refusal must be the constraint report, got {message:?}"
        );
        assert!(
            message.contains(&format!("at 'blockToken' failed to satisfy constraint: {requirement}")),
            "case {case:?}: the report must name the member and the requirement {requirement:?}, got {message:?}"
        );
    }

    // The recorded instance, asserted whole: an empty token names the member, echoes the
    // value and quotes the expression the service enforces, inside the modelled error.
    let empty = get_block(&router, &snapshot_id, 0, "").await;
    assert_validation(
        "empty",
        &empty,
        CONSTRAINT_REFUSAL_REASON,
        &format!(
            "1 validation error detected: Value '' at 'blockToken' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"
        ),
    );

    // Rows INSIDE the enforced expression: each gets past validation and is answered by
    // token resolution — the modelled `ValidationException` carrying `Reason` and the
    // service's request-derived template naming the rejected token. `_` and `-` are the
    // characters the model's narrower pattern would have refused.
    let other = write_snapshot(&router, "another snapshot", 1, &[(0, 32)]).await;
    let other_token = listed_blocks(&list_blocks(&router, &other, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the other snapshot lists its block");
    let accepted: Vec<(&str, String, i32)> = vec![
        ("URL-safe base64 alphabet", "AAA_BBB-CCC=".to_string(), 0),
        (
            "standard base64 alphabet",
            "YW5vdGhlci10b2tlbg==".to_string(),
            0,
        ),
        ("bare alphanumerics", "token0".to_string(), 0),
        ("a token issued for another snapshot", other_token, 0),
        ("the issued token at an index with no block", real_token.clone(), 3),
    ];

    for (case, token, block_index) in &accepted {
        let reply = get_block(&router, &snapshot_id, *block_index, token).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token that names no block must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_BLOCK_TOKEN",
            "case {case:?}: a token inside the enforced expression must be answered by token resolution, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("The block token {token} is invalid."),
            "case {case:?}: the refusal must name the rejected token, got {}",
            reply.json()
        );
        assert!(
            member(&reply, "message").is_empty(),
            "case {case:?}: a token inside the enforced expression must not be a constraint violation, got {}",
            reply.json()
        );
    }

    // And the token the service really issued still reads its block.
    let fetched = get_block(&router, &snapshot_id, 0, &real_token).await;
    assert_eq!(
        fetched.status, 200,
        "case \"the issued token\": GetSnapshotBlock must answer 200, got {} with body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
}

/// RULE 4: a checksum algorithm the service does not support is refused through the
/// service's OWN modelled `ValidationException` — HTTP 400 with `Message` naming the
/// rejected algorithm and the supported set, and `Reason` carrying the
/// `ValidationExceptionReason` a client branches on.
#[tokio::test]
async fn an_unsupported_checksum_algorithm_is_refused_with_message_and_reason() {
    let router = router();
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let snapshot_id = member(&started, "SnapshotId");
    let data = block_bytes(41);
    let data_checksum = checksum(&data);

    // Rows differ along which unsupported algorithm was asked for, including one that
    // differs from the supported value only in case.
    let unsupported: &[&str] = &["MD5", "SHA1", "CRC32C", "sha256"];

    for algorithm in unsupported {
        let case = format!("PutSnapshotBlock with {algorithm}");
        let reply = put_block_with_algorithm(
            &router,
            &snapshot_id,
            0,
            data.clone(),
            BLOCK_SIZE,
            &data_checksum,
            algorithm,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unsupported algorithm must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!(
                "Unsupported Checksum Algorithm '{algorithm}', supported algorithms are \
                 {SUPPORTED_ALGORITHM}"
            ),
            "case {case:?}: the refusal must name the rejected algorithm and the supported set, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the matching ValidationExceptionReason, got {}",
            reply.json()
        );

        // The same rule in the other operation that carries the member.
        let case = format!("CompleteSnapshot with {algorithm}");
        let completed = complete_snapshot_with_algorithm(&router, &snapshot_id, 0, algorithm).await;
        assert_eq!(
            completed.status, 400,
            "case {case:?}: an unsupported algorithm must be refused 400, got {} with body {}",
            completed.status,
            String::from_utf8_lossy(&completed.body)
        );
        assert_eq!(
            member(&completed, "Message"),
            format!(
                "Unsupported Checksum Algorithm '{algorithm}', supported algorithms are \
                 {SUPPORTED_ALGORITHM}"
            ),
            "case {case:?}: the refusal must name the rejected algorithm and the supported set, got {}",
            completed.json()
        );
        assert_eq!(
            member(&completed, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the matching ValidationExceptionReason, got {}",
            completed.json()
        );
    }

    // The supported algorithm is still accepted, so the refusal above is about the value
    // and not about the member.
    let written = put_block_with_algorithm(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        &data_checksum,
        SUPPORTED_ALGORITHM,
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"the supported algorithm\": PutSnapshotBlock must answer 201, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );

    // The same rule covers the other header members the service judges: a `Checksum`
    // outside its modelled form and a `Progress` outside its modelled range are refused
    // through the modelled error too, each carrying a Message and a Reason.
    let malformed_checksum = put_block_with_algorithm(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        "not*a*checksum",
        SUPPORTED_ALGORITHM,
    )
    .await;
    assert_eq!(
        malformed_checksum.status, 400,
        "case \"a Checksum outside its modelled form\": expected HTTP 400, got {} with body {}",
        malformed_checksum.status,
        String::from_utf8_lossy(&malformed_checksum.body)
    );
    assert!(
        member(&malformed_checksum, "Message").contains("at 'checksum' failed to satisfy constraint"),
        "case \"a Checksum outside its modelled form\": the refusal must name the member, got {}",
        malformed_checksum.json()
    );
    assert_eq!(
        member(&malformed_checksum, "Reason"),
        "INVALID_PARAMETER_VALUE",
        "case \"a Checksum outside its modelled form\": the refusal must carry a Reason, got {}",
        malformed_checksum.json()
    );

    // `Progress` is modelled `@range` 0..100; one past the maximum is the smallest input
    // outside it.
    let over_progress = put_block_with_progress(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        &data_checksum,
        "101",
    )
    .await;
    assert_eq!(
        over_progress.status, 400,
        "case \"a Progress past the modelled maximum\": expected HTTP 400, got {} with body {}",
        over_progress.status,
        String::from_utf8_lossy(&over_progress.body)
    );
    assert!(
        member(&over_progress, "Message").contains("at 'progress' failed to satisfy constraint"),
        "case \"a Progress past the modelled maximum\": the refusal must name the member, got {}",
        over_progress.json()
    );
    assert_eq!(
        member(&over_progress, "Reason"),
        "INVALID_PARAMETER_VALUE",
        "case \"a Progress past the modelled maximum\": the refusal must carry a Reason, got {}",
        over_progress.json()
    );

    // A `Progress` inside the range is accepted, so the refusal is about the value.
    let in_progress = put_block_with_progress(
        &router,
        &snapshot_id,
        0,
        data,
        BLOCK_SIZE,
        &data_checksum,
        "100",
    )
    .await;
    assert_eq!(
        in_progress.status, 201,
        "case \"a Progress at the modelled maximum\": expected HTTP 201, got {} with body {}",
        in_progress.status,
        String::from_utf8_lossy(&in_progress.body)
    );

    let sealed = complete_snapshot_with_algorithm(&router, &snapshot_id, 1, SUPPORTED_ALGORITHM).await;
    assert_eq!(
        sealed.status, 202,
        "case \"the supported algorithm\": CompleteSnapshot must answer 202, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
}

/// RULE 5: the `ClientToken` conflict carries a FIXED, request-independent message. It
/// is the same sentence whatever the token and whatever changed, and it echoes neither
/// the caller's token nor the operation.
#[tokio::test]
async fn a_client_token_conflict_carries_the_fixed_message() {
    let router = router();
    let expected = "Current request does not match request stored with this token.";

    // Rows differ along the token's text and along WHICH member of the repeat request
    // differs from the stored one.
    let cases: &[(&str, serde_json::Value, serde_json::Value)] = &[
        (
            "a changed volume size",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-volume" }),
            serde_json::json!({ "VolumeSize": 2, "ClientToken": "conflict-volume" }),
        ),
        (
            "a changed description",
            serde_json::json!({
                "VolumeSize": 1,
                "Description": "first",
                "ClientToken": "snap-055f871ca34010e74",
            }),
            serde_json::json!({
                "VolumeSize": 1,
                "Description": "second",
                "ClientToken": "snap-055f871ca34010e74",
            }),
        ),
        (
            "a tag added",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-tags" }),
            serde_json::json!({
                "VolumeSize": 1,
                "Tags": [{ "Key": "k", "Value": "v" }],
                "ClientToken": "conflict-tags",
            }),
        ),
        (
            "encryption added",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-encrypted" }),
            serde_json::json!({ "VolumeSize": 1, "Encrypted": true, "ClientToken": "conflict-encrypted" }),
        ),
    ];

    for (case, first, second) in cases {
        let created = start_snapshot(&router, first).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the first StartSnapshot must answer 201, got {} with body {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let conflict = start_snapshot(&router, second).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: a changed request under the same token must answer 409, got {} with body {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert!(
            conflict.error_code().ends_with("ConflictException"),
            "case {case:?}: expected the ConflictException error code, got {:?}",
            conflict.error_code()
        );
        assert_eq!(
            member(&conflict, "Message"),
            expected,
            "case {case:?}: the conflict message is fixed, got {}",
            conflict.json()
        );
        let token = first
            .get("ClientToken")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            !member(&conflict, "Message").contains(token),
            "case {case:?}: the refusal must not echo the caller's token {token:?}, got {}",
            conflict.json()
        );
    }
}

/// RULE 6: a `VolumeSize` the service will not accept is refused with its request-derived
/// template, which names only the rejected value — no bound, no member prose — and
/// carries the `INVALID_VOLUME_SIZE` reason.
#[tokio::test]
async fn an_out_of_range_volume_size_is_refused_with_the_service_template() {
    let router = router();

    // `StartSnapshotRequest$VolumeSize`: "The maximum size is 65536 GiB (64 TiB)". Rows
    // differ along how far past that bound they are, the first being one past it.
    let refused: &[i64] = &[65_537, 99_999, 1_048_576, i64::from(i32::MAX)];

    for volume_size in refused {
        let case = format!("VolumeSize {volume_size}");
        let reply = start_snapshot(&router, &serde_json::json!({ "VolumeSize": volume_size })).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an out-of-range volume size must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Message"),
            invalid_volume_size_message(*volume_size),
            "case {case:?}: the refusal must name only the rejected value, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_VOLUME_SIZE",
            "case {case:?}: the refusal must carry the INVALID_VOLUME_SIZE reason, got {}",
            reply.json()
        );
    }

    // The published bound itself is accepted, so the refusal is about being past it.
    let accepted = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 65_536 })).await;
    assert_eq!(
        accepted.status, 201,
        "case \"the published maximum\": StartSnapshot must answer 201, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// RULE 7: every operation that resolves a snapshot id words "that snapshot is not
/// stored" the same way — `The snapshot '<id>' does not exist.`, HTTP 404
/// `ResourceNotFoundException` with reason `SNAPSHOT_NOT_FOUND` — and says nothing about
/// a snapshot existing in some other state, which is a different refusal.
#[tokio::test]
async fn every_snapshot_not_found_refusal_uses_one_template() {
    let router = router();
    let data = block_bytes(51);
    let data_checksum = checksum(&data);
    // Inside the enforced `blockToken` expression, so the request reaches the lookup.
    let token = "QUJDREVG";

    for id in UNSTORED_IDS {
        // Every row must be WELL-FORMED, or it would be exercising the malformed-id rule
        // instead; the width is asserted rather than counted by eye.
        let hex = id.strip_prefix("snap-").unwrap_or_default();
        assert!(
            matches!(hex.len(), 8 | 17),
            "case {id:?}: a not-found row needs 8 or 17 hex digits, got {}",
            hex.len()
        );

        // Rows: every operation that resolves a snapshot id, over every unstored id.
        let replies: Vec<(String, Reply)> = vec![
            (
                format!("ListSnapshotBlocks on {id}"),
                list_blocks(&router, id, "").await,
            ),
            (
                format!("GetSnapshotBlock on {id}"),
                get_block(&router, id, 0, token).await,
            ),
            (
                format!("PutSnapshotBlock on {id}"),
                put_block(&router, id, 0, data.clone(), BLOCK_SIZE, &data_checksum).await,
            ),
            (
                format!("CompleteSnapshot on {id}"),
                complete_snapshot(&router, id, 0).await,
            ),
            (
                format!("StartSnapshot with ParentSnapshotId {id}"),
                start_snapshot(
                    &router,
                    &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": id }),
                )
                .await,
            ),
        ];

        for (case, reply) in replies {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an unstored snapshot must be refused 404, got {} with body {}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_code().ends_with("ResourceNotFoundException"),
                "case {case:?}: expected the ResourceNotFoundException error code, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                member(&reply, "Message"),
                format!("The snapshot '{id}' does not exist."),
                "case {case:?}: every not-found refusal shares one template, got {}",
                reply.json()
            );
            assert_eq!(
                member(&reply, "Reason"),
                "SNAPSHOT_NOT_FOUND",
                "case {case:?}: the refusal must carry the SNAPSHOT_NOT_FOUND reason, got {}",
                reply.json()
            );
        }
    }

    // A snapshot that IS stored but is still being written is a different condition, and
    // must not borrow the not-found template: it is refused as a validation failure
    // naming the state, not as an absent resource.
    let pending = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let pending_id = member(&pending, "SnapshotId");
    let child = start_snapshot(
        &router,
        &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": pending_id }),
    )
    .await;
    assert_eq!(
        child.status, 400,
        "case \"a parent that is still pending\": expected HTTP 400, got {} with body {}",
        child.status,
        String::from_utf8_lossy(&child.body)
    );
    assert!(
        child.error_code().ends_with("ValidationException"),
        "case \"a parent that is still pending\": expected the ValidationException error code, got {:?}",
        child.error_code()
    );
    assert!(
        !member(&child, "Message").contains("does not exist"),
        "case \"a parent that is still pending\": the refusal must not claim the snapshot is absent, got {}",
        child.json()
    );
}

/// RULE 8 (the other half of rule 7): a value that is not even SHAPED like a snapshot id
/// is a malformed parameter, not a snapshot the service does not hold. It is refused
/// HTTP 400 `ValidationException` with the service's request-derived template naming the
/// rejected value and the parameter it arrived in — `Value (<v>) for parameter <p> is
/// invalid. Expected: 'snap-...'.` — and the `INVALID_SNAPSHOT_ID` reason.
///
/// The widths a snapshot id has are 8 and 17 hex digits
/// (`crate::state::SNAPSHOT_ID_HEX_WIDTHS`); the modelled `SnapshotId` pattern
/// `^snap-[0-9a-f]+$` admits any of them, so every row below is a value the boundary
/// lets through.
#[tokio::test]
async fn a_snapshot_id_of_the_wrong_shape_is_a_malformed_parameter() {
    let router = router();
    let data = block_bytes(61);
    let data_checksum = checksum(&data);
    let token = "QUJDREVG";

    // Rows differ along how far the hex width is from the two the service accepts —
    // built from a repeat count rather than written out, so no row can be miscounted —
    // plus an all-zero id of each accepted width, whose suffix is not a resource number.
    // Every row satisfies the MODELLED pattern `^snap-[0-9a-f]+$`, so every row reaches
    // the handler: a value outside that pattern is the boundary's to refuse, asserted
    // separately below.
    let widths: &[usize] = &[1, 7, 9, 16, 18, 32];
    let mut malformed: Vec<String> = widths
        .iter()
        .map(|width| format!("snap-{}", "a".repeat(*width)))
        .collect();
    for width in SNAPSHOT_ID_HEX_WIDTHS {
        malformed.push(format!("snap-{}", "0".repeat(width)));
    }

    for id in &malformed {
        let replies: Vec<(String, Reply, &str)> = vec![
            (
                format!("ListSnapshotBlocks on {id}"),
                list_blocks(&router, id, "").await,
                "snapshotId",
            ),
            (
                format!("GetSnapshotBlock on {id}"),
                get_block(&router, id, 0, token).await,
                "snapshotId",
            ),
            (
                format!("PutSnapshotBlock on {id}"),
                put_block(&router, id, 0, data.clone(), BLOCK_SIZE, &data_checksum).await,
                "snapshotId",
            ),
            (
                format!("CompleteSnapshot on {id}"),
                complete_snapshot(&router, id, 0).await,
                "snapshotId",
            ),
            (
                format!("StartSnapshot with ParentSnapshotId {id}"),
                start_snapshot(
                    &router,
                    &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": id }),
                )
                .await,
                "parentSnapshotId",
            ),
        ];

        for (case, reply, parameter) in replies {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a malformed snapshot id must be refused 400, got {} with body {}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_code().ends_with("ValidationException"),
                "case {case:?}: expected the ValidationException error code, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                member(&reply, "Message"),
                format!("Value ({id}) for parameter {parameter} is invalid. Expected: 'snap-...'."),
                "case {case:?}: the refusal must name the value and the parameter, got {}",
                reply.json()
            );
            assert_eq!(
                member(&reply, "Reason"),
                "INVALID_SNAPSHOT_ID",
                "case {case:?}: the refusal must carry the INVALID_SNAPSHOT_ID reason, got {}",
                reply.json()
            );
        }
    }

    // And a well-formed id of each accepted width is NOT refused for its shape: it is
    // answered as a snapshot the service does not hold.
    for width in [8_usize, 17] {
        let id = format!("snap-{}", "a".repeat(width));
        let reply = list_blocks(&router, &id, "").await;
        assert_eq!(
            reply.status, 404,
            "case \"a well-formed {width}-digit id\": expected HTTP 404, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("The snapshot '{id}' does not exist."),
            "case \"a well-formed {width}-digit id\": expected the not-found template, got {}",
            reply.json()
        );
    }

    // The other half of the split: a value outside the MODELLED `SnapshotId` pattern is
    // refused by the boundary before any handler runs, with the framework's constraint
    // report — the same layer, and the same wording, that judges every other member the
    // model constrains.
    for outside in ["snap-", "not-an-id", "SNAP-DEADBEEF"] {
        let reply = list_blocks(&router, outside, "").await;
        assert_eq!(
            reply.status, 400,
            "case {outside:?}: a value outside the modelled pattern must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            member(&reply, "message").starts_with("1 validation error detected: Value '"),
            "case {outside:?}: expected the boundary's constraint report, got {}",
            reply.json()
        );
    }
}

/// The modelled form of `com.amazonaws.ebs#Checksum`, which `CompleteSnapshot` and
/// `PutSnapshotBlock` both bind to `x-amz-Checksum`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#Checksum")`: `@length` 0..64 and
/// this `@pattern`).
pub(crate) const CHECKSUM_PATTERN: &str = "^[A-Za-z0-9+/=]+$";
const CHECKSUM_MAX_LENGTH: usize = 64;

/// The modelled form of `com.amazonaws.ebs#ChecksumAggregationMethod`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ChecksumAggregationMethod")`:
/// `@length` 0..32, this `@pattern`, and the one value the enum declares).
const AGGREGATION_METHOD_PATTERN: &str = "^[A-Za-z0-9]+$";
const AGGREGATION_METHODS: &[&str] = &["LINEAR"];

/// The modelled minimum of `com.amazonaws.ebs#ChangedBlocksCount`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ChangedBlocksCount")`: `@range`
/// min 0, with no maximum).
pub(crate) const CHANGED_BLOCKS_COUNT_MIN: i64 = 0;

/// One clause of a constraint report: the value echoed, the member as the report names it
/// (its Smithy name lower-camelled) and the requirement the value failed.
pub(crate) fn clause(value: &str, member_path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The envelope the constraint report travels in, which is the one the request boundary
/// renders for the members it judges itself: one clause per rejected value, in the order
/// the input shape declares its members.
pub(crate) fn constraint_report(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// The requirement a value outside a member's `@pattern` failed.
pub(crate) fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The requirement a value longer than a member's `@length` maximum failed.
pub(crate) fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a value outside a member's enum value set failed.
fn enum_requirement(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The requirement a number below a member's `@range` minimum failed.
pub(crate) fn min_value_requirement(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement a number above a member's `@range` maximum failed.
fn max_value_requirement(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The ceiling a constraint report puts on the rejected value it echoes: a caller controls
/// that value's length, so a long one arrives as a 256-character prefix followed by its
/// byte count. Asserted here because a row below rejects a value past the ceiling.
pub(crate) const MAX_ECHOED_VALUE: usize = 256;

/// The rejected value as a clause echoes it.
pub(crate) fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    format!("{}... [{} bytes]", &value[..MAX_ECHOED_VALUE], value.len())
}

/// `CompleteSnapshot` with the header members this operation judges itself set to
/// whatever the row carries; an optional header the row leaves `None` is not sent at all.
async fn complete_snapshot_with_headers(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    changed_blocks_count: i32,
    checksum_value: Option<&str>,
    aggregation_method: Option<&str>,
) -> Reply {
    let mut headers = vec![(
        "x-amz-ChangedBlocksCount",
        changed_blocks_count.to_string(),
    )];
    if let Some(value) = checksum_value {
        headers.push(("x-amz-Checksum", value.to_string()));
    }
    if let Some(value) = aggregation_method {
        headers.push(("x-amz-Checksum-Aggregation-Method", value.to_string()));
    }
    send(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &headers,
        Vec::new(),
    )
    .await
}

/// One row of the judged-header table: what the request carries beside its snapshot id,
/// and the clauses the refusal must report.
struct JudgedHeaders {
    case: String,
    changed_blocks_count: i32,
    checksum: Option<String>,
    aggregation_method: Option<String>,
    clauses: Vec<String>,
}

/// RULE 8: a `CompleteSnapshot` header member outside its MODELLED form is refused with
/// this service's own modelled `ValidationException` — HTTP 400, the `ValidationException`
/// error code, `Reason` `INVALID_PARAMETER_VALUE` and the constraint report as the
/// `Message` — exactly as the identical members on `PutSnapshotBlock` are (asserted
/// above). A client branches on `Reason`, so a refusal that carries none tells it nothing
/// about whether to fix its request or retry it.
///
/// Rows differ along what the rule generalises over: which member carries the bad value,
/// which modelled constraint it breaks (a pattern, a length maximum, an enum value set, a
/// range minimum), how far outside it is, and how many members are wrong at once.
#[tokio::test]
async fn a_judged_complete_snapshot_header_is_refused_through_the_modelled_error() {
    let router = router();
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    assert_eq!(
        started.status, 201,
        "the fixture snapshot must start, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = member(&started, "SnapshotId");

    // Inside the modelled form — base64 of a SHA256 digest, which is what an aggregate
    // checksum is — and the aggregate this snapshot's own content produces: no block is
    // written into it, so the LINEAR aggregation hashes the empty concatenation of
    // per-block checksums (`aggregate_checksum_of_a_completion.rs` owns that rule). The
    // control at the end is therefore refused by nothing, by its form or by its value.
    let aggregate = linear_aggregate(&[]);
    let over_long = "A".repeat(CHECKSUM_MAX_LENGTH + 1);
    let malformed_checksum = "not*a*checksum";
    let outside_the_set = "NOTLINEAR";
    let malformed_method = "NOT_LINEAR";

    let rows: Vec<JudgedHeaders> = vec![
        JudgedHeaders {
            case: "a Checksum outside the modelled pattern".to_string(),
            changed_blocks_count: 0,
            checksum: Some(malformed_checksum.to_string()),
            aggregation_method: None,
            clauses: vec![clause(
                malformed_checksum,
                "checksum",
                &pattern_requirement(CHECKSUM_PATTERN),
            )],
        },
        JudgedHeaders {
            case: "an empty Checksum, which the modelled pattern admits no value of"
                .to_string(),
            changed_blocks_count: 0,
            checksum: Some(String::new()),
            aggregation_method: None,
            clauses: vec![clause(
                "",
                "checksum",
                &pattern_requirement(CHECKSUM_PATTERN),
            )],
        },
        JudgedHeaders {
            case: "a Checksum one character past the modelled maximum length".to_string(),
            changed_blocks_count: 0,
            checksum: Some(over_long.clone()),
            aggregation_method: None,
            clauses: vec![clause(
                &over_long,
                "checksum",
                &max_length_requirement(CHECKSUM_MAX_LENGTH),
            )],
        },
        JudgedHeaders {
            case: "a ChecksumAggregationMethod outside the set its enum declares"
                .to_string(),
            changed_blocks_count: 0,
            checksum: None,
            aggregation_method: Some(outside_the_set.to_string()),
            clauses: vec![clause(
                outside_the_set,
                "checksumAggregationMethod",
                &enum_requirement(AGGREGATION_METHODS),
            )],
        },
        JudgedHeaders {
            case: "a ChecksumAggregationMethod outside the modelled pattern AND the set"
                .to_string(),
            changed_blocks_count: 0,
            checksum: None,
            aggregation_method: Some(malformed_method.to_string()),
            clauses: vec![
                clause(
                    malformed_method,
                    "checksumAggregationMethod",
                    &pattern_requirement(AGGREGATION_METHOD_PATTERN),
                ),
                clause(
                    malformed_method,
                    "checksumAggregationMethod",
                    &enum_requirement(AGGREGATION_METHODS),
                ),
            ],
        },
        JudgedHeaders {
            case: "a ChangedBlocksCount one below the modelled minimum".to_string(),
            changed_blocks_count: -1,
            checksum: None,
            aggregation_method: None,
            clauses: vec![clause(
                "-1",
                "changedBlocksCount",
                &min_value_requirement(CHANGED_BLOCKS_COUNT_MIN),
            )],
        },
        JudgedHeaders {
            case: "a ChangedBlocksCount far below the modelled minimum".to_string(),
            changed_blocks_count: -524_288,
            checksum: None,
            aggregation_method: None,
            clauses: vec![clause(
                "-524288",
                "changedBlocksCount",
                &min_value_requirement(CHANGED_BLOCKS_COUNT_MIN),
            )],
        },
        JudgedHeaders {
            case: "every judged member outside its modelled form at once".to_string(),
            changed_blocks_count: -1,
            checksum: Some(malformed_checksum.to_string()),
            aggregation_method: Some(outside_the_set.to_string()),
            clauses: vec![
                clause(
                    "-1",
                    "changedBlocksCount",
                    &min_value_requirement(CHANGED_BLOCKS_COUNT_MIN),
                ),
                clause(
                    malformed_checksum,
                    "checksum",
                    &pattern_requirement(CHECKSUM_PATTERN),
                ),
                clause(
                    outside_the_set,
                    "checksumAggregationMethod",
                    &enum_requirement(AGGREGATION_METHODS),
                ),
            ],
        },
        JudgedHeaders {
            case: "a well-formed aggregate Checksum beside a method outside the set"
                .to_string(),
            changed_blocks_count: 0,
            checksum: Some(aggregate.clone()),
            aggregation_method: Some(outside_the_set.to_string()),
            clauses: vec![clause(
                outside_the_set,
                "checksumAggregationMethod",
                &enum_requirement(AGGREGATION_METHODS),
            )],
        },
    ];

    for row in &rows {
        let reply = complete_snapshot_with_headers(
            &router,
            &snapshot_id,
            row.changed_blocks_count,
            row.checksum.as_deref(),
            row.aggregation_method.as_deref(),
        )
        .await;
        assert_validation(
            &row.case,
            &reply,
            "INVALID_PARAMETER_VALUE",
            &constraint_report(&row.clauses),
        );
    }

    // The control that makes every row above mean something: the SAME request with each
    // judged member inside its modelled form is served — and it reports `pending`, the
    // status the snapshot was in when it arrived, so not one of the refusals above sealed
    // it on the way out.
    let accepted =
        complete_snapshot_with_headers(&router, &snapshot_id, 0, Some(&aggregate), Some("LINEAR"))
            .await;
    assert_eq!(
        accepted.status, 202,
        "case \"every judged member inside its modelled form\": CompleteSnapshot must answer \
         202, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    assert_eq!(
        member(&accepted, "Status"),
        "pending",
        "case \"every judged member inside its modelled form\": the 202 must report the status \
         the snapshot was in when the completion was accepted, got {}",
        accepted.json()
    );

    // And the seal that completion applied is what every later call reads back, so the
    // accepted request really did the work the refused ones did not.
    let repeated = complete_snapshot(&router, &snapshot_id, 0).await;
    assert_eq!(
        repeated.status, 202,
        "case \"the snapshot read back after it was sealed\": expected 202, got {} with body {}",
        repeated.status,
        String::from_utf8_lossy(&repeated.body)
    );
    assert_eq!(
        member(&repeated, "Status"),
        "completed",
        "case \"the snapshot read back after it was sealed\": the stored snapshot must read \
         back completed, got {}",
        repeated.json()
    );
}

/// The modelled `@range` of `com.amazonaws.ebs#PutSnapshotBlockRequest$Progress`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#PutSnapshotBlockRequest")`
/// reports min 0, max 100).
const PROGRESS_RANGE: (i64, i64) = (0, 100);

/// The modelled `@length` maximum of `com.amazonaws.ebs#BlockToken`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#BlockToken")`: `@length` 0..256),
/// which the operation enforces alongside the expression it reports.
const BLOCK_TOKEN_MAX_LENGTH: usize = 256;

/// Which input a row of the RULE 9 table puts outside its form, and the value it carries.
///
/// Every variant names a member its operation takes OFF the request boundary and judges in
/// its own handler: `GetSnapshotBlock`'s `BlockToken`, `PutSnapshotBlock`'s `Checksum`,
/// `Progress` and `ChecksumAlgorithm`, and `CompleteSnapshot`'s `Checksum`,
/// `ChecksumAggregationMethod` and `ChangedBlocksCount` — the whole set this service's
/// handlers judge for themselves.
enum JudgedInput {
    /// `GetSnapshotBlock`'s `blockToken` query member.
    BlockToken(String),
    /// `PutSnapshotBlock`'s `x-amz-Checksum` header.
    PutChecksum(String),
    /// `PutSnapshotBlock`'s `x-amz-Progress` header.
    PutProgress(String),
    /// `PutSnapshotBlock`'s `x-amz-Checksum-Algorithm` header.
    PutChecksumAlgorithm(String),
    /// `CompleteSnapshot`'s `x-amz-Checksum` header.
    CompleteChecksum(String),
    /// `CompleteSnapshot`'s `x-amz-Checksum-Aggregation-Method` header.
    CompleteAggregationMethod(String),
    /// `CompleteSnapshot`'s `x-amz-ChangedBlocksCount` header.
    CompleteChangedBlocksCount(i32),
}

/// Send the request a row describes: the read rows against `readable` (a sealed snapshot
/// whose block is there to be read) and the write and completion rows against `pending`
/// (a snapshot still open), so no row can be answered by the snapshot's state instead of
/// by the member it carries.
async fn send_judged(
    router: &MultiServiceRouter,
    readable: &str,
    pending: &str,
    input: &JudgedInput,
) -> Reply {
    let data = block_bytes(13);
    let data_checksum = checksum(&data);
    match input {
        JudgedInput::BlockToken(token) => get_block(router, readable, 0, token).await,
        JudgedInput::PutChecksum(value) => {
            put_block(router, pending, 0, data, BLOCK_SIZE, value).await
        }
        JudgedInput::PutProgress(value) => {
            put_block_with_progress(router, pending, 0, data, BLOCK_SIZE, &data_checksum, value)
                .await
        }
        JudgedInput::PutChecksumAlgorithm(value) => {
            put_block_with_algorithm(router, pending, 0, data, BLOCK_SIZE, &data_checksum, value)
                .await
        }
        JudgedInput::CompleteChecksum(value) => {
            complete_snapshot_with_headers(router, pending, 0, Some(value), None).await
        }
        JudgedInput::CompleteAggregationMethod(value) => {
            complete_snapshot_with_headers(router, pending, 0, None, Some(value)).await
        }
        JudgedInput::CompleteChangedBlocksCount(count) => {
            complete_snapshot_with_headers(router, pending, *count, None, None).await
        }
    }
}

/// RULE 9: a value this service judges in a HANDLER — every member an operation takes off
/// the request boundary and checks itself — is refused through the service's OWN modelled
/// `ValidationException`, carrying the `Message` and the `Reason` a client reads, and never
/// through the framework's synthetic envelope, whose only member is a lowercase `message`.
/// One class, one envelope: a client that branches on `ValidationExceptionReason` gets an
/// answer for every member, not for all but one of them.
///
/// Rows differ along what the rule generalises over: which operation owns the member, where
/// the member travels (a query, a header), which constraint the value breaks (a pattern, a
/// length maximum, a range bound, an enum set, a value the service does not support) and
/// how far outside it is. The controls at the end send each operation the same request with
/// every judged member inside its form, so each refusal is about the value and not about
/// the member or the snapshot.
#[tokio::test]
async fn every_handler_judged_member_is_refused_through_the_modelled_error() {
    let router = router();
    let readable = write_snapshot(&router, "a handler-judged member", 1, &[(0, 5)]).await;
    let real_token = listed_blocks(&list_blocks(&router, &readable, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the sealed snapshot lists its block");
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    assert_eq!(
        started.status, 201,
        "the open fixture must start, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let pending = member(&started, "SnapshotId");

    let over_long_token = "A".repeat(BLOCK_TOKEN_MAX_LENGTH + 1);
    let over_long_checksum = "A".repeat(CHECKSUM_MAX_LENGTH + 1);
    let malformed_checksum = "not*a*checksum";
    let unsupported_algorithm = "MD5";
    let outside_the_set = "NOTLINEAR";

    let rows: Vec<(&str, JudgedInput, String)> = vec![
        (
            "GetSnapshotBlock with an empty blockToken",
            JudgedInput::BlockToken(String::new()),
            constraint_report(&[clause(
                "",
                "blockToken",
                &pattern_requirement(BLOCK_TOKEN_PATTERN),
            )]),
        ),
        (
            "GetSnapshotBlock with a blockToken outside the enforced alphabet",
            JudgedInput::BlockToken("token!".to_string()),
            constraint_report(&[clause(
                "token!",
                "blockToken",
                &pattern_requirement(BLOCK_TOKEN_PATTERN),
            )]),
        ),
        (
            "GetSnapshotBlock with a blockToken one character past the modelled maximum",
            JudgedInput::BlockToken(over_long_token.clone()),
            constraint_report(&[clause(
                &echoed(&over_long_token),
                "blockToken",
                &max_length_requirement(BLOCK_TOKEN_MAX_LENGTH),
            )]),
        ),
        (
            "PutSnapshotBlock with a Checksum outside the modelled pattern",
            JudgedInput::PutChecksum(malformed_checksum.to_string()),
            constraint_report(&[clause(
                malformed_checksum,
                "checksum",
                &pattern_requirement(CHECKSUM_PATTERN),
            )]),
        ),
        (
            "PutSnapshotBlock with a Checksum one character past the modelled maximum",
            JudgedInput::PutChecksum(over_long_checksum.clone()),
            constraint_report(&[clause(
                &over_long_checksum,
                "checksum",
                &max_length_requirement(CHECKSUM_MAX_LENGTH),
            )]),
        ),
        (
            "PutSnapshotBlock with a Progress one past the modelled maximum",
            JudgedInput::PutProgress("101".to_string()),
            constraint_report(&[clause(
                "101",
                "progress",
                &max_value_requirement(PROGRESS_RANGE.1),
            )]),
        ),
        (
            "PutSnapshotBlock with a Progress one below the modelled minimum",
            JudgedInput::PutProgress("-1".to_string()),
            constraint_report(&[clause(
                "-1",
                "progress",
                &min_value_requirement(PROGRESS_RANGE.0),
            )]),
        ),
        (
            "PutSnapshotBlock with a ChecksumAlgorithm the service does not support",
            JudgedInput::PutChecksumAlgorithm(unsupported_algorithm.to_string()),
            format!(
                "Unsupported Checksum Algorithm '{unsupported_algorithm}', supported algorithms \
                 are {SUPPORTED_ALGORITHM}"
            ),
        ),
        (
            "CompleteSnapshot with a Checksum outside the modelled pattern",
            JudgedInput::CompleteChecksum(malformed_checksum.to_string()),
            constraint_report(&[clause(
                malformed_checksum,
                "checksum",
                &pattern_requirement(CHECKSUM_PATTERN),
            )]),
        ),
        (
            "CompleteSnapshot with a ChecksumAggregationMethod outside the set its enum declares",
            JudgedInput::CompleteAggregationMethod(outside_the_set.to_string()),
            constraint_report(&[clause(
                outside_the_set,
                "checksumAggregationMethod",
                &enum_requirement(AGGREGATION_METHODS),
            )]),
        ),
        (
            "CompleteSnapshot with a ChangedBlocksCount one below the modelled minimum",
            JudgedInput::CompleteChangedBlocksCount(-1),
            constraint_report(&[clause(
                "-1",
                "changedBlocksCount",
                &min_value_requirement(CHANGED_BLOCKS_COUNT_MIN),
            )]),
        ),
    ];

    for (case, input, message) in &rows {
        let reply = send_judged(&router, &readable, &pending, input).await;
        assert_validation(case, &reply, CONSTRAINT_REFUSAL_REASON, message);
        assert!(
            member(&reply, "message").is_empty(),
            "case {case:?}: the refusal must be the service's modelled shape, not the synthetic \
             envelope whose only member is a lowercase message, got {}",
            reply.json()
        );
    }

    // The controls. The read the refused tokens were aimed at is served, so a token
    // INSIDE the enforced form is no constraint violation.
    let fetched = get_block(&router, &readable, 0, &real_token).await;
    assert_eq!(
        fetched.status, 200,
        "case \"the token the service issued\": GetSnapshotBlock must answer 200, got {} with \
         body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    // The write, with every judged member inside its form.
    let data = block_bytes(13);
    let data_checksum = checksum(&data);
    let written = put_block_with_progress(
        &router,
        &pending,
        0,
        data,
        BLOCK_SIZE,
        &data_checksum,
        "100",
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"every judged member of the write inside its form\": PutSnapshotBlock must answer \
         201, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
    // And the completion, which is also what proves the refused completions above sealed
    // nothing: the snapshot was still open to take that write and is still open now.
    // The aggregate this snapshot's content produces: one block was written into it, so the
    // LINEAR aggregation hashes that block's own per-block checksum.
    let aggregate = linear_aggregate(std::slice::from_ref(&data_checksum));
    let completed =
        complete_snapshot_with_headers(&router, &pending, 1, Some(&aggregate), Some("LINEAR"))
            .await;
    assert_eq!(
        completed.status, 202,
        "case \"every judged member of the completion inside its form\": CompleteSnapshot must \
         answer 202, got {} with body {}",
        completed.status,
        String::from_utf8_lossy(&completed.body)
    );
    assert_eq!(
        member(&completed, "Status"),
        "pending",
        "case \"every judged member of the completion inside its form\": the 202 must report the \
         status the snapshot was in when the completion arrived, got {}",
        completed.json()
    );
}
