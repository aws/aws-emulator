// SPDX-License-Identifier: CC0-1.0

//! The DAG a workflow run EXECUTES: the tasks the run's version declared, the order the
//! dependencies between them put those tasks in, and what the run therefore finishes as.
//!
//! `StartWorkflowRun` "creates a workflow run that executes the tasks defined in your
//! workflow definition", and a definition "defines the workflow structure using supported
//! Amazon Web Services operators and tasks" with "task dependencies" in it
//! (`smithy_query(query="doc", shape="CreateWorkflowRequest",
//! member="DefinitionS3Location")`). So a run is not a status that moves on its own: it is
//! the execution of the task graph its version froze, and the status it finishes in is the
//! one that execution produced.
//!
//! The plan is built ONCE, when the run is recorded, out of the definition that version
//! snapshotted — a version is immutable, so what it holds IS what the run executes, and a
//! plan built once is what makes two reads of the same run answer identically. Nothing is
//! slept on: the plan says what happens, at which dependency LEVEL and for how long, and
//! [`crate::state::WorkflowRun::settle`] materialises each level as the clock makes it
//! due.
//!
//! ## What the plan decides
//!
//! * A graph that can be ordered runs: every task declared in it gets one task instance,
//!   the tasks that wait for nothing run first, and a task runs once the tasks it
//!   declared as dependencies have finished. The run finishes in the modelled `SUCCESS`
//!   status with no `ErrorMessage`, which is the outcome `WorkflowRunDetail$ErrorMessage`
//!   documents ("This value is null if the run completed successfully") — unless one of
//!   its tasks declares work that cannot succeed, see below.
//! * WHETHER each task succeeds: a task's declaration states the work it performs, and a
//!   declared program the session performs to a non-zero status is work that did not
//!   succeed ([`super::declared_runtime`]). Such a task's instance ends `FAILED`, the
//!   tasks waiting on it never run (`UPSTREAM_FAILED`), and the run reaches the modelled
//!   `FAILED` status with an `ErrorMessage` naming the task it failed on
//!   ([`PlannedTask::failure`]). A declaration whose program this session does not perform
//!   states no outcome, so the task it declares is not failed on that ground: the
//!   operator's own work against the resources it names is the worker's, not this
//!   emulator's.
//! * How LONG each task occupies, and so how long the run is executing: the span a task
//!   runs for is the one its own declaration states ([`super::declared_runtime`]), and the
//!   default span for a task that declares none ([`super::TASK_LEVEL_RUNS_FOR`]). The
//!   tasks at one level run together, so a level occupies the longest of them
//!   ([`ExecutionPlan::level_runs_for`]) and the run occupies the levels one after
//!   another — which is the window its `StartedOn`, `CompletedOn` and `Duration` report.
//! * Which of the values the definition DECLARES each instance reports: a value read out
//!   of the caller's definition is reported only when it is one the modelled member that
//!   carries it can hold, so an operator name outside that member is not reported at all
//!   and an undeclarable task identifier makes the graph unbuildable, because an instance
//!   names the task it executes.
//! * A graph that CANNOT be ordered is a workflow definition no run of it can execute: a
//!   task that depends on a task the definition does not declare, a dependency cycle, two
//!   tasks declaring one identifier, a task identifier outside what the modelled `TaskId`
//!   can carry, or more tasks than
//!   [`workflow_definition::MAX_DECLARED_TASKS`]. Such a run reaches the modelled `FAILED`
//!   status with an `ErrorMessage` naming the condition and the task it was found on, and
//!   holds no task instance: nothing of it ran.
//!
//! What a task's operator DOES to the resources it names is the operator's own work
//! against another service, performed by the worker the real service runs it on; this
//! emulator serves the orchestration — admitting the run, building the graph, ordering it
//! and recording an instance per task through the modelled statuses — and carries out no
//! operator's side effect. Nothing here invents an outcome for a task either: an instance
//! reports the outcome of the orchestration it took part in and the outcome the task's own
//! declaration states.

use std::collections::HashMap;
use std::time::Duration;

