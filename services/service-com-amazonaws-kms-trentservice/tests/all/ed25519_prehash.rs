// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PREHASHED Edwards25519 signature scheme, driven over the
//! wire through the shared [`super::harness`].
//!
//! The rule under test: each signing algorithm a key spec supports is the SIGNATURE
//! SCHEME that algorithm names, performed by the library that implements it — so for an
//! `ECC_NIST_EDWARDS25519` key, the prehashed algorithm signs the SHA-512 digest the
//! caller submits under the prehashed scheme of RFC 8032 Section 5.1, which is a
//! different scheme from the pure one the other Edwards25519 algorithm names: the
//! signature verifies as a prehashed signature under the public half `GetPublicKey`
//! publishes, the two schemes' signatures over one message are different bytes, neither
//! verifies as the other, and `Verify` refuses a signature over any other digest.
//!
//! Three suites assert it over TABLES rather than one call:
//!
//! 1. the signing and verifying path, over messages spanning what a caller can prehash —
//!    from one byte to well past the `Message` bound a whole message is held to, which
//!    is the reason the scheme exists — each answer checked INDEPENDENTLY against the
//!    published public key by a library this service does not sign with;
//! 2. the signatures of the two Edwards25519 schemes over one message, which differ, and
//!    the cross-scheme and cross-key checks that must fail;
//! 3. the message type each Edwards25519 algorithm takes, and the wording of the refusal
//!    of a request that names the other one, over both operations.

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};

use super::harness::{
    b64, call, create_key, error_code, error_message, ok, router, text, unb64, REGION,
};

/// The key spec whose key pair is an Edwards25519 one
/// (`smithy_query(query="shape",shape="KeySpec")`).
const KEY_SPEC: &str = "ECC_NIST_EDWARDS25519";

/// The `SigningAlgorithmSpec` of the prehashed scheme, and of the pure one
/// (`smithy_query(query="shape",shape="SigningAlgorithmSpec")`) — the two algorithms an
/// Edwards25519 key supports.
const PREHASH: &str = "ED25519_PH_SHA_512";
const PURE: &str = "ED25519_SHA_512";

/// The `MessageType` values of a digest and of a whole message
/// (`smithy_query(query="shape",shape="MessageType")`).
const DIGEST: &str = "DIGEST";
const RAW: &str = "RAW";

/// How many bytes an Edwards25519 signature is: the `R` and `S` encoding of RFC 8032
/// Section 5.1.6, 32 bytes each.
const SIGNATURE_LEN: usize = 64;

/// How long the messages a row prehashes are. The caller hashes the message itself under
/// this scheme, so the lengths span from the smallest message upward and past 4096 — the
/// bound the `Message` member holds a WHOLE message to
/// (`smithy_query(query="operation",operation="Sign")`), which prehashing is the way
/// around.
const MESSAGE_LENS: &[usize] = &[1, 3, 32, 4096, 20_000];

/// The message a row signs: `len` bytes that are neither constant nor the message of any
/// other length, so no signature can be right by accident of its input.
fn message_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + len) % 251).unwrap_or_default())
        .collect()
}

/// The SHA-512 digest of `message`: what a caller submits under the prehashed algorithm,
/// computed here by the test's own hash so the digest does not come from the service's.
fn prehash(message: &[u8]) -> Vec<u8> {
    Sha512::digest(message).to_vec()
}

