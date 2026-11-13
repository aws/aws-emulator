// SPDX-License-Identifier: CC0-1.0

//! The notebooks a Spark enabled workgroup holds, and the ipynb content each of them is.
//!
//! `ImportNotebook` writes a [`NotebookRecord`] into the [`NotebookRegistry`] on this
//! service's store and `ExportNotebook` answers out of that same record, so the content a
//! read reports is the content an earlier import persisted, byte for byte, rather than
//! anything derived from the reading request.
//!
//! A notebook has no lifecycle the clock owes: Athena answers an import with the notebook
//! already readable, so the record is stored complete and no read has to materialise a
//! pending transition.
//!
//! The content a request did not carry inline is read from the ONE object store this
//! emulator has ([`notebook_file_at`]): a notebook imported from an Amazon S3 location is
//! the object that location names, so an import reports success only where that object
//! really is.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::*;

use crate::ops::types::NotebookMetadata;

/// The only `NotebookType` the model declares, and the one Athena documents: "The type of
/// notebook. Currently, the only valid type is `IPYNB`" (`NotebookMetadata$Type`). One of
/// `NOTEBOOK_TYPE_VALUES` in [`crate::ops::types`], so the boundary has already refused
/// every other spelling before a handler runs.
pub const NOTEBOOK_TYPE_IPYNB: &str = "IPYNB";

/// The largest ipynb file an import may carry.
///
/// `ImportNotebook` documents it: "The maximum file size that can be imported is 10
/// megabytes", which is also the modelled `Payload` length ceiling (1..10485760) — so an
/// inline payload past it is refused at the boundary, and this is what bounds the content
/// read from an object store, where the model constrains nothing.
pub const MAX_NOTEBOOK_PAYLOAD_BYTES: usize = 10_485_760;

/// How many notebooks this emulator holds across every workgroup.
///
/// The EMULATOR's own bound, with no counterpart in the model: each notebook retains its
/// whole ipynb content, so without a bound a sequence of imports could make the emulator
/// keep notebook content without limit. A count AND a byte budget
/// ([`MAX_NOTEBOOK_BYTES_TOTAL`]), because one notebook's size is itself bounded only by
/// the 10 megabytes above.
pub const MAX_NOTEBOOKS: usize = 100;

/// How many bytes of notebook content this emulator holds across every workgroup. The
/// other half of the bound [`MAX_NOTEBOOKS`] starts; see there.
pub const MAX_NOTEBOOK_BYTES_TOTAL: usize = 64 * 1024 * 1024;

/// The engine family whose workgroups run notebooks.
///
/// Athena's two engine families are named by the engine versions `ListEngineVersions`
/// reports: an Athena SQL workgroup selects an "Athena engine version", and an Apache
/// Spark workgroup — the kind `ImportNotebook` documents it imports into, "a Spark enabled
/// workgroup" — selects a PySpark one. The PREFIX is what identifies the family, so a
/// workgroup on any PySpark engine version is Spark enabled.
pub const PYSPARK_ENGINE_VERSION_PREFIX: &str = "PySpark engine version";

/// One notebook, as this service persists it.
#[derive(Debug, Clone)]
pub struct NotebookRecord {
    /// The `NotebookId` this notebook was minted under, which is also its key in the
    /// registry.
    pub id: String,
    /// The notebook's name, as the create or import request gave it.
    pub name: String,
    /// The Spark enabled workgroup the notebook belongs to.
    pub work_group: String,
    /// A `NotebookType` value: [`NOTEBOOK_TYPE_IPYNB`], the only one the model declares.
    pub notebook_type: String,
    /// The ipynb content, held exactly as it arrived — the bytes an export answers with.
    pub payload: String,
    /// When the notebook was created, reported as `NotebookMetadata$CreationTime`.
    pub created_at: SystemTime,
    /// When the notebook's content last changed, reported as
    /// `NotebookMetadata$LastModifiedTime`. The creation instant until something updates
    /// the notebook.
    pub last_modified_at: SystemTime,
}

