// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the preconditions the maintenance-action and DB snapshot front
//! doors hold their callers to, and for the CLASS of answer each refusal belongs to.
//!
//! The rules under test, each asserted over a table that spans the class rather than one
//! request:
//!
//! * `ApplyPendingMaintenanceAction` acts on the resource its `ResourceIdentifier` names,
//!   and an identifier that resolves to no resource of the account is refused on the MEMBER
//!   that carried it: HTTP 400 `InvalidParameterValue`, carrying the fixed text `Unknown
//!   resource: ` and the identifier the request supplied, as it supplied it — whichever
//!   resource kind the ARN names (a DB instance, a DB cluster, another resource type,
//!   another vendor) and whether the value is an ARN at all. The identifier RESOLVES when it
//!   is the ARN a DB cluster or a DB instance of the request's region reports for itself,
//!   which is what the rows below vary: the resource kind, the ARN's account, region and
//!   resource type, and a value that is no ARN at all.
//! * `CreateDBSnapshot` refuses a source DB instance that is not in the `available` state
//!   with `InvalidDBInstanceState` (HTTP 400) and the sentence the service words for that
//!   class, whatever stage of its lifecycle the instance is in.
//! * `DescribeDBSnapshots` naming a `DBSnapshotIdentifier` that this region holds no record
//!   of answers its modelled `DBSnapshotNotFound` (HTTP 404), rather than reporting an
//!   empty page as though the snapshot existed.
//!
//! Every call goes over the wire through the harness `db_instances.rs` keeps, so each
//! assertion reads the XML the emulator itself answered with; the cluster and instance the
//! positive rows resolve against are stood up through the same implemented operations a
//! client would use.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    create_db_snapshot_instance_state_message, DB_INSTANCE_STATUS_STOPPED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_cluster_subject_operations::{cluster_arn, create_available_cluster};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents, and so the account every
/// ARN this service mints belongs to.
const ACCOUNT: &str = "000000000000";

/// The Query API code an identifier that resolves to no resource is refused with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The fixed text that refusal carries, written out here rather than read from the code
/// under test: the wording is part of what this suite pins, so a drift away from it must
/// fail.
const UNKNOWN_RESOURCE: &str = "Unknown resource: ";

/// The ARN a stored DB instance of [`REGION`] reports for itself (`db` is the resource type
/// RDS gives a DB instance in its ARN format).
fn instance_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{identifier}")
}

/// `CreateDBInstance`, left in the state the create starts it in.
async fn create_instance(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    call(
        router,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", "postgres"),
            ("DBInstanceClass", "db.t3.micro"),
        ],
    )
    .await
}

/// Create a DB instance and wait until the clock has made it `available`.
async fn create_available_instance(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        &format!("create {identifier}"),
        create_instance(router, identifier).await,
    );
    wait_for_settle();
}

/// `ApplyPendingMaintenanceAction` over `resource`, with one documented
/// `ApplyAction` / `OptInType` pairing.
async fn apply(
    router: &MultiServiceRouter,
    resource: &str,
    action: &str,
    opt_in: &str,
) -> (u16, String) {
    call(
        router,
        "ApplyPendingMaintenanceAction",
        &[
            ("ResourceIdentifier", resource),
            ("ApplyAction", action),
            ("OptInType", opt_in),
        ],
    )
    .await
}

