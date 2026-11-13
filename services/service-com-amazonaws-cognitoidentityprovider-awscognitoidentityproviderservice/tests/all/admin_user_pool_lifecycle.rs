// SPDX-License-Identifier: CC0-1.0

//! The admin user-pool lifecycle over the wire: `CreateUserPool` ->
//! `CreateUserPoolClient` -> `AdminCreateUser` -> `AdminSetUserPassword` ->
//! `AdminInitiateAuth` -> `AdminGetUser` -> `AdminDeleteUser` ->
//! `DeleteUserPool`, driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! Every suite asserts response BODIES, not just status codes, and each table
//! row names the case it is asserting.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service and what
/// names the region its pool ids and ARNs are minted in. The signature is never
/// checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The region of [`AUTHORIZATION`], which every minted pool id must carry.
const REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// One emulator with just this service mounted, serving `engine`'s store, so a
/// suite that holds the engine sees the very state the handlers read and write.
fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The modelled `UserPoolIdType` pattern `^[\w-]+_[0-9a-zA-Z]+$`, checked
/// without a regex dependency: a non-empty prefix of word or `-` characters, an
/// `_`, then a non-empty alphanumeric suffix.
fn matches_user_pool_id_pattern(id: &str) -> bool {
    let Some((prefix, suffix)) = id.rsplit_once('_') else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The modelled `ClientIdType` / `ClientSecretType` pattern `^[\w+]+$`.
fn matches_client_id_pattern(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
}

/// The claims of a JWS compact token, which also asserts its shape: three
/// NON-EMPTY base64url segments, as RFC 7515 §3.1 defines for a signed JWS —
/// the third being the signature a pool's key set verifies (see
/// `tests/all/token_signing.rs`, which checks that signature).
fn jwt_claims(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a token must have three JWS segments, got {token}"
    );
    assert!(
        !segments[2].is_empty(),
        "a signed JWS carries a signature, got {token}"
    );
    let payload = decode_base64url(segments[1])
        .unwrap_or_else(|| panic!("the claims must be base64url: {token}"));
    serde_json::from_slice(&payload)
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// A base64url segment's bytes, decoded with the codec the wire serializer uses
/// (`aws_smithy_types::base64`): the two alphabet substitutions and the padding
/// RFC 7515 §2 strips are restored first.
fn decode_base64url(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard).ok()
}

/// Create a pool, an app client that allows admin password auth, and a user with
/// a permanent password. Returns `(user_pool_id, client_id, sub)`.
async fn pool_with_signed_in_user(router: &MultiServiceRouter) -> (String, String, String) {
    let pool = ok(router, "CreateUserPool", &json!({ "PoolName": "fixture-pool" })).await;
    let user_pool_id = pool["UserPool"]["Id"].as_str().expect("a pool id").to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "fixture-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a client id")
        .to_string();
    let user = ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "UserAttributes": [{ "Name": "email", "Value": "alice@example.com" }],
        }),
    )
    .await;
    let sub = user["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    (user_pool_id, client_id, sub)
}

