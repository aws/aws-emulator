// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EBS direct-APIs snapshot write-then-read loop.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production
//! router uses, so the assertions are about the wire behaviour: status codes, headers,
//! payload bytes and JSON bodies. Nothing reaches into the state struct directly —
//! a write is only ever proven by reading it back through another operation.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! 1. A block read back is byte-identical to the block written, with `DataLength` and
//!    `Checksum` derived from the persisted bytes.
//! 2. `ListSnapshotBlocks` enumerates exactly the persisted block indexes, ascending,
//!    honouring `StartingBlockIndex`, `MaxResults` and `NextToken`.
//! 3. `StartSnapshot` mints a fresh, modelled-pattern id per call and reports the
//!    snapshot it stored.
//! 4. An operation naming a resource that is not stored fails with the modelled error
//!    and status for that condition.
//! 5. A repeated `ClientToken` replays the first answer; a changed request under the
//!    same token conflicts.
//!
//! The harness below (`router`, `Reply`, the per-operation request helpers) is
//! `pub(crate)` so the sibling suites in `tests/all/` drive the same wire path instead of
//! growing a second copy of it.

use std::sync::Arc;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ebs_ebs::EmulatedService;

/// The only block size the EBS direct APIs support: `PutSnapshotBlockRequest$DataLength`
/// documents "Currently, the only supported size is `524288` bytes. Valid values:
/// `524288`".
pub(crate) const BLOCK_SIZE: i32 = 524_288;

/// A SigV4 `Authorization` header whose credential scope names the `ebs` signing name,
/// which is how the router picks the service for a request. No authenticator is
/// registered in these tests, so the signature itself is never verified.
const AUTHORIZATION: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEBSTEST/20260101/us-east-1/ebs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A router carrying just this service over a fresh shared engine, WITH that engine.
///
/// For a suite whose rule is about a change to this service's state that another service's
/// operation makes — the EBS direct APIs model no delete of a snapshot, so the removal a
/// `DeleteSnapshot` performs is reached through the store rather than over this wire.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A router carrying just this service over a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// One response, reduced to what these tests assert on.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl Reply {
    /// The response body parsed as JSON, or `Null` when it is not JSON.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, falling back
    /// to the `__type` discriminator in the body.
    pub(crate) fn error_code(&self) -> String {
        self.headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    /// A response header as a string, or the empty string when absent.
    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> Reply {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "ebs.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = builder.body(Bytes::from(body)).expect("a valid request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: body.to_vec(),
    }
}

/// Percent-encode a value for use inside a query string.
///
/// The modelled `BlockToken` and `PageToken` pattern `^[A-Za-z0-9+/=]+$` admits `+`, `/`
/// and `=`, which a query reserves (`+` means a space), so a value a test invents has to
/// be escaped exactly as an SDK escapes it. A token this service MINTS needs no escaping
/// at all — that is asserted in `minted_query_tokens.rs`.
///
/// Written out rather than pulled from a crate deliberately: this is ten lines of
/// test-only escaping, and adding a `[dev-dependencies]` entry for it is the only thing
/// that would make this change touch `Cargo.lock` — a file every other branch in flight
/// also touches, which the serialized squash merge cannot reconcile.
fn query_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Base64 SHA256, the form `Checksum` carries on the wire. Computed here independently
/// of the service so the service's value is compared against a second implementation.
pub(crate) fn checksum(data: &[u8]) -> String {
    aws_smithy_types::base64::encode(Sha256::digest(data))
}

/// `BLOCK_SIZE` bytes that differ for every `seed`, so a block read back from the wrong
/// index or the wrong snapshot cannot pass as the right one.
pub(crate) fn block_bytes(seed: u8) -> Vec<u8> {
    let length = usize::try_from(BLOCK_SIZE).expect("the block size fits a usize");
    (0..length)
        .map(|offset| {
            let mixed = (offset as u64)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(u64::from(seed).wrapping_mul(1_099_511_628_211));
            u8::try_from(mixed % 251).expect("a value below 251 fits a u8")
        })
        .collect()
}

pub(crate) async fn start_snapshot(
    router: &MultiServiceRouter,
    body: &serde_json::Value,
) -> Reply {
    send(
        router,
        "POST",
        "/snapshots",
        &[("content-type", "application/json".to_string())],
        serde_json::to_vec(body).expect("a serializable body"),
    )
    .await
}

pub(crate) async fn put_block(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    block_index: i32,
    data: Vec<u8>,
    data_length: i32,
    checksum_value: &str,
) -> Reply {
    put_block_with_algorithm(
        router,
        snapshot_id,
        block_index,
        data,
        data_length,
        checksum_value,
        "SHA256",
    )
    .await
}

/// `PutSnapshotBlock` with the `x-amz-Checksum-Algorithm` header set to `algorithm`,
/// whatever it is: the member the service judges itself rather than at the boundary.
pub(crate) async fn put_block_with_algorithm(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    block_index: i32,
    data: Vec<u8>,
    data_length: i32,
    checksum_value: &str,
    algorithm: &str,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/snapshots/{snapshot_id}/blocks/{block_index}"),
        &[
            ("content-type", "application/octet-stream".to_string()),
            ("x-amz-Data-Length", data_length.to_string()),
            ("x-amz-Checksum", checksum_value.to_string()),
            ("x-amz-Checksum-Algorithm", algorithm.to_string()),
        ],
        data,
    )
    .await
}

