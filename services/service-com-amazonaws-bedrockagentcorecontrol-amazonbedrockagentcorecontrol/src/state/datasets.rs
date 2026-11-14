// SPDX-License-Identifier: CC0-1.0

//! The evaluation DATASET's state: one [`DatasetRecord`] per `datasetId`,
//! holding the configuration the create persisted, the examples its source
//! yielded into the DRAFT working copy, every version published from that copy,
//! and the transition out of `CREATING` the clock owes it.
//!
//! The examples live on the record rather than in a store of their own, because
//! an example is addressed only THROUGH the dataset that holds it
//! (`/datasets/{datasetId}/examples`) and is gone when the dataset is — the same
//! reason a gateway target lives inside its gateway. A published version
//! ([`DatasetVersionRecord`]) is held the same way, and for the same reason.
//!
//! The ingestion of a source lives here too ([`ingest_dataset_source`]): an
//! inline source carries its examples in the request, and an Amazon S3 source
//! names a JSONL file this emulator READS out of its own object storage, so the
//! number of examples a dataset reports is the number it really holds either
//! way. The SCHEMA check an inline batch has to pass before any of it is
//! persisted is here as well ([`validate_inline_examples`]), so the create and
//! any later write of examples cannot disagree about what an example must carry.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;
use ::serde_json::Value;

use crate::ops::types::{
    ConflictException, CreateDatasetInput, CreateDatasetOutput, CreateDatasetVersionOutput,
    DataSourceType, DatasetSummary, DatasetVersionSummary, GetDatasetOutput, UpdateDatasetOutput,
    ValidationException,
};

use super::{sorted_map, timestamp, VALIDATION_REASON_FIELD_VALIDATION_FAILED};

/// The ARN resource type a dataset carries — `DatasetArn` is
/// `…:bedrock-agentcore:{region}:{account}:dataset/{datasetId}` (`smithy_query
/// shape=DatasetArn`) — and the resource noun a missing dataset, or a missing
/// VERSION of one, is reported by.
const ARN_TYPE_DATASET: &str = "dataset";
pub const RESOURCE_DATASET: &str = "dataset";
pub const RESOURCE_DATASET_VERSION: &str = "dataset version";

/// `DatasetStatus` members this service drives. The enum's full value list is
/// `CREATING | UPDATING | DELETING | ACTIVE | CREATE_FAILED | UPDATE_FAILED |
/// DELETE_FAILED` (`smithy_query shape=DatasetStatus`), where `CREATING` is
/// "CreateDataset async ingestion in progress", `UPDATING` is "An async example
/// mutation or CreateDatasetVersion is in progress" — in both of which "All
/// writes are blocked" — and `ACTIVE` is "Dataset is stable".
pub const DATASET_STATUS_CREATING: &str = "CREATING";
pub const DATASET_STATUS_ACTIVE: &str = "ACTIVE";
pub const DATASET_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";
pub const DATASET_STATUS_UPDATING: &str = "UPDATING";
pub const DATASET_STATUS_DELETING: &str = "DELETING";

/// `DraftStatus` members a dataset reports. The enum's full value list is
/// `MODIFIED | UNMODIFIED` (`smithy_query shape=DraftStatus`): `MODIFIED` is
/// what a DRAFT with "changes not yet reflected in any published version, or no
/// versions … published yet" reports, and `UNMODIFIED` what one whose "DRAFT
/// content matches the latest published version exactly" reports.
pub const DRAFT_STATUS_MODIFIED: &str = "MODIFIED";
pub const DRAFT_STATUS_UNMODIFIED: &str = "UNMODIFIED";

/// The version a dataset read resolves to when the request names none: "Use the
/// `datasetVersion` query parameter to retrieve a specific version's metadata.
/// If absent, defaults to DRAFT" (`smithy_query doc shape=GetDataset`).
/// `DatasetVersion` is `^(DRAFT|[0-9]+)$`, so every other spelling is a
/// published version number.
pub const DATASET_VERSION_DRAFT: &str = "DRAFT";

