// SPDX-License-Identifier: CC0-1.0

//! The certificate signing request `GetCertificateSigningRequest` makes, end to end
//! over the wire.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rule under test: a certificate signing request is a real PKCS#10 request over
//! the key pair the named key has held since it was created — its subject is the
//! metadata the call named, its `SubjectPublicKeyInfo` is that pair's public half (the
//! same public key on every call, whether the key is named by ARN or by alias), and its
//! signature is made by that pair's private half under the signature algorithm the
//! requested hash and the key's own algorithm name. A pair this service signs no
//! request for is refused rather than answered with some other signature, and so is a
//! key that is not a key pair or an identifier that names no key.
//!
//! The tables below span what the rule generalises over: every modelled `KeyAlgorithm`
//! a key pair is generated for, every modelled `SigningAlgorithm`, and subjects from
//! the one required member up to every optional one.

use serde_json::{json, Value};
use x509_cert::der::{Decode, Encode};
use x509_cert::request::CertReq;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ops::types::SIGNING_ALGORITHM_TYPE_VALUES;

use crate::key_lifecycle::{call, error_code, ok_call, router, string};

/// One asymmetric key algorithm, with the attributes a key of it is created with.
///
/// Source: `smithy_query(query="shape",shape="KeyAlgorithm,KeyClass,KeyUsage")`; the
/// rows are every modelled algorithm this service generates a key PAIR for, which
/// `every_asymmetric_algorithm_is_covered` pins against the generated value list.
struct KeyRow {
    key_algorithm: &'static str,
    key_usage: &'static str,
    /// The flags the create sets in `KeyModesOfUse`.
    modes: &'static [&'static str],
    /// The modelled `SigningAlgorithm` values a certificate signing request for this
    /// key is signed under, and nothing else.
    ///
    /// An RSA key signs with RSASSA-PKCS1-v1_5, which RFC 8017 appendix C gives an
    /// algorithm identifier for over each SHA-2 hash; an elliptic curve key signs with
    /// ECDSA under the hash matched to its curve's security strength (FIPS 186-4
    /// section 6.1.1).
    signed_with: &'static [&'static str],
}

const KEY_ROWS: &[KeyRow] = &[
    KeyRow {
        key_algorithm: "RSA_2048",
        key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
        modes: &["Sign", "Verify"],
        signed_with: &["SHA256", "SHA384", "SHA512"],
    },
    KeyRow {
        key_algorithm: "RSA_3072",
        key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
        modes: &["Sign", "Verify"],
        signed_with: &["SHA256", "SHA384", "SHA512"],
    },
    KeyRow {
        key_algorithm: "RSA_4096",
        key_usage: "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION",
        modes: &["Encrypt", "Decrypt"],
        signed_with: &["SHA256", "SHA384", "SHA512"],
    },
    KeyRow {
        key_algorithm: "ECC_NIST_P256",
        key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
        modes: &["DeriveKey"],
        signed_with: &["SHA256"],
    },
    KeyRow {
        key_algorithm: "ECC_NIST_P384",
        key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
        modes: &["DeriveKey"],
        signed_with: &["SHA384"],
    },
    KeyRow {
        key_algorithm: "ECC_NIST_P521",
        key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
        modes: &["DeriveKey"],
        signed_with: &["SHA512"],
    },
];

/// The object identifiers of the subject attributes a `CertificateSubjectType` member
/// is written as (RFC 5280 appendix A, and PKCS#9 for the email address).
const OID_COUNTRY: &str = "2.5.4.6";
const OID_STATE_OR_PROVINCE: &str = "2.5.4.8";
const OID_LOCALITY: &str = "2.5.4.7";
const OID_ORGANIZATION: &str = "2.5.4.10";
const OID_ORGANIZATIONAL_UNIT: &str = "2.5.4.11";
const OID_COMMON_NAME: &str = "2.5.4.3";
const OID_EMAIL_ADDRESS: &str = "1.2.840.113549.1.9.1";

/// A `CreateKey` body for one row.
fn create_key_body(row: &KeyRow) -> Value {
    let mut modes = serde_json::Map::new();
    for flag in row.modes {
        modes.insert((*flag).to_string(), json!(true));
    }
    json!({
        "KeyAttributes": {
            "KeyUsage": row.key_usage,
            "KeyClass": "ASYMMETRIC_KEY_PAIR",
            "KeyAlgorithm": row.key_algorithm,
            "KeyModesOfUse": Value::Object(modes),
        },
        "Exportable": true,
    })
}

