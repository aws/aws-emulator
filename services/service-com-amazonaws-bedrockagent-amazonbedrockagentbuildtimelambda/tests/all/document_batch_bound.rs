// SPDX-License-Identifier: CC0-1.0

//! How many documents ONE `IngestKnowledgeBaseDocuments` request may carry, end
//! to end over HTTP, and how a batch outside that bound is refused.
//!
//! The rule this suite pins is that the bound is the one the SERVICE applies on
//! this path rather than the `@length` the published model declares on
//! `KnowledgeBaseDocuments` (`min 1, max 10`,
//! `smithy_query(query="shape",shape="KnowledgeBaseDocuments")`), which is
//! behind it:
//!
//! * the front door takes a batch of up to `MAX_DOCUMENTS_PER_REQUEST`
//!   documents, whatever the request addresses, and refuses a larger one — or an
//!   empty one — with the constraint layer's own envelope, quoting the list the
//!   request carried and naming the bound it broke;
//! * a knowledge base the SERVICE manages takes
//!   `MAX_MANAGED_DOCUMENTS_PER_REQUEST` documents per request, and refuses a
//!   larger batch with the sentence that names the count, the maximum and the
//!   kind of knowledge base;
//! * the value the refusal quotes is the documents themselves, printed as the
//!   structures they are, with the value of every member the model marks
//!   `@sensitive` withheld — the inline text and bytes of a document, a metadata
//!   attribute's key and its number and string values, and a whole
//!   access-control entry.
//!
//! The rows differ along what the rule generalises over: how many documents the
//! request carries (none, one, a few, the bound, one past it, far past it),
//! which KIND of knowledge base they are addressed to, and which kind of content
//! and metadata each document carries.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    MAX_DOCUMENTS_PER_REQUEST, MAX_MANAGED_DOCUMENTS_PER_REQUEST, MAX_QUOTED_DOCUMENTS,
    MIN_DOCUMENTS_PER_REQUEST,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket the self-managed knowledge base here binds to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/batch-bound-vectors";

/// The index of [`VECTOR_BUCKET`] the ingested vectors are written into.
const INDEX: &str = "batch-bound-index";

/// What a rendered structure states in place of the value of a `@sensitive`
/// member, written out here rather than read from the crate so the test pins the
/// marker instead of restating it.
const WITHHELD: &str = "*** REDACTED ***";

/// The `DocumentStatus` the model documents for a document that "wasn't found",
/// which is what a read of an identifier nothing was stored under reports.
const NOT_FOUND: &str = "NOT_FOUND";

/// The most document identifiers one `GetKnowledgeBaseDocuments` takes: the
/// `@length` maximum the model declares on its `documentIdentifiers`
/// (`smithy_query(query="operation",operation="GetKnowledgeBaseDocuments")`).
const IDENTIFIERS_PER_READ: usize = 10;

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The `message` the modelled `ValidationException` carries.
    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("the refusal carries no message in {:?}", self.body))
            .to_string()
    }

    /// The `customDocumentIdentifier` ids the answer echoes back, in order.
    fn echoed_ids(&self) -> Vec<String> {
        self.body["documentDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is not a list in {:?}", self.body))
            .iter()
            .map(|detail| {
                detail["identifier"]["custom"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The self-managed knowledge base binds to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index exists before anything names it, as against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            VECTOR_BUCKET,
            INDEX,
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base(&self, request: Value) -> String {
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.body["knowledgeBase"]["knowledgeBaseId"]
            .as_str()
            .expect("a minted knowledgeBaseId")
            .to_string()
    }

    /// Attach a data source to `knowledge_base_id` and answer its minted
    /// `dataSourceId`.
    async fn data_source(&self, knowledge_base_id: &str, request: Value) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(request),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.body["dataSource"]["dataSourceId"]
            .as_str()
            .expect("a minted dataSourceId")
            .to_string()
    }

    async fn ingest(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        documents: Vec<Value>,
    ) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(json!({ "documents": documents })),
        )
        .await
    }

    /// The `DocumentStatus` `GetKnowledgeBaseDocuments` reports for each of the
    /// custom ids `ids` names, in that order.
    ///
    /// Asked in batches of [`IDENTIFIERS_PER_READ`], the maximum that read takes
    /// per request (`smithy_query(query="operation",operation="GetKnowledgeBaseDocuments")`),
    /// so reading a batch back is not itself bounded by what one read accepts.
    async fn document_statuses(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ids: &[String],
    ) -> Vec<String> {
        let mut statuses: Vec<String> = Vec::new();
        for batch in ids.chunks(IDENTIFIERS_PER_READ) {
            let identifiers: Vec<Value> = batch
                .iter()
                .map(|id| json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } }))
                .collect();
            let read = self
                .call(
                    "POST",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                         /documents/getDocuments"
                    ),
                    Some(json!({ "documentIdentifiers": identifiers })),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case setup: expected GetKnowledgeBaseDocuments -> 200, got {read:?}"
            );
            statuses.extend(
                read.body["documentDetails"]
                    .as_array()
                    .unwrap_or_else(|| panic!("documentDetails is a list in {:?}", read.body))
                    .iter()
                    .map(|detail| detail["status"].as_str().unwrap_or_default().to_string()),
            );
        }
        statuses
    }
}

