// SPDX-License-Identifier: CC0-1.0

//! The type discriminator this service puts on a serialized `KeyModesOfUse`.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0), through the same
//! harness `key_lifecycle` uses, so what these tests assert is what a signed SDK
//! request observes.
//!
//! The rule under test: wherever a response serializes the modes of use of a key, the
//! object carries — beside the nine modelled boolean flags, and nothing else — a
//! `__type` member naming the structure the service resolves that shape to. The rule
//! is about the shape, not about one operation, so it is driven over every served
//! operation that answers with a key or a key summary, and over key families whose
//! modes of use differ in which flags they set.
//!
//! The read side is checked too: the discriminator is not a modelled member, so a
//! request that carries it back is read exactly as one that does not.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::key_lifecycle::{ok_call, router, string};

/// The structure the service resolves the modes of use to, as the discriminator on
/// every serialized `KeyModesOfUse` names it. Fixed text identifying a shape: it is
/// the same for every key, every operation and every caller.
const TYPE_DISCRIMINATOR: &str = "com.amazonaws.magnus#KeyModesOfUse";

/// The nine modelled members of `KeyModesOfUse`, in the model's member order.
///
/// Source: `smithy_query(query="shape",shape="KeyModesOfUse")` — nine
/// `PrimitiveBoolean` members, each `@default false`, so every one of them is on the
/// wire whether the request named it or not.
const MODE_MEMBERS: [&str; 9] = [
    "Encrypt",
    "Decrypt",
    "Wrap",
    "Unwrap",
    "Generate",
    "Sign",
    "Verify",
    "DeriveKey",
    "NoRestrictions",
];

/// One row of the key-family table: a key this service generates, and the modes of use
/// it is created with.
///
/// Source: `smithy_query(query="shape",shape="KeyAlgorithm,KeyClass,KeyUsage")`. The
/// rows differ in WHICH of the nine flags they set — one of each cryptographic family,
/// a single-flag row and the unrestricted row — because that is what the rule
/// generalises over; the issue that prompted this suite named only an encrypt/decrypt
/// key.
struct KeyFamily {
    key_algorithm: &'static str,
    key_class: &'static str,
    key_usage: &'static str,
    /// The flags the request sets, by modelled member name.
    set_flags: &'static [&'static str],
}

const KEY_FAMILIES: &[KeyFamily] = &[
    KeyFamily {
        key_algorithm: "TDES_3KEY",
        key_class: "SYMMETRIC_KEY",
        key_usage: "TR31_P0_PIN_ENCRYPTION_KEY",
        set_flags: &["Encrypt", "Decrypt"],
    },
    KeyFamily {
        key_algorithm: "AES_256",
        key_class: "SYMMETRIC_KEY",
        key_usage: "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
        set_flags: &["Wrap", "Unwrap"],
    },
    KeyFamily {
        key_algorithm: "HMAC_SHA256",
        key_class: "SYMMETRIC_KEY",
        key_usage: "TR31_M7_HMAC_KEY",
        set_flags: &["Generate", "Verify"],
    },
    KeyFamily {
        key_algorithm: "RSA_2048",
        key_class: "ASYMMETRIC_KEY_PAIR",
        key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
        set_flags: &["Sign", "Verify"],
    },
    KeyFamily {
        key_algorithm: "ECC_NIST_P384",
        key_class: "ASYMMETRIC_KEY_PAIR",
        key_usage: "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT",
        set_flags: &["DeriveKey"],
    },
    KeyFamily {
        key_algorithm: "AES_128",
        key_class: "SYMMETRIC_KEY",
        key_usage: "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
        set_flags: &["NoRestrictions"],
    },
];

/// Every modelled flag, mapped to whether this family sets it.
fn expected_flags(family: &KeyFamily) -> BTreeMap<&'static str, bool> {
    MODE_MEMBERS
        .iter()
        .map(|member| (*member, family.set_flags.contains(member)))
        .collect()
}

/// The `KeyModesOfUse` request member for one family: only the flags it sets, as a
/// caller writes them.
fn modes_request(family: &KeyFamily) -> Value {
    let mut modes = serde_json::Map::new();
    for flag in family.set_flags {
        modes.insert((*flag).to_string(), json!(true));
    }
    Value::Object(modes)
}

/// A `CreateKey` body for one family, with `modes` as its `KeyModesOfUse`.
fn create_body(family: &KeyFamily, modes: Value) -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": family.key_usage,
            "KeyClass": family.key_class,
            "KeyAlgorithm": family.key_algorithm,
            "KeyModesOfUse": modes,
        },
        "Exportable": true,
    })
}

/// Every `KeyModesOfUse` object the document carries, each with the JSON path it sits
/// at — so the rule is checked wherever in a response the shape appears rather than at
/// one hand-picked place.
fn modes_objects(path: &str, document: &Value) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    match document {
        Value::Object(members) => {
            for (member, value) in members {
                let child = format!("{path}.{member}");
                if member == "KeyModesOfUse" {
                    found.push((child.clone(), value.clone()));
                }
                found.extend(modes_objects(&child, value));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                found.extend(modes_objects(&format!("{path}[{index}]"), item));
            }
        }
        _ => {}
    }
    found
}

