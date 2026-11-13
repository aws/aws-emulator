// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchExecuteStatement`, driven over the wire through the
//! shared harness in [`super::emulator`]: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the emulator's
//! HTTP behaviour and not a handler's return value.
//!
//! The rule under test, asserted over a TABLE of batches rather than one case: a
//! batch runs its `Sqls` serially, in the order of the array, on the database the
//! request names; it is recorded as ONE statement whose `SubStatements` report each
//! statement's own text, status and outcome under the `Id` `<batch id>:<n>`; and its
//! statements are committed together, with every one of them rolled back as soon as
//! one fails, unless `ExecutionMode` is `AUTO_COMMIT`, where each statement is
//! committed on its own and a failure stops nothing.

use serde_json::{json, Value};

use service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_RETAINED_STATEMENTS;

use super::emulator::{
    error_code, flag, id_of, list, number, wait_for_terminal, wait_until_started, Emulator,
    SLOW_SQL, UNBOUNDED_SQL, WAIT_SECONDS,
};

/// The connection every batch below is submitted with: one documented
/// authorization combination (a provisioned cluster as a database user).
fn connection() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// A `BatchExecuteStatement` request for `sqls`, in `mode` when one is named.
fn batch(sqls: &[&str], mode: Option<&str>) -> Value {
    let mut request = connection();
    request["Sqls"] = json!(sqls);
    if let Some(mode) = mode {
        request["ExecutionMode"] = json!(mode);
    }
    request
}

/// The `SubStatements` a `DescribeStatement` response carries.
fn sub_statements(described: &Value) -> Vec<Value> {
    list(described, "SubStatements")
}

/// The `Status` each of those statements reports, in submission order.
fn statuses_of(subs: &[Value]) -> Vec<&str> {
    subs.iter()
        .map(|sub| sub["Status"].as_str().unwrap_or("<none>"))
        .collect()
}

// ---------------------------------------------------------------------------
// What a batch runs, and what it reports about each statement
// ---------------------------------------------------------------------------

/// One row of the batch table: the statements submitted, and what each one must
/// be shown to have produced.
struct BatchCase {
    label: &'static str,
    sqls: &'static [&'static str],
    /// Per statement, in submission order: `HasResultSet` and `ResultRows`.
    reported: &'static [(bool, i64)],
    /// Per statement, in submission order: the exact `Records`
    /// `GetStatementResult` must return for `<batch id>:<n>`.
    records: fn() -> Value,
}

