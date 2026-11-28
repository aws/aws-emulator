// SPDX-License-Identifier: CC0-1.0

//! The per-country business rules `PutTaxRegistration` documents for
//! `taxRegistrationEntry`, over the wire.
//!
//! ONE rule: an entry that does not satisfy the requirements the operation's
//! model documentation states for the country of its `legalAddress.countryCode`
//! is refused with `ValidationException` (HTTP 400) and nothing is persisted,
//! while an entry that satisfies them is stored and reads back.
//!
//! The table below spans the class that rule generalises over — every country
//! the documentation names a requirement for, each documented requirement of
//! that country (a missing member, a value outside the documented set, a member
//! the country refuses, and the conditional rules that turn on another member of
//! the same entry), and both sides of each: the entry that breaks the rule and
//! the entry that satisfies it. The recorded Kenyan entry carrying no
//! `kenyaAdditionalInfo.personType` is ONE row.
//!
//! The source of every row is
//! `smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`,
//! whose country-by-country list is also what `crate::state::COUNTRY_RULES`
//! carries, plus the per-country requirements the model states on the entry's
//! own members — `Address$stateOrRegion` and
//! `CanadaAdditionalInfo$isResellerAccount`, each quoted above its rows;
//! `every_documented_country_rule_is_exercised` pins that this table
//! covers that rule set row for row.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs`;
//! nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, COUNTRY_RULES, ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_MISSING_INPUT,
    STATUS_PENDING,
};

use crate::tax_registration_lifecycle::{call, emulator};

/// What the service must answer for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The country's rules admit the entry: it is stored and reads back.
    Stored,
    /// The country's rules refuse the entry: `ValidationException` (400)
    /// carrying `error_code` and naming `parameter` in `fieldList`, with
    /// nothing persisted.
    Refused {
        error_code: &'static str,
        parameter: &'static str,
    },
}

/// One entry submitted to `PutTaxRegistration`, and what must come back.
struct Case {
    name: &'static str,
    entry: Value,
    expect: Expect,
}

