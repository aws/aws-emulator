// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-textract-textract service, plus the one
//! document/`Block` model every read path derives its answer from.
//!
//! ## Why the derivation lives here
//!
//! Amazon Textract has four read paths over the SAME result — `DetectDocumentText`,
//! `AnalyzeDocument`, `GetDocumentTextDetection` and `GetDocumentAnalysis` — and the
//! synchronous and asynchronous answers for one document must agree. So the text of a
//! document is read once by [`read_submitted_document`] and every path derives its
//! blocks from that content with [`blocks`], keyed by the document's identity (an S3
//! object, or the SHA-256 of its inline bytes). `AnalyzeExpense`'s `ExpenseDocuments`
//! come from that same text and the same line layout, with [`expense_documents`].
//!
//! Inline bytes are identified by their own digest, so a document this service has
//! already read is the same document and its stored text is reported again. An S3 object
//! is identified by its LOCATION, whose bytes a later `PutObject` can replace, so its
//! current bytes are fetched on every call and nothing stale is ever reported.
//!
//! ## What this emulator can read, and what it refuses
//!
//! [`read_content`] is the ONLY source of a document's text, and it never invents one.
//! The bytes it reads are the request's own inline `Bytes` or the object this emulator's
//! Amazon S3 holds at the named location ([`s3_source`]), and a document is read only
//! when those bytes are in a format Amazon Textract processes on the channel they arrived
//! on ([`DocumentChannel`]):
//!
//! * bytes that are a raster in an accepted format — PNG or JPEG either way, and TIFF
//!   from an Amazon S3 object, which is the split AWS states for `Document$Bytes` — the
//!   pixels are decoded and the text recognised off them by [`ocr`], which reports the
//!   characters it measured and the confidence it measured them with, and reports nothing
//!   for ink it could not match to a glyph. A page with no ink is a page with no words,
//!   answered as such rather than refused;
//! * bytes no document can be read out of — in no document format at all, in an accepted
//!   format but truncated or otherwise undecodable, or in a document format the channel
//!   does not accept — refused as [`Unreadable::UnsupportedFormat`], which the handlers
//!   answer with the modelled `UnsupportedDocumentException` and the one sentence Amazon
//!   Textract states that refusal with ([`UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE`]). A PDF
//!   is answered there too: Textract accepts PDF, but this emulator has no page
//!   rasteriser and parses no PDF content stream, so it reads nothing out of one;
//! * an S3 object whose bytes Amazon S3 does not serve — refused as
//!   [`Unreadable::S3Unreadable`], which the handlers answer with the modelled
//!   `InvalidS3ObjectException`. Where Amazon S3 ESTABLISHED that it holds no such
//!   object, that is a fact about the request, so an asynchronous `Start*` refuses the
//!   submit with it and creates no job ([`DocumentRead::NotTakenOn`]); where nothing
//!   answered the read at all, nothing is established, so the job is created and ends in
//!   the modelled `FAILED` state carrying the reason ([`DocumentRead::Unreadable`]).
//!
//! Nothing here is derived from a digest, echoed from the request, or randomised: a
//! block only ever describes text that was actually read out of the document.
//!
//! ## Where a block's `Geometry` and `Confidence` come from
//!
//! Both are MEASUREMENTS of the part of the document the block was read from, never a
//! page template:
//!
//! * a page recognised off pixels reports, per line and per word, the extent of the ink
//!   [`ocr`] found it at and the mean score it matched that ink's glyphs with, so two
//!   words of one line, and one word on two documents, carry their own boxes and their
//!   own confidences;
//! * a line whose measured words do not line up with the words laid out for it has no
//!   measurement to give them, so its words are laid out on the character grid their own
//!   columns and lengths place them on ([`bounding_box`]) and report the page's own
//!   confidence — a per-word spread there would be a measurement nothing made;
//! * every block above a word spans exactly the words it was read from (a `LINE`, a
//!   `KEY_VALUE_SET`, a `CELL`, an `ExpenseDetection`), and a `PAGE` spans the page's
//!   CONTENT — every mark found on it, whether or not it was read as a character, so a
//!   rule, a border or a logo is part of the page it is drawn on — rather than claiming
//!   the whole page;
//! * a `WORD` reports as its `Geometry$RotationAngle` the quarter turn its characters were
//!   read at, which [`ocr`] establishes by reading the page at each of its turns;
//! * `BoundingBox` is "an axis-aligned COARSE representation of the location of the
//!   recognized item" and `Polygon` is, "within the bounding box, a FINE-GRAINED polygon
//!   around the recognized item", so a block reports both over the same marks: the box
//!   around them, and their own outline — the extreme marks themselves, clockwise
//!   from the corner the characters were read from — which follows the item's shape
//!   instead of restating the box's corners.

pub mod ocr;
pub mod s3_source;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AdapterOverview, AdapterVersionOverview, Block, BoundingBox, Document, DocumentLocation,
    DocumentMetadata, ExpenseCurrency, ExpenseDetection, ExpenseDocument, ExpenseField,
    ExpenseType, Geometry, InvalidParameterException, LineItemFields, LineItemGroup,
    NotificationChannel, OutputConfig, Point, QueriesConfig, Query, Relationship,
    ResourceNotFoundException, ValidationException, Warning,
    AUTO_UPDATE_VALUES, FEATURE_TYPE_VALUES,
};

/// The value Textract reports as `DetectDocumentTextModelVersion`.
pub const DETECT_DOCUMENT_TEXT_MODEL_VERSION: &str = "1.0";

/// The value Textract reports as `AnalyzeDocumentModelVersion`.
pub const ANALYZE_DOCUMENT_MODEL_VERSION: &str = "1.0";

/// How long after a `Start*` call this emulator makes the job's `IN_PROGRESS` ->
/// terminal transition due.
///
/// AWS documents no duration for asynchronous Textract jobs, so this is a CHOSEN span
/// inside the emulator's 250–750 ms window. It is never zero: a caller that polls
/// `Get*` immediately after `Start*` still observes `IN_PROGRESS`.
pub const JOB_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// The page size a paginated read uses when the caller passes no `MaxResults`.
pub const DEFAULT_MAX_RESULTS: usize = 1000;

/// The largest inline document this service accepts.
///
/// AWS, on `Document$Bytes`: "The maximum size of a document that's provided in a blob
/// of bytes is 5 MB." A larger one is refused with `DocumentTooLargeException`.
pub const MAX_INLINE_DOCUMENT_BYTES: usize = 5 * 1024 * 1024;

/// The largest document this service reads out of an S3 object.
///
/// AWS, on `DocumentTooLargeException`: "The maximum document size for synchronous
/// operations 10 MB." The size is checked against what Amazon S3 DECLARES the object holds
/// before any of its bytes are taken into this process ([`s3_source`]).
///
/// DELIBERATELY the same ceiling on the ASYNCHRONOUS path, where AWS publishes a much
/// larger one (500 MB for a PDF): this emulator reads a whole document into this process's
/// memory and recognises it there, so what bounds the read is what one request may make it
/// allocate and scan rather than what AWS can store. An object over the ceiling is refused
/// with the modelled `DocumentTooLargeException` — the error that says exactly why — rather
/// than read in part. A document between the two ceilings is therefore refused where AWS
/// would accept it: that is the trade this bound makes, and the only one that keeps the
/// read bounded by a constant instead of by the caller.
pub const MAX_S3_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;

/// The most pages one document may hold.
///
/// AWS's Textract hard limits cap a PDF or TIFF document at 3,000 pages.
pub const MAX_DOCUMENT_PAGES: usize = 3000;

/// An emulator-side ceiling on how many `Block` values one document may expand into.
///
/// NOT an AWS limit: it exists so a caller-supplied document cannot amplify one request
/// into unbounded CPU, resident memory and response bytes. A document that would exceed
/// it is refused with `DocumentTooLargeException` BEFORE any block is built, so the work
/// a single request can cause is bounded by this constant.
pub const MAX_BLOCKS_PER_DOCUMENT: usize = 50_000;

/// The share of a query's terms that must appear in a field's label for that field to be
/// the query's answer.
///
/// NOT an AWS constant: it is this emulator's retrieval threshold. Below it the document
/// holds no answer to the query, which is reported as a `QUERY` block with no `ANSWER`
/// relationship rather than as a guess.
pub const MIN_QUERY_MATCH: f32 = 0.5;

/// How many queries one SYNCHRONOUS `AnalyzeDocument` request may ask.
///
/// AWS's Textract quotas publish 15 queries per synchronous request, and a request asking
/// more is refused with `InvalidParameterException`. That ceiling also bounds the work a
/// request can cause here: answering a query costs one pass over the document's fields,
/// so an uncapped `QueriesConfig.Queries` would let one request choose both factors of
/// that product.
pub const MAX_QUERIES_PER_REQUEST: usize = 15;

/// How many queries one ASYNCHRONOUS `StartDocumentAnalysis` request may ask.
///
/// AWS's Textract quotas publish a higher per-request figure for the asynchronous
/// analysis than for the synchronous one (30 against 15), so the asynchronous path carries
/// its own ceiling rather than refusing input the real service accepts.
pub const MAX_QUERIES_PER_ASYNC_REQUEST: usize = 30;

/// How many `Query.Pages` entries one query may name. NOT an AWS limit; it bounds the
/// page filter a single caller-supplied query can make this service evaluate.
pub const MAX_PAGES_PER_QUERY: usize = 100;

/// Terms a label or a question is matched on.
///
/// NOT an AWS limit: it is what bounds retrieval. A label is split on every
/// non-alphanumeric character, so without this a single line of punctuation-separated
/// fragments would carry a term list as long as the document. Past it, a label or a
/// question is matched on its first terms, which is where what it is about is said.
pub const MAX_RETRIEVAL_TERMS: usize = 32;

/// How many documents this service keeps. NOT an AWS limit: an emulator-side bound so a
/// caller cannot grow the heap without end. The least recently registered is evicted.
pub const MAX_STORED_DOCUMENTS: usize = 256;

/// How many jobs this service keeps. NOT an AWS limit: see [`MAX_STORED_DOCUMENTS`].
/// The oldest job (and the idempotency entry that named it) is evicted.
pub const MAX_STORED_JOBS: usize = 256;

/// How many adapters this service keeps.
///
/// NOT an AWS value: an emulator-side ceiling so a caller cannot grow the heap without
/// end. Unlike the document and job stores, nothing here is evicted — an adapter is a
/// control-plane resource a later call must still be able to read — so a create that
/// would exceed the ceiling is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_STORED_ADAPTERS: usize = 1000;

/// The `AutoUpdate` an adapter carries when the caller names none.
///
/// AWS, on `CreateAdapter`: "You can choose whether or not the adapter should be
/// AutoUpdated with the AutoUpdate argument. By default, AutoUpdate is set to DISABLED."
pub const DEFAULT_AUTO_UPDATE: &str = "DISABLED";

/// The only `FeatureType` an adapter can be trained on.
///
/// AWS, on `CreateAdapterRequest$FeatureTypes`: "The type of feature that the adapter is
/// being trained on. Currrenly, supported feature types are: QUERIES".
pub const ADAPTER_FEATURE_TYPE: &str = "QUERIES";

/// Characters in a minted `AdapterId`, inside `AdapterId`'s modelled `12..1011` length.
const ADAPTER_ID_LEN: usize = 32;

/// The `ResourceNotFoundException` message the `AdapterId`-addressed adapter operations
/// answer for an id that names no adapter — one never created, or one already deleted.
///
/// ONE sentence for that one failure, whichever of those operations looked the adapter up,
/// and it carries nothing from the request: the id the caller sent is already in its own
/// request. Amazon Textract states this refusal as a SENTENCE on the path that takes the
/// id directly, terminal period included.
pub const ADAPTER_NOT_FOUND_MESSAGE: &str = "Adapter Id not found.";

/// The `ResourceNotFoundException` message the tagging operations answer when the resource
/// their `ResourceARN` names is an adapter this service holds none of.
///
/// The same words, and Amazon Textract states them WITHOUT the terminal period on this
/// path: resolving a `ResourceARN` is a different resolution from taking an `AdapterId`,
/// and the two are worded independently. Fixed text either way — the ARN and the id it
/// carries are already in the caller's own request, so neither reaches the error surface.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Adapter Id not found";

/// The message Amazon Textract states `UnsupportedDocumentException` with, for every
/// document it cannot read a page out of.
///
/// WHOLLY FIXED text for this refusal class: it names neither the format the document was
/// sent in nor the part of it that could not be read, so nothing of the request reaches
/// the error surface. It is also the `StatusMessage` of an asynchronous job that failed
/// for that reason, so one document answers the same sentence on both paths.
pub const UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE: &str = "Request has unsupported document format";

/// The message Amazon Textract states `InvalidParameterException` with for an input that
/// violates a modelled constraint — an absent required member, or a value outside a
/// declared length, pattern, range or enum.
///
/// WHOLLY FIXED text for this refusal class: it names neither the member nor the value,
/// so a rejected value is never echoed back to the caller.
pub const INVALID_PARAMETERS_MESSAGE: &str = "Request has invalid parameters";

/// The message Amazon Textract states `ValidationException` with for a creation-time
/// window whose bounds do not describe a range.
///
/// WHOLLY FIXED text for this refusal class: it names neither the bound that was wrong nor
/// the instants the request carried, so nothing of the request reaches the error surface.
pub const INVALID_TIME_RANGE_MESSAGE: &str = "Request has invalid time range.";

/// The message Amazon Textract states `InvalidJobIdException` with, for a `JobId` no
/// asynchronous `Start*` of this service issued.
///
/// WHOLLY FIXED text for this refusal class, and the same sentence whichever `Get*`
/// looked the job up: the id the caller sent is already in its own request, and
/// interpolating it would make the refusal a different string on every call.
pub const INVALID_JOB_ID_MESSAGE: &str = "Request has invalid Job Id";

/// The `AdapterVersionStatus` a version reports while its creation is still owed by the
/// clock.
///
/// From the modelled `AdapterVersionStatus` enum
/// (`ADAPTER_VERSION_STATUS_VALUES`): a version AWS has accepted but not finished
/// training reports `CREATION_IN_PROGRESS` until it settles.
pub const ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// The service segment every Amazon Textract ARN carries, as `engine::emulator_arn`'s
/// first argument.
pub const ARN_SERVICE: &str = "textract";

/// The resource-type segment of an adapter's ARN.
///
/// Amazon Textract writes an adapter's ARN
/// `arn:{partition}:textract:{region}:{account}:/adapters/{AdapterId}` — a resource
/// field of this type segment and the `AdapterId` after it, led by a `/`. No Textract
/// operation RETURNS that ARN (an adapter is addressed by its `AdapterId` everywhere
/// else), and the model puts no pattern on `AmazonResourceName`, so the form is the one
/// the service itself accepts for a `ResourceARN`. See [`adapter_arn`].
pub const ADAPTER_RESOURCE_TYPE: &str = "adapters";

/// The most tags one resource may hold.
///
/// `TagMap`'s modelled `0..200` length (`smithy_query(query="shape", shape="TagMap")`),
/// which bounds one REQUEST's map; the same ceiling bounds the resource, so a merge that
/// would leave it holding more pairs than a `TagMap` can express is refused.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// Characters in a pagination token issued by [`encode_page_token`].
const PAGE_TOKEN_LEN: usize = 32;

