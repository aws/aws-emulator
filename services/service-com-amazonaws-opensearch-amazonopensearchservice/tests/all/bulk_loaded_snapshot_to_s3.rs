// SPDX-License-Identifier: CC0-1.0

//! The whole SNAPSHOT-TO-S3 path a client walks when it backs a domain up to its own
//! bucket, driven over the real wire path and over the addressing
//! `DomainStatus$Endpoint` publishes: the documents are loaded through `POST /_bulk` on
//! the published endpoint host, an `s3` repository naming a bucket and a `base_path` is
//! registered, the snapshot is taken, and the blobs it wrote in that bucket are read back.
//!
//! The rules, each over a SET of inputs rather than one recorded call:
//!
//! * a RESERVED first path segment — one beginning with `_` — names an API of the engine
//!   and never an index, so the bulk API is served on the domain endpoint under every
//!   spelling of its path: the cluster-wide `/_bulk`, whose actions name their own index,
//!   and the index-scoped `/{index}/_bulk`, whose path names the index an action that
//!   names none lands in;
//! * what a snapshot copies is what the cluster HOLDS at the instant it is taken, so the
//!   documents a bulk stream settled on — whichever actions it used to get there — are the
//!   documents the shard data blob under the declared base path carries, and deleting the
//!   snapshot removes the blobs it wrote while leaving the documents in the cluster.
//!
//! The rows span how the stream names its index (per action, or from the path), which
//! actions it carries (`index`, `create`, `delete`), how many indexes one stream loads, and
//! the forms a declared base path takes.
//!
//! The bucket is the one Amazon S3 itself holds in the SAME shared engine, read through the
//! seam `s3:ListObjectsV2` lists a bucket's objects through. Every call path comes from the
//! sibling suites ([`super::domain_lifecycle`], [`super::data_source_registry`],
//! [`super::domain_rest_data_plane`], [`super::domain_rest_bulk_and_index_apis`],
//! [`super::snapshot_repositories`]), so this suite holds no second copy of one.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Answer, Emulator, DEFAULT_ACCESS_KEY_ID};
use super::domain_rest_bulk_and_index_apis::{bulk, created_index};
use super::domain_rest_data_plane::{endpoint_of, error_type, rest};
use super::snapshot_repositories::{body_at, given_bucket, keys, s3_definition};

/// The schema every index of a case is created with: one shard and no replica copy, so the
/// repository's layout has exactly one shard directory per index to assert over.
fn index_schema() -> Value {
    json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
        "mappings": { "properties": { "title": { "type": "text" } } },
    })
}

/// One row: the bulk stream, where it is addressed, the base path the repository declares,
/// the key prefix its blobs must then live under, and the documents the cluster is left
/// holding per index.
struct Row {
    case: &'static str,
    /// The bulk path, relative to the published endpoint host.
    path: &'static str,
    /// The indexes the case creates before it loads, in the order it creates them.
    indexes: Vec<&'static str>,
    /// The NDJSON lines the stream carries, action lines and source lines together.
    lines: Vec<Value>,
    /// How many ACTIONS those lines name — the items the answer must carry.
    actions: usize,
    base_path: Option<&'static str>,
    prefix: &'static str,
    /// What each index holds once the stream has been applied.
    held: Vec<(&'static str, Vec<(&'static str, Value)>)>,
}

