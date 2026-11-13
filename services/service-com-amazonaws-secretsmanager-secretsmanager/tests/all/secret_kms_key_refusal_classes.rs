// SPDX-License-Identifier: CC0-1.0

//! WHICH error class a write under a customer managed KMS key is refused with, when
//! the key cannot be used.
//!
//! The rule these tables assert: a KMS refusal is classified by WHY the key could not
//! be used, which is the error code KMS answered under, and not collapsed into one
//! class —
//!
//! * a `KmsKeyId` the request NAMED that resolves to no key is a fault of the value
//!   sent, so the call is refused `InvalidParameterException` ("The parameter name or
//!   value is invalid.", this service's Smithy model) carrying this service's clause
//!   for an invalid KMS key and the KMS refusal that says what is wrong with the
//!   identifier;
//! * a key that EXISTS and will not perform the cryptographic operation is a
//!   prerequisite of CREATING a secret under it that the account did not complete, so
//!   the create is refused `PreconditionNotMetException` ("The request failed because
//!   you did not complete all the prerequisite steps.");
//! * nothing is stored either way, and the identical call succeeds once the caller
//!   puts right what its class named — a key that exists, or a key brought back into a
//!   state that encrypts.
//!
//! The rows span what the rule generalises over rather than one recorded request: the
//! FORM the unresolvable identifier takes (a key ARN, a bare key id, a multi-Region
//! key id, an alias, a value that is no key identifier at all), the member the value
//! travels in, the KIND of key failure, and the OPERATION that names the key
//! (`CreateSecret`, `UpdateSecret`).
//!
//! KMS is mounted beside Secrets Manager on ONE engine, so every key below is a real
//! key minted through KMS's own operations and every state change is made through
//! them too.

use serde_json::{json, Map, Value};

use crate::secret_kms_key_state::{create_body, version_count, Emulator};
use crate::secret_lifecycle::ValueCase;

/// The region every request in this suite signs for, which is the one the keys below
/// are minted in: Secrets Manager calls the key in the Region the request was
/// addressed to.
const REGION: &str = "us-west-2";

/// How a refusal of the KEY A REQUEST NAMED introduces the KMS refusal it relays: this
/// service states that the KMS key the request carried is not a valid one, and then
/// relays the answer KMS gave about that identifier.
const INVALID_KEY_SENTENCE: &str = "The operation failed because of an invalid KMS key:";

/// The mirror clause for a key that exists and would not encrypt, which states what
/// this service could not do with it rather than anything about the identifier.
const ENCRYPTION_SENTENCE: &str = "Secrets Manager can't encrypt the secret value:";

/// What the relayed clause states about the wrapped KMS refusal beside its message:
/// the service that answered and the status its class is answered with, before the
/// code it was stated under.
const RELAY_FIELDS: &str = "(Service: AWSKMS; Status Code: 400; Error Code: ";

/// How the relayed clause ends: the identifier of the answer it relays, then the proxy
/// field, which no in-process answer came through.
const RELAY_TAIL: &str = "; Proxy: null)";

/// The code KMS answers an identifier that resolves to no key under: "The request was
/// rejected because the specified entity or resource could not be found"
/// (`NotFoundException`, the KMS model), which every cryptographic operation models.
const KEY_NOT_FOUND_CODE: &str = "NotFoundException";

/// What this service answers a read of a secret it does not hold, which is what a
/// refused create must leave behind.
const SECRET_NOT_FOUND_MESSAGE: &str = "Secrets Manager can't find the specified secret.";

/// One row of the identifier table: a `KmsKeyId` in a form KMS accepts that resolves
/// to no key at this endpoint, and the phrase KMS's own refusal of that form carries.
struct UnresolvableKey {
    case: &'static str,
    /// Built per row, because a key ARN names the account the endpoint answers for.
    identifier: String,
    /// The wording KMS answers this FORM with: a well-formed key id is reported by the
    /// key ARN it would have, an alias by its own ARN, and a value that is no key
    /// identifier by the value itself.
    kms_phrase: String,
}

