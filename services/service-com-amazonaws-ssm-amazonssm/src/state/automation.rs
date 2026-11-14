// SPDX-License-Identifier: CC0-1.0

//! The Automation slice: an automation execution a caller started, the step executions it
//! is made of, and the walk through them the clock owes.
//!
//! `StartAutomationExecution` resolves the Automation runbook a start names, reads the
//! steps out of that version's content ([`runbook`]), resolves the execution's parameters
//! against the ones the runbook declares and writes ONE
//! [`AutomationExecutionRecord`] here; `GetAutomationExecution` reports that record. An
//! execution is keyed by `(region, execution id)`: an automation runs at the Systems
//! Manager endpoint of one Region and is identified there by the id the start minted.
//!
//! # What runs, and what does not
//!
//! An execution is minted `Pending` — nothing has picked it up yet — and the service picks
//! it up [`PICKUP_DELAY`] later, which is the one transition this emulator owes a start.
//! From then on the walk runs the steps the runbook declares, in the order its `nextStep`
//! and `onFailure` members lay out, and every status, instant and input it reports is
//! COMPUTED by that walk:
//!
//! * `aws:branch` decides its own choice from the execution's own values and goes to the
//!   step that choice names ([`runbook::holds`]) — the whole of what that action does;
//! * `aws:sleep` waits out the span its `Duration` names, or until the instant its
//!   `Timestamp` names, read with the one ISO 8601 reading this emulator has
//!   (`service_common::iso8601_span`), and the step is reported `InProgress` until that
//!   instant has passed;
//! * `aws:changeInstanceState` brings the Amazon EC2 instances its `InstanceIds` name to
//!   the state its `DesiredState` asks for, by calling THAT service — the instance is its
//!   resource and the transition is its own — and the step is reported `InProgress` until
//!   that service reports every named instance at a state the step accepts
//!   ([`instance_state`]);
//! * `aws:pause` stops the walk with the step `Waiting`, which is what the action does:
//!   the execution resumes on the `Resume` signal a caller sends, and
//!   `SendAutomationSignal` is a separate scaffold, so it stays `Waiting` until that
//!   operation is implemented. An `Interactive` execution waits the same way, before its
//!   first step, for the `StartStep` signal that mode requires.
//!
//! Every other action runs somewhere this emulator does not reach — a script runtime, a
//! managed node's agent, another service's API — so the walk runs NOTHING for it and
//! reports the step `Failed` with a failure message naming the action it did not run.
//! Nothing here reports a step as having succeeded, produced output or taken time unless
//! the walk above did that work.
//!
//! # Rate control
//!
//! A start that names a `TargetParameterName` and a `Targets` entry is a rate-controlled
//! automation: "A key-value mapping to target resources" resolves to a set of VALUES, and
//! each of them gets its own execution with that parameter bound to it and
//! `ParentAutomationExecutionId` naming the start's own execution
//! ([`ComAmazonawsSsmAmazonssmState::advance_automations`]). `MaxConcurrency` is how many
//! of those child executions run at a time and `MaxErrors` how many of them may fail
//! before the rest are cancelled, both read through [`rate_count`]. The parent runs no
//! step of its own: its status is the aggregate of its children's.
//!
//! # What one call does
//!
//! The walk is pure computation and runs under the one store lock, so what it can do in
//! one call is bounded by this store's own bounds rather than by the request: at most
//! [`MAX_EXECUTIONS_RETAINED`] executions are held at all and [`MAX_CONCURRENT_EXECUTIONS`]
//! of one Region's are walkable, each walks at most [`MAX_STEP_ENTRIES`] steps of a runbook
//! whose whole content was admitted within the document-content limit
//! ([`super::MAX_DOCUMENT_CONTENT`]), each branch condition is bounded in depth
//! ([`runbook`]), and what resolving those steps' references may EXPAND that content into
//! is bounded per execution by [`runbook::MAX_RESOLVED_INPUT_BYTES`] — the one cap on the
//! product of how many references a runbook carries and how long each of them resolves to.
//! Nothing in it sleeps, reads a clock twice, allocates from a size the request chose or
//! reaches outside this store.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::serde_json::{Map, Value};

use crate::ops::types::{
    AlarmConfiguration, AutomationExecution, FailureDetails, ResolvedTargets, StepExecution, Target,
    TargetLocation,
};

use super::ComAmazonawsSsmAmazonssmState;

/// The Automation runbook an execution walks: the steps its content declares and the
/// decisions they carry.
pub mod runbook;

/// The report of the executions a Region holds: which of them a request's filters select,
/// and the metadata each is reported as.
pub mod report;

/// The `aws:changeInstanceState` action: the step that changes the state of the Amazon EC2
/// instances it names, in that service itself.
pub mod instance_state;

/// The `AutomationExecutionStatus` an execution is minted with: nothing has picked it up
/// yet. From the modelled enum (`crate::ops::types::AUTOMATION_EXECUTION_STATUS_VALUES`).
pub const STATUS_PENDING: &str = "Pending";

/// The `AutomationExecutionStatus` of an execution the service has picked up and is
/// walking (same enum).
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// The `AutomationExecutionStatus` of an execution that is waiting for a caller's signal
/// (same enum).
pub const STATUS_WAITING: &str = "Waiting";

/// The `AutomationExecutionStatus` of an execution whose every step succeeded (same enum).
pub const STATUS_SUCCESS: &str = "Success";

/// The `AutomationExecutionStatus` of an execution a critical step failed in (same enum).
pub const STATUS_FAILED: &str = "Failed";

/// The `AutomationExecutionStatus` of an execution that was stopped before it ran (same
/// enum).
pub const STATUS_CANCELLED: &str = "Cancelled";

/// The `AutomationExecutionStatus` values an execution never leaves again: a read of one
/// reports it unchanged however long after it ended.
pub const TERMINAL_STATUSES: &[&str] = &[STATUS_SUCCESS, STATUS_FAILED, STATUS_CANCELLED];

/// Whether `status` is one an execution or a step never leaves ([`TERMINAL_STATUSES`]).
pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// The `ExecutionMode` an automation runs in when the start names none: "The default mode
/// is Auto" (`StartAutomationExecutionRequest$Mode`). From the modelled `ExecutionMode`
/// enum.
pub const EXECUTION_MODE_AUTO: &str = "Auto";

/// The `ExecutionMode` in which each step waits for the `StartStep` signal before it runs
/// (same enum).
pub const EXECUTION_MODE_INTERACTIVE: &str = "Interactive";

/// The `DocumentType` of a document `StartAutomationExecution` runs: an Automation runbook
/// ("Initiates execution of an Automation runbook"). From the modelled `DocumentType`
/// enum.
pub const AUTOMATION_DOCUMENT_TYPE: &str = "Automation";

/// The `MaxConcurrency` a start that names none runs with: "The default value is 10"
/// (`StartAutomationExecutionRequest$MaxConcurrency`).
pub const DEFAULT_MAX_CONCURRENCY: &str = "10";

/// The `MaxErrors` a start that names none runs with, as the Systems Manager
/// `start-automation-execution` reference gives it.
pub const DEFAULT_MAX_ERRORS: &str = "1";

/// How many tags a start may carry: "When calling the StartAutomationExecution action, you
/// can specify a maximum of 5 tags" (`StartAutomationExecutionRequest$Tags`).
pub const MAX_TAGS_PER_AUTOMATION: usize = 5;

/// How many automation executions one Region runs at a time, from the Systems Manager
/// service quota for concurrently running automations. A start past it is what
/// `AutomationExecutionLimitExceededException` ("The number of simultaneously running
/// Automation executions exceeded the allowable limit") reports.
pub const MAX_CONCURRENT_EXECUTIONS: usize = 100;