/// `PutSnapshotBlock` with the optional `x-amz-Progress` header set to `progress`,
/// whatever it is: another member the service judges itself.
pub(crate) async fn put_block_with_progress(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    block_index: i32,
    data: Vec<u8>,
    data_length: i32,
    checksum_value: &str,
    progress: &str,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/snapshots/{snapshot_id}/blocks/{block_index}"),
        &[
            ("content-type", "application/octet-stream".to_string()),
            ("x-amz-Data-Length", data_length.to_string()),
            ("x-amz-Checksum", checksum_value.to_string()),
            ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
            ("x-amz-Progress", progress.to_string()),
        ],
        data,
    )
    .await
}

pub(crate) async fn complete_snapshot(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    changed_blocks_count: i32,
) -> Reply {
    send(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &[(
            "x-amz-ChangedBlocksCount",
            changed_blocks_count.to_string(),
        )],
        Vec::new(),
    )
    .await
}

/// `CompleteSnapshot` with the optional `x-amz-Checksum-Algorithm` header set to
/// `algorithm`, whatever it is.
pub(crate) async fn complete_snapshot_with_algorithm(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    changed_blocks_count: i32,
    algorithm: &str,
) -> Reply {
    send(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &[
            (
                "x-amz-ChangedBlocksCount",
                changed_blocks_count.to_string(),
            ),
            ("x-amz-Checksum-Algorithm", algorithm.to_string()),
        ],
        Vec::new(),
    )
    .await
}

pub(crate) async fn list_blocks(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    query: &str,
) -> Reply {
    let uri = if query.is_empty() {
        format!("/snapshots/{snapshot_id}/blocks")
    } else {
        format!("/snapshots/{snapshot_id}/blocks?{query}")
    };
    send(router, "GET", &uri, &[], Vec::new()).await
}

pub(crate) async fn get_block(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    block_index: i32,
    block_token: &str,
) -> Reply {
    send(
        router,
        "GET",
        &format!(
            "/snapshots/{snapshot_id}/blocks/{block_index}?blockToken={}",
            query_escape(block_token)
        ),
        &[],
        Vec::new(),
    )
    .await
}