/// How long a freshly created dataset stays `CREATING` before the clock owes it
/// the status its ingestion earned, and how long one stays `UPDATING` after a
/// version is published from it.
///
/// The create is asynchronous — "Returns immediately with status CREATING. Poll
/// GetDataset until status transitions to ACTIVE or CREATE_FAILED"
/// (`smithy_query doc shape=CreateDataset`) — and AWS documents no duration for
/// the transition, so the span is chosen: long enough that a caller polling
/// straight after the call still observes the transient status, short enough to
/// keep a client script quick.
pub const DATASET_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many datasets this emulator will hold.
///
/// `CreateDataset` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_DATASETS: usize = 1_000;

/// How many examples one ingestion will yield into a dataset.
///
/// An INLINE source is already bounded by the model — `DatasetExampleList` is
/// `length 1..1000` (`smithy_query shape=InlineExamplesSource`) — and the same
/// ceiling is applied to the examples read out of a JSONL file, so neither
/// source can grow this process's memory without bound.
pub const MAX_DATASET_EXAMPLES: usize = 1_000;

/// How many bytes of a caller's JSONL file this emulator reads, beside
/// [`MAX_DATASET_EXAMPLES`] and for the same reason.
pub const MAX_DATASET_FILE_BYTES: i64 = 32 * 1024 * 1024;

/// How deeply nested one example read out of a JSONL file may be.
///
/// The bound is this module's own: it keeps the conversion below from being
/// driven as deep as a caller's file asks. It is measured with an explicit
/// stack rather than by recursion, so measuring cannot itself overflow.
pub const MAX_EXAMPLE_DEPTH: usize = 64;

/// How many published versions this emulator will hold for one dataset.
///
/// `CreateDatasetVersion` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is the emulator's own
/// ceiling, so a caller looping on publish cannot grow this process's memory
/// without bound. It is lower than the ceilings on the resources beside it
/// because every version SNAPSHOTS the whole working copy
/// ([`MAX_DATASET_EXAMPLES`]), so the memory one dataset can hold is the product
/// of the two.
pub const MAX_VERSIONS_PER_DATASET: usize = 100;

/// `arn:aws:bedrock-agentcore:{region}:{account}:dataset/{datasetId}` —
/// `DatasetArn`'s pattern.
pub fn dataset_arn(region: &str, dataset_id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_DATASET}/{dataset_id}"),
    )
}

/// The field every example of an AgentCore evaluation dataset is keyed by: both
/// AgentCore schema types describe an example as a scenario — "each example is a
/// scenario used to generate full conversations" for the simulated schema, and
/// "pre-written inputs per conversation turn" for the predefined one, each turn
/// belonging to a scenario (`smithy_query doc shape=DatasetSchemaType
/// member=AGENTCORE_EVALUATION_SIMULATED_V1` /
/// `member=AGENTCORE_EVALUATION_PREDEFINED_V1`) — so an example that names no
/// scenario belongs to none.
const EXAMPLE_FIELD_SCENARIO_ID: &str = "scenario_id";

/// The field every example of a third-party evaluation dataset carries: that
/// schema "supports single-turn (string input) and multi-turn (message list
/// input) across third-party evaluation frameworks" (`smithy_query doc
/// shape=DatasetSchemaType member=THIRD_PARTY_EVALUATION_V1`), so the input is
/// the example.
const EXAMPLE_FIELD_INPUT: &str = "input";

/// `DatasetSchemaType` members, each of which "identifies both the source format
/// and the version of that format's schema"
/// (`crate::ops::types::DATASET_SCHEMA_TYPE_VALUES`).
pub const DATASET_SCHEMA_AGENTCORE_EVALUATION_PREDEFINED_V1: &str =
    "AGENTCORE_EVALUATION_PREDEFINED_V1";
pub const DATASET_SCHEMA_AGENTCORE_EVALUATION_SIMULATED_V1: &str =
    "AGENTCORE_EVALUATION_SIMULATED_V1";
pub const DATASET_SCHEMA_THIRD_PARTY_EVALUATION_V1: &str = "THIRD_PARTY_EVALUATION_V1";

