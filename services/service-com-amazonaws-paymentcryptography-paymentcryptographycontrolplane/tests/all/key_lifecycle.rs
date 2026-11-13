// SPDX-License-Identifier: CC0-1.0

//! The key lifecycle, end to end over the wire: `CreateKey`, `GetKey`,
//! `CreateAlias`, `GetAlias`, `ListKeys`, `ListAliases`, `DeleteKey`, `RestoreKey`.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, so what these tests assert is what a signed SDK request
//! observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted key
//! or alias record — the key check value is computed from that key's own stored
//! material by the KCV algorithm in effect, the ARN is the one the record was stored
//! under, and whatever a create or a delete wrote is what every later reader sees.
//! Each case is one row of a table that spans the class the rule generalises over
//! (every modelled key algorithm, every KCV algorithm, every page size, the
//! published bounds of `DeleteKeyInDays` and `MaxResults`), not just the one
//! algorithm the issue happened to name.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ops::types::{
    KEY_ALGORITHM_VALUES, KEY_CHECK_VALUE_ALGORITHM_VALUES, KEY_CLASS_VALUES,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    delete_pending_instant, generated_key_class, key_check_value,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::{
    ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore,
    EmulatedService,
};

/// The region every request below signs for; every minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";
/// The `X-Amz-Target` prefix from this crate's OPERATIONS.md wire-protocol section.
pub(crate) const TARGET_PREFIX: &str = "PaymentCryptographyControlPlane";
/// A day in seconds, the unit `DeleteKeyInDays` counts in.
const ONE_DAY_SECS: f64 = 24.0 * 60.0 * 60.0;

/// A router serving this one service over a fresh engine, as `build_router` does in
/// production, plus a handle on that engine so a test can read the persisted record
/// and check the response was computed from it.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A router serving this one service over a fresh engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope
        // names this service's signing name and the region the handlers build ARNs
        // for. Request authentication is off, so the signature is never checked.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/payment-cryptography/\
                 aws4_request, SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
            ),
        )
        .header("host", format!("controlplane.payment-cryptography.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
pub(crate) async fn ok_call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} {body}: expected 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error shape of a failed call (`__type`, with any namespace stripped).
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A string member that must be present.
pub(crate) fn string<'a>(value: &'a Value, member: &str) -> &'a str {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
}

/// A timestamp member, in epoch seconds (awsJson1_0 renders a `Timestamp` as a
/// number).
pub(crate) fn timestamp(value: &Value, member: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric {member} in {value}"))
}

/// Now, in epoch seconds.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Assert `arn` satisfies the modelled `KeyArn` pattern
/// `^arn:aws:payment-cryptography:[a-z]{2}-[a-z]{1,16}-[0-9]+:[0-9]{12}:key/[0-9a-zA-Z]{16,64}$`
/// and its 70..150 length (source: `smithy_query(query="shape",shape="KeyArn")`),
/// checked by hand because this crate has no regex dependency.
fn assert_key_arn(case: &str, arn: &str) {
    assert!(
        (70..=150).contains(&arn.len()),
        "case {case}: KeyArn {arn} has length {}, outside the modelled 70..150",
        arn.len()
    );
    let prefix = format!("arn:aws:payment-cryptography:{REGION}:{ACCOUNT}:key/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case}: KeyArn {arn} does not start with {prefix}");
    });
    assert!(
        (16..=64).contains(&id.len()) && id.chars().all(char::is_alphanumeric),
        "case {case}: KeyArn {arn} carries the id segment {id:?}, which is not [0-9a-zA-Z]{{16,64}}"
    );
}

/// Assert `kcv` satisfies the modelled `KeyCheckValue` pattern `^[0-9a-fA-F]+$` and
/// its 4..16 length (source: `smithy_query(query="shape",shape="KeyCheckValue")`).
fn assert_key_check_value(case: &str, kcv: &str) {
    assert!(
        (4..=16).contains(&kcv.len()) && kcv.chars().all(|c| c.is_ascii_hexdigit()),
        "case {case}: KeyCheckValue {kcv:?} is not 4..16 hex characters"
    );
}

/// One `CreateKey` request body for a symmetric or asymmetric key.
fn create_key_body(row: &AlgorithmRow, extra: Value) -> Value {
    let mut body = json!({
        "KeyAttributes": {
            "KeyUsage": row.key_usage,
            "KeyClass": row.key_class,
            "KeyAlgorithm": row.key_algorithm,
            "KeyModesOfUse": row.modes.clone(),
        },
        "Exportable": true,
    });
    if let (Value::Object(target), Value::Object(extra)) = (&mut body, extra) {
        target.extend(extra);
    }
    body
}

/// One row of the key-algorithm table: an algorithm, an attribute set the service's
/// own documentation pairs it with, and the key check value algorithm the service
/// applies when the request names none.
struct AlgorithmRow {
    key_algorithm: &'static str,
    key_class: &'static str,
    key_usage: &'static str,
    modes: Value,
    default_kcv_algorithm: &'static str,
}