/// How many of those characters carry the offset, in hex, at the end of the token.
const PAGE_TOKEN_OFFSET_DIGITS: usize = 8;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-textract-textract. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTextractTextractState {
    /// Every readable document this service has been handed, keyed by
    /// [`DocumentRef::identity_key`]. Bounded by [`MAX_STORED_DOCUMENTS`].
    pub documents: HashMap<String, DocumentRecord>,
    /// Asynchronous jobs, keyed by `JobId`. Bounded by [`MAX_STORED_JOBS`].
    pub jobs: HashMap<String, Job>,
    /// `"<job kind>:<ClientRequestToken>"` -> `JobId`, backing `Start*` idempotency.
    pub idempotency: HashMap<String, String>,
    /// Monotonic counter mixed into a minted `JobId` so two starts of the same
    /// document never collide.
    pub next_job_seq: u64,
    /// Every adapter `CreateAdapter` has minted, keyed by its `AdapterId`. Bounded by
    /// [`MAX_STORED_ADAPTERS`]; nothing is evicted from it.
    pub adapters: HashMap<String, AdapterRecord>,
    /// `ClientRequestToken` -> `AdapterId`, backing `CreateAdapter` idempotency.
    ///
    /// Separate from [`Self::idempotency`], which names JOBS and is pruned when a job is
    /// evicted: an adapter token must keep resolving for as long as its adapter exists.
    pub adapter_idempotency: HashMap<String, String>,
    /// Monotonic counter mixed into a minted `AdapterId` so two creates of the same
    /// adapter name never collide.
    pub next_adapter_seq: u64,
}

impl ComAmazonawsTextractTextractState {
    /// The job a `ClientRequestToken` slot already names, and whether that job was
    /// started for the SAME request (same document, same `FeatureTypes`).
    ///
    /// One helper for both `Start*` handlers, so their idempotency semantics cannot
    /// drift apart: `Some((job_id, true))` is the idempotent answer, `Some((job_id,
    /// false))` is an `IdempotentParameterMismatchException`, `None` means the token is
    /// new.
    pub fn job_for_token(
        &self,
        slot: &str,
        document: &DocumentRef,
        feature_types: &[String],
    ) -> Option<(String, bool)> {
        self.idempotency
            .get(slot)
            .and_then(|job_id| self.jobs.get(job_id))
            .map(|job| {
                (
                    job.job_id.clone(),
                    job.document == *document && job.feature_types == *feature_types,
                )
            })
    }

    /// The stored content of `key`, if this service has already read that document.
    pub fn stored_content(&self, key: &str) -> Option<DocumentContent> {
        self.documents.get(key).map(|record| record.content.clone())
    }

    /// Store `content` for `reference` unless it is already stored, and hand back the
    /// stored copy.
    ///
    /// Idempotent: a second call with the same document returns the content stored by
    /// the first, which is what makes the sync and async answers for one document agree.
    pub fn register_document(
        &mut self,
        reference: DocumentRef,
        content: DocumentContent,
    ) -> DocumentContent {
        let key = reference.identity_key();
        if let Some(record) = self.documents.get(&key) {
            return record.content.clone();
        }
        // Bounded store: make room before inserting, oldest registration first.
        while self.documents.len() >= MAX_STORED_DOCUMENTS {
            let oldest = self
                .documents
                .iter()
                .min_by_key(|(_, record)| record.first_seen)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.documents.remove(&key);
                }
                None => break,
            }
        }
        self.documents.insert(
            key,
            DocumentRecord {
                reference,
                content: content.clone(),
                first_seen: SystemTime::now(),
            },
        );
        content
    }

    /// Store `job` under its own id, evicting the oldest job when the store is full.
    pub fn insert_job(&mut self, job: Job) {
        while self.jobs.len() >= MAX_STORED_JOBS {
            let oldest = self
                .jobs
                .iter()
                .min_by_key(|(_, job)| job.created_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.jobs.remove(&id);
                    // An idempotency entry naming an evicted job would answer with a
                    // JobId that no longer resolves, so it goes with the job.
                    self.idempotency.retain(|_, job_id| *job_id != id);
                }
                None => break,
            }
        }
        self.jobs.insert(job.job_id.clone(), job);
    }

    /// The adapter a `ClientRequestToken` already names, if that token has been used.
    ///
    /// A token whose adapter is gone resolves to `None`, so the token can be used again
    /// rather than answering with an `AdapterId` that no longer reads back.
    pub fn adapter_for_token(&self, token: &str) -> Option<&AdapterRecord> {
        self.adapter_idempotency
            .get(token)
            .and_then(|adapter_id| self.adapters.get(adapter_id))
    }

    /// Mint an `AdapterId`: [`ADAPTER_ID_LEN`] hex characters, inside `AdapterId`'s
    /// modelled `12..1011` length. Re-mints on the (practically impossible) collision
    /// rather than overwriting a live adapter.
    pub fn mint_adapter_id(&mut self, adapter_name: &str) -> String {
        loop {
            self.next_adapter_seq = self.next_adapter_seq.wrapping_add(1);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let digest = hex::encode(Sha256::digest(
                format!("{adapter_name}|{}|{nanos}", self.next_adapter_seq).as_bytes(),
            ));
            let adapter_id: String = digest.chars().take(ADAPTER_ID_LEN).collect();
            if !self.adapters.contains_key(&adapter_id) {
                return adapter_id;
            }
        }
    }

    /// Store `adapter` under its own id, and under its `ClientRequestToken` when it
    /// carried one, so a replay of that token answers with this adapter.
    pub fn insert_adapter(&mut self, adapter: AdapterRecord) {
        if let Some(token) = adapter.client_request_token.clone() {
            self.adapter_idempotency
                .insert(token, adapter.adapter_id.clone());
        }
        self.adapters.insert(adapter.adapter_id.clone(), adapter);
    }

    /// Remove the adapter `adapter_id` names, with every `ClientRequestToken` entry that
    /// resolved to it, and hand the removed record back.
    ///
    /// `None` when no adapter carries that id — which is how a delete answers the
    /// modelled `ResourceNotFoundException`. Dropping the token entries keeps
    /// [`adapter_idempotency`](Self::adapter_idempotency) from growing past the live
    /// adapters, and matches [`adapter_for_token`](Self::adapter_for_token): a token whose
    /// adapter is gone names nothing.
    pub fn remove_adapter(&mut self, adapter_id: &str) -> Option<AdapterRecord> {
        let removed = self.adapters.remove(adapter_id)?;
        self.adapter_idempotency.retain(|_, id| id != adapter_id);
        Some(removed)
    }

    /// Every stored adapter version in scope for a listing, oldest first.
    ///
    /// `adapter_id` scopes the listing to ONE adapter — `None` lists the versions of
    /// every adapter this service holds. `None` is ALSO the answer when `adapter_id`
    /// names an adapter that does not exist, which is how a listing of an unknown
    /// adapter reaches the modelled `ResourceNotFoundException` instead of reporting an
    /// empty list.
    ///
    /// The order is `(CreationTime, AdapterVersion, AdapterId)` ascending: AWS documents
    /// no order for an adapter-version listing, and a listing must be ordered by
    /// SOMETHING total and stable or an offset cursor could skip or repeat a version
    /// between pages.
    pub fn adapter_versions(&self, adapter_id: Option<&str>) -> Option<Vec<&AdapterVersionRecord>> {
        let mut versions: Vec<&AdapterVersionRecord> = match adapter_id {
            Some(adapter_id) => self.adapters.get(adapter_id)?.versions.values().collect(),
            None => self
                .adapters
                .values()
                .flat_map(|adapter| adapter.versions.values())
                .collect(),
        };
        versions.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.adapter_version.cmp(&right.adapter_version))
                .then_with(|| left.adapter_id.cmp(&right.adapter_id))
        });
        Some(versions)
    }

    /// The tags of the resource a tagging `ResourceARN` names, if this service holds it.
    ///
    /// The ONE place a parsed ARN becomes the tag map it addresses, so `TagResource`,
    /// `UntagResource` and `ListTagsForResource` cannot disagree about which ARNs name
    /// something here: the resource a tag write lands on is the resource a tag read
    /// reports.
    ///
    /// Every resource this service holds that can carry tags is an adapter, keyed by its
    /// `AdapterId`. `None` is a resource that is ABSENT rather than a request that is
    /// malformed — an adapter id no create minted, an adapter already deleted, an ARN of
    /// another Amazon Textract resource type, or an ARN carrying further path segments
    /// after the `AdapterId` (a nested resource, which arrives with that whole path as
    /// its identifier and so matches no stored adapter: an adapter VERSION is a resource
    /// this service mints none of while `CreateAdapterVersion` is a stub).
    pub fn resource_tags(&self, resource: &ArnResource) -> Option<&HashMap<String, String>> {
        if resource.resource_type != ADAPTER_RESOURCE_TYPE {
            return None;
        }
        self.adapters
            .get(&resource.resource_id)
            .map(|adapter| &adapter.tags)
    }

    /// [`Self::resource_tags`] for a caller that WRITES the tags — the same resolution,
    /// so a tag write and a tag read never disagree about which ARNs name something.
    pub fn resource_tags_mut(
        &mut self,
        resource: &ArnResource,
    ) -> Option<&mut HashMap<String, String>> {
        if resource.resource_type != ADAPTER_RESOURCE_TYPE {
            return None;
        }
        self.adapters
            .get_mut(&resource.resource_id)
            .map(|adapter| &mut adapter.tags)
    }

    /// Mint a `JobId`: 64 hex characters, within `JobId`'s `1..64` length and its
    /// `^[a-zA-Z0-9-_]+$` pattern.
    pub fn mint_job_id(&mut self, document_key: &str) -> String {
        self.next_job_seq = self.next_job_seq.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        hex::encode(Sha256::digest(
            format!("{document_key}|{}|{nanos}", self.next_job_seq).as_bytes(),
        ))
    }
}

/// The members of an adapter's configuration a mutation can change.
///
/// AWS, on `UpdateAdapter`: "Update the configuration for an adapter. FeatureTypes
/// configurations cannot be updated." — so this is exactly `AdapterName`, `Description`
/// and `AutoUpdate`, and neither `FeatureTypes` nor `Tags` belongs here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterConfiguration {
    pub adapter_name: String,
    pub description: Option<String>,
    /// `ENABLED` or `DISABLED`; [`DEFAULT_AUTO_UPDATE`] when the request named none.
    pub auto_update: String,
}

/// An adapter created by `CreateAdapter`, as `GetAdapter` reports it back.
#[derive(Debug, Clone)]
pub struct AdapterRecord {
    /// The id this service minted for the adapter. Its identity.
    pub adapter_id: String,
    /// The adapter's LIVE name: the create request's, with every later mutation applied.
    pub adapter_name: String,
    /// The adapter's LIVE description, or `None` while nothing has set one.
    pub description: Option<String>,
    /// The feature types the adapter is trained on, as the create request listed them.
    /// Immutable: `UpdateAdapter` cannot change a `FeatureTypes` configuration.
    pub feature_types: Vec<String>,
    /// The adapter's LIVE `AutoUpdate`: `ENABLED` or `DISABLED`.
    pub auto_update: String,
    pub tags: HashMap<String, String>,
    /// When this service created the adapter, reported as `CreationTime`. A mutation does
    /// not move it: the adapter is still the one that was created then.
    pub created_at: SystemTime,
    /// The `ClientRequestToken` the create carried, if any.
    pub client_request_token: Option<String>,
    /// The configuration the CREATE request asked for, kept as it was asked for even
    /// after a mutation changed the live one: `ClientRequestToken` idempotency compares
    /// create REQUESTS, so an adapter that was updated since must still answer a replay
    /// of its own create token with itself rather than with a parameter mismatch.
    pub created_with: AdapterConfiguration,
    /// Every version of THIS adapter, keyed by its `AdapterVersion`.
    ///
    /// A version is a child of its adapter — AWS, on `DeleteAdapter`: "Deletes an Amazon
    /// Textract adapter. Takes an AdapterId and deletes the adapter permanently, along
    /// with all adapter versions" — so it lives inside the adapter's record and goes
    /// away with it. A `BTreeMap` so a listing's order comes out the same on every read.
    pub versions: BTreeMap<String, AdapterVersionRecord>,
}

impl AdapterRecord {
    /// The record for a create request: its live configuration IS what the request asked
    /// for, and that same configuration is remembered as [`Self::created_with`].
    pub fn created(
        adapter_id: String,
        configuration: AdapterConfiguration,
        feature_types: Vec<String>,
        tags: HashMap<String, String>,
        created_at: SystemTime,
        client_request_token: Option<String>,
    ) -> Self {
        Self {
            adapter_id,
            adapter_name: configuration.adapter_name.clone(),
            description: configuration.description.clone(),
            feature_types,
            auto_update: configuration.auto_update.clone(),
            tags,
            created_at,
            client_request_token,
            created_with: configuration,
            // A newly created adapter has no versions: `CreateAdapterVersion` is what
            // adds one, so a create-then-list answers an empty version list.
            versions: BTreeMap::new(),
        }
    }

    /// Apply a mutation to this adapter: every member the request NAMED replaces the
    /// stored one, and every member it left out keeps its stored value.
    ///
    /// This is what makes a read after a write report the written values: the stored
    /// record is the one source every read derives its answer from.
    pub fn apply_update(
        &mut self,
        adapter_name: Option<String>,
        description: Option<String>,
        auto_update: Option<String>,
    ) {
        if let Some(adapter_name) = adapter_name {
            self.adapter_name = adapter_name;
        }
        if let Some(description) = description {
            self.description = Some(description);
        }
        if let Some(auto_update) = auto_update {
            self.auto_update = auto_update;
        }
    }

    /// Whether `other` describes the SAME create request as this adapter was made from.
    ///
    /// This is the comparison `IdempotentParameterMismatchException` is about: a reused
    /// token is idempotent only when every member that shapes the adapter agrees.
    /// `FeatureTypes` is compared as a SET, because the list is which features the
    /// adapter is trained on and not an ordering the resource keeps. The comparison is
    /// against the configuration each record was CREATED with, never the live one, so a
    /// mutation does not retroactively turn a replay into a mismatch.
    pub fn same_request_as(&self, other: &AdapterRecord) -> bool {
        self.created_with == other.created_with
            && self.tags == other.tags
            && feature_type_set(&self.feature_types) == feature_type_set(&other.feature_types)
    }
}

/// One version of an adapter, as `ListAdapterVersions` and `GetAdapterVersion` report it.
///
/// The reported `Status` is NOT stored: it is derived from [`Self::settles_at`] the way
/// [`Job`] derives a job's status, so a reporting call never has to write state to move a
/// version along. The operation that CREATES a version picks that due instant.
#[derive(Debug, Clone)]
pub struct AdapterVersionRecord {
    /// The adapter this version belongs to. Carried on the record so a listing across
    /// every adapter can report which adapter each version came from.
    pub adapter_id: String,
    /// This version's own identifier, unique within its adapter.
    pub adapter_version: String,
    /// When the version was created, reported as `CreationTime`.
    pub created_at: SystemTime,
    /// The feature types the version operates on — its adapter's, at creation.
    pub feature_types: Vec<String>,
    /// The instant the `CREATION_IN_PROGRESS` -> [`Self::settled_status`] transition
    /// became due. Before it, every read reports `CREATION_IN_PROGRESS`.
    pub settles_at: SystemTime,
    /// The `AdapterVersionStatus` the version settles into, one of the modelled
    /// `ADAPTER_VERSION_STATUS_VALUES`.
    pub settled_status: String,
    /// The `StatusMessage` that explains [`Self::settled_status`], if there is one. Not
    /// reported before the version settles: it describes the settled status.
    pub settled_status_message: Option<String>,
}

impl AdapterVersionRecord {
    /// The version's `Status` at `now`.
    pub fn status(&self, now: SystemTime) -> &str {
        if now >= self.settles_at {
            self.settled_status.as_str()
        } else {
            ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS
        }
    }

    /// The version's `StatusMessage` at `now`: the settled status's message, once the
    /// version has settled into it.
    pub fn status_message(&self, now: SystemTime) -> Option<String> {
        if now >= self.settles_at {
            self.settled_status_message.clone()
        } else {
            None
        }
    }

