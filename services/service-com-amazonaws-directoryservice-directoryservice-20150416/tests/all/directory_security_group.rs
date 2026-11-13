// SPDX-License-Identifier: CC0-1.0

//! The security group a directory's servers sit behind, driven end to end over HTTP with the
//! owner of security groups mounted beside this service.
//!
//! RULE: a directory this service places in a VPC is provisioned a security group for the
//! servers it runs there, in the VPC the chosen subnets belong to, and reports that group in
//! the placement it describes — `DirectoryVpcSettingsDescription$SecurityGroupId` ("the
//! domain controller security group identifier for the directory") for a directory whose
//! servers this service runs, `DirectoryConnectSettingsDescription$SecurityGroupId` ("the
//! security group identifier for the AD Connector directory") for an AD Connector — while a
//! directory whose placement resolved to nothing reports no group at all.
//!
//! The rows differ along what the rule generalises over: the directory type and therefore the
//! placement member the group is reported in (Managed Microsoft AD, Simple AD, AD Connector),
//! the subnets chosen and the blocks they carry, the order the create names them in, whether
//! they are a client's own or the account's default ones, and whether the account has them at
//! all.
//!
//! A security group is an Amazon EC2 resource, so EC2 is mounted beside this service on ONE
//! engine (`crate::directory_dns_addresses::network`, the account network this crate's
//! placement suites share) and every id this suite asserts on is READ BACK from EC2 itself:
//! an id this service invented would name no group there and could not pass. Nothing reaches
//! into either store.
//!
//! One row places a directory in the account's DEFAULT subnets while its settings block names
//! the client's own VPC — a VPC the account really holds, so the create is not refused for it
//! — so the group can only be found where the SUBNETS are, not where the request said.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::ad_connector::connect_directory;
use crate::directory_dns_addresses::{ec2_client, network, AccountSubnet, ABSENT_SUBNET};
use crate::directory_lifecycle::{
    create_directory, describe_one, ok, string, PASSWORD, STAGE_ACTIVE,
};
use crate::microsoft_ad::create_microsoft_ad;

/// How long a polled read may take to report the directory in service before a case fails:
/// far longer than any span this service publishes, and well inside what a client's waiter
/// allows, so a loaded or faster host changes nothing.
const POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// How often such a read asks again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// The `CustomerDnsIps` an AD Connector forwards to: addresses of a self-managed network this
/// emulator does not hold, which is what a connector's create names.
const CUSTOMER_DNS_IPS: [&str; 2] = ["10.9.0.10", "10.9.1.10"];

/// Assert `id` is a `SecurityGroupId` the model admits: the member's own pattern,
/// `^(sg-[0-9a-f]{8}|sg-[0-9a-f]{17})$`
/// (`smithy_query(query="shape", shape="SecurityGroupId")`).
fn assert_security_group_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("sg-").unwrap_or_else(|| {
        panic!("case {case:?}: expected a SecurityGroupId starting with sg-, got {id:?}")
    });
    assert!(
        (suffix.len() == 8 || suffix.len() == 17)
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected a SecurityGroupId matching \
         ^(sg-[0-9a-f]{{8}}|sg-[0-9a-f]{{17}})$, got {id:?}"
    );
}

/// The VPC EC2 reports for `subnet_id` — the network the directory's servers really sit on,
/// which is what the group it is given has to be in.
async fn vpc_of(ec2: &::aws_sdk_ec2::Client, subnet_id: &str) -> String {
    let described = ec2
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"));
    described
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .and_then(::aws_sdk_ec2::types::Subnet::vpc_id)
        .unwrap_or_else(|| panic!("EC2 must report a VPC for the subnet {subnet_id}"))
        .to_string()
}

/// The group `group_id` as EC2 — the service that owns security groups — reports it, failing
/// the case when it holds none: an id this service reported that EC2 does not have is exactly
/// what must not pass.
async fn group_reported_by_ec2(
    ec2: &::aws_sdk_ec2::Client,
    group_id: &str,
    case: &str,
) -> ::aws_sdk_ec2::types::SecurityGroup {
    let described = ec2
        .describe_security_groups()
        .group_ids(group_id)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must hold the group {group_id}: {error:?}")
        });
    described
        .security_groups()
        .iter()
        .find(|group| group.group_id() == Some(group_id))
        .unwrap_or_else(|| panic!("case {case:?}: EC2 must report the group {group_id}"))
        .clone()
}

