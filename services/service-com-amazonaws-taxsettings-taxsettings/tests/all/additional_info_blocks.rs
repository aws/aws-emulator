// SPDX-License-Identifier: CC0-1.0

//! The `additionalTaxInformation` object's own rule, over the restJson1 wire.
//!
//! ONE rule: an `additionalTaxInformation` object a write submits must carry
//! exactly ONE country block, and that block must be the one belonging to the
//! jurisdiction the entry identifies; an object with no block, with more than
//! one, or with a block of another jurisdiction is refused with
//! `ValidationException` (HTTP 400) carrying `FieldValidationFailed` and nothing
//! is persisted.
//!
//! The source of the rule is the shape's own documentation
//! (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#AdditionalInfoRequest")`):
//! "You can only specify one of the following parameters and the value can't be
//! empty", and "The parameter that you specify must match the country for the
//! TRN".
//!
//! The tables span the class the rule generalises over rather than the one
//! reported request (a Polish entry carrying both `polandAdditionalInfo` and
//! `canadaAdditionalInfo`, which is one row of `count_cases`): EVERY country
//! block the model declares is driven through the jurisdiction rule, the block
//! count is driven at 0, 2 and 3 blocks, and the accepted side — one block
//! matching the jurisdiction — is driven for blocks the issue never cited and for
//! a lower-case `countryCode`.
//!
//! The harness (`emulator`, `call`, `field_list`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, ADDITIONAL_INFO_BLOCKS, ERROR_CODE_FIELD_VALIDATION_FAILED, STATUS_PENDING,
};

use crate::declared_member_defaults::country_block_as_stored;
use crate::tax_registration_lifecycle::{call, emulator, field_list};

/// One well-formed instance of each country block `AdditionalInfoRequest`
/// declares, keyed by the member that carries it.
///
/// Every `@required` member of each block is filled, and every value satisfies
/// the member's own `@pattern` / `@length` / enum, so what decides the answer is
/// the object's rule rather than a constraint the boundary validator enforces.
/// The source of each shape's members is
/// `smithy_query(query="shape",shape="com.amazonaws.taxsettings#<Block>")`;
/// `every_declared_block_is_exercised` pins that this catalog covers the model's
/// member list row for row, so a block added to the model cannot be left untested
/// here.
fn block_catalog() -> Vec<(&'static str, Value)> {
    vec![
        (
            "malaysiaAdditionalInfo",
            json!({ "taxInformationNumber": "C1234567890" }),
        ),
        (
            "israelAdditionalInfo",
            json!({ "dealerType": "Authorized", "customerType": "Business" }),
        ),
        (
            "estoniaAdditionalInfo",
            json!({ "registryCommercialCode": "12345678" }),
        ),
        (
            "canadaAdditionalInfo",
            json!({ "canadaQuebecSalesTaxNumber": "1234567890TQ0001" }),
        ),
        (
            "spainAdditionalInfo",
            json!({ "registrationType": "Intra-EU" }),
        ),
        (
            "kenyaAdditionalInfo",
            json!({ "personType": "Business" }),
        ),
        (
            "southKoreaAdditionalInfo",
            // Every member of this block admits Hangul, digits and punctuation
            // alone (`^[0-9\u{3130}-\u{318F}\u{AC00}-\u{D7AF},.( )-\s]*$`), so
            // the instance is written in the script the member's pattern admits.
            json!({
                "businessRepresentativeName": "홍길동",
                "lineOfBusiness": "소프트웨어",
                "itemOfBusiness": "클라우드",
            }),
        ),
        (
            "turkeyAdditionalInfo",
            json!({ "taxOffice": "Kadikoy Vergi Dairesi" }),
        ),
        (
            "georgiaAdditionalInfo",
            json!({ "personType": "Business" }),
        ),
        ("italyAdditionalInfo", json!({ "sdiAccountId": "ABC123" })),
        (
            "romaniaAdditionalInfo",
            json!({ "taxRegistrationNumberType": "TaxRegistrationNumber" }),
        ),
        (
            "ukraineAdditionalInfo",
            json!({ "ukraineTrnType": "Business" }),
        ),
        (
            "polandAdditionalInfo",
            json!({ "taxRegistrationNumberType": "LocalRegistrationNumber" }),
        ),
        (
            "saudiArabiaAdditionalInfo",
            json!({ "taxRegistrationNumberType": "TaxRegistrationNumber" }),
        ),
        (
            "indonesiaAdditionalInfo",
            json!({ "taxRegistrationNumberType": "NPWP" }),
        ),
        (
            "vietnamAdditionalInfo",
            json!({ "enterpriseIdentificationNumber": "1234567890" }),
        ),
        (
            "egyptAdditionalInfo",
            json!({
                "uniqueIdentificationNumber":
                    "123456789012345678901234567890123456789",
            }),
        ),
        (
            "greeceAdditionalInfo",
            json!({ "contractingAuthorityCode": "1234.A56789.0123" }),
        ),
        (
            "uzbekistanAdditionalInfo",
            json!({ "vatRegistrationNumber": "123456789012" }),
        ),
        (
            "philippinesAdditionalInfo",
            json!({ "isVatRegistered": true }),
        ),
        ("belgiumAdditionalInfo", json!({ "peppolId": "1234567890" })),
        ("chileAdditionalInfo", json!({ "documentType": "Invoice" })),
        (
            "franceAdditionalInfo",
            json!({ "sirenNumber": "123456789" }),
        ),
        (
            "monacoAdditionalInfo",
            json!({ "businessNumber": "MC1234567" }),
        ),
        (
            "germanyAdditionalInfo",
            json!({ "economicIdentificationNumber": "DE123456789" }),
        ),
        ("mexicoAdditionalInfo", json!({ "taxRegime": "601" })),
        (
            "unitedArabEmiratesAdditionalInfo",
            json!({ "taxRegistrationNumberType": "VATTaxRegistrationNumber" }),
        ),
        (
            "norwayAdditionalInfo",
            json!({ "taxRegistrationNumberType": "TaxRegistrationNumber" }),
        ),
        (
            "newZealandAdditionalInfo",
            json!({ "peppolId": "1234567890123" }),
        ),
    ]
}

