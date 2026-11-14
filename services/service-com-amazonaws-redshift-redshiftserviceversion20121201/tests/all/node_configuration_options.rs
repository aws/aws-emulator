// SPDX-License-Identifier: CC0-1.0

//! The node configurations `DescribeNodeConfigurationOptions` offers for an
//! action, end to end over the awsQuery wire.
//!
//! The RULE these cases assert: the options are the ORDERABLE shapes evaluated
//! against the source the request names — "restore-cluster to get configuration
//! combinations based on an existing snapshot", "resize-cluster to get
//! configuration combinations for elastic resize based on an existing cluster",
//! "recommend-node-config … based on an existing cluster or snapshot" — so every
//! option is a shape a cluster of this service can really have, the source has to
//! be one this account holds (each name refused with that resource's own modelled
//! fault), and the request's `Filters` narrow the same list.
//!
//! The rows differ along what the rule generalises over: the action, which source
//! names it, and which filter narrows the result. The harness is shared with
//! [`crate::cluster_lifecycle`], which owns it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The node type every cluster in this suite runs on.
const NODE_TYPE: &str = "ra3.xlplus";

/// The members one row of a table sends.
type Params = Vec<(&'static str, &'static str)>;

/// One row of the action table: the case, the action, and the source the request
/// names.
type ActionRow = (&'static str, &'static str, Params);

