// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateDataKeyPairWithoutPlaintext`, driven over the wire
//! through the shared [`super::harness`].
//!
//! The rule under test: the operation generates a fresh key pair of the requested
//! `KeyPairSpec`, answers the public key as a DER-encoded X.509 `SubjectPublicKeyInfo`
//! and the matching private key sealed — as a DER-encoded PKCS#8 `PrivateKeyInfo` —
//! under the current key material of the symmetric encryption KMS key the `KeyId`
//! resolves to, and nothing else. The plaintext private key is nowhere in the
//! response, so the only way back to it is `Decrypt` on the
//! `PrivateKeyCiphertextBlob` with the same encryption context, and two calls never
//! yield the same pair.
//!
//! Every assertion is made over a TABLE whose rows are EVERY `DataKeyPairSpec` value,
//! so each row differs in the key pair type (RSA modulus size, elliptic curve,
//! Edwards curve, SM2, ML-KEM parameter set) — crossed, where the rule is about the
//! ciphertext rather than the pair, with the encryption context bound into it and with
//! how the request names the key.
//!
//! That the two halves of the answer belong to ONE pair is checked by deriving the
//! public key from the private key `Decrypt` recovers, with the same libraries that
//! generated it, and comparing the encodings byte for byte.

use aws_lc_rs::encoding::{AsDer, PublicKeyX509Der};
use aws_lc_rs::kem::{Algorithm, AlgorithmId, ML_KEM_1024, ML_KEM_512, ML_KEM_768};
use aws_lc_rs::signature::{
    EcdsaKeyPair, EcdsaSigningAlgorithm, Ed25519KeyPair, KeyPair as _,
    ECDSA_P256K1_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_ASN1_SIGNING,
    ECDSA_P384_SHA384_ASN1_SIGNING, ECDSA_P521_SHA512_ASN1_SIGNING,
};
use serde_json::{json, Map, Value};
use service_com_amazonaws_kms_trentservice::ops::types::DATA_KEY_PAIR_SPEC_VALUES;
use smithy_server_runtime::MultiServiceRouter;
use sm2::pkcs8::{DecodePrivateKey, EncodePublicKey};

use super::harness::{
    call, contains, create_alias, create_key, describe, error_code, is_lower_hex,
    ml_kem_public_key_spki, ok, router, text, unb64, REGION,
};

/// The operation under test.
const OPERATION: &str = "GenerateDataKeyPairWithoutPlaintext";

/// The length of a `BackingKeyIdType`: 64 characters matching `^[a-f0-9]+$`
/// (`smithy_query(query="operation",operation="GenerateDataKeyPairWithoutPlaintext")`).
const KEY_MATERIAL_ID_LEN: usize = 64;

/// The modelled maximum of a `CiphertextType`, which bounds the sealed private key
/// (`smithy_query(query="shape",shape="CiphertextType")`).
const MAX_CIPHERTEXT_BYTES: usize = 6144;

/// The modelled maximum of a `PublicKeyType`, which bounds the public key
/// (`smithy_query(query="shape",shape="PublicKeyType")`).
const MAX_PUBLIC_KEY_BYTES: usize = 8192;

/// Every member `GenerateDataKeyPairWithoutPlaintextResponse` declares
/// (`smithy_query(query="operation",operation="GenerateDataKeyPairWithoutPlaintext")`).
/// The set has no plaintext private key member at all, which is what distinguishes
/// this operation from `GenerateDataKeyPair`.
const RESPONSE_MEMBERS: [&str; 5] = [
    "PrivateKeyCiphertextBlob",
    "PublicKey",
    "KeyId",
    "KeyPairSpec",
    "KeyMaterialId",
];