// ---------------------------------------------------------------------------
// ApplyPendingMaintenanceAction — the identifier is a parameter value
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_maintenance_action_resource_is_refused_on_the_member_that_carried_it() {
    let router = emulator();

    // One cluster and one instance that DO resolve, so every row below fails for the
    // identifier it names and not because this service holds no resource at all.
    create_available_cluster(&router, "emu-maint-guard-cluster", "aurora-postgresql", &[]).await;
    create_available_instance(&router, "emu-maint-guard-instance").await;

    // The shapes an identifier that names no resource of this account can take. The
    // `ApplyAction` / `OptInType` pairings rotate through the values those two members
    // publish, so no row can pass on one pairing being special — and the last row carries an
    // action AWS does not publish, which pins that the identifier is decided FIRST. Every
    // row is refused on the MEMBER, with the identifier the request carried, so the rows are
    // the message's one variable part.
    //
    // (case, the identifier, ApplyAction, OptInType)
    let rows: &[(&str, String, &str, &str)] = &[
        (
            "a DB instance ARN for an instance that was never created",
            instance_arn("emu-maint-guard-absent"),
            "system-update",
            "immediate",
        ),
        (
            "a DB cluster ARN for a cluster that was never created",
            cluster_arn("emu-maint-guard-absent-cluster"),
            "db-upgrade",
            "next-maintenance",
        ),
        (
            "a value that is no ARN at all",
            "emu-maint-guard-instance".to_string(),
            "os-upgrade",
            "undo-opt-in",
        ),
        (
            "the stored instance's name under the cluster resource type",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-maint-guard-instance"),
            "hardware-maintenance",
            "immediate",
        ),
        (
            "a DB snapshot ARN, which carries no maintenance action",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:emu-maint-guard-snap"),
            "ca-certificate-rotation",
            "next-maintenance",
        ),
        (
            "the stored instance's ARN in another account",
            format!("arn:aws:rds:{REGION}:210987654321:db:emu-maint-guard-instance"),
            "system-update",
            "immediate",
        ),
        (
            "the stored instance's ARN in another region",
            "arn:aws:rds:eu-west-1:000000000000:db:emu-maint-guard-instance".to_string(),
            "db-upgrade",
            "immediate",
        ),
        (
            "the stored cluster's ARN in another region",
            "arn:aws:rds:eu-west-1:000000000000:cluster:emu-maint-guard-cluster".to_string(),
            "db-upgrade",
            "immediate",
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance:i-0a1b2c3d4e5f60718"),
            "os-upgrade",
            "next-maintenance",
        ),
        (
            "an absent resource named with an unpublished action",
            instance_arn("emu-maint-guard-absent-too"),
            "reboot-now",
            "immediate",
        ),
    ];

    for (case, resource, action, opt_in) in rows {
        // Every row is a value the member does not accept, and the refusal names the
        // identifier the REQUEST carried, so a caller that logs the message still has the
        // value it sent.
        let expected_message = format!("{UNKNOWN_RESOURCE}{resource}");
        let body = expect_error(
            case,
            apply(&router, resource, action, opt_in).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got \
             {message:?}"
        );
        // The awsQuery envelope types a client error `Sender`, and a client that branches
        // on that must not read `Receiver` for a value it can correct itself.
        let fault_type = text(&body, "Type");
        assert_eq!(
            fault_type, "Sender",
            "case {case:?}: expected a Sender fault, got {fault_type:?} in {body}"
        );
        assert!(
            !text(&body, "RequestId").is_empty(),
            "case {case:?}: expected the error envelope to carry a RequestId, got {body}"
        );
    }

    // And an identifier that DOES resolve is answered with that resource's own pending
    // action list — empty, because nothing in this service schedules a maintenance action.
    // Both resource kinds resolve, and surrounding whitespace is not part of the value.
    let resolving: &[(&str, String, String)] = &[
        (
            "the stored DB instance's ARN",
            instance_arn("emu-maint-guard-instance"),
            instance_arn("emu-maint-guard-instance"),
        ),
        (
            "the stored DB cluster's ARN",
            cluster_arn("emu-maint-guard-cluster"),
            cluster_arn("emu-maint-guard-cluster"),
        ),
        (
            "the stored DB instance's ARN with surrounding whitespace",
            format!("  {}  ", instance_arn("emu-maint-guard-instance")),
            instance_arn("emu-maint-guard-instance"),
        ),
    ];
    for (case, resource, expected_arn) in resolving {
        let body = expect_ok(case, apply(&router, resource, "system-update", "immediate").await);
        let reported = text(&body, "ResourceIdentifier");
        assert_eq!(
            &reported, expected_arn,
            "case {case:?}: expected the report to name the resolved resource, got {body}"
        );
        assert!(
            texts(&body, "PendingMaintenanceAction").is_empty(),
            "case {case:?}: nothing here schedules a maintenance action, so none may be \
             reported as pending, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The DB snapshot preconditions the same workflow depends on
// ---------------------------------------------------------------------------

/// Assert that `CreateDBSnapshot` refuses `identifier` for its state, with the sentence the
/// service words that class with — and that nothing was stored for the refused snapshot, so
/// the guard really did serialise the workflow.
async fn expect_snapshot_refused(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let snapshot = format!("{identifier}-snap");
    let body = expect_error(
        case,
        call(
            router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", snapshot.as_str()),
                ("DBInstanceIdentifier", identifier),
            ],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert_eq!(
        message,
        create_db_snapshot_instance_state_message(identifier),
        "case {case:?}: expected the sentence this class is worded with, got {message:?}"
    );
    expect_error(
        case,
        call(
            router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", snapshot.as_str())],
        )
        .await,
        404,
        "DBSnapshotNotFound",
    );
}

#[tokio::test]
async fn snapshotting_an_instance_that_is_not_available_is_refused() {
    let router = emulator();

    // Each case is a lifecycle stage a source instance can be in that is not `available`:
    // the stage a create starts it in, and the one a stop leaves it in. The identifiers
    // differ so no case can pass on a value baked into the handler, and the instance that
    // is still coming up is asked for its snapshot INSIDE the window the clock still owes.
    expect_ok(
        "an instance that is still coming up",
        create_instance(&router, "emu-snap-guard-creating").await,
    );
    expect_snapshot_refused(
        &router,
        "an instance the create has not finished yet",
        "emu-snap-guard-creating",
    )
    .await;

    create_available_instance(&router, "emu-snap-guard-stopped").await;
    expect_ok(
        "stop the instance",
        call(
            &router,
            "StopDBInstance",
            &[("DBInstanceIdentifier", "emu-snap-guard-stopped")],
        )
        .await,
    );
    wait_for_settle();
    let status = text(
        &expect_ok(
            "read the stopped instance back",
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "emu-snap-guard-stopped")],
            )
            .await,
        ),
        "DBInstanceStatus",
    );
    assert_eq!(
        status, DB_INSTANCE_STATUS_STOPPED,
        "expected the stopped instance to report {DB_INSTANCE_STATUS_STOPPED}, got {status:?}"
    );
    expect_snapshot_refused(
        &router,
        "an instance a stop has taken down",
        "emu-snap-guard-stopped",
    )
    .await;

    // The same call against an `available` instance is served and reads back, so the rows
    // above refuse for the STATE and not for the call.
    create_available_instance(&router, "emu-snap-guard-live").await;
    expect_ok(
        "snapshot the available instance",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", "emu-snap-guard-live-snap"),
                ("DBInstanceIdentifier", "emu-snap-guard-live"),
            ],
        )
        .await,
    );
    let read_back = expect_ok(
        "read the snapshot back",
        call(
            &router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "emu-snap-guard-live-snap")],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "DBSnapshotIdentifier"),
        "emu-snap-guard-live-snap",
        "expected the stored snapshot to be reported, got {read_back}"
    );
}

