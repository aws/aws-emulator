// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `OperatorName` a task instance reports out of the operator
//! its workflow definition declared.
//!
//! The rule under test: a value this service reports out of the caller's definition file
//! is reported only when it is one the modelled member that carries it can hold. Both
//! `GetTaskInstanceResponse$OperatorName` and `TaskInstanceSummary$OperatorName` target
//! `GenericString`, modelled `@length(min: 1, max: 255)` with `@pattern(^.*$)`
//! (`smithy_query(query="shape",
//! shape="GetTaskInstanceResponse,TaskInstanceSummary,GenericString")`), so a declared
//! operator name inside those constraints is reported verbatim by both readers and one
//! outside them is reported as no member at all — never clipped, because a cut operator
//! name names a different operator than the definition declared. The task still runs
//! either way: an operator name is not what an instance is reported BY, so the run
//! executes the task and reports its outcome whatever the declaration carried.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a
//! definition is a YAML object a run's version snapshotted out of Amazon S3. The first
//! table spans what the declared VALUE varies over: whether an operator is declared at
//! all, both published length bounds and one character past each of them, a declaration
//! of several hundred characters, the same lengths written in multi-byte characters (the
//! modelled length counts characters, not bytes), a value whose punctuation and spaces no
//! identifier would carry but the member's pattern does, and a value carrying a line
//! break, which that pattern does not.
//!
//! The second table spans where that value is WRITTEN, because the rule is about the
//! member the value is reported through and not about the spelling the definition
//! declared it with: every key a task entry may name its operator under, crossed with
//! every form a definition may write that entry in, each with a value the member carries
//! and one past its maximum.

use crate::run_execution_outcome::{instances_by_task, read_task_instance, run_to_completion};
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The fewest characters the `GenericString` both `OperatorName` members target can
/// carry (`smithy_query(query="shape", shape="GenericString")` declares it 1..255).
const MIN_OPERATOR_NAME_CHARS: usize = 1;

/// The most characters that same `GenericString` can carry.
const MAX_OPERATOR_NAME_CHARS: usize = 255;

/// The identifier every row's single task is declared, stored and reported under.
const TASK_ID: &str = "t1";

/// The `TaskInstanceStatus` a task that ran to completion reports
/// (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
const TASK_SUCCESS: &str = "SUCCESS";

/// Every key a task entry may name its operator under, as the definition reader's own
/// `TASK_OPERATOR_KEYS` lists them: Apache Airflow's `operator` and `operator_name` DAG
/// vocabulary plus the shorter `type` spelling a declaration carries it under. All three
/// feed the one member, so the rule holds whichever of them a definition writes — and
/// only the first of them is the one the reported symptom was written against.
const OPERATOR_KEYS: &[&str] = &["operator", "operator_name", "type"];

/// An operator name well inside the modelled member, spelled as Apache Airflow spells a
/// provider operator's class.
const CARRIED_OPERATOR_NAME: &str = "S3ToRedshiftOperator";

/// One row: the operator a definition's single task declares, and the `OperatorName` the
/// two readers must therefore report for the instance of that task.
struct Row {
    case: String,
    /// The scalar the task entry declares `operator:` with, written exactly as the YAML
    /// document carries it, and `None` for an entry that declares no operator at all.
    declared: Option<String>,
    /// The `OperatorName` both readers must report, and `None` for a declaration the
    /// modelled member cannot carry, which is reported as no member at all.
    expected: Option<String>,
}

/// `character`, `count` times.
fn repeated(character: char, count: usize) -> String {
    std::iter::repeat_n(character, count).collect()
}

/// `value` as a double-quoted YAML scalar, so the document carries it verbatim however
/// it is punctuated. None of the values below holds a quote or a backslash of its own.
fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

