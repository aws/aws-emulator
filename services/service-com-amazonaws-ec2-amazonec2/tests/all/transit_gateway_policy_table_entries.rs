// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the transit gateway policy table ENTRY surface:
//! `CreateTransitGatewayPolicyTableEntry`, `GetTransitGatewayPolicyTableEntries`,
//! `ModifyTransitGatewayPolicyTableEntry` and `DeleteTransitGatewayPolicyTableEntry`, driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an entry of a transit gateway policy table is the record the request
//! describes — stored at the rule number it names, carrying the matching criteria it
//! describes and the route table it targets — so every later reader of that table reports it,
//! the entries come back in the order their rule NUMBERS are evaluated in, a modify lays the
//! request over the entry field by field, a delete takes it out, and a request naming a value
//! outside a published catalog, or a resource the account does not hold, is refused while
//! nothing is written.
//!
//! Every table row below is a distinct input along what that generalises over: which matching
//! criteria a rule names, which protocol of the published catalog it matches on, whether it
//! names ports and in which form, which address family its CIDR blocks are, which rule number
//! the entry takes, how many entries one table holds, how large a page is asked for, which
//! attribute a filter selects on, and which field a modify names. Nothing here is seeded
//! through the store: the transit gateway, its route tables, the policy table and every entry
//! are created through the operations themselves.

use ec2::state::{
    MIN_TRANSIT_GATEWAY_POLICY_TABLE_RULE_NUMBER, TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS,
    TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE,
    TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_DELETED, TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, emulator, error_code, filters, indexed};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The HTTP status EC2 answers a refused request with.
const REFUSED_STATUS: u16 = 400;

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// The transit gateway, two route tables and the policy table every test here runs against,
/// each minted by the operation that mints one rather than by reaching into the store.
struct Fixtures {
    router: MultiServiceRouter,
    transit_gateway_id: String,
    policy_table_id: String,
    route_table_id: String,
    other_route_table_id: String,
}

/// A fresh emulator holding one transit gateway, two of its route tables and one policy
/// table of it.
async fn fixtures() -> Fixtures {
    let (router, _engine) = emulator();

    let (status, body) = invoke(&router, "CreateTransitGateway", &[]).await;
    assert_eq!(status, 200, "the transit gateway must be created: {body}");
    let transit_gateway_id =
        element(&body, "transitGatewayId").expect("the created transit gateway's id");

    let mut route_tables = Vec::new();
    for _ in 0..2 {
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayRouteTable",
            &[param("TransitGatewayId", &transit_gateway_id)],
        )
        .await;
        assert_eq!(status, 200, "the route table must be created: {body}");
        route_tables.push(
            element(&body, "transitGatewayRouteTableId").expect("the created route table's id"),
        );
    }

    let (status, body) = invoke(
        &router,
        "CreateTransitGatewayPolicyTable",
        &[param("TransitGatewayId", &transit_gateway_id)],
    )
    .await;
    assert_eq!(status, 200, "the policy table must be created: {body}");
    let policy_table_id =
        element(&body, "transitGatewayPolicyTableId").expect("the created policy table's id");

    Fixtures {
        router,
        transit_gateway_id,
        policy_table_id,
        route_table_id: route_tables[0].clone(),
        other_route_table_id: route_tables[1].clone(),
    }
}

/// The request members one entry create carries: the table, the rule number, the route table
/// it targets and the `PolicyRule.*` members of `rule`.
fn entry_params(
    policy_table_id: &str,
    rule_number: &str,
    target_route_table_id: &str,
    rule: &[(&str, &str)],
) -> Vec<Param> {
    let mut params = vec![
        param("TransitGatewayPolicyTableId", policy_table_id),
        param("PolicyRuleNumber", rule_number),
        param("TargetRouteTableId", target_route_table_id),
    ];
    params.extend(rule_members(rule));
    params
}

/// The `PolicyRule.<member>` form fields for one requested rule — the nested structure
/// ec2Query flattens onto its parent's name and a dot.
fn rule_members(rule: &[(&str, &str)]) -> Vec<Param> {
    rule.iter()
        .map(|(member, value)| param(&format!("PolicyRule.{member}"), value))
        .collect()
}

/// The text of the member `path` of the one entry a response carries, where a `/` walks into
/// a nested structure of the entry (`policyRule/metaData/metaDataKey`).
fn entry_member(case: &str, body: &str, path: &str) -> Option<String> {
    let parsed = document(case, body);
    let entry = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "policyRuleNumber").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry one entry: {body}"));
    let mut node = entry;
    let mut names = path.split('/').peekable();
    while let Some(name) = names.next() {
        if names.peek().is_none() {
            return child_text(node, name);
        }
        node = child(node, name)?;
    }
    None
}

/// Whether the one entry a response carries holds the member `path` at all.
fn entry_has_member(case: &str, body: &str, path: &str) -> bool {
    entry_member(case, body, path).is_some()
}

/// The entries a `GetTransitGatewayPolicyTableEntries` response reports, as their rule
/// numbers in the order the report lists them.
fn reported_rule_numbers(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let set = child(root, "transitGatewayPolicyTableEntries")
        .unwrap_or_else(|| panic!("case {case:?}: the report must carry an entry set: {body}"));
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| child_text(item, "policyRuleNumber").unwrap_or_default())
        .collect()
}

/// Every entry of the policy table, as the report lists them.
async fn report(router: &MultiServiceRouter, policy_table_id: &str) -> (u16, String) {
    invoke(
        router,
        "GetTransitGatewayPolicyTableEntries",
        &[param("TransitGatewayPolicyTableId", policy_table_id)],
    )
    .await
}

