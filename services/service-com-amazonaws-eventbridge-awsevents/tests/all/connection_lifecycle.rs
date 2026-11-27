// SPDX-License-Identifier: CC0-1.0

//! The connection resource end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`:
//! `CreateConnection` writes it, `DescribeConnection` reports it back,
//! `DeauthorizeConnection` strips its credentials and `DeleteConnection` removes
//! it.
//!
//! The RULES asserted here:
//!
//! * a created connection is a PERSISTED resource: it answers an ARN of the
//!   modelled `ConnectionArn` form (`…:connection/<name>/<id>`), it is readable
//!   back under its name through `DescribeConnection` with every member it was
//!   created with, and its ARN and timestamps do not change between reads;
//! * a connection AWS answers in a non-terminal state reaches its next state on
//!   its own, a moment later rather than instantly: `DEAUTHORIZING` ->
//!   `DEAUTHORIZED`, `DELETING` -> gone. A caller reading straight after the call
//!   still observes the transient state, while a connection the call itself
//!   settled (`AUTHORIZED`) stays where it is;
//! * no credential a connection was created with is ever reported: the response
//!   parameters carry a user name, an API key NAME and a client id, and never the
//!   password, key value or client secret;
//! * the name is the connection's identity — a second create under one name is
//!   `ResourceAlreadyExistsException`, and every operation naming a connection
//!   that does not exist is `ResourceNotFoundException`;
//! * "you must include only authorization parameters for the AuthorizationType
//!   you specify" (`CreateConnection$AuthParameters`), so a request whose
//!   parameters do not match its type is refused.
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along the authorization type and its parameter block, along which member of a
//! request is wrong, and along which operation names a connection that is not
//! there.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// How long a test waits before reading a connection whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not
/// make the wait the flaky part of the assertion.
fn past_the_transition() -> Duration {
    state::CONNECTION_TRANSITION + Duration::from_millis(250)
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The emulator the suite calls: EventBridge, and Secrets Manager beside it on the
/// SAME engine — a connection's credential is held in a secret that service holds,
/// so the ARN a read reports is one Secrets Manager allocated.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    router
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// One authorization type, the parameter block that belongs to it, the members a
/// read must report back, and the secrets a read must never report.
struct AuthCase {
    kind: &'static str,
    parameters: Value,
    /// The `ConnectionState` a create of this type is answered in.
    created_state: &'static str,
    /// The `ConnectionState` the clock then owes it.
    settled_state: &'static str,
    /// (the path into `AuthParameters`, the value the read must report)
    reported: Vec<(Vec<&'static str>, Value)>,
    /// Text a read must not contain anywhere: the credentials themselves.
    secrets: Vec<&'static str>,
}

/// Every `ConnectionAuthorizationType` the model admits, with its own parameter
/// block. Source: `smithy_query(query="operation", operation="CreateConnection")`
/// — `AuthorizationType` is one of `API_KEY | BASIC |
/// OAUTH_CLIENT_CREDENTIALS`, and `CreateConnectionAuthRequestParameters` carries
/// exactly one member per type.
///
/// The states differ with the type: credentials that ARE the authorization are
/// accepted by the create itself, which leaves the connection `AUTHORIZED` with
/// nothing further owed to it, while OAuth client credentials have to be
/// exchanged for a token at the connection's authorization endpoint first, so
/// the connection is `AUTHORIZING` and settles by what that exchange came to —
/// `DEAUTHORIZED` here, because nothing answers at the endpoint the row names.
fn auth_cases() -> Vec<AuthCase> {
    vec![
        AuthCase {
            kind: "API_KEY",
            parameters: json!({
                "ApiKeyAuthParameters": {
                    "ApiKeyName": "x-api-key",
                    "ApiKeyValue": "sekrit-api-key-value",
                },
            }),
            created_state: state::CONNECTION_STATE_AUTHORIZED,
            settled_state: state::CONNECTION_STATE_AUTHORIZED,
            reported: vec![(
                vec!["ApiKeyAuthParameters", "ApiKeyName"],
                json!("x-api-key"),
            )],
            secrets: vec!["sekrit-api-key-value"],
        },
        AuthCase {
            kind: "BASIC",
            parameters: json!({
                "BasicAuthParameters": { "Username": "deep-user", "Password": "sekrit-password" },
            }),
            created_state: state::CONNECTION_STATE_AUTHORIZED,
            settled_state: state::CONNECTION_STATE_AUTHORIZED,
            reported: vec![(vec!["BasicAuthParameters", "Username"], json!("deep-user"))],
            secrets: vec!["sekrit-password"],
        },
        AuthCase {
            kind: "OAUTH_CLIENT_CREDENTIALS",
            parameters: json!({
                "OAuthParameters": {
                    "ClientParameters": {
                        "ClientID": "deep-client",
                        "ClientSecret": "sekrit-client-secret",
                    },
                    "AuthorizationEndpoint": "https://auth.example.com/token",
                    "HttpMethod": "POST",
                },
            }),
            created_state: state::CONNECTION_STATE_AUTHORIZING,
            settled_state: state::CONNECTION_STATE_DEAUTHORIZED,
            reported: vec![
                (
                    vec!["OAuthParameters", "ClientParameters", "ClientID"],
                    json!("deep-client"),
                ),
                (
                    vec!["OAuthParameters", "AuthorizationEndpoint"],
                    json!("https://auth.example.com/token"),
                ),
                (vec!["OAuthParameters", "HttpMethod"], json!("POST")),
            ],
            secrets: vec!["sekrit-client-secret"],
        },
    ]
}

/// The member at `path` of `body`, or `Value::Null`.
fn at<'body>(body: &'body Value, path: &[&str]) -> &'body Value {
    let mut cursor = body;
    for step in path {
        cursor = &cursor[*step];
    }
    cursor
}

