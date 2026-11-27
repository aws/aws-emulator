// SPDX-License-Identifier: CC0-1.0

//! The data-protection loop of the Payment Cryptography data plane, end to
//! end over the wire: `EncryptData`, `DecryptData`, `ReEncryptData`, `GenerateMac`,
//! `VerifyMac`, `GenerateCardValidationData`, `VerifyCardValidationData`,
//! `GenerateAuthRequestCryptogram`, `VerifyAuthRequestCryptogram`, `GeneratePinData`,
//! `VerifyPinData`, `TranslatePinData`.
//!
//! Every call goes through the real `MultiServiceRouter`, and the keys these cases use
//! are minted by the CONTROL plane's own `CreateKey` / `CreateAlias` on the SAME
//! engine — so what is asserted here is what a signed SDK request observes against a
//! running emulator, over the key material that service stored.
//!
//! The rule under test: each of these operations answers from the ONE persisted key
//! the identifier resolves to — the `KeyArn` is that key's canonical ARN whatever
//! identifier the request named, the `KeyCheckValue` is the one the control plane
//! computed from that key's own secret, and the ciphertext, MAC and card
//! verification value are the published computation over that key's material, so a
//! decrypt inverts an encrypt and a verify recomputes a generate. Each case is one
//! row of a table spanning the class the rule generalises over — every modelled
//! `EncryptionMode`, every symmetric `KeyAlgorithm`, every `MacAlgorithm` with a
//! computation, both published bounds of `PrimaryAccountNumber`, every
//! `ValidationDataLength`, every modelled `SessionKeyDerivation` scheme, both
//! `MajorKeyDerivationMode` options, every modelled `PinBlockFormatForPinData` value
//! and every member of `PinGenerationAttributes` and `PinVerificationAttributes` —
//! rather than the single row the issue named.
//!
//! For the operations that take a data-encryption attribute set the rule has a second
//! half: WHICH KEY the cipher runs under is what the attribute set names — the named key
//! itself for `Symmetric` and `Asymmetric`, and for `Dukpt` and `Emv` the key the
//! published derivation produces from it — so every member of
//! `EncryptionDecryptionAttributes` and of `ReEncryptionAttributes` is served, and the
//! tables below span every modelled `DukptDerivationType`, `DukptKeyVariant`,
//! `DukptEncryptionMode`, `EmvEncryptionMode` and `PaddingType` as well.
//!
//! `TranslatePinData` is the same rule over a PIN rather than over data: a translation
//! reads the PIN out of the incoming block under the incoming key and the incoming ISO
//! 9564 format and writes that same PIN into a block of the outgoing format under the
//! outgoing key, so the table below spans every ordered PAIR of the modelled
//! `TranslationIsoFormats` ISO members and asserts the PIN survived each one.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::ops::types::{
    DUKPT_DERIVATION_TYPE_VALUES, DUKPT_ENCRYPTION_MODE_VALUES, DUKPT_KEY_VARIANT_VALUES,
    EMV_ENCRYPTION_MODE_VALUES, ENCRYPTION_MODE_VALUES, MAC_ALGORITHM_VALUES,
    MAJOR_KEY_DERIVATION_MODE_VALUES, PADDING_TYPE_VALUES, PIN_BLOCK_FORMAT_FOR_PIN_DATA_VALUES,
    VERIFICATION_FAILED_REASON_VALUES,
};
use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::EmulatedService as DataPlane;
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService as ControlPlane;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix of the control plane's `awsJson1_0` surface, from that
/// crate's OPERATIONS.md wire-protocol section.
const CONTROL_TARGET_PREFIX: &str = "PaymentCryptographyControlPlane";

/// One emulator: the production router with BOTH Payment Cryptography services
/// mounted on one shared engine, exactly as `build_router` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator core's business, not this service's.
    router.set_request_authentication(false);
    router.register_service(ControlPlane::new(Arc::clone(&engine)));
    router.register_service(DataPlane::new(Arc::clone(&engine)));
    router
}

