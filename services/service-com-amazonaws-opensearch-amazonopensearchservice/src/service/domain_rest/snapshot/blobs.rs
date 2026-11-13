// SPDX-License-Identifier: CC0-1.0

//! The BLOBS one snapshot repository keeps in its bucket: where each lives under the
//! repository's base path, what each holds, and how a repository the cluster is handed again
//! is read back out of them.
//!
//! A snapshot repository is a BLOB STORE layout, and the layout is what makes a snapshot
//! portable: the repository's own data blob lists the snapshots it holds, a blob per snapshot
//! records what that snapshot was, and a blob per index and shard carries the documents that
//! were copied. Every key here is formed under the repository's declared base path
//! ([`prefix`]), so nothing a repository writes can land outside the prefix its settings
//! named, and every body is rendered from the records the cluster holds — never from anything
//! a request carried about the blobs themselves.
//!
//! The bodies are JSON documents rather than the engine's own SMILE encoding: what matters
//! for a client of this emulator is that the layout is there, that each blob holds the
//! snapshot's real content, and that a restore reads its indexes back out of these bytes.

use ::std::collections::BTreeMap;
use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::serde_json::{json, Map, Value};

use crate::state::{RepositorySnapshot, SnapshotRepository};

/// One blob of a repository: where it goes, what it holds, and what it is.
#[derive(Debug, Clone)]
pub(super) struct Blob {
    pub(super) key: String,
    pub(super) body: Vec<u8>,
    pub(super) content_type: &'static str,
}

/// The content type the repository's own documents are kept as.
const JSON: &str = "application/json";

/// The content type of the blobs that are not one document: the generation marker and the
/// line-per-document data a shard's copy holds.
const TEXT: &str = "text/plain";

/// One index as a snapshot copies it: what the cluster held under that name at the instant
/// the snapshot was taken.
#[derive(Debug, Clone)]
pub(super) struct CopiedIndex {
    pub(super) name: String,
    /// The uuid the index was held under, which its blobs live beneath.
    pub(super) uuid: String,
    pub(super) settings: Value,
    pub(super) mappings: Value,
    /// The documents the index held, each as the id it was held under and its source.
    pub(super) documents: Vec<(String, Value)>,
}

/// The shard every index's documents are copied as: this cluster holds an index's documents
/// as one shard, so one index contributes one shard to a snapshot.
pub(super) const COPIED_SHARD: i64 = 0;

/// The prefix every blob of a repository whose `base_path` setting is `base_path` lives
/// under: the path as the settings wrote it, with the separators around it normalised, and
/// nothing at all for a repository that declared no base path.
///
/// This is the ONE place a repository's key prefix is formed, which is what makes "every
/// object stays under the declared base path" a property of the layout rather than of each
/// writer.
pub(super) fn prefix(base_path: &str) -> String {
    let trimmed = base_path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}/")
}

/// The key of the repository-data blob of generation `generation` — `index-N`, the blob that
/// lists what the repository holds.
pub(super) fn repository_data_key(prefix: &str, generation: u64) -> String {
    format!("{prefix}index-{generation}")
}

/// The key of the blob that names the current generation — `index.latest`, which is what a
/// reader consults to find the repository data.
pub(super) fn generation_key(prefix: &str) -> String {
    format!("{prefix}index.latest")
}

/// The key of one snapshot's own blob — `snap-<uuid>.dat`.
fn snapshot_key(prefix: &str, snapshot_uuid: &str) -> String {
    format!("{prefix}snap-{snapshot_uuid}.dat")
}

/// The key of the global-metadata blob of one snapshot — `meta-<uuid>.dat`.
fn global_metadata_key(prefix: &str, snapshot_uuid: &str) -> String {
    format!("{prefix}meta-{snapshot_uuid}.dat")
}

/// The key of one index's metadata blob inside one snapshot —
/// `indices/<index-uuid>/meta-<snapshot-uuid>.dat`.
pub(super) fn index_metadata_key(prefix: &str, index_uuid: &str, snapshot_uuid: &str) -> String {
    format!("{prefix}indices/{index_uuid}/meta-{snapshot_uuid}.dat")
}

/// The key of one shard's snapshot blob — `indices/<index-uuid>/<shard>/snap-<uuid>.dat`.
fn shard_snapshot_key(prefix: &str, index_uuid: &str, shard: i64, snapshot_uuid: &str) -> String {
    format!("{prefix}indices/{index_uuid}/{shard}/snap-{snapshot_uuid}.dat")
}

/// The key of the DATA blob one shard's copy wrote — `indices/<index-uuid>/<shard>/__<uuid>`,
/// the `__`-prefixed blob a repository keeps a shard's copied content in.
pub(super) fn shard_data_key(
    prefix: &str,
    index_uuid: &str,
    shard: i64,
    snapshot_uuid: &str,
) -> String {
    format!("{prefix}indices/{index_uuid}/{shard}/__{snapshot_uuid}")
}