#[tokio::test]
async fn the_admin_lifecycle_round_trips_through_stored_state() {
    let router = router();

    // ---- CreateUserPool -------------------------------------------------
    let created = ok(&router, "CreateUserPool", &json!({ "PoolName": "demo-pool" })).await;
    let pool = &created["UserPool"];
    let user_pool_id = pool["Id"].as_str().expect("a minted pool id").to_string();
    assert!(
        matches_user_pool_id_pattern(&user_pool_id),
        "the pool id must match ^[\\w-]+_[0-9a-zA-Z]+$, got {user_pool_id:?}"
    );
    assert!(
        user_pool_id.starts_with(&format!("{REGION}_")),
        "the pool id must name the request's region {REGION}, got {user_pool_id:?}"
    );
    assert_eq!(
        pool["Name"], "demo-pool",
        "the stored pool name must be the created one, got {pool}"
    );
    assert_eq!(
        pool["Arn"],
        json!(format!(
            "arn:aws:cognito-idp:{REGION}:000000000000:userpool/{user_pool_id}"
        )),
        "the ARN must name the stored pool, got {pool}"
    );
    assert!(
        pool["CreationDate"].is_number() && pool["LastModifiedDate"].is_number(),
        "a stored pool carries both timestamps, got {pool}"
    );

    // ---- CreateUserPoolClient -------------------------------------------
    let created = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "demo-client",
            "GenerateSecret": false,
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client = &created["UserPoolClient"];
    let client_id = client["ClientId"].as_str().expect("a client id").to_string();
    assert!(
        matches_client_id_pattern(&client_id),
        "the client id must match ^[\\w+]+$, got {client_id:?}"
    );
    assert_eq!(
        client["UserPoolId"], json!(user_pool_id),
        "the client must name the pool it was created on, got {client}"
    );
    assert_eq!(
        client["ClientName"], "demo-client",
        "the stored client name must be the created one, got {client}"
    );
    assert!(
        client.get("ClientSecret").is_none(),
        "GenerateSecret=false mints no secret, got {client}"
    );
    assert_eq!(
        client["ExplicitAuthFlows"],
        json!(["ALLOW_ADMIN_USER_PASSWORD_AUTH"]),
        "the stored auth flows must be the requested ones, got {client}"
    );

    // An unknown pool is a 400, for the client and the user alike.
    let (status, payload) = call(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": "us-east-1_absent", "ClientName": "orphan" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a client on an absent pool is a 400 ResourceNotFoundException, got {payload}"
    );

    // ---- AdminCreateUser ------------------------------------------------
    let created = ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "UserAttributes": [{ "Name": "email", "Value": "alice@example.com" }],
        }),
    )
    .await;
    let user = &created["User"];
    assert_eq!(
        user["Username"], "alice",
        "the stored username must be the created one, got {user}"
    );
    assert_eq!(
        user["Enabled"], json!(true),
        "an administrator-created user is enabled, got {user}"
    );
    assert_eq!(
        user["UserStatus"], "FORCE_CHANGE_PASSWORD",
        "an administrator-created user must change its password, got {user}"
    );
    let attributes = user["Attributes"].as_array().expect("stored attributes");
    let sub = attributes
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .unwrap_or_else(|| panic!("a generated sub must be stored, got {user}"));
    assert_eq!(
        sub.len(),
        36,
        "sub must be a hyphenated UUID, got {sub:?} in {user}"
    );
    assert!(
        attributes
            .iter()
            .any(|attribute| attribute["Name"] == "email"
                && attribute["Value"] == "alice@example.com"),
        "the requested attribute must be stored, got {user}"
    );

    // The same username twice in one pool is refused.
    let (status, payload) = call(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UsernameExistsException"),
        "a username already taken is a 400 UsernameExistsException, got {payload}"
    );

    // ---- AdminSetUserPassword -------------------------------------------
    let empty = ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    assert_eq!(
        empty,
        json!({}),
        "AdminSetUserPassword answers an empty body, got {empty}"
    );

    // ---- AdminGetUser ---------------------------------------------------
    let read = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        read["UserStatus"], "CONFIRMED",
        "a permanent password confirms the stored user, got {read}"
    );
    assert_eq!(
        read["Username"], "alice",
        "the read must be of the stored user, got {read}"
    );
    assert!(
        read["UserAttributes"]
            .as_array()
            .expect("the stored attributes")
            .iter()
            .any(|attribute| attribute["Name"] == "email"
                && attribute["Value"] == "alice@example.com"),
        "the read must carry the attribute AdminCreateUser stored, got {read}"
    );
    assert_eq!(
        read["Enabled"], json!(true),
        "the read must carry the stored Enabled, got {read}"
    );

    // Reading twice sees the same thing: the read changed nothing.
    let read_again = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        read, read_again,
        "a read must not change what the next caller sees"
    );

    // ---- AdminInitiateAuth ----------------------------------------------
    let signed_in = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let result = &signed_in["AuthenticationResult"];
    assert_eq!(
        result["TokenType"], "Bearer",
        "the token type must be Bearer, got {signed_in}"
    );
    assert_eq!(
        result["ExpiresIn"],
        json!(3600),
        "an app client with no AccessTokenValidity issues one-hour tokens, got {signed_in}"
    );
    for token_member in ["AccessToken", "IdToken", "RefreshToken"] {
        let token = result[token_member]
            .as_str()
            .unwrap_or_else(|| panic!("{token_member} must be issued, got {signed_in}"));
        assert!(
            !token.is_empty(),
            "{token_member} must not be empty, got {signed_in}"
        );
    }
    let access_claims = jwt_claims(result["AccessToken"].as_str().expect("an access token"));
    assert_eq!(
        access_claims["sub"], json!(sub),
        "the access token's sub must be the stored user's, got {access_claims}"
    );
    assert_eq!(
        access_claims["username"], "alice",
        "the access token must name the stored user, got {access_claims}"
    );
    assert_eq!(
        access_claims["client_id"], json!(client_id),
        "the access token must name the app client it was issued to, got {access_claims}"
    );
    assert_eq!(
        access_claims["iss"],
        json!(format!(
            "https://cognito-idp.{REGION}.amazonaws.com/{user_pool_id}"
        )),
        "the issuer must be the stored pool's endpoint, got {access_claims}"
    );
    assert_eq!(
        access_claims["token_use"], "access",
        "an access token says so, got {access_claims}"
    );
    let id_claims = jwt_claims(result["IdToken"].as_str().expect("an id token"));
    assert_eq!(
        id_claims["cognito:username"], "alice",
        "the id token must name the stored user, got {id_claims}"
    );
    assert_eq!(
        id_claims["email"], "alice@example.com",
        "the id token must carry the stored email attribute, got {id_claims}"
    );
    // The refresh token beside them states nothing: it is a SEALED credential in JWE
    // compact serialization (RFC 7516 §7.1, five segments), not the three of a readable
    // JWS.
    let refresh = result["RefreshToken"].as_str().expect("a refresh token");
    assert_eq!(
        refresh.split('.').count(),
        5,
        "a refresh token is sealed in JWE compact serialization, not a \
         compact-serialized JWS, got {refresh}"
    );
    assert!(
        !refresh.contains("alice"),
        "a refresh token states nothing of its sign-in, found the user's own name in \
         {refresh}"
    );

    // The wrong password is refused.
    let (status, payload) = call(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": "wrong" },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "a wrong password is a 400 NotAuthorizedException, got {payload}"
    );

    // An absent user is a 400 for this client (PreventUserExistenceErrors=LEGACY).
    let (status, payload) = call(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "bob" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UserNotFoundException"),
        "reading an absent user is a 400 UserNotFoundException, got {payload}"
    );

    // ---- AdminDeleteUser ------------------------------------------------
    let empty = ok(
        &router,
        "AdminDeleteUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        empty,
        json!({}),
        "AdminDeleteUser answers an empty body, got {empty}"
    );
    let (status, payload) = call(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UserNotFoundException"),
        "a deleted user is gone, got {payload}"
    );

    // ---- DeleteUserPool -------------------------------------------------
    let empty = ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        empty,
        json!({}),
        "DeleteUserPool answers an empty body, got {empty}"
    );
    for (operation, body) in [
        (
            "AdminCreateUser",
            json!({ "UserPoolId": user_pool_id, "Username": "carol" }),
        ),
        (
            "CreateUserPoolClient",
            json!({ "UserPoolId": user_pool_id, "ClientName": "late-client" }),
        ),
        (
            "AdminGetUser",
            json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "{operation} on a deleted pool is a 400 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_created_pool_and_client_read_back_through_describe() {
    let router = router();

    let created_pool = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "describe-pool",
            "UserPoolTags": { "owner": "platform" },
            "MfaConfiguration": "OPTIONAL",
        }),
    )
    .await["UserPool"]
        .clone();
    let user_pool_id = created_pool["Id"].as_str().expect("a pool id").to_string();
    let described_pool = ok(
        &router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone();
    // A describe reports exactly the pool the create stored, with ONE addition
    // to the schema it reports: the attribute the service maintains for every
    // pool (`identities`), which belongs to the pool rather than to the
    // configuration the create's own answer reports.
    let schema_names = |pool: &Value| -> Vec<String> {
        pool["SchemaAttributes"]
            .as_array()
            .unwrap_or_else(|| panic!("a pool reports its schema, got {pool}"))
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let without_schema = |pool: &Value| -> Value {
        let mut pool = pool.clone();
        if let Some(members) = pool.as_object_mut() {
            members.remove("SchemaAttributes");
        }
        pool
    };
    let mut expected_schema = schema_names(&created_pool);
    expected_schema.push("identities".to_string());
    assert_eq!(
        schema_names(&described_pool),
        expected_schema,
        "a describe reports the configured schema and the attribute the service maintains, got \
         {described_pool}"
    );
    assert_eq!(
        without_schema(&created_pool),
        without_schema(&described_pool),
        "a describe must report exactly the pool the create stored"
    );

    let created_client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "describe-client",
            "GenerateSecret": true,
            "AccessTokenValidity": 2,
            "ReadAttributes": ["email"],
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]
        .clone();
    let client_id = created_client["ClientId"]
        .as_str()
        .expect("a client id")
        .to_string();
    let described_client = ok(
        &router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await["UserPoolClient"]
        .clone();
    assert_eq!(
        created_client, described_client,
        "a describe must report exactly the app client the create stored"
    );

    // EstimatedNumberOfUsers is computed from the pool's users, so it moves when
    // a user is created rather than being frozen at create time.
    assert_eq!(
        described_pool["EstimatedNumberOfUsers"],
        json!(0),
        "a pool with no users estimates none, got {described_pool}"
    );
    for username in ["alice", "bob"] {
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
    }
    let described_pool = ok(
        &router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone();
    assert_eq!(
        described_pool["EstimatedNumberOfUsers"],
        json!(2),
        "the estimate must count the stored users, got {described_pool}"
    );

    // Unknown ids are 400s, for either resource.
    for (case, operation, request) in [
        (
            "an absent pool",
            "DescribeUserPool",
            json!({ "UserPoolId": "us-east-1_absent" }),
        ),
        (
            "an absent client",
            "DescribeUserPoolClient",
            json!({ "UserPoolId": user_pool_id, "ClientId": "absentclient" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: an absent resource is a 400, got {payload}"
        );
    }
}

#[tokio::test]
async fn every_created_pool_gets_a_unique_id_and_the_documented_defaults() {
    let router = router();

    // The rows differ along what the rule generalises over: which configuration
    // members the request carries, and of which kind (enum, list, map, nested
    // structure, schema).
    let cases = [
        ("no configuration at all", json!({ "PoolName": "plain" })),
        (
            "an enum member",
            json!({ "PoolName": "tiered", "UserPoolTier": "LITE" }),
        ),
        (
            "two list members",
            json!({
                "PoolName": "aliased",
                "UsernameAttributes": ["email"],
                "AutoVerifiedAttributes": ["email"],
            }),
        ),
        (
            "a tag map",
            json!({ "PoolName": "tagged", "UserPoolTags": { "owner": "platform" } }),
        ),
        (
            "a nested policy structure",
            json!({
                "PoolName": "lenient",
                "Policies": { "PasswordPolicy": { "MinimumLength": 6, "RequireSymbols": false } },
            }),
        ),
        (
            "a custom schema attribute",
            json!({
                "PoolName": "extended",
                "Schema": [{ "Name": "tenant", "AttributeDataType": "String" }],
            }),
        ),
        (
            "an MFA setting and deletion protection",
            json!({
                "PoolName": "protected",
                "MfaConfiguration": "OPTIONAL",
                "DeletionProtection": "ACTIVE",
            }),
        ),
    ];

    let mut ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let pool = ok(&router, "CreateUserPool", request).await["UserPool"].clone();
        let id = pool["Id"].as_str().expect("a pool id").to_string();
        assert!(
            matches_user_pool_id_pattern(&id),
            "case {case:?}: the pool id must match ^[\\w-]+_[0-9a-zA-Z]+$, got {id:?}"
        );
        assert!(
            id.len() <= 55,
            "case {case:?}: the pool id must fit the modelled length 1..55, got {id:?}"
        );
        assert!(
            !ids.contains(&id),
            "case {case:?}: every pool must get its own id, got the repeat {id:?}"
        );
        ids.push(id.clone());

        assert_eq!(
            pool["Name"], request["PoolName"],
            "case {case:?}: the stored name must be the requested one, got {pool}"
        );
        assert_eq!(
            pool["EstimatedNumberOfUsers"],
            json!(0),
            "case {case:?}: a new pool holds no users, got {pool}"
        );
        // Defaults the pool is stored WITH, so a later read sees them too.
        assert_eq!(
            pool["UserPoolTier"],
            request
                .get("UserPoolTier")
                .cloned()
                .unwrap_or_else(|| json!("ESSENTIALS")),
            "case {case:?}: UserPoolTier defaults to ESSENTIALS, got {pool}"
        );
        assert_eq!(
            pool["MfaConfiguration"],
            request
                .get("MfaConfiguration")
                .cloned()
                .unwrap_or_else(|| json!("OFF")),
            "case {case:?}: MfaConfiguration defaults to OFF, got {pool}"
        );
        assert_eq!(
            pool["DeletionProtection"],
            request
                .get("DeletionProtection")
                .cloned()
                .unwrap_or_else(|| json!("INACTIVE")),
            "case {case:?}: deletion protection is opt-in, got {pool}"
        );
        assert_eq!(
            pool["AdminCreateUserConfig"]["UnusedAccountValidityDays"],
            json!(7),
            "case {case:?}: UnusedAccountValidityDays defaults to 7, got {pool}"
        );
        let password_policy = &pool["Policies"]["PasswordPolicy"];
        let expected_minimum = request["Policies"]["PasswordPolicy"]["MinimumLength"]
            .as_i64()
            .unwrap_or(8);
        assert_eq!(
            password_policy["MinimumLength"],
            json!(expected_minimum),
            "case {case:?}: the stored minimum length must be the requested one or 8, got {pool}"
        );
        assert_eq!(
            password_policy["TemporaryPasswordValidityDays"],
            json!(7),
            "case {case:?}: TemporaryPasswordValidityDays defaults to 7, got {pool}"
        );

        // The standard attribute schema every pool carries, checked on an entry
        // the issue did not cite.
        let schema = pool["SchemaAttributes"]
            .as_array()
            .expect("a stored schema")
            .clone();
        for standard in ["sub", "email", "phone_number", "updated_at"] {
            let entry = schema
                .iter()
                .find(|entry| entry["Name"] == standard)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the schema must declare {standard}, got {pool}")
                });
            let expected_type = if standard == "updated_at" {
                "Number"
            } else {
                "String"
            };
            assert_eq!(
                entry["AttributeDataType"], expected_type,
                "case {case:?}: {standard} is a {expected_type}, got {entry}"
            );
        }
        assert_eq!(
            schema
                .iter()
                .find(|entry| entry["Name"] == "sub")
                .map(|entry| entry["Required"].clone()),
            Some(json!(true)),
            "case {case:?}: sub is a required attribute, got {pool}"
        );

        // Echoed-as-persisted members, for the rows that carry them.
        if let Some(tags) = request.get("UserPoolTags") {
            assert_eq!(
                &pool["UserPoolTags"], tags,
                "case {case:?}: the stored tags must be the requested ones, got {pool}"
            );
        }
        if let Some(username_attributes) = request.get("UsernameAttributes") {
            assert_eq!(
                &pool["UsernameAttributes"], username_attributes,
                "case {case:?}: the stored username attributes must be the requested ones, got {pool}"
            );
        }
        if request.get("Schema").is_some() {
            assert!(
                schema
                    .iter()
                    .any(|entry| entry["Name"] == "custom:tenant"),
                "case {case:?}: a requested non-standard attribute is stored under custom:, got {pool}"
            );
        }
    }
}

#[tokio::test]
async fn a_password_must_satisfy_the_pools_own_password_policy() {
    let router = router();

    // A pool on the default policy (8 characters, upper, lower, number, symbol)
    // and one that relaxes two of its terms: the rule is the POOL's policy, not
    // one fixed password shape.
    let strict = ok(&router, "CreateUserPool", &json!({ "PoolName": "strict" })).await["UserPool"]
        ["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let lenient = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "lenient",
            "Policies": {
                "PasswordPolicy": {
                    "MinimumLength": 6,
                    "RequireUppercase": false,
                    "RequireLowercase": true,
                    "RequireNumbers": true,
                    "RequireSymbols": false,
                },
            },
        }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();

    for (pool, username) in [(&strict, "strict-user"), (&lenient, "lenient-user")] {
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": pool, "Username": username }),
        )
        .await;
    }

    // Each row differs along one term of the policy.
    let cases = [
        (&strict, "too short for the minimum length", "Pw1!", false),
        (&strict, "no uppercase letter", "passw0rd!", false),
        (&strict, "no lowercase letter", "PASSW0RD!", false),
        (&strict, "no number", "Password!", false),
        (&strict, "no symbol", "Passw0rdd", false),
        (&strict, "every term satisfied", GOOD_PASSWORD, true),
        (&lenient, "at the relaxed minimum length", "abc123", true),
        (&lenient, "one below the relaxed minimum", "abc12", false),
        (&lenient, "no symbol, which this pool allows", "abcd1234", true),
        (&lenient, "no number, which this pool requires", "abcdefg", false),
    ];

    for (pool, case, password, accepted) in cases {
        let username = if pool == &strict {
            "strict-user"
        } else {
            "lenient-user"
        };
        let (status, payload) = call(
            &router,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": pool,
                "Username": username,
                "Password": password,
                "Permanent": true,
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {password:?} satisfies the pool's policy, got {status} {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidPasswordException"),
                "case {case:?}: {password:?} misses the pool's policy, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn token_lifetimes_come_from_the_app_clients_validity_settings() {
    let router = router();
    let user_pool_id = ok(&router, "CreateUserPool", &json!({ "PoolName": "ttl-pool" })).await
        ["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;

    // The rows differ along the validity value AND its unit; the unit defaults
    // to hours for an access token. Each names a lifetime inside the window the
    // model documents for an access token, five minutes to one day.
    let cases = [
        ("no validity at all", json!({}), 3_600_i64),
        ("hours by default", json!({ "AccessTokenValidity": 2 }), 7_200),
        (
            "an explicit unit of seconds",
            json!({
                "AccessTokenValidity": 300,
                "TokenValidityUnits": { "AccessToken": "seconds" },
            }),
            300,
        ),
        (
            "an explicit unit of minutes",
            json!({
                "AccessTokenValidity": 10,
                "TokenValidityUnits": { "AccessToken": "minutes" },
            }),
            600,
        ),
        (
            "an explicit unit of days",
            json!({
                "AccessTokenValidity": 1,
                "TokenValidityUnits": { "AccessToken": "days" },
            }),
            86_400,
        ),
    ];

    for (case, validity, expected_expires_in) in cases {
        let mut request = json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("client-{}", expected_expires_in),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        });
        for (member, value) in validity.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let client = ok(&router, "CreateUserPoolClient", &request).await["UserPoolClient"].clone();
        let client_id = client["ClientId"].as_str().expect("a client id").to_string();

        let signed_in = ok(
            &router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        let result = &signed_in["AuthenticationResult"];
        assert_eq!(
            result["ExpiresIn"],
            json!(expected_expires_in),
            "case {case:?}: ExpiresIn must be the access token's lifetime in seconds, got {result}"
        );
        let claims = jwt_claims(result["AccessToken"].as_str().expect("an access token"));
        let issued_at = claims["iat"].as_i64().expect("an iat claim");
        let expires_at = claims["exp"].as_i64().expect("an exp claim");
        assert_eq!(
            expires_at - issued_at,
            expected_expires_in,
            "case {case:?}: the token's own exp must agree with ExpiresIn, got {claims}"
        );
    }
}

#[tokio::test]
async fn a_generated_client_secret_fits_the_modelled_constraints() {
    let router = router();
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "secret-pool" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();

    // Rows: no secret asked for, a generated one, a caller-supplied one, and the
    // combination the model forbids.
    let cases = [
        ("GenerateSecret absent", json!({}), Some(false)),
        ("GenerateSecret false", json!({ "GenerateSecret": false }), Some(false)),
        ("GenerateSecret true", json!({ "GenerateSecret": true }), Some(true)),
        (
            "a caller-supplied secret",
            json!({ "ClientSecret": "abcdefghijklmnopqrstuvwx" }),
            Some(true),
        ),
        (
            "both GenerateSecret and ClientSecret",
            json!({ "GenerateSecret": true, "ClientSecret": "abcdefghijklmnopqrstuvwx" }),
            None,
        ),
    ];

    let mut secrets: Vec<String> = Vec::new();
    for (case, extra, expected) in cases {
        let mut request = json!({ "UserPoolId": user_pool_id, "ClientName": "secret-client" });
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let (status, payload) = call(&router, "CreateUserPoolClient", &request).await;
        let Some(expects_secret) = expected else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: the model forbids both together, got {payload}"
            );
            continue;
        };
        assert_eq!(
            status, 200,
            "case {case:?}: the client must be created, got {status} {payload}"
        );
        let client = &payload["UserPoolClient"];
        match client.get("ClientSecret").and_then(Value::as_str) {
            Some(secret) => {
                assert!(
                    expects_secret,
                    "case {case:?}: no secret was asked for, got {client}"
                );
                assert!(
                    (24..=64).contains(&secret.len()),
                    "case {case:?}: a secret must fit the modelled length 24..64, got {secret:?}"
                );
                assert!(
                    matches_client_id_pattern(secret),
                    "case {case:?}: a secret must match ^[\\w+]+$, got {secret:?}"
                );
                assert!(
                    !secrets.contains(&secret.to_string()),
                    "case {case:?}: each generated secret must be its own, got the repeat {secret:?}"
                );
                secrets.push(secret.to_string());
            }
            None => assert!(
                !expects_secret,
                "case {case:?}: a secret was asked for, got {client}"
            ),
        }
    }
}

