// SPDX-License-Identifier: CC0-1.0

//! What evaluating a stage's CONDITIONS records, and what `ListRuleExecutions` reports
//! from it — end to end over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.ListRuleExecutions`).
//!
//! The rule under test: a rule execution comes into being when a stage condition's
//! rule is EVALUATED, so the listing reports one record per declared rule per
//! evaluation — most recent first — and a pipeline whose conditions have never been
//! reached reports none. A rule's verdict is its provider's: a `VariableCheck` compares
//! a variable of the execution against a value, so its verdict is COMPUTED here, while
//! every other published provider decides outside this service and its evaluation
//! reaches no verdict (recorded `Abandoned` with the reason, never `Succeeded`).
//! `Condition$result` is "the action to be done when the condition is MET", so it is
//! applied only where a rule actually reached a failing verdict: `SKIP` on an entry
//! condition takes the execution past the stage, anything else fails it, and a
//! condition nothing decided does not act on the execution at all.
//!
//! The rows span what the rules generalise over: the condition site, how many rules a
//! condition declares, the comparison a `VariableCheck` states (its operator, its two
//! sides and their boundaries), which `Result` the condition states, how many
//! executions have reached it, and the page size and filter the listing is asked for.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so the pipelines here run
//! their real actions through the emulated S3 service and every assertion goes through
//! the router.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, creation_run, declaration, source_stage, wait_for_terminal, zip_of, Emulator,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "source-bucket";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The one entry the source archive carries, so a stage's delivery is observable.
const ENTRY: (&str, &[u8]) = ("app.txt", b"the app");

/// The `RuleExecutionStatus` an evaluation that reached no verdict carries
/// (`state::rules::RULE_EXECUTION_ABANDONED`).
const ABANDONED: &str = "Abandoned";

/// One `RuleDeclaration`. The providers are the rule types AWS publishes for stage
/// conditions (`DeploymentWindow`, `VariableCheck`, `LambdaInvoke`,
/// `CloudWatchAlarm`); `RuleTypeId` requires `category` and `provider`, and
/// `RuleCategory` declares the single value `Rule`.
fn rule(name: &str, provider: &str) -> Value {
    json!({
        "name": name,
        "ruleTypeId": {
            "category": "Rule",
            "owner": "AWS",
            "provider": provider,
            "version": "1",
        },
    })
}

/// A rule carrying a configuration, so what the record reports as the declared and the
/// resolved configuration is observable.
fn configured_rule(name: &str, provider: &str, configuration: Value) -> Value {
    let mut declared = rule(name, provider);
    declared["configuration"] = configuration;
    declared
}

/// A deploy stage delivering `ARTIFACT` into `bucket`, with a condition site spliced in
/// when one is given.
fn deploy_stage(name: &str, bucket: &str, site: Option<(&str, Value)>) -> Value {
    let mut stage = json!({
        "name": name,
        "actions": [{
            "name": "DeployAction",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {"BucketName": bucket, "Extract": "true"},
            "inputArtifacts": [{"name": ARTIFACT}],
        }],
    });
    if let Some((member, value)) = site {
        stage[member] = value;
    }
    stage
}

/// A source stage with a condition site spliced in.
fn conditioned_source_stage(site: &str, value: Value) -> Value {
    let mut stage = source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT);
    stage[site] = value;
    stage
}

/// Create the buckets a pipeline of this suite needs and write its source object.
async fn seed_buckets(emulator: &Emulator, pipeline: &str, deploy_buckets: &[&str]) {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    for bucket in deploy_buckets {
        emulator.bucket(bucket).await;
    }
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, &zip_of(&[ENTRY]))
        .await;
}

/// Start one execution of `pipeline` and answer its id.
async fn start(emulator: &Emulator, pipeline: &str) -> String {
    emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string()
}

/// `ListRuleExecutions` for `pipeline`, with the extra members `request` carries.
async fn list(emulator: &Emulator, pipeline: &str, request: Value) -> Value {
    let mut body = json!({"pipelineName": pipeline});
    for (member, value) in request.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    emulator.ok("ListRuleExecutions", &body).await
}

