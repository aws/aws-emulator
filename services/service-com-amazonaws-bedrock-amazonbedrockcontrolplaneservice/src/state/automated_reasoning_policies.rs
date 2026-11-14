// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy family of [`super`]: the quotas, statuses, spans and
//! refusal sentences of a policy, its versions, its build workflows and its test cases and
//! runs, with the stored record of each.
//!
//! One file per resource family of this service's store, so a rule about a policy's
//! lifecycle is read and changed in one place; the store that holds them, and the lookups
//! that reach them, stay in [`super`].

use super::*;

/// The most Automated Reasoning policies this account may hold at once, across every
/// region it has created one in, past which a create is refused with
/// `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota: without a server-side cap a
/// caller could grow this part of the store with nothing but fresh policy names, or with
/// fresh regions, which is why the bound is over the whole store rather than per region.
pub const MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT: usize = 100;

/// `AutomatedReasoningPolicyDefinition$version` when the request carried no version of
/// its own — the member's modelled `@default("1")`
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyDefinition")`).
pub const POLICY_DEFINITION_FORMAT_VERSION: &str = "1";

/// The most build workflows ONE Automated Reasoning policy may hold at once, past which a
/// start is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, and it is the length the
/// listing that reports them is modelled with: `ListAutomatedReasoningPolicyBuildWorkflows`
/// answers an `AutomatedReasoningPolicyBuildWorkflowSummaries` of `@length(min: 0, max:
/// 1000)`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyBuildWorkflows")`),
/// so a policy that held more could not report them all. Without a server-side cap a
/// caller could also grow this part of the store with nothing but fresh starts.
pub const MAX_BUILD_WORKFLOWS_PER_POLICY: usize = 1000;

/// How long a build workflow dwells in each status of its progression before the clock
/// owes it the next one: the span from the start to [`BUILD_WORKFLOW_STATUS_PREPROCESSING`],
/// and then the span each processing status dwells for
/// ([`AutomatedReasoningPolicyBuildWorkflowClock::armed_at`]).
///
/// The emulator's own span, chosen like [`MODEL_INVOCATION_JOB_COMPLETE_AFTER`] rather than
/// documented by AWS, and chosen for the same reason it is: a build AWS documents as
/// asynchronous — "this initiates the process of analyzing source documents and generating
/// policy rules, variables, and types"
/// (`smithy_query(query="operation",operation="StartAutomatedReasoningPolicyBuildWorkflow")`)
/// — is driven by a client that polls it at intervals of seconds, so a status that dwelt for
/// less than such an interval would be one no such client ever reads the workflow in. Each
/// status of the progression therefore dwells long enough to be read, and no longer: the
/// whole progression is still behind the workflow inside one short polling pass rather than
/// after a wall-clock wait of the size a caller budgets for the interaction.
pub const BUILD_WORKFLOW_STAGE_AFTER: Duration = Duration::from_secs(3);

/// How long after `StartAutomatedReasoningPolicyBuildWorkflow` accepts a build workflow its
/// `status` is owed its move to [`BUILD_WORKFLOW_STATUS_COMPLETED`] — the whole progression,
/// one [`BUILD_WORKFLOW_STAGE_AFTER`] per status it passes through on the way there.
///
/// Derived from that one span rather than written out, and derived WITHOUT narrowing it to
/// whole seconds, so the two constants cannot drift and a sub-second stage span stays the
/// quarter of this one. A stage span so large that four of them are no `Duration` is a
/// progression [`AutomatedReasoningPolicyBuildWorkflowClock::armed_at`] could not arm either,
/// so it fails the BUILD rather than standing for a shorter progression here.
pub const BUILD_WORKFLOW_COMPLETE_AFTER: Duration =
    match BUILD_WORKFLOW_STAGE_AFTER.checked_mul(4) {
        Some(whole) => whole,
        None => panic!("the build workflow stage span must admit the four stages of a build"),
    };

