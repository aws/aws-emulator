// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PromoteReadReplica`: the operation that "promotes a read replica DB
//! instance to a standalone DB instance".
//!
//! The rule under test: a DB instance can be promoted exactly while it IS a read replica — it
//! carries the source `CreateDBInstanceReadReplica` resolved for it
//! (`ReadReplicaSourceDBInstanceIdentifier`) — and its database engine is running. The
//! promotion clears that source, so the instance replicates nothing and the source stops
//! reporting it in `ReadReplicaDBInstanceIdentifiers`; the standalone instance keeps
//! automated backups of its own, configured by `BackupRetentionPeriod` and
//! `PreferredBackupWindow` or left at what it already keeps, with the member's documented
//! default where it keeps none; and the accepted call answers the transient status, which the
//! instance leaves on its own. A `DBInstanceIdentifier` naming no instance of the region is
//! the modelled `DBInstanceNotFoundFault` (HTTP 404); an instance that carries no source —
//! never a replica, or already promoted — or that is not running is the modelled
//! `InvalidDBInstanceStateFault` (HTTP 400), which leaves the instance exactly as it was.
//!
//! The success side is asserted over a TABLE spanning what the rule generalises over: three
//! engine products, the two published bounds of `BackupRetentionPeriod` (0 and 35) and the
//! documented default, a backup window the request names and one it does not, and a replica
//! that already keeps automated backups of its own. The refusal side spans the identifier
//! forms that name nothing, the two states no promotion can be performed in, an instance that
//! is no replica at all, and the value just past each published bound of the retention period.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST; a settled status is reached by POLLING
//! a reporting operation, never by waiting a fixed time.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, texts,
};

/// How long a poll loop is given before this suite calls a transition unobservable.
///
/// Generous next to the span a transition this service owes becomes due after, so a loaded
/// test machine cannot fail the suite.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The status an accepted promotion answers with: this service applies the promotion to the
/// instance, which reports it until the change has run its course.
const PROMOTING: &str = "modifying";

/// The status the promoted instance settles on, on its own.
const AVAILABLE: &str = "available";

/// The sentence Amazon RDS refuses a promotion of an instance that is no read replica with.
/// It is FIXED text: neither the identifier the request carried nor the instance's status is
/// in it. Spelled out here so the wording is pinned by the test rather than by the code
/// under test.
const NOT_A_READ_REPLICA: &str = "DB Instance is not a read replica.";

/// The sentence Amazon RDS refuses a `BackupRetentionPeriod` of 0 for an instance that is
/// itself a replication source with: the published constraint of the member, which names
/// neither the instance nor the replicas built from its backups. Spelled out here so the
/// wording is pinned by the test rather than by the code under test.
const BACKUPS_OFF_FOR_A_REPLICATION_SOURCE: &str =
    "BackupRetentionPeriod can't be set to 0 if the DB instance is a source to read replicas.";

/// The sentence Amazon RDS refuses a promotion of a replica whose engine is not running with.
fn not_available(identifier: &str) -> String {
    format!("Instance {identifier} is not in available state.")
}

/// `DescribeDBInstances` naming one instance.
async fn describe(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    )
}

/// Poll `DescribeDBInstances` until the instance reports `status`, answering the body it
/// settled on; panics with the last answer when [`POLL_DEADLINE`] passes first.
///
/// This is the shape of the loop a real client writes: the loop makes a REPORTING call and
/// nothing else, so only a transition this service's clock owes can end it.
async fn poll_until_status(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    status: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (http_status, body) = call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await;
        if http_status == 200
            && optional_text(&body, "DBInstanceStatus").as_deref() == Some(status)
        {
            return body;
        }
        last = format!("HTTP {http_status}: {body}");
        sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {identifier} never reached {status} in {POLL_DEADLINE:?}; last answer: \
         {last}"
    )
}

/// `CreateDBInstance` for a source instance, polled until it is running so a replica of it
/// can be created.
async fn create_running_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    retention: &str,
) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", engine),
                ("DBInstanceClass", "db.t3.micro"),
                ("AllocatedStorage", "20"),
                ("BackupRetentionPeriod", retention),
            ],
        )
        .await,
    );
    poll_until_status(router, case, identifier, AVAILABLE).await;
}

