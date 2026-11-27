// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetDataQualityRulesetEvaluationRun`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the batch read answers 200 and reports, for EACH id in `RunIds` and
//! in the order the request listed them, either the evaluation run stored under that id —
//! the same projection `GetDataQualityRulesetEvaluationRun` reports — or that id in
//! `RunsNotFound`; an id no run is stored under is an absence the answer reports and not a
//! failure of the whole batch, and the call changes nothing a later caller sees beyond the
//! status the CLOCK had already made due.
//!
//! Every id below therefore carries the grammar a data quality run id has
//! ([`DATA_QUALITY_RUN_ID_PREFIX`]): an id outside it addresses no run that could exist, so
//! it is refused rather than reported as an absence, which
//! `tests/all/data_quality_run_id_grammar.rs` covers.
//!
//! The recorded case — two started runs read back beside one id nothing minted — is ONE row
//! of the tables below; the rows differ along how many ids are asked for, how many of them
//! are stored, where the misses sit in the order, whether an id repeats, which `DataSource`
//! member carries the run's table, how many rulesets the run evaluates, which optional
//! members its start named, and whether the run is still in flight or has reached its
//! terminal status.
//!
//! The due instants the run lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, so the transient window is deterministic.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DATA_QUALITY_RUN_ID_PREFIX, DATA_QUALITY_RUN_STATE_STARTING,
    TERMINAL_DATA_QUALITY_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, ok, string_list, ACCOUNT_ID};

/// The IAM role every start below names: "An IAM role supplied to encrypt the results of the
/// run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// The largest `RunIds` list the model accepts, and the smallest.
///
/// Source: `smithy_query(query="shape",shape="DataQualityRulesetEvaluationRunIdList")`, whose
/// `length` is `1..100`; the same bound is on the response's `RunsNotFound`.
const MIN_RUN_IDS: usize = 1;
const MAX_RUN_IDS: usize = 100;

/// The published bounds of one element of that list: `HashString`, `length 1..255`.
const MIN_RUN_ID_LENGTH: usize = 1;
const MAX_RUN_ID_LENGTH: usize = 255;

/// The smallest `Timeout` the model accepts: `Timeout`, `range 1..` minutes.
const MIN_TIMEOUT_MINUTES: i64 = 1;

/// The bucket every table below points its data at.
const DATA_BUCKET: &str = "dq-batch-read-data";

/// The databases and tables the runs below evaluate.
const DATABASE: &str = "dq_batch_db";
const TABLE: &str = "events";
const OTHER_TABLE: &str = "orders";

/// The rulesets the runs below evaluate.
const RULESET: &str = "batch-events-ruleset";
const OTHER_RULESET: &str = "batch-orders-ruleset";

/// An emulator plus the engine it runs on, so a test can move the due instants the run
/// lifecycle turns on instead of waiting for them.
///
/// Amazon S3 is mounted on the same engine because an evaluation run READS the objects behind
/// its data source's table, through the emulator's own in-process callback.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(engine.clone());
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Store `database` holding `table`, whose data is staged in this emulator's own Amazon S3.
/// A database several tables share is created once — a repeat create is the catalog's
/// `AlreadyExistsException`, which is not what these tests are about.
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
    stage_table_data(router, database, table).await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://{DATA_BUCKET}/{database}/{table}/"),
                    "Columns": [{ "Name": "id", "Type": "string" }],
                },
            },
        }),
    )
    .await;
}

/// Put one object under the prefix the table's storage descriptor names, creating the bucket
/// the first time: one row carrying the one column those tables declare.
async fn stage_table_data(router: &MultiServiceRouter, database: &str, table: &str) {
    let client = crate::wire::s3_client(router);
    // The bucket is shared by every table, so the create of an existing one is no failure.
    let _ = client.create_bucket().bucket(DATA_BUCKET).send().await;
    client
        .put_object()
        .bucket(DATA_BUCKET)
        .key(format!("{database}/{table}/rows.csv"))
        .body(aws_sdk_s3::primitives::ByteStream::from(
            b"row-1\n".to_vec(),
        ))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("the data of {database}.{table} must be staged, got {error:?}")
        });
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

