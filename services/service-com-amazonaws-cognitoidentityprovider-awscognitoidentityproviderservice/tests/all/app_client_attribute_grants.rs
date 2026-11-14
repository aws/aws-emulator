// SPDX-License-Identifier: CC0-1.0

//! What an app client's `ReadAttributes` and `WriteAttributes` let a request
//! authorized by ITS access token see and change.
//!
//! RULE: the app client an access token was issued for bounds the user
//! attributes the self-service half of this API reaches with it. `ReadAttributes`
//! is "the list of user attributes that you want your app client to have read
//! access to. After your user authenticates in your app, their access token
//! authorizes them to read their own attribute value for any attribute in this
//! list" and `WriteAttributes` the same "to set or modify their own attribute
//! value for any attribute in this list" (`CreateUserPoolClient` in this
//! service's model). So where a client configures such a list it is exhaustive:
//! a read reports no attribute outside it, the identity token it issues states no
//! claim for one either — an identity token being where a client reads those
//! values — a write of one is refused, and a client that configures no list
//! narrows nothing. The lists are read as they stand at the time of the call, so
//! narrowing a client narrows the tokens it has already issued.
//!
//! The read rows differ along what the rule generalises over: no list at all, an
//! empty list, a list of one standard attribute, a list naming a custom
//! attribute, and a list naming an attribute the user does not hold. The write
//! rows differ along the operation that writes (`UpdateUserAttributes` and
//! `DeleteUserAttributes`), whether the attribute named is granted, and whether
//! one granted attribute is named beside an ungranted one.
//!
//! The administrative read of the same user names no app client and is bounded by
//! none, so `AdminGetUser` is read beside every narrowed response as the control.
//!
//! Every pool, custom attribute, app client, user and token here is real: they
//! come from `CreateUserPool`, `AddCustomAttributes`, `CreateUserPoolClient`,
//! `UpdateUserPoolClient`, `AdminCreateUser`, `AdminSetUserPassword` and
//! `InitiateAuth`.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The user every row reads and writes through its own app client.
const USERNAME: &str = "restricted";

/// The attributes that user is created holding, beside the `sub` the pool mints
/// for it.
const STORED_ATTRIBUTES: [(&str, &str); 4] = [
    ("email", "reader@example.test"),
    ("name", "Ada Restricted Lovelace"),
    ("given_name", "Ada"),
    ("custom:tier", "gold"),
];

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The attribute NAMES an `AttributeListType` member carries.
fn attribute_names(attributes: &Value) -> BTreeSet<String> {
    attributes
        .as_array()
        .unwrap_or_else(|| panic!("an attribute list must be an array, got {attributes}"))
        .iter()
        .map(|attribute| {
            attribute["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("an attribute must carry a name, got {attribute}"))
                .to_string()
        })
        .collect()
}

/// The value the stored user holds for `name`, read administratively — the
/// control every narrowed read and refused write is compared against.
async fn stored_value(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
) -> Option<String> {
    let user = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
    user["UserAttributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == name)
        .and_then(|attribute| attribute["Value"].as_str())
        .map(str::to_string)
}

/// A pool holding the custom attribute every row's user carries, with the user
/// itself created and confirmed.
async fn pool_with_user(router: &MultiServiceRouter) -> String {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": "attribute-grant-pool" }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    ok(
        router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [{ "Name": "tier" }],
        }),
    )
    .await;
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "UserAttributes": Value::Array(
                STORED_ATTRIBUTES
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    user_pool_id
}

/// Create an app client of `user_pool_id` with the grants `read` and `write`
/// (each `None` for a client that configures no such list) and return its
/// `ClientId`.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    read: Option<&[&str]>,
    write: Option<&[&str]>,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]),
    );
    if let Some(read) = read {
        body.insert("ReadAttributes".to_string(), json!(read));
    }
    if let Some(write) = write {
        body.insert("WriteAttributes".to_string(), json!(write));
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Sign the suite's user in on `client_id` and return the `AuthenticationResult`
/// the sign-in issued.
async fn sign_in(router: &MultiServiceRouter, client_id: &str) -> Value {
    let result = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    result["AuthenticationResult"].clone()
}

/// The access token a sign-in on `client_id` issued.
async fn access_token(router: &MultiServiceRouter, client_id: &str) -> String {
    sign_in(router, client_id).await["AccessToken"]
        .as_str()
        .expect("an issued access token")
        .to_string()
}

/// A base64url JWS segment's bytes (RFC 7515 §2), decoded by the one JOSE codec
/// this workspace holds (`service_common::jwt`).
fn decode_base64url(segment: &str) -> Vec<u8> {
    service_common::jwt::decode_base64url(segment)
        .unwrap_or_else(|| panic!("{segment:?} must be base64url"))
}

/// The claim set of the compact-serialized JWS `token`.
fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// One row of the read table: the grant the app client configures and the
/// attribute names `GetUser` must then report.
struct ReadCase {
    case: &'static str,
    read: Option<&'static [&'static str]>,
    reported: &'static [&'static str],
}

fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "a client configuring no ReadAttributes narrows nothing",
            read: None,
            reported: &["custom:tier", "email", "given_name", "name", "sub"],
        },
        ReadCase {
            case: "one standard attribute granted",
            read: Some(&["email"]),
            reported: &["email", "sub"],
        },
        ReadCase {
            case: "a standard and a custom attribute granted",
            read: Some(&["given_name", "custom:tier"]),
            reported: &["custom:tier", "given_name", "sub"],
        },
        ReadCase {
            case: "a grant naming an attribute the user does not hold",
            read: Some(&["email", "nickname"]),
            reported: &["email", "sub"],
        },
        ReadCase {
            case: "an empty grant",
            read: Some(&[]),
            reported: &["sub"],
        },
    ]
}

#[tokio::test]
async fn a_read_reports_only_the_attributes_the_app_client_may_read() {
    let router = router();
    let user_pool_id = pool_with_user(&router).await;

    // The whole stored profile, which the administrative read — bounded by no app
    // client — reports whatever any app client grants.
    let administrative = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
    let stored = attribute_names(&administrative["UserAttributes"]);

    for (index, row) in read_cases().into_iter().enumerate() {
        let case = row.case;
        let client_id = client(
            &router,
            &user_pool_id,
            &format!("read-client-{index}"),
            row.read,
            None,
        )
        .await;
        let signed_in = sign_in(&router, &client_id).await;
        let token = signed_in["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an access token, got {signed_in}"))
            .to_string();
        let profile = ok(&router, "GetUser", &json!({ "AccessToken": token })).await;
        let reported = attribute_names(&profile["UserAttributes"]);
        let expected: BTreeSet<String> =
            row.reported.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: GetUser must report {expected:?}, got {reported:?}"
        );

        // The identity token the same sign-in issued states the user's
        // attributes, so the grant bounds its claims too: an attribute outside
        // the grant is no claim of it.
        let id_claims = claims_of(signed_in["IdToken"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: the sign-in must issue an IdToken, got {signed_in}")
        }));
        for (name, _value) in STORED_ATTRIBUTES {
            let carried = id_claims.get(name).is_some();
            assert_eq!(
                carried,
                expected.contains(name),
                "case {case:?}: the identity token must carry the claim {name:?} only where the \
                 grant admits it, got {id_claims}"
            );
        }

        // A narrowed read withholds attributes; it does not change them. The
        // administrative read still reports the whole stored profile.
        let after = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
        )
        .await;
        assert_eq!(
            attribute_names(&after["UserAttributes"]),
            stored,
            "case {case:?}: a narrowed read must leave the stored profile alone, got {after}"
        );
    }
}

#[tokio::test]
async fn narrowing_a_client_narrows_the_token_it_already_issued() {
    let router = router();
    let user_pool_id = pool_with_user(&router).await;
    let client_id = client(&router, &user_pool_id, "narrowed-client", None, None).await;
    let token = access_token(&router, &client_id).await;

    let before = attribute_names(
        &ok(&router, "GetUser", &json!({ "AccessToken": token })).await["UserAttributes"],
    );
    assert!(
        before.contains("name") && before.contains("custom:tier"),
        "a client granting nothing explicitly reads the whole profile, got {before:?}"
    );

    ok(
        &router,
        "UpdateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "ReadAttributes": ["email"],
        }),
    )
    .await;

    let after = attribute_names(
        &ok(&router, "GetUser", &json!({ "AccessToken": token })).await["UserAttributes"],
    );
    let expected: BTreeSet<String> = ["email", "sub"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        after, expected,
        "the same token must lose every attribute the narrowed grant leaves out, got {after:?}"
    );
}

