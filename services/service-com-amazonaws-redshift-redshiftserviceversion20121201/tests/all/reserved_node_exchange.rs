// SPDX-License-Identifier: CC0-1.0

//! The reserved-node EXCHANGE operations, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns and the offering/reservation helpers the reserved-node suite owns rather
//! than keeping a second copy of either.
//!
//! The suite asserts the RULES the four exchange operations share, not one
//! recorded request:
//!
//! * every identifier these operations name is one this service MINTED, so its
//!   FORM is ruled out as a bad parameter value before any lookup, and only a
//!   well-formed identifier that names nothing is the modelled not-found fault;
//! * an operation whose subject is named by optional members refuses a request
//!   that names none of them on the parameter combination, before any lookup;
//! * the offerings a reservation may be exchanged onto are the exchange-target
//!   offerings matching its payment type, term and usage price — the same set
//!   `GetReservedNodeExchangeOfferings` reports and `AcceptReservedNodeExchange`
//!   accepts;
//! * an exchange moves the reservation it names onto the offering it names and
//!   is reported by every later read.
//!
//! Each table row differs along what the rule generalises over: which part of
//! the identifier form is broken, which operation and member carries it, which
//! offering the reservation was bought from (node type, term, payment option),
//! and
//! which kind of resource an options read is asked about.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::{
    Cluster, ReservedNode,
};
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{element_blocks, Emulator, REGION};
use crate::reserved_nodes::{
    catalog, offering_of, one, poll_until_state, purchase, reservation, ONE_YEAR, POLL_DEADLINE,
    POLL_INTERVAL, THREE_YEARS,
};
use crate::snapshot_copy_configuration::assert_refused;

/// A well-formed identifier this account holds nothing under: the form every
/// identifier these operations name has, with no value behind it.
const UNHELD_IDENTIFIER: &str = "00000000-0000-0000-0000-000000000000";

/// Another one, so a row that needs two distinct missing identifiers has them.
const OTHER_UNHELD_IDENTIFIER: &str = "abcdef12-3456-7890-abcd-ef1234567890";

/// Values that are NOT of the 8-4-4-4-12 lowercase-hex form this service mints,
/// each breaking a different part of it.
const MALFORMED: [(&str, &str); 6] = [
    ("a plain name", "sim2real-behav-missing-reserved-node"),
    ("hex in upper case", "A1B2C3D4-0000-0000-0000-000000000000"),
    (
        "a group one character short",
        "0000000-0000-0000-0000-000000000000",
    ),
    (
        "one group too many",
        "00000000-0000-0000-0000-0000-00000000",
    ),
    (
        "a non-hex character",
        "0000000g-0000-0000-0000-000000000000",
    ),
    (
        "a well-formed value with surrounding space",
        " 00000000-0000-0000-0000-000000000000 ",
    ),
];

/// The node type the seeded DS2 records carry.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NodeType")` lists no DS2 node type, so no cluster and no reservation
/// this emulator's own operations can create is one reserved-node exchange
/// covers — the only way to reach the SERVING path of
/// `GetReservedNodeExchangeConfigurationOptions` is to seed the records, exactly
/// as the snapshot suites seed a snapshot Redshift took on its own schedule.
const SEEDED_DS2_NODE_TYPE: &str = "ds2.xlarge";

/// How many nodes the seeded DS2 reservation holds.
const SEEDED_DS2_NODE_COUNT: i32 = 2;

/// The error message AWS answers a reserved-node exchange request about a
/// resource whose node type the operation does not cover.
const ONLY_DS2: &str = "You can only exchange DS2 reserved nodes.";

/// Buy the offering of this node type, term and payment option and wait for the
/// clock-owed payment confirmation, answering the reservation's id.
async fn active_reservation(
    emulator: &Emulator,
    offerings: &[String],
    case: &str,
    node_type: &str,
    duration: &str,
    offering_type: &str,
) -> String {
    let offering = offering_of(case, offerings, node_type, duration, offering_type);
    let bought = purchase(emulator, &offering, Some(2), case).await;
    assert_eq!(
        bought.status, 200,
        "case {case}: expected PurchaseReservedNodeOffering 200, got {} / {:?} with body {}",
        bought.status,
        bought.error_code(),
        bought.body
    );
    let reserved_node_id = bought
        .text("ReservedNodeId")
        .unwrap_or_else(|| panic!("case {case}: the purchase must report a ReservedNodeId"));
    poll_until_state(
        emulator,
        case,
        &reserved_node_id,
        state::RESERVED_NODE_STATE_ACTIVE,
    )
    .await;
    reserved_node_id
}

