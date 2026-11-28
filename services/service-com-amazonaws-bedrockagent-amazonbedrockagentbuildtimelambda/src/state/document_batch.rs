// SPDX-License-Identifier: CC0-1.0

//! How many documents ONE `IngestKnowledgeBaseDocuments` request may carry, and
//! how a refusal of a batch outside that bound quotes the documents it carried.
//!
//! The bound is NOT the `@length` the model declares on `KnowledgeBaseDocuments`
//! (`min 1, max 10`,
//! `smithy_query(query="shape",shape="KnowledgeBaseDocuments")`): the published
//! model is behind the service on this path, and the front door accepts
//! [`MAX_DOCUMENTS_PER_REQUEST`] documents. So the member carries
//! `skip_validation` in `src/ops/types/ingest_knowledge_base_documents_types.rs`
//! and this module takes the boundary's place, as the ONE statement of both the
//! bound and the sentence a batch outside it is refused with:
//!
//! * the FRONT DOOR's bound ([`documents_length_refusal`]) is a property of the
//!   member and of nothing else — it is judged before any resource the request
//!   names is resolved, and stated in the constraint layer's own envelope
//!   (`N validation error(s) detected: Value '<value>' at '<member>' failed to
//!   satisfy constraint: <requirement>`), with the clause, the requirement and
//!   the envelope all coming from the one place that words them
//!   (`service_common::validation_message`);
//! * the SERVICE's bound ([`knowledge_base_batch_refusal`]) is a property of the
//!   addressed knowledge base's `KnowledgeBaseType`: a knowledge base the
//!   service manages takes [`MAX_MANAGED_DOCUMENTS_PER_REQUEST`] documents per
//!   request, and a batch over that is refused with the service's own sentence
//!   ([`documents_per_request_exceeded_message`]) rather than a constraint
//!   clause, because no modelled constraint expresses it.
//!
//! The value half of the front door's clause is the LIST the request carried,
//! rendered the way a structure is quoted back inside one
//! (`service_common::validation::rendered_record` /`rendered_list`, the tree's
//! one rendering of those forms) with the value of every member the model marks
//! `@sensitive` replaced by [`WITHHELD_VALUE`] — a server does not echo back what
//! the model says not to disclose, and the documents of this operation carry a
//! great many such members (a document's inline text and bytes, a metadata
//! attribute's key and values, a whole access-control entry).
//!
//! A structure is printed over EVERY member its shape declares, one the request
//! left unset as `service_common::validation::UNSET_MEMBER`: that is the form the
//! validation layer IN FRONT of a service prints a modelled structure in, while
//! the client SDKs print only the members that were set — so the member lists
//! here are the model's own, in the order the model declares them, and a member
//! the request omitted is still named.

use ::aws_smithy_schema_server::Violation;
use ::service_common::validation::{
    quoted_echo, rendered_list, rendered_record, UNSET_MEMBER,
};
use ::service_common::validation_message::{envelope_over, valueless_clause};
use ::smithy_server_runtime::ViolationKind;

use super::*;
use crate::ops::types::{
    ByteContentDoc, CustomContent, CustomS3Location, DocumentAccessControlEntry, InlineContent,
    KnowledgeBaseDocument, MetadataAttribute, MetadataAttributeValue, TextContentDoc,
};

/// The member of `IngestKnowledgeBaseDocumentsRequest` the bound is about, as a
/// clause names it.
pub const DOCUMENTS_MEMBER: &str = "documents";

/// The fewest documents one request may carry: the `@length` minimum the model
/// declares on `KnowledgeBaseDocuments`, which the recorded behaviour of this
/// path agrees with — the member is required and a batch is at least one
/// document.
pub const MIN_DOCUMENTS_PER_REQUEST: i64 = 1;

/// The most documents one request may carry past the front door.
///
/// RECORDED, not modelled: the model's `@length` maximum on
/// `KnowledgeBaseDocuments` is 10, and a request carrying 25 documents is
/// accepted on this path while 26 is refused with the constraint clause naming
/// 25 as the bound — so the published maximum is behind the service and this is
/// the bound the front door applies.
pub const MAX_DOCUMENTS_PER_REQUEST: i64 = 25;

