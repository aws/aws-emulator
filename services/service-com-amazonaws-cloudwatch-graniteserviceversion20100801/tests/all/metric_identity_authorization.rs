// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION of the metric operations this service judges
//! over every resource, with IAM and the token service mounted beside it on ONE engine — so
//! the principal a call is judged as is the one THOSE services minted, carrying the documents
//! they wrote.
//!
//! RULE: a call to a CloudWatch operation whose IAM action supports no resource types
//! (`cloudwatch:PutMetricData` and the metric reads beside it, which the IAM Service
//! Authorization Reference lists under "actions with no resource types", so IAM judges each
//! over every resource) is decided against the CALLING PRINCIPAL's identity-based policies
//! before this service reads or writes anything: the principal being the identity the access
//! key id the request presents speaks for, and for an `sts:AssumeRole` credential that is the
//! session, whose permissions are the role's. The decision is IAM's own — an explicit `Deny`
//! about the request wins over every `Allow`, an `Allow` about it permits, and a principal
//! with neither is refused — and a grant scoped to one resource ARN does not cover an action
//! judged over every resource. A refusal is HTTP 403 under the code a client branches on for
//! this class, naming the principal, the action and the rule that withheld the grant, and the
//! call persists nothing.
//!
//! The rows span what the rule generalises over: which operation is asked for (a publish and
//! each of the metric reads), whether the principal holds no policy / a grant of that action /
//! a grant of every action of this service / a grant of ANOTHER action of it / a grant of
//! another service's actions / a grant plus an explicit deny / a grant scoped to one resource
//! ARN, and which protocol the call arrives in. Beside the table: an operation whose action
//! IAM judges against the ARN of the alarm it addresses is NOT decided here, the account's own
//! published credential keeps being served, and a refused publish leaves the series empty.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below signs for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix this service's model declares
/// (`smithy_query(query="service_info")`: service shape `GraniteServiceVersion20100801`).
const TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// This service's signing name, which is what routes a request to it.
const SIGNING_NAME: &str = "monitoring";

/// The API version an `awsQuery` call of this service names.
const API_VERSION: &str = "2010-08-01";

/// The error code an authorization refusal carries.
const ACCESS_DENIED: &str = "AccessDenied";

/// The status it carries.
const DENIED_STATUS: u16 = 403;

/// The namespace every datapoint and every read below names: a custom one, so nothing here
/// touches a reserved `AWS/` namespace.
const NAMESPACE: &str = "Emulator/Authorization";

/// The metric every datapoint and every read below names.
const METRIC: &str = "Probe";

/// The access-key id standing for the account's own published credential: neither the session
/// directory nor IAM holds a record for it, so it resolves to the account's ambient identity,
/// which holds no IAM policies — the position AWS account root credentials are in. Every
/// fixture is made with it.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// This service beside the IAM and token services its callers are made with, all on ONE
/// shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope, which
    // is where a service reads the caller's access key id and Region from.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `access_key_id` and `signing_name` in [`REGION`].
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// One call to this service in its `awsJson1_0` protocol, signed with `access_key_id`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(access_key_id, SIGNING_NAME))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One call to this service in its `awsQuery` protocol, signed with `access_key_id`, as the
/// status and the raw XML it answered.
async fn query_call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, SIGNING_NAME))
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One `awsQuery` call to IAM or the token service, signed with the account's own credential,
/// which must succeed; answers the raw XML.
async fn setup_query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(ACCOUNT_KEY, signing_name))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action:?}: the setup call must succeed, got {status} with {text}"
    );
    text
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Create an IAM role carrying one inline policy per document in `policies`, assume it through
/// the token service, and answer the session's access key id with the session ARN.
async fn assumed_role(
    router: &MultiServiceRouter,
    role_name: &str,
    policies: &[String],
) -> Principal {
    let account = engine::emulator_account_id();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    setup_query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    // SEVERAL inline policies, so a decision taken over the union of the documents in force
    // is distinguishable from one taken inside a single document.
    for (index, policy) in policies.iter().enumerate() {
        setup_query(
            router,
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", format!("inline-{index}")),
                ("PolicyDocument", policy.clone()),
            ],
        )
        .await;
    }
    let assumed = setup_query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
            ("RoleSessionName", SESSION.to_string()),
        ],
    )
    .await;
    Principal {
        access_key_id: xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
        arn: xml_element(&assumed, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
    }
}

