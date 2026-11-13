// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue data-quality ruleset-evaluation slice:
//! `StartDataQualityRulesetEvaluationRun` and the
//! `GetDataQualityRulesetEvaluationRun` that reports the run it started.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a start of STORED rulesets over a STORED data source records an
//! evaluation run and answers 200 with the `RunId` it minted; the run carries the data
//! sources it evaluates, the rulesets it evaluates over them and the configuration the
//! request named — with every member the request left unset filled in with the default the
//! model documents for it — and the read reports it under that id, in a non-terminal
//! `TaskStatusType` until the status the clock owes it falls due and in a terminal one
//! afterwards, where a run whose tables the catalog still holds succeeds and reports one
//! result id per ruleset it evaluated and a run whose table the catalog has lost fails
//! carrying the catalog's own miss. A start naming a ruleset this account holds none of is
//! refused as bad input, a start addressing a table the catalog holds nothing for is refused
//! as the missing entity it is, and a start repeating a `ClientToken` an earlier start
//! already used is refused as the duplicate it is — a token stands for one start — while two
//! runs of one ruleset over one table are both accepted, each computing its own results.
//!
//! The recorded cases — a start refused for an unknown ruleset, a start refused as a
//! duplicate, and a run read to its terminal state — are ONE row each of the tables below;
//! the rows differ along which `DataSource` member carries the table, how many rulesets the
//! start lists, how many additional data sources it names, which optional members it names,
//! the number of workers, the timeout, the casing of the names and the catalog id.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather than
//! slept for, so the transient window is deterministic: one test sleeps, to pin that the
//! transition is owed by the wall clock and not by a reader.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DataQualityRulesetEvaluationRunEntry,
    DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER, DATA_QUALITY_RUN_STATE_FAILED,
    DATA_QUALITY_RUN_STATE_STARTING, DATA_QUALITY_RUN_STATE_SUCCEEDED,
    DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES, DEFAULT_DATA_QUALITY_RUN_WORKERS,
    MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS, TERMINAL_DATA_QUALITY_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a start names: "An IAM role supplied to encrypt the results of the run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// The lower and upper bound of the window a transition this emulator owes falls due in.
/// [`DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER`] must sit inside it, and so must
/// every run's stored due instant.
const OWED_TRANSITION_WINDOW: (Duration, Duration) =
    (Duration::from_millis(250), Duration::from_millis(750));

/// An emulator plus the engine it runs on, so a test can read and move the due instants the
/// run lifecycle turns on instead of waiting for them.
///
/// Amazon S3 is mounted on the same engine because an evaluation run READS the objects
/// behind the data source's table: the run reaches them over the emulator's own in-process
/// callback, so the object [`store_table`] stages is the data its rules are checked against.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(engine.clone());
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Store `database` holding `table`, and answer nothing: the catalog state the starts below
/// are resolved against. A database several rows share is created once — a repeat create is
/// the catalog's `AlreadyExistsException`, which is not what these tests are about.
///
/// The table points at a prefix of this emulator's OWN Amazon S3 and one object is staged
/// there, because a run evaluates its rules against the table's data: a table pointing at
/// nothing readable is a run that evaluates nothing, which the refusal suites cover.
async fn store_table(router: &MultiServiceRouter, database: &str, table: &str) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    stage_table_data(router, table).await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://{DATA_BUCKET}/{}/", table.to_lowercase()),
                    "Columns": [{ "Name": "id", "Type": "string" }],
                },
            },
        }),
    )
    .await;
}

/// The bucket every row's table points its data at.
const DATA_BUCKET: &str = "dq-evaluation-data";

/// Put one object under the prefix `table`'s storage descriptor names, creating the bucket
/// the first time: one row carrying the one column those tables declare. The client is the
/// shared harness's ([`crate::wire::s3_client`]).
async fn stage_table_data(router: &MultiServiceRouter, table: &str) {
    let client = crate::wire::s3_client(router);
    // The bucket is shared by every row, so the create of an existing one is no failure.
    let _ = client.create_bucket().bucket(DATA_BUCKET).send().await;
    client
        .put_object()
        .bucket(DATA_BUCKET)
        .key(format!("{}/rows.csv", table.to_lowercase()))
        .body(aws_sdk_s3::primitives::ByteStream::from(
            b"row-1\n".to_vec(),
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("the data of {table} must be staged, got {error:?}"));
}

/// Store a data quality ruleset under `name`, so a start may evaluate it.
async fn store_ruleset(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDataQualityRuleset",
        &json!({ "Name": name, "Ruleset": "Rules = [\n    ColumnExists \"id\"\n]" }),
    )
    .await;
}