#[tokio::test]
async fn an_unmodelled_or_unimplemented_value_is_answered_by_its_own_refusal() {
    let router = router();
    let (user_pool_id, client_id, _sub) = pool_with_signed_in_user(&router).await;

    // A value outside an enumerated set is a 400, whichever member carries it.
    let bad_values = [
        (
            "an unmodelled MfaConfiguration",
            "CreateUserPool",
            json!({ "PoolName": "bad-mfa", "MfaConfiguration": "SOMETIMES" }),
        ),
        (
            "an unmodelled DeletionProtection",
            "CreateUserPool",
            json!({ "PoolName": "bad-protection", "DeletionProtection": "MAYBE" }),
        ),
        (
            "an unmodelled UserPoolTier",
            "CreateUserPool",
            json!({ "PoolName": "bad-tier", "UserPoolTier": "PREMIUM" }),
        ),
        (
            "an unmodelled ExplicitAuthFlows entry",
            "CreateUserPoolClient",
            json!({
                "UserPoolId": user_pool_id,
                "ClientName": "bad-flows",
                // The current spelling is ALLOW_ADMIN_USER_PASSWORD_AUTH.
                "ExplicitAuthFlows": ["ADMIN_USER_PASSWORD_AUTH"],
            }),
        ),
        (
            "an unmodelled AuthFlow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "MAGIC_LINK_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "an unmodelled MessageAction",
            "AdminCreateUser",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "MessageAction": "REMIND",
            }),
        ),
        (
            "an unmodelled DesiredDeliveryMediums entry",
            "AdminCreateUser",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "erin",
                "DesiredDeliveryMediums": ["PIGEON"],
            }),
        ),
        (
            "an unmodelled PreventUserExistenceErrors",
            "CreateUserPoolClient",
            json!({
                "UserPoolId": user_pool_id,
                "ClientName": "bad-existence",
                "PreventUserExistenceErrors": "SOMETIMES",
            }),
        ),
        (
            "an absent required member",
            "CreateUserPool",
            json!({}),
        ),
        (
            "a value outside a modelled range",
            "CreateUserPoolClient",
            json!({
                "UserPoolId": user_pool_id,
                "ClientName": "bad-session",
                // AuthSessionValidity is modelled range 3..15.
                "AuthSessionValidity": 99,
            }),
        ),
    ];
    for (case, operation, request) in &bad_values {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: a request that breaks a modelled rule is a 400 \
             InvalidParameterException, got {payload}"
        );
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|message| message.contains("validation error")),
            "case {case:?}: the refusal must say what it refused, got {payload}"
        );
    }

    // A modelled auth flow this slice does not perform says so with a 501 rather
    // than answering a sign-in nothing performed.
    for auth_flow in [
        "USER_SRP_AUTH",
        "USER_PASSWORD_AUTH",
        "CUSTOM_AUTH",
        "REFRESH_TOKEN_AUTH",
        "USER_AUTH",
    ] {
        let (status, payload) = call(
            &router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": auth_flow,
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (501, "NotImplemented"),
            "case {auth_flow:?}: an auth flow nobody has written answers 501, got {payload}"
        );
    }

    // An app client the flow is not enabled for is refused, not signed in.
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "srp-only" }),
    )
    .await["UserPoolClient"]
        .clone();
    assert_eq!(
        client["ExplicitAuthFlows"],
        json!([
            "ALLOW_REFRESH_TOKEN_AUTH",
            "ALLOW_USER_SRP_AUTH",
            "ALLOW_CUSTOM_AUTH"
        ]),
        "a client that names no flows gets the documented default set, got {client}"
    );
    let (status, payload) = call(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client["ClientId"].as_str().expect("a client id"),
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "a flow the app client does not enable is refused, got {payload}"
    );
}

