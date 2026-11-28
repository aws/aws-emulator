// SPDX-License-Identifier: CC0-1.0

//! The shapes more than one operation reaches, as the published model snapshot declares
//! them.
//!
//! Every such shape except the five this service answers with members the snapshot does
//! not declare; those are declared in [`super::overlay`] from this crate's overlay model
//! (see `smithy-model/LOCAL-ADDITIONS.md`).
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ExecutionTimeFilter")]
#[derive(Debug, Clone)]
pub struct ExecutionTimeFilter {
    pub oldest_date: ::aws_smithy_types::DateTime,
    pub latest_date: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionFilter")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionFilter {
    pub workflow_id: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowTypeFilter")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowTypeFilter {
    pub name: String,
    pub version: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TagFilter")]
#[derive(Debug, Clone, Default)]
pub struct TagFilter {
    pub tag: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#CloseStatusFilter")]
#[derive(Debug, Clone, Default)]
pub struct CloseStatusFilter {
    pub status: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionCount")]
#[derive(Debug, Clone, Default)]
pub struct CountClosedWorkflowExecutionsOutput {
    pub count: i32,
    pub truncated: bool,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#OperationNotPermittedFault")]
#[derive(Debug, Clone, Default)]
pub struct OperationNotPermittedFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#UnknownResourceFault")]
#[derive(Debug, Clone, Default)]
pub struct UnknownResourceFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TaskList")]
#[derive(Debug, Clone, Default)]
pub struct TaskList {
    pub name: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#PendingTaskCount")]
#[derive(Debug, Clone, Default)]
pub struct CountPendingActivityTasksOutput {
    pub count: i32,
    pub truncated: bool,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityType")]
#[derive(Debug, Clone, Default)]
pub struct ActivityType {
    pub name: String,
    pub version: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TypeNotDeprecatedFault")]
#[derive(Debug, Clone, Default)]
pub struct TypeNotDeprecatedFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowType")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowType {
    pub name: String,
    pub version: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TypeDeprecatedFault")]
#[derive(Debug, Clone, Default)]
pub struct TypeDeprecatedFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTypeInfo")]
#[derive(Debug, Clone)]
pub struct ActivityTypeInfo {
    pub activity_type: ActivityType,
    pub status: String,
    pub description: Option<String>,
    pub creation_date: ::aws_smithy_types::DateTime,
    pub deprecation_date: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DomainInfo")]
#[derive(Debug, Clone, Default)]
pub struct DomainInfo {
    pub name: String,
    pub status: String,
    pub description: Option<String>,
    pub arn: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecution")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecution {
    pub workflow_id: String,
    pub run_id: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowTypeInfo")]
#[derive(Debug, Clone)]
pub struct WorkflowTypeInfo {
    pub workflow_type: WorkflowType,
    pub status: String,
    pub description: Option<String>,
    pub creation_date: ::aws_smithy_types::DateTime,
    pub deprecation_date: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#HistoryEvent")]
#[derive(Debug, Clone)]
pub struct HistoryEvent {
    pub event_timestamp: ::aws_smithy_types::DateTime,
    pub event_type: String,
    pub event_id: i64,
    pub workflow_execution_started_event_attributes: Option<WorkflowExecutionStartedEventAttributes>,
    pub workflow_execution_completed_event_attributes: Option<WorkflowExecutionCompletedEventAttributes>,
    pub complete_workflow_execution_failed_event_attributes: Option<CompleteWorkflowExecutionFailedEventAttributes>,
    pub workflow_execution_failed_event_attributes: Option<WorkflowExecutionFailedEventAttributes>,
    pub fail_workflow_execution_failed_event_attributes: Option<FailWorkflowExecutionFailedEventAttributes>,
    pub workflow_execution_timed_out_event_attributes: Option<WorkflowExecutionTimedOutEventAttributes>,
    pub workflow_execution_canceled_event_attributes: Option<WorkflowExecutionCanceledEventAttributes>,
    pub cancel_workflow_execution_failed_event_attributes: Option<CancelWorkflowExecutionFailedEventAttributes>,
    pub workflow_execution_continued_as_new_event_attributes: Option<WorkflowExecutionContinuedAsNewEventAttributes>,
    pub continue_as_new_workflow_execution_failed_event_attributes: Option<ContinueAsNewWorkflowExecutionFailedEventAttributes>,
    pub workflow_execution_terminated_event_attributes: Option<WorkflowExecutionTerminatedEventAttributes>,
    pub workflow_execution_cancel_requested_event_attributes: Option<WorkflowExecutionCancelRequestedEventAttributes>,
    pub decision_task_scheduled_event_attributes: Option<DecisionTaskScheduledEventAttributes>,
    pub decision_task_started_event_attributes: Option<DecisionTaskStartedEventAttributes>,
    pub decision_task_completed_event_attributes: Option<DecisionTaskCompletedEventAttributes>,
    pub decision_task_timed_out_event_attributes: Option<DecisionTaskTimedOutEventAttributes>,
    pub activity_task_scheduled_event_attributes: Option<ActivityTaskScheduledEventAttributes>,
    pub activity_task_started_event_attributes: Option<ActivityTaskStartedEventAttributes>,
    pub activity_task_completed_event_attributes: Option<ActivityTaskCompletedEventAttributes>,
    pub activity_task_failed_event_attributes: Option<ActivityTaskFailedEventAttributes>,
    pub activity_task_timed_out_event_attributes: Option<ActivityTaskTimedOutEventAttributes>,
    pub activity_task_canceled_event_attributes: Option<ActivityTaskCanceledEventAttributes>,
    pub activity_task_cancel_requested_event_attributes: Option<ActivityTaskCancelRequestedEventAttributes>,
    pub workflow_execution_signaled_event_attributes: Option<WorkflowExecutionSignaledEventAttributes>,
    pub marker_recorded_event_attributes: Option<MarkerRecordedEventAttributes>,
    pub record_marker_failed_event_attributes: Option<RecordMarkerFailedEventAttributes>,
    pub timer_started_event_attributes: Option<TimerStartedEventAttributes>,
    pub timer_fired_event_attributes: Option<TimerFiredEventAttributes>,
    pub timer_canceled_event_attributes: Option<TimerCanceledEventAttributes>,
    pub start_child_workflow_execution_initiated_event_attributes: Option<StartChildWorkflowExecutionInitiatedEventAttributes>,
    pub child_workflow_execution_started_event_attributes: Option<ChildWorkflowExecutionStartedEventAttributes>,
    pub child_workflow_execution_completed_event_attributes: Option<ChildWorkflowExecutionCompletedEventAttributes>,
    pub child_workflow_execution_failed_event_attributes: Option<ChildWorkflowExecutionFailedEventAttributes>,
    pub child_workflow_execution_timed_out_event_attributes: Option<ChildWorkflowExecutionTimedOutEventAttributes>,
    pub child_workflow_execution_canceled_event_attributes: Option<ChildWorkflowExecutionCanceledEventAttributes>,
    pub child_workflow_execution_terminated_event_attributes: Option<ChildWorkflowExecutionTerminatedEventAttributes>,
    pub signal_external_workflow_execution_initiated_event_attributes: Option<SignalExternalWorkflowExecutionInitiatedEventAttributes>,
    pub external_workflow_execution_signaled_event_attributes: Option<ExternalWorkflowExecutionSignaledEventAttributes>,
    pub signal_external_workflow_execution_failed_event_attributes: Option<SignalExternalWorkflowExecutionFailedEventAttributes>,
    pub external_workflow_execution_cancel_requested_event_attributes: Option<ExternalWorkflowExecutionCancelRequestedEventAttributes>,
    pub request_cancel_external_workflow_execution_initiated_event_attributes: Option<RequestCancelExternalWorkflowExecutionInitiatedEventAttributes>,
    pub request_cancel_external_workflow_execution_failed_event_attributes: Option<RequestCancelExternalWorkflowExecutionFailedEventAttributes>,
    pub schedule_activity_task_failed_event_attributes: Option<ScheduleActivityTaskFailedEventAttributes>,
    pub request_cancel_activity_task_failed_event_attributes: Option<RequestCancelActivityTaskFailedEventAttributes>,
    pub start_timer_failed_event_attributes: Option<StartTimerFailedEventAttributes>,
    pub cancel_timer_failed_event_attributes: Option<CancelTimerFailedEventAttributes>,
    pub start_child_workflow_execution_failed_event_attributes: Option<StartChildWorkflowExecutionFailedEventAttributes>,
    pub lambda_function_scheduled_event_attributes: Option<LambdaFunctionScheduledEventAttributes>,
    pub lambda_function_started_event_attributes: Option<LambdaFunctionStartedEventAttributes>,
    pub lambda_function_completed_event_attributes: Option<LambdaFunctionCompletedEventAttributes>,
    pub lambda_function_failed_event_attributes: Option<LambdaFunctionFailedEventAttributes>,
    pub lambda_function_timed_out_event_attributes: Option<LambdaFunctionTimedOutEventAttributes>,
    pub schedule_lambda_function_failed_event_attributes: Option<ScheduleLambdaFunctionFailedEventAttributes>,
    pub start_lambda_function_failed_event_attributes: Option<StartLambdaFunctionFailedEventAttributes>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionStartedEventAttributes {
    pub input: Option<String>,
    pub execution_start_to_close_timeout: Option<String>,
    pub task_start_to_close_timeout: Option<String>,
    pub child_policy: String,
    pub task_list: TaskList,
    pub task_priority: Option<String>,
    pub workflow_type: WorkflowType,
    pub tag_list: Option<Vec<String>>,
    pub continued_execution_run_id: Option<String>,
    pub parent_workflow_execution: Option<WorkflowExecution>,
    pub parent_initiated_event_id: i64,
    pub lambda_role: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionCompletedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionCompletedEventAttributes {
    pub result: Option<String>,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#CompleteWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct CompleteWorkflowExecutionFailedEventAttributes {
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionFailedEventAttributes {
    pub reason: Option<String>,
    pub details: Option<String>,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#FailWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct FailWorkflowExecutionFailedEventAttributes {
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionTimedOutEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionTimedOutEventAttributes {
    pub timeout_type: String,
    pub child_policy: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionCanceledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionCanceledEventAttributes {
    pub details: Option<String>,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#CancelWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct CancelWorkflowExecutionFailedEventAttributes {
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionContinuedAsNewEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionContinuedAsNewEventAttributes {
    pub input: Option<String>,
    pub decision_task_completed_event_id: i64,
    pub new_execution_run_id: String,
    pub execution_start_to_close_timeout: Option<String>,
    pub task_list: TaskList,
    pub task_priority: Option<String>,
    pub task_start_to_close_timeout: Option<String>,
    pub child_policy: String,
    pub tag_list: Option<Vec<String>>,
    pub workflow_type: WorkflowType,
    pub lambda_role: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ContinueAsNewWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ContinueAsNewWorkflowExecutionFailedEventAttributes {
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionTerminatedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionTerminatedEventAttributes {
    pub reason: Option<String>,
    pub details: Option<String>,
    pub child_policy: String,
    pub cause: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionCancelRequestedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionCancelRequestedEventAttributes {
    pub external_workflow_execution: Option<WorkflowExecution>,
    pub external_initiated_event_id: i64,
    pub cause: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DecisionTaskScheduledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct DecisionTaskScheduledEventAttributes {
    pub task_list: TaskList,
    pub task_priority: Option<String>,
    pub start_to_close_timeout: Option<String>,
    pub schedule_to_start_timeout: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DecisionTaskCompletedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct DecisionTaskCompletedEventAttributes {
    pub execution_context: Option<String>,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub task_list: Option<TaskList>,
    pub task_list_schedule_to_start_timeout: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DecisionTaskTimedOutEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct DecisionTaskTimedOutEventAttributes {
    pub timeout_type: String,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskCompletedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskCompletedEventAttributes {
    pub result: Option<String>,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskFailedEventAttributes {
    pub reason: Option<String>,
    pub details: Option<String>,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskTimedOutEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskTimedOutEventAttributes {
    pub timeout_type: String,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub details: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskCanceledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskCanceledEventAttributes {
    pub details: Option<String>,
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub latest_cancel_requested_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskCancelRequestedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskCancelRequestedEventAttributes {
    pub decision_task_completed_event_id: i64,
    pub activity_id: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionSignaledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionSignaledEventAttributes {
    pub signal_name: String,
    pub input: Option<String>,
    pub external_workflow_execution: Option<WorkflowExecution>,
    pub external_initiated_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#MarkerRecordedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct MarkerRecordedEventAttributes {
    pub marker_name: String,
    pub details: Option<String>,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#RecordMarkerFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct RecordMarkerFailedEventAttributes {
    pub marker_name: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TimerStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct TimerStartedEventAttributes {
    pub timer_id: String,
    pub control: Option<String>,
    pub start_to_fire_timeout: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TimerFiredEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct TimerFiredEventAttributes {
    pub timer_id: String,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TimerCanceledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct TimerCanceledEventAttributes {
    pub timer_id: String,
    pub started_event_id: i64,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#StartChildWorkflowExecutionInitiatedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct StartChildWorkflowExecutionInitiatedEventAttributes {
    pub workflow_id: String,
    pub workflow_type: WorkflowType,
    pub control: Option<String>,
    pub input: Option<String>,
    pub execution_start_to_close_timeout: Option<String>,
    pub task_list: TaskList,
    pub task_priority: Option<String>,
    pub decision_task_completed_event_id: i64,
    pub child_policy: String,
    pub task_start_to_close_timeout: Option<String>,
    pub tag_list: Option<Vec<String>>,
    pub lambda_role: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionStartedEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub initiated_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionCompletedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionCompletedEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub result: Option<String>,
    pub initiated_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionFailedEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub reason: Option<String>,
    pub details: Option<String>,
    pub initiated_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionTimedOutEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionTimedOutEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub timeout_type: String,
    pub initiated_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionCanceledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionCanceledEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub details: Option<String>,
    pub initiated_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ChildWorkflowExecutionTerminatedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ChildWorkflowExecutionTerminatedEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub initiated_event_id: i64,
    pub started_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#SignalExternalWorkflowExecutionInitiatedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct SignalExternalWorkflowExecutionInitiatedEventAttributes {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub signal_name: String,
    pub input: Option<String>,
    pub decision_task_completed_event_id: i64,
    pub control: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ExternalWorkflowExecutionSignaledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ExternalWorkflowExecutionSignaledEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub initiated_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#SignalExternalWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct SignalExternalWorkflowExecutionFailedEventAttributes {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub cause: String,
    pub initiated_event_id: i64,
    pub decision_task_completed_event_id: i64,
    pub control: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ExternalWorkflowExecutionCancelRequestedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ExternalWorkflowExecutionCancelRequestedEventAttributes {
    pub workflow_execution: WorkflowExecution,
    pub initiated_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#RequestCancelExternalWorkflowExecutionInitiatedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct RequestCancelExternalWorkflowExecutionInitiatedEventAttributes {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub decision_task_completed_event_id: i64,
    pub control: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#RequestCancelExternalWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct RequestCancelExternalWorkflowExecutionFailedEventAttributes {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub cause: String,
    pub initiated_event_id: i64,
    pub decision_task_completed_event_id: i64,
    pub control: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ScheduleActivityTaskFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ScheduleActivityTaskFailedEventAttributes {
    pub activity_type: ActivityType,
    pub activity_id: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#RequestCancelActivityTaskFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct RequestCancelActivityTaskFailedEventAttributes {
    pub activity_id: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#StartTimerFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct StartTimerFailedEventAttributes {
    pub timer_id: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#CancelTimerFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct CancelTimerFailedEventAttributes {
    pub timer_id: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#StartChildWorkflowExecutionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct StartChildWorkflowExecutionFailedEventAttributes {
    pub workflow_type: WorkflowType,
    pub cause: String,
    pub workflow_id: String,
    pub initiated_event_id: i64,
    pub decision_task_completed_event_id: i64,
    pub control: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LambdaFunctionScheduledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct LambdaFunctionScheduledEventAttributes {
    pub id: String,
    pub name: String,
    pub control: Option<String>,
    pub input: Option<String>,
    pub start_to_close_timeout: Option<String>,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LambdaFunctionStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct LambdaFunctionStartedEventAttributes {
    pub scheduled_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LambdaFunctionCompletedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct LambdaFunctionCompletedEventAttributes {
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub result: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LambdaFunctionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct LambdaFunctionFailedEventAttributes {
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub reason: Option<String>,
    pub details: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LambdaFunctionTimedOutEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct LambdaFunctionTimedOutEventAttributes {
    pub scheduled_event_id: i64,
    pub started_event_id: i64,
    pub timeout_type: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#ScheduleLambdaFunctionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ScheduleLambdaFunctionFailedEventAttributes {
    pub id: String,
    pub name: String,
    pub cause: String,
    pub decision_task_completed_event_id: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#StartLambdaFunctionFailedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct StartLambdaFunctionFailedEventAttributes {
    pub scheduled_event_id: i64,
    pub cause: Option<String>,
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionInfos")]
#[derive(Debug, Clone, Default)]
pub struct ListClosedWorkflowExecutionsOutput {
    pub execution_infos: Vec<WorkflowExecutionInfo>,
    pub next_page_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#LimitExceededFault")]
#[derive(Debug, Clone, Default)]
pub struct LimitExceededFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TypeAlreadyExistsFault")]
#[derive(Debug, Clone, Default)]
pub struct TypeAlreadyExistsFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DomainAlreadyExistsFault")]
#[derive(Debug, Clone, Default)]
pub struct DomainAlreadyExistsFault {
    pub message: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#TooManyTagsFault")]
#[derive(Debug, Clone, Default)]
pub struct TooManyTagsFault {
    pub message: Option<String>,
}

/// The values `ActivityTaskTimeoutType` accepts on the wire.
pub const ACTIVITY_TASK_TIMEOUT_TYPE_VALUES: &[&str] = &["HEARTBEAT", "SCHEDULE_TO_CLOSE", "SCHEDULE_TO_START", "START_TO_CLOSE"];


/// The values `CancelTimerFailedCause` accepts on the wire.
pub const CANCEL_TIMER_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "TIMER_ID_UNKNOWN"];


/// The values `CancelWorkflowExecutionFailedCause` accepts on the wire.
pub const CANCEL_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "UNHANDLED_DECISION"];


/// The values `ChildPolicy` accepts on the wire.
pub const CHILD_POLICY_VALUES: &[&str] = &["ABANDON", "REQUEST_CANCEL", "TERMINATE"];


/// The values `CloseStatus` accepts on the wire.
pub const CLOSE_STATUS_VALUES: &[&str] = &["CANCELED", "COMPLETED", "CONTINUED_AS_NEW", "FAILED", "TERMINATED", "TIMED_OUT"];


/// The values `CompleteWorkflowExecutionFailedCause` accepts on the wire.
pub const COMPLETE_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "UNHANDLED_DECISION"];


/// The values `ContinueAsNewWorkflowExecutionFailedCause` accepts on the wire.
pub const CONTINUE_AS_NEW_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["CONTINUE_AS_NEW_WORKFLOW_EXECUTION_RATE_EXCEEDED", "DEFAULT_CHILD_POLICY_UNDEFINED", "DEFAULT_EXECUTION_START_TO_CLOSE_TIMEOUT_UNDEFINED", "DEFAULT_TASK_LIST_UNDEFINED", "DEFAULT_TASK_START_TO_CLOSE_TIMEOUT_UNDEFINED", "OPERATION_NOT_PERMITTED", "UNHANDLED_DECISION", "WORKFLOW_TYPE_DEPRECATED", "WORKFLOW_TYPE_DOES_NOT_EXIST"];


/// The values `DecisionTaskTimeoutType` accepts on the wire.
pub const DECISION_TASK_TIMEOUT_TYPE_VALUES: &[&str] = &["SCHEDULE_TO_START", "START_TO_CLOSE"];


/// The values `DecisionType` accepts on the wire.
pub const DECISION_TYPE_VALUES: &[&str] = &["CancelTimer", "CancelWorkflowExecution", "CompleteWorkflowExecution", "ContinueAsNewWorkflowExecution", "FailWorkflowExecution", "RecordMarker", "RequestCancelActivityTask", "RequestCancelExternalWorkflowExecution", "ScheduleActivityTask", "ScheduleLambdaFunction", "SignalExternalWorkflowExecution", "StartChildWorkflowExecution", "StartTimer"];


/// The values `EventType` accepts on the wire.
pub const EVENT_TYPE_VALUES: &[&str] = &["ActivityTaskCancelRequested", "ActivityTaskCanceled", "ActivityTaskCompleted", "ActivityTaskFailed", "ActivityTaskScheduled", "ActivityTaskStarted", "ActivityTaskTimedOut", "CancelTimerFailed", "CancelWorkflowExecutionFailed", "ChildWorkflowExecutionCanceled", "ChildWorkflowExecutionCompleted", "ChildWorkflowExecutionFailed", "ChildWorkflowExecutionStarted", "ChildWorkflowExecutionTerminated", "ChildWorkflowExecutionTimedOut", "CompleteWorkflowExecutionFailed", "ContinueAsNewWorkflowExecutionFailed", "DecisionTaskCompleted", "DecisionTaskScheduled", "DecisionTaskStarted", "DecisionTaskTimedOut", "ExternalWorkflowExecutionCancelRequested", "ExternalWorkflowExecutionSignaled", "FailWorkflowExecutionFailed", "LambdaFunctionCompleted", "LambdaFunctionFailed", "LambdaFunctionScheduled", "LambdaFunctionStarted", "LambdaFunctionTimedOut", "MarkerRecorded", "RecordMarkerFailed", "RequestCancelActivityTaskFailed", "RequestCancelExternalWorkflowExecutionFailed", "RequestCancelExternalWorkflowExecutionInitiated", "ScheduleActivityTaskFailed", "ScheduleLambdaFunctionFailed", "SignalExternalWorkflowExecutionFailed", "SignalExternalWorkflowExecutionInitiated", "StartChildWorkflowExecutionFailed", "StartChildWorkflowExecutionInitiated", "StartLambdaFunctionFailed", "StartTimerFailed", "TimerCanceled", "TimerFired", "TimerStarted", "WorkflowExecutionCancelRequested", "WorkflowExecutionCanceled", "WorkflowExecutionCompleted", "WorkflowExecutionContinuedAsNew", "WorkflowExecutionFailed", "WorkflowExecutionSignaled", "WorkflowExecutionStarted", "WorkflowExecutionTerminated", "WorkflowExecutionTimedOut"];


/// The values `ExecutionStatus` accepts on the wire.
pub const EXECUTION_STATUS_VALUES: &[&str] = &["CLOSED", "OPEN"];


/// The values `FailWorkflowExecutionFailedCause` accepts on the wire.
pub const FAIL_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "UNHANDLED_DECISION"];


/// The values `LambdaFunctionTimeoutType` accepts on the wire.
pub const LAMBDA_FUNCTION_TIMEOUT_TYPE_VALUES: &[&str] = &["START_TO_CLOSE"];


/// The values `RecordMarkerFailedCause` accepts on the wire.
pub const RECORD_MARKER_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED"];


/// The values `RegistrationStatus` accepts on the wire.
pub const REGISTRATION_STATUS_VALUES: &[&str] = &["DEPRECATED", "REGISTERED"];


/// The values `RequestCancelActivityTaskFailedCause` accepts on the wire.
pub const REQUEST_CANCEL_ACTIVITY_TASK_FAILED_CAUSE_VALUES: &[&str] = &["ACTIVITY_ID_UNKNOWN", "OPERATION_NOT_PERMITTED"];


/// The values `RequestCancelExternalWorkflowExecutionFailedCause` accepts on the wire.
pub const REQUEST_CANCEL_EXTERNAL_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "REQUEST_CANCEL_EXTERNAL_WORKFLOW_EXECUTION_RATE_EXCEEDED", "UNKNOWN_EXTERNAL_WORKFLOW_EXECUTION"];


/// The values `ScheduleActivityTaskFailedCause` accepts on the wire.
pub const SCHEDULE_ACTIVITY_TASK_FAILED_CAUSE_VALUES: &[&str] = &["ACTIVITY_CREATION_RATE_EXCEEDED", "ACTIVITY_ID_ALREADY_IN_USE", "ACTIVITY_TYPE_DEPRECATED", "ACTIVITY_TYPE_DOES_NOT_EXIST", "DEFAULT_HEARTBEAT_TIMEOUT_UNDEFINED", "DEFAULT_SCHEDULE_TO_CLOSE_TIMEOUT_UNDEFINED", "DEFAULT_SCHEDULE_TO_START_TIMEOUT_UNDEFINED", "DEFAULT_START_TO_CLOSE_TIMEOUT_UNDEFINED", "DEFAULT_TASK_LIST_UNDEFINED", "OPEN_ACTIVITIES_LIMIT_EXCEEDED", "OPERATION_NOT_PERMITTED"];


/// The values `ScheduleLambdaFunctionFailedCause` accepts on the wire.
pub const SCHEDULE_LAMBDA_FUNCTION_FAILED_CAUSE_VALUES: &[&str] = &["ID_ALREADY_IN_USE", "LAMBDA_FUNCTION_CREATION_RATE_EXCEEDED", "LAMBDA_SERVICE_NOT_AVAILABLE_IN_REGION", "OPEN_LAMBDA_FUNCTIONS_LIMIT_EXCEEDED"];


/// The values `SignalExternalWorkflowExecutionFailedCause` accepts on the wire.
pub const SIGNAL_EXTERNAL_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["OPERATION_NOT_PERMITTED", "SIGNAL_EXTERNAL_WORKFLOW_EXECUTION_RATE_EXCEEDED", "UNKNOWN_EXTERNAL_WORKFLOW_EXECUTION"];


/// The values `StartChildWorkflowExecutionFailedCause` accepts on the wire.
pub const START_CHILD_WORKFLOW_EXECUTION_FAILED_CAUSE_VALUES: &[&str] = &["CHILD_CREATION_RATE_EXCEEDED", "DEFAULT_CHILD_POLICY_UNDEFINED", "DEFAULT_EXECUTION_START_TO_CLOSE_TIMEOUT_UNDEFINED", "DEFAULT_TASK_LIST_UNDEFINED", "DEFAULT_TASK_START_TO_CLOSE_TIMEOUT_UNDEFINED", "OPEN_CHILDREN_LIMIT_EXCEEDED", "OPEN_WORKFLOWS_LIMIT_EXCEEDED", "OPERATION_NOT_PERMITTED", "WORKFLOW_ALREADY_RUNNING", "WORKFLOW_TYPE_DEPRECATED", "WORKFLOW_TYPE_DOES_NOT_EXIST"];


/// The values `StartLambdaFunctionFailedCause` accepts on the wire.
pub const START_LAMBDA_FUNCTION_FAILED_CAUSE_VALUES: &[&str] = &["ASSUME_ROLE_FAILED"];


/// The values `StartTimerFailedCause` accepts on the wire.
pub const START_TIMER_FAILED_CAUSE_VALUES: &[&str] = &["OPEN_TIMERS_LIMIT_EXCEEDED", "OPERATION_NOT_PERMITTED", "TIMER_CREATION_RATE_EXCEEDED", "TIMER_ID_ALREADY_IN_USE"];


/// The values `WorkflowExecutionCancelRequestedCause` accepts on the wire.
pub const WORKFLOW_EXECUTION_CANCEL_REQUESTED_CAUSE_VALUES: &[&str] = &["CHILD_POLICY_APPLIED"];


/// The values `WorkflowExecutionTerminatedCause` accepts on the wire.
pub const WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES: &[&str] = &["CHILD_POLICY_APPLIED", "EVENT_LIMIT_EXCEEDED", "OPERATOR_INITIATED"];


/// The values `WorkflowExecutionTimeoutType` accepts on the wire.
pub const WORKFLOW_EXECUTION_TIMEOUT_TYPE_VALUES: &[&str] = &["START_TO_CLOSE"];
