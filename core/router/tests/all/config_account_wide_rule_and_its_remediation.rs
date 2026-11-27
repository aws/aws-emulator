// SPDX-License-Identifier: CC0-1.0

//! What an ACCOUNT-WIDE AWS Config managed rule places the account at, and what the
//! remediation of the finding it records then does, on the assembled emulator.
//!
//! RULE: a Config managed rule whose subject is the ACCOUNT is evaluated against the account
//! pseudo-resource (`AWS::::Account` carrying the account id) from the configuration the
//! service that OWNS that configuration reports, so the rule records a
//! `COMPLIANT`/`NON_COMPLIANT` verdict for the account on the evaluation pass its write owes
//! — computed from what that service answered and from the rule's own `InputParameters`. A
//! resource a rule finds `NON_COMPLIANT`, the account included, is one
//! `StartRemediationExecution` ACCEPTS: the start answers an EMPTY `FailedItems` and the
//! execution it records settles at a terminal `RemediationExecutionState`, so a client
//! following the documented remediation workflow — start, then poll the status — is never
//! left polling an answer that cannot arrive. A rule with NO remediation configuration is
//! refused 400 `NoSuchRemediationConfigurationException` by both the start and the status
//! read, each worded as that operation words it.
//!
//! The rows of the first test differ along what the rule generalises over: which account-wide
//! managed rule of the catalogue the account holds (both the ones this emulator evaluates,
//! including the one the report did not cite), what the owning service reports about the
//! account (nothing at all, a configuration that satisfies the rule, one that does not), and
//! which requirements the rule's own parameters state (none, one the configuration meets, one
//! it does not).
//!
//! The suite lives here because the behaviour spans crates: AWS CloudTrail holds the
//! account's trails and the logging state of each, IAM holds the account's password policy,
//! Amazon S3 holds the bucket a trail delivers into, and Systems Manager owns and RUNS the
//! automation document a remediation names — a crate-local test can only ever see one side of
//! those calls. Every call is made the way a client makes it, over the wire through the same
//! `MultiServiceRouter` the production router builds.
//!
//! Nothing here asserts on elapsed time: each wait is for the CONDITION the clock owes — an
//! evaluation pass, a started run reaching its end — polled under a generous deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder this crate writes ONCE (`tests/all/query_wire.rs`): a suite that
// writes its own drifts from the others.
use super::query_wire::form_encode;

/// The Region every request here is signed for, and so the Region each resource belongs to.
const REGION: &str = "us-east-1";

/// Config's `X-Amz-Target` prefix, from its model's service shape, and the signing name its
/// credential scope carries.
const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";
/// CloudTrail's `X-Amz-Target` prefix, from that crate's `OPERATIONS.md`.
const CLOUDTRAIL_TARGET_PREFIX: &str = "CloudTrail_20131101";
/// Systems Manager's `X-Amz-Target` prefix, from that crate's `OPERATIONS.md`.
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// IAM's awsQuery API version, from that crate's `OPERATIONS.md` wire-protocol section.
const IAM_VERSION: &str = "2010-05-08";

/// The resource type Config evaluates an account-wide rule against: the account
/// pseudo-resource type ("Config accepts `AWS::::Account` resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// `ComplianceType` values an account-wide rule places the account at
/// (`smithy_query(query="shape", shape="ComplianceType")`).
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// `RemediationExecutionState` values this suite reads back
/// (`smithy_query(query="shape", shape="RemediationExecutionState")`: `QUEUED`,
/// `IN_PROGRESS`, `SUCCEEDED`, `FAILED`, `UNKNOWN`).
const SUCCEEDED: &str = "SUCCEEDED";
/// See [`SUCCEEDED`].
const FAILED: &str = "FAILED";

/// The account-wide AWS managed rules this emulator evaluates, from the *List of Config
/// Managed Rules* that `Source$SourceIdentifier` points at: `CLOUD_TRAIL_ENABLED` ("Checks
/// if an CloudTrail trail is enabled in your Amazon Web Services account", optionally
/// checking the `s3BucketName`, `snsTopicArn` and `cloudWatchLogsLogGroupArn` the trail
/// delivers to) and `IAM_PASSWORD_POLICY` ("Checks if the account password policy for users
/// meets the specified requirements indicated in the parameters").
const CLOUD_TRAIL_ENABLED: &str = "CLOUD_TRAIL_ENABLED";
/// See [`CLOUD_TRAIL_ENABLED`].
const IAM_PASSWORD_POLICY: &str = "IAM_PASSWORD_POLICY";

