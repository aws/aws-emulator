// SPDX-License-Identifier: CC0-1.0

//! The AWS Secrets Manager secret a CONNECTION's credential is held in, end to end
//! over the wire (`awsJson1_1`, `POST /`) through the real `MultiServiceRouter` with
//! EventBridge and Secrets Manager mounted on ONE engine.
//!
//! The RULE asserted here: EventBridge holds the authorization credential a
//! connection is created or updated with in a secret it OWNS in Secrets Manager and
//! reports that secret's ARN as the connection's `SecretArn` ("the ARN of the secret
//! created from the authorization parameters specified for the connection"). So,
//! while the connection exists:
//!
//! * `GetSecretValue` on the announced ARN answers with the credential the
//!   connection was created with, as the one JSON document the secret holds;
//! * `DescribeSecret` reports `OwningService` for the service that created it ("the
//!   ID of the service that created this secret"), no deletion date, and one version;
//! * because the secret is that service's, a caller's own `DeleteSecret` of it —
//!   even a force delete — is refused with `InvalidRequestException` and changes
//!   nothing;
//! * an update that supplies new credentials stakes a new VERSION of the SAME
//!   secret: the ARN is unchanged, the value read back is the new one, and the
//!   earlier version is still in `VersionIdsToStages`;
//! * deleting the connection removes the secret, after which `GetSecretValue` on
//!   that ARN answers `ResourceNotFoundException`.
//!
//! The table spans the class rather than one recorded case: the rows differ along
//! the authorization type and so along which credential members the secret's
//! document carries, and each row rotates a different member of its credential.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The service id a secret EventBridge created reports as its `OwningService`, which
/// is also the prefix such a secret is named under.
const OWNING_SERVICE: &str = "events";

/// How long a poll waits for a condition another call made due — generously above any
/// in-process call, so a loaded machine does not decide the assertion.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How long one pass of a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The emulator the suite calls: EventBridge and Secrets Manager over ONE engine,
/// because the secret a connection announces is the other service's resource and is
/// read where a client reads it.
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

/// One `awsJson1_1` call to one of the two services.
async fn call(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{service}/\
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

/// One call to EventBridge.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "events", "AWSEvents", operation, body).await
}

/// One call to Secrets Manager.
async fn secrets(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "secretsmanager", "secretsmanager", operation, body).await
}

