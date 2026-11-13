// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codecommit-codecommit-20150413` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

pub mod conflict_resolution;
pub mod git_authorization;
pub mod git_http;
pub mod git_pack;

/// The first label of the public CodeCommit git host
/// (`git-codecommit.<region>.amazonaws.com`), which is what the framework
/// resolves as a route key for a client that was handed that host and had it
/// pointed at this emulator. It names no emulated service — CodeCommit's own
/// signing name is `codecommit` — so a git request carrying it is this service's
/// to claim.
const GIT_HOST_LABEL: &str = "git-codecommit";

/// Run `work` without occupying an async worker.
///
/// The git transport's work is CPU-bound and holds this service's store lock, and
/// it is driven from a SYNCHRONOUS hook (`override_routing_error`), so it cannot
/// be handed to `spawn_blocking(..).await` the way an async handler elsewhere in
/// this workspace hands its interpreter work. `block_in_place` is the construct
/// for that case: it moves the current task off the worker thread so the runtime
/// keeps serving every other request while this one runs. It is available only on
/// a multi-thread runtime — the one the emulator boots — so on a current-thread
/// runtime (a `#[tokio::test]`, or a caller driving the router in-process) the work
/// runs inline, where there is no other worker to protect.
///
/// `pub(crate)` because a modeled handler whose CPU-bound work also runs under the
/// store lock uses the same construct for the same reason: a merge evaluation
/// (`crate::ops::get_merge_options`, `crate::ops::get_merge_conflicts`) compares
/// two tree snapshots against their merge base and may merge file content, and a
/// merge that WRITES its result
/// (`crate::ops::create_unreferenced_merge_commit`, through `crate::state::merge`)
/// walks history and runs a diff3 over whole blobs — each holding the guard it
/// reads them through for that whole span, so each takes this function plus its
/// own wall-clock deadline.
pub(crate) fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The `com-amazonaws-codecommit-codecommit-20150413` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same shared engine the generated entry constructs each operation from.
    ///
    /// The git smart-HTTP transport ([`git_http`]) is not a modeled operation —
    /// git's wire protocol is not in the Smithy model at all — so it is served
    /// from `override_routing_error` below, which is handed the request but no
    /// operation. It reaches this service's store through this handle, exactly as
    /// a handler reaches it through its own `self.engine`.
    engine: Arc<Eng>,
    /// The refusal [`Service::override_operation`] hands back for a merge request
    /// whose `conflictResolution` its strategy cannot honour
    /// ([`conflict_resolution`]). Held on the service because that seam hands the
    /// router a BORROW of an operation; it carries no state of its own.
    unhonourable_conflict_resolution: conflict_resolution::UnhonourableConflictResolution<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unhonourable_conflict_resolution:
                conflict_resolution::UnhonourableConflictResolution::new(),
        }
    }
}

