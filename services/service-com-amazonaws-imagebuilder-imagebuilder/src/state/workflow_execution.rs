// SPDX-License-Identifier: CC0-1.0

//! The workflow RUNS of an image build: what the build recorded about each one, and what a
//! read of one reports.
//!
//! `GetWorkflowExecution` "retrieves runtime information for a specific runtime instance of
//! the workflow", named by a `workflowExecutionId` Image Builder assigned "to keep track of
//! runtime details when it ran the workflow". A runtime instance exists because a BUILD ran a
//! workflow, so it is a fact of that build's run: one instance per workflow configuration the
//! build was configured with ([`super::ImageBuildRun::workflows`]), recorded where the build
//! is queued and held ON the build version record it belongs to — so what a read of an
//! instance reports can never disagree with the build that ran it, and a build version this
//! store forgets takes its runtime instances with it.
//!
//! What each instance HOLDS is what the run established and nothing else:
//!
//! * the id Image Builder assigned it ([`new_workflow_execution_id`]), which is what a read
//!   names it by. The VALUE is per-run and is not a requirement — nothing can predict one —
//!   so it is minted the way every other id of this service is ([`super::new_uuid`]) behind
//!   the prefix the modelled `WorkflowExecutionId` pattern requires;
//! * the workflow BUILD VERSION that ran, which is "the Amazon Resource Name (ARN) of the
//!   build version for the Image Builder workflow resource that defines the specified runtime
//!   instance of the workflow": a configuration may name a workflow by a version ARN whose
//!   trailing nodes are wildcards, and what RAN was one build version, so the reference is
//!   resolved when the run starts, exactly as the base image the build runs against is
//!   ([`super::ImageBuildRun::resolved_parent_image`]);
//! * the TYPE of that workflow, "the type of workflow that Image Builder ran", which is the
//!   stage its ARN carries ([`super::WorkflowReference`]) — a property of the workflow
//!   resource rather than of the run;
//! * the PARALLEL GROUP the instance ran in, "if configured", which is what the configuration
//!   that ran it named;
//! * how many STEPS the workflow document defines, which is "the total number of steps that
//!   the workflow document defines for this runtime instance of the workflow. Image Builder
//!   sets this count before any steps run" — read from the document this store holds for that
//!   workflow ([`super::workflow_document_step_count`]). A workflow AWS itself owns is not a
//!   resource of this store and this emulator holds no document for one, so nothing is
//!   claimed about the steps of its document.
//!
//! Everything a read reports that the CLOCK decides is derived from the build the instance
//! belongs to rather than stored: this emulator runs a build as ONE span
//! ([`super::IMAGE_BUILD_SPAN`]), so the workflows of that build run for exactly that span —
//! they start when the build was queued and reach their terminal status when the build reaches
//! its own ([`WorkflowExecutionRecord::outcome_at`]). Nothing is written when an instance is
//! read.

use std::time::SystemTime;

use crate::ops::types::{
    GetWorkflowExecutionOutput, WorkflowConfiguration, WorkflowExecutionMetadata,
};

use super::{
    iso8601, new_uuid, text, workflow_document_step_count,
    ComAmazonawsImagebuilderImagebuilderState, ImageBuildVersionRecord, WorkflowReference,
    IMAGE_STATUS_AVAILABLE, IMAGE_STATUS_CANCELLED, IMAGE_STATUS_FAILED,
};

/// The operation a page token of the runtime-instance listing is issued under. See
/// [`super::LIST_WAITING_WORKFLOW_STEPS_OPERATION`].
pub const LIST_WORKFLOW_EXECUTIONS_OPERATION: &str = "ListWorkflowExecutions";

/// A deterministic rendering of which runtime instances one `ListWorkflowExecutions` request
/// asked for, so a token issued for one image build version is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`super::image_recipe_query_fingerprint`] states.
#[must_use]
pub fn workflow_execution_query_fingerprint(image_build_version_arn: &str) -> String {
    format!(
        "imageBuildVersionArn={};",
        text(Some(image_build_version_arn))
    )
}

