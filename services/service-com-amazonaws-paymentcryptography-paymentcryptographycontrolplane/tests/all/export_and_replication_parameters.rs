// SPDX-License-Identifier: CC0-1.0

//! Three rules of this control plane, end to end over the wire: the attribute
//! combinations `CreateKey` accepts, the account's default key replication regions, and
//! the TR-34 export parameters `GetParametersForExport` hands out.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rules under test:
//!
//! * a key's `KeyUsage`, `KeyClass` and `KeyAlgorithm` each already say whether the key
//!   is a single secret or an asymmetric pair, and a create whose attributes disagree
//!   describes no key, so it is refused with `ValidationException`. The table below
//!   spans EVERY modelled `KeyUsage` against an algorithm of each kind.
//! * the default replication regions are account state: an enable adds to the set, a
//!   disable removes from it, a read reports it, and a key created without a
//!   replication configuration of its own takes the set as it stands.
//! * an export parameter set is minted state: its token names the set, its signing key
//!   certificate is a real X.509 certificate of a real RSA key pair signed by the
//!   chain it is handed with, both expire thirty days out, and the set is reused only
//!   when the caller asks for the last generated token and one with enough validity
//!   left exists for the same key material type and signing key algorithm.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ops::types::{
    KEY_ALGORITHM_VALUES, KEY_MATERIAL_TYPE_VALUES, KEY_USAGE_VALUES,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    EXPORT_PARAMETERS_VALIDITY, EXPORT_TOKEN_PREFIX, MAX_ENABLED_REPLICATION_REGIONS,
    TR34_SIGNING_KEY_ALGORITHM,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{
    call, error_code, now_secs, ok_call, router, router_with_engine, string, timestamp,
};

/// The modelled `KeyUsage` values that name an ASYMMETRIC key — a key pair rather than
/// one secret.
///
/// Source: `smithy_query(query="shape",shape="KeyUsage")`, whose values are the TR-31
/// key block usages ("as defined in section A.5.2 of the TR-31 spec", `KeyUsage`'s own
/// documentation); these four name a key for data encryption under a public key, for
/// key agreement, for a digital signature and the TR-34 asymmetric key. Every other
/// modelled usage names a symmetric key, which `every_modelled_key_usage_is_covered`
/// below pins against the generated value list.
const ASYMMETRIC_KEY_USAGES: &[&str] = &[
    "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION",
    "TR31_K2_TR34_ASYMMETRIC_KEY",
    "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
    "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
];

/// One algorithm of each kind, with the `KeyClass` this service generates it as.
///
/// Source: `smithy_query(query="shape",shape="SymmetricKeyAlgorithm")` for the
/// symmetric side — `AES_128` is in that list and `RSA_2048` is not.
const ALGORITHMS: &[(&str, &str, bool)] = &[
    ("AES_128", "SYMMETRIC_KEY", false),
    ("RSA_2048", "ASYMMETRIC_KEY_PAIR", true),
];

/// Regions the replication cases enable and disable.
///
/// Each matches the modelled `Region` pattern `^[a-z]{2}-[a-z]{1,16}-[0-9]+$`
/// (`smithy_query(query="shape",shape="Region")`), and none of them is the region the
/// harness signs for, so a default region is never also the key's own.
const REPLICATION_REGIONS: &[&str] = &["ap-southeast-2", "eu-west-1", "us-east-2"];

