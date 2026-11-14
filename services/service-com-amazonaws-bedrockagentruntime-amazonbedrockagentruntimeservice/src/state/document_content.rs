// SPDX-License-Identifier: CC0-1.0

//! What `GetDocumentContent` may hand a caller: the CONTENT of one ingested document of a
//! knowledge base, and where a caller can fetch it.
//!
//! A knowledge base, its data sources and the documents their ingestions took in all
//! belong to the Bedrock Agent build-time plane, and the bytes of a document taken from an
//! S3 data source belong to the object store. This plane writes neither, so everything
//! here is a READ of those two stores, in that lock order:
//!
//! * a knowledge base, a data source or a document that plane does not hold is the
//!   modelled `ResourceNotFoundException`, never an invented document;
//! * the content a caller is pointed at is the content the ingestion really took in, so
//!   the `mimeType` and the length reported are that content's own and the URL leads to
//!   those very bytes in this emulator — a client that follows it reads what was ingested.
//!
//! WHERE those bytes live is not a property of the request, and the model draws no
//! distinction between the ways a document reaches a knowledge base: a content read
//! answers the same 200 for every document the plane holds. So each way of holding the
//! bytes has an address of its own ([`ContentLocation`]):
//!
//! * a document taken from an OBJECT is addressed at that object in the store that holds
//!   it, so a fetch reads the same bytes a re-ingestion would;
//! * a document whose bytes travelled INLINE in the ingestion request lives in the
//!   build-time plane's own record of it, so it is addressed on this service's own content
//!   plane (`crate::service`), which serves those stored bytes back.
//!
//! An address is PRE-SIGNED, and it also carries an authenticator this service's store
//! minted for exactly that document ([`ContentAddress::cursor`],
//! [`ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState::content_token`]):
//! a pre-signed fetch carries no caller identity of its own, so the authenticator is what
//! stands in for the authorization decision the read made before it minted the address. An
//! address a caller assembled, or one minted for another document, reaches nothing.

use std::time::SystemTime;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;
use ::service_com_amazonaws_s3_amazons3::state as s3_state;

use super::ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState;

/// Why a document's content cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRefusal {
    /// A store this read needs is not on this engine, so nothing can be said about the
    /// document — answered with a modelled server error, never with a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no knowledge base of that identifier.
    UnknownKnowledgeBase,
    /// That knowledge base holds no data source of that identifier.
    UnknownDataSource,
    /// That data source holds no document of that identifier.
    UnknownDocument,
    /// The document names a source object the object store no longer holds, so its content
    /// cannot be described or fetched. The string is the URI the ingestion recorded.
    SourceObjectGone(String),
    /// The ingestion that took the document in carried no content for it at all: its
    /// content names neither an object nor any of the inline members the kind it declares
    /// carries bytes in, so there is nothing to describe or fetch.
    ContentNotIngested,
}

/// Where the content of one ingested document lives, and so what address a caller fetches
/// it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentLocation {
    /// The object the ingestion read the document from, addressed in the object store.
    Object {
        /// The bucket the source object lives in.
        bucket: String,
        /// The key of the source object within that bucket.
        key: String,
    },
    /// Bytes the ingestion request itself carried, held on the build-time plane's record of
    /// the document and served by this service's own content plane.
    Ingested,
}

/// The content of one ingested document: what it is, how long it is, and where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentContent {
    /// The content's own media type, as the store that holds the bytes reports it.
    pub mime_type: String,
    /// How many bytes the content carries.
    pub length: i64,
    /// Where those bytes live, which is what the address a caller is handed leads to.
    pub location: ContentLocation,
}

/// The `DocumentOutputFormat` a request that names none takes: the document as it was
/// ingested (`smithy_query(query="shape",shape="DocumentOutputFormat")` -> RAW,
/// EXTRACTED).
pub const OUTPUT_FORMAT_RAW: &str = "RAW";

/// The `DocumentOutputFormat` naming the text extracted from the document rather than the
/// document itself. See [`OUTPUT_FORMAT_RAW`].
pub const OUTPUT_FORMAT_EXTRACTED: &str = "EXTRACTED";

/// The `InlineContentType` whose bytes are TEXT, the other being carried as bytes with a
/// media type of their own
/// (`smithy_query(query="shape",shape="InlineContentType",model_path="services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json")`
/// -> BYTE, TEXT).
const INLINE_CONTENT_TYPE_TEXT: &str = "TEXT";