/// The identifier forms a `KmsKeyId` can take while naming no key: "the ARN, key ID,
/// or alias of the KMS key" (`CreateSecretRequest$KmsKeyId`, this service's Smithy
/// model), each in a spelling KMS parses and resolves to nothing.
fn unresolvable_keys() -> Vec<UnresolvableKey> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // A key id of the two forms KMS issues — the hyphenated UUID of a single-Region
    // key and the `mrk-` form of a multi-Region one — that no CreateKey here minted.
    let single_region = "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d";
    let multi_region = "mrk-0123456789abcdef0123456789abcdef";
    vec![
        UnresolvableKey {
            case: "a key ARN for a key id nothing was minted under",
            identifier: format!("arn:aws:kms:{REGION}:{account}:key/{single_region}"),
            kms_phrase: format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/{single_region}' does not exist"
            ),
        },
        UnresolvableKey {
            case: "the all-zero key id, as an ARN",
            identifier: format!(
                "arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"
            ),
            kms_phrase: format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/\
                 00000000-0000-0000-0000-000000000000' does not exist"
            ),
        },
        UnresolvableKey {
            case: "a bare key id nothing was minted under",
            identifier: single_region.to_string(),
            kms_phrase: format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/{single_region}' does not exist"
            ),
        },
        UnresolvableKey {
            case: "a multi-Region key id nothing was minted under",
            identifier: multi_region.to_string(),
            kms_phrase: format!(
                "Key 'arn:aws:kms:{REGION}:{account}:key/{multi_region}' does not exist"
            ),
        },
        UnresolvableKey {
            case: "an alias no key carries",
            identifier: "alias/no-key-carries-this".to_string(),
            kms_phrase: format!(
                "Alias arn:aws:kms:{REGION}:{account}:alias/no-key-carries-this is not found."
            ),
        },
        UnresolvableKey {
            case: "an alias ARN no key carries",
            identifier: format!("arn:aws:kms:{REGION}:{account}:alias/no-key-carries-this"),
            kms_phrase: format!(
                "Alias arn:aws:kms:{REGION}:{account}:alias/no-key-carries-this is not found."
            ),
        },
        UnresolvableKey {
            case: "a value that is no key identifier at all",
            identifier: "not-a-key-identifier".to_string(),
            kms_phrase: "Invalid keyId 'not-a-key-identifier'".to_string(),
        },
    ]
}

/// The value table: the member a caller stores the value in, which the key is asked
/// about the same way for.
fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase::string("a string value", "s3cr3t"),
        ValueCase::binary("a binary value", vec![0x01, 0x02, 0x03, 0xff]),
    ]
}

/// Assert a refusal is worded as this class is: this service's own clause, then the
/// wrapped KMS refusal — the message KMS stated, the code it stated it under, and the
/// fields that name the answer it came from.
fn assert_relays_kms(message: &str, sentence: &str, kms_code: &str, kms_phrase: &str, case: &str) {
    assert!(
        message.starts_with(sentence),
        "case {case:?}: expected the message to start {sentence:?}, got {message:?}"
    );
    let clause = format!("{RELAY_FIELDS}{kms_code}; Request ID: ");
    assert!(
        message.contains(&clause),
        "case {case:?}: expected the relayed clause {clause:?} in the message, got {message:?}"
    );
    assert!(
        message.contains(kms_phrase),
        "case {case:?}: expected the wrapped KMS refusal {kms_phrase:?} in the message, got \
         {message:?}"
    );
    // The clause ends with the identifier of the answer it relays, which is one
    // answer's own value and so is checked by its FORM: a non-empty identifier, and
    // nothing of the clause's own punctuation inside it.
    let request_id = message
        .split_once(&clause)
        .and_then(|(_, tail)| tail.strip_suffix(RELAY_TAIL))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the message to end {RELAY_TAIL:?}, got {message:?}")
        });
    assert!(
        !request_id.is_empty() && !request_id.contains(';') && !request_id.contains(' '),
        "case {case:?}: expected the relayed answer's identifier, got {request_id:?} in \
         {message:?}"
    );
}

