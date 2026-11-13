// SPDX-License-Identifier: CC0-1.0

//! The VPC a placement names, resolved against the service that OWNS VPCs, driven end to end
//! over HTTP with both services on one engine.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, dispatched
//! by the multi-service router. The network each case runs against is built through EC2's own
//! API (`crate::directory_dns_addresses::network`), so the VPC a case names as held is one
//! `ec2:CreateVpc` really minted and the one it names as absent is one EC2 reports nothing
//! for.
//!
//! The rule under test: a call that places a directory's servers in a VPC resolves that VPC
//! against the account, and one naming a VPC the account does not have is refused with the
//! resource-semantics failure class — `ClientException`, "Invalid VPC ID." — before anything
//! is stored, while one naming a VPC the account HAS is served. A directory is not reported
//! as placed in a VPC that does not exist.
//!
//! It is asserted over a TABLE rather than one recorded call, along what the rule generalises
//! over: every operation that names a VPC for a placement (both creates that make a
//! directory, the create that connects one, and the add that replicates one into another
//! Region), and the form of the identifier the account has no VPC for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_dns_addresses::network;
use crate::directory_lifecycle::{call, describe_one, ok, string, PASSWORD, REGION};

/// The sentence Directory Service refuses a placement whose VPC the account does not have
/// with: fixed text, naming neither the identifier nor the account.
const INVALID_VPC_ID: &str = "Invalid VPC ID.";

/// Well-formed `VpcId`s the account has no VPC for, as `(id, class)`: the form EC2 mints one
/// in, with different digit widths and digits, so no row can pass by being malformed.
const ABSENT_VPCS: [(&str, &str); 3] = [
    ("vpc-0a1b2c3d", "the eight-digit form"),
    ("vpc-00000000000000000", "a seventeen-digit id of all zeros"),
    ("vpc-0f1e2d3c4b5a69788", "a seventeen-digit id of every digit"),
];

/// One row: an operation that names a VPC for a placement, and the body it names it in.
struct PlacementCase {
    case: &'static str,
    operation: &'static str,
    body: fn(&str, &[String]) -> Value,
}

/// Every operation of this service that places servers in a VPC. The bodies carry the two
/// subnets a placement needs, so what refuses a case is the VPC and nothing else.
fn placement_rows() -> Vec<PlacementCase> {
    vec![
        PlacementCase {
            case: "a Simple AD create",
            operation: "CreateDirectory",
            body: |vpc, subnets| {
                json!({
                    "Name": "vpc-simple.example.com",
                    "Password": PASSWORD,
                    "Size": "Small",
                    "VpcSettings": { "VpcId": vpc, "SubnetIds": subnets }
                })
            },
        },
        PlacementCase {
            case: "a Managed Microsoft AD create",
            operation: "CreateMicrosoftAD",
            body: |vpc, subnets| {
                json!({
                    "Name": "vpc-managed.example.com",
                    "Password": PASSWORD,
                    "VpcSettings": { "VpcId": vpc, "SubnetIds": subnets }
                })
            },
        },
        PlacementCase {
            case: "an AD Connector create",
            operation: "ConnectDirectory",
            body: |vpc, subnets| {
                json!({
                    "Name": "vpc-connected.example.com",
                    "Password": "Sim2Real-Connect-Pw1!",
                    "Size": "Small",
                    "ConnectSettings": {
                        "VpcId": vpc,
                        "SubnetIds": subnets,
                        "CustomerDnsIps": ["10.0.0.10", "10.0.0.11"],
                        "CustomerUserName": "ad-connector-svc"
                    }
                })
            },
        },
    ]
}

/// Assert `operation` with `body` is refused with the modelled `ClientException` and the
/// fixed sentence, whose only variable part is the call's own request id.
async fn refused_as_invalid_vpc(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse the absent VPC, got {status} with \
         {parsed}"
    );
    assert_eq!(
        parsed["__type"],
        json!("ClientException"),
        "case {case:?}: expected the resource-semantics refusal, got {parsed}"
    );
    let reported = string(&parsed, "RequestId", case);
    let stated = parsed["Message"].as_str().unwrap_or_default();
    assert_eq!(
        stated,
        format!("{INVALID_VPC_ID} : RequestId: {reported}"),
        "case {case:?}: expected the invalid-VPC sentence, got {parsed}"
    );
}

