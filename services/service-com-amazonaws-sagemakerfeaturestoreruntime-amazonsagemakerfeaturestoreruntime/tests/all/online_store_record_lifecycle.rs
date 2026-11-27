// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the OnlineStore record lifecycle: `PutRecord`,
//! `GetRecord`, `BatchGetRecord`, `ListRecords` and `DeleteRecord`.
//!
//! Every case goes through the real front door — the multi-service router, the
//! restJson1 protocol, the modelled constraint validation — so what is asserted is the
//! wire behaviour a client sees, not a handler's return value.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::state::StoredRecord;
use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::{
    ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState as OnlineStore,
    EmulatedService,
};

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A booted emulator with the feature groups these suites write to already created.
    ///
    /// Every operation of this data plane resolves its `FeatureGroupName` against the
    /// feature groups the SageMaker CONTROL plane holds, so a group has to exist before a
    /// record can be written to it — exactly as a client has to call
    /// `CreateFeatureGroup` first.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let emulator = Self { engine, router };
        for (group, identifier) in [
            ("customers", "customer_id"),
            ("orders", "order_id"),
            ("ttlgroup", "customer_id"),
            ("expiring", "customer_id"),
        ] {
            emulator.create_feature_group(group, identifier, "event_time", true);
        }
        emulator
    }

    /// Create a feature group with an OnlineStore, the way the SageMaker control plane's
    /// `CreateFeatureGroup` does: by registering its definition in the ONE shared
    /// feature-group directory this data plane resolves names against.
    fn create_feature_group(
        &self,
        name: &str,
        record_identifier_feature_name: &str,
        event_time_feature_name: &str,
        online_store_enabled: bool,
    ) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!(
                    "arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"
                ),
                record_identifier_feature_name: record_identifier_feature_name.to_owned(),
                event_time_feature_name: event_time_feature_name.to_owned(),
                online_store_enabled,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
                creation_failed: false,
            });
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for
    /// an empty body).
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        // The SigV4 credential scope is how the router picks the claiming service: the
        // 4th field is this service's signing name.
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/us-east-1/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "featurestore-runtime.sagemaker.us-east-1.amazonaws.com")
            .body(
                body.map(|value| bytes::Bytes::from(value.to_string()))
                    .unwrap_or_default(),
            )
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    async fn put_record(&self, group: &str, body: Value) -> (u16, Value) {
        self.send("PUT", &format!("/FeatureGroup/{group}"), Some(body))
            .await
    }

    async fn get_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.send("GET", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    async fn delete_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.send("DELETE", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    async fn list_records(&self, group: &str, body: Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{group}/ListRecords"),
            Some(body),
        )
        .await
    }

    async fn batch_get_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/BatchGetRecord", Some(body)).await
    }

    /// The record `identifier` names in `group`'s online store, read through the store's
    /// OWN API ([`OnlineStore`]) rather than off the wire.
    ///
    /// A soft-deleted record is served by no operation of this data plane — `GetRecord`
    /// and `BatchGetRecord` withhold it and `ListRecords` reports only its identifier — so
    /// the FEATURE VALUES a tombstone keeps are read here, from the same store the handlers
    /// write. That this IS that store is shown by every other assertion in this suite, each
    /// of which goes through the front door.
    fn stored_record(&self, group: &str, identifier: &str) -> Option<StoredRecord> {
        let state = self.engine.state::<OnlineStore>();
        let mut store = state.lock();
        store.group_mut(group)?.record(identifier).cloned()
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// The `Record` member of a `GetRecord` response, as `(FeatureName, Value)` pairs where
/// the value is the scalar or the list the response carried.
fn record_pairs(body: &Value) -> Vec<(String, Value)> {
    body.get("Record")
        .and_then(Value::as_array)
        .map(|features| {
            features
                .iter()
                .map(|feature| {
                    let name = feature["FeatureName"].as_str().unwrap_or_default().to_owned();
                    let value = feature
                        .get("ValueAsString")
                        .or_else(|| feature.get("ValueAsStringList"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    (name, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The wire error code of an error response: restJson1 carries it as the body's
/// `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Whether `code` is one of the two 400 client-refusal codes this emulator answers: the
/// boundary validator's synthetic `ValidationException` or this service's own modelled
/// `ValidationError`.
fn is_client_refusal(code: &str) -> bool {
    matches!(code, "ValidationError" | "ValidationException")
}

/// `event_time + seconds`, as the instant an ISO-8601 `ExpiresAt` must denote.
fn secs(iso_or_epoch: &str) -> f64 {
    DateTime::from_str(iso_or_epoch, Format::DateTime)
        .map(|parsed| parsed.as_secs_f64())
        .unwrap_or_else(|_| iso_or_epoch.parse::<f64>().expect("a parsable event time"))
}

/// An event time one day from now, so a record written with a TTL of any size in these
/// tests is still live when the test reads it back.
fn tomorrow() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs() as f64;
    now + 86_400.0
}

/// [`tomorrow`] as the ISO-8601 string an event-time feature of type `String` carries.
fn tomorrow_iso() -> String {
    DateTime::from_secs_f64(tomorrow())
        .fmt(Format::DateTime)
        .expect("format an ISO-8601 event time")
}

/// [`tomorrow`] as the epoch-seconds string an event-time feature of type `Fractional`
/// carries.
fn tomorrow_epoch() -> String {
    format!("{}", tomorrow() as i64)
}

#[tokio::test]
async fn get_record_returns_exactly_the_feature_values_put_record_stored() {
    let emulator = Emulator::new();

    // The class the rule spans: what a record's feature values can look like. One row
    // per shape of value — scalar, list, numeric string, empty string, unicode, a
    // single-feature record, and a record whose features are in an unusual order.
    let cases: Vec<(&str, Value)> = vec![
        (
            "scalar features",
            json!([
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle")
            ]),
        ),
        (
            "collection feature",
            json!([
                feature("customer_id", "C2"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                { "FeatureName": "tags", "ValueAsStringList": ["a", "b", "c"] }
            ]),
        ),
        (
            "numeric and empty values",
            json!([
                feature("customer_id", "C3"),
                feature("event_time", "1704067200"),
                feature("balance", "-17.5"),
                feature("note", "")
            ]),
        ),
        (
            "unicode value",
            json!([
                feature("customer_id", "C4"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "東京 / Zürich")
            ]),
        ),
        (
            "identifier-only record",
            json!([feature("customer_id", "C5")]),
        ),
        (
            "event time before the identifier is not the identifier",
            json!([
                feature("customer_id", "C6"),
                feature("event_time", "2024-01-02T00:00:00Z")
            ]),
        ),
    ];

    for (case, record) in &cases {
        let (status, _) = emulator
            .put_record("customers", json!({ "Record": record }))
            .await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");
    }

    for (case, record) in &cases {
        let identifier = record[0]["ValueAsString"].as_str().expect("an identifier");
        let expected: Vec<(String, Value)> = record
            .as_array()
            .expect("a record")
            .iter()
            .map(|feature| {
                let name = feature["FeatureName"].as_str().unwrap_or_default().to_owned();
                let value = feature
                    .get("ValueAsString")
                    .or_else(|| feature.get("ValueAsStringList"))
                    .cloned()
                    .unwrap_or(Value::Null);
                (name, value)
            })
            .collect();

        let (status, body) = emulator
            .get_record(
                "customers",
                &format!("RecordIdentifierValueAsString={identifier}"),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = record_pairs(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored record {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn put_record_is_a_full_over_write_of_the_identified_record() {
    let emulator = Emulator::new();

    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle"),
                feature("note", "first")
            ] }),
        )
        .await;
    // A later event time, so this is not a historic record.
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-02T00:00:00Z"),
                feature("city", "Boston")
            ] }),
        )
        .await;

    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C1")
        .await;
    let actual = record_pairs(&body);
    let expected: Vec<(String, Value)> = vec![
        ("customer_id".to_owned(), json!("C1")),
        ("event_time".to_owned(), json!("2024-01-02T00:00:00Z")),
        ("city".to_owned(), json!("Boston")),
    ];
    assert_eq!(status, 200, "expected GetRecord 200, got {status}");
    assert_eq!(
        actual, expected,
        "case \"over-write\": expected only the second record's values {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn put_record_of_a_historic_record_leaves_the_online_record_standing() {
    let emulator = Emulator::new();

    // "If a new record's EventTime is greater, the new record is written to both the
    // OnlineStore and OfflineStore. Otherwise, the record is a historic record and it
    // is written only to the OfflineStore." (PutRecord, Smithy model doc.)
    // Rows: an older, an equal and a later event time, in both accepted event-time
    // formats, plus a write aimed at the OfflineStore alone.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        ("older", "2023-12-31T00:00:00Z", Value::Null, "Seattle"),
        ("equal", "2024-01-01T00:00:00Z", Value::Null, "Seattle"),
        ("later", "2024-01-02T00:00:00Z", Value::Null, "Boston"),
        ("older epoch seconds", "1700000000", Value::Null, "Seattle"),
        ("later epoch seconds", "1800000000", Value::Null, "Boston"),
        (
            "later but offline only",
            "2030-01-01T00:00:00Z",
            json!(["OfflineStore"]),
            "Seattle",
        ),
    ];

    for (case, event_time, target_stores, expected_city) in cases {
        let group = "customers";
        emulator
            .put_record(
                group,
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;

        let mut request = json!({ "Record": [
            feature("customer_id", "C1"),
            feature("event_time", event_time),
            feature("city", "Boston")
        ] });
        if !target_stores.is_null() {
            request["TargetStores"] = target_stores;
        }
        let (status, _) = emulator.put_record(group, request).await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");

        let (_, body) = emulator
            .get_record(group, "RecordIdentifierValueAsString=C1")
            .await;
        let actual = record_pairs(&body);
        let city = actual
            .iter()
            .find(|(name, _)| name == "city")
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            city,
            json!(expected_city),
            "case {case:?}: expected city {expected_city:?}, got {city:?} (record {actual:?})"
        );

        // Reset the group for the next row.
        emulator
            .delete_record(
                group,
                "RecordIdentifierValueAsString=C1&EventTime=2035-01-01T00:00:00Z&DeletionMode=HardDelete",
            )
            .await;
    }
}

#[tokio::test]
async fn get_record_projects_exactly_the_feature_names_the_caller_asked_for() {
    let emulator = Emulator::new();
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle"),
                feature("plan", "gold")
            ] }),
        )
        .await;

    // "List of names of Features to be retrieved. If not specified, the latest value
    // for all the Features are returned." (GetRecord$FeatureNames, Smithy model doc.)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("no projection", "", vec!["customer_id", "event_time", "city", "plan"]),
        ("one feature", "&FeatureName=city", vec!["city"]),
        (
            "two features, stored order kept",
            "&FeatureName=plan&FeatureName=customer_id",
            vec!["customer_id", "plan"],
        ),
        (
            "every feature named",
            "&FeatureName=customer_id&FeatureName=event_time&FeatureName=city&FeatureName=plan",
            vec!["customer_id", "event_time", "city", "plan"],
        ),
        ("a feature the record has not", "&FeatureName=nosuch", vec![]),
    ];

    for (case, query, expected) in cases {
        let (status, body) = emulator
            .get_record(
                "customers",
                &format!("RecordIdentifierValueAsString=C1{query}"),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual: Vec<String> = record_pairs(&body)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected features {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn ttl_duration_sets_expires_at_to_event_time_plus_the_duration() {
    let emulator = Emulator::new();

    // Every unit the model's `TtlDurationUnit` enum declares (see
    // `TTL_DURATION_UNIT_VALUES` in `ops::types::shared_types`), with the published
    // lower bound of `TtlDurationValue` (range 1..) and a larger value, over both
    // accepted event-time formats. The event time is a day ahead, so no row's expiry
    // is already due.
    let iso = tomorrow_iso();
    let epoch = tomorrow_epoch();
    let cases: Vec<(&str, &str, &str, i64, f64)> = vec![
        ("Seconds, lower bound", &iso, "Seconds", 1, 1.0),
        ("Seconds", &iso, "Seconds", 90, 90.0),
        ("Minutes", &iso, "Minutes", 90, 5_400.0),
        ("Hours", &iso, "Hours", 36, 129_600.0),
        ("Days", &iso, "Days", 1, 86_400.0),
        ("Weeks", &iso, "Weeks", 2, 1_209_600.0),
        ("Days on an epoch event time", &epoch, "Days", 3, 259_200.0),
        ("Weeks, large value", &iso, "Weeks", 52, 31_449_600.0),
    ];

    for (case, event_time, unit, value, expected_delta) in cases {
        let identifier = format!("C-{unit}-{value}");
        let (status, _) = emulator
            .put_record(
                "ttlgroup",
                json!({
                    "Record": [
                        feature("customer_id", &identifier),
                        feature("event_time", event_time),
                        feature("city", "Seattle")
                    ],
                    "TtlDuration": { "Unit": unit, "Value": value }
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");

        let (status, body) = emulator
            .get_record(
                "ttlgroup",
                &format!(
                    "RecordIdentifierValueAsString={identifier}&ExpirationTimeResponse=Enabled"
                ),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let expires_at = body
            .get("ExpiresAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            DateTime::from_str(&expires_at, Format::DateTime).is_ok(),
            "case {case:?}: expected an ISO-8601 ExpiresAt, got {expires_at:?}"
        );
        let delta = secs(&expires_at) - secs(event_time);
        assert!(
            (delta - expected_delta).abs() < 0.001,
            "case {case:?}: expected ExpiresAt {expected_delta} s after the event time, \
             got {delta} s (ExpiresAt {expires_at:?})"
        );

        // "If Enabled, GetRecord will return the value of ExpiresAt": a request that
        // does not enable it gets no ExpiresAt.
        for (sub_case, query) in [
            ("absent", String::new()),
            ("Disabled", "&ExpirationTimeResponse=Disabled".to_owned()),
        ] {
            let (_, body) = emulator
                .get_record(
                    "ttlgroup",
                    &format!("RecordIdentifierValueAsString={identifier}{query}"),
                )
                .await;
            let actual = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
            assert!(
                actual.is_null(),
                "case {case:?}/{sub_case:?}: expected no ExpiresAt, got {actual:?}"
            );
        }
    }

    // A record written with no TtlDuration has no ExpiresAt at all.
    emulator
        .put_record(
            "ttlgroup",
            json!({ "Record": [
                feature("customer_id", "C-no-ttl"),
                feature("event_time", &iso)
            ] }),
        )
        .await;
    let (_, body) = emulator
        .get_record(
            "ttlgroup",
            "RecordIdentifierValueAsString=C-no-ttl&ExpirationTimeResponse=Enabled",
        )
        .await;
    let actual = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"no TtlDuration\": expected no ExpiresAt, got {actual:?}"
    );
}

#[tokio::test]
async fn a_record_whose_ttl_has_passed_is_gone_from_the_online_store() {
    let emulator = Emulator::new();

    // An event time far in the past plus a one-second TTL: the expiry is already due,
    // so the record is hard-deleted by the time anything reads the group. The second
    // record's expiry is a year out, so it stays.
    emulator
        .put_record(
            "expiring",
            json!({
                "Record": [
                    feature("customer_id", "C-expired"),
                    feature("event_time", "2000-01-01T00:00:00Z")
                ],
                "TtlDuration": { "Unit": "Seconds", "Value": 1 }
            }),
        )
        .await;
    emulator
        .put_record(
            "expiring",
            json!({
                "Record": [
                    feature("customer_id", "C-live"),
                    feature("event_time", &tomorrow_iso())
                ],
                "TtlDuration": { "Unit": "Weeks", "Value": 52 }
            }),
        )
        .await;

    let (status, body) = emulator
        .get_record("expiring", "RecordIdentifierValueAsString=C-expired")
        .await;
    assert_eq!(status, 200, "expected GetRecord 200, got {status}");
    let actual = body.get("Record").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"expired record\": expected no Record, got {actual:?}"
    );

    let (_, body) = emulator.list_records("expiring", json!({})).await;
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!(["C-live"]),
        "case \"expired record\": expected only the live identifier, got {listed:?}"
    );
}

#[tokio::test]
async fn list_records_pages_through_the_identifiers_in_state() {
    let emulator = Emulator::new();
    let written = ["C1", "C2", "C3", "C4", "C5"];
    for identifier in written {
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", identifier),
                    feature("event_time", "2024-01-01T00:00:00Z")
                ] }),
            )
            .await;
    }

    // The rule spans every page size the model allows: each page holds at most
    // MaxResults identifiers, NextToken resumes exactly where the page stopped, and
    // the pages together are every identifier in state, once.
    for max_results in [Value::Null, json!(1), json!(2), json!(3), json!(5), json!(100)] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if !max_results.is_null() {
                request["MaxResults"] = max_results.clone();
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.list_records("customers", request).await;
            assert_eq!(status, 200, "case {case:?}: expected ListRecords 200, got {status}");
            let page: Vec<String> = body["RecordIdentifiers"]
                .as_array()
                .expect("RecordIdentifiers is required")
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect();
            if let Some(max) = max_results.as_u64() {
                assert!(
                    page.len() as u64 <= max,
                    "case {case:?}: expected at most {max} identifiers in a page, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(pages <= 10, "case {case:?}: pagination did not terminate");
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if next_token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted,
            written.map(str::to_owned).to_vec(),
            "case {case:?}: expected every identifier exactly once, got {seen:?}"
        );
    }

    // A token this operation never handed out is a client error, not a page.
    let (status, body) = emulator
        .list_records("customers", json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case \"bogus NextToken\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"bogus NextToken\": expected the modelled ValidationError error code, \
         got {code:?}"
    );
}

/// One `ListRecords` page: the identifiers it carried, and the `NextToken` it emitted.
async fn list_one_page(
    emulator: &Emulator,
    group: &str,
    max_results: Option<u64>,
    token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let mut request = json!({});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = emulator.list_records(group, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRecords 200, got {status} with {body}"
    );
    let page: Vec<String> = body["RecordIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: RecordIdentifiers is required, got {body}"))
        .iter()
        .map(|identifier| identifier.as_str().unwrap_or_default().to_owned())
        .collect();
    if let Some(max) = max_results {
        assert!(
            page.len() as u64 <= max,
            "case {case:?}: expected at most {max} identifiers in a page, got {page:?}"
        );
    }
    let next = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_owned);
    (page, next)
}

/// Every identifier a listing yields from `token` on, following each `NextToken` to the
/// end: a page that hands back a token is never empty, and the pages strictly ascend.
async fn page_through(
    emulator: &Emulator,
    group: &str,
    max_results: Option<u64>,
    token: Option<String>,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token = token;
    // A bound on the number of requests, not on time: every page returns at least one
    // identifier, so a terminating listing of these rows is far inside it.
    for _ in 0..64 {
        let (page, next) = list_one_page(emulator, group, max_results, token.as_deref(), case).await;
        if next.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: a page that hands back a NextToken must not be empty"
            );
        }
        if let (Some(last), Some(first)) = (seen.last(), page.first()) {
            assert!(
                last < first,
                "case {case:?}: pages must advance in ascending identifier order, got \
                 {last:?} then {first:?}"
            );
        }
        seen.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return seen,
        }
    }
    panic!("case {case:?}: pagination did not terminate");
}