/// Every value the modelled `KeyAlgorithm` enum admits, each paired with a key class,
/// a key usage and modes of use.
///
/// Source: `smithy_query(query="shape",shape="KeyAlgorithm,KeyClass,KeyUsage")`,
/// mirrored by the generated `KEY_ALGORITHM_VALUES` the test below checks this table
/// against — so a value added to the model fails this suite rather than going
/// untested.
fn algorithm_table() -> Vec<AlgorithmRow> {
    let encrypt_decrypt = json!({"Encrypt": true, "Decrypt": true});
    let wrap_unwrap = json!({"Wrap": true, "Unwrap": true});
    let mac = json!({"Generate": true, "Verify": true});
    let sign_verify = json!({"Sign": true, "Verify": true});
    let derive = json!({"DeriveKey": true});
    vec![
        AlgorithmRow {
            key_algorithm: "TDES_2KEY",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_P0_PIN_ENCRYPTION_KEY",
            modes: encrypt_decrypt.clone(),
            default_kcv_algorithm: "ANSI_X9_24",
        },
        AlgorithmRow {
            key_algorithm: "TDES_3KEY",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_K0_KEY_ENCRYPTION_KEY",
            modes: wrap_unwrap.clone(),
            default_kcv_algorithm: "ANSI_X9_24",
        },
        AlgorithmRow {
            key_algorithm: "AES_128",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            modes: encrypt_decrypt.clone(),
            default_kcv_algorithm: "CMAC",
        },
        AlgorithmRow {
            key_algorithm: "AES_192",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            modes: encrypt_decrypt.clone(),
            default_kcv_algorithm: "CMAC",
        },
        AlgorithmRow {
            key_algorithm: "AES_256",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
            modes: wrap_unwrap,
            default_kcv_algorithm: "CMAC",
        },
        AlgorithmRow {
            key_algorithm: "HMAC_SHA224",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: mac.clone(),
            default_kcv_algorithm: "HMAC",
        },
        AlgorithmRow {
            key_algorithm: "HMAC_SHA256",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: mac.clone(),
            default_kcv_algorithm: "HMAC",
        },
        AlgorithmRow {
            key_algorithm: "HMAC_SHA384",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: mac.clone(),
            default_kcv_algorithm: "HMAC",
        },
        AlgorithmRow {
            key_algorithm: "HMAC_SHA512",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: mac,
            default_kcv_algorithm: "HMAC",
        },
        AlgorithmRow {
            key_algorithm: "RSA_2048",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
            modes: sign_verify.clone(),
            default_kcv_algorithm: "SHA_1",
        },
        AlgorithmRow {
            key_algorithm: "RSA_3072",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
            modes: sign_verify.clone(),
            default_kcv_algorithm: "SHA_1",
        },
        AlgorithmRow {
            key_algorithm: "RSA_4096",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION",
            modes: encrypt_decrypt,
            default_kcv_algorithm: "SHA_1",
        },
        AlgorithmRow {
            key_algorithm: "ECC_NIST_P256",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
            modes: derive.clone(),
            default_kcv_algorithm: "SHA_1",
        },
        AlgorithmRow {
            key_algorithm: "ECC_NIST_P384",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
            modes: derive.clone(),
            default_kcv_algorithm: "SHA_1",
        },
        AlgorithmRow {
            key_algorithm: "ECC_NIST_P521",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
            modes: derive,
            default_kcv_algorithm: "SHA_1",
        },
    ]
}

#[tokio::test]
async fn create_key_stores_every_modelled_algorithm_and_get_key_reports_it_back() {
    let table = algorithm_table();
    // The table is the whole modelled catalog, not a sample of it.
    let covered: BTreeSet<&str> = table.iter().map(|row| row.key_algorithm).collect();
    let modelled: BTreeSet<&str> = KEY_ALGORITHM_VALUES.iter().copied().collect();
    assert_eq!(
        covered, modelled,
        "the table must cover every modelled KeyAlgorithm; covered {covered:?}"
    );

    let (router, engine) = router_with_engine();
    let mut arns = BTreeSet::new();
    for row in &table {
        let case = row.key_algorithm;
        let before = now_secs();
        let created = ok_call(
            &router,
            "CreateKey",
            &create_key_body(row, json!({"Tags": [{"Key": "env", "Value": "test"}]})),
        )
        .await;
        let after = now_secs();
        let key = created
            .get("Key")
            .unwrap_or_else(|| panic!("case {case}: CreateKey answered no Key: {created}"));

        let arn = string(key, "KeyArn").to_string();
        assert_key_arn(case, &arn);
        assert!(
            arns.insert(arn.clone()),
            "case {case}: CreateKey reused the ARN {arn}"
        );

        assert_key_check_value(case, string(key, "KeyCheckValue"));
        assert_eq!(
            string(key, "KeyCheckValueAlgorithm"),
            row.default_kcv_algorithm,
            "case {case}: expected the default KCV algorithm {}, got {}",
            row.default_kcv_algorithm,
            string(key, "KeyCheckValueAlgorithm")
        );
        assert_eq!(
            string(key, "KeyState"),
            "CREATE_COMPLETE",
            "case {case}: expected KeyState CREATE_COMPLETE, got {key}"
        );
        assert_eq!(
            string(key, "KeyOrigin"),
            "AWS_PAYMENT_CRYPTOGRAPHY",
            "case {case}: expected KeyOrigin AWS_PAYMENT_CRYPTOGRAPHY, got {key}"
        );
        // `Enabled` defaults to true when the request omits it, per that member's
        // own documentation.
        assert_eq!(
            key.get("Enabled").and_then(Value::as_bool),
            Some(true),
            "case {case}: expected Enabled true by default, got {key}"
        );
        assert_eq!(
            key.get("Exportable").and_then(Value::as_bool),
            Some(true),
            "case {case}: expected the stored Exportable true, got {key}"
        );
        assert_eq!(
            key.get("KeyAttributes").and_then(|a| a.get("KeyAlgorithm")),
            Some(&Value::String(row.key_algorithm.to_string())),
            "case {case}: expected the stored KeyAlgorithm {}, got {key}",
            row.key_algorithm
        );
        let created_at = timestamp(key, "CreateTimestamp");
        assert!(
            created_at >= before - 1.0 && created_at <= after + 1.0,
            "case {case}: CreateTimestamp {created_at} is not within the call window \
             {before}..{after}"
        );
        assert_eq!(
            timestamp(key, "UsageStartTimestamp"),
            created_at,
            "case {case}: a key is usable from the moment it exists, so \
             UsageStartTimestamp must equal CreateTimestamp: {key}"
        );
        assert!(
            key.get("DeletePendingTimestamp").is_none() && key.get("DeleteTimestamp").is_none(),
            "case {case}: a fresh key is not scheduled for deletion: {key}"
        );

        // The reported check value is the one this key's own stored material
        // produces under the algorithm in effect — not a value minted for the
        // response.
        {
            let store = engine.state::<KeyStore>();
            let guard = store.lock();
            let stored = guard
                .keys
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: no key stored under {arn}"));
            assert_eq!(
                stored
                    .check_value_under(&stored.key_check_value_algorithm)
                    .as_deref(),
                Some(string(key, "KeyCheckValue")),
                "case {case}: the reported KeyCheckValue must be what the stored key \
                 material computes"
            );
        }

        // The round trip: a read reports exactly what the create stored.
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            fetched.get("Key"),
            Some(key),
            "case {case}: GetKey must report the created Key unchanged"
        );
        // And a second read reports the same thing again: the key check value is a
        // function of the stored material, not of the call.
        let again = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            again.get("Key"),
            Some(key),
            "case {case}: a second GetKey must report the same Key"
        );
    }
}