/// The directory `directory_id`, polled through `DescribeDirectories` until it reports itself
/// in service, so a case can assert on a directory whose build has finished.
async fn described_in_service(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Value {
    let started = std::time::Instant::now();
    loop {
        let described = describe_one(router, directory_id, case).await;
        if described["Stage"] == json!(STAGE_ACTIVE) {
            return described;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected {directory_id} to report itself in service within \
             {POLL_DEADLINE:?}, last observed {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The identifiers of `subnets`, in the order a create names them.
fn subnet_ids(subnets: &[AccountSubnet]) -> Vec<String> {
    subnets.iter().map(|subnet| subnet.id.clone()).collect()
}

/// A `CreateMicrosoftAD` body placing a directory in `subnets`, with `vpc_id` as the `VpcId`
/// its settings block names.
fn microsoft_ad_in(name: &str, vpc_id: &str, subnets: &[AccountSubnet]) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": { "VpcId": vpc_id, "SubnetIds": subnet_ids(subnets) }
    })
}

/// A `CreateDirectory` (Simple AD) body placing a directory in `subnets`.
fn simple_ad_in(name: &str, vpc_id: &str, subnets: &[AccountSubnet]) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "Size": "Small",
        "VpcSettings": { "VpcId": vpc_id, "SubnetIds": subnet_ids(subnets) }
    })
}

/// A `ConnectDirectory` body placing an AD Connector's servers in `subnets`.
fn connector_in(name: &str, vpc_id: &str, subnets: &[AccountSubnet]) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "Size": "Small",
        "ConnectSettings": {
            "VpcId": vpc_id,
            "SubnetIds": subnet_ids(subnets),
            "CustomerDnsIps": CUSTOMER_DNS_IPS,
            "CustomerUserName": "ad-connector-svc",
        }
    })
}

/// Which create a row drives, and so which placement member the directory reports its group
/// in: the modelled `DirectoryDescription` carries both, and a directory's type decides which
/// one it fills.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// `CreateMicrosoftAD`, reported as `VpcSettings`.
    MicrosoftAd,
    /// `CreateDirectory` (Simple AD), reported as `VpcSettings`.
    SimpleAd,
    /// `ConnectDirectory` (AD Connector), reported as `ConnectSettings`.
    Connector,
}

impl Kind {
    /// The member of the description this kind of directory reports its placement — and so
    /// its security group — in.
    fn placement(self) -> &'static str {
        match self {
            Kind::MicrosoftAd | Kind::SimpleAd => "VpcSettings",
            Kind::Connector => "ConnectSettings",
        }
    }
}

/// Create a directory of `kind` named `name`, whose settings block names `vpc_id` and places
/// its servers in `subnets`, and return its id.
async fn create_in(
    router: &MultiServiceRouter,
    kind: Kind,
    name: &str,
    vpc_id: &str,
    subnets: &[AccountSubnet],
    case: &str,
) -> String {
    match kind {
        Kind::MicrosoftAd => {
            create_microsoft_ad(router, microsoft_ad_in(name, vpc_id, subnets), case).await
        }
        Kind::SimpleAd => {
            create_directory(router, simple_ad_in(name, vpc_id, subnets), case).await
        }
        Kind::Connector => {
            connect_directory(router, connector_in(name, vpc_id, subnets), case).await
        }
    }
}