/// A `DataSource` addressing `database`.`table` through its plain `GlueTable` member, plus
/// whatever `extra` adds to that table reference.
fn glue_table_source(database: &str, table: &str, extra: Value) -> Value {
    let mut glue_table = json!({ "DatabaseName": database, "TableName": table });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            glue_table[member] = value.clone();
        }
    }
    json!({ "GlueTable": glue_table })
}

/// A start body naming the three `@required` members, plus whatever `extra` adds.
fn start_body(data_source: &Value, ruleset_names: &[&str], extra: Value) -> Value {
    let mut body = json!({
        "DataSource": data_source,
        "Role": ROLE,
        "RulesetNames": ruleset_names,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Start an evaluation run and answer the `RunId` it reports.
async fn start_run(router: &MultiServiceRouter, body: &Value) -> String {
    let started = ok(router, "StartDataQualityRulesetEvaluationRun", body).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("StartDataQualityRulesetEvaluationRun must answer a RunId, got {started}")
        })
        .to_string()
}

/// What `GetDataQualityRulesetEvaluationRun` reports for one run.
async fn get_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    ok(
        router,
        "GetDataQualityRulesetEvaluationRun",
        &json!({ "RunId": run_id }),
    )
    .await
}

/// The ruleset evaluation runs the store holds.
fn stored_runs(engine: &Arc<engine::InMemoryEngine>) -> Vec<DataQualityRulesetEvaluationRunEntry> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store.data_quality_ruleset_evaluation_runs.clone()
}

/// Move the due instant of every unsettled run to `at`, so a test can hold the transient
/// window open or make the owed transition due without sleeping.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for run in &mut store.data_quality_ruleset_evaluation_runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