#[tokio::test]
async fn key_check_value_algorithm_is_the_one_the_request_names_where_it_is_defined() {
    // Every pairing of a KCV algorithm with a key algorithm family, from
    // `KeyCheckValueAlgorithm`'s own documentation: the zero-block encryption for
    // TDES keys, CMAC for AES keys, the selected hash for HMAC keys, and the SHA-1
    // digest for any key. A pairing that documentation gives no computation for is
    // refused — including an AES key named with `ANSI_X9_24`, which the
    // documentation pairs with CMAC instead.
    let rows: &[(&str, &str, bool)] = &[
        ("TDES_2KEY", "ANSI_X9_24", true),
        ("TDES_3KEY", "ANSI_X9_24", true),
        ("AES_128", "ANSI_X9_24", false),
        ("AES_256", "ANSI_X9_24", false),
        ("AES_128", "CMAC", true),
        ("AES_192", "CMAC", true),
        ("AES_256", "CMAC", true),
        ("TDES_3KEY", "CMAC", false),
        ("HMAC_SHA256", "HMAC", true),
        ("HMAC_SHA512", "HMAC", true),
        ("AES_128", "HMAC", false),
        ("RSA_2048", "SHA_1", true),
        ("ECC_NIST_P521", "SHA_1", true),
        ("TDES_2KEY", "SHA_1", true),
        ("RSA_2048", "CMAC", false),
        ("ECC_NIST_P256", "ANSI_X9_24", false),
    ];
    // Every modelled KCV algorithm appears above.
    let covered: BTreeSet<&str> = rows.iter().map(|(_, kcv, _)| *kcv).collect();
    let modelled: BTreeSet<&str> = KEY_CHECK_VALUE_ALGORITHM_VALUES.iter().copied().collect();
    assert_eq!(
        covered, modelled,
        "the table must cover every modelled KeyCheckValueAlgorithm; covered {covered:?}"
    );

    let table = algorithm_table();
    let router = router();
    for (key_algorithm, kcv_algorithm, accepted) in rows {
        let case = format!("{key_algorithm}+{kcv_algorithm}");
        let row = table
            .iter()
            .find(|row| row.key_algorithm == *key_algorithm)
            .unwrap_or_else(|| panic!("case {case}: no table row for {key_algorithm}"));
        let body = create_key_body(row, json!({"KeyCheckValueAlgorithm": kcv_algorithm}));
        let (status, parsed) = call(&router, "CreateKey", &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {parsed}"
            );
            let key = parsed
                .get("Key")
                .unwrap_or_else(|| panic!("case {case}: no Key in {parsed}"));
            assert_eq!(
                string(key, "KeyCheckValueAlgorithm"),
                *kcv_algorithm,
                "case {case}: the stored KCV algorithm must be the one requested: {key}"
            );
            assert_key_check_value(&case, string(key, "KeyCheckValue"));
            // The stored value is reported identically by a later read.
            let arn = string(key, "KeyArn").to_string();
            let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
            assert_eq!(
                fetched.get("Key"),
                Some(key),
                "case {case}: GetKey must report the created Key unchanged"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: a KCV algorithm with no definition for this key must be \
                 refused, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: expected ValidationException, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn create_key_refuses_attributes_and_tags_the_service_does_not_accept() {
    let router = router();
    let modes = json!({"Encrypt": true, "Decrypt": true});

    // A key's algorithm already says whether this service generates one secret or a
    // whole pair, so its class has to say the same — and `PUBLIC_KEY`/`PRIVATE_KEY`
    // each name one half of a pair that already exists, which a generate call cannot
    // mint. Every `KeyClass` the model admits appears below.
    let rows: &[(&str, &str, bool)] = &[
        ("AES_256", "SYMMETRIC_KEY", true),
        ("TDES_2KEY", "SYMMETRIC_KEY", true),
        ("HMAC_SHA256", "SYMMETRIC_KEY", true),
        ("RSA_2048", "ASYMMETRIC_KEY_PAIR", true),
        ("ECC_NIST_P384", "ASYMMETRIC_KEY_PAIR", true),
        ("ECC_NIST_P384", "PUBLIC_KEY", false),
        ("RSA_3072", "PRIVATE_KEY", false),
        ("AES_128", "ASYMMETRIC_KEY_PAIR", false),
        ("TDES_3KEY", "PUBLIC_KEY", false),
        ("HMAC_SHA512", "PRIVATE_KEY", false),
        ("RSA_4096", "SYMMETRIC_KEY", false),
        ("ECC_NIST_P521", "SYMMETRIC_KEY", false),
    ];
    let covered: BTreeSet<&str> = rows.iter().map(|(_, class, _)| *class).collect();
    let modelled: BTreeSet<&str> = KEY_CLASS_VALUES.iter().copied().collect();
    assert_eq!(
        covered, modelled,
        "the table must cover every modelled KeyClass; covered {covered:?}"
    );

    for (key_algorithm, key_class, accepted) in rows {
        let case = format!("{key_algorithm} as {key_class}");
        // The usage is the one of this ALGORITHM's own family, read from the same
        // table the service classifies an algorithm with rather than from the spelling
        // of its name, so an algorithm the model gains later cannot be mis-classified
        // here. What the negative rows below are refused for is the class their
        // algorithm cannot carry.
        let key_usage = if generated_key_class(key_algorithm) == "SYMMETRIC_KEY" {
            "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY"
        } else {
            "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE"
        };
        let body = json!({
            "KeyAttributes": {
                "KeyUsage": key_usage,
                "KeyClass": key_class,
                "KeyAlgorithm": key_algorithm,
                "KeyModesOfUse": modes.clone(),
            },
            "Exportable": true,
        });
        let (status, parsed) = call(&router, "CreateKey", &body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} {parsed}"
        );
        if !*accepted {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: expected ValidationException, got {parsed}"
            );
        }
    }

    // Tag rules the model cannot express: one key twice, and the reserved prefix.
    let table = algorithm_table();
    let tag_rows: &[(Value, bool)] = &[
        (json!([{"Key": "env", "Value": "test"}]), true),
        (
            json!([{"Key": "env", "Value": "test"}, {"Key": "owner", "Value": "payments"}]),
            true,
        ),
        (json!([{"Key": "awsome", "Value": "fine"}]), true),
        (
            json!([{"Key": "env", "Value": "test"}, {"Key": "env", "Value": "prod"}]),
            false,
        ),
        (json!([{"Key": "aws:created-by", "Value": "me"}]), false),
        (json!([{"Key": "AWS:Created", "Value": "me"}]), false),
    ];
    for (tags, accepted) in tag_rows {
        let case = format!("Tags {tags}");
        let body = create_key_body(&table[0], json!({"Tags": tags.clone()}));
        let (status, parsed) = call(&router, "CreateKey", &body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} {parsed}"
        );
        if !*accepted {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: expected ValidationException, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn an_alias_binds_a_key_and_resolves_in_every_identifier_form() {
    let router = router();
    let table = algorithm_table();
    let key = ok_call(&router, "CreateKey", &create_key_body(&table[0], json!({}))).await;
    let arn = string(
        key.get("Key").expect("CreateKey answers a Key"),
        "KeyArn",
    )
    .to_string();

    // Every form of alias name the modelled `AliasName` pattern admits, including
    // the nested and punctuated ones the issue did not cite.
    for name in [
        "alias/pin-key-1",
        "alias/team/payments/pin_key",
        "alias/UPPER-and-lower_09",
    ] {
        let created = ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": arn}),
        )
        .await;
        let alias = created
            .get("Alias")
            .unwrap_or_else(|| panic!("case {name}: CreateAlias answered no Alias: {created}"));
        assert_eq!(
            string(alias, "AliasName"),
            name,
            "case {name}: the stored alias name must be the one created"
        );
        assert_eq!(
            string(alias, "KeyArn"),
            arn,
            "case {name}: the stored binding must name the created key"
        );

        // The binding is readable back.
        let fetched = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            fetched.get("Alias"),
            Some(alias),
            "case {name}: GetAlias must report the stored binding"
        );

        // And it resolves a `KeyIdentifier`, both bare and as an alias ARN.
        let alias_arn = format!("arn:aws:payment-cryptography:{REGION}:{ACCOUNT}:{name}");
        for identifier in [name.to_string(), alias_arn] {
            let through_alias =
                ok_call(&router, "GetKey", &json!({"KeyIdentifier": identifier})).await;
            assert_eq!(
                through_alias.get("Key"),
                key.get("Key"),
                "case {name}: GetKey through {identifier} must report the same key"
            );
        }

        // An alias name is unique in the account and region, so the repeat of even
        // the byte-identical create is refused: the name is taken, and nothing about
        // the stored binding changes.
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": arn}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name}: re-creating a stored alias name must conflict, got {status} \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {name}: expected ConflictException, got {parsed}"
        );
        let after_retry = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            after_retry.get("Alias"),
            Some(alias),
            "case {name}: a refused repeat must leave the stored binding alone"
        );

        // Rebinding the same name to a different key is the inconsistent state
        // `ConflictException` describes; `UpdateAlias` is the operation for that.
        let other = ok_call(&router, "CreateKey", &create_key_body(&table[1], json!({}))).await;
        let other_arn = string(other.get("Key").expect("a Key"), "KeyArn").to_string();
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": other_arn}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name}: rebinding an alias to another key must conflict, got {status} \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {name}: expected ConflictException, got {parsed}"
        );
        // And the refused rebind changed nothing.
        let after = ok_call(&router, "GetAlias", &json!({"AliasName": name})).await;
        assert_eq!(
            after.get("Alias"),
            Some(alias),
            "case {name}: a refused rebind must leave the stored binding alone"
        );
    }

    // An alias cannot be created without a key: this operation's own documentation
    // says so ("You can't create an alias without a key"), even though the model
    // leaves the member optional on the input shape.
    let (status, parsed) = call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/unbound"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case unbound: an alias without a key must be refused, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case unbound: expected ValidationException, got {parsed}"
    );
    let (status, parsed) = call(&router, "GetAlias", &json!({"AliasName": "alias/unbound"})).await;
    assert_eq!(
        status, 404,
        "case unbound: the refused alias must not have been stored, got {status} {parsed}"
    );

    // An alias cannot be bound to a key that was never stored.
    let absent = format!("arn:aws:payment-cryptography:{REGION}:{ACCOUNT}:key/doesnotexist0000");
    let (status, parsed) = call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/nowhere", "KeyArn": absent}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case nowhere: binding to an unknown key must 404, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFoundException",
        "case nowhere: expected ResourceNotFoundException, got {parsed}"
    );

    // Unknown identifiers: an alias never created, and a key ARN never minted.
    for identifier in ["alias/never-created", absent.as_str()] {
        let (status, parsed) = call(&router, "GetKey", &json!({"KeyIdentifier": identifier})).await;
        assert_eq!(
            status, 404,
            "case {identifier}: GetKey on an unknown identifier must 404, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {identifier}: expected ResourceNotFoundException, got {parsed}"
        );
    }
    let (status, parsed) = call(
        &router,
        "GetAlias",
        &json!({"AliasName": "alias/never-created"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case GetAlias unknown: expected 404, got {status} {parsed}"
    );
}

/// Walk every page of `operation`, answering the items it yielded in order.
async fn page_through(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
    filter: Value,
    page_size: usize,
) -> Vec<Value> {
    let mut items = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..100 {
        let mut body = filter.clone();
        if let Value::Object(map) = &mut body {
            map.insert("MaxResults".to_string(), json!(page_size));
            if let Some(token) = &next_token {
                map.insert("NextToken".to_string(), json!(token));
            }
        }
        let answered = ok_call(router, operation, &body).await;
        let page_items = answered
            .get(items_member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{operation} answered no {items_member}: {answered}"));
        assert!(
            page_items.len() <= page_size,
            "page {page} of {operation} with MaxResults {page_size} returned {} items",
            page_items.len()
        );
        items.extend(page_items.iter().cloned());
        next_token = answered
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return items;
        }
        assert!(
            !page_items.is_empty(),
            "page {page} of {operation} handed out a NextToken for an empty page"
        );
    }
    panic!("{operation} never stopped paginating");
}

#[tokio::test]
async fn list_keys_pages_every_stored_key_and_filters_by_state() {
    let router = router();
    let table = algorithm_table();
    let mut arns = Vec::new();
    for row in table.iter().take(5) {
        let created = ok_call(&router, "CreateKey", &create_key_body(row, json!({}))).await;
        arns.push(string(created.get("Key").expect("a Key"), "KeyArn").to_string());
    }
    let expected: BTreeSet<String> = arns.iter().cloned().collect();

    // Every page size from one-at-a-time to larger than the store yields exactly the
    // stored keys, once each.
    for page_size in [1, 2, 3, 5, 7, 100] {
        let items = page_through(&router, "ListKeys", "Keys", json!({}), page_size).await;
        let seen: Vec<String> = items
            .iter()
            .map(|item| string(item, "KeyArn").to_string())
            .collect();
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique, expected,
            "case MaxResults {page_size}: paging must yield every stored key"
        );
        assert_eq!(
            seen.len(),
            unique.len(),
            "case MaxResults {page_size}: paging must not repeat a key: {seen:?}"
        );
        // The summaries are built from the stored keys, not from a canned body.
        for item in &items {
            let case = format!("MaxResults {page_size}");
            assert_key_arn(&case, string(item, "KeyArn"));
            assert_key_check_value(&case, string(item, "KeyCheckValue"));
            assert_eq!(
                string(item, "KeyState"),
                "CREATE_COMPLETE",
                "case {case}: every listed key is CREATE_COMPLETE here: {item}"
            );
            assert!(
                item.get("KeyAttributes").is_some() && item.get("Enabled").is_some(),
                "case {case}: a KeySummary carries the stored attributes: {item}"
            );
        }
    }

    // A key summary reports the same check value the key itself does.
    let one = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arns[0]})).await;
    let listed = page_through(&router, "ListKeys", "Keys", json!({}), 100).await;
    let summary = listed
        .iter()
        .find(|item| string(item, "KeyArn") == arns[0])
        .expect("the key is listed");
    assert_eq!(
        string(summary, "KeyCheckValue"),
        string(one.get("Key").expect("a Key"), "KeyCheckValue"),
        "ListKeys and GetKey must report the same stored check value"
    );

    // Filtering by state follows what the records say: schedule one for deletion and
    // it moves between the two filters.
    ok_call(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": arns[0], "DeleteKeyInDays": 7}),
    )
    .await;
    for (state, expected_arns) in [
        (
            "DELETE_PENDING",
            BTreeSet::from([arns[0].clone()]),
        ),
        (
            "CREATE_COMPLETE",
            arns[1..].iter().cloned().collect::<BTreeSet<String>>(),
        ),
    ] {
        let items = page_through(&router, "ListKeys", "Keys", json!({"KeyState": state}), 2).await;
        let seen: BTreeSet<String> = items
            .iter()
            .map(|item| string(item, "KeyArn").to_string())
            .collect();
        assert_eq!(
            seen, expected_arns,
            "case KeyState {state}: expected {expected_arns:?}, got {seen:?}"
        );
    }

    // A token this service never issued is refused rather than silently restarting.
    for token in ["not-hex", "00", "6e6f742d616e2d61726e"] {
        let (status, parsed) = call(&router, "ListKeys", &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case token {token}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case token {token}: expected ValidationException, got {parsed}"
        );
    }

    // `MaxResults`' published bounds (1..100) are accepted and one past each is not.
    for (max_results, accepted) in [(1, true), (100, true), (0, false), (101, false)] {
        let (status, parsed) = call(&router, "ListKeys", &json!({"MaxResults": max_results})).await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case MaxResults {max_results}: expected {expected}, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn list_aliases_pages_every_stored_alias_and_filters_by_key() {
    let router = router();
    let table = algorithm_table();
    let mut arns = Vec::new();
    for row in table.iter().take(2) {
        let created = ok_call(&router, "CreateKey", &create_key_body(row, json!({}))).await;
        arns.push(string(created.get("Key").expect("a Key"), "KeyArn").to_string());
    }

    let mut bound: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let name = format!("alias/key-{index}");
        let arn = arns[index % arns.len()].clone();
        ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": name, "KeyArn": arn}),
        )
        .await;
        bound.push((name, arn));
    }

    let expected: BTreeSet<String> = bound.iter().map(|(name, _)| name.clone()).collect();
    for page_size in [1, 2, 4, 100] {
        let items = page_through(&router, "ListAliases", "Aliases", json!({}), page_size).await;
        let seen: Vec<String> = items
            .iter()
            .map(|item| string(item, "AliasName").to_string())
            .collect();
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique, expected,
            "case MaxResults {page_size}: paging must yield every stored alias"
        );
        assert_eq!(
            seen.len(),
            unique.len(),
            "case MaxResults {page_size}: paging must not repeat an alias: {seen:?}"
        );
    }

    // Filtering by key yields exactly the aliases bound to that key.
    for arn in &arns {
        let expected: BTreeSet<String> = bound
            .iter()
            .filter(|(_, bound_to)| bound_to == arn)
            .map(|(name, _)| name.clone())
            .collect();
        let items =
            page_through(&router, "ListAliases", "Aliases", json!({"KeyArn": arn}), 2).await;
        let seen: BTreeSet<String> = items
            .iter()
            .map(|item| string(item, "AliasName").to_string())
            .collect();
        assert_eq!(
            seen, expected,
            "case KeyArn {arn}: expected {expected:?}, got {seen:?}"
        );
    }
}