/// The table. Rows differ along the declared value's presence, length, character width
/// and form — never merely along the workflow's name.
fn rows() -> Vec<Row> {
    let shortest = repeated('a', MIN_OPERATOR_NAME_CHARS);
    let longest = repeated('A', MAX_OPERATOR_NAME_CHARS);
    let one_past_longest = repeated('A', MAX_OPERATOR_NAME_CHARS + 1);
    // Two bytes per character, so these two span the modelled bound in CHARACTERS while
    // holding twice as many bytes.
    let longest_wide = repeated('é', MAX_OPERATOR_NAME_CHARS);
    let one_past_longest_wide = repeated('é', MAX_OPERATOR_NAME_CHARS + 1);
    vec![
        Row {
            case: "an ordinary operator name".to_string(),
            declared: Some(quoted("EmptyOperator")),
            expected: Some("EmptyOperator".to_string()),
        },
        Row {
            case: "a task entry declaring no operator".to_string(),
            declared: None,
            expected: None,
        },
        Row {
            case: format!("the shortest name the member carries ({MIN_OPERATOR_NAME_CHARS})"),
            declared: Some(quoted(&shortest)),
            expected: Some(shortest.clone()),
        },
        Row {
            case: "an empty declared operator name".to_string(),
            declared: Some(quoted("")),
            expected: None,
        },
        Row {
            case: format!("the longest name the member carries ({MAX_OPERATOR_NAME_CHARS})"),
            declared: Some(quoted(&longest)),
            expected: Some(longest.clone()),
        },
        Row {
            case: format!(
                "one character past the modelled maximum ({})",
                MAX_OPERATOR_NAME_CHARS + 1
            ),
            declared: Some(quoted(&one_past_longest)),
            expected: None,
        },
        Row {
            case: "a declaration of three hundred characters".to_string(),
            declared: Some(quoted(&repeated('b', 300))),
            expected: None,
        },
        Row {
            case: format!("{MAX_OPERATOR_NAME_CHARS} multi-byte characters"),
            declared: Some(quoted(&longest_wide)),
            expected: Some(longest_wide.clone()),
        },
        Row {
            case: format!("{} multi-byte characters", MAX_OPERATOR_NAME_CHARS + 1),
            declared: Some(quoted(&one_past_longest_wide)),
            expected: None,
        },
        Row {
            // The member's pattern is `^.*$`, so a name no identifier shape would carry
            // is still one this member can: the check is the modelled one, not a
            // spelling rule of this emulator's own.
            case: "a name carrying spaces and punctuation".to_string(),
            declared: Some(quoted("my custom.operator-v2 (beta)")),
            expected: Some("my custom.operator-v2 (beta)".to_string()),
        },
        Row {
            // A double-quoted YAML scalar turns the escape into a real line break, which
            // that same pattern does not carry.
            case: "a name carrying a line break".to_string(),
            declared: Some(r#""Empty\nOperator""#.to_string()),
            expected: None,
        },
    ]
}

/// The definition one row stages: a single task under `TASK_ID`, declaring the row's
/// operator scalar when it has one.
fn definition(row: &Row) -> String {
    match &row.declared {
        Some(scalar) => format!("tasks:\n  - id: {TASK_ID}\n    operator: {scalar}\n"),
        None => format!("tasks:\n  - id: {TASK_ID}\n"),
    }
}

/// The forms a definition may write ONE task entry in, all read by the same definition
/// walk: an entry of a `tasks` SEQUENCE naming its own identifier, an entry of a `tasks`
/// MAPPING named by the key it sits under, either of those nested under the `dag` key a
/// definition may wrap its DAG-level declarations in, and the JSON spelling of the same
/// document (YAML 1.2 is a superset of JSON).
#[derive(Debug, Clone, Copy)]
enum EntryForm {
    Sequence,
    Mapping,
    NestedUnderDag,
    Json,
}

impl EntryForm {
    /// Every form, so the table below crosses all of them with every operator key.
    const ALL: &'static [Self] = &[
        Self::Sequence,
        Self::Mapping,
        Self::NestedUnderDag,
        Self::Json,
    ];

    /// How the case names this form.
    fn label(self) -> &'static str {
        match self {
            Self::Sequence => "an entry of a tasks sequence",
            Self::Mapping => "an entry of a tasks mapping",
            Self::NestedUnderDag => "an entry nested under the dag key",
            Self::Json => "an entry of a document written as JSON",
        }
    }

    /// A definition declaring one task under `TASK_ID` in this form, naming its operator
    /// under `key` with the already-quoted scalar `scalar`.
    fn definition(self, key: &str, scalar: &str) -> String {
        match self {
            Self::Sequence => format!("tasks:\n  - id: {TASK_ID}\n    {key}: {scalar}\n"),
            Self::Mapping => format!("tasks:\n  {TASK_ID}:\n    {key}: {scalar}\n"),
            Self::NestedUnderDag => {
                format!("dag:\n  tasks:\n    - id: {TASK_ID}\n      {key}: {scalar}\n")
            }
            Self::Json => {
                format!("{{\"tasks\": [{{\"id\": \"{TASK_ID}\", \"{key}\": {scalar}}}]}}")
            }
        }
    }
}

/// One row of the second table: a whole definition, and the `OperatorName` both readers
/// must report for the task it declares.
struct FormRow {
    case: String,
    definition: String,
    expected: Option<String>,
}