/// A self-managed vector knowledge base: the kind the front door's own bound is
/// the only one for.
fn vector_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": {
            "type": "S3_VECTORS",
            "s3VectorsConfiguration": {
                "vectorBucketArn": VECTOR_BUCKET,
                "indexName": INDEX,
            },
        },
    })
}

/// A service-managed knowledge base: the kind that takes a smaller batch.
fn managed_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "MANAGED",
            "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
        },
    })
}

/// A CUSTOM data source of a self-managed knowledge base: documents are pushed
/// into it rather than crawled.
fn custom_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": { "type": "CUSTOM" },
    })
}

/// A managed-connector data source running the push-only `CUSTOM` connector: the
/// kind of data source a service-managed knowledge base holds, and the one that
/// takes directly ingested custom documents.
fn managed_custom_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": {
                "connectorParameters": { "type": "CUSTOM", "version": "1" },
            },
        },
    })
}

/// A custom document whose content is defined inline as text.
fn text_document(id: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "TEXT",
                    "textContent": { "data": inline_text(id) },
                },
            },
        },
    })
}

/// The inline text one document carries: a `@sensitive` value, so no refusal may
/// echo it.
fn inline_text(id: &str) -> String {
    format!("the confidential text of {id}")
}

/// How [`text_document`] renders inside the value a refusal quotes: every member
/// the model declares, one the request left unset as `null`, which is the form
/// the validation layer in front of a service prints a structure in.
fn rendered_text_document(id: &str) -> String {
    format!(
        "KnowledgeBaseDocument(metadata=null, \
         content=DocumentContent(dataSourceType=CUSTOM, \
         custom=CustomContent(customDocumentIdentifier=CustomDocumentIdentifier(id={id}), \
         sourceType=IN_LINE, s3Location=null, inlineContent=InlineContent(type=TEXT, \
         byteContent=null, textContent=TextContentDoc(data={WITHHELD}))), s3=null))"
    )
}

/// A custom document whose content is inline BYTES, carrying metadata: an
/// attribute of each value type the model declares and one access-control entry.
fn byte_document(id: &str) -> Value {
    json!({
        "metadata": {
            "type": "IN_LINE_ATTRIBUTE",
            "inlineAttributes": [
                { "key": "department", "value": { "type": "STRING", "stringValue": "finance" } },
                { "key": "year", "value": { "type": "NUMBER", "numberValue": 2026 } },
                { "key": "current", "value": { "type": "BOOLEAN", "booleanValue": true } },
            ],
            "accessControlList": [
                { "name": "analyst", "type": "USER", "access": "ALLOW" },
            ],
        },
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "BYTE",
                    "byteContent": { "mimeType": "text/plain", "data": "aGVsbG8sIGRvY3VtZW50" },
                },
            },
        },
    })
}