/// The catalog instance of one block.
fn block(member: &str) -> Value {
    block_catalog()
        .into_iter()
        .find(|(name, _)| *name == member)
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("{member} is in the block catalog"))
}

/// An entry held in `country_code` carrying `additional`, with every member the
/// jurisdiction-independent checks ask for.
///
/// `registrationType` is `VAT`, which the model ties to no single country, so the
/// jurisdiction of each row is the `countryCode` alone.
fn entry(country_code: &str, additional: Value) -> Value {
    json!({
        "registrationId": "VAT1234567890",
        "registrationType": "VAT",
        "legalName": "Example Holdings",
        "legalAddress": { "postalCode": "00001", "countryCode": country_code },
        "additionalTaxInformation": additional,
    })
}

/// RULE (completeness): the catalog above carries one instance of every country
/// block the model declares — the set `crate::state::ADDITIONAL_INFO_BLOCKS` is
/// read off — so the tables below span the whole member list rather than the
/// blocks one request happened to carry.
#[test]
fn every_declared_block_is_exercised() {
    let catalogued: BTreeSet<&str> = block_catalog().iter().map(|(name, _)| *name).collect();
    let declared: BTreeSet<&str> = ADDITIONAL_INFO_BLOCKS
        .iter()
        .map(|block| block.member)
        .collect();
    assert_eq!(
        catalogued, declared,
        "the block catalog must carry exactly the blocks the model declares, got {catalogued:?} \
         against {declared:?}"
    );
}

/// RULE: a block that does not belong to the jurisdiction the entry identifies is
/// refused, naming that block, and nothing is persisted.
///
/// Driven for EVERY declared block: each row holds its registration in one
/// block's country while carrying the NEXT block's object, so every member of the
/// model's list is exercised both as the jurisdiction and as the foreign block.
#[tokio::test]
async fn a_block_for_another_jurisdiction_is_refused() {
    let blocks = ADDITIONAL_INFO_BLOCKS;
    for (index, held_in) in blocks.iter().enumerate() {
        let foreign = &blocks[(index + 1) % blocks.len()];
        let case = format!(
            "a registration in {} carrying {}",
            held_in.country_code, foreign.member
        );
        let router = emulator();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": account_id,
                "taxRegistrationEntry": entry(
                    held_in.country_code,
                    json!({ foreign.member: block(foreign.member) }),
                ),
            }),
        )
        .await;

        assert_eq!(
            (put.status, put.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {:?}",
            put.body
        );
        assert_eq!(
            put.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {case:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
            put.body
        );
        assert_eq!(
            field_list(&put),
            vec![parameter_name(foreign.member)],
            "case {case:?}: expected fieldList to name the foreign block, got {:?}",
            put.body
        );
        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused entry must persist nothing, got {:?}",
            got.body
        );
    }
}

