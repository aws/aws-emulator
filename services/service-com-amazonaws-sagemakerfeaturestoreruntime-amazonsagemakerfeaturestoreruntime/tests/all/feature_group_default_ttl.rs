// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TTL a record of a feature group is stored under: the
//! duration the write states when it states one, and otherwise the DEFAULT the group's
//! `OnlineStoreConfig.TtlDuration` configures — and what every later call that moves the
//! record's event time, an update or a soft delete, re-derives its `ExpiresAt` to.
//!
//! The group's default is a property of the STORED record, not a condition on a later
//! request: `UpdateRecord` requires an `EventTime` beside a `TtlDuration` the REQUEST
//! states ("If you specify a TtlDuration, you must also provide an EventTime in the
//! request. Otherwise, the operation returns a ValidationError"), and an update that states
//! no `TtlDuration` applies with the record's own event time ("If you omit EventTime, the
//! service keeps the record's existing EventTime and applies the update") whatever TTL the
//! record already stands under.
//!
//! Every case goes through the real front door — the multi-service router, the restJson1
//! protocol, the modelled constraint validation — so what is asserted is the wire
//! behaviour a client sees, not a handler's return value.

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

/// The feature whose value identifies a record in every group these suites create.
const IDENTIFIER_FEATURE: &str = "customer_id";

/// The feature carrying a record's event time in every group these suites create.
const EVENT_TIME_FEATURE: &str = "event_time";

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Create a feature group with an OnlineStore and the default TTL
    /// `default_ttl_seconds`, the way the SageMaker control plane's `CreateFeatureGroup`
    /// does: by registering its definition in the ONE shared feature-group directory this
    /// data plane resolves names against. `None` is a group configured with no
    /// `OnlineStoreConfig.TtlDuration` at all.
    fn create_feature_group(&self, name: &str, default_ttl_seconds: Option<f64>) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"),
                record_identifier_feature_name: IDENTIFIER_FEATURE.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled: true,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds,
                // A group this harness registers is one the control plane created
                // successfully.
                creation_failed: false,
            });
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
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
            .header("content-type", "application/json")
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

    async fn batch_write_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/BatchWriteRecord", Some(body)).await
    }

    async fn update_record(&self, group: &str, body: Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{group}/Record"),
            Some(body),
        )
        .await
    }

    async fn get_record(&self, group: &str, identifier: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!(
                "/FeatureGroup/{group}?RecordIdentifierValueAsString={identifier}\
                 &ExpirationTimeResponse=Enabled"
            ),
            None,
        )
        .await
    }

    async fn list_records(&self, group: &str) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{group}/ListRecords"),
            Some(json!({})),
        )
        .await
    }

    /// `ListRecords` asking for the soft-deleted records too, which is the one call that
    /// reports a tombstone a `SoftDelete` left behind.
    async fn list_records_including_soft_deleted(&self, group: &str) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{group}/ListRecords"),
            Some(json!({ "IncludeSoftDeletedRecords": true })),
        )
        .await
    }

    async fn delete_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.send("DELETE", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    /// The record `identifier` names in `group`'s online store, read through the store's
    /// OWN API ([`OnlineStore`]) rather than off the wire.
    ///
    /// A soft-deleted record is served by no operation of this data plane — `GetRecord`
    /// and `BatchGetRecord` withhold it and `ListRecords` reports only its identifier — so
    /// the TTL state a tombstone keeps is read here, from the same store the handlers
    /// write. That this IS that store is shown by every other assertion in these suites,
    /// each of which goes through the front door.
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

/// The instant an event time or an `ExpiresAt` denotes, in epoch seconds, in either
/// format a Feature Store event time takes.
fn secs(iso_or_epoch: &str) -> f64 {
    DateTime::from_str(iso_or_epoch, Format::DateTime)
        .map(|parsed| parsed.as_secs_f64())
        .unwrap_or_else(|_| iso_or_epoch.parse::<f64>().expect("a parsable event time"))
}

/// An instant one day from now, so a record written under a TTL of any size in these
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

/// The `ExpiresAt` a `GetRecord` response carries, or `None` when it carries none.
fn expires_at(body: &Value) -> Option<String> {
    body.get("ExpiresAt")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Assert that `body`'s `ExpiresAt` is `expected_delta` seconds after `event_time`, or
/// absent when `expected_delta` is `None`.
fn assert_expiry(case: &str, body: &Value, event_time: &str, expected_delta: Option<f64>) {
    let actual = expires_at(body);
    match (expected_delta, actual.as_deref()) {
        (None, actual) => assert!(
            actual.is_none(),
            "case {case:?}: expected no ExpiresAt, got {actual:?}"
        ),
        (Some(expected), Some(at)) => {
            assert!(
                DateTime::from_str(at, Format::DateTime).is_ok(),
                "case {case:?}: expected an ISO-8601 ExpiresAt, got {at:?}"
            );
            let delta = secs(at) - secs(event_time);
            assert!(
                (delta - expected).abs() < 0.001,
                "case {case:?}: expected ExpiresAt {expected} s after the event time, got \
                 {delta} s (ExpiresAt {at:?})"
            );
        }
        (Some(expected), None) => panic!(
            "case {case:?}: expected ExpiresAt {expected} s after the event time, got no \
             ExpiresAt at all"
        ),
    }
}

/// Every unit the model's `TtlDurationUnit` enum declares, with its span in seconds (see
/// `TTL_DURATION_UNIT_VALUES` in `ops::types::shared_types`, from the model's
/// `TtlDurationUnit` shape): the default a feature group can be configured with is one of
/// these durations, so the table spans them all, with the published lower bound of
/// `TtlDurationValue` (range 1..) and larger values.
///
/// `(case, unit, value, the seconds the duration is)`
const DEFAULT_TTL_ROWS: [(&str, &str, i32, f64); 7] = [
    ("Seconds, the lower bound of TtlDurationValue", "Seconds", 1, 1.0),
    ("Seconds", "Seconds", 45, 45.0),
    ("Minutes", "Minutes", 15, 900.0),
    ("Hours", "Hours", 36, 129_600.0),
    ("Days", "Days", 2, 172_800.0),
    ("Weeks", "Weeks", 4, 2_419_200.0),
    ("Weeks, a large value", "Weeks", 52, 31_449_600.0),
];

#[tokio::test]
async fn an_ingestion_with_no_request_ttl_takes_the_feature_groups_default() {
    let emulator = Emulator::new();
    let iso = tomorrow_iso();
    let epoch = tomorrow_epoch();

    for (index, (case, unit, value, expected)) in DEFAULT_TTL_ROWS.iter().enumerate() {
        // Each row needs its own feature group: the default TTL is the GROUP's.
        let group = format!("defaultttl{index}");
        // The control plane records the group's `OnlineStoreConfig.TtlDuration` in
        // seconds, which is what this row's unit and value convert to.
        emulator.create_feature_group(&group, Some(*expected));
        // Both formats a Feature Store event time takes, alternating over the rows, so
        // the default applies whichever one the record carries.
        let event_time = if index % 2 == 0 {
            iso.as_str()
        } else {
            epoch.as_str()
        };

        // PutRecord with NO TtlDuration member at all.
        let (status, body) = emulator
            .put_record(
                &group,
                json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, "C-put"),
                    feature(EVENT_TIME_FEATURE, event_time),
                    feature("city", "Seattle")
                ] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );
        let (status, body) = emulator.get_record(&group, "C-put").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} ({body})"
        );
        assert_expiry(
            &format!("{case} via PutRecord with no TtlDuration ({unit} {value})"),
            &body,
            event_time,
            Some(*expected),
        );

        // The same ingestion as one entry of a batch, with no TtlDuration on the entry
        // and none on the request.
        let (status, body) = emulator
            .batch_write_record(json!({ "Entries": [{
                "FeatureGroupName": group,
                "Record": [
                    feature(IDENTIFIER_FEATURE, "C-batch"),
                    feature(EVENT_TIME_FEATURE, event_time),
                    feature("city", "Denver")
                ]
            }] }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected BatchWriteRecord 200, got {status} ({body})"
        );
        assert_eq!(
            body["Errors"],
            json!([]),
            "case {case:?}: expected no entry errors, got {}",
            body["Errors"]
        );
        let (_, body) = emulator.get_record(&group, "C-batch").await;
        assert_expiry(
            &format!("{case} via BatchWriteRecord with no TtlDuration ({unit} {value})"),
            &body,
            event_time,
            Some(*expected),
        );
    }

    // A group configured with NO default TtlDuration leaves a record written without one
    // under no TTL at all, so it reports no ExpiresAt.
    emulator.create_feature_group("nodefaultttl", None);
    let (status, _) = emulator
        .put_record(
            "nodefaultttl",
            json!({ "Record": [
                feature(IDENTIFIER_FEATURE, "C-none"),
                feature(EVENT_TIME_FEATURE, &iso)
            ] }),
        )
        .await;
    assert_eq!(status, 200, "case \"no group default\": expected PutRecord 200, got {status}");
    let (_, body) = emulator.get_record("nodefaultttl", "C-none").await;
    assert_expiry("no group default, no request TtlDuration", &body, &iso, None);
}