/// The epoch seconds a read's window is stated around, as this service's JSON wire carries a
/// timestamp.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The request body of one operation under test.
fn body_of(operation: &str) -> Value {
    let now = now_seconds();
    match operation {
        "PutMetricData" => json!({
            "Namespace": NAMESPACE,
            "MetricData": [{ "MetricName": METRIC, "Value": 1.0 }],
        }),
        "ListMetrics" => json!({ "Namespace": NAMESPACE }),
        "GetMetricStatistics" => json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
        "GetMetricData" => json!({
            "MetricDataQueries": [{
                "Id": "probe",
                "MetricStat": {
                    "Metric": { "Namespace": NAMESPACE, "MetricName": METRIC },
                    "Period": 60,
                    "Stat": "Sum",
                },
            }],
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
        }),
        "DescribeAlarmsForMetric" => json!({ "Namespace": NAMESPACE, "MetricName": METRIC }),
        "PutMetricAlarm" => json!({
            "AlarmName": "authorization-probe",
            "ComparisonOperator": "GreaterThanThreshold",
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Period": 60,
            "Statistic": "Sum",
        }),
        other => panic!("no request body is stated for {other}"),
    }
}

/// One row: the documents the calling role carries, the operation it asks for, and whether
/// IAM lets it.
struct Row {
    case: &'static str,
    /// The role name the row's principal is made under, which also names its inline policies.
    role: &'static str,
    /// The identity-based documents in force on that role.
    policies: Vec<String>,
    /// The operation the row calls.
    operation: &'static str,
    /// Whether the call must be served.
    allowed: bool,
}

/// The rows of the table below.
fn rows() -> Vec<Row> {
    let account = engine::emulator_account_id();
    let one_alarm = format!("arn:aws:cloudwatch:{REGION}:{account}:alarm:authorization-probe");
    vec![
        Row {
            case: "a grant of the action asked for",
            role: "cw-grant-put",
            policies: vec![statement("Allow", "cloudwatch:PutMetricData", "*")],
            operation: "PutMetricData",
            allowed: true,
        },
        Row {
            case: "a grant of every action of this service, publishing",
            role: "cw-grant-all",
            policies: vec![statement("Allow", "cloudwatch:*", "*")],
            operation: "PutMetricData",
            allowed: true,
        },
        Row {
            case: "a grant of every action of this service, listing the catalogue",
            role: "cw-grant-all-list",
            policies: vec![statement("Allow", "cloudwatch:*", "*")],
            operation: "ListMetrics",
            allowed: true,
        },
        Row {
            case: "a grant of the read asked for",
            role: "cw-grant-read",
            policies: vec![statement("Allow", "cloudwatch:GetMetricStatistics", "*")],
            operation: "GetMetricStatistics",
            allowed: true,
        },
        Row {
            case: "a grant of ANOTHER action of this service, asked for a read",
            role: "cw-grant-other",
            policies: vec![statement("Allow", "cloudwatch:PutMetricData", "*")],
            operation: "GetMetricStatistics",
            allowed: false,
        },
        Row {
            case: "a grant of the retrieving read",
            role: "cw-grant-get-data",
            policies: vec![statement("Allow", "cloudwatch:GetMetricData", "*")],
            operation: "GetMetricData",
            allowed: true,
        },
        Row {
            case: "no policy at all, asked for the alarm listing of a metric",
            role: "cw-no-policy-alarms",
            policies: Vec::new(),
            operation: "DescribeAlarmsForMetric",
            allowed: false,
        },
        Row {
            case: "a grant of every action of this service, asked for that listing",
            role: "cw-grant-all-alarms",
            policies: vec![statement("Allow", "cloudwatch:*", "*")],
            operation: "DescribeAlarmsForMetric",
            allowed: true,
        },
        Row {
            case: "no policy at all",
            role: "cw-no-policy",
            policies: Vec::new(),
            operation: "PutMetricData",
            allowed: false,
        },
        Row {
            case: "a grant of another service's actions",
            role: "cw-other-service",
            policies: vec![statement("Allow", "logs:*", "*")],
            operation: "PutMetricData",
            allowed: false,
        },
        Row {
            case: "a grant of every action of this service and an explicit deny of this one",
            role: "cw-denied",
            policies: vec![
                statement("Allow", "cloudwatch:*", "*"),
                statement("Deny", "cloudwatch:PutMetricData", "*"),
            ],
            operation: "PutMetricData",
            allowed: false,
        },
        Row {
            case: "a grant scoped to one alarm's ARN",
            role: "cw-scoped-grant",
            policies: vec![statement("Allow", "cloudwatch:PutMetricData", &one_alarm)],
            operation: "PutMetricData",
            allowed: false,
        },
        Row {
            case: "a grant spelled with this service's endpoint prefix",
            role: "cw-endpoint-prefix",
            policies: vec![statement("Allow", "monitoring:PutMetricData", "*")],
            operation: "PutMetricData",
            allowed: false,
        },
    ]
}

