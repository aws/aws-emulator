// SPDX-License-Identifier: CC0-1.0

//! The sessions a worker runs, and the session actions each one hands it.
//!
//! A session is the service's record of ONE worker working on ONE job: it hangs off that
//! job exactly as the model's paths nest it
//! (`/2023-10-12/farms/{farmId}/queues/{queueId}/jobs/{jobId}/sessions/{sessionId}`), and
//! the actions it holds hang off it in the order the worker is to run them. The ids follow
//! the nesting the modeled label patterns spell: a session is `session-` plus 32 hex
//! characters, and its actions are that same hex with an index appended
//! (`^sessionaction-[0-9a-f]{32}-(0|([1-9][0-9]{0,9}))$`).
//!
//! Only the scheduler creates a session (see `super::scheduling`), and only the worker the
//! session was created for reports what happened to its actions.

use std::time::SystemTime;

use crate::ops::types::{
    AssignedEnvironmentEnterSessionActionDefinition,
    AssignedEnvironmentExitSessionActionDefinition, AssignedSession, AssignedSessionAction,
    AssignedSessionActionDefinition, AssignedSyncInputJobAttachmentsSessionActionDefinition,
    AssignedTaskRunSessionActionDefinition, EnvironmentEnterSessionActionDefinition,
    EnvironmentExitSessionActionDefinition, LogConfiguration, SessionActionDefinition,
    SyncInputJobAttachmentsSessionActionDefinition, TaskParameterValue,
    TaskRunSessionActionDefinition,
};

/// `resourceType` this service reports for a session in `ResourceNotFoundException`.
///
/// Spelled the way the API path spells that resource, like every other type this service
/// refuses — the segment `/sessions/` names it `session`.
pub const RESOURCE_TYPE_SESSION: &str = "session";

/// `resourceType` this service reports for a session action in `ResourceNotFoundException`.
///
/// The segment `/session-actions/` names it `session-action`.
pub const RESOURCE_TYPE_SESSION_ACTION: &str = "session-action";

/// `SessionLifecycleStatus` a session carries while the worker is working through it.
///
/// From `smithy_query(query="shape", shape="SessionLifecycleStatus")`: `STARTED`,
/// `UPDATE_IN_PROGRESS`, `UPDATE_SUCCEEDED`, `UPDATE_FAILED`, `ENDED`. A session starts the
/// moment the scheduler assigns it, so that is the status it is created in.
pub const SESSION_LIFECYCLE_STARTED: &str = "STARTED";

/// `SessionLifecycleStatus` a session carries once every action it held is finished and the
/// environments it entered have been exited. Also the only `SessionLifecycleTargetStatus`
/// the model spells.
pub const SESSION_LIFECYCLE_ENDED: &str = "ENDED";

/// `SessionActionStatus` an action carries from the moment it is handed to its worker until
/// that worker reports it started.
///
/// From `smithy_query(query="shape", shape="SessionActionStatus")`: `ASSIGNED`, `RUNNING`,
/// `CANCELING`, `SUCCEEDED`, `FAILED`, `INTERRUPTED`, `CANCELED`, `NEVER_ATTEMPTED`,
/// `SCHEDULED`, `RECLAIMING`, `RECLAIMED`.
pub const SESSION_ACTION_STATUS_ASSIGNED: &str = "ASSIGNED";

/// `SessionActionStatus` an action its worker has reported itself working on carries.
pub const SESSION_ACTION_STATUS_RUNNING: &str = "RUNNING";