/// How many automation executions this store keeps at once, oldest evicted first.
///
/// The emulator's own guard — not an AWS quota — the same shape as the command store's:
/// past it the oldest execution is forgotten, so a caller starting automations in a loop
/// cannot grow this store without bound.
pub const MAX_EXECUTIONS_RETAINED: usize = 1_000;

/// How long after a start the service picks the execution up: the one transition this
/// emulator owes an automation, from `Pending` ("the execution hasn't started") to
/// `InProgress`. A span this emulator chooses, so that a caller acting straight after the
/// start still sees the execution it was answered with.
pub const PICKUP_DELAY: Duration = Duration::from_secs(3);

/// The `Key` of a target whose `Values` ARE the target values, rather than a criterion
/// resolved against the managed nodes: `Key=ParameterValues,Values=value-1,value-2`, the
/// form the Systems Manager User Guide's rate-control walkthrough uses to run an
/// automation once per value.
pub const TARGET_KEY_PARAMETER_VALUES: &str = "ParameterValues";

/// The `FailureDetails$FailureStage` of a step that failed while it was running, from the
/// stages Automation reports a failure in (`Validation`, `Execution`, `Verification`).
pub const FAILURE_STAGE_EXECUTION: &str = "Execution";

/// The `FailureDetails$FailureType` of a step whose action could not be run at all.
pub const FAILURE_TYPE_ACTION: &str = "Action";

/// The `aws:branch` action: "aws:branch" runs a conditional branch over the execution's
/// own values, from the Systems Manager Automation action reference.
pub const ACTION_BRANCH: &str = "aws:branch";

/// The `aws:sleep` action: it delays the automation for a span or until an instant (same
/// reference).
pub const ACTION_SLEEP: &str = "aws:sleep";

/// The `aws:pause` action: it pauses the automation until a caller resumes it (same
/// reference).
pub const ACTION_PAUSE: &str = "aws:pause";

/// How many step entries one walk makes before this emulator gives up on it.
///
/// `nextStep` and the `step:` form of `onFailure` both name a step by name, so a runbook
/// can name a step the walk has already run. The walk refuses to re-enter a step that has
/// already ended (that is the loop), and this is the second bound: whatever the content
/// says, one call walks a bounded number of steps.
pub const MAX_STEP_ENTRIES: usize = runbook::MAX_RUNBOOK_STEPS * 2;

/// The `StepExecutionFilterKey` that selects the steps that started before an instant.
pub const STEP_FILTER_KEY_START_TIME_BEFORE: &str = "StartTimeBefore";

/// The `StepExecutionFilterKey` that selects the steps that started after an instant.
pub const STEP_FILTER_KEY_START_TIME_AFTER: &str = "StartTimeAfter";

/// The `StepExecutionFilterKey` that selects the steps a status holds for.
pub const STEP_FILTER_KEY_STEP_EXECUTION_STATUS: &str = "StepExecutionStatus";

/// The `StepExecutionFilterKey` that selects ONE step execution by its id.
pub const STEP_FILTER_KEY_STEP_EXECUTION_ID: &str = "StepExecutionId";

/// The `StepExecutionFilterKey` that selects the steps the runbook declares under a name.
pub const STEP_FILTER_KEY_STEP_NAME: &str = "StepName";

/// The `StepExecutionFilterKey` that selects the steps running an action.
pub const STEP_FILTER_KEY_ACTION: &str = "Action";

/// The `StepExecutionFilterKey` that selects the steps run inside the step of that id —
/// `ParentStepDetails$StepExecutionId`.
pub const STEP_FILTER_KEY_PARENT_STEP_EXECUTION_ID: &str = "ParentStepExecutionId";

/// The `StepExecutionFilterKey` that selects the steps of one iteration of a parent step —
/// `ParentStepDetails$Iteration`, a number.
pub const STEP_FILTER_KEY_PARENT_STEP_ITERATION: &str = "ParentStepIteration";

/// The `StepExecutionFilterKey` that selects the steps of the iteration a parent step was
/// at one value of its iterator — `ParentStepDetails$IteratorValue`.
pub const STEP_FILTER_KEY_PARENT_STEP_ITERATOR_VALUE: &str = "ParentStepIteratorValue";

/// The bounds the modelled `MaxResults` of a step-execution report carries (range 1..50).
pub const MIN_STEP_EXECUTION_PAGE_SIZE: i32 = 1;

/// See [`MIN_STEP_EXECUTION_PAGE_SIZE`]. It is also the page size a request naming no
/// `MaxResults` is served, the way every other read of this service serves one.
pub const MAX_STEP_EXECUTION_PAGE_SIZE: i32 = 50;

/// The page size a step-execution report serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses,
/// so this is a backstop rather than the first line of defence.
pub fn step_execution_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_STEP_EXECUTION_PAGE_SIZE,
        MAX_STEP_EXECUTION_PAGE_SIZE,
    )
}

/// Whether `key` is a filter key a step-execution report accepts: the modelled
/// `StepExecutionFilterKey` enum is that whole set, so nothing else names a property of a
/// step execution to narrow by.
pub fn is_step_execution_filter_key(key: &str) -> bool {
    crate::ops::types::STEP_EXECUTION_FILTER_KEY_VALUES.contains(&key)
}

/// Why `value` is not a value the step-execution filter key `key` may carry, or `None`
/// when it is one.
///
/// A filter value has to be a legal value of what it filters: a timestamp for the two
/// instant keys, an `AutomationExecutionStatus` for the status key — which is what
/// `StepExecution$StepStatus` carries — and a number for the parent-iteration key, which
/// filters `ParentStepDetails$Iteration`. A name, an id and an action constrain nothing
/// here: a value no step carries selects no step rather than refusing the read.
pub fn step_execution_filter_value_rejection(key: &str, value: &str) -> Option<String> {
    let takes = |what: String| Some(invalid_filter_value_message(key, value, &what));
    match key {
        STEP_FILTER_KEY_START_TIME_BEFORE | STEP_FILTER_KEY_START_TIME_AFTER
            if super::commands::parse_filter_instant(value).is_none() =>
        {
            takes("a timestamp, for example 2024-07-07T00:00:00Z".to_string())
        }
        STEP_FILTER_KEY_STEP_EXECUTION_STATUS
            if !crate::ops::types::AUTOMATION_EXECUTION_STATUS_VALUES.contains(&value) =>
        {
            takes(format!(
                "one of [{}]",
                crate::ops::types::AUTOMATION_EXECUTION_STATUS_VALUES.join(", ")
            ))
        }
        STEP_FILTER_KEY_PARENT_STEP_ITERATION if value.parse::<u32>().is_err() => {
            takes("a step iteration number".to_string())
        }
        _ => None,
    }
}

/// How this slice words the refusal of a filter value its key cannot carry: the value that
/// was sent, the key it was sent for, and what that key does take.
///
/// One wording for every filter of the Automation slice, so the two reports that take
/// filters — the executions a Region holds and the steps of one of them — cannot word the
/// same refusal differently.
pub fn invalid_filter_value_message(key: &str, value: &str, takes: &str) -> String {
    format!("The following filter value is not valid: {value}. The {key} filter key takes {takes}.")
}

/// What one entry into a step did to the walk.
enum Walked {
    /// The step is running and has not ended: the walk stops here and the execution stays
    /// `InProgress` (an `aws:sleep` whose instant has not passed).
    Running,
    /// The step is waiting for a caller's signal: the execution is `Waiting`.
    Waiting,
    /// The step ended; the walk goes to the step this names, or ends when it names none.
    Moved(Option<usize>),
}

