// SPDX-License-Identifier: CC0-1.0

//! Pointing a secret at another KMS key RE-ENCRYPTS the versions the service's own
//! staging labels reach, so the key the secret came from is no longer consulted by
//! any read of them.
//!
//! The rule these tables assert: `KmsKeyId` is "the KMS key that Secrets Manager
//! uses to encrypt new secret versions as well as any existing versions with the
//! staging labels AWSCURRENT, AWSPENDING, or AWSPREVIOUS"
//! (`UpdateSecretRequest$KmsKeyId`, this service's Smithy model), so an update that
//! names another key opens each version those labels reach and seals it again under
//! the named key, keeping that version's id, its labels and its plaintext and
//! minting no version at all. What follows from it, and is what a key retirement
//! depends on: once the key change is made, the key the versions came from can be
//! disabled and every one of those versions still reads back, while a version
//! OUTSIDE those labels keeps the key its own data key was wrapped by, and only the
//! key the secret now names refuses the read when it is the unusable one.
//!
//! The rows span what the rule generalises over rather than one recorded request:
//! the FORM the update names the new key in (key ARN, bare key id, alias name,
//! alias ARN — "the ARN, key ID, or alias of the KMS key"), the member and size of
//! the value each version holds (a short string, a JSON document, a long string, a
//! binary blob, a one-byte blob), the LABEL a version carries (each of the three the
//! service moves, a caller's own label, and none at all), and the DIRECTION the
//! change goes in (from a customer managed key, and from the Amazon Web Services
//! managed key, which holds its values as sent).
//!
//! KMS is mounted beside Secrets Manager on ONE engine, so every key below is a real
//! key minted through KMS's own operations and every state change is made through
//! them too; the harness is the one [`crate::secret_kms_key_state`] builds.

use serde_json::{json, Value};

use crate::secret_kms_key_state::{
    assert_relays_kms, create_body, value_body, version_count, version_id_of, Emulator,
    DECRYPTION_SENTENCE,
};
use crate::secret_lifecycle::ValueCase;

/// The state of a key KMS will not perform a cryptographic operation with that these
/// cases retire a key by, and the sentence KMS refuses such a call with — what the
/// read refusal below relays.
const DISABLED_CODE: &str = "DisabledException";
const DISABLED_PHRASE: &str = "is disabled.";

/// The three staging labels the service itself moves, which are the versions a key
/// change re-encrypts (`UpdateSecretRequest$KmsKeyId`, this service's Smithy model).
const AWSCURRENT: &str = "AWSCURRENT";
const AWSPREVIOUS: &str = "AWSPREVIOUS";
const AWSPENDING: &str = "AWSPENDING";

/// Which form of `KmsKeyId` an update names the new key in: "the ARN, key ID, or
/// alias of the KMS key" (`UpdateSecretRequest$KmsKeyId`, this service's Smithy
/// model), with an alias in both spellings KMS accepts for one.
#[derive(Clone, Copy, Debug)]
enum KeyForm {
    KeyArn,
    KeyId,
    AliasName,
    AliasArn,
}

/// Every form, with the label each case reports itself under.
const KEY_FORMS: [(&str, KeyForm); 4] = [
    ("the key ARN", KeyForm::KeyArn),
    ("the bare key id", KeyForm::KeyId),
    ("the alias name", KeyForm::AliasName),
    ("the alias ARN", KeyForm::AliasArn),
];

/// One row of the value table: what the three labelled versions hold, which the
/// envelope is the same for and so has to survive the key change unchanged.
struct ValueRow {
    label: &'static str,
    previous: ValueCase,
    current: ValueCase,
    pending: ValueCase,
}

/// The value table: the member and the size of what each version holds.
fn value_rows() -> Vec<ValueRow> {
    vec![
        ValueRow {
            label: "short strings",
            previous: ValueCase::string("the superseded value", "value-under-the-first-key"),
            current: ValueCase::string("the current value", "s3cr3t"),
            pending: ValueCase::string("the candidate value", "candidate"),
        },
        ValueRow {
            label: "a JSON document and a long string",
            previous: ValueCase::string("the superseded value", "value-under-the-first-key"),
            current: ValueCase::string(
                "a JSON document",
                json!({"username": "admin", "password": "p@ssw0rd", "host": "db.internal"})
                    .to_string(),
            ),
            // Larger than one AES block and not a multiple of it, so the ciphertext
            // is not a single block however the AEAD pads.
            pending: ValueCase::string("a long string", "x".repeat(1_000)),
        },
        ValueRow {
            label: "binary blobs",
            previous: ValueCase::string("the superseded value", "value-under-the-first-key"),
            current: ValueCase::binary("a binary blob", (0u8..=255).cycle().take(300).collect()),
            pending: ValueCase::binary("a one-byte blob", vec![0x00]),
        },
    ]
}