/// The media type of inline TEXT content: the ingestion carried the document as text, and
/// `mimeType` reports "the original file type" of the content a read hands out.
const TEXT_CONTENT_MEDIA_TYPE: &str = "text/plain";

/// The content of the document `document_id` of `data_source_id` in `knowledge_base_id`.
///
/// `document_id` is the document's own identity within its data source: the id a custom
/// document was given, or the `s3://` URI an S3 document lives at
/// (`agent_state::DocumentKey`), so both forms of identity resolve here the way the plane
/// that owns them records them.
///
/// READ-ONLY for everything a later caller can observe. The one write on its path is the
/// Bedrock Agent plane materialising the status transitions its own clock had already made
/// due, which is owed whether or not anybody calls.
///
/// LOCK ORDER: the Bedrock Agent plane's store first and released before the object store
/// is reached, the order every cross-plane path of these services keeps.
pub fn document_content<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> Result<DocumentContent, ContentRefusal> {
    match ingested_bytes_of(engine, knowledge_base_id, data_source_id, document_id)? {
        // The bytes the ingestion carried: this service's own content plane serves them, so
        // the content is described from the record that holds them.
        Ingested::Inline { mime_type, bytes } => Ok(DocumentContent {
            mime_type,
            length: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            location: ContentLocation::Ingested,
        }),
        Ingested::Object(source_uri) => object_content(engine, &source_uri),
    }
}

/// The content of the object at `source_uri`, read in the service that OWNS it: its own
/// content type and byte count, and its address in that store.
fn object_content<Eng: Send + Sync + 'static>(
    engine: &Eng,
    source_uri: &str,
) -> Result<DocumentContent, ContentRefusal> {
    let Some((bucket, key)) = split_object_uri(source_uri) else {
        return Err(ContentRefusal::SourceObjectGone(source_uri.to_string()));
    };
    let Some(objects) = s3_state::service_state(engine) else {
        return Err(ContentRefusal::StoreUnavailable);
    };
    let store = objects.lock();
    let object = store
        .bucket(bucket)
        .and_then(|held| held.object(key))
        .ok_or_else(|| ContentRefusal::SourceObjectGone(source_uri.to_string()))?;
    Ok(DocumentContent {
        // The object store's own content type, which is what a `GetObject` of the same
        // object reports; S3 stores its documented default for a write that named none.
        mime_type: object
            .content_type
            .clone()
            .unwrap_or_else(|| s3_state::DEFAULT_CONTENT_TYPE.to_string()),
        length: i64::try_from(object.body.len()).unwrap_or(i64::MAX),
        location: ContentLocation::Object {
            bucket: bucket.to_string(),
            key: key.to_string(),
        },
    })
}

/// The bucket and the key an `s3://` URI addresses, or `None` for text that addresses no
/// object.
fn split_object_uri(uri: &str) -> Option<(&str, &str)> {
    uri.strip_prefix("s3://")?
        .split_once('/')
        .filter(|(bucket, key)| !bucket.is_empty() && !key.is_empty())
}

/// What the ingestion recorded as one document's content.
enum Ingested {
    /// The bytes travelled in the ingestion request, with the media type the kind the
    /// content declares gives them.
    Inline { mime_type: String, bytes: Vec<u8> },
    /// The document IS the object at this `s3://` URI.
    Object(String),
}