/// An instant far enough out that no owed transition falls due during a test.
fn far_future() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant already past, so the next call settles what the clock owed.
fn already_past() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// A numeric member of a reported run, taking an absent member as the model's own default of
/// 0 — awsJson omits a member whose value equals its modelled default.
fn reported_number(run: &Value, member: &str) -> f64 {
    run.get(member).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The `ResultIds` a reported run carries, in the order it reports them.
fn reported_result_ids(run: &Value) -> Vec<String> {
    run.get("ResultIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `id` is one of this service's minted data quality ids: 32 lowercase hexadecimal
/// characters, which is inside `HashString`'s `length 1..255`.
fn is_minted_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// One row of the start-and-read-back table below: the catalog state, the start over it, and
/// what the reported run must carry. A named structure rather than a tuple, so every column
/// of the table reads as what it is.
struct StartCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The rulesets stored before the start, which are the ones it lists.
    rulesets: &'a [&'a str],
    /// The `DataSource` the start carries.
    data_source: Value,
    /// The start members beyond the three `@required` ones.
    extra: Value,
    /// The `NumberOfWorkers` the run must report.
    expected_workers: f64,
    /// The `Timeout` in minutes the run must report.
    expected_timeout: f64,
}

/// What `StartDataQualityRulesetEvaluationRun` records is what
/// `GetDataQualityRulesetEvaluationRun` reports: the data sources the run evaluates, the
/// rulesets it evaluates over them, the configuration the request named, the documented
/// default of every member it left unset, and — once the clock has finished the run — one
/// result id per ruleset it evaluated.
///
/// The rows differ along which `DataSource` member carries the table, how many rulesets the
/// start lists (one, several), how many additional data sources it names (none, one, two),
/// which optional members it names, the number of workers, the timeout (absent, the
/// published lower bound, a value well above it), the casing of the names and whether the
/// start names a catalog id.
#[tokio::test]
async fn a_started_run_is_read_back_through_get_data_quality_ruleset_evaluation_run() {
    let (router, engine) = emulator_with_engine();
    // The table every additional data source below points at, stored once.
    store_table(&router, "dq_db", "lookup").await;

    let cases: Vec<StartCase<'_>> = vec![
        StartCase {
            // The recorded case's start: a plain `GlueTable`, one ruleset, nothing optional.
            case: "a GlueTable, one ruleset and no optional member",
            database: "dq_db",
            table: "events",
            rulesets: &["events-ruleset"],
            data_source: glue_table_source("dq_db", "events", json!({})),
            extra: json!({}),
            expected_workers: f64::from(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            expected_timeout: f64::from(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
        },
        StartCase {
            case: "several rulesets and every optional member named",
            database: "dq_db",
            table: "orders",
            rulesets: &["orders-ruleset", "orders-extra-ruleset", "orders-third"],
            data_source: glue_table_source(
                "dq_db",
                "orders",
                json!({ "ConnectionName": "dq-connection",
                        "AdditionalOptions": { "pushDownPredicate": "day=1" } }),
            ),
            extra: json!({
                "NumberOfWorkers": 2,
                "Timeout": 10,
                "AdditionalRunOptions": { "CloudWatchMetricsEnabled": true,
                                          "ResultsS3Prefix": "s3://bucket/results/" },
                "AdditionalDataSources": {
                    "reference_one": { "GlueTable": { "DatabaseName": "dq_db",
                                                      "TableName": "lookup" } },
                },
            }),
            expected_workers: 2.0,
            expected_timeout: 10.0,
        },
        StartCase {
            // The other member a `DataSource` can carry: "an Glue table for Data Quality
            // Operations", which addresses a table the same way.
            case: "a DataQualityGlueTable and two additional data sources",
            database: "dq_db",
            table: "customers",
            rulesets: &["customers-ruleset"],
            data_source: json!({
                "DataQualityGlueTable": {
                    "DatabaseName": "dq_db",
                    "TableName": "customers",
                    "PreProcessingQuery": "SELECT * FROM customers",
                }
            }),
            extra: json!({
                "AdditionalDataSources": {
                    "second_reference": { "GlueTable": { "DatabaseName": "dq_db",
                                                         "TableName": "lookup" } },
                    "first_reference": { "DataQualityGlueTable": { "DatabaseName": "dq_db",
                                                                   "TableName": "events" } },
                },
                "Timeout": 1,
                "NumberOfWorkers": 1,
            }),
            // `Timeout` has `range 1..`: the published lower bound.
            expected_workers: 1.0,
            expected_timeout: 1.0,
        },
        StartCase {
            // Database and table names are folded to lowercase when stored, so a start that
            // spells them in another casing still addresses the stored table. A ruleset name
            // is NOT folded — the model documents no fold for it — so it is listed as stored.
            case: "mixed-case names, an explicit catalog id and a timeout well above the bound",
            database: "DQ_DB_Mixed",
            table: "Daily_Sales",
            rulesets: &["Mixed_Case-Ruleset"],
            data_source: glue_table_source(
                "DQ_DB_Mixed",
                "Daily_Sales",
                json!({ "CatalogId": ACCOUNT_ID }),
            ),
            extra: json!({ "Timeout": 525_600, "NumberOfWorkers": 100 }),
            expected_workers: 100.0,
            expected_timeout: 525_600.0,
        },
    ];

    let mut minted_result_ids: BTreeSet<String> = BTreeSet::new();
    for StartCase {
        case,
        database,
        table,
        rulesets,
        data_source,
        extra,
        expected_workers,
        expected_timeout,
    } in cases
    {
        for ruleset in rulesets {
            store_ruleset(&router, ruleset).await;
        }
        store_table(&router, database, table).await;
        let run_id = start_run(&router, &start_body(&data_source, rulesets, extra.clone())).await;
        assert!(
            is_minted_id(&run_id),
            "case {case:?}: the run id must be 32 lowercase hex characters, got {run_id:?}"
        );

        // Hold the owed transition open, so the run is read in its transient status.
        set_due_instant(&engine, far_future());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("RunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the run is reported under the id the start minted, got {run}"
        );
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_STARTING),
            "case {case:?}: a run whose owed transition is not due yet is still \
             {DATA_QUALITY_RUN_STATE_STARTING}, got {run}"
        );
        assert_echoed(
            case,
            &data_source,
            run.get("DataSource").unwrap_or(&Value::Null),
        );
        assert_echoed(
            case,
            extra.get("AdditionalDataSources").unwrap_or(&Value::Null),
            run.get("AdditionalDataSources").unwrap_or(&Value::Null),
        );
        assert_eq!(
            run.get("Role").and_then(Value::as_str),
            Some(ROLE),
            "case {case:?}: the run reports the role the start named, got {run}"
        );
        assert_eq!(
            run.get("RulesetNames")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(rulesets.to_vec()),
            "case {case:?}: the run reports the rulesets the start listed, got {run}"
        );
        assert_eq!(
            reported_number(&run, "NumberOfWorkers"),
            expected_workers,
            "case {case:?}: expected NumberOfWorkers {expected_workers}, got {run}"
        );
        assert_eq!(
            reported_number(&run, "Timeout"),
            expected_timeout,
            "case {case:?}: expected Timeout {expected_timeout}, got {run}"
        );
        assert_eq!(
            run.get("AdditionalRunOptions"),
            extra.get("AdditionalRunOptions"),
            "case {case:?}: the run reports the additional run options the start named, got {run}"
        );
        assert!(
            run.get("CompletedOn").is_none() && run.get("ResultIds").is_none(),
            "case {case:?}: a run that has not finished has neither a CompletedOn nor a result \
             id, got {run}"
        );
        assert!(
            run.get("ErrorString").is_none(),
            "case {case:?}: a run that has not failed carries no ErrorString, got {run}"
        );
        assert_eq!(
            reported_number(&run, "ExecutionTime"),
            0.0,
            "case {case:?}: a run still in flight has consumed no completed time, got {run}"
        );
        let started_on = reported_number(&run, "StartedOn");
        assert!(
            started_on > 0.0
                && (reported_number(&run, "LastModifiedOn") - started_on).abs() < f64::EPSILON,
            "case {case:?}: a run that has not moved reports one instant for its start and its \
             last modification, got {run}"
        );

        // A read reports the run, it never moves it: a second read of the same not-yet-due
        // run answers exactly what the first did.
        let read_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_again, run,
            "case {case:?}: reading a run must not change what a later caller sees, got \
             {read_again} after {run}"
        );

        // Now let the clock's owed status fall due: the run finishes over the stored tables.
        set_due_instant(&engine, already_past());
        let settled = get_run(&router, &run_id).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
            "case {case:?}: a run over stored tables finishes \
             {DATA_QUALITY_RUN_STATE_SUCCEEDED}, got {settled}"
        );
        assert!(
            TERMINAL_DATA_QUALITY_RUN_STATES
                .contains(&settled["Status"].as_str().unwrap_or_default()),
            "case {case:?}: the settled status must be terminal, got {settled}"
        );
        let result_ids = reported_result_ids(&settled);
        assert_eq!(
            result_ids.len(),
            rulesets.len(),
            "case {case:?}: a finished run reports one result id per ruleset it evaluated, got \
             {settled}"
        );
        for result_id in &result_ids {
            assert!(
                is_minted_id(result_id),
                "case {case:?}: a result id must be 32 lowercase hex characters, got \
                 {result_id:?}"
            );
            assert!(
                minted_result_ids.insert(result_id.clone()),
                "case {case:?}: every result the service mints is its own, got {result_id:?} twice"
            );
        }
        assert!(
            settled.get("ErrorString").is_none(),
            "case {case:?}: a run that finished its evaluation carries no ErrorString, got \
             {settled}"
        );
        assert!(
            reported_number(&settled, "CompletedOn") > 0.0,
            "case {case:?}: a finished run reports a CompletedOn, got {settled}"
        );
        let read_settled_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_settled_again, settled,
            "case {case:?}: a settled run reports the same instants and ids on every later read, \
             got {read_settled_again} after {settled}"
        );
    }

    assert!(
        stored_runs(&engine).len() <= MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS,
        "case {:?}: the store keeps at most {MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS} runs, got \
         {}",
        "the retention bound",
        stored_runs(&engine).len()
    );
}