/// A `CreateKey` body for one attribute combination.
fn create_key_body(key_usage: &str, key_class: &str, key_algorithm: &str, extra: Value) -> Value {
    let mut body = json!({
        "KeyAttributes": {
            "KeyUsage": key_usage,
            "KeyClass": key_class,
            "KeyAlgorithm": key_algorithm,
            "KeyModesOfUse": {"NoRestrictions": true},
        },
        "Exportable": true,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

/// The table above has to name every modelled `KeyUsage`, so a usage the model gains
/// later cannot slip through untested.
#[test]
fn every_modelled_key_usage_is_covered() {
    let asymmetric: BTreeSet<&str> = ASYMMETRIC_KEY_USAGES.iter().copied().collect();
    let modelled: BTreeSet<&str> = KEY_USAGE_VALUES.iter().copied().collect();
    let unknown: Vec<&&str> = asymmetric.difference(&modelled).collect();
    assert!(
        unknown.is_empty(),
        "the asymmetric-usage table names values the model does not declare: {unknown:?}"
    );
    assert!(
        !modelled.is_empty() && modelled.len() > asymmetric.len(),
        "expected the model to declare both kinds of usage, got {} usages of which {} are \
         asymmetric",
        modelled.len(),
        asymmetric.len()
    );
}

#[tokio::test]
async fn create_key_accepts_only_attributes_describing_one_kind_of_key() {
    let router = router();

    for key_usage in KEY_USAGE_VALUES {
        let usage_is_asymmetric = ASYMMETRIC_KEY_USAGES.contains(key_usage);
        for (key_algorithm, key_class, algorithm_is_asymmetric) in ALGORITHMS {
            let case = format!("{key_usage} with {key_algorithm}");
            let accepted = usage_is_asymmetric == *algorithm_is_asymmetric;
            let body = create_key_body(key_usage, key_class, key_algorithm, json!({}));
            let (status, parsed) = call(&router, "CreateKey", &body).await;

            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case}: the attributes describe one key, expected 200, got {status} \
                     {parsed}"
                );
                // Round trip: the stored key reports back the attributes it was created
                // with.
                let key = parsed
                    .get("Key")
                    .unwrap_or_else(|| panic!("case {case}: no Key in {parsed}"));
                let arn = string(key, "KeyArn").to_string();
                let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
                let attributes = fetched
                    .get("Key")
                    .and_then(|key| key.get("KeyAttributes"))
                    .unwrap_or_else(|| panic!("case {case}: no KeyAttributes in {fetched}"));
                assert_eq!(
                    string(attributes, "KeyUsage"),
                    *key_usage,
                    "case {case}: GetKey must report the KeyUsage the key was created with, got \
                     {attributes}"
                );
                assert_eq!(
                    string(attributes, "KeyAlgorithm"),
                    *key_algorithm,
                    "case {case}: GetKey must report the KeyAlgorithm the key was created with, \
                     got {attributes}"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case}: the usage names the other kind of key, expected 400, got {status} \
                 {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: expected ValidationException, got {parsed}"
            );
            let expected =
                format!("KeyAlgorithm {key_algorithm} is not valid for the KeyUsage {key_usage}.");
            assert_eq!(
                parsed.get("Message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {parsed}"
            );
        }
    }
}

/// The regions a `GetDefaultKeyReplicationRegions`-shaped answer reports.
fn enabled_regions(body: &Value) -> Vec<String> {
    body.get("EnabledReplicationRegions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected EnabledReplicationRegions in {body}"))
        .iter()
        .map(|region| {
            region
                .as_str()
                .unwrap_or_else(|| panic!("expected a string region in {body}"))
                .to_string()
        })
        .collect()
}

/// The replication regions the store holds for the key `arn`.
fn stored_regions(engine: &engine::InMemoryEngine, arn: &str) -> Vec<String> {
    let state = engine.state::<KeyStore>();
    let store = state.lock();
    store
        .keys
        .get(arn)
        .unwrap_or_else(|| panic!("the store holds the key {arn}"))
        .replication_regions
        .keys()
        .cloned()
        .collect()
}

#[tokio::test]
async fn default_key_replication_regions_round_trip_and_new_keys_inherit_them() {
    let (router, engine) = router_with_engine();
    let [first, second, third] = <[&str; 3]>::try_from(REPLICATION_REGIONS).expect("three regions");

    // An account that has enabled none reports none.
    let empty = ok_call(&router, "GetDefaultKeyReplicationRegions", &json!({})).await;
    assert!(
        enabled_regions(&empty).is_empty(),
        "case initial: expected no enabled regions, got {empty}"
    );

    // The rows differ along what the rule generalises over: how many regions one call
    // names, whether they overlap what is already enabled, whether a disabled region
    // was ever enabled, and the order the caller writes them in.
    let cases: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "enable two",
            "EnableDefaultKeyReplicationRegions",
            &[second, first],
            &[first, second],
        ),
        (
            "enable one already enabled, plus a new one",
            "EnableDefaultKeyReplicationRegions",
            &[second, third],
            &[first, second, third],
        ),
        (
            "enable the same two again",
            "EnableDefaultKeyReplicationRegions",
            &[first, second],
            &[first, second, third],
        ),
        (
            "disable one",
            "DisableDefaultKeyReplicationRegions",
            &[second],
            &[first, third],
        ),
        (
            "disable one that was never enabled",
            "DisableDefaultKeyReplicationRegions",
            &[second],
            &[first, third],
        ),
    ];

    for (case, operation, requested, expected) in cases {
        let answered = ok_call(
            &router,
            operation,
            &json!({"ReplicationRegions": requested}),
        )
        .await;
        assert_eq!(
            enabled_regions(&answered),
            *expected,
            "case {case}: expected {expected:?} enabled, got {answered}"
        );
        // The read reports what the write left behind.
        let read = ok_call(&router, "GetDefaultKeyReplicationRegions", &json!({})).await;
        assert_eq!(
            enabled_regions(&read),
            *expected,
            "case {case}: GetDefaultKeyReplicationRegions must report what the call left \
             enabled, got {read}"
        );
    }

    // A key created without a replication configuration of its own takes the account
    // defaults as they now stand, and says it is using them.
    let defaults = vec![first.to_string(), third.to_string()];
    let created = ok_call(
        &router,
        "CreateKey",
        &create_key_body(
            "TR31_P0_PIN_ENCRYPTION_KEY",
            "SYMMETRIC_KEY",
            "AES_128",
            json!({}),
        ),
    )
    .await;
    let inherited = created
        .get("Key")
        .unwrap_or_else(|| panic!("no Key in {created}"));
    let inherited_arn = string(inherited, "KeyArn").to_string();
    assert_eq!(
        inherited.get("UsingDefaultReplicationRegions"),
        Some(&Value::Bool(true)),
        "case inherited: a key created with no ReplicationRegions uses the account defaults, \
         got {inherited}"
    );
    assert_eq!(
        stored_regions(&engine, &inherited_arn),
        defaults,
        "case inherited: expected the key to replicate to the account defaults {defaults:?}, got \
         {:?}",
        stored_regions(&engine, &inherited_arn)
    );

    // A key created WITH its own regions overrides the defaults.
    let own = ok_call(
        &router,
        "CreateKey",
        &create_key_body(
            "TR31_P0_PIN_ENCRYPTION_KEY",
            "SYMMETRIC_KEY",
            "AES_128",
            json!({"ReplicationRegions": [second]}),
        ),
    )
    .await;
    let overriding = own.get("Key").unwrap_or_else(|| panic!("no Key in {own}"));
    let overriding_arn = string(overriding, "KeyArn").to_string();
    assert_eq!(
        overriding.get("UsingDefaultReplicationRegions"),
        Some(&Value::Bool(false)),
        "case own regions: a key created with ReplicationRegions overrides the defaults, got \
         {overriding}"
    );
    assert_eq!(
        stored_regions(&engine, &overriding_arn),
        vec![second.to_string()],
        "case own regions: expected the key to replicate to {second}, got {:?}",
        stored_regions(&engine, &overriding_arn)
    );

    // "Existing keys are not affected by this operation"
    // (`EnableDefaultKeyReplicationRegions`' own documentation): a later enable leaves
    // the key minted above where it was.
    ok_call(
        &router,
        "EnableDefaultKeyReplicationRegions",
        &json!({"ReplicationRegions": [second]}),
    )
    .await;
    assert_eq!(
        stored_regions(&engine, &inherited_arn),
        defaults,
        "case existing key: enabling another default region must not move an existing key's \
         replication regions, got {:?}",
        stored_regions(&engine, &inherited_arn)
    );

    // The set this emulator holds is bounded, and a call that would take the account
    // past it is refused with the modelled `ServiceQuotaExceededException` and stores
    // nothing. Each generated name matches the modelled `Region` pattern.
    let enabled_before = enabled_regions(
        &ok_call(&router, "GetDefaultKeyReplicationRegions", &json!({})).await,
    );
    let too_many: Vec<String> = (0..=MAX_ENABLED_REPLICATION_REGIONS)
        .map(|n| format!("eu-west-{n}"))
        .collect();
    let (status, parsed) = call(
        &router,
        "EnableDefaultKeyReplicationRegions",
        &json!({"ReplicationRegions": too_many}),
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
    let enabled_after =
        enabled_regions(&ok_call(&router, "GetDefaultKeyReplicationRegions", &json!({})).await);
    assert_eq!(
        enabled_after, enabled_before,
        "case past the bound: a refused enable must store nothing, expected {enabled_before:?}, \
         got {enabled_after:?}"
    );
}