/// The prefix the modelled `WorkflowExecutionId` requires of every runtime instance id
/// (`smithy_query(query="shape",shape="WorkflowExecutionId")`: `^wf-` followed by the
/// 8-4-4-4-12 hex form this API's ids take).
pub const WORKFLOW_EXECUTION_ID_PREFIX: &str = "wf-";

/// The value of the modelled `WorkflowExecutionStatus` enum a runtime instance that is still
/// running carries (`smithy_query(query="shape",shape="WorkflowExecutionStatus")`).
pub const WORKFLOW_EXECUTION_STATUS_RUNNING: &str = "RUNNING";

/// The value that enum carries for an instance that finished its work: one of the terminal
/// states its documentation names.
pub const WORKFLOW_EXECUTION_STATUS_COMPLETED: &str = "COMPLETED";

/// The value that enum carries for an instance whose build was stopped.
pub const WORKFLOW_EXECUTION_STATUS_CANCELLED: &str = "CANCELLED";

/// The value that enum carries for an instance whose build ended in failure.
pub const WORKFLOW_EXECUTION_STATUS_FAILED: &str = "FAILED";

/// The kind of resource a report of a runtime instance this store holds nothing for names
/// ([`super::missing_resource_message`]).
///
/// A `workflowExecutionId` is not an ARN, so it carries no resource type of its own to read
/// one off ([`super::arn_resource_type`]); the kind is this model's own name for the resource
/// the id identifies — the runtime instance of a workflow, whose metadata this model carries
/// as `WorkflowExecutionMetadata`.
pub const WORKFLOW_EXECUTION_RESOURCE_TYPE: &str = "WorkflowExecution";

/// Mint the id Image Builder assigns one runtime instance of a workflow.
///
/// The one mint of this crate ([`super::new_uuid`]) behind the prefix the modelled
/// `WorkflowExecutionId` requires, so an id names exactly one instance for the same reason
/// every other id of this service is unique.
#[must_use]
pub fn new_workflow_execution_id() -> String {
    format!("{WORKFLOW_EXECUTION_ID_PREFIX}{}", new_uuid())
}

/// ONE runtime instance of a workflow an image build ran, as the build recorded it.
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionRecord {
    /// The id Image Builder assigned this instance ([`new_workflow_execution_id`]), which is
    /// what a read names it by.
    pub workflow_execution_id: String,
    /// The workflow build version that ran: the ONE build version the configuration's
    /// reference reached when the run started, and the reference itself when it reached no
    /// workflow of this store — which is the case for a workflow AWS itself owns.
    pub workflow_build_version_arn: String,
    /// The stage of that workflow, as one of the modelled `WorkflowType` values, when the
    /// workflow it ran names one.
    pub workflow_type: Option<String>,
    /// The parallel group the configuration that ran it named, when it named one.
    pub parallel_group: Option<String>,
    /// How many steps the workflow document defines, which "Image Builder sets before any
    /// steps run": the count read off the document this store holds for that workflow, and
    /// zero for a workflow it holds no document for.
    pub total_step_count: i32,
}

/// What a runtime instance's run has come to at one instant: the status it reports, and the
/// instant it reached a terminal one, when it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowRunOutcome {
    /// One of the modelled `WorkflowExecutionStatus` values.
    pub status: &'static str,
    /// The instant the run finished, for a run that has.
    pub ended_at: Option<SystemTime>,
}