/// `AutomatedReasoningPolicyBuildWorkflowStatus$SCHEDULED` — the status a build workflow a
/// start has just accepted carries, before the clock owes it the first of its processing
/// statuses.
pub const BUILD_WORKFLOW_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$PREPROCESSING` — the first status a
/// WORKING build workflow carries: the source documents the start carried are being read
/// before any rule is generated from them.
pub const BUILD_WORKFLOW_STATUS_PREPROCESSING: &str = "PREPROCESSING";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$BUILDING` — the status a build workflow
/// carries while the policy's rules, variables and types are being generated.
pub const BUILD_WORKFLOW_STATUS_BUILDING: &str = "BUILDING";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$TESTING` — the last status a working build
/// workflow carries: what was built is being checked before the workflow completes.
pub const BUILD_WORKFLOW_STATUS_TESTING: &str = "TESTING";

/// The `AutomatedReasoningPolicyBuildWorkflowStatus` values a WORKING build workflow passes
/// through, in the order the clock owes them
/// ([`AutomatedReasoningPolicyBuildWorkflowClock::armed_at`]).
///
/// The enum carries these three values beside its terminal ones because a build is
/// asynchronous work a caller watches the progress of, so each is a status a read of a
/// started workflow reports while the build is running.
pub const BUILD_WORKFLOW_PROCESSING_STATUSES: &[&str] = &[
    BUILD_WORKFLOW_STATUS_PREPROCESSING,
    BUILD_WORKFLOW_STATUS_BUILDING,
    BUILD_WORKFLOW_STATUS_TESTING,
];

/// `AutomatedReasoningPolicyBuildWorkflowStatus$COMPLETED` — the terminal status the clock
/// owes a build workflow [`BUILD_WORKFLOW_COMPLETE_AFTER`] from its start.
pub const BUILD_WORKFLOW_STATUS_COMPLETED: &str = "COMPLETED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$CANCEL_REQUESTED` — the status a build
/// workflow carries from the `CancelAutomatedReasoningPolicyBuildWorkflow` that asked for
/// its cancellation until the clock owes it [`BUILD_WORKFLOW_STATUS_CANCELLED`]. The enum
/// carries this status beside the terminal one because stopping a running build is
/// asynchronous: the ask is recorded first and the workflow winds down afterwards.
pub const BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED: &str = "CANCEL_REQUESTED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$CANCELLED` — the terminal status the clock
/// owes a build workflow whose cancellation was asked for.
pub const BUILD_WORKFLOW_STATUS_CANCELLED: &str = "CANCELLED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$FAILED` — the other terminal status the
/// enum admits. Nothing in this emulator moves a workflow to it, and it is named here so
/// [`AutomatedReasoningPolicyBuildWorkflowRecord::is_active`] decides on the whole set of
/// terminal statuses rather than on the two this emulator produces.
pub const BUILD_WORKFLOW_STATUS_FAILED: &str = "FAILED";

/// How long after `CancelAutomatedReasoningPolicyBuildWorkflow` accepts a cancellation the
/// workflow's `status` is owed its move from [`BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED`] to
/// [`BUILD_WORKFLOW_STATUS_CANCELLED`].
///
/// The emulator's own span, chosen like [`BUILD_WORKFLOW_STAGE_AFTER`] rather than
/// documented by AWS, and non-zero so a caller reading straight after the cancel still
/// sees the status that cancel recorded.
pub const BUILD_WORKFLOW_CANCELLED_AFTER: Duration = Duration::from_millis(500);

/// The EARLIEST instants a build workflow started at a given moment is owed each of its
/// status transitions at: the whole progression `SCHEDULED -> PREPROCESSING -> BUILDING ->
/// TESTING -> COMPLETED`, each status dwelling its own [`BUILD_WORKFLOW_STAGE_AFTER`] after
/// the one before it.
///
/// The start arms them all and nothing re-counts them afterwards
/// ([`AutomatedReasoningPolicyBuildWorkflowRecord::settle`]), so these are the instants
/// every caller reads the workflow's statuses at, whether it polls once or a hundred times:
/// a status whose instant has elapsed is behind the workflow for everybody. Built like
/// [`ModelInvocationJobClock`], which arms the other asynchronous progression of this
/// service the same way.
#[derive(Debug, Clone, Copy)]
pub struct AutomatedReasoningPolicyBuildWorkflowClock {
    /// When [`BUILD_WORKFLOW_STATUS_PREPROCESSING`] is owed.
    pub preprocessing_at: SystemTime,
    /// When [`BUILD_WORKFLOW_STATUS_BUILDING`] is owed.
    pub building_at: SystemTime,
    /// When [`BUILD_WORKFLOW_STATUS_TESTING`] is owed.
    pub testing_at: SystemTime,
    /// When [`BUILD_WORKFLOW_STATUS_COMPLETED`] is owed.
    pub completed_at: SystemTime,
}

impl AutomatedReasoningPolicyBuildWorkflowClock {
    /// The clock of a workflow started at `start_time`: each transition stands its own span
    /// after the transition before it.
    ///
    /// The instants are strictly increasing, so the progression is walked in order whatever
    /// instant a caller observes it at: a workflow read exactly as one status falls due holds
    /// that status, and one first read long after the start is carried through every status
    /// whose instant has already passed, to the one the elapsed time owes it.
    #[must_use]
    pub fn armed_at(start_time: SystemTime) -> Self {
        let preprocessing_at = start_time + BUILD_WORKFLOW_STAGE_AFTER;
        let building_at = preprocessing_at + BUILD_WORKFLOW_STAGE_AFTER;
        let testing_at = building_at + BUILD_WORKFLOW_STAGE_AFTER;
        let completed_at = testing_at + BUILD_WORKFLOW_STAGE_AFTER;
        Self {
            preprocessing_at,
            building_at,
            testing_at,
            completed_at,
        }
    }
}