/// The error CodeCommit names for a rejected member of a `RepositoryTriggersList`,
/// or `None` to leave the framework's wording in place.
///
/// A trigger's members sit inside a list of structures, so each is a LEAF of a path
/// like `triggers.1.member.events.1.member` rather than a top-level member, and the
/// trigger operations name an error per member of the trigger exactly as the rest of
/// the service names one per top-level member. The wording comes from
/// [`crate::state::TriggerProblem`], the one place this crate words a trigger
/// refusal, so a configuration the BOUNDARY rejects is refused in the same terms as
/// one a handler rejects.
fn trigger_refusal_for(violation: &Violation) -> Option<Refused> {
    use crate::state::TriggerProblem;

    let missing = matches!(violation.kind, ViolationKind::MissingRequiredMember);
    let names = |member: &str| violation.path.split('.').any(|segment| segment == member);
    let (code, message) = match &violation.kind {
        // The list itself, rather than anything inside one of its entries.
        _ if missing && violation.path == "triggers" => (
            "RepositoryTriggersListRequiredException",
            crate::state::REPOSITORY_TRIGGERS_LIST_REQUIRED_MESSAGE.to_string(),
        ),
        _ if missing && names("name") => (
            "RepositoryTriggerNameRequiredException",
            TriggerProblem::NameMissing.message(),
        ),
        _ if missing && names("destinationArn") => (
            "RepositoryTriggerDestinationArnRequiredException",
            TriggerProblem::DestinationArnMissing.message(),
        ),
        _ if missing && names("events") => (
            "RepositoryTriggerEventsListRequiredException",
            TriggerProblem::EventsMissing.message(),
        ),
        // `events` is the one enumerated member a trigger carries, so a value
        // outside an enum inside a trigger is an event that is not one.
        ViolationKind::EnumValue(_) => (
            "InvalidRepositoryTriggerEventsException",
            TriggerProblem::EventsInvalid.message(),
        ),
        _ if names("events") => (
            "InvalidRepositoryTriggerEventsException",
            TriggerProblem::EventsInvalid.message(),
        ),
        _ if names("branches") => (
            "InvalidRepositoryTriggerBranchNameException",
            TriggerProblem::BranchNameInvalid.message(),
        ),
        _ if names("customData") => (
            "InvalidRepositoryTriggerCustomDataException",
            TriggerProblem::CustomDataInvalid.message(),
        ),
        _ if names("name") => (
            "InvalidRepositoryTriggerNameException",
            TriggerProblem::NameInvalid.message(),
        ),
        _ if names("destinationArn") => (
            "InvalidRepositoryTriggerDestinationArnException",
            TriggerProblem::DestinationArnInvalid.message(),
        ),
        // Anything else about the list keeps the framework's own wording: the model
        // declares no error for the trigger list being malformed as a whole.
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message,
    })
}

