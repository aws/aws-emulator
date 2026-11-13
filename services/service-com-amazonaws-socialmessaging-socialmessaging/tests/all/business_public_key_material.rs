// SPDX-License-Identifier: CC0-1.0

//! What `PutWhatsAppBusinessPublicKey` accepts as a business public key, and what it
//! refuses.
//!
//! RULE: a request member documented to carry key material is judged as that material from
//! the REQUEST alone, before any resource the request names is resolved — so a value that
//! is not a PEM-encoded X.509 RSA public key is refused 400 `InvalidParametersException`
//! naming the offending member, whatever the rest of the request says, and a value that is
//! one is stored and read back.
//!
//! `businessPublicKey` (`BusinessPublicKeyPem`, `@length 1..8192`, no `@pattern`) is the
//! only input member of this service that carries key material, and it is documented as
//! "The PEM-encoded 2048-bit RSA public key to set", so the rule is asserted over that
//! member here.
//!
//! The rows span what the rule generalises over: the FORM the submitted value takes (no PEM
//! envelope at all, an envelope under another label, an envelope holding something that is
//! not a public key structure, a public key of another algorithm, the PKCS#1 form rather
//! than the X.509 one) and, on the accepted side, the SIZE of the key and the envelope's
//! line endings. Which phone number the request names is varied independently, since the
//! rule does not depend on it.

use serde_json::{json, Value};

use crate::harness::{
    call, ed25519_public_key_pem, emulator, link, pem_block, pkcs1_rsa_public_key_der,
    query_escape, rsa_public_key_pem, Answer, INVALID_PARAMETERS_ERROR_TYPE,
    PUBLIC_KEY_PEM_LABEL,
};

/// The member the submitted key material arrives in, as the wire spells it
/// (`smithy_query(query="operation", operation="PutWhatsAppBusinessPublicKey")`).
const MEMBER: &str = "businessPublicKey";

/// The sentence a value that is not a public key is refused with: the offending member
/// name, then fixed text.
fn refusal_message(member: &str) -> String {
    format!("{member} is not a valid X.509 RSA public key.")
}

/// The message the phone-number lookup answers an unregistered number with
/// (`ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE` in `src/state.rs`).
const ORIGINATION_NOT_FOUND: &str = "Origination phone number id not found for account.";

/// A phone-number id matching the modelled pattern that names no registered number.
const UNKNOWN_NUMBER: &str = "phone-number-id-00000000000000000000000000000000";

async fn put_public_key(router: &smithy_server_runtime::MultiServiceRouter, body: Value) -> Answer {
    call(router, "PUT", "/v1/whatsapp/business-public-key", Some(body)).await
}

async fn get_public_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    phone_number_id: &str,
) -> Answer {
    let uri = format!(
        "/v1/whatsapp/business-public-key?originationPhoneNumberId={}",
        query_escape(phone_number_id)
    );
    call(router, "GET", &uri, None).await
}

