// SPDX-License-Identifier: CC0-1.0

//! The two mutations a created key can take, end to end over the wire:
//! `StopKeyUsage` / `StartKeyUsage` and `AddKeyReplicationRegions` /
//! `RemoveKeyReplicationRegions`.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rule under test: a mutation writes the ONE stored key record, so every later
//! Key-returning read — `GetKey`, `ListKeys`, the next mutation's own answer —
//! reports what it wrote. For the usage switch that is `Enabled` and
//! `UsageStopTimestamp`; for replication it is `MultiRegionKeyType`,
//! `UsingDefaultReplicationRegions` and a `ReplicationStatus` entry per region whose
//! `Status` moves `IN_PROGRESS` -> `SYNCHRONIZED` -> `DELETE_IN_PROGRESS` as the
//! clock makes each step due.
//!
//! Each case is one row of a table spanning what the rule generalises over: the key
//! families this service generates, the identifier forms `KeyArnOrKeyAliasType`
//! admits, how many regions one call names, whether they overlap the configuration
//! already held, and both published bounds of a region set.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ops::types::{
    KEY_REPLICATION_STATE_VALUES, MULTI_REGION_KEY_TYPE_VALUES,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    MAX_KEY_REPLICATION_REGIONS, REPLICATION_SPAN,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{
    call, error_code, now_secs, ok_call, router, router_with_engine, string, timestamp,
};

/// The three regions the replication cases use.
///
/// Each matches the modelled `Region` pattern `^[a-z]{2}-[a-z]{1,16}-[0-9]+$`
/// (`smithy_query(query="shape",shape="Region")`), and none is the region the harness
/// signs for, so a replication region is never also the key's own.
const REGIONS: [&str; 3] = ["ap-southeast-2", "eu-west-1", "us-east-1"];

/// One row of the key-kind table: a key family this service generates, with the class,
/// usage and modes of use that describe one key of it.
///
/// Source: `smithy_query(query="shape",shape="KeyAlgorithm,KeyClass,KeyUsage")`. The
/// rules below are about a key record, not about one algorithm, so each is driven over
/// a symmetric block cipher, a double-length TDES key, a MAC key and an asymmetric
/// pair.
struct KeyKind {
    key_algorithm: &'static str,
    key_class: &'static str,
    key_usage: &'static str,
    modes: Value,
}

fn key_kinds() -> Vec<KeyKind> {
    vec![
        KeyKind {
            key_algorithm: "TDES_2KEY",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_P0_PIN_ENCRYPTION_KEY",
            modes: json!({"Encrypt": true, "Decrypt": true}),
        },
        KeyKind {
            key_algorithm: "AES_256",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
            modes: json!({"Wrap": true, "Unwrap": true}),
        },
        KeyKind {
            key_algorithm: "HMAC_SHA256",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: json!({"Generate": true, "Verify": true}),
        },
        KeyKind {
            key_algorithm: "RSA_2048",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
            modes: json!({"Sign": true, "Verify": true}),
        },
    ]
}

/// A `CreateKey` body for one key kind, with `extra` merged into it.
fn create_body(kind: &KeyKind, extra: Value) -> Value {
    let mut body = json!({
        "KeyAttributes": {
            "KeyUsage": kind.key_usage,
            "KeyClass": kind.key_class,
            "KeyAlgorithm": kind.key_algorithm,
            "KeyModesOfUse": kind.modes.clone(),
        },
        "Exportable": true,
    });
    if let (Value::Object(target), Value::Object(extra)) = (&mut body, extra) {
        target.extend(extra);
    }
    body
}

/// The `Key` of a response.
fn key_of<'a>(case: &str, body: &'a Value) -> &'a Value {
    body.get("Key")
        .unwrap_or_else(|| panic!("case {case}: no Key in {body}"))
}

/// Create one key of `kind` and answer `(the Key, its KeyArn)`.
async fn created_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    kind: &KeyKind,
    extra: Value,
) -> (Value, String) {
    let created = ok_call(router, "CreateKey", &create_body(kind, extra)).await;
    let key = key_of(case, &created).clone();
    let arn = string(&key, "KeyArn").to_string();
    (key, arn)
}