    /// This version as the modelled `AdapterVersionOverview`, as of `now`.
    ///
    /// ONE projection, so every operation that reports a version summary reports the
    /// same members from the same stored record.
    pub fn overview(&self, now: SystemTime) -> AdapterVersionOverview {
        AdapterVersionOverview {
            adapter_id: Some(self.adapter_id.clone()),
            adapter_version: Some(self.adapter_version.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            feature_types: Some(self.feature_types.clone()),
            status: Some(self.status(now).to_string()),
            status_message: self.status_message(now),
        }
    }
}

/// A `FeatureTypes` list as the set it denotes, so order and repetition do not make two
/// otherwise identical requests look different.
fn feature_type_set(feature_types: &[String]) -> BTreeSet<&str> {
    feature_types.iter().map(String::as_str).collect()
}

// ---------------------------------------------------------------------------
// Tagging: the ARN a tagging operation addresses its resource by
// ---------------------------------------------------------------------------

/// The resource an Amazon Textract ARN names: the region segment it carries, its
/// resource-type segment and the identifier after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArnResource {
    /// The region segment, as the ARN spells it (empty when the ARN carries none).
    /// Reported rather than matched on — see [`parse_resource_arn`].
    pub region: String,
    /// The resource type, e.g. [`ADAPTER_RESOURCE_TYPE`].
    pub resource_type: String,
    /// The resource's own identifier: everything after the type segment.
    pub resource_id: String,
}

/// The ARN of an adapter: `arn:aws:textract:{region}:{account}:/adapters/{AdapterId}`,
/// built through the one ARN helper so the account segment is the emulator's canonical
/// one.
///
/// No Amazon Textract operation RETURNS an adapter's ARN — an adapter is addressed by
/// its `AdapterId` everywhere except the tagging operations — so this is the form a
/// caller composes for a `ResourceARN` (see [`ADAPTER_RESOURCE_TYPE`]).
pub fn adapter_arn(region: &str, adapter_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/{ADAPTER_RESOURCE_TYPE}/{adapter_id}"),
    )
}

/// The resource a tagging `ResourceARN` names, or `None` when the value is not an
/// Amazon Textract resource ARN at all.
///
/// An ARN is the six colon-separated fields
/// `arn:{partition}:{service}:{region}:{account}:{resource}`, and this service's
/// resource field is `{type}/{id}`; anything else — too few fields, a service field
/// that is not this service's, a resource field with no type segment — names nothing
/// here. A leading `/` on the resource field is accepted and dropped: Amazon Textract
/// spells an adapter's resource field `/adapters/{AdapterId}`, and the slash is a
/// separator rather than part of the type, so both spellings address one resource.
///
/// The partition and account fields take no part in the lookup: this emulator serves a
/// single account, and a resource id already names at most one stored resource, so the
/// type and the id are what resolve it. The region is reported rather than matched on,
/// for a caller that needs the region an ARN was written for.
///
/// This is a SYNTAX test only, kept separate from the lookup on purpose: a value that is
/// not a Textract ARN at all is a bad parameter, which is a different answer from a
/// well-formed ARN naming no resource this service holds (see
/// [`ComAmazonawsTextractTextractState::resource_tags`]).
pub fn parse_resource_arn(arn: &str) -> Option<ArnResource> {
    // `splitn(6, ..)` keeps the resource field whole: it is the only field that may
    // itself contain a colon or a slash.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    let resource = resource.strip_prefix('/').unwrap_or(resource);
    let (resource_type, resource_id) = resource.split_once('/')?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some(ArnResource {
        region: region.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// The modelled refusal of an input that violates a constraint on this service's input —
/// an absent or inconsistent required member, a value outside a declared length, pattern,
/// range or enum, or a cursor this listing never issued.
///
/// ONE sentence for the whole class ([`INVALID_PARAMETERS_MESSAGE`]), whichever operation
/// and whichever member was refused: Amazon Textract names neither the member nor the
/// rule it broke, so nothing of the request reaches the error surface and the refusal is
/// the same string on every call.
pub fn invalid_parameters() -> InvalidParameterException {
    InvalidParameterException {
        message: Some(INVALID_PARAMETERS_MESSAGE.to_string()),
        code: None,
    }
}

/// The modelled refusal of a creation-time window that is not a range
/// ([`INVALID_TIME_RANGE_MESSAGE`]), as every listing taking an
/// `AfterCreationTime`/`BeforeCreationTime` pair answers it.
pub fn invalid_time_range() -> ValidationException {
    ValidationException {
        message: Some(INVALID_TIME_RANGE_MESSAGE.to_string()),
        code: None,
    }
}

/// Whether the creation-time window a listing request named is a RANGE: its lower bound
/// at or before its upper bound.
///
/// `AfterCreationTime` "[s]pecifies the lower bound" and `BeforeCreationTime` the upper
/// bound of the window a listing filters on, and both bounds are inclusive
/// ([`within_creation_window`]), so a lower bound AFTER the upper one describes no window
/// at all and is refused rather than answered with the empty listing it would select. A
/// window with one bound, or neither, is unbounded on that side and always a range.
pub fn creation_window_is_range(
    after_creation_time: Option<&::aws_smithy_types::DateTime>,
    before_creation_time: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    match (after_creation_time, before_creation_time) {
        (Some(after), Some(before)) => datetime_nanos(after) <= datetime_nanos(before),
        _ => true,
    }
}

/// The modelled refusal of a well-formed Amazon Textract ARN that names no resource
/// this service holds.
///
/// Every resource this service can tag is an adapter, so this is the one sentence all
/// three tagging operations answer with ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`]), whichever
/// of them resolved the ARN.
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        code: None,
    }
}

/// A document this service has read, and the text it holds.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The document's identity.
    pub reference: DocumentRef,
    /// Its text content, page by page.
    pub content: DocumentContent,
    /// When this service first read it. Read by the store's eviction policy.
    pub first_seen: SystemTime,
}

/// Which asynchronous operation started a job. A `Get*` of the other kind must not
/// serve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Started by `StartDocumentTextDetection`, read by `GetDocumentTextDetection`.
    TextDetection,
    /// Started by `StartDocumentAnalysis`, read by `GetDocumentAnalysis`.
    DocumentAnalysis,
}

impl JobKind {
    /// A stable name, used to scope `ClientRequestToken` idempotency per operation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextDetection => "TextDetection",
            Self::DocumentAnalysis => "DocumentAnalysis",
        }
    }
}

/// The terminal state a job reaches once the clock passes its due instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// The document was read and its blocks derived.
    Succeeded,
    /// The document could not be read; `Job::status_message` says why.
    Failed,
}

impl JobOutcome {
    /// The modelled `JobStatus` this outcome reports.
    pub const fn as_job_status(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// An asynchronous Textract job and its stored result.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub kind: JobKind,
    pub document: DocumentRef,
    pub feature_types: Vec<String>,
    pub client_request_token: Option<String>,
    pub job_tag: Option<String>,
    pub notification_channel: Option<NotificationChannel>,
    pub output_config: Option<OutputConfig>,
    pub kms_key_id: Option<String>,
    /// The terminal state the job settles into.
    pub outcome: JobOutcome,
    /// Why the job failed, for a `Failed` outcome.
    pub status_message: Option<String>,
    pub warnings: Vec<Warning>,
    /// When the job was started. Read by the job store's eviction policy.
    pub created_at: SystemTime,
    /// The instant the `IN_PROGRESS` -> terminal transition became due.
    pub settles_at: SystemTime,
    pub pages: i32,
    /// The job's whole result, computed once when the job was started.
    pub blocks: Vec<Block>,
    pub model_version: String,
}

impl Job {
    /// The job's status at `now`.
    ///
    /// Derived from the stored due instant rather than from a stored status, so a
    /// reporting call never has to write state to move the job along.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.settles_at {
            self.outcome.as_job_status()
        } else {
            "IN_PROGRESS"
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTextractTextractState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTextractTextractState>)
}

/// The Region a request carrying no SigV4 credential scope, or one that names no Region,
/// is served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The Region the request was signed for, or [`DEFAULT_REGION`].
///
/// The scope is caller-supplied and this value reaches the address an in-process read of
/// Amazon S3 is sent to, so a value that is not shaped like a Region identifier is not
/// reflected: the real service would not have served the request under a Region it does
/// not have.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region identifier: two or more
/// hyphen-separated lowercase-letter parts followed by a digit group (`us-east-1`,
/// `ap-southeast-3`, `us-gov-west-1`, `cn-north-1`).
fn is_region_like(region: &str) -> bool {
    let Some((parts, index)) = region.rsplit_once('-') else {
        return false;
    };
    region.len() <= 32
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && parts.split('-').count() >= 2
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
}

// ---------------------------------------------------------------------------
// Document identity
// ---------------------------------------------------------------------------

/// An S3 object, as a document source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Ref {
    pub bucket: String,
    pub name: String,
    pub version: Option<String>,
}

/// The stable identity of a submitted document: an S3 object, or the digest of the
/// inline bytes. Everything stored or derived for a document keys off this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentRef {
    pub s3: Option<S3Ref>,
    pub bytes_digest: Option<String>,
}

impl DocumentRef {
    /// The document's identity as one string.
    pub fn identity_key(&self) -> String {
        match (&self.s3, &self.bytes_digest) {
            (Some(s3), _) => match &s3.version {
                Some(version) => format!("s3://{}/{}?versionId={version}", s3.bucket, s3.name),
                None => format!("s3://{}/{}", s3.bucket, s3.name),
            },
            (None, Some(digest)) => format!("bytes:sha256:{digest}"),
            // Not constructible: both constructors below refuse an empty document.
            (None, None) => "bytes:sha256:none".to_string(),
        }
    }
}

/// The identity of a synchronous `Document`.
///
/// Textract requires exactly one of `Bytes` and `S3Object`: neither (and, equally,
/// both) violates the input constraint `InvalidParameterException` describes. `Err`
/// carries the sentence that refusal is stated with ([`INVALID_PARAMETERS_MESSAGE`]).
pub fn reference_for_document(document: &Document) -> Result<DocumentRef, String> {
    match (document.bytes.as_deref(), document.s3_object.as_ref()) {
        (None, None) | (Some(_), Some(_)) => Err(INVALID_PARAMETERS_MESSAGE.to_string()),
        (Some(bytes), None) => Ok(DocumentRef {
            s3: None,
            bytes_digest: Some(hex::encode(Sha256::digest(bytes))),
        }),
        (None, Some(s3)) => Ok(DocumentRef {
            s3: Some(s3_ref(
                s3.bucket.as_deref(),
                s3.name.as_deref(),
                s3.version.as_deref(),
            )?),
            bytes_digest: None,
        }),
    }
}

/// The identity of an asynchronous `DocumentLocation`, which is always an S3 object.
pub fn reference_for_location(location: &DocumentLocation) -> Result<DocumentRef, String> {
    let s3 = location
        .s3_object
        .as_ref()
        .ok_or_else(|| INVALID_PARAMETERS_MESSAGE.to_string())?;
    Ok(DocumentRef {
        s3: Some(s3_ref(
            s3.bucket.as_deref(),
            s3.name.as_deref(),
            s3.version.as_deref(),
        )?),
        bytes_digest: None,
    })
}