/// The kind of key pair a `DataKeyPairSpec` names — what a row needs in order to
/// derive the public key again from the private key the answer sealed.
enum Pair {
    /// An RSA key pair of any modulus size.
    Rsa,
    /// An elliptic curve key pair on the curve this signing algorithm names.
    Ecdsa(&'static EcdsaSigningAlgorithm),
    /// An Ed25519 key pair.
    Ed25519,
    /// An SM2 key pair.
    Sm2,
    /// An ML-KEM key-encapsulation pair of this parameter set, named in an X.509
    /// `SubjectPublicKeyInfo` by this object identifier — `id-alg-ml-kem-512`, `-768` and
    /// `-1024` of the NIST Computer Security Objects Register, under the
    /// `2.16.840.1.101.3.4.4` arc.
    MlKem(&'static Algorithm<AlgorithmId>, &'static str),
}

/// One row: a `DataKeyPairSpec` value and the kind of pair it names.
struct SpecCase {
    spec: &'static str,
    pair: Pair,
}

/// EVERY `DataKeyPairSpec` value (`smithy_query(query="shape",shape="DataKeyPairSpec")`:
/// `ECC_NIST_EDWARDS25519`, `ECC_NIST_P256`, `ECC_NIST_P384`, `ECC_NIST_P521`,
/// `ECC_SECG_P256K1`, `ML_KEM_512`, `ML_KEM_768`, `ML_KEM_1024`, `RSA_2048`, `RSA_3072`,
/// `RSA_4096`, `SM2`), each with the kind of key pair it names — "Use the KeyPairSpec
/// parameter to choose an RSA or Elliptic Curve (ECC) data key pair. In China Regions, you
/// can also choose an SM2 data key pair"
/// (`smithy_query(query="doc",shape="GenerateDataKeyPairWithoutPlaintext")`).
const SPEC_CASES: &[SpecCase] = &[
    SpecCase {
        spec: "RSA_2048",
        pair: Pair::Rsa,
    },
    SpecCase {
        spec: "RSA_3072",
        pair: Pair::Rsa,
    },
    SpecCase {
        spec: "RSA_4096",
        pair: Pair::Rsa,
    },
    SpecCase {
        spec: "ECC_NIST_P256",
        pair: Pair::Ecdsa(&ECDSA_P256_SHA256_ASN1_SIGNING),
    },
    SpecCase {
        spec: "ECC_NIST_P384",
        pair: Pair::Ecdsa(&ECDSA_P384_SHA384_ASN1_SIGNING),
    },
    SpecCase {
        spec: "ECC_NIST_P521",
        pair: Pair::Ecdsa(&ECDSA_P521_SHA512_ASN1_SIGNING),
    },
    SpecCase {
        spec: "ECC_SECG_P256K1",
        pair: Pair::Ecdsa(&ECDSA_P256K1_SHA256_ASN1_SIGNING),
    },
    SpecCase {
        spec: "ECC_NIST_EDWARDS25519",
        pair: Pair::Ed25519,
    },
    SpecCase {
        spec: "SM2",
        pair: Pair::Sm2,
    },
    SpecCase {
        spec: "ML_KEM_512",
        pair: Pair::MlKem(&ML_KEM_512, "2.16.840.1.101.3.4.4.1"),
    },
    SpecCase {
        spec: "ML_KEM_768",
        pair: Pair::MlKem(&ML_KEM_768, "2.16.840.1.101.3.4.4.2"),
    },
    SpecCase {
        spec: "ML_KEM_1024",
        pair: Pair::MlKem(&ML_KEM_1024, "2.16.840.1.101.3.4.4.3"),
    },
];

/// The spec a case that is about the ciphertext rather than about the pair uses: the
/// cheapest pair to generate, so the ciphertext rows can be crossed with every
/// encryption context and every way of naming the key.
const CIPHERTEXT_SPEC: &str = "ECC_NIST_P256";

/// The rows span the model: every `DataKeyPairSpec` value the model declares is a
/// row, and no row names a value it does not — so a spec added to the model fails
/// here rather than going untested.
#[test]
fn the_rows_are_every_modelled_data_key_pair_spec() {
    let mut tabled: Vec<&str> = SPEC_CASES.iter().map(|row| row.spec).collect();
    tabled.sort_unstable();
    let mut modelled: Vec<&str> = DATA_KEY_PAIR_SPEC_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case the spec table: the rows must be exactly the modelled DataKeyPairSpec \
         values {modelled:?}, got {tabled:?}"
    );
}