/// The only published `RemediationTargetType`: an SSM automation document.
const TARGET_TYPE: &str = "SSM_DOCUMENT";

/// The `Parameters` key a remediation hands the role it runs as in, which an automation
/// document declares as a parameter of its own.
const AUTOMATION_ASSUME_ROLE: &str = "AutomationAssumeRole";

/// The bucket a trail of this suite delivers its log files into.
const TRAIL_BUCKET: &str = "sim2real-trail-logs";

/// Another bucket name, which no trail of this suite delivers into: what a rule's parameter
/// names when the account's trail delivers somewhere else.
const OTHER_BUCKET: &str = "sim2real-audit-elsewhere";

/// The trail each row that holds one creates.
const TRAIL_NAME: &str = "account-trail";

/// Generously long for the spans the clock owes here — an evaluation pass, a started
/// automation being walked to its end. The suite polls for the condition and stops as soon as
/// it holds, so this bounds only a failure.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(60);

/// How long a poll waits between reads: short enough that the wait is the condition's own,
/// long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Config, CloudTrail, IAM, S3 and Systems Manager over ONE engine.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so signatures are not verified; the credential scope
    // is still carried, because it is what names the Region and routes the request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsJson1_1 RPC call: `POST /` carrying the target header the SDKs send.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One Config call, answering its status and body.
async fn config(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    rpc(router, "config", CONFIG_TARGET_PREFIX, operation, request).await
}

/// One Config call that must be served.
async fn config_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, body) = config(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served, got {status} with {body}"
    );
    body
}

/// One CloudTrail call that must be served.
async fn cloudtrail_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, body) = rpc(
        router,
        "cloudtrail",
        CLOUDTRAIL_TARGET_PREFIX,
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served, got {status} with {body}"
    );
    body
}

/// One Systems Manager call that must be served.
async fn ssm_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
    let (status, body) = rpc(router, "ssm", SSM_TARGET_PREFIX, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served, got {status} with {body}"
    );
    body
}

/// One awsQuery call to IAM, every component percent-encoded so a document travels verbatim.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={IAM_VERSION}");
    for (name, value) in members {
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
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One S3 call on the path-style address.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("s3"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The wire error code of an awsJson refusal: the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The bucket policy a client attaches so CloudTrail may deliver a trail's log files into
/// `bucket`: the ACL check on the bucket and the object write under it, granted to the
/// CloudTrail service principal and scoped by the canned ACL the delivery carries — the
/// policy CloudTrail's own documentation states.
fn cloudtrail_bucket_policy(bucket: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSCloudTrailAclCheck",
                "Effect": "Allow",
                "Principal": { "Service": "cloudtrail.amazonaws.com" },
                "Action": "s3:GetBucketAcl",
                "Resource": format!("arn:aws:s3:::{bucket}"),
            },
            {
                "Sid": "AWSCloudTrailWrite",
                "Effect": "Allow",
                "Principal": { "Service": "cloudtrail.amazonaws.com" },
                "Action": "s3:PutObject",
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
                "Condition": {
                    "StringEquals": { "s3:x-amz-acl": "bucket-owner-full-control" },
                },
            },
        ],
    })
    .to_string()
}

/// Hold the bucket `bucket` with the policy CloudTrail requires of a trail's destination, so
/// a trail can be created against it.
async fn trail_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let (status, created) = s3(router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateBucket({bucket:?}) must succeed, got {status} with {created}"
    );
    let (status, answered) = s3(
        router,
        "PUT",
        &format!("/{bucket}?policy"),
        &cloudtrail_bucket_policy(bucket),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: setup PutBucketPolicy({bucket:?}) must succeed, got {status} with \
         {answered}"
    );
}

/// Hold a trail delivering into `bucket`, and start its logging when `logging` — a trail
/// CloudTrail created is not logging until something starts it.
async fn account_trail(router: &MultiServiceRouter, case: &str, bucket: &str, logging: bool) {
    trail_bucket(router, case, bucket).await;
    cloudtrail_ok(
        router,
        case,
        "CreateTrail",
        &json!({ "Name": TRAIL_NAME, "S3BucketName": bucket }),
    )
    .await;
    if logging {
        cloudtrail_ok(router, case, "StartLogging", &json!({ "Name": TRAIL_NAME })).await;
    }
}