#[tokio::test]
async fn a_record_level_ttl_supersedes_the_feature_groups_default() {
    let emulator = Emulator::new();
    let iso = tomorrow_iso();
    // The group's default is a week; every row below states its own duration, which must
    // be the one in force.
    const GROUP_DEFAULT_SECS: f64 = 604_800.0;
    emulator.create_feature_group("supersede", Some(GROUP_DEFAULT_SECS));

    for (index, (case, unit, value, expected)) in DEFAULT_TTL_ROWS.iter().enumerate() {
        let ttl = json!({ "Unit": unit, "Value": value });

        // PutRecord's own `TtlDuration`.
        let identifier = format!("C-put-{index}");
        let (status, body) = emulator
            .put_record(
                "supersede",
                json!({
                    "Record": [
                        feature(IDENTIFIER_FEATURE, &identifier),
                        feature(EVENT_TIME_FEATURE, &iso)
                    ],
                    "TtlDuration": ttl
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );
        let (_, body) = emulator.get_record("supersede", &identifier).await;
        assert_expiry(
            &format!("a record-level {unit} {value} over the group default, via PutRecord"),
            &body,
            &iso,
            Some(*expected),
        );

        // A batch entry's own `TtlDuration`, and the request-level one that applies to an
        // entry naming none: both are record-level durations, so both supersede the
        // group's default.
        let entry_identifier = format!("C-entry-{index}");
        let request_identifier = format!("C-request-{index}");
        let (status, body) = emulator
            .batch_write_record(json!({
                "TtlDuration": ttl,
                "Entries": [
                    {
                        "FeatureGroupName": "supersede",
                        "Record": [
                            feature(IDENTIFIER_FEATURE, &entry_identifier),
                            feature(EVENT_TIME_FEATURE, &iso)
                        ],
                        "TtlDuration": ttl
                    },
                    {
                        "FeatureGroupName": "supersede",
                        "Record": [
                            feature(IDENTIFIER_FEATURE, &request_identifier),
                            feature(EVENT_TIME_FEATURE, &iso)
                        ]
                    }
                ]
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected BatchWriteRecord 200, got {status} ({body})"
        );
        for (sub_case, identifier) in [
            ("an entry-level duration", entry_identifier),
            ("a request-level duration", request_identifier),
        ] {
            let (_, body) = emulator.get_record("supersede", &identifier).await;
            assert_expiry(
                &format!("{sub_case} of {unit} {value} over the group default"),
                &body,
                &iso,
                Some(*expected),
            );
        }
    }
}

#[tokio::test]
async fn a_record_under_the_groups_default_ttl_is_hard_deleted_once_it_elapses() {
    let emulator = Emulator::new();
    // A one-second default TTL and an event time long past: the expiry is already due by
    // the time anything reads the group, so the record is hard-deleted — no waiting on a
    // clock.
    emulator.create_feature_group("expiringdefault", Some(1.0));
    let live_event_time = tomorrow_iso();

    // (case, identifier, event time, whether the record outlives the read)
    let rows = [
        ("an expiry already due", "C-expired", "2000-01-01T00:00:00Z", false),
        ("an expiry a day out", "C-live", live_event_time.as_str(), true),
    ];
    for (case, identifier, event_time, _) in rows {
        let (status, body) = emulator
            .put_record(
                "expiringdefault",
                json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, identifier),
                    feature(EVENT_TIME_FEATURE, event_time)
                ] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );
    }

    for (case, identifier, _, expected_live) in rows {
        let (status, body) = emulator.get_record("expiringdefault", identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} ({body})"
        );
        let actual = body.get("Record").cloned().unwrap_or(Value::Null);
        assert_eq!(
            !actual.is_null(),
            expected_live,
            "case {case:?}: expected the record present={expected_live}, got {actual:?}"
        );
    }

    let (_, body) = emulator.list_records("expiringdefault").await;
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!(["C-live"]),
        "case \"the group listed\": expected only the unexpired record, got {listed}"
    );
}