use super::workflow_definition::{self, TaskDeclaration, MAX_DECLARED_TASKS};
use super::{declared_runtime, ConstrainedValue, M_OPERATOR_NAME, M_TASK_INSTANCE_ID};

/// The most characters a run's `ErrorMessage` carries: the modelled length of the
/// `GenericString` it targets (`smithy_query(query="shape", shape="WorkflowRunDetail")`
/// declares `ErrorMessage` 1..255).
const MAX_ERROR_MESSAGE_CHARS: usize = 255;

/// ONE task of a run's plan: the task the definition declared, and the dependency LEVEL
/// the run's instance of it runs at.
///
/// The plan names the DECLARED task only. The identifier the run's instance of it is
/// stored and reported under is not in the definition at all: it is minted per (run, task,
/// attempt) by the run that records the instance
/// ([`super::mint_task_instance_id`]), so a plan built from one definition serves every
/// run of it without two runs sharing an instance identifier.
#[derive(Debug, Clone)]
pub struct PlannedTask {
    /// `TaskId`: the task in the workflow definition this instance executes.
    pub task_id: String,
    /// `OperatorName`: the operator the definition declared for the task, by the NAME that
    /// declaration names it with (see [`reported_operator_name`]), when it declared one
    /// this service can report.
    pub operator_name: Option<String>,
    /// How many dependency levels deep this task sits: 0 for a task that waits for
    /// nothing, and otherwise one past the deepest task it waits for. Tasks at one level
    /// run together, because nothing between them has to wait.
    pub level: usize,
    /// How long this task's instance is `RUNNING` for: the wait its own declaration
    /// states ([`super::declared_runtime::declared_program`]), and the run's default span
    /// ([`super::TASK_LEVEL_RUNS_FOR`]) for a task whose declaration states none.
    pub runs_for: Duration,
    /// Why an execution of this task cannot succeed, when its own declaration says so:
    /// the reason the run reports for failing on it ([`clipped`] to what the modelled
    /// `ErrorMessage` can carry). `None` for a task whose declaration states no failing
    /// outcome, which is every task whose declared work this emulator does not perform.
    pub failure: Option<String>,
}

/// What a run executes: the plan built from its version's definition.
#[derive(Debug, Clone, Default)]
pub struct ExecutionPlan {
    /// One entry per task the definition declares, in declaration order.
    pub tasks: Vec<PlannedTask>,
    /// Why this definition's graph could not be built, when it could not be: the
    /// `ErrorMessage` the run reports when it fails. `None` for a graph that runs.
    pub build_error: Option<String>,
}

impl ExecutionPlan {
    /// The plan for a run of `definition`.
    ///
    /// A version that snapshotted no definition content, and a definition that declares
    /// no task, both plan NO task: there is nothing to run and so nothing that can fail,
    /// which is a run that completes successfully.
    pub fn of(definition: Option<&str>) -> Self {
        let Some(definition) = definition else {
            return Self::default();
        };
        let declared = workflow_definition::declared_tasks(definition);
        if declared.past_ceiling {
            return Self::unbuildable(format!(
                "This workflow definition declares more than the {MAX_DECLARED_TASKS} tasks \
                 this emulator runs in one workflow run."
            ));
        }
        match plan_tasks(&declared.tasks) {
            Ok(tasks) => Self {
                tasks,
                build_error: None,
            },
            Err(reason) => Self::unbuildable(reason),
        }
    }

    /// How many dependency levels the plan has: 0 for a plan with no task.
    pub fn levels(&self) -> usize {
        self.tasks
            .iter()
            .map(|task| task.level + 1)
            .max()
            .unwrap_or(0)
    }

    /// The tasks that run at `level`.
    pub fn tasks_at(&self, level: usize) -> impl Iterator<Item = &PlannedTask> {
        self.tasks.iter().filter(move |task| task.level == level)
    }

    /// How long `level` occupies: the longest span any task at it runs for, because the
    /// tasks at one level run together and the level is done when the last of them is.
    ///
    /// A level the plan holds no task at — which is every level of a plan with no task at
    /// all — occupies the default span, so a run with nothing to execute still passes
    /// through `RUNNING` ([`super::TASK_LEVEL_RUNS_FOR`]).
    pub fn level_runs_for(&self, level: usize) -> Duration {
        self.tasks_at(level)
            .map(|task| task.runs_for)
            .max()
            .unwrap_or(super::TASK_LEVEL_RUNS_FOR)
    }

