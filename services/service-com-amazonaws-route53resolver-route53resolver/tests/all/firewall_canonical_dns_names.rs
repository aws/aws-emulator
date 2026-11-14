// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE spelling a DNS Firewall resource holds a DNS
//! name in.
//!
//! The rule under test: a DNS name a DNS Firewall resource is given is stored
//! canonical — rooted with exactly one trailing dot and case folded — and every
//! reader projects that one spelling, whatever spelling the request wrote. DNS
//! labels are compared without regard to case (RFC 4343 §1) and a name is
//! written relative to the root, which is written as a trailing dot (RFC 1035
//! §3.1), so `Foo.test` and `foo.test.` are ONE name: they are one entry of a
//! domain list rather than two, and one override target rather than two. The
//! reduction is idempotent, so writing the canonical spelling again changes
//! nothing.
//!
//! Neither `FirewallDomainName` nor `BlockOverrideDomain` carries a `pattern` or
//! a `format` in the model
//! (`smithy_query(query="shape",shape="FirewallDomainName,BlockOverrideDomain")`),
//! so the canonical form is recorded behavior.
//!
//! The tables span what the rule generalises over: the case of the name written
//! (lower, upper, mixed), whether it arrived rooted, the kind of labels it
//! carries (ASCII, a punycode/ACE label case folding leaves alone, an
//! underscored label, a single label), whether it is a wildcard, and the
//! operation that wrote it.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{emulator, ok, string_member, Emulator};

/// One row: the spelling a request writes, and the spelling every reader
/// projects it in.
struct NameRow {
    case: &'static str,
    written: &'static str,
    canonical: &'static str,
}

/// The spellings a domain list is written with, and the one spelling it holds
/// each of them in.
const DOMAIN_ROWS: &[NameRow] = &[
    NameRow {
        case: "an unrooted name in lower case",
        written: "blocked.test",
        canonical: "blocked.test.",
    },
    NameRow {
        case: "a rooted name in mixed case",
        written: "ExAmple-MiXeDcAsE.TEST.",
        canonical: "example-mixedcase.test.",
    },
    NameRow {
        case: "an unrooted name in upper case",
        written: "CASED.EXAMPLE.COM",
        canonical: "cased.example.com.",
    },
    NameRow {
        case: "a rooted punycode name, which case folding leaves alone",
        written: "xn--e1afmkfd.xn--80akhbyknj4f.",
        canonical: "xn--e1afmkfd.xn--80akhbyknj4f.",
    },
    NameRow {
        case: "a punycode wildcard",
        written: "*.xn--mnchen-3ya.example",
        canonical: "*.xn--mnchen-3ya.example.",
    },
    NameRow {
        case: "a wildcard in mixed case",
        written: "*.Wild.Example.COM",
        canonical: "*.wild.example.com.",
    },
    NameRow {
        case: "an underscored label in mixed case",
        written: "_DMARC.Example.COM",
        canonical: "_dmarc.example.com.",
    },
    NameRow {
        case: "a single label in upper case",
        written: "LOCALHOST",
        canonical: "localhost.",
    },
];

/// The spellings a rule's block override target is written with, and the one
/// spelling the rule reports it in.
const OVERRIDE_ROWS: &[NameRow] = &[
    NameRow {
        case: "an unrooted override target",
        written: "override.example.com",
        canonical: "override.example.com.",
    },
    NameRow {
        case: "an override target already rooted",
        written: "override.example.com.",
        canonical: "override.example.com.",
    },
    NameRow {
        case: "an override target in mixed case",
        written: "OverRide.Example.COM",
        canonical: "override.example.com.",
    },
    NameRow {
        case: "a punycode override target",
        written: "XN--E1AFMKFD.Example",
        canonical: "xn--e1afmkfd.example.",
    },
    NameRow {
        case: "a single-label override target",
        written: "SinkHole",
        canonical: "sinkhole.",
    },
];

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &Emulator, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": format!("group-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallRuleGroup")
            .expect("the create answers with the FirewallRuleGroup"),
        "Id",
    )
}

/// Create one domain list and answer the id Route 53 Resolver minted for it.
async fn domain_list(router: &Emulator, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    )
}

/// Write `domains` to the list through `operation`.
async fn write_domains(router: &Emulator, id: &str, operation: &str, domains: &[&str]) {
    ok(
        router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": id,
            "Operation": operation,
            "Domains": domains,
        }),
    )
    .await;
}

/// Every domain the list holds, in the order it reports them.
async fn listed_domains(router: &Emulator, id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id, "MaxResults": 100 }),
    )
    .await;
    listed
        .get("Domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the response carries a Domains array, got {listed}"))
        .iter()
        .map(|domain| {
            domain
                .as_str()
                .unwrap_or_else(|| panic!("a domain is a string, got {domain}"))
                .to_string()
        })
        .collect()
}

/// The `DomainCount` the list reports.
async fn domain_count(router: &Emulator, id: &str) -> Option<i64> {
    ok(
        router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": id }),
    )
    .await
    .get("FirewallDomainList")
    .and_then(|list| list.get("DomainCount"))
    .and_then(Value::as_i64)
}

