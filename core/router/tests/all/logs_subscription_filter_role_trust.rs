// SPDX-License-Identifier: CC0-1.0

//! A subscription delivery is configured only under a role CloudWatch Logs can ASSUME.
//!
//! RULE: a put that configures a delivery to a streaming destination names the role the
//! service delivers under (`PutSubscriptionFilter$roleArn`, "an IAM role that grants CloudWatch
//! Logs permissions to deliver ingested log events to the destination stream"), and CloudWatch
//! Logs validates that wiring while it configures it: it assumes the role and delivers a test
//! message to the stream. A role whose trust policy does not let this service's own principal
//! assume it cannot be assumed, so the put is refused with `InvalidParameterException` — the
//! test message could not be delivered to that kind of stream — and no filter is stored. A role
//! the trust document DOES admit is configured, and so is one this account's IAM holds nothing
//! under, a delivery role being routinely provisioned out of band: a refusal has to be grounded
//! in what IAM answered.
//!
//! This suite needs CloudWatch Logs, IAM and Kinesis on one engine — CloudWatch Logs reaches
//! IAM and the destination through the in-process callback, so a single-service router has
//! neither to ask — which is why it lives here rather than in the CloudWatch Logs crate. What a
//! role IAM DOES admit is then ALLOWED to write is the neighbouring
//! `logs_subscription_delivery_role.rs`.
//!
//! Every row drives BOTH operations that name a role CloudWatch Logs delivers under, because the
//! rule is one: the filter's own `roleArn` and the `roleArn` a `PutDestination` logical
//! destination carries for its target.
//!
//! The rows differ along what the rule generalises over: which service principal the trust
//! document names (this service, another service, both), the effect it states (allow, deny),
//! the action it names (the assume itself, a wildcard covering it, another action), whether IAM
//! holds the role at all, and which kind of streaming destination the filter names — the kind
//! being what the refusal reports the undeliverable test message for.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the destination ARNs name.
const REGION: &str = "us-west-2";

/// The filter every row configures.
const FILTER: &str = "to-stream";

/// The refusal a role this service cannot assume is answered with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The three services this suite exercises, mounted on one engine exactly as the router mounts
/// them, with request authentication off: each call carries a credential scope (which is what
/// names the Region) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The signature is
/// never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one request and answer its status with the body as text.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action that must succeed: `POST /` with the parameters in the form
/// body. Every value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
async fn iam(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    let (status, answered) = send(
        router,
        "iam",
        "application/x-www-form-urlencoded",
        None,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: IAM {action} must succeed, got {status} {answered}"
    );
}

