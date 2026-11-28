// SPDX-License-Identifier: CC0-1.0

//! Every country block the model declares on `AdditionalInfoRequest` is a block
//! a write may carry — over the restJson1 wire.
//!
//! ONE rule: the `additionalTaxInformation` object of a stored registration is
//! projected onto the response shape member for member, so a block the model
//! declares on the REQUEST shape is stored for a registration held in that
//! block's own jurisdiction and reported, unchanged and alone, by every reader of
//! the record; the blocks the model declares only on the RESPONSE shape
//! (`brazilAdditionalInfo`, `colombiaAdditionalInfo`, `indiaAdditionalInfo`) are
//! carried by no request and reported for none.
//!
//! The source of the member list is the model itself
//! (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#AdditionalInfoRequest")`
//! and `…#AdditionalInfoResponse`), which the first test reads off the snapshot in
//! this crate's `smithy-model/` and compares to the set the service places
//! (`crate::state::ADDITIONAL_INFO_BLOCKS`): a block a refreshed model adds is
//! then a failure here rather than a member no request can use and no read
//! reports.
//!
//! The table spans the class rather than one block: a row per block whose
//! jurisdiction states its own extra requirement (a document, an address member,
//! a sector, a TRN type) and a row per block whose jurisdiction states none, each
//! driven through `PutTaxRegistration`, `GetTaxRegistration` and
//! `ListTaxRegistrations`.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs`;
//! nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ADDITIONAL_INFO_BLOCKS, STATUS_PENDING,
};

use crate::country_requirements::certificate_document;
use crate::tax_registration_lifecycle::{call, emulator};

/// The blocks the model declares on the RESPONSE shape alone: AWS reports them
/// from what it holds for the account, and `AdditionalInfoRequest` declares no
/// member for any of them, so no request can submit one.
const RESPONSE_ONLY_BLOCKS: &[&str] = &[
    "brazilAdditionalInfo",
    "colombiaAdditionalInfo",
    "indiaAdditionalInfo",
];

/// The member names one shape of the snapshot declares, in the snapshot's own
/// spelling.
fn declared_members(shape: &str) -> BTreeSet<String> {
    let path = "smithy-model/com-amazonaws-taxsettings-taxsettings.json";
    let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("case {shape:?}: the model file reads back: {error}"));
    let model: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {shape:?}: the model file parses: {error}"));
    model["shapes"][format!("com.amazonaws.taxsettings#{shape}")]["members"]
        .as_object()
        .unwrap_or(&Map::new())
        .keys()
        .cloned()
        .collect()
}

/// RULE (completeness): the service places exactly the blocks the model declares
/// on `AdditionalInfoRequest`, so every member of that shape is a block a write
/// may carry and a read reports.
#[test]
fn every_request_block_the_model_declares_is_placed() {
    let declared = declared_members("AdditionalInfoRequest");
    let placed: BTreeSet<String> = ADDITIONAL_INFO_BLOCKS
        .iter()
        .map(|block| block.member.to_string())
        .collect();
    assert_eq!(
        placed, declared,
        "the placed blocks must be exactly the members AdditionalInfoRequest declares, got \
         {placed:?} against {declared:?}"
    );
}

/// RULE (completeness, the other shape): every member of
/// `AdditionalInfoResponse` is either a block a request can carry or one of the
/// response-only blocks, so no reported member is unaccounted for.
#[test]
fn every_response_block_is_a_request_block_or_response_only() {
    let declared = declared_members("AdditionalInfoResponse");
    let placed: BTreeSet<String> = ADDITIONAL_INFO_BLOCKS
        .iter()
        .map(|block| block.member.to_string())
        .chain(RESPONSE_ONLY_BLOCKS.iter().map(|&name| name.to_string()))
        .collect();
    assert_eq!(
        declared, placed,
        "every member AdditionalInfoResponse declares must be a request block or a \
         response-only block, got {declared:?} against {placed:?}"
    );
    for response_only in RESPONSE_ONLY_BLOCKS {
        assert!(
            !declared_members("AdditionalInfoRequest").contains(*response_only),
            "case {response_only:?}: a response-only block must not be a member of \
             AdditionalInfoRequest"
        );
    }
}

