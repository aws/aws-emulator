// SPDX-License-Identifier: CC0-1.0

//! The FORM of `CheckDNSAvailability$FullyQualifiedCNAME`, over the service's
//! awsQuery HTTP surface.
//!
//! The rule under test: the member is a fully qualified DOMAIN NAME — a
//! dot-separated name whose first label is exactly the requested `CNAMEPrefix`,
//! whose second label is the Region the request was signed for, followed by the
//! service's own domain (itself at least two labels, so it carries a dot), with
//! no `:` and no `/` anywhere and within the member's modelled length
//! (`DNSCname` @length 1..255, from
//! `smithy_query(operation="CheckDNSAvailability")`). The member exists so a
//! client can predict the host name a future environment answers on, split the
//! first dot-label back off to recover the prefix, or write the value into a DNS
//! record — none of which a host literal with a port or a URL path supports.
//!
//! `dns_availability.rs` holds the `Available` rule and spells the whole name
//! out; this suite holds the name apart into its parts and checks the form that
//! every row must share. The member states the name the prefix WOULD reserve, so
//! it is reported exactly for a prefix that can be had: a prefix another
//! environment holds, or one no host name label can carry, reserves nothing and
//! reports no name — the rows below cover both. The service domain is not written
//! here: it is the serving target's own, so what is asserted is that it is a
//! domain and that every row agrees on it.
//!
//! The HTTP harness is the shared one, so this suite adds no second copy of it.

use crate::lifecycle::{Emulator, SOLUTION_STACK, TEST_REGION};

/// The modelled length bounds of the `DNSCname` shape the member targets, from
/// `smithy_query(operation="CheckDNSAvailability")` -> output
/// `{FullyQualifiedCNAME: DNSCname @length 1..255}`.
const DNS_CNAME_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// One `CheckDNSAvailability` call whose answer must carry the form.
struct FormCase<'a> {
    name: &'static str,
    /// The `CNAMEPrefix` sent, which must come back as the FIRST label.
    prefix: &'a str,
    /// The Region the call is signed for, which must come back as the SECOND label.
    region: &'static str,
}

/// The parts `value` splits into under the rule: the first label, the second
/// label and the domain behind them.
struct Parts<'a> {
    first_label: &'a str,
    second_label: &'a str,
    domain: &'a str,
}

/// Hold `value` to the form and hand back its parts, or fail naming the case and
/// the actual value.
fn parts<'a>(value: &'a str, case: &str) -> Parts<'a> {
    assert!(
        !value.contains(':') && !value.contains('/'),
        "case {case:?}: a DNS name carries neither ':' nor '/', got {value:?}"
    );
    assert!(
        DNS_CNAME_LENGTH.contains(&value.len()),
        "case {case:?}: expected a length within the modelled {DNS_CNAME_LENGTH:?}, got {} in {value:?}",
        value.len()
    );
    assert!(
        !value.starts_with('.') && !value.ends_with('.'),
        "case {case:?}: expected no empty leading or trailing label, got {value:?}"
    );
    // Three parts, so the domain behind the two leading labels comes back whole
    // rather than being reassembled from them.
    let mut labels = value.splitn(3, '.');
    let (first_label, second_label, domain) = match (labels.next(), labels.next(), labels.next()) {
        (Some(first_label), Some(second_label), Some(domain)) => (first_label, second_label, domain),
        _ => panic!(
            "case {case:?}: expected a prefix label, a region label and a domain behind them, \
             got {value:?}"
        ),
    };
    assert!(
        domain.contains('.'),
        "case {case:?}: expected the trailing domain to carry at least one dot, got {domain:?} in {value:?}"
    );
    assert!(
        domain.split('.').all(|label| !label.is_empty()),
        "case {case:?}: expected the trailing domain to be non-empty labels, got {domain:?} in {value:?}"
    );
    Parts {
        first_label,
        second_label,
        domain,
    }
}

