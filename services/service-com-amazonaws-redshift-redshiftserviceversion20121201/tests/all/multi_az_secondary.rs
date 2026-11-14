// SPDX-License-Identifier: CC0-1.0

//! A Multi-AZ cluster's SECOND compute unit, and the failover that moves its
//! primary — end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: a cluster deployed in two Availability Zones
//! reports BOTH halves of that deployment — `MultiAZ` says it is deployed in two
//! zones, and `MultiAZSecondary` says where the second compute unit is and which
//! nodes it runs — while a cluster deployed in one zone reports no second unit at
//! all. Concretely:
//!
//! * `MultiAZ` is `Enabled` exactly when the write asked for a Multi-AZ
//!   deployment, and `Disabled` otherwise
//!   (`smithy_query(query="doc", shape="Cluster", member="MultiAZ")` — "a boolean
//!   value that, if true, indicates that the cluster is deployed in two
//!   Availability Zones");
//! * a cluster reporting `Enabled` carries `MultiAZSecondary`, whose
//!   `AvailabilityZone` is "the name of the Availability Zone in which the
//!   secondary compute unit of the cluster is located" — a zone of the cluster's
//!   own Region that is NOT the primary's — and whose `ClusterNodes` are "the
//!   nodes in the secondary compute unit", one per node of the primary unit;
//! * a cluster reporting `Disabled` carries no `MultiAZSecondary` member;
//! * the rule holds whatever PATH set the member: the create, a later modify that
//!   turns the deployment on or off, and the read that reports the stored record;
//! * `FailoverPrimaryCompute` "fails over the primary compute unit of the
//!   specified Multi-AZ cluster to another Availability Zone", so the two zones
//!   SWAP — the cluster's `AvailabilityZone` becomes the zone its secondary unit
//!   was in and the secondary unit takes the zone the primary was in — and a
//!   cluster that is not Multi-AZ is refused `InvalidClusterState` rather than
//!   failed over.
//!
//! The rows differ along what the rule generalises over: whether the deployment
//! was asked for on the create or by a later modify, whether the cluster is
//! single-node or multi-node (and so how many nodes each unit runs), whether the
//! write named the primary zone or left it to the service, and which Region the
//! cluster is in (a Region publishes its own number of zones, so the secondary is
//! chosen from a different set).

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Answer, Emulator, REGION};

/// The two values `Cluster$MultiAZ` takes, spelled out here so a change to the
/// reported TEXT fails a test rather than passing one computed from the same code
/// under test.
const ENABLED: &str = "Enabled";
const DISABLED: &str = "Disabled";

/// A second Region every zone-choice assertion is repeated in, so no case can
/// pass by accident on the one Region the harness signs for by default. AWS
/// publishes a different number of Availability Zones per Region, which is what
/// the secondary zone is chosen from.
const OTHER_REGION: &str = "eu-west-1";

/// The `<Cluster>` block Amazon Redshift reports for `identifier`, or a panic
/// naming the case: every row below reads the cluster back through the service's
/// own read rather than off the write's answer.
async fn described(emulator: &Emulator, case: &str, region: &str, identifier: &str) -> String {
    let answered = emulator
        .call_in_region(
            region,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the cluster to be readable, got {} with body {}",
        answered.status, answered.body
    );
    element_blocks(&answered.body, "Cluster")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeClusters must report a Cluster block"))
}

/// `xml` with every `<name …>…</name>` span removed, so a member of the OUTER
/// structure is read without a nested structure's same-named member standing in
/// for it: a cluster's `Endpoint` carries its VPC endpoint's network interfaces,
/// each with an `AvailabilityZone` of its own, and `MultiAZSecondary` carries the
/// secondary unit's zone and nodes.
fn without_block(xml: &str, name: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let close = format!("</{name}>");
    let mut rest = xml;
    while let Some(start) = rest.find(&format!("<{name}")) {
        let after = &rest[start..];
        let Some(end) = after.find(&close) else { break };
        out.push_str(&rest[..start]);
        rest = &after[end + close.len()..];
    }
    out.push_str(rest);
    out
}

/// How many nodes the compute unit `block` describes runs: one `<member>` of its
/// `ClusterNodes` list per node (`ClusterNodesList`'s member fixes no `@xmlName`,
/// so the entries carry the Smithy default element name).
fn nodes_in(block: &str) -> usize {
    element_blocks(block, "ClusterNodes")
        .into_iter()
        .next()
        .map_or(0, |nodes| element_blocks(&nodes, "member").len())
}

/// The secondary compute unit a `<Cluster>` block reports: its zone and how many
/// nodes it runs. `None` where the block carries no `MultiAZSecondary` at all.
fn secondary_unit(cluster: &str) -> Option<(String, usize)> {
    let block = element_blocks(cluster, "MultiAZSecondary")
        .into_iter()
        .next()?;
    let zone = all_text(&block, "AvailabilityZone")
        .into_iter()
        .next()
        .unwrap_or_default();
    Some((zone, nodes_in(&block)))
}