#[tokio::test]
async fn a_user_that_must_change_its_password_gets_the_challenge_instead_of_tokens() {
    let router = router();
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "challenge-pool" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let client_id = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "challenge-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a client id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "TemporaryPassword": GOOD_PASSWORD,
        }),
    )
    .await;

    // Each row is a stored user state, and each says what sign-in answers.
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    assert_eq!(
        challenged["ChallengeName"], "NEW_PASSWORD_REQUIRED",
        "case \"temporary password\": a user in FORCE_CHANGE_PASSWORD is challenged, \
         got {challenged}"
    );
    assert!(
        challenged.get("AuthenticationResult").is_none(),
        "case \"temporary password\": a challenged sign-in issues no tokens, got {challenged}"
    );
    let session = challenged["Session"].as_str().expect("a session token");
    assert!(
        (20..=4096).contains(&session.len()),
        "case \"temporary password\": the session must fit the modelled length 20..4096, \
         got {session:?}"
    );
    assert_eq!(
        challenged["ChallengeParameters"]["USER_ID_FOR_SRP"], "alice",
        "case \"temporary password\": the challenge names the stored user, got {challenged}"
    );

    // Permanent password: the same request now signs in.
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let signed_in = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    assert!(
        signed_in.get("ChallengeName").is_none(),
        "case \"permanent password\": a confirmed user is not challenged, got {signed_in}"
    );
    assert!(
        signed_in["AuthenticationResult"]["AccessToken"].is_string(),
        "case \"permanent password\": a confirmed user is issued tokens, got {signed_in}"
    );

    // A temporary password set with Permanent=false puts the user back in
    // FORCE_CHANGE_PASSWORD, and sign-in is challenged again.
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": "Temp0rary!1",
            "Permanent": false,
        }),
    )
    .await;
    let read = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        read["UserStatus"], "FORCE_CHANGE_PASSWORD",
        "case \"Permanent=false\": a temporary password must be changed at next sign-in, \
         got {read}"
    );
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": "Temp0rary!1" },
        }),
    )
    .await;
    assert_eq!(
        challenged["ChallengeName"], "NEW_PASSWORD_REQUIRED",
        "case \"Permanent=false\": sign-in is challenged again, got {challenged}"
    );
}

