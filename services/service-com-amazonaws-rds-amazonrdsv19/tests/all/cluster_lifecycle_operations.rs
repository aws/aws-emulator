// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB-cluster lifecycle operations and the promotion of a global
//! database's primary: `StartDBCluster`, `StopDBCluster`, `RebootDBCluster`,
//! `BacktrackDBCluster`, `FailoverGlobalCluster` and `SwitchoverGlobalCluster`.
//!
//! Every call goes over the wire through `db_instances::emulator`, as a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on.
//!
//! The rules under test:
//!
//! * An operation that serves ONE kind of DB cluster refuses a `DBClusterIdentifier` naming
//!   a cluster of the other kind as a value the parameter does not accept: the start and the
//!   stop "only appl[y] to Aurora DB clusters", the reboot is for "a non-Aurora Multi-AZ DB
//!   cluster", and the backtrack "applies only to Aurora MySQL DB clusters".
//! * Each of them acts only on a cluster in a status it can act in, and reports the status
//!   the cluster is in against the statuses it needs; a cluster the request names and the
//!   store does not hold is the modelled `DBClusterNotFoundFault`.
//! * An accepted call is observable: the cluster and its member instances report the
//!   transient status the clock then settles, and a backtrack rewinds the cluster to a
//!   consistent time it kept, which it reports as `BacktrackTo`.
//! * Promoting a global database's primary promotes a SECONDARY member of it: an unknown
//!   global database and an unknown target are the two 404s, a target that is not a
//!   promotable member of that global database is an `InvalidParameterCombination`, and an
//!   accepted promotion makes the target the global database's writer.
//!
//! The tables below span what each rule generalises over: a row per operation for the
//! cluster-kind refusal and for the not-found fault, both promotion operations against every
//! refusal they share, and the statuses each lifecycle call is and is not accepted from.

use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text, texts,
    wait_for_settle, REGION};
use smithy_server_runtime::MultiServiceRouter;

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The canonical account id every generated emulator presents, for the ARNs a promotion
/// target is named by.
const ACCOUNT: &str = "000000000000";

/// Create an Aurora DB cluster and wait for it to settle `available`.
async fn an_aurora_cluster(router: &MultiServiceRouter, identifier: &str, extra: &[(&str, String)]) {
    let mut settings: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", "aurora-mysql".to_string()),
        ("MasterUsername", "clusteradmin".to_string()),
        ("MasterUserPassword", "a-strong-password-1".to_string()),
    ];
    // A member the case names of its own REPLACES this helper's value for it, rather than
    // being sent beside it: one call carries one value per member.
    settings.retain(|(member, _)| !extra.iter().any(|(named, _)| named == member));
    settings.extend(extra.iter().cloned());
    expect_ok(identifier, call(router, "CreateDBCluster", &settings).await);
    wait_for_settle();
}

/// Create a Multi-AZ DB cluster — the kind the reboot serves — and wait for it to settle.
///
/// The four settings that cluster type is required to carry are what `CreateDBCluster`
/// documents for it, with `Iops` inside the documented ratio of the storage amount.
async fn a_multi_az_cluster(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        identifier,
        call(
            router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => identifier,
                "Engine" => "mysql",
                "MasterUsername" => "clusteradmin",
                "MasterUserPassword" => "a-strong-password-1",
                "AllocatedStorage" => 100,
                "DBClusterInstanceClass" => "db.m5d.large",
                "Iops" => 1000,
                "StorageType" => "io1",
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// The one `<DBCluster>` `DescribeDBClusters` reports for `identifier`.
async fn describe_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBClusters",
            &params!["DBClusterIdentifier" => identifier],
        )
        .await,
    );
    let found = subtrees(&body, "DBCluster");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBCluster, got {found:?}"
    );
    found.into_iter().next().expect("checked just above")
}

/// The `Error/Message` of a refused call.
fn error_message(body: &str) -> String {
    text(body, "Message")
}