/// A tax registration certificate document, as `VerificationDetails` carries
/// one.
///
/// `pub(crate)` because `tests/all/derived_registration_members.rs` needs the
/// same document for the Mexican rows of its own table.
pub(crate) fn certificate_document() -> Value {
    json!({
        "taxRegistrationDocuments": [
            { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
        ],
    })
}

/// A refusal expectation for a member the country requires and the entry omits.
const fn missing(parameter: &'static str) -> Expect {
    Expect::Refused {
        error_code: ERROR_CODE_MISSING_INPUT,
        parameter,
    }
}

/// A refusal expectation for a member the entry carries with a value, or at all,
/// that the country does not admit.
const fn rejected_value(parameter: &'static str) -> Expect {
    Expect::Refused {
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        parameter,
    }
}

/// Every row of the table. Each entry satisfies every constraint the boundary
/// validator enforces, so what decides the answer is the country's own rules.
fn cases() -> Vec<Case> {
    vec![
        // Kenya: "You must specify the personType in the kenyaAdditionalInfo
        // field of the additionalTaxInformation object." The first row is the
        // recorded request.
        Case {
            name: "Kenya with no additionalTaxInformation at all",
            entry: json!({
                "registrationId": "P987654321K",
                "registrationType": "VAT",
                "legalName": "Sim2Real Kenya Missing Field Ltd",
                "legalAddress": {
                    "addressLine1": "Kenyatta Ave 2",
                    "city": "Nairobi",
                    "postalCode": "00100",
                    "countryCode": "KE",
                },
            }),
            expect: missing("PERSON_TYPE"),
        },
        Case {
            name: "Kenya with a lower-case country code and no personType",
            entry: json!({
                "registrationId": "P987654321K",
                "registrationType": "VAT",
                "legalName": "Example Kenya Ltd",
                "legalAddress": { "postalCode": "00100", "countryCode": "ke" },
            }),
            expect: missing("PERSON_TYPE"),
        },
        Case {
            name: "Kenya with a Legal Person personType",
            entry: json!({
                "registrationId": "P987654321K",
                "registrationType": "VAT",
                "legalName": "Example Kenya Ltd",
                "legalAddress": { "postalCode": "00100", "countryCode": "KE" },
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Legal Person" },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Kenya with a Business personType",
            entry: json!({
                "registrationId": "P987654321K",
                "registrationType": "VAT",
                "legalName": "Example Kenya Ltd",
                "legalAddress": { "postalCode": "00100", "countryCode": "KE" },
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Business" },
                },
            }),
            expect: Expect::Stored,
        },
        // Kenya: "If the personType is Physical Person, you must specify the tax
        // registration certificate document in the taxRegistrationDocuments
        // field of the VerificationDetails object."
        Case {
            name: "Kenya Physical Person with no certificate document",
            entry: json!({
                "registrationId": "A987654321K",
                "registrationType": "VAT",
                "legalName": "Wanjiku Example",
                "legalAddress": { "postalCode": "00100", "countryCode": "KE" },
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Physical Person" },
                },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        Case {
            name: "Kenya Physical Person with a certificate document",
            entry: json!({
                "registrationId": "A987654321K",
                "registrationType": "VAT",
                "legalName": "Wanjiku Example",
                "legalAddress": { "postalCode": "00100", "countryCode": "KE" },
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Physical Person" },
                },
                "verificationDetails": certificate_document(),
            }),
            expect: Expect::Stored,
        },
        // Bangladesh: "You must specify the tax registration certificate
        // document in the taxRegistrationDocuments field of the
        // VerificationDetails object."
        Case {
            name: "Bangladesh with no certificate document",
            entry: json!({
                "registrationId": "123456789-0101",
                "registrationType": "VAT",
                "legalName": "Example Bangladesh Ltd",
                "legalAddress": { "postalCode": "1230", "countryCode": "BD" },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        Case {
            name: "Bangladesh with a certificate document",
            entry: json!({
                "registrationId": "123456789-0101",
                "registrationType": "VAT",
                "legalName": "Example Bangladesh Ltd",
                "legalAddress": { "postalCode": "1230", "countryCode": "BD" },
                "verificationDetails": certificate_document(),
            }),
            expect: Expect::Stored,
        },
        // Indonesia: "You must specify the taxRegistrationNumberType in the
        // indonesiaAdditionalInfo field of the additionalTaxInformation object."
        Case {
            name: "Indonesia with no indonesiaAdditionalInfo",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
            }),
            expect: missing("TAX_REGISTRATION_NUMBER_TYPE"),
        },
        Case {
            name: "Indonesia with a NIK taxRegistrationNumberType",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": { "taxRegistrationNumberType": "NIK" },
                },
            }),
            expect: Expect::Stored,
        },
        // Indonesia: "If you specify decisionNumber, you must specify the
        // ppnExceptionDesignationCode."
        Case {
            name: "Indonesia with a decisionNumber and no ppnExceptionDesignationCode",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NPWP",
                        "decisionNumber": "KEP-123/2024",
                    },
                },
            }),
            expect: missing("PPN_EXCEPTION_DESIGNATION_CODE"),
        },
        // Indonesia: "If the taxRegistrationNumberType is set to NPWP or NITKU,
        // valid values for ppnExceptionDesignationCode are either 01, 02, 03, 07,
        // or 08. For other taxRegistrationNumberType values,
        // ppnExceptionDesignationCode must be either 01, 07, or 08."
        Case {
            name: "Indonesia NIK with the NPWP-only ppnExceptionDesignationCode 02",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NIK",
                        "ppnExceptionDesignationCode": "02",
                    },
                },
            }),
            expect: rejected_value("PPN_EXCEPTION_DESIGNATION_CODE"),
        },
        Case {
            name: "Indonesia NPWP with ppnExceptionDesignationCode 02",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NPWP",
                        "ppnExceptionDesignationCode": "02",
                    },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Indonesia NIK with the shared ppnExceptionDesignationCode 01",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NIK",
                        "ppnExceptionDesignationCode": "01",
                    },
                },
            }),
            expect: Expect::Stored,
        },
        // Indonesia: "If ppnExceptionDesignationCode is 07 or 08, you must
        // specify the decisionNumber."
        Case {
            name: "Indonesia with ppnExceptionDesignationCode 07 and no decisionNumber",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NPWP",
                        "ppnExceptionDesignationCode": "07",
                    },
                },
            }),
            expect: missing("DECISION_NUMBER"),
        },
        Case {
            name: "Indonesia with ppnExceptionDesignationCode 08 and a decisionNumber",
            entry: json!({
                "registrationId": "012345678901234",
                "registrationType": "VAT",
                "legalName": "PT Contoh",
                "legalAddress": { "postalCode": "10110", "countryCode": "ID" },
                "additionalTaxInformation": {
                    "indonesiaAdditionalInfo": {
                        "taxRegistrationNumberType": "NITKU",
                        "ppnExceptionDesignationCode": "08",
                        "decisionNumber": "KEP-456/2024",
                    },
                },
            }),
            expect: Expect::Stored,
        },
        // South Korea: certifiedEmailId, the southKoreaAdditionalInfo block and
        // the certificate document are all required.
        Case {
            name: "South Korea with no certifiedEmailId",
            entry: json!({
                "registrationId": "123-45-67890",
                "registrationType": "VAT",
                "legalName": "예시 주식회사",
                "legalAddress": { "postalCode": "04524", "countryCode": "KR" },
                "additionalTaxInformation": {
                    "southKoreaAdditionalInfo": {
                        "businessRepresentativeName": "홍길동",
                        "lineOfBusiness": "소프트웨어",
                        "itemOfBusiness": "클라우드",
                    },
                },
                "verificationDetails": certificate_document(),
            }),
            expect: missing("CERTIFIED_EMAIL_ID"),
        },
        Case {
            name: "South Korea with no southKoreaAdditionalInfo",
            entry: json!({
                "registrationId": "123-45-67890",
                "registrationType": "VAT",
                "legalName": "예시 주식회사",
                "certifiedEmailId": "trn@example.com",
                "legalAddress": { "postalCode": "04524", "countryCode": "KR" },
                "verificationDetails": certificate_document(),
            }),
            expect: missing("SOUTH_KOREA_ADDITIONAL_INFO"),
        },
        Case {
            name: "South Korea with no certificate document",
            entry: json!({
                "registrationId": "123-45-67890",
                "registrationType": "VAT",
                "legalName": "예시 주식회사",
                "certifiedEmailId": "trn@example.com",
                "legalAddress": { "postalCode": "04524", "countryCode": "KR" },
                "additionalTaxInformation": {
                    "southKoreaAdditionalInfo": {
                        "businessRepresentativeName": "홍길동",
                        "lineOfBusiness": "소프트웨어",
                        "itemOfBusiness": "클라우드",
                    },
                },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        Case {
            name: "South Korea with everything its rules ask for",
            entry: json!({
                "registrationId": "123-45-67890",
                "registrationType": "VAT",
                "legalName": "예시 주식회사",
                "certifiedEmailId": "trn@example.com",
                "legalAddress": { "postalCode": "04524", "countryCode": "KR" },
                "additionalTaxInformation": {
                    "southKoreaAdditionalInfo": {
                        "businessRepresentativeName": "홍길동",
                        "lineOfBusiness": "소프트웨어",
                        "itemOfBusiness": "클라우드",
                    },
                },
                "verificationDetails": certificate_document(),
            }),
            expect: Expect::Stored,
        },
        // Spain: "You must specify the registrationType in the
        // spainAdditionalInfo field", and "If the registrationType is Local, you
        // must specify the tax registration certificate document".
        Case {
            name: "Spain with no spainAdditionalInfo",
            entry: json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
            }),
            expect: missing("REGISTRATION_TYPE"),
        },
        Case {
            name: "Spain Intra-EU with no certificate document",
            entry: json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Intra-EU" },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Spain Local with no certificate document",
            entry: json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Local" },
                },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        Case {
            name: "Spain Local with a certificate document",
            entry: json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Local" },
                },
                "verificationDetails": certificate_document(),
            }),
            expect: Expect::Stored,
        },
        // France: "You must specify the sirenNumber in the franceAdditionalInfo
        // field of the additionalTaxInformation object."
        Case {
            name: "France with no franceAdditionalInfo",
            entry: json!({
                "registrationId": "FR12345678901",
                "registrationType": "TIN",
                "legalName": "Exemple SARL",
                "legalAddress": { "postalCode": "75001", "countryCode": "FR" },
            }),
            expect: missing("SIREN_NUMBER"),
        },
        Case {
            name: "France with a sirenNumber",
            entry: json!({
                "registrationId": "FR12345678901",
                "registrationType": "TIN",
                "legalName": "Exemple SARL",
                "legalAddress": { "postalCode": "75001", "countryCode": "FR" },
                "additionalTaxInformation": {
                    "franceAdditionalInfo": { "sirenNumber": "123456789" },
                },
            }),
            expect: Expect::Stored,
        },
        // Monaco: "You must specify the businessNumber in the
        // monacoAdditionalInfo field of the additionalTaxInformation object."
        Case {
            name: "Monaco with no monacoAdditionalInfo",
            entry: json!({
                "registrationId": "MC12345678901",
                "registrationType": "VAT",
                "legalName": "Exemple SAM",
                "legalAddress": { "postalCode": "98000", "countryCode": "MC" },
            }),
            expect: missing("BUSINESS_NUMBER"),
        },
        Case {
            name: "Monaco with a businessNumber",
            entry: json!({
                "registrationId": "MC12345678901",
                "registrationType": "VAT",
                "legalName": "Exemple SAM",
                "legalAddress": { "postalCode": "98000", "countryCode": "MC" },
                "additionalTaxInformation": {
                    "monacoAdditionalInfo": { "businessNumber": "MC1234567" },
                },
            }),
            expect: Expect::Stored,
        },
        // Saudi Arabia: "For address, you must specify addressLine3."
        Case {
            name: "Saudi Arabia with no addressLine3",
            entry: json!({
                "registrationId": "300000000000003",
                "registrationType": "VAT",
                "legalName": "Example Arabia Co",
                "legalAddress": {
                    "addressLine1": "King Fahd Road",
                    "city": "Riyadh",
                    "postalCode": "12345",
                    "countryCode": "SA",
                },
            }),
            expect: missing("ADDRESS_LINE3"),
        },
        Case {
            name: "Saudi Arabia with an addressLine3",
            entry: json!({
                "registrationId": "300000000000003",
                "registrationType": "VAT",
                "legalName": "Example Arabia Co",
                "legalAddress": {
                    "addressLine1": "King Fahd Road",
                    "addressLine3": "1234",
                    "city": "Riyadh",
                    "postalCode": "12345",
                    "countryCode": "SA",
                },
            }),
            expect: Expect::Stored,
        },
        // Turkey: the sector and the address's districtOrCounty are required,
        // and the taxOffice is required unless the sector is Individual — which
        // takes neither a taxOffice nor a kepEmailId.
        Case {
            name: "Turkey with no sector",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek AS",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
            }),
            expect: missing("SECTOR"),
        },
        Case {
            name: "Turkey with no districtOrCounty",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Kisi",
                "sector": "Individual",
                "legalAddress": { "postalCode": "34000", "countryCode": "TR" },
            }),
            expect: missing("DISTRICT_OR_COUNTY"),
        },
        Case {
            name: "Turkey Business with no taxOffice",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek AS",
                "sector": "Business",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
            }),
            expect: missing("TAX_OFFICE"),
        },
        Case {
            name: "Turkey Government with a taxOffice",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Kurumu",
                "sector": "Government",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
                "additionalTaxInformation": {
                    "turkeyAdditionalInfo": { "taxOffice": "Kadikoy Vergi Dairesi" },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Turkey Individual carrying a taxOffice",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Kisi",
                "sector": "Individual",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
                "additionalTaxInformation": {
                    "turkeyAdditionalInfo": { "taxOffice": "Kadikoy Vergi Dairesi" },
                },
            }),
            expect: rejected_value("TAX_OFFICE"),
        },
        Case {
            name: "Turkey Individual carrying a kepEmailId",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Kisi",
                "sector": "Individual",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
                "additionalTaxInformation": {
                    "turkeyAdditionalInfo": { "kepEmailId": "ornek@hs01.kep.tr" },
                },
            }),
            expect: rejected_value("KEP_EMAIL_ID"),
        },
        Case {
            name: "Turkey Individual with neither",
            entry: json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Kisi",
                "sector": "Individual",
                "legalAddress": {
                    "districtOrCounty": "Kadikoy",
                    "postalCode": "34000",
                    "countryCode": "TR",
                },
            }),
            expect: Expect::Stored,
        },
        // Malaysia: the sector and registrationType value sets, and the business
        // and business-reseller members of malaysiaAdditionalInfo.
        Case {
            name: "Malaysia with the Government sector",
            entry: json!({
                "registrationId": "880101011234",
                "registrationType": "NRIC",
                "legalName": "Contoh Sdn Bhd",
                "sector": "Government",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
            }),
            expect: rejected_value("SECTOR"),
        },
        Case {
            name: "Malaysia with a registrationType outside its documented set",
            entry: json!({
                "registrationId": "880101011234",
                "registrationType": "VAT",
                "legalName": "Contoh Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
            }),
            expect: rejected_value("REGISTRATION_TYPE"),
        },
        Case {
            name: "Malaysia TIN with no businessRegistrationNumber",
            entry: json!({
                "registrationId": "202001001234",
                "registrationType": "TIN",
                "legalName": "Contoh Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
            }),
            expect: missing("BUSINESS_REGISTRATION_NUMBER"),
        },
        Case {
            name: "Malaysia TIN with a businessRegistrationNumber",
            entry: json!({
                "registrationId": "202001001234",
                "registrationType": "TIN",
                "legalName": "Contoh Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "businessRegistrationNumber": "202001001234" },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Malaysia SST with no taxInformationNumber",
            entry: json!({
                "registrationId": "W10-1808-32000055",
                "registrationType": "SST",
                "legalName": "Contoh Services Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "businessRegistrationNumber": "202001001234" },
                },
            }),
            expect: missing("TAX_INFORMATION_NUMBER"),
        },
        Case {
            name: "Malaysia SST with both reseller members",
            entry: json!({
                "registrationId": "W10-1808-32000055",
                "registrationType": "SST",
                "legalName": "Contoh Services Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": {
                        "businessRegistrationNumber": "202001001234",
                        "taxInformationNumber": "C1234567890",
                        "serviceTaxCodes": ["IT Services"],
                    },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Malaysia NRIC for an individual",
            entry: json!({
                "registrationId": "880101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Contoh",
                "sector": "Individual",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "taxInformationNumber": "C1234567890" },
                },
            }),
            expect: Expect::Stored,
        },
        // Mexico: "You must provide a Constancia de Situación fiscal (CSF)
        // document in the verificationDetails field."
        Case {
            name: "Mexico with no CSF document",
            entry: json!({
                "registrationId": "EJEM800101XX1",
                "registrationType": "VAT",
                "legalName": "Ejemplo SA de CV",
                "legalAddress": { "postalCode": "01000", "countryCode": "MX" },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        Case {
            name: "Mexico with a CSF document",
            entry: json!({
                "registrationId": "EJEM800101XX1",
                "registrationType": "VAT",
                "legalName": "Ejemplo SA de CV",
                "legalAddress": { "postalCode": "01000", "countryCode": "MX" },
                "verificationDetails": certificate_document(),
            }),
            expect: Expect::Stored,
        },
        // Nepal and Ukraine: "The sector valid values are Business and
        // Individual."
        Case {
            name: "Nepal with the Government sector",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Example Nepal Pvt Ltd",
                "sector": "Government",
                "legalAddress": { "postalCode": "44600", "countryCode": "NP" },
            }),
            expect: rejected_value("SECTOR"),
        },
        Case {
            name: "Nepal with the Business sector",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Example Nepal Pvt Ltd",
                "sector": "Business",
                "legalAddress": { "postalCode": "44600", "countryCode": "NP" },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Ukraine with the Government sector",
            entry: json!({
                "registrationId": "123456789012",
                "registrationType": "VAT",
                "legalName": "Pryklad TOV",
                "sector": "Government",
                "legalAddress": { "postalCode": "01001", "countryCode": "UA" },
                "additionalTaxInformation": {
                    "ukraineAdditionalInfo": { "ukraineTrnType": "Business" },
                },
            }),
            expect: rejected_value("SECTOR"),
        },
        Case {
            name: "Ukraine with the Individual sector",
            entry: json!({
                "registrationId": "123456789012",
                "registrationType": "VAT",
                "legalName": "Pryklad TOV",
                "sector": "Individual",
                "legalAddress": { "postalCode": "01001", "countryCode": "UA" },
                "additionalTaxInformation": {
                    "ukraineAdditionalInfo": { "ukraineTrnType": "Individual" },
                },
            }),
            expect: Expect::Stored,
        },
        // Georgia: "The valid personType values are Physical Person and
        // Business." The rule is about the value, so an entry carrying no
        // Georgian block has nothing to refuse.
        Case {
            name: "Georgia with the Legal Person personType",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Magaliti LLC",
                "legalAddress": { "postalCode": "0105", "countryCode": "GE" },
                "additionalTaxInformation": {
                    "georgiaAdditionalInfo": { "personType": "Legal Person" },
                },
            }),
            expect: rejected_value("PERSON_TYPE"),
        },
        Case {
            name: "Georgia with the Physical Person personType",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Magaliti Pirovneba",
                "legalAddress": { "postalCode": "0105", "countryCode": "GE" },
                "additionalTaxInformation": {
                    "georgiaAdditionalInfo": { "personType": "Physical Person" },
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "Georgia with no georgiaAdditionalInfo",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Magaliti LLC",
                "legalAddress": { "postalCode": "0105", "countryCode": "GE" },
            }),
            expect: Expect::Stored,
        },
        // `Address$stateOrRegion`: "This field is only required for Canada,
        // India, United Arab Emirates, Romania, and Brazil (CPF). It is
        // optional for all other countries."
        // (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#Address",member="stateOrRegion")`)
        Case {
            name: "Canada with no stateOrRegion",
            entry: json!({
                "registrationId": "123456789RT0001",
                "registrationType": "GST",
                "legalName": "Example Canada Inc",
                "legalAddress": {
                    "addressLine1": "1 Robson Street",
                    "city": "Vancouver",
                    "postalCode": "V6B 1A1",
                    "countryCode": "CA",
                },
            }),
            expect: missing("STATE_OR_REGION"),
        },
        Case {
            name: "Canada with a stateOrRegion",
            entry: json!({
                "registrationId": "123456789RT0001",
                "registrationType": "GST",
                "legalName": "Example Canada Inc",
                "legalAddress": {
                    "addressLine1": "1 Robson Street",
                    "city": "Vancouver",
                    "stateOrRegion": "British Columbia",
                    "postalCode": "V6B 1A1",
                    "countryCode": "CA",
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "India with no stateOrRegion",
            entry: json!({
                "registrationId": "29ABCDE1234F1Z5",
                "registrationType": "GST",
                "legalName": "Acme India Private Limited",
                "legalAddress": {
                    "addressLine1": "1 Residency Road",
                    "city": "Bengaluru",
                    "postalCode": "560025",
                    "countryCode": "IN",
                },
            }),
            expect: missing("STATE_OR_REGION"),
        },
        Case {
            name: "India with a stateOrRegion",
            entry: json!({
                "registrationId": "29ABCDE1234F1Z5",
                "registrationType": "GST",
                "legalName": "Acme India Private Limited",
                "legalAddress": {
                    "addressLine1": "1 Residency Road",
                    "city": "Bengaluru",
                    "stateOrRegion": "Karnataka",
                    "postalCode": "560025",
                    "countryCode": "IN",
                },
            }),
            expect: Expect::Stored,
        },
        Case {
            name: "the United Arab Emirates with no stateOrRegion",
            entry: json!({
                "registrationId": "100123456700003",
                "registrationType": "VAT",
                "legalName": "Example Trading LLC",
                "legalAddress": { "postalCode": "00000", "countryCode": "AE" },
                // The TRN type its own documentation requires, so what is left
                // unsatisfied is the address member this row is about.
                "additionalTaxInformation": {
                    "unitedArabEmiratesAdditionalInfo": {
                        "taxRegistrationNumberType": "VATTaxRegistrationNumber",
                    },
                },
            }),
            expect: missing("STATE_OR_REGION"),
        },
        Case {
            name: "Romania with no stateOrRegion",
            entry: json!({
                "registrationId": "RO1234567890",
                "registrationType": "VAT",
                "legalName": "Exemplu SRL",
                "legalAddress": { "postalCode": "010101", "countryCode": "RO" },
            }),
            expect: missing("STATE_OR_REGION"),
        },
        Case {
            name: "Brazil CPF with no stateOrRegion",
            entry: json!({
                "registrationId": "123.456.789-09",
                "registrationType": "CPF",
                "legalName": "Maria Exemplo",
                "legalAddress": { "postalCode": "22041-001", "countryCode": "BR" },
            }),
            expect: missing("STATE_OR_REGION"),
        },
        Case {
            name: "Brazil CNPJ with no stateOrRegion, which only CPF requires",
            entry: json!({
                "registrationId": "12.345.678/0001-95",
                "registrationType": "CNPJ",
                "legalName": "Exemplo Industria Ltda",
                "legalAddress": { "postalCode": "01001-000", "countryCode": "BR" },
            }),
            expect: Expect::Stored,
        },
        // `CanadaAdditionalInfo$isResellerAccount`: "The value for this
        // parameter must be true if the provincialSalesTaxId value is provided
        // for a TRN in British Columbia, Saskatchewan, or Manitoba provinces."
        Case {
            name: "Canada claiming a provincial sales tax ID with no isResellerAccount",
            entry: json!({
                "registrationId": "123456789RT0001",
                "registrationType": "GST",
                "legalName": "Example Canada Inc",
                "legalAddress": {
                    "stateOrRegion": "British Columbia",
                    "postalCode": "V6B 1A1",
                    "countryCode": "CA",
                },
                "additionalTaxInformation": {
                    "canadaAdditionalInfo": { "provincialSalesTaxId": "PST-1234567" },
                },
            }),
            expect: rejected_value("IS_RESELLER_ACCOUNT"),
        },
        Case {
            name: "Canada claiming a provincial sales tax ID as a reseller",
            entry: json!({
                "registrationId": "123456789RT0001",
                "registrationType": "GST",
                "legalName": "Example Canada Inc",
                "legalAddress": {
                    "stateOrRegion": "British Columbia",
                    "postalCode": "V6B 1A1",
                    "countryCode": "CA",
                },
                "additionalTaxInformation": {
                    "canadaAdditionalInfo": {
                        "provincialSalesTaxId": "PST-1234567",
                        "isResellerAccount": true,
                    },
                },
            }),
            expect: Expect::Stored,
        },
        // New Zealand: "You can optionally specify the sector. Valid values are
        // Government and Business."
        Case {
            name: "New Zealand with a sector its documentation does not admit",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "GST",
                "legalName": "Example NZ Limited",
                "sector": "Individual",
                "legalAddress": { "postalCode": "6011", "countryCode": "NZ" },
            }),
            expect: rejected_value("SECTOR"),
        },
        Case {
            name: "New Zealand with one of the two sectors it admits",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "GST",
                "legalName": "Example NZ Limited",
                "sector": "Business",
                "legalAddress": { "postalCode": "6011", "countryCode": "NZ" },
            }),
            expect: Expect::Stored,
        },
        // New Zealand: `NewZealandAdditionalInfo$peppolId` is "Only accepted when
        // the sector is Government".
        Case {
            name: "New Zealand submitting a peppolId for a non-government sector",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "GST",
                "legalName": "Example NZ Limited",
                "sector": "Business",
                "legalAddress": { "postalCode": "6011", "countryCode": "NZ" },
                "additionalTaxInformation": {
                    "newZealandAdditionalInfo": { "peppolId": "9429038762371" },
                },
            }),
            expect: rejected_value("PEPPOL_ID"),
        },
        Case {
            name: "New Zealand submitting a peppolId as a government customer",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "GST",
                "legalName": "Example NZ Agency",
                "sector": "Government",
                "legalAddress": { "postalCode": "6011", "countryCode": "NZ" },
                "additionalTaxInformation": {
                    "newZealandAdditionalInfo": { "peppolId": "9429038762371" },
                },
            }),
            expect: Expect::Stored,
        },
        // Norway: "Use LocalRegistrationNumber to register a Norwegian
        // Organisation Number for a business that isn't VAT registered. In that
        // case, the registrationId must be the 9-digit Organisation Number."
        Case {
            name: "Norway registering an Organisation Number that is not nine digits",
            entry: json!({
                "registrationId": "NO123456789MVA",
                "registrationType": "VAT",
                "legalName": "Eksempel AS",
                "legalAddress": { "postalCode": "0150", "countryCode": "NO" },
                "additionalTaxInformation": {
                    "norwayAdditionalInfo": {
                        "taxRegistrationNumberType": "LocalRegistrationNumber",
                    },
                },
            }),
            expect: rejected_value("REGISTRATION_ID"),
        },
        Case {
            name: "Norway registering the nine-digit Organisation Number",
            entry: json!({
                "registrationId": "123456789",
                "registrationType": "VAT",
                "legalName": "Eksempel AS",
                "legalAddress": { "postalCode": "0150", "countryCode": "NO" },
                "additionalTaxInformation": {
                    "norwayAdditionalInfo": {
                        "taxRegistrationNumberType": "LocalRegistrationNumber",
                    },
                },
            }),
            expect: Expect::Stored,
        },
        // United Arab Emirates: "You must specify the taxRegistrationNumberType
        // in the unitedArabEmiratesAdditionalInfo field of the
        // additionalTaxInformation object."
        Case {
            name: "the United Arab Emirates with no taxRegistrationNumberType",
            entry: json!({
                "registrationId": "100123456700003",
                "registrationType": "VAT",
                "legalName": "Example Trading LLC",
                "legalAddress": {
                    "stateOrRegion": "Dubai",
                    "postalCode": "00000",
                    "countryCode": "AE",
                },
            }),
            expect: missing("TAX_REGISTRATION_NUMBER_TYPE"),
        },
        Case {
            name: "the United Arab Emirates with the TRN type its documentation requires",
            entry: json!({
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
                        "taxRegistrationNumberType": "LocalTaxRegistrationNumber",
                    },
                },
            }),
            expect: Expect::Stored,
        },
        // Germany: "You must provide at least one of the following: a Tax
        // Registration Number (TRN) (registrationId), the
        // economicIdentificationNumber, or the steuernummer." `registrationId` is
        // `@required` on `TaxRegistrationEntry`, so every entry carries one of
        // the three and no row can break this rule.
        Case {
            name: "Germany with its TRN alone, which its documentation admits",
            entry: json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
            expect: Expect::Stored,
        },
    ]
}

