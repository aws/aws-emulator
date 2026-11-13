// SPDX-License-Identifier: CC0-1.0

//! The registry's answer to `CheckDomainTransferability`, over awsJson1_1: what it says
//! about a name, and what the response carries for each answer.
//!
//! The rules pinned here:
//!
//! * a transfer in moves an EXISTING registration (`TransferDomain` "[t]ransfers a
//!   domain from another registrar to Amazon Route 53"), so `Transferable` turns on
//!   WHERE the name's registration lives: a name this account already holds is
//!   `DOMAIN_IN_OWN_ACCOUNT` ("The domain already exists in the current Amazon Web
//!   Services account") and every other name is `UNTRANSFERABLE` ("The domain name can't
//!   be transferred to Route 53") — a name no registry holds has no registration to
//!   move, and a third party's registration moves only on that registrant's
//!   authorization;
//! * so a name `CheckDomainAvailability` reports `AVAILABLE` is never transferable, and
//!   the two operations read one record: availability says WHETHER the name is
//!   registered, transferability says WHERE;
//! * `Message` explains the answer (`CheckDomainTransferabilityResponse.Message`:
//!   "Provides an explanation for when a domain can't be transferred"), naming the
//!   requested domain;
//! * `Price` is present whenever the answer is determined
//!   (`CheckDomainTransferabilityResponse.Price`: "This element is returned only when
//!   Route 53 can determine whether the domain is transferable") and carries the standard
//!   transfer rate of the name's TLD with its required `Price` and `Currency` members and
//!   `PriceType` `STANDARD`;
//! * domain names are not case sensitive (the `DomainName` member's own documentation),
//!   so the case of the request selects nothing in either operation;
//! * `AuthCode` belongs to the registrar the name is leaving, and no answer this
//!   operation documents depends on one;
//! * a name the registry cannot answer for at all is refused, not answered.
//!
//! Every test drives the real wire path through the shared harness in
//! `tests/all/domain_lifecycle.rs`.

use crate::domain_lifecycle::{error_code, person_contact, register, registration, Emulator};
use serde_json::{json, Value};

/// The members `Transferable` accepts on the wire. Source: the `Transferable` enum in
/// this service's Smithy model (`smithy_query(query="shape",shape="Transferable")`),
/// which `crate::ops::types::TRANSFERABLE_VALUES` carries too.
const TRANSFERABLE_MEMBERS: &[&str] = &[
    "DOMAIN_IN_ANOTHER_ACCOUNT",
    "DOMAIN_IN_OWN_ACCOUNT",
    "DONT_KNOW",
    "PENDING",
    "PREMIUM_DOMAIN",
    "TRANSFERABLE",
    "UNTRANSFERABLE",
];

/// The standard transfer rates these cases expect, in USD. Source: the per-TLD table
/// `STANDARD_REGISTRATION_RATES` in `src/state/registry.rs`, which a transfer in is
/// charged at too (`standard_transfer_price`); a TLD the table does not list takes
/// `DEFAULT_STANDARD_REGISTRATION_RATE` (20.00), which the `.shop` row below exercises.
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

/// `CheckDomainTransferability` for `name`, asserting a 200 and returning the body.
async fn check(emulator: &Emulator, name: &str) -> Value {
    let (status, body) = emulator
        .call("CheckDomainTransferability", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CheckDomainTransferability to answer 200, got {status} {body}"
    );
    body
}

