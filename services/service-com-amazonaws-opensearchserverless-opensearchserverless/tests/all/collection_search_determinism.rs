// SPDX-License-Identifier: CC0-1.0

//! What a client of a collection's published endpoint is promised about REPEATING a
//! vector search: the ranking and the scores do not move.
//!
//! A retrieval-augmented client embeds a question, asks the collection's `_search` for the
//! nearest chunks, and expects the same question over an unchanged index to answer the same
//! chunks in the same order with the same scores — that determinism is the contract a
//! knowledge base's retrieval is measured by, and it holds only if every score is a pure
//! function of the query vector, the stored vectors and the space the index's own mapping
//! names. Nothing in it may depend on iteration order, on the clock, or on how often the
//! search has been run.
//!
//! The suite drives the real request path — `MultiServiceRouter::dispatch` with the method,
//! the path and the `Host` header a client of `CollectionDetail$collectionEndpoint` sends,
//! signed for `aoss` and carrying NO `x-amz-target`, which is the request shape
//! `opensearch-py`'s `indices.create` / `index` / `search` issue. The table spans what the
//! rule generalises over: the spaces a `knn_vector` field's distances can be measured in,
//! including a mapping that names none.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{emulator, text};
use crate::encryption_indexes_and_tagging::reachable_collection;

/// The region the collections below are created in.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which is the name a client signs a request to a
/// collection's endpoint with (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The mapped field the vectors below are written to and searched by.
const VECTOR_FIELD: &str = "bedrock-knowledge-base-default-vector";

/// A SigV4 `Authorization` header scoped to this service: the suite runs with the router's
/// request authentication off, so the header's job is to present the credential scope a
/// client of the endpoint signs with — the data plane is reached as a credentialed
/// operation only for a request that presents one.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDDETERMINISM/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One data-plane request: `method path` to `host`, carrying `body` when there is one.
/// Answers the HTTP status and the parsed body.
async fn send(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(bytes::Bytes::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        serde_json::from_slice(response.body()).unwrap_or(Value::Null),
    )
}

/// The host a collection published as its `collectionEndpoint`, which is what a client puts
/// in the `Host` header: the published URL without its scheme.
fn endpoint_host(detail: &Value) -> String {
    let endpoint = text(detail, "collectionEndpoint");
    let host = endpoint.strip_prefix("https://").unwrap_or(&endpoint);
    host.split('/').next().unwrap_or(host).to_string()
}

/// The `hits` object of a search answer: the ids, their order, their scores and the
/// `max_score` beside them — everything the determinism contract covers.
///
/// The answer's `took` is deliberately NOT part of it: that member is the wall-clock span
/// the search occupied, which two runs are not promised to share and which no ranking
/// depends on.
fn hits(answer: &Value) -> Value {
    answer.get("hits").cloned().unwrap_or(Value::Null)
}

/// The ids of a search answer's hits, in the order it reported them.
fn hit_ids(answer: &Value) -> Vec<String> {
    answer
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
        .unwrap_or_default()
}

/// RULE: a `knn` search over an unchanged index is a pure function of the query — repeating
/// it answers the same hits in the same order with the same scores — and it ranks the
/// stored vector nearest the query first, in whichever space the index's own mapping names.
#[test]
fn a_repeated_vector_search_answers_the_same_ranking_and_the_same_scores() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the test runtime builds");
    runtime.block_on(async {
        let router = emulator();
        let collection = reachable_collection(&router, "vector-search-determinism").await;
        let host = endpoint_host(&collection);

        // The documents every case indexes: vectors at known distances from the query
        // below, so the nearest one is the same whichever space measures them.
        let documents: &[(&str, [f64; 3])] = &[
            ("nearest", [0.9, 0.1, 0.0]),
            ("middle", [0.0, 1.0, 0.0]),
            ("farthest", [-1.0, 0.0, 0.0]),
        ];
        let query = [1.0_f64, 0.0, 0.0];

        // The rows span the spaces a `knn_vector` mapping can name — the k-NN plugin's own
        // `space_type` values — plus a mapping that names none, which the plugin defaults.
        // Each row states the WHOLE order its space ranks the documents in, so what is
        // pinned is the space's own arithmetic and not the handler's sort.
        let cases: &[(&str, Option<&str>, [&str; 3])] = &[
            ("the default space", None, ["nearest", "middle", "farthest"]),
            ("l2", Some("l2"), ["nearest", "middle", "farthest"]),
            (
                "cosinesimil",
                Some("cosinesimil"),
                ["nearest", "middle", "farthest"],
            ),
            (
                "innerproduct",
                Some("innerproduct"),
                ["nearest", "middle", "farthest"],
            ),
        ];
        for (case, space_type, order) in cases {
            let index = format!("chunks-{}", space_type.unwrap_or("default"));
            let mut vector = json!({ "type": "knn_vector", "dimension": 3 });
            if let Some(space_type) = space_type {
                vector["method"] = json!({
                    "name": "hnsw",
                    "engine": "faiss",
                    "space_type": space_type,
                });
            }
            let body = json!({
                "settings": { "index": { "knn": true } },
                "mappings": { "properties": { VECTOR_FIELD: vector } },
            });
            let (status, answer) =
                send(&router, &host, "PUT", &format!("/{index}"), Some(&body)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected the index create accepted, got {status} with {answer}"
            );
            for (id, stored) in documents {
                let source = json!({ VECTOR_FIELD: stored.to_vec() });
                let (status, written) = send(
                    &router,
                    &host,
                    "PUT",
                    &format!("/{index}/_doc/{id}"),
                    Some(&source),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case}/{id}: expected the document created, got {status} with {written}"
                );
            }

            let search = json!({
                "size": 3,
                "query": { "knn": { VECTOR_FIELD: { "vector": query.to_vec(), "k": 3 } } },
            });
            let (status, first) = send(
                &router,
                &host,
                "POST",
                &format!("/{index}/_search"),
                Some(&search),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected the search served, got {status} with {first}"
            );
            let ranked = hit_ids(&first);
            assert_eq!(
                ranked, *order,
                "case {case}: expected the documents ranked {order:?}, got {ranked:?} in {first}"
            );
            let scores: Vec<f64> = first
                .get("hits")
                .and_then(|hits| hits.get("hits"))
                .and_then(Value::as_array)
                .map(|hits| {
                    hits.iter()
                        .map(|hit| hit.get("_score").and_then(Value::as_f64).unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !scores.is_empty() && scores.windows(2).all(|pair| pair[0] >= pair[1]),
                "case {case}: expected the scores ordered by nearness, got {scores:?} in {first}"
            );

            // The same search over the unchanged index: the hits agree — the same ids, in
            // the same order, with the same scores — so the ranking and every score are
            // the query's own function and nothing else's.
            let (status, again) = send(
                &router,
                &host,
                "POST",
                &format!("/{index}/_search"),
                Some(&search),
            )
            .await;
            assert_eq!(
                (status, hits(&again)),
                (200, hits(&first)),
                "case {case}: expected a repeated search to agree with the first, got {again}"
            );
        }
    });
}