#[tokio::test]
async fn an_update_re_derives_the_expiry_from_the_ttl_in_force() {
    let emulator = Emulator::new();
    let iso = tomorrow_iso();
    let later = DateTime::from_secs_f64(tomorrow() + 3_600.0)
        .fmt(Format::DateTime)
        .expect("format an ISO-8601 event time");
    const GROUP_DEFAULT_SECS: f64 = 172_800.0;
    emulator.create_feature_group("updatedefault", Some(GROUP_DEFAULT_SECS));

    // (case, the TtlDuration the update states, the expiry the updated record must carry
    //  relative to its new event time)
    let rows: [(&str, Option<Value>, f64); 3] = [
        // "If you omit this parameter, the record's existing TTL setting remains
        // unchanged" — the setting here is the group's default, under which the record
        // was ingested, so the expiry moves with the event time.
        ("an update naming no TtlDuration", None, GROUP_DEFAULT_SECS),
        (
            "an update naming its own TtlDuration",
            Some(json!({ "Unit": "Hours", "Value": 3 })),
            10_800.0,
        ),
        (
            "an update naming a TtlDuration in another unit",
            Some(json!({ "Unit": "Weeks", "Value": 1 })),
            604_800.0,
        ),
    ];

    for (index, (case, ttl, expected)) in rows.into_iter().enumerate() {
        let identifier = format!("C-update-{index}");
        // Ingested with NO request-level TtlDuration, so the record is under the group's
        // default.
        let (status, body) = emulator
            .put_record(
                "updatedefault",
                json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, &identifier),
                    feature(EVENT_TIME_FEATURE, &iso),
                    feature("city", "Seattle")
                ] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );
        let (_, body) = emulator.get_record("updatedefault", &identifier).await;
        assert_expiry(
            &format!("{case}: the ingested record"),
            &body,
            &iso,
            Some(GROUP_DEFAULT_SECS),
        );

        let mut request = json!({
            "RecordIdentifierValueAsString": identifier,
            "Features": [
                feature(EVENT_TIME_FEATURE, &later),
                feature("city", "Portland")
            ]
        });
        if let Some(ttl) = ttl {
            request["TtlDuration"] = ttl;
        }
        let (status, body) = emulator.update_record("updatedefault", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateRecord 200, got {status} ({body})"
        );

        let (_, body) = emulator.get_record("updatedefault", &identifier).await;
        assert_expiry(
            &format!("{case}: after the update"),
            &body,
            &later,
            Some(expected),
        );
    }
}

