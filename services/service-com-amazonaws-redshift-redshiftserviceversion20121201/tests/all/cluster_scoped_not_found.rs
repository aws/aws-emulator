// SPDX-License-Identifier: CC0-1.0

//! Every CLUSTER-SCOPED operation resolves the cluster it names first, end to end
//! over the awsQuery wire.
//!
//! The RULE every case here asserts: an operation whose request names a cluster
//! looks that `ClusterIdentifier` up against this service's store before any other
//! work, and when it resolves to nothing answers `ClusterNotFoundFault` — error
//! code `ClusterNotFound`, HTTP 404 — whatever else the request carried. A cluster
//! that was deleted resolves to nothing like one that never existed, so the
//! refusal is observable through every later call naming it.
//!
//! The tables span what that rule generalises over: WHICH operation (reads,
//! mutations, credential issuance and the two families that dereference a cluster
//! through another resource), and the absent name's spelling, length and case —
//! rather than the one request a recording carried. A cluster that IS there is
//! driven through the same operations, so the refusals are the precondition and
//! not a blanket refusal.
//!
//! The harness (the signed awsQuery call, the poll loop) is shared with
//! [`crate::cluster_lifecycle`], which owns it.

use crate::cluster_lifecycle::Emulator;

/// The error code and status every cluster-scoped operation answers for a
/// `ClusterIdentifier` that resolves to nothing.
const CLUSTER_NOT_FOUND: &str = "ClusterNotFound";
const NOT_FOUND_STATUS: u16 = 404;

/// One row of the operation table: the case name, the action, and the request
/// parameters BESIDE the `ClusterIdentifier` every row carries.
type OperationRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// Every operation of this service whose request names a cluster, with the other
/// members each one needs to reach its handler at all.
///
/// The list is the cluster-scoped surface: the two reads, the writes that change a
/// cluster, the two that suspend and resume its compute, the three that resize it,
/// the four that configure it, the two that issue credentials for its database,
/// the two that report work done on it, the usage limit created on it, the
/// Multi-AZ failover, and the two that dereference a cluster while naming another
/// resource.
const CLUSTER_SCOPED_OPERATIONS: &[OperationRow] = &[
    ("DescribeClusters", "DescribeClusters", &[]),
    (
        "DeleteCluster",
        "DeleteCluster",
        &[("SkipFinalClusterSnapshot", "true")],
    ),
    ("ModifyCluster", "ModifyCluster", &[]),
    ("RebootCluster", "RebootCluster", &[]),
    ("PauseCluster", "PauseCluster", &[]),
    ("ResumeCluster", "ResumeCluster", &[]),
    ("ResizeCluster", "ResizeCluster", &[("NumberOfNodes", "4")]),
    ("CancelResize", "CancelResize", &[]),
    ("DescribeResize", "DescribeResize", &[]),
    ("RotateEncryptionKey", "RotateEncryptionKey", &[]),
    (
        "ModifyClusterMaintenance",
        "ModifyClusterMaintenance",
        &[("DeferMaintenance", "false")],
    ),
    ("ModifyClusterIamRoles", "ModifyClusterIamRoles", &[]),
    (
        "ModifyClusterSnapshotSchedule",
        "ModifyClusterSnapshotSchedule",
        &[("ScheduleIdentifier", "nightly-schedule")],
    ),
    (
        "DescribeClusterDbRevisions",
        "DescribeClusterDbRevisions",
        &[],
    ),
    ("DescribeLoggingStatus", "DescribeLoggingStatus", &[]),
    ("DisableLogging", "DisableLogging", &[]),
    (
        "EnableLogging",
        "EnableLogging",
        &[("BucketName", "audit-logs-bucket")],
    ),
    (
        "GetClusterCredentials",
        "GetClusterCredentials",
        &[("DbUser", "awsuser")],
    ),
    (
        "GetClusterCredentialsWithIAM",
        "GetClusterCredentialsWithIAM",
        &[],
    ),
    (
        "DescribeTableRestoreStatus",
        "DescribeTableRestoreStatus",
        &[],
    ),
    (
        "CreateUsageLimit",
        "CreateUsageLimit",
        &[
            ("FeatureType", "spectrum"),
            ("LimitType", "data-scanned"),
            ("Amount", "100"),
        ],
    ),
    ("FailoverPrimaryCompute", "FailoverPrimaryCompute", &[]),
    (
        "RestoreTableFromClusterSnapshot",
        "RestoreTableFromClusterSnapshot",
        &[
            ("SnapshotIdentifier", "nowhere-snapshot"),
            ("SourceDatabaseName", "dev"),
            ("SourceTableName", "people"),
            ("NewTableName", "people_restored"),
        ],
    ),
    (
        "DescribeNodeConfigurationOptions",
        "DescribeNodeConfigurationOptions",
        &[("ActionType", "resize-cluster")],
    ),
];