/// What an operation ADDRESSED BY a `buildWorkflowId` — the read of a build workflow, its
/// cancel, its delete — answers as its `ResourceNotFoundException` message when this store
/// holds no such workflow.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock names neither
/// the policy the request addressed nor the workflow id it carried, so the wording is the
/// whole requirement and is shared by every operation that ADDRESSES the resource.
///
/// The TEST surface words the same miss differently
/// ([`REQUEST_BUILD_WORKFLOW_NOT_FOUND`]), as the two guardrail sentences differ
/// ([`GUARDRAIL_NOT_FOUND`] against [`REQUEST_GUARDRAIL_NOT_FOUND`]): a resource a request
/// is addressed by and a resource a request NAMES as the subject of other work are reported
/// by different parts of the service, and each has its own recorded sentence.
#[must_use]
pub fn build_workflow_not_found_message() -> &'static str {
    "Build WorkflowExecution not found"
}

/// What the TEST surface — `StartAutomatedReasoningPolicyTestWorkflow` and
/// `GetAutomatedReasoningPolicyTestResult`, whose requests NAME the build workflow the tests
/// are run against — answers as its `ResourceNotFoundException` message when the named
/// workflow is not there.
///
/// RECORDED behaviour, and wholly FIXED: Amazon Bedrock names the resource in the words its
/// own API spells it with and carries neither the policy ARN nor the id the request sent. It
/// is a different sentence from the one an operation addressed BY a workflow id answers
/// ([`build_workflow_not_found_message`]) — the same split the guardrail has — so a caller
/// learns which half of its request named the missing workflow.
pub const REQUEST_BUILD_WORKFLOW_NOT_FOUND: &str = "Build workflow not found";

/// What an unforced `DeleteAutomatedReasoningPolicy` of a policy that still holds an
/// artifact answers as its `ValidationException` message, for the artifact `kind` that is in
/// the way.
///
/// RECORDED behaviour: the sentence is fixed text — "There are " and " that have not been
/// deleted." — around the KIND of artifact, which Amazon Bedrock names in the singular and
/// in the words the operation's own `force` member lists those kinds with ("policy version,
/// test case, test result",
/// `smithy_query(query="doc",shape="DeleteAutomatedReasoningPolicyRequest",member="force")`).
/// Neither the policy ARN nor a count appears in it, so one kind is what a refusal names
/// however many artifacts of that kind the policy holds.
#[must_use]
pub fn automated_reasoning_policy_artifact_not_deleted_message(kind: &str) -> String {
    format!("There are {kind} that have not been deleted.")
}

/// What a write to a build workflow whose `lastUpdatedAt` concurrency token is not the one
/// the stored workflow holds answers as its `ConflictException` message, for the write named
/// by `action` ("deleting").
///
/// Worded as this service words that error class for a child resource of an Automated
/// Reasoning policy ([`automated_reasoning_policy_test_case_conflict_message`]): the
/// conflict, the write it conflicts with, the resource, and then the id and the policy ARN
/// the REQUEST carried — so the wording is fixed and no instant appears in it, neither the
/// one the workflow holds nor the one the request submitted.
///
/// The resource NAME here is the one this conflict refusal carries, and it is not derived
/// from [`build_workflow_not_found_message`]: the two refusals of this resource are worded
/// independently, each from its own recording, so the noun in one is no evidence about the
/// other.
#[must_use]
pub fn automated_reasoning_policy_build_workflow_conflict_message(
    action: &str,
    build_workflow_id: &str,
    policy_arn: &str,
) -> String {
    format!(
        "Conflict with {action} Build WorkflowExecution with buildWorkflowId: \
         {build_workflow_id} and policyArn: {policy_arn}"
    )
}

