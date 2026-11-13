// SPDX-License-Identifier: CC0-1.0

//! What the alias registry says exists, end to end over the wire.
//!
//! The rules under test, one per cause of the issue this suite was written for:
//!
//! 1. An alias name is unique in the account and region, so `CreateAlias` refuses a
//!    name the registry already holds — whatever key the second request names — with
//!    `ConflictException` and the duplicate-name sentence, and the stored binding is
//!    left exactly as it was.
//! 2. An alias exists exactly while the registry holds its name: `DeleteAlias` drops
//!    the binding, so every later read of that name (`GetAlias`, `ListAliases`, a
//!    `KeyIdentifier` resolving through it) and every later `DeleteAlias` sees a name
//!    that no longer exists, and the name is free for a new `CreateAlias`. The key the
//!    alias named is untouched.
//!
//! A name the registry does not hold is refused with `ResourceNotFoundException`,
//! whose `ResourceId` is the alias name the request asked for — the identifier that
//! could not be located.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router, string};

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

/// Alias names spanning the class the modelled `AliasName` admits: its minimum and
/// maximum length, a nested name, a name using every punctuation character the
/// pattern allows, and a digits-only name.
///
/// Source: `smithy_query(query="operation",operation="DeleteAlias")` — `AliasName`
/// carries `length 7..256` and `pattern ^alias/[a-zA-Z0-9/_-]+$`. None of these names
/// is the one the issue cited; the rules are the same for each.
fn alias_names() -> Vec<String> {
    vec![
        // The shortest name the 7..256 length admits.
        "alias/z".to_string(),
        "alias/0123456789".to_string(),
        "alias/estate/pin/verify_key-1".to_string(),
        "alias/Mixed_Case-and/slashes".to_string(),
        // The longest name the 7..256 length admits.
        format!("alias/{}", "d".repeat(250)),
    ]
}

/// Assert `(status, body)` is the modelled not-found refusal naming `alias_name`.
fn assert_unknown_alias_refusal(case: &str, status: u16, body: &Value, alias_name: &str) {
    assert_eq!(
        status, 400,
        "case {case}: an alias the registry does not hold must be refused with this \
         service's client-error status, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body.get("ResourceId").and_then(Value::as_str),
        Some(alias_name),
        "case {case}: ResourceId must be the alias name that could not be located, got {body}"
    );
}

/// Every alias name `ListAliases` reports, over every page.
async fn listed_alias_names(router: &MultiServiceRouter) -> Vec<String> {
    let mut names = Vec::new();
    let mut body = json!({"MaxResults": 100});
    loop {
        let page = ok_call(router, "ListAliases", &body).await;
        let aliases = page
            .get("Aliases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("ListAliases answers an Aliases list: {page}"));
        for alias in aliases {
            names.push(string(alias, "AliasName").to_string());
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => body = json!({"MaxResults": 100, "NextToken": token}),
            None => return names,
        }
    }
}

#[tokio::test]
async fn deleting_an_alias_removes_it_and_frees_its_name() {
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
        let live = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            string(
                live.get("Alias")
                    .unwrap_or_else(|| panic!("case {case}: GetAlias answered no Alias: {live}")),
                "KeyArn"
            ),
            first,
            "case {case}: the created alias must resolve to the key it was bound to"
        );
        assert!(
            listed_alias_names(&router).await.contains(&name),
            "case {case}: a live alias must be listed"
        );

        let deleted = ok_call(&router, "DeleteAlias", &json!({"AliasName": name})).await;
        assert!(
            deleted.is_null() || deleted.as_object().is_some_and(|body| body.is_empty()),
            "case {case}: DeleteAlias answers an empty output, got {deleted}"
        );

        // The alias is gone: no read resolves it, and `ListAliases` no longer carries
        // it.
        let (status, parsed) = call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_unknown_alias_refusal(&format!("{case}, read after delete"), status, &parsed, &name);
        assert!(
            !listed_alias_names(&router).await.contains(&name),
            "case {case}: a deleted alias must not be listed"
        );
        let (status, parsed) = call(&router, "GetKey", &json!({"KeyIdentifier": name})).await;
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {case}: a deleted alias must resolve no key, got {status} {parsed}"
        );

        // Deleting the alias did not touch the key it named.
        let key = ok_call(&router, "GetKey", &json!({"KeyIdentifier": first})).await;
        assert_eq!(
            string(key.get("Key").expect("GetKey answers a Key"), "KeyArn"),
            first,
            "case {case}: DeleteAlias must leave the underlying key in place"
        );

        // Deleting it again finds nothing to delete.
        let (status, parsed) = call(&router, "DeleteAlias", &json!({"AliasName": name})).await;
        assert_unknown_alias_refusal(
            &format!("{case}, second delete"),
            status,
            &parsed,
            &name,
        );

        // And the name is free again — for a different key, since the binding that
        // held it is gone.
        let recreated = ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": second}),
        )
        .await;
        assert_eq!(
            string(
                recreated.get("Alias").unwrap_or_else(|| panic!(
                    "case {case}: CreateAlias answered no Alias: {recreated}"
                )),
                "KeyArn"
            ),
            second,
            "case {case}: a deleted name must be free to bind to another key"
        );
        let after = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            string(after.get("Alias").expect("an Alias"), "KeyArn"),
            second,
            "case {case}: the re-created alias must resolve to the newly bound key"
        );
    }
}

#[tokio::test]
async fn a_name_the_registry_never_held_is_refused_by_both_readers_and_the_delete() {
    let router = router();
    // Rows span the same name class as the live cases, none of them ever created.
    for name in alias_names() {
        let case = format!("{} chars: {name}", name.len());
        for operation in ["GetAlias", "DeleteAlias"] {
            let (status, parsed) = call(&router, operation, &json!({"AliasName": name})).await;
            assert_unknown_alias_refusal(
                &format!("{case}, {operation}"),
                status,
                &parsed,
                &name,
            );
        }
    }
}

#[tokio::test]
async fn deleting_one_alias_of_a_key_leaves_the_key_s_other_aliases_bound() {
    let router = router();
    let arn = fresh_key_arn(&router).await;
    // A key can have multiple aliases, so rows differ along how many names the one
    // key carries: the lone alias, and one of several.
    for count in [1usize, 2, 4] {
        let names: Vec<String> = (0..count)
            .map(|index| format!("alias/shared-{count}-{index}"))
            .collect();
        for name in &names {
            ok_call(
                &router,
                "CreateAlias",
                &json!({"AliasName": name, "KeyArn": arn}),
            )
            .await;
        }

        let case = format!("{count} aliases on one key");
        let removed = names[0].clone();
        ok_call(&router, "DeleteAlias", &json!({"AliasName": removed})).await;

        let (status, parsed) = call(&router, "GetAlias", &json!({"AliasName": removed})).await;
        assert_unknown_alias_refusal(&format!("{case}, deleted"), status, &parsed, &removed);
        for survivor in names.iter().skip(1) {
            let fetched = ok_call(&router, "GetAlias", &json!({"AliasName": survivor})).await;
            assert_eq!(
                string(fetched.get("Alias").expect("an Alias"), "KeyArn"),
                arn,
                "case {case}: deleting {removed} must leave {survivor} bound"
            );
        }
    }
}
