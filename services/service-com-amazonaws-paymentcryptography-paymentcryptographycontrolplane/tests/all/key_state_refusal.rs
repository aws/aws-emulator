// SPDX-License-Identifier: CC0-1.0

//! The refusal a key in the wrong `KeyState` earns, end to end over the wire.
//!
//! The rule under test: an operation of this service that can only act on a key in
//! ONE `KeyState` refuses a key in any other state with `ValidationException` and the
//! sentence `Key <KeyArn> not in <RequiredState> state.` — the key's own ARN and the
//! state the operation requires are the only variable parts. The gate is the key's
//! state ALONE: no other member of the request can turn the refusal into a success or
//! change its wording, and a refused call writes nothing, so the record a later read
//! reports is the one that was already there.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{
    call, error_code, error_message, ok_call, router_with_engine, string,
};

/// One operation of this service whose handler can only act on a key in a single
/// `KeyState`, with that state and the input members it needs beyond `KeyIdentifier`.
///
/// Source: `smithy_query(query="digest")` for the operations that take a
/// `KeyIdentifier` and change the key, plus each operation's own documentation for the
/// state it requires — "The key must be in an active state to add Replication
/// Regions" (`AddKeyReplicationRegions`), and `RestoreKey`, which "Cancels a scheduled
/// key deletion during the waiting period" and so needs a key inside that window.
struct StateGated {
    operation: &'static str,
    required_state: &'static str,
    extra: Value,
}

/// Every state-gated operation of this service.
fn state_gated_operations() -> Vec<StateGated> {
    let regions = json!({"ReplicationRegions": ["eu-west-1"]});
    vec![
        StateGated {
            operation: "DeleteKey",
            required_state: "CREATE_COMPLETE",
            extra: json!({}),
        },
        StateGated {
            operation: "StartKeyUsage",
            required_state: "CREATE_COMPLETE",
            extra: json!({}),
        },
        StateGated {
            operation: "StopKeyUsage",
            required_state: "CREATE_COMPLETE",
            extra: json!({}),
        },
        StateGated {
            operation: "AddKeyReplicationRegions",
            required_state: "CREATE_COMPLETE",
            extra: regions.clone(),
        },
        StateGated {
            operation: "RemoveKeyReplicationRegions",
            required_state: "CREATE_COMPLETE",
            extra: regions,
        },
        StateGated {
            operation: "RestoreKey",
            required_state: "DELETE_PENDING",
            extra: json!({}),
        },
    ]
}

/// Every `KeyState` a stored key of this emulator can be observed in.
///
/// Source: `smithy_query(query="shape",shape="KeyState")` lists `CREATE_COMPLETE`,
/// `CREATE_IN_PROGRESS`, `DELETE_PENDING` and `DELETE_COMPLETE`; a key this service
/// mints is complete the moment it is created, so nothing here can observe
/// `CREATE_IN_PROGRESS`.
const OBSERVABLE_STATES: &[&str] = &["CREATE_COMPLETE", "DELETE_PENDING", "DELETE_COMPLETE"];

/// `CreateKey` bodies spanning the key families this service mints, so the state gate
/// is checked on more than one kind of key.
///
/// Source: `smithy_query(query="operation",operation="CreateKey")` — each row is a
/// `KeyAttributes` set whose `KeyUsage`, `KeyClass` and `KeyModesOfUse` match its
/// `KeyAlgorithm`.
fn key_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AES_128 data encryption",
            json!({
                "KeyAttributes": {
                    "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
                    "KeyClass": "SYMMETRIC_KEY",
                    "KeyAlgorithm": "AES_128",
                    "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
                },
                "Exportable": true,
            }),
        ),
        (
            "TDES_3KEY PIN encryption",
            json!({
                "KeyAttributes": {
                    "KeyUsage": "TR31_P0_PIN_ENCRYPTION_KEY",
                    "KeyClass": "SYMMETRIC_KEY",
                    "KeyAlgorithm": "TDES_3KEY",
                    "KeyModesOfUse": {"Encrypt": true, "Decrypt": true, "Wrap": true, "Unwrap": true},
                },
                "Exportable": true,
            }),
        ),
        (
            "RSA_2048 key wrapping",
            json!({
                "KeyAttributes": {
                    "KeyUsage": "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION",
                    "KeyClass": "ASYMMETRIC_KEY_PAIR",
                    "KeyAlgorithm": "RSA_2048",
                    "KeyModesOfUse": {"Encrypt": true, "Decrypt": true, "Wrap": true, "Unwrap": true},
                },
                "Exportable": true,
            }),
        ),
    ]
}