#[tokio::test]
async fn a_key_change_rekeys_every_staged_version_whatever_form_names_the_new_key() {
    for (form_index, (form_label, form)) in KEY_FORMS.into_iter().enumerate() {
        for (row_index, row) in value_rows().into_iter().enumerate() {
            let case = format!("named by {form_label}, holding {}", row.label);
            let emulator = Emulator::new();
            let (first_key, _) = emulator.create_key(&case).await;
            let (second_key, second_key_id) = emulator.create_key(&case).await;
            let alias = format!("alias/rekeyed-{form_index}-{row_index}");
            emulator
                .kms_ok(
                    "CreateAlias",
                    &json!({"AliasName": alias.clone(), "TargetKeyId": second_key_id.clone()}),
                    &case,
                )
                .await;
            let named_key = match form {
                KeyForm::KeyArn => second_key.clone(),
                KeyForm::KeyId => second_key_id.clone(),
                KeyForm::AliasName => alias.clone(),
                KeyForm::AliasArn => alias_arn_of(&second_key, &alias, &case),
            };
            let name = format!("rekeyed-{form_index}-{row_index}");

            // Three versions under the FIRST key, one for each label the service
            // moves: AWSPREVIOUS and AWSCURRENT from two writes of a value, and
            // AWSPENDING from a write that stakes the label a rotation stages.
            let previous_id = version_id_of(
                &emulator
                    .sm_ok(
                        "CreateSecret",
                        &create_body(&name, Some(&first_key), &row.previous),
                        &case,
                    )
                    .await,
                &case,
            );
            let current_id = version_id_of(
                &emulator
                    .sm_ok("PutSecretValue", &value_body(&name, &row.current, &[]), &case)
                    .await,
                &case,
            );
            let pending_id = version_id_of(
                &emulator
                    .sm_ok(
                        "PutSecretValue",
                        &value_body(&name, &row.pending, &[AWSPENDING]),
                        &case,
                    )
                    .await,
                &case,
            );
            let staged_before = emulator
                .sm_ok("DescribeSecret", &json!({"SecretId": name.clone()}), &case)
                .await["VersionIdsToStages"]
                .clone();
            let count_before = version_count(&emulator, &name, &case).await;

            // The key change itself, carrying no value at all.
            let updated = emulator
                .sm_ok(
                    "UpdateSecret",
                    &json!({"SecretId": name.clone(), "KmsKeyId": named_key.clone()}),
                    &case,
                )
                .await;
            // "If Secrets Manager created a new version of the secret during this
            // operation, then VersionId contains the unique identifier of the new
            // version." (`UpdateSecretResponse$VersionId`, this service's Smithy
            // model.) A key change stores no value, so it creates none.
            assert!(
                updated.get("VersionId").is_none_or(Value::is_null),
                "case {case:?}: expected the key change to create no version, got {updated}"
            );
            let described = emulator
                .sm_ok("DescribeSecret", &json!({"SecretId": name.clone()}), &case)
                .await;
            assert_eq!(
                described.get("KmsKeyId").and_then(Value::as_str),
                Some(named_key.as_str()),
                "case {case:?}: expected the secret to name the key it was pointed at, got \
                 {described}"
            );
            assert_eq!(
                described["VersionIdsToStages"], staged_before,
                "case {case:?}: expected the key change to disturb no staging label, got \
                 {described}"
            );
            assert_eq!(
                version_count(&emulator, &name, &case).await,
                count_before,
                "case {case:?}: expected the key change to mint no version"
            );

            // With the key the versions CAME FROM disabled, every one of them still
            // reads back: the change moved them off that key, so no read consults it.
            emulator
                .kms_ok("DisableKey", &json!({"KeyId": first_key.clone()}), &case)
                .await;
            for (version_id, value) in [
                (&previous_id, &row.previous),
                (&current_id, &row.current),
                (&pending_id, &row.pending),
            ] {
                value.assert_round_trip(
                    &emulator
                        .sm_ok(
                            "GetSecretValue",
                            &json!({"SecretId": name.clone(), "VersionId": version_id.clone()}),
                            &case,
                        )
                        .await,
                    &case,
                );
            }
            for (stage, value) in [
                (AWSCURRENT, &row.current),
                (AWSPREVIOUS, &row.previous),
                (AWSPENDING, &row.pending),
            ] {
                value.assert_round_trip(
                    &emulator
                        .sm_ok(
                            "GetSecretValue",
                            &json!({"SecretId": name.clone(), "VersionStage": stage}),
                            &case,
                        )
                        .await,
                    &case,
                );
            }

            // And the key the versions moved TO is the one their reads now depend
            // on: it is the only key whose state refuses them, which is the one case
            // this error class is correct here.
            emulator
                .kms_ok("DisableKey", &json!({"KeyId": second_key.clone()}), &case)
                .await;
            for version_id in [&previous_id, &current_id, &pending_id] {
                let (code, message) = emulator
                    .sm_err(
                        "GetSecretValue",
                        &json!({"SecretId": name.clone(), "VersionId": version_id.clone()}),
                        &case,
                    )
                    .await;
                assert_eq!(
                    code, "DecryptionFailure",
                    "case {case:?}: expected the read of {version_id} to depend on the new key, \
                     got {code} with {message:?}"
                );
                assert_relays_kms(
                    &message,
                    DECRYPTION_SENTENCE,
                    DISABLED_CODE,
                    DISABLED_PHRASE,
                    &case,
                );
                assert!(
                    message.contains(&second_key),
                    "case {case:?}: expected the refusal to relay a refusal of the new key \
                     {second_key}, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_key_change_leaves_a_version_outside_those_labels_under_the_key_holding_it() {
    // The labels the service moves are named ones: "any existing versions with the
    // staging labels AWSCURRENT, AWSPENDING, or AWSPREVIOUS"
    // (`UpdateSecretRequest$KmsKeyId`, this service's Smithy model). A version
    // carrying something else, or nothing, is not among them and keeps the key its
    // own data key was wrapped by — so the key change retires the old key only for
    // the versions the labels reach.
    //
    // The rows are the two ways a version sits outside those labels: a caller's own
    // label, and no label at all.
    for (row_index, (label_form, caller_label)) in [
        ("a caller's own label", Some("ANALYTICS")),
        ("no label at all", None),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("left behind: {label_form}");
        let emulator = Emulator::new();
        let (first_key, _) = emulator.create_key(&case).await;
        let (second_key, _) = emulator.create_key(&case).await;
        let name = format!("left-behind-{row_index}");

        let kept = ValueCase::string("the value outside those labels", "kept-under-the-first-key");
        let left_out_id = match caller_label {
            // A write that stakes only a caller's label leaves that version with no
            // label the service moves.
            Some(caller_label) => {
                emulator
                    .sm_ok(
                        "CreateSecret",
                        &create_body(
                            &name,
                            Some(&first_key),
                            &ValueCase::string("the first value", "value-one"),
                        ),
                        &case,
                    )
                    .await;
                version_id_of(
                    &emulator
                        .sm_ok(
                            "PutSecretValue",
                            &value_body(&name, &kept, &[caller_label]),
                            &case,
                        )
                        .await,
                    &case,
                )
            }
            // Three writes leave the first version deprecated: AWSCURRENT on the
            // third, AWSPREVIOUS on the second, no label on the first.
            None => {
                let created = version_id_of(
                    &emulator
                        .sm_ok(
                            "CreateSecret",
                            &create_body(&name, Some(&first_key), &kept),
                            &case,
                        )
                        .await,
                    &case,
                );
                for value in [
                    ValueCase::string("the second value", "value-two"),
                    ValueCase::string("the third value", "value-three"),
                ] {
                    emulator
                        .sm_ok("PutSecretValue", &value_body(&name, &value, &[]), &case)
                        .await;
                }
                created
            }
        };

        emulator
            .sm_ok(
                "UpdateSecret",
                &json!({"SecretId": name.clone(), "KmsKeyId": second_key.clone()}),
                &case,
            )
            .await;
        emulator
            .kms_ok("DisableKey", &json!({"KeyId": first_key.clone()}), &case)
            .await;

        // The version the labels reach moved to the new key and reads back; the one
        // outside them did not, so the key holding it decides its read.
        emulator
            .sm_ok(
                "GetSecretValue",
                &json!({"SecretId": name.clone(), "VersionStage": AWSCURRENT}),
                &case,
            )
            .await;
        let (code, message) = emulator
            .sm_err(
                "GetSecretValue",
                &json!({"SecretId": name.clone(), "VersionId": left_out_id.clone()}),
                &case,
            )
            .await;
        assert_eq!(
            code, "DecryptionFailure",
            "case {case:?}: expected the version outside those labels to stay under the first \
             key, got {code} with {message:?}"
        );
        assert_relays_kms(
            &message,
            DECRYPTION_SENTENCE,
            DISABLED_CODE,
            DISABLED_PHRASE,
            &case,
        );
        assert!(
            message.contains(&first_key),
            "case {case:?}: expected the refusal to relay a refusal of the first key \
             {first_key}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_key_change_onto_a_customer_key_seals_the_staged_versions_under_it() {
    // The change moves the labelled versions ONTO the named key whatever they were
    // held as, so a secret on the Amazon Web Services managed key — which holds its
    // values as sent, since that key is one no caller can act on — ends up with its
    // labelled versions sealed under the customer managed key the update names: the
    // reads depend on that key from then on, and recover with it.
    //
    // The rows span the member and size of the value the versions hold.
    for (row_index, row) in value_rows().into_iter().enumerate() {
        let case = format!("onto a customer managed key, holding {}", row.label);
        let emulator = Emulator::new();
        let (key_arn, _) = emulator.create_key(&case).await;
        let name = format!("onto-a-key-{row_index}");

        emulator
            .sm_ok("CreateSecret", &create_body(&name, None, &row.previous), &case)
            .await;
        let current_id = version_id_of(
            &emulator
                .sm_ok("PutSecretValue", &value_body(&name, &row.current, &[]), &case)
                .await,
            &case,
        );
        emulator
            .sm_ok(
                "UpdateSecret",
                &json!({"SecretId": name.clone(), "KmsKeyId": key_arn.clone()}),
                &case,
            )
            .await;

        // Sealed under that key now: its state decides the read.
        emulator
            .kms_ok("DisableKey", &json!({"KeyId": key_arn.clone()}), &case)
            .await;
        let (code, message) = emulator
            .sm_err("GetSecretValue", &json!({"SecretId": name.clone()}), &case)
            .await;
        assert_eq!(
            code, "DecryptionFailure",
            "case {case:?}: expected the read to depend on the key the versions moved to, got \
             {code} with {message:?}"
        );
        assert_relays_kms(
            &message,
            DECRYPTION_SENTENCE,
            DISABLED_CODE,
            DISABLED_PHRASE,
            &case,
        );
        assert!(
            message.contains(&key_arn),
            "case {case:?}: expected the refusal to relay a refusal of {key_arn}, got {message:?}"
        );

        // And the bytes are the ones the caller stored: only the key they are held
        // under changed.
        emulator
            .kms_ok("EnableKey", &json!({"KeyId": key_arn.clone()}), &case)
            .await;
        row.current.assert_round_trip(
            &emulator
                .sm_ok(
                    "GetSecretValue",
                    &json!({"SecretId": name.clone(), "VersionId": current_id.clone()}),
                    &case,
                )
                .await,
            &case,
        );
        row.previous.assert_round_trip(
            &emulator
                .sm_ok(
                    "GetSecretValue",
                    &json!({"SecretId": name.clone(), "VersionStage": AWSPREVIOUS}),
                    &case,
                )
                .await,
            &case,
        );
    }
}

/// The ARN of `alias` at the endpoint `key_arn` names: an alias ARN is the key ARN's
/// own account and region with the alias as its resource
/// (`arn:aws:kms:<region>:<account>:alias/<name>`).
fn alias_arn_of(key_arn: &str, alias: &str, case: &str) -> String {
    let (account_scope, _) = key_arn
        .split_once(":key/")
        .unwrap_or_else(|| panic!("case {case:?}: {key_arn} is not a key ARN"));
    format!("{account_scope}:{alias}")
}