/// A created connection carries an ARN of the modelled form, reads back under its
/// name with every member it was created with, and settles into `AUTHORIZED` on
/// its own — reporting no credential on the way.
#[tokio::test]
async fn a_created_connection_is_read_back_and_settles_into_authorized() {
    let account = engine::emulator_account_id();
    for case in auth_cases() {
        let router = emulator();
        let name = format!("conn-{}", case.kind.to_lowercase().replace('_', "-"));
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": name,
                "Description": "the deep http task's connection",
                "AuthorizationType": case.kind,
                "AuthParameters": case.parameters,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: CreateConnection answered {status} {created}",
            case.kind
        );

        // `ConnectionArn`'s modelled pattern ends `:connection/<name>/<id>`,
        // where the id segment is `[\-A-Za-z0-9]+`.
        let arn = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no ConnectionArn in {created}", case.kind))
            .to_string();
        let prefix = format!("arn:aws:events:{REGION}:{account}:connection/{name}/");
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!(
                "case {}: expected an ARN starting {prefix:?}, got {arn:?}",
                case.kind
            )
        });
        assert!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "case {}: expected a hyphen-and-alphanumeric id segment, got {id:?}",
            case.kind
        );
        assert_eq!(
            created["ConnectionState"],
            json!(case.created_state),
            "case {}: a created connection is answered in {}, got {created}",
            case.kind,
            case.created_state
        );
        for member in ["CreationTime", "LastModifiedTime"] {
            assert!(
                created[member].as_f64().is_some(),
                "case {}: expected a {member} on the create, got {created}",
                case.kind
            );
        }

        // The state the create answered is the state a caller reading straight
        // after it sees: a transient one is still observable, and one the call
        // settled has not moved.
        let (status, transient) =
            events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {}: DescribeConnection answered {status} {transient}",
            case.kind
        );
        assert_eq!(
            transient["ConnectionState"],
            json!(case.created_state),
            "case {}: a read taken at once still reports {}, got {transient}",
            case.kind,
            case.created_state
        );

        tokio::time::sleep(past_the_transition()).await;
        let (status, settled) =
            events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {}: DescribeConnection answered {status} {settled}",
            case.kind
        );
        assert_eq!(
            settled["ConnectionState"],
            json!(case.settled_state),
            "case {}: the clock owes the connection {}, got {settled}",
            case.kind,
            case.settled_state
        );
        assert_eq!(
            settled["ConnectionArn"],
            json!(arn),
            "case {}: a read reports the stored ARN, got {settled}",
            case.kind
        );
        assert_eq!(
            settled["Name"],
            json!(name),
            "case {}: a read reports the stored name, got {settled}",
            case.kind
        );
        assert_eq!(
            settled["Description"],
            json!("the deep http task's connection"),
            "case {}: a read reports the stored description, got {settled}",
            case.kind
        );
        assert_eq!(
            settled["AuthorizationType"],
            json!(case.kind),
            "case {}: a read reports the stored authorization type, got {settled}",
            case.kind
        );
        assert_eq!(
            settled["LastAuthorizedTime"].as_f64().is_some(),
            case.settled_state == state::CONNECTION_STATE_AUTHORIZED,
            "case {}: only an authorized connection reports when it was authorized, \
             got {settled}",
            case.kind
        );
        // `SecretArn` is "the ARN of the secret created from the authorization
        // parameters specified for the connection"; its modelled pattern is a
        // Secrets Manager secret ARN.
        let secret = settled["SecretArn"].as_str().unwrap_or_default();
        assert!(
            secret.starts_with(&format!("arn:aws:secretsmanager:{REGION}:{account}:secret:"))
                && secret.contains(&name),
            "case {}: expected a secret ARN naming the connection, got {secret:?}",
            case.kind
        );

        for (path, expected) in &case.reported {
            assert_eq!(
                at(&settled["AuthParameters"], path),
                expected,
                "case {}: expected AuthParameters{path:?} to be {expected}, got {settled}",
                case.kind
            );
        }
        let rendered = settled.to_string();
        for secret in &case.secrets {
            assert!(
                !rendered.contains(secret),
                "case {}: a read must report no credential, but {secret:?} is in {rendered}",
                case.kind
            );
        }

        // Nothing about the connection moves because it was read: a second read
        // reports the same ARN and the same timestamps.
        let (_, again) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        for member in [
            "ConnectionArn",
            "ConnectionState",
            "CreationTime",
            "LastModifiedTime",
            "LastAuthorizedTime",
            "SecretArn",
        ] {
            assert_eq!(
                again[member], settled[member],
                "case {}: {member} must not change because the connection was read, \
                 got {again}",
                case.kind
            );
        }
    }
}

