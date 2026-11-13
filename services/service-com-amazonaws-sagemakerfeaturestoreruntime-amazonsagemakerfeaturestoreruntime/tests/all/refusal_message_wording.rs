// SPDX-License-Identifier: CC0-1.0

//! The WORDING of this data plane's refusals: the message a client reads, branches on and
//! logs, for each class of request this service declines.
//!
//! An error message is part of the contract, not prose: a status and an error code say
//! which class a failure is in, and the message says WHICH value, resource or rule the
//! verdict is about. The rules asserted here, one per class:
//!
//! * A `FeatureGroupName` that denotes no feature group is refused with a message naming
//!   the group by the fully qualified ARN the request's name resolves to — the ARN
//!   namespace of this data plane's resources, the Region the request was signed for and
//!   the caller's account — so the refusal says which account and Region the lookup looked
//!   in. One wording, for every operation that names a feature group.
//! * A RECORD that is absent from a feature group that DOES resolve is a different class:
//!   it is named by the `RecordIdentifierValueAsString` the request carried and is
//!   prefixed as a missing record, so the two absences stay distinguishable by message as
//!   well as by code.
//! * `UpdateRecord`'s stale-`EventTime` conflict states the verdict and the fact a retry
//!   path keys on — that the record was not modified — and names no value from the
//!   request, because the verdict is on the ordering alone.
//! * An update made under a TTL (the request's own `TtlDuration`, or the DEFAULT one the
//!   feature group's `OnlineStoreConfig` configures) must carry an `EventTime`, since the
//!   record's expiry is derived from it; the refusal states that rule and nothing of this
//!   request.
//! * A value outside a modelled constraint is refused with a message BUILT FROM THE
//!   REQUEST, and for a `@length` broken by a LIST member the value the clause states is
//!   the list itself, rendered as a list of the modelled structure renders.
//!
//! Every case goes through the real front door — the multi-service router, the restJson1
//! protocol, the modelled constraint validation — so what is asserted is the wire
//! behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group most cases work in, and the two feature names its definition fixes.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The event time the seeded record carries, and its epoch-seconds spelling of the same
/// instant.
const SEEDED_EVENT_TIME: &str = "2026-10-23T18:20:13Z";
const SEEDED_EVENT_SECS: f64 = 1_792_779_613.0;

/// Now, in the epoch-seconds spelling an event-time feature takes, for a seed that must
/// outlive a feature group's default TTL.
fn now_epoch_seconds() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs();
    format!("{now}")
}

/// The account id every ARN this emulator mints carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The Region the suite's requests are signed for unless a row names another.
const REGION: &str = "us-east-1";