/// A `DataSource` addressing `database`.`table` through the named member: either the plain
/// `GlueTable` ("An Glue table") or the `DataQualityGlueTable` ("An Glue table for Data
/// Quality Operations"), which address a table the same way.
fn source_through(member: &str, database: &str, table: &str, catalog_id: Option<&str>) -> Value {
    let mut glue_table = json!({ "DatabaseName": database, "TableName": table });
    if let Some(catalog) = catalog_id {
        glue_table["CatalogId"] = json!(catalog);
    }
    json!({ member: glue_table })
}

/// Start an evaluation run of `rulesets` over `data_source`, plus whatever `extra` adds to
/// the three `@required` members, and answer the `RunId` it reports.
async fn start_run(
    router: &MultiServiceRouter,
    data_source: &Value,
    rulesets: &[&str],
    extra: &Value,
) -> String {
    let mut body = json!({
        "DataSource": data_source,
        "Role": ROLE,
        "RulesetNames": rulesets,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let started = ok(router, "StartDataQualityRulesetEvaluationRun", &body).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("StartDataQualityRulesetEvaluationRun must answer a RunId, got {started}")
        })
        .to_string()
}

/// One `BatchGetDataQualityRulesetEvaluationRun` call that must succeed.
async fn batch_get(router: &MultiServiceRouter, run_ids: &[String]) -> Value {
    ok(
        router,
        "BatchGetDataQualityRulesetEvaluationRun",
        &json!({ "RunIds": run_ids }),
    )
    .await
}

/// An address of a run nothing minted: OF the grammar every data quality run id carries
/// ([`DATA_QUALITY_RUN_ID_PREFIX`]), so it reaches the lookup and is reported as the absence
/// it is. An id OUTSIDE that grammar addresses no run at all and refuses the whole batch,
/// which `tests/all/data_quality_run_id_grammar.rs` covers.
fn unknown_id(token: &str) -> String {
    format!("{DATA_QUALITY_RUN_ID_PREFIX}{token}")
}

/// The runs one answer reports, in the order it reports them.
fn batched_runs(body: &Value) -> Vec<Value> {
    body.get("Runs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the batch must report a Runs list, got {body}"))
        .clone()
}

/// The `RunId` of every run one answer reports, in the order it reports them.
fn batched_run_ids(body: &Value) -> Vec<String> {
    batched_runs(body)
        .iter()
        .map(|run| {
            run.get("RunId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every batched run carries a RunId, got {run}"))
                .to_string()
        })
        .collect()
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

/// The status a reported run carries.
fn reported_status(run: &Value) -> String {
    run.get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every reported run carries a Status, got {run}"))
        .to_string()
}

/// The catalog state and the three runs every test below reads: one per row of the start
/// table, so the batch spans the configurations a run can carry.
///
/// Answers the ids in the order they were started.
async fn started_runs(router: &MultiServiceRouter) -> Vec<String> {
    store_table(router, DATABASE, TABLE).await;
    store_table(router, DATABASE, OTHER_TABLE).await;
    store_ruleset(router, RULESET).await;
    store_ruleset(router, OTHER_RULESET).await;

    // (case, the DataSource the start carries, the rulesets it lists, its other members).
    let starts: [(&str, Value, &[&str], Value); 3] = [
        (
            "a GlueTable, one ruleset and nothing optional",
            source_through("GlueTable", DATABASE, TABLE, None),
            &[RULESET],
            json!({}),
        ),
        (
            "a DataQualityGlueTable, a catalog id and the smallest timeout the model admits",
            source_through("DataQualityGlueTable", DATABASE, OTHER_TABLE, Some(ACCOUNT_ID)),
            &[OTHER_RULESET],
            json!({ "NumberOfWorkers": 2, "Timeout": MIN_TIMEOUT_MINUTES }),
        ),
        (
            "several rulesets, an additional data source and run options",
            source_through("GlueTable", DATABASE, TABLE, None),
            &[RULESET, OTHER_RULESET],
            json!({
                "NumberOfWorkers": 7,
                "Timeout": 2880,
                "AdditionalDataSources": {
                    "lookup": source_through("GlueTable", DATABASE, OTHER_TABLE, None),
                },
                "AdditionalRunOptions": {
                    "CloudWatchMetricsEnabled": true,
                    "ResultsS3Prefix": format!("s3://{DATA_BUCKET}/results/"),
                },
            }),
        ),
    ];

    let mut ids = Vec::new();
    for (case, data_source, rulesets, extra) in &starts {
        let id = start_run(router, data_source, rulesets, extra).await;
        assert!(
            !id.is_empty(),
            "case {case:?}: the start must report an id the batch can address"
        );
        ids.push(id);
    }
    ids
}

