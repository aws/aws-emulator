// SPDX-License-Identifier: CC0-1.0

//! `CreateCluster` refuses what its model and its network refuse, and the `Cluster` both
//! it and `DescribeClusters` project.
//!
//! Four rules are covered, each over a table rather than over one request:
//!
//! 1. A request that violates the modelled input shape is refused `400`
//!    `CloudHsmInvalidRequestException` — the one error this model declares for a request
//!    that "is not a valid request" — carrying the AWS input-validation envelope in the
//!    `Message` member every error shape of this model declares. Rows span the constraint
//!    classes the input shape declares: a missing `@required` member, `@pattern` on a
//!    member and on a list's elements, `@length` on a list at each bound, an enum's value
//!    set, and two violations in one request.
//! 2. `SubnetIds` is resolved against the subnets the ACCOUNT has — EC2's resources, read
//!    back from EC2 itself — so an identifier it holds no subnet for is refused `400`
//!    `CloudHsmResourceNotFoundException` with AWS's sentence for that resource, and a
//!    cluster is stored only for subnets the account really has.
//! 3. The `Cluster` both answers project carries `Certificates` — the empty structure
//!    until the cluster is initialized — and a `VpcId` and `SubnetMapping` read off what
//!    EC2 reports for the requested subnets.
//! 4. An `HsmType` no cluster can be created with is refused before the subnets are
//!    resolved at all.
//!
//! Every case drives the real wire path (`POST /` with
//! `X-Amz-Target: BaldrApiService.<Operation>` through the multi-service router), so what
//! is asserted is what a client sees, body and HTTP status included.

use serde_json::{json, Value};

use crate::cluster_lifecycle::{error_code, Emulator, REGION};

