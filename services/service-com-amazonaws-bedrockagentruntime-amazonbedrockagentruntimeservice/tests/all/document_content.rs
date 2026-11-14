// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDocumentContent`
//! (`POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents/{documentId}/content`).
//!
//! THE RULES UNDER TEST:
//!
//! * a content read answers WHERE the content of one ingested document lives: the
//!   `presignedUrl` addresses the content the ingestion really took in, in this emulator,
//!   and `mimeType` and `documentContentLength` are that content's own media type and byte
//!   count. Following the URL yields exactly those bytes. HOW the document reached the
//!   knowledge base is no part of the request: a document taken from an object is addressed
//!   at that object, and one whose bytes travelled inline in the ingestion is addressed on
//!   this service's own content plane — which serves it only to the address the read that
//!   was authorized for it handed out;
//! * the knowledge base, the data source and the document are the Bedrock Agent build-time
//!   plane's, so each one that plane does not hold is the modelled
//!   `ResourceNotFoundException` (404), naming what could not be found;
//! * the authorization decision comes FIRST: a caller whose identity-based policies do not
//!   allow `bedrock:GetDocumentContent` is refused with `AccessDeniedException` (403)
//!   whether or not the document exists, while a permitted caller making the identical
//!   call reaches the service — which is what proves the first answer was authorization.
//!
//! The tables span what those rules generalise over: the content type and the size of the
//! object a document was ingested from, which half of the identity is unknown, the
//! `outputFormat` and `userContext` legs this emulator cannot serve, and where the caller's
//! grant comes from.
//!
//! The knowledge base, the data source and the documents are all created through the
//! build-time plane's own API; the objects are written through S3's own store, the way the
//! retrieval suite beside this one writes them.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Value};

use service_com_amazonaws_s3_amazons3::state as s3_state;
use service_com_amazonaws_s3vectors_s3vectors::state as vectors_state;

use crate::agent_memory::{allow, Emulator, Reply, ACCESS_DENIED, ACCOUNT_KEY, NOT_FOUND, REGION};
use crate::sessions::label;

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the build-time plane's modelled `KnowledgeBaseRoleArn`.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket the knowledge bases here bind to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/document-content-vectors";

/// The index inside [`VECTOR_BUCKET`] every knowledge base here binds to.
const VECTOR_INDEX: &str = "document-content-index";

/// The number of values a vector of [`EMBEDDING_MODEL`] carries, which is the dimension
/// the bound index is created with.
const EMBEDDING_DIMENSION: i32 = 1024;

/// The `dataType` of the index: the one value the modelled `DataType` enum admits.
const VECTOR_DATA_TYPE: &str = "float32";

/// The bucket the documents' source objects live in.
const BUCKET: &str = "document-content-sources";

/// A `knowledgeBaseId` the plane never minted (`^[0-9a-zA-Z]{10}$`).
const UNKNOWN_KNOWLEDGE_BASE: &str = "ABCDEFGHIJ";

/// A `dataSourceId` the plane never minted.
const UNKNOWN_DATA_SOURCE: &str = "DATASRC001";

/// One custom data source holding documents whose content is in S3, and the ids they were
/// ingested under.
struct Ingested {
    knowledge_base_id: String,
    data_source_id: String,
}

