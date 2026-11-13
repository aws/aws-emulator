// SPDX-License-Identifier: CC0-1.0

//! The health a scheduled report reports for the ROLE it executes as — end to end over the
//! wire, with IAM and EventBridge mounted beside this service on one engine.
//!
//! The RULE asserted below: a scheduled report runs ACTING AS the role its
//! `scheduledReportExecutionRoleArn` names — the model documents it as the role this service
//! "will assume while executing the scheduled report" — so where the account's IAM records
//! show that role's trust policy withholding `sts:AssumeRole` from this service's principal,
//! every view of the report reports `healthStatus.statusCode` `UNHEALTHY` with
//! `EXECUTION_ROLE_ASSUME_FAILED` among its `statusReasons` and no execution is triggered,
//! while a role whose trust policy admits that principal leaves the report `HEALTHY`. The
//! verdict is re-derived from IAM on every resolution, never cached on the record, so
//! repairing (or breaking) the trust policy changes the report's health with no further call
//! to this service — and the call that reports it is a health verdict, HTTP 200 throughout,
//! not an API failure.
//!
//! The table spans the trust-document forms the rule generalises over rather than one
//! document: the principal named alone, named among a list, named with a wildcard action,
//! named in another letter case, named but explicitly denied, named for another action only,
//! not named at all (another service, an account principal), and a role ARN the account's IAM
//! holds nothing under — which is evidence about this emulator's store rather than about the
//! role, and so leaves the health alone.
//!
//! The OTHER half of what a role has to allow — the work the execution does under it once it
//! HAS been assumed — is `scheduled_report_execution_role_permissions`, which shares this
//! suite's harness.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{call, ok_call};
use super::scheduled_report_execution::{
    create_report, listed_summary, stored_report, DASHBOARD_NOT_FOUND, HEALTHY, UNHEALTHY,
};
use super::scheduled_report_lifecycle::source_dashboard;

/// The `StatusReason` a report whose execution role cannot be assumed carries. Source:
/// `smithy_query(query="shape",shape="StatusReason")` — the value that names a failure of
/// the assume itself, beside `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS`, which names what the
/// role's own policies do or do not allow once it HAS been assumed.
const EXECUTION_ROLE_ASSUME_FAILED: &str = "EXECUTION_ROLE_ASSUME_FAILED";

/// The Region every request below is signed for.
pub(crate) const REGION: &str = "us-east-1";

/// A well-formed `ServiceRoleArn` the account's IAM holds no role under, so a report naming
/// it is one this emulator knows nothing about rather than one with a broken role.
const UNHELD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/NoSuchReportRole";

/// This service, EventBridge and IAM on ONE engine — the shape `core/router/src/lib.rs`
/// mounts them in.
///
/// EventBridge is there because an execution delivers one notification onto the account's
/// default event bus, so a report that IS executed is not reported unable to deliver it; IAM
/// is there because the role a report executes as lives in IAM, and the health derivation
/// reads the very records `CreateRole` and `UpdateAssumeRolePolicy` wrote.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One IAM call: IAM speaks `awsQuery`, so its parameters travel as a form body and its
/// answer comes back as XML.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// The text of the first `<tag>` of an XML body.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// A trust document whose one statement allows `principal` the plain `sts:AssumeRole`, as a
/// client sends one.
pub(crate) fn grant(principal: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// Create the role `name` with the trust document `trust`, and answer the ARN IAM persisted.
pub(crate) async fn given_role_trusting(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    trust: &Value,
) -> String {
    let document = trust.to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateRole answered {status}: {created}"
    );
    xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {label:?}: no Arn in {created}"))
}