/// A batch runs every statement of `Sqls` serially, in order, and reports each
/// one as its own `SubStatements` entry with the rows the engine produced for
/// it — across batches of different sizes, and over DDL, DML and queries,
/// including statements that read what an earlier statement of the same batch
/// wrote.
#[tokio::test]
async fn a_batch_reports_each_statement_it_ran_in_order() {
    let cases = [
        BatchCase {
            label: "one statement, the modelled minimum",
            sqls: &["SELECT 1"],
            reported: &[(true, 1)],
            records: || json!([[[{"longValue": 1}]]]),
        },
        BatchCase {
            label: "two queries and no writes",
            sqls: &["SELECT 'a' AS letter", "SELECT 2 AS n"],
            reported: &[(true, 1), (true, 1)],
            records: || json!([[[{"stringValue": "a"}]], [[{"longValue": 2}]]]),
        },
        BatchCase {
            label: "DDL only, so no statement has a result set",
            sqls: &[
                "CREATE TABLE a (id INTEGER)",
                "CREATE TABLE b (id INTEGER)",
            ],
            reported: &[(false, 0), (false, 0)],
            records: || json!([[], []]),
        },
        BatchCase {
            label: "DDL, then DML, then a query over what it wrote",
            sqls: &[
                "CREATE TABLE t (id INTEGER)",
                "INSERT INTO t (id) VALUES (1), (2)",
                "SELECT id FROM t ORDER BY id",
            ],
            reported: &[(false, 0), (false, 2), (true, 2)],
            records: || json!([[], [], [[{"longValue": 1}], [{"longValue": 2}]]]),
        },
        BatchCase {
            label: "five statements, each one reading the previous one's work",
            sqls: &[
                "CREATE TABLE s (n INTEGER)",
                "INSERT INTO s (n) VALUES (1)",
                "UPDATE s SET n = n + 10",
                "INSERT INTO s (n) VALUES (2), (3)",
                "SELECT n FROM s ORDER BY n",
            ],
            reported: &[(false, 0), (false, 1), (false, 1), (false, 2), (true, 3)],
            records: || {
                json!([
                    [],
                    [],
                    [],
                    [],
                    [[{"longValue": 2}], [{"longValue": 3}], [{"longValue": 11}]]
                ])
            },
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        let submitted = emulator.batch_and_wait(&batch(case.sqls, None)).await;
        let id = id_of(&submitted);
        let records = (case.records)();

        assert_eq!(
            submitted["Status"], "FINISHED",
            "case {}: a waited-for batch must report its terminal status, got {submitted}",
            case.label
        );
        assert_eq!(
            flag(&submitted, "HasResultSet"),
            case.reported.iter().any(|(has_rows, _)| *has_rows),
            "case {}: HasResultSet is true if any statement returns a result set, got {submitted}",
            case.label
        );
        for (member, expected) in [
            ("ClusterIdentifier", "my-cluster"),
            ("Database", "dev"),
            ("DbUser", "awsuser"),
        ] {
            assert_eq!(
                submitted[member], expected,
                "case {}: {member} must be reported as submitted, got {submitted}",
                case.label
            );
        }

        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {}: the batch is finished, got {described}",
            case.label
        );
        assert_eq!(
            described["ExecutionMode"], "TRANSACTION",
            "case {}: TRANSACTION is the default ExecutionMode, got {described}",
            case.label
        );
        assert!(
            described.get("QueryString").is_none(),
            "case {}: a batch has no single QueryString, got {described}",
            case.label
        );
        // The rows belong to the statements, so the batch reports the modelled
        // -1 null marker for both counters.
        assert_eq!(
            (
                number(&described, "ResultRows"),
                number(&described, "ResultSize")
            ),
            (-1, -1),
            "case {}: a batch's own counters are null, got {described}",
            case.label
        );

        let subs = sub_statements(&described);
        assert_eq!(
            subs.len(),
            case.sqls.len(),
            "case {}: one SubStatements entry per submitted statement, got {described}",
            case.label
        );

        for (index, sub) in subs.iter().enumerate() {
            let (has_result_set, result_rows) = case.reported[index];
            let position = index + 1;
            assert_eq!(
                sub["Id"],
                json!(format!("{id}:{position}")),
                "case {}: statement {position} is identified by its position in the batch, got \
                 {sub}",
                case.label
            );
            assert_eq!(
                sub["QueryString"],
                json!(case.sqls[index]),
                "case {}: statement {position} must report its own text, got {sub}",
                case.label
            );
            assert_eq!(
                sub["Status"], "FINISHED",
                "case {}: statement {position} finished, got {sub}",
                case.label
            );
            assert_eq!(
                flag(sub, "HasResultSet"),
                has_result_set,
                "case {}: statement {position} HasResultSet must be {has_result_set}, got {sub}",
                case.label
            );
            assert_eq!(
                number(sub, "ResultRows"),
                result_rows,
                "case {}: statement {position} must report {result_rows} rows, got {sub}",
                case.label
            );
            assert!(
                sub.get("Error").is_none(),
                "case {}: statement {position} did not fail, so it carries no Error: {sub}",
                case.label
            );
            assert!(
                number(sub, "RedshiftQueryId") > 0 && sub["RedshiftQueryUuid"].is_string(),
                "case {}: statement {position} is a query the cluster ran, so it carries its own \
                 identifiers: {sub}",
                case.label
            );

            // Each statement's rows are fetched through its own `Id` — and a
            // statement that produced no result set has no result to fetch, so
            // that read is refused rather than answered with an empty page.
            if has_result_set {
                let fetched = emulator
                    .ok(
                        "GetStatementResult",
                        &json!({"Id": format!("{id}:{position}")}),
                    )
                    .await;
                assert_eq!(
                    fetched["Records"], records[index],
                    "case {}: statement {position} must return {}, got {fetched}",
                    case.label, records[index]
                );
                assert_eq!(
                    number(&fetched, "TotalNumRows"),
                    i64::try_from(records[index].as_array().map_or(0, Vec::len)).unwrap_or(0),
                    "case {}: statement {position} must report the rows it returned, got \
                     {fetched}",
                    case.label
                );
            } else {
                let (status, refused) = emulator
                    .call(
                        "GetStatementResult",
                        &json!({"Id": format!("{id}:{position}")}),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {}: statement {position} produced no result set, so it has no result \
                     to fetch, got {status} {refused}",
                    case.label
                );
                assert_eq!(
                    error_code(&refused),
                    "ResourceNotFoundException",
                    "case {}: statement {position}: expected ResourceNotFoundException, got \
                     {refused}",
                    case.label
                );
            }

            // And it describes itself through the same `Id`, reporting the
            // BATCH's `Id` as its own: the `:<n>` suffix is the address that
            // reaches it, not the identity of the submission it belongs to.
            let described_sub = emulator
                .ok(
                    "DescribeStatement",
                    &json!({"Id": format!("{id}:{position}")}),
                )
                .await;
            assert_eq!(
                described_sub["QueryString"],
                json!(case.sqls[index]),
                "case {}: statement {position} is readable through its own Id, got \
                 {described_sub}",
                case.label
            );
            assert_eq!(
                described_sub["Id"],
                json!(id),
                "case {}: statement {position} reports the batch's Id, got {described_sub}",
                case.label
            );
        }
    }
}