/// The most documents one request may carry into a data source of a knowledge
/// base the SERVICE manages ([`MANAGED_KNOWLEDGE_BASE_TYPE`]).
///
/// RECORDED: a MANAGED knowledge base refuses an 11-document batch with the
/// service's own sentence naming 10 as the maximum
/// ([`documents_per_request_exceeded_message`]) — "The number of documents (11)
/// exceeds the maximum allowed (10) for MANAGED knowledge base type.", recorded
/// in `issues/things-that-do-not-work/\
/// 20261127T133852Z-bedrock-agent-refusal-messages-diverge-from-aws-text.md`
/// (cause 5). So the bound this kind of knowledge base applies is the model's
/// own number rather than the front door's, and the sentence is the service's
/// rather than a constraint clause.
pub const MAX_MANAGED_DOCUMENTS_PER_REQUEST: usize = 10;

/// What a rendered structure states in place of the value of a member the model
/// marks `@sensitive`: the value is withheld, and the marker says so where the
/// value would have been.
pub const WITHHELD_VALUE: &str = "*** REDACTED ***";

/// The most documents one clause renders, past which it states no value at all.
///
/// The member carries `skip_validation`, so the length of the list a request
/// sends is bounded by nothing but the request itself, while every document of
/// it renders to text. This is the ceiling on that work, well clear of the bound
/// a batch is judged against ([`MAX_DOCUMENTS_PER_REQUEST`]) so a clause about a
/// batch anywhere near the bound still quotes the batch in full, and past it the
/// clause names the member and the requirement alone — the same fail-safe
/// direction the framework's own walk takes for a value it will not render.
///
/// WHAT ONE DOCUMENT may render to is bounded by the MODEL and not by this
/// constant: `skip_validation` takes the constraints off this one member alone,
/// so the boundary walk still refuses a document before the handler runs unless
/// every nested member satisfies its own `@length` — `inlineAttributes` at most
/// 50 entries, `stringListValue` at most 10, each id, URI and media type bounded
/// in characters, and every `@sensitive` value rendered as the fixed
/// [`WITHHELD_VALUE`] marker however long the value was
/// (`smithy_query(query="closure",operation="IngestKnowledgeBaseDocuments")`).
/// The one repeated member the model bounds only below, `accessControlList`,
/// renders each entry as that same marker — shorter than the JSON entry it came
/// from — so a rendering is never a multiple of the request that produced it.
pub const MAX_QUOTED_DOCUMENTS: usize = 10 * (MAX_DOCUMENTS_PER_REQUEST as usize);

/// The whole `ValidationException` message the FRONT DOOR refuses a `documents`
/// list outside its own bound with, or `None` for a batch inside it.
///
/// A property of the member alone: it is decided before any resource the request
/// names is resolved, exactly as the boundary walk this takes the place of ran
/// before the handler.
pub fn documents_length_refusal(documents: &[KnowledgeBaseDocument]) -> Option<String> {
    let length = i64::try_from(documents.len()).unwrap_or(i64::MAX);
    let kind = if length < MIN_DOCUMENTS_PER_REQUEST {
        ViolationKind::MinLength(MIN_DOCUMENTS_PER_REQUEST)
    } else if length > MAX_DOCUMENTS_PER_REQUEST {
        ViolationKind::MaxLength(MAX_DOCUMENTS_PER_REQUEST)
    } else {
        return None;
    };
    // The list the request carried is the value the clause quotes, held to the
    // ceiling one clause may carry — the same ceiling every clause restored from
    // the framework's own walk is held to, so neither refusal path of this
    // service is sized by the request. A list of more documents than one clause
    // renders ([`MAX_QUOTED_DOCUMENTS`]) is stated with no value at all.
    let value = (documents.len() <= MAX_QUOTED_DOCUMENTS)
        .then(|| quoted_echo(&rendered_documents(documents)).into_owned());
    let violation = Violation {
        path: DOCUMENTS_MEMBER.to_string(),
        kind,
        value,
        sensitive: false,
    };
    let clause = match violation.value {
        Some(_) => violation.message(),
        // `Value null at …` is how an ABSENT member is stated, so a value this
        // declines to render drops the value half of the clause instead.
        None => valueless_clause(&violation).unwrap_or_else(|| violation.message()),
    };
    envelope_over(&[clause])
}

/// The `ValidationException` message the SERVICE refuses a batch of `documents`
/// documents addressed to a knowledge base of `knowledge_base_type` with, or
/// `None` when that kind of knowledge base takes a batch that size.
pub fn knowledge_base_batch_refusal(
    knowledge_base_type: &str,
    documents: usize,
) -> Option<String> {
    (knowledge_base_type == MANAGED_KNOWLEDGE_BASE_TYPE
        && documents > MAX_MANAGED_DOCUMENTS_PER_REQUEST)
        .then(|| {
            documents_per_request_exceeded_message(
                knowledge_base_type,
                documents,
                MAX_MANAGED_DOCUMENTS_PER_REQUEST,
            )
        })
}