/// Replace the trust document of the role `name` with `trust`, the way a client repairs (or
/// breaks) a role's trust relationship outside this service.
async fn retrust_role(router: &MultiServiceRouter, label: &str, name: &str, trust: &Value) {
    let document = trust.to_string();
    let (status, updated) = iam(
        router,
        "UpdateAssumeRolePolicy",
        &[("RoleName", name), ("PolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: UpdateAssumeRolePolicy answered {status}: {updated}"
    );
}

/// Create a scheduled report on `dashboard_arn` that executes as `role_arn`, and answer its
/// ARN. The sibling suites' `create_report` always names one fixed role, which is exactly
/// what varies here.
pub(crate) async fn create_report_on_role(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    role_arn: &str,
) -> String {
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({"scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": role_arn,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 ? * MON *)",
                               "scheduleExpressionTimeZone": "UTC"},
        }}),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// `ExecuteScheduledReport` with `dryRun` as asked, answered as `(http status, body)`.
pub(crate) async fn execute(router: &MultiServiceRouter, arn: &str, dry_run: bool) -> (u16, Value) {
    call(
        router,
        "ExecuteScheduledReport",
        &json!({"arn": arn, "dryRun": dry_run}),
    )
    .await
}

/// Assert one `healthStatus` member: its code and its reasons (`None` for "the member must
/// be absent", which is what a healthy status carries).
pub(crate) fn assert_health(
    health: &Value,
    label: &str,
    view: &str,
    code: &str,
    reasons: Option<&[&str]>,
) {
    assert_eq!(
        health.get("statusCode").and_then(Value::as_str),
        Some(code),
        "case {label:?} ({view}): expected healthStatus.statusCode {code:?}, got {health}"
    );
    match reasons {
        Some(expected) => assert_eq!(
            health.get("statusReasons"),
            Some(&json!(expected)),
            "case {label:?} ({view}): expected statusReasons {expected:?}, got {health}"
        ),
        None => assert!(
            health.get("statusReasons").is_none(),
            "case {label:?} ({view}): a healthy report carries no statusReasons, got {health}"
        ),
    }
}

/// Assert the health EVERY view of the report reports: the dry run, the detail reader and
/// the listing.
pub(crate) async fn assert_every_view(
    router: &MultiServiceRouter,
    label: &str,
    arn: &str,
    code: &str,
    reasons: Option<&[&str]>,
) {
    let (status, dry) = execute(router, arn, true).await;
    assert_eq!(
        status, 200,
        "case {label:?}: a dry run is a health verdict, not an API failure: {dry}"
    );
    assert_eq!(
        dry.get("executionTriggered").and_then(Value::as_bool),
        Some(false),
        "case {label:?}: a dry run triggers nothing, got {dry}"
    );
    let dry_health = dry
        .get("healthStatus")
        .unwrap_or_else(|| panic!("case {label:?}: the dry run reports no healthStatus: {dry}"));
    assert_health(dry_health, label, "dry run", code, reasons);

    let detail = stored_report(router, arn).await;
    let detail_health = detail
        .get("healthStatus")
        .unwrap_or_else(|| panic!("case {label:?}: the detail view reports no health: {detail}"));
    assert_health(detail_health, label, "detail", code, reasons);

    let summary = listed_summary(router, arn).await;
    let summary_health = summary
        .get("healthStatus")
        .unwrap_or_else(|| panic!("case {label:?}: the summary reports no health: {summary}"));
    assert_health(summary_health, label, "summary", code, reasons);
}

/// One row: the trust document the report's role carries (or `None` for a report whose role
/// ARN the account's IAM holds nothing under), and the health every view must report.
struct TrustCase {
    label: &'static str,
    /// Built per row, because a document is a value rather than a literal.
    trust: Option<fn() -> Value>,
    expected_code: &'static str,
    /// `None` means `statusReasons` must be absent.
    expected_reasons: Option<&'static [&'static str]>,
}

/// The reasons an unassumable execution role is reported with.
const ASSUME_FAILED: &[&str] = &[EXECUTION_ROLE_ASSUME_FAILED];

