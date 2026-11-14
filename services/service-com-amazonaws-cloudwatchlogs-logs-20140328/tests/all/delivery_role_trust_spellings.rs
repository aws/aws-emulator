// SPDX-License-Identifier: CC0-1.0

//! Which delivery role CloudWatch Logs can assume, over the TRUST DOCUMENTS a role carries —
//! end to end, with the IAM emulator mounted beside this service on one engine, so the
//! document read is the one IAM itself stored.
//!
//! RULE: a put that configures a delivery to a streaming destination is refused for its
//! `roleArn` only when IAM reports a role of this account whose trust policy admits NEITHER
//! spelling of this service's principal — the global `logs.amazonaws.com` and the Regional
//! `logs.<region>.amazonaws.com`, both of which name this service, so a trust policy written
//! in either admits it. Everything else is served: a document naming either spelling, a grant
//! narrowed by a condition, and a role this account's IAM holds nothing under.
//!
//! The rows differ along what the rule generalises over: which spelling the document names,
//! whether it names one or several, whether the grant carries a condition, which effect the
//! statement states, whether the action is written with a wildcard, and whether IAM holds the
//! role at all. Each row is driven through BOTH puts that take a delivery role
//! (`PutDestination` and `PutSubscriptionFilter`), because the trust question is one reader
//! shared by them.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_code, REGION};

/// The access-key id every call here presents; nothing signs, so it only fills the credential
/// scope the Region is read from.
const ACCESS_KEY: &str = "AKIDEXAMPLE";

/// The error code a put refuses an unassumable delivery role with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// This service beside the IAM emulator, both on ONE shared engine: the trust document this
/// service reads is the one IAM stored, written through IAM's own `CreateRole`.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One call to this service, returning `(status, parsed body)`.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/20260101/{REGION}/logs/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One `awsQuery` call to IAM, which must succeed.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/20260101/{REGION}/iam/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: the {action} setup call must succeed, got {status} with {text}"
        );
        text
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive.
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

/// A trust policy with one statement naming `services` (a JSON string or array), `action` and
/// `effect`, plus `condition` when the grant carries one.
fn trust(effect: &str, action: &str, services: &str, condition: &str) -> String {
    let clause = if condition.is_empty() {
        String::new()
    } else {
        format!(",\"Condition\":{condition}")
    };
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{services}}},\"Action\":\"{action}\"{clause}}}]}}"
    )
}

/// One row: a trust document (or none at all) and whether the puts under that role are served.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The trust document IAM holds for the row's role, or `None` for a role IAM holds
    /// nothing under.
    trust: Option<String>,
    /// Whether a put naming that role is served.
    served: bool,
}

/// The table.
fn rows() -> Vec<Row> {
    let regional = format!("logs.{REGION}.amazonaws.com");
    vec![
        Row {
            case: "the global spelling of this service's principal",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                "\"logs.amazonaws.com\"",
                "",
            )),
            served: true,
        },
        Row {
            case: "the Regional spelling of this service's principal",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                &format!("\"{regional}\""),
                "",
            )),
            served: true,
        },
        Row {
            case: "both spellings in one list",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                &format!("[\"logs.amazonaws.com\",\"{regional}\"]"),
                "",
            )),
            served: true,
        },
        Row {
            case: "the Regional spelling, the grant narrowed by a source condition",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                &format!("\"{regional}\""),
                "{\"StringLike\":{\"aws:SourceArn\":\"arn:aws:logs:*:*:*\"}}",
            )),
            served: true,
        },
        Row {
            case: "the Regional spelling, the action written with a wildcard",
            trust: Some(trust("Allow", "sts:*", &format!("\"{regional}\""), "")),
            served: true,
        },
        Row {
            case: "another service's principal only",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                "\"lambda.amazonaws.com\"",
                "",
            )),
            served: false,
        },
        Row {
            case: "another Region's spelling of this service's principal",
            trust: Some(trust(
                "Allow",
                "sts:AssumeRole",
                "\"logs.ap-south-1.amazonaws.com\"",
                "",
            )),
            served: false,
        },
        Row {
            case: "the Regional spelling, explicitly denied",
            trust: Some(trust(
                "Deny",
                "sts:AssumeRole",
                &format!("\"{regional}\""),
                "",
            )),
            served: false,
        },
        Row {
            case: "a role IAM holds nothing under",
            trust: None,
            served: true,
        },
    ]
}

/// The refusal both puts answer for a role they cannot assume, for a Kinesis target.
const UNASSUMABLE: &str = "Could not deliver test message to specified Kinesis stream. Check if \
                           the given Kinesis stream is in ACTIVE state.";

/// The role ARN of this account's role `name`.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The ARN of the Kinesis stream every row delivers to.
fn stream_arn() -> String {
    format!(
        "arn:aws:kinesis:{REGION}:{}:stream/trust-target",
        engine::emulator_account_id()
    )
}

/// RULE: both puts serve a delivery role whose trust policy admits EITHER spelling of this
/// service's principal, and refuse one whose trust policy admits neither.
#[tokio::test]
async fn either_spelling_of_this_service_s_principal_admits_a_delivery_role() {
    let aws = Emulator::new();
    for (nth, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let name = format!("r{nth}");
        let role = format!("delivery-{name}");
        if let Some(document) = row.trust.as_ref() {
            aws.iam(
                case,
                "CreateRole",
                &[
                    ("RoleName", role.clone()),
                    ("AssumeRolePolicyDocument", document.clone()),
                ],
            )
            .await;
        }

        // PutDestination: the logical destination's own role.
        let destination = format!("destination-{name}");
        let (status, body) = aws
            .logs(
                "PutDestination",
                &json!({
                    "destinationName": &destination,
                    "targetArn": stream_arn(),
                    "roleArn": role_arn(&role),
                }),
            )
            .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: PutDestination is served, got {status} {body}"
            );
            assert_eq!(
                body["destination"]["roleArn"].as_str(),
                Some(role_arn(&role).as_str()),
                "case {case:?}: the stored destination carries the role, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: PutDestination is refused, got {status} {body}"
            );
            assert_eq!(
                (error_code(&body).as_str(), body["message"].as_str()),
                (INVALID_PARAMETER, Some(UNASSUMABLE)),
                "case {case:?}: expected {INVALID_PARAMETER} {UNASSUMABLE:?}, got {body}"
            );
        }

        // PutSubscriptionFilter: the filter's own role, for the same target.
        let group = format!("/aws/emulator/{name}");
        let (status, body) = aws
            .logs("CreateLogGroup", &json!({"logGroupName": &group}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the log group is created, got {status} {body}"
        );
        let (status, body) = aws
            .logs(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": &group,
                    "filterName": "trust-filter",
                    "filterPattern": "",
                    "destinationArn": stream_arn(),
                    "roleArn": role_arn(&role),
                }),
            )
            .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: PutSubscriptionFilter is served, got {status} {body}"
            );
            let (status, described) = aws
                .logs(
                    "DescribeSubscriptionFilters",
                    &json!({"logGroupName": &group}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the filters are reported, got {status} {described}"
            );
            assert_eq!(
                described["subscriptionFilters"][0]["roleArn"].as_str(),
                Some(role_arn(&role).as_str()),
                "case {case:?}: the stored filter carries the role, got {described}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: PutSubscriptionFilter is refused, got {status} {body}"
            );
            assert_eq!(
                (error_code(&body).as_str(), body["message"].as_str()),
                (INVALID_PARAMETER, Some(UNASSUMABLE)),
                "case {case:?}: expected {INVALID_PARAMETER} {UNASSUMABLE:?}, got {body}"
            );
        }
    }
}
