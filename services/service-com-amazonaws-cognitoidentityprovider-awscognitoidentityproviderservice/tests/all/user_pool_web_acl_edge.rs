// SPDX-License-Identifier: CC0-1.0

//! A WAFv2 WEB ACL in front of a Cognito user pool, end to end over the wire.
//!
//! The rule this suite pins: once a REGIONAL web ACL is associated with a user pool's
//! ARN, every operation of that pool the model declares `ForbiddenException` for is
//! DECIDED BY THAT WEB ACL before the pool serves it — the rules run in ascending
//! priority, the first terminating action settles the request, and the `DefaultAction`
//! decides one no rule matched. A request the web ACL does not allow is refused in
//! front of the pool with `ForbiddenException` (or with the blocking action's own
//! `CustomResponse`), a request it allows reaches the operation and gets exactly the
//! answer it got with no web ACL in front of the pool, and the decision follows the
//! association's CURRENT rules — an `UpdateWebACL` that changes a rule's action changes
//! what the live association decides, with no re-association.
//!
//! Each test asserts that rule over a TABLE rather than one recorded request: the rows
//! differ along what the rule generalises over — which operation is called, which
//! member of its request names the pool, which component of the request the rule
//! inspects, which action the matching rule applies, and which status a custom response
//! serves — and every assertion names its case and reports the actual answer.
//!
//! Every call goes through the real wire path: the pool, its app client and its users
//! are built through Amazon Cognito's own API, the web ACLs through WAFv2's, and both
//! services are mounted on ONE shared engine behind the multi-service router, which is
//! what lets one request to the pool reach the web ACL the wafv2 control plane stored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The region both control planes are addressed in, and the one the pool's ARN carries.
const REGION: &str = "us-east-1";

/// The `Scope` a user pool's web ACL lives in: a user pool is a REGIONAL resource
/// (`Scope`'s `CLOUDFRONT` value is for "a global resource type, such as a Amazon
/// CloudFront distribution").
const SCOPE: &str = "REGIONAL";

/// A request's SigV4 credential scope, which routes a call to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The header this suite's rules inspect, and the value they look for in it. A header
/// a client chooses to send, so a rule can be written against it and the same call can
/// be made with and without it.
const PROBE_HEADER: &str = "x-emulator-waf-probe";
const PROBE_VALUE: &str = "refuse-this-request";

/// The error shape Amazon Cognito refuses a request its web ACL did not allow with, and
/// the status the model declares for it
/// (`smithy_query(query="shape",shape="ForbiddenException")`: `httpError` 403).
const FORBIDDEN_EXCEPTION: &str = "ForbiddenException";
const FORBIDDEN_STATUS: u16 = 403;

/// The sentence that refusal carries, which is fixed text for this error class:
/// `ForbiddenException$message` is "the message returned when WAF doesn't allow your
/// request based on a web ACL that's associated with your user pool" and carries nothing
/// from the request, from the web ACL or from the pool's state.
const WAF_REFUSAL_MESSAGE: &str = "Request not allowed due to WAF block.";

/// A password satisfying a default pool's password policy, and a second one the
/// temporary-password sign-in uses.
const GOOD_PASSWORD: &str = "Passw0rd!2024";
const TEMPORARY_PASSWORD: &str = "Temp0rary!2024";

/// One emulator with Amazon Cognito and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
}

/// One answered request: its status and its JSON body.
#[derive(Debug, Clone)]
struct Answered {
    status: u16,
    body: Value,
}

impl Answered {
    /// The wire error code of an error body: the `__type` without the shape namespace
    /// an AWS JSON error may carry it under, or an empty string for a success.
    fn code(&self) -> String {
        let raw = self
            .body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw).to_string()
    }

    /// The `message` of an error body.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The status and the error code together: what a caller branches on, and what two
    /// legs of one case are compared by.
    fn outcome(&self) -> (u16, String) {
        (self.status, self.code())
    }
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    /// One call to the user pool's API, carrying `headers` beside the ones every call of
    /// this API carries.
    async fn cognito(&self, operation: &str, body: &Value, headers: &[(&str, &str)]) -> Answered {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", AUTHORIZATION)
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityProviderService.{operation}"),
            );
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(Bytes::from(body.to_string()))
                    .expect("the test request is well formed"),
            )
            .await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} on {:?}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        Answered { status, body }
    }

    /// A call to the user pool's API that must succeed, with no probe header.
    async fn cognito_ok(&self, operation: &str, body: &Value) -> Value {
        let answered = self.cognito(operation, body, &[]).await;
        assert_eq!(
            answered.status, 200,
            "case {operation}: expected 200, got {} with {}",
            answered.status, answered.body
        );
        answered.body
    }

    /// One WAFv2 call, answering its status and parsed body.
    async fn waf(&self, operation: &str, body: Value) -> Answered {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("wafv2.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/wafv2/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answered { status, body }
    }

    /// A WAFv2 call that must succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let answered = self.waf(operation, body).await;
        assert_eq!(
            answered.status, 200,
            "case {operation}: expected 200, got {} with {}",
            answered.status, answered.body
        );
        answered.body
    }
}