fn s3_ref(
    bucket: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> Result<S3Ref, String> {
    let bucket = bucket.filter(|b| !b.is_empty());
    let name = name.filter(|n| !n.is_empty());
    match (bucket, name) {
        (Some(bucket), Some(name)) => Ok(S3Ref {
            bucket: bucket.to_string(),
            name: name.to_string(),
            version: version.filter(|v| !v.is_empty()).map(ToString::to_string),
        }),
        (None, _) | (_, None) => Err(INVALID_PARAMETERS_MESSAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Feature types
// ---------------------------------------------------------------------------

/// Which of the requested `FeatureTypes` change the derived block set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureSet {
    /// `FORMS`: add `KEY_VALUE_SET` blocks.
    pub forms: bool,
    /// `TABLES`: add `TABLE` and `CELL` blocks.
    pub tables: bool,
}

/// Why a `FeatureTypes` list cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureError {
    /// The list is empty, which no analysis can satisfy.
    Empty,
    /// A value that is not a `FeatureType` at all.
    Unknown(String),
    /// A modelled `FeatureType` this emulator does not derive blocks for yet.
    NotDerived(String),
}

/// Read a `FeatureTypes` list.
///
/// `FORMS`, `TABLES` and `QUERIES` are derived; the other modelled values (`SIGNATURES`,
/// `LAYOUT`) are reported as [`FeatureError::NotDerived`] so the caller answers 501 rather
/// than a 200 that silently omits the blocks they asked for.
///
/// `QUERIES` needs no flag of its own: the `QUERY` and `QUERY_RESULT` blocks come from the
/// queries themselves, which [`queries_of`] reads off the request, so there is no second
/// representation of "QUERIES was asked for" to drift from it.
pub fn parse_features(values: &[String]) -> Result<FeatureSet, FeatureError> {
    if values.is_empty() {
        return Err(FeatureError::Empty);
    }
    let mut features = FeatureSet::default();
    for value in values {
        match value.as_str() {
            "FORMS" => features.forms = true,
            "TABLES" => features.tables = true,
            QUERIES_FEATURE => {}
            // `FEATURE_TYPE_VALUES` is the FeatureType enum from the Smithy model.
            other if FEATURE_TYPE_VALUES.contains(&other) => {
                return Err(FeatureError::NotDerived(other.to_string()))
            }
            other => return Err(FeatureError::Unknown(other.to_string())),
        }
    }
    Ok(features)
}

/// The `FeatureType` that asks for the queries `QueriesConfig` supplies.
pub const QUERIES_FEATURE: &str = "QUERIES";

/// The message Amazon Textract states `InvalidParameterException` with when
/// `QueriesConfig` is supplied and the requested `FeatureTypes` do not ask for
/// [`QUERIES_FEATURE`].
///
/// WHOLLY FIXED text for this refusal, down to its trailing period: it names the pair
/// that must be supplied together and interpolates nothing from the request.
pub const QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE: &str =
    "QueriesConfig must be used with QUERIES FeatureType.";

/// The queries an analysis must answer, or why the request cannot be served.
///
/// AWS pairs the `QUERIES` feature with the `QueriesConfig` member: `QueriesConfig` is
/// what the queries are supplied in and `QUERIES` is what asks for them, so one without
/// the other violates the input constraint `InvalidParameterException` describes.
/// `QueriesConfig` without the feature is answered with the sentence AWS states that
/// refusal with ([`QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE`]); every other violation of
/// this input's constraints is answered with the one sentence the class is stated with
/// ([`INVALID_PARAMETERS_MESSAGE`]), which carries nothing from the request.
///
/// Read straight off the requested `FeatureTypes`, so the pairing is checked whatever else
/// that list holds — including a modelled feature this emulator derives no blocks for,
/// whose 501 must not stand in front of a bad request.
///
/// Each query's `Query.Pages` is parsed ONCE here, so no caller-supplied page entry is
/// re-parsed while the document is searched, and both caller-controlled counts are bounded
/// (`max_queries`, which the caller passes as the quota for ITS path, and
/// [`MAX_PAGES_PER_QUERY`]): the work a query answer costs is `queries x fields`, and
/// neither factor may be chosen without limit.
pub fn queries_of(
    feature_types: &[String],
    config: Option<&QueriesConfig>,
    max_queries: usize,
) -> Result<Vec<PreparedQuery>, String> {
    let queries_requested = feature_types.iter().any(|value| value == QUERIES_FEATURE);
    match (queries_requested, config) {
        (true, Some(config)) => {
            if config.queries.len() > max_queries {
                return Err(INVALID_PARAMETERS_MESSAGE.to_string());
            }
            config.queries.iter().map(PreparedQuery::new).collect()
        }
        (false, None) => Ok(Vec::new()),
        (false, Some(_)) => Err(QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE.to_string()),
        (true, None) => Err(INVALID_PARAMETERS_MESSAGE.to_string()),
    }
}

/// Whether a derived block list plus the blocks `queries` would add exceeds
/// [`MAX_BLOCKS_PER_DOCUMENT`].
///
/// A `QUERY` block, and the `QUERY_RESULT` that may answer it, are blocks the response
/// carries, so they count against the same ceiling the document's own blocks are checked
/// against. The caller answers `true` with `DocumentTooLargeException`.
pub fn exceeds_block_ceiling(pages: &[String], queries: usize) -> bool {
    estimated_block_count(pages).saturating_add(queries.saturating_mul(2))
        > MAX_BLOCKS_PER_DOCUMENT
}

/// A query as it was asked, with its page filter parsed once.
#[derive(Debug, Clone)]
pub struct PreparedQuery {
    /// The query the caller asked, as a `QUERY` block reports it.
    pub query: Query,
    /// The pages it asks about.
    pages: PageFilter,
}

/// The pages a query asks about, read out of `Query.Pages` once.
#[derive(Debug, Clone)]
enum PageFilter {
    /// `Pages` absent: page 1, which is what AWS documents as its default.
    FirstPage,
    /// `*`: every page of the document.
    EveryPage,
    /// The pages and intervals the caller named; `None` is the open end of `N-*`.
    Intervals(Vec<(u32, Option<u32>)>),
}

impl PreparedQuery {
    /// Read one query, or refuse a `Pages` that is not one of the forms AWS documents.
    ///
    /// AWS, on `Query$Pages`: the only characters allowed are `0 1 2 3 4 5 6 7 8 9 - *`,
    /// with no whitespace; `*` must be the only element when used; intervals such as `1-3`
    /// and `4-*` are allowed; a page must be greater than 0. `Err` carries the one
    /// sentence a constraint violation is refused with ([`INVALID_PARAMETERS_MESSAGE`]),
    /// so no entry of the caller's list is quoted back.
    fn new(query: &Query) -> Result<Self, String> {
        let pages = match query.pages.as_deref() {
            None => PageFilter::FirstPage,
            Some(entries) if entries.len() == 1 && entries[0] == "*" => PageFilter::EveryPage,
            Some(entries) => {
                if entries.len() > MAX_PAGES_PER_QUERY {
                    return Err(INVALID_PARAMETERS_MESSAGE.to_string());
                }
                let mut intervals = Vec::with_capacity(entries.len());
                for entry in entries {
                    if entry == "*" {
                        return Err(INVALID_PARAMETERS_MESSAGE.to_string());
                    }
                    let interval = parse_page_interval(entry)
                        .ok_or_else(|| INVALID_PARAMETERS_MESSAGE.to_string())?;
                    intervals.push(interval);
                }
                PageFilter::Intervals(intervals)
            }
        };
        Ok(Self {
            query: query.clone(),
            pages,
        })
    }

    /// Whether this query asks about `page_number`.
    fn covers(&self, page_number: i32) -> bool {
        let Ok(page_number) = u32::try_from(page_number) else {
            return false;
        };
        match &self.pages {
            PageFilter::FirstPage => page_number == 1,
            PageFilter::EveryPage => true,
            PageFilter::Intervals(intervals) => intervals.iter().any(|(first, last)| {
                page_number >= *first && last.is_none_or(|last| page_number <= last)
            }),
        }
    }
}

/// `(first, last)` of a `Query.Pages` entry, `last` absent for an open interval.
/// `None` when the entry is not a page or an interval at all.
fn parse_page_interval(entry: &str) -> Option<(u32, Option<u32>)> {
    let page = |value: &str| value.parse::<u32>().ok().filter(|page| *page > 0);
    match entry.split_once('-') {
        None => page(entry).map(|only| (only, Some(only))),
        Some((first, "*")) => page(first).map(|first| (first, None)),
        Some((first, last)) => {
            let (first, last) = (page(first)?, page(last)?);
            (first <= last).then_some((first, Some(last)))
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter inputs
// ---------------------------------------------------------------------------

/// Read a `FeatureTypes` list as an ADAPTER's feature set.
///
/// A narrower rule than [`parse_features`], which reads the list an analysis request
/// carries: an adapter can only be trained on the feature types the service supports for
/// adapters, which is [`ADAPTER_FEATURE_TYPE`] alone. `Err` carries the one sentence a
/// constraint violation is refused with ([`INVALID_PARAMETERS_MESSAGE`]), which the
/// caller answers with the modelled `InvalidParameterException`.
pub fn validate_adapter_features(feature_types: &[String]) -> Result<(), String> {
    if feature_types.is_empty() {
        return Err(INVALID_PARAMETERS_MESSAGE.to_string());
    }
    for value in feature_types {
        if value != ADAPTER_FEATURE_TYPE {
            return Err(INVALID_PARAMETERS_MESSAGE.to_string());
        }
    }
    Ok(())
}

/// Read a mutation's members as the configuration change it asks for.
///
/// AWS, on `UpdateAdapter`: "Update the configuration for an adapter. FeatureTypes
/// configurations cannot be updated. At least one new parameter must be specified as an
/// argument." So a request naming none of the three updatable members asks for no change
/// at all and is refused; `Err` carries the one sentence a constraint violation is
/// refused with ([`INVALID_PARAMETERS_MESSAGE`]), which the caller answers with the
/// modelled `InvalidParameterException`. The members' own length, pattern and enum
/// constraints were already enforced at the boundary.
pub fn validate_adapter_update(
    adapter_name: Option<&str>,
    description: Option<&str>,
    auto_update: Option<&str>,
) -> Result<(), String> {
    if adapter_name.is_none() && description.is_none() && auto_update.is_none() {
        return Err(INVALID_PARAMETERS_MESSAGE.to_string());
    }
    if let Some(auto_update) = auto_update {
        adapter_auto_update(Some(auto_update))?;
    }
    Ok(())
}

/// The `AutoUpdate` an adapter carries for `value`, defaulting per
/// [`DEFAULT_AUTO_UPDATE`] when the request named none.
///
/// `Err` refuses a value outside the modelled `AutoUpdate` enum with the one sentence a
/// constraint violation is stated with ([`INVALID_PARAMETERS_MESSAGE`]), which the
/// caller answers with the modelled `InvalidParameterException`.
pub fn adapter_auto_update(value: Option<&str>) -> Result<String, String> {
    match value {
        None => Ok(DEFAULT_AUTO_UPDATE.to_string()),
        // `AUTO_UPDATE_VALUES` is the AutoUpdate enum from the Smithy model.
        Some(value) if AUTO_UPDATE_VALUES.contains(&value) => Ok(value.to_string()),
        Some(_) => Err(INVALID_PARAMETERS_MESSAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Reading a document
// ---------------------------------------------------------------------------

/// One page as [`ocr`] MEASURED it off the submitted pixels: its lines in reading order,
/// the angle its characters were read at, and the extent of the page's own content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeasuredPage {
    /// Where the text of this page sits on it, one entry per line of the page's text and
    /// in the same order.
    pub lines: Vec<ocr::MeasuredLine>,
    /// The angle, in degrees, the page's characters were read at — `0.0` for a page read
    /// as it was submitted — which is what its `WORD` blocks report as
    /// `Geometry$RotationAngle`.
    pub rotation: f32,
    /// The extent of the page's CONTENT: every mark found on it, read as text or not.
    /// `None` for a page carrying no ink at all.
    pub content: Option<ocr::Marks>,
}

/// A document's text, one entry per page, the confidence it was read with, and where on
/// each page that text was found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentContent {
    pub pages: Vec<String>,
    /// The `Confidence` a block derived from this text reports when nothing finer was
    /// measured for it: the mean score the recogniser measured over the whole page.
    pub confidence: f32,
    /// Where the text of each page sits on it, index-aligned with [`Self::pages`].
    ///
    /// A page's text is RECOGNISED off its pixels, so this is where each line and word of
    /// it was measured. A line whose measured words do not line up with the words laid
    /// out for it is laid out on the character grid [`bounding_box`] computes from the
    /// line's own columns and word lengths instead.
    pub measured: Vec<MeasuredPage>,
}

impl DocumentContent {
    /// The measurements of page `page_index`, when that page's text was recognised off
    /// pixels.
    fn measured_page(&self, page_index: usize) -> Option<&MeasuredPage> {
        self.measured.get(page_index)
    }

    /// The angle the characters of page `page_index` were read at.
    ///
    /// A page whose text was not recognised off pixels carries no rotation: the characters
    /// of a text document are not drawn on a page at all, so nothing turned them.
    fn page_rotation(&self, page_index: usize) -> f32 {
        self.measured_page(page_index)
            .map_or(0.0, |page| page.rotation)
    }

    /// The page count `DocumentMetadata.Pages` reports.
    pub fn page_count(&self) -> i32 {
        i32::try_from(self.pages.len()).unwrap_or(i32::MAX)
    }

    /// `DocumentMetadata` for this document.
    pub fn metadata(&self) -> DocumentMetadata {
        DocumentMetadata {
            pages: Some(self.page_count()),
        }
    }
}

/// Why a document's text cannot be read. Each variant has exactly one modelled answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    /// No document can be read out of the bytes: they are in no format this emulator
    /// reads, or in one of the accepted ones but truncated or otherwise undecodable.
    /// Answered with `UnsupportedDocumentException`.
    ///
    /// ONE variant for both, because the service draws no distinction between them: a
    /// document whose bytes a decoder cannot read is a document in an unsupported format,
    /// and carries no reason of its own — so this variant holds no message, and the
    /// detail of what failed to decode is logged rather than answered.
    UnsupportedFormat,
    /// The document is an S3 object whose bytes this emulator cannot obtain.
    /// Answered with `InvalidS3ObjectException`.
    S3Unreadable(String),
    /// The document, or the block list it would expand into, is over a documented or
    /// emulator-side limit. Answered with `DocumentTooLargeException`.
    TooLarge(String),
}

impl Unreadable {
    /// The reason, as a message for the modelled error or a job's `StatusMessage`.
    pub fn message(&self) -> &str {
        match self {
            Self::UnsupportedFormat => UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
            Self::S3Unreadable(message) | Self::TooLarge(message) => message,
        }
    }
}

/// Why a document's text could not be read, including the case where the read itself did
/// not run to completion.
#[derive(Debug)]
pub enum ReadFailure {
    /// The document: answer with the modelled error [`Unreadable`] names.
    Unreadable(Unreadable),
    /// The read did not complete — a panic or a cancellation on the blocking pool. This
    /// says nothing about the document, so it is answered as the server fault it is.
    DidNotComplete,
}

/// Read the text of the document `reference` names, OFF the async worker.
///
/// Reading a raster is bounded (see [`ocr`]) but CPU-bound: the decode, the binarisation
/// and the recognition walk every pixel of a page the caller chose the size of. So the
/// read runs on the BLOCKING pool, where it occupies no async worker that serves health
/// checks and other requests, and the `spawn_blocking(..).await` is a real await point a
/// timeout, a cancellation or a shutdown can land on.
///
/// `bytes` are the document's own bytes, whichever source they came from — the request's
/// inline `Bytes`, or the S3 object [`s3_source::document_bytes`] fetched.
///
/// This is how a request path reads a document; [`read_content`] itself is the
/// synchronous core, for a caller that is already off the async worker.
pub async fn read_document(
    reference: DocumentRef,
    bytes: Option<Vec<u8>>,
) -> Result<DocumentContent, ReadFailure> {
    let read =
        ::tokio::task::spawn_blocking(move || read_content(&reference, bytes.as_deref())).await;
    match read {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(reason)) => Err(ReadFailure::Unreadable(reason)),
        Err(error) => {
            ::tracing::error!(error = %error, "reading the document did not complete");
            Err(ReadFailure::DidNotComplete)
        }
    }
}

/// What reading a submitted document established — the one outcome every handler of this
/// service decides its answer from.
#[derive(Debug)]
pub enum DocumentRead {
    /// The document's text.
    Read(DocumentContent),
    /// Amazon Textract cannot take the document ON, and the reason is a fact this
    /// service ESTABLISHED about the request before any of the document was processed:
    /// Amazon S3 answered that it holds no such object, or the object it holds is over a
    /// documented size limit. A synchronous operation answers the modelled error for the
    /// reason, and so does an asynchronous `Start*` — which creates no job, because AWS
    /// validates the location and the size of an asynchronous document at submit time.
    NotTakenOn(Unreadable),
    /// The document was taken on and its text could not be read out of it. A synchronous
    /// operation answers the modelled error for the reason; an asynchronous job is
    /// created and ends in the modelled `FAILED` state carrying it.
    Unreadable(Unreadable),
    /// The read did not complete — a panic or a cancellation on the blocking pool, which
    /// says nothing about the document and is answered as the server fault it is.
    DidNotComplete,
}

/// Read the text of a submitted document: the ONE path every handler of this service
/// reads a `Document` or a `DocumentLocation` through.
///
/// The bytes come from the request's inline `Bytes` when it carried them, and otherwise
/// from the object Amazon S3 holds at the location the request named
/// ([`s3_source::document_bytes`]) — read over the emulator's own in-process transport,
/// so an object a caller staged with `PutObject` is the one Textract reads. From there
/// the two sources are decoded and recognised exactly alike ([`read_content`]), so one
/// document answers the same whichever way it was submitted.
///
/// Inline bytes are keyed by their own digest, so a document already read is the same
/// document and its stored text is reported again. An S3 object's bytes can be replaced
/// by a later `PutObject`, so its current bytes are fetched every time and no stale text
/// is reported for it.
pub async fn read_submitted_document(
    state: &::engine::ServiceState<ComAmazonawsTextractTextractState>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    reference: &DocumentRef,
    inline: Option<Vec<u8>>,
) -> DocumentRead {
    if reference.s3.is_none() {
        if let Some(content) = state.lock().stored_content(&reference.identity_key()) {
            return DocumentRead::Read(content);
        }
    }
    let bytes = match s3_source::document_bytes(ctx, reference, inline).await {
        s3_source::DocumentBytes::Read(bytes) => bytes,
        s3_source::DocumentBytes::Established(reason) => return DocumentRead::NotTakenOn(reason),
        s3_source::DocumentBytes::Unestablished(reason) => return DocumentRead::Unreadable(reason),
    };
    match read_document(reference.clone(), Some(bytes)).await {
        Ok(content) => {
            if reference.s3.is_some() {
                return DocumentRead::Read(content);
            }
            DocumentRead::Read(state.lock().register_document(reference.clone(), content))
        }
        // The SIZE of a document is a fact about it that holds before its text is read,
        // which is why AWS refuses an oversized document at submit time; every other
        // reason is what reading the document established.
        Err(ReadFailure::Unreadable(reason @ Unreadable::TooLarge(_))) => {
            DocumentRead::NotTakenOn(reason)
        }
        Err(ReadFailure::Unreadable(reason)) => DocumentRead::Unreadable(reason),
        Err(ReadFailure::DidNotComplete) => DocumentRead::DidNotComplete,
    }
}

/// Read the text of the document `reference` names.
///
/// The ONLY source of a document's text. It extracts what is really there or refuses;
/// it never derives, guesses or fabricates content. See the module docs for the cases.
///
/// CPU-bound for a raster: a request path reaches it through [`read_document`], which
/// runs it on the blocking pool.
pub fn read_content(
    reference: &DocumentRef,
    bytes: Option<&[u8]>,
) -> Result<DocumentContent, Unreadable> {
    // No bytes for a document whose source is an S3 object means the object's bytes were
    // never fetched, so Textract has nothing of the document to read. Only
    // `s3_source::document_bytes` fetches them, and it reports WHY it could not.
    let Some(bytes) = bytes else {
        return Err(Unreadable::S3Unreadable(
            s3_source::UNREADABLE_OBJECT_MESSAGE.to_string(),
        ));
    };
    // A document provided as inline bytes and one read out of an S3 object carry
    // different published ceilings, so each is held to its own.
    let channel = DocumentChannel::of(reference);
    let (ceiling, ceiling_source) = match channel {
        DocumentChannel::S3Object => (MAX_S3_DOCUMENT_BYTES, "read from an Amazon S3 object"),
        DocumentChannel::Inline => (MAX_INLINE_DOCUMENT_BYTES, "provided as inline bytes"),
    };
    if bytes.len() > ceiling {
        return Err(Unreadable::TooLarge(format!(
            "the document is {} bytes; the maximum size of a document {ceiling_source} is \
             {ceiling} bytes",
            bytes.len()
        )));
    }
    // A document is read only when its bytes are in a format Amazon Textract processes on
    // the channel they were submitted on, and the format comes from the bytes' own magic
    // numbers rather than from anything the request declared about them. A page the
    // recogniser finds no text on is a page with no words, not a document in an
    // unsupported format.
    if let Some(format) = ocr::raster_format(bytes, channel) {
        let page = ocr::read_raster(bytes, format)?;
        return Ok(DocumentContent {
            pages: bounded(vec![page.text])?,
            confidence: page.confidence,
            measured: vec![MeasuredPage {
                lines: page.lines,
                rotation: page.rotation,
                content: page.content,
            }],
        });
    }
    // Every other byte string is a document this service reads no page out of, and they
    // all get the ONE refusal the service states for that: bytes in no document format at
    // all, bytes in a document format this channel does not take, and a PDF — which
    // Textract itself accepts, while this emulator reads nothing out of one, because
    // rendering a PDF page needs a page rasteriser it does not have and the text a PDF
    // carries sits in content streams nothing here parses. WHICH it was is logged rather
    // than answered: the refusal names neither the format the document was sent in nor
    // the part of it that could not be read.
    ::tracing::debug!(
        declared = declared_format(bytes),
        channel = ?channel,
        "the document is in no format this service reads on the channel it arrived on"
    );
    Err(Unreadable::UnsupportedFormat)
}

/// The name of the format these bytes declare, for the log of a refused document: the
/// raster container `image`'s own sniffing recognises, a PDF by its file header, or
/// `"unrecognised"` for bytes that declare no format this emulator knows at all.
///
/// Reported to the emulator's operator only. It is never part of an answer: a caller is
/// told the one sentence Amazon Textract states for a document it cannot read, which
/// names no format.
fn declared_format(bytes: &[u8]) -> &'static str {
    if let Some(raster) = ocr::declared_raster_format(bytes) {
        return raster;
    }
    if bytes.starts_with(PDF_MAGIC) {
        return "PDF";
    }
    "unrecognised"
}

/// The bytes every PDF document starts with ("%PDF-", per the PDF file header).
const PDF_MAGIC: &[u8] = b"%PDF-";

/// HOW a document was submitted, which decides the formats it may be in.
///
/// AWS states the accepted formats per channel: `Document$Bytes` is "a blob of
/// base64-encoded document bytes … The document bytes must be in PNG or JPEG format",
/// while a document named as an Amazon S3 object (`Document$S3Object`, or the
/// `DocumentLocation` of an asynchronous `Start*`) may be in any of the formats the
/// operations name — "PNG, JPEG, PDF, or TIFF format".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentChannel {
    /// The request's own inline `Document$Bytes`.
    Inline,
    /// An object of Amazon S3.
    S3Object,
}

impl DocumentChannel {
    /// The channel the document `reference` names was submitted on.
    pub fn of(reference: &DocumentRef) -> Self {
        if reference.s3.is_some() {
            Self::S3Object
        } else {
            Self::Inline
        }
    }
}

/// `pages` unchanged, or the reason they are over a documented or emulator-side limit.
fn bounded(pages: Vec<String>) -> Result<Vec<String>, Unreadable> {
    if pages.len() > MAX_DOCUMENT_PAGES {
        return Err(Unreadable::TooLarge(format!(
            "the document has {} pages; the maximum is {MAX_DOCUMENT_PAGES}",
            pages.len()
        )));
    }
    let estimated = estimated_block_count(&pages);
    if estimated > MAX_BLOCKS_PER_DOCUMENT {
        return Err(Unreadable::TooLarge(format!(
            "the document would expand into about {estimated} blocks; this emulator derives at \
             most {MAX_BLOCKS_PER_DOCUMENT} blocks for one document"
        )));
    }
    Ok(pages)
}

/// An upper bound on the number of `Block` values these pages expand into, over EVERY
/// feature set: one PAGE per page; per line one LINE, at most two `KEY_VALUE_SET` and at
/// most one TABLE; per word one WORD and at most one CELL; plus one CELL per tab.
///
/// Checked before any block is built, so [`blocks`] does work bounded by
/// [`MAX_BLOCKS_PER_DOCUMENT`] whatever the caller submits.
fn estimated_block_count(pages: &[String]) -> usize {
    let mut total = pages.len();
    for page in pages {
        for line in page.lines().filter(|line| !line.trim().is_empty()) {
            total = total.saturating_add(4);
            total = total.saturating_add(line.split_whitespace().count().saturating_mul(2));
            total = total.saturating_add(line.matches('\t').count());
            if total > MAX_BLOCKS_PER_DOCUMENT {
                return total;
            }
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Block derivation
// ---------------------------------------------------------------------------

/// Left and top margin of the character grid a TEXT document is laid out on, as a
/// page-relative fraction. Only a document whose text carries no measured geometry (see
/// [`DocumentContent::measured`]) is laid out here.
const PAGE_MARGIN: f32 = 0.05;
/// Width of one character cell of that grid, as a page-relative fraction.
const CHAR_WIDTH: f32 = 0.0075;
/// Height of a line of that grid, as a page-relative fraction.
const LINE_HEIGHT: f32 = 0.018;
/// Baseline-to-baseline distance of that grid, as a page-relative fraction.
const LINE_PITCH: f32 = 0.024;

/// A word of a line, with the character column it starts at.
struct WordLayout {
    column: usize,
    text: String,
    id: String,
    /// Where the word sits on the page: its own ink, measured, when it was recognised off
    /// pixels, and otherwise its place on the character grid.
    region: Region,
    /// The `Confidence` a block derived from this word reports: the mean score ITS OWN
    /// characters were recognised with, or the document's one confidence when they were
    /// not recognised but read.
    confidence: f32,
}

/// A line of a page: its text, its words, where it sits, and how analysis reads it.
struct LineLayout {
    id: String,
    index: usize,
    text: String,
    words: Vec<WordLayout>,
    /// The `Confidence` a block derived from the line as a whole reports: the mean score
    /// the characters of THIS LINE were recognised with, or the page's own confidence
    /// where the line itself carries no measurement, since a per-line spread there would
    /// be a measurement nothing made.
    confidence: f32,
    /// Where the line sits on the page: the ink it was read from when it was recognised
    /// off pixels, and otherwise its place on the character grid.
    region: Region,
    /// The angle, in degrees, this page's characters were read at, which is the frame
    /// every polygon on the page is outlined in.
    rotation: f32,
    /// Word counts per tab-separated cell, when the line is a table row.
    cells: Vec<usize>,
    /// Index in [`Self::words`] of each cell's FIRST word, index-aligned with
    /// [`Self::cells`].
    ///
    /// The running sum of `cells`, taken once when the row is laid out: a cell's words
    /// are then found by its own offset instead of by re-adding every cell before it, so
    /// reading a row costs time proportional to its cells rather than to their square —
    /// which one line of a caller's document can hold many thousands of.
    cell_starts: Vec<usize>,
    /// Character column of the `:` that separates a label from its value.
    colon: Option<usize>,
}

/// A deterministic block id, rendered in the 8-4-4-4-12 form Textract's ids take.
///
/// Derived from the document's identity and the block's position in it, so the four read
/// paths over one document name the same block by the same id.
fn block_id(key: &str, kind: &str, path: &[usize]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.update(b"|");
    hasher.update(kind.as_bytes());
    for part in path {
        hasher.update(format!("|{part}").as_bytes());
    }
    let d = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &d[0..8],
        &d[8..12],
        &d[12..16],
        &d[16..20],
        &d[20..32]
    )
}

/// Where a line or a word of a TEXT document sits, on the character grid the document's
/// own lines, columns and word lengths place it on.
///
/// The fallback layout for a line that carries no measured geometry — one whose measured
/// words do not line up with the words laid out for it — so its geometry is computed from
/// the text itself rather than borrowed from another word's rectangle. A line measured on
/// the page reports instead the rectangle [`ocr`] measured for it.
fn bounding_box(line_index: usize, column: usize, chars: usize) -> BoundingBox {
    // The two bounds are computed independently and never derived from each other:
    // `f32::clamp` panics when its min exceeds its max, and rounding makes `1.0 - left`
    // fall just under `CHAR_WIDTH` once `left` saturates — which any line past the
    // page's character ceiling reaches, from a single caller-supplied document.
    let left = (PAGE_MARGIN + column as f32 * CHAR_WIDTH).clamp(0.0, 1.0 - CHAR_WIDTH);
    let top = (PAGE_MARGIN + line_index as f32 * LINE_PITCH).clamp(0.0, 1.0 - LINE_HEIGHT);
    let max_width = (1.0 - left).max(0.0);
    let width = (chars as f32 * CHAR_WIDTH).max(0.0).min(max_width);
    BoundingBox {
        width,
        height: LINE_HEIGHT,
        left,
        top,
    }
}

/// A measured rectangle as the `BoundingBox` a block reports.
fn box_of(rect: &ocr::Rect) -> BoundingBox {
    BoundingBox {
        width: rect.width,
        height: rect.height,
        left: rect.left,
        top: rect.top,
    }
}

/// The four corners of `bbox`, clockwise from its top-left one.
fn corners(bbox: &BoundingBox) -> Vec<Point> {
    vec![
        Point {
            x: bbox.left,
            y: bbox.top,
        },
        Point {
            x: bbox.left + bbox.width,
            y: bbox.top,
        },
        Point {
            x: bbox.left + bbox.width,
            y: bbox.top + bbox.height,
        },
        Point {
            x: bbox.left,
            y: bbox.top + bbox.height,
        },
    ]
}

/// How many quarter turns a page whose characters are turned by `rotation` degrees was
/// read at, as an index into a list of corners running clockwise from the top-left.
fn read_from_corner(rotation: f32) -> usize {
    (((rotation / 90.0).round() as i32).rem_euclid(4)) as usize
}

/// The outline of the marks at `points`: the four extreme ones — the topmost, the
/// rightmost, the bottommost and the leftmost — in the order the item they bound was
/// read.
///
/// `Geometry$Polygon` is, "within the bounding box, a fine-grained polygon around the
/// recognized item", against a `BoundingBox` that is "an axis-aligned COARSE
/// representation" of the same place: so the polygon is taken from the marks themselves.
/// Each of these four points is one of them, which puts the polygon inside the box around
/// the same marks and on each of its edges, and lets it follow the item's own skew — the
/// marks of a line whose glyphs do not all reach its top, or of a page photographed out of
/// square, outline a slanted quadrilateral, while a solid rectangle of ink outlines its own
/// rectangle.
///
/// The list runs clockwise from the corner the item's characters were READ from, which for
/// a page read as it was submitted is its top-left one and for a page read at a quarter
/// turn is the corner that turn started at.
fn outline(points: &[Point], rotation: f32) -> Vec<Point> {
    if points.is_empty() {
        return Vec::new();
    }
    let (mut top, mut right, mut bottom, mut left) = (0usize, 0usize, 0usize, 0usize);
    for (index, point) in points.iter().enumerate() {
        let (x, y) = (point.x, point.y);
        if y < points[top].y || (y == points[top].y && x < points[top].x) {
            top = index;
        }
        if x > points[right].x || (x == points[right].x && y < points[right].y) {
            right = index;
        }
        if y > points[bottom].y || (y == points[bottom].y && x > points[bottom].x) {
            bottom = index;
        }
        if x < points[left].x || (x == points[left].x && y > points[left].y) {
            left = index;
        }
    }
    // Clockwise from the extreme mark nearest the page's top-left corner, turned to start
    // at the corner the characters were read from.
    let clockwise = [top, right, bottom, left];
    let first = read_from_corner(rotation);
    (0..clockwise.len())
        .map(|step| points[clockwise[(step + first) % clockwise.len()]].clone())
        .collect()
}

/// WHERE a block was measured: the extent of the marks it spans, and their outline.
///
/// The one place a block's `Geometry` comes from, so the coarse box and the fine-grained
/// polygon a block reports are always measurements of the SAME marks.
#[derive(Debug, Clone)]
struct Region {
    bbox: BoundingBox,
    /// The polygon the block reports, as [`outline`] orders it.
    polygon: Vec<Point>,
}

impl Region {
    /// The region [`ocr`] measured for one item off the submitted pixels.
    fn measured(marks: &ocr::Marks, rotation: f32) -> Self {
        let points: Vec<Point> = marks
            .outline
            .iter()
            .map(|vertex| Point {
                x: vertex.x,
                y: vertex.y,
            })
            .collect();
        Self {
            bbox: box_of(&marks.rect),
            polygon: outline(&points, rotation),
        }
    }

    /// The region a box is the whole of what was measured of: a block laid out on the
    /// character grid, or one bounded by the blocks around it, has no finer outline than
    /// that box's own corners.
    fn of_box(bbox: BoundingBox, rotation: f32) -> Self {
        let polygon = outline(&corners(&bbox), rotation);
        Self { bbox, polygon }
    }

    /// The region spanning every one of `regions`: the box around them all, outlined by
    /// the marks they were themselves measured over.
    fn spanning<'a>(
        regions: impl IntoIterator<Item = &'a Self>,
        rotation: f32,
    ) -> Option<Self> {
        let mut boxes: Vec<BoundingBox> = Vec::new();
        let mut points: Vec<Point> = Vec::new();
        for region in regions {
            boxes.push(region.bbox.clone());
            points.extend(region.polygon.iter().cloned());
        }
        Some(Self {
            bbox: union_of(&boxes)?,
            polygon: outline(&points, rotation),
        })
    }

    /// This region as the `Geometry` a block reports it.
    fn geometry(&self) -> Geometry {
        Geometry {
            bounding_box: Some(self.bbox.clone()),
            polygon: Some(self.polygon.clone()),
            rotation_angle: None,
        }
    }
}

/// The box enclosing every one of `boxes`, or `None` when there are none.
fn union_of(boxes: &[BoundingBox]) -> Option<BoundingBox> {
    if boxes.is_empty() {
        return None;
    }
    Some(union(boxes))
}

fn union(boxes: &[BoundingBox]) -> BoundingBox {
    let left = boxes.iter().map(|b| b.left).fold(f32::MAX, f32::min);
    let top = boxes.iter().map(|b| b.top).fold(f32::MAX, f32::min);
    let right = boxes.iter().map(|b| b.left + b.width).fold(0.0, f32::max);
    let bottom = boxes.iter().map(|b| b.top + b.height).fold(0.0, f32::max);
    BoundingBox {
        width: right - left,
        height: bottom - top,
        left,
        top,
    }
}

fn relationship(kind: &str, ids: Vec<String>) -> Relationship {
    Relationship {
        r#type: Some(kind.to_string()),
        ids: Some(ids),
    }
}

fn new_block(block_type: &str, id: String, page: i32) -> Block {
    Block {
        block_type: Some(block_type.to_string()),
        id: Some(id),
        page: Some(page),
        ..Block::default()
    }
}

/// The words of a line, each with the character column it starts at.
fn words_of(line: &str) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;
    for (column, ch) in line.chars().enumerate() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push((start, std::mem::take(&mut current)));
            }
        } else {
            if current.is_empty() {
                start = column;
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push((start, current));
    }
    words
}

/// Whether `word` is an AMOUNT: figures, with the currency sign, the separators and the
/// sign an amount or a date is written with, and no letters.
///
/// What tells the value of a labelled line from the words that label it, on a line that
/// punctuates neither: `12.34`, `$12.34`, `-1,250.00` and `01/02/2023` are amounts, while
/// `RECEIPT` and `Net30` are not.
fn is_amount(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit())
        && word
            .chars()
            .all(|c| c.is_ascii_digit() || "$€£¥+-.,/%()".contains(c))
}

