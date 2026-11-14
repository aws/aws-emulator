// SPDX-License-Identifier: CC0-1.0

//! The domain data plane's SNAPSHOT API, driven over the real wire path: a signed request to
//! the endpoint a domain published registers a snapshot repository, takes a snapshot into the
//! Amazon S3 bucket that repository names, reads it back and deletes it.
//!
//! Four rules, each over a SET of inputs rather than one recorded call:
//!
//! * a snapshot WRITES the repository's blobs into the bucket its settings name, every one of
//!   them under the declared `base_path`, and deleting the snapshot removes exactly the blobs
//!   it wrote — over a table spanning the forms a base path can take (a nested path, a single
//!   segment, none at all, one written with leading and trailing separators) and what the
//!   snapshot copies (one index, one index with several documents, two indexes);
//! * the blobs are the repository's own data: a repository registered AGAIN at a location the
//!   bucket already holds blobs at reports the snapshots those blobs state;
//! * a restore reads an index back out of those blobs — its settings, its mappings and every
//!   document — including under a renamed target, and refuses a target the cluster already
//!   holds;
//! * the snapshot API refuses what the cluster refuses: a repository nothing registered, a
//!   definition with no type or a type the cluster has no plugin for, settings naming no
//!   bucket, a bucket the object store does not hold, a snapshot name the engine will not
//!   take, a name already taken, and a snapshot the repository does not hold.
//!
//! The bucket is the one Amazon S3 itself holds in the SAME shared engine — the store
//! `s3:CreateBucket` and `s3:PutObject` write — so what the assertions read is where a client
//! of that service sees it: `Bucket::objects` is the read seam `s3:ListObjectsV2` lists a
//! bucket's current objects through. Every call path comes from the sibling suites
//! ([`super::domain_lifecycle`], [`super::data_source_registry`],
//! [`super::domain_rest_data_plane`]), so this suite holds no second copy of one.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_s3_amazons3::state as s3;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Answer, Emulator, REGION, SETTLE_DEADLINE};
use super::domain_rest_data_plane::{error_type, rest_prefixed};

/// How long a case that waits for a snapshot's copy to fall due keeps looking before it calls
/// the wait a failure.
///
/// The span itself is the service's and no case here is testing how long it takes: the wait is
/// for the CONDITION, so the deadline is the sibling suites' own generous one.
const SNAPSHOT_DEADLINE: std::time::Duration = SETTLE_DEADLINE;

/// How long a case waits between two looks at a snapshot it is waiting for.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The role a repository names as the one the domain copies with: stored and reported back as
/// the client wrote it, like every other repository setting.
fn snapshot_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/opensearch-snapshot",
        ::engine::emulator_account_id()
    )
}

/// Record the bucket a repository keeps its blobs in, as `s3:CreateBucket` records one.
pub(crate) fn given_bucket(emulator: &Emulator, bucket: &str) {
    let store = s3::service_state(&**emulator.engine()).expect("the S3 store");
    let created = store.lock().create_bucket(bucket, REGION);
    assert!(
        matches!(created, s3::BucketCreation::Created),
        "case \"the repository's bucket\": expected S3 to create {bucket}, got {created:?}"
    );
}

/// Every key the bucket holds, in the order `s3:ListObjectsV2` lists them.
pub(crate) fn keys(emulator: &Emulator, bucket: &str) -> Vec<String> {
    let store = s3::service_state(&**emulator.engine()).expect("the S3 store");
    let guard = store.lock();
    let held = guard
        .bucket(bucket)
        .unwrap_or_else(|| panic!("expected S3 to hold bucket {bucket}"));
    held.objects().map(|(key, _)| key.to_string()).collect()
}

/// The bytes the bucket holds at `key`, or `None` when it holds nothing there.
pub(crate) fn body_at(emulator: &Emulator, bucket: &str, key: &str) -> Option<Vec<u8>> {
    let store = s3::service_state(&**emulator.engine()).expect("the S3 store");
    let guard = store.lock();
    Some(guard.bucket(bucket)?.object(key)?.body.clone())
}

