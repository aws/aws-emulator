// SPDX-License-Identifier: CC0-1.0

//! The data plane's SNAPSHOT API: the repositories a domain's cluster registers
//! (`/_snapshot/{repository}`), the snapshots it takes into one
//! (`/_snapshot/{repository}/{snapshot}`) and the restore of one
//! (`/_snapshot/{repository}/{snapshot}/_restore`).
//!
//! A snapshot repository is where a cluster KEEPS its snapshots, and on Amazon OpenSearch
//! Service that place is an Amazon S3 bucket the caller owns: the registration names the
//! bucket, the `base_path` every blob lives under inside it, and the IAM role the domain
//! copies with. So this surface is not a record-keeping API — taking a snapshot WRITES the
//! repository's blobs into that bucket, deleting one REMOVES the blobs it wrote, and
//! restoring one reads its indexes back out of them ([`blobs`] owns the layout). The bucket
//! is the one Amazon S3 itself holds, in the same shared engine, so what a snapshot wrote is
//! exactly what `s3:ListObjectsV2` and `s3:GetObject` report.
//!
//! # Where each half runs
//!
//! Two stores are involved and this emulator never holds two at once, so each mutating route
//! is served in two phases — the same shape a monitor run is served in
//! ([`super::monitor_run`]):
//!
//! 1. PLAN, under this service's store guard: the route authorizes the request, refuses
//!    everything the cluster refuses, and builds — from the domain's own records — the whole
//!    of what the repository is about to hold: the snapshot record, every blob and every key.
//!    Nothing is recorded yet, and the response the route returns is a placeholder.
//! 2. PERFORM, with no store of this service held ([`performed`]): the object store is
//!    opened, the blobs are written, read or removed, and only then is this service's store
//!    taken again to record what the repository now holds. A blob write the object store
//!    refuses records nothing, so the cluster never reports a snapshot the bucket does not
//!    hold.

use ::std::collections::BTreeMap;
use ::std::time::SystemTime;

use ::aws_smithy_runtime_api::http::Response as RtResponse;
use ::serde_json::{json, Map, Value};
use ::service_com_amazonaws_s3_amazons3::state as objects;

use crate::state::{
    self, fgac, ingest, service_state, Domain, RepositorySnapshot, SnapshotRepository,
};

use super::{error, guarded, ok, stable_uuid, wrong_method, Request, PARSE_EXCEPTION};

/// The layout of one repository's blobs: where each lives under the declared base path and
/// what each holds.
mod blobs;

/// The engine's error type for a repository the cluster has not registered.
const REPOSITORY_MISSING_EXCEPTION: &str = "repository_missing_exception";

/// The engine's error type for a repository definition it cannot make a repository of.
const REPOSITORY_EXCEPTION: &str = "repository_exception";

/// The engine's error type for a repository whose own location it cannot reach.
const REPOSITORY_VERIFICATION_EXCEPTION: &str = "repository_verification_exception";

/// The engine's error type for a snapshot the repository does not hold.
const SNAPSHOT_MISSING_EXCEPTION: &str = "snapshot_missing_exception";

/// The engine's error type for a snapshot name it will not take a snapshot under.
const INVALID_SNAPSHOT_NAME_EXCEPTION: &str = "invalid_snapshot_name_exception";

/// The engine's error type for a restore it cannot perform.
const SNAPSHOT_RESTORE_EXCEPTION: &str = "snapshot_restore_exception";

/// The engine's error type for a request whose own members it validates before acting.
const ACTION_REQUEST_VALIDATION_EXCEPTION: &str = "action_request_validation_exception";

/// The engine's error type for a snapshot that cannot run because another is.
const CONCURRENT_SNAPSHOT_EXCEPTION: &str = "concurrent_snapshot_execution_exception";

/// The characters a snapshot name may not carry: the engine's own invalid-file-name set,
/// since a snapshot's name becomes part of the blob names it writes.
const FORBIDDEN_NAME_CHARACTERS: [char; 10] =
    ['\\', '/', '*', '?', '"', '<', '>', '|', ' ', ','];

/// That set as the refusal names it, in the order the engine lists it.
const FORBIDDEN_NAME_LIST: &str = "[\\, /, *, ?, \", <, >, |,  , ,]";

/// The reserved name a request addresses every snapshot or repository by.
const ALL: &str = "_all";

/// The parameter a client asks to be answered only once the copy is made with.
const WAIT_FOR_COMPLETION: &str = "wait_for_completion";

/// The most bytes of documents ONE snapshot copies into a repository.
///
/// The emulator's OWN bound, like [`super::MAX_REQUEST_BODY_BYTES`]: a snapshot copies the
/// documents of every index it names, and both the copy and the blob it is written into live
/// in this process's heap, so a snapshot past this is refused rather than built. The object
/// store's own byte and object ceilings bound what a repository accumulates across snapshots.
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

/// The longest `rename_pattern` a restore may carry.
///
/// The emulator's OWN bound: the expression comes from the request and is compiled per
/// restore, so its length is what the compilation costs — and an index name, which is all it
/// is matched against, is at most 255 characters.
const MAX_RENAME_PATTERN_CHARS: usize = 256;

