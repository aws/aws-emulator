// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emr-elasticmapreduce service.
//!
//! The job-flow (cluster) lifecycle lives here: `RunJobFlow` inserts a
//! [`ClusterRecord`] built from the request, every later request resolves that record
//! out of the single shared engine, and the EMR wire shapes
//! (`Cluster`/`ClusterSummary`/`Step`/`StepSummary`/`InstanceGroup`) are PROJECTIONS of
//! it — see [`ClusterRecord::to_cluster`], [`ClusterRecord::to_summary`],
//! [`StepRecord::to_step`], [`StepRecord::to_summary`] and
//! [`InstanceGroupRecord::to_instance_group`]. No handler builds one of
//! those shapes from its own input.
//!
//! Beside the clusters sits the account's single block public access configuration
//! ([`BlockPublicAccessRecord`]), which always exists: it starts as the documented
//! account default, `PutBlockPublicAccessConfiguration` replaces it whole, and
//! `GetBlockPublicAccessConfiguration` reports it.
//!
//! The one concern split out of this root is the instance-type catalog
//! ([`instance_catalog`]), a fact about the service rather than account state, and the
//! notebook-execution lifecycle ([`notebook_executions`]); the names of both are
//! re-exported here, so every caller keeps reaching them through `crate::state`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Application, AutoScalingPolicy, AutoScalingPolicyDescription, AutoScalingPolicyStatus,
    AutoTerminationPolicy, BlockPublicAccessConfiguration, BlockPublicAccessConfigurationMetadata,
    BootstrapActionConfig,
    Cluster, ClusterStateChangeReason, ClusterStatus, ClusterSummary, ClusterTimeline,
    Configuration, EbsBlockDevice, EbsConfiguration, Ec2InstanceAttributes, FailureDetails,
    HadoopStepConfig, InstanceFleet, InstanceFleetConfig, InstanceFleetStateChangeReason,
    InstanceFleetStatus, InstanceFleetTimeline, InstanceGroup, InstanceGroupConfig,
    InstanceFleetModifyConfig,
    InstanceGroupStateChangeReason, InstanceGroupStatus, InstanceGroupTimeline,
    InstanceTypeConfig, InstanceTypeSpecification, InvalidRequestException,
    JobFlowInstancesConfig, KerberosAttributes, ManagedScalingPolicy, MonitoringConfiguration,
    NotebookExecution, NotebookExecutionSummary, NotebookS3LocationForOutput, OSRelease,
    OutputNotebookS3LocationForOutput, PersistentAppUI, PlacementGroupConfig, PortRange, Session,
    SessionMappingDetail, ShrinkPolicy, SimplifiedApplication, StartNotebookExecutionInput, Step,
    StepConfig, StepStateChangeReason, StepStatus, StepSummary, StepTimeline, Studio,
    SupportedInstanceType, Tag,
};

/// The instance-type catalog, split out of this root as its own concern; its names are
/// re-exported so every caller keeps reaching them through `crate::state`.
mod instance_catalog;
pub use instance_catalog::*;

/// The cluster's step runner — what a submitted step's work answered — split out of this
/// root as its own concern; its names are re-exported so every caller keeps reaching them
/// through `crate::state`.
mod step_runner;
pub use step_runner::*;

/// Announcing a cluster's state changes on the account's default event bus, split out of
/// this root as its own concern and re-exported the same way.
mod cluster_events;
pub use cluster_events::*;

/// A cluster's master node — the address it holds and the DNS name the cluster reports
/// for it — split out of this root as its own concern and re-exported the same way.
mod master_node;
pub use master_node::*;

/// The Amazon EC2 instances a cluster runs on — launched in the service that OWNS
/// instances, and reported by `ListInstances` — split out of this root as its own concern
/// and re-exported the same way.
mod cluster_instances;
pub use cluster_instances::*;

/// How many instances ONE instance group may be asked for, the sibling of the instance
/// fleet's own ceiling; split out and re-exported the same way.
mod instance_group_capacity;
pub use instance_group_capacity::*;

/// The Amazon EC2 network and the IAM role an EMR Studio is wired to, which are resolved
/// in the services that own them; split out and re-exported the same way.
mod studio_network;
pub use studio_network::*;

/// The IAM Identity Center users and groups an EMR Studio session mapping maps, resolved
/// in the service that owns them; split out and re-exported the same way.
mod studio_identities;
pub use studio_identities::*;

/// The notebook-execution lifecycle, split out of this root as its own concern for the
/// same reason, and re-exported the same way.
mod notebook_executions;
pub use notebook_executions::*;

/// RUNNING a notebook execution's notebook — fetching it out of Amazon S3 and executing
/// its code cells — split out of the lifecycle above as its own concern and re-exported
/// the same way.
mod notebook_run;
pub use notebook_run::*;

/// The sentences every MODIFICATION refusal of this service is worded by, split out and
/// re-exported the same way.
mod modification_refusals;
pub use modification_refusals::*;

/// Modifying an instance fleet a cluster already carries — which capacities a fleet is
/// held to, and what a modification leaves on its record — split out and re-exported the
/// same way.
mod instance_fleet_modification;
pub use instance_fleet_modification::*;

/// The members Amazon EMR fills in for a launch that left them out — the effective
/// configuration every read of a cluster or of its instance fleets reports — split out
/// and re-exported the same way.
mod launch_defaults;
pub use launch_defaults::*;

/// What a cluster has COST so far — the normalized instance hours every read of it
/// derives — split out and re-exported the same way.
mod normalized_hours;
pub use normalized_hours::*;

/// WHERE a cluster runs — the Availability Zone its launch placed it in, resolved in the
/// service that owns the subnet — split out and re-exported the same way.
mod launch_placement;
pub use launch_placement::*;

/// The launch's own INSTANCE configuration — what a cluster reports back of the
/// `JobFlowInstancesConfig` it was created with — split out of this root as its own concern
/// and re-exported the same way. Its file sits in the directory the release-catalog split
/// created, which is what the `#[path]` names; the concern is this root's, not that
/// catalog's, so the module is a sibling of it.
#[path = "state/release_catalog/launch_placement.rs"]
mod launch_instances;
pub use launch_instances::*;

/// WHAT a step submits when what it submits is a Spark application — the reading of its
/// `spark-submit` command line — split out and re-exported the same way.
mod spark_application;
pub use spark_application::*;

/// The Spark runtime a submitted application finds on its import path, split out and
/// re-exported the same way.
mod spark_runtime;
pub use spark_runtime::*;

/// RUNNING the application a step submitted — fetching it out of Amazon S3, executing it and
/// reading what the step ends in off that run — split out and re-exported the same way.
mod spark_step_run;
pub use spark_step_run::*;

/// PERFORMING the command a step submitted — the command line its arguments are, and the
/// status it exits with, answered by a parse and a fixed allow-list of session-local
/// utilities with no host command, process or shell started — split out and re-exported the
/// same way.
mod step_command_status;
pub use step_command_status::*;

/// WHEN that answer is given, and what one call may spend giving it: the queue side of the
/// same concern, split out and re-exported the same way.
mod command_step_run;
pub use command_step_run::*;

/// The EventBridge events a step's state changes are published as, split out and
/// re-exported the same way.
mod step_events;
pub use step_events::*;

/// The Spark Connect session surface of a cluster — which clusters carry sessions, what
/// `StartSession` records and the states the clock walks one through — split out and
/// re-exported the same way.
mod sessions;
pub use sessions::*;

/// The SPARK release family of the release catalog — the labels an Amazon EMR Spark
/// release carries and what one bundles — split out and re-exported the same way.
mod spark_releases;
mod release_catalog;
mod step_records;
pub use step_records::*;
pub use release_catalog::*;
pub use spark_releases::*;

/// `com.amazonaws.emr#ClusterState`, as the wire carries it.
pub mod cluster_state {
    pub const STARTING: &str = "STARTING";
    pub const BOOTSTRAPPING: &str = "BOOTSTRAPPING";
    pub const RUNNING: &str = "RUNNING";
    pub const WAITING: &str = "WAITING";
    pub const TERMINATING: &str = "TERMINATING";
    pub const TERMINATED: &str = "TERMINATED";
    pub const TERMINATED_WITH_ERRORS: &str = "TERMINATED_WITH_ERRORS";

    /// The states `AddJobFlowSteps` accepts a step in: "You can only add steps to a
    /// cluster that is in one of the following states: STARTING, BOOTSTRAPPING,
    /// RUNNING, or WAITING." (`AddJobFlowSteps` doc, Smithy model.)
    pub const ACCEPTS_STEPS: &[&str] = &[STARTING, BOOTSTRAPPING, RUNNING, WAITING];
}

/// `com.amazonaws.emr#StepState`, as the wire carries it.
pub mod step_state {
    pub const PENDING: &str = "PENDING";
    pub const CANCEL_PENDING: &str = "CANCEL_PENDING";
    pub const RUNNING: &str = "RUNNING";
    pub const COMPLETED: &str = "COMPLETED";
    pub const CANCELLED: &str = "CANCELLED";
    pub const FAILED: &str = "FAILED";
    pub const INTERRUPTED: &str = "INTERRUPTED";
}

/// `com.amazonaws.emr#InstanceCollectionType`.
pub mod instance_collection_type {
    pub const INSTANCE_FLEET: &str = "INSTANCE_FLEET";
    pub const INSTANCE_GROUP: &str = "INSTANCE_GROUP";
}

/// `com.amazonaws.emr#InstanceFleetState`, for the states this emulator sets
/// (`smithy_query(query="shape", shape="InstanceFleetState")`).
pub mod instance_fleet_state {
    /// The fleet is provisioning its Amazon EC2 resources and is not ready to run jobs.
    pub const PROVISIONING: &str = "PROVISIONING";
    /// The fleet's Amazon EC2 instances and other resources are running.
    pub const RUNNING: &str = "RUNNING";
    /// The fleet is no longer active, because its cluster is gone.
    pub const TERMINATED: &str = "TERMINATED";
}

/// `com.amazonaws.emr#InstanceFleetStateChangeReasonCode`, for the codes this emulator
/// sets (`smithy_query(query="shape", shape="InstanceFleetStateChangeReasonCode")`).
pub mod instance_fleet_state_change_reason_code {
    pub const CLUSTER_TERMINATED: &str = "CLUSTER_TERMINATED";
}

/// `com.amazonaws.emr#InstanceGroupState`, for the states this emulator sets
/// (`smithy_query(query="shape", shape="InstanceGroupState")`).
pub mod instance_group_state {
    /// The group is acquiring its Amazon EC2 instances and is not up yet.
    pub const PROVISIONING: &str = "PROVISIONING";
    /// The group's requested instances are up.
    pub const RUNNING: &str = "RUNNING";
    /// The group is moving to the instance count a modification asked for.
    pub const RESIZING: &str = "RESIZING";
    /// The group is applying the configurations a modification asked for.
    pub const RECONFIGURING: &str = "RECONFIGURING";
    /// The group is gone because its cluster is.
    pub const ENDED: &str = "ENDED";

    /// The states a group's requested instances are up in, so a read reports them as
    /// running: the group is up, and a resize or a reconfiguration leaves the instances
    /// it already holds running while it is applied.
    pub const UP: &[&str] = &[RUNNING, RESIZING, RECONFIGURING];
}

/// `com.amazonaws.emr#ReconfigurationType`: "Type of reconfiguration requested. Valid
/// values are MERGE and OVERWRITE." (`InstanceGroupModifyConfig$ReconfigurationType`
/// doc.)
pub mod reconfiguration_type {
    /// The submitted configurations are merged into the ones the group holds.
    pub const MERGE: &str = "MERGE";
    /// The submitted configurations replace the ones the group holds.
    pub const OVERWRITE: &str = "OVERWRITE";
}

/// `com.amazonaws.emr#InstanceGroupStateChangeReasonCode`, for the codes this emulator
/// sets (`smithy_query(query="shape", shape="InstanceGroupStateChangeReasonCode")`).
pub mod instance_group_state_change_reason_code {
    pub const CLUSTER_TERMINATED: &str = "CLUSTER_TERMINATED";
}

/// `com.amazonaws.emr#AutoScalingPolicyState`, for the states this emulator sets
/// (`smithy_query(query="shape", shape="AutoScalingPolicyState")`).
pub mod auto_scaling_policy_state {
    /// The policy is being attached to a group that is not up yet.
    pub const ATTACHING: &str = "ATTACHING";
    /// The policy is attached to the group and in force.
    pub const ATTACHED: &str = "ATTACHED";
}

/// `com.amazonaws.emr#MarketType`: "Valid values are ON_DEMAND or SPOT."
/// (`InstanceGroup$Market` doc.)
pub mod market_type {
    pub const ON_DEMAND: &str = "ON_DEMAND";
    pub const SPOT: &str = "SPOT";
}

/// `com.amazonaws.emr#InstanceGroupType` / `InstanceRoleType`: "Valid values are
/// MASTER, CORE or TASK." (`InstanceGroup$InstanceGroupType` doc.)
pub mod instance_group_type {
    pub const MASTER: &str = "MASTER";
    pub const CORE: &str = "CORE";
    pub const TASK: &str = "TASK";

    /// The roles an automatic scaling policy may be attached to: "Creates or updates an
    /// automatic scaling policy for a core instance group or task instance group in an
    /// Amazon EMR cluster." (`PutAutoScalingPolicy` doc, Smithy model.) The master group
    /// runs the one primary node, so nothing scales it.
    pub const AUTO_SCALABLE: &[&str] = &[CORE, TASK];
}

/// `com.amazonaws.emr#CancelStepsRequestStatus`: "The value may be SUBMITTED or
/// FAILED." (`CancelStepsInfo$Status` doc.)
pub mod cancel_steps_request_status {
    pub const SUBMITTED: &str = "SUBMITTED";
    pub const FAILED: &str = "FAILED";
}

/// `com.amazonaws.emr#NotebookExecutionStatus`, for the statuses this emulator sets.
pub mod notebook_execution_status {
    /// "`START_PENDING` indicates that the cluster has received the execution request
    /// but execution has not begun."
    pub const START_PENDING: &str = "START_PENDING";
    /// "`STARTING` indicates that the execution is starting on the cluster."
    pub const STARTING: &str = "STARTING";
    /// "`RUNNING` indicates that the execution is being processed by the cluster."
    pub const RUNNING: &str = "RUNNING";
    /// "`FINISHING` indicates that execution processing is in the final stages."
    pub const FINISHING: &str = "FINISHING";
    /// "`FAILING` indicates that the execution is failing and will not finish
    /// successfully."
    pub const FAILING: &str = "FAILING";
    /// "`STOP_PENDING` indicates that the cluster has received a
    /// `StopNotebookExecution` request and the stop is pending."
    pub const STOP_PENDING: &str = "STOP_PENDING";
    /// "`STOPPING` indicates that the cluster is in the process of stopping the
    /// execution as a result of a `StopNotebookExecution` request."
    pub const STOPPING: &str = "STOPPING";
    /// "`STOPPED` indicates that the execution stopped because of a
    /// `StopNotebookExecution` request."
    pub const STOPPED: &str = "STOPPED";
    /// "`FINISHED` indicates that the execution has completed without error."
    pub const FINISHED: &str = "FINISHED";
    /// "`FAILED` indicates that the execution failed."
    pub const FAILED: &str = "FAILED";

    /// The statuses a notebook execution never leaves, so a stop has nothing left to
    /// do (`NotebookExecutionStatus` doc).
    pub const TERMINAL: &[&str] = &[FINISHED, FAILED, STOPPED];
}

/// `PersistentAppUI$PersistentAppUIStatus` ("The status for the persistent application
/// user interface object"), as this emulator sets it.
///
/// CHOSEN, not modelled: the member targets a plain string and the model enumerates no
/// value for it. A UI is not serving the instant it is created — the service has to
/// attach it to its target resource first — so it starts at [`STARTING`] and the clock
/// owes it the move to [`ATTACHED`] after [`TRANSITION_SPAN`], the same way a cluster
/// owes its move out of `STARTING`.
pub mod persistent_app_ui_status {
    /// The UI has been created and is not serving yet.
    pub const STARTING: &str = "STARTING";
    /// The UI is attached to its target resource and serving.
    pub const ATTACHED: &str = "ATTACHED";
}

/// `com.amazonaws.emr#AuthMode`
/// (`smithy_query(query="shape", shape="AuthMode")`: SSO, IAM).
pub mod auth_mode {
    pub const IAM: &str = "IAM";

    /// The Studio authenticates its users through IAM Identity Center.
    pub const SSO: &str = "SSO";
}

/// How a Studio create that authenticates through IAM Identity Center and names no
/// `UserRole` is refused.
///
/// Recorded behavior for the text, which is entirely FIXED: Amazon EMR names the member
/// the request has to carry and the authentication mode that requires it, and
/// interpolates nothing, so every instance of the class is refused with the one sentence.
pub const USER_ROLE_REQUIRED_FOR_SSO_MESSAGE: &str =
    "UserRole is required to create a studio with SSO auth-mode.";

/// `com.amazonaws.emr#ClusterStateChangeReasonCode`, for the codes this emulator sets.
pub mod cluster_state_change_reason_code {
    pub const USER_REQUEST: &str = "USER_REQUEST";

    /// The cluster shut itself down because it had run every step it was given: "after the
    /// steps complete, the cluster stops" for a cluster whose
    /// `KeepJobFlowAliveWhenNoSteps` is false (`RunJobFlow` doc), which is what
    /// `Cluster$AutoTerminate` reports.
    pub const ALL_STEPS_COMPLETED: &str = "ALL_STEPS_COMPLETED";
}

/// `com.amazonaws.emr#InvalidRequestException$ErrorCode` — "the error code associated
/// with the exception" — for the refusal classes this service answers with a
/// discriminator.
///
/// Amazon EMR funnels every bad request through one exception shape, so this member is
/// the only thing beside prose that tells one refusal class from another. A class AWS
/// publishes no code for carries none: the member stays absent rather than being given
/// a value this emulator invented.
pub mod error_code {
    /// The code for a request that names a resource the service holds none of, which is
    /// what lets a caller tell "gone" from "malformed" — an idempotent teardown that
    /// swallows exactly this code and re-raises every other refusal.
    pub const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

    /// The code for a cluster id the service resolves no cluster for, as the CLUSTER
    /// surface reports it — the surface that words that refusal as a cluster id it does
    /// not accept ([`super::unknown_cluster_message`],
    /// [`super::unknown_cluster_without_id_message`]).
    pub const MALFORMED_CLUSTER_ID: &str = "MalformedClusterId";

    /// The same class on the LEGACY job-flow surface, which reaches a cluster as a job
    /// flow and words the refusal that way ([`super::unknown_job_flow_message`]): one
    /// exception shape, one status, but its own discriminator — so a client branching on
    /// the code learns which surface refused it.
    pub const INVALID_JOB_ID_MALFORMED: &str = "INVALID_JOB_ID_MALFORMED";

    /// The code for a `Marker` a listing did not mint — the continuation token class
    /// ([`super::malformed_marker_message`]). It is what lets a paginating client tell a
    /// stale or tampered page token from any other refusal of the same listing, instead
    /// of retrying the token that cannot work.
    pub const MALFORMED_MARKER: &str = "MalformedMarker";

    /// The code for a `ReleaseLabel` that names no release of the service's catalog
    /// ([`super::invalid_release_label_message`]).
    pub const INVALID_RELEASE_LABEL: &str = "INVALID_RELEASE_LABEL";

    /// The code the SESSION-CREDENTIALS read publishes for a cluster id it resolves no
    /// cluster for ([`super::unknown_cluster_without_id`]). That read states the
    /// cluster-id sentence without the id, and publishes its own discriminator for the
    /// class rather than the cluster reads' [`MALFORMED_CLUSTER_ID`] — so a client
    /// branching on the code learns which read refused it.
    pub const INVALID_CLUSTER_ID: &str = "InvalidClusterId";

    /// The code for an input parameter the service will not accept, as the
    /// release-DESCRIBE read publishes it for a label naming none of the catalog's
    /// releases ([`super::release_label_not_valid`]). That read words the class its own
    /// way, so it carries its own code too: the listing beside it answers
    /// [`INVALID_RELEASE_LABEL`].
    pub const INVALID_INPUT_PARAMETER: &str = "InvalidInputParameter";

    /// The code for a `SecurityConfiguration` document the service will not store
    /// ([`super::invalid_security_configuration`]).
    pub const INVALID_SECURITY_CONFIGURATION: &str = "INVALID_SECURITY_CONFIGURATION";

    /// The code for a creation-date window whose beginning is after its end
    /// ([`super::inverted_created_window_message`]). The code names the two members and
    /// the order they were given in, so a client learns which pair of filters to swap
    /// rather than only that the request was invalid.
    pub const INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE: &str =
        "INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE";
}

/// How a sentence that quotes an input member's value renders a value the request did
/// NOT carry: the member was absent, and the slot the value fills says so.
///
/// Recorded behavior: a cluster-id refusal of a request that omitted the member states
/// the same sentence an unusable id gets, with this in the id's place.
pub const ABSENT_VALUE: &str = "null";

/// The refusal for a request that names a resource this service holds none of: the
/// absent-resource discriminator ([`error_code::RESOURCE_NOT_FOUND`]) beside the
/// sentence that states which resource is absent.
///
/// Every operation that reports a named resource as absent refuses it through here, so
/// the discriminator is the same whichever resource and whichever operation the caller
/// asked about. A refusal of a class Amazon EMR publishes its own code for carries THAT
/// code, built where its wording is ([`unknown_cluster`], [`unknown_job_flow`]); a class
/// it publishes none for — a quota, a conflict, a state, another identifier it rejects
/// as not valid — is built at its own call site and carries no code at all.
pub fn resource_not_found(message: String) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::RESOURCE_NOT_FOUND.to_string()),
        message: Some(message),
    }
}

/// How the security-configuration surface words a `Name` the account holds no
/// configuration under: Amazon EMR's own sentence for that refusal, which quotes the name
/// the request sent with DOUBLE quotes.
///
/// Recorded behavior for the text, shared by `DescribeSecurityConfiguration` and
/// `DeleteSecurityConfiguration`: one template in one place, so the two operations cannot
/// come to word the same refusal differently. The class is an ABSENT resource, so the
/// refusal is built through [`resource_not_found`] and carries that discriminator.
pub fn unknown_security_configuration(name: &str) -> InvalidRequestException {
    resource_not_found(format!(
        "Security configuration with name \"{name}\" does not exist."
    ))
}

/// How `CreateSecurityConfiguration` words a `Name` the account already holds a
/// configuration under: Amazon EMR's own sentence for that refusal, which names the
/// SHAPE (`SecurityConfiguration`) and single-quotes the name the request sent.
///
/// Recorded behavior for the text. The class is a conflict rather than an absent
/// resource, so the refusal carries no discriminator — Amazon EMR publishes none for it.
pub fn security_configuration_exists_message(name: &str) -> String {
    format!("SecurityConfiguration with name '{name}' already exists.")
}

/// How `CreateSecurityConfiguration` refuses a `SecurityConfiguration` document that is
/// not JSON: Amazon EMR's own CONSTANT sentence for that refusal.
///
/// Recorded behavior for the text. It is constant by design — it names neither the
/// parser, nor the position it stopped at, nor anything the document carried — so a
/// caller matching it recognises every malformed document the same way.
pub const MALFORMED_SECURITY_CONFIGURATION_MESSAGE: &str =
    "Invalid security configuration; Malformed Json string.";

/// The refusal `CreateSecurityConfiguration` answers a document it will not store with:
/// the sentence for what is wrong with the document, under the discriminator Amazon EMR
/// publishes for an unusable security configuration
/// ([`error_code::INVALID_SECURITY_CONFIGURATION`]).
///
/// The code is paired with the wording HERE, in the one place a document refusal is
/// built, so no branch of the document validation can answer the class without its code.
pub fn invalid_security_configuration(message: String) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::INVALID_SECURITY_CONFIGURATION.to_string()),
        message: Some(message),
    }
}

/// How the notebook START refuses an `ExecutionEngine` that resolves no usable Amazon EMR
/// cluster: Amazon EMR's own sentence for that class, which states the cluster it
/// RESOLVED for the engine — none, which is the very reason the call is refused — and
/// that the cluster is not available.
///
/// Recorded behavior for the text AND for what fills the slot: a recorded request
/// carrying an `ExecutionEngine.Id` is refused with the absent-value rendering
/// ([`ABSENT_VALUE`]) there and NOT with that id, so the sentence echoes nothing of the
/// request and is constant for the class — the same way the release-label and
/// security-configuration sentences above it are. The rendering is composed from
/// [`ABSENT_VALUE`] rather than spelled out again, so one statement of "the member was
/// absent" serves every sentence that has to render it.
pub fn engine_not_available_message() -> String {
    format!("The EMR cluster Id {ABSENT_VALUE} is not available")
}


/// for. What IS the contract beside the text is the refusal itself — HTTP 400, with the
/// client error the operation models — rather than an empty success.
///
/// Every cluster-scoped operation answers THIS sentence, except the two whose refusal
/// Amazon EMR words differently and which say so at their own call site: the legacy
/// job-flow surface ([`unknown_job_flow_message`]) and `GetClusterSessionCredentials`
/// ([`unknown_cluster_without_id_message`]).
pub fn unknown_cluster_message(cluster_id: &str) -> String {
    format!("Cluster id {}", not_valid_message(cluster_id))
}