/// The modelled maximum of `BatchGetRecordRequest$Identifiers`.
///
/// Source: `smithy_query(query="shape", shape="BatchGetRecordIdentifiers")` on this
/// crate's `smithy-model/` — `length {min: 1, max: 100}`.
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
        Self { engine, router }
    }

    /// Create a feature group the way the SageMaker control plane's `CreateFeatureGroup`
    /// does: by registering its definition — its two key feature names, whether its
    /// `OnlineStore` is on, and the default TTL its `OnlineStoreConfig` configures — in
    /// the ONE shared directory this data plane resolves names against.
    fn create_feature_group(&self, name: &str, default_ttl_seconds: Option<f64>) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:feature-group/{name}"),
                record_identifier_feature_name: IDENTIFIER_FEATURE.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled: true,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds,
            });
    }

    /// Send one request, signed for `region` — the 4th field of the SigV4 credential
    /// scope is this service's signing name, and the 3rd is the Region a resolved ARN
    /// carries.
    async fn send_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header(
                "host",
                format!("featurestore-runtime.sagemaker.{region}.amazonaws.com"),
            )
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

    /// Send one request signed for [`REGION`].
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.send_in(REGION, method, uri, body).await
    }

    /// Ingest one record of [`GROUP`] at `event_time`.
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

    /// `UpdateRecord` against [`GROUP`], with whatever body the row states.
    async fn update_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", &format!("/FeatureGroup/{GROUP}/Record"), Some(body))
            .await
    }

    /// The features `GetRecord` serves for `identifier`, as `(name, value)` pairs, and an
    /// empty list when it serves no record at all.
    async fn record_pairs(&self, identifier: &str) -> Vec<(String, String)> {
        let (_, body) = self
            .send(
                "GET",
                &format!("/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}"),
                None,
            )
            .await;
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
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
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
/// modelled errors declare a `Message` member (`ValidationError$Message`), which restJson1
/// serialises under that name — so both spellings are read here.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The per-element `(ErrorCode, ErrorMessage)` pairs a batch response reports.
fn batch_errors(body: &Value) -> Vec<(String, String)> {
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

/// How this service states a feature group its name resolved to nothing: the group named
/// by the ARN a group of that name would carry in the caller's account and the Region the
/// request was signed for.
fn no_such_feature_group(region: &str, name: &str) -> String {
    format!(
        "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name \
         [arn:aws:sagemaker:{region}:{ACCOUNT}:feature-group/{name}]."
    )
}

#[tokio::test]
async fn every_operation_names_an_unresolvable_feature_group_by_its_resolved_arn() {
    // The class the rule spans: every operation that carries a `FeatureGroupName`,
    // crossed with the spellings the modelled `FeatureGroupNameOrArn` admits and the
    // Region the request is signed for. The group named is never created, so each call
    // reaches the one resolution step.
    //
    // (case, the Region signed for, the group name, the spelling the call carries)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a bare name, in the default Region",
            "us-east-1",
            "ghost",
            "ghost".to_owned(),
        ),
        (
            "a bare name, signed for another Region",
            "eu-west-1",
            "ghost",
            "ghost".to_owned(),
        ),
        (
            "a bare name, signed for a third Region",
            "ap-southeast-2",
            "nosuchgroup",
            "nosuchgroup".to_owned(),
        ),
        // The ARN spelling denotes the same group, and the ARN the refusal states is the
        // one the REQUEST resolves to — the caller's account and the signed Region —
        // never the account or Region the caller happened to spell.
        (
            "an ARN of this account and Region",
            "us-east-1",
            "ghost",
            "arn:aws:sagemaker:us-east-1:000000000000:feature-group/ghost".to_owned(),
        ),
        (
            "an ARN of another account and Region",
            "us-east-1",
            "ghost",
            "arn:aws:sagemaker:eu-west-1:123456789012:feature-group/ghost".to_owned(),
        ),
        (
            "a name of the maximum length a feature group may carry",
            "us-west-2",
            "a0123456789012345678901234567890123456789012345678901234567890b",
            "a0123456789012345678901234567890123456789012345678901234567890b".to_owned(),
        ),
    ];

    for (case, region, name, spelling) in rows {
        let emulator = Emulator::new();
        // One OTHER group exists in every row, so no verdict can come from an empty
        // directory.
        emulator.create_feature_group(GROUP, None);
        let expected = no_such_feature_group(region, name);
        let label = spelling.replace(':', "%3A").replace('/', "%2F");

        // (the operation, its method, its URI, its body) — the single-record calls, which
        // carry the refusal in the modelled `ValidationError`.
        let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "PutRecord",
                "PUT",
                format!("/FeatureGroup/{label}"),
                Some(json!({ "Record": [
                    feature(IDENTIFIER_FEATURE, "R1"),
                    feature(EVENT_TIME_FEATURE, SEEDED_EVENT_TIME)
                ] })),
            ),
            (
                "GetRecord",
                "GET",
                format!("/FeatureGroup/{label}?RecordIdentifierValueAsString=R1"),
                None,
            ),
            (
                "UpdateRecord",
                "POST",
                format!("/FeatureGroup/{label}/Record"),
                Some(json!({
                    "RecordIdentifierValueAsString": "R1",
                    "Features": [feature("feature_a", "updated")]
                })),
            ),
            (
                "ListRecords",
                "POST",
                format!("/FeatureGroup/{label}/ListRecords"),
                Some(json!({})),
            ),
            (
                "DeleteRecord",
                "DELETE",
                format!(
                    "/FeatureGroup/{label}?RecordIdentifierValueAsString=R1\
                     &EventTime={SEEDED_EVENT_TIME}"
                ),
                None,
            ),
        ];
        for (operation, method, uri, body) in calls {
            let (status, answered) = emulator.send_in(region, method, &uri, body).await;
            let code = error_code(&answered);
            let stated = message(&answered);
            assert_eq!(
                (status, code.as_str(), stated.as_str()),
                (400, "ValidationError", expected.as_str()),
                "case {case:?}/{operation}: expected HTTP 400 ValidationError stating \
                 {expected:?}, got {status} {code:?} with {stated:?}"
            );
        }

        // The batch operations report the same absence per element, beside the same code
        // every single-record call answers it with.
        let batches: Vec<(&str, &str, Value)> = vec![
            (
                "BatchGetRecord",
                "/BatchGetRecord",
                json!({ "Identifiers": [{
                    "FeatureGroupName": spelling,
                    "RecordIdentifiersValueAsString": ["R1"]
                }] }),
            ),
            (
                "BatchWriteRecord",
                "/BatchWriteRecord",
                json!({ "Entries": [{
                    "FeatureGroupName": spelling,
                    "Record": [
                        feature(IDENTIFIER_FEATURE, "R1"),
                        feature(EVENT_TIME_FEATURE, SEEDED_EVENT_TIME)
                    ]
                }] }),
            ),
        ];
        for (operation, uri, body) in batches {
            let (status, answered) = emulator.send_in(region, "POST", uri, Some(body)).await;
            let reported = batch_errors(&answered);
            let expected_errors = vec![("ValidationError".to_owned(), expected.clone())];
            assert_eq!(
                (status, reported.clone()),
                (200, expected_errors.clone()),
                "case {case:?}/{operation}: expected HTTP 200 with per-element errors \
                 {expected_errors:?}, got {status} with {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_of_a_record_the_group_does_not_hold_names_the_record_identifier() {
    // The class the rule spans: every way a feature group that RESOLVES can hold no live
    // record under an identifier, and the identifiers a request may carry
    // (`RecordIdentifierValueAsString` is modelled `len 0..358400, pattern .*`).
    //
    // (case, the identifier the update names, whether to write it first and then
    // soft-delete it)
    let rows: [(&str, &str, bool); 5] = [
        ("an identifier never written", "rec-never-ingested", false),
        ("an identifier written and soft-deleted", "rec-gone", true),
        ("an identifier that is one character", "x", false),
        (
            "an identifier carrying characters the pattern admits",
            "tenant/42 rec:7",
            false,
        ),
        (
            "an identifier that differs from a written one by one character",
            "rec-gon",
            false,
        ),
    ];

    for (case, identifier, soft_deleted) in rows {
        let emulator = Emulator::new();
        emulator.create_feature_group(GROUP, None);
        // One record the group DOES hold, so no verdict can come from an empty store.
        emulator.put_record("rec-held", SEEDED_EVENT_TIME).await;
        if soft_deleted {
            emulator.put_record(identifier, SEEDED_EVENT_TIME).await;
            let (status, body) = emulator
                .send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}\
                         &EventTime=2030-01-01T00:00:00Z&DeletionMode=SoftDelete"
                    ),
                    None,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the soft delete to be accepted, got {status} with \
                 body {body:?}"
            );
        }

        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": identifier,
                "Features": [
                    feature(EVENT_TIME_FEATURE, SEEDED_EVENT_TIME),
                    feature("feature_a", "x")
                ]
            }))
            .await;
        let code = error_code(&body);
        let stated = message(&body);
        let expected =
            format!("Record Not Found: Record not found with record identifier [{identifier}].");
        assert_eq!(
            (status, code.as_str(), stated.as_str()),
            (404, "ResourceNotFound", expected.as_str()),
            "case {case:?}: expected HTTP 404 ResourceNotFound stating {expected:?}, got \
             {status} {code:?} with {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_stale_event_time_states_the_conflict_and_leaves_the_record_standing() {
    // The class the rule spans: how far before the record's event time the update's falls,
    // in both spellings a Feature Store event-time feature admits, and whether the stored
    // record got its event time from a `PutRecord` or from an `UpdateRecord` that omitted
    // one. The message is FIXED — it names no identifier, instant or feature group — so
    // every row expects the same text.
    let expected = "Update rejected: the provided EventTime is older than the record's current \
                    EventTime. The record was not modified.";

    // (case, the EventTime the update carries, whether an update with no EventTime runs
    // first)
    let rows: [(&str, String, bool); 6] = [
        (
            "one second earlier, as epoch seconds",
            format!("{}", SEEDED_EVENT_SECS - 1.0),
            false,
        ),
        (
            "a thousand seconds earlier, as epoch seconds",
            format!("{}", SEEDED_EVENT_SECS - 1000.0),
            false,
        ),
        (
            "years earlier, as ISO-8601",
            "2020-01-01T00:00:00Z".to_owned(),
            false,
        ),
        (
            "a fraction of a second earlier",
            format!("{}", SEEDED_EVENT_SECS - 0.5),
            false,
        ),
        // The same verdict after an update that carried no EventTime: "If you omit
        // EventTime, the service keeps the record's existing EventTime", so the stored
        // instant the next update is ordered against is still the seeded one.
        (
            "five seconds earlier, after an update that omitted the EventTime",
            format!("{}", SEEDED_EVENT_SECS - 5.0),
            true,
        ),
        (
            "years earlier, after an update that omitted the EventTime",
            "2019-06-01T00:00:00Z".to_owned(),
            true,
        ),
    ];

    for (case, event_time, update_first) in rows {
        let emulator = Emulator::new();
        emulator.create_feature_group(GROUP, None);
        emulator.put_record("rec-conf", SEEDED_EVENT_TIME).await;
        if update_first {
            let (status, body) = emulator
                .update_record(json!({
                    "RecordIdentifierValueAsString": "rec-conf",
                    "Features": [feature("feature_a", "a2")]
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected an update with no EventTime to be applied, got \
                 {status} with body {body:?}"
            );
        }
        let before = emulator.record_pairs("rec-conf").await;

        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "rec-conf",
                "Features": [
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("feature_b", "should-not-apply")
                ]
            }))
            .await;
        let code = error_code(&body);
        let stated = message(&body);
        assert_eq!(
            (status, code.as_str(), stated.as_str()),
            (409, "ConflictException", expected),
            "case {case:?}: expected HTTP 409 ConflictException stating {expected:?}, got \
             {status} {code:?} with {stated:?}"
        );
        // The message says the record was not modified, so it was not.
        let after = emulator.record_pairs("rec-conf").await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to leave the record as it was \
             ({before:?}), got {after:?}"
        );
    }

    // The control: an EventTime at or after the record's is applied, so the refusal above
    // is the ordering and nothing else.
    for (case, event_time) in [
        ("exactly the record's", SEEDED_EVENT_TIME.to_owned()),
        ("one second after", format!("{}", SEEDED_EVENT_SECS + 1.0)),
    ] {
        let emulator = Emulator::new();
        emulator.create_feature_group(GROUP, None);
        emulator.put_record("rec-conf", SEEDED_EVENT_TIME).await;
        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "rec-conf",
                "Features": [
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("feature_b", "applied")
                ]
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an update at or after the record's event time to be \
             applied, got {status} with body {body:?}"
        );
    }
}