/// The entries the table stores: the block, and an entry held in that block's own
/// jurisdiction that satisfies every other rule of that country.
///
/// Each instance fills more than one member where the block declares more than
/// one, and every value satisfies the member's own `@pattern` / enum, so what the
/// row exercises is the projection rather than a constraint the boundary
/// validator enforces.
fn rows() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        // Germany: the two members of its own block, beside the TRN.
        (
            "Germany with both members of its block",
            "germanyAdditionalInfo",
            json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Beispiel GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                "additionalTaxInformation": {
                    "germanyAdditionalInfo": {
                        "economicIdentificationNumber": "DE123456789",
                        "steuernummer": "1012345678901",
                    },
                },
            }),
        ),
        // Mexico: its own block, for a country that also requires a document.
        (
            "Mexico with its block and the document its documentation requires",
            "mexicoAdditionalInfo",
            json!({
                "registrationId": "EJEM800101XX1",
                "registrationType": "VAT",
                "legalName": "Ejemplo SA de CV",
                "legalAddress": { "postalCode": "01000", "countryCode": "MX" },
                "verificationDetails": certificate_document(),
                "additionalTaxInformation": {
                    "mexicoAdditionalInfo": {
                        "taxRegime": "601",
                        "taxRegimeFullName": "General de Ley Personas Morales",
                        "cfdiUsageCode": "G01",
                    },
                },
            }),
        ),
        // The United Arab Emirates: the block its documentation REQUIRES, for a
        // country that also requires an address member.
        (
            "the United Arab Emirates with the TRN type its block must carry",
            "unitedArabEmiratesAdditionalInfo",
            json!({
                "registrationId": "100123456700003",
                "registrationType": "VAT",
                "legalName": "Example Trading LLC",
                "legalAddress": {
                    "stateOrRegion": "Dubai",
                    "postalCode": "00000",
                    "countryCode": "AE",
                },
                "additionalTaxInformation": {
                    "unitedArabEmiratesAdditionalInfo": {
                        "taxRegistrationNumberType": "VATTaxRegistrationNumber",
                        "peppolId": "1234567890",
                        "tradeLicenseNumber": "1234567890",
                        "emiratesId": "784-1234-1234567-1",
                    },
                },
            }),
        ),
        // Norway: the VAT type, for which the submitted number is not the
        // Organisation Number the other type names.
        (
            "Norway with its block",
            "norwayAdditionalInfo",
            json!({
                "registrationId": "123456789MVA",
                "registrationType": "VAT",
                "legalName": "Eksempel AS",
                "legalAddress": { "postalCode": "0150", "countryCode": "NO" },
                "additionalTaxInformation": {
                    "norwayAdditionalInfo": {
                        "taxRegistrationNumberType": "TaxRegistrationNumber",
                        "peppolId": "123456789",
                    },
                },
            }),
        ),
        // New Zealand: the sector its block's one member is accepted for.
        (
            "New Zealand with its block, as a government customer",
            "newZealandAdditionalInfo",
            json!({
                "registrationId": "123456789",
                "registrationType": "GST",
                "legalName": "Example NZ Agency",
                "sector": "Government",
                "legalAddress": { "postalCode": "6011", "countryCode": "NZ" },
                "additionalTaxInformation": {
                    "newZealandAdditionalInfo": { "peppolId": "9429038762371" },
                },
            }),
        ),
        // A block the model declared all along, driven through the same
        // assertions: the rule is about every member of the shape, not about the
        // ones a refresh added.
        (
            "Poland with its block",
            "polandAdditionalInfo",
            json!({
                "registrationId": "1234563218",
                "registrationType": "NIP",
                "legalName": "Przyklad Sp. z o.o.",
                "sector": "Business",
                "legalAddress": { "postalCode": "00-001", "countryCode": "PL" },
                "additionalTaxInformation": {
                    "polandAdditionalInfo": { "taxRegistrationNumberType": "EUTaxRegistrationNumber" },
                },
            }),
        ),
    ]
}

/// RULE: a block the model declares on the request shape is stored and reported,
/// unchanged and alone, by both readers of the record.
#[tokio::test]
async fn a_declared_block_round_trips_through_both_readers() {
    for (case, member, entry) in rows() {
        let router = emulator();
        let account_id = "000000000001";
        let submitted = entry
            .pointer(&format!("/additionalTaxInformation/{member}"))
            .unwrap_or_else(|| panic!("case {case:?}: every row submits its own block"))
            .clone();

        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            (put.status, put.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the entry to be stored, got {:?}",
            put.body
        );
        assert_eq!(
            put.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {case:?}: expected the write to report the status it stored, got {:?}",
            put.body
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected the stored registration to read back, got {:?}",
            got.body
        );
        assert_eq!(
            reported_blocks(got.body.pointer("/taxRegistration/additionalTaxInformation")),
            vec![(member.to_string(), submitted.clone())],
            "case {case:?}: expected GetTaxRegistration to report that block alone, got {:?}",
            got.body
        );

        let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the listing to answer, got {:?}",
            listed.body
        );
        assert_eq!(
            reported_blocks(
                listed
                    .body
                    .pointer("/accountDetails/0/taxRegistration/additionalTaxInformation")
            ),
            vec![(member.to_string(), submitted)],
            "case {case:?}: expected ListTaxRegistrations to report that block alone, got {:?}",
            listed.body
        );
    }
}

/// The blocks a reported `additionalTaxInformation` object carries, each with its
/// own body — the whole object, so a row asserting on it also asserts that no
/// other block, response-only or otherwise, was reported.
fn reported_blocks(reported: Option<&Value>) -> Vec<(String, Value)> {
    reported
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(member, body)| (member.clone(), body.clone()))
                .collect()
        })
        .unwrap_or_default()
}
