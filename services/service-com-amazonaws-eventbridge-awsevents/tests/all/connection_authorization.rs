// SPDX-License-Identifier: CC0-1.0

//! What a connection reports about its credentials, and how it becomes usable —
//! end to end over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * no value a write declared SECRET is ever reported by a read. A secret value
//!   lives in the connection's secret, which `SecretArn` names and no operation
//!   of this service reads back, so the response shapes carry the non-secret half
//!   only: a user name, an API key NAME and a client id, and — for the header,
//!   query-string and body parameters a connection sends — the key and the
//!   `IsValueSecret` flag, with the value withheld wherever that flag is set.
//!   A value the write did not flag secret is reported as it was given;
//! * a connection whose credentials ARE the authorization is usable as soon as it
//!   is written (the create answers `AUTHORIZED`), while an
//!   `OAUTH_CLIENT_CREDENTIALS` connection must first exchange its client
//!   credentials for a token at its own authorization endpoint ("OAUTH tokens are
//!   refreshed when a 401 or 407 response is returned"): it is `AUTHORIZING`
//!   while that exchange is outstanding and settles to `AUTHORIZED` when the
//!   endpoint issued a token and `DEAUTHORIZED` when nothing answered — and an
//!   API destination is `ACTIVE` only while the connection it invokes through is
//!   authorized.
//!
//! Each table spans the class rather than one case: the rows differ along the
//! authorization type, along which parameter block (invocation or OAuth) and
//! which kind of parameter (header, query string, body) carries the value, and
//! along whether that value is flagged secret.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// An authorization endpoint this emulator serves nothing at, so the token
/// exchange made against it reaches no endpoint at all. Requests go through the
/// in-process transport only: no socket is opened and no name is resolved.
const UNREACHABLE_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// How long a test waits before reading a connection whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
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

/// The OAuth parameters of a connection whose authorization endpoint nothing
/// answers at.
fn oauth_parameters() -> Value {
    json!({
        "OAuthParameters": {
            "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit-secret" },
            "AuthorizationEndpoint": UNREACHABLE_ENDPOINT,
            "HttpMethod": "POST",
        },
    })
}

/// The whole response body of a read of `name`, as text: what a principal
/// allowed to read connection metadata gets to see.
async fn rendered_reads(router: &MultiServiceRouter, name: &str) -> String {
    let (status, described) = events(router, "DescribeConnection", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: DescribeConnection answered {status} {described}"
    );
    let (status, listed) = events(router, "ListConnections", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {name}: ListConnections answered {status} {listed}"
    );
    format!("{described}{listed}")
}

/// One row of the secret-withholding table: the case, the `AuthorizationType`,
/// its `AuthParameters`, the path of the parameter list a read must report, and
/// the credentials no read may contain.
type SecretRow = (
    &'static str,
    &'static str,
    Value,
    Vec<&'static str>,
    Vec<&'static str>,
);

