// SPDX-License-Identifier: CC0-1.0

//! What a GraphQL API whose authorization mode is `AMAZON_COGNITO_USER_POOLS` serves, driven
//! over its published socket with tokens a user pool of this emulator really minted.
//!
//! The rule these rows assert, one level above any single API:
//!
//! > A call on a GraphQL API configured for `AMAZON_COGNITO_USER_POOLS` is decided by the
//! > TOKEN it presents in its `authorization` field: the call is served only when that value
//! > is a compact JWS whose `iss` names a user pool the API configured, whose signature
//! > checks out against the key set that pool publishes, which is inside its own validity
//! > window, and whose app client matches the configuration's `appIdClientRegex` where it
//! > declares one ("if this value isn't set, no filtering is applied"). Anything else
//! > authorizes nobody and is refused `401` `UnauthorizedException`, reaching no resolver.
//! > What an admitted token CLAIMS is who the call speaks for: `$context.identity` reports
//! > its subject, its user name and the groups it claims, and a field the schema guards with
//! > `@aws_auth(cognito_groups: […])` — or `@aws_cognito_user_pools(cognito_groups: […])`,
//! > the same guard — resolves for a caller in one of those groups and is reported as a
//! > field the caller may not access for every other, with no resolver of it run — on a field
//! > of the operation's root type and on a field of any type the selection reaches. The same
//! > guard decides a SUBSCRIPTION field: a `start` on one is acked for a caller in its groups
//! > and refused for every other.
//!
//! The rows span what that rule generalises over: the token KIND (an identity token, an
//! access token), how it is presented (bare, with the RFC 6750 bearer scheme, absent), what
//! makes a value no token the API accepts (a token of another pool, a rewritten claim set, a
//! rewritten signature, a value that is no token at all, a token whose window has elapsed, a
//! token of an app client the configuration filters out), how many groups a guard names (one,
//! two) and where the caller's group sits in that list (first, last, absent), which spelling
//! of the guard the schema writes, and which endpoint the call is made on (the GraphQL one,
//! and the real-time one for a subscription).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake: every
//! token is one `AdminInitiateAuth` minted for a user of a pool this suite created — so
//! Amazon Cognito user pools is mounted beside AppSync on one engine — and the one token no
//! sign-in produces, the one whose window has elapsed, is signed with that same pool's own
//! signing key.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{exchange, handshake, started_message};

