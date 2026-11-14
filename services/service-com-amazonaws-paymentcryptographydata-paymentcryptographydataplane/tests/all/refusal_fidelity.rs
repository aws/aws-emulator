// SPDX-License-Identifier: CC0-1.0

//! How this data plane states the requests it REFUSES, end to end over the wire.
//!
//! Every call goes through the real `MultiServiceRouter`, with the Payment Cryptography
//! CONTROL plane mounted on the same engine so the keys these cases use are the ones
//! `CreateKey` minted and stored.
//!
//! Four rules are under test, each over a table that spans the class it generalises over
//! rather than the one request a recording happened to carry:
//!
//! 1. A request that breaks a rule the MODEL declares is the modelled
//!    `ValidationException` (HTTP 400): one envelope sentence counting the violations,
//!    one clause per violation naming the member as a JSON pointer spelled the way the
//!    model spells it and the requirement that member failed, and the same clause in a
//!    `ValidationException$fieldList` entry per member. No value the caller submitted is
//!    echoed anywhere in it.
//! 2. A request the codec cannot READ into the modelled input shape broke no modelled
//!    rule: it is a serialization failure, announced as such in `x-amzn-errortype`, which
//!    is where a restJson1 client reads the error class it raises.
//! 3. A verifying operation's `VerificationFailedException` states the verification class
//!    that failed — one sentence per class, the same for every input of that class.
//! 4. A MAC operation judges the key's own algorithm against the requested MAC algorithm
//!    BEFORE it judges that key's modes of use, and words that refusal with the requested
//!    algorithm and the key's algorithm and nothing of the key's identity.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService as ControlPlane;
use service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::EmulatedService as DataPlane;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix of the control plane's `awsJson1_0` surface.
const CONTROL_TARGET_PREFIX: &str = "PaymentCryptographyControlPlane";

/// One emulator: the production router with BOTH Payment Cryptography services mounted on
/// one shared engine, exactly as `build_router` mounts them.
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

/// One answer as the wire carries it: the status, the error type the protocol announces,
/// and the body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error code the body's `__type` carries, with any `shapeId#` prefix dropped.
    fn code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or("<no __type>")
    }

    /// The `message` the refusal states, under either spelling the model uses for it
    /// (`ValidationException$message`, `VerificationFailedException$Message`).
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }

    /// The `fieldList` entries as `(path, message)` pairs, or `None` when the refusal
    /// carries no field list at all.
    fn field_list(&self) -> Option<Vec<(String, String)>> {
        Some(
            self.body
                .get("fieldList")?
                .as_array()?
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect(),
        )
    }
}

/// Call one CONTROL plane operation (`awsJson1_0`, `POST /` with `X-Amz-Target`).
async fn control(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", authorization())
        .header(
            "host",
            format!("controlplane.payment-cryptography.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CONTROL_TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    dispatch(router, request, operation).await
}

/// Call one DATA plane operation (`restJson1`, `POST` to the operation's own URI) with a
/// body this suite hands over verbatim — including one the SDK would never send.
async fn data(router: &MultiServiceRouter, uri: &str, body: &str) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization())
        .header(
            "host",
            format!("dataplane.payment-cryptography.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    dispatch(router, request, uri).await
}

/// Call one DATA plane operation with a JSON document.
async fn data_json(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    data(
        router,
        uri,
        &serde_json::to_string(body).expect("the request body serializes"),
    )
    .await
}

/// Dispatch and read back the status, the announced error type and the JSON body.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Bytes>, what: &str) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no x-amzn-errortype>")
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{what} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// One key this suite works with.
#[derive(Debug, Clone)]
struct Key {
    arn: String,
}

/// Mint a key through the control plane's own `CreateKey` and read its ARN back out.
async fn create_key(
    router: &MultiServiceRouter,
    key_algorithm: &str,
    key_usage: &str,
    modes: Value,
) -> Key {
    let body = json!({
        "KeyAttributes": {
            "KeyUsage": key_usage,
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": key_algorithm,
            "KeyModesOfUse": modes,
        },
        "Exportable": true,
    });
    let answer = control(router, "CreateKey", &body).await;
    assert_eq!(
        answer.status, 200,
        "CreateKey {key_algorithm}/{key_usage}: expected 200, got {answer:?}"
    );
    let arn = answer.body["Key"]["KeyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey {key_algorithm}/{key_usage} answered no KeyArn: {answer:?}"))
        .to_string();
    Key { arn }
}

/// The `KeyUsage` values this suite creates keys under, from the control plane's modelled
/// `KeyUsage`.
const USAGE_DATA_ENCRYPTION: &str = "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY";
const USAGE_ISO_9797_3_MAC: &str = "TR31_M3_ISO_9797_3_MAC_KEY";
const USAGE_CMAC: &str = "TR31_M6_ISO_9797_5_CMAC_KEY";
const USAGE_HMAC: &str = "TR31_M7_HMAC_KEY";
const USAGE_CARD_VERIFICATION: &str = "TR31_C0_CARD_VERIFICATION_KEY";

/// Percent-encode the reserved characters a key ARN carries, so it fits the single URI
/// label `EncryptData` and `DecryptData` bind it to.
fn encoded(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// A message of `bytes` bytes, as the uppercase hex every data member's pattern admits.
fn hex_data(bytes: usize) -> String {
    (0..bytes)
        .map(|index| format!("{:02X}", (index * 7 + 3) % 256))
        .collect()
}

/// The envelope sentence a refusal of `n` violations opens with, as this service states
/// it: the count, then the clauses.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected. {}", clauses[0]),
        n => format!("{n} validation errors detected. {}", clauses.join("; ")),
    }
}