/// The SigV4 credential scope both services are routed by.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/payment-cryptography/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Call one CONTROL plane operation (`awsJson1_0`, `POST /` with `X-Amz-Target`).
async fn control(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", authorization())
        .header("host", format!("controlplane.payment-cryptography.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CONTROL_TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    dispatch(router, request, operation).await
}

/// Call one DATA plane operation (`restJson1`, `POST` to the operation's own URI).
async fn data(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization())
        .header("host", format!("dataplane.payment-cryptography.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    dispatch(router, request, uri).await
}

/// Dispatch and read back the status and the JSON body.
async fn dispatch(
    router: &MultiServiceRouter,
    request: http::Request<Bytes>,
    what: &str,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{what} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain for a data plane call.
async fn ok_data(router: &MultiServiceRouter, uri: &str, body: &Value) -> Value {
    let (status, parsed) = data(router, uri, body).await;
    assert_eq!(
        status, 200,
        "case {uri} {body}: expected 200, got {status} {parsed}"
    );
    parsed
}

/// `__type` as the error envelope carries it, with any `shapeId#` prefix dropped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A string member that must be present.
fn string<'a>(value: &'a Value, member: &str) -> &'a str {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
}

/// Percent-encode the reserved characters a key ARN carries, so it fits the single
/// URI label `EncryptData` and `DecryptData` bind it to.
fn encoded(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// One key this suite works with: the identifiers it answers to, and the check value
/// the control plane computed for it.
#[derive(Debug, Clone)]
struct Key {
    arn: String,
    key_check_value: String,
}

/// Mint a key through the control plane's own `CreateKey` and read its ARN and check
/// value back out of the response.
async fn create_key(
    router: &MultiServiceRouter,
    key_algorithm: &str,
    key_usage: &str,
    modes: Value,
) -> Key {
    create_key_of_class(router, "SYMMETRIC_KEY", key_algorithm, key_usage, modes).await
}

/// Mint a key of one `KeyClass` through the control plane's own `CreateKey` and read its
/// ARN and check value back out of the response.
///
/// The class is a parameter because an asymmetric key is a whole key pair
/// (`ASYMMETRIC_KEY_PAIR`) where a symmetric one is a single secret, and the data plane's
/// `Asymmetric` attribute sets run under the former.
async fn create_key_of_class(
    router: &MultiServiceRouter,
    key_class: &str,
    key_algorithm: &str,
    key_usage: &str,
    modes: Value,
) -> Key {
    let body = json!({
        "KeyAttributes": {
            "KeyUsage": key_usage,
            "KeyClass": key_class,
            "KeyAlgorithm": key_algorithm,
            "KeyModesOfUse": modes,
        },
        "Exportable": true,
    });
    let (status, parsed) = control(router, "CreateKey", &body).await;
    assert_eq!(
        status, 200,
        "CreateKey {key_algorithm}/{key_usage}: expected 200, got {status} {parsed}"
    );
    let key = parsed.get("Key").unwrap_or_else(|| {
        panic!("CreateKey {key_algorithm}/{key_usage} answered no Key: {parsed}")
    });
    Key {
        arn: string(key, "KeyArn").to_string(),
        key_check_value: string(key, "KeyCheckValue").to_string(),
    }
}

/// Modes of use for a data encryption key, and for a key that generates and verifies.
fn encrypt_decrypt() -> Value {
    json!({"Encrypt": true, "Decrypt": true})
}
fn generate_verify() -> Value {
    json!({"Generate": true, "Verify": true})
}

/// The `KeyUsage` values this suite creates keys under, from the control plane's
/// modelled `KeyUsage`: the symmetric data encryption key, the ISO 9797 and HMAC MAC
/// keys, and the card verification key.
const USAGE_DATA_ENCRYPTION: &str = "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY";
const USAGE_ISO_9797_3_MAC: &str = "TR31_M3_ISO_9797_3_MAC_KEY";
const USAGE_CMAC: &str = "TR31_M6_ISO_9797_5_CMAC_KEY";
const USAGE_HMAC: &str = "TR31_M7_HMAC_KEY";
const USAGE_CARD_VERIFICATION: &str = "TR31_C0_CARD_VERIFICATION_KEY";

/// Every symmetric `KeyAlgorithm` that names a block cipher, with the block length it
/// works in. Source: the control plane's `SymmetricKeyAlgorithm`, less the HMAC
/// algorithms, which name a hash rather than a cipher.
fn block_cipher_algorithms() -> Vec<(&'static str, usize)> {
    vec![
        ("TDES_2KEY", 8),
        ("TDES_3KEY", 8),
        ("AES_128", 16),
        ("AES_192", 16),
        ("AES_256", 16),
    ]
}

/// A plaintext of `bytes` bytes, as the uppercase hex the model's pattern admits.
fn hex_data(bytes: usize) -> String {
    (0..bytes)
        .map(|index| format!("{:02X}", (index * 7 + 3) % 256))
        .collect()
}

/// The `EncryptData` / `DecryptData` request body for a symmetric call.
fn symmetric_body(member: &str, value: &str, mode: &str, iv: Option<&str>) -> Value {
    let mut attributes = json!({"Mode": mode});
    if let (Some(iv), Value::Object(target)) = (iv, &mut attributes) {
        target.insert("InitializationVector".to_string(), json!(iv));
    }
    json!({member: value, "EncryptionAttributes": {"Symmetric": attributes}})
}

/// The `DecryptData` body, which names its attributes member differently.
fn decrypt_body(cipher_text: &str, mode: &str, iv: Option<&str>) -> Value {
    let mut attributes = json!({"Mode": mode});
    if let (Some(iv), Value::Object(target)) = (iv, &mut attributes) {
        target.insert("InitializationVector".to_string(), json!(iv));
    }
    json!({"CipherText": cipher_text, "DecryptionAttributes": {"Symmetric": attributes}})
}

/// `POST /keys/{KeyIdentifier}/encrypt` for `identifier`.
fn encrypt_uri(identifier: &str) -> String {
    format!("/keys/{}/encrypt", encoded(identifier))
}

/// `POST /keys/{KeyIdentifier}/decrypt` for `identifier`.
fn decrypt_uri(identifier: &str) -> String {
    format!("/keys/{}/decrypt", encoded(identifier))
}

#[tokio::test]
async fn every_modelled_mode_round_trips_under_every_block_cipher_key() {
    let router = emulator();
    // The table spans the whole modelled `EncryptionMode` catalog: a value the model
    // gains later fails this assertion rather than going untested.
    let modes: Vec<&str> = ENCRYPTION_MODE_VALUES.to_vec();
    assert_eq!(
        modes.iter().copied().collect::<BTreeSet<_>>(),
        ["CBC", "CFB", "CFB1", "CFB128", "CFB64", "CFB8", "ECB", "OFB"]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "the mode table must span the modelled EncryptionMode catalog, which is {modes:?}"
    );

    for (key_algorithm, block_len) in block_cipher_algorithms() {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_DATA_ENCRYPTION,
            encrypt_decrypt(),
        )
        .await;
        for mode in &modes {
            // One block and three blocks: the data length rule EncryptData documents
            // is a multiple of the cipher's block size either way.
            for blocks in [1usize, 3] {
                let plain_text = hex_data(block_len * blocks);
                let case = format!("{key_algorithm}/{mode}/{blocks} blocks");
                let (status, parsed) = data(
                    &router,
                    &encrypt_uri(&key.arn),
                    &symmetric_body("PlainText", &plain_text, mode, None),
                )
                .await;
                // A CFB segment wider than the cipher's own block names no mode for
                // that cipher, and is the one combination refused.
                if *mode == "CFB128" && block_len == 8 {
                    assert_eq!(
                        (status, error_code(&parsed).as_str()),
                        (400, "ValidationException"),
                        "case {case}: a 128-bit segment over a 64-bit block must be \
                         refused, got {status} {parsed}"
                    );
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} {parsed}"
                );
                let cipher_text = string(&parsed, "CipherText").to_string();
                assert_eq!(
                    string(&parsed, "KeyArn"),
                    key.arn,
                    "case {case}: the answer must carry the canonical key ARN"
                );
                assert_eq!(
                    string(&parsed, "KeyCheckValue"),
                    key.key_check_value,
                    "case {case}: the answer must carry the key's own check value"
                );
                assert!(
                    cipher_text.len() == plain_text.len()
                        && cipher_text.chars().all(|c| c.is_ascii_hexdigit()),
                    "case {case}: CipherText {cipher_text} must be {} hex characters",
                    plain_text.len()
                );
                assert_ne!(
                    cipher_text, plain_text,
                    "case {case}: the ciphertext must not be the plaintext echoed back"
                );

                let decrypted = ok_data(
                    &router,
                    &decrypt_uri(&key.arn),
                    &decrypt_body(&cipher_text, mode, None),
                )
                .await;
                assert_eq!(
                    string(&decrypted, "PlainText"),
                    plain_text,
                    "case {case}: decrypt must return the plaintext encrypt was given"
                );
                assert_eq!(
                    string(&decrypted, "KeyArn"),
                    key.arn,
                    "case {case}: decrypt must carry the canonical key ARN"
                );
                assert_eq!(
                    string(&decrypted, "KeyCheckValue"),
                    key.key_check_value,
                    "case {case}: decrypt must carry the key's own check value"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_initialization_vector_is_what_the_mode_starts_from() {
    let router = emulator();
    // Every chaining mode starts from the IV, and ECB starts from nothing, so an IV
    // changes the ciphertext of each of the former and of none of the latter.
    for (key_algorithm, block_len) in block_cipher_algorithms() {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_DATA_ENCRYPTION,
            encrypt_decrypt(),
        )
        .await;
        let plain_text = hex_data(block_len * 2);
        let iv = hex_data(block_len);
        for mode in ["CBC", "CFB", "CFB8", "OFB", "ECB"] {
            let case = format!("{key_algorithm}/{mode}");
            let zero_iv = ok_data(
                &router,
                &encrypt_uri(&key.arn),
                &symmetric_body("PlainText", &plain_text, mode, None),
            )
            .await;
            let with_iv = ok_data(
                &router,
                &encrypt_uri(&key.arn),
                &symmetric_body("PlainText", &plain_text, mode, Some(&iv)),
            )
            .await;
            let (zero, nonzero) = (
                string(&zero_iv, "CipherText").to_string(),
                string(&with_iv, "CipherText").to_string(),
            );
            if mode == "ECB" {
                assert_eq!(
                    zero, nonzero,
                    "case {case}: ECB chains nothing, so an IV cannot change its output"
                );
                continue;
            }
            assert_ne!(
                zero, nonzero,
                "case {case}: the IV must change what the mode starts from"
            );
            let round_tripped = ok_data(
                &router,
                &decrypt_uri(&key.arn),
                &decrypt_body(&nonzero, mode, Some(&iv)),
            )
            .await;
            assert_eq!(
                string(&round_tripped, "PlainText"),
                plain_text,
                "case {case}: decrypt under the same IV must return the plaintext"
            );
            let under_default = ok_data(
                &router,
                &decrypt_uri(&key.arn),
                &decrypt_body(&nonzero, mode, None),
            )
            .await;
            assert_ne!(
                string(&under_default, "PlainText"),
                plain_text,
                "case {case}: decrypt under the default IV must not return the plaintext"
            );
        }
    }
}

#[tokio::test]
async fn a_ciphertext_is_bound_to_the_key_and_the_mode_it_was_made_under() {
    let router = emulator();
    let first = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let second = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    assert_ne!(
        first.arn, second.arn,
        "two creates must mint two keys, got {} twice",
        first.arn
    );
    let plain_text = hex_data(32);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&first.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let cipher_text = string(&encrypted, "CipherText").to_string();

    // The same ciphertext under the OTHER key, and under another mode of the same
    // key: each is a different plaintext, and neither is the one that was encrypted.
    for (case, identifier, mode) in [
        ("another key", second.arn.as_str(), "CBC"),
        ("another mode", first.arn.as_str(), "ECB"),
    ] {
        let decrypted = ok_data(
            &router,
            &decrypt_uri(identifier),
            &decrypt_body(&cipher_text, mode, None),
        )
        .await;
        assert_ne!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {case}: must not answer the plaintext the ciphertext was made from"
        );
    }

    // And the same key, mode and plaintext answer the same ciphertext on a later
    // call: the key is persisted, not redrawn per request.
    let again = ok_data(
        &router,
        &encrypt_uri(&first.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&again, "CipherText"),
        cipher_text,
        "a second call under the same key must answer the same ciphertext"
    );
    assert_eq!(
        string(&again, "KeyCheckValue"),
        first.key_check_value,
        "a second call must answer the same key check value"
    );
}

#[tokio::test]
async fn an_alias_resolves_to_the_key_it_is_bound_to() {
    let router = emulator();
    let key = create_key(&router, "AES_256", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let alias = "alias/data-plane-suite";
    let (status, parsed) = control(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias, "KeyArn": key.arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAlias: expected 200, got {status} {parsed}"
    );

    let plain_text = hex_data(16);
    // Both identifier forms the modelled `KeyArnOrKeyAliasType` admits: the key ARN,
    // and the bare alias name.
    let by_arn = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let by_alias = ok_data(
        &router,
        &encrypt_uri(alias),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&by_alias, "KeyArn"),
        key.arn,
        "an alias must answer the canonical key ARN, never the alias itself"
    );
    assert_eq!(
        string(&by_alias, "CipherText"),
        string(&by_arn, "CipherText"),
        "an alias and its key ARN must reach the same key material"
    );
    assert_eq!(
        string(&by_alias, "KeyCheckValue"),
        key.key_check_value,
        "an alias must answer the key's own check value"
    );

    // And through the body-bound identifier of a MAC call, with a key of its own.
    let mac_key = create_key(&router, "TDES_2KEY", USAGE_CMAC, generate_verify()).await;
    let mac_alias = "alias/data-plane-mac";
    let (status, parsed) = control(
        &router,
        "CreateAlias",
        &json!({"AliasName": mac_alias, "KeyArn": mac_key.arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAlias for the MAC key: expected 200, got {status} {parsed}"
    );
    let by_alias = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": mac_alias,
            "MessageData": hex_data(8),
            "GenerationAttributes": {"Algorithm": "CMAC"},
        }),
    )
    .await;
    assert_eq!(
        string(&by_alias, "KeyArn"),
        mac_key.arn,
        "GenerateMac must answer the canonical key ARN for an alias"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_key_is_not_found() {
    let router = emulator();
    let key = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let (account, id) = {
        let tail = key.arn.rsplit_once(":key/").expect("a key ARN");
        let account = tail
            .0
            .rsplit_once(':')
            .expect("the account segment")
            .1
            .to_string();
        (account, tail.1.to_string())
    };
    // Every form the modelled `KeyArnOrKeyAliasType` admits, each naming no key: an
    // id nothing minted, this key's id in another region, an alias ARN and a bare
    // alias that were never created.
    let unknown = vec![
        (
            "an id nothing minted",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:key/{}", "a".repeat(16)),
        ),
        (
            "this key in another region",
            format!("arn:aws:payment-cryptography:eu-west-1:{account}:key/{id}"),
        ),
        (
            "an alias ARN that was never created",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:alias/absent"),
        ),
        ("a bare alias that was never created", "alias/absent".to_string()),
    ];
    for (case, identifier) in unknown {
        let (status, parsed) = data(
            &router,
            &encrypt_uri(&identifier),
            &symmetric_body("PlainText", &hex_data(16), "CBC", None),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case} ({identifier}): expected a 404 ResourceNotFoundException, \
             got {status} {parsed}"
        );
        assert_eq!(
            string(&parsed, "ResourceId"),
            identifier,
            "case {case}: the refusal must name the resource the request asked for"
        );
    }

    // The same refusal from each of the operations that take the identifier in the
    // body, so no operation invents a key of its own.
    let absent = "alias/absent";
    let body_bound = vec![
        (
            "/mac/generate",
            json!({
                "KeyIdentifier": absent,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": "CMAC"},
            }),
        ),
        (
            "/mac/verify",
            json!({
                "KeyIdentifier": absent,
                "MessageData": hex_data(8),
                "Mac": "0011223344556677",
                "VerificationAttributes": {"Algorithm": "CMAC"},
            }),
        ),
        (
            "/cardvalidationdata/generate",
            json!({
                "KeyIdentifier": absent,
                "PrimaryAccountNumber": "4111111111111111",
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
        ),
        (
            "/cardvalidationdata/verify",
            json!({
                "KeyIdentifier": absent,
                "PrimaryAccountNumber": "4111111111111111",
                "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
                "ValidationData": "123",
            }),
        ),
    ];
    for (uri, body) in body_bound {
        let (status, parsed) = data(&router, uri, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {uri}: expected a 404 ResourceNotFoundException, got {status} {parsed}"
        );
    }
}

/// One row of the MAC table: the MAC algorithm, the key algorithm that computes it,
/// and the key usage such a key is created under.
struct MacRow {
    algorithm: &'static str,
    key_algorithm: &'static str,
    key_usage: &'static str,
}

/// Every modelled `MacAlgorithm` this emulator computes, each paired with a key
/// algorithm that can compute it. Source: `MAC_ALGORITHM_VALUES`, which the case below
/// checks this table against, so an algorithm the model gains later is noticed.
fn mac_table() -> Vec<MacRow> {
    let mut rows = Vec::new();
    for key_algorithm in ["TDES_2KEY", "TDES_3KEY"] {
        for algorithm in ["ISO9797_ALGORITHM1", "ISO9797_ALGORITHM3", "CMAC"] {
            rows.push(MacRow {
                algorithm,
                key_algorithm,
                key_usage: USAGE_ISO_9797_3_MAC,
            });
        }
    }
    for key_algorithm in ["AES_128", "AES_192", "AES_256"] {
        rows.push(MacRow {
            algorithm: "CMAC",
            key_algorithm,
            key_usage: USAGE_CMAC,
        });
    }
    for key_algorithm in [
        "HMAC_SHA224",
        "HMAC_SHA256",
        "HMAC_SHA384",
        "HMAC_SHA512",
    ] {
        // The explicit algorithm, and `HMAC`, which names whichever hash the key
        // selects.
        for algorithm in [key_algorithm, "HMAC"] {
            rows.push(MacRow {
                algorithm,
                key_algorithm,
                key_usage: USAGE_HMAC,
            });
        }
    }
    rows
}

#[tokio::test]
async fn a_mac_verifies_only_against_what_generated_it() {
    let router = emulator();
    let message = hex_data(16);
    let tampered = {
        // One nibble of the message flipped: the same length, one bit of content
        // different.
        let mut flipped = message.clone();
        flipped.replace_range(0..1, if message.starts_with('0') { "1" } else { "0" });
        flipped
    };
    let computed: BTreeSet<&str> = mac_table().iter().map(|row| row.algorithm).collect();
    let uncomputed: Vec<&&str> = MAC_ALGORITHM_VALUES
        .iter()
        .filter(|algorithm| !computed.contains(**algorithm))
        .collect();
    assert_eq!(
        uncomputed,
        vec![&"AS2805_4_1"],
        "every modelled MacAlgorithm but AS2805_4_1 must have a row, missing {uncomputed:?}"
    );

    for row in mac_table() {
        let case = format!("{}/{}", row.key_algorithm, row.algorithm);
        let key = create_key(
            &router,
            row.key_algorithm,
            row.key_usage,
            generate_verify(),
        )
        .await;
        let generate = |message: String, mac_length: Option<i32>| {
            let mut body = json!({
                "KeyIdentifier": key.arn,
                "MessageData": message,
                "GenerationAttributes": {"Algorithm": row.algorithm},
            });
            if let (Some(length), Value::Object(target)) = (mac_length, &mut body) {
                target.insert("MacLength".to_string(), json!(length));
            }
            body
        };

        let generated = ok_data(&router, "/mac/generate", &generate(message.clone(), None)).await;
        let mac = string(&generated, "Mac").to_string();
        assert!(
            (4..=128).contains(&mac.len()) && mac.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case}: Mac {mac} must be 4..128 hex characters"
        );
        assert_eq!(
            string(&generated, "KeyArn"),
            key.arn,
            "case {case}: GenerateMac must answer the canonical key ARN"
        );
        assert_eq!(
            string(&generated, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: GenerateMac must answer the key's own check value"
        );
        let again = ok_data(&router, "/mac/generate", &generate(message.clone(), None)).await;
        assert_eq!(
            string(&again, "Mac"),
            mac,
            "case {case}: the MAC must be a function of key, message and algorithm"
        );
        let other_message =
            ok_data(&router, "/mac/generate", &generate(tampered.clone(), None)).await;
        assert_ne!(
            string(&other_message, "Mac"),
            mac,
            "case {case}: a different message must not answer the same MAC"
        );

        let verify = |message: &str, mac: &str, mac_length: Option<i32>| {
            let mut body = json!({
                "KeyIdentifier": key.arn,
                "MessageData": message,
                "Mac": mac,
                "VerificationAttributes": {"Algorithm": row.algorithm},
            });
            if let (Some(length), Value::Object(target)) = (mac_length, &mut body) {
                target.insert("MacLength".to_string(), json!(length));
            }
            body
        };
        let verified = ok_data(&router, "/mac/verify", &verify(&message, &mac, None)).await;
        assert_eq!(
            string(&verified, "KeyArn"),
            key.arn,
            "case {case}: VerifyMac must answer the canonical key ARN"
        );
        assert_eq!(
            string(&verified, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: VerifyMac must answer the key's own check value"
        );
        // A MAC that is lowercase hex is the same MAC.
        ok_data(
            &router,
            "/mac/verify",
            &verify(&message, &mac.to_lowercase(), None),
        )
        .await;

        // Each way a verification can fail, each the modelled
        // VerificationFailedException with the INVALID_MAC reason.
        let same_length_other_mac = {
            let mut other = mac.clone();
            other.replace_range(0..1, if mac.starts_with('0') { "1" } else { "0" });
            other
        };
        for (why, message_data, presented) in [
            ("a tampered message", tampered.as_str(), mac.as_str()),
            ("a tampered MAC", message.as_str(), same_length_other_mac.as_str()),
        ] {
            let (status, parsed) = data(
                &router,
                "/mac/verify",
                &verify(message_data, presented, None),
            )
            .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "VerificationFailedException"),
                "case {case} with {why}: expected a 400 VerificationFailedException, \
                 got {status} {parsed}"
            );
            assert_eq!(
                string(&parsed, "Reason"),
                "INVALID_MAC",
                "case {case} with {why}: the refusal must name the reason"
            );
            assert!(
                VERIFICATION_FAILED_REASON_VALUES.contains(&string(&parsed, "Reason")),
                "case {case} with {why}: the reason must be a modelled one"
            );
            assert!(
                !string(&parsed, "Message").is_empty(),
                "case {case} with {why}: the refusal must carry a message"
            );
        }

        // `MacLength` cuts the MAC it asks for, and the cut MAC verifies under the
        // same length and under no other.
        let full_length = i32::try_from(mac.len()).expect("a MAC length fits an i32");
        for mac_length in [4, 8, full_length.min(32)] {
            let truncated =
                ok_data(&router, "/mac/generate", &generate(message.clone(), Some(mac_length))).await;
            let short = string(&truncated, "Mac").to_string();
            assert_eq!(
                i32::try_from(short.len()).unwrap_or_default(),
                mac_length,
                "case {case}: MacLength {mac_length} must cut the MAC to that many \
                 hexadecimal characters, got {short}"
            );
            assert!(
                mac.starts_with(&short),
                "case {case}: the cut MAC must be the leading characters of {mac}, got {short}"
            );
            ok_data(
                &router,
                "/mac/verify",
                &verify(&message, &short, Some(mac_length)),
            )
            .await;
        }
        // A length longer than the MAC the algorithm computes names no MAC.
        if full_length < 32 {
            let (status, parsed) = data(
                &router,
                "/mac/generate",
                &generate(message.clone(), Some(32)),
            )
            .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: a MacLength longer than the MAC must be refused, \
                 got {status} {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_mac_algorithm_the_key_cannot_compute_is_refused() {
    let router = emulator();
    // The algorithm has to be one the KEY can compute: an HMAC needs the HMAC key
    // whose own algorithm selects that hash, and the Triple DES CBC-MACs need a
    // Triple DES key.
    let rows = vec![
        ("AES_128", USAGE_CMAC, "HMAC_SHA256"),
        ("AES_128", USAGE_CMAC, "ISO9797_ALGORITHM1"),
        ("AES_128", USAGE_CMAC, "ISO9797_ALGORITHM3"),
        ("HMAC_SHA256", USAGE_HMAC, "HMAC_SHA512"),
        ("HMAC_SHA256", USAGE_HMAC, "CMAC"),
        ("TDES_2KEY", USAGE_ISO_9797_3_MAC, "HMAC"),
    ];
    for (key_algorithm, key_usage, algorithm) in rows {
        let key = create_key(&router, key_algorithm, key_usage, generate_verify()).await;
        let (status, parsed) = data(
            &router,
            "/mac/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": algorithm},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {key_algorithm}/{algorithm}: expected a 400 ValidationException, \
             got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn a_key_whose_modes_of_use_exclude_the_operation_is_refused() {
    let router = emulator();
    // Each operation's own documentation names the mode of use it needs: Encrypt,
    // Decrypt, Generate, Verify. A key created without it cannot perform it.
    let encrypt_only = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"Encrypt": true}),
    )
    .await;
    let plain_text = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&encrypt_only.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let (status, parsed) = data(
        &router,
        &decrypt_uri(&encrypt_only.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key without Decrypt must not decrypt, got {status} {parsed}"
    );

    let generate_only = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true}),
    )
    .await;
    let generated = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": generate_only.arn,
            "MessageData": plain_text,
            "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM3"},
        }),
    )
    .await;
    let (status, parsed) = data(
        &router,
        "/mac/verify",
        &json!({
            "KeyIdentifier": generate_only.arn,
            "MessageData": plain_text,
            "Mac": string(&generated, "Mac"),
            "VerificationAttributes": {"Algorithm": "ISO9797_ALGORITHM3"},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key without Verify must not verify, got {status} {parsed}"
    );

    // And a key with NoRestrictions carries every mode.
    let unrestricted = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"NoRestrictions": true}),
    )
    .await;
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&unrestricted.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&unrestricted.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    assert_eq!(
        string(&decrypted, "PlainText"),
        plain_text,
        "a key with NoRestrictions must encrypt and decrypt"
    );
}

#[tokio::test]
async fn card_validation_data_verifies_only_against_the_card_it_was_generated_for() {
    let router = emulator();
    // The published bounds of `PrimaryAccountNumber` (12..19 digits) and one length
    // between them, under each Triple DES card verification key algorithm, for both
    // attribute sets the computation is defined for, at every modelled
    // `ValidationDataLength` and at the default.
    for key_algorithm in ["TDES_2KEY", "TDES_3KEY"] {
        let key = create_key(
            &router,
            key_algorithm,
            USAGE_CARD_VERIFICATION,
            generate_verify(),
        )
        .await;
        for pan in ["411111111111", "4111111111111111", "4111111111111111111"] {
            for attributes in [
                json!({"CardVerificationValue1": {"CardExpiryDate": "0123", "ServiceCode": "101"}}),
                json!({"CardVerificationValue2": {"CardExpiryDate": "0123"}}),
            ] {
                for length in [None, Some(3), Some(4), Some(5)] {
                    let case =
                        format!("{key_algorithm}/{}/{attributes}/{length:?}", pan.len());
                    let mut body = json!({
                        "KeyIdentifier": key.arn,
                        "PrimaryAccountNumber": pan,
                        "GenerationAttributes": attributes.clone(),
                    });
                    if let (Some(length), Value::Object(target)) = (length, &mut body) {
                        target.insert("ValidationDataLength".to_string(), json!(length));
                    }
                    let generated =
                        ok_data(&router, "/cardvalidationdata/generate", &body).await;
                    let validation_data = string(&generated, "ValidationData").to_string();
                    // "The default value is 3" when the request names no length.
                    let expected_length = length.unwrap_or(3);
                    assert_eq!(
                        i32::try_from(validation_data.len()).unwrap_or_default(),
                        expected_length,
                        "case {case}: ValidationData {validation_data} must be \
                         {expected_length} digits"
                    );
                    assert!(
                        validation_data.bytes().all(|b| b.is_ascii_digit()),
                        "case {case}: ValidationData {validation_data} must be digits"
                    );
                    assert_eq!(
                        string(&generated, "KeyArn"),
                        key.arn,
                        "case {case}: the answer must carry the canonical key ARN"
                    );
                    assert_eq!(
                        string(&generated, "KeyCheckValue"),
                        key.key_check_value,
                        "case {case}: the answer must carry the key's own check value"
                    );

                    let verify = |pan: &str, value: &str, attributes: &Value| {
                        json!({
                            "KeyIdentifier": key.arn,
                            "PrimaryAccountNumber": pan,
                            "VerificationAttributes": attributes.clone(),
                            "ValidationData": value,
                        })
                    };
                    let verified = ok_data(
                        &router,
                        "/cardvalidationdata/verify",
                        &verify(pan, &validation_data, &attributes),
                    )
                    .await;
                    assert_eq!(
                        string(&verified, "KeyCheckValue"),
                        key.key_check_value,
                        "case {case}: verify must carry the key's own check value"
                    );

                    // Each way the verification fails: another value of the same
                    // length, another account number, another expiry date. The two
                    // that change the CARD DATA are judged against what the key
                    // computes for that data, so a legitimate collision on a short
                    // value is a pass rather than a flake.
                    let other_value: String = validation_data
                        .chars()
                        .map(|digit| if digit == '0' { '1' } else { '0' })
                        .collect();
                    let other_pan = format!("5{}", &pan[1..]);
                    let other_attributes = if attributes.get("CardVerificationValue1").is_some() {
                        json!({"CardVerificationValue1": {"CardExpiryDate": "0124", "ServiceCode": "101"}})
                    } else {
                        json!({"CardVerificationValue2": {"CardExpiryDate": "0124"}})
                    };
                    let for_other_pan =
                        generated_validation_data(&router, &key.arn, &other_pan, &attributes, length)
                            .await;
                    let for_other_expiry = generated_validation_data(
                        &router,
                        &key.arn,
                        pan,
                        &other_attributes,
                        length,
                    )
                    .await;
                    for (why, body, presented, computed) in [
                        (
                            "another value",
                            verify(pan, &other_value, &attributes),
                            other_value.as_str(),
                            validation_data.as_str(),
                        ),
                        (
                            "another account number",
                            verify(&other_pan, &validation_data, &attributes),
                            validation_data.as_str(),
                            for_other_pan.as_str(),
                        ),
                        (
                            "another expiry date",
                            verify(pan, &validation_data, &other_attributes),
                            validation_data.as_str(),
                            for_other_expiry.as_str(),
                        ),
                    ] {
                        let (status, parsed) =
                            data(&router, "/cardvalidationdata/verify", &body).await;
                        if presented == computed {
                            assert_eq!(
                                status, 200,
                                "case {case} with {why}: the key computes {computed} for \
                                 this card data too, so it must verify, got {status} {parsed}"
                            );
                            continue;
                        }
                        assert_eq!(
                            (status, error_code(&parsed).as_str()),
                            (400, "VerificationFailedException"),
                            "case {case} with {why}: expected a 400 \
                             VerificationFailedException for {presented} against \
                             {computed}, got {status} {parsed}"
                        );
                        assert_eq!(
                            string(&parsed, "Reason"),
                            "INVALID_VALIDATION_DATA",
                            "case {case} with {why}: the refusal must name the reason"
                        );
                    }
                }
            }
        }
    }
}

/// What the key computes for this card data, read back through
/// `GenerateCardValidationData`.
///
/// A case that changes the card data compares against THIS rather than assuming two
/// different cards never produce the same value: a three-digit value collides about
/// once in a thousand, and the rule under test is that verification follows the
/// computation.
async fn generated_validation_data(
    router: &MultiServiceRouter,
    key_arn: &str,
    pan: &str,
    attributes: &Value,
    length: Option<i32>,
) -> String {
    let mut body = json!({
        "KeyIdentifier": key_arn,
        "PrimaryAccountNumber": pan,
        "GenerationAttributes": attributes.clone(),
    });
    if let (Some(length), Value::Object(target)) = (length, &mut body) {
        target.insert("ValidationDataLength".to_string(), json!(length));
    }
    let generated = ok_data(router, "/cardvalidationdata/generate", &body).await;
    string(&generated, "ValidationData").to_string()
}

#[tokio::test]
async fn card_data_operations_need_a_triple_des_card_verification_key() {
    let router = emulator();
    // "a CVK (Card Verification Key) encryption key is required"
    // (`GenerateCardValidationData`): a key of another usage, and a card
    // verification key of an algorithm the computation is not defined for, are each
    // refused rather than run under the wrong key.
    let rows = vec![
        ("another key usage", "TDES_2KEY", USAGE_DATA_ENCRYPTION),
        ("an AES card verification key", "AES_128", USAGE_CARD_VERIFICATION),
    ];
    for (case, key_algorithm, key_usage) in rows {
        let key = create_key(
            &router,
            key_algorithm,
            key_usage,
            json!({"Encrypt": true, "Decrypt": true, "Generate": true, "Verify": true}),
        )
        .await;
        let (status, parsed) = data(
            &router,
            "/cardvalidationdata/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "4111111111111111",
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: expected a 400 ValidationException, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn one_key_reports_one_check_value_to_every_operation() {
    let router = emulator();
    // The key check value is a property of the key, so every operation that can use a
    // key reports the value the control plane's `GetKey` reports for it.
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_CARD_VERIFICATION,
        json!({"Encrypt": true, "Decrypt": true, "Generate": true, "Verify": true}),
    )
    .await;
    let (status, parsed) = control(&router, "GetKey", &json!({"KeyIdentifier": key.arn})).await;
    assert_eq!(status, 200, "GetKey: expected 200, got {status} {parsed}");
    let reported = string(
        parsed.get("Key").expect("GetKey answers a Key"),
        "KeyCheckValue",
    )
    .to_string();
    assert_eq!(
        reported, key.key_check_value,
        "GetKey must report the check value CreateKey answered"
    );

    let message = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &message, "CBC", None),
    )
    .await;
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&key.arn),
        &decrypt_body(string(&encrypted, "CipherText"), "CBC", None),
    )
    .await;
    let mac = ok_data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": message,
            "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
        }),
    )
    .await;
    let verified_mac = ok_data(
        &router,
        "/mac/verify",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": message,
            "Mac": string(&mac, "Mac"),
            "VerificationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
        }),
    )
    .await;
    let card = ok_data(
        &router,
        "/cardvalidationdata/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "PrimaryAccountNumber": "4111111111111111",
            "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
        }),
    )
    .await;
    let verified_card = ok_data(
        &router,
        "/cardvalidationdata/verify",
        &json!({
            "KeyIdentifier": key.arn,
            "PrimaryAccountNumber": "4111111111111111",
            "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            "ValidationData": string(&card, "ValidationData"),
        }),
    )
    .await;
    for (operation, answered) in [
        ("EncryptData", &encrypted),
        ("DecryptData", &decrypted),
        ("GenerateMac", &mac),
        ("VerifyMac", &verified_mac),
        ("GenerateCardValidationData", &card),
        ("VerifyCardValidationData", &verified_card),
    ] {
        assert_eq!(
            string(answered, "KeyCheckValue"),
            reported,
            "case {operation}: must report the key's one check value"
        );
        assert_eq!(
            string(answered, "KeyArn"),
            key.arn,
            "case {operation}: must report the key's canonical ARN"
        );
    }
}