// ---------------------------------------------------------------------------
// What a start has to address
// ---------------------------------------------------------------------------

/// Every ruleset a start lists has to be one this account holds: a start naming one it does
/// not is refused as bad input, carrying the catalog's own ruleset miss and naming the first
/// name of the list that resolved to nothing, and records no run.
#[tokio::test]
async fn a_start_naming_a_ruleset_this_account_does_not_hold_is_refused() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "miss_db", "miss_table").await;
    store_ruleset(&router, "stored-ruleset").await;
    store_ruleset(&router, "second-stored-ruleset").await;

    // (case, the rulesets the start lists, the name the refusal must carry)
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "one unknown ruleset",
            vec!["no-such-ruleset"],
            "no-such-ruleset",
        ),
        (
            "an unknown ruleset after a stored one",
            vec!["stored-ruleset", "also-missing"],
            "also-missing",
        ),
        (
            "an unknown ruleset before a stored one",
            vec!["missing-first", "stored-ruleset"],
            "missing-first",
        ),
        (
            // A ruleset name is not folded, so another casing of a stored name is another
            // ruleset — and this account holds none under it.
            "a stored ruleset's name in another casing",
            vec!["STORED-RULESET"],
            "STORED-RULESET",
        ),
        (
            // `RulesetNames` is `length 1..10`: the published upper bound, with the last
            // entry unknown.
            "ten rulesets, the last of them unknown",
            vec![
                "stored-ruleset",
                "stored-ruleset",
                "stored-ruleset",
                "stored-ruleset",
                "stored-ruleset",
                "second-stored-ruleset",
                "second-stored-ruleset",
                "second-stored-ruleset",
                "second-stored-ruleset",
                "tenth-missing",
            ],
            "tenth-missing",
        ),
    ];

    for (case, rulesets, expected_name) in cases {
        let (status, parsed) = call(
            &router,
            "StartDataQualityRulesetEvaluationRun",
            &start_body(
                &glue_table_source("miss_db", "miss_table", json!({})),
                &rulesets,
                json!({}),
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a start naming a ruleset this account does not hold is refused, got \
             {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            format!(
                "Cannot find Data Quality Ruleset in account {ACCOUNT_ID} with name \
                 {expected_name}"
            ),
            "case {case:?}: expected the ruleset miss naming {expected_name:?}, got {:?}",
            error_message(&parsed)
        );
    }
    assert!(
        stored_runs(&engine).is_empty(),
        "case {:?}: a refused start records no run, got {:?}",
        "the refusals persisted nothing",
        stored_runs(&engine)
    );
}

/// A start addresses the table the run would read: a data source the catalog holds nothing
/// for is the operation's modelled miss, carrying the catalog's own reason, whether it is the
/// run's own data source or one of the additional ones; and a data source that names neither
/// kind of Glue table is bad input.
#[tokio::test]
async fn a_start_whose_data_source_the_catalog_does_not_hold_is_refused() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "evaluated_db", "evaluated_table").await;
    store_ruleset(&router, "evaluated-ruleset").await;

    // (case, the start's body, the expected error code, the refusal's exact wording)
    let cases: Vec<(&str, Value, &str, String)> = vec![
        (
            "a database the catalog does not hold",
            start_body(
                &glue_table_source("no_such_db", "evaluated_table", json!({})),
                &["evaluated-ruleset"],
                json!({}),
            ),
            "EntityNotFoundException",
            "Database no_such_db not found.".to_string(),
        ),
        (
            "a table the database does not hold",
            start_body(
                &glue_table_source("evaluated_db", "no_such_table", json!({})),
                &["evaluated-ruleset"],
                json!({}),
            ),
            "EntityNotFoundException",
            "Table no_such_table not found.".to_string(),
        ),
        (
            // The refusal names the table as THIS request spelled it, not as the catalog
            // folds a name to store it.
            "a missing table spelled in mixed case",
            start_body(
                &glue_table_source("Evaluated_DB", "No_Such_Table", json!({})),
                &["evaluated-ruleset"],
                json!({}),
            ),
            "EntityNotFoundException",
            "Table No_Such_Table not found.".to_string(),
        ),
        (
            "a catalog of another account",
            start_body(
                &glue_table_source(
                    "evaluated_db",
                    "evaluated_table",
                    json!({ "CatalogId": "210987654321" }),
                ),
                &["evaluated-ruleset"],
                json!({}),
            ),
            "EntityNotFoundException",
            "Database evaluated_db not found.".to_string(),
        ),
        (
            "an additional data source the catalog does not hold",
            start_body(
                &glue_table_source("evaluated_db", "evaluated_table", json!({})),
                &["evaluated-ruleset"],
                json!({ "AdditionalDataSources": {
                    "reference": { "GlueTable": { "DatabaseName": "evaluated_db",
                                                  "TableName": "absent_table" } } } }),
            ),
            "EntityNotFoundException",
            "Table absent_table not found.".to_string(),
        ),
        (
            "a data source naming neither kind of table",
            start_body(&json!({}), &["evaluated-ruleset"], json!({})),
            "InvalidInputException",
            "DataSource must specify either a GlueTable or a DataQualityGlueTable.".to_string(),
        ),
        (
            "an additional data source naming neither kind of table",
            start_body(
                &glue_table_source("evaluated_db", "evaluated_table", json!({})),
                &["evaluated-ruleset"],
                json!({ "AdditionalDataSources": { "reference": {} } }),
            ),
            "InvalidInputException",
            "DataSource must specify either a GlueTable or a DataQualityGlueTable.".to_string(),
        ),
    ];

    for (case, body, expected_code, expected_message) in cases {
        let (status, parsed) = call(&router, "StartDataQualityRulesetEvaluationRun", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the start is refused with 400, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            expected_message,
            "case {case:?}: expected the refusal {expected_message:?}, got {:?}",
            error_message(&parsed)
        );
    }
    assert!(
        stored_runs(&engine).is_empty(),
        "case {:?}: a refused start records no run, got {:?}",
        "the refusals persisted nothing",
        stored_runs(&engine)
    );
}

