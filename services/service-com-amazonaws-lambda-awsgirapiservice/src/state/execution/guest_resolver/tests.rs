// SPDX-License-Identifier: CC0-1.0

//! The unit tests of the environment's name service: which addresses a name resolves to, and
//! which names resolve to nothing at all.

use ::std::net::IpAddr;

use super::super::vpc_network::tests::two_subnet_attachment;
use super::{addresses_of, resolution_of};
use ::service_common::vpc_dns_firewall::{FirewallDecision, VpcQueryFilter, VpcQueryFilters};

/// The addresses `rendered` spells, as the rows below state what a name must resolve to.
fn parsed(rendered: &[&str]) -> Vec<IpAddr> {
    rendered
        .iter()
        .map(|address| address.parse::<IpAddr>().expect("a test address"))
        .collect()
}

/// RULE: a name resolves to the addresses the zones this resolver can name an address in hold
/// for it — this machine's own names, the private zone of the VPC the function is attached to,
/// and the public names the DNS standards fix — and a name none of them holds an address under
/// resolves to nothing at all.
///
/// The rows span what the rule generalises over: the FORM the value takes (an IPv4 literal, an
/// IPv6 literal, a bracketed one, a name spelled with the root and in capitals), the ZONE the
/// name belongs to (this machine, the `localhost` domain, the VPC's internal zone, a
/// documentation domain), and the ways a name comes to resolve to nothing (a host of the
/// internal zone outside the VPC, a label a documentation domain publishes nothing for, a name
/// under each TLD reserved to resolve nowhere, a public name no zone here holds, and no name
/// at all).
#[test]
fn a_name_resolves_to_what_the_zones_this_resolver_holds_name() {
    let engine = ::engine::InMemoryEngine::new();
    let attachment = two_subnet_attachment(&engine);
    // The documentation names all answer the same pair: the documentation address of each
    // family ([`super::DOCUMENTATION_ADDRESS`], [`super::DOCUMENTATION_ADDRESS_V6`]).
    let documented: &[&str] = &["192.0.2.1", "2001:db8::1"];
    let rows: &[(&str, &str, &[&str])] = &[
        ("an IPv4 address", "198.51.100.7", &["198.51.100.7"]),
        ("an IPv6 address", "2001:db8::5", &["2001:db8::5"]),
        ("a bracketed IPv6 address", "[2001:db8::5]", &["2001:db8::5"]),
        // A value spelling the brackets or the root twice is malformed, and is not read as the
        // well-formed name it resembles.
        ("a doubly bracketed address", "[[2001:db8::5]]", &[]),
        ("a name spelling the root twice", "example.com..", &[]),
        ("a loopback address", "127.0.0.3", &["127.0.0.3"]),
        ("this machine's own name", "localhost", &["127.0.0.1"]),
        ("the same name in capitals", "LOCALHOST", &["127.0.0.1"]),
        // RFC 6761 §6.3: any name falling within `.localhost` resolves to loopback.
        ("a name under the localhost domain", "db.localhost", &["127.0.0.1"]),
        (
            "the Amazon-provided hostname of an address in the function's own subnet",
            "ip-10-7-1-37.us-west-2.compute.internal",
            &["10.7.1.37"],
        ),
        (
            "the hostname of an address elsewhere in the VPC",
            "ip-10-7-200-1.us-west-2.compute.internal",
            &["10.7.200.1"],
        ),
        (
            "the same hostname spelled with the root and in capitals",
            "IP-10-7-1-37.US-WEST-2.COMPUTE.INTERNAL.",
            &["10.7.1.37"],
        ),
        (
            "the hostname of an address outside the VPC",
            "ip-10-9-0-1.us-west-2.compute.internal",
            &[],
        ),
        (
            "a name of the internal zone of no hostname form",
            "database.us-west-2.compute.internal",
            &[],
        ),
        ("a documentation domain", "example.com", documented),
        ("another documentation domain", "example.net", documented),
        ("the third documentation domain", "example.org", documented),
        (
            "the host label a documentation domain publishes",
            "www.example.org",
            documented,
        ),
        (
            "a documentation domain spelled with the root and in capitals",
            "EXAMPLE.COM.",
            documented,
        ),
        (
            "a label a documentation domain publishes nothing for",
            "broker.example.com",
            &[],
        ),
        // RFC 2606 §2 reserves these TLDs, which are delegated in no root zone.
        ("a name under the reserved invalid TLD", "nothing-here.invalid", &[]),
        ("a name under the reserved test TLD", "db.test", &[]),
        ("a name under the reserved example TLD", "host.example", &[]),
        ("a public name no zone here holds", "aws.amazon.com", &[]),
        (
            "a host of AWS's own naming for a data plane",
            "cluster-x.cluster-abc.us-east-1.docdb.amazonaws.com",
            &[],
        ),
        ("no name at all", "", &[]),
        ("a name of nothing but space", "   ", &[]),
    ];
    for (case, host, expected) in rows {
        let resolved = addresses_of(Some(&attachment), host);
        let expected = parsed(expected);
        assert_eq!(
            resolved, expected,
            "case {case:?}: expected {host:?} to resolve to {expected:?}, got {resolved:?}"
        );
    }
}