impl NotebookRecord {
    /// The record as the modelled `NotebookMetadata` shape a read answers with.
    pub fn to_notebook_metadata(&self) -> NotebookMetadata {
        NotebookMetadata {
            notebook_id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            work_group: Some(self.work_group.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            r#type: Some(self.notebook_type.clone()),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_at)),
        }
    }
}

/// Every notebook this account holds, and the import tokens that already minted one.
///
/// A struct of its own rather than loose fields on the store, so the count and the byte
/// budget above are applied in the ONE place a notebook is written
/// ([`NotebookRegistry::insert`]) and cannot be checked on one path and skipped on
/// another.
#[derive(Debug, Default)]
pub struct NotebookRegistry {
    /// The notebooks, keyed by `NotebookId`. A `BTreeMap` so a walk of the registry is in
    /// one stable order.
    notebooks: BTreeMap<String, NotebookRecord>,
    /// Which `NotebookId` each `ClientRequestToken` already minted, so the documented
    /// idempotency of an import ("ensure the request to import the notebook is idempotent
    /// (executes only once)") is answered from state rather than by importing twice.
    import_tokens: BTreeMap<String, String>,
}

impl NotebookRegistry {
    /// The notebook `id` names, or `None` when this account holds none.
    pub fn notebook(&self, id: &str) -> Option<&NotebookRecord> {
        self.notebooks.get(id)
    }

    /// The notebook a `ClientRequestToken` already minted, if it did.
    pub fn notebook_of_token(&self, token: &str) -> Option<&NotebookRecord> {
        self.notebooks.get(self.import_tokens.get(token)?)
    }

    /// Whether `work_group` already holds a notebook called `name`.
    ///
    /// `ImportNotebook` documents the rule this answers: "If an ipynb file with the same
    /// name already exists in the workgroup, throws an error" — so the name is unique
    /// within ONE workgroup, and the same name in another workgroup names another
    /// notebook. Case-sensitive, as `NotebookName` is.
    pub fn holds_name(&self, work_group: &str, name: &str) -> bool {
        self.notebooks
            .values()
            .any(|record| record.work_group == work_group && record.name == name)
    }

    /// How many notebooks are held, the count [`MAX_NOTEBOOKS`] bounds.
    pub fn len(&self) -> usize {
        self.notebooks.len()
    }

    /// How many notebooks `work_group` holds — the count a delete of that workgroup
    /// weighs, a notebook belonging to the workgroup it was imported into.
    pub fn len_in(&self, work_group: &str) -> usize {
        self.notebooks
            .values()
            .filter(|record| record.work_group == work_group)
            .count()
    }

    /// Remove every notebook `work_group` holds, as a recursive delete of that workgroup
    /// does, and answer with how many were removed.
    ///
    /// Any import token that named one goes with it: a token whose notebook is gone cannot
    /// answer with the same id, so it is not kept pointing at nothing.
    pub fn remove_work_group(&mut self, work_group: &str) -> usize {
        let removed: Vec<String> = self
            .notebooks
            .values()
            .filter(|record| record.work_group == work_group)
            .map(|record| record.id.clone())
            .collect();
        for id in &removed {
            self.notebooks.remove(id);
        }
        self.import_tokens
            .retain(|_, minted| !removed.iter().any(|id| id == minted));
        removed.len()
    }

    /// Whether no notebook is held at all.
    pub fn is_empty(&self) -> bool {
        self.notebooks.is_empty()
    }

    /// How many bytes of notebook content are held, the total
    /// [`MAX_NOTEBOOK_BYTES_TOTAL`] bounds.
    pub fn stored_bytes(&self) -> usize {
        self.notebooks
            .values()
            .fold(0usize, |bytes, record| {
                bytes.saturating_add(record.payload.len())
            })
    }

    /// Whether storing `payload_bytes` more would pass either bound.
    pub fn would_exceed_capacity(&self, payload_bytes: usize) -> bool {
        self.len() >= MAX_NOTEBOOKS
            || self.stored_bytes().saturating_add(payload_bytes) > MAX_NOTEBOOK_BYTES_TOTAL
    }