/// One row of the cluster-kind table: an operation, the cluster kind it does NOT serve, and
/// the parameters the call carries beyond the identifier.
struct KindCase {
    action: &'static str,
    /// The cluster this operation cannot act on.
    identifier: &'static str,
    /// Whether that cluster is an Aurora one (the reboot's refusal) or a Multi-AZ one (the
    /// start's and the stop's).
    aurora: bool,
    extra: &'static [(&'static str, &'static str)],
}

const KIND_CASES: &[KindCase] = &[
    KindCase {
        action: "StartDBCluster",
        identifier: "emu-kind-multi-az",
        aurora: false,
        extra: &[],
    },
    KindCase {
        action: "StopDBCluster",
        identifier: "emu-kind-multi-az",
        aurora: false,
        extra: &[],
    },
    KindCase {
        action: "RebootDBCluster",
        identifier: "emu-kind-aurora",
        aurora: true,
        extra: &[],
    },
    KindCase {
        action: "BacktrackDBCluster",
        identifier: "emu-kind-aurora-pg",
        aurora: true,
        extra: &[("BacktrackTo", "2020-01-01T00:00:00Z")],
    },
];

#[tokio::test]
async fn an_operation_that_serves_one_cluster_kind_refuses_the_other() {
    let router = emulator();
    a_multi_az_cluster(&router, "emu-kind-multi-az").await;
    an_aurora_cluster(&router, "emu-kind-aurora", &[]).await;
    // The backtrack's kind is narrower than "Aurora": it applies to Aurora MySQL only, so
    // an Aurora PostgreSQL cluster is the other kind for it.
    an_aurora_cluster(
        &router,
        "emu-kind-aurora-pg",
        &[("Engine", "aurora-postgresql".to_string())],
    )
    .await;

    for case in KIND_CASES {
        let mut settings: Vec<(&str, String)> =
            vec![("DBClusterIdentifier", case.identifier.to_string())];
        for (member, value) in case.extra {
            settings.push((member, (*value).to_string()));
        }
        let body = expect_error(
            case.action,
            call(&router, case.action, &settings).await,
            400,
            "InvalidParameterValue",
        );
        let message = error_message(&body);
        assert!(
            !message.is_empty(),
            "case {:?}: the refusal must state why, got {message:?}",
            case.action
        );
        // The cluster is left exactly as it was: the refusal changed nothing.
        let read_back = describe_cluster(&router, case.action, case.identifier).await;
        assert_eq!(
            text(&read_back, "Status"),
            "available",
            "case {:?}: a refused call must leave the cluster available, got {read_back}",
            case.action
        );
        let _ = case.aurora;
    }
}

/// Every one of these operations answers the modelled not-found fault for a cluster the
/// store does not hold: `(action, extra members)`.
const NOT_FOUND_CASES: &[(&str, &[(&str, &str)])] = &[
    ("StartDBCluster", &[]),
    ("StopDBCluster", &[]),
    ("RebootDBCluster", &[]),
    (
        "BacktrackDBCluster",
        &[("BacktrackTo", "2020-01-01T00:00:00Z")],
    ),
];

#[tokio::test]
async fn a_cluster_the_store_does_not_hold_is_the_modelled_not_found_fault() {
    let router = emulator();
    for (action, extra) in NOT_FOUND_CASES {
        let mut settings: Vec<(&str, String)> =
            vec![("DBClusterIdentifier", "emu-absent-cluster".to_string())];
        for (member, value) in *extra {
            settings.push((member, (*value).to_string()));
        }
        let body = expect_error(
            action,
            call(&router, action, &settings).await,
            404,
            "DBClusterNotFoundFault",
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {action:?}: the fault must carry a message, got {body}"
        );
    }
}