/// The zone, node count and `MultiAZ` value a `<Cluster>` block reports for its
/// PRIMARY compute unit.
fn primary_unit(cluster: &str) -> (String, usize, String) {
    let outer = without_block(&without_block(cluster, "Endpoint"), "MultiAZSecondary");
    (
        all_text(&outer, "AvailabilityZone")
            .into_iter()
            .next()
            .unwrap_or_default(),
        nodes_in(&outer),
        all_text(&outer, "MultiAZ")
            .into_iter()
            .next()
            .unwrap_or_default(),
    )
}

/// One row of the deployment table.
struct Row {
    case: &'static str,
    identifier: &'static str,
    /// The Region the cluster is created in.
    region: &'static str,
    /// What the CREATE asks for: `None` leaves the member out altogether.
    created_multi_az: Option<bool>,
    /// What a later `ModifyCluster` asks for, where the row makes one.
    modified_multi_az: Option<bool>,
    /// `("ClusterType", "NumberOfNodes")` the create declares, if any.
    shape: &'static [(&'static str, &'static str)],
    /// The Availability Zone the create names, or `None` to leave the choice to
    /// the service.
    availability_zone: Option<&'static str>,
    /// How many nodes each compute unit runs once the row's writes are done.
    nodes_per_unit: usize,
    /// Whether the cluster is deployed in two zones once they are done.
    multi_az: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a single-node cluster created Multi-AZ",
        identifier: "multi-az-single",
        region: REGION,
        created_multi_az: Some(true),
        modified_multi_az: None,
        shape: &[("ClusterType", "single-node")],
        availability_zone: None,
        nodes_per_unit: 1,
        multi_az: true,
    },
    Row {
        case: "a four-node cluster created Multi-AZ in a named zone",
        identifier: "multi-az-four-nodes",
        region: REGION,
        created_multi_az: Some(true),
        modified_multi_az: None,
        shape: &[("ClusterType", "multi-node"), ("NumberOfNodes", "4")],
        availability_zone: Some("us-west-2b"),
        // A multi-node cluster reports its leader node beside the four compute
        // nodes, and the secondary unit runs the same topology.
        nodes_per_unit: 5,
        multi_az: true,
    },
    Row {
        case: "a cluster created Multi-AZ in another Region",
        identifier: "multi-az-other-region",
        region: OTHER_REGION,
        created_multi_az: Some(true),
        modified_multi_az: None,
        shape: &[],
        availability_zone: None,
        nodes_per_unit: 1,
        multi_az: true,
    },
    Row {
        case: "a cluster created in a single zone",
        identifier: "single-az-cluster",
        region: REGION,
        created_multi_az: Some(false),
        modified_multi_az: None,
        shape: &[],
        availability_zone: None,
        nodes_per_unit: 1,
        multi_az: false,
    },
    Row {
        case: "a cluster whose create named no deployment at all",
        identifier: "default-az-cluster",
        region: REGION,
        created_multi_az: None,
        modified_multi_az: None,
        shape: &[],
        availability_zone: None,
        nodes_per_unit: 1,
        multi_az: false,
    },
    Row {
        case: "a single-zone cluster a modify deploys in two",
        identifier: "modified-into-multi-az",
        region: REGION,
        created_multi_az: Some(false),
        modified_multi_az: Some(true),
        shape: &[("ClusterType", "multi-node"), ("NumberOfNodes", "2")],
        availability_zone: None,
        nodes_per_unit: 3,
        multi_az: true,
    },
    Row {
        case: "a Multi-AZ cluster a modify brings back to one zone",
        identifier: "modified-out-of-multi-az",
        region: REGION,
        created_multi_az: Some(true),
        modified_multi_az: Some(false),
        shape: &[],
        availability_zone: None,
        nodes_per_unit: 1,
        multi_az: false,
    },
];

/// Create the cluster `row` declares, through the Region it names.
async fn create(emulator: &Emulator, row: &Row) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", row.identifier),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
    ];
    let multi_az = row.created_multi_az.map(|on| on.to_string());
    if let Some(multi_az) = multi_az.as_deref() {
        params.push(("MultiAZ", multi_az));
    }
    if let Some(zone) = row.availability_zone {
        params.push(("AvailabilityZone", zone));
    }
    params.extend_from_slice(row.shape);
    emulator
        .call_in_region(row.region, "CreateCluster", &params)
        .await
}

