// SPDX-License-Identifier: CC0-1.0

//! What a REPORT answers while the clock still owes the resource a change it has already
//! scheduled.
//!
//! The rule under test: the call that STARTS work answers with the transient status the
//! service documents for that phase, and the FIRST report issued after it is answered with
//! the state the clock owes the resource — the transition already scheduled to fall due
//! within the emulator's transition span is waited for and materialised, so a client
//! polling on an AWS-sized interval converges on its first poll instead of paying a whole
//! interval per wait. A MUTATION arriving inside the span is a different matter: it is
//! answered from the record as it stands and so is still refused by the state gate that
//! goes with the transient status (asserted in the suites that own those gates).
//!
//! The tables below span what the rule generalises over — WHICH resource the report is
//! about (cache cluster, snapshot, replication group, serverless cache, RBAC user, user
//! group, reservation), WHICH phase it is in (being created, being changed, being
//! restarted, going away) and whether the settled state is a STATUS or the resource's
//! ABSENCE. The snapshot round trip of the recorded flow — create, copy, delete the
//! source, delete the copy, delete the cluster — is one row of that class, driven here
//! with exactly ONE read per wait, which is what a client that converges on its first poll
//! does.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and nothing here sleeps: the whole point of the rule is that a client does not
//! have to.

use crate::harness::{params, Emulator, Reply};