/// The public key that belongs to the PKCS#8 private key `private`, as a DER-encoded
/// X.509 `SubjectPublicKeyInfo`.
///
/// The private key is re-imported by the library that implements the algorithm, which
/// is what makes this an independent derivation rather than a restatement of the
/// answer: a private key of another curve, or bytes that are not a PKCS#8 private key
/// of this type at all, cannot be imported here and the case fails.
fn derived_public_key(case: &str, pair: &Pair, private: &[u8]) -> Vec<u8> {
    match pair {
        Pair::Rsa => {
            let key = aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private)
                .unwrap_or_else(|error| panic!("case {case}: not an RSA PKCS#8 key: {error}"));
            let public: PublicKeyX509Der<'static> = AsDer::as_der(&key.public_key())
                .unwrap_or_else(|error| panic!("case {case}: {error}"));
            public.as_ref().to_vec()
        }
        Pair::Ecdsa(algorithm) => {
            let key = EcdsaKeyPair::from_pkcs8(algorithm, private).unwrap_or_else(|error| {
                panic!("case {case}: not an elliptic curve PKCS#8 key of this curve: {error}")
            });
            let public: PublicKeyX509Der<'static> = AsDer::as_der(key.public_key())
                .unwrap_or_else(|error| panic!("case {case}: {error}"));
            public.as_ref().to_vec()
        }
        Pair::Ed25519 => {
            let key = Ed25519KeyPair::from_pkcs8(private)
                .unwrap_or_else(|error| panic!("case {case}: not an Ed25519 PKCS#8 key: {error}"));
            let public: PublicKeyX509Der<'static> = AsDer::as_der(key.public_key())
                .unwrap_or_else(|error| panic!("case {case}: {error}"));
            public.as_ref().to_vec()
        }
        Pair::Sm2 => {
            let key = sm2::SecretKey::from_pkcs8_der(private)
                .unwrap_or_else(|error| panic!("case {case}: not an SM2 PKCS#8 key: {error}"));
            key.public_key()
                .to_public_key_der()
                .unwrap_or_else(|error| panic!("case {case}: {error}"))
                .as_ref()
                .to_vec()
        }
        Pair::MlKem(algorithm, object_identifier) => {
            ml_kem_public_key_spki(case, algorithm, object_identifier, private)
        }
    }
}

/// An encryption-context map as awsJson1_1 carries one.
fn context(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// The encryption contexts the ciphertext rows are crossed with: absent, one entry,
/// several.
fn context_cases() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no encryption context", None),
        ("one context entry", Some(context(&[("purpose", "test")]))),
        (
            "several context entries",
            Some(context(&[("a", "1"), ("b", "2"), ("c", "3")])),
        ),
    ]
}

/// The request a row describes.
fn request(spec: &str, key_id: &str, encryption_context: Option<&Value>) -> Value {
    let mut request = json!({"KeyId": key_id, "KeyPairSpec": spec});
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    request
}