#[tokio::test]
async fn list_records_resumes_the_ordered_store_at_the_identifier_its_cursor_names() {
    // Each row differs along what the rule generalises over: how the identifiers sort
    // against the order they were WRITTEN in, their form, their size, and how many there
    // are. The identifier is modelled `RecordIdentifierValueAsString` (ValueAsString, len
    // 0..358400), so a row of 1 000-character identifiers is well inside it.
    let long = |suffix: &str| format!("{}{suffix}", "x".repeat(997));
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one record", vec!["only".to_owned()]),
        (
            "written in descending order",
            ["C5", "C4", "C3", "C2", "C1"]
                .map(str::to_owned)
                .to_vec(),
        ),
        (
            "ordered lexicographically, not numerically",
            ["C9", "C10", "C1", "C100", "C2"]
                .map(str::to_owned)
                .to_vec(),
        ),
        (
            "mixed case and punctuation",
            ["b", "A", "a", "B-2", "B_1"].map(str::to_owned).to_vec(),
        ),
        ("non-ascii", ["\u{e9}", "e", "Z"].map(str::to_owned).to_vec()),
        (
            "long identifiers",
            vec![long("003"), long("001"), long("002")],
        ),
    ];

    for (index, (case, written)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let group = format!("paging{index}");
        emulator.create_feature_group(&group, "customer_id", "event_time", true);
        for identifier in written {
            let (status, body) = emulator
                .put_record(
                    &group,
                    json!({ "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", "2024-01-01T00:00:00Z")
                    ] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutRecord 200, got {status} with {body}"
            );
        }
        // The one order the store enumerates in: ascending identifier.
        let mut ascending = written.clone();
        ascending.sort();

        // Paged at any size the model allows, the pages together are that order, once.
        for max_results in [Some(1), Some(2), Some(100), None] {
            let label = format!("{case} / MaxResults={max_results:?}");
            let seen = page_through(&emulator, &group, max_results, None, &label).await;
            assert_eq!(
                seen, ascending,
                "case {label:?}: expected every identifier in ascending order exactly \
                 once, got {seen:?}"
            );
        }

        // Every identifier is a resume point: the token a page of `cut` identifiers hands
        // back names the (cut + 1)-th identifier, and resuming at it returns exactly the
        // ordered suffix from that identifier on — nothing before it, nothing twice.
        for cut in 1..ascending.len() {
            let label = format!("{case} / resume after {cut}");
            let (first, token) =
                list_one_page(&emulator, &group, Some(cut as u64), None, &label).await;
            assert_eq!(
                first,
                ascending[..cut],
                "case {label:?}: expected the first {cut} identifiers, got {first:?}"
            );
            let token = token.unwrap_or_else(|| {
                panic!("case {label:?}: a truncated page must hand back a NextToken")
            });
            let rest = page_through(&emulator, &group, Some(100), Some(token), &label).await;
            assert_eq!(
                rest,
                ascending[cut..],
                "case {label:?}: expected the suffix {:?}, got {rest:?}",
                &ascending[cut..]
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_names_an_identifier_so_it_resumes_from_wherever_that_identifier_sorts() {
    // Rows: what has happened to the record the cursor names by the time the next page is
    // asked for. A cursor names an IDENTIFIER, not a position, so the resumed listing is
    // the ordered suffix at or after it: the record itself when it is still listable, and
    // otherwise the records above it.
    let written = ["A", "B", "C", "D"];
    let cases: Vec<(&str, &str, &str, bool, Vec<&str>)> = vec![
        (
            "the cursor's own record hard-deleted mid-order",
            "B",
            "HardDelete",
            false,
            vec!["C", "D"],
        ),
        (
            "the cursor's own record hard-deleted at the end of the order",
            "D",
            "HardDelete",
            false,
            vec![],
        ),
        (
            "the cursor's own record soft-deleted, not asked for",
            "B",
            "SoftDelete",
            false,
            vec!["C", "D"],
        ),
        (
            "the cursor's own record soft-deleted and asked for",
            "B",
            "SoftDelete",
            true,
            vec!["B", "C", "D"],
        ),
    ];

    for (index, (case, cursor_at, mode, include_soft_deleted, expected)) in
        cases.iter().enumerate()
    {
        let emulator = Emulator::new();
        let group = format!("cursorgap{index}");
        emulator.create_feature_group(&group, "customer_id", "event_time", true);
        for identifier in written {
            emulator
                .put_record(
                    &group,
                    json!({ "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", "2024-01-01T00:00:00Z")
                    ] }),
                )
                .await;
        }

        // The page that stops just before the cursor's record hands back the token that
        // names it.
        let cut = written
            .iter()
            .position(|identifier| identifier == cursor_at)
            .expect("the cursor's identifier is one of the written ones");
        let (page, token) = list_one_page(&emulator, &group, Some(cut as u64), None, case).await;
        assert_eq!(
            page,
            written[..cut],
            "case {case:?}: expected the identifiers before the cursor, got {page:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a truncated page must hand back a NextToken")
        });

        let (status, _) = emulator
            .delete_record(
                &group,
                &format!(
                    "RecordIdentifierValueAsString={cursor_at}\
                     &EventTime=2024-01-02T00:00:00Z&DeletionMode={mode}"
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRecord 200, got {status}"
        );

        let (status, body) = emulator
            .list_records(
                &group,
                json!({
                    "NextToken": token,
                    "MaxResults": 100,
                    "IncludeSoftDeletedRecords": include_soft_deleted
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRecords 200, got {status} with {body}"
        );
        let resumed: Vec<String> = body["RecordIdentifiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: RecordIdentifiers is required, got {body}"))
            .iter()
            .map(|identifier| identifier.as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            resumed, *expected,
            "case {case:?}: expected the suffix {expected:?}, got {resumed:?}"
        );
        let trailing = body.get("NextToken").cloned().unwrap_or(Value::Null);
        assert!(
            trailing.is_null(),
            "case {case:?}: the last page must hand back no NextToken, got {trailing:?}"
        );
    }
}

#[tokio::test]
async fn delete_record_removes_the_record_from_the_online_store() {
    // Rows: the two deletion modes, explicit and defaulted, plus a delete aimed at the
    // OfflineStore alone. `SoftDelete` keeps a tombstone `ListRecords` can still show;
    // `HardDelete` removes the record entirely.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("default (SoftDelete)", "", true),
        ("explicit SoftDelete", "&DeletionMode=SoftDelete", true),
        ("HardDelete", "&DeletionMode=HardDelete", false),
    ];

    for (case, mode, soft_deleted_listable) in cases {
        let emulator = Emulator::new();
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C2"),
                    feature("event_time", "2024-01-01T00:00:00Z")
                ] }),
            )
            .await;

        let (status, _) = emulator
            .delete_record(
                "customers",
                &format!(
                    "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z{mode}"
                ),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected DeleteRecord 200, got {status}");

        let (status, body) = emulator
            .get_record("customers", "RecordIdentifierValueAsString=C1")
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = body.get("Record").cloned().unwrap_or(Value::Null);
        assert!(
            actual.is_null(),
            "case {case:?}: expected no Record after the delete, got {actual:?}"
        );

        let (_, body) = emulator.list_records("customers", json!({})).await;
        let listed = body["RecordIdentifiers"].clone();
        assert_eq!(
            listed,
            json!(["C2"]),
            "case {case:?}: expected the deleted identifier gone from ListRecords, got {listed:?}"
        );

        let (_, body) = emulator
            .list_records("customers", json!({ "IncludeSoftDeletedRecords": true }))
            .await;
        let listed: Vec<String> = body["RecordIdentifiers"]
            .as_array()
            .expect("RecordIdentifiers is required")
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_owned())
            .collect();
        let expected: Vec<String> = if soft_deleted_listable {
            vec!["C1".to_owned(), "C2".to_owned()]
        } else {
            vec!["C2".to_owned()]
        };
        assert_eq!(
            listed, expected,
            "case {case:?}: expected IncludeSoftDeletedRecords to list {expected:?}, got {listed:?}"
        );

        // BatchGetRecord reads the same store, so it no longer serves the record.
        let (_, body) = emulator
            .batch_get_record(json!({ "Identifiers": [
                { "FeatureGroupName": "customers", "RecordIdentifiersValueAsString": ["C1"] }
            ] }))
            .await;
        let records = body["Records"].clone();
        assert_eq!(
            records,
            json!([]),
            "case {case:?}: expected BatchGetRecord to serve no deleted record, got {records:?}"
        );
    }
}

