// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the rejections whose error code a client branches on first: a
//! `BatchGetRecord` batch larger than the modelled maximum, a `DeleteRecord` whose
//! `EventTime` is before the record's, and a write that states no readable value for one
//! of the two features the feature group's definition fixes.
//!
//! RULES:
//!
//! * A value outside a modelled constraint is refused with `400 ValidationException` and
//!   a message BUILT FROM THE REQUEST: `N validation error(s) detected: Value '<value>'
//!   at '<member path>' failed to satisfy constraint: <the constraint the model states>`.
//!   For `BatchGetRecordRequest$Identifiers` (`@length` min 1, max 100, `@required`) the
//!   member path is `identifiers` and the constraint is the `@length` maximum, whatever
//!   the batch's size past it.
//! * A deletion is ordered against the stored record's own `EventTime`: the request's
//!   must be the SAME as it or after it. A strictly older one does not delete — a
//!   `SoftDelete` leaves the record standing, a `HardDelete` is refused with the modelled
//!   `ValidationError` (400) stating that rule.
//! * A feature whose `CollectionType` is `None` carries its value in `ValueAsString`
//!   ("Used when your CollectionType is None"), while `ValueAsStringList` is "Used when
//!   your CollectionType is a List, Set, or Vector"
//!   (`smithy_query(query="shape", shape="FeatureValue")`). The group's
//!   `RecordIdentifierFeatureName` and `EventTimeFeatureName` are both features of the
//!   first kind, so a request that names one of them with its value anywhere but
//!   `ValueAsString` states no value this plane can read for it and is REFUSED with the
//!   modelled `ValidationError` — never read as a record that states no identifier or no
//!   event time at all, which would drop the record's TTL and let a stale write replace a
//!   newer record. Every writer reads the two features that way: `PutRecord`,
//!   `BatchWriteRecord` (as that one entry's own error) and `UpdateRecord`.
//! * A write the emulator's BOUNDED store has no room for — a record carrying more feature
//!   values than the store keeps per record — is answered as the store's own limit (the
//!   modelled `ServiceUnavailable`, or an entry reported as not processed), never as the
//!   caller's mistake; and an event time of any magnitude a request can state is ANSWERED,
//!   the expiry derived from it being reported only where an ISO-8601 timestamp states it.
//!
//! Every case goes through the real front door, so what is asserted is the wire behaviour
//! a client sees.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group every case reads from, and the two feature names its definition
/// fixes.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The modelled maximum of `BatchGetRecordRequest$Identifiers`.
///
/// Source: `smithy_query(query="operation", operation="BatchGetRecord")` on this crate's
/// `smithy-model/` — `Identifiers` targets `BatchGetRecordIdentifiers` with
/// `length {min: 1, max: 100}` and is `@required`.
const MAX_IDENTIFIERS: usize = 100;

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
        let emulator = Self { engine, router };
        emulator.create_feature_group(GROUP);
        emulator
    }

    /// Create a feature group with an OnlineStore, the way the SageMaker control plane's
    /// `CreateFeatureGroup` does.
    fn create_feature_group(&self, name: &str) {
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
                default_ttl_seconds: None,
                creation_failed: false,
            });
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
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

    /// Ingest one record at `event_time`.
    async fn put_record(&self, identifier: &str, event_time: &str) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/FeatureGroup/{GROUP}"),
            Some(json!({ "Record": [
                feature(IDENTIFIER_FEATURE, identifier),
                feature(EVENT_TIME_FEATURE, event_time),
                feature("feature_a", "original")
            ] })),
        )
        .await
    }

    /// Whether `GetRecord` still serves the record.
    async fn record_is_served(&self, identifier: &str) -> bool {
        let (_, body) = self
            .send(
                "GET",
                &format!(
                    "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}"
                ),
                None,
            )
            .await;
        body.get("Record").and_then(Value::as_array).is_some()
    }

    /// One whole-record write, with the `Record` and the `TtlDuration` the caller states.
    async fn put(&self, body: Value) -> (u16, Value) {
        self.send("PUT", &format!("/FeatureGroup/{GROUP}"), Some(body))
            .await
    }

    /// The same whole-record write, arriving as the one entry of a batch.
    async fn batch_write(&self, record: &Value, ttl: &Value) -> (u16, Value) {
        self.send(
            "POST",
            "/BatchWriteRecord",
            Some(json!({ "Entries": [{
                "FeatureGroupName": GROUP,
                "Record": record,
                "TtlDuration": ttl
            }] })),
        )
        .await
    }

    /// One partial update of a record.
    async fn update(&self, body: Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{GROUP}/Record"),
            Some(body),
        )
        .await
    }

    /// The record `identifier` as `GetRecord` serves it, with its `ExpiresAt`.
    async fn get(&self, identifier: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!(
                "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}\
                 &ExpirationTimeResponse=Enabled"
            ),
            None,
        )
        .await
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// One `FeatureValue` carrying a LIST of values — the form a feature whose
/// `CollectionType` is a `List`, `Set` or `Vector` takes.
fn feature_list(name: &str, values: &[&str]) -> Value {
    json!({ "FeatureName": name, "ValueAsStringList": values })
}