/// The clause one violated member contributes: its JSON pointer and the requirement.
fn clause(pointer: &str, requirement: &str) -> String {
    format!("Value at '{pointer}' failed to satisfy constraint: {requirement}")
}

/// Assert that `answer` is the modelled `ValidationException` stating exactly `clauses` —
/// the envelope as the `message`, and one `fieldList` entry per clause under the pointer
/// it names.
fn assert_validation(case: &str, answer: &Answer, pointers_and_requirements: &[(&str, String)]) {
    let clauses: Vec<String> = pointers_and_requirements
        .iter()
        .map(|(pointer, requirement)| clause(pointer, requirement))
        .collect();
    assert_eq!(
        (answer.status, answer.code(), answer.error_type.as_str()),
        (400, "ValidationException", "ValidationException"),
        "case {case}: expected a 400 ValidationException announced in the error-type \
         header, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        envelope(&clauses),
        "case {case}: expected the validation envelope, got {answer:?}"
    );
    let expected: Vec<(String, String)> = pointers_and_requirements
        .iter()
        .zip(&clauses)
        .map(|((pointer, _), clause)| ((*pointer).to_string(), clause.clone()))
        .collect();
    let fields = answer
        .field_list()
        .unwrap_or_else(|| panic!("case {case}: expected a fieldList, got {answer:?}"));
    assert_eq!(
        fields.iter().cloned().collect::<BTreeSet<_>>(),
        expected.iter().cloned().collect::<BTreeSet<_>>(),
        "case {case}: expected fieldList {expected:?}, got {fields:?}"
    );
}

