// SPDX-License-Identifier: CC0-1.0

//! The FLOW EXECUTION this plane owns: the asynchronous run of a flow a caller starts, and
//! the status machine `GetFlowExecution` reports and `StopFlowExecution` moves.
//!
//! A flow, its versions and its aliases belong to the Bedrock Agent build-time plane
//! ([`super::flow_invocation`]); the EXECUTION is this plane's own resource, so this module
//! holds what one is and how it is addressed:
//!
//! * an execution is identified inside the flow alias it runs on, by the
//!   `flowExecutionName` the caller supplied or the one this service minted for it, so the
//!   store is keyed by the triple ([`FlowExecutionKey`]) and the same name on two aliases is
//!   two executions;
//! * `executionIdentifier` admits either that name or the execution's whole ARN
//!   (`smithy_query(query="operation",operation="GetFlowExecution")` -> the member's
//!   pattern), so every execution-scoped operation resolves its label through the ONE helper
//!   [`execution_name_from_identifier`] and they cannot disagree about what an identifier
//!   names;
//! * the refusals the lookup and the status machine produce are worded here too
//!   ([`execution_not_found`], [`execution_name_in_use`], [`execution_not_running`]), for the
//!   reason the parent module states: one sentence per rule rather than one per operation.
//!
//! ## The status the clock owes
//!
//! A started execution is `Running`, and the run the start computed is due to finish
//! [`FLOW_EXECUTION_SPAN`] later: `materialise_due_transitions` is what moves it to the
//! status that run came to ([`FlowExecution::completes_as`]), a write owed at that instant
//! whether or not anybody calls, so a REPORTING operation may make it. The span is never
//! zero, so a caller acting straight after the start still sees the execution `Running` and
//! can stop it.
//!
//! A run whose path leaves the flow's input node for a node kind nothing in this emulator
//! performs the work of comes to `Failed`, carrying the sentence saying which node it could
//! compute no value for: NOTHING RAN THAT NODE, so the execution reports the failure that
//! is, and never a `Succeeded` with a value nothing produced.
//!
//! `TimedOut` is modelled and understood here, but no code path mints it: the 24 hours an
//! execution may run for are longer than any run here stays `Running`, so there is no
//! instant at which the clock owes it.

use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{ConflictException, ResourceNotFoundException};

use super::ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState as ServiceState;

/// How long after the start the run the start computed is due to finish, which is when the
/// clock owes `Running` -> `Succeeded`.
///
/// The span this emulator gives a transition its clock owes, as the flow plane's own
/// preparation takes (`agent_state::FLOW_PREPARATION_SPAN`). It is NOT the 24 hours the
/// model documents an execution times out after: that is the bound on a run nothing here
/// keeps running, not the length of one.
pub const FLOW_EXECUTION_SPAN: Duration = Duration::from_secs(3);

/// The most executions this emulator holds at once, over every flow alias.
///
/// A resource guard, not an AWS quota: the store has no TTL and nothing removes an
/// execution, so this is where unbounded growth is refused (with the modelled
/// `ServiceQuotaExceededException`) rather than allocated. It matches the bound this
/// service's other growable collections take (`super::MAX_SESSIONS`).
pub const MAX_FLOW_EXECUTIONS: usize = 1_000;

/// The status of one execution, as the modelled `FlowExecutionStatus` declares it
/// (`smithy_query(query="shape",shape="FlowExecutionStatus")` -> Running, Succeeded,
/// Failed, TimedOut, Aborted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowExecutionStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Aborted,
}

impl FlowExecutionStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::TimedOut => "TimedOut",
            Self::Aborted => "Aborted",
        }
    }

    /// Whether the execution has finished: a terminal execution never moves again, which is
    /// what a stop of one is refused against.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// What identifies one execution: the flow and the alias it runs on, and its own name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlowExecutionKey {
    pub flow_id: String,
    pub alias_id: String,
    pub name: String,
}

/// One started execution of a flow.
#[derive(Debug)]
pub struct FlowExecution {
    /// The `flowExecutionName` this execution is identified by inside its alias: the one
    /// the caller supplied, else the one this service minted for it.
    pub name: String,
    /// The ARN [`execution_arn`] minted for it, which is what the start reported and every
    /// read hands back.
    pub execution_arn: String,
    /// The flow this execution runs, as the build-time plane identifies it.
    pub flow_id: String,
    /// The alias of that flow the execution was started on.
    pub alias_id: String,
    /// The version the alias routed to AT THE START, so an alias repointed afterwards does
    /// not change what this execution ran.
    pub flow_version: String,
    /// The flow metadata and definition that version packaged AT THE START
    /// ([`super::flow_snapshot::FlowSnapshot`]), which is what
    /// `GetExecutionFlowSnapshot` reports: a flow edited after the start does not move it,
    /// because the copy was taken when the start resolved the run.
    pub snapshot: super::flow_snapshot::FlowSnapshot,
    pub started_at: DateTime,
    /// The instant the run is due to finish, stored rather than waited for: see the module
    /// docs.
    pub completes_at: SystemTime,
    /// The status the run the start computed came to, which is the status this execution
    /// reaches at [`FlowExecution::completes_at`]: `Succeeded` for a run that delivered the
    /// flow's outputs, `Failed` for one whose path reached a node nothing here computes a
    /// value for.
    pub completes_as: FlowExecutionStatus,
    /// The sentence saying what the run could compute no value for, set for a run that comes
    /// to `Failed` and only then. It is what the read of a failed execution reports as the
    /// error of its run.
    pub failure: Option<String>,
    /// The trace of the run the start made: where its input entered the flow, which field of
    /// which node each value left by and arrived at, and the node a failed run is about.
    /// `ListFlowExecutionEvents` composes the events of this execution from it
    /// ([`super::flow_execution_event`]), so every event it reports is a step this run really
    /// took through the stored flow.
    pub trace: super::flow_invocation::FlowRunTrace,
    pub status: FlowExecutionStatus,
    /// Set when the execution reached a terminal status, and only then — the member is
    /// "only populated when the execution has completed, failed, timed out, or been
    /// aborted" (`smithy_query(query="doc",shape="GetFlowExecutionResponse",member="endedAt")`).
    pub ended_at: Option<DateTime>,
    /// Position in this service's global store order, as [`super::InvocationStepRecord::order`].
    pub order: u64,
}