/// How much backtracking one rename match may do before it is abandoned — the budget this
/// service's other backtracking expression runs under, for the same reason.
const RENAME_BACKTRACK_LIMIT: usize = 1_000_000;

/// `GET /_snapshot` — every repository this cluster has registered.
pub(super) fn repositories_request(domain: &mut Domain, request: &Request<'_>) -> RtResponse {
    match request.method {
        "GET" => guarded(domain, request, fgac::ACTION_GET_REPOSITORY, |domain| {
            match repositories_view(domain, None) {
                Ok(view) => ok(&view),
                Err(refusal) => *refusal,
            }
        }),
        _ => wrong_method(request, "GET"),
    }
}

/// `PUT`, `GET` or `DELETE` on ONE repository — or on the set a pattern names, for the read.
pub(super) fn repository_request(
    domain: &mut Domain,
    request: &Request<'_>,
    named: &str,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => guarded(domain, request, fgac::ACTION_PUT_REPOSITORY, |domain| {
            planned_registration(domain, request, named)
        }),
        "GET" => guarded(domain, request, fgac::ACTION_GET_REPOSITORY, |domain| {
            match repositories_view(domain, Some(named)) {
                Ok(view) => ok(&view),
                Err(refusal) => *refusal,
            }
        }),
        // Unregistering a repository takes the cluster's registration away and leaves the
        // bucket exactly as it is: the snapshots are the repository's own data, and a client
        // that registers the same location again finds them there.
        "DELETE" => guarded(domain, request, fgac::ACTION_DELETE_REPOSITORY, |domain| {
            match domain.snapshot_repositories.remove(named) {
                Some(_) => ok(&json!({ "acknowledged": true })),
                None => repository_missing(named),
            }
        }),
        _ => wrong_method(request, "PUT, POST, GET, DELETE"),
    }
}

/// `PUT`, `GET` or `DELETE` on the snapshots of one repository: one snapshot by name, or the
/// set a pattern names for the read.
pub(super) fn snapshots_request(
    domain: &mut Domain,
    request: &Request<'_>,
    repository: &str,
    named: &str,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => guarded(domain, request, fgac::ACTION_CREATE_SNAPSHOT, |domain| {
            planned_snapshot(domain, request, repository, named, now)
        }),
        "GET" => guarded(domain, request, fgac::ACTION_GET_SNAPSHOT, |domain| {
            match snapshots_view(domain, repository, named, now) {
                Ok(view) => ok(&view),
                Err(refusal) => *refusal,
            }
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_DELETE_SNAPSHOT, |domain| {
            planned_deletion(domain, request, repository, named)
        }),
        _ => wrong_method(request, "PUT, POST, GET, DELETE"),
    }
}

/// `POST /_snapshot/{repository}/{snapshot}/_restore` — the indexes of a snapshot, read back
/// into this cluster.
pub(super) fn restore_request(
    domain: &mut Domain,
    request: &Request<'_>,
    repository: &str,
    snapshot: &str,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_RESTORE_SNAPSHOT, |domain| {
            planned_restore(domain, request, repository, snapshot)
        }),
        _ => wrong_method(request, "POST"),
    }
}

/// The repositories a read reports: every one the cluster holds, or the ones the names the
/// path carries resolve to — a comma-separated list, `_all`, or a wildcard pattern.
///
/// A plain name the cluster has not registered is the missing repository it names, which is
/// what the engine answers such a read with; a pattern that matches none reports none.
fn repositories_view(domain: &Domain, named: Option<&str>) -> Result<Value, Box<RtResponse>> {
    let mut reported = Map::new();
    let Some(named) = named else {
        for repository in domain.snapshot_repositories.values() {
            reported.insert(repository.name.clone(), repository.definition());
        }
        return Ok(Value::Object(reported));
    };
    for pattern in named.split(',').filter(|name| !name.is_empty()) {
        if pattern == ALL || pattern.contains('*') {
            for repository in domain
                .snapshot_repositories
                .values()
                .filter(|held| named_by(pattern, &held.name))
            {
                reported.insert(repository.name.clone(), repository.definition());
            }
            continue;
        }
        let Some(repository) = domain.snapshot_repositories.get(pattern) else {
            return Err(Box::new(repository_missing(pattern)));
        };
        reported.insert(repository.name.clone(), repository.definition());
    }
    Ok(Value::Object(reported))
}

/// Whether `pattern` — one name, `_all`, or a wildcard pattern — names `name`. The reading
/// the index resolution applies ([`super::matched_index`]), so a repository pattern and an
/// index one select by the same rule.
fn named_by(pattern: &str, name: &str) -> bool {
    super::matched_index(pattern, name)
}

/// The snapshots a read reports, as `{"snapshots": [...]}`: the ones `named` resolves to in
/// `repository` — a comma-separated list, `_all`, or a wildcard pattern.
fn snapshots_view(
    domain: &Domain,
    repository: &str,
    named: &str,
    now: SystemTime,
) -> Result<Value, Box<RtResponse>> {
    let held = registered(domain, repository)?;
    let mut reported = Vec::new();
    for pattern in named.split(',').filter(|name| !name.is_empty()) {
        if pattern == ALL || pattern.contains('*') {
            reported.extend(
                held.snapshots
                    .values()
                    .filter(|snapshot| named_by(pattern, &snapshot.name))
                    .map(|snapshot| snapshot.info_at(now)),
            );
            continue;
        }
        let Some(snapshot) = held.snapshots.get(pattern) else {
            return Err(Box::new(snapshot_missing(repository, pattern)));
        };
        reported.push(snapshot.info_at(now));
    }
    Ok(json!({ "snapshots": reported }))
}