/// `Sqls` holds "one or more SQL statements", up to the modelled maximum of 40:
/// a batch of exactly 40 runs every one of them, and one of 41 is refused.
#[tokio::test]
async fn a_batch_runs_up_to_the_modelled_maximum_of_forty_statements() {
    // The bound comes from `SqlList`'s modelled length, 1..40
    // (smithy_query(query="shape", shape="SqlList")).
    const MAX_STATEMENTS: usize = 40;

    let emulator = Emulator::new();
    let mut sqls: Vec<String> = vec!["CREATE TABLE counter (n INTEGER)".to_string()];
    while sqls.len() < MAX_STATEMENTS - 1 {
        sqls.push(format!("INSERT INTO counter (n) VALUES ({})", sqls.len()));
    }
    sqls.push("SELECT COUNT(*) AS rows_written FROM counter".to_string());

    let mut request = connection();
    request["Sqls"] = json!(sqls);
    request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let submitted = emulator.ok("BatchExecuteStatement", &request).await;
    let id = id_of(&submitted);
    let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    let subs = sub_statements(&described);
    assert_eq!(
        subs.len(),
        MAX_STATEMENTS,
        "a batch of {MAX_STATEMENTS} statements reports all of them, got {described}"
    );
    assert!(
        subs.iter().all(|sub| sub["Status"] == "FINISHED"),
        "every statement of the batch must have finished, got {described}"
    );
    let fetched = emulator
        .ok(
            "GetStatementResult",
            &json!({"Id": format!("{id}:{MAX_STATEMENTS}")}),
        )
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": MAX_STATEMENTS - 2}]]),
        "the last statement counts the rows the ones before it wrote, got {fetched}"
    );

    // One past the bound is refused by the modelled constraint, and nothing ran.
    let mut too_many: Vec<String> = sqls.clone();
    too_many.push("SELECT 1".to_string());
    let mut request = connection();
    request["Sqls"] = json!(too_many);
    let (status, response) = emulator.call("BatchExecuteStatement", &request).await;
    assert_eq!(
        status, 400,
        "{} statements is past the modelled maximum, got {status} {response}",
        too_many.len()
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response}"
    );
}

// ---------------------------------------------------------------------------
// How a batch commits
// ---------------------------------------------------------------------------

/// One row of the commit table: where the failing statement sits, in which
/// `ExecutionMode`, and what the batch must then report and leave behind.
struct CommitCase {
    label: &'static str,
    mode: &'static str,
    sqls: &'static [&'static str],
    /// The status every statement of the batch must report, in order.
    statuses: &'static [&'static str],
    /// Rows left in `ledger` once the batch has run: the batch's writes either
    /// survived or were rolled back.
    surviving_rows: i64,
}