/// The `CompletedStatus` a worker reports for an action that ran to completion.
///
/// From `smithy_query(query="shape", shape="CompletedStatus")`: `SUCCEEDED`, `FAILED`,
/// `INTERRUPTED`, `CANCELED`, `NEVER_ATTEMPTED`. Every one of them is also a
/// `SessionActionStatus`, so what the worker reports is what the action then carries.
pub const COMPLETED_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// The `CompletedStatus` of an action whose work failed.
pub const COMPLETED_STATUS_FAILED: &str = "FAILED";
/// The `CompletedStatus` of an action the worker was interrupted part way through.
pub const COMPLETED_STATUS_INTERRUPTED: &str = "INTERRUPTED";
/// The `CompletedStatus` of an action that was cancelled.
pub const COMPLETED_STATUS_CANCELED: &str = "CANCELED";
/// The `CompletedStatus` of an action the worker never started.
pub const COMPLETED_STATUS_NEVER_ATTEMPTED: &str = "NEVER_ATTEMPTED";

/// Whether `status` is one of the `CompletedStatus` values a worker may report.
pub fn is_completed_status(status: &str) -> bool {
    matches!(
        status,
        COMPLETED_STATUS_SUCCEEDED
            | COMPLETED_STATUS_FAILED
            | COMPLETED_STATUS_INTERRUPTED
            | COMPLETED_STATUS_CANCELED
            | COMPLETED_STATUS_NEVER_ATTEMPTED
    )
}

/// The `logDriver` a session's log configuration names.
///
/// The service's own driver for a session's logs; the options beside it name the group and
/// stream the service derives from the ids of the queue the session runs for and the session
/// itself. See [`Session::log_configuration`].
pub const SESSION_LOG_DRIVER: &str = "awslogs";

/// How many actions one session holds at once.
///
/// A session grows by one action per task its worker runs in it, and a task a worker keeps
/// reporting failed is run again while the job allows a retry — a count the request chooses
/// (`maxRetriesPerTask, range 0..2147483647`). So the actions one session holds are bounded
/// here: a session that has held this many is ended rather than handed another task, and the
/// worker takes a fresh one. Far above the actions a render session runs.
pub const MAX_SESSION_ACTIONS: usize = 1024;

/// How many sessions one job holds at once.
///
/// The bound beneath [`MAX_SESSION_ACTIONS`]: together they cap what the scheduler can store
/// for one job however many times its tasks are run, the way every other level of this store
/// carries a bound of its own (see [`super::MAX_FARMS`]). A job that holds this many sessions
/// is scheduled onto no further worker.
pub const MAX_SESSIONS_PER_JOB: usize = 256;

/// The `EnvironmentId` scope of an environment a whole job runs under: the queue's own
/// environments and the ones its template declares.
///
/// From `smithy_query(query="shape", shape="EnvironmentId")`: the id is
/// `JOB:<jobId>:<name>` or `STEP:<stepId>:<name>`, `len 1..1024`.
pub const ENVIRONMENT_SCOPE_JOB: &str = "JOB";

/// The `EnvironmentId` scope of an environment only one step runs under.
pub const ENVIRONMENT_SCOPE_STEP: &str = "STEP";

/// The longest an `EnvironmentId` may be: the shape is `len 1..1024`.
const MAX_ENVIRONMENT_ID: usize = 1024;

/// The id a session action names one environment by, or `None` for a name that id cannot
/// hold.
///
/// The name is the environment's own, so the id is only as valid as the name: the pattern's
/// trailing `.*` matches no line break, and the whole id is bounded, so a name that is
/// empty, carries one, or is too long for the id is a name no response could name.
pub fn environment_id(scope: &str, owner_id: &str, name: &str) -> Option<String> {
    if name.is_empty() || name.contains('\n') || name.contains('\r') {
        return None;
    }
    let id = format!("{scope}:{owner_id}:{name}");
    (id.len() <= MAX_ENVIRONMENT_ID).then_some(id)
}

