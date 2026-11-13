// SPDX-License-Identifier: CC0-1.0

//! `ExportKey`, end to end over the wire: the wrapped key every export mechanism
//! answers with, and the preconditions each of them refuses.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rules under test:
//!
//! * a key leaves the service wrapped under ANOTHER key this store holds — the key
//!   block is built from the key under export and from the wrapping key the request
//!   names, its header describes the key it carries (usage, algorithm, mode of use,
//!   version, exportability and the optional blocks the caller asked for), and the
//!   check value answered is the check value of the material inside it;
//! * `Exportable` is what decides whether a key may leave at all, and an identifier
//!   that names no key, or a wrapping key that cannot wrap, is refused rather than
//!   served;
//! * an IPEK export answers the initial DUKPT key the base derivation key and the key
//!   serial number derive, which is a function of those two and of nothing else.
//!
//! Each case is one row of a table spanning what the rule generalises over — both key
//! block binding methods, each key algorithm family, each mode of use the header can
//! carry, each AS 2805 key variant — not the one algorithm an issue happened to name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router, string};

/// The `KeyUsage` of the key encryption key every symmetric export below wraps under.
const WRAPPING_KEY_USAGE: &str = "TR31_K0_KEY_ENCRYPTION_KEY";

/// A key ARN of this emulator's account and region that no key was ever minted under;
/// it satisfies the modelled `KeyArn` pattern `key/[0-9a-zA-Z]{16,64}`.
fn unknown_key_arn() -> String {
    format!(
        "arn:aws:payment-cryptography:{}:000000000000:key/neverminted00000",
        crate::key_lifecycle::REGION
    )
}

/// A `CreateKey` body for one key.
fn create_key_body(key_usage: &str, key_algorithm: &str, modes: Value, exportable: bool) -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": key_usage,
            // Every algorithm these tests create a key of is symmetric, which is the
            // class the model gives those algorithms.
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": key_algorithm,
            "KeyModesOfUse": modes,
        },
        "Exportable": exportable,
    })
}

/// One created key: its ARN and the check value `CreateKey` reported for it.
struct CreatedKey {
    key_arn: String,
    key_check_value: String,
    key_check_value_algorithm: String,
}

/// Create one key and answer what the service stored for it.
async fn create_key(
    router: &MultiServiceRouter,
    key_usage: &str,
    key_algorithm: &str,
    modes: Value,
    exportable: bool,
) -> CreatedKey {
    let body = create_key_body(key_usage, key_algorithm, modes, exportable);
    let created = ok_call(router, "CreateKey", &body).await;
    let key = created
        .get("Key")
        .unwrap_or_else(|| panic!("no Key in {created}"));
    CreatedKey {
        key_arn: string(key, "KeyArn").to_string(),
        key_check_value: string(key, "KeyCheckValue").to_string(),
        key_check_value_algorithm: string(key, "KeyCheckValueAlgorithm").to_string(),
    }
}

/// Create a key encryption key of `key_algorithm` that may wrap a key for export.
async fn create_wrapping_key(router: &MultiServiceRouter, key_algorithm: &str) -> String {
    create_key(
        router,
        WRAPPING_KEY_USAGE,
        key_algorithm,
        json!({"Wrap": true, "Unwrap": true}),
        false,
    )
    .await
    .key_arn
}

/// The `WrappedKey` a successful export answers with.
async fn export(router: &MultiServiceRouter, body: &Value) -> Value {
    let answered = ok_call(router, "ExportKey", body).await;
    answered
        .get("WrappedKey")
        .cloned()
        .unwrap_or_else(|| panic!("no WrappedKey in {answered}"))
}

/// A TR-31 export body.
fn tr31_body(export_key_identifier: &str, wrapping_key_identifier: &str, headers: Value) -> Value {
    let mut block = json!({"WrappingKeyIdentifier": wrapping_key_identifier});
    if !headers.is_null() {
        block["KeyBlockHeaders"] = headers;
    }
    json!({
        "ExportKeyIdentifier": export_key_identifier,
        "KeyMaterial": {"Tr31KeyBlock": block},
    })
}

/// The fields of a key block header, read off the block the way a receiving party
/// does: the header is fixed width and transmitted in the clear.
struct KeyBlockHeader {
    version: char,
    length: usize,
    usage: String,
    algorithm: char,
    mode: char,
    key_version: String,
    exportability: char,
    optional_blocks: usize,
    reserved: String,
}

