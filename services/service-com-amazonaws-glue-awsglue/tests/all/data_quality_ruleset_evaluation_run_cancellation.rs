// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelDataQualityRulesetEvaluationRun`: the operation that stops a
//! run `StartDataQualityRulesetEvaluationRun` recorded.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a cancel of a data quality run addressed by an id of the grammar this
//! service mints run ids in moves a run still in flight into `STOPPING`, with `STOPPED` owed
//! by the clock one span later and nothing evaluated further — so the stopped run publishes no
//! data quality result — and leaves a run that has already reached a terminal status exactly
//! as it stands; an id OF that grammar this account holds no run under is the operation's
//! modelled `EntityNotFoundException`, and an id outside the grammar is refused up front as
//! bad input. The same rule the rule recommendation run's cancel follows
//! (`crate::data_quality_rule_recommendation_runs`), applied to the other kind of data
//! quality run.
//!
//! The rows differ along how many rulesets the cancelled run evaluates, how many data sources
//! it reads, which optional members its start named, the status the run is in when the cancel
//! arrives, how many times it is cancelled, and the id the cancel addresses.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather than
//! slept for, so what a row observes never depends on how fast the host runs.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DATA_QUALITY_RUN_ID_PREFIX, DATA_QUALITY_RUN_STATE_FAILED,
    DATA_QUALITY_RUN_STATE_STARTING, DATA_QUALITY_RUN_STATE_STOPPED,
    DATA_QUALITY_RUN_STATE_STOPPING, DATA_QUALITY_RUN_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a start names: "An IAM role supplied to encrypt the results of the run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// The bucket every table below points its data at.
const DATA_BUCKET: &str = "dq-cancel-data";

/// How wide the token of a minted data quality run id is: the 32 lowercase hexadecimal
/// characters this store mints every data quality run id with.
const MINTED_TOKEN_WIDTH: usize = 32;

/// An emulator plus the engine it runs on, so a test can move the due instants the run
/// lifecycle turns on instead of waiting for them.
///
/// Amazon S3 is mounted on the same engine because an evaluation run READS the objects behind
/// its data source's table, over the emulator's own in-process callback.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(engine.clone());
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Store `database` holding `table`, with one object staged under the prefix the table points
/// at: the catalog state the starts below are resolved against.
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

/// Store a data quality ruleset under `name`, so a start may evaluate it.
async fn store_ruleset(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDataQualityRuleset",
        &json!({ "Name": name, "Ruleset": "Rules = [\n    ColumnExists \"id\"\n]" }),
    )
    .await;
}

/// A `DataSource` addressing `database`.`table` through its plain `GlueTable` member.
fn glue_table_source(database: &str, table: &str) -> Value {
    json!({ "GlueTable": { "DatabaseName": database, "TableName": table } })
}

/// Start an evaluation run of `ruleset_names` over `data_source`, plus whatever `extra` adds,
/// and answer the `RunId` it reports.
async fn start_run(
    router: &MultiServiceRouter,
    data_source: &Value,
    ruleset_names: &[&str],
    extra: Value,
) -> String {
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
    let started = ok(router, "StartDataQualityRulesetEvaluationRun", &body).await;
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

/// Cancel one run, which must be served.
async fn cancel_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    ok(
        router,
        "CancelDataQualityRulesetEvaluationRun",
        &json!({ "RunId": run_id }),
    )
    .await
}

