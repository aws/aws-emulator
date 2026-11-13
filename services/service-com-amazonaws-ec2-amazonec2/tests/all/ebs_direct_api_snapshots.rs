// SPDX-License-Identifier: CC0-1.0

//! The ONE snapshot resource the EBS DIRECT APIs and EC2 both describe, driven over both
//! services' own wires on one engine: `ebs:StartSnapshot` / `PutSnapshotBlock` /
//! `CompleteSnapshot` through restJson1, and `ec2:DescribeSnapshots` /
//! `Enable`/`DisableFastSnapshotRestores` / `CopySnapshot` through ec2Query.
//!
//! The rule under test: a snapshot an EBS direct-API call minted for the account is a
//! snapshot every EC2 read resolves and reports, with the state that API gives it —
//! `pending` while it is still being written, `completed` with `Progress` `100%` once
//! `CompleteSnapshot` seals it, and `error` once its `Timeout` cancels it — and the
//! `InvalidSnapshot.NotFound` refusal stays for an id nothing holds.
//!
//! Each table row is a distinct input along what the rule generalises over — the declared
//! volume size, whether a description or tags were asked for, whether the snapshot is
//! encrypted, how many blocks were written, which phase of the lifecycle the read lands in,
//! which narrowing selects it, and which EC2 operation resolves the id — so no row is the
//! specification.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ebs_ebs as ebs;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::state::{
    SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_PROGRESS_PENDING, SNAPSHOT_STATE_COMPLETED,
    SNAPSHOT_STATE_ERROR, SNAPSHOT_STATE_PENDING, SNAPSHOT_STORAGE_TIER_STANDARD,
};

use super::ec2_query::{as_timestamp, call, error_code, error_message, filters, indexed};
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The only block size the EBS direct APIs support: `PutSnapshotBlockRequest$DataLength`
/// documents "Currently, the only supported size is `524288` bytes".
const BLOCK_SIZE: i32 = 524_288;

/// A snapshot id of the long form no snapshot of the account carries, for the refusal table:
/// a well-formed id naming nothing is a different cause from a value that is not a snapshot
/// id at all.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// Both services on ONE engine, which is what makes the two sides of the snapshot resource
/// the same resource. The engine is handed back so the one row that needs a `Timeout` the
/// clock has already passed can move the instant the EBS store recorded for it.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ebs::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One EBS direct-API request: the service is picked by the `ebs` signing name in the
/// credential scope, exactly as a client's signer writes it.
pub(crate) async fn ebs_send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> (u16, serde_json::Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/ebs/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    );
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ebs.{REGION}.amazonaws.com"))
        .header("authorization", authorization);
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed EBS direct-API request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// `ebs:StartSnapshot`, answering the `SnapshotId` it minted.
pub(crate) async fn start_snapshot(router: &MultiServiceRouter, case: &str, body: serde_json::Value) -> String {
    let (status, answer) = ebs_send(
        router,
        "POST",
        "/snapshots",
        &[("content-type", "application/json".to_string())],
        serde_json::to_vec(&body).expect("a serializable StartSnapshot body"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from ebs:StartSnapshot, got {status} with {answer}"
    );
    answer
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: StartSnapshot reported no SnapshotId: {answer}"))
        .to_string()
}

/// `BLOCK_SIZE` bytes that differ per `index`, so no block can pass as another's.
fn block_bytes(index: i32) -> Vec<u8> {
    let length = usize::try_from(BLOCK_SIZE).expect("the block size fits a usize");
    (0..length)
        .map(|offset| {
            let mixed = (offset as u64)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(u64::from(index.unsigned_abs()).wrapping_mul(1_099_511_628_211));
            u8::try_from(mixed % 251).expect("a value below 251 fits a u8")
        })
        .collect()
}

/// `ebs:PutSnapshotBlock` of one whole block at `index`.
pub(crate) async fn put_block(router: &MultiServiceRouter, case: &str, snapshot_id: &str, index: i32) {
    let data = block_bytes(index);
    let checksum = ::aws_smithy_types::base64::encode(Sha256::digest(&data));
    let (status, answer) = ebs_send(
        router,
        "PUT",
        &format!("/snapshots/{snapshot_id}/blocks/{index}"),
        &[
            ("content-type", "application/octet-stream".to_string()),
            ("x-amz-Data-Length", BLOCK_SIZE.to_string()),
            ("x-amz-Checksum", checksum),
            ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
        ],
        data,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from ebs:PutSnapshotBlock, got {status} with {answer}"
    );
}

/// `ebs:CompleteSnapshot`, which seals the snapshot.
pub(crate) async fn complete_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
    blocks: i32,
) {
    let (status, answer) = ebs_send(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &[("x-amz-ChangedBlocksCount", blocks.to_string())],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected HTTP 202 from ebs:CompleteSnapshot, got {status} with {answer}"
    );
}