/// ONE step execution of an automation: the step as the runbook declares it, and what the
/// walk has done with it.
#[derive(Debug, Clone)]
pub struct StepExecutionRecord {
    /// The step as the runbook declared it when the execution started, so a later edit of
    /// the document cannot change what this execution runs.
    pub definition: runbook::RunbookStep,
    /// "The unique ID of a step execution" (`StepExecution$StepExecutionId`), minted by
    /// this store's own minter.
    pub step_execution_id: String,
    /// An `AutomationExecutionStatus` value, which is what `StepExecution$StepStatus`
    /// carries.
    pub status: String,
    /// "Fully-resolved values passed into the step before execution"
    /// (`StepExecution$Inputs`), written when the step starts and never before: a step in
    /// `Pending` has had nothing resolved into it.
    pub inputs: BTreeMap<String, String>,
    /// "If a step has begun execution, this contains the time the step started"
    /// (`StepExecution$ExecutionStartTime`).
    pub started_at: Option<SystemTime>,
    /// "If a step has finished execution, this contains the time the execution ended"
    /// (`StepExecution$ExecutionEndTime`).
    pub ended_at: Option<SystemTime>,
    /// The instant this step's own wait ends, for the one action that waits out a span
    /// (`aws:sleep`); `None` for every other step.
    pub due_at: Option<SystemTime>,
    /// "If a step failed, this message explains why the execution failed"
    /// (`StepExecution$FailureMessage`).
    pub failure_message: Option<String>,
    /// What this step asked Amazon EC2 for, for the one action whose work is that service's
    /// ([`instance_state`]); `None` for every other step. The step is running until that
    /// service reports the instances at the state it asked for.
    pub instance_state: Option<instance_state::InstanceStateChange>,
}

impl StepExecutionRecord {
    /// The step execution a start creates for one declared step: nothing has run it, so it
    /// is `Pending` with no resolved inputs, no window and no outcome.
    pub fn pending(definition: runbook::RunbookStep, step_execution_id: String) -> Self {
        Self {
            definition,
            step_execution_id,
            status: STATUS_PENDING.to_string(),
            inputs: BTreeMap::new(),
            started_at: None,
            ended_at: None,
            due_at: None,
            failure_message: None,
            instance_state: None,
        }
    }

