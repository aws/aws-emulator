// SPDX-License-Identifier: CC0-1.0

//! The collection's own DATA plane, end to end over the wire: the OpenSearch REST surface
//! served at the host `CollectionDetail$collectionEndpoint` publishes.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with the
//! method, the path and the `Host` header a client of that endpoint sends, signed for the
//! `aoss` signing name and carrying NO `x-amz-target` header, because the OpenSearch REST
//! API has no RPC target. That is exactly the request shape `opensearch-py`'s
//! `indices.create` / `index` / `search` issue.
//!
//! The rule under test: the host a collection publishes as its `collectionEndpoint` serves
//! the OpenSearch REST data plane out of the indexes and documents that collection holds —
//! not the control plane's RPC dispatcher — so an index create, a document write and a read
//! addressed there are served and answered from persisted state, and a request the engine
//! refuses carries the engine's own status and error type. The tables span what the rule
//! generalises over: which published host (and prefixed path) addresses the plane, which
//! verb and route is asked for, the shapes an index body and a document source take, and
//! the query forms a search is written in.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{call, emulator, text};
use crate::encryption_indexes_and_tagging::reachable_collection;

/// This service's SigV4 signing name, which is the name a client signs a request to a
/// collection's endpoint with (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The region the suite's collections are created in.
const REGION: &str = "us-east-1";

/// A `knn_vector` mapping of `dimensions` dimensions — the index body a Bedrock knowledge
/// base requires to exist in the collection before `CreateKnowledgeBase` can name it, and
/// the body the recorded `indices.create` carries.
fn knn_index_body(dimensions: usize, space_type: Option<&str>) -> Value {
    let mut vector = json!({ "type": "knn_vector", "dimension": dimensions });
    if let Some(space_type) = space_type {
        vector["method"] = json!({
            "name": "hnsw",
            "engine": "faiss",
            "space_type": space_type,
        });
    }
    json!({
        "settings": { "index": { "knn": true } },
        "mappings": {
            "properties": {
                "bedrock-knowledge-base-default-vector": vector,
                "AMAZON_BEDROCK_TEXT_CHUNK": { "type": "text" },
                "AMAZON_BEDROCK_METADATA": { "type": "text", "index": false },
            }
        },
    })
}

/// A SigV4 `Authorization` header scoped to this service in `REGION`. The suites run with
/// the router's request authentication off, so the header's job is to present the
/// credential scope a client of the endpoint signs with — the data plane is reached as a
/// credentialed operation only for a request that presents one.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDDATAPLANE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host;x-amz-content-sha256, Signature=deadbeef"
    )
}

/// One data-plane request: `method path` to `host`, carrying `body` when there is one.
/// Answers the HTTP status and the parsed body (`Value::Null` for an empty one).
async fn send(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    send_text(
        router,
        host,
        method,
        path,
        body.map(|body| body.to_string()).unwrap_or_default(),
        true,
    )
    .await
}