/// Read `block`'s header, or explain which part of it is not there.
fn header(case: &str, block: &str) -> KeyBlockHeader {
    assert!(
        block.len() >= 16 && block.is_ascii(),
        "case {case}: a key block is at least a 16-character ASCII header, got {block:?}"
    );
    let field = |from: usize, to: usize| -> String {
        block
            .get(from..to)
            .unwrap_or_else(|| panic!("case {case}: the header is shorter than {to} characters"))
            .to_string()
    };
    let character = |at: usize| -> char {
        field(at, at + 1)
            .chars()
            .next()
            .unwrap_or_else(|| panic!("case {case}: no character at {at}"))
    };
    let length = field(1, 5);
    KeyBlockHeader {
        version: character(0),
        length: length
            .parse()
            .unwrap_or_else(|e| panic!("case {case}: the length field {length:?} is not a number ({e})")),
        usage: field(5, 7),
        algorithm: character(7),
        mode: character(8),
        key_version: field(9, 11),
        exportability: character(11),
        optional_blocks: field(12, 14)
            .parse()
            .unwrap_or_else(|e| panic!("case {case}: the optional block count is not a number ({e})")),
        reserved: field(14, 16),
    }
}

/// One row of the TR-31 table: a key to export, and what its key block must say about
/// it.
struct Tr31Case {
    case: &'static str,
    /// The modelled `KeyUsage` of the key under export.
    key_usage: &'static str,
    /// Its `KeyAlgorithm`.
    key_algorithm: &'static str,
    /// How many bytes of key material that algorithm names, where the algorithm itself
    /// fixes it; `None` for HMAC, whose key length is not fixed by the algorithm name.
    key_bytes: Option<usize>,
    /// Its `KeyModesOfUse`.
    modes: Value,
    /// The `KeyAlgorithm` of the key encryption key it is wrapped under.
    wrapping_key_algorithm: &'static str,
    /// The version id the binding method of that wrapping key carries: `B` for the TDES
    /// key derivation method, `D` for the AES one.
    version: char,
    /// The two-character key usage the header carries, which is the TR-31 usage the
    /// modelled `KeyUsage` value names.
    usage_code: &'static str,
    /// The header's algorithm character for the key under export.
    algorithm: char,
    /// The header's mode of use character for its modes of use.
    mode: char,
}

/// The TR-31 rows: both binding methods, each symmetric algorithm family, and a
/// different mode of use in every row.
fn tr31_cases() -> Vec<Tr31Case> {
    vec![
        Tr31Case {
            case: "a PIN encryption key under a double-length TDES key encryption key",
            key_usage: "TR31_P0_PIN_ENCRYPTION_KEY",
            key_algorithm: "TDES_3KEY",
            key_bytes: Some(24),
            modes: json!({"Encrypt": true, "Decrypt": true}),
            wrapping_key_algorithm: "TDES_2KEY",
            version: 'B',
            usage_code: "P0",
            algorithm: 'T',
            mode: 'B',
        },
        Tr31Case {
            case: "a data encryption key under an AES-256 key encryption key",
            key_usage: "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            key_algorithm: "AES_128",
            key_bytes: Some(16),
            modes: json!({"Encrypt": true, "Decrypt": true}),
            wrapping_key_algorithm: "AES_256",
            version: 'D',
            usage_code: "D0",
            algorithm: 'A',
            mode: 'B',
        },
        Tr31Case {
            case: "an HMAC key, generate and verify, under an AES-192 key encryption key",
            key_usage: "TR31_M7_HMAC_KEY",
            key_algorithm: "HMAC_SHA256",
            key_bytes: None,
            modes: json!({"Generate": true, "Verify": true}),
            wrapping_key_algorithm: "AES_192",
            version: 'D',
            usage_code: "M7",
            algorithm: 'H',
            mode: 'C',
        },
        Tr31Case {
            case: "a PIN verification key, verify only, under a triple-length TDES key",
            key_usage: "TR31_V2_VISA_PIN_VERIFICATION_KEY",
            key_algorithm: "TDES_2KEY",
            key_bytes: Some(16),
            modes: json!({"Verify": true}),
            wrapping_key_algorithm: "TDES_3KEY",
            version: 'B',
            usage_code: "V2",
            algorithm: 'T',
            mode: 'V',
        },
        Tr31Case {
            case: "a key block protection key with no restrictions under an AES-128 key",
            key_usage: "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
            key_algorithm: "AES_256",
            key_bytes: Some(32),
            modes: json!({"NoRestrictions": true}),
            wrapping_key_algorithm: "AES_128",
            version: 'D',
            usage_code: "K1",
            algorithm: 'A',
            mode: 'N',
        },
        Tr31Case {
            case: "a PIN generation key, generate only, under a TDES key encryption key",
            key_usage: "TR31_P1_PIN_GENERATION_KEY",
            key_algorithm: "TDES_2KEY",
            key_bytes: Some(16),
            modes: json!({"Generate": true}),
            wrapping_key_algorithm: "TDES_2KEY",
            version: 'B',
            usage_code: "P1",
            algorithm: 'T',
            mode: 'G',
        },
        Tr31Case {
            case: "a base derivation key, derivation only, under an AES-256 key",
            key_usage: "TR31_B0_BASE_DERIVATION_KEY",
            key_algorithm: "TDES_2KEY",
            key_bytes: Some(16),
            modes: json!({"DeriveKey": true}),
            wrapping_key_algorithm: "AES_256",
            version: 'D',
            usage_code: "B0",
            algorithm: 'T',
            mode: 'X',
        },
    ]
}

