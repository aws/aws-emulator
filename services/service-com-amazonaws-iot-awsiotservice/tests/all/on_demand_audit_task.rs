// SPDX-License-Identifier: CC0-1.0

//! ON-DEMAND DEVICE DEFENDER AUDITS, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's service is
//! mounted in, so routing, the restJson1 `POST /audit/tasks` and `GET /audit/tasks/{taskId}`
//! bindings, the handlers and the response serialization are all exercised.
//!
//! The rules under test, none of them specific to one audit:
//!
//! * `StartOnDemandAuditTask` mints an audit task id for the checks the request targets and
//!   persists the audit under it, so `DescribeAuditTask` reports the audit that call
//!   performed: its type, its start time, one entry per targeted check and statistics over
//!   all of them;
//! * every targeted name must be an audit check the account has ENABLED ("the checks you
//!   specify must be enabled for your account or an exception occurs"), and a request that
//!   targets no check, one outside the catalog or one the account has not enabled is refused
//!   with the modelled `InvalidRequestException` (400), persisting nothing;
//! * an audit is answered IN_PROGRESS and the clock completes it, so the status a reader
//!   reports is derived from the recorded due instant and repeating the read changes
//!   nothing;
//! * ONE CHECK IS ONE EVALUATION over the account's resources: the resources a check
//!   counted and how many of them it found non-compliant come from what this account holds
//!   at the instant the audit ran — its certificates, its IoT policies, its logging
//!   configuration — so a later audit of a changed account amounts to something different
//!   while the earlier audit still reports what it saw;
//! * a `taskId` the account holds no audit under is a 404 `ResourceNotFoundException`, never
//!   a hollow 200.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    AUDIT_CHECK_NAMES, AUDIT_TASK_COMPLETED, AUDIT_TASK_IN_PROGRESS, ON_DEMAND_AUDIT_TASK_TYPE,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. An audit is account-wide, so no
/// assertion depends on it.
const REGION: &str = "eu-west-2";

/// The role the account's audit configuration names, of more than the modelled minimum
/// length (`RoleArn`, len 20..2048).
const AUDIT_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/device-defender-audit";

/// How long a started audit's completion is waited for before the test gives up.
///
/// Generous beside the span the emulator completes an audit after: the test waits for the
/// COMPLETED status to be reported, never for a particular amount of time to pass, so a
/// loaded or a fast host reaches the same verdict.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(120);

