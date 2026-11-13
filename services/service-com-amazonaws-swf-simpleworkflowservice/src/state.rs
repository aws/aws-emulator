// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-swf-simpleworkflowservice service.
//!
//! One map of domains; a domain owns its registered workflow/activity types and its
//! workflow executions, and an execution owns its history event log. Every handler
//! computes its response from what is stored here — nothing is canned and nothing is
//! echoed back out of the request that asked for it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use ::aws_smithy_types::DateTime;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    ActivityTaskCancelRequestedEventAttributes, ActivityTaskCanceledEventAttributes,
    ActivityTaskCompletedEventAttributes, ActivityTaskFailedEventAttributes,
    ActivityTaskScheduledEventAttributes, ActivityTaskStartedEventAttributes,
    ActivityTaskTimedOutEventAttributes, ActivityType, ActivityTypeConfiguration, ActivityTypeInfo,
    CancelTimerFailedEventAttributes, CancelWorkflowExecutionFailedEventAttributes,
    ChildWorkflowExecutionCanceledEventAttributes, ChildWorkflowExecutionCompletedEventAttributes,
    ChildWorkflowExecutionFailedEventAttributes, ChildWorkflowExecutionStartedEventAttributes,
    ChildWorkflowExecutionTerminatedEventAttributes, ChildWorkflowExecutionTimedOutEventAttributes,
    CompleteWorkflowExecutionFailedEventAttributes,
    ContinueAsNewWorkflowExecutionFailedEventAttributes,
    DecisionTaskCompletedEventAttributes, DecisionTaskScheduledEventAttributes,
    DecisionTaskStartedEventAttributes, DecisionTaskTimedOutEventAttributes, DomainConfiguration,
    DomainInfo, ExecutionTimeFilter, ExternalWorkflowExecutionCancelRequestedEventAttributes,
    ExternalWorkflowExecutionSignaledEventAttributes, FailWorkflowExecutionFailedEventAttributes,
    HistoryEvent, LambdaFunctionScheduledEventAttributes, MarkerRecordedEventAttributes,
    RequestCancelActivityTaskFailedEventAttributes,
    RequestCancelExternalWorkflowExecutionFailedEventAttributes,
    RequestCancelExternalWorkflowExecutionInitiatedEventAttributes, ResourceTag,
    ScheduleActivityTaskFailedEventAttributes, ScheduleLambdaFunctionFailedEventAttributes,
    SignalExternalWorkflowExecutionFailedEventAttributes,
    SignalExternalWorkflowExecutionInitiatedEventAttributes,
    StartChildWorkflowExecutionFailedEventAttributes,
    StartChildWorkflowExecutionInitiatedEventAttributes, StartLambdaFunctionFailedEventAttributes,
    StartTimerFailedEventAttributes, TagFilter, TaskList, TimerCanceledEventAttributes,
    TimerFiredEventAttributes, TimerStartedEventAttributes, WorkflowExecution,
    WorkflowExecutionCancelRequestedEventAttributes, WorkflowExecutionCanceledEventAttributes,
    WorkflowExecutionCompletedEventAttributes, WorkflowExecutionConfiguration,
    WorkflowExecutionContinuedAsNewEventAttributes, WorkflowExecutionFailedEventAttributes,
    WorkflowExecutionFilter, WorkflowExecutionInfo,
    WorkflowExecutionOpenCounts, WorkflowExecutionSignaledEventAttributes,
    WorkflowExecutionStartedEventAttributes,
    WorkflowExecutionTerminatedEventAttributes, WorkflowExecutionTimedOutEventAttributes,
    WorkflowType, WorkflowTypeConfiguration,
    WorkflowTypeFilter, WorkflowTypeInfo,
};

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as the model's
/// `aws.api#service` trait records it (`arnNamespace: swf`).
pub const ARN_NAMESPACE: &str = "swf";

/// `RegistrationStatus` of a freshly registered domain or type. One of
/// [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
pub const STATUS_REGISTERED: &str = "REGISTERED";

/// `RegistrationStatus` of a domain or type that has been deprecated. One of
/// [`crate::ops::types::REGISTRATION_STATUS_VALUES`]; `RegistrationStatus`'s own doc
/// calls it "DEPRECATED – The type was deprecated".
pub const STATUS_DEPRECATED: &str = "DEPRECATED";

/// `ExecutionStatus` of a running execution. One of
/// [`crate::ops::types::EXECUTION_STATUS_VALUES`].
pub const EXECUTION_STATUS_OPEN: &str = "OPEN";

/// `ExecutionStatus` of an execution that has closed. One of
/// [`crate::ops::types::EXECUTION_STATUS_VALUES`].
pub const EXECUTION_STATUS_CLOSED: &str = "CLOSED";

/// `CloseStatus` of an execution closed by `TerminateWorkflowExecution`. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_TERMINATED: &str = "TERMINATED";

/// `CloseStatus` of an execution a `CompleteWorkflowExecution` decision closed. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_COMPLETED: &str = "COMPLETED";

/// `CloseStatus` of an execution a `FailWorkflowExecution` decision closed. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_FAILED: &str = "FAILED";

/// `CloseStatus` of an execution a `CancelWorkflowExecution` decision closed. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_CANCELED: &str = "CANCELED";

/// `CloseStatus` of an execution a `ContinueAsNewWorkflowExecution` decision closed in
/// favour of a new run of the same `workflowId`. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`].
pub const CLOSE_STATUS_CONTINUED_AS_NEW: &str = "CONTINUED_AS_NEW";

/// The `cause` a `WorkflowExecutionTerminated` or `WorkflowExecutionCancelRequested`
/// event carries when the SERVICE closed or asked to cancel the run because its PARENT
/// run closed and the parent's `childPolicy` said so. One of
/// [`crate::ops::types::WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES`] and the only
/// member of [`crate::ops::types::WORKFLOW_EXECUTION_CANCEL_REQUESTED_CAUSE_VALUES`].
pub const CAUSE_CHILD_POLICY_APPLIED: &str = "CHILD_POLICY_APPLIED";

/// The `childPolicy` that terminates a closing run's open children. One of
/// [`crate::ops::types::CHILD_POLICY_VALUES`], whose own doc calls it "TERMINATE – The
/// child executions are terminated".
pub const CHILD_POLICY_TERMINATE: &str = "TERMINATE";

/// The `childPolicy` that asks a closing run's open children to cancel themselves. One
/// of [`crate::ops::types::CHILD_POLICY_VALUES`]: "REQUEST_CANCEL – A request to cancel
/// is attempted for each child execution".
pub const CHILD_POLICY_REQUEST_CANCEL: &str = "REQUEST_CANCEL";

/// `CloseStatus` of an execution the service itself closed because its
/// `executionStartToCloseTimeout` ran out. One of
/// [`crate::ops::types::CLOSE_STATUS_VALUES`], whose own doc calls it "TIMED_OUT – the
/// execution did not complete in the alloted time and was automatically timed out".
pub const CLOSE_STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// The `timeoutType` a `WorkflowExecutionTimedOut` event carries: the only member of
/// [`crate::ops::types::WORKFLOW_EXECUTION_TIMEOUT_TYPE_VALUES`], because the
/// start-to-close budget is the one timeout an execution as a whole has.
pub const WORKFLOW_EXECUTION_TIMEOUT_START_TO_CLOSE: &str = "START_TO_CLOSE";

/// How long a run whose `executionStartToCloseTimeout` is the member's published lower
/// bound of zero stays OPEN before the start-to-close sweep closes it.
///
/// `StartWorkflowExecution` answers with the `runId` of a run it has OPENED and
/// recorded a first decision task for, so a caller that acts straight after the call
/// must be able to see that run open: a zero-second budget cannot mean "closed in the
/// same instant it started". Emulator-chosen, and the only budget it applies to — a
/// duration of one second or more is honoured exactly.
pub const MIN_OPEN_EXECUTION_SPAN: Duration = Duration::from_millis(500);

/// The shortest span one task's own timeout is measured over, for the same reason a
/// run's budget has one ([`MIN_OPEN_EXECUTION_SPAN`]): every SWF task duration is "an
/// integer greater than or equal to 0", and a zero-second one would have the task time
/// out in the instant it was scheduled or handed over, so no caller could see the task
/// the scheduling decision recorded or the poll answered with. Every duration of one
/// second or more is honoured exactly.
pub const MIN_TASK_SPAN: Duration = MIN_OPEN_EXECUTION_SPAN;

/// The `cause` a `WorkflowExecutionTerminated` event carries when the SERVICE closed
/// the run because its history reached [`MAX_HISTORY_EVENTS_PER_EXECUTION`]. One of
/// [`crate::ops::types::WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES`].
pub const TERMINATED_CAUSE_EVENT_LIMIT_EXCEEDED: &str = "EVENT_LIMIT_EXCEEDED";

/// The `cause` a `WorkflowExecutionTerminated` event carries when an operator's own
/// `TerminateWorkflowExecution` call closed the run. One of
/// [`crate::ops::types::WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES`].
pub const TERMINATED_CAUSE_OPERATOR_INITIATED: &str = "OPERATOR_INITIATED";

/// How many history events one workflow execution holds. Amazon SWF Quotas records a
/// maximum of 25,000 events per workflow execution history, and an execution that
/// reaches it is terminated by the service rather than grown further — which is what
/// [`RunClose::event_limit_exceeded`] records, and what bounds the memory one run's
/// history can take however many times a caller signals or polls it.
pub const MAX_HISTORY_EVENTS_PER_EXECUTION: usize = 25_000;

/// How many simultaneous long polls one task list admits, whichever kind of task is
/// polled for. The Amazon SWF Developer Guide's "Quotas on task executions" records a
/// maximum of 1,000 pollers per task list; a poll offered past it is answered with the
/// modelled `LimitExceededFault` ("Returned by any operation if a system imposed
/// limitation has been reached").
pub const MAX_POLLERS_PER_TASK_LIST: usize = 1_000;

/// The `LimitExceededFault` message a poll offered past [`MAX_POLLERS_PER_TASK_LIST`]
/// carries, whichever kind of task is polled for. SWF reports this condition with one
/// fixed sentence: nothing of the request or of the store is interpolated into it — not
/// the task list, not the domain, not the number of pollers the list admits — so every
/// such refusal carries this text byte for byte.
pub const POLLER_LIMIT_EXCEEDED_MESSAGE: &str =
    "Limit on maximum concurrent poll requests per task list reached";

/// How long a long poll holds its answer open while its task list is empty. Both polling
/// operations document the same hold: `PollForDecisionTask`'s "If no decision task is
/// available in the specified task list before the timeout of 60 seconds expires, an
/// empty result is returned", and `PollForActivityTask`'s "The maximum time the service
/// holds on to the request before responding is 60 seconds. If no task is available
/// within 60 seconds, the poll returns an empty result."
pub const POLL_TIMEOUT: Duration = Duration::from_secs(60);

/// How often an open long poll re-reads its task list. Emulator-chosen, and small enough
/// that a task scheduled while a poll is waiting is handed over promptly ("responds as
/// soon as a task becomes available") without the pollers one list admits spinning on
/// the state lock between them.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The maximum `workflowExecutionRetentionPeriodInDays` RegisterDomain accepts. The
/// member's own doc: "The maximum workflow execution retention period is 90 days."
pub const MAX_RETENTION_PERIOD_IN_DAYS: u32 = 90;

/// The largest value an execution start-to-close timeout may take, in seconds. The
/// `executionStartToCloseTimeout` doc: "there is a one-year max limit on the time that
/// a workflow execution can run."
pub const MAX_EXECUTION_START_TO_CLOSE_SECONDS: u64 = 365 * 24 * 60 * 60;

/// Tags one domain may carry; `RegisterDomain`'s modelled `TooManyTagsFault` ("You've
/// exceeded the number of tags allowed for a domain") is the answer past it. Amazon SWF
/// documents a limit of 50 tags per domain.
pub const MAX_TAGS_PER_DOMAIN: usize = 50;

/// How many domains this emulator holds. Emulator-chosen, so a caller cannot grow the
/// in-memory store without limit; reported with the modelled `LimitExceededFault`
/// ("Returned by any operation if a system imposed limitation has been reached").
pub const MAX_DOMAINS: usize = 100;

/// How many workflow types one domain holds. Emulator-chosen, for the same reason as
/// [`MAX_DOMAINS`], and answered with the same modelled `LimitExceededFault`.
pub const MAX_WORKFLOW_TYPES_PER_DOMAIN: usize = 1_000;

/// How many activity types one domain holds. Emulator-chosen, for the same reason as
/// [`MAX_DOMAINS`], and answered with the same modelled `LimitExceededFault`.
pub const MAX_ACTIVITY_TYPES_PER_DOMAIN: usize = 1_000;

/// How many workflow executions one domain retains. Emulator-chosen, for the same
/// reason as [`MAX_DOMAINS`], and answered with the same modelled
/// `LimitExceededFault`.
pub const MAX_EXECUTIONS_PER_DOMAIN: usize = 1_000;

/// The largest count a visibility `Count*` operation reports in one answer. The
/// modelled `truncated` member is what reports a count clipped to it: "If set to true,
/// indicates that the actual count was more than the maximum supported by this API and
/// the count returned is the truncated value." Emulator-chosen, and equal to
/// [`MAX_EXECUTIONS_PER_DOMAIN`] — the most executions one domain can hold — so a count
/// this emulator can actually reach is never clipped and `truncated` stays false.
pub const MAX_REPORTED_EXECUTION_COUNT: usize = MAX_EXECUTIONS_PER_DOMAIN;

/// The page size `GetWorkflowExecutionHistory` uses when the caller passes no
/// `maximumPageSize` (the member defaults to `0`), so one unparameterised request
/// cannot materialise a response that grows without bound. It is the modelled
/// `PageSize` maximum (range 0..1000).
pub const MAX_HISTORY_PAGE_SIZE: usize = 1_000;

/// The page size a visibility list of workflow executions uses when the caller passes
/// no `maximumPageSize` (the member defaults to `0`), so one unparameterised request
/// cannot materialise a response that grows without bound. It is the modelled
/// `PageSize` maximum (range 0..1000).
pub const MAX_EXECUTION_PAGE_SIZE: usize = 1_000;

/// The page size a listing of a domain's registered types uses when the caller passes
/// no `maximumPageSize` (the member defaults to `0`), so one unparameterised request
/// cannot materialise a response that grows without bound. It is the modelled
/// `PageSize` maximum (range 0..1000).
pub const MAX_TYPE_PAGE_SIZE: usize = 1_000;

/// The `timeoutType` of a decision task that was handed to a decider and not completed
/// within its `startToCloseTimeout` ("The maximum duration for this decision task. The
/// task is considered timed out if it doesn't completed within this duration."). One of
/// [`crate::ops::types::DECISION_TASK_TIMEOUT_TYPE_VALUES`].
pub const DECISION_TASK_TIMEOUT_START_TO_CLOSE: &str = "START_TO_CLOSE";

/// The `timeoutType` of a decision task that was scheduled on an overridden task list
/// and not started within the `taskListScheduleToStartTimeout` the override carried
/// ("If a decision task scheduled on the override task list is not started within the
/// timeout, the decision task will time out"). One of
/// [`crate::ops::types::DECISION_TASK_TIMEOUT_TYPE_VALUES`].
pub const DECISION_TASK_TIMEOUT_SCHEDULE_TO_START: &str = "SCHEDULE_TO_START";

/// The `timeoutType` of an activity task that waited in its task list longer than its
/// `scheduleToStartTimeout` ("The maximum amount of time the activity task can wait to
/// be assigned to a worker."). One of
/// [`crate::ops::types::ACTIVITY_TASK_TIMEOUT_TYPE_VALUES`].
pub const ACTIVITY_TASK_TIMEOUT_SCHEDULE_TO_START: &str = "SCHEDULE_TO_START";

/// The `timeoutType` of an activity task whose `scheduleToCloseTimeout` ran out before
/// it closed ("The maximum amount of time for this activity task."), counted from the
/// instant it was scheduled. One of
/// [`crate::ops::types::ACTIVITY_TASK_TIMEOUT_TYPE_VALUES`].
pub const ACTIVITY_TASK_TIMEOUT_SCHEDULE_TO_CLOSE: &str = "SCHEDULE_TO_CLOSE";

/// The `timeoutType` of an activity task a worker was handed and did not close within
/// its `startToCloseTimeout` ("The maximum amount of time a worker may take to process
/// the activity task."). One of
/// [`crate::ops::types::ACTIVITY_TASK_TIMEOUT_TYPE_VALUES`].
pub const ACTIVITY_TASK_TIMEOUT_START_TO_CLOSE: &str = "START_TO_CLOSE";

/// The `timeoutType` of an activity task whose worker stopped reporting progress:
/// `heartbeatTimeout` is "The maximum time before which the worker processing this task
/// must report progress by calling RecordActivityTaskHeartbeat. If the timeout is
/// exceeded, the activity task is automatically timed out." One of
/// [`crate::ops::types::ACTIVITY_TASK_TIMEOUT_TYPE_VALUES`].
pub const ACTIVITY_TASK_TIMEOUT_HEARTBEAT: &str = "HEARTBEAT";

/// The duration value every SWF timeout member accepts for "unlimited": `NONE`, as
/// `startToCloseTimeout`'s own doc puts it ("You can use NONE to specify unlimited
/// duration"). A task scheduled with it never times out.
pub const DURATION_NONE: &str = "NONE";

/// The `taskPriority` a decision task scheduled without one is dispatched at. The
/// member's doc: "A task priority that, if set, specifies the priority for this decision
/// task. Valid values are integers that range from Java's Integer.MIN_VALUE
/// (-2147483648) to Integer.MAX_VALUE (2147483647). Higher numbers indicate higher
/// priority." Amazon SWF documents 0 as the default priority.
pub const DEFAULT_TASK_PRIORITY: i64 = 0;

/// The `reopenGenerationId` of a workflow execution that has never been reopened, and
/// so of every run this service stores: no operation it models reopens a closed run.
pub const FIRST_RUN_GENERATION: i64 = 0;

/// What separates a task-start `clientToken`'s digest from the task tokens derived over
/// the same identity ([`task_token`]), so the handle the service keeps on a start is
/// never also the handle it hands the worker.
const CLIENT_TOKEN_TAG: &str = "client-token";

/// The lengths of the five hex groups of a canonical UUID, in order.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// A registered workflow type, as `RegisterWorkflowType` left it. The `default_*`
/// members are what `StartWorkflowExecution` falls back to when the start request
/// does not override them.
#[derive(Debug, Clone)]
pub struct WorkflowTypeRecord {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub creation_date: DateTime,
    pub deprecation_date: Option<DateTime>,
    pub default_task_list: Option<String>,
    pub default_task_start_to_close_timeout: Option<String>,
    pub default_execution_start_to_close_timeout: Option<String>,
    pub default_task_priority: Option<String>,
    pub default_child_policy: Option<String>,
    pub default_lambda_role: Option<String>,
}

