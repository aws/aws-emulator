// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the CloudFront KeyValueStore key-value lifecycle.
//!
//! Every assertion here goes over the wire: a request is dispatched through the same
//! `MultiServiceRouter` the production router builds, so routing, HTTP binding, the
//! JSON codec, modelled-constraint validation and the handler all participate. The
//! tables span the classes the rules generalise over — the pair set, its byte content,
//! the page bound, the operation, the ARN form — rather than one recorded request.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfrontkeyvaluestore_cloudfrontkeyvaluestore::EmulatedService;

/// The ARN the issue's reproduction steps name, used verbatim.
const ISSUE_ARN: &str = "arn:aws:cloudfront::123456789012:key-value-store/11111111-2222-3333-4444-555555555555";

/// A well-formed key value store ARN for `label`. CloudFront is global, so the region
/// field is empty, and the store id is the UUID CloudFront mints:
/// `arn:aws:cloudfront::<account>:key-value-store/<uuid>`.
///
/// The UUID is derived from `label` so each case addresses its own store and the same
/// case always addresses the same one.
///
/// `pub(crate)` so the sibling suite that drives these same routes through every way a
/// client ADDRESSES this service (`sigv4_flavour_routing`) names its stores the one way.
pub(crate) fn arn(label: &str) -> String {
    let digest = hex::encode(Sha256::digest(label.as_bytes()));
    format!(
        "arn:aws:cloudfront::123456789012:key-value-store/{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// One dispatched response, decoded.
struct Reply {
    status: u16,
    headers: HashMap<String, String>,
    body: serde_json::Value,
    raw: String,
}

impl Reply {
    /// The modelled error shape restJson1 resolves from `x-amzn-errortype`.
    fn error_type(&self) -> &str {
        self.headers
            .get("x-amzn-errortype")
            .map_or("", String::as_str)
    }

    fn etag(&self) -> String {
        self.headers
            .get("etag")
            .cloned()
            .unwrap_or_else(|| panic!("no ETag header on {} {}", self.status, self.raw))
    }

    fn i64_at(&self, member: &str) -> i64 {
        self.body[member]
            .as_i64()
            .unwrap_or_else(|| panic!("{member} is not an integer in {}", self.raw))
    }

    fn str_at(&self, member: &str) -> String {
        self.body[member]
            .as_str()
            .unwrap_or_else(|| panic!("{member} is not a string in {}", self.raw))
            .to_string()
    }
}

/// The emulated service behind the production router's dispatch path.
struct Kvs {
    router: MultiServiceRouter,
}

impl Kvs {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator's concern, not this service's; drive the
        // routing and handler path unsigned.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(path)
            // The host-style endpoint is what routes the request to this service.
            .header("host", "cloudfront-keyvaluestore.us-east-1.amazonaws.com")
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::from(body.to_string()))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let raw = String::from_utf8_lossy(response.body()).to_string();
        let body = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        Reply {
            status,
            headers,
            body,
            raw,
        }
    }

    async fn describe(&self, store: &str) -> Reply {
        self.send(
            "GET",
            &format!("/key-value-stores/{}", segment(store)),
            &[],
            "",
        )
        .await
    }

    async fn put(&self, store: &str, key: &str, value: &str, if_match: &str) -> Reply {
        self.send(
            "PUT",
            &format!("/key-value-stores/{}/keys/{}", segment(store), segment(key)),
            &[("if-match", if_match)],
            &serde_json::json!({ "Value": value }).to_string(),
        )
        .await
    }

    async fn get(&self, store: &str, key: &str) -> Reply {
        self.send(
            "GET",
            &format!("/key-value-stores/{}/keys/{}", segment(store), segment(key)),
            &[],
            "",
        )
        .await
    }

    async fn delete(&self, store: &str, key: &str, if_match: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/key-value-stores/{}/keys/{}", segment(store), segment(key)),
            &[("if-match", if_match)],
            "",
        )
        .await
    }

    async fn list(&self, store: &str, max_results: Option<i64>, next_token: Option<&str>) -> Reply {
        let mut query = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = next_token {
            query.push(format!("NextToken={token}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.send(
            "GET",
            &format!("/key-value-stores/{}/keys{suffix}", segment(store)),
            &[],
            "",
        )
        .await
    }

    async fn update(
        &self,
        store: &str,
        if_match: &str,
        puts: &[(&str, &str)],
        deletes: &[&str],
    ) -> Reply {
        let body = serde_json::json!({
            "Puts": puts
                .iter()
                .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
                .collect::<Vec<_>>(),
            "Deletes": deletes
                .iter()
                .map(|key| serde_json::json!({ "Key": key }))
                .collect::<Vec<_>>(),
        });
        self.send(
            "POST",
            &format!("/key-value-stores/{}/keys", segment(store)),
            &[("if-match", if_match)],
            &body.to_string(),
        )
        .await
    }

    /// Put every pair into a fresh store, threading the ETag the way a client must,
    /// and answer the store's ETag afterwards.
    async fn seed(&self, store: &str, pairs: &[(String, String)]) -> String {
        let mut etag = self.describe(store).await.etag();
        for (key, value) in pairs {
            let reply = self.put(store, key, value, &etag).await;
            assert_eq!(
                reply.status, 200,
                "seeding {key:?}: expected 200, got {} {}",
                reply.status, reply.raw
            );
            etag = reply.etag();
        }
        etag
    }
}

/// Percent-encode one URI path segment. An ARN carries `:` and `/`, both of which a
/// non-greedy `@httpLabel` must arrive escaped — exactly as an AWS SDK sends it.
///
/// `pub(crate)` for the same reason as [`arn`]: both suites address the same routes.
pub(crate) fn segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `TotalSizeInBytes` as the service documents it: the summed BYTE length of every
/// stored key and value.
fn expected_total_size(pairs: &BTreeMap<String, String>) -> i64 {
    pairs
        .iter()
        .map(|(key, value)| (key.len() + value.len()) as i64)
        .sum()
}

/// The issue's reproduction sequence, end to end: every value a later call reports is
/// the one the earlier calls wrote.
#[tokio::test]
async fn the_key_value_lifecycle_is_computed_from_persisted_state() {
    let kvs = Kvs::new();
    let store = ISSUE_ARN.to_string();

    // An empty store describes as empty, with a usable version and a Created stamp.
    let empty = kvs.describe(&store).await;
    assert_eq!(empty.status, 200, "describe: got {} {}", empty.status, empty.raw);
    assert_eq!(empty.i64_at("ItemCount"), 0, "describe: {}", empty.raw);
    assert_eq!(empty.i64_at("TotalSizeInBytes"), 0, "describe: {}", empty.raw);
    assert_eq!(empty.str_at("KvsARN"), store, "describe: {}", empty.raw);
    assert_eq!(empty.str_at("Status"), "READY", "describe: {}", empty.raw);
    assert!(
        empty.body["Created"].as_f64().is_some_and(|secs| secs > 0.0),
        "describe: Created must be a timestamp, got {}",
        empty.raw
    );
    let v0 = empty.etag();

    // First put: counts recomputed, a NEW version.
    let first = kvs.put(&store, "color", "blue", &v0).await;
    assert_eq!(first.status, 200, "put color: got {} {}", first.status, first.raw);
    assert_eq!(first.i64_at("ItemCount"), 1, "put color: {}", first.raw);
    assert_eq!(
        first.i64_at("TotalSizeInBytes"),
        "color".len() as i64 + "blue".len() as i64,
        "put color: {}",
        first.raw
    );
    let v1 = first.etag();
    assert_ne!(v1, v0, "put color: the version must advance");

    // The stale version from before the first put is refused, and changes nothing.
    let stale = kvs.put(&store, "size", "large", &v0).await;
    assert_eq!(stale.status, 409, "stale put: got {} {}", stale.status, stale.raw);
    assert_eq!(
        stale.error_type(),
        "ConflictException",
        "stale put: {}",
        stale.raw
    );
    let after_stale = kvs.describe(&store).await;
    assert_eq!(after_stale.etag(), v1, "stale put must not advance the version");
    assert_eq!(
        after_stale.i64_at("ItemCount"),
        1,
        "stale put must not insert: {}",
        after_stale.raw
    );

    // The current version is accepted.
    let second = kvs.put(&store, "size", "large", &v1).await;
    assert_eq!(second.status, 200, "put size: got {} {}", second.status, second.raw);
    assert_eq!(second.i64_at("ItemCount"), 2, "put size: {}", second.raw);
    let v2 = second.etag();

    // GetKey answers the value written, plus the store-wide counts.
    let got = kvs.get(&store, "color").await;
    assert_eq!(got.status, 200, "get color: got {} {}", got.status, got.raw);
    assert_eq!(got.str_at("Key"), "color", "get color: {}", got.raw);
    assert_eq!(got.str_at("Value"), "blue", "get color: {}", got.raw);
    assert_eq!(got.i64_at("ItemCount"), 2, "get color: {}", got.raw);

    // One key per page, then the rest.
    let page1 = kvs.list(&store, Some(1), None).await;
    assert_eq!(page1.status, 200, "list page 1: got {} {}", page1.status, page1.raw);
    let items1 = page1.body["Items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items1.len(), 1, "list page 1: {}", page1.raw);
    let token = page1.str_at("NextToken");
    let page2 = kvs.list(&store, Some(1), Some(&token)).await;
    let items2 = page2.body["Items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items2.len(), 1, "list page 2: {}", page2.raw);
    assert!(
        page2.body["NextToken"].is_null(),
        "list page 2 is the last page, got {}",
        page2.raw
    );
    let listed: Vec<String> = items1
        .iter()
        .chain(items2.iter())
        .map(|item| item["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec!["color".to_string(), "size".to_string()],
        "paging must walk the store in key order"
    );

    // DeleteKey removes the key, recomputes the counts and advances the version.
    let deleted = kvs.delete(&store, "color", &v2).await;
    assert_eq!(deleted.status, 200, "delete color: got {} {}", deleted.status, deleted.raw);
    assert_eq!(deleted.i64_at("ItemCount"), 1, "delete color: {}", deleted.raw);
    let v3 = deleted.etag();
    assert_ne!(v3, v2, "delete color: the version must advance");

    let gone = kvs.get(&store, "color").await;
    assert_eq!(gone.status, 404, "get deleted color: got {} {}", gone.status, gone.raw);
    assert_eq!(
        gone.error_type(),
        "ResourceNotFoundException",
        "get deleted color: {}",
        gone.raw
    );
    assert_eq!(
        kvs.describe(&store).await.i64_at("ItemCount"),
        1,
        "describe after delete must show the decremented count"
    );

    // One batch, one version.
    let batch = kvs
        .update(&store, &v3, &[("a", "1"), ("b", "2")], &["size"])
        .await;
    assert_eq!(batch.status, 200, "update keys: got {} {}", batch.status, batch.raw);
    assert_eq!(batch.i64_at("ItemCount"), 2, "update keys: {}", batch.raw);
    let v4 = batch.etag();
    let final_state = kvs.describe(&store).await;
    assert_eq!(
        final_state.etag(),
        v4,
        "describe must report the version UpdateKeys returned"
    );
    assert_eq!(
        final_state.i64_at("ItemCount"),
        2,
        "describe after the batch: {}",
        final_state.raw
    );
    let remaining: Vec<String> = kvs.list(&store, Some(50), None).await.body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec!["a".to_string(), "b".to_string()],
        "the batch must apply both puts and the delete"
    );
}

/// `ItemCount` is the number of pairs held and `TotalSizeInBytes` their summed BYTE
/// length, whatever the pairs are — reported identically by PutKey, GetKey and
/// DescribeKeyValueStore.
#[tokio::test]
async fn counts_and_sizes_are_derived_from_the_stored_pairs() {
    // Rows differ along count, byte content and length: the empty store, one pair, an
    // empty value, multi-byte UTF-8 (byte length, not character count), the published
    // 1024-byte `Key` maximum, a key re-put under a new value, and a many-key store.
    let long_key = "k".repeat(1024);
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("empty", vec![]),
        ("one", vec![("a".to_string(), "1".to_string())]),
        (
            "empty-value",
            vec![("k".to_string(), String::new())],
        ),
        (
            "two",
            vec![
                ("color".to_string(), "blue".to_string()),
                ("size".to_string(), "large".to_string()),
            ],
        ),
        (
            "multi-byte-utf8",
            vec![("ключ".to_string(), "значение".to_string())],
        ),
        (
            "max-length-key",
            vec![(long_key.clone(), "v".repeat(4096))],
        ),
        (
            "overwrite",
            vec![
                ("k".to_string(), "short".to_string()),
                ("k".to_string(), "a-much-longer-value".to_string()),
            ],
        ),
        (
            "many",
            (0..25)
                .map(|n| (format!("key-{n:03}"), format!("value-{n}")))
                .collect(),
        ),
    ];

    for (case, pairs) in cases {
        let kvs = Kvs::new();
        let store = arn(case);
        let last_etag = kvs.seed(&store, &pairs).await;

        // The pairs that actually survived the sequence (a later put replaces an
        // earlier one for the same key).
        let expected: BTreeMap<String, String> = pairs.iter().cloned().collect();
        let expected_count = expected.len() as i64;
        let expected_size = expected_total_size(&expected);

        let described = kvs.describe(&store).await;
        assert_eq!(
            described.i64_at("ItemCount"),
            expected_count,
            "case {case:?}: expected ItemCount {expected_count}, got {}",
            described.raw
        );
        assert_eq!(
            described.i64_at("TotalSizeInBytes"),
            expected_size,
            "case {case:?}: expected TotalSizeInBytes {expected_size}, got {}",
            described.raw
        );
        assert_eq!(
            described.etag(),
            last_etag,
            "case {case:?}: describe must report the version the last write returned"
        );

        for (key, value) in &expected {
            let got = kvs.get(&store, key).await;
            assert_eq!(
                got.status, 200,
                "case {case:?} key {key:?}: expected 200, got {} {}",
                got.status, got.raw
            );
            assert_eq!(
                got.str_at("Value"),
                *value,
                "case {case:?} key {key:?}: value must round-trip byte for byte"
            );
            assert_eq!(
                got.i64_at("ItemCount"),
                expected_count,
                "case {case:?} key {key:?}: GetKey must report the store-wide count"
            );
            assert_eq!(
                got.i64_at("TotalSizeInBytes"),
                expected_size,
                "case {case:?} key {key:?}: GetKey must report the store-wide size"
            );
        }
    }
}

/// Paging walks the whole store exactly once, in key order, honouring the page bound.
#[tokio::test]
async fn list_keys_walks_the_whole_store_exactly_once_in_key_order() {
    // Rows differ along the key count and the page bound, including both published
    // bounds of `MaxResults` (1 and 50), an omitted `MaxResults` (the model's default
    // of 10), and counts just under, equal to and just over the bound.
    let cases: &[(&str, usize, Option<i64>)] = &[
        ("empty-default", 0, None),
        ("one-per-page", 3, Some(1)),
        ("under-the-bound", 4, Some(5)),
        ("exactly-the-bound", 5, Some(5)),
        ("just-over-the-bound", 6, Some(5)),
        ("omitted-max-results", 12, None),
        ("published-max", 120, Some(50)),
        ("odd-bound", 23, Some(7)),
    ];

    for (case, count, max_results) in cases {
        let kvs = Kvs::new();
        let store = arn(case);
        let pairs: Vec<(String, String)> = (0..*count)
            .map(|n| (format!("key-{n:04}"), format!("value-{n}")))
            .collect();
        kvs.seed(&store, &pairs).await;

        let page_bound = max_results.unwrap_or(10) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let reply = kvs.list(&store, *max_results, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} page {pages}: expected 200, got {} {}",
                reply.status, reply.raw
            );
            let items = reply.body["Items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_bound,
                "case {case:?} page {pages}: expected at most {page_bound} items, got {}",
                items.len()
            );
            for item in &items {
                seen.push(item["Key"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= count + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match reply.body["NextToken"].as_str() {
                None => break,
                Some(next) => {
                    assert_eq!(
                        items.len(),
                        page_bound,
                        "case {case:?} page {pages}: a short page must be the last one, got {}",
                        reply.raw
                    );
                    token = Some(next.to_string());
                }
            }
        }

        let expected: Vec<String> = pairs.iter().map(|(key, _)| key.clone()).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every key exactly once, in key order"
        );
    }
}

/// `MaxResults` outside the range the model publishes (1..50) is refused before any
/// listing happens; the bounds themselves are served.
#[tokio::test]
async fn max_results_outside_its_published_range_is_refused() {
    // The range comes from the Smithy model: ListKeysRequest$MaxResults @range(1..50).
    let cases: &[(i64, u16)] = &[(0, 400), (1, 200), (25, 200), (50, 200), (51, 400)];
    let kvs = Kvs::new();
    let store = arn("max-results-range");
    kvs.seed(&store, &[("a".to_string(), "1".to_string())]).await;

    for (max_results, expected) in cases {
        let reply = kvs.list(&store, Some(*max_results), None).await;
        assert_eq!(
            reply.status, *expected,
            "MaxResults {max_results}: expected HTTP {expected}, got {} {}",
            reply.status, reply.raw
        );
        if *expected == 400 {
            assert_eq!(
                reply.error_type(),
                "ValidationException",
                "MaxResults {max_results}: {}",
                reply.raw
            );
        }
    }
}

/// A page token this service did not mint names no position, so the request is refused
/// rather than silently answered from the first key.
#[tokio::test]
async fn an_unrecognised_next_token_is_refused() {
    let kvs = Kvs::new();
    let store = arn("bad-token");
    kvs.seed(&store, &[("a".to_string(), "1".to_string())]).await;

    for token in ["not-a-token", "zzzz", "abc"] {
        let reply = kvs.list(&store, Some(10), Some(token)).await;
        assert_eq!(
            reply.status, 400,
            "token {token:?}: expected HTTP 400, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ValidationException",
            "token {token:?}: {}",
            reply.raw
        );
    }
}

/// Every mutating operation checks `If-Match` against the store's current version, and
/// a mismatch is a 409 that leaves the store, its version and its contents untouched.
#[tokio::test]
async fn a_stale_if_match_is_refused_and_mutates_nothing() {
    #[derive(Debug, Clone, Copy)]
    enum Mutation {
        Put,
        Delete,
        Update,
    }

    for mutation in [Mutation::Put, Mutation::Delete, Mutation::Update] {
        let kvs = Kvs::new();
        let store = arn("stale-if-match");
        let stale = kvs.describe(&store).await.etag();
        let current = kvs
            .seed(&store, &[("color".to_string(), "blue".to_string())])
            .await;
        assert_ne!(stale, current, "{mutation:?}: the seed must advance the version");

        let before = kvs.describe(&store).await;
        let reply = match mutation {
            Mutation::Put => kvs.put(&store, "size", "large", &stale).await,
            Mutation::Delete => kvs.delete(&store, "color", &stale).await,
            Mutation::Update => {
                kvs.update(&store, &stale, &[("size", "large")], &["color"])
                    .await
            }
        };
        assert_eq!(
            reply.status, 409,
            "{mutation:?}: expected HTTP 409, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ConflictException",
            "{mutation:?}: {}",
            reply.raw
        );

        let after = kvs.describe(&store).await;
        assert_eq!(after.etag(), current, "{mutation:?}: the version must not advance");
        assert_eq!(
            after.i64_at("ItemCount"),
            before.i64_at("ItemCount"),
            "{mutation:?}: the pair count must not change"
        );
        assert_eq!(
            after.body["LastModified"], before.body["LastModified"],
            "{mutation:?}: LastModified must not move on a refused request"
        );
        assert_eq!(
            kvs.get(&store, "color").await.str_at("Value"),
            "blue",
            "{mutation:?}: the stored pair must survive a refused request"
        );
    }
}

/// `If-Match` is `@required` on every mutating operation, so omitting it is a 400
/// `ValidationException` — not a routing failure, and never a silent write.
#[tokio::test]
async fn a_missing_if_match_is_refused_as_a_validation_error() {
    let kvs = Kvs::new();
    let store = arn("no-if-match");
    let etag = kvs.describe(&store).await.etag();

    let path_keys = format!("/key-value-stores/{}/keys", segment(&store));
    let path_one_key = format!("{path_keys}/color");
    let cases: &[(&str, &str, &str, &str)] = &[
        ("PutKey", "PUT", &path_one_key, r#"{"Value":"blue"}"#),
        ("DeleteKey", "DELETE", &path_one_key, ""),
        (
            "UpdateKeys",
            "POST",
            &path_keys,
            r#"{"Puts":[{"Key":"color","Value":"blue"}]}"#,
        ),
    ];

    for (operation, method, path, body) in cases {
        let reply = kvs.send(method, path, &[], body).await;
        assert_eq!(
            reply.status, 400,
            "{operation}: expected HTTP 400, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ValidationException",
            "{operation}: {}",
            reply.raw
        );
    }

    // And nothing was written by any of them.
    let after = kvs.describe(&store).await;
    assert_eq!(after.etag(), etag, "a refused request must not advance the version");
    assert_eq!(
        after.i64_at("ItemCount"),
        0,
        "a refused request must not write: {}",
        after.raw
    );
}

/// Every operation is addressed by a key-value-store ARN; an identifier that is not one
/// resolves to no store, on every operation.
#[tokio::test]
async fn an_arn_that_names_no_key_value_store_is_not_found() {
    // Each row breaks exactly one part of
    // `arn:<partition>:cloudfront::<account-id>:key-value-store/<uuid>`.
    let bad = [
        ("not-an-arn", "my-store".to_string()),
        (
            "wrong-service",
            "arn:aws:s3::123456789012:key-value-store/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "region-present",
            "arn:aws:cloudfront:us-east-1:123456789012:key-value-store/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "short-account",
            "arn:aws:cloudfront::12345:key-value-store/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "non-numeric-account",
            "arn:aws:cloudfront::abcdefghijkl:key-value-store/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "wrong-resource-type",
            "arn:aws:cloudfront::123456789012:distribution/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "missing-id",
            "arn:aws:cloudfront::123456789012:key-value-store/".to_string(),
        ),
        (
            "id-is-not-a-uuid",
            "arn:aws:cloudfront::123456789012:key-value-store/my-store".to_string(),
        ),
        (
            "id-uuid-group-too-short",
            "arn:aws:cloudfront::123456789012:key-value-store/1111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "id-uuid-non-hex",
            "arn:aws:cloudfront::123456789012:key-value-store/zzzzzzzz-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "missing-arn-prefix",
            "aws:cloudfront::123456789012:key-value-store/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
    ];

    let kvs = Kvs::new();
    for (case, store) in &bad {
        let probes: Vec<(&str, Reply)> = vec![
            ("DescribeKeyValueStore", kvs.describe(store).await),
            ("GetKey", kvs.get(store, "color").await),
            ("ListKeys", kvs.list(store, Some(10), None).await),
            ("PutKey", kvs.put(store, "color", "blue", "KV0").await),
            ("DeleteKey", kvs.delete(store, "color", "KV0").await),
            (
                "UpdateKeys",
                kvs.update(store, "KV0", &[("color", "blue")], &[]).await,
            ),
        ];
        for (operation, reply) in probes {
            assert_eq!(
                reply.status, 404,
                "case {case:?} {operation}: expected HTTP 404, got {} {}",
                reply.status, reply.raw
            );
            assert_eq!(
                reply.error_type(),
                "ResourceNotFoundException",
                "case {case:?} {operation}: {}",
                reply.raw
            );
        }
    }

    // A well-formed ARN in any partition names a store, including ones the issue did
    // not cite.
    for partition in ["aws", "aws-cn", "aws-us-gov"] {
        let store = format!(
            "arn:{partition}:cloudfront::123456789012:key-value-store/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
        let reply = kvs.describe(&store).await;
        assert_eq!(
            reply.status, 200,
            "partition {partition:?}: expected HTTP 200, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.str_at("KvsARN"),
            store,
            "partition {partition:?}: the described ARN must be the one addressed"
        );
    }
}

/// `UpdateKeys` applies the whole batch under one version; `DeleteKey` is idempotent.
#[tokio::test]
async fn update_keys_applies_a_whole_batch_and_delete_is_idempotent() {
    let kvs = Kvs::new();
    let store = arn("batch");
    let seeded = kvs
        .seed(
            &store,
            &[
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("c".to_string(), "3".to_string()),
            ],
        )
        .await;

    // A key named by both Deletes and Puts ends up holding the value the batch puts;
    // one call, one new version.
    let batch = kvs
        .update(&store, &seeded, &[("b", "two"), ("d", "4")], &["a", "b"])
        .await;
    assert_eq!(batch.status, 200, "batch: got {} {}", batch.status, batch.raw);
    let after_batch = batch.etag();
    assert_ne!(after_batch, seeded, "batch: the version must advance once");
    assert_eq!(batch.i64_at("ItemCount"), 3, "batch: {}", batch.raw);

    let listed: BTreeMap<String, String> = kvs.list(&store, Some(50), None).await.body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["Key"].as_str().unwrap_or_default().to_string(),
                item["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        BTreeMap::from([
            ("b".to_string(), "two".to_string()),
            ("c".to_string(), "3".to_string()),
            ("d".to_string(), "4".to_string()),
        ]),
        "the batch must apply its deletes and then its puts"
    );

    // An empty batch is accepted and still answers the store's current version.
    let empty_batch = kvs.update(&store, &after_batch, &[], &[]).await;
    assert_eq!(
        empty_batch.status, 200,
        "empty batch: got {} {}",
        empty_batch.status, empty_batch.raw
    );
    assert_eq!(
        empty_batch.i64_at("ItemCount"),
        3,
        "empty batch: {}",
        empty_batch.raw
    );

    // Deleting a key the store does not hold leaves the same end state.
    let absent = kvs
        .delete(&store, "never-written", &empty_batch.etag())
        .await;
    assert_eq!(
        absent.status, 200,
        "delete of an absent key: got {} {}",
        absent.status, absent.raw
    );
    assert_eq!(
        absent.i64_at("ItemCount"),
        3,
        "delete of an absent key must not change the count: {}",
        absent.raw
    );
    assert_eq!(
        kvs.describe(&store).await.etag(),
        absent.etag(),
        "describe must report the version DeleteKey returned"
    );
}

/// Two stores are independent: the pairs, counts and versions of one are invisible to
/// the other.
#[tokio::test]
async fn stores_are_isolated_from_each_other() {
    let kvs = Kvs::new();
    let left = arn("isolation-left");
    let right = arn("isolation-right");

    kvs.seed(&left, &[("only-left".to_string(), "L".to_string())])
        .await;
    kvs.seed(
        &right,
        &[
            ("only-right".to_string(), "R".to_string()),
            ("second".to_string(), "R2".to_string()),
        ],
    )
    .await;

    assert_eq!(
        kvs.describe(&left).await.i64_at("ItemCount"),
        1,
        "the left store must hold only its own pair"
    );
    assert_eq!(
        kvs.describe(&right).await.i64_at("ItemCount"),
        2,
        "the right store must hold only its own pairs"
    );
    assert_eq!(
        kvs.get(&left, "only-right").await.status,
        404,
        "a key of the right store must not be visible in the left one"
    );
    assert_ne!(
        kvs.describe(&left).await.etag(),
        kvs.describe(&right).await.etag(),
        "two stores must not share one version identifier"
    );
}

/// `GetKey` and `ListKeys` are reads: they answer the empty view for a well-formed ARN
/// this emulator holds no record for, rather than materialising one.
#[tokio::test]
async fn reads_answer_the_empty_view_without_materialising_a_store() {
    let kvs = Kvs::new();
    // Never described, never written: no record exists for either of these.
    for label in ["unreferenced-one", "unreferenced-two"] {
        let store = arn(label);

        let got = kvs.get(&store, "color").await;
        assert_eq!(
            got.status, 404,
            "case {label:?}: GetKey must be 404, got {} {}",
            got.status, got.raw
        );
        assert_eq!(
            got.error_type(),
            "ResourceNotFoundException",
            "case {label:?}: {}",
            got.raw
        );

        let listed = kvs.list(&store, Some(10), None).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListKeys must be 200, got {} {}",
            listed.status, listed.raw
        );
        assert_eq!(
            listed.body["Items"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: ListKeys must list nothing, got {}",
            listed.raw
        );
        assert!(
            listed.body["NextToken"].is_null(),
            "case {label:?}: an empty listing has no NextToken, got {}",
            listed.raw
        );
    }
}

/// A write whose result would take the store past its published size quota is refused
/// with the modelled error, and changes nothing.
#[tokio::test]
async fn a_write_past_the_store_size_quota_is_refused() {
    // AWS publishes a maximum key value store size of 5 MB (CloudFront Developer
    // Guide, key value store quotas), which is what `state::MAX_STORE_BYTES` carries.
    const MAX_STORE_BYTES: usize = 5 * 1024 * 1024;

    let kvs = Kvs::new();
    let store = arn("size-quota");
    let empty_version = kvs.describe(&store).await.etag();

    // 4 MiB fits.
    let fits = "x".repeat(4 * 1024 * 1024);
    let accepted = kvs.put(&store, "big", &fits, &empty_version).await;
    assert_eq!(
        accepted.status, 200,
        "a 4 MiB value fits the 5 MB quota: got {} {}",
        accepted.status, accepted.raw
    );
    let version = accepted.etag();
    let held = "big".len() + fits.len();
    assert!(
        held <= MAX_STORE_BYTES,
        "case within-quota: expected {held} bytes to be within {MAX_STORE_BYTES}"
    );

    // Another 2 MiB does not.
    let too_big = "y".repeat(2 * 1024 * 1024);
    assert!(
        held + "bigger".len() + too_big.len() > MAX_STORE_BYTES,
        "case past-quota: the second value must cross {MAX_STORE_BYTES}"
    );
    for (case, reply) in [
        (
            "PutKey",
            kvs.put(&store, "bigger", &too_big, &version).await,
        ),
        (
            "UpdateKeys",
            kvs.update(&store, &version, &[("bigger", too_big.as_str())], &[])
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 402,
            "case {case}: expected HTTP 402, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ServiceQuotaExceededException",
            "case {case}: {}",
            reply.raw
        );
        let after = kvs.describe(&store).await;
        assert_eq!(
            after.etag(),
            version,
            "case {case}: a refused write must not advance the version"
        );
        assert_eq!(
            after.i64_at("ItemCount"),
            1,
            "case {case}: a refused write must not insert: {}",
            after.raw
        );
        assert_eq!(
            after.i64_at("TotalSizeInBytes"),
            held as i64,
            "case {case}: a refused write must not change the size: {}",
            after.raw
        );
    }
}

/// A batch naming more keys than one store may ever hold is refused before anything is
/// staged or written.
#[tokio::test]
async fn an_oversized_batch_is_refused() {
    // `state::MAX_ITEMS_PER_STORE` — a batch longer than the pairs a store may hold can
    // never succeed, so it is refused up front.
    const MAX_ITEMS_PER_STORE: usize = 50_000;

    let kvs = Kvs::new();
    let store = arn("batch-quota");
    let version = kvs.describe(&store).await.etag();

    let owned: Vec<(String, String)> = (0..=MAX_ITEMS_PER_STORE)
        .map(|n| (format!("k{n:06}"), "v".to_string()))
        .collect();
    let puts: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let reply = kvs.update(&store, &version, &puts, &[]).await;
    assert_eq!(
        reply.status, 402,
        "a batch of {} keys must be refused, got {} {}",
        puts.len(),
        reply.status,
        reply.raw
    );
    assert_eq!(
        reply.error_type(),
        "ServiceQuotaExceededException",
        "oversized batch: {}",
        reply.raw
    );
    let after = kvs.describe(&store).await;
    assert_eq!(
        after.etag(),
        version,
        "a refused batch must not advance the version"
    );
    assert_eq!(
        after.i64_at("ItemCount"),
        0,
        "a refused batch must apply none of its puts: {}",
        after.raw
    );
}

/// A wrong `If-Match` is a conflict even for a store this emulator holds no record for
/// yet: the version is compared against the store's initial version, so the request is
/// refused rather than silently accepted or reported as a missing resource.
#[tokio::test]
async fn a_wrong_if_match_on_a_never_referenced_store_is_a_conflict() {
    let kvs = Kvs::new();
    let store = arn("never-referenced-conflict");

    for (operation, reply) in [
        ("PutKey", kvs.put(&store, "color", "blue", "KVNOPE").await),
        ("DeleteKey", kvs.delete(&store, "color", "KVNOPE").await),
        (
            "UpdateKeys",
            kvs.update(&store, "KVNOPE", &[("color", "blue")], &[]).await,
        ),
    ] {
        assert_eq!(
            reply.status, 409,
            "{operation}: expected HTTP 409, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ConflictException",
            "{operation}: {}",
            reply.raw
        );
    }

    // And the store still holds nothing: the refusals wrote neither pairs nor version.
    let described = kvs.describe(&store).await;
    assert_eq!(
        described.i64_at("ItemCount"),
        0,
        "a refused request must not write: {}",
        described.raw
    );

    // The version Describe reports IS the one a mutating call accepts, so the store can
    // be written once the caller has read it.
    let accepted = kvs
        .put(&store, "color", "blue", &described.etag())
        .await;
    assert_eq!(
        accepted.status, 200,
        "the version Describe reports must be accepted, got {} {}",
        accepted.status, accepted.raw
    );
    assert_eq!(accepted.i64_at("ItemCount"), 1, "{}", accepted.raw);
}

/// The missing-`If-Match` refusal holds whatever form the request target takes: a
/// hand-built client that leaves the ARN label's `/` unescaped reaches the same route
/// and gets the same 400, not a routing failure.
#[tokio::test]
async fn a_missing_if_match_is_refused_for_an_unescaped_arn_label() {
    let kvs = Kvs::new();
    let cases: &[(&str, &str, String, &str)] = &[
        (
            "PutKey",
            "PUT",
            format!("/key-value-stores/{ISSUE_ARN}/keys/color"),
            r#"{"Value":"blue"}"#,
        ),
        (
            "DeleteKey",
            "DELETE",
            format!("/key-value-stores/{ISSUE_ARN}/keys/color"),
            "",
        ),
        (
            "UpdateKeys",
            "POST",
            format!("/key-value-stores/{ISSUE_ARN}/keys"),
            r#"{"Puts":[{"Key":"color","Value":"blue"}]}"#,
        ),
    ];

    for (operation, method, path, body) in cases {
        let reply = kvs.send(method, path, &[], body).await;
        assert_eq!(
            reply.status, 400,
            "{operation} with an unescaped ARN label: expected HTTP 400, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_type(),
            "ValidationException",
            "{operation} with an unescaped ARN label: {}",
            reply.raw
        );
    }
}

/// A key value store is addressed by its ARN alone: every partition and account this
/// service publishes an endpoint for names a store of its own, and the metadata members
/// `DescribeKeyValueStore` reports come from that store's record.
///
/// The partition and the account are part of the IDENTIFIER, not of the route — this
/// API's endpoint resolution resolves one account-based endpoint per ARN
/// (`https://<account-id>.cloudfront-kvs.global.api.aws` for a store in `aws`, and
/// `https://<account-id>.cloudfront-kvs.global.api.amazonwebservices.eu` for one in
/// `aws-eusc`), and the store ARN then travels as the `KvsARN` label of every route. So
/// the rows below span that class: both partitions whose ARNs resolve an endpoint, two
/// accounts, and a distinct store id each. The partition and account the data-plane
/// reproduction names are one row of it.
#[tokio::test]
async fn a_store_is_served_for_every_partition_and_account_the_model_addresses() {
    // partition, account id, store id. The partitions are the ones this service's
    // endpoint resolution resolves an endpoint for
    // (`smithy.rules#endpointRuleSet` on
    // `com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore`); the accounts and
    // ids differ row by row so no two rows can name the same store.
    let rows = [
        (
            "aws",
            "123456789012",
            "11111111-2222-3333-4444-555555555555",
        ),
        (
            "aws",
            "000000000000",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        ),
        (
            "aws-eusc",
            "123456789012",
            "0f0f0f0f-1e1e-2d2d-3c3c-4b4b4b4b4b4b",
        ),
        (
            "aws-eusc",
            "210987654321",
            "99999999-8888-7777-6666-555555555555",
        ),
    ];

    let kvs = Kvs::new();
    for (partition, account, id) in rows {
        let case = format!("{partition}/{account}");
        let store = format!("arn:{partition}:cloudfront::{account}:key-value-store/{id}");

        let empty = kvs.describe(&store).await;
        assert_eq!(
            empty.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeKeyValueStore, got {} {}",
            empty.status, empty.raw
        );
        assert_eq!(
            empty.str_at("KvsARN"),
            store,
            "case {case:?}: expected the addressed ARN back, got {}",
            empty.raw
        );
        assert_eq!(
            empty.i64_at("ItemCount"),
            0,
            "case {case:?}: expected an empty store, got {}",
            empty.raw
        );
        assert_eq!(
            empty.i64_at("TotalSizeInBytes"),
            0,
            "case {case:?}: expected an empty store, got {}",
            empty.raw
        );
        // `Status` is the store's current lifecycle status and `LastModified` is when its
        // pairs last changed, so both are reported for a store that serves; `FailureReason`
        // is the reason a store was NOT created, so a store that serves carries none.
        assert_eq!(
            empty.str_at("Status"),
            "READY",
            "case {case:?}: expected a serving store's status, got {}",
            empty.raw
        );
        let created = empty.body["Created"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected Created to be a timestamp, got {}",
                empty.raw
            )
        });
        let last_modified = empty.body["LastModified"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected LastModified to be a timestamp, got {}",
                empty.raw
            )
        });
        assert!(
            last_modified >= created,
            "case {case:?}: expected LastModified at or after Created, got {last_modified} and {created}"
        );
        assert!(
            empty.body.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason on a serving store, got {}",
            empty.raw
        );

        // The ARN that reads the store is the one that writes it: the value this row puts
        // is the value this row reads back.
        let version = empty.etag();
        let put = kvs.put(&store, "color", partition, &version).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from PutKey, got {} {}",
            put.status, put.raw
        );
        assert_ne!(
            put.etag(),
            version,
            "case {case:?}: expected the version to advance past {version}"
        );

        let got = kvs.get(&store, "color").await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetKey, got {} {}",
            got.status, got.raw
        );
        assert_eq!(
            got.str_at("Value"),
            partition,
            "case {case:?}: expected the value written through this ARN, got {}",
            got.raw
        );
        assert_eq!(
            got.i64_at("ItemCount"),
            1,
            "case {case:?}: expected the one pair this ARN wrote, got {}",
            got.raw
        );
    }

    // Every row is its own record: after all four writes each store still holds exactly
    // the one pair IT wrote, so no two partitions or accounts share a store.
    for (partition, account, id) in rows {
        let case = format!("{partition}/{account}");
        let store = format!("arn:{partition}:cloudfront::{account}:key-value-store/{id}");
        let listed = kvs.list(&store, Some(50), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListKeys, got {} {}",
            listed.status, listed.raw
        );
        let items = listed.body["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected exactly the pair this ARN wrote, got {}",
            listed.raw
        );
        assert_eq!(
            items[0]["Value"].as_str(),
            Some(partition),
            "case {case:?}: expected the value this ARN wrote, got {}",
            listed.raw
        );
    }
}

