// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KEY-ENCAPSULATION surface of this service: a KMS key whose
//! `KeyUsage` is `ENCAPSULATE_DECAPSULATE`, the public encapsulation key a caller
//! downloads for it, and the two directions `Encapsulate` and `Decapsulate` drive.
//!
//! The RULE asserted here: **a key spec that names a key-encapsulation mechanism is
//! minted under `ENCAPSULATE_DECAPSULATE` like any other spec under its own usage, its
//! public half is downloadable as a DER `SubjectPublicKeyInfo` naming that mechanism's
//! parameter set, `Encapsulate` answers a fresh shared secret with the ciphertext that
//! recovers it, and `Decapsulate` run on that ciphertext under the same key recovers the
//! identical secret** — with the mechanism's own rules for everything else: a ciphertext
//! of the same parameter set made under another key decapsulates to an unrelated secret
//! rather than failing, and a byte string that is no ciphertext of the key's parameter set
//! is refused.
//!
//! Every test is a TABLE over the three `ML_KEM_*` key specs, which is what the rule
//! generalises over: the parameter set decides the size of the encapsulation key, the size
//! of the ciphertext and the algorithm identifier the public key is named by, and nothing
//! else about the surface changes with it. The sizes and identifiers are the published
//! ones (see [`ML_KEM_SPECS`]); no row carries a key id, a secret or a ciphertext written
//! here, because every one of those is a value the service produced for that run.

use serde_json::json;
use spki::der::Decode;

use super::harness::{
    b64, call, contains, create_key, describe, error_code, ok, one_violation, principal, refused,
    router, text, unb64, REGION,
};

/// One ML-KEM key spec as this suite drives it.
struct MlKemCase {
    /// The `KeySpec` value (`smithy_query(shape="KeySpec")`).
    key_spec: &'static str,
    /// The size in bytes of the mechanism's encapsulation key (its public half) and of
    /// its ciphertext, from FIPS 203 Table 3 ("Sizes (in bytes) of keys and ciphertexts of
    /// ML-KEM"): ML-KEM-512 800/768, ML-KEM-768 1184/1088, ML-KEM-1024 1568/1568.
    encapsulation_key_len: usize,
    ciphertext_len: usize,
    /// The algorithm identifier an X.509 `SubjectPublicKeyInfo` names that parameter set
    /// by: `id-alg-ml-kem-512`, `-768` and `-1024` of the NIST Computer Security Objects
    /// Register, under the `2.16.840.1.101.3.4.4` arc.
    object_identifier: &'static str,
}

/// Every key spec the mechanism has a parameter set for. A row the issue did not cite
/// (ML-KEM-512 and ML-KEM-1024) is asserted on exactly like the one it did.
const ML_KEM_SPECS: &[MlKemCase] = &[
    MlKemCase {
        key_spec: "ML_KEM_512",
        encapsulation_key_len: 800,
        ciphertext_len: 768,
        object_identifier: "2.16.840.1.101.3.4.4.1",
    },
    MlKemCase {
        key_spec: "ML_KEM_768",
        encapsulation_key_len: 1184,
        ciphertext_len: 1088,
        object_identifier: "2.16.840.1.101.3.4.4.2",
    },
    MlKemCase {
        key_spec: "ML_KEM_1024",
        encapsulation_key_len: 1568,
        ciphertext_len: 1568,
        object_identifier: "2.16.840.1.101.3.4.4.3",
    },
];

/// The `KeyUsage` a key-encapsulation key carries
/// (`smithy_query(shape="KeyUsageType")`).
const ENCAPSULATE_DECAPSULATE: &str = "ENCAPSULATE_DECAPSULATE";

/// The one `EncapsulationAlgorithmSpec` the model declares
/// (`smithy_query(shape="EncapsulationAlgorithmSpec")`). `EncapsulationAlgorithm` is a
/// REQUIRED member of both requests
/// (`smithy_query(query="operation",operation="Encapsulate,Decapsulate")`), so every
/// call below names it.
const ML_KEM: &str = "ML_KEM";