/// The trust policy that lets Config assume the configuration recorder's role: one `Allow`
/// of `sts:AssumeRole` to the Config service principal.
fn config_trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "config.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Hold the configuration recorder every Config rule write needs ("You must create a
/// configuration recorder before you can create or update a Config rule"), with a role IAM
/// holds and Config can assume.
///
/// The recorder is not started: the pass that evaluates an account-wide rule is the one its
/// own write owes ("when you create a rule, Config evaluates your resources against the rule
/// automatically"), and the account's configuration is no resource a recorder discovers.
async fn with_a_configuration_recorder(router: &MultiServiceRouter, case: &str) {
    let role = "sim2real-config-recorder-role";
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role),
            ("AssumeRolePolicyDocument", &config_trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateRole({role:?}) must succeed, got {status} with {created}"
    );
    let role_arn = format!(
        "arn:aws:iam::{}:role/{role}",
        engine::emulator_account_id()
    );
    config_ok(
        router,
        case,
        "PutConfigurationRecorder",
        &json!({
            "ConfigurationRecorder": {
                "name": "the-recorder",
                "roleARN": role_arn,
                "recordingGroup": { "allSupported": true },
            }
        }),
    )
    .await;
}

/// The `PutConfigRule` body for the account-wide managed rule `identifier` under `name`,
/// stating `parameters` as its `InputParameters` where the row carries any.
fn account_rule(name: &str, identifier: &str, parameters: Option<String>) -> Value {
    let mut rule = json!({
        "ConfigRuleName": name,
        "Source": { "Owner": "AWS", "SourceIdentifier": identifier },
    });
    if let Some(parameters) = parameters {
        rule["InputParameters"] = json!(parameters);
    }
    json!({ "ConfigRule": rule })
}

/// The account key every remediation of this suite names: the account pseudo-resource, whose
/// id is the account this emulator serves.
fn account_key() -> Value {
    json!({
        "resourceType": ACCOUNT_RESOURCE_TYPE,
        "resourceId": engine::emulator_account_id(),
    })
}

/// Wait until `rule` holds a verdict about the ACCOUNT and answer it: the evaluation is a
/// pass the clock owes, so the suite looks again until it has happened rather than waiting
/// out a fixed span.
async fn account_verdict(router: &MultiServiceRouter, case: &str, rule: &str) -> String {
    let account = engine::emulator_account_id();
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let body = config_ok(
            router,
            case,
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
        let held: Vec<(String, String, String)> = body["EvaluationResults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|result| {
                let qualifier =
                    &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
                let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
                (
                    text(&qualifier["ResourceType"]),
                    text(&qualifier["ResourceId"]),
                    text(&result["ComplianceType"]),
                )
            })
            .collect();
        if let Some((_, _, compliance)) = held
            .iter()
            .find(|(resource_type, resource_id, _)| {
                resource_type == ACCOUNT_RESOURCE_TYPE && resource_id == account
            })
            .cloned()
        {
            return compliance;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rule {rule:?} must place the account at a verdict, got {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What the owning service holds about the account for one row.
#[derive(Debug, Clone, Copy)]
enum AccountHolds {
    /// Nothing at all: no trail, and no password policy.
    Nothing,
    /// One trail delivering into [`TRAIL_BUCKET`], logging or not.
    Trail { logging: bool },
    /// A password policy requiring passwords of at least this many characters.
    PasswordPolicy { minimum_length: u32 },
}

/// One row of the verdict table.
struct Row {
    case: &'static str,
    identifier: &'static str,
    /// What the rule's own `InputParameters` require, or `None` for a rule that states none.
    parameters: Option<Value>,
    holds: AccountHolds,
    verdict: &'static str,
}

/// Every row of the verdict table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the account holds no trail at all",
            identifier: CLOUD_TRAIL_ENABLED,
            parameters: None,
            holds: AccountHolds::Nothing,
            verdict: NON_COMPLIANT,
        },
        Row {
            case: "a trail that is logging",
            identifier: CLOUD_TRAIL_ENABLED,
            parameters: None,
            holds: AccountHolds::Trail { logging: true },
            verdict: COMPLIANT,
        },
        Row {
            case: "a trail whose logging was never started",
            identifier: CLOUD_TRAIL_ENABLED,
            parameters: None,
            holds: AccountHolds::Trail { logging: false },
            verdict: NON_COMPLIANT,
        },
        Row {
            case: "a logging trail delivering into the bucket the rule names",
            identifier: CLOUD_TRAIL_ENABLED,
            parameters: Some(json!({ "s3BucketName": TRAIL_BUCKET })),
            holds: AccountHolds::Trail { logging: true },
            verdict: COMPLIANT,
        },
        Row {
            case: "a logging trail delivering somewhere other than the rule names",
            identifier: CLOUD_TRAIL_ENABLED,
            parameters: Some(json!({ "s3BucketName": OTHER_BUCKET })),
            holds: AccountHolds::Trail { logging: true },
            verdict: NON_COMPLIANT,
        },
        Row {
            case: "the account has set no password policy",
            identifier: IAM_PASSWORD_POLICY,
            parameters: None,
            holds: AccountHolds::Nothing,
            verdict: NON_COMPLIANT,
        },
        Row {
            case: "a password policy longer than the rule asks for",
            identifier: IAM_PASSWORD_POLICY,
            parameters: Some(json!({ "MinimumPasswordLength": "8" })),
            holds: AccountHolds::PasswordPolicy { minimum_length: 14 },
            verdict: COMPLIANT,
        },
        Row {
            case: "a password policy shorter than the rule asks for",
            identifier: IAM_PASSWORD_POLICY,
            parameters: Some(json!({ "MinimumPasswordLength": "20" })),
            holds: AccountHolds::PasswordPolicy { minimum_length: 14 },
            verdict: NON_COMPLIANT,
        },
    ]
}

