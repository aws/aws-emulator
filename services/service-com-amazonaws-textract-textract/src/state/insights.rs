// SPDX-License-Identifier: CC0-1.0

//! What a document insights job EXTRACTS from the document it was pointed at, and where it
//! writes that extraction.
//!
//! `StartDocumentInsights` "analyzes a document using the configuration defined by a
//! project or a set of blueprints" and `OutputConfiguration$S3Uri` is "[t]he Amazon S3 URI
//! where the analysis results are written", so a job is real work with a real artefact:
//! the document's bytes are read from the object store this emulator holds
//! ([`super::read_submitted_document`], the one path every other read path of this service
//! reads a document through), the content is derived from THAT text, and the result is
//! written back as an object the caller can fetch with `GetObject`. Nothing here is
//! echoed from the request or synthesised to look computed — an element only ever describes
//! text that was really read out of the document.
//!
//! ## What the configuration decides
//!
//! The project's `ExtractionConfiguration` is the whole contract for the output:
//!
//! * `Granularity$Types` says at which levels content is extracted — the whole document,
//!   each page, each structural element, each line, each word — so the output carries one
//!   entry per item at each level the request asked for and nothing at a level it did not;
//! * `BoundingBox$State` says whether each entry carries the geometry the item was read
//!   at; `DISABLED` omits it rather than reporting a box nothing measured;
//! * `OutputFormat$TextFormat$Types` says which renderings of the document's text the
//!   output carries, and `OutputFormat$AdditionalFileFormat$State` whether a second object
//!   carrying the document's plain text is written beside the primary one.
//!
//! A blueprint adds the CUSTOM half: its schema "defines the fields to extract and their
//! expected structure" (`CreateBlueprintRequest$Schema`), so each field the schema names is
//! looked up in the document's own labelled fields ([`super::fields`], the same reading
//! `FORMS` analysis and a `QUERIES` answer come from) and reported with the value and the
//! confidence that field was read with. A field the document does not carry is reported as
//! absent; nothing is invented to fill it.
//!
//! ## What a job does NOT carry out
//!
//! A project may also ask for a generative summary or a figure caption, for personally
//! identifiable information to be detected and redacted, or for a multi-document packet to
//! be split — capabilities this emulator does not have ([`unapplied_configurations`]). Such
//! a job still writes the extraction it CAN derive and then settles into the modelled
//! `PARTIAL_SUCCESS`, with `ErrorType` and `ErrorMessage` naming exactly the members that
//! asked: a caller asking for `DETECTION_AND_REDACTION` is told the output was not
//! redacted, rather than handed unredacted content under an unqualified success.
//!
//! ## What bounds one job
//!
//! Every factor a caller chooses is capped before the work is done: the document's bytes by
//! [`super::MAX_S3_DOCUMENT_BYTES`], the items it expands into by
//! [`super::MAX_BLOCKS_PER_DOCUMENT`], and the fields one schema can ask for by
//! [`super::intelligence::MAX_BLUEPRINT_FIELDS`]. The read itself runs on the blocking
//! pool, where it occupies no async worker.

use std::time::SystemTime;

use ::serde_json::{json, Map, Value};

use crate::ops::types::{
    BoundingBoxConfiguration, ExtractionConfiguration, ExtractionGranularity, OutputFormat,
    OutputAdditionalFileFormat, OutputTextFormat,
};

use super::intelligence::{ProjectConfiguration, MAX_BLUEPRINT_FIELDS};
use super::{DocumentContent, DocumentRef, DocumentRead, FeatureSet, S3Ref, Unreadable};

/// The `State` value that turns a configuration knob on, from the modelled `State` enum
/// (`ENABLED | DISABLED`).
pub const STATE_ENABLED: &str = "ENABLED";

/// The `State` value that turns it off.
pub const STATE_DISABLED: &str = "DISABLED";

/// `ExtractionGranularityType$DOCUMENT`: the whole document as one item.
pub const GRANULARITY_DOCUMENT: &str = "DOCUMENT";

/// `ExtractionGranularityType$PAGE`.
pub const GRANULARITY_PAGE: &str = "PAGE";

/// `ExtractionGranularityType$ELEMENT`: the structural items of a page.
pub const GRANULARITY_ELEMENT: &str = "ELEMENT";

/// `ExtractionGranularityType$LINE`.
pub const GRANULARITY_LINE: &str = "LINE";