/// Every member `EncapsulateResponse` declares
/// (`smithy_query(query="operation",operation="Encapsulate")`).
const ENCAPSULATE_RESPONSE_MEMBERS: &[&str] = &[
    "KeyId",
    "SharedSecret",
    "SharedSecretCiphertextBlob",
    "EncapsulationAlgorithm",
];

/// Every member `DecapsulateResponse` declares
/// (`smithy_query(query="operation",operation="Decapsulate")`).
const DECAPSULATE_RESPONSE_MEMBERS: &[&str] = &["KeyId", "SharedSecret", "EncapsulationAlgorithm"];

/// Algorithm names drawn from the service's OTHER algorithm enums
/// (`smithy_query(shape="EncryptionAlgorithmSpec")` and
/// `smithy_query(shape="KeyAgreementAlgorithmSpec")`), one ML-KEM KEY SPEC name, and one
/// casing of the declared value — none of them a value `EncapsulationAlgorithmSpec`
/// declares.
const OTHER_ALGORITHMS: &[&str] = &[
    "SYMMETRIC_DEFAULT",
    "RSAES_OAEP_SHA_256",
    "ECDH",
    "ML_KEM_768",
    "ml_kem",
];

/// A success carries exactly the members the response shape declares, so no member the
/// model does not declare reaches the wire and none it declares is missing.
fn assert_members(case: &str, answer: &serde_json::Value, declared: &[&str]) {
    let mut reported: Vec<&str> = answer
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a JSON object, got {answer}"))
        .keys()
        .map(String::as_str)
        .collect();
    reported.sort_unstable();
    let mut expected: Vec<&str> = declared.to_vec();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case {case}: expected exactly the declared members {expected:?}, got {reported:?} \
         in {answer}"
    );
}

/// The clause the boundary states for a REQUIRED member a request does not carry, under
/// the member's wire path.
fn absent_member(member: &str) -> String {
    one_violation(&format!(
        "Value null at '{member}' failed to satisfy constraint: Member must not be null"
    ))
}

/// The clause the boundary states for a value outside the value set an enum member's
/// target declares, under the member's wire path.
fn outside_value_set(member: &str, value: &str, declared: &[&str]) -> String {
    one_violation(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         enum value set: [{}]",
        declared.join(", ")
    ))
}

/// The length of a shared secret the mechanism establishes: 32 bytes, the shared-secret
/// size FIPS 203 fixes for every ML-KEM parameter set.
const SHARED_SECRET_LEN: usize = 32;

/// How KMS words the refusal of a ciphertext whose length is not the one `spec`'s
/// parameter set gives: fixed text, with the key spec and both lengths read from the key
/// and the request. Unlike the service's other invalid-ciphertext class (`Decrypt`, which
/// carries no message), this one states what is wrong with the blob, so the wording is
/// asserted here rather than left open.
fn length_mismatch(spec: &MlKemCase, submitted: usize) -> String {
    format!(
        "SharedSecretCiphertextBlob length mismatch for {} KeySpec: expected {} bytes, \
         got {submitted} bytes.",
        spec.key_spec, spec.ciphertext_len
    )
}

/// A key of `key_spec` under [`ENCAPSULATE_DECAPSULATE`], as (key id, key ARN).
async fn encapsulation_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_spec: &str,
) -> (String, String) {
    let metadata = create_key(
        router,
        &json!({"Description": key_spec, "KeySpec": key_spec,
                "KeyUsage": ENCAPSULATE_DECAPSULATE}),
    )
    .await;
    (
        text(key_spec, &metadata, "KeyId"),
        text(key_spec, &metadata, "Arn"),
    )
}