/// The refusal a CLUSTER-surface operation answers a `ClusterId` it resolves no cluster
/// for with: the cluster-id sentence and, beside it, the discriminator that surface
/// publishes for the class ([`error_code::MALFORMED_CLUSTER_ID`]).
///
/// The code is paired with the wording HERE, in the one place the wording is built, so
/// an operation cannot answer one surface's sentence under another surface's code.
pub fn unknown_cluster(cluster_id: &str) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::MALFORMED_CLUSTER_ID.to_string()),
        message: Some(unknown_cluster_message(cluster_id)),
    }
}

/// The characters a cluster id is drawn from, beside ASCII letters and digits.
///
/// Amazon EMR mints a cluster id as [`CLUSTER_ID_PREFIX`] and a body of letters and
/// digits, so the hyphen and the underscore are the only non-alphanumerics one can
/// carry. `ClusterId` is constrained by length alone (`len 0..256`, no `@pattern`:
/// `smithy_query(query="shape", shape="ClusterId")`), so the boundary validator passes
/// anything that fits and this class is what the service itself then enforces
/// ([`is_cluster_id_shaped`]) — the same division the session surface follows
/// ([`is_session_id_shaped`]).
const CLUSTER_ID_PUNCTUATION: [char; 2] = ['-', '_'];

/// Whether `cluster_id` could be a cluster id at all: every character is an ASCII letter
/// or digit, or one of [`CLUSTER_ID_PUNCTUATION`].
///
/// A value carrying anything else — an ARN's colons and slashes, a URL, a path, a space
/// — names no cluster Amazon EMR could ever have minted, so it is refused on its SHAPE
/// rather than looked up (see [`malformed_cluster_id_message`]).
pub fn is_cluster_id_shaped(cluster_id: &str) -> bool {
    cluster_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || CLUSTER_ID_PUNCTUATION.contains(&c))
}

/// How a cluster-scoped operation refuses a `ClusterId` that is not a cluster id at all.
///
/// Recorded behavior for the text: Amazon EMR states the MEMBER the value came in on,
/// named in the casing the wire uses for it, then the value exactly as the request sent
/// it, then that it is not valid — the same form the session surface states a malformed
/// session id in ([`malformed_session_id_message`]). This is the request-validation
/// refusal, which is why it names the member rather than the resource: nothing has been
/// looked up.
pub fn malformed_cluster_id_message(cluster_id: &str) -> String {
    format!("clusterId {}", not_valid_message(cluster_id))
}

/// How every listing refuses a `Marker` it did not mint: the member, the value the
/// request sent and that it is not valid.
///
/// Recorded behavior for the text, shared by every `Marker`-paged listing of this
/// service — one template in one place, so two listings cannot come to word the same
/// refusal differently.
pub fn malformed_marker_message(marker: &str) -> String {
    format!("Marker {}", not_valid_message(marker))
}

/// The refusal a listing answers a `Marker` it did not mint with: that sentence under
/// the continuation-token discriminator ([`error_code::MALFORMED_MARKER`]).
///
/// The code is paired with the wording HERE, in the one place the wording is built, so
/// no listing can answer the marker sentence without the marker code.
pub fn malformed_marker(marker: &str) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::MALFORMED_MARKER.to_string()),
        message: Some(malformed_marker_message(marker)),
    }
}

/// How a cluster launch that names BOTH an `AmiVersion` and a `ReleaseLabel` is refused.
///
/// A cluster runs one of the two, never both: `AmiVersion` "applies only to Amazon EMR AMI
/// versions 3.x and 2.x. For Amazon EMR releases 4.0 and later, `ReleaseLabel` is used"
/// (`RunJobFlow$AmiVersion` doc), so the two members name two different ways of choosing
/// what the cluster runs and a launch carrying both names no single one.
///
/// Recorded behavior for the text: Amazon EMR states the rule and then echoes both values
/// the request carried, which are what a caller has to choose between.
pub fn ami_version_and_release_label_message(ami_version: &str, release_label: &str) -> String {
    format!(
        "Only one AMI version and release label may be specified. Provided AMI: \
         {ami_version}, release label: {release_label}."
    )
}

/// How an operation that answers a fact ABOUT a release refuses a `ReleaseLabel` that
/// names none of the catalog's releases.
///
/// Recorded behavior for the text: Amazon EMR states that the supplied label is invalid
/// and echoes the label the request sent, with [`ABSENT_VALUE`] in its place when the
/// request carried no label at all.
pub fn invalid_release_label_message(release_label: &str) -> String {
    format!("The supplied release label is invalid: {release_label}.")
}

/// The refusal an operation answers an unusable `ReleaseLabel` with: that sentence under
/// the release-label discriminator ([`error_code::INVALID_RELEASE_LABEL`]).
pub fn invalid_release_label(release_label: &str) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::INVALID_RELEASE_LABEL.to_string()),
        message: Some(invalid_release_label_message(release_label)),
    }
}

/// How the operation that DESCRIBES a release refuses a `ReleaseLabel` naming none of
/// the catalog's releases: Amazon EMR's own CONSTANT sentence for that class on that
/// operation.
///
/// Recorded behavior for the text, and it is constant by design — the label the request
/// sent is deliberately not echoed, so every unoffered label is refused the same way.
/// The listing beside it words the same class its own way
/// ([`invalid_release_label_message`]), so the two sentences are not interchangeable.
pub const RELEASE_LABEL_NOT_VALID_MESSAGE: &str = "The release label isn't valid.";

/// The refusal the release DESCRIBE answers an unoffered label with: that constant
/// sentence under the discriminator it publishes for the class
/// ([`error_code::INVALID_INPUT_PARAMETER`]).
///
/// The code is paired with the wording HERE, in the one place that wording is built, so
/// neither release-label surface can answer its sentence under the other's code.
pub fn release_label_not_valid() -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::INVALID_INPUT_PARAMETER.to_string()),
        message: Some(RELEASE_LABEL_NOT_VALID_MESSAGE.to_string()),
    }
}

/// How a listing bounded by a CREATION-DATE window refuses a window whose beginning is
/// after its end.
///
/// Recorded behavior for the text, which is entirely FIXED: Amazon EMR names the two
/// members in the casing the wire uses for them and interpolates neither instant, so a
/// caller matching the sentence recognises every inverted window the same way.
pub fn inverted_created_window_message() -> String {
    "createdAfter date cannot be after createdBefore date.".to_string()
}

/// The refusal a listing answers an inverted creation-date window with: that sentence
/// under the inverted-window discriminator
/// ([`error_code::INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE`]).
///
/// The code is paired with the wording HERE, in the one place the wording is built, so
/// no listing can answer the inverted-window sentence without its code.
pub fn inverted_created_window() -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(
            error_code::INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE.to_string(),
        ),
        message: Some(inverted_created_window_message()),
    }
}

/// Whether a creation-date window is one no cluster can fall inside: its beginning is
/// strictly after its end.
///
/// A window is inverted only when the request bounds it at BOTH ends — one bound alone
/// is open on the other side — and an instant equal at both ends is the single-instant
/// window, which is satisfiable and therefore filtered rather than refused.
pub fn is_inverted_window(
    after: Option<&::aws_smithy_types::DateTime>,
    before: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    match (after, before) {
        (Some(after), Some(before)) => after.as_secs_f64() > before.as_secs_f64(),
        _ => false,
    }
}

/// How the LEGACY job-flow surface refuses a cluster id it holds no cluster for: the
/// operations that reach a cluster as a "job flow" answer one fixed sentence, naming
/// neither the id nor the `ClusterId` member.
///
/// Recorded behavior: the sentence is CONSTANT — no operation of this surface appends
/// the id it rejected. Which surface an operation belongs to is not read off its input
/// member: the managed-scaling MUTATORS take a `ClusterId` and answer this sentence,
/// while `GetManagedScalingPolicy` beside them reports a cluster id
/// ([`unknown_cluster_message`]), and `SetUnhealthyNodeReplacement` takes `JobFlowIds`
/// and reports a cluster id too.
pub fn unknown_job_flow_message() -> String {
    "Specified job flow ID not valid.".to_string()
}

/// The refusal a LEGACY job-flow-surface operation that models the client error answers
/// an id it resolves no cluster for with: that surface's sentence
/// ([`unknown_job_flow_message`]) under that surface's discriminator
/// ([`error_code::INVALID_JOB_ID_MALFORMED`]).
pub fn unknown_job_flow() -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::INVALID_JOB_ID_MALFORMED.to_string()),
        message: Some(unknown_job_flow_message()),
    }
}

/// How a shutdown request is refused when a cluster it names is locked against
/// termination: the lock "prevent[s] the Amazon EC2 instances from being terminated by an
/// API call or user intervention" (`Cluster$TerminationProtected` doc), and "to terminate
/// a cluster that has been locked ... you must first unlock the job flow by a subsequent
/// call to SetTerminationProtection in which you set the value to false"
/// (`SetTerminationProtection` doc).
///
/// Recorded behavior: the sentence is CONSTANT — plural and id-free, because the whole
/// batch is decided before any cluster is shut down, so nothing of the request or of the
/// state is interpolated into it.
pub fn termination_protected_message() -> String {
    "Could not shut down one or more job flows since they are termination protected."
        .to_string()
}

/// How `GetClusterSessionCredentials` refuses a cluster id it holds no cluster for: the
/// same statement as [`unknown_cluster_message`] with no id in it.
pub fn unknown_cluster_without_id_message() -> String {
    "Cluster id is not valid.".to_string()
}

/// The same refusal as [`unknown_cluster`] for the operation that states it without the
/// id: same surface, so the same discriminator.
pub fn unknown_cluster_without_id() -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::INVALID_CLUSTER_ID.to_string()),
        message: Some(unknown_cluster_without_id_message()),
    }
}

/// How `GetClusterSessionCredentials` refuses a cluster whose security configuration
/// enables neither runtime roles nor IAM Identity Center: Amazon EMR's own sentence,
/// which names the missing security-configuration feature and no cluster id. The
/// apostrophe is the typographic one Amazon EMR writes it with.
pub fn runtime_roles_not_enabled_message() -> String {
    "You haven\u{2019}t enabled runtime roles or IAM Identity Center for your cluster in \
     its security configuration."
        .to_string()
}

/// How the backend behind EMR Studio and EMR Notebooks words EVERY refusal it raises:
/// the sentence, then its own service name and the wire fields of the error it raised.
///
/// Recorded behavior for the text, and it is one template rather than one per operation:
/// the Studio and notebook-execution surfaces are served by the same backend, so each of
/// their refusals carries the same parenthesised service detail — the backend's name
/// ([`STUDIO_BACKEND_SERVICE`]), the status and error code of the refusal, and this
/// response's own request id
/// ([`::service_common::query_error_envelope::new_request_id`]). Nothing of the request
/// is interpolated: a caller matching the sentence sees the same text for every instance
/// of the class, and an operator reads from the detail which backend refused.
pub fn editors_refusal_message(sentence: &str) -> String {
    format!(
        "{sentence} (Service: {STUDIO_BACKEND_SERVICE}; Status Code: 400; Error Code: \
         InvalidRequestException; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// How a Studio operation refuses a `StudioId` the service holds no Studio for.
///
/// The sentence states that the Studio does not exist and names no Studio id — the id the
/// caller sent is not part of it — inside the backend's own wrapper
/// ([`editors_refusal_message`]).
pub fn unknown_studio_message() -> String {
    editors_refusal_message("Studio does not exist.")
}

/// How a notebook-execution operation refuses a `NotebookExecutionId` the service holds
/// no execution for.
///
/// Recorded behavior for the text: the same backend wrapper
/// ([`editors_refusal_message`]) in front of the statement that the execution does not
/// exist. The execution id the caller sent is NOT echoed, so every notebook-execution
/// operation refuses an unknown id with the one sentence.
pub fn unknown_notebook_execution_message() -> String {
    editors_refusal_message("Execution does not exist")
}

/// The service name the EMR Studio backend reports in the refusals it wraps.
const STUDIO_BACKEND_SERVICE: &str = "AWSEditors";

/// How an operation refuses a value that names no resource it can resolve: the value,
/// quoted, and that it is not valid.
///
/// The TEXT is built from the request — it names the value the caller sent — which is
/// the form Amazon EMR answers a rejected `TargetResourceArn` with, and the tail
/// [`unknown_cluster_message`] puts a cluster id in front of, so the wording lives in
/// one place.
pub fn not_valid_message(value: &str) -> String {
    format!("'{value}' is not valid.")
}

/// How a persistent-application-UI operation refuses a `PersistentAppUIId` the service
/// holds no UI for.
///
/// AWS does not document the wording, so the text is built from the request: it names
/// the id the caller asked for. What IS the contract is the refusal itself — HTTP 400
/// with the error code `InvalidRequestException` — rather than an empty success.
pub fn unknown_persistent_app_ui_message(persistent_app_ui_id: &str) -> String {
    format!("Persistent application user interface id '{persistent_app_ui_id}' is not valid.")
}

/// How an instance-group-scoped operation that models no client error refuses an
/// `InstanceGroupId` the named cluster carries no group for.
///
/// AWS does not document the wording, so the text is built from the request: it names
/// the id the caller asked for, through the same tail [`unknown_cluster_message`] uses.
/// What IS the contract is the refusal itself — HTTP 400 with the error code
/// `ValidationException` — rather than an empty success.
pub fn unknown_instance_group_message(instance_group_id: &str) -> String {
    format!("Instance group id {}", not_valid_message(instance_group_id))
}

/// How `RemoveAutoScalingPolicy` refuses an instance group that carries no automatic
/// scaling policy for it to remove.
///
/// "Removes an automatic scaling policy from a specified instance group"
/// (`RemoveAutoScalingPolicy` doc, Smithy model): there is no policy to remove from a
/// group carrying none, and Amazon EMR refuses that call — HTTP 400, with the group the
/// caller named quoted in the message — rather than answering an empty success.
pub fn no_auto_scaling_policy_message(instance_group_id: &str) -> String {
    format!("Auto Scaling policy was not found for instance group '{instance_group_id}'.")
}

/// Refuse an `AutoScalingPolicy` this emulator cannot hold as submitted.
///
/// `MinCapacity` is "the lower boundary of Amazon EC2 units" and `MaxCapacity` "the
/// upper boundary of Amazon EC2 units" of the scaling activity (`ScalingConstraints`
/// member docs, Smithy model), so a negative lower boundary, and a lower boundary above
/// the upper one, name no range the activity could stay inside. The rule list is bounded
/// by [`MAX_SCALING_RULES`] because every rule is retained and reported on every later
/// read of the group.
///
/// Every path that persists an `AutoScalingPolicy` calls this before storing it.
pub fn validate_auto_scaling_policy(policy: &AutoScalingPolicy) -> Result<(), String> {
    let constraints = &policy.constraints;
    if constraints.min_capacity < 0 {
        return Err(format!(
            "Constraints.MinCapacity {} is below 0",
            constraints.min_capacity
        ));
    }
    if constraints.min_capacity > constraints.max_capacity {
        return Err(format!(
            "Constraints.MinCapacity {} is above Constraints.MaxCapacity {}",
            constraints.min_capacity, constraints.max_capacity
        ));
    }
    if policy.rules.len() > MAX_SCALING_RULES {
        return Err(format!(
            "Rules carries {} rules; an automatic scaling policy names at most \
             {MAX_SCALING_RULES}",
            policy.rules.len()
        ));
    }
    Ok(())
}

/// Refuse a `ManagedScalingPolicy` whose compute limits cannot hold together.
///
/// `MinimumCapacityUnits` is "the lower boundary of Amazon EC2 units" and
/// `MaximumCapacityUnits` "the upper boundary of Amazon EC2 units"
/// (`ComputeLimits` member docs, Smithy model), so a minimum above the maximum names no
/// range the scaling activity could stay inside. Answers the refusal text, which is
/// Amazon EMR's own fixed sentence for the class
/// ([`invalid_managed_scaling_compute_limits_message`]) and so names neither member nor
/// capacity.
pub fn validate_managed_scaling_policy(policy: &ManagedScalingPolicy) -> Result<(), String> {
    if let Some(limits) = policy.compute_limits.as_ref() {
        if limits.minimum_capacity_units > limits.maximum_capacity_units {
            return Err(invalid_managed_scaling_compute_limits_message());
        }
    }
    Ok(())
}

/// The smallest idle time an auto-termination policy may name, in seconds: "You can
/// specify a minimum of 60 seconds and a maximum of 604800 seconds (seven days)"
/// (`AutoTerminationPolicy$IdleTimeout` doc, Smithy model —
/// `smithy_query(query="doc", shape="AutoTerminationPolicy", member="IdleTimeout")`).
pub const MIN_AUTO_TERMINATION_IDLE_TIMEOUT: i64 = 60;

/// The largest idle time an auto-termination policy may name, in seconds, from the same
/// sentence of the same member's documentation ([`MIN_AUTO_TERMINATION_IDLE_TIMEOUT`]).
pub const MAX_AUTO_TERMINATION_IDLE_TIMEOUT: i64 = 604_800;

/// How an auto-termination-policy attachment refuses an `IdleTimeout` outside the window
/// that member documents.
///
/// Recorded behavior for the text: Amazon EMR states the member and the two bounds it
/// holds the member to, and nothing of the request — the rejected value is NOT part of
/// the sentence — so the sentence is the same for every out-of-range value. The numbers
/// are the documented bounds themselves
/// ([`MIN_AUTO_TERMINATION_IDLE_TIMEOUT`], [`MAX_AUTO_TERMINATION_IDLE_TIMEOUT`]), so the
/// window the service enforces and the window it states can never drift apart.
pub fn invalid_idle_timeout_message() -> String {
    format!(
        "Auto-termination policy IdleTimeout must be within a minimum of \
         {MIN_AUTO_TERMINATION_IDLE_TIMEOUT} seconds, and a maximum of \
         {MAX_AUTO_TERMINATION_IDLE_TIMEOUT} seconds."
    )
}

/// Refuse an `AutoTerminationPolicy` whose `IdleTimeout` falls outside the documented
/// window, inclusive of both bounds.
///
/// "Specifies the amount of idle time in seconds after which the cluster automatically
/// terminates. You can specify a minimum of 60 seconds and a maximum of 604800 seconds
/// (seven days)." (`AutoTerminationPolicy$IdleTimeout` doc.) The model bounds the member
/// by its type alone (a `Long`, with no `@range`), so the service is what holds it to that
/// window. The member is optional, so a policy carrying none names no idle time to hold —
/// that is an attached policy rather than an out-of-range one.
///
/// Every path that persists an `AutoTerminationPolicy` calls this before storing it, so a
/// policy outside the window is never attached, whichever call attached it.
pub fn validate_auto_termination_policy(policy: &AutoTerminationPolicy) -> Result<(), String> {
    if let Some(idle_timeout) = policy.idle_timeout {
        if !(MIN_AUTO_TERMINATION_IDLE_TIMEOUT..=MAX_AUTO_TERMINATION_IDLE_TIMEOUT)
            .contains(&idle_timeout)
        {
            return Err(invalid_idle_timeout_message());
        }
    }
    Ok(())
}

/// How an instance-group modification refuses an Amazon EC2 instance id that names no
/// instance of the group.
///
/// This emulator launches no Amazon EC2 instances, so no id names one: the refusal — HTTP
/// 400, naming the id the caller sent, through the same tail
/// [`unknown_instance_group_message`] uses — is what keeps a termination the service
/// cannot carry out from being answered as a success.
pub fn unknown_ec2_instance_message(instance_id: &str) -> String {
    format!("EC2 instance id {}", not_valid_message(instance_id))
}

/// How a session-scoped operation refuses a session id the named cluster carries no
/// session for: Amazon EMR's own sentence for that refusal, which names the SESSION id
/// the request sent and nothing else — no cluster id, and no clause about where the
/// session was looked for.
///
/// Recorded behavior for the text: the cluster the lookup ran against is already the one
/// the request named, so Amazon EMR states only the id that did not resolve. A cluster
/// that resolves no cluster at all is the OTHER refusal, worded by
/// [`unknown_cluster_message`] and reached before this one.
pub fn unknown_session_message(session_id: &str) -> String {
    format!("Session id {}", not_valid_message(session_id))
}

/// The prefix every session id carries, as Amazon EMR mints it: an interactive session
/// is identified by this prefix and a body of letters and digits, the same way a cluster
/// id is [`CLUSTER_ID_PREFIX`] and a body ([`is_session_id_shaped`]).
///
/// EVIDENCE for the prefix, and why it is a CONSTANT rather than a literal in the gate:
/// recorded behavior is the only statement of the form — the model constrains `SessionId`
/// by length alone — and it is recorded on both sides: a session read of an id of THIS
/// form reaches the lookups and is refused for the resource it names, while one carrying
/// another resource's prefix is refused on its shape before any resource is resolved. The
/// constant is therefore the one place the form is written down, so the operation that
/// MINTS a session id (`StartSession`, still a scaffold stub and its own issue) mints it
/// from the same statement this gate enforces, and the two cannot come apart.
pub const SESSION_ID_PREFIX: &str = "is-";

/// Whether `session_id` could be a session id at all: the session prefix
/// ([`SESSION_ID_PREFIX`]) followed by a non-empty body of ASCII letters and digits.
///
/// A value of any other form — another resource's prefix, no prefix, a bare prefix, a
/// space, a slash, a control character, punctuation outside the class — names no session
/// Amazon EMR could ever have minted, so it is refused on its SHAPE rather than looked up
/// (see [`malformed_session_id_message`]). The Smithy model constrains `SessionId` by
/// length alone (`len 0..256`, no `@pattern`), so the boundary validator passes anything
/// that fits and this is the class the service itself then enforces — the same division
/// the cluster surface follows ([`is_cluster_id_shaped`]).
pub fn is_session_id_shaped(session_id: &str) -> bool {
    match session_id.strip_prefix(SESSION_ID_PREFIX) {
        Some(body) => !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// How a session-scoped operation refuses a `SessionId` that is not a session id at all.
///
/// Recorded behavior for the text: Amazon EMR states the session's QUALIFIED path — the
/// cluster id, the `session` segment and the session id, exactly as the request sent
/// them — and that it is not valid. The member name leads the sentence in the casing the
/// wire uses for it.
pub fn malformed_session_id_message(cluster_id: &str, session_id: &str) -> String {
    format!("sessionId '{cluster_id}/session/{session_id}' is not valid.")
}

/// How many tags one Amazon EMR resource carries at the most.
///
/// Amazon EMR's tagging restrictions publish a maximum of 50 tags per resource (Amazon
/// EMR Management Guide, "Tag Clusters"), and that ceiling is also what keeps the tag
/// list this emulator persists — and clones into every later read of the resource —
/// bounded however many times a caller adds to it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key, in Unicode characters: a tag is "a required key string with a
/// maximum of 128 characters" (`AddTags$Tags` doc, Smithy model). Characters, not bytes,
/// so a key of non-ASCII text is measured the way Amazon EMR measures it.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value, in Unicode characters: a tag carries "an optional value string
/// with a maximum of 256 characters" (`AddTags$Tags` doc, Smithy model).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The symbols a tag key or value may carry beside letters, digits and spaces.
///
/// The symbol set of the tag character class Amazon EMR states for a tag
/// ([`TAG_CHARACTER_PATTERN`]), so a tag carrying any other character — a symbol such as
/// an emoji, punctuation outside this list, a control character — is not a tag Amazon EMR
/// holds. Listed in the order the refusal enumerates them
/// ([`tag_charset_message`]), which is also the order the pattern states them in: the
/// class a caller is TOLD about and the class that is ENFORCED are this one list, so they
/// cannot come apart.
///
/// EVIDENCE for the list: Amazon EMR states this class in ONE place only — the sentence
/// it refuses an out-of-class tag with, which enumerates `_ . : / = + \ -` — and the
/// Smithy model constrains neither `Tag$Key` nor `Tag$Value` with a `@pattern`. So the
/// enumeration in that sentence is the whole of what is known about the class, and it is
/// what is enforced here: the backslash belongs to it, and `@`, which the sentence does
/// not name, does not. A symbol the service tells no caller about is not one it is known
/// to hold.
pub const TAG_PUNCTUATION: [char; 8] = ['_', '.', ':', '/', '=', '+', '\\', '-'];

/// The character class a tag key and value are drawn from, as Amazon EMR states it for a
/// tag: Unicode letters (`\p{L}`), separators (`\p{Z}`) and numbers (`\p{N}`), plus
/// [`TAG_PUNCTUATION`].
///
/// This is the one statement of the class, so a caller reading it and
/// [`is_tag_char`] applying it cannot come to disagree about what a tag may hold.
pub const TAG_CHARACTER_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\\\-]*)$";

/// How a tag key or value carrying a character outside the tag character class is
/// refused: Amazon EMR's own sentence for that class of request, which names the half of
/// the pair it rejected, quotes the text the request sent, and then states the class
/// itself — so the caller learns from the refusal what a tag may hold.
///
/// `member` is the singular member name (`key` or `value`); the sentence names it once as
/// the member it rejected and once in the plural the rule is stated over. The symbol half
/// of the class is rendered from [`TAG_PUNCTUATION`] rather than spelled out again, which
/// is what keeps the enumerated class the enforced one ([`is_tag_char`]).
fn tag_charset_message(member: &str, value: &str) -> String {
    // The enumeration, one symbol at a time with a space between: two characters per
    // symbol, so the string is built in one allocation.
    let mut symbols = String::with_capacity(TAG_PUNCTUATION.len() * 2);
    for symbol in TAG_PUNCTUATION {
        if !symbols.is_empty() {
            symbols.push(' ');
        }
        symbols.push(symbol);
    }
    format!(
        "Invalid tag {member}: '{value}'. Tag {member}s may only contain unicode letters, \
         digits, whitespace, or one of these symbols: '{symbols}'."
    )
}

/// How a tag key or value past its documented length bound is refused.
///
/// AWS publishes no wording for this class, so it is this service's own sentence for a
/// value it rejects ([`not_valid_message`]) with the half of the pair in front of it, the
/// way [`unknown_cluster_message`] is built: the member the caller has to fix and the
/// value it sent. What IS the contract is the refusal itself — HTTP 400 with the client
/// error the operation models — rather than a tag Amazon EMR would never have held.
fn tag_too_long_message(member: &str, value: &str) -> String {
    format!("Tag {member} {}", not_valid_message(value))
}

/// Whether `c` may appear in a tag key or value: a letter or number of any script, a
/// separator, or one of [`TAG_PUNCTUATION`] — the class [`TAG_CHARACTER_PATTERN`] states.
///
/// `char::is_alphanumeric` is what makes the rule "representable in UTF-8" rather than
/// ASCII: a Cyrillic or CJK letter is a letter, while an emoji is a symbol and is not. A
/// control character is not a separator, so a tab or a newline is outside the class even
/// though both are whitespace.
pub fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || (c.is_whitespace() && !c.is_control()) || TAG_PUNCTUATION.contains(&c)
}

/// Refuse a submitted tag list that holds something no Amazon EMR tag could be.
///
/// `Tag$Key` is "a user-defined key, which is the minimum required information for a
/// valid tag" (Smithy model), so an entry carrying none — absent or empty — names no tag;
/// a tag is "a required key string with a maximum of 128 characters, and an optional
/// value string with a maximum of 256 characters" (`AddTags$Tags` doc), which bounds each
/// half; and [`TAG_CHARACTER_PATTERN`] is the class both are drawn from. The model types
/// both members as plain strings and constrains neither, so none of this is refused at
/// the boundary.
///
/// Each class has the sentence Amazon EMR answers it with, and both name the member of
/// the request that was rejected and the text it carried: the character class states the
/// class a tag is drawn from ([`tag_charset_message`]), the length bound this service's
/// own sentence for a value it will not hold ([`tag_too_long_message`]).
pub fn validate_tags(tags: &[Tag]) -> Result<(), String> {
    for tag in tags {
        let Some(key) = tag.key.as_deref().filter(|key| !key.is_empty()) else {
            // There is no value to quote: an entry with no key, or an empty one, names
            // no tag at all.
            return Err("Tag key is not valid.".to_string());
        };
        let value = tag.value.as_deref().unwrap_or_default();
        for (member, text, max) in [
            ("key", key, MAX_TAG_KEY_CHARS),
            ("value", value, MAX_TAG_VALUE_CHARS),
        ] {
            if text.chars().count() > max {
                return Err(tag_too_long_message(member, text));
            }
            if text.chars().any(|c| !is_tag_char(c)) {
                return Err(tag_charset_message(member, text));
            }
        }
    }
    Ok(())
}

/// Merge `submitted` into the tag set `held` a resource already carries: a key the
/// resource does not carry is added, and a key it does carry keeps its place and takes
/// the submitted value.
///
/// A tag is "a key-value pair containing user-defined metadata" (`Tag` doc, Smithy
/// model), so one key names at most one tag of a resource and re-submitting a key
/// replaces its value rather than adding a second entry — which is what makes adding the
/// same tag twice leave the resource exactly as the first call did.
///
/// The SIZE the merge would leave behind is computed first, in one pass over the two
/// lists and without copying either of them: the set a merge leaves is what the resource
/// holds plus the DISTINCT submitted keys it does not, so a request that would take the
/// resource past [`MAX_TAGS_PER_RESOURCE`] is refused before anything is allocated for
/// it, and the resource's tags are left exactly as they were rather than half merged.
/// The count is the exact resulting one rather than the submitted length, because a list
/// that repeats one key adds one tag however long it is.
///
/// Both passes are keyed lookups, so the work is linear in the submitted list a caller
/// already sent rather than quadratic in it, and the list that IS built is bounded by
/// the ceiling checked above.
pub fn merge_tags(held: &mut Vec<Tag>, submitted: &[Tag]) -> Result<(), String> {
    let carried: HashSet<Option<&str>> = held.iter().map(|tag| tag.key.as_deref()).collect();
    let added: HashSet<Option<&str>> = submitted
        .iter()
        .map(|tag| tag.key.as_deref())
        .filter(|key| !carried.contains(key))
        .collect();
    let resulting = held.len() + added.len();
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "These tags would leave the resource carrying {resulting} tags; a resource \
             carries at most {MAX_TAGS_PER_RESOURCE} tags"
        ));
    }

    let mut merged = held.clone();
    // Where each key sits in `merged`, so a re-submitted key is found by lookup rather
    // than by scanning the list again for every entry submitted.
    let mut position: HashMap<Option<String>, usize> = HashMap::new();
    for (at, tag) in merged.iter().enumerate() {
        position.entry(tag.key.clone()).or_insert(at);
    }
    for tag in submitted {
        match position.get(&tag.key) {
            Some(&at) => merged[at].value = tag.value.clone(),
            None => {
                position.insert(tag.key.clone(), merged.len());
                merged.push(tag.clone());
            }
        }
    }
    *held = merged;
    Ok(())
}