impl WorkflowTypeRecord {
    /// The `WorkflowType` identity this record is keyed by.
    pub fn workflow_type(&self) -> WorkflowType {
        WorkflowType {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// Whether this type is deprecated, i.e. whether its stored `RegistrationStatus`
    /// is [`STATUS_DEPRECATED`].
    pub fn is_deprecated(&self) -> bool {
        self.status == STATUS_DEPRECATED
    }

    /// Move this type off `REGISTERED` as of `at`: the status move `DeprecateWorkflowType`
    /// makes on the record it addresses. A record that is already deprecated keeps the
    /// `deprecationDate` its own deprecation gave it, so no later call can rewrite an
    /// instant that already happened.
    pub fn deprecate(&mut self, at: DateTime) {
        if self.is_deprecated() {
            return;
        }
        self.status = STATUS_DEPRECATED.to_string();
        self.deprecation_date = Some(at);
    }

    /// Return this type to `REGISTERED`, the move `UndeprecateWorkflowType` makes
    /// ("Undeprecates a previously deprecated workflow type. After a workflow type has
    /// been undeprecated, you can create new executions of that type").
    /// `WorkflowTypeInfo.deprecationDate`'s doc scopes that stamp to a deprecated type
    /// ("If the type is in deprecated state, then it is set to the date when the type was
    /// deprecated"), so a record back on `REGISTERED` reports none. Nothing else the
    /// record holds — identity, description, registration-time defaults, creation date —
    /// is touched.
    pub fn undeprecate(&mut self) {
        self.status = STATUS_REGISTERED.to_string();
        self.deprecation_date = None;
    }

    /// The `WorkflowTypeInfo` every workflow-type reader returns, built from what is
    /// stored.
    pub fn info(&self) -> WorkflowTypeInfo {
        WorkflowTypeInfo {
            workflow_type: self.workflow_type(),
            status: self.status.clone(),
            description: self.description.clone(),
            creation_date: self.creation_date,
            deprecation_date: self.deprecation_date,
        }
    }

    /// The `WorkflowTypeConfiguration` `DescribeWorkflowType` reports: the
    /// registration-time defaults, exactly as stored.
    pub fn configuration(&self) -> WorkflowTypeConfiguration {
        WorkflowTypeConfiguration {
            default_task_start_to_close_timeout: self.default_task_start_to_close_timeout.clone(),
            default_execution_start_to_close_timeout: self
                .default_execution_start_to_close_timeout
                .clone(),
            default_task_list: self
                .default_task_list
                .clone()
                .map(|name| TaskList { name }),
            default_task_priority: self.default_task_priority.clone(),
            default_child_policy: self.default_child_policy.clone(),
            default_lambda_role: self.default_lambda_role.clone(),
        }
    }
}

/// A registered activity type, as `RegisterActivityType` left it. The `default_*`
/// members are the registration-time settings a `ScheduleActivityTask` decision falls
/// back to when it does not override them, and are reported verbatim by
/// `DescribeActivityType`: the type's configuration cannot change after registration.
#[derive(Debug, Clone)]
pub struct ActivityTypeRecord {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub creation_date: DateTime,
    pub deprecation_date: Option<DateTime>,
    pub default_task_list: Option<String>,
    pub default_task_priority: Option<String>,
    pub default_task_heartbeat_timeout: Option<String>,
    pub default_task_schedule_to_close_timeout: Option<String>,
    pub default_task_schedule_to_start_timeout: Option<String>,
    pub default_task_start_to_close_timeout: Option<String>,
}

impl ActivityTypeRecord {
    /// The `ActivityType` identity this record is keyed by.
    pub fn activity_type(&self) -> ActivityType {
        ActivityType {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    /// Whether this type is deprecated, i.e. whether its stored `RegistrationStatus`
    /// is [`STATUS_DEPRECATED`].
    pub fn is_deprecated(&self) -> bool {
        self.status == STATUS_DEPRECATED
    }

    /// Move this type off `REGISTERED` as of `at`: the status move
    /// `DeprecateActivityType` makes on the record it addresses. A record that is
    /// already deprecated keeps the `deprecationDate` its own deprecation gave it, so
    /// no later call can rewrite an instant that already happened.
    pub fn deprecate(&mut self, at: DateTime) {
        if self.is_deprecated() {
            return;
        }
        self.status = STATUS_DEPRECATED.to_string();
        self.deprecation_date = Some(at);
    }

    /// Return this type to `REGISTERED`, the move `UndeprecateActivityType` makes.
    /// `ActivityTypeInfo.deprecationDate`'s doc scopes that stamp to a deprecated type
    /// ("If DEPRECATED, the date and time DeprecateActivityType was called"), so a
    /// record back on `REGISTERED` reports none.
    pub fn undeprecate(&mut self) {
        self.status = STATUS_REGISTERED.to_string();
        self.deprecation_date = None;
    }

    /// The `ActivityTypeInfo` every activity-type reader returns, built from what is
    /// stored.
    pub fn info(&self) -> ActivityTypeInfo {
        ActivityTypeInfo {
            activity_type: self.activity_type(),
            status: self.status.clone(),
            description: self.description.clone(),
            creation_date: self.creation_date,
            deprecation_date: self.deprecation_date,
        }
    }

    /// The `ActivityTypeConfiguration` `DescribeActivityType` reports: the
    /// registration-time defaults, exactly as stored.
    pub fn configuration(&self) -> ActivityTypeConfiguration {
        ActivityTypeConfiguration {
            default_task_start_to_close_timeout: self.default_task_start_to_close_timeout.clone(),
            default_task_heartbeat_timeout: self.default_task_heartbeat_timeout.clone(),
            default_task_list: self
                .default_task_list
                .clone()
                .map(|name| TaskList { name }),
            default_task_priority: self.default_task_priority.clone(),
            default_task_schedule_to_start_timeout: self
                .default_task_schedule_to_start_timeout
                .clone(),
            default_task_schedule_to_close_timeout: self
                .default_task_schedule_to_close_timeout
                .clone(),
        }
    }
}

/// The timeouts, task list and child policy an execution actually runs under:
/// resolved ONCE at start (the request value where given, else the workflow type's
/// registered default) and reported verbatim by `DescribeWorkflowExecution`, so a
/// later re-registration of the type cannot change what a running execution reports.
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    pub task_list: String,
    pub task_priority: Option<String>,
    pub task_start_to_close_timeout: String,
    pub execution_start_to_close_timeout: String,
    /// One of [`crate::ops::types::CHILD_POLICY_VALUES`].
    pub child_policy: String,
    pub lambda_role: Option<String>,
}

impl ExecutionConfig {
    /// The `WorkflowExecutionConfiguration` `DescribeWorkflowExecution` reports.
    pub fn configuration(&self) -> WorkflowExecutionConfiguration {
        WorkflowExecutionConfiguration {
            task_start_to_close_timeout: self.task_start_to_close_timeout.clone(),
            execution_start_to_close_timeout: self.execution_start_to_close_timeout.clone(),
            task_list: TaskList {
                name: self.task_list.clone(),
            },
            task_priority: self.task_priority.clone(),
            child_policy: self.child_policy.clone(),
            lambda_role: self.lambda_role.clone(),
        }
    }
}

/// The attributes of one recorded history event. One variant per event type this slice
/// records; the variant IS the event type, so a stored event can never carry the
/// attributes of a different kind than its `eventType` claims.
#[derive(Debug, Clone)]
pub enum EventAttributes {
    /// Boxed: `WorkflowExecutionStartedEventAttributes` is several times the size of
    /// the other variants, and every stored history event pays for the largest one.
    WorkflowExecutionStarted(Box<WorkflowExecutionStartedEventAttributes>),
    DecisionTaskScheduled(DecisionTaskScheduledEventAttributes),
    DecisionTaskStarted(DecisionTaskStartedEventAttributes),
    DecisionTaskCompleted(DecisionTaskCompletedEventAttributes),
    DecisionTaskTimedOut(DecisionTaskTimedOutEventAttributes),
    /// Boxed for the same reason as the started attributes above:
    /// `ActivityTaskScheduledEventAttributes` carries eleven members, several times the
    /// size of the other variants.
    ActivityTaskScheduled(Box<ActivityTaskScheduledEventAttributes>),
    ScheduleActivityTaskFailed(ScheduleActivityTaskFailedEventAttributes),
    ActivityTaskStarted(ActivityTaskStartedEventAttributes),
    ActivityTaskTimedOut(ActivityTaskTimedOutEventAttributes),
    ActivityTaskCancelRequested(ActivityTaskCancelRequestedEventAttributes),
    ActivityTaskCanceled(ActivityTaskCanceledEventAttributes),
    ActivityTaskCompleted(ActivityTaskCompletedEventAttributes),
    ActivityTaskFailed(ActivityTaskFailedEventAttributes),
    RequestCancelActivityTaskFailed(RequestCancelActivityTaskFailedEventAttributes),
    MarkerRecorded(MarkerRecordedEventAttributes),
    TimerStarted(TimerStartedEventAttributes),
    TimerFired(TimerFiredEventAttributes),
    TimerCanceled(TimerCanceledEventAttributes),
    StartTimerFailed(StartTimerFailedEventAttributes),
    CancelTimerFailed(CancelTimerFailedEventAttributes),
    WorkflowExecutionSignaled(WorkflowExecutionSignaledEventAttributes),
    WorkflowExecutionCompleted(WorkflowExecutionCompletedEventAttributes),
    CompleteWorkflowExecutionFailed(CompleteWorkflowExecutionFailedEventAttributes),
    WorkflowExecutionFailed(WorkflowExecutionFailedEventAttributes),
    FailWorkflowExecutionFailed(FailWorkflowExecutionFailedEventAttributes),
    WorkflowExecutionCanceled(WorkflowExecutionCanceledEventAttributes),
    CancelWorkflowExecutionFailed(CancelWorkflowExecutionFailedEventAttributes),
    /// Boxed for the same reason as the started attributes above: eleven members.
    WorkflowExecutionContinuedAsNew(Box<WorkflowExecutionContinuedAsNewEventAttributes>),
    ContinueAsNewWorkflowExecutionFailed(ContinueAsNewWorkflowExecutionFailedEventAttributes),
    WorkflowExecutionTerminated(WorkflowExecutionTerminatedEventAttributes),
    WorkflowExecutionTimedOut(WorkflowExecutionTimedOutEventAttributes),
    WorkflowExecutionCancelRequested(WorkflowExecutionCancelRequestedEventAttributes),
    /// Boxed for the same reason as the started attributes above: twelve members.
    StartChildWorkflowExecutionInitiated(Box<StartChildWorkflowExecutionInitiatedEventAttributes>),
    StartChildWorkflowExecutionFailed(StartChildWorkflowExecutionFailedEventAttributes),
    ChildWorkflowExecutionStarted(ChildWorkflowExecutionStartedEventAttributes),
    ChildWorkflowExecutionCompleted(ChildWorkflowExecutionCompletedEventAttributes),
    ChildWorkflowExecutionFailed(ChildWorkflowExecutionFailedEventAttributes),
    ChildWorkflowExecutionCanceled(ChildWorkflowExecutionCanceledEventAttributes),
    ChildWorkflowExecutionTimedOut(ChildWorkflowExecutionTimedOutEventAttributes),
    ChildWorkflowExecutionTerminated(ChildWorkflowExecutionTerminatedEventAttributes),
    SignalExternalWorkflowExecutionInitiated(SignalExternalWorkflowExecutionInitiatedEventAttributes),
    ExternalWorkflowExecutionSignaled(ExternalWorkflowExecutionSignaledEventAttributes),
    SignalExternalWorkflowExecutionFailed(SignalExternalWorkflowExecutionFailedEventAttributes),
    RequestCancelExternalWorkflowExecutionInitiated(
        RequestCancelExternalWorkflowExecutionInitiatedEventAttributes,
    ),
    ExternalWorkflowExecutionCancelRequested(ExternalWorkflowExecutionCancelRequestedEventAttributes),
    RequestCancelExternalWorkflowExecutionFailed(
        RequestCancelExternalWorkflowExecutionFailedEventAttributes,
    ),
    LambdaFunctionScheduled(LambdaFunctionScheduledEventAttributes),
    ScheduleLambdaFunctionFailed(ScheduleLambdaFunctionFailedEventAttributes),
    StartLambdaFunctionFailed(StartLambdaFunctionFailedEventAttributes),
}

/// What one recorded event does to the work its run has in flight, and whether a
/// decider has to see it.
///
/// Every field is read off ONE exhaustive match ([`EventAttributes::effect`]), so an
/// event type added later cannot be forgotten by any of the readers: each of them asks
/// for a field of this rather than matching the variant itself.
#[derive(Debug, Clone, Default)]
pub struct EventEffect {
    /// The `scheduledEventId` of the decision task this event takes OUT of its task
    /// list: a decider was handed it, or it timed out waiting there.
    pub decision_task_left_task_list: Option<i64>,
    /// The `startedEventId` of the decision task this event CLOSES — what tells a
    /// started decision task that is still outstanding from one already answered.
    pub decision_task_closed: Option<i64>,
    /// The `scheduledEventId` of the activity task this event takes OUT of its task
    /// list: a worker was handed it, or it left the list without being handed over.
    pub activity_task_left_task_list: Option<i64>,
    /// The `scheduledEventId` of the activity task this event takes out of flight.
    pub activity_task_closed: Option<i64>,
    /// Whether this event is the one that CLOSED the run: the single event each way of
    /// closing records. It is what [`Execution::closing_event`] finds, and so what
    /// `WorkflowExecutionInfo`'s `lastClosedEventId`/`lastClosedTimestamp` report.
    pub closes_run: bool,
    /// The `startedEventId` of the timer this event takes out of flight: it fired or
    /// it was cancelled.
    pub timer_closed: Option<i64>,
    /// The `scheduledEventId` of the Lambda task this event takes out of flight.
    pub lambda_function_closed: Option<i64>,
    /// Whether a decider has to act on this event. An event recorded while a decision
    /// task was outstanding — or recorded by the completion of one — gets a decision
    /// task of its own, which is how the decider is told about it.
    pub needs_decision: bool,
}

impl EventAttributes {
    /// The `eventType` these attributes belong to — one of
    /// [`crate::ops::types::EVENT_TYPE_VALUES`].
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::WorkflowExecutionStarted(_) => "WorkflowExecutionStarted",
            Self::DecisionTaskScheduled(_) => "DecisionTaskScheduled",
            Self::DecisionTaskStarted(_) => "DecisionTaskStarted",
            Self::DecisionTaskCompleted(_) => "DecisionTaskCompleted",
            Self::DecisionTaskTimedOut(_) => "DecisionTaskTimedOut",
            Self::ActivityTaskScheduled(_) => "ActivityTaskScheduled",
            Self::ScheduleActivityTaskFailed(_) => "ScheduleActivityTaskFailed",
            Self::ActivityTaskStarted(_) => "ActivityTaskStarted",
            Self::ActivityTaskTimedOut(_) => "ActivityTaskTimedOut",
            Self::ActivityTaskCancelRequested(_) => "ActivityTaskCancelRequested",
            Self::ActivityTaskCanceled(_) => "ActivityTaskCanceled",
            Self::ActivityTaskCompleted(_) => "ActivityTaskCompleted",
            Self::ActivityTaskFailed(_) => "ActivityTaskFailed",
            Self::RequestCancelActivityTaskFailed(_) => "RequestCancelActivityTaskFailed",
            Self::MarkerRecorded(_) => "MarkerRecorded",
            Self::TimerStarted(_) => "TimerStarted",
            Self::TimerFired(_) => "TimerFired",
            Self::TimerCanceled(_) => "TimerCanceled",
            Self::StartTimerFailed(_) => "StartTimerFailed",
            Self::CancelTimerFailed(_) => "CancelTimerFailed",
            Self::WorkflowExecutionSignaled(_) => "WorkflowExecutionSignaled",
            Self::WorkflowExecutionCompleted(_) => "WorkflowExecutionCompleted",
            Self::CompleteWorkflowExecutionFailed(_) => "CompleteWorkflowExecutionFailed",
            Self::WorkflowExecutionFailed(_) => "WorkflowExecutionFailed",
            Self::FailWorkflowExecutionFailed(_) => "FailWorkflowExecutionFailed",
            Self::WorkflowExecutionCanceled(_) => "WorkflowExecutionCanceled",
            Self::CancelWorkflowExecutionFailed(_) => "CancelWorkflowExecutionFailed",
            Self::WorkflowExecutionContinuedAsNew(_) => "WorkflowExecutionContinuedAsNew",
            Self::ContinueAsNewWorkflowExecutionFailed(_) => "ContinueAsNewWorkflowExecutionFailed",
            Self::WorkflowExecutionTerminated(_) => "WorkflowExecutionTerminated",
            Self::WorkflowExecutionTimedOut(_) => "WorkflowExecutionTimedOut",
            Self::WorkflowExecutionCancelRequested(_) => "WorkflowExecutionCancelRequested",
            Self::StartChildWorkflowExecutionInitiated(_) => "StartChildWorkflowExecutionInitiated",
            Self::StartChildWorkflowExecutionFailed(_) => "StartChildWorkflowExecutionFailed",
            Self::ChildWorkflowExecutionStarted(_) => "ChildWorkflowExecutionStarted",
            Self::ChildWorkflowExecutionCompleted(_) => "ChildWorkflowExecutionCompleted",
            Self::ChildWorkflowExecutionFailed(_) => "ChildWorkflowExecutionFailed",
            Self::ChildWorkflowExecutionCanceled(_) => "ChildWorkflowExecutionCanceled",
            Self::ChildWorkflowExecutionTimedOut(_) => "ChildWorkflowExecutionTimedOut",
            Self::ChildWorkflowExecutionTerminated(_) => "ChildWorkflowExecutionTerminated",
            Self::SignalExternalWorkflowExecutionInitiated(_) => {
                "SignalExternalWorkflowExecutionInitiated"
            }
            Self::ExternalWorkflowExecutionSignaled(_) => "ExternalWorkflowExecutionSignaled",
            Self::SignalExternalWorkflowExecutionFailed(_) => {
                "SignalExternalWorkflowExecutionFailed"
            }
            Self::RequestCancelExternalWorkflowExecutionInitiated(_) => {
                "RequestCancelExternalWorkflowExecutionInitiated"
            }
            Self::ExternalWorkflowExecutionCancelRequested(_) => {
                "ExternalWorkflowExecutionCancelRequested"
            }
            Self::RequestCancelExternalWorkflowExecutionFailed(_) => {
                "RequestCancelExternalWorkflowExecutionFailed"
            }
            Self::LambdaFunctionScheduled(_) => "LambdaFunctionScheduled",
            Self::ScheduleLambdaFunctionFailed(_) => "ScheduleLambdaFunctionFailed",
            Self::StartLambdaFunctionFailed(_) => "StartLambdaFunctionFailed",
        }
    }

    /// What this event does to its run's work in flight. The ONE exhaustive match over
    /// the event types, so a variant added later cannot be forgotten by the readers
    /// ([`Execution::pending_decision_tasks_on`],
    /// [`Execution::pending_activity_tasks_on`], [`Execution::open_timer`] and the
    /// rest), which each read one field of the answer.
    pub fn effect(&self) -> EventEffect {
        match self {
            // A scheduled decision task waits in its task list until a decider is
            // handed it, or it times out there first; both events name the task by its
            // `scheduledEventId`, and both close a task that had been handed over by
            // its `startedEventId`.
            Self::DecisionTaskStarted(started) => EventEffect {
                decision_task_left_task_list: Some(started.scheduled_event_id),
                ..Default::default()
            },
            Self::DecisionTaskTimedOut(timed_out) => EventEffect {
                decision_task_left_task_list: Some(timed_out.scheduled_event_id),
                decision_task_closed: Some(timed_out.started_event_id),
                ..Default::default()
            },
            Self::DecisionTaskCompleted(completed) => EventEffect {
                decision_task_closed: Some(completed.started_event_id),
                ..Default::default()
            },
            // A scheduled activity task waits in its task list until a worker is handed
            // it, or it leaves the list without ever being handed over; both events name
            // the task by its `scheduledEventId`.
            Self::ActivityTaskStarted(started) => EventEffect {
                activity_task_left_task_list: Some(started.scheduled_event_id),
                ..Default::default()
            },
            // A task that timed out is over however far it had got — waiting in the list
            // or already with a worker — and the decider has to see the timeout.
            Self::ActivityTaskTimedOut(timed_out) => EventEffect {
                activity_task_left_task_list: Some(timed_out.scheduled_event_id),
                activity_task_closed: Some(timed_out.scheduled_event_id),
                needs_decision: true,
                ..Default::default()
            },
            // A scheduled activity task that is cancelled before a worker took it both
            // leaves its task list and goes out of flight, and the decider has to see
            // the cancellation.
            Self::ActivityTaskCanceled(canceled) => EventEffect {
                activity_task_left_task_list: Some(canceled.scheduled_event_id),
                activity_task_closed: Some(canceled.scheduled_event_id),
                needs_decision: true,
                ..Default::default()
            },
            // A worker's own terminal report closes the task it was handed, and the
            // decider has to see the result it reported.
            Self::ActivityTaskCompleted(completed) => EventEffect {
                activity_task_left_task_list: Some(completed.scheduled_event_id),
                activity_task_closed: Some(completed.scheduled_event_id),
                needs_decision: true,
                ..Default::default()
            },
            Self::ActivityTaskFailed(failed) => EventEffect {
                activity_task_left_task_list: Some(failed.scheduled_event_id),
                activity_task_closed: Some(failed.scheduled_event_id),
                needs_decision: true,
                ..Default::default()
            },
            Self::TimerFired(fired) => EventEffect {
                timer_closed: Some(fired.started_event_id),
                needs_decision: true,
                ..Default::default()
            },
            Self::TimerCanceled(canceled) => EventEffect {
                timer_closed: Some(canceled.started_event_id),
                ..Default::default()
            },
            Self::StartLambdaFunctionFailed(failed) => EventEffect {
                lambda_function_closed: Some(failed.scheduled_event_id),
                needs_decision: true,
                ..Default::default()
            },
            // Everything a decider has to act on but that takes nothing out of flight:
            // what arrived from outside the run, what a decision of its own failed on,
            // and what a child run reported back.
            Self::WorkflowExecutionSignaled(_)
            | Self::WorkflowExecutionCancelRequested(_)
            | Self::ScheduleActivityTaskFailed(_)
            | Self::RequestCancelActivityTaskFailed(_)
            | Self::StartTimerFailed(_)
            | Self::CancelTimerFailed(_)
            | Self::CompleteWorkflowExecutionFailed(_)
            | Self::FailWorkflowExecutionFailed(_)
            | Self::CancelWorkflowExecutionFailed(_)
            | Self::ContinueAsNewWorkflowExecutionFailed(_)
            | Self::StartChildWorkflowExecutionFailed(_)
            | Self::ChildWorkflowExecutionCompleted(_)
            | Self::ChildWorkflowExecutionFailed(_)
            | Self::ChildWorkflowExecutionCanceled(_)
            | Self::ChildWorkflowExecutionTimedOut(_)
            | Self::ChildWorkflowExecutionTerminated(_)
            | Self::ExternalWorkflowExecutionSignaled(_)
            | Self::SignalExternalWorkflowExecutionFailed(_)
            | Self::ExternalWorkflowExecutionCancelRequested(_)
            | Self::RequestCancelExternalWorkflowExecutionFailed(_)
            | Self::ScheduleLambdaFunctionFailed(_) => EventEffect {
                needs_decision: true,
                ..Default::default()
            },
            // The one event each way of closing a run records: nothing of the run is in
            // flight afterwards, and there is no decider left to tell.
            Self::WorkflowExecutionCompleted(_)
            | Self::WorkflowExecutionFailed(_)
            | Self::WorkflowExecutionCanceled(_)
            | Self::WorkflowExecutionContinuedAsNew(_)
            | Self::WorkflowExecutionTerminated(_)
            | Self::WorkflowExecutionTimedOut(_) => EventEffect {
                closes_run: true,
                ..Default::default()
            },
            // Everything else either starts work the decider is now waiting on, or
            // records what has already happened to the run as a whole.
            Self::WorkflowExecutionStarted(_)
            | Self::DecisionTaskScheduled(_)
            | Self::ActivityTaskScheduled(_)
            | Self::ActivityTaskCancelRequested(_)
            | Self::MarkerRecorded(_)
            | Self::TimerStarted(_)
            | Self::StartChildWorkflowExecutionInitiated(_)
            | Self::ChildWorkflowExecutionStarted(_)
            | Self::SignalExternalWorkflowExecutionInitiated(_)
            | Self::RequestCancelExternalWorkflowExecutionInitiated(_)
            | Self::LambdaFunctionScheduled(_) => EventEffect::default(),
        }
    }

    /// The `scheduledEventId` of the decision task these attributes take OUT of its
    /// task list, when that is what the event records.
    pub fn decision_task_left_task_list(&self) -> Option<i64> {
        self.effect().decision_task_left_task_list
    }

    /// The `startedEventId` of the decision task these attributes CLOSE.
    pub fn decision_task_closed(&self) -> Option<i64> {
        self.effect().decision_task_closed
    }

    /// The `scheduledEventId` of the activity task these attributes take OUT of its
    /// task list, when that is what the event records.
    pub fn activity_task_left_task_list(&self) -> Option<i64> {
        self.effect().activity_task_left_task_list
    }
}

/// One event of a run's persisted history.
#[derive(Debug, Clone)]
pub struct HistoryEventRecord {
    pub event_id: i64,
    pub event_timestamp: DateTime,
    pub attributes: EventAttributes,
}

impl HistoryEventRecord {
    /// The modelled `HistoryEvent` for this record: the `eventType` the variant names,
    /// and the one matching `*EventAttributes` member populated. Every other
    /// attributes member stays absent — an event carries exactly one.
    pub fn event(&self) -> HistoryEvent {
        let mut event = blank_history_event(
            self.event_id,
            self.event_timestamp,
            self.attributes.event_type(),
        );
        match &self.attributes {
            EventAttributes::WorkflowExecutionStarted(attributes) => {
                event.workflow_execution_started_event_attributes =
                    Some(attributes.as_ref().clone());
            }
            EventAttributes::DecisionTaskScheduled(attributes) => {
                event.decision_task_scheduled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::DecisionTaskStarted(attributes) => {
                event.decision_task_started_event_attributes = Some(attributes.clone());
            }
            EventAttributes::DecisionTaskCompleted(attributes) => {
                event.decision_task_completed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::DecisionTaskTimedOut(attributes) => {
                event.decision_task_timed_out_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskScheduled(attributes) => {
                event.activity_task_scheduled_event_attributes = Some(attributes.as_ref().clone());
            }
            EventAttributes::ScheduleActivityTaskFailed(attributes) => {
                event.schedule_activity_task_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskStarted(attributes) => {
                event.activity_task_started_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskTimedOut(attributes) => {
                event.activity_task_timed_out_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskCancelRequested(attributes) => {
                event.activity_task_cancel_requested_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskCanceled(attributes) => {
                event.activity_task_canceled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskCompleted(attributes) => {
                event.activity_task_completed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ActivityTaskFailed(attributes) => {
                event.activity_task_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::RequestCancelActivityTaskFailed(attributes) => {
                event.request_cancel_activity_task_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::MarkerRecorded(attributes) => {
                event.marker_recorded_event_attributes = Some(attributes.clone());
            }
            EventAttributes::TimerStarted(attributes) => {
                event.timer_started_event_attributes = Some(attributes.clone());
            }
            EventAttributes::TimerFired(attributes) => {
                event.timer_fired_event_attributes = Some(attributes.clone());
            }
            EventAttributes::TimerCanceled(attributes) => {
                event.timer_canceled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::StartTimerFailed(attributes) => {
                event.start_timer_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::CancelTimerFailed(attributes) => {
                event.cancel_timer_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionSignaled(attributes) => {
                event.workflow_execution_signaled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionCompleted(attributes) => {
                event.workflow_execution_completed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::CompleteWorkflowExecutionFailed(attributes) => {
                event.complete_workflow_execution_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionFailed(attributes) => {
                event.workflow_execution_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::FailWorkflowExecutionFailed(attributes) => {
                event.fail_workflow_execution_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionCanceled(attributes) => {
                event.workflow_execution_canceled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::CancelWorkflowExecutionFailed(attributes) => {
                event.cancel_workflow_execution_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionContinuedAsNew(attributes) => {
                event.workflow_execution_continued_as_new_event_attributes =
                    Some(attributes.as_ref().clone());
            }
            EventAttributes::ContinueAsNewWorkflowExecutionFailed(attributes) => {
                event.continue_as_new_workflow_execution_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionTerminated(attributes) => {
                event.workflow_execution_terminated_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionTimedOut(attributes) => {
                event.workflow_execution_timed_out_event_attributes = Some(attributes.clone());
            }
            EventAttributes::WorkflowExecutionCancelRequested(attributes) => {
                event.workflow_execution_cancel_requested_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::StartChildWorkflowExecutionInitiated(attributes) => {
                event.start_child_workflow_execution_initiated_event_attributes =
                    Some(attributes.as_ref().clone());
            }
            EventAttributes::StartChildWorkflowExecutionFailed(attributes) => {
                event.start_child_workflow_execution_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionStarted(attributes) => {
                event.child_workflow_execution_started_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionCompleted(attributes) => {
                event.child_workflow_execution_completed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionFailed(attributes) => {
                event.child_workflow_execution_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionCanceled(attributes) => {
                event.child_workflow_execution_canceled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionTimedOut(attributes) => {
                event.child_workflow_execution_timed_out_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::ChildWorkflowExecutionTerminated(attributes) => {
                event.child_workflow_execution_terminated_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::SignalExternalWorkflowExecutionInitiated(attributes) => {
                event.signal_external_workflow_execution_initiated_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::ExternalWorkflowExecutionSignaled(attributes) => {
                event.external_workflow_execution_signaled_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::SignalExternalWorkflowExecutionFailed(attributes) => {
                event.signal_external_workflow_execution_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::RequestCancelExternalWorkflowExecutionInitiated(attributes) => {
                event.request_cancel_external_workflow_execution_initiated_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::ExternalWorkflowExecutionCancelRequested(attributes) => {
                event.external_workflow_execution_cancel_requested_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::RequestCancelExternalWorkflowExecutionFailed(attributes) => {
                event.request_cancel_external_workflow_execution_failed_event_attributes =
                    Some(attributes.clone());
            }
            EventAttributes::LambdaFunctionScheduled(attributes) => {
                event.lambda_function_scheduled_event_attributes = Some(attributes.clone());
            }
            EventAttributes::ScheduleLambdaFunctionFailed(attributes) => {
                event.schedule_lambda_function_failed_event_attributes = Some(attributes.clone());
            }
            EventAttributes::StartLambdaFunctionFailed(attributes) => {
                event.start_lambda_function_failed_event_attributes = Some(attributes.clone());
            }
        }
        event
    }
}

/// A `HistoryEvent` with its identity set and EVERY `*EventAttributes` member absent.
/// `HistoryEvent` models no default (its `eventTimestamp`/`eventType`/`eventId` are
/// required), and it carries one optional member per event type, so this is the single
/// place that enumerates them — [`HistoryEventRecord::event`] then fills in the one
/// that matches.
fn blank_history_event(event_id: i64, event_timestamp: DateTime, event_type: &str) -> HistoryEvent {
    HistoryEvent {
        event_timestamp,
        event_type: event_type.to_string(),
        event_id,
        workflow_execution_started_event_attributes: None,
        workflow_execution_completed_event_attributes: None,
        complete_workflow_execution_failed_event_attributes: None,
        workflow_execution_failed_event_attributes: None,
        fail_workflow_execution_failed_event_attributes: None,
        workflow_execution_timed_out_event_attributes: None,
        workflow_execution_canceled_event_attributes: None,
        cancel_workflow_execution_failed_event_attributes: None,
        workflow_execution_continued_as_new_event_attributes: None,
        continue_as_new_workflow_execution_failed_event_attributes: None,
        workflow_execution_terminated_event_attributes: None,
        workflow_execution_cancel_requested_event_attributes: None,
        decision_task_scheduled_event_attributes: None,
        decision_task_started_event_attributes: None,
        decision_task_completed_event_attributes: None,
        decision_task_timed_out_event_attributes: None,
        activity_task_scheduled_event_attributes: None,
        activity_task_started_event_attributes: None,
        activity_task_completed_event_attributes: None,
        activity_task_failed_event_attributes: None,
        activity_task_timed_out_event_attributes: None,
        activity_task_canceled_event_attributes: None,
        activity_task_cancel_requested_event_attributes: None,
        workflow_execution_signaled_event_attributes: None,
        marker_recorded_event_attributes: None,
        record_marker_failed_event_attributes: None,
        timer_started_event_attributes: None,
        timer_fired_event_attributes: None,
        timer_canceled_event_attributes: None,
        start_child_workflow_execution_initiated_event_attributes: None,
        child_workflow_execution_started_event_attributes: None,
        child_workflow_execution_completed_event_attributes: None,
        child_workflow_execution_failed_event_attributes: None,
        child_workflow_execution_timed_out_event_attributes: None,
        child_workflow_execution_canceled_event_attributes: None,
        child_workflow_execution_terminated_event_attributes: None,
        signal_external_workflow_execution_initiated_event_attributes: None,
        external_workflow_execution_signaled_event_attributes: None,
        signal_external_workflow_execution_failed_event_attributes: None,
        external_workflow_execution_cancel_requested_event_attributes: None,
        request_cancel_external_workflow_execution_initiated_event_attributes: None,
        request_cancel_external_workflow_execution_failed_event_attributes: None,
        schedule_activity_task_failed_event_attributes: None,
        request_cancel_activity_task_failed_event_attributes: None,
        start_timer_failed_event_attributes: None,
        cancel_timer_failed_event_attributes: None,
        start_child_workflow_execution_failed_event_attributes: None,
        lambda_function_scheduled_event_attributes: None,
        lambda_function_started_event_attributes: None,
        lambda_function_completed_event_attributes: None,
        lambda_function_failed_event_attributes: None,
        lambda_function_timed_out_event_attributes: None,
        schedule_lambda_function_failed_event_attributes: None,
        start_lambda_function_failed_event_attributes: None,
    }
}

/// How a run CLOSED: the close event the service records for it, and the `closeStatus`
/// that close leaves on the record.
///
/// Every close goes through [`Domain::close_run`] carrying one of these, so the close
/// event, the `closeStatus`, the domain's open-run index, the child policy and the
/// parent run's notification can never disagree about how a run ended.
#[derive(Debug, Clone)]
pub enum RunClose {
    /// A `CompleteWorkflowExecution` decision: "Closes the workflow execution and
    /// records a `WorkflowExecutionCompleted` event in the history".
    Completed {
        result: Option<String>,
        decision_task_completed_event_id: i64,
    },
    /// A `FailWorkflowExecution` decision.
    Failed {
        reason: Option<String>,
        details: Option<String>,
        decision_task_completed_event_id: i64,
    },
    /// A `CancelWorkflowExecution` decision.
    Canceled {
        details: Option<String>,
        decision_task_completed_event_id: i64,
    },
    /// A `ContinueAsNewWorkflowExecution` decision: this run closes and the run its
    /// attributes name carries on in its place.
    ContinuedAsNew(Box<WorkflowExecutionContinuedAsNewEventAttributes>),
    /// The run's own `executionStartToCloseTimeout` ran out.
    TimedOut,
    /// A terminate: a caller's, or the service's own when it carries a `cause`.
    Terminated {
        reason: Option<String>,
        details: Option<String>,
        cause: Option<String>,
        /// The policy to apply to this run's open children, when the terminate named
        /// one; `None` leaves the run's own `childPolicy` in force.
        child_policy: Option<String>,
    },
}

impl RunClose {
    /// The close of a run that has reached the history-size quota: the service's own
    /// terminate, carrying the cause that says so.
    ///
    /// `WorkflowExecutionTerminatedCause` models exactly this cause, and `cause`'s own
    /// doc — "If set, indicates that the workflow execution was automatically
    /// terminated, and specifies the cause" — says the service, not a caller, did it.
    pub fn event_limit_exceeded() -> Self {
        Self::Terminated {
            reason: None,
            details: None,
            cause: Some(TERMINATED_CAUSE_EVENT_LIMIT_EXCEEDED.to_string()),
            child_policy: None,
        }
    }

    /// The `closeStatus` this close leaves on the record — one of
    /// [`crate::ops::types::CLOSE_STATUS_VALUES`].
    pub fn close_status(&self) -> &'static str {
        match self {
            Self::Completed { .. } => CLOSE_STATUS_COMPLETED,
            Self::Failed { .. } => CLOSE_STATUS_FAILED,
            Self::Canceled { .. } => CLOSE_STATUS_CANCELED,
            Self::ContinuedAsNew(_) => CLOSE_STATUS_CONTINUED_AS_NEW,
            Self::TimedOut => CLOSE_STATUS_TIMED_OUT,
            Self::Terminated { .. } => CLOSE_STATUS_TERMINATED,
        }
    }

    /// The policy this close applies to the run's open child executions: the one the
    /// close named, else the run's own `childPolicy`.
    fn child_policy(&self, run_policy: &str) -> String {
        match self {
            Self::Terminated {
                child_policy: Some(policy),
                ..
            } => policy.clone(),
            _ => run_policy.to_string(),
        }
    }

    /// Whether this close is one the run's `childPolicy` applies to: `childPolicy`
    /// "specifies the policy to use for the child workflow executions of this workflow
    /// execution IF IT IS TERMINATED, by calling the TerminateWorkflowExecution action
    /// explicitly or due to an expired timeout" — so a run that completed, failed, was
    /// cancelled or continued as new leaves its children running.
    fn applies_child_policy(&self) -> bool {
        matches!(self, Self::Terminated { .. } | Self::TimedOut)
    }

    /// The `reason` this close carried, which only a failure has.
    fn closed_reason(&self) -> Option<String> {
        match self {
            Self::Failed { reason, .. }
            | Self::Terminated { reason, .. } => reason.clone(),
            _ => None,
        }
    }

    /// The result or details this close carried — what a parent run is told its child
    /// ended with.
    fn closed_payload(&self) -> Option<String> {
        match self {
            Self::Completed { result, .. } => result.clone(),
            Self::Failed { details, .. }
            | Self::Canceled { details, .. }
            | Self::Terminated { details, .. } => details.clone(),
            Self::ContinuedAsNew(_) | Self::TimedOut => None,
        }
    }

    /// The one history event this close records, carrying the run's own `childPolicy`
    /// where the event models it.
    fn event(self, run_policy: String) -> EventAttributes {
        let child_policy = self.child_policy(&run_policy);
        match self {
            Self::Completed {
                result,
                decision_task_completed_event_id,
            } => EventAttributes::WorkflowExecutionCompleted(
                WorkflowExecutionCompletedEventAttributes {
                    result,
                    decision_task_completed_event_id,
                },
            ),
            Self::Failed {
                reason,
                details,
                decision_task_completed_event_id,
            } => {
                EventAttributes::WorkflowExecutionFailed(WorkflowExecutionFailedEventAttributes {
                    reason,
                    details,
                    decision_task_completed_event_id,
                })
            }
            Self::Canceled {
                details,
                decision_task_completed_event_id,
            } => EventAttributes::WorkflowExecutionCanceled(
                WorkflowExecutionCanceledEventAttributes {
                    details,
                    decision_task_completed_event_id,
                },
            ),
            Self::ContinuedAsNew(attributes) => {
                EventAttributes::WorkflowExecutionContinuedAsNew(attributes)
            }
            Self::TimedOut => EventAttributes::WorkflowExecutionTimedOut(
                WorkflowExecutionTimedOutEventAttributes {
                    timeout_type: WORKFLOW_EXECUTION_TIMEOUT_START_TO_CLOSE.to_string(),
                    child_policy,
                },
            ),
            Self::Terminated {
                reason,
                details,
                cause,
                ..
            } => EventAttributes::WorkflowExecutionTerminated(
                WorkflowExecutionTerminatedEventAttributes {
                    reason,
                    details,
                    child_policy,
                    cause,
                },
            ),
        }
    }
}

/// The run a start is asked for: everything the stored record needs that the caller has
/// already resolved. Passed to [`Domain::start_run`], the one place a run is created.
#[derive(Debug, Clone)]
pub struct NewRun {
    pub workflow_id: String,
    /// The registered type this run is of; its `(name, version)` is what the stored
    /// record is keyed by.
    pub workflow_type: WorkflowType,
    pub input: Option<String>,
    pub tag_list: Vec<String>,
    /// The timeouts, task list and child policy the run actually runs under, resolved
    /// once at start from the request and the type's registered defaults.
    pub config: ExecutionConfig,
    /// The `runId` this run continues, when a `ContinueAsNewWorkflowExecution` decision
    /// started it.
    pub continued_execution_run_id: Option<String>,
    /// The run that started this one as a child and the `eventId` of the
    /// `StartChildWorkflowExecutionInitiated` event that did.
    pub parent: Option<(WorkflowExecution, i64)>,
}

/// What a run that just closed ended with, read off it before it was closed, so the
/// parent run can be told (see [`Domain::close_run`]).
#[derive(Debug, Clone)]
struct CloseOutcome {
    /// The `closeStatus` the close left — one of
    /// [`crate::ops::types::CLOSE_STATUS_VALUES`].
    status: &'static str,
    reason: Option<String>,
    payload: Option<String>,
    execution: WorkflowExecution,
    workflow_type: WorkflowType,
    parent: Option<WorkflowExecution>,
    parent_initiated_event_id: i64,
}

/// One workflow execution — a `(workflowId, runId)` pair — and its history.
#[derive(Debug, Clone)]
pub struct Execution {
    pub workflow_id: String,
    pub run_id: String,
    /// The `(name, version)` of the workflow type this run was started with.
    pub workflow_type: (String, String),
    /// One of [`crate::ops::types::EXECUTION_STATUS_VALUES`].
    pub status: String,
    /// One of [`crate::ops::types::CLOSE_STATUS_VALUES`], set only once closed.
    pub close_status: Option<String>,
    pub start_timestamp: DateTime,
    pub close_timestamp: Option<DateTime>,
    pub tag_list: Vec<String>,
    pub cancel_requested: bool,
    pub input: Option<String>,
    pub effective_config: ExecutionConfig,
    /// The persisted event log, oldest first. Append only through
    /// [`Execution::append_event`], which is the single `event_id` allocator.
    pub history: Vec<HistoryEventRecord>,
    /// The id the NEXT appended event takes; ids therefore stay unique and strictly
    /// increasing within a run. The first event of a run is `1`.
    pub next_event_id: i64,
    pub open_decision_tasks: i32,
    pub open_activity_tasks: i32,
    pub open_timers: i32,
    pub open_child_executions: i32,
    pub open_lambda_functions: i32,
    pub latest_execution_context: Option<String>,
    /// When an activity task of this run was last scheduled or handed to a worker — what
    /// `DescribeWorkflowExecution`'s `latestActivityTaskTimestamp` reports. Written by the
    /// `ScheduleActivityTask` decision and by the poll that hands a task over.
    pub latest_activity_task_timestamp: Option<DateTime>,
    /// The run that started this one as a child, and the `eventId` of the
    /// `StartChildWorkflowExecutionInitiated` event that did — what
    /// `WorkflowExecutionInfo.parent` and the started event's own
    /// `parentWorkflowExecution`/`parentInitiatedEventId` report, and who is told when
    /// this run closes. `None` for a run a client started directly.
    pub parent: Option<WorkflowExecution>,
    pub parent_initiated_event_id: i64,
    /// A task-list override that is in force only for as long as its timeout allows
    /// (`RespondDecisionTaskCompleted`'s `taskListScheduleToStartTimeout`); `None` when
    /// the run's task list is simply the one it runs under.
    pub task_list_override: Option<TaskListOverride>,
    /// The LAST progress report a worker made about one activity task of this run,
    /// keyed by the `eventId` of the `ActivityTaskStarted` event the task's token is
    /// derived from. Written by `RecordActivityTaskHeartbeat`, which "resets the
    /// taskHeartbeatTimeout clock" and "doesn't in itself create an event in the
    /// workflow execution history" — so the report lives here rather than in `history`,
    /// and the entry of a task that has closed is dropped with it.
    pub activity_task_heartbeats: HashMap<i64, ActivityTaskHeartbeat>,
}

/// One `RecordActivityTaskHeartbeat` call's lasting effect on the activity task it
/// reported progress for.
#[derive(Debug, Clone)]
pub struct ActivityTaskHeartbeat {
    /// When the report was made — the instant the task's `heartbeatTimeout` is measured
    /// from from then on, because the call "resets the taskHeartbeatTimeout clock".
    pub at: DateTime,
    /// The `details` the worker reported ("If specified, contains details about the
    /// progress of the task"), which `ActivityTaskTimedOut` carries if the task does
    /// time out: its own `details` is "the content of the details parameter for the last
    /// call made by the activity to RecordActivityTaskHeartbeat".
    pub details: Option<String>,
}

/// A `taskList` override a decider asked for WITH a
/// `taskListScheduleToStartTimeout`: "This parameter makes it possible to temporarily
/// override the task list. If a decision task scheduled on the override task list is
/// not started within the timeout, the decision task will time out. Amazon SWF will
/// revert the override and schedule a new decision task to the original task list."
#[derive(Debug, Clone)]
pub struct TaskListOverride {
    /// The task list the run goes back to when a decision task on the override list is
    /// not started in time.
    pub revert_to: String,
    /// The schedule-to-start timeout every decision task scheduled while the override
    /// is in force carries.
    pub schedule_to_start_timeout: String,
}

impl Execution {
    /// Append one event to this run's history and return the id it was given. The ONLY
    /// writer of `history`/`next_event_id`, so no two events of a run share an id.
    pub fn append_event(&mut self, at: DateTime, attributes: EventAttributes) -> i64 {
        let event_id = self.next_event_id;
        self.next_event_id += 1;
        self.history.push(HistoryEventRecord {
            event_id,
            event_timestamp: at,
            attributes,
        });
        event_id
    }

    /// The `WorkflowExecution` identity this run is keyed by.
    pub fn execution(&self) -> WorkflowExecution {
        WorkflowExecution {
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id.clone(),
        }
    }

    /// The `WorkflowExecutionInfo` every execution reader returns, built from what is
    /// stored.
    pub fn info(&self) -> WorkflowExecutionInfo {
        let closed_by = self.closing_event();
        WorkflowExecutionInfo {
            execution: self.execution(),
            workflow_type: WorkflowType {
                name: self.workflow_type.0.clone(),
                version: self.workflow_type.1.clone(),
            },
            start_timestamp: self.start_timestamp,
            close_timestamp: self.close_timestamp,
            execution_status: self.status.clone(),
            close_status: self.close_status.clone(),
            // The run that started this one as a child, when a
            // `StartChildWorkflowExecution` decision did.
            parent: self.parent.clone(),
            tag_list: (!self.tag_list.is_empty()).then(|| self.tag_list.clone()),
            // "Set to true if a cancellation is REQUESTED for this workflow execution":
            // the flag reports a request still outstanding against a running execution,
            // so a run that is no longer OPEN has none outstanding whatever was asked of
            // it while it ran.
            cancel_requested: self.cancel_requested && self.is_open(),
            // The event that closed the run and the instant that event was stamped at. A
            // run that is still open has closed at no event, which the member's own
            // modelled default of `0` reports, and at no instant at all.
            last_closed_event_id: closed_by.map_or(0, |event| event.event_id),
            last_closed_timestamp: closed_by.map(|event| event.event_timestamp),
            // Reopening a closed run is not an operation this service models, so no
            // stored run has ever been reopened: every one of them is on its first
            // generation, and none of them is reopenable.
            reopen_generation_id: FIRST_RUN_GENERATION,
            reopenable: false,
        }
    }

    /// The history event that CLOSED this run — the one event the close recorded (see
    /// [`EventEffect::closes_run`]) — when this run has closed at all.
    ///
    /// Read off the stored history rather than kept beside it, so the id and the instant
    /// `WorkflowExecutionInfo` reports can never disagree with the event itself. A run
    /// that is still OPEN has recorded no such event, and answers without walking its
    /// history at all.
    pub fn closing_event(&self) -> Option<&HistoryEventRecord> {
        if self.is_open() {
            return None;
        }
        self.history
            .iter()
            .rev()
            .find(|event| event.attributes.effect().closes_run)
    }

    /// The `WorkflowExecutionOpenCounts` `DescribeWorkflowExecution` reports, counted
    /// from this run's live state.
    ///
    /// Each counter is the work of this run whose own status is OPEN —
    /// `openActivityTasks` is "The count of activity tasks whose status is OPEN" — so the
    /// event that closes one piece of work is what takes it out of its counter. Closing
    /// the RUN takes nothing out: a decision task scheduled and never answered is still
    /// outstanding at the instant the run closed, and is still reported.
    pub fn open_counts(&self) -> WorkflowExecutionOpenCounts {
        WorkflowExecutionOpenCounts {
            open_activity_tasks: self.open_activity_tasks,
            open_decision_tasks: self.open_decision_tasks,
            open_timers: self.open_timers,
            open_child_workflow_executions: self.open_child_executions,
            open_lambda_functions: self.open_lambda_functions,
        }
    }

    /// Whether this run is still OPEN.
    pub fn is_open(&self) -> bool {
        self.status == EXECUTION_STATUS_OPEN
    }

    /// The instant this run's start-to-close budget runs out: the instant it started
    /// plus the `executionStartToCloseTimeout` it resolved at start.
    ///
    /// The budget keeps the value the caller asked for — `executionStartToCloseTimeout`
    /// is "The total duration for this workflow execution", in seconds — so it is read
    /// off the stored configuration rather than chosen here. The one exception is the
    /// published lower bound of that member ("an integer greater than or equal to 0"):
    /// a zero budget would have the run close in the same instant it opened, so no
    /// caller could ever observe the OPEN run `StartWorkflowExecution` reports, and it
    /// is floored at [`MIN_OPEN_EXECUTION_SPAN`]. Every value of one second or more is
    /// exact.
    ///
    /// `None` when the stored duration is not a number of seconds, which is the only
    /// way a run can have no deadline at all.
    pub fn start_to_close_deadline(&self) -> Option<DateTime> {
        let budget = Duration::from_secs(decimal(
            &self.effective_config.execution_start_to_close_timeout,
        )?)
        .max(MIN_OPEN_EXECUTION_SPAN);
        instant_after(self.start_timestamp, budget)
    }

    /// The instant at which the clock already owed this run a start-to-close timeout,
    /// when it is still open and that instant is no later than `at`; `None` when
    /// nothing is owed.
    ///
    /// The answer is the DEADLINE, not `at`: the timeout happened when the budget ran
    /// out, whether or not anybody called in that instant, so the event and the close
    /// instant it produces are stamped there.
    pub fn due_start_to_close_timeout(&self, at: DateTime) -> Option<DateTime> {
        if !self.is_open() {
            return None;
        }
        let due = self.start_to_close_deadline()?;
        (instant_key(due) <= instant_key(at)).then_some(due)
    }

    /// Close this run as `close` describes, as of `at`: the one close event that kind of
    /// close records, and `executionStatus=CLOSED` with the matching `closeStatus`.
    ///
    /// Reached only through [`Domain::close_run`], which is what also drops the run from
    /// its domain's open-run index and tells its parent run; closing it here alone would
    /// leave those two records disagreeing with this one.
    fn close(&mut self, at: DateTime, close: RunClose) {
        self.status = EXECUTION_STATUS_CLOSED.to_string();
        self.close_status = Some(close.close_status().to_string());
        self.close_timestamp = Some(at);
        // The open counts are NOT cleared here: each of them counts the work of this run
        // whose own status is OPEN (`openActivityTasks` is "The count of activity tasks
        // whose status is OPEN"), and the event that closes one piece of work is what
        // takes it out of its counter. A decision task scheduled and never answered is
        // still outstanding at the instant the run closes, so it stays counted.
        let child_policy = self.effective_config.child_policy.clone();
        self.append_event(at, close.event(child_policy));
        // No task of a closed run can time out or report again, so the progress reports
        // kept only to measure a live task's heartbeat clock are dropped with the run.
        self.activity_task_heartbeats.clear();
    }

    /// Whether this run's history can take `events` more events and still hold the one
    /// `WorkflowExecutionTerminated` event the service appends when it cannot, without
    /// passing [`MAX_HISTORY_EVENTS_PER_EXECUTION`].
    ///
    /// Every caller-driven append asks this first, which is what bounds the memory one
    /// run's history can hold however many times a caller signals or polls it.
    pub fn has_history_room_for(&self, events: usize) -> bool {
        self.history.len() + events < MAX_HISTORY_EVENTS_PER_EXECUTION
    }

    /// How many of this run's decision tasks are still WAITING in `task_list`: one per
    /// `DecisionTaskScheduled` event of its history that names that list and that no
    /// later event has taken out of it (see
    /// [`EventAttributes::decision_task_left_task_list`]).
    ///
    /// The history IS the record of which tasks were scheduled where — the scheduling
    /// event carries its own `taskList`, so a run whose decision tasks were scheduled
    /// on more than one list contributes to each list separately.
    pub fn pending_decision_tasks_on(&self, task_list: &str) -> usize {
        let taken: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.decision_task_left_task_list())
            .collect();
        self.history
            .iter()
            .filter(|event| match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => {
                    scheduled.task_list.name == task_list && !taken.contains(&event.event_id)
                }
                _ => false,
            })
            .count()
    }

    /// How many of this run's activity tasks are still WAITING in `task_list`: one per
    /// `ActivityTaskScheduled` event of its history that names that list and that no
    /// later event has taken out of it (see
    /// [`EventAttributes::activity_task_left_task_list`]).
    ///
    /// The same record and the same reasoning as
    /// [`Execution::pending_decision_tasks_on`]: the history IS the record of which
    /// tasks were scheduled where, each scheduling event carrying its own `taskList`, so
    /// a run that scheduled activity tasks on more than one list contributes to each
    /// list separately.
    pub fn pending_activity_tasks_on(&self, task_list: &str) -> usize {
        self.waiting_activity_tasks_on(task_list).len()
    }

    /// The activity tasks of this run still WAITING in `task_list`, as the
    /// `ActivityTaskScheduled` events that put them there — the same record
    /// [`Execution::pending_activity_tasks_on`] counts, in the order the events were
    /// recorded.
    pub fn waiting_activity_tasks_on(&self, task_list: &str) -> Vec<&HistoryEventRecord> {
        self.waiting_activity_tasks()
            .into_iter()
            .filter(|event| match &event.attributes {
                EventAttributes::ActivityTaskScheduled(scheduled) => {
                    scheduled.task_list.name == task_list
                }
                _ => false,
            })
            .collect()
    }

    /// Every activity task of this run still waiting in a task list, whichever list that
    /// is: the `ActivityTaskScheduled` events no later event has taken out of one.
    pub fn waiting_activity_tasks(&self) -> Vec<&HistoryEventRecord> {
        let taken: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.activity_task_left_task_list())
            .collect();
        self.history
            .iter()
            .filter(|event| {
                matches!(event.attributes, EventAttributes::ActivityTaskScheduled(_))
                    && !taken.contains(&event.event_id)
            })
            .collect()
    }

    /// The decision tasks of this run still WAITING in `task_list`, as the
    /// `DecisionTaskScheduled` events that put them there — the same record
    /// [`Execution::pending_decision_tasks_on`] counts, in the order the events were
    /// recorded.
    pub fn waiting_decision_tasks_on(&self, task_list: &str) -> Vec<&HistoryEventRecord> {
        self.waiting_decision_tasks()
            .into_iter()
            .filter(|event| match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => {
                    scheduled.task_list.name == task_list
                }
                _ => false,
            })
            .collect()
    }

    /// Every decision task of this run still waiting in a task list, whichever list
    /// that is: the `DecisionTaskScheduled` events no later event has taken out of one.
    pub fn waiting_decision_tasks(&self) -> Vec<&HistoryEventRecord> {
        let taken: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.decision_task_left_task_list())
            .collect();
        self.history
            .iter()
            .filter(|event| {
                matches!(event.attributes, EventAttributes::DecisionTaskScheduled(_))
                    && !taken.contains(&event.event_id)
            })
            .collect()
    }

    /// The `scheduledEventId` of the activity task of this run with `activity_id` that
    /// is still in flight — scheduled and neither finished nor cancelled — which is
    /// what makes a second schedule of that id a collision and a cancel of an id with
    /// none unknown.
    pub fn open_activity_task(&self, activity_id: &str) -> Option<i64> {
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.effect().activity_task_closed)
            .collect();
        self.history
            .iter()
            .rev()
            .find_map(|event| match &event.attributes {
                EventAttributes::ActivityTaskScheduled(scheduled)
                    if scheduled.activity_id == activity_id
                        && !closed.contains(&event.event_id) =>
                {
                    Some(event.event_id)
                }
                _ => None,
            })
    }

    /// Whether the activity task scheduled by `scheduled_event_id` has been handed to a
    /// worker: its history holds the `ActivityTaskStarted` event a poll of its task list
    /// records. A task no worker has been handed is cancelled outright, while one already
    /// assigned is told through its heartbeat and reports back itself.
    pub fn activity_task_is_started(&self, scheduled_event_id: i64) -> bool {
        self.history.iter().any(|event| match &event.attributes {
            EventAttributes::ActivityTaskStarted(started) => {
                started.scheduled_event_id == scheduled_event_id
            }
            _ => false,
        })
    }

    /// The activity tasks of this run a worker holds and that are still OUTSTANDING: one
    /// `(scheduledEventId, startedEventId, startedAt)` per `ActivityTaskStarted` event
    /// that no later event has closed the task of.
    ///
    /// The record is the stored history, so a token's liveness is a property of it rather
    /// than of a second record that could disagree
    /// ([`Execution::activity_task_is_outstanding`]).
    pub fn outstanding_activity_tasks(&self) -> Vec<(i64, i64, DateTime)> {
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.effect().activity_task_closed)
            .collect();
        self.history
            .iter()
            .filter_map(|event| match &event.attributes {
                EventAttributes::ActivityTaskStarted(started)
                    if !closed.contains(&started.scheduled_event_id) =>
                {
                    Some((
                        started.scheduled_event_id,
                        event.event_id,
                        event.event_timestamp,
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether the activity task recorded by `ActivityTaskStarted` event
    /// `started_event_id` is still outstanding — what makes an activity task token a live
    /// handle rather than a stale one.
    pub fn activity_task_is_outstanding(&self, started_event_id: i64) -> bool {
        self.outstanding_activity_tasks()
            .iter()
            .any(|(_, started, _)| *started == started_event_id)
    }

    /// The `eventId` of the last `ActivityTaskCancelRequested` event recorded for the
    /// activity task scheduled by `scheduled_event_id`, which is what
    /// `ActivityTaskCanceled`'s own `latestCancelRequestedEventId` reports ("If set,
    /// contains the ID of the last `ActivityTaskCancelRequested` event recorded for this
    /// activity task"); `0` — the `EventId` member's own modelled default — when a worker
    /// cancelled a task nobody had asked it to.
    ///
    /// Only the events of THIS task's lifetime count: a run may schedule the same
    /// `activityId` again once the earlier task has closed, so the search stops at the
    /// scheduling event of the task being closed and a request made against an earlier
    /// task of the same id is not attributed to this one.
    pub fn latest_cancel_requested_event_id(&self, scheduled_event_id: i64) -> i64 {
        let Some(activity_id) = self
            .scheduled_activity_task(scheduled_event_id)
            .map(|scheduled| scheduled.activity_id.clone())
        else {
            return 0;
        };
        self.history
            .iter()
            .rev()
            .take_while(|event| event.event_id > scheduled_event_id)
            .find_map(|event| match &event.attributes {
                EventAttributes::ActivityTaskCancelRequested(requested)
                    if requested.activity_id == activity_id =>
                {
                    Some(event.event_id)
                }
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The `ActivityTaskScheduled` attributes of the task scheduled by event
    /// `scheduled_event_id`, when this run's history holds one.
    pub fn scheduled_activity_task(
        &self,
        scheduled_event_id: i64,
    ) -> Option<&ActivityTaskScheduledEventAttributes> {
        self.history
            .iter()
            .filter(|event| event.event_id == scheduled_event_id)
            .find_map(|event| match &event.attributes {
                EventAttributes::ActivityTaskScheduled(scheduled) => Some(scheduled.as_ref()),
                _ => None,
            })
    }

    /// Record every activity-task timeout `now` has already made due, and return how many
    /// were recorded.
    ///
    /// Each of the four modelled `ActivityTaskTimeoutType`s is the task's OWN duration,
    /// resolved when it was scheduled, measured from the instant that duration started
    /// running: `scheduleToStartTimeout` and `scheduleToCloseTimeout` from the
    /// `ActivityTaskScheduled` event, `startToCloseTimeout` from the `ActivityTaskStarted`
    /// event that handed the task to a worker, and `heartbeatTimeout` from that worker's
    /// last progress report ([`Execution::activity_task_heartbeats`]), or from the same
    /// started event while it has made none, because a heartbeat "resets the
    /// taskHeartbeatTimeout clock" — so a task is timed out at the instant its earliest
    /// due deadline ran out, whether or not anybody polls, and the `ActivityTaskTimedOut`
    /// event is stamped there. The decider is told by the decision task that event
    /// schedules.
    pub fn expire_due_activity_tasks(&mut self, now: DateTime) -> usize {
        if !self.is_open() {
            return 0;
        }
        let mut due: Vec<(DateTime, &'static str, i64, i64, Option<String>)> = Vec::new();
        for event in self.waiting_activity_tasks() {
            let Some(scheduled) = self.scheduled_activity_task(event.event_id) else {
                continue;
            };
            // A task still in its list has not started, so only the two durations that
            // run from the scheduling instant can have expired.
            let deadlines = [
                (
                    ACTIVITY_TASK_TIMEOUT_SCHEDULE_TO_START,
                    scheduled.schedule_to_start_timeout.as_deref(),
                ),
                (
                    ACTIVITY_TASK_TIMEOUT_SCHEDULE_TO_CLOSE,
                    scheduled.schedule_to_close_timeout.as_deref(),
                ),
            ];
            if let Some((at, timeout_type)) = earliest_due(event.event_timestamp, &deadlines, now)
            {
                // A task still waiting in its list has no worker, so no worker has
                // reported progress on it either.
                due.push((at, timeout_type, event.event_id, 0, None));
            }
        }
        for (scheduled_event_id, started_event_id, started_at) in self.outstanding_activity_tasks()
        {
            let Some(scheduled) = self.scheduled_activity_task(scheduled_event_id) else {
                continue;
            };
            let scheduled_at = self
                .history
                .iter()
                .find(|event| event.event_id == scheduled_event_id)
                .map_or(started_at, |event| event.event_timestamp);
            // The schedule-to-close budget covers the whole task, so it keeps running
            // after a worker takes it; start-to-close starts when the worker does.
            let from_schedule = [(
                ACTIVITY_TASK_TIMEOUT_SCHEDULE_TO_CLOSE,
                scheduled.schedule_to_close_timeout.as_deref(),
            )];
            let from_start = [(
                ACTIVITY_TASK_TIMEOUT_START_TO_CLOSE,
                scheduled.start_to_close_timeout.as_deref(),
            )];
            // The heartbeat budget is the only one a worker can push back: each
            // `RecordActivityTaskHeartbeat` call "resets the taskHeartbeatTimeout
            // clock", so it runs from the last report the worker made, and from the
            // instant it was handed the task while it has made none.
            let heartbeat = self
                .activity_task_heartbeats
                .get(&started_event_id)
                .map(|reported| (reported.at, reported.details.clone()))
                .unwrap_or((started_at, None));
            let from_heartbeat = [(
                ACTIVITY_TASK_TIMEOUT_HEARTBEAT,
                scheduled.heartbeat_timeout.as_deref(),
            )];
            let earliest = [
                earliest_due(scheduled_at, &from_schedule, now),
                earliest_due(started_at, &from_start, now),
                earliest_due(heartbeat.0, &from_heartbeat, now),
            ]
            .into_iter()
            .flatten()
            .min_by_key(|(at, _)| instant_key(*at));
            if let Some((at, timeout_type)) = earliest {
                // Whatever the task timed out under, the event reports the progress its
                // worker last reported.
                due.push((
                    at,
                    timeout_type,
                    scheduled_event_id,
                    started_event_id,
                    heartbeat.1,
                ));
            }
        }
        due.sort_by_key(|(at, _, scheduled_event_id, _, _)| {
            (instant_key(*at), *scheduled_event_id)
        });

        let recorded = due.len();
        for (at, timeout_type, scheduled_event_id, started_event_id, details) in due {
            // The timeout and the decision task telling the decider about it are two
            // events, and a run with no room for them has reached the history quota: it
            // is left for the writer that finds it to close rather than grown past the
            // bound.
            if !self.has_history_room_for(2) {
                return recorded;
            }
            self.append_event(
                at,
                EventAttributes::ActivityTaskTimedOut(ActivityTaskTimedOutEventAttributes {
                    timeout_type: timeout_type.to_string(),
                    scheduled_event_id,
                    started_event_id,
                    // `details` carries "the content of the details parameter for the
                    // last call made by the activity to RecordActivityTaskHeartbeat",
                    // so a task whose worker reported no progress carries none.
                    details,
                }),
            );
            // The task is closed, so the progress report kept for its heartbeat clock
            // has nothing left to time out.
            self.activity_task_heartbeats.remove(&started_event_id);
            self.open_activity_tasks = self.open_activity_tasks.saturating_sub(1);
            self.schedule_decision_task_if_idle(at);
        }
        recorded
    }

    /// The `startedEventId` of the timer of this run with `timer_id` that is still
    /// running — started and neither fired nor cancelled.
    pub fn open_timer(&self, timer_id: &str) -> Option<i64> {
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.effect().timer_closed)
            .collect();
        self.history
            .iter()
            .rev()
            .find_map(|event| match &event.attributes {
                EventAttributes::TimerStarted(started)
                    if started.timer_id == timer_id && !closed.contains(&event.event_id) =>
                {
                    Some(event.event_id)
                }
                _ => None,
            })
    }

    /// The `scheduledEventId` of the Lambda task of this run with `id` that is still in
    /// flight, which is what makes a second schedule of that id a collision.
    pub fn open_lambda_function(&self, id: &str) -> Option<i64> {
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.effect().lambda_function_closed)
            .collect();
        self.history
            .iter()
            .rev()
            .find_map(|event| match &event.attributes {
                EventAttributes::LambdaFunctionScheduled(scheduled)
                    if scheduled.id == id && !closed.contains(&event.event_id) =>
                {
                    Some(event.event_id)
                }
                _ => None,
            })
    }

    /// Whether this run's history holds an event after `event_id` that a decider has to
    /// act on (see [`EventEffect::needs_decision`]).
    ///
    /// Measured from the `DecisionTaskStarted` event of the task a decider is holding,
    /// this is exactly what it has NOT seen: a signal or a result that arrived while it
    /// was deciding, which is why a closing decision taken without it is refused, and
    /// why completing a decision task schedules another one.
    pub fn needs_decision_since(&self, event_id: i64) -> bool {
        self.history
            .iter()
            .any(|event| event.event_id > event_id && event.attributes.effect().needs_decision)
    }

    /// Schedule one decision task on this run's effective task list at `at`, unless a
    /// decision task of this run is already in flight or the run is closed. Returns the
    /// `eventId` of the `DecisionTaskScheduled` event when one was recorded.
    ///
    /// A run that already has a decision task in flight does not get a second: whatever
    /// the new task would carry is in the history that task's decider is handed, which
    /// is why SWF batches what arrives while a decision task is outstanding.
    pub fn schedule_decision_task_if_idle(&mut self, at: DateTime) -> Option<i64> {
        if !self.is_open() || self.open_decision_tasks != 0 || !self.has_history_room_for(1) {
            return None;
        }
        let attributes = DecisionTaskScheduledEventAttributes {
            task_list: TaskList {
                name: self.effective_config.task_list.clone(),
            },
            task_priority: self.effective_config.task_priority.clone(),
            start_to_close_timeout: Some(
                self.effective_config.task_start_to_close_timeout.clone(),
            ),
            // A decision task's schedule-to-start bound is the one a temporary task-list
            // override carries (`taskListScheduleToStartTimeout`) while that override is
            // in force. With no override in force the task waits for a decider for as
            // long as it takes, which is what SWF's unlimited-duration sentinel says.
            schedule_to_start_timeout: Some(
                self.task_list_override
                    .as_ref()
                    .map_or_else(
                        || DURATION_NONE.to_string(),
                        |override_| override_.schedule_to_start_timeout.clone(),
                    ),
            ),
        };
        let event_id = self.append_event(at, EventAttributes::DecisionTaskScheduled(attributes));
        self.open_decision_tasks = 1;
        Some(event_id)
    }

    /// Latch a cancellation request on this run as of `at`: the one
    /// `WorkflowExecutionCancelRequested` event that records it, and the decision task
    /// the decider answers it in. Returns the `eventId` of the appended event.
    ///
    /// The ONE place a cancellation request is recorded, shared by every way SWF delivers
    /// one — a caller's `RequestCancelWorkflowExecution`, another run's
    /// `RequestCancelExternalWorkflowExecution` decision, and a parent's
    /// `REQUEST_CANCEL` child policy. "It is up to the decider to take appropriate
    /// actions when it receives an execution history with this event", and a decider only
    /// ever receives a history through a decision task, so recording the event and making
    /// the decider's reaction pollable are one step. The one-decision-task-per-execution
    /// rule stays [`Execution::schedule_decision_task_if_idle`]'s: a run that already has
    /// one in flight gets no second one, because the history that task's decider is
    /// handed carries this event anyway.
    pub fn request_cancel(
        &mut self,
        attributes: WorkflowExecutionCancelRequestedEventAttributes,
        at: DateTime,
    ) -> i64 {
        self.cancel_requested = true;
        let event_id = self.append_event(
            at,
            EventAttributes::WorkflowExecutionCancelRequested(attributes),
        );
        self.schedule_decision_task_if_idle(at);
        event_id
    }

    /// Fire every timer of this run the clock has already made due at `now`: "This timer
    /// fires after the specified delay and record a `TimerFired` event." Each event is
    /// stamped at the instant its own `startToFireTimeout` ran out, whether or not
    /// anybody called then, and a decision task is scheduled so the decider is told.
    ///
    /// Returns how many fired.
    pub fn fire_due_timers(&mut self, now: DateTime) -> usize {
        if !self.is_open() {
            return 0;
        }
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.effect().timer_closed)
            .collect();
        let mut due: Vec<(DateTime, i64, String)> = self
            .history
            .iter()
            .filter_map(|event| match &event.attributes {
                EventAttributes::TimerStarted(started) if !closed.contains(&event.event_id) => {
                    let fires_at =
                        deadline_after(event.event_timestamp, Some(&started.start_to_fire_timeout))?;
                    (instant_key(fires_at) <= instant_key(now))
                        .then(|| (fires_at, event.event_id, started.timer_id.clone()))
                }
                _ => None,
            })
            .collect();
        due.sort_by_key(|(fires_at, started_event_id, _)| {
            (instant_key(*fires_at), *started_event_id)
        });

        let mut fired = 0;
        for (fires_at, started_event_id, timer_id) in due {
            if !self.has_history_room_for(2) {
                break;
            }
            self.append_event(
                fires_at,
                EventAttributes::TimerFired(TimerFiredEventAttributes {
                    timer_id,
                    started_event_id,
                }),
            );
            self.open_timers = self.open_timers.saturating_sub(1);
            self.schedule_decision_task_if_idle(fires_at);
            fired += 1;
        }
        fired
    }

    /// Record every decision-task SCHEDULE-to-start timeout `now` has already made due:
    /// a task scheduled while a temporary task-list override was in force and not
    /// started within the override's own `taskListScheduleToStartTimeout`. "Amazon SWF
    /// will revert the override and schedule a new decision task to the original task
    /// list", so each one times out at the instant the duration ran out, the override is
    /// reverted, and a replacement task is scheduled on the reverted list.
    ///
    /// Returns how many were recorded.
    fn expire_due_schedule_to_start(&mut self, now: DateTime) -> usize {
        if !self.is_open() {
            return 0;
        }
        let due: Vec<(DateTime, i64)> = self
            .waiting_decision_tasks()
            .into_iter()
            .filter_map(|event| match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => {
                    let deadline = deadline_after(
                        event.event_timestamp,
                        scheduled.schedule_to_start_timeout.as_deref(),
                    )?;
                    (instant_key(deadline) <= instant_key(now))
                        .then_some((deadline, event.event_id))
                }
                _ => None,
            })
            .collect();

        for (deadline, scheduled_event_id) in &due {
            if !self.has_history_room_for(2) {
                break;
            }
            self.append_event(
                *deadline,
                EventAttributes::DecisionTaskTimedOut(DecisionTaskTimedOutEventAttributes {
                    timeout_type: DECISION_TASK_TIMEOUT_SCHEDULE_TO_START.to_string(),
                    scheduled_event_id: *scheduled_event_id,
                    // The task was never handed to a decider, so no `DecisionTaskStarted`
                    // event closes with it; the member's own modelled default says so.
                    started_event_id: 0,
                }),
            );
            // The override lasted exactly as long as its timeout allowed.
            if let Some(reverted) = self.task_list_override.take() {
                self.effective_config.task_list = reverted.revert_to;
            }
            self.open_decision_tasks = 0;
            self.schedule_decision_task_if_idle(*deadline);
        }
        due.len()
    }

    /// The `DecisionTaskScheduled` attributes of the task scheduled by event
    /// `scheduled_event_id`, when this run's history holds one.
    pub fn scheduled_decision_task(
        &self,
        scheduled_event_id: i64,
    ) -> Option<&DecisionTaskScheduledEventAttributes> {
        self.history
            .iter()
            .filter(|event| event.event_id == scheduled_event_id)
            .find_map(|event| match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => Some(scheduled),
                _ => None,
            })
    }

    /// The decision tasks of this run that were handed to a decider and are still
    /// OUTSTANDING: one `(scheduledEventId, startedEventId, startedAt)` per
    /// `DecisionTaskStarted` event that no later `DecisionTaskCompleted` or
    /// `DecisionTaskTimedOut` event names.
    pub fn outstanding_decision_tasks(&self) -> Vec<(i64, i64, DateTime)> {
        let closed: HashSet<i64> = self
            .history
            .iter()
            .filter_map(|event| event.attributes.decision_task_closed())
            .collect();
        self.history
            .iter()
            .filter_map(|event| match &event.attributes {
                EventAttributes::DecisionTaskStarted(started)
                    if !closed.contains(&event.event_id) =>
                {
                    Some((
                        started.scheduled_event_id,
                        event.event_id,
                        event.event_timestamp,
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether the decision task recorded by `DecisionTaskStarted` event
    /// `started_event_id` is still outstanding — what makes a task token a live handle
    /// rather than a stale one.
    pub fn decision_task_is_outstanding(&self, started_event_id: i64) -> bool {
        self.outstanding_decision_tasks()
            .iter()
            .any(|(_, started, _)| *started == started_event_id)
    }

    /// Record every start-to-close timeout of this run's outstanding decision tasks
    /// that `now` has already made due, and return how many were recorded.
    ///
    /// A decision task's own `startToCloseTimeout` is "The maximum duration for this
    /// decision task. The task is considered timed out if it doesn't completed within
    /// this duration", with [`DURATION_NONE`] meaning unlimited — so the deadline is
    /// the instant the task was STARTED plus that duration, and it is the caller's own
    /// `taskStartToCloseTimeout`, never a span this emulator picks. Timing one out
    /// appends `DecisionTaskTimedOut` and then schedules a replacement decision task on
    /// the same list with the same attributes, so the run keeps exactly one decision
    /// task to hand out and a decider that lost its task is given a new one.
    ///
    /// The schedule-to-start timeout of a task still waiting in an overridden task list
    /// falls due the same way, so it is materialised here too (see
    /// [`Execution::expire_due_schedule_to_start`]).
    pub fn expire_due_decision_tasks(&mut self, now: DateTime) -> usize {
        if !self.is_open() {
            return 0;
        }
        let reverted = self.expire_due_schedule_to_start(now);
        let due: Vec<(i64, i64)> = self
            .outstanding_decision_tasks()
            .into_iter()
            .filter(|(scheduled_event_id, _, started_at)| {
                self.scheduled_decision_task(*scheduled_event_id)
                    .and_then(|scheduled| {
                        deadline_after(*started_at, scheduled.start_to_close_timeout.as_deref())
                    })
                    .is_some_and(|deadline| instant_key(now) >= instant_key(deadline))
            })
            .map(|(scheduled_event_id, started_event_id, _)| {
                (scheduled_event_id, started_event_id)
            })
            .collect();

        for (scheduled_event_id, started_event_id) in &due {
            // The timeout and the replacement task it schedules are two events, and a
            // run with no room for them has reached the history quota: it is left for
            // the writer that finds it to close rather than grown past the bound.
            if !self.has_history_room_for(2) {
                break;
            }
            let Some(replacement) = self.scheduled_decision_task(*scheduled_event_id).cloned()
            else {
                continue;
            };
            self.append_event(
                now,
                EventAttributes::DecisionTaskTimedOut(DecisionTaskTimedOutEventAttributes {
                    timeout_type: DECISION_TASK_TIMEOUT_START_TO_CLOSE.to_string(),
                    scheduled_event_id: *scheduled_event_id,
                    started_event_id: *started_event_id,
                }),
            );
            self.append_event(now, EventAttributes::DecisionTaskScheduled(replacement));
        }
        due.len() + reverted
    }

    /// The `startedEventId` of the last decision task of this run a decider actually
    /// PROCESSED before `before_started_event_id` — `previousStartedEventId`, "The ID
    /// of the DecisionTaskStarted event of the previous decision task of this workflow
    /// execution that was processed by the decider". A task that timed out was not
    /// processed, so it does not count; the modelled default `0` is the answer when no
    /// decision task of this run has been completed yet.
    pub fn previous_started_event_id(&self, before_started_event_id: i64) -> i64 {
        self.history
            .iter()
            .filter_map(|event| match &event.attributes {
                EventAttributes::DecisionTaskCompleted(completed)
                    if completed.started_event_id < before_started_event_id =>
                {
                    Some(completed.started_event_id)
                }
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// The most recent `DecisionTaskStarted` event this run recorded BEFORE
    /// `before_started_event_id` — the previous decision task's own started event,
    /// which is where `startAtPreviousStartedEvent` makes a returned history begin.
    pub fn previous_decision_task_started(
        &self,
        before_started_event_id: i64,
    ) -> Option<&HistoryEventRecord> {
        self.history.iter().rfind(|event| {
            event.event_id < before_started_event_id
                && matches!(event.attributes, EventAttributes::DecisionTaskStarted(_))
        })
    }
}

/// The instant `timeout` seconds after `from`, or `None` when that duration is
/// unlimited ([`DURATION_NONE`], or absent) and so names no instant at all.
///
/// Every SWF duration member is "specified in seconds, an integer greater than or equal
/// to 0", so one deadline helper serves them all: a decision task's start-to-close and
/// schedule-to-start budgets and a timer's start-to-fire delay.
fn deadline_after(from: DateTime, timeout: Option<&str>) -> Option<DateTime> {
    let seconds = timeout
        .filter(|timeout| *timeout != DURATION_NONE)
        .and_then(decimal)?;
    let seconds = i64::try_from(seconds).ok()?;
    Some(DateTime::from_secs_and_nanos(
        from.secs().checked_add(seconds)?,
        from.subsec_nanos(),
    ))
}

/// The instant `span` after `from`, or `None` when that instant is not representable.
fn instant_after(from: DateTime, span: Duration) -> Option<DateTime> {
    let nanos = u64::from(from.subsec_nanos()) + u64::from(span.subsec_nanos());
    let seconds = i64::try_from(span.as_secs().checked_add(nanos / 1_000_000_000)?).ok()?;
    Some(DateTime::from_secs_and_nanos(
        from.secs().checked_add(seconds)?,
        u32::try_from(nanos % 1_000_000_000).ok()?,
    ))
}

/// The earliest of `timeouts` that `now` has already made due, as `(the instant it ran
/// out, the `timeoutType` that names it)`; `None` when none of them has.
///
/// Each entry is one `(timeoutType, duration)` pair measured from `from`, so the caller
/// groups the durations by the instant they start running and this picks the one that
/// expired first — a task times out ONCE, under the budget that ran out soonest.
fn earliest_due(
    from: DateTime,
    timeouts: &[(&'static str, Option<&str>)],
    now: DateTime,
) -> Option<(DateTime, &'static str)> {
    timeouts
        .iter()
        .filter_map(|(timeout_type, timeout)| {
            let deadline = deadline_after(from, *timeout)?;
            // A task whose own duration is the published lower bound of zero seconds
            // would be over in the instant it began, so the span it is measured over is
            // floored at [`MIN_TASK_SPAN`] — the same reason a run's budget has a floor.
            let deadline = match instant_after(from, MIN_TASK_SPAN) {
                Some(floor) if instant_key(floor) > instant_key(deadline) => floor,
                _ => deadline,
            };
            (instant_key(deadline) <= instant_key(now)).then_some((deadline, *timeout_type))
        })
        .min_by_key(|(deadline, _)| instant_key(*deadline))
}

/// Which kind of task a task list holds, a poller waits for, and a task token is a
/// handle on. SWF's two task kinds live in separate lists of the same domain and are
/// polled, counted and answered by the same rules, so the few places that differ take
/// this rather than each being written twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Decision,
    Activity,
}

impl TaskKind {
    /// The tag that separates this kind in a derived task token, so a token minted for
    /// one kind of task can never be a handle on the other.
    fn token_tag(self) -> &'static str {
        match self {
            Self::Decision => "DecisionTask",
            Self::Activity => "ActivityTask",
        }
    }
}

/// One task waiting in a task list, as the dispatcher sees it: which run it belongs to,
/// the event that scheduled it, and what orders it against the other tasks waiting in
/// the same list. Both kinds of task list dispatch the same way, so both polls read
/// this.
#[derive(Debug, Clone)]
pub struct WaitingTask {
    pub workflow_id: String,
    pub run_id: String,
    pub scheduled_event_id: i64,
    /// The task's own `taskPriority` as a number, defaulting to
    /// [`DEFAULT_TASK_PRIORITY`] when the task carries none.
    pub priority: i64,
    pub scheduled_at: (i64, u32),
}

impl WaitingTask {
    /// One waiting task read off the `*Scheduled` event that put it in its list, for the
    /// run `execution`.
    fn of(execution: &Execution, event: &HistoryEventRecord) -> Self {
        Self {
            workflow_id: execution.workflow_id.clone(),
            run_id: execution.run_id.clone(),
            scheduled_event_id: event.event_id,
            priority: match &event.attributes {
                EventAttributes::DecisionTaskScheduled(scheduled) => {
                    task_priority(scheduled.task_priority.as_deref())
                }
                EventAttributes::ActivityTaskScheduled(scheduled) => {
                    task_priority(scheduled.task_priority.as_deref())
                }
                _ => DEFAULT_TASK_PRIORITY,
            },
            scheduled_at: instant_key(event.event_timestamp),
        }
    }

    /// The order a poll hands the waiting tasks of one list out in: higher
    /// `taskPriority` first ("Higher numbers indicate higher priority"), then
    /// first-scheduled first. The run identity and the scheduling event id break a tie
    /// so that two tasks scheduled in the same instant still have one defined position
    /// — the store holds runs in a map with no order of its own.
    fn dispatch_key(&self) -> (i128, (i64, u32), String, String, i64) {
        (
            -i128::from(self.priority),
            self.scheduled_at,
            self.workflow_id.clone(),
            self.run_id.clone(),
            self.scheduled_event_id,
        )
    }
}

/// A task's `taskPriority` as the number it is dispatched by, defaulting to
/// [`DEFAULT_TASK_PRIORITY`] when the task carries none or carries one this service
/// cannot read as an integer.
fn task_priority(priority: Option<&str>) -> i64 {
    priority
        .and_then(|priority| priority.parse::<i64>().ok())
        .unwrap_or(DEFAULT_TASK_PRIORITY)
}

/// One registered domain and everything scoped to it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub name: String,
    pub description: Option<String>,
    pub retention_period_in_days: String,
    /// One of [`crate::ops::types::REGISTRATION_STATUS_VALUES`].
    pub status: String,
    pub arn: String,
    pub tags: Vec<ResourceTag>,
    /// `(name, version)` -> the registered workflow type.
    pub workflow_types: HashMap<(String, String), WorkflowTypeRecord>,
    /// `(name, version)` -> the registered activity type, written by
    /// `RegisterActivityType` and read back by `DescribeActivityType`.
    pub activity_types: HashMap<(String, String), ActivityTypeRecord>,
    /// `(workflowId, runId)` -> the execution.
    pub executions: HashMap<(String, String), Execution>,
    /// `workflowId` -> the `runId` of the one OPEN run with that id. This is what
    /// makes a second start of a still-open `workflowId` a
    /// `WorkflowExecutionAlreadyStartedFault`, and what resolves a
    /// Terminate/Signal/Cancel call that omits `runId`.
    pub open_by_workflow_id: HashMap<String, String>,
    /// The ordinal the NEXT start in this domain takes, feeding
    /// [`new_run_id`]. Strictly increasing and never reused, so no two runs of this
    /// domain can be given the same `runId`.
    pub next_run_sequence: u64,
    /// `(which kind of task, task-list name)` -> how many long polls of that kind are
    /// open on that list of this domain RIGHT NOW. A poll is counted while it holds its
    /// answer open and uncounted the instant it ends, which is what
    /// [`MAX_POLLERS_PER_TASK_LIST`] is enforced against; an entry drops out of the map
    /// when its last poller leaves, so the map holds only lists with live pollers.
    pub pollers: HashMap<(TaskKind, String), usize>,
}

impl Domain {
    /// A domain registered now under `name`, with no types and no executions.
    pub fn registered(name: &str, region: &str, retention_period_in_days: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            retention_period_in_days: retention_period_in_days.to_string(),
            status: STATUS_REGISTERED.to_string(),
            arn: domain_arn(region, name),
            tags: Vec::new(),
            workflow_types: HashMap::new(),
            activity_types: HashMap::new(),
            executions: HashMap::new(),
            open_by_workflow_id: HashMap::new(),
            next_run_sequence: 0,
            pollers: HashMap::new(),
        }
    }

    /// The `DomainInfo` every domain reader returns, built from what is stored.
    pub fn info(&self) -> DomainInfo {
        DomainInfo {
            name: self.name.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
        }
    }

    /// The `DomainConfiguration` `DescribeDomain` reports: the retention period this
    /// domain was registered with, exactly as stored.
    pub fn configuration(&self) -> DomainConfiguration {
        DomainConfiguration {
            workflow_execution_retention_period_in_days: self.retention_period_in_days.clone(),
        }
    }

    /// Whether this domain is deprecated, i.e. whether its stored
    /// `RegistrationStatus` is [`STATUS_DEPRECATED`].
    pub fn is_deprecated(&self) -> bool {
        self.status == STATUS_DEPRECATED
    }

    /// Deprecate this domain: the ONE record's own `RegistrationStatus` moves to
    /// [`STATUS_DEPRECATED`] and nothing else in the domain changes. The status of a
    /// registered workflow or activity type is that type's own, moved only by a call
    /// addressing that type (`DeprecateActivityType` / `DeprecateWorkflowType`), which is
    /// what Amazon SWF reports: a type registered before a domain's deprecation still
    /// reads back `REGISTERED` from `DescribeWorkflowType` / `DescribeActivityType`
    /// afterwards. Executions are untouched too — "Executions that were started before
    /// the domain was deprecated continues to run."
    pub fn deprecate(&mut self) {
        self.status = STATUS_DEPRECATED.to_string();
    }

    /// Return this domain to [`STATUS_REGISTERED`], the move `UndeprecateDomain` makes:
    /// "Undeprecates a previously deprecated domain. After a domain has been
    /// undeprecated it can be used to create new workflow executions or register new
    /// types." Like [`Domain::deprecate`] it touches the ONE record's own
    /// `RegistrationStatus` and nothing else: the types registered in the domain keep
    /// their own status, and the executions, tags, ARN and retention period are left
    /// exactly as they are.
    pub fn undeprecate(&mut self) {
        self.status = STATUS_REGISTERED.to_string();
    }

    /// How many decision tasks are waiting in `task_list` across this domain — what
    /// `CountPendingDecisionTasks` answers with. A task list is scoped to its domain,
    /// so only this domain's runs contribute, and a list no decision task was ever
    /// scheduled in holds none.
    ///
    /// Only an OPEN run has tasks in the list: a closed run hands nothing to a decider,
    /// so whatever it had waiting is no longer PENDING in the list. That is a different
    /// question from `DescribeWorkflowExecution`'s `openCounts`, which reports the work of
    /// the run whose own status is open and so still counts a decision task the run was
    /// closed without ever answering.
    pub fn pending_decision_tasks_on(&self, task_list: &str) -> usize {
        self.executions
            .values()
            .filter(|execution| execution.is_open())
            .map(|execution| execution.pending_decision_tasks_on(task_list))
            .sum()
    }

    /// How many activity tasks are waiting in `task_list` across this domain — what
    /// `CountPendingActivityTasks` answers with. The same scoping as
    /// [`Domain::pending_decision_tasks_on`]: a task list lives inside its domain, so
    /// only this domain's runs contribute, a list no activity task was ever scheduled in
    /// holds none, and only an OPEN run has tasks waiting in a list.
    pub fn pending_activity_tasks_on(&self, task_list: &str) -> usize {
        self.executions
            .values()
            .filter(|execution| execution.is_open())
            .map(|execution| execution.pending_activity_tasks_on(task_list))
            .sum()
    }

    /// Record every decision-task start-to-close timeout this domain's OPEN runs have
    /// already fallen due for at `now`, and return how many were recorded.
    ///
    /// The timeout is the SERVER's, not the poller's: it falls due at the instant the
    /// task's own `startToCloseTimeout` names, whether or not anybody calls. Any
    /// operation that reports or hands out decision tasks materialises it first, so
    /// every caller sees the same history at the same instant; a closed run has no
    /// outstanding task to time out. See [`Execution::expire_due_decision_tasks`].
    pub fn expire_due_decision_tasks(&mut self, now: DateTime) -> usize {
        self.executions
            .values_mut()
            .filter(|execution| execution.is_open())
            .map(|execution| execution.expire_due_decision_tasks(now))
            .sum()
    }

    /// The decision task a poll of `task_list` is handed: the one waiting task of this
    /// domain's OPEN runs that comes first under [`WaitingTask::dispatch_key`], or
    /// `None` when the list holds none.
    pub fn next_decision_task_on(&self, task_list: &str) -> Option<WaitingTask> {
        self.next_task_on(task_list, Execution::waiting_decision_tasks_on)
    }

    /// The activity task a poll of `task_list` is handed, by the same rule and the same
    /// dispatch order as [`Domain::next_decision_task_on`]: the one activity task of
    /// this domain's OPEN runs waiting in that list that comes first under
    /// [`WaitingTask::dispatch_key`], or `None` when the list holds none.
    pub fn next_activity_task_on(&self, task_list: &str) -> Option<WaitingTask> {
        self.next_task_on(task_list, Execution::waiting_activity_tasks_on)
    }

    /// The task a poll of `task_list` is handed out of the tasks `waiting` reports for
    /// each OPEN run of this domain.
    fn next_task_on(
        &self,
        task_list: &str,
        waiting: for<'a> fn(&'a Execution, &str) -> Vec<&'a HistoryEventRecord>,
    ) -> Option<WaitingTask> {
        self.executions
            .values()
            .filter(|execution| execution.is_open())
            .flat_map(|execution| {
                waiting(execution, task_list)
                    .into_iter()
                    .map(|event| WaitingTask::of(execution, event))
                    .collect::<Vec<_>>()
            })
            .min_by_key(WaitingTask::dispatch_key)
    }

    /// The `runId` a call naming `workflow_id` addresses: the one it gave, when a run
    /// with that pair is stored, else the currently OPEN run of that `workflowId`.
    /// `None` when neither resolves to a stored execution.
    pub fn resolve_run_id(&self, workflow_id: &str, run_id: Option<&str>) -> Option<String> {
        match run_id.filter(|id| !id.is_empty()) {
            Some(id) => self
                .executions
                .contains_key(&(workflow_id.to_string(), id.to_string()))
                .then(|| id.to_string()),
            None => self.open_by_workflow_id.get(workflow_id).cloned(),
        }
    }

    /// Close every run of this domain whose start-to-close budget the clock has
    /// already spent, and fire every timer it has already made due, as of `at`.
    ///
    /// This is the service's own reaping of abandoned executions: each move was owed at
    /// its own instant whether or not anybody called, so every operation materialises
    /// what is already due before it answers — which is why a run left completely
    /// unattended still reads back CLOSED/TIMED_OUT, is gone from the open listings and
    /// appears in the closed ones, and why a timer started by a decider fires without
    /// anyone polling for it.
    pub fn materialise_due_timeouts(&mut self, at: DateTime) {
        let mut timed_out: Vec<(String, String)> = Vec::new();
        for execution in self.executions.values_mut() {
            if !execution.is_open() {
                continue;
            }
            let close_due = execution.due_start_to_close_timeout(at);
            // A timer due only AFTER the run's own start-to-close deadline never fires:
            // by then the run is closed and its history takes nothing more.
            execution.fire_due_timers(close_due.unwrap_or(at));
            // An activity task's own timeouts fall due the same way, and for the same
            // reason are materialised before any caller reads the run: a task nobody
            // took, or took and never closed, is timed out by the service itself.
            execution.expire_due_activity_tasks(close_due.unwrap_or(at));
            if close_due.is_some() {
                timed_out.push((execution.workflow_id.clone(), execution.run_id.clone()));
            }
        }
        for key in timed_out {
            // The close is stamped at the DEADLINE, not at `at`: the timeout happened
            // when the budget ran out, whether or not anybody called in that instant.
            let Some(due) = self
                .executions
                .get(&key)
                .and_then(|execution| execution.due_start_to_close_timeout(at))
            else {
                continue;
            };
            self.close_run(&key, RunClose::TimedOut, due);
        }
    }

    /// Start one run in this domain and return the `runId` it was given: the ONE place a
    /// workflow execution comes into existence, whether a client started it
    /// (`StartWorkflowExecution`), a decider continued one as new, or a parent run
    /// started it as a child.
    ///
    /// The run is stored OPEN with its `WorkflowExecutionStarted` event and the first
    /// decision task SWF schedules as soon as an execution starts, and it becomes the
    /// open run of its `workflowId`. The caller has already resolved the effective
    /// configuration and checked that the type is registered and the id free.
    ///
    /// `None` when this domain already holds [`MAX_EXECUTIONS_PER_DOMAIN`] runs — the
    /// bound lives HERE, so no caller can start a run past it however it asked: a run
    /// stays stored after it closes (its history is what the readers answer from), so
    /// every start, continue-as-new and child start adds to the same bounded store.
    /// Each call site reports the refusal the way its own operation models it.
    #[must_use = "a start that the per-domain run bound refused must be reported"]
    pub fn start_run(&mut self, run: NewRun, at: DateTime) -> Option<String> {
        if self.executions.len() >= MAX_EXECUTIONS_PER_DOMAIN {
            return None;
        }
        // The per-domain ordinal is what makes the minted id unique.
        self.next_run_sequence += 1;
        let run_id = new_run_id(&self.name, &run.workflow_id, self.next_run_sequence, at);
        let (parent, parent_initiated_event_id) = match &run.parent {
            Some((execution, initiated_event_id)) => {
                (Some(execution.clone()), *initiated_event_id)
            }
            None => (None, 0),
        };
        let mut execution = Execution {
            workflow_id: run.workflow_id.clone(),
            run_id: run_id.clone(),
            workflow_type: (
                run.workflow_type.name.clone(),
                run.workflow_type.version.clone(),
            ),
            status: EXECUTION_STATUS_OPEN.to_string(),
            close_status: None,
            start_timestamp: at,
            close_timestamp: None,
            tag_list: run.tag_list.clone(),
            cancel_requested: false,
            input: run.input.clone(),
            effective_config: run.config.clone(),
            history: Vec::new(),
            next_event_id: 1,
            open_decision_tasks: 0,
            open_activity_tasks: 0,
            open_timers: 0,
            open_child_executions: 0,
            open_lambda_functions: 0,
            latest_execution_context: None,
            latest_activity_task_timestamp: None,
            parent,
            parent_initiated_event_id,
            task_list_override: None,
            activity_task_heartbeats: HashMap::new(),
        };

        execution.append_event(
            at,
            EventAttributes::WorkflowExecutionStarted(Box::new(
                WorkflowExecutionStartedEventAttributes {
                    input: run.input,
                    execution_start_to_close_timeout: Some(
                        run.config.execution_start_to_close_timeout.clone(),
                    ),
                    task_start_to_close_timeout: Some(
                        run.config.task_start_to_close_timeout.clone(),
                    ),
                    child_policy: run.config.child_policy.clone(),
                    task_list: TaskList {
                        name: run.config.task_list.clone(),
                    },
                    task_priority: run.config.task_priority.clone(),
                    workflow_type: run.workflow_type,
                    tag_list: (!run.tag_list.is_empty()).then_some(run.tag_list),
                    continued_execution_run_id: run.continued_execution_run_id,
                    parent_workflow_execution: run
                        .parent
                        .as_ref()
                        .map(|(execution, _)| execution.clone()),
                    parent_initiated_event_id,
                    lambda_role: run.config.lambda_role.clone(),
                },
            )),
        );
        // SWF schedules the first decision task as soon as the execution starts, so
        // the run begins with exactly one open decision task.
        execution.schedule_decision_task_if_idle(at);

        self.executions
            .insert((run.workflow_id.clone(), run_id.clone()), execution);
        self.open_by_workflow_id
            .insert(run.workflow_id, run_id.clone());
        Some(run_id)
    }

    /// Close the run `key` names the way `close` describes, as of `at`: the close event
    /// and `closeStatus` on the run itself, the `workflowId` released for a new start,
    /// the `childPolicy` applied to the children a TERMINATED run leaves behind, and the
    /// parent run told what became of it.
    ///
    /// The ONE place a run closes, so those five records can never disagree. A run that
    /// is already closed is left exactly as it is, which is what makes a close that two
    /// callers race for record its events once.
    ///
    /// A `TERMINATE` child policy cascades down the parent/child chain, and the cascade
    /// is a WORKLIST rather than recursion: the children a close terminates are pushed
    /// onto the queue this loop drains, so the chain's depth costs queue entries instead
    /// of stack frames however deep a decider built it. The loop ends because every turn
    /// closes one open run and only open runs are pushed, and the runs of a domain are
    /// bounded by [`MAX_EXECUTIONS_PER_DOMAIN`].
    pub fn close_run(&mut self, key: &(String, String), close: RunClose, at: DateTime) {
        let mut pending: VecDeque<((String, String), RunClose)> = VecDeque::new();
        pending.push_back((key.clone(), close));
        while let Some((key, close)) = pending.pop_front() {
            let Some(execution) = self.executions.get_mut(&key) else {
                continue;
            };
            if !execution.is_open() {
                continue;
            }
            let child_policy = close.child_policy(&execution.effective_config.child_policy);
            let applies_child_policy = close.applies_child_policy();
            let continued_as_new = matches!(close, RunClose::ContinuedAsNew(_));
            let closed = CloseOutcome {
                status: close.close_status(),
                reason: close.closed_reason(),
                payload: close.closed_payload(),
                execution: execution.execution(),
                workflow_type: WorkflowType {
                    name: execution.workflow_type.0.clone(),
                    version: execution.workflow_type.1.clone(),
                },
                parent: execution.parent.clone(),
                parent_initiated_event_id: execution.parent_initiated_event_id,
            };
            execution.close(at, close);
            self.release_open_run(&key.0, &key.1);
            if applies_child_policy {
                self.apply_child_policy(&key, &child_policy, at, &mut pending);
            }
            // A run that continued as new did not finish: the run that took its place
            // carries its parent link on, and reports back when IT closes.
            if !continued_as_new {
                self.report_close_to_parent(&closed, at);
            }
        }
    }

    /// Apply `child_policy` to every OPEN child of the run `key` names, the move a close
    /// makes on the children it leaves behind: `TERMINATE` closes each one (queued onto
    /// `pending` for [`Domain::close_run`]'s own loop, which then applies that child's
    /// policy to ITS children in turn), `REQUEST_CANCEL` asks each to cancel itself, and
    /// `ABANDON` leaves them running.
    fn apply_child_policy(
        &mut self,
        key: &(String, String),
        child_policy: &str,
        at: DateTime,
        pending: &mut VecDeque<((String, String), RunClose)>,
    ) {
        if child_policy != CHILD_POLICY_TERMINATE && child_policy != CHILD_POLICY_REQUEST_CANCEL {
            return;
        }
        let children: Vec<(String, String)> = self
            .executions
            .values()
            .filter(|execution| {
                execution.is_open()
                    && execution.parent.as_ref().is_some_and(|parent| {
                        parent.workflow_id == key.0 && parent.run_id == key.1
                    })
            })
            .map(|execution| (execution.workflow_id.clone(), execution.run_id.clone()))
            .collect();
        for child in children {
            if child_policy == CHILD_POLICY_TERMINATE {
                pending.push_back((
                    child,
                    RunClose::Terminated {
                        reason: None,
                        details: None,
                        // "If set, indicates that the workflow execution was
                        // automatically terminated, and specifies the cause": the
                        // parent's policy is what terminated this child.
                        cause: Some(CAUSE_CHILD_POLICY_APPLIED.to_string()),
                        child_policy: None,
                    },
                ));
                continue;
            }
            let Some(execution) = self.executions.get_mut(&child) else {
                continue;
            };
            if !execution.has_history_room_for(2) {
                continue;
            }
            execution.request_cancel(
                WorkflowExecutionCancelRequestedEventAttributes {
                    // The request came from the parent's policy, not from another
                    // run's decision, so no external execution initiated it.
                    external_workflow_execution: None,
                    external_initiated_event_id: 0,
                    cause: Some(CAUSE_CHILD_POLICY_APPLIED.to_string()),
                },
                at,
            );
        }
    }

    /// Record in the PARENT run what became of a child that just closed, and schedule a
    /// decision task so its decider is told: a child's result is an event of the parent's
    /// history ("The child workflow execution is a separate workflow execution with its
    /// own history"), one per way a run can close.
    ///
    /// Nothing is recorded when the child had no parent, or when the parent has closed
    /// too: a closed run's history takes no further events.
    fn report_close_to_parent(&mut self, closed: &CloseOutcome, at: DateTime) {
        let Some(parent) = closed.parent.clone() else {
            return;
        };
        let initiated_event_id = closed.parent_initiated_event_id;
        let Some(parent_execution) = self
            .executions
            .get_mut(&(parent.workflow_id.clone(), parent.run_id.clone()))
        else {
            return;
        };
        if !parent_execution.is_open() || !parent_execution.has_history_room_for(2) {
            return;
        }
        // The `startedEventId` of the child's own start, as the parent recorded it.
        let started_event_id = parent_execution
            .history
            .iter()
            .find_map(|event| match &event.attributes {
                EventAttributes::ChildWorkflowExecutionStarted(started)
                    if started.initiated_event_id == initiated_event_id =>
                {
                    Some(event.event_id)
                }
                _ => None,
            })
            .unwrap_or(0);
        let execution = closed.execution.clone();
        let workflow_type = closed.workflow_type.clone();
        let attributes = match closed.status {
            CLOSE_STATUS_COMPLETED => EventAttributes::ChildWorkflowExecutionCompleted(
                ChildWorkflowExecutionCompletedEventAttributes {
                    workflow_execution: execution,
                    workflow_type,
                    result: closed.payload.clone(),
                    initiated_event_id,
                    started_event_id,
                },
            ),
            CLOSE_STATUS_FAILED => EventAttributes::ChildWorkflowExecutionFailed(
                ChildWorkflowExecutionFailedEventAttributes {
                    workflow_execution: execution,
                    workflow_type,
                    reason: closed.reason.clone(),
                    details: closed.payload.clone(),
                    initiated_event_id,
                    started_event_id,
                },
            ),
            CLOSE_STATUS_CANCELED => EventAttributes::ChildWorkflowExecutionCanceled(
                ChildWorkflowExecutionCanceledEventAttributes {
                    workflow_execution: execution,
                    workflow_type,
                    details: closed.payload.clone(),
                    initiated_event_id,
                    started_event_id,
                },
            ),
            CLOSE_STATUS_TIMED_OUT => EventAttributes::ChildWorkflowExecutionTimedOut(
                ChildWorkflowExecutionTimedOutEventAttributes {
                    workflow_execution: execution,
                    workflow_type,
                    timeout_type: WORKFLOW_EXECUTION_TIMEOUT_START_TO_CLOSE.to_string(),
                    initiated_event_id,
                    started_event_id,
                },
            ),
            _ => EventAttributes::ChildWorkflowExecutionTerminated(
                ChildWorkflowExecutionTerminatedEventAttributes {
                    workflow_execution: execution,
                    workflow_type,
                    initiated_event_id,
                    started_event_id,
                },
            ),
        };
        parent_execution.append_event(at, attributes);
        parent_execution.open_child_executions =
            parent_execution.open_child_executions.saturating_sub(1);
        parent_execution.schedule_decision_task_if_idle(at);
    }

    /// How many decision-task long polls are open on `task_list` of this domain.
    pub fn open_decision_pollers(&self, task_list: &str) -> usize {
        self.open_pollers(TaskKind::Decision, task_list)
    }

    /// How many activity-task long polls are open on `task_list` of this domain.
    pub fn open_activity_pollers(&self, task_list: &str) -> usize {
        self.open_pollers(TaskKind::Activity, task_list)
    }

    /// How many long polls of `kind` are open on `task_list` of this domain.
    pub fn open_pollers(&self, kind: TaskKind, task_list: &str) -> usize {
        self.pollers
            .get(&(kind, task_list.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// Count one more poller of `kind` on `task_list`, or answer `false` when that list
    /// already holds [`MAX_POLLERS_PER_TASK_LIST`] of them — the admission a
    /// `LimitExceededFault` refuses.
    pub fn admit_poller(&mut self, kind: TaskKind, task_list: &str) -> bool {
        let open = self
            .pollers
            .entry((kind, task_list.to_string()))
            .or_insert(0);
        if *open >= MAX_POLLERS_PER_TASK_LIST {
            return false;
        }
        *open += 1;
        true
    }

    /// Give back one poller slot of `kind` on `task_list`.
    pub fn release_poller(&mut self, kind: TaskKind, task_list: &str) {
        let key = (kind, task_list.to_string());
        if let Some(open) = self.pollers.get_mut(&key) {
            *open = open.saturating_sub(1);
            if *open == 0 {
                self.pollers.remove(&key);
            }
        }
    }

    /// Drop this domain's open-run index entry for `(workflow_id, run_id)`, the move a
    /// close makes: the `workflowId` becomes startable again.
    ///
    /// Only that run's own entry is cleared — it is removed when the `runId` it points
    /// at is the one just closed — so a future operation that relaxes the
    /// one-open-run-per-`workflowId` invariant cannot drop another run's entry.
    pub fn release_open_run(&mut self, workflow_id: &str, run_id: &str) {
        if self
            .open_by_workflow_id
            .get(workflow_id)
            .is_some_and(|open| open == run_id)
        {
            self.open_by_workflow_id.remove(workflow_id);
        }
    }
}

/// The task-list admission one open long poll holds.
///
/// Dropping it releases the slot, so a poll that ends — with a task, with an empty
/// result, or because the caller hung up and the handler's future was cancelled —
/// never leaves the task list's poller count inflated. The slot keeps its own handle on
/// the store for that, so it does not borrow the guard the admission was taken under.
///
/// NEVER drop one while holding the state lock: the drop takes that same lock to give
/// the slot back.
pub struct PollerSlot {
    state: ::engine::ServiceState<ComAmazonawsSwfSimpleworkflowserviceState>,
    kind: TaskKind,
    domain: String,
    task_list: String,
}

impl PollerSlot {
    /// Take one poller slot of `kind` on `(domain, task_list)` through the already-held
    /// `guard`, or answer `None` when that task list is at
    /// [`MAX_POLLERS_PER_TASK_LIST`].
    pub fn admit(
        state: &::engine::ServiceState<ComAmazonawsSwfSimpleworkflowserviceState>,
        guard: &mut ComAmazonawsSwfSimpleworkflowserviceState,
        kind: TaskKind,
        domain: &str,
        task_list: &str,
    ) -> Option<Self> {
        guard
            .domains
            .get_mut(domain)?
            .admit_poller(kind, task_list)
            .then(|| Self {
                state: state.clone(),
                kind,
                domain: domain.to_string(),
                task_list: task_list.to_string(),
            })
    }
}

impl ::std::fmt::Debug for PollerSlot {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("PollerSlot")
            .field("kind", &self.kind)
            .field("domain", &self.domain)
            .field("task_list", &self.task_list)
            .finish()
    }
}

impl Drop for PollerSlot {
    fn drop(&mut self) {
        let mut guard = self.state.lock();
        if let Some(domain) = guard.domains.get_mut(&self.domain) {
            domain.release_poller(self.kind, &self.task_list);
        }
    }
}

/// Per-service state for com-amazonaws-swf-simpleworkflowservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSwfSimpleworkflowserviceState {
    /// Domain name -> the domain. Domain names are unique per region, and this
    /// emulator serves one account, so the name is the whole key.
    pub domains: HashMap<String, Domain>,
}

impl ComAmazonawsSwfSimpleworkflowserviceState {
    /// The domain a `resourceArn` addresses: the stored domain whose own ARN is
    /// exactly this one.
    ///
    /// Comparing the WHOLE ARN, not just the name parsed out of it, is what scopes the
    /// lookup to the resource's region and account — `UnknownResourceFault` is
    /// documented as "Returned when the named resource cannot be found with in the
    /// scope of this operation (region or domain)" — so one region's ARN never
    /// resolves to a same-named domain registered in another.
    pub fn domain_by_arn(&self, arn: &str) -> Option<&Domain> {
        let name = domain_name_from_arn(arn)?;
        self.domains.get(name).filter(|domain| domain.arn == arn)
    }

    /// Materialise the clock-owed closes of ONE domain — the domain the call addresses
    /// — before that domain is read or written (see
    /// [`Domain::materialise_due_timeouts`]).
    ///
    /// Every handler that reads or writes an execution calls this first, so no caller
    /// can observe a run that the service itself should already have reaped. It is
    /// scoped to the one domain the call answers from: another domain's runs are reaped
    /// by the call that asks about them, so the work one request does stays
    /// proportional to what it reports on.
    pub fn materialise_due_timeouts_in(&mut self, domain: &str, at: DateTime) {
        if let Some(domain) = self.domains.get_mut(domain) {
            domain.materialise_due_timeouts(at);
        }
    }

    /// Record a worker's TERMINAL report about the activity task `token` is the handle
    /// on, as of `at`: the one event that report records, the task out of its run's
    /// open-activity-task count, and a decision task for the decider that is waiting on
    /// the result.
    ///
    /// The ONE place an activity task closes on a worker's own report, shared by
    /// `RespondActivityTaskCompleted`, `RespondActivityTaskFailed` and
    /// `RespondActivityTaskCanceled` — the three calls the task's own doc names: "A task
    /// is closed after it has been specified in a call to RespondActivityTaskCompleted,
    /// RespondActivityTaskCanceled, RespondActivityTaskFailed, or the task has timed
    /// out." Exactly one of them can win, because the first one recorded closes the task
    /// and a handle on a closed task addresses nothing.
    ///
    /// `Err` carries the `UnknownResourceFault` message for a token that addresses no
    /// OPEN activity task — one already closed, timed out, belonging to a run that has
    /// closed, or not an activity task token at all — which each caller reports as its
    /// own modelled fault. Nothing is written in that case.
    pub fn close_activity_task(
        &mut self,
        token: &str,
        close: ActivityTaskClose,
        at: DateTime,
    ) -> Result<(), String> {
        let Some(handle) = activity_task_by_token(self, token) else {
            // The token is a handle on no activity task, but it can still be a handle on
            // a task of another KIND this service minted a token for, and the fault names
            // the task the token itself addresses.
            return Err(unknown_activity_message(scheduled_event_id_by_token(self, token)));
        };
        let unknown = unknown_activity_message(handle.scheduled_event_id);
        // The service's own sweeps first: a task the clock timed out, or a run it closed,
        // was over at its own instant whether or not this worker called, so a report that
        // lost that race is refused instead of being recorded into a history that moved
        // on.
        self.materialise_due_timeouts_in(&handle.domain, at);
        let Some(domain) = self.domains.get_mut(&handle.domain) else {
            return Err(unknown);
        };
        let key = (handle.workflow_id.clone(), handle.run_id.clone());
        let Some(execution) = domain.executions.get_mut(&key) else {
            return Err(unknown);
        };
        if !execution.is_open() || !execution.activity_task_is_outstanding(handle.started_event_id)
        {
            return Err(unknown);
        }
        // The report and the decision task that tells the decider about it are two
        // events, and a run with no room for them has reached the history quota: it is
        // terminated by the service rather than grown past the bound, and the token it
        // held is then a handle on a closed task.
        if !execution.has_history_room_for(2) {
            domain.close_run(&key, RunClose::event_limit_exceeded(), at);
            return Err(unknown);
        }
        let latest_cancel_requested_event_id =
            execution.latest_cancel_requested_event_id(handle.scheduled_event_id);
        execution.append_event(
            at,
            close.event(
                handle.scheduled_event_id,
                handle.started_event_id,
                latest_cancel_requested_event_id,
            ),
        );
        // `openActivityTasks` is "The count of activity tasks whose status is OPEN", and
        // this report is what took this one out of flight.
        execution.open_activity_tasks = execution.open_activity_tasks.saturating_sub(1);
        // The task is closed, so the progress its worker reported through its heartbeat
        // has no clock left to reset.
        execution
            .activity_task_heartbeats
            .remove(&handle.started_event_id);
        // The decider has to act on the result, and the decision task scheduled here is
        // what carries it to one.
        execution.schedule_decision_task_if_idle(at);
        Ok(())
    }

    /// Record a worker's PROGRESS report about the activity task `token` is the handle
    /// on, as of `at`, and answer whether a cancellation is being attempted for that
    /// task.
    ///
    /// `RecordActivityTaskHeartbeat`'s two documented effects, and nothing else: the
    /// call "resets the taskHeartbeatTimeout clock", so the task's `heartbeatTimeout`
    /// runs from `at` from now on and the `details` reported here is what an
    /// `ActivityTaskTimedOut` event would carry; and it "doesn't in itself create an
    /// event in the workflow execution history", so no event is appended, the task stays
    /// open and the decider is told nothing. `Ok` carries `cancelRequested`, "set to true
    /// if cancellation of the task is requested" — which is what a
    /// `RequestCancelActivityTask` decision recorded against this task asked for, since a
    /// task a worker already holds "is told through its heartbeat" rather than cancelled
    /// outright.
    ///
    /// `Err` carries the `UnknownResourceFault` message for a token that addresses no
    /// OPEN activity task, exactly as [`Self::close_activity_task`] does: the two calls
    /// resolve the same kind of handle against the same stored history, so a handle that
    /// is stale for one is stale for the other. Nothing is written in that case.
    pub fn record_activity_task_heartbeat(
        &mut self,
        token: &str,
        details: Option<String>,
        at: DateTime,
    ) -> Result<bool, String> {
        let Some(handle) = activity_task_by_token(self, token) else {
            // The token is a handle on no activity task; a token of another KIND this
            // service mints still addresses a task of the stored history, and that is the
            // task the fault names.
            return Err(unknown_activity_message(scheduled_event_id_by_token(self, token)));
        };
        let unknown = unknown_activity_message(handle.scheduled_event_id);
        // The service's own sweeps first: a task the clock timed out, or a run it closed,
        // was over at its own instant whether or not this worker called, so a heartbeat
        // that lost that race is refused instead of reviving a task that is gone.
        self.materialise_due_timeouts_in(&handle.domain, at);
        let Some(domain) = self.domains.get_mut(&handle.domain) else {
            return Err(unknown);
        };
        let key = (handle.workflow_id.clone(), handle.run_id.clone());
        let Some(execution) = domain.executions.get_mut(&key) else {
            return Err(unknown);
        };
        if !execution.is_open() || !execution.activity_task_is_outstanding(handle.started_event_id)
        {
            return Err(unknown);
        }
        execution.activity_task_heartbeats.insert(
            handle.started_event_id,
            ActivityTaskHeartbeat { at, details },
        );
        Ok(execution.latest_cancel_requested_event_id(handle.scheduled_event_id) != 0)
    }
}

/// The terminal report a worker makes about ONE activity task it was handed: the three
/// ways an activity task closes on its worker's own say-so, one per `Respond*` call.
#[derive(Debug, Clone)]
pub enum ActivityTaskClose {
    /// `RespondActivityTaskCompleted`: "the ActivityTask identified by the taskToken
    /// completed successfully with a result".
    Completed { result: Option<String> },
    /// `RespondActivityTaskFailed`: "the ActivityTask identified by the taskToken has
    /// failed with reason (if specified)".
    Failed {
        reason: Option<String>,
        details: Option<String>,
    },
    /// `RespondActivityTaskCanceled`: "the ActivityTask identified by the taskToken was
    /// successfully canceled".
    Canceled { details: Option<String> },
}

impl ActivityTaskClose {
    /// The one history event this report records, naming the task it closes by the two
    /// events that scheduled and started it.
    fn event(
        self,
        scheduled_event_id: i64,
        started_event_id: i64,
        latest_cancel_requested_event_id: i64,
    ) -> EventAttributes {
        match self {
            Self::Completed { result } => {
                EventAttributes::ActivityTaskCompleted(ActivityTaskCompletedEventAttributes {
                    result,
                    scheduled_event_id,
                    started_event_id,
                })
            }
            Self::Failed { reason, details } => {
                EventAttributes::ActivityTaskFailed(ActivityTaskFailedEventAttributes {
                    reason,
                    details,
                    scheduled_event_id,
                    started_event_id,
                })
            }
            Self::Canceled { details } => {
                EventAttributes::ActivityTaskCanceled(ActivityTaskCanceledEventAttributes {
                    details,
                    scheduled_event_id,
                    started_event_id,
                    latest_cancel_requested_event_id,
                })
            }
        }
    }
}

/// The resource portion of a domain ARN, before the domain's own name. SWF's domain
/// ARNs are the slash-prefixed form the service's own `domainInfo.arn` carries.
pub const DOMAIN_RESOURCE_PREFIX: &str = "/domain/";

/// The ARN of a domain: `arn:aws:swf:<region>:<account>:/domain/<name>`.
pub fn domain_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DOMAIN_RESOURCE_PREFIX}{name}"),
    )
}

/// The domain name a `resourceArn` addresses, or `None` when the string is not a
/// domain ARN of this service.
///
/// The inverse of [`domain_arn`]: the tagging operations take an ARN where every other
/// operation takes a domain name, so they resolve the name back out of it. A domain
/// name can contain neither `:` nor `/` ([`validate_name`]), so the segment after the
/// resource prefix is the whole name.
pub fn domain_name_from_arn(arn: &str) -> Option<&str> {
    let mut segments = arn.splitn(6, ':');
    let prefix = segments.next()?;
    let _partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let _account = segments.next()?;
    let resource = segments.next()?;
    if prefix != "arn" || service != ARN_NAMESPACE {
        return None;
    }
    let name = resource.strip_prefix(DOMAIN_RESOURCE_PREFIX)?;
    (!name.is_empty()).then_some(name)
}

/// What Amazon SWF reports when a delete addresses a type that is still `REGISTERED`:
/// the state of the record, in one fixed sentence that names nothing out of the request,
/// so every refused delete of an activity or a workflow type carries exactly this
/// message.
pub const TYPE_NOT_DEPRECATED_MESSAGE: &str =
    "The type is currently registered and cannot be deleted in its current state";

/// The `ActivityType` kind name a type-scoped fault message carries, as the modelled
/// shape is named.
pub const ACTIVITY_TYPE_KIND: &str = "ActivityType";

/// The `WorkflowType` kind name a type-scoped fault message carries, as the modelled
/// shape is named.
pub const WORKFLOW_TYPE_KIND: &str = "WorkflowType";

/// How Amazon SWF names one type in a fault message: `<Kind>=[name=.., version=..]`,
/// built from the requested identity alone. `kind` is [`ACTIVITY_TYPE_KIND`] or
/// [`WORKFLOW_TYPE_KIND`].
///
/// This is the whole clause of a type-scoped fault — the domain the lookup was scoped
/// to is never part of it, so one message form serves every operation that reports a
/// type, whichever domain the request addressed.
pub fn type_reference(kind: &str, name: &str, version: &str) -> String {
    format!("{kind}=[name={name}, version={version}]")
}

/// The `UnknownResourceFault` message for an activity type no stored record of the
/// addressed domain carries: the requested identity and nothing else.
pub fn unknown_activity_type_message(name: &str, version: &str) -> String {
    format!(
        "Unknown type: {}",
        type_reference(ACTIVITY_TYPE_KIND, name, version)
    )
}

/// The `UnknownResourceFault` message for a workflow type no stored record of the
/// addressed domain carries: the requested identity and nothing else.
pub fn unknown_workflow_type_message(name: &str, version: &str) -> String {
    format!(
        "Unknown type: {}",
        type_reference(WORKFLOW_TYPE_KIND, name, version)
    )
}

/// The `UnknownResourceFault` message for an execution no stored run of the addressed
/// domain carries: `Unknown execution: WorkflowExecution=[workflowId=.., runId=..]`,
/// built from the requested execution alone.
///
/// As with [`type_reference`], the domain the lookup was scoped to is NOT part of the
/// clause: every execution-scoped reader of this service reports a missing run this
/// way, whichever domain the request named.
pub fn unknown_execution_message(workflow_id: &str, run_id: &str) -> String {
    format!("Unknown execution: WorkflowExecution=[workflowId={workflow_id}, runId={run_id}]")
}

/// The `UnknownResourceFault` message for an execution addressed by `workflowId` ALONE:
/// `Unknown execution, workflowId = <workflowId>`.
///
/// A request that names no `runId` addresses "the current open workflow execution with
/// the specified workflowId", so the only identifier it supplied is that id — and a
/// fault message renders what the request supplied and nothing else, never a member the
/// caller left out. [`unknown_execution_requested_message`] is what chooses between this
/// form and the `(workflowId, runId)` pair of [`unknown_execution_message`].
pub fn unknown_execution_by_workflow_id_message(workflow_id: &str) -> String {
    format!("Unknown execution, workflowId = {workflow_id}")
}

/// The `UnknownResourceFault` message for an execution an operation taking an OPTIONAL
/// `runId` failed to resolve: the pair when the request addressed a run, the `workflowId`
/// alone when it did not.
pub fn unknown_execution_requested_message(workflow_id: &str, run_id: Option<&str>) -> String {
    match run_id.filter(|id| !id.is_empty()) {
        Some(run_id) => unknown_execution_message(workflow_id, run_id),
        None => unknown_execution_by_workflow_id_message(workflow_id),
    }
}

/// The `UnknownResourceFault` message for a name no domain record of this service is
/// stored under: `Unknown domain: <name>`, built from the request alone.
///
/// This is the clause every operation whose unresolved resource is the DOMAIN reports —
/// a domain lifecycle call, whose subject is the domain itself, and a call that works
/// inside a domain (a listing, a poll, a registration or a start writing into it), which
/// resolves the enclosing domain before anything in it.
pub fn unknown_domain_message(name: &str) -> String {
    format!("Unknown domain: {name}")
}

/// The `UnknownResourceFault` message for an ARN that names no stored domain of this
/// service, shared by the three tag operations that address a domain by its ARN.
///
/// Built from the request: when the ARN is a well-formed SWF domain ARN it names the
/// domain the caller asked about, in the same `Unknown domain: <name>` form
/// `DescribeDomain` answers for a name that was never registered. A string that is not a
/// domain ARN of this service names no domain at all, so the message reports what was
/// asked for instead.
pub fn unknown_domain_resource_message(resource_arn: &str) -> String {
    match domain_name_from_arn(resource_arn) {
        Some(name) => unknown_domain_message(name),
        None => format!("Unknown resource: {resource_arn}"),
    }
}

/// The `UnknownResourceFault` message for an activity task a worker's terminal response
/// addresses and that is no longer open: `Unknown activity, scheduledEventId = <id>`.
///
/// The id is the `scheduledEventId` of the task the SUPPLIED TOKEN addresses, taken from
/// the stored history whichever KIND of task token it is
/// ([`scheduled_event_id_by_token`]) — AWS reads that id out of the token itself, so a
/// token of the wrong kind still names the task it really is a handle on. Only a token
/// that is a handle on no started task of any stored run names no scheduling event, and
/// reports the `EventId` member's own modelled default of `0`.
pub fn unknown_activity_message(scheduled_event_id: i64) -> String {
    format!("Unknown activity, scheduledEventId = {scheduled_event_id}")
}

/// The `UnknownResourceFault` message for a decision task a decider's completion
/// addresses and that is no longer outstanding, while its run is still available:
/// `Unknown decision task, scheduledEventId = <id>`.
///
/// The id is the `scheduledEventId` of the task the SUPPLIED TOKEN addresses, taken from
/// the stored history whichever KIND of task token it is
/// ([`scheduled_event_id_by_token`]); only a token that is a handle on no started task of
/// any stored run names no scheduling event, and reports the `EventId` member's own
/// modelled default of `0`. Built the same way, and for the same reason, as
/// [`unknown_activity_message`]: the fault names the resource the lookup failed on, and
/// the only pointer back to a task that is over is the history event that scheduled it.
pub fn unknown_decision_task_message(scheduled_event_id: i64) -> String {
    format!("Unknown decision task, scheduledEventId = {scheduled_event_id}")
}

/// What Amazon SWF reports when a request would carry a resource past the documented
/// maximum of [`MAX_TAGS_PER_DOMAIN`] tags: one fixed sentence that names no part of the
/// request and no part of the store, so every refusal of that bound reads the same
/// whichever call tripped it and whichever bound tripped first.
pub const TOO_MANY_TAGS_MESSAGE: &str = "Too many tags provided.";

/// One started task a `taskToken` is the handle on, as the stored history records it,
/// whichever KIND of task this service minted that token for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHandle {
    /// Which kind of task the token is a handle on, as the started event it was derived
    /// from records.
    pub kind: TaskKind,
    pub domain: String,
    pub workflow_id: String,
    pub run_id: String,
    /// The `eventId` of the event that scheduled the task, as its started event names it.
    pub scheduled_event_id: i64,
    /// The `eventId` of the started event that handed the task over, which is the
    /// identity the token is derived from.
    pub started_event_id: i64,
}

/// The task `token` is the handle on, of WHICHEVER kind this service minted it for,
/// whether or not that task is still outstanding and whether or not its run is still
/// open.
///
/// Every task token of this service is derived from the identity of the started event
/// that handed one task over — `(kind, domain, workflowId, runId, startedEventId)`
/// ([`task_token`]) — so the stored `DecisionTaskStarted` and `ActivityTaskStarted`
/// events ARE the record of which tokens were ever minted, and one scan over them
/// resolves a handle of either kind. The kind-specific lookups
/// ([`decision_task_by_token`], [`activity_task_by_token`]) are this scan narrowed to the
/// kind their caller can act on; a caller that only needs the task the token names,
/// whatever kind it is — a fault that reports WHICH task the lookup failed on — takes it
/// from here.
pub fn task_by_token(
    guard: &ComAmazonawsSwfSimpleworkflowserviceState,
    token: &str,
) -> Option<TaskHandle> {
    guard.domains.iter().find_map(|(name, domain)| {
        domain.executions.values().find_map(|execution| {
            execution.history.iter().find_map(|event| {
                let (kind, scheduled_event_id) = match &event.attributes {
                    EventAttributes::DecisionTaskStarted(started) => {
                        (TaskKind::Decision, started.scheduled_event_id)
                    }
                    EventAttributes::ActivityTaskStarted(started) => {
                        (TaskKind::Activity, started.scheduled_event_id)
                    }
                    _ => return None,
                };
                let minted = task_token(
                    kind,
                    name,
                    &execution.workflow_id,
                    &execution.run_id,
                    event.event_id,
                );
                (minted == token).then(|| TaskHandle {
                    kind,
                    domain: name.clone(),
                    workflow_id: execution.workflow_id.clone(),
                    run_id: execution.run_id.clone(),
                    scheduled_event_id,
                    started_event_id: event.event_id,
                })
            })
        })
    })
}

/// The `scheduledEventId` of the task `token` addresses, whichever KIND of task token it
/// is, and the `EventId` member's own modelled default of `0` for a token that is a
/// handle on no started task of any stored run.
///
/// What an `UnknownResourceFault` about a task reports, so a refusal names the task the
/// supplied token really is a handle on even when that task is not of the kind the call
/// can act on: the id belongs to the token, not to the lookup that failed.
pub fn scheduled_event_id_by_token(
    guard: &ComAmazonawsSwfSimpleworkflowserviceState,
    token: &str,
) -> i64 {
    task_by_token(guard, token).map_or(0, |handle| handle.scheduled_event_id)
}

/// One decision task a decision `taskToken` is the handle on, as the stored history
/// records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTaskHandle {
    pub domain: String,
    pub workflow_id: String,
    pub run_id: String,
    /// The `eventId` of the `DecisionTaskScheduled` event that put the task in its list.
    pub scheduled_event_id: i64,
    /// The `eventId` of the `DecisionTaskStarted` event that handed it to a decider,
    /// which is the identity the token is derived from.
    pub started_event_id: i64,
}

/// The decision task `token` is the handle on, whether or not that task is still
/// outstanding and whether or not its run is still open.
///
/// The token is derived from `(domain, workflowId, runId, startedEventId)`
/// ([`decision_task_token`]), so the stored `DecisionTaskStarted` events ARE the record of
/// which tokens were ever minted: a handle on a task that has since been completed or
/// timed out still resolves here, which is what lets a decider's late completion be
/// refused naming what it was holding rather than nothing at all. Whether the task is
/// still outstanding is a separate question, answered by
/// [`Execution::decision_task_is_outstanding`].
pub fn decision_task_by_token(
    guard: &ComAmazonawsSwfSimpleworkflowserviceState,
    token: &str,
) -> Option<DecisionTaskHandle> {
    let handle = task_by_token(guard, token).filter(|handle| handle.kind == TaskKind::Decision)?;
    Some(DecisionTaskHandle {
        domain: handle.domain,
        workflow_id: handle.workflow_id,
        run_id: handle.run_id,
        scheduled_event_id: handle.scheduled_event_id,
        started_event_id: handle.started_event_id,
    })
}

/// One activity task an activity `taskToken` is the handle on, as the stored history
/// records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityTaskHandle {
    pub domain: String,
    pub workflow_id: String,
    pub run_id: String,
    /// The `eventId` of the `ActivityTaskScheduled` event that put the task in its list.
    pub scheduled_event_id: i64,
    /// The `eventId` of the `ActivityTaskStarted` event that handed it to a worker, which
    /// is the identity the token is derived from.
    pub started_event_id: i64,
}

/// The activity task `token` is the handle on, whether or not that task is still open.
///
/// The token is derived from `(domain, workflowId, runId, startedEventId)`
/// ([`activity_task_token`]), so the stored `ActivityTaskStarted` events ARE the record of
/// which tokens were ever minted: a handle on a task that has since closed or timed out
/// still resolves here, which is what lets a worker's late response be refused naming the
/// task it was holding rather than nothing at all. Whether the task is still open is a
/// separate question, answered by [`Execution::activity_task_is_outstanding`].
pub fn activity_task_by_token(
    guard: &ComAmazonawsSwfSimpleworkflowserviceState,
    token: &str,
) -> Option<ActivityTaskHandle> {
    let handle = task_by_token(guard, token).filter(|handle| handle.kind == TaskKind::Activity)?;
    Some(ActivityTaskHandle {
        domain: handle.domain,
        workflow_id: handle.workflow_id,
        run_id: handle.run_id,
        scheduled_event_id: handle.scheduled_event_id,
        started_event_id: handle.started_event_id,
    })
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// A freshly minted, unique `runId` for a start in `domain`.
///
/// `sequence` comes from [`Domain::next_run_sequence`], which is strictly increasing
/// per domain, so `(domain, sequence)` is unique across the store and therefore so is
/// this id — two starts in the same nanosecond still get different ones. The digest
/// makes the id opaque the way the real service's run ids are: a caller cannot derive
/// it from its own request.
///
/// The 32 digest bytes are written in base64, the mixed-case alphanumeric-plus-`+/=`
/// alphabet Amazon SWF's own run ids are written in, which is 44 characters — inside
/// the modelled `WorkflowRunId` length (1..64) — and the same for every start, so one
/// generator serves every run this service mints.
pub fn new_run_id(domain: &str, workflow_id: &str, sequence: u64, at: DateTime) -> String {
    use ::base64::Engine as _;

    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0u8]);
    digest.update(workflow_id.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    digest.update(at.secs().to_be_bytes());
    digest.update(at.subsec_nanos().to_be_bytes());
    ::base64::engine::general_purpose::STANDARD.encode(digest.finalize())
}

/// The opaque `taskToken` a `DecisionTask` carries: a handle on ONE decision task of
/// one run, derived from the task's identity.
///
/// `TaskToken`'s doc calls it "The opaque string used as a handle on the task", and the
/// task it handles is exactly `(domain, workflowId, runId, startedEventId)` — the
/// `DecisionTaskStarted` event that handed the task over. Deriving the token from that
/// identity (and not from a counter) means it addresses the task itself: the responder
/// resolves it back to the run, and whether the token is still LIVE is a property of
/// the stored history ([`Execution::decision_task_is_outstanding`]) rather than of a
/// second record that could disagree with it. The SHA-256 hex digest is 64 characters,
/// inside the modelled `TaskToken` length (1..1024), and keeps the handle opaque.
pub fn decision_task_token(
    domain: &str,
    workflow_id: &str,
    run_id: &str,
    started_event_id: i64,
) -> String {
    task_token(TaskKind::Decision, domain, workflow_id, run_id, started_event_id)
}

/// The opaque `taskToken` an `ActivityTask` carries: a handle on ONE activity task of
/// one run, derived the same way as a decision task's
/// ([`decision_task_token`]) from the `ActivityTaskStarted` event that handed the task
/// to a worker, and whose liveness is likewise a property of the stored history
/// ([`Execution::activity_task_is_outstanding`]).
pub fn activity_task_token(
    domain: &str,
    workflow_id: &str,
    run_id: &str,
    started_event_id: i64,
) -> String {
    task_token(TaskKind::Activity, domain, workflow_id, run_id, started_event_id)
}

/// The token of one task of `kind`, over the identity that task has and nothing else.
///
/// The kind is part of the digest, so the handle on a decision task is never also a
/// handle on an activity task: a responder that resolves a token of its own kind refuses
/// the other kind's token instead of acting on the wrong task.
fn task_token(
    kind: TaskKind,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
    started_event_id: i64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.token_tag().as_bytes());
    digest.update([0u8]);
    digest.update(domain.as_bytes());
    digest.update([0u8]);
    digest.update(workflow_id.as_bytes());
    digest.update([0u8]);
    digest.update(run_id.as_bytes());
    digest.update([0u8]);
    digest.update(started_event_id.to_be_bytes());
    ::hex::encode(digest.finalize())
}

/// The `clientToken` a `DecisionTaskStarted` or `ActivityTaskStarted` event carries: the
/// service's own handle on ONE start of one task, in the UUID form AWS reports it in.
///
/// Derived from the identity of that start — `(domain, workflowId, runId, the eventId the
/// started event takes)` — so the token is minted once, stays with the stored event, and
/// reads back the same however often the history is paged; two starts never share one,
/// because no two events of a run share an `eventId`, whichever kind of task they handed
/// over. Deriving it from a digest (the same `sha2` hash the run ids and task tokens are
/// built from, and the hash RFC 4122's name-based form prescribes) is what makes it opaque
/// to the caller: nothing of the request can be recovered from it.
///
/// The 16 digest bytes are laid out as a name-based UUID: the version nibble carries `5`
/// and the variant bits `0b10`, as RFC 4122 §4.3 defines for a UUID derived from a name
/// rather than from a clock or a random source.
pub fn task_start_client_token(
    domain: &str,
    workflow_id: &str,
    run_id: &str,
    started_event_id: i64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(CLIENT_TOKEN_TAG.as_bytes());
    digest.update([0u8]);
    digest.update(domain.as_bytes());
    digest.update([0u8]);
    digest.update(workflow_id.as_bytes());
    digest.update([0u8]);
    digest.update(run_id.as_bytes());
    digest.update([0u8]);
    digest.update(started_event_id.to_be_bytes());

    let mut bytes = [0u8; 16];
    for (slot, byte) in bytes.iter_mut().zip(digest.finalize()) {
        *slot = byte;
    }
    if let Some(version) = bytes.get_mut(6) {
        *version = (*version & 0x0f) | 0x50;
    }
    if let Some(variant) = bytes.get_mut(8) {
        *variant = (*variant & 0x3f) | 0x80;
    }

    // The canonical 8-4-4-4-12 grouping of the 32 hex digits, taken off the encoder in
    // order so no index can run past it.
    let encoded = ::hex::encode(bytes);
    let mut digits = encoded.chars();
    let groups: Vec<String> = UUID_GROUP_LENGTHS
        .iter()
        .map(|length| digits.by_ref().take(*length).collect())
        .collect();
    groups.join("-")
}

/// Which page of which decision task's history a `PollForDecisionTask` `nextPageToken`
/// resumes: the run the task belongs to, the `DecisionTaskStarted` event that handed it
/// over, and the `eventId` the previous page ended on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTaskPage {
    pub workflow_id: String,
    pub run_id: String,
    pub started_event_id: i64,
    pub resume_after_event_id: i64,
}

/// `PollForDecisionTask`'s own `nextPageToken`.
///
/// The operation's doc is explicit that this token is not a history token: "The
/// nextPageToken returned by this action cannot be used with
/// GetWorkflowExecutionHistory to get the next page. You must call
/// PollForDecisionTask again (with the nextPageToken)". A poll names only a domain and
/// a task list, so the token has to carry WHICH task's history is being paged —
/// otherwise the second call could not tell a further page from a request for a new
/// task. It therefore carries the run, the started event that identifies the task, and
/// the event the page ended on, plus the ordering tag every token of this service
/// carries so a token replayed under the other `reverseOrder` is refused instead of
/// silently skipping or repeating events.
pub fn encode_decision_task_page_token(reverse_order: bool, page: &DecisionTaskPage) -> String {
    // Hex-encoded for the same reason as [`encode_execution_page_token`]: the token is
    // a plain response member and a `workflowId` may hold any character SWF's naming
    // rule allows.
    let payload = format!(
        "{}\u{0}{}\u{0}{}\u{0}{}",
        page.started_event_id, page.resume_after_event_id, page.workflow_id, page.run_id
    );
    format!(
        "{}{}",
        decision_task_order_tag(reverse_order),
        ::hex::encode(payload)
    )
}

/// The [`DecisionTaskPage`] a `PollForDecisionTask` `nextPageToken` resumes, for a poll
/// reading in `reverse_order`.
pub fn decode_decision_task_page_token(
    token: &str,
    reverse_order: bool,
) -> Result<DecisionTaskPage, String> {
    let rejected = || {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} \
             PollForDecisionTask; pass the token back unchanged, with every other argument \
             unchanged"
        )
    };
    let payload = token
        .strip_prefix(decision_task_order_tag(reverse_order))
        .and_then(|encoded| ::hex::decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;
    let mut parts = payload.split('\u{0}');
    let (Some(started), Some(resume_after), Some(workflow_id), Some(run_id), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(rejected());
    };
    Ok(DecisionTaskPage {
        workflow_id: workflow_id.to_string(),
        run_id: run_id.to_string(),
        started_event_id: started.parse::<i64>().map_err(|_| rejected())?,
        resume_after_event_id: resume_after.parse::<i64>().map_err(|_| rejected())?,
    })
}

/// The ordering tag [`encode_decision_task_page_token`] stamps into a page token. It
/// differs from [`history_order_tag`] so that a `GetWorkflowExecutionHistory` token and
/// a `PollForDecisionTask` token cannot be mistaken for one another — the operation's
/// doc says they are not interchangeable.
fn decision_task_order_tag(reverse_order: bool) -> &'static str {
    if reverse_order {
        "dr"
    } else {
        "df"
    }
}

/// An optional string member, with an empty string read as "no value given".
///
/// The model draws exactly this distinction: every overridable duration targets
/// `DurationInSecondsOptional` (length 0..8, so `""` is on the wire) while every
/// duration that must carry a value targets `DurationInSeconds` (length 1..8). So an
/// empty optional member is an absent one, and falls back to the registered default.
pub fn non_empty(value: &Option<String>) -> Option<String> {
    value.as_ref().filter(|text| !text.is_empty()).cloned()
}

/// `GetWorkflowExecutionHistory`'s `nextPageToken` for the next unread event: the
/// ordering the page was read in, then the `eventId` the page ended on.
///
/// The token names an EVENT, not a position, so appending to the log between pages
/// cannot shift what it addresses — which matters most under `reverseOrder`, where a
/// newly appended event lands at the front of the ordering. The ordering is part of
/// the token because "the event after this one" differs between the two orderings;
/// carrying it lets a token replayed against the other ordering be refused instead of
/// silently skipping or repeating events.
pub fn encode_history_page_token(reverse_order: bool, last_event_id: i64) -> String {
    format!("{}{last_event_id}", history_order_tag(reverse_order))
}

/// The `eventId` a `nextPageToken` resumes AFTER, for a request reading in
/// `reverse_order`.
pub fn decode_history_page_token(token: &str, reverse_order: bool) -> Result<i64, String> {
    let expected = history_order_tag(reverse_order);
    let event_id = token
        .strip_prefix(expected)
        .and_then(|event_id| event_id.parse::<i64>().ok());
    event_id.ok_or_else(|| {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} read of \
             this execution's history; pass the token back with the same reverseOrder"
        )
    })
}

/// A timestamp as an orderable pair of whole seconds and nanoseconds.
///
/// The wire timestamp type carries no total order of its own, and every visibility
/// listing has to order executions by time, so this is the single place that turns one
/// into a comparable key.
pub fn instant_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// One page of a run's history, and whether the log holds more after it.
///
/// Both readers of a history page follow this one rule — `GetWorkflowExecutionHistory`
/// ("the result returned might be split up across a number of pages") and
/// `PollForDecisionTask` ("a paginated view of the history of the workflow execution"):
/// `reverse_order` chooses the direction ("By default the results are returned in
/// ascending order of the eventTimestamp of the events"), `page_size` bounds the page,
/// and `resume_after` names the event the previous page ended on, so appending to the
/// log between two pages cannot shift what the next one holds.
///
/// `Err` when `resume_after` names an event this log does not hold — the token does not
/// address this history.
pub fn history_page(
    history: &[HistoryEventRecord],
    reverse_order: bool,
    page_size: usize,
    resume_after: Option<i64>,
) -> Result<(Vec<&HistoryEventRecord>, bool), String> {
    let ordered: Vec<&HistoryEventRecord> = if reverse_order {
        history.iter().rev().collect()
    } else {
        history.iter().collect()
    };
    let total = ordered.len();
    // The position is searched for rather than computed from the id, so the cursor
    // stays correct for any log the store can hold — including one whose ids are not a
    // dense 1..n range.
    let start = match resume_after {
        Some(event_id) => ordered
            .iter()
            .position(|record| record.event_id == event_id)
            .map(|position| position + 1)
            .ok_or_else(|| {
                format!(
                    "nextPageToken resumes after event {event_id}, which this execution's \
                     history does not hold"
                )
            })?,
        None => 0,
    };
    let end = (start + page_size).min(total);
    Ok((ordered[start..end].to_vec(), end < total))
}

/// The total order a visibility listing of workflow executions pages through: the
/// instant the listing works from, then the execution's `(workflowId, runId)` identity.
///
/// The identity is part of the key because the store holds executions in a map with no
/// order of its own: two runs sharing an instant would otherwise have no defined
/// position, and a page boundary between them would not be reproducible.
pub type ExecutionOrderKey = ((i64, u32), String, String);

/// Which stored instant a visibility listing of workflow executions filters, orders
/// and pages by.
///
/// The listing itself chooses: `ListClosedWorkflowExecutions` documents the choice on
/// each of its two time filters — `closeTimeFilter` "if this parameter is specified,
/// the returned results are ordered by their close times" against `startTimeFilter`
/// "ordered by their start times" — so the instant is a parameter of the listing
/// rather than a property of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTimeBasis {
    /// The instant the run was started — `WorkflowExecutionInfo.startTimestamp`,
    /// which every stored run carries.
    StartTime,
    /// The instant the run was closed — `WorkflowExecutionInfo.closeTimestamp`, "Set
    /// only if the execution status is CLOSED", so an OPEN run has none.
    CloseTime,
}

/// The instant `basis` names on one stored execution, or `None` when that execution
/// has no such instant (only a CLOSED run carries a close instant).
pub fn execution_instant(execution: &Execution, basis: ExecutionTimeBasis) -> Option<DateTime> {
    match basis {
        ExecutionTimeBasis::StartTime => Some(execution.start_timestamp),
        ExecutionTimeBasis::CloseTime => execution.close_timestamp,
    }
}

/// The [`ExecutionOrderKey`] of one stored execution under `basis`.
///
/// An execution with no instant for `basis` falls back to its start instant, which
/// keeps the order total: such a run is still OPEN, and a listing ordered by close
/// time reports only closed ones, so the fallback orders nothing a caller sees.
pub fn execution_order_key(execution: &Execution, basis: ExecutionTimeBasis) -> ExecutionOrderKey {
    (
        instant_key(execution_instant(execution, basis).unwrap_or(execution.start_timestamp)),
        execution.workflow_id.clone(),
        execution.run_id.clone(),
    )
}

/// Whether an execution's start (or close) instant falls inside an
/// `ExecutionTimeFilter` window.
///
/// The filter's doc: "Each parameter, if specified, defines a rule that must be
/// satisfied by each returned query result" — `oldestDate` is required and
/// `latestDate` is optional, so an absent `latestDate` leaves the window open-ended at
/// the recent end. Both ends are inclusive.
pub fn within_time_window(at: DateTime, filter: &ExecutionTimeFilter) -> bool {
    let at = instant_key(at);
    at >= instant_key(filter.oldest_date)
        && filter
            .latest_date
            .is_none_or(|latest| at <= instant_key(latest))
}

/// Whether an execution satisfies the `executionFilter` / `typeFilter` / `tagFilter`
/// the request named. A filter that was not given constrains nothing.
///
/// Each member's doc states its own rule: `executionFilter` keeps "only workflow
/// executions matching the workflow ID specified in the filter", `typeFilter` "only
/// executions of the type specified in the filter" (whose `version` is optional, so an
/// absent one keeps every version registered under that name), and `tagFilter` "only
/// executions that have the matching tag".
pub fn execution_matches_filters(
    execution: &Execution,
    execution_filter: Option<&WorkflowExecutionFilter>,
    type_filter: Option<&WorkflowTypeFilter>,
    tag_filter: Option<&TagFilter>,
) -> bool {
    let id_matches =
        execution_filter.is_none_or(|filter| execution.workflow_id == filter.workflow_id);
    let type_matches = type_filter.is_none_or(|filter| {
        execution.workflow_type.0 == filter.name
            && non_empty(&filter.version).is_none_or(|version| execution.workflow_type.1 == version)
    });
    let tag_matches = tag_filter.is_none_or(|filter| execution.tag_list.contains(&filter.tag));
    id_matches && type_matches && tag_matches
}

/// Refuse a visibility request that names more than one of the mutually exclusive
/// filters, naming the ones it did supply.
///
/// Every visibility operation documents the same rule on each of them:
/// "executionFilter, typeFilter and tagFilter are mutually exclusive. You can specify
/// at most one of these in a request." The model cannot express it, so the boundary
/// validator passes it and the handler applies this.
///
/// The refusal names the conflicting filters by the SHAPE each input member targets
/// (`WorkflowExecutionFilter`, `WorkflowTypeFilter`, `TagFilter`, `CloseStatusFilter`),
/// as a bracketed list in the order the operation declares them, which is the
/// machine-parsable list a caller reads the conflict out of. Each caller passes the
/// shape names its own input members target.
pub fn validate_exclusive_filters(filters: &[(&str, bool)]) -> Result<(), String> {
    let supplied: Vec<&str> = filters
        .iter()
        .filter(|(_, given)| *given)
        .map(|(shape, _)| *shape)
        .collect();
    if supplied.len() > 1 {
        return Err(format!(
            "Cannot specify more than one exclusive filters in the same query: [{}]",
            supplied.join(", ")
        ));
    }
    Ok(())
}

/// The time window a closed-execution visibility request selects on, and the instant
/// it is taken on, from the two time filters such a request may name.
///
/// Both members document the same rule: "startTimeFilter and closeTimeFilter are
/// mutually exclusive. You must specify one of these in a request but not both." So
/// exactly one is given, and which one it is chooses the basis; a request naming both,
/// or neither, names no window and is refused. The model cannot express the rule, so
/// the boundary validator passes it and the handler applies this.
pub fn resolve_execution_time_window<'a>(
    start_time_filter: Option<&'a ExecutionTimeFilter>,
    close_time_filter: Option<&'a ExecutionTimeFilter>,
) -> Result<(ExecutionTimeBasis, &'a ExecutionTimeFilter), String> {
    let refused = |got: &str| {
        format!(
            "startTimeFilter and closeTimeFilter are mutually exclusive; specify one of them \
             in a request but not both, got {got}"
        )
    };
    match (start_time_filter, close_time_filter) {
        (Some(window), None) => Ok((ExecutionTimeBasis::StartTime, window)),
        (None, Some(window)) => Ok((ExecutionTimeBasis::CloseTime, window)),
        (Some(_), Some(_)) => Err(refused("both")),
        (None, None) => Err(refused("neither")),
    }
}

/// A visibility listing's `nextPageToken` for the next unread execution: the ordering
/// the page was read in, then the [`ExecutionOrderKey`] the page ended on.
///
/// The token names a POSITION IN THE ORDER rather than a stored execution, so a run
/// that starts or closes between two pages cannot invalidate it: the next page is
/// whatever still sorts after that key. The ordering — both its direction and the
/// instant it is taken on — is part of the token because "after this one" means
/// something different in each of them; carrying it lets a token replayed against
/// another ordering be refused instead of silently skipping or repeating executions,
/// which is what "Keep all other arguments unchanged" asks of the caller.
pub fn encode_execution_page_token(
    reverse_order: bool,
    basis: ExecutionTimeBasis,
    key: &ExecutionOrderKey,
) -> String {
    let ((secs, nanos), workflow_id, run_id) = key;
    // Hex-encoded because the token is a response member with no structure of its own,
    // and a `workflowId` may hold any character SWF's naming rule allows.
    let payload = format!("{secs}.{nanos}\u{0}{workflow_id}\u{0}{run_id}");
    format!(
        "{}{}",
        execution_order_tag(reverse_order, basis),
        ::hex::encode(payload)
    )
}

/// The [`ExecutionOrderKey`] a `nextPageToken` resumes AFTER, for a request listing in
/// `reverse_order` on `basis`.
pub fn decode_execution_page_token(
    token: &str,
    reverse_order: bool,
    basis: ExecutionTimeBasis,
) -> Result<ExecutionOrderKey, String> {
    let rejected = || {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} listing \
             of this domain's workflow executions ordered by {}; pass the token back \
             unchanged, with every other argument unchanged",
            match basis {
                ExecutionTimeBasis::StartTime => "start time",
                ExecutionTimeBasis::CloseTime => "close time",
            }
        )
    };
    let payload = token
        .strip_prefix(execution_order_tag(reverse_order, basis))
        .and_then(|encoded| ::hex::decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;
    let mut parts = payload.split('\u{0}');
    let (Some(instant), Some(workflow_id), Some(run_id), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(rejected());
    };
    let (secs, nanos) = instant.split_once('.').ok_or_else(rejected)?;
    let secs = secs.parse::<i64>().map_err(|_| rejected())?;
    let nanos = nanos.parse::<u32>().map_err(|_| rejected())?;
    Ok((
        (secs, nanos),
        workflow_id.to_string(),
        run_id.to_string(),
    ))
}

/// The ordering tag [`encode_execution_page_token`] stamps into a page token: the
/// direction the page was walked in, then the instant the order was taken on.
fn execution_order_tag(reverse_order: bool, basis: ExecutionTimeBasis) -> &'static str {
    match (reverse_order, basis) {
        (false, ExecutionTimeBasis::StartTime) => "ds",
        (true, ExecutionTimeBasis::StartTime) => "as",
        (false, ExecutionTimeBasis::CloseTime) => "dc",
        (true, ExecutionTimeBasis::CloseTime) => "ac",
    }
}

/// The total order a listing of a domain's registered types pages through: the type's
/// `name`, then its `version`.
///
/// The name comes first because that is the order the listings document — "the results
/// are returned in ascending alphabetical order by name" — and the version is part of
/// the key because the store holds types in a map with no order of its own: the several
/// versions registered under one name would otherwise have no defined position, and a
/// page boundary between them would not be reproducible.
pub type TypeOrderKey = (String, String);

/// The [`TypeOrderKey`] of one registered type identity.
pub fn type_order_key(name: &str, version: &str) -> TypeOrderKey {
    (name.to_string(), version.to_string())
}

/// A type listing's `nextPageToken` for the next unread type: the ordering the page was
/// read in, then the [`TypeOrderKey`] the page ended on.
///
/// The token names a POSITION IN THE ORDER rather than a stored record, so a type
/// registered, deprecated or deleted between two pages cannot invalidate it: the next
/// page is whatever still sorts after that key. The direction is part of the token
/// because "after this one" means something different in each of them; carrying it lets
/// a token replayed against the other ordering be refused instead of silently skipping
/// or repeating types, which is what "Keep all other arguments unchanged" asks of the
/// caller.
pub fn encode_type_page_token(reverse_order: bool, key: &TypeOrderKey) -> String {
    let (name, version) = key;
    // Hex-encoded for the same reason as [`encode_execution_page_token`]: the token is a
    // response member with no structure of its own, and a type `name` may hold any
    // character SWF's naming rule allows.
    let payload = format!("{name}\u{0}{version}");
    format!("{}{}", type_order_tag(reverse_order), ::hex::encode(payload))
}

/// The [`TypeOrderKey`] a `nextPageToken` resumes AFTER, for a listing in
/// `reverse_order`.
pub fn decode_type_page_token(token: &str, reverse_order: bool) -> Result<TypeOrderKey, String> {
    let rejected = || {
        format!(
            "nextPageToken {token:?} was not issued for a reverseOrder={reverse_order} listing \
             of this domain's registered types; pass the token back unchanged, with every \
             other argument unchanged"
        )
    };
    let payload = token
        .strip_prefix(type_order_tag(reverse_order))
        .and_then(|encoded| ::hex::decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;
    let mut parts = payload.split('\u{0}');
    let (Some(name), Some(version), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(rejected());
    };
    Ok((name.to_string(), version.to_string()))
}

/// The ordering tag [`encode_type_page_token`] stamps into a page token: the direction
/// the page was walked in. It differs from every other tag of this module so that two
/// kinds of token cannot be mistaken for one another.
fn type_order_tag(reverse_order: bool) -> &'static str {
    if reverse_order {
        "tr"
    } else {
        "tf"
    }
}

/// The ordering tag [`encode_history_page_token`] stamps into a page token.
fn history_order_tag(reverse_order: bool) -> &'static str {
    if reverse_order {
        "r"
    } else {
        "f"
    }
}

/// Refuse a name SWF's own naming rule rejects, naming the member and what is wrong.
///
/// The rule is stated identically on every user-supplied SWF name (domain `name`,
/// workflow type `name`/`version`, `workflowId`, task list `name`): "The specified
/// string must not start or end with whitespace. It must not contain a `:` (colon),
/// `/` (slash), `|` (vertical bar), or any control characters (`\u0000-\u001f` |
/// `\u007f-\u009f`). Also, it must not be the literal string `arn`." The model cannot
/// express it, so the boundary validator passes it and each handler applies this.
pub fn validate_name(member: &str, value: &str) -> Result<(), String> {
    if value.trim() != value {
        return Err(format!(
            "{member} must not start or end with whitespace, got {value:?}"
        ));
    }
    if let Some(rejected) = value
        .chars()
        .find(|&c| matches!(c, ':' | '/' | '|' | '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
    {
        return Err(format!(
            "{member} must not contain {rejected:?} (a colon, slash, vertical bar or \
             control character), got {value:?}"
        ));
    }
    if value == "arn" {
        return Err(format!(
            "{member} must not be the literal string \"arn\""
        ));
    }
    Ok(())
}

/// Refuse a `workflowExecutionRetentionPeriodInDays` outside the documented range.
///
/// The member's doc: "If you pass the value NONE or 0 (zero), then the workflow
/// execution history isn't retained. ... The maximum workflow execution retention
/// period is 90 days."
pub fn validate_retention_period(value: &str) -> Result<(), String> {
    if value == "NONE" {
        return Ok(());
    }
    match decimal(value) {
        Some(days) if days <= u64::from(MAX_RETENTION_PERIOD_IN_DAYS) => Ok(()),
        _ => Err(format!(
            "workflowExecutionRetentionPeriodInDays must be \"NONE\" or an integer from 0 to \
             {MAX_RETENTION_PERIOD_IN_DAYS}, got {value:?}"
        )),
    }
}

/// Refuse a decision-task start-to-close duration that is neither a non-negative
/// integer of seconds nor the literal `NONE`.
///
/// `defaultTaskStartToCloseTimeout`'s doc: "The duration is specified in seconds, an
/// integer greater than or equal to 0. You can use NONE to specify unlimited
/// duration."
pub fn validate_task_duration(member: &str, value: &str) -> Result<(), String> {
    if value == "NONE" || decimal(value).is_some() {
        return Ok(());
    }
    Err(format!(
        "{member} must be \"NONE\" or an integer number of seconds, got {value:?}"
    ))
}

/// Refuse an execution start-to-close duration outside what the model documents.
///
/// `executionStartToCloseTimeout`'s doc: "The duration is specified in seconds; an
/// integer greater than or equal to 0. Unlike some of the other timeout parameters in
/// Amazon SWF, you cannot specify a value of \"NONE\" for this timeout; there is a
/// one-year max limit on the time that a workflow execution can run."
pub fn validate_execution_duration(member: &str, value: &str) -> Result<(), String> {
    match decimal(value) {
        Some(seconds) if seconds <= MAX_EXECUTION_START_TO_CLOSE_SECONDS => Ok(()),
        _ => Err(format!(
            "{member} must be an integer number of seconds from 0 to \
             {MAX_EXECUTION_START_TO_CLOSE_SECONDS} (\"NONE\" is not accepted for this timeout), \
             got {value:?}"
        )),
    }
}

/// A plain decimal string as a number, or `None` when it is not one.
///
/// Every SWF duration is documented as "an integer greater than or equal to 0", so a
/// value must be decimal digits only: `str::parse` alone would also accept a signed
/// `"+5"`, which is not that form and which the service stores and echoes back
/// verbatim. Zero padding IS still that form (`"0007"` is seven), and the model
/// constrains only the member's length, so it is accepted.
fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok()
}

/// Refuse a domain tag outside the documented character set.
///
/// `ResourceTag`'s own doc: "Tags may only contain unicode letters, digits,
/// whitespace, or these symbols: `_ . : / = + - @`". The key's and value's lengths are
/// modelled (1..128 and 0..256) and so are refused at the boundary; the character set
/// is not, so it is enforced here.
pub fn validate_tag(key: &str, value: Option<&str>) -> Result<(), String> {
    validate_tag_text("tags.key", key)?;
    match value {
        Some(value) => validate_tag_text("tags.value", value),
        None => Ok(()),
    }
}

/// One side of a tag pair, against the character set [`validate_tag`] documents.
fn validate_tag_text(member: &str, text: &str) -> Result<(), String> {
    if let Some(rejected) = text.chars().find(|c| {
        !(c.is_alphabetic()
            || c.is_numeric()
            || c.is_whitespace()
            || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@'))
    }) {
        return Err(format!(
            "{member} may only contain unicode letters, digits, whitespace or the symbols \
             _ . : / = + - @, got {rejected:?} in {text:?}"
        ));
    }
    Ok(())
}

/// Refuse a child policy outside the model's `ChildPolicy` enumeration.
///
/// Every `childPolicy` / `defaultChildPolicy` member targets that closed enum, so the
/// boundary validator already refuses anything outside it; this keeps a policy no
/// reader could interpret out of the store even if that trait stops being enforced,
/// and keeps the check identical on every handler that stores one.
pub fn validate_child_policy(member: &str, value: &str) -> Result<(), String> {
    if crate::ops::types::CHILD_POLICY_VALUES.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member} must be one of {}, got {value:?}",
        crate::ops::types::CHILD_POLICY_VALUES.join(", ")
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSwfSimpleworkflowserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSwfSimpleworkflowserviceState>)
}