#[tokio::test]
async fn an_absent_required_member_is_refused_with_its_pointer_and_a_field_list() {
    let router = emulator();
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true}),
    )
    .await;
    let encrypt_key = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"Encrypt": true, "Decrypt": true}),
    )
    .await;
    let encrypt_uri = format!("/keys/{}/encrypt", encoded(&encrypt_key.arn));
    let decrypt_uri = format!("/keys/{}/decrypt", encoded(&encrypt_key.arn));

    // One row per `@required` member left out, spanning several operations, both
    // top-level and nested members, and both the body and the shapes a union carries.
    // Every pointer is the member as the MODEL spells it.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "EncryptData without EncryptionAttributes",
            encrypt_uri.as_str(),
            json!({"PlainText": hex_data(16)}),
            "/EncryptionAttributes",
        ),
        (
            "EncryptData without PlainText",
            encrypt_uri.as_str(),
            json!({"EncryptionAttributes": {"Symmetric": {"Mode": "CBC"}}}),
            "/PlainText",
        ),
        (
            // A member nested inside the attributes union: the pointer is the trail of
            // modelled member names the absent member was found under, as the
            // framework's own deserializer reports it (a union's variant is read as the
            // one member present, so the trail is the enclosing member and then the
            // absent one).
            "EncryptData without the Symmetric attributes' Mode",
            encrypt_uri.as_str(),
            json!({"PlainText": hex_data(16), "EncryptionAttributes": {"Symmetric": {}}}),
            "/EncryptionAttributes/Mode",
        ),
        (
            "DecryptData without DecryptionAttributes",
            decrypt_uri.as_str(),
            json!({"CipherText": hex_data(16)}),
            "/DecryptionAttributes",
        ),
        (
            "GenerateMac without MessageData",
            "/mac/generate",
            json!({"KeyIdentifier": key.arn, "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"}}),
            "/MessageData",
        ),
        (
            "VerifyMac without Mac",
            "/mac/verify",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "VerificationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
            }),
            "/Mac",
        ),
        (
            "GenerateCardValidationData without PrimaryAccountNumber",
            "/cardvalidationdata/generate",
            json!({
                "KeyIdentifier": key.arn,
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
            "/PrimaryAccountNumber",
        ),
        (
            "VerifyCardValidationData without ValidationData",
            "/cardvalidationdata/verify",
            json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "171234567890123",
                "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
            "/ValidationData",
        ),
        (
            "GenerateCardValidationData without the attributes' CardExpiryDate",
            "/cardvalidationdata/generate",
            json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "171234567890123",
                "GenerationAttributes": {"CardVerificationValue2": {}},
            }),
            "/GenerationAttributes/CardExpiryDate",
        ),
    ];

    for (case, uri, body, pointer) in rows {
        let answer = data_json(&router, uri, &body).await;
        assert_validation(
            case,
            &answer,
            // `MissingRequiredMember` is the one requirement the validation framework
            // words this way.
            &[(pointer, "Member must not be null".to_string())],
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_names_the_member_and_the_requirement() {
    let router = emulator();
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true}),
    )
    .await;
    let encrypt_key = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"Encrypt": true, "Decrypt": true}),
    )
    .await;
    let encrypt_uri = format!("/keys/{}/encrypt", encoded(&encrypt_key.arn));

    // The constraint classes the model declares on this service's inputs, one row each:
    // `@range` at and past each published bound, `@length` under its minimum, `@pattern`,
    // and an enumerated value set. The requirement text is the validation framework's own
    // for that class; the submitted value appears in NONE of them, which is what keeps a
    // `@sensitive` member's value (`MessageData`, `PlainText`, `PrimaryAccountNumber`) out
    // of a refusal.
    let hex = hex_data(8);
    let rows: Vec<(&str, &str, Value, &str, String, String)> = vec![
        (
            // `IntegerRangeBetween4And32`, one below the published minimum.
            "GenerateMac with MacLength 3",
            "/mac/generate",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex.clone(),
                "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
                "MacLength": 3,
            }),
            "/MacLength",
            "Member must have value greater than or equal to 4".to_string(),
            "3".to_string(),
        ),
        (
            // The same shape, one above the published maximum.
            "GenerateMac with MacLength 33",
            "/mac/generate",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex.clone(),
                "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
                "MacLength": 33,
            }),
            "/MacLength",
            "Member must have value less than or equal to 32".to_string(),
            "33".to_string(),
        ),
        (
            // `IntegerRangeBetween3And5Type`, one above its published maximum.
            "GenerateCardValidationData with ValidationDataLength 6",
            "/cardvalidationdata/generate",
            json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "171234567890123",
                "GenerationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
                "ValidationDataLength": 6,
            }),
            "/ValidationDataLength",
            "Member must have value less than or equal to 5".to_string(),
            "6".to_string(),
        ),
        (
            // `PrimaryAccountNumberType`, shorter than its published minimum of 12.
            "VerifyCardValidationData with a short PrimaryAccountNumber",
            "/cardvalidationdata/verify",
            json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": "17123456789",
                "ValidationData": "123",
                "VerificationAttributes": {"CardVerificationValue2": {"CardExpiryDate": "0123"}},
            }),
            "/PrimaryAccountNumber",
            "Member must have length greater than or equal to 12".to_string(),
            "17123456789".to_string(),
        ),
        (
            // `MessageDataType`'s `@pattern`, quoted as the expression a value must
            // satisfy rather than with the anchoring the model wraps it in.
            "GenerateMac with MessageData that is not hexadecimal",
            "/mac/generate",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": "zzzz",
                "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
            }),
            "/MessageData",
            "Member must satisfy regular expression pattern: (?:[0-9a-fA-F][0-9a-fA-F])+"
                .to_string(),
            "zzzz".to_string(),
        ),
        (
            // The same class on a `@sensitive` member of another operation.
            "EncryptData with PlainText that is not hexadecimal",
            encrypt_uri.as_str(),
            json!({
                "PlainText": "zzzz",
                "EncryptionAttributes": {"Symmetric": {"Mode": "CBC"}},
            }),
            "/PlainText",
            "Member must satisfy regular expression pattern: (?:[0-9a-fA-F][0-9a-fA-F])+"
                .to_string(),
            "zzzz".to_string(),
        ),
    ];

    for (case, uri, body, pointer, requirement, submitted) in rows {
        let answer = data_json(&router, uri, &body).await;
        assert_validation(case, &answer, &[(pointer, requirement)]);
        assert!(
            !answer.message().contains(&submitted),
            "case {case}: the refusal must not echo the submitted value {submitted}, \
             got {answer:?}"
        );
    }
}