/// What one session action tells its worker to do.
///
/// The four kinds are the members of `AssignedSessionActionDefinition`
/// (`smithy_query(query="shape", shape="AssignedSessionActionDefinition")`), in the order a
/// session runs them: the job's attachments are synchronised, the environments are entered,
/// the task runs, and the environments are exited.
#[derive(Debug, Clone)]
pub enum SessionActionKind {
    /// Synchronise the input job attachments, for the whole job or for one step.
    SyncInputJobAttachments { step_id: Option<String> },
    /// Enter the environment the id names.
    EnvironmentEnter { environment_id: String },
    /// Run one task of a step, with the parameters that task carries.
    TaskRun {
        step_id: String,
        task_id: String,
        parameters: ::std::collections::HashMap<String, TaskParameterValue>,
    },
    /// Exit the environment the id names.
    EnvironmentExit { environment_id: String },
}

impl SessionActionKind {
    /// This action as the response hands it to the worker.
    fn assigned(&self) -> AssignedSessionActionDefinition {
        match self {
            Self::SyncInputJobAttachments { step_id } => {
                AssignedSessionActionDefinition::syncInputJobAttachments(
                    AssignedSyncInputJobAttachmentsSessionActionDefinition {
                        step_id: step_id.clone(),
                    },
                )
            }
            Self::EnvironmentEnter { environment_id } => {
                AssignedSessionActionDefinition::envEnter(
                    AssignedEnvironmentEnterSessionActionDefinition {
                        environment_id: environment_id.clone(),
                    },
                )
            }
            Self::TaskRun {
                step_id,
                task_id,
                parameters,
            } => AssignedSessionActionDefinition::taskRun(AssignedTaskRunSessionActionDefinition {
                task_id: Some(task_id.clone()),
                step_id: step_id.clone(),
                parameters: parameters.clone(),
            }),
            Self::EnvironmentExit { environment_id } => AssignedSessionActionDefinition::envExit(
                AssignedEnvironmentExitSessionActionDefinition {
                    environment_id: environment_id.clone(),
                },
            ),
        }
    }

    /// This action as a read of the action itself reports it.
    ///
    /// The same four kinds [`Self::assigned`] hands a worker, in the shapes a reader of one
    /// action carries them (`smithy_query(query="shape",
    /// shape="SessionActionDefinition")`): what the action was told to do is stored once and
    /// reported through whichever of the two unions the operation models.
    pub fn definition(&self) -> SessionActionDefinition {
        match self {
            Self::SyncInputJobAttachments { step_id } => {
                SessionActionDefinition::syncInputJobAttachments(
                    SyncInputJobAttachmentsSessionActionDefinition {
                        step_id: step_id.clone(),
                    },
                )
            }
            Self::EnvironmentEnter { environment_id } => SessionActionDefinition::envEnter(
                EnvironmentEnterSessionActionDefinition {
                    environment_id: environment_id.clone(),
                },
            ),
            Self::TaskRun {
                step_id,
                task_id,
                parameters,
            } => SessionActionDefinition::taskRun(TaskRunSessionActionDefinition {
                task_id: Some(task_id.clone()),
                step_id: step_id.clone(),
                parameters: parameters.clone(),
            }),
            Self::EnvironmentExit { environment_id } => SessionActionDefinition::envExit(
                EnvironmentExitSessionActionDefinition {
                    environment_id: environment_id.clone(),
                },
            ),
        }
    }
}

/// One action of a session: what the worker was told to do, and what it has reported about
/// doing it.
#[derive(Debug, Clone)]
pub struct SessionAction {
    pub session_action_id: String,
    pub kind: SessionActionKind,
    pub status: String,
    pub started_at: Option<SystemTime>,
    pub ended_at: Option<SystemTime>,
    /// When the worker last reported anything about this action.
    pub worker_updated_at: Option<SystemTime>,
    pub process_exit_code: Option<i32>,
    pub progress_message: Option<String>,
    pub progress_percent: Option<f32>,
    pub manifests: Vec<crate::ops::types::TaskRunManifestPropertiesResponse>,
}

impl SessionAction {
    /// Whether the worker has reported a completed status for this action.
    pub fn completed(&self) -> bool {
        is_completed_status(&self.status)
    }
}