/// `DeauthorizeConnection` strips the credentials and keeps the connection;
/// `DeleteConnection` removes it, and its name is then free for a connection with
/// a NEW ARN.
#[tokio::test]
async fn deauthorizing_and_deleting_move_the_connection_through_its_states() {
    let router = emulator();
    let name = "deep-connection";
    let created = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(created.0, 200, "CreateConnection answered {:?}", created);
    let first_arn = created.1["ConnectionArn"].as_str().unwrap_or_default().to_string();
    tokio::time::sleep(past_the_transition()).await;

    let (status, deauthorized) =
        events(&router, "DeauthorizeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "DeauthorizeConnection answered {status} {deauthorized}"
    );
    assert_eq!(
        deauthorized["ConnectionState"],
        json!(state::CONNECTION_STATE_DEAUTHORIZING),
        "deauthorizing is answered in {}, got {deauthorized}",
        state::CONNECTION_STATE_DEAUTHORIZING
    );
    let (_, transient) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        transient["ConnectionState"],
        json!(state::CONNECTION_STATE_DEAUTHORIZING),
        "a read taken at once still reports {}, got {transient}",
        state::CONNECTION_STATE_DEAUTHORIZING
    );

    tokio::time::sleep(past_the_transition()).await;
    let (_, settled) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        settled["ConnectionState"],
        json!(state::CONNECTION_STATE_DEAUTHORIZED),
        "the clock owes the connection {}, got {settled}",
        state::CONNECTION_STATE_DEAUTHORIZED
    );
    // "Removes ALL authorization parameters from the connection": the connection
    // is still there, under the same ARN, carrying none.
    assert_eq!(
        settled["ConnectionArn"],
        json!(first_arn),
        "a deauthorized connection keeps its ARN, got {settled}"
    );
    assert_eq!(
        settled["AuthParameters"],
        json!({}),
        "a deauthorized connection reports no authorization parameters, got {settled}"
    );
    assert!(
        settled["LastAuthorizedTime"].as_f64().is_some(),
        "when the connection WAS last authorized does not stop being true, got {settled}"
    );

    let (status, deleted) = events(&router, "DeleteConnection", &json!({ "Name": name })).await;
    assert_eq!(status, 200, "DeleteConnection answered {status} {deleted}");
    assert_eq!(
        deleted["ConnectionState"],
        json!(state::CONNECTION_STATE_DELETING),
        "deleting is answered in {}, got {deleted}",
        state::CONNECTION_STATE_DELETING
    );
    let (_, deleting) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        deleting["ConnectionState"],
        json!(state::CONNECTION_STATE_DELETING),
        "a read taken at once still reports {}, got {deleting}",
        state::CONNECTION_STATE_DELETING
    );

    tokio::time::sleep(past_the_transition()).await;
    let (status, gone) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        status, 400,
        "a deleted connection is gone, got {status} {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for a deleted connection, got {gone}"
    );

    // The name is free again, and the connection created under it is a NEW
    // resource: its ARN carries a different id segment.
    let (status, recreated) = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {recreated}");
    assert_ne!(
        recreated["ConnectionArn"],
        json!(first_arn),
        "a connection recreated under a freed name is a new resource, got {recreated}"
    );
}