/// One stored Automated Reasoning policy build workflow: the identity
/// `StartAutomatedReasoningPolicyBuildWorkflow` minted, the document that request named,
/// and the status `GetAutomatedReasoningPolicyBuildWorkflow` reports.
///
/// The source DOCUMENTS themselves are not kept: a build workflow's reads report only a
/// document's name, content type and description
/// (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyBuildWorkflow")`),
/// so holding the up-to-5-MB blob a start carried would grow the store with bytes no
/// operation reads.
///
/// `status` is a `&'static str` for the reason [`ModelImportJobRecord::status`] is: it is a
/// value this emulator derives rather than one a request names.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyBuildWorkflowRecord {
    /// The `AutomatedReasoningPolicyArn` of the policy this workflow builds — the first
    /// half of the key this family's store is keyed by.
    pub policy_arn: String,
    /// `buildWorkflowId` — minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_build_workflow_id`],
    /// and the second half of that key.
    pub id: String,
    /// `buildWorkflowType`, as the start request named it
    /// (`AutomatedReasoningPolicyBuildWorkflowType`).
    pub workflow_type: String,
    /// `AutomatedReasoningPolicyBuildWorkflowStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`BUILD_WORKFLOW_STATUS_PREPROCESSING`], or `None` once that move has been made,
    /// the workflow is terminal or a cancellation has been asked for.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one. The whole progression is armed by
    /// the START ([`AutomatedReasoningPolicyBuildWorkflowClock::armed_at`]) and walked in
    /// order, so each processing status dwells its own span whoever reads the workflow and
    /// however often.
    pub preprocessing_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_BUILDING`], on the
    /// same terms as [`Self::preprocessing_due_at`].
    pub building_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_TESTING`], on the
    /// same terms as [`Self::preprocessing_due_at`].
    pub testing_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_COMPLETED`], or
    /// `None` once it is terminal or a cancellation has been asked for.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_CANCELLED`], or
    /// `None` while no cancellation is in flight.
    pub cancelled_due_at: Option<SystemTime>,
    /// `documentName` of the source document the start carried, or `None` when it carried
    /// no document at all.
    pub document_name: Option<String>,
    /// `documentContentType` of that same document.
    pub document_content_type: Option<String>,
    /// `documentDescription` of that same document, absent when the document carried none.
    pub document_description: Option<String>,
    /// The workflow's ANNOTATION SET: the corrections to the policy's rules, variables and
    /// types this build carries, which `GetAutomatedReasoningPolicyAnnotations` reports
    /// with the hash of the set ([`annotation_set_hash`]).
    ///
    /// The annotations a start carried, or the EMPTY set when it carried none —
    /// `AutomatedReasoningPolicyAnnotationList` is `@length(min: 0, max: 10)`
    /// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyAnnotationList")`), so a
    /// workflow nothing has annotated holds a set the shape admits rather than no set.
    ///
    /// The hash is NOT held beside the set: it is a function of the set's content alone, so
    /// deriving it on each read is what keeps the two from ever disagreeing.
    pub annotations: Vec<AutomatedReasoningPolicyAnnotation>,
    /// When [`Self::annotations`] was last written — the instant the start recorded them,
    /// until a write to the set replaces them.
    ///
    /// Beside the set rather than taken from [`Self::updated_at`], because the workflow's
    /// own clock moves that instant on every status transition while the annotations stay
    /// as they were written.
    pub annotations_updated_at: SystemTime,
    pub created_at: SystemTime,
    /// When the workflow last changed: the start instant until the clock moves its status.
    pub updated_at: SystemTime,
}

impl AutomatedReasoningPolicyBuildWorkflowRecord {
    /// Whether this workflow is still working, so the policy it builds is in use by it.
    ///
    /// A workflow whose cancellation has been asked for is still winding down, so it is
    /// still working: only a terminal status frees the policy it builds.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(
            self.status,
            BUILD_WORKFLOW_STATUS_COMPLETED
                | BUILD_WORKFLOW_STATUS_CANCELLED
                | BUILD_WORKFLOW_STATUS_FAILED
        )
    }

    /// Whether a cancellation has already taken this workflow over: it is winding down, or
    /// it has wound down.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.status == BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED
            || self.status == BUILD_WORKFLOW_STATUS_CANCELLED
    }

    /// Materialise every status transition the clock has already made due at `now`: the
    /// processing statuses and then the completion a start owes a workflow, or the
    /// cancellation a cancel owes one.
    ///
    /// The progression runs on the START's own timeline: the start fixed the instant of each
    /// status ([`AutomatedReasoningPolicyBuildWorkflowClock::armed_at`]) and nothing
    /// re-counts them, so what a workflow's status is at `now` is decided by the time
    /// elapsed since it was started and by nothing else — not by how many times, or how
    /// often, anybody read it. A workflow polled while it runs observes each processing
    /// status in turn, because each dwells its own span; one first read long after the start
    /// is carried straight to the status that elapsed time owes it.
    ///
    /// The statuses are SEQUENTIAL and their instants strictly increasing, so the walk stops
    /// at the first one still ahead of `now` and never materialises a later status before an
    /// earlier one.
    ///
    /// The only write a reporting operation on this resource makes: each move was owed at
    /// its due instant even if nobody had called, which is why that instant — not `now` — is
    /// the one the workflow reports as its last update, and why settling twice over reports
    /// the same instant. The completion and the cancellation are mutually exclusive: a
    /// cancel drops every transition the start had owed ([`Self::cancel`]).
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.preprocessing_due_at {
            if now < due {
                return;
            }
            self.status = BUILD_WORKFLOW_STATUS_PREPROCESSING;
            self.preprocessing_due_at = None;
            self.updated_at = due;
        }
        if let Some(due) = self.building_due_at {
            if now < due {
                return;
            }
            self.status = BUILD_WORKFLOW_STATUS_BUILDING;
            self.building_due_at = None;
            self.updated_at = due;
        }
        if let Some(due) = self.testing_due_at {
            if now < due {
                return;
            }
            self.status = BUILD_WORKFLOW_STATUS_TESTING;
            self.testing_due_at = None;
            self.updated_at = due;
        }
        if let Some(due) = self.completed_due_at {
            if now < due {
                return;
            }
            self.status = BUILD_WORKFLOW_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.updated_at = due;
            return;
        }
        if let Some(due) = self.cancelled_due_at.filter(|due| now >= *due) {
            self.status = BUILD_WORKFLOW_STATUS_CANCELLED;
            self.cancelled_due_at = None;
            self.updated_at = due;
        }
    }

    /// Forget every transition the clock still owes this workflow, so its `status` is the
    /// one it holds now and no span moves it again.
    ///
    /// What a workflow that has stopped progressing on its own is left owing: the write
    /// [`Self::cancel`] makes before it arms the cancellation, and what a workflow stored
    /// directly in a status of the enum this emulator's clock never leaves one in holds.
    pub fn forget_owed_transitions(&mut self) {
        self.preprocessing_due_at = None;
        self.building_due_at = None;
        self.testing_due_at = None;
        self.completed_due_at = None;
        self.cancelled_due_at = None;
    }

    /// Move this workflow to [`BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED`] and record when it
    /// is owed its move to [`BUILD_WORKFLOW_STATUS_CANCELLED`].
    ///
    /// The caller's own write, asked for by
    /// `CancelAutomatedReasoningPolicyBuildWorkflow` — not a clock transition. The build it
    /// was running is being stopped, so the workflow is owed neither the rest of its
    /// progression nor its completion any more.
    pub fn cancel(&mut self, now: SystemTime) {
        self.forget_owed_transitions();
        self.status = BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED;
        self.cancelled_due_at = Some(now + BUILD_WORKFLOW_CANCELLED_AFTER);
        self.updated_at = now;
    }
}

