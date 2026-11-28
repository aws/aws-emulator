// SPDX-License-Identifier: CC0-1.0

//! The already-started fault, as AWS carries it on the wire: the message the published
//! snapshot declares, plus the identity of the open run that already holds the requested
//! `workflowId`, which the snapshot does not declare.
//!
//! The snapshot in this crate's `smithy-model/` is kept EXACTLY as the upstream update
//! wrote it, so this shape is declared HERE rather than in [`super::snapshot`] —
//! generated, like every modelled shape in this crate, from the small overlay model
//! `smithy-model/local-additions.json`, which restates it with that member beside the one
//! the snapshot declares, each against the snapshot's own target, and with the same error
//! class, so the code and the HTTP status are unchanged (see
//! `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is hand-written.
//!
//! What the member CARRIES is not decided here: the handler fills in the requested
//! `workflowId` and the `runId` its own open-run index holds for it.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The refusal of a start that collides with a run that is still open: the service's
/// message and the execution that holds the id.
#[smithy_macros::smithy(shape = "com.amazonaws.swf#WorkflowExecutionAlreadyStartedFault")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionAlreadyStartedFault {
    pub message: Option<String>,
    pub workflow_execution: Option<WorkflowExecution>,
}
