// SPDX-License-Identifier: CC0-1.0

//! The registry's answer to `CheckDomainAvailability`, over awsJson1_1: WHICH names are
//! available, and the standard price the response carries for the ones that are.
//!
//! Two rules are pinned here:
//!
//! * `Availability` is the registry's answer, not this account's — a name ANY registrant
//!   holds is `UNAVAILABLE` (`DomainAvailability`: "The domain name is not available"),
//!   and only a well-formed, unheld name under a TLD the registrar offers is `AVAILABLE`
//!   ("You can register only domains designated as AVAILABLE");
//! * `Price` is present exactly when `Availability` is `AVAILABLE`
//!   (`CheckDomainAvailabilityResponse.Price`: "This element is returned only when
//!   Availability is AVAILABLE"), and carries the standard rate of the name's TLD with
//!   its required `Price` and `Currency` members and `PriceType` `STANDARD`.
//!
//! A name the registry cannot answer for at all is refused instead, and those refusals
//! are pinned here too: the TLD it does not delegate, a blank name, and the envelope
//! every constraint violation is reported in.
//!
//! `GetDomainSuggestions` is pinned here as well, because a suggestion is the registry
//! answering for a name it derived rather than one the caller named: the last section of
//! this suite holds the derivation, the bound on how many names come back, and the
//! requirement that a suggestion's own `Availability` and `Price` are the ones
//! `CheckDomainAvailability` gives for that same name.
//!
//! Every test drives the real wire path through the shared harness in
//! `tests/all/domain_lifecycle.rs`.

use crate::domain_lifecycle::{error_code, person_contact, registration, Emulator};
use serde_json::{json, Value};

/// The standard yearly registration rates these cases expect, in USD. Source: the
/// per-TLD table `STANDARD_REGISTRATION_RATES` in
/// `src/state/registry.rs`, which carries Route 53's published registration price per
/// TLD; a TLD the table does not list takes `DEFAULT_STANDARD_REGISTRATION_RATE`
/// (20.00), which the `.shop` row below exercises.
const EXPECTED_RATES: &[(&str, f64)] = &[
    ("com", 16.00),
    ("net", 17.00),
    ("org", 16.00),
    ("io", 47.00),
    ("co.uk", 10.00),
    ("dev", 15.00),
    ("jp", 95.00),
    // Not in the table: the stated default rate.
    ("shop", 20.00),
];

/// `CheckDomainAvailability` for `name`, asserting a 200 and returning the body.
async fn check(emulator: &Emulator, name: &str) -> Value {
    let (status, body) = emulator
        .call("CheckDomainAvailability", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CheckDomainAvailability to answer 200, got {status} {body}"
    );
    body
}

/// Assert that `body` reports `name` as available at the standard rate `rate`.
fn assert_available_at(body: &Value, name: &str, rate: f64) {
    assert_eq!(
        body["Availability"].as_str(),
        Some("AVAILABLE"),
        "case {name:?}: an unheld name must be AVAILABLE, got {body}"
    );
    let price = &body["Price"];
    assert!(
        price.is_object(),
        "case {name:?}: Price is returned whenever Availability is AVAILABLE, got {body}"
    );
    assert_eq!(
        price["Price"].as_f64(),
        Some(rate),
        "case {name:?}: expected the TLD's standard rate {rate}, got {price}"
    );
    assert!(
        price["Price"].as_f64().is_some_and(|value| value >= 0.0),
        "case {name:?}: Price is @range(min: 0), got {price}"
    );
    assert_eq!(
        price["Currency"].as_str(),
        Some("USD"),
        "case {name:?}: expected the required Currency member, got {price}"
    );
    assert_eq!(
        price["PriceType"].as_str(),
        Some("STANDARD"),
        "case {name:?}: a regular TLD rate is PriceType STANDARD, got {price}"
    );
}

/// Assert that `body` reports `name` as unavailable and carries no price.
fn assert_unavailable(body: &Value, name: &str) {
    assert_eq!(
        body["Availability"].as_str(),
        Some("UNAVAILABLE"),
        "case {name:?}: a name the registry already holds must be UNAVAILABLE, got {body}"
    );
    assert!(
        body.get("Price").is_none_or(Value::is_null),
        "case {name:?}: Price is returned only when Availability is AVAILABLE, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The rule: an AVAILABLE name is priced at its TLD's standard rate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_available_name_carries_the_standard_price_of_its_tld() {
    // Rows differ along the TLD (generic, country-code, two-label suffix, a TLD the
    // price table does not list), the label's shape and the case of the request. Two
    // labels are checked under every TLD, because a standard rate belongs to the TLD
    // rather than to the name.
    let labels = ["sim-probe-free", "z0-9"];
    let emulator = Emulator::new();

    for (tld, rate) in EXPECTED_RATES {
        for label in labels {
            let name = format!("{label}.{tld}");
            let body = check(&emulator, &name).await;
            assert_available_at(&body, &name, *rate);

            // Domain names are not case sensitive, so the same name in another case is
            // the same answer at the same price.
            let upper = name.to_uppercase();
            let body = check(&emulator, &upper).await;
            assert_available_at(&body, &upper, *rate);
        }
    }
}

#[tokio::test]
async fn checking_a_name_twice_answers_the_same_thing() {
    // The operation is a pure read: nothing a caller sees may change because somebody
    // asked. Rows: an available name and one the registry holds.
    let emulator = Emulator::new();
    for name in ["sim-probe-repeat.com", "google.com"] {
        let first = check(&emulator, name).await;
        let second = check(&emulator, name).await;
        assert_eq!(
            first, second,
            "case {name:?}: a second check must answer exactly the first, got {first} then {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: Availability is the registry's answer, not this account's
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_held_by_another_registrant_is_unavailable() {
    // Rows differ along the TLD and the case of the request. Every one of these names is
    // registered in the real registries and none of them is held by this account, which
    // is exactly the case an account-scoped answer gets wrong.
    let held = [
        "google.com",
        "amazon.net",
        "example.org",
        "x.com",
        "iana.org",
        "amazon.co.uk",
        "GOOGLE.COM",
        "Example.Org",
    ];
    // Controls: free names under the same TLDs must still be available, so the fix
    // cannot be a blanket UNAVAILABLE.
    let free = [
        ("sim-probe-control.com", 16.00),
        ("sim-probe-control.net", 17.00),
        ("sim-probe-control.org", 16.00),
        ("sim-probe-control.co.uk", 10.00),
    ];

    let emulator = Emulator::new();
    for name in held {
        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);
    }
    for (name, rate) in free {
        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);
    }
}

#[tokio::test]
async fn a_name_held_by_another_registrant_is_neither_registrable_nor_readable_here() {
    // The answer has to be backed: a name reported UNAVAILABLE cannot then be registered,
    // and it is still not one of this account's domains. Rows differ along the TLD.
    let emulator = Emulator::new();
    for name in ["google.com", "example.org", "amazon.co.uk"] {
        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, &person_contact()))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {name:?}: a name the registry holds cannot be registered, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Domain {name} is not available for registration").as_str()),
            "case {name:?}: the refusal must name the requested domain, got {body}"
        );

        let (status, body) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {name:?}: a third party's name is not this account's domain, got {status} {body}"
        );

        // The refused registration left no trace: the name is still the registry's.
        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);
    }
}