/// One worker working on one job: the session the scheduler assigned it.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub farm_id: String,
    pub queue_id: String,
    pub job_id: String,
    pub fleet_id: String,
    pub worker_id: String,
    pub lifecycle_status: String,
    pub target_lifecycle_status: Option<String>,
    pub started_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    /// The actions this session has handed the worker, in the order it is to run them.
    pub actions: Vec<SessionAction>,
    /// The ids of the environments this session has entered and not yet exited, innermost
    /// last: the exits it still owes are these in reverse.
    pub entered_environments: Vec<String>,
    /// The step every task this session has run belongs to: a session keeps the
    /// environments it entered, so the only further work it takes is another task of that
    /// same step.
    pub step_id: Option<String>,
}

/// Where this service's agent writes the logs of the sessions a queue's jobs run in: one log
/// group per queue.
///
/// Computed from the ids of the farm and the queue, and nothing else — every reader of a
/// session's log destination derives it here, so a session's `logConfiguration` and the
/// `logGroupName` a worker is told to write its job's logs to are the same group.
pub fn session_log_group_name(farm_id: &str, queue_id: &str) -> String {
    format!("/aws/deadline/{farm_id}/{queue_id}")
}

impl Session {
    /// A fresh session of `worker_id` on `job_id`, holding no action yet.
    pub fn new(
        session_id: String,
        farm_id: &str,
        queue_id: &str,
        job_id: &str,
        fleet_id: &str,
        worker_id: &str,
    ) -> Self {
        Self {
            session_id,
            farm_id: farm_id.to_string(),
            queue_id: queue_id.to_string(),
            job_id: job_id.to_string(),
            fleet_id: fleet_id.to_string(),
            worker_id: worker_id.to_string(),
            lifecycle_status: SESSION_LIFECYCLE_STARTED.to_string(),
            target_lifecycle_status: None,
            started_at: SystemTime::now(),
            ended_at: None,
            updated_at: None,
            updated_by: None,
            actions: Vec::new(),
            entered_environments: Vec::new(),
            step_id: None,
        }
    }

    /// Hand `kind` to this session's worker as its next action, and answer that action's id.
    ///
    /// The id is this session's hex with the index of the action appended, which is what the
    /// modeled `SessionActionId` pattern spells; the index counts every action the session
    /// has ever held, so an id is never reused.
    pub fn push_action(&mut self, kind: SessionActionKind) -> String {
        let hex = self
            .session_id
            .strip_prefix("session-")
            .unwrap_or(&self.session_id);
        let session_action_id = format!("sessionaction-{hex}-{}", self.actions.len());
        self.actions.push(SessionAction {
            session_action_id: session_action_id.clone(),
            kind,
            status: SESSION_ACTION_STATUS_ASSIGNED.to_string(),
            started_at: None,
            ended_at: None,
            worker_updated_at: None,
            process_exit_code: None,
            progress_message: None,
            progress_percent: None,
            manifests: Vec::new(),
        });
        session_action_id
    }

    /// Whether this session is still the worker's to work on.
    pub fn open(&self) -> bool {
        self.lifecycle_status != SESSION_LIFECYCLE_ENDED
    }

    /// Whether this session has held as many actions as it may hold, so the next task its
    /// worker runs belongs in a session of its own. See [`MAX_SESSION_ACTIONS`].
    pub fn full(&self) -> bool {
        self.actions.len() >= MAX_SESSION_ACTIONS
    }

    /// Whether this session has an action the worker has not finished.
    pub fn working(&self) -> bool {
        self.actions.iter().any(|action| !action.completed())
    }

    /// The action `session_action_id` names, mutably.
    pub fn action_mut(&mut self, session_action_id: &str) -> Option<&mut SessionAction> {
        self.actions
            .iter_mut()
            .find(|action| action.session_action_id == session_action_id)
    }