    /// A plan for a definition whose graph could not be built: nothing runs, and `reason`
    /// is what the run reports as the reason it failed.
    fn unbuildable(reason: String) -> Self {
        Self {
            tasks: Vec::new(),
            build_error: Some(clipped(reason)),
        }
    }
}

/// Order the declared tasks: resolve the dependencies each one declares and give every
/// task the level it runs at, or report why this graph cannot be ordered.
///
/// A declaration this reader found no identifier in is not a task it can run — a task
/// instance is reported BY its identifier — so it takes no part in the graph, the same way
/// the schedule walk ignores a declaration it cannot name.
fn plan_tasks(declared: &[TaskDeclaration]) -> Result<Vec<PlannedTask>, String> {
    let named: Vec<(&str, &TaskDeclaration)> = declared
        .iter()
        .filter_map(|task| Some((task.task_id.as_deref()?, task)))
        .collect();

    let mut position: HashMap<&str, usize> = HashMap::with_capacity(named.len());
    for (index, (task_id, _)) in named.iter().enumerate() {
        if !reportable_task_id(task_id) {
            return Err(format!(
                "Task id {task_id:?} has to be made of at most 255 alphanumeric characters, \
                 dashes, dots and underscores, starting with an alphanumeric one."
            ));
        }
        if position.insert(*task_id, index).is_some() {
            return Err(format!(
                "Task id {task_id:?} has already been added to this workflow definition."
            ));
        }
    }

    // The dependencies, as positions in `named`. A dependency naming no declared task is
    // one no run can wait for, so the graph cannot be ordered.
    let mut upstream: Vec<Vec<usize>> = Vec::with_capacity(named.len());
    for (task_id, task) in &named {
        let mut edges = Vec::with_capacity(task.dependencies.len());
        for dependency in &task.dependencies {
            let Some(at) = position.get(dependency.as_str()) else {
                return Err(format!(
                    "Task {dependency:?}, declared as a dependency of task {task_id:?}, is not \
                     defined in this workflow definition."
                ));
            };
            edges.push(*at);
        }
        upstream.push(edges);
    }

    // Level by level: a task whose dependencies all have a level sits one past the
    // deepest of them, and a task that waits for nothing sits at level 0. The walk stops
    // when a pass places nothing, which is exactly when what is left depends on itself.
    let mut levels: Vec<Option<usize>> = vec![None; named.len()];
    loop {
        let mut placed = false;
        for index in 0..named.len() {
            if levels[index].is_some() {
                continue;
            }
            let mut level = 0;
            let mut ready = true;
            for at in &upstream[index] {
                match levels[*at] {
                    Some(above) => level = level.max(above + 1),
                    None => {
                        ready = false;
                        break;
                    }
                }
            }
            if ready {
                levels[index] = Some(level);
                placed = true;
            }
        }
        if !placed {
            break;
        }
    }

    if let Some(index) = levels.iter().position(Option::is_none) {
        let (task_id, _) = named[index];
        return Err(format!(
            "Cycle detected in this workflow definition. Faulty task: {task_id:?}."
        ));
    }

    Ok(named
        .iter()
        .zip(levels)
        .map(|((task_id, task), level)| {
            // What the task's own declaration states about the work it performs: the time
            // it takes, and whether it answers failure ([`super::declared_runtime`]). One
            // reading, because one performance of the program states both.
            let declared = task
                .command
                .as_deref()
                .map(declared_runtime::declared_program)
                .unwrap_or_default();
            PlannedTask {
                task_id: (*task_id).to_string(),
                operator_name: task
                    .operator_name
                    .as_deref()
                    .and_then(reported_operator_name),
                level: level.unwrap_or_default(),
                runs_for: declared.runs_for.unwrap_or(super::TASK_LEVEL_RUNS_FOR),
                failure: declared.failed.then(|| task_failure_reason(task_id)),
            }
        })
        .collect())
}