/// The cipher block, in bytes, of the binding method a key block version uses.
fn binding_block(version: char) -> usize {
    if version == 'B' {
        8
    } else {
        16
    }
}

/// How long a key block carrying `key_bytes` of key material under `version` is: the
/// 16-character header, then the payload — the key's length in bits, the key, and
/// padding up to a whole cipher block — and the MAC, each rendered as hexadecimal.
fn expected_block_len(version: char, key_bytes: usize) -> usize {
    let block = binding_block(version);
    let payload = (2 + key_bytes).div_ceil(block) * block;
    16 + 2 * payload + 2 * block
}

/// Assert everything after a key block's clear-text header region is uppercase
/// hexadecimal, which is what the encrypted payload and the MAC are rendered as.
fn assert_wrapped_tail(case: &str, block: &str, clear_len: usize) {
    let tail = block
        .get(clear_len..)
        .unwrap_or_else(|| panic!("case {case}: the block is shorter than its header"));
    assert!(
        !tail.is_empty()
            && tail
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
        "case {case}: expected the wrapped payload and MAC as uppercase hex, got {tail:?}"
    );
}

#[tokio::test]
async fn export_key_wraps_a_key_in_a_tr31_key_block_under_the_wrapping_key() {
    let router = router();

    for row in tr31_cases() {
        let case = row.case;
        let wrapping_key = create_wrapping_key(&router, row.wrapping_key_algorithm).await;
        let key = create_key(
            &router,
            row.key_usage,
            row.key_algorithm,
            row.modes.clone(),
            true,
        )
        .await;

        let body = tr31_body(&key.key_arn, &wrapping_key, Value::Null);
        let wrapped = export(&router, &body).await;

        assert_eq!(
            string(&wrapped, "WrappedKeyMaterialFormat"),
            "TR31_KEY_BLOCK",
            "case {case}: expected a TR-31 key block, got {wrapped}"
        );
        // The wrapping key is reported from the record the identifier resolved to.
        assert_eq!(
            string(&wrapped, "WrappingKeyArn"),
            wrapping_key,
            "case {case}: expected the wrapping key's own ARN, got {wrapped}"
        );
        // The check value answered is the check value of the material inside the
        // block, which for this export is the key under export's own — the same value
        // every reader of that key reports.
        assert_eq!(
            string(&wrapped, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: expected the exported key's own check value, got {wrapped}"
        );
        assert_eq!(
            string(&wrapped, "KeyCheckValueAlgorithm"),
            key.key_check_value_algorithm,
            "case {case}: expected the key's own check value algorithm, got {wrapped}"
        );

        let block = string(&wrapped, "KeyMaterial").to_string();
        let parsed = header(case, &block);
        assert_eq!(
            parsed.version, row.version,
            "case {case}: expected the version id of the binding method of a \
             {} wrapping key, got {}",
            row.wrapping_key_algorithm, parsed.version
        );
        assert_eq!(
            parsed.length,
            block.len(),
            "case {case}: the header's length field must be the block's own length {}, got {}",
            block.len(),
            parsed.length
        );
        assert_eq!(
            parsed.usage, row.usage_code,
            "case {case}: expected the key usage {}, got {}",
            row.usage_code, parsed.usage
        );
        assert_eq!(
            parsed.algorithm, row.algorithm,
            "case {case}: expected the algorithm {}, got {}",
            row.algorithm, parsed.algorithm
        );
        assert_eq!(
            parsed.mode, row.mode,
            "case {case}: expected the mode of use {}, got {}",
            row.mode, parsed.mode
        );
        // A key with no version of its own carries the "not used" key version number,
        // the key itself is exportable, and nothing asked for an optional block.
        assert_eq!(
            parsed.key_version, "00",
            "case {case}: expected the unused key version number, got {}",
            parsed.key_version
        );
        assert_eq!(
            parsed.exportability, 'E',
            "case {case}: an exportable key's block says so, got {}",
            parsed.exportability
        );
        assert_eq!(
            parsed.optional_blocks, 0,
            "case {case}: expected no optional blocks, got {}",
            parsed.optional_blocks
        );
        assert_eq!(
            parsed.reserved, "00",
            "case {case}: expected the reserved field, got {}",
            parsed.reserved
        );
        assert_wrapped_tail(case, &block, 16);

        if let Some(key_bytes) = row.key_bytes {
            let expected = expected_block_len(row.version, key_bytes);
            assert_eq!(
                block.len(),
                expected,
                "case {case}: a {key_bytes}-byte key under a {} block cipher makes a {expected}-\
                 character key block, got {} ({block})",
                binding_block(row.version),
                block.len()
            );
        }

        // The payload's padding is drawn per export, so the same key wrapped twice
        // under the same key is not the same block — and the header, which describes
        // the key rather than the wrapping, is.
        let again = export(&router, &body).await;
        let second = string(&again, "KeyMaterial").to_string();
        assert_ne!(
            second, block,
            "case {case}: two exports of one key must not be the same block"
        );
        assert_eq!(
            second.get(..16),
            block.get(..16),
            "case {case}: two exports of one key carry the same header, got {second}"
        );
        assert_eq!(
            string(&again, "KeyCheckValue"),
            key.key_check_value,
            "case {case}: the second export carries the same key, so the same check value"
        );

        // Wrapping the same key under a DIFFERENT key encryption key gives a different
        // block: the wrapping key's material is what the payload is encrypted under.
        let other_wrapping_key = create_wrapping_key(&router, row.wrapping_key_algorithm).await;
        let other = export(
            &router,
            &tr31_body(&key.key_arn, &other_wrapping_key, Value::Null),
        )
        .await;
        assert_ne!(
            string(&other, "KeyMaterial"),
            block,
            "case {case}: a key wrapped under another key encryption key must differ"
        );
        assert_eq!(
            string(&other, "WrappingKeyArn"),
            other_wrapping_key,
            "case {case}: expected the other wrapping key's ARN, got {other}"
        );
    }
}

#[tokio::test]
async fn export_key_renders_the_key_block_headers_the_request_names() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "AES_256").await;
    let key = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "AES_128",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;

    // The rows differ along what the header rule generalises over: the key version, the
    // exportability the export restricts the key to, the modes of use the block
    // declares, and how many optional blocks the header carries. The optional block
    // ids and values satisfy the modelled `OptionalBlockId` / `OptionalBlockValue`
    // patterns (`^[0-9A-Z]{2}+$`, `^[0-9a-zA-Z]+$`).
    let cases: &[(&str, Value, &str, char, char, usize)] = &[
        (
            "a key version",
            json!({"KeyVersion": "01"}),
            "01",
            'E',
            'B',
            0,
        ),
        (
            "non-exportable after export",
            json!({"KeyExportability": "NON_EXPORTABLE"}),
            "00",
            'N',
            'B',
            0,
        ),
        (
            "sensitive after export",
            json!({"KeyExportability": "SENSITIVE"}),
            "00",
            'S',
            'B',
            0,
        ),
        (
            "narrowed modes of use",
            json!({"KeyModesOfUse": {"Decrypt": true}}),
            "00",
            'E',
            'D',
            0,
        ),
        (
            "one optional block",
            json!({"OptionalBlocks": {"KS": "0123456789ABCDEF"}}),
            "00",
            'E',
            'B',
            // The optional block, plus the padding block that aligns the header region
            // to the cipher's block size.
            2,
        ),
        (
            "two optional blocks and every other header",
            json!({
                "KeyVersion": "C1",
                "KeyExportability": "SENSITIVE",
                "KeyModesOfUse": {"Encrypt": true},
                "OptionalBlocks": {"KS": "00112233445566", "TS": "2026"},
            }),
            "C1",
            'S',
            'E',
            3,
        ),
    ];

    for (case, headers, key_version, exportability, mode, optional_blocks) in cases {
        let wrapped = export(
            &router,
            &tr31_body(&key.key_arn, &wrapping_key, headers.clone()),
        )
        .await;
        let block = string(&wrapped, "KeyMaterial").to_string();
        let parsed = header(case, &block);

        assert_eq!(
            parsed.key_version, *key_version,
            "case {case}: expected the key version {key_version}, got {}",
            parsed.key_version
        );
        assert_eq!(
            parsed.exportability, *exportability,
            "case {case}: expected the exportability {exportability}, got {}",
            parsed.exportability
        );
        assert_eq!(
            parsed.mode, *mode,
            "case {case}: expected the mode of use {mode}, got {}",
            parsed.mode
        );
        assert_eq!(
            parsed.optional_blocks, *optional_blocks,
            "case {case}: expected {optional_blocks} optional blocks, got {} ({block})",
            parsed.optional_blocks
        );
        assert_eq!(
            parsed.length,
            block.len(),
            "case {case}: the length field must be the block's own length, got {}",
            parsed.length
        );

        // "Data included in key block headers is signed but transmitted in clear text"
        // (the operation's own documentation), so each optional block the request named
        // is in the header region: its id, its length counting the id and the length
        // itself, then its value.
        let optional_region = headers
            .get("OptionalBlocks")
            .and_then(Value::as_object)
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|(id, value)| {
                        let value = value.as_str().unwrap_or_default();
                        format!("{id}{:02X}{value}", value.len() + 4)
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        let mut clear_len = 16;
        for rendered in &optional_region {
            assert!(
                block.contains(rendered),
                "case {case}: expected the optional block {rendered} in the clear, got {block}"
            );
            clear_len += rendered.len();
        }
        // The padding block that follows them is clear text too, so the hexadecimal
        // tail starts after it.
        if *optional_blocks > optional_region.len() {
            let padding = block
                .get(clear_len..)
                .and_then(|rest| rest.get(..4))
                .unwrap_or_default();
            assert!(
                padding.starts_with("PB"),
                "case {case}: expected a padding block after the optional blocks, got {padding:?}"
            );
            let length = usize::from_str_radix(padding.get(2..4).unwrap_or_default(), 16)
                .unwrap_or_else(|e| panic!("case {case}: the padding length is hex ({e})"));
            clear_len += length;
        }
        assert_eq!(
            (clear_len) % binding_block('D'),
            0,
            "case {case}: the header region must be a whole number of cipher blocks, got \
             {clear_len}"
        );
        assert_wrapped_tail(case, &block, clear_len);
    }
}