/// RULE: an entry that does not satisfy the requirements `PutTaxRegistration`
/// documents for the country of its `legalAddress.countryCode` is refused with
/// `ValidationException` (400) naming the offending parameter, and nothing is
/// persisted — while an entry that satisfies them is stored and reads back.
#[tokio::test]
async fn a_country_rule_refuses_the_entry_it_is_about_and_stores_nothing() {
    for case in cases() {
        let router = emulator();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": case.entry }),
        )
        .await;
        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;

        match case.expect {
            Expect::Stored => {
                assert_eq!(
                    (put.status, put.error_code.as_str()),
                    (200, ""),
                    "case {:?}: expected the entry to be stored, got {:?}",
                    case.name,
                    put.body
                );
                assert_eq!(
                    put.get("status").as_str(),
                    Some(STATUS_PENDING),
                    "case {:?}: expected the write to report the status it stored, got {:?}",
                    case.name,
                    put.body
                );
                assert_eq!(
                    got.status, 200,
                    "case {:?}: expected the stored registration to read back, got {:?}",
                    case.name, got.body
                );
                assert_eq!(
                    got.body
                        .pointer("/taxRegistration/registrationId")
                        .and_then(Value::as_str),
                    case.entry.get("registrationId").and_then(Value::as_str),
                    "case {:?}: expected the submitted TRN to read back, got {:?}",
                    case.name,
                    got.body
                );
            }
            Expect::Refused {
                error_code,
                parameter,
            } => {
                assert_eq!(
                    (put.status, put.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {:?}: expected 400 ValidationException, got {:?}",
                    case.name,
                    put.body
                );
                assert_eq!(
                    put.get("errorCode").as_str(),
                    Some(error_code),
                    "case {:?}: expected errorCode {error_code:?}, got {:?}",
                    case.name,
                    put.body
                );
                assert_eq!(
                    refused_parameters(&put.body),
                    vec![parameter.to_string()],
                    "case {:?}: expected fieldList to name {parameter:?}, got {:?}",
                    case.name,
                    put.body
                );
                assert!(
                    put.get("message").as_str().is_some_and(|message| !message.trim().is_empty()),
                    "case {:?}: expected the refusal to carry a message, got {:?}",
                    case.name,
                    put.body
                );
                assert_eq!(
                    (got.status, got.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {:?}: a refused entry must persist nothing, got {:?}",
                    case.name,
                    got.body
                );
            }
        }
    }
}

/// RULE (completeness): every rule the shared set carries is exercised by the
/// table above — same country, same parameter, same error code — so the table
/// spans the documented rule set rather than the one recorded request.
#[test]
fn every_documented_country_rule_is_exercised() {
    let exercised: BTreeSet<(String, String, &str)> = cases()
        .iter()
        .filter_map(|case| match case.expect {
            Expect::Stored => None,
            Expect::Refused {
                error_code,
                parameter,
            } => {
                let country = case
                    .entry
                    .pointer("/legalAddress/countryCode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                Some((country, parameter.to_string(), error_code))
            }
        })
        .collect();

    for rule in COUNTRY_RULES {
        let wanted = (
            rule.country_code.to_ascii_uppercase(),
            parameter_name(rule.member_path),
            rule.error_code,
        );
        assert!(
            exercised.contains(&wanted),
            "rule {:?} on {:?} is not exercised by any case: {exercised:?}",
            rule.member_path,
            rule.country_code
        );
    }
}

/// The parameter names a refusal's `fieldList` carries.
fn refused_parameters(body: &Value) -> Vec<String> {
    body.get("fieldList")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}
