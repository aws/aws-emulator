// SPDX-License-Identifier: CC0-1.0

//! `CreateScheduledReport` and the read that proves it, end to end over the wire.
//!
//! The rule under test: `CreateScheduledReport` mints a fresh `ScheduledReportArn`,
//! persists the submitted `scheduledReport` configuration under it with the defaults its
//! members document resolved (`scheduleConfig.state` to `ENABLED` when the request left it
//! unset, and the bounds of `scheduleConfig.schedulePeriod` to the instants their own
//! members name), and `GetScheduledReport` answers that stored record — every optional
//! member exactly as it was stored, `createdAt == updatedAt` on a report that was never
//! updated, and no execution time until something executes the report (its health reads
//! HEALTHY from the moment the service accepted it).
//! A `clientToken` keys ONE create — the repeat of the request that spent it is answered
//! that create's ARN and a different request under it is refused — and a `dashboardArn`
//! no stored dashboard answers to is refused.
//!
//! Each assertion spans the class rather than one recorded request: the table below
//! differs in which optional members are present, in schedule state (absent, `ENABLED`,
//! `DISABLED`), in widget-id count (absent, empty, one) and in whether a client token
//! is presented.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, graph_config, ok_call, router, timestamp};

/// A well-formed `ServiceRoleArn` (pattern
/// `^arn:aws[-a-z0-9]*:iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$`, source:
/// `smithy_query(query="operation",operation="CreateScheduledReport")`).
pub(crate) const EXECUTION_ROLE_ARN: &str =
    "arn:aws:iam::123456789012:role/BCMDashboardsReportRole";

/// A well-formed `DashboardArn` that no dashboard in a fresh store is keyed by, so a
/// create naming it is a reference to a missing resource rather than a malformed input.
const MISSING_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::123456789012:dashboard/no-such-board";

/// A `schedulePeriod` both write paths accept, as the suites of this crate submit one: a
/// start time at this instant and an end time a year on.
///
/// Built relative to the call rather than spelled as a calendar date, because each bound is
/// held to a rule measured from the request that submits it — "The start time cannot be more
/// than 5 minutes before the time of the request" and "The maximum allowed value is 3 years
/// from the current time" (`smithy_query(query="shape",shape="SchedulePeriod")`) — so a
/// fixed date would be a period the service refuses once the clock passes it.
pub(crate) fn acceptable_period() -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    json!({"startTime": now, "endTime": now + 365 * 86_400})
}