/// Creating a policy table stores a table of the transit gateway the request names, and the
/// describe of that table answers with it — so the create's answer is the record the store
/// holds rather than one the handler composed.
///
/// The policy table is the resource the entries below live in, and the operation this suite's
/// fixtures mint one with, so the round trip is pinned here.
#[tokio::test]
async fn a_created_policy_table_round_trips_through_the_describe() {
    let fixtures = fixtures().await;
    let (status, body) = invoke(
        &fixtures.router,
        "DescribeTransitGatewayPolicyTables",
        &indexed(
            "TransitGatewayPolicyTableIds",
            &[fixtures.policy_table_id.as_str()],
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "the describe must succeed, got {status} with {body}"
    );
    assert_eq!(
        elements(&body, "transitGatewayPolicyTableId"),
        vec![fixtures.policy_table_id.clone()],
        "expected exactly the created policy table, got {body}"
    );
    assert_eq!(
        element(&body, "transitGatewayId").as_deref(),
        Some(fixtures.transit_gateway_id.as_str()),
        "the table must belong to the transit gateway the create named, got {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE),
        "the table must be reported in the state the create recorded, got {body}"
    );
    assert!(
        element(&body, "creationTime").is_some(),
        "the table must carry the instant it was created, got {body}"
    );
}

/// One created entry: the rule its request describes, and the members the entry must then
/// report — with the value each of them must carry.
struct Created {
    /// What distinguishes this row from the others.
    case: &'static str,
    /// The rule number the entry is created at.
    rule_number: &'static str,
    /// The `PolicyRule.<member>` members the request carries.
    rule: &'static [(&'static str, &'static str)],
    /// The member paths of the entry that must come back, each with its value. A criterion
    /// of [`CRITERIA`] this does not name must come back as [`ANY`], the rule's own word for
    /// "matches anything".
    reported: &'static [(&'static str, &'static str)],
    /// The member paths that must NOT come back at all.
    absent: &'static [&'static str],
}

/// The value a criterion the request did not narrow is reported with: "`*` for all
/// protocols" (`TransitGatewayRequestPolicyRule$Protocol`), the word the rule's members
/// carry for "matches anything".
const ANY: &str = "*";

/// The five matching criteria every policy rule carries, as the entry reports them
/// (`smithy_query(query="shape",shape="TransitGatewayPolicyRule")`, whose members are
/// `SourceCidrBlock`, `SourcePortRange`, `DestinationCidrBlock`, `DestinationPortRange`,
/// `Protocol` and the `MetaData` pair — which is not a criterion but a key-value pair the
/// caller attaches, so it is reported only when a request names one).
const CRITERIA: [&str; 5] = [
    "sourceCidrBlock",
    "sourcePortRange",
    "destinationCidrBlock",
    "destinationPortRange",
    "protocol",
];

/// The rows every round-trip assertion runs over: one per kind of matching criteria a rule
/// can describe.
///
/// The protocols are the catalog the model publishes — "Valid values are 1 (ICMP), 6 (TCP),
/// 17 (UDP), 47 (GRE), or * for all protocols"
/// (`smithy_query(query="doc",shape="TransitGatewayRequestPolicyRule",member="Protocol")`) —
/// with GRE and ICMP among them, which the report of a single recorded call does not cover.
fn created_rows() -> Vec<Created> {
    vec![
        Created {
            case: "an entry whose request describes no rule at all",
            rule_number: "1",
            rule: &[],
            reported: &[],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "a source CIDR block on its own",
            rule_number: "2",
            rule: &[("SourceCidrBlock", "10.0.0.0/16")],
            reported: &[("policyRule/sourceCidrBlock", "10.0.0.0/16")],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "TCP with port ranges, both CIDR blocks and metadata",
            rule_number: "3",
            rule: &[
                ("Protocol", "6"),
                ("SourceCidrBlock", "10.0.0.0/8"),
                ("SourcePortRange", "1024-2048"),
                ("DestinationCidrBlock", "192.168.0.0/16"),
                ("DestinationPortRange", "443"),
                ("MetaData.MetaDataKey", "tenant"),
                ("MetaData.MetaDataValue", "blue"),
            ],
            reported: &[
                ("policyRule/protocol", "6"),
                ("policyRule/sourceCidrBlock", "10.0.0.0/8"),
                ("policyRule/sourcePortRange", "1024-2048"),
                ("policyRule/destinationCidrBlock", "192.168.0.0/16"),
                ("policyRule/destinationPortRange", "443"),
                ("policyRule/metaData/metaDataKey", "tenant"),
                ("policyRule/metaData/metaDataValue", "blue"),
            ],
            absent: &[],
        },
        Created {
            case: "UDP with a single port on each side",
            rule_number: "4",
            rule: &[
                ("Protocol", "17"),
                ("SourcePortRange", "53"),
                ("DestinationPortRange", "53"),
            ],
            reported: &[
                ("policyRule/protocol", "17"),
                ("policyRule/sourcePortRange", "53"),
                ("policyRule/destinationPortRange", "53"),
            ],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "GRE, which carries no ports",
            rule_number: "5",
            rule: &[("Protocol", "47"), ("SourcePortRange", "*")],
            reported: &[
                ("policyRule/protocol", "47"),
                ("policyRule/sourcePortRange", "*"),
            ],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "ICMP, which carries no ports either",
            rule_number: "6",
            rule: &[("Protocol", "1"), ("DestinationPortRange", "*")],
            reported: &[
                ("policyRule/protocol", "1"),
                ("policyRule/destinationPortRange", "*"),
            ],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "every protocol, matching every port",
            rule_number: "7",
            rule: &[
                ("Protocol", "*"),
                ("SourcePortRange", "*"),
                ("DestinationPortRange", "*"),
            ],
            reported: &[
                ("policyRule/protocol", "*"),
                ("policyRule/sourcePortRange", "*"),
                ("policyRule/destinationPortRange", "*"),
            ],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "IPv6 CIDR blocks",
            rule_number: "8",
            rule: &[
                ("SourceCidrBlock", "2001:db8::/32"),
                ("DestinationCidrBlock", "2001:db8:1::/48"),
            ],
            reported: &[
                ("policyRule/sourceCidrBlock", "2001:db8::/32"),
                ("policyRule/destinationCidrBlock", "2001:db8:1::/48"),
            ],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "a CIDR block that is not written as its own range",
            rule_number: "9",
            rule: &[("SourceCidrBlock", "10.0.5.7/16")],
            // "We modify the specified CIDR block to its canonical form", the form every
            // reader of this crate reports a block in.
            reported: &[("policyRule/sourceCidrBlock", "10.0.0.0/16")],
            absent: &["policyRule/metaData"],
        },
        Created {
            case: "metadata on its own, with no matching criterion",
            rule_number: "10",
            rule: &[("MetaData.MetaDataKey", "owner")],
            reported: &[("policyRule/metaData/metaDataKey", "owner")],
            absent: &["policyRule/metaData/metaDataValue"],
        },
        Created {
            case: "the smallest rule number an entry may take",
            rule_number: "1",
            rule: &[("Protocol", "6"), ("SourcePortRange", "0-65535")],
            reported: &[
                ("policyRule/protocol", "6"),
                ("policyRule/sourcePortRange", "0-65535"),
            ],
            absent: &["policyRule/metaData"],
        },
    ]
}