/// Assert `token` satisfies the modelled `ExportTokenId` pattern
/// `^(export-token-[0-9a-zA-Z]{16,64})?$`
/// (`smithy_query(query="operation",operation="GetParametersForExport")`), checked by
/// hand because this crate has no regex dependency.
fn assert_export_token(case: &str, token: &str) {
    let id = token.strip_prefix(EXPORT_TOKEN_PREFIX).unwrap_or_else(|| {
        panic!("case {case}: ExportToken {token:?} does not start with {EXPORT_TOKEN_PREFIX}")
    });
    assert!(
        (16..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: ExportToken {token:?} carries the id segment {id:?}, which is not \
         [0-9a-zA-Z]{{16,64}}"
    );
}

/// The DER of the one PEM certificate a `SigningKeyCertificate` member carries: the
/// member is "the signing key certificate in PEM format (base64 encoded)", so the value
/// is base64 of a PEM block.
fn certificate_der(case: &str, member: &str, value: &str) -> Vec<u8> {
    let pem = ::aws_smithy_types::base64::decode(value)
        .unwrap_or_else(|e| panic!("case {case}: {member} is not base64 ({e}): {value}"));
    let pem = String::from_utf8(pem)
        .unwrap_or_else(|e| panic!("case {case}: {member} is not a PEM document ({e})"));
    let body = pem
        .strip_prefix("-----BEGIN CERTIFICATE-----\n")
        .and_then(|rest| rest.split_once("-----END CERTIFICATE-----"))
        .unwrap_or_else(|| panic!("case {case}: {member} is not one PEM certificate: {pem}"))
        .0;
    let base64: String = body.split_whitespace().collect();
    ::aws_smithy_types::base64::decode(&base64)
        .unwrap_or_else(|e| panic!("case {case}: {member}'s PEM body is not base64 ({e})"))
}

/// Check the signing key certificate and the chain it came with are real: both parse as
/// X.509 certificates, the chain certificate issued the signing one, and the signature
/// over the signing certificate verifies under the chain certificate's public key.
fn assert_real_certificate_chain(case: &str, answer: &Value) {
    use x509_cert::der::{Decode, Encode};

    let signing_der = certificate_der(
        case,
        "SigningKeyCertificate",
        string(answer, "SigningKeyCertificate"),
    );
    let chain_der = certificate_der(
        case,
        "SigningKeyCertificateChain",
        string(answer, "SigningKeyCertificateChain"),
    );
    let signing = x509_cert::Certificate::from_der(&signing_der)
        .unwrap_or_else(|e| panic!("case {case}: SigningKeyCertificate is not X.509 DER ({e})"));
    let chain = x509_cert::Certificate::from_der(&chain_der).unwrap_or_else(|e| {
        panic!("case {case}: SigningKeyCertificateChain is not X.509 DER ({e})")
    });

    assert_eq!(
        signing.tbs_certificate.issuer, chain.tbs_certificate.subject,
        "case {case}: expected the chain certificate to be the issuer of the signing \
         certificate, got issuer {:?} and chain subject {:?}",
        signing.tbs_certificate.issuer, chain.tbs_certificate.subject
    );

    // The signature is verified with the chain certificate's own public key, so a
    // certificate whose signature was not made by that key fails here.
    let signed = signing
        .tbs_certificate
        .to_der()
        .unwrap_or_else(|e| panic!("case {case}: the signed certificate body re-encodes ({e})"));
    let signature = signing
        .signature
        .as_bytes()
        .unwrap_or_else(|| panic!("case {case}: the certificate signature is not whole bytes"));
    let issuer_key = chain
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .unwrap_or_else(|e| panic!("case {case}: the issuer public key re-encodes ({e})"));
    let parsed = ::aws_lc_rs::signature::ParsedPublicKey::new(
        &::aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
        &issuer_key,
    )
    .unwrap_or_else(|e| panic!("case {case}: the issuer public key is not an RSA key ({e})"));
    parsed.verify_sig(&signed, signature).unwrap_or_else(|e| {
        panic!("case {case}: the signing certificate's signature must verify under its issuer ({e})")
    });
}

#[tokio::test]
async fn get_parameters_for_export_mints_a_real_signing_certificate_per_token() {
    let router = router();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    // The last token each material type was handed, so the reuse case below can be held
    // to its OWN type's set rather than to whichever set was minted last.
    let mut last_for_type: BTreeMap<&str, String> = BTreeMap::new();

    // One row per modelled `KeyMaterialType`
    // (`smithy_query(query="shape",shape="KeyMaterialType")`), since the set a reuse
    // request matches is keyed on it.
    for key_material_type in KEY_MATERIAL_TYPE_VALUES {
        let case = format!("{key_material_type} first call");
        let body = json!({
            "KeyMaterialType": key_material_type,
            "SigningKeyAlgorithm": TR34_SIGNING_KEY_ALGORITHM,
        });
        let answered = ok_call(&router, "GetParametersForExport", &body).await;

        assert_export_token(&case, string(&answered, "ExportToken"));
        assert_eq!(
            string(&answered, "SigningKeyAlgorithm"),
            TR34_SIGNING_KEY_ALGORITHM,
            "case {case}: expected the signing key algorithm asked for, got {answered}"
        );
        // "The export token expires in 30 days" (the operation's own documentation).
        let expires = timestamp(&answered, "ParametersValidUntilTimestamp");
        let expected = now_secs() + EXPORT_PARAMETERS_VALIDITY.as_secs_f64();
        assert!(
            (expires - expected).abs() < 60.0,
            "case {case}: expected the parameters to expire around {expected}, got {expires}"
        );
        assert_real_certificate_chain(&case, &answered);

        // Each call that does not reuse mints its own token and its own key pair.
        let token = string(&answered, "ExportToken").to_string();
        assert!(
            tokens.insert(token.clone()),
            "case {case}: expected a token no earlier call handed out, got {token} again"
        );

        // A second call with no reuse flag: "The default value is false, which generates
        // a new export token and signing key certificate on every call"
        // (`ReuseLastGeneratedToken`' own documentation).
        let fresh = ok_call(&router, "GetParametersForExport", &body).await;
        let fresh_token = string(&fresh, "ExportToken").to_string();
        assert!(
            tokens.insert(fresh_token.clone()),
            "case {key_material_type} default: expected a NEW token, got {fresh_token} again"
        );
        assert_ne!(
            string(&fresh, "SigningKeyCertificate"),
            string(&answered, "SigningKeyCertificate"),
            "case {key_material_type} default: expected a new signing key certificate too"
        );

        // …and one that asks to reuse answers the last set for this material type.
        let mut reuse = body.clone();
        reuse["ReuseLastGeneratedToken"] = Value::Bool(true);
        let reused = ok_call(&router, "GetParametersForExport", &reuse).await;
        assert_eq!(
            string(&reused, "ExportToken"),
            fresh_token,
            "case {key_material_type} reuse: expected the last generated token, got {reused}"
        );
        assert_eq!(
            string(&reused, "SigningKeyCertificate"),
            string(&fresh, "SigningKeyCertificate"),
            "case {key_material_type} reuse: expected the certificate of the token it reused"
        );
        assert_eq!(
            timestamp(&reused, "ParametersValidUntilTimestamp"),
            timestamp(&fresh, "ParametersValidUntilTimestamp"),
            "case {key_material_type} reuse: a reused token keeps its own expiry"
        );
        last_for_type.insert(key_material_type, fresh_token);
    }

    // Reuse is keyed on the key material type, not on "the set minted last": asking the
    // FIRST material type to reuse, after every later type has minted sets of its own,
    // must answer that first type's own last token.
    let first_type = KEY_MATERIAL_TYPE_VALUES[0];
    let expected_token = last_for_type
        .get(first_type)
        .unwrap_or_else(|| panic!("the loop recorded a token for {first_type}"));
    let other = ok_call(
        &router,
        "GetParametersForExport",
        &json!({
            "KeyMaterialType": first_type,
            "SigningKeyAlgorithm": TR34_SIGNING_KEY_ALGORITHM,
            "ReuseLastGeneratedToken": true,
        }),
    )
    .await;
    assert_eq!(
        string(&other, "ExportToken"),
        expected_token,
        "case reuse of {first_type}: expected that type's own last token {expected_token}, got \
         {other}"
    );
}

#[tokio::test]
async fn get_parameters_for_export_refuses_every_other_signing_key_algorithm() {
    let router = router();

    // "RSA_2048 is the only signing key algorithm allowed"
    // (`SigningKeyAlgorithm`'s own documentation), so every other member of the
    // modelled `KeyAlgorithm` enum is refused — the whole published list, not a
    // sample.
    for signing_key_algorithm in KEY_ALGORITHM_VALUES {
        let case = format!("sign with {signing_key_algorithm}");
        let body = json!({
            "KeyMaterialType": "TR34_KEY_BLOCK",
            "SigningKeyAlgorithm": signing_key_algorithm,
        });
        let (status, parsed) = call(&router, "GetParametersForExport", &body).await;

        if *signing_key_algorithm == TR34_SIGNING_KEY_ALGORITHM {
            assert_eq!(
                status, 200,
                "case {case}: the one allowed algorithm must be served, got {status} {parsed}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
        // The refusal is worded as `SigningKeyAlgorithm`'s own documentation states the
        // rule.
        let expected = format!("{TR34_SIGNING_KEY_ALGORITHM} is the only signing key algorithm allowed");
        assert_eq!(
            parsed.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {parsed}"
        );
    }
}