/// What the first report issued after the starting call must show.
#[derive(Debug, Clone, Copy)]
enum Settled {
    /// The resource reports this value under `path`.
    Status { path: &'static str, value: &'static str },
    /// The resource is gone: the lookup answers this HTTP status and error code.
    Absent { status: u16, code: &'static str },
    /// The resource is gone from a reader that answers a name it holds nothing for with
    /// an empty page rather than a fault: no node is reported under `path`.
    AbsentFromPage { path: &'static str },
}

/// One row: a call that starts work on a resource, the transient status its own answer
/// must carry, and the report that follows it.
struct Phase {
    case: &'static str,
    /// The call that starts the work, and the request it is made with.
    start: &'static str,
    start_request: Vec<(String, String)>,
    /// Where that call's answer carries the resource's status, and the transient value it
    /// must report.
    transient_path: &'static str,
    transient: &'static str,
    /// The report issued straight after it, and what it must be answered with.
    report: &'static str,
    report_request: Vec<(String, String)>,
    settled: Settled,
}

/// Drive one row: the starting call, then exactly one report.
async fn assert_converges(emulator: &Emulator, phase: &Phase) {
    let started = emulator.call(phase.start, &phase.start_request).await;
    assert_eq!(
        started.status, 200,
        "case {}: {} must answer 200, got {} {}",
        phase.case, phase.start, started.status, started.body
    );
    assert_eq!(
        started.text(phase.transient_path),
        Some(phase.transient),
        "case {}: {} must answer with the transient status {:?}, got {:?}",
        phase.case,
        phase.start,
        phase.transient,
        started.text(phase.transient_path)
    );

    let reported = emulator.call(phase.report, &phase.report_request).await;
    match phase.settled {
        Settled::Status { path, value } => {
            assert_eq!(
                reported.status, 200,
                "case {}: the first {} must answer 200, got {} {}",
                phase.case, phase.report, reported.status, reported.body
            );
            assert_eq!(
                reported.text(path),
                Some(value),
                "case {}: the first {} must be answered with {value:?}, got {:?}",
                phase.case,
                phase.report,
                reported.text(path)
            );
        }
        Settled::Absent { status, code } => {
            assert_eq!(
                (reported.status, reported.error_code()),
                (status, code),
                "case {}: the first {} must find the resource gone ({status} {code}), got {} {}",
                phase.case,
                phase.report,
                reported.status,
                reported.body
            );
        }
        Settled::AbsentFromPage { path } => {
            assert_eq!(
                reported.status, 200,
                "case {}: the first {} must answer 200, got {} {}",
                phase.case, phase.report, reported.status, reported.body
            );
            assert!(
                reported.all(path).is_empty(),
                "case {}: the first {} must report nothing under {path}, got {}",
                phase.case,
                phase.report,
                reported.body
            );
        }
    }
}

/// Create a cluster and read it back once, which is the row the table below repeats: the
/// read is answered with the settled cluster, so the cluster is usable from here on.
async fn available_cluster(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) {
    let mut request = params(&[
        ("CacheClusterId", id),
        ("Engine", "redis"),
        ("CacheNodeType", "cache.t4g.micro"),
        ("NumCacheNodes", "1"),
    ]);
    request.extend(params(extra));
    let created = emulator.call("CreateCacheCluster", &request).await;
    assert_eq!(
        created.status, 200,
        "the cluster {id} must be creatable, got {} {}",
        created.status, created.body
    );
    let described = emulator
        .call("DescribeCacheClusters", &params(&[("CacheClusterId", id)]))
        .await;
    assert_eq!(
        described.text("CacheCluster/CacheClusterStatus"),
        Some("available"),
        "the first read of {id} must be answered with the settled cluster, got {}",
        described.body
    );
}

/// Create one RBAC user with a password, asserting the create succeeded.
async fn create_user(emulator: &Emulator, id: &str, password: &str) {
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("Passwords.member.1", password),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the user {id} must be creatable, got {} {}",
        created.status, created.body
    );
}

#[tokio::test]
async fn the_first_report_after_a_call_that_starts_work_is_answered_settled() {
    let emulator = Emulator::new();

    // The resources the phase rows act on, each already read back once.
    available_cluster(&emulator, "converge-modify", &[]).await;
    available_cluster(&emulator, "converge-reboot", &[]).await;
    available_cluster(&emulator, "converge-snapshot", &[]).await;
    available_cluster(&emulator, "converge-delete", &[]).await;
    create_user(&emulator, "converge-user", "a-password-of-sixteen-plus").await;
    create_user(&emulator, "converge-doomed", "another-password-of-sixteen").await;
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "converge-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the user group must be creatable, got {} {}",
        group.status, group.body
    );
    let group_active = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "converge-group")]),
        )
        .await;
    assert_eq!(
        group_active.text("Status"),
        Some("active"),
        "the first read of the group must be answered with it active, got {}",
        group_active.body
    );
    // The snapshot the copy row copies and the delete row deletes.
    let taken = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("CacheClusterId", "converge-snapshot"),
                ("SnapshotName", "converge-source"),
            ]),
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "the source snapshot must be creatable, got {} {}",
        taken.status, taken.body
    );
    let available = emulator
        .call(
            "DescribeSnapshots",
            &params(&[("SnapshotName", "converge-source")]),
        )
        .await;
    assert_eq!(
        available.text("Snapshot/SnapshotStatus"),
        Some("available"),
        "the first read of the snapshot must be answered with it available, got {}",
        available.body
    );

    // The axis: the resource the report is about, the phase it is in, and whether the
    // settled state is a status or the resource's absence.
    let phases = vec![
        Phase {
            case: "a cache cluster being created",
            start: "CreateCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "converge-create"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t4g.micro"),
                ("NumCacheNodes", "1"),
            ]),
            transient_path: "CacheCluster/CacheClusterStatus",
            transient: "creating",
            report: "DescribeCacheClusters",
            report_request: params(&[("CacheClusterId", "converge-create")]),
            settled: Settled::Status {
                path: "CacheCluster/CacheClusterStatus",
                value: "available",
            },
        },
        Phase {
            case: "a cache cluster being changed",
            start: "ModifyCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "converge-modify"),
                ("CacheNodeType", "cache.m7g.large"),
                ("ApplyImmediately", "true"),
            ]),
            transient_path: "CacheCluster/CacheClusterStatus",
            transient: "modifying",
            report: "DescribeCacheClusters",
            report_request: params(&[("CacheClusterId", "converge-modify")]),
            settled: Settled::Status {
                path: "CacheCluster/CacheClusterStatus",
                value: "available",
            },
        },
        Phase {
            case: "a cache cluster being restarted",
            start: "RebootCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "converge-reboot"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
            transient_path: "CacheCluster/CacheClusterStatus",
            transient: "rebooting cluster nodes",
            report: "DescribeCacheClusters",
            report_request: params(&[("CacheClusterId", "converge-reboot")]),
            settled: Settled::Status {
                path: "CacheCluster/CacheClusterStatus",
                value: "available",
            },
        },
        Phase {
            case: "a snapshot being created",
            start: "CreateSnapshot",
            start_request: params(&[
                ("CacheClusterId", "converge-snapshot"),
                ("SnapshotName", "converge-taken"),
            ]),
            transient_path: "Snapshot/SnapshotStatus",
            transient: "creating",
            report: "DescribeSnapshots",
            report_request: params(&[("SnapshotName", "converge-taken")]),
            settled: Settled::Status {
                path: "Snapshot/SnapshotStatus",
                value: "available",
            },
        },
        Phase {
            case: "a snapshot being copied",
            start: "CopySnapshot",
            start_request: params(&[
                ("SourceSnapshotName", "converge-source"),
                ("TargetSnapshotName", "converge-copy"),
            ]),
            transient_path: "Snapshot/SnapshotStatus",
            transient: "creating",
            report: "DescribeSnapshots",
            report_request: params(&[("SnapshotName", "converge-copy")]),
            settled: Settled::Status {
                path: "Snapshot/SnapshotStatus",
                value: "available",
            },
        },
        Phase {
            case: "a snapshot going away",
            start: "DeleteSnapshot",
            start_request: params(&[("SnapshotName", "converge-source")]),
            transient_path: "Snapshot/SnapshotStatus",
            transient: "deleting",
            report: "DescribeSnapshots",
            report_request: params(&[("SnapshotName", "converge-source")]),
            settled: Settled::AbsentFromPage {
                path: "Snapshot/SnapshotName",
            },
        },
        Phase {
            case: "a cache cluster going away",
            start: "DeleteCacheCluster",
            start_request: params(&[("CacheClusterId", "converge-delete")]),
            transient_path: "CacheCluster/CacheClusterStatus",
            transient: "deleting",
            report: "DescribeCacheClusters",
            report_request: params(&[("CacheClusterId", "converge-delete")]),
            settled: Settled::Absent {
                status: 404,
                code: "CacheClusterNotFound",
            },
        },
        Phase {
            case: "a replication group being created",
            start: "CreateReplicationGroup",
            start_request: params(&[
                ("ReplicationGroupId", "converge-rg"),
                ("ReplicationGroupDescription", "the convergence suite"),
                ("Engine", "redis"),
                ("NumCacheClusters", "1"),
            ]),
            transient_path: "ReplicationGroup/Status",
            transient: "creating",
            report: "DescribeReplicationGroups",
            report_request: params(&[("ReplicationGroupId", "converge-rg")]),
            settled: Settled::Status {
                path: "ReplicationGroup/Status",
                value: "available",
            },
        },
        Phase {
            case: "a serverless cache being created",
            start: "CreateServerlessCache",
            start_request: params(&[
                ("ServerlessCacheName", "converge-serverless"),
                ("Engine", "valkey"),
            ]),
            transient_path: "Status",
            transient: "creating",
            report: "DescribeServerlessCaches",
            report_request: params(&[("ServerlessCacheName", "converge-serverless")]),
            settled: Settled::Status {
                path: "Status",
                value: "available",
            },
        },
        Phase {
            case: "an RBAC user being changed",
            start: "ModifyUser",
            start_request: params(&[
                ("UserId", "converge-user"),
                ("AccessString", "on ~app:* +@read"),
            ]),
            transient_path: "Status",
            transient: "modifying",
            report: "DescribeUsers",
            report_request: params(&[("UserId", "converge-user")]),
            settled: Settled::Status {
                path: "Status",
                value: "active",
            },
        },
        Phase {
            case: "a user group taking a membership change",
            start: "ModifyUserGroup",
            start_request: params(&[
                ("UserGroupId", "converge-group"),
                ("UserIdsToAdd.member.1", "converge-user"),
            ]),
            transient_path: "Status",
            transient: "modifying",
            report: "DescribeUserGroups",
            report_request: params(&[("UserGroupId", "converge-group")]),
            settled: Settled::Status {
                path: "Status",
                value: "active",
            },
        },
        Phase {
            case: "an RBAC user going away",
            start: "DeleteUser",
            start_request: params(&[("UserId", "converge-doomed")]),
            transient_path: "Status",
            transient: "deleting",
            report: "DescribeUsers",
            report_request: params(&[("UserId", "converge-doomed")]),
            settled: Settled::Absent {
                status: 404,
                code: "UserNotFound",
            },
        },
    ];

    for phase in &phases {
        assert_converges(&emulator, phase).await;
    }

    // The membership change the user-group row started is applied together with the
    // status, so the settled group holds the member the change named and reports nothing
    // outstanding — the convergence is of the whole change, not of a status word.
    let described = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "converge-group")]),
        )
        .await;
    assert!(
        described.all("UserIds/member").contains(&"converge-user"),
        "the settled group must hold the member the change added, got {}",
        described.body
    );
    assert!(
        !described.body.contains("PendingChanges"),
        "a settled group reports nothing outstanding, got {}",
        described.body
    );
}