/// Whatever spelling a write uses, the list holds and reports the canonical
/// name, through both operations that write one — and the count follows the
/// entries, so a second spelling of one name adds nothing.
#[tokio::test]
async fn a_domain_list_holds_every_name_in_its_canonical_spelling() {
    let router = emulator();

    for (index, NameRow {
        case,
        written,
        canonical,
    }) in DOMAIN_ROWS.iter().enumerate()
    {
        for operation in ["ADD", "REPLACE"] {
            let case = format!("{case} written by {operation}");
            // One list per row and per operation, so what a case reads is the
            // name that case wrote.
            let list = domain_list(&router, &format!("canonical-{index}-{operation}")).await;
            write_domains(&router, &list, operation, &[written]).await;

            let domains = listed_domains(&router, &list).await;
            assert_eq!(
                domains,
                vec![(*canonical).to_string()],
                "case {case:?}: expected the canonical name {canonical:?}, got {domains:?}"
            );
            assert_eq!(
                domain_count(&router, &list).await,
                Some(1),
                "case {case:?}: the list holds exactly the one name it was written"
            );

            // The canonical spelling written again changes nothing: the
            // reduction is idempotent.
            write_domains(&router, &list, "ADD", &[canonical]).await;
            let again = listed_domains(&router, &list).await;
            assert_eq!(
                again,
                vec![(*canonical).to_string()],
                "case {case:?}: the canonical spelling adds no second entry, got {again:?}"
            );

            // So does the SAME name written in another case, and a removal
            // written in any of those spellings takes the one entry out.
            let shouted = written.to_uppercase();
            write_domains(&router, &list, "ADD", &[shouted.as_str()]).await;
            let shouted_back = listed_domains(&router, &list).await;
            assert_eq!(
                shouted_back,
                vec![(*canonical).to_string()],
                "case {case:?}: another case of a stored name adds no second entry, got \
                 {shouted_back:?}"
            );
            write_domains(&router, &list, "REMOVE", &[shouted.as_str()]).await;
            let emptied = listed_domains(&router, &list).await;
            assert!(
                emptied.is_empty(),
                "case {case:?}: a removal in any case must take the entry out, got {emptied:?}"
            );
            assert_eq!(
                domain_count(&router, &list).await,
                Some(0),
                "case {case:?}: the count follows the entries the list holds"
            );
        }
    }
}

/// Two spellings of one name written in ONE request are one entry, and the
/// names of a blocklist read back exactly as a caller that stores the canonical
/// form expects them.
#[tokio::test]
async fn two_spellings_of_one_name_written_together_are_one_entry() {
    let router = emulator();
    let list = domain_list(&router, "collapsed").await;

    write_domains(
        &router,
        &list,
        "ADD",
        &[
            "Foo.test",
            "foo.test.",
            "FOO.TEST",
            "bar.test",
            "BAR.test.",
            "*.Baz.test",
            "*.baz.test.",
        ],
    )
    .await;

    let domains = listed_domains(&router, &list).await;
    assert_eq!(
        domains,
        vec![
            "foo.test.".to_string(),
            "bar.test.".to_string(),
            "*.baz.test.".to_string(),
        ],
        "every spelling of one name must collapse to its canonical entry, got {domains:?}"
    );
    assert_eq!(
        domain_count(&router, &list).await,
        Some(3),
        "DomainCount must follow the entries the list holds, got {domains:?}"
    );
}

/// A rule reports its block override target in the canonical spelling, whichever
/// spelling created or updated it, and a later read reports the same one.
#[tokio::test]
async fn a_rule_reports_its_block_override_target_canonical() {
    let router = emulator();
    let group = rule_group(&router, "override-group").await;

    for (index, NameRow {
        case,
        written,
        canonical,
    }) in OVERRIDE_ROWS.iter().enumerate()
    {
        // One domain list per row, so each rule matches a source of its own and
        // is addressable by it.
        let list = domain_list(&router, &format!("override-{index}")).await;
        let created = ok(
            &router,
            "CreateFirewallRule",
            json!({
                "CreatorRequestId": format!("override-{index}"),
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Name": format!("override-{index}"),
                "Priority": 100 + i64::try_from(index).expect("a small index") * 10,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": written,
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            }),
        )
        .await;
        let rule = created
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the create answers the rule, got {created}"));
        assert_eq!(
            string_member(rule, "BlockOverrideDomain"),
            *canonical,
            "case {case:?}: the create must report the canonical override target, got {created}"
        );

        // A later read reports the same one, so what the rule HOLDS is the
        // canonical name and not something the response computed.
        let listed = ok(
            &router,
            "ListFirewallRules",
            json!({ "FirewallRuleGroupId": group }),
        )
        .await;
        let found = listed
            .get("FirewallRules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the listing carries FirewallRules"))
            .iter()
            .find(|rule| string_member(rule, "FirewallDomainListId") == list)
            .unwrap_or_else(|| panic!("case {case:?}: the rule must be listed, got {listed}"))
            .clone();
        assert_eq!(
            string_member(&found, "BlockOverrideDomain"),
            *canonical,
            "case {case:?}: a later read must report the canonical override target, got {found}"
        );

        // The update path reduces the name the same way: the row's name written
        // in upper case is the same override target, so the rule reports the
        // canonical spelling again.
        let updated = ok(
            &router,
            "UpdateFirewallRule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "BlockOverrideDomain": written.to_uppercase(),
            }),
        )
        .await;
        let rule = updated
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the update answers the rule, got {updated}"));
        assert_eq!(
            string_member(rule, "BlockOverrideDomain"),
            *canonical,
            "case {case:?}: the update must report the canonical override target, got {updated}"
        );
    }
}