/// The repository `named`, or the refusal for one the cluster has not registered.
fn registered<'d>(
    domain: &'d Domain,
    named: &str,
) -> Result<&'d SnapshotRepository, Box<RtResponse>> {
    domain
        .snapshot_repositories
        .get(named)
        .ok_or_else(|| Box::new(repository_missing(named)))
}

// ---------------------------------------------------------------------------
// PHASE 1 — planning, under this service's store guard.
// ---------------------------------------------------------------------------

/// The repository work a request PLANNED: everything the object store and the records need,
/// with nothing borrowed from the store it was planned under.
#[derive(Debug)]
pub(super) enum PendingWork {
    /// A repository to register, once its bucket is reachable and whatever it already holds
    /// has been read back.
    Register(Registration),
    /// A snapshot to copy into a repository.
    Create(Creation),
    /// A snapshot's blobs to remove from a repository.
    Delete(Deletion),
    /// A snapshot's indexes to read back into this cluster.
    Restore(Restoration),
}

/// A planned registration.
#[derive(Debug)]
pub(super) struct Registration {
    repository: String,
    repository_type: String,
    /// The definition as the request wrote it, which is what the API reports back.
    definition: Value,
    bucket: String,
    /// The key prefix the declared base path forms, under which every blob of this
    /// repository lives.
    prefix: String,
}

/// A planned snapshot: the record it will be, every blob it writes, and the repository-data
/// generation it moves the repository to.
#[derive(Debug)]
pub(super) struct Creation {
    repository: String,
    bucket: String,
    prefix: String,
    snapshot: RepositorySnapshot,
    written: Vec<blobs::Blob>,
    /// The generation the repository stood at when this was planned, whose data blob this
    /// change supersedes — `None` for a repository that holds no data blob yet.
    superseded: Option<u64>,
    generation: u64,
    /// Whether the client asked to be answered only once the copy was made.
    waited: bool,
}

/// A planned deletion: the snapshot's own blobs, and the repository data left behind.
#[derive(Debug)]
pub(super) struct Deletion {
    repository: String,
    bucket: String,
    prefix: String,
    snapshot: String,
    removed: Vec<String>,
    written: Vec<blobs::Blob>,
    superseded: Option<u64>,
    generation: u64,
}

/// A planned restore: which blobs hold each index, and the name each is restored under.
#[derive(Debug)]
pub(super) struct Restoration {
    repository: String,
    bucket: String,
    snapshot: String,
    /// One per index the restore reads: the name it is restored UNDER, the metadata blob and
    /// the data blob it is read from.
    restored: Vec<RestoredIndex>,
}

/// One index a restore reads back.
#[derive(Debug)]
struct RestoredIndex {
    target: String,
    metadata_key: String,
    data_key: String,
}

/// Plan the registration the request carries: the repository definition is validated here,
/// and whether its location can be reached is decided once the object store is open
/// ([`register`]).
fn planned_registration(domain: &mut Domain, request: &Request<'_>, named: &str) -> RtResponse {
    let definition = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        // A registration that carries no document at all names no type, which is what the
        // validation below states.
        Ok(None) => json!({}),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    let repository_type = definition
        .get(state::REPOSITORY_TYPE_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // "type is missing": the engine validates a put-repository request's own members before
    // it asks any repository plugin for anything.
    if repository_type.is_empty() {
        return error(
            400,
            ACTION_REQUEST_VALIDATION_EXCEPTION,
            "Validation Failed: 1: type is missing;",
        );
    }
    // Amazon OpenSearch Service registers the Amazon S3 repository plugin, and this emulator
    // backs a repository with the bucket that plugin's settings name; a type it has no plugin
    // for is the type the cluster does not have.
    if repository_type != state::REPOSITORY_TYPE_S3 {
        return error(
            500,
            REPOSITORY_EXCEPTION,
            &format!("[{named}] repository type [{repository_type}] does not exist"),
        );
    }
    let bucket = definition
        .get(state::REPOSITORY_SETTINGS_MEMBER)
        .and_then(|settings| settings.get(state::REPOSITORY_SETTING_BUCKET))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if bucket.is_empty() {
        return error(
            500,
            REPOSITORY_EXCEPTION,
            &format!("[{named}] No bucket defined for s3 repository"),
        );
    }
    if !domain.snapshot_repositories.contains_key(named)
        && domain.snapshot_repositories.len() >= state::REPOSITORY_QUOTA
    {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "domain [{}] already holds the maximum of {} snapshot repositories",
                domain.domain_name,
                state::REPOSITORY_QUOTA
            ),
        );
    }
    let prefix = blobs::prefix(
        definition
            .get(state::REPOSITORY_SETTINGS_MEMBER)
            .and_then(|settings| settings.get(state::REPOSITORY_SETTING_BASE_PATH))
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    *request.pending_snapshot.borrow_mut() = Some(PendingWork::Register(Registration {
        repository: named.to_string(),
        repository_type,
        definition,
        bucket,
        prefix,
    }));
    ok(&Value::Null)
}