/// RULE: the private zone is the zone of the VPC the function is ATTACHED to, so a function
/// attached to none resolves no name of it — while every other zone this resolver holds
/// answers the same for both.
#[test]
fn a_function_attached_to_no_vpc_resolves_no_name_of_a_private_zone() {
    let engine = ::engine::InMemoryEngine::new();
    let attachment = two_subnet_attachment(&engine);
    // (case, the name, whether a function ATTACHED to the VPC resolves it)
    let rows: &[(&str, &str, bool)] = &[
        (
            "the hostname of an address in the VPC",
            "ip-10-7-1-37.us-west-2.compute.internal",
            true,
        ),
        ("this machine's own name", "localhost", true),
        ("a documentation domain", "example.com", true),
        ("a name nothing holds", "nothing-here.invalid", false),
    ];
    for (case, host, attached_resolves) in rows {
        let inside = addresses_of(Some(&attachment), host);
        assert_eq!(
            !inside.is_empty(),
            *attached_resolves,
            "case {case:?}: expected the attached function's answer for {host:?} to be \
             {attached_resolves}, got {inside:?}"
        );
        let outside = addresses_of(None, host);
        let expected: Vec<IpAddr> = if host.ends_with(".internal") {
            Vec::new()
        } else {
            inside.clone()
        };
        assert_eq!(
            outside, expected,
            "case {case:?}: expected a function attached to no VPC to resolve {host:?} to \
             {expected:?}, got {outside:?}"
        );
    }
}

/// A filter that answers each name of `substituted` with a record of its own standing for the
/// name beside it, for the rows below: the stand-in for the service that owns DNS Firewall,
/// which records its own filter in this register when it is mounted on the engine.
///
/// A table rather than one name, because what the rows are about is a CHAIN: the name a record
/// substitutes may itself be one the firewall answers with another record.
struct SubstitutedNames {
    /// Each name the filter answers, and the name its record stands for.
    substituted: Vec<(&'static str, &'static str)>,
}

impl VpcQueryFilter for SubstitutedNames {
    fn decisions(
        &self,
        _vpc_id: &str,
        asked: &[(String, Option<u16>)],
    ) -> Vec<Option<FirewallDecision>> {
        asked
            .iter()
            .map(|(name, _qtype)| {
                let asked = name.trim_end_matches('.').to_ascii_lowercase();
                self.substituted
                    .iter()
                    .find(|(held, _)| *held == asked)
                    .map(|(_, stands_for)| FirewallDecision::BlockOverride {
                        domain: (*stands_for).to_string(),
                        dns_type: "CNAME".to_string(),
                        ttl: 30,
                    })
            })
            .collect()
    }
}

/// RULE: an ALIAS is followed — a name whose answer stands for another name resolves to the
/// addresses of the name the chain ends at, and a lookup asking for the whole answer reports
/// that name as the canonical one with the names it passed through as the aliases.
///
/// The rows span what the rule generalises over: how many names the chain passes through (none,
/// one, two), where it ends (a name an address is held under, a name nothing holds), and the
/// chain that never ends — a pair of names standing for each other, which resolves to nothing
/// rather than being followed for ever.
#[test]
fn an_alias_resolves_to_the_name_it_stands_for() {
    // (case, the names the firewall substitutes, the name asked about, the canonical name, the
    // aliases, whether an address is held under the canonical name)
    type Row = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
        &'static str,
        &'static [&'static str],
        bool,
    );
    let rows: &[Row] = &[
        (
            "a name no record stands in for",
            &[],
            "example.net",
            "example.net",
            &[],
            true,
        ),
        (
            "a name one record stands in for",
            &[("blocked.example.com", "example.net")],
            "blocked.example.com",
            "example.net",
            &["blocked.example.com"],
            true,
        ),
        (
            "the same name spelled with the root and in capitals",
            &[("blocked.example.com", "example.net")],
            "BLOCKED.EXAMPLE.COM.",
            "example.net",
            &["blocked.example.com"],
            true,
        ),
        (
            "a chain of two records",
            &[
                ("first.example.com", "second.example.com"),
                ("second.example.com", "example.net"),
            ],
            "first.example.com",
            "example.net",
            &["first.example.com", "second.example.com"],
            true,
        ),
        (
            "a chain ending at a name nothing holds an address under",
            &[("dangling.example.com", "nowhere.invalid")],
            "dangling.example.com",
            "",
            &[],
            false,
        ),
        (
            "a chain that never ends",
            &[
                ("loop-a.example.com", "loop-b.example.com"),
                ("loop-b.example.com", "loop-a.example.com"),
            ],
            "loop-a.example.com",
            "",
            &[],
            false,
        ),
    ];
    for (case, substituted, host, canonical, aliases, holds_address) in rows {
        let engine = ::engine::InMemoryEngine::new();
        if !substituted.is_empty() {
            VpcQueryFilters::of(&engine)
                .expect("the engine vends the register")
                .registering(::std::sync::Arc::new(SubstitutedNames {
                    substituted: substituted.to_vec(),
                }));
        }
        // The attachment is read AFTER the filter is recorded, as an invocation's seam is.
        let attachment = two_subnet_attachment(&engine);
        let resolved = resolution_of(Some(&attachment), host);
        assert_eq!(
            resolved.canonical, *canonical,
            "case {case:?}: expected {host:?} to resolve to the canonical name {canonical:?}, \
             got {resolved:?}"
        );
        assert_eq!(
            resolved.aliases, *aliases,
            "case {case:?}: expected the aliases {aliases:?} for {host:?}, got {resolved:?}"
        );
        assert_eq!(
            !resolved.addresses.is_empty(),
            *holds_address,
            "case {case:?}: expected an address for {host:?} to be {holds_address}, got \
             {resolved:?}"
        );
        // The two readings of one answer agree: the addresses a lookup that asks for them alone
        // is given are this resolution's own.
        let addresses = addresses_of(Some(&attachment), host);
        assert_eq!(
            addresses, resolved.addresses,
            "case {case:?}: expected both readings of {host:?} to hold the same addresses, got \
             {addresses:?} and {resolved:?}"
        );
    }
}
