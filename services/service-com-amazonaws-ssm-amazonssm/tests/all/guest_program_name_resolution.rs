// SPDX-License-Identifier: CC0-1.0

//! The NAMES a program run inside a managed node's guest resolves, driven end to end with
//! Systems Manager, Amazon EC2, IAM, Route 53 and Route 53 Resolver on one engine.
//!
//! RULE under test: A PROGRAM RUN IN A MANAGED NODE'S GUEST RESOLVES A NAME THROUGH THE DNS
//! OF THE VPC THAT NODE IS IN — the same server, and the same records, the guest's own
//! resolver clients are answered by — and a lookup that asks for the whole answer reports the
//! name the chain ends at, the names it passed through on the way, and that name's addresses;
//! a name that server holds no address under is reported as a name that does not resolve.
//!
//! Every case goes through the front door: the VPC, its subnet, its security group and the
//! launch over EC2's `ec2Query` wire, the role and instance profile over IAM's `awsQuery`
//! wire, the private hosted zone and its records over Route 53's `restXml` wire, the DNS
//! Firewall rule group, its domain lists, its rules and their association with the VPC over
//! Route 53 Resolver's `awsJson1_1` wire, and the lookup itself as a script
//! `AmazonSSM.SendCommand` delivers to the node, read back with
//! `AmazonSSM.GetCommandInvocation`.
//!
//! The rows span what the rule generalises over: WHICH authority answers the name (a private
//! hosted zone of the VPC, the VPC's DNS Firewall, the public names a DNS standard fixes), how
//! many names the chain passes through (none, one of a zone's own aliases, one the firewall
//! substitutes), and the two ways a name fails to resolve (the firewall answering it absent,
//! and a public name nothing here holds).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::ec2_instance_managed_nodes::{
    launched_long_enough_ago, Emulator, Grant, AL2023_NAME_PATTERN, REGION, SSM_CORE_POLICY_ARN,
};
use super::run_command_in_the_guest::{finish_due, finished, send};

/// The VPC the whole suite's node stands in.
const VPC_CIDR: &str = "10.92.0.0/16";

/// The one subnet the node is launched into.
const SUBNET_CIDR: &str = "10.92.1.0/24";

/// The address every VPC's own resolver answers at, which is where the guest's resolver
/// client sends a query of its own.
const LINK_LOCAL_RESOLVER: &str = "169.254.169.253";

/// The private zone associated with that VPC.
const ZONE: &str = "example.internal";

/// The address the zone's own `A` record holds, from the documentation range RFC 5737
/// reserves.
const RECORD_ADDRESS: &str = "192.0.2.10";

/// The IPv4 address a name a DNS standard RESERVES for documentation is answered with, which
/// is the one address this emulator answers such a name at
/// (`service_common::public_dns_standard::DOCUMENTATION_ADDRESS`).
const DOCUMENTATION_ADDRESS: &str = "192.0.2.1";

/// The name the firewall rule BLOCKS with a record of its own, under a documentation domain
/// so it names no host of anything.
const OVERRIDDEN_NAME: &str = "blocked.example.org";

/// The name that rule substitutes for it: a reserved documentation domain, which is a name
/// this emulator can answer an address for.
const OVERRIDE_TARGET: &str = "example.com";

/// The name the second firewall rule answers as a name that does not exist.
const ABSENT_NAME: &str = "filtered.example.org";

/// A public name no zone of the account holds and no standard fixes, so nothing here can
/// resolve it.
const UNHELD_NAME: &str = "broker.example.net";

/// How long a case looks for a clock-owed move, far longer than the span it is owed after.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often it looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The settled `FirewallRuleGroupAssociationStatus`/`FirewallRuleStatus` a rule group that is
/// filtering reports (`smithy_query(query="shape",shape="FirewallRuleGroupAssociationStatus")`).
const SETTLED: &str = "COMPLETE";

/// The estate every case queries from: the node, and the VPC it stands in.
struct Estate {
    emulator: Emulator,
    instance: String,
}