/// [`send`] with the body as text, so a bulk request can send NDJSON, and with the
/// credential optional, so an unsigned request can be sent too.
async fn send_text(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    body: String,
    signed: bool,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json");
    if signed {
        request = request.header("authorization", authorization());
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The host a collection publishes as one of its endpoints, which is what a client puts
/// in the `Host` header: the published URL without its scheme or its path.
fn endpoint_host(detail: &Value, member: &str) -> String {
    let endpoint = text(detail, member);
    let after_scheme = endpoint
        .strip_prefix("https://")
        .unwrap_or(&endpoint)
        .to_string();
    after_scheme
        .split('/')
        .next()
        .unwrap_or(&after_scheme)
        .to_string()
}

/// The `error.type` an OpenSearch error body carries.
fn error_type(answer: &Value) -> String {
    answer
        .get("error")
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create one index over the data plane, asserting the create was accepted.
async fn create_index(
    router: &MultiServiceRouter,
    host: &str,
    index: &str,
    body: Option<&Value>,
    case: &str,
) {
    let (status, answer) = send(router, host, "PUT", &format!("/{index}"), body).await;
    assert_eq!(
        status, 200,
        "case PUT /{index} ({case}): expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer.get("acknowledged"),
        Some(&json!(true)),
        "case PUT /{index} ({case}): expected acknowledged, got {answer}"
    );
    assert_eq!(
        answer.get("index"),
        Some(&json!(index)),
        "case PUT /{index} ({case}): expected the index named back, got {answer}"
    );
}

/// RULE: a SigV4-`aoss`-signed `PUT https://<collectionEndpoint>/<index>` carrying an index
/// body and no RPC target is SERVED — the index is created in the collection, reported by
/// the data plane's own reads, and reported by the control plane's `GetIndex` as the same
/// index.
///
/// The rows span the index-body shapes a client sends: the vector mapping a knowledge base
/// requires (with and without an explicit space type, and at two dimension counts), a plain
/// text mapping, settings with no mappings, an empty body, and the `IndexName` forms the
/// member's own rule admits.
#[tokio::test]
async fn an_index_created_at_the_published_endpoint_is_held_by_the_collection() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-index-create").await;
    let host = endpoint_host(&collection, "collectionEndpoint");
    let id = text(&collection, "id");

    let rows: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "the recorded knn_vector mapping",
            "bedrock-knowledge-base-index",
            Some(knn_index_body(1536, None)),
        ),
        (
            "a knn_vector mapping naming its space type",
            "vectors-cosine",
            Some(knn_index_body(384, Some("cosinesimil"))),
        ),
        ("a text mapping", "documents", Some(json!({
            "mappings": { "properties": { "title": { "type": "text" } } }
        }))),
        (
            "settings with no mappings",
            "settings-only",
            Some(json!({ "settings": { "index": { "knn": true } } })),
        ),
        ("an empty body", "no-body-at-all", None),
        (
            "a name with an inner hyphen and digits",
            "orders-2026",
            Some(json!({ "mappings": { "properties": { "sku": { "type": "keyword" } } } })),
        ),
        ("a name with an inner underscore", "logs_v2", None),
        ("a name with a dot", "metrics.daily", None),
        ("a digits-only name", "2026", None),
    ];

    for (case, index, body) in rows {
        create_index(&router, &host, index, body.as_ref(), case).await;

        // The data plane reports the index it created: its mappings are the ones the body
        // carried, and the settings carry the three the service owns.
        let (status, answer) = send(&router, &host, "GET", &format!("/{index}"), None).await;
        assert_eq!(
            status, 200,
            "case GET /{index} ({case}): expected 200, got {status} with {answer}"
        );
        let expected_mappings = body
            .as_ref()
            .and_then(|body| body.get("mappings").cloned())
            .unwrap_or_else(|| json!({}));
        let reported = answer
            .get(index)
            .and_then(|view| view.get("mappings"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported, expected_mappings,
            "case GET /{index} ({case}): expected the stored mappings {expected_mappings}, got \
             {reported}"
        );
        let settings = answer
            .get(index)
            .and_then(|view| view.get("settings"))
            .and_then(|settings| settings.get("index"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            settings.get("provided_name"),
            Some(&json!(index)),
            "case GET /{index} ({case}): expected provided_name, got {settings}"
        );
        assert!(
            settings
                .get("uuid")
                .and_then(Value::as_str)
                .is_some_and(|uuid| uuid.len() == 36),
            "case GET /{index} ({case}): expected a uuid setting, got {settings}"
        );

        // `HEAD` answers that the index is there, with no body.
        let (status, answer) = send(&router, &host, "HEAD", &format!("/{index}"), None).await;
        assert_eq!(
            status, 200,
            "case HEAD /{index} ({case}): expected 200, got {status} with {answer}"
        );

        // And the CONTROL plane reports the same index: one index store per collection.
        let (status, code, control) = call(
            &router,
            "GetIndex",
            json!({ "id": &id, "indexName": index }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case GetIndex({index}) ({case}): expected 200, got {status} {code} with {control}"
        );
        let stored = control.get("indexSchema").cloned();
        let expected = body.clone();
        assert_eq!(
            stored, expected,
            "case GetIndex({index}) ({case}): expected the body the create carried \
             {expected:?}, got {stored:?}"
        );
    }

    // A create of a name the collection already holds is the engine's own conflict.
    let (status, answer) = send(
        &router,
        &host,
        "PUT",
        "/documents",
        Some(&knn_index_body(8, None)),
    )
    .await;
    assert_eq!(
        (status, error_type(&answer).as_str()),
        (400, "resource_already_exists_exception"),
        "case PUT /documents (already there): expected 400 resource_already_exists_exception, got \
         {status} with {answer}"
    );
}

/// RULE: every host a collection PUBLISHES addresses its data plane, and so does the
/// reserved path namespace naming this plane and the collection — by id, by name or by a
/// published host.
///
/// The rows span the published addresses `BatchGetCollection` reports and the three ways a
/// prefixed path names the collection.
#[tokio::test]
async fn every_published_address_of_a_collection_reaches_its_data_plane() {
    let router = emulator();
    let name = "data-plane-addressing";
    let collection = reachable_collection(&router, name).await;
    let id = text(&collection, "id");
    let endpoint = endpoint_host(&collection, "collectionEndpoint");
    let dashboards = endpoint_host(&collection, "dashboardEndpoint");
    let fips = endpoint_host(&collection, "fipsEndpoints.collectionEndpoint");
    create_index(&router, &endpoint, "reachable", None, "one index").await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "the published collection endpoint",
            endpoint.clone(),
            "/reachable".to_string(),
        ),
        (
            "the FIPS collection endpoint",
            fips.clone(),
            "/reachable".to_string(),
        ),
        (
            "the Dashboards host",
            dashboards.clone(),
            "/reachable".to_string(),
        ),
        (
            "a prefixed path naming the collection id",
            "127.0.0.1:9297".to_string(),
            format!("/_aws/{SIGNING_NAME}/{id}/reachable"),
        ),
        (
            "a prefixed path naming the collection name",
            "127.0.0.1:9297".to_string(),
            format!("/_aws/{SIGNING_NAME}/{name}/reachable"),
        ),
        (
            "a prefixed path naming the endpoint host",
            "127.0.0.1:9297".to_string(),
            format!("/_aws/{SIGNING_NAME}/{endpoint}/reachable"),
        ),
    ];

    for (case, host, path) in rows {
        let (status, answer) = send(&router, &host, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case GET {path} at {host} ({case}): expected 200, got {status} with {answer}"
        );

        // The same address serves a request carrying NO credential at all, which is the
        // request a client with no AWS credentials sends.
        let (status, answer) =
            send_text(&router, &host, "HEAD", &path, String::new(), false).await;
        assert_eq!(
            status, 200,
            "case HEAD {path} at {host} unsigned ({case}): expected 200, got {status} with {answer}"
        );
    }

    // A host no collection publishes is not this plane's, so the control-plane front door
    // still answers for it — and a control-plane call addressed to the collection's own
    // host is still a control-plane call, because it names an RPC target.
    let (status, code, answer) = call(&router, "BatchGetCollection", json!({ "ids": [&id] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection after the data plane is mounted: expected 200, got {status} \
         {code} with {answer}"
    );
}

/// RULE: a document written over the published endpoint is held by the index, so it is read
/// back, counted and searched out of that one store, and a delete removes it.
///
/// The rows span the document-source shapes a client indexes: scalars, nested objects, a
/// list-valued field, and the vector a knowledge base writes.
#[tokio::test]
async fn a_document_written_at_the_endpoint_is_read_counted_and_searched_back() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-documents").await;
    let host = endpoint_host(&collection, "collectionEndpoint");
    let index = "chunks";
    create_index(
        &router,
        &host,
        index,
        Some(&knn_index_body(3, None)),
        "the vector index",
    )
    .await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a vector chunk",
            "chunk-1",
            json!({
                "bedrock-knowledge-base-default-vector": [1.0, 0.0, 0.0],
                "AMAZON_BEDROCK_TEXT_CHUNK": "the first paragraph of the document",
                "AMAZON_BEDROCK_METADATA": "{\"source\":\"s3://bucket/key\"}",
            }),
        ),
        (
            "a nested object",
            "chunk-2",
            json!({
                "bedrock-knowledge-base-default-vector": [0.0, 1.0, 0.0],
                "AMAZON_BEDROCK_TEXT_CHUNK": "the second paragraph",
                "attributes": { "page": 2, "tags": ["intro", "summary"] },
            }),
        ),
        (
            "scalars only",
            "chunk-3",
            json!({ "AMAZON_BEDROCK_TEXT_CHUNK": "a third paragraph", "page": 3 }),
        ),
    ];

    for (case, document_id, source) in &rows {
        let (status, answer) = send(
            &router,
            &host,
            "PUT",
            &format!("/{index}/_doc/{document_id}"),
            Some(source),
        )
        .await;
        assert_eq!(
            (status, answer.get("result").cloned()),
            (201, Some(json!("created"))),
            "case PUT /{index}/_doc/{document_id} ({case}): expected 201 created, got {status} \
             with {answer}"
        );

        let (status, answer) = send(
            &router,
            &host,
            "GET",
            &format!("/{index}/_doc/{document_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case GET /{index}/_doc/{document_id} ({case}): expected 200, got {status} with \
             {answer}"
        );
        assert_eq!(
            answer.get("_source"),
            Some(source),
            "case GET /{index}/_doc/{document_id} ({case}): expected the written source \
             {source}, got {answer}"
        );
    }

    // A write to an id the index already holds replaces it and advances its version.
    let replaced = json!({ "AMAZON_BEDROCK_TEXT_CHUNK": "a third paragraph, rewritten" });
    let (status, answer) = send(
        &router,
        &host,
        "PUT",
        &format!("/{index}/_doc/chunk-3"),
        Some(&replaced),
    )
    .await;
    assert_eq!(
        (status, answer.get("result").cloned(), answer.get("_version").cloned()),
        (200, Some(json!("updated")), Some(json!(2))),
        "case PUT /{index}/_doc/chunk-3 (replaced): expected 200 updated at version 2, got \
         {status} with {answer}"
    );

    // A write naming no id is answered with the id the service minted for it, and that id
    // reads the document back.
    let (status, answer) = send(
        &router,
        &host,
        "POST",
        &format!("/{index}/_doc"),
        Some(&json!({ "AMAZON_BEDROCK_TEXT_CHUNK": "a minted paragraph" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case POST /{index}/_doc (minted id): expected 201, got {status} with {answer}"
    );
    let minted = answer
        .get("_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !minted.is_empty(),
        "case POST /{index}/_doc (minted id): expected an id, got {answer}"
    );
    let (status, answer) = send(
        &router,
        &host,
        "GET",
        &format!("/{index}/_doc/{minted}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case GET the minted id {minted}: expected 200, got {status} with {answer}"
    );

    // The count is the number of documents the index holds, over every write above.
    let (status, answer) = send(&router, &host, "GET", &format!("/{index}/_count"), None).await;
    assert_eq!(
        (status, answer.get("count").cloned()),
        (200, Some(json!(4))),
        "case GET /{index}/_count: expected 4 documents, got {status} with {answer}"
    );

    // A refresh is served, so a client that refreshes before searching is answered.
    let (status, answer) = send(&router, &host, "POST", &format!("/{index}/_refresh"), None).await;
    assert_eq!(
        status, 200,
        "case POST /{index}/_refresh: expected 200, got {status} with {answer}"
    );

    // Documents one query scores alike are reported in id order, so the expectations for
    // the queries that score every hit alike are the ids sorted.
    let mut all_ids = vec!["chunk-1", "chunk-2", "chunk-3", minted.as_str()];
    all_ids.sort_unstable();
    let mut without_the_first_two = vec!["chunk-3", minted.as_str()];
    without_the_first_two.sort_unstable();

    // (case, the search body, the ids expected in the order the search reports them)
    let searches: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "match_all",
            json!({ "query": { "match_all": {} }, "size": 10 }),
            all_ids,
        ),
        (
            "a term on a nested field",
            json!({ "query": { "term": { "attributes.page": 2 } } }),
            vec!["chunk-2"],
        ),
        (
            "a term on a list-valued field",
            json!({ "query": { "term": { "attributes.tags": "summary" } } }),
            vec!["chunk-2"],
        ),
        (
            "a match on the text chunk",
            json!({ "query": { "match": { "AMAZON_BEDROCK_TEXT_CHUNK": "second" } } }),
            vec!["chunk-2"],
        ),
        (
            // The engine's default operator is OR, so every document carrying ANY of the
            // tokens is a hit and the one carrying more of the field's tokens scores
            // higher: chunk-2 carries both of two tokens over three, the minted document
            // one over three, chunk-3 one over four and chunk-1 one over six.
            "a match whose tokens the documents carry in part",
            json!({ "query": { "match": { "AMAZON_BEDROCK_TEXT_CHUNK": "paragraph second" } } }),
            vec!["chunk-2", minted.as_str(), "chunk-3", "chunk-1"],
        ),
        (
            "a match whose and operator requires every token",
            json!({
                "query": {
                    "match": {
                        "AMAZON_BEDROCK_TEXT_CHUNK": {
                            "query": "paragraph second",
                            "operator": "and",
                        }
                    }
                }
            }),
            vec!["chunk-2"],
        ),
        (
            "a match whose or operator is named explicitly",
            json!({
                "query": {
                    "match": {
                        "AMAZON_BEDROCK_TEXT_CHUNK": {
                            "query": "second rewritten",
                            "operator": "or",
                        }
                    }
                }
            }),
            vec!["chunk-2", "chunk-3"],
        ),
        (
            "a match_phrase whose tokens are in the field in order and adjacent",
            json!({
                "query": { "match_phrase": { "AMAZON_BEDROCK_TEXT_CHUNK": "second paragraph" } }
            }),
            vec!["chunk-2"],
        ),
        (
            "a match_phrase whose tokens are in the field out of order",
            json!({
                "query": { "match_phrase": { "AMAZON_BEDROCK_TEXT_CHUNK": "paragraph second" } }
            }),
            vec![],
        ),
        (
            "a match_phrase whose tokens are in the field but not adjacent",
            json!({
                "query": { "match_phrase": { "AMAZON_BEDROCK_TEXT_CHUNK": "the paragraph" } }
            }),
            vec![],
        ),
        (
            "a bool whose minimum_should_match requires two of three should clauses",
            json!({
                "query": {
                    "bool": {
                        "minimum_should_match": 2,
                        "should": [
                            { "exists": { "field": "AMAZON_BEDROCK_TEXT_CHUNK" } },
                            { "exists": { "field": "attributes.page" } },
                            { "exists": { "field": "bedrock-knowledge-base-default-vector" } },
                        ],
                    }
                }
            }),
            // chunk-2 matches all three clauses and chunk-1 two of them (its text and its
            // vector), so both are hits and the one matching more scores higher; chunk-3
            // and the minted document match one clause each and are not hits.
            vec!["chunk-2", "chunk-1"],
        ),
        (
            "ids",
            json!({ "query": { "ids": { "values": ["chunk-1", "chunk-3"] } } }),
            vec!["chunk-1", "chunk-3"],
        ),
        (
            "exists",
            json!({ "query": { "exists": { "field": "attributes.page" } } }),
            vec!["chunk-2"],
        ),
        (
            "a bool with a must and a must_not",
            json!({
                "query": {
                    "bool": {
                        "must": [{ "exists": { "field": "AMAZON_BEDROCK_TEXT_CHUNK" } }],
                        "must_not": [{ "ids": { "values": ["chunk-1", "chunk-2"] } }],
                    }
                }
            }),
            without_the_first_two,
        ),
        (
            "a knn query nearest the first chunk's vector",
            json!({
                "size": 2,
                "query": {
                    "knn": {
                        "bedrock-knowledge-base-default-vector": {
                            "vector": [0.9, 0.1, 0.0],
                            "k": 2,
                        }
                    }
                }
            }),
            vec!["chunk-1", "chunk-2"],
        ),
    ];

    for (case, body, expected) in searches {
        let (status, answer) = send(
            &router,
            &host,
            "POST",
            &format!("/{index}/_search"),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case POST /{index}/_search ({case}): expected 200, got {status} with {answer}"
        );
        let got: Vec<String> = answer
            .get("hits")
            .and_then(|hits| hits.get("hits"))
            .and_then(Value::as_array)
            .map(|hits| {
                hits.iter()
                    .map(|hit| {
                        hit.get("_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got, expected,
            "case POST /{index}/_search ({case}): expected the hits {expected:?}, got {got:?} in \
             {answer}"
        );
    }

    // A delete removes the document the id names, and the read that follows says so.
    let (status, answer) = send(
        &router,
        &host,
        "DELETE",
        &format!("/{index}/_doc/chunk-1"),
        None,
    )
    .await;
    assert_eq!(
        (status, answer.get("result").cloned()),
        (200, Some(json!("deleted"))),
        "case DELETE /{index}/_doc/chunk-1: expected 200 deleted, got {status} with {answer}"
    );
    let (status, answer) = send(&router, &host, "GET", &format!("/{index}/_doc/chunk-1"), None).await;
    assert_eq!(
        (status, answer.get("found").cloned()),
        (404, Some(json!(false))),
        "case GET /{index}/_doc/chunk-1 (after the delete): expected 404 not found, got {status} \
         with {answer}"
    );
}

/// RULE: a bulk body performs each of its actions on the index store, and reports each
/// action's own outcome.
#[tokio::test]
async fn a_bulk_body_performs_each_action_it_carries() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-bulk").await;
    let host = endpoint_host(&collection, "collectionEndpoint");
    create_index(&router, &host, "bulked", None, "the bulk target").await;

    // One action per line, as the bulk API's NDJSON body is written. The actions span
    // what a bulk body carries: an index, a create, an update and a delete.
    let body = [
        r#"{"index":{"_index":"bulked","_id":"one"}}"#,
        r#"{"title":"the first","words":3}"#,
        r#"{"create":{"_index":"bulked","_id":"two"}}"#,
        r#"{"title":"the second","words":3}"#,
        r#"{"update":{"_index":"bulked","_id":"one"}}"#,
        r#"{"doc":{"words":4}}"#,
        r#"{"delete":{"_index":"bulked","_id":"two"}}"#,
        "",
    ]
    .join("\n");

    let (status, answer) = send_text(&router, &host, "POST", "/_bulk", body, true).await;
    assert_eq!(
        status, 200,
        "case POST /_bulk: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer.get("errors"),
        Some(&json!(false)),
        "case POST /_bulk: expected no errors, got {answer}"
    );
    let items = answer
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let results: Vec<String> = items
        .iter()
        .map(|item| {
            item.as_object()
                .and_then(|members| members.values().next())
                .and_then(|outcome| outcome.get("result"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        results,
        vec!["created", "created", "updated", "deleted"],
        "case POST /_bulk: expected one outcome per action, got {results:?} in {answer}"
    );

    // The store is what the actions left: the updated document carries the merged member
    // and the deleted one is gone.
    let (status, answer) = send(&router, &host, "GET", "/bulked/_doc/one", None).await;
    assert_eq!(
        (status, answer.get("_source").cloned()),
        (200, Some(json!({ "title": "the first", "words": 4 }))),
        "case GET /bulked/_doc/one: expected the merged source, got {status} with {answer}"
    );
    let (status, answer) = send(&router, &host, "GET", "/bulked/_count", None).await;
    assert_eq!(
        (status, answer.get("count").cloned()),
        (200, Some(json!(1))),
        "case GET /bulked/_count: expected 1 document left, got {status} with {answer}"
    );
}

/// RULE: a data-plane request the engine refuses carries the engine's own status and error
/// type, computed from what the collection holds — never the control plane's
/// `SerializationException` about a missing RPC target.
///
/// The rows span the refusal classes: a missing index, an index name the rule rejects, a
/// method a route does not take, a path the surface does not serve, a create of a document
/// id already taken, a query clause this plane does not evaluate, and a page past the
/// result window.
#[tokio::test]
async fn the_data_plane_refuses_what_the_engine_refuses() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-refusals").await;
    let host = endpoint_host(&collection, "collectionEndpoint");
    create_index(&router, &host, "present", None, "the refusal target").await;
    create_index(
        &router,
        &host,
        "vectors",
        Some(&knn_index_body(3, None)),
        "the vector refusal target",
    )
    .await;
    let (status, _) = send(
        &router,
        &host,
        "PUT",
        "/present/_doc/taken",
        Some(&json!({ "title": "already written" })),
    )
    .await;
    assert_eq!(status, 201, "case the document the refusals need: expected 201, got {status}");

    /// One refused request: what it is, how it is addressed, and the status and error type
    /// the engine answers it with.
    struct Row {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<Value>,
        status: u16,
        error_type: &'static str,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a read of an index the collection does not hold",
            method: "GET",
            path: "/absent",
            body: None,
            status: 404,
            error_type: "index_not_found_exception",
        },
        Row {
            case: "a search of an index the collection does not hold",
            method: "POST",
            path: "/absent/_search",
            body: Some(json!({ "query": { "match_all": {} } })),
            status: 404,
            error_type: "index_not_found_exception",
        },
        Row {
            case: "an index name with an uppercase letter",
            method: "PUT",
            path: "/Present",
            body: None,
            status: 400,
            error_type: "invalid_index_name_exception",
        },
        Row {
            case: "an index name beginning with a hyphen",
            method: "PUT",
            path: "/-leading",
            body: None,
            status: 400,
            error_type: "invalid_index_name_exception",
        },
        Row {
            case: "a create of a document id already taken",
            method: "PUT",
            path: "/present/_create/taken",
            body: Some(json!({ "title": "a second write" })),
            status: 409,
            error_type: "version_conflict_engine_exception",
        },
        Row {
            case: "a query clause this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "query": { "span_near": {} } })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a page past the result window",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "from": 10_000, "size": 10 })),
            status: 400,
            error_type: "illegal_argument_exception",
        },
        Row {
            case: "a negative page size",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "size": -1 })),
            status: 400,
            error_type: "illegal_argument_exception",
        },
        Row {
            case: "a knn vector of a dimension the mapping does not declare",
            method: "POST",
            path: "/vectors/_search",
            body: Some(json!({
                "query": {
                    "knn": {
                        "bedrock-knowledge-base-default-vector": { "vector": [1.0], "k": 1 }
                    }
                }
            })),
            status: 400,
            error_type: "illegal_argument_exception",
        },
        Row {
            case: "a bool naming a minimum_should_match this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({
                "query": {
                    "bool": {
                        "minimum_should_match": "75%",
                        "should": [{ "exists": { "field": "title" } }],
                    }
                }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a knn clause naming a filter this plane does not evaluate",
            method: "POST",
            path: "/vectors/_search",
            body: Some(json!({
                "query": {
                    "knn": {
                        "bedrock-knowledge-base-default-vector": {
                            "vector": [1.0, 0.0, 0.0],
                            "k": 1,
                            "filter": { "term": { "tenant": "acme" } },
                        }
                    }
                }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a knn clause naming a min_score this plane does not evaluate",
            method: "POST",
            path: "/vectors/_search",
            body: Some(json!({
                "query": {
                    "knn": {
                        "bedrock-knowledge-base-default-vector": {
                            "vector": [1.0, 0.0, 0.0],
                            "min_score": 0.5,
                        }
                    }
                }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a search body naming a sort this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "query": { "match_all": {} }, "sort": ["title"] })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a search body naming aggregations this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "aggs": { "titles": { "terms": { "field": "title" } } } })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a count body naming a search-only member",
            method: "POST",
            path: "/present/_count",
            body: Some(json!({ "query": { "match_all": {} }, "size": 1 })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a query object naming two clauses",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({
                "query": { "match_all": {}, "term": { "title": "present" } }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a knn clause naming no k",
            method: "POST",
            path: "/vectors/_search",
            body: Some(json!({
                "query": {
                    "knn": {
                        "bedrock-knowledge-base-default-vector": { "vector": [1.0, 0.0, 0.0] }
                    }
                }
            })),
            status: 400,
            error_type: "illegal_argument_exception",
        },
        Row {
            case: "a match clause naming a fuzziness this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({
                "query": { "match": { "title": { "query": "present", "fuzziness": 2 } } }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a match clause naming an operator the engine does not define",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({
                "query": { "match": { "title": { "query": "present", "operator": "xor" } } }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "a term clause naming a boost this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({
                "query": { "term": { "title": { "value": "present", "boost": 2 } } }
            })),
            status: 400,
            error_type: "parsing_exception",
        },
        Row {
            case: "an exists clause naming a member this plane does not evaluate",
            method: "POST",
            path: "/present/_search",
            body: Some(json!({ "query": { "exists": { "field": "title", "boost": 2 } } })),
            status: 400,
            error_type: "parsing_exception",
        },
    ];

    for row in rows {
        let (status, answer) = send(&router, &host, row.method, row.path, row.body.as_ref()).await;
        assert_eq!(
            (status, error_type(&answer).as_str()),
            (row.status, row.error_type),
            "case {} {} ({}): expected {} {}, got {status} with {answer}",
            row.method,
            row.path,
            row.case,
            row.status,
            row.error_type
        );
    }

    // A query whose evaluation would be past what one request may perform is refused
    // before any document is scanned, whether the caller asked for it by nesting a bool
    // too deep or by widening one too far. The clauses are generated rather than written
    // out, so the bound is what the case spans.
    let deep = (0..40).fold(json!({ "match_all": {} }), |nested, _| {
        json!({ "bool": { "must": [nested] } })
    });
    let wide: Vec<Value> = (0..2_000)
        .map(|_| json!({ "exists": { "field": "title" } }))
        .collect();
    let bounded: Vec<(&str, Value, &str)> = vec![
        (
            "a bool nested past the depth bound",
            json!({ "query": deep }),
            "parsing_exception",
        ),
        (
            "a bool wider than the clause bound",
            json!({ "query": { "bool": { "should": wide } } }),
            "too_many_clauses",
        ),
    ];
    for (case, body, expected_type) in bounded {
        let (status, answer) = send(&router, &host, "POST", "/present/_search", Some(&body)).await;
        assert_eq!(
            (status, error_type(&answer).as_str()),
            (400, expected_type),
            "case POST /present/_search ({case}): expected 400 {expected_type}, got {status} with \
             {answer}"
        );
    }

    // A reserved endpoint this surface serves nothing for is answered as a path the engine
    // has no handler for — not read as an index of that name — and the two names the
    // engine reads as every index are served as such.
    for path in ["/_nodes", "/_cat/indices", "/_cluster/health"] {
        let (status, answer) = send(&router, &host, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case GET {path}: expected 400, got {status} with {answer}"
        );
        assert!(
            answer
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|reason| reason.contains("no handler found")),
            "case GET {path}: expected the engine's no-handler refusal, got {answer}"
        );
    }
    for path in ["/_all/_count", "/*/_count"] {
        let (status, answer) = send(&router, &host, "GET", path, None).await;
        assert_eq!(
            (status, answer.get("count").cloned()),
            (200, Some(json!(1))),
            "case GET {path}: expected the collection's one document counted, got {status} with \
             {answer}"
        );
    }

    // A method a route does not take, and a path the surface serves nothing for, are the
    // engine's own two other refusal shapes — neither carries an `error.type`.
    let (status, answer) = send(&router, &host, "DELETE", "/present/_search", None).await;
    assert_eq!(
        status, 405,
        "case DELETE /present/_search: expected 405, got {status} with {answer}"
    );
    let (status, answer) = send(&router, &host, "GET", "/_cat/indices", None).await;
    assert_eq!(
        status, 400,
        "case GET /_cat/indices: expected 400, got {status} with {answer}"
    );
    assert!(
        answer
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("no handler found")),
        "case GET /_cat/indices: expected the engine's no-handler refusal, got {answer}"
    );
}