/// Take every tag whose key is one of `keys` off the tag set `held` a resource carries.
///
/// `TagKeys` is "a list of tag keys to remove from the resource" (`RemoveTags$TagKeys`
/// doc, Smithy model), so a key names the resource's tag of that key whatever value it
/// holds, the tags beside the named ones keep their place and their values, and a key the
/// resource carries no tag for takes nothing off — which is what makes removing the same
/// key twice leave the resource exactly as the first call did.
pub fn remove_tag_keys(held: &mut Vec<Tag>, keys: &[String]) {
    held.retain(|tag| {
        tag.key
            .as_deref()
            .is_none_or(|key| !keys.iter().any(|named| named == key))
    });
}

/// The tag set of ONE Amazon EMR resource, for an operation that changes it.
///
/// A record holds its tags in one of two ways: a record the wire shapes are PROJECTED
/// from ([`ClusterRecord`]) always holds the list, while a record held AS its wire shape
/// (a Studio, a session) reports `Tags` only when the resource carries some. A tag
/// operation reads the set the resource holds and stores the set it computed through
/// here, so it never has to know which kind of record its request reached.
#[derive(Debug)]
pub enum TagSetMut<'a> {
    /// A record that always holds its tag list.
    List(&'a mut Vec<Tag>),
    /// A record that reports its tags only when it carries some.
    Optional(&'a mut Option<Vec<Tag>>),
}

impl TagSetMut<'_> {
    /// The tags the resource carries now.
    pub fn held(&self) -> Vec<Tag> {
        match self {
            Self::List(held) => held.to_vec(),
            Self::Optional(Some(held)) => held.to_vec(),
            Self::Optional(None) => Vec::new(),
        }
    }

    /// Store `tags` as the resource's WHOLE tag set. A resource left carrying none
    /// reports no tags at all, the way one created without any does.
    pub fn store(&mut self, tags: Vec<Tag>) {
        match self {
            Self::List(held) => **held = tags,
            Self::Optional(held) => **held = (!tags.is_empty()).then_some(tags),
        }
    }
}

/// Why a tag operation could not reach the resource its request named.
///
/// The resolution is the same for every tag operation, so it fails the same way whichever
/// of them asked, while the client error each one models is its own: the class travels out
/// of the resolution and is lifted into that error at the call site
/// ([`TagTargetRefusal::into_invalid_request`]).
#[derive(Debug)]
pub enum TagTargetRefusal {
    /// A cluster id this service resolves no cluster for, which the cluster surface
    /// refuses under its own discriminator ([`unknown_cluster`]) whichever operation
    /// asked.
    MalformedClusterId(String),
    /// A value naming no resource this service holds, worded the way the surface that
    /// owns an id of that form words it: a rejected identifier, which carries no
    /// discriminator.
    NotValid(String),
    /// A named resource the service holds none of, which carries the absent-resource
    /// discriminator ([`resource_not_found`]).
    Absent(String),
}

impl TagTargetRefusal {
    /// This refusal as the client error every tag operation models.
    pub fn into_invalid_request(self) -> InvalidRequestException {
        match self {
            Self::MalformedClusterId(cluster_id) => unknown_cluster(&cluster_id),
            Self::NotValid(message) => InvalidRequestException {
                error_code: None,
                message: Some(message),
            },
            Self::Absent(message) => resource_not_found(message),
        }
    }
}

/// The configurations one instance group may hold, counted over the whole nested tree.
///
/// CHOSEN, not documented: every entry is retained on the group's record and reported on
/// every later read of it, and a `MERGE` reconfiguration adds to what is already there,
/// so the stored tree needs a ceiling of its own. The same reasoning bounds
/// [`MAX_SCALING_RULES`].
pub const MAX_INSTANCE_GROUP_CONFIGURATIONS: usize = 100;

/// The number of entries `configurations` holds, nested children included, stopping at
/// `cap`.
///
/// Bounded by construction: the walk stops as soon as the count passes `cap`, so a tree
/// that busts the ceiling is refused without being walked to its end.
pub fn configuration_entries(configurations: &[Configuration], cap: usize) -> usize {
    let mut seen = 0usize;
    let mut pending: Vec<&Configuration> = configurations.iter().collect();
    while let Some(configuration) = pending.pop() {
        seen += 1;
        if seen > cap {
            return seen;
        }
        if let Some(children) = configuration.configurations.as_deref() {
            pending.extend(children.iter());
        }
    }
    seen
}

/// The region an ARN carries when the caller's request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a clock-owed cluster transition
/// (`STARTING` -> ready, `TERMINATING` -> `TERMINATED`) comes due.
///
/// CHOSEN, not documented: the real service takes minutes. It is deliberately
/// non-zero so a caller acting straight after `RunJobFlow`/`TerminateJobFlows` still
/// observes the transient state, and it is stored as a due instant — nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// How long a new cluster spends in EACH phase of coming up: `STARTING` while the cluster's
/// Amazon EC2 capacity is provisioned, and then `BOOTSTRAPPING` while its bootstrap actions
/// run — "a list of bootstrap actions to run before Hadoop starts on the cluster nodes"
/// (`RunJobFlow$BootstrapActions` doc, Smithy model) — after which the cluster is ready and
/// its steps can run.
///
/// CHOSEN, not documented: the real service spends minutes on each phase, since it acquires
/// real instances. It is one span PER PHASE rather than one for the whole launch because
/// each phase is a state of its own a client reads and waits on, and it is deliberately
/// long enough that a client polling `DescribeCluster` right after `RunJobFlow` observes
/// the cluster before it is ready. Stored as a due instant — nothing sleeps.
pub const CLUSTER_PHASE_SPAN: Duration = Duration::from_secs(3);

/// How long a step occupies one of its cluster's concurrency slots: the span between the
/// `Status.Timeline.StartDateTime` and the `EndDateTime` of a step this service ran the
/// lifecycle of.
///
/// CHOSEN, not documented: a real step takes as long as the application it submits does.
/// Long enough that a client polling `DescribeStep` observes the step `RUNNING` before it
/// ends, which is the phase the runner exists to make observable.
pub const STEP_RUN_SPAN: Duration = Duration::from_secs(2);

/// `FailureDetails$Reason` — and `StepStateChangeReason$Message` — of a step whose
/// lifecycle this service ran and whose application it did not.
///
/// This service always runs the step LIFECYCLE: it accepts a step, queues it behind whatever
/// its cluster is already running, starts it when a concurrency slot frees and records its
/// timeline. What it can RUN is the submission it holds an engine and bytes for — a
/// `spark-submit` of a Python application in Amazon S3 ([`spark_step_run`]), a read-only
/// statement resolvable in the cluster's metastore ([`step_runner`]) and a command whose exit
/// status the session-local utilities answer ([`step_command_status`]). Every other
/// submission — a jar, a command this service answers for nothing of — produced no outcome,
/// so the step ENDS in the modelled failure state with this reason saying exactly why,
/// rather than reporting `COMPLETED` for work nothing performed.
pub const STEP_NOT_EXECUTED_REASON: &str =
    "This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended \
     without running its application.";

/// `FailureDetails$Message` for the same step: "the descriptive message including the
/// error the Amazon EMR service has identified as the cause of step failure"
/// (`FailureDetails$Message` doc), which here is what was not run.
pub const STEP_NOT_EXECUTED_MESSAGE: &str =
    "No application was started for this step: this emulator holds the cluster's step \
     state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects \
     did not happen.";

/// `ClusterStatus$StateChangeReason.Message` for the shutdown a cluster that does not stay
/// up takes once its step queue has emptied and every step of it COMPLETED.
///
/// Recorded behavior for the text, beside the modelled code for that shutdown
/// (`ALL_STEPS_COMPLETED`): the message is Amazon EMR's own fixed sentence, with nothing of
/// the request or of the cluster interpolated into it.
pub const CLUSTER_STEPS_COMPLETED_REASON: &str = "Steps completed";

/// `ClusterStatus$StateChangeReason.Message` for the same shutdown of a cluster whose queue
/// emptied with a step that did NOT complete — one that failed, was cancelled or was cut
/// short.
///
/// Recorded behavior for the text, in the same fixed form: the queue has run either way, so
/// the code is still `ALL_STEPS_COMPLETED`, and the message is what says the steps did not
/// all succeed.
pub const CLUSTER_STEPS_COMPLETED_WITH_ERRORS_REASON: &str = "Steps completed with errors";

/// How long each phase of a notebook execution's status progression lasts
/// ([`NOTEBOOK_EXECUTION_PROGRESSION`]).
///
/// CHOSEN, not documented: a real execution takes as long as its notebook does, and here the
/// notebook's own run is what the `RUNNING` phase waits for, so the clock only paces the
/// moves AROUND that run. Two seconds per move, so a client that polls
/// `DescribeNotebookExecution` observes the phases the model names on the way rather than
/// only the status the execution ended in, and the whole walk still settles well inside one
/// poll of a client that polls seconds apart. Stored as a due instant — nothing sleeps.
pub const NOTEBOOK_PHASE_SPAN: Duration = Duration::from_secs(2);

/// `ListClusters` page size: "This call returns a maximum of 50 clusters in unsorted
/// order per call, but returns a marker to track the paging of the cluster list across
/// multiple ListClusters calls." (`ListClusters` doc, Smithy model.)
pub const LIST_CLUSTERS_PAGE_SIZE: usize = 50;

/// `ListSteps` page size: "The CLI automatically paginates results to return a list
/// greater than 50 steps. To return more than 50 steps using the CLI, specify a
/// `Marker` …" (`ListSteps` doc, Smithy model.)
pub const LIST_STEPS_PAGE_SIZE: usize = 50;

/// `ListSecurityConfigurations`, `ListStudios` and `ListStudioSessionMappings` page size.
///
/// CHOSEN, not documented: those three operations paginate on `Marker` and state no page
/// size, so each one takes the 50 Amazon EMR publishes for the listings that DO state one
/// ([`LIST_CLUSTERS_PAGE_SIZE`], [`LIST_STEPS_PAGE_SIZE`]).
pub const LIST_PAGE_SIZE: usize = 50;

/// "You can specify a maximum of 10 `stepIDs`." (`ListSteps` doc, Smithy model.)
pub const LIST_STEPS_MAX_STEP_IDS: usize = 10;

/// "A maximum of 256 steps are allowed in each job flow." (`RunJobFlow` and
/// `AddJobFlowSteps` docs, Smithy model.)
pub const MAX_STEPS_PER_JOB_FLOW: usize = 256;

/// "The maximum number of clusters allowed is 10." (`TerminateJobFlows` doc, Smithy
/// model.)
pub const MAX_TERMINATE_JOB_FLOW_IDS: usize = 10;

/// "Specifies the number of steps that can be executed concurrently. The default value
/// is 1. The maximum value is 256." (`RunJobFlow$StepConcurrencyLevel` doc, Smithy
/// model.)
pub const DEFAULT_STEP_CONCURRENCY_LEVEL: i32 = 1;
/// The band's lower bound, which the documented DEFAULT also sits on: "You can specify a
/// minimum of 1 step" (`ModifyCluster$StepConcurrencyLevel` doc, Smithy model). Named
/// apart from [`DEFAULT_STEP_CONCURRENCY_LEVEL`] so the band describes itself where it is
/// enforced and stated ([`step_concurrency_level_violation`]).
pub const MIN_STEP_CONCURRENCY_LEVEL: i32 = DEFAULT_STEP_CONCURRENCY_LEVEL;
/// See [`DEFAULT_STEP_CONCURRENCY_LEVEL`].
pub const MAX_STEP_CONCURRENCY_LEVEL: i32 = 256;

/// How every operation that takes a `StepConcurrencyLevel` refuses one that lies outside
/// EVERY band Amazon EMR offers — below the lower bound, or above the widest ceiling any
/// step execution mode raises it to: Amazon EMR's own sentence for that class, which
/// states the band a cluster's own mode allows and nothing of the request.
///
/// Recorded behavior for the text. Both figures are read from the band this service holds
/// a level to ([`MIN_STEP_CONCURRENCY_LEVEL`], [`MAX_STEP_CONCURRENCY_LEVEL`]), so the
/// band a caller is TOLD about is the band that is ENFORCED, and the level the request
/// carried is not echoed back into it. A level that one mode WOULD take is the other class
/// ([`step_concurrency_level_exceeded_message`]), and the two are not interchangeable.
pub fn invalid_step_concurrency_level_message() -> String {
    format!(
        "Invalid step concurrency level. Allowed level is from \
         {MIN_STEP_CONCURRENCY_LEVEL} to {MAX_STEP_CONCURRENCY_LEVEL}"
    )
}

/// The mode a cluster runs its steps in, which is what sets the ceiling a
/// `StepConcurrencyLevel` is held to.
///
/// The Smithy model carries no member to ask for a mode on any operation
/// (`smithy_query(query="operation", operation="RunJobFlow,ModifyCluster")`), so every
/// cluster this service holds runs in the default mode and is held to its ceiling
/// ([`MAX_STEP_CONCURRENCY_LEVEL`]); the other mode is what the refusal points a caller
/// who needs more concurrency at.
pub mod step_execution_mode {
    /// The default mode, whose ceiling is [`super::MAX_STEP_CONCURRENCY_LEVEL`].
    pub const DEFAULT: &str = "STANDARD";

    /// The mode that raises the ceiling to [`DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL`].
    pub const DISTRIBUTED: &str = "DISTRIBUTED";

    /// The concurrency ceiling of [`DISTRIBUTED`], as the refusal states it.
    pub const DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL: i32 = 6400;
}

/// How every operation that takes a `StepConcurrencyLevel` refuses one ABOVE the ceiling
/// of the mode the cluster runs its steps in but inside the widest ceiling a mode raises
/// it to: Amazon EMR's own sentence for that class, which states the level it could not
/// take, the ceiling and the mode that ceiling belongs to, then the mode that would take it
/// and that mode's own ceiling.
///
/// Recorded behavior for the text. Every variable part is read from the request and from
/// this service's own bounds — the `level` the request carried, the ceiling of the mode the
/// cluster runs its steps in, and the other mode's name and ceiling — so the band a caller
/// is TOLD about is the band that is ENFORCED.
pub fn step_concurrency_level_exceeded_message(level: i32) -> String {
    format!(
        "StepConcurrencyLevel {level} exceeds the maximum ({MAX_STEP_CONCURRENCY_LEVEL}) for \
         StepExecutionMode {mode}. Set StepExecutionMode to {other} for up to {ceiling} \
         concurrent steps.",
        mode = step_execution_mode::DEFAULT,
        other = step_execution_mode::DISTRIBUTED,
        ceiling = step_execution_mode::DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL,
    )
}

/// Why this `StepConcurrencyLevel` cannot be accepted, if it cannot: the sentence for the
/// class it falls into.
///
/// A level the cluster's own mode would not take but another mode WOULD is told the band
/// that mode allows ([`step_concurrency_level_exceeded_message`]); a level no mode of the
/// service could take at all — below the lower bound, or above the widest ceiling any mode
/// raises it to — is refused with the band of the cluster's own mode
/// ([`invalid_step_concurrency_level_message`]). Every operation that takes the member
/// judges it through here, so one level cannot be refused two ways.
pub fn step_concurrency_level_violation(level: i32) -> Option<String> {
    let servable =
        MIN_STEP_CONCURRENCY_LEVEL..=step_execution_mode::DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL;
    if !servable.contains(&level) {
        return Some(invalid_step_concurrency_level_message());
    }
    if level > MAX_STEP_CONCURRENCY_LEVEL {
        return Some(step_concurrency_level_exceeded_message(level));
    }
    None
}

/// "A maximum of 256 steps are allowed in each CancelSteps request." (`CancelSteps`
/// doc, Smithy model.)
pub const MAX_CANCEL_STEP_IDS: usize = 256;

/// The step states a cancel can still act on: "When you use Amazon EMR releases 5.28.0
/// and later, you can cancel steps that are in a `PENDING` or `RUNNING` state."
/// (`CancelSteps` doc, Smithy model.)
pub const CANCELLABLE_STEP_STATES: &[&str] = &[step_state::PENDING, step_state::RUNNING];

/// The step states a step never leaves: the step has run, or it never will.
///
/// `StepState` is `PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED |
/// INTERRUPTED` (`smithy_query(query="shape", shape="StepState")`); the three before
/// `COMPLETED` are the ones a step is still moving through, so the rest are terminal.
pub const TERMINAL_STEP_STATES: &[&str] = &[
    step_state::COMPLETED,
    step_state::CANCELLED,
    step_state::FAILED,
    step_state::INTERRUPTED,
];

/// "A Studio can have a maximum of 5 subnets." (`CreateStudio$SubnetIds` doc, Smithy
/// model.)
pub const MAX_STUDIO_SUBNETS: usize = 5;

/// How many EMR Studios this emulator holds at once.
///
/// CHOSEN, not documented: AWS bounds Studios per account, and this number is what
/// stops the studio map growing without end — a create past it is refused with the
/// modelled `InvalidRequestException` rather than accepted.
pub const MAX_STUDIOS: usize = 100;

/// How many session mappings one Studio holds at once.
///
/// CHOSEN, not documented, for the same reason [`MAX_STUDIOS`] is: the identity a
/// mapping is keyed by comes from the request, so without a bound one studio could
/// accumulate mappings without end.
pub const MAX_SESSION_MAPPINGS_PER_STUDIO: usize = 500;

/// How many notebook executions this emulator holds at once.
///
/// CHOSEN, not documented, for the same reason [`MAX_STUDIOS`] is: every
/// `StartNotebookExecution` records one more execution for the life of the process, so
/// without a bound a client loop grows the map without end. A start past it is refused
/// with the operation's modelled `InvalidRequestException`, the way a quota is.
pub const MAX_NOTEBOOK_EXECUTIONS: usize = 1_000;

/// How many instance fleets one cluster holds at once.
///
/// CHOSEN, not documented here, and shaped after EMR's instance-group limit (a cluster
/// carries one master and one core group plus up to 48 task groups): it is what stops
/// one cluster's fleet list growing without end, the same way
/// [`MAX_STEPS_PER_JOB_FLOW`] bounds its step list.
pub const MAX_INSTANCE_FLEETS_PER_CLUSTER: usize = 50;

/// How many instance groups one cluster holds at once: a cluster carries one master and
/// one core group plus up to 48 task groups ("Amazon EMR Management Guide", instance
/// group limits), the same bound [`MAX_INSTANCE_FLEETS_PER_CLUSTER`] is shaped after.
pub const MAX_INSTANCE_GROUPS_PER_CLUSTER: usize = 50;

/// `ListInstanceGroups` page size.
///
/// CHOSEN, not documented: the operation paginates on `Marker` but AWS publishes no
/// page size for it. It is deliberately below [`MAX_INSTANCE_GROUPS_PER_CLUSTER`], so a
/// cluster can hold more groups than one page reports and the pagination contract — a
/// `Marker` that resumes the listing where the page stopped — is reachable rather than
/// dead.
pub const LIST_INSTANCE_GROUPS_PAGE_SIZE: usize = 20;

/// `ListInstanceFleets` page size.
///
/// CHOSEN, not documented, for the same reason [`LIST_INSTANCE_GROUPS_PAGE_SIZE`] is: the
/// operation paginates on `Marker` but AWS publishes no page size for it, and it is
/// deliberately below [`MAX_INSTANCE_FLEETS_PER_CLUSTER`] so the pagination contract is
/// reachable rather than dead.
pub const LIST_INSTANCE_FLEETS_PAGE_SIZE: usize = 20;

/// How many scaling rules one automatic scaling policy may name.
///
/// CHOSEN, not documented: `ScalingRuleList` carries no length constraint, and every
/// rule a policy was given is retained for the life of the group and cloned into every
/// later read of it, so without a bound one request would make every
/// `ListInstanceGroups` answer arbitrarily large. It is orders of magnitude above the
/// handful of rules a real policy carries, so no faithful request is refused by it.
pub const MAX_SCALING_RULES: usize = 100;