#[tokio::test]
async fn several_violated_members_are_counted_once_and_listed_each() {
    let router = emulator();
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true}),
    )
    .await;
    // Two members of one request, each breaking a different constraint class: the
    // envelope counts both, and the field list carries one entry per member.
    let answer = data_json(
        &router,
        "/mac/generate",
        &json!({
            "KeyIdentifier": key.arn,
            "MessageData": "zzzz",
            "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
            "MacLength": 99,
        }),
    )
    .await;
    assert_validation(
        "GenerateMac with a bad MessageData and a bad MacLength",
        &answer,
        &[
            (
                "/MessageData",
                "Member must satisfy regular expression pattern: \
                 (?:[0-9a-fA-F][0-9a-fA-F])+"
                    .to_string(),
            ),
            (
                "/MacLength",
                "Member must have value less than or equal to 32".to_string(),
            ),
        ],
    );
    assert!(
        answer.message().starts_with("2 validation errors detected. "),
        "the envelope must count the violations it states, got {answer:?}"
    );
}

#[tokio::test]
async fn a_request_the_codec_cannot_read_is_a_serialization_failure() {
    let router = emulator();
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_ISO_9797_3_MAC,
        json!({"Generate": true, "Verify": true}),
    )
    .await;
    let encrypt_key = create_key(
        &router,
        "AES_128",
        USAGE_DATA_ENCRYPTION,
        json!({"Encrypt": true, "Decrypt": true}),
    )
    .await;
    let encrypt_uri = format!("/keys/{}/encrypt", encoded(&encrypt_key.arn));
    let decrypt_uri = format!("/keys/{}/decrypt", encoded(&encrypt_key.arn));

    // One row per JSON type a member is NOT modelled as — a number, an array, an object
    // and a boolean where a string belongs, a string where an integer belongs, a scalar
    // where a union belongs — plus a body that is not even the document the protocol
    // expects. None of these breaks a modelled rule: nothing of them ever became a value
    // of a modelled shape.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "EncryptData with a number for PlainText",
            encrypt_uri.as_str(),
            json!({"PlainText": 1234, "EncryptionAttributes": {"Symmetric": {"Mode": "CBC"}}})
                .to_string(),
        ),
        (
            "EncryptData with a boolean for PlainText",
            encrypt_uri.as_str(),
            json!({"PlainText": true, "EncryptionAttributes": {"Symmetric": {"Mode": "CBC"}}})
                .to_string(),
        ),
        (
            "DecryptData with an array for CipherText",
            decrypt_uri.as_str(),
            json!({
                "CipherText": ["00112233"],
                "DecryptionAttributes": {"Symmetric": {"Mode": "CBC"}},
            })
            .to_string(),
        ),
        (
            "GenerateMac with an object for MessageData",
            "/mac/generate",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": {"hex": "0011"},
                "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
            })
            .to_string(),
        ),
        (
            "GenerateMac with a string for MacLength",
            "/mac/generate",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": "ISO9797_ALGORITHM1"},
                "MacLength": "8",
            })
            .to_string(),
        ),
        (
            "VerifyMac with a scalar for VerificationAttributes",
            "/mac/verify",
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "Mac": "00112233",
                "VerificationAttributes": "ISO9797_ALGORITHM1",
            })
            .to_string(),
        ),
        (
            "GenerateMac with a body that is not a JSON object",
            "/mac/generate",
            "[\"GenerateMac\"]".to_string(),
        ),
    ];

    for (case, uri, body) in rows {
        let answer = data(&router, uri, &body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "SerializationException"),
            "case {case}: a request the codec cannot read must be announced as a \
             serialization failure, got {answer:?}"
        );
        assert_ne!(
            answer.error_type, "ValidationException",
            "case {case}: a serialization failure must not be announced as the modelled \
             ValidationException, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_failed_verification_states_the_class_that_failed() {
    let router = emulator();
    // Every MAC the key's own algorithm can compute, each verified against a message it
    // was not computed over: one sentence for the whole MAC class, whatever the key,
    // the algorithm or the length.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("TDES_2KEY", USAGE_ISO_9797_3_MAC, "ISO9797_ALGORITHM1"),
        ("TDES_3KEY", USAGE_ISO_9797_3_MAC, "ISO9797_ALGORITHM3"),
        ("AES_128", USAGE_CMAC, "CMAC"),
        ("TDES_2KEY", USAGE_CMAC, "CMAC"),
        ("HMAC_SHA256", USAGE_HMAC, "HMAC"),
        ("HMAC_SHA512", USAGE_HMAC, "HMAC_SHA512"),
    ];
    for (key_algorithm, key_usage, algorithm) in rows {
        let case = format!("{key_algorithm}/{algorithm}");
        let key = create_key(
            &router,
            key_algorithm,
            key_usage,
            json!({"Generate": true, "Verify": true}),
        )
        .await;
        let message = hex_data(16);
        let generated = data_json(
            &router,
            "/mac/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "MessageData": message,
                "GenerationAttributes": {"Algorithm": algorithm},
            }),
        )
        .await;
        assert_eq!(
            generated.status, 200,
            "case {case}: GenerateMac must answer a MAC, got {generated:?}"
        );
        let mac = generated.body["Mac"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: GenerateMac answered no Mac: {generated:?}"))
            .to_string();
        // The same MAC over a message it was not computed over cannot verify.
        let answer = data_json(
            &router,
            "/mac/verify",
            &json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(24),
                "Mac": mac,
                "VerificationAttributes": {"Algorithm": algorithm},
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "VerificationFailedException"),
            "case {case}: expected a 400 VerificationFailedException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            "Mac verification failed.",
            "case {case}: a failed MAC verification states the MAC class, got {answer:?}"
        );
        assert_eq!(
            answer.body["Reason"].as_str().unwrap_or_default(),
            "INVALID_MAC",
            "case {case}: the refusal must name the modelled reason, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_failed_card_validation_states_its_own_class() {
    let router = emulator();
    let key = create_key(
        &router,
        "TDES_2KEY",
        USAGE_CARD_VERIFICATION,
        json!({"Generate": true, "Verify": true}),
    )
    .await;
    // Both card verification attribute sets this service computes, at each published
    // bound of `ValidationDataLength`: one sentence for the whole card-validation class,
    // and a different one from the MAC class above.
    let rows: Vec<(&str, Value, i32)> = vec![
        (
            "CardVerificationValue1",
            json!({"CardVerificationValue1": {"CardExpiryDate": "0123", "ServiceCode": "101"}}),
            3,
        ),
        (
            "CardVerificationValue2",
            json!({"CardVerificationValue2": {"CardExpiryDate": "0123"}}),
            5,
        ),
    ];
    for (case, attributes, length) in rows {
        let pan = "171234567890123";
        let generated = data_json(
            &router,
            "/cardvalidationdata/generate",
            &json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": pan,
                "GenerationAttributes": attributes,
                "ValidationDataLength": length,
            }),
        )
        .await;
        assert_eq!(
            generated.status, 200,
            "case {case}: GenerateCardValidationData must answer a value, got {generated:?}"
        );
        let value = generated.body["ValidationData"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no ValidationData: {generated:?}"))
            .to_string();
        // Another value of the same length. It differs from the one this key computes for
        // this card data — which is the value just read back — so the verification fails
        // for certain rather than by chance.
        let presented: String = value
            .chars()
            .map(|digit| if digit == '0' { '1' } else { '0' })
            .collect();
        let answer = data_json(
            &router,
            "/cardvalidationdata/verify",
            &json!({
                "KeyIdentifier": key.arn,
                "PrimaryAccountNumber": pan,
                "ValidationData": presented,
                "VerificationAttributes": attributes,
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "VerificationFailedException"),
            "case {case}: expected a 400 VerificationFailedException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            "Card validation data verification failed.",
            "case {case}: a failed card validation states its own class, got {answer:?}"
        );
        assert_eq!(
            answer.body["Reason"].as_str().unwrap_or_default(),
            "INVALID_VALIDATION_DATA",
            "case {case}: the refusal must name the modelled reason, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_mac_key_of_the_wrong_algorithm_is_refused_before_its_modes_of_use() {
    let router = emulator();
    // Each row is a key whose own algorithm cannot compute the requested MAC algorithm.
    // The first four also LACK the mode of use the operation needs, which is the case
    // that decides the precedence: the refusal names the key, not the mode.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "TDES_2KEY",
            USAGE_DATA_ENCRYPTION,
            json!({"Encrypt": true, "Decrypt": true}),
            "HMAC",
        ),
        (
            "AES_128",
            USAGE_DATA_ENCRYPTION,
            json!({"Encrypt": true}),
            "HMAC_SHA256",
        ),
        (
            "HMAC_SHA256",
            USAGE_HMAC,
            json!({"Decrypt": true}),
            "CMAC",
        ),
        (
            "TDES_3KEY",
            USAGE_ISO_9797_3_MAC,
            json!({"Decrypt": true}),
            "HMAC_SHA512",
        ),
        (
            "HMAC_SHA256",
            USAGE_HMAC,
            json!({"Generate": true, "Verify": true}),
            "HMAC_SHA384",
        ),
        (
            "AES_192",
            USAGE_CMAC,
            json!({"NoRestrictions": true}),
            "ISO9797_ALGORITHM1",
        ),
    ];
    for (key_algorithm, key_usage, modes, algorithm) in rows {
        let key = create_key(&router, key_algorithm, key_usage, modes).await;
        let expected = format!(
            "Invalid key with KeyAlgorithm {algorithm} used with MacAlgorithm \
             {key_algorithm}. Please use valid {algorithm} Key."
        );
        for (operation, uri, body) in [
            (
                "GenerateMac",
                "/mac/generate",
                json!({
                    "KeyIdentifier": key.arn,
                    "MessageData": hex_data(8),
                    "GenerationAttributes": {"Algorithm": algorithm},
                }),
            ),
            (
                "VerifyMac",
                "/mac/verify",
                json!({
                    "KeyIdentifier": key.arn,
                    "MessageData": hex_data(8),
                    "Mac": "00112233",
                    "VerificationAttributes": {"Algorithm": algorithm},
                }),
            ),
        ] {
            let case = format!("{operation} {key_algorithm}/{algorithm}");
            let answer = data_json(&router, uri, &body).await;
            assert_eq!(
                (answer.status, answer.code()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case}: expected the key-algorithm refusal, got {answer:?}"
            );
            assert!(
                !answer.message().contains("KeyModesOfUse"),
                "case {case}: the refusal must name the key's algorithm rather than a \
                 mode of use, got {answer:?}"
            );
            assert!(
                !answer.message().contains(&key.arn),
                "case {case}: the refusal must not carry the resolved key ARN, \
                 got {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_mac_key_the_algorithm_admits_is_still_judged_by_its_modes_of_use() {
    let router = emulator();
    // The complement of the precedence above: when the key's algorithm CAN compute the
    // requested MAC, the modes-of-use gate is what refuses it, and it names the mode the
    // operation needs.
    let rows: Vec<(&str, &str, &str, Value, &str, &str)> = vec![
        (
            "GenerateMac",
            "/mac/generate",
            "TDES_2KEY",
            json!({"Verify": true}),
            "ISO9797_ALGORITHM1",
            "Generate",
        ),
        (
            "VerifyMac",
            "/mac/verify",
            "TDES_2KEY",
            json!({"Generate": true}),
            "ISO9797_ALGORITHM1",
            "Verify",
        ),
        (
            "GenerateMac",
            "/mac/generate",
            "HMAC_SHA256",
            json!({"Verify": true}),
            "HMAC",
            "Generate",
        ),
    ];
    for (operation, uri, key_algorithm, modes, algorithm, mode) in rows {
        let usage = if key_algorithm.starts_with("HMAC") {
            USAGE_HMAC
        } else {
            USAGE_ISO_9797_3_MAC
        };
        let key = create_key(&router, key_algorithm, usage, modes).await;
        let case = format!("{operation} {key_algorithm}/{algorithm} without {mode}");
        let body = if operation == "GenerateMac" {
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "GenerationAttributes": {"Algorithm": algorithm},
            })
        } else {
            json!({
                "KeyIdentifier": key.arn,
                "MessageData": hex_data(8),
                "Mac": "00112233",
                "VerificationAttributes": {"Algorithm": algorithm},
            })
        };
        let answer = data_json(&router, uri, &body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException"),
            "case {case}: expected a 400 ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!(
                "The KeyModesOfUse of key {} do not include {mode}.",
                key.arn
            ),
            "case {case}: expected the modes-of-use refusal, got {answer:?}"
        );
    }
}
