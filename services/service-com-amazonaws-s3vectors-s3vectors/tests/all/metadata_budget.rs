// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how much metadata ONE vector may carry, served over the real
//! `restJson1` wire through the router production uses.
//!
//! THE RULE: a vector's metadata is bounded by two SEPARATE per-vector budgets — the
//! FILTERABLE part, every key the index does not declare in
//! `MetadataConfiguration$nonFilterableMetadataKeys`, has the smaller one, and the whole
//! document, filterable and non-filterable together, the larger — and `PutVectors` refuses
//! a vector over either with `ValidationException` (HTTP 400), storing nothing of the
//! batch. Size is counted over the document as a whole rather than per key, so several
//! small filterable keys that together cross the budget are refused exactly as one big one
//! is, and declaring a key non-filterable takes its bytes out of the smaller budget and
//! leaves them in the larger — which is what that declaration is for.
//!
//! The refusal's SENTENCE is part of the rule: it names the budget that was crossed as the
//! sentence's subject and states that budget's PUBLISHED bound — "<budget> must have at
//! most <n> bytes" — carried after the per-record prefix naming the record by the `key` the
//! caller submitted it under, and never echoing the submitted size. That sentence is the
//! only thing telling a client which budget to trim for, since every per-record defect of
//! `PutVectors` shares `ValidationException` (400).
//!
//! The rows span what the rule generalises over: which budget is at stake, whether the key
//! carrying the bytes is declared non-filterable, the size (far under, exactly at each
//! published bound, one byte past it, and far over), how the bytes are spread (one key,
//! several keys, a nested document, an array), and whether the index declares any
//! non-filterable keys at all.

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::metadata_budget::{
    MAX_FILTERABLE_METADATA_BYTES, MAX_TOTAL_METADATA_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, field_list, message_of, ok, router};

/// The clause a vector over the FILTERABLE budget is refused with, and the one a vector
/// over the TOTAL budget is refused with. LITERALS, down to the published bound in bytes,
/// not the handler's own helper: a suite asserting that helper would agree with any
/// wording the handler chose.
const OVER_FILTERABLE: &str = "Filterable metadata must have at most 2048 bytes";
const OVER_TOTAL: &str = "Metadata must have at most 40960 bytes";

/// The sentence a refusal about ONE record of a batch carries: the record named by the
/// `key` the caller submitted it under, then the defect's own clause.
fn record_sentence(key: &str, clause: &str) -> String {
    format!("Invalid record for key '{key}': {clause}")
}

/// The bucket both indexes live in.
const BUCKET: &str = "metadata-budget-bucket";

/// The index that declares one metadata key non-filterable, and the key it declares.
const DECLARED_INDEX: &str = "metadata-budget-declared";
const DECLARED_KEY: &str = "blob";

/// The index that declares none, where every metadata key is filterable.
const PLAIN_INDEX: &str = "metadata-budget-plain";

/// A key the neither index declares, so its bytes always count against the filterable
/// budget.
const FILTERABLE_KEY: &str = "note";

/// A router holding the bucket and both indexes.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": DECLARED_INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
            "metadataConfiguration": { "nonFilterableMetadataKeys": [DECLARED_KEY] },
        }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": PLAIN_INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    router
}

/// How many bytes the JSON of a one-member object costs BESIDES the member's value: the
/// braces, the quoted key and the colon after it.
fn object_overhead(key: &str) -> usize {
    // `{` `}` + `"` key `"` + `:` + the value's own two quotes.
    2 + key.len() + 2 + 1 + 2
}

/// A metadata document `{ key: "x…" }` whose JSON is exactly `bytes` long.
fn metadata_of_size(key: &str, bytes: usize) -> Value {
    let filler = "x".repeat(bytes - object_overhead(key));
    json!({ key: filler })
}

/// `PutVectors` of one vector carrying `metadata` into `index`.
async fn put(router: &MultiServiceRouter, index: &str, key: &str, metadata: &Value) -> (u16, Value) {
    call(
        router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": index,
            "vectors": [{
                "key": key,
                "data": { "float32": [1.0, 0.0] },
                "metadata": metadata,
            }],
        }),
    )
    .await
}

/// One row: the index it is put into, the document, and the clause the put must be refused
/// with — `None` when the document fits both budgets and must be stored.
struct BudgetCase {
    case: String,
    index: &'static str,
    metadata: Value,
    over: Option<&'static str>,
}