// ---------------------------------------------------------------------------
// Idempotency, and what is NOT a conflict
// ---------------------------------------------------------------------------

/// `ClientToken` is "used for idempotency … to avoid creating or starting multiple instances
/// of the same resource": a start repeating a token an earlier start already used is refused
/// as a duplicate and records no second run, whatever else it names, while a start under a
/// token of its own is a run of its own.
#[tokio::test]
async fn a_start_repeating_a_client_token_is_refused_as_a_duplicate() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "token_db", "token_table").await;
    store_table(&router, "token_db", "other_table").await;
    store_ruleset(&router, "token-ruleset").await;
    store_ruleset(&router, "other-ruleset").await;
    set_due_instant(&engine, far_future());

    let token = "11111111-2222-3333-4444-555555555555";
    let body = start_body(
        &glue_table_source("token_db", "token_table", json!({})),
        &["token-ruleset"],
        json!({ "ClientToken": token }),
    );
    let first = start_run(&router, &body).await;
    set_due_instant(&engine, far_future());

    // (case, a start that carries the token again)
    let duplicates: Vec<(&str, Value)> = vec![
        ("the same start again", body.clone()),
        (
            "the same token with another number of workers",
            start_body(
                &glue_table_source("token_db", "token_table", json!({})),
                &["token-ruleset"],
                json!({ "ClientToken": token, "NumberOfWorkers": 7 }),
            ),
        ),
        (
            "the same token with another data source",
            start_body(
                &glue_table_source("token_db", "other_table", json!({})),
                &["token-ruleset"],
                json!({ "ClientToken": token }),
            ),
        ),
        (
            "the same token with another ruleset",
            start_body(
                &glue_table_source("token_db", "token_table", json!({})),
                &["other-ruleset"],
                json!({ "ClientToken": token }),
            ),
        ),
        ("the same token with another role", {
            let mut other = body.clone();
            other["Role"] = json!("arn:aws:iam::000000000000:role/another-role");
            other
        }),
    ];
    for (case, duplicate) in duplicates {
        let (status, parsed) =
            call(&router, "StartDataQualityRulesetEvaluationRun", &duplicate).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a start repeating a spent token is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            format!("Idempotent start Data Quality run with token {token}"),
            "case {case:?}: expected the duplicate-start refusal naming the token, got {:?}",
            error_message(&parsed)
        );
    }
    assert_eq!(
        stored_runs(&engine).len(),
        1,
        "case {:?}: a refused start records no run, got {:?}",
        "the refusals persisted nothing",
        stored_runs(&engine)
    );

    // A token of its own is a run of its own.
    let other = start_run(
        &router,
        &start_body(
            &glue_table_source("token_db", "other_table", json!({})),
            &["token-ruleset"],
            json!({ "ClientToken": "66666666-7777-8888-9999-000000000000" }),
        ),
    )
    .await;
    assert_ne!(
        other, first,
        "case {:?}: a start under another token is another run, got {other} twice",
        "another client token"
    );
}