#[tokio::test]
async fn an_absent_user_is_reported_as_the_app_client_configures() {
    let router = router();
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "existence-pool" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();

    // `PreventUserExistenceErrors` is the rule: LEGACY names the absent user,
    // ENABLED answers the same error a wrong password gets.
    let cases = [
        ("the default, which is LEGACY", None, 400, "UserNotFoundException"),
        ("LEGACY", Some("LEGACY"), 400, "UserNotFoundException"),
        ("ENABLED", Some("ENABLED"), 400, "NotAuthorizedException"),
    ];

    for (case, setting, expected_status, expected_code) in cases {
        let mut request = json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("client-{}", setting.unwrap_or("default")),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        });
        if let Some(setting) = setting {
            request["PreventUserExistenceErrors"] = json!(setting);
        }
        let client = ok(&router, "CreateUserPoolClient", &request).await["UserPoolClient"].clone();
        assert_eq!(
            client["PreventUserExistenceErrors"],
            json!(setting.unwrap_or("LEGACY")),
            "case {case:?}: the stored setting must be the requested one or LEGACY, got {client}"
        );
        let (status, payload) = call(
            &router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client["ClientId"].as_str().expect("a client id"),
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "nobody", "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (expected_status, expected_code),
            "case {case:?}: an absent user is reported as the client configures, got {payload}"
        );
    }
}