/// The fields an example of `schema_type` must carry.
///
/// The schema type "identifies both the source format and the version of that
/// format's schema", so which fields an example needs is a property of the
/// schema and not of any one request.
pub fn required_example_fields(schema_type: &str) -> &'static [&'static str] {
    match schema_type {
        DATASET_SCHEMA_THIRD_PARTY_EVALUATION_V1 => &[EXAMPLE_FIELD_INPUT],
        // Both AgentCore evaluation schemas address an example as a scenario.
        _ => &[EXAMPLE_FIELD_SCENARIO_ID],
    }
}

/// The modelled validation error for an example that does not satisfy the
/// dataset's schema: which example in the submitted batch failed, and the field
/// of the schema it omits.
///
/// `index` is the zero-based position of the offending example in the batch the
/// request carried and `field` the schema field it does not carry, so the
/// sentence around them is fixed and both values come from the request.
pub fn example_schema_violation(index: usize, field: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Example at index {index} failed schema validation: Required field '{field}' is \
             missing from example content"
        ),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// Check every example a request carried INLINE against the dataset's schema,
/// answering the modelled refusal of the FIRST example that does not satisfy it.
///
/// Whole-batch, before anything is persisted: a batch is one commit, so one
/// example that fails the schema refuses the whole call and leaves no dataset
/// holding the examples that would have passed.
pub fn validate_inline_examples(
    schema_type: &str,
    examples: &[Document],
) -> Result<(), ValidationException> {
    let required = required_example_fields(schema_type);
    for (index, example) in examples.iter().enumerate() {
        // An example is "a free-form JSON document whose structure is defined by
        // the dataset's schema type": anything that is not a JSON object carries
        // no field of that structure at all, so it is missing the first one the
        // schema requires.
        let members = match example {
            Document::Object(members) => Some(members),
            _ => None,
        };
        for field in required {
            let present = members.is_some_and(|members| members.contains_key(*field));
            if !present {
                return Err(example_schema_violation(index, field));
            }
        }
    }
    Ok(())
}

/// Whether a dataset in `status` can serve a WRITE.
///
/// Which statuses block one is the model's own account of them: `CREATING` is
/// "CreateDataset async ingestion in progress. All writes are blocked",
/// `UPDATING` is "An async example mutation or CreateDatasetVersion is in
/// progress. All writes are blocked", and for a `DELETING` dataset on its way
/// out "Read operations are still allowed" — writes are not. A dataset is
/// stable in `ACTIVE`, where "All operations are allowed per operation-specific
/// guards" (`smithy_query shape=DatasetStatus`).
///
/// Shared by the handlers rather than written per operation, so no write path
/// can disagree about when a dataset is busy.
pub fn dataset_blocks_writes(status: &str) -> bool {
    status == DATASET_STATUS_CREATING
        || status == DATASET_STATUS_UPDATING
        || status == DATASET_STATUS_DELETING
}

/// The modelled conflict a dataset write gets while the dataset is in a status
/// that blocks it: the operation that was asked for, and the status it found.
///
/// The sentence is fixed; its variable parts are the asking operation's own name
/// and the dataset's current status, spelled as the wire's enum token. Shared by
/// the handlers so one refusal cannot be worded two ways.
pub fn dataset_status_conflict(operation: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Cannot perform operation {operation} when dataset is in {status} status"
        )),
    }
}

/// What ingesting a dataset's source established.
#[derive(Debug, Clone)]
pub enum DatasetIngestion {
    /// The examples the source yielded, in the order the source lists them.
    Ingested(Vec<Document>),
    /// Nothing could be ingested. The sentence is what the dataset reports as
    /// its `failureReason` once the clock owes it `CREATE_FAILED`.
    Failed(String),
}