/// Plan the snapshot the request asks for: which indexes it copies, the record it will be and
/// every blob it writes, all read off the domain under this guard.
fn planned_snapshot(
    domain: &mut Domain,
    request: &Request<'_>,
    repository: &str,
    named: &str,
    now: SystemTime,
) -> RtResponse {
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(None) => json!({}),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    let held = match registered(domain, repository) {
        Ok(held) => held,
        Err(refusal) => return *refusal,
    };
    if let Some(refusal) = invalid_snapshot_name(repository, named) {
        return refusal;
    }
    if held.snapshots.contains_key(named) {
        return error(
            400,
            INVALID_SNAPSHOT_NAME_EXCEPTION,
            &format!(
                "[{repository}:{named}] Invalid snapshot name [{named}], snapshot with the \
                 same name already exists"
            ),
        );
    }
    if held.snapshots.len() >= state::SNAPSHOT_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "repository [{repository}] already holds the maximum of {} snapshots",
                state::SNAPSHOT_QUOTA
            ),
        );
    }
    let (bucket, prefix) = blobs::location(held);
    let superseded = (held.generation > 0).then_some(held.generation);
    let generation = held.generation.saturating_add(1);
    // "indices: a comma-separated list of data streams and indices to include in the
    // snapshot", defaulting to every index the cluster holds.
    let named_indexes = body
        .get("indices")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ignore_unavailable = body
        .get("ignore_unavailable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let names = match snapshotted_names(domain, named_indexes.as_deref(), ignore_unavailable) {
        Ok(names) => names,
        Err(refusal) => return *refusal,
    };
    // The copy is measured BEFORE it is made: the documents of every named index are about to
    // be rendered into this process's heap a second time, so the bound is checked over the
    // documents the cluster holds rather than over a copy already taken.
    let bytes = copied_bytes(domain, &names);
    if bytes > MAX_SNAPSHOT_BYTES {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "the indexes named hold {bytes} bytes of documents, more than the \
                 {MAX_SNAPSHOT_BYTES} bytes this cluster copies into a repository"
            ),
        );
    }
    let copied = copied_indexes(domain, &names);
    // "include_global_state: whether to include the cluster state in the snapshot", which
    // the engine includes unless the request says otherwise.
    let include_global_state = body
        .get("include_global_state")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let waited = request
        .parameter(WAIT_FOR_COMPLETION)
        .is_some_and(|asked| asked == "true");
    let snapshot = RepositorySnapshot {
        name: named.to_string(),
        uuid: ::uuid::Uuid::new_v4().to_string(),
        version: super::engine_distribution(&domain.engine_version).1,
        indices: copied.iter().map(|index| index.name.clone()).collect(),
        index_uuids: copied
            .iter()
            .map(|index| (index.name.clone(), index.uuid.clone()))
            .collect(),
        include_global_state,
        shards: i64::try_from(copied.len()).unwrap_or(i64::MAX),
        started_at: now,
        // A client that asked to WAIT is answered a snapshot whose copy has been made by the
        // time the call returns; one that did not is answered straight away, and the copy
        // falls due a span later ([`state::SNAPSHOT_COPY`]), which is the window its
        // `IN_PROGRESS` state is visible in.
        completes_at: if waited {
            now
        } else {
            now.checked_add(state::SNAPSHOT_COPY).unwrap_or(now)
        },
    };
    let global = global_state(domain, include_global_state);
    let mut written = blobs::snapshot_blobs(&prefix, &snapshot, &copied, (&global.0, &global.1));
    let mut after = held.snapshots.clone();
    after.insert(snapshot.name.clone(), snapshot.clone());
    written.extend(blobs::repository_data_blobs(&prefix, &after, generation));
    *request.pending_snapshot.borrow_mut() = Some(PendingWork::Create(Creation {
        repository: repository.to_string(),
        bucket,
        prefix,
        snapshot,
        written,
        superseded,
        generation,
        waited,
    }));
    ok(&Value::Null)
}

/// The NAMES of the indexes one snapshot copies: what the request's `indices` resolved to
/// against the indexes the cluster holds.
fn snapshotted_names(
    domain: &Domain,
    named: Option<&str>,
    ignore_unavailable: bool,
) -> Result<Vec<String>, Box<RtResponse>> {
    match super::resolved_indexes(domain, named) {
        Ok(resolved) => Ok(resolved.into_iter().map(str::to_string).collect()),
        // "ignore_unavailable: if false, the request returns an error for any data stream or
        // index that is missing or closed" — with it true, the names that DO resolve are the
        // ones the snapshot copies and the rest are passed over.
        Err(refusal) if ignore_unavailable => {
            let _ = refusal;
            Ok(named
                .unwrap_or(state::ALL_INDEXES)
                .split(',')
                .filter(|pattern| !pattern.is_empty())
                .filter_map(|pattern| super::matched_indexes(domain, pattern).ok())
                .flatten()
                .map(str::to_string)
                .collect::<::std::collections::BTreeSet<String>>()
                .into_iter()
                .collect())
        }
        Err(refusal) => Err(refusal),
    }
}