/// The request body for `operation` against `identifier`.
fn request_body(identifier: &str, extra: &Value) -> Value {
    let mut body = json!({"KeyIdentifier": identifier});
    for (member, value) in extra.as_object().expect("extra members are an object") {
        body[member] = value.clone();
    }
    body
}

/// The one sentence a key in the wrong state earns.
fn expected_sentence(key_arn: &str, required_state: &str) -> String {
    format!("Key {key_arn} not in {required_state} state.")
}

/// Create one key of `body` and answer its `KeyArn`.
async fn fresh_key(router: &MultiServiceRouter, body: &Value) -> String {
    let created = ok_call(router, "CreateKey", body).await;
    string(created.get("Key").expect("CreateKey answers a Key"), "KeyArn").to_string()
}

/// Leave a freshly created key in `state` and answer its `KeyArn`.
///
/// The shortest waiting period the model admits is three days (`DeleteKeyInDays` range
/// 3..180), so no request a test can send reaches the elapsed side of the deletion
/// window in real time: for `DELETE_COMPLETE` the stored deletion instant is moved
/// into the past — exactly the state the clock would have left — and the handlers are
/// then driven over the wire.
async fn key_in_state(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    body: &Value,
    state: &str,
) -> String {
    let arn = fresh_key(router, body).await;
    if state != "CREATE_COMPLETE" {
        ok_call(
            router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": 3}),
        )
        .await;
    }
    if state == "DELETE_COMPLETE" {
        let store = engine.state::<KeyStore>();
        let mut guard = store.lock();
        let stored = guard
            .keys
            .get_mut(&arn)
            .unwrap_or_else(|| panic!("the store holds the key {arn}"));
        stored.delete_pending_timestamp = Some(SystemTime::now() - Duration::from_secs(1));
    }
    let fetched = ok_call(router, "GetKey", &json!({"KeyIdentifier": arn})).await;
    assert_eq!(
        string(fetched.get("Key").expect("GetKey answers a Key"), "KeyState"),
        state,
        "setup: the key must be left in {state}: {fetched}"
    );
    arn
}