#[tokio::test]
async fn delete_record_with_an_event_time_before_the_records_does_not_delete() {
    // "Note that the EventTime specified in DeleteRecord should be set later than the
    // EventTime of the existing record … If it is not, the deletion does not occur:
    // For SoftDelete, the existing (not deleted) record remains in the OnlineStore …
    // HardDelete returns EventTime: 400 ValidationException." (DeleteRecord doc.) The
    // boundary the service enforces is SAME-or-after, so a delete submitted at exactly
    // the record's own EventTime — the value a client reads back from GetRecord —
    // deletes it, and only a strictly older one does not.
    let cases: Vec<(&str, &str, &str, u16, bool)> = vec![
        ("soft, older", "", "2023-12-31T00:00:00Z", 200, true),
        ("soft, equal", "&DeletionMode=SoftDelete", "2024-01-01T00:00:00Z", 200, false),
        ("hard, older", "&DeletionMode=HardDelete", "2023-12-31T00:00:00Z", 400, true),
        ("hard, equal", "&DeletionMode=HardDelete", "2024-01-01T00:00:00Z", 200, false),
        (
            "hard, one second older",
            "&DeletionMode=HardDelete",
            "2023-12-31T23:59:59Z",
            400,
            true,
        ),
        (
            "hard, one second later",
            "&DeletionMode=HardDelete",
            "2024-01-01T00:00:01Z",
            200,
            false,
        ),
        ("hard, later", "&DeletionMode=HardDelete", "2024-01-02T00:00:00Z", 200, false),
        ("soft, later", "&DeletionMode=SoftDelete", "2024-01-02T00:00:00Z", 200, false),
        // The same instant in the other accepted event-time format: 2024-01-01T00:00:00Z
        // is 1 704 067 200 epoch seconds, so this row is "equal" spelled as a Fractional.
        (
            "hard, equal as epoch seconds",
            "&DeletionMode=HardDelete",
            "1704067200",
            200,
            false,
        ),
    ];

    for (case, mode, event_time, expected_status, still_there) in cases {
        let emulator = Emulator::new();
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;

        let (status, body) = emulator
            .delete_record(
                "customers",
                &format!("RecordIdentifierValueAsString=C1&EventTime={event_time}{mode}"),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected DeleteRecord {expected_status}, got {status}"
        );
        if expected_status == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationError",
                "case {case:?}: expected the modelled ValidationError error code, got {code:?}"
            );
        }

        let (_, body) = emulator
            .get_record("customers", "RecordIdentifierValueAsString=C1")
            .await;
        let served = !body.get("Record").cloned().unwrap_or(Value::Null).is_null();
        assert_eq!(
            served, still_there,
            "case {case:?}: expected the record to be retrievable={still_there}, got {served}"
        );
    }
}