/// A statement that fails rolls the whole batch back under `TRANSACTION` — the
/// statements before it, the statements after it, whatever the position of the
/// failure — while under `AUTO_COMMIT` every other statement's work is
/// committed and the statements after the failure still run.
#[tokio::test]
async fn a_failing_statement_rolls_a_transaction_back_and_only_a_transaction() {
    let cases = [
        CommitCase {
            label: "TRANSACTION, the last statement fails",
            mode: "TRANSACTION",
            sqls: &[
                "INSERT INTO ledger (n) VALUES (1)",
                "INSERT INTO ledger (n) VALUES (2)",
                "INSERT INTO nowhere (n) VALUES (3)",
            ],
            statuses: &["FINISHED", "FINISHED", "FAILED"],
            surviving_rows: 0,
        },
        CommitCase {
            label: "TRANSACTION, a middle statement fails",
            mode: "TRANSACTION",
            sqls: &[
                "INSERT INTO ledger (n) VALUES (1)",
                "INSERT INTO nowhere (n) VALUES (2)",
                "INSERT INTO ledger (n) VALUES (3)",
            ],
            // The statement after the failure never ran: the batch stopped
            // there and everything it had done was rolled back.
            statuses: &["FINISHED", "FAILED", "ABORTED"],
            surviving_rows: 0,
        },
        CommitCase {
            label: "TRANSACTION, the first statement fails",
            mode: "TRANSACTION",
            sqls: &[
                "SELECT n FROM nowhere",
                "INSERT INTO ledger (n) VALUES (1)",
            ],
            statuses: &["FAILED", "ABORTED"],
            surviving_rows: 0,
        },
        CommitCase {
            label: "AUTO_COMMIT, the last statement fails",
            mode: "AUTO_COMMIT",
            sqls: &[
                "INSERT INTO ledger (n) VALUES (1)",
                "INSERT INTO ledger (n) VALUES (2)",
                "INSERT INTO nowhere (n) VALUES (3)",
            ],
            statuses: &["FINISHED", "FINISHED", "FAILED"],
            surviving_rows: 2,
        },
        CommitCase {
            label: "AUTO_COMMIT, a middle statement fails",
            mode: "AUTO_COMMIT",
            sqls: &[
                "INSERT INTO ledger (n) VALUES (1)",
                "INSERT INTO nowhere (n) VALUES (2)",
                "INSERT INTO ledger (n) VALUES (3)",
            ],
            // "A failure of one statement does not affect the others", so the
            // statement after the failure ran and was committed.
            statuses: &["FINISHED", "FAILED", "FINISHED"],
            surviving_rows: 2,
        },
        CommitCase {
            label: "AUTO_COMMIT, the first statement fails",
            mode: "AUTO_COMMIT",
            sqls: &[
                "SELECT n FROM nowhere",
                "INSERT INTO ledger (n) VALUES (1)",
            ],
            statuses: &["FAILED", "FINISHED"],
            surviving_rows: 1,
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        // Created OUTSIDE the batch, so the table survives a rollback of the
        // batch and the rows it holds afterwards are the batch's own.
        emulator.run("CREATE TABLE ledger (n INTEGER)").await;

        let submitted = emulator
            .batch_and_wait(&batch(case.sqls, Some(case.mode)))
            .await;
        let id = id_of(&submitted);
        assert_eq!(
            submitted["Status"], "FAILED",
            "case {}: a batch with a failing statement failed, got {submitted}",
            case.label
        );

        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["ExecutionMode"],
            json!(case.mode),
            "case {}: the batch reports the mode it was submitted in, got {described}",
            case.label
        );
        let subs = sub_statements(&described);
        let statuses = statuses_of(&subs);
        assert_eq!(
            statuses, case.statuses,
            "case {}: expected {:?}, got {described}",
            case.label, case.statuses
        );

        for (index, sub) in subs.iter().enumerate() {
            let failed = case.statuses[index] == "FAILED";
            assert_eq!(
                sub["Error"].is_string(),
                failed,
                "case {}: statement {} reports the engine's message only when it failed, got \
                 {sub}",
                case.label,
                index + 1
            );
            if failed {
                assert_eq!(
                    described["Error"], sub["Error"],
                    "case {}: the batch reports the failing statement's own error, got \
                     {described}",
                    case.label
                );
            }
        }

        // A batch that did not finish successfully has no result set to fetch.
        let (status, refused) = emulator
            .call("GetStatementResult", &json!({"Id": id}))
            .await;
        assert_eq!(
            status, 400,
            "case {}: a failed batch has no rows to fetch, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {}: expected ValidationException, got {refused}",
            case.label
        );

        // What the batch left behind, read back through a statement of its own.
        let counted = emulator.run("SELECT COUNT(*) AS n FROM ledger").await;
        let rows = emulator
            .ok("GetStatementResult", &json!({"Id": counted}))
            .await;
        assert_eq!(
            rows["Records"],
            json!([[{"longValue": case.surviving_rows}]]),
            "case {}: expected {} surviving rows, got {rows}",
            case.label,
            case.surviving_rows
        );
    }
}