/// The rows.
fn budget_cases() -> Vec<BudgetCase> {
    // A value whose own JSON is this long sits exactly at the filterable budget; one byte
    // more is one byte past it.
    let at_filterable = MAX_FILTERABLE_METADATA_BYTES;
    let past_filterable = MAX_FILTERABLE_METADATA_BYTES + 1;
    // The ~8 KiB value the recorded case put under an undeclared key.
    let far_over_filterable = 8 * 1024;
    vec![
        BudgetCase {
            case: "a small filterable value".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({ FILTERABLE_KEY: "scifi" }),
            over: None,
        },
        BudgetCase {
            case: "no metadata at all".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({}),
            over: None,
        },
        BudgetCase {
            case: format!("a filterable value exactly at the {at_filterable}-byte budget"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, at_filterable),
            over: None,
        },
        BudgetCase {
            case: format!("a filterable value one byte past the {at_filterable}-byte budget"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, past_filterable),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: format!("a {far_over_filterable}-byte value under an UNDECLARED key"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, far_over_filterable),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: format!("the same {far_over_filterable} bytes under the DECLARED key"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, far_over_filterable),
            over: None,
        },
        BudgetCase {
            case: "the declared key's name on an index that declares nothing".to_string(),
            index: PLAIN_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, far_over_filterable),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: "the budget counted over the whole document, not per key".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                "one": "y".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
                "two": "z".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
            }),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: "the declared key's bytes left out of the sum".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: "y".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
                DECLARED_KEY: "z".repeat(MAX_FILTERABLE_METADATA_BYTES),
            }),
            over: None,
        },
        BudgetCase {
            case: "a NESTED filterable document over the budget".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: { "inner": { "deeper": "y".repeat(MAX_FILTERABLE_METADATA_BYTES) } },
            }),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: "a filterable ARRAY over the budget".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: vec!["y".repeat(MAX_FILTERABLE_METADATA_BYTES / 4); 8],
            }),
            over: Some(OVER_FILTERABLE),
        },
        BudgetCase {
            case: format!(
                "a declared value exactly at the {MAX_TOTAL_METADATA_BYTES}-byte total budget"
            ),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, MAX_TOTAL_METADATA_BYTES),
            over: None,
        },
        BudgetCase {
            case: format!(
                "a declared value one byte past the {MAX_TOTAL_METADATA_BYTES}-byte total budget"
            ),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, MAX_TOTAL_METADATA_BYTES + 1),
            over: Some(OVER_TOTAL),
        },
        BudgetCase {
            case: "a declared value far past the total budget".to_string(),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, 2 * MAX_TOTAL_METADATA_BYTES),
            over: Some(OVER_TOTAL),
        },
    ]
}

/// Each document is accepted exactly when it fits both budgets of the index it is put
/// into, and refused with the modelled 400 when it does not.
#[tokio::test]
async fn a_vectors_metadata_is_accepted_exactly_when_it_fits_both_budgets() {
    let router = fixture().await;
    for (position, row) in budget_cases().into_iter().enumerate() {
        let BudgetCase {
            case,
            index,
            metadata,
            over,
        } = row;
        let vector_key = format!("v{position}");
        let (status, body) = put(&router, index, &vector_key, &metadata).await;
        if let Some(clause) = over {
            // The refusal says WHICH budget was crossed and what its PUBLISHED bound is,
            // so a client can tell the two apart and knows what to trim for, after the
            // prefix naming the record by its own key; the submitted size is not in it.
            assert_eq!(
                (
                    status,
                    error_code(&body).as_str(),
                    message_of(&body).as_str()
                ),
                (
                    400,
                    "ValidationException",
                    record_sentence(&vector_key, clause).as_str()
                ),
                "case {case:?}: must be refused with the budget's own sentence, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: must be accepted, got {status} {body}"
            );
            // A stored vector reads back with the metadata it was stored with.
            let reported = ok(
                &router,
                "GetVectors",
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": index,
                    "keys": [vector_key],
                    "returnMetadata": true,
                }),
            )
            .await;
            assert_eq!(
                reported["vectors"][0]["metadata"], metadata,
                "case {case:?}: the stored metadata must read back, got {reported}"
            );
        }
    }
}

