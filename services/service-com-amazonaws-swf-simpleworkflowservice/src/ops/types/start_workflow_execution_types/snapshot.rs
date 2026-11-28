// SPDX-License-Identifier: CC0-1.0

//! The shapes `start_workflow_execution` reaches, as the published model declares them.
//!
//! Every shape the operation reaches except the already-started fault, which AWS answers
//! with the identity of the run that holds the id and which is therefore declared in
//! [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.swf#StartWorkflowExecutionInput")]
#[derive(Debug, Clone, Default)]
pub struct StartWorkflowExecutionInput {
    pub domain: String,
    pub workflow_id: String,
    pub workflow_type: WorkflowType,
    pub task_list: Option<TaskList>,
    pub task_priority: Option<String>,
    pub input: Option<String>,
    pub execution_start_to_close_timeout: Option<String>,
    pub tag_list: Option<Vec<String>>,
    pub task_start_to_close_timeout: Option<String>,
    pub child_policy: Option<String>,
    pub lambda_role: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#Run")]
#[derive(Debug, Clone, Default)]
pub struct StartWorkflowExecutionOutput {
    pub run_id: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.swf#DefaultUndefinedFault")]
#[derive(Debug, Clone, Default)]
pub struct DefaultUndefinedFault {
    pub message: Option<String>,
}