/// The examples the `source` of a create yields, read where they live.
///
/// An inline source carries them in the request. An Amazon S3 source names a
/// JSONL file in the caller's own bucket, which is READ out of this emulator's
/// own object storage over the in-process callback, so the examples a dataset
/// holds are the ones the file really holds. A file object storage holds nothing
/// for — or one that is not a JSONL file of examples — is a failed ingestion:
/// "Initial ingestion failed. DRAFT record exists but contains no examples"
/// (`smithy_query shape=DatasetStatus`).
pub async fn ingest_dataset_source(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: &DataSourceType,
) -> DatasetIngestion {
    match source {
        DataSourceType::inlineExamples(inline) => {
            if inline.examples.len() > MAX_DATASET_EXAMPLES {
                return DatasetIngestion::Failed(too_many_examples_reason());
            }
            DatasetIngestion::Ingested(inline.examples.clone())
        }
        DataSourceType::s3Source(s3) => ingest_s3_file(ctx, region, &s3.s3_uri).await,
    }
}

/// The examples the JSONL file at `uri` holds.
async fn ingest_s3_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
) -> DatasetIngestion {
    let Some((bucket, key)) = object_address(uri) else {
        return DatasetIngestion::Failed(unreadable_reason(uri));
    };
    let answer = super::s3_client(ctx, region)
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await;
    let object = match answer {
        Ok(object) => {
            // The size of the file, bounded BEFORE its bytes are collected: what
            // object storage reports for the object is what reading it would
            // allocate.
            if object
                .content_length()
                .is_some_and(|length| length > MAX_DATASET_FILE_BYTES)
            {
                return DatasetIngestion::Failed(oversized_reason(uri));
            }
            object
        }
        Err(error) => {
            if !names_nothing(&error) {
                // The answer says nothing about the caller's object — an
                // emulator assembled without object storage answers the
                // router's own `UnknownService`, and a transport failure answers
                // nothing at all — so it is worth a warning. The caller's own
                // values are rendered in their escaped `Debug` form, so a key
                // carrying a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the dataset file could not be read from this emulator's object storage, so \
                     no examples were ingested",
                );
            }
            // Either way nothing was ingested, so the dataset reports the
            // ingestion as failed rather than an empty success.
            return DatasetIngestion::Failed(unreadable_reason(uri));
        }
    };
    let bytes = match object.body.collect().await {
        Ok(body) => {
            let bytes = body.into_bytes();
            if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > MAX_DATASET_FILE_BYTES {
                return DatasetIngestion::Failed(oversized_reason(uri));
            }
            bytes
        }
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                detail = %error,
                "the dataset file could not be read from this emulator's object storage, so no \
                 examples were ingested",
            );
            return DatasetIngestion::Failed(unreadable_reason(uri));
        }
    };
    let Ok(text) = ::std::str::from_utf8(&bytes) else {
        return DatasetIngestion::Failed(not_jsonl_reason(uri));
    };
    parse_jsonl_examples(uri, text)
}

/// The examples a JSONL file holds: one example per non-blank line, in the order
/// the file lists them, each line a JSON value — `DatasetExampleList`'s member
/// is a `document` (`smithy_query shape=DatasetExampleList`), so an example is
/// whatever JSON the schema type gives it.
fn parse_jsonl_examples(uri: &str, text: &str) -> DatasetIngestion {
    let mut examples: Vec<Document> = Vec::new();
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        // The line NUMBER a reader counts, so the sentence names the line of the
        // file.
        let number = index + 1;
        if examples.len() >= MAX_DATASET_EXAMPLES {
            return DatasetIngestion::Failed(too_many_examples_reason());
        }
        let Ok(value) = ::serde_json::from_str::<Value>(line) else {
            return DatasetIngestion::Failed(format!(
                "Line {number} of the dataset file at {uri} is not JSON. Each example must be a \
                 JSON value on its own line."
            ));
        };
        if depth_exceeded(&value) {
            return DatasetIngestion::Failed(format!(
                "Line {number} of the dataset file at {uri} is nested deeper than the \
                 {MAX_EXAMPLE_DEPTH} levels an ingestion reads. Flatten the example and create \
                 the dataset again."
            ));
        }
        examples.push(json_to_document(&value));
    }
    if examples.is_empty() {
        return DatasetIngestion::Failed(format!(
            "The dataset file at {uri} holds no examples. Each example must be a JSON value on \
             its own line."
        ));
    }
    DatasetIngestion::Ingested(examples)
}