/// How many bytes of documents the indexes `names` hold — what the copy about to be made
/// would occupy, measured one document at a time so nothing larger than a single document is
/// held for the measurement.
fn copied_bytes(domain: &Domain, names: &[String]) -> usize {
    names
        .iter()
        .filter_map(|name| domain.indexes.get(name))
        .flat_map(|index| index.documents.values())
        .map(|document| {
            document.id.len()
                + ::serde_json::to_vec(&document.source)
                    .map(|rendered| rendered.len())
                    .unwrap_or_default()
        })
        .sum()
}

/// The indexes one snapshot copies, as the cluster holds them at this instant: each with the
/// uuid its blobs live under, its settings and mappings, and the documents it holds.
fn copied_indexes(domain: &Domain, names: &[String]) -> Vec<blobs::CopiedIndex> {
    names
        .iter()
        .filter_map(|name| {
            let index = domain.indexes.get(name)?;
            let schema = state::json_from_document(&index.schema);
            Some(blobs::CopiedIndex {
                name: name.clone(),
                uuid: stable_uuid(&format!("{}/{name}", domain.arn)),
                settings: Value::Object(state::index_settings(&index.schema)),
                mappings: schema
                    .get(state::SCHEMA_MAPPINGS)
                    .cloned()
                    .unwrap_or(json!({})),
                documents: index
                    .documents
                    .values()
                    .map(|document| (document.id.clone(), document.source.clone()))
                    .collect(),
            })
        })
        .collect()
}

/// The cluster-wide state a snapshot copies: the index templates and the cluster's own
/// persistent settings, and the empty sections for a snapshot that includes no global state.
fn global_state(domain: &Domain, include_global_state: bool) -> (Value, Value) {
    if !include_global_state {
        return (json!({}), json!({}));
    }
    let mut templates = Map::new();
    for template in domain.index_templates.values() {
        templates.insert(template.name.clone(), template.definition.clone());
    }
    let mut settings = Map::new();
    for (name, value) in &domain.cluster_settings.persistent {
        settings.insert(name.clone(), Value::String(value.clone()));
    }
    (Value::Object(templates), Value::Object(settings))
}

/// Plan the deletion of one snapshot: the blobs it wrote, and the repository data the
/// repository is left holding.
fn planned_deletion(
    domain: &mut Domain,
    request: &Request<'_>,
    repository: &str,
    named: &str,
) -> RtResponse {
    let held = match registered(domain, repository) {
        Ok(held) => held,
        Err(refusal) => return *refusal,
    };
    let Some(snapshot) = held.snapshots.get(named) else {
        return snapshot_missing(repository, named);
    };
    let (bucket, prefix) = blobs::location(held);
    let superseded = (held.generation > 0).then_some(held.generation);
    let generation = held.generation.saturating_add(1);
    let removed = blobs::snapshot_keys(&prefix, snapshot);
    let mut after = held.snapshots.clone();
    after.remove(named);
    let written = blobs::repository_data_blobs(&prefix, &after, generation);
    *request.pending_snapshot.borrow_mut() = Some(PendingWork::Delete(Deletion {
        repository: repository.to_string(),
        bucket,
        prefix,
        snapshot: named.to_string(),
        removed,
        written,
        superseded,
        generation,
    }));
    ok(&Value::Null)
}

/// Plan the restore of one snapshot: which of its indexes are read, and under which name
/// each is restored.
fn planned_restore(
    domain: &mut Domain,
    request: &Request<'_>,
    repository: &str,
    named: &str,
) -> RtResponse {
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(None) => json!({}),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    let held = match registered(domain, repository) {
        Ok(held) => held,
        Err(refusal) => return *refusal,
    };
    let Some(snapshot) = held.snapshots.get(named) else {
        // A RESTORE states a snapshot it cannot find as a restore that cannot be made — its
        // own exception class, not the missing-resource one a read of the snapshot answers:
        // the request named a restore the cluster could not carry out.
        return error(
            500,
            SNAPSHOT_RESTORE_EXCEPTION,
            &format!("[{repository}:{named}] snapshot does not exist"),
        );
    };
    let (bucket, prefix) = blobs::location(held);
    let selected = body
        .get("indices")
        .and_then(Value::as_str)
        .unwrap_or(state::ALL_INDEXES);
    let renaming = match renamed(&body) {
        Ok(renaming) => renaming,
        Err(refusal) => return *refusal,
    };
    let mut restored = Vec::new();
    for index in &snapshot.indices {
        if !selected
            .split(',')
            .any(|pattern| !pattern.is_empty() && named_by(pattern, index))
        {
            continue;
        }
        let Some(uuid) = snapshot.index_uuids.get(index) else {
            continue;
        };
        let target = match &renaming {
            Some((pattern, replacement)) => pattern.replace_all(index, replacement).to_string(),
            None => index.clone(),
        };
        // "cannot restore index […] because an open index with same name already exists in
        // the cluster": a restore writes the index itself, so the cluster refuses one it
        // already holds rather than replacing it.
        if domain.indexes.contains_key(&target) {
            return error(
                500,
                SNAPSHOT_RESTORE_EXCEPTION,
                &format!(
                    "[{repository}:{named}] cannot restore index [{target}] because an open \
                     index with same name already exists in the cluster. Either close or \
                     delete the existing index or restore the index under a different name by \
                     providing a rename pattern and replacement name"
                ),
            );
        }
        restored.push(RestoredIndex {
            target,
            metadata_key: blobs::index_metadata_key(&prefix, uuid, &snapshot.uuid),
            data_key: blobs::shard_data_key(&prefix, uuid, blobs::COPIED_SHARD, &snapshot.uuid),
        });
    }
    *request.pending_snapshot.borrow_mut() = Some(PendingWork::Restore(Restoration {
        repository: repository.to_string(),
        bucket,
        snapshot: named.to_string(),
        restored,
    }));
    ok(&Value::Null)
}

