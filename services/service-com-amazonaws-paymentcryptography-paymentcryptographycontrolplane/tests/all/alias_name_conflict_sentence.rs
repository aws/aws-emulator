// SPDX-License-Identifier: CC0-1.0

//! The refusal a taken alias name earns, end to end over the wire.
//!
//! The rule under test: an alias name is unique in the account and region, so a
//! `CreateAlias` naming a name the registry already holds is
//! refused with `ConflictException` (HTTP 409) and the service's duplicate-name
//! sentence — the fixed text `Alias `, the alias name exactly as the request supplied
//! it, and the fixed text ` already exists.`. The name is the only variable part, and
//! the rule does not depend on how the collision arose: a second create naming the
//! same key, one naming another key, and every loser of a race between callers that
//! all ask for one name earn the same sentence.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router, string};

/// A `CreateKey` body for one symmetric data-encryption key: the attribute set this
/// service's own documentation pairs with `AES_128`, as `key_lifecycle`'s algorithm
/// table does.
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
/// maximum length, a nested name, and a name using every punctuation character the
/// pattern allows.
///
/// Source: `smithy_query(query="operation",operation="CreateAlias")` — `AliasName`
/// carries `length 7..256` and `pattern ^alias/[a-zA-Z0-9/_-]+$`. None of these names
/// is the one the issue cited; the rule is the same for each.
fn alias_names() -> Vec<String> {
    vec![
        // The shortest name the 7..256 length admits.
        "alias/a".to_string(),
        "alias/pin-key-1".to_string(),
        "alias/team/payments/pin_key".to_string(),
        "alias/UPPER-and-lower_09".to_string(),
        // The longest name the 7..256 length admits.
        format!("alias/{}", "x".repeat(250)),
    ]
}

/// The one sentence a refused duplicate alias name earns.
fn expected_sentence(alias_name: &str) -> String {
    format!("Alias {alias_name} already exists.")
}

/// Assert `(status, body)` is the modelled duplicate-name refusal for `alias_name`.
fn assert_duplicate_name_refusal(case: &str, status: u16, body: &Value, alias_name: &str) {
    assert_eq!(
        status, 409,
        "case {case}: a taken alias name must be refused with 409, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case}: expected ConflictException, got {body}"
    );
    let expected = expected_sentence(alias_name);
    assert_eq!(
        body.get("Message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case}: expected the message {expected:?}, got {body}"
    );
}

#[tokio::test]
async fn a_taken_alias_name_is_refused_with_the_duplicate_name_sentence() {
    let router = router();
    for name in alias_names() {
        let case = format!("{} chars: {name}", name.len());
        let first = fresh_key_arn(&router).await;
        let second = fresh_key_arn(&router).await;

        let created = ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": first}),
        )
        .await;
        let bound = created
            .get("Alias")
            .unwrap_or_else(|| panic!("case {case}: CreateAlias answered no Alias: {created}"))
            .clone();

        // The name is taken, so binding it to another key is refused — with the
        // service's own sentence, the single signal a caller has for "this name is
        // already in use".
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": second}),
        )
        .await;
        assert_duplicate_name_refusal(&case, status, &parsed, &name);

        // The refused rebind changed nothing: the stored binding is still the first
        // key's.
        let after = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            after.get("Alias"),
            Some(&bound),
            "case {case}: a refused rebind must leave the stored binding alone"
        );

        // The rule does not depend on WHICH key the second request names: the repeat
        // of the identical create earns the same sentence, because the name — not the
        // binding — is what is already taken.
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": first}),
        )
        .await;
        assert_duplicate_name_refusal(&format!("{case}, identical repeat"), status, &parsed, &name);
        let after_repeat = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            after_repeat.get("Alias"),
            Some(&bound),
            "case {case}: a refused repeat must leave the stored binding alone"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_loser_of_a_race_for_one_alias_name_gets_the_duplicate_name_sentence() {
    // Rows differ along how many callers race for the one name, each with a key of
    // its own, and along the form of the name they contend for. One caller is the
    // degenerate row where nobody loses.
    let rows: Vec<(usize, String)> = vec![
        (1usize, "alias/race-1".to_string()),
        (2, "alias/race/two_callers".to_string()),
        (5, "alias/RACE-5".to_string()),
        (9, format!("alias/{}", "r".repeat(250))),
    ];
    for (racers, name) in rows {
        let case = format!("{racers} concurrent callers for {} chars", name.len());
        let router = Arc::new(router());
        let mut arns = Vec::with_capacity(racers);
        for _ in 0..racers {
            arns.push(fresh_key_arn(&router).await);
        }

        let mut running = Vec::with_capacity(racers);
        for arn in arns.clone() {
            let router = Arc::clone(&router);
            let name = name.clone();
            running.push(tokio::spawn(async move {
                let (status, body) = call(
                    &router,
                    "CreateAlias",
                    &json!({"AliasName": name, "KeyArn": arn}),
                )
                .await;
                (arn, status, body)
            }));
        }
        let mut outcomes = Vec::with_capacity(racers);
        for handle in running {
            outcomes.push(
                handle
                    .await
                    .unwrap_or_else(|err| panic!("case {case}: a caller panicked: {err}")),
            );
        }

        let winners: Vec<&String> = outcomes
            .iter()
            .filter(|(_, status, _)| *status == 200)
            .map(|(arn, _, _)| arn)
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "case {case}: exactly one caller may take the name, got {outcomes:?}"
        );
        for (arn, status, body) in outcomes.iter().filter(|(_, status, _)| *status != 200) {
            assert_duplicate_name_refusal(
                &format!("{case}, loser bound for {arn}"),
                *status,
                body,
                &name,
            );
        }

        // The name converged on the winner's key, and a re-read reports the same
        // binding: the losers wrote nothing.
        let winner = winners[0].clone();
        let mut read_back = BTreeSet::new();
        for _ in 0..2 {
            let fetched = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
            let alias = fetched
                .get("Alias")
                .unwrap_or_else(|| panic!("case {case}: GetAlias answered no Alias: {fetched}"));
            assert_eq!(
                string(alias, "AliasName"),
                name,
                "case {case}: GetAlias must report the contended name"
            );
            read_back.insert(string(alias, "KeyArn").to_string());
        }
        assert_eq!(
            read_back.into_iter().collect::<Vec<_>>(),
            vec![winner.clone()],
            "case {case}: the stored binding must be the winning racer's key {winner}"
        );
    }
}