/// How many EBS volumes the instances of one instance group may carry IN TOTAL, summed
/// over every `EbsBlockDeviceConfig` its `EbsConfiguration` names.
///
/// CHOSEN, not documented in the model: `EbsBlockDeviceConfig$VolumesPerInstance` is an
/// unconstrained integer and `EbsBlockDeviceConfigList` carries no length, and every
/// requested volume becomes an `EbsBlockDevice` entry in each read of the group, so
/// without a bound on the TOTAL one request would make every later `ListInstanceGroups`
/// answer arbitrarily large. A request past it is refused rather than silently
/// truncated, on every path that persists an `InstanceGroupConfig` — see
/// [`validate_instance_group_config`].
pub const MAX_EBS_VOLUMES_PER_INSTANCE: i32 = 25;

/// How many `EbsBlockDeviceConfig` entries one instance group may name: no more than the
/// volumes it may attach at all ([`MAX_EBS_VOLUMES_PER_INSTANCE`]), since every entry
/// asks for at least one volume of its own configuration. Bounding the COUNT as well as
/// the total keeps the list this emulator persists, and walks on every read, bounded even
/// when the entries ask for no volumes.
pub const MAX_EBS_BLOCK_DEVICE_CONFIGS: usize = MAX_EBS_VOLUMES_PER_INSTANCE as usize;

/// How many EBS volumes an `EbsBlockDeviceConfig` requests when it names no count:
/// "Number of EBS volumes with a specific volume configuration that are associated with
/// every instance in the instance group" (`EbsBlockDeviceConfig$VolumesPerInstance`
/// doc) — one volume of that configuration.
pub const DEFAULT_VOLUMES_PER_INSTANCE: i32 = 1;

/// The account-wide default for `BlockPublicSecurityGroupRules`: "By default, the value
/// is `false` for accounts that have created Amazon EMR clusters before July 2019. For
/// accounts created after this, the default is `true`."
/// (`BlockPublicAccessConfiguration$BlockPublicSecurityGroupRules` doc, Smithy model.)
/// This emulator's account created no cluster before July 2019, so it takes the later
/// default.
pub const DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES: bool = true;

/// The port the account-wide default permits publicly: "By default, Port 22, which is
/// used for SSH access to the cluster Amazon EC2 instances, is in the list of
/// `PermittedPublicSecurityGroupRuleRanges`."
/// (`BlockPublicAccessConfiguration$PermittedPublicSecurityGroupRuleRanges` doc, Smithy
/// model.)
pub const DEFAULT_PERMITTED_PUBLIC_PORT: i32 = 22;

/// `com.amazonaws.emr#Port`'s published bounds, which `PortRange$MinRange` and
/// `PortRange$MaxRange` both target: `range { min: -1, max: 65535 }` (Smithy model).
pub const PORT_MIN: i32 = -1;
/// See [`PORT_MIN`].
pub const PORT_MAX: i32 = 65535;

/// How many ranges an exception list may carry, and the cap on everything one
/// `PutBlockPublicAccessConfiguration` stores.
///
/// DERIVED, not documented: a permitted range covers at least one of the
/// [`PORT_MIN`]..=[`PORT_MAX`] ports and the ranges may not overlap, so a list longer
/// than the port space necessarily repeats a port and can be refused by its length
/// alone — before any per-element work is done. The same number caps the entries of a
/// stored configuration as a whole (ranges, nested configurations and properties
/// entries), so one `Put` cannot make every later `GetBlockPublicAccessConfiguration`
/// clone and serialise an arbitrarily large body.
pub const MAX_PERMITTED_PUBLIC_RANGES: usize = (PORT_MAX - PORT_MIN + 1) as usize;
/// See [`MAX_PERMITTED_PUBLIC_RANGES`].
pub const MAX_CONFIGURATION_ENTRIES: usize = MAX_PERMITTED_PUBLIC_RANGES;

/// Width of the uppercase base-36 body of a minted `j-`/`s-` id.
const ID_BODY_WIDTH: usize = 13;

/// The prefix every cluster (job flow) id carries, as Amazon EMR mints it and as the
/// `DescribeCluster` example ids show it.
pub const CLUSTER_ID_PREFIX: &str = "j-";

/// The prefix every EMR Studio id carries (`Studio$StudioId`, whose example ids are of
/// the form `es-…`).
pub const STUDIO_ID_PREFIX: &str = "es-";

/// The largest `CreateSecurityConfiguration$SecurityConfiguration` document this
/// emulator accepts, in bytes.
///
/// CHOSEN, not documented: the model leaves that member an unconstrained `String`, but
/// an unbounded document would be parsed and then retained for the life of the process.
/// 256 KiB is two orders of magnitude above the longest bounded string EMR models
/// (`XmlString`, 10280 bytes) and far above any real security configuration, so a
/// faithful request is never refused by it. A document past it is refused BEFORE it is
/// parsed.
pub const MAX_SECURITY_CONFIGURATION_BYTES: usize = 256 * 1024;

/// How many security configurations this emulator stores for the account.
///
/// CHOSEN, not documented: EMR publishes a service quota on security configurations per
/// account and region but the model carries no value for it, so this stands in for it —
/// high enough that no faithful client reaches it, and a bound rather than unlimited
/// growth. Reaching it is refused with the operation's modeled client error, the way a
/// quota is.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 1_000;

/// How many persistent application user interfaces this emulator holds for the account.
///
/// CHOSEN, not documented, for the same reason [`MAX_STUDIOS`] is: every
/// `CreatePersistentAppUI` mints a record, so without a bound a caller could grow the
/// map without end. A create past it is refused — with the 400 `ValidationException`
/// that operation answers a rejected request with — rather than accepted.
pub const MAX_PERSISTENT_APP_UIS: usize = 1_000;

/// How many clusters this emulator holds for the account.
///
/// CHOSEN, not documented, for the same reason [`MAX_STUDIOS`] is: every `RunJobFlow`
/// inserts a [`ClusterRecord`] — with its steps, instance groups and instance fleets —
/// and a terminated cluster is still reported by `ListClusters`, so nothing removes one.
/// Without a bound a caller could grow the map, and the work every listing does walking
/// it, without end. A launch past it is refused, the way a quota is, and nothing is
/// stored.
pub const MAX_CLUSTERS: usize = 1_000;

/// Per-service state for com-amazonaws-emr-elasticmapreduce. A handler reaches it through [`service_state`].
///
/// `clusters` is keyed by cluster id, and a minted id is the zero-padded base-36
/// rendering of a monotonic sequence, so the map's key order IS creation order and
/// every listing is deterministic.
#[derive(Debug)]
pub struct ComAmazonawsEmrElasticmapreduceState {
    pub clusters: BTreeMap<String, ClusterRecord>,
    pub next_cluster_seq: u64,
    /// One counter for every step of every cluster, so a step id is unique
    /// service-wide rather than only within its own cluster.
    pub next_step_seq: u64,
    /// One counter for every instance fleet of every cluster, for the same reason.
    pub next_instance_fleet_seq: u64,
    /// One counter for every instance group of every cluster, for the same reason.
    pub next_instance_group_seq: u64,
    /// One counter for every Amazon EC2 instance of every cluster, for the same reason.
    pub next_cluster_instance_seq: u64,
    /// The persisted EMR Studios, keyed by studio id. `CreateStudio` inserts the
    /// `Studio` the request describes plus the id, ARN, URL and creation instant it
    /// mints, and every studio operation resolves that record: the shape is complete
    /// and nothing is derived from it, so it is held as the wire shape rather than
    /// projected (unlike [`ClusterRecord`], whose wire shapes ARE projections).
    /// Bounded by [`MAX_STUDIOS`].
    pub studios: BTreeMap<String, Studio>,
    pub next_studio_seq: u64,
    /// The persisted studio session mappings, keyed by
    /// [`session_mapping_key`] — one mapping per (studio, identity type, identity),
    /// which is the identity `GetStudioSessionMapping` addresses one by. Bounded per
    /// studio by [`MAX_SESSION_MAPPINGS_PER_STUDIO`].
    pub studio_session_mappings: BTreeMap<String, SessionMappingDetail>,
    /// The persisted notebook executions, keyed by execution id — minted from a
    /// monotonic sequence like every other id here, so the map's key order IS the order
    /// `StartNotebookExecution` recorded them in.
    ///
    /// `StartNotebookExecution` is what inserts one; `DescribeNotebookExecution`,
    /// `ListNotebookExecutions` and `StopNotebookExecution` all resolve these records.
    pub notebook_executions: BTreeMap<String, NotebookExecutionRecord>,
    pub next_notebook_execution_seq: u64,
    /// The ONE block public access configuration of this account and Region, which
    /// always exists: `PutBlockPublicAccessConfiguration` REPLACES it and
    /// `GetBlockPublicAccessConfiguration` reports it.
    pub block_public_access: BlockPublicAccessRecord,
    /// The persistent application user interfaces, keyed by the id
    /// `CreatePersistentAppUI` minted for each. `DescribePersistentAppUI` projects a
    /// record onto the `PersistentAppUI` wire shape
    /// ([`PersistentAppUIRecord::to_wire`]) and `GetPersistentAppUIPresignedURL`
    /// derives its URL and readiness from the same record. Bounded by
    /// [`MAX_PERSISTENT_APP_UIS`].
    pub persistent_app_uis: BTreeMap<String, PersistentAppUIRecord>,
    pub next_persistent_app_ui_seq: u64,
    /// Security configurations keyed by NAME: a security configuration "is stored in
    /// the service and can be specified when a cluster is created"
    /// (`CreateSecurityConfiguration` doc, Smithy model) by that name alone, so the
    /// name is its identity. Keying the map by it is what makes the name unique and
    /// what gives every listing a deterministic, name-ordered sequence.
    pub security_configurations: BTreeMap<String, SecurityConfigurationRecord>,
    /// The sequence every session id of this account is minted from
    /// ([`ComAmazonawsEmrElasticmapreduceState::mint_session_id`]); the sessions
    /// themselves hang off the cluster they were started on.
    pub next_session_seq: u64,
    /// The cluster state last ANNOUNCED on the event bus, per cluster id: what a
    /// transition is recognised against ([`Self::record_cluster_state_changes`]), so no
    /// state change is announced twice and none is missed. Bounded by the cluster map
    /// itself — an entry goes when its cluster does.
    announced_cluster_states: BTreeMap<String, String>,
    /// The cluster state changes waiting to be published, oldest first. Bounded in
    /// [`cluster_events`].
    pending_cluster_events: Vec<ClusterStateChange>,
    /// Whether a cluster clock pump holds this store's single claim
    /// ([`drive_cluster_clock`]).
    cluster_clock_pump_running: bool,
    /// The generation of that claim, so a pump releases its own claim and never a later
    /// pump's.
    cluster_clock_pump_generation: u64,
    /// The running pump's task handle, kept so the pump it replaces is aborted rather
    /// than merely forgotten.
    cluster_clock_pump: Option<::tokio::task::JoinHandle<()>>,
    /// The task each notebook execution's own run is performed by, per execution id
    /// ([`start_notebook_run`]), kept so a run can be ABANDONED rather than merely
    /// forgotten: a caller that stops an execution stops its run with it. An entry goes
    /// when the run answers or when it is abandoned, so this holds no more entries than
    /// there are executions ([`MAX_NOTEBOOK_EXECUTIONS`]).
    notebook_runs: BTreeMap<String, ::tokio::task::JoinHandle<()>>,
}

/// The account's default configuration is in place before any caller puts one, so the
/// state starts out holding it rather than an absent one — exactly what
/// `GetBlockPublicAccessConfiguration` reports for an account that has never called
/// `PutBlockPublicAccessConfiguration`. Its `CreationDateTime` is fixed when this store
/// is first reached and never moves again, so repeated reads report the same instant.
impl Default for ComAmazonawsEmrElasticmapreduceState {
    fn default() -> Self {
        Self {
            clusters: BTreeMap::new(),
            next_cluster_seq: 0,
            next_step_seq: 0,
            next_instance_fleet_seq: 0,
            next_instance_group_seq: 0,
            next_cluster_instance_seq: 0,
            studios: BTreeMap::new(),
            next_studio_seq: 0,
            studio_session_mappings: BTreeMap::new(),
            notebook_executions: BTreeMap::new(),
            next_notebook_execution_seq: 0,
            block_public_access: BlockPublicAccessRecord::account_default(SystemTime::now()),
            persistent_app_uis: BTreeMap::new(),
            next_persistent_app_ui_seq: 0,
            security_configurations: BTreeMap::new(),
            next_session_seq: 0,
            announced_cluster_states: BTreeMap::new(),
            pending_cluster_events: Vec::new(),
            cluster_clock_pump_running: false,
            cluster_clock_pump_generation: 0,
            cluster_clock_pump: None,
            notebook_runs: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// A fresh cluster id — [`CLUSTER_ID_PREFIX`] followed by uppercase alphanumerics —
    /// and the DNS name the master node of that cluster carries in `region`.
    ///
    /// The two are minted TOGETHER because both come out of this account's cluster
    /// sequence: the id is the sequence in base 36 ([`base36_id`]), and the master node's
    /// address is the same sequence laid out over the master network ([`master_dns_name`]),
    /// so one cluster's master node never holds another's address.
    pub fn mint_cluster(&mut self, region: &str) -> (String, String) {
        self.next_cluster_seq += 1;
        (
            format!("{CLUSTER_ID_PREFIX}{}", base36_id(self.next_cluster_seq)),
            master_dns_name(region, self.next_cluster_seq),
        )
    }

    /// A fresh step id: `s-` followed by uppercase alphanumerics.
    pub fn mint_step_id(&mut self) -> String {
        self.next_step_seq += 1;
        format!("s-{}", base36_id(self.next_step_seq))
    }

    /// A fresh instance fleet id: `if-` followed by uppercase alphanumerics.
    pub fn mint_instance_fleet_id(&mut self) -> String {
        self.next_instance_fleet_seq += 1;
        format!("if-{}", base36_id(self.next_instance_fleet_seq))
    }

    /// A fresh instance group id: `ig-` followed by uppercase alphanumerics.
    pub fn mint_instance_group_id(&mut self) -> String {
        self.next_instance_group_seq += 1;
        format!("ig-{}", base36_id(self.next_instance_group_seq))
    }

    /// A fresh id for one Amazon EC2 instance of a cluster
    /// ([`CLUSTER_INSTANCE_ID_PREFIX`] followed by uppercase alphanumerics), minted from
    /// the same monotonic, zero-padded sequence every other id in this service is, so no two
    /// instances of the account carry one id.
    pub fn mint_cluster_instance_id(&mut self) -> String {
        self.next_cluster_instance_seq += 1;
        format!(
            "{CLUSTER_INSTANCE_ID_PREFIX}{}",
            base36_id(self.next_cluster_instance_seq)
        )
    }

    /// A fresh studio id: [`STUDIO_ID_PREFIX`] followed by uppercase alphanumerics.
    pub fn mint_studio_id(&mut self) -> String {
        self.next_studio_seq += 1;
        format!("{STUDIO_ID_PREFIX}{}", base36_id(self.next_studio_seq))
    }

    /// A fresh persistent application user interface id: `p-` followed by uppercase
    /// alphanumerics, minted from the same monotonic, zero-padded sequence every other
    /// id in this service is ([`base36_id`]), so the map's key order is creation order.
    pub fn mint_persistent_app_ui_id(&mut self) -> String {
        self.next_persistent_app_ui_seq += 1;
        format!("p-{}", base36_id(self.next_persistent_app_ui_seq))
    }

    /// The persistent application user interface `id`, with every transition the clock
    /// already made due applied.
    pub fn persistent_app_ui_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut PersistentAppUIRecord> {
        let record = self.persistent_app_uis.get_mut(id)?;
        record.advance_to(now);
        Some(record)
    }

    /// A fresh notebook execution id: `ex-` followed by uppercase alphanumerics.
    pub fn mint_notebook_execution_id(&mut self) -> String {
        self.next_notebook_execution_seq += 1;
        format!("ex-{}", base36_id(self.next_notebook_execution_seq))
    }

    /// The notebook execution `id`, with the status the clock already made due applied.
    pub fn notebook_execution_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut NotebookExecutionRecord> {
        let record = self.notebook_executions.get_mut(id)?;
        record.advance_to(now);
        Some(record)
    }

    /// Whether the cluster `cluster_id` is a runtime role enabled cluster.
    ///
    /// Runtime roles are switched on by the security configuration the cluster was
    /// created with: its `AuthorizationConfiguration` carries an `IAMConfiguration`
    /// whose `EnableApplicationScopedIAMRole` is true (Amazon EMR Management Guide,
    /// "Runtime roles for Amazon EMR steps"). So the answer is COMPUTED from the
    /// configuration the cluster names and the document that configuration holds,
    /// rather than stored twice: a cluster naming no security configuration, one naming
    /// a configuration this account does not hold, and one whose document leaves the
    /// flag out are all not runtime role enabled.
    pub fn is_runtime_role_enabled_cluster(&self, cluster_id: &str) -> bool {
        let Some(cluster) = self.clusters.get(cluster_id) else {
            return false;
        };
        let Some(name) = cluster.security_configuration.as_deref() else {
            return false;
        };
        let Some(record) = self.security_configurations.get(name) else {
            return false;
        };
        ::serde_json::from_str::<::serde_json::Value>(&record.security_configuration)
            .ok()
            .and_then(|document| {
                document
                    .get("AuthorizationConfiguration")
                    .and_then(|section| section.get("IAMConfiguration"))
                    .and_then(|section| section.get("EnableApplicationScopedIAMRole"))
                    .and_then(::serde_json::Value::as_bool)
            })
            .unwrap_or(false)
    }

    /// Apply the clock to every notebook execution. Called by the listing, which
    /// filters on status.
    pub fn advance_all_notebook_executions(&mut self, now: SystemTime) {
        for record in self.notebook_executions.values_mut() {
            record.advance_to(now);
        }
    }

    /// How many session mappings the studio `studio_id` holds — the count
    /// [`MAX_SESSION_MAPPINGS_PER_STUDIO`] is taken against.
    pub fn session_mappings_of(&self, studio_id: &str) -> usize {
        let prefix = session_mapping_key_prefix(studio_id);
        self.studio_session_mappings
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .count()
    }

    /// The key the session mapping `identity` addresses on the studio `studio_id` is held
    /// under, or `None` when that studio holds no mapping for it.
    ///
    /// ONE identity, two members: "specify `IdentityId` or `IdentityName`"
    /// (`GetStudioSessionMapping$IdentityId` doc) addresses the SAME mapping either way,
    /// so a mapping created from a GUID is found by the identity's name and the other way
    /// round — the mapping carries both members once the identity resolved
    /// ([`resolve_identity`]). The key the identity itself produces is tried first, so a
    /// studio holding many mappings costs one lookup for the member the create was given.
    pub fn session_mapping_key_of(
        &self,
        studio_id: &str,
        identity_type: &str,
        identity: &str,
    ) -> Option<String> {
        let key = session_mapping_key(studio_id, identity_type, identity);
        if self.studio_session_mappings.contains_key(&key) {
            return Some(key);
        }
        let prefix = session_mapping_key_prefix(studio_id);
        self.studio_session_mappings
            .iter()
            .find(|(key, mapping)| {
                key.starts_with(&prefix)
                    && mapping.identity_type.as_deref() == Some(identity_type)
                    && identity_addresses(
                        mapping.identity_id.as_deref(),
                        mapping.identity_name.as_deref(),
                        identity,
                    )
            })
            .map(|(key, _)| key.clone())
    }

    /// The cluster `id`, with every transition the clock already made due applied.
    pub fn cluster_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut ClusterRecord> {
        let record = self.clusters.get_mut(id)?;
        record.advance_to(now);
        Some(record)
    }

    /// The tag set of the resource a tag operation's `ResourceId` names, scoped by the
    /// `ClusterId` the request carries, or why no resource of this account answers to it.
    ///
    /// Resolving the resource is the SAME for every tag operation — the pair of members
    /// says which resource the call is about, not what it does to it — so `AddTags` and
    /// `RemoveTags` reach their target through here:
    ///
    /// * a request carrying a `ClusterId` is about "the cluster that scopes the tag
    ///   operation" (`AddTags$ClusterId` / `RemoveTags$ClusterId` doc), so the
    ///   `ResourceId` is resolved against that cluster alone — the cluster itself, or a
    ///   session of it. `ClusterId` is `len 0..256`, so the empty string reaches a
    ///   handler, and it scopes nothing.
    /// * otherwise the id alone says which resource it names — "a cluster identifier or
    ///   an Amazon EMR Studio ID" — so it is resolved against the resources this service
    ///   holds a tag set for.
    pub fn tag_target_mut(
        &mut self,
        cluster_id: Option<&str>,
        resource_id: &str,
        now: SystemTime,
    ) -> Result<TagSetMut<'_>, TagTargetRefusal> {
        if let Some(cluster_id) = cluster_id.filter(|id| !id.is_empty()) {
            let Some(cluster) = self.clusters.get_mut(cluster_id) else {
                return Err(TagTargetRefusal::MalformedClusterId(
                    cluster_id.to_string(),
                ));
            };
            // Materialises the transitions the clock already made due, which were owed
            // at this instant whether or not anybody asked about a tag.
            cluster.advance_to(now);
            if cluster.id == resource_id {
                return Ok(TagSetMut::List(&mut cluster.tags));
            }
            return match cluster.session_mut(resource_id) {
                Some(session) => Ok(TagSetMut::Optional(&mut session.session.tags)),
                None => Err(TagTargetRefusal::NotValid(unknown_session_message(
                    resource_id,
                ))),
            };
        }

        if let Some(cluster) = self.clusters.get_mut(resource_id) {
            cluster.advance_to(now);
            return Ok(TagSetMut::List(&mut cluster.tags));
        }
        if let Some(studio) = self.studios.get_mut(resource_id) {
            return Ok(TagSetMut::Optional(&mut studio.tags));
        }

        // Nothing resolved: the refusal is the one the surface that owns an id of this
        // form answers, so a caller reads here the same sentence a read of the resource
        // it asked about answers.
        if resource_id.starts_with(STUDIO_ID_PREFIX) {
            return Err(TagTargetRefusal::Absent(unknown_studio_message()));
        }
        Err(TagTargetRefusal::MalformedClusterId(
            resource_id.to_string(),
        ))
    }

    /// Apply the clock to every cluster. Called by the listings, which filter on state.
    pub fn advance_all(&mut self, now: SystemTime) {
        for record in self.clusters.values_mut() {
            record.advance_to(now);
        }
    }
}

/// `seq` as `ID_BODY_WIDTH` uppercase base-36 digits, zero padded.
///
/// Fixed width and padded so the lexicographic order of the ids matches the order the
/// sequence minted them in — which is what makes the `BTreeMap` creation-ordered.
fn base36_id(seq: u64) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut body = [b'0'; ID_BODY_WIDTH];
    let mut remaining = seq;
    let mut at = ID_BODY_WIDTH;
    while remaining > 0 && at > 0 {
        at -= 1;
        body[at] = DIGITS[(remaining % 36) as usize];
        remaining /= 36;
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// The ARN of a cluster: `arn:aws:elasticmapreduce:<region>:<account>:cluster/<id>`.
pub fn cluster_arn(region: &str, cluster_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_VENDOR,
        region,
        &format!("{CLUSTER_RESOURCE_TYPE}/{cluster_id}"),
    )
}

/// The ARN of a studio: `arn:aws:elasticmapreduce:<region>:<account>:studio/<id>`.
pub fn studio_arn(region: &str, studio_id: &str) -> String {
    ::engine::emulator_arn(ARN_VENDOR, region, &format!("studio/{studio_id}"))
}

/// "The unique access URL of the Amazon EMR Studio" (`Studio$Url` doc): the studio's
/// own id under the regional EMR Studio domain, so the URL is derived from the studio
/// rather than invented per read.
pub fn studio_url(region: &str, studio_id: &str) -> String {
    format!("https://{studio_id}.emrstudio-prod.{region}.amazonaws.com")
}

/// The key one studio session mapping is held under: its studio, its identity type and
/// the identity itself.
///
/// `GetStudioSessionMapping` addresses a mapping by `StudioId` + `IdentityType` +
/// either `IdentityId` or `IdentityName` ("Specify `IdentityId` or `IdentityName`"),
/// so those three are its identity. The identity is case-folded because an IAM
/// Identity Center user or group name is not distinguished by case.
pub fn session_mapping_key(studio_id: &str, identity_type: &str, identity: &str) -> String {
    format!(
        "{}{identity_type}\u{1f}{}",
        session_mapping_key_prefix(studio_id),
        identity.to_lowercase()
    )
}

/// The identity a studio session mapping names, and which member named it.
///
/// "Either `IdentityName` or `IdentityId` must be specified, but not both."
/// (`CreateStudioSessionMapping$IdentityId` doc.) `None` means the request named
/// neither or both, which every mapping operation refuses.
pub fn session_mapping_identity(
    identity_id: Option<&String>,
    identity_name: Option<&String>,
) -> Option<(Option<String>, Option<String>, String)> {
    let identity_id = identity_id.filter(|value| !value.trim().is_empty());
    let identity_name = identity_name.filter(|value| !value.trim().is_empty());
    match (identity_id, identity_name) {
        (Some(id), None) => Some((Some(id.clone()), None, id.clone())),
        (None, Some(name)) => Some((None, Some(name.clone()), name.clone())),
        _ => None,
    }
}

/// The prefix every session-mapping key of `studio_id` begins with, so the mappings
/// that belong to one studio can be found (and dropped with it) without a second map
/// keyed by studio.
pub fn session_mapping_key_prefix(studio_id: &str) -> String {
    format!("{studio_id}\u{1f}")
}

/// The region this request is addressed to: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The IAM roles every cluster launch has to name, as AWS names each in the sentence
/// that refuses a launch carrying none of it.
///
/// Recorded behavior for each name: the EC2 role is named by the thing it IS ("also
/// called instance profile and Amazon EC2 role", `RunJobFlow$JobFlowRole` doc), the
/// service role by its member name. They are checked in the order a launch NEEDS them:
/// the instance profile is required and then resolved against IAM, and the service role
/// is required after that — the order Amazon EMR is recorded answering for a launch that
/// names neither.
pub mod required_cluster_role {
    /// `RunJobFlow$JobFlowRole`, the profile the cluster's Amazon EC2 instances assume.
    pub const INSTANCE_PROFILE: &str = "InstanceProfile";
    /// `RunJobFlow$ServiceRole`, the role "Amazon EMR assumes in order to access Amazon
    /// Web Services resources on your behalf" (`RunJobFlow$ServiceRole` doc).
    pub const SERVICE_ROLE: &str = "ServiceRole";
}

/// What AWS answers when a cluster launch names none of an IAM role it requires: the
/// role, named as AWS names it ([`required_cluster_role`]), then the fixed tail.
///
/// Recorded behavior for the text. Its one variable part is WHICH role is missing, taken
/// from the request — nothing of the request's own values is interpolated, because a
/// request that carried none of the role carried no value to state.
pub fn required_cluster_role_message(role: &str) -> String {
    format!("{role} is required for creating cluster.")
}

/// Whether a role member of a cluster launch names a role AT ALL.
///
/// A member the request omitted and the empty value its `@length(0..10280)` admits name
/// no role alike: "The default role is EMR_EC2_DefaultRole. In order to use the default
/// role, you must have already created it" (`RunJobFlow$JobFlowRole` doc) — naming no
/// role does not select that default.
pub fn names_a_cluster_role(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// What AWS answers when a cluster launch names an EC2 instance profile that does
/// not resolve: `Invalid InstanceProfile: <name>.`, built from the request — the value
/// the caller sent, so it is told which of its names resolved to nothing.
pub fn invalid_instance_profile_message(job_flow_role: &str) -> String {
    format!("Invalid InstanceProfile: {job_flow_role}.")
}

/// What AWS answers when a cluster launch names an EC2 instance profile that EXISTS and
/// that it cannot authorize: the fixed sentence and, as its one variable part, the ARN
/// IAM holds that profile under.
///
/// Recorded behavior for the text. It carries no trailing stop: the ARN ends the
/// sentence as AWS writes it.
pub fn unauthorized_instance_profile_message(instance_profile_arn: &str) -> String {
    format!("Failed to authorize instance profile {instance_profile_arn}")
}

/// The service principal the role an EC2 instance profile carries has to trust for the
/// cluster's instances to assume it.
///
/// "The Amazon EC2 instances of the cluster assume this role"
/// (`RunJobFlow$JobFlowRole` doc), and an instance assumes a role through its instance
/// profile, so the role's trust policy has to admit Amazon EC2.
const INSTANCE_PROFILE_PRINCIPAL: &str = "ec2.amazonaws.com";

/// Why a cluster launch's `JobFlowRole` is one Amazon EMR will not launch with, and
/// therefore WHICH refusal the launch is answered with.
///
/// The two are different error classes, so a client branching on the code learns which
/// of the two it has to fix: a value that names no instance profile of this account is a
/// malformed REQUEST, while a profile that EXISTS and cannot be authorized is an
/// authorization failure for a request that was otherwise well formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobFlowRoleRefusal {
    /// The value names no instance profile this account holds
    /// ([`invalid_instance_profile_message`]).
    Invalid(String),
    /// The instance profile exists and could not be authorized, with the sentence that
    /// states it ([`unauthorized_instance_profile_message`]).
    Unauthorized(String),
}

impl JobFlowRoleRefusal {
    /// The sentence this refusal is stated in.
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(message) | Self::Unauthorized(message) => message.clone(),
        }
    }
}

