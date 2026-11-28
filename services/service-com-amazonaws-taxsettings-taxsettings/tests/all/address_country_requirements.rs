// SPDX-License-Identifier: CC0-1.0

//! The per-country requirements the model states on the MEMBERS of a
//! `TaxRegistrationEntry`, over the wire.
//!
//! TWO rules, each asserted over a table that spans what it generalises over,
//! through BOTH writers of an entry (`PutTaxRegistration` and
//! `BatchPutTaxRegistration`), because the rule set is shared:
//!
//! 1. A submitted legal address held in a jurisdiction whose registration the
//!    model documents `stateOrRegion` as required for must name one: an entry
//!    that omits it is refused with `ValidationException` (HTTP 400,
//!    `MissingInput`) naming that parameter and nothing is stored, while an
//!    address held anywhere else, or one the service fills in itself, is stored
//!    without it. The rows differ along the jurisdiction (each country of the
//!    documented set, two countries outside it), the registration type the
//!    Brazilian requirement turns on, the case the country code is spelled in,
//!    and whether the address was submitted at all.
//! 2. A Canadian entry that claims a provincial sales tax ID for a TRN held in
//!    one of the provinces where that claim is a reseller claim must confirm
//!    that purchases are for resale: an entry that does not is refused with
//!    `ValidationException` (HTTP 400, `FieldValidationFailed`) naming that
//!    parameter. The rows differ along the province (each of the three, by name
//!    and by subdivision code, and two provinces outside the set), whether the
//!    ID is claimed, and the value of the confirmation.
//!
//! The source of both rule statements is the model:
//! `smithy_query(query="doc",shape="com.amazonaws.taxsettings#Address",member="stateOrRegion")`
//! and
//! `smithy_query(query="shape",shape="com.amazonaws.taxsettings#CanadaAdditionalInfo")`.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs` and
//! the two writers in `country_rules.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    CANADA_RESELLER_PROVINCES, ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_MISSING_INPUT,
};

use crate::country_rules::{write, WRITERS};
use crate::tax_registration_lifecycle::{call, emulator, field_list, Answer};

/// What the service must answer for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The entry is stored and reads back.
    Stored,
    /// The entry is refused with `ValidationException` (400) carrying
    /// `error_code` and naming `parameter`, and nothing is stored.
    Refused {
        error_code: &'static str,
        parameter: &'static str,
    },
}

/// The refusal of a member a jurisdiction requires and the entry omits.
const MISSING_STATE_OR_REGION: Expect = Expect::Refused {
    error_code: ERROR_CODE_MISSING_INPUT,
    parameter: "STATE_OR_REGION",
};

/// The refusal of a confirmation the claim the entry makes requires.
const UNCONFIRMED_RESALE: Expect = Expect::Refused {
    error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
    parameter: "IS_RESELLER_ACCOUNT",
};

/// One entry, and what both writers must answer for it.
struct Case {
    name: &'static str,
    entry: Value,
    expect: Expect,
}

/// A legal address held in `country_code`, naming a state, region or province
/// only when `state_or_region` is given.
fn address(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut address = json!({
        "addressLine1": "1 Example Street",
        "city": "Example City",
        "postalCode": "10115",
        "countryCode": country_code,
    });
    if let Some(value) = state_or_region {
        address
            .as_object_mut()
            .expect("an object")
            .insert("stateOrRegion".to_string(), json!(value));
    }
    address
}

/// An entry of `registration_type` whose legal address is held in
/// `country_code`.
fn entry(registration_type: &str, country_code: &str, state_or_region: Option<&str>) -> Value {
    json!({
        "registrationId": crate::fidelity_rules::number_of_type(
            registration_type,
            Some(country_code),
        ),
        "registrationType": registration_type,
        "legalName": "Example Holdings",
        "legalAddress": address(country_code, state_or_region),
    })
}

/// An entry held in the United Arab Emirates: the address of [`entry`], plus the
/// country block that jurisdiction's own documentation requires ("You must
/// specify the `taxRegistrationNumberType` in the
/// `unitedArabEmiratesAdditionalInfo` field of the `additionalTaxInformation`
/// object"), so what its rows vary is the address member alone.
fn emirates_entry(state_or_region: Option<&str>) -> Value {
    let mut entry = entry("VAT", "AE", state_or_region);
    entry.as_object_mut().expect("an object").insert(
        "additionalTaxInformation".to_string(),
        json!({
            "unitedArabEmiratesAdditionalInfo": {
                "taxRegistrationNumberType": "VATTaxRegistrationNumber",
            },
        }),
    );
    entry
}