/// The `s3` repository definition a case registers: the bucket, the base path it declares (a
/// definition that declares none omits the setting entirely) and the role the domain copies
/// with.
pub(crate) fn s3_definition(bucket: &str, base_path: Option<&str>) -> Value {
    let mut settings = json!({ "bucket": bucket, "role_arn": snapshot_role_arn() });
    if let Some(base_path) = base_path {
        settings["base_path"] = json!(base_path);
    }
    json!({ "type": "s3", "settings": settings })
}

/// `PUT /_snapshot/{repository}` with `definition`.
async fn register(emulator: &Emulator, domain: &str, repository: &str, definition: Value) -> Answer {
    rest_prefixed(
        emulator,
        domain,
        "PUT",
        &format!("/_snapshot/{repository}"),
        Some(definition),
    )
    .await
}

/// `PUT /_snapshot/{repository}/{snapshot}`, waiting for the copy or not.
async fn take(
    emulator: &Emulator,
    domain: &str,
    repository: &str,
    snapshot: &str,
    body: Option<Value>,
    waited: bool,
) -> Answer {
    let path = if waited {
        format!("/_snapshot/{repository}/{snapshot}?wait_for_completion=true")
    } else {
        format!("/_snapshot/{repository}/{snapshot}")
    };
    rest_prefixed(emulator, domain, "PUT", &path, body).await
}

/// `GET /_snapshot/{repository}/{named}`.
async fn read_snapshots(
    emulator: &Emulator,
    domain: &str,
    repository: &str,
    named: &str,
) -> Answer {
    rest_prefixed(
        emulator,
        domain,
        "GET",
        &format!("/_snapshot/{repository}/{named}"),
        None,
    )
    .await
}

/// An index of `domain` holding `documents`, written through the data plane the way a client
/// loads them.
async fn given_index(
    emulator: &Emulator,
    domain: &str,
    index: &str,
    documents: &[(&str, Value)],
) {
    let created = rest_prefixed(
        emulator,
        domain,
        "PUT",
        &format!("/{index}"),
        Some(json!({
            "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "properties": { "title": { "type": "text" } } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"PUT /{index}\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    for (id, source) in documents {
        let written = rest_prefixed(
            emulator,
            domain,
            "PUT",
            &format!("/{index}/_doc/{id}"),
            Some(source.clone()),
        )
        .await;
        assert!(
            written.status == 200 || written.status == 201,
            "case \"PUT /{index}/_doc/{id}\": expected the write to be accepted, got {} with \
             {:?}",
            written.status,
            written.body
        );
    }
}

/// One row of the base-path table: what the repository declares, the key prefix its blobs
/// must live under, and the indexes the snapshot copies.
struct Row {
    case: &'static str,
    base_path: Option<&'static str>,
    prefix: &'static str,
    indexes: Vec<(&'static str, Vec<(&'static str, Value)>)>,
}

/// The rows: the forms a declared base path takes, and what one snapshot copies.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a nested base path, one index of one document",
            base_path: Some("backups/nightly"),
            prefix: "backups/nightly/",
            indexes: vec![(
                "orders",
                vec![("1", json!({ "title": "first order", "total": 10 }))],
            )],
        },
        Row {
            case: "a single-segment base path, one index of three documents",
            base_path: Some("nightly"),
            prefix: "nightly/",
            indexes: vec![(
                "orders",
                vec![
                    ("1", json!({ "title": "first order" })),
                    ("2", json!({ "title": "second order" })),
                    ("3", json!({ "title": "third order", "total": 3 })),
                ],
            )],
        },
        Row {
            case: "no base path at all, two indexes",
            base_path: None,
            prefix: "",
            indexes: vec![
                ("orders", vec![("1", json!({ "title": "an order" }))]),
                ("customers", vec![("c1", json!({ "title": "a customer" }))]),
            ],
        },
        Row {
            case: "a base path written with leading and trailing separators",
            base_path: Some("/wrapped/path/"),
            prefix: "wrapped/path/",
            indexes: vec![(
                "orders",
                vec![("1", json!({ "title": "an order" }))],
            )],
        },
    ]
}