fn rows() -> Vec<TrustCase> {
    vec![
        TrustCase {
            label: "this service's principal, plain sts:AssumeRole",
            trust: Some(|| grant(SERVICE_PRINCIPAL)),
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        TrustCase {
            label: "another service's principal only",
            trust: Some(|| grant("ec2.amazonaws.com")),
            expected_code: UNHEALTHY,
            expected_reasons: Some(ASSUME_FAILED),
        },
        TrustCase {
            label: "this principal with a wildcard action",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:*",
                    }],
                })
            }),
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        TrustCase {
            label: "this principal among a list of service principals",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": ["ec2.amazonaws.com", SERVICE_PRINCIPAL] },
                        "Action": ["sts:TagSession", "sts:AssumeRole"],
                    }],
                })
            }),
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        TrustCase {
            label: "this principal in another letter case",
            trust: Some(|| grant(&SERVICE_PRINCIPAL.to_uppercase())),
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        TrustCase {
            label: "this principal allowed and then explicitly denied",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Deny",
                            "Principal": { "Service": SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
            }),
            expected_code: UNHEALTHY,
            expected_reasons: Some(ASSUME_FAILED),
        },
        TrustCase {
            label: "this principal, but for another action only",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:TagSession",
                    }],
                })
            }),
            expected_code: UNHEALTHY,
            expected_reasons: Some(ASSUME_FAILED),
        },
        TrustCase {
            label: "an account principal and no service principal at all",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                        "Action": "sts:AssumeRole",
                    }],
                })
            }),
            expected_code: UNHEALTHY,
            expected_reasons: Some(ASSUME_FAILED),
        },
        TrustCase {
            label: "a role ARN the account's IAM holds nothing under",
            trust: None,
            expected_code: HEALTHY,
            expected_reasons: None,
        },
    ]
}

/// RULE: every view of a scheduled report reports `UNHEALTHY` with
/// `EXECUTION_ROLE_ASSUME_FAILED` exactly while the account's IAM shows the role it executes
/// as withholding the assume from this service's principal — and a non-dry-run execution of
/// such a report triggers nothing, while one whose role admits the principal is triggered.
#[tokio::test]
async fn a_reports_health_follows_the_trust_policy_of_the_role_it_executes_as() {
    for (index, row) in rows().into_iter().enumerate() {
        let label = row.label;
        // A fresh store per row, so one row's role cannot answer for another's.
        let router = emulator();
        let (dashboard_arn, _) = source_dashboard(&router, "ExecutionRoleSource").await;
        let role_arn = match row.trust {
            Some(trust) => {
                given_role_trusting(&router, label, &format!("report-role-{index}"), &trust())
                    .await
            }
            None => UNHELD_ROLE_ARN.to_string(),
        };
        let arn = create_report_on_role(
            &router,
            &dashboard_arn,
            &format!("Role Case {index}"),
            &role_arn,
        )
        .await;

        assert_every_view(
            &router,
            label,
            &arn,
            row.expected_code,
            row.expected_reasons,
        )
        .await;

        // The execution itself: a report whose role cannot be assumed has nothing to run
        // as, so nothing is triggered and its `lastExecutionAt` does not move.
        let (status, executed) = execute(&router, &arn, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an execution request is answered, not refused: {executed}"
        );
        let healthy = row.expected_code == HEALTHY;
        assert_eq!(
            executed.get("executionTriggered").and_then(Value::as_bool),
            Some(healthy),
            "case {label:?}: expected executionTriggered {healthy}, got {executed}"
        );
        let executed_health = executed
            .get("healthStatus")
            .unwrap_or_else(|| panic!("case {label:?}: the execution reports no health"));
        assert_health(
            executed_health,
            label,
            "execution",
            row.expected_code,
            row.expected_reasons,
        );
        let stored = stored_report(&router, &arn).await;
        assert_eq!(
            stored.get("lastExecutionAt").is_some(),
            healthy,
            "case {label:?}: expected lastExecutionAt present={healthy}, got {stored}"
        );
    }
}