#[tokio::test]
async fn batch_get_record_fans_out_over_the_same_store_get_record_reads() {
    let emulator = Emulator::new();
    let event_time = tomorrow_iso();
    for (identifier, city) in [("C1", "Seattle"), ("C2", "Boston")] {
        emulator
            .put_record(
                "customers",
                json!({
                    "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", &event_time),
                        feature("city", city)
                    ],
                    "TtlDuration": { "Unit": "Weeks", "Value": 52 }
                }),
            )
            .await;
    }
    emulator
        .put_record(
            "orders",
            json!({ "Record": [
                feature("order_id", "O1"),
                feature("event_time", &event_time)
            ] }),
        )
        .await;

    let (status, body) = emulator
        .batch_get_record(json!({
            "Identifiers": [
                { "FeatureGroupName": "customers",
                  "RecordIdentifiersValueAsString": ["C1", "C-missing", "C2"] },
                { "FeatureGroupName": "orders",
                  "RecordIdentifiersValueAsString": ["O1"],
                  "FeatureNames": ["order_id"] },
                { "FeatureGroupName": "nosuchgroup",
                  "RecordIdentifiersValueAsString": ["X1", "X2"] }
            ],
            "ExpirationTimeResponse": "Enabled"
        }))
        .await;
    assert_eq!(status, 200, "expected BatchGetRecord 200, got {status}");

    // All three output members are `@required`, so all three are always present.
    for member in ["Records", "Errors", "UnprocessedIdentifiers"] {
        assert!(
            body.get(member).and_then(Value::as_array).is_some(),
            "case {member:?}: expected the required member to be present, got {body}"
        );
    }
    let unprocessed = body["UnprocessedIdentifiers"].clone();
    assert_eq!(
        unprocessed,
        json!([]),
        "case \"UnprocessedIdentifiers\": expected an empty list, got {unprocessed:?}"
    );

    let served: Vec<(String, String)> = body["Records"]
        .as_array()
        .expect("Records is required")
        .iter()
        .map(|detail| {
            (
                detail["FeatureGroupName"].as_str().unwrap_or_default().to_owned(),
                detail["RecordIdentifierValueAsString"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    let expected = vec![
        ("customers".to_owned(), "C1".to_owned()),
        ("customers".to_owned(), "C2".to_owned()),
        ("orders".to_owned(), "O1".to_owned()),
    ];
    assert_eq!(
        served, expected,
        "case \"Records\": expected every written identifier once {expected:?}, got {served:?}"
    );

    // The record detail carries what the store holds, projected and expiring exactly
    // as GetRecord would answer.
    let c1 = body["Records"]
        .as_array()
        .and_then(|records| {
            records
                .iter()
                .find(|r| r["RecordIdentifierValueAsString"] == json!("C1"))
        })
        .cloned()
        .expect("C1 is served");
    let c1_features = record_pairs(&json!({ "Record": c1["Record"].clone() }));
    let expected_features: Vec<(String, Value)> = vec![
        ("customer_id".to_owned(), json!("C1")),
        ("event_time".to_owned(), json!(event_time)),
        ("city".to_owned(), json!("Seattle")),
    ];
    assert_eq!(
        c1_features, expected_features,
        "case \"C1 record\": expected {expected_features:?}, got {c1_features:?}"
    );
    let expires_at = c1["ExpiresAt"].as_str().unwrap_or_default().to_owned();
    let delta = secs(&expires_at) - secs(&event_time);
    assert!(
        (delta - 31_449_600.0).abs() < 0.001,
        "case \"C1 ExpiresAt\": expected 52 weeks after the event time, got {delta} s \
         (ExpiresAt {expires_at:?})"
    );

    let o1 = body["Records"]
        .as_array()
        .and_then(|records| {
            records
                .iter()
                .find(|r| r["RecordIdentifierValueAsString"] == json!("O1"))
        })
        .cloned()
        .expect("O1 is served");
    let o1_features = record_pairs(&json!({ "Record": o1["Record"].clone() }));
    assert_eq!(
        o1_features,
        vec![("order_id".to_owned(), json!("O1"))],
        "case \"O1 projection\": expected only the named feature, got {o1_features:?}"
    );

    // Every identifier of an entry whose feature group does not resolve is reported per
    // identifier, with the error code this data plane gives an unresolvable name.
    let errors: Vec<(String, String, String)> = body["Errors"]
        .as_array()
        .expect("Errors is required")
        .iter()
        .map(|e| {
            (
                e["FeatureGroupName"].as_str().unwrap_or_default().to_owned(),
                e["RecordIdentifierValueAsString"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                e["ErrorCode"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected_errors = vec![
        (
            "nosuchgroup".to_owned(),
            "X1".to_owned(),
            "ValidationError".to_owned(),
        ),
        (
            "nosuchgroup".to_owned(),
            "X2".to_owned(),
            "ValidationError".to_owned(),
        ),
    ];
    assert_eq!(
        errors, expected_errors,
        "case \"Errors\": expected {expected_errors:?}, got {errors:?}"
    );
}

#[tokio::test]
async fn a_feature_group_arn_and_its_name_reach_one_online_store() {
    let emulator = Emulator::new();
    // The `FeatureGroupNameOrArn` pattern makes the ARN prefix optional, so both forms
    // name one feature group. On the wire the ARN is one `@httpLabel` segment, so its
    // `:` and `/` arrive percent-encoded.
    let arn = "arn:aws:sagemaker:us-east-1:123456789012:feature-group/customers";
    let arn_on_the_wire =
        "arn%3Aaws%3Asagemaker%3Aus-east-1%3A123456789012%3Afeature-group%2Fcustomers";
    emulator
        .put_record(
            arn_on_the_wire,
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle")
            ] }),
        )
        .await;

    for (case, group) in [("bare name", "customers"), ("ARN", arn_on_the_wire)] {
        let (status, body) = emulator
            .get_record(group, "RecordIdentifierValueAsString=C1")
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = record_pairs(&body);
        assert_eq!(
            actual.len(),
            3,
            "case {case:?}: expected the record written through the other form, got {actual:?}"
        );
    }

    let (_, body) = emulator.list_records(arn_on_the_wire, json!({})).await;
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!(["C1"]),
        "case \"ListRecords by ARN\" ({arn}): expected the identifier written by name, \
         got {listed:?}"
    );
}

#[tokio::test]
async fn a_feature_group_that_does_not_resolve_is_refused_and_an_absent_record_is_not() {
    let emulator = Emulator::new();

    // A `FeatureGroupName` that names no feature group is a request defect: every
    // operation of this data plane refuses it with the modelled ValidationError (400),
    // never with ResourceNotFound (404), which would say a record is missing from a
    // feature group that exists. The table that spans this rule over every operation and
    // both name spellings is `tests/all/feature_group_resolution.rs`.
    let (status, body) = emulator
        .get_record("nosuchgroup", "RecordIdentifierValueAsString=C1")
        .await;
    assert_eq!(status, 400, "case \"GetRecord\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"GetRecord\": expected the ValidationError error code, got {code:?}"
    );

    let (status, body) = emulator.list_records("nosuchgroup", json!({})).await;
    assert_eq!(status, 400, "case \"ListRecords\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"ListRecords\": expected the ValidationError error code, got {code:?}"
    );

    // An identifier that was never written to a group that DOES exist is an empty
    // result, not an error: "If no Record with RecordIdentifierValue is found, then an
    // empty result is returned."
    emulator
        .put_record(
            "customers",
            json!({ "Record": [feature("customer_id", "C1")] }),
        )
        .await;
    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C-missing")
        .await;
    assert_eq!(
        status, 200,
        "case \"unknown identifier\": expected 200, got {status}"
    );
    let actual = body.get("Record").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"unknown identifier\": expected no Record, got {actual:?}"
    );

    // Nor is a group that exists but holds no record yet: it lists nothing and serves
    // nothing, with no error at all.
    emulator.create_feature_group("emptygroup", "customer_id", "event_time", true);
    let (status, body) = emulator.list_records("emptygroup", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"a group with no record yet\": expected ListRecords 200, got {status}"
    );
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!([]),
        "case \"a group with no record yet\": expected no identifiers, got {listed:?}"
    );

    // DeleteRecord is refused for the same reason the others are: the delete marker it
    // documents belongs to a feature group that exists.
    let (status, body) = emulator
        .delete_record(
            "nosuchgroup",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-01T00:00:00Z",
        )
        .await;
    assert_eq!(status, 400, "case \"DeleteRecord\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"DeleteRecord\": expected the ValidationError error code, got {code:?}"
    );
}

#[tokio::test]
async fn a_request_outside_the_models_constraints_is_refused_with_400() {
    let emulator = Emulator::new();

    // Each row breaks one modelled constraint of the member it names.
    let too_long = "a".repeat(151); // FeatureGroupName: len 1..150

    for (case, group) in [
        ("FeatureGroupName longer than 150", too_long.as_str()),
        ("FeatureGroupName outside the pattern", "bad!name"),
    ] {
        let (status, _) = emulator
            .put_record(group, json!({ "Record": [feature("customer_id", "C1")] }))
            .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
    }

    // `Record` is `len 1..`.
    let (status, _) = emulator
        .put_record("customers", json!({ "Record": [] }))
        .await;
    assert_eq!(status, 400, "case \"empty Record\": expected 400, got {status}");

    // `Identifiers` is `len 1..100`.
    let (status, _) = emulator
        .batch_get_record(json!({ "Identifiers": [] }))
        .await;
    assert_eq!(
        status, 400,
        "case \"empty Identifiers\": expected 400, got {status}"
    );

    // `MaxResults` is `range 1..100`.
    for (case, max_results) in [("below the range", json!(0)), ("above the range", json!(101))] {
        let (status, _) = emulator
            .list_records("customers", json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults={max_results} refused with 400, got {status}"
        );
    }

    // A record that does not carry the group's record-identifier feature cannot be
    // stored, and is refused rather than silently dropped.
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z")
            ] }),
        )
        .await;
    let (status, body) = emulator
        .put_record(
            "customers",
            json!({ "Record": [feature("city", "Seattle")] }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"record without the identifier feature\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"record without the identifier feature\": expected the modelled \
         ValidationError error code, got {code:?}"
    );
}