/// Assert every `KeyModesOfUse` in `document` carries the discriminator beside exactly
/// the nine modelled flags, with the flag values this family was created with.
fn assert_discriminated(case: &str, document: &Value, flags: &BTreeMap<&'static str, bool>) {
    let found = modes_objects("$", document);
    assert!(
        !found.is_empty(),
        "case {case}: expected the answer to carry a KeyModesOfUse, got {document}"
    );
    for (path, modes) in found {
        let members = modes.as_object().unwrap_or_else(|| {
            panic!("case {case}: expected {path} to be an object, got {modes}")
        });

        assert_eq!(
            modes.get("__type").and_then(Value::as_str),
            Some(TYPE_DISCRIMINATOR),
            "case {case}: expected {path} to carry __type {TYPE_DISCRIMINATOR:?}, got {modes}"
        );

        // The nine modelled flags, each as the stored key carries it.
        for (member, expected) in flags {
            assert_eq!(
                modes.get(*member).and_then(Value::as_bool),
                Some(*expected),
                "case {case}: expected {path}.{member} to be {expected}, got {modes}"
            );
        }

        // And nothing else: the discriminator is the ONE member beyond the model.
        let seen: BTreeSet<&str> = members.keys().map(String::as_str).collect();
        let mut expected: BTreeSet<&str> = MODE_MEMBERS.iter().copied().collect();
        expected.insert("__type");
        assert_eq!(
            seen, expected,
            "case {case}: expected {path} to carry exactly {expected:?}, got {seen:?}"
        );
    }
}

/// The one `Keys` entry of a `ListKeys` answer that names `arn`, as its own document.
fn listed_summary(case: &str, listed: &Value, arn: &str) -> Value {
    listed
        .get("Keys")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: ListKeys answered no Keys: {listed}"))
        .iter()
        .find(|item| string(item, "KeyArn") == arn)
        .unwrap_or_else(|| panic!("case {case}: ListKeys does not list {arn}: {listed}"))
        .clone()
}

#[tokio::test]
async fn every_key_answer_carries_the_modes_of_use_type_discriminator() {
    let router = router();

    for family in KEY_FAMILIES {
        let flags = expected_flags(family);
        let algorithm = family.key_algorithm;

        let case = format!("{algorithm}: CreateKey");
        let created = ok_call(&router, "CreateKey", &create_body(family, modes_request(family)))
            .await;
        assert_discriminated(&case, &created, &flags);
        let arn = string(
            created
                .get("Key")
                .unwrap_or_else(|| panic!("case {case}: no Key in {created}")),
            "KeyArn",
        )
        .to_string();

        // Every other served operation that answers with this key, or with a summary
        // of it, writes the same shape through the same writer.
        let identified = json!({"KeyIdentifier": arn});
        for operation in ["GetKey", "StopKeyUsage", "StartKeyUsage"] {
            let case = format!("{algorithm}: {operation}");
            let answered = ok_call(&router, operation, &identified).await;
            assert_discriminated(&case, &answered, &flags);
        }

        // The list path serializes a KeySummary rather than a Key, and its nested
        // modes of use carry the discriminator too.
        let case = format!("{algorithm}: ListKeys");
        let listed = ok_call(&router, "ListKeys", &json!({})).await;
        let summary = listed_summary(&case, &listed, &arn);
        assert_discriminated(&case, &summary, &flags);

        // Scheduling the deletion and taking it back answer the same key.
        let case = format!("{algorithm}: DeleteKey");
        let deleted = ok_call(
            &router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": 3}),
        )
        .await;
        assert_discriminated(&case, &deleted, &flags);

        let case = format!("{algorithm}: RestoreKey");
        let restored = ok_call(&router, "RestoreKey", &identified).await;
        assert_discriminated(&case, &restored, &flags);

        // And the reader after all of that still answers the one stored record,
        // discriminator included.
        let case = format!("{algorithm}: GetKey after the delete/restore round trip");
        let fetched = ok_call(&router, "GetKey", &identified).await;
        assert_discriminated(&case, &fetched, &flags);
    }
}

#[tokio::test]
async fn a_request_that_carries_the_discriminator_back_reads_the_same_modes() {
    let router = router();

    // The discriminator is not a modelled member, so a client that round-trips a
    // response into its next request — the shape AWS's own readers accept — is read
    // exactly as one that sends only the modelled flags: the key it creates reports
    // the same modes of use either way.
    for family in KEY_FAMILIES {
        let flags = expected_flags(family);
        let algorithm = family.key_algorithm;

        let mut echoed = modes_request(family);
        if let Value::Object(modes) = &mut echoed {
            modes.insert("__type".to_string(), json!(TYPE_DISCRIMINATOR));
        }

        for (label, modes) in [
            ("the modelled flags alone", modes_request(family)),
            ("the flags with the discriminator echoed back", echoed),
        ] {
            let case = format!("{algorithm} created from {label}");
            let created = ok_call(&router, "CreateKey", &create_body(family, modes)).await;
            assert_discriminated(&case, &created, &flags);
        }
    }
}