    /// Whether this step execution meets the filter `key` = `values`.
    ///
    /// A filter carries one or more values of its key ("The values of the filter key",
    /// `StepExecutionFilter$Values`), so the step meets the filter when it meets the key's
    /// test for ANY of them; the report then keeps the steps that meet EVERY filter.
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        values
            .iter()
            .any(|value| self.matches_filter_value(key, value))
    }

    /// Whether this step execution meets the filter key `key` at the single value `value`.
    ///
    /// The three parent keys filter `StepExecution$ParentStepDetails` — the step an action
    /// that runs other steps ran this one inside of. Nothing in this emulator runs a step
    /// inside another step, so no step execution carries those details and such a filter
    /// selects no step.
    fn matches_filter_value(&self, key: &str, value: &str) -> bool {
        let started_at_satisfies = |ordering: ::std::cmp::Ordering| {
            match (
                self.started_at,
                super::commands::parse_filter_instant(value),
            ) {
                (Some(started), Some(bound)) => started.cmp(&bound) == ordering,
                // "If the step is in Pending status, this field isn't populated"
                // (`StepExecution$ExecutionStartTime`): a step that has not started is
                // neither before nor after an instant.
                _ => false,
            }
        };
        match key {
            STEP_FILTER_KEY_STEP_NAME => self.definition.name == value,
            STEP_FILTER_KEY_ACTION => self.definition.action == value,
            STEP_FILTER_KEY_STEP_EXECUTION_ID => self.step_execution_id == value,
            STEP_FILTER_KEY_STEP_EXECUTION_STATUS => self.status == value,
            STEP_FILTER_KEY_START_TIME_BEFORE => started_at_satisfies(::std::cmp::Ordering::Less),
            STEP_FILTER_KEY_START_TIME_AFTER => started_at_satisfies(::std::cmp::Ordering::Greater),
            _ => false,
        }
    }

    /// The modelled `StepExecution` for this step: the runbook's own declaration beside
    /// what the walk did. A member nothing has produced is left unset rather than filled
    /// in — a step that has not started reports no window, and one that ran no action
    /// reports no outputs and no response.
    pub fn to_step_execution(&self) -> StepExecution {
        StepExecution {
            step_name: Some(self.definition.name.clone()),
            action: Some(self.definition.action.clone()),
            timeout_seconds: self.definition.timeout_seconds,
            on_failure: Some(self.definition.on_failure().to_string()),
            max_attempts: Some(
                self.definition
                    .max_attempts
                    .unwrap_or(runbook::DEFAULT_MAX_ATTEMPTS),
            ),
            execution_start_time: self.started_at.map(::aws_smithy_types::DateTime::from),
            execution_end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            step_status: Some(self.status.clone()),
            response_code: None,
            inputs: (!self.inputs.is_empty()).then(|| {
                self.inputs
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
            outputs: None,
            response: None,
            failure_message: self.failure_message.clone(),
            warning_message: None,
            failure_details: self.failure_message.as_ref().map(|_| FailureDetails {
                failure_stage: Some(FAILURE_STAGE_EXECUTION.to_string()),
                failure_type: Some(FAILURE_TYPE_ACTION.to_string()),
                details: None,
            }),
            step_execution_id: Some(self.step_execution_id.clone()),
            overridden_parameters: None,
            is_end: self.definition.is_end,
            next_step: self.definition.next_step.clone(),
            is_critical: Some(self.definition.critical()),
            valid_next_steps: None,
            targets: None,
            target_location: None,
            triggered_alarms: None,
            parent_step_details: None,
        }
    }
}

/// One automation execution as this store holds it: the start as Systems Manager received
/// it, the runbook it resolved, and what the walk has done with it.
#[derive(Debug, Clone)]
pub struct AutomationExecutionRecord {
    /// "The execution ID" (`AutomationExecution$AutomationExecutionId`), and part of the
    /// store key.
    pub execution_id: String,
    /// The Region the automation runs in; the other part of its store key.
    pub region: String,
    /// "The name of the Automation runbook used during the execution", by the name the
    /// document is held under rather than the ARN spelling a start may have named it by.
    pub document_name: String,
    /// The version the start RESOLVED, which is the one whose content this execution runs.
    pub document_version: String,
    /// An `ExecutionMode` value.
    pub mode: String,
    /// "The key-value map of execution parameters, which were supplied when calling
    /// StartAutomationExecution" — the start's own values, ordered so two reads agree.
    pub supplied_parameters: BTreeMap<String, Vec<String>>,
    /// The parameters the walk resolves references against: the supplied values, plus the
    /// default of every declared parameter the start named no value for.
    pub resolved_parameters: BTreeMap<String, Vec<String>>,
    /// The search criteria the start named.
    pub targets: Vec<Target>,
    /// "The name of the parameter used as the target resource for the rate-controlled
    /// execution" (`StartAutomationExecutionRequest$TargetParameterName`).
    pub target_parameter_name: Option<String>,
    /// "The target of the execution" (`AutomationExecution$Target`): the one value a child
    /// of a rate-controlled automation runs for.
    pub target: Option<String>,
    /// The values the start's targets resolved to, which is what `ResolvedTargets` reports
    /// and what the children were created for.
    pub resolved_target_values: Vec<String>,
    /// "A key-value mapping of document parameters to target resources"
    /// (`StartAutomationExecutionRequest$TargetMaps`), as the start supplied it.
    pub target_maps: Vec<::std::collections::HashMap<String, Vec<String>>>,
    /// The Regions and accounts the start named.
    pub target_locations: Vec<TargetLocation>,
    /// "A publicly accessible URL for a file that contains the TargetLocations body".
    pub target_locations_url: Option<String>,
    /// "The AutomationExecutionId of the parent automation", for a child of a
    /// rate-controlled automation.
    pub parent_execution_id: Option<String>,
    /// The executions this one started, one per resolved target value, by their ids.
    pub children: Vec<String>,
    pub max_concurrency: String,
    pub max_errors: String,
    /// The tags the start carried, reachable through the tag plane by this execution's id.
    pub tags: BTreeMap<String, String>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    /// "User-provided idempotency token" — what makes a retried start the SAME execution.
    pub client_token: Option<String>,
    /// What the idempotency token was used WITH, so a retry carrying the same token and
    /// different members is told apart from a retry of the same call
    /// ([`request_fingerprint`]).
    pub fingerprint: String,
    /// "The Amazon Resource Name (ARN) of the user who ran the automation"
    /// (`AutomationExecution$ExecutedBy`).
    pub executed_by: String,
    /// "The time the execution started" (`AutomationExecution$ExecutionStartTime`).
    pub started_at: SystemTime,
    /// "The time the execution finished", once it has.
    pub ended_at: Option<SystemTime>,
    /// When the service picks this execution up ([`PICKUP_DELAY`]); `None` for a child a
    /// rate-controlled parent has not released yet, which is why such a child stays
    /// `Pending`.
    pub runnable_at: Option<SystemTime>,
    /// An `AutomationExecutionStatus` value.
    pub status: String,
    /// "A message describing why an execution has failed, if the status is set to Failed",
    /// written by the walk from the step that failed it.
    pub failure_message: Option<String>,
    /// One record per step the runbook declares, in the runbook's own order. Empty for a
    /// rate-controlled parent, which runs its children rather than steps of its own.
    pub steps: Vec<StepExecutionRecord>,
    /// The step the walk is at, by index into [`Self::steps`]; `None` once the walk has
    /// left the last step.
    pub current: Option<usize>,
    /// What is LEFT of this execution's budget for RESOLVED step inputs
    /// ([`runbook::MAX_RESOLVED_INPUT_BYTES`]): resolving a reference expands the
    /// runbook's own content into the values the execution holds, so the walk charges
    /// every step it resolves against this one budget and fails the step that runs over
    /// it. One budget per execution is what bounds the product of how many references a
    /// runbook carries and how long each of them resolves to.
    pub resolved_input_budget: usize,
}

impl AutomationExecutionRecord {
    /// A child of this rate-controlled execution: the same start, with the target
    /// parameter bound to `target` and its own steps.
    ///
    /// "The combination of … where you want to run the automation" is the parent's; what
    /// differs per child is the ONE value of the target parameter, so a child resolves its
    /// references against the parent's parameters with that value written over them.
    pub fn child(
        &self,
        execution_id: String,
        target: String,
        steps: Vec<StepExecutionRecord>,
        runnable_at: Option<SystemTime>,
    ) -> Self {
        let mut resolved_parameters = self.resolved_parameters.clone();
        if let Some(name) = &self.target_parameter_name {
            resolved_parameters.insert(name.clone(), vec![target.clone()]);
        }
        Self {
            execution_id,
            resolved_parameters,
            target: Some(target),
            targets: Vec::new(),
            resolved_target_values: Vec::new(),
            parent_execution_id: Some(self.execution_id.clone()),
            children: Vec::new(),
            client_token: None,
            runnable_at,
            current: (!steps.is_empty()).then_some(0),
            steps,
            // Each child walks its own copy of the runbook, so each has its own budget.
            resolved_input_budget: runbook::MAX_RESOLVED_INPUT_BYTES,
            ..self.clone()
        }
    }

    /// Whether this execution has ended ([`TERMINAL_STATUSES`]).
    pub fn ended(&self) -> bool {
        is_terminal(&self.status)
    }

    /// This execution's step executions in the order a report reads them: the order the
    /// walk started them in, with the steps nothing has started yet after those, in the
    /// runbook's own order.
    ///
    /// `reverse` reads that order backwards: "Indicates whether to list step executions in
    /// reverse order by start time" (`DescribeAutomationStepExecutionsRequest$ReverseOrder`).
    /// Two steps started at the one instant — and two steps that have not started at all —
    /// are ordered by their place in the runbook, which is what makes the order total and a
    /// page of it resumable.
    pub fn step_executions_in_order(&self, reverse: bool) -> Vec<&StepExecutionRecord> {
        let mut ordered: Vec<(usize, &StepExecutionRecord)> =
            self.steps.iter().enumerate().collect();
        ordered.sort_by(|(left_at, left), (right_at, right)| {
            match (left.started_at, right.started_at) {
                (Some(left_start), Some(right_start)) => left_start.cmp(&right_start),
                (Some(_), None) => ::std::cmp::Ordering::Less,
                (None, Some(_)) => ::std::cmp::Ordering::Greater,
                (None, None) => ::std::cmp::Ordering::Equal,
            }
            .then_with(|| left_at.cmp(right_at))
        });
        let mut ordered: Vec<&StepExecutionRecord> =
            ordered.into_iter().map(|(_, step)| step).collect();
        if reverse {
            ordered.reverse();
        }
        ordered
    }

    /// Materialise everything the clock already owes this execution at `now`: the pickup
    /// its start was answered with, and every step the walk can run from where it stands.
    ///
    /// Owed whether or not anybody called, which is why a read may run it: nothing here
    /// depends on the request, only on the instant.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.ended() {
            return;
        }
        if self.status == STATUS_PENDING {
            // A child the parent has not released yet has no pickup owed to it.
            let Some(runnable_at) = self.runnable_at else {
                return;
            };
            if now < runnable_at {
                return;
            }
            self.status = STATUS_IN_PROGRESS.to_string();
        }
        if self.status == STATUS_WAITING || self.steps.is_empty() {
            return;
        }
        let mut entries = 0usize;
        while let Some(index) = self.current {
            entries += 1;
            if entries > MAX_STEP_ENTRIES {
                self.failure_message = Some(format!(
                    "The runbook's steps did not end after {MAX_STEP_ENTRIES} step entries"
                ));
                self.current = None;
                break;
            }
            match self.enter_step(index, now) {
                Walked::Running => return,
                Walked::Waiting => {
                    self.status = STATUS_WAITING.to_string();
                    return;
                }
                Walked::Moved(next) => self.current = next,
            }
        }
        self.status = if self.failure_message.is_some() {
            STATUS_FAILED.to_string()
        } else {
            STATUS_SUCCESS.to_string()
        };
        self.ended_at = Some(now);
    }

    /// Run what the step at `index` can run at `now`.
    ///
    /// `index` is this record's own ([`Self::current`], which [`Self::step_after`] keeps
    /// inside [`Self::steps`]), never a request's; every read of it below still goes
    /// through `get`, so a walk can only ever end early rather than reach past the steps
    /// the runbook declared.
    fn enter_step(&mut self, index: usize, now: SystemTime) -> Walked {
        let Some(step) = self.steps.get(index) else {
            return Walked::Moved(None);
        };
        if is_terminal(&step.status) {
            // The walk was sent back to a step that has already ended, so the runbook's
            // own flow members loop. Nothing runs a step twice here.
            let name = step.definition.name.clone();
            self.failure_message = Some(format!(
                "Step {name} was reached twice; a runbook's steps cannot loop"
            ));
            return Walked::Moved(None);
        }
        if step.status == STATUS_PENDING {
            return self.start_step(index, now);
        }
        // A step whose work is another service's is running until that service answers: what
        // it asked for is recorded on the step, and the answer is fed back into it
        // ([`Self::record_instance_state`]), so the walk reads the outcome here rather than
        // deciding one of its own.
        if let Some(change) = step.instance_state.clone() {
            let name = step.definition.name.clone();
            if let Some(why) = change.failure {
                return self.fail_step(index, now, why);
            }
            if change.settled {
                return self.succeed_step(index, now, None);
            }
            // "DesiredStateTransitionTimeout — The amount of time to complete the desired
            // state transition": past it the instances are not where the step asked for
            // them, which fails the step.
            return match step.due_at {
                Some(due) if due <= now => self.fail_step(
                    index,
                    now,
                    instance_state::transition_timed_out_message(&change, &name),
                ),
                _ => Walked::Running,
            };
        }
        // The only step the walk leaves running is a wait, and what it is waiting for is
        // its own instant.
        match step.due_at {
            Some(due) if now < due => Walked::Running,
            _ => self.succeed_step(index, now, None),
        }
    }

    /// Resolve the step's inputs and run its action, as far as `now` allows.
    fn start_step(&mut self, index: usize, now: SystemTime) -> Walked {
        // "Interactive … you must manually start each step": a step of an interactive
        // execution runs when the caller signals it, so the walk waits here.
        if self.mode == EXECUTION_MODE_INTERACTIVE {
            return Walked::Waiting;
        }
        let variables = self.variables(now);
        let Some(definition) = self.steps.get(index).map(|step| step.definition.clone()) else {
            return Walked::Moved(None);
        };
        let mut budget = self.resolved_input_budget;
        let resolved = match runbook::resolve_inputs(&definition.inputs, &variables, &mut budget) {
            Ok(resolved) => resolved,
            Err(why) => return self.fail_step(index, now, why),
        };
        self.resolved_input_budget = budget;
        if let Some(step) = self.steps.get_mut(index) {
            step.inputs = resolved
                .iter()
                .map(|(name, value)| (name.clone(), runbook::rendered(value)))
                .collect();
            step.status = STATUS_IN_PROGRESS.to_string();
            step.started_at = Some(now);
        }
        match definition.action.as_str() {
            ACTION_BRANCH => self.run_branch(index, now, &resolved),
            ACTION_SLEEP => self.run_sleep(index, now, &definition.name, &resolved),
            instance_state::ACTION_CHANGE_INSTANCE_STATE => {
                self.run_change_instance_state(index, now, &definition.name, &resolved)
            }
            // "aws:pause … pauses the automation": the execution waits for the caller's
            // signal, which is exactly what the action does.
            ACTION_PAUSE => {
                if let Some(step) = self.steps.get_mut(index) {
                    step.status = STATUS_WAITING.to_string();
                }
                Walked::Waiting
            }
            action => self.fail_step(
                index,
                now,
                format!(
                    "The {action} action of step {} runs where this emulator does not reach, so \
                     nothing ran for this step",
                    definition.name
                ),
            ),
        }
    }

    /// Decide an `aws:branch` step's choice and go where it points.
    fn run_branch(&mut self, index: usize, now: SystemTime, inputs: &Map<String, Value>) -> Walked {
        let branch = match runbook::branch_of(inputs) {
            Ok(branch) => branch,
            Err(why) => return self.fail_step(index, now, why),
        };
        for choice in &branch.choices {
            match runbook::holds(&choice.condition, 0) {
                Ok(true) => return self.succeed_step(index, now, Some(choice.next_step.clone())),
                Ok(false) => {}
                Err(why) => return self.fail_step(index, now, why),
            }
        }
        // "If no choices match and a Default is specified, the automation goes to that
        // step"; with no default there is nowhere left to go, so the automation ends.
        match branch.default.clone() {
            Some(default) => self.succeed_step(index, now, Some(default)),
            None => {
                self.end_step(index, now, STATUS_SUCCESS);
                Walked::Moved(None)
            }
        }
    }

    /// Wait out an `aws:sleep` step's own span.
    fn run_sleep(
        &mut self,
        index: usize,
        now: SystemTime,
        name: &str,
        inputs: &Map<String, Value>,
    ) -> Walked {
        let duration = inputs.get("Duration").map(runbook::rendered);
        let timestamp = inputs.get("Timestamp").map(runbook::rendered);
        let due = match (duration.as_deref(), timestamp.as_deref()) {
            // The span the step asks to wait keeps its value: it is the wait the runbook
            // asked for, not one this emulator chose. A span no instant can be reached by
            // adding is no span to wait out, so it fails the step rather than the call.
            (Some(duration), _) if !duration.is_empty() => {
                match ::service_common::iso8601_span::iso8601_span(duration)
                    .and_then(|span| now.checked_add(span))
                {
                    Some(due) => due,
                    None => {
                        return self.fail_step(
                            index,
                            now,
                            format!(
                                "The Duration {duration:?} of step {name} is not an ISO 8601 \
                                 duration"
                            ),
                        )
                    }
                }
            }
            (_, Some(timestamp)) if !timestamp.is_empty() => {
                match ::aws_smithy_types::DateTime::from_str(
                    timestamp,
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .ok()
                .and_then(|at| SystemTime::try_from(at).ok())
                {
                    Some(at) => at,
                    None => {
                        return self.fail_step(
                            index,
                            now,
                            format!(
                                "The Timestamp {timestamp:?} of step {name} is not an ISO 8601 \
                                 timestamp"
                            ),
                        )
                    }
                }
            }
            _ => {
                return self.fail_step(
                    index,
                    now,
                    format!("Step {name} declares neither a Duration nor a Timestamp to sleep for"),
                )
            }
        };
        if let Some(step) = self.steps.get_mut(index) {
            step.due_at = Some(due);
        }
        if now < due {
            return Walked::Running;
        }
        self.succeed_step(index, now, None)
    }

    /// Record what an `aws:changeInstanceState` step asks Amazon EC2 for and leave the step
    /// running.
    ///
    /// The instance is that service's resource and the transition is its own, so nothing is
    /// decided here: the call is made off the store lock
    /// ([`instance_state::drive_due`]) and its answer comes back through
    /// [`Self::record_instance_state`]. The step's own window is the one its
    /// `DesiredStateTransitionTimeout` asks for, which is what [`Self::enter_step`] holds it
    /// to.
    fn run_change_instance_state(
        &mut self,
        index: usize,
        now: SystemTime,
        name: &str,
        inputs: &Map<String, Value>,
    ) -> Walked {
        let change = match instance_state::change_of(inputs, name) {
            Ok(change) => change,
            Err(why) => return self.fail_step(index, now, why),
        };
        let Some(due) = now.checked_add(change.transition_timeout) else {
            return self.fail_step(
                index,
                now,
                format!(
                    "The DesiredStateTransitionTimeout of step {name} names no instant this \
                     step could wait until"
                ),
            );
        };
        if let Some(step) = self.steps.get_mut(index) {
            step.due_at = Some(due);
            step.instance_state = Some(change);
        }
        Walked::Running
    }

    /// Record what Amazon EC2 answered about the step `step_execution_id`, and let the walk
    /// go on from where it stopped.
    ///
    /// The one way that service's answer reaches this execution: a step waiting on it is
    /// `InProgress` until the answer says the instances are where it asked for them
    /// ([`instance_state::Answered::Settled`]), that service refused the call
    /// ([`instance_state::Answered::Failed`], which fails the step as the action's own
    /// failure does), or the step's window runs out — and the walk decides each of those in
    /// [`Self::enter_step`], so the transition is the walk's whatever a caller polls.
    pub fn record_instance_state(
        &mut self,
        step_execution_id: &str,
        answered: instance_state::Answered,
        now: SystemTime,
    ) {
        let Some(step) = self
            .steps
            .iter_mut()
            .find(|step| step.step_execution_id == step_execution_id)
        else {
            return;
        };
        let Some(change) = step.instance_state.as_mut() else {
            return;
        };
        match answered {
            instance_state::Answered::Asked => change.asked = true,
            instance_state::Answered::Settled => {
                change.asked = true;
                change.settled = true;
            }
            instance_state::Answered::Failed(why) => change.failure = Some(why),
        }
        self.materialize_due(now);
    }

    /// End the step at `index` with `status`, closing its execution window.
    fn end_step(&mut self, index: usize, now: SystemTime, status: &str) {
        if let Some(step) = self.steps.get_mut(index) {
            step.status = status.to_string();
            step.ended_at = Some(now);
            step.due_at = None;
        }
    }

    /// The step succeeded: the walk goes to `named`, to the step the definition's
    /// `nextStep` names, or to the step declared after it.
    fn succeed_step(&mut self, index: usize, now: SystemTime, named: Option<String>) -> Walked {
        self.end_step(index, now, STATUS_SUCCESS);
        let Some(definition) = self.steps.get(index).map(|step| step.definition.clone()) else {
            return Walked::Moved(None);
        };
        if definition.is_end.unwrap_or(false) {
            return Walked::Moved(None);
        }
        let next = named.or(definition.next_step);
        self.step_after(index, next)
    }

    /// The step failed: the execution fails with it when the step is critical, and the
    /// walk does what the step's `onFailure` says.
    fn fail_step(&mut self, index: usize, now: SystemTime, why: String) -> Walked {
        self.end_step(index, now, STATUS_FAILED);
        let Some(step) = self.steps.get_mut(index) else {
            return Walked::Moved(None);
        };
        step.failure_message = Some(why.clone());
        let definition = step.definition.clone();
        if definition.critical() && self.failure_message.is_none() {
            self.failure_message = Some(why);
        }
        if definition.is_end.unwrap_or(false) {
            return Walked::Moved(None);
        }
        let on_failure = definition.on_failure();
        if let Some(named) = on_failure.strip_prefix(runbook::ON_FAILURE_STEP_PREFIX) {
            return self.step_after(index, Some(named.to_string()));
        }
        if on_failure == runbook::ON_FAILURE_CONTINUE {
            return self.step_after(index, definition.next_step.clone());
        }
        // "Abort will fail the automation when the step fails", which is the default.
        Walked::Moved(None)
    }

    /// Where the walk goes after the step at `index`: the step `named` names, or the one
    /// the runbook declares after it. A name the runbook holds no step of ends the
    /// execution with that as its failure.
    fn step_after(&mut self, index: usize, named: Option<String>) -> Walked {
        match named {
            None => Walked::Moved(Some(index.saturating_add(1)).filter(|next| *next < self.steps.len())),
            Some(name) => match self.steps.iter().position(|step| step.definition.name == name) {
                Some(next) => Walked::Moved(Some(next)),
                None => {
                    if self.failure_message.is_none() {
                        self.failure_message =
                            Some(format!("The runbook declares no step named {name}"));
                    }
                    Walked::Moved(None)
                }
            },
        }
    }

    /// The step the walk is RUNNING, which is what "the step that is currently running"
    /// (`AutomationExecution$CurrentStepName`) names: the step the walk stands at, once it
    /// has started and before it has ended. `None` for an execution nothing has picked up
    /// yet, one whose walk has left its last step, and one that runs children rather than
    /// steps of its own.
    pub fn running_step(&self) -> Option<&StepExecutionRecord> {
        self.current
            .and_then(|index| self.steps.get(index))
            .filter(|step| !is_terminal(&step.status) && step.started_at.is_some())
    }

    /// The values a step's `{{ … }}` references resolve to: this execution's parameters,
    /// and the system variables Automation defines for every execution.
    ///
    /// A parameter holding one value resolves to that value; one holding several resolves
    /// to them separated by commas, the way a `StringList` is written.
    fn variables(&self, now: SystemTime) -> BTreeMap<String, String> {
        let mut variables: BTreeMap<String, String> = self
            .resolved_parameters
            .iter()
            .map(|(name, values)| (name.clone(), values.join(",")))
            .collect();
        variables.insert(
            "automation:EXECUTION_ID".to_string(),
            self.execution_id.clone(),
        );
        variables.insert(
            "global:ACCOUNT_ID".to_string(),
            ::engine::emulator_account_id().to_string(),
        );
        variables.insert("global:REGION".to_string(), self.region.clone());
        if let Ok(stamped) = ::aws_smithy_types::DateTime::from(now)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
        {
            if let Some(date) = stamped.get(..10) {
                variables.insert("global:DATE".to_string(), date.to_string());
            }
            variables.insert("global:DATE_TIME".to_string(), stamped);
        }
        variables
    }

    /// The modelled `AutomationExecution` for this execution: every member either echoed
    /// from the start as it was received, resolved by the start, or written by the walk. A
    /// member nothing has produced is left unset rather than invented.
    pub fn to_automation_execution(&self) -> AutomationExecution {
        let running = self.running_step();
        AutomationExecution {
            automation_execution_id: Some(self.execution_id.clone()),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            execution_start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            execution_end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            automation_execution_status: Some(self.status.clone()),
            step_executions: (!self.steps.is_empty()).then(|| {
                self.steps
                    .iter()
                    .map(StepExecutionRecord::to_step_execution)
                    .collect()
            }),
            // Every step this execution holds is reported, so the list is never the
            // truncated one `DescribeAutomationStepExecutions` exists for.
            step_executions_truncated: false,
            parameters: (!self.supplied_parameters.is_empty()).then(|| {
                self.supplied_parameters
                    .iter()
                    .map(|(name, values)| (name.clone(), values.clone()))
                    .collect()
            }),
            // "The list of execution outputs as defined in the Automation runbook": an
            // output is a value a step produced, and the actions this emulator runs
            // produce none.
            outputs: None,
            failure_message: self.failure_message.clone(),
            warning_message: None,
            mode: Some(self.mode.clone()),
            parent_automation_execution_id: self.parent_execution_id.clone(),
            executed_by: Some(self.executed_by.clone()),
            current_step_name: running.map(|step| step.definition.name.clone()),
            current_action: running.map(|step| step.definition.action.clone()),
            target_parameter_name: self.target_parameter_name.clone(),
            targets: (!self.targets.is_empty()).then(|| self.targets.clone()),
            target_maps: (!self.target_maps.is_empty()).then(|| self.target_maps.clone()),
            resolved_targets: (!self.resolved_target_values.is_empty()).then(|| ResolvedTargets {
                parameter_values: Some(self.resolved_target_values.clone()),
                truncated: false,
            }),
            max_concurrency: Some(self.max_concurrency.clone()),
            max_errors: Some(self.max_errors.clone()),
            target: self.target.clone(),
            target_locations: (!self.target_locations.is_empty())
                .then(|| self.target_locations.clone()),
            // "An aggregate of step execution statuses displayed … for a multi-Region and
            // multi-account Automation execution", which this execution is not.
            progress_counters: None,
            alarm_configuration: self.alarm_configuration.clone(),
            // No alarm has been evaluated for this execution, so none was invoked.
            triggered_alarms: None,
            target_locations_url: self.target_locations_url.clone(),
            // The Change Manager members: an automation started through this operation is
            // not a change request, so it carries no subtype, schedule, runbook list,
            // OpsItem, association or change request name.
            automation_subtype: None,
            scheduled_time: None,
            runbooks: None,
            ops_item_id: None,
            association_id: None,
            change_request_name: None,
            // "Variables defined for the automation": a runbook declares none through this
            // operation's members.
            variables: None,
        }
    }
}