/// The offering identifiers the EXCHANGE-TARGET catalog publishes that the
/// reservation in `reservation_block` may be exchanged onto: the upgradable
/// offerings whose payment type, term and usage price are the reservation's own.
fn expected_targets(case: &str, reservation_block: &str) -> Vec<String> {
    let offering_type = one(case, reservation_block, "OfferingType");
    let duration = one(case, reservation_block, "Duration");
    let usage_price: f64 = one(case, reservation_block, "UsagePrice")
        .parse()
        .unwrap_or_else(|_| panic!("case {case}: a reservation reports a numeric UsagePrice"));
    state::published_exchange_reserved_node_offerings()
        .into_iter()
        .filter(|offering| {
            offering.reserved_node_offering_type.as_deref()
                == Some(state::RESERVED_NODE_OFFERING_TYPE_UPGRADABLE)
                && offering.offering_type.as_deref() == Some(offering_type.as_str())
                && offering.duration.map(|term| term.to_string()).as_deref()
                    == Some(duration.as_str())
                && offering
                    .usage_price
                    .is_some_and(|price| (price - usage_price).abs() < 1e-6)
        })
        .filter_map(|offering| offering.reserved_node_offering_id)
        .collect()
}

/// The offering identifiers one `GetReservedNodeExchangeOfferings` page reports,
/// asserting the read succeeded.
async fn exchange_offerings(emulator: &Emulator, case: &str, reserved_node_id: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "GetReservedNodeExchangeOfferings",
            &[("ReservedNodeId", reserved_node_id)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    element_blocks(&answer.body, "ReservedNodeOffering")
        .iter()
        .map(|block| one(case, block, "ReservedNodeOfferingId"))
        .collect()
}

/// Create one cluster of this node type and wait out the clock-owed
/// `creating` -> `available` transition.
async fn available_cluster_of(emulator: &Emulator, identifier: &str, node_type: &str) {
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", node_type),
                ("MasterUsername", "admin"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case the cluster {identifier}: the create must be served, got {} with body {}",
        created.status, created.body
    );
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
}

/// Seed a DS2 cluster and a DS2 reservation straight into the shared store.
///
/// Nothing this emulator serves can create either (no DS2 node type is
/// orderable and the offering catalog publishes none), so seeding is the only
/// way to observe what the operation SERVES rather than what it refuses. The
/// seeded reservation carries a published offering's term, payment type and
/// prices, so the candidate rule has real dimensions to match against.
fn seed_ds2(emulator: &Emulator, cluster_identifier: &str) -> String {
    let offering = state::published_reserved_node_offerings()
        .into_iter()
        .find(|offering| {
            offering.node_type.as_deref() == Some("dc2.large")
                && offering.offering_type.as_deref() == Some(state::PARTIAL_UPFRONT_OFFERING_TYPE)
                && offering.duration == Some(31_536_000)
        })
        .expect("the catalog publishes a one-year part-upfront dc2.large offering");
    let reserved_node_id = state::new_reserved_node_id();
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store.clusters.push(state::StoredCluster {
        cluster: Cluster {
            cluster_identifier: Some(cluster_identifier.to_string()),
            node_type: Some(SEEDED_DS2_NODE_TYPE.to_string()),
            cluster_status: Some("available".to_string()),
            number_of_nodes: Some(SEEDED_DS2_NODE_COUNT),
            ..Default::default()
        },
        cluster_type: state::CLUSTER_TYPE_MULTI_NODE.to_string(),
        master_user_password: None,
        // Already past, so the record needs no clock-owed transition.
        available_at: std::time::Instant::now(),
        // A seeded cluster serves on the endpoint the service would have given
        // it, built by the one rule every cluster's endpoint comes from.
        endpoint: state::cluster_endpoint(
            cluster_identifier,
            REGION,
            state::DEFAULT_PORT,
            Some("vpc-seeded"),
            &format!("{REGION}a"),
        ),
        // A seeded cluster names no admin user, so its database holds none.
        database_users: Default::default(),
        // A seeded cluster has had no logging enabled and no resize asked for.
        logging: Default::default(),
        resize: None,
        // It is already serving, and no lakehouse configuration was written on
        // it.
        settles_to: None,
        lakehouse_idc_application_arn: None,
    });
    store.reserved_nodes.push(state::StoredReservedNode {
        node: ReservedNode {
            reserved_node_id: Some(reserved_node_id.clone()),
            reserved_node_offering_id: None,
            node_type: Some(SEEDED_DS2_NODE_TYPE.to_string()),
            start_time: Some(aws_smithy_types::DateTime::from(
                std::time::SystemTime::now(),
            )),
            duration: offering.duration,
            fixed_price: offering.fixed_price,
            usage_price: offering.usage_price,
            currency_code: offering.currency_code.clone(),
            node_count: Some(SEEDED_DS2_NODE_COUNT),
            state: Some(state::RESERVED_NODE_STATE_ACTIVE.to_string()),
            offering_type: offering.offering_type.clone(),
            recurring_charges: offering.recurring_charges.clone(),
            reserved_node_offering_type: Some(
                state::RESERVED_NODE_OFFERING_TYPE_REGULAR.to_string(),
            ),
        },
        active_at: std::time::Instant::now(),
    });
    reserved_node_id
}

/// An identifier of a form this service never mints is refused as a bad
/// parameter VALUE, on every exchange operation and every member that carries
/// one — and before any lookup, which the next test's 404s are the other half
/// of.
#[tokio::test]
async fn a_malformed_identifier_is_refused_before_any_lookup() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;

    // The account HOLDS a reservation throughout, so no refusal below can be
    // explained by an empty inventory.
    let held = active_reservation(
        &emulator,
        &offerings,
        "the held reservation",
        "dc2.large",
        ONE_YEAR,
        state::ALL_UPFRONT_OFFERING_TYPE,
    )
    .await;
    let a_published_offering = one(
        "the held reservation",
        &offering_of(
            "the held reservation",
            &offerings,
            "dc2.large",
            ONE_YEAR,
            state::ALL_UPFRONT_OFFERING_TYPE,
        ),
        "ReservedNodeOfferingId",
    );

    for (form, value) in MALFORMED {
        let rows: [(&str, Vec<(&str, &str)>); 5] = [
            (
                "GetReservedNodeExchangeOfferings",
                vec![("ReservedNodeId", value)],
            ),
            (
                "AcceptReservedNodeExchange on ReservedNodeId",
                vec![
                    ("ReservedNodeId", value),
                    ("TargetReservedNodeOfferingId", a_published_offering.as_str()),
                ],
            ),
            (
                "AcceptReservedNodeExchange on TargetReservedNodeOfferingId",
                vec![
                    ("ReservedNodeId", held.as_str()),
                    ("TargetReservedNodeOfferingId", value),
                ],
            ),
            (
                "DescribeReservedNodeExchangeStatus on ReservedNodeId",
                vec![("ReservedNodeId", value)],
            ),
            (
                "DescribeReservedNodeExchangeStatus on ReservedNodeExchangeRequestId",
                vec![("ReservedNodeExchangeRequestId", value)],
            ),
        ];
        for (where_, params) in rows {
            let action = where_
                .split_once(' ')
                .map_or(where_, |(operation, _)| operation);
            let case = format!("{form} on {where_}");
            let answer = emulator.call(action, &params).await;
            assert_refused(&case, &answer, 400, "InvalidParameterValue");
        }
    }
}

