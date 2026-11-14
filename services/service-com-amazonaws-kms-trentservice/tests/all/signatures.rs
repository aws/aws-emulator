// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Sign`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: for an asymmetric KMS key in a usable key state whose
//! `KeyUsage` is `SIGN_VERIFY`, `Sign` answers a real signature of the caller's message
//! under the PRIVATE half of that key's own key pair — the message hashed with the hash
//! the requested `SigningAlgorithm` names, or signed as it stands when the caller says
//! the message already IS that hash — encoded as the algorithm is defined to encode a
//! signature, beside the key ARN and the algorithm it was produced with; a request no
//! such signature can be produced for is refused with the modelled error for why; and
//! signing uses the key without changing anything else a later caller sees.
//!
//! Three suites assert it over TABLES rather than one call:
//!
//! 1. every asymmetric `KeySpec` that signs, under each signing algorithm that spec
//!    supports and each message type that algorithm takes, over messages spanning the
//!    modelled message bound — each answer checked by VERIFYING it against the public
//!    half `GetPublicKey` publishes for the same key;
//! 2. the refusals, over every kind of key, identifier, algorithm, message type and
//!    digest length that cannot yield a signature;
//! 3. the state: the same key signs the same message identically through a
//!    deterministic scheme, two keys of one spec sign it differently, and a signature
//!    leaves the key's own metadata exactly as it was.

use std::collections::BTreeSet;

use aws_lc_rs::digest;
use aws_lc_rs::signature::{ParsedPublicKey, VerificationAlgorithm};
use serde_json::{json, Value};

use super::harness::{
    b64, call, contains, create_key, describe, error_code, ok, router, text, tracked_last_usage,
    unb64, REGION,
};

/// The longest signature the model lets this operation answer:
/// `Signature` is a `CiphertextType` of "len 1..6144"
/// (`smithy_query(query="operation",operation="Sign")`).
const MAX_SIGNATURE_LEN: usize = 6144;

/// One row of the signing table: a key spec, an algorithm that spec supports, the
/// `MessageType` the request names and how long the message is.
struct SignCase {
    key_spec: &'static str,
    signing_algorithm: &'static str,
    /// The `MessageType` the request carries, or `None` to omit the member — which is
    /// the same as naming `RAW`, since that is what KMS hashes an unqualified message
    /// as ("use RAW for unhashed messages",
    /// `smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
    message_type: Option<&'static str>,
    /// How many bytes the signed message is. `Message` is "len 1..4096"
    /// (`smithy_query(query="operation",operation="Sign")`), so the rows span that
    /// bound: its smallest value, a middling one and its largest.
    message_len: usize,
}