/// Creating an entry stores exactly the record the request describes, and the report of the
/// table answers with that same record — so the write is observable to a later request
/// rather than being invented by the create's own answer.
#[tokio::test]
async fn a_created_entry_round_trips_through_the_report_of_its_table() {
    for row in created_rows() {
        let case = row.case;
        // One table per row, so the report carries exactly the entry the row created.
        let fixtures = fixtures().await;
        let params = entry_params(
            &fixtures.policy_table_id,
            row.rule_number,
            &fixtures.route_table_id,
            row.rule,
        );
        let (status, created) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {created}"
        );

        let (status, reported) = report(&fixtures.router, &fixtures.policy_table_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {reported}"
        );
        assert_eq!(
            reported_rule_numbers(case, &reported),
            vec![row.rule_number.to_string()],
            "case {case:?}: expected exactly the created entry, got {reported}"
        );

        // Both answers are read the same way, so the create cannot report an entry the
        // store does not hold.
        for (body, source) in [(&created, "the create"), (&reported, "the report")] {
            assert_eq!(
                entry_member(case, body, "policyRuleNumber").as_deref(),
                Some(row.rule_number),
                "case {case:?}: {source} must carry the rule number the request named, got \
                 {body}"
            );
            assert_eq!(
                entry_member(case, body, "targetRouteTableId").as_deref(),
                Some(fixtures.route_table_id.as_str()),
                "case {case:?}: {source} must carry the route table the request named, got \
                 {body}"
            );
            assert_eq!(
                entry_member(case, body, "state").as_deref(),
                Some(TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE),
                "case {case:?}: {source} must report a created entry active, got {body}"
            );
            for (path, expected) in row.reported {
                assert_eq!(
                    entry_member(case, body, path).as_deref(),
                    Some(*expected),
                    "case {case:?}: {source} must carry {path} as {expected:?}, got {body}"
                );
            }
            // An entry always HAS matching criteria: each of the five is reported, narrowed
            // where the request narrowed it and matching anything where it did not.
            for criterion in CRITERIA {
                let path = format!("policyRule/{criterion}");
                let expected = row
                    .reported
                    .iter()
                    .find(|(reported, _)| *reported == path)
                    .map_or(ANY, |(_, value)| *value);
                assert_eq!(
                    entry_member(case, body, &path).as_deref(),
                    Some(expected),
                    "case {case:?}: {source} must carry {path} as {expected:?}, got {body}"
                );
            }
            for path in row.absent {
                assert!(
                    !entry_has_member(case, body, path),
                    "case {case:?}: {source} must carry no {path}, got {body}"
                );
            }
        }
    }
}

/// Every protocol the published catalog admits is accepted, and the entry reports the one it
/// was created with.
///
/// The catalog itself is the source
/// (`TransitGatewayRequestPolicyRule$Protocol`), reached through the crate's one copy of it,
/// so a protocol added to the model is covered here without this test being edited.
#[tokio::test]
async fn every_published_protocol_is_accepted() {
    for (index, protocol) in TRANSIT_GATEWAY_POLICY_RULE_PROTOCOLS.iter().enumerate() {
        let case = format!("protocol {protocol:?}");
        let fixtures = fixtures().await;
        let rule_number = (index + 1).to_string();
        let mut params = entry_params(
            &fixtures.policy_table_id,
            &rule_number,
            &fixtures.route_table_id,
            &[],
        );
        params.push(param("PolicyRule.Protocol", protocol));
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a protocol the model publishes must be accepted, got {status} \
             with {body}"
        );
        assert_eq!(
            entry_member(&case, &body, "policyRule/protocol").as_deref(),
            Some(*protocol),
            "case {case:?}: the entry must report the protocol it was created with, got {body}"
        );
    }
}