/// Create an Edwards25519 signing key and report its key id, its ARN and the public half
/// `GetPublicKey` publishes for it.
async fn signing_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> (String, String, VerifyingKey) {
    let metadata = create_key(
        router,
        &json!({"Description": case, "KeySpec": KEY_SPEC, "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let key_id = text(case, &metadata, "KeyId");
    let key_arn = text(case, &metadata, "Arn");
    let published = ok(router, "GetPublicKey", &json!({"KeyId": key_id})).await;
    let algorithms: Vec<&str> = published["SigningAlgorithms"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected SigningAlgorithms in {published}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        algorithms.contains(&PREHASH),
        "case {case}: the key advertises the prehashed algorithm, got {algorithms:?}"
    );
    let der = unb64(case, &published, "PublicKey");
    // RFC 8410 Section 4: an Edwards25519 `SubjectPublicKeyInfo` is a 12-byte prefix
    // (the SEQUENCE, the id-Ed25519 algorithm identifier and the BIT STRING header)
    // followed by the 32-byte public key, so the key is the DER's last 32 bytes.
    let public_key: [u8; 32] = der[der.len().saturating_sub(32)..]
        .try_into()
        .unwrap_or_else(|_| panic!("case {case}: expected a 32-byte public key in {der:?}"));
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .unwrap_or_else(|error| panic!("case {case}: the published key is a point: {error}"));
    (key_id, key_arn, verifying_key)
}

/// Sign through the service and report the signature bytes, asserting the answer's own
/// members.
async fn sign(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
    key_arn: &str,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Vec<u8> {
    let answer = ok(
        router,
        "Sign",
        &json!({
            "KeyId": key_id,
            "Message": b64(message),
            "MessageType": message_type,
            "SigningAlgorithm": signing_algorithm,
        }),
    )
    .await;
    assert_eq!(
        text(case, &answer, "KeyId"),
        key_arn,
        "case {case:?}: the answer names the key by its ARN, got {answer}"
    );
    assert_eq!(
        text(case, &answer, "SigningAlgorithm"),
        signing_algorithm,
        "case {case:?}: the answer names the algorithm it signed with, got {answer}"
    );
    unb64(case, &answer, "Signature")
}

/// Verify through the service and report the HTTP status and body.
async fn verify(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_id: &str,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
    signature: &[u8],
) -> (u16, Value) {
    call(
        router,
        REGION,
        "Verify",
        &json!({
            "KeyId": key_id,
            "Message": b64(message),
            "MessageType": message_type,
            "Signature": b64(signature),
            "SigningAlgorithm": signing_algorithm,
        }),
    )
    .await
}

/// Whether `signature` is the PREHASHED signature of `message` under `verifying_key`,
/// decided OUTSIDE the service: the verifying side is handed the whole message and
/// hashes it itself, so a signature that passes here is a signature of the prehashed
/// scheme over that message and of nothing else.
fn verifies_as_prehashed(verifying_key: &VerifyingKey, message: &[u8], signature: &[u8]) -> bool {
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    verifying_key
        .verify_prehashed(Sha512::new_with_prefix(message), None, &signature)
        .is_ok()
}

/// Rule part 1: the prehashed algorithm answers a real prehashed signature of the
/// submitted digest, which the published public key verifies and `Verify` accepts,
/// however long the message behind that digest was.
#[tokio::test]
async fn signs_and_verifies_a_prehashed_digest_of_any_message() {
    let router = router();
    let (key_id, key_arn, verifying_key) = signing_key(&router, "prehashed signing").await;

    for &len in MESSAGE_LENS {
        let case = format!("a {len}-byte message");
        let message = message_of(len);
        let digest = prehash(&message);
        let signature = sign(
            &router, &case, &key_id, &key_arn, PREHASH, DIGEST, &digest,
        )
        .await;

        assert_eq!(
            signature.len(),
            SIGNATURE_LEN,
            "case {case:?}: an Edwards25519 signature is {SIGNATURE_LEN} bytes, got {}",
            signature.len()
        );
        assert!(
            verifies_as_prehashed(&verifying_key, &message, &signature),
            "case {case:?}: the signature must verify as a prehashed signature of the \
             message under the published public key, got {signature:?}"
        );
        // The same key over the same digest is the same signature: the prehashed scheme
        // is deterministic (RFC 8032 Section 5.1.6), which it can only be if the
        // signature comes from the key pair the store KEPT.
        let again = sign(
            &router, &case, &key_id, &key_arn, PREHASH, DIGEST, &digest,
        )
        .await;
        assert_eq!(
            again, signature,
            "case {case:?}: the deterministic scheme signs one digest identically"
        );

        let (status, payload) = verify(&router, &key_id, PREHASH, DIGEST, &digest, &signature).await;
        assert_eq!(
            (status, payload["SignatureValid"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the key's own Verify accepts the signature, got {status} with \
             {payload}"
        );
        assert_eq!(
            text(&case, &payload, "KeyId"),
            key_arn,
            "case {case:?}: the verification names the key by its ARN, got {payload}"
        );

        // A digest that differs in ONE BIT is another message, and no signature of this
        // one is a signature of it.
        let mut tampered = digest.clone();
        let bit = len % tampered.len();
        tampered[bit] ^= 1;
        let (status, payload) = verify(&router, &key_id, PREHASH, DIGEST, &tampered, &signature)
            .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "KMSInvalidSignatureException"),
            "case {case:?}: a one-bit-different digest must not verify, got {status} with \
             {payload}"
        );
        assert!(
            payload["SignatureValid"].is_null(),
            "case {case:?}: a refused verification reports no result, got {payload}"
        );
    }
}

/// Rule part 2: the two Edwards25519 schemes are different schemes, and each key's
/// signatures are its own.
#[tokio::test]
async fn the_prehashed_scheme_is_not_the_pure_scheme_and_not_another_key() {
    let router = router();
    let (key_id, key_arn, verifying_key) = signing_key(&router, "one key").await;
    let (other_id, other_arn, _) = signing_key(&router, "another key").await;

    for &len in MESSAGE_LENS {
        // The pure scheme signs the message itself, so only a message within the
        // `Message` bound can be signed both ways and compared.
        if len > 4096 {
            continue;
        }
        let case = format!("a {len}-byte message");
        let message = message_of(len);
        let digest = prehash(&message);

        let prehashed = sign(
            &router, &case, &key_id, &key_arn, PREHASH, DIGEST, &digest,
        )
        .await;
        let pure = sign(&router, &case, &key_id, &key_arn, PURE, RAW, &message).await;
        assert_ne!(
            prehashed, pure,
            "case {case:?}: the prehashed signature of a message is not its pure \
             signature"
        );

        // Neither signature is accepted as the other scheme's.
        for (scheme, algorithm, message_type, signed, signature) in [
            ("a pure", PREHASH, DIGEST, digest.as_slice(), &pure),
            ("a prehashed", PURE, RAW, message.as_slice(), &prehashed),
        ] {
            let (status, payload) =
                verify(&router, &key_id, algorithm, message_type, signed, signature).await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "KMSInvalidSignatureException"),
                "case {case:?}: {scheme} signature must not verify as {algorithm}, got \
                 {status} with {payload}"
            );
        }

        // Another key of the same spec signs the same digest differently, and does not
        // verify this key's signature.
        let other = sign(
            &router, &case, &other_id, &other_arn, PREHASH, DIGEST, &digest,
        )
        .await;
        assert_ne!(
            other, prehashed,
            "case {case:?}: two keys of one spec sign a digest differently"
        );
        assert!(
            !verifies_as_prehashed(&verifying_key, &message, &other),
            "case {case:?}: the other key's signature must not verify under this key's \
             public half"
        );
        let (status, payload) = verify(&router, &key_id, PREHASH, DIGEST, &digest, &other).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "KMSInvalidSignatureException"),
            "case {case:?}: another key's signature must not verify, got {status} with \
             {payload}"
        );
    }
}

