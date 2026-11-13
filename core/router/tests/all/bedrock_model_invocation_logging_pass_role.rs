// SPDX-License-Identifier: CC0-1.0

//! Amazon Bedrock settles the `iam:PassRole` step of
//! `PutModelInvocationLoggingConfiguration` against this account's IAM.
//!
//! RULE: the CloudWatch Logs destination of the account's model invocation logging
//! configuration names a role Amazon Bedrock TAKES ON to write the log events, so the put
//! is authorized before anything is stored and is ADMITTED where the value names a role of
//! the CALLER's OWN account (`iam:PassRole` is granted by the account that holds the role)
//! whose trust policy — where this account's IAM holds the role — admits
//! `bedrock.amazonaws.com` (the service's own `awsService.cloudTrailEventSource`) the
//! `sts:AssumeRole` that hand-off is. An admitted put answers HTTP 200 with an empty body
//! and the configuration is reported back by `GetModelInvocationLoggingConfiguration`
//! exactly as it was submitted; a pass IAM does not let Amazon Bedrock make is refused with
//! the modelled `AccessDeniedException` (HTTP 403) and installs NOTHING, so the
//! configuration in force is left exactly as it was.
//!
//! A role this account's IAM holds NOTHING under is a different answer, and this suite pins
//! it too: such a destination does not RESOLVE, so the delivery permissions it describes
//! cannot be validated at all and it is refused with the modelled `ValidationException`
//! (HTTP 400) naming the submitted log group and role, ahead of the pass-role decision — the
//! whole table of that rule lives in the service crate's
//! `tests/all/model_invocation_logging_destination_validation.rs`.
//!
//! The put stores a trust RELATIONSHIP for every later invocation to be logged under rather
//! than authorizing one named assume, so the trust is validated as a relationship: a
//! statement's `Condition` narrows which assumes the grant covers, not whether the role
//! trusts the service, and that is why a grant narrowed by the confused-deputy keys a
//! single invocation carries is admitted here.
//!
//! The role is resolved through the in-process callback, so a single-service router has no
//! IAM to ask: that is why this suite lives here rather than in the service's own crate,
//! whose `tests/all/model_invocation_logging.rs` pins the half of the rule that needs no
//! IAM (the account the ARN names, and the round-trip of every role-free configuration).
//!
//! The rows span what the rule generalises over: the trust policy's shape (`Action` and
//! `Principal.Service` as one string or as a list, the wildcards IAM admits in the action's
//! place, a `Condition`, an explicit `Deny` beside an `Allow`, a grant to an account
//! principal, a policy naming another service only), where the role lives (the default path
//! and a path), which account and partition the ARN names (including the EMPTY account the
//! member's published pattern admits), whether the role was ever created, and whether the
//! configuration names a role-bearing destination at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder an awsQuery form body needs is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns: a form component is escaped
// exactly as a URI path segment is, so the codec is shared rather than written again.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here so this suite
/// pins the wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// What Amazon Bedrock states for a destination whose delivery permissions it could not
/// validate — here, a role this account's IAM holds nothing under: the fixed text, with the
/// log group and the role the request SUBMITTED.
fn unvalidated_destination(log_group: &str, role_arn: &str) -> String {
    format!(
        "Failed to validate permissions for log group: {log_group}, with role: {role_arn}. \
         Verify the IAM role permissions are correct."
    )
}

/// The URI `PutModelInvocationLoggingConfiguration`,
/// `GetModelInvocationLoggingConfiguration` and
/// `DeleteModelInvocationLoggingConfiguration` are all bound to
/// (`smithy_query(query="operation",operation="PutModelInvocationLoggingConfiguration")`).
const LOGGING_URI: &str = "/logging/modelinvocations";

/// The service principal a logging role's trust policy has to grant the assume to, spelled
/// as a JSON string so a row can name it beside others in a list.
const SERVICE_PRINCIPAL: &str = "\"bedrock.amazonaws.com\"";