/// Lay out one page's lines, numbering their words so every read path agrees on ids.
///
/// The ONE place a line or a word gets its position, its size and its confidence, so the
/// four read paths over one document cannot disagree on them. Both come from what was
/// really read: the rectangle and the score [`ocr`] measured for the line's own ink, and
/// the character grid plus the page's own confidence for a line whose measured words do
/// not line up with the words laid out here.
fn layout_page(
    key: &str,
    page_index: usize,
    page_text: &str,
    content: &DocumentContent,
) -> Vec<LineLayout> {
    let measured = content.measured_page(page_index);
    let rotation = content.page_rotation(page_index);
    let mut laid_out: Vec<LineLayout> = page_text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            let cell_texts: Vec<&str> = line.split('\t').collect();
            let is_row = cell_texts.len() > 1;
            let laid_out = words_of(line);
            // The measured line for this one, used only when its words are the words
            // laid out here: the recogniser separates the words of its text on exactly
            // the gaps it measured them at, so the two lists agree, and a line where
            // they somehow did not is laid out on the character grid rather than given
            // another word's rectangle.
            let measured_line = measured
                .and_then(|page| page.lines.get(index))
                .filter(|line| line.words.len() == laid_out.len());
            let words: Vec<WordLayout> = laid_out
                .into_iter()
                .enumerate()
                .map(|(wi, (column, text))| {
                    let chars = text.chars().count();
                    let measured_word = measured_line.and_then(|line| line.words.get(wi));
                    WordLayout {
                        column,
                        text,
                        id: block_id(key, "WORD", &[page_index, index, wi]),
                        region: measured_word.map_or_else(
                            || Region::of_box(bounding_box(index, column, chars), rotation),
                            |word| Region::measured(&word.marks, rotation),
                        ),
                        confidence: measured_word
                            .map_or(content.confidence, |word| word.confidence),
                    }
                })
                .collect();
            // What splits a line into a LABEL and the VALUE it labels, as a character
            // position of the line:
            //
            // * a `:` with words on both sides of it;
            // * on a line laid out in TWO columns, the break between them, since a label
            //   standing in one column and its value in the next is how a form is set out
            //   whether or not the label is punctuated;
            // * on a plain line whose last word is an AMOUNT, the space before it: a line
            //   of words ending in figures labels those figures, which is how a receipt
            //   sets out its total and a statement its balance.
            //
            // A line of three columns or more is a table row, which labels nothing.
            let labelled = |at: usize| {
                words.iter().any(|w| w.column <= at) && words.iter().any(|w| w.column > at)
            };
            let punctuated = line.chars().position(|c| c == ':');
            let amount = words
                .last()
                .filter(|last| words.len() > 1 && is_amount(&last.text))
                .and_then(|last| last.column.checked_sub(1));
            let colon = match cell_texts.len() {
                0 => None,
                1 => punctuated.or(amount).filter(|at| labelled(*at)),
                2 => punctuated
                    .or_else(|| line.chars().position(|c| c == '\t'))
                    .filter(|at| labelled(*at)),
                _ => None,
            };
            let cells: Vec<usize> = if is_row {
                cell_texts
                    .iter()
                    .map(|cell| cell.split_whitespace().count())
                    .collect()
            } else {
                Vec::new()
            };
            let cell_starts: Vec<usize> = cells
                .iter()
                .scan(0usize, |start, count| {
                    let at = *start;
                    *start = start.saturating_add(*count);
                    Some(at)
                })
                .collect();
            LineLayout {
                id: block_id(key, "LINE", &[page_index, index]),
                index,
                // A line spans exactly the words it was read from, whichever way those
                // words got their boxes; a line with no words at all (which a non-blank
                // line never is) falls back to its place on the character grid.
                region: Region::spanning(words.iter().map(|word| &word.region), rotation)
                    .unwrap_or_else(|| {
                        Region::of_box(
                            bounding_box(index, 0, line.trim_end().chars().count()),
                            rotation,
                        )
                    }),
                rotation,
                confidence: measured_line.map_or(content.confidence, |line| line.confidence),
                text: line.to_string(),
                words,
                cells,
                cell_starts,
                colon,
            }
        })
        .collect();

    // A line of two columns labels its value only where it stands ALONE: several such lines
    // one after another are the rows of a two-column table, and a row labels nothing (see
    // the split above).
    let rows: Vec<bool> = laid_out
        .iter()
        .map(|line: &LineLayout| line.cells.len() > 1)
        .collect();
    for (index, line) in laid_out.iter_mut().enumerate() {
        let beside = index
            .checked_sub(1)
            .is_some_and(|before| rows[before])
            || rows.get(index + 1).copied().unwrap_or(false);
        if rows[index] && beside {
            line.colon = None;
        }
    }
    laid_out
}