/// One snapshot an EC2 report carried: its scalar members as the text each was reported as,
/// and its tags as the pairs they were reported in.
#[derive(Debug, Clone, Default)]
struct Reported {
    members: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl Reported {
    /// The text of `name`, or the empty string when the report carried no such element —
    /// which is what an absent member and an empty one are told apart by below.
    fn member(&self, name: &str) -> &str {
        self.members.get(name).map_or("", String::as_str)
    }

    fn sorted_tags(&self) -> Vec<(String, String)> {
        let mut tags = self.tags.clone();
        tags.sort();
        tags
    }
}

/// Every snapshot a `DescribeSnapshots` answer reported, in report order.
fn reported_snapshots(case: &str, body: &str) -> Vec<Reported> {
    let document = super::wire::document(case, body);
    let mut snapshots = Vec::new();
    for item in super::wire::set_items(document.root_element(), "snapshotSet") {
        let mut reported = Reported::default();
        for member in item.children().filter(roxmltree::Node::is_element) {
            if member.tag_name().name() == "tagSet" {
                for tag in member.children().filter(roxmltree::Node::is_element) {
                    reported.tags.push((
                        super::wire::child_text(tag, "key").unwrap_or_default(),
                        super::wire::child_text(tag, "value").unwrap_or_default(),
                    ));
                }
                continue;
            }
            reported.members.insert(
                member.tag_name().name().to_string(),
                member.text().unwrap_or_default().to_string(),
            );
        }
        snapshots.push(reported);
    }
    snapshots
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeSnapshots", params).await
}

/// The snapshots `DescribeSnapshots` reports for `params`, asserting it answered at all.
async fn described(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<Reported> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    reported_snapshots(case, &body)
}

/// The ids `DescribeSnapshots` reports for `params`, in report order.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    described(router, case, params)
        .await
        .iter()
        .map(|snapshot| snapshot.member("snapshotId").to_string())
        .collect()
}

/// The one snapshot a report of `snapshot_id` carried, or a panic naming the case: the read
/// under test is "does this id resolve on the EC2 side at all".
async fn described_one(router: &MultiServiceRouter, case: &str, snapshot_id: &str) -> Reported {
    let params = indexed("SnapshotId", &[snapshot_id]);
    let reported = described(router, case, &params).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the snapshot {snapshot_id}, got {reported:?}"
    );
    reported.into_iter().next().expect("one reported snapshot")
}

/// One row of the lifecycle table: a direct-API snapshot differing in the volume size it
/// declares, whether it was asked for a description, how many tags it carries, whether it is
/// encrypted, and how many blocks are written into it before it is sealed.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    volume_size: i64,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    encrypted: bool,
    blocks: i32,
}

