// SPDX-License-Identifier: CC0-1.0

//! The EC2 network a directory's create provisioned LEAVES with the directory, driven end to
//! end over HTTP with the owner of security groups mounted beside this service.
//!
//! RULE: a resource this service creates in another service FOR a directory's servers exists
//! exactly as long as those servers do — so the teardown `DeleteDirectory` starts deletes the
//! domain controller security group the create was given
//! (`DirectoryVpcSettingsDescription$SecurityGroupId`, "the domain controller security group
//! identifier for the directory", and `DirectoryConnectSettingsDescription$SecurityGroupId`
//! for an AD Connector) in the service that OWNS security groups, and the directory stops
//! naming a group that owner no longer holds. A directory that was given no group has nothing
//! to release, and one directory's teardown touches no other directory's group.
//!
//! The rows differ along what the rule generalises over: the create that provisioned the group
//! and therefore the placement member it is reported in (Managed Microsoft AD, Simple AD, AD
//! Connector), the subnets the servers were placed in and the blocks they carry, the order the
//! create named them in, whether they are a client's own or the account's default ones, and
//! whether the account has them at all.
//!
//! A security group is an Amazon EC2 resource, so EC2 is mounted beside this service on ONE
//! engine (`crate::directory_dns_addresses::network`, the account network this crate's
//! placement suites share) and every assertion about a group is READ BACK from EC2 itself: a
//! group this service only pretended to delete would still be in EC2's own listing and could
//! not pass. Nothing reaches into either store.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::ad_connector::connect_directory;
use crate::directory_dns_addresses::{ec2_client, network, AccountSubnet, ABSENT_SUBNET};
use crate::directory_lifecycle::{create_directory, descriptions, ok, string, PASSWORD};
use crate::microsoft_ad::create_microsoft_ad;

/// The `CustomerDnsIps` an AD Connector forwards to: addresses of a self-managed network this
/// emulator does not hold, which is what a connector's create names.
const CUSTOMER_DNS_IPS: [&str; 2] = ["10.9.0.10", "10.9.1.10"];

/// Every security group identifier EC2 — the service that owns security groups — reports the
/// account holding, so a case can assert what is there and what is gone without naming an id
/// EC2 would refuse to describe.
async fn groups_held_by_ec2(ec2: &::aws_sdk_ec2::Client, case: &str) -> BTreeSet<String> {
    let described = ec2
        .describe_security_groups()
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must report the account's security groups: {error:?}")
        });
    described
        .security_groups()
        .iter()
        .filter_map(|group| group.group_id().map(str::to_string))
        .collect()
}

/// The identifiers of `subnets`, in the order a create names them.
fn subnet_ids(subnets: &[AccountSubnet]) -> Vec<String> {
    subnets.iter().map(|subnet| subnet.id.clone()).collect()
}

/// Which create a row drives, and so which placement member the directory reports the group
/// its servers sit behind in.
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
    let ids = subnet_ids(subnets);
    let body = match kind {
        Kind::MicrosoftAd => json!({
            "Name": name,
            "Password": PASSWORD,
            "VpcSettings": { "VpcId": vpc_id, "SubnetIds": ids }
        }),
        Kind::SimpleAd => json!({
            "Name": name,
            "Password": PASSWORD,
            "Size": "Small",
            "VpcSettings": { "VpcId": vpc_id, "SubnetIds": ids }
        }),
        Kind::Connector => json!({
            "Name": name,
            "Password": PASSWORD,
            "Size": "Small",
            "ConnectSettings": {
                "VpcId": vpc_id,
                "SubnetIds": ids,
                "CustomerDnsIps": CUSTOMER_DNS_IPS,
                "CustomerUserName": "ad-connector-svc",
            }
        }),
    };
    match kind {
        Kind::MicrosoftAd => create_microsoft_ad(router, body, case).await,
        Kind::SimpleAd => create_directory(router, body, case).await,
        Kind::Connector => connect_directory(router, body, case).await,
    }
}