/// Move every execution whose run the clock has already finished to the status that run came
/// to.
///
/// Owed at [`FlowExecution::completes_at`] whether or not anybody calls, so every operation
/// that reads or writes this store materialises it first and no caller can observe an
/// execution still `Running` past its own due instant. `endedAt` is that due instant and not
/// the instant of the call that noticed it, so two readers report the same end.
pub fn materialise_due_transitions(store: &mut ServiceState, now: SystemTime) {
    for execution in store.flow_executions.values_mut() {
        if execution.status == FlowExecutionStatus::Running && execution.completes_at <= now {
            execution.status = execution.completes_as;
            execution.ended_at = Some(DateTime::from(execution.completes_at));
        }
    }
}

/// The ARN of the execution `name` of `alias_id` on `flow_id`, in `region`.
///
/// The form is the one the modelled `FlowExecutionIdentifier` spells
/// (`arn:aws:bedrock:<region>:<account>:flow/<flowId>/alias/<aliasId>/execution/<name>`),
/// built through the ONE ARN constructor this tree has so the partition and the account are
/// the ones this emulator mints everywhere.
pub fn execution_arn(region: &str, flow_id: &str, alias_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_NAMESPACE,
        region,
        &format!("flow/{flow_id}/alias/{alias_id}/execution/{name}"),
    )
}

/// The `flowExecutionName` an `executionIdentifier` names on the alias `alias_id` of
/// `flow_id`, or `None` when it names an execution of some other place.
///
/// The modelled shape admits either the bare name or the execution's whole ARN, and both
/// forms name the same execution, so both select the same stored record. An ARN selects one
/// only when it is one this emulator could itself have minted — the `bedrock` namespace, the
/// canonical emulator account, and the flow and alias the request addressed — so an ARN of
/// another account, another service or another alias names no execution here and its caller
/// is answered not-found. The region segment is NOT checked, for the reason
/// [`super::session_id_from_identifier`] states: one process serves every region from one
/// store.
pub fn execution_name_from_identifier<'a>(
    identifier: &'a str,
    flow_id: &str,
    alias_id: &str,
) -> Option<&'a str> {
    if !identifier.starts_with("arn:") {
        return Some(identifier);
    }
    let named = ::service_common::arn::parse(identifier)?;
    if named.vendor != super::ARN_NAMESPACE || named.account != ::engine::EMULATOR_ACCOUNT_ID {
        return None;
    }
    named
        .resource
        .strip_prefix(&format!("flow/{flow_id}/alias/{alias_id}/execution/"))
        .filter(|name| !name.is_empty())
}

/// The modelled `ResourceNotFoundException` (HTTP 404) an execution-scoped operation answers
/// when its `executionIdentifier` names no execution of the alias it addressed.
///
/// Fixed text for this class, as AWS words it, stated through the ONE frame this plane's flow
/// surface words every unresolved resource with ([`super::flow_resource_not_found`]): the
/// sentence names the TYPE of resource that could not be resolved and nothing else — neither
/// the identifier the caller submitted nor the flow or alias it was addressed through, either
/// of which may be the reason nothing resolved.
pub fn execution_not_found() -> ResourceNotFoundException {
    super::flow_resource_not_found(EXECUTION_RESOURCE_TYPE)
}

/// The resource type [`execution_not_found`] names, spelled as the model spells the resource
/// a flow execution is (`FlowExecution`).
const EXECUTION_RESOURCE_TYPE: &str = "FlowExecution";

/// The modelled `ConflictException` (HTTP 409) a start answers when the alias already holds
/// an execution of that name: a `flowExecutionName` is "the unique name for the flow
/// execution" (`smithy_query(query="doc",shape="StartFlowExecutionRequest",member="flowExecutionName")`),
/// so a second start under a name in use conflicts with the execution holding it rather than
/// displacing it.
///
/// The variable part is the name the request carried, which is the one thing the caller has
/// to change to be served.
pub fn execution_name_in_use(name: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Flow execution {name} already exists and cannot be started again."
        )),
    }
}

/// The modelled `ConflictException` (HTTP 409) a stop answers for an execution that has
/// already finished: a terminal execution never moves again, so there is no run left to
/// stop.
///
/// Fixed text for this class, as AWS words it, with two variable parts read from the
/// execution itself: the name it is identified by and the terminal status it already stands
/// in — which is what tells the caller its stop arrived too late rather than being wrong.
pub fn execution_not_running(name: &str, status: FlowExecutionStatus) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Flow execution {name} is in {} status and cannot be aborted.",
            status.as_wire()
        )),
    }
}