/// `CreateDBInstanceReadReplica` of `source`, WITHOUT polling, so a case can address a
/// replica that is still coming up.
async fn create_replica(
    router: &MultiServiceRouter,
    identifier: &str,
    source: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBInstanceReadReplica",
        &[
            ("DBInstanceIdentifier", identifier),
            ("SourceDBInstanceIdentifier", source),
        ],
    )
    .await
}

/// `PromoteReadReplica` naming one instance, with `extra` beyond the identifier.
async fn promote(
    router: &MultiServiceRouter,
    named: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![("DBInstanceIdentifier", named.to_string())];
    for (member, value) in extra {
        params.push((member, (*value).to_string()));
    }
    call(router, "PromoteReadReplica", &params).await
}

/// Assert the modelled state fault for an instance that carries no replication source at all.
async fn expect_not_a_replica_refusal(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let refused = expect_error(
        case,
        promote(router, identifier, &[]).await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message, NOT_A_READ_REPLICA,
        "case {case:?}: expected the message {NOT_A_READ_REPLICA:?}, got {message:?}"
    );
}

/// One promotion row: the engine of the two instances, their identifiers, the automated
/// backups the replica keeps BEFORE the promotion, the members the promote request carries,
/// and the backup settings the promoted instance must report.
struct PromotionCase {
    case: &'static str,
    engine: &'static str,
    source: &'static str,
    replica: &'static str,
    /// The `BackupRetentionPeriod` a `ModifyDBInstance` sets on the replica first, where the
    /// row gives it one of its own; a replica is created keeping none.
    retention_before: Option<&'static str>,
    /// The members the promote carries beyond `DBInstanceIdentifier`.
    extra: &'static [(&'static str, &'static str)],
    /// The `BackupRetentionPeriod` the promoted instance must report.
    expect_retention: &'static str,
    /// The `PreferredBackupWindow` it must report, where the row names one; `None` asserts
    /// the window the replica already had is kept.
    expect_window: Option<&'static str>,
}

/// The class the promotion rule generalises over.
const PROMOTIONS: &[PromotionCase] = &[
    PromotionCase {
        case: "a mysql replica keeping no backups, promoted with no members set",
        engine: "mysql",
        source: "promote-src-mysql",
        replica: "promote-replica-mysql",
        retention_before: None,
        extra: &[],
        // `BackupRetentionPeriod`'s documented default for this operation ("Default: 1").
        expect_retention: "1",
        expect_window: None,
    },
    PromotionCase {
        case: "a postgres replica, promoted at the published upper bound with a window",
        engine: "postgres",
        source: "promote-src-postgres",
        replica: "promote-replica-postgres",
        retention_before: None,
        // 35 is the top of the published range ("Must be a value from 0 to 35"), and the
        // window is "in the format hh24:mi-hh24:mi".
        extra: &[
            ("BackupRetentionPeriod", "35"),
            ("PreferredBackupWindow", "03:15-03:45"),
        ],
        expect_retention: "35",
        expect_window: Some("03:15-03:45"),
    },
    PromotionCase {
        case: "a mariadb replica, promoted at the published lower bound (backups off)",
        engine: "mariadb",
        source: "promote-src-mariadb",
        replica: "promote-replica-mariadb",
        retention_before: None,
        // 0 is the bottom of the published range: "Setting this parameter to 0 disables
        // automated backups."
        extra: &[("BackupRetentionPeriod", "0")],
        expect_retention: "0",
        expect_window: None,
    },
    PromotionCase {
        case: "a replica that already keeps backups, promoted with no period named",
        engine: "mysql",
        source: "promote-src-kept",
        replica: "promote-replica-kept",
        retention_before: Some("7"),
        extra: &[],
        // The period the instance already keeps: the request asked to change nothing, so
        // the documented default does not overwrite it.
        expect_retention: "7",
        expect_window: None,
    },
];