/// The Bedrock control plane and IAM over ONE shared engine, as `router/src/lib.rs` mounts
/// them, so the control plane's in-process callback reaches this very IAM.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One awsQuery call to IAM: `POST /` with the parameters in the form body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
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
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One restJson1 call to the Bedrock control plane, answered with the status, the wire error
/// code and the JSON body (`Value::Null` for the empty body a put answers with).
async fn bedrock(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(bare_code);
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let error = header_error.unwrap_or_else(|| {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(bare_code)
            .unwrap_or_default()
    });
    (status, error, parsed)
}

/// The bare error code out of an `__type` / `x-amzn-errortype` value, which may carry a
/// namespace or a URL beside it.
fn bare_code(raw: &str) -> String {
    let without_url = raw.split(':').next().unwrap_or(raw);
    without_url
        .rsplit('#')
        .next()
        .unwrap_or(without_url)
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of a role of this account, under the default path.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A trust policy with one statement, built from the pieces that vary between rows: the
/// effect, the action(s), the service principal(s) and the `Condition` the grant is narrowed
/// by (`""` for a statement that carries none).
fn trust_policy(effect: &str, action: &str, service: &str, condition: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}{condition}}}]}}"
    )
}

/// A `Condition` element as a statement member, ready to append to one.
fn condition(body: &str) -> String {
    format!(",\"Condition\":{body}")
}

/// One IAM role the rows name, as `CreateRole` parameters.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "logging-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "logging-role-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"lambda.amazonaws.com\",\"bedrock.amazonaws.com\"]",
                "",
            ),
        },
        RoleFixture {
            role_name: "logging-role-wildcard",
            path: None,
            policy: trust_policy("Allow", "\"sts:*\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "logging-role-path",
            path: Some("/service-role/"),
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            // The confused-deputy guard a logging role carries: the grant is narrowed to
            // the account whose invocations are logged and to the invocations themselves.
            // Those keys belong to the assume this configuration will later be used for,
            // not to this put, which stores the relationship — so the role still TRUSTS
            // the service and the pass is admitted.
            role_name: "logging-role-conditioned",
            path: None,
            policy: trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                SERVICE_PRINCIPAL,
                &condition(&format!(
                    "{{\"StringEquals\":{{\"aws:SourceAccount\":\"{ACCOUNT}\"}},\
                     \"ArnLike\":{{\"aws:SourceArn\":\"arn:aws:bedrock:{REGION}:{ACCOUNT}:*\"}}}}"
                )),
            ),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\"", ""),
        },
        RoleFixture {
            role_name: "other-action-role",
            path: None,
            policy: trust_policy("Allow", "\"s3:GetObject\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL, ""),
        },
        RoleFixture {
            // IAM's explicit deny wins over an `Allow` for the same principal.
            role_name: "allow-then-deny-role",
            path: None,
            policy: format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
                 \"Action\":\"sts:AssumeRole\"}},\
                 {{\"Effect\":\"Deny\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
                 \"Action\":\"sts:AssumeRole\"}}]}}"
            ),
        },
        RoleFixture {
            // A document that IS a policy and grants the assume to an ACCOUNT principal
            // rather than to a service one, so it grants nothing to Amazon Bedrock.
            role_name: "account-principal-role",
            path: None,
            policy: format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
                 \"Action\":\"sts:AssumeRole\"}}]}}"
            ),
        },
    ]
}

/// One row: the configuration a put carries, and the answer the decision gives it.
struct Row {
    case: &'static str,
    logging_config: Value,
    expected: Answer,
}

/// What a row expects the put to answer.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// 200, and the configuration is the one in force afterwards.
    Installed,
    /// 403 `AccessDeniedException`: the pass this account may not make.
    PassRoleDenied,
    /// 400 `ValidationException`: the destination does not resolve, so the permissions it
    /// describes cannot be validated.
    Unvalidated,
}

