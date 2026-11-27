// SPDX-License-Identifier: CC0-1.0

//! The account an account-list preference key names, end to end over the awsJson1_0
//! wire.
//!
//! One rule: a preference key whose trailing `/`-separated segment is a 12-digit
//! account id NAMES that account, and `UpdateBillingPreferences` resolves it against
//! the caller's consolidated billing family whichever feature carries the key — an
//! account the family holds is stored and reported, an account outside it is refused
//! 403 `AccessDeniedException` and nothing is written. The model is what makes the
//! rule feature-independent: `BillingPreferenceSummary$accountId` is "Populated for
//! account-list keys" (not "for sharing keys"), beside the `accountName` it is
//! "Populated together with", and a display name exists only for an account of the
//! family
//! (`smithy_query(query="shape", shape="BillingPreferenceSummary")`).
//!
//! The rows span what the rule generalises over: which feature carries the key
//! (every updatable `BillingFeature`, the sharing ones and the ones that are not),
//! the key's prefix and segment count, which account the trailing segment names (the
//! family's payer account, a linked member of the family, an account in no family
//! this store holds), a trailing segment that is NO account id (one digit short, one
//! digit long, twelve characters with a letter), and how many keys one request
//! carries.
//!
//! The wire harness (`call`, `error_code`, `message`) and the credit builder are the
//! ones `billing_views.rs`, `rejection_fidelity.rs` and `credits.rs` own.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::{
    ComAmazonawsBillingAwsbillingState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, error_code, ACCOUNT_ID};
use crate::credits::{credit, seed};
use crate::rejection_fidelity::message;

/// The display name the payer account carries in the family.
const ACCOUNT_NAME: &str = service_com_amazonaws_billing_awsbilling::state::MANAGEMENT_ACCOUNT_NAME;

/// A second, linked account of the same consolidated billing family.
const MEMBER_ACCOUNT_ID: &str = "210987654321";

/// The display name that linked account carries.
const MEMBER_NAME: &str = "Linked Account";

/// A pattern-valid 12-digit account id that is in no consolidated billing family
/// this store holds.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// The one feature whose key form the `UpdateBillingPreferences` doc fixes:
/// `credit/{creditId}/{option}`.
const CREDIT_FEATURE: &str = "CREDIT_PREFERENCE_OPTIONS";

/// The credit the credit-feature rows name, owned by the account every request here
/// authenticates as, so those rows are about the account segment rather than about
/// the credit lookup (`tests/all/credit_preference_keys.rs` owns that one).
const CREDIT_ID: &str = "1";

/// The `BillingFeature` values `UpdateBillingPreferences` accepts: the `feature`
/// doc's "Valid values: BILLING_ALERTS, RI_SHARING, CREDIT_SHARING,
/// CREDIT_LEVEL_SHARING, CREDIT_PREFERENCE_OPTIONS".
const UPDATABLE_FEATURES: &[&str] = &[
    "BILLING_ALERTS",
    "RI_SHARING",
    "CREDIT_SHARING",
    "CREDIT_LEVEL_SHARING",
    CREDIT_FEATURE,
];

/// One router over ONE shared engine — what `core/router` builds in production, so
/// every call in a test observes the same state — holding a consolidated billing
/// family of two accounts and the credit [`CREDIT_ID`].
///
/// Every updatable feature is ACTIVATED on that engine
/// (`ComAmazonawsBillingAwsbillingState::grant_feature_access`): this suite is about
/// the account a key names, not about the per-feature access gate, which
/// `tests/all/billing_feature_access.rs` pins.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        for feature in UPDATABLE_FEATURES {
            state.grant_feature_access(feature);
        }
        state
            .consolidated_billing_family
            .insert(MEMBER_ACCOUNT_ID.to_string(), MEMBER_NAME.to_string());
    }
    seed(&engine, vec![credit(CREDIT_ID, ACCOUNT_ID)]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// `UpdateBillingPreferences` for one feature and one or more key/value pairs.
async fn update(
    router: &MultiServiceRouter,
    feature: &str,
    pairs: &[(&str, &str)],
) -> (u16, Value) {
    let keys: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": value }))
        .collect();
    call(
        router,
        "UpdateBillingPreferences",
        &json!({ "feature": feature, "billingPreferencesPerKey": keys }),
    )
    .await
}

/// What `GetBillingPreferences` reports for `feature`, as
/// `(key, value, accountId, accountName)` per entry.
async fn stored(
    router: &MultiServiceRouter,
    feature: &str,
) -> Vec<(String, String, Option<String>, Option<String>)> {
    let (status, body) = call(
        router,
        "GetBillingPreferences",
        &json!({ "features": [feature] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {feature:?}: the preference read must answer, got {status} {body:?}"
    );
    body["billingPreferences"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = |name: &str| entry[name].as_str().unwrap_or_default().to_string();
            let optional = |name: &str| entry.get(name).and_then(Value::as_str).map(str::to_string);
            (
                text("key"),
                text("value"),
                optional("accountId"),
                optional("accountName"),
            )
        })
        .collect()
}

/// The account-list keys this suite sends for `feature`, each naming `account` in its
/// trailing segment.
///
/// What comes BEFORE that segment is the caller's to choose: `PreferenceKey`'s
/// "Format depends on the feature" over `^[a-zA-Z0-9-\/\*: ]+$`
/// (`smithy_query(query="shape", shape="PreferenceKey")`), so the rows vary the
/// prefix and the segment count. [`CREDIT_FEATURE`] is the one feature whose form the
/// doc fixes, so under it the account is the option segment of a key naming the
/// seeded credit.
fn account_list_keys(feature: &str, account: &str) -> Vec<String> {
    if feature == CREDIT_FEATURE {
        return vec![format!("credit/{CREDIT_ID}/{account}")];
    }
    vec![
        account.to_string(),
        format!("account/{account}"),
        format!("linked/account/{account}"),
    ]
}

/// The accounts the rows name, with the display name the family holds for one and
/// `None` for an account outside it.
fn account_rows() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        (ACCOUNT_ID, Some(ACCOUNT_NAME)),
        (MEMBER_ACCOUNT_ID, Some(MEMBER_NAME)),
        (OUTSIDE_ACCOUNT_ID, None),
    ]
}