/// The description `DescribeDirectories` reports for `directory_id`, or `None` once the
/// teardown has taken the record away.
///
/// `Option` rather than a required description: a directory the clock has finished tearing
/// down is one this service no longer holds, and a case that reads one AFTER its delete must
/// not depend on how fast the host got there.
async fn described_if_held(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Option<Value> {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    let page = descriptions(&answer, case);
    assert!(
        page.len() <= 1,
        "case {case:?}: expected at most one description for {directory_id}, got {page:?}"
    );
    page.first().cloned()
}

/// `DeleteDirectory`, asserting it answers 200 with the id it was asked about.
async fn delete(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let answer = ok(
        router,
        "DeleteDirectory",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    assert_eq!(
        answer["DirectoryId"],
        json!(directory_id),
        "case {case:?}: expected the delete to report the id it was asked about, got {answer}"
    );
}

/// One directory a row created, and the group EC2 minted for its servers.
struct Provisioned {
    case: &'static str,
    directory_id: String,
    placement: &'static str,
    group_id: String,
}

/// RULE: the security group the create was given in EC2 is deleted THERE when the directory
/// is deleted, only that directory's group goes, and the directory stops naming it.
#[tokio::test]
async fn the_security_group_a_create_provisioned_leaves_with_the_directory() {
    let network = network().await;
    let router = &network.router;
    let ec2 = ec2_client(&EmbeddedEmulator::from_router(router.clone()));
    let case = "the account's groups before any directory";
    let before = groups_held_by_ec2(&ec2, case).await;

    // The rows differ in the create that provisioned the group (and so in the placement
    // member it is reported in), in the blocks of the chosen subnets, in the order the create
    // named them, and in whether the subnets are the client's own or the account's default
    // ones.
    let rows: [(&'static str, Kind, Vec<AccountSubnet>); 5] = [
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
            "a Simple AD in two /24 subnets, named in the other order",
            Kind::SimpleAd,
            vec![network.client[1].clone(), network.client[0].clone()],
        ),
        (
            "an AD Connector in a /20 and a /28",
            Kind::Connector,
            vec![network.client[3].clone(), network.client[2].clone()],
        ),
    ];

    // Every row's directory is created first, so each delete below runs with the OTHER
    // directories' groups still in EC2: a teardown that took the account's groups with it
    // rather than its own could not pass.
    let mut provisioned: Vec<Provisioned> = Vec::with_capacity(rows.len());
    for (index, (case, kind, subnets)) in rows.iter().enumerate() {
        let name = format!("release{index}.example.com");
        let directory_id = create_in(router, *kind, &name, &network.vpc, subnets, case).await;
        let placement = kind.placement();
        let described = described_if_held(router, &directory_id, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {directory_id} to be described"));
        let group_id = string(&described[placement], "SecurityGroupId", case);
        let held = groups_held_by_ec2(&ec2, case).await;
        assert!(
            held.contains(&group_id),
            "case {case:?}: expected EC2 to hold the group {group_id} the create reported, \
             got {held:?}"
        );
        provisioned.push(Provisioned {
            case,
            directory_id,
            placement,
            group_id,
        });
    }

    // The account's own groups — the ones that were there before any directory — are not
    // this service's to touch, and every directory got its own.
    let groups: BTreeSet<&String> = provisioned.iter().map(|row| &row.group_id).collect();
    assert_eq!(
        groups.len(),
        provisioned.len(),
        "case \"every provisioned group\": expected each directory a group of its own, got \
         {groups:?}"
    );

    for (index, row) in provisioned.iter().enumerate() {
        let case = row.case;
        delete(router, &row.directory_id, case).await;

        // The group is GONE from the owner of security groups: the delete released it there,
        // not just in this service's own record.
        let held = groups_held_by_ec2(&ec2, case).await;
        assert!(
            !held.contains(&row.group_id),
            "case {case:?}: expected EC2 to have released the group {group}, got {held:?}",
            group = row.group_id
        );

        // ...and only that one: every directory not yet deleted still has its group, and so
        // does the account.
        for other in &provisioned[index + 1..] {
            assert!(
                held.contains(&other.group_id),
                "case {case:?}: expected the group {group} of {directory} to be untouched, \
                 got {held:?}",
                group = other.group_id,
                directory = other.directory_id
            );
        }
        for group_id in &before {
            assert!(
                held.contains(group_id),
                "case {case:?}: expected the account's own group {group_id} to be untouched, \
                 got {held:?}"
            );
        }

        // While the teardown is still being reported — a directory the clock has finished
        // tearing down is gone, which is this service's own lifecycle and not what this row
        // is about — the directory names no group, because there is none to name.
        if let Some(described) = described_if_held(router, &row.directory_id, case).await {
            assert!(
                described[row.placement]["SecurityGroupId"].is_null(),
                "case {case:?}: expected a directory whose group has been released to name \
                 none, got {described}"
            );
        }
    }

    // Nothing of the account's own network was taken by any of those teardowns.
    let after = groups_held_by_ec2(&ec2, "the account's groups after every teardown").await;
    assert_eq!(
        after, before,
        "case \"after every teardown\": expected the account to hold exactly the groups it \
         held before the directories"
    );
}

/// RULE: a directory that was given no security group has none to release, so its teardown
/// asks the owner of security groups for nothing and leaves the account's network exactly as
/// it was.
///
/// The rows differ in WHY there is no group: a placement the account has no subnet for, so
/// there was no network to provision one in, and a directory created with no placement at all.
#[tokio::test]
async fn a_directory_that_was_given_no_security_group_releases_nothing() {
    let network = network().await;
    let router = &network.router;
    let ec2 = ec2_client(&EmbeddedEmulator::from_router(router.clone()));
    let before = groups_held_by_ec2(&ec2, "the account's groups before any directory").await;

    let unresolved = "a Managed Microsoft AD whose subnets the account has none of";
    let placed_in_nothing = create_microsoft_ad(
        router,
        json!({
            "Name": "nogroup.example.com",
            "Password": PASSWORD,
            "VpcSettings": {
                "VpcId": network.vpc,
                "SubnetIds": [ABSENT_SUBNET, "subnet-0000beef"],
            }
        }),
        unresolved,
    )
    .await;

    let unplaced = "a Simple AD created with no VpcSettings at all";
    let without_placement = create_directory(
        router,
        json!({ "Name": "noplacement.example.com", "Password": PASSWORD, "Size": "Small" }),
        unplaced,
    )
    .await;

    for (case, directory_id) in [
        (unresolved, &placed_in_nothing),
        (unplaced, &without_placement),
    ] {
        let described = described_if_held(router, directory_id, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {directory_id} to be described"));
        assert!(
            described["VpcSettings"]["SecurityGroupId"].is_null()
                && described["ConnectSettings"]["SecurityGroupId"].is_null(),
            "case {case:?}: expected no SecurityGroupId to be reported at all, got {described}"
        );

        delete(router, directory_id, case).await;
        let held = groups_held_by_ec2(&ec2, case).await;
        assert_eq!(
            held, before,
            "case {case:?}: expected a teardown with no group to release to leave the \
             account's groups alone"
        );
    }
}