/// `ExtractionGranularityType$WORD`.
pub const GRANULARITY_WORD: &str = "WORD";

/// `OutputTextFormatType$PLAIN_TEXT`.
pub const TEXT_FORMAT_PLAIN_TEXT: &str = "PLAIN_TEXT";

/// `OutputTextFormatType$MARKDOWN`.
pub const TEXT_FORMAT_MARKDOWN: &str = "MARKDOWN";

/// `OutputTextFormatType$HTML`.
pub const TEXT_FORMAT_HTML: &str = "HTML";

/// `OutputTextFormatType$CSV`.
pub const TEXT_FORMAT_CSV: &str = "CSV";

/// The `BlockType`s an `ELEMENT`-granularity extraction reports.
///
/// The items of a page ABOVE a line: the one `KEY_VALUE_SET` pair of a labelled field and
/// the `TABLE` and its `CELL`s of a run of rows — the structural blocks this service
/// derives (`super::blocks`).
const ELEMENT_BLOCK_TYPES: &[&str] = &["KEY_VALUE_SET", "TABLE", "CELL", "MERGED_CELL"];

/// The media type of the primary output object.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type of the additional plain-text object.
const TEXT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// The message a job carries when the service itself could not finish it — the document
/// was read and the result could not be stored.
///
/// The sentence AWS states `InternalServerError` with: "Amazon Textract experienced a
/// service issue. Try your call again."
pub const SERVICE_ISSUE_MESSAGE: &str =
    "Amazon Textract experienced a service issue. Try your call again.";

/// The `ErrorType` that names a failure the service itself had.
pub const INTERNAL_ERROR_TYPE: &str = "InternalServerError";

/// The `ErrorType` a job carries when part of what the project asked for is a capability
/// this emulator does not have.
///
/// The job's `Status` is then the modelled `PARTIAL_SUCCESS`: the extraction output WAS
/// written, so the job neither failed nor can claim the rest was done.
pub const UNSUPPORTED_CONFIGURATION_ERROR_TYPE: &str = "UnsupportedConfiguration";

/// The configurations a project can ask for that this emulator does not carry out, named as
/// the members of the project that asked for them.
///
/// A generative summary and a figure caption are produced by a generative model this
/// emulator does not run; detecting and redacting personally identifiable information, and
/// splitting a multi-document packet into its documents, are analyses it does not perform.
/// The extraction it CAN derive is still written, so a job asking for any of these settles
/// into the modelled `PARTIAL_SUCCESS` naming exactly these members rather than reporting an
/// unqualified success for work nothing did — a caller asking for
/// `DETECTION_AND_REDACTION` is told the output was not redacted.
///
/// A configuration set to `DISABLED`, or absent, asks for nothing and is not named.
#[must_use]
pub fn unapplied_configurations(configuration: &ProjectConfiguration) -> Vec<&'static str> {
    let mut unapplied: Vec<&'static str> = Vec::new();
    if let Some(generative) = &configuration.generative {
        if generative
            .summarization
            .as_ref()
            .is_some_and(|summarization| summarization.state == STATE_ENABLED)
        {
            unapplied.push("GenerativeConfiguration.Summarization");
        }
        if generative
            .figure_caption
            .as_ref()
            .is_some_and(|caption| caption.state == STATE_ENABLED)
        {
            unapplied.push("GenerativeConfiguration.FigureCaption");
        }
    }
    if configuration
        .sensitive_data
        .as_ref()
        .is_some_and(|sensitive| sensitive.detection_mode != SENSITIVE_DATA_DISABLED)
    {
        unapplied.push("SensitiveDataConfiguration");
    }
    if configuration
        .splitter
        .as_ref()
        .is_some_and(|splitter| splitter.state == STATE_ENABLED)
    {
        unapplied.push("SplitterConfiguration");
    }
    unapplied
}

/// The `SensitiveDataDetectionMode` that asks for no detection at all, from the modelled
/// `SensitiveDataDetectionMode` enum (`DETECTION | DETECTION_AND_REDACTION | DISABLED`).
pub const SENSITIVE_DATA_DISABLED: &str = "DISABLED";