#[tokio::test]
async fn a_stopped_aurora_cluster_is_started_and_its_members_come_back_up() {
    let router = emulator();
    an_aurora_cluster(&router, "emu-start-cluster", &[]).await;
    expect_ok(
        "a member instance",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-start-member",
                "Engine" => "aurora-mysql",
                "DBInstanceClass" => "db.t3.medium",
                "DBClusterIdentifier" => "emu-start-cluster",
            ],
        )
        .await,
    );
    wait_for_settle();

    // A running cluster has nothing to start: the refusal states the status it is in and
    // the status a start needs.
    let refused = expect_error(
        "start a running cluster",
        call(
            &router,
            "StartDBCluster",
            &params!["DBClusterIdentifier" => "emu-start-cluster"],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = error_message(&refused);
    assert!(
        message.contains("available") && message.contains("stopped"),
        "the refusal must name the status the cluster is in and the one a start needs, got \
         {message:?}"
    );

    expect_ok(
        "stop",
        call(
            &router,
            "StopDBCluster",
            &params!["DBClusterIdentifier" => "emu-start-cluster"],
        )
        .await,
    );
    wait_for_settle();
    let started = expect_ok(
        "start",
        call(
            &router,
            "StartDBCluster",
            &params!["DBClusterIdentifier" => "emu-start-cluster"],
        )
        .await,
    );
    assert_eq!(
        text(&started, "Status"),
        "starting",
        "an accepted start answers with the cluster starting, got {started}"
    );
    wait_for_settle();
    let read_back = describe_cluster(&router, "after the start", "emu-start-cluster").await;
    assert_eq!(
        text(&read_back, "Status"),
        "available",
        "the cluster must settle back to available, got {read_back}"
    );
    // "When you stop a DB cluster, its DB instances are also stopped": the start brings the
    // members back up with it.
    let member = expect_ok(
        "the member after the start",
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-start-member"],
        )
        .await,
    );
    assert_eq!(
        text(&member, "DBInstanceStatus"),
        "available",
        "the member instance must come back up with the cluster, got {member}"
    );
}