/// What the ingestion recorded as the content of that one document, read under the Bedrock
/// Agent plane's own guard.
fn ingested_bytes_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> Result<Ingested, ContentRefusal> {
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(ContentRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());
    if store.live_knowledge_base(knowledge_base_id).is_none() {
        return Err(ContentRefusal::UnknownKnowledgeBase);
    }
    if store
        .live_data_source_of(knowledge_base_id, data_source_id)
        .is_none()
    {
        return Err(ContentRefusal::UnknownDataSource);
    }
    // A document is identified within its data source by the kind of source it came from
    // and its own id in that source, so both kinds the plane records are looked for: the
    // id a custom document was pushed in under, and the URI an S3 document lives at.
    let document = [
        agent_state::DocumentKey::Custom(document_id.to_string()),
        agent_state::DocumentKey::S3(document_id.to_string()),
    ]
    .into_iter()
    .find_map(|key| store.document_of(knowledge_base_id, data_source_id, &key))
    .ok_or(ContentRefusal::UnknownDocument)?;

    // `CustomContent` and `InlineContent` are both discriminated — each documents its own
    // "choose a type and include the field that corresponds to it" — so the bytes are taken
    // from the member the kind the content names carries them in. Content that names an
    // object IS that object, for both the custom content that names one and the content of
    // an S3 data source.
    let content = &document.content;
    if let Some(inline) = content
        .custom
        .as_ref()
        .and_then(|custom| custom.inline_content.as_ref())
    {
        if inline.r#type == INLINE_CONTENT_TYPE_TEXT {
            return inline
                .text_content
                .as_ref()
                .map(|text| Ingested::Inline {
                    mime_type: TEXT_CONTENT_MEDIA_TYPE.to_string(),
                    bytes: text.data.clone().into_bytes(),
                })
                .ok_or(ContentRefusal::ContentNotIngested);
        }
        return inline
            .byte_content
            .as_ref()
            .map(|byte| Ingested::Inline {
                // The media type the ingestion declared for those bytes, which is the
                // document's "original file type" a content read reports.
                mime_type: byte.mime_type.clone(),
                bytes: byte.data.clone(),
            })
            .ok_or(ContentRefusal::ContentNotIngested);
    }
    content
        .s3
        .as_ref()
        .map(|s3| s3.s3_location.uri.clone())
        .or_else(|| {
            content
                .custom
                .as_ref()
                .and_then(|custom| custom.s3_location.as_ref())
                .map(|location| location.uri.clone())
        })
        .map(Ingested::Object)
        .ok_or(ContentRefusal::ContentNotIngested)
}

/// The `ResourceNotFoundException` message a content read answers for a knowledge base,
/// data source or document this emulator holds nothing for.
///
/// One sentence per resource KIND, built from the identifiers the request carried: the
/// variable part is the identifier of the resource that was not found, the rest is fixed
/// text for this error class, as AWS words it for this operation ("Knowledge Base with id
/// <id> does not exist"). The sibling resources are worded the same way, naming the kind
/// that was not found — a client reads the kind and the id out of the sentence to fix its
/// request.
pub fn not_found_message(
    refusal: &ContentRefusal,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> String {
    match refusal {
        ContentRefusal::UnknownDataSource => {
            format!("Data Source with id {data_source_id} does not exist")
        }
        ContentRefusal::UnknownDocument => {
            format!("Document with id {document_id} does not exist")
        }
        ContentRefusal::SourceObjectGone(uri) => format!(
            "Document with id {document_id} does not exist: its source {uri} is no longer \
             held by the object store"
        ),
        ContentRefusal::ContentNotIngested => format!(
            "Document with id {document_id} does not exist: the ingestion that took it in \
             carried no content for it"
        ),
        // Every other refusal is answered with another error entirely; the knowledge base
        // is the resource a request that got no further than its identifier names.
        _ => format!("Knowledge Base with id {knowledge_base_id} does not exist"),
    }
}

/// The first path segment every address of this service's own content plane carries, which
/// is what makes a request one that plane serves. No modelled route of any Bedrock model
/// publishes it, so an address here cannot collide with one.
const CONTENT_ROOT_SEGMENT: &str = "ingested-document-content";

/// The query field an address carries its AUTHENTICATOR in (see the module docs).
pub const DOCUMENT_TOKEN_FIELD: &str = "documentToken";

/// The byte that separates the fields of what an address's authenticator is computed over
/// ([`ContentAddress::cursor`]): a control character, which no identifier of the three can
/// spell (`DocumentId` is `@pattern ^\P{C}*$`).
const CURSOR_SEPARATOR: char = '\u{1f}';

/// Which document an address of this service's content plane names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentAddress {
    /// The knowledge base the document belongs to, by the bare id its plane keys it under.
    pub knowledge_base_id: String,
    /// The data source of that knowledge base the document belongs to.
    pub data_source_id: String,
    /// The document's own id within that data source.
    pub document_id: String,
}

impl ContentAddress {
    /// The address of the document `document_id` of `data_source_id` in
    /// `knowledge_base_id`.
    #[must_use]
    pub fn new(knowledge_base_id: &str, data_source_id: &str, document_id: &str) -> Self {
        Self {
            knowledge_base_id: knowledge_base_id.to_string(),
            data_source_id: data_source_id.to_string(),
            document_id: document_id.to_string(),
        }
    }

    /// The path this document's content is served at, DECODED, as the signer takes it.
    ///
    /// The document id is the address's TAIL because `DocumentId` admits any text but a
    /// control character — a `/` included — and SigV4 leaves a path's separators as
    /// written, so reading the id as everything after the fixed segments names the same
    /// document the URL was signed for whatever the id holds.
    #[must_use]
    pub fn path(&self) -> String {
        format!(
            "/{CONTENT_ROOT_SEGMENT}/{knowledge_base}/{data_source}/{document}",
            knowledge_base = self.knowledge_base_id,
            data_source = self.data_source_id,
            document = self.document_id,
        )
    }

