// SPDX-License-Identifier: CC0-1.0

//! The rules the collection control plane enforces beyond storing a record, end to end
//! over the wire: where a collection's ENCRYPTION setting comes from, how a reported
//! `@pattern` constraint is worded, the index a collection holds, its tags, and what an
//! update may change.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The tables below span what each rule generalises over — where the encryption setting
//! comes from and which key it names, which modelled member and which pattern a
//! constraint clause reports, the shape of an index schema, which members an update names
//! — rather than the one request the issue quoted.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::CLIENT_ERROR;

/// The account every ARN this emulator mints carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "OpenSearchServerless";

/// This service's SigV4 signing name, which is what routes a request to it
/// (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// Long enough for a collection's clock-owed transition (`CREATING`/`UPDATING` ->
/// `ACTIVE`, and the purge of a `DELETING` collection) to fall due; the emulator's span is
/// 500 ms and nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// One emulator with an empty store: the shared engine the router builds in production,
/// carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise the service, not SigV4: the request carries a credential scope so
    // the router can name the target service and the region, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call `operation` with `body`; answer its HTTP status, the error code when it failed,
/// and the parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = shape_name(
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or(&header_code),
    );
    (status, code, parsed)
}

/// The bare shape name of an awsJson error code, with any namespace or URI prefix dropped
/// (`com.amazonaws.opensearchserverless#ConflictException` -> `ConflictException`).
fn shape_name(code: &str) -> String {
    code.split('#')
        .next_back()
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// The member of `value` named by `path` (`"createCollectionDetail.id"`), or `Value::Null`.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment).unwrap_or(&Value::Null);
    }
    current
}

/// The string at `path`, or the empty string.
fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