/// One row of the TTL table: the case name, the feature group's default TTL in seconds,
/// the `TtlDuration` the request carries, whether the update carries an `EventTime`, and
/// the status the update must answer.
type TtlRow<'a> = (&'a str, Option<f64>, Option<Value>, bool, u16);

#[tokio::test]
async fn an_update_under_a_ttl_requires_an_event_time_and_states_the_whole_rule() {
    // The rule's fixed text: it names the two ways a TTL comes to be in force and the
    // feature a request carries an event time in, and nothing of this request or this
    // feature group.
    let expected = "Validation Error: EventTime is required when TtlDuration is specified or when \
                    the feature group has a default TTL configured. Include the EventTime feature \
                    in the Features list.";

    // The class the rule spans: whether a TTL is in force through the REQUEST, through the
    // feature group's DEFAULT, through both or through neither, crossed with whether the
    // update carries an EventTime. The TTL units are the model's own `TtlDurationUnit`
    // values (`smithy_query(query="shape", shape="TtlDurationUnit")`: Seconds, Minutes,
    // Hours, Days, Weeks).
    //
    // (case, the group's default TTL in seconds, the request's TtlDuration, whether the
    // update carries an EventTime, expected status)
    let rows: Vec<TtlRow<'_>> = vec![
        (
            "no TTL at all, no EventTime",
            None,
            None,
            false,
            200,
        ),
        (
            "a request TtlDuration in hours, with an EventTime",
            None,
            Some(json!({ "Unit": "Hours", "Value": 1 })),
            true,
            200,
        ),
        (
            "a request TtlDuration in hours, no EventTime",
            None,
            Some(json!({ "Unit": "Hours", "Value": 1 })),
            false,
            400,
        ),
        (
            "a request TtlDuration in weeks, no EventTime",
            None,
            Some(json!({ "Unit": "Weeks", "Value": 2 })),
            false,
            400,
        ),
        (
            "a request TtlDuration in seconds, no EventTime",
            None,
            Some(json!({ "Unit": "Seconds", "Value": 30 })),
            false,
            400,
        ),
        (
            "a default TTL on the feature group, no EventTime",
            Some(3_600.0),
            None,
            false,
            400,
        ),
        (
            "a default TTL on the feature group, with an EventTime",
            Some(3_600.0),
            None,
            true,
            200,
        ),
        (
            "a default TTL and a request TtlDuration, no EventTime",
            Some(604_800.0),
            Some(json!({ "Unit": "Days", "Value": 1 })),
            false,
            400,
        ),
        (
            "a default TTL and a request TtlDuration, with an EventTime",
            Some(604_800.0),
            Some(json!({ "Unit": "Days", "Value": 1 })),
            true,
            200,
        ),
    ];

    for (case, default_ttl, ttl_duration, with_event_time, expected_status) in rows {
        let emulator = Emulator::new();
        emulator.create_feature_group(GROUP, default_ttl);
        // The seeded record's event time is NOW, in the epoch-seconds spelling an
        // event-time feature also takes: a record of a group with a default TTL is
        // ingested under that TTL, so a record seeded at a fixed past instant would
        // already be hard-deleted before the update under test reached it. What each row
        // asserts is the update's own verdict, so the seed only has to leave the record
        // standing.
        let seeded_event_time = now_epoch_seconds();
        emulator.put_record("rec-ttl", &seeded_event_time).await;
        let before = emulator.record_pairs("rec-ttl").await;

        let mut features = vec![feature("feature_a", "applied")];
        if with_event_time {
            features.push(feature(EVENT_TIME_FEATURE, &seeded_event_time));
        }
        let mut body = json!({
            "RecordIdentifierValueAsString": "rec-ttl",
            "Features": features
        });
        if let Some(ttl) = ttl_duration {
            body["TtlDuration"] = ttl;
        }
        let (status, answered) = emulator.update_record(body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body \
             {answered:?}"
        );
        if expected_status == 200 {
            continue;
        }
        let code = error_code(&answered);
        let stated = message(&answered);
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationError", expected),
            "case {case:?}: expected ValidationError stating {expected:?}, got {code:?} with \
             {stated:?}"
        );
        // A refused update changes nothing, so the record is exactly as it was.
        let after = emulator.record_pairs("rec-ttl").await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to leave the record as it was \
             ({before:?}), got {after:?}"
        );
    }
}