/// A stored Automated Reasoning policy: the identity `CreateAutomatedReasoningPolicy`
/// minted, the draft definition every later read of that policy reports, and every
/// numbered version snapshotted from that draft.
///
/// The `definitionHash` is held beside the definition rather than recomputed per read, so
/// the hash the create answered is the hash the read reports; it is a function of the
/// DEFINITION alone ([`policy_definition_hash`]), which is what makes it usable as the
/// concurrency token its member documents.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyRecord {
    /// The `AutomatedReasoningPolicyArn` this policy is addressed by, minted by
    /// [`automated_reasoning_policy_arn`] — also the key this family's store is keyed by.
    pub arn: String,
    /// `policyId` — the resource half of [`Self::arn`] (`^[a-z0-9]{12}$`), which
    /// `GetAutomatedReasoningPolicy` reports beside it.
    pub id: String,
    /// `name`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`CustomModelRecord::region`] is.
    pub region: String,
    /// `description`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `policyDefinition`, as the create request carried it (absent when it carried none,
    /// which is how a policy whose rules a build workflow is to extract starts out).
    pub definition: Option<AutomatedReasoningPolicyDefinition>,
    /// `definitionHash` — [`policy_definition_hash`] of [`Self::definition`].
    pub definition_hash: String,
    /// `kmsKeyArn` — the customer managed key the create named, completed to a `KmsKeyArn`
    /// by [`kms_key_arn`], or `None` when the create named none (Amazon Bedrock then uses
    /// an AWS managed key, which is no key of this account to report).
    pub kms_key_arn: Option<String>,
    /// `tags` — the tags the create request attached to the policy.
    ///
    /// The ONE copy of this DRAFT's tags: the list the create filled is the list
    /// `ops/tag_resource.rs` and `ops/untag_resource.rs` rewrite and
    /// `ops/list_tags_for_resource.rs` reports, reached through
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::tagged_resource`] under
    /// the policy's unversioned ARN. The policy's own reads report no tag member, so a
    /// tagging call is where they are read back.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// When the policy was last written. The creation instant until an update rewrites
    /// the draft.
    pub updated_at: SystemTime,
    /// Every numbered version cut from this policy's draft, keyed by its version number —
    /// the `:<version>` suffix an `AutomatedReasoningPolicyArn` carries.
    ///
    /// Held INSIDE the policy record, like a guardrail's [`Guardrail::versions`], so a
    /// version can neither outlive the policy it was cut from nor be reached under another
    /// one. A version is a SNAPSHOT: nothing rewrites an entry once it is inserted, which
    /// is what makes a later write to the draft invisible to it.
    pub versions: BTreeMap<u32, AutomatedReasoningPolicyVersionRecord>,
    /// `CreateAutomatedReasoningPolicyVersion` idempotency: `clientRequestToken` -> the
    /// version number that token cut.
    ///
    /// Inside the record for the reason [`Guardrail::version_tokens`] is: it holds at most
    /// one entry per version of this policy, and dies with the policy by construction.
    pub version_tokens: HashMap<String, u32>,
    /// The number `CreateAutomatedReasoningPolicyVersion` will mint next, from `1` upwards
    /// — the smallest number `AutomatedReasoningPolicyVersion` (`^([1-9][0-9]{0,11})$`)
    /// admits.
    pub next_version: u32,
}

impl AutomatedReasoningPolicyRecord {
    /// The version `number` names, or `None` when this policy has cut no such version.
    #[must_use]
    pub fn version(&self, number: u32) -> Option<&AutomatedReasoningPolicyVersionRecord> {
        self.versions.get(&number)
    }

    /// Remove version `number` and release the idempotency tokens that named it, through
    /// the one rule [`drop_version`] states for both versioned resources of this service.
    pub fn remove_version(
        &mut self,
        number: u32,
    ) -> Option<AutomatedReasoningPolicyVersionRecord> {
        drop_version(&mut self.versions, &mut self.version_tokens, number)
    }
}

/// The most numbered versions one Automated Reasoning policy may hold at once, past which
/// cutting another is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota: version numbers are never
/// reused, so even with `DeleteAutomatedReasoningPolicy` removing one version at a time a
/// caller could grow one policy's snapshots without end were there no server-side cap.
pub const MAX_AUTOMATED_REASONING_POLICY_VERSIONS_PER_POLICY: usize = 100;

/// One numbered version of an Automated Reasoning policy: the snapshot
/// `CreateAutomatedReasoningPolicyVersion` cut from the policy's draft, which every later
/// read of that version reports.
///
/// The definition and its hash are COPIED out of the draft at the instant the version was
/// cut rather than borrowed from it, which is what makes the version immutable: a later
/// write to the draft changes the draft's hash and leaves every version's alone.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyVersionRecord {
    /// The versioned `AutomatedReasoningPolicyArn` this version is addressed by, minted by
    /// [`automated_reasoning_policy_version_arn`].
    pub arn: String,
    /// `version` — the number this snapshot was cut as, from `1` upwards.
    pub version: u32,
    /// `name`, as the policy carried it when the version was cut.
    pub name: String,
    /// `description`, as the policy carried it when the version was cut (absent when it
    /// carried none).
    pub description: Option<String>,
    /// The policy definition this version froze, absent when the draft carried none.
    pub definition: Option<AutomatedReasoningPolicyDefinition>,
    /// `definitionHash` — [`policy_definition_hash`] of [`Self::definition`], so the hash
    /// the version answers is the hash of what it froze.
    pub definition_hash: String,
    /// `tags` — the tags the create-version request attached to this version.
    ///
    /// The ONE copy of this VERSION's tags, reached by a tagging call under the versioned
    /// ARN exactly as the draft's are reached under the unversioned one
    /// ([`AutomatedReasoningPolicyRecord::tags`]) — a version is its own taggable revision,
    /// so what is tagged on it is not on the draft.
    pub tags: Vec<Tag>,
    /// `createdAt` — when this version was cut. Also the instant a read reports as its
    /// last update, since nothing rewrites a version.
    pub created_at: SystemTime,
}

/// The most tests one Automated Reasoning policy may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// `AutomatedReasoningPolicyTestCaseList` is `@length(0, 1000)`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyTestCases")`), so
/// a policy holding more tests than this could not be listed; the bound is also what keeps
/// this part of the store from growing without end, since a test carries no name a second
/// create of it could collide with.
pub const MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY: usize = 1000;