impl Emulator {
    /// Create a VECTOR knowledge base and answer its minted id.
    async fn knowledge_base(&self, name: &str) -> String {
        self.seed_index();
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "PUT",
                "/knowledgebases/",
                Some(json!({
                    "name": name,
                    "roleArn": KB_ROLE,
                    "knowledgeBaseConfiguration": {
                        "type": "VECTOR",
                        "vectorKnowledgeBaseConfiguration": {
                            "embeddingModelArn": EMBEDDING_MODEL
                        },
                    },
                    "storageConfiguration": {
                        "type": "S3_VECTORS",
                        "s3VectorsConfiguration": {
                            "vectorBucketArn": VECTOR_BUCKET,
                            "indexName": VECTOR_INDEX,
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 202,
            "setup: CreateKnowledgeBase must answer 202, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        reply.json(name)["knowledgeBase"]["knowledgeBaseId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no knowledgeBaseId in {:?}", reply.json(name)))
            .to_string()
    }

    /// Create [`VECTOR_BUCKET`] and the index inside it a knowledge base binds to — the
    /// store a caller sets up before naming it, which the build-time plane's
    /// `CreateKnowledgeBase` checks exists.
    fn seed_index(&self) {
        let bucket = vectors_state::bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
            .expect("the suite's vectorBucketArn is a vector bucket ARN");
        let state = vectors_state::service_state(&*self.engine)
            .expect("the S3 Vectors store is on this engine");
        let mut store = state.lock();
        let held = store
            .buckets
            .entry(bucket.clone())
            .or_insert_with(|| vectors_state::VectorBucketRecord {
                name: bucket.clone(),
                arn: VECTOR_BUCKET.to_string(),
                creation_time: std::time::SystemTime::now(),
                encryption: vectors_state::EncryptionRecord::default(),
                default_index_mode: vectors_state::DEFAULT_INDEX_MODE.to_string(),
                policy: None,
                indexes: BTreeMap::new(),
            });
        let encryption = held.encryption.clone();
        let index_mode = held.default_index_mode.clone();
        held.indexes.insert(
            VECTOR_INDEX.to_string(),
            vectors_state::VectorIndexRecord {
                bucket_name: bucket.clone(),
                name: VECTOR_INDEX.to_string(),
                arn: format!("{VECTOR_BUCKET}/index/{VECTOR_INDEX}"),
                creation_time: std::time::SystemTime::now(),
                data_type: VECTOR_DATA_TYPE.to_string(),
                dimension: EMBEDDING_DIMENSION,
                distance_metric: vectors_state::DISTANCE_METRIC_COSINE.to_string(),
                non_filterable_metadata_keys: None,
                encryption,
                index_mode,
                vectors: BTreeMap::new(),
                tombstones: std::collections::VecDeque::new(),
            },
        );
    }

    /// Attach a CUSTOM data source — the kind a caller pushes documents into — and answer
    /// its minted id.
    async fn custom_data_source(&self, knowledge_base_id: &str, name: &str) -> String {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(json!({
                    "name": name,
                    "dataSourceConfiguration": { "type": "CUSTOM" },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "setup: CreateDataSource must answer 200, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        reply.json(name)["dataSource"]["dataSourceId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no dataSourceId in {:?}", reply.json(name)))
            .to_string()
    }

    /// Store `body` at `BUCKET`/`key` with `content_type`, creating the bucket on the first
    /// write — the state a caller's `CreateBucket` + `PutObject` leaves behind, written
    /// through S3's own store so the object a content read describes is the object S3
    /// serves.
    fn put_object(&self, key: &str, body: &[u8], content_type: Option<&str>) {
        let state = s3_state::service_state(&*self.engine).expect("the S3 store is on this engine");
        let mut store = state.lock();
        let _ = store.create_bucket(BUCKET, REGION);
        let written = store.put_object(
            BUCKET,
            key,
            s3_state::Object::stored(body.to_vec(), content_type.map(str::to_string)),
        );
        assert!(
            written.is_some(),
            "setup: expected s3://{BUCKET}/{key} to be stored, got {written:?}"
        );
    }

    /// Push `documents` into a custom data source through the build-time plane's own
    /// `IngestKnowledgeBaseDocuments`.
    async fn ingest_documents(&self, case: &str, ingested: &Ingested, documents: Value) {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "PUT",
                &format!(
                    "/knowledgebases/{}/datasources/{}/documents",
                    ingested.knowledge_base_id, ingested.data_source_id
                ),
                Some(json!({ "documents": documents })),
            )
            .await;
        assert_eq!(
            reply.status, 202,
            "setup ({case}): IngestKnowledgeBaseDocuments must answer 202, got {} {:?}",
            reply.status,
            reply.json(case)
        );
    }

    /// `GetDocumentContent` over one document of one data source.
    async fn document_content(
        &self,
        key: &str,
        knowledge_base_id: &str,
        data_source_id: &str,
        document_id: &str,
        body: Option<Value>,
    ) -> Reply {
        self.bedrock(
            key,
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /documents/{document_id}/content"
            ),
            body,
        )
        .await
    }

    /// Fetch a URL this emulator signed, the way a client does: the whole URL, through the
    /// same router, with no credential of its own.
    async fn follow(&self, case: &str, url: &str) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method("GET")
            .uri(url)
            .body(bytes::Bytes::new())
            .unwrap_or_else(|error| panic!("case {case:?}: {url} must be a URL ({error})"));
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.into_body())
    }

    /// A knowledge base with one custom data source, ready to be pushed documents.
    async fn custom_source(&self, name: &str) -> Ingested {
        let knowledge_base_id = self.knowledge_base(name).await;
        let data_source_id = self.custom_data_source(&knowledge_base_id, name).await;
        Ingested {
            knowledge_base_id,
            data_source_id,
        }
    }
}

/// One document pushed in with its content in S3.
fn s3_document(id: &str, key: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "S3_LOCATION",
                "s3Location": { "uri": format!("s3://{BUCKET}/{key}") },
            },
        },
    })
}