#[tokio::test]
async fn usernames_are_compared_as_the_pool_configures() {
    let router = router();

    // The rule is the pool's `UsernameConfiguration.CaseSensitive`; the rows are
    // the two settings plus the default.
    let cases = [
        ("the default, which is case-sensitive", None, false),
        ("CaseSensitive true", Some(true), false),
        ("CaseSensitive false", Some(false), true),
    ];

    for (case, case_sensitive, variant_resolves) in cases {
        let mut request = json!({ "PoolName": "case-pool" });
        if let Some(case_sensitive) = case_sensitive {
            request["UsernameConfiguration"] = json!({ "CaseSensitive": case_sensitive });
        }
        let user_pool_id = ok(&router, "CreateUserPool", &request).await["UserPool"]["Id"]
            .as_str()
            .expect("a pool id")
            .to_string();
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": "ALICE" }),
        )
        .await;
        if variant_resolves {
            assert_eq!(
                status, 200,
                "case {case:?}: a case variant names the same user, got {status} {payload}"
            );
            assert_eq!(
                payload["Username"], "alice",
                "case {case:?}: the read must report the STORED username, got {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: a case variant is a different user, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_protected_pool_is_not_deleted_until_protection_is_off() {
    let router = router();
    let protected = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "protected", "DeletionProtection": "ACTIVE" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let unprotected = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "unprotected", "DeletionProtection": "INACTIVE" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();

    let (status, payload) = call(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": protected }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"DeletionProtection=ACTIVE\": a protected pool is not deleted, got {payload}"
    );
    // And the pool is still there to be read.
    let (status, _payload) = call(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": protected, "Username": "nobody" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"DeletionProtection=ACTIVE\": the pool is still stored, so the ABSENT USER \
         is what is missing"
    );

    let (status, payload) = call(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": unprotected }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DeletionProtection=INACTIVE\": an unprotected pool is deleted, got {payload}"
    );
    let (status, payload) = call(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": unprotected }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "case \"already deleted\": a deleted pool is gone, got {payload}"
    );
}

#[tokio::test]
async fn resending_an_invitation_is_the_only_second_create_a_username_accepts() {
    let router = router();
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "resend-pool" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let created = ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "TemporaryPassword": GOOD_PASSWORD,
        }),
    )
    .await;
    let original_sub = created["User"]["Attributes"]
        .as_array()
        .expect("stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();

    // Each row is one `MessageAction` on an EXISTING username.
    let cases = [
        ("MessageAction absent", None, Some("UsernameExistsException")),
        (
            "MessageAction SUPPRESS",
            Some("SUPPRESS"),
            Some("UsernameExistsException"),
        ),
        ("MessageAction RESEND", Some("RESEND"), None),
    ];
    for (case, message_action, expected_error) in cases {
        let mut request = json!({ "UserPoolId": user_pool_id, "Username": "alice" });
        if let Some(message_action) = message_action {
            request["MessageAction"] = json!(message_action);
        }
        let (status, payload) = call(&router, "AdminCreateUser", &request).await;
        match expected_error {
            Some(code) => assert_eq!(
                (status, error_code(&payload)),
                (400, code),
                "case {case:?}: a second create of one username is refused, got {payload}"
            ),
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: RESEND re-invites the stored user, got {status} {payload}"
                );
                assert_eq!(
                    payload["User"]["Attributes"]
                        .as_array()
                        .expect("stored attributes")
                        .iter()
                        .find(|attribute| attribute["Name"] == "sub")
                        .and_then(|attribute| attribute["Value"].as_str()),
                    Some(original_sub.as_str()),
                    "case {case:?}: RESEND must keep the STORED user, not mint a new one, \
                     got {payload}"
                );
            }
        }
    }

    // RESEND to a username that does not exist has no invitation to resend.
    let (status, payload) = call(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "nobody",
            "MessageAction": "RESEND",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UserNotFoundException"),
        "case \"RESEND to an absent user\": there is nothing to resend, got {payload}"
    );
}