/// One `BatchGetRecordIdentifier` a row submits: the feature group it names, the record
/// identifiers it asks for, and the feature names it narrows them to (`None` where the
/// request omits `FeatureNames`).
struct Identifier {
    group: String,
    records: Vec<String>,
    features: Option<Vec<String>>,
}

impl Identifier {
    /// The identifier as the request carries it.
    fn posted(&self) -> Value {
        let mut posted = json!({
            "FeatureGroupName": self.group,
            "RecordIdentifiersValueAsString": self.records
        });
        if let Some(features) = &self.features {
            posted["FeatureNames"] = json!(features);
        }
        posted
    }

    /// The identifier as a constraint clause states it: the modelled structure's name over
    /// every member its shape declares, in declaration order, with a member the request
    /// left unset stated as `null`
    /// (`smithy_query(query="shape", shape="BatchGetRecordIdentifier")` gives
    /// `FeatureGroupName RecordIdentifiersValueAsString FeatureNames`).
    fn rendered(&self) -> String {
        let features = match &self.features {
            Some(features) => format!("[{}]", features.join(", ")),
            None => "null".to_owned(),
        };
        format!(
            "BatchGetRecordIdentifier(featureGroupName={}, recordIdentifiersValueAsString=[{}], \
             featureNames={features})",
            self.group,
            self.records.join(", ")
        )
    }
}