/// The error CodeCommit names for one rejected member, or `None` to leave the
/// framework's wording in place.
///
/// Each code is an error shape this service's model declares, and each message is
/// the text the SERVICE answers that code with: that shape's own documented
/// sentence (the `//! ## Errors` list in `src/ops/<operation>.rs`), or, where this
/// crate words the code in one place for every operation that raises it, that
/// wording from `crate::state`. Nothing is appended: a message naming the member at
/// fault, or the validator that rejected it, would mark the refusal as the
/// emulator's rather than the service's, and no AWS client-error message takes that
/// shape.
fn refusal_for(violation: &Violation) -> Option<Refused> {
    // A nested path names its top-level member first (`tags.aws:x.member`), and a
    // member of a nested shape is its LAST segment
    // (`location.relativeFileVersion`); CodeCommit names the error for the member
    // actually at fault, so the table below is keyed on the top-level member and,
    // for a nested shape's own member, on that leaf.
    let member = violation.path.split('.').next().unwrap_or_default();
    let leaf = violation.path.rsplit('.').next().unwrap_or_default();
    let missing = matches!(violation.kind, ViolationKind::MissingRequiredMember);
    // A `triggers` list is the one member whose ENTRIES carry members of their own
    // that the service names an error for, so it is answered by its own table.
    if member == "triggers" {
        return trigger_refusal_for(violation);
    }
    let (code, requirement) = match (member, &violation.kind) {
        // A `repositoryName` names a repository wherever it sits: as the top-level
        // member of most operations, and as a member of the nested `Target` shape a
        // pull request's target list carries. One row speaks for both, so the
        // wording for this code lives in one place.
        (_, _) if missing && (member == "repositoryName" || leaf == "repositoryName") => (
            "RepositoryNameRequiredException",
            crate::state::REPOSITORY_NAME_REQUIRED_MESSAGE,
        ),
        (_, _) if member == "repositoryName" || leaf == "repositoryName" => (
            "InvalidRepositoryNameException",
            crate::state::INVALID_REPOSITORY_NAME_MESSAGE,
        ),
        // `UpdateRepositoryName` addresses the repository in `oldName` and names
        // the name it asks for in `newName`; both carry a `RepositoryName`, so
        // both take the repository-name errors, exactly as the member called
        // `repositoryName` does.
        ("oldName" | "newName", _) if missing => (
            "RepositoryNameRequiredException",
            crate::state::REPOSITORY_NAME_REQUIRED_MESSAGE,
        ),
        ("oldName" | "newName", _) => (
            "InvalidRepositoryNameException",
            crate::state::INVALID_REPOSITORY_NAME_MESSAGE,
        ),
        ("repositoryNames", _) if missing => (
            "RepositoryNamesRequiredException",
            "A repository names object is required, but was not specified.",
        ),
        ("repositoryDescription", _) => (
            "InvalidRepositoryDescriptionException",
            "The specified repository description is not valid.",
        ),
        // Every member that carries a `BranchName` names a branch, so each takes the
        // branch errors: `defaultBranchName` is `UpdateDefaultBranch`'s name for one.
        ("branchName" | "defaultBranchName", _) if missing => (
            "BranchNameRequiredException",
            "A branch name is required, but was not specified.",
        ),
        // A `branchName` that is PRESENT but empty is refused by `BranchName`'s
        // `@length min 1`, and CodeCommit words that case with its own sentence
        // rather than the generic not-a-valid-reference-name text.
        ("branchName" | "defaultBranchName", ViolationKind::MinLength(_)) => (
            "InvalidBranchNameException",
            crate::state::EMPTY_BRANCH_NAME_MESSAGE,
        ),
        ("branchName" | "defaultBranchName", _) => (
            "InvalidBranchNameException",
            "The specified reference name is not valid.",
        ),
        ("filePath" | "folderPath", _) if missing => (
            "PathRequiredException",
            "The folderPath for a location cannot be null.",
        ),
        ("filePath" | "folderPath", _) => {
            ("InvalidPathException", "The specified path is not valid.")
        }
        ("fileContent", _) if missing => (
            "FileContentRequiredException",
            "The file cannot be added because it is empty.",
        ),
        ("fileContent", ViolationKind::MaxLength(_)) => (
            "FileContentSizeLimitExceededException",
            "The file cannot be added because it is too large.",
        ),
        ("fileMode", ViolationKind::EnumValue(_)) => (
            "InvalidFileModeException",
            "The specified file mode permission is not valid.",
        ),
        // `fileMode` is also a member of the nested `SetFileModeEntry` and
        // `ReplaceContentEntry` of a merge's `conflictResolution`, where it is the
        // path's LEAF rather than its top-level member.
        (_, _) if leaf == "fileMode" && missing => (
            "FileModeRequiredException",
            "A file mode is required to update mode permissions for a file.",
        ),
        (_, ViolationKind::EnumValue(_)) if leaf == "fileMode" => (
            "InvalidFileModeException",
            "The specified file mode permission is not valid.",
        ),
        ("parentCommitId", _) if missing => (
            "ParentCommitIdRequiredException",
            "A parent commit ID is required.",
        ),
        ("parentCommitId", _) => (
            "InvalidParentCommitIdException",
            "The parent commit ID is not valid.",
        ),
        ("commitId" | "afterCommitId" | "beforeCommitId", _) if missing => (
            "CommitIdRequiredException",
            crate::state::COMMIT_ID_REQUIRED_MESSAGE,
        ),
        ("commitId" | "afterCommitId" | "beforeCommitId", _) => (
            "InvalidCommitIdException",
            crate::state::INVALID_COMMIT_ID_MESSAGE,
        ),
        // Every member that carries a `CommentId` names a comment, so each takes the
        // comment id errors: `inReplyTo` is `PostCommentReply`'s name for one, and it
        // models these same codes for it.
        ("commentId" | "inReplyTo", _) if missing => (
            "CommentIdRequiredException",
            "The comment ID is missing or null. A comment ID is required.",
        ),
        ("commentId" | "inReplyTo", _) => (
            "InvalidCommentIdException",
            "The comment ID is not in a valid format.",
        ),
        ("reactionValue", _) if missing => (
            "ReactionValueRequiredException",
            "A reaction value is required.",
        ),
        // Every member that carries an `ObjectId` naming a blob takes the blob
        // errors: `beforeBlobId` and `afterBlobId` are `GetBlobDifferences`'s names
        // for the two sides of one, and it models these same codes for them.
        //
        // Only the ABSENCE of one is refused here. `ObjectId` carries no modelled
        // constraint, so a value that names no git object reaches the handler, which
        // is what refuses it as `InvalidBlobIdException` — and that refusal names the
        // rejected id (`crate::state::invalid_blob_id_message`), which this seam
        // cannot do because it words a refusal with a fixed sentence.
        ("blobId" | "beforeBlobId" | "afterBlobId", _) if missing => (
            "BlobIdRequiredException",
            crate::state::BLOB_ID_REQUIRED_MESSAGE,
        ),
        ("commitMessage", ViolationKind::MaxLength(_)) => (
            "CommitMessageLengthExceededException",
            "The commit message is too long.",
        ),
        ("name", ViolationKind::MaxLength(_)) => (
            "NameLengthExceededException",
            "The user name is not valid because it has exceeded the character limit for author names.",
        ),
        ("email", _) => (
            "InvalidEmailException",
            "The specified email address either contains one or more characters that are not allowed, or it exceeds the maximum number of characters allowed for an email address.",
        ),
        ("content", _) if missing => (
            "CommentContentRequiredException",
            crate::state::COMMENT_CONTENT_REQUIRED_MESSAGE,
        ),
        ("content", ViolationKind::MaxLength(_)) => (
            "CommentContentSizeLimitExceededException",
            "The comment is too large. Comments are limited to 10,240 characters.",
        ),
        // `relativeFileVersion` is a member of the nested `Location` shape, so it
        // is the path's LEAF rather than its top-level member.
        (_, ViolationKind::EnumValue(_)) if leaf == "relativeFileVersion" => (
            "InvalidRelativeFileVersionEnumException",
            "Either the enum is not in a valid format, or the specified file version enum is not valid in respect to the current file version.",
        ),
        ("clientRequestToken", _) if missing => (
            "ClientRequestTokenRequiredException",
            crate::state::CLIENT_REQUEST_TOKEN_REQUIRED_MESSAGE,
        ),
        ("clientRequestToken", _) => (
            "InvalidClientRequestTokenException",
            "The client request token is not valid.",
        ),
        ("sortBy", ViolationKind::EnumValue(_)) => (
            "InvalidSortByException",
            "The specified sort by value is not valid.",
        ),
        // `pullRequestEventType` is a modelled enum, so a value outside it is
        // refused at the boundary rather than by the handler that reads the log;
        // the code and sentence are the ones `DescribePullRequestEvents` models
        // for that member.
        ("pullRequestEventType", _) => (
            "InvalidPullRequestEventTypeException",
            crate::state::INVALID_PULL_REQUEST_EVENT_TYPE_MESSAGE,
        ),
        // `pullRequestStatus` is a modelled enum too, so a value outside it is
        // likewise refused at the boundary rather than by the handler that reads
        // the pull requests; the code and sentence are the ones the pull request
        // operations model for that member. An operation that REQUIRES the member
        // names its own code for one that was not provided at all.
        ("pullRequestStatus", _) if missing => (
            "PullRequestStatusRequiredException",
            crate::state::PULL_REQUEST_STATUS_REQUIRED_MESSAGE,
        ),
        ("pullRequestStatus", _) => (
            "InvalidPullRequestStatusException",
            crate::state::INVALID_PULL_REQUEST_STATUS_MESSAGE,
        ),
        // The merge members. Each error is the one the merge operations model for
        // that member, with that shape's own documented sentence.
        ("mergeOption", _) if missing => (
            "MergeOptionRequiredException",
            "A merge option or stategy is required, and none was provided.",
        ),
        ("mergeOption", _) => (
            "InvalidMergeOptionException",
            "The specified merge option is not valid for this operation. Not all merge strategies are supported for all operations.",
        ),
        ("conflictDetailLevel", ViolationKind::EnumValue(_)) => (
            "InvalidConflictDetailLevelException",
            "The specified conflict detail level is not valid.",
        ),
        ("conflictResolutionStrategy", ViolationKind::EnumValue(_)) => (
            "InvalidConflictResolutionStrategyException",
            "The specified conflict resolution strategy is not valid.",
        ),
        // `replacementType` is a member of the nested `ReplaceContentEntry`, so it
        // is the path's LEAF rather than its top-level member.
        (_, _) if leaf == "replacementType" && missing => (
            "ReplacementTypeRequiredException",
            "A replacement type is required.",
        ),
        (_, ViolationKind::EnumValue(_)) if leaf == "replacementType" => (
            "InvalidReplacementTypeException",
            "Automerge was specified for resolving the conflict, but the specified replacement type is not valid.",
        ),
        ("order", ViolationKind::EnumValue(_)) => (
            "InvalidOrderException",
            "The specified sort order is not valid.",
        ),
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            _,
        ) if missing => (
            "ApprovalRuleTemplateNameRequiredException",
            "An approval rule template name is required, but was not specified.",
        ),
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            ViolationKind::MinLength(_),
        ) => (
            // A name that is present but empty names no template either, so it is
            // the same failure as not specifying one.
            "ApprovalRuleTemplateNameRequiredException",
            "An approval rule template name is required, but was not specified.",
        ),
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            _,
        ) => (
            "InvalidApprovalRuleTemplateNameException",
            "The name of the approval rule template is not valid. Template names must be between 1 and 100 valid characters in length.",
        ),
        // `newRuleContent` is deliberately NOT listed: UpdateApprovalRuleTemplateContent
        // and UpdatePullRequestApprovalRuleContent both spell their content member that
        // way while modelling different error classes for it (the template's and the
        // rule's), so one name-keyed row cannot speak for both.
        //
        // `approvalRuleContent` names only a pull request's OWN approval rule
        // (`CreatePullRequestApprovalRule`), which models the rule's classes. Content
        // that carries no document at all — the member left out, or present and empty
        // against its `@length min 1` — is the same failure stated by one sentence
        // covering both, and the service answers it with the rule's "not valid" code
        // rather than a protocol-level refusal.
        ("approvalRuleContent", _) if missing => (
            "InvalidApprovalRuleContentException",
            crate::state::APPROVAL_RULE_CONTENT_REQUIRED_MESSAGE,
        ),
        ("approvalRuleContent", ViolationKind::MinLength(_)) => (
            "InvalidApprovalRuleContentException",
            crate::state::APPROVAL_RULE_CONTENT_REQUIRED_MESSAGE,
        ),
        ("approvalRuleContent", _) => (
            "InvalidApprovalRuleContentException",
            crate::state::INVALID_APPROVAL_RULE_CONTENT_MESSAGE,
        ),
        ("approvalRuleTemplateContent", _) if missing => (
            "ApprovalRuleTemplateContentRequiredException",
            "The content for the approval rule template is empty. You must provide some content for an approval rule template. The content cannot be null.",
        ),
        ("approvalRuleTemplateContent", ViolationKind::MinLength(_)) => (
            "ApprovalRuleTemplateContentRequiredException",
            "The content for the approval rule template is empty. You must provide some content for an approval rule template. The content cannot be null.",
        ),
        ("approvalRuleTemplateContent", _) => (
            "InvalidApprovalRuleTemplateContentException",
            crate::state::INVALID_APPROVAL_RULE_TEMPLATE_CONTENT_MESSAGE,
        ),
        ("approvalRuleTemplateDescription", _) => (
            "InvalidApprovalRuleTemplateDescriptionException",
            "The description for the approval rule template is not valid because it exceeds the maximum characters allowed for a description.",
        ),
        ("nextToken", _) => (
            "InvalidContinuationTokenException",
            "The specified continuation token is not valid.",
        ),
        // The two commit specifiers a merge is evaluated between: each models the
        // error named for it, and each "must provide a valid branch name, tag, or
        // full commit ID"; an absent one specifies no commit at all.
        ("sourceCommitSpecifier" | "destinationCommitSpecifier", _) if missing => (
            "CommitRequiredException",
            "A commit was not specified.",
        ),
        ("sourceCommitSpecifier", _) => (
            "InvalidSourceCommitSpecifierException",
            "The source commit specifier is not valid. You must provide a valid branch name, tag, or full commit ID.",
        ),
        ("destinationCommitSpecifier", _) => (
            "InvalidDestinationCommitSpecifierException",
            "The destination commit specifier is not valid. You must provide a valid branch name, tag, or full commit ID.",
        ),
        ("maxConflictFiles", _) => (
            "InvalidMaxConflictFilesException",
            "The specified value for the number of conflict files to return is not valid.",
        ),
        // `title` and `description` carry a pull request's text: the only members
        // of either name in this model belong to the pull request operations,
        // which model these codes for them.
        ("title", _) if missing => (
            "TitleRequiredException",
            crate::state::TITLE_REQUIRED_MESSAGE,
        ),
        ("title", _) => (
            "InvalidTitleException",
            "The title of the pull request is not valid.",
        ),
        ("description", _) => (
            "InvalidDescriptionException",
            "The pull request description is not valid.",
        ),
        ("targets", _) if missing && leaf == "targets" => (
            "TargetsRequiredException",
            "An array of target objects is required. It cannot be empty or null.",
        ),
        // `sourceReference` and `destinationReference` are members of the nested
        // `Target` shape, so each is the path's LEAF rather than its top-level
        // member — and each takes the error for the member actually at fault
        // rather than the one for the list that holds it.
        (_, _) if missing && matches!(leaf, "sourceReference" | "destinationReference") => (
            "ReferenceNameRequiredException",
            crate::state::REFERENCE_NAME_REQUIRED_MESSAGE,
        ),
        (_, _) if matches!(leaf, "sourceReference" | "destinationReference") => (
            "InvalidReferenceNameException",
            crate::state::INVALID_REFERENCE_NAME_MESSAGE,
        ),
        // The list itself, rather than a member of one of its entries.
        ("targets", _) if leaf == "targets" => (
            "InvalidTargetsException",
            "The targets for the pull request is not valid or not in a valid format.",
        ),
        ("targets", _) => (
            "InvalidTargetException",
            "The target for the pull request is not valid. A target must contain the full values for the repository name, source branch, and destination branch for the pull request.",
        ),
        ("pullRequestId", _) if missing => (
            "PullRequestIdRequiredException",
            crate::state::PULL_REQUEST_ID_REQUIRED_MESSAGE,
        ),
        ("pullRequestId", _) => (
            "InvalidPullRequestIdException",
            crate::state::INVALID_PULL_REQUEST_ID_MESSAGE,
        ),
        // `revisionId` names a revision of a pull request for every operation that
        // takes one, and each of them models the same pair of codes for it: a
        // revision that was not provided, and one that is not a revision id.
        ("revisionId", _) if missing => (
            "RevisionIdRequiredException",
            crate::state::REVISION_ID_REQUIRED_MESSAGE,
        ),
        ("revisionId", _) => (
            "InvalidRevisionIdException",
            crate::state::INVALID_REVISION_ID_MESSAGE,
        ),
        // `approvalState` is a modelled enum, so a value outside it is refused at
        // the boundary rather than by the handler that records the approval; the
        // code and sentence are the ones `UpdatePullRequestApprovalState` models for
        // that member, which requires it, so one that was not provided at all is
        // reported as missing rather than as invalid.
        ("approvalState", _) if missing => (
            "ApprovalStateRequiredException",
            crate::state::APPROVAL_STATE_REQUIRED_MESSAGE,
        ),
        ("approvalState", _) => (
            "InvalidApprovalStateException",
            crate::state::INVALID_APPROVAL_STATE_MESSAGE,
        ),
        // `overrideStatus` is a modelled enum too, so a value outside it is refused
        // at the boundary rather than by the handler that sets the rules aside; the
        // code and sentence are the ones `OverridePullRequestApprovalRules` models
        // for that member, which requires it, so one that was not provided at all is
        // reported as missing rather than as invalid.
        ("overrideStatus", _) if missing => (
            "OverrideStatusRequiredException",
            crate::state::OVERRIDE_STATUS_REQUIRED_MESSAGE,
        ),
        ("overrideStatus", _) => (
            "InvalidOverrideStatusException",
            crate::state::INVALID_OVERRIDE_STATUS_MESSAGE,
        ),
        ("tags", _) if missing => (
            "TagsMapRequiredException",
            "A map of tags is required.",
        ),
        ("tags", _) => ("InvalidTagsMapException", "The map of tags is not valid."),
        ("tagKeys", _) if missing => (
            "TagKeysListRequiredException",
            "A list of tag keys is required. The list cannot be empty or null.",
        ),
        ("tagKeys", _) => (
            "InvalidTagKeysListException",
            "The list of tags is not valid.",
        ),
        ("resourceArn", _) if missing => (
            "ResourceArnRequiredException",
            "A valid Amazon Resource Name (ARN) for an CodeCommit resource is required.",
        ),
        ("resourceArn", _) => (
            "InvalidResourceArnException",
            crate::state::INVALID_RESOURCE_ARN_MESSAGE,
        ),
        // Everything else keeps the framework's own wording.
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message: requirement.to_string(),
    })
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        let route_key = smithy_server_runtime::resolve_route_key(parts);
        if route_key
            .as_deref()
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request to the git smart-HTTP transport, which is claimed by PATH
        // because that is the only thing that identifies it: a git client sends no
        // SigV4 credential and no `X-Amz-Target`, and addresses whatever host the
        // clone URL named — the emulator's own endpoint (an IP literal or
        // `localhost`, which name no service) or the public CodeCommit git host,
        // whose label names no service either. Without this the router answers
        // `UnknownService` for a URL this service handed out.
        //
        // A request that DOES name another service is left to it: a second
        // claimant would only make the router refuse both as `AmbiguousService`.
        if route_key
            .as_deref()
            .is_some_and(|key| key != GIT_HOST_LABEL)
        {
            return false;
        }
        // `http::Uri::path` already strips the scheme, the authority and the
        // query, for an origin-form request line and an absolute-form one alike.
        git_http::route(parts.method.as_str(), parts.uri.path(), parts.uri.query()).is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A merge whose `conflictResolution` its strategy cannot honour is
        // refused for that COMBINATION, before any entry of the list is looked
        // at — which the handler cannot do for an entry the MODEL refuses,
        // because the framework validates the request against the model before
        // any handler runs. The one seam that runs earlier is this one, so the
        // refusal is answered from here; [`conflict_resolution`] carries the whole
        // argument, decides it from the framework's own deserializer and
        // constraint walk, and answers `None` for every request the model accepts
        // (whose handler then refuses the combination itself, in the same words).
        let protocol = self.entry.protocol();
        if let Ok(key) = protocol.routing_key(req) {
            if conflict_resolution::refusal_for::<Eng>(key, protocol, req).is_some() {
                return Some(&self.unhonourable_conflict_resolution);
            }
        }
        // Everything else resolves through the generated routing table.
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // CodeCommit does not answer one generic `ValidationException`: it names a
        // specific error per member — a missing repository name is
        // `RepositoryNameRequiredException`, a `sortBy` outside its enum is
        // `InvalidSortByException`, and so on. A client branches on that code, so the
        // framework's refusal is reworded here (code AND message) into the error the
        // model declares for the member at fault. The HTTP status stays with the
        // protocol, and every error named below is a 400 in this model.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(refusal_for),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A git smart-HTTP request is exactly such a request and is served here:
        // git's transport is not in the Smithy model, so it names no modeled
        // operation and no routing key, and it carries no SigV4 signature for the
        // framework to verify (a git client authenticates with HTTPS git
        // credentials, which this emulator does not mint). It is answered in git's
        // own wire protocol rather than in an AWS error envelope, because a git
        // client is what reads it.
        let (path, query) = git_http::split_uri(req.uri());
        let request = git_http::route(req.method(), &path, query.as_deref())?;
        // WHO the request speaks for, resolved before this service's store is
        // locked: the principal reads the IAM and STS stores, and the decisions the
        // transport takes per authorization step are then made from that snapshot
        // alone, so no two stores are held at once. `None` is a request that
        // presented no credential, which the transport answers with a challenge.
        let caller = git_authorization::presented_principal(
            &*self.engine,
            req.headers().get("authorization"),
        );
        // A body that is not in memory is a body this service never read: it must
        // not be served as an EMPTY one, which a push would answer as a no-op
        // success. The router buffers every request body, so this is the
        // can't-happen arm, refused the same way an unreachable store is.
        let unreadable = || git_http::Response::text(500, "the request body could not be read");
        let response = match (
            crate::state::service_state(&*self.engine),
            req.body().bytes(),
        ) {
            (Some(state), Some(body)) => {
                // BOUNDED BEFORE ANYTHING IS READ. The body is caller-supplied and
                // the work below is inflate and delta resolution over it, so the raw
                // body is refused above the size this transport could accept at all
                // ([`git_http::MAX_REQUEST_BODY_BYTES`]) before it is decoded, before
                // the store guard is taken and before a single byte is expanded.
                if body.len() > git_http::MAX_REQUEST_BODY_BYTES {
                    git_http::Response::text(
                        413,
                        &format!(
                            "the request body is larger than the {} bytes this emulator accepts",
                            git_http::MAX_REQUEST_BODY_BYTES
                        ),
                    )
                } else {
                    let content_encoding = req.headers().get("content-encoding");
                    // OFF THE ASYNC WORKER, and OUTSIDE the store guard. Undoing the
                    // body's `Content-Encoding` is caller-driven CPU over
                    // caller-supplied bytes, bounded by its own inflated-size
                    // ceiling, and it needs no store at all — so it runs before the
                    // lock is taken and holds nothing while it runs.
                    match off_the_async_worker(|| {
                        git_http::decode_body(content_encoding, body)
                    }) {
                        // OFF THE ASYNC WORKER. Reading a pack (inflate, delta
                        // resolution) and projecting a push are CPU-bound, and they
                        // hold this service's store lock for their whole span, so
                        // they must not occupy a runtime worker: `block_in_place`
                        // hands the current task its own thread and lets the runtime
                        // keep serving every other request while this one runs. This
                        // hook is synchronous — the vendored router calls it from
                        // `dispatch` — so there is no `spawn_blocking(..).await` to
                        // use instead; `block_in_place` is the construct for exactly
                        // that case. It exists only on a multi-thread runtime, which
                        // is the one the emulator boots, so a current-thread runtime
                        // (a `#[tokio::test]`, say) runs the work inline, where there
                        // is no other worker to protect.
                        //
                        // Each phase under the guard carries its own wall-clock
                        // deadline and its own quantity bounds — the pack reader's
                        // (`git_pack::MAX_PACK_SECONDS`, `MAX_PACK_BYTES`,
                        // `MAX_PACK_OBJECTS`, `MAX_PACK_INFLATED_BYTES`) and the
                        // push projection's (`crate::state::MAX_PROJECTION_SECONDS`
                        // and its budget) — so the span the lock is held for is
                        // bounded whatever the body carries.
                        Ok(decoded) => off_the_async_worker(|| {
                            git_http::serve(&mut state.lock(), &request, &decoded, caller.as_ref())
                        }),
                        Err(message) => git_http::Response::text(400, &message),
                    }
                }
            }
            (None, _) => git_http::Response::text(500, "the repository store is unavailable"),
            (_, None) => unreadable(),
        };
        let mut rendered = aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(response.status)
                .unwrap_or_else(|_| aws_smithy_runtime_api::http::StatusCode::try_from(500u16).expect("500 is a status")),
            aws_smithy_types::body::SdkBody::from(response.body),
        );
        rendered
            .headers_mut()
            .insert("content-type", response.content_type);
        // RFC 9110 requires a `WWW-Authenticate` challenge on a `401`, and it is the
        // challenge that makes a git client ask its credential helper for the
        // credentials and send the request again.
        if let Some(challenge) = response.www_authenticate {
            rendered.headers_mut().insert("www-authenticate", challenge);
        }
        // A git client must never serve a ref advertisement or a packfile from a
        // cache: both are a snapshot of refs that the next request may move.
        rendered.headers_mut().insert("cache-control", "no-cache");
        rendered.headers_mut().insert("pragma", "no-cache");
        Some(rendered)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