/// RULE: a snapshot writes the repository's blobs into the bucket its settings name, every one
/// of them under the declared base path, and deleting the snapshot removes exactly the blobs
/// it wrote.
#[tokio::test]
async fn a_snapshot_writes_its_blobs_under_the_declared_base_path_and_a_delete_removes_them() {
    let emulator = Emulator::new();
    for (row_number, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let domain = format!("snapshots-{row_number}");
        let bucket = format!("snapshot-bucket-{row_number}");
        let repository = "backup-repo";
        let snapshot = "nightly-1";
        given_bucket(&emulator, &bucket);
        created_domain(&emulator, &domain).await;
        for (index, documents) in &row.indexes {
            given_index(&emulator, &domain, index, documents).await;
        }

        // The registration is accepted and reported back as it was written.
        let registered = register(
            &emulator,
            &domain,
            repository,
            s3_definition(&bucket, row.base_path),
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected the registration to answer 200, got {} with {:?}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.body["acknowledged"].as_bool(),
            Some(true),
            "case {case:?}: expected the registration to be acknowledged, got {:?}",
            registered.body
        );
        let reported = rest_prefixed(
            &emulator,
            &domain,
            "GET",
            &format!("/_snapshot/{repository}"),
            None,
        )
        .await;
        assert_eq!(
            reported.body[repository]["settings"]["bucket"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected the repository read back to name the bucket, got {:?}",
            reported.body
        );
        assert_eq!(
            reported.body[repository]["settings"]["role_arn"].as_str(),
            Some(snapshot_role_arn().as_str()),
            "case {case:?}: expected the repository read back to name the role, got {:?}",
            reported.body
        );
        // A registration writes no snapshot of its own.
        assert!(
            keys(&emulator, &bucket).is_empty(),
            "case {case:?}: expected the registration to write no object, got {:?}",
            keys(&emulator, &bucket)
        );

        // The snapshot: taken waiting for the copy, so it is complete when the call answers.
        let taken = take(&emulator, &domain, repository, snapshot, None, true).await;
        assert_eq!(
            taken.status, 200,
            "case {case:?}: expected the snapshot to answer 200, got {} with {:?}",
            taken.status, taken.body
        );
        assert_eq!(
            taken.body["snapshot"]["state"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: expected a completed snapshot, got {:?}",
            taken.body
        );
        let copied: Vec<String> = taken.body["snapshot"]["indices"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|index| index.as_str().map(str::to_string))
            .collect();
        let mut expected_indexes: Vec<String> = row
            .indexes
            .iter()
            .map(|(index, _)| (*index).to_string())
            .collect();
        expected_indexes.sort();
        let mut answered = copied.clone();
        answered.sort();
        assert_eq!(
            answered, expected_indexes,
            "case {case:?}: expected the snapshot to copy every index, got {:?}",
            taken.body
        );
        assert_eq!(
            taken.body["snapshot"]["shards"]["total"].as_i64(),
            Some(row.indexes.len() as i64),
            "case {case:?}: expected one shard copied per index, got {:?}",
            taken.body
        );
        let uuid = taken.body["snapshot"]["uuid"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !uuid.is_empty(),
            "case {case:?}: expected the snapshot to report the uuid its blobs are named by, \
             got {:?}",
            taken.body
        );

        // EVERY object the repository wrote is under the declared base path, and the set is
        // the documented layout: the repository data and its generation marker, the
        // snapshot's own blob and its global state, and per copied index the index metadata,
        // its shard's snapshot and that shard's data.
        let written = keys(&emulator, &bucket);
        for key in &written {
            assert!(
                key.starts_with(row.prefix),
                "case {case:?}: expected every object under {:?}, got {key:?}",
                row.prefix
            );
        }
        let prefix = row.prefix;
        let mut expected = vec![
            format!("{prefix}index-1"),
            format!("{prefix}index.latest"),
            format!("{prefix}snap-{uuid}.dat"),
            format!("{prefix}meta-{uuid}.dat"),
        ];
        for (index, documents) in &row.indexes {
            let uuids: Vec<String> = written
                .iter()
                .filter(|key| key.starts_with(&format!("{prefix}indices/")))
                .filter_map(|key| key.split('/').nth(prefix.split('/').count()).map(str::to_string))
                .collect();
            assert!(
                !uuids.is_empty(),
                "case {case:?}: expected the snapshot to write a blob per index, got {written:?}"
            );
            // The index's own blobs are addressed by the uuid the repository holds it under,
            // which is the one the data blob of that index carries its documents at.
            let found = uuids
                .iter()
                .find(|candidate| {
                    body_at(
                        &emulator,
                        &bucket,
                        &format!("{prefix}indices/{candidate}/meta-{uuid}.dat"),
                    )
                    .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
                    .is_some_and(|metadata| metadata["name"].as_str() == Some(index))
                })
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no index metadata blob names {index:?} in {written:?}")
                });
            expected.push(format!("{prefix}indices/{found}/meta-{uuid}.dat"));
            expected.push(format!("{prefix}indices/{found}/0/snap-{uuid}.dat"));
            expected.push(format!("{prefix}indices/{found}/0/__{uuid}"));

            // The shard's data blob holds the documents the index held — the content the
            // snapshot copied, not a placeholder.
            let data = body_at(
                &emulator,
                &bucket,
                &format!("{prefix}indices/{found}/0/__{uuid}"),
            )
            .unwrap_or_default();
            let held: BTreeMap<String, Value> = String::from_utf8_lossy(&data)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| {
                    let document: Value = serde_json::from_str(line).ok()?;
                    Some((
                        document["_id"].as_str()?.to_string(),
                        document["_source"].clone(),
                    ))
                })
                .collect();
            let want: BTreeMap<String, Value> = documents
                .iter()
                .map(|(id, source)| ((*id).to_string(), source.clone()))
                .collect();
            assert_eq!(
                held, want,
                "case {case:?}: expected the shard data blob of {index:?} to hold the \
                 documents the index held, got {held:?}"
            );
        }
        expected.sort();
        let mut found = written.clone();
        found.sort();
        assert_eq!(
            found, expected,
            "case {case:?}: expected exactly the repository's own layout in the bucket"
        );

        // The snapshot reads back through the API out of the same records.
        let read = read_snapshots(&emulator, &domain, repository, snapshot).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the snapshot read to answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["snapshots"][0]["uuid"].as_str(),
            Some(uuid.as_str()),
            "case {case:?}: expected the read to report the snapshot that was taken, got {:?}",
            read.body
        );

        // The DELETE removes exactly the blobs that snapshot wrote, and leaves the
        // repository's own data behind at its next generation.
        let deleted = rest_prefixed(
            &emulator,
            &domain,
            "DELETE",
            &format!("/_snapshot/{repository}/{snapshot}"),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body["acknowledged"].as_bool(),
            Some(true),
            "case {case:?}: expected the delete to be acknowledged, got {:?}",
            deleted.body
        );
        let left = keys(&emulator, &bucket);
        assert!(
            !left.iter().any(|key| key.contains(&uuid)),
            "case {case:?}: expected no blob of the deleted snapshot to be left, got {left:?}"
        );
        let mut remaining = left.clone();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![format!("{prefix}index-2"), format!("{prefix}index.latest")],
            "case {case:?}: expected the repository data of the next generation to be what is \
             left"
        );
        let gone = read_snapshots(&emulator, &domain, repository, snapshot).await;
        assert_eq!(
            (gone.status, error_type(&gone).as_str()),
            (404, "snapshot_missing_exception"),
            "case {case:?}: expected the deleted snapshot to be missing, got {} with {:?}",
            gone.status,
            gone.body
        );
    }
}