/// Call `action` naming `cluster_identifier`, with the row's other parameters.
async fn call_naming(
    emulator: &Emulator,
    action: &str,
    extra: &[(&str, &str)],
    cluster_identifier: &str,
) -> crate::cluster_lifecycle::Answer {
    let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", cluster_identifier)];
    params.extend_from_slice(extra);
    emulator.call(action, &params).await
}

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_cluster_not_found(case: &str, answer: &crate::cluster_lifecycle::Answer) {
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case}: expected status {NOT_FOUND_STATUS}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(CLUSTER_NOT_FOUND),
        "case {case}: expected error code {CLUSTER_NOT_FOUND:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
    // The refusal names what the caller has to fix, so the sentence is not empty:
    // a client that logs the message gets the cluster's own fault, not a bare code.
    assert!(
        answer
            .text("Message")
            .is_some_and(|message| !message.trim().is_empty()),
        "case {case}: expected a message naming the refusal, got body {}",
        answer.body
    );
}

/// A name of exactly `len` characters that satisfies every other part of the
/// documented cluster-identifier syntax: a leading letter, then alphanumerics.
fn name_of_length(len: usize) -> String {
    std::iter::once('c')
        .chain(std::iter::repeat_n('x', len - 1))
        .collect()
}

/// Every cluster-scoped operation answers the cluster's own modelled fault for an
/// identifier this account holds no cluster under — for a table of absent names
/// that spans the spelling, the documented length bounds and the case.
#[tokio::test]
async fn every_cluster_scoped_operation_refuses_an_absent_cluster() {
    let emulator = Emulator::new();
    // One cluster that DOES exist, so the refusals below cannot be an "every
    // request is refused" artefact.
    let present = emulator.create_minimal("present-cluster").await;
    assert_eq!(
        present.status, 200,
        "case seed present-cluster: expected 200, got {} with body {}",
        present.status, present.body
    );

    // The documented identifier syntax is "1 to 63 alphanumeric characters or
    // hyphens" (`smithy_query(query="doc", shape="CreateClusterMessage",
    // member="ClusterIdentifier")`), so both bounds are rows here.
    let absent_names: Vec<(String, String)> = vec![
        ("a plain unknown name".to_string(), "no-such-cluster".to_string()),
        ("the shortest unknown name".to_string(), name_of_length(1)),
        ("the longest unknown name".to_string(), name_of_length(63)),
        (
            "an unknown name in mixed case".to_string(),
            "No-Such-Cluster".to_string(),
        ),
        (
            "a name one character off a stored one".to_string(),
            "present-cluste".to_string(),
        ),
        (
            "a stored name in another case, which names no cluster".to_string(),
            "PRESENT-CLUSTER".to_string(),
        ),
    ];

    for (name_case, identifier) in &absent_names {
        for (operation, action, extra) in CLUSTER_SCOPED_OPERATIONS {
            let case = format!("{operation} with {name_case}");
            let answer = call_naming(&emulator, action, extra, identifier).await;
            assert_cluster_not_found(&case, &answer);
        }
    }

    // The cluster that exists is still readable after the whole table: nothing
    // above wrote anything.
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "present-cluster")])
        .await;
    assert_eq!(
        described.status, 200,
        "case the stored cluster is still served: expected 200, got {} with body {}",
        described.status, described.body
    );
}

/// A DELETED cluster resolves to nothing like one that never existed: every
/// cluster-scoped operation naming it answers the same modelled fault, which is
/// what lets a client branch on "create it" versus "it exists".
#[tokio::test]
async fn a_deleted_cluster_is_absent_from_every_cluster_scoped_operation() {
    let emulator = Emulator::new();
    let identifier = "doomed-cluster";
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    // The cluster is serving before it is deleted, so the delete is of a cluster
    // every operation below could have reached. Waits for the CONDITION.
    emulator
        .poll_until(
            "the cluster is serving",
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some("available"),
        )
        .await;

    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete {identifier}: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    for (operation, action, extra) in CLUSTER_SCOPED_OPERATIONS {
        let case = format!("{operation} against the deleted cluster");
        let answer = call_naming(&emulator, action, extra, identifier).await;
        assert_cluster_not_found(&case, &answer);
    }
}