/// The `Message` member every error shape of this model declares — where a failure's text
/// is, whichever layer produced it.
fn error_message(response: &Value) -> String {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// The modelled error this service answers a request that violates its input shape with.
const INVALID_REQUEST: &str = "CloudHsmInvalidRequestException";

/// The modelled error it answers a request naming a resource it cannot find with.
const NOT_FOUND: &str = "CloudHsmResourceNotFoundException";

/// A subnet id inside the modelled `^subnet-[0-9a-fA-F]{8,17}$` that no account holds.
const UNHELD_SUBNET: &str = "subnet-aaaaaaaaaaaaaaaa";

/// A second one, so a row can name two.
const OTHER_UNHELD_SUBNET: &str = "subnet-bbbbbbbbbbbbbbbb";

/// The `HsmType` pattern as a refusal of this service QUOTES it: the expression the model
/// declares (`smithy_query(query="operation",operation="CreateCluster")`,
/// `^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$`) without the whole-value anchors, which are how
/// Smithy turns a find into a whole-string match rather than part of the expression a
/// value has to satisfy.
const HSM_TYPE_PATTERN: &str = r"((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)";

/// The `SubnetId` pattern quoted the same way
/// (`smithy_query(query="shape",shape="SubnetIds")`, `^subnet-[0-9a-fA-F]{8,17}$`).
const SUBNET_ID_PATTERN: &str = "subnet-[0-9a-fA-F]{8,17}";

#[tokio::test]
async fn a_request_outside_the_modelled_input_shape_is_refused_as_an_invalid_request() {
    let emulator = Emulator::new();
    let subnets = emulator.subnet_ids_in(REGION).await;
    let held = subnets[0].clone();

    // (case, the request, the whole message it must answer with). The message is the AWS
    // input-validation envelope: `<n> validation error(s) detected: ` over one clause per
    // violated constraint, naming the member as a path spells it and the requirement it
    // failed.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a required member absent",
            json!({ "SubnetIds": [held] }),
            "1 validation error detected: Value null at 'hsmType' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "the other required member absent",
            json!({ "HsmType": "hsm2m.medium" }),
            "1 validation error detected: Value null at 'subnetIds' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a value outside the member's pattern",
            json!({ "HsmType": "not-an-hsm-type", "SubnetIds": [held] }),
            format!(
                "1 validation error detected: Value 'not-an-hsm-type' at 'hsmType' failed \
                 to satisfy constraint: Member must satisfy regular expression pattern: \
                 {HSM_TYPE_PATTERN}"
            ),
        ),
        (
            "a list element outside the element's pattern",
            json!({ "HsmType": "hsm2m.medium", "SubnetIds": ["subnet-nothex"] }),
            format!(
                "1 validation error detected: Value '[subnet-nothex]' at 'subnetIds' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member \
                 must satisfy regular expression pattern: {SUBNET_ID_PATTERN}]"
            ),
        ),
        (
            // Two offending elements of one list are ONE validation error about one
            // member, stated at the list and quoting the whole list.
            "two list elements outside the element's pattern",
            json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": ["subnet-nothex", "subnet-alsonothex"],
            }),
            format!(
                "1 validation error detected: Value '[subnet-nothex, \
                 subnet-alsonothex]' at 'subnetIds' failed to satisfy constraint: Member \
                 must satisfy constraint: [Member must satisfy regular expression \
                 pattern: {SUBNET_ID_PATTERN}]"
            ),
        ),
        (
            "a list below its @length minimum",
            json!({ "HsmType": "hsm2m.medium", "SubnetIds": [] }),
            "1 validation error detected: Value '[]' at 'subnetIds' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a list above its @length maximum",
            json!({
                "HsmType": "hsm2m.medium",
                // Eleven, one past the modelled `SubnetIds` maximum of ten.
                "SubnetIds": [
                    "subnet-00000000", "subnet-00000001", "subnet-00000002",
                    "subnet-00000003", "subnet-00000004", "subnet-00000005",
                    "subnet-00000006", "subnet-00000007", "subnet-00000008",
                    "subnet-00000009", "subnet-0000000a",
                ],
            }),
            "1 validation error detected: Value '[subnet-00000000, subnet-00000001, \
             subnet-00000002, subnet-00000003, subnet-00000004, subnet-00000005, \
             subnet-00000006, subnet-00000007, subnet-00000008, subnet-00000009, \
             subnet-0000000a]' at 'subnetIds' failed to satisfy constraint: Member must \
             have length less than or equal to 10"
                .to_string(),
        ),
        (
            "a value outside an enum's value set",
            json!({ "HsmType": "hsm2m.medium", "SubnetIds": [held], "Mode": "LENIENT" }),
            "1 validation error detected: Value 'LENIENT' at 'mode' failed to satisfy \
             constraint: Member must satisfy enum value set: [FIPS, NON_FIPS]"
                .to_string(),
        ),
        (
            "two violated constraints in one request",
            json!({ "HsmType": "not-an-hsm-type", "SubnetIds": [], "NetworkType": "IPV4" }),
            format!(
                "2 validation errors detected: Value 'not-an-hsm-type' at 'hsmType' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {HSM_TYPE_PATTERN}; Value '[]' at 'subnetIds' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1"
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let (status, response) = emulator.call("CreateCluster", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, INVALID_REQUEST),
            "case {case:?}: expected 400 {INVALID_REQUEST}, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            expected,
            "case {case:?}: expected the AWS validation envelope, got {response}"
        );
    }
}