/// Region -> `ReplicationStatusType.Status`, as a `Key` reports it; empty when the
/// member is absent.
fn replication_status(case: &str, key: &Value) -> BTreeMap<String, String> {
    let Some(status) = key.get("ReplicationStatus") else {
        return BTreeMap::new();
    };
    status
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: ReplicationStatus is not a map in {key}"))
        .iter()
        .map(|(region, entry)| {
            (
                region.clone(),
                string(entry, "Status").to_string(),
            )
        })
        .collect()
}

/// The replication state every region in `regions` must report, as one map.
fn expected_status(regions: &[&str], status: &str) -> BTreeMap<String, String> {
    regions
        .iter()
        .map(|region| ((*region).to_string(), status.to_string()))
        .collect()
}

/// Move the instants this key's replication work started back by `by`, so a test can
/// observe a transition the clock owes without sleeping for it.
fn rewind_replication(engine: &engine::InMemoryEngine, arn: &str, by: Duration) {
    let state = engine.state::<KeyStore>();
    let mut store = state.lock();
    let key = store
        .keys
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store holds the key {arn}"));
    for replica in key.replication_regions.values_mut() {
        replica.replication_requested -= by;
        replica.removal_requested = replica.removal_requested.map(|requested| requested - by);
    }
}

/// The states and roles these tests name have to be the ones the model declares, so a
/// value the model renames fails here rather than going unnoticed.
#[test]
fn the_replication_states_and_roles_named_here_are_modelled() {
    for state in ["IN_PROGRESS", "SYNCHRONIZED", "DELETE_IN_PROGRESS"] {
        assert!(
            KEY_REPLICATION_STATE_VALUES.contains(&state),
            "case {state}: the model's KeyReplicationState values are \
             {KEY_REPLICATION_STATE_VALUES:?}"
        );
    }
    assert!(
        MULTI_REGION_KEY_TYPE_VALUES.contains(&"PRIMARY"),
        "the model's MultiRegionKeyType values are {MULTI_REGION_KEY_TYPE_VALUES:?}"
    );
}

#[tokio::test]
async fn stopping_and_starting_key_usage_is_what_every_later_read_reports() {
    let router = router();

    for kind in key_kinds() {
        let algorithm = kind.key_algorithm;
        // The rows differ along the key family and along whether the create asked for
        // the default enabled key or named `Enabled` itself.
        for enabled_at_create in [None, Some(true)] {
            let case = format!("{algorithm} created with Enabled={enabled_at_create:?}");
            let extra = match enabled_at_create {
                Some(enabled) => json!({"Enabled": enabled}),
                None => json!({}),
            };
            let (key, arn) = created_key(&router, &case, &kind, extra).await;
            assert_eq!(
                key.get("Enabled").and_then(Value::as_bool),
                Some(true),
                "case {case}: a created key is enabled: {key}"
            );
            assert!(
                key.get("UsageStopTimestamp").is_none(),
                "case {case}: nothing has stopped this key's usage yet: {key}"
            );

            // Stopping it disables the key and stamps the instant its material
            // stopped being used.
            let stopped = ok_call(&router, "StopKeyUsage", &json!({"KeyIdentifier": arn})).await;
            let stopped_key = key_of(&case, &stopped);
            assert_eq!(
                stopped_key.get("Enabled").and_then(Value::as_bool),
                Some(false),
                "case {case}: StopKeyUsage disables the key: {stopped}"
            );
            let stop_instant = timestamp(stopped_key, "UsageStopTimestamp");
            assert!(
                (stop_instant - now_secs()).abs() < 60.0,
                "case {case}: expected UsageStopTimestamp around {}, got {stop_instant}",
                now_secs()
            );

            // …and every later reader says the same thing, because the record is what
            // they all answer from.
            let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
            let fetched_key = key_of(&case, &fetched);
            assert_eq!(
                fetched_key.get("Enabled").and_then(Value::as_bool),
                Some(false),
                "case {case}: GetKey must report the disabled key: {fetched}"
            );
            assert_eq!(
                timestamp(fetched_key, "UsageStopTimestamp"),
                stop_instant,
                "case {case}: GetKey must report the stored UsageStopTimestamp: {fetched}"
            );
            let listed = ok_call(&router, "ListKeys", &json!({})).await;
            let summary = listed
                .get("Keys")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: no Keys in {listed}"))
                .iter()
                .find(|item| item.get("KeyArn").and_then(Value::as_str) == Some(arn.as_str()))
                .unwrap_or_else(|| panic!("case {case}: {arn} is not listed in {listed}"));
            assert_eq!(
                summary.get("Enabled").and_then(Value::as_bool),
                Some(false),
                "case {case}: the listed summary must report the disabled key: {summary}"
            );

            // Starting it again enables the key and clears the stop instant: nothing
            // stops its use any more.
            let started = ok_call(&router, "StartKeyUsage", &json!({"KeyIdentifier": arn})).await;
            let started_key = key_of(&case, &started);
            assert_eq!(
                started_key.get("Enabled").and_then(Value::as_bool),
                Some(true),
                "case {case}: StartKeyUsage enables the key: {started}"
            );
            assert!(
                started_key.get("UsageStopTimestamp").is_none(),
                "case {case}: a key in use again has no stop instant: {started}"
            );
            let usage_start = timestamp(started_key, "UsageStartTimestamp");
            assert!(
                usage_start >= stop_instant,
                "case {case}: expected usage to start at or after it stopped ({stop_instant}), \
                 got {usage_start}"
            );
            let reread = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
            let reread_key = key_of(&case, &reread);
            assert_eq!(
                reread_key.get("Enabled").and_then(Value::as_bool),
                Some(true),
                "case {case}: GetKey must report the enabled key: {reread}"
            );
            assert!(
                reread_key.get("UsageStopTimestamp").is_none(),
                "case {case}: GetKey must report no stop instant: {reread}"
            );
        }
    }
}