#[tokio::test]
async fn a_key_the_control_plane_has_scheduled_for_deletion_cannot_be_used() {
    let router = emulator();
    // The key the data plane reads is the LIVE control-plane record, not a copy: a key
    // scheduled for deletion there cannot be used here, and restoring it there makes
    // it usable again.
    let key = create_key(&router, "AES_128", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let plain_text = hex_data(16);
    let encrypted = ok_data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    let cipher_text = string(&encrypted, "CipherText").to_string();

    let (status, parsed) = control(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": key.arn, "DeleteKeyInDays": 3}),
    )
    .await;
    assert_eq!(status, 200, "DeleteKey: expected 200, got {status} {parsed}");
    let (status, parsed) = data(
        &router,
        &encrypt_uri(&key.arn),
        &symmetric_body("PlainText", &plain_text, "CBC", None),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a key waiting to be deleted must not encrypt, got {status} {parsed}"
    );

    let (status, parsed) = control(&router, "RestoreKey", &json!({"KeyIdentifier": key.arn})).await;
    assert_eq!(status, 200, "RestoreKey: expected 200, got {status} {parsed}");
    let decrypted = ok_data(
        &router,
        &decrypt_uri(&key.arn),
        &decrypt_body(&cipher_text, "CBC", None),
    )
    .await;
    assert_eq!(
        string(&decrypted, "PlainText"),
        plain_text,
        "a restored key must run the same computation it ran before"
    );
}

#[tokio::test]
async fn an_attribute_set_with_no_computation_here_still_says_so() {
    let router = emulator();
    // The attribute sets this slice does not compute answer 501 — "nobody has written
    // it yet" — rather than a value no documented algorithm produced. Each is the
    // record of what the next slice of this service has to implement.
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true, "DeriveKey": true}),
    )
    .await;
    let (status, parsed) = data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": hex_data(8),
            "GenerationAttributes": {"Algorithm": "AS2805_4_1"},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (501, "NotImplemented"),
        "an algorithm with no computation here must say so, got {status} {parsed}"
    );

    let (status, parsed) = data(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": hex_data(8),
            "GenerationAttributes": {
                "DukptCmac": {"KeySerialNumber": "FFFF9876543210E00008", "DukptKeyVariant": "BIDIRECTIONAL"},
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (501, "NotImplemented"),
        "a DUKPT derivation must say so, got {status} {parsed}"
    );
}

/// The `KeyUsage` an EMV issuer master key for application cryptograms carries, from
/// the control plane's modelled `KeyUsage`: the IMK-AC the auth request cryptogram
/// operations' own documentation requires.
const USAGE_EMV_APP_CRYPTOGRAMS: &str = "TR31_E0_EMV_MKEY_APP_CRYPTOGRAMS";

/// Modes of use for a key whose job is deriving other keys, which is what the cryptogram
/// operations do with an issuer master key.
fn derive_key() -> Value {
    json!({"DeriveKey": true})
}

/// `POST /cryptogram/generate`, the URI the model binds `GenerateAuthRequestCryptogram`
/// to.
const GENERATE_CRYPTOGRAM_URI: &str = "/cryptogram/generate";

/// `POST /cryptogram/verify`, the URI the model binds `VerifyAuthRequestCryptogram` to.
const VERIFY_CRYPTOGRAM_URI: &str = "/cryptogram/verify";

/// The `KeyAlgorithm` values an EMV issuer master key can carry: the Triple DES
/// algorithms, since the derivation produces a double-length Triple DES key. Source:
/// the control plane's `SymmetricKeyAlgorithm`, less the AES and HMAC algorithms, which
/// name no Triple DES cipher.
const ISSUER_MASTER_KEY_ALGORITHMS: &[&str] = &["TDES_2KEY", "TDES_3KEY"];

/// Every modelled `SessionKeyDerivation` member, with the attributes that member
/// carries for `pan`. Source:
/// `smithy_query(query="shape",shape="SessionKeyDerivation")` and the members of each
/// target shape — the EMV common, Mastercard, EMV2000 and UnionPay members carry a
/// transaction counter, Mastercard's an unpredictable number as well, and the American
/// Express and Visa members the card alone.
fn session_key_schemes(pan: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "EmvCommon",
            json!({"EmvCommon": {
                "PrimaryAccountNumber": pan,
                "PanSequenceNumber": "01",
                "ApplicationTransactionCounter": "006D",
            }}),
        ),
        (
            "Mastercard",
            json!({"Mastercard": {
                "PrimaryAccountNumber": pan,
                "PanSequenceNumber": "01",
                "ApplicationTransactionCounter": "006D",
                "UnpredictableNumber": "1A2B3C4D",
            }}),
        ),
        (
            "Emv2000",
            json!({"Emv2000": {
                "PrimaryAccountNumber": pan,
                "PanSequenceNumber": "01",
                "ApplicationTransactionCounter": "006D",
            }}),
        ),
        (
            "Amex",
            json!({"Amex": {"PrimaryAccountNumber": pan, "PanSequenceNumber": "01"}}),
        ),
        (
            "Visa",
            json!({"Visa": {"PrimaryAccountNumber": pan, "PanSequenceNumber": "01"}}),
        ),
        (
            "UnionPay",
            json!({"UnionPay": {
                "PrimaryAccountNumber": pan,
                "PanSequenceNumber": "01",
                "ApplicationTransactionCounter": "006D",
            }}),
        ),
    ]
}

/// The `GenerateAuthRequestCryptogram` request body.
fn generate_cryptogram_body(
    key_identifier: &str,
    mode: &str,
    attributes: &Value,
    transaction_data: &str,
) -> Value {
    json!({
        "KeyIdentifier": key_identifier,
        "MajorKeyDerivationMode": mode,
        "SessionKeyDerivationAttributes": attributes.clone(),
        "TransactionData": transaction_data,
    })
}

/// The `VerifyAuthRequestCryptogram` request body, with the response attributes a
/// request asking for an ARPC carries.
fn verify_cryptogram_body(
    key_identifier: &str,
    mode: &str,
    attributes: &Value,
    transaction_data: &str,
    cryptogram: &str,
    response: Option<&Value>,
) -> Value {
    let mut body = generate_cryptogram_body(key_identifier, mode, attributes, transaction_data);
    if let Value::Object(target) = &mut body {
        target.insert(
            "AuthRequestCryptogram".to_string(),
            json!(cryptogram.to_string()),
        );
        if let Some(response) = response {
            target.insert("AuthResponseAttributes".to_string(), response.clone());
        }
    }
    body
}