/// A `VisibilityConfig` that stores samples, so `GetSampledRequests` has a population
/// to draw from.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A web ACL rule: a name, a priority, a statement and an action.
fn rule(name: &str, priority: i64, statement: Value, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A `ByteMatchStatement` over `field` with no text transformation.
fn byte_match(field: Value, search: &str, constraint: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(search),
            "FieldToMatch": field,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": constraint,
        }
    })
}

/// A `ByteMatchStatement` matching the probe header exactly: the rule every case that
/// needs "the same call with and without one difference" is written against.
fn probe_header_match() -> Value {
    byte_match(
        json!({ "SingleHeader": { "Name": PROBE_HEADER } }),
        PROBE_VALUE,
        "EXACTLY",
    )
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A created web ACL: what later calls address it by.
struct WebAcl {
    id: String,
    arn: String,
    lock_token: String,
}

/// Create a web ACL with `default_action` and `rules`.
async fn create_web_acl(edge: &Edge, name: &str, default_action: Value, rules: Value) -> WebAcl {
    let created = edge
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": default_action,
                "VisibilityConfig": visibility(name),
                "Rules": rules,
            }),
        )
        .await;
    let summary = &created["Summary"];
    WebAcl {
        id: summary["Id"].as_str().expect("Summary.Id").to_string(),
        arn: summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        lock_token: summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    }
}

/// A web ACL whose one rule BLOCKS a request carrying the probe header, and whose
/// default action lets everything else through.
async fn blocking_web_acl(edge: &Edge, name: &str) -> WebAcl {
    create_web_acl(
        edge,
        name,
        json!({ "Allow": {} }),
        json!([rule(
            "refuse-the-probe",
            0,
            probe_header_match(),
            json!({ "Block": {} }),
        )]),
    )
    .await
}

/// Put the web ACL `web_acl_arn` in front of the resource `resource_arn`.
async fn associate(edge: &Edge, web_acl_arn: &str, resource_arn: &str) {
    edge.waf_ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource_arn }),
    )
    .await;
}

/// A user pool with an app client, a signed-in user and a user part way through a
/// challenge: everything the governed half of this API is addressed by.
struct Pool {
    id: String,
    arn: String,
    client_id: String,
    username: String,
    access_token: String,
    /// The refresh token of a SECOND sign-in, so the case that revokes one does not
    /// revoke the sign-in every other case reads through.
    revocable_refresh_token: String,
    /// The `Session` an unanswered challenge handed out, which names this pool too, and
    /// the user that challenge is waiting on.
    session: String,
    challenged_username: String,
}