/// What a request addressing a test this store does not hold answers as its
/// `ResourceNotFoundException` message, wherever that request came from: a read of the test
/// itself, or a read of a run of it.
///
/// RECORDED behaviour: Amazon Bedrock names the test and then the policy the request
/// addressed, so the wording is fixed and both ids are variable parts taken from the
/// request. Stated once here because the message belongs to the RESOURCE rather than to one
/// operation, so every operation that addresses a test refuses it the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case_not_found_message(
    test_case_id: &str,
    policy_arn: &str,
) -> String {
    format!(
        "The TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn} does not exist"
    )
}

/// What a write to a test whose `lastUpdatedAt` concurrency token is not the one the stored
/// test holds answers as its `ConflictException` message, for the write named by `action`
/// ("deleting", "updating").
///
/// RECORDED behaviour: Amazon Bedrock states the conflict, the write it conflicts with, and
/// then the same two ids
/// [`automated_reasoning_policy_test_case_not_found_message`] carries — so the wording is
/// fixed, its variable parts are the ids the REQUEST carried, and no instant appears in it:
/// neither the one the test holds nor the one the request submitted. Stated once here
/// because every write of a test is held against that token the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case_conflict_message(
    action: &str,
    test_case_id: &str,
    policy_arn: &str,
) -> String {
    format!("Conflict with {action} TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn}")
}

/// The `ConflictException` message a request whose `lastUpdatedDefinitionHash` is not the
/// DRAFT policy's current definition hash is refused with.
///
/// RECORDED behaviour: Amazon Bedrock states that the hash the request PROVIDED is not the
/// draft's — a wholly fixed sentence, carrying neither the policy ARN nor either hash — so
/// an optimistic-concurrency client matches it to decide "re-read the policy and retry".
pub const AUTOMATED_REASONING_POLICY_DEFINITION_HASH_CONFLICT: &str =
    "Provided definition hash does not match the DRAFT policy definition hash.";

/// The `ResourceInUseException` message a request to start a test workflow for a build
/// workflow whose tests are still running is refused with.
///
/// RECORDED behaviour: a wholly fixed sentence — no build workflow id and no status name, so
/// the refusal couples a caller to nothing but the fact that testing is under way, which is
/// what it backs off and retries on.
pub const AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_IN_PROGRESS: &str =
    "Testing workflow is currently in progress";