/// The `message` an error body carries, whichever casing the envelope used.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create an `encryption` or `network` security policy, asserting it was accepted.
async fn create_policy(router: &MultiServiceRouter, kind: &str, name: &str, policy: Value) {
    let (status, code, answer) = call(
        router,
        "CreateSecurityPolicy",
        json!({ "type": kind, "name": name, "policy": policy.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy({kind}/{name}): expected 200, got {status} {code} with {answer}"
    );
}

/// Create a collection, asserting it was accepted, and answer its `createCollectionDetail`.
async fn create_collection(router: &MultiServiceRouter, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateCollection", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// The `CollectionDetail` `BatchGetCollection` reports for `id`, or `Value::Null`.
pub(crate) async fn read_collection(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, code, answer) = call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection({id}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "collectionDetails")
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poll until `id` reports `wanted`, answering its `CollectionDetail`. The transition is
/// owed by the clock, so this waits for the wall clock rather than for a call.
async fn await_status(router: &MultiServiceRouter, id: &str, wanted: &str) -> Value {
    for _ in 0..10 {
        let detail = read_collection(router, id).await;
        if text(&detail, "status") == wanted {
            return detail;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case await_status({id}): the collection never reached {wanted}");
}

/// Create a collection encrypted with the AWS-owned key and wait for it to be usable.
pub(crate) async fn active_collection(router: &MultiServiceRouter, name: &str) -> Value {
    let detail = create_collection(
        router,
        json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    let id = text(&detail, "id");
    await_status(router, &id, "ACTIVE").await
}

/// The `network` security policy every collection of one emulator is reached through.
pub(crate) const NETWORK_POLICY_NAME: &str = "net-every-collection";

/// A `network` security policy document: one rule about the collections `resources` names,
/// with public access.
///
/// The shape is the one AWS's "Network access for Amazon OpenSearch Serverless" documents:
/// `Rules` of `ResourceType` `collection` over `collection/<name>` patterns, beside
/// `AllowFromPublic`.
pub(crate) fn network_policy(resources: &[&str]) -> Value {
    json!({
        "Rules": [{ "ResourceType": "collection", "Resource": resources }],
        "AllowFromPublic": true,
    })
}

/// Admit traffic to the endpoints of every collection of this emulator, which is what AWS
/// requires before a collection's endpoint answers anything: "you must have at least one
/// network policy that applies to a collection in order to access its endpoint". Idempotent,
/// so a suite that creates several collections calls it per collection.
pub(crate) async fn allow_network_access(router: &MultiServiceRouter) {
    let (status, _, _) = call(
        router,
        "GetSecurityPolicy",
        json!({ "type": "network", "name": NETWORK_POLICY_NAME }),
    )
    .await;
    if status == 200 {
        return;
    }
    create_policy(
        router,
        "network",
        NETWORK_POLICY_NAME,
        network_policy(&["collection/*"]),
    )
    .await;
}

/// [`active_collection`], with the network policy that makes its endpoints REACHABLE: the
/// collection a data-plane suite addresses.
pub(crate) async fn reachable_collection(router: &MultiServiceRouter, name: &str) -> Value {
    allow_network_access(router).await;
    active_collection(router, name).await
}

/// An encryption policy document: one rule about the collections `resources` names, with
/// the key the policy assigns (a `KmsARN`, or the AWS-owned key).
///
/// The shape is the one AWS's "Encryption policies for Amazon OpenSearch Serverless"
/// documents: `Rules` of `ResourceType` `collection` over `collection/<name>` patterns,
/// beside `AWSOwnedKey` or `KmsARN`.
fn encryption_policy(resources: &[&str], kms_key_arn: Option<&str>) -> Value {
    let mut policy = json!({
        "Rules": [{ "ResourceType": "collection", "Resource": resources }],
    });
    match kms_key_arn {
        Some(arn) => policy["KmsARN"] = json!(arn),
        None => policy["AWSOwnedKey"] = json!(true),
    }
    policy
}

/// A customer managed key ARN satisfying `EncryptionConfig$kmsKeyArn`'s modelled pattern.
fn customer_key() -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/12345678-1234-1234-1234-123456789012")
}

/// A SECOND customer managed key ARN, so a row can tell the key the request named apart
/// from the key the covering policy assigns.
fn other_customer_key() -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/abcdabcd-abcd-abcd-abcd-abcdabcdabcd")
}

// ---------------------------------------------------------------------------
// A collection's encryption setting must come from the request's `encryptionConfig`
// or from a stored `encryption` security policy whose rules cover `collection/<name>`.
// ---------------------------------------------------------------------------

/// One row of the encryption table: the create request, and the `kmsKeyArn` the collection
/// must report — or `None` when the create must be refused.
struct EncryptionCase {
    case: &'static str,
    request: Value,
    expected_key: Option<String>,
}

#[tokio::test]
async fn a_collection_is_created_only_when_an_encryption_setting_exists_for_it() {
    let router = emulator();
    let key = customer_key();
    let other_key = other_customer_key();

    // The policies the rows below are covered (or not) by: an exact resource, a wildcard
    // prefix, one assigning a customer managed key, and a NETWORK policy whose rules name
    // a collection — which says nothing about encryption.
    create_policy(
        &router,
        "encryption",
        "exact-policy",
        encryption_policy(&["collection/exact-covered"], None),
    )
    .await;
    create_policy(
        &router,
        "encryption",
        "wildcard-policy",
        encryption_policy(&["collection/wild-*"], None),
    )
    .await;
    create_policy(
        &router,
        "encryption",
        "cmk-policy",
        encryption_policy(&["collection/cmk-covered*"], Some(&key)),
    )
    .await;
    create_policy(
        &router,
        "network",
        "network-policy",
        json!({
            "Rules": [{ "ResourceType": "collection", "Resource": ["collection/network-covered"] }],
            "AllowFromPublic": true,
        }),
    )
    .await;

    let cases = vec![
        EncryptionCase {
            case: "no encryptionConfig and no policy covering the name",
            request: json!({ "name": "bare-collection" }),
            expected_key: None,
        },
        EncryptionCase {
            case: "the request names the AWS-owned key",
            request: json!({ "name": "owned-key-collection", "encryptionConfig": { "aWSOwnedKey": true } }),
            expected_key: Some("auto".to_string()),
        },
        EncryptionCase {
            case: "the request names a customer managed key",
            request: json!({ "name": "cmk-request-collection", "encryptionConfig": { "kmsKeyArn": key.clone() } }),
            expected_key: Some(key.clone()),
        },
        EncryptionCase {
            case: "an encryption policy matching the name exactly",
            request: json!({ "name": "exact-covered" }),
            expected_key: Some("auto".to_string()),
        },
        EncryptionCase {
            case: "an encryption policy matching the name by wildcard",
            request: json!({ "name": "wild-one" }),
            expected_key: Some("auto".to_string()),
        },
        EncryptionCase {
            case: "another name under the same wildcard policy",
            request: json!({ "name": "wild-two" }),
            expected_key: Some("auto".to_string()),
        },
        EncryptionCase {
            case: "an encryption policy assigning a customer managed key",
            request: json!({ "name": "cmk-covered" }),
            expected_key: Some(key.clone()),
        },
        EncryptionCase {
            case: "a request selecting the AWS-owned key under a policy assigning a CMK",
            request: json!({
                "name": "cmk-covered-owned",
                "encryptionConfig": { "aWSOwnedKey": true },
            }),
            // The request's own choice wins over the covering policy's key.
            expected_key: Some("auto".to_string()),
        },
        EncryptionCase {
            case: "a request naming a CMK under a policy assigning another key",
            request: json!({
                "name": "cmk-covered-other",
                "encryptionConfig": { "kmsKeyArn": other_key.clone() },
            }),
            expected_key: Some(other_key.clone()),
        },
        EncryptionCase {
            case: "a policy covering a different name only",
            request: json!({ "name": "exact-covered-too" }),
            expected_key: None,
        },
        EncryptionCase {
            case: "a NETWORK policy covering the name",
            request: json!({ "name": "network-covered" }),
            expected_key: None,
        },
        EncryptionCase {
            case: "an encryptionConfig naming neither member",
            request: json!({ "name": "empty-config-collection", "encryptionConfig": {} }),
            expected_key: None,
        },
        EncryptionCase {
            case: "an encryptionConfig declining the AWS-owned key and naming no key",
            request: json!({ "name": "no-key-collection", "encryptionConfig": { "aWSOwnedKey": false } }),
            expected_key: None,
        },
    ];

    for EncryptionCase {
        case,
        request,
        expected_key,
    } in cases
    {
        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        match expected_key {
            None => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationException"),
                    "case CreateCollection({case}): expected 400 ValidationException, got {status} {code} with {answer}"
                );
                let stated = message(&answer);
                assert!(
                    stated.contains("encryption"),
                    "case CreateCollection({case}): the refusal must name encryption, got {stated:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case CreateCollection({case}): expected 200, got {status} {code} with {answer}"
                );
                let detail = at(&answer, "createCollectionDetail").clone();
                assert_eq!(
                    text(&detail, "kmsKeyArn"),
                    expected,
                    "case CreateCollection({case}): expected kmsKeyArn {expected}, got {detail}"
                );
                // The stored record is what a later read reports, so the key is not a
                // rendering of this one request.
                let id = text(&detail, "id");
                let read = read_collection(&router, &id).await;
                assert_eq!(
                    text(&read, "kmsKeyArn"),
                    expected,
                    "case CreateCollection({case}): the read-back reports the same key, got {read}"
                );
            }
        }
    }

    // A policy covering EVERY collection name admits a create that names no encryption at
    // all — on its own engine, so it cannot cover the rows above.
    let wide = emulator();
    create_policy(
        &wide,
        "encryption",
        "catch-all-policy",
        encryption_policy(&["collection/*"], None),
    )
    .await;
    let detail = create_collection(&wide, json!({ "name": "any-collection" })).await;
    assert_eq!(
        text(&detail, "kmsKeyArn"),
        "auto",
        "case CreateCollection(covered by a collection/* policy): expected the AWS-owned key marker, got {detail}"
    );
}

// ---------------------------------------------------------------------------
// A constraint clause reports the `@pattern` expression the service advertises:
// the model's expression WITHOUT its `^`/`$` anchors.
// ---------------------------------------------------------------------------

/// One row of the pattern table: a request that violates one modelled `@pattern`, the
/// member path the clause must name, the value it must echo, and the model's own pattern.
struct PatternCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    path: &'static str,
    value: &'static str,
    /// The `@pattern` as the MODEL spells it (`smithy_query(query="shape", shape=..)`),
    /// anchors included — the clause must report it without them.
    modelled: &'static str,
}

#[tokio::test]
async fn a_constraint_clause_reports_the_pattern_without_the_model_anchors() {
    let router = emulator();

    // Each row's `modelled` pattern is the one the model declares for that member:
    // `CollectionGroupName` and `PolicyName` -> ^[a-z][a-z0-9-]+$, `CollectionId` ->
    // ^[a-z0-9]{3,40}$, `PolicyVersion` ->
    // ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$. A member typed
    // `CollectionName` is NOT a row here: its clause reports the class its own validator
    // applies rather than the trait, which `tests/all/reported_pattern.rs` owns.
    let cases = vec![
        PatternCase {
            case: "CollectionGroupName on CreateCollection",
            operation: "CreateCollection",
            request: json!({ "name": "valid-collection", "collectionGroupName": "1-invalid-group", "encryptionConfig": { "aWSOwnedKey": true } }),
            path: "collectionGroupName",
            value: "1-invalid-group",
            modelled: "^[a-z][a-z0-9-]+$",
        },
        PatternCase {
            case: "PolicyName on CreateSecurityPolicy",
            operation: "CreateSecurityPolicy",
            request: json!({ "type": "encryption", "name": "1-invalid-policy", "policy": "{\"Rules\":[]}" }),
            path: "name",
            value: "1-invalid-policy",
            modelled: "^[a-z][a-z0-9-]+$",
        },
        PatternCase {
            case: "PolicyVersion on UpdateSecurityPolicy",
            operation: "UpdateSecurityPolicy",
            request: json!({
                "type": "encryption",
                "name": "some-policy",
                "policyVersion": "not-a-valid-version-token!!",
            }),
            path: "policyVersion",
            value: "not-a-valid-version-token!!",
            modelled: "^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$",
        },
        PatternCase {
            case: "CollectionId inside BatchGetCollection's ids list",
            operation: "BatchGetCollection",
            request: json!({ "ids": ["Not-A-Collection-Id"] }),
            path: "ids.1.member",
            value: "Not-A-Collection-Id",
            modelled: "^[a-z0-9]{3,40}$",
        },
    ];

    for PatternCase {
        case,
        operation,
        request,
        path,
        value,
        modelled,
    } in cases
    {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {code} with {answer}"
        );
        let stated = message(&answer);
        let unanchored = modelled
            .strip_prefix('^')
            .and_then(|body| body.strip_suffix('$'))
            .expect("every row's modelled pattern is anchored at both ends");
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {unanchored}"
            ),
            "case {case}: the clause must echo the submitted value and the unanchored pattern, got {stated:?}"
        );
        assert!(
            !stated.contains(modelled),
            "case {case}: the reported pattern must not carry the model's anchors, got {stated:?}"
        );
    }

    // The other constraint classes keep the framework's own wording, so the rewording
    // above is the pattern clause and nothing else.
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({ "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case CreateCollection(name absent): expected 400 ValidationException, got {status} {code} with {answer}"
    );
    assert_eq!(
        message(&answer),
        "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member \
         must not be null",
        "case CreateCollection(name absent): an absent member keeps the null subject, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// CreateIndex / GetIndex: the schema round-trips through the store, and each
// refusal is the modelled one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_index_schema_round_trips_through_get_index() {
    let router = emulator();
    let collection = active_collection(&router, "indexed-collection").await;
    let id = text(&collection, "id");

    // `IndexSchema` is a document, so the rows span the shapes a document takes: a
    // mappings object, one with nested objects and a list, a scalar, and no schema at all
    // (the member is not `@required`).
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "a mappings object",
            "simple-index",
            Some(json!({ "mappings": { "properties": { "title": { "type": "text" } } } })),
        ),
        (
            "nested objects and a list",
            "nested-index",
            Some(json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": {
                    "properties": {
                        "embedding": { "type": "knn_vector", "dimension": 1536 },
                        "tags": { "type": "keyword" },
                    },
                },
                "aliases": ["read", "write"],
            })),
        ),
        ("a scalar document", "scalar-index", Some(json!("text"))),
        ("no schema at all", "schemaless-index", None),
    ];

    for (case, index_name, schema) in &cases {
        let mut request = json!({ "id": id.clone(), "indexName": index_name });
        if let Some(schema) = schema {
            request["indexSchema"] = schema.clone();
        }
        let (status, code, answer) = call(&router, "CreateIndex", request.clone()).await;
        assert_eq!(
            status, 200,
            "case CreateIndex({case}): expected 200, got {status} {code} with {answer}"
        );

        let (status, code, read) = call(
            &router,
            "GetIndex",
            json!({ "id": id.clone(), "indexName": index_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case GetIndex({case}): expected 200, got {status} {code} with {read}"
        );
        assert_eq!(
            read.get("indexSchema"),
            schema.as_ref(),
            "case GetIndex({case}): the schema must be the one the create stored, got {read}"
        );

        // `CreateIndex` is `@idempotent`: the same create again answers the index it made.
        let (status, code, answer) = call(&router, "CreateIndex", request).await;
        assert_eq!(
            status, 200,
            "case CreateIndex({case}, replayed): expected 200, got {status} {code} with {answer}"
        );
    }

    // A name already taken by an index with a DIFFERENT schema is the modelled conflict.
    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({
            "id": id.clone(),
            "indexName": "simple-index",
            "indexSchema": { "mappings": { "properties": { "title": { "type": "keyword" } } } },
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case CreateIndex(name taken, different schema): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );

    // The refusals, over a table that spans the classes: a collection id that names
    // nothing, a collection that is not yet usable, and the index-name rule
    // `CreateIndexRequest$indexName` documents ("must be lowercase and can't begin with
    // underscores (_) or hyphens (-)").
    let creating = create_collection(
        &router,
        json!({ "name": "creating-collection", "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    let creating_id = text(&creating, "id");
    let absent_id = "0".repeat(20);
    let refusals: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a collection id that names no collection",
            json!({ "id": absent_id.clone(), "indexName": "any-index" }),
            CLIENT_ERROR,
            "ResourceNotFoundException",
        ),
        (
            "a collection still CREATING",
            json!({ "id": creating_id.clone(), "indexName": "any-index" }),
            CLIENT_ERROR,
            "ConflictException",
        ),
        (
            "an index name beginning with an underscore",
            json!({ "id": id.clone(), "indexName": "_system-index" }),
            400,
            "ValidationException",
        ),
        (
            "an index name beginning with a hyphen",
            json!({ "id": id.clone(), "indexName": "-leading-hyphen" }),
            400,
            "ValidationException",
        ),
        (
            "an index name with an uppercase letter",
            json!({ "id": id.clone(), "indexName": "MixedCase" }),
            400,
            "ValidationException",
        ),
        (
            "an empty index name",
            json!({ "id": id.clone(), "indexName": "" }),
            400,
            "ValidationException",
        ),
    ];
    for (case, request, expected_status, expected_code) in refusals {
        let (status, code, answer) = call(&router, "CreateIndex", request).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case CreateIndex({case}): expected {expected_status} {expected_code}, got {status} {code} with {answer}"
        );
    }

    // And the reads: an index that was never created, and a collection that does not
    // exist, are both the modelled not-found.
    for (case, request) in [
        (
            "an index name that names no index",
            json!({ "id": id.clone(), "indexName": "never-created" }),
        ),
        (
            "a collection id that names no collection",
            json!({ "id": absent_id, "indexName": "simple-index" }),
        ),
    ] {
        let (status, code, answer) = call(&router, "GetIndex", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case GetIndex({case}): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// The per-collection index store is BOUNDED: one caller-named entry per request
// to a collection that may live indefinitely cannot grow without limit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_index_store_of_one_collection_stays_bounded() {
    use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_INDEXES_PER_COLLECTION;

    let router = emulator();
    let collection = active_collection(&router, "bounded-collection").await;
    let id = text(&collection, "id");

    // Exactly the cap's worth of indexes is accepted.
    for index in 0..MAX_INDEXES_PER_COLLECTION {
        let (status, code, answer) = call(
            &router,
            "CreateIndex",
            json!({ "id": id.clone(), "indexName": format!("index-{index}") }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateIndex(index {index} of the cap): expected 200, got {status} {code} with {answer}"
        );
    }

    // One past it is refused, and the refusal states the bound.
    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({ "id": id.clone(), "indexName": "one-index-too-many" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case CreateIndex(one past the cap): expected 400 ValidationException, got {status} {code} with {answer}"
    );
    assert!(
        message(&answer).contains(&MAX_INDEXES_PER_COLLECTION.to_string()),
        "case CreateIndex(one past the cap): the refusal states the bound, got {answer}"
    );

    // And a RETRY of an index already stored still succeeds at the cap: the idempotent
    // short-circuit comes before the bound, so a retry is never refused by it.
    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({ "id": id.clone(), "indexName": "index-0" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateIndex(retry at the cap): expected 200, got {status} {code} with {answer}"
    );
}

/// The SIZE of one stored schema is bounded too, along both axes the stored document grows
/// on: how deep it nests and how many values it carries.
#[tokio::test]
async fn one_stored_index_schema_stays_bounded() {
    use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
        MAX_DOCUMENT_DEPTH, MAX_INDEX_SCHEMA_NODES,
    };

    let router = emulator();
    let collection = active_collection(&router, "bounded-schema-collection").await;
    let id = text(&collection, "id");

    /// A schema nesting `levels` deep under one member per level.
    fn nested(levels: usize) -> Value {
        let mut document = json!("leaf");
        for _ in 0..levels {
            document = json!({ "properties": document });
        }
        document
    }

    /// A schema of `values` sibling members, which nests only one level.
    fn wide(values: usize) -> Value {
        let mut members = serde_json::Map::new();
        for value in 0..values {
            members.insert(format!("field-{value}"), json!("keyword"));
        }
        Value::Object(members)
    }

    // (case, schema, expected status) — each row differs along ONE axis of the bound, and
    // each axis is tested at a size the bound admits and at one past it.
    let rows = [
        ("a shallow schema", nested(3), 200),
        ("a schema at the depth bound", nested(MAX_DOCUMENT_DEPTH - 2), 200),
        ("a schema one level past the depth bound", nested(MAX_DOCUMENT_DEPTH + 1), 400),
        ("a schema at the value bound", wide(MAX_INDEX_SCHEMA_NODES - 1), 200),
        ("a schema one value past it", wide(MAX_INDEX_SCHEMA_NODES + 1), 400),
    ];
    for (index, (case, schema, expected)) in rows.into_iter().enumerate() {
        let (status, code, answer) = call(
            &router,
            "CreateIndex",
            json!({
                "id": id.clone(),
                "indexName": format!("schema-{index}"),
                "indexSchema": schema,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {code} with {answer}"
        );
        if expected == 400 {
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException, got {code} with {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tags: what a write leaves, and the refusal every tagging operation owes a
// resource ARN that names no live collection.
// ---------------------------------------------------------------------------

/// The tags `ListTagsForResource` reports for `arn`, sorted by key so the comparison does
/// not depend on write order.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({arn}): expected 200, got {status} {code} with {answer}"
    );
    let mut tags: Vec<(String, String)> = at(&answer, "tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| (text(tag, "key"), text(tag, "value")))
        .collect();
    tags.sort();
    tags
}

/// One row of the tag table: the write, and the tag set the resource must report after it.
struct TagCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn tags_round_trip_and_a_deleted_collection_cannot_be_tagged() {
    let router = emulator();

    // The create's own `tags` are the resource's first tag set.
    let detail = create_collection(
        &router,
        json!({
            "name": "tagged-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
            "tags": [{ "key": "team", "value": "search" }],
        }),
    )
    .await;
    let id = text(&detail, "id");
    let arn = text(&detail, "arn");
    await_status(&router, &id, "ACTIVE").await;
    assert_eq!(
        listed_tags(&router, &arn).await,
        vec![("team".to_string(), "search".to_string())],
        "case ListTagsForResource(after the create): the create's tags are the stored set"
    );

    // One row per shape of tag write: adding keys, replacing a key's value, a no-op list,
    // removing a subset, removing a key the resource never carried, and removing them all.
    let cases = vec![
        TagCase {
            case: "adding two keys",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": [{ "key": "env", "value": "prod" }, { "key": "owner", "value": "data" }] }),
            expected: vec![("env", "prod"), ("owner", "data"), ("team", "search")],
        },
        TagCase {
            case: "replacing the value of a key already carried",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": [{ "key": "env", "value": "staging" }] }),
            expected: vec![("env", "staging"), ("owner", "data"), ("team", "search")],
        },
        TagCase {
            case: "an empty tag list",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": [] }),
            expected: vec![("env", "staging"), ("owner", "data"), ("team", "search")],
        },
        TagCase {
            case: "an empty value, which the model allows",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": [{ "key": "owner", "value": "" }] }),
            expected: vec![("env", "staging"), ("owner", ""), ("team", "search")],
        },
        TagCase {
            case: "removing one key",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["owner"] }),
            expected: vec![("env", "staging"), ("team", "search")],
        },
        TagCase {
            case: "removing a key the resource does not carry",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["never-set"] }),
            expected: vec![("env", "staging"), ("team", "search")],
        },
        TagCase {
            case: "removing the rest",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["env", "team"] }),
            expected: vec![],
        },
    ];
    for TagCase {
        case,
        operation,
        request,
        expected,
    } in cases
    {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {operation}({case}): expected 200, got {status} {code} with {answer}"
        );
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&router, &arn).await,
            expected,
            "case {operation}({case}): expected the stored tags to be {expected:?}"
        );
    }

    // A resource ARN that names no collection this account holds is the modelled
    // not-found, for every tagging operation and for each way the ARN can miss.
    let other_region_arn = format!("arn:aws:aoss:eu-west-1:{ACCOUNT}:collection/{id}");
    let absent_arn = format!(
        "arn:aws:aoss:{REGION}:{ACCOUNT}:collection/{}",
        "0".repeat(20)
    );
    let tagging: Vec<(&str, Value)> = vec![
        ("TagResource", json!({ "tags": [{ "key": "k", "value": "v" }] })),
        ("UntagResource", json!({ "tagKeys": ["k"] })),
        ("ListTagsForResource", json!({})),
    ];
    for (operation, extra) in &tagging {
        for (case, arn) in [
            ("an id no collection carries", absent_arn.clone()),
            ("a collection of another Region", other_region_arn.clone()),
        ] {
            let mut request = extra.clone();
            request["resourceArn"] = json!(arn);
            let (status, code, answer) = call(&router, operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {operation}({case}): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
            );
        }
    }

    // The tag set of one resource is BOUNDED at the maximum `Tags` declares: a write that
    // would take the resource past it is the modelled quota refusal, which no single
    // request's `tags` list could break on its own.
    {
        use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_TAGS_PER_RESOURCE;

        let bounded = create_collection(
            &router,
            json!({ "name": "tag-bounded-collection", "encryptionConfig": { "aWSOwnedKey": true } }),
        )
        .await;
        let bounded_arn = text(&bounded, "arn");
        await_status(&router, &text(&bounded, "id"), "ACTIVE").await;
        // Filled in several writes, so the cap is held over the ACCUMULATED set and not
        // just over one request's list.
        for chunk in 0..(MAX_TAGS_PER_RESOURCE / 10) {
            let tags: Vec<Value> = (0..10)
                .map(|index| json!({ "key": format!("key-{chunk}-{index}"), "value": "v" }))
                .collect();
            let (status, code, answer) = call(
                &router,
                "TagResource",
                json!({ "resourceArn": bounded_arn, "tags": tags }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case TagResource(chunk {chunk} of the cap): expected 200, got {status} {code} with {answer}"
            );
        }
        assert_eq!(
            listed_tags(&router, &bounded_arn).await.len(),
            MAX_TAGS_PER_RESOURCE,
            "case TagResource(at the cap): the resource carries exactly {MAX_TAGS_PER_RESOURCE} tags"
        );
        let (status, code, answer) = call(
            &router,
            "TagResource",
            json!({ "resourceArn": bounded_arn, "tags": [{ "key": "one-tag-too-many", "value": "v" }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ServiceQuotaExceededException"),
            "case TagResource(one past the cap): expected {CLIENT_ERROR} ServiceQuotaExceededException, got {status} {code} with {answer}"
        );
        // Replacing the VALUE of a key already carried is not growth, so it is still
        // accepted at the cap.
        let (status, code, answer) = call(
            &router,
            "TagResource",
            json!({ "resourceArn": bounded_arn, "tags": [{ "key": "key-0-0", "value": "replaced" }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case TagResource(replacing a value at the cap): expected 200, got {status} {code} with {answer}"
        );
    }

    // And the collection whose deletion has been ACCEPTED: its `resourceArn` addresses no
    // resource the tagging plane serves any more, so the writes AND the read are refused
    // alike — and once it is purged nothing addresses it either.
    let (status, code, answer) = call(&router, "TagResource", json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "search" }] })).await;
    assert_eq!(
        status, 200,
        "case TagResource(before the delete): expected 200, got {status} {code} with {answer}"
    );
    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollection(tagged collection): expected 200, got {status} {code} with {answer}"
    );
    // While DELETING: every tagging operation is refused, the read included.
    for (operation, extra) in &tagging {
        let mut request = extra.clone();
        request["resourceArn"] = json!(arn);
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {operation}(while DELETING): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }
    // The collection itself is still READ in `DELETING` until its purge, so the refusal
    // above is the tagging plane's precondition and not the record having gone early.
    assert_eq!(
        text(&read_collection(&router, &id).await, "status"),
        "DELETING",
        "case BatchGetCollection(while DELETING): the collection is still reported in DELETING"
    );
    // After the purge nothing addresses it, for any of the three.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (operation, extra) in &tagging {
        let mut request = extra.clone();
        request["resourceArn"] = json!(arn);
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {operation}(after the purge): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateCollection: the members it applies, the status it leaves the collection
// in, and what the next writer gets.
// ---------------------------------------------------------------------------

/// One row of the update table: the members the request names, and what the collection
/// must report afterwards — including the members the request did NOT name.
struct UpdateCase {
    case: &'static str,
    members: Value,
    expected_description: Option<&'static str>,
    expected_protection: Option<&'static str>,
    expected_vector: Option<Value>,
}

#[tokio::test]
async fn an_update_applies_the_members_it_names_and_leaves_the_rest() {
    let router = emulator();

    // One row per set of members an update may name — `description`, `vectorOptions`,
    // `deletionProtection`, all of them, and none of them.
    let cases = vec![
        UpdateCase {
            case: "description only",
            members: json!({ "description": "the updated description" }),
            expected_description: Some("the updated description"),
            expected_protection: Some("DISABLED"),
            expected_vector: None,
        },
        UpdateCase {
            case: "deletionProtection only",
            members: json!({ "deletionProtection": "ENABLED" }),
            expected_description: None,
            expected_protection: Some("ENABLED"),
            expected_vector: None,
        },
        UpdateCase {
            case: "vectorOptions only",
            members: json!({ "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" } }),
            expected_description: None,
            expected_protection: Some("DISABLED"),
            expected_vector: Some(json!({ "ServerlessVectorAcceleration": "ENABLED" })),
        },
        UpdateCase {
            case: "every member at once",
            members: json!({
                "description": "all of them",
                "deletionProtection": "ENABLED",
                "vectorOptions": { "ServerlessVectorAcceleration": "DISABLED" },
            }),
            expected_description: Some("all of them"),
            expected_protection: Some("ENABLED"),
            expected_vector: Some(json!({ "ServerlessVectorAcceleration": "DISABLED" })),
        },
        UpdateCase {
            case: "no member at all",
            members: json!({}),
            expected_description: None,
            expected_protection: Some("DISABLED"),
            expected_vector: None,
        },
    ];

    for (
        index,
        UpdateCase {
            case,
            members,
            expected_description,
            expected_protection,
            expected_vector,
        },
    ) in cases.into_iter().enumerate()
    {
        let name = format!("updated-collection-{index}");
        let collection = active_collection(&router, &name).await;
        let id = text(&collection, "id");
        let created = at(&collection, "createdDate").as_i64().unwrap_or_default();

        // `vectorOptions` is the one member that reconfigures what the collection RUNS, so
        // it is the one that leaves the collection in the non-terminal `UPDATING`;
        // `description` and `deletionProtection` rewrite what the record remembers, which
        // is done when the call answers, so the collection keeps the `ACTIVE` it was in.
        let reconfigures = members.get("vectorOptions").is_some();
        let expected_status = if reconfigures { "UPDATING" } else { "ACTIVE" };
        let mut request = members.clone();
        request["id"] = json!(id.clone());
        let (status, code, answer) = call(&router, "UpdateCollection", request.clone()).await;
        assert_eq!(
            status, 200,
            "case UpdateCollection({case}): expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "updateCollectionDetail").clone();
        assert_eq!(
            text(&detail, "status"),
            expected_status,
            "case UpdateCollection({case}): expected the update to report {expected_status}, got {detail}"
        );
        assert_eq!(
            (text(&detail, "id"), text(&detail, "name")),
            (id.clone(), name.clone()),
            "case UpdateCollection({case}): the detail names the stored record, got {detail}"
        );
        assert_eq!(
            at(&detail, "createdDate").as_i64(),
            Some(created),
            "case UpdateCollection({case}): createdDate does not move, got {detail}"
        );
        assert!(
            at(&detail, "lastModifiedDate").as_i64().unwrap_or_default() >= created,
            "case UpdateCollection({case}): lastModifiedDate is the write instant, got {detail}"
        );

        // A second writer while a RECONFIGURING update is still outstanding is the modelled
        // conflict, never another code. A metadata-only update leaves nothing outstanding,
        // so the very next write is accepted instead — repeating the row's own members, so
        // what the collection must report below is unchanged either way.
        if reconfigures {
            let (status, code, answer) = call(
                &router,
                "UpdateCollection",
                json!({ "id": id.clone(), "description": "a concurrent write" }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ConflictException"),
                "case UpdateCollection({case}, concurrent writer): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
            );
        } else {
            let (status, code, answer) = call(&router, "UpdateCollection", request).await;
            assert_eq!(
                status, 200,
                "case UpdateCollection({case}, the next write): a metadata-only update leaves the \
                 collection writable, got {status} {code} with {answer}"
            );
        }

        // The collection settles back to ACTIVE on its own, carrying what the update
        // applied — and what it did not name is what the create left.
        let read = await_status(&router, &id, "ACTIVE").await;
        assert_eq!(
            text(&read, "description"),
            expected_description.unwrap_or_default(),
            "case UpdateCollection({case}): expected description {expected_description:?}, got {read}"
        );
        assert_eq!(
            text(&read, "deletionProtection"),
            expected_protection.unwrap_or_default(),
            "case UpdateCollection({case}): expected deletionProtection {expected_protection:?}, got {read}"
        );
        assert_eq!(
            read.get("vectorOptions"),
            expected_vector.as_ref(),
            "case UpdateCollection({case}): expected vectorOptions {expected_vector:?}, got {read}"
        );
        assert_eq!(
            text(&read, "type"),
            text(&collection, "type"),
            "case UpdateCollection({case}): the type the create stored is untouched, got {read}"
        );
    }

    // The refusals an update owes: an id that names no collection (the operation models no
    // not-found error, so it is invalid input) and a value outside a modelled enum.
    let refusals: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an id that names no collection",
            json!({ "id": "0".repeat(20), "description": "nothing to update" }),
            400,
            "ValidationException",
        ),
        (
            "deletionProtection outside DeletionProtection",
            json!({ "id": "0".repeat(20), "deletionProtection": "ON" }),
            400,
            "ValidationException",
        ),
        (
            "vectorOptions outside ServerlessVectorAccelerationStatus",
            json!({ "id": "0".repeat(20), "vectorOptions": { "ServerlessVectorAcceleration": "SOMETIMES" } }),
            400,
            "ValidationException",
        ),
    ];
    for (case, request, expected_status, expected_code) in refusals {
        let (status, code, answer) = call(&router, "UpdateCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case UpdateCollection({case}): expected {expected_status} {expected_code}, got {status} {code} with {answer}"
        );
    }
}

#[tokio::test]
async fn deletion_protection_is_refused_as_invalid_until_an_update_turns_it_off() {
    let router = emulator();

    let detail = create_collection(
        &router,
        json!({
            "name": "protected-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
            "deletionProtection": "ENABLED",
        }),
    )
    .await;
    let id = text(&detail, "id");
    await_status(&router, &id, "ACTIVE").await;

    // The refusal is the collection's own CONFIGURATION, which no retry clears: the
    // modelled `ValidationException` (400), not the `ConflictException` (409) a transient
    // state answers.
    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case DeleteCollection(deletionProtection ENABLED): expected 400 ValidationException, got {status} {code} with {answer}"
    );

    // Turning the setting off is what makes the collection deletable, which is the whole
    // of what the refusal above tells a caller to do.
    let (status, code, answer) = call(
        &router,
        "UpdateCollection",
        json!({ "id": id.clone(), "deletionProtection": "DISABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateCollection(deletionProtection DISABLED): expected 200, got {status} {code} with {answer}"
    );
    await_status(&router, &id, "ACTIVE").await;

    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollection(after the protection was turned off): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(at(&answer, "deleteCollectionDetail"), "status"),
        "DELETING",
        "case DeleteCollection(after the protection was turned off): the delete reports DELETING, got {answer}"
    );
}