/// RULE: a metric operation whose IAM action supports no resource types is served only to a
/// caller the identity-based policies in force on it allow to perform `cloudwatch:<that
/// operation>` over every resource; every other caller is refused 403 `AccessDenied` naming
/// the principal, the action and the rule that withheld the grant.
#[tokio::test]
async fn a_metric_call_is_decided_against_the_callers_identity_policies() {
    let router = emulator();
    for row in rows() {
        let Row {
            case,
            role,
            policies,
            operation,
            allowed,
        } = row;
        let principal = assumed_role(&router, role, &policies).await;
        let (status, answered) =
            call_as(&router, &principal.access_key_id, operation, &body_of(operation)).await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to be served, got {status} {answered}"
            );
            continue;
        }
        assert_eq!(
            status, DENIED_STATUS,
            "case {case:?}: expected {operation} to be refused {DENIED_STATUS}, got {status} \
             {answered}"
        );
        assert_eq!(
            answered["__type"],
            json!(ACCESS_DENIED),
            "case {case:?}: expected the {ACCESS_DENIED} class, got {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(&format!(
                "User: {} is not authorized to perform: cloudwatch:{operation}",
                principal.arn
            )),
            "case {case:?}: the refusal must name the session and the action, got {message:?}"
        );
        assert!(
            message.contains("on resource: *"),
            "case {case:?}: the refusal must name the resource it was judged against, got \
             {message:?}"
        );
    }
}