/// A batch's `Parameters` "are available to all SQL statements in the batch.
/// Each statement can reference any subset of the provided parameters" — so a
/// statement that references one of them gets it bound, and a statement that
/// references none still runs.
#[tokio::test]
async fn batch_parameters_are_bound_to_the_statements_that_reference_them() {
    let emulator = Emulator::new();
    let parameters = json!([
        {"name": "first", "value": "alpha"},
        {"name": "second", "value": "beta"},
    ]);
    let sqls = [
        // References no parameter at all.
        "CREATE TABLE labels (label VARCHAR(8))",
        // One each, a different subset per statement.
        "INSERT INTO labels (label) VALUES (:first)",
        "INSERT INTO labels (label) VALUES (:second)",
        // Both at once.
        "SELECT label FROM labels WHERE label IN (:first, :second) ORDER BY label",
    ];

    let mut request = batch(&sqls, None);
    request["Parameters"] = parameters.clone();
    let submitted = emulator.batch_and_wait(&request).await;
    let id = id_of(&submitted);
    assert_eq!(
        submitted["Status"], "FINISHED",
        "every statement binds the parameters it references, got {submitted}"
    );

    let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        described["QueryParameters"], parameters,
        "the submitted parameters must be reported, got {described}"
    );
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": format!("{id}:4")}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"stringValue": "alpha"}], [{"stringValue": "beta"}]]),
        "the last statement must see both parameters' values, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// The batch's lifecycle, listing and idempotency
// ---------------------------------------------------------------------------

/// A batch nobody waited for walks the lifecycle its transaction walks —
/// `STARTED` while the engine runs it, with every statement of it still
/// `SUBMITTED` until the transaction reports; cancelling it stops the whole
/// batch, and one statement of a batch cannot be cancelled on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unwaited_batch_is_submitted_while_its_transaction_runs() {
    let emulator = Emulator::new();
    // The first statement takes real time and the second runs until a bound of
    // the engine stops it, so the transaction is certainly still open while it
    // is read and cancelled below: what keeps that window open is the engine's
    // own execution bound and never a span of this test's choosing.
    let submitted = emulator
        .ok(
            "BatchExecuteStatement",
            &batch(&[SLOW_SQL, UNBOUNDED_SQL], None),
        )
        .await;
    let id = id_of(&submitted);
    assert!(
        ["SUBMITTED", "PICKED", "STARTED"].contains(&submitted["Status"].as_str().unwrap_or("")),
        "a batch nobody waited for is answered with the state it has reached, which is not a \
         terminal one: {submitted}"
    );

    let pending = wait_until_started(&emulator, "a batch whose transaction takes real time", &id)
        .await;
    assert_eq!(
        pending["Status"], "STARTED",
        "the batch whose transaction the engine is running is STARTED: {pending}"
    );
    let pending_subs = sub_statements(&pending);
    assert_eq!(
        statuses_of(&pending_subs),
        ["SUBMITTED", "SUBMITTED"],
        "a batch's statements share its timeline: {pending}"
    );

    // One statement of a batch is not a query that can be cancelled on its own.
    let (status, refused) = emulator
        .call("CancelStatement", &json!({"Id": format!("{id}:1")}))
        .await;
    assert_eq!(
        status, 400,
        "a batch's statement cannot be cancelled on its own, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );

    // Cancelling the batch stops every statement of it.
    let cancelled = emulator.ok("CancelStatement", &json!({"Id": id})).await;
    assert_eq!(
        cancelled["Status"],
        json!(true),
        "a still-running batch is cancellable: {cancelled}"
    );
    let aborted = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        aborted["Status"], "ABORTED",
        "the cancelled batch reports ABORTED: {aborted}"
    );
    let aborted_subs = sub_statements(&aborted);
    assert_eq!(
        statuses_of(&aborted_subs),
        ["ABORTED", "ABORTED"],
        "cancelling a batch stops every statement of it: {aborted}"
    );

    // A batch nobody cancels reaches its terminal status once its transaction
    // has run.
    let other = emulator
        .ok("BatchExecuteStatement", &batch(&["SELECT 1"], None))
        .await;
    // Polled rather than waited out: the batch reports when its transaction
    // does, so what this reads is the outcome that transaction really reached.
    let finished = wait_for_terminal(
        &emulator,
        "a batch nobody waited for",
        &id_of(&other),
    )
    .await;
    assert_eq!(
        finished["Status"], "FINISHED",
        "a batch whose transaction committed reports FINISHED: {finished}"
    );
    let finished_subs = sub_statements(&finished);
    assert_eq!(
        statuses_of(&finished_subs),
        ["FINISHED"],
        "and its statements' statuses with it: {finished}"
    );
}

