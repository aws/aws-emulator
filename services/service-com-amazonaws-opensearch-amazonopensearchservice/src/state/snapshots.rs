// SPDX-License-Identifier: CC0-1.0

//! SNAPSHOT REPOSITORIES a domain's cluster has registered, and the snapshots taken into
//! one.
//!
//! A snapshot repository is where a cluster KEEPS its snapshots, and on Amazon OpenSearch
//! Service that place is an Amazon S3 bucket: the repository a client registers names the
//! bucket, the path inside it every blob of the repository lives under, and the IAM role the
//! domain copies with. The registration itself is cluster metadata — which is why it is held
//! on the [`Domain`] record beside the indexes it snapshots — while the snapshot DATA is the
//! repository's own, written into that bucket as blobs
//! ([`crate::service::domain_rest::snapshot`]).
//!
//! What a record here holds is therefore the cluster's view of the repository: its settings as
//! the client wrote them, the generation of the repository-data blob the last change left
//! behind, and one entry per snapshot the repository holds. The BYTES live in the object store,
//! and a repository registered again at the same location is read back out of them, so the two
//! cannot drift into two independent copies of one repository.

use super::*;

/// The one repository type this service's domains take: "s3", the repository plugin Amazon
/// OpenSearch Service enables, whose settings name the bucket a domain is backed up into.
pub const REPOSITORY_TYPE_S3: &str = "s3";

/// The setting that names the bucket an `s3` repository keeps its blobs in.
pub const REPOSITORY_SETTING_BUCKET: &str = "bucket";

/// The setting that names the path INSIDE that bucket every blob of the repository lives
/// under. A repository that names none keeps its blobs at the root of the bucket.
pub const REPOSITORY_SETTING_BASE_PATH: &str = "base_path";

/// The member a repository's definition carries its type under, as the snapshot API's own
/// documents spell it.
pub const REPOSITORY_TYPE_MEMBER: &str = "type";

/// The member it carries the type's settings under.
pub const REPOSITORY_SETTINGS_MEMBER: &str = "settings";

/// How many snapshot repositories one domain's cluster may hold before a registration is
/// refused.
///
/// The emulator's OWN bound on this process's state, like [`INDEX_QUOTA`]: nothing AWS
/// publishes limits the registrations, and each one is held in this process's heap, so a
/// caller looping with fresh repository names cannot grow it without end.
pub const REPOSITORY_QUOTA: usize = 20;

/// How many snapshots one repository may hold before a snapshot is refused.
///
/// The emulator's own bound, for the same reason: every snapshot copies the documents of the
/// indexes it names into the object store, so this — together with the object store's own
/// byte and count ceilings — is what bounds what one repository costs.
pub const SNAPSHOT_QUOTA: usize = 20;

/// How long the copy a snapshot performs takes before the clock owes its outcome: a snapshot
/// a client did NOT ask to wait for stays `IN_PROGRESS` for this long and is `SUCCESS` from
/// then on.
///
/// Sized like [`DOMAIN_WORK`], and for the same reason: `IN_PROGRESS` is a status a client
/// POLLS for, so the window has to stay open for as long as a client takes to make its next
/// call and read the answer. A client that asked to wait for completion
/// (`wait_for_completion=true`) is answered a snapshot that is already complete, because that
/// call does not return until the copy has been made.
pub const SNAPSHOT_COPY: Duration = Duration::from_secs(3);

/// `SnapshotState`'s `IN_PROGRESS` — the state of a snapshot whose copy the clock still owes.
pub const SNAPSHOT_STATE_IN_PROGRESS: &str = "IN_PROGRESS";

/// `SnapshotState`'s `SUCCESS` — the state of a snapshot whose copy was made.
pub const SNAPSHOT_STATE_SUCCESS: &str = "SUCCESS";

/// One snapshot repository a domain's cluster has registered.
#[derive(Debug, Clone)]
pub struct SnapshotRepository {
    /// The name the registration addressed it by — unique on the cluster, so it IS the key
    /// this record is held under.
    pub name: String,
    /// The repository type, as the registration named it ([`REPOSITORY_TYPE_S3`]).
    pub repository_type: String,
    /// The type's settings, as the registration wrote them: reported back verbatim, because
    /// a repository's settings are the client's own document and not a projection of it.
    pub settings: ::serde_json::Value,
    /// The generation of the repository-data blob (`index-<generation>`) that the last change
    /// to this repository left in the bucket. A fresh repository holding nothing is at
    /// generation 0 and has no such blob until its first snapshot.
    pub generation: u64,
    /// The snapshots this repository holds, keyed by their name — unique within a
    /// repository, so the name IS the key, and iterating a `BTreeMap` is order-stable, which
    /// is what lets two reads report the same snapshots in the same order.
    pub snapshots: BTreeMap<String, RepositorySnapshot>,
}