/// Every asymmetric `KeySpec` whose key pair signs, each with the signing algorithms
/// its "Key spec reference" gives it
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`, and the
/// per-spec lists `DescribeKey` reports under `SigningAlgorithms`), under each message
/// type that algorithm takes.
///
/// The RSA rows cover both schemes of RFC 8017 at all three hashes and all three
/// modulus sizes; the elliptic-curve rows cover each NIST curve and secp256k1; the
/// Edwards25519 row covers the one algorithm that signs the message within the scheme
/// itself. The ML-DSA specs and SM2 are NOT here: no library in this workspace signs
/// with those schemes, which [`unsupported_signature_schemes_are_not_implemented`]
/// pins instead.
const SIGN_CASES: &[SignCase] = &[
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        message_type: Some("RAW"),
        message_len: 1,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        message_type: Some("DIGEST"),
        message_len: 4096,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_384",
        message_type: None,
        message_len: 64,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        message_type: Some("DIGEST"),
        message_len: 300,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some("RAW"),
        message_len: 4096,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_384",
        message_type: Some("DIGEST"),
        message_len: 1,
    },
    SignCase {
        key_spec: "RSA_2048",
        signing_algorithm: "RSASSA_PSS_SHA_512",
        message_type: None,
        message_len: 32,
    },
    SignCase {
        key_spec: "RSA_3072",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_384",
        message_type: Some("DIGEST"),
        message_len: 17,
    },
    SignCase {
        key_spec: "RSA_3072",
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some("RAW"),
        message_len: 2048,
    },
    SignCase {
        key_spec: "RSA_4096",
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        message_type: Some("RAW"),
        message_len: 1,
    },
    SignCase {
        key_spec: "RSA_4096",
        signing_algorithm: "RSASSA_PSS_SHA_512",
        message_type: Some("DIGEST"),
        message_len: 4096,
    },
    SignCase {
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("RAW"),
        message_len: 11,
    },
    SignCase {
        key_spec: "ECC_NIST_P256",
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("DIGEST"),
        message_len: 4096,
    },
    SignCase {
        key_spec: "ECC_NIST_P384",
        signing_algorithm: "ECDSA_SHA_384",
        message_type: None,
        message_len: 128,
    },
    SignCase {
        key_spec: "ECC_NIST_P384",
        signing_algorithm: "ECDSA_SHA_384",
        message_type: Some("DIGEST"),
        message_len: 1,
    },
    SignCase {
        key_spec: "ECC_NIST_P521",
        signing_algorithm: "ECDSA_SHA_512",
        message_type: Some("RAW"),
        message_len: 1000,
    },
    SignCase {
        key_spec: "ECC_NIST_P521",
        signing_algorithm: "ECDSA_SHA_512",
        message_type: Some("DIGEST"),
        message_len: 7,
    },
    SignCase {
        key_spec: "ECC_SECG_P256K1",
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("DIGEST"),
        message_len: 64,
    },
    SignCase {
        key_spec: "ECC_SECG_P256K1",
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("RAW"),
        message_len: 4096,
    },
    SignCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        message_type: Some("RAW"),
        message_len: 1,
    },
    SignCase {
        key_spec: "ECC_NIST_EDWARDS25519",
        signing_algorithm: "ED25519_SHA_512",
        message_type: None,
        message_len: 4096,
    },
];

/// The `MessageType` of a message signed as given, and the one of a message that is
/// already the hash the algorithm would have computed
/// (`smithy_query(query="shape",shape="MessageType")`).
pub(crate) const RAW: &str = "RAW";
/// See [`RAW`].
pub(crate) const DIGEST: &str = "DIGEST";

/// The hash each `SigningAlgorithmSpec` hashes its message with:
/// "signing algorithms that end in SHA_256 use the SHA_256 hashing algorithm", and the
/// same sentence for SHA_384 and SHA_512
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`). Written out
/// per algorithm so the test's idea of the hash does not come from the service's.
pub(crate) fn hash_of(case: &str, signing_algorithm: &str) -> &'static digest::Algorithm {
    match signing_algorithm {
        "RSASSA_PKCS1_V1_5_SHA_256" | "RSASSA_PSS_SHA_256" | "ECDSA_SHA_256" => &digest::SHA256,
        "RSASSA_PKCS1_V1_5_SHA_384" | "RSASSA_PSS_SHA_384" | "ECDSA_SHA_384" => &digest::SHA384,
        "RSASSA_PKCS1_V1_5_SHA_512"
        | "RSASSA_PSS_SHA_512"
        | "ECDSA_SHA_512"
        | "ED25519_SHA_512"
        | "ED25519_PH_SHA_512" => &digest::SHA512,
        other => panic!("case {case}: this table gives {other} no hash"),
    }
}