#[tokio::test]
async fn a_cluster_deployed_in_two_zones_reports_its_secondary_compute_unit() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let created = create(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateCluster, got {} with body {}",
            created.status, created.body
        );
        if let Some(wanted) = row.modified_multi_az {
            let modified = emulator
                .call_in_region(
                    row.region,
                    "ModifyCluster",
                    &[
                        ("ClusterIdentifier", row.identifier),
                        ("MultiAZ", if wanted { "true" } else { "false" }),
                    ],
                )
                .await;
            assert_eq!(
                modified.status, 200,
                "case {case:?}: expected HTTP 200 from ModifyCluster, got {} with body {}",
                modified.status, modified.body
            );
        }

        let cluster = described(&emulator, case, row.region, row.identifier).await;
        let (primary_zone, primary_nodes, reported_multi_az) = primary_unit(&cluster);
        let expected_multi_az = if row.multi_az { ENABLED } else { DISABLED };
        assert_eq!(
            reported_multi_az, expected_multi_az,
            "case {case:?}: expected MultiAZ {expected_multi_az:?}, got \
             {reported_multi_az:?} in {cluster}"
        );
        assert_eq!(
            primary_nodes, row.nodes_per_unit,
            "case {case:?}: expected the primary unit to run {} nodes, got {primary_nodes}",
            row.nodes_per_unit
        );

        let secondary = secondary_unit(&cluster);
        if !row.multi_az {
            assert!(
                secondary.is_none(),
                "case {case:?}: a cluster deployed in one zone reports no secondary \
                 compute unit, got {secondary:?}"
            );
            continue;
        }
        let Some((secondary_zone, secondary_nodes)) = secondary else {
            panic!(
                "case {case:?}: a cluster reporting MultiAZ {ENABLED} must carry \
                 MultiAZSecondary, got {cluster}"
            );
        };
        assert_ne!(
            secondary_zone, primary_zone,
            "case {case:?}: the secondary compute unit must be in another zone than the \
             primary one ({primary_zone})"
        );
        assert!(
            secondary_zone.starts_with(row.region),
            "case {case:?}: expected the secondary zone to be one of {}'s, got \
             {secondary_zone:?}",
            row.region
        );
        assert_eq!(
            secondary_nodes, row.nodes_per_unit,
            "case {case:?}: expected the secondary unit to run the primary's {} nodes, got \
             {secondary_nodes}",
            row.nodes_per_unit
        );
    }
}

#[tokio::test]
async fn the_reported_secondary_unit_is_stable_across_reads() {
    let case = "two reads of one Multi-AZ cluster";
    let emulator = Emulator::new();
    let identifier = "multi-az-stable";
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("MultiAZ", "true"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateCluster, got {} with body {}",
        created.status, created.body
    );

    // The create's own answer and every later read describe ONE cluster, so they
    // report the same secondary unit: nothing here is drawn per call.
    let from_create = secondary_unit(
        &element_blocks(&created.body, "Cluster")
            .into_iter()
            .next()
            .unwrap_or_default(),
    );
    let first = secondary_unit(&described(&emulator, case, REGION, identifier).await);
    let second = secondary_unit(&described(&emulator, case, REGION, identifier).await);
    assert_eq!(
        (&from_create, &first),
        (&first, &second),
        "case {case:?}: expected the same secondary compute unit from the create and from \
         every read, got {from_create:?} then {first:?} then {second:?}"
    );
}

/// Cross the clock-owed transition a write started, so the next write sees an
/// `available` cluster — by ADVANCING this emulator's own clock across
/// [`state::PROVISIONING_SPAN`] rather than waiting for it, so no case of this
/// suite depends on how fast the host runs.
fn settle(emulator: &Emulator) {
    emulator.advance_clock(state::PROVISIONING_SPAN);
}

#[tokio::test]
async fn failing_over_a_multi_az_cluster_swaps_its_two_zones() {
    let case = "fail over a Multi-AZ cluster";
    let emulator = Emulator::new();
    let identifier = "failover-multi-az";
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
                ("MultiAZ", "true"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateCluster, got {} with body {}",
        created.status, created.body
    );
    settle(&emulator);

    let before = described(&emulator, case, REGION, identifier).await;
    let (primary_before, _, _) = primary_unit(&before);
    let (secondary_before, nodes_before) = secondary_unit(&before)
        .unwrap_or_else(|| panic!("case {case:?}: the cluster must report a secondary unit"));

    let answered = emulator
        .call(
            "FailoverPrimaryCompute",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from FailoverPrimaryCompute, got {} with body {}",
        answered.status, answered.body
    );
    // The failover is work the service does on the cluster, so the cluster
    // reports it in progress from the call that asked for it.
    assert_eq!(
        answered.text("ClusterStatus").as_deref(),
        Some("modifying"),
        "case {case:?}: expected the failover to put the cluster in modifying, got {}",
        answered.body
    );
    settle(&emulator);

    let after = described(&emulator, case, REGION, identifier).await;
    let (primary_after, _, multi_az_after) = primary_unit(&after);
    let (secondary_after, nodes_after) = secondary_unit(&after)
        .unwrap_or_else(|| panic!("case {case:?}: the cluster must still report a secondary unit"));
    assert_eq!(
        (primary_after.as_str(), secondary_after.as_str()),
        (secondary_before.as_str(), primary_before.as_str()),
        "case {case:?}: expected the primary and secondary zones to swap, got primary \
         {primary_after:?} and secondary {secondary_after:?} from primary {primary_before:?} \
         and secondary {secondary_before:?}"
    );
    assert_eq!(
        (multi_az_after.as_str(), nodes_after),
        (ENABLED, nodes_before),
        "case {case:?}: expected the cluster to stay Multi-AZ with the same topology, got \
         {multi_az_after:?} and {nodes_after} nodes per unit"
    );
    assert_eq!(
        all_text(&after, "ClusterStatus").first().map(String::as_str),
        Some("available"),
        "case {case:?}: expected the cluster to be available again after its span, got {after}"
    );
}