#[tokio::test]
async fn export_key_refuses_a_key_that_may_not_leave_the_service() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "AES_256").await;

    // The rule is `Exportable`, not the algorithm: a key created without it cannot be
    // exported whatever kind of key it is.
    for key_algorithm in ["TDES_2KEY", "AES_128", "AES_256", "HMAC_SHA256"] {
        let key = create_key(
            &router,
            "TR31_P0_PIN_ENCRYPTION_KEY",
            key_algorithm,
            json!({"Encrypt": true, "Decrypt": true}),
            false,
        )
        .await;
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &tr31_body(&key.key_arn, &wrapping_key, Value::Null),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {key_algorithm}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {key_algorithm}: expected ValidationException, got {parsed}"
        );
        let expected = format!("Export is not enabled for the key {}.", key.key_arn);
        assert_eq!(
            parsed.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {key_algorithm}: expected the message {expected:?}, got {parsed}"
        );
    }
}

#[tokio::test]
async fn export_key_refuses_an_identifier_it_holds_no_key_for() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "AES_256").await;
    let key = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "AES_128",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;
    let absent = unknown_key_arn();

    // Either identifier the request carries names a key this store must hold: the key
    // under export, and the key that wraps it. Both forms the modelled
    // `KeyArnOrKeyAliasType` admits are covered — a key ARN and an alias name.
    let cases: &[(&str, &str, &str)] = &[
        ("the key under export", absent.as_str(), wrapping_key.as_str()),
        ("the key under export by alias", "alias/never-created", wrapping_key.as_str()),
        ("the wrapping key", key.key_arn.as_str(), absent.as_str()),
        ("the wrapping key by alias", key.key_arn.as_str(), "alias/never-created"),
    ];

    for (case, export_identifier, wrapping_identifier) in cases {
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &tr31_body(export_identifier, wrapping_identifier, Value::Null),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {parsed}"
        );
        // The error names the identifier the request asked for, whichever of the two it
        // was.
        let missing = if case.starts_with("the key under export") {
            export_identifier
        } else {
            wrapping_identifier
        };
        assert_eq!(
            parsed.get("ResourceId").and_then(Value::as_str),
            Some(*missing),
            "case {case}: expected the identifier {missing} in ResourceId, got {parsed}"
        );
    }
}