/// A CloudWatch Logs destination naming `role`, under a log group named for `stem` so each
/// row's stored record is distinguishable from the row before it.
fn cloud_watch_config(stem: &str, role: String) -> Value {
    json!({
        "cloudWatchConfig": { "logGroupName": format!("aws/bedrock/{stem}"), "roleArn": role },
        "textDataDeliveryEnabled": true,
    })
}

/// The rows, ordered so an admitted put is followed by a refused one: the get after a
/// refusal then witnesses that the configuration in force is the one the last ADMITTED put
/// installed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role this account holds that grants the assume",
            logging_config: cloud_watch_config("trusted", role_arn("logging-role")),
            expected: Answer::Installed,
        },
        Row {
            case: "a role that trusts another service only",
            logging_config: cloud_watch_config("lambda-role", role_arn("lambda-only-role")),
            expected: Answer::PassRoleDenied,
        },
        Row {
            case: "a trust policy whose Action and Service are lists",
            logging_config: cloud_watch_config("trusted-lists", role_arn("logging-role-lists")),
            expected: Answer::Installed,
        },
        Row {
            case: "a role whose statement names another action",
            logging_config: cloud_watch_config("other-action", role_arn("other-action-role")),
            expected: Answer::PassRoleDenied,
        },
        Row {
            case: "a trust policy granting the whole sts namespace",
            logging_config: cloud_watch_config(
                "trusted-wildcard",
                role_arn("logging-role-wildcard"),
            ),
            expected: Answer::Installed,
        },
        Row {
            case: "a role whose statement denies the assume",
            logging_config: cloud_watch_config("denied-role", role_arn("denied-role")),
            expected: Answer::PassRoleDenied,
        },
        Row {
            case: "a role under a path",
            logging_config: cloud_watch_config(
                "trusted-path",
                format!("arn:aws:iam::{ACCOUNT}:role/service-role/logging-role-path"),
            ),
            expected: Answer::Installed,
        },
        Row {
            case: "a role that both grants and explicitly denies the assume",
            logging_config: cloud_watch_config(
                "allow-then-deny",
                role_arn("allow-then-deny-role"),
            ),
            expected: Answer::PassRoleDenied,
        },
        Row {
            // The grant is narrowed to the assumes a logged invocation makes; the role
            // trusts the service, which is what this put stores.
            case: "a grant narrowed by the keys a logged invocation carries",
            logging_config: cloud_watch_config(
                "trusted-conditioned",
                role_arn("logging-role-conditioned"),
            ),
            expected: Answer::Installed,
        },
        Row {
            case: "a role whose trust policy grants an account principal rather than a \
                   service one",
            logging_config: cloud_watch_config(
                "account-principal",
                role_arn("account-principal-role"),
            ),
            expected: Answer::PassRoleDenied,
        },
        Row {
            // The member's published pattern admits an EMPTY account field, which names no
            // OTHER account: this one's role, and this one holds it.
            case: "a role ARN carrying no account at all",
            logging_config: cloud_watch_config(
                "no-account",
                "arn:aws:iam:::role/logging-role".to_string(),
            ),
            expected: Answer::Installed,
        },
        Row {
            case: "a role this account was never given",
            logging_config: cloud_watch_config("absent-role", role_arn("never-created-role")),
            expected: Answer::Unvalidated,
        },
        Row {
            // An IAM ARN is partition-global and the account is this one, so the role it
            // names is the role IAM holds.
            case: "a role of this account in another partition",
            logging_config: cloud_watch_config(
                "other-partition",
                format!("arn:aws-cn:iam::{ACCOUNT}:role/logging-role"),
            ),
            expected: Answer::Installed,
        },
        Row {
            // `iam:PassRole` is granted by the account that HOLDS the role, so another
            // account's role is one no principal of this account can pass, whatever it
            // trusts.
            case: "a role of another account",
            logging_config: cloud_watch_config(
                "foreign-role",
                "arn:aws:iam::123456789012:role/logging-role".to_string(),
            ),
            expected: Answer::PassRoleDenied,
        },
        Row {
            // `S3Config` declares no role member at all, so this configuration passes no
            // role and has no such decision to take.
            case: "a configuration that names no role-bearing destination",
            logging_config: json!({
                "s3Config": { "bucketName": "bedrock-invocation-logs", "keyPrefix": "prod/" },
                "imageDataDeliveryEnabled": true,
            }),
            expected: Answer::Installed,
        },
    ]
}