/// The sentence a batch over the bound the addressed knowledge base's own kind
/// applies is refused with: the documents the request carried, the maximum that
/// kind takes and the kind itself fill the variable parts, and the rest is fixed
/// text for this error class.
pub fn documents_per_request_exceeded_message(
    knowledge_base_type: &str,
    documents: usize,
    maximum: usize,
) -> String {
    format!(
        "The number of documents ({documents}) exceeds the maximum allowed ({maximum}) for \
         {knowledge_base_type} knowledge base type."
    )
}

/// `documents` as the value half of a clause about the list member quotes it:
/// the elements in position order, comma-separated, in brackets, each document
/// printed as the structure it is.
pub fn rendered_documents(documents: &[KnowledgeBaseDocument]) -> String {
    rendered_sequence(documents, rendered_document)
}

/// `values` as a list renders, each element rendered by `render`, through the
/// tree's one rendering of that form.
fn rendered_sequence<T>(values: &[T], render: impl Fn(&T) -> String) -> String {
    let rendered: Vec<String> = values.iter().map(render).collect();
    let elements: Vec<&str> = rendered.iter().map(String::as_str).collect();
    rendered_list(&elements)
}

/// One structure as a clause prints one: the shape's name over EVERY member the
/// shape declares, each `<member>=<value>`, in the order the model declares
/// them, with a member the request left unset printed as [`UNSET_MEMBER`].
fn record(name: &str, members: &[(&str, String)]) -> String {
    let members: Vec<(&str, &str)> = members
        .iter()
        .map(|(member, value)| (*member, value.as_str()))
        .collect();
    rendered_record(name, &members)
}

/// A member the request left unset, as a printed structure states one.
fn unset() -> String {
    UNSET_MEMBER.to_string()
}

/// The value of a `@sensitive` member, as a printed structure states one.
fn withheld() -> String {
    WITHHELD_VALUE.to_string()
}

/// One `KnowledgeBaseDocument` printed
/// (`smithy_query(query="shape",shape="KnowledgeBaseDocument")`).
fn rendered_document(document: &KnowledgeBaseDocument) -> String {
    record(
        "KnowledgeBaseDocument",
        &[
            (
                "metadata",
                document.metadata.as_ref().map_or_else(unset, rendered_metadata),
            ),
            ("content", rendered_content(&document.content)),
        ],
    )
}