/// The `Status` a reported run carries.
fn reported_status(run: &Value) -> &str {
    run.get("Status").and_then(Value::as_str).unwrap_or_default()
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

/// The data quality results this account holds for the table `database`.`table`, as
/// `ListDataQualityResults` reports them.
async fn results_of_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
) -> Vec<String> {
    let listed = ok(
        router,
        "ListDataQualityResults",
        &json!({ "Filter": { "DataSource": glue_table_source(database, table) } }),
    )
    .await;
    listed["Results"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|result| {
            result["ResultId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// A run still in flight
// ---------------------------------------------------------------------------

/// One row of the in-flight table below: a run of its own shape, cancelled while the clock
/// still owes it its terminal status.
struct InFlightCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The rulesets the start lists.
    rulesets: &'a [&'a str],
    /// The start members beyond the three `@required` ones.
    extra: Value,
    /// The tables its `AdditionalDataSources` address, by reference string.
    additional: &'a [(&'a str, &'a str)],
}

/// A cancel of a run still in flight moves it into `STOPPING`, and the clock owes it
/// `STOPPED` — whatever the run evaluates and however its start was configured. The stopped
/// run completed nothing, so it reports no `ResultIds` and publishes no data quality result.
#[tokio::test]
async fn a_cancel_stops_a_run_still_in_flight() {
    let (router, engine) = emulator_with_engine();

    let cases = [
        InFlightCase {
            case: "one ruleset over one data source",
            database: "cancel_db",
            table: "single",
            rulesets: &["cancel-ruleset-a"],
            extra: json!({}),
            additional: &[],
        },
        InFlightCase {
            case: "two rulesets over one data source",
            database: "cancel_db",
            table: "two_rulesets",
            rulesets: &["cancel-ruleset-a", "cancel-ruleset-b"],
            extra: json!({}),
            additional: &[],
        },
        InFlightCase {
            case: "one ruleset over an additional data source too",
            database: "cancel_db",
            table: "with_additional",
            rulesets: &["cancel-ruleset-a"],
            extra: json!({}),
            additional: &[("extra", "additional_source")],
        },
        InFlightCase {
            case: "a start naming the optional configuration",
            database: "cancel_db",
            table: "configured",
            rulesets: &["cancel-ruleset-a"],
            extra: json!({
                "NumberOfWorkers": 9,
                "Timeout": 17,
                "ClientToken": "11111111-2222-3333-4444-555555555555",
            }),
            additional: &[],
        },
    ];

    for ruleset in ["cancel-ruleset-a", "cancel-ruleset-b"] {
        store_ruleset(&router, ruleset).await;
    }

    for case in &cases {
        store_table(&router, case.database, case.table).await;
        let mut extra = case.extra.clone();
        if !case.additional.is_empty() {
            let mut sources = serde_json::Map::new();
            for (reference, table) in case.additional {
                store_table(&router, case.database, table).await;
                sources.insert(
                    (*reference).to_string(),
                    glue_table_source(case.database, table),
                );
            }
            extra["AdditionalDataSources"] = Value::Object(sources);
        }
        let run_id = start_run(
            &router,
            &glue_table_source(case.database, case.table),
            case.rulesets,
            extra,
        )
        .await;

        // The run is held in flight, so what the cancel moves is a run the clock has not
        // finished.
        set_due_instant(&engine, far_future());
        let in_flight = get_run(&router, &run_id).await;
        assert_eq!(
            reported_status(&in_flight),
            DATA_QUALITY_RUN_STATE_STARTING,
            "case {}: the run must still be in flight before the cancel, got {in_flight}",
            case.case
        );

        let answered = cancel_run(&router, &run_id).await;
        assert_eq!(
            answered,
            json!({}),
            "case {}: the cancel's response carries no member, got {answered}",
            case.case
        );

        let stopping = get_run(&router, &run_id).await;
        assert_eq!(
            reported_status(&stopping),
            DATA_QUALITY_RUN_STATE_STOPPING,
            "case {}: a cancelled run is draining, got {stopping}",
            case.case
        );
        assert!(
            stopping.get("CompletedOn").is_none(),
            "case {}: a draining run has not completed, got {stopping}",
            case.case
        );
        assert!(
            stopping.get("ResultIds").is_none(),
            "case {}: a draining run published no result, got {stopping}",
            case.case
        );

        // The stop the clock owes it: once due, the run is STOPPED, and it stays stopped.
        set_due_instant(&engine, already_past());
        for read in 1..=2 {
            let stopped = get_run(&router, &run_id).await;
            assert_eq!(
                reported_status(&stopped),
                DATA_QUALITY_RUN_STATE_STOPPED,
                "case {}: read {read} of a cancelled run whose stop is due must report it \
                 stopped, got {stopped}",
                case.case
            );
            assert!(
                stopped.get("ResultIds").is_none(),
                "case {}: read {read} of a stopped run reports no result id, got {stopped}",
                case.case
            );
            assert!(
                stopped.get("CompletedOn").is_some(),
                "case {}: read {read} of a stopped run reports when it completed, got {stopped}",
                case.case
            );
        }
        assert_eq!(
            results_of_table(&router, case.database, case.table).await,
            Vec::<String>::new(),
            "case {}: a run that was stopped publishes no data quality result",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// A run that has already finished
// ---------------------------------------------------------------------------

/// A cancel of a run that has already reached a terminal status is served and changes
/// nothing: the run keeps the status it finished in, and everything it reported before the
/// cancel it reports after it.
///
/// The rows are the terminal status the run reached: one whose tables the catalog still holds,
/// which succeeded, and one whose table the catalog lost, which failed.
#[tokio::test]
async fn a_cancel_of_a_finished_run_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    store_ruleset(&router, "finished-ruleset").await;

    for (case, table, expected_status, lose_the_table) in [
        (
            "a run that succeeded",
            "already_succeeded",
            DATA_QUALITY_RUN_STATE_SUCCEEDED,
            false,
        ),
        (
            "a run that failed",
            "already_failed",
            DATA_QUALITY_RUN_STATE_FAILED,
            true,
        ),
    ] {
        store_table(&router, "finished_db", table).await;
        let run_id = start_run(
            &router,
            &glue_table_source("finished_db", table),
            &["finished-ruleset"],
            json!({}),
        )
        .await;
        if lose_the_table {
            ok(
                &router,
                "DeleteTable",
                &json!({ "DatabaseName": "finished_db", "Name": table }),
            )
            .await;
        }

        set_due_instant(&engine, already_past());
        let before = get_run(&router, &run_id).await;
        assert_eq!(
            reported_status(&before),
            expected_status,
            "case {case}: the run must have finished before the cancel, got {before}"
        );

        cancel_run(&router, &run_id).await;
        let after = get_run(&router, &run_id).await;
        assert_eq!(
            after, before,
            "case {case}: a cancel of a finished run changes nothing, got {after} after {before}"
        );
    }
}

// ---------------------------------------------------------------------------
// Repeating the cancel
// ---------------------------------------------------------------------------

/// Cancelling the same run again is served and keeps it on the one path a cancel puts it on:
/// `STOPPING` while it drains, `STOPPED` once the clock's stop is due, and still `STOPPED`
/// when a cancel arrives after that.
#[tokio::test]
async fn repeating_the_cancel_keeps_the_run_on_the_one_stop_path() {
    let (router, engine) = emulator_with_engine();
    store_ruleset(&router, "repeat-ruleset").await;
    store_table(&router, "repeat_db", "twice").await;
    let run_id = start_run(
        &router,
        &glue_table_source("repeat_db", "twice"),
        &["repeat-ruleset"],
        json!({}),
    )
    .await;

    set_due_instant(&engine, far_future());
    for cancels in 1..=3 {
        cancel_run(&router, &run_id).await;
        set_due_instant(&engine, far_future());
        let draining = get_run(&router, &run_id).await;
        assert_eq!(
            reported_status(&draining),
            DATA_QUALITY_RUN_STATE_STOPPING,
            "case cancel {cancels}: a repeated cancel leaves the run draining, got {draining}"
        );
    }

    set_due_instant(&engine, already_past());
    let stopped = get_run(&router, &run_id).await;
    assert_eq!(
        reported_status(&stopped),
        DATA_QUALITY_RUN_STATE_STOPPED,
        "case the stop falls due: the clock owes a cancelled run its stop, got {stopped}"
    );

    cancel_run(&router, &run_id).await;
    let after = get_run(&router, &run_id).await;
    assert_eq!(
        after, stopped,
        "case a cancel after the stop: nothing changes, got {after} after {stopped}"
    );
}

// ---------------------------------------------------------------------------
// The cancel addresses one run
// ---------------------------------------------------------------------------

/// A cancel moves ONLY the run its `RunId` names: a sibling run started over another table
/// stays in flight and goes on to succeed, publishing the results its own evaluation produced.
#[tokio::test]
async fn a_cancel_moves_only_the_run_it_names() {
    let (router, engine) = emulator_with_engine();
    store_ruleset(&router, "sibling-ruleset").await;
    store_table(&router, "sibling_db", "cancelled").await;
    store_table(&router, "sibling_db", "untouched").await;

    let cancelled = start_run(
        &router,
        &glue_table_source("sibling_db", "cancelled"),
        &["sibling-ruleset"],
        json!({}),
    )
    .await;
    let untouched = start_run(
        &router,
        &glue_table_source("sibling_db", "untouched"),
        &["sibling-ruleset"],
        json!({}),
    )
    .await;

    set_due_instant(&engine, far_future());
    cancel_run(&router, &cancelled).await;
    set_due_instant(&engine, far_future());

    let sibling = get_run(&router, &untouched).await;
    assert_eq!(
        reported_status(&sibling),
        DATA_QUALITY_RUN_STATE_STARTING,
        "case the sibling run: a cancel of another run leaves it in flight, got {sibling}"
    );

    set_due_instant(&engine, already_past());
    let stopped = get_run(&router, &cancelled).await;
    assert_eq!(
        reported_status(&stopped),
        DATA_QUALITY_RUN_STATE_STOPPED,
        "case the cancelled run: it stops, got {stopped}"
    );
    let succeeded = get_run(&router, &untouched).await;
    assert_eq!(
        reported_status(&succeeded),
        DATA_QUALITY_RUN_STATE_SUCCEEDED,
        "case the sibling run: it finishes its own evaluation, got {succeeded}"
    );
    assert_eq!(
        succeeded["ResultIds"].as_array().map(Vec::len),
        Some(1),
        "case the sibling run: it publishes one result per ruleset it evaluated, got {succeeded}"
    );
    assert_eq!(
        results_of_table(&router, "sibling_db", "cancelled").await,
        Vec::<String>::new(),
        "case the cancelled run: it publishes no data quality result"
    );
    assert_eq!(
        results_of_table(&router, "sibling_db", "untouched").await.len(),
        1,
        "case the sibling run: its result is listed"
    );
}

// ---------------------------------------------------------------------------
// The id the cancel addresses
// ---------------------------------------------------------------------------

/// A cancel of a run nothing minted, under an id OF the grammar a minted one carries, is the
/// operation's modelled `EntityNotFoundException`, naming the id it was handed; an id OUTSIDE
/// that grammar is refused up front as bad input, stating the grammar.
///
/// The rows are how long the well-formed token is, and how a malformed id departs from the
/// grammar.
#[tokio::test]
async fn the_cancel_refuses_an_id_no_run_answers_to() {
    let (router, _engine) = emulator_with_engine();

    for width in [1, MINTED_TOKEN_WIDTH, 64] {
        let run_id = format!("{DATA_QUALITY_RUN_ID_PREFIX}{}", "f".repeat(width));
        let (status, body) = call(
            &router,
            "CancelDataQualityRulesetEvaluationRun",
            &json!({ "RunId": &run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token of {width}: a cancel of a run nothing minted is refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case token of {width}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Cannot find Data Quality Run in account {ACCOUNT_ID} with id {run_id}"),
            "case token of {width}: the miss must name the requested id, got {:?}",
            error_message(&body)
        );
    }

    for (case, run_id) in [
        ("an id of no recognisable form", "not-a-run-id".to_string()),
        (
            "the prefix in the wrong case",
            "DQRUN-0123456789abcdef".to_string(),
        ),
        (
            "a bare token with no prefix",
            "f".repeat(MINTED_TOKEN_WIDTH),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CancelDataQualityRulesetEvaluationRun",
            &json!({ "RunId": &run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an id outside the grammar is refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Invalid RunId: must start with '{DATA_QUALITY_RUN_ID_PREFIX}'"),
            "case {case}: the refusal must state the grammar, got {:?}",
            error_message(&body)
        );
    }
}