/// Create one key and answer its `KeyArn`.
async fn create_key(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> String {
    let created = ok_call(router, "CreateKey", body).await;
    let key = created
        .get("Key")
        .unwrap_or_else(|| panic!("CreateKey answered no Key: {created}"));
    string(key, "KeyArn").to_string()
}

/// The signature algorithm identifier a request for this pair must name: RFC 8017
/// appendix C for `sha{256,384,512}WithRSAEncryption`, RFC 5758 section 3.2 for
/// `ecdsa-with-SHA{256,384,512}`.
fn expected_signature_oid(key_algorithm: &str, signing_algorithm: &str) -> &'static str {
    match (key_algorithm, signing_algorithm) {
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA256") => "1.2.840.113549.1.1.11",
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA384") => "1.2.840.113549.1.1.12",
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA512") => "1.2.840.113549.1.1.13",
        ("ECC_NIST_P256", "SHA256") => "1.2.840.10045.4.3.2",
        ("ECC_NIST_P384", "SHA384") => "1.2.840.10045.4.3.3",
        ("ECC_NIST_P521", "SHA512") => "1.2.840.10045.4.3.4",
        _ => panic!("{key_algorithm} with {signing_algorithm} signs no request"),
    }
}

/// The `aws-lc-rs` verification algorithm that checks a signature made for this pair.
fn verification_algorithm(
    key_algorithm: &str,
    signing_algorithm: &str,
) -> &'static dyn ::aws_lc_rs::signature::VerificationAlgorithm {
    match (key_algorithm, signing_algorithm) {
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA256") => {
            &::aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256
        }
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA384") => {
            &::aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA384
        }
        ("RSA_2048" | "RSA_3072" | "RSA_4096", "SHA512") => {
            &::aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA512
        }
        ("ECC_NIST_P256", "SHA256") => &::aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1,
        ("ECC_NIST_P384", "SHA384") => &::aws_lc_rs::signature::ECDSA_P384_SHA384_ASN1,
        ("ECC_NIST_P521", "SHA512") => &::aws_lc_rs::signature::ECDSA_P521_SHA512_ASN1,
        _ => panic!("{key_algorithm} with {signing_algorithm} signs no request"),
    }
}

/// The DER of the one PEM certificate request a `CertificateSigningRequest` carries.
///
/// Every certificate this service hands out is "in PEM format (base64 encoded)" (the
/// `CertificateType` members' own documentation) and a request is handed out the same
/// way, so the value is base64 of a PKCS#10 PEM block.
fn request_der(case: &str, value: &str) -> Vec<u8> {
    // The modelled `CertificateSigningRequestType`: 1..32768 with the pattern
    // `^[^\[;\]<>]+$` (source: `smithy_query(query="operation",
    // operation="GetCertificateSigningRequest")`).
    assert!(
        (1..=32768).contains(&value.len())
            && !value.chars().any(|c| "[;]<>".contains(c)),
        "case {case}: CertificateSigningRequest has length {} and must match the modelled \
         pattern, got {value}",
        value.len()
    );
    let pem = ::aws_smithy_types::base64::decode(value)
        .unwrap_or_else(|e| panic!("case {case}: the request is not base64 ({e}): {value}"));
    let pem = String::from_utf8(pem)
        .unwrap_or_else(|e| panic!("case {case}: the request is not a PEM document ({e})"));
    let body = pem
        .strip_prefix("-----BEGIN CERTIFICATE REQUEST-----\n")
        .and_then(|rest| rest.split_once("-----END CERTIFICATE REQUEST-----"))
        .unwrap_or_else(|| panic!("case {case}: not one PEM certificate request: {pem}"))
        .0;
    let base64: String = body.split_whitespace().collect();
    ::aws_smithy_types::base64::decode(&base64)
        .unwrap_or_else(|e| panic!("case {case}: the PEM body is not base64 ({e})"))
}

/// One parsed certificate signing request.
fn parse_request(case: &str, value: &str) -> CertReq {
    let der = request_der(case, value);
    CertReq::from_der(&der)
        .unwrap_or_else(|e| panic!("case {case}: the request is not PKCS#10 DER ({e})"))
}

/// The subject attributes a request carries, in DER order, as `(oid, value)` pairs.
fn subject_attributes(request: &CertReq) -> Vec<(String, String)> {
    request
        .info
        .subject
        .0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .map(|attribute| {
            (
                attribute.oid.to_string(),
                String::from_utf8_lossy(attribute.value.value()).to_string(),
            )
        })
        .collect()
}