/// A WELL-FORMED identifier that names nothing is the modelled not-found fault
/// of the resource it names, each with the message AWS words for that class.
#[tokio::test]
async fn a_well_formed_identifier_naming_nothing_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let held = active_reservation(
        &emulator,
        &offerings,
        "the held reservation",
        "dc2.large",
        ONE_YEAR,
        state::ALL_UPFRONT_OFFERING_TYPE,
    )
    .await;
    let a_published_offering = one(
        "the held reservation",
        &offering_of(
            "the held reservation",
            &offerings,
            "dc2.large",
            ONE_YEAR,
            state::ALL_UPFRONT_OFFERING_TYPE,
        ),
        "ReservedNodeOfferingId",
    );

    // Rows differ along the operation, the member and so the resource the
    // identifier names; the two unheld values differ in their digits alone.
    for (case, action, params, code, message) in [
        (
            "an unheld reservation on GetReservedNodeExchangeOfferings",
            "GetReservedNodeExchangeOfferings",
            vec![("ReservedNodeId", UNHELD_IDENTIFIER)],
            "ReservedNodeNotFound",
            format!("Reserved Node {UNHELD_IDENTIFIER} not found."),
        ),
        (
            "another unheld reservation on GetReservedNodeExchangeOfferings",
            "GetReservedNodeExchangeOfferings",
            vec![("ReservedNodeId", OTHER_UNHELD_IDENTIFIER)],
            "ReservedNodeNotFound",
            format!("Reserved Node {OTHER_UNHELD_IDENTIFIER} not found."),
        ),
        (
            "an unheld reservation on AcceptReservedNodeExchange",
            "AcceptReservedNodeExchange",
            vec![
                ("ReservedNodeId", UNHELD_IDENTIFIER),
                ("TargetReservedNodeOfferingId", a_published_offering.as_str()),
            ],
            "ReservedNodeNotFound",
            format!("Reserved Node {UNHELD_IDENTIFIER} not found."),
        ),
        (
            "an unheld reservation on DescribeReservedNodeExchangeStatus",
            "DescribeReservedNodeExchangeStatus",
            vec![("ReservedNodeId", UNHELD_IDENTIFIER)],
            "ReservedNodeNotFound",
            format!("Reserved Node {UNHELD_IDENTIFIER} not found."),
        ),
        (
            "an exchange nobody asked for",
            "DescribeReservedNodeExchangeStatus",
            vec![("ReservedNodeExchangeRequestId", UNHELD_IDENTIFIER)],
            "ReservedNodeExchangeNotFond",
            format!("Reserved Node Exchange Request {UNHELD_IDENTIFIER} not found."),
        ),
        (
            "an offering the catalog does not publish",
            "AcceptReservedNodeExchange",
            vec![
                ("ReservedNodeId", held.as_str()),
                ("TargetReservedNodeOfferingId", UNHELD_IDENTIFIER),
            ],
            "ReservedNodeOfferingNotFound",
            format!("Reserved node offering not found: '{UNHELD_IDENTIFIER}'."),
        ),
    ] {
        let answer = emulator.call(action, &params).await;
        assert_refused(case, &answer, 404, code);
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(message.as_str()),
            "case {case}: expected the message {message:?}, got {:?}",
            answer.text("Message")
        );
    }
}