/// Two evaluations of one ruleset over one table are both accepted: each run computes its own
/// results, so a run in flight is no conflict for the next start, whatever it addresses.
#[tokio::test]
async fn a_run_in_flight_does_not_refuse_the_next_start() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "busy_db", "busy_table").await;
    store_table(&router, "busy_db", "other_table").await;
    store_ruleset(&router, "busy-ruleset").await;
    store_ruleset(&router, "idle-ruleset").await;

    // (case, the start that must be accepted while the ones before it are in flight)
    let accepted: Vec<(&str, Value)> = vec![
        (
            "the first run of the ruleset over the table",
            start_body(
                &glue_table_source("busy_db", "busy_table", json!({})),
                &["busy-ruleset"],
                json!({}),
            ),
        ),
        (
            "the same ruleset over the same table again",
            start_body(
                &glue_table_source("busy_db", "busy_table", json!({})),
                &["busy-ruleset"],
                json!({}),
            ),
        ),
        (
            "the same table in another casing",
            start_body(
                &glue_table_source("BUSY_DB", "Busy_Table", json!({})),
                &["busy-ruleset"],
                json!({}),
            ),
        ),
        (
            "another ruleset over the same table",
            start_body(
                &glue_table_source("busy_db", "busy_table", json!({})),
                &["idle-ruleset"],
                json!({}),
            ),
        ),
        (
            "the same ruleset over another table",
            start_body(
                &glue_table_source("busy_db", "other_table", json!({})),
                &["busy-ruleset"],
                json!({}),
            ),
        ),
    ];

    let mut run_ids: Vec<String> = Vec::new();
    for (case, body) in accepted {
        let run_id = start_run(&router, &body).await;
        // Every run started so far stays in flight, so each row is answered with a run of
        // its own while the ones before it have not finished.
        set_due_instant(&engine, far_future());
        assert!(
            !run_ids.contains(&run_id),
            "case {case:?}: each start mints its own run id, got {run_id} twice"
        );
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_STARTING),
            "case {case:?}: the accepted run is in flight, got {run}"
        );
        run_ids.push(run_id);
    }
    assert_eq!(
        stored_runs(&engine).len(),
        run_ids.len(),
        "case {:?}: every start recorded its own run, got {:?}",
        "one run per start",
        stored_runs(&engine).len()
    );
}