/// The records on a page.
fn records(page: &Value) -> Vec<Value> {
    page["ruleExecutionDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("a page reports ruleExecutionDetails, got {page}"))
        .clone()
}

/// The `ruleName` of every record on a page, in the order the page reports them.
fn rule_names(page: &Value) -> Vec<String> {
    records(page)
        .iter()
        .map(|record| {
            record["ruleName"]
                .as_str()
                .unwrap_or_else(|| panic!("a record names its rule, got {record}"))
                .to_string()
        })
        .collect()
}

/// RULE: every rule a reached condition declares is evaluated and recorded, and the
/// listing reports those records most recent first.
///
/// The rows differ along the condition site, the number of rules a condition declares
/// (both ends of `RuleDeclarationList`'s modelled 1..5 length) and the providers
/// declared.
#[tokio::test]
async fn every_rule_of_a_reached_condition_is_recorded_most_recent_first() {
    // (case, condition site on the first stage, the value that site carries, the rule
    // names in DECLARATION order)
    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "one beforeEntry rule",
            "beforeEntry",
            json!({
                "conditions": [{"result": "FAIL", "rules": [rule("Window", "DeploymentWindow")]}]
            }),
            vec!["Window"],
        ),
        (
            "the five rules RuleDeclarationList allows at most",
            "beforeEntry",
            json!({
                "conditions": [{
                    "result": "FAIL",
                    "rules": [
                        rule("One", "DeploymentWindow"),
                        rule("Two", "VariableCheck"),
                        rule("Three", "CloudWatchAlarm"),
                        rule("Four", "LambdaInvoke"),
                        rule("Five", "DeploymentWindow"),
                    ],
                }]
            }),
            vec!["One", "Two", "Three", "Four", "Five"],
        ),
        (
            "a condition that states no result at all",
            "beforeEntry",
            json!({"conditions": [{"rules": [rule("Alarm", "CloudWatchAlarm")]}]}),
            vec!["Alarm"],
        ),
        (
            "an onSuccess rule, evaluated once the stage's actions have succeeded",
            "onSuccess",
            json!({
                "conditions": [{"result": "FAIL", "rules": [rule("Check", "VariableCheck")]}]
            }),
            vec!["Check"],
        ),
    ];

    for (index, (case, site, value, declared)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("ruled-{index}");
        let deploy_bucket = format!("ruled-{index}-delivery");
        seed_buckets(&emulator, &pipeline, &[deploy_bucket.as_str()]).await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    // The delivery stage is the non-`Source` action every declaration
                    // carries (`state::validate_declaration`); the conditions under
                    // test sit on the source stage.
                    json!([
                        conditioned_source_stage(site, value),
                        deploy_stage("Delivery", &deploy_bucket, None),
                    ]),
                    None,
                ),
            )
            .await;

        // The execution under test is the one the pipeline's creation started: creating
        // a pipeline runs it once, so that run is what reaches the stage's condition.
        // (Rules on a stage NO execution has reached are the sibling suite's subject,
        // `tests/all/rule_executions.rs`.)
        let execution = creation_run(&emulator, &pipeline).await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!("Succeeded"),
            "case {case:?}: no condition was met, so none of them acted on the \
             execution, got {answer}"
        );

        let page = list(&emulator, &pipeline, json!({})).await;
        let mut expected: Vec<String> = declared.iter().map(|name| name.to_string()).collect();
        expected.reverse();
        assert_eq!(
            rule_names(&page),
            expected,
            "case {case:?}: every declared rule is recorded, most recent first, got {page}"
        );
        assert!(
            page["nextToken"].is_null(),
            "case {case:?}: a page that was not truncated issues no cursor, got {page}"
        );
        for record in records(&page) {
            assert_eq!(
                record["pipelineExecutionId"],
                json!(execution),
                "case {case:?}: a record belongs to the execution that evaluated it, got \
                 {record}"
            );
            assert_eq!(
                record["stageName"],
                json!("Source"),
                "case {case:?}: a record names the stage whose condition declared it, got \
                 {record}"
            );
            assert_eq!(
                record["status"],
                json!(ABANDONED),
                "case {case:?}: no provider's verdict was obtained, so nothing passed or \
                 failed, got {record}"
            );
            assert_eq!(
                record["input"]["ruleTypeId"]["category"],
                json!("Rule"),
                "case {case:?}: a record echoes the declared ruleTypeId, got {record}"
            );
            assert!(
                record["output"]["executionResult"]["errorDetails"]["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: a record that reached no verdict says why, got {record}"
            );
            assert_eq!(
                record["pipelineVersion"],
                json!(1),
                "case {case:?}: a record names the pipeline version it was read from, got \
                 {record}"
            );
        }

        // One record per rule, each under its own id.
        let ids: Vec<String> = records(&page)
            .iter()
            .map(|record| {
                record["ruleExecutionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: each evaluation is recorded under its own id, got {ids:?}"
        );
    }
}

/// RULE: the `Result` a condition declares is what happens when the condition is MET,
/// so a declared result changes nothing while no verdict is reached — the stage runs
/// and delivers whichever of the four `Result` values the condition states, and the
/// evaluation is recorded all the same.
#[tokio::test]
async fn a_declared_result_does_not_act_on_a_condition_nothing_decided() {
    // `Result`'s modelled values (`RESULT_VALUES` -> FAIL, RETRY, ROLLBACK, SKIP): the
    // whole set, because the rule is about every one of them.
    for result in ["FAIL", "RETRY", "ROLLBACK", "SKIP"] {
        let emulator = Emulator::new();
        let pipeline = format!("result-{result}");
        let deploy_bucket = format!("deploy-{}", result.to_lowercase());
        seed_buckets(&emulator, &pipeline, &[deploy_bucket.as_str()]).await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage(
                            "Gated",
                            &deploy_bucket,
                            Some((
                                "beforeEntry",
                                json!({
                                    "conditions": [{
                                        "result": result,
                                        "rules": [rule("Window", "DeploymentWindow")],
                                    }]
                                }),
                            )),
                        ),
                    ]),
                    None,
                ),
            )
            .await;

        // The execution under test is the run the pipeline's creation started.
        let execution = creation_run(&emulator, &pipeline).await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!("Succeeded"),
            "case {result:?}: a condition nothing decided is not met, so its result does \
             not act on the execution, got {answer}"
        );

        let state = emulator
            .ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
        let stages: BTreeMap<String, String> = state["stageStates"]
            .as_array()
            .expect("the state reports its stages")
            .iter()
            .map(|stage| {
                (
                    stage["stageName"].as_str().unwrap_or_default().to_string(),
                    stage["latestExecution"]["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            stages.get("Gated").map(String::as_str),
            Some("Succeeded"),
            "case {result:?}: the gated stage ran, got {state}"
        );

        let (status, bytes) = emulator.get_object(&deploy_bucket, ENTRY.0).await;
        assert_eq!(
            (status, bytes.as_slice()),
            (200, ENTRY.1),
            "case {result:?}: the gated stage delivered its artifact"
        );

        let page = list(&emulator, &pipeline, json!({})).await;
        assert_eq!(
            rule_names(&page),
            vec!["Window".to_string()],
            "case {result:?}: the evaluation is recorded whatever the result, got {page}"
        );
        assert_eq!(
            records(&page)[0]["stageName"],
            json!("Gated"),
            "case {result:?}: the record names the stage the condition belongs to, got {page}"
        );
    }
}

/// RULE: a `VariableCheck` rule's verdict is COMPUTED — it is the comparison its
/// configuration states, over a variable of the execution — so the rule passes when the
/// comparison holds, fails when it does not, and reaches no verdict when the
/// comparison cannot be performed here at all.
///
/// The rows differ along every part of the comparison the rule generalises over: the
/// operator, whether the variable is a reference or literal text, whether the two
/// sides are numbers or text, and whether the values sit at the boundary of the
/// ordering.
#[tokio::test]
async fn a_variable_check_rules_verdict_is_computed_from_the_executions_variables() {
    /// The variable the pipeline declares, and its value for every execution.
    const DECLARED: (&str, &str) = ("Release", "4");

    // (case, the Variable to check, the Value to check it against, the Operator, the
    // status the evaluation is recorded with, the status the execution reaches)
    let rows: Vec<(&str, &str, &str, &str, &str, &str)> = vec![
        (
            "equality that holds",
            "#{variables.Release}",
            "4",
            "EQ",
            "Succeeded",
            "Succeeded",
        ),
        (
            "equality that does not hold",
            "#{variables.Release}",
            "5",
            "EQ",
            "Failed",
            "Failed",
        ),
        (
            "inequality that holds",
            "#{variables.Release}",
            "5",
            "NE",
            "Succeeded",
            "Succeeded",
        ),
        (
            "an ordering over two numbers",
            "#{variables.Release}",
            "3",
            "GT",
            "Succeeded",
            "Succeeded",
        ),
        (
            "an ordering at its boundary",
            "#{variables.Release}",
            "4",
            "GE",
            "Succeeded",
            "Succeeded",
        ),
        (
            "an ordering that does not hold",
            "#{variables.Release}",
            "3",
            "LT",
            "Failed",
            "Failed",
        ),
        (
            "an ordering at its boundary the other way",
            "#{variables.Release}",
            "4",
            "LE",
            "Succeeded",
            "Succeeded",
        ),
        (
            "two pieces of text rather than two numbers",
            "release-candidate",
            "release-candidate",
            "EQ",
            "Succeeded",
            "Succeeded",
        ),
        (
            "text ordered against text",
            "release-a",
            "release-b",
            "LT",
            "Succeeded",
            "Succeeded",
        ),
        (
            // Spelled out, so it is none of the eight operators the rule publishes
            // (`PUBLISHED_OPERATORS`) and names no comparison this plane can perform.
            "an operator this plane does not apply",
            "#{variables.Release}",
            "4",
            "EQUALS",
            "Abandoned",
            "Succeeded",
        ),
        (
            "a variable the execution carries no value for",
            "#{variables.absent}",
            "4",
            "EQ",
            "Abandoned",
            "Succeeded",
        ),
    ];

    for (index, (case, variable, value, operator, recorded, expected)) in
        rows.into_iter().enumerate()
    {
        let emulator = Emulator::new();
        let pipeline = format!("checked-{index}");
        let deploy_bucket = format!("checked-deploy-{index}");
        seed_buckets(&emulator, &pipeline, &[deploy_bucket.as_str()]).await;
        let mut body = declaration(
            &pipeline,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                deploy_stage(
                    "Gated",
                    &deploy_bucket,
                    Some((
                        "beforeEntry",
                        json!({
                            "conditions": [{
                                "result": "FAIL",
                                "rules": [configured_rule(
                                    "Check",
                                    "VariableCheck",
                                    json!({
                                        "Variable": variable,
                                        "Value": value,
                                        "Operator": operator,
                                    }),
                                )],
                            }]
                        }),
                    )),
                ),
            ]),
            None,
        );
        // The variable the rule checks is one the pipeline declares, so the execution
        // carries a value for it (`PipelineVariableDeclaration$defaultValue`).
        body["pipeline"]["pipelineType"] = json!("V2");
        body["pipeline"]["variables"] =
            json!([{"name": DECLARED.0, "defaultValue": DECLARED.1}]);
        emulator.ok("CreatePipeline", &body).await;

        let execution = start(&emulator, &pipeline).await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!(expected),
            "case {case:?}: expected the execution to reach {expected}, got {answer}"
        );

        let page = list(&emulator, &pipeline, json!({})).await;
        let record = records(&page)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the evaluation is recorded, got {page}"));
        assert_eq!(
            record["status"],
            json!(recorded),
            "case {case:?}: expected the evaluation recorded {recorded}, got {record}"
        );
        assert_eq!(
            record["output"]["executionResult"]["errorDetails"].is_null(),
            recorded == "Succeeded",
            "case {case:?}: only an evaluation with nothing to report carries no \
             errorDetails, got {record}"
        );
        // What the comparison was performed over is reported as resolved, so a caller
        // can see the value the verdict was reached on.
        assert_eq!(
            record["input"]["resolvedConfiguration"]["Variable"],
            json!(if variable.contains("#{variables.Release}") {
                DECLARED.1
            } else {
                variable
            }),
            "case {case:?}: the resolved configuration is what was compared, got {record}"
        );

        // A rule that reached no failing verdict let the stage run and deliver; one
        // that failed stopped the stage before it ran.
        let (status, _) = emulator.get_object(&deploy_bucket, ENTRY.0).await;
        assert_eq!(
            status == 200,
            expected == "Succeeded",
            "case {case:?}: the gated stage delivered iff the gate did not trip, got \
             {status}"
        );
    }
}