/// Why a file this emulator could not read ingested nothing.
fn unreadable_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} could not be read, so no examples were ingested. Make sure \
         that the object exists and that the service has access to it, then create the dataset \
         again."
    )
}

/// Why a file larger than this emulator reads ingested nothing.
fn oversized_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} is larger than the {MAX_DATASET_FILE_BYTES} bytes an \
         ingestion reads. Split the file and create the dataset again."
    )
}

/// Why a file that is not JSONL text ingested nothing.
fn not_jsonl_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} is not a JSONL file: it is not UTF-8 text. Each example must \
         be a JSON value on its own line."
    )
}

/// Why a source carrying more examples than one ingestion yields ingested
/// nothing.
fn too_many_examples_reason() -> String {
    format!(
        "A dataset holds at most {MAX_DATASET_EXAMPLES} examples. Split the examples across \
         datasets and create the dataset again."
    )
}

/// The bucket and key an `S3Uri` names. The member's own pattern is
/// `^s3://[a-z0-9][a-z0-9.\-]{1,61}[a-z0-9]/.{1,1024}$` (`smithy_query
/// shape=S3Source`), so a value that reached a handler names a bucket and a
/// non-empty key under it; a value that does not is read as naming no object.
fn object_address(uri: &str) -> Option<(String, String)> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// Whether object storage's answer is object storage ITSELF saying it holds
/// nothing at the address the read named: the status and the codes it sends for
/// a missing bucket, key or version.
fn names_nothing<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    error
        .raw_response()
        .map(|response| response.status().as_u16())
        == Some(super::OBJECT_ABSENT_STATUS)
        && super::ABSENT_OBJECT_CODES.contains(&error.code().unwrap_or_default())
}

/// Whether `value` nests deeper than [`MAX_EXAMPLE_DEPTH`], walked with an
/// explicit stack so the measurement itself cannot overflow.
fn depth_exceeded(value: &Value) -> bool {
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((current, depth)) = pending.pop() {
        if depth > MAX_EXAMPLE_DEPTH {
            return true;
        }
        match current {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// One JSON value as the `document` an example is modelled as. The caller has
/// already established that the value nests no deeper than
/// [`MAX_EXAMPLE_DEPTH`], which bounds this walk.
fn json_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(flag) => Document::Bool(*flag),
        Value::Number(number) => Document::Number(json_number(number)),
        Value::String(text) => Document::String(text.clone()),
        Value::Array(items) => Document::Array(items.iter().map(json_to_document).collect()),
        Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), json_to_document(member)))
                .collect(),
        ),
    }
}

/// One JSON number as the document number form that holds it without loss: an
/// unsigned integer, a signed one, or a float for everything else.
fn json_number(number: &::serde_json::Number) -> ::aws_smithy_types::Number {
    use ::aws_smithy_types::Number;

    if let Some(value) = number.as_u64() {
        Number::PosInt(value)
    } else if let Some(value) = number.as_i64() {
        Number::NegInt(value)
    } else {
        Number::Float(number.as_f64().unwrap_or_default())
    }
}

/// One published version of a dataset: the numbered snapshot
/// `CreateDatasetVersion` publishes the working copy as.
///
/// "The DRAFT is preserved and remains editable after publishing", so a version
/// holds the examples the draft carried at the instant it was published rather
/// than a reference to the draft.
#[derive(Debug, Clone)]
pub struct DatasetVersionRecord {
    /// The version number, as `DatasetVersion` spells it: "a non-negative
    /// integer string representing a published version number".
    pub version: u32,
    /// The examples the draft held when this version was published.
    pub examples: Vec<Document>,
    pub created_at: SystemTime,
}

impl DatasetVersionRecord {
    /// How many examples this version holds, as the `Long` the responses
    /// declare.
    pub fn example_count(&self) -> i64 {
        i64::try_from(self.examples.len()).unwrap_or(i64::MAX)
    }