/// Assert the answer carries only the modelled members — in particular no plaintext
/// private key — and that both halves respect their modelled bounds. Returns the
/// sealed private key as the wire carries it, with the public key's bytes.
fn sealed_only(case: &str, answer: &Value) -> (Value, Vec<u8>) {
    let members = answer
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a response object, got {answer}"));
    for member in members.keys() {
        assert!(
            RESPONSE_MEMBERS.contains(&member.as_str()),
            "case {case}: the response must carry only {RESPONSE_MEMBERS:?}, got the member \
             {member:?} in {answer}"
        );
    }
    assert!(
        answer["PrivateKeyCiphertextBlob"].is_string(),
        "case {case}: expected a PrivateKeyCiphertextBlob, got {answer}"
    );
    let blob = unb64(case, answer, "PrivateKeyCiphertextBlob");
    assert!(
        !blob.is_empty() && blob.len() <= MAX_CIPHERTEXT_BYTES,
        "case {case}: a sealed private key is 1..{MAX_CIPHERTEXT_BYTES} bytes, got {}",
        blob.len()
    );
    let public_key = unb64(case, answer, "PublicKey");
    assert!(
        !public_key.is_empty() && public_key.len() <= MAX_PUBLIC_KEY_BYTES,
        "case {case}: a public key is 1..{MAX_PUBLIC_KEY_BYTES} bytes, got {}",
        public_key.len()
    );
    (answer["PrivateKeyCiphertextBlob"].clone(), public_key)
}

/// The plaintext `Decrypt` recovers from `blob`, with the ciphertext's own encryption
/// context, together with the key material id and key ARN it reports.
async fn decrypted(
    router: &MultiServiceRouter,
    case: &str,
    blob: &Value,
    encryption_context: Option<&Value>,
) -> (Vec<u8>, Value, Value) {
    let mut request = json!({"CiphertextBlob": blob.clone()});
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    let answer = ok(router, "Decrypt", &request).await;
    (
        unb64(case, &answer, "Plaintext"),
        answer["KeyMaterialId"].clone(),
        answer["KeyId"].clone(),
    )
}

/// For every `DataKeyPairSpec`, the answer carries a public key and the matching
/// private key sealed and only sealed, the pair is of the requested type, and every
/// call generates a new one.
#[tokio::test]
async fn a_pair_of_the_requested_spec_comes_back_with_its_private_key_only_sealed() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "data key pairs"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let material_id = text("setup", &describe(&router, &key_id).await, "CurrentKeyMaterialId");

    for row in SPEC_CASES {
        let case = row.spec;
        let body = request(row.spec, &key_id, None);

        let first = ok(&router, OPERATION, &body).await;
        let (first_blob, first_public) = sealed_only(case, &first);
        let second = ok(&router, OPERATION, &body).await;
        let (second_blob, second_public) = sealed_only(case, &second);

        assert_eq!(
            first["KeyPairSpec"],
            json!(row.spec),
            "case {case}: the answer must report the spec that was generated, got {first}"
        );
        assert_eq!(
            first["KeyId"],
            json!(key_arn),
            "case {case}: the answer must name the sealing key by its ARN, got {first}"
        );
        let reported_material = text(case, &first, "KeyMaterialId");
        assert_eq!(
            reported_material, material_id,
            "case {case}: the answer must report the key material that sealed the private \
             key ({material_id}), got {reported_material}"
        );
        assert!(
            reported_material.len() == KEY_MATERIAL_ID_LEN && is_lower_hex(&reported_material),
            "case {case}: a KeyMaterialId is {KEY_MATERIAL_ID_LEN} lowercase hex characters, \
             got {reported_material:?}"
        );

        // The private key is reachable only through `Decrypt`, which is the round trip
        // the operation exists for — and what it recovers is the other half of the
        // public key that came back in the clear.
        let (private, decrypt_material, decrypt_key) =
            decrypted(&router, case, &first_blob, None).await;
        assert_eq!(
            decrypt_material,
            json!(material_id),
            "case {case}: Decrypt must report the material that sealed the private key, got \
             {decrypt_material}"
        );
        assert_eq!(
            decrypt_key,
            json!(key_arn),
            "case {case}: Decrypt must report the key that sealed the private key, got \
             {decrypt_key}"
        );
        let derived = derived_public_key(case, &row.pair, &private);
        assert_eq!(
            derived, first_public,
            "case {case}: the public key must be the one that belongs to the sealed private \
             key; the private key derives {} bytes of SubjectPublicKeyInfo and the answer \
             carries {} bytes",
            derived.len(),
            first_public.len()
        );
        assert!(
            !contains(&unb64(case, &first, "PrivateKeyCiphertextBlob"), &private),
            "case {case}: the ciphertext must not carry the private key in the clear"
        );

        // "returns a unique data key pair for each request": the second call answers
        // another pair, not the same one re-sealed.
        let (other, _, _) = decrypted(&router, case, &second_blob, None).await;
        assert_ne!(
            private, other,
            "case {case}: two calls must generate two different private keys"
        );
        assert_ne!(
            first_public, second_public,
            "case {case}: two calls must generate two different public keys"
        );
        assert_eq!(
            derived_public_key(case, &row.pair, &other),
            second_public,
            "case {case}: every call must answer the public key of the pair it generated"
        );
    }
}

