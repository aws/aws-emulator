// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Verify`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: for an asymmetric KMS key in a usable key state whose `KeyUsage`
//! is `SIGN_VERIFY`, `Verify` answers `SignatureValid: true` exactly when the signature
//! the caller presents is a signature of the caller's message under the PUBLIC half of
//! that key's own key pair with the named `SigningAlgorithm` — the message hashed with
//! the hash that algorithm names, or taken as that hash already computed when the caller
//! says it is one, so a message and its digest are the same message — refuses a
//! signature that is not one with `KMSInvalidSignatureException`, refuses a request no
//! such check applies to with the modelled error for why, and uses the key without
//! changing anything else a later caller sees.
//!
//! Five suites assert it over TABLES rather than one call:
//!
//! 1. every asymmetric `KeySpec` that signs, under each algorithm it supports, with each
//!    combination of the `MessageType` the signature was produced under and the one it
//!    is verified under, over messages spanning the modelled message bound;
//! 2. the same signature verified through every form of identifier that names its key;
//! 3. the signatures that must NOT verify: another message, another signature, another
//!    key of the same spec, another algorithm of the same key, another digest;
//! 4. the refusals, over every kind of key, identifier, algorithm, message type, digest
//!    length and grant token a signature check cannot be applied to, and the schemes no
//!    library here verifies with;
//! 5. the state: verifying is repeatable, is recorded as the key's own last usage, and
//!    leaves the key's metadata exactly as it was, while a refused check records nothing.

use std::collections::BTreeSet;

use aws_lc_rs::digest;
use serde_json::{json, Value};

use super::harness::{
    alias_arn, b64, call, create_alias, create_key, describe, error_code, ok, refused, router,
    text, tracked_last_usage, unb64, REGION,
};
use super::signatures::{hash_of, message_of, DIGEST, RAW};

/// One row of the verification table: a key spec, an algorithm that spec supports, the
/// `MessageType` the signature was produced under, the one the verification names, and
/// how long the message is.
struct VerifyCase {
    key_spec: &'static str,
    signing_algorithm: &'static str,
    /// The `MessageType` the `Sign` request carried.
    signed_as: &'static str,
    /// The `MessageType` the `Verify` request carries, or `None` to omit the member —
    /// which is the same as naming `RAW`, since that is what KMS hashes an unqualified
    /// message as. "The message type does not need to be the same as the one used for
    /// signing" (`smithy_query(query="doc",shape="Verify")`), so the rows cross the two.
    verified_as: Option<&'static str>,
    /// How many bytes the message is. `Message` is "len 1..4096"
    /// (`smithy_query(query="operation",operation="Verify")`), so the rows span that
    /// bound: its smallest value, middling ones and its largest.
    message_len: usize,
}