/// What a request whose `AutomatedReasoningPolicyArn` carries a `:<version>` suffix the
/// addressed policy has not cut answers as its `ResourceNotFoundException` message.
///
/// RECORDED behaviour: Amazon Bedrock reports the missing VERSION — a wholly fixed sentence,
/// carrying neither the ARN nor the number — rather than reporting the policy as absent, so
/// a caller learns the policy is there and the revision it addressed is not.
pub const AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND: &str = "Policy version does not exist";

/// What a request addressing an Automated Reasoning policy this account holds no record of
/// answers as its `ResourceNotFoundException` message: a read of the policy, a listing of
/// its children, or a call addressing one of those children.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock reports that the
/// policy is not there and names neither the ARN the request carried nor the account it
/// looked in, so the wording is the whole requirement. Worded as its sibling
/// [`AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`] is, which reports the missing VERSION of
/// a policy that IS there. The two writes of the POLICY ITSELF word this same error class
/// their own way ([`AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_WRITE`]), and so does the
/// LISTING of the policies themselves
/// ([`AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_LISTING`]).
pub const AUTOMATED_REASONING_POLICY_NOT_FOUND: &str = "Policy does not exist";

/// What a write of the POLICY ITSELF — `UpdateAutomatedReasoningPolicy` rewriting its draft,
/// `DeleteAutomatedReasoningPolicy` removing it — answers as its `ResourceNotFoundException`
/// message for an `AutomatedReasoningPolicyArn` this account holds no policy under.
///
/// RECORDED behaviour, and a message with NO variable parts like its sibling
/// [`AUTOMATED_REASONING_POLICY_NOT_FOUND`]: Amazon Bedrock words this error class
/// differently for the two calls that write the policy as a whole, so the two sentences are
/// held apart here rather than one standing for both. The calls that address a CHILD of the
/// policy — its tests, its build workflows, its versions — report the absent policy in the
/// other sentence, whether they read or write that child.
pub const AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_WRITE: &str = "Policy was not found";

/// What `ListAutomatedReasoningPolicies` answers as its `ResourceNotFoundException` message
/// when its `policyArn` FILTER names a policy this account holds no record of.
///
/// RECORDED behaviour, and a message with NO variable parts like its two siblings
/// [`AUTOMATED_REASONING_POLICY_NOT_FOUND`] and
/// [`AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_WRITE`]: Amazon Bedrock words this error class
/// per operation, and the policy LISTING words it as a terminated sentence — the same words
/// a read of the policy answers, carrying the full stop that read's wording does not. The
/// three sentences are held apart here, one per wording, rather than one standing for the
/// others, so changing what an operation answers never silently changes what another one
/// answers.
pub const AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_LISTING: &str = "Policy does not exist.";

/// One stored Automated Reasoning policy test: the identity
/// `CreateAutomatedReasoningPolicyTestCase` minted, and the sample input and expected
/// outcome every later read of that test reports.
///
/// Every member is held exactly as the request carried it, because the test is what a
/// policy is validated against: `guardContent` and `queryContent` are opaque text whose
/// characters are the test, so they are stored byte for byte rather than normalised.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyTestCaseRecord {
    /// `testCaseId` — the `^[0-9A-Z]{12}$` identity minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_automated_reasoning_policy_test_case_id`].
    pub id: String,
    /// `guardContent` — the output content the policy validates, as the request spelled it.
    pub guard_content: String,
    /// `queryContent` — the query that produced that content, absent when the request
    /// carried none.
    pub query_content: Option<String>,
    /// `expectedAggregatedFindingsResult` — the `AutomatedReasoningCheckResult` the test
    /// expects. Required on the create, so a stored test always carries one.
    pub expected_aggregated_findings_result: String,
    /// `confidenceThreshold` — the minimum translation confidence this test validates at,
    /// absent when the request carried none.
    pub confidence_threshold: Option<f64>,
    pub created_at: SystemTime,
    /// When the test was last written. The creation instant until an update rewrites it.
    pub updated_at: SystemTime,
    /// The `clientRequestToken` the write this test currently holds was made under, when
    /// that write was an UPDATE that carried one, and `None` while the test still holds
    /// what the create wrote.
    ///
    /// `UpdateAutomatedReasoningPolicyTestCase$clientRequestToken` is an
    /// `@idempotencyToken` whose purpose is that "the operation completes no more than one
    /// time", so the token that produced the current write is kept WITH that write: a
    /// retried update carries it beside the `lastUpdatedAt` the retried request was built
    /// from, which the landed write has already moved past. Holding it here — rather than
    /// in a map beside the store — is what lets the retry be recognised and ignored, and
    /// makes the token go with the test when the test is deleted.
    pub update_client_request_token: Option<String>,
}

/// `AutomatedReasoningPolicyTestRunStatus$SCHEDULED` — the status a test run that a test
/// workflow has just accepted carries, before the clock owes it its completion
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyTestRunStatus")`).
pub const TEST_RUN_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `AutomatedReasoningPolicyTestRunStatus$COMPLETED` — the terminal status the clock owes a
/// scheduled test run [`TEST_RUN_COMPLETE_AFTER`] from the workflow that scheduled it.
pub const TEST_RUN_STATUS_COMPLETED: &str = "COMPLETED";