#[tokio::test]
async fn a_value_outside_an_enum_or_a_fabricated_page_token_is_refused_with_400() {
    let emulator = Emulator::new();
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z")
            ] }),
        )
        .await;

    // Every enum-typed member of this service, with a value the model's value list
    // does not declare — a typo must not be read as "not the value I compare against"
    // and silently change what the call does. Sources: `TARGET_STORE_VALUES`,
    // `DELETION_MODE_VALUES`, `EXPIRATION_TIME_RESPONSE_VALUES` in
    // `src/ops/types/shared_types.rs`, rendered from the model's enums. The refusal is
    // a 400 whether the boundary validator catches the value (`ValidationException`)
    // or the handler does (the modelled `ValidationError`).
    let (status, body) = emulator
        .put_record(
            "customers",
            json!({
                "Record": [feature("customer_id", "C1")],
                "TargetStores": ["Online"]
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"PutRecord TargetStores typo\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert!(
        is_client_refusal(&code),
        "case \"PutRecord TargetStores typo\": expected a validation error code, got {code:?}"
    );

    for (case, query) in [
        (
            "DeleteRecord DeletionMode typo",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z&DeletionMode=Hard",
        ),
        (
            "DeleteRecord TargetStores typo",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z&TargetStores=Online",
        ),
        (
            "DeleteRecord EventTime that is no timestamp",
            "RecordIdentifierValueAsString=C1&EventTime=yesterday",
        ),
    ] {
        let (status, body) = emulator.delete_record("customers", query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
        let code = error_code(&body);
        assert!(
            is_client_refusal(&code),
            "case {case:?}: expected a validation error code, got {code:?}"
        );
    }

    let (status, body) = emulator
        .get_record(
            "customers",
            "RecordIdentifierValueAsString=C1&ExpirationTimeResponse=On",
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"GetRecord ExpirationTimeResponse typo\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert!(
        is_client_refusal(&code),
        "case \"GetRecord ExpirationTimeResponse typo\": expected a validation error code, \
         got {code:?}"
    );

    let (status, _) = emulator
        .batch_get_record(json!({
            "Identifiers": [
                { "FeatureGroupName": "customers", "RecordIdentifiersValueAsString": ["C1"] }
            ],
            "ExpirationTimeResponse": "On"
        }))
        .await;
    assert_eq!(
        status, 400,
        "case \"BatchGetRecord ExpirationTimeResponse typo\": expected 400, got {status}"
    );

    // A page token the caller fabricated is refused even when it is well-formed hex:
    // only a token this operation issued resumes a page.
    let fabricated = hex::encode("C1".as_bytes());
    for (case, token) in [
        ("not hex at all", "not-a-token".to_owned()),
        ("valid hex, never issued", fabricated),
    ] {
        let (status, body) = emulator
            .list_records("customers", json!({ "NextToken": token }))
            .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected ValidationError, got {code:?}"
        );
    }
}

#[tokio::test]
async fn the_online_store_refuses_a_write_past_its_capacity_bound() {
    use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::state::MAX_FEATURE_GROUPS;

    let emulator = Emulator::new();
    let record = json!({ "Record": [
        feature("customer_id", "C1"),
        feature("event_time", "2024-01-01T00:00:00Z")
    ] });

    // The store is process memory, so it holds a bounded number of feature groups. Each
    // one has to exist in the control plane before a record can reach it.
    for index in 0..MAX_FEATURE_GROUPS {
        emulator.create_feature_group(
            &format!("group{index}"),
            "customer_id",
            "event_time",
            true,
        );
    }
    emulator.create_feature_group("onegrouptoomany", "customer_id", "event_time", true);
    for index in 0..MAX_FEATURE_GROUPS {
        let (status, _) = emulator
            .put_record(&format!("group{index}"), record.clone())
            .await;
        assert_eq!(
            status, 200,
            "case \"group {index} of {MAX_FEATURE_GROUPS}\": expected PutRecord 200, got {status}"
        );
    }

    // One past the bound is refused as a server-side capacity limit (503), not as the
    // caller's mistake (400) — the request is well-formed.
    let (status, body) = emulator
        .put_record("onegrouptoomany", record.clone())
        .await;
    assert_eq!(
        status, 503,
        "case \"one group past the bound\": expected 503, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ServiceUnavailable",
        "case \"one group past the bound\": expected ServiceUnavailable, got {code:?}"
    );

    // A group already in the store still takes writes.
    let (status, _) = emulator.put_record("group0", record).await;
    assert_eq!(
        status, 200,
        "case \"an existing group at the bound\": expected PutRecord 200, got {status}"
    );
}

#[tokio::test]
async fn every_operation_of_this_slice_answers_from_the_store() {
    let emulator = Emulator::new();

    // Every operation of this data plane is implemented, so none of them answers the
    // stub's 501 any more — `BatchWriteRecord` last (see
    // `tests/all/batch_write_record_bulk_ingestion.rs` for its own rules).
    let (status, body) = emulator
        .send(
            "POST",
            "/BatchWriteRecord",
            Some(json!({ "Entries": [
                { "FeatureGroupName": "customers",
                  "Record": [feature("customer_id", "C1")] }
            ] })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (200, ""),
        "case \"BatchWriteRecord\": expected HTTP 200, got {status} with body {body:?}"
    );
    assert_eq!(
        body["Errors"],
        json!([]),
        "case \"BatchWriteRecord\": expected no per-entry error, got {body:?}"
    );

    // And what it wrote reads back through `GetRecord`.
    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C1")
        .await;
    assert_eq!(
        (status, record_pairs(&body)),
        (200, vec![("customer_id".to_owned(), json!("C1"))]),
        "case \"GetRecord after BatchWriteRecord\": expected the written record, got {status} \
         with body {body:?}"
    );
}

/// The modelled maximum length of a `ListRecords` page token: `ListRecordsNextToken` is
/// `@length 1..8192` on the response member a token is written to and on the request
/// member it comes back on alike
/// (`smithy_query(query="shape", shape="ListRecordsNextToken")`).
const MAX_NEXT_TOKEN_LENGTH: usize = 8192;

/// The modelled maximum length of a record identifier: a `RecordIdentifierValueAsString`
/// targets `ValueAsString`, `@length 0..358400`
/// (`smithy_query(query="shape", shape="ValueAsString")`).
const MAX_IDENTIFIER_LENGTH: usize = 358_400;

/// `count` distinct record identifiers of exactly `length` characters that share every
/// character but their last three — so identifiers past the length a token has room for
/// share the whole leading part of themselves with each other.
fn identifiers_sharing_a_prefix(length: usize, count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("{}{index:03}", "c".repeat(length - 3)))
        .collect()
}

/// `count` distinct record identifiers of exactly `length` characters that differ in their
/// FIRST three characters, so no two of them share a leading part at all.
fn identifiers_with_distinct_prefixes(length: usize, count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("{index:03}{}", "c".repeat(length - 3)))
        .collect()
}

