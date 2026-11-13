// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue data quality RESULT surface: what
//! `StartDataQualityRulesetEvaluationRun` evaluates against a table's data, and what
//! `GetDataQualityResult` reports about it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. Amazon S3 is mounted on
//! the same engine, because the rules are evaluated against the objects behind the data
//! source's table: the object a test stages is the data the run reads.
//!
//! The rule under test: an evaluation run checks every rule of every ruleset it was started
//! for against the rows of its data source, mints one result per ruleset when it reaches its
//! terminal status, and that result reports one `RuleResults` entry per rule of the ruleset —
//! `PASS` where the data satisfied it and `FAIL` where it did not — with `Score` the ratio of
//! rules that passed to the rules evaluated. A result is addressed by the `ResultId` the run
//! reports, an id no run has published is the modelled `EntityNotFoundException`, and a run
//! whose data source cannot be read evaluates nothing: it fails and publishes no result at
//! all, rather than claiming a score nothing computed.
//!
//! The recorded case — a 3-row table and a 3-rule ruleset scoring 2/3 — is ONE row of the
//! table below; the rows differ along how many rules the ruleset declares, which rule types
//! they use (counts, comparisons, completeness and uniqueness fractions, value conditions
//! over text and over numbers, a negated one), how many of them the data satisfies and how
//! many rulesets one run evaluates.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DATA_QUALITY_RULE_RESULT_FAIL, DATA_QUALITY_RULE_RESULT_PASS,
    DATA_QUALITY_RUN_STATE_FAILED, DATA_QUALITY_RUN_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, ok};

/// The IAM role a start names: "An IAM role supplied to encrypt the results of the run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// The bucket every table of this suite points its data at.
const DATA_BUCKET: &str = "dq-result-data";

/// The database every table of this suite lives in.
const DATABASE: &str = "dq_result_db";

/// The rows every ruleset below is evaluated against: three rows of the three columns the
/// tables declare, one of which holds the layout's NULL marker — so completeness is a
/// fraction rather than 0 or 1, one value occurs twice and one occurs once.
const ROWS: &str = "1,ACTIVE,first\n2,ACTIVE,\\N\n3,INACTIVE,third\n";

/// How close two scores have to be to count as the same: the ratio is a double, so a third
/// is compared with a tolerance rather than bit for bit.
const SCORE_TOLERANCE: f64 = 1e-9;

/// An emulator with Glue AND Amazon S3 mounted on one shared engine, plus the engine itself
/// so a test can make the status the clock owes a run due instead of waiting for it.
fn emulator_with_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(engine.clone());
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a run reads
/// through, so what a test stages here is what the run evaluates. The shared harness owns
/// the one client configuration ([`crate::wire::s3_client`]).
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    crate::wire::s3_client(router)
}

/// Store the database, and a table whose data is `rows` — or, where `rows` is `None`, a table
/// whose location holds no object at all.
async fn store_table(router: &MultiServiceRouter, table: &str, rows: Option<&str>) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    if let Some(rows) = rows {
        let client = s3(router);
        // The bucket is shared by every table of this suite, so the create of an existing
        // one is no failure.
        let _ = client.create_bucket().bucket(DATA_BUCKET).send().await;
        client
            .put_object()
            .bucket(DATA_BUCKET)
            .key(format!("{table}/rows.csv"))
            .body(aws_sdk_s3::primitives::ByteStream::from(
                rows.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("the data of {table} must be staged, got {error:?}"));
    }
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://{DATA_BUCKET}/{table}/"),
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "status", "Type": "string" },
                        { "Name": "note", "Type": "string" },
                    ],
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                    "SerdeInfo": {
                        "SerializationLibrary": "org.apache.hadoop.hive.serde2.OpenCSVSerde",
                        "Parameters": { "separatorChar": "," },
                    },
                },
            },
        }),
    )
    .await;
}

/// Store a ruleset whose rule list is `rules`, each entry one rule.
async fn store_ruleset(router: &MultiServiceRouter, name: &str, rules: &[&str]) {
    store_ruleset_text(
        router,
        name,
        &format!("Rules = [\n    {}\n]", rules.join(",\n    ")),
    )
    .await;
}

/// Store a ruleset whose DQDL text is `ruleset`, exactly as written.
async fn store_ruleset_text(router: &MultiServiceRouter, name: &str, ruleset: &str) {
    ok(
        router,
        "CreateDataQualityRuleset",
        &json!({ "Name": name, "Ruleset": ruleset }),
    )
    .await;
}

