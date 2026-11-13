// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what makes a DB instance a READ REPLICA, and for
//! `PromoteReadReplica`, the operation that ends that relationship.
//!
//! The rules under test:
//!
//! * a DB instance that replicates another reports `StatusInfos` — one entry whose
//!   `StatusType` is the one the member publishes ("read replication"), reporting the
//!   replication as normal — and an instance that replicates nothing reports none, which is
//!   the blank value the member documents;
//! * what the SOURCE has COMMITTED is what its replica holds: a committed change reaches the
//!   replica's own storage, a change a transaction has not committed does not, and the
//!   replica's storage is its own — after a promotion the two instances' writes are
//!   independent;
//! * a session on a replica is a session on a STANDBY: it cannot put its transaction into
//!   read-write mode (`feature_not_supported`, "cannot set transaction read-write mode
//!   during recovery"), while setting the session's DEFAULT for later transactions is taken
//!   and changes nothing — every write is still refused under
//!   `read_only_sql_transaction`. The same session setting on the WRITER instance is
//!   enforced there too, and turning it off again restores its writes;
//! * `PromoteReadReplica` makes a replica a standalone DB instance: it reports no source and
//!   no `StatusInfos`, the source stops reporting it as one of its replicas, it keeps the
//!   automated backups the request asked for (the member's documented default where it asked
//!   for none), and it passes through `modifying` on its way back to `available`. Only a
//!   read replica can be promoted, and only a value in the documented range is accepted.
//!
//! The rules are asserted over TABLES spanning what they generalise over — the engines and
//! both wires an RDS read replica runs, the backup retention periods the promotion accepts
//! (its default, each published bound and one past it), and the statements a standby refuses
//! against the ones it takes — rather than over one request.
//!
//! Every control-plane call goes over the wire as a real awsQuery form POST through
//! `db_instances::emulator`, and every data-plane statement goes over the published endpoint
//! through the wire clients the sibling endpoint suites drive, in ONE process. Nothing waits
//! on the wall clock: every wait is a poll loop on a REPORTING call with a generous
//! deadline.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_instance_engine_endpoint::{
    available_instance, endpoint_of, mysql_login, postgres_login, MySqlSession, PostgresSession,
    Refusal, Rows, Transport, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
};

/// How long a poll loop is given before this suite calls a transition unobservable.
///
/// Far longer than the span a transition this service owes becomes due after, and than any
/// carry-over the engine takes, so a heavily loaded test machine running the whole
/// workspace's suites at once cannot fail this one; a client's patience is longer still.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `StatusType` a read replica's status info carries, as `DBInstanceStatusInfo$StatusType`
/// publishes it ("this value is currently \"read replication\"").
const READ_REPLICATION: &str = "read replication";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// `DescribeDBInstances` for one instance, as the HTTP status and the body.
async fn read_instance(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => identifier],
    )
    .await
}

/// Poll `DescribeDBInstances` until the instance `identifier` reports `status`, answering
/// its `<DBInstance>` subtree; panics with the last answer when [`POLL_DEADLINE`] passes.
///
/// A REPORTING call and nothing else, polled for the CONDITION rather than waited out, which
/// is the loop a real client writes.
async fn poll_until_status(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    status: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (http_status, body) = read_instance(router, identifier).await;
        if http_status == 200 && text(&body, "DBInstanceStatus") == status {
            return the_instance(case, &body);
        }
        last = format!("HTTP {http_status}: {body}");
        sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {identifier} never reported {status:?}; last answer: {last}")
}

/// The one `<DBInstance>` subtree a report carries.
fn the_instance(case: &str, body: &str) -> String {
    let reported = subtrees(body, "DBInstance");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: exactly one DBInstance must be reported, got {body}"
    );
    reported.into_iter().next().expect("checked just above")
}

/// The `<DBInstanceStatusInfo>` subtrees an instance's envelope carries.
fn status_infos(instance: &str) -> Vec<String> {
    subtrees(instance, "DBInstanceStatusInfo")
}

/// Create a read replica of `source` and poll it to `available`, answering its endpoint and
/// its settled envelope.
async fn available_replica(
    router: &MultiServiceRouter,
    case: &str,
    source: &str,
    replica: &str,
    extra: &[(&str, String)],
) -> ((String, u16), String) {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => replica,
        "SourceDBInstanceIdentifier" => source,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    let created = expect_ok(
        case,
        call(router, "CreateDBInstanceReadReplica", &request).await,
    );
    let endpoint = endpoint_of(&created);
    let settled = poll_until_status(router, case, replica, "available").await;
    (endpoint, settled)
}