#[tokio::test]
async fn export_key_refuses_a_wrapping_key_that_cannot_wrap_a_key_for_export() {
    let router = router();
    let key = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "AES_128",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;

    // The rows differ along what the rule generalises over: what the candidate wrapping
    // key is FOR, what it is permitted to do, and whether it is still in use.
    let not_a_key_encryption_key = create_key(
        &router,
        "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
        "AES_256",
        json!({"Encrypt": true, "Decrypt": true}),
        false,
    )
    .await;
    let cannot_wrap = create_key(
        &router,
        WRAPPING_KEY_USAGE,
        "AES_256",
        json!({"Unwrap": true}),
        false,
    )
    .await;
    let not_symmetric = create_key(
        &router,
        WRAPPING_KEY_USAGE,
        "HMAC_SHA256",
        json!({"Wrap": true, "Unwrap": true}),
        false,
    )
    .await;
    let stopped = create_wrapping_key(&router, "AES_256").await;
    ok_call(&router, "StopKeyUsage", &json!({"KeyIdentifier": stopped})).await;

    let cases: &[(&str, &str)] = &[
        (
            "a data encryption key is not a key encryption key",
            not_a_key_encryption_key.key_arn.as_str(),
        ),
        (
            "a key encryption key with no Wrap cannot wrap",
            cannot_wrap.key_arn.as_str(),
        ),
        (
            "an HMAC key is not a key block protection key algorithm",
            not_symmetric.key_arn.as_str(),
        ),
        ("a disabled key performs no cryptography", stopped.as_str()),
    ];

    for (case, wrapping_identifier) in cases {
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &tr31_body(&key.key_arn, wrapping_identifier, Value::Null),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
        let message = parsed.get("Message").and_then(Value::as_str).unwrap_or("");
        assert!(
            message.contains(wrapping_identifier),
            "case {case}: expected the refusal to name the wrapping key \
             {wrapping_identifier}, got {parsed}"
        );
    }
}