/// The instance profile NAME a `JobFlowRole` value denotes, or `None` when the
/// value denotes no instance profile of this account at all.
///
/// `RunJobFlow$JobFlowRole` is "also called instance profile and Amazon EC2 role",
/// and an instance profile is named either by its friendly name or by its ARN, so an
/// ARN is read through the tree's shared IAM-ARN reading
/// ([`service_common::iam_role::instance_profile_name`]): a value that is ARN-shaped
/// but names another vendor, another IAM entity type, another account or a Region an
/// IAM ARN does not carry resolves to nothing rather than to an unrelated name. The
/// empty value, which `@length(0..10280)` admits, names nothing either.
fn instance_profile_name(job_flow_role: &str) -> Option<&str> {
    if !job_flow_role.starts_with("arn:") {
        return (!job_flow_role.is_empty()).then_some(job_flow_role);
    }
    ::service_common::iam_role::instance_profile_name(job_flow_role)
}

/// Amazon EMR resolves a cluster's `JobFlowRole` to a real EC2 instance profile
/// when the cluster is LAUNCHED, and refuses the launch — writing no cluster — when the
/// value names no instance profile of this account, or when the profile it names cannot
/// be authorized.
///
/// `Some(refusal)` is that refusal, classed and worded as AWS words it
/// ([`JobFlowRoleRefusal`]). `None` means the launch may proceed.
///
/// A profile is AUTHORIZED when it carries a role whose trust policy admits
/// [`INSTANCE_PROFILE_PRINCIPAL`] — the one reading of a trust policy this tree has
/// ([`service_common::iam_trust_policy::admits_service_principal`]) — because that is
/// what lets the cluster's Amazon EC2 instances assume it. A profile carrying none, or
/// one whose role trusts another principal, exists and cannot be authorized, which is a
/// different refusal from a profile that does not exist at all.
///
/// IAM is reached the way any emulated service reaches another: through the ONE
/// in-process IAM client every service that resolves a caller-supplied IAM entity
/// shares ([`service_common::iam_client::iam_client`]), so this crate names no other
/// service crate. `None` therefore also covers the case where this emulator has no
/// IAM to ask (a router with only this service mounted, say): nothing is then known
/// about the profile, and a refusal has to be grounded in what IAM answered, never
/// assumed — which is also why a trust document IAM holds and this cannot read refuses
/// nothing.
pub async fn instance_profile_violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_flow_role: &str,
) -> Option<JobFlowRoleRefusal> {
    let invalid = || {
        Some(JobFlowRoleRefusal::Invalid(
            invalid_instance_profile_message(job_flow_role),
        ))
    };
    let Some(name) = instance_profile_name(job_flow_role) else {
        return invalid();
    };
    let answer = ::service_common::iam_client::iam_client(ctx, region)
        .get_instance_profile()
        .instance_profile_name(name)
        .send()
        .await;
    match answer {
        Ok(held) => {
            let profile = held.instance_profile()?;
            // A role whose trust document is absent or unreadable is no evidence either
            // way, so it withholds the refusal rather than inventing one; a profile
            // carrying NO role has nothing for an instance to assume, which is the
            // refusal itself.
            let trusts_instances = |role: &::aws_sdk_iam::types::Role| -> Option<bool> {
                ::service_common::iam_trust_policy::admits_service_principal(
                    role.assume_role_policy_document()?,
                    INSTANCE_PROFILE_PRINCIPAL,
                )
            };
            if profile
                .roles()
                .iter()
                .map(trusts_instances)
                .any(|verdict| verdict != Some(false))
            {
                return None;
            }
            Some(JobFlowRoleRefusal::Unauthorized(
                unauthorized_instance_profile_message(profile.arn()),
            ))
        }
        // IAM says this account holds no such instance profile: the refusal is
        // grounded in what IAM answered.
        Err(error) if instance_profile_is_absent(&error) => invalid(),
        // Any other answer is this emulator failing to ask, not IAM reporting an
        // unresolvable profile, so it refuses nothing — and it is worth a warning,
        // because a deployment with no IAM to ask is one where this gate does not
        // run.
        Err(error) => {
            ::tracing::warn!(
                %job_flow_role,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the job flow role could not be resolved against IAM"
            );
            None
        }
    }
}

/// The account's block public access configuration, with the metadata EMR keeps beside
/// it: "Properties that describe the Amazon Web Services principal that created the
/// `BlockPublicAccessConfiguration` using the `PutBlockPublicAccessConfiguration` action
/// as well as the date and time that the configuration was created. Each time a
/// configuration for block public access is updated, Amazon EMR updates this metadata."
/// (`BlockPublicAccessConfigurationMetadata` doc, Smithy model.)
///
/// The configuration is held as the wire shape the caller supplied: a `Put` replaces the
/// whole configuration, so what a later `Get` reports is what was put, and every port
/// bound keeps the value it was given. The one member canonicalized on the way in is the
/// exception list, which an account that has put a configuration always HAS: a `Put` that
/// omits it permits no public port, exactly as an explicit empty list does, so
/// [`BlockPublicAccessRecord::put`] stores the empty list for both and the two cases read
/// back as one representation.
#[derive(Debug, Clone)]
pub struct BlockPublicAccessRecord {
    pub configuration: BlockPublicAccessConfiguration,
    pub creation_date_time: SystemTime,
    pub created_by_arn: String,
}

impl BlockPublicAccessRecord {
    /// The configuration an account has before anybody puts one: block public access on,
    /// with Port 22 permitted. See [`DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES`] and
    /// [`DEFAULT_PERMITTED_PUBLIC_PORT`].
    ///
    /// `CreatedByArn` is "the Amazon Resource Name that created or last modified the
    /// configuration" (`BlockPublicAccessConfigurationMetadata$CreatedByArn` doc), and NO
    /// principal has created or last modified this one: the member is modelled required,
    /// so it is reported — EMPTY — rather than filled with an ARN naming a principal that
    /// never acted. A caller reading it therefore learns whether the configuration has
    /// ever been written: empty while it is still the service default, the writing
    /// principal once a `Put` has written it ([`Self::put`]).
    pub fn account_default(at: SystemTime) -> Self {
        Self {
            configuration: BlockPublicAccessConfiguration {
                block_public_security_group_rules: DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES,
                permitted_public_security_group_rule_ranges: Some(vec![PortRange {
                    min_range: DEFAULT_PERMITTED_PUBLIC_PORT,
                    max_range: Some(DEFAULT_PERMITTED_PUBLIC_PORT),
                }]),
                classification: None,
                configurations: None,
                properties: None,
            },
            creation_date_time: at,
            created_by_arn: String::new(),
        }
    }

    /// What `PutBlockPublicAccessConfiguration` stored, under the principal that called
    /// it.
    ///
    /// The configuration REPLACES whatever was held, and the exception list it leaves out
    /// is the empty list rather than an unknown: "Specifies ports and port ranges that are
    /// permitted to have security group rules that allow inbound traffic from all public
    /// sources" (`BlockPublicAccessConfiguration$PermittedPublicSecurityGroupRuleRanges`
    /// doc) — a configuration that names none permits none.
    pub fn put(
        mut configuration: BlockPublicAccessConfiguration,
        created_by_arn: String,
        at: SystemTime,
    ) -> Self {
        configuration
            .permitted_public_security_group_rule_ranges
            .get_or_insert_with(Vec::new);
        Self {
            configuration,
            creation_date_time: at,
            created_by_arn,
        }
    }

    /// The wire `BlockPublicAccessConfigurationMetadata` for this record.
    pub fn to_metadata(&self) -> BlockPublicAccessConfigurationMetadata {
        BlockPublicAccessConfigurationMetadata {
            creation_date_time: timestamp(self.creation_date_time),
            created_by_arn: self.created_by_arn.clone(),
        }
    }
}

/// Whether an IAM `GetInstanceProfile` failure is IAM reporting that the profile
/// does not exist (`NoSuchEntity`), as opposed to the call not reaching IAM.
///
/// The call this classifies is made over the emulator's own IN-PROCESS transport and
/// reaches nothing outside this process: the one client it is made with is built by
/// [`::service_common::iam_client::iam_client`] from the framework's operation callback
/// (`smithy_server_runtime::OperationContext::callback`), which dispatches the request to
/// the IAM service mounted in this same emulator — no socket is opened, no name is
/// resolved, the SDK's default endpoint and credential provider chains are never consulted,
/// and the host's own credentials are never read. Its one caller is
/// [`instance_profile_violation`] above, which is where that client is constructed; "the
/// call did not reach IAM" therefore means this emulator has no IAM mounted, not that a
/// remote endpoint was unreachable.
fn instance_profile_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_instance_profile::GetInstanceProfileError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error.as_service_error().is_some_and(
        ::aws_sdk_iam::operation::get_instance_profile::GetInstanceProfileError::is_no_such_entity_exception,
    )
}

/// The IAM principal this request was signed by: what a configuration it writes records
/// as having created it.
///
/// The emulator models no identity store beyond the access key id a request is signed
/// with, which is exactly what names the caller here; it is reduced to the characters an
/// access key id can hold, and an unsigned request is the account's `default` user.
pub fn caller_arn(parts: &::http::request::Parts) -> String {
    let filtered: String = ::engine::extract_akid(&parts.headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let user = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{user}"))
}

// ---------------------------------------------------------------------------
// The Amazon EMR release catalog
// ---------------------------------------------------------------------------
//
// `DescribeReleaseLabel` reports "the releases available [in] the Region where the API
// request is run, and the available applications for a specific Amazon EMR release
// label" (its own doc), so the catalog describes the SERVICE rather than anything a
// caller created: it is a table here, beside the per-caller state, and reading it
// changes nothing a later caller sees.
//
// Source for the labels, the application versions and which release first (or last)
// bundles an application: the Amazon EMR Release Guide's release-version and
// "Application versions" tables, which also give each application's name.
//
// An application name is an IDENTIFIER, not prose: the catalog carries the one
// spelling Amazon EMR publishes for each application — the same spelling
// `RunJobFlow`'s `Application$Name` accepts and `DescribeCluster` reports back — and
// reports it verbatim. That spelling is per application rather than a transform of a
// lower-cased name (`HBase`, `ZooKeeper`, `JupyterHub` and `JupyterEnterpriseGateway`
// capitalize inside the word), so it can only come from the table below. The
// lower-case spellings `SimplifiedApplication$Name` and
// `ReleaseLabelFilter$Application` use in their doc EXAMPLES are a caller's input
// form, which is why `release_bundles_application` compares names without regard to
// case; they are not what the catalog reports.

impl InstancesRecord {
    /// Derive the summary from the `Instances` member of a `RunJobFlow` request.
    pub fn from_config(config: &JobFlowInstancesConfig) -> Self {
        let fleets = config
            .instance_fleets
            .as_ref()
            .is_some_and(|fleets| !fleets.is_empty());
        let placement = config.placement.as_ref();
        Self {
            instance_collection_type: if fleets {
                instance_collection_type::INSTANCE_FLEET.to_string()
            } else {
                instance_collection_type::INSTANCE_GROUP.to_string()
            },
            master_instance_type: config.master_instance_type.clone(),
            slave_instance_type: config.slave_instance_type.clone(),
            instance_count: config.instance_count,
            ec2_key_name: config.ec2_key_name.clone(),
            ec2_subnet_id: launched_in(
                config.ec2_subnet_id.as_deref(),
                config.ec2_subnet_ids.as_deref().unwrap_or_default(),
            ),
            requested_ec2_subnet_ids: config.ec2_subnet_ids.clone().unwrap_or_default(),
            ec2_availability_zone: launched_in(
                placement.and_then(|p| p.availability_zone.as_deref()),
                placement
                    .and_then(|p| p.availability_zones.as_deref())
                    .unwrap_or_default(),
            ),
            requested_ec2_availability_zones: placement
                .and_then(|p| p.availability_zones.clone())
                .unwrap_or_default(),
            emr_managed_master_security_group: config.emr_managed_master_security_group.clone(),
            emr_managed_slave_security_group: config.emr_managed_slave_security_group.clone(),
            service_access_security_group: config.service_access_security_group.clone(),
            additional_master_security_groups: config
                .additional_master_security_groups
                .clone()
                .unwrap_or_default(),
            additional_slave_security_groups: config
                .additional_slave_security_groups
                .clone()
                .unwrap_or_default(),
            // "Specifies whether the cluster should remain available after completing
            // all steps. Defaults to false." (JobFlowInstancesConfig doc.)
            keep_job_flow_alive_when_no_steps: config
                .keep_job_flow_alive_when_no_steps
                .unwrap_or(false),
            termination_protected: config.termination_protected.unwrap_or(false),
            unhealthy_node_replacement: config
                .unhealthy_node_replacement
                .unwrap_or(DEFAULT_UNHEALTHY_NODE_REPLACEMENT),
        }
    }
}

/// How many primary nodes a cluster's requested instance configuration provisions.
///
/// A launch states its primary nodes in one of the two forms `RunJobFlow` takes: instance
/// GROUPS, where "valid values are MASTER, CORE or TASK"
/// (`InstanceGroupConfig$InstanceRole` doc) and the `MASTER` groups' `InstanceCount`s are
/// the primaries, or instance FLEETS, where the `MASTER` fleet's target capacity is — "the
/// target capacity of On-Demand units for the instance fleet" plus the Spot equivalent
/// (`InstanceFleetConfig$TargetOnDemandCapacity` / `$TargetSpotCapacity` docs). Both
/// vocabularies spell the primary role `MASTER` (`InstanceRoleType`, `InstanceFleetType`),
/// so one reading serves both.
///
/// The counts are the ones the cluster is actually created with, which for a launch stating
/// its instances the uniform way are the groups that form implies
/// ([`implied_instance_group_configs`]) — one primary.
///
/// Every count comes straight off the request, which bounds none of them (`InstanceCount`
/// and the two target capacities are plain integers), so the arithmetic is TOTAL: each
/// count widens to `i64` and every accumulation saturates, and a request naming absurd
/// capacities is answered rather than overflowing.
pub fn primary_node_count(groups: &[InstanceGroupConfig], fleets: &[InstanceFleetConfig]) -> i64 {
    let in_groups = groups
        .iter()
        .filter(|group| group.instance_role == instance_group_type::MASTER)
        .fold(0i64, |total, group| {
            total.saturating_add(i64::from(group.instance_count.max(0)))
        });
    let in_fleets = fleets
        .iter()
        .filter(|fleet| fleet.instance_fleet_type == instance_group_type::MASTER)
        .fold(0i64, |total, fleet| {
            total
                .saturating_add(i64::from(fleet.target_on_demand_capacity.unwrap_or(0).max(0)))
                .saturating_add(i64::from(fleet.target_spot_capacity.unwrap_or(0).max(0)))
        });
    in_groups.saturating_add(in_fleets)
}

/// The fewest primary nodes that make a cluster highly available, which is what forces its
/// termination protection on ([`forces_termination_protection`]).
pub const MIN_HIGHLY_AVAILABLE_PRIMARY_NODES: i64 = 2;

/// Whether a cluster with `primary_nodes` primaries has termination protection forced on.
///
/// A cluster with more than one primary node is a highly available cluster, and Amazon EMR
/// locks one against a shutdown it did not mean: the launch turns termination protection on
/// whatever `JobFlowInstancesConfig$TerminationProtected` asked for, so `DescribeCluster`
/// reports `TerminationProtected` true and `TerminateJobFlows` on it is refused until a
/// `SetTerminationProtection` with `false` clears the lock — which is the one way to clear
/// it, exactly as it is for a cluster the launch asked protection for.
pub fn forces_termination_protection(primary_nodes: i64) -> bool {
    primary_nodes >= MIN_HIGHLY_AVAILABLE_PRIMARY_NODES
}

/// A cluster's state, its reason, and the instants of its lifecycle.
#[derive(Debug, Clone)]
pub struct ClusterStatusRecord {
    pub state: String,
    pub state_change_reason: Option<StateChangeReasonRecord>,
    pub creation_date_time: SystemTime,
    pub ready_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the `STARTING` -> `BOOTSTRAPPING` transition the clock owes comes due: the
    /// instant the cluster's Amazon EC2 capacity has been provisioned and its bootstrap
    /// actions can run.
    pub bootstrapping_due_at: Option<SystemTime>,
    /// When the `BOOTSTRAPPING` -> ready transition the clock owes comes due — the instant
    /// the cluster can run steps, which is also what a queued step waits for.
    pub ready_due_at: Option<SystemTime>,
    /// When the `TERMINATING` -> `TERMINATED` transition the clock owes comes due.
    pub terminated_due_at: Option<SystemTime>,
}

/// A `(code, message)` state-change reason.
#[derive(Debug, Clone)]
pub struct StateChangeReasonRecord {
    pub code: String,
    pub message: Option<String>,
}

/// One persisted cluster (job flow), built from the `RunJobFlow` request.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub release_label: Option<String>,
    pub requested_ami_version: Option<String>,
    pub log_uri: Option<String>,
    pub log_encryption_kms_key_id: Option<String>,
    pub service_role: Option<String>,
    pub job_flow_role: Option<String>,
    pub auto_scaling_role: Option<String>,
    /// The scale-down behaviour this cluster runs under: the one its launch asked for, or
    /// the one Amazon EMR applies to a launch that asked for none
    /// ([`effective_scale_down_behavior`]). A described cluster always states it.
    pub scale_down_behavior: String,
    pub custom_ami_id: Option<String>,
    pub security_configuration: Option<String>,
    pub os_release_label: Option<String>,
    pub repo_upgrade_on_boot: Option<String>,
    pub visible_to_all_users: bool,
    pub auto_terminate: bool,
    pub step_concurrency_level: i32,
    pub ebs_root_volume_size: Option<i32>,
    pub ebs_root_volume_iops: Option<i32>,
    pub ebs_root_volume_throughput: Option<i32>,
    pub extended_support: Option<bool>,
    pub session_enabled: Option<bool>,
    pub applications: Vec<Application>,
    pub configurations: Vec<Configuration>,
    pub bootstrap_actions: Vec<BootstrapActionConfig>,
    pub placement_groups: Vec<PlacementGroupConfig>,
    pub kerberos_attributes: Option<KerberosAttributes>,
    pub monitoring_configuration: Option<MonitoringConfiguration>,
    pub instances: InstancesRecord,
    /// The managed scaling policy attached to this cluster, by `RunJobFlow` or by
    /// `PutManagedScalingPolicy`, and `None` once `RemoveManagedScalingPolicy` has taken
    /// it off. `GetManagedScalingPolicy` reports exactly this.
    pub managed_scaling_policy: Option<ManagedScalingPolicy>,
    /// The auto-termination policy attached to this cluster, by `RunJobFlow` or by
    /// `PutAutoTerminationPolicy`, and `None` once `RemoveAutoTerminationPolicy` has
    /// taken it off. `GetAutoTerminationPolicy` reports exactly this.
    ///
    /// The OPTION is the attachment and the structure inside it is the policy, so a
    /// policy that carries no `IdleTimeout` is still an attached policy and stays
    /// distinguishable from no policy at all: whatever a request submitted is what a
    /// later read reports.
    pub auto_termination_policy: Option<AutoTerminationPolicy>,
    pub tags: Vec<Tag>,
    pub status: ClusterStatusRecord,
    /// Steps in submission order. `ListSteps` reverses this.
    pub steps: Vec<StepRecord>,
    /// The instance fleets this cluster carries, in the order they were added: the
    /// ones `RunJobFlow` was given, then every `AddInstanceFleet`.
    pub fleets: Vec<InstanceFleetRecord>,
    /// The instance groups this cluster carries, in the order they were created: the
    /// ones `RunJobFlow` was given (or the ones its `MasterInstanceType` /
    /// `SlaveInstanceType` / `InstanceCount` imply — see
    /// [`implied_instance_group_configs`]), and then every `AddInstanceGroups`, which is
    /// still a scaffold stub under its own issue. `ListInstanceGroups` projects these.
    pub groups: Vec<InstanceGroupRecord>,
    /// The DNS name of this cluster's master node, decided when the cluster was launched
    /// ([`master_dns_name`]) and reported once the clock has brought that node up
    /// ([`ClusterRecord::master_public_dns_name`]).
    pub master_dns_name: String,
    /// The Spark Connect sessions this cluster carries, in the order `StartSession`
    /// started them ([`SessionRecord`], [`ClusterRecord::start_session`]).
    ///
    /// `GetSession` reports one of these records and `ListSessions` pages over them;
    /// each carries the state the clock still owes it beside the wire shape, the way a
    /// notebook execution does.
    pub sessions: Vec<SessionRecord>,
    /// The Amazon EC2 instances this cluster's collections were provisioned from, in launch
    /// order: one record per instance Amazon EC2 minted for it
    /// ([`ClusterInstanceRecord`]), which is what `ListInstances` reports. A cluster whose
    /// instances Amazon EC2 did not launch holds none, so nothing is reported for it.
    pub ec2_instances: Vec<ClusterInstanceRecord>,
}

