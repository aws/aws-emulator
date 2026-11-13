// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDomainNodes`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test is not one recorded call: `DescribeDomainNodes` reports one
//! node per node the stored domain's `ClusterConfig` provisions — a data node per
//! `InstanceCount`, a dedicated master node per `DedicatedMasterCount` while
//! `DedicatedMasterEnabled` is on and a warm node per `WarmCount` while
//! `WarmEnabled` is — each carrying the instance type of the member that defines its
//! role, the Availability Zone the domain's zone awareness places it in, the storage
//! the domain's `EBSOptions` give it, and whether it is serving or in standby —
//! while a domain whose nodes are not up, because it still has work in flight, has
//! no census to read at all and the report is refused.
//!
//! So the table below spans the configurations the rule generalises over: the three
//! roles alone and together, one zone and the two published values of
//! `ZoneAwarenessConfig$AvailabilityZoneCount` plus one on each side of them, an even
//! and an uneven spread, a standby zone, EBS on and off, a count past the emulator's
//! census bound, counts the record carries no value for, and a domain with no cluster
//! configuration at all. Each row is read in both provisioning regimes, and the
//! suite then pins the invariants: the census follows an update of the record, two
//! reads report the same nodes and change nothing, the zones agree with
//! `DescribeDomainHealth`, and a name that names no domain is not found.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::NODE_INFORMATION_UNAVAILABLE;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// The Region the test requests are signed for — the one the domains' Availability
/// Zones are named from.
const REGION: &str = "us-east-1";

/// The letters the emulator names a Region's Availability Zones from, in order
/// (`us-east-1` + `a`); no case here configures more zones than this.
const ZONE_LETTERS: [char; 3] = ['a', 'b', 'c'];

/// Every value `NodeType` models
/// (smithy_query query="shape" shape="NodeType"). A reported node is one of these
/// roles and nothing else.
const NODE_TYPE_VALUES: &[&str] = &["Data", "Ultrawarm", "Master", "Warm"];

/// Every value `NodeStatus` models
/// (smithy_query query="shape" shape="NodeStatus").
const NODE_STATUS_VALUES: &[&str] = &["Active", "StandBy", "NotAvailable"];

/// The storage types `StorageTypeName` documents — "instance: built-in storage
/// available for the instance" and "ebs: Elastic Block Storage"
/// (smithy_query query="doc" shape="StorageTypeName").
const STORAGE_TYPE_VALUES: &[&str] = &["instance", "ebs"];

/// `NodeId`'s published length bounds
/// (smithy_query query="shape" shape="NodeId").
const NODE_ID_LENGTH: std::ops::RangeInclusive<usize> = 10..=40;

/// `NodeStatus`'s `NotAvailable`.
const NOT_AVAILABLE: &str = "NotAvailable";

/// `NodeStatus`'s `Active`.
const ACTIVE: &str = "Active";

/// `NodeStatus`'s `StandBy`.
const STANDBY: &str = "StandBy";

/// The most nodes of one role the emulator reports — its own bound on the answer, so
/// a count a caller sends cannot size the response. One row drives a count past it.
const CENSUS_LIMIT: i64 = 80;

/// Where a role's nodes are expected to sit.
enum Placement {
    /// How many of them sit in each of the domain's Availability Zones, in zone
    /// order; the last [`Case::standby_zones`] of those zones are the standby ones.
    Zones(&'static [i64]),
    /// How many of them the configuration names no Availability Zone for.
    Unzoned(i64),
}

/// One role's expected census: the `NodeType` it is reported under, where its nodes
/// sit, the instance type they run and the storage they are on.
struct ExpectedRole {
    node_type: &'static str,
    placement: Placement,
    instance_type: Option<&'static str>,
    storage_type: &'static str,
    storage_volume_type: Option<&'static str>,
    storage_size: Option<&'static str>,
}

/// One case: a domain created with `cluster` and `ebs`, and the nodes it must report.
struct Case {
    case: &'static str,
    domain: &'static str,
    /// The `ClusterConfig` to create with, or `Value::Null` for none at all.
    cluster: Value,
    /// The `EBSOptions` to create with, or `Value::Null` for none at all.
    ebs: Value,
    /// How many of the domain's zones its configuration holds in standby.
    standby_zones: usize,
    roles: Vec<ExpectedRole>,
}

/// One node as the response reports it, flattened so a whole census compares in one
/// assertion. `NodeId` is checked on its own (it is derived, not configured).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Node {
    node_type: String,
    availability_zone: Option<String>,
    instance_type: Option<String>,
    node_status: String,
    storage_type: String,
    storage_volume_type: Option<String>,
    storage_size: Option<String>,
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/nodes`.
async fn describe_nodes(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/nodes"),
            None,
        )
        .await
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/health`.
async fn describe_health(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/health"),
            None,
        )
        .await
}