/// The sealed private key is bound to the encryption context the request gave, and
/// `Decrypt` recovers it under that context alone.
#[tokio::test]
async fn the_private_key_is_sealed_under_the_requests_encryption_context() {
    let router = router();
    let key_id = text(
        "setup",
        &create_key(&router, &json!({"Description": "contexts"})).await,
        "KeyId",
    );
    let pair = Pair::Ecdsa(&ECDSA_P256_SHA256_ASN1_SIGNING);

    for (case, encryption_context) in context_cases() {
        let body = request(CIPHERTEXT_SPEC, &key_id, encryption_context.as_ref());
        let answer = ok(&router, OPERATION, &body).await;
        let (blob, public) = sealed_only(case, &answer);

        let (private, _, _) = decrypted(&router, case, &blob, encryption_context.as_ref()).await;
        assert_eq!(
            derived_public_key(case, &pair, &private),
            public,
            "case {case}: the pair must survive the round trip through the encryption \
             context it was sealed under"
        );

        // The encryption context is authenticated, so opening the ciphertext under any
        // other context fails rather than returning the private key.
        let wrong = match &encryption_context {
            // A ciphertext sealed with no context is not openable under one.
            None => Some(context(&[("unexpected", "context")])),
            // A ciphertext sealed WITH a context is not openable without it.
            Some(_) => None,
        };
        let mut probe = json!({"CiphertextBlob": blob.clone()});
        if let Some(wrong) = &wrong {
            probe["EncryptionContext"] = wrong.clone();
        }
        let (status, payload) = call(&router, REGION, "Decrypt", &probe).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "InvalidCiphertextException"),
            "case {case}: a private key sealed under one encryption context must not open \
             under another, got {status} with {payload}"
        );
    }
}

/// Whatever identifier names the key — key id, key ARN or alias — the answer reports
/// that key's ARN and the material of THAT key, read back through `DescribeKey`.
#[tokio::test]
async fn the_answer_names_the_resolved_key_by_arn_whatever_identifier_named_it() {
    let router = router();

    // Two keys, so the answer is read per key rather than from one global value, and
    // an alias so each can also be named indirectly.
    let mut keys = Vec::new();
    for (case, description) in [("first key", "first"), ("second key", "second")] {
        let metadata = create_key(&router, &json!({"Description": description})).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let alias = format!("alias/data-key-pair-{description}");
        create_alias(&router, case, &alias, &key_id).await;
        let material_id = text(case, &describe(&router, &key_id).await, "CurrentKeyMaterialId");
        keys.push((case, key_id, key_arn, alias, material_id));
    }

    for (key_case, key_id, key_arn, alias, material_id) in &keys {
        for (form, identifier) in [
            ("by key id", key_id.as_str()),
            ("by key ARN", key_arn.as_str()),
            ("by alias", alias.as_str()),
        ] {
            let case = format!("{key_case} / {form}");
            let answer = ok(
                &router,
                OPERATION,
                &request(CIPHERTEXT_SPEC, identifier, None),
            )
            .await;
            let (blob, public) = sealed_only(&case, &answer);
            assert_eq!(
                answer["KeyId"],
                json!(key_arn),
                "case {case}: the answer must name the resolved key by its ARN, got {answer}"
            );
            assert_eq!(
                answer["KeyMaterialId"],
                json!(material_id),
                "case {case}: the answer must report that key's current material, got {answer}"
            );

            // The ciphertext is a ciphertext OF that key: `Decrypt` opens it, reports
            // the same key, and yields the private key of the public key that came
            // back.
            let (private, _, decrypt_key) = decrypted(&router, &case, &blob, None).await;
            assert_eq!(
                decrypt_key,
                json!(key_arn),
                "case {case}: Decrypt must report the key that sealed the private key, got \
                 {decrypt_key}"
            );
            assert_eq!(
                derived_public_key(&case, &Pair::Ecdsa(&ECDSA_P256_SHA256_ASN1_SIGNING), &private),
                public,
                "case {case}: the answer must carry the two halves of one pair"
            );
        }
    }
}