/// A `documentId` as a client sends it in a non-greedy `@httpLabel`: `DocumentId` admits any
/// text but a control character (`@pattern ^\P{C}*$`), so the characters a URL path cannot
/// carry verbatim are percent-encoded.
fn document_label(id: &str) -> String {
    id.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace(' ', "%20")
}

/// One document pushed in with its content defined inline in TEXT.
fn inline_text_document(id: &str, text: &str) -> Value {    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "TEXT",
                    "textContent": { "data": text },
                },
            },
        },
    })
}

/// One document pushed in with its content defined inline in BYTES, which travel
/// base64-encoded on the wire (`ByteContentDoc$data` is a `@blob`), under the media type
/// the ingestion declares for them.
fn inline_byte_document(id: &str, mime_type: &str, bytes: &[u8]) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "BYTE",
                    "byteContent": {
                        "mimeType": mime_type,
                        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                    },
                },
            },
        },
    })
}

/// RULE: HOW a document's bytes reached the knowledge base is not a property of the
/// request — the model draws no distinction between a document crawled from an object store
/// and one whose bytes travelled in the ingestion request — so a content read of ANY
/// document the plane holds is a 200 reporting that content's own media type and byte count
/// with a pre-signed URL this emulator answers, whose `GET` yields the ingested bytes
/// byte for byte.
///
/// The rows span what the rule generalises over: the kind of inline content (text, bytes),
/// the media type the ingestion declared for it, the published minimum length of the
/// content, text whose byte count differs from its character count, and a `documentId`
/// carrying the separators and spaces a URL has to survive.
#[tokio::test]
async fn a_content_read_serves_a_document_whose_bytes_travelled_in_its_ingestion() {
    /// One inline-ingested document, and the content a read of it must report.
    struct Row {
        case: &'static str,
        document_id: &'static str,
        /// The document as `IngestKnowledgeBaseDocuments` takes it in.
        ingested: Value,
        /// The `mimeType` the read must report.
        reported_mime_type: &'static str,
        /// The bytes the pre-signed URL must yield, which are the ingested ones.
        content: Vec<u8>,
    }

    let binary: Vec<u8> = vec![0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x00, 0xff, 0xfe, 0x0a];
    let rows = vec![
        Row {
            case: "text defined inline",
            document_id: "inline-text",
            ingested: inline_text_document("inline-text", "the pump is serviced every 90 days"),
            reported_mime_type: "text/plain",
            content: b"the pump is serviced every 90 days".to_vec(),
        },
        Row {
            case: "inline text whose byte count is not its character count",
            document_id: "inline-unicode",
            ingested: inline_text_document("inline-unicode", "résumé\nabrégé — ✓\n"),
            reported_mime_type: "text/plain",
            content: "résumé\nabrégé — ✓\n".as_bytes().to_vec(),
        },
        Row {
            case: "inline text at the published minimum length of one character",
            document_id: "inline-minimal",
            ingested: inline_text_document("inline-minimal", "x"),
            reported_mime_type: "text/plain",
            content: b"x".to_vec(),
        },
        Row {
            case: "bytes defined inline under the media type the ingestion declared",
            document_id: "inline-pdf",
            ingested: inline_byte_document("inline-pdf", "application/pdf", &binary),
            reported_mime_type: "application/pdf",
            content: binary.clone(),
        },
        Row {
            case: "inline bytes under another declared media type",
            document_id: "reports/2026 Q1/notes.md",
            ingested: inline_byte_document(
                "reports/2026 Q1/notes.md",
                "text/markdown",
                b"# notes\n\n- one\n- two\n",
            ),
            reported_mime_type: "text/markdown",
            content: b"# notes\n\n- one\n- two\n".to_vec(),
        },
        Row {
            // `ByteContentDoc$mimeType` is a `String` whose `@pattern [a-z]{1,20}/.{1,20}`
            // (`smithy_query(query="shape",shape="ByteContentDoc")`) admits characters no
            // HTTP header value can carry, so the read reports the declared type as stored
            // and the fetch still serves the bytes within a bounded response.
            case: "a declared media type no HTTP header value can carry",
            document_id: "inline-hostile-type",
            ingested: inline_byte_document(
                "inline-hostile-type",
                "text/plain\r\nx-injected: yes",
                b"the bytes are served whatever the declared type says",
            ),
            reported_mime_type: "text/plain\r\nx-injected: yes",
            content: b"the bytes are served whatever the declared type says".to_vec(),
        },
    ];

    let emulator = Emulator::new();
    let source = emulator.custom_source("inline-content").await;
    for row in &rows {
        emulator
            .ingest_documents(row.case, &source, json!([row.ingested.clone()]))
            .await;
    }

    for row in &rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                &document_label(row.document_id),
                None,
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        let answer = reply.json(row.case);
        assert_eq!(
            answer["mimeType"].as_str(),
            Some(row.reported_mime_type),
            "case {:?}: expected the content's own media type, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer["documentContentLength"].as_i64(),
            Some(row.content.len() as i64),
            "case {:?}: expected the ingested byte count, got {answer:?}",
            row.case
        );
        let url = answer["presignedUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no presignedUrl in {answer:?}", row.case))
            .to_string();
        assert!(
            url.contains("X-Amz-Signature="),
            "case {:?}: the URL must be a pre-signed address, got {url}",
            row.case
        );
        let (status, body) = emulator.follow(row.case, &url).await;
        assert_eq!(
            status,
            200,
            "case {:?}: following {url} must reach the content, got {status} {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            body.as_ref(),
            row.content.as_slice(),
            "case {:?}: following the URL must yield the ingested bytes, got {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
    }
}