/// The `ErrorMessage` a run reports for failing on the task `task_id`: the task it failed
/// on, named, and nothing of the work itself, because the reason a program answered
/// failure is the program's and not this service's to state.
///
/// Cut to what the modelled member can carry ([`clipped`]), so a long declared identifier
/// cannot push the reported reason past its declared length.
fn task_failure_reason(task_id: &str) -> String {
    clipped(format!(
        "Task {task_id:?} of this workflow definition did not complete successfully."
    ))
}

/// Whether a declared task identifier is one this service can REPORT: an instance names
/// the task it executes in `TaskId`, and `TaskId` and `TaskInstanceId` both target the
/// modelled `IdString`, so the walk over either member's schema is the same check.
///
/// The check is the framework's own constraint walk over that member's schema
/// ([`super::validate_members`]), so the pattern and length are the model's rather than
/// this module's.
fn reportable_task_id(task_id: &str) -> bool {
    super::validate_members(&[(&M_TASK_INSTANCE_ID, ConstrainedValue::Text(task_id))]).is_empty()
}

/// The `OperatorName` a task instance reports for the operator its definition declared,
/// and `None` for a declaration this service cannot report at all.
///
/// `OperatorName` is "The name of the Apache Airflow operator used for this task
/// instance" (`smithy_query(query="shape",
/// shape="GetTaskInstanceResponse,TaskInstanceSummary")`), and an Apache Airflow DAG names
/// an operator either by that NAME alone (`BashOperator`) or by the fully qualified Python
/// import path of the class that implements it
/// (`airflow.providers.standard.operators.bash.BashOperator`). The name is the class at the
/// end of such a path, so a declaration written as one is reported by its last segment
/// ([`operator_class_name`]) and a declaration that is not an import path is reported as it
/// stands.
fn reported_operator_name(declared: &str) -> Option<String> {
    let name = operator_class_name(declared);
    reportable_operator_name(name).then(|| name.to_string())
}

/// The class name at the end of `declared` when `declared` is a dotted Python import path
/// — more than one segment, every one of them a Python identifier — and the whole of
/// `declared` otherwise.
///
/// The identifier check is what keeps a declared name that merely CARRIES a dot (one
/// written with punctuation or spaces, which the modelled `GenericString` admits) reported
/// whole: only a path whose every segment could be a Python module or class name names a
/// class at its end.
fn operator_class_name(declared: &str) -> &str {
    let segments: Vec<&str> = declared.split('.').collect();
    if segments.len() > 1 && segments.iter().all(|segment| is_python_identifier(segment)) {
        return segments.last().copied().unwrap_or(declared);
    }
    declared
}

/// Whether `text` could be one segment of a Python import path: a non-empty name of ASCII
/// letters, digits and underscores that does not start with a digit.
fn is_python_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Whether a declared operator name is one this service can REPORT:
/// `GetTaskInstanceResponse$OperatorName` and `TaskInstanceSummary$OperatorName` both
/// target the modelled `GenericString`, so one check covers both readers of the value.
///
/// The check is the same framework constraint walk [`reportable_task_id`] runs, over that
/// member's own schema ([`super::M_OPERATOR_NAME`]), so the length and pattern are the
/// model's rather than this module's. An unreportable value is not clipped into one that
/// fits: a cut operator name names a DIFFERENT operator than the definition declared,
/// where a cut reason ([`clipped`]) is still the reason. It is left out instead, the
/// conclusion the snapshot reader reaches for definition content the member that carries
/// it cannot hold — the task itself still runs, because an operator name is not what an
/// instance is reported BY.
fn reportable_operator_name(operator_name: &str) -> bool {
    super::validate_members(&[(&M_OPERATOR_NAME, ConstrainedValue::Text(operator_name))]).is_empty()
}

/// `message`, cut to what the modelled `ErrorMessage` can carry
/// ([`MAX_ERROR_MESSAGE_CHARS`]) on a character boundary, so a long task identifier in it
/// cannot push the reported reason past its declared length.
fn clipped(message: String) -> String {
    match message.char_indices().nth(MAX_ERROR_MESSAGE_CHARS) {
        Some((at, _)) => message[..at].to_string(),
        None => message,
    }
}