/// How many of `total` a `MaxConcurrency` or `MaxErrors` value names: "You can specify
/// either an absolute number … or a percentage of the target set, for example 10%".
///
/// A percentage is taken of the target set and floored, and a value that names fewer than
/// nothing names nothing: `MaxErrors` of `0` means "stop after the first error result"
/// while `MaxConcurrency` is held to at least one target at a time by its own pattern. A
/// value that is neither — which the modelled patterns already refuse before a handler
/// sees it — names ONE target, the most conservative reading of both members, rather than
/// every target.
pub fn rate_count(value: &str, total: usize) -> usize {
    match value.strip_suffix('%') {
        Some(percentage) => percentage
            .parse::<usize>()
            .map_or(1, |percentage| total.saturating_mul(percentage) / 100),
        None => value.parse::<usize>().unwrap_or(1),
    }
}

/// What an idempotency token was used WITH: the members of the start that have to match
/// for a retry carrying the same `ClientToken` to be a retry of the SAME call.
///
/// "Error returned when an idempotent operation is retried and the parameters don't match
/// the original call to the API with the same idempotency token"
/// (`IdempotentParameterMismatch`), so these are what the caller asked for — the runbook,
/// the version, the parameters, the mode, the targets and the rate — and nothing the
/// service resolved for it.
pub struct StartedWith<'a> {
    pub document_name: &'a str,
    pub document_version: &'a str,
    pub parameters: &'a BTreeMap<String, Vec<String>>,
    pub mode: &'a str,
    pub target_parameter_name: Option<&'a str>,
    pub targets: &'a [Target],
    pub target_maps: &'a [::std::collections::HashMap<String, Vec<String>>],
    pub target_locations: &'a [TargetLocation],
    pub max_concurrency: &'a str,
    pub max_errors: &'a str,
}