/// The wire error code of a failure body (`__type` on awsJson), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit(['#', '/']).next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `message` of a failure body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `SecretString` of a secret, parsed as the JSON document it is.
fn stored_document(body: &Value, case: &str) -> Value {
    let text = body["SecretString"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SecretString, got {body}"));
    serde_json::from_str::<Value>(text).unwrap_or_else(|error| {
        panic!("case {case:?}: the SecretString is not JSON ({error}): {text:?}")
    })
}

/// The version ids of a secret, from `DescribeSecret`'s `VersionIdsToStages`.
fn version_ids(described: &Value, case: &str) -> Vec<String> {
    let versions = described["VersionIdsToStages"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected VersionIdsToStages, got {described}"));
    let mut ids: Vec<String> = versions.keys().cloned().collect();
    ids.sort();
    ids
}

/// One authorization type: the parameters a connection of it is created and updated
/// with, and the document its secret holds in each case.
struct Case {
    /// The `ConnectionAuthorizationType` this row creates its connection with.
    kind: &'static str,
    /// The connection name this row uses.
    name: &'static str,
    /// `AuthParameters` for the create.
    create: Value,
    /// `AuthParameters` for the update, which replaces ONE member of the credential.
    update: Value,
    /// The document the secret holds after the create.
    stored: Value,
    /// The document the secret holds after the update.
    rotated: Value,
    /// The credential VALUES a read of the connection must never report: the half
    /// the response shapes withhold (a password, a key value, a client secret),
    /// which lives in the secret and nowhere else.
    withheld: Vec<&'static str>,
}

/// Every `ConnectionAuthorizationType` the model admits, with the credential members
/// of its own request block. Source:
/// `smithy_query(query="shape",shape="CreateConnectionAuthRequestParameters")` —
/// `API_KEY` carries `ApiKeyAuthParameters` (`ApiKeyName`, `ApiKeyValue`), `BASIC`
/// carries `BasicAuthParameters` (`Username`, `Password`) and
/// `OAUTH_CLIENT_CREDENTIALS` carries `OAuthParameters` (`ClientParameters` with
/// `ClientID` and `ClientSecret`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            kind: "API_KEY",
            name: "api-key-connection",
            create: json!({
                "ApiKeyAuthParameters": {
                    "ApiKeyName": "x-emulator-api-key",
                    "ApiKeyValue": "key-the-first",
                },
            }),
            update: json!({
                "ApiKeyAuthParameters": {
                    "ApiKeyName": "x-emulator-api-key",
                    "ApiKeyValue": "key-the-second",
                },
            }),
            stored: json!({
                "api_key_name": "x-emulator-api-key",
                "api_key_value": "key-the-first",
            }),
            rotated: json!({
                "api_key_name": "x-emulator-api-key",
                "api_key_value": "key-the-second",
            }),
            withheld: vec!["key-the-first"],
        },
        Case {
            kind: "BASIC",
            name: "basic-connection",
            create: json!({
                "BasicAuthParameters": { "Username": "deep", "Password": "pass-the-first" },
            }),
            update: json!({
                "BasicAuthParameters": { "Username": "deep", "Password": "pass-the-second" },
            }),
            stored: json!({ "username": "deep", "password": "pass-the-first" }),
            rotated: json!({ "username": "deep", "password": "pass-the-second" }),
            withheld: vec!["pass-the-first"],
        },
        Case {
            kind: "OAUTH_CLIENT_CREDENTIALS",
            name: "oauth-connection",
            create: json!({
                "OAuthParameters": {
                    "ClientParameters": {
                        "ClientID": "client-the-only",
                        "ClientSecret": "secret-the-first",
                    },
                    "AuthorizationEndpoint": "https://authorization.example.com/token",
                    "HttpMethod": "POST",
                },
            }),
            update: json!({
                "OAuthParameters": {
                    "ClientParameters": {
                        "ClientID": "client-the-only",
                        "ClientSecret": "secret-the-second",
                    },
                    "AuthorizationEndpoint": "https://authorization.example.com/token",
                    "HttpMethod": "POST",
                },
            }),
            stored: json!({
                "client_id": "client-the-only",
                "client_secret": "secret-the-first",
            }),
            rotated: json!({
                "client_id": "client-the-only",
                "client_secret": "secret-the-second",
            }),
            withheld: vec!["secret-the-first"],
        },
    ]
}

/// A connection's credential is READABLE through the secret its `SecretArn` names,
/// and that secret is one EventBridge owns.
#[tokio::test]
async fn a_connections_credential_is_held_in_a_secret_the_service_owns() {
    let router = emulator();
    let account = ::engine::emulator_account_id();
    for case in cases() {
        let label = case.kind;
        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": case.name,
                "AuthorizationType": case.kind,
                "AuthParameters": case.create,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateConnection answered {status} {created}"
        );

        let (status, described) =
            events(&router, "DescribeConnection", &json!({ "Name": case.name })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeConnection answered {status} {described}"
        );
        let secret_arn = described["SecretArn"].as_str().unwrap_or_default().to_string();
        // The secret is named under the prefix AWS reserves for the owning service,
        // which is what no caller's own `CreateSecret` can mint.
        let expected_prefix = format!(
            "arn:aws:secretsmanager:{REGION}:{account}:secret:{OWNING_SERVICE}!connection/{}/",
            case.name
        );
        assert!(
            secret_arn.starts_with(&expected_prefix),
            "case {label:?}: expected a SecretArn under {expected_prefix:?}, got \
             {secret_arn:?} in {described}"
        );

        // The credential itself: the one document the secret holds, carrying exactly
        // the credential members of the connection's own authorization block.
        let (status, held) = secrets(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetSecretValue on the announced SecretArn answered \
             {status} {held}"
        );
        let document = stored_document(&held, label);
        assert_eq!(
            document, case.stored,
            "case {label:?}: expected the secret to hold {}, got {document}",
            case.stored
        );
        assert_eq!(
            held["ARN"], json!(secret_arn),
            "case {label:?}: the secret read is the one the connection names, got {held}"
        );

        // It is a secret the OWNING SERVICE created, with one version and no deletion
        // pending.
        let (status, secret) = secrets(
            &router,
            "DescribeSecret",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeSecret answered {status} {secret}"
        );
        assert_eq!(
            secret["OwningService"],
            json!(OWNING_SERVICE),
            "case {label:?}: expected OwningService {OWNING_SERVICE:?}, got {secret}"
        );
        assert!(
            secret.get("DeletedDate").is_none_or(Value::is_null),
            "case {label:?}: a connection's live secret reports no deletion date, got {secret}"
        );
        let versions = version_ids(&secret, label);
        assert_eq!(
            versions.len(),
            1,
            "case {label:?}: expected one version for a freshly created connection, got \
             {versions:?}"
        );
    }
}