/// `ScheduledReportArn`'s modelled `@pattern`, checked by hand because this crate has
/// no regex dependency. Source: the `arn` member of `CreateScheduledReportResponse`
/// reported by `smithy_query(query="operation",operation="CreateScheduledReport")` —
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$`,
/// length 20..2048.
fn is_scheduled_report_arn(arn: &str) -> bool {
    fn lower_or_digit_or_dash(s: &str) -> bool {
        s.chars()
            .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
    }
    let Some(rest) = arn.strip_prefix("arn:aws") else {
        return false;
    };
    let Some((partition_suffix, rest)) = rest.split_once(':') else {
        return false;
    };
    if !lower_or_digit_or_dash(partition_suffix) {
        return false;
    }
    let Some(rest) = rest.strip_prefix("bcm-dashboards::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("scheduled-report/") else {
        return false;
    };
    let id_ok = id == "*" || (!id.is_empty() && lower_or_digit_or_dash(id));
    id_ok && (20..=2048).contains(&arn.len())
}

/// The wire error code (`__type`) of an error body.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Create a dashboard with one widget and return `(dashboard arn, widget id)` — the
/// resource a scheduled report has to point at.
pub(crate) async fn source_dashboard(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": name,
            "widgets": [{
                "title": "Spend",
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            }]
        }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .expect("CreateDashboard returns an arn")
        .to_string();
    let fetched = ok_call(router, "GetDashboard", &json!({"arn": arn})).await;
    let widget_id = fetched["widgets"][0]["id"]
        .as_str()
        .expect("the stored widget carries a minted id")
        .to_string();
    (arn, widget_id)
}

/// One row of the create table: a request, and what the stored record must read back as.
struct ReportCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    /// The `scheduleConfig` as submitted.
    schedule_config: Value,
    /// The `scheduleConfig.state` the stored record must carry — `ENABLED` where the
    /// request omitted it.
    expected_state: &'static str,
    /// `None` when the request omits `widgetIds`, otherwise the list it submits.
    widget_ids: Option<Vec<String>>,
    widget_date_range_override: Option<Value>,
    resource_tags: Option<Value>,
    client_token: Option<&'static str>,
}

/// The table. `widget_id` is a real widget of the source dashboard, so the one row
/// that names a widget names an existing one.
fn cases(widget_id: &str) -> Vec<ReportCase> {
    vec![
        ReportCase {
            label: "required members only, no state, no token",
            name: "Monthly Rollup",
            description: None,
            // `scheduleExpression` states the form of its own value — a six-field
            // `cron(...)` — so a well-formed cron is stored verbatim.
            schedule_config: json!({"scheduleExpression": "cron(0 8 1 * ? *)"}),
            expected_state: "ENABLED",
            widget_ids: None,
            widget_date_range_override: None,
            resource_tags: None,
            client_token: None,
        },
        ReportCase {
            label: "state omitted, timezone and schedule period present",
            name: "Weekly_Spend-1",
            description: Some("Weekly spend, by service."),
            schedule_config: json!({
                "scheduleExpression": "cron(0/15 * * * ? *)",
                "scheduleExpressionTimeZone": "Europe/Berlin",
                // GenericTimeStamp members, epoch seconds on the awsJson1_0 wire. Both
                // bounds are built relative to this instant, because both are held to a
                // rule measured from the request that submits them (the start time's grace
                // and the end time's ceiling, `tests/all/schedule_period_defaults.rs`).
                "schedulePeriod": acceptable_period()
            }),
            expected_state: "ENABLED",
            widget_ids: Some(vec![]),
            widget_date_range_override: None,
            resource_tags: Some(json!([{"key": "team", "value": "finops"}])),
            client_token: Some("token-weekly-0001"),
        },
        ReportCase {
            label: "state ENABLED explicitly, one widget id, date range override",
            name: "Quarterly Deep Dive",
            description: Some("One widget only."),
            // `ScheduleState` values, source: the `ScheduleState` enum in the model.
            schedule_config: json!({
                "scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)",
                "state": "ENABLED"
            }),
            expected_state: "ENABLED",
            widget_ids: Some(vec![widget_id.to_string()]),
            widget_date_range_override: Some(json!({
                "startTime": {"type": "RELATIVE", "value": "-P3M"},
                "endTime": {"type": "RELATIVE", "value": "P0D"}
            })),
            resource_tags: Some(json!([
                {"key": "team", "value": "finops"},
                {"key": "empty-value-is-legal", "value": ""}
            ])),
            client_token: Some("token-quarterly-0002"),
        },
        ReportCase {
            label: "state DISABLED is kept, absolute date range override",
            name: "Paused Report",
            description: None,
            schedule_config: json!({"scheduleExpression": "cron(0 7 ? * MON *)", "state": "DISABLED"}),
            expected_state: "DISABLED",
            widget_ids: Some(vec![widget_id.to_string()]),
            widget_date_range_override: Some(json!({
                "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
            })),
            resource_tags: None,
            client_token: Some("token-paused-0003"),
        },
    ]
}

/// The `scheduledReport` member of a create request for `case`.
fn report_input(case: &ReportCase, dashboard_arn: &str) -> Value {
    let mut report = json!({
        "name": case.name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": case.schedule_config.clone(),
    });
    if let Some(description) = case.description {
        report["description"] = json!(description);
    }
    if let Some(widget_ids) = &case.widget_ids {
        report["widgetIds"] = json!(widget_ids);
    }
    if let Some(range) = &case.widget_date_range_override {
        report["widgetDateRangeOverride"] = range.clone();
    }
    report
}

fn create_request(case: &ReportCase, dashboard_arn: &str) -> Value {
    let mut request = json!({"scheduledReport": report_input(case, dashboard_arn)});
    if let Some(tags) = &case.resource_tags {
        request["resourceTags"] = tags.clone();
    }
    if let Some(token) = case.client_token {
        request["clientToken"] = json!(token);
    }
    request
}

/// CreateScheduledReport stores the submitted configuration under a freshly minted ARN
/// and GetScheduledReport reads that record back — over a table of reports differing in
/// which optional members they carry, in schedule state, in widget-id count and in
/// whether they present a client token.
#[tokio::test]
async fn create_scheduled_report_persists_the_configuration_and_get_reads_it_back() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "ReportSource").await;

    let mut minted: Vec<String> = Vec::new();
    for case in cases(&widget_id) {
        let label = case.label;
        let request = create_request(&case, &dashboard_arn);
        let created = ok_call(&router, "CreateScheduledReport", &request).await;

        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: CreateScheduledReport returned no arn: {created}"))
            .to_string();
        assert!(
            is_scheduled_report_arn(&arn),
            "case {label}: expected a ScheduledReportArn, got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {label}: every create mints a distinct arn, but {arn:?} repeats {minted:?}"
        );
        minted.push(arn.clone());

        let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        let report = fetched
            .get("scheduledReport")
            .unwrap_or_else(|| panic!("case {label}: no scheduledReport in {fetched}"));

        assert_eq!(
            report.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {label}: the record reports the arn it is stored under, got {report}"
        );
        assert_eq!(
            report.get("name").and_then(Value::as_str),
            Some(case.name),
            "case {label}: expected name {:?}, got {report}",
            case.name
        );
        assert_eq!(
            report.get("dashboardArn").and_then(Value::as_str),
            Some(dashboard_arn.as_str()),
            "case {label}: expected dashboardArn {dashboard_arn:?}, got {report}"
        );
        assert_eq!(
            report
                .get("scheduledReportExecutionRoleArn")
                .and_then(Value::as_str),
            Some(EXECUTION_ROLE_ARN),
            "case {label}: expected the submitted execution role, got {report}"
        );
        assert_eq!(
            report.get("description").and_then(Value::as_str),
            case.description,
            "case {label}: expected description {:?}, got {report}",
            case.description
        );

        // The schedule config is stored verbatim except for `state`, which is resolved
        // to ENABLED when the request left it unset.
        let created_at = timestamp(report, "createdAt", label);
        let stored_schedule = report
            .get("scheduleConfig")
            .unwrap_or_else(|| panic!("case {label}: no scheduleConfig in {report}"));
        assert_eq!(
            stored_schedule.get("state").and_then(Value::as_str),
            Some(case.expected_state),
            "case {label}: expected scheduleConfig.state {:?}, got {stored_schedule}",
            case.expected_state
        );
        for member in ["scheduleExpression", "scheduleExpressionTimeZone"] {
            assert_eq!(
                stored_schedule.get(member),
                case.schedule_config.get(member),
                "case {label}: scheduleConfig.{member} must be stored verbatim, got {stored_schedule}"
            );
        }
        // The schedule period is stored with each bound the request named verbatim and
        // each bound it left unset resolved to the default that member documents: the
        // instant of the create for the start, 3 years on for the end — where 3 calendar
        // years span 1095 days (three 365-day years) plus the leap days the window holds.
        // The resolution over the shapes a period can take is tabled in
        // `tests/all/schedule_period_defaults.rs`.
        let stored_period = stored_schedule
            .get("schedulePeriod")
            .unwrap_or_else(|| panic!("case {label}: the stored schedule has no schedulePeriod: {stored_schedule}"));
        for (member, default_days) in [("startTime", 0.0..=0.0), ("endTime", 1095.0..=1096.0)] {
            let submitted = case
                .schedule_config
                .get("schedulePeriod")
                .and_then(|period| period.get(member))
                .and_then(Value::as_f64);
            let stored = stored_period
                .get(member)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {label}: schedulePeriod.{member} is absent or not an instant: {stored_period}"));
            match submitted {
                Some(submitted) => assert_eq!(
                    stored, submitted,
                    "case {label}: schedulePeriod.{member} must round-trip, got {stored_period}"
                ),
                None => {
                    let days = (stored - created_at) / 86_400.0;
                    assert!(
                        default_days.contains(&days),
                        "case {label}: schedulePeriod.{member} must default to {default_days:?} \
                         days from the create at {created_at}, got {stored}, which is {days} days on"
                    );
                }
            }
        }

        match &case.widget_ids {
            Some(submitted) => assert_eq!(
                report.get("widgetIds"),
                Some(&json!(submitted)),
                "case {label}: expected widgetIds {submitted:?}, got {report}"
            ),
            None => assert!(
                report.get("widgetIds").is_none(),
                "case {label}: a report that named no widget must not grow widgetIds: {report}"
            ),
        }
        assert_eq!(
            report.get("widgetDateRangeOverride"),
            case.widget_date_range_override.as_ref(),
            "case {label}: expected widgetDateRangeOverride {:?}, got {report}",
            case.widget_date_range_override
        );

        let updated_at = timestamp(report, "updatedAt", label);
        assert_eq!(
            created_at, updated_at,
            "case {label}: a report that was never updated has createdAt == updatedAt"
        );
        assert!(
            created_at > 1_700_000_000.0,
            "case {label}: createdAt {created_at} is not a plausible epoch-seconds instant"
        );

        // Nothing has executed the report, so there is no execution time — and the
        // health a report the service accepted carries before any refresh is HEALTHY,
        // with neither a refresh instant nor reasons.
        assert!(
            report.get("lastExecutionAt").is_none(),
            "case {label}: a report that never ran has no lastExecutionAt: {report}"
        );
        assert_eq!(
            report.get("healthStatus"),
            Some(&json!({"statusCode": "HEALTHY"})),
            "case {label}: an unrefreshed report reports HEALTHY and nothing else: {report}"
        );

        // A read changes nothing a later reader sees.
        let again = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            again, fetched,
            "case {label}: reading the report twice must answer the same record"
        );
    }

    assert_eq!(
        minted.len(),
        4,
        "every row of the table must have created a report, got {minted:?}"
    );
}

/// A `clientToken` creates at most one report: the first call presenting it creates the
/// report, and every later call presenting it — the identical request as much as a
/// DIFFERENT one — is refused with the modelled ConflictException (409), leaving the first
/// record untouched. The rule is tabled in `tests/all/create_report_token_spent.rs`, with
/// the wording each conflict carries.
#[tokio::test]
async fn a_client_token_creates_at_most_one_scheduled_report() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "TokenSource").await;

    let token = "retry-token-9f";
    let first = json!({
        "clientToken": token,
        "scheduledReport": {
            "name": "Token Winner",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
        }
    });
    let winner = ok_call(&router, "CreateScheduledReport", &first).await;
    let winner_arn = winner["arn"].as_str().expect("an arn").to_string();
    let winner_record = ok_call(&router, "GetScheduledReport", &json!({"arn": winner_arn})).await;

    // The identical request again — what an SDK transport retry resends — is refused as
    // well: the token is spent by the create that took it.
    let (status, body) = call(&router, "CreateScheduledReport", &first).await;
    assert_eq!(
        status, 409,
        "a retry under the same clientToken is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "expected ConflictException for the retry, got {body}"
    );

    // A different request reusing the token is refused: one token cannot answer for two
    // reports.
    let mut different = first.clone();
    different["scheduledReport"]["name"] = json!("Token Loser");
    different["scheduledReport"]["scheduleConfig"] = json!({"scheduleExpression": "cron(0 12 L * ? *)", "state": "DISABLED"});
    let (status, body) = call(&router, "CreateScheduledReport", &different).await;
    assert_eq!(
        status, 409,
        "a clientToken reused for a different request is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "expected ConflictException, got {body}"
    );

    // The winner is still there, unchanged, and is still the only report that token
    // ever created: a fresh token mints a different ARN.
    assert_eq!(
        ok_call(&router, "GetScheduledReport", &json!({"arn": winner_arn})).await,
        winner_record,
        "neither refused presentation of the token may touch the winning record"
    );
    let second = ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "clientToken": "another-token-0a",
            "scheduledReport": {
                "name": "Token Winner",
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
            }
        }),
    )
    .await;
    assert_ne!(
        second["arn"].as_str(),
        Some(winner_arn.as_str()),
        "a different clientToken creates a distinct report: {second}"
    );
}

/// With no `clientToken` there is no create key, so identical requests are distinct
/// creates — each one mints its own ARN and its own record.
#[tokio::test]
async fn creates_without_a_client_token_are_distinct_reports() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "NoTokenSource").await;

    let request = json!({
        "scheduledReport": {
            "name": "Same Name",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
        }
    });
    let mut arns = Vec::new();
    for attempt in 0..3 {
        let created = ok_call(&router, "CreateScheduledReport", &request).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("attempt {attempt}: no arn in {created}"))
            .to_string();
        assert!(
            !arns.contains(&arn),
            "attempt {attempt}: expected a distinct arn, but {arn:?} repeats {arns:?}"
        );
        arns.push(arn);
    }
    for arn in &arns {
        let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["scheduledReport"]["arn"].as_str(),
            Some(arn.as_str()),
            "each create must have its own stored record: {fetched}"
        );
    }
}

/// A create that names a dashboard this account does not store is refused with the
/// modelled ValidationException (400), nothing is persisted, and the clientToken it
/// presented is still free for the retry that gets the dashboard right.
#[tokio::test]
async fn a_report_on_a_missing_dashboard_is_refused_and_persists_nothing() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "MissingSource").await;

    let token = "fix-the-arn-then-retry";
    let mut request = json!({
        "clientToken": token,
        "scheduledReport": {
            "name": "Orphan Report",
            "dashboardArn": MISSING_DASHBOARD_ARN,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
        }
    });
    let (status, body) = call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        status, 400,
        "a report on a missing dashboard is refused with 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ValidationException"),
        "expected ValidationException, got {body}"
    );
    assert!(
        body.get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains(MISSING_DASHBOARD_ARN)),
        "the refusal names the dashboard arn it could not resolve: {body}"
    );

    // The refused call created nothing, so it consumed neither the token nor an ARN.
    request["scheduledReport"]["dashboardArn"] = json!(dashboard_arn);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
    assert_eq!(
        fetched["scheduledReport"]["dashboardArn"].as_str(),
        Some(dashboard_arn.as_str()),
        "the retry under the same token must have created the report: {fetched}"
    );
}

/// GetScheduledReport refuses an ARN no report is stored under with the service's
/// not-found answer — the `ResourceNotFoundException` code at HTTP 400 (see
/// `tests/all/resource_not_found.rs`) — including the ARN of a report that was never
/// created and a well-formed ARN of another resource's shape.
#[tokio::test]
async fn getting_a_report_that_was_never_created_is_not_found() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "NotFoundSource").await;
    // One real report exists, so a miss is a miss on a populated store.
    ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "The Only One",
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
            }
        }),
    )
    .await;

    for arn in [
        "arn:aws:bcm-dashboards::123456789012:scheduled-report/never-created",
        "arn:aws-cn:bcm-dashboards::000000000000:scheduled-report/ffffffffffffffffffffffffffffffff",
    ] {
        let (status, body) = call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {arn}: an unknown report arn answers 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {arn}: expected ResourceNotFoundException, got {body}"
        );
    }
}