/// Every `Block` for a document's stored text, for the requested features.
///
/// With [`FeatureSet::default`] (which is what `DetectDocumentText` and
/// `GetDocumentTextDetection` pass) the result holds only `PAGE`, `LINE` and `WORD`
/// blocks, and by construction none of them carries `RowIndex`, `ColumnIndex`,
/// `RowSpan`, `ColumnSpan` or `EntityTypes` — the `Block` docs say those are not
/// returned by the text-detection operations.
///
/// Bounded work: `read_content` refuses a document whose block count would exceed
/// [`MAX_BLOCKS_PER_DOCUMENT`], so this only ever runs over content under that ceiling.
pub fn blocks(key: &str, content: &DocumentContent, features: FeatureSet) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for (page_index, page_text) in content.pages.iter().enumerate() {
        let page_number = i32::try_from(page_index + 1).unwrap_or(i32::MAX);
        let lines = layout_page(key, page_index, page_text, content);
        let rotation = content.page_rotation(page_index);

        let mut text_blocks: Vec<Block> = Vec::new();
        for line in &lines {
            let mut line_block = new_block("LINE", line.id.clone(), page_number);
            line_block.text = Some(line.text.trim().to_string());
            line_block.confidence = Some(line.confidence);
            line_block.geometry = Some(line.region.geometry());
            if !line.words.is_empty() {
                line_block.relationships = Some(vec![relationship(
                    "CHILD",
                    line.words.iter().map(|w| w.id.clone()).collect(),
                )]);
            }
            text_blocks.push(line_block);
            for word in &line.words {
                let mut word_block = new_block("WORD", word.id.clone(), page_number);
                word_block.text = Some(word.text.clone());
                word_block.confidence = Some(word.confidence);
                word_block.text_type = Some("PRINTED".to_string());
                let mut word_geometry = word.region.geometry();
                // `Geometry$RotationAngle` is "a numerical value corresponding to the
                // rotation of the WORD block. Possible values are 0, 90, 180, and 270",
                // so it is reported for WORD blocks only. The angle is the one the page's
                // characters were READ at: a page is read at each of its quarter turns and
                // reported at the turn its marks recognised best (see [`ocr`]), so a
                // document scanned or photographed turned reports the turn its words are
                // drawn at, and one read along its own rows reports 0.
                word_geometry.rotation_angle = Some(rotation);
                word_block.geometry = Some(word_geometry);
                text_blocks.push(word_block);
            }
        }

        let (form_blocks, form_ids) = if features.forms {
            form_blocks(key, page_index, page_number, &lines)
        } else {
            (Vec::new(), Vec::new())
        };
        let (table_blocks, table_ids) = if features.tables {
            table_blocks(key, page_index, page_number, &lines)
        } else {
            (Vec::new(), Vec::new())
        };

        // A PAGE block's CHILD relationship lists every top-level block on the page.
        let mut children: Vec<String> = lines.iter().map(|l| l.id.clone()).collect();
        children.extend(form_ids);
        children.extend(table_ids);

        let mut page_block = new_block("PAGE", block_id(key, "PAGE", &[page_index]), page_number);
        // A PAGE's `Geometry` bounds what is ON the page: the extent of the content the
        // reader found there, together with the blocks it was read into. A page read off
        // pixels reports the extent of its own marks, so a rule, a border or a logo the
        // recogniser could not name is still part of the content the page block spans; a
        // page whose text carries no measurement reports the box enclosing its blocks.
        // A page nothing was found on has nothing to bound, so it reports the whole page:
        // that IS what was examined.
        let content_region = content
            .measured_page(page_index)
            .and_then(|page| page.content.as_ref())
            .map(|marks| Region::measured(marks, rotation));
        let page_region = Region::spanning(
            lines
                .iter()
                .map(|line| &line.region)
                .chain(content_region.iter()),
            rotation,
        )
        .unwrap_or_else(|| Region::of_box(box_of(&ocr::Rect::whole_page()), rotation));
        page_block.geometry = Some(page_region.geometry());
        if !children.is_empty() {
            page_block.relationships = Some(vec![relationship("CHILD", children)]);
        }

        out.push(page_block);
        out.extend(text_blocks);
        out.extend(form_blocks);
        out.extend(table_blocks);
    }
    out
}

/// `KEY_VALUE_SET` blocks for the page's labelled fields, and their ids in page order.
fn form_blocks(
    key: &str,
    page_index: usize,
    page_number: i32,
    lines: &[LineLayout],
) -> (Vec<Block>, Vec<String>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for line in lines {
        let Some((_, key_words, value_words)) = labelled_split(line) else {
            continue;
        };
        let key_id = block_id(key, "KEY", &[page_index, line.index]);
        let value_id = block_id(key, "VALUE", &[page_index, line.index]);

        let mut key_block = new_block("KEY_VALUE_SET", key_id.clone(), page_number);
        key_block.entity_types = Some(vec!["KEY".to_string()]);
        key_block.confidence = Some(words_confidence(line, &key_words));
        key_block.geometry = Some(words_region(line, &key_words).geometry());
        key_block.relationships = Some(vec![
            relationship("VALUE", vec![value_id.clone()]),
            relationship("CHILD", key_words.iter().map(|w| w.id.clone()).collect()),
        ]);

        let mut value_block = new_block("KEY_VALUE_SET", value_id.clone(), page_number);
        value_block.entity_types = Some(vec!["VALUE".to_string()]);
        value_block.confidence = Some(words_confidence(line, &value_words));
        value_block.geometry = Some(words_region(line, &value_words).geometry());
        value_block.relationships = Some(vec![relationship(
            "CHILD",
            value_words.iter().map(|w| w.id.clone()).collect(),
        )]);

        ids.push(key_id);
        ids.push(value_id);
        out.push(key_block);
        out.push(value_block);
    }
    (out, ids)
}

/// Where each of `words` sits on the page, as it was measured or laid out.
fn word_regions<'a>(words: &'a [&'a WordLayout]) -> impl Iterator<Item = &'a Region> {
    words.iter().map(|word| &word.region)
}

/// The region covering every one of `words`, or the line's own when it has none: the box
/// around them and the outline of the marks they were measured over.
fn words_region(line: &LineLayout, words: &[&WordLayout]) -> Region {
    Region::spanning(word_regions(words), line.rotation).unwrap_or_else(|| line.region.clone())
}

/// The `Confidence` a block spanning `words` reports.
///
/// The confidence of the characters THAT BLOCK was read from: the mean over its own
/// words, weighted by how many characters each contributed, which for a recognised page
/// is the mean score of exactly the glyphs inside the block. A block with no words of its
/// own has no characters to have measured, so it reports its line's.
fn words_confidence(line: &LineLayout, words: &[&WordLayout]) -> f32 {
    let characters: usize = words.iter().map(|w| w.text.chars().count()).sum();
    if characters == 0 {
        return line.confidence;
    }
    let total: f32 = words
        .iter()
        .map(|w| w.confidence * w.text.chars().count() as f32)
        .sum();
    (total / characters as f32).clamp(0.0, 100.0)
}

/// The label and the value halves of a labelled line, with the column of the `:` that
/// separates them, or `None` when the line labels nothing.
///
/// The ONE reader of a labelled line: the `FORMS` KEY/VALUE pairs and `AnalyzeExpense`'s
/// summary fields both split a line here, so the two paths cannot disagree on which
/// lines carry a field. A half with no text of its own labels nothing.
fn labelled_split(line: &LineLayout) -> Option<(usize, Vec<&WordLayout>, Vec<&WordLayout>)> {
    let colon = line.colon?;
    let label: Vec<&WordLayout> = line.words.iter().filter(|w| w.column <= colon).collect();
    let value: Vec<&WordLayout> = line.words.iter().filter(|w| w.column > colon).collect();
    if label.is_empty() || value.is_empty() {
        return None;
    }
    let chars = line.text.chars().count();
    if char_span(&line.text, 0, colon).trim().is_empty()
        || char_span(&line.text, colon + 1, chars).trim().is_empty()
    {
        return None;
    }
    Some((colon, label, value))
}

/// The page's tables: each one a contiguous run of tab-separated lines, in page order.
///
/// The ONE reader of where a table starts and ends, for both the `TABLE`/`CELL` blocks
/// and `AnalyzeExpense`'s line-item groups.
fn table_runs(lines: &[LineLayout]) -> Vec<&[LineLayout]> {
    let mut runs = Vec::new();
    let mut cursor = 0usize;
    while cursor < lines.len() {
        if lines[cursor].cells.is_empty() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < lines.len() && !lines[cursor].cells.is_empty() {
            cursor += 1;
        }
        runs.push(&lines[start..cursor]);
    }
    runs
}

/// The words of column `column` of a table row.
///
/// The cell's first word is read off [`LineLayout::cell_starts`], so this costs the cell's
/// own words and nothing per cell before it.
fn cell_words(line: &LineLayout, column: usize) -> Vec<&WordLayout> {
    let start = line.cell_starts.get(column).copied().unwrap_or_default();
    let count = line.cells.get(column).copied().unwrap_or_default();
    line.words
        .get(start..start.saturating_add(count))
        .unwrap_or_default()
        .iter()
        .collect()
}

/// Where each cell of a table row sits on the page, for the whole row at once.
///
/// A cell is the box of its own words, which is what was read in it. A cell with no words
/// at all has no text to have been read anywhere, so it is bounded by the cells AROUND it:
/// from the right edge of the nearest populated cell before it to the left edge of the
/// nearest one after it, on its row's own vertical band — never a cell-sized box at a
/// position nothing measured.
///
/// Those two neighbours are carried ALONG the row, one pass each way, rather than looked
/// for from each empty cell: a row of `n` cells costs `O(n)` plus its own words, so a line
/// that is nothing but separators is laid out in time proportional to the blocks it
/// becomes.
fn cell_regions(line: &LineLayout) -> Vec<Region> {
    let own: Vec<Option<Region>> = (0..line.cells.len())
        .map(|column| Region::spanning(word_regions(&cell_words(line, column)), line.rotation))
        .collect();

    let row_left = line.region.bbox.left;
    let row_right = line.region.bbox.left + line.region.bbox.width;

    let mut left_edges: Vec<f32> = Vec::with_capacity(own.len());
    let mut nearest_before: Option<f32> = None;
    for cell in &own {
        left_edges.push(nearest_before.unwrap_or(row_left));
        if let Some(cell) = cell {
            nearest_before = Some(cell.bbox.left + cell.bbox.width);
        }
    }

    let mut right_edges: Vec<f32> = vec![row_right; own.len()];
    let mut nearest_after: Option<f32> = None;
    for (column, cell) in own.iter().enumerate().rev() {
        right_edges[column] = nearest_after.unwrap_or(row_right);
        if let Some(cell) = cell {
            nearest_after = Some(cell.bbox.left);
        }
    }

    own.into_iter()
        .enumerate()
        .map(|(column, cell)| match cell {
            Some(region) => region,
            None => Region::of_box(
                BoundingBox {
                    width: (right_edges[column] - left_edges[column]).max(0.0),
                    height: line.region.bbox.height,
                    left: left_edges[column],
                    top: line.region.bbox.top,
                },
                line.rotation,
            ),
        })
        .collect()
}

/// `TABLE` and `CELL` blocks for the page's tab-separated rows, and the table ids.
///
/// A contiguous run of tab-separated lines is one table; a cell's `RowIndex` and
/// `ColumnIndex` are 1-based, as the `Block` docs state.
fn table_blocks(
    key: &str,
    page_index: usize,
    page_number: i32,
    lines: &[LineLayout],
) -> (Vec<Block>, Vec<String>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for (table_index, run) in table_runs(lines).into_iter().enumerate() {
        // A run is a CONTIGUOUS set of tab-separated lines, so `table_runs` never yields
        // an empty one. Binding the first row here makes that invariant explicit: a
        // `TABLE` block is only ever emitted for a run that has rows, and it reports the
        // `Confidence` of the rows it was read from rather than none at all.
        let Some(first_row) = run.first() else {
            continue;
        };
        let table_id = block_id(key, "TABLE", &[page_index, table_index]);
        let mut cell_blocks = Vec::new();
        let mut cell_ids = Vec::new();
        for (row, line) in run.iter().enumerate() {
            // Every cell's region for this row at once, so no cell rescans the row.
            let regions = cell_regions(line);
            for column in 0..line.cells.len() {
                let cell_words = cell_words(line, column);
                let cell_id = block_id(key, "CELL", &[page_index, table_index, row, column]);
                let mut cell = new_block("CELL", cell_id.clone(), page_number);
                cell.row_index = Some(i32::try_from(row + 1).unwrap_or(i32::MAX));
                cell.column_index = Some(i32::try_from(column + 1).unwrap_or(i32::MAX));
                cell.row_span = Some(1);
                cell.column_span = Some(1);
                cell.confidence = Some(words_confidence(line, &cell_words));
                cell.geometry = regions.get(column).map(Region::geometry);
                if !cell_words.is_empty() {
                    cell.relationships = Some(vec![relationship(
                        "CHILD",
                        cell_words.iter().map(|w| w.id.clone()).collect(),
                    )]);
                }
                cell_ids.push(cell_id);
                cell_blocks.push(cell);
            }
        }
        let mut table = new_block("TABLE", table_id.clone(), page_number);
        // The table's own characters: every word of every row it was read from.
        let table_words: Vec<&WordLayout> = run.iter().flat_map(|line| line.words.iter()).collect();
        table.confidence = Some(words_confidence(first_row, &table_words));
        table.geometry = Region::spanning(
            run.iter().map(|line| &line.region),
            first_row.rotation,
        )
        .as_ref()
        .map(Region::geometry);
        table.relationships = Some(vec![relationship("CHILD", cell_ids)]);

        ids.push(table_id);
        out.push(table);
        out.extend(cell_blocks);
    }
    (out, ids)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// A labelled field read out of a document: its label, the value it carries, and where on
/// the page that value sits.
#[derive(Debug, Clone)]
pub struct Field {
    /// The page the field was read from, as `Block$Page` numbers pages.
    pub page_number: i32,
    /// The label, as written in the document.
    pub label: String,
    /// The value the label carries, as written in the document.
    pub value: String,
    /// Where the value sits on the page, for the answer's `Geometry`.
    value_region: Region,
    /// The confidence the value's own characters were read with, for the answer's
    /// `Confidence`.
    pub value_confidence: f32,
    /// The label's retrieval terms, computed ONCE when the field is read and held as a
    /// set, so matching a query against every field of a document costs a lookup per term
    /// rather than a scan.
    label_terms: BTreeSet<String>,
}

/// Every labelled field of a document, in reading order.
///
/// Read through [`labelled_split`], the ONE reader of a labelled line, so a query is
/// answered from exactly the fields `FORMS` analysis and `AnalyzeExpense` report.
pub fn fields(key: &str, content: &DocumentContent) -> Vec<Field> {
    let mut out = Vec::new();
    for (page_index, page_text) in content.pages.iter().enumerate() {
        let page_number = i32::try_from(page_index + 1).unwrap_or(i32::MAX);
        for line in layout_page(key, page_index, page_text, content) {
            let Some((colon, _, value_words)) = labelled_split(&line) else {
                continue;
            };
            let label = char_span(&line.text, 0, colon).trim().to_string();
            let value_region = words_region(&line, &value_words);
            let value_confidence = words_confidence(&line, &value_words);
            out.push(Field {
                page_number,
                label_terms: terms(&label).into_iter().collect(),
                label,
                value: char_span(&line.text, colon + 1, line.text.chars().count())
                    .trim()
                    .to_string(),
                value_region,
                value_confidence,
            });
        }
    }
    out
}

/// Words a question is phrased with rather than asks about, so they say nothing about
/// which field answers it. NOT an AWS list: it is this emulator's retrieval stop list.
const QUERY_STOPWORDS: &[&str] = &[
    "the", "what", "which", "who", "whom", "whose", "when", "where", "was", "were", "is", "are",
    "does", "do", "did", "of", "for", "in", "on", "at", "to", "this", "that", "from", "and", "me",
    "my", "your", "please", "tell", "show", "give", "document", "there", "any", "has", "have",
];

/// The terms a query or a label is about: its alphanumeric words, lowercased, without the
/// stop list and without one-character fragments (an apostrophe's tail, say), at most
/// [`MAX_RETRIEVAL_TERMS`] of them.
fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() > 1 && !QUERY_STOPWORDS.contains(&term.as_str()))
        .take(MAX_RETRIEVAL_TERMS)
        .collect()
}