/// A TR-31 export body that asks for the initial DUKPT key `key_serial_number` names.
fn ipek_body(
    export_key_identifier: &str,
    wrapping_key_identifier: &str,
    key_serial_number: &str,
) -> Value {
    let mut body = tr31_body(export_key_identifier, wrapping_key_identifier, Value::Null);
    body["ExportAttributes"] = json!({
        "ExportDukptInitialKey": {"KeySerialNumber": key_serial_number},
    });
    body
}

#[tokio::test]
async fn export_key_derives_an_initial_dukpt_key_from_the_base_derivation_key_and_the_ksn() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "AES_256").await;

    // One row per DUKPT family: a double-length TDES base derivation key, whose key
    // serial number is 20 hexadecimal characters, and AES ones, whose key serial number
    // is 24 (the modelled `HexLength20Or24` pattern admits exactly those two lengths).
    // `initial`, `counter` and `other` share an initial key id in the first two and
    // differ in it in the third, which is what the derivation reads.
    let cases: &[(&str, &str, &str, &str, &str, char)] = &[
        (
            "a TDES base derivation key",
            "TDES_2KEY",
            "FFFF9876543210E00000",
            "FFFF9876543210E00008",
            "FFFF9876543211E00000",
            'T',
        ),
        (
            "an AES-128 base derivation key",
            "AES_128",
            "FFFF9876543210E000000001",
            "FFFF9876543210E000000002",
            "FFFF9876543211E000000001",
            'A',
        ),
        (
            "an AES-256 base derivation key",
            "AES_256",
            "FFFF9876543210E000000001",
            "FFFF9876543210E000000002",
            "FFFF9876543211E000000001",
            'A',
        ),
    ];

    for (case, key_algorithm, initial, counter, other, algorithm) in cases {
        let bdk = create_key(
            &router,
            "TR31_B0_BASE_DERIVATION_KEY",
            key_algorithm,
            json!({"DeriveKey": true}),
            true,
        )
        .await;

        let derived = export(&router, &ipek_body(&bdk.key_arn, &wrapping_key, initial)).await;
        let block = string(&derived, "KeyMaterial").to_string();
        let parsed = header(case, &block);
        // The block carries the initial DUKPT key, not the base derivation key it was
        // derived from, so its header says so and its check value is the derived key's.
        assert_eq!(
            parsed.usage, "B1",
            "case {case}: expected the initial DUKPT key usage, got {}",
            parsed.usage
        );
        assert_eq!(
            parsed.algorithm, *algorithm,
            "case {case}: expected the algorithm {algorithm}, got {}",
            parsed.algorithm
        );
        assert_ne!(
            string(&derived, "KeyCheckValue"),
            bdk.key_check_value,
            "case {case}: an IPEK is not the base derivation key, so its check value differs"
        );

        // The derivation is a function of the base derivation key and the initial key
        // id in the key serial number: the same pair derives the same key, a key serial
        // number that differs only in its transaction counter derives the same key, and
        // a different initial key id derives another one.
        let repeated = export(&router, &ipek_body(&bdk.key_arn, &wrapping_key, initial)).await;
        assert_eq!(
            string(&repeated, "KeyCheckValue"),
            string(&derived, "KeyCheckValue"),
            "case {case}: the same KSN must derive the same initial key"
        );
        let same_initial_id =
            export(&router, &ipek_body(&bdk.key_arn, &wrapping_key, counter)).await;
        assert_eq!(
            string(&same_initial_id, "KeyCheckValue"),
            string(&derived, "KeyCheckValue"),
            "case {case}: a KSN differing only in its transaction counter must derive the same \
             initial key"
        );
        let other_initial_id = export(&router, &ipek_body(&bdk.key_arn, &wrapping_key, other)).await;
        assert_ne!(
            string(&other_initial_id, "KeyCheckValue"),
            string(&derived, "KeyCheckValue"),
            "case {case}: another initial key id must derive another initial key"
        );

        // Two different base derivation keys derive different initial keys from one key
        // serial number.
        let another_bdk = create_key(
            &router,
            "TR31_B0_BASE_DERIVATION_KEY",
            key_algorithm,
            json!({"DeriveKey": true}),
            true,
        )
        .await;
        let from_another =
            export(&router, &ipek_body(&another_bdk.key_arn, &wrapping_key, initial)).await;
        assert_ne!(
            string(&from_another, "KeyCheckValue"),
            string(&derived, "KeyCheckValue"),
            "case {case}: another base derivation key must derive another initial key"
        );
    }
}