/// Start an evaluation run of `rulesets` over `table` and answer the `RunId` it reports.
async fn start_run(router: &MultiServiceRouter, table: &str, rulesets: &[&str]) -> String {
    let started = ok(
        router,
        "StartDataQualityRulesetEvaluationRun",
        &json!({
            "DataSource": { "GlueTable": { "DatabaseName": DATABASE, "TableName": table } },
            "Role": ROLE,
            "RulesetNames": rulesets,
        }),
    )
    .await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the start must answer a RunId, got {started}"))
        .to_string()
}

/// Make the terminal status the clock owes every unsettled run due, so the run below reaches
/// it without this test waiting on the wall clock. The transition is owed by the clock, so
/// moving its due instant is the same thing as the instant arriving.
fn make_terminal_status_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for run in &mut store.data_quality_ruleset_evaluation_runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(past);
        }
    }
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

/// What `GetDataQualityResult` reports for one result.
async fn get_result(router: &MultiServiceRouter, result_id: &str) -> Value {
    ok(
        router,
        "GetDataQualityResult",
        &json!({ "ResultId": result_id }),
    )
    .await
}

/// The result ids a finished run reports, in the order it reports them.
fn result_ids(run: &Value) -> Vec<String> {
    run.get("ResultIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the evaluation table: a ruleset's rules and, for each of them, whether the
/// staged rows satisfy it.
struct RulesetCase {
    case: &'static str,
    table: &'static str,
    /// The rules of the ruleset, in the order it declares them.
    rules: &'static [&'static str],
    /// Whether each of those rules passes over [`ROWS`], in the same order.
    passes: &'static [bool],
}

/// Every rule of the ruleset is evaluated against the table's own rows, and the result
/// reports what each of them answered plus the ratio that passed.
///
/// The rules of each row name the metric they are of, so the expectations below are read off
/// the three staged rows: three rows of three columns, `note` holding the NULL marker in one
/// of them, `status` holding one value twice and another once, and `id` holding three
/// distinct numbers.
#[tokio::test]
async fn every_rule_of_a_ruleset_is_evaluated_against_the_tables_rows() {
    let (router, engine) = emulator_with_s3();

    let cases: Vec<RulesetCase> = vec![
        RulesetCase {
            case: "three rules, two of them satisfied",
            table: "three_rules",
            rules: &[
                "RowCount = 3",
                "RowCount = 999",
                "ColumnValues \"status\" in [ \"ACTIVE\", \"INACTIVE\" ]",
            ],
            passes: &[true, false, true],
        },
        RulesetCase {
            case: "one rule the schema satisfies",
            table: "one_rule_pass",
            rules: &["ColumnExists \"status\""],
            passes: &[true],
        },
        RulesetCase {
            case: "one rule the schema does not satisfy",
            table: "one_rule_fail",
            rules: &["ColumnExists \"absent_column\""],
            passes: &[false],
        },
        RulesetCase {
            case: "completeness and uniqueness fractions",
            table: "fractions",
            rules: &[
                "IsComplete \"id\"",
                "IsComplete \"note\"",
                "Uniqueness \"id\" = 1.0",
                "Uniqueness \"status\" < 0.5",
                "Completeness \"note\" between 0.5 and 0.7",
            ],
            passes: &[true, false, true, true, true],
        },
        RulesetCase {
            case: "counts under every comparison",
            table: "counts",
            rules: &[
                "RowCount > 2",
                "RowCount <= 3",
                "RowCount != 3",
                "RowCount between 2 and 4",
                "ColumnCount = 3",
                "DistinctValuesCount \"status\" = 2",
                "RowCount in [ 1, 2 ]",
            ],
            passes: &[true, true, false, true, true, true, false],
        },
        RulesetCase {
            case: "value conditions over text and over numbers",
            table: "values",
            rules: &[
                "ColumnValues \"status\" in [ \"ACTIVE\", \"INACTIVE\" ]",
                "ColumnValues \"status\" not in [ \"ACTIVE\" ]",
                "ColumnValues \"status\" = \"ACTIVE\"",
                "ColumnValues \"id\" between 0 and 10",
                "ColumnValues \"id\" <= 2",
            ],
            passes: &[true, false, false, true, false],
        },
        RulesetCase {
            case: "a rule list spread over several lines",
            table: "commented",
            rules: &["RowCount = 3", "IsUnique \"id\""],
            passes: &[true, true],
        },
    ];

    for RulesetCase {
        case,
        table,
        rules,
        passes,
    } in cases
    {
        store_table(&router, table, Some(ROWS)).await;
        let ruleset = format!("{table}-ruleset");
        store_ruleset(&router, &ruleset, rules).await;

        let run_id = start_run(&router, table, &[ruleset.as_str()]).await;
        make_terminal_status_due(&engine);
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
            "case {case:?}: a run that evaluated its ruleset over readable data succeeds, got \
             {run}"
        );
        let ids = result_ids(&run);
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: one ruleset was evaluated, so the run reports one result id, got \
             {run}"
        );

        let result = get_result(&router, &ids[0]).await;
        assert_eq!(
            result.get("ResultId").and_then(Value::as_str),
            Some(ids[0].as_str()),
            "case {case:?}: the result reports the id it was addressed by, got {result}"
        );
        assert_eq!(
            result
                .get("RulesetEvaluationRunId")
                .and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the result names the run it came out of, got {result}"
        );
        assert_eq!(
            result.get("RulesetName").and_then(Value::as_str),
            Some(ruleset.as_str()),
            "case {case:?}: the result names the ruleset its rules came from, got {result}"
        );
        assert_eq!(
            result
                .get("DataSource")
                .and_then(|source| source.get("GlueTable"))
                .and_then(|glue_table| glue_table.get("TableName"))
                .and_then(Value::as_str),
            Some(table),
            "case {case:?}: the result names the data source the rules were evaluated over, \
             got {result}"
        );
        assert!(
            result.get("StartedOn").is_some() && result.get("CompletedOn").is_some(),
            "case {case:?}: the result reports when the run started and completed, got {result}"
        );

        let reported = result
            .get("RuleResults")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the result reports RuleResults, got {result}"));
        assert_eq!(
            reported.len(),
            rules.len(),
            "case {case:?}: one entry per rule of the ruleset, got {reported:?}"
        );
        for (position, (entry, rule)) in reported.iter().zip(rules.iter()).enumerate() {
            let expected = if passes[position] {
                DATA_QUALITY_RULE_RESULT_PASS
            } else {
                DATA_QUALITY_RULE_RESULT_FAIL
            };
            assert_eq!(
                entry.get("Result").and_then(Value::as_str),
                Some(expected),
                "case {case:?}: rule {rule:?} must be {expected}, got {entry}"
            );
            assert_eq!(
                entry.get("Name").and_then(Value::as_str),
                Some(format!("Rule_{}", position + 1).as_str()),
                "case {case:?}: a rule is named by its place in the rule list, got {entry}"
            );
            assert_eq!(
                entry.get("Description").and_then(Value::as_str),
                Some(*rule),
                "case {case:?}: the entry describes the rule it is of, got {entry}"
            );
        }

        let passed = passes.iter().filter(|passed| **passed).count();
        let expected_score = passed as f64 / rules.len() as f64;
        let score = result
            .get("Score")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: the result reports a Score, got {result}"));
        assert!(
            (score - expected_score).abs() < SCORE_TOLERANCE,
            "case {case:?}: {passed} of {} rules passed, so the score is {expected_score}, got \
             {score}",
            rules.len()
        );

        // A read changes nothing: the same result reads back identically.
        let again = get_result(&router, &ids[0]).await;
        assert_eq!(
            again, result,
            "case {case:?}: two reads of one result report the same result, got {again}"
        );
    }
}

