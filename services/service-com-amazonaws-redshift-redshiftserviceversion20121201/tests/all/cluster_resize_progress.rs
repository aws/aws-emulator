// SPDX-License-Identifier: CC0-1.0

//! What a cluster's RESIZE progress reads as, end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: `DescribeResize` reports the LAST resize of
//! the cluster it names, and a cluster no resize has ever been started for is
//! refused 404 `ResizeNotFound` rather than answered with an empty progress
//! record (`smithy_query(query="doc", operation="DescribeResize")` — "returns
//! information about the last resize operation for the specified cluster. If no
//! resize operation has ever been initiated for the specified cluster, a HTTP 404
//! error is returned. If a resize operation was initiated and completed, the
//! status of the resize remains as SUCCEEDED until the next resize"). Concretely:
//!
//! * a cluster whose shape nothing has changed has no resize: `ResizeNotFound`,
//!   with `No Resize found for Cluster <ClusterIdentifier>`;
//! * changing a cluster's SHAPE is what starts one — `ModifyCluster`'s own
//!   documentation says "when you decide to change your cluster's node type or
//!   number of nodes … you can use DescribeResize to track the progress of the
//!   resize request" — and the reported target IS the shape the cluster is moving
//!   to;
//! * a modify that changes anything ELSE starts no resize, so such a cluster is
//!   still `ResizeNotFound`;
//! * the resize is `IN_PROGRESS` while its span is still running and `SUCCEEDED`
//!   from then on, and it stays reported after it finished;
//! * a request naming a cluster the account does not have is that resource's own
//!   `ClusterNotFound` instead.
//!
//! The rows differ along what the rule generalises over: which part of the shape
//! the modify changes (the node count, the node type, the cluster type, several at
//! once, nothing), in which direction, and whether the cluster is encrypted (which
//! decides the reported target encryption type).

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// Create a cluster of this shape, and answer nothing: every assertion below
/// reads the cluster back through a service read.
async fn create(emulator: &Emulator, identifier: &str, extra: &[(&str, &str)]) {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
    ];
    params.extend_from_slice(extra);
    let created = emulator.call("CreateCluster", &params).await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateCluster for {identifier}, got {} with body {}",
        created.status, created.body
    );
}

/// `DescribeResize` for this cluster.
async fn describe_resize(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call("DescribeResize", &[("ClusterIdentifier", identifier)])
        .await
}

/// One row of the no-resize table: the case name, the cluster's identifier, and
/// the `ModifyCluster` members that must start no resize.
type NoResizeRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

#[tokio::test]
async fn a_cluster_that_was_never_resized_reports_no_resize() {
    let emulator = Emulator::new();

    let rows: [NoResizeRow; 4] = [
        ("a cluster nothing has been done to", "resize-untouched", &[]),
        (
            "a modify of the maintenance window",
            "resize-window-only",
            &[("PreferredMaintenanceWindow", "wed:04:00-wed:04:30")],
        ),
        (
            "a modify of the snapshot retention period",
            "resize-retention-only",
            &[("AutomatedSnapshotRetentionPeriod", "7")],
        ),
        (
            "a modify that re-states the shape the cluster already has",
            "resize-same-shape",
            &[("ClusterType", "single-node"), ("NumberOfNodes", "1")],
        ),
    ];

    for (case, identifier, modify) in rows {
        create(&emulator, identifier, &[]).await;
        if !modify.is_empty() {
            let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier)];
            params.extend_from_slice(modify);
            let modified = emulator.call("ModifyCluster", &params).await;
            assert_eq!(
                modified.status, 200,
                "case {case:?}: expected HTTP 200 from ModifyCluster, got {} with body {}",
                modified.status, modified.body
            );
        }

        let answered = describe_resize(&emulator, identifier).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (404, Some("ResizeNotFound")),
            "case {case:?}: expected 404 ResizeNotFound, got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        let expected = format!("No Resize found for Cluster {identifier}");
        assert_eq!(
            answered.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            answered.text("Message")
        );
    }
}

#[tokio::test]
async fn an_absent_cluster_is_refused_as_an_absent_cluster() {
    let case = "a cluster this account does not have";
    let emulator = Emulator::new();
    let answered = describe_resize(&emulator, "resize-no-such-cluster").await;
    assert_eq!(
        (answered.status, answered.error_code().as_deref()),
        (404, Some("ClusterNotFound")),
        "case {case:?}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
        answered.status,
        answered.error_code(),
        answered.body
    );
}