/// A request that names none of the members its subject could be named by is
/// refused on the parameter COMBINATION, before any lookup — which the rows that
/// DO name one and reach a 404 are the other half of.
#[tokio::test]
async fn a_request_naming_no_subject_is_refused_on_the_parameter_combination() {
    let emulator = Emulator::new();

    for (case, action, params) in [
        (
            "an exchange-status read with no filter at all",
            "DescribeReservedNodeExchangeStatus",
            vec![],
        ),
        (
            "an exchange-status read filtered only by page size",
            "DescribeReservedNodeExchangeStatus",
            vec![("MaxRecords", "20")],
        ),
        (
            "a resize-cluster options read naming nothing",
            "GetReservedNodeExchangeConfigurationOptions",
            vec![("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER)],
        ),
        (
            "a resize-cluster options read naming only a snapshot",
            "GetReservedNodeExchangeConfigurationOptions",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER),
                ("SnapshotIdentifier", "a-snapshot"),
            ],
        ),
        (
            "a restore-cluster options read naming nothing",
            "GetReservedNodeExchangeConfigurationOptions",
            vec![("ActionType", state::EXCHANGE_ACTION_RESTORE_CLUSTER)],
        ),
        (
            "a restore-cluster options read naming only a cluster",
            "GetReservedNodeExchangeConfigurationOptions",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESTORE_CLUSTER),
                ("ClusterIdentifier", "a-cluster"),
            ],
        ),
    ] {
        let answer = emulator.call(action, &params).await;
        assert_refused(case, &answer, 400, "InvalidParameterCombination");
    }

    // Naming the subject gets PAST the combination check, so the same requests
    // are answered by the lookup instead.
    for (case, params, code) in [
        (
            "a resize-cluster options read naming a cluster",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER),
                ("ClusterIdentifier", "a-cluster"),
            ],
            "ClusterNotFound",
        ),
        (
            "a restore-cluster options read naming a snapshot",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESTORE_CLUSTER),
                ("SnapshotIdentifier", "a-snapshot"),
            ],
            "ClusterSnapshotNotFound",
        ),
    ] {
        let answer = emulator
            .call("GetReservedNodeExchangeConfigurationOptions", &params)
            .await;
        assert_refused(case, &answer, 404, code);
    }
}