#[tokio::test]
async fn the_usage_switch_is_reached_through_every_identifier_form() {
    let router = router();
    let kinds = key_kinds();
    let case = "alias identifier";
    let (_, arn) = created_key(&router, case, &kinds[0], json!({})).await;
    let alias_name = "alias/stopped-by-alias";
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias_name, "KeyArn": arn}),
    )
    .await;

    // `KeyArnOrKeyAliasType` admits a key ARN and an alias, so the same stored key is
    // reached through either.
    let stopped = ok_call(
        &router,
        "StopKeyUsage",
        &json!({"KeyIdentifier": alias_name}),
    )
    .await;
    assert_eq!(
        string(key_of(case, &stopped), "KeyArn"),
        arn,
        "case {case}: the alias must resolve to the key it is bound to: {stopped}"
    );
    let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
    assert_eq!(
        key_of(case, &fetched).get("Enabled").and_then(Value::as_bool),
        Some(false),
        "case {case}: the key reached by alias must be disabled: {fetched}"
    );
    let started = ok_call(
        &router,
        "StartKeyUsage",
        &json!({"KeyIdentifier": alias_name}),
    )
    .await;
    assert_eq!(
        key_of(case, &started).get("Enabled").and_then(Value::as_bool),
        Some(true),
        "case {case}: the key reached by alias must be enabled again: {started}"
    );
}

#[tokio::test]
async fn the_usage_switch_refuses_a_key_that_is_not_active() {
    let router = router();
    let kinds = key_kinds();

    // A key the account does not hold, in each identifier form.
    let absent = [
        format!(
            "arn:aws:payment-cryptography:us-west-2:000000000000:key/{}",
            "a".repeat(16)
        ),
        "alias/nothing-here".to_string(),
    ];
    for operation in ["StopKeyUsage", "StartKeyUsage"] {
        for identifier in &absent {
            let case = format!("{operation} on {identifier}");
            let (status, parsed) =
                call(&router, operation, &json!({"KeyIdentifier": identifier})).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {parsed}"
            );
        }
    }

    // A key already on its way out is not active, so neither call may move it.
    for operation in ["StopKeyUsage", "StartKeyUsage"] {
        let case = format!("{operation} on a key pending deletion");
        let (_, arn) = created_key(&router, &case, &kinds[1], json!({})).await;
        ok_call(&router, "DeleteKey", &json!({"KeyIdentifier": arn})).await;
        let (status, parsed) = call(&router, operation, &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {case}: expected ConflictException, got {parsed}"
        );
        // The refused call stored nothing: the key is still the enabled key waiting to
        // be deleted.
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        let fetched_key = key_of(&case, &fetched);
        assert_eq!(
            fetched_key.get("Enabled").and_then(Value::as_bool),
            Some(true),
            "case {case}: a refused usage call must change nothing: {fetched}"
        );
        assert_eq!(
            string(fetched_key, "KeyState"),
            "DELETE_PENDING",
            "case {case}: the key must still be waiting to be deleted: {fetched}"
        );
    }
}