impl WorkflowExecutionRecord {
    /// What this instance's run has come to at the instant `at`, for the `build` that ran it.
    ///
    /// The workflows of a build run FOR that build: "a new image moves through build, test,
    /// and distribution statuses during creation, and ends in the AVAILABLE, FAILED, or
    /// CANCELLED state" (the modelled `ImageState`), and this emulator runs the whole build as
    /// one span ([`super::IMAGE_BUILD_SPAN`]) — so a build that is still outstanding is one
    /// whose workflows are running, and a build that has ended reports its workflows in the
    /// terminal state it ended in, at the instant it reached it. Read from the CLOCK against
    /// the build's own instants ([`ImageBuildVersionRecord::status_at`]), so nothing has to be
    /// materialised by a call.
    #[must_use]
    pub fn outcome_at(build: &ImageBuildVersionRecord, at: SystemTime) -> WorkflowRunOutcome {
        match build.status_at(at) {
            IMAGE_STATUS_AVAILABLE => WorkflowRunOutcome {
                status: WORKFLOW_EXECUTION_STATUS_COMPLETED,
                ended_at: build.available_at,
            },
            IMAGE_STATUS_CANCELLED => WorkflowRunOutcome {
                status: WORKFLOW_EXECUTION_STATUS_CANCELLED,
                ended_at: build.cancelled_at,
            },
            IMAGE_STATUS_FAILED => WorkflowRunOutcome {
                status: WORKFLOW_EXECUTION_STATUS_FAILED,
                ended_at: None,
            },
            _ => WorkflowRunOutcome {
                status: WORKFLOW_EXECUTION_STATUS_RUNNING,
                ended_at: None,
            },
        }
    }

    /// This instance as the answer a read of it reports, for the `build` that ran it, read at
    /// the instant `at`.
    ///
    /// The step COUNTS follow the status the run reports, because the model ties them
    /// together: `totalStepCount` is what the document defines, and "the sum of succeeded,
    /// skipped, and failed steps only reaches this total if every step finishes in one of
    /// those states" — so a run this emulator reports as COMPLETED has every step of its
    /// document succeeded, and a run that has not finished reports no step in a final state
    /// yet. Nothing here skips or fails a step: this emulator reports no step outcome it did
    /// not derive from the status of the build that ran the workflow.
    #[must_use]
    pub fn view(
        &self,
        build: &ImageBuildVersionRecord,
        at: SystemTime,
    ) -> GetWorkflowExecutionOutput {
        let outcome = Self::outcome_at(build, at);
        let completed = outcome.status == WORKFLOW_EXECUTION_STATUS_COMPLETED;
        GetWorkflowExecutionOutput {
            // The request this answer names is reported by the operation that answers it,
            // which is where this service mints a request id.
            request_id: None,
            workflow_execution_id: Some(self.workflow_execution_id.clone()),
            workflow_build_version_arn: Some(self.workflow_build_version_arn.clone()),
            // "The Amazon Resource Name (ARN) of the image build version that owns the
            // specified runtime instance of the workflow", which is the build this instance is
            // recorded on.
            image_build_version_arn: Some(build.arn.clone()),
            r#type: self.workflow_type.clone(),
            status: Some(outcome.status.to_string()),
            // "The output message from the specified runtime instance of the workflow, if
            // applicable": nothing of this run produced a message.
            message: None,
            total_step_count: self.total_step_count,
            total_steps_succeeded: if completed { self.total_step_count } else { 0 },
            // No step of a run is failed or skipped by anything here.
            total_steps_failed: 0,
            total_steps_skipped: 0,
            // "The timestamp when the specified runtime instance of the workflow started":
            // the workflows of a build start with the build, which is the instant the build
            // version records as its own.
            start_time: Some(build.date_created.clone()),
            end_time: outcome.ended_at.map(iso8601),
            parallel_group: self.parallel_group.clone(),
        }
    }