/// `AutomatedReasoningPolicyTestRunStatus$FAILED` — the other terminal status the enum
/// admits. Nothing in this emulator moves a run to it, and it is named here so
/// [`AutomatedReasoningPolicyTestRunRecord::is_active`] decides on the whole set of
/// terminal statuses rather than on the one this emulator produces.
pub const TEST_RUN_STATUS_FAILED: &str = "FAILED";

/// How long after `StartAutomatedReasoningPolicyTestWorkflow` accepts a test run its
/// `testRunStatus` is owed its move to [`TEST_RUN_STATUS_COMPLETED`].
///
/// The emulator's own span, chosen like [`BUILD_WORKFLOW_COMPLETE_AFTER`] rather than
/// documented by AWS: a run the workflow answers in a non-terminal status is owed a
/// transition, and the span is non-zero so a caller reading straight after the start still
/// sees the status that start recorded.
pub const TEST_RUN_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// One run of one stored test against one build workflow of its policy: what
/// `StartAutomatedReasoningPolicyTestWorkflow` schedules and
/// `GetAutomatedReasoningPolicyTestResult` reports.
///
/// A run is keyed by the TRIPLE `(policyArn, buildWorkflowId, testCaseId)` its read is
/// addressed by (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyTestResult")`),
/// so running the same test against two build workflows of a policy is two runs and
/// re-running it against one build workflow replaces that build workflow's run of it.
///
/// The test's own content is not copied here: a result reports the stored test
/// ([`AutomatedReasoningPolicyTestCaseRecord`]) beside the run, so there is only ever one
/// copy of a test whichever operation reads it.
///
/// `status` is a `&'static str` for the reason
/// [`AutomatedReasoningPolicyBuildWorkflowRecord::status`] is: it is a value this emulator
/// derives rather than one a request names.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyTestRunRecord {
    /// The `AutomatedReasoningPolicyArn` of the policy whose test this run ran — the first
    /// third of the key this family's store is keyed by.
    pub policy_arn: String,
    /// The `buildWorkflowId` of the build workflow this run tested — the second third of
    /// that key.
    pub build_workflow_id: String,
    /// The `testCaseId` of the test this run ran — the last third of that key.
    pub test_case_id: String,
    /// `testRunStatus` (`AutomatedReasoningPolicyTestRunStatus`).
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`TEST_RUN_STATUS_COMPLETED`], or `None`
    /// once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`AutomatedReasoningPolicyBuildWorkflowRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// `updatedAt` — when this run last changed: the instant the workflow scheduled it
    /// until the clock moves its status.
    pub updated_at: SystemTime,
}

/// The test workflow a build workflow last accepted: the `clientRequestToken` that request
/// carried and the `testCaseIds` it selected (`None` when it named none, which selects
/// every test of the policy).
///
/// What `StartAutomatedReasoningPolicyTestWorkflow` recognises a request it has already
/// accepted by, which is why the selection is held beside the token: a request naming other
/// tests asks for other work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedAutomatedReasoningPolicyTestWorkflow {
    /// `clientRequestToken`, as the accepted request carried it.
    pub client_request_token: String,
    /// `testCaseIds`, as that request carried it.
    pub test_case_ids: Option<Vec<String>>,
}

impl AutomatedReasoningPolicyTestRunRecord {
    /// One run of `test_case_id` against `build_workflow_id`, as the test workflow that
    /// accepted it at `now` leaves it: the ONE place a run's initial state is written, so a
    /// fresh run and a re-run start alike (both go on through [`Self::schedule`]).
    #[must_use]
    pub fn scheduled(
        policy_arn: &str,
        build_workflow_id: &str,
        test_case_id: &str,
        now: SystemTime,
    ) -> Self {
        let mut record = Self {
            policy_arn: policy_arn.to_string(),
            build_workflow_id: build_workflow_id.to_string(),
            test_case_id: test_case_id.to_string(),
            status: TEST_RUN_STATUS_SCHEDULED,
            completed_due_at: None,
            updated_at: now,
        };
        record.schedule(now);
        record
    }

    /// Whether this run is still running, so the build workflow it is testing is in use by
    /// it: only a terminal status frees that build workflow for another test workflow.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(
            self.status,
            TEST_RUN_STATUS_COMPLETED | TEST_RUN_STATUS_FAILED
        )
    }

    /// Materialise the status transition the clock has already made due at `now`: the
    /// completion the workflow that scheduled this run owes it.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the run reports as its last update.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = TEST_RUN_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.updated_at = due;
        }
    }

    /// Schedule this run as of `now`: the write
    /// `StartAutomatedReasoningPolicyTestWorkflow` makes, for a run it has just accepted
    /// and for one it is re-running.
    pub fn schedule(&mut self, now: SystemTime) {
        self.status = TEST_RUN_STATUS_SCHEDULED;
        self.completed_due_at = Some(now + TEST_RUN_COMPLETE_AFTER);
        self.updated_at = now;
    }
}