/// The rename a restore applies to the indexes it reads: `rename_pattern` is "a rename
/// pattern to apply to restored data streams and indices" and `rename_replacement` what it is
/// replaced with, so the pair is a regular expression and its replacement — compiled by the
/// maintained engine rather than matched by hand here.
///
/// `None` is a restore that renames nothing, and a pattern that is no regular expression is
/// refused rather than read as a literal.
///
/// The expression is REQUEST TEXT and the engine backtracks, so the work one restore can cost
/// is bounded twice before anything is matched: the pattern itself is refused past
/// [`MAX_RENAME_PATTERN_CHARS`], and the match runs under the same backtracking budget this
/// service's other expression does
/// ([`state::custom_endpoint`]) — a match that exhausts it renames nothing instead of spinning.
/// What is matched is an index name the cluster holds, never the body.
#[allow(clippy::type_complexity)]
fn renamed(body: &Value) -> Result<Option<(::fancy_regex::Regex, String)>, Box<RtResponse>> {
    let Some(pattern) = body.get("rename_pattern").and_then(Value::as_str) else {
        return Ok(None);
    };
    let replacement = body
        .get("rename_replacement")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if pattern.chars().count() > MAX_RENAME_PATTERN_CHARS {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "rename pattern is longer than the {MAX_RENAME_PATTERN_CHARS} characters this \
                 cluster compiles"
            ),
        )));
    }
    match ::fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(RENAME_BACKTRACK_LIMIT)
        .build()
    {
        Ok(compiled) => Ok(Some((compiled, replacement))),
        Err(failed) => Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("rename pattern [{pattern}] is invalid: {failed}"),
        ))),
    }
}

// ---------------------------------------------------------------------------
// PHASE 2 — performing, with no store of this service held.
// ---------------------------------------------------------------------------

/// Perform the repository work `planned` and answer the request with what it came to.
///
/// Called by [`super::served`] once this service's store has been released, so the object
/// store this opens is the only one held while the blobs are written, read or removed; the
/// records are taken up again afterwards, under this service's own guard.
pub(super) fn performed<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    planned: PendingWork,
    now: SystemTime,
) -> RtResponse {
    match planned {
        PendingWork::Register(registration) => register(engine, domain_name, &registration),
        PendingWork::Create(creation) => create(engine, domain_name, creation, now),
        PendingWork::Delete(deletion) => delete(engine, domain_name, &deletion),
        PendingWork::Restore(restoration) => restore(engine, domain_name, &restoration, now),
    }
}

/// Register the planned repository: its location has to be reachable, and whatever the
/// location already holds is read back as the repository's snapshots.
fn register<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    registration: &Registration,
) -> RtResponse {
    let Some(store) = objects::service_state(engine) else {
        return unreachable_location(&registration.repository, &registration.prefix);
    };
    let (generation, snapshots) = {
        let objects = store.lock();
        // A repository is registered with its location verified, so a bucket the object store
        // does not hold is a repository whose path the cluster cannot reach.
        let Some(bucket) = objects.bucket(&registration.bucket) else {
            return unreachable_location(&registration.repository, &registration.prefix);
        };
        let generation = bucket
            .object(&blobs::generation_key(&registration.prefix))
            .and_then(|marker| blobs::read_generation(&marker.body))
            .unwrap_or_default();
        let snapshots = bucket
            .object(&blobs::repository_data_key(
                &registration.prefix,
                generation,
            ))
            .map(|data| blobs::read_repository_data(&data.body))
            .unwrap_or_default();
        (generation, snapshots)
    };
    let Some(state) = service_state(engine) else {
        return unavailable();
    };
    let mut store = state.lock();
    let Some(domain) = store.settled_mut(domain_name, SystemTime::now()) else {
        return unavailable();
    };
    domain.snapshot_repositories.insert(
        registration.repository.clone(),
        SnapshotRepository {
            name: registration.repository.clone(),
            repository_type: registration.repository_type.clone(),
            settings: registration.definition.clone(),
            generation,
            snapshots,
        },
    );
    ok(&json!({ "acknowledged": true }))
}