    /// Store `record`, under `token` when the request carried one.
    ///
    /// The caller has already asked [`Self::would_exceed_capacity`]; this is the one place
    /// a notebook enters the registry, so the token index cannot come to name a notebook
    /// the map does not hold.
    pub fn insert(&mut self, record: NotebookRecord, token: Option<String>) {
        if let Some(token) = token {
            self.import_tokens.insert(token, record.id.clone());
        }
        self.notebooks.insert(record.id.clone(), record);
    }
}

/// A fresh `NotebookId`: a version-4 UUID in its hyphenated, lower-case form, which is
/// exactly what the modelled `NotebookId` accepts
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). Minted from the
/// maintained crate rather than laid out here.
pub fn new_notebook_id() -> String {
    ::uuid::Uuid::new_v4().as_hyphenated().to_string()
}

/// Whether `record` is the Spark enabled workgroup a notebook can live in.
///
/// The engine version the workgroup selected is what decides
/// ([`PYSPARK_ENGINE_VERSION_PREFIX`]), and the three configuration members Athena
/// documents as Spark-only say the same thing where a request set them instead:
/// `ExecutionRole` ("This property applies only to Spark enabled workgroups"),
/// `AdditionalConfiguration` ("a user defined JSON string that is passed to the notebook
/// engine") and `EngineConfiguration` (the coordinator and executor DPU sizes a Spark
/// session runs on). A workgroup carrying none of them runs Athena SQL, and holds no
/// notebooks.
pub fn is_spark_enabled(record: &WorkGroupRecord) -> bool {
    let configuration = &record.configuration;
    let selected = configuration
        .engine_version
        .as_ref()
        .and_then(|version| version.selected_engine_version.as_deref())
        .unwrap_or_default();
    selected.starts_with(PYSPARK_ENGINE_VERSION_PREFIX)
        || configuration.execution_role.is_some()
        || configuration.additional_configuration.is_some()
        || configuration.engine_configuration.is_some()
}

/// The message a read or an export of a notebook no import or create minted answers with.
///
/// The form Athena words a missing notebook in, shared by every operation of this service
/// that addresses ONE notebook, so the same absent notebook reads the same way whichever
/// of them was called. The `NotebookId` the CALLER supplied is the sentence's one variable
/// part.
pub fn notebook_not_found(notebook_id: &str) -> String {
    format!("Notebook {notebook_id} was not found")
}

/// The message an import into a workgroup that already holds a notebook of that name is
/// refused with: the error `ImportNotebook` documents for it ("If an ipynb file with the
/// same name already exists in the workgroup, throws an error").
pub fn notebook_name_exists(work_group: &str, name: &str) -> String {
    format!("Notebook {name} already exists in WorkGroup {work_group}.")
}

/// The message an operation that needs a Spark enabled workgroup
/// ([`is_spark_enabled`]) is refused with when the workgroup it named runs Athena SQL.
/// Worded beside [`work_group_disabled`], the other per-workgroup refusal this emulator
/// words itself, so both name the addressed workgroup the same way.
pub fn work_group_not_spark_enabled(name: &str) -> String {
    format!("WorkGroup {name} is not a Spark enabled workgroup.")
}

/// The message an import that names neither source, or both, is refused with.
///
/// `ImportNotebook` documents the rule and the error: "the request must specify a value
/// for either Payload or NoteBookS3LocationUri. If neither is specified or both are
/// specified, an InvalidRequestException occurs."
pub const NOTEBOOK_SOURCE_REQUIRED: &str =
    "Either Payload or NotebookS3LocationUri must be specified, but not both.";

/// The message an import whose ipynb file is larger than Athena accepts is refused with:
/// the limit `ImportNotebook` publishes (see [`MAX_NOTEBOOK_PAYLOAD_BYTES`]), stated with
/// the size of the file the request named.
pub fn notebook_file_too_large(bytes: usize) -> String {
    format!(
        "The notebook file is {bytes} bytes. The maximum file size that can be imported \
         is {MAX_NOTEBOOK_PAYLOAD_BYTES} bytes."
    )
}