/// The features a `GetRecord` response serves, as `(name, value)` pairs, and an empty list
/// when it serves no record at all.
fn record_pairs(body: &Value) -> Vec<(String, String)> {
    body.get("Record")
        .and_then(Value::as_array)
        .map(|features| {
            features
                .iter()
                .map(|held| {
                    (
                        held["FeatureName"].as_str().unwrap_or_default().to_owned(),
                        held["ValueAsString"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value a served record carries for `name`, or `None` when it carries no such
/// feature.
fn served_value(body: &Value, name: &str) -> Option<String> {
    record_pairs(body)
        .into_iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value)
}

/// The wire error code of an error response: restJson1 carries it as the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The message an error response carries (`ValidationError$Message`, which restJson1
/// serialises under that name; the framework's own refusals use the lowercase spelling).
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// How `UpdateRecord` states a request-stated `TtlDuration` with no `EventTime` beside it:
/// fixed text, as the service words this error class.
const EVENT_TIME_REQUIRED_FOR_TTL: &str =
    "Validation Error: EventTime is required when TtlDuration is specified or when the feature \
     group has a default TTL configured. Include the EventTime feature in the Features list.";

/// One row of the omitted-`EventTime` table: the case name, the feature group's default TTL
/// in seconds, the `TtlDuration` the record was INGESTED under and the seconds that is,
/// whether the record's event time is spelled ISO-8601 (rather than epoch seconds), and the
/// `(feature, value)` pairs the update posts.
type KeptEventTimeRow<'a> = (
    &'a str,
    Option<f64>,
    Option<Value>,
    Option<f64>,
    bool,
    Vec<(&'a str, &'a str)>,
);

#[tokio::test]
async fn an_update_stating_no_ttl_duration_applies_with_the_records_own_event_time() {
    // The class the rule spans: whether a TTL is in force through the feature group's
    // DEFAULT, through the record's OWN duration, through both or through neither, crossed
    // with the two accepted event-time spellings and how many features the update posts —
    // none of which is a reason to refuse an update that states no `TtlDuration`. The
    // durations are the model's own `TtlDurationUnit` spans (see `DEFAULT_TTL_ROWS`), as
    // the seconds the control plane records them in.
    let rows: Vec<KeptEventTimeRow<'_>> = vec![
        (
            "no TTL anywhere",
            None,
            None,
            None,
            true,
            vec![("city", "Portland")],
        ),
        (
            "a group default in seconds",
            Some(45.0),
            None,
            None,
            true,
            vec![("city", "Portland")],
        ),
        (
            "a group default in minutes, an epoch event time",
            Some(900.0),
            None,
            None,
            false,
            vec![("city", "Portland")],
        ),
        (
            "a group default in hours",
            Some(129_600.0),
            None,
            None,
            true,
            vec![("city", "Portland")],
        ),
        (
            "a group default in days, two posted features",
            Some(172_800.0),
            None,
            None,
            true,
            vec![("city", "Portland"), ("tier", "gold")],
        ),
        (
            "a group default in weeks, a feature the record does not carry yet",
            Some(2_419_200.0),
            None,
            None,
            false,
            vec![("segment", "appended")],
        ),
        (
            "a record-level TTL over a group default",
            Some(3_600.0),
            Some(json!({ "Unit": "Weeks", "Value": 1 })),
            Some(604_800.0),
            true,
            vec![("city", "Portland")],
        ),
        (
            "a record-level TTL with no group default",
            None,
            Some(json!({ "Unit": "Days", "Value": 2 })),
            Some(172_800.0),
            false,
            vec![("city", "Portland")],
        ),
    ];

    for (index, (case, default_ttl, record_ttl, record_ttl_secs, iso_event_time, posted)) in
        rows.into_iter().enumerate()
    {
        // Each row needs its own feature group: the default TTL is the GROUP's.
        let emulator = Emulator::new();
        let group = format!("keptevent{index}");
        emulator.create_feature_group(&group, default_ttl);

        // The record is ingested with an event time a day out, in whichever spelling the
        // row names, so it is still live under every TTL in the table.
        let event_time = if iso_event_time {
            tomorrow_iso()
        } else {
            tomorrow_epoch()
        };
        let mut seed = json!({ "Record": [
            feature(IDENTIFIER_FEATURE, "C-kept"),
            feature(EVENT_TIME_FEATURE, &event_time),
            feature("city", "Seattle"),
            feature("tier", "silver")
        ] });
        if let Some(ttl) = record_ttl {
            seed["TtlDuration"] = ttl;
        }
        let (status, body) = emulator.put_record(&group, seed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seeding PutRecord to answer 200, got {status} ({body})"
        );

        // The update posts only the features it changes: no `TtlDuration` member, and not
        // the group's event-time feature.
        let (status, body) = emulator
            .update_record(
                &group,
                json!({
                    "RecordIdentifierValueAsString": "C-kept",
                    "Features": posted
                        .iter()
                        .map(|(name, value)| feature(name, value))
                        .collect::<Vec<_>>()
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an update that states no TtlDuration and omits the \
             EventTime to answer 200, got {status} ({body})"
        );

        let (status, served) = emulator.get_record(&group, "C-kept").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} ({served})"
        );

        // The merge was applied…
        for (name, value) in &posted {
            let actual = served_value(&served, name);
            assert_eq!(
                actual.as_deref(),
                Some(*value),
                "case {case:?}: expected the posted feature {name:?} to read back as {value:?}, \
                 got {actual:?}"
            );
        }
        // …every feature the update did not name keeps its stored value…
        for (name, value) in [("city", "Seattle"), ("tier", "silver")] {
            if posted.iter().any(|(posted_name, _)| *posted_name == name) {
                continue;
            }
            let actual = served_value(&served, name);
            assert_eq!(
                actual.as_deref(),
                Some(value),
                "case {case:?}: expected the unposted feature {name:?} to keep {value:?}, got \
                 {actual:?}"
            );
        }
        // …the record keeps its own EventTime, in the spelling it was stored in…
        let kept = served_value(&served, EVENT_TIME_FEATURE);
        assert_eq!(
            kept.as_deref(),
            Some(event_time.as_str()),
            "case {case:?}: expected the record to keep its EventTime {event_time:?}, got \
             {kept:?}"
        );
        // …and `ExpiresAt` is re-derived from that kept event time under the TTL the record
        // already stands under: its own duration where it has one, and the group's default
        // otherwise.
        assert_expiry(
            case,
            &served,
            &event_time,
            record_ttl_secs.or(default_ttl),
        );
    }
}

/// One row of the request-`TtlDuration` table: the case name, the feature group's default
/// TTL in seconds, the `TtlDuration` the update states and the seconds that is, whether the
/// update carries an `EventTime`, and the status it must answer.
type StatedTtlRow<'a> = (&'a str, Option<f64>, Value, f64, bool, u16);

#[tokio::test]
async fn an_update_stating_a_ttl_duration_needs_an_event_time_beside_it() {
    // The class the rule spans: a request-level `TtlDuration` in several of the model's
    // units, with and without a default TTL on the feature group, crossed with whether the
    // update carries the group's event-time feature. Only the last decides the verdict.
    let rows: Vec<StatedTtlRow<'_>> = vec![
        (
            "a request TtlDuration in minutes, no group default, no EventTime",
            None,
            json!({ "Unit": "Minutes", "Value": 30 }),
            1_800.0,
            false,
            400,
        ),
        (
            "a request TtlDuration in minutes, a group default, no EventTime",
            Some(3_600.0),
            json!({ "Unit": "Minutes", "Value": 30 }),
            1_800.0,
            false,
            400,
        ),
        (
            "a request TtlDuration in weeks, a group default, no EventTime",
            Some(3_600.0),
            json!({ "Unit": "Weeks", "Value": 2 }),
            1_209_600.0,
            false,
            400,
        ),
        (
            "a request TtlDuration in days, a group default, with an EventTime",
            Some(3_600.0),
            json!({ "Unit": "Days", "Value": 1 }),
            86_400.0,
            true,
            200,
        ),
        (
            "a request TtlDuration in seconds, no group default, with an EventTime",
            None,
            json!({ "Unit": "Seconds", "Value": 600 }),
            600.0,
            true,
            200,
        ),
    ];

    for (index, (case, default_ttl, ttl, ttl_secs, with_event_time, expected_status)) in
        rows.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        let group = format!("statedttl{index}");
        emulator.create_feature_group(&group, default_ttl);
        let event_time = tomorrow_iso();
        let (status, body) = emulator
            .put_record(
                &group,
                json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, "C-stated"),
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("city", "Seattle")
                ] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seeding PutRecord to answer 200, got {status} ({body})"
        );
        let (_, before) = emulator.get_record(&group, "C-stated").await;
        let before_pairs = record_pairs(&before);

        let mut features = vec![feature("city", "Portland")];
        if with_event_time {
            features.push(feature(EVENT_TIME_FEATURE, &event_time));
        }
        let (status, answered) = emulator
            .update_record(
                &group,
                json!({
                    "RecordIdentifierValueAsString": "C-stated",
                    "Features": features,
                    "TtlDuration": ttl
                }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );

        let (_, served) = emulator.get_record(&group, "C-stated").await;
        if expected_status == 200 {
            // The duration the request states replaces the one the record stood under, and
            // the expiry is re-derived from the event time beside it.
            assert_expiry(case, &served, &event_time, Some(ttl_secs));
            continue;
        }

        let code = error_code(&answered);
        let stated = message(&answered);
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationError", EVENT_TIME_REQUIRED_FOR_TTL),
            "case {case:?}: expected ValidationError stating \
             {EVENT_TIME_REQUIRED_FOR_TTL:?}, got {code:?} with {stated:?}"
        );
        // A refused update changes nothing: the record is as it was, under the TTL it was
        // ingested beneath.
        let after_pairs = record_pairs(&served);
        assert_eq!(
            after_pairs, before_pairs,
            "case {case:?}: expected the refused update to leave the record as it was \
             ({before_pairs:?}), got {after_pairs:?}"
        );
        assert_expiry(case, &served, &event_time, default_ttl);
    }
}