impl ClusterRecord {
    /// Materialise every transition the clock already made due, and nothing else.
    ///
    /// Every span below was owed at the same instant whether or not anybody called, so a
    /// reporting operation may apply them: a cluster that was `STARTING` reaches
    /// `BOOTSTRAPPING` once its capacity is up and then `WAITING` (it is to stay up after
    /// its steps) or `RUNNING` once its bootstrap actions have run, a queued step of a
    /// cluster that is ready runs and then completes, and one the caller terminated
    /// reaches `TERMINATED`.
    pub fn advance_to(&mut self, now: SystemTime) {
        for session in &mut self.sessions {
            session.advance_to(now);
        }
        for instance in &mut self.ec2_instances {
            instance.advance_to(now);
        }
        for fleet in &mut self.fleets {
            fleet.advance_to(now);
        }
        for group in &mut self.groups {
            group.advance_to(now);
        }
        for step in &mut self.steps {
            step.advance_to(now);
        }
        if self.status.state == cluster_state::TERMINATING {
            if let Some(due) = self.status.terminated_due_at {
                if now >= due {
                    self.status.state = cluster_state::TERMINATED.to_string();
                    self.status.end_date_time = Some(due);
                    self.status.terminated_due_at = None;
                    // A group or a fleet outlives its cluster by nothing: once the
                    // cluster is gone its collections have ended, for the modelled
                    // reason `CLUSTER_TERMINATED`.
                    for group in &mut self.groups {
                        group.end_at(due);
                    }
                    for fleet in &mut self.fleets {
                        fleet.end_at(due);
                    }
                    // Nor does one of its instances: "all Amazon EC2 instances have been
                    // terminated" is what a terminated collection means, so every instance
                    // of this cluster ended at that same instant.
                    for instance in &mut self.ec2_instances {
                        instance.end_at(due);
                    }
                }
            }
            return;
        }
        // The two phases of coming up, applied in order, so a read that comes after both
        // were due reports the state the clock actually reached rather than the next one
        // along.
        if self.status.state == cluster_state::STARTING {
            if let Some(due) = self.status.bootstrapping_due_at {
                if now >= due {
                    self.status.state = cluster_state::BOOTSTRAPPING.to_string();
                    self.status.bootstrapping_due_at = None;
                }
            }
        }
        if self.status.state == cluster_state::BOOTSTRAPPING {
            if let Some(due) = self.status.ready_due_at {
                if now >= due {
                    self.status.state = if self.instances.keep_job_flow_alive_when_no_steps {
                        cluster_state::WAITING.to_string()
                    } else {
                        cluster_state::RUNNING.to_string()
                    };
                    self.status.ready_date_time = Some(due);
                    self.status.ready_due_at = None;
                }
            }
        }
        self.advance_steps(now);
        self.stop_once_every_step_has_run(now);
    }

    /// The instant the shutdown a cluster that does not stay up is owed comes due, or
    /// `None` for a cluster that is owed none.
    ///
    /// "RunJobFlow creates and starts running a new cluster. The cluster runs the steps
    /// specified. After the steps complete, the cluster stops … If the
    /// `JobFlowInstancesConfig` `KeepJobFlowAliveWhenNoSteps` parameter is set to TRUE, the
    /// cluster transitions to the `WAITING` state rather than shutting down after the steps
    /// have completed" (`RunJobFlow` doc) — which is exactly what `Cluster$AutoTerminate`
    /// reports: "specifies whether the cluster should terminate after completing all steps".
    /// So a cluster that did not ask to stay up shuts itself down once its queue holds no
    /// step still to run: the step list is "completed" when every step of it has ended, and
    /// a queue that is EMPTY has completed too — which is the whole of a transient cluster
    /// launched with no steps at all, the launch `KeepJobFlowAliveWhenNoSteps` is documented
    /// against ("specifies whether the cluster should remain available after completing all
    /// steps", default false).
    ///
    /// The shutdown is owed from the instant the queue finished — the last step's end, or
    /// the instant the cluster became ready to run steps when it has none — so the cluster
    /// reports the same timeline however often it is read. A cluster still running a step is
    /// owed nothing yet, which is also what makes a step submitted to a transient cluster
    /// hold its shutdown off until that step has run.
    pub(crate) fn auto_terminate_due_at(&self) -> Option<SystemTime> {
        if !self.auto_terminate || self.status.state != cluster_state::RUNNING {
            return None;
        }
        // A cluster that has just become ready has been ready for no time at all, so the
        // shutdown is owed from one span after it rather than from the instant itself: a
        // caller reading straight after the cluster came up still observes it `RUNNING`,
        // the state a cluster that is up and has work to take reports. A cluster whose
        // queue ran holds the shutdown off until its last step ended, which is later than
        // that instant for any step that really ran.
        let mut finished = self
            .status
            .ready_date_time
            .unwrap_or(self.status.creation_date_time)
            + TRANSITION_SPAN;
        for step in &self.steps {
            let ended = step.end_date_time.filter(|_| step.is_terminal())?;
            finished = finished.max(ended);
        }
        Some(finished)
    }

    /// Materialise the shutdown [`Self::auto_terminate_due_at`] says this cluster is owed,
    /// once that instant has passed.
    fn stop_once_every_step_has_run(&mut self, now: SystemTime) {
        let Some(last_end) = self.auto_terminate_due_at() else {
            return;
        };
        if now < last_end {
            return;
        }
        self.status.state = cluster_state::TERMINATING.to_string();
        // Every step of the queue has run: the cluster reports the modelled code for the
        // shutdown that brings (`ClusterStateChangeReasonCode`), and the message Amazon EMR
        // words it with — which says whether those steps all succeeded.
        let every_step_completed = self
            .steps
            .iter()
            .all(|step| step.state == step_state::COMPLETED);
        self.status.state_change_reason = Some(StateChangeReasonRecord {
            code: cluster_state_change_reason_code::ALL_STEPS_COMPLETED.to_string(),
            message: Some(
                if every_step_completed {
                    CLUSTER_STEPS_COMPLETED_REASON
                } else {
                    CLUSTER_STEPS_COMPLETED_WITH_ERRORS_REASON
                }
                .to_string(),
            ),
        });
        self.status.terminated_due_at = Some(last_end + TRANSITION_SPAN);
    }

    /// The instant this cluster's steps may start running from, or `None` for a cluster
    /// whose steps never will.
    ///
    /// A cluster runs its steps once it is up: "If the `JobFlowInstancesConfig`
    /// `KeepJobFlowAliveWhenNoSteps` parameter is set to TRUE, the cluster transitions to
    /// the `WAITING` state rather than shutting down after the steps have completed"
    /// (`RunJobFlow` doc), so a cluster that is ready is one of `RUNNING`/`WAITING` and the
    /// instant it became ready is when its queue started moving. A cluster still coming up
    /// has that instant in the FUTURE — its queued steps stay `PENDING` until then, which
    /// is why the due instant is answered rather than nothing — and one that is shutting
    /// down or gone runs nothing more.
    fn steps_runnable_from(&self) -> Option<SystemTime> {
        match self.status.state.as_str() {
            cluster_state::RUNNING | cluster_state::WAITING => self
                .status
                .ready_date_time
                .or(self.status.ready_due_at)
                .or(Some(self.status.creation_date_time)),
            cluster_state::STARTING | cluster_state::BOOTSTRAPPING => self.status.ready_due_at,
            _ => None,
        }
    }

    /// Run this cluster's step queue up to `now`: start every step whose turn has come and
    /// complete every running step whose span has passed.
    ///
    /// The schedule is a function of what is persisted — the queue in submission order,
    /// each step's creation instant, the instant the cluster became ready and its
    /// `StepConcurrencyLevel` — and never of when somebody looked: a step's start is the
    /// first instant at which one of the cluster's concurrency slots was free and the step
    /// had been submitted, and its end is one [`STEP_RUN_SPAN`] later. Both instants are
    /// stored once they are reached ([`StepRecord::start_date_time`],
    /// [`StepRecord::end_date_time`]), so a later read re-derives the same timeline.
    ///
    /// "Specifies the number of steps that can be executed concurrently"
    /// (`RunJobFlow$StepConcurrencyLevel` doc): that many steps run at once and the rest
    /// wait, so a cluster at the default level of one runs its queue strictly in order.
    fn advance_steps(&mut self, now: SystemTime) {
        let Some(live_at) = self.steps_runnable_from() else {
            return;
        };
        // The slot count is read back under the ceiling the band allows
        // ([`MAX_STEP_CONCURRENCY_LEVEL`]), which every entry point already refuses a level
        // outside ([`step_concurrency_level_violation`]): a level this service should never
        // have persisted still cannot make a read allocate without bound.
        let ceiling = usize::try_from(MAX_STEP_CONCURRENCY_LEVEL).unwrap_or(1);
        let slots = usize::try_from(self.step_concurrency_level)
            .unwrap_or(1)
            .clamp(1, ceiling);
        // The instant each concurrency slot next falls free. A cluster that is not ready
        // yet has every slot falling free when it becomes ready.
        let mut free_at: Vec<SystemTime> = vec![live_at; slots];
        for step in &mut self.steps {
            // A step the queue dropped without running it — cancelled before its turn
            // came — occupied no slot, so it does not hold one up.
            if step.start_date_time.is_none() && step.will_not_run() {
                continue;
            }
            let slot = free_at
                .iter()
                .enumerate()
                .min_by_key(|(_, free)| **free)
                .map_or(0, |(index, _)| index);
            let start = step
                .start_date_time
                .unwrap_or_else(|| free_at[slot].max(step.creation_date_time));
            // A step that was stopped gives its slot back when the stop comes due; one
            // running to completion holds it for its whole span.
            let end = step
                .end_date_time
                .or(step.stop_due_at)
                .unwrap_or(start + STEP_RUN_SPAN);
            free_at[slot] = end;
            step.run_between(start, end, now);
        }
    }

    /// Whether this cluster's termination protection has a shutdown to refuse: the lock
    /// "prevent[s] the Amazon EC2 instances from being terminated by an API call or user
    /// intervention" (`Cluster$TerminationProtected` doc), so it holds only while the
    /// cluster still has instances to lose — a cluster that is already shutting down or
    /// gone has nothing left for the lock to protect, which is why a repeated shutdown of
    /// one stays a no-op rather than becoming a refusal.
    pub fn termination_protection_holds(&self) -> bool {
        self.instances.termination_protected
            && !self.is_terminal()
            && self.status.state != cluster_state::TERMINATING
    }

    /// Start the `TERMINATING` -> `TERMINATED` span, recording why.
    ///
    /// Answers `false` without touching the cluster when its termination protection holds
    /// ([`Self::termination_protection_holds`]) — a shutdown request is refused for that
    /// before it reaches here — and when the cluster is already shutting down or gone,
    /// which makes a repeated `TerminateJobFlows` idempotent.
    pub fn begin_termination(&mut self, now: SystemTime) -> bool {
        if self.termination_protection_holds() || self.is_terminal() {
            return false;
        }
        if self.status.state == cluster_state::TERMINATING {
            return false;
        }
        self.status.state = cluster_state::TERMINATING.to_string();
        self.status.state_change_reason = Some(StateChangeReasonRecord {
            code: cluster_state_change_reason_code::USER_REQUEST.to_string(),
            message: Some("Terminated by user request".to_string()),
        });
        self.status.bootstrapping_due_at = None;
        self.status.ready_due_at = None;
        let terminated_at = now + TRANSITION_SPAN;
        self.status.terminated_due_at = Some(terminated_at);
        // The cluster takes its step queue with it: a step still to run on it never will,
        // and one running on it is cut short, both when the shutdown completes. "After the
        // steps complete, the cluster stops" (`RunJobFlow` doc) is the other way round —
        // here the cluster stops first, so no step of it stays `PENDING` for a cluster
        // that no longer exists.
        for step in &mut self.steps {
            step.stop_with_cluster(terminated_at);
        }
        true
    }

    /// Whether this cluster has reached a state it never leaves.
    pub fn is_terminal(&self) -> bool {
        self.status.state == cluster_state::TERMINATED
            || self.status.state == cluster_state::TERMINATED_WITH_ERRORS
    }

    /// The step `step_id` of this cluster, if it has one.
    pub fn step(&self, step_id: &str) -> Option<&StepRecord> {
        self.steps.iter().find(|step| step.id == step_id)
    }

    /// The step `step_id` of this cluster, for an operation that changes it.
    pub fn step_mut(&mut self, step_id: &str) -> Option<&mut StepRecord> {
        self.steps.iter_mut().find(|step| step.id == step_id)
    }

    /// The instance group `instance_group_id` of this cluster, for an operation that
    /// changes it.
    pub fn group_mut(&mut self, instance_group_id: &str) -> Option<&mut InstanceGroupRecord> {
        self.groups
            .iter_mut()
            .find(|group| group.id == instance_group_id)
    }

    /// The wire `ClusterStatus` for this record.
    pub fn to_status(&self) -> ClusterStatus {
        ClusterStatus {
            state: Some(self.status.state.clone()),
            state_change_reason: self.status.state_change_reason.as_ref().map(|reason| {
                ClusterStateChangeReason {
                    code: Some(reason.code.clone()),
                    message: reason.message.clone(),
                }
            }),
            timeline: Some(ClusterTimeline {
                creation_date_time: Some(timestamp(self.status.creation_date_time)),
                ready_date_time: self.status.ready_date_time.map(timestamp),
                end_date_time: self.status.end_date_time.map(timestamp),
            }),
            // Nothing has failed: this cluster carries no error details.
            error_details: None,
        }
    }

    /// The wire `Cluster` for this record — what `DescribeCluster` answers with, as it
    /// stands at `read`.
    pub fn to_cluster(&self, read: &ClusterRead<'_>) -> Cluster {
        Cluster {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.to_status()),
            ec2_instance_attributes: Some(Ec2InstanceAttributes {
                ec2_key_name: self.instances.ec2_key_name.clone(),
                ec2_subnet_id: self.instances.ec2_subnet_id.clone(),
                // The subnet and zone CANDIDATES the launch named, which are a collection
                // the cluster carries: a launch that named none carries an empty one, and
                // an absent list would say the member is unknown where the service knows it
                // is empty.
                requested_ec2_subnet_ids: Some(self.instances.requested_ec2_subnet_ids.clone()),
                ec2_availability_zone: self.instances.ec2_availability_zone.clone(),
                requested_ec2_availability_zones: Some(
                    self.instances.requested_ec2_availability_zones.clone(),
                ),
                // "Also called instance profile and Amazon EC2 role" —
                // RunJobFlow$JobFlowRole is what the cluster's instances assume.
                iam_instance_profile: self.job_flow_role.clone(),
                emr_managed_master_security_group: self
                    .instances
                    .emr_managed_master_security_group
                    .clone(),
                emr_managed_slave_security_group: self
                    .instances
                    .emr_managed_slave_security_group
                    .clone(),
                service_access_security_group: self
                    .instances
                    .service_access_security_group
                    .clone(),
                additional_master_security_groups: optional_list(
                    &self.instances.additional_master_security_groups,
                ),
                additional_slave_security_groups: optional_list(
                    &self.instances.additional_slave_security_groups,
                ),
            }),
            instance_collection_type: Some(self.instances.instance_collection_type.clone()),
            log_uri: self.log_uri.clone(),
            log_encryption_kms_key_id: self.log_encryption_kms_key_id.clone(),
            requested_ami_version: self.requested_ami_version.clone(),
            // No AMI is running: this emulator launches no instances.
            running_ami_version: None,
            release_label: self.release_label.clone(),
            auto_terminate: Some(self.auto_terminate),
            termination_protected: Some(self.instances.termination_protected),
            unhealthy_node_replacement: Some(self.instances.unhealthy_node_replacement),
            visible_to_all_users: Some(self.visible_to_all_users),
            // A described cluster reports the collections it carries, empty for a launch
            // that supplied none of that kind: an absent list would say the member is
            // unknown where the service knows it is empty.
            applications: Some(self.applications.clone()),
            tags: Some(self.tags.clone()),
            service_role: self.service_role.clone(),
            normalized_instance_hours: Some(normalized_instance_hours(self, read.now)),
            master_public_dns_name: self.master_public_dns_name(read.master_host),
            configurations: Some(served_configurations(&self.configurations)),
            security_configuration: self.security_configuration.clone(),
            auto_scaling_role: self.auto_scaling_role.clone(),
            scale_down_behavior: Some(self.scale_down_behavior.clone()),
            custom_ami_id: self.custom_ami_id.clone(),
            ebs_root_volume_size: self.ebs_root_volume_size,
            repo_upgrade_on_boot: self.repo_upgrade_on_boot.clone(),
            kerberos_attributes: self.kerberos_attributes.clone(),
            cluster_arn: Some(self.arn.clone()),
            // Not launched on an Outpost.
            outpost_arn: None,
            step_concurrency_level: Some(self.step_concurrency_level),
            placement_groups: Some(self.placement_groups.clone()),
            os_release_label: self.os_release_label.clone(),
            ebs_root_volume_iops: self.ebs_root_volume_iops,
            ebs_root_volume_throughput: self.ebs_root_volume_throughput,
            extended_support: self.extended_support,
            monitoring_configuration: self.monitoring_configuration.clone(),
            session_enabled: self.session_enabled,
        }
    }

    /// The wire `ClusterSummary` for this record — what `ListClusters` answers with, as it
    /// stands at `now`.
    pub fn to_summary(&self, now: SystemTime) -> ClusterSummary {
        ClusterSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.to_status()),
            normalized_instance_hours: Some(normalized_instance_hours(self, now)),
            cluster_arn: Some(self.arn.clone()),
            outpost_arn: None,
        }
    }

    /// "The DNS name of the master node." (`Cluster$MasterPublicDnsName` doc.)
    ///
    /// A cluster reports it once its master node is UP: the node is provisioned while the
    /// cluster starts, so the name appears when the cluster leaves `STARTING` — the
    /// instant its status records as `ReadyDateTime` — and stays on the record from then
    /// on, a cluster that has since terminated included. A cluster that never became
    /// ready never had a master node to name, so it reports none.
    /// WHICH name that is follows the one rule an announced address obeys here: the member
    /// exists for a client to REACH ("If the cluster is on a private subnet, this is the
    /// private DNS name. On a public subnet, this is the public DNS name"), so where this
    /// emulator answers on a host of its own, `host` is that host and the master node is
    /// named by it — the one address a client of this emulator can reach the cluster at.
    /// Where it answers on none, there is no address to announce and the node is named by
    /// the Amazon-provided private hostname of the address it holds ([`master_dns_name`]),
    /// which is the name AWS reports for a node with no public address.
    fn master_public_dns_name(&self, host: Option<&str>) -> Option<String> {
        self.status
            .ready_date_time
            .map(|_| host.unwrap_or(&self.master_dns_name).to_string())
    }
}

/// What a read of a cluster needs beside the record: the instant it is read at, and the host
/// this emulator answers on for the caller that read it.
///
/// Both are facts of the REQUEST rather than of the cluster: the accrued cost follows from
/// how long the cluster has been up by now ([`normalized_instance_hours`]), and the address a
/// client can reach the cluster's master node at is the one this emulator is served on
/// ([`ClusterRecord::master_public_dns_name`]).
#[derive(Debug, Clone, Copy)]
pub struct ClusterRead<'a> {
    /// The instant this read happens at.
    pub now: SystemTime,
    /// The host this emulator answers on, or `None` when it answers on none.
    pub master_host: Option<&'a str>,
}

/// One persisted instance fleet, built from a submitted `InstanceFleetConfig`.
///
/// [`InstanceFleetRecord::to_instance_fleet`] projects this to the modelled
/// `InstanceFleet` shape, so `ListInstanceFleets` reports what was persisted rather than
/// anything built from its own input; what is held here is the submitted configuration
/// plus the id EMR minted for it and the lifecycle the clock moves.
#[derive(Debug, Clone)]
pub struct InstanceFleetRecord {
    pub id: String,
    pub config: InstanceFleetConfig,
    pub state: String,
    pub state_change_reason: Option<StateChangeReasonRecord>,
    pub creation_date_time: SystemTime,
    pub ready_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the `PROVISIONING` -> `RUNNING` transition the clock owes comes due.
    pub ready_due_at: Option<SystemTime>,
}

impl InstanceFleetRecord {
    /// Persist a submitted `InstanceFleetConfig` under `id`.
    ///
    /// A fleet EMR has just accepted is `PROVISIONING` — "the instance fleet is
    /// provisioning Amazon EC2 resources and is not yet ready to run jobs"
    /// (`InstanceFleetState`) — and the clock owes it the move to `RUNNING`.
    pub fn from_config(id: String, config: InstanceFleetConfig, now: SystemTime) -> Self {
        Self {
            id,
            config,
            state: instance_fleet_state::PROVISIONING.to_string(),
            // Nothing has moved this fleet's state by request or by failure, so there is
            // no reason to report beside it.
            state_change_reason: None,
            creation_date_time: now,
            ready_date_time: None,
            end_date_time: None,
            ready_due_at: Some(now + TRANSITION_SPAN),
        }
    }

    /// Materialise the transition the clock already made due, and nothing else: a
    /// fleet that was `PROVISIONING` is `RUNNING` once its span has passed, which was
    /// owed at that instant whether or not anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.state != instance_fleet_state::PROVISIONING {
            return;
        }
        if let Some(due) = self.ready_due_at {
            if now >= due {
                self.state = instance_fleet_state::RUNNING.to_string();
                // "The time and date the instance fleet was ready to run jobs."
                // (`InstanceFleetTimeline$ReadyDateTime` doc.) The instant the move came
                // DUE is when it happened, not the instant somebody looked.
                self.ready_date_time = Some(due);
                self.ready_due_at = None;
            }
        }
    }

    /// End this fleet because its cluster has terminated, at the instant the cluster's
    /// own termination came due.
    ///
    /// "TERMINATED—The instance fleet is no longer active, and all Amazon EC2 instances
    /// have been terminated." (`InstanceFleetStatus$State` doc.) A fleet outlives its
    /// cluster by nothing, and the modelled reason for it is `CLUSTER_TERMINATED` — the
    /// same end an instance group of a terminated cluster reaches
    /// ([`InstanceGroupRecord::end_at`]).
    pub fn end_at(&mut self, at: SystemTime) {
        if self.state == instance_fleet_state::TERMINATED {
            return;
        }
        self.state = instance_fleet_state::TERMINATED.to_string();
        self.state_change_reason = Some(StateChangeReasonRecord {
            code: instance_fleet_state_change_reason_code::CLUSTER_TERMINATED.to_string(),
            message: Some("The cluster was terminated.".to_string()),
        });
        self.ready_due_at = None;
        self.end_date_time = Some(at);
    }

    /// The wire `InstanceFleetStatus` for this record.
    pub fn to_status(&self) -> InstanceFleetStatus {
        InstanceFleetStatus {
            state: Some(self.state.clone()),
            state_change_reason: self.state_change_reason.as_ref().map(|reason| {
                InstanceFleetStateChangeReason {
                    code: Some(reason.code.clone()),
                    message: reason.message.clone(),
                }
            }),
            timeline: Some(InstanceFleetTimeline {
                creation_date_time: Some(timestamp(self.creation_date_time)),
                ready_date_time: self.ready_date_time.map(timestamp),
                end_date_time: self.end_date_time.map(timestamp),
            }),
        }
    }

    /// The wire `InstanceFleet` for this record — what `ListInstanceFleets` reports.
    ///
    /// The targets, the launch and resize specifications and the per-instance-type
    /// configurations are the ones that were SUBMITTED; the status is where the clock has
    /// got to, and the provisioned capacities are what that status implies
    /// ([`InstanceFleetRecord::provisioned_capacity`]).
    pub fn to_instance_fleet(&self) -> InstanceFleet {
        let target_on_demand_capacity = target_capacity(self.config.target_on_demand_capacity);
        let target_spot_capacity = target_capacity(self.config.target_spot_capacity);
        InstanceFleet {
            id: Some(self.id.clone()),
            name: self.config.name.clone(),
            status: Some(self.to_status()),
            // "The node type that the instance fleet hosts" is required on the
            // configuration, so a persisted fleet always reports one.
            instance_fleet_type: Some(self.config.instance_fleet_type.clone()),
            target_on_demand_capacity: Some(target_on_demand_capacity),
            target_spot_capacity: Some(target_spot_capacity),
            provisioned_on_demand_capacity: Some(
                self.provisioned_capacity(target_on_demand_capacity),
            ),
            provisioned_spot_capacity: Some(self.provisioned_capacity(target_spot_capacity)),
            instance_type_specifications: self
                .config
                .instance_type_configs
                .as_deref()
                .map(|configs| {
                    configs
                        .iter()
                        .map(instance_type_specification)
                        .collect::<Vec<_>>()
                })
                .as_deref()
                .and_then(optional_list),
            launch_specifications: self.config.launch_specifications.clone(),
            // The resize specifications this fleet runs under: the ones it was configured
            // with, with the Spot resize timeout the service supplies for a fleet it can
            // resize.
            resize_specifications: effective_resize_specifications(
                &self.config.instance_fleet_type,
                self.config.resize_specifications.as_ref(),
            ),
            context: self.config.context.clone(),
        }
    }

    /// "The number of On-Demand units that have been provisioned for the instance fleet
    /// to fulfill `TargetOnDemandCapacity`" (`InstanceFleet$ProvisionedOnDemandCapacity`
    /// doc), and the same for the Spot units.
    ///
    /// A fleet still acquiring its instances has provisioned none of them, and one that
    /// has terminated holds none any more; a fleet that is up has provisioned the target
    /// it asked for.
    fn provisioned_capacity(&self, target: i32) -> i32 {
        if self.state == instance_fleet_state::RUNNING {
            target
        } else {
            0
        }
    }
}

/// "The target capacity of On-Demand units for the instance fleet, which determines how
/// many On-Demand Instances to provision" (`InstanceFleet$TargetOnDemandCapacity` doc),
/// and the same for the Spot units — as a fleet that was configured with `requested`
/// reports it.
///
/// Both members are `WholeNumber` (`@range` min 0) and a fleet AWS describes carries both:
/// "if not specified or set to 0, only Spot Instances are provisioned for the instance
/// fleet using `TargetSpotCapacity`" reads the ABSENT value and the explicit zero as one
/// and the same request, so a fleet that asked for no units of a market reports zero of it
/// rather than leaving the member out. That is also what lets a client read the capacity it
/// is NOT changing and pass it back to `ModifyInstanceFleet`, whose two capacities travel
/// in one call.
fn target_capacity(requested: Option<i32>) -> i32 {
    requested.unwrap_or(0).max(0)
}