/// A list request that names no `MaxResults` is served the DOCUMENTED default page,
/// not the whole store.
///
/// The rule: each list operation's `MaxResults` member documents its own default —
/// "This value is optional. If you include a value, it must be between 1 and 100,
/// inclusive. If you do not include a value, it defaults to 50." (the same sentence
/// on `ListKeys`, `ListAliases` and `ListTagsForResource`; source: that member's doc
/// in each handler's modelled contract) — so a request that asks for no page size
/// gets fifty items and a `NextToken` that resumes where it stopped. The store is
/// seeded with one item MORE than the default, so a default-sized page is necessarily
/// truncated, and both list operations of this slice are checked, not just the one
/// the issue named.
#[tokio::test]
async fn a_list_request_with_no_max_results_serves_the_documented_default_page() {
    /// The documented default page size, from the `MaxResults` doc quoted above.
    const DEFAULT_PAGE: usize = 50;
    let total = DEFAULT_PAGE + 1;

    let router = router();
    let table = algorithm_table();
    let mut last_key_arn = String::new();
    for index in 0..total {
        // Cycle the algorithm table so the seeded keys are not all alike.
        let row = &table[index % table.len()];
        let created = ok_call(&router, "CreateKey", &create_key_body(row, json!({}))).await;
        last_key_arn = string(created.get("Key").expect("a Key"), "KeyArn").to_string();
    }
    for index in 0..total {
        ok_call(
            &router,
            "CreateAlias",
            &json!({
                "AliasName": format!("alias/default-page-{index:03}"),
                "KeyArn": last_key_arn,
            }),
        )
        .await;
    }

    for (operation, items_member) in [("ListKeys", "Keys"), ("ListAliases", "Aliases")] {
        let first = ok_call(&router, operation, &json!({})).await;
        let page = first
            .get(items_member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {operation}: answered no {items_member}: {first}"));
        assert_eq!(
            page.len(),
            DEFAULT_PAGE,
            "case {operation} with no MaxResults: expected the documented default of \
             {DEFAULT_PAGE} items, got {}",
            page.len()
        );
        let token = first
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {operation} with no MaxResults: {total} stored items do not fit \
                     one default page, so a NextToken is owed: {first}"
                )
            })
            .to_string();

        let second = ok_call(&router, operation, &json!({"NextToken": token})).await;
        let rest = second
            .get(items_member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {operation}: answered no {items_member}: {second}"));
        assert_eq!(
            rest.len(),
            total - DEFAULT_PAGE,
            "case {operation} page two: expected the remaining {} items, got {}",
            total - DEFAULT_PAGE,
            rest.len()
        );
        assert!(
            second.get("NextToken").is_none(),
            "case {operation} page two: the last page carries no NextToken: {second}"
        );
    }
}