/// The rows every table below reads.
const ROWS: [Row; 4] = [
    Row {
        case: "a 1 GiB snapshot, undescribed, untagged, one block",
        volume_size: 1,
        description: None,
        tags: &[],
        encrypted: false,
        blocks: 1,
    },
    Row {
        case: "a 1 GiB described snapshot with one tag, two blocks",
        volume_size: 1,
        description: Some("direct api upload"),
        tags: &[("Purpose", "probe")],
        encrypted: false,
        blocks: 2,
    },
    Row {
        case: "a 4 GiB encrypted snapshot with two tags, no block written",
        volume_size: 4,
        description: Some("encrypted direct api upload"),
        tags: &[("Purpose", "probe"), ("Stage", "test")],
        encrypted: true,
        blocks: 0,
    },
    Row {
        case: "a 64 GiB snapshot at the largest page a row needs, three blocks",
        volume_size: 64,
        description: Some("wide volume"),
        tags: &[],
        encrypted: false,
        blocks: 3,
    },
];

impl Row {
    /// The `StartSnapshot` body this row asks for.
    fn start_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert("VolumeSize".to_string(), self.volume_size.into());
        if let Some(description) = self.description {
            body.insert("Description".to_string(), description.into());
        }
        if self.encrypted {
            body.insert("Encrypted".to_string(), true.into());
        }
        if !self.tags.is_empty() {
            body.insert(
                "Tags".to_string(),
                self.tags
                    .iter()
                    .map(|(key, value)| {
                        serde_json::json!({ "Key": *key, "Value": *value })
                    })
                    .collect(),
            );
        }
        serde_json::Value::Object(body)
    }

    /// This row's tags as the pairs a report must carry, in key order.
    fn expected_tags(&self) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = self
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        tags.sort();
        tags
    }
}

/// Start the snapshot of `row`, write its blocks, and answer its id — the whole write side
/// of the loop, driven over the EBS direct-API wire.
async fn write_snapshot(router: &MultiServiceRouter, row: &Row) -> String {
    let snapshot_id = start_snapshot(router, row.case, row.start_body()).await;
    for index in 0..row.blocks {
        put_block(router, row.case, &snapshot_id, index).await;
    }
    snapshot_id
}

