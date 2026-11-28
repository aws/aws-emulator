// SPDX-License-Identifier: CC0-1.0

//! The shared-pool delivery mode, at the account and at a configuration set, end to end
//! over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: a shared-pool delivery mode is a PERSISTED setting of the
//! resource the write addresses — `PutAccountDeliveryAttributes` replaces the account's,
//! `PutConfigurationSetDeliveryOptions` and `CreateConfigurationSet` replace one
//! configuration set's — so the read of that resource reports the mode last written to
//! it, the two resources' modes are independent of each other, a read changes neither,
//! and a value outside the member's own modelled value set is refused with
//! `BadRequestException`, leaving the stored mode as it was.
//!
//! The tables span what the rule generalises over: every value of each of the two value
//! sets (the account's `AccountSharedPoolDeliveryMode` and the set's wider
//! `SharedPoolDeliveryMode`), each written alone and beside the other members of the
//! structure that carries it, written through each operation that writes it, read back
//! through the operation that reports it, and each value set's refusal legs — including
//! the value only the OTHER value set holds.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES, SHARED_POOL_DELIVERY_MODE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::SHARED_POOL_DELIVERY_MODE_GLOBAL;

use crate::configuration_sets::{Reply, Ses};

/// `PutAccountDeliveryAttributes`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="PutAccountDeliveryAttributes")`:
/// `POST /v2/email/account/delivery`).
const ACCOUNT_DELIVERY_PATH: &str = "/v2/email/account/delivery";

/// `GetAccount`'s route, which reads the stored attributes back.
const ACCOUNT_PATH: &str = "/v2/email/account";

/// Write the account's shared-pool delivery mode.
async fn put_account_mode(ses: &Ses, mode: &str) -> Reply {
    ses.call(
        "POST",
        ACCOUNT_DELIVERY_PATH,
        Some(json!({"SharedPoolDeliveryMode": mode})),
    )
    .await
}

/// The `DeliveryAttributes` the account reports, or `Value::Null` when it reports none.
async fn reported_account_attributes(ses: &Ses) -> Value {
    ses.call("GET", ACCOUNT_PATH, None)
        .await
        .member("DeliveryAttributes")
}

/// Write one configuration set's delivery options.
async fn put_set_delivery(ses: &Ses, set: &str, body: Value) -> Reply {
    ses.call(
        "PUT",
        &format!("/v2/email/configuration-sets/{set}/delivery-options"),
        Some(body),
    )
    .await
}

/// A configuration set the account holds, under `name`.
async fn create_set(ses: &Ses, name: &str, body: Value) {
    let created = ses.create_set(body).await;
    assert_eq!(
        created.status,
        200,
        "case {name:?}: expected 200 creating the configuration set, got {} {:?}",
        created.status,
        created.message()
    );
}

