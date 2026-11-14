// SPDX-License-Identifier: CC0-1.0

//! The IAM authorization of a WebSocket api's `@connections` endpoint, through the WHOLE
//! assembled router: who may push to, read and close the connections of a deployed stage.
//!
//! > **RULE: the three `@connections` operations are published on the owning api's own
//! > `execute-api` invoke endpoint, and every call made there is authorized against the
//! > calling principal's identity-based policies: a call is served only where a readable
//! > document allows `execute-api:ManageConnections` on the resource it addresses —
//! > `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/@connections/{connectionId}`,
//! > the route within the stage the endpoint resolved — and every other call is refused `403`
//! > with the error code `AccessDeniedException` and the sentence IAM words an authorization
//! > failure in, before the connection store is consulted at all.**
//!
//! Four crates have to agree for that to hold, which is why this is a router suite: API
//! Gateway V2 holds the api, the stage and the deployment that make the addressing resolve a
//! resource at all, IAM holds the role and the document in force on it, the token service
//! mints the session the request signs with, and the API Gateway Management API takes the
//! decision and words the refusal. Everything is driven the way a client reaches it —
//! `iam:CreateRole`, `iam:PutRolePolicy`, `sts:AssumeRole`, then the signed `@connections`
//! call — so the document under test is the one IAM stored and the principal a refusal blames
//! is the one the session was minted for.
//!
//! The rows span what the rule generalises over: the operation (all three, so all three
//! methods of the resource ARN), the document in force on the caller (none at all, the action
//! on every resource, the action on this very route, the action on another stage of the same
//! api, another action of the same service, an explicit deny beside a grant of everything),
//! and the principal (an assumed-role session and the account's own published credential,
//! which holds no IAM policies).
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every resource below is created and addressed in.
const REGION: &str = "us-west-2";

/// The stage the api is deployed to, and the connection the calls address.
const STAGE: &str = "test";
const CONNECTION_ID: &str = "L0SM9cOFyjUCIhA=";

/// The IAM action a `@connections` call asks for, and the code AWS words an authorization
/// failure in.
const MANAGE_CONNECTIONS: &str = "execute-api:ManageConnections";
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The access key id neither the token service nor IAM holds a record for: the account's own
/// published credential.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The awsQuery API versions the two setup services are called at.
const IAM_VERSION: &str = "2010-05-08";
const STS_VERSION: &str = "2011-06-15";

/// One answered request.
struct Answered {
    status: u16,
    headers: http::HeaderMap,
    text: String,
}

impl Answered {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The error SHAPE a restJson1 failure names, where a restJson1 client reads it.
    fn error_code(&self) -> String {
        self.headers
            .get("x-amzn-errortype")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
            .unwrap_or_else(|| "<absent>".to_string())
    }

    /// The `message` member of an error body.
    fn error_message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }
}

/// A SigV4 `Authorization` whose credential names `access_key_id` and whose scope names
/// `signing_name`, which is what routes the request. The signature itself is never checked.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(bytes::Bytes::from(body)).expect("a request"))
        .await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    Answered {
        status,
        headers,
        text: String::from_utf8_lossy(response.body().as_ref()).to_string(),
    }
}

/// One awsQuery call to `signing_name`'s own host, which must answer 200.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let authorization = authorization(ACCOUNT_KEY_ID, signing_name);
    let mut form = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        form.push_str(&format!("&{name}={value}"));
    }
    let answered = dispatch(
        router,
        &format!("{signing_name}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization),
        ],
        form.into_bytes(),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "setup: {signing_name} {action} must answer 200, got {} {}",
        answered.status, answered.text
    );
    answered.text
}

/// The text of one XML element of an awsQuery answer.
fn xml_text(what: &str, body: &str, element: &str) -> String {
    let open = format!("<{element}>");
    let close = format!("</{element}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("setup: {what} must carry {open} , got {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("setup: {what} must carry {close}, got {body}"))
        + start;
    body[start..end].to_string()
}

/// One API Gateway V2 control-plane call, which must answer `expected_status`.
async fn control_plane(
    router: &MultiServiceRouter,
    what: &str,
    path: &str,
    body: &Value,
    expected_status: u16,
) -> Value {
    let authorization = authorization(ACCOUNT_KEY_ID, "apigateway");
    let answered = dispatch(
        router,
        &format!("apigateway.{REGION}.amazonaws.com"),
        "POST",
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        answered.status, expected_status,
        "setup: {what} must answer HTTP {expected_status}, got {} {}",
        answered.status, answered.text
    );
    answered.json()
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup: {what} must carry a non-empty {member:?}, got {body}"))
}

/// An identity-based policy document stating `effect` about `action` on `resource`.
///
/// Compact and free of the characters an urlencoded body treats specially (`&`, `=`, `+`,
/// `%`, space), so it needs no encoding.
fn policy_document(effect: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// The trust policy of a role the account's own credential assumes.
fn account_trust_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}",
        account = engine::emulator_account_id()
    )
}