/// `CreateKey` mints a key of every ML-KEM key spec under the key-encapsulation usage,
/// and `DescribeKey` reports the same record afterwards — by key id and by key ARN.
#[tokio::test]
async fn create_key_mints_a_key_of_every_ml_kem_key_spec() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let metadata = create_key(
            &router,
            &json!({"Description": case, "KeySpec": case,
                    "KeyUsage": ENCAPSULATE_DECAPSULATE}),
        )
        .await;

        assert_eq!(
            text(case, &metadata, "KeySpec"),
            case,
            "case {case}: expected the requested KeySpec, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeyUsage"),
            ENCAPSULATE_DECAPSULATE,
            "case {case}: expected KeyUsage {ENCAPSULATE_DECAPSULATE}, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeyState"),
            "Enabled",
            "case {case}: a new key with KMS-generated material is Enabled, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "Origin"),
            "AWS_KMS",
            "case {case}: KMS generated this key's material, got {metadata}"
        );
        // The deprecated spec enum was frozen before these specs existed, so it names
        // none of them, and the per-usage algorithm lists belong to the other usages.
        assert!(
            metadata["CustomerMasterKeySpec"].is_null(),
            "case {case}: the deprecated CustomerMasterKeySpec cannot name this spec, so \
             it must be omitted, got {metadata}"
        );
        for member in [
            "EncryptionAlgorithms",
            "SigningAlgorithms",
            "MacAlgorithms",
            "KeyAgreementAlgorithms",
        ] {
            assert!(
                metadata[member].is_null(),
                "case {case}: {member} belongs to another KeyUsage and must be omitted, \
                 got {metadata}"
            );
        }
        // The list that DOES belong to this usage is the one mechanism the spec names.
        assert_eq!(
            metadata["EncapsulationAlgorithms"],
            json!([ML_KEM]),
            "case {case}: expected EncapsulationAlgorithms [{ML_KEM}], got {metadata}"
        );

        let key_id = text(case, &metadata, "KeyId");
        let arn = text(case, &metadata, "Arn");
        for identifier in [key_id.as_str(), arn.as_str()] {
            assert_eq!(
                describe(&router, identifier).await,
                metadata,
                "case {case}: DescribeKey({identifier}) must report what CreateKey returned"
            );
        }
    }
}

/// `GetPublicKey` answers the key's ENCAPSULATION key as a DER `SubjectPublicKeyInfo`
/// that names the spec's parameter set and carries a key of that parameter set's size —
/// and reading it twice answers the same bytes, since a read computes the public half
/// from the material the key already holds.
#[tokio::test]
async fn get_public_key_downloads_the_encapsulation_key_of_every_ml_kem_spec() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (key_id, arn) = encapsulation_key(&router, spec.key_spec).await;

        let answer = ok(&router, "GetPublicKey", &json!({"KeyId": key_id})).await;
        assert_eq!(
            text(case, &answer, "KeyId"),
            arn,
            "case {case}: GetPublicKey answers the key ARN, got {answer}"
        );
        assert_eq!(
            text(case, &answer, "KeySpec"),
            case,
            "case {case}: expected KeySpec {case}, got {answer}"
        );
        assert_eq!(
            text(case, &answer, "KeyUsage"),
            ENCAPSULATE_DECAPSULATE,
            "case {case}: expected KeyUsage {ENCAPSULATE_DECAPSULATE}, got {answer}"
        );
        // The algorithm list a public key reports is the one belonging to its usage.
        assert_eq!(
            answer["EncapsulationAlgorithms"],
            json!([ML_KEM]),
            "case {case}: expected EncapsulationAlgorithms [{ML_KEM}], got {answer}"
        );
        for member in [
            "EncryptionAlgorithms",
            "SigningAlgorithms",
            "KeyAgreementAlgorithms",
        ] {
            assert!(
                answer[member].is_null(),
                "case {case}: {member} belongs to another KeyUsage and must be omitted, \
                 got {answer}"
            );
        }

        let public_key = unb64(case, &answer, "PublicKey");
        let parsed = spki::SubjectPublicKeyInfoOwned::from_der(&public_key).unwrap_or_else(|error| {
            panic!("case {case}: the PublicKey must be a DER SubjectPublicKeyInfo: {error}")
        });
        assert_eq!(
            parsed.algorithm.oid.to_string(),
            spec.object_identifier,
            "case {case}: expected the algorithm identifier {}, got {}",
            spec.object_identifier,
            parsed.algorithm.oid
        );
        assert!(
            parsed.algorithm.parameters.is_none(),
            "case {case}: this algorithm identifier carries no parameters, got {:?}",
            parsed.algorithm.parameters
        );
        let key_bytes = parsed
            .subject_public_key
            .as_bytes()
            .unwrap_or_else(|| panic!("case {case}: the key must be whole bytes"));
        assert_eq!(
            key_bytes.len(),
            spec.encapsulation_key_len,
            "case {case}: expected a {}-byte encapsulation key, got {}",
            spec.encapsulation_key_len,
            key_bytes.len()
        );

        // A read changes nothing: the same key answers the same public half.
        let again = ok(&router, "GetPublicKey", &json!({"KeyId": arn})).await;
        assert_eq!(
            unb64(case, &again, "PublicKey"),
            public_key,
            "case {case}: the public half must be stable across reads, got {again}"
        );
    }
}