/// One `InstanceTypeConfig` a fleet was configured with, as the
/// `InstanceTypeSpecification` that fleet reports it back as.
///
/// "The configuration specification for each instance type in an instance fleet"
/// (`InstanceTypeSpecification` doc): the two shapes carry the same facts, so every
/// member common to both passes through. The one difference is EBS — the configuration
/// states it as an `EbsConfiguration` (block-device configurations plus the optimized
/// flag) where the specification states it as the `EbsBlockDevice` list those
/// configurations produce, mapped exactly as an instance group's are
/// ([`ebs_device_name`]) — so one rule names the devices for both collection types.
fn instance_type_specification(config: &InstanceTypeConfig) -> InstanceTypeSpecification {
    InstanceTypeSpecification {
        instance_type: Some(config.instance_type.clone()),
        // The units one instance of this type provides, and the Spot bid it makes, as the
        // fleet provisions it: both default where the configuration named neither
        // ([`DEFAULT_WEIGHTED_CAPACITY`], [`DEFAULT_BID_PRICE_PERCENTAGE_OF_ON_DEMAND`]),
        // so a client can account for the capacity a fleet provisions.
        weighted_capacity: Some(config.weighted_capacity.unwrap_or(DEFAULT_WEIGHTED_CAPACITY)),
        bid_price: config.bid_price.clone(),
        bid_price_as_percentage_of_on_demand_price: effective_bid_price_percentage(
            config.bid_price.as_deref(),
            config.bid_price_as_percentage_of_on_demand_price,
        ),
        configurations: config
            .configurations
            .as_deref()
            .map(served_configurations)
            // A configuration list the instance type supplied nothing for stays ABSENT,
            // unlike `Cluster$Configurations`, which a described cluster always carries:
            // what the service reports for a fleet's instance type that configured none
            // is not settled here, so the member is left as it was.
            .filter(|configurations| !configurations.is_empty()),
        ebs_block_devices: optional_list(&fleet_ebs_block_devices(config)),
        ebs_optimized: config
            .ebs_configuration
            .as_ref()
            .and_then(|configuration| configuration.ebs_optimized),
        custom_ami_id: config.custom_ami_id.clone(),
        priority: config.priority,
    }
}

/// One persisted instance group: "a group of instances that have common purpose. For
/// example, CORE instance group is used for HDFS." (`InstanceGroup` doc.)
///
/// What is held is the submitted `InstanceGroupConfig` plus the id EMR minted for it and
/// the lifecycle the clock moves; [`InstanceGroupRecord::to_instance_group`] is what
/// projects it to the modelled `InstanceGroup` shape, so `ListInstanceGroups` reports
/// what was persisted rather than anything built from its own input.
#[derive(Debug, Clone)]
pub struct InstanceGroupRecord {
    pub id: String,
    pub config: InstanceGroupConfig,
    pub state: String,
    pub state_change_reason: Option<StateChangeReasonRecord>,
    pub creation_date_time: SystemTime,
    pub ready_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the `PROVISIONING` -> `RUNNING` transition the clock owes comes due.
    pub ready_due_at: Option<SystemTime>,
    /// When the `ATTACHING` -> `ATTACHED` transition the clock owes the automatic
    /// scaling policy this group carries comes due, and `None` once that transition has
    /// been materialised or the group carries no policy.
    pub policy_attaching_due_at: Option<SystemTime>,
    /// When the `RESIZING`/`RECONFIGURING` -> `RUNNING` transition the clock owes the
    /// modification in flight comes due, and `None` when nothing is being applied.
    pub change_due_at: Option<SystemTime>,
    /// The instances still running while a resize is in flight — the count the group was
    /// already up at — and `None` when no resize is.
    pub resizing_from_count: Option<i32>,
    /// "Policy for customizing shrink operations" (`InstanceGroup$ShrinkPolicy` doc): the
    /// one the last modification submitted, and `None` until one does.
    pub shrink_policy: Option<ShrinkPolicy>,
    /// "The version number of the requested configuration specification for this
    /// instance group" (`InstanceGroup$ConfigurationsVersion` doc): a reconfiguration
    /// moves it on, so a group nothing has reconfigured carries none.
    pub configurations_version: Option<i64>,
    /// The version of the reconfiguration in flight, which becomes
    /// [`InstanceGroupRecord::last_successfully_applied_configurations_version`] once the
    /// clock makes it applied.
    pub pending_configurations_version: Option<i64>,
    /// "A list of configurations that were successfully applied for an instance group
    /// last time" (`InstanceGroup$LastSuccessfullyAppliedConfigurations` doc).
    pub last_successfully_applied_configurations: Option<Vec<Configuration>>,
    /// The version of those configurations
    /// (`InstanceGroup$LastSuccessfullyAppliedConfigurationsVersion` doc).
    pub last_successfully_applied_configurations_version: Option<i64>,
}

impl InstanceGroupRecord {
    /// Persist a submitted `InstanceGroupConfig` under `id`.
    ///
    /// A group EMR has just accepted is `PROVISIONING` (`InstanceGroupState`) — its
    /// Amazon EC2 instances are not up yet — and the clock owes it the move to
    /// `RUNNING`. A policy the configuration carries is `ATTACHING` for a span of its
    /// own.
    pub fn from_config(id: String, config: InstanceGroupConfig, now: SystemTime) -> Self {
        let policy_attaching_due_at = config
            .auto_scaling_policy
            .as_ref()
            .map(|_| now + TRANSITION_SPAN);
        Self {
            id,
            config,
            state: instance_group_state::PROVISIONING.to_string(),
            state_change_reason: None,
            creation_date_time: now,
            ready_date_time: None,
            end_date_time: None,
            ready_due_at: Some(now + TRANSITION_SPAN),
            policy_attaching_due_at,
            change_due_at: None,
            resizing_from_count: None,
            shrink_policy: None,
            configurations_version: None,
            pending_configurations_version: None,
            last_successfully_applied_configurations: None,
            last_successfully_applied_configurations_version: None,
        }
    }

    /// Start the resize to `target` instances a modification asked for.
    ///
    /// "ModifyInstanceGroups modifies the number of nodes … The input parameters include
    /// the new target instance count for the group" (`ModifyInstanceGroups` doc, Smithy
    /// model): the target is what the group requests from here on, which is what a later
    /// read reports as its `RequestedInstanceCount`. A group whose instances are already
    /// up is `RESIZING` until the clock makes the new count due, and goes on reporting the
    /// instances it already runs while it is; one still provisioning stays
    /// `PROVISIONING` — it is yet to acquire anything, and what it acquires is the new
    /// target.
    pub fn begin_resize(&mut self, target: i32, now: SystemTime) {
        if instance_group_state::UP.contains(&self.state.as_str()) {
            self.resizing_from_count = Some(self.running_instance_count());
            self.state = instance_group_state::RESIZING.to_string();
            self.change_due_at = Some(now + TRANSITION_SPAN);
        }
        self.config.instance_count = target;
    }

    /// Start the reconfiguration a modification asked for, merging `configurations` into
    /// the ones this group holds when `merge` is set and replacing them otherwise.
    ///
    /// "Type of reconfiguration requested. Valid values are MERGE and OVERWRITE."
    /// (`InstanceGroupModifyConfig$ReconfigurationType` doc.) A merge keeps every
    /// classification the group holds that the request does not name again, in the order
    /// it held them, so what the group requests is the two specifications merged; an
    /// overwrite is the submitted specification alone. Either way the requested
    /// specification moves to a new version, and that version only becomes the last
    /// SUCCESSFULLY APPLIED one when the clock makes the reconfiguration applied.
    pub fn begin_reconfiguration(
        &mut self,
        configurations: Vec<Configuration>,
        merge: bool,
        now: SystemTime,
    ) {
        let requested = if merge {
            let mut merged = self.config.configurations.clone().unwrap_or_default();
            for configuration in configurations {
                match merged
                    .iter_mut()
                    .find(|held| held.classification == configuration.classification)
                {
                    Some(held) => *held = configuration,
                    None => merged.push(configuration),
                }
            }
            merged
        } else {
            configurations
        };
        self.config.configurations = Some(requested);
        let version = self.configurations_version.unwrap_or(0).saturating_add(1);
        self.configurations_version = Some(version);
        self.pending_configurations_version = Some(version);
        if instance_group_state::UP.contains(&self.state.as_str())
            && self.state != instance_group_state::RESIZING
        {
            self.state = instance_group_state::RECONFIGURING.to_string();
        }
        self.change_due_at = Some(now + TRANSITION_SPAN);
    }

    /// Attach `policy` to this group, replacing whatever policy it carried before.
    ///
    /// "Creates or updates an automatic scaling policy for a core instance group or task
    /// instance group" (`PutAutoScalingPolicy` doc, Smithy model): the submitted policy
    /// is what the group carries from here on. A policy EMR has just accepted is still
    /// `ATTACHING` (`AutoScalingPolicyState`), and the clock owes it the move to
    /// `ATTACHED`.
    pub fn attach_auto_scaling_policy(&mut self, policy: AutoScalingPolicy, now: SystemTime) {
        self.config.auto_scaling_policy = Some(policy);
        self.policy_attaching_due_at = Some(now + TRANSITION_SPAN);
    }

    /// Take the automatic scaling policy off this group, reporting whether it carried
    /// one.
    ///
    /// "Removes an automatic scaling policy from a specified instance group"
    /// (`RemoveAutoScalingPolicy` doc, Smithy model): the group carries no policy from
    /// here on, so the attach the clock may still have owed it goes with it. `false` says
    /// the group carried no policy, which is what leaves the caller nothing to remove —
    /// and in that case the record is left exactly as it was, so a refused call writes
    /// nothing at all.
    pub fn remove_auto_scaling_policy(&mut self) -> bool {
        if self.config.auto_scaling_policy.take().is_none() {
            return false;
        }
        self.policy_attaching_due_at = None;
        true
    }

    /// Materialise the transitions the clock already made due, and nothing else: a group
    /// that was `PROVISIONING` is `RUNNING` once its span has passed, a group that was
    /// `RESIZING` or `RECONFIGURING` is `RUNNING` once the modification's span has (with
    /// the configurations it requested now the ones last successfully applied), and a
    /// policy that was `ATTACHING` is `ATTACHED` once its own has — each owed at that
    /// instant whether or not anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        if let Some(due) = self.policy_attaching_due_at {
            if now >= due {
                self.policy_attaching_due_at = None;
            }
        }
        if let Some(due) = self.change_due_at {
            if now >= due {
                self.change_due_at = None;
                self.resizing_from_count = None;
                if let Some(version) = self.pending_configurations_version.take() {
                    self.last_successfully_applied_configurations =
                        self.config.configurations.clone();
                    self.last_successfully_applied_configurations_version = Some(version);
                }
                if self.state == instance_group_state::RESIZING
                    || self.state == instance_group_state::RECONFIGURING
                {
                    self.state = instance_group_state::RUNNING.to_string();
                }
            }
        }
        if self.state != instance_group_state::PROVISIONING {
            return;
        }
        if let Some(due) = self.ready_due_at {
            if now >= due {
                self.state = instance_group_state::RUNNING.to_string();
                self.ready_date_time = Some(due);
                self.ready_due_at = None;
            }
        }
    }

    /// End this group at `at`, because its cluster reached `TERMINATED`.
    pub fn end_at(&mut self, at: SystemTime) {
        if self.state == instance_group_state::ENDED {
            return;
        }
        self.state = instance_group_state::ENDED.to_string();
        self.state_change_reason = Some(StateChangeReasonRecord {
            code: instance_group_state_change_reason_code::CLUSTER_TERMINATED.to_string(),
            message: Some("The cluster was terminated.".to_string()),
        });
        self.ready_due_at = None;
        self.policy_attaching_due_at = None;
        self.change_due_at = None;
        self.resizing_from_count = None;
        self.end_date_time = Some(at);
    }

    /// "The marketplace to provision instances for this group. Valid values are
    /// ON_DEMAND or SPOT." (`InstanceGroup$Market` doc.) A configuration that names no
    /// market provisions On-Demand instances, which is Amazon EMR's default market.
    fn market(&self) -> &str {
        self.config
            .market
            .as_deref()
            .filter(|market| !market.is_empty())
            .unwrap_or(market_type::ON_DEMAND)
    }

    /// "The number of instances currently running in this instance group."
    /// (`InstanceGroup$RunningInstanceCount` doc.) None are running while the group is
    /// still provisioning them or once it has ended; a group that is up runs the target
    /// number it requested, and one part way through a resize goes on running the count
    /// it was already up at until the new target comes due.
    fn running_instance_count(&self) -> i32 {
        if !instance_group_state::UP.contains(&self.state.as_str()) {
            return 0;
        }
        self.resizing_from_count
            .unwrap_or(self.config.instance_count)
            .max(0)
    }

    /// The wire `AutoScalingPolicyDescription` for the automatic scaling policy this
    /// group carries — the one its configuration was given, or the one the last
    /// `PutAutoScalingPolicy` attached — and `None` when it carries none.
    ///
    /// The constraints and rules are the ones that were submitted. The status is where
    /// the clock has got to: a policy whose attach span has not passed is `ATTACHING`,
    /// and one past it is `ATTACHED` (`AutoScalingPolicyState`).
    pub fn to_auto_scaling_policy(&self) -> Option<AutoScalingPolicyDescription> {
        let policy = self.config.auto_scaling_policy.as_ref()?;
        let state = if self.policy_attaching_due_at.is_some() {
            auto_scaling_policy_state::ATTACHING
        } else {
            auto_scaling_policy_state::ATTACHED
        };
        Some(AutoScalingPolicyDescription {
            status: Some(AutoScalingPolicyStatus {
                state: Some(state.to_string()),
                // Nothing moved this policy's state by request or by failure, so there
                // is no reason to report beside it.
                state_change_reason: None,
            }),
            constraints: Some(policy.constraints.clone()),
            rules: optional_list(&policy.rules),
        })
    }

    /// The wire `InstanceGroupStatus` for this record.
    pub fn to_status(&self) -> InstanceGroupStatus {
        InstanceGroupStatus {
            state: Some(self.state.clone()),
            state_change_reason: self.state_change_reason.as_ref().map(|reason| {
                InstanceGroupStateChangeReason {
                    code: Some(reason.code.clone()),
                    message: reason.message.clone(),
                }
            }),
            timeline: Some(InstanceGroupTimeline {
                creation_date_time: Some(timestamp(self.creation_date_time)),
                ready_date_time: self.ready_date_time.map(timestamp),
                end_date_time: self.end_date_time.map(timestamp),
            }),
        }
    }

    /// The wire `InstanceGroup` for this record — what `ListInstanceGroups` reports.
    pub fn to_instance_group(&self) -> InstanceGroup {
        InstanceGroup {
            id: Some(self.id.clone()),
            name: self.config.name.clone(),
            market: Some(self.market().to_string()),
            // "The role of the instance group in the cluster"
            // (`InstanceGroupConfig$InstanceRole`) IS the group's type: both carry
            // MASTER, CORE or TASK.
            instance_group_type: Some(self.config.instance_role.clone()),
            bid_price: self.config.bid_price.clone(),
            instance_type: Some(self.config.instance_type.clone()),
            requested_instance_count: Some(self.config.instance_count),
            running_instance_count: Some(self.running_instance_count()),
            status: Some(self.to_status()),
            // Served with the property map every configuration carries; an empty list is
            // left absent for the reason [`instance_type_specification`] states.
            configurations: self
                .config
                .configurations
                .as_deref()
                .map(served_configurations)
                .filter(|configurations| !configurations.is_empty()),
            // The configuration versions are what a RECONFIGURATION moves ("the version
            // number of the requested configuration specification"), so a group nothing
            // has reconfigured carries none, and the ones last SUCCESSFULLY APPLIED are
            // the specification of the last reconfiguration the clock has made applied —
            // not the one still in flight.
            configurations_version: self.configurations_version,
            last_successfully_applied_configurations: self
                .last_successfully_applied_configurations
                .as_deref()
                .map(served_configurations)
                .filter(|configurations| !configurations.is_empty()),
            last_successfully_applied_configurations_version: self
                .last_successfully_applied_configurations_version,
            ebs_block_devices: optional_list(&ebs_block_devices(
                self.config.ebs_configuration.as_ref(),
            )),
            ebs_optimized: self
                .config
                .ebs_configuration
                .as_ref()
                .and_then(|configuration| configuration.ebs_optimized),
            // "Policy for customizing shrink operations" — a shrink policy arrives with a
            // `ModifyInstanceGroups` resize, so a group nobody has resized carries none.
            shrink_policy: self.shrink_policy.clone(),
            auto_scaling_policy: self.to_auto_scaling_policy(),
            custom_ami_id: self.config.custom_ami_id.clone(),
        }
    }
}

/// The Amazon EBS block devices an `EbsConfiguration` maps onto each of a collection's
/// instances.
///
/// "The EBS block devices that are mapped to this instance group."
/// (`InstanceGroup$EbsBlockDevices` doc; `InstanceTypeSpecification$EbsBlockDevices` is
/// the same list for a fleet's instance type.) One device per volume the configuration
/// requested — `VolumesPerInstance` of each block device configuration — in configuration
/// order, which is why instance groups and instance fleets share this one rule.
///
/// Both the number of configurations walked and the number of devices produced are capped
/// at the bounds [`validate_instance_group_config`] refuses a request past, so a record
/// this emulator should never have persisted still cannot amplify a read.
fn ebs_block_devices(configuration: Option<&EbsConfiguration>) -> Vec<EbsBlockDevice> {
    let Some(configuration) = configuration else {
        return Vec::new();
    };
    let total_cap = MAX_EBS_VOLUMES_PER_INSTANCE as usize;
    let mut devices = Vec::new();
    for block_device in configuration
        .ebs_block_device_configs
        .iter()
        .flatten()
        .take(MAX_EBS_BLOCK_DEVICE_CONFIGS)
    {
        let volumes = block_device
            .volumes_per_instance
            .unwrap_or(DEFAULT_VOLUMES_PER_INSTANCE)
            .clamp(0, MAX_EBS_VOLUMES_PER_INSTANCE);
        for _ in 0..volumes {
            if devices.len() >= total_cap {
                return devices;
            }
            devices.push(EbsBlockDevice {
                volume_specification: Some(block_device.volume_specification.clone()),
                device: Some(ebs_device_name(devices.len())),
            });
        }
    }
    devices
}

/// The device name Amazon EMR exposes an instance group's `index`-th requested EBS
/// volume at.
///
/// "The device name that is exposed to the instance, such as `/dev/sdh`."
/// (`EbsBlockDevice$Device` doc.) The form comes from that doc; the first letter is
/// CHOSEN: `/dev/sda` is the root volume, so the requested volumes start at `/dev/sdb`
/// and run through the alphabet, continuing `/dev/sdaa` once the single-letter names run
/// out.
fn ebs_device_name(index: usize) -> String {
    // `index` 0 is the SECOND device letter, `b`.
    let mut remaining = index + 1;
    let mut suffix = String::new();
    loop {
        suffix.insert(0, char::from(b'a' + (remaining % 26) as u8));
        if remaining < 26 {
            break;
        }
        remaining = remaining / 26 - 1;
    }
    format!("/dev/sd{suffix}")
}

/// The instance groups a `RunJobFlow` request implies when it configures its instances
/// the uniform way, naming node instance types instead of an `InstanceGroups` list.
///
/// "The Amazon EC2 instance type of the master node", "The Amazon EC2 instance type of
/// the core and task nodes" and "The number of Amazon EC2 instances in the cluster"
/// (`JobFlowInstancesConfig$MasterInstanceType`, `$SlaveInstanceType` and
/// `$InstanceCount` docs): one instance is the master, so the cluster gets a MASTER
/// group of one instance and — when it asked for more than one instance and named the
/// type of its core and task nodes — a CORE group holding the rest.
///
/// Answers an empty list when the request named instance groups itself (those are
/// persisted verbatim) or named no master instance type at all, since then nothing in
/// the request says what to provision.
///
/// A request that asks for more than one instance but names no core/task node type gets
/// the master group alone: nothing in it says what to provision the remaining nodes as,
/// and the model marks neither member required, so the groups reported are exactly the
/// ones the request describes. Whether `RunJobFlow` should refuse that combination
/// outright is `RunJobFlow`'s own input validation, which neither the model nor the
/// operation's documentation settles here.
pub fn implied_instance_group_configs(instances: &JobFlowInstancesConfig) -> Vec<InstanceGroupConfig> {
    if instances
        .instance_groups
        .as_ref()
        .is_some_and(|groups| !groups.is_empty())
    {
        return Vec::new();
    }
    let Some(master_instance_type) = instances
        .master_instance_type
        .as_ref()
        .filter(|instance_type| !instance_type.is_empty())
    else {
        return Vec::new();
    };
    // A request that names no count asks for the one master instance.
    let total = instances.instance_count.unwrap_or(1).max(1);
    let mut configs = vec![InstanceGroupConfig {
        // Nothing in the request names these groups, so neither does the emulator.
        name: None,
        market: None,
        instance_role: instance_group_type::MASTER.to_string(),
        bid_price: None,
        instance_type: master_instance_type.clone(),
        instance_count: 1,
        configurations: None,
        ebs_configuration: None,
        auto_scaling_policy: None,
        custom_ami_id: None,
    }];
    if let Some(slave_instance_type) = instances
        .slave_instance_type
        .as_ref()
        .filter(|instance_type| !instance_type.is_empty())
    {
        if total > 1 {
            configs.push(InstanceGroupConfig {
                name: None,
                market: None,
                instance_role: instance_group_type::CORE.to_string(),
                bid_price: None,
                instance_type: slave_instance_type.clone(),
                instance_count: total - 1,
                configurations: None,
                ebs_configuration: None,
                auto_scaling_policy: None,
                custom_ami_id: None,
            });
        }
    }
    configs
}

/// How a cluster launch that provisions no PRIMARY node is refused.
///
/// A cluster has exactly one primary instance group: "valid values are MASTER, CORE or
/// TASK" (`InstanceGroupConfig$InstanceRole` doc), and the MASTER group is the one the
/// cluster's coordinators run on, so a launch stating its capacity as instance groups and
/// naming no MASTER group among them describes a cluster that cannot come up.
///
/// Recorded behavior for the text, which is entirely FIXED: Amazon EMR states how many
/// primary groups it was given and how many it needs, and interpolates nothing of the
/// request.
pub const NO_PRIMARY_INSTANCE_GROUP_MESSAGE: &str =
    "Zero master instance groups supplied, you must specify exactly one master instance \
     group.";

/// How a launch naming MORE than one primary instance group is refused: the other half of
/// the same "exactly one" requirement, in the same sentence with the count it was given.
///
/// One primary group is the most a cluster has — a highly available cluster states its
/// several primary nodes as that one group's `InstanceCount`
/// ([`primary_node_count`]) — so two primary groups describe no cluster Amazon EMR
/// provisions either.
pub const MULTIPLE_PRIMARY_INSTANCE_GROUPS_MESSAGE: &str =
    "Multiple master instance groups supplied, you must specify exactly one master \
     instance group.";

/// The refusal a launch stating its capacity as instance GROUPS earns for the primary
/// group it names, or `None` when it names exactly one.
///
/// The groups a launch provisions are the ones it named, or — for a launch stating its
/// instances the uniform way — the ones that form implies
/// ([`implied_instance_group_configs`]), so a request naming a master instance type names
/// a primary even though it names no group at all. `requested` is the group list the
/// launch carried, which the handler has already taken off the request.
///
/// A launch stating its capacity as instance FLEETS states its primary in that other
/// vocabulary ([`primary_node_count`]), so this reading is asked only of the groups form.
pub fn primary_instance_group_refusal(
    instances: &JobFlowInstancesConfig,
    requested: &[InstanceGroupConfig],
) -> Option<&'static str> {
    // The implied groups are derived only where they are the ones the launch provisions.
    let implied = if requested.is_empty() {
        implied_instance_group_configs(instances)
    } else {
        Vec::new()
    };
    let groups = if requested.is_empty() {
        implied.as_slice()
    } else {
        requested
    };
    match groups
        .iter()
        .filter(|group| group.instance_role == instance_group_type::MASTER)
        .count()
    {
        1 => None,
        0 => Some(NO_PRIMARY_INSTANCE_GROUP_MESSAGE),
        _ => Some(MULTIPLE_PRIMARY_INSTANCE_GROUPS_MESSAGE),
    }
}