#[tokio::test]
async fn adding_replication_regions_is_what_every_later_read_reports() {
    let (router, engine) = router_with_engine();
    let kinds = key_kinds();
    let [first, second, third] = REGIONS;

    for kind in &kinds {
        let case = format!("{} replication", kind.key_algorithm);
        let (key, arn) = created_key(&router, &case, kind, json!({})).await;
        // Nothing has replicated this key, so it is not a Multi-Region key and it
        // follows the account's (empty) default configuration.
        assert!(
            key.get("MultiRegionKeyType").is_none() && key.get("ReplicationStatus").is_none(),
            "case {case}: an unreplicated key reports no multi-region role: {key}"
        );
        assert_eq!(
            key.get("UsingDefaultReplicationRegions")
                .and_then(Value::as_bool),
            Some(true),
            "case {case}: a key with no configuration of its own follows the defaults: {key}"
        );

        // The rows differ along how many regions one call names and whether they
        // overlap the configuration the key already holds.
        let additions: &[(&str, &[&str], &[&str])] = &[
            ("one region", &[first], &[first]),
            ("two more regions", &[second, third], &[first, second, third]),
            (
                "a region already replicating, plus one of them again",
                &[first, second],
                &[first, second, third],
            ),
        ];
        for (row, requested, held) in additions {
            let case = format!("{case}: {row}");
            let added = ok_call(
                &router,
                "AddKeyReplicationRegions",
                &json!({"KeyIdentifier": arn, "ReplicationRegions": requested}),
            )
            .await;
            let added_key = key_of(&case, &added);
            assert_eq!(
                added_key.get("MultiRegionKeyType").and_then(Value::as_str),
                Some("PRIMARY"),
                "case {case}: the key the regions were added to is the primary: {added}"
            );
            assert_eq!(
                added_key
                    .get("UsingDefaultReplicationRegions")
                    .and_then(Value::as_bool),
                Some(false),
                "case {case}: an explicit configuration overrides the account defaults: {added}"
            );
            let reported: BTreeSet<String> = replication_status(&case, added_key)
                .into_keys()
                .collect();
            let expected: BTreeSet<String> =
                held.iter().map(|region| (*region).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case}: expected one entry per configured region, got {added}"
            );
            // The regions this call named are still replicating: the span is not zero,
            // so the caller observes the work in flight.
            for region in *requested {
                let status = replication_status(&case, added_key);
                let state = status
                    .get(*region)
                    .unwrap_or_else(|| panic!("case {case}: no entry for {region} in {added}"));
                assert!(
                    state == "IN_PROGRESS" || state == "SYNCHRONIZED",
                    "case {case}: {region} must be replicating or settled, got {state}"
                );
            }

            // A read answers from the same record.
            let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
            let fetched_key = key_of(&case, &fetched);
            assert_eq!(
                replication_status(&case, fetched_key),
                replication_status(&case, added_key),
                "case {case}: GetKey must report the stored replication state: {fetched}"
            );
            assert_eq!(
                fetched_key.get("MultiRegionKeyType").and_then(Value::as_str),
                Some("PRIMARY"),
                "case {case}: GetKey must report the multi-region role: {fetched}"
            );
        }

        // Replication settles once the span the clock owes has elapsed — derived from
        // the stored instants, so moving them back is all it takes to reach it.
        rewind_replication(&engine, &arn, REPLICATION_SPAN);
        let settled = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        let settled_key = key_of(&case, &settled);
        assert_eq!(
            replication_status(&case, settled_key),
            expected_status(&[first, second, third], "SYNCHRONIZED"),
            "case {case}: every region must converge to SYNCHRONIZED: {settled}"
        );
        // …and the listed summary reports the same role.
        let listed = ok_call(&router, "ListKeys", &json!({})).await;
        let summary = listed
            .get("Keys")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: no Keys in {listed}"))
            .iter()
            .find(|item| item.get("KeyArn").and_then(Value::as_str) == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case}: {arn} is not listed in {listed}"));
        assert_eq!(
            summary.get("MultiRegionKeyType").and_then(Value::as_str),
            Some("PRIMARY"),
            "case {case}: the listed summary must report the multi-region role: {summary}"
        );
    }
}