/// One row of the write table: the operation that writes, the attributes it
/// names, and whether the app client's grant authorizes the whole request.
struct WriteCase {
    case: &'static str,
    operation: &'static str,
    /// The attribute names the request writes or deletes.
    attributes: &'static [&'static str],
    authorized: bool,
}

/// The grant every write row's app client is configured with, which authorizes
/// two of the user's attributes and neither of the other two.
const WRITE_GRANT: [&str; 2] = ["email", "given_name"];

fn write_cases() -> Vec<WriteCase> {
    vec![
        WriteCase {
            case: "an update naming only granted attributes",
            operation: "UpdateUserAttributes",
            attributes: &["email", "given_name"],
            authorized: true,
        },
        WriteCase {
            case: "an update naming a standard attribute outside the grant",
            operation: "UpdateUserAttributes",
            attributes: &["name"],
            authorized: false,
        },
        WriteCase {
            case: "an update naming a custom attribute outside the grant",
            operation: "UpdateUserAttributes",
            attributes: &["custom:tier"],
            authorized: false,
        },
        WriteCase {
            case: "an update naming one granted attribute beside an ungranted one",
            operation: "UpdateUserAttributes",
            attributes: &["email", "name"],
            authorized: false,
        },
        WriteCase {
            case: "a deletion naming a granted attribute",
            operation: "DeleteUserAttributes",
            attributes: &["given_name"],
            authorized: true,
        },
        WriteCase {
            case: "a deletion naming an attribute outside the grant",
            operation: "DeleteUserAttributes",
            attributes: &["name"],
            authorized: false,
        },
    ]
}

/// The value every authorized update in the write table writes. One value for
/// every attribute a row names, so it has to be acceptable as each of them: an
/// address form, which `email` requires and a name attribute admits.
const WRITTEN_VALUE: &str = "written@example.test";

#[tokio::test]
async fn a_write_outside_the_app_clients_grant_is_refused_and_stores_nothing() {
    for row in write_cases() {
        let case = row.case;
        // One pool per row, so a row that WRITES leaves the next row's user as
        // the suite created it.
        let router = router();
        let user_pool_id = pool_with_user(&router).await;
        let client_id = client(
            &router,
            &user_pool_id,
            "write-client",
            None,
            Some(&WRITE_GRANT),
        )
        .await;
        let token = access_token(&router, &client_id).await;

        let body = match row.operation {
            "UpdateUserAttributes" => json!({
                "AccessToken": token,
                "UserAttributes": Value::Array(
                    row.attributes
                        .iter()
                        .map(|name| json!({ "Name": name, "Value": WRITTEN_VALUE }))
                        .collect(),
                ),
            }),
            _ => json!({
                "AccessToken": token,
                "UserAttributeNames": row.attributes,
            }),
        };
        let (status, payload) = call(&router, row.operation, &body).await;

        if row.authorized {
            assert_eq!(
                status, 200,
                "case {case:?}: a write inside the grant must be served, got {status} with \
                 {payload}"
            );
            // A served write is observable: the administrative read reports what
            // the update set, and no value at all for what the deletion removed.
            for name in row.attributes {
                let held = stored_value(&router, &user_pool_id, name).await;
                let expected =
                    (row.operation == "UpdateUserAttributes").then(|| WRITTEN_VALUE.to_string());
                assert_eq!(
                    held, expected,
                    "case {case:?}: a served write must be what a later read of {name:?} reports, \
                     got {held:?}"
                );
            }
            continue;
        }

        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: a write outside the grant must be refused, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            "A client attempted to write unauthorized attribute",
            "case {case:?}: the refusal carries its own sentence, got {payload}"
        );

        // Refused before anything was written: every attribute the request
        // named, granted or not, still holds the value the create stored.
        for name in row.attributes {
            let held = stored_value(&router, &user_pool_id, name).await;
            let created = STORED_ATTRIBUTES
                .iter()
                .find(|(stored, _)| stored == name)
                .map(|(_, value)| (*value).to_string());
            assert_eq!(
                held, created,
                "case {case:?}: a refused write must leave {name:?} as it was, got {held:?}"
            );
        }
    }
}
