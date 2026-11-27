// SPDX-License-Identifier: CC0-1.0

//! Where a DB instance is PLACED, as the service that owns the network answers for it.
//!
//! Two rules, each asserted over a table spanning what it generalises over:
//!
//! * A DB instance is created in a SUBNET of the DB subnet group it is placed in, and a
//!   subnet lies in exactly one Availability Zone — so an `AvailabilityZone` a create names
//!   has to be one of the zones that group's subnets lie in, and a zone the group covers no
//!   subnet in leaves the instance with no address to come up on, which is the modelled
//!   `InvalidVPCNetworkStateFault` (HTTP 400) naming the class and the zone. A create that
//!   names no zone is placed by the service.
//! * `VpcSecurityGroupIds` names Amazon EC2 security groups, so every id a create carries is
//!   resolved against the service that owns them: an id the account holds no group for is
//!   refused (`InvalidParameterValue`, HTTP 400), and the groups it does hold are the
//!   memberships the instance reports.
//!
//! The suite lives beside the router because the network belongs to another crate: Amazon
//! EC2 mints the VPC, its subnets and their Availability Zones, and a crate-local RDS suite
//! can see no EC2 answer at all — the zones of a DB subnet group are unknown there, and a
//! placement rule has nothing to be checked against.
//!
//! Sources: `smithy_query(query="operation", operation="CreateDBInstance")` and
//! `smithy_query(query="doc", shape="CreateDBInstanceMessage", member="AvailabilityZone")`
//! on the rds model.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API versions the two awsQuery front doors here take.
const RDS_API_VERSION: &str = "2014-10-31";
const EC2_API_VERSION: &str = "2016-11-15";

/// The engine and the DB instance class every instance below is created with.
const ENGINE: &str = "mysql";
const CLASS: &str = "db.t3.medium";

/// The emulator as it is ASSEMBLED, on one engine: the hops Amazon RDS makes to EC2 are
/// dispatched back into this same router, so every id a request carries is one EC2 handed
/// out.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Amazon EC2 action that must be served, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "ec2", EC2_API_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {body}"
    );
    body
}

/// One Amazon RDS operation that must be served, answered with its XML body.
async fn rds_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(router, "rds", RDS_API_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with {body}"
    );
    body
}

/// One Amazon RDS operation that must be REFUSED with `status` and the wire error `code`,
/// answered with its XML body.
async fn rds_refused(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
    expected_status: u16,
    expected_code: &str,
) -> String {
    let (status, body) = query(router, "rds", RDS_API_VERSION, action, members).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from {action}, got {status} with {body}"
    );
    let code = element(&body, "Code");
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected the wire error code {expected_code}, got {code} in {body}"
    );
    body
}

/// The zones the DB subnet group below covers, and the one it does not: a group over two
/// subnets covers two of the Region's zones, which is also the coverage a DB subnet group
/// must have ("at least one subnet in at least two AZs").
const COVERED_ZONES: [&str; 2] = ["us-west-2a", "us-west-2b"];

/// The DB subnet group every placement row below names.
const SUBNET_GROUP: &str = "emu-placement-subnets";

/// Create a VPC with one subnet in each of [`COVERED_ZONES`], and the DB subnet group over
/// them; answer the VPC's id.
async fn network_with_subnet_group(router: &MultiServiceRouter) -> String {
    let vpc_id = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.9.0.0/16")]).await,
        "vpcId",
    );
    let mut members: Vec<(String, String)> = vec![
        ("DBSubnetGroupName".to_string(), SUBNET_GROUP.to_string()),
        (
            "DBSubnetGroupDescription".to_string(),
            "one subnet per covered zone".to_string(),
        ),
    ];
    for (index, zone) in COVERED_ZONES.iter().enumerate() {
        let cidr = format!("10.9.{}.0/24", index + 1);
        let subnet_id = element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc_id.as_str()),
                    ("CidrBlock", cidr.as_str()),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        );
        members.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet_id,
        ));
    }
    let borrowed: Vec<(&str, &str)> = members
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    rds_ok(
        router,
        "the DB subnet group every placement row names",
        "CreateDBSubnetGroup",
        &borrowed,
    )
    .await;
    vpc_id
}

/// `CreateDBInstance` members: the three required ones, plus `extra`.
fn create_members<'a>(identifier: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut members: Vec<(&str, &str)> = vec![
        ("DBInstanceIdentifier", identifier),
        ("Engine", ENGINE),
        ("DBInstanceClass", CLASS),
    ];
    members.extend_from_slice(extra);
    members
}