/// The rule numbers the seven entries of the paging and filtering tests are created at,
/// written out of order so the report's own order is what puts them in order.
const SEEDED_RULE_NUMBERS: [&str; 7] = ["10", "2", "100", "1", "65535", "7", "3"];

/// Those rule numbers as the report must list them: by the NUMBER each names, so 10 follows
/// 2 — which comparing them as text would not give.
const ORDERED_RULE_NUMBERS: [&str; 7] = ["1", "2", "3", "7", "10", "100", "65535"];

/// The page size the paging assertions ask for: the smallest the model admits
/// (`TransitGatewayMaxResults`, `range 5..1000`), so a shorter second page follows the first.
const PAGE: usize = 5;

/// Create the seven entries above, half of them matching TCP and targeting the second route
/// table, and answer the fixtures they were created in.
async fn seven_entries() -> Fixtures {
    let fixtures = fixtures().await;
    for (index, rule_number) in SEEDED_RULE_NUMBERS.iter().enumerate() {
        let tcp = index % 2 == 0;
        let target = if tcp {
            &fixtures.route_table_id
        } else {
            &fixtures.other_route_table_id
        };
        let rule: &[(&str, &str)] = if tcp {
            &[("Protocol", "6"), ("SourcePortRange", "443")]
        } else {
            &[("Protocol", "17"), ("MetaData.MetaDataKey", "tenant")]
        };
        let params = entry_params(&fixtures.policy_table_id, rule_number, target, rule);
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "entry {rule_number} must be created, got {status} with {body}"
        );
    }
    fixtures
}

/// The report lists a table's entries by the rule number each is evaluated in order of, and
/// pages over them with a token naming where the next page starts.
#[tokio::test]
async fn the_report_lists_the_entries_in_rule_number_order_and_pages_them() {
    let fixtures = seven_entries().await;
    let expected: Vec<String> = ORDERED_RULE_NUMBERS
        .iter()
        .map(|number| (*number).to_string())
        .collect();

    let (status, body) = report(&fixtures.router, &fixtures.policy_table_id).await;
    assert_eq!(
        status, 200,
        "the whole report must succeed, got {status} with {body}"
    );
    assert_eq!(
        reported_rule_numbers("every entry", &body),
        expected,
        "expected every entry in rule-number order, got {body}"
    );

    let mut first_page = vec![param(
        "TransitGatewayPolicyTableId",
        &fixtures.policy_table_id,
    )];
    first_page.push(param("MaxResults", &PAGE.to_string()));
    let (status, body) = invoke(
        &fixtures.router,
        "GetTransitGatewayPolicyTableEntries",
        &first_page,
    )
    .await;
    assert_eq!(
        status, 200,
        "the first page must succeed, got {status} with {body}"
    );
    assert_eq!(
        reported_rule_numbers("first page", &body),
        expected[..PAGE].to_vec(),
        "first page: expected the first {PAGE} entries in rule-number order, got {body}"
    );
    let next_token = element(&body, "nextToken").expect("a token naming the second page");

    let mut second_page = first_page.clone();
    second_page.push(param("NextToken", &next_token));
    let (status, body) = invoke(
        &fixtures.router,
        "GetTransitGatewayPolicyTableEntries",
        &second_page,
    )
    .await;
    assert_eq!(
        status, 200,
        "the second page must succeed, got {status} with {body}"
    );
    assert_eq!(
        reported_rule_numbers("second page", &body),
        expected[PAGE..].to_vec(),
        "second page: expected the remaining entries, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "second page: the last page carries no token, got {body}"
    );
}

/// One filtering case: what distinguishes it, the `Filters` entries it sends, and the rule
/// numbers the report must then list.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<String>);