#[tokio::test]
async fn delete_key_schedules_the_deletion_and_restore_key_cancels_it() {
    let router = router();
    let table = algorithm_table();

    // `DeleteKeyInDays` spans its published 3..180 range; omitting it applies the
    // documented default of seven days.
    let rows: &[(Option<i32>, i32)] = &[(None, 7), (Some(3), 3), (Some(7), 7), (Some(180), 180)];
    for (requested, expected_days) in rows {
        let case = format!("DeleteKeyInDays {requested:?}");
        let created = ok_call(&router, "CreateKey", &create_key_body(&table[0], json!({}))).await;
        let arn = string(created.get("Key").expect("a Key"), "KeyArn").to_string();

        let mut body = json!({"KeyIdentifier": arn});
        if let (Value::Object(map), Some(days)) = (&mut body, requested) {
            map.insert("DeleteKeyInDays".to_string(), json!(days));
        }
        let before = now_secs();
        let deleted = ok_call(&router, "DeleteKey", &body).await;
        let key = deleted
            .get("Key")
            .unwrap_or_else(|| panic!("case {case}: DeleteKey answered no Key: {deleted}"));
        assert_eq!(
            string(key, "KeyState"),
            "DELETE_PENDING",
            "case {case}: expected DELETE_PENDING, got {key}"
        );
        let pending_at = timestamp(key, "DeletePendingTimestamp");
        let expected_at = before + f64::from(*expected_days) * ONE_DAY_SECS;
        assert!(
            (pending_at - expected_at).abs() < 120.0,
            "case {case}: DeletePendingTimestamp {pending_at} is not {expected_days} days \
             out (expected about {expected_at})"
        );
        assert!(
            key.get("DeleteTimestamp").is_none(),
            "case {case}: a key still inside its waiting period is not deleted yet: {key}"
        );

        // The next reader sees the scheduled deletion.
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            fetched.get("Key"),
            Some(key),
            "case {case}: GetKey must report the key as DeleteKey left it"
        );

        // `DeleteKey` is `@idempotent`: a retry neither fails nor moves the date.
        let retried = ok_call(&router, "DeleteKey", &body).await;
        assert_eq!(
            retried.get("Key"),
            Some(key),
            "case {case}: a repeated DeleteKey must not change the scheduled deletion"
        );

        // A second request naming a DIFFERENT waiting period is not that retry: it
        // would move a deletion date the caller was already promised, so it is
        // refused and the stored schedule is left alone.
        let other_days = if *expected_days == 7 { 30 } else { 7 };
        let (status, parsed) = call(
            &router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": other_days}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case}: re-scheduling the deletion for {other_days} days must \
             conflict, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {case}: expected ConflictException, got {parsed}"
        );
        let after = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            after.get("Key"),
            Some(key),
            "case {case}: a refused re-schedule must leave the stored deletion date \
             alone"
        );

        // Restoring takes it back, and the restored key reports no pending deletion.
        let restored = ok_call(&router, "RestoreKey", &json!({"KeyIdentifier": arn})).await;
        let key = restored
            .get("Key")
            .unwrap_or_else(|| panic!("case {case}: RestoreKey answered no Key: {restored}"));
        assert_eq!(
            string(key, "KeyState"),
            "CREATE_COMPLETE",
            "case {case}: expected CREATE_COMPLETE after RestoreKey, got {key}"
        );
        assert!(
            key.get("DeletePendingTimestamp").is_none(),
            "case {case}: RestoreKey must clear DeletePendingTimestamp: {key}"
        );
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            fetched.get("Key"),
            Some(key),
            "case {case}: GetKey must report the restored key"
        );
        assert_eq!(
            string(fetched.get("Key").expect("a Key"), "KeyCheckValue"),
            string(created.get("Key").expect("a Key"), "KeyCheckValue"),
            "case {case}: the delete/restore round trip must not change the stored key \
             check value"
        );

        // A key that is not pending deletion has nothing to restore.
        let (status, parsed) = call(&router, "RestoreKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            status, 409,
            "case {case}: restoring a key that is not pending deletion must conflict, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConflictException",
            "case {case}: expected ConflictException, got {parsed}"
        );
    }

    // One past each published bound of `DeleteKeyInDays` is refused.
    let created = ok_call(&router, "CreateKey", &create_key_body(&table[0], json!({}))).await;
    let arn = string(created.get("Key").expect("a Key"), "KeyArn").to_string();
    for days in [2, 181] {
        let (status, parsed) = call(
            &router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": days}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case DeleteKeyInDays {days}: outside the modelled 3..180 range, expected \
             400, got {status} {parsed}"
        );
    }

    // The handler does not lean on the modelled range for the value it scales into a
    // duration: the schedule it computes is bounded and its arithmetic is checked, so
    // every value outside 3..=180 — including the extremes of the member's own i32 —
    // answers nothing rather than overflowing.
    let base = SystemTime::now();
    for days in [i32::MIN, -1, 0, 2, 181, 1_000_000, i32::MAX] {
        assert!(
            delete_pending_instant(base, days).is_none(),
            "case DeleteKeyInDays {days}: outside 3..=180, so no deletion instant is \
             computed"
        );
    }
    for days in [3, 7, 180] {
        let expected = base + Duration::from_secs(u64::from(days as u32) * 24 * 60 * 60);
        assert_eq!(
            delete_pending_instant(base, days),
            Some(expected),
            "case DeleteKeyInDays {days}: expected a schedule {days} days out"
        );
    }

    // Deleting and restoring through an alias reaches the same record.
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/deletable", "KeyArn": arn}),
    )
    .await;
    let deleted = ok_call(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": "alias/deletable"}),
    )
    .await;
    assert_eq!(
        string(deleted.get("Key").expect("a Key"), "KeyArn"),
        arn,
        "case alias: DeleteKey through an alias must reach the key it is bound to"
    );
    let restored = ok_call(
        &router,
        "RestoreKey",
        &json!({"KeyIdentifier": "alias/deletable"}),
    )
    .await;
    assert_eq!(
        string(restored.get("Key").expect("a Key"), "KeyState"),
        "CREATE_COMPLETE",
        "case alias: RestoreKey through an alias must restore the bound key"
    );
    let (status, parsed) = call(
        &router,
        "RestoreKey",
        &json!({"KeyIdentifier": format!("arn:aws:payment-cryptography:{REGION}:{ACCOUNT}:key/neverminted00000")}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case RestoreKey unknown: expected 404, got {status} {parsed}"
    );
}