/// Check a request's signature verifies under the public key the request itself
/// carries, with the primitive the key's algorithm and the requested hash name.
fn assert_signature_verifies(
    case: &str,
    key_algorithm: &str,
    signing_algorithm: &str,
    request: &CertReq,
) {
    assert_eq!(
        request.algorithm.oid.to_string(),
        expected_signature_oid(key_algorithm, signing_algorithm),
        "case {case}: expected the request to name {}, got {}",
        expected_signature_oid(key_algorithm, signing_algorithm),
        request.algorithm.oid
    );
    let signed = request
        .info
        .to_der()
        .unwrap_or_else(|e| panic!("case {case}: the signed request body re-encodes ({e})"));
    let signature = request
        .signature
        .as_bytes()
        .unwrap_or_else(|| panic!("case {case}: the request signature is not whole bytes"));
    let public_key = request
        .info
        .public_key
        .to_der()
        .unwrap_or_else(|e| panic!("case {case}: the request public key re-encodes ({e})"));
    let parsed = ::aws_lc_rs::signature::ParsedPublicKey::new(
        verification_algorithm(key_algorithm, signing_algorithm),
        &public_key,
    )
    .unwrap_or_else(|e| panic!("case {case}: the request's public key does not parse ({e})"));
    parsed.verify_sig(&signed, signature).unwrap_or_else(|e| {
        panic!(
            "case {case}: the signature must verify under the public key the request carries ({e})"
        )
    });
}

/// The table above has to name every modelled algorithm a key pair is generated for, so
/// an algorithm the model gains later cannot slip through untested.
#[test]
fn every_asymmetric_algorithm_is_covered() {
    use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ops::types::{
        KEY_ALGORITHM_VALUES, SYMMETRIC_KEY_ALGORITHM_VALUES,
    };

    let asymmetric: Vec<&&str> = KEY_ALGORITHM_VALUES
        .iter()
        .filter(|algorithm| !SYMMETRIC_KEY_ALGORITHM_VALUES.contains(algorithm))
        .collect();
    let covered: Vec<&str> = KEY_ROWS.iter().map(|row| row.key_algorithm).collect();
    for algorithm in &asymmetric {
        assert!(
            covered.contains(*algorithm),
            "the table does not cover the modelled asymmetric algorithm {algorithm}, covered: \
             {covered:?}"
        );
    }
    assert_eq!(
        covered.len(),
        asymmetric.len(),
        "the table names {} algorithms and the model declares {} asymmetric ones",
        covered.len(),
        asymmetric.len()
    );
}

/// One row of [`KEY_ROWS`], driven over every modelled `SigningAlgorithm`.
///
/// One key per test rather than every row in one: each test then costs a single key
/// generation, which for the larger RSA modulus sizes is real CPU, and the rows run
/// alongside each other instead of in series.
async fn signs_only_the_algorithms_the_key_admits(row: &KeyRow) {
    let router = router();
    let arn = create_key(&router, &create_key_body(row)).await;

    // Every modelled `SigningAlgorithm`, not only the ones this key admits: the rule
    // says which pairs are signed AND that the rest are refused.
    for signing_algorithm in SIGNING_ALGORITHM_TYPE_VALUES {
        let case = format!("{} with {signing_algorithm}", row.key_algorithm);
        let body = json!({
            "KeyIdentifier": arn,
            "SigningAlgorithm": signing_algorithm,
            "CertificateSubject": {"CommonName": format!("csr-{}", row.key_algorithm)},
        });
        let (status, parsed) = call(&router, "GetCertificateSigningRequest", &body).await;

        if !row.signed_with.contains(signing_algorithm) {
            assert_eq!(
                status, 400,
                "case {case}: this service signs no such request, expected 400, got {status} \
                 {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: expected ValidationException, got {parsed}"
            );
            let expected = format!(
                "SigningAlgorithm {signing_algorithm} is not valid for the KeyAlgorithm {}. \
                 Use one of {}.",
                row.key_algorithm,
                row.signed_with.join(", ")
            );
            assert_eq!(
                parsed.get("Message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {parsed}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {parsed}"
        );
        let request = parse_request(&case, string(&parsed, "CertificateSigningRequest"));
        assert_eq!(
            subject_attributes(&request),
            vec![(
                OID_COMMON_NAME.to_string(),
                format!("csr-{}", row.key_algorithm)
            )],
            "case {case}: expected the subject to name only the CommonName the call gave, got \
             {:?}",
            subject_attributes(&request)
        );
        assert_signature_verifies(&case, row.key_algorithm, signing_algorithm, &request);
    }
}

#[tokio::test]
async fn an_rsa_2048_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[0]).await;
}

#[tokio::test]
async fn an_rsa_3072_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[1]).await;
}