/// RULE: the address a content read hands out is good for the ONE document it was minted
/// for: it carries an authenticator only this emulator's store can produce, so an address a
/// caller assembled, one whose authenticator was altered, and one re-pointed at another
/// document all reach nothing.
#[tokio::test]
async fn an_address_no_content_read_minted_reaches_nothing() {
    let emulator = Emulator::new();
    let source = emulator.custom_source("inline-addresses").await;
    for (id, text) in [("inline-one", "the first document"), ("inline-two", "the second")] {
        emulator
            .ingest_documents(id, &source, json!([inline_text_document(id, text)]))
            .await;
    }

    let mut addresses: Vec<String> = Vec::new();
    for document in ["inline-one", "inline-two"] {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                document,
                None,
            )
            .await;
        addresses.push(
            reply.json(document)["presignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("setup: no presignedUrl for {document}"))
                .to_string(),
        );
    }
    let first = addresses[0].clone();
    let second = addresses[1].clone();

    // (case, the address that is fetched) — each one differs from the minted address in a
    // single way, and none of them is one this emulator handed out.
    let token_field = "documentToken=";
    let altered = {
        let (head, tail) = first
            .split_once(token_field)
            .expect("a minted address carries the authenticator");
        let (token, rest) = tail.split_once('&').unwrap_or((tail, ""));
        // One hex digit of the authenticator changed, which no other document's tag is.
        let flipped = match token.starts_with('0') {
            true => format!("1{}", &token[1..]),
            false => format!("0{}", &token[1..]),
        };
        format!("{head}{token_field}{flipped}&{rest}")
    };
    let without_token = {
        let (head, tail) = first
            .split_once(token_field)
            .expect("a minted address carries the authenticator");
        let rest = tail.split_once('&').map(|(_, rest)| rest).unwrap_or("");
        format!("{head}{rest}")
    };
    // The authenticator of the second document on the first document's path.
    let swapped = {
        let (head, _) = first
            .split_once(token_field)
            .expect("a minted address carries the authenticator");
        let (_, tail) = second
            .split_once(token_field)
            .expect("a minted address carries the authenticator");
        format!("{head}{token_field}{tail}")
    };
    let unsigned = first
        .split_once('?')
        .map(|(path, _)| path.to_string())
        .expect("a minted address carries a query");

    for (case, url) in [
        ("an authenticator one digit away from the minted one", altered),
        ("an address carrying no authenticator at all", without_token),
        ("another document's authenticator", swapped),
        ("the same path with no pre-signed query", unsigned),
    ] {
        let (status, body) = emulator.follow(case, &url).await;
        assert_ne!(
            status,
            200,
            "case {case:?}: {url} must not serve any content, got {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    // The positive control: the address this emulator really minted still serves.
    let (status, body) = emulator.follow("the minted address", &first).await;
    assert_eq!(
        (status, String::from_utf8_lossy(&body).to_string()),
        (200, "the first document".to_string()),
        "the minted address must serve the document it was minted for"
    );
}

/// RULE: a content read describes and addresses the OBJECT the ingestion took the document
/// from — its own content type and byte count — and the URL it answers leads back to this
/// emulator and yields exactly those bytes.
#[tokio::test]
async fn a_content_read_addresses_the_object_the_document_was_ingested_from() {
    /// One document, and the object it was ingested from.
    struct Row {
        case: &'static str,
        document_id: &'static str,
        key: &'static str,
        body: &'static str,
        /// The content type the object was stored with, if any.
        stored_content_type: Option<&'static str>,
        /// The `mimeType` the read must report for it: the stored one, or S3's documented
        /// default for an object written without one.
        reported_mime_type: &'static str,
    }

    let rows = [
        Row {
            case: "a text document",
            document_id: "doc-text",
            key: "notes/first.txt",
            body: "the quick brown fox",
            stored_content_type: Some("text/plain"),
            reported_mime_type: "text/plain",
        },
        Row {
            case: "a larger document of another type",
            document_id: "doc-csv",
            key: "tables/rows.csv",
            body: "a,b,c\n1,2,3\n4,5,6\n",
            stored_content_type: Some("text/csv"),
            reported_mime_type: "text/csv",
        },
        Row {
            case: "an object stored without a content type",
            document_id: "doc-untyped",
            key: "blobs/opaque.bin",
            body: "\u{1}\u{2}\u{3} opaque bytes",
            stored_content_type: None,
            reported_mime_type: s3_state::DEFAULT_CONTENT_TYPE,
        },
        Row {
            case: "an empty document",
            document_id: "doc-empty",
            key: "blobs/empty.txt",
            body: "",
            stored_content_type: Some("text/plain"),
            reported_mime_type: "text/plain",
        },
    ];

    let emulator = Emulator::new();
    let source = emulator.custom_source("content-reads").await;
    for row in &rows {
        emulator.put_object(row.key, row.body.as_bytes(), row.stored_content_type);
        emulator
            .ingest_documents(
                row.case,
                &source,
                json!([s3_document(row.document_id, row.key)]),
            )
            .await;
    }

    for row in &rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                row.document_id,
                None,
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        let answer = reply.json(row.case);
        assert_eq!(
            answer["mimeType"].as_str(),
            Some(row.reported_mime_type),
            "case {:?}: expected the object's own content type, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer["documentContentLength"].as_i64(),
            Some(row.body.len() as i64),
            "case {:?}: expected the object's byte count, got {answer:?}",
            row.case
        );
        let url = answer["presignedUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no presignedUrl in {answer:?}", row.case))
            .to_string();
        assert!(
            url.contains(&format!("/{BUCKET}/{}", row.key)) && url.contains("X-Amz-Signature="),
            "case {:?}: the URL must be a signed address of the object, got {url}",
            row.case
        );
        let (status, body) = emulator.follow(row.case, &url).await;
        assert_eq!(
            status, 200,
            "case {:?}: following {url} must reach the object, got {status} {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            String::from_utf8_lossy(&body),
            row.body,
            "case {:?}: following the URL must yield the stored bytes",
            row.case
        );
    }
}