/// Every blob ONE snapshot writes: its own document, the global state it copied, and per
/// copied index the index's metadata, its shard's snapshot and the data that shard holds.
///
/// `global` is the cluster-wide state the snapshot copied — the index templates and the
/// cluster's own persistent settings — which a snapshot that did not include the global state
/// copies as the empty sections.
pub(super) fn snapshot_blobs(
    prefix: &str,
    snapshot: &RepositorySnapshot,
    copied: &[CopiedIndex],
    global: (&Value, &Value),
) -> Vec<Blob> {
    let (templates, settings) = global;
    let mut written = vec![
        Blob {
            key: snapshot_key(prefix, &snapshot.uuid),
            // The snapshot's own document, as a reader of this repository reports it: taken
            // at the instant its copy completes, so the blob records the finished snapshot
            // rather than the moment the write happened to be made.
            body: rendered(&snapshot.info_at(snapshot.completes_at)),
            content_type: JSON,
        },
        Blob {
            key: global_metadata_key(prefix, &snapshot.uuid),
            body: rendered(&json!({ "templates": templates, "settings": settings })),
            content_type: JSON,
        },
    ];
    for index in copied {
        written.push(Blob {
            key: index_metadata_key(prefix, &index.uuid, &snapshot.uuid),
            body: rendered(&json!({
                "name": index.name,
                "uuid": index.uuid,
                "settings": index.settings,
                "mappings": index.mappings,
            })),
            content_type: JSON,
        });
        let data_key = shard_data_key(prefix, &index.uuid, COPIED_SHARD, &snapshot.uuid);
        let file = data_key.rsplit('/').next().unwrap_or(&data_key).to_string();
        written.push(Blob {
            key: shard_snapshot_key(prefix, &index.uuid, COPIED_SHARD, &snapshot.uuid),
            body: rendered(&json!({
                "index": index.name,
                "shard": COPIED_SHARD,
                "snapshot": snapshot.name,
                "documents": index.documents.len(),
                "files": [file],
            })),
            content_type: JSON,
        });
        written.push(Blob {
            key: data_key,
            body: shard_data(&index.documents),
            content_type: TEXT,
        });
    }
    written
}

/// The keys of every blob ONE snapshot wrote, which is what deleting that snapshot removes:
/// its own document, its global state, and per index it copied that index's metadata, shard
/// snapshot and shard data.
///
/// Formed from the snapshot's own record — the uuid it was taken under and the uuid of each
/// index it copied — so a delete addresses exactly the blobs the create wrote, whatever has
/// happened to the cluster's indexes since.
pub(super) fn snapshot_keys(prefix: &str, snapshot: &RepositorySnapshot) -> Vec<String> {
    let mut keys = vec![
        snapshot_key(prefix, &snapshot.uuid),
        global_metadata_key(prefix, &snapshot.uuid),
    ];
    for index in &snapshot.indices {
        let Some(uuid) = snapshot.index_uuids.get(index) else {
            continue;
        };
        keys.push(index_metadata_key(prefix, uuid, &snapshot.uuid));
        keys.push(shard_snapshot_key(
            prefix,
            uuid,
            COPIED_SHARD,
            &snapshot.uuid,
        ));
        keys.push(shard_data_key(prefix, uuid, COPIED_SHARD, &snapshot.uuid));
    }
    keys
}

/// The two blobs that state what the repository HOLDS after a change: the repository data of
/// the new generation, and the marker that names that generation as the current one.
///
/// `snapshots` is the repository's whole snapshot set as it stands after the change, so the
/// blob and the cluster's view of the repository cannot state different snapshots.
pub(super) fn repository_data_blobs(
    prefix: &str,
    snapshots: &BTreeMap<String, RepositorySnapshot>,
    generation: u64,
) -> Vec<Blob> {
    let mut indices: Map<String, Value> = Map::new();
    let stated: Vec<Value> = snapshots
        .values()
        .map(|held| {
            for index in &held.indices {
                let uuid = held.index_uuids.get(index).cloned().unwrap_or_default();
                let entry = indices
                    .entry(index.clone())
                    .or_insert_with(|| json!({ "id": uuid, "snapshots": [] }));
                if let Some(taken) = entry.get_mut("snapshots").and_then(Value::as_array_mut) {
                    taken.push(Value::String(held.uuid.clone()));
                }
            }
            json!({
                "name": held.name,
                "uuid": held.uuid,
                "index_uuids": held.index_uuids,
                "info": held.info_at(held.completes_at),
            })
        })
        .collect();
    vec![
        Blob {
            key: repository_data_key(prefix, generation),
            body: rendered(&json!({
                "generation": generation,
                "snapshots": stated,
                "indices": Value::Object(indices),
            })),
            content_type: JSON,
        },
        Blob {
            key: generation_key(prefix),
            body: generation.to_string().into_bytes(),
            content_type: TEXT,
        },
    ]
}