/// Each filter selects on one attribute of an entry, and the ones that hold are joined with
/// an AND.
#[tokio::test]
async fn each_filter_selects_on_one_attribute_of_an_entry() {
    let fixtures = seven_entries().await;
    // The entries created at an even index match TCP and target the first route table; the
    // others match UDP, carry the metadata key and target the second.
    let tcp: Vec<String> = ["3", "10", "100", "65535"]
        .iter()
        .map(|number| (*number).to_string())
        .collect();
    let udp: Vec<String> = ["1", "2", "7"]
        .iter()
        .map(|number| (*number).to_string())
        .collect();
    let rows: [FilterCase<'_>; 7] = [
        (
            "the rule number of one entry",
            vec![("policy-rule-number", &["7"])],
            vec!["7".to_string()],
        ),
        (
            "the protocol every TCP entry matches",
            vec![("policy-rule.protocol", &["6"])],
            tcp.clone(),
        ),
        (
            "the port range only the TCP entries carry",
            vec![("policy-rule.source-port-range", &["443"])],
            tcp.clone(),
        ),
        (
            "the metadata key only the UDP entries carry",
            vec![("policy-rule.meta-data.meta-data-key", &["tenant"])],
            udp.clone(),
        ),
        (
            "the state every entry is in",
            vec![(
                "state",
                &[TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE],
            )],
            ORDERED_RULE_NUMBERS
                .iter()
                .map(|number| (*number).to_string())
                .collect(),
        ),
        (
            "two filters, joined with an AND",
            vec![
                ("policy-rule.protocol", &["6"]),
                ("policy-rule-number", &["3"]),
            ],
            vec!["3".to_string()],
        ),
        (
            "an attribute no entry carries that value for",
            vec![("policy-rule.destination-cidr-block", &["172.16.0.0/12"])],
            Vec::new(),
        ),
    ];
    for (case, entries, expected) in rows {
        let mut params = vec![param(
            "TransitGatewayPolicyTableId",
            &fixtures.policy_table_id,
        )];
        params.extend(filters(&entries));
        let (status, body) = invoke(
            &fixtures.router,
            "GetTransitGatewayPolicyTableEntries",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            reported_rule_numbers(case, &body),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }

    // The targets are read off the entries themselves, so a filter on the route table one
    // targets selects the entries that were pointed at it.
    let mut params = vec![param(
        "TransitGatewayPolicyTableId",
        &fixtures.policy_table_id,
    )];
    params.extend(filters(&[(
        "target-route-table-id",
        &[fixtures.other_route_table_id.as_str()],
    )]));
    let (status, body) = invoke(
        &fixtures.router,
        "GetTransitGatewayPolicyTableEntries",
        &params,
    )
    .await;
    assert_eq!(
        status, 200,
        "the route table filter must succeed, got {status} with {body}"
    );
    assert_eq!(
        reported_rule_numbers("the other route table", &body),
        udp,
        "expected the entries targeting the second route table, got {body}"
    );
}

/// One refused request: what it names, and the code EC2 answers it with.
struct Refusal {
    /// What distinguishes this row.
    case: &'static str,
    /// The rule number the request names.
    rule_number: &'static str,
    /// The `PolicyRule.*` members it carries.
    rule: &'static [(&'static str, &'static str)],
    /// The code the answer must carry.
    code: &'static str,
}

/// A request naming a value outside a published catalog, or a rule number that is no rule
/// number, is refused — and the table is left holding exactly the entry it already held, so
/// nothing a refused request named was written.
#[tokio::test]
async fn a_refused_create_writes_nothing() {
    let rows = [
        Refusal {
            case: "a protocol the published catalog does not admit",
            rule_number: "20",
            rule: &[("Protocol", "7")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a protocol named by its word rather than its number",
            rule_number: "20",
            rule: &[("Protocol", "udp")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a port range for a protocol that has no ports",
            rule_number: "20",
            rule: &[("Protocol", "1"), ("SourcePortRange", "443")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a port range for no protocol at all",
            rule_number: "20",
            rule: &[("DestinationPortRange", "443")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a port past the last port there is",
            rule_number: "20",
            rule: &[("Protocol", "6"), ("SourcePortRange", "65536")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a port range whose ends are the wrong way round",
            rule_number: "20",
            rule: &[("Protocol", "6"), ("SourcePortRange", "8080-80")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a source CIDR block that is not a CIDR block",
            rule_number: "20",
            rule: &[("SourceCidrBlock", "10.0.0.0")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a destination CIDR block whose prefix length is not one",
            rule_number: "20",
            rule: &[("DestinationCidrBlock", "10.0.0.0/33")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "the rule number one below the first there is",
            rule_number: "0",
            rule: &[],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a negative rule number",
            rule_number: "-1",
            rule: &[],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "a rule number that is not a number",
            rule_number: "first",
            rule: &[],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "no rule number at all",
            rule_number: "",
            rule: &[],
            code: "MissingParameter",
        },
        Refusal {
            case: "the rule number the table already holds an entry at",
            rule_number: "1",
            rule: &[],
            code: "TransitGatewayPolicyTableEntryAlreadyExists",
        },
    ];
    let fixtures = fixtures().await;
    // The one entry every row is asked against: the table must still hold exactly it.
    let seeded = entry_params(
        &fixtures.policy_table_id,
        "1",
        &fixtures.route_table_id,
        &[("Protocol", "6"), ("SourcePortRange", "443")],
    );
    let (status, body) = invoke(
        &fixtures.router,
        "CreateTransitGatewayPolicyTableEntry",
        &seeded,
    )
    .await;
    assert_eq!(status, 200, "the seeded entry must be created: {body}");

    for row in rows {
        let case = row.case;
        let params = entry_params(
            &fixtures.policy_table_id,
            row.rule_number,
            &fixtures.route_table_id,
            row.rule,
        );
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got {body}",
            row.code
        );

        let (status, body) = report(&fixtures.router, &fixtures.policy_table_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            reported_rule_numbers(case, &body),
            vec!["1".to_string()],
            "case {case:?}: a refused create must write nothing, got {body}"
        );
    }
}

/// An id naming a resource the account does not hold is refused with THAT resource's own
/// code, a malformed one before an unknown one — whichever operation of the entry surface
/// names it.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_that_resources_own_code() {
    let fixtures = fixtures().await;
    // Well-formed EC2 ids of their kind that no created resource has, and text that is not
    // an id of that kind at all.
    let rows: [(&str, &str, &str, &str); 6] = [
        (
            "a policy table id nothing holds",
            "tgw-ptb-0123456789abcdef0",
            &fixtures.route_table_id,
            "InvalidTransitGatewayPolicyTableId.NotFound",
        ),
        (
            "a policy table id of no kind",
            "not-a-policy-table",
            &fixtures.route_table_id,
            "InvalidTransitGatewayPolicyTableId.Malformed",
        ),
        (
            "no policy table id at all",
            "",
            &fixtures.route_table_id,
            "MissingParameter",
        ),
        (
            "a route table id nothing holds",
            &fixtures.policy_table_id,
            "tgw-rtb-0123456789abcdef0",
            "InvalidRouteTableID.NotFound",
        ),
        (
            "a route table id of no kind",
            &fixtures.policy_table_id,
            "not-a-route-table",
            "InvalidRouteTableId.Malformed",
        ),
        (
            "no route table id at all",
            &fixtures.policy_table_id,
            "",
            "MissingParameter",
        ),
    ];
    for (case, policy_table_id, route_table_id, code) in rows {
        let params = entry_params(policy_table_id, "1", route_table_id, &[]);
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got {body}"
        );
    }

    // The report, the modify and the delete resolve the policy table the same way, so an id
    // none of them can resolve is refused rather than read as "no entries".
    let unknown = "tgw-ptb-0123456789abcdef0";
    for action in [
        "GetTransitGatewayPolicyTableEntries",
        "ModifyTransitGatewayPolicyTableEntry",
        "DeleteTransitGatewayPolicyTableEntry",
    ] {
        let params = vec![
            param("TransitGatewayPolicyTableId", unknown),
            param("PolicyRuleNumber", "1"),
        ];
        let (status, body) = invoke(&fixtures.router, action, &params).await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {action:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidTransitGatewayPolicyTableId.NotFound",
            "case {action:?}: expected the policy table's own NotFound code, got {body}"
        );
    }

    // An entry the table does not hold is refused by the two operations that name one, with
    // the entry's own code rather than the table's.
    for action in [
        "ModifyTransitGatewayPolicyTableEntry",
        "DeleteTransitGatewayPolicyTableEntry",
    ] {
        let params = vec![
            param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
            param(
                "PolicyRuleNumber",
                &MIN_TRANSIT_GATEWAY_POLICY_TABLE_RULE_NUMBER.to_string(),
            ),
        ];
        let (status, body) = invoke(&fixtures.router, action, &params).await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {action:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidTransitGatewayPolicyTableEntry.NotFound",
            "case {action:?}: expected the entry's own NotFound code, got {body}"
        );
    }
}

/// One modification: the members it names, and what the entry must carry afterwards.
struct Modification {
    /// What distinguishes this row.
    case: &'static str,
    /// The `PolicyRule.*` members the modify request carries.
    rule: &'static [(&'static str, &'static str)],
    /// Whether the modify also names the second route table as the entry's target.
    retargets: bool,
    /// The member paths the entry must carry afterwards, each with its value.
    reported: &'static [(&'static str, &'static str)],
}

/// A modify lays the request over the entry FIELD BY FIELD — "unspecified fields retain
/// their current values" — and the report of the table answers with the entry as it now
/// stands.
#[tokio::test]
async fn a_modify_replaces_only_the_fields_the_request_names() {
    // The rule every row starts from: TCP, both CIDR blocks, both port ranges, and metadata.
    let created: &[(&str, &str)] = &[
        ("Protocol", "6"),
        ("SourceCidrBlock", "10.0.0.0/8"),
        ("SourcePortRange", "1024-2048"),
        ("DestinationCidrBlock", "192.168.0.0/16"),
        ("DestinationPortRange", "443"),
        ("MetaData.MetaDataKey", "tenant"),
        ("MetaData.MetaDataValue", "blue"),
    ];
    let rows = [
        Modification {
            case: "one field of the rule, leaving every other as it was",
            rule: &[("SourceCidrBlock", "172.16.0.0/12")],
            retargets: false,
            reported: &[
                ("policyRule/sourceCidrBlock", "172.16.0.0/12"),
                ("policyRule/protocol", "6"),
                ("policyRule/sourcePortRange", "1024-2048"),
                ("policyRule/destinationCidrBlock", "192.168.0.0/16"),
                ("policyRule/destinationPortRange", "443"),
                ("policyRule/metaData/metaDataKey", "tenant"),
                ("policyRule/metaData/metaDataValue", "blue"),
            ],
        },
        Modification {
            case: "one field of the rule's metadata",
            rule: &[("MetaData.MetaDataValue", "green")],
            retargets: false,
            reported: &[
                ("policyRule/metaData/metaDataKey", "tenant"),
                ("policyRule/metaData/metaDataValue", "green"),
            ],
        },
        Modification {
            case: "the protocol and the ports it carries together",
            rule: &[("Protocol", "17"), ("SourcePortRange", "53")],
            retargets: false,
            reported: &[
                ("policyRule/protocol", "17"),
                ("policyRule/sourcePortRange", "53"),
                ("policyRule/destinationPortRange", "443"),
            ],
        },
        Modification {
            case: "the route table the entry targets, and no field of the rule",
            rule: &[],
            retargets: true,
            reported: &[
                ("policyRule/protocol", "6"),
                ("policyRule/sourceCidrBlock", "10.0.0.0/8"),
            ],
        },
        Modification {
            case: "a field of the rule and the route table together",
            rule: &[("DestinationCidrBlock", "10.1.0.0/16")],
            retargets: true,
            reported: &[
                ("policyRule/destinationCidrBlock", "10.1.0.0/16"),
                ("policyRule/protocol", "6"),
            ],
        },
        Modification {
            case: "nothing at all, which leaves the entry as it was",
            rule: &[],
            retargets: false,
            reported: &[
                ("policyRule/protocol", "6"),
                ("policyRule/sourceCidrBlock", "10.0.0.0/8"),
                ("policyRule/sourcePortRange", "1024-2048"),
                ("policyRule/metaData/metaDataValue", "blue"),
            ],
        },
    ];
    for row in rows {
        let case = row.case;
        let fixtures = fixtures().await;
        let params = entry_params(
            &fixtures.policy_table_id,
            "1",
            &fixtures.route_table_id,
            created,
        );
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the entry must be created, got {status} with {body}"
        );

        let mut params = vec![
            param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
            param("PolicyRuleNumber", "1"),
        ];
        params.extend(rule_members(row.rule));
        let expected_target = if row.retargets {
            params.push(param("TargetRouteTableId", &fixtures.other_route_table_id));
            &fixtures.other_route_table_id
        } else {
            &fixtures.route_table_id
        };
        let (status, modified) = invoke(
            &fixtures.router,
            "ModifyTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {modified}"
        );

        let (status, reported) = report(&fixtures.router, &fixtures.policy_table_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {reported}"
        );
        for (body, source) in [(&modified, "the modify"), (&reported, "the report")] {
            assert_eq!(
                entry_member(case, body, "policyRuleNumber").as_deref(),
                Some("1"),
                "case {case:?}: {source} must keep the entry's rule number, got {body}"
            );
            assert_eq!(
                entry_member(case, body, "targetRouteTableId").as_deref(),
                Some(expected_target.as_str()),
                "case {case:?}: {source} must carry the target the entry now has, got {body}"
            );
            assert_eq!(
                entry_member(case, body, "state").as_deref(),
                Some(TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_ACTIVE),
                "case {case:?}: a modify does not move an entry's state, got {body}"
            );
            for (path, expected) in row.reported {
                assert_eq!(
                    entry_member(case, body, path).as_deref(),
                    Some(*expected),
                    "case {case:?}: {source} must carry {path} as {expected:?}, got {body}"
                );
            }
        }
    }
}

/// A modify is held to the same constraints a create is, so it cannot leave an entry
/// carrying a rule a create would have refused — and the entry is left exactly as it was.
#[tokio::test]
async fn a_refused_modify_changes_nothing() {
    let rows: [(&str, &[(&str, &str)]); 3] = [
        (
            "a protocol the published catalog does not admit",
            &[("Protocol", "99")],
        ),
        (
            "a port range left over a protocol that has no ports",
            &[("Protocol", "47")],
        ),
        (
            "a CIDR block that is not a CIDR block",
            &[("SourceCidrBlock", "ten-dot-nothing")],
        ),
    ];
    for (case, rule) in rows {
        let fixtures = fixtures().await;
        let created = entry_params(
            &fixtures.policy_table_id,
            "1",
            &fixtures.route_table_id,
            &[("Protocol", "6"), ("SourcePortRange", "443")],
        );
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &created,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the entry must be created, got {status} with {body}"
        );

        let mut params = vec![
            param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
            param("PolicyRuleNumber", "1"),
        ];
        params.extend(rule_members(rule));
        let (status, body) = invoke(
            &fixtures.router,
            "ModifyTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );

        let (status, body) = report(&fixtures.router, &fixtures.policy_table_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            entry_member(case, &body, "policyRule/protocol").as_deref(),
            Some("6"),
            "case {case:?}: a refused modify must leave the rule as it was, got {body}"
        );
        assert_eq!(
            entry_member(case, &body, "policyRule/sourcePortRange").as_deref(),
            Some("443"),
            "case {case:?}: a refused modify must leave the ports as they were, got {body}"
        );
    }
}

/// Deleting an entry takes it out of its table: the delete answers the entry it removed in
/// the `deleted` state, the report no longer lists it, the table's other entries are left
/// alone, and a second delete of the same entry is refused rather than answering again.
#[tokio::test]
async fn a_deleted_entry_is_gone_from_the_report() {
    let fixtures = seven_entries().await;
    let mut expected: Vec<String> = ORDERED_RULE_NUMBERS
        .iter()
        .map(|number| (*number).to_string())
        .collect();

    // Every rule number in turn, so the delete is exercised at the first entry, the last and
    // the ones between.
    for rule_number in ["1", "100", "7", "65535", "2", "10", "3"] {
        let case = format!("deleting entry {rule_number}");
        let params = vec![
            param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
            param("PolicyRuleNumber", rule_number),
        ];
        let (status, body) = invoke(
            &fixtures.router,
            "DeleteTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            entry_member(&case, &body, "policyRuleNumber").as_deref(),
            Some(rule_number),
            "case {case:?}: the delete must answer the entry it removed, got {body}"
        );
        assert_eq!(
            entry_member(&case, &body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_POLICY_TABLE_ENTRY_STATE_DELETED),
            "case {case:?}: the delete must report the entry deleted, got {body}"
        );

        expected.retain(|number| number != rule_number);
        let (status, body) = report(&fixtures.router, &fixtures.policy_table_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            reported_rule_numbers(&case, &body),
            expected,
            "case {case:?}: the report must list the entries that are left, got {body}"
        );

        // The entry is gone, so deleting it again is a refusal rather than a second answer.
        let (status, body) = invoke(
            &fixtures.router,
            "DeleteTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: a second delete must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidTransitGatewayPolicyTableEntry.NotFound",
            "case {case:?}: expected the entry's own NotFound code, got {body}"
        );
    }

    // The rule number of a deleted entry is free again: a create takes it.
    let params = entry_params(&fixtures.policy_table_id, "1", &fixtures.route_table_id, &[]);
    let (status, body) = invoke(
        &fixtures.router,
        "CreateTransitGatewayPolicyTableEntry",
        &params,
    )
    .await;
    assert_eq!(
        status, 200,
        "the rule number of a deleted entry must be free again, got {status} with {body}"
    );
}

/// A dry run of a request that WOULD have succeeded performs nothing: every operation of the
/// entry surface answers `DryRunOperation` and leaves the table exactly as it was.
#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let fixtures = fixtures().await;
    let created = entry_params(
        &fixtures.policy_table_id,
        "1",
        &fixtures.route_table_id,
        &[("Protocol", "6"), ("SourcePortRange", "443")],
    );
    let (status, body) = invoke(
        &fixtures.router,
        "CreateTransitGatewayPolicyTableEntry",
        &created,
    )
    .await;
    assert_eq!(status, 200, "the entry must be created: {body}");

    let entry = vec![
        param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
        param("PolicyRuleNumber", "1"),
    ];
    let mut another = entry_params(
        &fixtures.policy_table_id,
        "2",
        &fixtures.route_table_id,
        &[("Protocol", "17")],
    );
    another.push(param("DryRun", "true"));
    let mut modify = entry.clone();
    modify.extend(rule_members(&[("Protocol", "17")]));
    modify.push(param("DryRun", "true"));
    let mut delete = entry.clone();
    delete.push(param("DryRun", "true"));
    let mut read = vec![param(
        "TransitGatewayPolicyTableId",
        &fixtures.policy_table_id,
    )];
    read.push(param("DryRun", "true"));

    for (action, params) in [
        ("CreateTransitGatewayPolicyTableEntry", another),
        ("ModifyTransitGatewayPolicyTableEntry", modify),
        ("DeleteTransitGatewayPolicyTableEntry", delete),
        ("GetTransitGatewayPolicyTableEntries", read),
    ] {
        let (status, body) = invoke(&fixtures.router, action, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {action:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {action:?}: expected DryRunOperation, got {body}"
        );
    }

    let (status, body) = report(&fixtures.router, &fixtures.policy_table_id).await;
    assert_eq!(status, 200, "the report must succeed: {body}");
    assert_eq!(
        reported_rule_numbers("after the dry runs", &body),
        vec!["1".to_string()],
        "a dry run must leave the table exactly as it was, got {body}"
    );
    assert_eq!(
        entry_member("after the dry runs", &body, "policyRule/protocol").as_deref(),
        Some("6"),
        "a dry run must not modify the entry it named, got {body}"
    );
}

/// The entries of one table are the entries of THAT table: a second policy table of the same
/// transit gateway holds its own, at rule numbers the first one already uses.
#[tokio::test]
async fn one_tables_entries_are_not_anothers() {
    let fixtures = fixtures().await;
    let (status, body) = invoke(
        &fixtures.router,
        "CreateTransitGatewayPolicyTable",
        &[param("TransitGatewayId", &fixtures.transit_gateway_id)],
    )
    .await;
    assert_eq!(status, 200, "a second policy table must be created: {body}");
    let second_table =
        element(&body, "transitGatewayPolicyTableId").expect("the second policy table's id");

    for (table, protocol) in [(&fixtures.policy_table_id, "6"), (&second_table, "17")] {
        let params = entry_params(table, "1", &fixtures.route_table_id, &[("Protocol", protocol)]);
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "table {table}: the entry must be created, got {status} with {body}"
        );
    }

    for (table, protocol) in [(&fixtures.policy_table_id, "6"), (&second_table, "17")] {
        let (status, body) = report(&fixtures.router, table).await;
        assert_eq!(
            status, 200,
            "table {table}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            reported_rule_numbers(table, &body),
            vec!["1".to_string()],
            "table {table}: expected exactly its own entry, got {body}"
        );
        assert_eq!(
            entry_member(table, &body, "policyRule/protocol").as_deref(),
            Some(protocol),
            "table {table}: expected the rule this table's entry was created with, got {body}"
        );
    }

    // Deleting one table's entry leaves the other table's entry where it was.
    let (status, body) = invoke(
        &fixtures.router,
        "DeleteTransitGatewayPolicyTableEntry",
        &[
            param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
            param("PolicyRuleNumber", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the first table's entry must be deleted: {body}");
    let (status, body) = report(&fixtures.router, &second_table).await;
    assert_eq!(status, 200, "the second report must succeed: {body}");
    assert_eq!(
        reported_rule_numbers("the other table", &body),
        vec!["1".to_string()],
        "the other table's entry must be left alone, got {body}"
    );
}

/// A token the service did not mint is refused rather than read as a position, and a filter
/// name the operation answers none of is refused rather than read as "no results".
#[tokio::test]
async fn the_report_refuses_what_it_cannot_read() {
    let fixtures = seven_entries().await;
    let table = param("TransitGatewayPolicyTableId", &fixtures.policy_table_id);

    let rows: [(&str, Vec<Param>, &str); 4] = [
        (
            "a token the service did not mint",
            vec![table.clone(), param("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
        (
            "a token this service minted that names no rule number",
            vec![
                table.clone(),
                // Minted the way the service mints one, so it passes the token's own
                // verification and is refused for naming no position in this order.
                param("NextToken", &ec2::state::encode_page_token("first")),
            ],
            "InvalidPaginationToken",
        ),
        (
            "a filter name the entry has no attribute for",
            {
                let mut params = vec![table.clone()];
                params.extend(filters(&[("policy-rule.source-port", &["443"])]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "a filter name of another resource",
            {
                let mut params = vec![table.clone()];
                params.extend(filters(&[("transit-gateway-id", &["tgw-0123456789abcdef0"])]));
                params
            },
            "InvalidParameterValue",
        ),
    ];
    for (case, params, code) in rows {
        let (status, body) = invoke(
            &fixtures.router,
            "GetTransitGatewayPolicyTableEntries",
            &params,
        )
        .await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: expected HTTP {REFUSED_STATUS}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got {body}"
        );
    }
}