/// RULE: each half of a document's identity the build-time plane does not hold is the
/// modelled `ResourceNotFoundException` (404), whose message names what could not be
/// found.
#[tokio::test]
async fn a_content_read_answers_not_found_for_what_the_plane_does_not_hold() {
    let emulator = Emulator::new();
    let source = emulator.custom_source("content-not-found").await;
    emulator.put_object("held/one.txt", b"held", Some("text/plain"));
    emulator
        .ingest_documents(
            "the document that exists",
            &source,
            json!([s3_document("doc-held", "held/one.txt")]),
        )
        .await;

    /// One read of something the plane does not hold, and the identifier the refusal has
    /// to name.
    struct Row {
        case: &'static str,
        knowledge_base: Option<&'static str>,
        data_source: Option<&'static str>,
        document: &'static str,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "a knowledge base that was never created",
            knowledge_base: Some(UNKNOWN_KNOWLEDGE_BASE),
            data_source: None,
            document: "doc-held",
            names: UNKNOWN_KNOWLEDGE_BASE,
        },
        Row {
            case: "a data source that was never created",
            knowledge_base: None,
            data_source: Some(UNKNOWN_DATA_SOURCE),
            document: "doc-held",
            names: UNKNOWN_DATA_SOURCE,
        },
        Row {
            case: "a document that was never ingested",
            knowledge_base: None,
            data_source: None,
            document: "doc-never-ingested",
            names: "doc-never-ingested",
        },
    ];

    for row in rows {
        let knowledge_base = row.knowledge_base.unwrap_or(&source.knowledge_base_id);
        let data_source = row.data_source.unwrap_or(&source.data_source_id);
        let reply = emulator
            .document_content(ACCOUNT_KEY, knowledge_base, data_source, row.document, None)
            .await;
        assert_eq!(
            (reply.error_code(row.case).as_str(), reply.status),
            NOT_FOUND,
            "case {:?}: expected {NOT_FOUND:?}, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.names),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.names,
            reply.message(row.case)
        );
    }
}

