// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STRING alternative of a profile member the model also declares
//! as an enumerated type.
//!
//! The rule under test: `Profile$PartyTypeString` is documented as "An alternative to
//! PartyType which accepts any string as input" and `GenderString` is the same alternative
//! to `Gender` (`smithy_query(query="doc", shape="Profile", member="PartyTypeString")`), so
//! the two members are two renderings of ONE value a profile carries. A profile CREATED
//! with the value in its enumerated member therefore reports it in the string alternative
//! too, and one created with the string form reports exactly what was sent, with the
//! enumerated member left unset when the value is not one it admits.
//!
//! Every case is read back through `SearchProfiles`, so what is asserted is the projection
//! a client sees rather than the handler's stored record. The HTTP plumbing and the create
//! helpers are this crate's one test harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::Emulator;

/// The domain the profiles are created in.
const DOMAIN: &str = "alternativesdom";

/// The one profile `SearchProfiles` finds by the account number it was created with.
async fn found_by_account(emulator: &Emulator, account: &str) -> Value {
    let (status, body) = emulator
        .search(
            DOMAIN,
            json!({ "KeyName": "_account", "Values": [account] }),
            "",
        )
        .await;
    let items = body
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (status, items.len()),
        (200, 1),
        "account {account:?}: expected the search to find exactly one profile, \
         got {status} {body:?}"
    );
    items[0].clone()
}

/// RULE over the party type and the gender, with rows differing along WHICH member the
/// write stated the value in (the enumerated one, the string one, both) and whether the
/// value is one the enumerated type admits.
#[tokio::test]
async fn a_value_stated_in_an_enumerated_member_is_reported_in_its_string_alternative() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    // (case, the members the create states, the members the read reports)
    let rows: [(&str, Value, Value); 6] = [
        (
            "a party type stated in the enumerated member",
            json!({ "PartyType": "INDIVIDUAL" }),
            json!({ "PartyType": "INDIVIDUAL", "PartyTypeString": "INDIVIDUAL" }),
        ),
        (
            "a party type stated as a string the enumerated type does not admit",
            json!({ "PartyTypeString": "Sole Trader" }),
            json!({ "PartyTypeString": "Sole Trader" }),
        ),
        (
            "a party type stated in both members",
            json!({ "PartyType": "BUSINESS", "PartyTypeString": "Partner" }),
            json!({ "PartyType": "BUSINESS", "PartyTypeString": "Partner" }),
        ),
        (
            "a gender stated in the enumerated member",
            json!({ "Gender": "FEMALE" }),
            json!({ "Gender": "FEMALE", "GenderString": "FEMALE" }),
        ),
        (
            "a gender stated as a string the enumerated type does not admit",
            json!({ "GenderString": "nonbinary" }),
            json!({ "GenderString": "nonbinary" }),
        ),
        (
            "neither member stated",
            json!({}),
            json!({}),
        ),
    ];

    for (index, (case, stated, reported)) in rows.into_iter().enumerate() {
        let account = format!("ACCT-{index}");
        let mut body = json!({ "AccountNumber": account.clone() });
        for (member, value) in stated.as_object().expect("the stated members are an object") {
            body[member] = value.clone();
        }
        emulator.create_profile(DOMAIN, body).await;
        let found = found_by_account(&emulator, &account).await;

        for member in ["PartyType", "PartyTypeString", "Gender", "GenderString"] {
            let expected = reported.get(member);
            let actual = found.get(member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} to be reported as {expected:?}, \
                 got {actual:?} (whole item {found:?})"
            );
        }
    }
}
