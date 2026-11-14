// SPDX-License-Identifier: CC0-1.0

//! What the three `@connections` operations answer a caller IAM does not allow to manage the
//! connections of the stage it addressed — driven over HTTP through the same
//! `MultiServiceRouter` the production emulator serves with.
//!
//! > **RULE: these operations are published on the owning api's `execute-api` invoke
//! > endpoint, and every call made there is authorized by IAM against the calling
//! > principal's own identity-based policies: a call is served only where a readable document
//! > allows `execute-api:ManageConnections` on the resource it addresses —
//! > `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/@connections/{connectionId}`,
//! > the route within the stage — and every other call is refused `403` with the error code
//! > `AccessDeniedException` and the sentence IAM words an authorization failure in, taken
//! > BEFORE the `ConnectionId` is judged and before the connection store is consulted.**
//!
//! WHO the api and the stage are comes from the service that owns them, through the shared
//! cell this service asks (`mgmt::state::connections_endpoint_authority`) — the same cell
//! that judges whether the addressing serves anything at all. These cases install a
//! resolution of their own, exactly as `connections_error_layer` installs a verdict of its
//! own: what this suite owns is the DECISION and its envelope, while the resolution against
//! real apis, stages and deployments is API Gateway V2's and is driven end to end in
//! `core/router/tests/all/apigatewayv2_websocket_deploy_then_connections.rs`.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The rows span what the rule generalises over: the operation (all three, so all three
//! methods of the resource ARN), the document in force on the caller (none at all, the action
//! on every resource, on this very route, on the stage's connections by wildcard, on another
//! stage of the same api, another action of the same service, and an explicit deny beside a
//! grant of everything), the PRINCIPAL the credential speaks for (an assumed-role session and
//! the account's own published credential), and the `ConnectionId` the refused call carries
//! (a live one, a well-formed one nothing holds, and one that could never have been minted).

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_common::execute_api_connections_endpoint::AddressedStage;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call_signed_by, error_code, error_message, rendered, HOST, MINTED_IDS};

/// The api, Region and stage the installed resolution answers for — what a real resolution
/// reads out of the owner's own records.
const API_ID: &str = "k1l2m3n4o5";
const REGION: &str = "us-east-1";
const STAGE: &str = "dev";

/// The invoke path a caller of that stage addresses, which is what names it.
const STAGE_PREFIX: &str = "/dev";

/// The role the seeded session is a session of, and the session's own name.
const ROLE: &str = "connections-pusher";
const SESSION: &str = "pushing";

/// The access key id that session was minted under: an `ASIA` key, the prefix a temporary
/// credential carries.
const SESSION_KEY_ID: &str = "ASIAEXAMPLECONNECT01";

/// An access key id neither store holds a record for: the position the account's own
/// published credential is in, which holds no IAM policies at all.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The name of the inline policy a row attaches to the role.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The action a `@connections` call asks for.
const MANAGE_CONNECTIONS: &str = "execute-api:ManageConnections";

/// The error code AWS words an authorization failure in.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The three operations, by the method each is reached with, the payload each sends, and the
/// status each answers a call it serves.
const OPERATIONS: [(&str, &[u8], u16); 3] =
    [("GET", b"", 200), ("POST", b"push", 200), ("DELETE", b"", 204)];

/// A registration instant in the past.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// The resolution standing in for the owner of apis and stages: one deployed stage, whose api
/// and Region are the ones every expected ARN below names.
fn the_deployed_stage(
    _engine: &dyn Any,
    _host: Option<&str>,
    prefix: &str,
) -> Option<AddressedStage> {
    (prefix == STAGE_PREFIX).then(|| AddressedStage {
        api_id: API_ID.to_string(),
        region: REGION.to_string(),
        stage_name: STAGE.to_string(),
    })
}

/// An engine whose shared cell carries [`the_deployed_stage`] and whose IAM and STS stores
/// hold the role and the session [`SESSION_KEY_ID`] speaks for.
fn engine_with_a_session() -> Arc<InMemoryEngine> {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    mgmt::state::connections_endpoint_authority(&*engine)
        .expect("the shared cell lives in this engine")
        .lock()
        .install_addressed_stage(the_deployed_stage);
    given_session(&engine);
    engine
}

/// The ARN the account holds the seeded role under.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", engine::emulator_account_id())
}