/// RULE, pending half: a snapshot `ebs:StartSnapshot` minted is reported by
/// `ec2:DescribeSnapshots` as `pending` with every member read off that snapshot, before
/// anything seals it.
#[tokio::test]
async fn a_started_direct_api_snapshot_is_described_as_pending_with_its_own_members() {
    let account = ::engine::emulator_account_id();
    for row in ROWS {
        let (router, _engine) = emulator();
        let snapshot_id = write_snapshot(&router, &row).await;

        let reported = described_one(&router, row.case, &snapshot_id).await;
        let expectations: &[(&str, String)] = &[
            ("snapshotId", snapshot_id.clone()),
            ("status", SNAPSHOT_STATE_PENDING.to_string()),
            ("progress", SNAPSHOT_PROGRESS_PENDING.to_string()),
            ("ownerId", account.to_string()),
            ("volumeSize", row.volume_size.to_string()),
            ("encrypted", row.encrypted.to_string()),
            ("storageTier", SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
            (
                "description",
                row.description.unwrap_or_default().to_string(),
            ),
        ];
        for (member, expected) in expectations {
            assert_eq!(
                reported.member(member),
                expected.as_str(),
                "case {:?}: expected {member} {expected:?}, got {:?} ({reported:?})",
                row.case,
                reported.member(member)
            );
        }
        assert_eq!(
            reported.sorted_tags(),
            row.expected_tags(),
            "case {:?}: expected the tags the start asked for, got {:?}",
            row.case,
            reported.sorted_tags()
        );
        assert!(
            as_timestamp(reported.member("startTime")).is_some(),
            "case {:?}: expected a startTime naming an instant, got {:?}",
            row.case,
            reported.member("startTime")
        );
        // "The time stamp when the snapshot was completed": a snapshot nothing has sealed
        // has none, and nothing has measured its full size either.
        for absent in ["completionTime", "fullSnapshotSizeInBytes", "volumeId"] {
            assert!(
                !reported.members.contains_key(absent),
                "case {:?}: expected no {absent} on a pending direct-API snapshot, got {:?}",
                row.case,
                reported.member(absent)
            );
        }
    }
}

/// RULE, completed half: `ebs:CompleteSnapshot` drives the SAME snapshot to `completed` with
/// `Progress` `100%`, a completion time and the size of the blocks it holds — and the read
/// that observes it changes nothing a later caller sees.
#[tokio::test]
async fn completing_a_direct_api_snapshot_is_described_as_completed_at_full_progress() {
    for row in ROWS {
        let (router, _engine) = emulator();
        let snapshot_id = write_snapshot(&router, &row).await;
        let started = described_one(&router, row.case, &snapshot_id).await;
        complete_snapshot(&router, row.case, &snapshot_id, row.blocks).await;

        let sealed = described_one(&router, row.case, &snapshot_id).await;
        assert_eq!(
            sealed.member("status"),
            SNAPSHOT_STATE_COMPLETED,
            "case {:?}: expected a sealed snapshot to be {SNAPSHOT_STATE_COMPLETED}, got {:?}",
            row.case,
            sealed.member("status")
        );
        assert_eq!(
            sealed.member("progress"),
            SNAPSHOT_PROGRESS_COMPLETED,
            "case {:?}: expected progress {SNAPSHOT_PROGRESS_COMPLETED}, got {:?}",
            row.case,
            sealed.member("progress")
        );
        let completion = as_timestamp(sealed.member("completionTime"));
        let start = as_timestamp(sealed.member("startTime"));
        assert!(
            completion.is_some() && completion >= start,
            "case {:?}: expected a completionTime at or after the startTime, got {:?} after {:?}",
            row.case,
            sealed.member("completionTime"),
            sealed.member("startTime")
        );
        let expected_size = i64::from(row.blocks) * i64::from(BLOCK_SIZE);
        assert_eq!(
            sealed.member("fullSnapshotSizeInBytes"),
            expected_size.to_string(),
            "case {:?}: expected the {} written block(s) to measure {expected_size} bytes, got {:?}",
            row.case,
            row.blocks,
            sealed.member("fullSnapshotSizeInBytes")
        );
        // The members that are facts of the snapshot rather than of its phase do not move
        // when it is sealed.
        for carried in ["snapshotId", "ownerId", "volumeSize", "encrypted", "startTime"] {
            assert_eq!(
                sealed.member(carried),
                started.member(carried),
                "case {:?}: expected {carried} unchanged by the seal, got {:?} after {:?}",
                row.case,
                sealed.member(carried),
                started.member(carried)
            );
        }
        // A read writes nothing: the next read of the same snapshot says the same thing.
        let again = described_one(&router, row.case, &snapshot_id).await;
        assert_eq!(
            again.members, sealed.members,
            "case {:?}: expected two reads of one snapshot to agree, got {:?} then {:?}",
            row.case, sealed.members, again.members
        );
    }
}

/// RULE, cancelled third: a snapshot whose `Timeout` has elapsed unsealed is reported
/// `error`, which is the state the EBS direct APIs give it too.
///
/// `StartSnapshotRequest$Timeout`'s smallest value is 10 minutes, so the row cannot wait the
/// clock out: it moves the expiry instant the EBS store recorded for this snapshot into the
/// past — the only thing a 10-minute wait would have done — and reads the EC2 side again.
#[tokio::test]
async fn a_direct_api_snapshot_whose_timeout_elapsed_is_described_as_error() {
    let row = ROWS[0];
    let (router, engine) = emulator();
    let snapshot_id = write_snapshot(&router, &row).await;
    let pending = described_one(&router, row.case, &snapshot_id).await;
    assert_eq!(
        pending.member("status"),
        SNAPSHOT_STATE_PENDING,
        "case {:?}: expected the snapshot to start out pending, got {:?}",
        row.case,
        pending.member("status")
    );

    {
        let state = ebs::state::service_state(&*engine).expect("the EBS store");
        let mut store = state.lock();
        let snapshot = store
            .snapshots
            .get_mut(&snapshot_id)
            .expect("the started snapshot");
        snapshot.expiry_time = SystemTime::now() - Duration::from_secs(1);
    }

    let cancelled = described_one(&router, row.case, &snapshot_id).await;
    assert_eq!(
        cancelled.member("status"),
        SNAPSHOT_STATE_ERROR,
        "case {:?}: expected a cancelled snapshot to be {SNAPSHOT_STATE_ERROR}, got {:?}",
        row.case,
        cancelled.member("status")
    );
    assert_eq!(
        cancelled.member("progress"),
        SNAPSHOT_PROGRESS_PENDING,
        "case {:?}: expected no progress on a cancelled snapshot, got {:?}",
        row.case,
        cancelled.member("progress")
    );
}

/// RULE over the NARROWINGS: a direct-API snapshot is selected by exactly the members and
/// filters whose attribute it carries, the same way one of this service's own creates is.
#[tokio::test]
async fn every_narrowing_selects_the_direct_api_snapshots_whose_attribute_it_names() {
    let account = ::engine::emulator_account_id();
    let (router, _engine) = emulator();
    // One snapshot left pending and one sealed, differing in size, description and tags, so
    // each row below selects a known subset of the two.
    let pending_row = ROWS[1];
    let sealed_row = ROWS[3];
    let pending = write_snapshot(&router, &pending_row).await;
    let sealed = write_snapshot(&router, &sealed_row).await;
    complete_snapshot(&router, sealed_row.case, &sealed, sealed_row.blocks).await;
    let mut both = vec![pending.clone(), sealed.clone()];
    both.sort();

    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no narrowing at all", Vec::new(), both.clone()),
        (
            "one SnapshotId",
            indexed("SnapshotId", &[pending.as_str()]),
            vec![pending.clone()],
        ),
        (
            "both SnapshotIds",
            indexed("SnapshotId", &[sealed.as_str(), pending.as_str()]),
            both.clone(),
        ),
        (
            "OwnerIds self",
            indexed("Owner", &["self"]),
            both.clone(),
        ),
        (
            "OwnerIds the account id",
            indexed("Owner", &[account]),
            both.clone(),
        ),
        (
            "OwnerIds amazon, which no account snapshot is published under",
            indexed("Owner", &["amazon"]),
            Vec::new(),
        ),
        (
            "RestorableByUserIds self",
            indexed("RestorableBy", &["self"]),
            both.clone(),
        ),
        (
            "the status filter on the pending one",
            filters(&[("status", &[SNAPSHOT_STATE_PENDING])]),
            vec![pending.clone()],
        ),
        (
            "the status filter on the sealed one",
            filters(&[("status", &[SNAPSHOT_STATE_COMPLETED])]),
            vec![sealed.clone()],
        ),
        (
            "the progress filter on the sealed one",
            filters(&[("progress", &[SNAPSHOT_PROGRESS_COMPLETED])]),
            vec![sealed.clone()],
        ),
        (
            "the volume-size filter",
            filters(&[("volume-size", &[&sealed_row.volume_size.to_string()])]),
            vec![sealed.clone()],
        ),
        (
            "the description filter",
            filters(&[(
                "description",
                &[pending_row.description.unwrap_or_default()],
            )]),
            vec![pending.clone()],
        ),
        (
            "a tag filter on the key the pending one carries",
            filters(&[("tag:Purpose", &["probe"])]),
            vec![pending.clone()],
        ),
        (
            "the tag-key filter",
            filters(&[("tag-key", &["Purpose"])]),
            vec![pending.clone()],
        ),
        (
            "the snapshot-id filter",
            filters(&[("snapshot-id", &[sealed.as_str()])]),
            vec![sealed.clone()],
        ),
        (
            "the owner-id filter",
            filters(&[("owner-id", &[account])]),
            both.clone(),
        ),
        (
            "the storage-tier filter",
            filters(&[("storage-tier", &[SNAPSHOT_STORAGE_TIER_STANDARD])]),
            both.clone(),
        ),
        (
            "the encrypted filter, which neither row asked for",
            filters(&[("encrypted", &["true"])]),
            Vec::new(),
        ),
        (
            "the owner-alias filter, which only an Amazon-published snapshot carries",
            filters(&[("owner-alias", &["amazon"])]),
            Vec::new(),
        ),
        (
            "the transfer-type filter: every snapshot is the standard transfer",
            filters(&[("transfer-type", &["standard"])]),
            both.clone(),
        ),
        (
            "two filters, which are joined with an AND",
            filters(&[
                ("status", &[SNAPSHOT_STATE_COMPLETED]),
                ("tag-key", &["Purpose"]),
            ]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        // Narrowed to the snapshots the ACCOUNT owns: the inventory also carries the public
        // snapshots of the published AMI catalogue, which is the sibling suite's subject
        // ([`super::describe_snapshots_public_catalogue`]).
        let narrowed = super::describe_snapshots::the_accounts_own(&params);
        let reported = described_ids(&router, case, &narrowed).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} in SnapshotId order, got {reported:?}"
        );
    }
}