/// `ListStatements` enumerates a batch ONCE, as a batch statement carrying its
/// statements' texts — never one entry per statement of it.
#[tokio::test]
async fn list_statements_reports_a_batch_once_with_its_query_strings() {
    let emulator = Emulator::new();
    let sqls = ["SELECT 1", "SELECT 2", "SELECT 3"];
    let submitted = emulator.batch_and_wait(&batch(&sqls, None)).await;
    let id = id_of(&submitted);
    let single = emulator.run("SELECT 4").await;

    let page = emulator.ok("ListStatements", &json!({})).await;
    let statements = list(&page, "Statements");
    assert_eq!(
        statements.len(),
        2,
        "a batch is one statement, beside the single one: {page}"
    );

    let batch_entry = statements
        .iter()
        .find(|statement| statement["Id"] == json!(id))
        .unwrap_or_else(|| panic!("the batch must be listed: {page}"));
    assert_eq!(
        batch_entry["IsBatchStatement"],
        json!(true),
        "the batch is reported as a batch statement: {batch_entry}"
    );
    assert_eq!(
        batch_entry["QueryStrings"],
        json!(sqls),
        "the batch reports its statements' texts in order: {batch_entry}"
    );
    assert!(
        batch_entry.get("QueryString").is_none(),
        "a batch has no single QueryString: {batch_entry}"
    );

    let single_entry = statements
        .iter()
        .find(|statement| statement["Id"] == json!(single))
        .unwrap_or_else(|| panic!("the single statement must be listed: {page}"));
    assert_eq!(
        single_entry["IsBatchStatement"],
        json!(false),
        "a single statement is not a batch statement: {single_entry}"
    );
    assert_eq!(
        single_entry["QueryString"], "SELECT 4",
        "a single statement reports its text as QueryString: {single_entry}"
    );
    assert!(
        single_entry.get("QueryStrings").is_none(),
        "and carries no QueryStrings: {single_entry}"
    );
}