    /// This version as `ListDatasetVersions` reports it: the three members
    /// `DatasetVersionSummary` declares, off the snapshot itself.
    pub fn summary(&self) -> DatasetVersionSummary {
        DatasetVersionSummary {
            dataset_version: self.version.to_string(),
            example_count: self.example_count(),
            created_at: timestamp(self.created_at),
        }
    }
}

/// One dataset: the configuration `CreateDataset` persisted, the examples its
/// source yielded into the DRAFT working copy, every version published from that
/// copy, and the transition out of `CREATING` the clock owes it.
///
/// Every read of the dataset reports this record, so a create and a later read
/// cannot disagree about its name, its configuration, how many examples it holds
/// or its status.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub dataset_id: String,
    pub dataset_arn: String,
    pub dataset_name: String,
    pub description: Option<String>,
    /// Immutable after creation, as the model documents the member.
    pub schema_type: String,
    pub kms_key_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The DRAFT working copy's examples, in the order the source listed them.
    pub draft_examples: Vec<Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the dataset reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on: what its ingestion earned.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept
    /// on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// Why the settled status is a failure state, for a dataset whose ingestion
    /// failed; `None` for one that ingested its source.
    pub failure_reason: Option<String>,
    /// Every version published from the working copy, in the order they were
    /// published. Grown only through [`Self::publish_version`], which caps it at
    /// [`MAX_VERSIONS_PER_DATASET`].
    versions: Vec<DatasetVersionRecord>,
    /// Whether the working copy holds changes no published version reflects,
    /// which is what `draftStatus` reports. True for a dataset no version has
    /// been published from, false from the instant one is.
    draft_modified: bool,
}

impl DatasetRecord {
    /// The dataset a create stores for the request `requested` in `region`,
    /// under the id that create minted, holding what `ingestion` yielded.
    ///
    /// A new dataset is `CREATING`, not yet settled: the transition to the
    /// status its ingestion earned — `ACTIVE`, or `CREATE_FAILED` for a source
    /// that yielded nothing — is owed by the clock rather than by this call. No
    /// version has been published from it yet, so its working copy holds changes
    /// no version reflects.
    pub fn created(
        region: &str,
        dataset_id: String,
        requested: CreateDatasetInput,
        ingestion: DatasetIngestion,
        now: SystemTime,
    ) -> Self {
        let (draft_examples, settled_status, failure_reason) = match ingestion {
            DatasetIngestion::Ingested(examples) => {
                (examples, DATASET_STATUS_ACTIVE.to_string(), None)
            }
            DatasetIngestion::Failed(reason) => (
                // "DRAFT record exists but contains no examples."
                Vec::new(),
                DATASET_STATUS_CREATE_FAILED.to_string(),
                Some(reason),
            ),
        };
        Self {
            dataset_arn: dataset_arn(region, &dataset_id),
            dataset_id,
            dataset_name: requested.dataset_name,
            description: requested.description,
            schema_type: requested.schema_type,
            kms_key_arn: requested.kms_key_arn,
            tags: sorted_map(requested.tags),
            draft_examples,
            created_at: now,
            updated_at: now,
            transient_status: DATASET_STATUS_CREATING.to_string(),
            settled_status,
            settles_at: now + DATASET_SETTLE_DELAY,
            failure_reason,
            versions: Vec::new(),
            draft_modified: true,
        }
    }

    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// Why the dataset is in its current status, reported only once that status
    /// IS the failure state the clock owed it: a dataset still `CREATING` has
    /// nothing to report yet.
    pub fn failure_reason_at(&self, now: SystemTime) -> Option<String> {
        if now < self.settles_at {
            return None;
        }
        self.failure_reason.clone()
    }

    /// How many examples the DRAFT holds, as the `Long` the responses declare.
    pub fn example_count(&self) -> i64 {
        i64::try_from(self.draft_examples.len()).unwrap_or(i64::MAX)
    }