/// The sentence a `PARTIAL_SUCCESS` job states its reason with: that the extraction output
/// was written, and which of the project's configurations were not applied.
///
/// The fixed text is this class's; the variable part is the member names the project itself
/// asked those capabilities for ([`unapplied_configurations`]), so nothing is quoted that
/// the caller did not configure.
#[must_use]
pub fn unapplied_message(unapplied: &[&'static str]) -> String {
    format!(
        "The extraction output was written. Amazon Textract did not apply: {}.",
        unapplied.join(", ")
    )
}

/// The extraction configuration a job runs under when the request named no project.
///
/// `StartDocumentInsights` takes `ProjectArn` as optional, so a job can be submitted with
/// blueprints alone, and AWS documents no default for that case. This is therefore a
/// CHOSEN default, and the widest one: content at every granularity the model declares,
/// with the geometry each item was read at, rendered as plain text. A project that wants
/// less says so in its own `ExtractionConfiguration`.
#[must_use]
pub fn default_extraction() -> ExtractionConfiguration {
    ExtractionConfiguration {
        bounding_box: BoundingBoxConfiguration {
            state: STATE_ENABLED.to_string(),
        },
        granularity: ExtractionGranularity {
            types: Some(
                [
                    GRANULARITY_DOCUMENT,
                    GRANULARITY_PAGE,
                    GRANULARITY_ELEMENT,
                    GRANULARITY_LINE,
                    GRANULARITY_WORD,
                ]
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            ),
        },
        output_format: OutputFormat {
            additional_file_format: OutputAdditionalFileFormat {
                state: STATE_DISABLED.to_string(),
            },
            text_format: OutputTextFormat {
                types: Some(vec![TEXT_FORMAT_PLAIN_TEXT.to_string()]),
            },
        },
    }
}

// ---------------------------------------------------------------------------
// The S3 locations a job reads from and writes to
// ---------------------------------------------------------------------------

/// An Amazon S3 location an `S3Uri` names: the bucket, and the key path after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    /// The key path, empty when the URI named the bucket alone.
    pub key: String,
}

impl S3Location {
    /// The OBJECT an `InputConfiguration$S3Uri` names: a bucket and a non-empty key.
    ///
    /// Read through the tree's ONE reading of an `s3://` URI
    /// ([`::service_common::s3_uri::object_address`]) rather than split here, so a location
    /// this service takes a document from is resolved exactly as every other service
    /// resolves one: the URI's shape by the maintained URL parser, the bucket held to the
    /// names Amazon S3 holds a bucket under, and the key taken as the raw bytes after the
    /// authority.
    #[must_use]
    pub fn object(s3_uri: &str) -> Option<Self> {
        ::service_common::s3_uri::object_address(s3_uri).map(|(bucket, key)| Self { bucket, key })
    }

    /// The PLACE an `OutputConfiguration$S3Uri` names: a bucket and the path under it, which
    /// may be empty — the bucket root. The same one reading
    /// ([`::service_common::s3_uri::bucket_and_prefix`]), because a job's results land in a
    /// place rather than at an object the caller already named.
    #[must_use]
    pub fn place(s3_uri: &str) -> Option<Self> {
        ::service_common::s3_uri::bucket_and_prefix(s3_uri)
            .map(|(bucket, key)| Self { bucket, key })
    }
}

/// The key one job's output object takes under the location its `OutputConfiguration`
/// named.
///
/// The URI names WHERE the results are written, and a job's results are its own, so the
/// object is named by the job's identifier under that path: two jobs writing to one
/// location never overwrite each other, and a caller holding only the `JobArn` can compute
/// the key. `suffix` distinguishes the primary output from the additional file.
#[must_use]
pub fn output_key(location: &S3Location, job_id: &str, suffix: &str) -> String {
    let prefix = &location.key;
    if prefix.is_empty() {
        format!("{job_id}{suffix}")
    } else {
        format!("{}/{job_id}{suffix}", prefix.trim_end_matches('/'))
    }
}

// ---------------------------------------------------------------------------
// Running a job
// ---------------------------------------------------------------------------

/// What a job extracts with: the project's extraction configuration (or the default), the
/// blueprint schemas in play, and the configurations the project asked for that this
/// emulator does not carry out.
#[derive(Debug, Clone)]
pub struct JobPlan {
    pub extraction: ExtractionConfiguration,
    /// One entry per blueprint the job runs under, in the order the request listed them,
    /// each the schema of the version it pinned.
    pub schemas: Vec<String>,
    /// The members of the project that asked for a capability this emulator does not have
    /// ([`unapplied_configurations`]). Non-empty makes the job's own outcome the modelled
    /// `PARTIAL_SUCCESS`.
    pub unapplied: Vec<&'static str>,
}