// ---------------------------------------------------------------------------
// What the clock settles the run into
// ---------------------------------------------------------------------------

/// One row of the settle table below: the catalog state, what is dropped while the run is in
/// flight, and what the settled run must report.
struct SettleCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The additional data source the start names, as `(database, table)`.
    additional: Option<(&'a str, &'a str)>,
    /// The table dropped before the run settles, as `(database, table)`.
    dropped: Option<(&'a str, &'a str)>,
    /// The `ErrorString` the settled run must carry, or `None` when it must succeed.
    expected_error: Option<&'a str>,
}

/// A run can only be evaluated over tables the catalog still holds: one whose table is
/// dropped while it is in flight fails when the clock settles it, carrying the catalog's own
/// miss and no result at all, while a run whose tables are still there succeeds.
///
/// The rows differ along which of the run's data sources loses its table — its own or an
/// additional one — and whether one is lost at all.
#[tokio::test]
async fn a_run_whose_table_is_dropped_while_it_is_in_flight_fails() {
    let (router, engine) = emulator_with_engine();
    store_ruleset(&router, "dropped-ruleset").await;

    let cases: Vec<SettleCase<'_>> = vec![
        SettleCase {
            case: "the evaluated table is dropped",
            database: "drop_db",
            table: "dropped_table",
            additional: None,
            dropped: Some(("drop_db", "dropped_table")),
            expected_error: Some("Table dropped_table not found."),
        },
        SettleCase {
            case: "an additional data source's table is dropped",
            database: "drop_db",
            table: "kept_primary",
            additional: Some(("drop_db", "dropped_additional")),
            dropped: Some(("drop_db", "dropped_additional")),
            expected_error: Some("Table dropped_additional not found."),
        },
        SettleCase {
            // The failed run names the table as the START spelled it, not as the catalog
            // folds a name to store it.
            case: "the evaluated table is dropped, addressed in mixed case",
            database: "Drop_DB_Mixed",
            table: "Dropped_Mixed",
            additional: None,
            dropped: Some(("Drop_DB_Mixed", "Dropped_Mixed")),
            expected_error: Some("Table Dropped_Mixed not found."),
        },
        SettleCase {
            case: "every evaluated table is still stored",
            database: "drop_db",
            table: "kept_table",
            additional: Some(("drop_db", "kept_additional")),
            dropped: None,
            expected_error: None,
        },
    ];

    for SettleCase {
        case,
        database,
        table,
        additional,
        dropped,
        expected_error,
    } in cases
    {
        store_table(&router, database, table).await;
        let mut extra = json!({});
        if let Some((additional_database, additional_table)) = additional {
            store_table(&router, additional_database, additional_table).await;
            extra = json!({ "AdditionalDataSources": {
                "reference": glue_table_source(additional_database, additional_table, json!({})) } });
        }
        let run_id = start_run(
            &router,
            &start_body(
                &glue_table_source(database, table, json!({})),
                &["dropped-ruleset"],
                extra,
            ),
        )
        .await;
        set_due_instant(&engine, far_future());
        if let Some((dropped_database, dropped_table)) = dropped {
            ok(
                &router,
                "DeleteTable",
                &json!({ "DatabaseName": dropped_database, "Name": dropped_table }),
            )
            .await;
        }

        set_due_instant(&engine, already_past());
        let settled = get_run(&router, &run_id).await;
        let expected_status = if expected_error.is_some() {
            DATA_QUALITY_RUN_STATE_FAILED
        } else {
            DATA_QUALITY_RUN_STATE_SUCCEEDED
        };
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(expected_status),
            "case {case:?}: expected the run to finish {expected_status}, got {settled}"
        );
        assert_eq!(
            settled.get("ErrorString").and_then(Value::as_str),
            expected_error,
            "case {case:?}: expected the error string {expected_error:?}, got {settled}"
        );
        assert_eq!(
            reported_result_ids(&settled).is_empty(),
            expected_error.is_some(),
            "case {case:?}: a run that evaluated nothing reports no result id, got {settled}"
        );
    }
}

