// SPDX-License-Identifier: CC0-1.0

//! The alias binding lifecycle, end to end over the wire: `CreateAlias`, `UpdateAlias`,
//! `GetAlias`, `DeleteAlias` and the `KeyIdentifier` forms that resolve through an alias.
//!
//! The rule under test: an alias is a name in the account and Region that binds AT MOST
//! ONE key, and every operation answers from the one stored binding — `UpdateAlias`
//! rebinds the name to the key the request names, or removes the binding when it names
//! none ("the key that you are updating or removing from the alias", `UpdateAliasInput`'s
//! own documentation), while the alias itself stays until `DeleteAlias` removes it; the
//! keys themselves are untouched by any of it.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with `X-Amz-Target:
//! PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the modelled-constraint walk,
//! through the same harness `key_lifecycle` uses, so what these tests assert is what a
//! signed SDK request observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router, string, REGION};

/// A `CreateKey` body for one symmetric data-encryption key: the attribute set this
/// service's own documentation pairs with `AES_128`.
fn key_body() -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": "AES_128",
            "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
        },
        "Exportable": true,
    })
}

/// Create one key and answer its `KeyArn`.
async fn fresh_key_arn(router: &MultiServiceRouter) -> String {
    let created = ok_call(router, "CreateKey", &key_body()).await;
    string(
        created.get("Key").expect("CreateKey answers a Key"),
        "KeyArn",
    )
    .to_string()
}

/// The `Alias` an answer carries.
fn alias_of(case: &str, answered: &Value) -> Value {
    answered
        .get("Alias")
        .unwrap_or_else(|| panic!("case {case}: no Alias in {answered}"))
        .clone()
}

/// Alias names spanning the class the modelled `AliasName` admits: its minimum and
/// maximum length, a nested name, and a name using every punctuation character the
/// pattern allows.
///
/// Source: `smithy_query(query="operation",operation="UpdateAlias")` — `AliasName`
/// carries `length 7..256` and `pattern ^alias/[a-zA-Z0-9/_-]+$`.
fn alias_names() -> Vec<String> {
    vec![
        "alias/u".to_string(),
        "alias/team/payments/pin_key-1".to_string(),
        "alias/UPPER-and-lower_09".to_string(),
        format!("alias/{}", "r".repeat(250)),
    ]
}

/// RULE: `UpdateAlias` moves the one stored binding, and every reader — `GetAlias`,
/// `ListAliases` and a `KeyIdentifier` naming the alias — answers from it.
///
/// The rows differ along the form of the alias name; each row drives the whole class of
/// updates: a rebind to another key, a rebind to the key the name already holds, and an
/// update naming no key at all, which removes the binding.
#[tokio::test]
async fn update_alias_moves_the_one_stored_binding() {
    let router = router();
    for name in alias_names() {
        let case = format!("{} chars: {name}", name.len());
        let first = fresh_key_arn(&router).await;
        let second = fresh_key_arn(&router).await;
        ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": first}),
        )
        .await;

        for (step, key_arn) in [("another key", &second), ("the same key", &second)] {
            let case = format!("{case}, rebound to {step}");
            let updated = ok_call(
                &router,
                "UpdateAlias",
                &json!({"AliasName": name, "KeyArn": key_arn}),
            )
            .await;
            let reported = alias_of(&case, &updated);
            assert_eq!(
                (string(&reported, "AliasName"), string(&reported, "KeyArn")),
                (name.as_str(), key_arn.as_str()),
                "case {case}: the update must answer the binding it stored"
            );
            // Read back through the implemented reader: the stored binding is the one
            // every later caller sees.
            let read = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
            assert_eq!(
                alias_of(&case, &read),
                reported,
                "case {case}: GetAlias must report the updated binding"
            );
            // And resolving a key through the alias reaches the key it now binds.
            let resolved = ok_call(&router, "GetKey", &json!({"KeyIdentifier": name})).await;
            assert_eq!(
                string(resolved.get("Key").expect("a Key"), "KeyArn"),
                key_arn.as_str(),
                "case {case}: the alias must resolve to the key it is bound to"
            );
        }

        // An update naming no key removes the binding and keeps the alias.
        let case = format!("{case}, unbound");
        let unbound = ok_call(&router, "UpdateAlias", &json!({"AliasName": name})).await;
        let reported = alias_of(&case, &unbound);
        assert_eq!(
            string(&reported, "AliasName"),
            name,
            "case {case}: the alias must still be named"
        );
        assert!(
            reported.get("KeyArn").is_none(),
            "case {case}: an unbound alias reports no KeyArn, got {reported}"
        );
        let read = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            alias_of(&case, &read),
            reported,
            "case {case}: GetAlias must report the alias as unbound"
        );
        // An alias bound to nothing resolves to no key.
        let (status, parsed) = call(&router, "GetKey", &json!({"KeyIdentifier": name})).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: an unbound alias resolves to no key, got {status} {parsed}"
        );

        // The keys themselves are untouched by any of it: both still answer by their own
        // ARNs.
        for key_arn in [&first, &second] {
            ok_call(&router, "GetKey", &json!({"KeyIdentifier": key_arn})).await;
        }
    }
}

/// RULE: an update of a name the account does not hold, or one naming a key it does not
/// hold, is the not-found class naming the identifier the request asked for — and writes
/// nothing.
#[tokio::test]
async fn update_alias_refuses_an_identifier_the_account_does_not_hold() {
    let router = router();
    let key_arn = fresh_key_arn(&router).await;
    let absent_key = format!(
        "arn:aws:payment-cryptography:{REGION}:{}:key/neverminted00000",
        engine::emulator_account_id()
    );
    // (case, the request, the identifier the refusal must name)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "an alias that was never created",
            json!({"AliasName": "alias/never-created", "KeyArn": key_arn}),
            "alias/never-created".to_string(),
        ),
        (
            "an alias that was never created, naming no key",
            json!({"AliasName": "alias/never-created"}),
            "alias/never-created".to_string(),
        ),
        (
            "a key that was never minted",
            json!({"AliasName": "alias/exists", "KeyArn": absent_key}),
            absent_key.clone(),
        ),
    ];
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/exists", "KeyArn": key_arn}),
    )
    .await;

    for (case, body, resource_id) in rows {
        let (status, parsed) = call(&router, "UpdateAlias", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected the not-found class, got {status} {parsed}"
        );
        assert_eq!(
            parsed.get("ResourceId").and_then(Value::as_str),
            Some(resource_id.as_str()),
            "case {case}: expected ResourceId {resource_id:?}, got {parsed}"
        );
    }

    // The refused updates stored nothing: the one alias that exists still binds the key
    // it was created with, and the name that does not exist still does not.
    let read = ok_call(&router, "GetAlias", &json!({"AliasName": "alias/exists"})).await;
    assert_eq!(
        string(&alias_of("after the refusals", &read), "KeyArn"),
        key_arn,
        "a refused update must leave the stored binding alone"
    );
    let (status, parsed) = call(
        &router,
        "GetAlias",
        &json!({"AliasName": "alias/never-created"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFoundException"),
        "a refused update must not create the alias it named, got {status} {parsed}"
    );
}