/// The options read resolves the resource its action is initiated from, THEN
/// inspects that resource's node type: "does not exist" and "exists but is not
/// exchange-eligible" are different answers.
#[tokio::test]
async fn the_options_read_resolves_the_resource_then_its_node_type() {
    let emulator = Emulator::new();

    // A cluster and a snapshot that do not exist, each refused as the kind of
    // resource its member names.
    for (case, params, code, message) in [
        (
            "a cluster nobody created",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER),
                ("ClusterIdentifier", "no-such-cluster"),
            ],
            "ClusterNotFound",
            "The cluster no-such-cluster was not found.",
        ),
        (
            "a snapshot nobody took",
            vec![
                ("ActionType", state::EXCHANGE_ACTION_RESTORE_CLUSTER),
                ("SnapshotIdentifier", "no-such-snapshot"),
            ],
            "ClusterSnapshotNotFound",
            "The snapshot no-such-snapshot was not found.",
        ),
    ] {
        let answer = emulator
            .call("GetReservedNodeExchangeConfigurationOptions", &params)
            .await;
        assert_refused(case, &answer, 404, code);
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(message),
            "case {case}: expected the message {message:?}, got {:?}",
            answer.text("Message")
        );
    }

    // A cluster that DOES exist, of each of several node types a cluster can be
    // created with — none of them a node type this operation covers, so each is
    // refused for its node type rather than for not existing.
    for (case, identifier, node_type) in [
        ("the smallest dense-compute node", "dc2-cluster", "dc2.large"),
        ("a managed-storage node", "ra3-cluster", "ra3.xlplus"),
        ("a reserve-group node", "rg-cluster", "rg.large"),
    ] {
        available_cluster_of(&emulator, identifier, node_type).await;
        let answer = emulator
            .call(
                "GetReservedNodeExchangeConfigurationOptions",
                &[
                    ("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER),
                    ("ClusterIdentifier", identifier),
                ],
            )
            .await;
        assert_refused(case, &answer, 400, "UnsupportedOperation");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(ONLY_DS2),
            "case {case}: expected the message {ONLY_DS2:?}, got {:?}",
            answer.text("Message")
        );

        // A snapshot OF that cluster carries the cluster's node type, so the
        // restore-initiated read answers the same way.
        let snapshot = format!("{identifier}-snapshot");
        let taken = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot.as_str()),
                    ("ClusterIdentifier", identifier),
                ],
            )
            .await;
        assert_eq!(
            taken.status, 200,
            "case {case}: the snapshot must be taken, got {} / {:?} with body {}",
            taken.status,
            taken.error_code(),
            taken.body
        );
        let answer = emulator
            .call(
                "GetReservedNodeExchangeConfigurationOptions",
                &[
                    ("ActionType", state::EXCHANGE_ACTION_RESTORE_CLUSTER),
                    ("SnapshotIdentifier", snapshot.as_str()),
                ],
            )
            .await;
        assert_refused(&format!("{case}, through its snapshot"), &answer, 400, "UnsupportedOperation");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(ONLY_DS2),
            "case {case}, through its snapshot: expected the message {ONLY_DS2:?}, got {:?}",
            answer.text("Message")
        );
    }
}

/// For a resource the operation DOES cover, the options are computed from the
/// account's own reservations and the offering catalog.
#[tokio::test]
async fn the_options_of_a_covered_cluster_are_computed_from_the_account_inventory() {
    let emulator = Emulator::new();
    let seeded = seed_ds2(&emulator, "ds2-cluster");

    let answer = emulator
        .call(
            "GetReservedNodeExchangeConfigurationOptions",
            &[
                ("ActionType", state::EXCHANGE_ACTION_RESIZE_CLUSTER),
                ("ClusterIdentifier", "ds2-cluster"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case a covered cluster: expected 200, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );

    let case = "a covered cluster";
    let held = reservation(&emulator, case, &seeded).await;
    let expected = expected_targets(case, &held);
    assert!(
        expected.len() > 1,
        "case {case}: the seeded reservation must have several candidate offerings, got {expected:?}"
    );

    let options = element_blocks(&answer.body, "ReservedNodeConfigurationOption");
    let served: Vec<String> = options
        .iter()
        .map(|block| {
            let target = element_blocks(block, "TargetReservedNodeOffering")
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case}: every option names its target offering"));
            one(case, &target, "ReservedNodeOfferingId")
        })
        .collect();
    assert_eq!(
        served, expected,
        "case {case}: expected one option per candidate offering, got {served:?} in body {}",
        answer.body
    );
    for block in &options {
        let source = element_blocks(block, "SourceReservedNode")
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: every option names its source reservation"));
        assert_eq!(
            one(case, &source, "ReservedNodeId"),
            seeded,
            "case {case}: every option must carry the reservation it is for, got {block}"
        );
        assert_eq!(
            one(case, block, "TargetReservedNodeCount"),
            SEEDED_DS2_NODE_COUNT.to_string(),
            "case {case}: an exchange keeps the number of nodes, got {block}"
        );
    }
}

/// The offerings a reservation may be exchanged onto are the exchange-target
/// offerings matching its payment type, term and usage price — computed from the
/// catalog, over reservations differing in every dimension the rule reads.
#[tokio::test]
async fn the_offerings_an_exchange_may_target_match_the_reservation() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;

    // Rows differ along node type (inside and outside the exchange-target
    // family), term and payment option — the three dimensions the candidate rule
    // reads off the reservation.
    let rows: [(&str, &str, &str, &str); 5] = [
        (
            "a dense-compute node, one year, all upfront",
            "dc2.large",
            ONE_YEAR,
            state::ALL_UPFRONT_OFFERING_TYPE,
        ),
        (
            "a dense-compute node, one year, nothing upfront",
            "dc2.large",
            ONE_YEAR,
            state::NO_UPFRONT_OFFERING_TYPE,
        ),
        (
            "the largest dense-compute node, three years, part upfront",
            "dc2.8xlarge",
            THREE_YEARS,
            state::PARTIAL_UPFRONT_OFFERING_TYPE,
        ),
        (
            "a managed-storage node, one year, nothing upfront",
            "ra3.large",
            ONE_YEAR,
            state::NO_UPFRONT_OFFERING_TYPE,
        ),
        (
            "a reserve-group node, three years, all upfront",
            "rg.large",
            THREE_YEARS,
            state::ALL_UPFRONT_OFFERING_TYPE,
        ),
    ];

    let mut candidate_sets: Vec<(&str, (&str, &str), Vec<String>)> = Vec::new();
    for (case, node_type, duration, offering_type) in rows {
        let reserved_node_id = active_reservation(
            &emulator,
            &offerings,
            case,
            node_type,
            duration,
            offering_type,
        )
        .await;
        let held = reservation(&emulator, case, &reserved_node_id).await;
        let expected = expected_targets(case, &held);
        let served = exchange_offerings(&emulator, case, &reserved_node_id).await;
        assert_eq!(
            served, expected,
            "case {case}: expected the candidate offerings the exchange catalog publishes, got {served:?}"
        );
        assert!(
            !served.is_empty(),
            "case {case}: a reservation of a published term and payment option has candidates, got {served:?}"
        );
        candidate_sets.push((case, (duration, offering_type), served));
    }

    // The rule READS the reservation: two reservations differing in term or
    // payment option can share no candidate, while one bought under a payment
    // option no exchange target carries has none at all.
    for (case, dimensions, served) in &candidate_sets {
        for (other_case, other_dimensions, other) in &candidate_sets {
            if dimensions == other_dimensions {
                continue;
            }
            assert!(
                served.iter().all(|id| !other.contains(id)),
                "case {case} against {other_case}: expected disjoint candidate sets, got {served:?} and {other:?}"
            );
        }
    }

    let retired = seed_retired_payment_option(&emulator);
    let none = exchange_offerings(&emulator, "a retired payment option", &retired).await;
    assert!(
        none.is_empty(),
        "case a retired payment option: expected no candidate offering, got {none:?}"
    );
}