/// Every parameter value a write flagged secret is withheld from every read,
/// while the key, the flag and the values NOT flagged secret are reported.
#[tokio::test]
async fn no_value_flagged_secret_is_ever_reported() {
    // (case, the AuthorizationType, its AuthParameters, the path of the
    //  parameter list a read must report, the secrets no read may contain)
    let rows: Vec<SecretRow> = vec![
        (
            "an API key, with secret and plain header parameters",
            "API_KEY",
            json!({
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
                "InvocationHttpParameters": {
                    "HeaderParameters": [
                        { "Key": "x-secret", "Value": "sekrit-header", "IsValueSecret": true },
                        { "Key": "x-plain", "Value": "plain-header", "IsValueSecret": false },
                    ],
                },
            }),
            vec!["InvocationHttpParameters", "HeaderParameters"],
            vec!["sekrit-key", "sekrit-header"],
        ),
        (
            "basic credentials, with secret and plain query-string parameters",
            "BASIC",
            json!({
                "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
                "InvocationHttpParameters": {
                    "QueryStringParameters": [
                        { "Key": "token", "Value": "sekrit-query", "IsValueSecret": true },
                        { "Key": "page", "Value": "plain-query", "IsValueSecret": false },
                    ],
                },
            }),
            vec!["InvocationHttpParameters", "QueryStringParameters"],
            vec!["sekrit-password", "sekrit-query"],
        ),
        (
            "basic credentials, with secret and plain body parameters",
            "BASIC",
            json!({
                "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
                "InvocationHttpParameters": {
                    "BodyParameters": [
                        { "Key": "secret-field", "Value": "sekrit-body", "IsValueSecret": true },
                        { "Key": "plain-field", "Value": "plain-body", "IsValueSecret": false },
                    ],
                },
            }),
            vec!["InvocationHttpParameters", "BodyParameters"],
            vec!["sekrit-password", "sekrit-body"],
        ),
        (
            "OAuth client credentials, whose exchange carries its own parameters",
            "OAUTH_CLIENT_CREDENTIALS",
            json!({
                "OAuthParameters": {
                    "ClientParameters": {
                        "ClientID": "the-client",
                        "ClientSecret": "sekrit-client-secret",
                    },
                    "AuthorizationEndpoint": UNREACHABLE_ENDPOINT,
                    "HttpMethod": "POST",
                    "OAuthHttpParameters": {
                        "HeaderParameters": [
                            { "Key": "x-secret", "Value": "sekrit-oauth", "IsValueSecret": true },
                            { "Key": "x-plain", "Value": "plain-oauth", "IsValueSecret": false },
                        ],
                    },
                },
            }),
            vec!["OAuthParameters", "OAuthHttpParameters", "HeaderParameters"],
            vec!["sekrit-client-secret", "sekrit-oauth"],
        ),
    ];

    for (case, kind, parameters, path, secrets) in &rows {
        let router = emulator();
        let name = "the-connection";
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({ "Name": name, "AuthorizationType": kind, "AuthParameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );

        let (status, described) =
            events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeConnection answered {status} {described}"
        );
        // The parameter list is still reported, so a client can see WHICH
        // parameters the connection sends — the values flagged secret are the
        // only thing withheld.
        let mut cursor = &described["AuthParameters"];
        for step in path {
            cursor = &cursor[*step];
        }
        let reported = cursor.as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected AuthParameters{path:?}, got {described}")
        });
        let submitted = {
            let mut cursor = parameters;
            for step in path {
                cursor = &cursor[*step];
            }
            cursor.as_array().cloned().unwrap_or_default()
        };
        assert_eq!(
            reported.len(),
            submitted.len(),
            "case {case:?}: every parameter is reported, got {described}"
        );
        for (held, sent) in reported.iter().zip(submitted.iter()) {
            assert_eq!(
                held["Key"], sent["Key"],
                "case {case:?}: a read reports the parameter's key, got {held}"
            );
            assert_eq!(
                held["IsValueSecret"], sent["IsValueSecret"],
                "case {case:?}: a read reports whether the value is secret, got {held}"
            );
            let is_secret = sent["IsValueSecret"] == json!(true);
            let expected_value = if is_secret { Value::Null } else { sent["Value"].clone() };
            assert_eq!(
                held["Value"], expected_value,
                "case {case:?}: expected the value {expected_value} for {held}, got {held}"
            );
        }

        // Nothing a read hands out carries a credential, on any read surface.
        let rendered = rendered_reads(&router, name).await;
        for secret in secrets {
            assert!(
                !rendered.contains(secret),
                "case {case:?}: a read must report no credential, but {secret:?} is in {rendered}"
            );
        }
        // The secret the credentials live in IS named, which is the indirection
        // the withheld values exist for.
        assert!(
            described["SecretArn"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!("arn:aws:secretsmanager:{REGION}:")),
            "case {case:?}: expected the ARN of the connection's secret, got {described}"
        );
    }
}

/// A connection whose OAuth authorization endpoint is not an `https` URL is
/// refused: `AuthorizationEndpoint` is modelled as an `HttpsEndpoint`, and an
/// endpoint no token request could be made to is not one to store.
#[tokio::test]
async fn an_authorization_endpoint_that_is_not_https_is_refused() {
    // (case, the endpoint, whether it is one EventBridge accepts)
    let rows: [(&str, &str, bool); 5] = [
        ("an https URL", UNREACHABLE_ENDPOINT, true),
        ("an https URL in mixed case", "HTTPS://authorization.example.com/oauth/token", true),
        ("a plain http URL", "http://authorization.example.com/oauth/token", false),
        ("another scheme entirely", "ftp://authorization.example.com/token", false),
        ("a host and path with no scheme", "authorization.example.com/token", false),
    ];

    for (case, endpoint, accepted) in rows {
        let router = emulator();
        let name = "the-connection";
        let request = json!({
            "Name": name,
            "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
            "AuthParameters": {
                "OAuthParameters": {
                    "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit" },
                    "AuthorizationEndpoint": endpoint,
                    "HttpMethod": "POST",
                },
            },
        });
        let (status, body) = events(&router, "CreateConnection", &request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: an https endpoint must be accepted, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: an endpoint that is not https must be refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("AuthorizationEndpoint"),
            "case {case:?}: the refusal names the member, got {body}"
        );

        // The same rule on the other writer of a connection's credentials.
        let (status, created) = events(
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
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        let (status, body) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": {
                    "OAuthParameters": {
                        "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit" },
                        "AuthorizationEndpoint": endpoint,
                        "HttpMethod": "POST",
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateConnection must refuse it too, got {status} {body}"
        );
    }
}