/// RULE: the blobs are the repository's own data — a repository registered again at a location
/// the bucket already holds blobs at reports the snapshots those blobs state.
#[tokio::test]
async fn a_repository_registered_again_reports_the_snapshots_its_bucket_holds() {
    let emulator = Emulator::new();
    let bucket = "shared-repository-bucket";
    given_bucket(&emulator, bucket);
    created_domain(&emulator, "first-domain").await;
    created_domain(&emulator, "second-domain").await;
    given_index(
        &emulator,
        "first-domain",
        "orders",
        &[("1", json!({ "title": "an order" }))],
    )
    .await;
    let definition = s3_definition(bucket, Some("shared"));
    let registered = register(&emulator, "first-domain", "repo", definition.clone()).await;
    assert_eq!(
        registered.status, 200,
        "case \"the first registration\": expected 200, got {} with {:?}",
        registered.status, registered.body
    );
    let taken = take(&emulator, "first-domain", "repo", "taken-once", None, true).await;
    assert_eq!(
        taken.status, 200,
        "case \"the snapshot\": expected 200, got {} with {:?}",
        taken.status, taken.body
    );
    let uuid = taken.body["snapshot"]["uuid"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // ANOTHER cluster registering the same location reads what that location holds.
    let again = register(&emulator, "second-domain", "same-repo", definition).await;
    assert_eq!(
        again.status, 200,
        "case \"the second registration\": expected 200, got {} with {:?}",
        again.status, again.body
    );
    let read = read_snapshots(&emulator, "second-domain", "same-repo", "_all").await;
    assert_eq!(
        read.body["snapshots"][0]["snapshot"].as_str(),
        Some("taken-once"),
        "case \"the second registration\": expected the snapshot the bucket holds, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["snapshots"][0]["uuid"].as_str(),
        Some(uuid.as_str()),
        "case \"the second registration\": expected the uuid the blobs state, got {:?}",
        read.body
    );
}

/// RULE: a restore reads an index back out of the repository's blobs — its settings, its
/// mappings and every document — and refuses a target the cluster already holds.
#[tokio::test]
async fn a_restore_reads_each_index_back_out_of_the_blobs_the_snapshot_wrote() {
    let emulator = Emulator::new();
    let bucket = "restore-bucket";
    given_bucket(&emulator, bucket);
    created_domain(&emulator, "restored").await;
    let documents = [
        ("1", json!({ "title": "first" })),
        ("2", json!({ "title": "second" })),
    ];
    given_index(&emulator, "restored", "orders", &documents).await;
    let registered = register(
        &emulator,
        "restored",
        "repo",
        s3_definition(bucket, Some("restores")),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case \"the registration\": expected 200, got {} with {:?}",
        registered.status, registered.body
    );
    let taken = take(&emulator, "restored", "repo", "before-drop", None, true).await;
    assert_eq!(
        taken.status, 200,
        "case \"the snapshot\": expected 200, got {} with {:?}",
        taken.status, taken.body
    );

    // A restore onto an index the cluster HOLDS is refused rather than replacing it.
    let occupied = rest_prefixed(
        &emulator,
        "restored",
        "POST",
        "/_snapshot/repo/before-drop/_restore",
        None,
    )
    .await;
    assert_eq!(
        (occupied.status, error_type(&occupied).as_str()),
        (500, "snapshot_restore_exception"),
        "case \"a restore onto an index the cluster holds\": expected the restore to be \
         refused, got {} with {:?}",
        occupied.status,
        occupied.body
    );

    // Renaming is the way to restore beside the index that is there.
    let renamed = rest_prefixed(
        &emulator,
        "restored",
        "POST",
        "/_snapshot/repo/before-drop/_restore",
        Some(json!({
            "indices": "orders",
            "rename_pattern": "(.+)",
            "rename_replacement": "copy-of-$1",
        })),
    )
    .await;
    assert_eq!(
        renamed.status, 200,
        "case \"a renamed restore\": expected 200, got {} with {:?}",
        renamed.status, renamed.body
    );
    assert_eq!(
        renamed.body["snapshot"]["indices"][0].as_str(),
        Some("copy-of-orders"),
        "case \"a renamed restore\": expected the renamed target, got {:?}",
        renamed.body
    );
    for (id, source) in &documents {
        let read = rest_prefixed(
            &emulator,
            "restored",
            "GET",
            &format!("/copy-of-orders/_doc/{id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case \"a renamed restore\": expected the restored document {id} to be readable, \
             got {} with {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            &read.body["_source"], source,
            "case \"a renamed restore\": expected the document the snapshot copied, got {:?}",
            read.body
        );
    }

    // And after the index is DROPPED, the snapshot is what brings it back — with the
    // mappings it carried.
    let dropped = rest_prefixed(&emulator, "restored", "DELETE", "/orders", None).await;
    assert_eq!(
        dropped.status, 200,
        "case \"the index dropped\": expected 200, got {} with {:?}",
        dropped.status, dropped.body
    );
    let restored = rest_prefixed(
        &emulator,
        "restored",
        "POST",
        "/_snapshot/repo/before-drop/_restore",
        Some(json!({ "indices": "orders" })),
    )
    .await;
    assert_eq!(
        restored.status, 200,
        "case \"the restore\": expected 200, got {} with {:?}",
        restored.status, restored.body
    );
    let read_back = rest_prefixed(&emulator, "restored", "GET", "/orders", None).await;
    assert_eq!(
        read_back.body["orders"]["mappings"]["properties"]["title"]["type"].as_str(),
        Some("text"),
        "case \"the restore\": expected the mappings the snapshot copied, got {:?}",
        read_back.body
    );
    let counted = rest_prefixed(&emulator, "restored", "GET", "/orders/_count", None).await;
    assert_eq!(
        counted.body["count"].as_i64(),
        Some(documents.len() as i64),
        "case \"the restore\": expected every copied document back, got {:?}",
        counted.body
    );
}

/// RULE: a snapshot a client did not ask to wait for is accepted straight away and reports
/// `SUCCESS` once the copy the clock owes falls due.
#[tokio::test]
async fn a_snapshot_taken_without_waiting_is_accepted_and_completes_on_its_own() {
    let emulator = Emulator::new();
    let bucket = "accepted-bucket";
    given_bucket(&emulator, bucket);
    created_domain(&emulator, "accepting").await;
    given_index(
        &emulator,
        "accepting",
        "orders",
        &[("1", json!({ "title": "an order" }))],
    )
    .await;
    let registered = register(
        &emulator,
        "accepting",
        "repo",
        s3_definition(bucket, Some("async")),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case \"the registration\": expected 200, got {} with {:?}",
        registered.status, registered.body
    );
    let taken = take(&emulator, "accepting", "repo", "unwaited", None, false).await;
    assert_eq!(
        taken.status, 200,
        "case \"a snapshot taken without waiting\": expected 200, got {} with {:?}",
        taken.status, taken.body
    );
    assert_eq!(
        taken.body["accepted"].as_bool(),
        Some(true),
        "case \"a snapshot taken without waiting\": expected it to be accepted, got {:?}",
        taken.body
    );
    // The copy itself was made — the blobs are there straight away — and the STATE the
    // snapshot reports is what the clock owes. Polled for the condition rather than timed.
    let deadline = std::time::Instant::now() + SNAPSHOT_DEADLINE;
    loop {
        let read = read_snapshots(&emulator, "accepting", "repo", "unwaited").await;
        let state = read.body["snapshots"][0]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == "SUCCESS" {
            assert!(
                read.body["snapshots"][0]["end_time_in_millis"]
                    .as_i64()
                    .unwrap_or_default()
                    > 0,
                "case \"a snapshot taken without waiting\": expected a completed snapshot to \
                 report when it ended, got {:?}",
                read.body
            );
            break;
        }
        assert_eq!(
            state, "IN_PROGRESS",
            "case \"a snapshot taken without waiting\": expected the snapshot to be in \
             progress until its copy falls due, got {:?}",
            read.body
        );
        assert!(
            std::time::Instant::now() < deadline,
            "case \"a snapshot taken without waiting\": the snapshot never completed, last \
             read {:?}",
            read.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One refusal row: the request, and the status and error type the cluster answers it with.
struct Refusal {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    status: u16,
    error_type: &'static str,
    /// A fragment the reason must carry, so the row pins what the message SAYS and not only
    /// its class.
    reason: &'static str,
}

/// RULE: the snapshot API refuses what the cluster refuses, in the class the engine states it.
///
/// The rows span the refusals: a repository nothing registered (read, snapshot, delete), a
/// definition with no type, a type the cluster has no plugin for, settings naming no bucket, a
/// bucket the object store does not hold, the snapshot names the engine will not take (one row
/// per condition a name can be sent literally in a URI for — whitespace and `#` cannot be, and
/// what a percent-escape in a path stands for is another suite's rule), a name already taken, a
/// snapshot the repository does not hold, and a method the route does not take.
#[tokio::test]
async fn the_snapshot_api_refuses_what_the_cluster_refuses() {
    let emulator = Emulator::new();
    let bucket = "refusal-bucket";
    given_bucket(&emulator, bucket);
    created_domain(&emulator, "refusing").await;
    given_index(
        &emulator,
        "refusing",
        "orders",
        &[("1", json!({ "title": "an order" }))],
    )
    .await;
    let registered = register(
        &emulator,
        "refusing",
        "repo",
        s3_definition(bucket, Some("refusals")),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case \"the registration\": expected 200, got {} with {:?}",
        registered.status, registered.body
    );
    let taken = take(&emulator, "refusing", "repo", "taken", None, true).await;
    assert_eq!(
        taken.status, 200,
        "case \"the snapshot\": expected 200, got {} with {:?}",
        taken.status, taken.body
    );

    let rows = vec![
        Refusal {
            case: "a read of a repository nothing registered",
            method: "GET",
            path: "/_snapshot/absent",
            body: None,
            status: 404,
            error_type: "repository_missing_exception",
            reason: "[absent] missing",
        },
        Refusal {
            case: "a snapshot into a repository nothing registered",
            method: "PUT",
            path: "/_snapshot/absent/snap",
            body: None,
            status: 404,
            error_type: "repository_missing_exception",
            reason: "[absent] missing",
        },
        Refusal {
            case: "a delete in a repository nothing registered",
            method: "DELETE",
            path: "/_snapshot/absent/snap",
            body: None,
            status: 404,
            error_type: "repository_missing_exception",
            reason: "[absent] missing",
        },
        Refusal {
            case: "a registration naming no type",
            method: "PUT",
            path: "/_snapshot/typeless",
            body: Some(json!({ "settings": { "bucket": "refusal-bucket" } })),
            status: 400,
            error_type: "action_request_validation_exception",
            reason: "Validation Failed: 1: type is missing;",
        },
        Refusal {
            case: "a registration naming a type the cluster has no plugin for",
            method: "PUT",
            path: "/_snapshot/filed",
            body: Some(json!({ "type": "fs", "settings": { "location": "/mnt/backups" } })),
            status: 500,
            error_type: "repository_exception",
            reason: "[filed] repository type [fs] does not exist",
        },
        Refusal {
            case: "an s3 registration whose settings name no bucket",
            method: "PUT",
            path: "/_snapshot/bucketless",
            body: Some(json!({ "type": "s3", "settings": { "base_path": "backups" } })),
            status: 500,
            error_type: "repository_exception",
            reason: "[bucketless] No bucket defined for s3 repository",
        },
        Refusal {
            case: "an s3 registration naming a bucket the object store does not hold",
            method: "PUT",
            path: "/_snapshot/unreachable",
            body: Some(json!({
                "type": "s3",
                "settings": { "bucket": "no-such-bucket", "base_path": "backups" },
            })),
            status: 500,
            error_type: "repository_verification_exception",
            reason: "[unreachable] path [backups] is not accessible on master node",
        },
        Refusal {
            case: "a snapshot name that is already taken",
            method: "PUT",
            path: "/_snapshot/repo/taken",
            body: None,
            status: 400,
            error_type: "invalid_snapshot_name_exception",
            reason: "snapshot with the same name already exists",
        },
        Refusal {
            case: "a snapshot name carrying a comma",
            method: "PUT",
            path: "/_snapshot/repo/with,comma",
            body: None,
            status: 400,
            error_type: "invalid_snapshot_name_exception",
            reason: "must not contain ','",
        },
        Refusal {
            case: "a snapshot name starting with the reserved character",
            method: "PUT",
            path: "/_snapshot/repo/_reserved",
            body: None,
            status: 400,
            error_type: "invalid_snapshot_name_exception",
            reason: "must not start with '_'",
        },
        Refusal {
            case: "a snapshot name that is not lowercase",
            method: "PUT",
            path: "/_snapshot/repo/Nightly",
            body: None,
            status: 400,
            error_type: "invalid_snapshot_name_exception",
            reason: "must be lowercase",
        },
        Refusal {
            case: "a snapshot name carrying a character no file name may",
            method: "PUT",
            path: "/_snapshot/repo/night*ly",
            body: None,
            status: 400,
            error_type: "invalid_snapshot_name_exception",
            reason: "must not contain the following characters",
        },
        Refusal {
            case: "a read of a snapshot the repository does not hold",
            method: "GET",
            path: "/_snapshot/repo/never-taken",
            body: None,
            status: 404,
            error_type: "snapshot_missing_exception",
            reason: "[repo:never-taken] is missing",
        },
        Refusal {
            case: "a delete of a snapshot the repository does not hold",
            method: "DELETE",
            path: "/_snapshot/repo/never-taken",
            body: None,
            status: 404,
            error_type: "snapshot_missing_exception",
            reason: "[repo:never-taken] is missing",
        },
        Refusal {
            case: "a restore of a snapshot the repository does not hold",
            method: "POST",
            path: "/_snapshot/repo/never-taken/_restore",
            body: None,
            status: 500,
            error_type: "snapshot_restore_exception",
            reason: "[repo:never-taken] snapshot does not exist",
        },
    ];
    for row in rows {
        let answer = rest_prefixed(&emulator, "refusing", row.method, row.path, row.body).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case, row.status, answer.status, answer.body
        );
        assert_eq!(
            error_type(&answer),
            row.error_type,
            "case {:?}: expected the error type the engine states, got {:?}",
            row.case,
            answer.body
        );
        let reason = answer.body["error"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.contains(row.reason),
            "case {:?}: expected the reason to carry {:?}, got {reason:?}",
            row.case,
            row.reason
        );
    }

    // A method no snapshot route takes is the method refusal, not a missing resource.
    let patched = rest_prefixed(&emulator, "refusing", "PATCH", "/_snapshot/repo", None).await;
    assert_eq!(
        patched.status, 405,
        "case \"a method the repository route does not take\": expected 405, got {} with {:?}",
        patched.status, patched.body
    );
}