/// A role carrying `policy` as its one inline document, and a session of it: the access key id
/// that session was minted under, and the principal ARN it speaks for.
///
/// Driven the way a client reaches it — `iam:CreateRole`, `iam:PutRolePolicy`,
/// `sts:AssumeRole` — so the document under test is the one IAM stored.
async fn assumed_session(
    router: &MultiServiceRouter,
    role_name: &str,
    session_name: &str,
    policy: Option<String>,
) -> (String, String) {
    let created = query(
        router,
        "iam",
        IAM_VERSION,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &account_trust_policy()),
        ],
    )
    .await;
    let role_arn = xml_text("iam CreateRole", &created, "Arn");
    if let Some(document) = policy {
        query(
            router,
            "iam",
            IAM_VERSION,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", "connections"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    }
    let assumed = query(
        router,
        "sts",
        STS_VERSION,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", session_name)],
    )
    .await;
    (
        xml_text("sts AssumeRole", &assumed, "AccessKeyId"),
        xml_text("sts AssumeRole", &assumed, "Arn"),
    )
}

/// The three operations, by the method each is reached with, the payload each sends, and the
/// status each answers a call it serves.
const OPERATIONS: [(&str, &[u8], u16); 3] =
    [("GET", b"", 200), ("POST", b"push", 200), ("DELETE", b"", 204)];

/// One row: the document in force on the session, and whether the call is served.
struct Row {
    case: &'static str,
    /// The role name and session name the row's own session is minted under, so each row's
    /// principal is its own.
    name: &'static str,
    /// The document, built from the api id and the METHOD the row's call is reached by.
    document: fn(&str, &str) -> Option<String>,
    served: bool,
    /// The clause the refusal closes with, when the row is refused.
    reason: Option<&'static str>,
}

/// The clause closing a refusal no readable document allowed, and the one closing a refusal a
/// readable document DENIED.
fn no_grant() -> String {
    format!("because no identity-based policy allows the {MANAGE_CONNECTIONS} action")
}
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The resource ARN a call reached by `method` on `api_id` addresses.
fn resource(api_id: &str, method: &str) -> String {
    format!(
        "arn:aws:execute-api:{REGION}:{account}:{api_id}/{STAGE}/{method}/@connections/\
         {{connectionId}}",
        account = engine::emulator_account_id()
    )
}

const ROWS: &[Row] = &[
    Row {
        case: "no document at all",
        name: "agwmgmt-none",
        document: |_api, _method| None,
        served: false,
        reason: None,
    },
    Row {
        case: "the action on every resource",
        name: "agwmgmt-every",
        document: |_api, _method| Some(policy_document("Allow", MANAGE_CONNECTIONS, "*")),
        served: true,
        reason: None,
    },
    Row {
        case: "the action on this very route",
        name: "agwmgmt-route",
        document: |api, method| {
            Some(policy_document(
                "Allow",
                MANAGE_CONNECTIONS,
                &resource(api, method),
            ))
        },
        served: true,
        reason: None,
    },
    Row {
        case: "the action on another stage of the same api",
        name: "agwmgmt-other-stage",
        document: |api, method| {
            Some(policy_document(
                "Allow",
                MANAGE_CONNECTIONS,
                &format!(
                    "arn:aws:execute-api:{REGION}:{account}:{api}/other/{method}/@connections/*",
                    account = engine::emulator_account_id()
                ),
            ))
        },
        served: false,
        reason: None,
    },
    Row {
        case: "another action of the same service on every resource",
        name: "agwmgmt-other-action",
        document: |_api, _method| Some(policy_document("Allow", "execute-api:Invoke", "*")),
        served: false,
        reason: None,
    },
    Row {
        case: "an explicit deny beside a grant of everything",
        name: "agwmgmt-denied",
        document: |_api, _method| {
            Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Action\":\"{MANAGE_CONNECTIONS}\",\"Resource\":\"*\"}},\
                 {{\"Effect\":\"Deny\",\"Action\":\"{MANAGE_CONNECTIONS}\",\"Resource\":\"*\"}}]}}"
            ))
        },
        served: false,
        reason: Some(EXPLICIT_DENY),
    },
];