/// One row of the authorization table: the case, the `AuthorizationType`, its
/// `AuthParameters`, the state the create is answered in, the state it settles
/// in, and the `StateReason` it then reports.
type AuthorizationRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// The state a created connection is in, and the one the clock then owes it,
/// follow from whether its credentials are themselves the authorization.
#[tokio::test]
async fn a_connection_is_authorized_by_the_exchange_its_type_needs() {
    let rows: Vec<AuthorizationRow> = vec![
        (
            "an API key, which IS the authorization",
            "API_KEY",
            json!({
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
            }),
            state::CONNECTION_STATE_AUTHORIZED,
            state::CONNECTION_STATE_AUTHORIZED,
            None,
        ),
        (
            "basic credentials, which ARE the authorization",
            "BASIC",
            json!({
                "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
            }),
            state::CONNECTION_STATE_AUTHORIZED,
            state::CONNECTION_STATE_AUTHORIZED,
            None,
        ),
        (
            "OAuth client credentials, whose authorization endpoint nothing answers at",
            "OAUTH_CLIENT_CREDENTIALS",
            oauth_parameters(),
            state::CONNECTION_STATE_AUTHORIZING,
            state::CONNECTION_STATE_DEAUTHORIZED,
            Some(state::ENDPOINT_NOT_REACHED),
        ),
    ];

    for (case, kind, parameters, created_state, settled_state, reason) in &rows {
        let router = emulator();
        let name = "the-connection";
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({ "Name": name, "AuthorizationType": kind, "AuthParameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        assert_eq!(
            created["ConnectionState"],
            json!(created_state),
            "case {case:?}: a created connection is answered in {created_state}, got {created}"
        );

        // The state the create answered is the state a caller reading straight
        // after it sees.
        let (_, transient) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            transient["ConnectionState"],
            json!(created_state),
            "case {case:?}: a read taken at once still reports {created_state}, got {transient}"
        );

        tokio::time::sleep(past_the_transition()).await;
        let (_, settled) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            settled["ConnectionState"],
            json!(settled_state),
            "case {case:?}: the clock owes the connection {settled_state}, got {settled}"
        );
        assert_eq!(
            settled["StateReason"],
            reason.map_or(Value::Null, |reason| json!(reason)),
            "case {case:?}: expected the state's own reason, got {settled}"
        );
        // A listing reports the same state as the read of one connection.
        let (_, listed) = events(&router, "ListConnections", &json!({})).await;
        assert_eq!(
            listed["Connections"][0]["ConnectionState"],
            json!(settled_state),
            "case {case:?}: a listing and a read cannot disagree, got {listed}"
        );

        // "The destination endpoint must support the authorization type
        // specified for the connection": a destination is invocable only while
        // its connection can authorize the invocation.
        let (status, destination) = events(
            &router,
            "CreateApiDestination",
            &json!({
                "Name": "the-destination",
                "ConnectionArn": settled["ConnectionArn"],
                "InvocationEndpoint": "https://destination.example.com/events",
                "HttpMethod": "POST",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApiDestination answered {status} {destination}"
        );
        let (_, read) = events(
            &router,
            "DescribeApiDestination",
            &json!({ "Name": "the-destination" }),
        )
        .await;
        let expected_destination_state = if *settled_state == state::CONNECTION_STATE_AUTHORIZED {
            state::API_DESTINATION_STATE_ACTIVE
        } else {
            state::API_DESTINATION_STATE_INACTIVE
        };
        assert_eq!(
            read["ApiDestinationState"],
            json!(expected_destination_state),
            "case {case:?}: a destination is {expected_destination_state} while its connection \
             is {settled_state}, got {read}"
        );
    }
}