/// Repeating a `BatchExecuteStatement` with the same `ClientToken` answers with
/// the batch the first call submitted instead of running a second one.
#[tokio::test]
async fn a_repeated_client_token_replays_the_first_batch() {
    let emulator = Emulator::new();
    let mut request = batch(&["CREATE TABLE once (n INTEGER)"], None);
    request["ClientToken"] = json!("batch-token");

    let first = emulator.batch_and_wait(&request).await;
    let second = emulator.batch_and_wait(&request).await;
    assert_eq!(
        id_of(&first),
        id_of(&second),
        "the same ClientToken must answer with the first batch: {first} then {second}"
    );

    let page = emulator.ok("ListStatements", &json!({})).await;
    assert_eq!(
        list(&page, "Statements").len(),
        1,
        "a replayed batch must not be submitted twice: {page}"
    );

    // And the replay really was a replay: running the same DDL again would have
    // failed, so a second batch would be visible as a failure.
    let described = emulator
        .ok("DescribeStatement", &json!({"Id": id_of(&first)}))
        .await;
    assert_eq!(
        described["Status"], "FINISHED",
        "the replayed batch is the first one, which finished: {described}"
    );

    // A token that names a single statement is not a replay of a batch: the
    // batch is submitted, and answers with a batch's own `Id`.
    let mut single = connection();
    single["Sql"] = json!("SELECT 1");
    single["ClientToken"] = json!("shared-token");
    let executed = emulator.execute_and_wait(&single).await;

    let mut as_batch = batch(&["SELECT 1"], None);
    as_batch["ClientToken"] = json!("shared-token");
    let batched = emulator.batch_and_wait(&as_batch).await;
    assert_ne!(
        id_of(&executed),
        id_of(&batched),
        "a single statement's ClientToken must not answer a batch: {executed} then {batched}"
    );
    let described = emulator
        .ok("DescribeStatement", &json!({"Id": id_of(&batched)}))
        .await;
    assert_eq!(
        sub_statements(&described).len(),
        1,
        "the batch really is a batch: {described}"
    );
}

// ---------------------------------------------------------------------------
// What a batch may retain
// ---------------------------------------------------------------------------

/// A batch costs the retained history itself PLUS each of its statements, so
/// the number of statements the emulator keeps is bounded however a caller
/// submits them.
#[tokio::test]
async fn a_batch_s_statements_count_against_the_retained_statement_bound() {
    const STATEMENTS_PER_BATCH: usize = 4;
    /// What one batch costs the history: itself and each of its statements.
    const PER_BATCH: usize = STATEMENTS_PER_BATCH + 1;

    let emulator = Emulator::new();
    // Twice as many batches as the bound can hold, so eviction has to happen.
    let submissions = 2 * MAX_RETAINED_STATEMENTS / PER_BATCH;
    let mut ids: Vec<String> = Vec::with_capacity(submissions);
    for index in 0..submissions {
        let sqls: Vec<String> = (0..STATEMENTS_PER_BATCH)
            .map(|position| format!("SELECT {index} + {position}"))
            .collect();
        let mut request = connection();
        request["Sqls"] = json!(sqls);
        ids.push(id_of(
            &emulator.ok("BatchExecuteStatement", &request).await,
        ));
    }

    let page = emulator.ok("ListStatements", &json!({})).await;
    let statements = list(&page, "Statements");
    assert_eq!(
        statements.len() * PER_BATCH,
        MAX_RETAINED_STATEMENTS,
        "the history must hold {MAX_RETAINED_STATEMENTS} statements, batches counting their own: \
         got {} batches of {PER_BATCH} in {page}",
        statements.len()
    );

    // The newest submission survived, with all of its statements...
    let newest = ids.last().expect("a submitted batch");
    let described = emulator
        .ok("DescribeStatement", &json!({"Id": newest}))
        .await;
    assert_eq!(
        sub_statements(&described).len(),
        STATEMENTS_PER_BATCH,
        "a retained batch keeps every statement of it: {described}"
    );

    // ...and the oldest was evicted, together with its statements.
    let oldest = ids.first().expect("a submitted batch");
    for id in [oldest.clone(), format!("{oldest}:1")] {
        let (status, response) = emulator
            .call("DescribeStatement", &json!({"Id": id}))
            .await;
        assert_eq!(
            status, 400,
            "the oldest batch and its statements are gone: {id} got {status} {response}"
        );
    }
}