/// A request that names no key pair this operation can generate, or a key that cannot
/// encrypt a private key, is refused — and nothing is generated.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_its_own_error() {
    let router = router();
    let account = engine::emulator_account_id();

    let enabled = create_key(&router, &json!({"Description": "enabled"})).await;
    let enabled_id = text("setup", &enabled, "KeyId");
    let disabled = create_key(&router, &json!({"Description": "disabled"})).await;
    let disabled_id = text("setup", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    let pending = create_key(&router, &json!({"Description": "pending"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;

    // Keys of each other key kind: a `KeyUsage` that is not ENCRYPT_DECRYPT, and an
    // asymmetric ENCRYPT_DECRYPT key, neither of which can encrypt the private key in
    // a data key pair.
    let hmac = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        )
        .await,
        "KeyId",
    );
    let signing = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        )
        .await,
        "KeyId",
    );
    let asymmetric_encrypt = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "RSA_3072", "KeyUsage": "ENCRYPT_DECRYPT"}),
        )
        .await,
        "KeyId",
    );
    let agreement = text(
        "setup",
        &create_key(
            &router,
            &json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "KEY_AGREEMENT"}),
        )
        .await,
        "KeyId",
    );

    // Rows: the case, the request, and the wire error code expected. Every client
    // refusal of this service answers HTTP 400.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a key id that names no key",
            json!({"KeyId": "00000000-0000-0000-0000-000000000000",
                   "KeyPairSpec": CIPHERTEXT_SPEC}),
            "NotFoundException",
        ),
        (
            "an alias that resolves to no key",
            json!({"KeyId": "alias/no-such-alias", "KeyPairSpec": CIPHERTEXT_SPEC}),
            "NotFoundException",
        ),
        (
            // This operation models no `InvalidArnException`, so a malformed ARN is
            // reported the way an identifier naming nothing is.
            "an ARN that is not a KMS key ARN",
            json!({"KeyId": format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
                   "KeyPairSpec": CIPHERTEXT_SPEC}),
            "NotFoundException",
        ),
        (
            "a disabled key",
            json!({"KeyId": disabled_id, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "DisabledException",
        ),
        (
            "a key pending deletion",
            json!({"KeyId": pending_id, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "KMSInvalidStateException",
        ),
        (
            "an HMAC key",
            json!({"KeyId": hmac, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "InvalidKeyUsageException",
        ),
        (
            "a signing key",
            json!({"KeyId": signing, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "InvalidKeyUsageException",
        ),
        (
            "an asymmetric encryption key",
            json!({"KeyId": asymmetric_encrypt, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "InvalidKeyUsageException",
        ),
        (
            "a key agreement key",
            json!({"KeyId": agreement, "KeyPairSpec": CIPHERTEXT_SPEC}),
            "InvalidKeyUsageException",
        ),
        (
            "no KeyPairSpec at all",
            json!({"KeyId": enabled_id}),
            "ValidationException",
        ),
        (
            "a KeyPairSpec that is not a DataKeyPairSpec value",
            json!({"KeyId": enabled_id, "KeyPairSpec": "RSA_1024"}),
            "ValidationException",
        ),
        (
            // A KMS key spec that is not a DATA key pair spec: `ML_DSA_65` is a
            // `KeySpec` a KMS key can have, and no `DataKeyPairSpec`.
            "a KeySpec that is not a DataKeyPairSpec value",
            json!({"KeyId": enabled_id, "KeyPairSpec": "ML_DSA_65"}),
            "ValidationException",
        ),
        (
            "a KeyPairSpec in the wrong case",
            json!({"KeyId": enabled_id, "KeyPairSpec": "rsa_2048"}),
            "ValidationException",
        ),
        (
            "a dry run",
            json!({"KeyId": enabled_id, "KeyPairSpec": CIPHERTEXT_SPEC, "DryRun": true}),
            "DryRunOperationException",
        ),
        (
            "a grant token KMS never issued",
            json!({"KeyId": enabled_id, "KeyPairSpec": CIPHERTEXT_SPEC,
                   "GrantTokens": ["not-a-grant-token"]}),
            "InvalidGrantTokenException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, payload) = call(&router, REGION, OPERATION, &body).await;
        let code = error_code(&payload);
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} with {payload}"
        );
    }
}

/// A request whose permission comes from a grant token is served exactly when that
/// grant covers it: the right operation on the right key, with the request's
/// encryption context satisfying the grant's constraints.
#[tokio::test]
async fn a_grant_token_is_honoured_only_for_what_the_grant_permits() {
    let router = router();
    let key_id = text(
        "setup",
        &create_key(&router, &json!({"Description": "granted"})).await,
        "KeyId",
    );
    let grantee = format!(
        "arn:aws:iam::{account}:user/Alice",
        account = engine::emulator_account_id()
    );

    let permitting = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": [OPERATION]}),
        )
        .await,
        "GrantToken",
    );
    let unrelated = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": ["Encrypt"]}),
        )
        .await,
        "GrantToken",
    );
    let constrained = text(
        "setup",
        &ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": key_id, "GranteePrincipal": grantee,
                    "Operations": [OPERATION],
                    "Constraints": {"EncryptionContextEquals": {"purpose": "test"}}}),
        )
        .await,
        "GrantToken",
    );

    // Rows: the case, the token presented, the encryption context the request carries,
    // and the error code expected — `None` where the grant covers the request.
    let cases: Vec<(&str, &str, Option<Value>, Option<&str>)> = vec![
        (
            "a grant permitting this operation",
            &permitting,
            None,
            None,
        ),
        (
            "a grant permitting only another operation",
            &unrelated,
            None,
            Some("AccessDeniedException"),
        ),
        (
            "a constrained grant with the context it requires",
            &constrained,
            Some(context(&[("purpose", "test")])),
            None,
        ),
        (
            "a constrained grant with another context",
            &constrained,
            Some(context(&[("purpose", "other")])),
            Some("AccessDeniedException"),
        ),
        (
            "a constrained grant with no context",
            &constrained,
            None,
            Some("AccessDeniedException"),
        ),
    ];

    for (case, token, encryption_context, expected) in cases {
        let mut body = request(CIPHERTEXT_SPEC, &key_id, encryption_context.as_ref());
        body["GrantTokens"] = json!([token]);
        let (status, payload) = call(&router, REGION, OPERATION, &body).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                let (blob, public) = sealed_only(case, &payload);
                let (private, _, _) =
                    decrypted(&router, case, &blob, encryption_context.as_ref()).await;
                assert_eq!(
                    derived_public_key(
                        case,
                        &Pair::Ecdsa(&ECDSA_P256_SHA256_ASN1_SIGNING),
                        &private
                    ),
                    public,
                    "case {case:?}: a grant-authorized request must answer one pair"
                );
            }
            Some(expected) => {
                let code = error_code(&payload);
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: expected HTTP 400 {expected}, got {status} with {payload}"
                );
            }
        }
    }
}