/// The principal the seeded session speaks for, as `sts:GetCallerIdentity` reports it — what
/// a refusal of a request signed with it must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, exactly as
/// `iam:CreateRole` and a vend of that role record them.
fn given_session(engine: &Arc<InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store lives in this engine");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: sts::role_id(&role_arn),
            arn: role_arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
            // The role's TRUST policy, which is what a vend of its credentials is authorized
            // by; nothing here is an identity-based grant.
            assume_role_policy_document: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }]
                })
                .to_string(),
            ),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let identity = sts::Identity {
        user_id: format!("{}:{SESSION}", sts::role_id(&role_arn)),
        account: engine::emulator_account_id().to_string(),
        arn: session_arn(),
    };
    // The vend as STS asked for it, through the registry's own constructor, so what is
    // recorded is exactly what a mint records. No session policy scopes it: what the
    // credential may do is the role's own policies, which is what each row puts on it.
    let vend = sts::NewSession {
        role_arn: Some(role_arn),
        ..sts::NewSession::new(identity, sts::SessionKind::AssumedRole, SESSION_SECONDS)
    };
    let sessions = sts::service_state(&**engine).expect("the STS store lives in this engine");
    sessions
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the vended session is recorded");
}

/// Attach `document` to the role as its one inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it.
fn given_policy(engine: &Arc<InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store lives in this engine");
    let mut store = store.lock();
    let key = iam::name_key(ROLE);
    match document {
        None => {
            store.role_inline_policies.remove(&key);
        }
        Some(document) => {
            store.role_inline_policies.insert(
                key,
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// An identity-based policy with one statement.
fn statement(effect: &str, action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }]
    })
}

/// The resource ARN a call reached by `method` addresses: the route within the stage, whose
/// connection id is the route's own parameter.
fn resource(method: &str) -> String {
    format!(
        "arn:aws:execute-api:{REGION}:{}:{API_ID}/{STAGE}/{method}/@connections/{{connectionId}}",
        engine::emulator_account_id()
    )
}

/// The sentence a refused call must carry: the principal, the action, the resource and the
/// clause IAM closes an authorization failure with.
fn refusal_sentence(method: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {MANAGE_CONNECTIONS} on resource: \
         {resource} {reason}",
        principal = session_arn(),
        resource = resource(method),
    )
}

/// The clause closing a refusal no readable document allowed.
fn no_grant() -> String {
    format!("because no identity-based policy allows the {MANAGE_CONNECTIONS} action")
}

/// The clause closing a refusal a readable document DENIED — a fact about that document, so
/// the sentence names it rather than the action.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The request path for `connection_id` on the deployed stage.
fn path(connection_id: &str) -> String {
    format!("{STAGE_PREFIX}/@connections/{connection_id}")
}

/// One row of the table: the document in force on the caller, and whether the call is served.
struct Row {
    case: &'static str,
    /// The document, built per row from the METHOD the row's call is reached by (so a grant
    /// scoped to this very route is the route of that call).
    document: fn(&str) -> Option<Value>,
    served: bool,
    /// The clause the refusal of a row this document does not admit closes with: the default
    /// deny of a call nothing allowed, or the explicit deny of a document that refused it.
    reason: Option<&'static str>,
}

/// The documents span the readings of `Action` and `Resource` a grant can take, plus the
/// absence of any document and an explicit deny beside a grant of everything.
const ROWS: &[Row] = &[
    Row {
        case: "no document at all",
        document: |_method| None,
        served: false,
        reason: None,
    },
    Row {
        case: "the action on every resource",
        document: |_method| Some(statement("Allow", MANAGE_CONNECTIONS, "*")),
        served: true,
        reason: None,
    },
    Row {
        case: "the action on this very route",
        document: |method| Some(statement("Allow", MANAGE_CONNECTIONS, &resource(method))),
        served: true,
        reason: None,
    },
    Row {
        case: "the stage's connections by wildcard",
        document: |method| {
            Some(statement(
                "Allow",
                MANAGE_CONNECTIONS,
                &format!(
                    "arn:aws:execute-api:{REGION}:{}:{API_ID}/{STAGE}/{method}/@connections/*",
                    engine::emulator_account_id()
                ),
            ))
        },
        served: true,
        reason: None,
    },
    Row {
        case: "the action on another stage of the same api",
        document: |method| {
            Some(statement(
                "Allow",
                MANAGE_CONNECTIONS,
                &format!(
                    "arn:aws:execute-api:{REGION}:{}:{API_ID}/other/{method}/@connections/*",
                    engine::emulator_account_id()
                ),
            ))
        },
        served: false,
        reason: None,
    },
    Row {
        case: "another action of the same service on every resource",
        document: |_method| Some(statement("Allow", "execute-api:Invoke", "*")),
        served: false,
        reason: None,
    },
    Row {
        case: "an explicit deny beside a grant of everything",
        document: |_method| {
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": MANAGE_CONNECTIONS, "Resource": "*" },
                    { "Effect": "Deny", "Action": MANAGE_CONNECTIONS, "Resource": "*" }
                ]
            }))
        },
        served: false,
        reason: Some(EXPLICIT_DENY),
    },
];