#[tokio::test]
async fn fully_qualified_cname_is_a_dns_name_built_from_the_prefix_and_the_region() {
    let emulator = Emulator::new();

    // One live environment, so the rows below can ask about a prefix that is
    // HELD as well as free ones.
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "cname-form")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    let held = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "cname-form"),
                ("EnvironmentName", "form-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", "form-held"),
            ],
        )
        .await;
    assert_eq!(
        held.status, 200,
        "case \"setup\": expected 200 from CreateEnvironment, got {} and body {}",
        held.status, held.body
    );

    // A prefix exactly at the length trait's maximum (63), bound here so the
    // table below stays literal.
    let max_length_prefix = "z".repeat(63);
    // Rows differ along everything the rule generalises over: the length of the
    // prefix (both published bounds of `DNSCnamePrefix` @length 4..63), its
    // character classes and case, the Region asked about, and whether the name
    // is free, held, or one no environment could hold at all.
    let cases = [
        FormCase {
            name: "free prefix",
            prefix: "form-free",
            region: TEST_REGION,
        },
        FormCase {
            name: "shortest legal prefix",
            prefix: "abcd",
            region: TEST_REGION,
        },
        FormCase {
            name: "longest legal prefix",
            prefix: max_length_prefix.as_str(),
            region: TEST_REGION,
        },
        FormCase {
            name: "digits only",
            prefix: "12345",
            region: TEST_REGION,
        },
        FormCase {
            name: "mixed case is preserved in the first label",
            prefix: "Form-Mixed",
            region: TEST_REGION,
        },
        FormCase {
            name: "another region",
            prefix: "form-free",
            region: "eu-central-1",
        },
        FormCase {
            name: "a region whose name has three parts",
            prefix: "form-free",
            region: "ap-southeast-2",
        },
    ];

    let mut domains: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let answer = emulator
            .call_in_region(
                case.region,
                "CheckDNSAvailability",
                &[("CNAMEPrefix", case.prefix)],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200 from CheckDNSAvailability, got {} and body {}",
            case.name, answer.status, answer.body
        );
        let value = answer.expect("FullyQualifiedCNAME", case.name).to_string();
        let split = parts(&value, case.name);
        assert_eq!(
            split.first_label, case.prefix,
            "case {:?}: expected the first label to be the requested prefix {:?}, got {:?} in {value:?}",
            case.name, case.prefix, split.first_label
        );
        assert_eq!(
            split.second_label, case.region,
            "case {:?}: expected the second label to be the request's region {:?}, got {:?} in {value:?}",
            case.name, case.region, split.second_label
        );
        domains.push((case.name, split.domain.to_string()));
    }

    // Every row's name sits in the SAME domain: the serving target's own, which
    // is neither per-request nor per-prefix.
    let (first_name, first_domain) = &domains[0];
    for (name, domain) in &domains[1..] {
        assert_eq!(
            domain, first_domain,
            "case {name:?}: expected the same service domain as case {first_name:?} ({first_domain:?}), got {domain:?}"
        );
    }

    // The other side of the rule: a prefix that cannot be had reserves no name,
    // so none is reported — one another environment holds, and one no host name
    // label can carry.
    for (name, prefix) in [
        ("prefix held by a live environment", "form-held"),
        ("a prefix no environment could hold", "form_bad"),
    ] {
        let answer = emulator
            .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
            .await;
        assert_eq!(
            answer.expect("Available", name),
            "false",
            "case {name:?}: expected the prefix to report unavailable, got {}",
            answer.body
        );
        assert!(
            answer.value("FullyQualifiedCNAME").is_none(),
            "case {name:?}: a prefix that is not available reserves no name, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_reported_name_does_not_change_between_calls() {
    let emulator = Emulator::new();
    let case = "stability";
    let prefix = "form-stable";

    // The name a prefix forms is a function of the prefix and the Region, so two
    // reads answer the same string — in particular it is not derived from a
    // socket a call may have opened in between.
    let first = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
        .await;
    let first_value = first.expect("FullyQualifiedCNAME", case).to_string();
    let before = parts(&first_value, case);
    assert_eq!(
        (before.first_label, before.second_label),
        (prefix, TEST_REGION),
        "case {case:?}: expected the prefix and the region as the first two labels, got {first_value:?}"
    );

    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "cname-form")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "cname-form"),
                ("EnvironmentName", "stable-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                // ANOTHER prefix, so what the second read below reports is the
                // name of a prefix that is still free: the launch in between is
                // what could have changed it.
                ("CNAMEPrefix", "form-other"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        created.status, created.body
    );

    let second = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
        .await;
    assert_eq!(
        second.expect("Available", case),
        "true",
        "case {case:?}: no environment holds this prefix, got {}",
        second.body
    );
    let second_value = second.expect("FullyQualifiedCNAME", case).to_string();
    assert_eq!(
        second_value, first_value,
        "case {case:?}: the name a prefix forms must not change, got {}",
        second.body
    );
    // The prefix the launch DID take reserves nothing for this caller any more,
    // so no name comes back for it.
    let taken = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", "form-other")])
        .await;
    assert_eq!(
        taken.expect("Available", case),
        "false",
        "case {case:?}: the created environment holds the name it took, got {}",
        taken.body
    );
    assert!(
        taken.value("FullyQualifiedCNAME").is_none(),
        "case {case:?}: a prefix that is not available reserves no name, got {}",
        taken.body
    );
    // Held to the form on its own as well, so a regression that made BOTH reads
    // equally wrong fails here rather than passing the equality above.
    let after = parts(&second_value, case);
    assert_eq!(
        (after.first_label, after.second_label),
        (prefix, TEST_REGION),
        "case {case:?}: expected the prefix and the region as the first two labels, got {second_value:?}"
    );
    assert_eq!(
        after.domain, before.domain,
        "case {case:?}: expected the same service domain as before the create, got {second_value:?}"
    );
}