impl Pool {
    /// Build the pool and everything the cases address it by.
    async fn build(edge: &Edge, name: &str) -> Self {
        let created = edge
            .cognito_ok("CreateUserPool", &json!({ "PoolName": name }))
            .await;
        let id = created["UserPool"]["Id"]
            .as_str()
            .expect("a minted pool id")
            .to_string();
        let arn = created["UserPool"]["Arn"]
            .as_str()
            .expect("the pool's ARN")
            .to_string();
        let client = edge
            .cognito_ok(
                "CreateUserPoolClient",
                &json!({
                    "UserPoolId": id,
                    "ClientName": format!("{name}-client"),
                    "ExplicitAuthFlows": [
                        "ALLOW_USER_PASSWORD_AUTH",
                        "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                        "ALLOW_REFRESH_TOKEN_AUTH",
                    ],
                }),
            )
            .await;
        let client_id = client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string();

        let username = format!("{name}-user");
        edge.cognito_ok(
            "AdminCreateUser",
            &json!({
                "UserPoolId": id,
                "Username": username,
                "UserAttributes": [{ "Name": "email", "Value": "user@example.test" }],
                "MessageAction": "SUPPRESS",
            }),
        )
        .await;
        edge.cognito_ok(
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "Permanent": true,
            }),
        )
        .await;
        let signed_in = edge
            .cognito_ok(
                "InitiateAuth",
                &json!({
                    "ClientId": client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            )
            .await;
        let result = &signed_in["AuthenticationResult"];
        let access_token = result["AccessToken"]
            .as_str()
            .expect("a sign-in issues an access token")
            .to_string();
        let revoked_sign_in = edge
            .cognito_ok(
                "InitiateAuth",
                &json!({
                    "ClientId": client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            )
            .await;
        let revocable_refresh_token = revoked_sign_in["AuthenticationResult"]["RefreshToken"]
            .as_str()
            .expect("a sign-in issues a refresh token")
            .to_string();

        // A second user whose password is TEMPORARY: signing in leaves it part way
        // through the new-password challenge, which is how a request carrying only a
        // `Session` names its pool.
        let challenged = format!("{name}-challenged");
        edge.cognito_ok(
            "AdminCreateUser",
            &json!({
                "UserPoolId": id,
                "Username": challenged,
                "TemporaryPassword": TEMPORARY_PASSWORD,
                "MessageAction": "SUPPRESS",
            }),
        )
        .await;
        let challenge = edge
            .cognito_ok(
                "InitiateAuth",
                &json!({
                    "ClientId": client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": challenged, "PASSWORD": TEMPORARY_PASSWORD },
                }),
            )
            .await;
        let session = challenge["Session"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("an unanswered challenge hands out a session, got {challenge}")
            })
            .to_string();

        Self {
            id,
            arn,
            client_id,
            username,
            access_token,
            revocable_refresh_token,
            session,
            challenged_username: challenged,
        }
    }
}

/// One case of the governed surface: an operation, the body that addresses this pool,
/// and which member of that body names the pool.
struct Governed {
    case: &'static str,
    operation: &'static str,
    named_by: &'static str,
    body: Value,
}

/// The governed operations this suite drives, one per way a request names its pool.
///
/// Each body is IDEMPOTENT — the same call can be made three times (a control before
/// the web ACL is associated, a blocked leg and an allowed leg) and answer the same
/// thing each time — so the only difference between the legs is the web ACL and the
/// probe header.
fn governed_rows(pool: &Pool) -> Vec<Governed> {
    vec![
        Governed {
            case: "the sign-in entry point, which names its pool by the app client",
            operation: "InitiateAuth",
            named_by: "ClientId",
            body: json!({
                "ClientId": pool.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
            }),
        },
        Governed {
            case: "a password recovery, which names its pool by the app client",
            operation: "ForgotPassword",
            named_by: "ClientId",
            body: json!({ "ClientId": pool.client_id, "Username": pool.username }),
        },
        Governed {
            case: "a self-service read, which names its pool by the access token",
            operation: "GetUser",
            named_by: "AccessToken",
            body: json!({ "AccessToken": pool.access_token }),
        },
        Governed {
            case: "a self-service listing, which names its pool by the access token",
            operation: "ListDevices",
            named_by: "AccessToken",
            body: json!({ "AccessToken": pool.access_token }),
        },
        Governed {
            case: "an operation this emulator has not implemented yet, so the web ACL \
                   decides a request that would have been answered 501",
            operation: "ChangePassword",
            named_by: "AccessToken",
            body: json!({
                "AccessToken": pool.access_token,
                "PreviousPassword": GOOD_PASSWORD,
                "ProposedPassword": GOOD_PASSWORD,
            }),
        },
        Governed {
            case: "a token revocation, which names its pool by the app client the token \
                   was issued through",
            operation: "RevokeToken",
            named_by: "ClientId",
            body: json!({ "ClientId": pool.client_id, "Token": pool.revocable_refresh_token }),
        },
        Governed {
            case: "a request part way through a challenge, which names its pool by the \
                   session",
            operation: "AssociateSoftwareToken",
            named_by: "Session",
            body: json!({ "Session": pool.session }),
        },
        Governed {
            case: "a challenge response whose app client resolves nothing, so the session \
                   it also carries is what names the pool",
            operation: "RespondToAuthChallenge",
            named_by: "Session",
            body: json!({
                // An id of the shape an app client's is, that no app client answers to.
                "ClientId": "zzzzzzzzzzzzzzzzzzzzzzzzzz",
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": pool.session,
                "ChallengeResponses": {
                    "USERNAME": pool.challenged_username,
                    "NEW_PASSWORD": GOOD_PASSWORD,
                },
            }),
        },
    ]
}

/// RULE: every operation the web ACL governs is decided by it, whichever member of the
/// request names the pool — a matching request is refused in front of the pool with
/// `ForbiddenException`, and the same request without the match gets exactly the answer
/// it got before any web ACL was associated.
#[tokio::test]
async fn a_web_acl_in_front_of_a_pool_decides_every_operation_it_governs() {
    let edge = Edge::new();
    let pool = Pool::build(&edge, "governed").await;
    let rows = governed_rows(&pool);

    // The CONTROL: what each call answers with no web ACL in front of the pool.
    let mut controls: Vec<(u16, String)> = Vec::new();
    for row in &rows {
        let answered = edge
            .cognito(row.operation, &row.body, &[(PROBE_HEADER, PROBE_VALUE)])
            .await;
        assert_ne!(
            answered.status, FORBIDDEN_STATUS,
            "case {:?}: with no web ACL associated, expected the pool's own answer, got {} with {}",
            row.case, answered.status, answered.body
        );
        controls.push(answered.outcome());
    }

    let web_acl = blocking_web_acl(&edge, "governed-operations").await;
    associate(&edge, &web_acl.arn, &pool.arn).await;

    for (row, control) in rows.iter().zip(&controls) {
        let blocked = edge
            .cognito(row.operation, &row.body, &[(PROBE_HEADER, PROBE_VALUE)])
            .await;
        assert_eq!(
            blocked.outcome(),
            (FORBIDDEN_STATUS, FORBIDDEN_EXCEPTION.to_string()),
            "case {:?} ({} named by {}): expected the web ACL's refusal, got {} with {}",
            row.case,
            row.operation,
            row.named_by,
            blocked.status,
            blocked.body
        );
        assert_eq!(
            blocked.message(),
            WAF_REFUSAL_MESSAGE,
            "case {:?}: expected the forbidden refusal's own sentence, got {:?}",
            row.case,
            blocked.message()
        );

        let allowed = edge.cognito(row.operation, &row.body, &[]).await;
        assert_eq!(
            &allowed.outcome(),
            control,
            "case {:?} ({}): the same call without the match must answer what it answered \
             with no web ACL in front of the pool ({control:?}), got {} with {}",
            row.case,
            row.operation,
            allowed.status,
            allowed.body
        );
    }
}

/// One case of the inspected request: the statement a rule carries, and whether the
/// call this suite makes matches it.
struct Inspected {
    case: &'static str,
    statement: Value,
    matches: bool,
}

/// RULE: the rule's statement decides over the components of the request the pool was
/// sent — the `FieldToMatch` the statement names, whichever one it is — so a web ACL in
/// front of a pool inspects the whole request rather than one component of it.
#[tokio::test]
async fn every_component_of_the_request_the_pool_was_sent_is_inspected() {
    let edge = Edge::new();
    let pool = Pool::build(&edge, "inspected").await;

    // The members a `FieldToMatch` names, from the model
    // (`smithy_query(query="shape",shape="FieldToMatch")`): the ones a request to an
    // awsJson endpoint carries are its method, its path, its headers and its body.
    let rows = [
        Inspected {
            case: "one named header, matched exactly",
            statement: probe_header_match(),
            matches: true,
        },
        Inspected {
            case: "one named header whose value is not the one the rule hunts for",
            statement: byte_match(
                json!({ "SingleHeader": { "Name": PROBE_HEADER } }),
                "some-other-value",
                "EXACTLY",
            ),
            matches: false,
        },
        Inspected {
            case: "the headers as a whole, matched on the probe value",
            statement: byte_match(
                json!({
                    "Headers": {
                        "MatchPattern": { "All": {} },
                        "MatchScope": "VALUE",
                        "OversizeHandling": "CONTINUE",
                    }
                }),
                PROBE_VALUE,
                "CONTAINS",
            ),
            matches: true,
        },
        Inspected {
            case: "the request body, matched on a value the call carries",
            statement: byte_match(
                json!({ "Body": { "OversizeHandling": "CONTINUE" } }),
                &pool.client_id,
                "CONTAINS",
            ),
            matches: true,
        },
        Inspected {
            case: "the request body, matched on a value no call carries",
            statement: byte_match(
                json!({ "Body": { "OversizeHandling": "CONTINUE" } }),
                "not-in-any-body",
                "CONTAINS",
            ),
            matches: false,
        },
        Inspected {
            case: "the method, which every awsJson call posts with",
            statement: byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
            matches: true,
        },
        Inspected {
            case: "the method, matched against one no awsJson call uses",
            statement: byte_match(json!({ "Method": {} }), "DELETE", "EXACTLY"),
            matches: false,
        },
        Inspected {
            case: "the path, which every awsJson call posts to",
            statement: byte_match(json!({ "UriPath": {} }), "/", "EXACTLY"),
            matches: true,
        },
        Inspected {
            case: "the query string, which an awsJson call carries none of",
            statement: byte_match(json!({ "QueryString": {} }), "flow=password", "CONTAINS"),
            matches: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let web_acl = create_web_acl(
            &edge,
            &format!("inspected-{index}"),
            json!({ "Allow": {} }),
            json!([rule(
                &format!("inspect-{index}"),
                0,
                row.statement.clone(),
                json!({ "Block": {} }),
            )]),
        )
        .await;
        associate(&edge, &web_acl.arn, &pool.arn).await;

        let answered = edge
            .cognito(
                "InitiateAuth",
                &json!({
                    "ClientId": pool.client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
                }),
                &[(PROBE_HEADER, PROBE_VALUE)],
            )
            .await;
        let refused = answered.outcome() == (FORBIDDEN_STATUS, FORBIDDEN_EXCEPTION.to_string());
        assert_eq!(
            refused, row.matches,
            "case {:?}: expected the request to be refused: {}, got {} with {}",
            row.case, row.matches, answered.status, answered.body
        );
    }
}

/// One case of a blocking action's own response: the custom status and body it serves.
struct Custom {
    case: &'static str,
    status: u16,
    body_key: &'static str,
    content_type: &'static str,
    content: &'static str,
}

/// RULE: a terminating action that configures its own `CustomResponse` is served AT THE
/// EDGE — the status, the headers and the body are the web ACL's own, and the pool's
/// error envelope is not used, because the pool never produced the response.
#[tokio::test]
async fn a_blocking_actions_own_custom_response_is_served_in_front_of_the_pool() {
    let edge = Edge::new();
    let pool = Pool::build(&edge, "custom").await;

    // `CustomResponse$ResponseCode` is "the HTTP status code to return to the client",
    // whose modelled range is 200..=599
    // (`smithy_query(query="shape",shape="ResponseStatusCode")`): the rows span it.
    let rows = [
        Custom {
            case: "the lowest status a custom response may serve",
            status: 200,
            body_key: "served-at-the-edge",
            content_type: "TEXT_PLAIN",
            content: "this request was not allowed",
        },
        Custom {
            case: "a throttling status with a JSON body",
            status: 429,
            body_key: "too-many",
            content_type: "APPLICATION_JSON",
            content: "{\"reason\":\"too many sign-in attempts\"}",
        },
        Custom {
            case: "the highest status a custom response may serve",
            status: 599,
            body_key: "unavailable",
            content_type: "TEXT_HTML",
            content: "<html><body>not allowed</body></html>",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // The create carries the plain block action: a `CustomResponseBodyKey` names "one
        // of the keys in the CustomResponseBodies map that you provide for this web ACL",
        // so the action that names a key and the map that defines it have to travel in
        // the SAME save — the update below is where both arrive.
        let web_acl = create_web_acl(
            &edge,
            &format!("custom-{index}"),
            json!({ "Allow": {} }),
            json!([rule(
                &format!("custom-{index}"),
                0,
                probe_header_match(),
                json!({ "Block": {} }),
            )]),
        )
        .await;
        // `CustomResponseBodies` is a map from the key an action names to the body WAF
        // serves for it.
        edge.waf_ok(
            "UpdateWebACL",
            json!({
                "Name": format!("custom-{index}"),
                "Scope": SCOPE,
                "Id": web_acl.id,
                "LockToken": web_acl.lock_token,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&format!("custom-{index}")),
                "Rules": [rule(
                    &format!("custom-{index}"),
                    0,
                    probe_header_match(),
                    json!({
                        "Block": {
                            "CustomResponse": {
                                "ResponseCode": row.status,
                                "CustomResponseBodyKey": row.body_key,
                                "ResponseHeaders": [
                                    { "Name": "x-waf-decision", "Value": row.case },
                                ],
                            }
                        }
                    }),
                )],
                "CustomResponseBodies": {
                    row.body_key: { "ContentType": row.content_type, "Content": row.content },
                },
            }),
        )
        .await;
        associate(&edge, &web_acl.arn, &pool.arn).await;

        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", AUTHORIZATION)
            .header(
                "x-amz-target",
                "AWSCognitoIdentityProviderService.InitiateAuth",
            )
            .header(PROBE_HEADER, PROBE_VALUE)
            .body(Bytes::from(
                json!({
                    "ClientId": pool.client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
                })
                .to_string(),
            ))
            .expect("the test request is well formed");
        let response = edge.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decision = response
            .headers()
            .get("x-waf-decision")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let body = String::from_utf8_lossy(response.body()).into_owned();

        assert_eq!(
            status, row.status,
            "case {:?}: expected the custom response's own status, got {status} with {body:?}",
            row.case
        );
        assert_eq!(
            body, row.content,
            "case {:?}: expected the custom response's own body, got {body:?}",
            row.case
        );
        assert_eq!(
            decision.as_deref(),
            Some(row.case),
            "case {:?}: expected the custom response's own header, got {decision:?}",
            row.case
        );
    }
}

/// RULE: the LIVE association follows the web ACL's current rules — changing a rule's
/// action through `UpdateWebACL` changes what the association decides, with no
/// re-association — and every decision enters the rule's sample population, which is
/// what `GetSampledRequests` reports.
#[tokio::test]
async fn the_live_association_follows_the_rules_current_action() {
    let edge = Edge::new();
    let pool = Pool::build(&edge, "live").await;
    let metric = "live-rule";
    let web_acl = create_web_acl(
        &edge,
        "live-association",
        json!({ "Allow": {} }),
        json!([{
            "Name": "live-rule",
            "Priority": 0,
            "Statement": probe_header_match(),
            "Action": { "Block": {} },
            "VisibilityConfig": visibility(metric),
        }]),
    )
    .await;
    associate(&edge, &web_acl.arn, &pool.arn).await;

    /// One leg: the action the rule carries now, and whether the marked call is refused.
    struct Leg {
        case: &'static str,
        action: Value,
        refused: bool,
    }

    let legs = [
        Leg {
            case: "the rule blocks, as it was created",
            action: json!({ "Block": {} }),
            refused: true,
        },
        Leg {
            case: "the same rule counts instead, on the live association",
            action: json!({ "Count": {} }),
            refused: false,
        },
        Leg {
            case: "and blocks again, still with no re-association",
            action: json!({ "Block": {} }),
            refused: true,
        },
    ];

    let mut lock_token = web_acl.lock_token.clone();
    for (index, leg) in legs.iter().enumerate() {
        // The first leg asserts the web ACL as created; every later one changes the
        // rule's action in place, which needs the CURRENT lock token.
        if index > 0 {
            let updated = edge
                .waf_ok(
                    "UpdateWebACL",
                    json!({
                        "Name": "live-association",
                        "Scope": SCOPE,
                        "Id": web_acl.id,
                        "LockToken": lock_token,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility("live-association"),
                        "Rules": [{
                            "Name": "live-rule",
                            "Priority": 0,
                            "Statement": probe_header_match(),
                            "Action": leg.action,
                            "VisibilityConfig": visibility(metric),
                        }],
                    }),
                )
                .await;
            lock_token = updated["NextLockToken"]
                .as_str()
                .expect("UpdateWebACL hands back the next lock token")
                .to_string();
        }

        let answered = edge
            .cognito(
                "InitiateAuth",
                &json!({
                    "ClientId": pool.client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
                }),
                &[(PROBE_HEADER, PROBE_VALUE)],
            )
            .await;
        let refused = answered.outcome() == (FORBIDDEN_STATUS, FORBIDDEN_EXCEPTION.to_string());
        assert_eq!(
            refused, leg.refused,
            "case {:?}: expected the request to be refused: {}, got {} with {}",
            leg.case, leg.refused, answered.status, answered.body
        );
        if !leg.refused {
            assert!(
                answered.body.get("AuthenticationResult").is_some(),
                "case {:?}: a counted request reaches the pool and signs in, got {}",
                leg.case,
                answered.body
            );
        }
    }

    // Every one of those decisions is in the rule's own sample population, under the
    // action it applied.
    let sampled = edge
        .waf_ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl.arn,
                "RuleMetricName": metric,
                "Scope": SCOPE,
                "TimeWindow": { "StartTime": 0, "EndTime": 4_102_444_800_i64 },
                "MaxItems": 500,
            }),
        )
        .await;
    let population = sampled["PopulationSize"].as_i64().unwrap_or_default();
    assert!(
        population >= legs.len() as i64,
        "case the sample population: expected at least {} decided requests, got {population} \
         in {sampled}",
        legs.len()
    );
    let actions: Vec<String> = sampled["SampledRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|sample| sample["Action"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["BLOCK", "COUNT"] {
        assert!(
            actions.iter().any(|action| action == expected),
            "case the sample actions: expected a {expected} sample, got {actions:?}"
        );
    }
    let probed = sampled["SampledRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|sample| {
            sample["Request"]["Headers"]
                .as_array()
                .is_some_and(|headers| {
                    headers.iter().any(|header| {
                        header["Name"].as_str() == Some(PROBE_HEADER)
                            && header["Value"].as_str() == Some(PROBE_VALUE)
                    })
                })
                && sample["Request"]["Method"].as_str() == Some("POST")
        });
    assert!(
        probed,
        "case the sampled request: expected a sample carrying the probe header and the \
         method the call was made with, got {sampled}"
    );
}