/// The transition this emulator owes a run is due inside the window the project fixes, and
/// never at once: a reader straight after the start still sees the transient status, and the
/// CLOCK alone makes the terminal status due for a run nobody read.
#[tokio::test]
async fn the_owed_transition_is_due_inside_the_chosen_window() {
    let (lower, upper) = OWED_TRANSITION_WINDOW;
    assert!(
        DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER >= lower
            && DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER <= upper,
        "case {:?}: the owed transition must fall due in {lower:?}..={upper:?}, got {:?}",
        "the configured span",
        DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER
    );

    let (router, engine) = emulator_with_engine();
    store_table(&router, "window_db", "window_table").await;
    store_ruleset(&router, "window-ruleset").await;
    let before = SystemTime::now();
    let run_id = start_run(
        &router,
        &start_body(
            &glue_table_source("window_db", "window_table", json!({})),
            &["window-ruleset"],
            json!({}),
        ),
    )
    .await;

    let runs = stored_runs(&engine);
    let run = runs
        .iter()
        .find(|run| run.id == run_id)
        .unwrap_or_else(|| panic!("the store must hold run {run_id:?}"));
    let due = run
        .settles_at
        .unwrap_or_else(|| panic!("a run in flight owes a transition, got {run:?}"));
    let span = due
        .duration_since(before)
        .unwrap_or_else(|error| panic!("the due instant must be after the start, got {error}"));
    assert!(
        span >= lower && span <= upper + Duration::from_millis(100),
        "case {:?}: the stored due instant must be {lower:?}..={upper:?} out, got {span:?}",
        "the stored due instant"
    );

    // Read straight after the start: the run is still in flight, so the status the start
    // recorded is observable.
    let reported = get_run(&router, &run_id).await;
    assert_eq!(
        reported.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_STARTING),
        "case {:?}: a run read straight after the start is still in flight, got {reported}",
        "read straight after the start"
    );

    // The clock owes the transition, not the reader: left alone for longer than the span, the
    // run is terminal on the first read afterwards.
    tokio::time::sleep(DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER + Duration::from_millis(150))
        .await;
    let settled = get_run(&router, &run_id).await;
    assert_eq!(
        settled.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
        "case {:?}: the wall clock alone makes the owed transition due, got {settled}",
        "slept past the span"
    );
}

// ---------------------------------------------------------------------------
// Reading a run nothing minted
// ---------------------------------------------------------------------------

/// A read of a run nothing minted is the operation's modelled miss, naming the id the
/// request sent; a value outside a modelled constraint is refused at the boundary.
#[tokio::test]
async fn a_read_of_a_run_nothing_minted_is_refused() {
    let (router, _engine) = emulator_with_engine();
    store_table(&router, "read_db", "read_table").await;
    store_ruleset(&router, "read-ruleset").await;
    let started = start_run(
        &router,
        &start_body(
            &glue_table_source("read_db", "read_table", json!({})),
            &["read-ruleset"],
            json!({}),
        ),
    )
    .await;

    // (case, the run id the read addresses)
    let changed = {
        let mut other = started.clone();
        let first = if started.starts_with('a') { "b" } else { "a" };
        other.replace_range(0..1, first);
        other
    };
    let misses: Vec<(&str, &str)> = vec![
        ("an id nothing minted", "ffffffffffffffffffffffffffffffff"),
        ("an id of another shape", "not-a-run-id"),
        (
            "the started run's id with one character changed",
            changed.as_str(),
        ),
    ];
    for (case, run_id) in misses {
        let (status, parsed) = call(
            &router,
            "GetDataQualityRulesetEvaluationRun",
            &json!({ "RunId": run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a read of a run nothing minted is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            format!("Data quality ruleset evaluation run with id {run_id} does not exist"),
            "case {case:?}: expected the run miss naming the requested id, got {:?}",
            error_message(&parsed)
        );
    }

    // One past the published lower bound of `Timeout` (`range 1..`) is refused before the
    // handler runs.
    let (status, parsed) = call(
        &router,
        "StartDataQualityRulesetEvaluationRun",
        &start_body(
            &glue_table_source("read_db", "read_table", json!({})),
            &["read-ruleset"],
            json!({ "Timeout": 0 }),
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a timeout below the modelled range is refused, got {status} with {parsed}",
        "one past the lower timeout bound"
    );
    assert_ne!(
        error_code(&parsed),
        "NotImplemented",
        "case {:?}: the refusal is a bad-request refusal, not a scaffold answer, got {parsed}",
        "one past the lower timeout bound"
    );
}