/// Once a key's deletion window has ELAPSED the key reports `DELETE_COMPLETE`, and
/// both `DeleteKey` and `RestoreKey` refuse it with `ConflictException`: there is
/// nothing left to schedule and nothing left to take back.
///
/// The shortest window the model admits is three days (`DeleteKeyInDays` range
/// 3..180), so no request a test can send reaches the elapsed side of the transition
/// in real time; the stored deletion instant is moved into the past instead — exactly
/// the state the clock would have left — and the handlers are then driven over the
/// wire. A key of each key family is checked, not one algorithm.
#[tokio::test]
async fn an_elapsed_deletion_window_refuses_delete_and_restore() {
    let (router, engine) = router_with_engine();
    for row in algorithm_table()
        .iter()
        .filter(|row| ["TDES_3KEY", "AES_256", "HMAC_SHA256", "RSA_2048"].contains(&row.key_algorithm))
    {
        let case = row.key_algorithm;
        let created = ok_call(&router, "CreateKey", &create_key_body(row, json!({}))).await;
        let arn = string(created.get("Key").expect("a Key"), "KeyArn").to_string();
        ok_call(
            &router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": 3}),
        )
        .await;

        // The window ends one second ago.
        {
            let store = engine.state::<KeyStore>();
            let mut guard = store.lock();
            let stored = guard
                .keys
                .get_mut(&arn)
                .unwrap_or_else(|| panic!("case {case}: no key stored under {arn}"));
            stored.delete_pending_timestamp = Some(SystemTime::now() - Duration::from_secs(1));
        }

        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        let key = fetched.get("Key").expect("a Key");
        assert_eq!(
            string(key, "KeyState"),
            "DELETE_COMPLETE",
            "case {case}: an elapsed deletion window reports DELETE_COMPLETE: {key}"
        );
        assert!(
            key.get("DeletePendingTimestamp").is_none() && key.get("DeleteTimestamp").is_some(),
            "case {case}: a deleted key reports DeleteTimestamp and no pending \
             timestamp: {key}"
        );

        for operation in ["DeleteKey", "RestoreKey"] {
            let (status, parsed) = call(&router, operation, &json!({"KeyIdentifier": arn})).await;
            assert_eq!(
                status, 409,
                "case {case}: {operation} on a deleted key must conflict, got {status} \
                 {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ConflictException",
                "case {case}: {operation} expected ConflictException, got {parsed}"
            );
        }

        // And the refusals changed nothing: the key is still reported deleted.
        let after = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        assert_eq!(
            after.get("Key"),
            Some(key),
            "case {case}: a refused DeleteKey or RestoreKey must leave the record alone"
        );
    }
}