/// A run started for SEVERAL rulesets publishes one result per ruleset, each carrying that
/// ruleset's own rules and its own score.
#[tokio::test]
async fn a_run_of_several_rulesets_publishes_one_result_for_each() {
    let (router, engine) = emulator_with_s3();
    store_table(&router, "multi", Some(ROWS)).await;
    // (the ruleset, its rules, how many of them pass)
    let rulesets: [(&str, &[&str], usize); 3] = [
        ("multi-all-pass", &["RowCount = 3", "ColumnExists \"id\""], 2),
        ("multi-none-pass", &["RowCount = 4"], 0),
        (
            "multi-some-pass",
            &["IsComplete \"id\"", "IsComplete \"note\""],
            1,
        ),
    ];
    for (name, rules, _) in rulesets {
        store_ruleset(&router, name, rules).await;
    }

    let names: Vec<&str> = rulesets.iter().map(|(name, _, _)| *name).collect();
    let run_id = start_run(&router, "multi", &names).await;
    make_terminal_status_due(&engine);
    let run = get_run(&router, &run_id).await;
    let ids = result_ids(&run);
    assert_eq!(
        ids.len(),
        rulesets.len(),
        "a run reports one result id per ruleset it evaluated, got {run}"
    );

    for (position, (name, rules, passed)) in rulesets.iter().enumerate() {
        let result = get_result(&router, &ids[position]).await;
        assert_eq!(
            result.get("RulesetName").and_then(Value::as_str),
            Some(*name),
            "case {name:?}: the results follow the order the start listed the rulesets in, got \
             {result}"
        );
        assert_eq!(
            result
                .get("RuleResults")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(rules.len()),
            "case {name:?}: the result carries that ruleset's own rules, got {result}"
        );
        let expected_score = *passed as f64 / rules.len() as f64;
        let score = result.get("Score").and_then(Value::as_f64).unwrap_or(-1.0);
        assert!(
            (score - expected_score).abs() < SCORE_TOLERANCE,
            "case {name:?}: the score is that ruleset's own {expected_score}, got {score}"
        );
    }
}