#[tokio::test]
async fn the_pool_store_is_bounded_and_a_delete_makes_room_again() {
    use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state::{
        new_user_pool, service_state, PoolSigningKeys, MAX_USER_POOLS,
    };

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let store = service_state(&*engine).expect("the cognito-idp store is reachable");

    // Fill the store to one short of its bound by SEEDING it, not by driving
    // `CreateUserPool` once per slot: each create mints a pool's RSA key pairs,
    // so the bound is reached here in a moment instead of in minutes of key
    // generation, and the requests below still cross the bound over the wire.
    // The seeded pools share one key set, which nothing in this suite reads.
    let seed_keys = PoolSigningKeys::generate();
    {
        let mut guard = store.lock();
        let mut seeded = 0;
        while guard.user_pools.len() < MAX_USER_POOLS - 1 {
            let pool = new_user_pool(
                format!("seeded-{seeded}"),
                REGION,
                std::time::SystemTime::now(),
                seed_keys.clone(),
            );
            guard.user_pools.entry(pool.id.clone()).or_insert(pool);
            seeded += 1;
        }
        assert_eq!(
            guard.user_pools.len(),
            MAX_USER_POOLS - 1,
            "case \"seeded to one short of the bound\": the store must hold \
             {} pools before the bound is crossed over the wire",
            MAX_USER_POOLS - 1
        );
    }

    // This emulator keeps its pools in memory, so the store has a maximum: the
    // request past it is refused rather than growing the process. The create
    // that takes the last free slot is served...
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "the-last-slot" }),
    )
    .await;
    let last_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("the pool in the last free slot must be created, got {created}"))
        .to_string();
    // ...and the one past the bound is not.
    let (status, payload) = call(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "LimitExceededException"),
        "case \"one past the bound\": the store is bounded, got {payload}"
    );

    // And the bound is on what is STORED, not on how many calls were made.
    ok(&router, "DeleteUserPool", &json!({ "UserPoolId": last_id })).await;
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "after-a-delete" }),
    )
    .await;
    assert!(
        created["UserPool"]["Id"].is_string(),
        "case \"after a delete\": freeing a slot must let the next create in, got {created}"
    );
}