    /// Where the service's agent is to write this session's logs.
    ///
    /// The driver is the service's own, and the options name the destination it derives from
    /// the ids: one log group per queue and one stream per session. The names are computed
    /// here, from stored state, and nothing else: this emulator runs no agent and stores no
    /// log event, so a session reports where its logs would go and never that any were
    /// collected.
    pub fn log_configuration(&self) -> LogConfiguration {
        LogConfiguration {
            log_driver: SESSION_LOG_DRIVER.to_string(),
            options: Some(
                [
                    (
                        "logGroupName".to_string(),
                        session_log_group_name(&self.farm_id, &self.queue_id),
                    ),
                    ("logStreamName".to_string(), self.session_id.clone()),
                ]
                .into_iter()
                .collect(),
            ),
            parameters: None,
            error: None,
        }
    }

    /// This session as `UpdateWorkerSchedule` reports it to the worker it belongs to: the
    /// actions it has not finished, in order.
    pub fn assigned(&self) -> AssignedSession {
        AssignedSession {
            queue_id: self.queue_id.clone(),
            job_id: self.job_id.clone(),
            session_actions: self
                .actions
                .iter()
                .filter(|action| !action.completed())
                .map(|action| AssignedSessionAction {
                    session_action_id: action.session_action_id.clone(),
                    definition: action.kind.assigned(),
                })
                .collect(),
            log_configuration: self.log_configuration(),
            // `SessionMetadata` is what the service attaches to a session for its own
            // agent; this emulator attaches nothing, so the member is left out rather than
            // sent empty.
            metadata: None,
        }
    }

    /// The ids of the actions this session is still waiting on, which is what a
    /// cancellation names.
    pub fn unfinished_action_ids(&self) -> Vec<String> {
        self.actions
            .iter()
            .filter(|action| !action.completed())
            .map(|action| action.session_action_id.clone())
            .collect()
    }
}

impl crate::state::ComAmazonawsDeadlineDeadlineState {
    /// The session `session_id` names UNDER `farm_id`/`queue_id`/`job_id`: a session of
    /// another job is not found here, because the model reaches a session only through its
    /// own job's path.
    pub fn session(
        &self,
        farm_id: &str,
        queue_id: &str,
        job_id: &str,
        session_id: &str,
    ) -> Option<&Session> {
        self.queue(farm_id, queue_id)?
            .jobs
            .get(job_id)?
            .sessions
            .get(session_id)
    }

    /// The session action `session_action_id` names UNDER `farm_id`/`queue_id`/`job_id`,
    /// with the session that holds it.
    ///
    /// An action is reached only through its own job's path, exactly as its session is: the
    /// model names an action by the three ids above and the action's own id, and an action id
    /// is unique within the job because it carries the hex of the session that minted it (see
    /// [`Session::push_action`]).
    pub fn session_action(
        &self,
        farm_id: &str,
        queue_id: &str,
        job_id: &str,
        session_action_id: &str,
    ) -> Option<(&Session, &SessionAction)> {
        self.queue(farm_id, queue_id)?
            .jobs
            .get(job_id)?
            .sessions
            .values()
            .find_map(|session| {
                session
                    .actions
                    .iter()
                    .find(|action| action.session_action_id == session_action_id)
                    .map(|action| (session, action))
            })
    }

    /// Whether the scheduler is holding `worker_id` on work of `queue_id` right now: the
    /// worker has a session of one of that queue's jobs that has not ended.
    ///
    /// A session is one worker working on one job of one queue (see [`Session`]), and it is
    /// the worker's until it ends, so this is the question "is this worker working on that
    /// queue" asked of stored state alone. A session of ANOTHER queue says nothing about this
    /// one, which is what makes the answer per queue rather than per worker.
    pub fn worker_works_on_queue(&self, farm_id: &str, queue_id: &str, worker_id: &str) -> bool {
        self.queue(farm_id, queue_id).is_some_and(|queue| {
            queue
                .jobs
                .values()
                .flat_map(|job| job.sessions.values())
                .any(|session| session.worker_id == worker_id && session.open())
        })
    }
}