/// `Encapsulate` answers a 32-byte shared secret, the ciphertext of the spec's parameter
/// set, the key ARN and the mechanism it ran; `Decapsulate` of that ciphertext under the
/// same key recovers the identical secret. Two encapsulations under one key establish
/// DIFFERENT secrets, because each draws its own randomness.
#[tokio::test]
async fn encapsulate_and_decapsulate_round_trip_one_shared_secret() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (key_id, arn) = encapsulation_key(&router, spec.key_spec).await;

        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let shared_secret = unb64(case, &encapsulated, "SharedSecret");
        let ciphertext = unb64(case, &encapsulated, "SharedSecretCiphertextBlob");
        assert_eq!(
            shared_secret.len(),
            SHARED_SECRET_LEN,
            "case {case}: expected a {SHARED_SECRET_LEN}-byte shared secret, got {}",
            shared_secret.len()
        );
        assert_eq!(
            ciphertext.len(),
            spec.ciphertext_len,
            "case {case}: expected a {}-byte ciphertext, got {}",
            spec.ciphertext_len,
            ciphertext.len()
        );
        assert_eq!(
            text(case, &encapsulated, "EncapsulationAlgorithm"),
            ML_KEM,
            "case {case}: expected EncapsulationAlgorithm {ML_KEM}, got {encapsulated}"
        );
        assert_eq!(
            text(case, &encapsulated, "KeyId"),
            arn,
            "case {case}: Encapsulate answers the key ARN, got {encapsulated}"
        );
        assert_members(case, &encapsulated, ENCAPSULATE_RESPONSE_MEMBERS);
        assert!(
            !contains(&ciphertext, &shared_secret),
            "case {case}: the ciphertext must not carry the shared secret"
        );

        // The round trip, with the key named by ARN this time: every identifier form
        // names the same key material.
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": arn, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &decapsulated, "SharedSecret"),
            shared_secret,
            "case {case}: Decapsulate must recover the secret Encapsulate established, \
             got {decapsulated}"
        );
        assert_eq!(
            text(case, &decapsulated, "EncapsulationAlgorithm"),
            ML_KEM,
            "case {case}: expected EncapsulationAlgorithm {ML_KEM}, got {decapsulated}"
        );
        assert_eq!(
            text(case, &decapsulated, "KeyId"),
            arn,
            "case {case}: Decapsulate answers the key ARN, got {decapsulated}"
        );
        assert_members(case, &decapsulated, DECAPSULATE_RESPONSE_MEMBERS);
        // Reading the same ciphertext again recovers the same secret: decapsulation
        // consumes nothing.
        let repeated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": arn, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &repeated, "SharedSecret"),
            shared_secret,
            "case {case}: a ciphertext decapsulates to the same secret every time, got \
             {repeated}"
        );

        // A second encapsulation under the same key is a new key establishment.
        let again = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": arn, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_ne!(
            unb64(case, &again, "SharedSecret"),
            shared_secret,
            "case {case}: each encapsulation establishes its own secret, got {again}"
        );
        assert_ne!(
            unb64(case, &again, "SharedSecretCiphertextBlob"),
            ciphertext,
            "case {case}: each encapsulation produces its own ciphertext, got {again}"
        );
        let other_secret = unb64(case, &again, "SharedSecret");
        let other_ciphertext = unb64(case, &again, "SharedSecretCiphertextBlob");
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": arn, "SharedSecretCiphertextBlob": b64(&other_ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &decapsulated, "SharedSecret"),
            other_secret,
            "case {case}: each ciphertext recovers its own secret, got {decapsulated}"
        );
    }
}