/// A WEBSOCKET api with one deployed stage, as the control plane writes it. Answers its id
/// and the `host:port` of the socket its `ApiEndpoint` announces.
async fn deployed_websocket_api(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = control_plane(
        router,
        "CreateApi",
        "/v2/apis",
        &json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
        201,
    )
    .await;
    let api_id = as_str("CreateApi", &created, "apiId");
    // The address the api's own data-plane socket landed on, which is what its `ApiEndpoint`
    // announces and what a backend's `@connections` client is configured with.
    let announced = as_str("CreateApi", &created, "apiEndpoint");
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| panic!("setup: expected a wss:// endpoint, got {announced:?}"))
        .to_string();
    let integration = control_plane(
        router,
        "CreateIntegration",
        &format!("/v2/apis/{api_id}/integrations"),
        &json!({ "integrationType": "MOCK" }),
        201,
    )
    .await;
    let integration_id = as_str("CreateIntegration", &integration, "integrationId");
    control_plane(
        router,
        "CreateRoute",
        &format!("/v2/apis/{api_id}/routes"),
        &json!({
            "routeKey": "$connect",
            "target": format!("integrations/{integration_id}")
        }),
        201,
    )
    .await;
    let deployment = control_plane(
        router,
        "CreateDeployment",
        &format!("/v2/apis/{api_id}/deployments"),
        &json!({}),
        201,
    )
    .await;
    let deployment_id = as_str("CreateDeployment", &deployment, "deploymentId");
    control_plane(
        router,
        "CreateStage",
        &format!("/v2/apis/{api_id}/stages"),
        &json!({ "stageName": STAGE, "deploymentId": deployment_id }),
        201,
    )
    .await;
    (api_id, authority)
}

/// One `@connections` call at `host`, signed by `access_key_id`.
async fn connections_call(
    router: &MultiServiceRouter,
    host: &str,
    access_key_id: &str,
    method: &str,
    payload: &[u8],
) -> Answered {
    dispatch(
        router,
        host,
        method,
        &format!("/{STAGE}/@connections/{CONNECTION_ID}"),
        &[("authorization", &authorization(access_key_id, "execute-api"))],
        payload.to_vec(),
    )
    .await
}

/// The `execute-api` host AWS publishes an api's endpoint on.
fn execute_api_host(api_id: &str) -> String {
    format!("{api_id}.execute-api.{REGION}.amazonaws.com")
}

/// Register the connection every call below addresses, through the seams that make a
/// connection exist and end: whatever the previous row left is closed first, so each row
/// addresses a connection of its own and the delivery counters below are that row's.
fn register(engine: &Arc<InMemoryEngine>) {
    let registry = mgmt::state::registry(&**engine).expect("setup: the registry");
    mgmt::state::close_connection(&registry, CONNECTION_ID);
    assert!(
        mgmt::state::register_connection(
            &**engine,
            CONNECTION_ID,
            mgmt::state::Identity {
                source_ip: "192.0.2.30".to_string(),
                user_agent: "pusher/1".to_string(),
            },
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        ),
        "setup: the registration seam must register the connection"
    );
}

#[test]
fn a_connections_call_is_served_only_where_iam_allows_manage_connections() {
    std::thread::Builder::new()
        .name("agwmgmt-manage-connections-authorization".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            // `enable_io`: a WEBSOCKET api's data plane is a SOCKET the core opens when the
            // api is created, so the runtime this check creates one on has to bind one.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(Arc::clone(&engine));
                // Signature verification belongs to the emulator, not to this check: what is
                // under test is the AUTHORIZATION of a credential, not its signature.
                router.set_request_authentication(false);

                let (api_id, _socket) =
                    deployed_websocket_api(&router, "connections-authorization").await;

                for (method, payload, served_status) in OPERATIONS {
                    for row in ROWS {
                        register(&engine);
                        let name = format!("{}-{}", row.name, method.to_lowercase());
                        let (access_key_id, principal) = assumed_session(
                            &router,
                            &name,
                            &name,
                            (row.document)(&api_id, method),
                        )
                        .await;

                        let case = format!("{method} with {}", row.case);
                        let answered = connections_call(
                            &router,
                            &execute_api_host(&api_id),
                            &access_key_id,
                            method,
                            payload,
                        )
                        .await;
                        if row.served {
                            assert_eq!(
                                answered.status, served_status,
                                "case {case:?}: expected the operation's own {served_status}, \
                                 got {} {}",
                                answered.status, answered.text
                            );
                            continue;
                        }
                        assert_eq!(
                            answered.status, 403,
                            "case {case:?}: expected HTTP 403, got {} {}",
                            answered.status, answered.text
                        );
                        assert_eq!(
                            answered.error_code(),
                            ACCESS_DENIED,
                            "case {case:?}: expected {ACCESS_DENIED} in x-amzn-errortype, got {}",
                            answered.error_code()
                        );
                        let reason = row.reason.map_or_else(no_grant, str::to_string);
                        let expected = format!(
                            "User: {principal} is not authorized to perform: \
                             {MANAGE_CONNECTIONS} on resource: {} {reason}",
                            resource(&api_id, method)
                        );
                        assert_eq!(
                            answered.error_message(),
                            expected,
                            "case {case:?}: expected {expected:?}, got {}",
                            answered.text
                        );

                        // Nothing about the connection changed: the decision precedes the
                        // connection store.
                        let stored = mgmt::state::connection(&*engine, CONNECTION_ID)
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: the connection must still exist")
                            });
                        assert_eq!(
                            stored.delivered_count, 0,
                            "case {case:?}: a refused call delivers nothing, got {} deliveries",
                            stored.delivered_count
                        );
                    }

                    // The account's own published credential holds no IAM policies, so it is
                    // not bounded by this gate: the positive control that the stage itself
                    // serves the very call every refused row made.
                    register(&engine);
                    let answered =
                        connections_call(&router, &execute_api_host(&api_id), ACCOUNT_KEY_ID, method, payload)
                            .await;
                    assert_eq!(
                        answered.status, served_status,
                        "case {method:?} signed with the account's own credential: expected \
                         {served_status}, got {} {}",
                        answered.status, answered.text
                    );
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}

