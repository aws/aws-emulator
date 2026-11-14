// SPDX-License-Identifier: CC0-1.0

//! The audit record every state-changing dashboard operation emits.
//!
//! A create, an update and a delete each have to leave a structured record saying when
//! the call happened, where it came from, which access key id it presented, which
//! operation ran, which resource it touched and how it came out — on the refusal paths
//! too, and never for an operation that only reports. These tests install a capturing
//! `tracing` subscriber scoped to the calling thread, drive the operations through the
//! real router, and assert over the records that come out. Nothing is written to disk
//! and no global subscriber is installed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::AUDIT_TARGET;

use super::dashboard_lifecycle::{call, ok_call, router, PRESENTED_AKID, REQUEST_SOURCE};
use super::scheduled_report_execution::create_report;
use super::scheduled_report_lifecycle::source_dashboard;

/// One captured event: its target and its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a test can read the
/// audit records back.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// The audit records captured so far, oldest first.
    fn audit_records(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| event.target == AUDIT_TARGET)
            .cloned()
            .collect()
    }

    /// Every audit record for `operation`.
    fn records_for(&self, operation: &str) -> Vec<CapturedEvent> {
        self.audit_records()
            .into_iter()
            .filter(|event| event.field("operation") == Some(operation))
            .collect()
    }

    /// The one audit record for `operation`, or an explanation of what was captured.
    fn record_for(&self, operation: &str) -> CapturedEvent {
        let matching = self.records_for(operation);
        assert_eq!(
            matching.len(),
            1,
            "case {operation}: expected exactly one audit record, captured {matching:?}"
        );
        matching.into_iter().next().expect("the one record")
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These tests assert on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields,
            });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events for the life of the returned guard.
fn capture_events() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(capture.clone());
    (capture, guard)
}

/// Assert the shape every audit record shares, whatever the operation and outcome.
fn assert_record(record: &CapturedEvent, operation: &str, resource: &str, outcome: &str) {
    assert_eq!(
        record.field("operation"),
        Some(operation),
        "case {operation}: wrong operation in {record:?}"
    );
    assert_eq!(
        record.field("resource"),
        Some(resource),
        "case {operation}: wrong resource in {record:?}"
    );
    assert_eq!(
        record.field("outcome"),
        Some(outcome),
        "case {operation}: wrong outcome in {record:?}"
    );
    assert_eq!(
        record.field("akid"),
        Some(PRESENTED_AKID),
        "case {operation}: the record must attribute the call to the presented access key id: {record:?}"
    );
    assert_eq!(
        record.field("source"),
        Some(REQUEST_SOURCE),
        "case {operation}: wrong request source in {record:?}"
    );
    let timestamp_ms: u64 = record
        .field("timestamp_ms")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("case {operation}: no timestamp_ms in {record:?}"));
    assert!(
        timestamp_ms > 1_700_000_000_000,
        "case {operation}: timestamp_ms {timestamp_ms} is not a plausible epoch-milliseconds instant"
    );
}

/// Each state-changing operation leaves one audit record naming the resource it touched
/// and a SUCCESS outcome — and the reporting operations leave none.
#[tokio::test]
async fn state_changing_operations_emit_one_audit_record_each() {
    let router = router();
    let (capture, _guard) = capture_events();

    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Audited Dashboard",
            "widgets": [],
            "resourceTags": [{"key": "env", "value": "prod"}],
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    assert_record(
        &capture.record_for("CreateDashboard"),
        "CreateDashboard",
        &arn,
        "SUCCESS",
    );

    ok_call(
        &router,
        "UpdateDashboard",
        &json!({"arn": arn, "name": "Audited Dashboard 2"}),
    )
    .await;
    assert_record(
        &capture.record_for("UpdateDashboard"),
        "UpdateDashboard",
        &arn,
        "SUCCESS",
    );

    // Adding a tag changes the stored record, so it accounts for itself.
    ok_call(
        &router,
        "TagResource",
        &json!({"resourceArn": arn, "resourceTags": [{"key": "team", "value": "finops"}]}),
    )
    .await;
    assert_record(
        &capture.record_for("TagResource"),
        "TagResource",
        &arn,
        "SUCCESS",
    );

    // Removing a tag changes the stored record too, so it accounts for itself.
    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
    )
    .await;
    assert_record(
        &capture.record_for("UntagResource"),
        "UntagResource",
        &arn,
        "SUCCESS",
    );

    // The reporting operations change nothing, so they account for nothing.
    ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    ok_call(&router, "ListDashboards", &json!({})).await;
    ok_call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    for operation in ["GetDashboard", "ListDashboards", "ListTagsForResource"] {
        let leaked = capture.records_for(operation);
        assert!(
            leaked.is_empty(),
            "case {operation}: a reporting operation must emit no audit record, captured {leaked:?}"
        );
    }

    ok_call(&router, "DeleteDashboard", &json!({"arn": arn})).await;
    assert_record(
        &capture.record_for("DeleteDashboard"),
        "DeleteDashboard",
        &arn,
        "SUCCESS",
    );
}