/// The `DomainNodesStatusList` of an answered call.
fn node_list(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeDomainNodes to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainNodesStatusList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainNodesStatusList, got {:?}",
                answer.body
            )
        })
        .clone()
}

fn optional(node: &Value, member: &str) -> Option<String> {
    node[member].as_str().map(str::to_owned)
}

fn required(node: &Value, member: &str, case: &str) -> String {
    optional(node, member)
        .unwrap_or_else(|| panic!("case {case}: expected a {member} on the node {node:?}"))
}

/// The reported nodes as [`Node`]s, in a canonical order so one assertion holds the
/// whole census (the response's own order is not part of the rule).
fn reported(nodes: &[Value], case: &str) -> Vec<Node> {
    let mut reported: Vec<Node> = nodes
        .iter()
        .map(|node| Node {
            node_type: required(node, "NodeType", case),
            availability_zone: optional(node, "AvailabilityZone"),
            instance_type: optional(node, "InstanceType"),
            node_status: required(node, "NodeStatus", case),
            storage_type: required(node, "StorageType", case),
            storage_volume_type: optional(node, "StorageVolumeType"),
            storage_size: optional(node, "StorageSize"),
        })
        .collect();
    reported.sort();
    reported
}

/// The census a case must report, in the same canonical order as [`reported`].
///
/// Every node is reported where it stands: a node of a zone the configuration names
/// is active, or in standby where that zone is one the domain holds in standby, and
/// a node whose zone the configuration does not name has no standby state to report.
fn expected(case: &Case) -> Vec<Node> {
    let mut expected = Vec::new();
    for role in &case.roles {
        let mut push = |zone: Option<usize>, status: &str| {
            expected.push(Node {
                node_type: role.node_type.to_string(),
                availability_zone: zone.map(|zone| format!("{REGION}{}", ZONE_LETTERS[zone])),
                instance_type: role.instance_type.map(str::to_owned),
                node_status: status.to_string(),
                storage_type: role.storage_type.to_string(),
                storage_volume_type: role.storage_volume_type.map(str::to_owned),
                storage_size: role.storage_size.map(str::to_owned),
            });
        };
        match role.placement {
            Placement::Zones(per_zone) => {
                let serving = per_zone.len().saturating_sub(case.standby_zones);
                for (zone, count) in per_zone.iter().enumerate() {
                    let status = if zone < serving { ACTIVE } else { STANDBY };
                    for _ in 0..*count {
                        push(Some(zone), status);
                    }
                }
            }
            Placement::Unzoned(count) => {
                for _ in 0..count {
                    push(None, NOT_AVAILABLE);
                }
            }
        }
    }
    expected.sort();
    expected
}