/// The Region every call of this suite is signed for, and the Region the pools are created
/// in.
const REGION: &str = "us-west-2";

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The refusal every unauthorized call on this data plane earns. The service's own copies are
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE` and `…_MESSAGE`.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str = "You are not authorized to make this call.";

/// What a caller presenting a value this mode cannot READ as a token is told instead; the
/// service's own copy is `service::graphql_api::user_pool::UNREADABLE_TOKEN_MESSAGE`.
const UNREADABLE_TOKEN_MESSAGE: &str = "Unable to parse JWT token.";

/// The error AppSync reports for a field the caller may not access; the service's own copies
/// are `service::graphql_api::execution::UNAUTHORIZED_FIELD_ERROR_TYPE` and
/// `UNAUTHORIZED_FIELD`.
const UNAUTHORIZED_FIELD_ERROR_TYPE: &str = "Unauthorized";

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s
/// own values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// A password satisfying a default pool's password policy. Written for this test and used
/// nowhere else: the pool it is set in lives only in this test's own in-memory engine.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The group the guarded fields of [`SCHEMA`] name, and a group no user of this suite is in.
const ADMINS: &str = "Admins";
const EDITORS: &str = "Editors";

/// The values the guarded and unguarded fields resolve to.
const OPEN_VALUE: &str = "open-value";
const ADMIN_VALUE: &str = "admin-value";
const EITHER_VALUE: &str = "either-value";
const COGNITO_VALUE: &str = "cognito-value";
const SECRET_VALUE: &str = "secret-value";

/// The schema every API of this suite is created with: one field under no guard, three under
/// a group guard — one naming one group, one naming two with the caller's group LAST, and one
/// written with the multi-mode spelling of the same guard — one field that reports the
/// identity the call resolved to, one field resolving to a type whose OWN field carries a
/// guard, and two SUBSCRIPTION fields, one of them under the same guard, because a guard
/// decides a subscription as it decides a query.
const SCHEMA: &str = "type Note {\n  \
                      id: ID!\n  \
                      secret: String @aws_auth(cognito_groups: [\"Admins\"])\n}\n\
     \n\
     type Query {\n  \
                      open: String\n  \
                      adminOnly: String @aws_auth(cognito_groups: [\"Admins\"])\n  \
                      editorsOrAdmins: String @aws_auth(cognito_groups: [\"Editors\", \"Admins\"])\n  \
                      cognitoGuarded: String @aws_cognito_user_pools(cognito_groups: [\"Admins\"])\n  \
                      whoami: String\n  \
                      note: Note\n}\n\
     \n\
     type Subscription {\n  \
     onOpen: String\n  \
     onAdminOnly: String @aws_auth(cognito_groups: [\"Admins\"])\n}\n";

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// A router with AppSync, CloudWatch and Amazon Cognito user pools mounted over ONE engine:
/// the pool whose tokens the API accepts is the one this suite created through that service's
/// own operations.
///
/// Shared with the sibling suite that drives an `OPENID_CONNECT` API over tokens of such a pool
/// ([`crate::graphql_api_open_id_connect_authorization`]), together with the pool, user and
/// sign-in helpers below, so the tree holds ONE reading of how a provider of this emulator and
/// a token of it are made.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One Amazon Cognito user pools call that must succeed (`awsJson1_1`, `POST /` with
/// `X-Amz-Target`).
///
/// Shared with the suite that drives a subscription's group guard over the real-time endpoint
/// ([`crate::graphql_api_realtime_group_guard_deferral`]), which puts a user in a group the
/// pool already holds, so the tree holds ONE reading of how a user-pool call is made.
pub(crate) async fn cognito(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-idp.{REGION}.amazonaws.com/"))
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/cognito-idp/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "setup: cognito-idp {operation} must answer 200, got {status} {parsed}"
    );
    parsed
}

/// One user pool with one app client that admits the administrative password flow, as
/// (`UserPoolId`, `ClientId`).
pub(crate) async fn pool_with_client(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let pool = cognito(router, "CreateUserPool", json!({ "PoolName": name })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an Id, got {pool}"))
        .to_string();
    let client_id = app_client(router, &user_pool_id, &format!("{name}-client")).await;
    (user_pool_id, client_id)
}

/// One more app client of `user_pool_id`, as `ClientId`.
async fn app_client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let client = cognito(
        router,
        "CreateUserPoolClient",
        json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a ClientId, got {client}"))
        .to_string()
}

/// Create `username` in `user_pool_id` with a permanent password, and put it in every group
/// of `groups` — creating each group first, as a pool holds no group it was not asked for.
pub(crate) async fn user_in_groups(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    groups: &[&str],
) {
    cognito(
        router,
        "AdminCreateUser",
        json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    for group in groups {
        cognito(
            router,
            "CreateGroup",
            json!({ "UserPoolId": user_pool_id, "GroupName": group }),
        )
        .await;
        cognito(
            router,
            "AdminAddUserToGroup",
            json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "GroupName": group,
            }),
        )
        .await;
    }
}

/// Sign `username` in and answer its `AuthenticationResult` — the tokens the pool minted.
pub(crate) async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    signed_in["AuthenticationResult"].clone()
}

/// The identity token of a sign-in.
pub(crate) fn id_token(result: &Value) -> String {
    result["IdToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in must mint an IdToken, got {result}"))
        .to_string()
}

/// The access token of a sign-in.
fn access_token(result: &Value) -> String {
    result["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in must mint an AccessToken, got {result}"))
        .to_string()
}

/// The claim set of a compact JWS, read with the tree's one base64url codec.
pub(crate) fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {token}"
    );
    let claims = service_common::jwt::decode_base64url(segments[1])
        .unwrap_or_else(|| panic!("a minted token's claim set must be base64url: {token}"));
    serde_json::from_slice(&claims)
        .unwrap_or_else(|error| panic!("a minted token's claim set must be JSON ({error})"))
}

/// `token` with its CLAIM SET rewritten: the same header and the same signature over claims
/// nobody signed, which is what a holder of a token can do to one it holds.
pub(crate) fn with_rewritten_claims(token: &str, rewrite: impl Fn(&mut Value)) -> String {
    let segments: Vec<&str> = token.split('.').collect();
    let mut claims = claims_of(token);
    rewrite(&mut claims);
    format!(
        "{}.{}.{}",
        segments[0],
        service_common::jwt::encode_base64url(claims.to_string().as_bytes()),
        segments[2]
    )
}

/// `token` with its SIGNATURE rewritten: the same header and claim set under a signature the
/// issuer did not produce.
fn with_rewritten_signature(token: &str) -> String {
    let segments: Vec<&str> = token.split('.').collect();
    let flipped: String = segments[2]
        .chars()
        .map(|character| match character {
            'a' => 'b',
            _ => 'a',
        })
        .collect();
    format!("{}.{}.{}", segments[0], segments[1], flipped)
}

/// A token `user_pool_id`'s own identity-token key really signed over `claims`.
///
/// The one token no sign-in of this suite can mint: the pool issues none whose validity
/// window has already elapsed ("the duration can range from five minutes to one day"), so the
/// row that presents one signs it with the pool's own key rather than asserting on a value no
/// pool would have signed.
pub(crate) fn pool_signed_token(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    claims: &Value,
) -> String {
    let pools = cognito_idp::state::service_state(&**engine)
        .unwrap_or_else(|| panic!("the user pools store must be on this engine"));
    let store = pools.lock();
    let pool = store
        .pool(user_pool_id)
        .unwrap_or_else(|| panic!("the pool {user_pool_id} must exist"));
    pool.signing_keys()
        .for_use(cognito_idp::state::SigningKeyUse::IdToken)
        .signed_compact_jws(claims)
        .unwrap_or_else(|| panic!("the pool must sign the token"))
}

/// Seconds since the epoch, `ago` before now — what an elapsed `exp` claim states.
pub(crate) fn seconds_ago(ago: Duration) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(now.saturating_sub(ago).as_secs()).unwrap_or_default()
}

/// One created GraphQL API: the addresses its two endpoints are served on.
struct GraphqlApi {
    graphql: String,
    realtime: String,
}

/// Create one GraphQL API in the `AMAZON_COGNITO_USER_POOLS` mode over `user_pool_id`, with
/// [`SCHEMA`], a `NONE` data source and one resolver per field.
///
/// `app_id_client_regex` is the configuration's own `appIdClientRegex`, or `None` for a
/// configuration that declares none — the seam the app-client rows vary.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    user_pool_id: &str,
    app_id_client_regex: Option<&str>,
) -> GraphqlApi {
    let mut user_pool_config = json!({
        "userPoolId": user_pool_id,
        "awsRegion": REGION,
        "defaultAction": "ALLOW",
    });
    if let Some(expression) = app_id_client_regex {
        user_pool_config["appIdClientRegex"] = json!(expression);
    }
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("user-pool-{name}"),
            "authenticationType": "AMAZON_COGNITO_USER_POOLS",
            "userPoolConfig": user_pool_config,
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    let realtime = crate::graphql_api_endpoint_dns_records::published_authority(api, "REALTIME");
    for published in [&graphql, &realtime] {
        assert!(
            published.starts_with(&format!("{LOOPBACK_HOST}:")),
            "an API's endpoints must be published on an address this emulator serves, got \
             {published:?}"
        );
    }
    install_schema(router, &api_id, SCHEMA).await;
    // The configuration the mode reads is the STORED one: what a later read reports is what
    // the create persisted, rather than what the request echoed.
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetGraphqlApi must answer 200, got {status} {fetched}"
    );
    assert_eq!(
        fetched["graphqlApi"]["userPoolConfig"], user_pool_config,
        "GetGraphqlApi must report the userPoolConfig the create stored, got {fetched}"
    );
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
    create_resolvers(router, &api_id).await;
    GraphqlApi { graphql, realtime }
}

/// Attach one resolver per field of [`SCHEMA`]: each is APPSYNC_JS `code` whose request
/// function returns the payload the local data source passes through.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    let constant = |value: &str| {
        format!(
            "export function request(ctx) {{ return {{payload: \"{value}\"}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}"
        )
    };
    // What the identity-reporting field answers: the members `$context.identity` carries for
    // a call the user pool mode admitted, joined so one string pins all three.
    let whoami = "export function request(ctx) {\n\
           const identity = ctx.identity || {};\n\
           const groups = identity.groups || [];\n\
           return {payload: identity.username + \"/\" + identity.sub + \"/\" + groups.join(\",\")};\n\
         }\n\
         export function response(ctx) { return ctx.result; }";
    let fields: &[(&str, String)] = &[
        ("open", constant(OPEN_VALUE)),
        ("adminOnly", constant(ADMIN_VALUE)),
        ("editorsOrAdmins", constant(EITHER_VALUE)),
        ("cognitoGuarded", constant(COGNITO_VALUE)),
        ("whoami", whoami.to_string()),
        (
            "note",
            format!(
                "export function request(ctx) {{ \
                   return {{payload: {{id: \"N-1\", secret: \"{SECRET_VALUE}\"}}}}; }}\n\
                 export function response(ctx) {{ return ctx.result; }}"
            ),
        ),
    ];
    for (field, code) in fields {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": code,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }
}

/// POST one GraphQL document to `api`'s published endpoint, presenting `presented` in the
/// request's `authorization` field — or nothing at all.
async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    query: &str,
    presented: Option<&str>,
) -> (u16, Value) {
    let body = json!({"query": query});
    let fields: Vec<(&str, &str)> = match presented {
        Some(value) => vec![("authorization", value)],
        None => Vec::new(),
    };
    let (status, _, document) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", &body, &fields)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the published GraphQL endpoint {:?} must answer this \
                     request",
                    api.graphql
                )
            });
    (status, document)
}

/// What one row of the token table owes the call.
#[derive(Debug)]
enum Expected {
    /// The call is served and `open` resolves.
    Served,
    /// The call authorizes nobody.
    Unauthorized,
}

#[tokio::test]
async fn a_cognito_authorized_api_serves_only_the_tokens_its_user_pool_signed() {
    let (router, engine) = emulator();

    // The pool the API accepts tokens of, with a user in the guarded group and a user in
    // none, and a SECOND pool whose tokens no API of this suite accepts.
    let (pool_id, client_id) = pool_with_client(&router, "appsync-pool").await;
    let other_client_id = app_client(&router, &pool_id, "another-client").await;
    user_in_groups(&router, &pool_id, "alice", &[ADMINS]).await;
    user_in_groups(&router, &pool_id, "bob", &[]).await;
    let (other_pool_id, other_pool_client_id) = pool_with_client(&router, "other-pool").await;
    user_in_groups(&router, &other_pool_id, "carol", &[]).await;

    let alice = sign_in(&router, &pool_id, &client_id, "alice").await;
    let bob = sign_in(&router, &pool_id, &client_id, "bob").await;
    let alice_other_client = sign_in(&router, &pool_id, &other_client_id, "alice").await;
    let carol = sign_in(&router, &other_pool_id, &other_pool_client_id, "carol").await;

    // The tokens are the POOL's: the claims the rows below rest on are the ones it minted.
    let alice_claims = claims_of(&id_token(&alice));
    assert_eq!(
        alice_claims.get("cognito:groups").cloned(),
        Some(json!([ADMINS])),
        "setup: the pool's identity token must claim the group the user was added to, got \
         {alice_claims}"
    );

    let api = create_api(&router, "tokens", &pool_id, None).await;
    // The same pool, under a configuration that filters app clients by the one the suite's
    // first client holds.
    let filtered = create_api(
        &router,
        "filtered",
        &pool_id,
        Some(&format!("^{client_id}$")),
    )
    .await;

    // A token the pool itself signed whose validity window has already elapsed: every other
    // claim is one of the pool's own identity token.
    let mut elapsed_claims = alice_claims.clone();
    elapsed_claims["exp"] = json!(seconds_ago(Duration::from_secs(600)));
    let elapsed = pool_signed_token(&engine, &pool_id, &elapsed_claims);

    /// One row: what it presents, and what the API owes it.
    struct Row {
        case: String,
        presented: Option<String>,
        expected: Expected,
    }
    let rows = vec![
        Row {
            case: "the pool's identity token, presented bare".to_string(),
            presented: Some(id_token(&alice)),
            expected: Expected::Served,
        },
        Row {
            case: "the pool's ACCESS token for the same sign-in".to_string(),
            presented: Some(access_token(&alice)),
            expected: Expected::Served,
        },
        Row {
            case: "the same identity token with the bearer scheme in front of it".to_string(),
            presented: Some(format!("Bearer {}", id_token(&alice))),
            expected: Expected::Served,
        },
        Row {
            case: "a token of another user of the same pool".to_string(),
            presented: Some(id_token(&bob)),
            expected: Expected::Served,
        },
        Row {
            case: "a token minted by another user pool entirely".to_string(),
            presented: Some(id_token(&carol)),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "the pool's token with its claim set rewritten after signing".to_string(),
            presented: Some(with_rewritten_claims(&id_token(&alice), |claims| {
                claims["sub"] = json!("somebody-else");
            })),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "the pool's token with its signature rewritten".to_string(),
            presented: Some(with_rewritten_signature(&id_token(&alice))),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "a token whose issuer is rewritten to a pool nothing created".to_string(),
            presented: Some(with_rewritten_claims(&id_token(&alice), |claims| {
                claims["iss"] = json!(format!("https://cognito-idp.{REGION}.amazonaws.com/{REGION}_nosuchpool"));
            })),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "a presented value that is no token at all".to_string(),
            presented: Some("not-a-token-at-all".to_string()),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "a token the pool signed whose window has elapsed".to_string(),
            presented: Some(elapsed.clone()),
            expected: Expected::Unauthorized,
        },
        Row {
            case: "nothing presented at all".to_string(),
            presented: None,
            expected: Expected::Unauthorized,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let (status, answered) =
            post_graphql(case, &api, "query { open }", row.presented.as_deref()).await;
        match row.expected {
            Expected::Served => {
                assert_eq!(
                    (status, answered["data"]["open"].as_str()),
                    (200, Some(OPEN_VALUE)),
                    "case {case:?}: the call must be served by the API's own resolver, got \
                     {status} and {answered}"
                );
            }
            Expected::Unauthorized => {
                let reported = answered["errors"][0]["errorType"].as_str();
                assert_eq!(
                    (status, reported),
                    (UNAUTHORIZED_STATUS, Some(UNAUTHORIZED_ERROR_TYPE)),
                    "case {case:?}: expected ({UNAUTHORIZED_STATUS}, \
                     {UNAUTHORIZED_ERROR_TYPE:?}), got {status} and {answered}"
                );
                assert!(
                    answered.get("data").is_none() || answered["data"].is_null(),
                    "case {case:?}: a refused call must reach no resolver, got {answered}"
                );
            }
        }
    }

    // The app-client filter: the same pool, the same user, and only the app client the
    // expression matches is admitted.
    let filter_rows: &[(&str, String, Expected)] = &[
        (
            "an app client the appIdClientRegex matches",
            id_token(&alice),
            Expected::Served,
        ),
        (
            "an access token of that same app client",
            access_token(&alice),
            Expected::Served,
        ),
        (
            "another app client of the same pool",
            id_token(&alice_other_client),
            Expected::Unauthorized,
        ),
    ];
    for (case, presented, expected) in filter_rows {
        let (status, answered) =
            post_graphql(case, &filtered, "query { open }", Some(presented)).await;
        match expected {
            Expected::Served => assert_eq!(
                (status, answered["data"]["open"].as_str()),
                (200, Some(OPEN_VALUE)),
                "case {case:?}: the call must be served, got {status} and {answered}"
            ),
            Expected::Unauthorized => assert_eq!(
                (status, answered["errors"][0]["errorType"].as_str()),
                (UNAUTHORIZED_STATUS, Some(UNAUTHORIZED_ERROR_TYPE)),
                "case {case:?}: expected ({UNAUTHORIZED_STATUS}, \
                 {UNAUTHORIZED_ERROR_TYPE:?}), got {status} and {answered}"
            ),
        }
    }
}

/// RULE: a refusal of a value presented in the `authorization` field states HOW FAR the
/// decision got, and nothing about the value itself. A value the mode could not READ as a
/// JSON Web Token is refused as a token that could not be parsed — nothing about it can be
/// judged until it has been read — and a token that WAS read and then not admitted, whichever
/// of the later checks it failed, earns the one refusal every unauthorized call on this data
/// plane earns. Both are `UnauthorizedException` with the status that refusal carries, and
/// neither reaches a resolver.
///
/// The rows span what the rule generalises over: WHICH form rule a value breaks (the segment
/// count, the header, the `alg` it names, the claim segment, the unsecured form nothing
/// signed), whether the bearer scheme is in front of it, and — on the other side — which
/// later check a readable token fails (its issuer, its signature, its window, its app client).
#[tokio::test]
async fn a_refusal_states_whether_the_presented_value_could_be_read_as_a_token() {
    let (router, engine) = emulator();
    let (pool_id, client_id) = pool_with_client(&router, "refusal-pool").await;
    user_in_groups(&router, &pool_id, "alice", &[ADMINS]).await;
    let (other_pool_id, other_pool_client_id) = pool_with_client(&router, "refusal-other").await;
    user_in_groups(&router, &other_pool_id, "carol", &[]).await;
    let alice = sign_in(&router, &pool_id, &client_id, "alice").await;
    let carol = sign_in(&router, &other_pool_id, &other_pool_client_id, "carol").await;
    let api = create_api(&router, "refusals", &pool_id, None).await;

    let minted = id_token(&alice);
    // A token the pool itself signed whose window has already elapsed, and one that names no
    // issuer at all: both are READ, so both are refused by a later check.
    let mut elapsed_claims = claims_of(&minted);
    elapsed_claims["exp"] = json!(seconds_ago(Duration::from_secs(600)));
    let elapsed = pool_signed_token(&engine, &pool_id, &elapsed_claims);
    let mut issuerless_claims = claims_of(&minted);
    if let Some(claims) = issuerless_claims.as_object_mut() {
        claims.remove("iss");
    }
    let issuerless = pool_signed_token(&engine, &pool_id, &issuerless_claims);

    /// One row: what it presents, and the sentence the refusal has to carry.
    struct Row {
        case: String,
        presented: String,
        message: &'static str,
    }
    let unreadable = |case: &str, presented: String| Row {
        case: case.to_string(),
        presented,
        message: UNREADABLE_TOKEN_MESSAGE,
    };
    let unauthorized = |case: &str, presented: String| Row {
        case: case.to_string(),
        presented,
        message: UNAUTHORIZED_MESSAGE,
    };
    let rows = vec![
        unreadable(
            "a value that is no token at all",
            "not-a-token-at-all".to_string(),
        ),
        unreadable(
            "a value of two segments only",
            segments_of(&minted)[..2].join("."),
        ),
        unreadable(
            "a value of four segments",
            format!("{minted}.extra"),
        ),
        unreadable(
            "a token whose header segment is not base64url",
            with_segment(&minted, 0, "!!!not-base64!!!"),
        ),
        unreadable(
            "a token whose header names no algorithm",
            with_segment(
                &minted,
                0,
                &service_common::jwt::encode_base64url(b"{\"kid\":\"k\"}"),
            ),
        ),
        unreadable(
            "a token whose claim segment is not base64url",
            with_segment(&minted, 1, "!!!not-base64!!!"),
        ),
        unreadable(
            "a token whose claim segment is no JSON object",
            with_segment(
                &minted,
                1,
                &service_common::jwt::encode_base64url(b"[\"not\",\"a\",\"claim\",\"set\"]"),
            ),
        ),
        unreadable(
            "the unsecured form, which nothing signed",
            format!(
                "{}.{}.",
                service_common::jwt::encode_base64url(b"{\"alg\":\"none\"}"),
                segments_of(&minted)[1]
            ),
        ),
        unreadable(
            "the bearer scheme in front of a value that is no token",
            "Bearer not-a-token-at-all".to_string(),
        ),
        unauthorized(
            "a readable token minted by another user pool",
            id_token(&carol),
        ),
        unauthorized(
            "a readable token with its signature rewritten",
            with_rewritten_signature(&minted),
        ),
        unauthorized(
            "a readable token whose claim set was rewritten after signing",
            with_rewritten_claims(&minted, |claims| {
                claims["sub"] = json!("somebody-else");
            }),
        ),
        unauthorized(
            "a token the pool signed whose window has elapsed",
            elapsed.clone(),
        ),
        unauthorized(
            "a token the pool signed that names no issuer",
            issuerless.clone(),
        ),
    ];

    for row in &rows {
        let case = &row.case;
        let (status, answered) =
            post_graphql(case, &api, "query { open }", Some(&row.presented)).await;
        assert_eq!(
            (
                status,
                answered["errors"][0]["errorType"].as_str(),
                answered["errors"][0]["message"].as_str()
            ),
            (
                UNAUTHORIZED_STATUS,
                Some(UNAUTHORIZED_ERROR_TYPE),
                Some(row.message)
            ),
            "case {case:?}: expected ({UNAUTHORIZED_STATUS}, {UNAUTHORIZED_ERROR_TYPE:?}, {:?}), \
             got {status} and {answered}",
            row.message
        );
        assert_eq!(
            answered["errors"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a refusal reports exactly one error, got {answered}"
        );
        assert!(
            answered.get("data").is_none() || answered["data"].is_null(),
            "case {case:?}: a refused call must reach no resolver, got {answered}"
        );
    }

    // And the token the pool minted for the same API is served, so the rows above fail on
    // what they present rather than on the API.
    let (status, answered) = post_graphql(
        "the pool's own identity token",
        &api,
        "query { open }",
        Some(&minted),
    )
    .await;
    assert_eq!(
        (status, answered["data"]["open"].as_str()),
        (200, Some(OPEN_VALUE)),
        "the pool's own token must still be served, got {status} and {answered}"
    );
}

/// The three segments of a compact JWS, as the rows above take one apart.
fn segments_of(token: &str) -> Vec<String> {
    token.split('.').map(str::to_string).collect()
}

/// `token` with the segment `at` replaced by `replacement` — how a row breaks ONE form rule
/// and leaves the rest of the value as the pool minted it.
fn with_segment(token: &str, at: usize, replacement: &str) -> String {
    let mut segments = segments_of(token);
    segments[at] = replacement.to_string();
    segments.join(".")
}

#[tokio::test]
async fn an_admitted_tokens_claims_are_the_identity_and_the_group_guards_are_judged_by_them() {
    let (router, _engine) = emulator();
    let (pool_id, client_id) = pool_with_client(&router, "groups-pool").await;
    user_in_groups(&router, &pool_id, "alice", &[ADMINS]).await;
    user_in_groups(&router, &pool_id, "bob", &[]).await;
    // A user in a group the guards do NOT name, so a membership on its own admits nothing.
    user_in_groups(&router, &pool_id, "dana", &[EDITORS]).await;

    let alice = sign_in(&router, &pool_id, &client_id, "alice").await;
    let bob = sign_in(&router, &pool_id, &client_id, "bob").await;
    let dana = sign_in(&router, &pool_id, &client_id, "dana").await;
    let api = create_api(&router, "groups", &pool_id, None).await;

    /// One row: the field selected, the token presented, and the value the field must
    /// resolve to — `None` where the caller may not access it.
    struct Row {
        case: String,
        field: &'static str,
        presented: String,
        resolved: Option<&'static str>,
    }
    let rows = vec![
        Row {
            case: "an unguarded field, for a caller in no group".to_string(),
            field: "open",
            presented: id_token(&bob),
            resolved: Some(OPEN_VALUE),
        },
        Row {
            case: "a guard naming one group, for a caller in it".to_string(),
            field: "adminOnly",
            presented: id_token(&alice),
            resolved: Some(ADMIN_VALUE),
        },
        Row {
            case: "the same guard, for a caller in no group at all".to_string(),
            field: "adminOnly",
            presented: id_token(&bob),
            resolved: None,
        },
        Row {
            case: "the same guard, for a caller in another group".to_string(),
            field: "adminOnly",
            presented: id_token(&dana),
            resolved: None,
        },
        Row {
            case: "a guard naming two groups, for a caller in the LAST of them".to_string(),
            field: "editorsOrAdmins",
            presented: id_token(&alice),
            resolved: Some(EITHER_VALUE),
        },
        Row {
            case: "the same guard, for a caller in the FIRST of them".to_string(),
            field: "editorsOrAdmins",
            presented: id_token(&dana),
            resolved: Some(EITHER_VALUE),
        },
        Row {
            case: "the same guard, for a caller in neither".to_string(),
            field: "editorsOrAdmins",
            presented: id_token(&bob),
            resolved: None,
        },
        Row {
            case: "the multi-mode spelling of the guard, for a caller in its group".to_string(),
            field: "cognitoGuarded",
            presented: id_token(&alice),
            resolved: Some(COGNITO_VALUE),
        },
        Row {
            case: "that spelling, for a caller in no group".to_string(),
            field: "cognitoGuarded",
            presented: id_token(&bob),
            resolved: None,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let query = format!("query {{ {} }}", row.field);
        let (status, answered) = post_graphql(case, &api, &query, Some(&row.presented)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call the user pool admitted must be served, got {answered}"
        );
        match row.resolved {
            Some(value) => {
                assert_eq!(
                    answered["data"][row.field].as_str(),
                    Some(value),
                    "case {case:?}: expected {value:?}, got {answered}"
                );
                assert!(
                    answered.get("errors").is_none(),
                    "case {case:?}: a field the caller may access reports no error, got \
                     {answered}"
                );
            }
            None => {
                assert!(
                    answered["data"][row.field].is_null(),
                    "case {case:?}: a guarded field must resolve to nothing for a caller \
                     outside its groups, got {answered}"
                );
                assert_eq!(
                    answered["errors"][0]["errorType"].as_str(),
                    Some(UNAUTHORIZED_FIELD_ERROR_TYPE),
                    "case {case:?}: expected the field to be reported as one the caller may \
                     not access, got {answered}"
                );
                let message = answered["errors"][0]["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(row.field) && message.contains("Query"),
                    "case {case:?}: the error must name the field and its type, got \
                     {message:?}"
                );
            }
        }
    }

    // A guard on a NON-ROOT type's field is judged where that field resolves, so the same
    // selection answers the member to a caller in its groups and withholds it from every
    // other.
    let nested_rows: &[(&str, &Value, Option<&str>)] = &[
        ("a caller in the guard's group", &alice, Some(SECRET_VALUE)),
        ("a caller in no group", &bob, None),
    ];
    for (case, signed_in, expected) in nested_rows {
        let (status, answered) = post_graphql(
            case,
            &api,
            "query { note { id secret } }",
            Some(&id_token(signed_in)),
        )
        .await;
        assert_eq!(
            (status, answered["data"]["note"]["id"].as_str()),
            (200, Some("N-1")),
            "case {case:?}: the unguarded member of the same selection must resolve, got \
             {status} and {answered}"
        );
        match expected {
            Some(value) => assert_eq!(
                answered["data"]["note"]["secret"].as_str(),
                Some(*value),
                "case {case:?}: expected the guarded member to resolve, got {answered}"
            ),
            None => {
                assert!(
                    answered["data"]["note"]["secret"].is_null(),
                    "case {case:?}: a guarded member must resolve to nothing for a caller \
                     outside its groups, got {answered}"
                );
                let message = answered["errors"][0]["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains("secret") && message.contains("Note"),
                    "case {case:?}: the error must name the member and its type, got \
                     {message:?}"
                );
            }
        }
    }

    // What a resolver reads as `$context.identity` is the TOKEN's own claims: the subject,
    // the user name and the groups it claims.
    let identity_rows: &[(&str, &Value, &str, &str)] = &[
        ("a caller in one group", &alice, "alice", ADMINS),
        ("a caller in no group", &bob, "bob", ""),
    ];
    for (case, signed_in, username, groups) in identity_rows {
        let presented = id_token(signed_in);
        let claims = claims_of(&presented);
        let subject = claims["sub"].as_str().unwrap_or_default().to_string();
        let (status, answered) =
            post_graphql(case, &api, "query { whoami }", Some(&presented)).await;
        let expected = format!("{username}/{subject}/{groups}");
        assert_eq!(
            (status, answered["data"]["whoami"].as_str()),
            (200, Some(expected.as_str())),
            "case {case:?}: $context.identity must report the token's own claims, expected \
             {expected:?}, got {status} and {answered}"
        );
    }
}

#[tokio::test]
async fn a_subscription_is_started_only_where_the_fields_guard_admits_the_caller() {
    let (router, _engine) = emulator();
    let (pool_id, client_id) = pool_with_client(&router, "realtime-pool").await;
    user_in_groups(&router, &pool_id, "alice", &[ADMINS]).await;
    user_in_groups(&router, &pool_id, "bob", &[]).await;
    let alice = id_token(&sign_in(&router, &pool_id, &client_id, "alice").await);
    let bob = id_token(&sign_in(&router, &pool_id, &client_id, "bob").await);
    let api = create_api(&router, "realtime", &pool_id, None).await;

    /// One row: the subscriber's token, the subscription field it starts on, and whether the
    /// endpoint must ack it.
    struct Row {
        case: &'static str,
        presented: String,
        field: &'static str,
        acked: bool,
    }
    // The rows differ along the field's guard (none, one naming a group), along which groups
    // the subscriber's own token claims, and along a token the pool did not sign.
    let rows = vec![
        Row {
            case: "an unguarded subscription field, for a caller in no group",
            presented: bob.clone(),
            field: "onOpen",
            acked: true,
        },
        Row {
            case: "a guarded subscription field, for a caller in its group",
            presented: alice.clone(),
            field: "onAdminOnly",
            acked: true,
        },
        Row {
            case: "the same guarded field, for a caller in no group",
            presented: bob.clone(),
            field: "onAdminOnly",
            acked: false,
        },
        Row {
            case: "a token of this pool rewritten after signing",
            presented: with_rewritten_signature(&alice),
            field: "onOpen",
            acked: false,
        },
    ];
    for row in &rows {
        let case = row.case;
        // The material travels in the handshake's base64url `header` object, which is where a
        // real-time client of a GraphQL API puts a request's own fields.
        let mut socket = handshake(
            case,
            &api.realtime,
            GRAPHQL_PATH,
            true,
            Some(&json!({"host": api.realtime, "Authorization": row.presented})),
            &[],
        )
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must complete, got {status}")
        });
        let acked = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
        // A token the mode refuses outright ends the connection at its init; one it admits is
        // judged per subscription field below.
        if !row.acked && row.field == "onOpen" {
            assert_eq!(
                acked.as_ref().and_then(|frame| frame["type"].as_str()),
                Some("connection_error"),
                "case {case:?}: a connection presenting a token the pool did not sign must be \
                 refused at its init, got {acked:?}"
            );
            continue;
        }
        assert_eq!(
            acked.as_ref().and_then(|frame| frame["type"].as_str()),
            Some("connection_ack"),
            "case {case:?}: connection_init must be acked, got {acked:?}"
        );
        let document = format!("subscription Watch {{ {} }}", row.field);
        let started = exchange(
            case,
            &mut socket,
            &started_message("sub-1", &document, &json!({})),
        )
        .await;
        let kind = started.as_ref().and_then(|frame| frame["type"].as_str());
        match row.acked {
            true => assert_eq!(
                kind,
                Some("start_ack"),
                "case {case:?}: the start must be acked, got {started:?}"
            ),
            false => {
                assert_eq!(
                    kind,
                    Some("error"),
                    "case {case:?}: a start on a field whose guard the caller does not \
                     satisfy must be refused, got {started:?}"
                );
                assert_eq!(
                    started
                        .as_ref()
                        .and_then(|frame| frame.pointer("/payload/errors/0/errorType"))
                        .and_then(Value::as_str),
                    Some(UNAUTHORIZED_ERROR_TYPE),
                    "case {case:?}: expected the refusal of an unauthorized call, got \
                     {started:?}"
                );
            }
        }
    }
}