/// The same decision when the call is addressed at the api's OWN SOCKET — the address its
/// `ApiEndpoint` announces and the one a backend's `@connections` client is configured with.
///
/// That address carries no api id of its own, so the api the resource ARN names is the one
/// THAT SOCKET serves: with two apis each holding a stage of the same name, a call at one
/// api's endpoint is authorized on that api's stage and never on the other's.
#[test]
fn a_call_at_the_apis_own_socket_is_authorized_on_that_apis_stage() {
    std::thread::Builder::new()
        .name("agwmgmt-manage-connections-socket".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(Arc::clone(&engine));
                router.set_request_authentication(false);

                // Two apis, each with a stage of the SAME name: the addressing is what tells
                // them apart.
                let (first_id, _first_socket) =
                    deployed_websocket_api(&router, "connections-socket-first").await;
                let (second_id, second_socket) =
                    deployed_websocket_api(&router, "connections-socket-second").await;
                assert_ne!(
                    first_id, second_id,
                    "setup: the two apis must be distinct, got {first_id}"
                );

                // The rows differ along which api's stage the caller was granted the action
                // on: the one it addresses, and the other one holding a stage of the same
                // name.
                let rows: &[(&str, Option<String>, bool)] = &[
                    ("no document at all", None, false),
                    (
                        "the addressed api's own stage",
                        Some(policy_document(
                            "Allow",
                            MANAGE_CONNECTIONS,
                            &resource(&second_id, "POST"),
                        )),
                        true,
                    ),
                    (
                        "the OTHER api's stage of the same name",
                        Some(policy_document(
                            "Allow",
                            MANAGE_CONNECTIONS,
                            &resource(&first_id, "POST"),
                        )),
                        false,
                    ),
                ];
                for (index, (case, document, admitted)) in rows.iter().enumerate() {
                    let name = format!("agwmgmt-socket-{index}");
                    let (access_key_id, principal) =
                        assumed_session(&router, &name, &name, document.clone()).await;
                    let answered = connections_call(
                        &router,
                        &second_socket,
                        &access_key_id,
                        "POST",
                        b"push",
                    )
                    .await;
                    if *admitted {
                        // Past the gate the call reaches the connection store, which holds no
                        // connection of that id established at this endpoint: the modelled
                        // `GoneException`. What is asserted here is that the gate admitted it.
                        assert_eq!(
                            answered.status, 410,
                            "case {case:?}: an admitted call reaches the connection store, so \
                             expected the modelled 410, got {} {}",
                            answered.status, answered.text
                        );
                        continue;
                    }
                    assert_eq!(
                        answered.status, 403,
                        "case {case:?}: expected HTTP 403, got {} {}",
                        answered.status, answered.text
                    );
                    let expected = format!(
                        "User: {principal} is not authorized to perform: {MANAGE_CONNECTIONS} \
                         on resource: {} {}",
                        resource(&second_id, "POST"),
                        no_grant()
                    );
                    assert_eq!(
                        answered.error_message(),
                        expected,
                        "case {case:?}: expected {expected:?}, got {}",
                        answered.text
                    );
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