/// A refused state change is audited too, with the modelled error as its outcome: a
/// table over the refusals the state-changing operations can answer. `DeleteDashboard` is
/// not among them for an absent resource: it models no not-found error and answers a 200
/// no-op, audited with its SUCCESS outcome by
/// [`an_idempotent_delete_is_audited_as_a_success`].
#[tokio::test]
async fn refused_state_changes_are_audited_with_their_outcome() {
    let absent = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";

    // (operation, request, expected status, expected audited resource, expected outcome)
    let cases: &[(&str, Value, u16, &str, &str)] = &[
        (
            "UpdateDashboard",
            json!({"arn": absent, "name": "Nothing To Update"}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
        (
            "UntagResource",
            json!({"resourceArn": absent, "resourceTagKeys": ["env"]}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
        (
            "TagResource",
            json!({"resourceArn": absent, "resourceTags": [{"key": "env", "value": "prod"}]}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
    ];

    for (operation, request, expected_status, resource, outcome) in cases {
        let router = router();
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, *expected_status,
            "case {operation} refusal: expected {expected_status}, got {status}: {body}"
        );
        assert_record(&capture.record_for(operation), operation, resource, outcome);
    }
}

/// `DeleteDashboard` answers an ARN no record is stored under with a 200 no-op, so the
/// one audit record it leaves carries the SUCCESS outcome its answer did — for a record
/// that was there and for one that never was.
#[tokio::test]
async fn an_idempotent_delete_is_audited_as_a_success() {
    let absent = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";

    // (label, the arn deleted, whether a dashboard is created under it first)
    let cases: &[(&str, &str, bool)] = &[
        ("an arn no dashboard was ever stored under", absent, false),
        ("the second delete of a dashboard that existed", "", true),
    ];

    for (label, fixed_arn, create_first) in cases {
        let router = router();
        let arn = if *create_first {
            let created = ok_call(
                &router,
                "CreateDashboard",
                &json!({"name": "Audited Twice", "widgets": []}),
            )
            .await;
            let arn = created["arn"].as_str().expect("an arn").to_string();
            ok_call(&router, "DeleteDashboard", &json!({"arn": arn})).await;
            arn
        } else {
            (*fixed_arn).to_string()
        };

        // Capture only the no-op delete, so the record asserted is that call's.
        let (capture, _guard) = capture_events();
        let answered = ok_call(&router, "DeleteDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            answered["arn"].as_str(),
            Some(arn.as_str()),
            "case {label}: the no-op must echo the requested arn: {answered}"
        );
        assert_record(
            &capture.record_for("DeleteDashboard"),
            "DeleteDashboard",
            &arn,
            "SUCCESS",
        );
    }
}

/// CreateDashboard's quota refusal is audited as well, with the modelled
/// `ServiceQuotaExceededException` as its outcome and the sentinel resource the handler
/// uses when no dashboard was reached.
#[tokio::test]
async fn the_quota_refusal_is_audited() {
    use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_DASHBOARDS;

    let router = router();
    // A distinct name per create: a dashboard's name is unique within the account, so the
    // quota is what must refuse the create past the ceiling. The AWS-managed catalog a
    // fresh store already holds counts against that same ceiling, so the account's own
    // creates fill the room it leaves.
    let room = MAX_DASHBOARDS - super::dashboard_lifecycle::managed_dashboard_count();
    for index in 0..room {
        let request = json!({"name": format!("Quota Filler {index:04}"), "widgets": []});
        ok_call(&router, "CreateDashboard", &request).await;
    }

    let (capture, _guard) = capture_events();
    let request = json!({"name": "Quota Filler Overflow", "widgets": []});
    let (status, body) = call(&router, "CreateDashboard", &request).await;
    assert_eq!(
        status, 402,
        "case quota refusal: expected 402, got {status}: {body}"
    );
    assert_record(
        &capture.record_for("CreateDashboard"),
        "CreateDashboard",
        "-",
        "ServiceQuotaExceededException",
    );
}

/// `CreateScheduledReport`'s idempotency paths account for themselves by what they
/// answer: the create leaves one SUCCESS record naming the report it stored, and every
/// later call presenting that token — the same request as much as a different one — leaves
/// its ConflictException record, naming no report because it stored none.
#[tokio::test]
async fn scheduled_report_create_reuses_are_audited_by_what_they_change() {
    use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;

    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "AuditTokenSource").await;
    let request = json!({
        "clientToken": "audited-create-token-0001",
        "scheduledReport": {
            "name": "Audited Create",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
        }
    });

    let arn = {
        let (capture, _guard) = capture_events();
        let created = ok_call(&router, "CreateScheduledReport", &request).await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        assert_record(
            &capture.record_for("CreateScheduledReport"),
            "CreateScheduledReport",
            &arn,
            "SUCCESS",
        );
        arn
    };

    {
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, "CreateScheduledReport", &request).await;
        assert_eq!(
            status, 409,
            "case the identical request again: expected the conflict's 409, got {status}: {body}"
        );
        assert_record(
            &capture.record_for("CreateScheduledReport"),
            "CreateScheduledReport",
            "-",
            "ConflictException",
        );
    }

    {
        let mut different = request.clone();
        different["scheduledReport"]["name"] = json!("Audited Other");
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, "CreateScheduledReport", &different).await;
        assert_eq!(
            status, 409,
            "case token reuse: expected 409, got {status}: {body}"
        );
        assert_record(
            &capture.record_for("CreateScheduledReport"),
            "CreateScheduledReport",
            "-",
            "ConflictException",
        );
    }

    // The audited create is still the only report the token made: neither the replay nor
    // the refused reuse created a second one.
    let listed = ok_call(&router, "ListScheduledReports", &json!({})).await;
    assert_eq!(
        listed["scheduledReports"].as_array().map(Vec::len),
        Some(1),
        "the replay and the refused reuse must have created no second report: {listed}"
    );
    assert_eq!(
        listed["scheduledReports"][0]["arn"].as_str(),
        Some(arn.as_str()),
        "the one stored report is the one the create minted: {listed}"
    );
}

/// `DeleteScheduledReport` accounts for itself the way every state-changing operation of
/// this service does: the delete that removed a report leaves one record naming it with a
/// SUCCESS outcome, and the refusal of an ARN no report is stored under leaves one naming
/// the requested ARN with the modelled error as its outcome — including the repeat of a
/// delete that already succeeded, which this operation refuses rather than replaying.
#[tokio::test]
async fn scheduled_report_deletes_are_audited() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "AuditDeleteSource").await;
    let doomed = create_report(&router, &dashboard_arn, "Audited Doomed", None, None).await;

    {
        let (capture, _guard) = capture_events();
        ok_call(
            &router,
            "DeleteScheduledReport",
            &json!({"arn": doomed}),
        )
        .await;
        assert_record(
            &capture.record_for("DeleteScheduledReport"),
            "DeleteScheduledReport",
            &doomed,
            "SUCCESS",
        );
    }

    let absent = "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";
    for (label, arn) in [
        ("the repeat of a delete that succeeded", doomed.as_str()),
        ("an arn no report was ever stored under", absent),
    ] {
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, "DeleteScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status}: {body}"
        );
        assert_record(
            &capture.record_for("DeleteScheduledReport"),
            "DeleteScheduledReport",
            arn,
            "ResourceNotFoundException",
        );
    }
}