/// A caller's own delete of a connection's secret is REFUSED, because the secret is
/// the owning service's — and nothing about the secret changes.
#[tokio::test]
async fn a_callers_force_delete_of_a_connections_secret_is_refused() {
    let router = emulator();
    for case in cases() {
        let label = case.kind;
        let secret_arn = create_connection(&router, &case).await;

        // "Operation is not allowed on secret owned by <owning service>": fixed text
        // and the id the secret reports as its `OwningService`.
        let expected = format!("Operation is not allowed on secret owned by {OWNING_SERVICE}");
        for (attempt, body) in [
            (
                "a force delete",
                json!({ "SecretId": secret_arn, "ForceDeleteWithoutRecovery": true }),
            ),
            (
                "a delete into the recovery window",
                json!({ "SecretId": secret_arn, "RecoveryWindowInDays": 7 }),
            ),
        ] {
            let (status, refused) = secrets(&router, "DeleteSecret", &body).await;
            assert_eq!(
                status, 400,
                "case {label:?}/{attempt}: expected 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidRequestException",
                "case {label:?}/{attempt}: expected InvalidRequestException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                expected,
                "case {label:?}/{attempt}: expected {expected:?}, got {refused}"
            );
        }

        // The refusal changed nothing: the credential is still readable and the secret
        // still reports no deletion date.
        let (status, held) = secrets(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused delete must leave the secret readable, got \
             {status} {held}"
        );
        let document = stored_document(&held, label);
        assert_eq!(
            document, case.stored,
            "case {label:?}: expected the secret to still hold {}, got {document}",
            case.stored
        );
        let (status, secret) = secrets(
            &router,
            "DescribeSecret",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeSecret answered {status} {secret}"
        );
        assert!(
            secret.get("DeletedDate").is_none_or(Value::is_null),
            "case {label:?}: the refused delete must schedule nothing, got {secret}"
        );

        // And the connection is still there, naming that same secret.
        let (status, described) =
            events(&router, "DescribeConnection", &json!({ "Name": case.name })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeConnection answered {status} {described}"
        );
        assert_eq!(
            described["SecretArn"],
            json!(secret_arn),
            "case {label:?}: the connection keeps naming its own secret, got {described}"
        );
    }
}