/// Copy the planned snapshot: write its blobs, then record it on the repository.
fn create<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    creation: Creation,
    now: SystemTime,
) -> RtResponse {
    let Some(store) = objects::service_state(engine) else {
        return unwritable_repository(&creation.repository);
    };
    {
        let mut objects = store.lock();
        if let Err(refused) = written(&mut objects, &creation.bucket, &creation.written) {
            ::tracing::warn!(
                repository = %creation.repository,
                bucket = %creation.bucket,
                %refused,
                "a snapshot's blobs could not be written, so the snapshot was not taken"
            );
            return unwritable_repository(&creation.repository);
        }
        if let Some(superseded) = creation.superseded {
            // The repository data of the generation this change supersedes is no longer read
            // by anything, so it is removed rather than left to accumulate — the way a
            // repository drops a stale index blob.
            let stale = blobs::repository_data_key(&creation.prefix, superseded);
            let _ = objects.remove_object(&creation.bucket, &stale);
        }
    }
    let Some(state) = service_state(engine) else {
        return unavailable();
    };
    let mut store = state.lock();
    let Some(domain) = store.settled_mut(domain_name, now) else {
        return unavailable();
    };
    let Some(repository) = domain.snapshot_repositories.get_mut(&creation.repository) else {
        return repository_missing(&creation.repository);
    };
    // Another call moved the repository on while this copy was being made, so the data blob
    // this one wrote is not the repository's any more: the snapshot is not recorded, which is
    // how the engine refuses two snapshots running over one repository at once.
    if repository.generation.saturating_add(1) != creation.generation
        || repository.snapshots.contains_key(&creation.snapshot.name)
    {
        return error(
            503,
            CONCURRENT_SNAPSHOT_EXCEPTION,
            &format!(
                "[{}:{}] a snapshot is already running",
                creation.repository, creation.snapshot.name
            ),
        );
    }
    repository.generation = creation.generation;
    repository
        .snapshots
        .insert(creation.snapshot.name.clone(), creation.snapshot.clone());
    if creation.waited {
        return ok(&json!({ "snapshot": creation.snapshot.info_at(now) }));
    }
    // A client that did not ask to wait is told the snapshot was accepted; its state is read
    // back through the snapshot API.
    ok(&json!({ "accepted": true }))
}

/// Remove the planned snapshot's blobs, then take it off the repository.
fn delete<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    deletion: &Deletion,
) -> RtResponse {
    let Some(store) = objects::service_state(engine) else {
        return unwritable_repository(&deletion.repository);
    };
    {
        let mut objects = store.lock();
        if objects.bucket(&deletion.bucket).is_none() {
            return unwritable_repository(&deletion.repository);
        }
        for key in &deletion.removed {
            let _ = objects.remove_object(&deletion.bucket, key);
        }
        if let Err(refused) = written(&mut objects, &deletion.bucket, &deletion.written) {
            ::tracing::warn!(
                repository = %deletion.repository,
                bucket = %deletion.bucket,
                %refused,
                "the repository data of a snapshot deletion could not be written"
            );
            return unwritable_repository(&deletion.repository);
        }
        if let Some(superseded) = deletion.superseded {
            let stale = blobs::repository_data_key(&deletion.prefix, superseded);
            let _ = objects.remove_object(&deletion.bucket, &stale);
        }
    }
    let Some(state) = service_state(engine) else {
        return unavailable();
    };
    let mut store = state.lock();
    let Some(domain) = store.settled_mut(domain_name, SystemTime::now()) else {
        return unavailable();
    };
    let Some(repository) = domain.snapshot_repositories.get_mut(&deletion.repository) else {
        return repository_missing(&deletion.repository);
    };
    if repository.snapshots.remove(&deletion.snapshot).is_none() {
        return snapshot_missing(&deletion.repository, &deletion.snapshot);
    }
    repository.generation = deletion.generation;
    ok(&json!({ "acknowledged": true }))
}

/// Read the planned snapshot's indexes back into the cluster: the settings, the mappings and
/// the documents each index is restored with all come out of the blobs the snapshot wrote.
fn restore<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain_name: &str,
    restoration: &Restoration,
    now: SystemTime,
) -> RtResponse {
    let Some(store) = objects::service_state(engine) else {
        return unreadable_repository(&restoration.repository);
    };
    let mut read = Vec::new();
    {
        let objects = store.lock();
        let Some(bucket) = objects.bucket(&restoration.bucket) else {
            return unreadable_repository(&restoration.repository);
        };
        for index in &restoration.restored {
            let Some((settings, mappings)) = bucket
                .object(&index.metadata_key)
                .and_then(|metadata| blobs::read_index_metadata(&metadata.body))
            else {
                return unreadable_repository(&restoration.repository);
            };
            let documents = bucket
                .object(&index.data_key)
                .map(|data| blobs::read_shard_data(&data.body))
                .unwrap_or_default();
            read.push((index.target.clone(), settings, mappings, documents));
        }
    }
    let Some(state) = service_state(engine) else {
        return unavailable();
    };
    let mut store = state.lock();
    let Some(domain) = store.settled_mut(domain_name, now) else {
        return unavailable();
    };
    let mut restored = Vec::new();
    for (target, settings, mappings, documents) in read {
        if domain.indexes.contains_key(&target) || domain.indexes.len() >= state::INDEX_QUOTA {
            continue;
        }
        let schema = state::document_from_json(&json!({
            state::SCHEMA_SETTINGS: settings,
            state::SCHEMA_MAPPINGS: mappings,
        }));
        let mut index = state::Index {
            index_name: target.clone(),
            schema,
            documents: BTreeMap::new(),
            // The aliases an index carried are the cluster's own state and are not part of
            // what a shard's copy holds, so a restored index carries none.
            aliases: BTreeMap::new(),
            next_seq_no: 0,
        };
        for (id, source) in documents {
            let seq_no = index.take_seq_no();
            index.documents.insert(
                id.clone(),
                ingest::IndexedDocument {
                    id,
                    source,
                    indexed_at: now,
                    // The restored index is a new index holding the first revision of each
                    // document it was copied with, numbered by the restore that wrote it.
                    version: 1,
                    seq_no,
                    primary_term: ingest::PRIMARY_TERM,
                },
            );
        }
        domain.indexes.insert(target.clone(), index);
        restored.push(target);
    }
    let shards = i64::try_from(restored.len()).unwrap_or(i64::MAX);
    ok(&json!({
        "snapshot": {
            "snapshot": restoration.snapshot,
            "indices": restored,
            "shards": { "total": shards, "failed": 0, "successful": shards },
        }
    }))
}