#[tokio::test]
async fn a_subnet_the_account_does_not_have_is_refused_and_mints_no_cluster() {
    let emulator = Emulator::new();
    let subnets = emulator.subnet_ids_in(REGION).await;
    let held = subnets[0].clone();
    let also_held = subnets[1].clone();

    // (case, the SubnetIds the request names, the identifier the refusal must name): the
    // one identifier the account has no subnet for, in the order the request lists them.
    let rows: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "the only subnet named",
            vec![UNHELD_SUBNET.to_string()],
            UNHELD_SUBNET,
        ),
        (
            "an unheld subnet after a held one",
            vec![held.clone(), UNHELD_SUBNET.to_string()],
            UNHELD_SUBNET,
        ),
        (
            "an unheld subnet before a held one",
            vec![UNHELD_SUBNET.to_string(), also_held.clone()],
            UNHELD_SUBNET,
        ),
        (
            "two unheld subnets, named by the first in request order",
            vec![
                OTHER_UNHELD_SUBNET.to_string(),
                UNHELD_SUBNET.to_string(),
            ],
            OTHER_UNHELD_SUBNET,
        ),
    ];

    for (case, subnet_ids, named) in rows {
        let (status, response) = emulator
            .call(
                "CreateCluster",
                &json!({ "HsmType": "hsm2m.medium", "SubnetIds": subnet_ids }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, NOT_FOUND),
            "case {case:?}: expected 400 {NOT_FOUND}, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("The subnet ID '{named}' does not exist"),
            "case {case:?}: expected AWS's sentence for a subnet that does not exist, got \
             {response}"
        );
    }

    // Nothing was stored for any of them: a refused create mints no cluster.
    let described = emulator
        .ok("after every refusal", "DescribeClusters", &json!({}))
        .await;
    assert_eq!(
        described["Clusters"].as_array().map(Vec::len),
        Some(0),
        "expected no cluster to have been minted, got {described}"
    );
}

/// One subnet a case places a cluster in: the Availability Zone EC2 reports for it, and
/// its identifier.
type PlacedSubnet = (String, String);