/// Assert every reported node carries modelled values: a `NodeType`, `NodeStatus` and
/// `StorageType` the model names, and a unique `NodeId` inside its length bounds.
fn assert_modelled(case: &str, nodes: &[Value]) {
    let mut ids = BTreeSet::new();
    for node in nodes {
        for (member, catalog) in [
            ("NodeType", NODE_TYPE_VALUES),
            ("NodeStatus", NODE_STATUS_VALUES),
            ("StorageType", STORAGE_TYPE_VALUES),
        ] {
            let value = required(node, member, case);
            assert!(
                catalog.contains(&value.as_str()),
                "case {case}: expected {member} to be one of {catalog:?}, got {value:?}"
            );
        }
        let id = required(node, "NodeId", case);
        assert!(
            NODE_ID_LENGTH.contains(&id.len()),
            "case {case}: expected a NodeId of {NODE_ID_LENGTH:?} characters, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected every NodeId to be unique, got {id:?} twice"
        );
    }
}

/// `CreateDomain` for a case's domain, asserting the create was accepted.
async fn create(emulator: &Emulator, case: &Case) {
    let mut body = json!({ "DomainName": case.domain });
    if !case.cluster.is_null() {
        body["ClusterConfig"] = case.cluster.clone();
    }
    if !case.ebs.is_null() {
        body["EBSOptions"] = case.ebs.clone();
    }
    let answer = emulator.create(body).await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected CreateDomain({:?}) to answer 200, got {} with {:?}",
        case.case, case.domain, answer.status, answer.body
    );
}

/// A data-node role on EBS storage.
fn data_on_ebs(
    instance_type: &'static str,
    placement: Placement,
    volume_type: &'static str,
    size: &'static str,
) -> ExpectedRole {
    ExpectedRole {
        node_type: "Data",
        placement,
        instance_type: Some(instance_type),
        storage_type: "ebs",
        storage_volume_type: Some(volume_type),
        storage_size: Some(size),
    }
}

/// A role on its instance's own storage: no EBS volume, so no volume type or size.
fn on_instance_storage(
    node_type: &'static str,
    instance_type: &'static str,
    placement: Placement,
) -> ExpectedRole {
    ExpectedRole {
        node_type,
        placement,
        instance_type: Some(instance_type),
        storage_type: "instance",
        storage_volume_type: None,
        storage_size: None,
    }
}