// ---------------------------------------------------------------------------
// Every requested id lands in exactly one of the two lists
// ---------------------------------------------------------------------------

/// One row of the batch table: the case name, the ids the request asks for, the ids the
/// `Runs` list must carry, and the ids `RunsNotFound` must carry.
type BatchCase<'a> = (&'a str, Vec<String>, Vec<String>, Vec<String>);

/// Each requested id is reported exactly once — as a run or as a miss — in the order asked,
/// and the answer is repeatable.
///
/// The rows differ along the batch's size, how many of its ids are stored, where the misses
/// sit (first, last, interleaved), whether an id repeats, and whether the batch stands at the
/// modelled ceiling. The recorded reference case — two started runs beside one id nothing
/// minted — is the "two stored ids beside a miss" row.
#[tokio::test]
async fn every_requested_run_id_is_reported_exactly_once() {
    let (router, engine) = emulator_with_engine();
    let started = started_runs(&router).await;
    // The runs stay in flight for the whole test, so two reads of one batch answer the same
    // thing without any wait on the wall clock.
    set_due_instant(&engine, far_future());

    let stored: Vec<String> = started.clone();
    let absent: Vec<String> = (0..3)
        .map(|index| unknown_id(&format!("no-such-run-{index}")))
        .collect();
    // A batch standing exactly at the modelled ceiling: every stored run plus enough ids
    // nothing minted to fill it.
    let ceiling: Vec<String> = stored
        .iter()
        .cloned()
        .chain(
            (0..MAX_RUN_IDS - stored.len()).map(|index| unknown_id(&format!("filler-{index:03}"))),
        )
        .collect();

    let cases: Vec<BatchCase> = vec![
        (
            "one stored id, the smallest batch the model admits",
            vec![stored[0].clone()],
            vec![stored[0].clone()],
            vec![],
        ),
        (
            // The recorded case.
            "two stored ids beside a miss",
            vec![stored[0].clone(), stored[1].clone(), absent[0].clone()],
            vec![stored[0].clone(), stored[1].clone()],
            vec![absent[0].clone()],
        ),
        (
            "every id stored",
            stored.clone(),
            stored.clone(),
            vec![],
        ),
        (
            "no id stored",
            absent.clone(),
            vec![],
            absent.clone(),
        ),
        (
            "the miss comes first",
            vec![absent[0].clone(), stored[2].clone()],
            vec![stored[2].clone()],
            vec![absent[0].clone()],
        ),
        (
            "the miss comes last",
            vec![stored[2].clone(), absent[1].clone()],
            vec![stored[2].clone()],
            vec![absent[1].clone()],
        ),
        (
            "misses interleaved with hits",
            vec![
                absent[0].clone(),
                stored[0].clone(),
                absent[1].clone(),
                stored[2].clone(),
                absent[2].clone(),
            ],
            vec![stored[0].clone(), stored[2].clone()],
            vec![absent[0].clone(), absent[1].clone(), absent[2].clone()],
        ),
        (
            // One answer per requested id, not per distinct id.
            "a repeated stored id",
            vec![stored[1].clone(), stored[1].clone()],
            vec![stored[1].clone(), stored[1].clone()],
            vec![],
        ),
        (
            "a repeated unknown id",
            vec![absent[2].clone(), absent[2].clone()],
            vec![],
            vec![absent[2].clone(), absent[2].clone()],
        ),
        (
            "a batch at the modelled ceiling",
            ceiling.clone(),
            stored.clone(),
            ceiling[stored.len()..].to_vec(),
        ),
    ];

    for (case, requested, expected_runs, expected_missing) in &cases {
        let body = batch_get(&router, requested).await;

        let reported = batched_run_ids(&body);
        assert_eq!(
            &reported, expected_runs,
            "case {case:?}: expected Runs {expected_runs:?}, got {reported:?} in {body}"
        );
        let missing = string_list(&body, "RunsNotFound");
        assert_eq!(
            &missing, expected_missing,
            "case {case:?}: expected RunsNotFound {expected_missing:?}, got {missing:?} in {body}"
        );
        // The two lists together account for every requested id, once each.
        assert_eq!(
            reported.len() + missing.len(),
            requested.len(),
            "case {case:?}: every requested id must be reported exactly once, got {body}"
        );

        // A read does not change what the next caller sees: the same batch answers the same
        // thing, and every stored run is still readable through the single-run read.
        let again = batch_get(&router, requested).await;
        assert_eq!(
            again, body,
            "case {case:?}: the batch read must be repeatable, got {again}"
        );
        for id in &stored {
            let single = get_run(&router, id).await;
            assert_eq!(
                single.get("RunId").and_then(Value::as_str),
                Some(id.as_str()),
                "case {case:?}: the batch read must not have touched {id:?}, got {single}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the batch reports is what the single-run read reports
// ---------------------------------------------------------------------------

/// A run the batch reports is member for member the run
/// `GetDataQualityRulesetEvaluationRun` reports for the same id — the batch is a different
/// addressing mode over one stored run, not a second projection of it.
///
/// The rows differ along which `DataSource` member carries the table, how many rulesets the
/// run evaluates and which optional members its start named, and each is read twice: while
/// the run is still in flight, and after the status the clock owed it has fallen due.
#[tokio::test]
async fn a_batched_run_is_the_run_the_single_read_reports() {
    let (router, engine) = emulator_with_engine();
    let started = started_runs(&router).await;

    // While every run is still in flight: a non-terminal status, and no result to report.
    set_due_instant(&engine, far_future());
    let body = batch_get(&router, &started).await;
    for (index, run) in batched_runs(&body).iter().enumerate() {
        let case = format!("run {index} in flight");
        let single = get_run(&router, &started[index]).await;
        assert_eq!(
            run, &single,
            "case {case:?}: expected the single read's run {single}, got {run}"
        );
        assert_eq!(
            reported_status(run),
            DATA_QUALITY_RUN_STATE_STARTING,
            "case {case:?}: a run still in flight carries a non-terminal status, got {run}"
        );
        assert!(
            run.get("ResultIds").is_none(),
            "case {case:?}: a run still in flight has minted no result, got {run}"
        );
    }

    // Once the clock owes the terminal status, the batch reports it — and still reports
    // exactly what the single-run read reports.
    set_due_instant(&engine, already_past());
    let settled = batch_get(&router, &started).await;
    for (index, run) in batched_runs(&settled).iter().enumerate() {
        let case = format!("run {index} settled");
        let single = get_run(&router, &started[index]).await;
        assert_eq!(
            run, &single,
            "case {case:?}: expected the single read's run {single}, got {run}"
        );
        let status = reported_status(run);
        assert!(
            TERMINAL_DATA_QUALITY_RUN_STATES.contains(&status.as_str()),
            "case {case:?}: expected a terminal status, got {status} in {run}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled bounds are refused, not reported as a miss
// ---------------------------------------------------------------------------

/// An id outside the modelled `length 1..255`, a batch outside the modelled
/// `length 1..100`, and an absent `@required` member are refused at the request boundary
/// rather than reported as misses — none of them addresses a run, so each is a bad request.
///
/// Source for both bounds:
/// `smithy_query(query="shape",shape="DataQualityRulesetEvaluationRunIdList")` — the list is
/// `length 1..100` and its member targets `HashString`, `length 1..255`. Each published bound
/// is exercised, plus one past it; at the element MINIMUM no value can carry the grammar a run
/// id has, so that bound is a malformed address rather than a miss.
#[tokio::test]
async fn run_ids_outside_the_modeled_bounds_are_refused() {
    let (router, engine) = emulator_with_engine();
    let started = started_runs(&router).await;
    set_due_instant(&engine, far_future());

    // At the published element CEILING an id of the grammar is a legal address, so it is
    // simply unknown — reported as a miss rather than refused.
    for length in [DATA_QUALITY_RUN_ID_PREFIX.len() + 1, MAX_RUN_ID_LENGTH] {
        let id = unknown_id(&"r".repeat(length - DATA_QUALITY_RUN_ID_PREFIX.len()));
        let body = batch_get(&router, std::slice::from_ref(&id)).await;
        assert_eq!(
            string_list(&body, "RunsNotFound"),
            vec![id.clone()],
            "case id of {length} characters: a legal but unknown id is a miss, got {body}"
        );
        assert!(
            batched_runs(&body).is_empty(),
            "case id of {length} characters: no run is stored under it, got {body}"
        );
    }

    // At the published element MINIMUM no id can carry the grammar every run id has — the
    // prefix alone is longer — so that bound is a malformed address rather than a miss.
    let shortest = "r".repeat(MIN_RUN_ID_LENGTH);
    let (status, answer) = call(
        &router,
        "BatchGetDataQualityRulesetEvaluationRun",
        &json!({ "RunIds": [&shortest] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an id at the element minimum: no id that short carries the run grammar, got \
         {status} with {answer}"
    );
    assert!(
        error_code(&answer).ends_with("InvalidInputException"),
        "case an id at the element minimum: expected InvalidInputException, got {} in {answer}",
        error_code(&answer)
    );

    // A batch at each published list bound is served: the smallest the model admits, and the
    // largest.
    for size in [MIN_RUN_IDS, MAX_RUN_IDS] {
        let ids: Vec<String> = (0..size)
            .map(|index| unknown_id(&format!("bounded-{index:03}")))
            .collect();
        let body = batch_get(&router, &ids).await;
        assert_eq!(
            string_list(&body, "RunsNotFound"),
            ids,
            "case a batch of {size} unknown ids: all of them miss, got {body}"
        );
    }

    // One past each bound is refused, and so is a request naming no `RunIds` at all.
    let over_ceiling: Vec<String> = (0..=MAX_RUN_IDS)
        .map(|index| unknown_id(&format!("over-{index:03}")))
        .collect();
    let refusals: Vec<(&str, Value)> = vec![
        ("an empty batch, one below the list's minimum", json!({ "RunIds": [] })),
        ("one id too many", json!({ "RunIds": over_ceiling })),
        ("an empty id", json!({ "RunIds": [""] })),
        (
            "an id of 256 characters",
            json!({ "RunIds": ["r".repeat(MAX_RUN_ID_LENGTH + 1)] }),
        ),
        (
            "a stored id beside an over-long one",
            json!({ "RunIds": [started[0].clone(), "r".repeat(MAX_RUN_ID_LENGTH + 1)] }),
        ),
        ("no RunIds member at all", json!({})),
    ];

    for (case, body) in &refusals {
        let (status, answer) = call(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            body,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            !error_code(&answer).is_empty(),
            "case {case:?}: the refusal must name an error, got {answer}"
        );
    }

    // The refusals above refuse the request and not the whole operation: the stored run is
    // still served.
    let body = batch_get(&router, &started[..1]).await;
    assert_eq!(
        batched_run_ids(&body),
        vec![started[0].clone()],
        "a well-formed batch must still be served after a refused one, got {body}"
    );
}