/// `DescribeSnapshots` for one snapshot by name.
async fn snapshot(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("DescribeSnapshots", &params(&[("SnapshotName", name)]))
        .await
}

#[tokio::test]
async fn the_snapshot_round_trip_converges_with_one_read_per_wait() {
    let emulator = Emulator::new();
    // The recorded flow, driven with ONE read per wait: a client that converges on its
    // first poll never issues a second.
    available_cluster(&emulator, "round-trip", &[("SnapshotRetentionLimit", "0")]).await;

    let source = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("CacheClusterId", "round-trip"),
                ("SnapshotName", "round-trip-source"),
            ]),
        )
        .await;
    assert_eq!(
        source.status, 200,
        "CreateSnapshot must answer 200, got {} {}",
        source.status, source.body
    );
    let settled_source = snapshot(&emulator, "round-trip-source").await;
    assert_eq!(
        settled_source.text("Snapshot/SnapshotStatus"),
        Some("available"),
        "the first read must find the source snapshot available, got {}",
        settled_source.body
    );
    // A snapshot of a cluster records the cluster it was taken from, with one
    // `NodeSnapshot` per node of it.
    assert_eq!(
        settled_source.text("Snapshot/CacheClusterId"),
        Some("round-trip"),
        "the snapshot must record the cluster it was taken from, got {}",
        settled_source.body
    );
    assert_eq!(
        settled_source.all("NodeSnapshot/CacheNodeId").len(),
        1,
        "a single-node cluster's snapshot holds exactly one NodeSnapshot, got {}",
        settled_source.body
    );

    // The copy is an independent snapshot of its own, which records the SOURCE cluster.
    let copied = emulator
        .call(
            "CopySnapshot",
            &params(&[
                ("SourceSnapshotName", "round-trip-source"),
                ("TargetSnapshotName", "round-trip-copy"),
            ]),
        )
        .await;
    assert_eq!(
        copied.status, 200,
        "CopySnapshot must answer 200, got {} {}",
        copied.status, copied.body
    );
    assert!(
        copied.text("Snapshot/SnapshotStatus") == Some("creating"),
        "the copy's answer must report the transient status this service gives a copy it \
         has started ({:?} of the modelled creating | copying | available), got {}",
        copied.text("Snapshot/SnapshotStatus"),
        copied.body
    );
    let settled_copy = snapshot(&emulator, "round-trip-copy").await;
    assert_eq!(
        settled_copy.text("Snapshot/SnapshotStatus"),
        Some("available"),
        "the first read must find the copy available, got {}",
        settled_copy.body
    );
    assert_eq!(
        settled_copy.text("Snapshot/CacheClusterId"),
        Some("round-trip"),
        "the copy must record the SOURCE cluster, got {}",
        settled_copy.body
    );
    assert_eq!(
        settled_copy.all("NodeSnapshot/CacheNodeId").len(),
        1,
        "the copy carries the source's node snapshots, got {}",
        settled_copy.body
    );

    // Both snapshots coexist: the copy is not a second view of the source.
    let listed = emulator.call("DescribeSnapshots", &params(&[])).await;
    let names = listed.all("Snapshot/SnapshotName");
    for name in ["round-trip-source", "round-trip-copy"] {
        assert!(
            names.contains(&name),
            "both snapshots must be listed, got {names:?}"
        );
    }

    // Deleting the source removes it, and the copy survives it untouched.
    let deleted = emulator
        .call(
            "DeleteSnapshot",
            &params(&[("SnapshotName", "round-trip-source")]),
        )
        .await;
    assert_eq!(
        deleted.text("Snapshot/SnapshotStatus"),
        Some("deleting"),
        "DeleteSnapshot must answer with the snapshot on its way out, got {}",
        deleted.body
    );
    let gone = snapshot(&emulator, "round-trip-source").await;
    assert!(
        gone.all("Snapshot/SnapshotName").is_empty(),
        "the first read after the delete must not find the source, got {} {}",
        gone.status,
        gone.body
    );
    let survivor = snapshot(&emulator, "round-trip-copy").await;
    assert_eq!(
        survivor.text("Snapshot/SnapshotStatus"),
        Some("available"),
        "the copy must survive its source's deletion, still available, got {}",
        survivor.body
    );
    assert_eq!(
        survivor.text("Snapshot/CacheClusterId"),
        Some("round-trip"),
        "the survivor still records the cluster it was taken from, got {}",
        survivor.body
    );

    // And the way out: the copy, then the cluster.
    let deleted_copy = emulator
        .call(
            "DeleteSnapshot",
            &params(&[("SnapshotName", "round-trip-copy")]),
        )
        .await;
    assert_eq!(
        deleted_copy.text("Snapshot/SnapshotStatus"),
        Some("deleting"),
        "DeleteSnapshot must answer with the copy on its way out, got {}",
        deleted_copy.body
    );
    let copy_gone = snapshot(&emulator, "round-trip-copy").await;
    assert!(
        copy_gone.all("Snapshot/SnapshotName").is_empty(),
        "the first read after the delete must not find the copy, got {} {}",
        copy_gone.status,
        copy_gone.body
    );
    let removed = emulator
        .call(
            "DeleteCacheCluster",
            &params(&[("CacheClusterId", "round-trip")]),
        )
        .await;
    assert_eq!(
        removed.text("CacheCluster/CacheClusterStatus"),
        Some("deleting"),
        "DeleteCacheCluster must answer with the cluster on its way out, got {}",
        removed.body
    );
    let cluster_gone = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "round-trip")]),
        )
        .await;
    assert_eq!(
        (cluster_gone.status, cluster_gone.error_code()),
        (404, "CacheClusterNotFound"),
        "the first read after the delete must not find the cluster, got {} {}",
        cluster_gone.status,
        cluster_gone.body
    );
}