/// RULE: every directory this service places in a VPC reports a security group for its
/// servers, in the placement member its own type describes, and that group is one the owner of
/// security groups really holds — in the VPC the chosen subnets belong to, named after the
/// directory it belongs to.
#[tokio::test]
async fn a_directory_placed_in_a_vpc_reports_the_security_group_ec2_holds_for_its_servers() {
    let network = network().await;
    let router = &network.router;
    let ec2 = ec2_client(&EmbeddedEmulator::from_router(router.clone()));

    // The rows differ in the directory type (and so in the placement member the group is
    // reported in), in the blocks of the chosen subnets, in the order the create names them,
    // and in whether the subnets are the client's own or the account's default ones.
    let rows: [(&str, Kind, Vec<AccountSubnet>); 5] = [
        (
            "a Managed Microsoft AD in two /24 subnets",
            Kind::MicrosoftAd,
            vec![network.client[0].clone(), network.client[1].clone()],
        ),
        (
            "a Managed Microsoft AD in a /28 and a /20, named in that order",
            Kind::MicrosoftAd,
            vec![network.client[2].clone(), network.client[3].clone()],
        ),
        (
            "a Managed Microsoft AD in the account's default subnets",
            Kind::MicrosoftAd,
            vec![network.default[0].clone(), network.default[1].clone()],
        ),
        (
            "a Simple AD in two /24 subnets",
            Kind::SimpleAd,
            vec![network.client[1].clone(), network.client[0].clone()],
        ),
        (
            "an AD Connector in a /20 and a /28",
            Kind::Connector,
            vec![network.client[3].clone(), network.client[2].clone()],
        ),
    ];

    let mut group_ids: BTreeSet<String> = BTreeSet::new();
    for (index, (case, kind, subnets)) in rows.iter().enumerate() {
        let name = format!("sg{index}.example.com");
        let placement = kind.placement();
        // Every row names the VPC the account really holds — the one `ec2:CreateVpc` minted —
        // whatever subnets it places the servers in, so a row placed in the account's default
        // subnets names a DIFFERENT VPC from the one those subnets are in.
        let directory_id = create_in(router, *kind, &name, &network.vpc, subnets, case).await;

        // The group is part of the network the create provisions, so it is reported from the
        // create on, before the build the directory owes has finished.
        let created = describe_one(router, &directory_id, case).await;
        let at_create = string(&created[placement], "SecurityGroupId", case);
        assert_security_group_id(&at_create, case);

        // ...and the same group is still reported once the directory is in service: a read
        // provisions nothing and changes nothing.
        let described = described_in_service(router, &directory_id, case).await;
        let reported = string(&described[placement], "SecurityGroupId", case);
        assert_eq!(
            reported, at_create,
            "case {case:?}: expected the group reported at create to be the one reported in \
             service, got {described}"
        );

        // The group EC2 HOLDS: in the VPC the chosen subnets belong to, as EC2 reports both,
        // and named after the directory whose servers it is for — Directory Service names the
        // group it creates for a directory `<DirectoryId>_controllers`.
        let group = group_reported_by_ec2(&ec2, &reported, case).await;
        let expected_vpc = vpc_of(&ec2, &subnets[0].id).await;
        assert_eq!(
            group.vpc_id(),
            Some(expected_vpc.as_str()),
            "case {case:?}: expected the group in the VPC of the chosen subnets \
             ({expected_vpc}), got {group:?}"
        );
        assert_eq!(
            group.group_name(),
            Some(format!("{directory_id}_controllers").as_str()),
            "case {case:?}: expected the group named after {directory_id}, got {group:?}"
        );
        assert!(
            group
                .description()
                .is_some_and(|description| description.contains(&directory_id)),
            "case {case:?}: expected the group's description to name {directory_id}, got \
             {group:?}"
        );

        // The directory reports the VPC its create NAMED, which is what this service echoes:
        // only the group is resolved against the network the servers are really on.
        assert_eq!(
            described[placement]["VpcId"],
            json!(network.vpc),
            "case {case:?}: expected the requested VpcId to be echoed, got {described}"
        );

        // No two directories share a group: each one is given its own.
        assert!(
            group_ids.insert(reported.clone()),
            "case {case:?}: expected a group of this directory's own, got {reported} again"
        );
    }
}

/// RULE: nothing is invented for a placement the account does not have — a directory whose
/// subnets the owner of subnets reports nothing for has no VPC to be given a group in, so it
/// reports no `SecurityGroupId` rather than an id naming no group anywhere.
#[tokio::test]
async fn a_directory_whose_placement_does_not_resolve_reports_no_security_group() {
    let network = network().await;
    let router = &network.router;

    // The rows differ in how little of the placement resolves: none of it, and a pair the
    // modelled pattern admits that EC2 mints no subnet for either way.
    let rows: [(&str, Vec<String>); 2] = [
        (
            "two identifiers the account has no subnet for",
            vec![ABSENT_SUBNET.to_string(), "subnet-0000beef".to_string()],
        ),
        (
            "an absent pair in the other order",
            vec!["subnet-0000beef".to_string(), ABSENT_SUBNET.to_string()],
        ),
    ];

    for (index, (case, ids)) in rows.iter().enumerate() {
        // The VPC is one the account really holds, so the create is accepted on its placement
        // and the only thing that resolves to nothing is the subnets.
        let body = json!({
            "Name": format!("nosg{index}.example.com"),
            "Password": PASSWORD,
            "VpcSettings": { "VpcId": network.vpc, "SubnetIds": ids }
        });
        let answer = ok(router, "CreateMicrosoftAD", body, case).await;
        let directory_id = string(&answer, "DirectoryId", case);

        let described = describe_one(router, &directory_id, case).await;
        assert_eq!(
            described["VpcSettings"]["SubnetIds"],
            json!(ids),
            "case {case:?}: expected the requested SubnetIds to be echoed, got {described}"
        );
        assert!(
            described["VpcSettings"]["SecurityGroupId"].is_null(),
            "case {case:?}: expected no SecurityGroupId for a placement that resolves to \
             nothing, got {described}"
        );
    }
}

/// RULE: a directory created with no VPC settings at all — the AD Connector's sibling, a
/// Simple AD created without a placement — has no network to be given a group in and reports
/// neither placement member.
#[tokio::test]
async fn a_directory_created_without_a_placement_reports_no_security_group() {
    let network = network().await;
    let router = &network.router;
    let case = "a Simple AD created with no VpcSettings";

    let directory_id = create_directory(
        router,
        json!({ "Name": "noplacement.example.com", "Password": PASSWORD, "Size": "Small" }),
        case,
    )
    .await;

    let described = describe_one(router, &directory_id, case).await;
    assert!(
        described["VpcSettings"].is_null() && described["ConnectSettings"].is_null(),
        "case {case:?}: expected no placement at all for a directory created without one, got \
         {described}"
    );
}