/// RULE over the REPORT: one answer carries the snapshots of BOTH sides of the resource, in
/// `SnapshotId` order, and `MaxResults` pages across the two of them.
#[tokio::test]
async fn one_report_carries_both_this_services_own_snapshots_and_the_direct_api_ones() {
    let (router, _engine) = emulator();
    // One snapshot of this service's own store, made the way its own writer makes one: a
    // volume, then a snapshot of it.
    let (status, body) = call(
        &router,
        REGION,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone_name(0)),
            p("VolumeType", "gp3"),
            p("Size", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected a created volume, got {status}: {body}");
    let volume_id = super::ec2_query::element(&body, "volumeId").expect("a created volume id");
    past_the_transition().await;
    let (status, body) = call(
        &router,
        REGION,
        "CreateSnapshot",
        &[p("VolumeId", volume_id.clone())],
    )
    .await;
    assert_eq!(status, 200, "expected a created snapshot, got {status}: {body}");
    let own = super::ec2_query::element(&body, "snapshotId").expect("a created snapshot id");

    let direct = write_snapshot(&router, &ROWS[0]).await;
    complete_snapshot(&router, ROWS[0].case, &direct, ROWS[0].blocks).await;

    let mut expected = vec![own.clone(), direct.clone()];
    expected.sort();
    let reported = described_ids(
        &router,
        "both sides at once",
        &super::describe_snapshots::the_accounts_own(&[]),
    )
    .await;
    assert_eq!(
        reported, expected,
        "case both sides at once: expected {expected:?} in SnapshotId order, got {reported:?}"
    );

    // A page of one walks the merged sequence, each snapshot once.
    let mut walked: Vec<String> = Vec::new();
    let mut params: Params =
        super::describe_snapshots::the_accounts_own(&[p("MaxResults", "1")]);
    loop {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case a page of one: expected HTTP 200, got {status} with body {body}"
        );
        let page = reported_snapshots("a page of one", &body);
        assert!(
            page.len() <= 1,
            "case a page of one: expected at most one snapshot a page, got {page:?}"
        );
        walked.extend(page.iter().map(|snapshot| snapshot.member("snapshotId").to_string()));
        match super::ec2_query::member(&body, "nextToken") {
            Some(token) if !token.is_empty() => {
                params = super::describe_snapshots::the_accounts_own(&[
                    p("MaxResults", "1"),
                    p("NextToken", token),
                ]);
            }
            _ => break,
        }
    }
    assert_eq!(
        walked, expected,
        "case a page of one: expected the whole merged set once each, got {walked:?}"
    );
}