/// One `FeatureValue` carrying NO value at all: both value members of `FeatureValue` are
/// optional, so a request can name a feature and state nothing for it.
fn feature_without_value(name: &str) -> Value {
    json!({ "FeatureName": name })
}

/// The forms in which a request can name `feature_name` and still state no value this
/// plane can read as ONE value, as rows of the tables below.
///
/// They differ along what the rule generalises over: the value member the request uses and
/// how many values it carries there — a list of one, a longer list, the empty list that
/// `ValueAsStringList` (`length min 0`) admits, and neither value member at all.
fn unreadable_forms(feature_name: &str) -> Vec<(String, Value)> {
    vec![
        (
            format!("'{feature_name}' as a list of one"),
            feature_list(feature_name, &["V1"]),
        ),
        (
            format!("'{feature_name}' as a list of three"),
            feature_list(feature_name, &["V1", "V2", "V3"]),
        ),
        (
            format!("'{feature_name}' as the empty list"),
            feature_list(feature_name, &[]),
        ),
        (
            format!("'{feature_name}' with neither value member"),
            feature_without_value(feature_name),
        ),
    ]
}

/// The `Errors` a `BatchWriteRecord` response reports, as `(ErrorCode, ErrorMessage)`.
fn reported_errors(body: &Value) -> Vec<(String, String)> {
    body.get("Errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|error| {
                    (
                        error["ErrorCode"].as_str().unwrap_or_default().to_owned(),
                        error["ErrorMessage"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value `GetRecord` reports for `feature_name`, as the scalar or the list the
/// response carried (`Value::Null` when the record carries no such feature).
fn served_value(body: &Value, feature_name: &str) -> Value {
    body.get("Record")
        .and_then(Value::as_array)
        .and_then(|features| {
            features
                .iter()
                .find(|feature| feature["FeatureName"] == json!(feature_name))
        })
        .and_then(|feature| {
            feature
                .get("ValueAsString")
                .or_else(|| feature.get("ValueAsStringList"))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The `ExpiresAt` a `GetRecord` with `ExpirationTimeResponse=Enabled` reports.
fn expires_at(body: &Value) -> Option<String> {
    body.get("ExpiresAt")
        .and_then(Value::as_str)
        .map(str::to_owned)
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

/// The message an error response carries.
///
/// The framework's own constraint refusal carries it as `message`, while this service's
/// modelled errors declare a `Message` member (`ValidationError$Message`), which
/// restJson1 serialises under that name — so both spellings are read here.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn a_batch_past_the_modeled_identifier_maximum_is_refused_with_the_constraint_clause() {
    let emulator = Emulator::new();
    emulator.put_record("R1", "2024-01-01T00:00:00Z").await;

    /// One `BatchGetRecordIdentifier` naming the one feature group.
    fn identifier(index: usize) -> Value {
        json!({
            "FeatureGroupName": GROUP,
            "RecordIdentifiersValueAsString": [format!("id-{index}")]
        })
    }

    // The class the rule spans: the batch's size, on both sides of the modelled bound.
    // The bound itself and one past it are the two rows that decide the constraint, and
    // the larger ones show the clause does not change with how far past it the request
    // is.
    //
    // (case, how many identifiers the batch carries, expected status)
    let rows: [(&str, usize, u16); 5] = [
        ("one identifier", 1, 200),
        ("the modelled maximum", MAX_IDENTIFIERS, 200),
        ("one identifier past the maximum", MAX_IDENTIFIERS + 1, 400),
        ("fifty past the maximum", MAX_IDENTIFIERS + 50, 400),
        ("ten times the maximum", MAX_IDENTIFIERS * 10, 400),
    ];

    // The clause AWS states for this violation, built from the request: the member path
    // the walk reports the list at and the constraint the model declares on it.
    let clause = format!(
        "at 'identifiers' failed to satisfy constraint: Member must have length less than or \
         equal to {MAX_IDENTIFIERS}"
    );

    for (case, count, expected_status) in rows {
        let identifiers: Vec<Value> = (0..count).map(identifier).collect();
        let (status, body) = emulator
            .send(
                "POST",
                "/BatchGetRecord",
                Some(json!({ "Identifiers": identifiers })),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {count} identifiers, got \
             {status} with body {body:?}"
        );
        if expected_status == 200 {
            // A batch within the bound is served, so the refusal above is the bound and
            // nothing else.
            assert!(
                body.get("Records").and_then(Value::as_array).is_some(),
                "case {case:?}: expected the batch to be served, got {body:?}"
            );
            continue;
        }
        let code = error_code(&body);
        let text = message(&body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the constraint-refusal error code, got {code:?} with \
             message {text:?}"
        );
        assert!(
            text.starts_with("1 validation error detected: Value '"),
            "case {case:?}: expected the AWS constraint-violation envelope stating one \
             violation and echoing the value, got {text:?}"
        );
        assert!(
            text.ends_with(&clause),
            "case {case:?}: expected the clause to end {clause:?}, got {text:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_at_or_after_the_records_event_time_deletes_and_an_earlier_one_does_not() {
    // The class the rule spans: where the deletion's `EventTime` falls relative to the
    // record's, in both accepted spellings and both deletion modes. The record is
    // written at one instant; each row deletes at another.
    let stored_iso = "2026-10-23T18:20:13Z";
    let stored_secs = 1_792_779_613.0_f64;

    // (case, the EventTime the delete carries)
    let earlier: Vec<(&str, String)> = vec![
        ("much earlier, as epoch seconds", format!("{}", stored_secs - 500.0)),
        ("one second earlier", format!("{}", stored_secs - 1.0)),
        ("earlier, as ISO-8601", "2020-01-01T00:00:00Z".to_owned()),
    ];
    let same_or_after: Vec<(&str, String)> = vec![
        ("exactly the record's, as ISO-8601", stored_iso.to_owned()),
        (
            "exactly the record's, as epoch seconds",
            format!("{stored_secs}"),
        ),
        ("one second after", format!("{}", stored_secs + 1.0)),
        ("much later, as ISO-8601", "2030-01-01T00:00:00Z".to_owned()),
    ];

    // The rule as this service words it: a message naming no value from the request,
    // because the rule names none.
    let expected_message =
        "Validation Error: Delete event time must be same or after latest record event time.";

    for (mode, hard) in [("", false), ("&DeletionMode=SoftDelete", false), ("&DeletionMode=HardDelete", true)] {
        for (case, event_time) in &earlier {
            let emulator = Emulator::new();
            emulator.put_record("R1", stored_iso).await;
            let (status, body) = emulator
                .send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString=R1\
                         &EventTime={event_time}{mode}"
                    ),
                    None,
                )
                .await;
            let case = format!("{case} ({})", if hard { "HardDelete" } else { "SoftDelete" });
            if hard {
                let code = error_code(&body);
                let text = message(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationError"),
                    "case {case:?}: expected HTTP 400 ValidationError for a HardDelete \
                     before the record's event time, got {status} {code:?} with message \
                     {text:?}"
                );
                assert_eq!(
                    text, expected_message,
                    "case {case:?}: expected the refusal to state the rule, got {text:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a SoftDelete before the record's event time to \
                     answer 200 and not delete, got {status} with body {body:?}"
                );
            }
            assert!(
                emulator.record_is_served("R1").await,
                "case {case:?}: expected the record to be left standing"
            );
        }

        for (case, event_time) in &same_or_after {
            let emulator = Emulator::new();
            emulator.put_record("R1", stored_iso).await;
            let (status, body) = emulator
                .send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString=R1\
                         &EventTime={event_time}{mode}"
                    ),
                    None,
                )
                .await;
            let case = format!("{case} ({})", if hard { "HardDelete" } else { "SoftDelete" });
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a delete at or after the record's \
                 event time, got {status} with body {body:?}"
            );
            assert!(
                !emulator.record_is_served("R1").await,
                "case {case:?}: expected the record to be deleted"
            );
        }
    }
}

/// An event time well in the future, so a record stored under a TTL is never already
/// expired when it is read back.
const FUTURE_EVENT_TIME: &str = "2035-01-01T00:00:00Z";

/// The `TtlDuration` the writes below state, so a stored record reports an `ExpiresAt`
/// (`ExpiresAt = EventTime + TtlDuration`) a later read can compare.
fn ttl() -> Value {
    json!({ "Unit": "Weeks", "Value": 52 })
}

/// The refusal the group's RECORD-IDENTIFIER feature's rule states.
fn identifier_refusal() -> String {
    format!(
        "the record identifier feature '{IDENTIFIER_FEATURE}' must carry its value in \
         ValueAsString"
    )
}

/// The refusal the group's EVENT-TIME feature's rule states, which every writer of this
/// data plane answers.
fn event_time_refusal() -> String {
    format!(
        "EventTime: the event time feature '{EVENT_TIME_FEATURE}' must carry its value in \
         ValueAsString"
    )
}

/// Assert a single-record writer refused the request with the modelled `ValidationError`
/// (400) stating `expected_message`.
fn assert_refused(case: &str, status: u16, body: &Value, expected_message: &str) {
    let code = error_code(body);
    let text = message(body);
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationError"),
        "case {case:?}: expected HTTP 400 ValidationError, got {status} {code:?} with \
         message {text:?}"
    );
    assert_eq!(
        text, expected_message,
        "case {case:?}: expected the refusal to state the rule, got {text:?}"
    );
}

#[tokio::test]
async fn a_key_feature_without_one_readable_value_is_refused_by_every_whole_record_write() {
    // The class the rule spans: WHICH of the two features the group's definition fixes
    // carries the unreadable value, and WHICH form that value arrives in.
    let keys: [(&str, String); 2] = [
        (IDENTIFIER_FEATURE, identifier_refusal()),
        (EVENT_TIME_FEATURE, event_time_refusal()),
    ];

    for (key_feature, expected_message) in keys {
        for (case, posted) in unreadable_forms(key_feature) {
            // The posted record carries both key features and one ordinary feature; the
            // row's key feature is the one in the form under test.
            let record = json!([
                if key_feature == IDENTIFIER_FEATURE {
                    posted.clone()
                } else {
                    feature(IDENTIFIER_FEATURE, "R1")
                },
                if key_feature == EVENT_TIME_FEATURE {
                    posted.clone()
                } else {
                    feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME)
                },
                feature("feature_a", "written")
            ]);

            let emulator = Emulator::new();
            let (status, body) = emulator
                .put(json!({ "Record": record, "TtlDuration": ttl() }))
                .await;
            assert_refused(&case, status, &body, &expected_message);
            assert!(
                !emulator.record_is_served("R1").await,
                "case {case:?}: expected the refused write to store no record"
            );

            // The SAME record as the one entry of a batch: the entry's own failure,
            // under the error code and the message the single-record writer refuses it
            // with.
            let (status, body) = emulator.batch_write(&record, &ttl()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected BatchWriteRecord to answer 200 and report the \
                 entry, got {status} with body {body:?}"
            );
            let errors = reported_errors(&body);
            assert_eq!(
                errors,
                vec![("ValidationError".to_owned(), expected_message.clone())],
                "case {case:?}: expected the batch to report that entry's own failure \
                 stating the rule, got {errors:?}"
            );
            assert!(
                !emulator.record_is_served("R1").await,
                "case {case:?}: expected the reported entry to store no record"
            );
        }
    }
}

#[tokio::test]
async fn a_record_whose_key_features_carry_one_value_is_stored_whatever_form_the_others_take() {
    // The rows differ in the form the record's ORDINARY features take: the refusal is
    // keyed to the two features the group's definition fixes — which are features no
    // `CollectionType` applies to — not to the value member a request happens to use.
    let rows: [(&str, Value); 3] = [
        ("an ordinary feature as a scalar", feature("tags", "gold")),
        (
            "an ordinary feature as a list of three",
            feature_list("tags", &["gold", "beta", "eu"]),
        ),
        (
            "an ordinary feature with neither value member",
            feature_without_value("tags"),
        ),
    ];

    for (case, tags) in rows {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .put(json!({
                "Record": [
                    feature(IDENTIFIER_FEATURE, "R1"),
                    feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME),
                    tags
                ],
                "TtlDuration": ttl()
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write to be stored, got {status} with body \
             {body:?}"
        );

        let (status, body) = emulator.get("R1").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stored record to be served, got {status} with \
             body {body:?}"
        );
        assert_eq!(
            served_value(&body, EVENT_TIME_FEATURE),
            json!(FUTURE_EVENT_TIME),
            "case {case:?}: expected the record to keep the event time it was written \
             with, got {body:?}"
        );
        assert!(
            expires_at(&body).is_some(),
            "case {case:?}: expected the record to carry the ExpiresAt its event time and \
             TtlDuration state, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_event_time_without_one_readable_value_leaves_the_record_and_its_expiry_standing() {
    let expected_message = event_time_refusal();

    // The class the rule spans: the form the event-time feature's value arrives in, and
    // WHICH writer carries it. Every row starts from the same stored record.
    for (form, posted) in unreadable_forms(EVENT_TIME_FEATURE) {
        for writer in ["PutRecord", "BatchWriteRecord", "UpdateRecord"] {
            let case = format!("{form} through {writer}");
            let emulator = Emulator::new();

            // The record already stored: a future event time, under a TTL, so the read
            // below reports the `ExpiresAt` derived from it.
            let (status, body) = emulator
                .put(json!({
                    "Record": [
                        feature(IDENTIFIER_FEATURE, "R1"),
                        feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME),
                        feature("feature_a", "original")
                    ],
                    "TtlDuration": ttl()
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the seed write to be stored, got {status} with \
                 body {body:?}"
            );
            let (_, seeded) = emulator.get("R1").await;
            let seeded_expiry = expires_at(&seeded);
            assert!(
                seeded_expiry.is_some(),
                "case {case:?}: expected the seeded record to carry an ExpiresAt, got \
                 {seeded:?}"
            );

            // The second write states the group's event-time feature in the form under
            // test and a new value for the record's one ordinary feature.
            let record = json!([
                feature(IDENTIFIER_FEATURE, "R1"),
                posted.clone(),
                feature("feature_a", "replacement")
            ]);
            match writer {
                "PutRecord" => {
                    let (status, body) = emulator
                        .put(json!({ "Record": record, "TtlDuration": ttl() }))
                        .await;
                    assert_refused(&case, status, &body, &expected_message);
                }
                "BatchWriteRecord" => {
                    let (status, body) = emulator.batch_write(&record, &ttl()).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected BatchWriteRecord to answer 200 and report \
                         the entry, got {status} with body {body:?}"
                    );
                    let errors = reported_errors(&body);
                    assert_eq!(
                        errors,
                        vec![("ValidationError".to_owned(), expected_message.clone())],
                        "case {case:?}: expected the batch to report that entry's own \
                         failure stating the rule, got {errors:?}"
                    );
                }
                _ => {
                    let (status, body) = emulator
                        .update(json!({
                            "RecordIdentifierValueAsString": "R1",
                            "Features": [posted.clone(), feature("feature_a", "replacement")]
                        }))
                        .await;
                    assert_refused(&case, status, &body, &expected_message);
                }
            }

            // Nothing of the refused write reached the store: the record, the event time
            // it was written with and the expiry derived from that event time all stand.
            let (status, body) = emulator.get("R1").await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the stored record to still be served, got \
                 {status} with body {body:?}"
            );
            assert_eq!(
                served_value(&body, "feature_a"),
                json!("original"),
                "case {case:?}: expected the stored record's values to stand, got {body:?}"
            );
            assert_eq!(
                served_value(&body, EVENT_TIME_FEATURE),
                json!(FUTURE_EVENT_TIME),
                "case {case:?}: expected the stored record to keep its event time, got \
                 {body:?}"
            );
            assert_eq!(
                expires_at(&body),
                seeded_expiry,
                "case {case:?}: expected the stored record to keep the expiry its TTL \
                 states, got {body:?}"
            );
        }
    }
}

/// How a partial update states a posted record-identifier feature whose value IS one this
/// plane can read but is not the identifier of the record being updated.
///
/// A different verdict from [`identifier_refusal`] about a different request: this one is
/// about a value that disagrees, so it is answered only where the request states a value to
/// disagree with — never for a request that states none the plane can read, which is what
/// the rows below separate.
fn identifier_mismatch_refusal(identifier: &str) -> String {
    format!(
        "Features: the record identifier feature '{IDENTIFIER_FEATURE}' must carry the \
         RecordIdentifierValueAsString of the record being updated ('{identifier}')"
    )
}

#[tokio::test]
async fn a_partial_update_refuses_an_unreadable_record_identifier_with_the_shared_statement() {
    // The PARTIAL update reads the group's record-identifier feature the way every write of
    // this data plane reads it, so the two verdicts it can answer for a posted identifier
    // feature are the two the rule states: the shared refusal for a request that states no
    // value this plane can read as ONE value, and the mismatch verdict for a readable value
    // that is not the record's. The rows differ along what the rule generalises over: which
    // value member the request uses, how many values it carries there, and whether the value
    // it does state is the record's own.
    //
    // (case, the posted record-identifier `FeatureValue`, the refusal expected — `None` when
    // the update is served)
    let rows: [(&str, Value, Option<String>); 7] = [
        (
            "the record's identifier in ValueAsString",
            feature(IDENTIFIER_FEATURE, "R1"),
            None,
        ),
        (
            "the record's identifier in both value members",
            json!({
                "FeatureName": IDENTIFIER_FEATURE,
                "ValueAsString": "R1",
                "ValueAsStringList": ["R1"]
            }),
            None,
        ),
        (
            "the record's identifier as a list of one",
            feature_list(IDENTIFIER_FEATURE, &["R1"]),
            Some(identifier_refusal()),
        ),
        (
            "the record's identifier among a list of three",
            feature_list(IDENTIFIER_FEATURE, &["R1", "R2", "R3"]),
            Some(identifier_refusal()),
        ),
        (
            "the empty list",
            feature_list(IDENTIFIER_FEATURE, &[]),
            Some(identifier_refusal()),
        ),
        (
            "neither value member",
            feature_without_value(IDENTIFIER_FEATURE),
            Some(identifier_refusal()),
        ),
        (
            "another record's identifier in ValueAsString",
            feature(IDENTIFIER_FEATURE, "R2"),
            Some(identifier_mismatch_refusal("R1")),
        ),
    ];

    for (case, posted, expected_refusal) in rows {
        let emulator = Emulator::new();

        // `UpdateRecord` is a PARTIAL update, so the record has to exist first: it is
        // written with a future event time under a TTL, so the read below reports the
        // `ExpiresAt` derived from them.
        let (status, body) = emulator
            .put(json!({
                "Record": [
                    feature(IDENTIFIER_FEATURE, "R1"),
                    feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME),
                    feature("feature_a", "original")
                ],
                "TtlDuration": ttl()
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seed write to be stored, got {status} with body \
             {body:?}"
        );
        let (_, seeded) = emulator.get("R1").await;
        let seeded_expiry = expires_at(&seeded);
        assert!(
            seeded_expiry.is_some(),
            "case {case:?}: expected the seeded record to carry an ExpiresAt, got {seeded:?}"
        );

        // The update posts the identifier feature in the form under test beside a new value
        // for the record's one ordinary feature.
        let (status, body) = emulator
            .update(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": [posted.clone(), feature("feature_a", "replacement")]
            }))
            .await;

        let (status_after, served) = emulator.get("R1").await;
        assert_eq!(
            status_after, 200,
            "case {case:?}: expected the record to still be served, got {status_after} with \
             body {served:?}"
        );

        match expected_refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to be served, got {status} with body \
                     {body:?}"
                );
                assert_eq!(
                    served_value(&served, "feature_a"),
                    json!("replacement"),
                    "case {case:?}: expected the posted value to be merged, got {served:?}"
                );
            }
            Some(expected_message) => {
                assert_refused(case, status, &body, &expected_message);
                // A refused update merges nothing: the record's values, its event time and
                // the expiry derived from that event time all stand.
                assert_eq!(
                    served_value(&served, "feature_a"),
                    json!("original"),
                    "case {case:?}: expected the refused update to leave the record's values \
                     standing, got {served:?}"
                );
                assert_eq!(
                    served_value(&served, EVENT_TIME_FEATURE),
                    json!(FUTURE_EVENT_TIME),
                    "case {case:?}: expected the refused update to leave the record's event \
                     time standing, got {served:?}"
                );
                assert_eq!(
                    expires_at(&served),
                    seeded_expiry,
                    "case {case:?}: expected the refused update to leave the record's expiry \
                     standing, got {served:?}"
                );
            }
        }

        // The identifier the record is stored under is never what an update changes.
        assert_eq!(
            served_value(&served, IDENTIFIER_FEATURE),
            json!("R1"),
            "case {case:?}: expected the record to keep its identifier, got {served:?}"
        );
    }
}