/// `count` identifiers of `group`, each asking for `records_each` record identifiers, and
/// each carrying `FeatureNames` where `features` says so for its position.
fn identifiers(
    group: &str,
    count: usize,
    records_each: usize,
    features: impl Fn(usize) -> Option<Vec<String>>,
) -> Vec<Identifier> {
    (0..count)
        .map(|index| Identifier {
            group: group.to_owned(),
            records: (0..records_each)
                .map(|slot| format!("id-{index}-{slot}"))
                .collect(),
            features: features(index),
        })
        .collect()
}

#[tokio::test]
async fn a_batch_past_the_identifier_maximum_quotes_the_identifiers_it_was_sent() {
    let emulator = Emulator::new();
    emulator.create_feature_group(GROUP, None);

    // The class the rule spans: what the submitted `Identifiers` list HOLDS, since the
    // clause states that list. Rows differ in how many identifiers are past the bound,
    // how many record identifiers each carries, and whether the optional `FeatureNames`
    // is set — on every element, on one, or on none.
    //
    // (case, the identifiers the batch carries)
    let rows: Vec<(&str, Vec<Identifier>)> = vec![
        (
            "one identifier past the maximum, no FeatureNames",
            identifiers("fg", MAX_IDENTIFIERS + 1, 1, |_| None),
        ),
        (
            "one past the maximum, two record identifiers each",
            identifiers("fg", MAX_IDENTIFIERS + 1, 2, |_| None),
        ),
        (
            "one past the maximum, FeatureNames on the first element only",
            identifiers("fg", MAX_IDENTIFIERS + 1, 1, |index| {
                (index == 0).then(|| vec!["feature_a".to_owned()])
            }),
        ),
        (
            "one past the maximum, FeatureNames on the first two elements",
            identifiers("fg", MAX_IDENTIFIERS + 1, 1, |index| {
                (index < 2).then(|| vec!["feature_a".to_owned(), "feature_b".to_owned()])
            }),
        ),
        (
            "five past the maximum",
            identifiers("fg", MAX_IDENTIFIERS + 5, 1, |_| None),
        ),
    ];

    for (case, submitted) in rows {
        let posted: Vec<Value> = submitted.iter().map(Identifier::posted).collect();
        let (status, body) = emulator
            .send(
                "POST",
                "/BatchGetRecord",
                Some(json!({ "Identifiers": posted })),
            )
            .await;
        let code = error_code(&body);
        let stated = message(&body);
        // The clause AWS states for this violation, built from the request: the value the
        // member held, the member path the walk reports it at, and the constraint the
        // model declares on it.
        let rendered: Vec<String> = submitted.iter().map(Identifier::rendered).collect();
        let expected = format!(
            "1 validation error detected: Value '[{}]' at 'identifiers' failed to satisfy \
             constraint: Member must have length less than or equal to {MAX_IDENTIFIERS}",
            rendered.join(", ")
        );
        assert_eq!(
            (status, code.as_str(), stated.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected HTTP 400 ValidationException stating {expected:?}, got \
             {status} {code:?} with {stated:?}"
        );
    }

    // The controls: a batch AT the modelled maximum is served, and one element short of it
    // too, so the refusal above is the bound and nothing else.
    for (case, count) in [("one identifier", 1), ("the modelled maximum", MAX_IDENTIFIERS)] {
        let posted: Vec<Value> = identifiers(GROUP, count, 1, |_| None)
            .iter()
            .map(Identifier::posted)
            .collect();
        let (status, body) = emulator
            .send(
                "POST",
                "/BatchGetRecord",
                Some(json!({ "Identifiers": posted })),
            )
            .await;
        assert!(
            status == 200 && body.get("Records").and_then(Value::as_array).is_some(),
            "case {case:?}: expected the batch to be served, got {status} with body {body:?}"
        );
    }
}