/// Each operation is served only where the caller's own policies allow the action on the
/// route it addresses, and refused `403 AccessDeniedException` with IAM's sentence otherwise.
#[tokio::test]
async fn each_operation_is_served_only_where_the_caller_may_manage_connections() {
    for (method, payload, served_status) in OPERATIONS {
        for row in ROWS {
            let engine = engine_with_a_session();
            let document = (row.document)(method);
            given_policy(&engine, document.as_ref());
            let live = MINTED_IDS[0].1;
            crate::support::register(&engine, live, "192.0.2.20", "pusher/1", connected_at());

            let case = format!("{method} with {}", row.case);
            let (status, headers, body) = call_signed_by(
                &engine,
                SESSION_KEY_ID,
                HOST,
                method,
                &path(live),
                payload.to_vec(),
            )
            .await;
            if row.served {
                assert_eq!(
                    status, served_status,
                    "case {case:?}: expected the operation's own {served_status}, got {status} {}",
                    rendered(&body)
                );
                continue;
            }
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED} in x-amzn-errortype, got {}",
                error_code(&headers)
            );
            let expected =
                refusal_sentence(method, row.reason.map_or_else(no_grant, str::to_string).as_str());
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {}",
                rendered(&body)
            );
        }
    }
}

/// The decision precedes the connection store: a refused call touches no connection, whatever
/// `ConnectionId` it carried — a live one, a well-formed one nothing holds, or one that could
/// never have been minted (which an allowed caller is refused `400` for).
#[tokio::test]
async fn an_unauthorized_call_is_refused_before_the_connection_is_read() {
    let malformed = crate::support::ids_api_gateway_does_not_mint();
    let (_, malformed_id) = malformed[0].clone();
    // The ids span the three classes a request can carry.
    let ids = [
        ("a live connection", MINTED_IDS[0].1.to_string()),
        ("a well-formed id nothing holds", MINTED_IDS[1].1.to_string()),
        ("an id that could never have been minted", malformed_id),
    ];
    for (method, payload, _) in OPERATIONS {
        for (id_case, connection_id) in &ids {
            let engine = engine_with_a_session();
            // No document at all: the caller holds no grant.
            given_policy(&engine, None);
            let live = MINTED_IDS[0].1;
            crate::support::register(&engine, live, "192.0.2.21", "pusher/2", connected_at());

            let case = format!("{method} on {id_case}");
            let (status, headers, body) = call_signed_by(
                &engine,
                SESSION_KEY_ID,
                HOST,
                method,
                &path(connection_id),
                payload.to_vec(),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: the authorization decision precedes the ConnectionId, so \
                 expected HTTP 403, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {}",
                error_code(&headers)
            );

            // Nothing about the connection changed, which is what "before the store is
            // consulted" means on the wire.
            let stored = mgmt::state::connection(&*engine, live)
                .unwrap_or_else(|| panic!("case {case:?}: the connection must still exist"));
            assert_eq!(
                stored.delivered_count, 0,
                "case {case:?}: a refused call delivers nothing, got {} deliveries",
                stored.delivered_count
            );
            assert_eq!(
                stored.last_active_at, stored.connected_at,
                "case {case:?}: a refused call must not move LastActiveAt, got {:?}",
                stored.last_active_at
            );
        }
    }
}

/// The account's own published credential holds no IAM policies, so it is not bounded by this
/// gate — and neither is a caller of an addressing nothing resolves a stage for.
#[tokio::test]
async fn the_callers_this_gate_cannot_judge_are_served() {
    for (method, payload, served_status) in OPERATIONS {
        // The account's own credential, on the resolved stage.
        let engine = engine_with_a_session();
        given_policy(&engine, None);
        let live = MINTED_IDS[0].1;
        crate::support::register(&engine, live, "192.0.2.22", "pusher/3", connected_at());
        let case = format!("{method} signed with the account's own credential");
        let (status, _, body) = call_signed_by(
            &engine,
            ACCOUNT_KEY_ID,
            HOST,
            method,
            &path(live),
            payload.to_vec(),
        )
        .await;
        assert_eq!(
            status, served_status,
            "case {case:?}: expected {served_status}, got {status} {}",
            rendered(&body)
        );

        // The session's own credential, where no resolution is installed at all — an emulator
        // the owner of apis and stages is not mounted in resolves no resource, so there is
        // nothing a statement could be about.
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        given_session(&engine);
        given_policy(&engine, None);
        crate::support::register(&engine, live, "192.0.2.23", "pusher/4", connected_at());
        let case = format!("{method} where nothing resolves the stage");
        let (status, _, body) = call_signed_by(
            &engine,
            SESSION_KEY_ID,
            HOST,
            method,
            &path(live),
            payload.to_vec(),
        )
        .await;
        assert_eq!(
            status, served_status,
            "case {case:?}: expected {served_status}, got {status} {}",
            rendered(&body)
        );
    }
}