/// Page through a whole listing one identifier at a time, asserting of EVERY token it
/// hands back that it is inside the modelled `ListRecordsNextToken` length — and, because
/// [`list_one_page`] requires HTTP 200, that the service itself accepts the token it just
/// minted — and return the identifiers the pages carried.
async fn page_through_asserting_bounded_tokens(
    emulator: &Emulator,
    group: &str,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A bound on the number of requests, not on time: each page carries one identifier.
    for _ in 0..64 {
        let (page, next) = list_one_page(emulator, group, Some(1), token.as_deref(), case).await;
        seen.extend(page);
        let Some(next) = next else { return seen };
        assert!(
            (1..=MAX_NEXT_TOKEN_LENGTH).contains(&next.len()),
            "case {case:?}: expected a NextToken inside the modelled length \
             1..{MAX_NEXT_TOKEN_LENGTH}, got one of {} characters",
            next.len()
        );
        token = Some(next);
    }
    panic!("case {case:?}: pagination did not terminate");
}

#[tokio::test]
async fn every_page_token_is_inside_the_modelled_bound_whatever_identifier_it_resumes_at() {
    // A token this operation hands back comes STRAIGHT BACK as the next request's
    // `NextToken`, where the same modelled maximum is enforced before any handler runs, so
    // a token outside it dead-ends the listing. The rows differ along what the rule
    // generalises over: the SIZE of the identifier the cursor resumes at — from the
    // shortest to the longest the model admits, across the length at which a token
    // carrying its identifier whole crosses the token's own maximum (a token is hex, so
    // half of it) — and whether the identifiers share their leading characters or differ
    // in them.
    let half_the_token_maximum = MAX_NEXT_TOKEN_LENGTH / 2;
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("three characters", identifiers_sharing_a_prefix(3, 3)),
        ("sixty-four characters", identifiers_sharing_a_prefix(64, 3)),
        (
            "just inside half the token maximum",
            identifiers_sharing_a_prefix(half_the_token_maximum - 64, 3),
        ),
        (
            "half the token maximum",
            identifiers_sharing_a_prefix(half_the_token_maximum, 3),
        ),
        (
            "one character past half the token maximum",
            identifiers_sharing_a_prefix(half_the_token_maximum + 1, 3),
        ),
        (
            "past the whole token maximum",
            identifiers_sharing_a_prefix(MAX_NEXT_TOKEN_LENGTH + 1, 3),
        ),
        (
            "the modelled identifier maximum",
            identifiers_sharing_a_prefix(MAX_IDENTIFIER_LENGTH, 3),
        ),
        (
            "long, differing in their leading characters",
            identifiers_with_distinct_prefixes(5_000, 3),
        ),
    ];

    for (index, (case, written)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let group = format!("tokenbound{index}");
        emulator.create_feature_group(&group, "customer_id", "event_time", true);
        for identifier in written {
            let (status, body) = emulator
                .put_record(
                    &group,
                    json!({ "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", "2024-01-01T00:00:00Z")
                    ] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutRecord 200 for a {}-character identifier, got \
                 {status} with {body}",
                identifier.len()
            );
        }
        let mut ascending = written.clone();
        ascending.sort();

        let seen = page_through_asserting_bounded_tokens(&emulator, &group, case).await;
        assert_eq!(
            seen.len(),
            ascending.len(),
            "case {case:?}: expected {} identifiers across the pages, got {}",
            ascending.len(),
            seen.len()
        );
        assert!(
            seen == ascending,
            "case {case:?}: expected every identifier in ascending order exactly once, got \
             {} identifiers whose lengths are {:?}",
            seen.len(),
            seen.iter().map(String::len).collect::<Vec<usize>>()
        );
    }
}