#[tokio::test]
async fn the_partial_update_and_the_whole_record_writes_state_one_rule_for_each_key_feature() {
    // One rule, three writers, both features a feature group's definition fixes: a request
    // that names a key feature without one readable value is refused with the statement for
    // THAT feature, whichever writer carries it. The rows differ along the two dimensions
    // the rule spans — which key feature, and which writer — on one form of the value;
    // `a_key_feature_without_one_readable_value_is_refused_by_every_whole_record_write` and
    // `an_event_time_without_one_readable_value_leaves_the_record_and_its_expiry_standing`
    // walk the forms.
    let keys: [(&str, String); 2] = [
        (IDENTIFIER_FEATURE, identifier_refusal()),
        (EVENT_TIME_FEATURE, event_time_refusal()),
    ];

    for (key_feature, expected_message) in keys {
        let posted = feature_list(key_feature, &["V1"]);
        let other_key = if key_feature == IDENTIFIER_FEATURE {
            feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME)
        } else {
            feature(IDENTIFIER_FEATURE, "R1")
        };

        for writer in ["PutRecord", "BatchWriteRecord", "UpdateRecord"] {
            let case = format!("'{key_feature}' as a list of one through {writer}");
            let emulator = Emulator::new();
            let record = json!([posted.clone(), other_key.clone(), feature("feature_a", "v")]);

            match writer {
                "PutRecord" => {
                    let (status, body) = emulator
                        .put(json!({ "Record": record, "TtlDuration": ttl() }))
                        .await;
                    assert_refused(&case, status, &body, &expected_message);
                }
                "BatchWriteRecord" => {
                    let (status, body) = emulator.batch_write(&record, &ttl()).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected BatchWriteRecord to answer 200 and report \
                         the entry, got {status} with body {body:?}"
                    );
                    let errors = reported_errors(&body);
                    assert_eq!(
                        errors,
                        vec![("ValidationError".to_owned(), expected_message.clone())],
                        "case {case:?}: expected the batch to report that entry's own failure \
                         stating the rule, got {errors:?}"
                    );
                }
                _ => {
                    // The partial update needs the record to exist, and refuses the posted
                    // value before it looks for it either way.
                    let (status, body) = emulator
                        .put(json!({
                            "Record": [
                                feature(IDENTIFIER_FEATURE, "R1"),
                                feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME),
                                feature("feature_a", "original")
                            ],
                            "TtlDuration": ttl()
                        }))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected the seed write to be stored, got {status} \
                         with body {body:?}"
                    );
                    let (status, body) = emulator
                        .update(json!({
                            "RecordIdentifierValueAsString": "R1",
                            "Features": [posted.clone(), feature("feature_a", "v")]
                        }))
                        .await;
                    assert_refused(&case, status, &body, &expected_message);
                }
            }
        }
    }
}