/// Assert this service holds no secret under `name`, which is what a refused create
/// leaves behind.
async fn assert_no_secret(emulator: &Emulator, name: &str, case: &str) {
    let (status, parsed) = emulator
        .sm("DescribeSecret", &json!({"SecretId": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the refused create to store nothing, got {status} with {parsed}"
    );
    assert_eq!(
        parsed["__type"]
            .as_str()
            .and_then(|raw| raw.split('#').next_back()),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected the refused create to store nothing, got {parsed}"
    );
    assert_eq!(
        parsed["Message"].as_str().or(parsed["message"].as_str()),
        Some(SECRET_NOT_FOUND_MESSAGE),
        "case {case:?}: expected this service's own sentence for a secret it does not hold, \
         got {parsed}"
    );
}

#[tokio::test]
async fn a_create_naming_a_key_that_resolves_to_nothing_is_an_invalid_parameter() {
    for (form_index, key) in unresolvable_keys().iter().enumerate() {
        for (index, value) in value_cases().iter().enumerate() {
            let emulator = Emulator::new();
            let case = format!("{}, {}", key.case, value.label);
            let name = format!("unresolvable-{form_index}-{index}");

            let (code, message) = emulator
                .sm_err(
                    "CreateSecret",
                    &create_body(&name, Some(&key.identifier), value),
                    &case,
                )
                .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code} with {message:?}"
            );
            assert_relays_kms(
                &message,
                INVALID_KEY_SENTENCE,
                KEY_NOT_FOUND_CODE,
                &key.kms_phrase,
                &case,
            );
            assert_no_secret(&emulator, &name, &case).await;

            // The refusal is about the key that create named and nothing else: the
            // same name, created without naming a key, is stored and reads back.
            let managed_case = format!("{case}, on the service-managed key");
            let created = emulator
                .sm_ok("CreateSecret", &create_body(&name, None, value), &managed_case)
                .await;
            let described = emulator
                .sm_ok(
                    "DescribeSecret",
                    &json!({"SecretId": created["ARN"].as_str().unwrap_or_default()}),
                    &managed_case,
                )
                .await;
            assert_eq!(
                described["Name"].as_str(),
                Some(name.as_str()),
                "case {managed_case:?}: expected the stored secret to report its name, got \
                 {described}"
            );
            let read = emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &managed_case)
                .await;
            value.assert_round_trip(&read, &managed_case);
        }
    }
}

#[tokio::test]
async fn the_class_of_a_create_refusal_follows_the_kind_of_key_failure() {
    // Each row is a KIND of key failure, made through KMS's own operations where a key
    // is involved at all, with the class this operation models for that kind.
    //
    // A disabled key answers `DisabledException` and any other unusable state answers
    // `KMSInvalidStateException`; both are keys that EXIST, so both are prerequisites
    // of the create rather than faults of the identifier it carried.
    let rows: &[(&str, &str, Value, &str, &str)] = &[
        (
            "the key is disabled",
            "DisableKey",
            json!({}),
            "PreconditionNotMetException",
            "is disabled.",
        ),
        (
            "the key is pending deletion",
            "ScheduleKeyDeletion",
            json!({"PendingWindowInDays": 7}),
            "PreconditionNotMetException",
            "is pending deletion.",
        ),
    ];

    let value = ValueCase::string("a string value", "v1");
    for (index, (case, operation, extra, expected_code, kms_phrase)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let (key_arn, _) = emulator.create_key(case).await;
        let mut request = Map::new();
        request.insert("KeyId".to_string(), json!(key_arn));
        if let Value::Object(members) = extra.clone() {
            request.extend(members);
        }
        emulator
            .kms_ok(operation, &Value::Object(request), case)
            .await;

        let name = format!("kind-{index}");
        let (code, message) = emulator
            .sm_err(
                "CreateSecret",
                &create_body(&name, Some(&key_arn), &value),
                case,
            )
            .await;
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} with {message:?}"
        );
        // A key that exists is reported by what this service could not do with it,
        // which is the clause that relays the key's own refusal.
        assert!(
            message.starts_with(ENCRYPTION_SENTENCE) && message.contains(kms_phrase),
            "case {case:?}: expected the message to state what the key refused, got {message:?}"
        );
        assert_no_secret(&emulator, &name, case).await;
    }

    // And the same create under a key nothing is wrong with is stored: the rows above
    // are the key's own failure and nothing else.
    let emulator = Emulator::new();
    let usable_case = "the key is usable";
    let (key_arn, _) = emulator.create_key(usable_case).await;
    emulator
        .sm_ok(
            "CreateSecret",
            &create_body("kind-usable", Some(&key_arn), &value),
            usable_case,
        )
        .await;
    let read = emulator
        .sm_ok(
            "GetSecretValue",
            &json!({"SecretId": "kind-usable"}),
            usable_case,
        )
        .await;
    value.assert_round_trip(&read, usable_case);
}