/// The key check value computation itself, pinned to published known answers rather
/// than to this emulator's own output.
///
/// The vector is the classic all-zero-key, all-zero-block result of the cipher
/// `KeyCheckValueAlgorithm` names for TDES keys, so it pins the algorithm and the
/// "retain the three highest order bytes" rule rather than restating the
/// implementation: DES of eight zero bytes under an all-zero key is
/// `8CA64DE9C1B123A7`, and a Triple DES EDE bundle of all-zero keys reduces to that
/// same single-DES result, so the ANSI X9.24 check value of an all-zero TDES key is
/// `8CA64D` for both the two-key and the three-key form.
///
/// The CMAC, HMAC and SHA-1 branches are pinned the same way by
/// `the_keyed_check_value_branches_match_their_published_vectors` in `src/state.rs`.
#[test]
fn the_key_check_value_computation_matches_published_vectors() {
    let rows: &[(&str, &str, usize, &str)] = &[
        ("ANSI_X9_24", "TDES_3KEY", 24, "8CA64D"),
        ("ANSI_X9_24", "TDES_2KEY", 16, "8CA64D"),
    ];
    for (kcv_algorithm, key_algorithm, material_len, expected) in rows {
        let material = vec![0u8; *material_len];
        let computed = key_check_value(kcv_algorithm, key_algorithm, &material);
        assert_eq!(
            computed.as_deref(),
            Some(*expected),
            "case {key_algorithm} under {kcv_algorithm}: expected the published \
             {expected}, got {computed:?}"
        );
    }
}