/// A refused put stores NOTHING of its batch: the vector it would have written is not in
/// the index afterwards, and a vector the same batch carried beside it is not either.
#[tokio::test]
async fn a_refused_put_stores_nothing_of_its_batch() {
    let router = fixture().await;
    let over_budget = metadata_of_size(FILTERABLE_KEY, 8 * 1024);
    let (status, body) = call(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": DECLARED_INDEX,
            "vectors": [
                { "key": "fits", "data": { "float32": [1.0, 0.0] } },
                {
                    "key": "over",
                    "data": { "float32": [0.0, 1.0] },
                    "metadata": over_budget,
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "the batch must be refused, got {status} {body}"
    );
    // The refusal names the element of `vectors` the over-budget record travelled in and
    // carries the budget's clause alone, while the top-level sentence names the record by
    // the key the caller submitted it under.
    assert_eq!(
        field_list(&body),
        vec![("vectors[1]".to_string(), OVER_FILTERABLE.to_string())],
        "the refusal must name the record's element and carry the clause alone, got {body}"
    );
    assert_eq!(
        message_of(&body),
        record_sentence("over", OVER_FILTERABLE),
        "the refusal must name the record's key, got {body}"
    );

    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": BUCKET, "indexName": DECLARED_INDEX }),
    )
    .await;
    assert_eq!(
        listed["vectors"],
        json!([]),
        "the refused batch must have stored nothing, got {listed}"
    );
}

/// The clause is the budget's own sentence whatever the request looked like: it names the
/// crossed budget and that budget's published bound, carries no value from the request
/// besides the record's key in the prefix, and differs between the two budgets — which is
/// what tells a client whether to trim the document or move a key under a non-filterable
/// one. The rows vary the budget crossed, how far past it the document is, how its bytes
/// are spread, the form of the record's key and the record's position in the batch.
#[tokio::test]
async fn the_over_budget_clause_names_the_crossed_budget_and_its_published_bound() {
    // The literals above are the published bounds, which this suite's documents are built
    // from, so a bound that moved would be caught here rather than in a stale sentence.
    assert_eq!(
        (MAX_FILTERABLE_METADATA_BYTES, MAX_TOTAL_METADATA_BYTES),
        (2048, 40960),
        "the clauses' literal bounds must be the published budgets"
    );

    // (case, the record's key, its position in the batch, the document, the clause)
    let rows: Vec<(&str, &str, usize, Value, &str)> = vec![
        (
            "one filterable key just past the filterable budget",
            "vec-big-filterable",
            0,
            metadata_of_size(FILTERABLE_KEY, MAX_FILTERABLE_METADATA_BYTES + 1),
            OVER_FILTERABLE,
        ),
        (
            "a filterable key far past the filterable budget",
            "a.key-with.punctuation",
            2,
            metadata_of_size(FILTERABLE_KEY, 8 * 1024),
            OVER_FILTERABLE,
        ),
        (
            "several filterable keys summing past the filterable budget",
            "9f41c2ab-7de0-4a11-8f3e-0b1d2c3e4f55",
            1,
            json!({
                "one": "y".repeat(MAX_FILTERABLE_METADATA_BYTES),
                "two": "z".repeat(MAX_FILTERABLE_METADATA_BYTES),
            }),
            OVER_FILTERABLE,
        ),
        (
            "a declared key just past the total budget",
            "k",
            0,
            metadata_of_size(DECLARED_KEY, MAX_TOTAL_METADATA_BYTES + 1),
            OVER_TOTAL,
        ),
        (
            "a declared key far past the total budget",
            "vec-huge",
            3,
            metadata_of_size(DECLARED_KEY, 4 * MAX_TOTAL_METADATA_BYTES),
            OVER_TOTAL,
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, key, position, metadata, clause) in rows {
        let router = fixture().await;
        // Well-formed records at every other position, so the refusal is about the one
        // record and the key it names is the offending one rather than the batch's first.
        let vectors: Vec<Value> = (0..=position)
            .map(|nth| {
                if nth == position {
                    json!({ "key": key, "data": { "float32": [1.0, 0.0] }, "metadata": metadata })
                } else {
                    json!({ "key": format!("good-{nth}"), "data": { "float32": [0.0, 1.0] } })
                }
            })
            .collect();
        let (status, body) = call(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": BUCKET,
                "indexName": DECLARED_INDEX,
                "vectors": vectors,
            }),
        )
        .await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (
                400,
                "ValidationException",
                record_sentence(key, clause).as_str()
            ),
            "case {case:?}: must carry the crossed budget's own sentence, got {status} {body}"
        );
        assert_eq!(
            field_list(&body),
            vec![(format!("vectors[{position}]"), clause.to_string())],
            "case {case:?}: fieldList must name the record's element and carry the clause \
             alone, got {body}"
        );
        // The bound is the published one; the SUBMITTED size is not interpolated.
        let submitted = ::serde_json::to_string(&metadata)
            .expect("the row's document renders")
            .len();
        assert!(
            !message.contains(&submitted.to_string()),
            "case {case:?}: the sentence must not echo the submitted size {submitted}, \
             got {message:?}"
        );
        seen.push(message);
    }
    assert_ne!(
        seen[0], seen[3],
        "the two budgets' refusals must be different sentences, got {:?} twice",
        seen[0]
    );
}