#[tokio::test]
async fn the_account_reports_the_shared_pool_delivery_mode_it_was_given() {
    // One row per value the account's own value set holds, so every published value is
    // covered rather than only the one a caller happened to send.
    for mode in ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES {
        let ses = Ses::new();

        // What an account nothing has configured delivers under.
        let initial = reported_account_attributes(&ses).await;
        assert_eq!(
            initial,
            json!({"SharedPoolDeliveryMode": SHARED_POOL_DELIVERY_MODE_GLOBAL}),
            "case {mode:?}: expected a fresh account to report the mode an account with no \
             sending history holds, got {initial:?}"
        );

        let written = put_account_mode(&ses, mode).await;
        assert_eq!(
            written.status,
            200,
            "case {mode:?}: expected 200 from PutAccountDeliveryAttributes, got {} {:?}",
            written.status,
            written.message()
        );
        assert!(
            written.is_empty_body(),
            "case {mode:?}: PutAccountDeliveryAttributesResponse declares no members, so the \
             body must be empty, got {:?}",
            written.member("DeliveryAttributes")
        );

        let expected = json!({"SharedPoolDeliveryMode": mode});
        let reported = reported_account_attributes(&ses).await;
        assert_eq!(
            reported, expected,
            "case {mode:?}: expected the account to report {expected:?}, got {reported:?}"
        );

        // Reading the account does not change what a later caller sees.
        let again = reported_account_attributes(&ses).await;
        assert_eq!(
            again, expected,
            "case {mode:?}: expected a second read to report {expected:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn each_write_replaces_the_mode_the_account_held() {
    let ses = Ses::new();

    // Every value, then every value again in the reverse order: each step covers a
    // change to a different value and a write of the value already held.
    let steps: Vec<&str> = ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES
        .iter()
        .chain(ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES.iter().rev())
        .copied()
        .collect();

    for (step, mode) in steps.into_iter().enumerate() {
        let case = format!("step {step}: {mode}");
        let written = put_account_mode(&ses, mode).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: expected 200 from PutAccountDeliveryAttributes, got {} {:?}",
            written.status,
            written.message()
        );
        let reported = reported_account_attributes(&ses).await;
        assert_eq!(
            reported,
            json!({"SharedPoolDeliveryMode": mode}),
            "case {case:?}: expected the account to report the mode this step wrote, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_mode_outside_the_accounts_value_set_is_refused_and_changes_nothing() {
    // The value only a CONFIGURATION SET's wider value set holds, which the account's
    // does not, plus values differing from a published one only in case, by surrounding
    // space or by being absent altogether.
    let set_only: Vec<&str> = SHARED_POOL_DELIVERY_MODE_VALUES
        .iter()
        .copied()
        .filter(|mode| !ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES.contains(mode))
        .collect();
    let mut rows: Vec<String> = set_only.iter().map(|mode| (*mode).to_string()).collect();
    rows.extend(
        [
            "global",
            "Regional",
            "",
            " GLOBAL",
            "GLOBAL ",
            "GLOBAL_SHARED",
        ]
        .map(str::to_string),
    );

    // The mode the account is left holding, which every refusal below must keep. It is
    // the value the fresh account does NOT hold, so the assertion shows the refusal kept
    // what was written rather than that nothing had been configured.
    let held = ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode != SHARED_POOL_DELIVERY_MODE_GLOBAL)
        .expect("the account's value set holds a value beside the default");

    for case in rows {
        let ses = Ses::new();
        let prepared = put_account_mode(&ses, held).await;
        assert_eq!(
            prepared.status, 200,
            "case {case:?}: expected 200 while preparing the account, got {}",
            prepared.status
        );

        let refused = put_account_mode(&ses, &case).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to name what was invalid, got an empty message"
        );

        let reported = reported_account_attributes(&ses).await;
        assert_eq!(
            reported,
            json!({"SharedPoolDeliveryMode": held}),
            "case {case:?}: expected the refused write to leave the mode {held:?} in place, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_configuration_set_reports_the_delivery_mode_it_was_configured_with() {
    // One row per value the SET's value set holds — one more than the account's — each
    // written alone and then beside the other members of the same structure, through
    // each of the two operations that write a set's delivery configuration.
    for mode in SHARED_POOL_DELIVERY_MODE_VALUES {
        let ses = Ses::new();

        let alone = format!("put-{}", mode.to_lowercase());
        create_set(&ses, &alone, json!({"ConfigurationSetName": alone})).await;
        let before = ses.get_set(&alone).await.member("DeliveryOptions");
        assert_eq!(
            before,
            Value::Null,
            "case {alone:?}: a set nobody configured delivery for must report no \
             DeliveryOptions, got {before:?}"
        );
        let written = put_set_delivery(&ses, &alone, json!({"SharedPoolDeliveryMode": mode})).await;
        assert_eq!(
            written.status,
            200,
            "case {alone:?}: expected 200 from PutConfigurationSetDeliveryOptions, got {} {:?}",
            written.status,
            written.message()
        );
        let reported = ses.get_set(&alone).await.member("DeliveryOptions");
        assert_eq!(
            reported,
            json!({"SharedPoolDeliveryMode": mode}),
            "case {alone:?}: expected the set to report only the mode the write carried, got \
             {reported:?}"
        );

        // Beside the other members of the structure: the mode is one member of the set's
        // delivery configuration, not a configuration of its own.
        let beside = format!("put-beside-{}", mode.to_lowercase());
        create_set(&ses, &beside, json!({"ConfigurationSetName": beside})).await;
        let written = put_set_delivery(
            &ses,
            &beside,
            json!({
                "SharedPoolDeliveryMode": mode,
                "TlsPolicy": "REQUIRE",
                "MaxDeliverySeconds": 300,
            }),
        )
        .await;
        assert_eq!(
            written.status,
            200,
            "case {beside:?}: expected 200 from PutConfigurationSetDeliveryOptions, got {} {:?}",
            written.status,
            written.message()
        );
        let reported = ses.get_set(&beside).await.member("DeliveryOptions");
        assert_eq!(
            reported,
            json!({
                "TlsPolicy": "REQUIRE",
                "MaxDeliverySeconds": 300,
                "SharedPoolDeliveryMode": mode,
            }),
            "case {beside:?}: expected the set to report every member the write carried, got \
             {reported:?}"
        );

        // The create carries the same structure, through the same stored projection.
        let created = format!("create-{}", mode.to_lowercase());
        create_set(
            &ses,
            &created,
            json!({
                "ConfigurationSetName": created,
                "DeliveryOptions": {"SharedPoolDeliveryMode": mode},
            }),
        )
        .await;
        let reported = ses.get_set(&created).await.member("DeliveryOptions");
        assert_eq!(
            reported,
            json!({"SharedPoolDeliveryMode": mode}),
            "case {created:?}: expected the created set to report the mode the create carried, \
             got {reported:?}"
        );

        // A later write states no member at all, which leaves the set carrying no
        // delivery configuration — the mode included.
        let cleared = put_set_delivery(&ses, &created, json!({})).await;
        assert_eq!(
            cleared.status,
            200,
            "case {created:?}: expected 200 from a write carrying no members, got {} {:?}",
            cleared.status,
            cleared.message()
        );
        let reported = ses.get_set(&created).await.member("DeliveryOptions");
        assert_eq!(
            reported,
            Value::Null,
            "case {created:?}: expected a write carrying no members to leave the set with no \
             DeliveryOptions, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_sets_delivery_mode_and_the_accounts_are_independent() {
    for set_mode in SHARED_POOL_DELIVERY_MODE_VALUES {
        for account_mode in ACCOUNT_SHARED_POOL_DELIVERY_MODE_VALUES {
            let case = format!("set {set_mode}, account {account_mode}");
            let ses = Ses::new();
            let set = "independent";
            create_set(&ses, set, json!({"ConfigurationSetName": set})).await;

            let body = json!({"SharedPoolDeliveryMode": set_mode});
            let written = put_set_delivery(&ses, set, body).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 writing the set's delivery options, got {}",
                written.status
            );
            // Writing a SET's delivery configuration configures that set, so the account
            // is left delivering under the mode it held.
            let account = reported_account_attributes(&ses).await;
            assert_eq!(
                account,
                json!({"SharedPoolDeliveryMode": SHARED_POOL_DELIVERY_MODE_GLOBAL}),
                "case {case:?}: expected the set's write to leave the account's mode alone, got \
                 {account:?}"
            );

            let written = put_account_mode(&ses, account_mode).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 writing the account's mode, got {}",
                written.status
            );
            // And writing the ACCOUNT's mode configures the account, so the set keeps the
            // mode it was configured with — including the value that defers to the
            // account's.
            let reported = ses.get_set(set).await.member("DeliveryOptions");
            assert_eq!(
                reported,
                json!({"SharedPoolDeliveryMode": set_mode}),
                "case {case:?}: expected the set to keep the mode it was configured with, got \
                 {reported:?}"
            );
            let account = reported_account_attributes(&ses).await;
            assert_eq!(
                account,
                json!({"SharedPoolDeliveryMode": account_mode}),
                "case {case:?}: expected the account to report the mode it was given, got \
                 {account:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_mode_outside_the_sets_value_set_is_refused_and_changes_nothing() {
    for case in ["account", "Global", "", " REGIONAL", "SHARED"] {
        let ses = Ses::new();
        let set = "refusal";
        create_set(&ses, set, json!({"ConfigurationSetName": set})).await;

        // A known delivery configuration to refuse against.
        let held = json!({"SharedPoolDeliveryMode": "GLOBAL", "TlsPolicy": "REQUIRE"});
        let prepared = put_set_delivery(&ses, set, held.clone()).await;
        assert_eq!(
            prepared.status, 200,
            "case {case:?}: expected 200 while preparing the set, got {}",
            prepared.status
        );

        let refused = put_set_delivery(&ses, set, json!({"SharedPoolDeliveryMode": case})).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to name what was invalid, got an empty message"
        );

        let reported = ses.get_set(set).await.member("DeliveryOptions");
        assert_eq!(
            reported,
            json!({"TlsPolicy": "REQUIRE", "SharedPoolDeliveryMode": "GLOBAL"}),
            "case {case:?}: expected the refused write to leave the set's delivery configuration \
             in place, got {reported:?}"
        );
    }
}