impl SnapshotRepository {
    /// The bucket this repository keeps its blobs in, empty when its settings name none.
    #[must_use]
    pub fn bucket(&self) -> &str {
        self.setting(REPOSITORY_SETTING_BUCKET)
    }

    /// The path inside that bucket every blob of this repository lives under, as the
    /// settings wrote it — empty for a repository that keeps them at the bucket's root.
    #[must_use]
    pub fn base_path(&self) -> &str {
        self.setting(REPOSITORY_SETTING_BASE_PATH)
    }

    /// The text one setting of this repository carries, empty when it carries none.
    fn setting(&self, name: &str) -> &str {
        self.settings
            .get(REPOSITORY_SETTINGS_MEMBER)
            .and_then(|settings| settings.get(name))
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
    }

    /// The repository as the snapshot API reports it: its type and its settings, which is
    /// the document the registration wrote.
    #[must_use]
    pub fn definition(&self) -> ::serde_json::Value {
        self.settings.clone()
    }
}

/// One snapshot a repository holds.
#[derive(Debug, Clone)]
pub struct RepositorySnapshot {
    /// The name the client took it under — unique within the repository, so it IS the key.
    pub name: String,
    /// The uuid the cluster minted for it, which every blob of this snapshot is named by, so
    /// two snapshots of one name taken at different times never share a blob.
    pub uuid: String,
    /// The engine version number the snapshot was taken by, as the cluster reports its own
    /// version.
    pub version: String,
    /// The indexes it copied, in the order the snapshot names them.
    pub indices: Vec<String>,
    /// The uuid each copied index was held under, keyed by the index name: the blobs of an
    /// index live under its uuid, so this is what lets a delete and a restore address them
    /// after the index itself is gone from the cluster.
    pub index_uuids: BTreeMap<String, String>,
    /// Whether the snapshot also copied the cluster's own global state.
    pub include_global_state: bool,
    /// How many shards it copied — one per index, which is how many shards of documents this
    /// cluster holds per index.
    pub shards: i64,
    /// When the snapshot was started.
    pub started_at: SystemTime,
    /// When its copy is complete: the instant the call was served for a client that waited
    /// for completion, and [`SNAPSHOT_COPY`] after it for one that did not.
    pub completes_at: SystemTime,
}

impl RepositorySnapshot {
    /// The state a reader at `now` sees: `IN_PROGRESS` while the clock still owes the copy,
    /// `SUCCESS` from then on.
    ///
    /// A pure function of the clock: a read reports it and writes nothing.
    #[must_use]
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now < self.completes_at {
            SNAPSHOT_STATE_IN_PROGRESS
        } else {
            SNAPSHOT_STATE_SUCCESS
        }
    }

    /// The `SnapshotInfo` document the snapshot API answers about this snapshot at `now`.
    ///
    /// Every value is read off the record: the name and uuid the create minted, the engine
    /// version that took it, the indexes it copied, the shard counts of that copy, and the
    /// instants it ran between. A snapshot whose copy the clock still owes has no end yet,
    /// which the engine reports as the zero instant and a zero duration.
    #[must_use]
    pub fn info_at(&self, now: SystemTime) -> ::serde_json::Value {
        let state = self.state_at(now);
        let complete = state == SNAPSHOT_STATE_SUCCESS;
        let started = epoch_millis(self.started_at);
        let ended = if complete {
            epoch_millis(self.completes_at)
        } else {
            0
        };
        let mut info = ::serde_json::Map::new();
        info.insert("snapshot".to_string(), self.name.clone().into());
        info.insert("uuid".to_string(), self.uuid.clone().into());
        info.insert("version".to_string(), self.version.clone().into());
        info.insert("indices".to_string(), self.indices.clone().into());
        // "data_streams": this cluster holds none, so a snapshot copies none.
        info.insert(
            "data_streams".to_string(),
            ::serde_json::Value::Array(Vec::new()),
        );
        info.insert(
            "include_global_state".to_string(),
            self.include_global_state.into(),
        );
        info.insert("state".to_string(), state.into());
        info.insert(
            "start_time".to_string(),
            datetime_text(self.started_at).into(),
        );
        info.insert("start_time_in_millis".to_string(), started.into());
        if complete {
            info.insert(
                "end_time".to_string(),
                datetime_text(self.completes_at).into(),
            );
        }
        info.insert("end_time_in_millis".to_string(), ended.into());
        info.insert(
            "duration_in_millis".to_string(),
            ended.saturating_sub(started).max(0).into(),
        );
        // A shard copy this emulator could not make would be reported here; every shard of
        // every named index was copied, so there is no failure to state.
        info.insert("failures".to_string(), ::serde_json::Value::Array(Vec::new()));
        info.insert(
            "shards".to_string(),
            ::serde_json::json!({
                "total": self.shards,
                "failed": 0,
                "successful": self.shards,
            }),
        );
        ::serde_json::Value::Object(info)
    }
}