/// Assert `(status, body)` is the state refusal `required_state` calls for, naming
/// `key_arn`.
fn assert_state_refusal(
    case: &str,
    status: u16,
    body: &Value,
    key_arn: &str,
    required_state: &str,
) {
    // This service's wire protocol is awsJson1_0, which reports WHICH error a caller
    // hit in the body's `__type` and answers every client error with HTTP 400; see
    // `client_error_status`.
    assert_eq!(
        status, 400,
        "case {case}: a state refusal must be answered with HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    let expected = expected_sentence(key_arn, required_state);
    assert_eq!(
        error_message(body),
        expected,
        "case {case}: expected the message {expected:?}, got {body}"
    );
}

/// RULE: every state-gated operation refuses a key in any state but the one it
/// requires, with `ValidationException` and the state sentence, and writes nothing.
///
/// The table is every state-gated operation crossed with every `KeyState` a stored key
/// can be observed in, one fresh key per cell, so neither the operation nor the state
/// the key happens to be in can change the class or the sentence.
#[tokio::test]
async fn every_state_gated_operation_refuses_a_key_in_the_wrong_state() {
    let (router, engine) = router_with_engine();
    let (_, body) = key_bodies().remove(0);
    for state in OBSERVABLE_STATES {
        for gated in state_gated_operations() {
            if gated.required_state == *state {
                continue;
            }
            let case = format!("{} on a {state} key", gated.operation);
            let arn = key_in_state(&router, &engine, &body, state).await;
            let before = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;

            let (status, parsed) = call(
                &router,
                gated.operation,
                &request_body(&arn, &gated.extra),
            )
            .await;
            assert_state_refusal(&case, status, &parsed, &arn, gated.required_state);

            let after = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
            assert_eq!(
                after.get("Key"),
                before.get("Key"),
                "case {case}: a refused call must leave the stored key alone"
            );
        }
    }
}

/// RULE, on the other axis: the same refusal holds for every key family this service
/// mints, and the sentence always names that key's own ARN.
#[tokio::test]
async fn the_state_refusal_holds_for_every_key_family() {
    let (router, engine) = router_with_engine();
    for (family, body) in key_bodies() {
        for gated in state_gated_operations() {
            // One state per row that the operation does not accept: the pending key
            // for the active-state operations, the active key for `RestoreKey`.
            let state = if gated.required_state == "CREATE_COMPLETE" {
                "DELETE_PENDING"
            } else {
                "CREATE_COMPLETE"
            };
            let case = format!("{family}: {} on a {state} key", gated.operation);
            let arn = key_in_state(&router, &engine, &body, state).await;
            let (status, parsed) = call(
                &router,
                gated.operation,
                &request_body(&arn, &gated.extra),
            )
            .await;
            assert_state_refusal(&case, status, &parsed, &arn, gated.required_state);
        }
    }
}

/// RULE applied to `DeleteKey`'s own other member: the gate is the key's state alone,
/// so the waiting period a second `DeleteKey` names — the one already scheduled, a
/// different one inside the published range, each published bound, or none at all —
/// changes neither the refusal nor the deletion date the first call promised.
///
/// The rows are values of `DeleteKeyInDays` spanning its published range
/// (source: `smithy_query(query="operation",operation="DeleteKey")`, `range 3..180`).
#[tokio::test]
async fn the_delete_key_state_refusal_ignores_the_requested_waiting_period() {
    let (router, _engine) = router_with_engine();
    let (_, body) = key_bodies().remove(0);
    let arn = fresh_key(&router, &body).await;
    let scheduled = ok_call(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": arn, "DeleteKeyInDays": 5}),
    )
    .await;
    let promised = scheduled
        .get("Key")
        .expect("DeleteKey answers a Key")
        .clone();

    for days in [None, Some(5), Some(3), Some(30), Some(180)] {
        let case = format!("second DeleteKey with DeleteKeyInDays {days:?}");
        let request = match days {
            Some(days) => json!({"KeyIdentifier": arn, "DeleteKeyInDays": days}),
            None => json!({"KeyIdentifier": arn}),
        };
        let (status, parsed) = call(&router, "DeleteKey", &request).await;
        assert_state_refusal(&case, status, &parsed, &arn, "CREATE_COMPLETE");

        let after = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            after.get("Key"),
            Some(&promised),
            "case {case}: a refused re-schedule must leave the promised deletion date \
             alone"
        );
    }
}

/// RULE applied to the identifier: the sentence names the KEY's own ARN whatever form
/// of `KeyArnOrKeyAliasType` the request used to reach it.
///
/// The rows are every form that member admits (source:
/// `smithy_query(query="operation",operation="DeleteKey")` — a key ARN, an alias ARN,
/// or a bare `alias/<name>`), all resolving to the one key.
#[tokio::test]
async fn the_state_refusal_names_the_keys_own_arn_for_every_identifier_form() {
    let (router, engine) = router_with_engine();
    let (_, body) = key_bodies().remove(0);
    let arn = key_in_state(&router, &engine, &body, "DELETE_PENDING").await;
    let alias = "alias/state-gated-key";
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias, "KeyArn": arn}),
    )
    .await;
    let account = engine::emulator_account_id();
    let region = arn
        .split(':')
        .nth(3)
        .expect("a key ARN carries its region")
        .to_string();
    let identifiers = [
        ("a key ARN", arn.clone()),
        ("a bare alias", alias.to_string()),
        (
            "an alias ARN",
            format!("arn:aws:payment-cryptography:{region}:{account}:{alias}"),
        ),
    ];

    for (form, identifier) in identifiers {
        for operation in ["DeleteKey", "StopKeyUsage", "StartKeyUsage"] {
            let case = format!("{operation} through {form}");
            let (status, parsed) =
                call(&router, operation, &json!({"KeyIdentifier": identifier})).await;
            assert_state_refusal(&case, status, &parsed, &arn, "CREATE_COMPLETE");
        }
    }
}