#[tokio::test]
async fn the_identical_create_succeeds_once_the_key_is_prepared() {
    // The two ways a caller puts right what each class named: a key that exists for
    // the invalid identifier, and a key restored to a state that encrypts for the
    // unmet prerequisite.
    let value = ValueCase::string("a string value", "v1");
    for (index, pending_deletion) in [false, true].into_iter().enumerate() {
        let emulator = Emulator::new();
        let case = if pending_deletion {
            "a key whose scheduled deletion is cancelled"
        } else {
            "an identifier only a minted key makes resolvable"
        };
        let (key_arn, key_id) = emulator.create_key(case).await;
        let name = format!("prepared-{index}");

        if pending_deletion {
            emulator
                .kms_ok(
                    "ScheduleKeyDeletion",
                    &json!({"KeyId": &key_arn, "PendingWindowInDays": 7}),
                    case,
                )
                .await;
            let (code, message) = emulator
                .sm_err(
                    "CreateSecret",
                    &create_body(&name, Some(&key_arn), &value),
                    case,
                )
                .await;
            assert_eq!(
                code, "PreconditionNotMetException",
                "case {case:?}: expected PreconditionNotMetException, got {code} with {message:?}"
            );
            assert_no_secret(&emulator, &name, case).await;
            emulator
                .kms_ok("CancelKeyDeletion", &json!({"KeyId": &key_arn}), case)
                .await;
            emulator
                .kms_ok("EnableKey", &json!({"KeyId": &key_arn}), case)
                .await;
        } else {
            // An alias that carries no key yet names no key: the create is refused for
            // the identifier it carried, and the identical create is stored once the
            // alias points at a key.
            let alias = format!("alias/prepared-{index}");
            let (code, message) = emulator
                .sm_err(
                    "CreateSecret",
                    &create_body(&name, Some(&alias), &value),
                    case,
                )
                .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code} with {message:?}"
            );
            assert_no_secret(&emulator, &name, case).await;
            emulator
                .kms_ok(
                    "CreateAlias",
                    &json!({"AliasName": &alias, "TargetKeyId": &key_id}),
                    case,
                )
                .await;
            let created = emulator
                .sm_ok(
                    "CreateSecret",
                    &create_body(&name, Some(&alias), &value),
                    case,
                )
                .await;
            let read = emulator
                .sm_ok(
                    "GetSecretValue",
                    &json!({"SecretId": created["ARN"].as_str().unwrap_or_default()}),
                    case,
                )
                .await;
            value.assert_round_trip(&read, case);
            continue;
        }

        let created = emulator
            .sm_ok(
                "CreateSecret",
                &create_body(&name, Some(&key_arn), &value),
                case,
            )
            .await;
        let read = emulator
            .sm_ok(
                "GetSecretValue",
                &json!({"SecretId": created["ARN"].as_str().unwrap_or_default()}),
                case,
            )
            .await;
        value.assert_round_trip(&read, case);
    }
}

#[tokio::test]
async fn an_update_naming_a_key_that_resolves_to_nothing_is_an_invalid_parameter() {
    // `UpdateSecretRequest$KmsKeyId` names a key the same way a create's does, so an
    // identifier that resolves to no key is the same fault of the value sent — and the
    // secret the update addressed is left exactly as it was.
    let stored = ValueCase::string("the stored value", "v1");
    let replacement = ValueCase::string("a replacement", "v2");
    for (index, key) in unresolvable_keys().iter().enumerate() {
        let emulator = Emulator::new();
        let case = format!("UpdateSecret naming {}", key.case);
        let name = format!("update-unresolvable-{index}");
        emulator
            .sm_ok("CreateSecret", &create_body(&name, None, &stored), &case)
            .await;
        let before = version_count(&emulator, &name, &case).await;

        let mut body = Map::new();
        body.insert("SecretId".to_string(), json!(&name));
        body.insert("KmsKeyId".to_string(), json!(&key.identifier));
        if let Value::Object(members) = replacement.wire_members() {
            body.extend(members);
        }
        let (code, message) = emulator
            .sm_err("UpdateSecret", &Value::Object(body), &case)
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code} with {message:?}"
        );
        assert_relays_kms(
            &message,
            INVALID_KEY_SENTENCE,
            KEY_NOT_FOUND_CODE,
            &key.kms_phrase,
            &case,
        );

        let read = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": &name}), &case)
            .await;
        stored.assert_round_trip(&read, &case);
        assert_eq!(
            version_count(&emulator, &name, &case).await,
            before,
            "case {case:?}: a refused update stored a version"
        );
        let described = emulator
            .sm_ok("DescribeSecret", &json!({"SecretId": &name}), &case)
            .await;
        assert_ne!(
            described["KmsKeyId"].as_str(),
            Some(key.identifier.as_str()),
            "case {case:?}: a refused update left the secret on the key it refused, got \
             {described}"
        );
    }
}
