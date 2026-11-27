// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PROJECTION the grant plane puts on the wire, driven over
//! the wire through the shared [`super::harness`].
//!
//! The sibling [`super::grants`] suite is about what the grant WRITERS persist and
//! [`super::list_grants_reads`] about which grants a reader selects. This suite is
//! about how the grants a reader selected are RENDERED, and the rules it asserts hold
//! for every grant read, not for one request shape:
//!
//! 1. A grant listing reports `GrantCount`: how many grants the `Grants` list of that
//!    response carries, including `0` beside an empty list.
//! 2. Every `GrantListEntry` carries `Name` — the name the grant was created with, or
//!    the empty string for a grant created without one.
//! 3. Every entry reports its permitted `Operations` in the order the model declares
//!    the `GrantOperation` enum's values, whatever order the creating request listed
//!    them in.
//! 4. A listing reports its grants ordered by `GrantId` ascending, which is also the
//!    order its pages concatenate in.
//! 5. `CreateGrant` reports `KeyOrigin`: the `OriginType` of the key the grant was
//!    created on, the same value `DescribeKey` reports as `KeyMetadata.Origin`.
//!
//! Both readers answer one modelled shape, so every row runs against `ListGrants` and
//! `ListRetirableGrants` alike.
//!
//! A last test here is about the MODEL rather than the wire: the snapshot in
//! `smithy-model/` is kept exactly as shipped and declares NEITHER `GrantCount` on
//! `ListGrantsResponse` nor `KeyOrigin` on `CreateGrantResponse`, which is why this
//! crate declares those members itself; it pins that, so a refreshed snapshot that
//! adds one of them is noticed instead of being shadowed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{b64, create_key, describe, ok, principal, router, text, unb64, wrap};

/// The grantee of every grant created here. Which principal holds a grant is not what
/// these rules are about, so one is enough.
const GRANTEE: &str = "role/Projected";

/// The retiring principal every grant created here carries, so the same grant is
/// reachable through `ListRetirableGrants` as well as `ListGrants`.
const RETIRER: &str = "role/Retirer";

/// Every `GrantOperation` the model declares, IN THE ORDER it declares them
/// (`smithy_query(query="shape",shape="GrantOperation")`, whose `values` are listed in
/// declaration order) — which is both the set a reported `Operations` list draws from
/// and the order it is reported in. The set is checked against the shipped snapshot by
/// the last test here.
const GRANT_OPERATION_DECLARATION_ORDER: &[&str] = &[
    "Decrypt",
    "Encrypt",
    "GenerateDataKey",
    "GenerateDataKeyWithoutPlaintext",
    "ReEncryptFrom",
    "ReEncryptTo",
    "Sign",
    "Verify",
    "GetPublicKey",
    "CreateGrant",
    "RetireGrant",
    "DescribeKey",
    "GenerateDataKeyPair",
    "GenerateDataKeyPairWithoutPlaintext",
    "GenerateMac",
    "VerifyMac",
    "DeriveSharedSecret",
    "Encapsulate",
    "Decapsulate",
];

/// Every `OriginType` the model declares
/// (`smithy_query(query="shape",shape="OriginType")`), which is the set a reported
/// origin comes from.
const ORIGIN_TYPE_VALUES: &[&str] = &["AWS_KMS", "EXTERNAL", "AWS_CLOUDHSM", "EXTERNAL_KEY_STORE"];

/// How many bytes of key material a symmetric encryption KMS key takes.
const SYMMETRIC_MATERIAL_LEN: usize = 32;

/// Create a grant on `key_id` for [`GRANTEE`], retirable by [`RETIRER`], permitting
/// `operations` and carrying `name` when one is given, and report the `CreateGrant`
/// response.
async fn create_grant(
    router: &MultiServiceRouter,
    case: &str,
    key_id: &str,
    operations: &[&str],
    name: Option<&str>,
) -> Value {
    let mut request = json!({
        "KeyId": key_id,
        "GranteePrincipal": principal(GRANTEE),
        "RetiringPrincipal": principal(RETIRER),
        "Operations": operations,
    });
    if let Some(name) = name {
        request["Name"] = json!(name);
    }
    let answer = ok(router, "CreateGrant", &request).await;
    assert!(
        answer["GrantId"].is_string(),
        "case {case}: CreateGrant must answer a grant id, got {answer}"
    );
    answer
}