/// RULE, the other direction: the verdict is re-derived from IAM on every resolution and
/// never cached, so breaking the trust policy of a role a healthy report already executed
/// under makes the report `UNHEALTHY`, and repairing it makes the report `HEALTHY` again —
/// with no call to this service in between either time.
#[tokio::test]
async fn breaking_and_repairing_the_trust_policy_moves_the_health_both_ways() {
    let label = "re-trusted";
    let router = emulator();
    let (dashboard_arn, _) = source_dashboard(&router, "RetrustSource").await;
    let role_name = "retrusted-report-role";
    let role_arn =
        given_role_trusting(&router, label, role_name, &grant(SERVICE_PRINCIPAL)).await;
    let arn = create_report_on_role(&router, &dashboard_arn, "Retrusted Report", &role_arn).await;

    // The positive control: the role admits this service, so the report is healthy and an
    // execution is triggered.
    assert_every_view(&router, label, &arn, HEALTHY, None).await;
    let (_, executed) = execute(&router, &arn, false).await;
    assert_eq!(
        executed.get("executionTriggered").and_then(Value::as_bool),
        Some(true),
        "case {label:?}: the report must execute under a role this service can assume: {executed}"
    );
    let executed_at = stored_report(&router, &arn)
        .await
        .get("lastExecutionAt")
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: the executed report reports no lastExecutionAt"));

    // The break, made outside this service: the role now trusts only another principal.
    retrust_role(&router, label, role_name, &grant("ec2.amazonaws.com")).await;
    assert_every_view(&router, label, &arn, UNHEALTHY, Some(ASSUME_FAILED)).await;
    let (_, refused) = execute(&router, &arn, false).await;
    assert_eq!(
        refused.get("executionTriggered").and_then(Value::as_bool),
        Some(false),
        "case {label:?}: a report whose role cannot be assumed triggers nothing: {refused}"
    );
    assert_eq!(
        stored_report(&router, &arn).await.get("lastExecutionAt"),
        Some(&executed_at),
        "case {label:?}: an execution that was not triggered must not move lastExecutionAt"
    );

    // The repair: the identical dry-run call reports the report healthy again, and the
    // reason the broken run stored is gone rather than carried forward.
    retrust_role(&router, label, role_name, &grant(SERVICE_PRINCIPAL)).await;
    assert_every_view(&router, label, &arn, HEALTHY, None).await;
    let (_, executed_again) = execute(&router, &arn, false).await;
    assert_eq!(
        executed_again
            .get("executionTriggered")
            .and_then(Value::as_bool),
        Some(true),
        "case {label:?}: the repaired report must execute again: {executed_again}"
    );
}

/// RULE: the reasons are independent, so a report whose role cannot be assumed AND whose
/// dashboard is gone reports both, in the order the model lists them — the role reason is not
/// a substitute for the dashboard one, nor the other way round.
#[tokio::test]
async fn a_report_with_two_broken_resources_reports_both_reasons() {
    let label = "role broken and dashboard deleted";
    let router = emulator();
    let (dashboard_arn, _) = source_dashboard(&router, "BothBrokenSource").await;
    let role_arn =
        given_role_trusting(&router, label, "both-broken-role", &grant("ec2.amazonaws.com")).await;
    let arn = create_report_on_role(&router, &dashboard_arn, "Both Broken", &role_arn).await;

    ok_call(&router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;

    assert_every_view(
        &router,
        label,
        &arn,
        UNHEALTHY,
        // `StatusReason`'s own order: the execution-role reason precedes the dashboard one
        // (`smithy_query(query="shape",shape="StatusReason")`).
        Some(&[EXECUTION_ROLE_ASSUME_FAILED, DASHBOARD_NOT_FOUND]),
    )
    .await;
}

/// RULE: a report whose role nothing in IAM contradicts is reported exactly as it was before
/// the role was consulted at all, whichever reader asks — the differential twin of the rows
/// above, asserted against the report every OTHER suite of this crate creates (one naming a
/// role ARN this emulator's IAM holds nothing under, with no IAM mounted at all).
#[tokio::test]
async fn an_emulator_without_iam_reports_the_report_healthy() {
    let label = "no IAM mounted";
    // The crate's own two-service harness: this service and EventBridge, no IAM.
    let router = super::dashboard_lifecycle::router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "NoIamSource").await;
    let arn = create_report(
        &router,
        &dashboard_arn,
        "No Iam Report",
        Some("ENABLED"),
        Some(vec![widget_id]),
    )
    .await;

    assert_every_view(&router, label, &arn, HEALTHY, None).await;
}
