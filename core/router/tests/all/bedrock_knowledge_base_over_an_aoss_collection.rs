// SPDX-License-Identifier: CC0-1.0

//! A vector knowledge base built on an index created through a collection's OWN published
//! endpoint, end to end: the OpenSearch Serverless data plane, Amazon Bedrock Agent and the
//! Bedrock Agent runtime, over one engine.
//!
//! RULE: the address a collection publishes as its `collectionEndpoint` — "the
//! collection-specific endpoint used to submit index, search, and data upload requests"
//! (`CollectionDetail$collectionEndpoint`) — serves the collection's OpenSearch REST data
//! plane, which names no RPC target, so the vector index a knowledge base's
//! `storageConfiguration` requires can be created there BEFORE the knowledge base names it
//! (`OpenSearchServerlessConfiguration` carries `collectionArn`, `vectorIndexName` and a
//! `fieldMapping`, so the index has to exist already); a document then ingested into a
//! custom data source of that knowledge base is retrievable through the runtime plane with
//! its content byte-identical to the bytes the ingestion carried.
//!
//! Three crates have to agree for that to hold — the collection's endpoint is served by
//! `service-com-amazonaws-opensearchserverless-opensearchserverless`, the knowledge base and
//! the ingestion by the Bedrock Agent build-time plane, the retrieval by the Bedrock Agent
//! runtime plane — so the suite lives here rather than in any one of them. The crate-local
//! halves are those crates' own suites: the data plane's index and document surface in
//! `services/service-com-amazonaws-opensearchserverless-opensearchserverless/tests/all/\
//! collection_data_plane.rs`, and what a collection's policies decide about a knowledge
//! base's role in `services/service-com-amazonaws-bedrockagent-\
//! amazonbedrockagentbuildtimelambda/tests/all/knowledge_base_role_and_vector_store_access.rs`.
//!
//! The rows differ along what the rule generalises over: the bytes the inline document
//! carries (ASCII, two-, three- and four-byte UTF-8, and a combining sequence), how many
//! documents one request ingests, the embedding model and with it the dimension the index is
//! created with, and the names of the collection, the index and the document. None of them
//! may change that the index create is SERVED at the published endpoint and that what comes
//! back out of the knowledge base is the bytes that went in.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The Region every call of this suite is made in.
const REGION: &str = "us-west-2";

/// The OpenSearch Serverless signing name, which routes a control-plane call to that
/// service and is the name a client signs a request to a collection endpoint with
/// (`smithy_query(query="service_info")` -> `sigv4Name`, the `aoss` model).
const AOSS_SIGNING_NAME: &str = "aoss";

/// The `X-Amz-Target` prefix the OpenSearch Serverless control plane routes on, from that
/// crate's `OPERATIONS.md` wire-protocol section. The collection's REST data plane carries
/// NO such header, which is the whole point of the rule under test.
const AOSS_TARGET_PREFIX: &str = "OpenSearchServerless";

/// The signing name every Bedrock-family endpoint shares, which is what routes both Bedrock
/// Agent planes (each claims its own path space).
const BEDROCK_SIGNING_NAME: &str = "bedrock";

/// The access key id every call here presents. It names no IAM record, which is the position
/// an account's own published credential is in: the collection's data access policies do not
/// bound it, so the index create a caller makes before any knowledge base exists is decided
/// by the network policy alone.
const ACCESS_KEY_ID: &str = "AKIDAMBIENT";

/// The IAM role the knowledge bases of this suite act as, and the principal the collection's
/// `data` access policy grants the index permissions to. Matches the modelled
/// `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// The field names AWS's "Create a vector index in Amazon OpenSearch Service" gives a
/// knowledge base's index, which are what the `fieldMapping` of the storage configuration
/// names and what the index's own `knn_vector` mapping declares.
const VECTOR_FIELD: &str = "bedrock-knowledge-base-default-vector";
const TEXT_FIELD: &str = "AMAZON_BEDROCK_TEXT_CHUNK";
const METADATA_FIELD: &str = "AMAZON_BEDROCK_METADATA";

/// How many times a read looks again for a transition the clock owes. Generous: every wait
/// here is for the CONDITION and nothing asserts how long it took.
const SETTLE_ATTEMPTS: usize = 400;