#[tokio::test]
async fn a_soft_delete_re_derives_the_expiry_from_the_ttl_the_record_was_written_under() {
    let emulator = Emulator::new();
    let ingested_at = tomorrow_iso();
    // The delete's own `EventTime`, an hour after the record's: a `SoftDelete` moves the
    // record's event time to it, so it is the instant the expiry is measured from.
    let deleted_at = DateTime::from_secs_f64(tomorrow() + 3_600.0)
        .fmt(Format::DateTime)
        .expect("format an ISO-8601 event time");

    for (index, (case, unit, value, expected)) in DEFAULT_TTL_ROWS.iter().enumerate() {
        // The same duration from each of the two sources a record can be under one from,
        // so neither the write's own `TtlDuration` nor the group's default is lost.
        let sources: [(&str, Option<f64>, Option<Value>); 2] = [
            (
                "a record-level TtlDuration",
                None,
                Some(json!({ "Unit": unit, "Value": value })),
            ),
            ("the feature group's default TTL", Some(*expected), None),
        ];
        for (source, group_default, request_ttl) in sources {
            let group = format!("softdelete{index}{}", group_default.is_some() as u8);
            let case = format!("{case} as {source}");
            emulator.create_feature_group(&group, group_default);

            let mut request = json!({ "Record": [
                feature(IDENTIFIER_FEATURE, "C1"),
                feature(EVENT_TIME_FEATURE, &ingested_at),
                feature("city", "Seattle")
            ] });
            if let Some(ttl) = request_ttl {
                request["TtlDuration"] = ttl;
            }
            let (status, body) = emulator.put_record(&group, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutRecord 200, got {status} ({body})"
            );
            let (_, body) = emulator.get_record(&group, "C1").await;
            assert_expiry(
                &format!("{case}: the ingested record"),
                &body,
                &ingested_at,
                Some(*expected),
            );

            let (status, body) = emulator
                .delete_record(
                    &group,
                    &format!(
                        "RecordIdentifierValueAsString=C1&EventTime={deleted_at}\
                         &DeletionMode=SoftDelete"
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteRecord 200, got {status} ({body})"
            );

            let stored = emulator
                .stored_record(&group, "C1")
                .unwrap_or_else(|| panic!("case {case:?}: the tombstone must still be held"));
            assert!(
                stored.soft_deleted,
                "case {case:?}: expected a soft-deleted record, got {stored:?}"
            );
            assert_eq!(
                stored.ttl_secs,
                Some(*expected),
                "case {case:?}: expected the tombstone to keep the duration of {expected} s, \
                 got {:?}",
                stored.ttl_secs
            );
            let at = stored.expires_at_secs.unwrap_or_else(|| {
                panic!("case {case:?}: expected the tombstone to carry an expiry, got {stored:?}")
            });
            let delta = at - secs(&deleted_at);
            assert!(
                (delta - *expected).abs() < 0.001,
                "case {case:?}: expected the expiry {expected} s after the delete's event \
                 time, got {delta} s"
            );
            // The ISO-8601 member and the seconds one are the same instant.
            let iso = stored
                .expires_at
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: expected an ExpiresAt, got {stored:?}"));
            assert!(
                (secs(&iso) - at).abs() < 0.001,
                "case {case:?}: expected ExpiresAt {iso:?} to be the stored instant {at}"
            );
        }
    }
}

/// One row of the tombstone-expiry table: which of the two TTL sources the deleted record
/// is under, the `EventTime` the delete states, and whether the tombstone the delete
/// leaves outlives the next read of its feature group.
struct TombstoneRow {
    case: &'static str,
    /// The `OnlineStoreConfig.TtlDuration` the feature group is created with, in seconds.
    group_default_secs: Option<f64>,
    /// The `TtlDuration` the write of the record states, when it states one.
    request_ttl: Option<Value>,
    deleted_at: String,
    outlives: bool,
}

#[tokio::test]
async fn a_soft_deleted_record_is_hard_deleted_once_its_re_derived_expiry_falls_due() {
    // An instant long past: a tombstone whose event time is this one has an expiry
    // already due under any duration, so the hard delete the clock owes is materialised
    // by the next read — no waiting on a clock.
    const LONG_PAST: &str = "2000-01-01T00:00:00Z";
    let a_day_out = tomorrow_iso();

    // Each record below is written with NO value for the group's event-time feature, so
    // the record itself is under a duration but at no instant, and the first event time
    // it has is the one the delete states: that is what makes the re-derived expiry of a
    // tombstone observable without waiting for one to fall due.
    let rows = vec![
        TombstoneRow {
            case: "a one-second record TTL, deleted long past",
            group_default_secs: None,
            request_ttl: Some(json!({ "Unit": "Seconds", "Value": 1 })),
            deleted_at: LONG_PAST.to_owned(),
            outlives: false,
        },
        TombstoneRow {
            case: "an hour's record TTL, deleted long past",
            group_default_secs: None,
            request_ttl: Some(json!({ "Unit": "Hours", "Value": 1 })),
            deleted_at: LONG_PAST.to_owned(),
            outlives: false,
        },
        TombstoneRow {
            case: "a 52-week record TTL, deleted long past",
            group_default_secs: None,
            request_ttl: Some(json!({ "Unit": "Weeks", "Value": 52 })),
            deleted_at: LONG_PAST.to_owned(),
            outlives: false,
        },
        TombstoneRow {
            case: "the feature group's default TTL, deleted long past",
            group_default_secs: Some(1.0),
            request_ttl: None,
            deleted_at: LONG_PAST.to_owned(),
            outlives: false,
        },
        TombstoneRow {
            case: "a 52-week record TTL, deleted a day out",
            group_default_secs: None,
            request_ttl: Some(json!({ "Unit": "Weeks", "Value": 52 })),
            deleted_at: a_day_out,
            outlives: true,
        },
        TombstoneRow {
            case: "no TTL in force at all",
            group_default_secs: None,
            request_ttl: None,
            deleted_at: LONG_PAST.to_owned(),
            outlives: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TombstoneRow {
            case,
            group_default_secs,
            request_ttl,
            deleted_at,
            outlives,
        } = row;
        let emulator = Emulator::new();
        let group = format!("tombstone{index}");
        emulator.create_feature_group(&group, group_default_secs);

        let mut request = json!({ "Record": [
            feature(IDENTIFIER_FEATURE, "C1"),
            feature("city", "Seattle")
        ] });
        if let Some(ttl) = request_ttl {
            request["TtlDuration"] = ttl;
        }
        let (status, body) = emulator.put_record(&group, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRecord 200, got {status} ({body})"
        );
        let (_, body) = emulator.list_records_including_soft_deleted(&group).await;
        assert_eq!(
            body["RecordIdentifiers"],
            json!(["C1"]),
            "case {case:?}: expected the written identifier to be listed, got {}",
            body["RecordIdentifiers"]
        );

        let (status, body) = emulator
            .delete_record(
                &group,
                &format!(
                    "RecordIdentifierValueAsString=C1&EventTime={deleted_at}\
                     &DeletionMode=SoftDelete"
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRecord 200, got {status} ({body})"
        );

        let (status, body) = emulator.list_records_including_soft_deleted(&group).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRecords 200, got {status} ({body})"
        );
        let expected = if outlives { json!(["C1"]) } else { json!([]) };
        assert_eq!(
            body["RecordIdentifiers"], expected,
            "case {case:?}: expected the soft-deleted identifiers {expected}, got {}",
            body["RecordIdentifiers"]
        );
    }
}

/// Whether `code` is the classification this data plane refuses a malformed request with:
/// the modelled `ValidationError` a handler answers, or the `ValidationException` the
/// protocol boundary answers for a value the model's own constraints already exclude.
fn is_validation_code(code: &str) -> bool {
    code == "ValidationError" || code == "ValidationException"
}

/// The `ErrorCode` of each entry `BatchWriteRecord` reports in `Errors`.
fn entry_error_codes(body: &Value) -> Vec<String> {
    body.get("Errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|failed| failed["ErrorCode"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the TTL-unit table: the case name, the `Unit` a request states, the `Value`
/// beside it, and the span in seconds the two denote — `None` for a unit the model's
/// `TtlDurationUnit` enum does not declare, which denotes no span at all.
type TtlUnitRow<'a> = (&'a str, &'a str, i32, Option<f64>);

#[tokio::test]
async fn one_ttl_duration_unit_rule_serves_every_member_that_carries_a_duration() {
    // `ExpiresAt = EventTime + TtlDuration` for a unit the model's `TtlDurationUnit` enum
    // declares, and a refusal for one it does not — the same reading wherever a
    // `TtlDuration` reaches this data plane: a whole-record write's own duration
    // (`PutRecord`, and each `BatchWriteRecord` entry), the request-level duration
    // `BatchWriteRecord` applies "to all entries that do not specify their own
    // TtlDuration", and the duration a partial update states.
    //
    // Rows: every unit the enum declares — Seconds, Minutes, Hours, Days, Weeks
    // (`smithy_query(query="shape", shape="TtlDurationUnit")`, the list
    // `TTL_DURATION_UNIT_VALUES` in `ops::types::shared_types` holds) — at the published
    // lower bound of `TtlDurationValue` (range 1..) and above it, crossed with spellings
    // the enum declares nowhere.
    let rows: [TtlUnitRow<'_>; 9] = [
        (
            "Seconds, at the lower bound of TtlDurationValue",
            "Seconds",
            1,
            Some(1.0),
        ),
        ("Minutes", "Minutes", 90, Some(5_400.0)),
        ("Hours", "Hours", 36, Some(129_600.0)),
        ("Days", "Days", 2, Some(172_800.0)),
        ("Weeks", "Weeks", 52, Some(31_449_600.0)),
        ("a unit the enum spells nowhere", "Fortnights", 2, None),
        ("a declared unit in lower case", "days", 2, None),
        ("a declared unit in upper case", "WEEKS", 1, None),
        ("a declared unit with a trailing space", "Hours ", 1, None),
    ];

    for (case, unit, value, span) in rows {
        let emulator = Emulator::new();
        // No default TTL on the group, so an `ExpiresAt` a read reports can only come
        // from the duration the request states.
        emulator.create_feature_group("ttlunit", None);
        let event_time = tomorrow_iso();
        let ttl = json!({ "Unit": unit, "Value": value });

        // The duration a WHOLE-RECORD write states for itself.
        let (status, body) = emulator
            .put_record(
                "ttlunit",
                json!({
                    "Record": [
                        feature(IDENTIFIER_FEATURE, "C-put"),
                        feature(EVENT_TIME_FEATURE, &event_time),
                        feature("city", "Seattle")
                    ],
                    "TtlDuration": ttl.clone()
                }),
            )
            .await;
        let code = error_code(&body);
        match span {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?} via PutRecord: expected 200 for a declared unit, got \
                     {status} with body {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-put").await;
                assert_expiry(
                    &format!("{case} via PutRecord"),
                    &served,
                    &event_time,
                    Some(expected),
                );
            }
            None => {
                assert!(
                    status == 400 && is_validation_code(&code),
                    "case {case:?} via PutRecord: expected HTTP 400 with a validation error \
                     code for an undeclared unit, got {status} {code:?} with body {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-put").await;
                let pairs = record_pairs(&served);
                assert!(
                    pairs.is_empty(),
                    "case {case:?} via PutRecord: expected the refused write to store \
                     nothing, got {pairs:?}"
                );
            }
        }

        // The same duration on ONE ENTRY of a batch, which is the same whole-record
        // write.
        let (status, body) = emulator
            .batch_write_record(json!({ "Entries": [{
                "FeatureGroupName": "ttlunit",
                "Record": [
                    feature(IDENTIFIER_FEATURE, "C-entry"),
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("city", "Denver")
                ],
                "TtlDuration": ttl.clone()
            }] }))
            .await;
        let code = error_code(&body);
        let entry_codes = entry_error_codes(&body);
        match span {
            Some(expected) => {
                assert_eq!(
                    (status, entry_codes.clone()),
                    (200, Vec::new()),
                    "case {case:?} via a BatchWriteRecord entry: expected 200 with no entry \
                     errors for a declared unit, got {status} {entry_codes:?} with body \
                     {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-entry").await;
                assert_expiry(
                    &format!("{case} via a BatchWriteRecord entry"),
                    &served,
                    &event_time,
                    Some(expected),
                );
            }
            None => {
                // Either layer may be the one that refuses it — the boundary validator
                // for the modelled enum, refusing the whole request, or the handler's own
                // guard, reporting the ONE entry it belongs to — and neither writes the
                // entry's record.
                let refused_whole = status == 400 && is_validation_code(&code);
                let refused_entry = status == 200
                    && entry_codes.len() == 1
                    && is_validation_code(&entry_codes[0]);
                assert!(
                    refused_whole || refused_entry,
                    "case {case:?} via a BatchWriteRecord entry: expected an undeclared unit \
                     to be refused, as HTTP 400 or as that entry's own validation error, got \
                     {status} {code:?} {entry_codes:?} with body {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-entry").await;
                let pairs = record_pairs(&served);
                assert!(
                    pairs.is_empty(),
                    "case {case:?} via a BatchWriteRecord entry: expected the refused entry \
                     to store nothing, got {pairs:?}"
                );
            }
        }

        // The REQUEST-level duration, which "applies to all entries that do not specify
        // their own TtlDuration": it would misread every entry, so it is the request's own
        // failure rather than one entry's.
        let (status, body) = emulator
            .batch_write_record(json!({
                "TtlDuration": ttl.clone(),
                "Entries": [{
                    "FeatureGroupName": "ttlunit",
                    "Record": [
                        feature(IDENTIFIER_FEATURE, "C-request"),
                        feature(EVENT_TIME_FEATURE, &event_time),
                        feature("city", "Austin")
                    ]
                }]
            }))
            .await;
        let code = error_code(&body);
        let entry_codes = entry_error_codes(&body);
        match span {
            Some(expected) => {
                assert_eq!(
                    (status, entry_codes.clone()),
                    (200, Vec::new()),
                    "case {case:?} via a request-level TtlDuration: expected 200 with no \
                     entry errors for a declared unit, got {status} {entry_codes:?} with \
                     body {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-request").await;
                assert_expiry(
                    &format!("{case} via a request-level TtlDuration"),
                    &served,
                    &event_time,
                    Some(expected),
                );
            }
            None => {
                assert!(
                    status == 400 && is_validation_code(&code),
                    "case {case:?} via a request-level TtlDuration: expected HTTP 400 with a \
                     validation error code for an undeclared unit, got {status} {code:?} \
                     with body {body:?}"
                );
                let (_, served) = emulator.get_record("ttlunit", "C-request").await;
                let pairs = record_pairs(&served);
                assert!(
                    pairs.is_empty(),
                    "case {case:?} via a request-level TtlDuration: expected the refused \
                     request to store nothing, got {pairs:?}"
                );
            }
        }

        // The duration a PARTIAL UPDATE states, beside the `EventTime` the same request
        // has to carry. The record it updates is ingested under no duration at all, so the
        // `ExpiresAt` it ends up with is the one this update derives.
        let (status, body) = emulator
            .put_record(
                "ttlunit",
                json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, "C-update"),
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("city", "Boise")
                ] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the record the update is applied to to be ingested, got \
             {status} with body {body:?}"
        );
        let (status, body) = emulator
            .update_record(
                "ttlunit",
                json!({
                    "RecordIdentifierValueAsString": "C-update",
                    "Features": [
                        feature(EVENT_TIME_FEATURE, &event_time),
                        feature("city", "Reno")
                    ],
                    "TtlDuration": ttl.clone()
                }),
            )
            .await;
        let code = error_code(&body);
        let (_, served) = emulator.get_record("ttlunit", "C-update").await;
        let city = served_value(&served, "city");
        match span {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?} via UpdateRecord: expected 200 for a declared unit, got \
                     {status} with body {body:?}"
                );
                assert_eq!(
                    city.as_deref(),
                    Some("Reno"),
                    "case {case:?} via UpdateRecord: expected the update to be applied, got \
                     {city:?}"
                );
                assert_expiry(
                    &format!("{case} via UpdateRecord"),
                    &served,
                    &event_time,
                    Some(expected),
                );
            }
            None => {
                assert!(
                    status == 400 && is_validation_code(&code),
                    "case {case:?} via UpdateRecord: expected HTTP 400 with a validation \
                     error code for an undeclared unit, got {status} {code:?} with body \
                     {body:?}"
                );
                assert_eq!(
                    city.as_deref(),
                    Some("Boise"),
                    "case {case:?} via UpdateRecord: expected the refused update to change \
                     nothing, got {city:?}"
                );
                let at = expires_at(&served);
                assert!(
                    at.is_none(),
                    "case {case:?} via UpdateRecord: expected the refused update to derive \
                     no ExpiresAt, got {at:?}"
                );
            }
        }
    }
}