#[tokio::test]
async fn removing_replication_regions_tears_down_only_those_replicas() {
    let (router, engine) = router_with_engine();
    let kinds = key_kinds();
    let [first, second, third] = REGIONS;
    let case = "remove one of three";

    let (_, arn) = created_key(
        &router,
        case,
        &kinds[2],
        json!({"ReplicationRegions": REGIONS}),
    )
    .await;
    rewind_replication(&engine, &arn, REPLICATION_SPAN);

    let removed = ok_call(
        &router,
        "RemoveKeyReplicationRegions",
        &json!({"KeyIdentifier": arn, "ReplicationRegions": [second]}),
    )
    .await;
    let removed_key = key_of(case, &removed);
    let mut expected = expected_status(&[first, third], "SYNCHRONIZED");
    expected.insert(second.to_string(), "DELETE_IN_PROGRESS".to_string());
    assert_eq!(
        replication_status(case, removed_key),
        expected,
        "case {case}: only the removed region's replica is torn down: {removed}"
    );

    // Once the teardown falls due the region is no longer one the key replicates to,
    // and the others are untouched.
    rewind_replication(&engine, &arn, REPLICATION_SPAN);
    let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
    assert_eq!(
        replication_status(case, key_of(case, &fetched)),
        expected_status(&[first, third], "SYNCHRONIZED"),
        "case {case}: the torn-down region must be gone: {fetched}"
    );

    // Removing every remaining region leaves a key that replicates nowhere, so it is
    // no longer a Multi-Region key — but it still has a configuration of its own.
    ok_call(
        &router,
        "RemoveKeyReplicationRegions",
        &json!({"KeyIdentifier": arn, "ReplicationRegions": [first, third]}),
    )
    .await;
    rewind_replication(&engine, &arn, REPLICATION_SPAN);
    let bare = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
    let bare_key = key_of(case, &bare);
    assert!(
        bare_key.get("ReplicationStatus").is_none() && bare_key.get("MultiRegionKeyType").is_none(),
        "case {case}: a key that replicates nowhere reports no replication state: {bare}"
    );
    assert_eq!(
        bare_key
            .get("UsingDefaultReplicationRegions")
            .and_then(Value::as_bool),
        Some(false),
        "case {case}: the key still has a configuration of its own: {bare}"
    );

    // A region the key does not replicate to describes a configuration it does not
    // have, and the refusal tears nothing down.
    let (_, other) = created_key(
        &router,
        "remove an unreplicated region",
        &kinds[0],
        json!({"ReplicationRegions": [first]}),
    )
    .await;
    let (status, parsed) = call(
        &router,
        "RemoveKeyReplicationRegions",
        &json!({"KeyIdentifier": other, "ReplicationRegions": [second]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case remove an unreplicated region: expected 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case remove an unreplicated region: expected ValidationException, got {parsed}"
    );
    let untouched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": other})).await;
    assert_eq!(
        replication_status("remove an unreplicated region", key_of("remove", &untouched)),
        expected_status(&[first], "IN_PROGRESS"),
        "case remove an unreplicated region: the refused call must tear nothing down: \
         {untouched}"
    );
}

#[tokio::test]
async fn deleting_a_multi_region_key_reports_its_replicas_tearing_down() {
    let (router, engine) = router_with_engine();
    let kinds = key_kinds();
    let [first, second, _] = REGIONS;

    // The rows differ along the waiting period the delete asks for: the default, and
    // both published bounds of `DeleteKeyInDays`
    // (`smithy_query(query="operation",operation="DeleteKey")` gives it
    // `range {min: 3, max: 180}`).
    for waiting_period in [None, Some(3), Some(180)] {
        let case = format!("DeleteKeyInDays={waiting_period:?}");
        let (_, arn) = created_key(
            &router,
            &case,
            &kinds[1],
            json!({"ReplicationRegions": [first, second]}),
        )
        .await;
        rewind_replication(&engine, &arn, REPLICATION_SPAN);

        let mut body = json!({"KeyIdentifier": arn});
        if let Some(days) = waiting_period {
            body["DeleteKeyInDays"] = json!(days);
        }
        let deleted = ok_call(&router, "DeleteKey", &body).await;
        let deleted_key = key_of(&case, &deleted);
        assert_eq!(
            deleted_key.get("MultiRegionKeyType").and_then(Value::as_str),
            Some("PRIMARY"),
            "case {case}: a deleted multi-region key keeps its role: {deleted}"
        );
        assert_eq!(
            deleted_key
                .get("UsingDefaultReplicationRegions")
                .and_then(Value::as_bool),
            Some(false),
            "case {case}: the key's own configuration still overrides the defaults: {deleted}"
        );
        assert_eq!(
            replication_status(&case, deleted_key),
            expected_status(&[first, second], "DELETE_IN_PROGRESS"),
            "case {case}: deleting the key tears down every replica: {deleted}"
        );

        // A later read reports the same teardown, so a caller verifying cleanup has
        // something to poll…
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            replication_status(&case, key_of(&case, &fetched)),
            expected_status(&[first, second], "DELETE_IN_PROGRESS"),
            "case {case}: GetKey must report the replicas tearing down: {fetched}"
        );

        // …and taking the key back restores them: nothing is tearing down any more.
        let restored = ok_call(&router, "RestoreKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            replication_status(&case, key_of(&case, &restored)),
            expected_status(&[first, second], "SYNCHRONIZED"),
            "case {case}: a restored key's replicas are no longer tearing down: {restored}"
        );
    }
}

#[tokio::test]
async fn a_replication_change_is_refused_outside_its_constraints() {
    let router = router();
    let kinds = key_kinds();
    let [first, _, third] = REGIONS;

    // A key the account does not hold.
    for operation in ["AddKeyReplicationRegions", "RemoveKeyReplicationRegions"] {
        let case = format!("{operation} on an absent key");
        let (status, parsed) = call(
            &router,
            operation,
            &json!({
                "KeyIdentifier": "alias/nothing-here",
                "ReplicationRegions": [first],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {parsed}"
        );
    }

    // A list naming no Region at all configures nothing.
    let (_, arn) = created_key(&router, "empty list", &kinds[0], json!({})).await;
    for operation in ["AddKeyReplicationRegions", "RemoveKeyReplicationRegions"] {
        let case = format!("{operation} with an empty list");
        let (status, parsed) = call(
            &router,
            operation,
            &json!({"KeyIdentifier": arn, "ReplicationRegions": []}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
    }

    // "The key must be in an active state to add Replication Regions"
    // (`AddKeyReplicationRegions`' own documentation).
    let (_, pending) = created_key(&router, "pending deletion", &kinds[0], json!({})).await;
    ok_call(&router, "DeleteKey", &json!({"KeyIdentifier": pending})).await;
    for operation in ["AddKeyReplicationRegions", "RemoveKeyReplicationRegions"] {
        let case = format!("{operation} on a key pending deletion");
        let (status, parsed) = call(
            &router,
            operation,
            &json!({"KeyIdentifier": pending, "ReplicationRegions": [first]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {case}: expected ConflictException, got {parsed}"
        );
    }

    // The configuration this emulator holds for one key is bounded: a call at the
    // bound is served, one past it is refused and stores nothing. Each generated name
    // matches the modelled `Region` pattern, and none of them is `past_bound` below.
    let (_, bounded) = created_key(&router, "at the bound", &kinds[0], json!({})).await;
    let past_bound = third;
    let at_bound: Vec<String> = (0..MAX_KEY_REPLICATION_REGIONS)
        .map(|n| format!("eu-west-{n}"))
        .collect();
    assert!(
        !at_bound.iter().any(|region| region == past_bound),
        "case past the bound: {past_bound} must not already be in the configuration at the \
         bound"
    );
    let served = ok_call(
        &router,
        "AddKeyReplicationRegions",
        &json!({"KeyIdentifier": bounded, "ReplicationRegions": at_bound}),
    )
    .await;
    assert_eq!(
        replication_status("at the bound", key_of("at the bound", &served)).len(),
        MAX_KEY_REPLICATION_REGIONS,
        "case at the bound: expected {MAX_KEY_REPLICATION_REGIONS} entries, got {served}"
    );
    let (status, parsed) = call(
        &router,
        "AddKeyReplicationRegions",
        &json!({"KeyIdentifier": bounded, "ReplicationRegions": [past_bound]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case past the bound: expected 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ServiceQuotaExceededException",
        "case past the bound: expected ServiceQuotaExceededException, got {parsed}"
    );
    let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": bounded})).await;
    let held = replication_status("past the bound", key_of("past the bound", &fetched));
    assert_eq!(
        held.len(),
        MAX_KEY_REPLICATION_REGIONS,
        "case past the bound: a refused add must store nothing, got {fetched}"
    );
    assert!(
        !held.contains_key(past_bound),
        "case past the bound: {past_bound} must not have been configured, got {fetched}"
    );
}