// ---------------------------------------------------------------------------
// The rule: a TLD no registry delegates is refused, naming the TLD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unsupported_tld_is_refused_with_the_tld_in_the_message() {
    // The registry can only answer for a name under a TLD it delegates, so a name under
    // any other suffix is `UnsupportedTLD` — and the message's one variable part is the
    // TLD (the labels after the first), not the whole requested name. Rows differ along
    // the suffix: an unassigned label and two suffixes RFC 2606 reserves out of the
    // DNS.
    let cases = [
        ("sim-probe.doesnotexisttld", "doesnotexisttld"),
        ("sim-probe.invalid", "invalid"),
        ("sim-probe.test", "test"),
        // The same request in another case names the same TLD: domain names are not
        // case sensitive.
        ("SIM-PROBE.DoesNotExistTld", "doesnotexisttld"),
    ];
    let emulator = Emulator::new();

    for (name, tld) in cases {
        let (status, body) = emulator
            .call("CheckDomainAvailability", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UnsupportedTLD".to_string()),
            "case {name:?}: expected 400 UnsupportedTLD, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The {tld} TLD is not supported").as_str()),
            "case {name:?}: the message must name the TLD alone, got {body}"
        );
    }

    // A third-level name is not a registrable name either, whatever a registry would
    // call its suffix, so only the refusal itself is asserted for it.
    let (status, body) = emulator
        .call(
            "CheckDomainAvailability",
            &json!({ "DomainName": "a.b.example.com" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UnsupportedTLD".to_string()),
        "case \"a third-level name\": expected 400 UnsupportedTLD, got {status} {body}"
    );
}

#[tokio::test]
async fn a_blank_domain_name_is_refused_as_blank() {
    // A member that carries no name at all is a different problem from a name that
    // broke one of the syntax rules, and it is reported in its own words. Rows differ
    // along the form the blank value takes.
    let emulator = Emulator::new();
    for case in ["", " ", "   "] {
        let (status, body) = emulator
            .call("CheckDomainAvailability", &json!({ "DomainName": case }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid request. Problems: Domain name cannot be blank"),
            "case {case:?}: a blank name is refused as blank, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: the answer and the price follow the store, in both directions
// ---------------------------------------------------------------------------
#[tokio::test]
async fn registering_a_name_withdraws_its_price_and_deleting_it_restores_it() {
    // Rows differ along the TLD, so the price the answer carries differs per row.
    let cases = [
        ("sim-probe-lifecycle.com", 16.00),
        ("sim-probe-lifecycle.io", 47.00),
        ("sim-probe-lifecycle.co.uk", 10.00),
    ];
    let emulator = Emulator::new();

    for (name, rate) in cases {
        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);

        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, &person_contact()))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected RegisterDomain to succeed, got {status} {body}"
        );

        // Read the registration back through the implemented reader: the name this
        // account now holds is the name that just became unavailable.
        let (status, detail) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, detail["DomainName"].as_str()),
            (200, Some(name)),
            "case {name:?}: expected GetDomainDetail to report the registration, got {status} {detail}"
        );

        let body = check(&emulator, name).await;
        assert_unavailable(&body, name);

        let (status, body) = emulator
            .call("DeleteDomain", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteDomain to succeed, got {status} {body}"
        );

        let body = check(&emulator, name).await;
        assert_available_at(&body, name, rate);
    }
}