/// The jurisdictions the model declares NO block for, each with a block some
/// other country owns. `countryCode` is free of any country list in the model
/// (its pattern is `^[a-zA-Z]+$`), so these are ordinary jurisdictions whose
/// registrations simply take no country block.
const JURISDICTIONS_WITH_NO_BLOCK: &[(&str, &str)] = &[
    ("GB", "italyAdditionalInfo"),
    ("US", "canadaAdditionalInfo"),
    ("JP", "philippinesAdditionalInfo"),
    ("NP", "indonesiaAdditionalInfo"),
];

/// RULE (the same rule, where the jurisdiction owns no block at all): a block is
/// refused for a registration held in a country the model declares no block for,
/// because no block matches that country.
#[tokio::test]
async fn a_jurisdiction_with_no_block_takes_none() {
    for (country_code, member) in JURISDICTIONS_WITH_NO_BLOCK {
        let case = format!("a registration in {country_code} carrying {member}");
        let router = emulator();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": account_id,
                "taxRegistrationEntry": entry(
                    country_code,
                    json!({ *member: block(member) }),
                ),
            }),
        )
        .await;

        assert_eq!(
            (put.status, put.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {:?}",
            put.body
        );
        assert_eq!(
            put.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {case:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
            put.body
        );
        assert_eq!(
            field_list(&put),
            vec![parameter_name(member)],
            "case {case:?}: expected fieldList to name the block, got {:?}",
            put.body
        );
        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused entry must persist nothing, got {:?}",
            got.body
        );
    }
}

/// The objects whose BLOCK COUNT breaks the rule, each with the jurisdiction the
/// entry is held in. The rows differ in the count (none, two, three) and in which
/// blocks are combined, including the matching block paired with a foreign one.
fn count_cases() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        // The reported request: a Polish entry carrying its own block AND
        // Canada's.
        (
            "the jurisdiction's own block and a foreign one",
            "PL",
            json!({
                "polandAdditionalInfo": block("polandAdditionalInfo"),
                "canadaAdditionalInfo": block("canadaAdditionalInfo"),
            }),
        ),
        ("an object with no block at all", "PL", json!({})),
        (
            "an object with no block at all, in a country whose block is required",
            "KE",
            json!({}),
        ),
        (
            "two foreign blocks",
            "DE",
            json!({
                "italyAdditionalInfo": block("italyAdditionalInfo"),
                "greeceAdditionalInfo": block("greeceAdditionalInfo"),
            }),
        ),
        (
            "three blocks, one of them the jurisdiction's own",
            "KE",
            json!({
                "kenyaAdditionalInfo": block("kenyaAdditionalInfo"),
                "georgiaAdditionalInfo": block("georgiaAdditionalInfo"),
                "italyAdditionalInfo": block("italyAdditionalInfo"),
            }),
        ),
        (
            "every block the catalog carries at once",
            "PL",
            Value::Object(
                block_catalog()
                    .into_iter()
                    .map(|(member, body)| (member.to_string(), body))
                    .collect(),
            ),
        ),
    ]
}