/// The entries one grant-listing answer reports, in the order reported — with the rule
/// that holds of EVERY such answer checked here: the count it carries is the number of
/// entries in it.
fn entries(case: &str, answer: &Value) -> Vec<Value> {
    let listed = answer["Grants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
        .clone();
    let counted = answer["GrantCount"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: expected a GrantCount number in {answer}"));
    assert_eq!(
        counted,
        listed.len() as i64,
        "case {case}: expected GrantCount {} for the {} grants answered, got {counted}",
        listed.len(),
        listed.len()
    );
    listed
}

/// Call one grant reader and report the entries it answered, checked by [`entries`].
async fn listing(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Vec<Value> {
    let answer = ok(router, operation, request).await;
    entries(&format!("{case} / {operation}"), &answer)
}

/// The request that asks each reader for the grants of `key_id`: `ListGrants` is scoped
/// to one key, `ListRetirableGrants` to one retiring principal.
fn read_requests(key_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListGrants", json!({"KeyId": key_id})),
        (
            "ListRetirableGrants",
            json!({"RetiringPrincipal": principal(RETIRER)}),
        ),
    ]
}

/// The `GrantId` of an entry.
fn grant_id(case: &str, entry: &Value) -> String {
    text(case, entry, "GrantId")
}

/// Rule 1: a grant listing reports how many grants it carries — over rows that differ
/// in how many grants there are to report, in the page size, and in which reader
/// answers.
#[tokio::test]
async fn a_grant_listing_reports_the_number_of_grants_it_carries() {
    let router = router();
    let key_id = text("setup", &create_key(&router, &json!({})).await, "KeyId");

    // The empty case first, where a client has no list to count for itself: no grant
    // exists yet, so every reader answers zero grants and must say so.
    for (operation, request) in read_requests(&key_id) {
        let answer = ok(&router, operation, &request).await;
        let listed = entries("nothing to report", &answer);
        assert!(
            listed.is_empty(),
            "case nothing to report / {operation}: expected no grants yet, got {listed:?}"
        );
        assert_eq!(
            answer["GrantCount"],
            json!(0),
            "case nothing to report / {operation}: expected GrantCount 0 beside an empty \
             list, got {answer}"
        );
    }

    // Rows: the number of grants the key carries, grown one grant at a time, and the
    // page size the read asks for — including no `Limit` at all, one that truncates and
    // one larger than the listing.
    for held in 1..=5usize {
        create_grant(
            &router,
            &format!("{held} grants"),
            &key_id,
            &["Decrypt"],
            None,
        )
        .await;

        for (operation, base) in read_requests(&key_id) {
            for limit in [None, Some(1usize), Some(2), Some(100)] {
                let case = format!("{held} grants, Limit {limit:?}");
                let mut request = base.clone();
                if let Some(limit) = limit {
                    request["Limit"] = json!(limit);
                }
                let answer = ok(&router, operation, &request).await;
                let listed = entries(&format!("{case} / {operation}"), &answer);
                let expected = limit.map_or(held, |limit| limit.min(held));
                assert_eq!(
                    listed.len(),
                    expected,
                    "case {case} / {operation}: expected {expected} grants on the page, got \
                     {}",
                    listed.len()
                );
                assert_eq!(
                    answer["GrantCount"],
                    json!(expected),
                    "case {case} / {operation}: expected GrantCount {expected}, got {answer}"
                );
            }
        }
    }
}

/// Rule 2: every entry carries `Name`, defaulting to the empty string.
#[tokio::test]
async fn every_grant_entry_reports_a_name_defaulting_to_the_empty_string() {
    let router = router();
    let key_id = text("setup", &create_key(&router, &json!({})).await, "KeyId");

    // Rows: the `Name` the grant was created with, or none at all. The named rows span
    // `GrantNameType`'s modelled `@length(1, 256)` and `^[a-zA-Z0-9:/_-]+$`
    // (`smithy_query(query="shape",shape="GrantNameType")`): the shortest name the
    // bound allows, the longest, and one using every character class the pattern
    // admits.
    let rows: Vec<(&str, Option<String>)> = vec![
        ("no Name at all", None),
        ("the shortest name", Some("a".to_string())),
        ("an ordinary name", Some("grant-of-the-reader".to_string())),
        (
            "every character class the pattern admits",
            Some("Grant:9/of_the-Reader".to_string()),
        ),
        ("the longest name", Some("n".repeat(256))),
    ];

    for (case, name) in rows {
        let answer = create_grant(&router, case, &key_id, &["Decrypt"], name.as_deref()).await;
        let created = grant_id(case, &answer);
        let expected = json!(name.clone().unwrap_or_default());

        for (operation, base) in read_requests(&key_id) {
            let mut request = base.clone();
            if operation == "ListGrants" {
                request["GrantId"] = json!(created);
            }
            let listed = listing(&router, case, operation, &request).await;
            let entry = listed
                .iter()
                .find(|entry| grant_id(case, entry) == created)
                .unwrap_or_else(|| {
                    panic!("case {case} / {operation}: expected the grant {created} in {listed:?}")
                });
            assert_eq!(
                entry["Name"], expected,
                "case {case} / {operation}: expected Name {expected}, got {entry}"
            );
            assert!(
                entry["Name"].is_string(),
                "case {case} / {operation}: Name must always be reported as a string, got \
                 {entry}"
            );
        }
    }
}

/// Rule 3: the permitted operations are reported in the order the enum declares them,
/// whatever order the request listed them in.
#[tokio::test]
async fn every_grant_entry_reports_its_operations_in_the_declared_order() {
    let router = router();

    // Rows: the key the grant is created on and the operations the request permits, in
    // the order the request lists them. They differ in how many operations are granted,
    // in whether the request's order is already the declared one, reversed or
    // arbitrary, in whether the declared order and the alphabetical one coincide, and
    // in which kind of key supports them — a grant may only permit operations the key
    // supports, which is `super::grants`' own rule.
    let rows: Vec<(&str, Value, &[&str])> = vec![
        (
            "two operations in reverse order",
            json!({}),
            &["Encrypt", "Decrypt"],
        ),
        ("one operation", json!({}), &["Decrypt"]),
        (
            "a data-key operation before Decrypt",
            json!({}),
            &["GenerateDataKey", "Decrypt"],
        ),
        (
            "a management operation before a cryptographic one",
            json!({}),
            &["DescribeKey", "Encrypt"],
        ),
        (
            "every operation a symmetric key supports, reversed",
            json!({}),
            &[
                "RetireGrant",
                "ReEncryptTo",
                "ReEncryptFrom",
                "GenerateDataKeyWithoutPlaintext",
                "GenerateDataKey",
                "Encrypt",
                "DescribeKey",
                "Decrypt",
                "CreateGrant",
            ],
        ),
        (
            "a signing key's operations",
            json!({"KeySpec": "RSA_4096", "KeyUsage": "SIGN_VERIFY"}),
            &["Verify", "Sign", "GetPublicKey"],
        ),
        (
            "an HMAC key's operations",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            &["VerifyMac", "GenerateMac"],
        ),
        (
            "a key-agreement key's operations",
            json!({"KeySpec": "ECC_NIST_P384", "KeyUsage": "KEY_AGREEMENT"}),
            &["GetPublicKey", "DeriveSharedSecret"],
        ),
    ];

    for (case, key_request, operations) in rows {
        for operation in operations {
            assert!(
                GRANT_OPERATION_DECLARATION_ORDER.contains(operation),
                "case {case}: {operation} must be a modelled GrantOperation"
            );
        }
        let key_id = text(case, &create_key(&router, &key_request).await, "KeyId");
        let answer = create_grant(&router, case, &key_id, operations, None).await;
        let created = grant_id(case, &answer);

        // What the entry must carry: the SAME operations, in the order the enum
        // declares them — computed from the row rather than written out beside it.
        let mut expected: Vec<&str> = operations.to_vec();
        expected.sort_by_key(|operation| declared_rank(case, operation));

        for (read, base) in read_requests(&key_id) {
            let mut request = base.clone();
            if read == "ListGrants" {
                request["GrantId"] = json!(created);
            }
            let listed = listing(&router, case, read, &request).await;
            let entry = listed
                .iter()
                .find(|entry| grant_id(case, entry) == created)
                .unwrap_or_else(|| {
                    panic!("case {case} / {read}: expected the grant {created} in {listed:?}")
                });
            let reported: Vec<String> = entry["Operations"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case} / {read}: expected Operations in {entry}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case} / {read}: an operation is a string, got {entry}")
                        })
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported, expected,
                "case {case} / {read}: expected Operations {expected:?}, got {reported:?}"
            );
            assert!(
                reported
                    .windows(2)
                    .all(|pair| declared_rank(case, &pair[0]) < declared_rank(case, &pair[1])),
                "case {case} / {read}: expected Operations in the declared order, got \
                 {reported:?}"
            );
        }
    }
}