/// A COMMENT declares nothing: it is no part of the rule's own text, and the rules it sits
/// between are still two rules — so the result reports each of them, described as its author
/// wrote it.
#[tokio::test]
async fn a_comment_is_no_part_of_the_rule_it_follows() {
    let (router, engine) = emulator_with_s3();
    store_table(&router, "comments", Some(ROWS)).await;
    store_ruleset_text(
        &router,
        "comment-ruleset",
        "# the rules of the staged table\nRules = [\n    RowCount = 3, # three rows are staged\n    \
         IsUnique \"id\"\n]",
    )
    .await;

    let run_id = start_run(&router, "comments", &["comment-ruleset"]).await;
    make_terminal_status_due(&engine);
    let ids = result_ids(&get_run(&router, &run_id).await);
    let result = get_result(&router, &ids[0]).await;
    let reported = result
        .get("RuleResults")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the result reports RuleResults, got {result}"));
    let described: Vec<&str> = reported
        .iter()
        .filter_map(|entry| entry.get("Description").and_then(Value::as_str))
        .collect();
    assert_eq!(
        described,
        vec!["RowCount = 3", "IsUnique \"id\""],
        "a comment is no part of the rule it follows, got {reported:?}"
    );
    assert!(
        reported.iter().all(|entry| entry.get("Result").and_then(Value::as_str)
            == Some(DATA_QUALITY_RULE_RESULT_PASS)),
        "the staged rows satisfy both rules, got {reported:?}"
    );
    let score = result.get("Score").and_then(Value::as_f64).unwrap_or(-1.0);
    assert!(
        (score - 1.0).abs() < SCORE_TOLERANCE,
        "every rule passed, so the score is 1, got {score}"
    );
}

/// A run whose data source holds nothing to read evaluates NOTHING: it fails, carries the
/// reason, and publishes no result — so there is no score for data nobody read.
#[tokio::test]
async fn a_run_whose_data_source_cannot_be_read_publishes_no_result() {
    let (router, engine) = emulator_with_s3();
    // (case, the table, whether its location holds an object)
    let cases: [(&str, &str, bool); 2] = [
        ("a location holding no object", "no_objects", false),
        ("a readable location", "readable", true),
    ];
    store_ruleset(&router, "unreadable-ruleset", &["RowCount = 3"]).await;

    for (case, table, staged) in cases {
        store_table(&router, table, staged.then_some(ROWS)).await;
        let run_id = start_run(&router, table, &["unreadable-ruleset"]).await;
        make_terminal_status_due(&engine);
        let run = get_run(&router, &run_id).await;
        let expected_status = if staged {
            DATA_QUALITY_RUN_STATE_SUCCEEDED
        } else {
            DATA_QUALITY_RUN_STATE_FAILED
        };
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(expected_status),
            "case {case:?}: expected the run to finish {expected_status}, got {run}"
        );
        if staged {
            continue;
        }
        assert!(
            result_ids(&run).is_empty(),
            "case {case:?}: a run that evaluated nothing publishes no result id, got {run}"
        );
        assert!(
            run.get("ErrorString")
                .and_then(Value::as_str)
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: a failed run carries the reason it evaluated nothing, got {run}"
        );
    }
}