/// A ciphertext of the key's parameter set made under ANOTHER key decapsulates to a
/// 32-byte secret that matches neither side — the mechanism's implicit rejection, which
/// is a success rather than a refusal, so a caller cannot learn from the answer whether
/// the ciphertext was meant for this key.
#[tokio::test]
async fn decapsulating_another_keys_ciphertext_yields_an_unrelated_secret() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (mine, _) = encapsulation_key(&router, spec.key_spec).await;
        let (theirs, _) = encapsulation_key(&router, spec.key_spec).await;

        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": theirs, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let their_secret = unb64(case, &encapsulated, "SharedSecret");
        let ciphertext = unb64(case, &encapsulated, "SharedSecretCiphertextBlob");

        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": mine, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let recovered = unb64(case, &decapsulated, "SharedSecret");
        assert_eq!(
            recovered.len(),
            SHARED_SECRET_LEN,
            "case {case}: implicit rejection still answers a {SHARED_SECRET_LEN}-byte \
             secret, got {}",
            recovered.len()
        );
        assert_ne!(
            recovered, their_secret,
            "case {case}: another key's ciphertext must not recover that key's secret"
        );
        // And it is stable: the rejection is a function of this key and this ciphertext.
        let repeated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": mine, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &repeated, "SharedSecret"),
            recovered,
            "case {case}: the same ciphertext and key answer the same secret, got {repeated}"
        );
    }
}

/// A `SharedSecretCiphertextBlob` whose LENGTH is not the one the named key's parameter
/// set gives is refused `InvalidCiphertextException`, by a sentence naming the member, the
/// key spec that decides the length, the length that spec requires and the length the
/// request carried.
///
/// The rows span what "not that length" can be: one byte, one byte short of the published
/// length, one byte past it, the length of ANOTHER parameter set's ciphertext, and a
/// ciphertext this very spec produced but truncated.
#[tokio::test]
async fn decapsulate_refuses_a_ciphertext_of_the_wrong_length() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (key_id, _) = encapsulation_key(&router, spec.key_spec).await;
        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let ciphertext = unb64(case, &encapsulated, "SharedSecretCiphertextBlob");

        let other_len = ML_KEM_SPECS
            .iter()
            .map(|other| other.ciphertext_len)
            .find(|len| *len != spec.ciphertext_len)
            .expect("the table holds more than one ciphertext size");
        let rows: Vec<(String, Vec<u8>)> = vec![
            ("a single byte".to_string(), vec![0u8; 1]),
            (
                "one byte short of the parameter set's ciphertext".to_string(),
                vec![0u8; spec.ciphertext_len - 1],
            ),
            (
                "one byte past the parameter set's ciphertext".to_string(),
                vec![0u8; spec.ciphertext_len + 1],
            ),
            (
                format!("another parameter set's {other_len}-byte ciphertext"),
                vec![0u8; other_len],
            ),
            (
                "this key's own ciphertext, truncated".to_string(),
                ciphertext[..ciphertext.len() - 1].to_vec(),
            ),
        ];

        for (row, bytes) in rows {
            let case = format!("{case}: {row}");
            let (status, code, message) = refused(
                &router,
                REGION,
                "Decapsulate",
                &json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&bytes),
                        "EncapsulationAlgorithm": ML_KEM}),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidCiphertextException"),
                "case {case}: expected HTTP 400 InvalidCiphertextException, got {status} \
                 {code}"
            );
            let expected = length_mismatch(spec, bytes.len());
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
        }

        // The whole ciphertext is still good: a refused call changed nothing.
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &decapsulated, "SharedSecret"),
            unb64(case, &encapsulated, "SharedSecret"),
            "case {case}: a refused ciphertext must not disturb a valid one, got \
             {decapsulated}"
        );
    }
}