// ---------------------------------------------------------------------------
// StatusInfos: the replication status of a read replica.
// ---------------------------------------------------------------------------

/// One source/replica pair the status-info rule is asserted over.
struct StatusCase {
    case: &'static str,
    engine: &'static str,
    source: &'static str,
    replica: &'static str,
}

/// The class the rule generalises over: the engine products an RDS read replica runs, which
/// is also both wires this emulator serves.
const STATUS_CASES: &[StatusCase] = &[
    StatusCase {
        case: "a read replica of an RDS for PostgreSQL instance",
        engine: "postgres",
        source: "si-src-pg",
        replica: "si-replica-pg",
    },
    StatusCase {
        case: "a read replica of an RDS for MySQL instance",
        engine: "mysql",
        source: "si-src-mysql",
        replica: "si-replica-mysql",
    },
    StatusCase {
        case: "a read replica of an RDS for MariaDB instance",
        engine: "mariadb",
        source: "si-src-mariadb",
        replica: "si-replica-mariadb",
    },
    StatusCase {
        case: "a read replica of an RDS for Oracle instance, whose engine this emulator \
               serves no data plane for",
        engine: "oracle-ee",
        source: "si-src-oracle",
        replica: "si-replica-oracle",
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_replica_reports_its_replication_status_and_a_standalone_instance_reports_none() {
    let router = emulator();
    for row in STATUS_CASES {
        let case = row.case;
        // A replica is built from the source's automated backups, so the source keeps them.
        available_instance(
            &router,
            row.source,
            row.engine,
            &params!["BackupRetentionPeriod" => 1],
        )
        .await;
        let (_, replica) =
            available_replica(&router, case, row.source, row.replica, &[]).await;

        // The replica reports exactly one status info, for the one status type the member
        // publishes, reporting the replication as normal.
        let reported = status_infos(&replica);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a read replica must report its replication status, got {replica}"
        );
        let info = &reported[0];
        assert_eq!(
            (
                text(info, "StatusType"),
                text(info, "Normal"),
                text(info, "Status"),
            ),
            (
                READ_REPLICATION.to_string(),
                "true".to_string(),
                "replicating".to_string(),
            ),
            "case {case:?}: got {info}"
        );
        // "If the instance isn't in an error state, this value is blank": no `Message`.
        assert_eq!(
            optional_text(info, "Message"),
            None,
            "case {case:?}: a replica that is replicating normally reports no error \
             message, got {info}"
        );

        // The SOURCE replicates nothing, so it reports no status info at all — the blank
        // value the member documents.
        let source = the_instance(case, &read_instance(&router, row.source).await.1);
        assert!(
            status_infos(&source).is_empty(),
            "case {case:?}: an instance that is not a read replica reports no StatusInfos, \
             got {source}"
        );

        // And a promotion ends the relationship, so the promoted instance reports none
        // either, and the source stops reporting it as a replica.
        expect_ok(
            case,
            call(
                &router,
                "PromoteReadReplica",
                &params!["DBInstanceIdentifier" => row.replica],
            )
            .await,
        );
        let promoted = poll_until_status(&router, case, row.replica, "available").await;
        assert!(
            status_infos(&promoted).is_empty()
                && optional_text(&promoted, "ReadReplicaSourceDBInstanceIdentifier").is_none(),
            "case {case:?}: a promoted replica replicates nothing, so it reports neither a \
             source nor StatusInfos, got {promoted}"
        );
        let source = the_instance(case, &read_instance(&router, row.source).await.1);
        assert!(
            !texts(&source, "ReadReplicaDBInstanceIdentifier").contains(&row.replica.to_string()),
            "case {case:?}: the source must stop reporting a promoted replica, got {source}"
        );
    }
}

// ---------------------------------------------------------------------------
// PromoteReadReplica: the standalone instance it leaves behind.
// ---------------------------------------------------------------------------

/// One promotion row: what the request asks for in automated backups, and what the promoted
/// instance keeps.
struct PromotionCase {
    case: &'static str,
    replica: &'static str,
    /// `BackupRetentionPeriod` the promotion asks for, or `None` to ask for none.
    retention: Option<i32>,
    /// `PreferredBackupWindow` the promotion asks for.
    window: Option<&'static str>,
    /// The period the promoted instance must report.
    expected_retention: &'static str,
}

/// The class the backup rule generalises over: asking for nothing (the member's documented
/// default of 1), each published bound of "must be a value from 0 to 35" — 0, which "disables
/// automated backups", and 35 — and a period with a window of its own.
const PROMOTIONS: &[PromotionCase] = &[
    PromotionCase {
        case: "a promotion that asks for no retention period takes the documented default",
        replica: "promo-default",
        retention: None,
        window: None,
        expected_retention: "1",
    },
    PromotionCase {
        case: "a promotion that disables automated backups",
        replica: "promo-zero",
        retention: Some(0),
        window: None,
        expected_retention: "0",
    },
    PromotionCase {
        case: "a promotion at the published upper bound",
        replica: "promo-max",
        retention: Some(35),
        window: None,
        expected_retention: "35",
    },
    PromotionCase {
        case: "a promotion with a backup window of its own",
        replica: "promo-window",
        retention: Some(7),
        window: Some("03:00-03:30"),
        expected_retention: "7",
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_promotion_makes_the_replica_standalone_with_the_backups_it_asked_for() {
    let router = emulator();
    available_instance(
        &router,
        "promo-src",
        "postgres",
        &params!["BackupRetentionPeriod" => 1],
    )
    .await;

    for row in PROMOTIONS {
        let case = row.case;
        let (_, replica) =
            available_replica(&router, case, "promo-src", row.replica, &[]).await;
        // A replica keeps no automated backup of its own until one is turned on for it.
        assert_eq!(
            text(&replica, "BackupRetentionPeriod"),
            "0",
            "case {case:?}: got {replica}"
        );

        let mut request: Vec<(&str, String)> =
            params!["DBInstanceIdentifier" => row.replica].to_vec();
        if let Some(retention) = row.retention {
            request.push(("BackupRetentionPeriod", retention.to_string()));
        }
        if let Some(window) = row.window {
            request.push(("PreferredBackupWindow", window.to_string()));
        }
        let promoted = expect_ok(case, call(&router, "PromoteReadReplica", &request).await);
        // The promotion is accepted with the instance applying it, and the clock owes it
        // `available`: the answer reports the transient status, which a caller polling
        // straight after the call sees too.
        assert_eq!(
            text(&promoted, "DBInstanceStatus"),
            "modifying",
            "case {case:?}: got {promoted}"
        );
        assert!(
            optional_text(&promoted, "ReadReplicaSourceDBInstanceIdentifier").is_none(),
            "case {case:?}: a promoted instance replicates nothing, got {promoted}"
        );

        let settled = poll_until_status(&router, case, row.replica, "available").await;
        assert_eq!(
            text(&settled, "BackupRetentionPeriod"),
            row.expected_retention,
            "case {case:?}: the promoted instance must keep the period the promotion set, \
             got {settled}"
        );
        if let Some(window) = row.window {
            assert_eq!(
                text(&settled, "PreferredBackupWindow"),
                window,
                "case {case:?}: got {settled}"
            );
        }
        // Promoting the same instance again is refused: it is no read replica any more.
        let refused = expect_error(
            case,
            call(
                &router,
                "PromoteReadReplica",
                &params!["DBInstanceIdentifier" => row.replica],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        assert_eq!(
            text(&refused, "Message"),
            "DB Instance is not a read replica.",
            "case {case:?}: got {refused}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_promotion_of_something_that_is_no_read_replica_or_out_of_range_is_refused() {
    let router = emulator();
    available_instance(
        &router,
        "promo-guard-src",
        "postgres",
        &params!["BackupRetentionPeriod" => 1],
    )
    .await;
    available_replica(&router, "the guard replica", "promo-guard-src", "promo-guard-rep", &[])
        .await;

    // One refusal row: what the request carries, and the status and wire code it is answered
    // with. The retention rows span the two values just past the published bounds.
    struct Guard {
        case: &'static str,
        params: &'static [(&'static str, &'static str)],
        status: u16,
        code: &'static str,
    }
    let rows: [Guard; 4] = [
        Guard {
            case: "an identifier nothing created",
            params: &[("DBInstanceIdentifier", "no-such-instance")],
            status: 404,
            code: "DBInstanceNotFound",
        },
        Guard {
            case: "an instance that replicates nothing",
            params: &[("DBInstanceIdentifier", "promo-guard-src")],
            status: 400,
            code: "InvalidDBInstanceState",
        },
        Guard {
            case: "a retention period past the published upper bound",
            params: &[
                ("DBInstanceIdentifier", "promo-guard-rep"),
                ("BackupRetentionPeriod", "36"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        Guard {
            case: "a retention period below the published lower bound",
            params: &[
                ("DBInstanceIdentifier", "promo-guard-rep"),
                ("BackupRetentionPeriod", "-1"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
    ];
    for row in rows {
        let case = row.case;
        expect_error(
            case,
            call(&router, "PromoteReadReplica", row.params).await,
            row.status,
            row.code,
        );
    }
    // None of those refusals promoted anything: the replica still replicates its source.
    let replica = the_instance(
        "the refused promotions changed nothing",
        &read_instance(&router, "promo-guard-rep").await.1,
    );
    assert_eq!(
        optional_text(&replica, "ReadReplicaSourceDBInstanceIdentifier").as_deref(),
        Some("promo-guard-src"),
        "case \"the refused promotions changed nothing\": got {replica}"
    );
}

// ---------------------------------------------------------------------------
// The replication itself, over both wires.
// ---------------------------------------------------------------------------

/// Which wire a row's instances are dialled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Postgres,
    MySql,
}

/// A session a client has opened on one of the two wires, so one table can drive both.
enum Session {
    Postgres(Box<PostgresSession>),
    MySql(Box<MySqlSession>),
}

impl Session {
    /// Run one statement and answer with what the server did.
    async fn query(&mut self, sql: &str) -> Result<Rows, Refusal> {
        match self {
            Self::Postgres(session) => session.query(sql).await,
            Self::MySql(session) => session.query(sql).await,
        }
    }

    /// Run one statement that must be served, and answer with the one value it reports.
    async fn value(&mut self, case: &str, sql: &str) -> String {
        let answer = self
            .query(sql)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
        answer
            .only()
            .unwrap_or_else(|| panic!("case {case:?}: {sql:?} must answer with a value"))
            .to_string()
    }

    /// Run one statement that must be served, for its effect.
    async fn run(&mut self, case: &str, sql: &str) {
        self.query(sql)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
    }
}

/// Open a session as the master user on `address:port` of `wire`, in the database every
/// instance of this suite is created with.
async fn login(case: &str, wire: Wire, address: &str, port: u16) -> Session {
    match wire {
        // The in-band TLS upgrade this wire negotiates on the same port: an RDS for
        // PostgreSQL instance of a current version refuses a session in the clear.
        Wire::Postgres => Session::Postgres(Box::new(
            postgres_login(
                address,
                port,
                Transport::Tls,
                MASTER_USER,
                MASTER_PASSWORD,
                Some(DATABASE),
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}: the master user must be admitted on {address}:{port}, got {refusal:?}")
            }),
        )),
        Wire::MySql => Session::MySql(Box::new(
            mysql_login(
                address,
                port,
                Transport::Plaintext,
                MASTER_USER,
                MASTER_PASSWORD,
                Some(DATABASE),
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}: the master user must be admitted on {address}:{port}, got {refusal:?}")
            }),
        )),
    }
}

/// Poll the replica's session until `sql` answers `expected`; panics with the last answer
/// when [`POLL_DEADLINE`] passes first.
///
/// Replication is ASYNCHRONOUS, as the engines' own is, so a reader waits for the row to
/// arrive rather than for a span of the clock to pass.
async fn poll_until_value(case: &str, session: &mut Session, sql: &str, expected: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = session.value(case, sql).await;
        if last == expected {
            return;
        }
        sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {sql:?} never answered {expected:?}; last answer: {last:?}")
}

/// One source/replica pair the replication rule is asserted over.
struct ReplicationCase {
    case: &'static str,
    engine: &'static str,
    source: &'static str,
    replica: &'static str,
    wire: Wire,
}

/// The class the rule generalises over: both wires this emulator serves a DB instance's
/// engine on.
const REPLICATIONS: &[ReplicationCase] = &[
    ReplicationCase {
        case: "an RDS for PostgreSQL source and its replica",
        engine: "postgres",
        source: "repl-src-pg",
        replica: "repl-rep-pg",
        wire: Wire::Postgres,
    },
    ReplicationCase {
        case: "an RDS for MySQL source and its replica",
        engine: "mysql",
        source: "repl-src-mysql",
        replica: "repl-rep-mysql",
        wire: Wire::MySql,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sources_committed_writes_reach_its_replica_and_promotion_ends_that() {
    let router = emulator();
    for row in REPLICATIONS {
        let case = row.case;
        let source_endpoint = available_instance(
            &router,
            row.source,
            row.engine,
            &params!["BackupRetentionPeriod" => 1],
        )
        .await;
        let mut writer = login(case, row.wire, &source_endpoint.0, source_endpoint.1).await;
        writer
            .run(
                case,
                "CREATE TABLE ledger (id integer PRIMARY KEY, note text)",
            )
            .await;
        writer
            .run(case, "INSERT INTO ledger (id, note) VALUES (1, 'before')")
            .await;

        let (replica_endpoint, _) =
            available_replica(&router, case, row.source, row.replica, &[]).await;
        let mut reader = login(case, row.wire, &replica_endpoint.0, replica_endpoint.1).await;
        // The rows the source held when the replica was created are the replica's rows.
        assert_eq!(
            reader
                .value(case, "SELECT note FROM ledger WHERE id = 1")
                .await,
            "before",
            "case {case:?}: the replica is built from the source's data"
        );

        // A change the source COMMITS reaches the replica.
        writer
            .run(case, "INSERT INTO ledger (id, note) VALUES (2, 'after')")
            .await;
        poll_until_value(case, &mut reader, "SELECT count(*) FROM ledger", "2").await;

        // A change a transaction has NOT committed does not: it is in that session's own
        // copy of the database and nowhere else.
        writer.run(case, "BEGIN").await;
        writer
            .run(
                case,
                "INSERT INTO ledger (id, note) VALUES (3, 'uncommitted')",
            )
            .await;
        assert_eq!(
            reader.value(case, "SELECT count(*) FROM ledger").await,
            "2",
            "case {case:?}: a write no transaction has committed must not reach the replica"
        );
        writer.run(case, "COMMIT").await;
        poll_until_value(case, &mut reader, "SELECT count(*) FROM ledger", "3").await;

        // The promotion ends the replication: each instance's storage is its own from then
        // on, so a write on one is absent from the other.
        expect_ok(
            case,
            call(
                &router,
                "PromoteReadReplica",
                &params!["DBInstanceIdentifier" => row.replica],
            )
            .await,
        );
        poll_until_status(&router, case, row.replica, "available").await;
        let mut promoted = login(case, row.wire, &replica_endpoint.0, replica_endpoint.1).await;
        promoted
            .run(
                case,
                "INSERT INTO ledger (id, note) VALUES (4, 'only on the promoted instance')",
            )
            .await;
        writer
            .run(
                case,
                "INSERT INTO ledger (id, note) VALUES (5, 'only on the source')",
            )
            .await;
        assert_eq!(
            (
                promoted.value(case, "SELECT count(*) FROM ledger").await,
                promoted
                    .value(case, "SELECT count(*) FROM ledger WHERE id = 5")
                    .await,
                writer.value(case, "SELECT count(*) FROM ledger").await,
                writer
                    .value(case, "SELECT count(*) FROM ledger WHERE id = 4")
                    .await,
            ),
            (
                "4".to_string(),
                "0".to_string(),
                "4".to_string(),
                "0".to_string(),
            ),
            "case {case:?}: after the promotion the two instances write independently"
        );
    }
}

// ---------------------------------------------------------------------------
// The standby's read-only mode, and the session setting beside it.
// ---------------------------------------------------------------------------

/// One statement a session on a standby is refused, with the condition PostgreSQL reports it
/// under.
///
/// Source: PostgreSQL's own `check_transaction_read_only`, which refuses the CURRENT
/// transaction's read-write mode while the server is in recovery under
/// `feature_not_supported`, and the `SET TRANSACTION` page's catalogue of statements a
/// read-only transaction refuses, which carry `read_only_sql_transaction`.
struct StandbyRefusal {
    case: &'static str,
    sql: &'static str,
    sqlstate: &'static str,
    message: &'static str,
}

const STANDBY_REFUSALS: &[StandbyRefusal] = &[
    StandbyRefusal {
        case: "setting the current transaction's parameter",
        sql: "SET transaction_read_only = off",
        sqlstate: "0A000",
        message: "cannot set transaction read-write mode during recovery",
    },
    StandbyRefusal {
        case: "the statement that sets the same mode",
        sql: "SET TRANSACTION READ WRITE",
        sqlstate: "0A000",
        message: "cannot set transaction read-write mode during recovery",
    },
    StandbyRefusal {
        case: "a transaction opened in that mode",
        sql: "BEGIN READ WRITE",
        sqlstate: "0A000",
        message: "cannot set transaction read-write mode during recovery",
    },
    StandbyRefusal {
        case: "a transaction started in that mode, the other spelling",
        sql: "START TRANSACTION READ WRITE",
        sqlstate: "0A000",
        message: "cannot set transaction read-write mode during recovery",
    },
    StandbyRefusal {
        case: "a write, which is what the mode would have been for",
        sql: "INSERT INTO ledger (id, note) VALUES (9, 'from the reader')",
        sqlstate: "25006",
        message: "cannot execute INSERT in a read-only transaction",
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standby_session_cannot_grant_itself_write_mode_and_the_setting_binds_the_writer() {
    let router = emulator();
    let case = "a read replica of an RDS for PostgreSQL instance";
    let source_endpoint = available_instance(
        &router,
        "ro-src-pg",
        "postgres",
        &params!["BackupRetentionPeriod" => 1],
    )
    .await;
    let mut writer = login(case, Wire::Postgres, &source_endpoint.0, source_endpoint.1).await;
    writer
        .run(
            case,
            "CREATE TABLE ledger (id integer PRIMARY KEY, note text)",
        )
        .await;
    writer
        .run(case, "INSERT INTO ledger (id, note) VALUES (1, 'seed')")
        .await;

    let (replica_endpoint, _) =
        available_replica(&router, case, "ro-src-pg", "ro-rep-pg", &[]).await;
    let mut reader = login(case, Wire::Postgres, &replica_endpoint.0, replica_endpoint.1).await;

    // The replica's engine is a standby: it is in recovery and its transactions are
    // read-only.
    assert_eq!(
        (
            reader.value(case, "SELECT pg_is_in_recovery()").await,
            reader.value(case, "SHOW transaction_read_only").await,
        ),
        ("t".to_string(), "on".to_string()),
        "case {case:?}: a read replica's engine must report itself as a standby"
    );

    // Setting the session's DEFAULT for later transactions is taken — the parameter has no
    // such refusal — and changes nothing: every write is still refused.
    for accepted in [
        "SET default_transaction_read_only = off",
        "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
    ] {
        reader.run(case, accepted).await;
    }
    assert_eq!(
        reader
            .value(case, "SHOW default_transaction_read_only")
            .await,
        "off",
        "case {case:?}: the session's default is what the session set"
    );
    for row in STANDBY_REFUSALS {
        let refusal = reader.query(row.sql).await.err().unwrap_or_else(|| {
            panic!(
                "case {:?}: {:?} must be refused on a standby",
                row.case, row.sql
            )
        });
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            (row.sqlstate, row.message),
            "case {:?}: expected the engine's own refusal of {:?}, got {refusal:?}",
            row.case,
            row.sql
        );
    }
    // Nothing of those refused statements reached the replica's storage.
    assert_eq!(
        reader.value(case, "SELECT count(*) FROM ledger").await,
        "1",
        "case {case:?}: the refused statements must have changed nothing"
    );

    // The SAME session setting binds the WRITER: a write under it is refused with the same
    // condition, and turning it off again — which a writable engine takes — restores the
    // session's writes.
    writer
        .run(case, "SET default_transaction_read_only = on")
        .await;
    let refusal = writer
        .query("INSERT INTO ledger (id, note) VALUES (2, 'under the session setting')")
        .await
        .err()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a write under the session's read-only mode must be refused")
        });
    assert_eq!(
        (refusal.code.as_str(), refusal.message.as_str()),
        ("25006", "cannot execute INSERT in a read-only transaction"),
        "case {case:?}: got {refusal:?}"
    );
    writer
        .run(case, "SET default_transaction_read_only = off")
        .await;
    writer
        .run(case, "SET transaction_read_only = off")
        .await;
    writer
        .run(case, "INSERT INTO ledger (id, note) VALUES (2, 'written again')")
        .await;
    assert_eq!(
        writer.value(case, "SELECT count(*) FROM ledger").await,
        "2",
        "case {case:?}: the writer's session writes again once its read-only mode is off"
    );
}