/// One case of what a web ACL does NOT decide.
struct Ungoverned {
    case: &'static str,
    operation: &'static str,
    body: Value,
}

/// RULE: a web ACL decides the operations the model declares `ForbiddenException` for
/// and nothing else, and it decides them only for the pool it is associated with — an
/// operation the model declares no such refusal for, a pool with no web ACL in front of
/// it and a pool whose web ACL has been disassociated are all served as they are with
/// no WAFv2 in the emulator at all.
#[tokio::test]
async fn nothing_the_web_acl_does_not_govern_is_decided_by_it() {
    let edge = Edge::new();
    let protected = Pool::build(&edge, "protected").await;
    let other = Pool::build(&edge, "other").await;

    // A web ACL that blocks EVERYTHING by default: no rule at all, so only the
    // `DefaultAction` decides. Anything it is asked about is refused, which is what
    // makes "it was not asked" observable.
    let web_acl = create_web_acl(
        &edge,
        "blocks-everything",
        json!({ "Block": {} }),
        json!([]),
    )
    .await;
    associate(&edge, &web_acl.arn, &protected.arn).await;

    let sign_in = |pool: &Pool| {
        json!({
            "ClientId": pool.client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
        })
    };

    // The pool the web ACL is in front of: its governed operation is refused by the
    // default action, with no rule and no probe header.
    let refused = edge.cognito("InitiateAuth", &sign_in(&protected), &[]).await;
    assert_eq!(
        refused.outcome(),
        (FORBIDDEN_STATUS, FORBIDDEN_EXCEPTION.to_string()),
        "case the default action: expected the web ACL's refusal, got {} with {}",
        refused.status,
        refused.body
    );

    // Another pool, which nothing was associated with: the same call signs in.
    let served = edge.cognito("InitiateAuth", &sign_in(&other), &[]).await;
    assert!(
        served.status == 200 && served.body.get("AuthenticationResult").is_some(),
        "case a pool with no web ACL: expected the pool's own answer, got {} with {}",
        served.status,
        served.body
    );

    // Operations the model declares no `ForbiddenException` for: the administrative
    // half of the API and the control plane, all on the PROTECTED pool.
    let rows = [
        Ungoverned {
            case: "the administrative sign-in",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": protected.id,
                "ClientId": protected.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": {
                    "USERNAME": protected.username,
                    "PASSWORD": GOOD_PASSWORD,
                },
            }),
        },
        Ungoverned {
            case: "an administrative read of the user",
            operation: "AdminGetUser",
            body: json!({ "UserPoolId": protected.id, "Username": protected.username }),
        },
        Ungoverned {
            case: "a control-plane read of the pool",
            operation: "DescribeUserPool",
            body: json!({ "UserPoolId": protected.id }),
        },
        Ungoverned {
            case: "a control-plane listing of the pool's app clients",
            operation: "ListUserPoolClients",
            body: json!({ "UserPoolId": protected.id }),
        },
    ];
    for row in &rows {
        let answered = edge
            .cognito(row.operation, &row.body, &[(PROBE_HEADER, PROBE_VALUE)])
            .await;
        assert_eq!(
            answered.status, 200,
            "case {:?} ({}): the model declares no forbidden refusal for it, so the web ACL \
             does not decide it; got {} with {}",
            row.case, row.operation, answered.status, answered.body
        );
    }

    // Taking the web ACL away puts the pool back where it started.
    edge.waf_ok(
        "DisassociateWebACL",
        json!({ "ResourceArn": protected.arn }),
    )
    .await;
    let after = edge.cognito("InitiateAuth", &sign_in(&protected), &[]).await;
    assert!(
        after.status == 200 && after.body.get("AuthenticationResult").is_some(),
        "case a disassociated web ACL: expected the pool's own answer, got {} with {}",
        after.status,
        after.body
    );
}

