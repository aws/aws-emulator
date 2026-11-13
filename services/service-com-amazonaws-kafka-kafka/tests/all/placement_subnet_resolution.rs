// SPDX-License-Identifier: CC0-1.0

//! A placement refusal is GROUNDED in what Amazon EC2 answered, never assumed from
//! silence.
//!
//! RULE: the subnets a create places a resource in are Amazon EC2's resources, so this
//! service resolves them against EC2 instead of holding an inventory of its own — and where
//! there is no EC2 to ask, nothing is known about the account's subnets, so the create is
//! accepted and the ids it named are stored and reported back as submitted. (The refusing
//! half of the same rule needs both services and is exercised in
//! `core/router/tests/all/kafka_placement_subnets_resolve_in_ec2.rs`, where Amazon EC2 is
//! mounted beside this one and mints the subnets itself.)
//!
//! This is what every other suite in this crate relies on: the harness mounts this service
//! alone, so the ids its rows name are never resolvable, and a create that refused on
//! silence would make every one of them fail.
//!
//! The rows differ along what the rule generalises over: the operation and the member it
//! names subnets in (`CreateCluster`'s and `CreateClusterV2`'s provisioned
//! `BrokerNodeGroupInfo$ClientSubnets`, `CreateClusterV2`'s serverless
//! `VpcConfig$SubnetIds`, `CreateVpcConnection`'s `ClientSubnets`), the FORM of the ids
//! (Amazon EC2's 17-hex and 8-hex id forms), and how many of them the request names.
//!
//! The last suite here is the one bound that is answered from the request alone, so it
//! holds with or without an EC2 to ask: how many subnets one create may name
//! (`state::network::MAX_PLACEMENT_SUBNETS`), tested at the ceiling and one past it.

use serde_json::{json, Value};

use crate::kafka_harness::{sleep_past_activation, ClusterRequest, ConnectionRequest, Emulator};

/// Subnet ids in both of Amazon EC2's published id forms, which no subnet of the account
/// carries while there is no EC2 to resolve them against.
const UNRESOLVED_SUBNETS: [(&str, &str); 2] = [
    ("EC2's 17-hex id form", "subnet-0f1e2d3c4b5a69780"),
    ("EC2's older 8-hex id form", "subnet-1a2b3c4d"),
];

/// `CreateCluster` stores and reports the subnets it was given when there is no Amazon EC2
/// to resolve them against.
#[tokio::test]
async fn a_provisioned_create_keeps_the_subnets_it_was_given_when_ec2_cannot_be_asked() {
    let emulator = Emulator::new();

    for (index, (form, subnet)) in UNRESOLVED_SUBNETS.iter().enumerate() {
        // The rows differ in how many subnets the create names as well as in their form:
        // one, and the same one beside a second id of the other form.
        for (count, subnets) in [
            (1, vec![*subnet]),
            (2, vec![*subnet, UNRESOLVED_SUBNETS[1 - index].1]),
        ] {
            let case = format!("{form}, {count} subnet(s)");
            let request = ClusterRequest {
                brokers: 2,
                subnets: subnets.clone(),
                ..ClusterRequest::named(&format!("ungrounded-v1-{index}-{count}"))
            };
            let arn = emulator.create_ok(&request).await;

            let described = emulator.describe(&arn).await;
            assert_eq!(
                described.at("/clusterInfo/brokerNodeGroupInfo/clientSubnets"),
                &json!(subnets),
                "case {case:?}: expected the cluster to report the subnets it was placed in, \
                 got {}",
                described.body
            );
        }
    }
}