/// An instant years in the PAST, as the ISO-8601 string an event-time feature of type
/// `String` carries.
///
/// A record's event time can be arbitrarily old, so `EventTime + TtlDuration` under a
/// short duration states an expiry that already fell due — the instant at which a record
/// is hard-deleted on the next call that reads its feature group.
const LONG_PAST_EVENT_TIME: &str = "2020-03-05T06:07:08Z";

/// How a row of the moved-default table spells the event time its record is written under.
#[derive(Clone, Copy, Debug)]
enum WrittenEventTime {
    /// An ISO-8601 instant a day out, so every duration in the table is still in force.
    Ahead,
    /// The same instant in epoch seconds, the other spelling an event-time feature takes.
    AheadEpoch,
    /// An ISO-8601 instant years in the past ([`LONG_PAST_EVENT_TIME`]).
    LongPast,
    /// That past instant in epoch seconds.
    LongPastEpoch,
}

impl WrittenEventTime {
    /// The value the event-time feature carries for this spelling.
    fn spelling(self) -> String {
        match self {
            Self::Ahead => tomorrow_iso(),
            Self::AheadEpoch => tomorrow_epoch(),
            Self::LongPast => LONG_PAST_EVENT_TIME.to_owned(),
            Self::LongPastEpoch => format!("{}", secs(LONG_PAST_EVENT_TIME) as i64),
        }
    }
}

