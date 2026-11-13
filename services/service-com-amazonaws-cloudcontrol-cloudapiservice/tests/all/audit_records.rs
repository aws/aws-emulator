// SPDX-License-Identifier: CC0-1.0

//! The record a state-changing call leaves behind.
//!
//! A resource operation is an administrative action on an account's resources, so each
//! call that starts one emits one structured record naming the operation, the resource,
//! the request it started and the credential presented for it — and nothing the caller
//! may have put a secret in.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::emulator::{progress, text, Emulator};

/// A resource type whose primary identifier the caller supplies and whose resources live
/// in this service's own store.
const TABLE: &str = "AWS::DynamoDB::Table";

/// One captured `tracing` event: its target and its fields as text.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// One field of the record, or `""` when it carries none.
    fn field(&self, name: &str) -> &str {
        self.fields.get(name).map(String::as_str).unwrap_or_default()
    }
}

/// Collects an event's fields. A `&str` field arrives through `record_str`; a
/// `%`-formatted one (`region`, `access_key_id`) through `record_debug` over
/// `format_args!`, which renders as the plain value.
#[derive(Default)]
struct FieldCollector(BTreeMap<String, String>);

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A subscriber that keeps every event it is handed, so this suite can assert on the
/// audit records the service emits. Hand-written over `tracing` itself, which this crate
/// already depends on, so the suite adds no dependency.
struct CapturingSubscriber {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        if let Ok(mut events) = self.events.lock() {
            events.push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields: fields.0,
            });
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The sink the capturing subscriber writes into: every event this test binary emits,
/// from any suite.
static CAPTURED_EVENTS: std::sync::OnceLock<Arc<Mutex<Vec<CapturedEvent>>>> =
    std::sync::OnceLock::new();

/// Install the capturing subscriber as this process's default, once.
fn install_the_audit_capture() -> Arc<Mutex<Vec<CapturedEvent>>> {
    let events = Arc::clone(CAPTURED_EVENTS.get_or_init(|| Arc::new(Mutex::new(Vec::new()))));
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let subscriber = CapturingSubscriber {
            events: Arc::clone(&events),
        };
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
    events
}

/// Every audit record captured so far. Suites run in parallel into the same sink, which
/// is why the assertions below match each record by the resource it names.
fn audit_records() -> Vec<CapturedEvent> {
    let events = install_the_audit_capture();
    let captured = events.lock().map(|events| events.clone()).unwrap_or_default();
    captured
        .into_iter()
        .filter(|event| {
            event.target == service_com_amazonaws_cloudcontrol_cloudapiservice::state::AUDIT_TARGET
        })
        .collect()
}

#[tokio::test]
async fn every_state_changing_call_leaves_one_record_and_no_resource_model() {
    install_the_audit_capture();
    let emulator = Emulator::new();
    // A property a caller could have put a secret in, so the assertions below can check
    // it reaches no record.
    let secret = "s3cret-value-not-for-a-log";
    let desired = json!({ "TableName": "audited-table", "Secret": secret });

    let created = emulator.create_ok(TABLE, &desired, Some("audit-tok")).await;
    let create_token = text(&created, "RequestToken");
    let replayed = emulator.create_ok(TABLE, &desired, Some("audit-tok")).await;
    assert_eq!(
        text(&replayed, "RequestToken"),
        create_token,
        "the retry must replay the first request, got {replayed:?}"
    );
    emulator.settled(&create_token).await;

    let patch = json!([{ "op": "replace", "path": "/Secret", "value": "another-secret" }]);
    let (status, body) = emulator
        .update(TABLE, "audited-table", &patch, None)
        .await;
    assert_eq!(
        status, 200,
        "the update: expected HTTP 200, got {status} with body {body:?}"
    );
    let update_token = text(&progress(&body), "RequestToken");
    emulator.settled(&update_token).await;

    let (status, body) = emulator.delete(TABLE, "audited-table").await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body:?}"
    );
    let delete_token = text(&progress(&body), "RequestToken");

    // A reporting call changes nothing, so it leaves no record of a change.
    emulator.list(TABLE, None, None).await;

    let records = audit_records();
    let rows = [
        ("the create", "CreateResource", create_token, "accepted"),
        (
            "the replayed create",
            "CreateResource",
            text(&replayed, "RequestToken"),
            "replayed",
        ),
        ("the update", "UpdateResource", update_token, "accepted"),
        ("the delete", "DeleteResource", delete_token, "accepted"),
    ];
    for (case, operation, request_token, outcome) in rows {
        let matching: Vec<&CapturedEvent> = records
            .iter()
            .filter(|record| {
                record.field("operation") == operation
                    && record.field("request_token") == request_token
                    && record.field("outcome") == outcome
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one record for {operation} on request \
             {request_token:?} with outcome {outcome:?}, got {matching:?}"
        );
        for record in matching {
            assert_eq!(
                record.field("resource"),
                "AWS::DynamoDB::Table/audited-table",
                "case {case:?}: the record must name the resource the call acted on, got \
                 {record:?}"
            );
            assert_eq!(
                record.field("region"),
                "us-east-1",
                "case {case:?}: the record must name the region the call was signed for, got \
                 {record:?}"
            );
            assert_eq!(
                record.field("access_key_id"),
                "AKIDEXAMPLE",
                "case {case:?}: the record must name the credential presented, got {record:?}"
            );
        }
    }

    // No record carries the resource model, the patch document or the signature: a
    // record names identifiers only.
    for record in &records {
        let rendered = format!("{record:?}");
        for withheld in [secret, "another-secret", "Signature", "AWS4-HMAC-SHA256"] {
            assert!(
                !rendered.contains(withheld),
                "a record must not carry {withheld:?}, got {rendered}"
            );
        }
    }

    // And a reporting call leaves none: `ListResources` names no request token at all.
    let reported: Vec<&CapturedEvent> = records
        .iter()
        .filter(|record| record.field("operation") == "ListResources")
        .collect();
    assert!(
        reported.is_empty(),
        "a reporting call must leave no state-change record, got {reported:?}"
    );
}

#[tokio::test]
async fn a_refused_call_leaves_no_record() {
    install_the_audit_capture();
    let emulator = Emulator::new();

    // Nothing is created, so each of these is refused before it changes anything.
    let (status, _) = emulator.get(TABLE, "refused-table").await;
    assert_eq!(status, 404, "the read of an absent resource must refuse");
    let (status, _) = emulator
        .update(
            TABLE,
            "refused-table",
            &json!([{ "op": "add", "path": "/Tags", "value": [] }]),
            None,
        )
        .await;
    assert_eq!(status, 404, "the patch of an absent resource must refuse");
    let (status, _) = emulator
        .create(TABLE, &json!({ "TableName": {} }), None)
        .await;
    assert_eq!(status, 400, "a desired state that names no identifier must refuse");

    let refused: Vec<CapturedEvent> = audit_records()
        .into_iter()
        .filter(|record| record.field("resource").ends_with("/refused-table"))
        .collect();
    assert!(
        refused.is_empty(),
        "a refused call changed nothing, so it must leave no state-change record, got \
         {refused:?}"
    );
}