/// RULE: the decision is about the CALL, so it is reached the same way whichever of this
/// service's protocols the call arrives in — the refusal is rendered in that protocol's own
/// error envelope.
#[tokio::test]
async fn the_refusal_is_rendered_in_the_protocol_the_call_arrived_in() {
    let router = emulator();
    let refused = assumed_role(&router, "cw-query-refused", &[]).await;
    let allowed = assumed_role(
        &router,
        "cw-query-allowed",
        &[statement("Allow", "cloudwatch:ListMetrics", "*")],
    )
    .await;
    let params = [("Namespace", NAMESPACE.to_string())];
    let (status, answered) =
        query_call_as(&router, &refused.access_key_id, "ListMetrics", &params).await;
    assert_eq!(
        status, DENIED_STATUS,
        "case \"a query call with no grant\": expected {DENIED_STATUS}, got {status} {answered}"
    );
    assert_eq!(
        xml_element(&answered, "Code").as_deref(),
        Some(ACCESS_DENIED),
        "case \"a query call with no grant\": expected the {ACCESS_DENIED} code, got {answered}"
    );
    assert_eq!(
        xml_element(&answered, "Type").as_deref(),
        Some("Sender"),
        "case \"a query call with no grant\": a refusal of the caller is a sender fault, got \
         {answered}"
    );
    let message = xml_element(&answered, "Message").unwrap_or_default();
    assert!(
        message.contains(&refused.arn) && message.contains("cloudwatch:ListMetrics"),
        "case \"a query call with no grant\": the refusal must name the session and the action, \
         got {message:?}"
    );
    // The envelope a query failure of this service carries names the request it answered.
    assert!(
        xml_element(&answered, "RequestId").is_some_and(|id| !id.is_empty()),
        "case \"a query call with no grant\": the query error envelope names the request, got \
         {answered}"
    );
    let (status, served) =
        query_call_as(&router, &allowed.access_key_id, "ListMetrics", &params).await;
    assert_eq!(
        status, 200,
        "case \"a query call with the grant\": expected the call to be served, got {status} \
         {served}"
    );
}

/// RULE: only the actions IAM judges over every resource are decided in front of the
/// operation; an action IAM judges against the ARN of the alarm the request addresses is left
/// to the handler that resolves it, so a caller holding no grant of it is still served.
#[tokio::test]
async fn an_action_judged_against_a_named_resource_is_not_decided_here() {
    let router = emulator();
    let principal = assumed_role(&router, "cw-alarm-writer", &[]).await;
    let (status, answered) = call_as(
        &router,
        &principal.access_key_id,
        "PutMetricAlarm",
        &body_of("PutMetricAlarm"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an alarm write with no grant\": expected the call to be served, got {status} \
         {answered}"
    );
}

/// RULE: the account's own published credential holds no IAM policies — the position AWS
/// account root credentials are in — so a call signed with it is served.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let router = emulator();
    for operation in [
        "PutMetricData",
        "ListMetrics",
        "GetMetricStatistics",
        "GetMetricData",
        "DescribeAlarmsForMetric",
    ] {
        let (status, answered) = call_as(&router, ACCOUNT_KEY, operation, &body_of(operation)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the account's own credential to be served, got \
             {status} {answered}"
        );
    }
}

/// RULE: a refused call persists nothing — the datapoint a publish the policies do not allow
/// carried does not land, so a reader sees no series for it.
#[tokio::test]
async fn a_refused_publish_lands_no_datapoint() {
    let router = emulator();
    let principal = assumed_role(&router, "cw-refused-publisher", &[]).await;
    let (status, refused) = call_as(
        &router,
        &principal.access_key_id,
        "PutMetricData",
        &body_of("PutMetricData"),
    )
    .await;
    assert_eq!(
        status, DENIED_STATUS,
        "the publish must be refused, got {status} {refused}"
    );
    let (status, listed) = call_as(&router, ACCOUNT_KEY, "ListMetrics", &body_of("ListMetrics")).await;
    assert_eq!(status, 200, "ListMetrics answered {status} {listed}");
    let metrics = listed["Metrics"].as_array().cloned().unwrap_or_default();
    assert!(
        metrics.is_empty(),
        "a refused publish must land no datapoint, got {listed}"
    );
    let (status, stats) = call_as(
        &router,
        ACCOUNT_KEY,
        "GetMetricStatistics",
        &body_of("GetMetricStatistics"),
    )
    .await;
    assert_eq!(status, 200, "GetMetricStatistics answered {status} {stats}");
    let points = stats["Datapoints"].as_array().cloned().unwrap_or_default();
    assert!(
        points.is_empty(),
        "a refused publish must land no datapoint, got {stats}"
    );
}