/// Where `operation` comes in [`GRANT_OPERATION_DECLARATION_ORDER`].
fn declared_rank(case: &str, operation: &str) -> usize {
    GRANT_OPERATION_DECLARATION_ORDER
        .iter()
        .position(|declared| *declared == operation)
        .unwrap_or_else(|| panic!("case {case}: {operation} is a declared GrantOperation"))
}

/// Rule 4: a listing reports its grants ordered by `GrantId` ascending, and its pages
/// concatenate in that order.
#[tokio::test]
async fn a_grant_listing_is_ordered_by_grant_id() {
    let router = router();

    // Two keys, so the ordering is shown to hold of the listing a reader ANSWERS rather
    // than of one key's grants: `ListRetirableGrants` spans both keys.
    let first = text("setup", &create_key(&router, &json!({})).await, "KeyId");
    let second = text("setup", &create_key(&router, &json!({})).await, "KeyId");
    let mut on_first: Vec<String> = Vec::new();
    let mut everything: Vec<String> = Vec::new();
    for index in 0..6u32 {
        let key_id = if index % 2 == 0 { &first } else { &second };
        let answer = create_grant(&router, "setup", key_id, &["Decrypt"], None).await;
        let created = grant_id("setup", &answer);
        if key_id == &first {
            on_first.push(created.clone());
        }
        everything.push(created);
    }

    // The order each read must answer: its own selection, sorted by grant id — a grant
    // id is minted at random, so this is not the order the grants were created in.
    let mut expected_first = on_first.clone();
    expected_first.sort();
    let mut expected_all = everything.clone();
    expected_all.sort();

    // Rows: the reader, the request that selects its listing, and the grants that
    // listing holds in the order it must report them.
    let reads: Vec<(&str, Value, Vec<String>)> = vec![
        ("ListGrants", json!({"KeyId": first}), expected_first),
        (
            "ListRetirableGrants",
            json!({"RetiringPrincipal": principal(RETIRER)}),
            expected_all,
        ),
    ];

    for (operation, base, expected) in reads {
        // Rows: the page size, including one that divides the listing exactly, one that
        // does not, and one larger than it. Every page size must walk the same order.
        for limit in [None, Some(1usize), Some(2), Some(4), Some(100)] {
            let case = format!("{operation}, Limit {limit:?}");
            let mut collected: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            loop {
                let mut request = base.clone();
                if let Some(limit) = limit {
                    request["Limit"] = json!(limit);
                }
                if let Some(marker) = marker.as_deref() {
                    request["Marker"] = json!(marker);
                }
                let answer = ok(&router, operation, &request).await;
                let page = entries(&case, &answer);
                collected.extend(page.iter().map(|entry| grant_id(&case, entry)));
                if !answer["Truncated"].as_bool().unwrap_or(false) {
                    break;
                }
                marker = Some(text(&case, &answer, "NextMarker"));
            }
            assert_eq!(
                collected, expected,
                "case {case}: expected the grants by ascending grant id {expected:?}, got \
                 {collected:?}"
            );
            assert!(
                collected.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case}: expected a strictly ascending listing, got {collected:?}"
            );
        }
    }
}