/// RULE: a read that names no index is over EVERY index the collection holds, and each hit
/// names the index that holds it.
#[tokio::test]
async fn a_read_that_names_no_index_covers_every_index_of_the_collection() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-collection-wide").await;
    let host = endpoint_host(&collection, "collectionEndpoint");

    // Two indexes, each holding one document, so a collection-wide read has to reach both.
    for (index, title) in [("first", "alpha"), ("second", "beta")] {
        create_index(&router, &host, index, None, "two indexes").await;
        let (status, answer) = send(
            &router,
            &host,
            "PUT",
            &format!("/{index}/_doc/{title}"),
            Some(&json!({ "title": title })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case PUT /{index}/_doc/{title}: expected 201, got {status} with {answer}"
        );
    }

    let (status, answer) = send(&router, &host, "GET", "/_count", None).await;
    assert_eq!(
        (status, answer.get("count").cloned()),
        (200, Some(json!(2))),
        "case GET /_count: expected both documents counted, got {status} with {answer}"
    );

    let (status, answer) = send(
        &router,
        &host,
        "POST",
        "/_search",
        Some(&json!({ "query": { "term": { "title": "beta" } } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case POST /_search: expected 200, got {status} with {answer}"
    );
    let hits = answer
        .get("hits")
        .and_then(|hits| hits.get("hits"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let named: Vec<(String, String)> = hits
        .iter()
        .map(|hit| {
            (
                hit.get("_index")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                hit.get("_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        named,
        vec![("second".to_string(), "beta".to_string())],
        "case POST /_search: expected the one hit of the second index, got {named:?} in {answer}"
    );
}

/// RULE: the cluster banner a `GET /` answers is computed from the collection's own record.
#[tokio::test]
async fn the_banner_names_the_collection_it_is_served_for() {
    let router = emulator();
    let collection = reachable_collection(&router, "data-plane-banner").await;
    let host = endpoint_host(&collection, "collectionEndpoint");

    let (status, answer) = send(&router, &host, "GET", "/", None).await;
    assert_eq!(
        status, 200,
        "case GET /: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer.get("name"),
        Some(&json!(text(&collection, "id"))),
        "case GET /: expected the collection id as the node name, got {answer}"
    );
    assert_eq!(
        answer.get("cluster_name"),
        Some(&json!(text(&collection, "name"))),
        "case GET /: expected the collection name as the cluster name, got {answer}"
    );
    assert_eq!(
        answer
            .get("version")
            .and_then(|version| version.get("distribution")),
        Some(&json!("opensearch")),
        "case GET /: expected the OpenSearch distribution, got {answer}"
    );
}