    /// This instance as the modelled `WorkflowExecutionMetadata` one entry of the listing of a
    /// build's runtime instances carries, read at the instant `at`.
    ///
    /// Every member it shares with a read of the instance is the same answer, read the same
    /// way ([`Self::view`]), so a listed entry and a read of the entry it names cannot
    /// disagree.
    #[must_use]
    pub fn metadata(
        &self,
        build: &ImageBuildVersionRecord,
        at: SystemTime,
    ) -> WorkflowExecutionMetadata {
        let read = self.view(build, at);
        WorkflowExecutionMetadata {
            workflow_build_version_arn: read.workflow_build_version_arn,
            workflow_execution_id: read.workflow_execution_id,
            r#type: read.r#type,
            status: read.status,
            message: read.message,
            total_step_count: read.total_step_count,
            total_steps_succeeded: read.total_steps_succeeded,
            total_steps_failed: read.total_steps_failed,
            total_steps_skipped: read.total_steps_skipped,
            start_time: read.start_time,
            end_time: read.end_time,
            parallel_group: read.parallel_group,
            // "Indicates whether a retry of the image build superseded this runtime instance
            // of the workflow. When you retry a failed image build, Image Builder sets this
            // flag to true on the original workflow executions that the retry re-ran": a
            // retry of this emulator's build re-runs the SAME build version in place
            // (`RetryImage`), so no runtime instance of it has been superseded by another.
            retried: None,
        }
    }
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The runtime instance one workflow CONFIGURATION of a build starts, in `region`.
    ///
    /// Called where a build is queued, once per configuration the build runs
    /// ([`super::ImageBuildRun::observed`]), so every fact of the run is observed at the
    /// instant the run starts: the reference the configuration carries is resolved to the one
    /// workflow build version it reaches now, and the document of that workflow is what the
    /// step count is read from.
    #[must_use]
    pub fn workflow_run(
        &self,
        configuration: &WorkflowConfiguration,
        region: &str,
    ) -> WorkflowExecutionRecord {
        let resolved = self.resolve_workflow(&configuration.workflow_arn, region);
        let workflow_build_version_arn = resolved
            .map_or_else(|| configuration.workflow_arn.clone(), |record| record.arn.clone());
        WorkflowExecutionRecord {
            workflow_execution_id: new_workflow_execution_id(),
            workflow_type: resolved.map(|record| record.workflow_type.clone()).or_else(|| {
                WorkflowReference::parse(&workflow_build_version_arn)
                    .map(|reference| reference.stage.to_ascii_uppercase())
            }),
            total_step_count: resolved
                .and_then(|record| record.data.as_deref())
                .map_or(0, workflow_document_step_count),
            workflow_build_version_arn,
            // "The name of the parallel group that this runtime instance of the workflow ran
            // in, if configured", which is what the configuration that ran it named.
            parallel_group: configuration.parallel_group.clone(),
        }
    }

    /// The runtime instance `workflow_execution_id` names, together with the image build
    /// version that ran it, when a build of `request_region` recorded one.
    ///
    /// A runtime instance id is not an ARN, so it names no Region of its own: an instance
    /// belongs to the build that ran it, and a regional endpoint reaches only the builds of
    /// its own Region ([`Self::image`]) — so an id minted by a build of another Region reaches
    /// nothing here.
    #[must_use]
    pub fn workflow_execution(
        &self,
        workflow_execution_id: &str,
        request_region: &str,
    ) -> Option<(&ImageBuildVersionRecord, &WorkflowExecutionRecord)> {
        self.images
            .values()
            .filter(|build| build.region == request_region)
            .find_map(|build| {
                build
                    .workflow_executions
                    .iter()
                    .find(|execution| execution.workflow_execution_id == workflow_execution_id)
                    .map(|execution| (build, execution))
            })
    }

    /// The answer a read of the runtime instance `workflow_execution_id` names reports at the
    /// instant `at`, or `None` when no build of `request_region` recorded that instance.
    ///
    /// A pure read: the instance and the build that ran it are copied out as the modelled
    /// answer and nothing is written.
    #[must_use]
    pub fn workflow_execution_answer(
        &self,
        workflow_execution_id: &str,
        request_region: &str,
        at: SystemTime,
    ) -> Option<GetWorkflowExecutionOutput> {
        let (build, execution) = self.workflow_execution(workflow_execution_id, request_region)?;
        Some(execution.view(build, at))
    }
}