/// The rows of rule 1.
///
/// The countries that require the member are the ones the member's own
/// documentation names ("only required for Canada, India, United Arab Emirates,
/// Romania, and Brazil (CPF)"); the countries that do not are two the same
/// sentence leaves optional.
fn state_or_region_cases() -> Vec<Case> {
    vec![
        Case {
            name: "Canada with no stateOrRegion",
            entry: entry("GST", "CA", None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "Canada with a stateOrRegion",
            entry: entry("GST", "CA", Some("Ontario")),
            expect: Expect::Stored,
        },
        Case {
            // The country code's own pattern admits either case, so either
            // spelling identifies the same jurisdiction.
            name: "Canada in lower case with no stateOrRegion",
            entry: entry("GST", "ca", None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "India with no stateOrRegion",
            entry: entry("GST", "IN", None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "India with a stateOrRegion",
            entry: entry("GST", "IN", Some("Karnataka")),
            expect: Expect::Stored,
        },
        Case {
            name: "the United Arab Emirates with no stateOrRegion",
            entry: emirates_entry(None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "the United Arab Emirates with a stateOrRegion",
            entry: emirates_entry(Some("Dubai")),
            expect: Expect::Stored,
        },
        Case {
            name: "Romania with no stateOrRegion",
            entry: entry("VAT", "RO", None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "Romania with a stateOrRegion",
            entry: entry("VAT", "RO", Some("Bucuresti")),
            expect: Expect::Stored,
        },
        Case {
            // Brazil is in the sentence for ONE registration type, so the type
            // is what decides a Brazilian entry.
            name: "Brazil CPF with no stateOrRegion",
            entry: entry("CPF", "BR", None),
            expect: MISSING_STATE_OR_REGION,
        },
        Case {
            name: "Brazil CPF with a stateOrRegion",
            entry: entry("CPF", "BR", Some("Rio de Janeiro")),
            expect: Expect::Stored,
        },
        Case {
            name: "Brazil CNPJ with no stateOrRegion",
            entry: entry("CNPJ", "BR", None),
            expect: Expect::Stored,
        },
        Case {
            name: "Brazil VAT with no stateOrRegion",
            entry: entry("VAT", "BR", None),
            expect: Expect::Stored,
        },
        Case {
            // A country the sentence leaves the member optional for.
            name: "Germany with no stateOrRegion",
            entry: entry("VAT", "DE", None),
            expect: Expect::Stored,
        },
        Case {
            name: "Japan with no stateOrRegion",
            entry: entry("VAT", "JP", None),
            expect: Expect::Stored,
        },
        Case {
            // An Indian PAN registration's legal address is the service's to
            // fill in, so the entry submits none and the requirement has no
            // submitted address to be read against.
            name: "an Indian PAN registration that submits no address at all",
            entry: json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
                "legalName": "Example Holdings",
            }),
            expect: Expect::Stored,
        },
    ]
}

/// The rows of rule 2: a Canadian entry whose province, provincial sales tax ID
/// and resale confirmation vary.
///
/// The provinces the rule turns on are the ones
/// `CanadaAdditionalInfo$isResellerAccount` names ("British Columbia,
/// Saskatchewan, or Manitoba"); Quebec and Ontario are outside that set.
fn canada_reseller_cases() -> Vec<Case> {
    /// A Canadian entry whose address names `province` and whose
    /// `canadaAdditionalInfo` carries the members given.
    fn canadian(
        province: &str,
        provincial_sales_tax_id: Option<&str>,
        is_reseller_account: Option<bool>,
    ) -> Value {
        let mut canada = json!({});
        let members = canada.as_object_mut().expect("an object");
        if let Some(id) = provincial_sales_tax_id {
            members.insert("provincialSalesTaxId".to_string(), json!(id));
        }
        if let Some(confirmed) = is_reseller_account {
            members.insert("isResellerAccount".to_string(), json!(confirmed));
        }
        json!({
            "registrationId": "123456789RT0001",
            "registrationType": "GST",
            "legalName": "Example Canada Inc",
            "legalAddress": address("CA", Some(province)),
            "additionalTaxInformation": { "canadaAdditionalInfo": canada },
        })
    }

    vec![
        Case {
            name: "British Columbia claiming a provincial sales tax ID with no confirmation",
            entry: canadian("British Columbia", Some("PST-1234567"), None),
            expect: UNCONFIRMED_RESALE,
        },
        Case {
            name: "British Columbia by subdivision code, claiming one with no confirmation",
            entry: canadian("BC", Some("PST-1234567"), None),
            expect: UNCONFIRMED_RESALE,
        },
        Case {
            name: "Saskatchewan claiming one and declining to confirm",
            entry: canadian("Saskatchewan", Some("PST-7654321"), Some(false)),
            expect: UNCONFIRMED_RESALE,
        },
        Case {
            name: "Manitoba claiming one with no confirmation",
            entry: canadian("Manitoba", Some("123456-7"), None),
            expect: UNCONFIRMED_RESALE,
        },
        Case {
            name: "British Columbia claiming one and confirming resale",
            entry: canadian("British Columbia", Some("PST-1234567"), Some(true)),
            expect: Expect::Stored,
        },
        Case {
            name: "Manitoba claiming one and confirming resale",
            entry: canadian("Manitoba", Some("123456-7"), Some(true)),
            expect: Expect::Stored,
        },
        Case {
            // The rule turns on the claim, not on the province alone.
            name: "British Columbia claiming no provincial sales tax ID",
            entry: canadian("British Columbia", None, None),
            expect: Expect::Stored,
        },
        Case {
            name: "Saskatchewan claiming none and declining to confirm",
            entry: canadian("Saskatchewan", None, Some(false)),
            expect: Expect::Stored,
        },
        Case {
            // A province outside the set: the claim stands on its own there.
            name: "Quebec claiming a provincial sales tax ID with no confirmation",
            entry: canadian("Quebec", Some("1234567890TQ0001"), None),
            expect: Expect::Stored,
        },
        Case {
            name: "Ontario claiming a provincial sales tax ID with no confirmation",
            entry: canadian("Ontario", Some("PST-1234567"), None),
            expect: Expect::Stored,
        },
    ]
}

/// Drive `case` through both writers and assert the answer, and what the store
/// holds afterwards.
async fn assert_case(case: &Case, context: &str) {
    for writer in WRITERS {
        let label = format!("{}{context} via {writer}", case.name);
        let router = emulator();
        let account_id = "000000000001";
        let answer = write(&router, writer, account_id, &case.entry).await;
        let read = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        match case.expect {
            Expect::Stored => {
                assert_eq!(
                    answer.status, 200,
                    "case {label:?}: expected the entry to be stored, got {answer:?}"
                );
                assert_eq!(
                    read.body
                        .pointer("/taxRegistration/registrationId")
                        .and_then(Value::as_str),
                    case.entry.get("registrationId").and_then(Value::as_str),
                    "case {label:?}: expected the stored registration to read back, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.body
                        .pointer("/taxRegistration/legalAddress/stateOrRegion")
                        .and_then(Value::as_str),
                    case.entry
                        .pointer("/legalAddress/stateOrRegion")
                        .and_then(Value::as_str),
                    "case {label:?}: expected the submitted stateOrRegion to read back, got {:?}",
                    read.body
                );
            }
            Expect::Refused {
                error_code,
                parameter,
            } => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {label:?}: expected 400 ValidationException, got {answer:?}"
                );
                assert_eq!(
                    answer.get("errorCode").as_str(),
                    Some(error_code),
                    "case {label:?}: expected errorCode {error_code:?}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    field_list(&answer),
                    vec![parameter.to_string()],
                    "case {label:?}: expected fieldList to name {parameter:?}, got {:?}",
                    answer.body
                );
                assert!(
                    message(&answer).is_some_and(|message| !message.trim().is_empty()),
                    "case {label:?}: expected the refusal to carry a message, got {:?}",
                    answer.body
                );
                assert_eq!(
                    read.status, 404,
                    "case {label:?}: a refused entry must store nothing, got {read:?}"
                );
            }
        }
    }
}

/// The refusal's message, if it carries one.
fn message(answer: &Answer) -> Option<&str> {
    answer.get("message").as_str()
}

/// RULE: a submitted legal address names its state, region or province exactly
/// where the jurisdiction it is held in requires one.
#[tokio::test]
async fn an_address_names_its_state_or_region_where_its_jurisdiction_requires_one() {
    for case in state_or_region_cases() {
        assert_case(&case, "").await;
    }
}

/// RULE: a Canadian provincial sales tax claim held in a province where that
/// claim is a reseller claim must confirm that purchases are for resale.
#[tokio::test]
async fn a_canadian_provincial_sales_tax_claim_confirms_resale() {
    for case in canada_reseller_cases() {
        assert_case(&case, "").await;
    }
}

/// RULE (coverage): every province the reseller rule turns on is driven by the
/// table above, under its name and under its subdivision code, so the rule is
/// asserted over the documented set rather than over the one province the
/// member's documentation is usually read for.
#[tokio::test]
async fn every_reseller_province_is_refused_without_a_confirmation() {
    for (code, province) in CANADA_RESELLER_PROVINCES {
        let upper = province.to_ascii_uppercase();
        for spelling in [*code, *province, upper.as_str()] {
            let case = Case {
                name: "a reseller province claiming a provincial sales tax ID",
                entry: json!({
                    "registrationId": "123456789RT0001",
                    "registrationType": "GST",
                    "legalName": "Example Canada Inc",
                    "legalAddress": address("CA", Some(spelling)),
                    "additionalTaxInformation": {
                        "canadaAdditionalInfo": { "provincialSalesTaxId": "PST-1234567" },
                    },
                }),
                expect: UNCONFIRMED_RESALE,
            };
            assert_case(&case, &format!(" spelled {spelling:?}")).await;
        }
    }
}