/// One submission may retain one submission's worth of result data: a batch's
/// statements share the budget a single statement has to itself, so a batch
/// cannot keep more rows by submitting more statements.
#[tokio::test]
async fn a_batch_shares_one_retained_result_budget() {
    // Each of these retains well under the rows one submission may keep
    // (`MAX_RESULT_ROWS`), while two of them together are past it.
    const ROWS: usize = 60_000;
    let wide = format!(
        "WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM counter \
         WHERE x < {ROWS}) SELECT x, x + 1 AS y FROM counter"
    );

    // Alone, the statement finishes and keeps every row.
    let emulator = Emulator::new();
    let mut single = connection();
    single["Sql"] = json!(wide);
    let alone = emulator.execute_and_wait(&single).await;
    assert_eq!(
        alone["Status"], "FINISHED",
        "one statement may retain this much on its own: {alone}"
    );
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": id_of(&alone)}))
        .await;
    assert_eq!(
        number(&fetched, "TotalNumRows"),
        i64::try_from(ROWS).unwrap_or(i64::MAX),
        "and every row of it is readable: {}",
        number(&fetched, "TotalNumRows")
    );

    // In a batch, the second one is past what the submission may still retain,
    // so it FAILS rather than reporting a truncated result as a whole one.
    // AUTO_COMMIT, so the failure is the budget's doing and not a rollback.
    let emulator = Emulator::new();
    let submitted = emulator
        .batch_and_wait(&batch(&[&wide, &wide], Some("AUTO_COMMIT")))
        .await;
    let described = emulator
        .ok("DescribeStatement", &json!({"Id": id_of(&submitted)}))
        .await;
    let subs = sub_statements(&described);
    assert_eq!(
        statuses_of(&subs),
        ["FINISHED", "FAILED"],
        "a batch's statements share one retained-result budget: {described}"
    );
    assert!(
        subs[1]["Error"].is_string(),
        "the statement past the budget reports why it failed: {described}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// `BatchExecuteStatement` refuses a request that does not name exactly one
/// data warehouse, one database and one authorization method, or whose `Sqls`
/// or `ExecutionMode` names nothing runnable — and stores nothing for any of
/// them.
#[tokio::test]
async fn batch_execute_statement_refuses_a_request_it_cannot_run() {
    let emulator = Emulator::new();
    let cases: [(&str, Value); 8] = [
        (
            "neither a cluster nor a workgroup",
            json!({"Database": "dev", "Sqls": ["SELECT 1"]}),
        ),
        (
            "both a cluster and a workgroup",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "Sqls": ["SELECT 1"],
            }),
        ),
        (
            "no database",
            json!({
                "ClusterIdentifier": "my-cluster",
                "DbUser": "awsuser",
                "Sqls": ["SELECT 1"],
            }),
        ),
        (
            "two authorization methods at once",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
                "Sqls": ["SELECT 1"],
            }),
        ),
        // `Sqls` is modelled with a minimum length of 1.
        ("no statements at all", batch(&[], None)),
        ("a blank statement", batch(&["SELECT 1", "   "], None)),
        (
            "an ExecutionMode the model does not declare",
            batch(&["SELECT 1"], Some("ROLLBACK_ALWAYS")),
        ),
        (
            "an ExecutionMode in the wrong case",
            batch(&["SELECT 1"], Some("transaction")),
        ),
    ];

    for (label, request) in &cases {
        let (status, response) = emulator.call("BatchExecuteStatement", request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label}: expected ValidationException, got {response}"
        );
    }

    // And nothing was stored for any of them.
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );

    // The session-reuse form carries its connection on the SESSION, so a
    // `SessionId` that names no live session names no warehouse and no
    // database either: a missing resource, not a 400 that would blame the
    // request.
    let (status, response) = emulator
        .call(
            "BatchExecuteStatement",
            &json!({
                "SessionId": "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e",
                "Sqls": ["SELECT 1"],
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "a SessionId naming no live session is a missing resource: got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response}"
    );
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );
}

/// An `Id` whose `:<n>` suffix names no statement of a batch is refused as a
/// query the service does not hold, and so is a suffix on a statement that is not
/// a batch at all.
#[tokio::test]
async fn an_id_naming_no_statement_of_a_batch_is_refused() {
    let emulator = Emulator::new();
    let batch_id = id_of(&emulator.batch_and_wait(&batch(&["SELECT 1"], None)).await);
    let single = emulator.run("SELECT 2").await;

    let cases = [
        ("past the end of the batch", format!("{batch_id}:2")),
        ("before the first statement", format!("{batch_id}:0")),
        ("a suffix on a single statement", format!("{single}:1")),
    ];

    for (label, id) in &cases {
        for operation in ["DescribeStatement", "GetStatementResult", "CancelStatement"] {
            let (status, response) = emulator.call(operation, &json!({"Id": id})).await;
            assert_eq!(
                status, 400,
                "case {label} on {operation}: expected 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {label} on {operation}: expected ResourceNotFoundException, got {response}"
            );
        }
    }
}