/// RULE: an object that does not carry exactly one block is refused, naming the
/// object itself, and nothing is persisted.
#[tokio::test]
async fn an_object_without_exactly_one_block_is_refused() {
    for (case, country_code, additional) in count_cases() {
        let router = emulator();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": account_id,
                "taxRegistrationEntry": entry(country_code, additional.clone()),
            }),
        )
        .await;

        assert_eq!(
            (put.status, put.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {:?}",
            put.body
        );
        assert_eq!(
            put.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {case:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
            put.body
        );
        assert_eq!(
            field_list(&put),
            vec![parameter_name("additionalTaxInformation")],
            "case {case:?}: expected fieldList to name the object, got {:?}",
            put.body
        );
        assert!(
            put.get("message")
                .as_str()
                .is_some_and(|message| !message.trim().is_empty()),
            "case {case:?}: expected the refusal to carry a message, got {:?}",
            put.body
        );

        // The batch writer judges one entry exactly as the single-account writer
        // does, so the same object refuses the whole call there too.
        let batch = call(
            &router,
            "BatchPutTaxRegistration",
            &json!({
                "accountIds": [account_id],
                "taxRegistrationEntry": entry(country_code, additional),
            }),
        )
        .await;
        assert_eq!(
            (batch.status, batch.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: the batch writer must refuse the same entry, got {:?}",
            batch.body
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused entry must persist nothing, got {:?}",
            got.body
        );
    }
}

/// The entries the accepted side stores: one block, belonging to the jurisdiction
/// the entry identifies. The rows differ in the block, in the registration type
/// and in the case of `countryCode`, and every country here is one whose own
/// business rules the entry satisfies.
fn accepted_cases() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        // The reported request, with the one block the model admits for it.
        (
            "Poland with its own block",
            "polandAdditionalInfo",
            json!({
                "registrationId": "1234563218",
                "registrationType": "NIP",
                "legalName": "Przyklad Sp. z o.o.",
                "sector": "Business",
                "legalAddress": { "postalCode": "00-001", "countryCode": "PL" },
                "additionalTaxInformation": {
                    "polandAdditionalInfo": { "taxRegistrationNumberType": "LocalRegistrationNumber" },
                },
            }),
        ),
        // The same entry with the jurisdiction written in lower case: the
        // `countryCode` pattern admits either case, so the block still matches.
        (
            "Poland in lower case with its own block",
            "polandAdditionalInfo",
            json!({
                "registrationId": "1234563218",
                "registrationType": "NIP",
                "legalName": "Przyklad Sp. z o.o.",
                "legalAddress": { "postalCode": "00-001", "countryCode": "pl" },
                "additionalTaxInformation": {
                    "polandAdditionalInfo": { "isGroupVatEnabled": true },
                },
            }),
        ),
        // A block the reported request never mentions.
        (
            "Italy with its own block",
            "italyAdditionalInfo",
            json!({
                "registrationId": "IT12345678901",
                "registrationType": "VAT",
                "legalName": "Esempio SRL",
                "legalAddress": { "postalCode": "00100", "countryCode": "IT" },
                "additionalTaxInformation": {
                    "italyAdditionalInfo": { "sdiAccountId": "ABC123", "customerType": "Business" },
                },
            }),
        ),
        (
            "Greece with its own block",
            "greeceAdditionalInfo",
            json!({
                "registrationId": "EL123456789",
                "registrationType": "VAT",
                "legalName": "Paradeigma AE",
                "legalAddress": { "postalCode": "10431", "countryCode": "GR" },
                "additionalTaxInformation": {
                    "greeceAdditionalInfo": { "contractingAuthorityCode": "1234.A56789.0123" },
                },
            }),
        ),
        // A block that a country rule of its own jurisdiction REQUIRES, so the
        // same object satisfies both rules.
        (
            "Monaco with its own, required block",
            "monacoAdditionalInfo",
            json!({
                "registrationId": "MC12345678901",
                "registrationType": "VAT",
                "legalName": "Exemple SAM",
                "legalAddress": { "postalCode": "98000", "countryCode": "MC" },
                "additionalTaxInformation": {
                    "monacoAdditionalInfo": { "businessNumber": "MC1234567" },
                },
            }),
        ),
        // The one block with a member that declares a default, which the stored
        // record materialises.
        (
            "Malaysia with its own block",
            "malaysiaAdditionalInfo",
            json!({
                "registrationId": "C1234567890",
                "registrationType": "TIN",
                "legalName": "Contoh Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "businessRegistrationNumber": "202001001234" },
                },
            }),
        ),
    ]
}

/// RULE (the other side): an object carrying exactly ONE block, the one belonging
/// to the entry's own jurisdiction, is stored and reads back carrying that block
/// and no other.
#[tokio::test]
async fn one_block_for_the_entry_jurisdiction_is_stored() {
    for (case, member, submitted) in accepted_cases() {
        let router = emulator();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": submitted }),
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
        let submitted_block = submitted
            .pointer(&format!("/additionalTaxInformation/{member}"))
            .expect("every accepted case submits its own block");
        assert_eq!(
            got.body
                .pointer(&format!("/taxRegistration/additionalTaxInformation/{member}")),
            Some(&country_block_as_stored(member, submitted_block)),
            "case {case:?}: expected the stored block to read back, got {:?}",
            got.body
        );
        let reported: Vec<String> = got
            .body
            .pointer("/taxRegistration/additionalTaxInformation")
            .and_then(Value::as_object)
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(
            reported,
            vec![member.to_string()],
            "case {case:?}: expected the read to report that block alone, got {:?}",
            got.body
        );
    }
}