/// `CheckDomainAvailability`'s `Availability` for `name`, asserting a 200.
async fn availability(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call("CheckDomainAvailability", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CheckDomainAvailability to answer 200, got {status} {body}"
    );
    body["Availability"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The reported `Transferable` member of a `CheckDomainTransferability` body.
fn transferable(body: &Value) -> String {
    body["Transferability"]["Transferable"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `body` reports `member`, explains it with `message`, and prices the
/// transfer at `rate`.
fn assert_answer(body: &Value, case: &str, member: &str, message: &str, rate: f64) {
    let reported = transferable(body);
    assert!(
        TRANSFERABLE_MEMBERS.contains(&reported.as_str()),
        "case {case:?}: Transferable must be a modelled member, got {reported:?} in {body}"
    );
    assert_eq!(
        reported, member,
        "case {case:?}: expected Transferable {member}, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(message),
        "case {case:?}: the answer must be explained, got {body}"
    );

    let price = &body["Price"];
    assert!(
        price.is_object(),
        "case {case:?}: Price is returned whenever the answer is determined, got {body}"
    );
    assert_eq!(
        price["Price"].as_f64(),
        Some(rate),
        "case {case:?}: expected the TLD's standard transfer rate {rate}, got {price}"
    );
    assert!(
        price["Price"].as_f64().is_some_and(|value| value >= 0.0),
        "case {case:?}: Price is @range(min: 0), got {price}"
    );
    assert_eq!(
        price["Currency"].as_str(),
        Some("USD"),
        "case {case:?}: expected the required Currency member, got {price}"
    );
    assert_eq!(
        price["PriceType"].as_str(),
        Some("STANDARD"),
        "case {case:?}: a regular TLD rate is PriceType STANDARD, got {price}"
    );
}

/// The explanation a name no registry holds is answered with.
fn not_registered_message(name: &str) -> String {
    format!("The domain {name} isn't registered. Register the domain first and then try again.")
}

/// The explanation a name another registrant holds is answered with.
fn held_elsewhere_message(name: &str) -> String {
    format!(
        "The domain {name} is registered with another registrant. Ask the current registrant to \
         authorize the transfer and then try again."
    )
}

/// The explanation a name this account holds is answered with.
fn own_account_message(name: &str) -> String {
    format!("The domain {name} already exists in the current Amazon Web Services account.")
}

// ---------------------------------------------------------------------------
// The rule: a name no registry holds has no registration to transfer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_no_registry_holds_is_untransferable() {
    // Rows differ along the TLD (generic, country-code, two-label suffix, a TLD the
    // price table does not list) and along the shape of the label. Two labels are
    // checked under every TLD, because a transfer rate belongs to the TLD rather than
    // to the name.
    let labels = ["sim-probe-free", "z0-9"];
    let emulator = Emulator::new();

    for (tld, rate) in EXPECTED_RATES {
        for label in labels {
            let name = format!("{label}.{tld}");
            let body = check(&emulator, &name).await;
            assert_answer(
                &body,
                &name,
                "UNTRANSFERABLE",
                &not_registered_message(&name),
                *rate,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The rule: a registration another registrant holds moves only on their authorization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_another_registrant_holds_is_untransferable_and_says_so() {
    // Rows differ along the TLD and along the case of the request. Every one of these
    // names is registered in the real registries and none of them is held by this
    // account, which is exactly the case an answer keyed to this account's store gets
    // wrong: it is registered, but not here.
    let held = [
        ("google.com", 16.00),
        ("amazon.net", 17.00),
        ("example.org", 16.00),
        ("x.com", 16.00),
        ("iana.org", 16.00),
        ("amazon.co.uk", 10.00),
        ("amazon.de", 10.00),
        ("GOOGLE.COM", 16.00),
        ("Example.Org", 16.00),
    ];
    let emulator = Emulator::new();

    for (name, rate) in held {
        let body = check(&emulator, name).await;
        // Domain names are not case sensitive, so the message names the lower-cased
        // name whatever case the request used.
        let lower = name.to_lowercase();
        assert_answer(
            &body,
            name,
            "UNTRANSFERABLE",
            &held_elsewhere_message(&lower),
            rate,
        );
    }

    // Controls: names nobody holds under the same TLDs are answered for being
    // unregistered, not for being somebody else's, so the two situations stay apart.
    for name in [
        "sim-probe-control.com",
        "sim-probe-control.net",
        "sim-probe-control.org",
        "sim-probe-control.co.uk",
    ] {
        let body = check(&emulator, name).await;
        assert_eq!(
            body["Message"].as_str(),
            Some(not_registered_message(name).as_str()),
            "case {name:?}: a name nobody holds is answered for being unregistered, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: a name this account holds is already here
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registering_a_name_puts_it_in_this_account_and_deleting_it_frees_it_again() {
    // The answer follows the store in both directions. Rows differ along the TLD, so
    // the rate each row's price carries differs too.
    let cases = [
        ("sim-probe-own.com", 16.00),
        ("sim-probe-own.io", 47.00),
        ("sim-probe-own.co.uk", 10.00),
    ];
    let emulator = Emulator::new();

    for (name, rate) in cases {
        let body = check(&emulator, name).await;
        assert_answer(
            &body,
            name,
            "UNTRANSFERABLE",
            &not_registered_message(name),
            rate,
        );

        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, &person_contact()))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected RegisterDomain to succeed, got {status} {body}"
        );

        // Read the registration back through the implemented reader: the name this
        // account now holds is the name now reported as already being here.
        let (status, detail) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            (status, detail["DomainName"].as_str()),
            (200, Some(name)),
            "case {name:?}: expected GetDomainDetail to report the registration, got {status} {detail}"
        );

        let body = check(&emulator, name).await;
        assert_answer(
            &body,
            name,
            "DOMAIN_IN_OWN_ACCOUNT",
            &own_account_message(name),
            rate,
        );

        let (status, body) = emulator
            .call("DeleteDomain", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteDomain to succeed, got {status} {body}"
        );

        let body = check(&emulator, name).await;
        assert_answer(
            &body,
            name,
            "UNTRANSFERABLE",
            &not_registered_message(name),
            rate,
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: availability and transferability read the same record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_available_name_is_never_transferable() {
    // Rows span every answer the registry can give, under several TLDs: names nobody
    // holds, names a third party holds, and a name this account holds. A name reported
    // AVAILABLE has no registration anywhere, so it is answered for being unregistered;
    // a name reported UNAVAILABLE is answered by WHERE its registration lives.
    let emulator = Emulator::new();
    register(&emulator, "sim-probe-agree-own.net").await;

    let names = [
        "sim-probe-agree.com",
        "sim-probe-agree.io",
        "sim-probe-agree.co.uk",
        "sim-probe-agree.shop",
        "google.com",
        "example.org",
        "amazon.co.uk",
        "sim-probe-agree-own.net",
    ];

    for name in names {
        let reported = availability(&emulator, name).await;
        let body = check(&emulator, name).await;
        let member = transferable(&body);
        assert_ne!(
            member, "TRANSFERABLE",
            "case {name:?}: no registration this registry knows of can move in, got {body}"
        );
        if reported == "AVAILABLE" {
            assert_eq!(
                (member.as_str(), body["Message"].as_str()),
                ("UNTRANSFERABLE", Some(not_registered_message(name).as_str())),
                "case {name:?}: an AVAILABLE name has no registration to transfer, got {body}"
            );
        } else {
            assert!(
                body["Message"]
                    .as_str()
                    .is_some_and(|message| message != not_registered_message(name)),
                "case {name:?}: an UNAVAILABLE name is registered, so it cannot be answered as \
                 unregistered, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn checking_a_name_twice_answers_the_same_thing() {
    // The operation is a pure read: nothing a caller sees may change because somebody
    // asked. Rows: a name nobody holds, a name the registry holds for somebody else,
    // and one this account holds.
    let emulator = Emulator::new();
    register(&emulator, "sim-probe-repeat-own.com").await;

    for name in [
        "sim-probe-repeat.com",
        "google.com",
        "sim-probe-repeat-own.com",
    ] {
        let first = check(&emulator, name).await;
        let second = check(&emulator, name).await;
        assert_eq!(
            first, second,
            "case {name:?}: a second check must answer exactly the first, got {first} then {second}"
        );
        // Nor may it have moved the availability answer.
        let before = availability(&emulator, name).await;
        let _ = check(&emulator, name).await;
        let after = availability(&emulator, name).await;
        assert_eq!(
            before, after,
            "case {name:?}: a check must not change what a later caller sees, got {before:?} then \
             {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: neither the case of the name nor an auth code selects anything
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_case_of_the_request_selects_nothing() {
    // Domain names are not case sensitive, so each row's variants must answer exactly
    // what the lower-cased name answers — in BOTH operations. Rows differ along the
    // answer (unheld, held by a third party, held by this account) and along where the
    // upper-case letters fall.
    let emulator = Emulator::new();
    register(&emulator, "sim-probe-case-own.com").await;

    for name in [
        "sim-probe-case.com",
        "google.com",
        "sim-probe-case-own.com",
        "sim-probe-case.co.uk",
    ] {
        let expected_transferability = check(&emulator, name).await;
        let expected_availability = availability(&emulator, name).await;

        let variants = [
            name.to_uppercase(),
            // Only the first label upper-cased, and only the suffix upper-cased.
            name.split_once('.').map_or_else(
                || name.to_uppercase(),
                |(label, rest)| format!("{}.{rest}", label.to_uppercase()),
            ),
            name.split_once('.').map_or_else(
                || name.to_uppercase(),
                |(label, rest)| format!("{label}.{}", rest.to_uppercase()),
            ),
        ];

        for variant in variants {
            let body = check(&emulator, &variant).await;
            assert_eq!(
                body, expected_transferability,
                "case {variant:?}: domain names are not case sensitive, so this must answer \
                 exactly what {name:?} answers ({expected_transferability}), got {body}"
            );
            let reported = availability(&emulator, &variant).await;
            assert_eq!(
                reported, expected_availability,
                "case {variant:?}: expected Availability {expected_availability:?}, got \
                 {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_auth_code_does_not_change_the_answer() {
    // `AuthCode` is the leaving registrar's code; no answer this operation documents
    // depends on one, so a request that carries one answers exactly as the request that
    // does not. Rows differ along the answer AND along the code: empty, a short code,
    // and one at the member's 1024-character length bound.
    let emulator = Emulator::new();
    register(&emulator, "sim-probe-authcode-own.com").await;

    for name in [
        "sim-probe-authcode.com",
        "google.com",
        "sim-probe-authcode-own.com",
    ] {
        let without = check(&emulator, name).await;
        for code in ["", "A1b2!c3d4", &"x".repeat(1024)] {
            let (status, with) = emulator
                .call(
                    "CheckDomainTransferability",
                    &json!({ "DomainName": name, "AuthCode": code }),
                )
                .await;
            assert_eq!(
                (status, &with),
                (200, &without),
                "case {name:?} with a {} character auth code: the answer must not depend on the \
                 code, expected {without}, got {status} {with}",
                code.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The rule: a name the registry cannot answer for is refused, not answered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_under_a_suffix_no_registry_delegates_is_refused() {
    // The registry can only answer for a name under a TLD it delegates, and the
    // message's one variable part is that TLD. Rows differ along the suffix: an
    // unassigned label, two suffixes RFC 2606 reserves out of the DNS, and the same
    // request in another case.
    let cases = [
        ("sim-probe.doesnotexisttld", "doesnotexisttld"),
        ("sim-probe.invalid", "invalid"),
        ("sim-probe.test", "test"),
        ("SIM-PROBE.DoesNotExistTld", "doesnotexisttld"),
    ];
    let emulator = Emulator::new();

    for (name, tld) in cases {
        let (status, body) = emulator
            .call("CheckDomainTransferability", &json!({ "DomainName": name }))
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
}

#[tokio::test]
async fn a_blank_or_malformed_domain_name_is_refused_as_invalid_input() {
    // Rows differ along what is wrong with the name: no name at all (refused in its own
    // words), a single label with no TLD, and labels with a leading or trailing hyphen
    // or a character the member does not allow.
    let emulator = Emulator::new();

    for case in ["", " ", "   "] {
        let (status, body) = emulator
            .call("CheckDomainTransferability", &json!({ "DomainName": case }))
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

    for case in [
        "sim-probe",
        "-sim-probe.com",
        "sim-probe-.com",
        "sim probe.com",
    ] {
        let (status, body) = emulator
            .call("CheckDomainTransferability", &json!({ "DomainName": case }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!(
                    "Value '{case}' at 'domainName' failed to satisfy constraint: Member must be \
                     a domain name of one label under a top-level domain, where every label is \
                     made up of letters, digits and interior hyphens"
                )
                .as_str()
            ),
            "case {case:?}: the refusal must name the member and the rule, got {body}"
        );
    }
}