/// A key whose `KeyUsage` is not `ENCAPSULATE_DECAPSULATE` cannot run either direction:
/// both operations refuse it `InvalidKeyUsageException`, and so does an ML-KEM key the
/// request asks the wrong thing of — there is no other usage such a key can carry.
///
/// The rows span the other usages a key can carry, over symmetric, HMAC and asymmetric
/// key specs.
#[tokio::test]
async fn encapsulate_and_decapsulate_refuse_a_key_that_does_not_encapsulate() {
    let router = router();

    let rows = [
        ("symmetric encryption key", "SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT"),
        ("HMAC key", "HMAC_256", "GENERATE_VERIFY_MAC"),
        ("RSA encryption key", "RSA_2048", "ENCRYPT_DECRYPT"),
        ("RSA signing key", "RSA_2048", "SIGN_VERIFY"),
        ("elliptic-curve agreement key", "ECC_NIST_P256", "KEY_AGREEMENT"),
        ("ML-DSA signing key", "ML_DSA_44", "SIGN_VERIFY"),
    ];

    for (kind, key_spec, key_usage) in rows {
        let metadata = create_key(
            &router,
            &json!({"Description": kind, "KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let key_id = text(kind, &metadata, "KeyId");
        let arn = text(kind, &metadata, "Arn");

        for (operation, request) in [
            (
                "Encapsulate",
                json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
            ),
            (
                "Decapsulate",
                json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&[0u8; 1088]),
                       "EncapsulationAlgorithm": ML_KEM}),
            ),
        ] {
            let case = format!("{operation} with a {kind}");
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidKeyUsageException"),
                "case {case}: expected HTTP 400 InvalidKeyUsageException, got {status} {code}"
            );
            // The refusal names the key, its usage and the operation — this service's one
            // sentence for an incompatible usage.
            assert!(
                message.contains(&arn) && message.contains(key_usage) && message.contains(operation),
                "case {case}: the refusal must name the key, its usage and the operation, \
                 got {message:?}"
            );
        }
    }
}

/// A key that is not enabled runs neither direction: both operations refuse a disabled
/// key `DisabledException`, and both serve it again once it is enabled.
#[tokio::test]
async fn a_disabled_key_runs_neither_direction() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (key_id, _) = encapsulation_key(&router, spec.key_spec).await;
        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let ciphertext = unb64(case, &encapsulated, "SharedSecretCiphertextBlob");

        ok(&router, "DisableKey", &json!({"KeyId": &key_id})).await;

        for (operation, request) in [
            (
                "Encapsulate",
                json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
            ),
            (
                "Decapsulate",
                json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                       "EncapsulationAlgorithm": ML_KEM}),
            ),
        ] {
            let case = format!("{case}: {operation} on a disabled key");
            let (status, code, _) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "DisabledException"),
                "case {case}: expected HTTP 400 DisabledException, got {status} {code}"
            );
        }

        // Enabling the key restores both directions, and the ciphertext taken before it
        // was disabled still recovers its secret.
        ok(&router, "EnableKey", &json!({"KeyId": &key_id})).await;
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &decapsulated, "SharedSecret"),
            unb64(case, &encapsulated, "SharedSecret"),
            "case {case}: the key's material outlives a disable, got {decapsulated}"
        );
    }
}

/// `DryRun` checks the request and then refuses it: both operations answer
/// `DryRunOperationException` with the sentence this service words that refusal in, and
/// nothing is established or consumed — the same request without `DryRun` succeeds
/// afterwards.
#[tokio::test]
async fn a_dry_run_refuses_both_directions_and_establishes_nothing() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let case = spec.key_spec;
        let (key_id, _) = encapsulation_key(&router, spec.key_spec).await;
        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let ciphertext = unb64(case, &encapsulated, "SharedSecretCiphertextBlob");

        for (operation, request) in [
            (
                "Encapsulate",
                json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM, "DryRun": true}),
            ),
            (
                "Decapsulate",
                json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                       "EncapsulationAlgorithm": ML_KEM, "DryRun": true}),
            ),
        ] {
            let case = format!("{case}: {operation} with DryRun");
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "DryRunOperationException"),
                "case {case}: expected HTTP 400 DryRunOperationException, got {status} {code}"
            );
            assert_eq!(
                message, DRY_RUN_MESSAGE,
                "case {case}: expected {DRY_RUN_MESSAGE:?}, got {message:?}"
            );
        }

        // A dry run consumed nothing: the ciphertext still decapsulates, and a plain
        // encapsulation still succeeds.
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &decapsulated, "SharedSecret"),
            unb64(case, &encapsulated, "SharedSecret"),
            "case {case}: a dry run must leave the key as it was, got {decapsulated}"
        );
        let after = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(case, &after, "SharedSecret").len(),
            SHARED_SECRET_LEN,
            "case {case}: the key still encapsulates after a dry run, got {after}"
        );
    }
}