/// Seed an ACTIVE reservation bought under a payment option neither published
/// catalog offers any more, so no exchange-target offering matches its payment
/// type: the zero-candidate row of the table above, which nothing this emulator
/// serves can create.
fn seed_retired_payment_option(emulator: &Emulator) -> String {
    let reserved_node_id = state::new_reserved_node_id();
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    store.lock().reserved_nodes.push(state::StoredReservedNode {
        node: ReservedNode {
            reserved_node_id: Some(reserved_node_id.clone()),
            reserved_node_offering_id: None,
            node_type: Some("dc2.large".to_string()),
            start_time: Some(aws_smithy_types::DateTime::from(
                std::time::SystemTime::now(),
            )),
            duration: Some(31_536_000),
            fixed_price: Some(0.0),
            usage_price: Some(0.215),
            currency_code: Some("USD".to_string()),
            node_count: Some(1),
            state: Some(state::RESERVED_NODE_STATE_ACTIVE.to_string()),
            offering_type: Some("Heavy Utilization".to_string()),
            recurring_charges: Some(Vec::new()),
            reserved_node_offering_type: Some(
                state::RESERVED_NODE_OFFERING_TYPE_REGULAR.to_string(),
            ),
        },
        active_at: std::time::Instant::now(),
    });
    reserved_node_id
}