    /// What `draftStatus` reports: whether the working copy holds changes no
    /// published version reflects.
    pub fn draft_status(&self) -> &'static str {
        if self.draft_modified {
            DRAFT_STATUS_MODIFIED
        } else {
            DRAFT_STATUS_UNMODIFIED
        }
    }

    /// Every version published from this dataset, in the order they were
    /// published.
    pub fn versions(&self) -> &[DatasetVersionRecord] {
        &self.versions
    }

    /// The version a `datasetVersion` names, or `None` when this dataset holds
    /// no such published version.
    pub fn version(&self, version: u32) -> Option<&DatasetVersionRecord> {
        self.versions
            .iter()
            .find(|published| published.version == version)
    }

    /// Publish the working copy as this dataset's next numbered version,
    /// answering the number it was published as — or `None` when the dataset
    /// already holds [`MAX_VERSIONS_PER_DATASET`] of them — whereupon the caller
    /// answers the operation's modelled quota error rather than growing this
    /// process's memory. The ceiling lives here, at the ONE place a version is
    /// appended (`versions` is private for exactly that reason), so no publish
    /// path can skip it.
    ///
    /// "The DRAFT is preserved and remains editable after publishing", so the
    /// version is a snapshot of the examples the draft holds now and the draft
    /// itself is left in place — matching the version it was published as until
    /// something changes it again. The dataset reports `UPDATING` until the
    /// clock owes it `ACTIVE`: "Always UPDATING immediately after this call".
    pub fn publish_version(&mut self, now: SystemTime) -> Option<u32> {
        if self.versions.len() >= MAX_VERSIONS_PER_DATASET {
            return None;
        }
        // Versions are numbered from one, in publication order.
        let version = u32::try_from(self.versions.len() + 1).unwrap_or(u32::MAX);
        self.versions.push(DatasetVersionRecord {
            version,
            examples: self.draft_examples.clone(),
            created_at: now,
        });
        self.draft_modified = false;
        self.updated_at = now;
        self.transient_status = DATASET_STATUS_UPDATING.to_string();
        self.settled_status = DATASET_STATUS_ACTIVE.to_string();
        self.settles_at = now + DATASET_SETTLE_DELAY;
        // The publication succeeded, so nothing of an earlier failure is owed.
        self.failure_reason = None;
        Some(version)
    }

    /// The tags as a response reports them: the map the create carried, or no
    /// member at all for a dataset carrying none, so an untagged dataset is not
    /// reported as one holding an empty tag set.
    fn reported_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// Apply an update of this dataset's METADATA: "Only provided fields are
    /// updated; omitted fields remain unchanged", so a member the request does
    /// not carry is left exactly as it was rather than cleared.
    ///
    /// The update is SYNCHRONOUS, so the dataset keeps the status it is already
    /// in and the clock owes it no transition. It touches no example — "To
    /// modify dataset content, use AddDatasetExamples, UpdateDatasetExamples, or
    /// DeleteDatasetExamples" — so the working copy, every published version and
    /// the DRAFT's publish synchronization state are all left alone.
    pub fn apply_metadata_update(&mut self, description: Option<String>, now: SystemTime) {
        if description.is_some() {
            self.description = description;
        }
        self.updated_at = now;
    }

    /// What an update of this dataset reports: the three members
    /// `UpdateDatasetResponse` declares, every one of them off the record just
    /// written, so the `updatedAt` it answers is the one a later read reports.
    pub fn update_output(&self) -> UpdateDatasetOutput {
        UpdateDatasetOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            updated_at: timestamp(self.updated_at),
        }
    }

    /// What the create of this dataset reports: the four members
    /// `CreateDatasetResponse` declares, every one of them off the record just
    /// written.
    pub fn create_output(&self, now: SystemTime) -> CreateDatasetOutput {
        CreateDatasetOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            status: self.status_at(now).to_string(),
            created_at: timestamp(self.created_at),
        }
    }

    /// What the publication of `version` from this dataset reports.
    pub fn create_version_output(
        &self,
        version: &DatasetVersionRecord,
        now: SystemTime,
    ) -> CreateDatasetVersionOutput {
        CreateDatasetVersionOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            status: self.status_at(now).to_string(),
            dataset_version: version.version.to_string(),
            created_at: timestamp(version.created_at),
        }
    }

    /// What a read of this dataset reports, for the version the request resolved
    /// to: the DRAFT working copy, or one version published from it.
    ///
    /// Only the version's own members differ between the two — the identifier it
    /// is addressed by and the examples it holds; everything else is a property
    /// of the dataset.
    pub fn get_output(
        &self,
        version: Option<&DatasetVersionRecord>,
        now: SystemTime,
    ) -> GetDatasetOutput {
        GetDatasetOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            dataset_version: version.map_or_else(
                || DATASET_VERSION_DRAFT.to_string(),
                |published| published.version.to_string(),
            ),
            dataset_name: self.dataset_name.clone(),
            description: self.description.clone(),
            status: self.status_at(now).to_string(),
            // The DRAFT's publish synchronization state, which is a property of
            // the working copy whichever version was read.
            draft_status: Some(self.draft_status().to_string()),
            failure_reason: self.failure_reason_at(now),
            schema_type: self.schema_type.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            example_count: version
                .map_or_else(|| self.example_count(), DatasetVersionRecord::example_count),
            // "Presigned Amazon S3 URL to download the consolidated dataset
            // file for the resolved version. … Omitted if the file does not yet
            // exist": this emulator consolidates no file into object storage, so
            // there is no URL to publish and none is invented.
            download_url: None,
            download_url_expires_at: None,
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            tags: self.reported_tags(),
        }
    }

    /// This dataset as `ListDatasets` reports it: the members `DatasetSummary`
    /// declares, off the same record a read answers from.
    pub fn summary(&self, now: SystemTime) -> DatasetSummary {
        DatasetSummary {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            dataset_name: self.dataset_name.clone(),
            description: self.description.clone(),
            status: self.status_at(now).to_string(),
            draft_status: Some(self.draft_status().to_string()),
            schema_type: self.schema_type.clone(),
            example_count: self.example_count(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
        }
    }
}