/// How this service words a `DryRun` refusal, which is one sentence for the whole
/// service (`crate::state::DRY_RUN_MESSAGE`).
const DRY_RUN_MESSAGE: &str = "The request would have succeeded, but the DryRun option is set.";

/// Both operations refuse an identifier that names no key of this account with
/// `NotFoundException`, so a caller that names the wrong key learns that rather than a
/// key-usage failure.
#[tokio::test]
async fn both_directions_refuse_an_identifier_that_names_no_key() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";

    for (operation, request) in [
        (
            "Encapsulate",
            json!({"KeyId": absent, "EncapsulationAlgorithm": ML_KEM}),
        ),
        (
            "Decapsulate",
            json!({"KeyId": absent, "SharedSecretCiphertextBlob": b64(&[0u8; 1088]),
                   "EncapsulationAlgorithm": ML_KEM}),
        ),
    ] {
        let case = format!("{operation} naming no key");
        let (status, code, _) = refused(&router, REGION, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {case}: expected HTTP 400 NotFoundException, got {status} {code}"
        );
    }
}

/// A grant can permit either direction of the mechanism: `GrantOperation` declares
/// `Encapsulate` and `Decapsulate` (`smithy_query(query="shape",shape="GrantOperation")`),
/// so a request whose permission comes from a grant token is served exactly when that
/// grant permits the direction it asks for — and a grant permitting a direction can only
/// be created on a key whose `KeyUsage` can run it.
///
/// The rows are every set of the two directions a grant can permit, crossed with both
/// directions a request can ask for; the parameter set is not what this rule is about, so
/// one is enough.
#[tokio::test]
async fn a_grant_permits_only_the_direction_it_names() {
    let router = router();
    let spec = &ML_KEM_SPECS[1];
    let (key_id, _) = encapsulation_key(&router, spec.key_spec).await;
    let grantee = principal("user/Alice");

    let mut grants: Vec<(Vec<&str>, String)> = Vec::new();
    for operations in [
        vec!["Encapsulate"],
        vec!["Decapsulate"],
        vec!["Encapsulate", "Decapsulate"],
    ] {
        let answer = ok(
            &router,
            "CreateGrant",
            &json!({"KeyId": &key_id, "GranteePrincipal": &grantee,
                    "Operations": operations}),
        )
        .await;
        grants.push((operations, text("setup", &answer, "GrantToken")));
    }

    // A ciphertext for the decapsulating rows, established before any token is presented.
    let encapsulated = ok(
        &router,
        "Encapsulate",
        &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
    )
    .await;
    let ciphertext = unb64(spec.key_spec, &encapsulated, "SharedSecretCiphertextBlob");

    for (operations, token) in &grants {
        for operation in ["Encapsulate", "Decapsulate"] {
            let case = format!("a grant permitting {operations:?}: {operation}");
            let mut body = if operation == "Encapsulate" {
                json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM})
            } else {
                json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                       "EncapsulationAlgorithm": ML_KEM})
            };
            body["GrantTokens"] = json!([token]);

            let (status, payload) = call(&router, REGION, operation, &body).await;
            if operations.contains(&operation) {
                assert_eq!(
                    status, 200,
                    "case {case}: the grant permits it, so expected HTTP 200, got {status} \
                     with {payload}"
                );
                assert_eq!(
                    unb64(&case, &payload, "SharedSecret").len(),
                    SHARED_SECRET_LEN,
                    "case {case}: expected a {SHARED_SECRET_LEN}-byte shared secret, got \
                     {payload}"
                );
            } else {
                let code = error_code(&payload);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "AccessDeniedException"),
                    "case {case}: the grant does not permit it, so expected HTTP 400 \
                     AccessDeniedException, got {status} with {payload}"
                );
            }
        }
    }

    // "the operation must be supported on the KMS key"
    // (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`): a key
    // of another usage cannot run either direction, so a grant naming one is refused.
    let other_keys = [
        ("symmetric encryption key", "SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT"),
        ("RSA signing key", "RSA_2048", "SIGN_VERIFY"),
        ("elliptic-curve agreement key", "ECC_NIST_P256", "KEY_AGREEMENT"),
    ];
    for (kind, key_spec, key_usage) in other_keys {
        let metadata = create_key(
            &router,
            &json!({"Description": kind, "KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let other = text(kind, &metadata, "KeyId");
        let arn = text(kind, &metadata, "Arn");
        for operation in ["Encapsulate", "Decapsulate"] {
            let case = format!("a grant permitting {operation} on a {kind}");
            let (status, code, message) = refused(
                &router,
                REGION,
                "CreateGrant",
                &json!({"KeyId": &other, "GranteePrincipal": &grantee,
                        "Operations": [operation]}),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected HTTP 400 ValidationException, got {status} {code}"
            );
            assert!(
                message.contains(operation)
                    && message.contains(&arn)
                    && message.contains(key_usage)
                    && message.contains(ENCAPSULATE_DECAPSULATE),
                "case {case}: the refusal must name the operation, the key, its usage and \
                 the usage the operation requires, got {message:?}"
            );
        }
    }
}

/// `EncapsulationAlgorithm` is a REQUIRED member of both requests whose target declares
/// one value, so a request that names none, or names anything outside that value set, is
/// refused by the boundary with `ValidationException` — in the envelope a constraint
/// refusal is stated in and under the member's own wire path, before any key is read, so
/// nothing is established. (A value INSIDE the set that the named key's own key spec could
/// not run is the handler's `InvalidKeyUsageException`; the set holds one value, which
/// every ML-KEM key spec names, so no request reaches that refusal.)
///
/// A table over the three ML-KEM key specs crossed with both directions and with every
/// algorithm row the member does not admit.
#[tokio::test]
async fn both_directions_require_a_declared_encapsulation_algorithm() {
    let router = router();

    for spec in ML_KEM_SPECS {
        let (key_id, _) = encapsulation_key(&router, spec.key_spec).await;
        let encapsulated = ok(
            &router,
            "Encapsulate",
            &json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        let ciphertext = unb64(spec.key_spec, &encapsulated, "SharedSecretCiphertextBlob");

        for (operation, request) in [
            (
                "Encapsulate",
                json!({"KeyId": &key_id, "EncapsulationAlgorithm": ML_KEM}),
            ),
            (
                "Decapsulate",
                json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                       "EncapsulationAlgorithm": ML_KEM}),
            ),
        ] {
            // The member absent altogether: the boundary never reaches the handler.
            let mut absent = request.clone();
            absent
                .as_object_mut()
                .expect("the request is a JSON object")
                .remove("EncapsulationAlgorithm");
            let case = format!("{}: {operation} naming no algorithm", spec.key_spec);
            let (status, code, message) = refused(&router, REGION, operation, &absent).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected HTTP 400 ValidationException, got {status} {code}"
            );
            let expected = absent_member("encapsulationAlgorithm");
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );

            // A value the member's enum does not declare: refused the same way.
            for algorithm in OTHER_ALGORITHMS {
                let mut named = request.clone();
                named["EncapsulationAlgorithm"] = json!(algorithm);
                let case = format!("{}: {operation} naming {algorithm}", spec.key_spec);
                let (status, code, message) = refused(&router, REGION, operation, &named).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationException"),
                    "case {case}: expected HTTP 400 ValidationException, got {status} {code}"
                );
                let expected = outside_value_set("encapsulationAlgorithm", algorithm, &[ML_KEM]);
                assert_eq!(
                    message, expected,
                    "case {case}: expected {expected:?}, got {message:?}"
                );
            }
        }

        // Every refused call left the key as it was: the ciphertext still decapsulates.
        let decapsulated = ok(
            &router,
            "Decapsulate",
            &json!({"KeyId": &key_id, "SharedSecretCiphertextBlob": b64(&ciphertext),
                    "EncapsulationAlgorithm": ML_KEM}),
        )
        .await;
        assert_eq!(
            unb64(spec.key_spec, &decapsulated, "SharedSecret"),
            unb64(spec.key_spec, &encapsulated, "SharedSecret"),
            "case {}: a refused request must leave the key as it was, got {decapsulated}",
            spec.key_spec
        );
    }
}