/// Every asymmetric `KeySpec` whose key pair signs, each with signing algorithms from
/// the per-spec lists `DescribeKey` reports under `SigningAlgorithms`
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`), under each
/// pairing of signed and verified message type that algorithm takes.
///
/// The RSA rows cover both schemes of RFC 8017 at all three hashes and all three modulus
/// sizes; the elliptic-curve rows cover each NIST curve and secp256k1; the Edwards25519
/// row covers the one algorithm that signs the message within the scheme itself, which
/// therefore takes only `RAW`. The ML-DSA specs and SM2 are NOT here: no library in this
/// workspace verifies those schemes, which [`unverifiable_schemes_are_not_implemented`]
/// pins instead.
const VERIFY_CASES: &[VerifyCase] = &[
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        signed_as: RAW,
        verified_as: Some(RAW),
        message_len: 1,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        signed_as: RAW,
        verified_as: Some(DIGEST),
        message_len: 4096,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_384",
        signed_as: DIGEST,
        verified_as: Some(RAW),
        message_len: 64,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        signed_as: DIGEST,
        verified_as: Some(DIGEST),
        message_len: 300,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_256",
        signed_as: RAW,
        verified_as: None,
        message_len: 4096,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_384",
        signed_as: DIGEST,
        verified_as: Some(RAW),
        message_len: 1,
    },
    VerifyCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_512",
        signed_as: RAW,
        verified_as: Some(DIGEST),
        message_len: 32,
    },
    VerifyCase {
        key_spec: "RSA_3072",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_384",
        signed_as: DIGEST,
        verified_as: Some(DIGEST),
        message_len: 17,
    },
    VerifyCase {
        key_spec: "RSA_3072",
        signing_algorithm: "RSASSA_PSS_SHA_256",
        signed_as: RAW,
        verified_as: Some(RAW),
        message_len: 2048,
    },
    VerifyCase {
        key_spec: "RSA_4096",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        signed_as: RAW,
        verified_as: Some(DIGEST),
        message_len: 128,
    },
    VerifyCase {
        key_spec: "RSA_4096",
        signing_algorithm: "RSASSA_PSS_SHA_384",
        signed_as: DIGEST,
        verified_as: None,
        message_len: 4096,
    },
    VerifyCase {
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        signed_as: RAW,
        verified_as: Some(RAW),
        message_len: 1,
    },
    VerifyCase {
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        signed_as: DIGEST,
        verified_as: Some(DIGEST),
        message_len: 4096,
    },
    VerifyCase {
        key_spec: "ECC_NIST_P384",
        signing_algorithm: "ECDSA_SHA_384",
        signed_as: DIGEST,
        verified_as: Some(RAW),
        message_len: 300,
    },
    VerifyCase {
        key_spec: "ECC_NIST_P521",
        signing_algorithm: "ECDSA_SHA_512",
        signed_as: RAW,
        verified_as: Some(DIGEST),
        message_len: 2,
    },
    VerifyCase {
        key_spec: "ECC_SECG_P256K1",
        signing_algorithm: "ECDSA_SHA_256",
        signed_as: RAW,
        verified_as: None,
        message_len: 64,
    },
    VerifyCase {
        key_spec: "ECC_SECG_P256K1",
        signing_algorithm: "ECDSA_SHA_256",
        signed_as: DIGEST,
        verified_as: Some(DIGEST),
        message_len: 1024,
    },
    VerifyCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        signed_as: RAW,
        verified_as: Some(RAW),
        message_len: 4096,
    },
    VerifyCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        signed_as: RAW,
        verified_as: None,
        message_len: 1,
    },
];

/// What the `Message` member carries under `message_type`: the message itself under
/// `RAW`, and the hash the algorithm names under `DIGEST`, because "a message and its
/// hash digest are considered to be the same message"
/// (`smithy_query(query="doc",shape="VerifyRequest",member="Message")`).
fn message_member(
    case: &str,
    signing_algorithm: &str,
    message_type: Option<&str>,
    message: &[u8],
) -> Vec<u8> {
    if message_type == Some(DIGEST) {
        return digest::digest(hash_of(case, signing_algorithm), message)
            .as_ref()
            .to_vec();
    }
    message.to_vec()
}

/// The `Verify` request a row makes.
fn verify_request(
    case: &str,
    key_id: &str,
    signing_algorithm: &str,
    message_type: Option<&str>,
    message: &[u8],
    signature: &[u8],
) -> Value {
    let mut request = json!({
        "KeyId": key_id,
        "Message": b64(&message_member(case, signing_algorithm, message_type, message)),
        "Signature": b64(signature),
        "SigningAlgorithm": signing_algorithm,
    });
    if let Some(message_type) = message_type {
        request["MessageType"] = json!(message_type);
    }
    request
}

/// Sign `message` under the key `key_id` names with `signing_algorithm`, presenting the
/// message as `message_type` says, and answer the signature bytes.
async fn signature(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Vec<u8> {
    let answered = ok(
        router,
        "Sign",
        &json!({
            "KeyId": key_id,
            "Message": b64(&message_member(case, signing_algorithm, Some(message_type), message)),
            "MessageType": message_type,
            "SigningAlgorithm": signing_algorithm,
        }),
    )
    .await;
    unb64(case, &answered, "Signature")
}

/// Assert a `Verify` answer reports a valid signature of the key `key_arn` names under
/// `signing_algorithm`.
fn assert_valid(case: &str, answered: &Value, key_arn: &str, signing_algorithm: &str) {
    let answered_key = text(case, answered, "KeyId");
    assert_eq!(
        answered_key, key_arn,
        "case {case}: expected the key ARN {key_arn}, got {answered_key}"
    );
    assert_eq!(
        answered["SignatureValid"],
        json!(true),
        "case {case}: expected SignatureValid true, got {answered}"
    );
    assert_eq!(
        text(case, answered, "SigningAlgorithm"),
        signing_algorithm,
        "case {case}: expected the signing algorithm the request named, got {answered}"
    );
}

/// The rule: a signature of the message under a key's own key pair verifies under that
/// key, under every algorithm its spec supports and whichever message type each side
/// presented the message as.
#[tokio::test]
async fn verifies_every_signature_each_signing_key_spec_produces() {
    let router = router();

    // One key per spec, created once: every row of that spec verifies against the same
    // persisted key pair, which is what makes the rows about the algorithm and the
    // message type rather than about the key.
    let mut keys: Vec<(&str, String, String)> = Vec::new();
    for case in VERIFY_CASES {
        if keys.iter().any(|(key_spec, ..)| *key_spec == case.key_spec) {
            continue;
        }
        let metadata = create_key(
            &router,
            &json!({
                "Description": case.key_spec,
                "KeySpec": case.key_spec,
                "KeyUsage": "SIGN_VERIFY"
            }),
        )
        .await;
        let key_id = text(case.key_spec, &metadata, "KeyId");
        let key_arn = text(case.key_spec, &metadata, "Arn");
        keys.push((case.key_spec, key_id, key_arn));
    }

    for case in VERIFY_CASES {
        let label = format!(
            "{} {} signed as {} verified as {} over {} bytes",
            case.key_spec,
            case.signing_algorithm,
            case.signed_as,
            case.verified_as.unwrap_or("(no MessageType)"),
            case.message_len
        );
        let (_, key_id, key_arn) = keys
            .iter()
            .find(|(key_spec, ..)| *key_spec == case.key_spec)
            .unwrap_or_else(|| panic!("case {label}: the key of this spec was created"));

        let message = message_of(case.message_len);
        let signed = signature(
            &router,
            &label,
            key_id,
            case.signing_algorithm,
            case.signed_as,
            &message,
        )
        .await;
        let answered = ok(
            &router,
            "Verify",
            &verify_request(
                &label,
                key_id,
                case.signing_algorithm,
                case.verified_as,
                &message,
                &signed,
            ),
        )
        .await;
        assert_valid(&label, &answered, key_arn, case.signing_algorithm);
    }
}

/// The rule, continued: a signature is verified under the KEY, so every identifier form
/// that names the same key verifies the same signature and answers that key's ARN.
#[tokio::test]
async fn verifies_under_every_identifier_form_of_the_key() {
    let router = router();
    let case = "identifier forms";
    let algorithm = "RSASSA_PSS_SHA_256";

    let metadata = create_key(
        &router,
        &json!({"Description": case, "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let key_id = text(case, &metadata, "KeyId");
    let key_arn = text(case, &metadata, "Arn");
    let alias = "alias/verification-identifier";
    create_alias(&router, case, alias, &key_id).await;

    let message = message_of(96);
    let signed = signature(&router, case, &key_id, algorithm, RAW, &message).await;

    // "To specify a KMS key, use its key ID, key ARN, alias name, or alias ARN"
    // (`smithy_query(query="doc",shape="VerifyRequest",member="KeyId")`).
    for identifier in [
        key_id.clone(),
        key_arn.clone(),
        alias.to_string(),
        alias_arn(REGION, alias),
    ] {
        let label = format!("{case}: {identifier}");
        let answered = ok(
            &router,
            "Verify",
            &verify_request(&label, &identifier, algorithm, Some(RAW), &message, &signed),
        )
        .await;
        assert_valid(&label, &answered, &key_arn, algorithm);
    }
}

/// How a row breaks the correspondence between the signature and what it is checked
/// against — each a way "it cannot confirm that signature was produced by signing the
/// specified message with the specified KMS key and signing algorithm"
/// (`smithy_query(query="doc",shape="KMSInvalidSignatureException")`).
enum Tamper {
    /// The verified message differs from the signed one in its last byte.
    Message,
    /// The signature differs from the one that was produced, in its last byte.
    Signature,
    /// The signature is a byte short of the one that was produced.
    Truncated,
    /// The signature was produced by ANOTHER key of the same spec.
    OtherKey,
    /// The signature is checked with another algorithm the same key supports.
    OtherAlgorithm(&'static str),
    /// The signature is checked against the digest of ANOTHER message, of the right
    /// length for the algorithm's hash.
    OtherDigest,
}

/// One row of the invalid-signature table.
struct InvalidCase {
    case: &'static str,
    key_spec: &'static str,
    signing_algorithm: &'static str,
    tamper: Tamper,
}

/// Every way a signature can fail to be a signature of the verified message under the
/// named key and algorithm, spread over the RSA schemes, the NIST and secp256k1 curves
/// and Edwards25519.
const INVALID_CASES: &[InvalidCase] = &[
    InvalidCase {
        case: "an RSA signature of another message",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        tamper: Tamper::Message,
    },
    InvalidCase {
        case: "an RSA signature with a byte changed",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_256",
        tamper: Tamper::Signature,
    },
    InvalidCase {
        case: "an RSA signature a byte short",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        tamper: Tamper::Truncated,
    },
    InvalidCase {
        case: "an RSA signature of another key of the same spec",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_384",
        tamper: Tamper::OtherKey,
    },
    InvalidCase {
        case: "an RSA PKCS1 signature checked as PSS",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        tamper: Tamper::OtherAlgorithm("RSASSA_PSS_SHA_256"),
    },
    InvalidCase {
        case: "an RSA signature checked under another hash",
        key_spec: "RSA_3072",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        tamper: Tamper::OtherAlgorithm("RSASSA_PKCS1_V1_5_SHA_384"),
    },
    InvalidCase {
        case: "an RSA signature checked against another digest",
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_512",
        tamper: Tamper::OtherDigest,
    },
    InvalidCase {
        case: "a P-256 signature of another message",
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        tamper: Tamper::Message,
    },
    InvalidCase {
        case: "a P-256 signature of another key of the same spec",
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        tamper: Tamper::OtherKey,
    },
    InvalidCase {
        case: "a P-384 signature checked against another digest",
        key_spec: "ECC_NIST_P384",
        signing_algorithm: "ECDSA_SHA_384",
        tamper: Tamper::OtherDigest,
    },
    InvalidCase {
        case: "a P-521 signature with a byte changed",
        key_spec: "ECC_NIST_P521",
        signing_algorithm: "ECDSA_SHA_512",
        tamper: Tamper::Signature,
    },
    InvalidCase {
        case: "a secp256k1 signature of another key of the same spec",
        key_spec: "ECC_SECG_P256K1",
        signing_algorithm: "ECDSA_SHA_256",
        tamper: Tamper::OtherKey,
    },
    InvalidCase {
        case: "an Ed25519 signature of another message",
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        tamper: Tamper::Message,
    },
    InvalidCase {
        case: "an Ed25519 signature with a byte changed",
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        tamper: Tamper::Signature,
    },
];

/// The rule, continued: a signature that is not a signature of the verified message
/// under the named key and algorithm is refused, and the answer carries no verification
/// at all.
#[tokio::test]
async fn refuses_a_signature_that_does_not_verify() {
    let router = router();

    // Every row names a distinct reason, so no reason is asserted twice.
    let reasons: BTreeSet<&str> = INVALID_CASES.iter().map(|case| case.case).collect();
    assert_eq!(
        reasons.len(),
        INVALID_CASES.len(),
        "case table: every invalid-signature row must name its own reason"
    );

    for case in INVALID_CASES {
        let metadata = create_key(
            &router,
            &json!({
                "Description": case.case,
                "KeySpec": case.key_spec,
                "KeyUsage": "SIGN_VERIFY"
            }),
        )
        .await;
        let key_id = text(case.case, &metadata, "KeyId");

        let message = message_of(192);
        // The key that SIGNS: the row's own key, or a second key of the same spec for a
        // row whose signature comes from elsewhere.
        let signer = match case.tamper {
            Tamper::OtherKey => {
                let other = create_key(
                    &router,
                    &json!({
                        "Description": format!("{} (the other key)", case.case),
                        "KeySpec": case.key_spec,
                        "KeyUsage": "SIGN_VERIFY"
                    }),
                )
                .await;
                text(case.case, &other, "KeyId")
            }
            _ => key_id.clone(),
        };
        let signed = signature(
            &router,
            case.case,
            &signer,
            case.signing_algorithm,
            RAW,
            &message,
        )
        .await;

        // What the verification is given, which differs from what was signed in exactly
        // the one way this row is about.
        let mut verified_message = message.clone();
        let mut verified_signature = signed.clone();
        let mut verified_algorithm = case.signing_algorithm;
        let mut verified_as = Some(RAW);
        match case.tamper {
            Tamper::Message => {
                let last = verified_message.len() - 1;
                verified_message[last] ^= 0x01;
            }
            Tamper::Signature => {
                let last = verified_signature.len() - 1;
                verified_signature[last] ^= 0x01;
            }
            Tamper::Truncated => {
                verified_signature.pop();
            }
            Tamper::OtherAlgorithm(algorithm) => verified_algorithm = algorithm,
            Tamper::OtherDigest => {
                verified_message = message_of(191);
                verified_as = Some(DIGEST);
            }
            Tamper::OtherKey => {}
        }

        let (status, payload) = call(
            &router,
            REGION,
            "Verify",
            &verify_request(
                case.case,
                &key_id,
                verified_algorithm,
                verified_as,
                &verified_message,
                &verified_signature,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400, got {status} with {payload}",
            case.case
        );
        assert_eq!(
            error_code(&payload),
            "KMSInvalidSignatureException",
            "case {}: expected KMSInvalidSignatureException, got {payload}",
            case.case
        );
        assert!(
            payload["SignatureValid"].is_null(),
            "case {}: a refused check reports no verification, got {payload}",
            case.case
        );
    }
}

/// One refusal row: what makes a signature check impossible, and what KMS answers.
struct RefusalCase {
    case: &'static str,
    /// The `KeySpec` and `KeyUsage` of the row's key, or `None` for a row whose `KeyId`
    /// names no key at all.
    key: Option<(&'static str, &'static str)>,
    /// Whether the row signs its message with its key BEFORE anything else, so the
    /// refusal is reached with a signature that would otherwise verify.
    signs_first: bool,
    /// An operation called on the key after that, which is what puts it in the key state
    /// the row is about.
    before: Option<&'static str>,
    /// The `KeyId` the request names, when it is not the row's own key.
    names: Option<&'static str>,
    signing_algorithm: &'static str,
    message_type: Option<&'static str>,
    /// How many bytes the `Message` carries, as it is sent — for a `DIGEST` row, what
    /// makes it the right or the wrong length for the algorithm's hash.
    message_len: usize,
    /// A grant token the request presents.
    grant_token: Option<&'static str>,
    dry_run: bool,
    /// The wire error code (`__type`) of the answer. This service answers every client
    /// error with HTTP 400 and carries the shape in `__type`.
    expected: &'static str,
}

/// Every reason a `Verify` request cannot be answered with a verification, each with the
/// modelled error for why.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "an identifier that names no key",
        key: None,
        signs_first: false,
        before: None,
        names: Some("2b8c0ea8-8a2a-4f5b-9f0c-2b0f5a7c1d33"),
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "NotFoundException",
    },
    RefusalCase {
        case: "an identifier that is not a well formed ARN",
        key: None,
        signs_first: false,
        before: None,
        names: Some("arn:aws:kms:::key/"),
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "NotFoundException",
    },
    RefusalCase {
        case: "a symmetric encryption key",
        key: Some(("SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "an asymmetric key whose usage is encryption",
        key: Some(("RSA_2048", "ENCRYPT_DECRYPT")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "an HMAC key",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "an algorithm the key spec does not support",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_384",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "a disabled key",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        signs_first: true,
        before: Some("DisableKey"),
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "DisabledException",
    },
    RefusalCase {
        case: "a key scheduled for deletion",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        signs_first: true,
        before: Some("ScheduleKeyDeletion"),
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "KMSInvalidStateException",
    },
    RefusalCase {
        case: "a grant token this service did not mint",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        signs_first: true,
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: Some("not-a-grant-token"),
        dry_run: false,
        expected: "InvalidGrantTokenException",
    },
    RefusalCase {
        case: "a dry run",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: true,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: true,
        expected: "DryRunOperationException",
    },
    // A dry run "checks if your request will succeed"
    // (`smithy_query(query="doc",shape="VerifyRequest",member="DryRun")`) and then always
    // fails, so it is answered on the request as checked above and never by verifying:
    // the row below presents bytes that are no signature at all and is answered the same
    // way as the row above, which presents one that would verify.
    RefusalCase {
        case: "a dry run presenting a signature that would not verify",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        grant_token: None,
        dry_run: true,
        expected: "DryRunOperationException",
    },
    RefusalCase {
        case: "a digest one byte short of the hash",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(DIGEST),
        message_len: 31,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
    RefusalCase {
        case: "a digest one byte longer than the hash",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(DIGEST),
        message_len: 33,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
    RefusalCase {
        case: "a digest of another hash",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        message_type: Some(DIGEST),
        message_len: 32,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
    RefusalCase {
        case: "pure Ed25519 over a digest",
        key: Some(("ECC_NIST_EDWARDS25519", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ED25519_SHA_512",
        message_type: Some(DIGEST),
        message_len: 64,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
    RefusalCase {
        case: "prehashed Ed25519 over a whole message",
        key: Some(("ECC_NIST_EDWARDS25519", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ED25519_PH_SHA_512",
        message_type: Some(RAW),
        message_len: 64,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
    RefusalCase {
        case: "an external mu for an elliptic-curve algorithm",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        signs_first: false,
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("EXTERNAL_MU"),
        message_len: 64,
        grant_token: None,
        dry_run: false,
        expected: "ValidationException",
    },
];

/// The rule, continued: a request no signature check applies to is refused with the
/// modelled error for why, whether or not the signature it carries would have verified.
#[tokio::test]
async fn refuses_every_request_that_cannot_be_checked() {
    let router = router();

    // Every row names a distinct reason, so no reason is asserted twice.
    let reasons: BTreeSet<&str> = REFUSAL_CASES.iter().map(|case| case.case).collect();
    assert_eq!(
        reasons.len(),
        REFUSAL_CASES.len(),
        "case table: every refusal row must name its own reason"
    );

    for case in REFUSAL_CASES {
        let message = message_of(case.message_len);
        let mut key_id = String::new();
        // A signature that would verify, for a row that gets that far, and otherwise
        // bytes of a signature's length: every row below refuses BEFORE the check.
        let mut signed = message_of(256);
        if let Some((key_spec, key_usage)) = case.key {
            let metadata = create_key(
                &router,
                &json!({
                    "Description": case.case,
                    "KeySpec": key_spec,
                    "KeyUsage": key_usage
                }),
            )
            .await;
            key_id = text(case.case, &metadata, "KeyId");
            if case.signs_first {
                signed = signature(
                    &router,
                    case.case,
                    &key_id,
                    case.signing_algorithm,
                    RAW,
                    &message,
                )
                .await;
            }
            if let Some(before) = case.before {
                ok(&router, before, &json!({"KeyId": key_id})).await;
            }
        }
        let named = case.names.unwrap_or(&key_id);

        let mut request = json!({
            "KeyId": named,
            // Sent as it stands: a row about a digest length is about the bytes the
            // request carries, not about a hash of them.
            "Message": b64(&message),
            "Signature": b64(&signed),
            "SigningAlgorithm": case.signing_algorithm,
        });
        if let Some(message_type) = case.message_type {
            request["MessageType"] = json!(message_type);
        }
        if let Some(grant_token) = case.grant_token {
            request["GrantTokens"] = json!([grant_token]);
        }
        if case.dry_run {
            request["DryRun"] = json!(true);
        }

        let (status, payload) = call(&router, REGION, "Verify", &request).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400, got {status} with {payload}",
            case.case
        );
        assert_eq!(
            error_code(&payload),
            case.expected,
            "case {}: expected {}, got {payload}",
            case.case,
            case.expected
        );
        assert!(
            payload["SignatureValid"].is_null(),
            "case {}: a refused request reports no verification, got {payload}",
            case.case
        );
    }
}

/// The signature schemes a KMS key can be created for and no library in this workspace
/// verifies a signature with: ML-DSA, whose key pair is not generated here at all, and
/// the SM2DSA of GM/T 0003-2012. Such a request is
/// answered as not implemented rather than with a verification nothing performed.
const UNVERIFIABLE_CASES: &[(&str, &str)] = &[
    ("ML_DSA_44", "ML_DSA_SHAKE_256"),
    ("ML_DSA_65", "ML_DSA_SHAKE_256"),
    ("ML_DSA_87", "ML_DSA_SHAKE_256"),
    ("SM2", "SM2DSA"),
];

/// A scheme nothing here verifies with answers the scaffold's 501, never a fabricated
/// verification and never a modelled refusal that would claim the signature was wrong.
#[tokio::test]
async fn unverifiable_schemes_are_not_implemented() {
    let router = router();

    for (key_spec, signing_algorithm) in UNVERIFIABLE_CASES {
        let label = format!("{key_spec} {signing_algorithm}");
        let metadata = create_key(
            &router,
            &json!({
                "Description": label,
                "KeySpec": key_spec,
                "KeyUsage": "SIGN_VERIFY"
            }),
        )
        .await;
        let key_id = text(&label, &metadata, "KeyId");
        // ML-DSA and SM2DSA verify over the message itself, so each row sends the
        // message type its algorithm takes and the 501 is about the scheme and not about
        // the request. Both Edwards25519 schemes are verified here now, so neither is a
        // row.
        let message_type = RAW;
        let message = message_of(32);

        let (status, payload) = call(
            &router,
            REGION,
            "Verify",
            &json!({
                "KeyId": key_id,
                "Message": b64(&message),
                "MessageType": message_type,
                "Signature": b64(&message_of(64)),
                "SigningAlgorithm": signing_algorithm
            }),
        )
        .await;
        assert_eq!(
            status, 501,
            "case {label}: expected HTTP 501, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "NotImplemented",
            "case {label}: expected NotImplemented, got {payload}"
        );
    }
}

/// The rule, continued: a verification READS the key's persisted key pair, so the same
/// check answers the same thing however often it is asked and leaves the key's metadata
/// as it was; the use itself is recorded as the key's last usage, and a check that is
/// refused records nothing.
#[tokio::test]
async fn verifying_uses_the_key_without_changing_it() {
    let router = router();
    let case = "verification is a use";
    let algorithm = "ECDSA_SHA_256";

    let metadata = create_key(
        &router,
        &json!({"Description": case, "KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let key_id = text(case, &metadata, "KeyId");
    let key_arn = text(case, &metadata, "Arn");

    let message = message_of(128);
    let signed = signature(&router, case, &key_id, algorithm, RAW, &message).await;
    let before = describe(&router, &key_id).await;

    let request = verify_request(case, &key_id, algorithm, Some(RAW), &message, &signed);
    for attempt in 0..2u8 {
        let label = format!("{case}: attempt {attempt}");
        let answered = ok(&router, "Verify", &request).await;
        assert_valid(&label, &answered, &key_arn, algorithm);
    }

    let after = describe(&router, &key_id).await;
    assert_eq!(
        after, before,
        "case {case}: verifying must not change the key's metadata"
    );

    // The use is recorded, and KMS's usage tracking produces that record a little after
    // the call: once it has, the record names this operation.
    let usage = tracked_last_usage(&router, case, &key_id, "Verify", &Value::Null).await;
    assert_eq!(
        usage["KeyLastUsage"]["Operation"]
            .as_str()
            .unwrap_or_default(),
        "Verify",
        "case {case}: the key's last usage must name Verify, got {usage}"
    );

    // A check that fails is not a use of the key: the record stays the one the last
    // successful call wrote.
    let mut tampered = message.clone();
    tampered[0] ^= 0x01;
    let (status, code, message_text) = refused(
        &router,
        REGION,
        "Verify",
        &verify_request(case, &key_id, algorithm, Some(RAW), &tampered, &signed),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "KMSInvalidSignatureException"),
        "case {case}: expected a 400 KMSInvalidSignatureException, got {status} {code} \
         {message_text}"
    );
    let usage_after = ok(&router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await;
    assert_eq!(
        usage_after, usage,
        "case {case}: a refused check must leave the last-usage record as it was, got \
         {usage_after}"
    );
}