/// RULE over the OTHER readers: every EC2 operation that resolves a `SnapshotId` resolves one
/// the EBS direct APIs minted — and still refuses an id nothing holds, with EC2's own code.
#[tokio::test]
async fn the_other_ec2_snapshot_readers_resolve_a_direct_api_snapshot_and_still_refuse_an_unknown_one()
 {
    let row = ROWS[0];
    let (router, _engine) = emulator();
    let snapshot_id = write_snapshot(&router, &row).await;
    complete_snapshot(&router, row.case, &snapshot_id, row.blocks).await;

    // `CopySnapshot` reads the source's own facts off the snapshot it resolves.
    let (status, body) = call(
        &router,
        REGION,
        "CopySnapshot",
        &[
            p("SourceRegion", REGION),
            p("SourceSnapshotId", snapshot_id.clone()),
            p("Description", "a copy of a direct-API snapshot"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case CopySnapshot of a direct-API snapshot: expected HTTP 200, got {status}: {body}"
    );
    let copy_id = super::ec2_query::element(&body, "snapshotId").expect("a copied snapshot id");
    let copy = described_one(&router, "the copy of a direct-API snapshot", &copy_id).await;
    assert_eq!(
        copy.member("volumeSize"),
        row.volume_size.to_string(),
        "case the copy of a direct-API snapshot: expected the source's volume size {}, got {:?}",
        row.volume_size,
        copy.member("volumeSize")
    );

    // The fast snapshot restores resolve the id per pair rather than refusing the request.
    let (status, body) = call(
        &router,
        REGION,
        "EnableFastSnapshotRestores",
        &[
            p("SourceSnapshotId.1", snapshot_id.clone()),
            p("AvailabilityZone.1", zone_name(0)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case EnableFastSnapshotRestores on a direct-API snapshot: expected HTTP 200, got \
         {status}: {body}"
    );
    assert!(
        body.contains(&snapshot_id) && !body.contains("InvalidSnapshot.NotFound"),
        "case EnableFastSnapshotRestores on a direct-API snapshot: expected the pair to be \
         enabled for it, got {body}"
    );

    // And the disable resolves the same id rather than reporting it unresolvable.
    let (status, body) = call(
        &router,
        REGION,
        "DisableFastSnapshotRestores",
        &[
            p("SourceSnapshotId.1", snapshot_id.clone()),
            p("AvailabilityZone.1", zone_name(0)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case DisableFastSnapshotRestores on a direct-API snapshot: expected HTTP 200, got \
         {status}: {body}"
    );
    assert!(
        body.contains(&snapshot_id) && !body.contains("InvalidSnapshot.NotFound"),
        "case DisableFastSnapshotRestores on a direct-API snapshot: expected the pair to be \
         reported for it, got {body}"
    );

    // And the refusals stay what they were for an id that really does not exist.
    let rows: Vec<(&str, &str, Params, &str)> = vec![
        (
            "a well-formed snapshot id nothing holds",
            "DescribeSnapshots",
            indexed("SnapshotId", &[UNKNOWN_SNAPSHOT_ID]),
            "InvalidSnapshot.NotFound",
        ),
        (
            "a value that is not a snapshot id at all",
            "DescribeSnapshots",
            indexed("SnapshotId", &["not-a-snapshot"]),
            "InvalidParameterValue",
        ),
        (
            // `CopySnapshot` answers a source it cannot resolve with its own recorded
            // refusal, which is about the PARAMETER rather than about a snapshot that does
            // not exist — unchanged by this fix.
            "a copy whose source nothing holds",
            "CopySnapshot",
            vec![
                p("SourceRegion", REGION),
                p("SourceSnapshotId", UNKNOWN_SNAPSHOT_ID),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, action, params, expected) in rows {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {:?} ({body})",
            error_code(&body)
        );
    }
    let (_, body) = describe(&router, &indexed("SnapshotId", &[UNKNOWN_SNAPSHOT_ID])).await;
    assert_eq!(
        error_message(&body),
        format!("The snapshot '{UNKNOWN_SNAPSHOT_ID}' does not exist."),
        "case a well-formed snapshot id nothing holds: expected EC2's own sentence for it, got \
         {:?}",
        error_message(&body)
    );
}