/// The fingerprint of what a start asked for ([`StartedWith`]), which is what a later
/// start carrying the same `ClientToken` is compared against.
///
/// The separators are the ASCII unit and record separators, which no modelled member of a
/// start can carry, so two different starts cannot render alike.
pub fn request_fingerprint(started_with: &StartedWith<'_>) -> String {
    use ::std::fmt::Write;

    let StartedWith {
        document_name,
        document_version,
        parameters,
        mode,
        target_parameter_name,
        targets,
        target_maps,
        target_locations,
        max_concurrency,
        max_errors,
    } = started_with;
    let mut rendered = String::new();
    let _ = write!(
        rendered,
        "{document_name}\u{1f}{document_version}\u{1f}{mode}\u{1f}{max_concurrency}\u{1f}\
         {max_errors}\u{1f}{}",
        target_parameter_name.unwrap_or_default()
    );
    for (name, values) in *parameters {
        let _ = write!(rendered, "\u{1f}{name}={}", values.join("\u{1e}"));
    }
    for target in *targets {
        let _ = write!(
            rendered,
            "\u{1f}{}={}",
            target.key.clone().unwrap_or_default(),
            target.values.clone().unwrap_or_default().join("\u{1e}")
        );
    }
    // A target map and a target location carry several members each, so each is rendered
    // member by member — a map in key order, so two starts that named the same mapping
    // render alike whatever order it arrived in.
    for map in *target_maps {
        for (key, values) in map.iter().collect::<BTreeMap<&String, &Vec<String>>>() {
            let _ = write!(rendered, "\u{1f}{key}={}", values.join("\u{1e}"));
        }
    }
    for location in *target_locations {
        let _ = write!(
            rendered,
            "\u{1f}{}\u{1e}{}\u{1e}{}\u{1e}{}\u{1e}{}",
            location.accounts.clone().unwrap_or_default().join("\u{1d}"),
            location.regions.clone().unwrap_or_default().join("\u{1d}"),
            location
                .target_location_max_concurrency
                .clone()
                .unwrap_or_default(),
            location
                .target_location_max_errors
                .clone()
                .unwrap_or_default(),
            location.execution_role_name.clone().unwrap_or_default()
        );
    }
    rendered
}