#[tokio::test]
async fn export_key_refuses_an_initial_dukpt_key_the_request_cannot_derive() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "AES_256").await;

    // An IPEK is derived from a base derivation key, so a key under export that is not
    // one derives nothing…
    let not_a_bdk = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "TDES_2KEY",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;
    // …and a key serial number of the other family's length does not name one either.
    let tdes_bdk = create_key(
        &router,
        "TR31_B0_BASE_DERIVATION_KEY",
        "TDES_2KEY",
        json!({"DeriveKey": true}),
        true,
    )
    .await;
    let aes_bdk = create_key(
        &router,
        "TR31_B0_BASE_DERIVATION_KEY",
        "AES_128",
        json!({"DeriveKey": true}),
        true,
    )
    .await;
    let triple_length_bdk = create_key(
        &router,
        "TR31_B0_BASE_DERIVATION_KEY",
        "TDES_3KEY",
        json!({"DeriveKey": true}),
        true,
    )
    .await;

    let cases: &[(&str, &str, &str)] = &[
        (
            "a PIN encryption key is not a base derivation key",
            not_a_bdk.key_arn.as_str(),
            "FFFF9876543210E00000",
        ),
        (
            "an AES key serial number for a TDES base derivation key",
            tdes_bdk.key_arn.as_str(),
            "FFFF9876543210E000000001",
        ),
        (
            "a TDES key serial number for an AES base derivation key",
            aes_bdk.key_arn.as_str(),
            "FFFF9876543210E00000",
        ),
        (
            "a triple-length TDES base derivation key has no DUKPT derivation",
            triple_length_bdk.key_arn.as_str(),
            "FFFF9876543210E00000",
        ),
    ];

    for (case, export_identifier, key_serial_number) in cases {
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &ipek_body(export_identifier, &wrapping_key, key_serial_number),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
    }
}

#[tokio::test]
async fn export_key_wraps_a_key_as_an_as2805_key_cryptogram_under_a_key_variant() {
    let router = router();
    let wrapping_key = create_wrapping_key(&router, "TDES_2KEY").await;
    let key = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "TDES_3KEY",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;
    let as2805_body = |variant: &str, wrapping: &str| {
        json!({
            "ExportKeyIdentifier": key.key_arn,
            "KeyMaterial": {"As2805KeyCryptogram": {
                "WrappingKeyIdentifier": wrapping,
                "As2805KeyVariant": variant,
            }},
        })
    };

    // Every modelled `As2805KeyVariant` (`smithy_query(query="shape",
    // shape="As2805KeyVariant")`), since each names its own key variant of the wrapping
    // key and so its own cryptogram of the same key.
    let variants = [
        "TERMINAL_MAJOR_KEY_VARIANT_00",
        "PIN_ENCRYPTION_KEY_VARIANT_28",
        "MESSAGE_AUTHENTICATION_KEY_VARIANT_24",
        "DATA_ENCRYPTION_KEY_VARIANT_22",
    ];
    let mut cryptograms: Vec<String> = Vec::new();
    for variant in variants {
        let wrapped = export(&router, &as2805_body(variant, &wrapping_key)).await;
        assert_eq!(
            string(&wrapped, "WrappedKeyMaterialFormat"),
            "KEY_CRYPTOGRAM",
            "case {variant}: expected a key cryptogram, got {wrapped}"
        );
        assert_eq!(
            string(&wrapped, "WrappingKeyArn"),
            wrapping_key,
            "case {variant}: expected the wrapping key's own ARN, got {wrapped}"
        );
        assert_eq!(
            string(&wrapped, "KeyCheckValue"),
            key.key_check_value,
            "case {variant}: the cryptogram carries the key under export, so its check value"
        );
        let cryptogram = string(&wrapped, "KeyMaterial").to_string();
        // A cryptogram is the key under export encrypted block by block, so it is as
        // long as that key: a triple-length TDES key is 24 bytes, 48 hex characters.
        assert_eq!(
            cryptogram.len(),
            48,
            "case {variant}: expected a 48-character cryptogram of a 24-byte key, got \
             {cryptogram}"
        );
        // The cipher is deterministic, so the same request answers the same cryptogram…
        let repeated = export(&router, &as2805_body(variant, &wrapping_key)).await;
        assert_eq!(
            string(&repeated, "KeyMaterial"),
            cryptogram,
            "case {variant}: the same key under the same variant is the same cryptogram"
        );
        // …and no other variant's cryptogram, since each encrypts under its own variant
        // of the key encryption key.
        assert!(
            !cryptograms.contains(&cryptogram),
            "case {variant}: expected a cryptogram no other key variant produced, got \
             {cryptogram}"
        );
        cryptograms.push(cryptogram);
    }

    // Another key encryption key gives another cryptogram of the same key.
    let other_wrapping_key = create_wrapping_key(&router, "TDES_2KEY").await;
    let other = export(&router, &as2805_body(variants[0], &other_wrapping_key)).await;
    assert!(
        !cryptograms.contains(&string(&other, "KeyMaterial").to_string()),
        "case another wrapping key: expected another cryptogram, got {other}"
    );

    // An AES key encryption key forms no AS 2805 key variant, so that pairing is
    // refused rather than wrapped under some other cipher.
    let aes_wrapping_key = create_wrapping_key(&router, "AES_256").await;
    let (status, parsed) = call(
        &router,
        "ExportKey",
        &as2805_body(variants[0], &aes_wrapping_key),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an AES wrapping key: expected 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case an AES wrapping key: expected ValidationException, got {parsed}"
    );
}