/// One row of the refusal table: the case, the parameters, and the status and
/// code the request is refused with.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// One row of the filter table: the case, the filter's name, its operator, its
/// values, and how many options it leaves (`None` when it only has to leave
/// fewer than all of them).
type FilterRow = (
    &'static str,
    &'static str,
    &'static str,
    Vec<&'static str>,
    Option<usize>,
);

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// An emulator holding one serving cluster of four nodes and one available
/// snapshot of it.
async fn harness(cluster: &str, snapshot: &str) -> Emulator {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", cluster),
                ("NodeType", NODE_TYPE),
                ("MasterUsername", "admin"),
                ("NumberOfNodes", "4"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed {cluster}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            "the cluster is serving",
            "DescribeClusters",
            &[("ClusterIdentifier", cluster)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
    let taken = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", snapshot),
                ("ClusterIdentifier", cluster),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case seed {snapshot}: expected 200, got {} with body {}",
        taken.status, taken.body
    );
    emulator
        .poll_until(
            "the snapshot is available",
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", snapshot)],
            |answer| answer.text("Status").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
    emulator
}

/// Each action is evaluated against the source it is documented to be evaluated
/// against, and every option it offers is an orderable shape.
#[tokio::test]
async fn each_action_offers_the_orderable_shapes_of_its_source() {
    let cluster = "node-options-cluster";
    let snapshot = "node-options-snapshot";
    let emulator = harness(cluster, snapshot).await;

    // (case, the action, the source the request names)
    let rows: Vec<ActionRow> = vec![
        (
            "an elastic resize of a cluster",
            state::ACTION_RESIZE_CLUSTER,
            vec![("ClusterIdentifier", cluster)],
        ),
        (
            "a restore of a snapshot",
            state::ACTION_RESTORE_CLUSTER,
            vec![("SnapshotIdentifier", snapshot)],
        ),
        (
            "a recommendation for a cluster",
            state::ACTION_RECOMMEND_NODE_CONFIG,
            vec![("ClusterIdentifier", cluster)],
        ),
        (
            "a recommendation for a snapshot",
            state::ACTION_RECOMMEND_NODE_CONFIG,
            vec![("SnapshotIdentifier", snapshot)],
        ),
    ];

    for (case, action, source) in rows {
        let mut params: Vec<(&str, &str)> = vec![("ActionType", action)];
        params.extend(source.iter().copied());
        let answer = emulator
            .call("DescribeNodeConfigurationOptions", &params)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        let node_types = all_text(&answer.body, "NodeType");
        assert!(
            !node_types.is_empty(),
            "case {case}: expected at least one node configuration, got body {}",
            answer.body
        );
        // Every option is a shape this service offers: its node type is one the
        // published catalog carries, and its node count is one that type is
        // orderable at.
        for node_type in &node_types {
            assert!(
                state::ORDERABLE_NODE_TYPES.contains(&node_type.as_str()),
                "case {case}: expected every option on a published node type, got {node_type:?}"
            );
        }
        let counts = all_text(&answer.body, "NumberOfNodes");
        assert_eq!(
            counts.len(),
            node_types.len(),
            "case {case}: expected one node count per option, got {counts:?} for {node_types:?}"
        );
        for count in &counts {
            let parsed: i32 = count.parse().unwrap_or_default();
            assert!(
                parsed == 1 || parsed >= state::MIN_MULTI_NODE_NODES,
                "case {case}: expected every count to be a shape a cluster can have, got \
                 {count:?}"
            );
        }
        // Every option carries the one configuration mode this service offers,
        // and none claims a disk utilisation: nothing here measures a disk.
        let modes = all_text(&answer.body, "Mode");
        assert!(
            modes.iter().all(|mode| mode == state::MODE_STANDARD),
            "case {case}: expected every option in mode {}, got {modes:?}",
            state::MODE_STANDARD
        );
        assert!(
            all_text(&answer.body, "EstimatedDiskUtilizationPercent").is_empty(),
            "case {case}: expected no disk utilisation reported, got body {}",
            answer.body
        );

        // An ELASTIC resize keeps the cluster's node type, so the options it
        // offers are all of that type; the other two actions are evaluated for a
        // new cluster, so they offer every orderable type.
        if action == state::ACTION_RESIZE_CLUSTER {
            assert!(
                node_types.iter().all(|held| held == NODE_TYPE),
                "case {case}: expected only the cluster's own node type, got {node_types:?}"
            );
        } else {
            let distinct: std::collections::BTreeSet<&str> =
                node_types.iter().map(String::as_str).collect();
            assert!(
                distinct.len() > 1,
                "case {case}: expected several node types offered, got {node_types:?}"
            );
        }
    }
}

/// The source an action is evaluated against has to be one this account holds,
/// and a request that names none describes nothing to evaluate.
#[tokio::test]
async fn the_source_an_action_is_evaluated_against_has_to_exist() {
    let cluster = "node-options-source-cluster";
    let snapshot = "node-options-source-snapshot";
    let emulator = harness(cluster, snapshot).await;

    // (case, the parameters, the status and code expected)
    let rows: Vec<RefusalRow> = vec![
        (
            "a snapshot this account does not hold",
            vec![
                ("ActionType", state::ACTION_RESTORE_CLUSTER),
                ("SnapshotIdentifier", "nowhere-snapshot"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a snapshot ARN this account does not hold",
            vec![
                ("ActionType", state::ACTION_RESTORE_CLUSTER),
                (
                    "SnapshotArn",
                    "arn:aws:redshift:us-west-2:000000000000:snapshot:nowhere/nowhere",
                ),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a cluster this account does not hold",
            vec![
                ("ActionType", state::ACTION_RESIZE_CLUSTER),
                ("ClusterIdentifier", "nowhere-cluster"),
            ],
            404,
            "ClusterNotFound",
        ),
        (
            "a restore naming no snapshot",
            vec![("ActionType", state::ACTION_RESTORE_CLUSTER)],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a resize naming no cluster",
            vec![("ActionType", state::ACTION_RESIZE_CLUSTER)],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a recommendation naming neither",
            vec![("ActionType", state::ACTION_RECOMMEND_NODE_CONFIG)],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, params, status, code) in rows {
        let answer = emulator
            .call("DescribeNodeConfigurationOptions", &params)
            .await;
        assert_refused(case, &answer, status, code);
    }
}

/// The request's filters narrow the same list, each comparing the property it
/// names — and a page size below the window this operation publishes is refused.
#[tokio::test]
async fn the_filters_narrow_the_options_the_request_is_served() {
    let cluster = "node-options-filtered-cluster";
    let snapshot = "node-options-filtered-snapshot";
    let emulator = harness(cluster, snapshot).await;

    // The unfiltered restore options, which every filtered row is measured
    // against.
    let all = emulator
        .call(
            "DescribeNodeConfigurationOptions",
            &[
                ("ActionType", state::ACTION_RESTORE_CLUSTER),
                ("SnapshotIdentifier", snapshot),
                ("MaxRecords", "500"),
            ],
        )
        .await;
    assert_eq!(
        all.status, 200,
        "case the unfiltered options: expected 200, got {} with body {}",
        all.status, all.body
    );
    let every_option = all_text(&all.body, "NodeType").len();
    assert!(
        every_option > 1,
        "case the unfiltered options: expected several options, got body {}",
        all.body
    );

    // (case, the filter's name, operator and values, how many options it leaves —
    // `None` when it only has to leave fewer than all of them)
    let rows: Vec<FilterRow> = vec![
        (
            "one node type by equality",
            state::FILTER_NODE_TYPE,
            "eq",
            vec![NODE_TYPE],
            None,
        ),
        (
            "two node types by membership",
            state::FILTER_NODE_TYPE,
            "in",
            vec![NODE_TYPE, "ra3.4xlarge"],
            None,
        ),
        (
            "a node type nothing offers",
            state::FILTER_NODE_TYPE,
            "eq",
            vec!["ds2.8xlarge"],
            Some(0),
        ),
        (
            "the one mode every option carries",
            state::FILTER_MODE,
            "eq",
            vec![state::MODE_STANDARD],
            Some(every_option),
        ),
        (
            "a mode nothing offers",
            state::FILTER_MODE,
            "eq",
            vec!["high-performance"],
            Some(0),
        ),
        (
            "more than one node",
            state::FILTER_NUMBER_OF_NODES,
            "gt",
            vec!["1"],
            None,
        ),
        (
            "a node count between two bounds",
            state::FILTER_NUMBER_OF_NODES,
            "between",
            vec!["2", "4"],
            None,
        ),
        (
            "a node count nothing offers",
            state::FILTER_NUMBER_OF_NODES,
            "gt",
            vec!["1000"],
            Some(0),
        ),
        (
            // No option reports a disk utilisation, so a filter over it leaves
            // none: that is the honest answer for a property nothing measures.
            "a disk utilisation nothing reports",
            state::FILTER_DISK_UTILIZATION,
            "lt",
            vec!["50"],
            Some(0),
        ),
    ];

    for (case, name, operator, values, expected) in rows {
        // The awsQuery key of a renamed list member is its `@xmlName` — `Filter`
        // for `Filters` and `Value` for `Values` — and the flattened layout hangs
        // the indices straight off it.
        let mut params: Vec<(String, String)> = vec![
            ("ActionType".to_string(), state::ACTION_RESTORE_CLUSTER.to_string()),
            ("SnapshotIdentifier".to_string(), snapshot.to_string()),
            ("MaxRecords".to_string(), "500".to_string()),
            ("Filter.1.Name".to_string(), name.to_string()),
            ("Filter.1.Operator".to_string(), operator.to_string()),
        ];
        for (index, value) in values.iter().enumerate() {
            params.push((
                format!("Filter.1.Value.{}", index + 1),
                (*value).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let answer = emulator
            .call("DescribeNodeConfigurationOptions", &borrowed)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let left = all_text(&answer.body, "NodeType").len();
        match expected {
            Some(count) => assert_eq!(
                left, count,
                "case {case}: expected {count} options, got {left} with body {}",
                answer.body
            ),
            None => assert!(
                left > 0 && left < every_option,
                "case {case}: expected the filter to narrow the {every_option} options, got \
                 {left}"
            ),
        }
    }

    // This operation publishes its OWN page window — "Default: 500 Constraints:
    // minimum 100, maximum 500" — so only its minimum is a refusal.
    for (case, max_records, served) in [
        ("the published minimum page size", "100", true),
        ("the published maximum page size", "500", true),
        ("one record under the published minimum", "99", false),
    ] {
        let answer = emulator
            .call(
                "DescribeNodeConfigurationOptions",
                &[
                    ("ActionType", state::ACTION_RESTORE_CLUSTER),
                    ("SnapshotIdentifier", snapshot),
                    ("MaxRecords", max_records),
                ],
            )
            .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }
}