/// What running a job established.
#[derive(Debug)]
pub enum RunOutcome {
    /// The document was read, the output written, and everything the project asked for was
    /// carried out.
    Succeeded,
    /// The document was read and the output written, and part of what the project asked for
    /// is a capability this emulator does not have: the job reports the modelled
    /// `PARTIAL_SUCCESS` with this reason, so nothing claims the rest was done.
    PartiallySucceeded {
        error_type: String,
        error_message: String,
    },
    /// The job was taken on and could not be run to completion. The caller stores this on
    /// the job, which reports it as `ErrorType` / `ErrorMessage`.
    Failed {
        error_type: String,
        error_message: String,
    },
    /// Amazon Textract cannot take the submit on, and the reason is a fact established
    /// about the request: the object store answered that it holds no such object, or the
    /// object it holds is over a documented size limit. The submit is refused and no job
    /// is created.
    NotTakenOn(Unreadable),
    /// The read did not complete — a panic or a cancellation on the blocking pool, which
    /// says nothing about the document and is answered as the server fault it is.
    DidNotComplete,
}

/// Read the document `input` names, extract it under `plan`, and write the result under
/// `output`.
///
/// This is the whole job, done when it is submitted: the document is read once and the
/// output is stored once, so what `GetDocumentInsights` later reports is backed by an
/// object a caller can really fetch. Only the job's `IN_PROGRESS` -> terminal transition is
/// left to the clock.
pub async fn run_job(
    state: &::engine::ServiceState<super::ComAmazonawsTextractTextractState>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    job_id: &str,
    input: &S3Location,
    output: &S3Location,
    plan: &JobPlan,
) -> RunOutcome {
    let reference = DocumentRef {
        s3: Some(S3Ref {
            bucket: input.bucket.clone(),
            name: input.key.clone(),
            version: None,
        }),
        bytes_digest: None,
    };
    let content = match super::read_submitted_document(state, ctx, &reference, None).await {
        DocumentRead::Read(content) => content,
        DocumentRead::NotTakenOn(reason) => return RunOutcome::NotTakenOn(reason),
        DocumentRead::Unreadable(reason) => {
            return RunOutcome::Failed {
                error_type: error_type_of(&reason).to_string(),
                error_message: reason.message().to_string(),
            }
        }
        DocumentRead::DidNotComplete => return RunOutcome::DidNotComplete,
    };
    // The items the document would expand into are capped BEFORE any of them is built, so
    // the work one submit can cause is bounded by a constant rather than by the caller.
    if super::exceeds_block_ceiling(&content.pages, 0) {
        return RunOutcome::NotTakenOn(Unreadable::TooLarge(format!(
            "the document expands into more than {} items, the most this emulator extracts \
             from one document",
            super::MAX_BLOCKS_PER_DOCUMENT
        )));
    }

    let identity = reference.identity_key();
    let document = extraction_document(&identity, &content, plan);
    let body = ::serde_json::to_vec_pretty(&document).unwrap_or_default();
    if let Err(reason) = put_object(
        ctx,
        output,
        &output_key(output, job_id, ".json"),
        body,
        JSON_MEDIA_TYPE,
    )
    .await
    {
        ::tracing::warn!(
            reason = %reason,
            "a document insights job's output could not be stored"
        );
        return RunOutcome::Failed {
            error_type: INTERNAL_ERROR_TYPE.to_string(),
            error_message: SERVICE_ISSUE_MESSAGE.to_string(),
        };
    }
    // `AdditionalFileFormat` "[c]ontrols whether an additional file format is produced
    // alongside the primary output": the additional file is the document's text, as text.
    if plan.extraction.output_format.additional_file_format.state == STATE_ENABLED {
        if let Err(reason) = put_object(
            ctx,
            output,
            &output_key(output, job_id, ".txt"),
            plain_text(&content).into_bytes(),
            TEXT_MEDIA_TYPE,
        )
        .await
        {
            ::tracing::warn!(
                reason = %reason,
                "a document insights job's additional output could not be stored"
            );
            return RunOutcome::Failed {
                error_type: INTERNAL_ERROR_TYPE.to_string(),
                error_message: SERVICE_ISSUE_MESSAGE.to_string(),
            };
        }
    }
    // The extraction this emulator CAN derive is written either way; what part of the
    // project it could not carry out is reported as the job's own reason.
    if plan.unapplied.is_empty() {
        RunOutcome::Succeeded
    } else {
        RunOutcome::PartiallySucceeded {
            error_type: UNSUPPORTED_CONFIGURATION_ERROR_TYPE.to_string(),
            error_message: unapplied_message(&plan.unapplied),
        }
    }
}