/// The message an import whose `NotebookS3LocationUri` names no object is refused with.
///
/// The member is "A URI that specifies the Amazon S3 location of a notebook file in ipynb
/// format", so a URI naming no object — a malformed one, one addressing a bucket rather
/// than a file, or one whose object the store does not hold — names no notebook to import.
pub fn notebook_file_not_found(uri: &str) -> String {
    format!("The notebook file at {uri} could not be read.")
}

/// The message an import whose object holds no ipynb text is refused with: an ipynb file
/// is a JSON document, so content that is not text at all — or no content — is not a
/// notebook.
pub fn notebook_file_not_ipynb(uri: &str) -> String {
    format!("The file at {uri} is not a notebook in ipynb format.")
}

/// What asking this emulator's own Amazon S3 for the file an import named established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotebookFile {
    /// The object the URI named, as the ipynb text it holds.
    Content(String),
    /// The URI names no object this emulator can read: it addresses no object at all, or
    /// the object store holds none there.
    Absent,
    /// The object is there and holds no ipynb text: content that is not text at all, or
    /// an object of no bytes, neither of which is a notebook file.
    NotText,
    /// The object is there and is larger than [`MAX_NOTEBOOK_PAYLOAD_BYTES`].
    TooLarge(usize),
}

/// The notebook file `uri` names, read from the ONE object store this emulator has.
///
/// A pure read of that store, and the content is what a `GetObject` of the same URI would
/// answer with — the CURRENT object at the key, so a key whose object was deleted again
/// reads as absent.
///
/// LOCK ORDER: this takes the object store's lock and holds no other while it does, and
/// hands back an owned value rather than a guard — so it must be called with NO guard of
/// this service's store held. Keeping the cross-service access one-directional and
/// guard-free is what makes a lock cycle between the two stores unreachable.
pub fn notebook_file_at<Eng: Send + Sync + 'static>(engine: &Eng, uri: &str) -> NotebookFile {
    let Some((bucket, key)) = s3_object_address(uri) else {
        return NotebookFile::Absent;
    };
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return NotebookFile::Absent;
    };
    let store = state.lock();
    let Some(object) = store
        .bucket(bucket)
        .and_then(|bucket| bucket.object(key))
    else {
        return NotebookFile::Absent;
    };
    if object.body.len() > MAX_NOTEBOOK_PAYLOAD_BYTES {
        return NotebookFile::TooLarge(object.body.len());
    }
    match ::std::str::from_utf8(&object.body) {
        // An object of no bytes holds no notebook either: an ipynb file is a JSON
        // document, and the modelled `Payload` length starts at 1.
        Ok("") | Err(_) => NotebookFile::NotText,
        Ok(content) => NotebookFile::Content(content.to_string()),
    }
}

/// The bucket and key an Amazon S3 URI addresses, or `None` when it addresses no object.
///
/// The three forms the modelled `S3Uri` pattern admits
/// (`^(https|s3|S3)://([^/]+)/?(.*)$`): the `s3://<bucket>/<key>` scheme in either case,
/// and the two HTTPS addressings of the same object — virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<key>`) and path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<key>`). A URI with no key under the
/// bucket addresses a bucket rather than a file and so addresses no object.
fn s3_object_address(uri: &str) -> Option<(&str, &str)> {
    let rest = ["s3://", "S3://", "https://"]
        .into_iter()
        .find_map(|scheme| uri.strip_prefix(scheme))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (bucket, key) = if uri.starts_with("https://") {
        // The LAST `.s3` label is the service one, so a bucket name that itself contains
        // `.s3` stays whole.
        match authority.rsplit_once(".s3") {
            // Virtual-hosted: the bucket is everything before the service label.
            Some((bucket, _)) if !bucket.is_empty() => (bucket, path),
            // Path-style: the bucket is the first segment of the path.
            _ => path.split_once('/').unwrap_or((path, "")),
        }
    } else {
        (authority, path)
    };
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}