/// One `DocumentMetadata` printed
/// (`smithy_query(query="shape",shape="DocumentMetadata")`).
fn rendered_metadata(metadata: &DocumentMetadata) -> String {
    record(
        "DocumentMetadata",
        &[
            ("type", metadata.r#type.clone()),
            (
                "inlineAttributes",
                metadata.inline_attributes.as_ref().map_or_else(unset, |attributes| {
                    rendered_sequence(attributes, rendered_metadata_attribute)
                }),
            ),
            (
                "s3Location",
                metadata
                    .s3_location
                    .as_ref()
                    .map_or_else(unset, rendered_custom_s3_location),
            ),
            (
                "accessControlList",
                metadata.access_control_list.as_ref().map_or_else(unset, |entries| {
                    rendered_sequence(entries, rendered_access_control_entry)
                }),
            ),
        ],
    )
}

/// One `DocumentAccessControlEntry` printed: the SHAPE itself is `@sensitive`
/// (`smithy_query(query="shape",shape="DocumentAccessControlList")`), so the
/// whole value is withheld rather than printed member by member.
fn rendered_access_control_entry(_entry: &DocumentAccessControlEntry) -> String {
    withheld()
}

/// One `MetadataAttribute` printed: its `key` targets a `@sensitive` shape, so
/// the key is withheld and its value is printed with the same rule applied
/// inside (`smithy_query(query="shape",shape="MetadataAttribute")`).
fn rendered_metadata_attribute(attribute: &MetadataAttribute) -> String {
    record(
        "MetadataAttribute",
        &[
            ("key", withheld()),
            ("value", rendered_metadata_value(&attribute.value)),
        ],
    )
}

/// One `MetadataAttributeValue` printed: `type` names which sibling carries the
/// value, and of those siblings the model marks the number, the string and the
/// elements of the string list `@sensitive`, leaving the boolean disclosed
/// (`smithy_query(query="shape",shape="MetadataAttributeValue")`).
fn rendered_metadata_value(value: &MetadataAttributeValue) -> String {
    record(
        "MetadataAttributeValue",
        &[
            ("type", value.r#type.clone()),
            (
                "numberValue",
                value.number_value.map_or_else(unset, |_| withheld()),
            ),
            (
                "booleanValue",
                value.boolean_value.map_or_else(unset, |held| held.to_string()),
            ),
            (
                "stringValue",
                value.string_value.as_ref().map_or_else(unset, |_| withheld()),
            ),
            (
                "stringListValue",
                value.string_list_value.as_ref().map_or_else(unset, |values| {
                    rendered_sequence(values, |_: &String| withheld())
                }),
            ),
        ],
    )
}

/// One `DocumentContent` printed: its `dataSourceType` names which sibling
/// carries the document's own details
/// (`smithy_query(query="shape",shape="DocumentContent")`).
fn rendered_content(content: &DocumentContent) -> String {
    record(
        "DocumentContent",
        &[
            ("dataSourceType", content.data_source_type.clone()),
            (
                "custom",
                content.custom.as_ref().map_or_else(unset, rendered_custom_content),
            ),
            (
                "s3",
                content.s3.as_ref().map_or_else(unset, rendered_s3_content),
            ),
        ],
    )
}

/// One `CustomContent` printed
/// (`smithy_query(query="shape",shape="CustomContent")`).
fn rendered_custom_content(custom: &CustomContent) -> String {
    record(
        "CustomContent",
        &[
            (
                "customDocumentIdentifier",
                rendered_custom_identifier(&custom.custom_document_identifier),
            ),
            ("sourceType", custom.source_type.clone()),
            (
                "s3Location",
                custom
                    .s3_location
                    .as_ref()
                    .map_or_else(unset, rendered_custom_s3_location),
            ),
            (
                "inlineContent",
                custom
                    .inline_content
                    .as_ref()
                    .map_or_else(unset, rendered_inline_content),
            ),
        ],
    )
}

/// One `CustomDocumentIdentifier` printed: the id is the caller's own choice of
/// name for the document and no member of it is `@sensitive`
/// (`smithy_query(query="shape",shape="CustomDocumentIdentifier")`).
fn rendered_custom_identifier(identifier: &CustomDocumentIdentifier) -> String {
    record("CustomDocumentIdentifier", &[("id", identifier.id.clone())])
}

/// One `InlineContent` printed: `type` names which of the two content members
/// carries the document's own bytes
/// (`smithy_query(query="shape",shape="InlineContent")`).
fn rendered_inline_content(inline: &InlineContent) -> String {
    record(
        "InlineContent",
        &[
            ("type", inline.r#type.clone()),
            (
                "byteContent",
                inline
                    .byte_content
                    .as_ref()
                    .map_or_else(unset, rendered_byte_content),
            ),
            (
                "textContent",
                inline
                    .text_content
                    .as_ref()
                    .map_or_else(unset, rendered_text_content),
            ),
        ],
    )
}

/// One `ByteContentDoc` printed: its `data` targets a `@sensitive` blob, so the
/// bytes are withheld while the media type they were sent as is not
/// (`smithy_query(query="shape",shape="ByteContentDoc")`).
fn rendered_byte_content(bytes: &ByteContentDoc) -> String {
    record(
        "ByteContentDoc",
        &[
            ("mimeType", bytes.mime_type.clone()),
            ("data", withheld()),
        ],
    )
}

/// One `TextContentDoc` printed: its `data` — the document's own text — targets
/// a `@sensitive` shape, so it is withheld
/// (`smithy_query(query="shape",shape="TextContentDoc")`).
fn rendered_text_content(_text: &TextContentDoc) -> String {
    record("TextContentDoc", &[("data", withheld())])
}

/// One `S3Content` printed
/// (`smithy_query(query="shape",shape="S3Content")`).
fn rendered_s3_content(content: &S3Content) -> String {
    record(
        "S3Content",
        &[("s3Location", rendered_s3_location(&content.s3_location))],
    )
}

/// One `S3Location` printed: where the document lives, which no member marks
/// `@sensitive` (`smithy_query(query="shape",shape="S3Location")`).
fn rendered_s3_location(location: &S3Location) -> String {
    record("S3Location", &[("uri", location.uri.clone())])
}

/// One `CustomS3Location` printed
/// (`smithy_query(query="shape",shape="CustomS3Location")`).
fn rendered_custom_s3_location(location: &CustomS3Location) -> String {
    record(
        "CustomS3Location",
        &[
            ("uri", location.uri.clone()),
            (
                "bucketOwnerAccountId",
                location
                    .bucket_owner_account_id
                    .clone()
                    .unwrap_or_else(unset),
            ),
        ],
    )
}