/// One request a web ACL did not allow: the operation that was called, the body that
/// addresses the pool, how the web ACL was written, and whether the call carries the
/// probe header the web ACL's rule hunts for.
struct NotAllowed {
    case: &'static str,
    operation: &'static str,
    body: Value,
    default_action: Value,
    rules: Value,
    probe: bool,
}

/// The requests a web ACL refuses, spanning what the refusal's wording generalises over:
/// which operation was called, which member of the request names the pool, whether a
/// RULE or the `DefaultAction` decided, and which terminating action it applied.
fn not_allowed_rows(pool: &Pool) -> Vec<NotAllowed> {
    let sign_in = json!({
        "ClientId": pool.client_id,
        "AuthFlow": "USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
    });
    // The terminating actions a rule may apply, from the model
    // (`smithy_query(query="shape",shape="RuleAction")`): `Block`, `Captcha` and
    // `Challenge` all end the evaluation, while `Count` lets the request through and so
    // produces no refusal to word.
    vec![
        NotAllowed {
            case: "a rule blocks the sign-in entry point, which names its pool by the app \
                   client",
            operation: "InitiateAuth",
            body: sign_in.clone(),
            default_action: json!({ "Allow": {} }),
            rules: json!([rule("block-the-probe", 0, probe_header_match(), json!({ "Block": {} }))]),
            probe: true,
        },
        NotAllowed {
            case: "a rule blocks a self-service read, which names its pool by the access \
                   token",
            operation: "GetUser",
            body: json!({ "AccessToken": pool.access_token }),
            default_action: json!({ "Allow": {} }),
            rules: json!([rule("block-the-probe", 0, probe_header_match(), json!({ "Block": {} }))]),
            probe: true,
        },
        NotAllowed {
            case: "a rule blocks a request part way through a challenge, which names its \
                   pool by the session",
            operation: "AssociateSoftwareToken",
            body: json!({ "Session": pool.session }),
            default_action: json!({ "Allow": {} }),
            rules: json!([rule("block-the-probe", 0, probe_header_match(), json!({ "Block": {} }))]),
            probe: true,
        },
        NotAllowed {
            case: "no rule matches and the DefaultAction blocks",
            operation: "InitiateAuth",
            body: sign_in.clone(),
            default_action: json!({ "Block": {} }),
            rules: json!([]),
            probe: false,
        },
        NotAllowed {
            case: "a later rule blocks after an earlier one only counted the request",
            operation: "InitiateAuth",
            body: sign_in.clone(),
            default_action: json!({ "Allow": {} }),
            rules: json!([
                rule("count-the-probe", 0, probe_header_match(), json!({ "Count": {} })),
                rule(
                    "block-every-post",
                    1,
                    byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
                    json!({ "Block": {} }),
                ),
            ]),
            probe: true,
        },
        NotAllowed {
            case: "a rule runs a CAPTCHA check, which this endpoint's caller has no \
                   browser to solve",
            operation: "InitiateAuth",
            body: sign_in.clone(),
            default_action: json!({ "Allow": {} }),
            rules: json!([rule(
                "captcha-the-probe",
                0,
                probe_header_match(),
                json!({ "Captcha": {} }),
            )]),
            probe: true,
        },
        NotAllowed {
            case: "a rule runs a silent challenge, which this endpoint's caller has no \
                   browser to run",
            operation: "ForgotPassword",
            body: json!({ "ClientId": pool.client_id, "Username": pool.username }),
            default_action: json!({ "Allow": {} }),
            rules: json!([rule(
                "challenge-the-probe",
                0,
                probe_header_match(),
                json!({ "Challenge": {} }),
            )]),
            probe: true,
        },
    ]
}