#[tokio::test]
async fn an_account_list_key_is_honoured_only_for_an_account_of_the_family() {
    for feature in UPDATABLE_FEATURES {
        for (account, name) in account_rows() {
            for key in account_list_keys(feature, account) {
                let (router, _engine) = emulator();
                let case = format!("{feature} {key}");
                let (status, body) = update(&router, feature, &[(key.as_str(), "ENABLED")]).await;
                let expected = if name.is_some() { 200 } else { 403 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
                );
                if let Some(name) = name {
                    assert_eq!(
                        stored(&router, feature).await,
                        vec![(
                            key.clone(),
                            "ENABLED".to_string(),
                            Some(account.to_string()),
                            Some(name.to_string())
                        )],
                        "case {case:?}: expected the key read back naming its account"
                    );
                    continue;
                }
                assert_eq!(
                    error_code(&body),
                    "AccessDeniedException",
                    "case {case:?}: expected the modeled refusal, got {body:?}"
                );
                assert_eq!(
                    message(&body),
                    format!("Account {account} is not a member of this consolidated billing family"),
                    "case {case:?}: expected the refusal to name the account, got {:?}",
                    message(&body)
                );
                assert_eq!(
                    stored(&router, feature).await,
                    Vec::new(),
                    "case {case:?}: a refused key stores nothing, got {:?}",
                    stored(&router, feature).await
                );
            }
        }
    }
}

#[tokio::test]
async fn a_trailing_segment_that_is_no_account_id_names_no_account() {
    // `AccountId` is `^[0-9]{12}$` (`smithy_query(query="shape",
    // shape="BillingPreferenceSummary", member="accountId")`): a trailing segment
    // outside that shape names no account at all, so there is nothing to resolve
    // against the family and nothing to report beside the key.
    let near_misses: Vec<String> = vec![
        // One digit short of an account id, one digit long, and twelve characters
        // with a letter among them.
        "99999999999".to_string(),
        "9999999999999".to_string(),
        "99999999999a".to_string(),
    ];
    for feature in UPDATABLE_FEATURES {
        for segment in &near_misses {
            let key = if *feature == CREDIT_FEATURE {
                format!("credit/{CREDIT_ID}/{segment}")
            } else {
                format!("alerts/{segment}")
            };
            let (router, _engine) = emulator();
            let case = format!("{feature} {key}");
            let (status, body) = update(&router, feature, &[(key.as_str(), "DISABLED")]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a key naming no account is stored, got {status} {body:?}"
            );
            assert_eq!(
                stored(&router, feature).await,
                vec![(key.clone(), "DISABLED".to_string(), None, None)],
                "case {case:?}: expected the key read back with no account reported"
            );
        }
    }
}

#[tokio::test]
async fn one_key_outside_the_family_refuses_the_whole_request() {
    // The whole request is validated before anything is written, so a batch that
    // names one account outside the family stores none of its keys — including the
    // ones that would have been honoured on their own.
    for feature in UPDATABLE_FEATURES {
        let honoured = account_list_keys(feature, ACCOUNT_ID)
            .into_iter()
            .next()
            .expect("every feature has an account-list key form");
        let refused = account_list_keys(feature, OUTSIDE_ACCOUNT_ID)
            .into_iter()
            .next()
            .expect("every feature has an account-list key form");
        let (router, _engine) = emulator();
        let (status, body) = update(
            &router,
            feature,
            &[(honoured.as_str(), "ENABLED"), (refused.as_str(), "ENABLED")],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {feature:?}: expected the batch refused, got {status} {body:?}"
        );
        assert_eq!(
            stored(&router, feature).await,
            Vec::new(),
            "case {feature:?}: a refused batch stores nothing, got {:?}",
            stored(&router, feature).await
        );
    }
}

#[tokio::test]
async fn the_rows_name_two_accounts_of_the_family_and_one_outside_it() {
    // What the rows above stand for, asserted against the store the harness builds,
    // so a typo in one of the id constants fails here instead of silently testing
    // another case. Every one of them is a modelled `AccountId` (`^[0-9]{12}$`), so
    // each is a key's account segment; what separates them is whether the family
    // holds it.
    let (_router, engine) = emulator();
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    for (account, name) in account_rows() {
        assert_eq!(
            account.len(),
            12,
            "case {account:?}: expected a 12-digit account id, got {account:?}"
        );
        assert!(
            account.bytes().all(|byte| byte.is_ascii_digit()),
            "case {account:?}: expected only digits, got {account:?}"
        );
        assert_eq!(
            state.family_account_name(account),
            name,
            "case {account:?}: expected the family name {name:?}, got {:?}",
            state.family_account_name(account)
        );
    }
}