/// One row of the projection table: the case, the subnets the cluster is placed in, and
/// the VPC it must report.
type ProjectionRow = (&'static str, Vec<PlacedSubnet>, String);

#[tokio::test]
async fn the_cluster_reports_its_certificates_and_the_vpc_owning_its_subnets() {
    let emulator = Emulator::new();
    let ec2 = emulator.ec2(REGION);

    // Rows: the account's default subnets, and the subnets of a VPC this caller minted —
    // so the VPC a cluster reports is read off the request's subnets rather than off the
    // account's only network.
    let default_subnets = emulator.subnets_in(REGION).await;
    let owned_vpc = ec2
        .create_vpc()
        .cidr_block("10.42.0.0/16")
        .send()
        .await
        .expect("EC2 must create the VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("the VPC must carry an id")
        .to_string();
    let owned_zone = default_subnets[0].0.clone();
    let owned_subnet = ec2
        .create_subnet()
        .vpc_id(&owned_vpc)
        .cidr_block("10.42.1.0/24")
        .availability_zone(&owned_zone)
        .send()
        .await
        .expect("EC2 must create the subnet")
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .expect("the subnet must carry an id")
        .to_string();

    let default_vpc = emulator.vpc_of(REGION, &default_subnets[0].1).await;

    // (case, the subnets the cluster is placed in as (zone, id), the VPC it must report)
    let rows: Vec<ProjectionRow> = vec![
        (
            "one default subnet",
            vec![default_subnets[0].clone()],
            default_vpc.clone(),
        ),
        (
            "two default subnets, one per Availability Zone",
            vec![default_subnets[0].clone(), default_subnets[1].clone()],
            default_vpc.clone(),
        ),
        (
            "a subnet of a VPC the caller minted",
            vec![(owned_zone.clone(), owned_subnet.clone())],
            owned_vpc.clone(),
        ),
    ];

    for (case, placed_in, expected_vpc) in rows {
        let requested: Vec<String> = placed_in.iter().map(|(_, id)| id.clone()).collect();
        let created = emulator
            .ok(
                case,
                "CreateCluster",
                &json!({ "HsmType": "hsm2m.medium", "SubnetIds": requested }),
            )
            .await;
        let cluster = created["Cluster"].clone();
        let cluster_id = cluster["ClusterId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ClusterId in {created}"))
            .to_string();

        // Both answers project the same `Cluster`, so each assertion is made on the create
        // and again on the read.
        let described = emulator
            .ok(
                case,
                "DescribeClusters",
                &json!({ "Filters": { "clusterIds": [cluster_id] } }),
            )
            .await;
        let read_back = described["Clusters"][0].clone();

        for (projection, cluster) in [("CreateCluster", &cluster), ("DescribeClusters", &read_back)]
        {
            assert_eq!(
                cluster.get("Certificates"),
                Some(&json!({})),
                "case {case:?} via {projection}: expected an uninitialized cluster to \
                 report an empty Certificates, got {cluster}"
            );
            assert_eq!(
                cluster["VpcId"].as_str(),
                Some(expected_vpc.as_str()),
                "case {case:?} via {projection}: expected the VPC owning the requested \
                 subnets, got {cluster}"
            );
            let mapping = cluster["SubnetMapping"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?} via {projection}: no SubnetMapping"));
            assert_eq!(
                mapping.len(),
                placed_in.len(),
                "case {case:?} via {projection}: expected one zone per requested subnet, \
                 got {cluster}"
            );
            for (zone, subnet_id) in &placed_in {
                assert_eq!(
                    mapping.get(zone).and_then(Value::as_str),
                    Some(subnet_id.as_str()),
                    "case {case:?} via {projection}: expected {subnet_id} under the zone \
                     EC2 reports for it ({zone}), got {cluster}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_hsm_type_no_cluster_can_be_created_with_is_refused_before_the_subnets() {
    let emulator = Emulator::new();
    let held = emulator.subnet_ids_in(REGION).await[0].clone();

    // (case, the HsmType, the SubnetIds). Every row is INSIDE the modelled `HsmType`
    // pattern, so the request reaches the handler, and none of them names a type a
    // cluster can be created with — `hsm1.medium`, which the member doc still lists,
    // included.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("a retired type", "hsm1.medium", vec![held.clone()]),
        (
            "a type no catalog published",
            "hsm9.large",
            vec![held.clone()],
        ),
        (
            "a type with the pattern's optional prefix",
            "phsm2.medium",
            vec![held.clone()],
        ),
        (
            // The type is judged BEFORE the subnets are resolved, so a request wrong in
            // both ways is refused for the type.
            "a retired type and a subnet the account does not have",
            "hsm1.medium",
            vec![UNHELD_SUBNET.to_string()],
        ),
    ];

    for (case, hsm_type, subnet_ids) in rows {
        let (status, response) = emulator
            .call(
                "CreateCluster",
                &json!({ "HsmType": hsm_type, "SubnetIds": subnet_ids }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, INVALID_REQUEST),
            "case {case:?}: expected 400 {INVALID_REQUEST}, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            "Provided HsmType is not supported.",
            "case {case:?}: expected AWS's sentence for an unsupported HSM type, got \
             {response}"
        );
    }
}

#[tokio::test]
async fn subnets_in_two_different_vpcs_cannot_hold_one_cluster() {
    let emulator = Emulator::new();
    let ec2 = emulator.ec2(REGION);
    let default_subnets = emulator.subnets_in(REGION).await;

    // A second VPC with a subnet in a zone the default subnets do not use, so the only
    // rule the request breaks is the one about the VPC.
    let other_vpc = ec2
        .create_vpc()
        .cidr_block("10.43.0.0/16")
        .send()
        .await
        .expect("EC2 must create the VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("the VPC must carry an id")
        .to_string();
    let other_zone = default_subnets
        .last()
        .expect("the account must have subnets")
        .0
        .clone();
    let other_subnet = ec2
        .create_subnet()
        .vpc_id(&other_vpc)
        .cidr_block("10.43.1.0/24")
        .availability_zone(&other_zone)
        .send()
        .await
        .expect("EC2 must create the subnet")
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .expect("the subnet must carry an id")
        .to_string();

    let (status, response) = emulator
        .call(
            "CreateCluster",
            &json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [default_subnets[0].1.clone(), other_subnet],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, INVALID_REQUEST),
        "expected subnets in two VPCs to be refused 400 {INVALID_REQUEST}, got {status} \
         {response}"
    );
    assert!(
        error_message(&response).contains("same virtual private cloud"),
        "expected the refusal to state the one-VPC rule, got {response}"
    );
}