/// The table: one row per cluster configuration the rule has to hold for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a single-zone cluster on EBS",
            domain: "nodes-single-zone",
            cluster: json!({ "InstanceType": "r6g.large.search", "InstanceCount": 2 }),
            ebs: json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 }),
            standby_zones: 0,
            roles: vec![data_on_ebs(
                "r6g.large.search",
                Placement::Zones(&[2]),
                "gp3",
                "20",
            )],
        },
        Case {
            // Two zones: the lower published value of AvailabilityZoneCount.
            case: "two zones, an even spread",
            domain: "nodes-two-zones",
            cluster: json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 4,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
            }),
            ebs: json!({ "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 10 }),
            standby_zones: 0,
            roles: vec![data_on_ebs(
                "m5.large.search",
                Placement::Zones(&[2, 2]),
                "gp2",
                "10",
            )],
        },
        Case {
            // Three zones: the upper published value, with a count that does not
            // divide evenly, so the first zones carry the remainder.
            case: "three zones, an uneven spread, no EBS options",
            domain: "nodes-three-zones",
            cluster: json!({
                "InstanceType": "c5.xlarge.search",
                "InstanceCount": 5,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![on_instance_storage(
                "Data",
                "c5.xlarge.search",
                Placement::Zones(&[2, 2, 1]),
            )],
        },
        Case {
            case: "three zones with a standby zone",
            domain: "nodes-standby",
            cluster: json!({
                "InstanceType": "or1.medium.search",
                "InstanceCount": 6,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
                "MultiAZWithStandbyEnabled": true,
            }),
            ebs: json!({ "EBSEnabled": true, "VolumeType": "io1", "VolumeSize": 35 }),
            standby_zones: 1,
            roles: vec![data_on_ebs(
                "or1.medium.search",
                Placement::Zones(&[2, 2, 2]),
                "io1",
                "35",
            )],
        },
        Case {
            case: "dedicated master nodes beside the data nodes",
            domain: "nodes-dedicated-master",
            cluster: json!({
                "InstanceType": "r6g.large.search",
                "InstanceCount": 3,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            }),
            ebs: json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 100 }),
            standby_zones: 0,
            roles: vec![
                data_on_ebs(
                    "r6g.large.search",
                    Placement::Zones(&[1, 1, 1]),
                    "gp3",
                    "100",
                ),
                on_instance_storage(
                    "Master",
                    "c5.large.search",
                    Placement::Zones(&[1, 1, 1]),
                ),
            ],
        },
        Case {
            case: "warm nodes beside the data nodes",
            domain: "nodes-warm",
            cluster: json!({
                "InstanceType": "m6g.large.search",
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
                "WarmType": "ultrawarm1.medium.search",
                "WarmCount": 2,
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![
                on_instance_storage("Data", "m6g.large.search", Placement::Zones(&[2])),
                on_instance_storage("Master", "c5.large.search", Placement::Zones(&[3])),
                on_instance_storage(
                    "Ultrawarm",
                    "ultrawarm1.medium.search",
                    Placement::Zones(&[2]),
                ),
            ],
        },
        Case {
            case: "EBS turned off",
            domain: "nodes-no-ebs",
            cluster: json!({ "InstanceType": "t3.small.search", "InstanceCount": 1 }),
            ebs: json!({ "EBSEnabled": false }),
            standby_zones: 0,
            roles: vec![on_instance_storage(
                "Data",
                "t3.small.search",
                Placement::Zones(&[1]),
            )],
        },
        Case {
            // Nothing configures a node, so there is no node to report — not an
            // invented default.
            case: "no cluster configuration at all",
            domain: "nodes-no-cluster",
            cluster: Value::Null,
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![],
        },
        Case {
            // One past the upper published value of AvailabilityZoneCount: the
            // configuration names no supported number of zones, so no node has a zone.
            case: "zone awareness with a zone count above the published values",
            domain: "nodes-zones-above",
            cluster: json!({
                "InstanceType": "r5.large.search",
                "InstanceCount": 4,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 4 },
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![ExpectedRole {
                node_type: "Data",
                placement: Placement::Unzoned(4),
                instance_type: Some("r5.large.search"),
                storage_type: "instance",
                storage_volume_type: None,
                storage_size: None,
            }],
        },
        Case {
            // One below the lower published value.
            case: "zone awareness with a zone count below the published values",
            domain: "nodes-zones-below",
            cluster: json!({
                "InstanceType": "r5.xlarge.search",
                "InstanceCount": 2,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 1 },
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![ExpectedRole {
                node_type: "Data",
                placement: Placement::Unzoned(2),
                instance_type: Some("r5.xlarge.search"),
                storage_type: "instance",
                storage_volume_type: None,
                storage_size: None,
            }],
        },
        Case {
            // A count a caller sends may not size the answer past the census bound.
            case: "a node count past the census bound",
            domain: "nodes-past-bound",
            cluster: json!({ "InstanceType": "t3.medium.search", "InstanceCount": 200 }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![on_instance_storage(
                "Data",
                "t3.medium.search",
                Placement::Zones(&[CENSUS_LIMIT]),
            )],
        },
        Case {
            // The master and warm members are set but their roles are not turned on,
            // so the cluster has no node of either role.
            case: "master and warm members set without their roles turned on",
            domain: "nodes-roles-off",
            cluster: json!({
                "InstanceType": "t3.large.search",
                "InstanceCount": 2,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
                "WarmType": "ultrawarm1.large.search",
                "WarmCount": 2,
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![on_instance_storage(
                "Data",
                "t3.large.search",
                Placement::Zones(&[2]),
            )],
        },
        Case {
            // Warm storage is on but the record carries no count for it, so how many
            // warm nodes there are is unknown — and an unknown node is not a node. The
            // hot tier runs on a family that offers warm storage, which is what a cluster
            // with the warm tier on has to run on.
            case: "warm storage on with no warm count",
            domain: "nodes-warm-no-count",
            cluster: json!({
                "InstanceType": "m6g.large.search",
                "InstanceCount": 1,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
                "WarmType": "ultrawarm1.large.search",
            }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![
                on_instance_storage("Data", "m6g.large.search", Placement::Zones(&[1])),
                on_instance_storage("Master", "c5.large.search", Placement::Zones(&[3])),
            ],
        },
        Case {
            // A data-node count of zero is a count, so the cluster has no data node.
            case: "a data node count of zero",
            domain: "nodes-zero-count",
            cluster: json!({ "InstanceType": "t3.small.search", "InstanceCount": 0 }),
            ebs: Value::Null,
            standby_zones: 0,
            roles: vec![],
        },
    ]
}

#[tokio::test]
async fn every_node_the_cluster_configuration_provisions_is_reported_from_the_stored_record() {
    let emulator = Emulator::new();
    let cases = cases();

    // Each domain is read straight after its OWN create, while the provisioning the
    // clock owes it is still outstanding: its nodes do not exist yet, so there is no
    // census to read and the report is refused rather than answered.
    for case in &cases {
        create(&emulator, case).await;
        let answer = describe_nodes(&emulator, case.domain).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (424, "DependencyFailureException"),
            "case {} while creating: expected the modelled dependency failure, got {} with {:?}",
            case.case,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(NODE_INFORMATION_UNAVAILABLE),
            "case {} while creating: expected the service's wording for node information that \
             cannot be fetched, got {:?}",
            case.case,
            answer.body
        );
    }

    // Once the provisioning has fallen due, the nodes are reported where they stand.
    tokio::time::sleep(SETTLE_WAIT).await;
    for case in &cases {
        let answer = describe_nodes(&emulator, case.domain).await;
        let list = node_list(&answer, case.case);
        assert_modelled(case.case, &list);
        let actual = reported(&list, case.case);
        let want = expected(case);
        assert_eq!(
            actual, want,
            "case {} once up: expected the nodes {want:?}, got {actual:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_node_census_follows_the_record_through_an_update() {
    let case = "an updated ClusterConfig";
    let emulator = Emulator::new();
    let domain = "nodes-updated";
    let answer = emulator
        .create(json!({
            "DomainName": domain,
            "ClusterConfig": { "InstanceType": "r6g.large.search", "InstanceCount": 2 },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected CreateDomain to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let before = reported(
        &node_list(&describe_nodes(&emulator, domain).await, case),
        case,
    );
    let want_before = vec![Node {
        node_type: "Data".to_string(),
        availability_zone: Some(format!("{REGION}a")),
        instance_type: Some("r6g.large.search".to_string()),
        node_status: ACTIVE.to_string(),
        storage_type: "instance".to_string(),
        storage_volume_type: None,
        storage_size: None,
    }; 2];
    assert_eq!(
        before, want_before,
        "case {case}: expected the created nodes {want_before:?}, got {before:?}"
    );

    let answer = emulator
        .update_config(
            domain,
            json!({
                "ClusterConfig": {
                    "InstanceType": "r6g.xlarge.search",
                    "InstanceCount": 3,
                    "ZoneAwarenessEnabled": true,
                    "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
                },
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 50 },
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected UpdateDomainConfig to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let after = reported(
        &node_list(&describe_nodes(&emulator, domain).await, case),
        case,
    );
    let node = |zone: char| Node {
        node_type: "Data".to_string(),
        availability_zone: Some(format!("{REGION}{zone}")),
        instance_type: Some("r6g.xlarge.search".to_string()),
        node_status: ACTIVE.to_string(),
        storage_type: "ebs".to_string(),
        storage_volume_type: Some("gp3".to_string()),
        storage_size: Some("50".to_string()),
    };
    let mut want_after = vec![node('a'), node('a'), node('b')];
    want_after.sort();
    assert_eq!(
        after, want_after,
        "case {case}: expected the updated nodes {want_after:?}, got {after:?}"
    );
}

#[tokio::test]
async fn reading_the_nodes_reports_the_same_nodes_every_time_and_changes_nothing() {
    let case = "two reads of one domain";
    let emulator = Emulator::new();
    let domain = "nodes-stable";
    let answer = emulator
        .create(json!({
            "DomainName": domain,
            "ClusterConfig": {
                "InstanceType": "m5.large.search",
                "InstanceCount": 3,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "c5.large.search",
                "DedicatedMasterCount": 3,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected CreateDomain to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let status_before = emulator.describe(domain).await.body;
    let first = describe_nodes(&emulator, domain).await.body;
    let second = describe_nodes(&emulator, domain).await.body;
    let status_after = emulator.describe(domain).await.body;

    assert_eq!(
        first, second,
        "case {case}: expected the second read to report the same nodes as the first, \
         got {second:?} after {first:?}"
    );
    assert_eq!(
        status_before, status_after,
        "case {case}: expected reading the nodes to leave the domain as it was, \
         got {status_after:?} after {status_before:?}"
    );
}

#[tokio::test]
async fn the_nodes_and_the_health_of_one_domain_agree_on_its_zones() {
    let emulator = Emulator::new();
    let cases = cases();
    for case in &cases {
        create(&emulator, case).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for case in &cases {
        let nodes = node_list(&describe_nodes(&emulator, case.domain).await, case.case);
        let health = describe_health(&emulator, case.domain).await;
        assert_eq!(
            health.status, 200,
            "case {}: expected DescribeDomainHealth to answer 200, got {} with {:?}",
            case.case, health.status, health.body
        );

        // The per-zone data node counts health reports, zone by zone.
        let zones = health.body["EnvironmentInformation"]
            .as_array()
            .and_then(|environments| environments.first())
            .and_then(|environment| environment["AvailabilityZoneInformation"].as_array())
            .cloned()
            .unwrap_or_default();
        // The census bound is the emulator's bound on this ANSWER, not on the record:
        // health reports a count rather than a list, so a domain configured with more
        // nodes than the bound is the one case where the two legitimately differ.
        let configured_nodes = health.body["DataNodeCount"]
            .as_str()
            .unwrap_or_default()
            .parse::<i64>()
            .unwrap_or_default();
        if configured_nodes > CENSUS_LIMIT {
            continue;
        }
        for (index, zone) in zones.iter().enumerate() {
            let name = zone["AvailabilityZoneName"].as_str().map(str::to_owned);
            let configured = zone["ConfiguredDataNodeCount"].as_str().unwrap_or_default();
            let reported_here = nodes
                .iter()
                .filter(|node| node["NodeType"].as_str() == Some("Data"))
                .filter(|node| node["AvailabilityZone"].as_str().map(str::to_owned) == name)
                .count();
            // Where health reports a number of configured data nodes for a zone, the
            // node list holds exactly that many data nodes in it; where health cannot
            // report one (the record carries no count), there is no node to report
            // either.
            let want = if configured == NOT_AVAILABLE {
                0
            } else {
                configured.parse::<usize>().unwrap_or_else(|_| {
                    panic!(
                        "case {}: expected zone {index} ({name:?}) to report a count, got \
                         {configured:?}",
                        case.case
                    )
                })
            };
            assert_eq!(
                reported_here, want,
                "case {}: expected zone {index} ({name:?}) to hold the {want} data nodes \
                 health reports as {configured:?}, got {reported_here} from the node list",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_name_that_names_no_domain_is_the_generic_envelope() {
    let emulator = Emulator::new();
    let domain = "nodes-deleted";
    let answer = emulator
        .create(json!({
            "DomainName": domain,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case a created domain: expected CreateDomain to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{domain}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case a created domain: expected DeleteDomain to answer 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    for (case, name) in [
        ("a name no domain was ever created with", "nodes-absent"),
        ("a deleted domain", domain),
    ] {
        let answer = describe_nodes(&emulator, name).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected DescribeDomainNodes({name:?}) to answer 400, got {} with {:?}",
            answer.status, answer.body
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "BaseException",
            "case {case}: expected BaseException, got {code:?} with {:?}",
            answer.body
        );
    }
}