/// Write `blobs` into `bucket` of the object store, answering what refused the write.
///
/// Every key is one [`blobs`] formed under the repository's declared base path, and every
/// body is one it rendered, so what reaches the object store is the repository's own layout.
fn written(
    store: &mut objects::ComAmazonawsS3Amazons3State,
    bucket: &str,
    written: &[blobs::Blob],
) -> Result<(), String> {
    for blob in written {
        let stored = store.put_object(
            bucket,
            &blob.key,
            objects::Object::stored(blob.body.clone(), Some(blob.content_type.to_string())),
        );
        match stored {
            Some(objects::ObjectWrite::Created | objects::ObjectWrite::Replaced) => {}
            Some(refused) => return Err(format!("{refused:?}")),
            None => return Err(format!("no bucket named [{bucket}]")),
        }
    }
    Ok(())
}

/// The engine's error type for a request the cluster is not serving at all.
const CLUSTER_BLOCK_EXCEPTION: &str = "cluster_block_exception";

/// The answer a request whose domain was released while its repository work was being
/// performed earns — the same one every other route of this plane answers such a request
/// with.
fn unavailable() -> RtResponse {
    error(
        503,
        CLUSTER_BLOCK_EXCEPTION,
        "blocked by: [SERVICE_UNAVAILABLE]",
    )
}

/// The refusal for a repository the cluster has not registered.
fn repository_missing(repository: &str) -> RtResponse {
    error(
        404,
        REPOSITORY_MISSING_EXCEPTION,
        &format!("[{repository}] missing"),
    )
}

/// The refusal for a snapshot the repository does not hold.
fn snapshot_missing(repository: &str, snapshot: &str) -> RtResponse {
    error(
        404,
        SNAPSHOT_MISSING_EXCEPTION,
        &format!("[{repository}:{snapshot}] is missing"),
    )
}

/// The refusal for a repository whose own location the cluster cannot reach — the answer a
/// registration whose verification failed earns.
fn unreachable_location(repository: &str, prefix: &str) -> RtResponse {
    error(
        500,
        REPOSITORY_VERIFICATION_EXCEPTION,
        &format!("[{repository}] path [{}] is not accessible on master node", prefix.trim_end_matches('/')),
    )
}

/// The refusal for a repository whose blobs could not be written.
fn unwritable_repository(repository: &str) -> RtResponse {
    error(
        500,
        REPOSITORY_EXCEPTION,
        &format!("[{repository}] could not write repository data"),
    )
}

/// The refusal for a repository whose blobs could not be read.
fn unreadable_repository(repository: &str) -> RtResponse {
    error(
        500,
        REPOSITORY_EXCEPTION,
        &format!("[{repository}] could not read repository data"),
    )
}

/// The refusal for a snapshot NAME the engine will not take a snapshot under, and `None` for
/// one it will: the conditions its own validation states, in the order it states them.
fn invalid_snapshot_name(repository: &str, named: &str) -> Option<RtResponse> {
    let invalid = |reason: &str| {
        Some(error(
            400,
            INVALID_SNAPSHOT_NAME_EXCEPTION,
            &format!("[{repository}:{named}] Invalid snapshot name [{named}], {reason}"),
        ))
    };
    if named.is_empty() {
        return invalid("cannot be empty");
    }
    if named.contains(char::is_whitespace) {
        return invalid("must not contain whitespace");
    }
    if named.contains(',') {
        return invalid("must not contain ','");
    }
    if named.contains('#') {
        return invalid("must not contain '#'");
    }
    if named.starts_with('_') {
        return invalid("must not start with '_'");
    }
    if named.chars().any(char::is_uppercase) {
        return invalid("must be lowercase");
    }
    if named
        .chars()
        .any(|character| FORBIDDEN_NAME_CHARACTERS.contains(&character))
    {
        return invalid(&format!(
            "must not contain the following characters {FORBIDDEN_NAME_LIST}"
        ));
    }
    None
}