/// The field that answers `query`, and the share of the query's terms its label carries.
///
/// Term-overlap retrieval over the document's own labelled fields: no answer is reported
/// unless a field's label really carries at least [`MIN_QUERY_MATCH`] of the query's
/// terms, and the answer is that field's value as it was read out of the document.
///
/// Bounded on every factor: one pass over the fields per query (both counts capped, by the
/// caller's query quota and, through [`MAX_BLOCKS_PER_DOCUMENT`], by the document's line
/// count), at most [`MAX_RETRIEVAL_TERMS`] set lookups per field, and no allocation — the
/// query's terms and each field's are already computed.
fn answer_for<'a>(
    query: &PreparedQuery,
    wanted: &[String],
    fields: &'a [Field],
) -> Option<(&'a Field, f32)> {
    if wanted.is_empty() {
        return None;
    }
    let mut best: Option<(&Field, f32)> = None;
    for field in fields
        .iter()
        .filter(|field| query.covers(field.page_number))
    {
        let matched = wanted
            .iter()
            .filter(|term| field.label_terms.contains(term.as_str()))
            .count();
        let score = matched as f32 / wanted.len() as f32;
        if score >= MIN_QUERY_MATCH && best.is_none_or(|(_, previous)| score > previous) {
            best = Some((field, score));
        }
    }
    best
}

/// The `QUERY` block of each requested query, plus a `QUERY_RESULT` for every query this
/// document answers.
///
/// A `QUERY` block carries the query as asked (`Query.Text` and `Query.Alias` are the
/// caller's, which is what AWS reports them as) and relates to its answer with an `ANSWER`
/// relationship. A query the document holds no answer to gets a `QUERY` block with no
/// `ANSWER`: nothing is invented to answer it.
pub fn query_blocks(key: &str, content: &DocumentContent, queries: &[PreparedQuery]) -> Vec<Block> {
    if queries.is_empty() {
        return Vec::new();
    }
    let fields = fields(key, content);
    let mut out = Vec::new();
    for (index, query) in queries.iter().enumerate() {
        // The first page of the document the query asks about: a query that names no page
        // asks about page 1, as `Query$Pages` documents.
        let page = (1..=content.page_count())
            .find(|page| query.covers(*page))
            .unwrap_or(1);
        let mut block = new_block("QUERY", block_id(key, "QUERY", &[index]), page);
        // A QUERY is a question asked of the page, not a region of it, so it carries no
        // Geometry.
        block.query = Some(query.query.clone());
        let wanted = terms(&query.query.text);
        match answer_for(query, &wanted, &fields) {
            Some((field, score)) => {
                let result_id = block_id(key, "QUERY_RESULT", &[index]);
                block.relationships = Some(vec![relationship("ANSWER", vec![result_id.clone()])]);
                let mut result = new_block("QUERY_RESULT", result_id, field.page_number);
                result.text = Some(field.value.clone());
                // Confidence in the answer is the confidence of the text it was read
                // from, scaled by how much of the query that field's label accounts for.
                result.confidence = Some((score * field.value_confidence).clamp(0.0, 100.0));
                result.geometry = Some(field.value_region.geometry());
                out.push(block);
                out.push(result);
            }
            None => out.push(block),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Expense derivation
// ---------------------------------------------------------------------------

/// The normalized `ExpenseField.Type` each recognised label is reported under.
///
/// Source: AWS's Textract documentation for `AnalyzeExpense` — a summary field's `Type`
/// is the IMPLIED label (`ExpenseField$Type`: "The implied label of a detected element.
/// Present alongside LabelDetection for explicit elements."), drawn from the normalized
/// invoice/receipt field names the Developer Guide lists ("Analyzing Invoices and
/// Receipts"); a label the service does not recognise is reported as
/// [`UNRECOGNISED_FIELD_TYPE`].
///
/// The left column is the label AS WRITTEN IN THE DOCUMENT, put through
/// [`normalize_label`]; the right column is the normalized type. Listed alphabetically
/// for readability; the lookup is a scan, so the order carries no meaning.
const SUMMARY_FIELD_TYPES: &[(&str, &str)] = &[
    ("ACCOUNT_NUMBER", "CUSTOMER_NUMBER"),
    ("ADDRESS", "VENDOR_ADDRESS"),
    ("AMOUNT_DUE", "AMOUNT_DUE"),
    ("AMOUNT_PAID", "AMOUNT_PAID"),
    ("BALANCE_DUE", "AMOUNT_DUE"),
    ("BILL_TO", "RECEIVER_NAME"),
    ("CUSTOMER", "RECEIVER_NAME"),
    ("CUSTOMER_ID", "CUSTOMER_NUMBER"),
    ("CUSTOMER_NUMBER", "CUSTOMER_NUMBER"),
    ("DATE", "INVOICE_RECEIPT_DATE"),
    ("DELIVERY_DATE", "DELIVERY_DATE"),
    ("DISCOUNT", "DISCOUNT"),
    ("DUE_DATE", "DUE_DATE"),
    ("GRAND_TOTAL", "TOTAL"),
    ("GRATUITY", "GRATUITY"),
    ("INVOICE", "INVOICE_RECEIPT_ID"),
    ("INVOICE_DATE", "INVOICE_RECEIPT_DATE"),
    ("INVOICE_ID", "INVOICE_RECEIPT_ID"),
    ("INVOICE_NO", "INVOICE_RECEIPT_ID"),
    ("INVOICE_NUMBER", "INVOICE_RECEIPT_ID"),
    ("MERCHANT", "VENDOR_NAME"),
    ("ORDER_DATE", "ORDER_DATE"),
    ("PAID", "AMOUNT_PAID"),
    ("PAYMENT_TERMS", "PAYMENT_TERMS"),
    ("PHONE", "VENDOR_PHONE"),
    ("PO_NUMBER", "PO_NUMBER"),
    ("PREVIOUS_BALANCE", "PRIOR_BALANCE"),
    ("PRIOR_BALANCE", "PRIOR_BALANCE"),
    ("PURCHASE_ORDER", "PO_NUMBER"),
    ("RECEIPT_DATE", "INVOICE_RECEIPT_DATE"),
    ("RECEIPT_ID", "INVOICE_RECEIPT_ID"),
    ("RECEIPT_NUMBER", "INVOICE_RECEIPT_ID"),
    ("RECEIVER_ADDRESS", "RECEIVER_ADDRESS"),
    ("RECEIVER_NAME", "RECEIVER_NAME"),
    ("RECEIVER_PHONE", "RECEIVER_PHONE"),
    ("SALES_TAX", "TAX"),
    ("SERVICE_CHARGE", "SERVICE_CHARGE"),
    ("SHIPPING", "SHIPPING_HANDLING_CHARGE"),
    ("SHIPPING_AND_HANDLING", "SHIPPING_HANDLING_CHARGE"),
    ("SHIP_TO", "RECEIVER_ADDRESS"),
    ("SOLD_TO", "RECEIVER_NAME"),
    ("STORE", "VENDOR_NAME"),
    ("SUBTOTAL", "SUBTOTAL"),
    ("SUB_TOTAL", "SUBTOTAL"),
    ("TAX", "TAX"),
    ("TAX_ID", "TAX_PAYER_ID"),
    ("TAX_PAYER_ID", "TAX_PAYER_ID"),
    ("TELEPHONE", "VENDOR_PHONE"),
    ("TERMS", "PAYMENT_TERMS"),
    ("TIP", "GRATUITY"),
    ("TOTAL", "TOTAL"),
    ("TOTAL_DUE", "TOTAL"),
    ("URL", "VENDOR_URL"),
    ("VAT", "TAX"),
    ("VENDOR", "VENDOR_NAME"),
    ("VENDOR_ADDRESS", "VENDOR_ADDRESS"),
    ("VENDOR_NAME", "VENDOR_NAME"),
    ("VENDOR_PHONE", "VENDOR_PHONE"),
    ("VENDOR_URL", "VENDOR_URL"),
    ("WEBSITE", "VENDOR_URL"),
];

/// The normalized `ExpenseField.Type` each recognised TABLE COLUMN header is reported
/// under, for a line item. Same source and same shape as [`SUMMARY_FIELD_TYPES`].
const LINE_ITEM_FIELD_TYPES: &[(&str, &str)] = &[
    ("AMOUNT", "PRICE"),
    ("DESCRIPTION", "ITEM"),
    ("ITEM", "ITEM"),
    ("ITEM_CODE", "PRODUCT_CODE"),
    ("LINE_TOTAL", "PRICE"),
    ("PRICE", "PRICE"),
    ("PRODUCT", "ITEM"),
    ("PRODUCT_CODE", "PRODUCT_CODE"),
    ("QTY", "QUANTITY"),
    ("QUANTITY", "QUANTITY"),
    ("RATE", "UNIT_PRICE"),
    ("SKU", "PRODUCT_CODE"),
    ("TOTAL", "PRICE"),
    ("UNIT_PRICE", "UNIT_PRICE"),
    ("UPC", "PRODUCT_CODE"),
];

/// The `Type` reported for a detected element whose label names no normalized field.
pub const UNRECOGNISED_FIELD_TYPE: &str = "OTHER";

/// The `Type` of the line-item field that carries a table row in full.
pub const EXPENSE_ROW_FIELD_TYPE: &str = "EXPENSE_ROW";

/// The currency codes `ExpenseCurrency$Code` documents: "the current supported codes
/// are: USD, EUR, GBP, CAD, INR, JPY, CHF, AUD, CNY, BZR, SEK, HKD".
pub const EXPENSE_CURRENCY_CODES: &[&str] = &[
    "USD", "EUR", "GBP", "CAD", "INR", "JPY", "CHF", "AUD", "CNY", "BZR", "SEK", "HKD",
];

/// The currency symbol a value may carry instead of a code, and the
/// [`EXPENSE_CURRENCY_CODES`] entry it denotes. Tried in order, so a qualified dollar
/// sign is read before the bare one.
///
/// Compared against the UPPERCASED value text, hence the uppercase spellings. A symbol
/// several supported currencies share and nothing qualifies (`¥`, written for both JPY
/// and CNY) is read as the first of them; an explicit code in the value always wins over
/// a symbol.
const CURRENCY_SYMBOLS: &[(&str, &str)] = &[
    ("R$", "BZR"),
    ("CA$", "CAD"),
    ("C$", "CAD"),
    ("AU$", "AUD"),
    ("A$", "AUD"),
    ("HK$", "HKD"),
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("₹", "INR"),
    ("¥", "JPY"),
];

/// A label as the field catalogs key it: uppercased, with every run of characters that
/// is not a letter or a digit folded to one `_`.
pub fn normalize_label(label: &str) -> String {
    let mut out = String::new();
    for ch in label.to_uppercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// The normalized summary-field type `label` is reported under, or
/// [`UNRECOGNISED_FIELD_TYPE`].
pub fn summary_field_type(label: &str) -> &'static str {
    let normalized = normalize_label(label);
    SUMMARY_FIELD_TYPES
        .iter()
        .find(|(alias, _)| *alias == normalized)
        .map_or(UNRECOGNISED_FIELD_TYPE, |(_, kind)| *kind)
}

/// The normalized line-item type a table column headed `header` is reported under, or
/// `None` when the header names no line-item field.
pub fn line_item_field_type(header: &str) -> Option<&'static str> {
    let normalized = normalize_label(header);
    LINE_ITEM_FIELD_TYPES
        .iter()
        .find(|(alias, _)| *alias == normalized)
        .map(|(_, kind)| *kind)
}

/// The supported currency code a detected value names, or `None`.
///
/// `ExpenseField$Currency` is "the kind of currency ... associated with any monatary
/// value detected", so a value carries one only when it holds a number AND names a
/// currency: an explicit [`EXPENSE_CURRENCY_CODES`] token, or a [`CURRENCY_SYMBOLS`]
/// symbol.
pub fn detect_currency(text: &str) -> Option<&'static str> {
    if !text.chars().any(char::is_numeric) {
        return None;
    }
    let upper = text.to_uppercase();
    for token in upper.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        if let Some(code) = EXPENSE_CURRENCY_CODES.iter().find(|code| **code == token) {
            return Some(code);
        }
    }
    CURRENCY_SYMBOLS
        .iter()
        .find(|(symbol, _)| upper.contains(symbol))
        .map(|(_, code)| *code)
}