/// The generation `index.latest`'s body names, or `None` for a body that names none.
pub(super) fn read_generation(body: &[u8]) -> Option<u64> {
    ::std::str::from_utf8(body).ok()?.trim().parse::<u64>().ok()
}

/// The settings and mappings an index-metadata blob holds, or `None` for a body that is not
/// one.
pub(super) fn read_index_metadata(body: &[u8]) -> Option<(Value, Value)> {
    let document: Value = ::serde_json::from_slice(body).ok()?;
    Some((
        document.get("settings").cloned().unwrap_or(json!({})),
        document.get("mappings").cloned().unwrap_or(json!({})),
    ))
}

/// One shard's DATA blob: a line per copied document, carrying the id it was held under and
/// the source the cluster held — the bytes a restore reads an index back out of.
fn shard_data(documents: &[(String, Value)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, source) in documents {
        body.extend_from_slice(&rendered(&json!({ "_id": id, "_source": source })));
        body.push(b'\n');
    }
    body
}

/// The documents one shard's data blob holds, in the order it holds them — and nothing for a
/// line that is not one, since a line this emulator cannot read holds no document to restore.
pub(super) fn read_shard_data(body: &[u8]) -> Vec<(String, Value)> {
    ::std::str::from_utf8(body)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let document: Value = ::serde_json::from_str(line).ok()?;
            let id = document.get("_id")?.as_str()?.to_string();
            Some((id, document.get("_source").cloned().unwrap_or(json!({}))))
        })
        .collect()
}

/// The snapshot records a repository-data blob states, keyed by the snapshot's name: what a
/// repository registered again at a location it already holds blobs at reports as its
/// snapshots.
///
/// A snapshot entry this emulator cannot read states no snapshot, rather than one with values
/// made up for it.
pub(super) fn read_repository_data(body: &[u8]) -> BTreeMap<String, RepositorySnapshot> {
    let Ok(document) = ::serde_json::from_slice::<Value>(body) else {
        return BTreeMap::new();
    };
    let mut held = BTreeMap::new();
    let stated = document
        .get("snapshots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for entry in &stated {
        if let Some(snapshot) = read_snapshot(entry) {
            held.insert(snapshot.name.clone(), snapshot);
        }
    }
    held
}

/// One snapshot record a repository-data entry states, or `None` when the entry states no
/// name, no uuid or no document for it.
fn read_snapshot(stated: &Value) -> Option<RepositorySnapshot> {
    let name = stated.get("name")?.as_str()?.to_string();
    let uuid = stated.get("uuid")?.as_str()?.to_string();
    let info = stated.get("info")?;
    let index_uuids = stated
        .get("index_uuids")
        .and_then(Value::as_object)
        .map(|held| {
            held.iter()
                .filter_map(|(index, uuid)| Some((index.clone(), uuid.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let indices = info
        .get("indices")
        .and_then(Value::as_array)
        .map(|named| {
            named
                .iter()
                .filter_map(|index| index.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let started_at = at_millis(info.get("start_time_in_millis").and_then(Value::as_i64)?);
    // A snapshot the blob records as complete carries the instant its copy finished; one the
    // blob records with no end never completed, so it is read as completing when it started —
    // nothing is still in flight in a repository read back out of the store.
    let ended = info
        .get("end_time_in_millis")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Some(RepositorySnapshot {
        name,
        uuid,
        version: info
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        indices,
        index_uuids,
        include_global_state: info
            .get("include_global_state")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        shards: info
            .get("shards")
            .and_then(|shards| shards.get("total"))
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        started_at,
        completes_at: if ended > 0 {
            at_millis(ended)
        } else {
            started_at
        },
    })
}

/// The instant `millis` epoch milliseconds names, and the epoch itself for a value no instant
/// can be formed from.
fn at_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(u64::try_from(millis).unwrap_or(0)))
        .unwrap_or(UNIX_EPOCH)
}

/// One blob body, as the bytes the object store holds.
fn rendered(body: &Value) -> Vec<u8> {
    ::serde_json::to_vec(body).unwrap_or_default()
}

/// The repository this blob set belongs to, as a caller naming one reads its location: the
/// bucket its settings name and the prefix every key is formed under.
pub(super) fn location(repository: &SnapshotRepository) -> (String, String) {
    (
        repository.bucket().to_string(),
        prefix(repository.base_path()),
    )
}