/// The modelled Amazon Textract error that names why a document could not be read, which a
/// failed job reports as its `ErrorType`.
#[must_use]
pub fn error_type_of(reason: &Unreadable) -> &'static str {
    match reason {
        Unreadable::UnsupportedFormat => "UnsupportedDocumentException",
        Unreadable::S3Unreadable(_) => "InvalidS3ObjectException",
        Unreadable::TooLarge(_) => "DocumentTooLargeException",
    }
}

/// Store `body` as the object `key` of `location`'s bucket.
///
/// Through the SAME in-process client the read path uses
/// ([`super::s3_source::s3_client`]), so the object store a job writes to is the one this
/// emulator serves and the one a caller's `GetObject` reads: no socket, no outbound
/// request, no host resolution.
async fn put_object(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    location: &S3Location,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let region = super::request_region(ctx.parts());
    super::s3_source::s3_client(ctx, &region)
        .put_object()
        .bucket(&location.bucket)
        .key(key)
        .content_type(content_type)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
        .map(|_| ())
        .map_err(|error| {
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
        })
}

// ---------------------------------------------------------------------------
// The output document
// ---------------------------------------------------------------------------

/// The whole extraction of one document, as the object a job writes.
///
/// `key` is the document's identity, which is what the item identifiers are derived from,
/// so re-reading one document twice describes it identically.
#[must_use]
pub fn extraction_document(key: &str, content: &DocumentContent, plan: &JobPlan) -> Value {
    let levels = granularity_levels(&plan.extraction.granularity);
    let with_geometry = plan.extraction.bounding_box.state == STATE_ENABLED;
    json!({
        "DocumentMetadata": { "Pages": content.page_count() },
        "Granularity": levels,
        "Elements": elements(key, content, &levels, with_geometry),
        "Text": text_renderings(content, &plan.extraction.output_format.text_format),
        "CustomOutput": custom_output(key, content, &plan.schemas),
    })
}

/// The granularity levels a job extracts at.
///
/// `ExtractionGranularity$Types` are "[t]he granularity levels at which Amazon Textract
/// extracts content, from the whole document down to individual lines of text"; a
/// configuration that names none chooses no narrower set, so every level the model declares
/// is extracted.
fn granularity_levels(granularity: &ExtractionGranularity) -> Vec<String> {
    match &granularity.types {
        Some(types) if !types.is_empty() => types.clone(),
        _ => default_extraction()
            .granularity
            .types
            .unwrap_or_default(),
    }
}

/// One entry per item the document carries at each requested level.
fn elements(
    key: &str,
    content: &DocumentContent,
    levels: &[String],
    with_geometry: bool,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if levels.iter().any(|level| level == GRANULARITY_DOCUMENT) {
        // The whole document is not a block of a page, so it is reported from the text
        // itself: one item, carrying every page's text in reading order.
        out.push(json!({
            "Type": GRANULARITY_DOCUMENT,
            "Text": plain_text(content),
            "Confidence": content.confidence,
        }));
    }
    let wanted: Vec<&str> = levels
        .iter()
        .filter(|level| *level != GRANULARITY_DOCUMENT)
        .flat_map(|level| match level.as_str() {
            GRANULARITY_ELEMENT => ELEMENT_BLOCK_TYPES.to_vec(),
            other => vec![other],
        })
        .collect();
    if wanted.is_empty() {
        return out;
    }
    // ONE derivation of the document's blocks, the same one every read path of this
    // service answers from, with the structural features on so an `ELEMENT` level has
    // something to report.
    let features = FeatureSet {
        forms: true,
        tables: true,
    };
    for block in super::blocks(key, content, features) {
        let block_type = block.block_type.clone().unwrap_or_default();
        if !wanted.contains(&block_type.as_str()) {
            continue;
        }
        let mut item = Map::new();
        item.insert("Type".to_string(), Value::from(block_type));
        if let Some(page) = block.page {
            item.insert("Page".to_string(), Value::from(page));
        }
        if let Some(text) = &block.text {
            item.insert("Text".to_string(), Value::from(text.clone()));
        }
        if let Some(confidence) = block.confidence {
            item.insert("Confidence".to_string(), Value::from(confidence));
        }
        if let Some(entity_types) = &block.entity_types {
            item.insert("EntityTypes".to_string(), Value::from(entity_types.clone()));
        }
        if with_geometry {
            if let Some(geometry) = &block.geometry {
                item.insert("Geometry".to_string(), geometry_value(geometry));
            }
        }
        out.push(Value::Object(item));
    }
    out
}