/// `CreateClusterV2` does the same for both of the cluster kinds it creates.
#[tokio::test]
async fn a_v2_create_of_either_kind_keeps_the_subnets_it_was_given() {
    let emulator = Emulator::new();

    for (index, (form, subnet)) in UNRESOLVED_SUBNETS.iter().enumerate() {
        let other = UNRESOLVED_SUBNETS[1 - index].1;

        let case = format!("a provisioned v2 create, {form}");
        let name = format!("ungrounded-v2-{index}");
        let created = emulator
            .create_cluster_v2(json!({
                "clusterName": name,
                "provisioned": {
                    "kafkaVersion": "3.6.0",
                    "numberOfBrokerNodes": 2,
                    "brokerNodeGroupInfo": {
                        "instanceType": "kafka.m5.large",
                        "clientSubnets": [subnet, other],
                    },
                },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {} with {}",
            created.status, created.body
        );
        let arn = created
            .at("/clusterArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no clusterArn in {}", created.body))
            .to_string();
        let described = emulator.describe_v2(&arn).await;
        assert_eq!(
            described.at("/clusterInfo/provisioned/brokerNodeGroupInfo/clientSubnets"),
            &json!([subnet, other]),
            "case {case:?}: expected the cluster to report the subnets it was placed in, got {}",
            described.body
        );

        // The serverless body names its subnets per VPC configuration, so this row names
        // two configurations of one subnet each.
        let case = format!("a serverless v2 create, {form}");
        let name = format!("ungrounded-serverless-{index}");
        let arn = emulator
            .create_serverless_ok(&name, &[vec![*subnet], vec![other]], None)
            .await;
        let described = emulator.describe_v2(&arn).await;
        let reported: Vec<Value> = described
            .at("/clusterInfo/serverless/vpcConfigs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported
                .iter()
                .map(|config| config.get("subnetIds").cloned().unwrap_or(Value::Null))
                .collect::<Vec<Value>>(),
            vec![json!([subnet]), json!([other])],
            "case {case:?}: expected the cluster to report the subnets of each VPC \
             configuration, got {}",
            described.body
        );
    }
}

/// How many subnets one create may name as a placement — read off the service rather than
/// written out again, so this suite cannot drift from the ceiling the handlers apply.
const MAX_PLACEMENT_SUBNETS: usize =
    service_com_amazonaws_kafka_kafka::state::network::MAX_PLACEMENT_SUBNETS;

/// Subnet ids in Amazon EC2's 8-hex form, as many as [`MAX_PLACEMENT_SUBNETS`] allows: the
/// placement a target cluster is created with for the ceiling rows below, so a VPC
/// connection naming the ceiling spans as many client subnets as its cluster does.
const CEILING_SUBNETS: [&str; 16] = [
    "subnet-000000c0",
    "subnet-000000c1",
    "subnet-000000c2",
    "subnet-000000c3",
    "subnet-000000c4",
    "subnet-000000c5",
    "subnet-000000c6",
    "subnet-000000c7",
    "subnet-000000c8",
    "subnet-000000c9",
    "subnet-000000ca",
    "subnet-000000cb",
    "subnet-000000cc",
    "subnet-000000cd",
    "subnet-000000ce",
    "subnet-000000cf",
];

/// The placement list is bounded at both ends of the ceiling: a create naming exactly
/// [`MAX_PLACEMENT_SUBNETS`] subnets is accepted and reports them all, and one naming a
/// single subnet more is refused `BadRequestException` naming the member at fault, whether
/// it is the v1 create, the v2 provisioned create or a VPC connection.
#[tokio::test]
async fn a_create_may_name_at_most_the_placement_ceiling_of_subnets() {
    let emulator = Emulator::new();
    assert_eq!(
        CEILING_SUBNETS.len(),
        MAX_PLACEMENT_SUBNETS,
        "the target cluster must be placed in exactly as many subnets as the ceiling allows"
    );
    // The target is placed in as many subnets as the ceiling row names, because a VPC
    // connection spans as many client subnets as the cluster it reaches.
    let cluster = emulator
        .create_ok(&ClusterRequest {
            subnets: CEILING_SUBNETS.to_vec(),
            ..ClusterRequest::vpc_connection_target("ceiling-connection-target")
        })
        .await;
    sleep_past_activation();

    // Distinct ids in Amazon EC2's 8-hex form, as many as the row needs.
    let ids = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("subnet-{index:08x}")).collect()
    };
    // (case, the count, whether the create must be accepted)
    for (case, count, accepted) in [
        ("the ceiling itself", MAX_PLACEMENT_SUBNETS, true),
        ("one past the ceiling", MAX_PLACEMENT_SUBNETS + 1, false),
    ] {
        let subnets = ids(count);
        // (the operation, its URI, the body it carries, the member a refusal names)
        let requests: Vec<(&str, &str, Value, &str)> = vec![
            (
                "CreateCluster",
                "/v1/clusters",
                json!({
                    "clusterName": format!("ceiling-v1-{count}"),
                    "kafkaVersion": "3.6.0",
                    "numberOfBrokerNodes": 2,
                    "brokerNodeGroupInfo": {
                        "instanceType": "kafka.m5.large",
                        "clientSubnets": subnets,
                    },
                }),
                "brokerNodeGroupInfo",
            ),
            (
                "CreateClusterV2",
                "/api/v2/clusters",
                json!({
                    "clusterName": format!("ceiling-v2-{count}"),
                    "provisioned": {
                        "kafkaVersion": "3.6.0",
                        "numberOfBrokerNodes": 2,
                        "brokerNodeGroupInfo": {
                            "instanceType": "kafka.m5.large",
                            "clientSubnets": subnets,
                        },
                    },
                }),
                "brokerNodeGroupInfo",
            ),
            (
                "CreateVpcConnection",
                "/v1/vpc-connection",
                json!({
                    "targetClusterArn": cluster,
                    "authentication": "SASL_IAM",
                    "vpcId": "vpc-0a1b2c3d",
                    "clientSubnets": subnets,
                    "securityGroups": ["sg-0a1b2c3d"],
                }),
                "ClientSubnets",
            ),
        ];

        for (operation, uri, body, parameter) in requests {
            let case = format!("{operation} with {case}");
            let reply = emulator.call("POST", uri, Some(body)).await;
            if accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the create to be accepted, got {} with {}",
                    reply.status, reply.body
                );
                continue;
            }
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {case:?}: expected a 400 BadRequestException, got {} with {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(parameter),
                "case {case:?}: expected the refusal to name {parameter} at fault, got {}",
                reply.body
            );
        }
    }
}

/// `CreateVpcConnection` does the same for the subnets its connection is reached from.
#[tokio::test]
async fn a_vpc_connection_keeps_the_subnets_it_was_given() {
    let emulator = Emulator::new();
    // Two subnets, because a connection spans as many client subnets as the cluster it
    // reaches: the rows differ in the FORM of the ids and in which form comes first.
    let cluster = emulator
        .create_ok(&ClusterRequest {
            subnets: vec![UNRESOLVED_SUBNETS[0].1, UNRESOLVED_SUBNETS[1].1],
            ..ClusterRequest::vpc_connection_target("ungrounded-connection-target")
        })
        .await;
    sleep_past_activation();

    for (index, (form, subnet)) in UNRESOLVED_SUBNETS.iter().enumerate() {
        let subnets = vec![*subnet, UNRESOLVED_SUBNETS[1 - index].1];
        let case = format!("a VPC connection reached from two subnets, {form} first");
        let request = ConnectionRequest {
            client_subnets: subnets.clone(),
            ..ConnectionRequest::to(&cluster)
        };
        let arn = emulator.create_vpc_connection_ok(&request).await;

        let described = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            described.at("/subnets"),
            &json!(subnets),
            "case {case:?}: expected the connection to report the subnets it was reached \
             from, got {}",
            described.body
        );
    }
}