/// Re-register `group`'s definition with the default TTL `default_ttl_seconds`, the way
/// the SageMaker control plane's `UpdateFeatureGroup` does: it re-registers the record of
/// the ONE shared feature-group directory this data plane resolves names against, so a
/// group's default is not fixed at creation ("To remove the default TtlDuration from an
/// existing feature group, use the UpdateFeatureGroup API"). `None` leaves the group
/// configured with no default at all.
fn move_feature_group_default_ttl(
    emulator: &Emulator,
    group: &str,
    default_ttl_seconds: Option<f64>,
) {
    let state = emulator.engine.state::<FeatureGroupDirectory>();
    let mut directory = state.lock();
    let mut definition = directory
        .get(group)
        .expect("the feature group the case created")
        .clone();
    definition.default_ttl_seconds = default_ttl_seconds;
    directory.register(definition);
}

/// One row of the moved-default table: the case name, the group's default TTL in seconds
/// when the record was WRITTEN, the `TtlDuration` that write stated and the span in
/// seconds it denotes, the default the control plane moves the group to afterwards, the
/// spelling of the record's event time, and the `TtlDuration` the update states together
/// with the span it denotes — `None` for the partial update that states none.
type MovedDefaultRow<'a> = (
    &'a str,
    Option<f64>,
    Option<Value>,
    Option<f64>,
    Option<f64>,
    WrittenEventTime,
    Option<(Value, f64)>,
);