/// RULE: the `Result` a condition declares is what the execution does when the
/// condition is MET — `SKIP` takes it past the stage and anything else fails the stage
/// — and a result this emulator would have to perform work for is not claimed as done.
#[tokio::test]
async fn the_result_of_a_met_condition_is_what_the_execution_does() {
    // (case, the Result declared, the status the stage reaches, the status the
    // execution reaches)
    let rows = [
        ("FAIL", "Failed", "Failed"),
        ("SKIP", "Skipped", "Succeeded"),
        ("ROLLBACK", "Failed", "Failed"),
        ("RETRY", "Failed", "Failed"),
    ];

    for (result, stage_status, execution_status) in rows {
        let emulator = Emulator::new();
        let pipeline = format!("met-{result}");
        let deploy_bucket = format!("met-deploy-{}", result.to_lowercase());
        seed_buckets(&emulator, &pipeline, &[deploy_bucket.as_str()]).await;
        let mut body = declaration(
            &pipeline,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                deploy_stage(
                    "Gated",
                    &deploy_bucket,
                    Some((
                        "beforeEntry",
                        json!({
                            "conditions": [{
                                "result": result,
                                // A comparison that does not hold, so the condition is
                                // met on a verdict that was actually reached.
                                "rules": [configured_rule(
                                    "Check",
                                    "VariableCheck",
                                    json!({
                                        "Variable": "#{variables.Release}",
                                        "Value": "99",
                                        "Operator": "EQ",
                                    }),
                                )],
                            }]
                        }),
                    )),
                ),
            ]),
            None,
        );
        body["pipeline"]["pipelineType"] = json!("V2");
        body["pipeline"]["variables"] = json!([{"name": "Release", "defaultValue": "4"}]);
        emulator.ok("CreatePipeline", &body).await;

        let execution = start(&emulator, &pipeline).await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!(execution_status),
            "case {result:?}: expected the execution to reach {execution_status}, got \
             {answer}"
        );
        let summary = answer["pipelineExecution"]["statusSummary"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if execution_status == "Failed" {
            assert!(
                summary.contains("Check"),
                "case {result:?}: the reason names the rule that tripped the gate, got \
                 {summary:?}"
            );
            // A result the stage could not simply be failed for says so by name.
            assert_eq!(
                summary.contains(result),
                result != "FAIL",
                "case {result:?}: the reason names a result it could not carry out, got \
                 {summary:?}"
            );
        }

        let state = emulator
            .ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
        let stages: BTreeMap<String, String> = state["stageStates"]
            .as_array()
            .expect("the state reports its stages")
            .iter()
            .map(|stage| {
                (
                    stage["stageName"].as_str().unwrap_or_default().to_string(),
                    stage["latestExecution"]["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            stages.get("Gated").map(String::as_str),
            Some(stage_status),
            "case {result:?}: expected the gated stage {stage_status}, got {state}"
        );

        // The gate tripped before the stage ran, whatever the result: nothing of it was
        // delivered.
        let (status, _) = emulator.get_object(&deploy_bucket, ENTRY.0).await;
        assert_eq!(
            status, 404,
            "case {result:?}: a stage the gate stopped runs no action, got {status}"
        );
    }
}

/// RULE: the listing is ordered newest-first across executions, pages by the cursor it
/// issues, and reports only the records of the execution a filter names.
#[tokio::test]
async fn the_listing_is_ordered_paged_and_filtered_by_execution() {
    let emulator = Emulator::new();
    let pipeline = "ordered-rules";
    let deploy_bucket = "ordered-rules-delivery";
    seed_buckets(&emulator, pipeline, &[deploy_bucket]).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    conditioned_source_stage(
                        "beforeEntry",
                        json!({
                            "conditions": [{
                                "result": "FAIL",
                                "rules": [
                                    configured_rule(
                                        "Older",
                                        "VariableCheck",
                                        json!({"Variable": "#{variables.absent}", "Value": "1"}),
                                    ),
                                    rule("Newer", "DeploymentWindow"),
                                ],
                            }]
                        }),
                    ),
                    deploy_stage("Delivery", deploy_bucket, None),
                ]),
                // PARALLEL, so the second execution runs on its own rather than being
                // held behind the first.
                Some("PARALLEL"),
            ),
        )
        .await;

    // The two executions whose evaluations this listing orders: the run the pipeline's
    // creation started, and one more start.
    let first = creation_run(&emulator, pipeline).await;
    let second = start(&emulator, pipeline).await;
    for execution in [&first, &second] {
        let answer = wait_for_terminal(&emulator, pipeline, execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!("Succeeded"),
            "both executions ran past the condition, got {answer}"
        );
    }

    // Newest first: the second execution's evaluations come before the first's, and
    // within one execution the later rule comes first.
    let page = list(&emulator, pipeline, json!({})).await;
    let order: Vec<(String, String)> = records(&page)
        .iter()
        .map(|record| {
            (
                record["pipelineExecutionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                record["ruleName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            (second.clone(), "Newer".to_string()),
            (second.clone(), "Older".to_string()),
            (first.clone(), "Newer".to_string()),
            (first.clone(), "Older".to_string()),
        ],
        "expected the evaluations newest first, got {page}"
    );

    // A record reports the configuration as declared AND as resolved: a placeholder
    // naming a variable the execution does not carry keeps its own text.
    let configured = records(&page)
        .into_iter()
        .find(|record| record["ruleName"] == json!("Older"))
        .expect("the configured rule is recorded");
    assert_eq!(
        configured["input"]["configuration"]["Variable"],
        json!("#{variables.absent}"),
        "the declared configuration is reported as declared, got {configured}"
    );
    assert_eq!(
        configured["input"]["resolvedConfiguration"]["Variable"],
        json!("#{variables.absent}"),
        "a placeholder the execution has no value for keeps its own text, got {configured}"
    );

    // Paged by the cursor the listing issues: one record per page, in the same order.
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..order.len() {
        let request = match &token {
            None => json!({"maxResults": 1}),
            Some(cursor) => json!({"maxResults": 1, "nextToken": cursor}),
        };
        let page = list(&emulator, pipeline, request).await;
        let listed = records(&page);
        assert_eq!(listed.len(), 1, "a page of one carries one record, got {page}");
        seen.push((
            listed[0]["pipelineExecutionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            listed[0]["ruleName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
        token = page["nextToken"].as_str().map(str::to_string);
    }
    assert_eq!(
        seen, order,
        "paging through one record at a time reports the same order"
    );
    assert!(
        token.is_none(),
        "the last page of the listing issues no cursor, got {token:?}"
    );

    // The filter selects one execution's records, through either member that names an
    // execution.
    for (case, filter) in [
        ("pipelineExecutionId", json!({"pipelineExecutionId": first})),
        (
            "latestInPipelineExecution",
            json!({
                "latestInPipelineExecution": {
                    "pipelineExecutionId": first,
                    "startTimeRange": "All",
                }
            }),
        ),
    ] {
        let page = list(&emulator, pipeline, json!({"filter": filter})).await;
        let executions: Vec<String> = records(&page)
            .iter()
            .map(|record| {
                record["pipelineExecutionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            executions,
            vec![first.clone(), first.clone()],
            "case {case:?}: only the named execution's evaluations are reported, got {page}"
        );
    }
}

/// Every comparison operator the `VariableCheck` rule publishes, from the CodePipeline
/// rule reference `RuleTypeId$provider` links to: equality, inequality, the four
/// orderings, and the two text predicates — whether the variable contains the value,
/// and whether it matches the regular expression the value states.
const PUBLISHED_OPERATORS: [&str; 8] =
    ["EQ", "NE", "GT", "GE", "LT", "LE", "CONTAINS", "MATCHES"];

/// One row of the operator table: the rule's name, the `Variable` and the `Value` it
/// compares, the `Operator` it compares them with, and the `RuleExecutionStatus` the
/// evaluation is expected to record.
type OperatorRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// One group of sibling rules declared by ONE condition: the group's name, the status
/// the execution reaches, and the group's rows.
type OperatorGroup = (&'static str, &'static str, Vec<OperatorRow>);

/// The variables every pipeline of the two operator suites below declares, and so the
/// left-hand side its comparisons are performed over
/// (`PipelineVariableDeclaration$defaultValue`).
const DECLARED: [(&str, &str); 2] = [("Release", "4"), ("Channel", "release-candidate")];

/// Run one pipeline whose gated stage declares `rules` as the ONE condition of its
/// `beforeEntry`, and answer what that run reached: the execution's status, the
/// `RuleExecutionStatus` recorded per rule name, and whether the gated stage delivered
/// its object.
///
/// Creating the pipeline runs it once, and that run is the one whose entry to the gated
/// stage the condition is evaluated for, so nothing else has to start one.
async fn gated_run(
    pipeline: &str,
    rules: Vec<Value>,
) -> (String, BTreeMap<String, String>, bool) {
    let emulator = Emulator::new();
    let deploy_bucket = format!("{pipeline}-delivery");
    seed_buckets(&emulator, pipeline, &[deploy_bucket.as_str()]).await;
    let mut body = declaration(
        pipeline,
        json!([
            source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
            deploy_stage(
                "Gated",
                &deploy_bucket,
                Some((
                    "beforeEntry",
                    json!({"conditions": [{"result": "FAIL", "rules": rules}]}),
                )),
            ),
        ]),
        None,
    );
    body["pipeline"]["pipelineType"] = json!("V2");
    body["pipeline"]["variables"] = json!(DECLARED
        .iter()
        .map(|(name, value)| json!({"name": name, "defaultValue": value}))
        .collect::<Vec<Value>>());
    emulator.ok("CreatePipeline", &body).await;

    let execution = creation_run(&emulator, pipeline).await;
    let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
    let status = answer["pipelineExecution"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let page = list(
        &emulator,
        pipeline,
        json!({"filter": {"pipelineExecutionId": execution}}),
    )
    .await;
    let recorded: BTreeMap<String, String> = records(&page)
        .iter()
        .map(|record| {
            (
                record["ruleName"].as_str().unwrap_or_default().to_string(),
                record["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let (delivered, _) = emulator.get_object(&deploy_bucket, ENTRY.0).await;
    (status, recorded, delivered == 200)
}

/// RULE: a `VariableCheck` rule's verdict is the comparison its `Operator` names,
/// performed over its two resolved sides, for EVERY operator the rule publishes; and
/// each rule of a condition records the verdict of its OWN comparison whatever a
/// sibling rule decided. Only a comparison that cannot be performed here at all — an
/// operator outside the published set, or a value that is not the expression its
/// operator compares with — reaches no verdict.
///
/// The groups span the whole published catalog with the sides of each comparison
/// side by side in ONE condition: comparisons that hold, comparisons that do not, and
/// comparisons nothing could perform.
#[tokio::test]
async fn every_published_variable_check_operator_decides_its_own_rule() {
    // (group, the status the execution reaches, [(rule name, Variable, Value, Operator,
    // the `RuleExecutionStatus` the evaluation records)])
    let groups: Vec<OperatorGroup> = vec![
        (
            "comparisons that hold",
            "Succeeded",
            vec![
                ("Equal", "#{variables.Release}", "4", "EQ", "Succeeded"),
                ("Unequal", "#{variables.Release}", "5", "NE", "Succeeded"),
                ("Greater", "#{variables.Release}", "3", "GT", "Succeeded"),
                ("AtLeast", "#{variables.Release}", "4", "GE", "Succeeded"),
                ("Less", "#{variables.Release}", "5", "LT", "Succeeded"),
            ],
        ),
        (
            "the remaining ordering, both text predicates, and two comparisons nothing \
             could perform",
            "Succeeded",
            vec![
                ("AtMost", "#{variables.Release}", "4", "LE", "Succeeded"),
                (
                    "Substring",
                    "#{variables.Channel}",
                    "candidate",
                    "CONTAINS",
                    "Succeeded",
                ),
                (
                    "Expression",
                    "#{variables.Channel}",
                    "^release-[a-z]+$",
                    "MATCHES",
                    "Succeeded",
                ),
                (
                    "NotAnExpression",
                    "#{variables.Channel}",
                    "release-[",
                    "MATCHES",
                    ABANDONED,
                ),
                (
                    "NotAnOperator",
                    "#{variables.Channel}",
                    "release-candidate",
                    "EQUALS",
                    ABANDONED,
                ),
            ],
        ),
        (
            "comparisons that do not hold",
            "Failed",
            vec![
                ("Equal", "#{variables.Release}", "5", "EQ", "Failed"),
                (
                    "Substring",
                    "#{variables.Channel}",
                    "beta",
                    "CONTAINS",
                    "Failed",
                ),
                (
                    "Expression",
                    "#{variables.Channel}",
                    "^beta-",
                    "MATCHES",
                    "Failed",
                ),
            ],
        ),
    ];

    // The operators that reached a verdict somewhere above, so the catalog is covered
    // rather than only the rows one group happens to carry.
    let mut decided: BTreeSet<String> = BTreeSet::new();

    for (index, (group, execution_status, rows)) in groups.iter().enumerate() {
        let rules: Vec<Value> = rows
            .iter()
            .map(|(name, variable, value, operator, _)| {
                configured_rule(
                    name,
                    "VariableCheck",
                    json!({
                        "Variable": variable,
                        "Value": value,
                        "Operator": operator,
                    }),
                )
            })
            .collect();
        let (status, recorded, delivered) = gated_run(&format!("operators-{index}"), rules).await;
        assert_eq!(
            status, *execution_status,
            "case {group:?}: expected the execution to reach {execution_status}, got \
             {status}"
        );

        let expected: BTreeMap<String, String> = rows
            .iter()
            .map(|(name, .., recorded_status)| (name.to_string(), recorded_status.to_string()))
            .collect();
        assert_eq!(
            recorded, expected,
            "case {group:?}: every rule of the condition records the verdict of its own \
             comparison, got {recorded:?}"
        );

        for (_, _, _, operator, recorded_status) in rows {
            if *recorded_status != ABANDONED {
                decided.insert((*operator).to_string());
            }
        }

        // The gate acts on the execution only where a rule reached a FAILING verdict:
        // the stage delivered iff none did.
        assert_eq!(
            delivered,
            *execution_status == "Succeeded",
            "case {group:?}: the gated stage delivered iff no rule failed, got \
             {delivered}"
        );
    }

    let covered: BTreeSet<String> = PUBLISHED_OPERATORS
        .iter()
        .map(|operator| (*operator).to_string())
        .collect();
    assert_eq!(
        decided, covered,
        "every operator the rule publishes reached a verdict, got {decided:?}"
    );
}

/// RULE: compiling the expression a `MATCHES` comparison states is work sized by the
/// declaration, so it is bounded: an expression within the bound is applied and decides
/// the rule, and one past it is not compiled at all — the evaluation reaches no verdict
/// and the execution carries on, rather than the declaration choosing how much work one
/// evaluation does.
///
/// The bound itself is this emulator's (`state::rules::MAX_EXPRESSION_CHARS`), so the
/// rows sit at it and one step past it.
#[tokio::test]
async fn the_expression_a_matches_comparison_compiles_is_bounded() {
    /// The bound, in characters of the expression.
    const MAX_EXPRESSION_CHARS: usize = 1_000;

    /// An expression of exactly `chars` characters that MATCHES the declared `Channel`:
    /// the literal it ends with, after as many optional characters as fill the rest.
    fn expression_of(chars: usize) -> String {
        let literal = format!("{}$", DECLARED[1].1);
        let padding = (chars - literal.len()) / 2;
        let expression = format!("{}{literal}", "x?".repeat(padding));
        assert_eq!(
            expression.chars().count(),
            chars,
            "the expression is built to exactly {chars} characters"
        );
        expression
    }

    // (case, the expression's length in characters, the status the evaluation records)
    let rows = [
        ("at the bound", MAX_EXPRESSION_CHARS, "Succeeded"),
        ("one step past the bound", MAX_EXPRESSION_CHARS + 2, ABANDONED),
    ];

    for (index, (case, length, recorded_status)) in rows.into_iter().enumerate() {
        let rules = vec![configured_rule(
            "Expression",
            "VariableCheck",
            json!({
                "Variable": format!("#{{variables.{}}}", DECLARED[1].0),
                "Value": expression_of(length),
                "Operator": "MATCHES",
            }),
        )];
        let (status, recorded, delivered) = gated_run(&format!("bounded-{index}"), rules).await;
        assert_eq!(
            recorded.get("Expression").map(String::as_str),
            Some(recorded_status),
            "case {case:?}: expected the evaluation recorded {recorded_status}, got \
             {recorded:?}"
        );
        // Neither row reached a FAILING verdict, so neither condition was met: the gate
        // acts on nothing and the stage runs.
        assert_eq!(
            (status.as_str(), delivered),
            ("Succeeded", true),
            "case {case:?}: an evaluation that reached no failing verdict lets the stage \
             run, got {status} and {delivered}"
        );
    }
}