impl ComAmazonawsSsmAmazonssmState {
    /// The automation execution `execution_id` names in `region`, or `None` when that
    /// Region holds none.
    pub fn automation_execution(
        &self,
        region: &str,
        execution_id: &str,
    ) -> Option<&AutomationExecutionRecord> {
        self.automation_executions
            .get(&(region.to_string(), execution_id.to_string()))
    }

    /// The same execution, to write it — its tags, say.
    pub fn automation_execution_mut(
        &mut self,
        region: &str,
        execution_id: &str,
    ) -> Option<&mut AutomationExecutionRecord> {
        self.automation_executions
            .get_mut(&(region.to_string(), execution_id.to_string()))
    }

    /// The execution a `ClientToken` already started in `region`, or `None` when that
    /// token has started none: "User-provided idempotency token … is case insensitive …
    /// and can't be reused", so the token names at most one execution whatever case it is
    /// spelled in.
    pub fn automation_execution_with_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&AutomationExecutionRecord> {
        self.automation_executions
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, execution)| execution)
            .find(|execution| {
                execution
                    .client_token
                    .as_deref()
                    .is_some_and(|held| held.eq_ignore_ascii_case(client_token))
            })
    }

    /// How many automation executions `region` is running: what the concurrent-automation
    /// quota ([`MAX_CONCURRENT_EXECUTIONS`]) is measured against.
    pub fn running_automation_executions(&self, region: &str) -> usize {
        self.automation_executions
            .iter()
            .filter(|((held, _), _)| held == region)
            .filter(|(_, execution)| !execution.ended())
            .count()
    }

    /// Store `record` as `region`'s automation execution of that id.
    ///
    /// Past [`MAX_EXECUTIONS_RETAINED`] the oldest execution is forgotten, so this store
    /// holds a bounded number of records however many starts arrive.
    pub fn insert_automation_execution(&mut self, record: AutomationExecutionRecord) {
        let key = (record.region.clone(), record.execution_id.clone());
        self.automation_executions.insert(key, record);
        while self.automation_executions.len() > MAX_EXECUTIONS_RETAINED {
            let Some(oldest) = self
                .automation_executions
                .iter()
                .min_by_key(|(_, execution)| execution.started_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.automation_executions.remove(&oldest);
        }
    }

    /// Mint the id of an automation execution, or of one of its step executions.
    ///
    /// "The unique ID of a newly scheduled automation execution"
    /// (`StartAutomationExecutionResult$AutomationExecutionId`), modelled exactly 36
    /// characters long — the hyphenated UUID form, minted through
    /// [`super::minted_uuid`] from this store's own creation instant and its mint counter,
    /// so no two executions of one run collide and the value is not a count a caller can
    /// read off the wire.
    pub fn mint_automation_id(&mut self) -> String {
        self.minted_automation_ids += 1;
        let since_epoch = self
            .created_at
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default();
        super::minted_uuid(since_epoch.as_nanos(), self.minted_automation_ids)
    }

    /// Materialise everything the clock owes every automation execution at `now`: each
    /// execution's own walk, and then the rate control a parent exercises over its
    /// children.
    pub fn advance_automations(&mut self, now: SystemTime) {
        for execution in self.automation_executions.values_mut() {
            execution.materialize_due(now);
        }
        let parents: Vec<(String, String)> = self
            .automation_executions
            .iter()
            .filter(|(_, execution)| !execution.children.is_empty() && !execution.ended())
            .map(|(key, _)| key.clone())
            .collect();
        for key in parents {
            self.advance_rate_control(&key, now);
        }
    }

    /// Release the children of one rate-controlled execution that its `MaxConcurrency`
    /// allows to start, cancel the rest once `MaxErrors` has been reached, and carry the
    /// children's outcome up to the parent.
    fn advance_rate_control(&mut self, key: &(String, String), now: SystemTime) {
        let Some(parent) = self.automation_executions.get(key) else {
            return;
        };
        if parent.status == STATUS_PENDING {
            return;
        }
        let region = parent.region.clone();
        let children = parent.children.clone();
        let concurrency = rate_count(&parent.max_concurrency, children.len()).max(1);
        // "If you specify 3, for example, the system stops running the automation when the
        // fourth error is received", so the release stops once MORE than that many
        // children have failed.
        let errors_allowed = rate_count(&parent.max_errors, children.len());

        let mut failed = 0usize;
        let mut in_flight = 0usize;
        let mut queued: Vec<String> = Vec::new();
        for child_id in &children {
            let Some(child) = self.automation_execution(&region, child_id) else {
                continue;
            };
            if child.ended() {
                if child.status != STATUS_SUCCESS {
                    failed += 1;
                }
            } else if child.runnable_at.is_none() {
                queued.push(child_id.clone());
            } else {
                in_flight += 1;
            }
        }
        let mut releasable = concurrency.saturating_sub(in_flight);
        // "the system stops running the automation on additional targets": once MORE than
        // `MaxErrors` children have failed, the targets that have not started are stopped
        // rather than left waiting; while that threshold is not reached, a target waits
        // its turn under `MaxConcurrency`.
        for child_id in queued {
            let stopped = failed > errors_allowed;
            if !stopped && releasable == 0 {
                continue;
            }
            let Some(child) = self.automation_execution_mut(&region, &child_id) else {
                continue;
            };
            if stopped {
                child.status = STATUS_CANCELLED.to_string();
                child.ended_at = Some(now);
                continue;
            }
            // Released now, and walked now: the release is a transition the clock already
            // owed, so it does not wait for another call.
            child.runnable_at = Some(now);
            child.materialize_due(now);
            releasable -= 1;
            if child.ended() && child.status != STATUS_SUCCESS {
                failed += 1;
            }
        }

        let outstanding = children
            .iter()
            .filter_map(|child_id| self.automation_execution(&region, child_id))
            .filter(|child| !child.ended())
            .count();
        let failed = children
            .iter()
            .filter_map(|child_id| self.automation_execution(&region, child_id))
            .filter(|child| child.ended() && child.status != STATUS_SUCCESS)
            .count();
        if outstanding > 0 {
            return;
        }
        let Some(parent) = self.automation_executions.get_mut(key) else {
            return;
        };
        // Every target's execution has ended, so the parent's own status is their
        // aggregate. The children carry their own failure messages; the parent reports the
        // status they add up to.
        parent.status = if failed > 0 {
            STATUS_FAILED.to_string()
        } else {
            STATUS_SUCCESS.to_string()
        };
        parent.ended_at = Some(now);
    }

    /// The values a rate-controlled start's targets resolve to, or `Err(key)` when the
    /// target's key is not one an automation targets by.
    ///
    /// `Key=ParameterValues` carries the values themselves, so they resolve to exactly
    /// what the start named. Every other key is a criterion over the managed nodes, which
    /// resolves through the one node matcher this service has
    /// ([`super::commands::nodes_matching_target`]) — so a criterion that describes no
    /// registered node resolves to nothing.
    pub fn automation_target_values(
        &self,
        region: &str,
        target: &Target,
    ) -> Result<Vec<String>, String> {
        if target.key.as_deref() == Some(TARGET_KEY_PARAMETER_VALUES) {
            return Ok(target.values.clone().unwrap_or_default());
        }
        let mut resolved: Vec<String> = Vec::new();
        for id in super::commands::nodes_matching_target(self, region, target)? {
            if !resolved.iter().any(|held| held == id) {
                resolved.push(id.to_string());
            }
        }
        Ok(resolved)
    }
}