/// A connection still in a non-terminal state is writable: only one already on
/// its way OUT conflicts, so creating a connection and deleting it at once
/// works, and a second delete while the first is still due is refused.
#[tokio::test]
async fn only_a_connection_already_being_removed_conflicts_with_a_write() {
    // (case, the operation taken at once after the create, the status it
    //  answers, the wire code it answers)
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a delete taken at once after the create",
            "DeleteConnection",
            200,
            "",
        ),
        (
            "a deauthorize taken at once after the create",
            "DeauthorizeConnection",
            200,
            "",
        ),
    ];

    for (case, operation, expected_status, expected_code) in rows {
        let router = emulator();
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": "at-once",
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        // No wait: the connection is still in the state the create left it in.
        let (status, body) = events(&router, operation, &json!({ "Name": "at-once" })).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the wire code {expected_code}, got {body}"
            );
        }
    }

    // A delete while a delete is already due is the one conflict: the removal
    // the clock owes the connection will happen, so a second one is refused.
    let router = emulator();
    let (status, created) = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": "twice",
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {created}");
    let (status, first) = events(&router, "DeleteConnection", &json!({ "Name": "twice" })).await;
    assert_eq!(status, 200, "the first delete answered {status} {first}");
    let (status, second) = events(&router, "DeleteConnection", &json!({ "Name": "twice" })).await;
    assert_eq!(
        status, 400,
        "a second delete while the first is due must be refused, got {status} {second}"
    );
    assert_eq!(
        error_code(&second),
        "ConcurrentModificationException",
        "expected ConcurrentModificationException for a connection already being removed, \
         got {second}"
    );
}

/// What the connection operations refuse, and with which modelled error.
#[tokio::test]
async fn the_connection_operations_refuse_what_the_resource_rules_forbid() {
    // (case, operation, request, expected status, expected wire code)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a second create under one name",
            "CreateConnection",
            json!({
                "Name": "taken",
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
            400,
            "ResourceAlreadyExistsException",
        ),
        (
            "a read of a connection that does not exist",
            "DescribeConnection",
            json!({ "Name": "absent" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a deauthorize of a connection that does not exist",
            "DeauthorizeConnection",
            json!({ "Name": "absent" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a delete of a connection that does not exist",
            "DeleteConnection",
            json!({ "Name": "absent" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "API_KEY with the parameters of another type",
            "CreateConnection",
            json!({
                "Name": "mismatched",
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "BASIC with no parameters of its own",
            "CreateConnection",
            json!({
                "Name": "parameterless",
                "AuthorizationType": "BASIC",
                "AuthParameters": {},
            }),
            400,
            "ValidationException",
        ),
        (
            "OAUTH_CLIENT_CREDENTIALS with an API key block beside its own",
            "CreateConnection",
            json!({
                "Name": "doubled",
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": {
                    "OAuthParameters": {
                        "ClientParameters": { "ClientID": "c", "ClientSecret": "s" },
                        "AuthorizationEndpoint": "https://auth.example.com/token",
                        "HttpMethod": "POST",
                    },
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "an authorization type the enum does not have",
            "CreateConnection",
            json!({
                "Name": "unknown-type",
                "AuthorizationType": "MUTUAL_TLS",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                },
            }),
            400,
            "ValidationException",
        ),
    ];

    let router = emulator();
    // The one connection the "already exists" row collides with.
    let (status, body) = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": "taken",
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "the colliding create answered {status} {body}");

    for (case, operation, request, expected_status, expected_code) in rows {
        let (status, body) = events(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the wire code {expected_code}, got {body}"
        );
    }
}