/// How a signature of `signing_algorithm` under a key of `key_spec` is VERIFIED: the
/// same scheme, hash and curve, as the verifying side of a key pair names them. A
/// signature that verifies under the published public key with these parameters is a
/// signature of the message under the key's private half, which is the whole claim the
/// suites make.
fn verification_algorithm(
    case: &str,
    key_spec: &str,
    signing_algorithm: &str,
) -> &'static dyn VerificationAlgorithm {
    use aws_lc_rs::signature::{
        ECDSA_P256K1_SHA256_ASN1, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
        ECDSA_P521_SHA512_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
        RSA_PKCS1_2048_8192_SHA512, RSA_PSS_2048_8192_SHA256, RSA_PSS_2048_8192_SHA384,
        RSA_PSS_2048_8192_SHA512,
    };

    match (key_spec, signing_algorithm) {
        (_, "RSASSA_PKCS1_V1_5_SHA_256") => &RSA_PKCS1_2048_8192_SHA256,
        (_, "RSASSA_PKCS1_V1_5_SHA_384") => &RSA_PKCS1_2048_8192_SHA384,
        (_, "RSASSA_PKCS1_V1_5_SHA_512") => &RSA_PKCS1_2048_8192_SHA512,
        (_, "RSASSA_PSS_SHA_256") => &RSA_PSS_2048_8192_SHA256,
        (_, "RSASSA_PSS_SHA_384") => &RSA_PSS_2048_8192_SHA384,
        (_, "RSASSA_PSS_SHA_512") => &RSA_PSS_2048_8192_SHA512,
        ("ECC_NIST_P256", "ECDSA_SHA_256") => &ECDSA_P256_SHA256_ASN1,
        ("ECC_NIST_P384", "ECDSA_SHA_384") => &ECDSA_P384_SHA384_ASN1,
        ("ECC_NIST_P521", "ECDSA_SHA_512") => &ECDSA_P521_SHA512_ASN1,
        ("ECC_SECG_P256K1", "ECDSA_SHA_256") => &ECDSA_P256K1_SHA256_ASN1,
        ("ECC_NIST_EDWARDS25519", "ED25519_SHA_512") => &ED25519,
        other => panic!("case {case}: this table verifies nothing for {other:?}"),
    }
}

/// The message a row signs: `len` bytes that are neither constant nor the message of
/// any other length, so no signature can be right by accident of its input.
pub(crate) fn message_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + len) % 251).unwrap_or_default())
        .collect()
}

/// The `Sign` request a row makes: the message itself under `RAW`, and the hash of
/// that message under `DIGEST`.
fn sign_request(
    key_id: &str,
    signing_algorithm: &str,
    message_type: Option<&str>,
    message: &[u8],
    hashed: &digest::Digest,
) -> Value {
    let signed: &[u8] = if message_type == Some(DIGEST) {
        hashed.as_ref()
    } else {
        message
    };
    let mut request = json!({
        "KeyId": key_id,
        "Message": b64(signed),
        "SigningAlgorithm": signing_algorithm,
    });
    if let Some(message_type) = message_type {
        request["MessageType"] = json!(message_type);
    }
    request
}

/// The rule: each asymmetric signing key spec signs under every algorithm it supports,
/// and the signature verifies against the public half of that same key.
#[tokio::test]
async fn signs_every_algorithm_each_signing_key_spec_supports() {
    let router = router();

    // One key per spec, created once: every row of that spec signs with the same
    // persisted key pair, which is what makes the rows about the algorithm rather than
    // about the key.
    let mut keys: Vec<(&str, String, String, Vec<u8>)> = Vec::new();
    for case in SIGN_CASES {
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
        let published = ok(&router, "GetPublicKey", &json!({"KeyId": key_id})).await;
        let public_key = unb64(case.key_spec, &published, "PublicKey");
        keys.push((case.key_spec, key_id, key_arn, public_key));
    }

    for case in SIGN_CASES {
        let label = format!(
            "{} {} {} {} bytes",
            case.key_spec,
            case.signing_algorithm,
            case.message_type.unwrap_or("(no MessageType)"),
            case.message_len
        );
        let (_, key_id, key_arn, public_key) = keys
            .iter()
            .find(|(key_spec, ..)| *key_spec == case.key_spec)
            .unwrap_or_else(|| panic!("case {label}: the key of this spec was created"));

        let message = message_of(case.message_len);
        let hash = hash_of(&label, case.signing_algorithm);
        let hashed = digest::digest(hash, &message);
        let request = sign_request(
            key_id,
            case.signing_algorithm,
            case.message_type,
            &message,
            &hashed,
        );
        let answered = ok(&router, "Sign", &request).await;

        // The answer names the key by its key ARN — "the Amazon Resource Name (key ARN)
        // of the asymmetric KMS key that was used to sign the message" — and the
        // algorithm that produced the signature.
        let answered_key = text(&label, &answered, "KeyId");
        assert_eq!(
            &answered_key, key_arn,
            "case {label}: expected the key ARN {key_arn}, got {answered_key}"
        );
        assert_eq!(
            text(&label, &answered, "SigningAlgorithm"),
            case.signing_algorithm,
            "case {label}: expected the signing algorithm the request named, got {answered}"
        );

        let signature = unb64(&label, &answered, "Signature");
        assert!(
            !signature.is_empty() && signature.len() <= MAX_SIGNATURE_LEN,
            "case {label}: expected 1..={MAX_SIGNATURE_LEN} signature bytes, got {}",
            signature.len()
        );
        // A signature is not a copy of what was signed: a message long enough not to
        // occur in it by chance must not appear in it at all.
        if case.message_len >= 16 {
            assert!(
                !contains(&signature, &message),
                "case {label}: the signature must not carry the message it signed"
            );
        }

        // The whole claim: the signature verifies under the public half the service
        // published for this key, with the scheme, hash and curve the algorithm names.
        let parsed = ParsedPublicKey::new(
            verification_algorithm(&label, case.key_spec, case.signing_algorithm),
            public_key,
        )
        .unwrap_or_else(|error| panic!("case {label}: the published public key parses: {error}"));
        parsed
            .verify_sig(&message, &signature)
            .unwrap_or_else(|error| {
                panic!(
                    "case {label}: the signature must verify over the message under the \
                     published public key: {error}"
                )
            });

        // A `DIGEST` row signed the hash, and KMS "skips the hashing step in the
        // signing algorithm" for it: so the signature is over the MESSAGE (verified
        // just above) and not over the digest bytes treated as a message of their own.
        if case.message_type == Some(DIGEST) {
            assert!(
                parsed.verify_sig(hashed.as_ref(), &signature).is_err(),
                "case {label}: a DIGEST message must not be hashed again before signing"
            );
        }
    }
}