#[tokio::test]
async fn a_multi_az_cluster_is_rebooted_and_settles_back() {
    let router = emulator();
    a_multi_az_cluster(&router, "emu-reboot-cluster").await;
    let rebooting = expect_ok(
        "reboot",
        call(
            &router,
            "RebootDBCluster",
            &params!["DBClusterIdentifier" => "emu-reboot-cluster"],
        )
        .await,
    );
    assert_eq!(
        text(&rebooting, "Status"),
        "rebooting",
        "an accepted reboot answers with the cluster rebooting, got {rebooting}"
    );
    // A second reboot while the first is running is refused for the status the cluster is
    // in, which is what a caller polling on it reads.
    let refused = expect_error(
        "reboot a rebooting cluster",
        call(
            &router,
            "RebootDBCluster",
            &params!["DBClusterIdentifier" => "emu-reboot-cluster"],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    assert!(
        error_message(&refused).contains("rebooting"),
        "the refusal must name the status the cluster is in, got {refused}"
    );
    wait_for_settle();
    let read_back = describe_cluster(&router, "after the reboot", "emu-reboot-cluster").await;
    assert_eq!(
        text(&read_back, "Status"),
        "available",
        "the cluster must settle back to available, got {read_back}"
    );
}

#[tokio::test]
async fn a_backtrack_rewinds_an_aurora_mysql_cluster_to_a_consistent_time_it_kept() {
    let router = emulator();
    // Backtracking disabled is the documented default, so a cluster created without a
    // window has no time it can be rewound to.
    an_aurora_cluster(&router, "emu-backtrack-off", &[]).await;
    let refused = expect_error(
        "backtrack with backtracking disabled",
        call(
            &router,
            "BacktrackDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-backtrack-off",
                "BacktrackTo" => "2020-01-01T00:00:00Z",
            ],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    assert!(
        !error_message(&refused).is_empty(),
        "the refusal must state why, got {refused}"
    );
    let read_back = describe_cluster(&router, "backtracking off", "emu-backtrack-off").await;
    assert!(
        subtrees(&read_back, "EarliestBacktrackTime").is_empty(),
        "a cluster with backtracking off reports no earliest backtrack time, got {read_back}"
    );

    an_aurora_cluster(
        &router,
        "emu-backtrack-on",
        &[("BacktrackWindow", "3600".to_string())],
    )
    .await;
    let enabled = describe_cluster(&router, "backtracking on", "emu-backtrack-on").await;
    assert_eq!(
        text(&enabled, "BacktrackWindow"),
        "3600",
        "the window the create asked for must read back, got {enabled}"
    );
    let earliest = text(&enabled, "EarliestBacktrackTime");
    assert!(
        earliest.ends_with('Z'),
        "a cluster with backtracking on reports the earliest time it can be rewound to, got \
         {earliest:?}"
    );

    // "Can't contain a timestamp set in the future."
    expect_error(
        "backtrack to a future time",
        call(
            &router,
            "BacktrackDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-backtrack-on",
                "BacktrackTo" => "2099-01-01T00:00:00Z",
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    // A time before the earliest the cluster kept, without the member that asks for the
    // earliest instead: "an error occurs".
    expect_error(
        "backtrack before the earliest time",
        call(
            &router,
            "BacktrackDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-backtrack-on",
                "BacktrackTo" => "2000-01-01T00:00:00Z",
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    // The same time WITH that member is the earliest point the cluster kept, and the
    // backtrack reports the time it rewound to.
    let backtracked = expect_ok(
        "backtrack to the earliest time",
        call(
            &router,
            "BacktrackDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-backtrack-on",
                "BacktrackTo" => "2000-01-01T00:00:00Z",
                "UseEarliestTimeOnPointInTimeUnavailable" => true,
            ],
        )
        .await,
    );
    assert_eq!(
        text(&backtracked, "Status"),
        "completed",
        "a backtrack this service has applied reports completed, got {backtracked}"
    );
    assert_eq!(
        text(&backtracked, "BacktrackTo"),
        earliest,
        "the backtrack must report the consistent time it rewound to, got {backtracked}"
    );
    assert_eq!(
        text(&backtracked, "DBClusterIdentifier"),
        "emu-backtrack-on",
        "the backtrack names the cluster it was performed on, got {backtracked}"
    );
    assert!(
        !text(&backtracked, "BacktrackIdentifier").is_empty(),
        "a backtrack carries the identifier the service assigned it, got {backtracked}"
    );
    // A time the cluster DOES hold a point at or before is accepted on its own, with no
    // need for the earliest-time member.
    expect_ok(
        "backtrack to a time the cluster kept",
        call(
            &router,
            "BacktrackDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-backtrack-on",
                "BacktrackTo" => &earliest,
            ],
        )
        .await,
    );
}

/// The two operations that promote a global database's primary: both perform the one
/// promotion, so both are held to every refusal of it.
const PROMOTIONS: &[&str] = &["FailoverGlobalCluster", "SwitchoverGlobalCluster"];

/// The ARN a promotion target is named by, which is what the member documents ("Use the
/// Amazon Resource Name (ARN) for the identifier so that Aurora can locate the cluster in
/// its Amazon Web Services Region").
fn cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

#[tokio::test]
async fn promoting_a_global_databases_primary_is_refused_for_every_target_it_cannot_promote() {
    let router = emulator();
    expect_ok(
        "a global database",
        call(
            &router,
            "CreateGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-refusals",
                "Engine" => "aurora-mysql",
            ],
        )
        .await,
    );
    an_aurora_cluster(
        &router,
        "emu-global-primary",
        &[(
            "GlobalClusterIdentifier",
            "emu-global-refusals".to_string(),
        )],
    )
    .await;
    // A cluster of this account that is NOT a member of that global database.
    an_aurora_cluster(&router, "emu-global-outsider", &[]).await;

    for action in PROMOTIONS {
        // A global database nothing created.
        expect_error(
            action,
            call(
                &router,
                action,
                &params![
                    "GlobalClusterIdentifier" => "emu-global-absent",
                    "TargetDbClusterIdentifier" => cluster_arn("emu-global-primary"),
                ],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );
        // A target that is no DB cluster at all.
        let missing = expect_error(
            action,
            call(
                &router,
                action,
                &params![
                    "GlobalClusterIdentifier" => "emu-global-refusals",
                    "TargetDbClusterIdentifier" => cluster_arn("emu-global-nothing"),
                ],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
        assert!(
            error_message(&missing).contains(&cluster_arn("emu-global-nothing")),
            "case {action:?}: the fault must name the target the request wrote, got {missing}"
        );
        // A DB cluster that is not a member of this global database, and the member that is
        // already its primary: neither is a secondary this global database can promote.
        for target in ["emu-global-outsider", "emu-global-primary"] {
            expect_error(
                action,
                call(
                    &router,
                    action,
                    &params![
                        "GlobalClusterIdentifier" => "emu-global-refusals",
                        "TargetDbClusterIdentifier" => cluster_arn(target),
                    ],
                )
                .await,
                400,
                "InvalidParameterCombination",
            );
        }
    }
}

#[tokio::test]
async fn a_promotion_makes_the_target_the_global_databases_writer() {
    let router = emulator();
    expect_ok(
        "a global database",
        call(
            &router,
            "CreateGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-promote",
                "Engine" => "aurora-mysql",
            ],
        )
        .await,
    );
    for identifier in ["emu-global-writer", "emu-global-reader"] {
        an_aurora_cluster(
            &router,
            identifier,
            &[("GlobalClusterIdentifier", "emu-global-promote".to_string())],
        )
        .await;
    }
    // The first member is the primary, so the second is the secondary a promotion promotes.
    let promoted = expect_ok(
        "switch over",
        call(
            &router,
            "SwitchoverGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-promote",
                "TargetDbClusterIdentifier" => cluster_arn("emu-global-reader"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&promoted, "Status"),
        "switching-over",
        "an accepted switchover answers with the global database switching over, got \
         {promoted}"
    );
    let members = subtrees(&promoted, "GlobalClusterMember");
    assert_eq!(
        members.len(),
        2,
        "the global database keeps both members through the promotion, got {promoted}"
    );
    assert!(
        members[0].contains(&cluster_arn("emu-global-reader"))
            && text(&members[0], "IsWriter") == "true",
        "the promoted cluster must be the global database's writer, got {promoted}"
    );
    assert!(
        text(&members[1], "IsWriter") == "false",
        "the cluster it replaced must be a secondary, got {promoted}"
    );
    wait_for_settle();
    // The promotion is in state, not in the one answer: a read of the global database
    // reports the same writer, and the global database settles back to available.
    let read_back = expect_ok(
        "the global database after the promotion",
        call(
            &router,
            "DescribeGlobalClusters",
            &params!["GlobalClusterIdentifier" => "emu-global-promote"],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "Status"),
        "available",
        "the global database must settle back to available, got {read_back}"
    );
    let arns = texts(&read_back, "DBClusterArn");
    assert_eq!(
        arns.first().map(String::as_str),
        Some(cluster_arn("emu-global-reader").as_str()),
        "the promoted cluster must still be the writer a later read reports, got {read_back}"
    );
    // A promotion while one is running is refused for the global database's own state.
    expect_ok(
        "fail back",
        call(
            &router,
            "FailoverGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-promote",
                "TargetDbClusterIdentifier" => cluster_arn("emu-global-writer"),
                "AllowDataLoss" => true,
            ],
        )
        .await,
    );
    expect_error(
        "a second promotion while one is running",
        call(
            &router,
            "FailoverGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-promote",
                "TargetDbClusterIdentifier" => cluster_arn("emu-global-reader"),
                "AllowDataLoss" => true,
            ],
        )
        .await,
        400,
        "InvalidGlobalClusterStateFault",
    );
}

#[tokio::test]
async fn a_failover_cannot_both_allow_data_loss_and_switch_over() {
    let router = emulator();
    // "Can't be specified together with the Switchover parameter": a request carrying both
    // is refused before anything is looked up, so the global database need not exist.
    expect_error(
        "both members",
        call(
            &router,
            "FailoverGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-global-absent",
                "TargetDbClusterIdentifier" => cluster_arn("emu-global-absent-target"),
                "AllowDataLoss" => true,
                "Switchover" => true,
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
}