/// Drive one CloudWatch Logs operation, answering its status and parsed body.
async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        "logs",
        "application/x-amz-json-1.1",
        Some(&format!("Logs_20140328.{operation}")),
        body.to_string(),
    )
    .await;
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch Logs operation that must succeed.
async fn logs_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, parsed) = logs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: logs {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of an awsJson error body (`__type`, which may carry a namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"].as_str().unwrap_or_default();
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// Drive one Kinesis operation that must succeed, answering its parsed body.
async fn kinesis(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, text) = send(
        router,
        "kinesis",
        "application/x-amz-json-1.1",
        Some(&format!("Kinesis_20131202.{operation}")),
        body.to_string(),
    )
    .await;
    let parsed: serde_json::Value = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: kinesis {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// The status a Kinesis stream reports once it can take a record.
const STREAM_ACTIVE: &str = "ACTIVE";

/// How long a row waits for a stream to report itself usable, and how often it looks. The
/// deadline is generous: it is not a limit any case tests, only room for a loaded host.
const ACTIVE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// A one-shard Kinesis stream that a `PutDestination` can deliver its test message to: the put
/// VALIDATES the target it is handed by writing one record to it, so the stream has to be there
/// and usable before the destination over it is configured.
///
/// The CONDITION is what is waited for rather than a span of the clock: a create arms a
/// transition the emulator's clock owes, and this looks again until the stream reports it has
/// been made or [`ACTIVE_DEADLINE`] has passed.
async fn usable_stream(router: &MultiServiceRouter, case: &str, stream: &str) {
    kinesis(
        router,
        case,
        "CreateStream",
        &serde_json::json!({"StreamName": stream, "ShardCount": 1}),
    )
    .await;
    let deadline = std::time::Instant::now() + ACTIVE_DEADLINE;
    loop {
        let summary = kinesis(
            router,
            case,
            "DescribeStreamSummary",
            &serde_json::json!({"StreamName": stream}),
        )
        .await;
        let status = summary["StreamDescriptionSummary"]["StreamStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == STREAM_ACTIVE {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the stream {stream:?} to reach {STREAM_ACTIVE}, got \
             {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A trust policy with one statement, as a client sends one.
fn trust(effect: &str, action: &str, principals: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{principals}}},\"Action\":{action}}}]}}"
    )
}

/// The IAM role ARN of this account that `name` names. An IAM ARN carries no Region.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The destination kinds a filter's own role governs the write to. Source:
/// `PutSubscriptionFilter$destinationArn` — "An Amazon Kinesis stream", "A Kinesis Data Firehose
/// delivery stream".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Kinesis,
    Firehose,
}

impl Kind {
    /// The ARN of one resource of this kind, named after the row.
    fn destination(self, name: &str) -> String {
        let account = engine::emulator_account_id();
        match self {
            Self::Kinesis => format!("arn:aws:kinesis:{REGION}:{account}:stream/{name}"),
            Self::Firehose => {
                format!("arn:aws:firehose:{REGION}:{account}:deliverystream/{name}")
            }
        }
    }

    /// How the refusal names this kind of stream.
    fn label(self) -> &'static str {
        match self {
            Self::Kinesis => "Kinesis",
            Self::Firehose => "Firehose",
        }
    }
}

/// One row: the trust document IAM holds for the filter's role, the destination kind it is
/// configured against, and whether the put must be accepted.
struct Row {
    case: &'static str,
    /// Names this row's role, log group and destination, so no row collides with another.
    name: &'static str,
    /// The trust document the role is created with, or `None` for a role IAM holds nothing
    /// under.
    document: Option<String>,
    kind: Kind,
    /// Whether the put must configure the filter.
    configured: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a trust policy admitting this service",
            name: "trusts-logs",
            document: Some(trust(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"logs.amazonaws.com\"",
            )),
            kind: Kind::Kinesis,
            configured: true,
        },
        // The recorded case: the role is trusted by another service's principal only.
        Row {
            case: "a trust policy admitting another service only",
            name: "trusts-ec2",
            document: Some(trust(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"ec2.amazonaws.com\"",
            )),
            kind: Kind::Kinesis,
            configured: false,
        },
        Row {
            case: "a trust policy admitting this service among others",
            name: "trusts-both",
            document: Some(trust(
                "Allow",
                "\"sts:AssumeRole\"",
                "[\"ec2.amazonaws.com\",\"logs.amazonaws.com\"]",
            )),
            kind: Kind::Kinesis,
            configured: true,
        },
        Row {
            case: "a wildcard action covering the assume",
            name: "wildcard-action",
            document: Some(trust("Allow", "\"sts:*\"", "\"logs.amazonaws.com\"")),
            kind: Kind::Kinesis,
            configured: true,
        },
        Row {
            case: "a statement naming this service and another action",
            name: "other-action",
            document: Some(trust(
                "Allow",
                "\"s3:GetObject\"",
                "\"logs.amazonaws.com\"",
            )),
            kind: Kind::Kinesis,
            configured: false,
        },
        Row {
            case: "an explicit deny of the assume",
            name: "denies-logs",
            document: Some(trust("Deny", "\"sts:AssumeRole\"", "\"logs.amazonaws.com\"")),
            kind: Kind::Kinesis,
            configured: false,
        },
        Row {
            case: "a role IAM holds nothing under",
            name: "absent-from-iam",
            document: None,
            kind: Kind::Kinesis,
            configured: true,
        },
        // The same two outcomes over the OTHER destination kind a filter's role governs, which
        // is what the refusal names the undeliverable test message for.
        Row {
            case: "a firehose destination behind a trusted role",
            name: "firehose-trusted",
            document: Some(trust(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"logs.amazonaws.com\"",
            )),
            kind: Kind::Firehose,
            configured: true,
        },
        Row {
            case: "a firehose destination behind a role admitting another service only",
            name: "firehose-untrusted",
            document: Some(trust(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"ec2.amazonaws.com\"",
            )),
            kind: Kind::Firehose,
            configured: false,
        },
    ]
}