/// One refusal row: what makes a signature impossible, and what KMS answers.
struct RefusalCase {
    case: &'static str,
    /// The `KeySpec` and `KeyUsage` of the key this row signs with, or `None` for a row
    /// whose `KeyId` names no key at all.
    key: Option<(&'static str, &'static str)>,
    /// An operation called on that key before signing, which is what puts it in the key
    /// state the row is about.
    before: Option<&'static str>,
    /// The `KeyId` the request names, when it is not the row's own key.
    names: Option<&'static str>,
    signing_algorithm: &'static str,
    message_type: Option<&'static str>,
    /// How many bytes the `Message` carries — for a `DIGEST` row, what makes it the
    /// right or the wrong length for the algorithm's hash.
    message_len: usize,
    dry_run: bool,
    /// The wire error code (`__type`) and the HTTP status of the answer. This service
    /// answers every client error with 400 and carries the shape in `__type`; a
    /// scheme nothing here signs with is the scaffold's 501.
    expected: &'static str,
    expected_status: u16,
}

/// Every reason a `Sign` request yields no signature.
///
/// The usage and algorithm rows are the two halves of `InvalidKeyUsageException`: "the
/// KeyUsage value of the KMS key is incompatible with the API operation" and "the
/// signing algorithm specified for the operation is incompatible with the type of key
/// material in the KMS key (KeySpec)"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`). The message-type and
/// digest-length rows are the request rules the same member's documentation states:
/// "the length of the Message value must match the length of hashed messages for the
/// specified signing algorithm", "ED25519_SHA_512 signing algorithm requires KMS
/// MessageType:RAW", "ED25519_PH_SHA_512 signing algorithm requires KMS
/// MessageType:DIGEST", and an external μ is "used in ML-DSA signing"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a symmetric encryption key",
        key: Some(("SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT")),
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an HMAC key",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an asymmetric key whose usage is encryption",
        key: Some(("RSA_2048", "ENCRYPT_DECRYPT")),
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a key-agreement key",
        key: Some(("ECC_NIST_P256", "KEY_AGREEMENT")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an elliptic-curve algorithm on an RSA key",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an RSA algorithm on an elliptic-curve key",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "the wrong hash for the curve",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_512",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "InvalidKeyUsageException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an identifier no key answers to",
        key: None,
        before: None,
        names: Some("00000000-0000-4000-8000-000000000000"),
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "NotFoundException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an alias that points at nothing",
        key: None,
        before: None,
        names: Some("alias/no-such-signing-key"),
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "NotFoundException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a disabled key",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        before: Some("DisableKey"),
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "DisabledException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a key scheduled for deletion",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        before: Some("ScheduleKeyDeletion"),
        names: None,
        signing_algorithm: "RSASSA_PSS_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: false,
        expected: "KMSInvalidStateException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a dry run",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(RAW),
        message_len: 32,
        dry_run: true,
        expected: "DryRunOperationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a digest one byte short of the hash",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(DIGEST),
        message_len: 31,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a digest one byte longer than the hash",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some(DIGEST),
        message_len: 33,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "a digest of another hash",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "RSASSA_PKCS1_V1_5_SHA_512",
        message_type: Some(DIGEST),
        message_len: 32,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "pure Ed25519 over a digest",
        key: Some(("ECC_NIST_EDWARDS25519", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ED25519_SHA_512",
        message_type: Some(DIGEST),
        message_len: 64,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "prehashed Ed25519 over a whole message",
        key: Some(("ECC_NIST_EDWARDS25519", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ED25519_PH_SHA_512",
        message_type: Some(RAW),
        message_len: 64,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
    RefusalCase {
        case: "an external mu for an elliptic-curve algorithm",
        key: Some(("ECC_NIST_P256", "SIGN_VERIFY")),
        before: None,
        names: None,
        signing_algorithm: "ECDSA_SHA_256",
        message_type: Some("EXTERNAL_MU"),
        message_len: 64,
        dry_run: false,
        expected: "ValidationException",
        expected_status: 400,
    },
];

/// The rule, continued: a request no signature can be produced for is refused with the
/// modelled error for why, and nothing is signed.
#[tokio::test]
async fn refuses_every_request_that_cannot_be_signed() {
    let router = router();

    // Every row names a distinct reason, so no reason is asserted twice and none is a
    // copy of another.
    let reasons: BTreeSet<&str> = REFUSAL_CASES.iter().map(|case| case.case).collect();
    assert_eq!(
        reasons.len(),
        REFUSAL_CASES.len(),
        "case table: every refusal row must name its own reason"
    );

    for case in REFUSAL_CASES {
        let key_id = match case.key {
            Some((key_spec, key_usage)) => {
                let metadata = create_key(
                    &router,
                    &json!({
                        "Description": case.case,
                        "KeySpec": key_spec,
                        "KeyUsage": key_usage
                    }),
                )
                .await;
                let key_id = text(case.case, &metadata, "KeyId");
                if let Some(before) = case.before {
                    ok(&router, before, &json!({"KeyId": key_id})).await;
                }
                key_id
            }
            None => String::new(),
        };
        let named = case.names.unwrap_or(&key_id);

        let mut request = json!({
            "KeyId": named,
            "Message": b64(&message_of(case.message_len)),
            "SigningAlgorithm": case.signing_algorithm,
        });
        if let Some(message_type) = case.message_type {
            request["MessageType"] = json!(message_type);
        }
        if case.dry_run {
            request["DryRun"] = json!(true);
        }

        let (status, payload) = call(&router, REGION, "Sign", &request).await;
        assert_eq!(
            status, case.expected_status,
            "case {}: expected HTTP {}, got {status} with {payload}",
            case.case, case.expected_status
        );
        assert_eq!(
            error_code(&payload),
            case.expected,
            "case {}: expected {}, got {payload}",
            case.case,
            case.expected
        );
        assert!(
            payload["Signature"].is_null(),
            "case {}: a refused request signs nothing, got {payload}",
            case.case
        );
    }
}

/// The signature schemes a KMS key can be created for and no library in this workspace
/// produces a signature with: ML-DSA, whose key pair is not generated here at all, and
/// the SM2DSA of GM/T 0003-2012. Such a request is answered as not implemented rather
/// than with bytes that are not a signature — the same answer `GetPublicKey` gives for
/// an ML-DSA key.
const UNSUPPORTED_CASES: &[(&str, &str)] = &[
    ("ML_DSA_44", "ML_DSA_SHAKE_256"),
    ("ML_DSA_65", "ML_DSA_SHAKE_256"),
    ("ML_DSA_87", "ML_DSA_SHAKE_256"),
    ("SM2", "SM2DSA"),
];

/// A scheme nothing here signs with answers the scaffold's 501, never a fabricated
/// signature and never a modelled refusal that would claim the request was wrong.
#[tokio::test]
async fn unsupported_signature_schemes_are_not_implemented() {
    let router = router();

    for (key_spec, signing_algorithm) in UNSUPPORTED_CASES {
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
        // ML-DSA and SM2DSA sign the message itself, so each row sends the message type
        // its algorithm takes and the 501 is about the scheme and not about the request.
        // Both Edwards25519 schemes are signed here now, so neither is a row.
        let message_type = RAW;
        let message = message_of(32);

        let (status, payload) = call(
            &router,
            REGION,
            "Sign",
            &json!({
                "KeyId": key_id,
                "Message": b64(&message),
                "MessageType": message_type,
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

/// The rule, continued: the signature comes from the key's own PERSISTED key pair, so a
/// deterministic scheme signs one message identically however often it is asked, two
/// keys of one spec sign it differently, and neither signature verifies under the other
/// key; and signing changes nothing else about the key.
#[tokio::test]
async fn a_signature_comes_from_the_persisted_key_pair() {
    let router = router();
    let message = message_of(64);
    // RSASSA-PKCS1-v1_5 is deterministic (RFC 8017 Section 8.2), so the same key over
    // the same message is the same signature — which is only true if the signature is
    // produced from material the store KEPT rather than minted for the call.
    let algorithm = "RSASSA_PKCS1_V1_5_SHA_256";

    let mut signatures: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
    for index in 0..2u8 {
        let case = format!("key {index}");
        let metadata = create_key(
            &router,
            &json!({"Description": case, "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        )
        .await;
        let key_id = text(&case, &metadata, "KeyId");
        let before = describe(&router, &key_id).await;

        let request = json!({
            "KeyId": key_id,
            "Message": b64(&message),
            "MessageType": RAW,
            "SigningAlgorithm": algorithm
        });
        let first = unb64(&case, &ok(&router, "Sign", &request).await, "Signature");
        let again = unb64(&case, &ok(&router, "Sign", &request).await, "Signature");
        assert_eq!(
            first, again,
            "case {case}: a deterministic scheme must sign one message identically twice"
        );

        // Signing is a USE of the key: it leaves the key's own metadata as it was.
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, before,
            "case {case}: signing must not change the key's metadata"
        );

        // The use is recorded, and KMS's usage tracking produces that record a little
        // after the call: once it has, the record names this operation.
        let usage = tracked_last_usage(&router, &case, &key_id, "Sign", &Value::Null).await;
        assert_eq!(
            usage["KeyLastUsage"]["Operation"].as_str().unwrap_or_default(),
            "Sign",
            "case {case}: the key's last usage must name Sign, got {usage}"
        );

        let published = ok(&router, "GetPublicKey", &json!({"KeyId": key_id})).await;
        signatures.push((case, first, unb64("published", &published, "PublicKey")));
    }

    let (first_case, first_signature, first_key) = &signatures[0];
    let (second_case, second_signature, second_key) = &signatures[1];
    assert_ne!(
        first_signature, second_signature,
        "case {first_case} vs {second_case}: two keys must not sign one message identically"
    );
    for (case, signature, own_key, other_key) in [
        (first_case, first_signature, first_key, second_key),
        (second_case, second_signature, second_key, first_key),
    ] {
        let verifier = |public_key: &[u8]| {
            ParsedPublicKey::new(
                verification_algorithm(case, "RSA_2048", algorithm),
                public_key,
            )
            .unwrap_or_else(|error| panic!("case {case}: the published public key parses: {error}"))
        };
        verifier(own_key)
            .verify_sig(&message, signature)
            .unwrap_or_else(|error| {
                panic!("case {case}: the signature must verify under its own key: {error}")
            });
        assert!(
            verifier(other_key).verify_sig(&message, signature).is_err(),
            "case {case}: the signature must not verify under another key's public key"
        );
    }
}