/// The second table: every operator key crossed with every entry form, each declaring a
/// value the modelled member carries and a value one character past its maximum.
fn form_rows() -> Vec<FormRow> {
    let one_past_longest = repeated('A', MAX_OPERATOR_NAME_CHARS + 1);
    let mut rows = Vec::with_capacity(OPERATOR_KEYS.len() * EntryForm::ALL.len() * 2);
    for key in OPERATOR_KEYS {
        for form in EntryForm::ALL {
            rows.push(FormRow {
                case: format!(
                    "a name the member carries, declared under {key:?} in {}",
                    form.label()
                ),
                definition: form.definition(key, &quoted(CARRIED_OPERATOR_NAME)),
                expected: Some(CARRIED_OPERATOR_NAME.to_string()),
            });
            rows.push(FormRow {
                case: format!(
                    "a name one character past the modelled maximum, declared under \
                     {key:?} in {}",
                    form.label()
                ),
                definition: form.definition(key, &quoted(&one_past_longest)),
                expected: None,
            });
        }
    }
    rows
}

/// The `OperatorName` a reported member holds, and `None` when the member is absent (or
/// explicitly null, which is the same answer on the wire).
fn reported(member: &Value) -> Option<&str> {
    member.as_str()
}

/// Stage `definition` as the object `key`, create the workflow `name` over it and start
/// ONE run of it, answering the workflow's ARN and the run's identifier.
///
/// Shared by both tables so every row's run is started before any of them is polled: the
/// executions then overlap on the one clock they all run on rather than being waited for
/// one after another.
async fn start_run(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    definition: &str,
    case: &str,
) -> (String, String) {
    stage(router, key, definition.as_bytes()).await;
    let arn = create_workflow(router, name, key).await;
    let (status, run) = call(
        router,
        "StartWorkflowRun",
        json!({"WorkflowArn": arn.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {run:?}"
    );
    let run_id = run["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a started run has a RunId"))
        .to_string();
    (arn, run_id)
}

/// Let the run finish and assert that BOTH readers of its single task instance report
/// `expected` as the `OperatorName`, and that the task ran whatever the definition
/// declared.
async fn assert_both_readers_report(
    router: &MultiServiceRouter,
    arn: &str,
    run_id: &str,
    case: &str,
    expected: Option<&str>,
) {
    run_to_completion(router, arn, run_id, case).await;

    // The instance of the one declared task, under the identifier the run minted for it.
    let instances = instances_by_task(router, arn, run_id, case).await;
    let instance_id = instances
        .get(TASK_ID)
        .unwrap_or_else(|| panic!("case {case:?}: expected the run to execute {TASK_ID:?}"));

    // The task ran whatever its declaration carried: an unreportable operator name
    // refuses nothing.
    let read = read_task_instance(router, arn, run_id, instance_id, case).await;
    assert_eq!(
        read["Status"].as_str(),
        Some(TASK_SUCCESS),
        "case {case:?}: the declared task must still run, got {:?}",
        read["Status"]
    );
    assert_eq!(
        read["TaskId"].as_str(),
        Some(TASK_ID),
        "case {case:?}: the instance must name the task it executes, got {:?}",
        read["TaskId"]
    );
    assert_eq!(
        reported(&read["OperatorName"]),
        expected,
        "case {case:?}: GetTaskInstance must report OperatorName {expected:?}, got {:?}",
        read["OperatorName"]
    );

    // The listing reads the same record through the other modelled member, so it
    // answers with the same value.
    let (status, page) = call(
        router,
        "ListTaskInstances",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTaskInstances to succeed, got {status} {page:?}"
    );
    let summaries = page["TaskInstances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TaskInstances list, got {page:?}"))
        .clone();
    let summary = summaries
        .iter()
        .find(|summary| summary["TaskInstanceId"].as_str() == Some(instance_id.as_str()))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for {instance_id:?}, got {summaries:?}")
        });
    assert_eq!(
        reported(&summary["OperatorName"]),
        expected,
        "case {case:?}: ListTaskInstances must report OperatorName {expected:?}, got {:?}",
        summary["OperatorName"]
    );
}

#[tokio::test]
async fn an_instance_reports_the_declared_operator_only_when_the_modelled_member_carries_it() {
    let router = definition_router();
    create_bucket(&router).await;

    let rows = rows();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let key = format!("dags/operator-{index}.yaml");
        let (arn, run_id) = start_run(
            &router,
            &format!("operatorName{index}"),
            &key,
            &definition(row),
            &row.case,
        )
        .await;
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        assert_both_readers_report(&router, &arn, &run_id, &row.case, row.expected.as_deref()).await;
    }
}

#[tokio::test]
async fn the_same_check_holds_for_every_operator_key_and_entry_form_a_definition_writes() {
    let router = definition_router();
    create_bucket(&router).await;

    let rows = form_rows();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let key = format!("dags/operator-key-{index}.yaml");
        let (arn, run_id) = start_run(
            &router,
            &format!("operatorKey{index}"),
            &key,
            &row.definition,
            &row.case,
        )
        .await;
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        assert_both_readers_report(&router, &arn, &run_id, &row.case, row.expected.as_deref()).await;
    }
}