/// How often the completion is looked for again while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Audit checks named individually by the rows below. Each is an entry of the emulator's
/// catalog of Device Defender audit checks
/// (`service_com_amazonaws_iot_awsiotservice::state::AUDIT_CHECK_NAMES`), which the
/// `every check in the catalog` rows cover in full.
const LOGGING_DISABLED_CHECK: &str = "LOGGING_DISABLED_CHECK";
const DEVICE_CERTIFICATE_EXPIRING_CHECK: &str = "DEVICE_CERTIFICATE_EXPIRING_CHECK";
const DEVICE_CERTIFICATE_AGE_CHECK: &str = "DEVICE_CERTIFICATE_AGE_CHECK";
const IOT_POLICY_OVERLY_PERMISSIVE_CHECK: &str = "IOT_POLICY_OVERLY_PERMISSIVE_CHECK";
const CONFLICTING_CLIENT_IDS_CHECK: &str = "CONFLICTING_CLIENT_IDS_CHECK";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the audit request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    /// Enable `check_names` for the account, each with the configuration entries given.
    async fn enable_checks(&self, case: &str, check_names: &[&str]) {
        let mut checks = Map::new();
        for check_name in check_names {
            checks.insert((*check_name).to_string(), json!({ "enabled": true }));
        }
        let request = json!({
            "roleArn": AUDIT_ROLE_ARN,
            "auditCheckConfigurations": Value::Object(checks),
        });
        let reply = self
            .dispatch("PATCH", "/audit/configuration", Some(&request))
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected UpdateAccountAuditConfiguration to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// Enable every audit check the emulator's catalog holds.
    async fn enable_every_check(&self, case: &str) {
        self.enable_checks(case, AUDIT_CHECK_NAMES).await;
    }

    async fn start_audit(&self, targets: &[String]) -> Reply {
        let request = json!({ "targetCheckNames": targets });
        self.dispatch("POST", "/audit/tasks", Some(&request)).await
    }

    /// Start an audit over `targets` and require it to succeed, answering its task id.
    async fn started_audit(&self, case: &str, targets: &[String]) -> String {
        let reply = self.start_audit(targets).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected StartOnDemandAuditTask to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no taskId in {}", reply.text()))
            .to_string()
    }

    async fn describe_audit(&self, task_id: &str) -> Reply {
        self.dispatch("GET", &format!("/audit/tasks/{task_id}"), None)
            .await
    }

    /// The audit `task_id`, read back and required to succeed.
    async fn reported_audit(&self, case: &str, task_id: &str) -> Value {
        let reply = self.describe_audit(task_id).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected DescribeAuditTask to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }

    /// The audit `task_id` once it reports [`AUDIT_TASK_COMPLETED`].
    ///
    /// The completion is waited for by LOOKING AGAIN until the status says so, so nothing
    /// here depends on how fast the host runs; the deadline only bounds a wait that is
    /// never going to end.
    async fn completed_audit(&self, case: &str, task_id: &str) -> Value {
        let deadline = SystemTime::now() + COMPLETION_DEADLINE;
        loop {
            let report = self.reported_audit(case, task_id).await;
            if report["taskStatus"].as_str() == Some(AUDIT_TASK_COMPLETED) {
                return report;
            }
            assert!(
                SystemTime::now() < deadline,
                "case {case}: audit {task_id} never reported {AUDIT_TASK_COMPLETED}, \
                 last report {report}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Register one X.509 device certificate, answering its id.
    async fn create_certificate(&self, case: &str) -> String {
        let reply = self
            .dispatch("POST", "/keys-and-certificate?setAsActive=true", None)
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected CreateKeysAndCertificate to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()["certificateId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no certificateId in {}", reply.text()))
            .to_string()
    }

    /// Create one IoT policy carrying `document`.
    async fn create_policy(&self, case: &str, policy_name: &str, document: &Value) {
        let request = json!({ "policyDocument": document.to_string() });
        let reply = self
            .dispatch(
                "POST",
                &format!("/policies/{policy_name}"),
                Some(&request),
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected CreatePolicy to answer 200 for {policy_name}, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// Set the account's V2 logging options to log at `default_log_level`.
    async fn set_logging_options(&self, case: &str, default_log_level: &str) {
        let request = json!({
            "roleArn": "arn:aws:iam::123456789012:role/iot-logging",
            "defaultLogLevel": default_log_level,
            "disableAllLogs": false,
        });
        let reply = self
            .dispatch("POST", "/v2LoggingOptions", Some(&request))
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected SetV2LoggingOptions to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    /// The message a restJson1 error response carries.
    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// A policy document that allows `action` on `resource`, in the IAM grammar
/// `CreatePolicy$policyDocument` takes.
fn policy_document(action: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
}

/// The names a report's `auditDetails` carries, in order.
fn reported_check_names(report: &Value) -> BTreeSet<String> {
    report["auditDetails"]
        .as_object()
        .unwrap_or_else(|| panic!("no auditDetails map in {report}"))
        .keys()
        .cloned()
        .collect()
}

/// What a report says about one check.
fn check_details<'a>(case: &str, report: &'a Value, check_name: &str) -> &'a Value {
    report["auditDetails"]
        .get(check_name)
        .unwrap_or_else(|| panic!("case {case}: {check_name} missing from {report}"))
}

/// One statistic of a report, as a number.
fn statistic(case: &str, report: &Value, member: &str) -> i64 {
    report["taskStatistics"][member]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: no taskStatistics.{member} in {report}"))
}

/// A `taskId` AWS IoT accepts: `AuditTaskId` is `len 1..40, pattern ^[a-zA-Z0-9\-]+$`.
fn is_modeled_task_id(task_id: &str) -> bool {
    (1..=40).contains(&task_id.chars().count())
        && task_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// One row of the target table: which checks an audit is started over.
struct TargetCase {
    case: &'static str,
    targets: Vec<String>,
}

/// An audit records the checks it targets, and the id it is started under identifies it.
///
/// The rows differ in how MANY checks are targeted and in which resource classes those
/// checks are about — one, two, three, and the whole catalog — because that is what the
/// request varies.
#[tokio::test]
async fn an_on_demand_audit_records_the_checks_it_targets() {
    let emulator = Emulator::new();
    emulator.enable_every_check("setup").await;

    let cases = vec![
        TargetCase {
            case: "one check",
            targets: vec![LOGGING_DISABLED_CHECK.to_string()],
        },
        TargetCase {
            case: "two checks about different resource classes",
            targets: vec![
                IOT_POLICY_OVERLY_PERMISSIVE_CHECK.to_string(),
                DEVICE_CERTIFICATE_AGE_CHECK.to_string(),
            ],
        },
        TargetCase {
            case: "three checks, one of them about resources this account has none of",
            targets: vec![
                DEVICE_CERTIFICATE_EXPIRING_CHECK.to_string(),
                DEVICE_CERTIFICATE_AGE_CHECK.to_string(),
                CONFLICTING_CLIENT_IDS_CHECK.to_string(),
            ],
        },
        TargetCase {
            // A check named twice is still one check of the audit, so the count the
            // statistics report follows the checks performed and not the list's length.
            case: "the same check named twice",
            targets: vec![
                DEVICE_CERTIFICATE_AGE_CHECK.to_string(),
                DEVICE_CERTIFICATE_AGE_CHECK.to_string(),
            ],
        },
        TargetCase {
            // The catalog is the emulator's own record of the Device Defender audit
            // checks, so this row covers every check name, including those no other row
            // names.
            case: "every check in the catalog",
            targets: AUDIT_CHECK_NAMES.iter().map(|n| (*n).to_string()).collect(),
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for TargetCase { case, targets } in &cases {
        let task_id = emulator.started_audit(case, targets).await;
        assert!(
            is_modeled_task_id(&task_id),
            "case {case}: expected a modelled AuditTaskId, got {task_id:?}"
        );
        assert!(
            minted.insert(task_id.clone()),
            "case {case}: expected a task id no earlier audit was started under, got \
             {task_id:?} again"
        );

        let report = emulator.reported_audit(case, &task_id).await;
        assert_eq!(
            report["taskType"].as_str(),
            Some(ON_DEMAND_AUDIT_TASK_TYPE),
            "case {case}: expected an on-demand audit, got {report}"
        );
        assert_eq!(
            report["scheduledAuditName"].as_str(),
            None,
            "case {case}: an audit no scheduled audit started reports no name, got {report}"
        );
        assert!(
            report["taskStartTime"].is_number(),
            "case {case}: expected the audit to report when it started, got {report}"
        );
        let expected: BTreeSet<String> = targets.iter().cloned().collect();
        assert_eq!(
            reported_check_names(&report),
            expected,
            "case {case}: expected one entry per targeted check, got {report}"
        );
        assert_eq!(
            statistic(case, &report, "totalChecks"),
            i64::try_from(expected.len()).expect("a row targets few checks"),
            "case {case}: expected totalChecks to count every check performed, got {report}"
        );
    }
}

/// A started audit is in progress, and the clock is what completes it.
#[tokio::test]
async fn an_audit_task_completes_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let case = "an audit over two checks";
    emulator
        .enable_checks(
            case,
            &[LOGGING_DISABLED_CHECK, IOT_POLICY_OVERLY_PERMISSIVE_CHECK],
        )
        .await;
    let targets = vec![
        LOGGING_DISABLED_CHECK.to_string(),
        IOT_POLICY_OVERLY_PERMISSIVE_CHECK.to_string(),
    ];
    let task_id = emulator.started_audit(case, &targets).await;

    // Answered in the non-terminal status, with every check of it in progress and no
    // outcome reported for any of them yet.
    let started = emulator.reported_audit(case, &task_id).await;
    assert_eq!(
        started["taskStatus"].as_str(),
        Some(AUDIT_TASK_IN_PROGRESS),
        "case {case}: expected a fresh audit to be in progress, got {started}"
    );
    assert_eq!(
        statistic(case, &started, "inProgressChecks"),
        2,
        "case {case}: expected both checks in progress, got {started}"
    );
    for check_name in &targets {
        let details = check_details(case, &started, check_name);
        assert_eq!(
            details["checkRunStatus"].as_str(),
            Some(AUDIT_TASK_IN_PROGRESS),
            "case {case}: expected {check_name} in progress, got {details}"
        );
        assert_eq!(
            details["checkCompliant"].as_bool(),
            None,
            "case {case}: expected {check_name} to report no outcome yet, got {details}"
        );
    }

    // The completion the clock owes: reported without anybody writing it.
    let completed = emulator.completed_audit(case, &task_id).await;
    assert_eq!(
        statistic(case, &completed, "inProgressChecks"),
        0,
        "case {case}: a completed audit has no check in progress, got {completed}"
    );
    let total = statistic(case, &completed, "totalChecks");
    let compliant = statistic(case, &completed, "compliantChecks");
    let non_compliant = statistic(case, &completed, "nonCompliantChecks");
    assert_eq!(
        compliant + non_compliant,
        total,
        "case {case}: every check of a completed audit is counted once, got {completed}"
    );
    for check_name in &targets {
        let details = check_details(case, &completed, check_name);
        let status = details["checkRunStatus"].as_str().unwrap_or_default();
        assert!(
            status == "COMPLETED_COMPLIANT" || status == "COMPLETED_NON_COMPLIANT",
            "case {case}: expected {check_name} to have run, got {details}"
        );
        assert!(
            details["totalResourcesCount"].is_number(),
            "case {case}: expected {check_name} to count the resources it evaluated, got \
             {details}"
        );
    }

    // The read is a read: it reports the same audit again and moves nothing.
    let again = emulator.reported_audit(case, &task_id).await;
    assert_eq!(
        again, completed,
        "case {case}: expected a second read to report the same audit, got {again}"
    );
}

/// One row of the outcome table: what one check amounts to over the account below.
struct OutcomeCase {
    case: &'static str,
    check_name: &'static str,
    total_resources: i64,
    non_compliant_resources: i64,
}

/// What a check amounts to is read off the account's resources, not off the request.
///
/// The account below holds two certificates, three IoT policies — one narrow, one granting
/// the whole service and one granting every resource — and no logging options, and the rows
/// differ in the resource class the check is about and in how many of that class break its
/// rule (none of them, some of them, all of them).
#[tokio::test]
async fn audit_check_outcomes_come_from_the_accounts_resources() {
    let emulator = Emulator::new();
    let case = "an audit of the whole catalog";
    emulator.enable_every_check(case).await;
    emulator.create_certificate(case).await;
    emulator.create_certificate(case).await;
    emulator
        .create_policy(
            case,
            "narrow-device-policy",
            &policy_document(
                json!(["iot:Connect"]),
                json!(["arn:aws:iot:eu-west-2:123456789012:client/sensor-1"]),
            ),
        )
        .await;
    emulator
        .create_policy(
            case,
            "service-wide-policy",
            &policy_document(
                json!("iot:*"),
                json!("arn:aws:iot:eu-west-2:123456789012:topic/telemetry"),
            ),
        )
        .await;
    emulator
        .create_policy(
            case,
            "every-resource-policy",
            &policy_document(json!(["iot:Publish"]), json!("*")),
        )
        .await;

    let targets: Vec<String> = AUDIT_CHECK_NAMES.iter().map(|n| (*n).to_string()).collect();
    let task_id = emulator.started_audit(case, &targets).await;
    let report = emulator.completed_audit(case, &task_id).await;

    let cases = [
        OutcomeCase {
            case: "the account's logging configuration, with logging not set up",
            check_name: LOGGING_DISABLED_CHECK,
            total_resources: 1,
            non_compliant_resources: 1,
        },
        OutcomeCase {
            case: "both certificates, neither near its expiry",
            check_name: DEVICE_CERTIFICATE_EXPIRING_CHECK,
            total_resources: 2,
            non_compliant_resources: 0,
        },
        OutcomeCase {
            case: "both certificates, neither older than the configured age",
            check_name: DEVICE_CERTIFICATE_AGE_CHECK,
            total_resources: 2,
            non_compliant_resources: 0,
        },
        OutcomeCase {
            case: "three policies, two of them granting more than they name",
            check_name: IOT_POLICY_OVERLY_PERMISSIVE_CHECK,
            total_resources: 3,
            non_compliant_resources: 2,
        },
        OutcomeCase {
            case: "a check about resources this account holds none of",
            check_name: CONFLICTING_CLIENT_IDS_CHECK,
            total_resources: 0,
            non_compliant_resources: 0,
        },
    ];

    for OutcomeCase {
        case,
        check_name,
        total_resources,
        non_compliant_resources,
    } in &cases
    {
        let details = check_details(case, &report, check_name);
        assert_eq!(
            details["totalResourcesCount"].as_i64(),
            Some(*total_resources),
            "case {case}: expected {check_name} to have evaluated {total_resources} \
             resources, got {details}"
        );
        assert_eq!(
            details["nonCompliantResourcesCount"].as_i64(),
            Some(*non_compliant_resources),
            "case {case}: expected {check_name} to have found {non_compliant_resources} \
             non-compliant, got {details}"
        );
        assert_eq!(
            details["checkCompliant"].as_bool(),
            Some(*non_compliant_resources == 0),
            "case {case}: expected {check_name}'s compliance to follow its findings, got \
             {details}"
        );
        assert_eq!(
            details["suppressedNonCompliantResourcesCount"].as_i64(),
            Some(0),
            "case {case}: nothing suppresses a finding in this account, got {details}"
        );
    }
    // The statistics count the checks the per-check outcomes report: two of the catalog's
    // checks found something here.
    assert_eq!(
        statistic(case, &report, "nonCompliantChecks"),
        2,
        "case {case}: expected the two checks that found something to be counted, got {report}"
    );
    assert_eq!(
        statistic(case, &report, "compliantChecks") + 2,
        statistic(case, &report, "totalChecks"),
        "case {case}: every other check of the catalog found nothing, got {report}"
    );

    // A LATER audit of a CHANGED account amounts to something else, and the earlier audit
    // still reports what it saw.
    let fixed = "the same check once logging is set up";
    emulator.set_logging_options(fixed, "INFO").await;
    let second = emulator
        .started_audit(fixed, &[LOGGING_DISABLED_CHECK.to_string()])
        .await;
    let second_report = emulator.completed_audit(fixed, &second).await;
    let details = check_details(fixed, &second_report, LOGGING_DISABLED_CHECK);
    assert_eq!(
        details["nonCompliantResourcesCount"].as_i64(),
        Some(0),
        "case {fixed}: expected the logging check to find nothing once logs are enabled, \
         got {details}"
    );
    let earlier = emulator.reported_audit(fixed, &task_id).await;
    assert_eq!(
        earlier, report,
        "case {fixed}: expected the earlier audit to still report the account it saw, got \
         {earlier}"
    );
}

/// One row of the refusal table: a request that starts no audit.
struct RefusalCase {
    case: &'static str,
    targets: Vec<String>,
    message: String,
}

/// A request that names nothing auditable is refused, and nothing is recorded for it.
#[tokio::test]
async fn start_on_demand_audit_task_refuses_what_it_cannot_audit() {
    let emulator = Emulator::new();
    // One check enabled, so a row can target a catalog check this account has NOT enabled.
    emulator.enable_checks("setup", &[LOGGING_DISABLED_CHECK]).await;

    let cases = vec![
        RefusalCase {
            case: "no check at all",
            targets: Vec::new(),
            message: "targetCheckNames must specify at least one audit check".to_string(),
        },
        RefusalCase {
            case: "a name outside the catalog of audit checks",
            targets: vec!["NOT_AN_AUDIT_CHECK".to_string()],
            message: "Invalid audit check name: NOT_AN_AUDIT_CHECK".to_string(),
        },
        RefusalCase {
            case: "a catalog check this account has not enabled",
            targets: vec![DEVICE_CERTIFICATE_AGE_CHECK.to_string()],
            message: format!(
                "Audit check {DEVICE_CERTIFICATE_AGE_CHECK} is not enabled for this account"
            ),
        },
        RefusalCase {
            case: "one enabled check beside one that is not",
            targets: vec![
                LOGGING_DISABLED_CHECK.to_string(),
                CONFLICTING_CLIENT_IDS_CHECK.to_string(),
            ],
            message: format!(
                "Audit check {CONFLICTING_CLIENT_IDS_CHECK} is not enabled for this account"
            ),
        },
    ];

    for RefusalCase {
        case,
        targets,
        message,
    } in &cases
    {
        let reply = emulator.start_audit(targets).await;
        assert_eq!(
            reply.status,
            400,
            "case {case}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {case}: expected InvalidRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            &reply.error_message(),
            message,
            "case {case}: expected the refusal to name the rule it broke, got {}",
            reply.text()
        );
    }

    // Nothing above started an audit, and the one request that satisfies every rule still
    // does.
    let allowed = "the enabled check on its own";
    let task_id = emulator
        .started_audit(allowed, &[LOGGING_DISABLED_CHECK.to_string()])
        .await;
    let report = emulator.reported_audit(allowed, &task_id).await;
    assert_eq!(
        reported_check_names(&report),
        BTreeSet::from([LOGGING_DISABLED_CHECK.to_string()]),
        "case {allowed}: expected the audit to carry the one targeted check, got {report}"
    );
}

/// A `taskId` the account holds no audit under is a 404, never a hollow 200.
#[tokio::test]
async fn describe_audit_task_refuses_an_id_no_audit_was_started_under() {
    let emulator = Emulator::new();
    // Ids of different shapes, each one `AuditTaskId` admits, so the refusal is about the
    // account holding no such audit and not about the id's form.
    let cases = [
        "a",
        "0123456789abcdef",
        "6f1a9c3e-2b7d-4f80-9a1c-5d3e7b2f4a60",
    ];
    for task_id in &cases {
        let reply = emulator.describe_audit(task_id).await;
        assert_eq!(
            reply.status,
            404,
            "case {task_id:?}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {task_id:?}: expected ResourceNotFoundException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.error_message(),
            format!("Audit task {task_id} does not exist."),
            "case {task_id:?}: expected the refusal to name the audit asked about, got {}",
            reply.text()
        );
    }
}