/// How many entries a `BatchWriteRecord` response reports as not processed.
fn unprocessed_count(body: &Value) -> usize {
    body.get("UnprocessedEntries")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn a_whole_record_write_past_the_stores_per_record_feature_bound_is_refused() {
    use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::state::MAX_FEATURES_PER_RECORD;

    /// A record of `count` feature values: the group's two key features and ordinary ones
    /// for the rest.
    fn record_of(count: usize) -> Value {
        let mut features = vec![
            feature(IDENTIFIER_FEATURE, "R1"),
            feature(EVENT_TIME_FEATURE, FUTURE_EVENT_TIME),
        ];
        for index in features.len()..count {
            features.push(feature(&format!("f{index}"), "v"));
        }
        Value::Array(features)
    }

    // The class the rule spans: how many feature values one record carries, on both sides
    // of the store's per-record bound — the bound itself and one past it are the two rows
    // that decide it. A write the bounded store has no room for is not the caller's
    // mistake, so it is the modelled `ServiceUnavailable` (503) rather than a 400.
    //
    // (case, how many feature values the record carries, expected status)
    let rows: [(&str, usize, u16); 3] = [
        ("one feature value under the bound", MAX_FEATURES_PER_RECORD - 1, 200),
        ("exactly the bound", MAX_FEATURES_PER_RECORD, 200),
        ("one feature value past the bound", MAX_FEATURES_PER_RECORD + 1, 503),
    ];

    for (case, count, expected_status) in rows {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .put(json!({ "Record": record_of(count), "TtlDuration": ttl() }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from PutRecord for a record of \
             {count} feature values, got {status} with body {body:?}"
        );
        if expected_status == 200 {
            assert!(
                emulator.record_is_served("R1").await,
                "case {case:?}: expected the record to be stored"
            );
        } else {
            let code = error_code(&body);
            assert_eq!(
                code, "ServiceUnavailable",
                "case {case:?}: expected the store's capacity refusal, not the caller's own \
                 mistake, got {code:?} with message {:?}",
                message(&body)
            );
            assert!(
                !emulator.record_is_served("R1").await,
                "case {case:?}: expected the refused write to store no record"
            );
        }

        // The same record as the one entry of a batch: a write this bounded store has no
        // room for is reported as an entry that was NOT PROCESSED — which the model
        // documents as one that can be retried — rather than as that entry's own error.
        let emulator = Emulator::new();
        let (status, body) = emulator.batch_write(&record_of(count), &ttl()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected BatchWriteRecord to answer 200, got {status} with body \
             {body:?}"
        );
        let errors = reported_errors(&body);
        assert!(
            errors.is_empty(),
            "case {case:?}: expected no entry error for a well-formed record, got {errors:?}"
        );
        let unprocessed = unprocessed_count(&body);
        let expected_unprocessed = usize::from(expected_status != 200);
        assert_eq!(
            unprocessed, expected_unprocessed,
            "case {case:?}: expected {expected_unprocessed} unprocessed entr(y|ies), got \
             {unprocessed} with body {body:?}"
        );
        assert_eq!(
            emulator.record_is_served("R1").await,
            expected_status == 200,
            "case {case:?}: expected the batch entry to be stored only within the bound"
        );
    }
}

#[tokio::test]
async fn an_event_time_of_any_magnitude_is_answered_and_its_expiry_is_reported_only_if_statable() {
    // The epoch-seconds window an ISO-8601 `ExpiresAt` states, as the four-digit year of the
    // timestamp format bounds it: `0001-01-01T00:00:00Z` to `9999-12-31T23:59:59Z`. An
    // event-time feature carries "seconds since the epoch" with no magnitude of its own, and
    // `ExpiresAt = EventTime + TtlDuration` is derived from whatever it states, so the rows
    // walk both published bounds, one past each, and magnitudes far outside the window.
    const EARLIEST: f64 = -62_135_596_800.0;
    const LATEST: f64 = 253_402_300_799.0;

    // (case, the event time the record states)
    let rows: [(&str, String); 8] = [
        ("an ordinary future instant", FUTURE_EVENT_TIME.to_owned()),
        ("one hour inside the latest statable instant", format!("{}", LATEST - 3600.0)),
        ("the latest statable instant", format!("{LATEST}")),
        ("one hour past the latest statable instant", format!("{}", LATEST + 3600.0)),
        ("the earliest statable instant", format!("{EARLIEST}")),
        ("one hour before the earliest statable instant", format!("{}", EARLIEST - 3600.0)),
        ("far above every statable instant", "1e300".to_owned()),
        ("far below every statable instant", "-1e300".to_owned()),
    ];

    for (case, event_time) in rows {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .put(json!({
                "Record": [
                    feature(IDENTIFIER_FEATURE, "R1"),
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("feature_a", "written")
                ],
                "TtlDuration": ttl()
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write of the event time {event_time:?} to be \
             answered 200, got {status} with body {body:?}"
        );

        // The read answers too, and the expiry it reports is one an ISO-8601 string states:
        // an instant outside the window has no `ExpiresAt` spelling, so none is reported.
        let (status, body) = emulator.get("R1").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRecord to be answered 200, got {status} with body \
             {body:?}"
        );
        if let Some(at) = expires_at(&body) {
            let parsed = DateTime::from_str(&at, Format::DateTime);
            assert!(
                parsed.is_ok(),
                "case {case:?}: expected a reported ExpiresAt to be an ISO-8601 timestamp, \
                 got {at:?}"
            );
            let secs = parsed.map(|at| at.as_secs_f64()).unwrap_or_default();
            assert!(
                (EARLIEST..=LATEST).contains(&secs),
                "case {case:?}: expected a reported ExpiresAt inside the statable window, \
                 got {at:?} ({secs} s)"
            );
        }
    }
}