/// RULE: a request the web ACL in front of the pool did not allow is refused with HTTP
/// 403 and `ForbiddenException`, whose message is the one sentence AWS words that error
/// class with — fixed text that carries nothing from the request, from the web ACL or
/// from the pool's state, so it does not vary with the operation called, with the rule
/// that decided, or with the terminating action that rule applied.
#[tokio::test]
async fn every_request_a_web_acl_does_not_allow_is_worded_the_one_way() {
    let edge = Edge::new();
    let pool = Pool::build(&edge, "worded").await;

    for (index, row) in not_allowed_rows(&pool).iter().enumerate() {
        let web_acl = create_web_acl(
            &edge,
            &format!("worded-{index}"),
            row.default_action.clone(),
            row.rules.clone(),
        )
        .await;
        associate(&edge, &web_acl.arn, &pool.arn).await;

        let headers: &[(&str, &str)] = if row.probe {
            &[(PROBE_HEADER, PROBE_VALUE)]
        } else {
            &[]
        };
        let refused = edge.cognito(row.operation, &row.body, headers).await;
        assert_eq!(
            refused.outcome(),
            (FORBIDDEN_STATUS, FORBIDDEN_EXCEPTION.to_string()),
            "case {:?} ({}): expected the web ACL's refusal, got {} with {}",
            row.case,
            row.operation,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            WAF_REFUSAL_MESSAGE,
            "case {:?} ({}): expected the one sentence this refusal is worded with, got {:?}",
            row.case,
            row.operation,
            refused.message()
        );
    }
}