/// The compute-node bound the purchase path enforces bounds the exchange path
/// too: an exchange RECORDS a reservation of as many nodes as the one it moves
/// off, so one that would carry the account past the bound is refused with the
/// quota fault and records nothing.
#[tokio::test]
async fn an_exchange_past_the_compute_node_quota_is_refused() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let case = "a reservation holding the whole compute-node quota";

    // One purchase of the whole quota: allowed, since it lands exactly on the
    // bound.
    let offering = offering_of(
        case,
        &offerings,
        "dc2.large",
        ONE_YEAR,
        state::ALL_UPFRONT_OFFERING_TYPE,
    );
    let bought = purchase(
        &emulator,
        &offering,
        Some(state::RESERVED_NODE_QUOTA),
        case,
    )
    .await;
    assert_eq!(
        bought.status, 200,
        "case {case}: a purchase of the whole quota must be served, got {} / {:?} with body {}",
        bought.status,
        bought.error_code(),
        bought.body
    );
    let source = bought
        .text("ReservedNodeId")
        .expect("the purchase reports a ReservedNodeId");
    poll_until_state(
        &emulator,
        case,
        &source,
        state::RESERVED_NODE_STATE_ACTIVE,
    )
    .await;

    // The read still answers — the bound is on RECORDING a reservation, not on
    // reporting what may be exchanged.
    let candidates = exchange_offerings(&emulator, case, &source).await;
    let target = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the reservation must have a candidate offering"));

    let refused = emulator
        .call(
            "AcceptReservedNodeExchange",
            &[
                ("ReservedNodeId", source.as_str()),
                ("TargetReservedNodeOfferingId", target.as_str()),
            ],
        )
        .await;
    assert_refused(case, &refused, 400, "ReservedNodeQuotaExceeded");

    // Nothing was recorded: the reservation is untouched and no exchange is
    // readable under it.
    let held = reservation(&emulator, case, &source).await;
    assert_eq!(
        one(case, &held, "State"),
        state::RESERVED_NODE_STATE_ACTIVE,
        "case {case}: the refused exchange must leave the reservation active, got {held}"
    );
    let status = emulator
        .call(
            "DescribeReservedNodeExchangeStatus",
            &[("ReservedNodeId", source.as_str())],
        )
        .await;
    assert_eq!(
        element_blocks(&status.body, "ReservedNodeExchangeStatus").len(),
        0,
        "case {case}: the refused exchange must record no request, got body {}",
        status.body
    );
}

