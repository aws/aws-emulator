// SPDX-License-Identifier: CC0-1.0

//! The attribute claims of the identity token a sign-in issues.
//!
//! RULE: an identity token "contains claims about the identity of the authenticated user,
//! such as `name`, `email`, and `phone_number`" (`AuthenticationResultType$IdToken`) — the
//! user's STORED attributes, each claimed under the name it is stored under, a custom
//! attribute under the `custom:` prefix Amazon Cognito gives it
//! (`SchemaAttributeType$Name`: "when you add an attribute with a `Name` value of
//! `MyAttribute`, Amazon Cognito creates the custom attribute `custom:MyAttribute`") — and
//! the two attributes the standard schema declares `Boolean`
//! (`smithy_query(query="shape",shape="AttributeDataType")`) are claimed as the boolean each
//! states rather than as the text an attribute value is stored in. An attribute the user
//! does not hold is claimed by nothing, and the claims a token states about itself — its
//! subject, audience, issuer, lifetime and use — are the service's own.
//!
//! The rows differ along what the rule generalises over: whether the attribute is a standard
//! one or a custom one, and whether its schema entry declares it a string or a boolean —
//! including a standard attribute no issue cited (`given_name`, `phone_number_verified`).

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service and what names the
/// region its pool ids and ARNs are minted in. The signature is never checked here: the
/// suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
            Arc::new(engine::InMemoryEngine::new()),
        ),
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
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
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

/// A base64url segment's bytes, with the two alphabet substitutions and the padding
/// RFC 7515 §2 strips restored first.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// The claim set of a compact-serialized JWS.
fn jwt_claims(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a token must have three JWS segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// One row: the case, the attribute as the user holds it, and the claim the identity token
/// must state for it.
struct Row<'a> {
    case: &'a str,
    /// The attribute's stored name, which is also the claim's name.
    attribute: &'a str,
    /// The value the user is created with.
    stored: &'a str,
    /// The JSON the claim must carry.
    claimed: Value,
}

#[tokio::test]
async fn the_identity_token_claims_the_users_stored_attributes() {
    let router = router();
    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "attribute-claims",
            // A custom attribute, which the pool stores under the `custom:` prefix.
            "Schema": [
                {"Name": "dept", "AttributeDataType": "String", "Mutable": true},
            ],
        }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a pool id")
        .to_string();
    let client_id = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientName": "attribute-claims-app",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a client id")
        .to_string();

    let rows = vec![
        Row {
            case: "a standard string attribute",
            attribute: "email",
            stored: "ada@example.com",
            claimed: json!("ada@example.com"),
        },
        Row {
            case: "another standard string attribute, which no issue cited",
            attribute: "given_name",
            stored: "Ada",
            claimed: json!("Ada"),
        },
        Row {
            case: "a standard boolean attribute",
            attribute: "email_verified",
            stored: "true",
            claimed: json!(true),
        },
        Row {
            case: "a standard boolean attribute that is false",
            attribute: "phone_number_verified",
            stored: "false",
            claimed: json!(false),
        },
        Row {
            case: "a custom string attribute",
            attribute: "custom:dept",
            stored: "engineering",
            claimed: json!("engineering"),
        },
    ];

    // ONE user holding every row's attribute: the claims of one sign-in are what each row
    // reads, so the rule is asserted over the whole set at once.
    let attributes: Vec<Value> = rows
        .iter()
        .map(|row| json!({"Name": row.attribute, "Value": row.stored}))
        .collect();
    let created = ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": &user_pool_id,
            "Username": "ada",
            "UserAttributes": attributes,
        }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": &user_pool_id,
            "Username": "ada",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let signed_in = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientId": &client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": {"USERNAME": "ada", "PASSWORD": GOOD_PASSWORD},
        }),
    )
    .await;
    let result = &signed_in["AuthenticationResult"];
    let id_claims = jwt_claims(result["IdToken"].as_str().expect("an id token"));
    let access_claims = jwt_claims(result["AccessToken"].as_str().expect("an access token"));

    for row in &rows {
        let case = row.case;
        let claimed = id_claims.get(row.attribute).cloned();
        assert_eq!(
            claimed,
            Some(row.claimed.clone()),
            "case {case:?}: expected the identity token to claim {} as {}, got {claimed:?} \
             in {id_claims}",
            row.attribute,
            row.claimed
        );
        // The access token states the grant, not the user's profile: it carries no claim
        // for an attribute.
        assert!(
            access_claims.get(row.attribute).is_none(),
            "case {case:?}: the access token states no attribute claim, got {access_claims}"
        );
    }

    // An attribute the user does not hold is claimed by nothing.
    for absent in ["family_name", "custom:absent", "phone_number"] {
        assert!(
            id_claims.get(absent).is_none(),
            "case \"an attribute the user does not hold\": no claim must be stated for \
             {absent:?}, got {id_claims}"
        );
    }

    // The token's own reserved claims stand: its subject is the user's minted id, stated
    // once, and the stored `sub` attribute does not restate it as something else.
    assert_eq!(
        id_claims.get("sub"),
        Some(&json!(sub)),
        "case \"the token's own subject\": the sub claim is the stored user's id, got \
         {id_claims}"
    );
    assert_eq!(
        id_claims.get("aud"),
        Some(&json!(client_id)),
        "case \"the token's own audience\": the aud claim names the app client, got \
         {id_claims}"
    );
    assert_eq!(
        id_claims.get("token_use"),
        Some(&json!("id")),
        "case \"the token's own use\": an identity token says so, got {id_claims}"
    );
}