/// How [`byte_document`] renders inside the value a refusal quotes: every
/// `@sensitive` member withheld — the attribute keys, the number and string
/// values, the bytes and the whole access-control entry — and the boolean value
/// and the media type, which the model marks neither, printed.
fn rendered_byte_document(id: &str) -> String {
    format!(
        "KnowledgeBaseDocument(metadata=DocumentMetadata(type=IN_LINE_ATTRIBUTE, \
         inlineAttributes=[MetadataAttribute(key={WITHHELD}, \
         value=MetadataAttributeValue(type=STRING, numberValue=null, booleanValue=null, \
         stringValue={WITHHELD}, stringListValue=null)), MetadataAttribute(key={WITHHELD}, \
         value=MetadataAttributeValue(type=NUMBER, numberValue={WITHHELD}, \
         booleanValue=null, stringValue=null, stringListValue=null)), \
         MetadataAttribute(key={WITHHELD}, value=MetadataAttributeValue(type=BOOLEAN, \
         numberValue=null, booleanValue=true, stringValue=null, stringListValue=null))], \
         s3Location=null, accessControlList=[{WITHHELD}]), \
         content=DocumentContent(dataSourceType=CUSTOM, \
         custom=CustomContent(customDocumentIdentifier=CustomDocumentIdentifier(id={id}), \
         sourceType=IN_LINE, s3Location=null, inlineContent=InlineContent(type=BYTE, \
         byteContent=ByteContentDoc(mimeType=text/plain, data={WITHHELD}), \
         textContent=null)), s3=null))"
    )
}

/// A custom document whose content is read from an S3 object instead of being
/// sent inline.
fn located_document(id: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "S3_LOCATION",
                "s3Location": { "uri": located_uri(id) },
            },
        },
    })
}

/// Where [`located_document`]'s content lives.
fn located_uri(id: &str) -> String {
    format!("s3://batch-bound-documents/{id}.txt")
}

/// How [`located_document`] renders inside the value a refusal quotes: the
/// object's URI is no `@sensitive` member, so it is printed.
fn rendered_located_document(id: &str) -> String {
    format!(
        "KnowledgeBaseDocument(metadata=null, \
         content=DocumentContent(dataSourceType=CUSTOM, \
         custom=CustomContent(customDocumentIdentifier=CustomDocumentIdentifier(id={id}), \
         sourceType=S3_LOCATION, s3Location=CustomS3Location(uri={}, \
         bucketOwnerAccountId=null), inlineContent=null), s3=null))",
        located_uri(id)
    )
}

/// The whole message the constraint layer refuses a list of `rendered` elements
/// at `member` with, for a list over its maximum.
fn too_long_message(member: &str, rendered: &[String], max: i64) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at '{member}' failed to satisfy constraint: \
         Member must have length less than or equal to {max}",
        rendered.join(", ")
    )
}

/// The same envelope for a list under its minimum.
fn too_short_message(member: &str, rendered: &[String], min: i64) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at '{member}' failed to satisfy constraint: \
         Member must have length greater than or equal to {min}",
        rendered.join(", ")
    )
}

/// The same envelope with NO value half, which is how a clause states a value it
/// does not render.
fn unquoted_too_long_message(member: &str, max: i64) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have length less than or equal to {max}"
    )
}

/// The sentence a knowledge base the SERVICE manages refuses a batch over its
/// own maximum with: the documents the request carried, that maximum and the
/// kind of knowledge base fill the variable parts.
fn managed_batch_message(documents: usize, maximum: usize) -> String {
    format!(
        "The number of documents ({documents}) exceeds the maximum allowed ({maximum}) for \
         MANAGED knowledge base type."
    )
}

/// What one row of the batch table expects of the answer.
#[derive(Debug)]
enum Expected {
    /// Every document of the request is taken in and echoed back.
    Accepted,
    /// The FRONT DOOR refused the batch: the constraint layer's envelope naming
    /// the bound it broke, quoting the documents the request carried.
    TooLong,
    /// The front door refused an empty batch the same way.
    TooShort,
    /// The front door refused a batch of more documents than one clause renders,
    /// so the clause names the member and the requirement alone.
    TooLongUnquoted,
    /// The addressed knowledge base's own kind refused the batch, with the
    /// sentence naming the count, the maximum and the kind.
    ManagedBatch,
}

/// One row of the batch table: the kind of knowledge base addressed and how many
/// documents the request carries.
struct BatchRow {
    case: &'static str,
    /// Whether the request is addressed to the MANAGED knowledge base rather
    /// than the self-managed one.
    managed: bool,
    documents: usize,
    expected: Expected,
}