/// Build it: a VPC with one subnet, a node whose profile lets its agent register, a private
/// hosted zone holding a record and an alias to it, and a DNS Firewall rule group — one rule
/// substituting a record for [`OVERRIDDEN_NAME`], one answering [`ABSENT_NAME`] absent —
/// associated with the VPC and settled.
async fn estate(case: &str) -> Estate {
    let emulator = Emulator::new();
    let vpc = Emulator::answered(
        &emulator
            .ec2(case, "CreateVpc", &[("CidrBlock", VPC_CIDR)])
            .await,
        "vpcId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must mint a VPC"));
    let subnet = Emulator::answered(
        &emulator
            .ec2(
                case,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", SUBNET_CIDR),
                    ("AvailabilityZone", format!("{REGION}a").as_str()),
                ],
            )
            .await,
        "subnetId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateSubnet must mint a subnet"));

    let image = emulator.image(case, AL2023_NAME_PATTERN).await;
    let profile = emulator
        .profile(case, "guest-lookup-role", &Grant::Attached(SSM_CORE_POLICY_ARN))
        .await;
    let instance = Emulator::answered(
        &emulator
            .ec2(
                case,
                "RunInstances",
                &[
                    ("ImageId", image.as_str()),
                    ("InstanceType", "t3.micro"),
                    ("MinCount", "1"),
                    ("MaxCount", "1"),
                    ("SubnetId", subnet.as_str()),
                    ("IamInstanceProfile.Name", profile.as_str()),
                ],
            )
            .await,
        "instanceId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: RunInstances must mint an instance"));
    emulator.launched_ago(&instance, launched_long_enough_ago());
    let reported = emulator.nodes(case, REGION).await;
    assert!(
        reported.contains_key(&instance),
        "case {case:?}: the launched instance must become a managed node first, got {:?}",
        reported.keys().collect::<Vec<_>>()
    );

    zone(&emulator, case, &vpc).await;
    firewall(&emulator, case, &vpc).await;

    Estate { emulator, instance }
}

/// The private hosted zone associated with `vpc`, holding one `A` record and one alias to it.
async fn zone(emulator: &Emulator, case: &str, vpc: &str) {
    let created = emulator
        .route53(
            case,
            "POST",
            "/hostedzone",
            &format!(
                "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                 <Name>{ZONE}</Name>\
                 <VPC><VPCId>{vpc}</VPCId><VPCRegion>{REGION}</VPCRegion></VPC>\
                 <CallerReference>guest-lookup-suite</CallerReference>\
                 <HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
                 </CreateHostedZoneRequest>"
            ),
            201,
        )
        .await;
    let zone_id = Emulator::answered(&created, "Id")
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer the zone id: {created}"));
    let zone_id = zone_id.rsplit('/').next().unwrap_or_default().to_string();
    let changes = [
        ("app", "A", RECORD_ADDRESS.to_string()),
        ("www", "CNAME", format!("app.{ZONE}")),
    ]
    .iter()
    .map(|(label, record_type, value)| {
        format!(
            "<Change><Action>CREATE</Action><ResourceRecordSet>\
             <Name>{label}.{ZONE}</Name><Type>{record_type}</Type><TTL>60</TTL>\
             <ResourceRecords><ResourceRecord><Value>{value}</Value></ResourceRecord>\
             </ResourceRecords></ResourceRecordSet></Change>"
        )
    })
    .collect::<String>();
    emulator
        .route53(
            case,
            "POST",
            &format!("/hostedzone/{zone_id}/rrset"),
            &format!(
                "<ChangeResourceRecordSetsRequest \
                 xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                 <ChangeBatch><Comment>the zone's own records</Comment>\
                 <Changes>{changes}</Changes></ChangeBatch>\
                 </ChangeResourceRecordSetsRequest>"
            ),
            200,
        )
        .await;
}