#[tokio::test]
async fn every_session_key_scheme_generates_a_cryptogram_its_verify_accepts() {
    let router = emulator();
    // The table spans both modelled `MajorKeyDerivationMode` options: one the model
    // gains later fails this assertion rather than going untested.
    let modes: Vec<&str> = MAJOR_KEY_DERIVATION_MODE_VALUES.to_vec();
    assert_eq!(
        modes.iter().copied().collect::<BTreeSet<_>>(),
        ["EMV_OPTION_A", "EMV_OPTION_B"]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "the mode table must span the modelled MajorKeyDerivationMode catalog, which is \
         {modes:?}"
    );
    // The published bounds of `PrimaryAccountNumber` (length 12..19), so the
    // diversification value is exercised both shorter and longer than the sixteen digits
    // it carries.
    for pan in ["411111111111", "4111111111111111", "4111111111111111999"] {
        for key_algorithm in ISSUER_MASTER_KEY_ALGORITHMS {
            let key = create_key(
                &router,
                key_algorithm,
                USAGE_EMV_APP_CRYPTOGRAMS,
                derive_key(),
            )
            .await;
            for mode in &modes {
                // The transaction data's own length is free (`TransactionDataType`,
                // length 2..1024), so a short, a block-sized and a multi-block value
                // each have to compute.
                for bytes in [1usize, 8, 24] {
                    let transaction_data = hex_data(bytes);
                    for (scheme, attributes) in session_key_schemes(pan) {
                        let case =
                            format!("{pan}/{key_algorithm}/{mode}/{scheme}/{bytes} bytes");
                        let generated = ok_data(
                            &router,
                            GENERATE_CRYPTOGRAM_URI,
                            &generate_cryptogram_body(
                                &key.arn,
                                mode,
                                &attributes,
                                &transaction_data,
                            ),
                        )
                        .await;
                        let cryptogram =
                            string(&generated, "AuthRequestCryptogram").to_string();
                        // `AuthRequestCryptogramType` is exactly sixteen hexadecimal
                        // characters: the eight-byte EMV MAC.
                        assert!(
                            cryptogram.len() == 16
                                && cryptogram.chars().all(|c| c.is_ascii_hexdigit()),
                            "case {case}: AuthRequestCryptogram {cryptogram} must be 16 \
                             hexadecimal characters"
                        );
                        assert_eq!(
                            string(&generated, "KeyArn"),
                            key.arn,
                            "case {case}: the answer must carry the canonical key ARN"
                        );
                        assert_eq!(
                            string(&generated, "KeyCheckValue"),
                            key.key_check_value,
                            "case {case}: the answer must carry the key's own check value"
                        );

                        // The cryptogram is a function of the key and the request, so
                        // the same call answers the same value.
                        let again = ok_data(
                            &router,
                            GENERATE_CRYPTOGRAM_URI,
                            &generate_cryptogram_body(
                                &key.arn,
                                mode,
                                &attributes,
                                &transaction_data,
                            ),
                        )
                        .await;
                        assert_eq!(
                            string(&again, "AuthRequestCryptogram"),
                            cryptogram,
                            "case {case}: the same request must reproduce the cryptogram"
                        );

                        let verified = ok_data(
                            &router,
                            VERIFY_CRYPTOGRAM_URI,
                            &verify_cryptogram_body(
                                &key.arn,
                                mode,
                                &attributes,
                                &transaction_data,
                                &cryptogram,
                                None,
                            ),
                        )
                        .await;
                        assert_eq!(
                            string(&verified, "KeyArn"),
                            key.arn,
                            "case {case}: verify must carry the canonical key ARN"
                        );
                        assert_eq!(
                            string(&verified, "KeyCheckValue"),
                            key.key_check_value,
                            "case {case}: verify must carry the key's own check value"
                        );
                        assert!(
                            verified.get("AuthResponseValue").is_none(),
                            "case {case}: a request asking for no ARPC must be answered \
                             none, got {verified}"
                        );
                        // The cryptogram the caller presents is hexadecimal, so the case
                        // it is written in cannot change the verdict.
                        ok_data(
                            &router,
                            VERIFY_CRYPTOGRAM_URI,
                            &verify_cryptogram_body(
                                &key.arn,
                                mode,
                                &attributes,
                                &transaction_data,
                                &cryptogram.to_lowercase(),
                                None,
                            ),
                        )
                        .await;
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn each_derivation_is_its_own_computation() {
    let router = emulator();
    // What the cryptogram is computed from is the key, the derivation mode and the
    // transaction's own data: change any one of them and the cryptogram changes. The
    // schemes compared here are the four with distinct derivations — the two that carry
    // no transaction counter compute under the card's master key itself, and the two
    // that carry a counter alone diversify by the same value.
    let pan = "4111111111111111";
    let transaction_data = hex_data(8);
    let keys = [
        create_key(
            &router,
            "TDES_2KEY",
            USAGE_EMV_APP_CRYPTOGRAMS,
            derive_key(),
        )
        .await,
        create_key(
            &router,
            "TDES_2KEY",
            USAGE_EMV_APP_CRYPTOGRAMS,
            derive_key(),
        )
        .await,
    ];
    let mut answered: Vec<(String, String)> = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        for mode in ["EMV_OPTION_A", "EMV_OPTION_B"] {
            for (scheme, attributes) in session_key_schemes(pan) {
                if !["EmvCommon", "Mastercard", "Emv2000", "Amex"].contains(&scheme) {
                    continue;
                }
                let generated = ok_data(
                    &router,
                    GENERATE_CRYPTOGRAM_URI,
                    &generate_cryptogram_body(&key.arn, mode, &attributes, &transaction_data),
                )
                .await;
                answered.push((
                    format!("key {index}/{mode}/{scheme}"),
                    string(&generated, "AuthRequestCryptogram").to_string(),
                ));
            }
        }
    }
    let distinct: BTreeSet<&String> = answered.iter().map(|(_, value)| value).collect();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "each key, derivation mode and scheme must compute its own cryptogram, got \
         {answered:?}"
    );

    // And the transaction data itself: one changed character changes the cryptogram.
    let key = &keys[0];
    let (_, attributes) = session_key_schemes(pan)
        .into_iter()
        .find(|(scheme, _)| *scheme == "EmvCommon")
        .expect("the EMV common scheme is in the table");
    let first = ok_data(
        &router,
        GENERATE_CRYPTOGRAM_URI,
        &generate_cryptogram_body(&key.arn, "EMV_OPTION_A", &attributes, &transaction_data),
    )
    .await;
    let longer = ok_data(
        &router,
        GENERATE_CRYPTOGRAM_URI,
        &generate_cryptogram_body(
            &key.arn,
            "EMV_OPTION_A",
            &attributes,
            &format!("{transaction_data}FF"),
        ),
    )
    .await;
    assert_ne!(
        string(&first, "AuthRequestCryptogram"),
        string(&longer, "AuthRequestCryptogram"),
        "the transaction data must change the cryptogram"
    );
}

#[tokio::test]
async fn a_cryptogram_that_the_transaction_did_not_produce_is_refused() {
    let router = emulator();
    // A verification compares what the transaction computes with what the caller
    // presented: a cryptogram that was tampered with, or one computed over any other
    // transaction, key or derivation, fails verification rather than being accepted.
    let pan = "4111111111111111";
    let other_pan = "5500000000000004";
    let transaction_data = hex_data(8);
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_EMV_APP_CRYPTOGRAMS,
        derive_key(),
    )
    .await;
    let other_key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_EMV_APP_CRYPTOGRAMS,
        derive_key(),
    )
    .await;
    let schemes = session_key_schemes(pan);
    let (_, attributes) = schemes
        .iter()
        .find(|(scheme, _)| *scheme == "EmvCommon")
        .expect("the EMV common scheme is in the table");
    let generated = ok_data(
        &router,
        GENERATE_CRYPTOGRAM_URI,
        &generate_cryptogram_body(&key.arn, "EMV_OPTION_A", attributes, &transaction_data),
    )
    .await;
    let cryptogram = string(&generated, "AuthRequestCryptogram").to_string();

    let tampered = format!(
        "{}{}",
        &cryptogram[..15],
        if cryptogram.ends_with('0') { '1' } else { '0' }
    );
    let other_counter = json!({"EmvCommon": {
        "PrimaryAccountNumber": pan,
        "PanSequenceNumber": "01",
        "ApplicationTransactionCounter": "006E",
    }});
    let other_card = json!({"EmvCommon": {
        "PrimaryAccountNumber": other_pan,
        "PanSequenceNumber": "01",
        "ApplicationTransactionCounter": "006D",
    }});
    let other_sequence = json!({"EmvCommon": {
        "PrimaryAccountNumber": pan,
        "PanSequenceNumber": "02",
        "ApplicationTransactionCounter": "006D",
    }});
    let (_, mastercard) = schemes
        .iter()
        .find(|(scheme, _)| *scheme == "Mastercard")
        .expect("the Mastercard scheme is in the table");
    // (case, the key, the mode, the attributes, the transaction data, the cryptogram)
    let rows: Vec<(&str, &str, &str, &Value, String, String)> = vec![
        (
            "a tampered cryptogram",
            &key.arn,
            "EMV_OPTION_A",
            attributes,
            transaction_data.clone(),
            tampered,
        ),
        (
            "another transaction's data",
            &key.arn,
            "EMV_OPTION_A",
            attributes,
            hex_data(9),
            cryptogram.clone(),
        ),
        (
            "another transaction counter",
            &key.arn,
            "EMV_OPTION_A",
            &other_counter,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
        (
            "another card",
            &key.arn,
            "EMV_OPTION_A",
            &other_card,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
        (
            "another card sequence number",
            &key.arn,
            "EMV_OPTION_A",
            &other_sequence,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
        (
            "the other major key derivation mode",
            &key.arn,
            "EMV_OPTION_B",
            attributes,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
        (
            "another session key derivation scheme",
            &key.arn,
            "EMV_OPTION_A",
            mastercard,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
        (
            "another issuer master key",
            &other_key.arn,
            "EMV_OPTION_A",
            attributes,
            transaction_data.clone(),
            cryptogram.clone(),
        ),
    ];
    for (case, key_arn, mode, attributes, transaction, presented) in rows {
        let (status, parsed) = data(
            &router,
            VERIFY_CRYPTOGRAM_URI,
            &verify_cryptogram_body(key_arn, mode, attributes, &transaction, &presented, None),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "VerificationFailedException"),
            "case {case}: expected a 400 VerificationFailedException, got {status} {parsed}"
        );
        assert_eq!(
            parsed.get("Reason").and_then(Value::as_str),
            Some("INVALID_AUTH_REQUEST_CRYPTOGRAM"),
            "case {case}: the refusal must name the modelled reason, got {parsed}"
        );
        assert!(
            VERIFICATION_FAILED_REASON_VALUES.contains(&"INVALID_AUTH_REQUEST_CRYPTOGRAM"),
            "the reason must be one the model's VerificationFailedReason carries"
        );
    }
}

#[tokio::test]
async fn the_response_cryptogram_is_the_one_its_method_computes() {
    let router = emulator();
    // "provide an APRC (Authorization Response Cryptogram) response using Method 1 or
    // Method 2" (`VerifyAuthRequestCryptogram`): each method is its own computation over
    // the verified request cryptogram, and what it is computed from changes it.
    let pan = "4111111111111111";
    let transaction_data = hex_data(8);
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_EMV_APP_CRYPTOGRAMS,
        derive_key(),
    )
    .await;
    let (_, attributes) = session_key_schemes(pan)
        .into_iter()
        .find(|(scheme, _)| *scheme == "Mastercard")
        .expect("the Mastercard scheme is in the table");
    let generated = ok_data(
        &router,
        GENERATE_CRYPTOGRAM_URI,
        &generate_cryptogram_body(&key.arn, "EMV_OPTION_A", &attributes, &transaction_data),
    )
    .await;
    let cryptogram = string(&generated, "AuthRequestCryptogram").to_string();

    // (case, the response attributes, the hexadecimal characters the ARPC carries)
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "method 1, an approval code",
            json!({"ArpcMethod1": {"AuthResponseCode": "3030"}}),
            16,
        ),
        (
            "method 1, a decline code",
            json!({"ArpcMethod1": {"AuthResponseCode": "3035"}}),
            16,
        ),
        (
            "method 2, a card status update",
            json!({"ArpcMethod2": {"CardStatusUpdate": "00112233"}}),
            8,
        ),
        (
            "method 2, with proprietary authentication data",
            json!({"ArpcMethod2": {
                "CardStatusUpdate": "00112233",
                "ProprietaryAuthenticationData": "AABBCCDD",
            }}),
            8,
        ),
    ];
    let mut answered: Vec<(String, String)> = Vec::new();
    for (case, response, characters) in rows {
        let verified = ok_data(
            &router,
            VERIFY_CRYPTOGRAM_URI,
            &verify_cryptogram_body(
                &key.arn,
                "EMV_OPTION_A",
                &attributes,
                &transaction_data,
                &cryptogram,
                Some(&response),
            ),
        )
        .await;
        let value = string(&verified, "AuthResponseValue").to_string();
        assert!(
            value.len() == characters && value.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case}: AuthResponseValue {value} must be {characters} hexadecimal \
             characters"
        );
        // `AuthResponseValueType` carries 1..16 characters, which both methods fit.
        assert!(
            (1..=16).contains(&value.len()),
            "case {case}: AuthResponseValue {value} must fit the modelled length bound"
        );
        answered.push((case.to_string(), value));
    }
    let distinct: BTreeSet<&String> = answered.iter().map(|(_, value)| value).collect();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "each method and each response datum must compute its own ARPC, got {answered:?}"
    );
}

#[tokio::test]
async fn the_cryptogram_operations_need_an_issuer_master_key_that_derives() {
    let router = emulator();
    // "ARQC generation uses an Issuer Master Key (IMK) for application cryptograms
    // (TR31_E0_EMV_MKEY_APP_CRYPTOGRAMS) to derive a session key ... The KeyModesOfUse
    // should be set to DeriveKey" (`GenerateAuthRequestCryptogram`): a key of another
    // usage, a key whose algorithm names no Triple DES cipher and a key that may not
    // derive are each refused rather than run under the wrong key.
    let pan = "4111111111111111";
    let transaction_data = hex_data(8);
    let (_, attributes) = session_key_schemes(pan)
        .into_iter()
        .find(|(scheme, _)| *scheme == "EmvCommon")
        .expect("the EMV common scheme is in the table");
    // (case, key algorithm, key usage, modes of use, whether the call is served)
    let rows: Vec<(&str, &str, &str, Value, bool)> = vec![
        (
            "an issuer master key that derives",
            "TDES_2KEY",
            USAGE_EMV_APP_CRYPTOGRAMS,
            derive_key(),
            true,
        ),
        (
            "a key with no restriction on its use",
            "TDES_2KEY",
            USAGE_EMV_APP_CRYPTOGRAMS,
            json!({"NoRestrictions": true}),
            true,
        ),
        (
            "another key usage",
            "TDES_2KEY",
            USAGE_DATA_ENCRYPTION,
            json!({"DeriveKey": true, "Encrypt": true, "Decrypt": true}),
            false,
        ),
        (
            "an AES issuer master key",
            "AES_128",
            USAGE_EMV_APP_CRYPTOGRAMS,
            derive_key(),
            false,
        ),
        (
            "modes of use without DeriveKey",
            "TDES_2KEY",
            USAGE_EMV_APP_CRYPTOGRAMS,
            generate_verify(),
            false,
        ),
    ];
    for (case, key_algorithm, key_usage, modes, served) in rows {
        let key = create_key(&router, key_algorithm, key_usage, modes).await;
        let (status, parsed) = data(
            &router,
            GENERATE_CRYPTOGRAM_URI,
            &generate_cryptogram_body(&key.arn, "EMV_OPTION_A", &attributes, &transaction_data),
        )
        .await;
        if !served {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {status} {parsed}"
            );
            // The verifying half of the pair judges the same key the same way.
            let (status, parsed) = data(
                &router,
                VERIFY_CRYPTOGRAM_URI,
                &verify_cryptogram_body(
                    &key.arn,
                    "EMV_OPTION_A",
                    &attributes,
                    &transaction_data,
                    "00112233445566FF",
                    None,
                ),
            )
            .await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: verify must refuse the same key, got {status} {parsed}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {parsed}"
        );
        let cryptogram = string(&parsed, "AuthRequestCryptogram").to_string();
        ok_data(
            &router,
            VERIFY_CRYPTOGRAM_URI,
            &verify_cryptogram_body(
                &key.arn,
                "EMV_OPTION_A",
                &attributes,
                &transaction_data,
                &cryptogram,
                None,
            ),
        )
        .await;
    }

    // An identifier naming no key is the modelled `ResourceNotFoundException`, as it is
    // for every other operation here.
    let (status, parsed) = data(
        &router,
        GENERATE_CRYPTOGRAM_URI,
        &generate_cryptogram_body(
            "alias/no-such-issuer-master-key",
            "EMV_OPTION_A",
            &attributes,
            &transaction_data,
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (404, "ResourceNotFoundException"),
        "an identifier naming no key must be a 404, got {status} {parsed}"
    );
}

// ---------------------------------------------------------------------------
// PIN data: `GeneratePinData` and `VerifyPinData`
//
// The rule these cases pin: a PIN travels only inside the ISO 9564 PIN block the
// request's `PinBlockFormat` names, encrypted under the PIN encryption key, and the
// datum a card issuer stores for it is the published Visa PIN verification value or
// IBM 3624 PIN offset computed under the PIN verification key's own material — so
// `VerifyPinData` confirms exactly the PIN `GeneratePinData` put in that block, and
// any other PIN is a failed verification.
// ---------------------------------------------------------------------------

/// `POST /pindata/generate`.
const GENERATE_PIN_DATA_URI: &str = "/pindata/generate";

/// `POST /pindata/verify`.
const VERIFY_PIN_DATA_URI: &str = "/pindata/verify";

/// The `KeyUsage` values the PIN data cases create keys under, from the control plane's
/// modelled `KeyUsage`: the PIN encryption key a PIN block travels under, and the two
/// PIN verification keys the modelled attribute families are defined for.
const USAGE_PIN_ENCRYPTION: &str = "TR31_P0_PIN_ENCRYPTION_KEY";
const USAGE_VISA_PIN_VERIFICATION: &str = "TR31_V2_VISA_PIN_VERIFICATION_KEY";
const USAGE_IBM3624_PIN_VERIFICATION: &str = "TR31_V1_IBM3624_PIN_VERIFICATION_KEY";

/// The account number the PIN data cases work with, within `PrimaryAccountNumberType`.
const PIN_DATA_PAN: &str = "4111111111111111";

/// The decimalization table the IBM 3624 cases supply, within the modelled
/// `DecimalizationTableType` (sixteen decimal digits).
const DECIMALIZATION_TABLE: &str = "0123456789012345";

/// The cardholder validation data they supply, within `PinValidationDataType`.
const PIN_VALIDATION_DATA: &str = "4111111111111111";

/// The pad character they supply, within `HexLengthEquals1`.
const PIN_VALIDATION_PAD: &str = "F";

/// The `PinVerificationKeyIndex` the Visa cases run under, within the modelled
/// `IntegerRangeBetween0And6`.
const PIN_VERIFICATION_KEY_INDEX: i64 = 1;

/// Every modelled `PinBlockFormatForPinData` value with the `KeyAlgorithm` a PIN
/// encryption key must carry for it. Source: that shape's own documentation — "The
/// ISO_Format_4 PIN block format is the only one supporting AES encryption" — so the
/// other three blocks are the Triple DES block.
fn pin_block_formats() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ISO_FORMAT_0", "TDES_2KEY"),
        ("ISO_FORMAT_1", "TDES_2KEY"),
        ("ISO_FORMAT_3", "TDES_3KEY"),
        ("ISO_FORMAT_4", "AES_128"),
    ]
}

/// How many hexadecimal characters a block of `format` carries: one cipher block, two
/// characters per byte.
fn pin_block_characters(format: &str) -> usize {
    if format == "ISO_FORMAT_4" {
        32
    } else {
        16
    }
}

/// The PIN encryption key and the PIN verification key one PIN data call runs under.
async fn pin_keys(
    router: &MultiServiceRouter,
    encryption_algorithm: &str,
    verification_usage: &str,
) -> (Key, Key) {
    let encryption = pin_encryption_key(router, encryption_algorithm).await;
    let verification = create_key(router, "TDES_2KEY", verification_usage, generate_verify()).await;
    (encryption, verification)
}

/// The `GeneratePinData` request body.
fn generate_pin_data_body(
    generation_key: &str,
    encryption_key: &str,
    attributes: Value,
    format: &str,
    pin_data_length: Option<i64>,
) -> Value {
    let mut body = json!({
        "GenerationKeyIdentifier": generation_key,
        "EncryptionKeyIdentifier": encryption_key,
        "GenerationAttributes": attributes,
        "PinBlockFormat": format,
        "PrimaryAccountNumber": PIN_DATA_PAN,
    });
    if let (Some(length), Value::Object(target)) = (pin_data_length, &mut body) {
        target.insert("PinDataLength".to_string(), json!(length));
    }
    body
}

/// The `VerifyPinData` request body.
fn verify_pin_data_body(
    verification_key: &str,
    encryption_key: &str,
    attributes: Value,
    format: &str,
    encrypted_pin_block: &str,
) -> Value {
    json!({
        "VerificationKeyIdentifier": verification_key,
        "EncryptionKeyIdentifier": encryption_key,
        "VerificationAttributes": attributes,
        "PinBlockFormat": format,
        "PrimaryAccountNumber": PIN_DATA_PAN,
        "EncryptedPinBlock": encrypted_pin_block,
    })
}