/// The parameter values an execution runs with: the ones the start supplied, plus the
/// default of every parameter the runbook declares that the start named no value for.
///
/// "A key-value map of execution parameters, which match the declared parameters in the
/// Automation runbook" is what the start supplies; a declared parameter the start left out
/// runs at the default the runbook declares for it, which is what makes that parameter
/// optional in the first place.
pub fn resolved_parameters(
    declared: &[crate::ops::types::DocumentParameter],
    supplied: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    let mut resolved = supplied.clone();
    for parameter in declared {
        let (Some(name), Some(default)) = (parameter.name.as_deref(), &parameter.default_value)
        else {
            continue;
        };
        resolved
            .entry(name.to_string())
            .or_insert_with(|| vec![default.clone()]);
    }
    resolved
}

/// How an operation that runs or previews an Automation runbook words the miss of a
/// runbook name: a name no document of this Region carries, and a document of that name
/// that is no Automation runbook, are the same miss — there is no Automation runbook of
/// that name to run.
///
/// Worded here rather than in either handler so the two operations that resolve a runbook
/// (`StartAutomationExecution`, `StartExecutionPreview`) cannot word one refusal two ways.
pub fn runbook_not_found_message(document_name: &str) -> String {
    format!("Document {document_name} does not exist.")
}

/// The same, for a version selector the resolved runbook holds no version for.
pub fn runbook_version_not_found_message(document_name: &str, version: &str) -> String {
    format!("Document {document_name} version {version} does not exist.")
}

/// The same, for the step names a runbook's flow members name that it does not declare
/// ([`unknown_step_references`]).
pub fn unknown_step_reference_message(unknown: &[String]) -> String {
    format!("The runbook declares no step named {}", unknown.join(", "))
}

/// The same, for a target `Key` that is not one an automation resolves its targets by: the
/// key the request sent, then the keys an automation does resolve by — the values form and
/// the node criteria ([`super::commands::supported_target_keys`]).
pub fn unresolvable_target_key_message(key: &str) -> String {
    format!(
        "Target key '{key}' isn't one of the target keys an automation resolves its targets \
         by: {TARGET_KEY_PARAMETER_VALUES}, {}",
        super::commands::supported_target_keys().join(", ")
    )
}

/// Why the start's `TargetParameterName`, `Targets` and `TargetMaps` cannot hold together,
/// or `None` when they can.
///
/// "Required if you specify targets" (`TargetParameterName`) and "Required if you specify
/// TargetParameterName" (`Targets`) make the pair all-or-nothing, "Both Targets and
/// TargetMaps can't be specified together" (`TargetMaps`) makes the two exclusive, and a
/// target parameter is one of the runbook's own parameters — it is the parameter each
/// target's value is bound to, so a name the runbook does not declare names nothing to
/// bind.
pub fn target_rejection(
    document_name: &str,
    declared: &[crate::ops::types::DocumentParameter],
    target_parameter_name: Option<&str>,
    targets: &[Target],
    target_maps: usize,
) -> Option<String> {
    match (target_parameter_name, targets.is_empty()) {
        (Some(name), true) => {
            return Some(format!(
                "Targets must be specified with TargetParameterName {name}"
            ))
        }
        (None, false) => {
            return Some("TargetParameterName must be specified with Targets".to_string())
        }
        _ => {}
    }
    if !targets.is_empty() && target_maps > 0 {
        return Some("Targets and TargetMaps can't both be specified".to_string());
    }
    let name = target_parameter_name?;
    let declares = declared
        .iter()
        .any(|parameter| parameter.name.as_deref() == Some(name));
    (!declares).then(|| {
        format!("Document {document_name} doesn't support a parameter named {name}")
    })
}

/// The step names a runbook's flow members name that the runbook does not declare.
///
/// `nextStep` and the `step:` form of `onFailure` both name a step by name, and a name no
/// step carries is a flow the walk could not follow — reported before the execution is
/// persisted rather than discovered halfway through the walk.
pub fn unknown_step_references(steps: &[runbook::RunbookStep]) -> Vec<String> {
    let declared: BTreeSet<&str> = steps.iter().map(|step| step.name.as_str()).collect();
    let mut unknown: Vec<String> = Vec::new();
    let mut note = |named: &str| {
        if !declared.contains(named) && !unknown.iter().any(|held| held == named) {
            unknown.push(named.to_string());
        }
    };
    for step in steps {
        if let Some(next) = step.next_step.as_deref() {
            note(next);
        }
        if let Some(named) = step
            .on_failure()
            .strip_prefix(runbook::ON_FAILURE_STEP_PREFIX)
        {
            note(named);
        }
        if step.action == ACTION_BRANCH {
            if let Ok(branch) = runbook::branch_of(&step.inputs) {
                for choice in &branch.choices {
                    note(&choice.next_step);
                }
                if let Some(default) = branch.default.as_deref() {
                    note(default);
                }
            }
        }
    }
    unknown
}