/// How long one look waits before the next.
const SETTLE_PAUSE: Duration = Duration::from_millis(25);

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them. Request authentication is off, as in every other suite here: what each call carries
/// is a credential scope, which is what routes it and what each handler reads its Region and
/// its caller from.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` in
/// [`REGION`] and whose credential is [`ACCESS_KEY_ID`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request and parse its JSON answer.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Bytes>) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One OpenSearch Serverless CONTROL-plane call, over the `awsJson1_0` wire its model
/// declares.
async fn aoss(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{AOSS_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{AOSS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(AOSS_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed control-plane request");
    dispatch(router, request).await
}

/// One OpenSearch REST request a client sends to a collection's published endpoint, as
/// `opensearch-py`'s `indices.create` / `indices.get` send it: addressed to the authority
/// the endpoint names, signed for the `aoss` signing name, and carrying NO `x-amz-target`,
/// because the OpenSearch REST API has no RPC target.
async fn rest(
    router: &MultiServiceRouter,
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let (authority, prefix) = addressed(endpoint);
    let request = http::Request::builder()
        .method(method)
        .uri(format!("{prefix}{path}"))
        .header("host", authority)
        .header("content-type", "application/json")
        .header("authorization", authorization(AOSS_SIGNING_NAME))
        .body(Bytes::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .expect("a well-formed data-plane request");
    dispatch(router, request).await
}

/// The authority a published endpoint is dialled at and the path prefix it carries, so a
/// REST request is addressed exactly where the collection said to address it — whether the
/// published address names the collection in its HOST or in its PATH.
fn addressed(endpoint: &str) -> (String, String) {
    let after_scheme = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint);
    match after_scheme.find('/') {
        Some(at) => (
            after_scheme[..at].to_string(),
            after_scheme[at..].trim_end_matches('/').to_string(),
        ),
        None => (after_scheme.to_string(), String::new()),
    }
}

/// One Bedrock call, over the `restJson1` wire both Bedrock Agent planes declare, on the
/// method and URI the operation's own `@http` binding names.
async fn bedrock(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(BEDROCK_SIGNING_NAME))
        .body(
            body.map(|body| {
                Bytes::from(serde_json::to_vec(body).expect("the request body serializes"))
            })
            .unwrap_or_default(),
        )
        .expect("a well-formed Bedrock request");
    dispatch(router, request).await
}

/// The value at a dotted path of a JSON document, or `Value::Null` where there is none.
///
/// The walk holds a REFERENCE at each step and clones the leaf alone, so reading a member of
/// a large answer in a poll loop copies that member and not the whole document.
fn at(document: &Value, path: &str) -> Value {
    let mut here = document;
    for segment in path.split('.') {
        match here.get(segment) {
            Some(next) => here = next,
            None => return Value::Null,
        }
    }
    here.clone()
}

/// The string at a dotted path, or the empty string where there is none.
fn text(document: &Value, path: &str) -> String {
    at(document, path).as_str().unwrap_or_default().to_string()
}

/// A `network` security policy admitting public traffic to every collection of this
/// emulator — AWS's "Network access for Amazon OpenSearch Serverless": "you must have at
/// least one network policy that applies to a collection in order to access its endpoint".
async fn admit_network_access(router: &MultiServiceRouter) {
    let (status, answer) = aoss(
        router,
        "CreateSecurityPolicy",
        json!({
            "type": "network",
            "name": "net-every-collection",
            "policy": json!({
                "Rules": [{ "ResourceType": "collection", "Resource": ["collection/*"] }],
                "AllowFromPublic": true,
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the network policy accepted, got {status} with {answer}"
    );
}

/// An `encryption` security policy for every collection of this emulator, which a collection
/// cannot be created without.
async fn encrypt_collections(router: &MultiServiceRouter) {
    let (status, answer) = aoss(
        router,
        "CreateSecurityPolicy",
        json!({
            "type": "encryption",
            "name": "enc-every-collection",
            "policy": json!({
                "Rules": [{ "ResourceType": "collection", "Resource": ["collection/*"] }],
                "AWSOwnedKey": true,
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the encryption policy accepted, got {status} with {answer}"
    );
}

/// A `data` access policy granting the knowledge base's own role every `aoss` permission on
/// one collection and its indexes — the grant AWS's "Create a vector index in Amazon
/// OpenSearch Service" has a caller make for the knowledge-base role.
///
/// What it is here FOR is the knowledge-base half: `CreateKnowledgeBase` resolves the
/// binding against the collection's stored policies, for the principal its `roleArn` names,
/// so without this grant the create below would be refused. It does not decide the index
/// create at the endpoint — that call presents the account's own published credential, which
/// the data access policies do not bound, so the network policy alone admits it. Which
/// principal and which permissions the two planes decide on is pinned by the crate-local
/// suites named in this file's header, not here.
async fn grant_the_role_data_access(router: &MultiServiceRouter, collection: &str) {
    let (status, answer) = aoss(
        router,
        "CreateAccessPolicy",
        json!({
            "type": "data",
            "name": format!("{collection}-data"),
            "policy": json!([{
                "Rules": [
                    {
                        "ResourceType": "index",
                        "Resource": [format!("index/{collection}/*")],
                        "Permission": ["aoss:*"],
                    },
                    {
                        "ResourceType": "collection",
                        "Resource": [format!("collection/{collection}")],
                        "Permission": ["aoss:*"],
                    },
                ],
                "Principal": [KB_ROLE],
            }])
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the data access policy accepted, got {status} with {answer}"
    );
}

/// Create a `VECTORSEARCH` collection and poll until it reports `ACTIVE`, answering the
/// `CollectionDetail` that read reports.
///
/// The `CREATING -> ACTIVE` move is owed by the clock, so this looks again until the
/// collection reports `ACTIVE` or a generous deadline passes.
async fn active_collection(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, created) = aoss(
        router,
        "CreateCollection",
        json!({ "name": name, "type": "VECTORSEARCH" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollection({name}) accepted, got {status} with {created}"
    );
    let id = text(&created, "createCollectionDetail.id");
    for _ in 0..SETTLE_ATTEMPTS {
        let (status, read) = aoss(router, "BatchGetCollection", json!({ "ids": [&id] })).await;
        assert_eq!(
            status, 200,
            "setup: expected BatchGetCollection({id}) to answer, got {status} with {read}"
        );
        let detail = at(&read, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == "ACTIVE" {
            return detail;
        }
        tokio::time::sleep(SETTLE_PAUSE).await;
    }
    panic!("setup: the collection {name} never reached ACTIVE");
}

/// The index body a Bedrock knowledge base's vector index is created with, of `dimension`
/// values: the body `opensearch-py`'s `indices.create` carries for one.
fn knn_index_body(dimension: usize) -> Value {
    json!({
        "settings": { "index": { "knn": true } },
        "mappings": { "properties": {
            VECTOR_FIELD: { "type": "knn_vector", "dimension": dimension },
            TEXT_FIELD: { "type": "text" },
            METADATA_FIELD: { "type": "text", "index": false },
        } },
    })
}

/// The `storageConfiguration` of a knowledge base whose vectors live in `index` of the
/// collection `collection_arn` names.
fn opensearch_storage(collection_arn: &str, index: &str) -> Value {
    json!({
        "type": "OPENSEARCH_SERVERLESS",
        "opensearchServerlessConfiguration": {
            "collectionArn": collection_arn,
            "vectorIndexName": index,
            "fieldMapping": {
                "vectorField": VECTOR_FIELD,
                "textField": TEXT_FIELD,
                "metadataField": METADATA_FIELD,
            },
        },
    })
}

/// A custom document whose content is defined inline IN BYTES, which is how a client pushes
/// bytes it holds straight into a knowledge base: `data` travels base64-encoded, as the
/// modelled `ByteContentDoc$data` blob does.
fn inline_byte_document(id: &str, bytes: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "BYTE",
                    "byteContent": {
                        "mimeType": "text/plain",
                        "data": ::aws_smithy_types::base64::encode(bytes),
                    },
                },
            },
        },
    })
}

/// One case of the table: what the caller sets up and what it pushes through it.
struct Row {
    case: &'static str,
    /// The collection, the index inside it and the knowledge base's name.
    collection: &'static str,
    index: &'static str,
    knowledge_base: &'static str,
    /// The embedding model the knowledge base uses, and the number of values its vectors
    /// carry — which is the dimension the caller creates the index with.
    model: &'static str,
    dimension: usize,
    /// The documents one ingestion request carries: an id and the bytes it holds.
    documents: &'static [(&'static str, &'static str)],
}

/// RULE: a collection's published `collectionEndpoint` serves the OpenSearch REST data
/// plane, so the vector index a knowledge base requires is created there before the
/// knowledge base names it, and a document ingested into that knowledge base comes back out
/// of the runtime plane with its bytes unchanged.
///
/// The rows span the bytes the document carries, how many documents one request ingests, the
/// embedding model and the dimension the index holds, and the names of all three resources.
#[tokio::test]
async fn an_index_created_at_the_collection_endpoint_backs_a_knowledge_base_round_trip() {
    // `amazon.titan-embed-text-v2:0` publishes 1024-value vectors and
    // `amazon.titan-embed-text-v1` 1536, so the index a caller creates for each holds
    // vectors of that length.
    let rows: &[Row] = &[
        Row {
            case: "ascii bytes, one document, 1024 values",
            collection: "kb-ascii",
            index: "bedrock-knowledge-base-ascii",
            knowledge_base: "kb-ascii",
            model: "amazon.titan-embed-text-v2:0",
            dimension: 1024,
            documents: &[("doc-ascii", "escalation-runbook-marker")],
        },
        Row {
            case: "three-byte UTF-8 bytes, one document, 1024 values",
            collection: "kb-cjk",
            index: "bedrock-knowledge-base-cjk",
            knowledge_base: "kb-cjk",
            model: "amazon.titan-embed-text-v2:0",
            dimension: 1024,
            documents: &[("doc-cjk", "\u{30a8}\u{30b9}\u{30ab}\u{30ec}-\u{6587}\u{66f8}")],
        },
        Row {
            case: "four-byte UTF-8 bytes beside a combining sequence, two documents, 1536 \
                   values",
            collection: "kb-wide",
            index: "bedrock-knowledge-base-wide",
            knowledge_base: "kb_wide",
            model: "amazon.titan-embed-text-v1",
            dimension: 1536,
            documents: &[
                ("doc-emoji", "runbook-\u{1f680}-\u{1f6a8}"),
                ("doc-combining", "cafe\u{301}-pre\u{301}cis-\u{e9}chéance"),
            ],
        },
        Row {
            case: "two-byte UTF-8 bytes, one document, 1536 values",
            collection: "kb-latin-1",
            index: "bedrock-knowledge-base-latin",
            knowledge_base: "kb-latin-1",
            model: "amazon.titan-embed-text-v1",
            dimension: 1536,
            documents: &[("doc-latin", "\u{dc}berwachung-\u{df}-marker")],
        },
    ];

    for row in rows {
        let Row {
            case,
            collection,
            index,
            knowledge_base,
            model,
            dimension,
            documents,
        } = row;
        let router = build_router();
        encrypt_collections(&router).await;
        admit_network_access(&router).await;
        grant_the_role_data_access(&router, collection).await;
        let detail = active_collection(&router, collection).await;
        let endpoint = text(&detail, "collectionEndpoint");
        let collection_arn = text(&detail, "arn");
        assert!(
            !endpoint.is_empty() && !collection_arn.is_empty(),
            "case {case:?}: expected an ACTIVE collection to publish an endpoint and an ARN, \
             got {detail}"
        );

        // The index create a knowledge base's vector store needs, exactly as a client sends
        // it: no RPC target, at the root of the address the collection published.
        let body = knn_index_body(*dimension);
        let (status, created) = rest(&router, &endpoint, "PUT", &format!("/{index}"), Some(&body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the index create at the published endpoint to be served, \
             got {status} with {created}"
        );
        assert_eq!(
            at(&created, "index"),
            json!(index),
            "case {case:?}: expected the create to acknowledge the index it created, got \
             {created}"
        );

        // ...and the index is really there: the mapping the create carried is what a read of
        // it reports, so the knowledge base names an index this collection holds.
        let (status, read) = rest(&router, &endpoint, "GET", &format!("/{index}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created index to be readable back, got {status} with \
             {read}"
        );
        assert_eq!(
            at(&read, &format!("{index}.mappings.properties.{VECTOR_FIELD}.dimension")),
            json!(dimension),
            "case {case:?}: expected the index to hold the knn_vector mapping the create \
             carried, got {read}"
        );

        // The knowledge base over that index. 202 Accepted, so its ACTIVE status is owed by
        // the clock and polled for.
        let (status, created) = bedrock(
            &router,
            "PUT",
            "/knowledgebases/",
            Some(&json!({
                "name": knowledge_base,
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "VECTOR",
                    "vectorKnowledgeBaseConfiguration": {
                        "embeddingModelArn":
                            format!("arn:aws:bedrock:{REGION}::foundation-model/{model}"),
                    },
                },
                "storageConfiguration": opensearch_storage(&collection_arn, index),
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected CreateKnowledgeBase over the created index to be \
             accepted, got {status} with {created}"
        );
        let knowledge_base_id = text(&created, "knowledgeBase.knowledgeBaseId");
        assert!(
            !knowledge_base_id.is_empty(),
            "case {case:?}: expected a minted knowledgeBaseId, got {created}"
        );

        let read = await_active_knowledge_base(&router, case, &knowledge_base_id).await;
        assert_eq!(
            at(&read, "storageConfiguration"),
            opensearch_storage(&collection_arn, index),
            "case {case:?}: expected the knowledge base to report the storage configuration \
             it was created with, got {read}"
        );
        assert_eq!(
            text(&read, "knowledgeBaseArn"),
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/{knowledge_base_id}"),
            "case {case:?}: expected the ARN the service mints for the id it minted, got \
             {read}"
        );

        // A CUSTOM data source: its documents are pushed straight in rather than crawled.
        let (status, created) = bedrock(
            &router,
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
            Some(&json!({
                "name": format!("{knowledge_base}-pushed"),
                "dataSourceConfiguration": { "type": "CUSTOM" },
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataSource(CUSTOM) accepted, got {status} with \
             {created}"
        );
        let data_source_id = text(&created, "dataSource.dataSourceId");
        assert!(
            !data_source_id.is_empty(),
            "case {case:?}: expected a minted dataSourceId, got {created}"
        );

        let (status, ingested) = bedrock(
            &router,
            "PUT",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"
            ),
            Some(&json!({
                "documents": documents
                    .iter()
                    .map(|(id, bytes)| inline_byte_document(id, bytes))
                    .collect::<Vec<Value>>(),
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the ingestion accepted, got {status} with {ingested}"
        );
        assert_eq!(
            at(&ingested, "documentDetails")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            documents.len(),
            "case {case:?}: expected one detail per ingested document, got {ingested}"
        );

        // Each document reaches INDEXED on its own, with nobody calling: the clock owes it.
        for (id, _) in *documents {
            let detail =
                await_indexed_document(&router, case, &knowledge_base_id, &data_source_id, id)
                    .await;
            assert_eq!(
                at(&detail, "identifier"),
                json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } }),
                "case {case:?}: expected the detail of the document the request identified, \
                 got {detail}"
            );
        }

        // ...and what the runtime plane retrieves is the bytes the ingestion carried, byte
        // for byte, cited under the document id the caller gave it.
        for (id, bytes) in *documents {
            let (status, retrieved) = bedrock(
                &router,
                "POST",
                &format!("/knowledgebases/{knowledge_base_id}/retrieve"),
                Some(&json!({ "retrievalQuery": { "text": bytes } })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the retrieval of {id:?} to be served, got {status} \
                 with {retrieved}"
            );
            let results = at(&retrieved, "retrievalResults");
            let results = results.as_array().cloned().unwrap_or_default();
            let found = results.iter().find(|result| {
                text(result, "location.customDocumentLocation.id") == *id
            });
            let found = found.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the retrieval to cite the document {id:?}, got \
                     {retrieved}"
                )
            });
            assert_eq!(
                text(found, "content.text"),
                *bytes,
                "case {case:?}: expected the retrieved content to be the bytes ingested, got \
                 {found}"
            );
            assert_eq!(
                text(found, "location.type"),
                "CUSTOM",
                "case {case:?}: expected a document of a custom data source to be cited as \
                 CUSTOM, got {found}"
            );
        }
    }
}

/// Poll `GetKnowledgeBase` until `knowledge_base_id` reports `ACTIVE`, answering the
/// `knowledgeBase` that read reports.
async fn await_active_knowledge_base(
    router: &MultiServiceRouter,
    case: &str,
    knowledge_base_id: &str,
) -> Value {
    for _ in 0..SETTLE_ATTEMPTS {
        let (status, read) = bedrock(
            router,
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetKnowledgeBase to answer, got {status} with {read}"
        );
        let knowledge_base = at(&read, "knowledgeBase");
        if text(&knowledge_base, "status") == "ACTIVE" {
            return knowledge_base;
        }
        tokio::time::sleep(SETTLE_PAUSE).await;
    }
    panic!("case {case:?}: the knowledge base never reached ACTIVE");
}

/// Poll `GetKnowledgeBaseDocuments` until the document `id` reports `INDEXED`, answering the
/// detail that read reports.
async fn await_indexed_document(
    router: &MultiServiceRouter,
    case: &str,
    knowledge_base_id: &str,
    data_source_id: &str,
    id: &str,
) -> Value {
    let uri = format!(
        "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents/getDocuments"
    );
    let body = json!({
        "documentIdentifiers": [{ "dataSourceType": "CUSTOM", "custom": { "id": id } }],
    });
    for _ in 0..SETTLE_ATTEMPTS {
        let (status, read) = bedrock(router, "POST", &uri, Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetKnowledgeBaseDocuments to answer for {id:?}, got \
             {status} with {read}"
        );
        let detail = at(&read, "documentDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == "INDEXED" {
            return detail;
        }
        tokio::time::sleep(SETTLE_PAUSE).await;
    }
    panic!("case {case:?}: the document {id:?} never reached INDEXED");
}