#[tokio::test]
async fn replication_regions_a_create_named_are_persisted_and_reported() {
    let (router, engine) = router_with_engine();
    let table = algorithm_table();

    // A key created with no replication regions follows the account's default
    // replication configuration, which `UsingDefaultReplicationRegions`' own
    // documentation is what reports. This account has enabled none, so the key
    // replicates nowhere and is not a Multi-Region key at all.
    let plain = ok_call(&router, "CreateKey", &create_key_body(&table[0], json!({}))).await;
    let key = plain.get("Key").expect("a Key");
    assert_eq!(
        key.get("UsingDefaultReplicationRegions")
            .and_then(Value::as_bool),
        Some(true),
        "case no regions: a key with no custom replication follows the defaults: {key}"
    );
    for member in ["ReplicationStatus", "MultiRegionKeyType", "PrimaryRegion"] {
        assert!(
            key.get(member).is_none(),
            "case no regions: {member} must be absent for a key that replicates nowhere: {key}"
        );
    }

    // A key created WITH regions carries a custom configuration, that configuration
    // is persisted, and it is the primary of its own Multi-Region hierarchy with one
    // replication entry per region it was asked to replicate to. The states those
    // entries move through are what `key_usage_and_replication` covers.
    for regions in [vec!["us-east-1"], vec!["us-east-2", "eu-west-1"]] {
        let case = format!("{regions:?}");
        let created = ok_call(
            &router,
            "CreateKey",
            &create_key_body(&table[2], json!({"ReplicationRegions": regions.clone()})),
        )
        .await;
        let key = created.get("Key").expect("a Key");
        let arn = string(key, "KeyArn").to_string();
        assert_eq!(
            key.get("UsingDefaultReplicationRegions")
                .and_then(Value::as_bool),
            Some(false),
            "case {case}: a key whose create named its regions overrides the defaults: {key}"
        );
        assert_eq!(
            key.get("MultiRegionKeyType").and_then(Value::as_str),
            Some("PRIMARY"),
            "case {case}: a replicated key is the primary of its hierarchy: {key}"
        );
        let status = key
            .get("ReplicationStatus")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case}: no ReplicationStatus in {key}"));
        let reported: BTreeSet<&str> = status.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = regions.iter().copied().collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected one replication entry per requested region, got {key}"
        );

        // The configuration the create accepted IS stored: read the persisted record
        // straight out of the engine.
        {
            let store = engine.state::<KeyStore>();
            let guard = store.lock();
            let stored = guard
                .keys
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: no key stored under {arn}"));
            let held: BTreeSet<&str> = stored
                .replication_regions
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                held, expected,
                "case {case}: the stored replication regions must be the ones requested"
            );
        }

        // And a later read answers from that record: the same entries, for the same
        // regions.
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        let fetched_key = fetched.get("Key").expect("a Key");
        assert_eq!(
            fetched_key.get("MultiRegionKeyType"),
            key.get("MultiRegionKeyType"),
            "case {case}: GetKey must report the same multi-region role: {fetched}"
        );
        let fetched_regions: BTreeSet<&str> = fetched_key
            .get("ReplicationStatus")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case}: no ReplicationStatus in {fetched}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fetched_regions, expected,
            "case {case}: GetKey must report the stored replication regions: {fetched}"
        );
    }
}