/// Refuse an `InstanceGroupConfig` this emulator cannot hold as submitted.
///
/// The only such configuration is one whose `InstanceCount` is above the instances Amazon
/// EMR provisions for one group ([`MAX_INSTANCES_PER_GROUP`]), or whose `AutoScalingPolicy`
/// names no range the
/// scaling activity could stay inside (see [`validate_auto_scaling_policy`]), or whose
/// `EbsConfiguration` names more block device
/// configurations than [`MAX_EBS_BLOCK_DEVICE_CONFIGS`], or asks for a negative number of
/// volumes, or asks for more volumes IN TOTAL than
/// [`MAX_EBS_VOLUMES_PER_INSTANCE`] — every volume of every configuration becomes an
/// `EbsBlockDevice` in each later read of the group, so it is the total that has to be
/// bounded. Everything else the model already constrains. `Ok(())` means the group can be
/// persisted.
///
/// Every path that persists an `InstanceGroupConfig` calls this before minting a record.
pub fn validate_instance_group_config(config: &InstanceGroupConfig) -> Result<(), String> {
    // The capacity the group asks for, bounded before anything is persisted or launched for
    // it: every instance a group requests is one Amazon EC2 instance and one record of this
    // store ([`instance_group_count_violation`]).
    if let Some(message) = instance_group_count_violation(config.instance_count) {
        return Err(message);
    }
    if let Some(policy) = config.auto_scaling_policy.as_ref() {
        validate_auto_scaling_policy(policy)?;
    }
    let block_devices = config
        .ebs_configuration
        .as_ref()
        .and_then(|configuration| configuration.ebs_block_device_configs.as_deref())
        .unwrap_or_default();
    if block_devices.len() > MAX_EBS_BLOCK_DEVICE_CONFIGS {
        return Err(format!(
            "EbsBlockDeviceConfigs names {} configurations; an instance group names at \
             most {MAX_EBS_BLOCK_DEVICE_CONFIGS}",
            block_devices.len()
        ));
    }
    let mut total: i64 = 0;
    for block_device in block_devices {
        let volumes = block_device
            .volumes_per_instance
            .unwrap_or(DEFAULT_VOLUMES_PER_INSTANCE);
        if !(0..=MAX_EBS_VOLUMES_PER_INSTANCE).contains(&volumes) {
            return Err(format!(
                "VolumesPerInstance {volumes} is outside 0..{MAX_EBS_VOLUMES_PER_INSTANCE}"
            ));
        }
        total += i64::from(volumes);
    }
    if total > i64::from(MAX_EBS_VOLUMES_PER_INSTANCE) {
        return Err(format!(
            "EbsBlockDeviceConfigs request {total} volumes per instance; an instance \
             carries at most {MAX_EBS_VOLUMES_PER_INSTANCE}"
        ));
    }
    Ok(())
}

/// One persisted security configuration, named by a `CreateSecurityConfiguration`
/// request.
///
/// The document is held VERBATIM: `DescribeSecurityConfiguration` "provides the details
/// of a security configuration by returning the configuration JSON" (its Smithy doc),
/// so what a reader gets back is the exact string that was stored, never a
/// re-serialisation of a parse of it.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationRecord {
    pub name: String,
    pub security_configuration: String,
    pub creation_date_time: SystemTime,
}

// ---------------------------------------------------------------------------
// Persistent application user interfaces
//
// `CreatePersistentAppUI` "creates a persistent application user interface" for the
// resource its `TargetResourceArn` names (its own doc); `DescribePersistentAppUI`
// projects the stored record onto the `PersistentAppUI` wire shape and
// `GetPersistentAppUIPresignedURL` derives that UI's URL and readiness from the same
// record. Nothing below is built from one request's input alone.
// ---------------------------------------------------------------------------

/// This service's own ARN namespace — the vendor field of every ARN it mints
/// ([`cluster_arn`], [`studio_arn`]) and reads.
pub const ARN_VENDOR: &str = "elasticmapreduce";

/// The resource type of an EMR on EC2 cluster's ARN: `…:cluster/<id>`.
pub const CLUSTER_RESOURCE_TYPE: &str = "cluster";

/// The ARN vendors whose resources Amazon EMR publishes a persistent application user
/// interface for: an EMR on EC2 cluster (`elasticmapreduce`), an EMR on EKS job run
/// (`emr-containers`, whose job run id `CreatePersistentAppUI$EMRContainersConfig`
/// carries) and an EMR Serverless job run (`emr-serverless`). An ARN of any other
/// vendor names nothing this operation can attach a UI to.
pub const PERSISTENT_APP_UI_TARGET_VENDORS: &[&str] =
    &[ARN_VENDOR, "emr-containers", "emr-serverless"];

/// The fields of a caller-supplied ARN come from the tree's ONE
/// caller-supplied-ARN parse ([`service_common::arn`]) — the same reading every other
/// service makes, so no two of them disagree about what an ARN is. Below is only what
/// Amazon EMR puts ON TOP of that parse: which vendors it serves a UI for, and how it
/// reads one of its own cluster ARNs.
///
/// Whether Amazon EMR publishes a persistent application user interface for the
/// resource the ARN `value` names: `value` is a well-formed resource ARN and its vendor
/// is one of [`PERSISTENT_APP_UI_TARGET_VENDORS`]. A value that is no ARN at all names
/// no resource either, so it fails the same way.
pub fn is_persistent_app_ui_target_arn(value: &str) -> bool {
    ::service_common::arn::vendor(value)
        .is_some_and(|vendor| PERSISTENT_APP_UI_TARGET_VENDORS.contains(&vendor))
}

/// Whether the ARN `value` is one of THIS service's own ([`ARN_VENDOR`]) — the class
/// whose only persistent-application-UI target is a cluster.
pub fn is_own_vendor_arn(value: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, ARN_VENDOR)
}

/// Whether `value` names an IAM role of this account, which is what a member documented
/// as a role ARN has to name.
///
/// Read through the tree's shared IAM-ARN reading
/// ([`service_common::iam_role::role_name`]), the one this service already reads a
/// `JobFlowRole` with: a value that is ARN-shaped but names another vendor, another IAM
/// entity type, another account or a Region an IAM ARN does not carry names no role to
/// assume.
pub fn is_this_accounts_role_arn(value: &str) -> bool {
    ::service_common::iam_role::role_name(value).is_some()
}

/// The EMR on EC2 cluster the ARN `value` names, when it names one: the
/// `cluster/<id>` resource of this service's own vendor ([`cluster_arn`] is what mints
/// it), read with the shared parse's resource readers.
pub fn arn_cluster_id(value: &str) -> Option<&str> {
    if !is_own_vendor_arn(value) {
        return None;
    }
    ::service_common::arn::resource_id_of_type(value, CLUSTER_RESOURCE_TYPE)
        .filter(|id| !id.is_empty())
}

/// The `PersistentAppUIType` a UI created for the profiler `profiler` is addressed by,
/// or `None` for a profiler type the model does not declare.
///
/// The two vocabularies differ by one spelling: `ProfilerType` is `SHS | TEZUI | YTS`
/// and `PersistentAppUIType` is `SHS | TEZ | YTS`
/// (`smithy_query(query="shape", shape="ProfilerType,PersistentAppUIType")`), so the
/// Tez UI profiler is published as the `TEZ` user interface and the other two keep
/// their names.
pub fn persistent_app_ui_type_of_profiler(profiler: &str) -> Option<&'static str> {
    match profiler {
        "SHS" => Some("SHS"),
        "TEZUI" => Some("TEZ"),
        "YTS" => Some("YTS"),
        _ => None,
    }
}

/// The user interface a persistent application UI publishes when the request names no
/// `ProfilerType`: the Spark history server, which is the UI Amazon EMR keeps for a
/// completed application's event logs.
pub const DEFAULT_PERSISTENT_APP_UI_TYPE: &str = "SHS";

/// How `GetPersistentAppUIPresignedURL` refuses a request it can resolve no published
/// user interface for: an id this account holds no UI for, or a type the UI it names
/// does not publish. Amazon EMR answers both with one refusal, naming the member that
/// selects the interface rather than echoing the id back.
pub const PRESIGNED_URL_NO_SUCH_UI_MESSAGE: &str = "PersistentAppUIType is not valid.";

/// The query parameter a presigned application-UI URL carries its session token in.
pub const PRESIGNED_URL_TOKEN_PARAMETER: &str = "authToken";

/// How many characters of a presigned URL's session token are kept.
///
/// A chosen length: 64 characters out of the token alphabet below carry far more entropy
/// than one browser session needs, and the member the URL travels in (`XmlString`, len
/// 0..10280) admits it comfortably.
pub const PRESIGNED_URL_TOKEN_LEN: usize = 64;

/// Whether `character` may appear in a presigned URL's session token: the unreserved
/// characters of RFC 3986 §2.3, which travel in a URL query as themselves. The alphabet
/// the credential store draws a session token from also carries `+` and `/`, which do not,
/// so those are filtered out rather than encoded.
pub fn is_presigned_url_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~')
}

/// How many credential-store mints one presigned URL's token may draw its characters from.
///
/// A chosen bound: the store's session token is 712 characters, so one draw almost always
/// leaves [`PRESIGNED_URL_TOKEN_LEN`] admissible ones, and a few bounded draws keep the
/// work per request finite.
const PRESIGNED_URL_TOKEN_MINT_ATTEMPTS: usize = 4;

/// A fresh session token for ONE presigned application-UI URL.
///
/// The secret is minted by the emulator's CREDENTIAL STORE — the one component that
/// generates secrets — and then removed from it again: this token authenticates a browser
/// to an application user interface, NOT a SigV4 request to this emulator, so the emulator
/// must not go on accepting it as one and nothing accumulates per call. It is the same seam
/// every other service that vends a non-SigV4 secret draws from, so this crate adds no
/// randomness source of its own.
///
/// Nothing of the request, of the stored user interface or of the clock enters it, so one
/// call's token is never predictable from another's. Only the characters a URL query
/// carries as themselves are kept ([`is_presigned_url_token_char`]), which is a filter over
/// a uniformly drawn secret rather than a derivation from anything.
///
/// `None` when the store minted no secret, when it could not be removed again, or when
/// [`PRESIGNED_URL_TOKEN_MINT_ATTEMPTS`] draws together held too few admissible characters:
/// the call then answers its modelled server error rather than a URL carrying something
/// weaker than the store gives.
pub fn mint_presigned_url_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let mut token = String::with_capacity(PRESIGNED_URL_TOKEN_LEN);
    for _ in 0..PRESIGNED_URL_TOKEN_MINT_ATTEMPTS {
        let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
        let secret = minted
            .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
            .map(str::to_string);
        // The token is not a credential of this emulator; see above.
        if let Err(error) = store.delete(&minted.id) {
            ::tracing::warn!(
                access_key_id = %minted.id,
                error = ?error,
                "a credential minted for a presigned URL token could not be deleted"
            );
            return None;
        }
        // Every admissible character is ASCII, so `len` counts them.
        let room = PRESIGNED_URL_TOKEN_LEN - token.len();
        token.extend(
            secret?
                .chars()
                .filter(|character| is_presigned_url_token_char(*character))
                .take(room),
        );
        if token.len() == PRESIGNED_URL_TOKEN_LEN {
            return Some(token);
        }
    }
    None
}

/// One persistent application user interface, as this service holds it.
///
/// The wire `PersistentAppUI` is a PROJECTION of this record
/// ([`PersistentAppUIRecord::to_wire`]) and so is every answer
/// `GetPersistentAppUIPresignedURL` gives, so two reads of one UI report the same
/// thing and a read reports what the create stored.
#[derive(Debug, Clone)]
pub struct PersistentAppUIRecord {
    pub id: String,
    /// "The unique Amazon Resource Name (ARN) of the target resource."
    /// (`CreatePersistentAppUI$TargetResourceArn` doc.)
    pub target_resource_arn: String,
    /// The EMR on EC2 cluster the target ARN names, when it names one.
    pub cluster_id: Option<String>,
    /// Whether that cluster is runtime role enabled, as
    /// [`ComAmazonawsEmrElasticmapreduceState::is_runtime_role_enabled_cluster`]
    /// computed it when the UI was created.
    pub runtime_role_enabled_cluster: bool,
    /// "The type list for the persistent application user interface object."
    /// (`PersistentAppUI$PersistentAppUITypeList` doc.)
    pub ui_types: Vec<String>,
    /// The Region the creating request was addressed to, which the UI's URL sits in.
    pub region: String,
    /// A [`persistent_app_ui_status`] value.
    pub status: String,
    pub author_id: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub last_state_change_reason: Option<String>,
    pub tags: Vec<Tag>,
    /// When the clock makes the move out of `STARTING` due, cleared once it is applied.
    pub attached_due_at: Option<SystemTime>,
}

impl PersistentAppUIRecord {
    /// Apply the transition the clock has already made due: once `now` has reached the
    /// stored due instant the UI is attached to its target resource, and the instant it
    /// became due is what it reports as last modified — so the answer does not depend
    /// on when it was read.
    pub fn advance_to(&mut self, now: SystemTime) {
        let Some(due) = self.attached_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.attached_due_at = None;
        self.status = persistent_app_ui_status::ATTACHED.to_string();
        self.last_modified_time = due;
        self.last_state_change_reason = Some(format!(
            "The persistent application user interface is attached to {}.",
            self.target_resource_arn
        ));
    }

    /// Whether this UI is attached to its target resource, which is what makes a
    /// presigned URL for it ready.
    pub fn is_attached(&self) -> bool {
        self.status == persistent_app_ui_status::ATTACHED
    }

    /// Whether this UI publishes the `PersistentAppUIType` `ui_type`.
    pub fn serves(&self, ui_type: &str) -> bool {
        self.ui_types.iter().any(|served| served == ui_type)
    }

    /// The UI type a presigned-URL request that names none is answered for: the first
    /// this UI publishes.
    pub fn default_ui_type(&self) -> &str {
        self.ui_types
            .first()
            .map_or(DEFAULT_PERSISTENT_APP_UI_TYPE, String::as_str)
    }

    /// The wire `PersistentAppUI` for this record — what `DescribePersistentAppUI`
    /// answers with.
    pub fn to_wire(&self) -> PersistentAppUI {
        PersistentAppUI {
            persistent_app_ui_id: Some(self.id.clone()),
            persistent_app_ui_type_list: optional_list(&self.ui_types),
            persistent_app_ui_status: Some(self.status.clone()),
            author_id: Some(self.author_id.clone()),
            creation_time: Some(timestamp(self.creation_time)),
            last_modified_time: Some(timestamp(self.last_modified_time)),
            last_state_change_reason: self.last_state_change_reason.clone(),
            tags: optional_list(&self.tags),
        }
    }

    /// The URL of this UI's `ui_type` user interface, for the application
    /// `application_id` when the caller named one, carrying the session token
    /// `session_token` the answering call minted for it.
    ///
    /// Everything but the token is derived from the UI itself — its own id under the
    /// regional EMR application-UI domain, with the UI type as the path — exactly the way a
    /// Studio's access URL is derived from the studio ([`studio_url`]), so two calls for one
    /// UI open the same interface. The TOKEN is what makes the URL a presigned one, and it
    /// is minted per call rather than stored, so no two calls hand back the same credential
    /// and a URL that has been shared or cached cannot outlive the token in it.
    pub fn presigned_url(
        &self,
        ui_type: &str,
        application_id: Option<&str>,
        session_token: &str,
    ) -> String {
        let mut url = format!(
            "https://{}.emrappui-prod.{}.amazonaws.com/{}/",
            self.id,
            self.region,
            ui_type.to_lowercase()
        );
        // "The application ID associated with the presigned URL."
        // (`GetPersistentAppUIPresignedURL$ApplicationId` doc): it selects WHICH
        // application's interface the URL opens, so it is part of the path.
        if let Some(application_id) = application_id {
            url.push_str("history/");
            url.push_str(application_id);
            url.push('/');
        }
        url.push('?');
        url.push_str(PRESIGNED_URL_TOKEN_PARAMETER);
        url.push('=');
        url.push_str(session_token);
        url
    }
}

// ---------------------------------------------------------------------------
// `ListReleaseLabels`: filtering and paging the release catalog
//
// The catalog itself is the one above ([`RELEASE_LINES`] / [`APPLICATION_CATALOG`],
// read through [`catalog_release_labels`] and [`release_line_applications`]); what
// follows is only what the LISTING needs on top of it — the `Application` filter's
// predicate and the page-token codec. `ListReleaseLabels` "retrieves release labels
// of Amazon EMR services in the Region where the API is called", the same answer for
// every caller, so the listing reads the catalog and writes nothing.

/// "Defines the maximum number of release labels to return in a single
/// response. The default is 100." (`ListReleaseLabelsInput$MaxResults` doc.)
/// The modelled `MaxResultsNumber` range is `1..100`, so 100 is both the
/// default and the largest page a caller can ask for.
pub const LIST_RELEASE_LABELS_MAX_RESULTS: i32 = 100;

/// Whether the release `label` bundles the application a `ReleaseLabelFilter`'s
/// `Application` half names.
///
/// "Optional release label application filter. For example, `spark@2.1.0`."
/// (`ReleaseLabelFilter$Application` doc): the bare name matches whatever version of
/// that application the release bundles, `name@version` only that one version. Both
/// halves are compared case-insensitively, so the documented lowercase form matches
/// however the catalog spells a name. A label the catalog does not offer bundles
/// nothing.
pub fn release_bundles_application(label: &str, filter: &str) -> bool {
    let Some(record) = release_label_record(label) else {
        return false;
    };
    let (name, version) = match filter.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (filter, None),
    };
    record
        .applications
        .iter()
        .any(|application| {
            application
                .name
                .as_deref()
                .is_some_and(|bundled| bundled.eq_ignore_ascii_case(name))
                && version.is_none_or(|wanted| {
                    application
                        .version
                        .as_deref()
                        .is_some_and(|bundled| bundled.eq_ignore_ascii_case(wanted))
                })
        })
}

/// A `NextToken` this listing did not mint for the filters it came back with.
#[derive(Debug, Clone, Copy)]
pub struct InvalidNextToken;

/// The marker every `ListReleaseLabels` page token carries, before the filters
/// it is bound to and the label it resumes after. It is what makes a token
/// verifiable: a string that merely happens to be base64 is not a resume point.
const RELEASE_LABEL_TOKEN_PREFIX: &str = "emr/v1/releaseLabels/";

/// The filters a page token is bound to, rendered with each half's length ahead
/// of it so no two different filter pairs render the same string.
pub fn release_label_filter_fingerprint(
    prefix: Option<&str>,
    application: Option<&str>,
) -> String {
    let prefix = prefix.unwrap_or_default();
    let application = application.unwrap_or_default();
    format!(
        "{}:{prefix}|{}:{application}",
        prefix.len(),
        application.len()
    )
}

/// The opaque token that resumes a `fingerprint`-filtered listing after the
/// release label `cursor`.
pub fn encode_release_label_token(fingerprint: &str, cursor: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{RELEASE_LABEL_TOKEN_PREFIX}{fingerprint}/{cursor}"
    ))
}

/// The release label a token resumes after, or [`InvalidNextToken`] when the
/// token was not minted by this listing for these filters.
///
/// "The ListReleaseLabels request fails if the identity (Amazon Web Services
/// account ID) and all filtering parameters are different from the original
/// request, or if the NextToken is expired or tampered with."
/// (`ListReleaseLabelsInput$NextToken` doc) — binding the filters into the token
/// and checking them back out is what detects both.
pub fn decode_release_label_token(
    token: &str,
    fingerprint: &str,
) -> Result<String, InvalidNextToken> {
    let bytes = ::aws_smithy_types::base64::decode(token).map_err(|_| InvalidNextToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidNextToken)?;
    decoded
        .strip_prefix(RELEASE_LABEL_TOKEN_PREFIX)
        .and_then(|rest| rest.strip_prefix(fingerprint))
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_string)
        .ok_or(InvalidNextToken)
}

/// A stored instant as the wire timestamp the model declares.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Whether `id` can name an Amazon EC2 resource of the kind `prefix` names
/// (`vpc`, `subnet`, `sg`).
///
/// An EC2 resource id is its type's prefix, a hyphen, and then the id itself in
/// lowercase hex — 8 characters in the short form and 17 in the long one ("Resource
/// IDs", Amazon EC2 User Guide). A value outside that form cannot be the id of any EC2
/// resource, so an operation given one is naming a resource that does not exist rather
/// than one it could look up.
pub fn is_ec2_resource_id(prefix: &str, id: &str) -> bool {
    let Some(body) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(body.len(), 8 | 17)
        && body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `location` is an Amazon S3 URI naming at least a bucket (`s3://bucket`,
/// optionally with a key), which is the form every EMR member documented as "the
/// Amazon S3 location …" carries.
pub fn is_s3_location(location: &str) -> bool {
    location
        .strip_prefix("s3://")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// A list member is absent on the wire when the cluster has none, rather than present
/// and empty — EMR omits what was never supplied.
fn optional_list<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// The tracing target every audit record below is emitted on, so a host can route or
/// filter the audit stream on its own. Keep it equal to the `target:` literal in
/// [`audit`] — a `tracing` target must be a literal, so the two cannot share one name.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured access record one state-changing call owes.
///
/// Every branch that returns from such a call records it: the operation, the resource it
/// names, the identity the request PRESENTED (its SigV4 access key id, an identifier and
/// not a secret), the request target it named, and the outcome. The subscriber supplies
/// the timestamp. A security configuration is created and deleted by name and a cluster
/// is launched under a minted id, and in both cases the effect on the store is otherwise
/// invisible after the fact, which is what the record attributes. A call that only
/// REPORTS leaves no record: it changed nothing to attribute.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// `OperationContext::parts()` carries the request's method, URI, headers and
/// extensions, and nothing in this framework puts a peer address in them.
///
/// Deliberately metadata only: no configuration document and no part of a cluster's
/// configuration is logged, so a caller's own data never reaches the log. Every
/// interpolated value is request-derived, so [`audit_field`] bounds it and strips the
/// control characters that would otherwise let one request forge a second record.
///
/// This only EMITS: it reads the request's own parts, reaches no service state and takes
/// no lock of its own, so it cannot contend with the store. Handlers nevertheless emit
/// their records with the store guard released, which keeps the emission out of the
/// critical section whatever a subscriber does with it.
pub fn audit(
    operation: &str,
    resource: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        resource = %audit_field(resource),
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        outcome = outcome.as_str(),
        "elasticmapreduce api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// A configuration name, an access key id and a request path are all caller-controlled,
/// so an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a state-changing call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A security configuration was created and stored.
    SecurityConfigurationCreated,
    /// A create re-submitted the document already stored under that name, so nothing
    /// changed (400 is not the answer: the create the caller already made stands).
    SecurityConfigurationUnchanged,
    /// A stored security configuration was deleted.
    SecurityConfigurationDeleted,
    /// A cluster (job flow) was launched and persisted, under the id EMR minted for it.
    ClusterLaunched,
    /// A cluster's shutdown was started: it was moved to `TERMINATING` and the clock owes
    /// it `TERMINATED`, so what the store held for it is going away.
    ClusterTerminationStarted,
    /// A persistent application user interface was created and persisted, under the id
    /// EMR minted for it.
    PersistentAppUICreated,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The name is already taken (400 `InvalidRequestException`).
    Conflict,
    /// A quota — the service's or this emulator's own ceiling — was met.
    QuotaExceeded,
    /// The named resource does not exist.
    NotFound,
    /// The service's own store was unreachable (500).
    StoreUnavailable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SecurityConfigurationCreated => "security-configuration-created",
            Self::SecurityConfigurationUnchanged => "security-configuration-unchanged",
            Self::SecurityConfigurationDeleted => "security-configuration-deleted",
            Self::ClusterLaunched => "cluster-launched",
            Self::ClusterTerminationStarted => "cluster-termination-started",
            Self::PersistentAppUICreated => "persistent-app-ui-created",
            Self::ValidationRefused => "validation-refused",
            Self::Conflict => "conflict",
            Self::QuotaExceeded => "quota-exceeded",
            Self::NotFound => "not-found",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

// ---------------------------------------------------------------------------
// The instance-type catalog
//
// The release catalog above ([`RELEASE_LINES`], [`catalog_release`]) says WHICH
// releases this emulator offers; this one says which Amazon EC2 instance types each of
// those releases supports. It is service data in the same way: no operation creates,
// changes or deletes an instance type, so `ListSupportedInstanceTypes` projects the
// table below and writes nothing. Two emulators built from this source answer the same
// catalog, and reading it changes nothing.
// ---------------------------------------------------------------------------

/// `com.amazonaws.emr#SupportedInstanceType$Architecture`: "The CPU architecture, for
/// example `X86_64` or `AARCH64`."
pub mod architecture {
    pub const X86_64: &str = "X86_64";
    pub const AARCH64: &str = "AARCH64";
}

/// `com.amazonaws.emr#SupportedInstanceType$InstanceFamilyId`: "The Amazon EC2 family and
/// generation for the `SupportedInstanceType`" — the workload class an EMR instance
/// family belongs to crossed with its generation, which is what Amazon EMR reports for
/// this member rather than the family part of the type name.
///
/// SOURCE: the values Amazon EMR reports for `InstanceFamilyId`, which are the workload
/// classes of the Amazon EC2 instance-type naming convention (general purpose, compute
/// optimized, memory optimized, storage optimized, accelerated) under the generation the
/// family belongs to. A catalogued family names its class with [`family_class`] and its
/// generation with [`InstanceFamilySpec::current_generation`]; the two are joined by
/// [`InstanceFamilySpec::instance_family_id`].
pub mod family_class {
    /// A general-purpose family (`m`).
    pub const GENERAL: &str = "GENERAL";
    /// A compute-optimised family (`c`).
    pub const COMPUTE: &str = "COMPUTE";
    /// A memory-optimised family (`r`, `x`, `z`).
    pub const HI_MEM: &str = "HI_MEM";
    /// A storage-optimised family (`i`, `d`, `h`).
    pub const STORAGE: &str = "STORAGE";
    /// An accelerated-computing family (`p`, `g`).
    pub const GPU: &str = "GPU";
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrElasticmapreduceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrElasticmapreduceState>)
}