/// The rows, spanning the bulk spellings and the base-path forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a cluster-wide stream whose actions name their own index, a nested base \
                   path",
            path: "/_bulk",
            indexes: vec!["orders"],
            lines: vec![
                json!({ "index": { "_index": "orders", "_id": "1" } }),
                json!({ "title": "first order", "total": 10 }),
                json!({ "index": { "_index": "orders", "_id": "2" } }),
                json!({ "title": "second order", "total": 20 }),
            ],
            actions: 2,
            base_path: Some("backups/nightly"),
            prefix: "backups/nightly/",
            held: vec![(
                "orders",
                vec![
                    ("1", json!({ "title": "first order", "total": 10 })),
                    ("2", json!({ "title": "second order", "total": 20 })),
                ],
            )],
        },
        Row {
            case: "an index-scoped stream taking its index from the path, a single-segment \
                   base path",
            path: "/orders/_bulk",
            indexes: vec!["orders"],
            lines: vec![
                json!({ "index": { "_id": "1" } }),
                json!({ "title": "an indexed order" }),
                json!({ "create": { "_id": "2" } }),
                json!({ "title": "a created order" }),
            ],
            actions: 2,
            base_path: Some("nightly"),
            prefix: "nightly/",
            held: vec![(
                "orders",
                vec![
                    ("1", json!({ "title": "an indexed order" })),
                    ("2", json!({ "title": "a created order" })),
                ],
            )],
        },
        Row {
            case: "one stream loading two indexes, no base path at all",
            path: "/_bulk",
            indexes: vec!["orders", "customers"],
            lines: vec![
                json!({ "index": { "_index": "orders", "_id": "1" } }),
                json!({ "title": "an order" }),
                json!({ "index": { "_index": "customers", "_id": "c1" } }),
                json!({ "title": "a customer" }),
            ],
            actions: 2,
            base_path: None,
            prefix: "",
            held: vec![
                ("orders", vec![("1", json!({ "title": "an order" }))]),
                ("customers", vec![("c1", json!({ "title": "a customer" }))]),
            ],
        },
        Row {
            case: "a stream whose delete removes a document it indexed, a base path written \
                   with leading and trailing separators",
            path: "/_bulk",
            indexes: vec!["orders"],
            lines: vec![
                json!({ "index": { "_index": "orders", "_id": "1" } }),
                json!({ "title": "a withdrawn order" }),
                json!({ "index": { "_index": "orders", "_id": "2" } }),
                json!({ "title": "a standing order" }),
                json!({ "delete": { "_index": "orders", "_id": "1" } }),
            ],
            actions: 3,
            base_path: Some("/wrapped/path/"),
            prefix: "wrapped/path/",
            held: vec![(
                "orders",
                vec![("2", json!({ "title": "a standing order" }))],
            )],
        },
    ]
}

/// The documents the shard data blob of `index` carries, read out of the bucket: the blob
/// is found by the index metadata that NAMES the index, since the directory it lives under
/// is the uuid the repository holds that index by.
fn snapshotted(
    emulator: &Emulator,
    bucket: &str,
    written: &[String],
    prefix: &str,
    snapshot_uuid: &str,
    index: &str,
) -> Option<BTreeMap<String, Value>> {
    let under = format!("{prefix}indices/");
    let depth = prefix.split('/').count();
    let held = written
        .iter()
        .filter(|key| key.starts_with(&under))
        .filter_map(|key| key.split('/').nth(depth).map(str::to_string))
        .find(|candidate| {
            body_at(
                emulator,
                bucket,
                &format!("{under}{candidate}/meta-{snapshot_uuid}.dat"),
            )
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .is_some_and(|metadata| metadata["name"].as_str() == Some(index))
        })?;
    let data = body_at(
        emulator,
        bucket,
        &format!("{under}{held}/0/__{snapshot_uuid}"),
    )?;
    Some(
        String::from_utf8_lossy(&data)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let document: Value = serde_json::from_str(line).ok()?;
                Some((
                    document["_id"].as_str()?.to_string(),
                    document["_source"].clone(),
                ))
            })
            .collect(),
    )
}

/// One request to the published endpoint host under the default credential.
async fn on_host(
    emulator: &Emulator,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    rest(emulator, DEFAULT_ACCESS_KEY_ID, host, method, path, body).await
}