#[tokio::test]
async fn an_rsa_4096_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[2]).await;
}

#[tokio::test]
async fn a_p256_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[3]).await;
}

#[tokio::test]
async fn a_p384_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[4]).await;
}

#[tokio::test]
async fn a_p521_key_signs_only_the_algorithms_it_admits() {
    signs_only_the_algorithms_the_key_admits(&KEY_ROWS[5]).await;
}

/// One subject row: the members a call names, and the attributes the request must
/// then carry, in DER order.
struct SubjectRow {
    case: &'static str,
    subject: fn() -> Value,
    expected: fn() -> Vec<(String, String)>,
}

/// The subject members, from only the required `CommonName` up to every optional one.
///
/// Source: `smithy_query(query="shape",shape="CertificateSubjectType")` — `CommonName`
/// required, `OrganizationUnit`, `Organization`, `City`, `Country`, `StateOrProvince`
/// and `EmailAddress` optional. The expected order is the X.500 one, most general
/// first, with the PKCS#9 email address last.
const SUBJECT_ROWS: &[SubjectRow] = &[
    SubjectRow {
        case: "only the required CommonName",
        subject: || json!({"CommonName": "terminal-1"}),
        expected: || vec![(OID_COMMON_NAME.to_string(), "terminal-1".to_string())],
    },
    SubjectRow {
        case: "an organization and its unit",
        subject: || {
            json!({
                "CommonName": "terminal-2",
                "Organization": "Acme Payments",
                "OrganizationUnit": "Terminal Estate",
            })
        },
        expected: || {
            vec![
                (OID_ORGANIZATION.to_string(), "Acme Payments".to_string()),
                (
                    OID_ORGANIZATIONAL_UNIT.to_string(),
                    "Terminal Estate".to_string(),
                ),
                (OID_COMMON_NAME.to_string(), "terminal-2".to_string()),
            ]
        },
    },
    SubjectRow {
        case: "every modelled member",
        subject: || {
            json!({
                "CommonName": "terminal-3",
                "OrganizationUnit": "Treasury",
                "Organization": "Acme Payments",
                "City": "Amsterdam",
                "Country": "NL",
                "StateOrProvince": "Noord-Holland",
                "EmailAddress": "keys@acme-payments.example",
            })
        },
        expected: || {
            vec![
                (OID_COUNTRY.to_string(), "NL".to_string()),
                (
                    OID_STATE_OR_PROVINCE.to_string(),
                    "Noord-Holland".to_string(),
                ),
                (OID_LOCALITY.to_string(), "Amsterdam".to_string()),
                (OID_ORGANIZATION.to_string(), "Acme Payments".to_string()),
                (OID_ORGANIZATIONAL_UNIT.to_string(), "Treasury".to_string()),
                (OID_COMMON_NAME.to_string(), "terminal-3".to_string()),
                (
                    OID_EMAIL_ADDRESS.to_string(),
                    "keys@acme-payments.example".to_string(),
                ),
            ]
        },
    },
];

#[tokio::test]
async fn a_request_carries_the_subject_the_call_names() {
    let router = router();
    let row = &KEY_ROWS[0];
    let arn = create_key(&router, &create_key_body(row)).await;

    for subject_row in SUBJECT_ROWS {
        let case = subject_row.case;
        let answered = ok_call(
            &router,
            "GetCertificateSigningRequest",
            &json!({
                "KeyIdentifier": arn,
                "SigningAlgorithm": "SHA256",
                "CertificateSubject": (subject_row.subject)(),
            }),
        )
        .await;
        let request = parse_request(case, string(&answered, "CertificateSigningRequest"));
        assert_eq!(
            subject_attributes(&request),
            (subject_row.expected)(),
            "case {case}: expected the subject attributes the call named, in X.500 order, got {:?}",
            subject_attributes(&request)
        );
        // Still a real request whatever the subject: the signature is over this very
        // subject, so a request assembled from someone else's subject fails here.
        assert_signature_verifies(case, row.key_algorithm, "SHA256", &request);
    }
}