#[tokio::test]
async fn an_availability_zone_the_db_subnet_group_covers_no_subnet_in_is_refused() {
    let router = build_router();
    network_with_subnet_group(&router).await;

    // One row per shape of the placement: the `AvailabilityZone` the create names, and
    // whether the group the instance is placed in covers it. The rows span both zones the
    // group covers, a zone of the Region it covers none of, a zone of another Region
    // entirely, and a create that names no zone at all — which the service places itself.
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("the first zone the group covers", Some(COVERED_ZONES[0]), true),
        ("the second zone the group covers", Some(COVERED_ZONES[1]), true),
        ("a zone of this Region the group covers no subnet in", Some("us-west-2c"), false),
        ("another zone of this Region the group covers none of", Some("us-west-2d"), false),
        ("a zone of another Region", Some("eu-west-1a"), false),
        ("no zone at all", None, true),
    ];

    for (index, (case, zone, covered)) in rows.iter().enumerate() {
        let identifier = format!("emu-placement-{index}");
        let mut extra: Vec<(&str, &str)> = vec![("DBSubnetGroupName", SUBNET_GROUP)];
        if let Some(zone) = zone {
            extra.push(("AvailabilityZone", zone));
        }
        let members = create_members(&identifier, &extra);
        if *covered {
            let created = rds_ok(&router, case, "CreateDBInstance", &members).await;
            let reported = elements(&created, "AvailabilityZone");
            match zone {
                Some(zone) => assert_eq!(
                    reported,
                    vec![(*zone).to_string()],
                    "case {case:?}: the instance must report the zone it was placed in, \
                     got {created}"
                ),
                // A create that names none is placed in a zone the service chooses, which
                // has to be one the group covers all the same.
                None => {
                    let placed = reported.first().cloned().unwrap_or_else(|| {
                        panic!("case {case:?}: the instance must report a zone, got {created}")
                    });
                    assert!(
                        COVERED_ZONES.contains(&placed.as_str()),
                        "case {case:?}: an instance placed by the service must sit in a zone \
                         its DB subnet group covers ({COVERED_ZONES:?}), got {placed:?}"
                    );
                }
            }
        } else {
            let refused = rds_refused(
                &router,
                case,
                "CreateDBInstance",
                &members,
                400,
                "InvalidVPCNetworkStateFault",
            )
            .await;
            let message = element(&refused, "Message");
            let zone = zone.expect("a refused row names a zone");
            assert!(
                message.contains(zone) && message.contains(CLASS),
                "case {case:?}: expected the refusal to name the zone the group covers no \
                 subnet in and the class the instance would have run, got {message:?}"
            );
            // Nothing was stored for the refused create.
            rds_refused(
                &router,
                case,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
                404,
                "DBInstanceNotFound",
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_vpc_security_group_the_account_holds_none_of_is_refused_and_the_rest_are_reported() {
    let router = build_router();
    let vpc_id = network_with_subnet_group(&router).await;
    // Two groups EC2 minted in that VPC: the ids below are the ones it handed out, never
    // values this suite chose.
    let mut minted: Vec<String> = Vec::new();
    for name in ["emu-placement-sg-one", "emu-placement-sg-two"] {
        minted.push(element(
            &ec2(
                &router,
                "CreateSecurityGroup",
                &[
                    ("GroupName", name),
                    ("GroupDescription", "a group a DB instance is placed in"),
                    ("VpcId", vpc_id.as_str()),
                ],
            )
            .await,
            "groupId",
        ));
    }
    // An id of the right FORM that the account holds no group for: a reference that resolves
    // to nothing however well formed it is.
    let absent = "sg-00000000000000bad";

    // One row per shape of the membership: one group EC2 minted, both of them, an id nothing
    // minted, and a list mixing a minted id with one nothing minted — which is refused as a
    // whole, since a membership the account cannot resolve cannot be stored.
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        ("one group the account holds", vec![minted[0].clone()], true),
        (
            "both groups the account holds",
            vec![minted[0].clone(), minted[1].clone()],
            true,
        ),
        ("an id nothing minted", vec![absent.to_string()], false),
        (
            "a minted id beside one nothing minted",
            vec![minted[1].clone(), absent.to_string()],
            false,
        ),
        (
            "an id nothing minted beside a minted one",
            vec![absent.to_string(), minted[0].clone()],
            false,
        ),
    ];

    for (index, (case, groups, resolved)) in rows.iter().enumerate() {
        let identifier = format!("emu-placement-sg-{index}");
        let named: Vec<(String, String)> = groups
            .iter()
            .enumerate()
            .map(|(position, group)| {
                (
                    format!("VpcSecurityGroupIds.VpcSecurityGroupId.{}", position + 1),
                    group.clone(),
                )
            })
            .collect();
        let mut members = create_members(&identifier, &[]);
        members.extend(
            named
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        );
        if *resolved {
            let created = rds_ok(&router, case, "CreateDBInstance", &members).await;
            assert_eq!(
                elements(&created, "VpcSecurityGroupId"),
                *groups,
                "case {case:?}: the instance must report one membership per id the create \
                 named, got {created}"
            );
        } else {
            let refused = rds_refused(
                &router,
                case,
                "CreateDBInstance",
                &members,
                400,
                "InvalidParameterValue",
            )
            .await;
            let message = element(&refused, "Message");
            assert!(
                message.contains(absent),
                "case {case:?}: expected the refusal to name the group id it could not \
                 resolve, got {message:?}"
            );
            // Nothing was stored for the refused create.
            rds_refused(
                &router,
                case,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
                404,
                "DBInstanceNotFound",
            )
            .await;
        }
    }
}