/// RULE: the bulk API is served on the published domain endpoint under every spelling of
/// its reserved path, and the documents a stream settles on are the documents a snapshot
/// writes under the repository's declared base path — which a delete then removes.
#[tokio::test]
async fn documents_loaded_in_bulk_are_the_documents_the_snapshot_writes_under_its_base_path() {
    let emulator = Emulator::new();
    for (row_number, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let domain = format!("bulk-snapshots-{row_number}");
        let bucket = format!("bulk-snapshot-bucket-{row_number}");
        let repository = "backup-repo";
        let snapshot = "nightly-1";
        given_bucket(&emulator, &bucket);
        created_domain(&emulator, &domain).await;
        let host = endpoint_of(&emulator, &domain);
        for index in &row.indexes {
            created_index(&emulator, &host, index, index_schema()).await;
        }

        // The LOAD: `POST /_bulk` on the published endpoint is answered by the bulk API,
        // one item per action, and not as a request about an index named `_bulk`.
        let loaded = bulk(&emulator, &host, row.path, &row.lines).await;
        assert_eq!(
            loaded.status, 200,
            "case {case:?}: expected \"POST {}\" to answer 200, got {} with {:?}",
            row.path, loaded.status, loaded.body
        );
        assert_eq!(
            loaded.body["errors"].as_bool(),
            Some(false),
            "case {case:?}: expected every item to have been applied, got {:?}",
            loaded.body
        );
        assert_eq!(
            loaded.body["items"].as_array().map(Vec::len),
            Some(row.actions),
            "case {case:?}: expected one item per action, got {:?}",
            loaded.body
        );
        assert!(
            loaded.body["took"].is_u64(),
            "case {case:?}: expected the answer to report how long the load took, got {:?}",
            loaded.body
        );

        // Every loaded document is backed by what the index now holds, read back through
        // the surface a client reads it through.
        for (index, documents) in &row.held {
            let counted = on_host(
                &emulator,
                &host,
                "GET",
                &format!("/{index}/_count"),
                None,
            )
            .await;
            assert_eq!(
                counted.body["count"].as_i64(),
                Some(documents.len() as i64),
                "case {case:?}: expected {index:?} to hold {} documents, got {:?}",
                documents.len(),
                counted.body
            );
        }

        // The REPOSITORY, and the snapshot taken into it waiting for the copy.
        let registered = on_host(
            &emulator,
            &host,
            "PUT",
            &format!("/_snapshot/{repository}"),
            Some(s3_definition(&bucket, row.base_path)),
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected the registration to answer 200, got {} with {:?}",
            registered.status, registered.body
        );
        let taken = on_host(
            &emulator,
            &host,
            "PUT",
            &format!("/_snapshot/{repository}/{snapshot}?wait_for_completion=true"),
            None,
        )
        .await;
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
        let uuid = taken.body["snapshot"]["uuid"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !uuid.is_empty(),
            "case {case:?}: expected the snapshot to report the uuid its blobs are named \
             by, got {:?}",
            taken.body
        );

        // Every object the repository wrote is under the DECLARED base path, and the shard
        // data blob of each index carries the documents the bulk stream settled on.
        let written = keys(&emulator, &bucket);
        for key in &written {
            assert!(
                key.starts_with(row.prefix),
                "case {case:?}: expected every object under {:?}, got {key:?}",
                row.prefix
            );
        }
        for (index, documents) in &row.held {
            let found = snapshotted(
                &emulator,
                &bucket,
                &written,
                row.prefix,
                &uuid,
                index,
            )
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no shard data blob of {index:?} in {written:?}"
                )
            });
            let want: BTreeMap<String, Value> = documents
                .iter()
                .map(|(id, source)| ((*id).to_string(), source.clone()))
                .collect();
            assert_eq!(
                found, want,
                "case {case:?}: expected the shard data blob of {index:?} to hold the \
                 documents the bulk stream left, got {found:?}"
            );
        }

        // The DELETE removes the blobs that snapshot wrote — and nothing of the cluster's
        // own data, which the count still reads back.
        let deleted = on_host(
            &emulator,
            &host,
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
        let left = keys(&emulator, &bucket);
        assert!(
            !left.iter().any(|key| key.contains(&uuid)),
            "case {case:?}: expected no blob of the deleted snapshot to be left, got {left:?}"
        );
        let gone = on_host(
            &emulator,
            &host,
            "GET",
            &format!("/_snapshot/{repository}/{snapshot}"),
            None,
        )
        .await;
        assert_eq!(
            (gone.status, error_type(&gone).as_str()),
            (404, "snapshot_missing_exception"),
            "case {case:?}: expected the deleted snapshot to be missing, got {} with {:?}",
            gone.status,
            gone.body
        );
        for (index, documents) in &row.held {
            let counted = on_host(
                &emulator,
                &host,
                "GET",
                &format!("/{index}/_count"),
                None,
            )
            .await;
            assert_eq!(
                counted.body["count"].as_i64(),
                Some(documents.len() as i64),
                "case {case:?}: expected {index:?} to still hold {} documents after the \
                 delete, got {:?}",
                documents.len(),
                counted.body
            );
        }
    }
}