#[tokio::test]
async fn every_request_for_one_key_carries_that_key_pair_whichever_name_it_is_asked_for() {
    let router = router();

    // One RSA key and one elliptic curve key, so the rule is checked on a key of each
    // family rather than on one algorithm.
    for row in [&KEY_ROWS[0], &KEY_ROWS[3]] {
        let case = row.key_algorithm;
        let arn = create_key(&router, &create_key_body(row)).await;
        let alias = format!("alias/csr-{}", row.key_algorithm.to_lowercase());
        ok_call(
            &router,
            "CreateAlias",
            &json!({"AliasName": alias, "KeyArn": arn}),
        )
        .await;

        let signing_algorithm = row.signed_with[0];
        let mut public_keys = Vec::new();
        // The same key, named by ARN, named by alias, and named by ARN again: a key
        // pair that was minted per call, or looked up by the identifier's spelling,
        // would answer a different public key to one of these.
        for identifier in [arn.as_str(), alias.as_str(), arn.as_str()] {
            let answered = ok_call(
                &router,
                "GetCertificateSigningRequest",
                &json!({
                    "KeyIdentifier": identifier,
                    "SigningAlgorithm": signing_algorithm,
                    "CertificateSubject": {"CommonName": "stable-key"},
                }),
            )
            .await;
            let request = parse_request(case, string(&answered, "CertificateSigningRequest"));
            assert_signature_verifies(case, row.key_algorithm, signing_algorithm, &request);
            public_keys.push(
                request
                    .info
                    .public_key
                    .to_der()
                    .unwrap_or_else(|e| panic!("case {case}: the public key re-encodes ({e})")),
            );
        }
        assert!(
            public_keys.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case}: every request for one key must carry that key's one public key, got {} \
             distinct keys",
            {
                let mut distinct = public_keys.clone();
                distinct.dedup();
                distinct.len()
            }
        );
    }
}

#[tokio::test]
async fn a_request_is_refused_for_a_key_that_is_not_a_key_pair_and_for_an_unknown_identifier() {
    let (router, _engine) = crate::key_lifecycle::router_with_engine();

    // A symmetric key is one secret, not a pair, so there is no public key to put in a
    // request. One row per symmetric family, so the refusal is not keyed to one
    // algorithm.
    let symmetric: &[(&str, &str, &[&str])] = &[
        ("AES_128", "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY", &["NoRestrictions"]),
        ("TDES_3KEY", "TR31_P0_PIN_ENCRYPTION_KEY", &["Encrypt", "Decrypt"]),
        ("HMAC_SHA256", "TR31_M7_HMAC_KEY", &["Generate", "Verify"]),
    ];
    for (key_algorithm, key_usage, modes) in symmetric {
        let mut flags = serde_json::Map::new();
        for flag in *modes {
            flags.insert((*flag).to_string(), json!(true));
        }
        let created = ok_call(
            &router,
            "CreateKey",
            &json!({
                "KeyAttributes": {
                    "KeyUsage": key_usage,
                    "KeyClass": "SYMMETRIC_KEY",
                    "KeyAlgorithm": key_algorithm,
                    "KeyModesOfUse": Value::Object(flags),
                },
                "Exportable": true,
            }),
        )
        .await;
        let arn = string(
            created
                .get("Key")
                .unwrap_or_else(|| panic!("case {key_algorithm}: CreateKey answered no Key")),
            "KeyArn",
        )
        .to_string();

        let (status, parsed) = call(
            &router,
            "GetCertificateSigningRequest",
            &json!({
                "KeyIdentifier": arn,
                "SigningAlgorithm": "SHA256",
                "CertificateSubject": {"CommonName": "no-pair"},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {key_algorithm}: a symmetric key has no key pair, expected 400, got {status} \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {key_algorithm}: expected ValidationException, got {parsed}"
        );
        let expected = format!(
            "KeyIdentifier names the key {arn}, which is a SYMMETRIC_KEY, and a certificate \
             signing request is created from the key pair of an ASYMMETRIC_KEY_PAIR"
        );
        assert_eq!(
            parsed.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {key_algorithm}: expected the message {expected:?}, got {parsed}"
        );
    }

    // An identifier this store holds no key for, in each form the modelled
    // `KeyArnOrKeyAliasType` admits: a well-formed ARN of no key, and an alias that was
    // never created.
    let absent = format!(
        "arn:aws:payment-cryptography:{}:000000000000:key/abcdefghijklmnop",
        crate::key_lifecycle::REGION
    );
    for identifier in [absent.as_str(), "alias/csr-never-created"] {
        let (status, parsed) = call(
            &router,
            "GetCertificateSigningRequest",
            &json!({
                "KeyIdentifier": identifier,
                "SigningAlgorithm": "SHA256",
                "CertificateSubject": {"CommonName": "nowhere"},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {identifier}: no key is named, expected this service's client-error status \
             400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {identifier}: expected ResourceNotFoundException, got {parsed}"
        );
        assert_eq!(
            parsed.get("ResourceId").and_then(Value::as_str),
            Some(identifier),
            "case {identifier}: expected the identifier the request gave as ResourceId, got \
             {parsed}"
        );
    }
}