/// Put the account into the configuration `holds` describes, through the API of the service
/// that owns it.
async fn with_account_configuration(
    router: &MultiServiceRouter,
    case: &str,
    holds: AccountHolds,
) {
    match holds {
        AccountHolds::Nothing => {}
        AccountHolds::Trail { logging } => {
            account_trail(router, case, TRAIL_BUCKET, logging).await;
        }
        AccountHolds::PasswordPolicy { minimum_length } => {
            let (status, answered) = iam(
                router,
                "UpdateAccountPasswordPolicy",
                &[("MinimumPasswordLength", &minimum_length.to_string())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: setup UpdateAccountPasswordPolicy must succeed, got {status} \
                 with {answered}"
            );
        }
    }
}

#[tokio::test]
async fn an_account_wide_rule_places_the_account_from_what_the_owning_service_reports() {
    for row in rows() {
        let Row {
            case,
            identifier,
            parameters,
            holds,
            verdict,
        } = row;
        // One account per row: the configuration a rule judges is the account's own, so each
        // row is provisioned from nothing.
        let router = build_router();
        with_a_configuration_recorder(&router, case).await;
        // The account's configuration is in place BEFORE the rule is written, so the pass that
        // write owes evaluates the configuration the row is about.
        with_account_configuration(&router, case, holds).await;

        let rule = "account-guardrail";
        config_ok(
            &router,
            case,
            "PutConfigRule",
            &account_rule(rule, identifier, parameters.map(|value| value.to_string())),
        )
        .await;

        let placed = account_verdict(&router, case, rule).await;
        assert_eq!(
            placed, verdict,
            "case {case:?}: expected the rule to place the account at {verdict}, got {placed}"
        );
    }
}

/// The automation document a remediation of this suite runs: the account's own runbook, whose
/// one step waits a moment — so what the run does is Systems Manager's own walk of it.
const WAITING_RUNBOOK: &str = "wait-a-moment";

/// That document's content, in the automation schema a runbook declares.
fn waiting_runbook() -> String {
    json!({
        "schemaVersion": "0.3",
        "assumeRole": "{{ AutomationAssumeRole }}",
        "parameters": { AUTOMATION_ASSUME_ROLE: { "type": "String" } },
        "mainSteps": [{
            "name": "wait",
            "action": "aws:sleep",
            "inputs": { "Duration": "PT1S" },
        }],
    })
    .to_string()
}

/// How `StartRemediationExecution` words the refusal of a rule that has no remediation
/// configuration: the fixed sentence and the rule the REQUEST named, with no trailing stop.
fn start_refusal(rule: &str) -> String {
    format!("There is no applicable RemediationConfiguration setup for AWS Config rule {rule}")
}

/// How `DescribeRemediationExecutionStatus` words the same precondition: a different
/// sentence, and one that does end in a stop.
fn status_refusal(rule: &str) -> String {
    format!("There is no RemediationConfiguration setup for AWS Config rule {rule}.")
}

#[tokio::test]
async fn the_remediation_of_an_account_a_rule_finds_non_compliant_settles_and_is_never_a_wait() {
    let case = "the account-wide remediation arc";
    let router = build_router();
    with_a_configuration_recorder(&router, case).await;

    // Two rules under the same account-wide managed rule, one with a remediation configuration
    // and one with none: what tells the refusal of a rule that has no remediation apart from
    // the acceptance of one that has.
    let remediated = "account-remediated";
    let unremediated = "account-unremediated";
    for rule in [remediated, unremediated] {
        config_ok(
            &router,
            case,
            "PutConfigRule",
            &account_rule(rule, CLOUD_TRAIL_ENABLED, None),
        )
        .await;
    }

    // The account holds no trail, so each rule finds the ACCOUNT non-compliant — the finding a
    // remediation acts on.
    for rule in [remediated, unremediated] {
        let placed = account_verdict(&router, case, rule).await;
        assert_eq!(
            placed, NON_COMPLIANT,
            "case {case:?}: the rule {rule:?} must find the account non-compliant, got {placed}"
        );
    }

    // The remediation one of them carries: the account's own automation document, run with the
    // role the configuration states.
    ssm_ok(
        &router,
        case,
        "CreateDocument",
        &json!({
            "Name": WAITING_RUNBOOK,
            "DocumentType": "Automation",
            "DocumentFormat": "JSON",
            "Content": waiting_runbook(),
        }),
    )
    .await;
    config_ok(
        &router,
        case,
        "PutRemediationConfigurations",
        &json!({
            "RemediationConfigurations": [{
                "ConfigRuleName": remediated,
                "TargetType": TARGET_TYPE,
                "TargetId": WAITING_RUNBOOK,
                "Automatic": false,
                "Parameters": {
                    AUTOMATION_ASSUME_ROLE: {
                        "StaticValue": {
                            "Values": [format!(
                                "arn:aws:iam::{}:role/config-remediation",
                                engine::emulator_account_id(),
                            )],
                        },
                    },
                },
            }],
        }),
    )
    .await;

    // The rule carrying NO remediation configuration: both the start and the status read refuse
    // it 400 `NoSuchRemediationConfigurationException`, each with its own wording, and the
    // refusal leaves nothing behind to describe.
    for (operation, expected) in [
        ("StartRemediationExecution", start_refusal(unremediated)),
        (
            "DescribeRemediationExecutionStatus",
            status_refusal(unremediated),
        ),
    ] {
        let (status, body) = config(
            &router,
            operation,
            &json!({ "ConfigRuleName": unremediated, "ResourceKeys": [account_key()] }),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&body),
                body["message"].as_str().unwrap_or_default().to_string(),
            ),
            (
                400,
                "NoSuchRemediationConfigurationException".to_string(),
                expected.clone(),
            ),
            "case {case:?}: expected {operation} on a rule with no remediation configuration to \
             be refused 400 NoSuchRemediationConfigurationException {expected:?}, got {status} \
             with {body}"
        );
    }

    // The positive control: the key the rule finds non-compliant is ACCEPTED, so the start
    // answers an EMPTY `FailedItems` — the answer a client looping on that member waits for.
    // The loop is bounded: a start that keeps reporting the key back is the defect.
    let request = json!({ "ConfigRuleName": remediated, "ResourceKeys": [account_key()] });
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    let accepted = loop {
        let answered = config_ok(&router, case, "StartRemediationExecution", &request).await;
        let failed = answered["FailedItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if failed.is_empty() {
            break answered;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the start for the account key must be accepted with an empty \
             FailedItems, last answer {answered}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        accepted["FailureMessage"].as_str(),
        None,
        "case {case:?}: an accepted start explains no failure, got {accepted}"
    );

    // And the execution it recorded SETTLES: the status read reports the account key and
    // reaches a terminal state — for a run Systems Manager performed, SUCCEEDED.
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    let settled = loop {
        let body = config_ok(
            &router,
            case,
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": remediated, "ResourceKeys": [account_key()] }),
        )
        .await;
        let reported = body["RemediationExecutionStatuses"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let entry = reported
            .iter()
            .find(|entry| {
                entry["ResourceKey"]["resourceId"].as_str()
                    == Some(engine::emulator_account_id())
            })
            .cloned();
        if let Some(entry) = entry.filter(|entry| {
            matches!(entry["State"].as_str(), Some(state) if state == SUCCEEDED || state == FAILED)
        }) {
            break entry;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the execution for the account key must reach a terminal state, last \
             read {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        settled["State"].as_str(),
        Some(SUCCEEDED),
        "case {case:?}: the run of a runbook whose one step waits cannot fail, got {settled}"
    );
    assert_eq!(
        settled["ResourceKey"]["resourceType"].as_str(),
        Some(ACCOUNT_RESOURCE_TYPE),
        "case {case:?}: the execution is about the account pseudo-resource, got {settled}"
    );
}