    /// What an address's authenticator is computed over: every part of WHICH document this
    /// is, in one text no two addresses share.
    fn cursor(&self) -> String {
        format!(
            "{knowledge_base}{CURSOR_SEPARATOR}{data_source}{CURSOR_SEPARATOR}{document}",
            knowledge_base = self.knowledge_base_id,
            data_source = self.data_source_id,
            document = self.document_id,
        )
    }
}

/// Whether `path` addresses this service's own content plane: its first segment is the one
/// every such address carries.
///
/// Read by `crate::service` to CLAIM such a request — the model binds no operation to this
/// path space, so the claim cannot be read off the routing table — while WHICH document an
/// address names, and whether this store minted it, are decided by
/// [`parse_content_address`] and [`ingested_content`].
#[must_use]
pub fn addresses_ingested_content(path: &str) -> bool {
    ::service_common::nth_path_segment(path, 0) == Some(CONTENT_ROOT_SEGMENT)
}

/// The document `segments` names — a path's segments, percent-decoded and with the empty
/// ones KEPT — or `None` when they address no document of this plane.
///
/// Built from the SAME parts [`ContentAddress::path`] lays out, so an address this service
/// minted resolves here and nothing else does, and an id is read back exactly as it was
/// written: the id is rejoined from every remaining segment, empty ones included, so two
/// ids differing only in their separators are two addresses rather than one.
#[must_use]
pub fn parse_content_address(segments: &[String]) -> Option<ContentAddress> {
    let (root, rest) = segments.split_first()?;
    if root != CONTENT_ROOT_SEGMENT {
        return None;
    }
    let (knowledge_base_id, rest) = rest.split_first()?;
    let (data_source_id, rest) = rest.split_first()?;
    let document_id = rest.join("/");
    if document_id.is_empty() {
        return None;
    }
    Some(ContentAddress {
        knowledge_base_id: knowledge_base_id.clone(),
        data_source_id: data_source_id.clone(),
        document_id,
    })
}

impl ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState {
    /// The authenticator the address of `target` carries: this store's own tag over WHICH
    /// document the address names ([`ContentAddress::cursor`]), rendered as the hex the
    /// address spells it in.
    ///
    /// Computed under the secret this store drew from the system's randomness when it was
    /// created and never reveals (`service_common::page_tokens::PageTokenKey`, the one
    /// place this workspace authenticates a value a store hands out and takes back). So an
    /// address is a value only this store could have produced: a caller cannot assemble one
    /// for a document it was not handed an address for, and an address another process
    /// minted does not resolve here either.
    #[must_use]
    pub fn content_token(&self, target: &ContentAddress) -> String {
        ::hex::encode(self.document_address_key.mint(&target.cursor()))
    }
}

/// The content the address `target` names, as `(media type, bytes)`, or `None` when this
/// emulator serves no such content to this address.
///
/// `token` is the authenticator the address carries, which is checked against the one this
/// service's store minted for exactly this document BEFORE any document store is read: a
/// pre-signed fetch carries no caller identity of its own, so the authenticator is what
/// stands in for the authorization decision the read that minted the address made. An
/// address this store did not mint, one minted for another document, a document the plane
/// no longer holds, and a document whose bytes did not travel in its ingestion are all
/// answered the same way, so a refusal tells a caller nothing about which it was.
///
/// LOCK ORDER: this service's own store first, released before the build-time plane is
/// reached.
pub fn ingested_content<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &ContentAddress,
    token: &str,
) -> Option<(String, Vec<u8>)> {
    let minted = {
        let state = super::service_state(engine)?;
        let store = state.lock();
        store
            .document_address_key
            .cursor_of(&::hex::decode(token).ok()?)?
    };
    if minted != target.cursor() {
        return None;
    }
    match ingested_bytes_of(
        engine,
        &target.knowledge_base_id,
        &target.data_source_id,
        &target.document_id,
    ) {
        Ok(Ingested::Inline { mime_type, bytes }) => Some((mime_type, bytes)),
        // An address of this plane is minted only for content that travelled in the
        // ingestion; a document whose bytes live in the object store is addressed there.
        Ok(Ingested::Object(_)) | Err(_) => None,
    }
}
