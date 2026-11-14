// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE this front door's state-precondition,
//! missing-combination and pagination-marker refusals carry.
//!
//! The rule under test: a refusal's `Message` is the sentence Amazon RDS words for that
//! error class — its fixed text exactly, with only the variable parts the service itself
//! puts in it (the resource identifier, the parameter name, the rejected marker) taken from
//! the request and from this service's state — never prose composed out of the modelled
//! member names and the status the resource happens to be in. Concretely, over the refusals
//! this suite spans:
//!
//! * `ModifyDBInstance` answers `InvalidDBInstanceState` (400, the `awsQueryError` code of
//!   `InvalidDBInstanceStateFault` — `smithy_query(query="operation",
//!   operation="ModifyDBInstance,StopDBInstance,StartDBInstance,CreateDBSnapshot,
//!   RebootDBInstance")`) with one FIXED sentence naming neither the instance nor its
//!   status, while `StopDBInstance`, `StartDBInstance`, `CreateDBSnapshot` and
//!   `RebootDBInstance` answer the SAME fault with four other sentences, each naming the
//!   instance — and only the one that lists the states a reboot is accepted from naming a
//!   status at all.
//! * `FailoverDBCluster` answers `InvalidDBClusterStateFault` (400) for a cluster holding
//!   fewer than two DB instances with the sentence stating that COUNT, naming the cluster
//!   straight after a colon.
//! * `ResetDBClusterParameterGroup` answers `InvalidDBParameterGroupState` (400) naming ONE
//!   single-quoted parameter that still has pending changes, and naming no group.
//! * `RegisterDBProxyTargets` and `DeregisterDBProxyTargets` answer
//!   `InvalidParameterCombination` (400) for a request that names no database with one fixed
//!   sentence that names the two KINDS of database rather than the two members.
//! * Every paginated report answers `InvalidParameterValue` (400) for a `Marker` this
//!   service's codec could not have produced, in the wording its own class of report uses:
//!   the parameter reports ECHO the token the request carried, every other report answers
//!   the fixed sentence that reflects nothing back.
//!
//! Each table spans what the rule generalises over: which status blocks the instance call
//! (still coming up, stopping, stopped, starting) across engine families and instance
//! classes; how few members the cluster holds (none, one) and the spelling the request names
//! it in (as written, upper-cased, its ARN); the name and family of the parameter that has
//! pending changes, how many are pending and how the reset was asked for (every parameter,
//! the pending one by name, another one by name); which proxy call refused and how its
//! request came to name no database; and the shape of the rejected marker (punctuation only,
//! a word, one character, encoded bytes that are no key at all, one carrying characters the
//! XML envelope has to escape) across the reports that serve a `Marker`.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each assertion
//! reads the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    create_db_snapshot_instance_state_message, failover_needs_two_instances_message,
    invalid_marker_message, invalid_parameter_marker_message,
    parameter_has_pending_changes_message,
    reboot_db_instance_state_message, start_db_instance_state_message,
    stop_db_instance_state_message, DB_INSTANCE_STATUS_STARTING, DB_INSTANCE_STATUS_STOPPED,
    DB_INSTANCE_STATUS_STOPPING, MIN_FAILOVER_DB_CLUSTER_INSTANCES,
    MISSING_DB_PROXY_TARGET_IDENTIFIER_MESSAGE, MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_parameter_groups::{
    create_group as create_cluster_parameter_group, modify as modify_cluster_parameter,
};
use crate::db_cluster_subject_operations::{
    cluster_arn, create_available_cluster, create_member, reported_members,
};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, subtrees, text, texts, wait_for_settle, REGION,
};
use crate::db_parameters::{
    create_group as create_db_parameter_group, describe as describe_db_parameters,
};
use crate::db_proxy_target_groups::create_proxy;

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`.
const INVALID_DB_INSTANCE_STATE: &str = "InvalidDBInstanceState";

/// The `awsQueryError` code of `InvalidDBClusterStateFault`.
const INVALID_DB_CLUSTER_STATE: &str = "InvalidDBClusterStateFault";

/// The `awsQueryError` code of `InvalidDBParameterGroupStateFault`.
const INVALID_DB_PARAMETER_GROUP_STATE: &str = "InvalidDBParameterGroupState";

/// The Query API's own code for a combination of members the API does not accept.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// The Query API's own code for a member value the service will not serve, which is what a
/// `Marker` no prior page issued is refused with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The status a DB instance reports while `CreateDBInstance` is still provisioning it.
const DB_INSTANCE_STATUS_CREATING: &str = "creating";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

// ---------------------------------------------------------------------------
// The DB instance state preconditions
// ---------------------------------------------------------------------------

/// How a row puts its instance into a status none of the acting calls can be performed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocked {
    /// Straight after `CreateDBInstance`, inside the span the create owes.
    StillCreating,
    /// `StopDBInstance` has been accepted and its span has not elapsed.
    Stopping,
    /// The stop has run its course and the instance sits in its settled status.
    Stopped,
    /// `StartDBInstance` has been accepted and its span has not elapsed.
    Starting,
}

impl Blocked {
    /// The `DBInstanceStatus` the instance reports when the refused call arrives.
    fn status(self) -> &'static str {
        match self {
            Blocked::StillCreating => DB_INSTANCE_STATUS_CREATING,
            Blocked::Stopping => DB_INSTANCE_STATUS_STOPPING,
            Blocked::Stopped => DB_INSTANCE_STATUS_STOPPED,
            Blocked::Starting => DB_INSTANCE_STATUS_STARTING,
        }
    }
}

/// One row: an instance, and the status it is in when each acting call arrives.
struct InstanceCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
    blocked: Blocked,
}

/// The acting DB instance calls that refuse an instance which is not in a state they can be
/// performed in, each answering the one fault in its own words.
const ACTING_CALLS: [&str; 5] = [
    "ModifyDBInstance",
    "StopDBInstance",
    "StartDBInstance",
    "CreateDBSnapshot",
    "RebootDBInstance",
];

/// The members `action` requires beyond `DBInstanceIdentifier`.
fn extra_members(action: &str, identifier: &str) -> Vec<(String, String)> {
    match action {
        // A real modification, so the refusal is the state precondition answering rather
        // than a request that asked for nothing.
        "ModifyDBInstance" => vec![("BackupRetentionPeriod".to_string(), "7".to_string())],
        "CreateDBSnapshot" => vec![(
            "DBSnapshotIdentifier".to_string(),
            format!("{identifier}-snapshot"),
        )],
        _ => Vec::new(),
    }
}

/// The sentence `action` must answer for `identifier` while it reports `status`.
fn expected_sentence(action: &str, identifier: &str, status: &str) -> String {
    match action {
        "ModifyDBInstance" => MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE.to_string(),
        "StopDBInstance" => stop_db_instance_state_message(identifier),
        "StartDBInstance" => start_db_instance_state_message(identifier),
        "CreateDBSnapshot" => create_db_snapshot_instance_state_message(identifier),
        "RebootDBInstance" => reboot_db_instance_state_message(identifier, status),
        other => panic!("this suite pins no sentence for {other}"),
    }
}

/// `CreateDBInstance` with the members every row carries.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", engine),
            ("DBInstanceClass", class),
            ("AllocatedStorage", "20"),
            ("MasterUsername", "emuadmin"),
        ],
    )
    .await
}

/// The `DBInstanceStatus` `DescribeDBInstances` reports for `identifier`.
async fn instance_status(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    let found = subtrees(&body, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance, got {body}"
    );
    text(&body, "DBInstanceStatus")
}

/// Create `row`'s instance and leave it in the status the row is about.
async fn drive_into_the_blocking_status(router: &MultiServiceRouter, row: &InstanceCase) {
    expect_ok(
        row.case,
        create_instance(router, row.identifier, row.engine, row.class).await,
    );
    if row.blocked == Blocked::StillCreating {
        return;
    }
    wait_for_settle();
    expect_ok(
        row.case,
        call(
            router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", row.identifier)],
        )
        .await,
    );
    if row.blocked == Blocked::Stopping {
        return;
    }
    wait_for_settle();
    if row.blocked == Blocked::Starting {
        expect_ok(
            row.case,
            call(
                router,
                "StartDBInstance",
                &[("DBInstanceIdentifier", row.identifier)],
            )
            .await,
        );
    }
}

#[tokio::test]
async fn each_acting_db_instance_call_words_the_one_state_fault_its_own_way() {
    let rows = [
        InstanceCase {
            case: "a MySQL instance still coming up",
            identifier: "emu-wording-creating",
            engine: "mysql",
            class: "db.t3.small",
            blocked: Blocked::StillCreating,
        },
        InstanceCase {
            case: "a PostgreSQL instance whose stop is still running",
            identifier: "emu-wording-stopping",
            engine: "postgres",
            class: "db.t3.micro",
            blocked: Blocked::Stopping,
        },
        InstanceCase {
            case: "a MariaDB instance whose stop has finished",
            identifier: "emu-wording-stopped",
            engine: "mariadb",
            class: "db.m5.large",
            blocked: Blocked::Stopped,
        },
        InstanceCase {
            case: "a SQL Server instance whose start is still running",
            identifier: "emu-wording-starting",
            engine: "sqlserver-ex",
            class: "db.t3.medium",
            blocked: Blocked::Starting,
        },
    ];

    for row in &rows {
        // One router per row: a stop and a start are state changes, so no row may see
        // another's.
        let router = emulator();
        drive_into_the_blocking_status(&router, row).await;

        // The instance really is in the status this row is about, so each refusal below is
        // the state precondition answering and not some other check.
        let status = instance_status(&router, row.case, row.identifier).await;
        assert_eq!(
            status,
            row.blocked.status(),
            "case {:?}: expected the instance to report {:?} when the call arrives, got \
             {status:?}",
            row.case,
            row.blocked.status()
        );

        for action in ACTING_CALLS {
            // A stopped instance IS started — that acceptance is what the row below shows —
            // so the start is not one of this row's refusals.
            if action == "StartDBInstance" && row.blocked == Blocked::Stopped {
                continue;
            }
            let mut params: Vec<(String, String)> = vec![(
                "DBInstanceIdentifier".to_string(),
                row.identifier.to_string(),
            )];
            params.extend(extra_members(action, row.identifier));
            let case = format!("{} refusing {action}", row.case);
            let body = expect_error(
                &case,
                call(&router, action, &params).await,
                400,
                INVALID_DB_INSTANCE_STATE,
            );
            let message = text(&body, "Message");
            assert_eq!(
                message,
                expected_sentence(action, row.identifier, row.blocked.status()),
                "case {case:?}: expected the sentence Amazon RDS words for {action}, got \
                 {message:?}"
            );
        }

        // The refusals changed nothing: the snapshot the refused `CreateDBSnapshot` named
        // was never taken.
        let snapshots = expect_ok(
            row.case,
            call(&router, "DescribeDBSnapshots", &[] as &[(&str, &str)]).await,
        );
        assert!(
            texts(&snapshots, "DBSnapshotIdentifier").is_empty(),
            "case {:?}: a refused snapshot must leave none behind, got {snapshots}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_available_instance_takes_the_calls_the_blocked_statuses_refuse() {
    // The same calls, against an instance in the one status they are accepted from: this is
    // what shows the refusals above are about the status and not about the operations.
    let router = emulator();
    expect_ok(
        "the available instance",
        create_instance(&router, "emu-wording-available", "mysql", "db.t3.small").await,
    );
    wait_for_settle();

    expect_ok(
        "the snapshot of an available instance",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", "emu-wording-available"),
                ("DBSnapshotIdentifier", "emu-wording-available-snapshot"),
            ],
        )
        .await,
    );
    // The modification comes after the snapshot: an accepted modification moves the instance
    // to `modifying`, which is a status a snapshot is not taken in.
    expect_ok(
        "the modification of an available instance",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-wording-available"),
                ("BackupRetentionPeriod", "7"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // Both calls are read back through the reports that serve them, so the acceptance is in
    // state rather than in the answer only.
    let reported = expect_ok(
        "the instance after the modification",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-wording-available")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "BackupRetentionPeriod"),
        "7",
        "expected the modification to be read back, got {reported}"
    );
    let snapshots = expect_ok(
        "the snapshot report",
        call(&router, "DescribeDBSnapshots", &[] as &[(&str, &str)]).await,
    );
    assert_eq!(
        texts(&snapshots, "DBSnapshotIdentifier"),
        vec!["emu-wording-available-snapshot".to_string()],
        "expected the snapshot to be read back, got {snapshots}"
    );
}

// ---------------------------------------------------------------------------
// The failover precondition
// ---------------------------------------------------------------------------

/// One row of the failover table: a cluster, how many members it holds, and the spelling the
/// request names it in.
struct FailoverCase {
    case: &'static str,
    cluster: &'static str,
    engine: &'static str,
    members: usize,
    /// The value the request's `DBClusterIdentifier` carries, derived from `cluster`: the
    /// member accepts the identifier in any case and the cluster's ARN too.
    named: fn(&str) -> String,
}

/// The identifier as the row wrote it.
fn as_written(identifier: &str) -> String {
    identifier.to_string()
}

/// The identifier upper-cased, which names the same cluster ("This parameter isn't
/// case-sensitive").
fn upper_cased(identifier: &str) -> String {
    identifier.to_ascii_uppercase()
}

#[tokio::test]
async fn a_failover_of_a_cluster_holding_too_few_members_states_the_count_it_needs() {
    let router = emulator();
    let rows = [
        FailoverCase {
            case: "an Aurora MySQL cluster holding no member, named as written",
            cluster: "emu-wording-none",
            engine: "aurora-mysql",
            members: 0,
            named: as_written,
        },
        FailoverCase {
            case: "an Aurora PostgreSQL cluster holding one member, named as written",
            cluster: "emu-wording-one",
            engine: "aurora-postgresql",
            members: 1,
            named: as_written,
        },
        FailoverCase {
            case: "an Aurora MySQL cluster holding one member, named in upper case",
            cluster: "emu-wording-upper",
            engine: "aurora-mysql",
            members: 1,
            named: upper_cased,
        },
        FailoverCase {
            case: "an Aurora PostgreSQL cluster holding no member, named by its ARN",
            cluster: "emu-wording-arn",
            engine: "aurora-postgresql",
            members: 0,
            named: cluster_arn,
        },
    ];

    for row in &rows {
        create_available_cluster(&router, row.cluster, row.engine, &[]).await;
        for index in 0..row.members {
            let member = format!("{}-m{index}", row.cluster);
            let zone = format!("{REGION}{}", char::from(b'a' + index as u8));
            create_member(&router, &member, row.engine, row.cluster, &zone).await;
        }
    }
    wait_for_settle();

    for row in &rows {
        let named = (row.named)(row.cluster);
        let body = expect_error(
            row.case,
            call(
                &router,
                "FailoverDBCluster",
                &[("DBClusterIdentifier", named.as_str())],
            )
            .await,
            400,
            INVALID_DB_CLUSTER_STATE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message,
            failover_needs_two_instances_message(row.cluster),
            "case {:?}: expected the sentence stating the member count a failover needs, got \
             {message:?}",
            row.case
        );
    }

    // A cluster that reaches the count IS failed over, which is what shows the refusals
    // above are about how few members the cluster holds: the second member of the
    // one-member cluster takes the writer role.
    let promoted = "emu-wording-one-m1";
    create_member(
        &router,
        promoted,
        "aurora-postgresql",
        "emu-wording-one",
        &format!("{REGION}b"),
    )
    .await;
    wait_for_settle();
    let answered = expect_ok(
        "the cluster that reaches the count",
        call(
            &router,
            "FailoverDBCluster",
            &[("DBClusterIdentifier", "emu-wording-one")],
        )
        .await,
    );
    let status = text(&answered, "Status");
    assert_eq!(
        status, "failing-over",
        "expected the accepted failover to report the transient status, got {status:?}"
    );
    let members = reported_members(&router, "after the failover", "emu-wording-one").await;
    assert_eq!(
        members.first().map(|(member, _)| member.as_str()),
        Some(promoted),
        "expected the promoted member to be the cluster writer, got {members:?}"
    );
    assert_eq!(
        members.len(),
        MIN_FAILOVER_DB_CLUSTER_INSTANCES,
        "expected the served failover to be the one cluster that reached the count, got \
         {members:?}"
    );
}

// ---------------------------------------------------------------------------
// The pending-change precondition of a cluster parameter group reset
// ---------------------------------------------------------------------------

/// One setting a row writes into its group: the parameter, its value, and the apply method
/// where the request names one.
type PendingSetting = (&'static str, &'static str, Option<&'static str>);

/// One row of the pending-change table.
struct PendingCase {
    case: &'static str,
    /// The `DBParameterGroupFamily` the group is created with.
    family: &'static str,
    /// What the group is modified with before the reset.
    settings: &'static [PendingSetting],
    /// The parameter the refusal must name: the service names ONE, and this one names the
    /// first of the pending parameters in name order.
    named: &'static str,
    /// What the refused reset carries beyond `DBClusterParameterGroupName`.
    reset: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_reset_blocked_by_a_pending_change_names_the_parameter_that_is_pending() {
    let rows = [
        PendingCase {
            case: "a static parameter, reset with every parameter",
            // `docdb5.0` publishes a parameter catalog, in which `tls` is static: "changes
            // to static parameters require a reboot", so the change is pending whatever the
            // caller asked for.
            family: "docdb5.0",
            settings: &[("tls", "disabled", None)],
            named: "tls",
            reset: &[("ResetAllParameters", "true")],
        },
        PendingCase {
            case: "a dynamic parameter deferred to the next reboot, reset by name",
            family: "docdb5.0",
            settings: &[("profiler", "enabled", Some("pending-reboot"))],
            named: "profiler",
            reset: &[("Parameters.Parameter.1.ParameterName", "profiler")],
        },
        PendingCase {
            case: "a parameter of a family that publishes no catalog, reset by name",
            family: "aurora-mysql8.0",
            settings: &[("max_connections", "500", Some("pending-reboot"))],
            named: "max_connections",
            reset: &[("Parameters.Parameter.1.ParameterName", "max_connections")],
        },
        PendingCase {
            case: "two pending parameters, reset with every parameter",
            family: "docdb5.0",
            settings: &[
                ("tls", "disabled", None),
                ("profiler", "enabled", Some("pending-reboot")),
            ],
            named: "profiler",
            reset: &[("ResetAllParameters", "true")],
        },
        PendingCase {
            case: "a reset naming a parameter other than the pending one",
            family: "docdb5.0",
            settings: &[("tls", "disabled", None)],
            named: "tls",
            reset: &[("Parameters.Parameter.1.ParameterName", "profiler")],
        },
    ];

    for row in &rows {
        // One router per row: a reset is a state change, so the rows must not see each
        // other's.
        let router = emulator();
        let group = create_cluster_parameter_group(&router, row.case, "emu-wording-cpg", row.family)
            .await;
        for (parameter, value, apply_method) in row.settings {
            expect_ok(
                row.case,
                modify_cluster_parameter(&router, &group, parameter, value, *apply_method).await,
            );
        }

        let mut params: Vec<(&str, &str)> = vec![("DBClusterParameterGroupName", group.as_str())];
        params.extend_from_slice(row.reset);
        let body = expect_error(
            row.case,
            call(&router, "ResetDBClusterParameterGroup", &params).await,
            400,
            INVALID_DB_PARAMETER_GROUP_STATE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message,
            parameter_has_pending_changes_message(row.named),
            "case {:?}: expected the sentence naming the pending parameter, got {message:?}",
            row.case
        );

        // The refusal changed nothing: every value the row wrote is still in the group.
        let reported = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[("DBClusterParameterGroupName", group.as_str())],
            )
            .await,
        );
        let held = texts(&reported, "ParameterName");
        for (parameter, _, _) in row.settings {
            assert!(
                held.contains(&(*parameter).to_string()),
                "case {:?}: a refused reset must leave {parameter} in the group, got \
                 {reported}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The proxy target calls that name no database
// ---------------------------------------------------------------------------

/// One row of the proxy table: the `DBProxyName` the request carries and the members it
/// carries beyond it — never `DBInstanceIdentifiers` or `DBClusterIdentifiers`.
struct ProxyCase {
    case: &'static str,
    proxy: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn the_proxy_target_calls_refuse_a_request_naming_no_database_in_one_fixed_sentence() {
    let router = emulator();
    create_proxy(&router, "emu-wording-proxy", "MYSQL").await;
    wait_for_settle();

    let rows = [
        ProxyCase {
            case: "neither identifier list",
            proxy: "emu-wording-proxy",
            extra: &[],
        },
        ProxyCase {
            case: "the proxy's own target group named, and no database",
            proxy: "emu-wording-proxy",
            extra: &[("TargetGroupName", "default")],
        },
        ProxyCase {
            // The missing database is answered BEFORE the target group is resolved, so this
            // row carries a name no proxy's group has and still reads the same sentence.
            case: "a target group name no proxy has",
            proxy: "emu-wording-proxy",
            extra: &[("TargetGroupName", "emu-wording-not-default")],
        },
        ProxyCase {
            // And before the proxy itself is resolved.
            case: "a proxy name nothing created",
            proxy: "emu-wording-absent-proxy",
            extra: &[],
        },
    ];

    for row in &rows {
        for action in ["RegisterDBProxyTargets", "DeregisterDBProxyTargets"] {
            let mut params: Vec<(&str, &str)> = vec![("DBProxyName", row.proxy)];
            params.extend_from_slice(row.extra);
            let case = format!("{action} with {}", row.case);
            let body = expect_error(
                &case,
                call(&router, action, &params).await,
                400,
                INVALID_PARAMETER_COMBINATION,
            );
            let message = text(&body, "Message");
            assert_eq!(
                message, MISSING_DB_PROXY_TARGET_IDENTIFIER_MESSAGE,
                "case {case:?}: expected the fixed sentence about the missing database, got \
                 {message:?}"
            );
        }
    }

    // A request that DOES name a database is served and the registration is read back, so
    // the refusals above are about the missing identifier rather than about the operations.
    expect_ok(
        "the registered instance",
        create_instance(&router, "emu-wording-target", "mysql", "db.t3.small").await,
    );
    wait_for_settle();
    let registered = expect_ok(
        "the registration",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-wording-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-wording-target"),
            ],
        )
        .await,
    );
    let target_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-wording-target");
    assert_eq!(
        texts(&registered, "TargetArn"),
        vec![target_arn.clone()],
        "expected the registration to answer with the database it registered, got {registered}"
    );
    let reported = expect_ok(
        "the target report",
        call(
            &router,
            "DescribeDBProxyTargets",
            &[("DBProxyName", "emu-wording-proxy")],
        )
        .await,
    );
    assert_eq!(
        texts(&reported, "TargetArn"),
        vec![target_arn],
        "expected the registered target to be read back, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// The marker every paginated report refuses
// ---------------------------------------------------------------------------

/// The reports this table drives, each `@paginated` on `Marker`, paired with the refusal
/// wording its own class answers a token with. The first is the PARAMETER report a DB
/// parameter group serves, whose refusal echoes the token; the rest need no resource to
/// refuse a marker and answer the fixed sentence.
const MARKER_REPORTS: [(&str, bool); 5] = [
    ("DescribeDBParameters", true),
    ("DescribeDBInstances", false),
    ("DescribeDBClusters", false),
    ("DescribeDBParameterGroups", false),
    ("DescribeOptionGroups", false),
];

#[tokio::test]
async fn an_unissued_marker_is_refused_in_its_own_reports_wording() {
    let router = emulator();
    // A family with no published catalog, so the group's report holds exactly what the group
    // holds — and nothing yet.
    let group = create_db_parameter_group(&router, "the marker rows", "emu-wording-pg", "db2-se")
        .await;

    // `(case, the Marker the request carries)`. The rows differ along the SHAPE of the
    // token, including one whose characters the codec reads but whose bytes are no key, and
    // one whose characters the XML envelope has to escape.
    let markers: Vec<(&str, String)> = vec![
        ("punctuation only", "!!!!".to_string()),
        ("a word", "not-a-marker".to_string()),
        ("one character", "m".to_string()),
        ("encoded bytes that are no key", "/////w==".to_string()),
        (
            "characters the envelope has to escape",
            "a&b<c>\"d'".to_string(),
        ),
    ];

    for (case, marker) in &markers {
        for (report, echoes) in MARKER_REPORTS {
            let case = format!("{report} resumed from {case}");
            let response = if report == "DescribeDBParameters" {
                describe_db_parameters(&router, &group, &[("Marker", marker.clone())]).await
            } else {
                call(&router, report, &[("Marker", marker.as_str())]).await
            };
            let body = expect_error(&case, response, 400, INVALID_PARAMETER_VALUE);
            let message = text(&body, "Message");
            let expected = if echoes {
                invalid_parameter_marker_message(marker)
            } else {
                invalid_marker_message()
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // A `Marker` a prior page DID issue is still served, so the refusals above are about the
    // token and not about the member: the group is filled past one page, the first page
    // hands out a marker and the next page resumes from it.
    let names: Vec<String> = (0..25).map(|index| format!("p_{index:03}")).collect();
    for name in &names {
        expect_ok(
            "filling the group",
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", group.as_str()),
                    ("Parameters.Parameter.1.ParameterName", name.as_str()),
                    ("Parameters.Parameter.1.ParameterValue", "1"),
                ],
            )
            .await,
        );
    }
    let first = expect_ok(
        "the first page",
        describe_db_parameters(&router, &group, &[("MaxRecords", "20".to_string())]).await,
    );
    let marker = text(&first, "Marker");
    assert!(
        !marker.is_empty(),
        "the first page of a report longer than the page must hand out a Marker, got {first}"
    );
    let resumed = expect_ok(
        "the page resumed from the marker the service issued",
        describe_db_parameters(&router, &group, &[("Marker", marker.clone())]).await,
    );
    assert_eq!(
        texts(&resumed, "ParameterName"),
        names[20..].to_vec(),
        "expected the issued marker to resume the report, got {resumed}"
    );
}