#[tokio::test]
async fn a_cursor_at_an_identifier_too_long_for_a_token_still_names_its_record() {
    // Identifiers that share every character a token has room for, so the cursor cannot
    // carry the identifier it resumes at and names the record another way. The rows differ
    // along what has happened to the store by the time the next page is asked for: a
    // cursor names a RECORD, not a position, so removing a record that sorts BEFORE the
    // cursor must not move the resume point, and removing the cursor's own record resumes
    // at the records above it.
    let written = identifiers_sharing_a_prefix(5_000, 4);
    let cases: Vec<(&str, Option<usize>, Vec<usize>)> = vec![
        ("nothing deleted", None, vec![1, 2, 3]),
        (
            "the record before the cursor hard-deleted",
            Some(0),
            vec![1, 2, 3],
        ),
        (
            "the cursor's own record hard-deleted",
            Some(1),
            vec![2, 3],
        ),
        (
            "the record after the cursor hard-deleted",
            Some(2),
            vec![1, 3],
        ),
    ];

    for (index, (case, deleted, expected)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let group = format!("longcursor{index}");
        emulator.create_feature_group(&group, "customer_id", "event_time", true);
        for identifier in &written {
            let (status, body) = emulator
                .put_record(
                    &group,
                    json!({ "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", "2024-01-01T00:00:00Z")
                    ] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutRecord 200, got {status} with {body}"
            );
        }

        let (page, token) = list_one_page(&emulator, &group, Some(1), None, case).await;
        assert!(
            page == written[..1],
            "case {case:?}: expected the first identifier alone, got {} of them",
            page.len()
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a truncated page must hand back a NextToken")
        });

        if let Some(deleted) = *deleted {
            let (status, body) = emulator
                .delete_record(
                    &group,
                    &format!(
                        "RecordIdentifierValueAsString={}&EventTime=2024-01-02T00:00:00Z\
                         &DeletionMode=HardDelete",
                        written[deleted]
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteRecord 200, got {status} with {body}"
            );
        }

        let rest = page_through(&emulator, &group, Some(1), Some(token), case).await;
        let expected: Vec<String> = expected.iter().map(|at| written[*at].clone()).collect();
        assert!(
            rest == expected,
            "case {case:?}: expected the {} identifiers ending {:?}, got {} ending {:?}",
            expected.len(),
            expected
                .iter()
                .map(|identifier| identifier[identifier.len() - 3..].to_owned())
                .collect::<Vec<String>>(),
            rest.len(),
            rest.iter()
                .map(|identifier| identifier[identifier.len() - 3..].to_owned())
                .collect::<Vec<String>>()
        );
    }
}

/// `Record` and `ExpiresAt` are INDEPENDENT members of `GetRecordResponse`
/// (`smithy_query(query="shape", shape="GetRecordResponse")`: neither is required, and
/// only `Record` is the `len 1..` list), and `FeatureNames` is "List of names of
/// Features to be retrieved" — the feature VALUES. So a record the identifier resolves
/// to answers with its stored `ExpiresAt` whenever the caller enabled it and the stored
/// value is not null, whatever the projection selected — down to no feature value at all.
#[tokio::test]
async fn get_record_answers_expires_at_for_every_projection_of_a_record_that_exists() {
    let emulator = Emulator::new();
    let event_time = tomorrow_iso();

    // One record under a TTL, so its stored `ExpiresAt` is not null, and one under no
    // TTL at all, whose `ExpiresAt` is null: "If Enabled, GetRecord will return the
    // value of ExpiresAt, if it is not null."
    let (status, body) = emulator
        .put_record(
            "ttlgroup",
            json!({
                "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", &event_time),
                    feature("city", "Seattle"),
                    feature("plan", "gold")
                ],
                "TtlDuration": { "Unit": "Weeks", "Value": 1 }
            }),
        )
        .await;
    assert_eq!(status, 200, "expected PutRecord 200, got {status} with {body}");
    let (status, body) = emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", &event_time),
                feature("city", "Seattle"),
                feature("plan", "gold")
            ] }),
        )
        .await;
    assert_eq!(status, 200, "expected PutRecord 200, got {status} with {body}");

    // The class the rule spans: every shape a projection of ONE stored record can take
    // — all of its features, every feature named explicitly, one, a stored name mixed
    // with one the record does not carry, and names that match nothing. Each row names
    // the feature values it expects back; the expiry is expected on every row.
    let projections: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "no projection",
            "",
            vec!["customer_id", "event_time", "city", "plan"],
        ),
        ("one stored feature", "&FeatureName=city", vec!["city"]),
        (
            "every stored feature named",
            "&FeatureName=customer_id&FeatureName=event_time&FeatureName=city&FeatureName=plan",
            vec!["customer_id", "event_time", "city", "plan"],
        ),
        (
            "a stored feature beside one the record has not",
            "&FeatureName=nosuch&FeatureName=plan",
            vec!["plan"],
        ),
        ("one name the record has not", "&FeatureName=nosuch", vec![]),
        (
            "several names, none of them the record's",
            "&FeatureName=nosuch&FeatureName=neither&FeatureName=alsonot",
            vec![],
        ),
    ];

    // `TtlDuration` {Weeks, 1} on the written record, in seconds.
    let expected_delta = 7.0 * 86_400.0;
    for (case, projection, expected_features) in &projections {
        let (status, body) = emulator
            .get_record(
                "ttlgroup",
                &format!(
                    "RecordIdentifierValueAsString=C1&ExpirationTimeResponse=Enabled{projection}"
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} with {body}"
        );
        let actual_features: Vec<String> = record_pairs(&body)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            actual_features, *expected_features,
            "case {case:?}: expected features {expected_features:?}, got {actual_features:?}"
        );
        // `Record` is modelled `len 1..`, so a projection that matched nothing carries
        // no `Record` member at all rather than an empty list.
        let record = body.get("Record").cloned().unwrap_or(Value::Null);
        assert_eq!(
            !record.is_null(),
            !expected_features.is_empty(),
            "case {case:?}: expected Record present={}, got {record:?}",
            !expected_features.is_empty()
        );
        let expires_at = body
            .get("ExpiresAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            DateTime::from_str(&expires_at, Format::DateTime).is_ok(),
            "case {case:?}: expected the record's ISO-8601 ExpiresAt, got {expires_at:?} \
             (body {body})"
        );
        let delta = secs(&expires_at) - secs(&event_time);
        assert!(
            (delta - expected_delta).abs() < 0.001,
            "case {case:?}: expected ExpiresAt {expected_delta} s after the event time, \
             got {delta} s (ExpiresAt {expires_at:?})"
        );

        // The expiry is withheld for the two reasons the member's own doc gives, and
        // the projection changes neither: the caller did not enable it, or the stored
        // value is null (the record written under no TTL, in `customers`).
        for (sub_case, group, expiration) in [
            ("not enabled", "ttlgroup", ""),
            ("Disabled", "ttlgroup", "&ExpirationTimeResponse=Disabled"),
            ("null expiry", "customers", "&ExpirationTimeResponse=Enabled"),
        ] {
            let (status, body) = emulator
                .get_record(
                    group,
                    &format!(
                        "RecordIdentifierValueAsString=C1{expiration}{projection}"
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{sub_case:?}: expected GetRecord 200, got {status} with {body}"
            );
            let actual = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
            assert!(
                actual.is_null(),
                "case {case:?}/{sub_case:?}: expected no ExpiresAt, got {actual:?}"
            );
            let actual_features: Vec<String> = record_pairs(&body)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            assert_eq!(
                actual_features, *expected_features,
                "case {case:?}/{sub_case:?}: expected features {expected_features:?}, \
                 got {actual_features:?}"
            );
        }
    }

    // "If no Record with RecordIdentifierValue is found, then an empty result is
    // returned": no record, so there is no expiry of one to report either — the one
    // path that clears BOTH members.
    for (case, projection) in [
        ("an identifier never written", ""),
        (
            "an identifier never written, with a projection",
            "&FeatureName=city",
        ),
        (
            "an identifier never written, projecting nothing it could carry",
            "&FeatureName=nosuch",
        ),
    ] {
        let (status, body) = emulator
            .get_record(
                "ttlgroup",
                &format!(
                    "RecordIdentifierValueAsString=absent&ExpirationTimeResponse=Enabled{projection}"
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} with {body}"
        );
        let record = body.get("Record").cloned().unwrap_or(Value::Null);
        let expires_at = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
        assert!(
            record.is_null() && expires_at.is_null(),
            "case {case:?}: expected an empty result, got Record {record:?} and \
             ExpiresAt {expires_at:?}"
        );
    }
}