/// One row of the shape-change table: what the modify asks for, and the target
/// the resize then reports.
struct Row {
    case: &'static str,
    identifier: &'static str,
    /// The shape the cluster is CREATED with.
    created: &'static [(&'static str, &'static str)],
    /// The shape change that starts the resize.
    modify: &'static [(&'static str, &'static str)],
    /// `TargetNodeType`, `TargetNumberOfNodes` and `TargetClusterType` the resize
    /// must report.
    target_node_type: &'static str,
    target_nodes: &'static str,
    target_cluster_type: &'static str,
    /// `TargetEncryptionType`: `KMS` for an encrypted cluster, `None` otherwise.
    target_encryption: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "growing a multi-node cluster",
        identifier: "resize-grown",
        created: &[("ClusterType", "multi-node"), ("NumberOfNodes", "2")],
        modify: &[("NumberOfNodes", "6")],
        target_node_type: "ra3.xlplus",
        target_nodes: "6",
        target_cluster_type: "multi-node",
        target_encryption: "KMS",
    },
    Row {
        case: "shrinking a multi-node cluster",
        identifier: "resize-shrunk",
        created: &[("ClusterType", "multi-node"), ("NumberOfNodes", "8")],
        modify: &[("NumberOfNodes", "4")],
        target_node_type: "ra3.xlplus",
        target_nodes: "4",
        target_cluster_type: "multi-node",
        target_encryption: "KMS",
    },
    Row {
        case: "changing only the node type",
        identifier: "resize-node-type",
        created: &[("ClusterType", "multi-node"), ("NumberOfNodes", "2")],
        modify: &[("NodeType", "ra3.4xlarge")],
        target_node_type: "ra3.4xlarge",
        target_nodes: "2",
        target_cluster_type: "multi-node",
        target_encryption: "KMS",
    },
    Row {
        case: "moving a single-node cluster to several nodes",
        identifier: "resize-single-to-multi",
        created: &[("ClusterType", "single-node")],
        modify: &[("ClusterType", "multi-node"), ("NumberOfNodes", "3")],
        target_node_type: "ra3.xlplus",
        target_nodes: "3",
        target_cluster_type: "multi-node",
        target_encryption: "KMS",
    },
    Row {
        case: "an unencrypted cluster changing node type and count at once",
        identifier: "resize-unencrypted",
        created: &[
            ("ClusterType", "multi-node"),
            ("NumberOfNodes", "2"),
            ("Encrypted", "false"),
        ],
        modify: &[("NodeType", "dc2.large"), ("NumberOfNodes", "4")],
        target_node_type: "dc2.large",
        target_nodes: "4",
        target_cluster_type: "multi-node",
        target_encryption: "None",
    },
];

#[tokio::test]
async fn a_shape_change_is_a_resize_the_cluster_then_reports() {
    let emulator = Emulator::new();
    // The clock is STOPPED for the whole table, so a resize this case means to
    // read while it is still running stays running however long the calls take,
    // and its completion falls due only where the case advances the clock.
    emulator.stop_clock();

    for row in ROWS {
        let case = row.case;
        create(&emulator, row.identifier, row.created).await;
        // Before the shape change there is nothing to report.
        let before = describe_resize(&emulator, row.identifier).await;
        assert_eq!(
            before.error_code().as_deref(),
            Some("ResizeNotFound"),
            "case {case:?}: expected no resize before the shape change, got body {}",
            before.body
        );

        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", row.identifier)];
        params.extend_from_slice(row.modify);
        let modified = emulator.call("ModifyCluster", &params).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: expected HTTP 200 from ModifyCluster, got {} with body {}",
            modified.status, modified.body
        );

        // The resize is reported from the call that started it, still running.
        let running = describe_resize(&emulator, row.identifier).await;
        assert_eq!(
            running.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeResize, got {} with body {}",
            running.status, running.body
        );
        for (member, expected) in [
            // The modelled values, spelled out: `ResizeProgressMessage$Status`
            // documents "Valid Values: NONE | IN_PROGRESS | FAILED | SUCCEEDED |
            // CANCELLING" and `ResizeInfo$ResizeType` "Returns the value
            // ClassicResize".
            ("Status", "IN_PROGRESS"),
            ("ResizeType", "ClassicResize"),
            ("TargetNodeType", row.target_node_type),
            ("TargetNumberOfNodes", row.target_nodes),
            ("TargetClusterType", row.target_cluster_type),
            ("TargetEncryptionType", row.target_encryption),
        ] {
            assert_eq!(
                running.text(member).as_deref(),
                Some(expected),
                "case {case:?}: expected {member}={expected:?} on the running resize, got \
                 {:?} in body {}",
                running.text(member),
                running.body
            );
        }

        // And once the span the resize was owed has passed it is finished — and
        // still reported, because a cluster holds its LAST resize. The span is
        // crossed by advancing the stopped clock, not by waiting for it.
        emulator.advance_clock(state::PROVISIONING_SPAN);
        let finished = describe_resize(&emulator, row.identifier).await;
        assert_eq!(
            (
                finished.status,
                finished.text("Status").as_deref(),
                finished.text("TargetNumberOfNodes").as_deref(),
                finished.text("EstimatedTimeToCompletionInSeconds").as_deref(),
            ),
            (200, Some("SUCCEEDED"), Some(row.target_nodes), Some("0")),
            "case {case:?}: expected the finished resize to stay reported as SUCCEEDED with \
             nothing left to do, got body {}",
            finished.body
        );

        // The cluster really has the shape the resize was for: the resize record
        // and the cluster cannot disagree about what it became.
        let described = emulator
            .call(
                "DescribeClusters",
                &[("ClusterIdentifier", row.identifier)],
            )
            .await;
        assert_eq!(
            (
                described.text("NodeType").as_deref(),
                described.text("NumberOfNodes").as_deref(),
            ),
            (Some(row.target_node_type), Some(row.target_nodes)),
            "case {case:?}: expected the cluster to carry the resize's target shape, got body \
             {}",
            described.body
        );
    }
}