#[tokio::test]
async fn describing_a_snapshot_identifier_that_names_nothing_is_a_404() {
    let router = emulator();

    // One stored snapshot, so the rows below answer about the identifier they name.
    create_available_instance(&router, "emu-snap-probe-source").await;
    expect_ok(
        "a snapshot to find",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", "emu-snap-probe-stored"),
                ("DBInstanceIdentifier", "emu-snap-probe-source"),
            ],
        )
        .await,
    );

    // The shapes an identifier naming no snapshot can take: a short name, a long one, a
    // name that differs from the stored one by a suffix, and the stored snapshot's name in
    // a region that holds no record of it.
    for (case, identifier) in [
        ("a one-character identifier", "x"),
        (
            "a 255-character identifier",
            "a2345678901234567890123456789012345678901234567890123456789012345678901234\
             5678901234567890123456789012345678901234567890123456789012345678901234567890\
             1234567890123456789012345678901234567890123456789012345678901234567890123456\
             789012345678901234",
        ),
        ("a near-miss of the stored name", "emu-snap-probe-stored-2"),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                "DescribeDBSnapshots",
                &[("DBSnapshotIdentifier", identifier)],
            )
            .await,
            404,
            "DBSnapshotNotFound",
        );
        assert!(
            texts(&body, "DBSnapshotIdentifier").is_empty(),
            "case {case:?}: a refusal reports no snapshot, got {body}"
        );
    }

    // The stored identifier itself resolves, which is what makes the rows above a statement
    // about the identifier rather than about the operation.
    let body = expect_ok(
        "the stored snapshot",
        call(
            &router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "emu-snap-probe-stored")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "DBSnapshotIdentifier"),
        "emu-snap-probe-stored",
        "expected the stored snapshot to be reported, got {body}"
    );
}