#[tokio::test]
async fn a_read_replica_is_promoted_to_a_standalone_db_instance() {
    let router = emulator();

    for row in PROMOTIONS {
        let case = row.case;
        create_running_instance(&router, case, row.source, row.engine, "1").await;
        expect_ok(case, create_replica(&router, row.replica, row.source).await);
        poll_until_status(&router, case, row.replica, AVAILABLE).await;

        if let Some(retention) = row.retention_before {
            expect_ok(
                case,
                call(
                    &router,
                    "ModifyDBInstance",
                    &[
                        ("DBInstanceIdentifier", row.replica),
                        ("BackupRetentionPeriod", retention),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            );
            poll_until_status(&router, case, row.replica, AVAILABLE).await;
        }

        // Before the promotion the instance IS a read replica, from both ends of the one
        // stored relationship.
        let before = describe(&router, case, row.replica).await;
        let source_before = optional_text(&before, "ReadReplicaSourceDBInstanceIdentifier");
        assert_eq!(
            source_before.as_deref(),
            Some(row.source),
            "case {case:?}: expected the replica to name the source it replicates, got \
             {source_before:?}"
        );
        let window_before = optional_text(&before, "PreferredBackupWindow");
        let described_source = describe(&router, case, row.source).await;
        let replicas = texts(&described_source, "ReadReplicaDBInstanceIdentifier");
        assert!(
            replicas.iter().any(|named| named == row.replica),
            "case {case:?}: expected the source to report the replica before the promotion, \
             got {replicas:?}"
        );

        // The promotion itself: the accepted call answers the instance it changed, reporting
        // the transient status, no replication source and the backups the standalone
        // instance keeps.
        let promoted = expect_ok(case, promote(&router, row.replica, row.extra).await);
        let status = text(&promoted, "DBInstanceStatus");
        assert_eq!(
            status, PROMOTING,
            "case {case:?}: expected the accepted promotion to report the transient status, \
             got {status:?} in {promoted}"
        );
        let source_of_promoted = optional_text(&promoted, "ReadReplicaSourceDBInstanceIdentifier");
        assert_eq!(
            source_of_promoted, None,
            "case {case:?}: expected the promoted instance to replicate nothing, got \
             {source_of_promoted:?} in {promoted}"
        );
        let retention = text(&promoted, "BackupRetentionPeriod");
        assert_eq!(
            retention, row.expect_retention,
            "case {case:?}: expected the promoted instance to keep {} days of automated \
             backups, got {retention:?} in {promoted}",
            row.expect_retention
        );
        let expected_window = row
            .expect_window
            .map(str::to_string)
            .or_else(|| window_before.clone());
        let window = optional_text(&promoted, "PreferredBackupWindow");
        assert_eq!(
            window, expected_window,
            "case {case:?}: expected the backup window {expected_window:?}, got {window:?} in \
             {promoted}"
        );

        // The source stops reporting the promoted instance: the membership is read off the
        // stored records rather than copied onto them.
        let described_source = describe(&router, case, row.source).await;
        let replicas = texts(&described_source, "ReadReplicaDBInstanceIdentifier");
        assert!(
            !replicas.iter().any(|named| named == row.replica),
            "case {case:?}: expected the source to stop reporting the promoted instance, got \
             {replicas:?}"
        );

        // The instance reaches the settled status on its own, and the read-back is what
        // proves the promotion was persisted rather than echoed.
        let settled = poll_until_status(&router, case, row.replica, AVAILABLE).await;
        let source_of_settled =
            optional_text(&settled, "ReadReplicaSourceDBInstanceIdentifier");
        assert_eq!(
            source_of_settled, None,
            "case {case:?}: expected the standalone instance to stay standalone, got \
             {source_of_settled:?}"
        );
        let retention = text(&settled, "BackupRetentionPeriod");
        assert_eq!(
            retention, row.expect_retention,
            "case {case:?}: expected the stored instance to keep {} days of automated \
             backups, got {retention:?} in {settled}",
            row.expect_retention
        );
        let window = optional_text(&settled, "PreferredBackupWindow");
        assert_eq!(
            window, expected_window,
            "case {case:?}: expected the stored backup window {expected_window:?}, got \
             {window:?} in {settled}"
        );

        // An instance this promotion already made standalone is no read replica, so a second
        // promotion of it is the modelled state fault rather than one that changes nothing.
        expect_not_a_replica_refusal(&router, case, row.replica).await;
    }
}

#[tokio::test]
async fn a_promotion_of_an_instance_it_cannot_be_performed_on_is_refused() {
    let router = emulator();
    // An instance stored throughout, so a 404 below is a decision about the identifier the
    // request named and never "this service holds no instance at all". It replicates
    // nothing, which is also the first state case.
    create_running_instance(&router, "the standalone instance", "promote-standalone", "mysql", "1")
        .await;

    // An identifier nothing created, in the spellings a member "stored as a lowercase
    // string" folds together.
    for named in ["promote-absent", "PROMOTE-Absent"] {
        let case = format!("an absent instance named {named:?}");
        let refused = expect_error(
            &case,
            promote(&router, named, &[]).await,
            404,
            "DBInstanceNotFound",
        );
        let expected = format!("DBInstance {} not found.", named.to_ascii_lowercase());
        let message = text(&refused, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // An instance that never replicated anything.
    expect_not_a_replica_refusal(
        &router,
        "an instance that never replicated",
        "promote-standalone",
    )
    .await;

    // A replica that is still coming up: the status the clock has yet to settle is real, so
    // the promotion is refused while the instance is in it — and the IDENTICAL call is
    // served once it is running.
    expect_ok(
        "a replica that is still coming up",
        create_replica(&router, "promote-creating", "promote-standalone").await,
    );
    let refused = expect_error(
        "a replica that is still coming up",
        promote(&router, "promote-creating", &[]).await,
        400,
        "InvalidDBInstanceState",
    );
    let expected = not_available("promote-creating");
    let message = text(&refused, "Message");
    assert_eq!(
        message, expected,
        "case \"a replica that is still coming up\": expected the message {expected:?}, got \
         {message:?}"
    );
    poll_until_status(&router, "a replica that is still coming up", "promote-creating", AVAILABLE)
        .await;
    let served = expect_ok(
        "the same replica once it is running",
        promote(&router, "promote-creating", &[]).await,
    );
    assert_eq!(
        optional_text(&served, "ReadReplicaSourceDBInstanceIdentifier"),
        None,
        "case \"the same replica once it is running\": got {served}"
    );

    // A replica taking a modification is in the same case: not running, so no promotion can
    // be performed on it, and a refused promotion leaves it a read replica.
    expect_ok(
        "a replica taking a modification",
        create_replica(&router, "promote-modifying", "promote-standalone").await,
    );
    poll_until_status(&router, "a replica taking a modification", "promote-modifying", AVAILABLE)
        .await;
    expect_ok(
        "modify the replica",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "promote-modifying"),
                ("BackupRetentionPeriod", "3"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let refused = expect_error(
        "a replica taking a modification",
        promote(&router, "promote-modifying", &[]).await,
        400,
        "InvalidDBInstanceState",
    );
    let expected = not_available("promote-modifying");
    let message = text(&refused, "Message");
    assert_eq!(
        message, expected,
        "case \"a replica taking a modification\": expected the message {expected:?}, got \
         {message:?}"
    );
    let unchanged = poll_until_status(
        &router,
        "a replica taking a modification",
        "promote-modifying",
        AVAILABLE,
    )
    .await;
    assert_eq!(
        optional_text(&unchanged, "ReadReplicaSourceDBInstanceIdentifier").as_deref(),
        Some("promote-standalone"),
        "case \"a replica taking a modification\": a refused promotion must leave the instance \
         a read replica, got {unchanged}"
    );
}

#[tokio::test]
async fn a_retention_period_outside_the_published_range_is_refused() {
    let router = emulator();
    create_running_instance(&router, "the source", "promote-range-src", "mysql", "1").await;
    expect_ok(
        "the replica",
        create_replica(&router, "promote-range-replica", "promote-range-src").await,
    );
    poll_until_status(&router, "the replica", "promote-range-replica", AVAILABLE).await;

    // One value past each published bound of "Must be a value from 0 to 35".
    for value in ["-1", "36"] {
        let case = format!("a retention period of {value}");
        expect_error(
            &case,
            promote(
                &router,
                "promote-range-replica",
                &[("BackupRetentionPeriod", value)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
    // The refused calls changed nothing: the instance is still a read replica, so the
    // identical promotion inside the range is served.
    let still_a_replica = describe(&router, "after the refusals", "promote-range-replica").await;
    assert_eq!(
        optional_text(&still_a_replica, "ReadReplicaSourceDBInstanceIdentifier").as_deref(),
        Some("promote-range-src"),
        "expected a refused promotion to leave the instance a read replica, got \
         {still_a_replica}"
    );
    let promoted = expect_ok(
        "a retention period inside the range",
        promote(
            &router,
            "promote-range-replica",
            &[("BackupRetentionPeriod", "12")],
        )
        .await,
    );
    assert_eq!(
        text(&promoted, "BackupRetentionPeriod"),
        "12",
        "expected the served promotion to keep the period it named, got {promoted}"
    );
}

#[tokio::test]
async fn backups_cannot_be_turned_off_for_an_instance_that_is_itself_a_replication_source() {
    let router = emulator();
    // A cascade: the middle instance replicates the first and is itself replicated by the
    // third, which is what makes its automated backups something another instance is built
    // from.
    create_running_instance(&router, "the root", "promote-cascade-root", "mysql", "1").await;
    expect_ok(
        "the middle replica",
        create_replica(&router, "promote-cascade-mid", "promote-cascade-root").await,
    );
    poll_until_status(&router, "the middle replica", "promote-cascade-mid", AVAILABLE).await;
    // A replica keeps no automated backups until one is turned on for it, and a source of a
    // replica must keep some.
    expect_ok(
        "turn the middle replica's backups on",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "promote-cascade-mid"),
                ("BackupRetentionPeriod", "5"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    poll_until_status(&router, "the middle replica", "promote-cascade-mid", AVAILABLE).await;
    expect_ok(
        "the cascaded replica",
        create_replica(&router, "promote-cascade-leaf", "promote-cascade-mid").await,
    );
    poll_until_status(&router, "the cascaded replica", "promote-cascade-leaf", AVAILABLE).await;

    // Turning the middle instance's backups off would leave the instance that is built from
    // them with nothing to replicate: the members cannot be served together.
    let refused = expect_error(
        "backups off for a replication source",
        promote(
            &router,
            "promote-cascade-mid",
            &[("BackupRetentionPeriod", "0")],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message, BACKUPS_OFF_FOR_A_REPLICATION_SOURCE,
        "expected the message {BACKUPS_OFF_FOR_A_REPLICATION_SOURCE:?}, got {message:?}"
    );
    // The refused call changed nothing, and the same promotion that leaves the backups on is
    // served.
    let still_a_replica = describe(&router, "after the refusal", "promote-cascade-mid").await;
    assert_eq!(
        optional_text(&still_a_replica, "ReadReplicaSourceDBInstanceIdentifier").as_deref(),
        Some("promote-cascade-root"),
        "expected the refused promotion to leave the instance a read replica, got \
         {still_a_replica}"
    );
    let promoted = expect_ok(
        "the same promotion leaving the backups on",
        promote(&router, "promote-cascade-mid", &[]).await,
    );
    assert_eq!(
        (
            optional_text(&promoted, "ReadReplicaSourceDBInstanceIdentifier"),
            text(&promoted, "BackupRetentionPeriod"),
        ),
        (None, "5".to_string()),
        "expected the promoted instance to replicate nothing and keep its own backups, got \
         {promoted}"
    );
    // The promoted instance keeps the replica of its own: promoting it ends ITS replication,
    // not the replication of the instances built from it.
    let replicas = texts(&promoted, "ReadReplicaDBInstanceIdentifier");
    assert!(
        replicas.iter().any(|named| named == "promote-cascade-leaf"),
        "expected the promoted instance to keep reporting its own replica, got {replicas:?}"
    );
}