/// The minted phone-number ids registered under `id`, read back through the account read —
/// the only way a caller learns them.
async fn phone_number_ids(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
) -> Vec<String> {
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(id));
    let account = call(router, "GET", &uri, None).await;
    assert_eq!(
        account.status, 200,
        "setup: expected 200 from GetLinkedWhatsAppBusinessAccount, got {} {:?}",
        account.status, account.body
    );
    account
        .at("/account/phoneNumbers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["phoneNumberId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The values that are NOT a PEM-encoded X.509 RSA public key, each with the case it
/// stands for. Every one of them is refused on the key, whatever number it is submitted
/// for.
fn values_that_are_not_a_public_key() -> Vec<(&'static str, String)> {
    vec![
        ("a value that is not PEM at all", "not-a-public-key".to_string()),
        (
            "base64 with no PEM envelope",
            "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A".to_string(),
        ),
        (
            "an envelope whose body is not base64",
            format!("-----BEGIN {PUBLIC_KEY_PEM_LABEL}-----\nnot base64 at all\n-----END {PUBLIC_KEY_PEM_LABEL}-----"),
        ),
        (
            "an envelope holding bytes that are not a public key structure",
            pem_block(PUBLIC_KEY_PEM_LABEL, &[0x2a; 64]),
        ),
        (
            "an envelope holding a truncated public key structure",
            {
                let whole = rsa_public_key_pem(2048);
                let der = ::pem::parse(&whole).expect("the builder's own PEM parses");
                let contents = der.contents();
                pem_block(PUBLIC_KEY_PEM_LABEL, &contents[..contents.len() / 2])
            },
        ),
        (
            "a public key of another algorithm",
            ed25519_public_key_pem(),
        ),
        (
            "the PKCS#1 form rather than the X.509 one",
            pem_block("RSA PUBLIC KEY", &pkcs1_rsa_public_key_der(2048)),
        ),
        (
            "the X.509 structure under the PKCS#1 label",
            {
                let whole = rsa_public_key_pem(2048);
                let der = ::pem::parse(&whole).expect("the builder's own PEM parses");
                pem_block("RSA PUBLIC KEY", der.contents())
            },
        ),
        (
            "an empty envelope",
            pem_block(PUBLIC_KEY_PEM_LABEL, &[]),
        ),
    ]
}

/// RULE, the refusal half: key material that is not a PEM-encoded X.509 RSA public key is
/// refused 400 `InvalidParametersException` naming the member it arrived in — and it is
/// refused BEFORE the phone number is resolved, so the same answer comes back for a
/// registered number and for one that names nothing.
#[tokio::test]
async fn key_material_that_is_not_an_x509_rsa_public_key_is_refused_on_the_member() {
    let router = emulator();
    let linked = link(
        &router,
        "waba-21212121212121212121212121212121",
        &["meta-phone-1"],
    )
    .await;
    let registered = phone_number_ids(&router, &linked).await;
    let registered = registered
        .first()
        .cloned()
        .expect("setup: the number the link registered");

    let expected_message = refusal_message(MEMBER);
    for (case, value) in values_that_are_not_a_public_key() {
        // The number the request names does not enter the decision: a registered number
        // and one that names nothing answer the same refusal.
        for (named, number) in [
            ("a registered number", registered.as_str()),
            ("a number that names nothing", UNKNOWN_NUMBER),
        ] {
            let refused = put_public_key(
                &router,
                json!({ "originationPhoneNumberId": number, "businessPublicKey": value }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error_type.as_str()),
                (400, INVALID_PARAMETERS_ERROR_TYPE),
                "case {case:?} on {named}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, \
                 got {} {:?} {:?}",
                refused.status,
                refused.error_type,
                refused.body
            );
            let actual = refused.message();
            assert_eq!(
                actual, expected_message,
                "case {case:?} on {named}: expected the message {expected_message:?}, got \
                 {actual:?}"
            );
        }
    }

    // None of those refusals set a key: the registered number still holds none.
    let read = get_public_key(&router, &registered).await;
    assert_eq!(
        (read.status, read.at("/businessPublicKey").clone()),
        (200, Value::Null),
        "case \"after every refusal\": expected the number to hold no key, got {} {:?}",
        read.status,
        read.body
    );
}

/// RULE, the accepted half: a PEM-encoded X.509 RSA public key is stored and read back as
/// submitted, whatever its size and however its envelope is wrapped — and a number that
/// names nothing is still the modelled 404, so the key check did not swallow the lookup.
#[tokio::test]
async fn an_x509_rsa_public_key_is_accepted_and_read_back() {
    let router = emulator();
    let linked = link(
        &router,
        "waba-23232323232323232323232323232323",
        &["meta-phone-1"],
    )
    .await;
    let registered = phone_number_ids(&router, &linked).await;
    let registered = registered
        .first()
        .cloned()
        .expect("setup: the number the link registered");

    // (case, the submitted key). The modelled documented size, two other RSA sizes, and
    // the same key with CRLF line endings — none of which changes the structure the
    // service judges.
    let accepted = vec![
        ("the documented 2048-bit key", rsa_public_key_pem(2048)),
        ("a 3072-bit key", rsa_public_key_pem(3072)),
        // The refusal states X.509 RSA VALIDITY, not a size, so a key of another size is
        // a key all the same.
        ("a 1024-bit key", rsa_public_key_pem(1024)),
        (
            "an envelope with CRLF line endings",
            rsa_public_key_pem(2048).replace('\n', "\r\n"),
        ),
    ];
    for (case, key) in accepted {
        let written = put_public_key(
            &router,
            json!({ "originationPhoneNumberId": registered, "businessPublicKey": key }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected 200 from PutWhatsAppBusinessPublicKey, got {} {:?}",
            written.status, written.body
        );
        let read = get_public_key(&router, &registered).await;
        assert_eq!(
            (
                read.status,
                read.at("/businessPublicKey").clone(),
                read.string_at("/businessPublicKeySignatureStatus")
            ),
            (200, json!(key), "VALID".to_string()),
            "case {case:?}: expected the read to report the key the write set, got {} {:?}",
            read.status,
            read.body
        );
    }

    // A key that IS one, for a number that names nothing: the lookup is what fails, and it
    // still answers the modelled not-found.
    let missing = put_public_key(
        &router,
        json!({
            "originationPhoneNumberId": UNKNOWN_NUMBER,
            "businessPublicKey": rsa_public_key_pem(2048),
        }),
    )
    .await;
    assert_eq!(
        (missing.status, missing.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a valid key for a number that names nothing\": expected 404 \
         ResourceNotFoundException, got {} {:?} {:?}",
        missing.status,
        missing.error_type,
        missing.body
    );
    let actual = missing.message();
    assert_eq!(
        actual, ORIGINATION_NOT_FOUND,
        "case \"a valid key for a number that names nothing\": expected the message \
         {ORIGINATION_NOT_FOUND:?}, got {actual:?}"
    );

    // A key held in KMS carries no key material, so there is nothing to judge: the write
    // is accepted and the read reports no PEM.
    let in_kms = put_public_key(
        &router,
        json!({
            "originationPhoneNumberId": registered,
            "kmsKeyArn": format!(
                "arn:aws:kms:us-east-1:{}:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                crate::harness::ACCOUNT_ID
            ),
        }),
    )
    .await;
    assert_eq!(
        in_kms.status, 200,
        "case \"a key held in KMS\": expected 200, got {} {:?}",
        in_kms.status, in_kms.body
    );
    let read = get_public_key(&router, &registered).await;
    assert_eq!(
        (read.status, read.at("/businessPublicKey").clone()),
        (200, Value::Null),
        "case \"a key held in KMS\": expected the read to report no PEM, got {} {:?}",
        read.status,
        read.body
    );
}

/// RULE, the naming half: every `InvalidParametersException` this service answers names its
/// shape in the located form, in the `x-amzn-errortype` header and in the body's `__type`
/// alike — while a resource lookup's miss keeps the bare shape name.
///
/// The rows span the refusals of that class the service has: the key material above, the
/// two members that name no key to set, and a request-content refusal of another
/// operation.
#[tokio::test]
async fn every_parameter_refusal_names_the_located_shape() {
    let router = emulator();
    let linked = link(
        &router,
        "waba-24242424242424242424242424242424",
        &["meta-phone-1"],
    )
    .await;
    let registered = phone_number_ids(&router, &linked).await;
    let registered = registered
        .first()
        .cloned()
        .expect("setup: the number the link registered");
    let kms_key_arn = format!(
        "arn:aws:kms:us-east-1:{}:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        crate::harness::ACCOUNT_ID
    );

    let refusals = vec![
        (
            "key material that is not a public key",
            "PUT",
            "/v1/whatsapp/business-public-key".to_string(),
            Some(json!({
                "originationPhoneNumberId": registered,
                "businessPublicKey": "not-a-public-key",
            })),
        ),
        (
            "both a PEM and a KMS key",
            "PUT",
            "/v1/whatsapp/business-public-key".to_string(),
            Some(json!({
                "originationPhoneNumberId": registered,
                "businessPublicKey": rsa_public_key_pem(2048),
                "kmsKeyArn": kms_key_arn,
            })),
        ),
        (
            "neither a PEM nor a KMS key",
            "PUT",
            "/v1/whatsapp/business-public-key".to_string(),
            Some(json!({ "originationPhoneNumberId": registered })),
        ),
        (
            "a template definition that defines no template",
            "POST",
            "/v1/whatsapp/template/put".to_string(),
            Some(json!({
                "id": linked,
                "templateDefinition": aws_smithy_types::base64::encode("not json at all"),
            })),
        ),
    ];
    for (case, method, uri, body) in refusals {
        let refused = call(&router, method, &uri, body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, INVALID_PARAMETERS_ERROR_TYPE),
            "case {case:?}: expected 400 {INVALID_PARAMETERS_ERROR_TYPE:?}, got {} {:?} {:?}",
            refused.status,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.string_at("/__type"),
            INVALID_PARAMETERS_ERROR_TYPE,
            "case {case:?}: expected the body to name {INVALID_PARAMETERS_ERROR_TYPE:?}, got {:?}",
            refused.body
        );
    }

    // The other side of the same fact: a lookup miss names the bare shape, in both places.
    let missing = get_public_key(&router, UNKNOWN_NUMBER).await;
    assert_eq!(
        (
            missing.status,
            missing.error_type.as_str(),
            missing.string_at("/__type").as_str()
        ),
        (
            404,
            "ResourceNotFoundException",
            "ResourceNotFoundException"
        ),
        "case \"a lookup miss\": expected the bare shape in both places, got {} {:?} {:?}",
        missing.status,
        missing.error_type,
        missing.body
    );
}