/// Create every role the rows name.
async fn create_roles(router: &MultiServiceRouter) {
    for fixture in role_fixtures() {
        let mut parameters = vec![
            ("RoleName", fixture.role_name),
            ("AssumeRolePolicyDocument", fixture.policy.as_str()),
        ];
        if let Some(path) = fixture.path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, "CreateRole", &parameters).await;
        assert_eq!(
            status, 200,
            "setup: CreateRole {} must succeed, got {status} {body}",
            fixture.role_name
        );
    }
}

/// `GET /logging/modelinvocations`, answered with the `loggingConfig` it reports
/// (`Value::Null` when it reports none).
async fn installed(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, error, body) = bedrock(router, "GET", LOGGING_URI, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetModelInvocationLoggingConfiguration must answer 200, got {status} \
         {error} ({body})"
    );
    body.get("loggingConfig").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn invocation_logging_is_installed_only_for_a_role_iam_lets_bedrock_assume() {
    let router = emulator();
    create_roles(&router).await;

    // What an independent get must report: nothing until the first ADMITTED put, and the
    // record that put installed from then on.
    let mut in_force = Value::Null;
    assert_eq!(
        installed(&router, "before any put").await,
        in_force,
        "case before any put: nothing is configured until a put installs something"
    );

    for row in rows() {
        let (status, error, body) = bedrock(
            &router,
            "PUT",
            LOGGING_URI,
            Some(json!({ "loggingConfig": row.logging_config.clone() })),
        )
        .await;

        match row.expected {
            Answer::Installed => {
                // `PutModelInvocationLoggingConfigurationOutput` declares no members, so the
                // answer carries nothing beyond the 200.
                let members = body.as_object().map(serde_json::Map::len).unwrap_or(0);
                assert_eq!(
                    (status, error.as_str(), members),
                    (200, "", 0),
                    "case {:?}: an admitted pass must answer 200 with no members, got {status} \
                     {error} ({body})",
                    row.case
                );
                in_force = row.logging_config.clone();
            }
            Answer::PassRoleDenied => {
                assert_eq!(
                    (status, error.as_str(), text(&body, "message").as_str()),
                    (403, "AccessDeniedException", PASS_ROLE_REFUSAL),
                    "case {:?}: a pass IAM does not authorize must be refused 403 \
                     AccessDeniedException, got {status} {error} ({body})",
                    row.case
                );
            }
            Answer::Unvalidated => {
                // The message is composed from the values THIS row submitted, so a refusal
                // naming anything else fails here.
                let destination = &row.logging_config["cloudWatchConfig"];
                let expected = unvalidated_destination(
                    destination["logGroupName"].as_str().unwrap_or_default(),
                    destination["roleArn"].as_str().unwrap_or_default(),
                );
                assert_eq!(
                    (status, error.as_str(), text(&body, "message").as_str()),
                    (400, "ValidationException", expected.as_str()),
                    "case {:?}: a destination IAM holds no such role for must be refused 400 \
                     ValidationException naming the submitted pair, got {status} {error} \
                     ({body})",
                    row.case
                );
            }
        }

        // The record a client reads back: exactly what the last ADMITTED put submitted,
        // member for member — so a refused put installed no part of its configuration and
        // did not disturb the one in force either.
        let reported = installed(&router, row.case).await;
        assert_eq!(
            reported, in_force,
            "case {:?}: the configuration in force must be the last admitted one, got \
             {reported:?}",
            row.case
        );
    }

    // The singleton is still the account's to clear, after every decision above.
    let (status, error, body) = bedrock(&router, "DELETE", LOGGING_URI, None).await;
    assert_eq!(
        (status, error.as_str()),
        (200, ""),
        "case after the rows: DeleteModelInvocationLoggingConfiguration must answer 200, got \
         {status} {error} ({body})"
    );
    assert_eq!(
        installed(&router, "after the delete").await,
        Value::Null,
        "case after the delete: nothing is configured once the record is removed"
    );
}

