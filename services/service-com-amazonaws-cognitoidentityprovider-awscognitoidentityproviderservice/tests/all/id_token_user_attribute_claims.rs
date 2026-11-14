// SPDX-License-Identifier: CC0-1.0

//! What the IDENTITY token a pool issues claims about its user.
//!
//! RULE: an identity token "contains claims about the identity of the authenticated user,
//! such as `name`, `family_name`, and `phone_number`"
//! (`AuthenticationResultType$IdToken`, this service's model) — so the user's stored
//! ATTRIBUTES are those claims: each travels under the name the pool stores it under, which
//! for a custom attribute is already its `custom:` name, and the two verification flags
//! travel as the boolean each states rather than as the text an attribute value is carried
//! in. The claims the sign-in MINTS itself are the sign-in's own and are never taken from an
//! attribute of the same name. The ACCESS token beside it carries no user attribute.
//!
//! The rows differ along what the rule generalises over: whether the attribute is standard or
//! custom, whether its claim is text or the boolean a verification flag states, and whether
//! its name is one the sign-in mints for itself.
//!
//! This is the claim set an attribute-mapped consumer of the token reads — an identity pool's
//! principal-tag attribute map among them — which is why it is pinned here rather than only
//! where one such consumer reads it.
//!
//! The harness is `admin_user_pool_lifecycle`'s: this service alone behind the same
//! `MultiServiceRouter` the production router builds, driven over the wire.

use serde_json::{json, Value};

use crate::admin_user_pool_lifecycle::{jwt_claims, ok, router, GOOD_PASSWORD};

/// One row: the attribute as the pool declares it and the user stores it, and the claim the
/// identity token must then carry.
struct Row<'a> {
    case: &'a str,
    /// The schema entry the pool declares, for a custom attribute; `None` for a standard one,
    /// which every pool declares already.
    declared: Option<Value>,
    /// The attribute name as the request names it.
    name: &'a str,
    /// The value as the request carries it — always text, as `AttributeType$Value` is.
    value: &'a str,
    /// The claim's value.
    expected: Value,
}

#[tokio::test]
async fn an_identity_token_claims_the_users_own_attributes() {
    let router = router();

    let rows: &[Row<'_>] = &[
        Row {
            case: "a standard attribute whose value is text",
            declared: None,
            name: "email",
            value: "alice@example.com",
            expected: json!("alice@example.com"),
        },
        Row {
            case: "the email verification flag, whose claim is the boolean it states",
            declared: None,
            name: "email_verified",
            value: "true",
            expected: json!(true),
        },
        Row {
            case: "another standard attribute, read on a second name",
            declared: None,
            name: "given_name",
            value: "Alice",
            expected: json!("Alice"),
        },
        Row {
            case: "a custom attribute, claimed under its prefixed name",
            declared: Some(json!({ "Name": "dept", "AttributeDataType": "String" })),
            name: "custom:dept",
            value: "eng",
            expected: json!("eng"),
        },
        Row {
            case: "a custom attribute whose value carries unicode and punctuation",
            declared: Some(json!({ "Name": "team", "AttributeDataType": "String" })),
            name: "custom:team",
            value: "équipe/platform=1",
            expected: json!("équipe/platform=1"),
        },
    ];

    // One pool declaring every custom attribute the rows name, and one user holding every
    // value: the whole table is read off ONE issued token, so the claim set is asserted as the
    // one document a consumer reads rather than per row.
    let schema: Vec<Value> = rows.iter().filter_map(|row| row.declared.clone()).collect();
    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "claimed-attributes", "Schema": schema }),
    )
    .await["UserPool"]
        .clone();
    let user_pool_id = pool["Id"].as_str().expect("a pool id").to_string();
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientName": "claims-reader",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]
        .clone();
    let client_id = client["ClientId"].as_str().expect("a client id").to_string();

    let attributes: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "Name": row.name, "Value": row.value }))
        .collect();
    let created = ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": &user_pool_id,
            "Username": "alice",
            "UserAttributes": attributes,
        }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|attribute| attribute["Name"] == json!("sub"))
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("the created user carries its minted sub")
        .to_string();
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": &user_pool_id,
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
            "UserPoolId": &user_pool_id,
            "ClientId": &client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let result = &signed_in["AuthenticationResult"];
    let id_claims = jwt_claims(result["IdToken"].as_str().expect("an id token"));
    let access_claims = jwt_claims(result["AccessToken"].as_str().expect("an access token"));

    for row in rows {
        let case = row.case;
        let actual = id_claims.get(row.name).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual, row.expected,
            "case {case:?}: the identity token must claim {} as {}, got {actual}",
            row.name, row.expected
        );
        assert!(
            access_claims.get(row.name).is_none(),
            "case {case:?}: the access token carries no user attribute, got {access_claims}"
        );
    }

    // The claims the sign-in MINTS are its own, not attribute values: the store holds the
    // user's `sub` as an attribute too, and the claim is the token's.
    assert_eq!(
        id_claims["sub"],
        json!(sub),
        "case \"a minted claim\": the sub claim is the sign-in's own, got {id_claims}"
    );
    assert_eq!(
        id_claims["cognito:username"],
        json!("alice"),
        "case \"a minted claim\": the username claim is the sign-in's own, got {id_claims}"
    );
    assert_eq!(
        id_claims["token_use"],
        json!("id"),
        "case \"a minted claim\": an identity token says so, got {id_claims}"
    );
}