// ---------------------------------------------------------------------------
// The rule: a constraint violation is reported in the validation envelope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_constraint_violation_names_how_many_violations_were_found() {
    // The envelope is "<N> validation error(s) detected: " followed by the clause for
    // each member, so the count is the number of violations in THIS request. Rows
    // differ along the member and the constraint broken, and along HOW MANY members
    // break one: `DomainName` is @length(max: 255) and `DurationInYears` is
    // @range(min: 1, max: 10) (smithy_query shapes DomainName and DurationInYears).
    let past_bound = format!("{}.com", "a".repeat(252));
    let emulator = Emulator::new();

    // (case, operation, request, the clauses the envelope must carry)
    let cases: [(&str, &str, Value, Vec<String>); 3] = [
        (
            "one member past its length bound",
            "CheckDomainAvailability",
            json!({ "DomainName": past_bound }),
            vec![format!(
                "Value '{past_bound}' at 'domainName' failed to satisfy constraint: Member must \
                 have length less than or equal to 255"
            )],
        ),
        (
            "one member past its range bound",
            "RegisterDomain",
            registration("range.com", 11, &person_contact()),
            vec![
                "Value '11' at 'durationInYears' failed to satisfy constraint: Member must have \
                 value less than or equal to 10"
                    .to_string(),
            ],
        ),
        (
            "two members at once",
            "RegisterDomain",
            registration(&past_bound, 0, &person_contact()),
            vec![
                format!(
                    "Value '{past_bound}' at 'domainName' failed to satisfy constraint: Member \
                     must have length less than or equal to 255"
                ),
                "Value '0' at 'durationInYears' failed to satisfy constraint: Member must have \
                 value greater than or equal to 1"
                    .to_string(),
            ],
        ),
    ];

    for (case, operation, request, clauses) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        let found = clauses.len();
        let noun = if found == 1 { "error" } else { "errors" };
        let envelope = format!("{found} validation {noun} detected: ");
        assert!(
            message.starts_with(&envelope),
            "case {case:?}: expected the message to open with {envelope:?}, got {message:?}"
        );
        for clause in &clauses {
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: expected the message to carry {clause:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The rule: a constraint clause quotes the value the REQUEST carried, whole
// ---------------------------------------------------------------------------

/// The one-violation envelope over `clause`.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

#[tokio::test]
async fn a_constraint_clause_quotes_the_whole_submitted_value() {
    // A clause quotes the value the request carried however long it was, so the rows
    // differ along the LENGTH of that value — at 256 bytes, one past it, and far past it,
    // the 256 being where the boundary walk starts eliding a value behind a marker naming
    // its byte count — and along the member and the operation refused, so the rule is not
    // one member's. `DomainName` and `OperationId` are both `@length(0, 255)`
    // (`smithy_query(query="shape",shape="DomainName,OperationId")`).
    let cases: [(&str, &str, &str, &str, usize); 6] = [
        (
            "a name at the elision ceiling",
            "CheckDomainAvailability",
            "DomainName",
            "domainName",
            256,
        ),
        (
            "a name one byte past it",
            "CheckDomainAvailability",
            "DomainName",
            "domainName",
            257,
        ),
        (
            "a name far past it",
            "CheckDomainAvailability",
            "DomainName",
            "domainName",
            4096,
        ),
        (
            "an operation id at the ceiling",
            "GetOperationDetail",
            "OperationId",
            "operationId",
            256,
        ),
        (
            "an operation id past it",
            "GetOperationDetail",
            "OperationId",
            "operationId",
            300,
        ),
        (
            "a deleted name far past it",
            "DeleteDomain",
            "DomainName",
            "domainName",
            1024,
        ),
    ];
    let emulator = Emulator::new();

    for (case, operation, member, path, length) in cases {
        let submitted = "a".repeat(length);
        let mut request = serde_json::Map::new();
        request.insert(member.to_string(), Value::String(submitted.clone()));
        let (status, body) = emulator.call(operation, &Value::Object(request)).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        let expected = one_violation(&format!(
            "Value '{submitted}' at '{path}' failed to satisfy constraint: Member must have \
             length less than or equal to 255"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: the clause must quote the whole submitted value, got {message:?}"
        );
        assert!(
            !message.contains("bytes]"),
            "case {case:?}: no clause states a byte-count marker in place of the value, \
             got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: a value the model withholds is stated by no clause
// ---------------------------------------------------------------------------

/// The value set a clause states for `ContactType`, in the order this service's front door
/// lists one.
///
/// The model declares that enum `[PERSON, COMPANY, ASSOCIATION, PUBLIC_BODY, RESELLER]`
/// (`smithy_query(query="shape",shape="ContactType")`); the order below is the one the
/// table those values are held in is walked in, which is neither the declaration order nor
/// lexicographic. It holds exactly the values the model declares, reordered.
const CONTACT_TYPE_SET: &str = "[ASSOCIATION, RESELLER, PERSON, PUBLIC_BODY, COMPANY]";

#[tokio::test]
async fn a_withheld_members_value_is_stated_by_no_clause() {
    // `ContactDetail` is `@sensitive`, so every value inside one is withheld whatever
    // constraint it broke. Rows differ along WHICH contact of the input carried it (all
    // four `ContactDetail` members: `smithy_query(query="operation",
    // operation="UpdateDomainContact")`), along the constraint broken (an enum value set,
    // two different `@length` maximums) and along the value submitted.
    //
    // `ContactNumber` is `@length(0, 30)` and `Email` `@length(0, 254)`
    // (`smithy_query(query="shape",shape="ContactDetail")`).
    let past_phone_bound = "1".repeat(31);
    let past_email_bound = "e".repeat(255);
    let enum_set = format!("Member must satisfy enum value set: {CONTACT_TYPE_SET}");
    let cases: [(&str, &str, &str, &str, &str); 5] = [
        (
            "an admin contact type outside the enum",
            "AdminContact",
            "contactType",
            "BOGUS",
            &enum_set,
        ),
        (
            "a registrant contact type in the wrong case",
            "RegistrantContact",
            "contactType",
            "person",
            &enum_set,
        ),
        (
            "a tech contact type left empty",
            "TechContact",
            "contactType",
            "",
            &enum_set,
        ),
        (
            "a billing contact phone number past its bound",
            "BillingContact",
            "phoneNumber",
            &past_phone_bound,
            "Member must have length less than or equal to 30",
        ),
        (
            "an admin contact email past its bound",
            "AdminContact",
            "email",
            &past_email_bound,
            "Member must have length less than or equal to 254",
        ),
    ];
    let emulator = Emulator::new();

    for (case, contact, member, submitted, requirement) in cases {
        // The modelled member name the request body carries is the clause's path segment
        // with its first letter upper-cased.
        let modelled = format!("{}{}", member[..1].to_uppercase(), &member[1..]);
        let mut detail = serde_json::Map::new();
        detail.insert(modelled, Value::String(submitted.to_string()));
        let mut request = serde_json::Map::new();
        request.insert(
            "DomainName".to_string(),
            Value::String("withheld-probe.com".to_string()),
        );
        request.insert(contact.to_string(), Value::Object(detail));
        let (status, body) = emulator
            .call("UpdateDomainContact", &Value::Object(request))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        let path = format!("{}{}.{member}", contact[..1].to_lowercase(), &contact[1..]);
        let expected = one_violation(&format!(
            "Value at '{path}' failed to satisfy constraint: {requirement}"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: a withheld member's clause states the member and the \
             requirement and no value, got {message:?}"
        );
        if !submitted.is_empty() {
            assert!(
                !message.contains(submitted),
                "case {case:?}: the submitted value must not be echoed, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The rule: a constraint a list's ELEMENTS break is stated at the list member
// ---------------------------------------------------------------------------

/// The value set a clause states for `OperationStatus`, in the order this service's front
/// door lists one.
///
/// The model declares that enum `[SUBMITTED, IN_PROGRESS, ERROR, SUCCESSFUL, FAILED]`
/// (`smithy_query(query="shape",shape="OperationStatus")`); as for
/// [`CONTACT_TYPE_SET`] the order below is the table's rather than the declaration's, and
/// holds exactly the values the model declares.
const OPERATION_STATUS_SET: &str = "[IN_PROGRESS, ERROR, SUCCESSFUL, SUBMITTED, FAILED]";

#[tokio::test]
async fn a_constraint_a_list_element_breaks_is_stated_at_the_list_member() {
    // One clause at the list member, quoting the whole list and nesting the element
    // shape's declared set, however many elements offended and wherever they sat: the rows
    // differ along how many of the list's elements break the constraint (one, one of two,
    // both) and along where the offending element sits.
    let cases: [(&str, Value, &str); 4] = [
        (
            "one offending element",
            json!(["BOGUS_STATUS"]),
            "[BOGUS_STATUS]",
        ),
        (
            "an offending element after a good one",
            json!(["SUBMITTED", "BOGUS_STATUS"]),
            "[SUBMITTED, BOGUS_STATUS]",
        ),
        (
            "an offending element before a good one",
            json!(["BOGUS_STATUS", "SUBMITTED"]),
            "[BOGUS_STATUS, SUBMITTED]",
        ),
        (
            "two offending elements",
            json!(["BOGUS_ONE", "BOGUS_TWO"]),
            "[BOGUS_ONE, BOGUS_TWO]",
        ),
    ];
    let emulator = Emulator::new();

    for (case, status_filter, quoted) in cases {
        let (status, body) = emulator
            .call("ListOperations", &json!({ "Status": status_filter }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        let expected = one_violation(&format!(
            "Value '{quoted}' at 'status' failed to satisfy constraint: Member must satisfy \
             constraint: [Member must satisfy enum value set: {OPERATION_STATUS_SET}]"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: one clause at the list member, quoting the list, \
             got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: a @pattern requirement states the expression, not its anchors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pattern_requirement_states_the_expression_without_its_anchors() {
    // `^`/`$` is how a model writes "the whole value", which the requirement sentence
    // already says, so the clause states the expression between them. Rows differ along
    // the member, the expression and the depth the member sits at: `IdnLangCode` targets
    // `LangCode` and a nameserver's `Name` targets `HostName`
    // (`smithy_query(query="shape",shape="LangCode,HostName")`).
    let lang_code = "Member must satisfy regular expression pattern: (|[A-Za-z]{2,3})";
    let host_name = "Member must satisfy regular expression pattern: [a-zA-Z0-9_\\-.]*";
    let cases: [(&str, &str, Value, &str, &str, &str); 4] = [
        (
            "a language name where a language code belongs",
            "CheckDomainAvailability",
            json!({ "DomainName": "pattern-probe.com", "IdnLangCode": "english" }),
            "english",
            "idnLangCode",
            lang_code,
        ),
        (
            "a language code of one letter",
            "CheckDomainAvailability",
            json!({ "DomainName": "pattern-probe.com", "IdnLangCode": "f" }),
            "f",
            "idnLangCode",
            lang_code,
        ),
        (
            "a language code of digits",
            "CheckDomainAvailability",
            json!({ "DomainName": "pattern-probe.com", "IdnLangCode": "12" }),
            "12",
            "idnLangCode",
            lang_code,
        ),
        (
            "a host name carrying a character the expression excludes",
            "UpdateDomainNameservers",
            json!({
                "DomainName": "pattern-probe.com",
                "Nameservers": [{ "Name": "ns one.example.com" }],
            }),
            "ns one.example.com",
            "nameservers.1.member.name",
            host_name,
        ),
    ];
    let emulator = Emulator::new();

    for (case, operation, request, submitted, path, requirement) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        let expected = one_violation(&format!(
            "Value '{submitted}' at '{path}' failed to satisfy constraint: {requirement}"
        ));
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: the clause states the expression without its anchors, got {body}"
        );
    }
}

#[tokio::test]
async fn a_nested_element_set_states_every_value_the_model_declares() {
    // The set a clause nests is the element shape's WHOLE declared set, not the one value
    // the request happened to break — read here off a list the issue's cases do not name,
    // `Type`, whose element enum the model declares with 21 values.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call("ListOperations", &json!({ "Type": ["BOGUS_TYPE"] }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "expected 400 InvalidInput, got {status} {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    let opened = format!(
        "{} Member must satisfy constraint: [Member must satisfy enum value set: [",
        one_violation("Value '[BOGUS_TYPE]' at 'type' failed to satisfy constraint:")
    );
    let stated = message
        .strip_prefix(opened.as_str())
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or_default();
    let mut listed: Vec<&str> = stated.split(", ").collect();
    listed.sort_unstable();
    let declared: Vec<&str> =
        service_com_amazonaws_route53domains_route53domains_v20140515::ops::types::OPERATION_TYPE_VALUES
            .to_vec();
    assert_eq!(
        listed, declared,
        "the nested set must list every declared `OperationType`, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// The rule: a suggestion is a registrable name derived from the requested one
// ---------------------------------------------------------------------------

/// A `GetDomainSuggestions` request body.
fn suggestion_request(name: &str, count: i64, only_available: bool) -> Value {
    json!({
        "DomainName": name,
        "SuggestionCount": count,
        "OnlyAvailable": only_available,
    })
}

/// `GetDomainSuggestions` for `request`, asserting a 200 and returning `SuggestionsList`.
async fn suggest(emulator: &Emulator, case: &str, request: &Value) -> Vec<Value> {
    let (status, body) = emulator.call("GetDomainSuggestions", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDomainSuggestions to answer 200, got {status} {body}"
    );
    body["SuggestionsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `DomainName` of every suggestion, in the order the answer listed them.
fn suggested_names(suggestions: &[Value]) -> Vec<String> {
    suggestions
        .iter()
        .map(|suggestion| {
            suggestion["DomainName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_suggestion_is_a_registrable_name_derived_from_the_requested_one() {
    // Rows differ along the TLD the caller asked under (generic, country-code, a
    // two-label suffix, one the price table does not carry), the shape of the label, the
    // number of names asked for (both documented bounds among them) and the case of the
    // request.
    let cases: [(&str, i64); 6] = [
        ("sim-probe-suggest.com", 1),
        ("sim-probe-suggest.com", 7),
        ("z0-9.io", 50),
        ("sim-probe-suggest.co.uk", 12),
        // A TLD outside the published rate table is still a TLD a registry delegates.
        ("sim-probe-suggest.shop", 5),
        // Domain names are not case sensitive.
        ("SIM-PROBE-SUGGEST.COM", 7),
    ];
    let emulator = Emulator::new();

    for (requested, count) in cases {
        let case = format!("{requested} x{count}");
        let lowered = requested.to_lowercase();
        let (label, tld) = lowered.split_once('.').expect("a label and a suffix");
        // `OnlyAvailable` false, so nothing is filtered out and the bound on the answer is
        // the only thing that limits it.
        let suggestions = suggest(
            &emulator,
            &case,
            &suggestion_request(requested, count, false),
        )
        .await;
        let names = suggested_names(&suggestions);

        assert_eq!(
            names.len(),
            usize::try_from(count).expect("a positive count"),
            "case {case:?}: at most, and here exactly, the requested number of \
             suggestions, got {names:?}"
        );
        let unique: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {case:?}: a suggestion must not repeat, got {names:?}"
        );

        for name in &names {
            assert_ne!(
                name, &lowered,
                "case {case:?}: the requested name is never suggested back, got {names:?}"
            );
            let (suggested_label, _) = name.split_once('.').unwrap_or((name.as_str(), ""));
            assert!(
                suggested_label.contains(label),
                "case {case:?}: a suggestion is derived from the requested label \
                 {label:?}, got {name:?}"
            );
            // Registrable: a caller can take any suggestion straight to the registry.
            let body = check(&emulator, name).await;
            assert!(
                body["Availability"].is_string(),
                "case {case:?}: suggestion {name:?} must be a name the registry answers \
                 for, got {body}"
            );
        }
        assert!(
            names.iter().any(|name| name.ends_with(&format!(".{tld}")))
                || names
                    .iter()
                    .any(|name| name.starts_with(&format!("{label}."))),
            "case {case:?}: suggestions keep either the requested label or the requested \
             suffix, got {names:?}"
        );

        // The same question twice is the same answer, in the same order: the list is
        // computed from the requested name, not from anything that varies per call.
        let again = suggest(
            &emulator,
            &case,
            &suggestion_request(requested, count, false),
        )
        .await;
        assert_eq!(
            suggested_names(&again),
            names,
            "case {case:?}: the same request must answer the same list"
        );
        let other_case = suggest(
            &emulator,
            &case,
            &suggestion_request(&requested.to_uppercase(), count, false),
        )
        .await;
        assert_eq!(
            suggested_names(&other_case),
            names,
            "case {case:?}: domain names are not case sensitive, so the same name in \
             another case answers the same list"
        );
    }
}

#[tokio::test]
async fn suggestions_are_unchecked_and_unpriced_unless_only_available_is_true() {
    // `DomainSuggestion.Availability` and `.Price` are each "populated only if you
    // specified true for OnlyAvailable in the request", and with false Route 53 "returns
    // domain names without checking whether they're available" — so a name the registry
    // already holds is among them. Rows differ along the requested name, including one
    // whose label is held by a third party under every generic suffix.
    let emulator = Emulator::new();

    for requested in ["example.io", "sim-probe-unchecked.com", "z0-9.dev"] {
        let suggestions = suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 30, false),
        )
        .await;
        assert!(
            !suggestions.is_empty(),
            "case {requested:?}: expected suggestions, got none"
        );
        for suggestion in &suggestions {
            assert!(
                suggestion.get("Availability").is_none_or(Value::is_null),
                "case {requested:?}: Availability is populated only for OnlyAvailable \
                 true, got {suggestion}"
            );
            assert!(
                suggestion.get("Price").is_none_or(Value::is_null),
                "case {requested:?}: Price is populated only for OnlyAvailable true, \
                 got {suggestion}"
            );
        }
    }

    // Nothing was filtered: the names a third party holds are suggested too, and the same
    // request with `OnlyAvailable` true leaves exactly those out.
    let unchecked = suggested_names(
        &suggest(
            &emulator,
            "example.io unchecked",
            &suggestion_request("example.io", 30, false),
        )
        .await,
    );
    let available = suggested_names(
        &suggest(
            &emulator,
            "example.io available",
            &suggestion_request("example.io", 30, true),
        )
        .await,
    );
    for held in ["example.com", "example.net", "example.org"] {
        assert!(
            unchecked.contains(&held.to_string()),
            "case {held:?}: an unchecked answer suggests a held name too, got {unchecked:?}"
        );
        assert!(
            !available.contains(&held.to_string()),
            "case {held:?}: an OnlyAvailable answer leaves a held name out, got {available:?}"
        );
    }
}

#[tokio::test]
async fn an_only_available_suggestion_carries_the_registrys_own_answer_for_it() {
    // Rows differ along the TLD asked under and the number asked for; every suggestion is
    // then checked against `CheckDomainAvailability` for that same name, which is the one
    // answer the registry has for it.
    let cases: [(&str, i64); 4] = [
        ("sim-probe-match.com", 1),
        ("sim-probe-match.io", 9),
        ("sim-probe-match.co.uk", 20),
        ("example.net", 15),
    ];
    let emulator = Emulator::new();

    for (requested, count) in cases {
        let case = format!("{requested} x{count}");
        let suggestions = suggest(
            &emulator,
            &case,
            &suggestion_request(requested, count, true),
        )
        .await;
        assert!(
            !suggestions.is_empty(),
            "case {case:?}: expected suggestions, got none"
        );
        assert!(
            suggestions.len() <= usize::try_from(count).expect("a positive count"),
            "case {case:?}: never more than the requested number, got {}",
            suggestions.len()
        );

        for suggestion in &suggestions {
            let name = suggestion["DomainName"].as_str().unwrap_or_default();
            let checked = check(&emulator, name).await;
            assert_eq!(
                suggestion["Availability"].as_str(),
                Some("AVAILABLE"),
                "case {case:?}: an OnlyAvailable suggestion is AVAILABLE, got {suggestion}"
            );
            assert_eq!(
                suggestion["Availability"], checked["Availability"],
                "case {case:?}: suggestion {name:?} must carry the registry's own \
                 answer, got {suggestion} against {checked}"
            );
            assert_eq!(
                suggestion["Price"], checked["Price"],
                "case {case:?}: suggestion {name:?} must carry the standard price of its \
                 TLD, got {suggestion} against {checked}"
            );
        }
    }
}

#[tokio::test]
async fn registering_a_suggested_name_takes_it_out_of_the_available_suggestions() {
    // The filter reads the registrar's store, so a name this account registers stops
    // being suggested as available — and is still suggested when nothing is checked.
    let emulator = Emulator::new();
    let requested = "sim-probe-pick.com";
    let before = suggested_names(
        &suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 50, true),
        )
        .await,
    );
    let taken = before
        .first()
        .cloned()
        .expect("at least one available suggestion");

    let (status, body) = emulator
        .call(
            "RegisterDomain",
            &registration(&taken, 1, &person_contact()),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {taken:?}: expected RegisterDomain to succeed, got {status} {body}"
    );
    // Read it back through the implemented reader: the suggestion is now a registration.
    let (status, detail) = emulator
        .call("GetDomainDetail", &json!({ "DomainName": taken }))
        .await;
    assert_eq!(
        (status, detail["DomainName"].as_str()),
        (200, Some(taken.as_str())),
        "case {taken:?}: expected GetDomainDetail to report the registration, got {status} {detail}"
    );

    let after = suggested_names(
        &suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 50, true),
        )
        .await,
    );
    assert!(
        !after.contains(&taken),
        "case {taken:?}: a registered name is no longer an available suggestion, got {after:?}"
    );
    let unchecked = suggested_names(
        &suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 50, false),
        )
        .await,
    );
    assert!(
        unchecked.contains(&taken),
        "case {taken:?}: an unchecked answer suggests it all the same, got {unchecked:?}"
    );

    // Deleting it puts it back among the available suggestions.
    let (status, body) = emulator
        .call("DeleteDomain", &json!({ "DomainName": taken }))
        .await;
    assert_eq!(
        status, 200,
        "case {taken:?}: expected DeleteDomain to succeed, got {status} {body}"
    );
    let restored = suggested_names(
        &suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 50, true),
        )
        .await,
    );
    assert_eq!(
        restored, before,
        "case {taken:?}: deleting the registration restores the earlier suggestions"
    );
}

#[tokio::test]
async fn only_standard_prices_selects_nothing_out_of_the_suggestions() {
    // Every name this registry quotes is priced at the standard rate of its TLD, so the
    // member that keeps premium-priced names out ("If true, Route 53 returns only domain
    // names that are priced at the standard registration rate for their TLD") cannot
    // change the answer. Rows differ along the value the member carries, absent included.
    let emulator = Emulator::new();
    let requested = "sim-probe-standard.com";
    let baseline = suggested_names(
        &suggest(
            &emulator,
            requested,
            &suggestion_request(requested, 10, true),
        )
        .await,
    );

    for only_standard in [Some(true), Some(false), None] {
        let mut request = suggestion_request(requested, 10, true);
        if let Some(flag) = only_standard {
            request["OnlyStandardPrices"] = json!(flag);
        }
        let case = format!("OnlyStandardPrices {only_standard:?}");
        let names = suggested_names(&suggest(&emulator, &case, &request).await);
        assert_eq!(
            names, baseline,
            "case {case:?}: every suggested name is priced at its TLD's standard rate, \
             so the answer is the same"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: a suggestion request outside what the model documents is refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_suggestion_count_outside_the_documented_range_is_refused() {
    // `SuggestionCount`: "Specify a value between 1 and 50." The model declares no
    // `@range`, so the service enforces it, and states the rule rather than the value it
    // was handed: the sentence is the same for every refused count. Rows differ along
    // which side of the range the value falls and how far outside it sits — each
    // published bound, one past it, and a value far beyond.
    let refused: [i64; 5] = [0, -1, 51, 100, 1000];
    let emulator = Emulator::new();

    for count in refused {
        let (status, body) = emulator
            .call(
                "GetDomainSuggestions",
                &suggestion_request("sim-probe-count.com", count, true),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {count}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid request. Problems: Suggestion count must be a value between 1 and 50"),
            "case {count}: the refusal states the documented range, got {body}"
        );
    }

    // Both published bounds are accepted.
    for count in [1_i64, 50] {
        let case = format!("count {count}");
        let suggestions = suggest(
            &emulator,
            &case,
            &suggestion_request("sim-probe-count.com", count, true),
        )
        .await;
        assert!(
            !suggestions.is_empty()
                && suggestions.len() <= usize::try_from(count).expect("a positive count"),
            "case {case:?}: a count inside the documented range is answered, got {} \
             suggestions",
            suggestions.len()
        );
    }
}

#[tokio::test]
async fn a_request_that_leaves_the_suggestion_count_out_is_refused_as_well() {
    // `SuggestionCount` is modelled `@required`, so a body that leaves it out never
    // reaches the handler at all: the boundary refuses the absent member in this service's
    // validation envelope, under the one client-input error it declares.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call(
            "GetDomainSuggestions",
            &json!({ "DomainName": "sim-probe-count.com", "OnlyAvailable": true }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "expected 400 InvalidInput, got {status} {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(
            one_violation(
                "Value null at 'suggestionCount' failed to satisfy constraint: Member must \
                 not be null"
            )
            .as_str()
        ),
        "an absent required member is refused as absent, got {body}"
    );
}

#[tokio::test]
async fn a_name_the_registry_cannot_answer_for_is_refused_by_the_suggestions_too() {
    // The requested name is resolved by the one rule every domain-scoped operation
    // applies, so `GetDomainSuggestions` refuses what `CheckDomainAvailability` refuses
    // and in the same words. Rows differ along the problem: a suffix no registry
    // delegates, a name below the level a registrar sells, and a member carrying no name.
    let cases: [(&str, &str, &str); 4] = [
        (
            "sim-probe.doesnotexisttld",
            "UnsupportedTLD",
            "The doesnotexisttld TLD is not supported",
        ),
        (
            "sim-probe.invalid",
            "UnsupportedTLD",
            "The invalid TLD is not supported",
        ),
        (
            "a.b.example.com",
            "UnsupportedTLD",
            "The b.example.com TLD is not supported",
        ),
        (
            "",
            "InvalidInput",
            "Invalid request. Problems: Domain name cannot be blank",
        ),
    ];
    let emulator = Emulator::new();

    for (requested, code, message) in cases {
        let (status, body) = emulator
            .call(
                "GetDomainSuggestions",
                &suggestion_request(requested, 5, true),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, code.to_string()),
            "case {requested:?}: expected 400 {code}, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message),
            "case {requested:?}: expected the same words `CheckDomainAvailability` uses, \
             got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: a TLD's price row is derived from that TLD's standard rate
// ---------------------------------------------------------------------------

/// What an owner change costs at this registrar, in USD. Source: `CHANGE_OWNERSHIP_RATE`
/// in `src/state/registry.rs` — handing a registration to a new owner buys no
/// registration year, so the published list charges nothing for it.
const CHANGE_OWNERSHIP: f64 = 0.00;

/// What restoring an expired registration costs, in USD. Source: `RESTORATION_RATE` in
/// `src/state/registry.rs` — one flat registrar fee, the same for every TLD.
const RESTORATION: f64 = 40.00;

/// `ListPrices` with `request`, asserting a 200 and returning its `Prices` rows and its
/// `NextPageMarker`.
async fn list_prices(
    emulator: &Emulator,
    case: &str,
    request: &Value,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = emulator.call("ListPrices", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPrices to answer 200, got {status} {body}"
    );
    let rows = body["Prices"].as_array().cloned().unwrap_or_default();
    let marker = body["NextPageMarker"].as_str().map(str::to_string);
    (rows, marker)
}

/// The `Name` of every row of a price page, in the order the page reported them.
fn priced_names(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| row["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert that `row` is the price row of `name`, a TLD whose standard yearly rate is
/// `rate`: registration, transfer and renewal each buy one year at that rate, an owner
/// change costs [`CHANGE_OWNERSHIP`] and a restoration [`RESTORATION`], and every one of
/// the five is quoted in the required `Currency` at `PriceType` `STANDARD`.
fn assert_price_row(row: &Value, case: &str, name: &str, rate: f64) {
    assert_eq!(
        row["Name"].as_str(),
        Some(name),
        "case {case:?}: Name reports the TLD the prices apply to, got {row}"
    );
    for (member, expected) in [
        ("RegistrationPrice", rate),
        ("TransferPrice", rate),
        ("RenewalPrice", rate),
        ("ChangeOwnershipPrice", CHANGE_OWNERSHIP),
        ("RestorationPrice", RESTORATION),
    ] {
        let price = &row[member];
        assert_eq!(
            price["Price"].as_f64(),
            Some(expected),
            "case {case:?}: expected {member} {expected}, got {price}"
        );
        assert!(
            price["Price"].as_f64().is_some_and(|value| value >= 0.0),
            "case {case:?}: {member}'s Price is @range(min: 0), got {price}"
        );
        assert_eq!(
            price["Currency"].as_str(),
            Some("USD"),
            "case {case:?}: {member} carries the required Currency member, got {price}"
        );
        assert_eq!(
            price["PriceType"].as_str(),
            Some("STANDARD"),
            "case {case:?}: a regular TLD rate is PriceType STANDARD, got {price}"
        );
    }
}

#[tokio::test]
async fn the_price_row_of_one_tld_is_derived_from_that_tlds_standard_rate() {
    // Rows differ along the TLD — generic, country-code, two-label suffix, and one the
    // published table does not list, which takes the stated default rate — and, for each
    // of them, along the FORM the caller writes it in: bare, with the leading dot the
    // `Tld` member's own example uses, and upper-cased, since a TLD is not case
    // sensitive.
    let emulator = Emulator::new();

    for (tld, rate) in EXPECTED_RATES {
        for requested in [(*tld).to_string(), format!(".{tld}"), tld.to_uppercase()] {
            let (rows, marker) =
                list_prices(&emulator, &requested, &json!({ "Tld": requested })).await;
            assert_eq!(
                rows.len(),
                1,
                "case {requested:?}: a request for one TLD prices exactly that TLD, got {rows:?}"
            );
            assert_price_row(&rows[0], &requested, tld, *rate);
            assert!(
                marker.is_none(),
                "case {requested:?}: one TLD is one page, so no NextPageMarker, got {marker:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_tld_no_registry_delegates_is_refused_in_the_same_words_as_every_other_surface() {
    // A TLD a registrar cannot offer names under is the one condition `UnsupportedTLD`
    // names, and it is the same condition whichever member the TLD arrived in: each row
    // is asked of `ListPrices` as a `Tld` and of `CheckDomainAvailability` as the suffix
    // of a `DomainName`, and both answers have to agree. Rows differ along why the value
    // is not a suffix a registry delegates: an unassigned label, two labels reserved by
    // RFC 2606, and a value that is a whole domain name rather than a suffix.
    let cases = ["doesnotexisttld", "zzz", "invalid", "test", "example.com"];
    let emulator = Emulator::new();

    for tld in cases {
        let (status, body) = emulator.call("ListPrices", &json!({ "Tld": tld })).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UnsupportedTLD".to_string()),
            "case {tld:?}: a TLD no registry delegates is refused as unsupported, got \
             {status} {body}"
        );
        let expected = format!("The {tld} TLD is not supported");
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {tld:?}: expected {expected:?}, got {body}"
        );

        let name = format!("sim-probe-prices.{tld}");
        let (availability_status, availability) = emulator
            .call("CheckDomainAvailability", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (
                availability_status,
                error_code(&availability),
                availability["message"].as_str()
            ),
            (400, "UnsupportedTLD".to_string(), Some(expected.as_str())),
            "case {tld:?}: every surface that resolves this TLD refuses it identically, \
             got {availability_status} {availability}"
        );
    }
}

#[tokio::test]
async fn the_whole_price_list_comes_back_in_tld_order_and_agrees_with_the_other_surfaces() {
    // An unqualified request returns the registrar's published list: "If a Tld value is
    // not provided, a list of prices for all TLDs supported by Route 53 is returned."
    let emulator = Emulator::new();
    let (rows, marker) = list_prices(&emulator, "all TLDs", &json!({})).await;
    let names = priced_names(&rows);

    assert!(
        rows.len() > 1,
        "the published price list carries more than one TLD, got {names:?}"
    );
    assert!(
        marker.is_none(),
        "the whole list is one page, so no NextPageMarker, got {marker:?}"
    );
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, names,
        "the list is in ascending TLD order with no TLD listed twice, got {names:?}"
    );

    // Every row is the SAME row a request for that one TLD answers with, and its
    // registration price is the one `CheckDomainAvailability` quotes for a name under
    // that TLD: one price rule, every surface.
    for row in &rows {
        let tld = row["Name"].as_str().unwrap_or_default().to_string();
        let (one, _) = list_prices(&emulator, &tld, &json!({ "Tld": tld })).await;
        assert_eq!(
            one.first(),
            Some(row),
            "case {tld:?}: the list's row and the one-TLD row are the same prices"
        );

        let name = format!("sim-probe-prices.{tld}");
        let body = check(&emulator, &name).await;
        assert_eq!(
            body["Price"], row["RegistrationPrice"],
            "case {tld:?}: an available name is priced at the registration price its TLD \
             is listed at, got {body}"
        );
    }
}

#[tokio::test]
async fn a_page_of_the_price_list_walks_the_whole_list_by_marker() {
    // The marker walk reaches every row exactly once whatever the page size, so the rows
    // differ along `MaxItems`: one row at a time, a size that does not divide the list,
    // and the published maximum, which is larger than the whole list.
    let emulator = Emulator::new();
    let (all, _) = list_prices(&emulator, "all TLDs", &json!({})).await;
    let expected = priced_names(&all);

    for max_items in [1_usize, 7, 1000] {
        let case = format!("MaxItems {max_items}");
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // Each page makes progress, so the walk cannot take more pages than there are
        // rows; a walk that exceeds that bound is a marker that does not advance.
        for _ in 0..=expected.len() {
            let mut request = json!({ "MaxItems": max_items });
            if let Some(marker) = marker.as_deref() {
                request["Marker"] = json!(marker);
            }
            let (page, next) = list_prices(&emulator, &case, &request).await;
            let names = priced_names(&page);
            assert!(
                !names.is_empty(),
                "case {case:?}: a page of the list is never empty, got {page:?}"
            );
            assert!(
                names.len() <= max_items,
                "case {case:?}: a page holds at most MaxItems rows, got {names:?}"
            );
            match next.as_deref() {
                Some(next) => {
                    assert_eq!(
                        names.len(),
                        max_items,
                        "case {case:?}: a page that is not the last is full, got {names:?}"
                    );
                    assert_eq!(
                        Some(next),
                        names.last().map(String::as_str),
                        "case {case:?}: NextPageMarker is the last row of the page, got \
                         {names:?}"
                    );
                }
                None => assert!(
                    names.len() <= max_items,
                    "case {case:?}: the last page holds what is left, got {names:?}"
                ),
            }
            collected.extend(names);
            marker = next;
            if marker.is_none() {
                break;
            }
        }
        assert!(
            marker.is_none(),
            "case {case:?}: the walk ended with a marker still outstanding ({marker:?})"
        );
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages together are the whole list, in order"
        );
    }
}

#[tokio::test]
async fn a_page_size_or_marker_the_list_cannot_serve_is_refused() {
    // A page of no rows could never walk the list, so the handler's lower bound on
    // `MaxItems` is 1; the upper bound is the model's `@range(max: 1000)`
    // (`smithy_query(query="shape",shape="ListPricesPageMaxItems")`), refused one past it
    // by the boundary validator in this service's validation envelope. A `Marker` that no
    // page of this list ever reported names no row to resume from.
    let emulator = Emulator::new();

    // (case, request, the message the refusal carries)
    let cases: [(&str, Value, String); 5] = [
        (
            "a page of no rows",
            json!({ "MaxItems": 0 }),
            "Value '0' at 'maxItems' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "a negative page",
            json!({ "MaxItems": -3 }),
            "Value '-3' at 'maxItems' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "one past the published maximum",
            json!({ "MaxItems": 1001 }),
            one_violation(
                "Value '1001' at 'maxItems' failed to satisfy constraint: Member must have \
                 value less than or equal to 1000",
            ),
        ),
        (
            "a marker no page reported",
            json!({ "Marker": "not-a-tld-of-this-list" }),
            "Value 'not-a-tld-of-this-list' at 'marker' failed to satisfy constraint: Member \
             must be the NextPageMarker of a previous ListPrices response"
                .to_string(),
        ),
        (
            "a marker that is a TLD outside the published list",
            json!({ "Marker": "shop" }),
            "Value 'shop' at 'marker' failed to satisfy constraint: Member must be the \
             NextPageMarker of a previous ListPrices response"
                .to_string(),
        ),
    ];

    for (case, request, message) in cases {
        let (status, body) = emulator.call("ListPrices", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message.as_str()),
            "case {case:?}: expected {message:?}, got {body}"
        );
    }

    // Both ends of the page sizes the handler serves are accepted.
    for max_items in [1_i64, 1000] {
        let case = format!("MaxItems {max_items}");
        let (rows, _) = list_prices(&emulator, &case, &json!({ "MaxItems": max_items })).await;
        assert!(
            !rows.is_empty(),
            "case {case:?}: a page size inside the bounds is served, got {rows:?}"
        );
    }
}