/// The documents bound is the service's own: a batch the published `@length`
/// maximum would refuse is taken whole, a batch past the bound the front door
/// applies is refused quoting the list it carried, and a service-managed
/// knowledge base refuses a smaller batch with its own sentence.
#[tokio::test]
async fn the_documents_batch_bound_is_the_one_the_service_applies() {
    let front_door = usize::try_from(MAX_DOCUMENTS_PER_REQUEST).expect("a small bound");
    let rows = [
        BatchRow {
            case: "one document",
            managed: false,
            documents: 1,
            expected: Expected::Accepted,
        },
        BatchRow {
            case: "the published modelled maximum",
            managed: false,
            documents: MAX_MANAGED_DOCUMENTS_PER_REQUEST,
            expected: Expected::Accepted,
        },
        BatchRow {
            case: "one document short of the bound",
            managed: false,
            documents: front_door - 1,
            expected: Expected::Accepted,
        },
        BatchRow {
            case: "the bound itself",
            managed: false,
            documents: front_door,
            expected: Expected::Accepted,
        },
        BatchRow {
            case: "one document past the bound",
            managed: false,
            documents: front_door + 1,
            expected: Expected::TooLong,
        },
        BatchRow {
            case: "several documents past the bound",
            managed: false,
            documents: front_door + 5,
            expected: Expected::TooLong,
        },
        BatchRow {
            case: "more documents than one clause renders",
            managed: false,
            documents: MAX_QUOTED_DOCUMENTS + 1,
            expected: Expected::TooLongUnquoted,
        },
        BatchRow {
            case: "no document at all",
            managed: false,
            documents: 0,
            expected: Expected::TooShort,
        },
        BatchRow {
            case: "a managed knowledge base at its own maximum",
            managed: true,
            documents: MAX_MANAGED_DOCUMENTS_PER_REQUEST,
            expected: Expected::Accepted,
        },
        BatchRow {
            case: "a managed knowledge base one document past its own maximum",
            managed: true,
            documents: MAX_MANAGED_DOCUMENTS_PER_REQUEST + 1,
            expected: Expected::ManagedBatch,
        },
        BatchRow {
            case: "a managed knowledge base past the front door's bound too, judged first",
            managed: true,
            documents: front_door + 1,
            expected: Expected::TooLong,
        },
    ];

    let emulator = Emulator::new();
    let vector_kb = emulator
        .knowledge_base(vector_knowledge_base("batch-bound-vector-kb"))
        .await;
    let vector_ds = emulator
        .data_source(&vector_kb, custom_data_source("batch-bound-vector-ds"))
        .await;
    let managed_kb = emulator
        .knowledge_base(managed_knowledge_base("batch-bound-managed-kb"))
        .await;
    let managed_ds = emulator
        .data_source(&managed_kb, managed_custom_data_source("batch-bound-managed-ds"))
        .await;

    for (position, row) in rows.iter().enumerate() {
        let ids: Vec<String> = (0..row.documents)
            .map(|index| format!("batch-{position}-document-{index}"))
            .collect();
        let documents: Vec<Value> = ids.iter().map(|id| text_document(id)).collect();
        let rendered: Vec<String> = ids.iter().map(|id| rendered_text_document(id)).collect();
        let (knowledge_base_id, data_source_id) = if row.managed {
            (&managed_kb, &managed_ds)
        } else {
            (&vector_kb, &vector_ds)
        };

        let answered = emulator
            .ingest(knowledge_base_id, data_source_id, documents)
            .await;
        match row.expected {
            Expected::Accepted => {
                assert_eq!(
                    answered.status, 202,
                    "case {}: expected a batch of {} documents to be accepted (202), got {:?}",
                    row.case, row.documents, answered
                );
                assert_eq!(
                    answered.echoed_ids(),
                    ids,
                    "case {}: expected every document of the batch echoed back, got {:?}",
                    row.case,
                    answered.echoed_ids()
                );
                // And the documents really are the data source's own
                // afterwards, so the acceptance is backed by stored state.
                let statuses = emulator
                    .document_statuses(knowledge_base_id, data_source_id, &ids)
                    .await;
                assert!(
                    statuses.len() == row.documents
                        && statuses.iter().all(|status| status != NOT_FOUND),
                    "case {}: expected all {} documents stored, got {statuses:?}",
                    row.case,
                    row.documents
                );
            }
            Expected::TooLong
            | Expected::TooShort
            | Expected::TooLongUnquoted
            | Expected::ManagedBatch => {
                assert_eq!(
                    answered.status, 400,
                    "case {}: expected a batch of {} documents refused (400), got {:?}",
                    row.case, row.documents, answered
                );
                assert_eq!(
                    answered.error_type.as_deref(),
                    Some("ValidationException"),
                    "case {}: expected ValidationException, got {:?}",
                    row.case,
                    answered
                );
                let expected = match row.expected {
                    Expected::TooLong => {
                        too_long_message("documents", &rendered, MAX_DOCUMENTS_PER_REQUEST)
                    }
                    Expected::TooShort => {
                        too_short_message("documents", &rendered, MIN_DOCUMENTS_PER_REQUEST)
                    }
                    Expected::TooLongUnquoted => {
                        unquoted_too_long_message("documents", MAX_DOCUMENTS_PER_REQUEST)
                    }
                    _ => managed_batch_message(row.documents, MAX_MANAGED_DOCUMENTS_PER_REQUEST),
                };
                assert_eq!(
                    answered.message(),
                    expected,
                    "case {}: expected the refusal to read {expected:?}, got {:?}",
                    row.case,
                    answered.message()
                );
                // Nothing of a refused batch is written.
                let statuses = emulator
                    .document_statuses(knowledge_base_id, data_source_id, &ids)
                    .await;
                for (index, status) in statuses.iter().enumerate() {
                    assert_eq!(
                        status, NOT_FOUND,
                        "case {}: document {index} of a refused batch must not be stored, \
                         got {status:?}",
                        row.case
                    );
                }
            }
        }
    }
}