#[tokio::test]
async fn a_subscription_filter_is_configured_only_under_a_role_this_service_can_assume() {
    let router = emulator();
    let rows = rows();

    for row in &rows {
        if let Some(document) = &row.document {
            iam(
                &router,
                row.case,
                "CreateRole",
                &[
                    ("RoleName", row.name),
                    ("AssumeRolePolicyDocument", document.as_str()),
                ],
            )
            .await;
        }
        logs_ok(
            &router,
            row.case,
            "CreateLogGroup",
            &serde_json::json!({"logGroupName": format!("/aws/emulator/{}", row.name)}),
        )
        .await;
        // The Kinesis rows' destination has to EXIST for the rule under test to be the one the
        // put answers: a `PutDestination` validates its target by delivering a test message to
        // it, so a target Kinesis holds nothing under is refused for that instead of for the
        // role. The Firehose rows name a service this suite does not mount, which answers
        // nothing about their target either way.
        if row.kind == Kind::Kinesis {
            usable_stream(&router, row.case, row.name).await;
        }
    }

    for row in &rows {
        let case = row.case;
        let group = format!("/aws/emulator/{}", row.name);
        let (status, answered) = logs(
            &router,
            "PutSubscriptionFilter",
            &serde_json::json!({
                "logGroupName": group,
                "filterName": FILTER,
                "filterPattern": "",
                "destinationArn": row.kind.destination(row.name),
                "roleArn": role_arn(row.name),
            }),
        )
        .await;

        if row.configured {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be accepted, got {status} {answered}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the put to be refused, got {status} {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER}, got {code} {answered}"
            );
            let label = row.kind.label();
            let expected = format!(
                "Could not deliver test message to specified {label} stream. Check if the given \
                 {label} stream is in ACTIVE state."
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {answered}"
            );
        }

        // What the log group HOLDS afterwards: a refused put stores no filter at all.
        let held = logs_ok(
            &router,
            case,
            "DescribeSubscriptionFilters",
            &serde_json::json!({"logGroupName": group}),
        )
        .await;
        let names: Vec<String> = held["subscriptionFilters"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|filter| filter["filterName"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = if row.configured {
            vec![FILTER.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            names, expected,
            "case {case:?}: expected the group to hold {expected:?}, got {names:?}"
        );

        // The same rule over the OTHER operation that names a role CloudWatch Logs delivers
        // under: a logical destination's own `roleArn` ("grants CloudWatch Logs permissions to
        // call the Amazon Kinesis PutRecord operation on the destination stream").
        let destination_name = format!("destination-{}", row.name);
        let (status, answered) = logs(
            &router,
            "PutDestination",
            &serde_json::json!({
                "destinationName": destination_name,
                "targetArn": row.kind.destination(row.name),
                "roleArn": role_arn(row.name),
            }),
        )
        .await;
        if row.configured {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the destination put to be accepted, got {status} \
                 {answered}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the destination put to be refused, got {status} \
                 {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER} from the destination put, got \
                 {code} {answered}"
            );
            let label = row.kind.label();
            let expected = format!(
                "Could not deliver test message to specified {label} stream. Check if the given \
                 {label} stream is in ACTIVE state."
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?} from the destination put, got {answered}"
            );
        }
        let described = logs_ok(
            &router,
            case,
            "DescribeDestinations",
            &serde_json::json!({"DestinationNamePrefix": destination_name}),
        )
        .await;
        let held: Vec<String> = described["destinations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|destination| {
                destination["destinationName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = if row.configured {
            vec![destination_name.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            held, expected,
            "case {case:?}: expected the account to hold {expected:?}, got {held:?}"
        );
    }
}