/// An update that supplies new credentials stakes a new VERSION of the same secret:
/// the ARN is unchanged, the value read back is the new one, and the earlier version
/// is retained.
#[tokio::test]
async fn an_update_stakes_a_new_version_of_the_same_secret() {
    let router = emulator();
    for case in cases() {
        let label = case.kind;
        let secret_arn = create_connection(&router, &case).await;
        let (_, before) = secrets(
            &router,
            "DescribeSecret",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        let first = version_ids(&before, label);

        let (status, updated) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": case.name,
                "AuthorizationType": case.kind,
                "AuthParameters": case.update,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateConnection answered {status} {updated}"
        );

        let (status, described) =
            events(&router, "DescribeConnection", &json!({ "Name": case.name })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeConnection answered {status} {described}"
        );
        assert_eq!(
            described["SecretArn"],
            json!(secret_arn),
            "case {label:?}: an update keeps the connection's SecretArn, got {described}"
        );

        let (status, held) = secrets(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetSecretValue answered {status} {held}"
        );
        let document = stored_document(&held, label);
        assert_eq!(
            document, case.rotated,
            "case {label:?}: expected the rotated credential {}, got {document}",
            case.rotated
        );

        let (_, after) = secrets(
            &router,
            "DescribeSecret",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        let versions = version_ids(&after, label);
        assert_eq!(
            versions.len(),
            2,
            "case {label:?}: expected the rotation to add a version to {first:?}, got \
             {versions:?}"
        );
        for held_before in &first {
            assert!(
                versions.contains(held_before),
                "case {label:?}: expected the earlier version {held_before:?} to be \
                 retained, got {versions:?}"
            );
        }
    }
}

/// Deleting the connection removes its secret: the credential it held is no longer
/// readable, and the ARN the connection announced resolves to nothing.
#[tokio::test]
async fn deleting_the_connection_removes_its_secret() {
    let router = emulator();
    for case in cases() {
        let label = case.kind;
        let secret_arn = create_connection(&router, &case).await;

        let (status, deleted) =
            events(&router, "DeleteConnection", &json!({ "Name": case.name })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeleteConnection answered {status} {deleted}"
        );

        // The secret goes with the connection. Look again until it is gone rather
        // than waiting a fixed time for it.
        let deadline = std::time::Instant::now() + POLL_DEADLINE;
        let gone = loop {
            let (status, answered) = secrets(
                &router,
                "GetSecretValue",
                &json!({ "SecretId": secret_arn }),
            )
            .await;
            if status == 400 && error_code(&answered) == "ResourceNotFoundException" {
                break answered;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {label:?}: expected GetSecretValue to settle on \
                 ResourceNotFoundException, last answered {status} {answered}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            error_message(&gone),
            "Secrets Manager can't find the specified secret.",
            "case {label:?}: expected the missing-secret sentence, got {gone}"
        );
    }
}

/// Create `case`'s connection and answer the ARN of the secret it announces.
async fn create_connection(router: &MultiServiceRouter, case: &Case) -> String {
    let label = case.kind;
    let (status, created) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": case.name,
            "AuthorizationType": case.kind,
            "AuthParameters": case.create,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateConnection answered {status} {created}"
    );
    let (status, described) =
        events(router, "DescribeConnection", &json!({ "Name": case.name })).await;
    assert_eq!(
        status, 200,
        "case {label:?}: DescribeConnection answered {status} {described}"
    );
    let secret_arn = described["SecretArn"].as_str().unwrap_or_default().to_string();
    assert!(
        secret_arn.contains(":secretsmanager:"),
        "case {label:?}: expected the connection to announce its secret, got {described}"
    );
    secret_arn
}

/// The document a secret holds carries ONLY the credential members: a read of the
/// connection reports none of them, so the secret is the one place the value lives.
#[tokio::test]
async fn the_secret_holds_the_credential_and_the_connection_reports_none_of_it() {
    let router = emulator();
    for case in cases() {
        let label = case.kind;
        let secret_arn = create_connection(&router, &case).await;
        let (_, held) = secrets(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        let document = stored_document(&held, label);
        let expected: &Map<String, Value> = case
            .stored
            .as_object()
            .expect("the expected document is an object");
        let got: &Map<String, Value> = document
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {document}"));
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "case {label:?}: expected exactly the credential members, got {document}"
        );

        // The connection itself reports the secret's ARN and never the credential
        // value held in it.
        let (_, described) =
            events(&router, "DescribeConnection", &json!({ "Name": case.name })).await;
        let rendered = described.to_string();
        for value in &case.withheld {
            assert!(
                !rendered.contains(value),
                "case {label:?}: a read of the connection must report no credential, but \
                 {value:?} is in {rendered}"
            );
        }
    }
}