/// An exchange moves the reservation it names onto the offering it names, and
/// every later read reports it: the source retires, the minted reservation is
/// held, the request is readable under both its filters, and the source cannot
/// be exchanged twice.
#[tokio::test]
async fn an_accepted_exchange_moves_the_reservation_and_is_reported_by_a_later_read() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let case = "an exchange of a one-year fully-upfront dc2.large reservation";

    // A reservation whose payment is not confirmed yet is not available for
    // use, so an exchange of it is refused for its STATE — the transient state
    // the purchase itself reported.
    let offering = offering_of(
        case,
        &offerings,
        "dc2.large",
        ONE_YEAR,
        state::ALL_UPFRONT_OFFERING_TYPE,
    );
    let bought = purchase(&emulator, &offering, Some(3), case).await;
    assert_eq!(
        bought.status, 200,
        "case {case}: expected PurchaseReservedNodeOffering 200, got {} with body {}",
        bought.status, bought.body
    );
    let source = bought
        .text("ReservedNodeId")
        .expect("the purchase reports a ReservedNodeId");
    assert_eq!(
        bought.text("State").as_deref(),
        Some(state::RESERVED_NODE_STATE_PENDING_PAYMENT),
        "case {case}: expected the purchase to report the transient state, got {:?}",
        bought.text("State")
    );
    let pending = emulator
        .call(
            "GetReservedNodeExchangeOfferings",
            &[("ReservedNodeId", source.as_str())],
        )
        .await;
    assert_refused(
        &format!("{case} while its payment is unconfirmed"),
        &pending,
        400,
        "InvalidReservedNodeState",
    );

    poll_until_state(
        &emulator,
        case,
        &source,
        state::RESERVED_NODE_STATE_ACTIVE,
    )
    .await;

    // The target is one of the offerings the service itself says this
    // reservation may be exchanged onto.
    let candidates = exchange_offerings(&emulator, case, &source).await;
    let target = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the reservation must have a candidate offering"));

    let accepted = emulator
        .call(
            "AcceptReservedNodeExchange",
            &[
                ("ReservedNodeId", source.as_str()),
                ("TargetReservedNodeOfferingId", target.as_str()),
            ],
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {case}: expected AcceptReservedNodeExchange 200, got {} / {:?} with body {}",
        accepted.status,
        accepted.error_code(),
        accepted.body
    );
    let exchanged_block = element_blocks(&accepted.body, "ExchangedReservedNode")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the accept must report the exchanged node"));
    let exchanged = one(case, &exchanged_block, "ReservedNodeId");
    assert_ne!(
        exchanged, source,
        "case {case}: the exchange mints a reservation of its own, got {exchanged_block}"
    );
    assert_eq!(
        one(case, &exchanged_block, "ReservedNodeOfferingId"),
        target,
        "case {case}: the exchanged node must be on the target offering, got {exchanged_block}"
    );
    assert_eq!(
        one(case, &exchanged_block, "NodeCount"),
        "3",
        "case {case}: an exchange keeps the number of nodes, got {exchanged_block}"
    );

    // An offering that is NOT a candidate changes the configuration, so the
    // exchange does not accept it. Asked of a SECOND reservation, because the
    // one just exchanged can no longer be exchanged at all.
    let another = active_reservation(
        &emulator,
        &offerings,
        "a second reservation",
        "dc2.large",
        ONE_YEAR,
        "No Upfront",
    )
    .await;
    let other = offering_of(
        case,
        &offerings,
        "ra3.large",
        THREE_YEARS,
        "No Upfront",
    );
    let other_id = one(case, &other, "ReservedNodeOfferingId");
    let refused = emulator
        .call(
            "AcceptReservedNodeExchange",
            &[
                ("ReservedNodeId", another.as_str()),
                ("TargetReservedNodeOfferingId", other_id.as_str()),
            ],
        )
        .await;
    assert_refused(
        &format!("{case} onto an offering of another term"),
        &refused,
        400,
        "UnsupportedOperation",
    );

    // The exchange is readable under the reservation it is OF and under its own
    // request id, and nowhere else.
    let by_node = emulator
        .call(
            "DescribeReservedNodeExchangeStatus",
            &[("ReservedNodeId", source.as_str())],
        )
        .await;
    assert_eq!(
        by_node.status, 200,
        "case {case}: expected the status read to be served, got {} / {:?} with body {}",
        by_node.status,
        by_node.error_code(),
        by_node.body
    );
    let records = element_blocks(&by_node.body, "ReservedNodeExchangeStatus");
    assert_eq!(
        records.len(),
        1,
        "case {case}: expected the one exchange of that reservation, got {records:?}"
    );
    let record = records[0].clone();
    let request_id = one(case, &record, "ReservedNodeExchangeRequestId");
    assert_eq!(
        one(case, &record, "SourceReservedNodeId"),
        source,
        "case {case}: the request must name the reservation it is of, got {record}"
    );
    assert_eq!(
        one(case, &record, "TargetReservedNodeOfferingId"),
        target,
        "case {case}: the request must name the offering it is onto, got {record}"
    );
    assert_eq!(
        (
            one(case, &record, "SourceReservedNodeCount"),
            one(case, &record, "TargetReservedNodeCount")
        ),
        ("3".to_string(), "3".to_string()),
        "case {case}: an exchange keeps the number of nodes, got {record}"
    );

    let by_request = emulator
        .call(
            "DescribeReservedNodeExchangeStatus",
            &[("ReservedNodeExchangeRequestId", request_id.as_str())],
        )
        .await;
    let under_request = element_blocks(&by_request.body, "ReservedNodeExchangeStatus");
    assert_eq!(
        under_request.len(),
        1,
        "case {case}: expected the one exchange under that request id, got {} / {:?} with body {}",
        by_request.status,
        by_request.error_code(),
        by_request.body
    );
    assert_eq!(
        one(case, &under_request[0], "SourceReservedNodeId"),
        source,
        "case {case}: both filters must report the same exchange, got {}",
        under_request[0]
    );
    let by_minted = emulator
        .call(
            "DescribeReservedNodeExchangeStatus",
            &[("ReservedNodeId", exchanged.as_str())],
        )
        .await;
    assert_eq!(
        element_blocks(&by_minted.body, "ReservedNodeExchangeStatus").len(),
        0,
        "case {case}: the reservation the exchange MINTED is the source of none, got body {}",
        by_minted.body
    );

    // The reservation an exchange moved off cannot be exchanged again, under
    // either operation.
    for (action, params) in [
        (
            "GetReservedNodeExchangeOfferings",
            vec![("ReservedNodeId", source.as_str())],
        ),
        (
            "AcceptReservedNodeExchange",
            vec![
                ("ReservedNodeId", source.as_str()),
                ("TargetReservedNodeOfferingId", target.as_str()),
            ],
        ),
    ] {
        let again = emulator.call(action, &params).await;
        assert_refused(
            &format!("{case}, asked of {action} a second time"),
            &again,
            400,
            "ReservedNodeAlreadyMigrated",
        );
    }

    // The clock owes the completion: the request succeeds, the source retires
    // and the minted reservation becomes active — all three without any further
    // mutating call.
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    loop {
        let answer = emulator
            .call(
                "DescribeReservedNodeExchangeStatus",
                &[("ReservedNodeExchangeRequestId", request_id.as_str())],
            )
            .await;
        let blocks = element_blocks(&answer.body, "ReservedNodeExchangeStatus");
        last = blocks.first().cloned().unwrap_or(last);
        if one(case, &last, "Status") == state::RESERVED_NODE_EXCHANGE_STATUS_SUCCEEDED {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: the exchange never completed within {POLL_DEADLINE:?}; last read: {last}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    poll_until_state(
        &emulator,
        &format!("{case}, the retired source"),
        &source,
        state::RESERVED_NODE_STATE_RETIRED,
    )
    .await;
    poll_until_state(
        &emulator,
        &format!("{case}, the minted reservation"),
        &exchanged,
        state::RESERVED_NODE_STATE_ACTIVE,
    )
    .await;
}