/// Rule 5: `CreateGrant` reports the origin of the key the grant was created on, and it
/// is exactly the origin `DescribeKey` reports for that key.
#[tokio::test]
async fn create_grant_reports_the_origin_describe_key_reports_for_the_same_key() {
    let router = router();

    // Rows: the `CreateKey` request, which varies the key material KMS generates and —
    // in the last row — whether KMS generated it at all. `DescribeKey` is granted on
    // any key, so one operation serves every row.
    let generated: Vec<(&str, Value)> = vec![
        ("a symmetric encryption key", json!({})),
        (
            "an asymmetric signing key",
            json!({"KeySpec": "RSA_4096", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "an HMAC key",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        ("a multi-Region key", json!({"MultiRegion": true})),
    ];

    let mut reported_origins: Vec<String> = Vec::new();
    for (case, key_request) in generated {
        let metadata = create_key(&router, &key_request).await;
        let key_id = text(case, &metadata, "KeyId");
        reported_origins.push(assert_origin(&router, case, &key_id).await);
    }

    // A key whose material KMS did NOT generate: its origin is a different value, so a
    // row cannot pass by answering one constant.
    let case = "a key with imported material";
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let key_id = text(case, &metadata, "KeyId");
    import_material(&router, case, &key_id).await;
    let imported = assert_origin(&router, case, &key_id).await;
    assert_eq!(
        imported, "EXTERNAL",
        "case {case}: a key whose material was imported has the EXTERNAL origin, got \
         {imported}"
    );
    assert!(
        reported_origins.iter().all(|origin| origin != &imported),
        "case {case}: the origin must be read per key, but every key reported {imported}"
    );
}

/// Create a grant on `key_id` and assert the origin `CreateGrant` reports is the one
/// `DescribeKey` reports for that key, answering that origin.
async fn assert_origin(router: &MultiServiceRouter, case: &str, key_id: &str) -> String {
    let expected = text(case, &describe(router, key_id).await, "Origin");
    let answer = create_grant(router, case, key_id, &["DescribeKey"], None).await;
    let reported = text(case, &answer, "KeyOrigin");
    assert_eq!(
        reported, expected,
        "case {case}: CreateGrant must report the origin DescribeKey reports ({expected}), \
         got {reported}"
    );
    assert!(
        ORIGIN_TYPE_VALUES.contains(&reported.as_str()),
        "case {case}: the reported origin must be a modelled OriginType, got {reported}"
    );
    reported
}

/// Import freshly generated symmetric key material into the `EXTERNAL`-origin key
/// `key_id`, which is what makes such a key usable.
async fn import_material(router: &MultiServiceRouter, case: &str, key_id: &str) {
    let wrapping_algorithm = "RSAES_OAEP_SHA_256";
    let parameters = ok(
        router,
        "GetParametersForImport",
        &json!({
            "KeyId": key_id,
            "WrappingAlgorithm": wrapping_algorithm,
            "WrappingKeySpec": "RSA_2048",
        }),
    )
    .await;
    let public_key = unb64(case, &parameters, "PublicKey");
    let import_token = text(case, &parameters, "ImportToken");
    let material = service_com_amazonaws_kms_trentservice::state::random_bytes(
        SYMMETRIC_MATERIAL_LEN,
    )
    .unwrap_or_else(|| panic!("case {case}: the platform produces entropy"));
    let wrapped = wrap(case, &public_key, wrapping_algorithm, &material);
    ok(
        router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": import_token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
        }),
    )
    .await;
}

/// The model snapshot is untouched: it declares neither `ListGrantsResponse$GrantCount`
/// nor `CreateGrantResponse$KeyOrigin`, which is why this crate declares those members
/// itself — and it still declares `GrantListEntry$Name` and the `GrantOperation` enum
/// the projection orders by.
///
/// This pins the REASON the two members are written locally. If a refreshed snapshot
/// ever declares one of them, this test fails, and the local declaration should give way
/// to the modelled member rather than the two drifting apart.
#[test]
fn the_model_snapshot_declares_neither_grant_count_nor_the_create_grant_key_origin() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("smithy-model/com-amazonaws-kms-trentservice.json");
    let model: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the model snapshot is readable"),
    )
    .expect("the model snapshot is JSON");
    let shapes = &model["shapes"];

    // Rows: the response shape, and exactly the members the snapshot declares on it.
    let responses: Vec<(&str, Vec<&str>)> = vec![
        (
            "com.amazonaws.kms#ListGrantsResponse",
            vec!["Grants", "NextMarker", "Truncated"],
        ),
        (
            "com.amazonaws.kms#CreateGrantResponse",
            vec!["GrantId", "GrantToken"],
        ),
    ];
    for (shape, published) in responses {
        let members = shapes[shape]["members"]
            .as_object()
            .unwrap_or_else(|| panic!("{shape} declares members"));
        let mut declared: Vec<&str> = members.keys().map(String::as_str).collect();
        declared.sort_unstable();
        let mut expected = published.clone();
        expected.sort_unstable();
        assert_eq!(
            declared, expected,
            "the snapshot must be as shipped: {shape} declares {expected:?}, got {declared:?}"
        );
    }

    // `Name` IS modelled on the entry, so the empty string a nameless grant reports is
    // this projection's choice of value for a member the model already has.
    assert_eq!(
        shapes["com.amazonaws.kms#GrantListEntry"]["members"]["Name"]["target"],
        json!("com.amazonaws.kms#GrantNameType"),
        "the published Name member must still be declared as it was"
    );

    // The enum whose declared values the reported `Operations` list is ordered by: the
    // ORDER is this suite's own statement of the rule, but the SET must be exactly the
    // snapshot's.
    let mut operations: Vec<&str> = shapes["com.amazonaws.kms#GrantOperation"]["members"]
        .as_object()
        .expect("GrantOperation declares members")
        .keys()
        .map(String::as_str)
        .collect();
    operations.sort_unstable();
    let mut published = GRANT_OPERATION_DECLARATION_ORDER.to_vec();
    published.sort_unstable();
    assert_eq!(
        operations, published,
        "GrantOperation must keep exactly its published values, got {operations:?}"
    );
}