#[tokio::test]
async fn export_key_refuses_an_asymmetric_mechanism_whose_certificate_authority_is_not_held() {
    let router = router();
    let key = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "AES_128",
        json!({"Encrypt": true, "Decrypt": true}),
        true,
    )
    .await;
    // A certificate in PEM format, base64 encoded, as the modelled `CertificateType`
    // takes it; it is never parsed, because the certificate authority precondition is
    // judged first.
    let certificate = "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCg==";
    let absent = unknown_key_arn();
    // A key this store DOES hold, but not a certificate authority public key: a
    // symmetric key is neither a public key nor a signature verification key.
    let symmetric = create_key(
        &router,
        "TR31_P0_PIN_ENCRYPTION_KEY",
        "AES_128",
        json!({"Encrypt": true, "Decrypt": true}),
        false,
    )
    .await;

    // Each asymmetric mechanism names the certificate authority whose root certificate
    // signed the wrapping certificate, and every one of them is held to it.
    type KeyMaterialFor = Box<dyn Fn(&str) -> Value>;
    let mechanisms: Vec<(&str, KeyMaterialFor)> = vec![
        (
            "Tr34KeyBlock",
            Box::new(move |authority: &str| {
                json!({"Tr34KeyBlock": {
                    "CertificateAuthorityPublicKeyIdentifier": authority,
                    "WrappingKeyCertificate": certificate,
                    "KeyBlockFormat": "X9_TR34_2012",
                }})
            }),
        ),
        (
            "KeyCryptogram",
            Box::new(move |authority: &str| {
                json!({"KeyCryptogram": {
                    "CertificateAuthorityPublicKeyIdentifier": authority,
                    "WrappingKeyCertificate": certificate,
                    "WrappingSpec": "RSA_OAEP_SHA_256",
                }})
            }),
        ),
        (
            "DiffieHellmanTr31KeyBlock",
            Box::new(move |authority: &str| {
                json!({"DiffieHellmanTr31KeyBlock": {
                    "PrivateKeyIdentifier": authority,
                    "CertificateAuthorityPublicKeyIdentifier": authority,
                    "PublicKeyCertificate": certificate,
                    "DeriveKeyAlgorithm": "AES_256",
                    "KeyDerivationFunction": "NIST_SP800",
                    "KeyDerivationHashAlgorithm": "SHA_256",
                    "DerivationData": {"SharedInformation": "0102"},
                }})
            }),
        ),
    ];

    for (mechanism, material) in &mechanisms {
        // An identifier this store holds no key for.
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &json!({
                "ExportKeyIdentifier": key.key_arn,
                "KeyMaterial": material(&absent),
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {mechanism} with no such authority: expected 404, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {mechanism} with no such authority: expected ResourceNotFoundException, got \
             {parsed}"
        );
        assert_eq!(
            parsed.get("ResourceId").and_then(Value::as_str),
            Some(absent.as_str()),
            "case {mechanism} with no such authority: expected the identifier asked for in \
             ResourceId, got {parsed}"
        );

        // A key it does hold, that is not a certificate authority public key.
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &json!({
                "ExportKeyIdentifier": key.key_arn,
                "KeyMaterial": material(&symmetric.key_arn),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {mechanism} with a symmetric authority: expected 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {mechanism} with a symmetric authority: expected ValidationException, got \
             {parsed}"
        );

        // The key under export is held to the same rule as everywhere else: an
        // identifier this store holds no key for is refused before any certificate is
        // looked at.
        let (status, parsed) = call(
            &router,
            "ExportKey",
            &json!({
                "ExportKeyIdentifier": absent,
                "KeyMaterial": material(&symmetric.key_arn),
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {mechanism} with no key under export: expected 404, got {status} {parsed}"
        );
    }
}