/// The `VisaPin` generation attributes.
fn visa_pin() -> Value {
    json!({"VisaPin": {"PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX}})
}

/// One member of the IBM 3624 attribute family: the members every variant of it
/// carries, plus the ones that variant adds.
fn ibm3624_attributes(variant: &str, extra: &[(&str, Value)]) -> Value {
    let mut members = json!({
        "DecimalizationTable": DECIMALIZATION_TABLE,
        "PinValidationDataPadCharacter": PIN_VALIDATION_PAD,
        "PinValidationData": PIN_VALIDATION_DATA,
    });
    if let Value::Object(target) = &mut members {
        for (member, value) in extra {
            target.insert((*member).to_string(), value.clone());
        }
    }
    json!({variant: members})
}

/// The one member a `PinData` union carries, and its value.
fn pin_data_member(parsed: &Value) -> (String, String) {
    let data = parsed
        .get("PinData")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected a PinData union in {parsed}"));
    let (member, value) = data
        .iter()
        .next()
        .unwrap_or_else(|| panic!("expected one member in the PinData union of {parsed}"));
    assert_eq!(
        data.len(),
        1,
        "a PinData union carries exactly one member, got {parsed}"
    );
    (
        member.clone(),
        value
            .as_str()
            .unwrap_or_else(|| panic!("expected a string {member} in {parsed}"))
            .to_string(),
    )
}

/// `digits` with its leading digit stepped on by one, modulo ten: a value of the same
/// shape that is certainly not the one it was made from.
fn stepped(digits: &str) -> String {
    let mut characters: Vec<char> = digits.chars().collect();
    let first = characters
        .first()
        .and_then(|c| c.to_digit(10))
        .unwrap_or_default();
    characters[0] = char::from_digit((first + 1) % 10, 10).unwrap_or('0');
    characters.into_iter().collect()
}

/// The `EncryptedPinBlock` a `GeneratePinData` call answers.
async fn generated_pin_block(
    router: &MultiServiceRouter,
    generation_key: &str,
    encryption_key: &str,
    attributes: Value,
    format: &str,
    pin_data_length: Option<i64>,
) -> String {
    let parsed = ok_data(
        router,
        GENERATE_PIN_DATA_URI,
        &generate_pin_data_body(
            generation_key,
            encryption_key,
            attributes,
            format,
            pin_data_length,
        ),
    )
    .await;
    string(&parsed, "EncryptedPinBlock").to_string()
}

#[tokio::test]
async fn pin_data_round_trips_through_every_modelled_pin_block_format() {
    let router = emulator();
    // The table spans the whole modelled `PinBlockFormatForPinData` catalog: a value
    // the model gains later fails this assertion rather than going untested.
    let covered: BTreeSet<&str> = pin_block_formats().into_iter().map(|(f, _)| f).collect();
    assert_eq!(
        covered,
        PIN_BLOCK_FORMAT_FOR_PIN_DATA_VALUES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        "the format table must span the modelled catalog, which is \
         {PIN_BLOCK_FORMAT_FOR_PIN_DATA_VALUES:?}"
    );

    for (format, encryption_algorithm) in pin_block_formats() {
        let (pek, pvk) = pin_keys(&router, encryption_algorithm, USAGE_VISA_PIN_VERIFICATION).await;
        // Both published bounds of `PinDataLength` (range 4..12) and one length between
        // them: the block carries the PIN's own length, so every one has to round trip.
        for length in [4i64, 6, 12] {
            let case = format!("{format}/{encryption_algorithm}/PIN of {length} digits");
            let parsed = ok_data(
                &router,
                GENERATE_PIN_DATA_URI,
                &generate_pin_data_body(&pvk.arn, &pek.arn, visa_pin(), format, Some(length)),
            )
            .await;
            assert_eq!(
                string(&parsed, "GenerationKeyArn"),
                pvk.arn,
                "case {case}: expected the generation key's canonical ARN, got {parsed}"
            );
            assert_eq!(
                string(&parsed, "GenerationKeyCheckValue"),
                pvk.key_check_value,
                "case {case}: expected the generation key's own check value, got {parsed}"
            );
            assert_eq!(
                string(&parsed, "EncryptionKeyArn"),
                pek.arn,
                "case {case}: expected the encryption key's canonical ARN, got {parsed}"
            );
            assert_eq!(
                string(&parsed, "EncryptionKeyCheckValue"),
                pek.key_check_value,
                "case {case}: expected the encryption key's own check value, got {parsed}"
            );
            let block = string(&parsed, "EncryptedPinBlock").to_string();
            assert!(
                block.len() == pin_block_characters(format)
                    && block.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case}: expected {} hexadecimal characters of PIN block, got {block:?}",
                pin_block_characters(format)
            );
            let (member, value) = pin_data_member(&parsed);
            assert_eq!(
                member, "VerificationValue",
                "case {case}: a Visa PVV is answered as a VerificationValue, got {parsed}"
            );
            // A Visa PIN verification value is four decimal digits, whatever the PIN's
            // own length.
            assert!(
                value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()),
                "case {case}: expected four decimal digits of PVV, got {value:?}"
            );

            let verified = ok_data(
                &router,
                VERIFY_PIN_DATA_URI,
                &verify_pin_data_body(
                    &pvk.arn,
                    &pek.arn,
                    json!({"VisaPin": {
                        "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                        "VerificationValue": value,
                    }}),
                    format,
                    &block,
                ),
            )
            .await;
            assert_eq!(
                (
                    string(&verified, "VerificationKeyArn"),
                    string(&verified, "VerificationKeyCheckValue"),
                    string(&verified, "EncryptionKeyArn"),
                    string(&verified, "EncryptionKeyCheckValue"),
                ),
                (
                    pvk.arn.as_str(),
                    pvk.key_check_value.as_str(),
                    pek.arn.as_str(),
                    pek.key_check_value.as_str(),
                ),
                "case {case}: the verification must answer from the two keys it ran under, \
                 got {verified}"
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_generation_attribute_answers_pin_data_a_verify_confirms() {
    let router = emulator();
    // Every member of the modelled `PinGenerationAttributes` union
    // (`smithy_query(query="shape",shape="PinGenerationAttributes")`), which is the
    // class this rule generalises over: two Visa members and four IBM 3624 members.
    let variants = [
        "VisaPin",
        "VisaPinVerificationValue",
        "Ibm3624NaturalPin",
        "Ibm3624RandomPin",
        "Ibm3624PinOffset",
        "Ibm3624PinFromOffset",
    ];
    // The offset `Ibm3624PinFromOffset` recovers its PIN from, four digits like the PIN.
    let from_offset = "1357";
    for variant in variants {
        let visa = variant.starts_with("Visa");
        let usage = if visa {
            USAGE_VISA_PIN_VERIFICATION
        } else {
            USAGE_IBM3624_PIN_VERIFICATION
        };
        let (pek, pvk) = pin_keys(&router, "TDES_2KEY", usage).await;
        // The two members that carry an `EncryptedPinBlock` present the cardholder's own
        // PIN, so a block is made for them first by the member of the same family that
        // draws one.
        let presented = match variant {
            "VisaPinVerificationValue" => {
                generated_pin_block(
                    &router,
                    &pvk.arn,
                    &pek.arn,
                    visa_pin(),
                    "ISO_FORMAT_0",
                    Some(6),
                )
                .await
            }
            "Ibm3624PinOffset" => {
                generated_pin_block(
                    &router,
                    &pvk.arn,
                    &pek.arn,
                    ibm3624_attributes("Ibm3624RandomPin", &[]),
                    "ISO_FORMAT_0",
                    Some(6),
                )
                .await
            }
            _ => String::new(),
        };
        // A member that DRAWS or COMPUTES the PIN is told how long it must be; one that
        // reads it out of a block or an offset takes the length from there.
        let (attributes, length) = match variant {
            "VisaPin" => (visa_pin(), Some(6)),
            "VisaPinVerificationValue" => (
                json!({"VisaPinVerificationValue": {
                    "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                    "EncryptedPinBlock": presented,
                }}),
                None,
            ),
            "Ibm3624NaturalPin" => (ibm3624_attributes(variant, &[]), Some(6)),
            "Ibm3624RandomPin" => (ibm3624_attributes(variant, &[]), Some(6)),
            "Ibm3624PinOffset" => (
                ibm3624_attributes(variant, &[("EncryptedPinBlock", json!(presented))]),
                None,
            ),
            "Ibm3624PinFromOffset" => (
                ibm3624_attributes(variant, &[("PinOffset", json!(from_offset))]),
                None,
            ),
            other => panic!("case {other:?}: the table names no member of the modelled union"),
        };
        let parsed = ok_data(
            &router,
            GENERATE_PIN_DATA_URI,
            &generate_pin_data_body(&pvk.arn, &pek.arn, attributes, "ISO_FORMAT_0", length),
        )
        .await;
        let block = string(&parsed, "EncryptedPinBlock").to_string();
        let (member, value) = pin_data_member(&parsed);
        let expected_member = if visa {
            "VerificationValue"
        } else {
            "PinOffset"
        };
        assert_eq!(
            member, expected_member,
            "case {variant}: expected the PinData to carry a {expected_member}, got {parsed}"
        );
        assert!(
            value.bytes().all(|byte| byte.is_ascii_digit()) && !value.is_empty(),
            "case {variant}: expected decimal digits of pin data, got {value:?}"
        );
        if variant == "Ibm3624NaturalPin" {
            // The natural PIN is the PIN the validation data itself computes to, so the
            // offset between the PIN and the natural PIN is zero at every digit.
            assert!(
                value.chars().all(|digit| digit == '0'),
                "case {variant}: a natural PIN has a zero offset, got {value:?}"
            );
        }
        if variant == "Ibm3624PinFromOffset" {
            assert_eq!(
                value, from_offset,
                "case {variant}: the offset the PIN was recovered from is the one stored, \
                 got {value:?}"
            );
        }

        let verification_attributes = if visa {
            json!({"VisaPin": {
                "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                "VerificationValue": value,
            }})
        } else {
            ibm3624_attributes("Ibm3624Pin", &[("PinOffset", json!(value))])
        };
        let (status, verified) = data(
            &router,
            VERIFY_PIN_DATA_URI,
            &verify_pin_data_body(
                &pvk.arn,
                &pek.arn,
                verification_attributes,
                "ISO_FORMAT_0",
                &block,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {variant}: the pin data just generated must verify, got {status} {verified}"
        );
    }
}

#[tokio::test]
async fn a_pin_that_is_not_the_one_the_stored_value_stands_for_is_refused() {
    let router = emulator();
    // `INVALID_PIN` is the modelled `VerificationFailedReason` for this class.
    assert!(
        VERIFICATION_FAILED_REASON_VALUES.contains(&"INVALID_PIN"),
        "INVALID_PIN must be a modelled VerificationFailedReason, the catalog is \
         {VERIFICATION_FAILED_REASON_VALUES:?}"
    );
    let (pek, ibm) = pin_keys(&router, "TDES_2KEY", USAGE_IBM3624_PIN_VERIFICATION).await;
    let (_, visa) = pin_keys(&router, "TDES_2KEY", USAGE_VISA_PIN_VERIFICATION).await;
    // Another PIN encryption key of the same algorithm: a block encrypted under it
    // presents no PIN this key can read.
    let other_pek = create_key(
        &router,
        "TDES_2KEY",
        USAGE_PIN_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;

    // Offsets of several lengths, each recovering its own PIN from the same validation
    // data: the stored offset is one row's, and the block presented is another's.
    let offsets = ["1357", "000000", "987654321098"];
    for offset in offsets {
        let block = generated_pin_block(
            &router,
            &ibm.arn,
            &pek.arn,
            ibm3624_attributes("Ibm3624PinFromOffset", &[("PinOffset", json!(offset))]),
            "ISO_FORMAT_0",
            None,
        )
        .await;
        // The offset the issuer stores, and the block the caller presents.
        let rows: Vec<(String, String, String, bool)> = vec![
            (
                format!("the PIN the stored offset stands for ({} digits)", offset.len()),
                offset.to_string(),
                block.clone(),
                true,
            ),
            (
                "a stored offset one digit away, which stands for another PIN".to_string(),
                stepped(offset),
                block.clone(),
                false,
            ),
            (
                "a block encrypted under another PIN encryption key".to_string(),
                offset.to_string(),
                generated_pin_block(
                    &router,
                    &ibm.arn,
                    &other_pek.arn,
                    ibm3624_attributes("Ibm3624PinFromOffset", &[("PinOffset", json!(offset))]),
                    "ISO_FORMAT_0",
                    None,
                )
                .await,
                false,
            ),
        ];
        for (case, stored, presented, served) in rows {
            let (status, parsed) = data(
                &router,
                VERIFY_PIN_DATA_URI,
                &verify_pin_data_body(
                    &ibm.arn,
                    &pek.arn,
                    ibm3624_attributes("Ibm3624Pin", &[("PinOffset", json!(stored))]),
                    "ISO_FORMAT_0",
                    &presented,
                ),
            )
            .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {parsed}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "VerificationFailedException"),
                "case {case:?}: expected a failed verification, got {status} {parsed}"
            );
            assert_eq!(
                string(&parsed, "Reason"),
                "INVALID_PIN",
                "case {case:?}: expected the PIN as the datum that failed, got {parsed}"
            );
            // One sentence for the whole PIN class, naming the PIN BLOCK as what was
            // verified and echoing nothing of the PIN, the block or either key.
            assert_eq!(
                string(&parsed, "Message"),
                "Pin block verification failed.",
                "case {case:?}: a failed PIN verification states its own class, got {parsed}"
            );
        }
    }

    // The same rule on the Visa side: a verification value one digit away from the one
    // this PIN computes to stands for another PIN.
    let parsed = ok_data(
        &router,
        GENERATE_PIN_DATA_URI,
        &generate_pin_data_body(&visa.arn, &pek.arn, visa_pin(), "ISO_FORMAT_0", Some(4)),
    )
    .await;
    let block = string(&parsed, "EncryptedPinBlock").to_string();
    let (_, value) = pin_data_member(&parsed);
    let (status, refused) = data(
        &router,
        VERIFY_PIN_DATA_URI,
        &verify_pin_data_body(
            &visa.arn,
            &pek.arn,
            json!({"VisaPin": {
                "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                "VerificationValue": stepped(&value),
            }}),
            "ISO_FORMAT_0",
            &block,
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "VerificationFailedException"),
        "a verification value that is not this PIN's must be refused, got {status} {refused}"
    );
}

#[tokio::test]
async fn the_pin_data_operations_need_the_keys_their_algorithms_are_defined_under() {
    let router = emulator();
    // (case, the generation key's usage, its modes of use, whether the call is served)
    let generation_rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "a Visa PIN verification key that generates",
            USAGE_VISA_PIN_VERIFICATION,
            generate_verify(),
            true,
        ),
        (
            "a Visa PIN verification key with no restriction on its use",
            USAGE_VISA_PIN_VERIFICATION,
            json!({"NoRestrictions": true}),
            true,
        ),
        (
            "a Visa PIN verification key that may not generate",
            USAGE_VISA_PIN_VERIFICATION,
            json!({"Verify": true}),
            false,
        ),
        (
            "an IBM 3624 PIN verification key, which is another algorithm's key",
            USAGE_IBM3624_PIN_VERIFICATION,
            generate_verify(),
            false,
        ),
        (
            "a PIN encryption key, which verifies no PIN",
            USAGE_PIN_ENCRYPTION,
            generate_verify(),
            false,
        ),
        (
            "a card verification key, which belongs to another operation",
            USAGE_CARD_VERIFICATION,
            generate_verify(),
            false,
        ),
    ];
    for (case, usage, modes, served) in generation_rows {
        let pek = create_key(
            &router,
            "TDES_2KEY",
            USAGE_PIN_ENCRYPTION,
            encrypt_decrypt(),
        )
        .await;
        let generation = create_key(&router, "TDES_2KEY", usage, modes).await;
        let (status, parsed) = data(
            &router,
            GENERATE_PIN_DATA_URI,
            &generate_pin_data_body(
                &generation.arn,
                &pek.arn,
                visa_pin(),
                "ISO_FORMAT_0",
                Some(4),
            ),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the key to be refused, got {status} {parsed}"
            );
        }
    }

    // (case, the encryption key's algorithm, usage, modes, the format, whether served)
    let encryption_rows: Vec<(&str, &str, &str, Value, &str, bool)> = vec![
        (
            "a Triple DES PIN encryption key that encrypts, for a Triple DES block",
            "TDES_2KEY",
            USAGE_PIN_ENCRYPTION,
            json!({"Encrypt": true}),
            "ISO_FORMAT_0",
            true,
        ),
        (
            "an AES PIN encryption key, for the one format defined over AES",
            "AES_256",
            USAGE_PIN_ENCRYPTION,
            encrypt_decrypt(),
            "ISO_FORMAT_4",
            true,
        ),
        (
            "a Triple DES PIN encryption key for the AES-only format",
            "TDES_2KEY",
            USAGE_PIN_ENCRYPTION,
            encrypt_decrypt(),
            "ISO_FORMAT_4",
            false,
        ),
        (
            "an AES PIN encryption key for a Triple DES format",
            "AES_128",
            USAGE_PIN_ENCRYPTION,
            encrypt_decrypt(),
            "ISO_FORMAT_0",
            false,
        ),
        (
            "a PIN encryption key that may not encrypt",
            "TDES_2KEY",
            USAGE_PIN_ENCRYPTION,
            json!({"Decrypt": true}),
            "ISO_FORMAT_0",
            false,
        ),
        (
            "a Visa PIN verification key, which carries no PIN block",
            "TDES_2KEY",
            USAGE_VISA_PIN_VERIFICATION,
            encrypt_decrypt(),
            "ISO_FORMAT_0",
            false,
        ),
    ];
    for (case, algorithm, usage, modes, format, served) in encryption_rows {
        let encryption = create_key(&router, algorithm, usage, modes).await;
        let generation = create_key(
            &router,
            "TDES_2KEY",
            USAGE_VISA_PIN_VERIFICATION,
            generate_verify(),
        )
        .await;
        let (status, parsed) = data(
            &router,
            GENERATE_PIN_DATA_URI,
            &generate_pin_data_body(&generation.arn, &encryption.arn, visa_pin(), format, Some(4)),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the key to be refused, got {status} {parsed}"
            );
        }
    }

    // An identifier naming no key is the modelled `ResourceNotFoundException`, for each
    // of the two keys these operations name and for both operations.
    let pek = create_key(
        &router,
        "TDES_2KEY",
        USAGE_PIN_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;
    let pvk = create_key(
        &router,
        "TDES_2KEY",
        USAGE_VISA_PIN_VERIFICATION,
        generate_verify(),
    )
    .await;
    let absent = "alias/no-such-pin-key";
    // (case, the generation key, the encryption key)
    let missing_rows: Vec<(&str, &str, &str)> = vec![
        ("the generation key", absent, pek.arn.as_str()),
        ("the encryption key", pvk.arn.as_str(), absent),
    ];
    for (case, generation, encryption) in missing_rows {
        let (status, parsed) = data(
            &router,
            GENERATE_PIN_DATA_URI,
            &generate_pin_data_body(generation, encryption, visa_pin(), "ISO_FORMAT_0", Some(4)),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an identifier naming no key must be a 404, got {status} {parsed}"
        );
        let (status, parsed) = data(
            &router,
            VERIFY_PIN_DATA_URI,
            &verify_pin_data_body(
                generation,
                encryption,
                json!({"VisaPin": {
                    "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                    "VerificationValue": "1234",
                }}),
                "ISO_FORMAT_0",
                "0123456789ABCDEF",
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: VerifyPinData must answer a 404 alike, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn the_pin_length_a_request_names_must_be_the_pin_it_works_with() {
    let router = emulator();
    let (pek, visa) = pin_keys(&router, "TDES_2KEY", USAGE_VISA_PIN_VERIFICATION).await;
    let ibm = create_key(
        &router,
        "TDES_2KEY",
        USAGE_IBM3624_PIN_VERIFICATION,
        generate_verify(),
    )
    .await;
    // An offset is aligned digit for digit with the PIN it describes, so this one
    // recovers a four-digit PIN whatever `PinDataLength` says.
    let from_offset = ibm3624_attributes("Ibm3624PinFromOffset", &[("PinOffset", json!("1357"))]);
    // (case, the generation key, the attributes, the length named, whether served)
    let rows: Vec<(&str, &str, Value, Option<i64>, bool)> = vec![
        (
            "a drawn PIN of the length the request names",
            visa.arn.as_str(),
            visa_pin(),
            Some(8),
            true,
        ),
        (
            "a PIN recovered from an offset, under that offset's own length",
            ibm.arn.as_str(),
            from_offset.clone(),
            Some(4),
            true,
        ),
        (
            "a PIN recovered from an offset, with no length named",
            ibm.arn.as_str(),
            from_offset.clone(),
            None,
            true,
        ),
        (
            "a PIN recovered from an offset, under a length that is not its own",
            ibm.arn.as_str(),
            from_offset.clone(),
            Some(8),
            false,
        ),
    ];
    for (case, generation, attributes, length, served) in rows {
        let (status, parsed) = data(
            &router,
            GENERATE_PIN_DATA_URI,
            &generate_pin_data_body(generation, &pek.arn, attributes, "ISO_FORMAT_0", length),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the length to be refused, got {status} {parsed}"
            );
        }
    }

    // The same rule on the verifying side: the block carries the PIN's own length, so a
    // request naming another one names a PIN that block does not carry.
    let block = generated_pin_block(
        &router,
        &ibm.arn,
        &pek.arn,
        from_offset,
        "ISO_FORMAT_0",
        None,
    )
    .await;
    // (case, the length named, whether served)
    let rows: Vec<(&str, Option<i64>, bool)> = vec![
        ("the length the block carries", Some(4), true),
        ("no length at all", None, true),
        ("a length the block does not carry", Some(8), false),
    ];
    for (case, length, served) in rows {
        let mut body = verify_pin_data_body(
            &ibm.arn,
            &pek.arn,
            ibm3624_attributes("Ibm3624Pin", &[("PinOffset", json!("1357"))]),
            "ISO_FORMAT_0",
            &block,
        );
        if let (Some(length), Value::Object(target)) = (length, &mut body) {
            target.insert("PinDataLength".to_string(), json!(length));
        }
        let (status, parsed) = data(&router, VERIFY_PIN_DATA_URI, &body).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "VerificationFailedException"),
                "case {case:?}: expected a failed verification, got {status} {parsed}"
            );
        }
    }
}

/// The `KeyUsage` a DUKPT base derivation key carries, from the control plane's modelled
/// `KeyUsage`: the BDK "To encrypt using DUKPT, you must already have a BDK (Base
/// Derivation Key) key in your account with KeyModesOfUse set to DeriveKey"
/// (`EncryptData`'s own documentation) names.
const USAGE_BASE_DERIVATION: &str = "TR31_B0_BASE_DERIVATION_KEY";

/// The `KeyUsage` an EMV issuer master key for data confidentiality carries, from the
/// same shape: the issuer master key an EMV data encryption derives its session key from.
const USAGE_EMV_CONFIDENTIALITY: &str = "TR31_E1_EMV_MKEY_CONFIDENTIALITY";

/// The `KeyUsage` an asymmetric data encryption key pair carries, from the same shape.
const USAGE_ASYMMETRIC_DATA_ENCRYPTION: &str = "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION";

/// The `KeyClass` an asymmetric key is: a whole key pair.
const CLASS_ASYMMETRIC_PAIR: &str = "ASYMMETRIC_KEY_PAIR";

/// `POST /keys/{IncomingKeyIdentifier}/reencrypt`, the URI the model binds
/// `ReEncryptData` to.
fn reencrypt_uri(identifier: &str) -> String {
    format!("/keys/{}/reencrypt", encoded(identifier))
}

/// The `message` a refusal states.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// `value` as an object with the members that are present, so an attribute set carries
/// only the members a case names and defaults the rest.
fn attributes(members: &[(&str, Option<Value>)]) -> Value {
    let mut object = serde_json::Map::new();
    for (member, value) in members {
        if let Some(value) = value {
            object.insert((*member).to_string(), value.clone());
        }
    }
    Value::Object(object)
}

/// One row of the DUKPT table: the base derivation key it derives from, the serial
/// number of the transaction, the attributes the request names, and the block length in
/// BYTES of the key that derivation produces — which is what the data length has to be a
/// multiple of.
struct DukptRow {
    case: &'static str,
    key_algorithm: &'static str,
    key_serial_number: &'static str,
    derivation_type: Option<&'static str>,
    key_variant: Option<&'static str>,
    mode: Option<&'static str>,
    initialization_vector: Option<&'static str>,
    derived_block_len: usize,
}

/// The DUKPT cases: both schemes the service supports — the Triple DES scheme of a
/// `TDES_2KEY` base derivation key with its 20-character serial number, and the AES
/// scheme of an AES one with its 24-character serial number — across every modelled
/// `DukptDerivationType`, `DukptKeyVariant` and `DukptEncryptionMode`, with and without
/// an initialization vector, and with the attributes the model makes optional left out
/// so the defaults are exercised too.
fn dukpt_table() -> Vec<DukptRow> {
    vec![
        DukptRow {
            case: "a Triple DES base derivation key, every optional attribute defaulted",
            key_algorithm: "TDES_2KEY",
            key_serial_number: "FFFF9876543210E00001",
            derivation_type: None,
            key_variant: None,
            mode: None,
            initialization_vector: None,
            derived_block_len: 8,
        },
        DukptRow {
            case: "a Triple DES base derivation key, ECB and the request variant",
            key_algorithm: "TDES_2KEY",
            key_serial_number: "FFFF9876543210E00002",
            derivation_type: Some("TDES_2KEY"),
            key_variant: Some("REQUEST"),
            mode: Some("ECB"),
            initialization_vector: None,
            derived_block_len: 8,
        },
        DukptRow {
            case: "a Triple DES base derivation key, CBC from an initialization vector",
            key_algorithm: "TDES_2KEY",
            key_serial_number: "FFFF9876543210E0000F",
            derivation_type: None,
            key_variant: Some("RESPONSE"),
            mode: Some("CBC"),
            initialization_vector: Some("0123456789ABCDEF"),
            derived_block_len: 8,
        },
        DukptRow {
            case: "an AES-128 base derivation key, every optional attribute defaulted",
            key_algorithm: "AES_128",
            key_serial_number: "123456789012345600000001",
            derivation_type: None,
            key_variant: None,
            mode: None,
            initialization_vector: None,
            derived_block_len: 16,
        },
        DukptRow {
            case: "an AES-192 base derivation key deriving an AES-128 working key",
            key_algorithm: "AES_192",
            key_serial_number: "123456789012345600000002",
            derivation_type: Some("AES_128"),
            key_variant: Some("REQUEST"),
            mode: Some("CBC"),
            initialization_vector: Some("000102030405060708090A0B0C0D0E0F"),
            derived_block_len: 16,
        },
        DukptRow {
            case: "an AES-256 base derivation key deriving an AES-192 working key",
            key_algorithm: "AES_256",
            key_serial_number: "1234567890123456000000FF",
            derivation_type: Some("AES_192"),
            key_variant: Some("BIDIRECTIONAL"),
            mode: Some("ECB"),
            initialization_vector: None,
            derived_block_len: 16,
        },
        DukptRow {
            case: "an AES-256 base derivation key deriving an AES-256 working key",
            key_algorithm: "AES_256",
            key_serial_number: "123456789012345600001000",
            derivation_type: Some("AES_256"),
            key_variant: Some("RESPONSE"),
            mode: Some("CBC"),
            initialization_vector: None,
            derived_block_len: 16,
        },
        DukptRow {
            case: "an AES-256 base derivation key deriving a Triple DES working key",
            key_algorithm: "AES_256",
            key_serial_number: "123456789012345600001001",
            derivation_type: Some("TDES_2KEY"),
            key_variant: Some("REQUEST"),
            mode: Some("ECB"),
            initialization_vector: None,
            derived_block_len: 8,
        },
        DukptRow {
            case: "an AES-256 base derivation key deriving a triple-length Triple DES key",
            key_algorithm: "AES_256",
            key_serial_number: "123456789012345600001002",
            derivation_type: Some("TDES_3KEY"),
            key_variant: None,
            mode: Some("CBC"),
            initialization_vector: None,
            derived_block_len: 8,
        },
    ]
}

/// The `EncryptData` or `DecryptData` body of one DUKPT case, with `key_serial_number`
/// naming the transaction (which a case varies to show the key is per transaction).
fn dukpt_body(
    data_member: &str,
    value: &str,
    attributes_member: &str,
    row: &DukptRow,
    key_serial_number: &str,
) -> Value {
    let set = attributes(&[
        ("KeySerialNumber", Some(json!(key_serial_number))),
        ("Mode", row.mode.map(|mode| json!(mode))),
        (
            "DukptKeyDerivationType",
            row.derivation_type.map(|named| json!(named)),
        ),
        (
            "DukptKeyVariant",
            row.key_variant.map(|variant| json!(variant)),
        ),
        (
            "InitializationVector",
            row.initialization_vector.map(|iv| json!(iv)),
        ),
    ]);
    json!({data_member: value, attributes_member: {"Dukpt": set}})
}

#[tokio::test]
async fn dukpt_encryption_round_trips_under_every_derived_key_type() {
    let router = emulator();
    let rows = dukpt_table();

    // The table spans the modelled catalogs the rule generalises over: a value any of
    // them gains later fails these assertions rather than going untested.
    let variants: BTreeSet<&str> = rows
        .iter()
        .map(|row| row.key_variant.unwrap_or("BIDIRECTIONAL"))
        .collect();
    assert_eq!(
        variants,
        DUKPT_KEY_VARIANT_VALUES.iter().copied().collect(),
        "the table must span the modelled DukptKeyVariant catalog"
    );
    let modes: BTreeSet<&str> = rows.iter().map(|row| row.mode.unwrap_or("CBC")).collect();
    assert_eq!(
        modes,
        DUKPT_ENCRYPTION_MODE_VALUES.iter().copied().collect(),
        "the table must span the modelled DukptEncryptionMode catalog"
    );
    let derived: BTreeSet<&str> = rows
        .iter()
        .map(|row| row.derivation_type.unwrap_or(row.key_algorithm))
        .collect();
    assert_eq!(
        derived,
        DUKPT_DERIVATION_TYPE_VALUES.iter().copied().collect(),
        "the table must span the modelled DukptDerivationType catalog"
    );

    for row in &rows {
        let bdk = create_key(
            &router,
            row.key_algorithm,
            USAGE_BASE_DERIVATION,
            derive_key(),
        )
        .await;
        // Two whole blocks of the key the derivation produces: "When you use TDES or
        // TDES DUKPT, the plaintext data length must be a multiple of 8 bytes. For AES
        // or AES DUKPT, the plaintext data length must be a multiple of 16 bytes"
        // (`EncryptData`'s own documentation).
        let plain_text = hex_data(row.derived_block_len * 2);
        let case = row.case;
        let (status, parsed) = data(
            &router,
            &encrypt_uri(&bdk.arn),
            &dukpt_body(
                "PlainText",
                &plain_text,
                "EncryptionAttributes",
                row,
                row.key_serial_number,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {parsed}"
        );
        let cipher_text = string(&parsed, "CipherText").to_string();
        assert_eq!(
            string(&parsed, "KeyArn"),
            bdk.arn,
            "case {case:?}: the answer must carry the base derivation key's canonical ARN, \
             got {parsed}"
        );
        assert_eq!(
            string(&parsed, "KeyCheckValue"),
            bdk.key_check_value,
            "case {case:?}: the answer must carry that key's own check value, got {parsed}"
        );
        assert!(
            cipher_text.len() == plain_text.len()
                && cipher_text.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: CipherText {cipher_text} must be {} hex characters",
            plain_text.len()
        );
        assert_ne!(
            cipher_text, plain_text,
            "case {case:?}: the ciphertext must not be the plaintext echoed back"
        );

        // The same serial number and the same attributes derive the same key, so the
        // decrypt inverts the encrypt.
        let decrypted = ok_data(
            &router,
            &decrypt_uri(&bdk.arn),
            &dukpt_body(
                "CipherText",
                &cipher_text,
                "DecryptionAttributes",
                row,
                row.key_serial_number,
            ),
        )
        .await;
        assert_eq!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {case:?}: the DUKPT decrypt must recover the plaintext"
        );

        // A DIFFERENT transaction counter is a different key, so the same ciphertext read
        // under it is not the plaintext: that is what "unique key per transaction" means.
        let other_transaction = another_transaction(row.key_serial_number);
        let elsewhere = ok_data(
            &router,
            &decrypt_uri(&bdk.arn),
            &dukpt_body(
                "CipherText",
                &cipher_text,
                "DecryptionAttributes",
                row,
                &other_transaction,
            ),
        )
        .await;
        assert_ne!(
            string(&elsewhere, "PlainText"),
            plain_text,
            "case {case:?}: the key of transaction {other_transaction} must not read the \
             ciphertext of transaction {}",
            row.key_serial_number
        );
    }
}

/// `key_serial_number` with its transaction counter stepped on: the same device, another
/// transaction.
fn another_transaction(key_serial_number: &str) -> String {
    let mut stepped = key_serial_number.to_string();
    stepped.pop();
    let last = key_serial_number.chars().next_back().unwrap_or('0');
    stepped.push(if last == '7' { '9' } else { '7' });
    stepped
}

/// One row of the EMV table: the issuer master key, the card, and the attributes the
/// request names.
struct EmvRow {
    case: &'static str,
    key_algorithm: &'static str,
    major_key_derivation_mode: &'static str,
    primary_account_number: &'static str,
    pan_sequence_number: &'static str,
    session_derivation_data: &'static str,
    mode: Option<&'static str>,
    initialization_vector: Option<&'static str>,
}

/// The EMV cases: both modelled `MajorKeyDerivationMode` options, both modelled
/// `EmvEncryptionMode` values and the default, both published bounds of
/// `PrimaryAccountNumber` (12 and 19 digits), under both Triple DES issuer master key
/// algorithms.
fn emv_table() -> Vec<EmvRow> {
    vec![
        EmvRow {
            case: "EMV option A, every optional attribute defaulted",
            key_algorithm: "TDES_2KEY",
            major_key_derivation_mode: "EMV_OPTION_A",
            primary_account_number: "123456789012",
            pan_sequence_number: "00",
            session_derivation_data: "0000000000000001",
            mode: None,
            initialization_vector: None,
        },
        EmvRow {
            case: "EMV option B, ECB, the longest account number",
            key_algorithm: "TDES_2KEY",
            major_key_derivation_mode: "EMV_OPTION_B",
            primary_account_number: "1234567890123456789",
            pan_sequence_number: "01",
            session_derivation_data: "00000000000000FF",
            mode: Some("ECB"),
            initialization_vector: None,
        },
        EmvRow {
            case: "EMV option A under a triple-length issuer master key, CBC from an \
                   initialization vector",
            key_algorithm: "TDES_3KEY",
            major_key_derivation_mode: "EMV_OPTION_A",
            primary_account_number: "4111111111111111",
            pan_sequence_number: "02",
            session_derivation_data: "ABCDEF0123456789",
            mode: Some("CBC"),
            initialization_vector: Some("FEDCBA9876543210"),
        },
    ]
}

/// The `EncryptData` or `DecryptData` body of one EMV case, with the session derivation
/// data a case can vary to show the key is per session.
fn emv_body(
    data_member: &str,
    value: &str,
    attributes_member: &str,
    row: &EmvRow,
    session_derivation_data: &str,
) -> Value {
    let set = attributes(&[
        (
            "MajorKeyDerivationMode",
            Some(json!(row.major_key_derivation_mode)),
        ),
        (
            "PrimaryAccountNumber",
            Some(json!(row.primary_account_number)),
        ),
        ("PanSequenceNumber", Some(json!(row.pan_sequence_number))),
        (
            "SessionDerivationData",
            Some(json!(session_derivation_data)),
        ),
        ("Mode", row.mode.map(|mode| json!(mode))),
        (
            "InitializationVector",
            row.initialization_vector.map(|iv| json!(iv)),
        ),
    ]);
    json!({data_member: value, attributes_member: {"Emv": set}})
}

#[tokio::test]
async fn emv_encryption_round_trips_under_the_session_key_the_card_derives() {
    let router = emulator();
    let rows = emv_table();
    let covered: BTreeSet<&str> = rows
        .iter()
        .map(|row| row.major_key_derivation_mode)
        .collect();
    assert_eq!(
        covered,
        MAJOR_KEY_DERIVATION_MODE_VALUES.iter().copied().collect(),
        "the table must span the modelled MajorKeyDerivationMode catalog"
    );
    let modes: BTreeSet<&str> = rows.iter().map(|row| row.mode.unwrap_or("CBC")).collect();
    assert_eq!(
        modes,
        EMV_ENCRYPTION_MODE_VALUES.iter().copied().collect(),
        "the table must span the modelled EmvEncryptionMode catalog"
    );

    for row in &rows {
        let imk = create_key(
            &router,
            row.key_algorithm,
            USAGE_EMV_CONFIDENTIALITY,
            derive_key(),
        )
        .await;
        // The EMV derivations produce a double-length Triple DES key, which chains an
        // 8-byte block: "For EMV encryption, Amazon Web Services Payment Cryptography
        // supports TDES algorithms" (`EncryptData`'s own documentation).
        let plain_text = hex_data(16);
        let case = row.case;
        let (status, parsed) = data(
            &router,
            &encrypt_uri(&imk.arn),
            &emv_body(
                "PlainText",
                &plain_text,
                "EncryptionAttributes",
                row,
                row.session_derivation_data,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {parsed}"
        );
        let cipher_text = string(&parsed, "CipherText").to_string();
        assert_eq!(
            (
                string(&parsed, "KeyArn"),
                string(&parsed, "KeyCheckValue")
            ),
            (imk.arn.as_str(), imk.key_check_value.as_str()),
            "case {case:?}: the answer must carry the issuer master key's own ARN and \
             check value, got {parsed}"
        );
        assert_ne!(
            cipher_text, plain_text,
            "case {case:?}: the ciphertext must not be the plaintext echoed back"
        );

        let decrypted = ok_data(
            &router,
            &decrypt_uri(&imk.arn),
            &emv_body(
                "CipherText",
                &cipher_text,
                "DecryptionAttributes",
                row,
                row.session_derivation_data,
            ),
        )
        .await;
        assert_eq!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {case:?}: the EMV decrypt must recover the plaintext"
        );

        // Another session of the same card is another key.
        let other_session = "1111111111111111";
        let elsewhere = ok_data(
            &router,
            &decrypt_uri(&imk.arn),
            &emv_body(
                "CipherText",
                &cipher_text,
                "DecryptionAttributes",
                row,
                other_session,
            ),
        )
        .await;
        assert_ne!(
            string(&elsewhere, "PlainText"),
            plain_text,
            "case {case:?}: the session key of {other_session} must not read the \
             ciphertext of session {}",
            row.session_derivation_data
        );
    }
}

#[tokio::test]
async fn asymmetric_encryption_round_trips_under_every_modelled_padding() {
    let router = emulator();
    let pair = create_key_of_class(
        &router,
        CLASS_ASYMMETRIC_PAIR,
        "RSA_2048",
        USAGE_ASYMMETRIC_DATA_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;
    // One modulus of ciphertext, whatever the padding: an RSA ciphertext is exactly as
    // wide as the key.
    const MODULUS_BYTES: usize = 256;
    let plain_text = hex_data(16);
    for padding_type in PADDING_TYPE_VALUES {
        let body = json!({
            "PlainText": plain_text,
            "EncryptionAttributes": {"Asymmetric": {"PaddingType": padding_type}},
        });
        let (status, parsed) = data(&router, &encrypt_uri(&pair.arn), &body).await;
        assert_eq!(
            status, 200,
            "case {padding_type}: expected 200, got {status} {parsed}"
        );
        let cipher_text = string(&parsed, "CipherText").to_string();
        assert_eq!(
            cipher_text.len(),
            MODULUS_BYTES * 2,
            "case {padding_type}: an RSA_2048 ciphertext is one modulus wide, got \
             {cipher_text}"
        );
        assert_eq!(
            string(&parsed, "KeyArn"),
            pair.arn,
            "case {padding_type}: the answer must carry the key pair's canonical ARN"
        );
        let decrypted = ok_data(
            &router,
            &decrypt_uri(&pair.arn),
            &json!({
                "CipherText": cipher_text,
                "DecryptionAttributes": {"Asymmetric": {"PaddingType": padding_type}},
            }),
        )
        .await;
        assert_eq!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {padding_type}: the private half must recover the plaintext"
        );

        // The padding is part of what produced the ciphertext, so reading it under an
        // OAEP scheme that did not produce it is refused rather than answered.
        let other = if *padding_type == "OAEP_SHA256" {
            "OAEP_SHA512"
        } else {
            "OAEP_SHA256"
        };
        let (status, parsed) = data(
            &router,
            &decrypt_uri(&pair.arn),
            &json!({
                "CipherText": cipher_text,
                "DecryptionAttributes": {"Asymmetric": {"PaddingType": other}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {padding_type}: a ciphertext read under {other} must be refused, got \
             {status} {parsed}"
        );
    }

    // Each scheme's own ceiling over this modulus (RFC 8017): a plaintext past it is
    // refused, naming the limit, rather than truncated or padded.
    // (the padding, the longest plaintext it admits in BYTES)
    let ceilings: &[(&str, usize)] = &[
        ("PKCS1", MODULUS_BYTES - 11),
        ("OAEP_SHA1", MODULUS_BYTES - 2 * 20 - 2),
        ("OAEP_SHA256", MODULUS_BYTES - 2 * 32 - 2),
        ("OAEP_SHA512", MODULUS_BYTES - 2 * 64 - 2),
    ];
    for (padding_type, longest) in ceilings {
        for (case, bytes, served) in [("at the ceiling", *longest, true), ("past it", longest + 1, false)] {
            let (status, parsed) = data(
                &router,
                &encrypt_uri(&pair.arn),
                &json!({
                    "PlainText": hex_data(bytes),
                    "EncryptionAttributes": {"Asymmetric": {"PaddingType": padding_type}},
                }),
            )
            .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {padding_type} {case}: {bytes} bytes must be served, got \
                     {status} {parsed}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "ValidationException"),
                    "case {padding_type} {case}: {bytes} bytes must be refused, got \
                     {status} {parsed}"
                );
                assert!(
                    error_message(&parsed).contains(&(longest * 2).to_string()),
                    "case {padding_type} {case}: the refusal must name the {} hexadecimal \
                     characters it admits, got {parsed}",
                    longest * 2
                );
            }
        }
    }

    // "You must specify a PaddingType" (`ReEncryptionAttributes$Asymmetric`): there is
    // no scheme to default to, so a set naming none is refused.
    let (status, parsed) = data(
        &router,
        &encrypt_uri(&pair.arn),
        &json!({"PlainText": plain_text, "EncryptionAttributes": {"Asymmetric": {}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "an asymmetric request naming no PaddingType must be refused, got {status} {parsed}"
    );
}

/// One row of the `ReEncryptData` table: the two keys, the attribute sets of the two
/// legs, and how long a plaintext both legs' ciphers admit.
struct ReEncryptRow {
    case: &'static str,
    incoming: String,
    incoming_attributes: Value,
    outgoing: Key,
    outgoing_attributes: Value,
    plain_text_bytes: usize,
}

#[tokio::test]
async fn re_encrypt_data_moves_a_ciphertext_from_one_key_to_another() {
    let router = emulator();
    let bdk = create_key(&router, "TDES_2KEY", USAGE_BASE_DERIVATION, derive_key()).await;
    let aes_bdk = create_key(&router, "AES_256", USAGE_BASE_DERIVATION, derive_key()).await;
    let network = create_key(
        &router,
        "TDES_2KEY",
        USAGE_DATA_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;
    let acquirer = create_key(&router, "AES_256", USAGE_DATA_ENCRYPTION, encrypt_decrypt()).await;
    let pair = create_key_of_class(
        &router,
        CLASS_ASYMMETRIC_PAIR,
        "RSA_2048",
        USAGE_ASYMMETRIC_DATA_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;

    let dukpt_tdes = json!({"Dukpt": {"KeySerialNumber": "FFFF9876543210E00001", "Mode": "ECB"}});
    let dukpt_aes = json!({
        "Dukpt": {"KeySerialNumber": "123456789012345600000003", "DukptKeyVariant": "REQUEST"},
    });
    let symmetric_tdes = json!({"Symmetric": {"Mode": "CBC"}});
    let symmetric_aes = json!({"Symmetric": {"Mode": "ECB"}});
    let asymmetric = json!({"Asymmetric": {"PaddingType": "OAEP_SHA256"}});

    // Every member of `ReEncryptionAttributes` on either leg, and both the symmetric and
    // the derived key schemes on both sides of the move.
    let rows = vec![
        ReEncryptRow {
            case: "a terminal's DUKPT ciphertext onto the network's own key",
            incoming: bdk.arn.clone(),
            incoming_attributes: dukpt_tdes.clone(),
            outgoing: network.clone(),
            outgoing_attributes: symmetric_tdes.clone(),
            plain_text_bytes: 16,
        },
        ReEncryptRow {
            case: "the network's ciphertext onto a terminal's DUKPT key",
            incoming: network.arn.clone(),
            incoming_attributes: symmetric_tdes.clone(),
            outgoing: bdk.clone(),
            outgoing_attributes: dukpt_tdes.clone(),
            plain_text_bytes: 16,
        },
        ReEncryptRow {
            case: "one symmetric key onto another of a different cipher",
            incoming: network.arn.clone(),
            incoming_attributes: symmetric_tdes.clone(),
            outgoing: acquirer.clone(),
            outgoing_attributes: symmetric_aes.clone(),
            plain_text_bytes: 16,
        },
        ReEncryptRow {
            case: "an asymmetric ciphertext onto a symmetric key",
            incoming: pair.arn.clone(),
            incoming_attributes: asymmetric.clone(),
            outgoing: network.clone(),
            outgoing_attributes: symmetric_tdes.clone(),
            plain_text_bytes: 16,
        },
        ReEncryptRow {
            case: "a symmetric ciphertext onto an asymmetric key pair",
            incoming: network.arn.clone(),
            incoming_attributes: symmetric_tdes.clone(),
            outgoing: pair.clone(),
            outgoing_attributes: asymmetric.clone(),
            plain_text_bytes: 16,
        },
        ReEncryptRow {
            case: "one DUKPT scheme onto the other",
            incoming: bdk.arn.clone(),
            incoming_attributes: dukpt_tdes.clone(),
            outgoing: aes_bdk.clone(),
            outgoing_attributes: dukpt_aes.clone(),
            plain_text_bytes: 16,
        },
    ];

    for row in &rows {
        let case = row.case;
        let plain_text = hex_data(row.plain_text_bytes);
        // The incoming ciphertext is one the incoming key itself produced, which is the
        // position a caller of this operation is in.
        let encrypted = ok_data(
            &router,
            &encrypt_uri(&row.incoming),
            &json!({
                "PlainText": plain_text,
                "EncryptionAttributes": row.incoming_attributes,
            }),
        )
        .await;
        let incoming_cipher_text = string(&encrypted, "CipherText").to_string();

        let (status, parsed) = data(
            &router,
            &reencrypt_uri(&row.incoming),
            &json!({
                "OutgoingKeyIdentifier": row.outgoing.arn,
                "CipherText": incoming_cipher_text,
                "IncomingEncryptionAttributes": row.incoming_attributes,
                "OutgoingEncryptionAttributes": row.outgoing_attributes,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {parsed}"
        );
        let cipher_text = string(&parsed, "CipherText").to_string();
        assert_eq!(
            (
                string(&parsed, "KeyArn"),
                string(&parsed, "KeyCheckValue")
            ),
            (
                row.outgoing.arn.as_str(),
                row.outgoing.key_check_value.as_str()
            ),
            "case {case:?}: the answer must carry the OUTGOING key's ARN and check value, \
             got {parsed}"
        );
        assert_ne!(
            cipher_text, incoming_cipher_text,
            "case {case:?}: the ciphertext answered is the outgoing key's, not the one \
             that came in"
        );
        assert_ne!(
            cipher_text, plain_text,
            "case {case:?}: the plaintext must not be answered"
        );

        // What the move preserves is the plaintext: the outgoing key and the outgoing
        // attributes read exactly what the incoming ones carried.
        let decrypted = ok_data(
            &router,
            &decrypt_uri(&row.outgoing.arn),
            &json!({
                "CipherText": cipher_text,
                "DecryptionAttributes": row.outgoing_attributes,
            }),
        )
        .await;
        assert_eq!(
            string(&decrypted, "PlainText"),
            plain_text,
            "case {case:?}: a decrypt under the outgoing key must recover the plaintext"
        );
    }
}

#[tokio::test]
async fn a_derived_key_request_the_scheme_does_not_admit_is_refused_as_invalid() {
    let router = emulator();
    let bdk = create_key(&router, "TDES_2KEY", USAGE_BASE_DERIVATION, derive_key()).await;
    let triple = create_key(&router, "TDES_3KEY", USAGE_BASE_DERIVATION, derive_key()).await;
    let no_derive = create_key(
        &router,
        "TDES_2KEY",
        USAGE_BASE_DERIVATION,
        encrypt_decrypt(),
    )
    .await;
    let data_key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_DATA_ENCRYPTION,
        json!({"Decrypt": true}),
    )
    .await;
    let pair = create_key_of_class(
        &router,
        CLASS_ASYMMETRIC_PAIR,
        "RSA_2048",
        USAGE_ASYMMETRIC_DATA_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await;

    let dukpt = |ksn: &str| json!({"Dukpt": {"KeySerialNumber": ksn, "Mode": "ECB"}});
    let emv = json!({"Emv": {
        "MajorKeyDerivationMode": "EMV_OPTION_A",
        "PrimaryAccountNumber": "4111111111111111",
        "PanSequenceNumber": "00",
        "SessionDerivationData": "0000000000000001",
    }});
    // (case, the key, the attributes, the sentence the refusal has to state)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a base derivation key whose modes of use do not derive",
            no_derive.arn.as_str(),
            dukpt("FFFF9876543210E00001"),
            format!(
                "The KeyModesOfUse of key {} do not include DeriveKey.",
                no_derive.arn
            ),
        ),
        (
            "an EMV derivation under a key whose modes of use do not derive",
            no_derive.arn.as_str(),
            emv.clone(),
            format!(
                "The KeyModesOfUse of key {} do not include DeriveKey.",
                no_derive.arn
            ),
        ),
        (
            "a symmetric encrypt under a key whose modes of use do not encrypt",
            data_key.arn.as_str(),
            json!({"Symmetric": {"Mode": "CBC"}}),
            format!(
                "The KeyModesOfUse of key {} do not include Encrypt.",
                data_key.arn
            ),
        ),
        (
            "a key serial number of the other scheme's length",
            bdk.arn.as_str(),
            dukpt("123456789012345600000001"),
            "KeySerialNumber must be 20 hexadecimal characters for KeyAlgorithm TDES_2KEY."
                .to_string(),
        ),
        (
            "a triple-length Triple DES base derivation key, which X9.24-1 derives \
             nothing under",
            triple.arn.as_str(),
            dukpt("FFFF9876543210E00001"),
            "KeyAlgorithm TDES_3KEY is not valid for DUKPT key derivation.".to_string(),
        ),
        (
            "a derived key type the base derivation key's own scheme does not derive",
            bdk.arn.as_str(),
            json!({"Dukpt": {
                "KeySerialNumber": "FFFF9876543210E00001",
                "DukptKeyDerivationType": "AES_128",
            }}),
            "DukptKeyDerivationType AES_128 is not valid for KeyAlgorithm TDES_2KEY."
                .to_string(),
        ),
        (
            "an asymmetric set naming a symmetric key",
            bdk.arn.as_str(),
            json!({"Asymmetric": {"PaddingType": "PKCS1"}}),
            "KeyAlgorithm TDES_2KEY is not valid for asymmetric encryption.".to_string(),
        ),
    ];
    for (case, identifier, attributes, expected) in rows {
        let (status, parsed) = data(
            &router,
            &encrypt_uri(identifier),
            &json!({"PlainText": hex_data(16), "EncryptionAttributes": attributes}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {parsed}"
        );
        assert_eq!(
            error_message(&parsed),
            expected,
            "case {case:?}: expected the refusal to state the rule it broke, got {parsed}"
        );
    }

    // The data length rule is the DERIVED key's: a plaintext that is not a whole number
    // of its blocks is refused naming the length, in the hex characters the caller sent.
    let (status, parsed) = data(
        &router,
        &encrypt_uri(&bdk.arn),
        &json!({
            "PlainText": hex_data(12),
            "EncryptionAttributes": dukpt("FFFF9876543210E00001"),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a plaintext that is not a whole number of the derived key's blocks must be \
         refused, got {status} {parsed}"
    );
    assert_eq!(
        error_message(&parsed),
        "Invalid data provided. Please provide data of length multiple of 16.",
        "the refusal must state the derived cipher's block length in hex characters, got \
         {parsed}"
    );

    // The one form of these operations that is still a scaffold: a key the request
    // carries with it, rather than one of the attribute sets.
    let wrapped = json!({
        "WrappedKeyMaterial": {"Tr31KeyBlock": "B0096B1TX00S0100KS1800604B120F9292800000A1B2C3D4"},
    });
    // (case, the URI, the body)
    let scaffolded: Vec<(&str, String, Value)> = vec![
        (
            "EncryptData with a WrappedKey",
            encrypt_uri(&pair.arn),
            json!({
                "PlainText": hex_data(16),
                "EncryptionAttributes": {"Symmetric": {"Mode": "CBC"}},
                "WrappedKey": wrapped,
            }),
        ),
        (
            "ReEncryptData with a WrappedKey",
            reencrypt_uri(&bdk.arn),
            json!({
                "OutgoingKeyIdentifier": data_key.arn,
                "CipherText": hex_data(16),
                "IncomingEncryptionAttributes": dukpt("FFFF9876543210E00001"),
                "OutgoingEncryptionAttributes": {"Symmetric": {"Mode": "CBC"}},
                "IncomingWrappedKey": wrapped,
            }),
        ),
    ];
    for (case, uri, body) in scaffolded {
        let (status, parsed) = data(&router, &uri, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (501, "NotImplemented"),
            "case {case:?}: a dynamic key nothing here establishes must say so, got \
             {status} {parsed}"
        );
    }
}

/// `POST /pindata/translate`, the URI the model binds `TranslatePinData` to.
const TRANSLATE_PIN_DATA_URI: &str = "/pindata/translate";

/// The `TranslationIsoFormats` member that names each ISO 9564 format, with the
/// `PinBlockFormatForPinData` value the PIN data operations name that same format by and
/// the `KeyAlgorithm` a PIN encryption key must carry for it.
///
/// Read off [`pin_block_formats`], which already owns that pairing, so one table serves
/// both halves of this suite.
fn translation_formats() -> Vec<(&'static str, &'static str, &'static str)> {
    pin_block_formats()
        .into_iter()
        .map(|(format, key_algorithm)| (translation_member(format), format, key_algorithm))
        .collect()
}

/// The `TranslationIsoFormats` member for one `PinBlockFormatForPinData` value: each ISO
/// member is the format of the same number.
///
/// Source: the modelled union's own members
/// (`smithy_query(query="shape",shape="TranslationIsoFormats")`: IsoFormat0, IsoFormat1,
/// IsoFormat3, IsoFormat4, As2805Format0) — the last names the Australian standard's own
/// block, which the scaffold case below covers rather than this table.
fn translation_member(format: &str) -> &'static str {
    match format {
        "ISO_FORMAT_0" => "IsoFormat0",
        "ISO_FORMAT_1" => "IsoFormat1",
        "ISO_FORMAT_3" => "IsoFormat3",
        "ISO_FORMAT_4" => "IsoFormat4",
        other => panic!("no TranslationIsoFormats member names PIN block format {other:?}"),
    }
}

/// The `TranslationIsoFormats` value for `member`: every ISO member but `IsoFormat1`
/// requires the account number, and `IsoFormat1`'s own structure has no member at all,
/// since a format 1 block binds no account number.
fn translation_attributes(member: &str) -> Value {
    if member == "IsoFormat1" {
        json!({member: {}})
    } else {
        json!({member: {"PrimaryAccountNumber": PIN_DATA_PAN}})
    }
}

/// The `TranslatePinData` request body.
fn translate_pin_data_body(
    incoming_key: &str,
    outgoing_key: &str,
    incoming_member: &str,
    outgoing_member: &str,
    encrypted_pin_block: &str,
) -> Value {
    json!({
        "IncomingKeyIdentifier": incoming_key,
        "OutgoingKeyIdentifier": outgoing_key,
        "IncomingTranslationAttributes": translation_attributes(incoming_member),
        "OutgoingTranslationAttributes": translation_attributes(outgoing_member),
        "EncryptedPinBlock": encrypted_pin_block,
    })
}

/// A PIN encryption key of `key_algorithm` that may both read and write a PIN block.
async fn pin_encryption_key(router: &MultiServiceRouter, key_algorithm: &str) -> Key {
    create_key(
        router,
        key_algorithm,
        USAGE_PIN_ENCRYPTION,
        encrypt_decrypt(),
    )
    .await
}

/// Whether a block of `incoming` may be translated into one of `outgoing`: "PIN block
/// format that requires PAN (example, formats 0,3,4) cannot be translated to a format
/// (format 1) that does not require a PAN for generation" (`TranslatePinData`'s own
/// documentation).
fn translatable(incoming: &str, outgoing: &str) -> bool {
    outgoing != "ISO_FORMAT_1" || incoming == "ISO_FORMAT_1"
}

/// One row of the translation-refusal table: the two identifiers, the incoming format
/// member, the block the request carries, and the refusal it must answer.
struct TranslationRefusal {
    case: &'static str,
    incoming: String,
    outgoing: String,
    incoming_member: &'static str,
    carried: String,
    status: u16,
    code: &'static str,
    /// The sentence the refusal states, or empty where the case asserts only the class.
    message: String,
}

#[tokio::test]
async fn a_pin_block_translates_between_every_pair_of_modelled_iso_formats() {
    let router = emulator();
    // The table spans the ISO members of the modelled `TranslationIsoFormats`, which are
    // the same four formats `PinBlockFormatForPinData` names: a format the model gains
    // later fails this assertion rather than going untested.
    let covered: BTreeSet<&str> = translation_formats()
        .iter()
        .map(|(_, format, _)| *format)
        .collect();
    assert_eq!(
        covered,
        PIN_BLOCK_FORMAT_FOR_PIN_DATA_VALUES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        "the translation table must span the modelled format catalog, which is \
         {PIN_BLOCK_FORMAT_FOR_PIN_DATA_VALUES:?}"
    );

    let pvk = create_key(
        &router,
        "TDES_2KEY",
        USAGE_VISA_PIN_VERIFICATION,
        generate_verify(),
    )
    .await;
    for (incoming_member, incoming_format, incoming_algorithm) in translation_formats() {
        for (outgoing_member, outgoing_format, outgoing_algorithm) in translation_formats() {
            if !translatable(incoming_format, outgoing_format) {
                continue;
            }
            // Both published bounds of the PIN a block carries (`IntegerRangeBetween4And12`
            // on `PinDataLength`): the block carries the PIN's own length, so a
            // translation has to preserve it whatever it is.
            for length in [4i64, 12] {
                let case = format!(
                    "{incoming_format} under {incoming_algorithm} to {outgoing_format} under \
                     {outgoing_algorithm}, PIN of {length} digits"
                );
                let incoming_key = pin_encryption_key(&router, incoming_algorithm).await;
                let outgoing_key = pin_encryption_key(&router, outgoing_algorithm).await;
                // The PIN this case works with: drawn by `GeneratePinData`, which answers
                // it inside a block encrypted under the incoming key and the Visa PIN
                // verification value of that very PIN.
                let generated = ok_data(
                    &router,
                    GENERATE_PIN_DATA_URI,
                    &generate_pin_data_body(
                        &pvk.arn,
                        &incoming_key.arn,
                        visa_pin(),
                        incoming_format,
                        Some(length),
                    ),
                )
                .await;
                let block = string(&generated, "EncryptedPinBlock").to_string();
                let (_, verification_value) = pin_data_member(&generated);

                let translated = ok_data(
                    &router,
                    TRANSLATE_PIN_DATA_URI,
                    &translate_pin_data_body(
                        &incoming_key.arn,
                        &outgoing_key.arn,
                        incoming_member,
                        outgoing_member,
                        &block,
                    ),
                )
                .await;
                assert_eq!(
                    string(&translated, "KeyArn"),
                    outgoing_key.arn,
                    "case {case}: the answer names the OUTGOING key's canonical ARN, got \
                     {translated}"
                );
                assert_eq!(
                    string(&translated, "KeyCheckValue"),
                    outgoing_key.key_check_value,
                    "case {case}: the answer names the outgoing key's own check value, got \
                     {translated}"
                );
                let outgoing_block = string(&translated, "PinBlock").to_string();
                assert!(
                    outgoing_block.len() == pin_block_characters(outgoing_format)
                        && outgoing_block.chars().all(|c| c.is_ascii_hexdigit()),
                    "case {case}: expected {} hexadecimal characters of outgoing PIN block, \
                     got {outgoing_block:?}",
                    pin_block_characters(outgoing_format)
                );
                assert_ne!(
                    outgoing_block, block,
                    "case {case}: a block encrypted under another key is not the block it \
                     was translated from, got {translated}"
                );

                // The PIN survived the translation: the value the generation answered is
                // the Visa PIN verification value of the PIN the TRANSLATED block carries,
                // read under the outgoing key in the outgoing format.
                let verified = ok_data(
                    &router,
                    VERIFY_PIN_DATA_URI,
                    &verify_pin_data_body(
                        &pvk.arn,
                        &outgoing_key.arn,
                        json!({"VisaPin": {
                            "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                            "VerificationValue": verification_value,
                        }}),
                        outgoing_format,
                        &outgoing_block,
                    ),
                )
                .await;
                assert_eq!(
                    string(&verified, "EncryptionKeyArn"),
                    outgoing_key.arn,
                    "case {case}: the verification runs under the outgoing key, got {verified}"
                );

                // And back the other way, wherever the return leg is a translation the
                // service performs: the PIN is still the one the first block carried.
                if translatable(outgoing_format, incoming_format) {
                    let returned = ok_data(
                        &router,
                        TRANSLATE_PIN_DATA_URI,
                        &translate_pin_data_body(
                            &outgoing_key.arn,
                            &incoming_key.arn,
                            outgoing_member,
                            incoming_member,
                            &outgoing_block,
                        ),
                    )
                    .await;
                    let returned_block = string(&returned, "PinBlock").to_string();
                    let reverified = ok_data(
                        &router,
                        VERIFY_PIN_DATA_URI,
                        &verify_pin_data_body(
                            &pvk.arn,
                            &incoming_key.arn,
                            json!({"VisaPin": {
                                "PinVerificationKeyIndex": PIN_VERIFICATION_KEY_INDEX,
                                "VerificationValue": verification_value,
                            }}),
                            incoming_format,
                            &returned_block,
                        ),
                    )
                    .await;
                    assert_eq!(
                        string(&reverified, "EncryptionKeyArn"),
                        incoming_key.arn,
                        "case {case}: translating back answers a block the incoming key \
                         reads the same PIN out of, got {reverified}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_translation_that_would_drop_the_account_number_is_refused() {
    let router = emulator();
    // Every format that binds the account number, translated to the one that does not.
    for (incoming_member, incoming_format, incoming_algorithm) in translation_formats() {
        if translatable(incoming_format, "ISO_FORMAT_1") {
            continue;
        }
        let case = format!("{incoming_format} to ISO_FORMAT_1");
        let incoming_key = pin_encryption_key(&router, incoming_algorithm).await;
        let outgoing_key = pin_encryption_key(&router, "TDES_2KEY").await;
        let pvk = create_key(
            &router,
            "TDES_2KEY",
            USAGE_VISA_PIN_VERIFICATION,
            generate_verify(),
        )
        .await;
        let block = generated_pin_block(
            &router,
            &pvk.arn,
            &incoming_key.arn,
            visa_pin(),
            incoming_format,
            None,
        )
        .await;
        let (status, parsed) = data(
            &router,
            TRANSLATE_PIN_DATA_URI,
            &translate_pin_data_body(
                &incoming_key.arn,
                &outgoing_key.arn,
                incoming_member,
                "IsoFormat1",
                &block,
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: a translation that drops the account number must be refused, got \
             {status} {parsed}"
        );
        assert_eq!(
            error_message(&parsed),
            format!(
                "A PIN block in PinBlockFormat {incoming_format} cannot be translated to \
                 PinBlockFormat ISO_FORMAT_1."
            ),
            "case {case}: the refusal must state the two formats, got {parsed}"
        );
    }
}

#[tokio::test]
async fn the_translation_keys_are_pin_encryption_keys_that_read_and_write_a_block() {
    let router = emulator();
    let pvk = create_key(
        &router,
        "TDES_2KEY",
        USAGE_VISA_PIN_VERIFICATION,
        generate_verify(),
    )
    .await;
    let incoming_key = pin_encryption_key(&router, "TDES_2KEY").await;
    let outgoing_key = pin_encryption_key(&router, "TDES_2KEY").await;
    let block = generated_pin_block(
        &router,
        &pvk.arn,
        &incoming_key.arn,
        visa_pin(),
        "ISO_FORMAT_0",
        None,
    )
    .await;
    // A PIN encryption key that may only write a block, and one that may only read one.
    let write_only = create_key(
        &router,
        "TDES_2KEY",
        USAGE_PIN_ENCRYPTION,
        json!({"Encrypt": true}),
    )
    .await;
    let read_only = create_key(
        &router,
        "TDES_2KEY",
        USAGE_PIN_ENCRYPTION,
        json!({"Decrypt": true}),
    )
    .await;
    let aes = pin_encryption_key(&router, "AES_128").await;

    let rows = vec![
        TranslationRefusal {
            case: "an incoming key that is not a PIN encryption key",
            incoming: pvk.arn.clone(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: format!(
                "KeyUsage {USAGE_VISA_PIN_VERIFICATION} is not valid for this operation."
            ),
        },
        TranslationRefusal {
            case: "an outgoing key that is not a PIN encryption key",
            incoming: incoming_key.arn.clone(),
            outgoing: pvk.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: format!(
                "KeyUsage {USAGE_VISA_PIN_VERIFICATION} is not valid for this operation."
            ),
        },
        TranslationRefusal {
            case: "an incoming key whose modes of use cannot read a block",
            incoming: write_only.arn.clone(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: format!(
                "The KeyModesOfUse of key {} do not include Decrypt.",
                write_only.arn
            ),
        },
        TranslationRefusal {
            case: "an outgoing key whose modes of use cannot write a block",
            incoming: incoming_key.arn.clone(),
            outgoing: read_only.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: format!(
                "The KeyModesOfUse of key {} do not include Encrypt.",
                read_only.arn
            ),
        },
        TranslationRefusal {
            case: "an incoming key whose algorithm names no cipher for the format",
            incoming: aes.arn.clone(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: "KeyAlgorithm AES_128 is not valid for PinBlockFormat ISO_FORMAT_0."
                .to_string(),
        },
        TranslationRefusal {
            case: "a block of another format's length",
            incoming: aes.arn.clone(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat4",
            carried: block.clone(),
            status: 400,
            code: "ValidationException",
            message: "An encrypted PIN block in PinBlockFormat ISO_FORMAT_4 carries 32 \
                      hexadecimal characters, not 16."
                .to_string(),
        },
        TranslationRefusal {
            case: "a block that carries no PIN field of the incoming format",
            incoming: incoming_key.arn.clone(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: hex_data(8),
            status: 400,
            code: "ValidationException",
            message: "The PIN block does not carry a PIN in PinBlockFormat ISO_FORMAT_0."
                .to_string(),
        },
        TranslationRefusal {
            case: "an incoming identifier that names no key",
            incoming: "alias/no-such-pin-key".to_string(),
            outgoing: outgoing_key.arn.clone(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message: String::new(),
        },
        TranslationRefusal {
            case: "an outgoing identifier that names no key",
            incoming: incoming_key.arn.clone(),
            outgoing: "alias/no-such-pin-key".to_string(),
            incoming_member: "IsoFormat0",
            carried: block.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message: String::new(),
        },
    ];
    for row in rows {
        let case = row.case;
        let (status, parsed) = data(
            &router,
            TRANSLATE_PIN_DATA_URI,
            &translate_pin_data_body(
                &row.incoming,
                &row.outgoing,
                row.incoming_member,
                "IsoFormat0",
                &row.carried,
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (row.status, row.code),
            "case {case:?}: expected {} {}, got {status} {parsed}",
            row.status,
            row.code
        );
        if !row.message.is_empty() {
            assert_eq!(
                error_message(&parsed),
                row.message,
                "case {case:?}: the refusal must state the rule it broke, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn the_translation_forms_this_slice_does_not_serve_still_say_so() {
    let router = emulator();
    let pvk = create_key(
        &router,
        "TDES_2KEY",
        USAGE_VISA_PIN_VERIFICATION,
        generate_verify(),
    )
    .await;
    let incoming_key = pin_encryption_key(&router, "TDES_2KEY").await;
    let outgoing_key = pin_encryption_key(&router, "TDES_2KEY").await;
    let block = generated_pin_block(
        &router,
        &pvk.arn,
        &incoming_key.arn,
        visa_pin(),
        "ISO_FORMAT_0",
        None,
    )
    .await;
    let base = translate_pin_data_body(
        &incoming_key.arn,
        &outgoing_key.arn,
        "IsoFormat0",
        "IsoFormat0",
        &block,
    );
    let wrapped = json!({
        "WrappedKeyMaterial": {"Tr31KeyBlock": "B0096P0TB00S0100KS1800604B120F9292800000A1B2C3D4"},
    });
    let dukpt = json!({"KeySerialNumber": "FFFF9876543210E00001"});
    // Each member of the input that names a key this slice does not derive, or a block
    // format it does not build: one request each, otherwise the one the round trip serves.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("an incoming DUKPT derived key", "IncomingDukptAttributes", dukpt.clone()),
        ("an outgoing DUKPT derived key", "OutgoingDukptAttributes", dukpt),
        ("an incoming dynamic or ECDH key", "IncomingWrappedKey", wrapped.clone()),
        ("an outgoing dynamic or ECDH key", "OutgoingWrappedKey", wrapped),
        (
            "an AS2805 derived key",
            "IncomingAs2805Attributes",
            json!({"SystemTraceAuditNumber": "123456", "TransactionAmount": "000000010000"}),
        ),
        (
            "the AS2805 format 0 block",
            "OutgoingTranslationAttributes",
            json!({"As2805Format0": {"PrimaryAccountNumber": PIN_DATA_PAN}}),
        ),
    ];
    for (case, member, value) in rows {
        let mut body = base.clone();
        if let Value::Object(target) = &mut body {
            target.insert(member.to_string(), value);
        }
        let (status, parsed) = data(&router, TRANSLATE_PIN_DATA_URI, &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (501, "NotImplemented"),
            "case {case:?}: a key or block nothing here derives must say so, got {status} \
             {parsed}"
        );
    }
}
