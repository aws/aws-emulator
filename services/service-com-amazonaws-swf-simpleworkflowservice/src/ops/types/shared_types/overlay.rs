// SPDX-License-Identifier: CC0-1.0

//! The shapes more than one operation reaches that this service answers with members the
//! published model snapshot does not declare.
//!
//! The snapshot in this crate's `smithy-model/` is kept EXACTLY as the upstream update
//! wrote it, so these five shapes are declared HERE rather than in [`super::snapshot`] —
//! generated, like every modelled shape in this crate, from the small overlay model
//! `smithy-model/local-additions.json`, which restates each of them with the extra members
//! beside the ones the snapshot declares, every modelled member against the snapshot's own
//! target, so each `@required` marking, each default and each length bound is captured
//! exactly as the snapshot spells it (see `smithy-model/LOCAL-ADDITIONS.md`). Nothing
//! about the wire is hand-written: the member names, the shape ids and the types all come
//! from a model file.
//!
//! The two model files are named from two sibling modules rather than from one file
//! because `smithy_macros::smithy_model!` defines the local `smithy!` macro that
//! `#[smithy]` expands into, and that definition covers the whole file it appears in, so a
//! single file naming two models cannot resolve the attribute.
//!
//! What each member CARRIES is not decided here: the hand-over handle and the
//! continuation flag come from the stored history event ([`crate::state`]), the close id
//! and instant from the event that closed the run, and the tag's discriminator from the
//! operation that reports it.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The record every reader of a stored run reports: what the published model declares,
/// plus the close the run's own history records and the reopening members such a record
/// carries.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionInfo")]
#[derive(Debug, Clone)]
pub struct WorkflowExecutionInfo {
    pub execution: WorkflowExecution,
    pub workflow_type: WorkflowType,
    pub start_timestamp: ::aws_smithy_types::DateTime,
    pub close_timestamp: Option<::aws_smithy_types::DateTime>,
    pub execution_status: String,
    pub close_status: Option<String>,
    pub parent: Option<WorkflowExecution>,
    pub tag_list: Option<Vec<String>>,
    pub cancel_requested: bool,
    pub last_closed_event_id: i64,
    pub last_closed_timestamp: Option<::aws_smithy_types::DateTime>,
    pub reopen_generation_id: i64,
    pub reopenable: bool,
}

/// A key-value pair associated with a domain, as the service reports and accepts it: the
/// stored key and value, plus the awsJson type discriminator naming the shape the element
/// IS, which a response carries on every element.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#ResourceTag")]
#[derive(Debug, Clone, Default)]
pub struct ResourceTag {
    pub key: String,
    pub value: Option<String>,
    pub __type: Option<String>,
}

/// The details of an `ActivityTaskScheduled` event, plus whether the service started the
/// task straight away rather than leaving it in its task list for a worker to poll for.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskScheduledEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskScheduledEventAttributes {
    pub activity_type: ActivityType,
    pub activity_id: String,
    pub input: Option<String>,
    pub control: Option<String>,
    pub schedule_to_start_timeout: Option<String>,
    pub schedule_to_close_timeout: Option<String>,
    pub start_to_close_timeout: Option<String>,
    pub task_list: TaskList,
    pub task_priority: Option<String>,
    pub decision_task_completed_event_id: i64,
    pub heartbeat_timeout: Option<String>,
    pub immediately_start: bool,
}

/// The details of an `ActivityTaskStarted` event, plus the opaque, service-generated
/// handle on THAT hand-over of the task to a worker.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#ActivityTaskStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct ActivityTaskStartedEventAttributes {
    pub identity: Option<String>,
    pub scheduled_event_id: i64,
    pub client_token: Option<String>,
}

/// The details of a `DecisionTaskStarted` event, plus the opaque, service-generated handle
/// on THAT hand-over of the task to a decider and whether the task was started as a
/// continuation of the previous task's completion rather than in answer to a poll.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#DecisionTaskStartedEventAttributes")]
#[derive(Debug, Clone, Default)]
pub struct DecisionTaskStartedEventAttributes {
    pub identity: Option<String>,
    pub scheduled_event_id: i64,
    pub client_token: Option<String>,
    pub started_on_previous_task_completion: bool,
}