/// The value the front door's refusal quotes is the documents the request
/// carried, each printed as the structure it is — and no value of a member the
/// model marks `@sensitive` is in it.
#[tokio::test]
async fn the_refusal_quotes_the_documents_with_every_sensitive_value_withheld() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("batch-quote-kb"))
        .await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, custom_data_source("batch-quote-ds"))
        .await;

    // One document of each kind the model admits for a custom data source,
    // filled out to the bound the front door applies plus one, so the batch is
    // refused and the whole list is quoted back.
    let front_door = usize::try_from(MAX_DOCUMENTS_PER_REQUEST).expect("a small bound");
    let mut documents: Vec<Value> = vec![
        text_document("quote-text"),
        byte_document("quote-bytes"),
        located_document("quote-located"),
    ];
    let mut rendered: Vec<String> = vec![
        rendered_text_document("quote-text"),
        rendered_byte_document("quote-bytes"),
        rendered_located_document("quote-located"),
    ];
    for index in documents.len()..=front_door {
        let id = format!("quote-filler-{index}");
        documents.push(text_document(&id));
        rendered.push(rendered_text_document(&id));
    }

    let refused = emulator
        .ingest(&knowledge_base_id, &data_source_id, documents)
        .await;
    assert_eq!(
        refused.status, 400,
        "case the quoted batch: expected the over-long batch refused (400), got {refused:?}"
    );
    let expected = too_long_message("documents", &rendered, MAX_DOCUMENTS_PER_REQUEST);
    assert_eq!(
        refused.message(),
        expected,
        "case the quoted batch: expected the refusal to read {expected:?}, got {:?}",
        refused.message()
    );
    // The rule behind the marker, stated on its own: not one `@sensitive` value
    // the request carried is anywhere in the message.
    for (case, secret) in [
        ("the inline text", inline_text("quote-text")),
        ("the inline bytes", "aGVsbG8sIGRvY3VtZW50".to_string()),
        ("a metadata attribute key", "department".to_string()),
        ("a metadata string value", "finance".to_string()),
        ("a metadata number value", "2026".to_string()),
        ("an access control entry's principal", "analyst".to_string()),
    ] {
        assert!(
            !refused.message().contains(&secret),
            "case {case}: expected {secret:?} withheld from the refusal, got {:?}",
            refused.message()
        );
    }
}