/// An `ExpenseDetection` for text that was really read in `region`, with the confidence
/// it was read with.
fn detection(text: &str, region: &Region, confidence: f32) -> ExpenseDetection {
    ExpenseDetection {
        text: Some(text.to_string()),
        geometry: Some(region.geometry()),
        confidence: Some(confidence),
    }
}

/// An `ExpenseField` of normalized type `kind`.
fn expense_field(
    kind: &str,
    label: Option<ExpenseDetection>,
    value: ExpenseDetection,
    page_number: i32,
    confidence: f32,
) -> ExpenseField {
    let currency = value
        .text
        .as_deref()
        .and_then(detect_currency)
        .map(|code| ExpenseCurrency {
            code: Some(code.to_string()),
            confidence: Some(confidence),
        });
    ExpenseField {
        r#type: Some(ExpenseType {
            text: Some(kind.to_string()),
            confidence: Some(confidence),
        }),
        label_detection: label,
        value_detection: Some(value),
        page_number: Some(page_number),
        currency,
        // This emulator reads one flat text document, so it resolves no name/address
        // groups to report; `GroupProperties` is optional and is left unset rather than
        // filled with a grouping nothing computed.
        group_properties: None,
    }
}

/// The character-indexed span `[from, to)` of a line's text.
fn char_span(text: &str, from: usize, to: usize) -> String {
    text.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// The summary fields of one page: every labelled line outside a table, in page order.
///
/// A labelled line is exactly what the `FORMS` path reads as a KEY/VALUE pair, so
/// `AnalyzeExpense` and `AnalyzeDocument` agree on which lines carry a field.
fn summary_fields(lines: &[LineLayout], page_number: i32) -> Vec<ExpenseField> {
    let mut out = Vec::new();
    for line in lines {
        let Some((colon, label_words, value_words)) = labelled_split(line) else {
            continue;
        };
        let chars = line.text.chars().count();
        let label = char_span(&line.text, 0, colon).trim().to_string();
        let value = char_span(&line.text, colon + 1, chars).trim().to_string();
        let value_confidence = words_confidence(line, &value_words);
        out.push(expense_field(
            summary_field_type(&label),
            Some(detection(
                &label,
                &words_region(line, &label_words),
                words_confidence(line, &label_words),
            )),
            detection(&value, &words_region(line, &value_words), value_confidence),
            page_number,
            value_confidence,
        ));
    }
    out
}

/// The cells of a table row, one entry per tab-separated column.
fn cell_texts(line: &LineLayout) -> Vec<String> {
    line.text
        .split('\t')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// The `LineItemFields` of one table row: the row in full, then one field per populated
/// cell, typed by the table's header row when it has one.
fn line_item(line: &LineLayout, header: Option<&LineLayout>, page_number: i32) -> LineItemFields {
    let texts = cell_texts(line);
    let row_words: Vec<&WordLayout> = line.words.iter().collect();
    let row_text = texts
        .iter()
        .filter(|cell| !cell.is_empty())
        .cloned()
        .collect::<Vec<String>>()
        .join(" ");
    let mut fields = vec![expense_field(
        EXPENSE_ROW_FIELD_TYPE,
        None,
        detection(
            &row_text,
            &words_region(line, &row_words),
            words_confidence(line, &row_words),
        ),
        page_number,
        words_confidence(line, &row_words),
    )];
    let header_texts = header.map(cell_texts).unwrap_or_default();
    for (column, text) in texts.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let header_text = header_texts.get(column).filter(|t| !t.is_empty());
        let kind = header_text
            .and_then(|t| line_item_field_type(t))
            .unwrap_or(UNRECOGNISED_FIELD_TYPE);
        // A header row states the column's label explicitly, so it is reported as the
        // field's `LabelDetection`; a headerless table has none to report.
        let label = match (header, header_text) {
            (Some(header), Some(header_text)) => Some(detection(
                header_text,
                &words_region(header, &cell_words(header, column)),
                words_confidence(header, &cell_words(header, column)),
            )),
            _ => None,
        };
        let cell_confidence = words_confidence(line, &cell_words(line, column));
        fields.push(expense_field(
            kind,
            label,
            detection(
                text,
                &words_region(line, &cell_words(line, column)),
                cell_confidence,
            ),
            page_number,
            cell_confidence,
        ));
    }
    LineItemFields {
        line_item_expense_fields: Some(fields),
    }
}

/// The line-item groups of one page: one group per table, as [`table_runs`] finds them,
/// indexed from 1 in page order.
///
/// Always at least one group: an `ExpenseDocument` AWS returns carries a
/// `LineItemGroups` entry with `LineItemGroupIndex` 1 even for a document on which it
/// detects no line items (its `LineItems` is then empty), so a caller can always read
/// the first group.
///
/// That is what the recorded `AnalyzeExpense` response this service is measured against
/// carries for a page with no line-item table: `LineItemGroups` is
/// `[{LineItemGroupIndex: 1, LineItems: []}]` alongside an empty `SummaryFields` — the
/// group is present and empty, not absent.
fn line_item_groups(lines: &[LineLayout], page_number: i32) -> Vec<LineItemGroup> {
    let mut groups: Vec<LineItemGroup> = table_runs(lines)
        .into_iter()
        .enumerate()
        .map(|(table_index, run)| {
            // The first row is the table's header when it has rows under it and every one
            // of its populated cells names a line-item field; otherwise the table is
            // headerless and every row is a line item.
            let header_row = run.first().filter(|first| {
                run.len() > 1
                    && cell_texts(first)
                        .iter()
                        .filter(|cell| !cell.is_empty())
                        .all(|cell| line_item_field_type(cell).is_some())
            });
            let rows = if header_row.is_some() { &run[1..] } else { run };
            let line_items: Vec<LineItemFields> = rows
                .iter()
                .map(|row| line_item(row, header_row, page_number))
                .collect();
            LineItemGroup {
                line_item_group_index: Some(i32::try_from(table_index + 1).unwrap_or(i32::MAX)),
                line_items: Some(line_items),
            }
        })
        .collect();
    if groups.is_empty() {
        groups.push(LineItemGroup {
            line_item_group_index: Some(1),
            line_items: Some(Vec::new()),
        });
    }
    groups
}

/// Every `ExpenseDocument` for a document's stored text.
///
/// `ExpenseIndex` "denotes which invoice or receipt in the document the information is
/// coming from. First document will be 1, the second 2, and so on" — one page of the
/// submitted document is one invoice or receipt here, so page N answers as expense
/// document N. Each one carries, per `ExpenseDocument$Blocks`, "the same [blocks] as
/// reported when DetectDocumentText is run on a document", for that page.
///
/// Derived from the SAME stored text and the SAME line layout every other read path
/// uses, so the fields only ever describe text that was really read out of the document;
/// bounded by [`MAX_BLOCKS_PER_DOCUMENT`], which `read_content` already enforced.
pub fn expense_documents(key: &str, content: &DocumentContent) -> Vec<ExpenseDocument> {
    // Grouped in ONE pass: a per-page scan of the whole block list would cost
    // pages x blocks for a single request.
    let mut by_page: HashMap<i32, Vec<Block>> = HashMap::new();
    for block in blocks(key, content, FeatureSet::default()) {
        by_page
            .entry(block.page.unwrap_or_default())
            .or_default()
            .push(block);
    }
    content
        .pages
        .iter()
        .enumerate()
        .map(|(page_index, page_text)| {
            let page_number = i32::try_from(page_index + 1).unwrap_or(i32::MAX);
            let lines = layout_page(key, page_index, page_text, content);
            ExpenseDocument {
                expense_index: Some(page_number),
                summary_fields: Some(summary_fields(&lines, page_number)),
                line_item_groups: Some(line_item_groups(&lines, page_number)),
                blocks: Some(by_page.remove(&page_number).unwrap_or_default()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// An opaque cursor into ONE listing: a job's block list, or a filtered resource
/// listing.
///
/// Stateless and self-verifying on purpose: a paginated read is a reporting call, so it
/// must not write state to hand out a cursor. The token binds its offset to `scope` — the
/// listing it was cut from — with a digest, so a token minted for one listing cannot be
/// replayed against another.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    let digest = hex::encode(Sha256::digest(format!("{scope}|{offset}").as_bytes()));
    let prefix = PAGE_TOKEN_LEN - PAGE_TOKEN_OFFSET_DIGITS;
    format!("{}{offset:08x}", &digest[..prefix])
}

/// The offset a token names, or `None` when it was not minted for this `scope`.
///
/// TOTAL over any `&str`: `NextToken` is caller-controlled, so the token is rejected
/// unless it is ASCII of exactly the issued length, and every read of it uses a
/// non-panicking accessor.
pub fn decode_page_token(scope: &str, token: &str) -> Option<usize> {
    if !token.is_ascii() || token.len() != PAGE_TOKEN_LEN {
        return None;
    }
    let digits = token.get(PAGE_TOKEN_LEN - PAGE_TOKEN_OFFSET_DIGITS..)?;
    let offset = usize::from_str_radix(digits, 16).ok()?;
    (encode_page_token(scope, offset) == token).then_some(offset)
}

/// One page of `items`, plus the token for the next page (`None` on the last one).
///
/// The ONE page-cutting rule every paginated operation of this service uses, so their
/// `MaxResults`/`NextToken` semantics cannot drift apart: `scope` names the listing the
/// page is cut from (see [`encode_page_token`]), `offset` is where this page starts, and
/// an offset past the end answers an empty page rather than failing.
///
/// `None` when `max_results` is present but not a usable page size, which the caller
/// refuses with `InvalidParameterException` rather than quietly substituting a default.
pub fn page_of<T: Clone>(
    scope: &str,
    items: &[T],
    max_results: Option<i32>,
    offset: usize,
) -> Option<(Vec<T>, Option<String>)> {
    let limit = match max_results {
        None => DEFAULT_MAX_RESULTS,
        Some(requested) => usize::try_from(requested).ok().filter(|m| *m > 0)?,
    };
    let end = offset.saturating_add(limit).min(items.len());
    let page = items.get(offset..end).unwrap_or_default().to_vec();
    let next = (end < items.len()).then(|| encode_page_token(scope, end));
    Some((page, next))
}

/// One page of a job's `blocks`: [`page_of`] over the job's own cursor scope.
pub fn page_of_blocks(
    job_id: &str,
    blocks: &[Block],
    max_results: Option<i32>,
    offset: usize,
) -> Option<(Vec<Block>, Option<String>)> {
    page_of(job_id, blocks, max_results, offset)
}

// ---------------------------------------------------------------------------
// Resource listings
// ---------------------------------------------------------------------------

/// A wire `CreationTime` bound as whole nanoseconds since the Unix epoch.
fn datetime_nanos(value: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(value.secs()) * 1_000_000_000 + i128::from(value.subsec_nanos())
}

/// A stored instant on the SAME scale as [`datetime_nanos`], so a filter bound and a
/// stored `CreationTime` are compared exactly rather than through a lossy float.
fn system_time_nanos(value: SystemTime) -> i128 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(since) => i128::try_from(since.as_nanos()).unwrap_or(i128::MAX),
        Err(earlier) => -i128::try_from(earlier.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// Whether something created at `created_at` falls inside the creation-time window the
/// request named.
///
/// The ONE window rule every listing of this service filters by, so they cannot drift
/// apart. AWS, on `ListAdaptersRequest$AfterCreationTime` and
/// `ListAdapterVersionsRequest$AfterCreationTime`: each "[s]pecifies the lower bound for
/// the ... operation", and the `$BeforeCreationTime` of each specifies the upper bound —
/// so a resource is listed exactly when its stored `CreationTime` lies within the bounds
/// the request gave, each bound inclusive, an absent one unbounded on that side, and a
/// request naming neither filtering nothing out.
pub fn within_creation_window(
    created_at: SystemTime,
    after_creation_time: Option<&::aws_smithy_types::DateTime>,
    before_creation_time: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let created = system_time_nanos(created_at);
    after_creation_time.is_none_or(|bound| created >= datetime_nanos(bound))
        && before_creation_time.is_none_or(|bound| created <= datetime_nanos(bound))
}

/// One creation-time bound as a cursor scope names it; `-` for an absent bound.
fn bound_key(value: Option<&::aws_smithy_types::DateTime>) -> String {
    value.map_or_else(|| "-".to_string(), |value| datetime_nanos(value).to_string())
}

/// The cursor scope a `ListAdapters` listing's pages are bound to: the filtration
/// criteria the listing was computed under.
///
/// A `NextToken` is an offset into ONE list, so a cursor cut under one creation-time
/// window names nothing in another: changing the window invalidates the cursor instead of
/// silently indexing a different list.
pub fn adapter_page_scope(
    after_creation_time: Option<&::aws_smithy_types::DateTime>,
    before_creation_time: Option<&::aws_smithy_types::DateTime>,
) -> String {
    format!(
        "ListAdapters|{}|{}",
        bound_key(after_creation_time),
        bound_key(before_creation_time)
    )
}

/// The cursor scope a `ListAdapterVersions` listing's pages are bound to: which adapter
/// the listing was scoped to, and the creation-time window it was computed under.
///
/// Same rule as [`adapter_page_scope`], with the adapter in the scope because an
/// `AdapterId` selects WHICH versions are listed: a cursor cut for one adapter names
/// nothing in another adapter's listing. `MaxResults` is deliberately absent from the
/// scope — a caller may change the page size between pages.
pub fn adapter_version_page_scope(
    adapter_id: Option<&str>,
    after_creation_time: Option<&::aws_smithy_types::DateTime>,
    before_creation_time: Option<&::aws_smithy_types::DateTime>,
) -> String {
    // The adapter is length-prefixed rather than interpolated bare: an `AdapterId` is
    // caller-controlled, so a scope that just concatenated it could be forged by an id
    // carrying the separator.
    let adapter = adapter_id.map_or_else(
        || "-".to_string(),
        |adapter_id| format!("{}:{adapter_id}", adapter_id.len()),
    );
    format!(
        "ListAdapterVersions|{adapter}|{}|{}",
        bound_key(after_creation_time),
        bound_key(before_creation_time)
    )
}

/// Every stored adapter the filtration criteria admit, as `ListAdapters` reports it, in
/// the ONE total order the listing's pages are cut from.
///
/// An adapter is listed exactly when its stored `CreationTime` lies inside the window the
/// request named — see [`within_creation_window`]. AWS documents no order for the listing
/// and an offset cursor is only coherent over a stable one, so adapters are reported
/// oldest first, with the `AdapterId` breaking ties between adapters created in the same
/// instant.
///
/// Derived from the stored adapter records alone — the same records `GetAdapter` reports —
/// so the listing can never disagree with a read of one adapter.
pub fn listed_adapters(
    store: &ComAmazonawsTextractTextractState,
    after_creation_time: Option<&::aws_smithy_types::DateTime>,
    before_creation_time: Option<&::aws_smithy_types::DateTime>,
) -> Vec<AdapterOverview> {
    let mut selected: Vec<&AdapterRecord> = store
        .adapters
        .values()
        .filter(|adapter| {
            within_creation_window(adapter.created_at, after_creation_time, before_creation_time)
        })
        .collect();
    selected.sort_by(|left, right| {
        system_time_nanos(left.created_at)
            .cmp(&system_time_nanos(right.created_at))
            .then_with(|| left.adapter_id.cmp(&right.adapter_id))
    });
    selected
        .into_iter()
        .map(|adapter| AdapterOverview {
            adapter_id: Some(adapter.adapter_id.clone()),
            adapter_name: Some(adapter.adapter_name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(adapter.created_at)),
            feature_types: Some(adapter.feature_types.clone()),
        })
        .collect()
}