/// A block's `Geometry` as the output records it: the box around the item, the fine-grained
/// outline inside that box, and the angle the item was read at.
fn geometry_value(geometry: &crate::ops::types::Geometry) -> Value {
    let mut out = Map::new();
    if let Some(box_) = &geometry.bounding_box {
        out.insert(
            "BoundingBox".to_string(),
            json!({
                "Width": box_.width,
                "Height": box_.height,
                "Left": box_.left,
                "Top": box_.top,
            }),
        );
    }
    if let Some(polygon) = &geometry.polygon {
        out.insert(
            "Polygon".to_string(),
            Value::from(
                polygon
                    .iter()
                    .map(|point| json!({ "X": point.x, "Y": point.y }))
                    .collect::<Vec<Value>>(),
            ),
        );
    }
    if let Some(rotation) = geometry.rotation_angle {
        out.insert("RotationAngle".to_string(), Value::from(rotation));
    }
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// The text renderings
// ---------------------------------------------------------------------------

/// The document's text in each requested format.
///
/// `OutputTextFormat$Types` are "[t]he text formats produced in the extraction output"; a
/// format naming none produces the document's plain text, which is what its text is when
/// nothing else was asked for.
fn text_renderings(content: &DocumentContent, text_format: &OutputTextFormat) -> Value {
    let requested = match &text_format.types {
        Some(types) if !types.is_empty() => types.clone(),
        _ => vec![TEXT_FORMAT_PLAIN_TEXT.to_string()],
    };
    let mut out = Map::new();
    for format in requested {
        let rendered = match format.as_str() {
            TEXT_FORMAT_MARKDOWN => Some(markdown_text(content)),
            TEXT_FORMAT_HTML => Some(html_text(content)),
            TEXT_FORMAT_CSV => Some(csv_text(content)),
            TEXT_FORMAT_PLAIN_TEXT => Some(plain_text(content)),
            // A value outside the modelled enum never reaches here: the boundary
            // validator refuses it before the handler runs.
            _ => None,
        };
        if let Some(rendered) = rendered {
            out.insert(format, Value::from(rendered));
        }
    }
    Value::Object(out)
}

/// The document's text as text: its pages in reading order, separated by a blank line.
#[must_use]
pub fn plain_text(content: &DocumentContent) -> String {
    content.pages.join("\n\n")
}

/// The document's text as Markdown: each page under a heading naming it, its lines beneath.
fn markdown_text(content: &DocumentContent) -> String {
    let mut out = String::new();
    for (index, page) in content.pages.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("## Page {}\n\n", index + 1));
        for line in page.lines() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The document's text as HTML: one section per page, one paragraph per line.
///
/// Every line is escaped with the `html-escape` crate, so a document whose text carries
/// markup is reported as that text rather than as markup.
fn html_text(content: &DocumentContent) -> String {
    let mut out = String::from("<!DOCTYPE html>\n<html>\n<body>\n");
    for (index, page) in content.pages.iter().enumerate() {
        out.push_str(&format!(
            "<section><h2>Page {}</h2>\n",
            index + 1
        ));
        for line in page.lines() {
            out.push_str(&format!(
                "<p>{}</p>\n",
                ::html_escape::encode_text(line)
            ));
        }
        out.push_str("</section>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// The document's text as CSV: a row per line, naming the page and the line it was read at.
///
/// Written with the `csv` crate, so a line carrying a separator or a quote is quoted the
/// way the format requires rather than breaking the row.
fn csv_text(content: &DocumentContent) -> String {
    let mut rows: Vec<[String; 3]> = vec![[
        "Page".to_string(),
        "Line".to_string(),
        "Text".to_string(),
    ]];
    for (page_index, page) in content.pages.iter().enumerate() {
        for (line_index, line) in page.lines().enumerate() {
            rows.push([
                (page_index + 1).to_string(),
                (line_index + 1).to_string(),
                line.to_string(),
            ]);
        }
    }
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    for row in &rows {
        if writer.write_record(row).is_err() {
            return String::new();
        }
    }
    writer
        .into_inner()
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The custom, blueprint-driven half
// ---------------------------------------------------------------------------

/// One entry per blueprint the job runs under: the fields its schema asks for, each with
/// the value the document carries for it.
///
/// [`MAX_BLUEPRINT_FIELDS`] is the budget for the WHOLE job rather than for one schema, so
/// the work a submit can cause is bounded by a constant even when it names the thousand
/// blueprints `BlueprintItems` allows.
fn custom_output(key: &str, content: &DocumentContent, schemas: &[String]) -> Vec<Value> {
    if schemas.is_empty() {
        return Vec::new();
    }
    // The document's own labelled fields, read ONCE for every blueprint: the same reading
    // `FORMS` analysis reports and a `QUERIES` answer comes from.
    let fields = super::fields(key, content);
    let mut budget = MAX_BLUEPRINT_FIELDS;
    schemas
        .iter()
        .map(|schema| {
            let wanted = schema_fields(schema, budget);
            budget = budget.saturating_sub(wanted.len());
            let extracted: Vec<Value> = wanted
                .iter()
                .map(|name| match best_field(name, &fields) {
                    Some((field, score)) => json!({
                        "Name": name,
                        "Value": field.value,
                        "Page": field.page_number,
                        // Confidence in the value is the confidence its own characters
                        // were read with, scaled by how much of the field name the
                        // document's label accounts for.
                        "Confidence": (score * field.value_confidence).clamp(0.0, 100.0),
                    }),
                    // The document carries no such field. Reported as absent — nothing is
                    // invented to fill it.
                    None => json!({ "Name": name, "Value": Value::Null }),
                })
                .collect();
            json!({ "Fields": extracted })
        })
        .collect()
}

/// The field names a blueprint schema asks for, at most `budget` of them.
///
/// A schema "defines the fields to extract and their expected structure"
/// (`CreateBlueprintRequest$Schema`), and `CreateBlueprint` accepts only a JSON object, so
/// the fields are the members of its `properties` object. Parsed with `serde_json` — the
/// schema is caller-supplied text, so nothing here hand-reads it — and capped by the
/// caller's remaining budget, which bounds the work one submit can ask for.
fn schema_fields(schema: &str, budget: usize) -> Vec<String> {
    let Ok(Value::Object(parsed)) = ::serde_json::from_str::<Value>(schema) else {
        return Vec::new();
    };
    let Some(Value::Object(properties)) = parsed.get("properties") else {
        return Vec::new();
    };
    properties.keys().take(budget).cloned().collect()
}

/// The document's field that answers `name`, and the share of `name`'s terms its label
/// carries.
///
/// Term-overlap retrieval over the document's own labelled fields, the same rule a
/// `QUERIES` answer is found by: no value is reported unless a field's label really carries
/// at least [`super::MIN_QUERY_MATCH`] of the name's terms.
fn best_field<'a>(name: &str, fields: &'a [super::Field]) -> Option<(&'a super::Field, f32)> {
    let wanted = super::terms(name);
    if wanted.is_empty() {
        return None;
    }
    let mut best: Option<(&super::Field, f32)> = None;
    for field in fields {
        let matched = wanted
            .iter()
            .filter(|term| field.label_terms.contains(term.as_str()))
            .count();
        let score = matched as f32 / wanted.len() as f32;
        if score >= super::MIN_QUERY_MATCH && best.is_none_or(|(_, previous)| score > previous) {
            best = Some((field, score));
        }
    }
    best
}

/// The instant a job whose submit is answered at `now` has its `IN_PROGRESS` -> terminal
/// transition due.
///
/// The same span every other asynchronous job of this service settles over
/// ([`super::JOB_COMPLETION_DELAY`]), so two kinds of job started together settle together.
/// It is never zero, and it is measured from the instant the submit is ANSWERED rather than
/// from the instant it arrived: a caller that polls `GetDocumentInsights` the moment it has
/// the `JobArn` still observes `IN_PROGRESS`, however long reading the document took.
#[must_use]
pub fn settles_at(now: SystemTime) -> SystemTime {
    now + super::JOB_COMPLETION_DELAY
}