/// A rule whose truth this evaluation cannot decide from the rows evaluates to NOTHING: the
/// run fails carrying the reason and publishes no result, so no rule is ever reported as
/// passed or failed unless the data was actually checked against it.
///
/// The rows differ along why the rule cannot be decided: a rule type nothing implements, a
/// rule truncated before its operand, an operand that is not the number the metric is
/// compared against, a metric of a column the table does not declare, a value condition in a
/// form this evaluation does not read, a fixed rule carrying a clause beyond its column
/// (which DQDL does put there, a `where` filter among them), and a rule carrying more tokens
/// than one rule holds.
#[tokio::test]
async fn a_rule_this_evaluation_cannot_decide_publishes_no_result() {
    let (router, engine) = emulator_with_s3();
    store_table(&router, "undecidable", Some(ROWS)).await;

    // A value list far longer than one rule's tokens, inside the modelled `length 1..65536`
    // of `Ruleset`.
    let oversized = format!(
        "ColumnValues \"status\" in [ {} \"ACTIVE\" ]",
        "\"ACTIVE\", ".repeat(1_000)
    );
    let cases: Vec<(&str, String)> = vec![
        ("a rule type nothing implements", "Frobnicate \"id\" = 3".to_string()),
        ("a rule truncated before its operand", "RowCount =".to_string()),
        (
            "an operand that is not a number",
            "RowCount = several".to_string(),
        ),
        (
            "a metric of a column the table does not declare",
            "IsComplete \"ghost_column\"".to_string(),
        ),
        (
            "a value condition in a form this evaluation does not read",
            "ColumnValues \"status\" matches \"ACT.*\"".to_string(),
        ),
        (
            "a fixed rule carrying a clause this evaluation does not read",
            "IsComplete \"id\" where \"status = 'ACTIVE'\"".to_string(),
        ),
        (
            "a uniqueness rule carrying a clause this evaluation does not read",
            "IsUnique \"id\" where \"status = 'ACTIVE'\"".to_string(),
        ),
        (
            "a column-exists rule carrying more than the column",
            "ColumnExists \"id\" \"status\"".to_string(),
        ),
        ("a rule carrying more tokens than one rule holds", oversized),
    ];

    for (index, (case, rule)) in cases.iter().enumerate() {
        let ruleset = format!("undecidable-ruleset-{index}");
        store_ruleset_text(
            &router,
            &ruleset,
            &format!("Rules = [\n    {rule}\n]"),
        )
        .await;
        let run_id = start_run(&router, "undecidable", &[ruleset.as_str()]).await;
        make_terminal_status_due(&engine);
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_FAILED),
            "case {case:?}: a run that could not decide a rule fails, got {run}"
        );
        assert!(
            result_ids(&run).is_empty(),
            "case {case:?}: a run that evaluated nothing publishes no result, got {run}"
        );
        assert!(
            run.get("ErrorString")
                .and_then(Value::as_str)
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: the run carries the reason it evaluated nothing, got {run}"
        );
    }

    // The evaluation still works for a ruleset it CAN decide, so the failures above are
    // about those rules and not about the surface having stopped working.
    store_ruleset(&router, "decidable-ruleset", &["RowCount = 3"]).await;
    let run_id = start_run(&router, "undecidable", &["decidable-ruleset"]).await;
    make_terminal_status_due(&engine);
    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
        "a ruleset this evaluation can decide still succeeds, got {run}"
    );
}

/// A `ResultId` no run has published is the modelled miss, and reading one changes nothing:
/// the result a run DID publish is still readable afterwards.
#[tokio::test]
async fn a_result_id_no_run_published_is_the_modelled_miss() {
    let (router, engine) = emulator_with_s3();
    store_table(&router, "miss", Some(ROWS)).await;
    store_ruleset(&router, "miss-ruleset", &["RowCount = 3"]).await;
    let run_id = start_run(&router, "miss", &["miss-ruleset"]).await;
    make_terminal_status_due(&engine);
    let published = result_ids(&get_run(&router, &run_id).await);
    let known = published
        .first()
        .unwrap_or_else(|| panic!("the finished run must publish one result id"))
        .clone();

    // (case, the id) — the rows differ along HOW the id misses: never minted, a prefix of a
    // published one, a published one with one more character, a published one recased, the
    // run's own id, and the shortest and longest `HashString` the model admits
    // (`length 1..255`, `smithy_query operation=GetDataQualityResult`).
    let cases: Vec<(&str, String)> = vec![
        ("an id nothing minted", "no-such-result".to_string()),
        (
            "a prefix of a published id",
            known[..known.len() - 1].to_string(),
        ),
        ("a published id with one more character", format!("{known}0")),
        ("a published id recased", known.to_uppercase()),
        ("the run's own id", run_id.clone()),
        ("the shortest id the model admits", "r".to_string()),
        ("the longest id the model admits", "r".repeat(255)),
    ];

    for (case, result_id) in &cases {
        assert_ne!(
            result_id, &known,
            "case {case:?}: the row must name an id no run published"
        );
        let (status, parsed) = call(
            &router,
            "GetDataQualityResult",
            &json!({ "ResultId": result_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a result id no run published is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&parsed)
        );
    }

    // None of those refusals touched the result that IS there.
    let result = get_result(&router, &known).await;
    assert_eq!(
        result.get("ResultId").and_then(Value::as_str),
        Some(known.as_str()),
        "the published result is still readable after every refused read, got {result}"
    );
}