/// RULE: a leg this emulator cannot compute is refused with the modelled
/// `ValidationException` (400) naming the member, rather than answered with a URL that
/// leads nowhere or content the caller asked to have filtered.
#[tokio::test]
async fn a_content_read_refuses_a_leg_it_cannot_compute() {
    let emulator = Emulator::new();
    let source = emulator.custom_source("content-refusals").await;
    emulator.put_object("held/two.txt", b"held", Some("text/plain"));
    emulator
        .ingest_documents(
            "the document that exists",
            &source,
            json!([s3_document("doc-in-s3", "held/two.txt")]),
        )
        .await;

    /// One request that cannot be served as asked, and the member the refusal must name.
    struct Row {
        case: &'static str,
        document: &'static str,
        body: Option<Value>,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "the extracted rendering of a document",
            document: "doc-in-s3",
            body: Some(json!({ "outputFormat": "EXTRACTED" })),
            names: "outputFormat",
        },
        Row {
            case: "a read narrowed to one user's documents",
            document: "doc-in-s3",
            body: Some(json!({ "userContext": { "dataAccessPolicyArn":
                "arn:aws:bedrock:us-west-2:000000000000:data-access-policy/ABCDEFGHIJ" } })),
            names: "userContext",
        },
    ];

    for row in rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                row.document,
                row.body.clone(),
            )
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        assert_eq!(
            reply.error_code(row.case),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.names),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.names,
            reply.message(row.case)
        );
    }
}