/// `ExecuteScheduledReport` accounts for itself: the execution it triggers and each
/// refusal it answers leave one record naming the report and the outcome, while the two
/// calls that change nothing — the dry run and the idempotent retry — leave none.
#[tokio::test]
async fn scheduled_report_executions_are_audited() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "AuditExecSource").await;
    let enabled = create_report(&router, &dashboard_arn, "Audited Report", None, None).await;
    let other = create_report(&router, &dashboard_arn, "Audited Other", None, None).await;
    let paused = create_report(
        &router,
        &dashboard_arn,
        "Audited Paused",
        Some("DISABLED"),
        None,
    )
    .await;

    let token = "audited-exec-token-0001";
    let triggering = json!({"arn": enabled, "clientToken": token});

    // The triggered execution.
    {
        let (capture, _guard) = capture_events();
        ok_call(&router, "ExecuteScheduledReport", &triggering).await;
        assert_record(
            &capture.record_for("ExecuteScheduledReport"),
            "ExecuteScheduledReport",
            &enabled,
            "SUCCESS",
        );
    }

    // The idempotent retry and the dry run each change nothing, so each accounts for
    // nothing — the same rule the reporting operations follow above.
    for (label, request) in [
        ("idempotent retry", triggering.clone()),
        ("dry run", json!({"arn": enabled, "dryRun": true})),
    ] {
        let (capture, _guard) = capture_events();
        ok_call(&router, "ExecuteScheduledReport", &request).await;
        let leaked = capture.records_for("ExecuteScheduledReport");
        assert!(
            leaked.is_empty(),
            "case {label}: a call that changes nothing must emit no audit record, captured {leaked:?}"
        );
    }

    // A second report swept under the SAME token is a different request: it executes,
    // so it accounts for its own execution under its own ARN.
    {
        let (capture, _guard) = capture_events();
        ok_call(
            &router,
            "ExecuteScheduledReport",
            &json!({"arn": other, "clientToken": token}),
        )
        .await;
        assert_record(
            &capture.record_for("ExecuteScheduledReport"),
            "ExecuteScheduledReport",
            &other,
            "SUCCESS",
        );
    }

    // Every refusal, with the modelled error as its outcome and the requested ARN as the
    // resource — including the ARN that resolves to no report at all.
    let absent = "arn:aws:bcm-dashboards::000000000000:scheduled-report/never-created";
    let cases: &[(Value, u16, &str, &str)] = &[
        (
            json!({"arn": paused}),
            400,
            paused.as_str(),
            "ValidationException",
        ),
        (
            json!({"arn": absent}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
    ];
    for (request, expected_status, resource, outcome) in cases {
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, "ExecuteScheduledReport", request).await;
        assert_eq!(
            status, *expected_status,
            "case {outcome}: expected {expected_status}, got {status}: {body}"
        );
        assert_record(
            &capture.record_for("ExecuteScheduledReport"),
            "ExecuteScheduledReport",
            resource,
            outcome,
        );
    }
}