#[tokio::test]
async fn an_update_leaves_the_ttl_setting_the_write_stored_the_record_under() {
    // A record's TTL SETTING is the duration the write that stored it settled on — its own
    // when that write stated one and otherwise the default its feature group configured AT
    // THAT TIME — and "If you omit this parameter, the record's existing TTL setting
    // remains unchanged" (`TtlDuration`,
    // `smithy_query(query="doc", shape="UpdateRecordRequest", member="TtlDuration")`), so
    // an update that states no duration re-derives `ExpiresAt` from the duration already
    // stored and never from the default the group carries now, while one that states a
    // duration replaces it.
    //
    // Rows span the ways a group's default can MOVE after the record was written — the
    // control plane adding one, removing one, raising and lowering one, including to the
    // published lower bound of `TtlDurationValue` (range 1..) — crossed with a record
    // stored under its own duration and under none, with both spellings of an event time,
    // and with an event time years in the past, where applying a short default would state
    // an expiry already fallen due and hard-delete the record the update just merged into.
    let rows: [MovedDefaultRow<'_>; 10] = [
        (
            "no default when the record was written, an hourly default added afterwards",
            None,
            None,
            None,
            Some(3_600.0),
            WrittenEventTime::Ahead,
            None,
        ),
        (
            "no default when the record was written, a one-minute default added afterwards, \
             an event time years past",
            None,
            None,
            None,
            Some(60.0),
            WrittenEventTime::LongPast,
            None,
        ),
        (
            "no default when the record was written, a weekly default added afterwards, an \
             epoch event time years past",
            None,
            None,
            None,
            Some(604_800.0),
            WrittenEventTime::LongPastEpoch,
            None,
        ),
        (
            "a default when the record was written, removed afterwards",
            Some(7_200.0),
            None,
            None,
            None,
            WrittenEventTime::Ahead,
            None,
        ),
        (
            "a default when the record was written, raised afterwards, an epoch event time",
            Some(3_600.0),
            None,
            None,
            Some(1_209_600.0),
            WrittenEventTime::AheadEpoch,
            None,
        ),
        (
            "a default when the record was written, lowered afterwards to the lower bound \
             of TtlDurationValue",
            Some(172_800.0),
            None,
            None,
            Some(1.0),
            WrittenEventTime::Ahead,
            None,
        ),
        (
            "a record-level duration written under no default, a default added afterwards",
            None,
            Some(json!({ "Unit": "Days", "Value": 2 })),
            Some(172_800.0),
            Some(60.0),
            WrittenEventTime::Ahead,
            None,
        ),
        (
            "a record-level duration over a default, the default raised afterwards",
            Some(45.0),
            Some(json!({ "Unit": "Weeks", "Value": 1 })),
            Some(604_800.0),
            Some(1_209_600.0),
            WrittenEventTime::AheadEpoch,
            None,
        ),
        (
            "an update stating its own duration, over a default added afterwards",
            None,
            None,
            None,
            Some(60.0),
            WrittenEventTime::Ahead,
            Some((json!({ "Unit": "Hours", "Value": 3 }), 10_800.0)),
        ),
        (
            "an update stating its own duration, over the record's duration and a moved \
             default",
            Some(3_600.0),
            Some(json!({ "Unit": "Days", "Value": 1 })),
            Some(86_400.0),
            Some(604_800.0),
            WrittenEventTime::AheadEpoch,
            Some((json!({ "Unit": "Seconds", "Value": 90 }), 90.0)),
        ),
    ];

    for (
        index,
        (case, default_at_write, write_ttl, write_ttl_secs, moved_default, written_at, update_ttl),
    ) in rows.into_iter().enumerate()
    {
        // Each row needs its own feature group: the default TTL is the GROUP's.
        let emulator = Emulator::new();
        let group = format!("moveddefault{index}");
        emulator.create_feature_group(&group, default_at_write);

        // The record is written while the group carries the row's original default, so the
        // TTL setting it is stored under is the one that write settles.
        let event_time = written_at.spelling();
        let mut seed = json!({ "Record": [
            feature(IDENTIFIER_FEATURE, "C-moved"),
            feature(EVENT_TIME_FEATURE, &event_time),
            feature("city", "Seattle"),
            feature("tier", "silver")
        ] });
        if let Some(ttl) = write_ttl {
            seed["TtlDuration"] = ttl;
        }
        let (status, body) = emulator.put_record(&group, seed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seeding PutRecord to answer 200, got {status} ({body})"
        );
        let stored_ttl_secs = write_ttl_secs.or(default_at_write);

        // Then the control plane moves the group's default, which changes what a later
        // ingestion would settle on — and nothing about the record already stored.
        move_feature_group_default_ttl(&emulator, &group, moved_default);

        // The update posts the feature it changes, the event time beside it only where it
        // states a `TtlDuration` of its own ("If you specify a TtlDuration, you must also
        // provide an EventTime in the request").
        let mut posted = vec![feature("city", "Portland")];
        if update_ttl.is_some() {
            posted.push(feature(EVENT_TIME_FEATURE, &event_time));
        }
        let mut request = json!({
            "RecordIdentifierValueAsString": "C-moved",
            "Features": posted
        });
        if let Some((ttl, _)) = update_ttl.clone() {
            request["TtlDuration"] = ttl;
        }
        let (status, body) = emulator.update_record(&group, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer 200, got {status} ({body})"
        );

        let (status, served) = emulator.get_record(&group, "C-moved").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord 200, got {status} ({served})"
        );

        // The record the update merged into is still there: a moved default settles no
        // expiry onto it, so nothing hard-deletes it out from under the merge.
        let pairs = record_pairs(&served);
        assert!(
            !pairs.is_empty(),
            "case {case:?}: expected the updated record to still be served, got {pairs:?}"
        );
        let city = served_value(&served, "city");
        assert_eq!(
            city.as_deref(),
            Some("Portland"),
            "case {case:?}: expected the merged feature to read back as \"Portland\", got \
             {city:?}"
        );
        let tier = served_value(&served, "tier");
        assert_eq!(
            tier.as_deref(),
            Some("silver"),
            "case {case:?}: expected the unposted feature to keep \"silver\", got {tier:?}"
        );
        let kept = served_value(&served, EVENT_TIME_FEATURE);
        assert_eq!(
            kept.as_deref(),
            Some(event_time.as_str()),
            "case {case:?}: expected the record to keep its EventTime {event_time:?}, got \
             {kept:?}"
        );

        // The expiry the update leaves is derived from the duration the record is UNDER:
        // the one the update states where it states one, and otherwise the one the write
        // stored it under — never the group's moved default.
        let expected_ttl_secs = update_ttl
            .as_ref()
            .map(|(_, secs)| *secs)
            .or(stored_ttl_secs);
        assert_expiry(case, &served, &event_time, expected_ttl_secs);

        // And the stored SETTING itself, the duration a later call re-derives from, is that
        // same span.
        let stored = emulator
            .stored_record(&group, "C-moved")
            .expect("the updated record in the group's online store");
        let settled = stored.ttl_secs;
        assert!(
            match (expected_ttl_secs, settled) {
                (None, actual) => actual.is_none(),
                (Some(expected), Some(actual)) => (actual - expected).abs() < 0.001,
                (Some(_), None) => false,
            },
            "case {case:?}: expected the record's stored TTL duration to be \
             {expected_ttl_secs:?} s, got {settled:?}"
        );
    }
}