/// RULE: a knowledge-base ARN identifies ONE knowledge base in ONE place, so an identifier
/// naming another account, another region or another partition names no knowledge base this
/// call can read — it is answered not-found rather than reduced to the bare id, which would
/// have resolved in the caller's own place. The caller's own ARN for the same knowledge base
/// is served, which is what proves the refusals are about the place and not the id.
#[tokio::test]
async fn an_identifier_naming_another_place_reads_no_document() {
    let emulator = Emulator::new();
    let source = emulator.custom_source("content-isolation").await;
    emulator.put_object("held/four.txt", b"held", Some("text/plain"));
    emulator
        .ingest_documents(
            "the document that exists",
            &source,
            json!([s3_document("doc-held", "held/four.txt")]),
        )
        .await;
    let held = &source.knowledge_base_id;

    // (case, the identifier the request carries, the status it earns). The forms are the
    // ones the modelled `KnowledgeBaseIdentifier` pattern admits: the bare id and a
    // knowledge-base ARN of any partition, region and account.
    let rows = [
        (
            "the bare id",
            held.clone(),
            200,
        ),
        (
            "this account's own ARN in the region the request is signed for",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/{held}"),
            200,
        ),
        (
            "an ARN of another account",
            format!("arn:aws:bedrock:{REGION}:210987654321:knowledge-base/{held}"),
            NOT_FOUND.1,
        ),
        (
            "an ARN of another region",
            format!("arn:aws:bedrock:eu-central-1:{ACCOUNT}:knowledge-base/{held}"),
            NOT_FOUND.1,
        ),
        (
            "an ARN of another partition",
            format!("arn:aws-cn:bedrock:cn-north-1:{ACCOUNT}:knowledge-base/{held}"),
            NOT_FOUND.1,
        ),
    ];

    for (case, identifier, expected) in rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &label(&identifier),
                &source.data_source_id,
                "doc-held",
                None,
            )
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected {expected} for {identifier:?}, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        if expected == NOT_FOUND.1 {
            assert_eq!(
                reply.error_code(case),
                NOT_FOUND.0,
                "case {case:?}: expected {}, got {:?}",
                NOT_FOUND.0,
                reply.json(case)
            );
        }
    }
}

/// RULE: the authorization decision is taken before the document is resolved, so an
/// unpermitted caller is refused with `AccessDeniedException` (403) on a document that
/// exists AND on one that does not, while the permitted caller making the identical calls
/// gets the document and the resource's own 404.
#[tokio::test]
async fn an_unpermitted_caller_is_refused_before_the_document_is_resolved() {
    let emulator = Emulator::new();
    let source = emulator.custom_source("content-authorization").await;
    emulator.put_object("held/three.txt", b"held", Some("text/plain"));
    emulator
        .ingest_documents(
            "the document that exists",
            &source,
            json!([s3_document("doc-held", "held/three.txt")]),
        )
        .await;

    let unpermitted = emulator.assumed_role_key("content-nothing", None).await;
    let permitted = emulator
        .assumed_role_key(
            "content-reader",
            Some(&allow(&["bedrock:GetDocumentContent"])),
        )
        .await;

    /// One document the two callers both ask for, and what the permitted one gets.
    struct Row {
        case: &'static str,
        document: &'static str,
        permitted_status: u16,
    }

    let rows = [
        Row {
            case: "a document that exists",
            document: "doc-held",
            permitted_status: 200,
        },
        Row {
            case: "a document that was never ingested",
            document: "doc-never-ingested",
            permitted_status: NOT_FOUND.1,
        },
    ];

    for row in rows {
        let denied = emulator
            .document_content(
                &unpermitted,
                &source.knowledge_base_id,
                &source.data_source_id,
                row.document,
                None,
            )
            .await;
        assert_eq!(
            (denied.error_code(row.case).as_str(), denied.status),
            ACCESS_DENIED,
            "case {:?}: expected {ACCESS_DENIED:?} for a caller granted nothing, got {} {:?}",
            row.case,
            denied.status,
            denied.json(row.case)
        );
        assert!(
            denied
                .message(row.case)
                .contains("bedrock:GetDocumentContent"),
            "case {:?}: the refusal must name the action, got {:?}",
            row.case,
            denied.message(row.case)
        );

        let served = emulator
            .document_content(
                &permitted,
                &source.knowledge_base_id,
                &source.data_source_id,
                row.document,
                None,
            )
            .await;
        assert_eq!(
            served.status,
            row.permitted_status,
            "case {:?}: expected the permitted caller to get {}, got {} {:?}",
            row.case,
            row.permitted_status,
            served.status,
            served.json(row.case)
        );
    }
}