/// The DNS Firewall rule group filtering `vpc`: one rule substituting [`OVERRIDE_TARGET`] for
/// [`OVERRIDDEN_NAME`], one answering [`ABSENT_NAME`] as a name that does not exist.
async fn firewall(emulator: &Emulator, case: &str, vpc: &str) {
    let group = emulator
        .resolver(
            case,
            "CreateFirewallRuleGroup",
            &json!({"CreatorRequestId": "guest-lookup-group", "Name": "guest-lookup"}),
        )
        .await["FirewallRuleGroup"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create must mint a rule group"))
        .to_string();
    let mut lists = Vec::new();
    for (index, domain) in [OVERRIDDEN_NAME, ABSENT_NAME].iter().enumerate() {
        let list = emulator
            .resolver(
                case,
                "CreateFirewallDomainList",
                &json!({
                    "CreatorRequestId": format!("guest-lookup-list-{index}"),
                    "Name": format!("guest-lookup-{index}"),
                }),
            )
            .await["FirewallDomainList"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must mint a domain list"))
            .to_string();
        emulator
            .resolver(
                case,
                "UpdateFirewallDomains",
                &json!({
                    "FirewallDomainListId": list,
                    "Operation": "ADD",
                    "Domains": [domain],
                }),
            )
            .await;
        lists.push(list);
    }
    emulator
        .resolver(
            case,
            "CreateFirewallRule",
            &json!({
                "CreatorRequestId": "guest-lookup-override",
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": lists[0],
                "Name": "substitute-a-record",
                "Priority": 100,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": OVERRIDE_TARGET,
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            }),
        )
        .await;
    emulator
        .resolver(
            case,
            "CreateFirewallRule",
            &json!({
                "CreatorRequestId": "guest-lookup-absent",
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": lists[1],
                "Name": "answer-absent",
                "Priority": 200,
                "Action": "BLOCK",
                "BlockResponse": "NXDOMAIN",
            }),
        )
        .await;
    emulator
        .resolver(
            case,
            "AssociateFirewallRuleGroup",
            &json!({
                "CreatorRequestId": "guest-lookup-association",
                "FirewallRuleGroupId": group,
                "VpcId": vpc,
                "Priority": 101,
                "Name": "guest-lookup-association",
            }),
        )
        .await;
    // A rule group filters only once its association has settled: the suite looks for that
    // status rather than waiting out a span of the clock.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let associations = emulator
            .resolver(
                case,
                "ListFirewallRuleGroupAssociations",
                &json!({"FirewallRuleGroupId": group}),
            )
            .await;
        if settled(&associations["FirewallRuleGroupAssociations"]) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rule group association must settle, got {associations}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether every member of the reported list has reached [`SETTLED`], and there is at least
/// one.
fn settled(reported: &Value) -> bool {
    let held = reported.as_array().map(Vec::as_slice).unwrap_or_default();
    !held.is_empty()
        && held
            .iter()
            .all(|entry| entry["Status"].as_str() == Some(SETTLED))
}

/// Run `script` in the node's guest and answer what it wrote to standard output, beside the
/// status its shell ended on.
async fn output(estate: &Estate, case: &str, script: &[&str]) -> (String, i64) {
    let command_id = send(&estate.emulator, case, &estate.instance, script).await;
    finish_due(&estate.emulator, &command_id);
    let answered = finished(&estate.emulator, case, &command_id, &estate.instance).await;
    (
        answered["StandardOutputContent"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        answered["ResponseCode"].as_i64().unwrap_or_default(),
    )
}

/// The lines of the one probe every row sends: the universal idiom for reading in-guest state
/// through Run Command — a script ending in an inline `python3` program that prints one line
/// for the client to parse. It asks for the WHOLE answer to a name
/// (`socket.gethostbyname_ex`, which reports the canonical name, the aliases and the
/// addresses) and prints what came back, or the name of the error a lookup that resolved
/// nothing raised.
fn probe(name: &str) -> Vec<String> {
    vec![
        "python3 - <<'PROBE'".to_string(),
        "import socket".to_string(),
        "try:".to_string(),
        format!("    cname, aliases, addresses = socket.gethostbyname_ex({name:?})"),
        "    print('cname', cname, 'aliases', aliases, 'addresses', addresses)".to_string(),
        "except OSError as failed:".to_string(),
        "    print('error', type(failed).__name__)".to_string(),
        "PROBE".to_string(),
    ]
}

/// RULE: a program in the guest resolves a name through the DNS of the node's own VPC, and
/// the whole answer it reads back is the name the chain ends at, the names it passed through
/// and that name's addresses — with a name that server holds no address under reported as one
/// that does not resolve.
#[tokio::test]
async fn a_guest_program_resolves_a_name_through_its_vpcs_dns() {
    let case = "the lookup table";
    let estate = estate(case).await;
    // (case, the name asked about, the line the program prints)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a name a private hosted zone of the VPC holds",
            format!("app.{ZONE}"),
            format!("cname app.{ZONE} aliases [] addresses ['{RECORD_ADDRESS}']"),
        ),
        (
            "an alias of that zone, chased to the name it stands for",
            format!("www.{ZONE}"),
            format!(
                "cname app.{ZONE} aliases ['www.{ZONE}'] addresses ['{RECORD_ADDRESS}']"
            ),
        ),
        (
            "a name the VPC's DNS Firewall substitutes a record for",
            OVERRIDDEN_NAME.to_string(),
            format!(
                "cname {OVERRIDE_TARGET} aliases ['{OVERRIDDEN_NAME}'] \
                 addresses ['{DOCUMENTATION_ADDRESS}']"
            ),
        ),
        (
            "a public name a DNS standard fixes the address of",
            OVERRIDE_TARGET.to_string(),
            format!("cname {OVERRIDE_TARGET} aliases [] addresses ['{DOCUMENTATION_ADDRESS}']"),
        ),
        (
            "a name the firewall answers as one that does not exist",
            ABSENT_NAME.to_string(),
            "error gaierror".to_string(),
        ),
        (
            "a public name nothing here holds an address under",
            UNHELD_NAME.to_string(),
            "error gaierror".to_string(),
        ),
    ];
    for (row, name, expected) in &rows {
        let script = probe(name);
        let lines: Vec<&str> = script.iter().map(String::as_str).collect();
        let (written, code) = output(&estate, row, &lines).await;
        assert_eq!(
            written,
            format!("{expected}\n"),
            "case {row:?}: expected {expected:?} for {name:?}, got {written:?}"
        );
        assert_eq!(
            code, 0,
            "case {row:?}: the program ran to its end, so it exits zero, got {code}"
        );
    }
}

/// RULE: the program and the guest's own resolver client are answered by the same server with
/// the same records, so the name a lookup reports as canonical is the one a query sent to the
/// VPC's resolver is answered with.
#[tokio::test]
async fn the_program_and_the_resolver_client_agree() {
    let case = "the two clients";
    let estate = estate(case).await;
    let (queried, code) = output(
        &estate,
        case,
        &[&format!(
            "dig +short @{LINK_LOCAL_RESOLVER} {OVERRIDDEN_NAME} A"
        )],
    )
    .await;
    assert_eq!(
        code, 0,
        "case {case:?}: a query that was answered exits zero, got {code}"
    );
    let values: Vec<&str> = queried.lines().collect();
    assert!(
        values.first() == Some(&format!("{OVERRIDE_TARGET}.").as_str())
            || values.first() == Some(&OVERRIDE_TARGET),
        "case {case:?}: expected the record the firewall substitutes, got {values:?}"
    );
    let script = probe(OVERRIDDEN_NAME);
    let lines: Vec<&str> = script.iter().map(String::as_str).collect();
    let (written, _) = output(&estate, case, &lines).await;
    assert!(
        written.contains(&format!("cname {OVERRIDE_TARGET} ")),
        "case {case:?}: expected the program to report the same name, got {written:?}"
    );
}