/// RULE (the ordering half): every decision is taken BEFORE anything is stored, so a put the
/// destination's resolution or the pass refuses leaves the configuration in force untouched
/// however many times it is repeated, and the same configuration under a passable role
/// installs at once.
#[tokio::test]
async fn a_refused_put_never_replaces_the_configuration_in_force() {
    let router = emulator();
    create_roles(&router).await;

    let admitted = cloud_watch_config("in-force", role_arn("logging-role"));
    let (status, error, body) = bedrock(
        &router,
        "PUT",
        LOGGING_URI,
        Some(json!({ "loggingConfig": admitted.clone() })),
    )
    .await;
    assert_eq!(
        (status, error.as_str()),
        (200, ""),
        "setup: the first put must install the configuration, got {status} {error} ({body})"
    );

    // The rows differ along the reason the put is refused: the role is another account's and
    // the role it holds trusts another service — both passes this account may not make — and
    // this account holds no such role, which is a destination that does not resolve at all.
    let refused = [
        (
            "another account's role",
            "arn:aws:iam::123456789012:role/logging-role".to_string(),
            Answer::PassRoleDenied,
        ),
        (
            "a role never created",
            role_arn("never-created-role"),
            Answer::Unvalidated,
        ),
        (
            "a role trusting another service",
            role_arn("lambda-only-role"),
            Answer::PassRoleDenied,
        ),
    ];
    for (case, role, expected) in refused {
        for attempt in 1..=2 {
            let config = cloud_watch_config("replacement", role.clone());
            let (status, error, body) = bedrock(
                &router,
                "PUT",
                LOGGING_URI,
                Some(json!({ "loggingConfig": config.clone() })),
            )
            .await;
            let (expected_status, expected_error, expected_message) = match expected {
                Answer::PassRoleDenied => {
                    (403, "AccessDeniedException", PASS_ROLE_REFUSAL.to_string())
                }
                Answer::Unvalidated => (
                    400,
                    "ValidationException",
                    unvalidated_destination(
                        config["cloudWatchConfig"]["logGroupName"]
                            .as_str()
                            .unwrap_or_default(),
                        &role,
                    ),
                ),
                Answer::Installed => unreachable!("every row of this table is refused"),
            };
            assert_eq!(
                (status, error.as_str(), text(&body, "message").as_str()),
                (expected_status, expected_error, expected_message.as_str()),
                "case {case:?} attempt {attempt}: the refusal is the same every time, got \
                 {status} {error} ({body})"
            );
            let reported = installed(&router, case).await;
            assert_eq!(
                reported, admitted,
                "case {case:?} attempt {attempt}: a refused put replaces nothing, got \
                 {reported:?}"
            );
        }
    }

    // The SAME configuration, under a role this account may pass, installs: the refusals
    // above were about the role and not about the configuration's shape.
    let replacement = cloud_watch_config("replacement", role_arn("logging-role-lists"));
    let (status, error, body) = bedrock(
        &router,
        "PUT",
        LOGGING_URI,
        Some(json!({ "loggingConfig": replacement.clone() })),
    )
    .await;
    assert_eq!(
        (status, error.as_str()),
        (200, ""),
        "case the replacement under a passable role: expected 200, got {status} {error} \
         ({body})"
    );
    let reported = installed(&router, "the replacement under a passable role").await;
    assert_eq!(
        reported, replacement,
        "case the replacement under a passable role: the put replaces the record whole, got \
         {reported:?}"
    );
}