/// A string member of a response body (`Status`, `SnapshotId`, `Message`, `Reason`), or
/// the empty string when the body does not carry it.
pub(crate) fn member(reply: &Reply, name: &str) -> String {
    reply
        .json()
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(BlockIndex, BlockToken)` pairs a listing reported, in the order it reported
/// them.
pub(crate) fn listed_blocks(reply: &Reply) -> Vec<(i32, String)> {
    reply
        .json()
        .get("Blocks")
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .map(|block| {
                    let index = block
                        .get("BlockIndex")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(-1);
                    let token = block
                        .get("BlockToken")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    (i32::try_from(index).unwrap_or(-1), token)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A snapshot id must match the modelled `SnapshotId` pattern `^snap-[0-9a-f]+$`.
fn is_modelled_snapshot_id(value: &str) -> bool {
    match value.strip_prefix("snap-") {
        Some(suffix) => {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        None => false,
    }
}

/// A block token must match the modelled `BlockToken` pattern `^[A-Za-z0-9+/=]+$`.
fn is_modelled_block_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// Start a snapshot, write `block_seeds` at the given indexes, seal it, and return its
/// id. Each row of the write-then-read table goes through this.
pub(crate) async fn write_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    volume_size: i64,
    blocks: &[(i32, u8)],
) -> String {
    let started = start_snapshot(
        router,
        &serde_json::json!({ "VolumeSize": volume_size }),
    )
    .await;
    assert_eq!(
        started.status, 201,
        "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = started
        .json()
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_modelled_snapshot_id(&snapshot_id),
        "case {case:?}: StartSnapshot must mint an id matching ^snap-[0-9a-f]+$, got {snapshot_id:?}"
    );

    for (block_index, seed) in blocks {
        let data = block_bytes(*seed);
        let expected = checksum(&data);
        let reply = put_block(
            router,
            &snapshot_id,
            *block_index,
            data,
            BLOCK_SIZE,
            &expected,
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: PutSnapshotBlock at index {block_index} must answer 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.header("x-amz-Checksum"),
            expected,
            "case {case:?}: PutSnapshotBlock at index {block_index} must report the checksum of the stored bytes, got {:?}",
            reply.header("x-amz-Checksum")
        );
        assert_eq!(
            reply.header("x-amz-Checksum-Algorithm"),
            "SHA256",
            "case {case:?}: PutSnapshotBlock at index {block_index} must report SHA256, got {:?}",
            reply.header("x-amz-Checksum-Algorithm")
        );
    }

    let completed = complete_snapshot(
        router,
        &snapshot_id,
        i32::try_from(blocks.len()).expect("a small block count"),
    )
    .await;
    assert_eq!(
        completed.status, 202,
        "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
        completed.status,
        String::from_utf8_lossy(&completed.body)
    );
    // The 202 ACKNOWLEDGES the snapshot the seal was applied to, so it reports the
    // status the record was in when the request arrived; the sealed `completed` is what
    // every later call reads back (asserted in
    // `sealed_status_and_fixed_refusal_text.rs`).
    assert_eq!(
        completed
            .json()
            .get("Status")
            .and_then(serde_json::Value::as_str),
        Some("pending"),
        "case {case:?}: CompleteSnapshot must acknowledge the status it sealed, got {}",
        completed.json()
    );

    snapshot_id
}

/// One row of a write-then-read table: a human-readable case name, the volume size to
/// start the snapshot with, and the `(BlockIndex, byte seed)` pairs to write into it.
/// `pub(crate)` so a sibling suite tabulating the same three columns names this type
/// rather than restating it.
pub(crate) type WriteCase<'a> = (&'a str, i64, &'a [(i32, u8)]);

/// RULE 1 + 2: whatever set of block indexes was written, the listing reports exactly
/// those indexes ascending and each block reads back byte-identical with a `DataLength`
/// and `Checksum` derived from the stored bytes.
#[tokio::test]
async fn blocks_read_back_byte_identical_for_every_written_layout() {
    // Rows differ along what the rule generalises over: how many blocks, which indexes,
    // whether the indexes are contiguous, and the bytes in each block.
    let cases: &[WriteCase<'_>] = &[
        ("one block at index 0", 1, &[(0, 1)]),
        ("two adjacent blocks", 2, &[(0, 7), (1, 8)]),
        ("blocks written out of index order", 4, &[(5, 21), (2, 22), (3, 23)]),
        ("sparse indexes far apart", 64, &[(0, 31), (7, 32), (4096, 33)]),
    ];

    for (case, volume_size, blocks) in cases {
        let router = router();
        let snapshot_id = write_snapshot(&router, case, *volume_size, blocks).await;

        let listed = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListSnapshotBlocks must answer 200, got {} with body {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );

        let mut expected_indexes: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        expected_indexes.sort_unstable();
        let reported = listed_blocks(&listed);
        let reported_indexes: Vec<i32> = reported.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            reported_indexes, expected_indexes,
            "case {case:?}: ListSnapshotBlocks must report exactly the persisted indexes ascending, got {reported_indexes:?}"
        );
        assert_eq!(
            listed.json().get("BlockSize").and_then(serde_json::Value::as_i64),
            Some(i64::from(BLOCK_SIZE)),
            "case {case:?}: ListSnapshotBlocks must report the stored BlockSize, got {}",
            listed.json()
        );
        assert_eq!(
            listed.json().get("VolumeSize").and_then(serde_json::Value::as_i64),
            Some(*volume_size),
            "case {case:?}: ListSnapshotBlocks must report the stored VolumeSize, got {}",
            listed.json()
        );
        assert!(
            listed.json().get("ExpiryTime").is_some(),
            "case {case:?}: ListSnapshotBlocks must report the stored ExpiryTime, got {}",
            listed.json()
        );
        assert!(
            listed.json().get("NextToken").is_none(),
            "case {case:?}: a listing with no further blocks must omit NextToken, got {}",
            listed.json()
        );

        for (block_index, token) in &reported {
            assert!(
                is_modelled_block_token(token),
                "case {case:?}: block {block_index} must carry a token matching ^[A-Za-z0-9+/=]+$, got {token:?}"
            );
            let seed = blocks
                .iter()
                .find(|(index, _)| index == block_index)
                .map(|(_, seed)| *seed)
                .expect("every listed index was written");
            let expected_bytes = block_bytes(seed);
            let fetched = get_block(&router, &snapshot_id, *block_index, token).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: GetSnapshotBlock at index {block_index} must answer 200, got {} with body {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert!(
                fetched.body == expected_bytes,
                "case {case:?}: GetSnapshotBlock at index {block_index} must return the stored bytes; got {} bytes with checksum {:?}",
                fetched.body.len(),
                checksum(&fetched.body)
            );
            assert_eq!(
                fetched.header("x-amz-Data-Length"),
                BLOCK_SIZE.to_string(),
                "case {case:?}: GetSnapshotBlock at index {block_index} must report the stored length, got {:?}",
                fetched.header("x-amz-Data-Length")
            );
            assert_eq!(
                fetched.header("x-amz-Checksum"),
                checksum(&expected_bytes),
                "case {case:?}: GetSnapshotBlock at index {block_index} must report the checksum of the stored bytes, got {:?}",
                fetched.header("x-amz-Checksum")
            );
            assert_eq!(
                fetched.header("x-amz-Checksum-Algorithm"),
                "SHA256",
                "case {case:?}: GetSnapshotBlock at index {block_index} must report SHA256, got {:?}",
                fetched.header("x-amz-Checksum-Algorithm")
            );
        }
    }
}

/// RULE 2: `StartingBlockIndex` starts the listing at that index or the next one
/// present, over a table of requested starting points.
#[tokio::test]
async fn listing_starts_at_the_requested_block_index() {
    let router = router();
    let written: &[(i32, u8)] = &[(0, 41), (3, 42), (9, 43), (10, 44)];
    let snapshot_id = write_snapshot(&router, "starting-index fixture", 8, written).await;

    // Rows span: before the first index, exactly on an index, between two indexes,
    // on the last index, and past every index.
    let cases: &[(&str, i32, &[i32])] = &[
        ("start before the first block", 0, &[0, 3, 9, 10]),
        ("start exactly on a written index", 3, &[3, 9, 10]),
        ("start between two written indexes", 4, &[9, 10]),
        ("start on the last written index", 10, &[10]),
        ("start past every written index", 11, &[]),
    ];

    for (case, starting_block_index, expected) in cases {
        let listed = list_blocks(
            &router,
            &snapshot_id,
            &format!("startingBlockIndex={starting_block_index}"),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListSnapshotBlocks must answer 200, got {} with body {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let reported: Vec<i32> = listed_blocks(&listed)
            .iter()
            .map(|(index, _)| *index)
            .collect();
        assert_eq!(
            reported,
            expected.to_vec(),
            "case {case:?}: listing from {starting_block_index} must report {expected:?}, got {reported:?}"
        );
    }
}

/// RULE 2: a page carries at most `MaxResults` blocks and a `NextToken` only while more
/// remain; following that token resumes exactly where the page stopped.
#[tokio::test]
async fn pagination_carries_at_most_max_results_and_resumes_from_next_token() {
    // The model bounds `MaxResults` at 100..10000, so the smallest page a client may ask
    // for is 100 — one more block than that is the smallest input that must paginate.
    let page_size = 100_i32;
    let total = 101_i32;
    let router = router();
    let blocks: Vec<(i32, u8)> = (0..total)
        .map(|index| (index, u8::try_from(index % 251).expect("below 251")))
        .collect();
    let snapshot_id = write_snapshot(&router, "pagination fixture", 128, &blocks).await;

    let first = list_blocks(&router, &snapshot_id, &format!("maxResults={page_size}")).await;
    assert_eq!(
        first.status, 200,
        "case \"first page\": ListSnapshotBlocks must answer 200, got {} with body {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let first_indexes: Vec<i32> = listed_blocks(&first).iter().map(|(i, _)| *i).collect();
    assert_eq!(
        first_indexes,
        (0..page_size).collect::<Vec<i32>>(),
        "case \"first page\": a page must carry at most {page_size} blocks, ascending; got {first_indexes:?}"
    );
    let next_token = first
        .json()
        .get("NextToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_modelled_block_token(&next_token),
        "case \"first page\": a page with more blocks remaining must carry a NextToken matching ^[A-Za-z0-9+/=]+$, got {next_token:?}"
    );

    let second = list_blocks(
        &router,
        &snapshot_id,
        &format!("maxResults={page_size}&pageToken={}", query_escape(&next_token)),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case \"second page\": ListSnapshotBlocks must answer 200, got {} with body {}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    let second_indexes: Vec<i32> = listed_blocks(&second).iter().map(|(i, _)| *i).collect();
    assert_eq!(
        second_indexes,
        vec![total - 1],
        "case \"second page\": following NextToken must resume at the first unreported block; got {second_indexes:?}"
    );
    assert!(
        second.json().get("NextToken").is_none(),
        "case \"second page\": the last page must omit NextToken, got {}",
        second.json()
    );
}

/// RULE 3: every `StartSnapshot` reports the record it stored — a fresh modelled id, the
/// `pending` status, the emulator's account as `OwnerId`, the supported block size and
/// the description/tags/encryption it persisted.
#[tokio::test]
async fn start_snapshot_reports_the_snapshot_it_stored() {
    let router = router();

    // Rows differ along volume size (including the documented maximum), description,
    // tag count, timeout and encryption inputs. The third column is the `SseType` the
    // response must carry, or `None` where the snapshot has no SSE type and the member
    // must be ABSENT.
    let cases: &[(&str, serde_json::Value, Option<&str>)] = &[
        (
            "minimum volume, nothing optional",
            serde_json::json!({ "VolumeSize": 1 }),
            None,
        ),
        (
            "description and one tag",
            serde_json::json!({
                "VolumeSize": 8,
                "Description": "a described snapshot",
                "Tags": [{ "Key": "team", "Value": "storage" }],
            }),
            None,
        ),
        (
            "three tags and an explicit timeout",
            serde_json::json!({
                "VolumeSize": 16,
                "Timeout": 10,
                "Tags": [
                    { "Key": "a", "Value": "1" },
                    { "Key": "b", "Value": "2" },
                    { "Key": "c", "Value": "3" },
                ],
            }),
            None,
        ),
        (
            "encrypted with the EBS-managed key",
            serde_json::json!({ "VolumeSize": 32, "Encrypted": true }),
            Some("sse-ebs"),
        ),
        (
            "encrypted with a KMS key",
            serde_json::json!({
                "VolumeSize": 64,
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
            Some("sse-kms"),
        ),
        (
            // `VolumeSize`: "The maximum size is 65536 GiB (64 TiB)" — the published bound.
            "the documented maximum volume size",
            serde_json::json!({ "VolumeSize": 65536, "Timeout": 4320 }),
            None,
        ),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (case, request, expected_sse) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json();
        let snapshot_id = body
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_snapshot_id(&snapshot_id),
            "case {case:?}: SnapshotId must match ^snap-[0-9a-f]+$, got {snapshot_id:?}"
        );
        assert!(
            !seen_ids.contains(&snapshot_id),
            "case {case:?}: every StartSnapshot must mint a NEW id, but {snapshot_id:?} was already handed out"
        );
        seen_ids.push(snapshot_id.clone());

        assert_eq!(
            body.get("Status").and_then(serde_json::Value::as_str),
            Some("pending"),
            "case {case:?}: a new snapshot enters the pending state, got {body}"
        );
        assert_eq!(
            body.get("BlockSize").and_then(serde_json::Value::as_i64),
            Some(i64::from(BLOCK_SIZE)),
            "case {case:?}: BlockSize must be the supported block size, got {body}"
        );
        assert_eq!(
            body.get("OwnerId").and_then(serde_json::Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: OwnerId must be the emulator's account, got {body}"
        );
        assert!(
            body.get("StartTime").is_some(),
            "case {case:?}: StartTime must be reported, got {body}"
        );
        assert_eq!(
            body.get("SseType").and_then(serde_json::Value::as_str),
            *expected_sse,
            "case {case:?}: SseType must follow the encryption inputs, got {body}"
        );
        assert_eq!(
            body.get("VolumeSize").and_then(serde_json::Value::as_i64),
            request.get("VolumeSize").and_then(serde_json::Value::as_i64),
            "case {case:?}: VolumeSize must be the stored size, got {body}"
        );
        assert_eq!(
            body.get("Description").and_then(serde_json::Value::as_str),
            request.get("Description").and_then(serde_json::Value::as_str),
            "case {case:?}: Description must be the stored description, got {body}"
        );
        let expected_tags = request.get("Tags").and_then(serde_json::Value::as_array);
        let reported_tags = body.get("Tags").and_then(serde_json::Value::as_array);
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: the stored tags must be reported back key for key, and the member\
             must be absent where the snapshot has none, got {body}"
        );

        // Round trip: the snapshot the create reported is the one a read observes. The
        // read path sees a snapshot only once its content is final, so the snapshot is
        // sealed first; nothing was written to it, so the count it declares is zero.
        let sealed = complete_snapshot(&router, &snapshot_id, 0).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        let listed = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the new snapshot must be readable back, got {} with body {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        assert_eq!(
            listed.json().get("VolumeSize").and_then(serde_json::Value::as_i64),
            body.get("VolumeSize").and_then(serde_json::Value::as_i64),
            "case {case:?}: a read of the new snapshot must agree with the create, got {}",
            listed.json()
        );
        assert_eq!(
            listed_blocks(&listed),
            Vec::new(),
            "case {case:?}: a snapshot with no blocks written must list none"
        );
    }
}

/// RULE 4: a request naming something that is not stored, or breaking a documented
/// constraint, fails with the modelled error and status for that condition — never a 501
/// and never a hollow success.
#[tokio::test]
async fn refusals_carry_the_modelled_error_and_status() {
    let router = router();

    // A sealed snapshot with one block, to refuse writes against and to borrow a token
    // from.
    let sealed = write_snapshot(&router, "refusal fixture", 1, &[(0, 97)]).await;
    let sealed_token = listed_blocks(&list_blocks(&router, &sealed, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the sealed snapshot lists its block");

    // A second sealed snapshot, so a token minted for one snapshot can be tried against
    // another READABLE one: a token that names no block of the snapshot it is presented
    // with is the token refusal, which only a snapshot the read path can see can report.
    let other_sealed = write_snapshot(&router, "second refusal fixture", 1, &[(0, 98)]).await;

    // A second, still-pending snapshot, so a write can be refused against one.
    let pending = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let pending_id = pending
        .json()
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let data = block_bytes(5);
    let data_checksum = checksum(&data);
    assert_eq!(
        put_block(&router, &pending_id, 0, data.clone(), BLOCK_SIZE, &data_checksum)
            .await
            .status,
        201,
        "the pending fixture snapshot must accept a block"
    );

    // An id that was never started.
    let missing = "snap-deadbeef";

    let expectations: Vec<(&str, Reply, u16, &str)> = vec![
        (
            "ListSnapshotBlocks on a snapshot that was never started",
            list_blocks(&router, missing, "").await,
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetSnapshotBlock on a snapshot that was never started",
            get_block(&router, missing, 0, &sealed_token).await,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CompleteSnapshot on a snapshot that was never started",
            complete_snapshot(&router, missing, 0).await,
            404,
            "ResourceNotFoundException",
        ),
        (
            "PutSnapshotBlock on a snapshot that was never started",
            put_block(&router, missing, 0, data.clone(), BLOCK_SIZE, &data_checksum).await,
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetSnapshotBlock with a token that names no block at the index",
            get_block(&router, &sealed, 4, &sealed_token).await,
            400,
            "ValidationException",
        ),
        (
            "StartSnapshot naming a parent that does not exist",
            start_snapshot(
                &router,
                &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": missing }),
            )
            .await,
            404,
            "ResourceNotFoundException",
        ),
        (
            "PutSnapshotBlock on a snapshot that is already completed",
            put_block(&router, &sealed, 1, data.clone(), BLOCK_SIZE, &data_checksum).await,
            400,
            "ValidationException",
        ),
        (
            "PutSnapshotBlock with a DataLength other than the supported block size",
            put_block(
                &router,
                &pending_id,
                1,
                vec![0_u8; 1024],
                1024,
                &checksum(&vec![0_u8; 1024]),
            )
            .await,
            400,
            "ValidationException",
        ),
        (
            // A 1 GiB volume addresses block indexes 0..2048 (1 GiB / 524288).
            "PutSnapshotBlock at a BlockIndex outside the snapshot's volume",
            put_block(
                &router,
                &pending_id,
                2048,
                data.clone(),
                BLOCK_SIZE,
                &data_checksum,
            )
            .await,
            400,
            "ValidationException",
        ),
        (
            "PutSnapshotBlock whose Checksum does not match the data",
            put_block(
                &router,
                &pending_id,
                1,
                data.clone(),
                BLOCK_SIZE,
                &checksum(&block_bytes(6)),
            )
            .await,
            // The payload is excluded from the request's signature and the checksum
            // header is what binds it, so a payload that is not the one the checksum
            // was computed over is the signing-integrity refusal, not a flat input
            // error. Asserted in full in `payload_checksum_integrity.rs`.
            403,
            "InvalidSignatureException",
        ),
        (
            "GetSnapshotBlock with a BlockToken this service never issued",
            get_block(&router, &sealed, 0, "bm90LWEtdG9rZW4=").await,
            400,
            "ValidationException",
        ),
        (
            "GetSnapshotBlock with a BlockToken belonging to another snapshot",
            get_block(&router, &other_sealed, 0, &sealed_token).await,
            400,
            "ValidationException",
        ),
        (
            "ListSnapshotBlocks with a NextToken this service never issued",
            list_blocks(&router, &sealed, "pageToken=bm90LWEtcGFnZS10b2tlbg==").await,
            400,
            "ValidationException",
        ),
        (
            // Shaped exactly like a real page token, but naming an index this snapshot
            // holds no block at: decodability alone must not be enough.
            "ListSnapshotBlocks with a forged NextToken naming an index with no block",
            list_blocks(
                &router,
                &sealed,
                &format!(
                    "pageToken={}",
                    query_escape(&aws_smithy_types::base64::encode(format!("{sealed}:7")))
                ),
            )
            .await,
            400,
            "ValidationException",
        ),
        (
            "StartSnapshot past the documented maximum volume size",
            start_snapshot(&router, &serde_json::json!({ "VolumeSize": 65537 })).await,
            400,
            "ValidationException",
        ),
        (
            "StartSnapshot specifying both Encrypted and ParentSnapshotId",
            start_snapshot(
                &router,
                &serde_json::json!({
                    "VolumeSize": 1,
                    "Encrypted": true,
                    "ParentSnapshotId": sealed.clone(),
                }),
            )
            .await,
            400,
            "ValidationException",
        ),
    ];

    for (case, reply, expected_status, expected_code) in expectations {
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with(expected_code),
            "case {case:?}: expected the {expected_code} error code, got {:?}",
            reply.error_code()
        );
    }

    // The refused writes left nothing behind: the pending snapshot, once sealed, holds
    // exactly the one block that succeeded. (Sealing is what makes it readable at all.)
    let sealed_pending = complete_snapshot(&router, &pending_id, 1).await;
    assert_eq!(
        sealed_pending.status, 202,
        "case \"refused writes\": CompleteSnapshot must answer 202, got {} with body {}",
        sealed_pending.status,
        String::from_utf8_lossy(&sealed_pending.body)
    );
    let still = listed_blocks(&list_blocks(&router, &pending_id, "").await);
    let still_indexes: Vec<i32> = still.iter().map(|(index, _)| *index).collect();
    assert_eq!(
        still_indexes,
        vec![0],
        "case \"refused writes\": a refused PutSnapshotBlock must store nothing, got {still_indexes:?}"
    );
}

/// RULE 5: a repeated `ClientToken` replays the first snapshot rather than creating a
/// second one; the same token with a different request is a `ConflictException`.
#[tokio::test]
async fn client_token_makes_start_snapshot_idempotent() {
    let router = router();

    // Rows differ along which members the repeated request carries, so idempotency is
    // asserted over more than one request shape.
    let cases: &[(&str, serde_json::Value)] = &[
        (
            "volume size only",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "token-volume-only" }),
        ),
        (
            "description and tags",
            serde_json::json!({
                "VolumeSize": 4,
                "Description": "idempotent",
                "Tags": [{ "Key": "k", "Value": "v" }],
                "ClientToken": "token-with-tags",
            }),
        ),
        (
            "encrypted with a timeout",
            serde_json::json!({
                "VolumeSize": 2,
                "Encrypted": true,
                "Timeout": 30,
                "ClientToken": "token-encrypted",
            }),
        ),
    ];

    for (case, request) in cases {
        let first = start_snapshot(&router, request).await;
        let second = start_snapshot(&router, request).await;
        assert_eq!(
            (first.status, second.status),
            (201, 201),
            "case {case:?}: both calls must answer 201, got {} and {}",
            first.status,
            second.status
        );
        let first_id = first.json().get("SnapshotId").cloned();
        let second_id = second.json().get("SnapshotId").cloned();
        assert_eq!(
            first_id, second_id,
            "case {case:?}: the same ClientToken must return the same SnapshotId, got {first_id:?} then {second_id:?}"
        );
        assert_eq!(
            first.json(),
            second.json(),
            "case {case:?}: the retry must replay the original answer"
        );

        // A different request under the same token is the modelled conflict.
        let mut changed = request.clone();
        changed["VolumeSize"] = serde_json::json!(999);
        let conflict = start_snapshot(&router, &changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: the same ClientToken with a different request must answer 409, got {} with body {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert!(
            conflict.error_code().ends_with("ConflictException"),
            "case {case:?}: expected the ConflictException error code, got {:?}",
            conflict.error_code()
        );
    }
}

/// `ParentSnapshotId` seeds the new snapshot with a COPY of the parent's blocks: the
/// child lists the same indexes, under its own tokens, and reads back the same bytes.
#[tokio::test]
async fn a_child_snapshot_starts_from_a_copy_of_the_parent_blocks() {
    let router = router();
    let written: &[(i32, u8)] = &[(0, 11), (4, 12)];
    let parent = write_snapshot(&router, "parent", 4, written).await;
    let parent_blocks = listed_blocks(&list_blocks(&router, &parent, "").await);

    let child_reply = start_snapshot(
        &router,
        &serde_json::json!({ "VolumeSize": 4, "ParentSnapshotId": parent.clone() }),
    )
    .await;
    assert_eq!(
        child_reply.status, 201,
        "case \"child\": StartSnapshot from a parent must answer 201, got {} with body {}",
        child_reply.status,
        String::from_utf8_lossy(&child_reply.body)
    );
    let child = child_reply
        .json()
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        child_reply
            .json()
            .get("ParentSnapshotId")
            .and_then(serde_json::Value::as_str),
        Some(parent.as_str()),
        "case \"child\": the stored ParentSnapshotId must be reported, got {}",
        child_reply.json()
    );

    // A block written to the CHILD, at an index the parent never held. Sealing the child
    // is what makes it readable, so the inherited blocks and this one are observed
    // afterwards, through the listing below.
    let new_bytes = block_bytes(13);
    let new_checksum = checksum(&new_bytes);
    let written_reply =
        put_block(&router, &child, 9, new_bytes.clone(), BLOCK_SIZE, &new_checksum).await;
    assert_eq!(
        written_reply.status, 201,
        "case \"child\": writing a new block to the child must answer 201, got {} with body {}",
        written_reply.status,
        String::from_utf8_lossy(&written_reply.body)
    );
    let sealed = complete_snapshot(&router, &child, 1).await;
    assert_eq!(
        sealed.status, 202,
        "case \"child\": CompleteSnapshot must seal the child snapshot, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
    assert_eq!(
        sealed.json().get("Status").and_then(serde_json::Value::as_str),
        Some("pending"),
        "case \"child\": the acknowledgement must report the status the seal was applied to, got {}",
        sealed.json()
    );
    assert_eq!(
        member(&complete_snapshot(&router, &child, 1).await, "Status"),
        "completed",
        "case \"child\": a later call must read the sealed status back off the record"
    );

    let child_blocks = listed_blocks(&list_blocks(&router, &child, "").await);
    let child_indexes: Vec<i32> = child_blocks.iter().map(|(index, _)| *index).collect();
    let mut expected_indexes: Vec<i32> = parent_blocks.iter().map(|(index, _)| *index).collect();
    expected_indexes.push(9);
    expected_indexes.sort_unstable();
    assert_eq!(
        child_indexes, expected_indexes,
        "case \"child\": the child must hold the parent's block indexes plus the one written to it, got {child_indexes:?}"
    );

    for (block_index, child_token) in &child_blocks {
        if *block_index == 9 {
            // The block written to the CHILD, not inherited: it reads back its own bytes.
            let fetched = get_block(&router, &child, 9, child_token).await;
            assert_eq!(
                fetched.status, 200,
                "case \"child\": GetSnapshotBlock at the written index must answer 200, got {} with body {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert!(
                fetched.body == new_bytes,
                "case \"child\": the block written to the child must read back its own bytes; got {} bytes with checksum {:?}",
                fetched.body.len(),
                checksum(&fetched.body)
            );
            continue;
        }
        let parent_token = parent_blocks
            .iter()
            .find(|(index, _)| index == block_index)
            .map(|(_, token)| token.clone())
            .expect("the parent lists the same index");
        assert_ne!(
            *child_token, parent_token,
            "case \"child\": block {block_index} must carry the child's own token, not the parent's"
        );
        let seed = written
            .iter()
            .find(|(index, _)| index == block_index)
            .map(|(_, seed)| *seed)
            .expect("every copied index was written");
        let fetched = get_block(&router, &child, *block_index, child_token).await;
        assert_eq!(
            fetched.status, 200,
            "case \"child\": GetSnapshotBlock at index {block_index} must answer 200, got {} with body {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert!(
            fetched.body == block_bytes(seed),
            "case \"child\": block {block_index} must read back the parent's bytes; got {} bytes with checksum {:?}",
            fetched.body.len(),
            checksum(&fetched.body)
        );
    }
}

/// A listing is a read: repeating it, and fetching through the tokens it handed out,
/// changes nothing a later caller sees.
#[tokio::test]
async fn reading_a_snapshot_does_not_change_what_a_later_caller_sees() {
    let router = router();
    let snapshot_id = write_snapshot(&router, "read-stability", 2, &[(0, 61), (1, 62)]).await;

    let first = list_blocks(&router, &snapshot_id, "").await;
    for (block_index, token) in &listed_blocks(&first) {
        assert_eq!(
            get_block(&router, &snapshot_id, *block_index, token)
                .await
                .status,
            200,
            "case \"read-stability\": block {block_index} must be fetchable through its listed token"
        );
    }
    let second = list_blocks(&router, &snapshot_id, "").await;
    assert_eq!(
        first.json(),
        second.json(),
        "case \"read-stability\": repeating a listing must report the same blocks, tokens and metadata"
    );
}

/// "If the specified block contains data, the existing data is overwritten": rewriting an
/// index leaves exactly one block at that index, holding the bytes of the LAST write,
/// under one token.
///
/// The rewrites are observed after the snapshot is sealed, because that is when the read
/// path can see it at all, and sealing is also what ends the writes: each row is therefore
/// its own snapshot, rewritten its own number of times.
#[tokio::test]
async fn rewriting_a_block_index_replaces_the_bytes_and_keeps_one_token() {
    // Rows differ in how many times the index is rewritten and in the bytes each write
    // carries, so the rule is asserted over a sequence of rewrites rather than one write.
    let cases: &[(&str, &[u8])] = &[
        ("written once", &[71]),
        ("rewritten once", &[71, 72]),
        ("rewritten twice", &[71, 72, 73]),
    ];

    for (case, seeds) in cases {
        let router = router();
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = started
            .json()
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        for seed in *seeds {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &snapshot_id, 0, data.clone(), BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: PutSnapshotBlock with seed {seed} must answer 201, got {} with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert_eq!(
                written.header("x-amz-Checksum"),
                expected,
                "case {case:?}: the write must report the checksum of the bytes it stored, got {:?}",
                written.header("x-amz-Checksum")
            );
        }

        // Sealing is what makes the snapshot readable; the listing below is what proves
        // one block was stored, however many times the index was rewritten.
        let sealed = complete_snapshot(&router, &snapshot_id, 1).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );

        let listed = listed_blocks(&list_blocks(&router, &snapshot_id, "").await);
        let indexes: Vec<i32> = listed.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            indexes,
            vec![0],
            "case {case:?}: rewriting index 0 must leave exactly one block, got {indexes:?}"
        );
        let token = listed
            .first()
            .map(|(_, token)| token.clone())
            .expect("the snapshot lists its one block");

        // The bytes of the LAST write are the ones stored.
        let last = block_bytes(*seeds.last().expect("every row writes at least once"));
        let fetched = get_block(&router, &snapshot_id, 0, &token).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSnapshotBlock must answer 200, got {} with body {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert!(
            fetched.body == last,
            "case {case:?}: GetSnapshotBlock must return the bytes stored by the latest write; got {} bytes with checksum {:?}",
            fetched.body.len(),
            checksum(&fetched.body)
        );
        assert_eq!(
            fetched.header("x-amz-Checksum"),
            checksum(&last),
            "case {case:?}: the checksum must be the one computed over the stored bytes, got {:?}",
            fetched.header("x-amz-Checksum")
        );
    }
}

/// This emulator holds snapshot state in memory, so the store it grows is capped. Past
/// the cap `StartSnapshot` answers the modelled `ServiceQuotaExceededException` (HTTP
/// 402) rather than growing without bound — and the snapshots already stored keep
/// working.
#[tokio::test]
async fn starting_more_snapshots_than_the_store_holds_is_refused_with_a_quota_error() {
    // Mirrors `crate::state::MAX_STORED_SNAPSHOTS`; a cap the test could not reach would
    // assert nothing.
    let capacity = 1024;
    let router = router();

    let mut first_id = String::new();
    for index in 0..capacity {
        let reply = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        assert_eq!(
            reply.status, 201,
            "case \"snapshot {index} of {capacity}\": StartSnapshot must answer 201 inside the cap, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        if index == 0 {
            first_id = reply
                .json()
                .get("SnapshotId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }

    let refused = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    assert_eq!(
        refused.status, 402,
        "case \"one past the cap\": StartSnapshot must answer 402, got {} with body {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert!(
        refused
            .error_code()
            .ends_with("ServiceQuotaExceededException"),
        "case \"one past the cap\": expected the ServiceQuotaExceededException error code, got {:?}",
        refused.error_code()
    );

    // The refusal did not disturb what was already stored. Sealing the first snapshot is
    // what makes it readable: the read path sees only a snapshot whose content is final.
    let sealed = complete_snapshot(&router, &first_id, 0).await;
    assert_eq!(
        sealed.status, 202,
        "case \"one past the cap\": CompleteSnapshot must answer 202, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
    let listed = list_blocks(&router, &first_id, "").await;
    assert_eq!(
        listed.status, 200,
        "case \"one past the cap\": the snapshots already stored must still be readable, got {} with body {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
}