/// One row of the tombstone-feature table: the two names a feature group fixes, the record
/// one `PutRecord` writes into it, and the `EventTime` the `SoftDelete` states.
struct TombstoneCase {
    case: &'static str,
    /// The feature group the row writes to, created with the two names below.
    group: &'static str,
    identifier_feature: &'static str,
    event_time_feature: &'static str,
    /// The record as posted, in order: one feature name and its scalar value each. The
    /// record's identifier is the value posted for `identifier_feature`.
    features: &'static [(&'static str, &'static str)],
    /// A collection-typed feature the record also carries, when it carries one.
    list_feature: Option<(&'static str, &'static [&'static str])>,
    /// The query the deletion adds for `DeletionMode`: empty is the defaulted
    /// `SoftDelete`.
    mode: &'static str,
    /// The `EventTime` the deletion states — same as, or after, the record's, so the
    /// deletion occurs.
    delete_event_time: &'static str,
}

#[tokio::test]
async fn a_soft_delete_keeps_the_record_identifier_features_value_and_times_only_a_distinct_event_time_feature(
) {
    // The class the rule spans: which of a feature group's two fixed feature names the
    // tombstone's values come from. The rows differ in the two names and whether they are
    // ONE name, in the event-time format (ISO-8601 and epoch seconds), in the order the
    // record posts its features, in how many other features there are to drop (including a
    // collection-typed one), and in whether the deletion states its mode.
    let cases: Vec<TombstoneCase> = vec![
        TombstoneCase {
            case: "distinct names, a feature to drop",
            group: "tombdistinct",
            identifier_feature: "customer_id",
            event_time_feature: "event_time",
            features: &[
                ("customer_id", "C1"),
                ("event_time", "2024-01-01T00:00:00Z"),
                ("city", "Seattle"),
            ],
            list_feature: None,
            mode: "",
            delete_event_time: "2024-01-02T00:00:00Z",
        },
        TombstoneCase {
            case: "distinct names, event time posted first, epoch seconds",
            group: "tombordered",
            identifier_feature: "recordid",
            event_time_feature: "ts",
            features: &[("ts", "1704067200"), ("recordid", "R-7"), ("score", "-0.5")],
            list_feature: None,
            mode: "&DeletionMode=SoftDelete",
            delete_event_time: "1704153600",
        },
        TombstoneCase {
            case: "distinct names, a collection feature to drop",
            group: "tomblist",
            identifier_feature: "customer_id",
            event_time_feature: "event_time",
            features: &[
                ("customer_id", "C2"),
                ("event_time", "2024-02-01T00:00:00Z"),
            ],
            list_feature: Some(("tags", &["a", "b", "c"])),
            mode: "",
            delete_event_time: "2024-02-02T00:00:00Z",
        },
        TombstoneCase {
            case: "distinct names, nothing to drop, deleted at the record's own event time",
            group: "tombonlytwo",
            identifier_feature: "customer_id",
            event_time_feature: "event_time",
            features: &[
                ("customer_id", "C9"),
                ("event_time", "2024-03-01T00:00:00Z"),
            ],
            list_feature: None,
            mode: "",
            delete_event_time: "2024-03-01T00:00:00Z",
        },
        TombstoneCase {
            case: "the identifier feature IS the event-time feature",
            group: "tombcollided",
            identifier_feature: "event_time",
            event_time_feature: "event_time",
            features: &[("event_time", "2024-01-01T00:00:00Z")],
            list_feature: None,
            mode: "",
            delete_event_time: "2024-01-02T00:00:00Z",
        },
        TombstoneCase {
            case: "the identifier feature IS the event-time feature, with features to drop",
            group: "tombcollidedextra",
            identifier_feature: "ts",
            event_time_feature: "ts",
            features: &[("ts", "1704067200"), ("city", "Seattle")],
            list_feature: Some(("tags", &["x"])),
            mode: "&DeletionMode=SoftDelete",
            delete_event_time: "1704153600",
        },
    ];

    for TombstoneCase {
        case,
        group,
        identifier_feature,
        event_time_feature,
        features,
        list_feature,
        mode,
        delete_event_time,
    } in &cases
    {
        let emulator = Emulator::new();
        emulator.create_feature_group(group, identifier_feature, event_time_feature, true);

        let identifier = features
            .iter()
            .find(|(name, _)| name == identifier_feature)
            .map(|(_, value)| *value)
            .expect("every row posts the group's record-identifier feature");

        let mut posted: Vec<Value> = features
            .iter()
            .map(|(name, value)| feature(name, value))
            .collect();
        if let Some((name, values)) = list_feature {
            posted.push(json!({ "FeatureName": name, "ValueAsStringList": values }));
        }
        let posted_names: Vec<String> = posted
            .iter()
            .map(|value| value["FeatureName"].as_str().unwrap_or_default().to_owned())
            .collect();

        let (status, body) = emulator
            .put_record(group, json!({ "Record": posted.clone() }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );

        // The record is served with every value it was written with, before the deletion
        // touches any of them.
        let (status, body) = emulator
            .get_record(
                group,
                &format!("RecordIdentifierValueAsString={identifier}"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} ({body})"
        );
        let served: Vec<String> = record_pairs(&body)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            served, posted_names,
            "case {case:?}: expected the written record to be served as {posted_names:?}, \
             got {served:?}"
        );

        let (status, body) = emulator
            .delete_record(
                group,
                &format!(
                    "RecordIdentifierValueAsString={identifier}&EventTime={delete_event_time}{mode}"
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRecord 200, got {status} ({body})"
        );

        let stored = emulator
            .stored_record(group, identifier)
            .unwrap_or_else(|| panic!("case {case:?}: expected the SoftDelete to leave a tombstone"));
        assert!(
            stored.soft_deleted,
            "case {case:?}: expected a soft-deleted record, got {stored:?}"
        );
        assert_eq!(
            stored.identifier, *identifier,
            "case {case:?}: expected the tombstone to stay keyed under {identifier:?}, got {:?}",
            stored.identifier
        );
        assert_eq!(
            stored.event_time.as_deref(),
            Some(*delete_event_time),
            "case {case:?}: expected the tombstone's EventTime to be the deletion's, got {:?}",
            stored.event_time
        );

        // The rule, read off the group's own two names: the record-identifier feature
        // carries the record's identifier, an event-time feature that is a DIFFERENT
        // feature carries the deletion's `EventTime`, and every other feature value is
        // dropped. Compared as a set, since the tombstone keeps the record's own order.
        let mut expected: Vec<(String, Option<String>)> = vec![(
            (*identifier_feature).to_owned(),
            Some((*identifier).to_owned()),
        )];
        if event_time_feature != identifier_feature {
            expected.push((
                (*event_time_feature).to_owned(),
                Some((*delete_event_time).to_owned()),
            ));
        }
        expected.sort();
        let mut actual: Vec<(String, Option<String>)> = stored
            .features
            .iter()
            .map(|value| (value.feature_name.clone(), value.value_as_string.clone()))
            .collect();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the tombstone to carry {expected:?}, got {actual:?}"
        );
        for value in &stored.features {
            assert!(
                value.value_as_string_list.is_none(),
                "case {case:?}: expected no collection value on the tombstone's {:?}, got {:?}",
                value.feature_name,
                value.value_as_string_list
            );
        }

        // The invariant the whole store is keyed by: a record's identifier and the value of
        // its record-identifier feature are one value.
        let in_record = stored
            .features
            .iter()
            .find(|value| value.feature_name == *identifier_feature)
            .and_then(|value| value.value_as_string.clone());
        assert_eq!(
            in_record.as_deref(),
            Some(stored.identifier.as_str()),
            "case {case:?}: expected the record-identifier feature to carry the record's \
             identifier {:?}, got {in_record:?}",
            stored.identifier
        );

        // Off the wire: the tombstone is served by no read, and it is listed — under the
        // identifier it was written with — only to a caller asking for soft-deleted records.
        let (status, body) = emulator
            .get_record(
                group,
                &format!("RecordIdentifierValueAsString={identifier}"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200 after the delete, got {status} ({body})"
        );
        let record = body.get("Record").cloned().unwrap_or(Value::Null);
        assert!(
            record.is_null(),
            "case {case:?}: expected no Record after the delete, got {record:?}"
        );
        let (_, body) = emulator
            .list_records(group, json!({ "IncludeSoftDeletedRecords": true }))
            .await;
        let listed = body["RecordIdentifiers"].clone();
        assert_eq!(
            listed,
            json!([identifier]),
            "case {case:?}: expected the tombstone listed as {identifier:?}, got {listed:?}"
        );
    }
}