/// Rule part 3: each Edwards25519 algorithm takes exactly ONE message type — the
/// prehashed one a digest, the pure one the message — and a request that names the other
/// is refused as that pairing being incompatible, in the same words from either
/// operation.
#[tokio::test]
async fn refuses_the_message_type_an_edwards25519_algorithm_does_not_take() {
    let router = router();
    let (key_id, key_arn, _) = signing_key(&router, "message types").await;
    let message = message_of(64);
    let digest = prehash(&message);
    let signature = sign(
        &router,
        "message types",
        &key_id,
        &key_arn,
        PREHASH,
        DIGEST,
        &digest,
    )
    .await;

    // Rows: each algorithm with the message type it does NOT take. Both are 64 bytes
    // long, so no row is refused for its length instead of its message type.
    for (algorithm, message_type) in [(PURE, DIGEST), (PREHASH, RAW)] {
        let case = format!("{algorithm} over {message_type}");
        let expected =
            format!("Message type {message_type} is incompatible with algorithm {algorithm}.");

        let (status, payload) = call(
            &router,
            REGION,
            "Sign",
            &json!({
                "KeyId": key_id,
                "Message": b64(&digest),
                "MessageType": message_type,
                "SigningAlgorithm": algorithm,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "ValidationException"),
            "case {case:?}: Sign must refuse the pairing, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: Sign must word the refusal as AWS does, got {payload}"
        );

        let (status, payload) = verify(
            &router,
            &key_id,
            algorithm,
            message_type,
            &digest,
            &signature,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "ValidationException"),
            "case {case:?}: Verify must refuse the pairing, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: Verify must word the refusal as AWS does, got {payload}"
        );
    }
}