/// How many directories the store holds, read back through `DescribeDirectories`.
async fn stored_count(router: &MultiServiceRouter, case: &str) -> usize {
    let answer = ok(router, "DescribeDirectories", json!({}), case).await;
    answer["DirectoryDescriptions"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: a placement naming a VPC the account does not have is refused with
/// `ClientException` "Invalid VPC ID.", and stores nothing.
#[tokio::test]
async fn a_placement_in_a_vpc_the_account_does_not_have_is_refused() {
    let network = network().await;
    let router = &network.router;
    // The subnets are named by the ids EC2 minted, read back through the owner the service
    // itself asks, so what refuses a row is the VPC and nothing else.
    let subnet_ids = held_subnet_ids(&network.client[..2]);

    let before = stored_count(router, "before any refused placement").await;
    for row in placement_rows() {
        for (absent, form) in ABSENT_VPCS {
            let case = format!("{}: {form}", row.case);
            refused_as_invalid_vpc(router, row.operation, (row.body)(absent, &subnet_ids), &case)
                .await;
        }
    }
    assert_eq!(
        stored_count(router, "after every refused placement").await,
        before,
        "expected a refused placement to store nothing"
    );
}

/// RULE: the same placement in a VPC the account HAS is served, and the directory reports the
/// VPC it was placed in — so the refusal above is about the resource and not about the shape
/// of the request.
#[tokio::test]
async fn a_placement_in_a_vpc_the_account_has_is_served() {
    let network = network().await;
    let router = &network.router;
    let subnet_ids = held_subnet_ids(&network.client[..2]);

    for row in placement_rows() {
        let case = format!("{}: the VPC the account holds", row.case);
        let answer = ok(
            router,
            row.operation,
            (row.body)(&network.vpc, &subnet_ids),
            &case,
        )
        .await;
        let directory_id = string(&answer, "DirectoryId", &case);
        let described = describe_one(router, &directory_id, &case).await;
        let settings = if row.operation == "ConnectDirectory" {
            &described["ConnectSettings"]
        } else {
            &described["VpcSettings"]
        };
        assert_eq!(
            settings["VpcId"],
            json!(&network.vpc),
            "case {case:?}: expected the placement to report the VPC it was made in, got \
             {described}"
        );
    }
}

/// RULE: the add that replicates a directory into another Region resolves the VPC there the
/// same way — refused for one the account does not have, in the resource-semantics class.
#[tokio::test]
async fn a_replica_in_a_vpc_the_account_does_not_have_is_refused() {
    let network = network().await;
    let router = &network.router;
    let subnet_ids = held_subnet_ids(&network.client[..2]);
    let case = "an Enterprise Managed Microsoft AD directory";

    let answer = ok(
        router,
        "CreateMicrosoftAD",
        json!({
            "Name": "vpc-replicated.example.com",
            "Password": PASSWORD,
            "Edition": "Enterprise",
            "VpcSettings": { "VpcId": &network.vpc, "SubnetIds": &subnet_ids }
        }),
        case,
    )
    .await;
    let directory_id = string(&answer, "DirectoryId", case);

    for (absent, form) in ABSENT_VPCS {
        refused_as_invalid_vpc(
            router,
            "AddRegion",
            json!({
                "DirectoryId": &directory_id,
                "RegionName": REGION,
                "VPCSettings": { "VpcId": absent, "SubnetIds": &subnet_ids }
            }),
            &format!("{case}: {form}"),
        )
        .await;
    }
}

/// The identifiers of `subnets`, as the owner of subnets minted them.
fn held_subnet_ids(subnets: &[crate::directory_dns_addresses::AccountSubnet]) -> Vec<String> {
    subnets.iter().map(|subnet| subnet.id.clone()).collect()
}