/// One row of the failover-refusal table: the case, what the cluster is, and the
/// status and error code the call is refused with.
struct Refusal {
    case: &'static str,
    identifier: &'static str,
    /// Whether the row creates the cluster at all.
    created: bool,
    /// What the create asks for, where the row names a deployment — `None` leaves
    /// the member out altogether.
    multi_az: Option<bool>,
    status: u16,
    code: &'static str,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a cluster deployed in a single zone",
        identifier: "failover-single-az",
        created: true,
        multi_az: Some(false),
        status: 400,
        code: "InvalidClusterState",
    },
    Refusal {
        case: "a cluster whose create named no deployment",
        identifier: "failover-default-az",
        created: true,
        multi_az: None,
        status: 400,
        code: "InvalidClusterState",
    },
    Refusal {
        case: "a cluster this account does not have",
        identifier: "failover-absent-cluster",
        created: false,
        multi_az: None,
        status: 404,
        code: "ClusterNotFound",
    },
];

#[tokio::test]
async fn a_multi_az_cluster_that_is_not_available_yet_cannot_fail_over_either() {
    let case = "a Multi-AZ cluster that is still being provisioned";
    let emulator = Emulator::new();
    // The clock is STOPPED before the create, so the cluster stays inside its
    // provisioning span however long the failover call takes: what is asserted
    // is the refusal a cluster that is still coming up carries.
    emulator.stop_clock();
    let identifier = "failover-still-creating";
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("MultiAZ", "true"),
            ],
        )
        .await;
    assert_eq!(
        created.text("ClusterStatus").as_deref(),
        Some("creating"),
        "case {case:?}: expected the create to report the cluster coming up, got {}",
        created.body
    );

    // The cluster IS Multi-AZ, so what refuses the failover here is the state it
    // is in — refused in the same words, and without failing anything over.
    let answered = emulator
        .call(
            "FailoverPrimaryCompute",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        (
            answered.status,
            answered.error_code().as_deref(),
            answered.text("Message").as_deref(),
        ),
        (
            400,
            Some("InvalidClusterState"),
            Some("You can't failover primary compute of your cluster now. Try again later."),
        ),
        "case {case:?}: expected the operation's own InvalidClusterState refusal, got body {}",
        answered.body
    );
}

#[tokio::test]
async fn a_cluster_that_is_not_multi_az_cannot_fail_its_primary_over() {
    let emulator = Emulator::new();

    for row in REFUSALS {
        let case = row.case;
        if row.created {
            let mut params: Vec<(&str, &str)> = vec![
                ("ClusterIdentifier", row.identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ];
            let value = row.multi_az.map(|on| on.to_string());
            if let Some(value) = value.as_deref() {
                params.push(("MultiAZ", value));
            }
            let created = emulator.call("CreateCluster", &params).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateCluster, got {} with body {}",
                created.status, created.body
            );
            settle(&emulator);
        }

        let answered = emulator
            .call(
                "FailoverPrimaryCompute",
                &[("ClusterIdentifier", row.identifier)],
            )
            .await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (row.status, Some(row.code)),
            "case {case:?}: expected {} {}, got {} / {:?} with body {}",
            row.status,
            row.code,
            answered.status,
            answered.error_code(),
            answered.body
        );
        // Every state a failover cannot be done from is refused in the same
        // words — the operation's own sentence, which names neither the cluster
        // nor the precondition.
        if row.code == "InvalidClusterState" {
            assert_eq!(
                answered.text("Message").as_deref(),
                Some("You can't failover primary compute of your cluster now. Try again later."),
                "case {case:?}: expected the operation's own refusal wording, got {:?}",
                answered.text("Message")
            );
        }
    }
}