impl super::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint a `datasetId` for `name` that no stored dataset holds: the name +
    /// `-` + 10 alphanumerics, which `DatasetId`'s pattern
    /// `^[a-zA-Z0-9_-]{1,110}$` admits for every `DatasetName`
    /// (`^[a-zA-Z][a-zA-Z0-9_]{0,47}$`) a create can carry.
    pub fn mint_dataset_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", super::suffix_for(name, self.mint_counter, at));
            if !self.datasets.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The dataset a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the dataset still exists.
    pub fn replayed_dataset_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let dataset_id = self
            .dataset_idempotency
            .get(&super::token_key(operation, client_token?))?;
        self.datasets
            .contains_key(dataset_id)
            .then(|| dataset_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once `MAX_REMEMBERED_CLIENT_TOKENS` are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_dataset_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        dataset_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        super::remember_token(
            &mut self.dataset_idempotency,
            &mut self.dataset_idempotency_order,
            super::token_key(operation, &token),
            dataset_id.to_string(),
        );
    }

    /// The (dataset, version number) a previous request to `operation` carrying
    /// this `clientToken` published, if the record is still held and the version
    /// it names still exists.
    pub fn replayed_dataset_version(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, u32)> {
        let (dataset_id, version) = self
            .dataset_version_idempotency
            .get(&super::token_key(operation, client_token?))?;
        self.datasets
            .get(dataset_id)
            .and_then(|record| record.version(*version))
            .map(|_| (dataset_id.clone(), *version))
    }

    /// Record which version a `clientToken` published for `operation`, evicting
    /// the oldest record once `MAX_REMEMBERED_CLIENT_TOKENS` are held, on the
    /// same rule as every other idempotency record of this service.
    pub fn remember_dataset_version_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        dataset_id: &str,
        version: u32,
    ) {
        let Some(token) = client_token else {
            return;
        };
        super::remember_token(
            &mut self.dataset_version_idempotency,
            &mut self.dataset_version_idempotency_order,
            super::token_key(operation, &token),
            (dataset_id.to_string(), version),
        );
    }
}