#[tokio::test]
async fn an_attribute_the_pools_schema_does_not_declare_is_refused() {
    let router = router();
    let plain = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "schema-plain" }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let extended = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "schema-extended",
            "Schema": [{ "Name": "tenant", "AttributeDataType": "String" }],
        }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();

    // The rule is the POOL's schema: a standard attribute everywhere, a custom
    // one only where the pool declares it.
    let cases = [
        (&plain, "a standard attribute", "email", "a@example.com", true),
        (
            &plain,
            "another standard attribute",
            "phone_number",
            "+15550100",
            true,
        ),
        (
            &plain,
            "a custom attribute this pool does not declare",
            "custom:tenant",
            "acme",
            false,
        ),
        (
            &extended,
            "a custom attribute this pool declares",
            "custom:tenant",
            "acme",
            true,
        ),
        (
            &extended,
            "a custom attribute no pool declares",
            "custom:department",
            "sales",
            false,
        ),
    ];

    for (index, (pool, case, name, value, accepted)) in cases.iter().enumerate() {
        let username = format!("user-{index}");
        let (status, payload) = call(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": pool,
                "Username": username,
                "UserAttributes": [{ "Name": name, "Value": value }],
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {name} is declared by this pool, got {status} {payload}"
            );
            let read = ok(
                &router,
                "AdminGetUser",
                &json!({ "UserPoolId": pool, "Username": username }),
            )
            .await;
            assert!(
                read["UserAttributes"]
                    .as_array()
                    .expect("stored attributes")
                    .iter()
                    .any(|attribute| attribute["Name"] == *name && attribute["Value"] == *value),
                "case {case:?}: the stored attribute must read back, got {read}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: {name} is not declared by this pool, got {payload}"
            );
        }
    }
}
