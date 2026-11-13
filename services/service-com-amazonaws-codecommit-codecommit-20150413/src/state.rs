// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codecommit-codecommit-20150413 service.
//!
//! The store is a map of live repositories, each carrying its own git object
//! store, branches, tags, the commit projection the control plane reads and the
//! comments posted on its commit comparisons, so every response an operation
//! returns is a lookup in state rather than a value made up on the request path.
//!
//! A repository has exactly ONE object store ([`Repository::objects`]), keyed by
//! git object name, and it is what both faces of the service read: the control
//! plane projects it into the modeled shapes (`GetFile`, `GetBranch`), and the git
//! smart-HTTP transport in [`crate::service::git_http`] serves and receives the
//! very same objects. Object ids are therefore real git object names — see
//! [`git_object`] for the name function — so a `git clone` of content written by
//! `PutFile` resolves, and a `GetFile` of content written by `git push` resolves.
//!
//! The [`Commit`] records in [`Repository::commits`] are a PROJECTION of the
//! commit objects: each carries the flattened tree snapshot at that commit, so
//! reading a file at a commit specifier is a lookup rather than a tree walk.

pub mod conflict_report;
pub mod git_endpoint;
pub mod git_object;
pub mod merge;
mod merge_record;
mod repository;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{
    Approval, ApprovalRule, ApprovalRuleEventMetadata, ApprovalRuleOverriddenEventMetadata,
    ApprovalRuleTemplate, ApprovalStateChangedEventMetadata, Comment, Location, MergeMetadata,
    OriginApprovalRuleTemplate, PullRequest, PullRequestCreatedEventMetadata, PullRequestEvent,
    PullRequestMergedStateChangedEventMetadata, PullRequestSourceReferenceUpdatedEventMetadata,
    PullRequestStatusChangedEventMetadata, PullRequestTarget, ReactionValueFormats,
    RepositoryMetadata, RepositoryTrigger, UserInfo,
};
use git_object::{Kind, Object, TreeEntry};

/// The region an ARN or clone URL uses when the request carries no SigV4
/// credential scope to read one from (an unsigned request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `relativeFileVersion` values the model's `RelativeFileVersionEnum` declares
/// (`smithy_query(query="shape",shape="RelativeFileVersionEnum")`): whether the
/// commented version of a file is the before or the after of the comparison.
pub const RELATIVE_FILE_VERSION_BEFORE: &str = "BEFORE";
pub const RELATIVE_FILE_VERSION_AFTER: &str = "AFTER";

/// The largest comment CodeCommit accepts, in characters. Documented on
/// `CommentContentSizeLimitExceededException`: "The comment is too large. Comments
/// are limited to 10,240 characters."
pub const MAX_COMMENT_CHARACTERS: usize = 10_240;

/// How many comment threads one page of a comment read carries when the caller
/// names no `maxResults`, and the largest value that member accepts.
/// Documented on `GetCommentsForComparedCommit`: "A non-zero, non-negative integer
/// used to limit the number of returned results. The default is 100 comments, but
/// you can configure up to 500.", and on `GetCommentsForPullRequest`: "The default
/// is 100 comments. You can return up to 500 comments with a single request."
pub const COMMENTS_PAGE_DEFAULT: i32 = 100;
pub const COMMENTS_PAGE_MAX: i32 = 500;

/// How many comments one repository holds before `PostCommentForComparedCommit`
/// refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a comment is caller-supplied text
/// kept for the life of the repository, so this is what keeps one repository's
/// stored size bounded. Every record it holds is released by `DeleteRepository`.
pub const MAX_COMMENTS_PER_REPOSITORY: usize = 1_000;

/// `pullRequestStatus` values the model's `PullRequestStatusEnum` declares
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`). The modelled
/// doc on `PullRequest.pullRequestStatus` states the only transition: "Pull
/// request status can only change from OPEN to CLOSED."
pub const PULL_REQUEST_STATUS_OPEN: &str = "OPEN";
pub const PULL_REQUEST_STATUS_CLOSED: &str = "CLOSED";

/// `pullRequestEventType` values the model's `PullRequestEventType` declares
/// (`smithy_query(query="shape",shape="PullRequestEventType")`). Each names one
/// kind of change to a pull request, which is what an entry in its event log
/// records ([`PullRequestEventDetail`]).
pub const PULL_REQUEST_EVENT_CREATED: &str = "PULL_REQUEST_CREATED";
pub const PULL_REQUEST_EVENT_STATUS_CHANGED: &str = "PULL_REQUEST_STATUS_CHANGED";
pub const PULL_REQUEST_EVENT_SOURCE_REFERENCE_UPDATED: &str =
    "PULL_REQUEST_SOURCE_REFERENCE_UPDATED";
pub const PULL_REQUEST_EVENT_MERGE_STATE_CHANGED: &str = "PULL_REQUEST_MERGE_STATE_CHANGED";
pub const PULL_REQUEST_EVENT_APPROVAL_RULE_CREATED: &str = "PULL_REQUEST_APPROVAL_RULE_CREATED";
pub const PULL_REQUEST_EVENT_APPROVAL_RULE_UPDATED: &str = "PULL_REQUEST_APPROVAL_RULE_UPDATED";
pub const PULL_REQUEST_EVENT_APPROVAL_RULE_DELETED: &str = "PULL_REQUEST_APPROVAL_RULE_DELETED";
pub const PULL_REQUEST_EVENT_APPROVAL_RULE_OVERRIDDEN: &str =
    "PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN";
pub const PULL_REQUEST_EVENT_APPROVAL_STATE_CHANGED: &str = "PULL_REQUEST_APPROVAL_STATE_CHANGED";

/// How many pull request events one `DescribePullRequestEvents` page carries
/// when the caller names no `maxResults`, and the largest value that member
/// accepts. Documented on it: "A non-zero, non-negative integer used to limit
/// the number of returned results. The default is 100 events, which is also the
/// maximum number of events that can be returned in a result."
pub const PULL_REQUEST_EVENTS_PAGE_DEFAULT: i32 = 100;
pub const PULL_REQUEST_EVENTS_PAGE_MAX: i32 = 100;

/// How many events one pull request's log RETAINS, the newest kept.
///
/// The emulator's OWN bound, not an AWS limit: every change to a pull request
/// appends an entry, and a caller can change one without end (adding and
/// removing an approval rule, pushing to the source branch), so this is what
/// keeps one pull request's stored size bounded. Every entry it holds is
/// released with its repository by `DeleteRepository`.
pub const MAX_PULL_REQUEST_EVENTS_RETAINED: usize = 1_000;

/// How many OPEN pull requests one repository holds before `CreatePullRequest`
/// refuses the write with the modelled
/// `MaximumOpenPullRequestsExceededException`.
///
/// Documented on that error shape: "You cannot create the pull request because
/// the repository has too many open pull requests. The maximum number of open
/// pull requests for a repository is 1,000."
pub const MAX_OPEN_PULL_REQUESTS_PER_REPOSITORY: usize = 1_000;

/// How many pull requests one repository RETAINS in total, open and closed.
///
/// The emulator's OWN bound, not an AWS limit, and the one
/// [`MAX_OPEN_PULL_REQUESTS_PER_REPOSITORY`] leaves open: a closed pull request
/// is kept for the life of the repository, so without this a caller could open
/// and close pull requests without end. Every record it holds is released by
/// `DeleteRepository`.
pub const MAX_PULL_REQUESTS_PER_REPOSITORY: usize = 2_000;

/// `fileMode` values the model's `FileModeTypeEnum` declares
/// (`smithy_query(query="shape",shape="FileModeTypeEnum")`).
pub const FILE_MODE_NORMAL: &str = "NORMAL";
pub const FILE_MODE_EXECUTABLE: &str = "EXECUTABLE";
pub const FILE_MODE_SYMLINK: &str = "SYMLINK";

/// Upper bound on the number of segments in a file path. Not an AWS limit: it
/// bounds the recursion in [`tree_objects`], which walks one level per path
/// segment, so a pathological path cannot exhaust the stack. Enforced in BOTH
/// places a path's depth can drive that walk: [`normalize_path`] refuses a deeper
/// `filePath` at the request boundary, and `build_tree` (under [`tree_objects`])
/// refuses a deeper snapshot however it was assembled — a pushed packfile reaches
/// the builder without passing a `filePath` through [`normalize_path`].
pub const MAX_PATH_SEGMENTS: usize = 256;

/// How many repositories the store holds before `CreateRepository` refuses with
/// the modelled `RepositoryLimitExceededException`.
///
/// CodeCommit publishes an adjustable quota on repositories per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is far above any workflow an emulator
/// serves, and it is what stops the repository map growing without end; every
/// record it holds is released by `DeleteRepository`.
pub const REPOSITORY_QUOTA: usize = 1_000;

/// How many files one repository's tree holds before `PutFile` refuses the write
/// and before the git transport refuses a push whose commit carries a larger
/// tree.
///
/// The emulator's OWN bound, not an AWS limit: a commit's projection stores the
/// whole tree snapshot, so this is what keeps both the per-request work and one
/// repository's stored size bounded.
pub const MAX_FILES_PER_TREE: usize = 1_000;

/// How many commits one repository's PROJECTION retains. Older commits that are
/// not the tip of a ref are dropped by [`Repository::prune_history`], so
/// repeated writes cannot grow the store without end.
///
/// The emulator's OWN bound, not an AWS limit: CodeCommit keeps all history. A
/// read at a commit that has been dropped answers `CommitDoesNotExistException`,
/// the same as a commit id the repository never held. With
/// [`MAX_FILES_PER_TREE`] it bounds the product of commits and files per commit,
/// which is what one repository can cost.
pub const MAX_COMMITS_RETAINED: usize = 256;

/// How many git objects one repository's object store holds. The emulator's OWN
/// bound, not an AWS limit: it is what stops a push growing the store without
/// end, and a push that would pass it is refused by the transport.
pub const MAX_OBJECTS_PER_REPOSITORY: usize = 20_000;

/// How many bytes of git object payload one repository's object store holds. The
/// emulator's OWN bound, for the same reason as [`MAX_OBJECTS_PER_REPOSITORY`].
pub const MAX_OBJECT_BYTES_PER_REPOSITORY: usize = 64 * 1024 * 1024;

/// How many branches one repository holds before `CreateBranch`, or a push
/// through the git transport, refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a branch is a caller-named
/// reference kept for the life of the repository, and
/// [`Repository::prune_history`] never drops a branch tip, so an unbounded number
/// of branches would pin an unbounded number of commits and defeat
/// [`MAX_COMMITS_RETAINED`]. It is therefore deliberately well BELOW that bound,
/// which is what lets pruning always reach it. Every record it holds is released
/// by `DeleteRepository`.
pub const MAX_BRANCHES_PER_REPOSITORY: usize = 64;

/// How many commits that no ref names one repository keeps readable — the
/// commits `CreateUnreferencedMergeCommit` writes.
///
/// The emulator's OWN bound, for exactly the reason
/// [`MAX_BRANCHES_PER_REPOSITORY`] gives and at the same figure: a retained
/// unreferenced commit roots its own history against
/// [`Repository::prune_history`], so an unbounded number of them would pin an
/// unbounded number of commits and defeat [`MAX_COMMITS_RETAINED`]. The oldest
/// are released first ([`Repository::retain_unreferenced`]), and every record is
/// released by `DeleteRepository`.
pub const MAX_UNREFERENCED_COMMITS: usize = MAX_BRANCHES_PER_REPOSITORY;

/// How many tags one repository holds before a push refuses to create another.
///
/// The emulator's OWN bound, for exactly the reason
/// [`MAX_BRANCHES_PER_REPOSITORY`] gives and at the same figure: a tag is a
/// caller-named reference kept for the life of the repository and never pruned,
/// so it pins history just as a branch does. CodeCommit has no control-plane API
/// that creates a tag, so the git transport is the only writer.
pub const MAX_TAGS_PER_REPOSITORY: usize = MAX_BRANCHES_PER_REPOSITORY;

/// The largest total content size of one folder, in bytes. CodeCommit documents
/// this limit on `FolderContentSizeLimitExceededException`: a commit cannot be
/// created when a change "results in a folder whose contents exceed the limit of
/// 6 MB".
pub const MAX_FOLDER_CONTENT_BYTES: i64 = 6 * 1024 * 1024;

/// The git ref namespace a branch lives in.
pub const HEADS_PREFIX: &str = "refs/heads/";
/// The git ref namespace a tag lives in.
pub const TAGS_PREFIX: &str = "refs/tags/";

/// How many tree entries one request's projection may walk in total, across
/// every commit it projects.
///
/// The emulator's OWN bound, and the bound that holds where the depth and the
/// file count do not: a git tree is a DAG, so a few tree objects naming one
/// another present exponentially many paths (see
/// [`Repository::flatten_tree`]). It is [`MAX_COMMITS_PROJECTED_PER_REQUEST`]
/// times four times [`MAX_FILES_PER_TREE`] — room for every commit the request
/// may project to carry a full snapshot and its directories — and it is charged
/// per entry, so no product of commands, commits and entries escapes it.
pub const MAX_TREE_ENTRIES_PER_REQUEST: usize =
    MAX_COMMITS_PROJECTED_PER_REQUEST * 4 * MAX_FILES_PER_TREE;

/// How many commits one request's projection may build.
///
/// The emulator's OWN bound: a repository retains at most
/// [`MAX_COMMITS_RETAINED`] projected commits, so projecting more than that in
/// one request would only build snapshots that are pruned on the way out.
pub const MAX_COMMITS_PROJECTED_PER_REQUEST: usize = MAX_COMMITS_RETAINED;

/// How long one request's projection may take before it is refused. The
/// emulator's own bound, at the same figure as the pack reader's deadline and the
/// workspace's other caller-driven CPU work: the budgets above bound the work, and
/// this bounds the time it may take on any machine, so the store lock the
/// projection runs under is never held for an unbounded span. The projection also
/// runs off the async worker — see `crate::service`'s `off_the_async_worker`.
pub const MAX_PROJECTION_SECONDS: u64 = 2;

/// A wall-clock deadline: the instant a piece of caller-driven CPU work has to
/// be finished by.
///
/// One type, so the two budgets this crate threads through such work — the push
/// projection's [`ProjectionBudget`] and a merge's
/// [`merge::MergeBudget`] — keep one reading of "out of time" rather than two
/// copies of it, each wording the refusal its own caller needs.
#[derive(Debug)]
pub struct Deadline {
    at: std::time::Instant,
}

impl Deadline {
    /// A deadline `seconds` from now.
    pub fn in_seconds(seconds: u64) -> Self {
        Self {
            at: std::time::Instant::now() + std::time::Duration::from_secs(seconds),
        }
    }

    /// Whether the work is already out of time.
    pub fn passed(&self) -> bool {
        std::time::Instant::now() > self.at
    }
}

/// What ONE request's whole projection may still spend: commits, tree entries
/// and time.
///
/// One budget is built per request and threaded through every projection a
/// request drives ([`Repository::project_history`], and so
/// [`Repository::flatten_tree`]), because the cost is a PRODUCT: a push carries
/// many ref updates, each naming a commit whose ancestors each carry a whole tree
/// snapshot. Per-call caps would bound each factor and none of the product.
#[derive(Debug)]
pub struct ProjectionBudget {
    commits_left: usize,
    entries_left: usize,
    deadline: Deadline,
}

impl Default for ProjectionBudget {
    fn default() -> Self {
        Self::for_one_request()
    }
}

impl ProjectionBudget {
    /// The allowance one request gets.
    pub fn for_one_request() -> Self {
        Self {
            commits_left: MAX_COMMITS_PROJECTED_PER_REQUEST,
            entries_left: MAX_TREE_ENTRIES_PER_REQUEST,
            deadline: Deadline::in_seconds(MAX_PROJECTION_SECONDS),
        }
    }

    /// Charge one projected commit.
    fn commit(&mut self) -> Result<(), String> {
        self.check_deadline()?;
        self.commits_left = self.commits_left.checked_sub(1).ok_or_else(|| {
            format!(
                "this request would project more than the {MAX_COMMITS_PROJECTED_PER_REQUEST} commits this emulator projects at once"
            )
        })?;
        Ok(())
    }

    /// Charge one tree entry walked.
    ///
    /// `pub` because the walks a request drives outside this file are charged to
    /// the same budget: a push's per-ref closure check
    /// ([`Repository::first_missing_object_within`]) walks tree entries exactly as
    /// a projection does, and one request may drive it once per ref it updates, so
    /// the SUM of every walk is what this bounds.
    pub fn entry(&mut self) -> Result<(), String> {
        self.entries_left = self.entries_left.checked_sub(1).ok_or_else(|| {
            format!(
                "this request would walk more than the {MAX_TREE_ENTRIES_PER_REQUEST} tree entries this emulator walks at once"
            )
        })?;
        // Checked here as well as per commit: a single commit's tree walk is the
        // one step that can run long on its own.
        if self.entries_left.is_multiple_of(1024) {
            self.check_deadline()?;
        }
        Ok(())
    }

    fn check_deadline(&self) -> Result<(), String> {
        if self.deadline.passed() {
            return Err(format!(
                "this request took longer than the {MAX_PROJECTION_SECONDS} seconds this emulator spends projecting a push"
            ));
        }
        Ok(())
    }
}

/// How many tags one CodeCommit resource carries before a tagging request is
/// refused with the modelled `TooManyTagsException`.
///
/// AWS's tagging limit for a CodeCommit resource: "You can add up to 50 tags to
/// a single CodeCommit repository" (CodeCommit User Guide, "Tag a repository"),
/// which is also the general AWS limit of 50 user-created tags per resource.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s and `TagValue`'s modelled `length` bounds
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`). The boundary
/// validator applies a `length` to a MEMBER, not to the keys and values inside a
/// map or list, so the tagging operations apply these themselves — a request
/// outside them is `InvalidTagsMapException` / `InvalidTagKeysListException`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;
pub const TAG_KEY_MAX_LENGTH: usize = 128;
pub const TAG_VALUE_MIN_LENGTH: usize = 0;
pub const TAG_VALUE_MAX_LENGTH: usize = 256;

/// Is this within `TagKey`'s modelled length? Smithy counts a string's length in
/// Unicode code points.
pub fn is_tag_key(candidate: &str) -> bool {
    (TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&candidate.chars().count())
}

/// Is this within `TagValue`'s modelled length?
pub fn is_tag_value(candidate: &str) -> bool {
    (TAG_VALUE_MIN_LENGTH..=TAG_VALUE_MAX_LENGTH).contains(&candidate.chars().count())
}

/// The tag key prefix AWS reserves for its own system tags. A key carrying it is
/// legal as a key but RESERVED to the service, so a request from an end-user
/// caller that would set or remove one is refused with
/// `InvalidSystemTagUsageException` and [`SYSTEM_TAG_MUTATION_MESSAGE`].
pub const SYSTEM_TAG_PREFIX: &str = "aws:";

/// Is this a tag key only AWS itself may set, i.e. one carrying
/// [`SYSTEM_TAG_PREFIX`]?
pub fn is_system_tag_key(key: &str) -> bool {
    key.starts_with(SYSTEM_TAG_PREFIX)
}

/// `InvalidSystemTagUsageException`'s message: what the service answers a caller
/// that tries to set or remove a tag whose key is reserved to AWS itself.
///
/// The refusal is about WHO is asking rather than about the key's form — a key
/// carrying [`SYSTEM_TAG_PREFIX`] is a well-formed key, and the caller simply is
/// not the service — so the sentence states that and names neither the key nor the
/// resource. It is not the error shape's documented sentence ("The specified tag is
/// not valid. Key names cannot be prefixed with aws:."), which is prose about the
/// class for a reader of the API reference. Worded here once for every operation
/// that can carry a tag key, so none of them can report the same refusal as a
/// malformed-key complaint or echo the requested key back into it.
pub const SYSTEM_TAG_MUTATION_MESSAGE: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// `RepositoryName`'s modelled `length` bounds
/// (`smithy_query(query="shape",shape="RepositoryName")`).
pub const REPOSITORY_NAME_MIN_LENGTH: usize = 1;
pub const REPOSITORY_NAME_MAX_LENGTH: usize = 100;

/// Does `candidate` satisfy `RepositoryName` — its modelled `length` 1..100 and
/// its modelled `pattern` `^[\w\.-]+$`
/// (`smithy_query(query="shape",shape="RepositoryName")`)? The boundary
/// validator already applies both to a `repositoryName` MEMBER; this is for a
/// name that arrives inside another value, such as the resource portion of a
/// resource ARN.
pub fn is_repository_name(candidate: &str) -> bool {
    let length = candidate.chars().count();
    (REPOSITORY_NAME_MIN_LENGTH..=REPOSITORY_NAME_MAX_LENGTH).contains(&length)
        // `\w` in a Smithy (ECMA-262) pattern is ASCII `[A-Za-z0-9_]`, so a
        // non-ASCII letter or digit does NOT satisfy `RepositoryName`.
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        // The one name rule the model cannot express, which the operations that
        // take a name apply too: "The suffix .git is prohibited." A name no
        // repository can ever carry is not a valid repository name here either.
        && !has_prohibited_git_suffix(candidate)
}

/// `InvalidRepositoryNameException`'s message: the sentence group the service
/// answers that code with, which states the naming rule IN FULL — the characters a
/// name may combine, the length bounds [`REPOSITORY_NAME_MIN_LENGTH`] and
/// [`REPOSITORY_NAME_MAX_LENGTH`] that `RepositoryName` publishes, and that names
/// are case sensitive — and then where the limits are documented. FIXED text
/// carrying no value from the request.
///
/// It is not the error shape's documented sentence ("A specified repository name is
/// not valid."), which is prose about the class for a reader of the API reference:
/// the service's own text is the only statement of the allowed character set and
/// the bounds, so it is what lets a caller correct the name it sent. Worded here
/// once for the request boundary (`crate::service::refusal_for`, which refuses the
/// modelled `length` and `pattern`) and for every handler that raises the code for
/// the part of the rule the model cannot express, so two operations cannot word it
/// differently.
pub const INVALID_REPOSITORY_NAME_MESSAGE: &str =
    "The repository name is not valid. Repository names can be any valid combination of letters, \
     numbers, periods, underscores, and dashes between 1 and 100 characters in length. Names are \
     case sensitive. For more information, see Limits in the AWS CodeCommit User Guide.";

/// Why a `resourceArn` cannot address a CodeCommit resource. Each variant is the
/// modelled error the tagging operations answer for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnProblem {
    /// The ARN is absent or blank: `ResourceArnRequiredException`.
    Missing,
    /// The value is not an ARN of a CodeCommit resource at all:
    /// `InvalidResourceArnException`.
    Invalid,
    /// A well-formed CodeCommit ARN whose resource portion is not a valid
    /// repository name: `InvalidRepositoryNameException`.
    RepositoryName,
}

/// The number of digits in an AWS account id, the fourth field of an ARN.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The repository a CodeCommit resource ARN names.
///
/// The only resource CodeCommit's tagging operations address is a repository, and
/// its ARN is `arn:<partition>:codecommit:<region>:<account-id>:<repository-name>`
/// — the form [`Repository::new`] stores and every CodeCommit response reports.
/// Anything else is refused; see [`ResourceArnProblem`] for which modelled error
/// each kind of refusal carries. Region and account are checked for FORM only:
/// this emulator's store holds one set of repositories rather than one per region
/// or account, so a well-formed ARN is resolved by the repository it names.
pub fn repository_name_of_resource_arn(raw: &str) -> Result<&str, ResourceArnProblem> {
    if raw.trim().is_empty() {
        return Err(ResourceArnProblem::Missing);
    }
    // An ARN is six colon-separated fields, `arn:<partition>:<vendor>:<region>:
    // <account>:<resource>`; `splitn` leaves the whole resource — any further
    // colons included — as the last of them, so a multi-segment resource is read
    // as one value and then refused by the repository-name rule below.
    let [prefix, partition, vendor, region, account, resource] =
        raw.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return Err(ResourceArnProblem::Invalid);
    };
    if prefix != "arn"
        || partition.is_empty()
        // The ARN namespace is a fixed lowercase token, so a value differing in
        // case names another vendor.
        || vendor != "codecommit"
        || region.is_empty()
        || account.len() != ACCOUNT_ID_DIGITS
        || !account.bytes().all(|byte| byte.is_ascii_digit())
        || resource.is_empty()
    {
        return Err(ResourceArnProblem::Invalid);
    }
    if !is_repository_name(resource) {
        return Err(ResourceArnProblem::RepositoryName);
    }
    Ok(resource)
}

/// Does `raw` name an identity, as an ARN?
///
/// `actorArn` names "the user whose actions resulted in the event", and
/// CodeCommit refuses a value that is not a full ARN with
/// `InvalidActorArnException`. An ARN is six colon-separated fields,
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`; an IAM ARN carries
/// no region, so that field may be empty, while the account is the 12-digit
/// account id and the resource names the identity. Nothing beyond the FORM is
/// checked here: which identities exist is not something this emulator holds —
/// a caller is identified by the access key id its request is signed with
/// ([`caller_identity_arn`]) and any access key id is served — so a well-formed
/// ARN is resolved by the events that name it.
pub fn is_identity_arn(raw: &str) -> bool {
    let [prefix, partition, vendor, _region, account, resource] =
        raw.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return false;
    };
    prefix == "arn"
        && !partition.is_empty()
        && !vendor.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && !resource.is_empty()
}

/// `InvalidActorArnException`'s message: the sentence the model documents for
/// the shape — the failure, then what to provide instead — naming nothing from
/// the request. The one place this crate words that code.
pub const INVALID_ACTOR_ARN_MESSAGE: &str =
    "The Amazon Resource Name (ARN) is not valid. Make sure that you have provided the full ARN \
     for the user who initiated the change for the pull request, and then try again.";

/// `InvalidAuthorArnException`'s message: the sentence the model documents for
/// that shape — the failure, then what to provide instead — naming nothing from
/// the request. The one place this crate words that code, exactly as
/// [`INVALID_ACTOR_ARN_MESSAGE`] is the one place it words the actor's.
pub const INVALID_AUTHOR_ARN_MESSAGE: &str =
    "The Amazon Resource Name (ARN) is not valid. Make sure that you have provided the full ARN \
     for the author of the pull request, and then try again.";

/// `InvalidReactionUserArnException`'s message: the sentence the model documents
/// for that shape, naming nothing from the request. The one place this crate words
/// that code, as [`INVALID_ACTOR_ARN_MESSAGE`] is the one place it words the
/// actor's.
pub const INVALID_REACTION_USER_ARN_MESSAGE: &str =
    "The Amazon Resource Name (ARN) of the user or identity is not valid.";

/// `InvalidPullRequestEventTypeException`'s message: likewise the shape's own
/// documented sentence, worded here once for every operation that raises the
/// code.
pub const INVALID_PULL_REQUEST_EVENT_TYPE_MESSAGE: &str = "The pull request event type is not valid.";

/// `InvalidPullRequestStatusException`'s message: likewise the shape's own
/// documented sentence, which states the two values the status admits (see
/// [`PULL_REQUEST_STATUS_OPEN`] and [`PULL_REQUEST_STATUS_CLOSED`]).
pub const INVALID_PULL_REQUEST_STATUS_MESSAGE: &str =
    "The pull request status is not valid. The only valid values are OPEN and CLOSED.";

/// `InvalidPullRequestStatusUpdateException`'s message: the sentence pair the
/// service answers that class with, stating the failure and then the ONE change of
/// value the status admits — what an attempt to move a pull request's status any
/// other way is refused with.
///
/// It is not the error shape's documented sentence: that one is prose about the
/// error class for a reader of the API reference, which spells the two states as the
/// enum's own `OPEN`/`CLOSED` values. The service's sentence names the transition in
/// PROSE casing instead — the status VALUES a response carries stay upper-case
/// ([`PULL_REQUEST_STATUS_OPEN`], [`PULL_REQUEST_STATUS_CLOSED`]), the message does
/// not — and a client branching on the text matches this one.
pub const INVALID_PULL_REQUEST_STATUS_UPDATE_MESSAGE: &str =
    "The pull request status update is not valid. The only valid value change is from Open to \
     Closed.";

/// `PullRequestStatusRequiredException`'s message: a FIXED sentence, worded here
/// once for every operation that requires a `pullRequestStatus`.
pub const PULL_REQUEST_STATUS_REQUIRED_MESSAGE: &str =
    "A pull request status is required, but none was provided.";

/// `RepositoryNameRequiredException`'s message: the sentence the model documents
/// for the shape, naming nothing from the request. Worded here once for the
/// request boundary (`crate::service`) and for the handlers that require a
/// `repositoryName` the model leaves optional.
pub const REPOSITORY_NAME_REQUIRED_MESSAGE: &str =
    "A repository name is required, but was not specified.";

/// The message AWS returns with `RepositoryDoesNotExistException`: the requested
/// repository name, then `" for "`, then the account the lookup ran in.
///
/// CodeCommit words each of its request-derived client errors with one fixed
/// template per error code, so this crate builds each such message in exactly one
/// place and every operation raising that code goes through it — two operations
/// cannot word the same code differently, and a client that matches on the message
/// reads the same text it reads against AWS.
pub fn repository_does_not_exist_message(name: &str) -> String {
    format!("{name} for {}", ::engine::emulator_account_id())
}

/// The per-entry `errorMessage` a batch operation reports for a repository name the
/// account holds no repository for.
///
/// A batch call serves the request and reports each name that failed only through
/// its own entry, so the entry carries the sentence CodeCommit words this cause
/// with: fixed text whose one slot is the name the entry asked for, telling the
/// caller what to do about it. The ONE place this crate words it, so the batch
/// operations on either side of an association cannot word it differently. The
/// top-level `RepositoryDoesNotExistException` a single-resource call answers is
/// worded by [`repository_does_not_exist_message`] instead, so neither form can be
/// mistaken for the other.
pub fn batch_repository_not_found_message(name: &str) -> String {
    format!("No repository with the name {name} was found. Please specify a valid repository name")
}

/// `ApprovalRuleTemplateDoesNotExistException`'s message, built from the requested
/// template name — the one place this crate words that code, for the same reason
/// [`repository_does_not_exist_message`] is the one place it words its own.
///
/// CodeCommit words this code with one template whose single slot is the name the
/// request asked for, and whose second sentence names the likeliest cause (a call
/// made in another Region than the one the template was created in). The template
/// is the requirement; the name in the slot comes from the request.
pub fn approval_rule_template_does_not_exist_message(name: &str) -> String {
    format!(
        "No approval rule template exists in this AWS Region with the following name: {name}. \
         Make sure that you are signed in to the AWS Region where you created the rule, and \
         then try again."
    )
}

/// `ApprovalRuleTemplateNameAlreadyExistsException`'s message: a FIXED sentence
/// carrying no value from the request.
///
/// CodeCommit scopes approval-rule-template names per ACCOUNT and words this code
/// with one constant that names neither the template nor a Region, so the operations
/// that refuse a name the account already holds — `CreateApprovalRuleTemplate` and
/// `UpdateApprovalRuleTemplateName` — cannot word it differently or interpolate the
/// requested name into it.
pub const APPROVAL_RULE_TEMPLATE_NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The approval rule template cannot be created because a template with that name already \
     exists in your AWS account.";

/// `ApprovalRuleTemplateInUseException`'s message: a FIXED sentence group carrying
/// no value from the request.
///
/// CodeCommit documents the cause on the error shape itself — "The approval rule
/// template is associated with one or more repositories. You cannot delete a
/// template that is associated with a repository. Remove all associations, and then
/// try again." — and words it with one constant that names neither the template nor
/// any of the repositories holding it, so a caller is told the rule and the way out
/// rather than which association it tripped over.
pub const APPROVAL_RULE_TEMPLATE_IN_USE_MESSAGE: &str =
    "The approval rule template cannot be deleted because it is associated with one or more \
     repositories. You cannot delete a template if it is associated with a repository. \
     Disassociate the template from all repositories in your AWS account, and then try again.";

/// Is this an approval rule template NAME one that can name a template at all?
///
/// `ApprovalRuleTemplateName`'s `@length` 1..100 is refused at the request boundary
/// (`crate::service::refusal_for`); this is the part of the rule the model cannot
/// express, documented on `InvalidApprovalRuleTemplateNameException`: "Template
/// names must be between 1 and 100 valid characters in length." A name of nothing
/// but whitespace names no template, and neither does one carrying a control
/// character.
///
/// The ONE place this crate decides it, so every operation that takes a template
/// name — minting one, updating one, deleting one — agrees about which names are
/// valid, and none of them can accept a name another would refuse.
pub fn is_approval_rule_template_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.chars().any(char::is_control)
}

/// `InvalidApprovalRuleTemplateNameException`'s message, built from the name the
/// request carried — the one place this crate words that code for a name
/// [`is_approval_rule_template_name`] refuses.
pub fn invalid_approval_rule_template_name_message(name: &str) -> String {
    format!("the approval rule template name {name} is not valid")
}

/// `InvalidApprovalRuleTemplateContentException`'s message: the sentence pair
/// CodeCommit answers that class with, carrying nothing from the request and no
/// diagnostic from whichever JSON reader rejected the document.
///
/// A parser's own complaint ("key must be a string at line 1 column 3") is an
/// implementation detail of the deserializer this emulator links, so it is never
/// what the caller is told: [`is_approval_rule_template_content`] answers whether
/// the document is one, and this constant is what the refusal says.
///
/// `InvalidApprovalRuleContentException` — the code for a document submitted as a
/// pull request's OWN approval rule rather than as a template — is worded for the
/// rule, in [`INVALID_APPROVAL_RULE_CONTENT_MESSAGE`]: each sentence names the thing
/// the caller submitted.
pub const INVALID_APPROVAL_RULE_TEMPLATE_CONTENT_MESSAGE: &str =
    "The JSON in the approval rule template is not valid. Review the JSON and make sure it is \
     valid, and then try again.";

/// `InvalidApprovalRuleContentException`'s message: the same sentence pair as
/// [`INVALID_APPROVAL_RULE_TEMPLATE_CONTENT_MESSAGE`] worded for an approval rule,
/// which is what the caller submitted when this code is raised.
pub const INVALID_APPROVAL_RULE_CONTENT_MESSAGE: &str =
    "The JSON in the approval rule is not valid. Review the JSON and make sure it is valid, and \
     then try again.";

/// `InvalidApprovalRuleContentException`'s message for content that carries NO
/// document at all — a member that is absent, empty, or nothing but whitespace.
///
/// A document is the thing `ApprovalRuleContent` carries, so a value holding none of
/// one is not a document this service failed to parse: it is content that was never
/// provided, and the service says so in one sentence that covers both forms the
/// absence takes (an empty value and a null one) and carries nothing from the
/// request. The code is the same `InvalidApprovalRuleContentException` a malformed
/// document takes ([`INVALID_APPROVAL_RULE_CONTENT_MESSAGE`]); only the sentence
/// differs, so a client catching that one modelled exception sees both.
///
/// Worded here once for the request boundary (`crate::service::refusal_for`, which
/// refuses the member's `@length min 1`) and for the handler that reaches the
/// whitespace-only case past it.
pub const APPROVAL_RULE_CONTENT_REQUIRED_MESSAGE: &str =
    "Content for an approval rule is required, but the content of the approval rule was either \
     empty or null.";

/// `InvalidResourceArnException`'s message: a FIXED sentence that echoes no ARN.
///
/// The tagging operations (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) all refuse a `resourceArn` that is not an ARN of a
/// CodeCommit resource with this one constant; the malformed value the caller sent
/// is NOT interpolated back into it.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Inputted resource arn is not formatted correctly.";

/// `CommentContentRequiredException`'s message: a FIXED sentence pair that states
/// the failure and then the rule.
///
/// Every operation that saves comment content (`PostCommentForComparedCommit`,
/// `PostCommentForPullRequest`, `PostCommentReply`, `UpdateComment`) refuses empty
/// content with this one constant.
pub const COMMENT_CONTENT_REQUIRED_MESSAGE: &str =
    "The comment could not be saved because no content was provided. Comments cannot be empty.";

/// The message `InvalidBranchNameException` carries for a `branchName` that is
/// present but EMPTY — a FIXED sentence, distinct from the wording of a branch name
/// that is non-empty and still not a valid reference name.
///
/// `BranchName` is `@length min 1`, so an empty value is refused at the request
/// boundary; this is what that refusal says (see `crate::service`), rather than a
/// generic constraint-violation text.
pub const EMPTY_BRANCH_NAME_MESSAGE: &str = "Branch name cannot be empty";

/// `RepositoryNameExistsException`'s message: `Repository named <name> already
/// exists`, built from the requested name.
pub fn repository_name_exists_message(repository_name: &str) -> String {
    format!("Repository named {repository_name} already exists")
}

/// `BranchDoesNotExistException`'s message. The template has no slot: CodeCommit
/// returns this constant and names neither the branch nor the repository, because
/// a reference that resolves to a tag rather than a branch answers the very same
/// message — an ambiguity clients are written against.
///
/// Whether the error body carries a message AT ALL is a per-operation property of
/// what the service answers, not formatting latitude: `GetBranch`'s own
/// branch-not-found body carries none (see `crate::ops::get_branch`), so that
/// operation raises the code with no message rather than with this constant.
pub const BRANCH_DOES_NOT_EXIST_MESSAGE: &str = "The specified branch does not exist";

/// The commit specifier an operation reads when the request names none.
///
/// `CommitName` documents the default on every member that takes one — GetFile's
/// and ListFileCommitHistory's "If none is provided, the head commit is used" and
/// GetFolder's "If no specifier is provided, the folder content is returned as it
/// exists in the HEAD commit" — so an absent specifier RESOLVES to `HEAD`
/// ([`Repository::resolve_commit`] takes it from there, to the repository's
/// default branch) and a refusal names `HEAD` rather than an empty string.
pub const DEFAULT_COMMIT_SPECIFIER: &str = "HEAD";

/// `CommitDoesNotExistException`'s message for a request that named a commit
/// SPECIFIER — a branch name, a tag, `HEAD` or a full commit id, the forms
/// `CommitName` documents — which resolved to no commit.
///
/// The template's single slot is that specifier, so the message names the
/// reference that resolved to nothing (`HEAD` for a request that named none) and
/// NOT the repository. The second sentence is the actionable half of the text: a
/// specifier that names nothing here is as often a reference the caller holds
/// only locally as one that does not exist at all.
///
/// A member the model types as a raw object id (`CommitId`) rather than as a
/// specifier is NOT resolved this way, and is worded by
/// [`commit_object_missing_message`] instead.
pub fn commit_does_not_exist_message(specifier: &str) -> String {
    format!(
        "AWS CodeCommit could not find the commit specifier {specifier}. Either the specifier \
         does not exist, or the commit has not yet been pushed to the remote repository."
    )
}

/// `CommitDoesNotExistException`'s message for a request that named a raw commit
/// OBJECT id — a member the model types `CommitId` — that the repository does not
/// hold.
///
/// Such a member is not a reference to resolve: it is a git object name, looked up
/// in the repository's object store, so the refusal is the git plumbing's own and
/// carries exactly the id that named no object. Nothing else of the request
/// reaches the text — in particular not the repository, and not the reference the
/// caller was pointing at that object.
pub fn commit_object_missing_message(commit_id: &str) -> String {
    format!("Object missing: {commit_id}")
}

/// `FileDoesNotExistException`'s message, built from the path the request asked
/// for — the one place this crate words that code, the path sibling of
/// [`commit_does_not_exist_message`].
///
/// The template's single slot is that path, in the canonical form that addresses a
/// tree entry ([`normalize_path`]: leading separators, which name the same entry,
/// removed). Nothing else of the request or of the state reaches the text: the
/// sentence states which path the service looked for and failed to find, so it
/// names neither the commit the path was resolved against nor the branch nor the
/// repository.
pub fn file_does_not_exist_message(path: &str) -> String {
    format!("Could not find path {path}")
}

/// `FileDoesNotExistException`'s message on the path that DELETES a file: a FIXED
/// sentence naming no path at all.
///
/// CodeCommit words this code per the call that raises it rather than once for the
/// code, and the deletion path is deliberately opaque: it states that the file named
/// is not there and nothing else — not the path, not the parent commit, not the
/// branch, not the repository. The READ path words the same code with the path it
/// looked for ([`file_does_not_exist_message`]), which is why the two cannot share
/// one template.
pub const DELETED_FILE_DOES_NOT_EXIST_MESSAGE: &str = "Cannot find the specified file.";

/// `ParentCommitDoesNotExistException`'s message: a FIXED sentence pair that
/// states the failure and then the reason for it, carrying NOTHING from the
/// request — neither the parent commit id that named no commit nor the repository
/// or branch it was looked for in.
///
/// The one place this crate words that code, so every write path that takes a
/// `parentCommitId` (`PutFile`, `CreateCommit`, `DeleteFile`) answers it with the
/// same text.
pub const PARENT_COMMIT_DOES_NOT_EXIST_MESSAGE: &str =
    "The parent commit ID is not valid. The specified parent commit ID does not exist.";

/// `ParentCommitIdOutdatedException`'s message: a FIXED sentence group carrying
/// nothing from the request — neither the stale id nor the branch.
///
/// This is the optimistic-concurrency refusal every write path hits under
/// contention, and the text states the rule and then the remediation (re-read the
/// tip with `GetBranch`, or pull). The one place this crate words that code, so
/// `PutFile`, `CreateCommit` and `DeleteFile` cannot word it differently.
pub const PARENT_COMMIT_ID_OUTDATED_MESSAGE: &str =
    "The parent commit ID is not valid because it is not the current tip of the branch. You must \
     provide the commit ID for the tip of the branch. The most common reason for this error is \
     that your local repository is not synchronized to the latest changes in the AWS CodeCommit \
     repository. Either synchronize your local repo by using the git pull command, or use \
     GetBranch to retrieve the latest commit ID for the branch, and then try again.";

/// `ParentCommitIdRequiredException`'s message: a FIXED sentence pair that states
/// the failure and then how to find the id, naming neither the branch nor the
/// repository. The one place this crate words that code, for the write operations
/// that refuse a request omitting `parentCommitId` on a branch that has a tip.
pub const PARENT_COMMIT_ID_REQUIRED_MESSAGE: &str =
    "A parent commit ID is required. Either use GetBranch to retrieve the latest commit ID for \
     the branch, or use a Git command such as git pull or git log, and then try again.";

/// `NoChangeException`'s message: a FIXED sentence group that states the rule and
/// then the only way to make the commit the caller asked for (a git client, which
/// is the only thing that writes an empty commit). It names no file, branch or
/// commit from the request.
pub const NO_CHANGE_MESSAGE: &str =
    "Cannot create the commit. At least one change is required to create a commit, but the files \
     in the commit are exactly the same as the files in the parent commit. If you want to create \
     an empty commit, use a Git client.";

/// `SameFileContentException`'s message: a FIXED sentence pair that states the
/// rule and then what did NOT happen, carrying nothing from the request.
///
/// The single-file sibling of [`NO_CHANGE_MESSAGE`] — `PutFile` raises this code
/// where `CreateCommit` raises `NoChangeException` — and the one place this crate
/// words it.
pub const SAME_FILE_CONTENT_MESSAGE: &str =
    "The content of the file is exactly the same as the content of the file in the AWS CodeCommit \
     repository and branch you specified. The file has not been added.";

/// `DefaultBranchCannotBeDeletedException`'s message, built from the branch the
/// request asked to delete — the one place this crate words that code.
///
/// The template's single slot is that branch as a FULLY QUALIFIED reference
/// (`refs/heads/<branch>`, [`qualified_reference`]), which is how a git host names
/// the reference it is refusing to drop.
pub fn default_branch_cannot_be_deleted_message(branch_name: &str) -> String {
    format!(
        "The current default branch {} cannot be deleted",
        qualified_reference(branch_name)
    )
}

/// `BlobIdDoesNotExistException`'s message, built from the object name the request
/// asked for — the one place this crate words that code.
///
/// The template's single slot is that object name, as the REQUEST spelled it: the
/// sentence states which blob could not be found, and the repository it was looked
/// for in reaches no part of the text.
///
/// It names the id for the same reason [`invalid_blob_id_message`] does — the value
/// the caller supplied is what the refusal is about — and differs from it in the
/// class: a value that is not an object name at all is rejected as malformed, while
/// a well-formed object name no object carries was simply not found here.
pub fn blob_id_does_not_exist_message(blob_id: &str) -> String {
    format!("AWS CodeCommit could not find the blob {blob_id} in the repository.")
}

/// `BranchNameExistsException`'s message, built from the branch the request asked
/// to create — the one place this crate words that code.
///
/// The template's single slot is that branch as a FULLY QUALIFIED reference
/// (`refs/heads/<branch>`, [`qualified_reference`]) in double quotes, which is how
/// a git host names the reference it refuses to create; the sentences after it
/// state what the name collides with and then the rule, which the error shape
/// documents as "Branch names must be unique." Neither the repository nor the
/// commit the caller was pointing the new reference at reaches the text.
pub fn branch_name_exists_message(branch_name: &str) -> String {
    format!(
        "A branch cannot be created with the following name: \"{}\". The commit conflicts with an \
         existing branch with the same name. Branch names must be unique.",
        qualified_reference(branch_name)
    )
}

/// `BlobIdRequiredException`'s message: the sentence the model documents for the
/// shape, naming nothing from the request. The one place this crate words that code,
/// for the request boundary (`crate::service`) and for the handlers that check a
/// blob id the model leaves unconstrained alike — `ObjectId` carries no length
/// bound, so a blank id reaches a handler rather than the boundary.
pub const BLOB_ID_REQUIRED_MESSAGE: &str = "A blob ID is required, but was not specified.";

/// `InvalidBlobIdException`'s message, built from the id that was rejected — the
/// one place this crate words that code.
///
/// The template's single slot is that id, copied as the request sent it: the
/// refusal is that the value does not name a git object at all, and one request
/// may carry TWO blob ids (`GetBlobDifferences` takes a before and an after side),
/// so naming the rejected one is the only way a client learns which side was at
/// fault. `ObjectId` carries no modelled constraint, so the form of a blob id is
/// checked by the handler that reads it rather than at the request boundary, and
/// this is what that check says.
pub fn invalid_blob_id_message(blob_id: &str) -> String {
    format!("{blob_id} is not a valid git object ID")
}

/// `FileTooLargeException`'s message: again the shape's own documented sentence,
/// naming nothing from the request, for every operation that refuses content it
/// cannot serve at that size.
pub const FILE_TOO_LARGE_MESSAGE: &str =
    "The specified file exceeds the file size limit for CodeCommit. For more information about \
     limits in CodeCommit, see Quotas in the CodeCommit User Guide.";

/// The combined size of all the file content one commit may carry, in bytes.
/// CodeCommit documents this on `FileContentSizeLimitExceededException`: "The
/// maximum file size is 6 MB, and the combined file content change size is 7 MB."
/// (The per-file 6 MB is `FileContent`'s modelled maximum, refused at the
/// boundary.)
pub const MAX_COMBINED_CONTENT_BYTES: usize = 7 * 1024 * 1024;

/// How many approval rule templates the store holds before
/// `CreateApprovalRuleTemplate` refuses with the modelled
/// `NumberOfRuleTemplatesExceededException`.
///
/// The emulator's OWN bound on the template map rather than a claim about any
/// account's adjustable quota: a caller creates one template per request, so this
/// is what stops the map growing without end. Every record it holds is released by
/// `DeleteApprovalRuleTemplate`.
pub const APPROVAL_RULE_TEMPLATE_QUOTA: usize = 1_000;

/// How many approval rule templates one repository may be associated with before
/// `AssociateApprovalRuleTemplateWithRepository` refuses with the modelled
/// `MaximumRuleTemplatesAssociatedWithRepositoryException`.
///
/// The emulator's OWN bound on one repository's association set, not a published
/// AWS quota; `DisassociateApprovalRuleTemplateFromRepository` is its release path.
pub const MAX_TEMPLATES_PER_REPOSITORY: usize = 100;

/// How many file entries one `CreateCommit` may carry before it refuses with the
/// modelled `MaximumFileEntriesExceededException`.
///
/// The bound CodeCommit publishes for the operation, and states in the refusal
/// itself ([`MAXIMUM_FILE_ENTRIES_EXCEEDED_MESSAGE`]: "The maximum limit for the
/// number of files is 100 in the AWS CodeCommit console and in the CreateCommit
/// API"). The `putFiles`, `deleteFiles` and `setFileModes` entries of one request
/// count TOGETHER against it, because each is one file the commit changes.
pub const MAX_FILE_ENTRIES_PER_COMMIT: usize = 100;

/// `MaximumFileEntriesExceededException`'s message: the FIXED sentence group
/// CodeCommit answers that code with, carrying nothing from the request — in
/// particular not how many entries the refused request held.
///
/// It is not the error shape's documented sentence, which is prose about the error
/// class; the service's own text states the bound, where it applies, and the two
/// ways out (several commits, or a git client). The bound it names is
/// [`MAX_FILE_ENTRIES_PER_COMMIT`].
pub const MAXIMUM_FILE_ENTRIES_EXCEEDED_MESSAGE: &str =
    "The commit cannot be created because it exceeds the maximum number of changed files allowed \
     in a commit. The maximum limit for the number of files is 100 in the AWS CodeCommit console \
     and in the CreateCommit API. Either create multiple commits to push these file changes, or \
     use a Git client.";

/// The smallest `maxResults` a paginated CodeCommit list operation accepts; one
/// below it is refused with `InvalidMaxResultsException`. CodeCommit documents the
/// member as "a non-zero, non-negative integer" and words the refusal "A valid
/// limit is between 1 and 1000".
pub const MIN_MAX_RESULTS: i32 = 1;

/// The largest `maxResults` a paginated CodeCommit list operation accepts — the
/// upper end of the same published range (see [`MIN_MAX_RESULTS`]).
pub const MAX_MAX_RESULTS: i32 = 1_000;

/// The page size a paginated list operation uses when the caller names no
/// `maxResults`: the largest page the service allows, so an unpaginated caller sees
/// everything in one answer.
pub const DEFAULT_MAX_RESULTS: i32 = MAX_MAX_RESULTS;

/// `InvalidMaxResultsException`'s message: the range of `maxResults` the operation
/// accepts, and nothing from the request.
///
/// One of the two places this crate words that code (the other is
/// [`NON_POSITIVE_MAX_RESULTS_MESSAGE`], for an operation that publishes no
/// maximum). The template is `A valid limit is between <min> and <max>` — the
/// sentence CodeCommit answers, which states the accepted range and does NOT echo
/// the rejected value. The bounds are arguments because an operation publishes its
/// own `maxResults` maximum (the paginated list operations' is
/// [`MAX_MAX_RESULTS`], and the recorded instance of this message is that
/// 1..[`MAX_MAX_RESULTS`] one; `GetCommentsForComparedCommit`'s is
/// [`COMMENTS_PAGE_MAX`]), so the TEMPLATE is what every such operation shares.
pub fn invalid_max_results_message(min: i32, max: i32) -> String {
    format!("A valid limit is between {min} and {max}")
}

/// `InvalidMaxResultsException`'s message where the operation publishes NO maximum
/// and so refuses only a value that is not a page size at all.
///
/// `GetDifferences` and `ListFileCommitHistory` document `maxResults`/`MaxResults`
/// as "A non-zero, non-negative integer used to limit the number of returned
/// results" and serve every positive value (shrunk to the page this service
/// builds), so there is no upper bound to state and this says the rule the member's
/// own documentation states instead. NOT a recorded message: the recording for this
/// code is the range form in [`invalid_max_results_message`], which an operation
/// with a published maximum answers. What is required of this refusal is its code
/// and its 400 status.
pub const NON_POSITIVE_MAX_RESULTS_MESSAGE: &str =
    "A valid limit is a non-zero, non-negative integer";

/// `InvalidContinuationTokenException`'s message: a template whose one slot is the
/// token the request sent, led by a capital `E`.
///
/// The one place this crate words that code, so the paginated listings that page
/// through [`page_of`] and the operations that carry their own cursor cannot word it
/// differently.
pub fn invalid_continuation_token_message(token: &str) -> String {
    format!("Error parsing next token: {token}")
}

/// `CommitIdRequiredException`'s message: the sentence CodeCommit answers that class
/// with — the requirement, then that nothing satisfied it. The one place this crate
/// words that code, for the request boundary (`crate::service`) and for the handlers
/// that check a commit id the model leaves unconstrained alike.
pub const COMMIT_ID_REQUIRED_MESSAGE: &str =
    "A commit ID is required, but no commit ID was specified.";

/// `InvalidCommitIdException`'s message: likewise the shape's own documented
/// sentence, worded here once for every operation that raises the code.
pub const INVALID_COMMIT_ID_MESSAGE: &str = "The specified commit ID is not valid.";

/// The longest `clientRequestToken` CodeCommit accepts, in characters — the upper
/// bound the service's own `InvalidClientRequestTokenException` text states for the
/// member ("Client request tokens must be between 1 and 256 ... characters"). The
/// lower bound of that same sentence is why a token that carries no character at all
/// is malformed rather than absent.
pub const CLIENT_REQUEST_TOKEN_MAX_LENGTH: usize = 256;

/// Is this EXPLICITLY supplied `clientRequestToken` a token CodeCommit accepts?
///
/// An idempotency token is an opaque string a client computes, so its characters are
/// not constrained here — the SDKs prepopulate the member with a UUID, whose hyphens
/// are part of a token every caller sends — but its LENGTH is: a token has to carry
/// at least one character to name a request, and no more than
/// [`CLIENT_REQUEST_TOKEN_MAX_LENGTH`]. A present token that fails this names no
/// request and is not a token one could be deduplicated against, which is why every
/// operation carrying the member refuses it with `InvalidClientRequestTokenException`
/// instead of treating it as the member having been left out.
pub fn is_client_request_token(candidate: &str) -> bool {
    let characters = candidate.chars().count();
    (1..=CLIENT_REQUEST_TOKEN_MAX_LENGTH).contains(&characters)
}

/// `ClientRequestTokenRequiredException`'s message: a FIXED sentence naming nothing
/// from the request, for a request that carries no `clientRequestToken` AT ALL where
/// the operation requires one. Worded here once for the request boundary
/// (`crate::service`) and for the handler that requires the member the model leaves
/// optional.
pub const CLIENT_REQUEST_TOKEN_REQUIRED_MESSAGE: &str = "A client request token is required.";

/// `InvalidClientRequestTokenException`'s message: the sentence pair the service
/// words that code with — the failure, then the rule — naming nothing from the
/// request. Worded here once for every operation that takes a `clientRequestToken`.
pub const INVALID_CLIENT_REQUEST_TOKEN_MESSAGE: &str =
    "The client request token is not valid. Client request tokens must be between 1 and 256 \
     alphanumeric characters.";

/// Why a commit id named in a request addresses no commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitIdProblem {
    /// The id is absent (empty): the operations model
    /// `CommitIdRequiredException` for this.
    Missing,
    /// The id is not a git object name: `InvalidCommitIdException`.
    Invalid,
}

impl CommitIdProblem {
    /// What the refusal says. Each sentence is the one this crate words that code
    /// with ([`COMMIT_ID_REQUIRED_MESSAGE`], [`INVALID_COMMIT_ID_MESSAGE`]), which
    /// `crate::service` also uses when the request boundary refuses the member:
    /// one code, one message, and nothing naming the emulator's own validator.
    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => COMMIT_ID_REQUIRED_MESSAGE,
            Self::Invalid => INVALID_COMMIT_ID_MESSAGE,
        }
    }
}

/// Does this commit id address a commit at all? The model leaves `CommitId`
/// unconstrained, so every operation that takes one classifies it here and only
/// maps the outcome onto the error IT models for that code.
pub fn checked_commit_id(commit_id: &str) -> Result<(), CommitIdProblem> {
    if commit_id.is_empty() {
        return Err(CommitIdProblem::Missing);
    }
    if !is_object_id(commit_id) {
        return Err(CommitIdProblem::Invalid);
    }
    Ok(())
}

/// `BeforeCommitIdAndAfterCommitIdAreSameException`'s message: the sentence pair
/// the model states for that error shape, naming neither commit. The one place
/// this crate words that code, so the operations that compare the two sides of a
/// comparison cannot word it differently.
pub const BEFORE_AND_AFTER_COMMIT_ID_SAME_MESSAGE: &str =
    "The before commit ID and the after commit ID are the same, which is not valid. The before \
     commit ID and the after commit ID must be different commit IDs.";

/// The file a commit leaves behind in a folder that its deletions emptied, when the
/// caller asked to keep the folder. CodeCommit documents `keepEmptyFolders`: "If
/// true, a ..gitkeep file is created for empty folders."
pub const KEEP_EMPTY_FOLDERS_PLACEHOLDER: &str = ".gitkeep";

/// How many distinct principals' reactions one comment holds before
/// `PutCommentReaction` refuses a new one.
///
/// CodeCommit documents the per-user half of this limit — "Reactions are limited
/// to one reaction per user for each individual comment ID" — and this is the
/// emulator's OWN bound on the number of such users, because the principal a
/// reaction is keyed by comes from the request's own credential and would
/// otherwise let one caller grow a comment's reaction map without end. A caller
/// already holding a reaction always gets to change it; it is only a new
/// principal past the cap that is refused, with the modelled
/// `ReactionLimitExceededException`.
pub const MAX_REACTIONS_PER_COMMENT: usize = 100;

/// One reaction CodeCommit accepts on a comment, in every form the API names it.
///
/// A reaction is REPORTED in two places, in a different form in each: as a
/// reaction VALUE, where it is the reaction's name (the keys of a comment's
/// `reactionCounts` and the entries of its `callerReactions`, both typed
/// `ReactionValue` by the model —
/// `smithy_query(query="shape",shape="ReactionCountsMap,CallerReactions")`), and
/// as the three spellings of `ReactionValueFormats`, which carries the graphic,
/// the short code and the Unicode codepoint. It is ACCEPTED in ANY of those
/// forms, so a row holds each reported spelling once and lists beside them the
/// further short codes a caller may send the same reaction as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    /// The reaction's name, which is the form a stored reaction is held as and
    /// reported under wherever a comment carries one as a reaction value.
    pub name: &'static str,
    /// The emoji short code this service reports the reaction under.
    pub short_code: &'static str,
    /// The Emoji Version 1.0 graphic of the reaction.
    pub emoji: &'static str,
    /// The further short codes that name this same reaction.
    pub aliases: &'static [&'static str],
}

impl Reaction {
    /// The Unicode codepoint for the reaction, as CodeCommit reports it: `U+` and
    /// the hexadecimal scalar value of the graphic's code point, at least four
    /// digits wide.
    pub fn unicode(&self) -> String {
        self.emoji
            .chars()
            .map(|point| format!("U+{:04X}", u32::from(point)))
            .collect::<Vec<String>>()
            .join(" ")
    }

    /// This reaction in the three forms the model reports it in, which is how
    /// every reader of a stored reaction renders one.
    pub fn formats(&self) -> ReactionValueFormats {
        ReactionValueFormats {
            emoji: Some(self.emoji.to_string()),
            short_code: Some(self.short_code.to_string()),
            unicode: Some(self.unicode()),
        }
    }

    /// Does `normalized` — a reaction value already reduced to the comparable
    /// form [`canonical_reaction`] reduces to — name this reaction? Every form
    /// the API publishes for a reaction names it: its name, its short code and
    /// the further short codes that spell the same reaction, its graphic, and
    /// its Unicode codepoint.
    fn named_by(&self, normalized: &str) -> bool {
        self.name.eq_ignore_ascii_case(normalized)
            || self.short_code == normalized
            || self.emoji == normalized
            || self.aliases.contains(&normalized)
            // Only a value spelled as a codepoint is rendered as one to compare.
            || (normalized.starts_with("u+") && self.unicode().eq_ignore_ascii_case(normalized))
    }
}

/// The emoji reactions CodeCommit accepts on a comment.
///
/// One row per reaction, in the order the catalog lists them, which is the order a
/// read of a comment's reactions reports them in. The catalog is the emoji
/// reaction table in the CodeCommit User Guide, which the model's `reactionValue`
/// documentation links to
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`).
pub const REACTION_TABLE: &[Reaction] = &[
    Reaction {
        name: "THUMBSUP",
        short_code: ":thumbsup:",
        emoji: "\u{1f44d}",
        aliases: &[":+1:"],
    },
    Reaction {
        name: "THUMBSDOWN",
        short_code: ":thumbsdown:",
        emoji: "\u{1f44e}",
        aliases: &[":-1:"],
    },
    Reaction {
        name: "SMILE",
        short_code: ":smile:",
        emoji: "\u{1f604}",
        aliases: &[],
    },
    Reaction {
        name: "CONFUSED",
        short_code: ":confused:",
        emoji: "\u{1f615}",
        aliases: &[],
    },
    Reaction {
        name: "EYES",
        short_code: ":eyes:",
        emoji: "\u{1f440}",
        aliases: &[],
    },
    Reaction {
        name: "HEART",
        short_code: ":heart:",
        emoji: "\u{2764}",
        aliases: &[],
    },
    Reaction {
        name: "HOORAY",
        short_code: ":hooray:",
        emoji: "\u{1f389}",
        aliases: &[":tada:"],
    },
    Reaction {
        name: "ROCKET",
        short_code: ":rocket:",
        emoji: "\u{1f680}",
        aliases: &[],
    },
];

/// The reaction value that REMOVES the caller's reaction rather than naming one:
/// "To remove a reaction, provide a value of blank or null. You can also provide
/// the value of none."
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`).
pub const REACTION_NONE: &str = "none";

/// The reaction a reaction value names, or `None` when the reaction table holds
/// no such reaction.
///
/// A caller may send a reaction as any of the forms the table lists — its name,
/// a short code, the graphic or the Unicode codepoint — so the value is first
/// reduced to one comparable form: trimmed, lowercased, and with the Unicode
/// variation selector an emoji may carry (U+FE0F) removed, so `"❤️"` and `"❤"`
/// are the one reaction the table lists.
pub fn canonical_reaction(value: &str) -> Option<&'static Reaction> {
    let normalized: String = value
        .trim()
        .chars()
        .filter(|character| *character != '\u{fe0f}')
        .flat_map(char::to_lowercase)
        .collect();
    REACTION_TABLE
        .iter()
        .find(|reaction| reaction.named_by(&normalized))
}

/// Does this reaction value ask for the caller's reaction to be REMOVED rather
/// than naming one? Blank, or the documented value `none`.
pub fn is_reaction_removal(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case(REACTION_NONE)
}

/// Per-service state for com-amazonaws-codecommit-codecommit-20150413. A handler reaches it through [`service_state`].
///
/// Bounded on every axis a caller drives: at most [`REPOSITORY_QUOTA`]
/// repositories, each holding at most [`MAX_FILES_PER_TREE`] files per tree,
/// [`MAX_COMMITS_RETAINED`] projected commits, [`MAX_OBJECTS_PER_REPOSITORY`]
/// git objects, [`MAX_BRANCHES_PER_REPOSITORY`] branches,
/// [`MAX_TAGS_PER_REPOSITORY`] tags and [`MAX_COMMENTS_PER_REPOSITORY`] comments,
/// with every object no ref reaches released by [`Repository::prune_history`], and
/// at most [`APPROVAL_RULE_TEMPLATE_QUOTA`] approval rule templates.
#[derive(Debug, Default)]
pub struct ComAmazonawsCodecommitCodecommit20150413State {
    /// Live repositories, keyed by `repositoryName` (unique per account).
    pub repositories: BTreeMap<String, Repository>,
    /// Live approval rule templates, keyed by `approvalRuleTemplateName` (unique
    /// per account per Region). Which repositories a template is associated with
    /// is held once, on the repository ([`Repository::associated_templates`]), so
    /// the two sides of an association cannot disagree.
    pub approval_rule_templates: BTreeMap<String, ApprovalRuleTemplateRecord>,
    /// How many pull requests this account has created, which is what the next
    /// one's `pullRequestId` counts from: CodeCommit ids a pull request with a
    /// decimal integer that is unique per ACCOUNT rather than per repository
    /// (`GetPullRequest` takes a `pullRequestId` and nothing else), and ids are
    /// never reused, so this counter only ever goes up.
    pub pull_requests_created: u64,
    /// The address this emulator serves the git transport of EVERY repository at:
    /// the authority each announced clone URL names, and the listener that answers
    /// there ([`git_endpoint`]). Opened by the call that creates a repository and
    /// held for the life of the store, so the announced address stays dialable for
    /// as long as a repository can be cloned; `None` on an engine that vends no
    /// listener, where a clone URL names the endpoint the asking request arrived at
    /// instead.
    pub git_endpoint: Option<git_endpoint::GitEndpoint>,
}

impl ComAmazonawsCodecommitCodecommit20150413State {
    /// The pull request this `pullRequestId` names, wherever it was created.
    ///
    /// A pull request id is addressable on its own — `GetPullRequest` takes
    /// nothing else — while a pull request's lifetime is its repository's, so
    /// pull requests live on the repository that holds them and are found
    /// through this one lookup rather than through a second index that could
    /// disagree with it, exactly as [`Self::comment`] finds a comment.
    pub fn pull_request(&self, pull_request_id: u64) -> Option<&PullRequestRecord> {
        self.repositories
            .values()
            .find_map(|repository| repository.pull_requests.get(&pull_request_id))
    }

    /// The same lookup, for a caller that goes on to modify the pull request.
    pub fn pull_request_mut(&mut self, pull_request_id: u64) -> Option<&mut PullRequestRecord> {
        self.repositories
            .values_mut()
            .find_map(|repository| repository.pull_requests.get_mut(&pull_request_id))
    }

    /// The comment this `commentId` names, wherever it was posted.
    ///
    /// A comment id is addressable on its own — `GetComment` takes nothing else —
    /// while a comment's lifetime is its repository's, so the comments live on the
    /// repository that holds them and are found through this one lookup rather
    /// than through a second index that could disagree with it.
    pub fn comment(&self, comment_id: &str) -> Option<&CommentRecord> {
        self.repositories
            .values()
            .find_map(|repository| repository.comments.get(comment_id))
    }

    /// The same lookup, for a caller that goes on to modify the comment.
    pub fn comment_mut(&mut self, comment_id: &str) -> Option<&mut CommentRecord> {
        self.repositories
            .values_mut()
            .find_map(|repository| repository.comments.get_mut(comment_id))
    }

    /// Is this approval rule template associated with any repository?
    ///
    /// An association is held once, on the repository
    /// ([`Repository::associated_templates`]), so the answer is a scan of the
    /// repository map — the same stored association
    /// `ListRepositoriesForApprovalRuleTemplate` reports and
    /// `DeleteApprovalRuleTemplate` refuses on, rather than a second index that
    /// could disagree with it.
    pub fn template_is_associated(&self, template_name: &str) -> bool {
        self.repositories
            .values()
            .any(|repository| repository.associated_templates.contains(template_name))
    }

    /// Re-key the approval rule template stored under `old_name` to `new_name`,
    /// answering the stored template under its new name.
    ///
    /// The template map is keyed by the name, and an association is held by that
    /// same name on each repository, so a rename moves BOTH: the repositories
    /// associated with the template stay associated with it, because it is the same
    /// template. Nothing else about it changes — its id (the identity a rename does
    /// not alter), its content, its content digest and its creation date all carry
    /// over, while the change moves `lastModifiedDate` and `lastModifiedUser` onto
    /// this call, exactly as the other two template updates do.
    ///
    /// Answers `None` when no template is stored under `old_name`. The CALLER
    /// decides whether `new_name` may be taken: this is the mechanics of the move,
    /// so it would overwrite a template already stored under `new_name`.
    pub fn rename_approval_rule_template(
        &mut self,
        old_name: &str,
        new_name: &str,
        modified_by: String,
        modified: SystemTime,
    ) -> Option<&ApprovalRuleTemplateRecord> {
        let mut template = self.approval_rule_templates.remove(old_name)?;
        template.rename(new_name, modified_by, modified);
        for repository in self.repositories.values_mut() {
            if repository.associated_templates.remove(old_name) {
                repository
                    .associated_templates
                    .insert(new_name.to_string());
            }
        }
        self.approval_rule_templates
            .insert(new_name.to_string(), template);
        self.approval_rule_templates.get(new_name)
    }
}

/// One repository and everything reachable from it.
#[derive(Debug, Clone)]
pub struct Repository {
    pub repository_id: String,
    pub account_id: String,
    pub name: String,
    /// The region this repository lives in, as the request that created it was
    /// scoped for. Held because the ARN and both clone URLs are built from the
    /// region AND the name, so a rename has to rebuild them for the SAME region.
    pub region: String,
    pub description: Option<String>,
    pub default_branch: Option<String>,
    pub arn: String,
    /// The KMS key this repository is encrypted under, in the single form
    /// [`canonical_encryption_key`] holds every selector in. Never absent: a
    /// repository created without a key of its own carries
    /// [`DEFAULT_ENCRYPTION_KEY_ALIAS`], the managed key `kmsKeyId`'s own
    /// documentation names — "If no key is specified, the default
    /// aws/codecommit Amazon Web Services managed key is used"
    /// (`smithy_query(query="doc",shape="CreateRepositoryInput",member="kmsKeyId")`).
    pub kms_key_id: String,
    /// The id of this repository's tagging record, reported beside the tags
    /// themselves. Minted with the repository and stable for its life, so two
    /// reads of one repository's tags report the same id and no two
    /// repositories share one.
    pub tagging_id: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// Branches (the `refs/heads/` namespace), keyed by branch name.
    pub branches: BTreeMap<String, Branch>,
    /// Tags (the `refs/tags/` namespace), keyed by tag name, each naming the
    /// object the tag points at. CodeCommit has no control-plane API that
    /// creates a tag; a tag gets here through the git transport.
    pub git_tags: BTreeMap<String, String>,
    /// The commit PROJECTION: one record per commit the control plane can read.
    pub commits: BTreeMap<String, Commit>,
    /// The git object store: every blob, tree, commit and tag this repository
    /// holds, keyed by object name.
    pub objects: BTreeMap<String, Object>,
    /// The names of the approval rule templates associated with this repository.
    /// A set, so associating the same template twice is the no-op CodeCommit makes
    /// it, and bounded by [`MAX_TEMPLATES_PER_REPOSITORY`].
    pub associated_templates: BTreeSet<String>,
    /// The sequence number the next commit takes, so the oldest commits can be
    /// told from the newest when [`Repository::prune_history`] runs.
    pub next_commit_sequence: u64,
    /// Comments posted on this repository, keyed by `commentId`.
    pub comments: BTreeMap<String, CommentRecord>,
    /// Each `clientRequestToken` a comment was posted with, mapped to the
    /// `commentId` that first request created, so a repeated request carrying the
    /// same token returns that comment instead of posting a second one.
    pub comment_tokens: BTreeMap<String, String>,
    /// The sequence number the next comment takes: comments are reported in the
    /// order they were posted.
    pub next_comment_sequence: u64,
    /// Commits this repository keeps although NO ref names them: what
    /// `CreateUnreferencedMergeCommit` writes, which "can only be accessed using
    /// the GetCommit API or through git commands such as git fetch". They are
    /// roots for [`Repository::prune_history`] exactly as a ref is — an id a
    /// response handed out has to stay readable — but they are NOT refs, so they
    /// are never advertised to a git client by [`Repository::refs`]. Bounded by
    /// [`MAX_UNREFERENCED_COMMITS`].
    pub unreferenced_commits: BTreeSet<String>,
    /// Pull requests opened against this repository, keyed by the numeric
    /// `pullRequestId`, so the map's order IS the order the ids were minted in.
    /// Bounded by [`MAX_PULL_REQUESTS_PER_REPOSITORY`].
    pub pull_requests: BTreeMap<u64, PullRequestRecord>,
    /// Each `clientRequestToken` a pull request was created with, mapped to the
    /// `pullRequestId` that first request created, so a repeated request
    /// carrying the same token returns that pull request instead of opening a
    /// second one.
    pub pull_request_tokens: BTreeMap<String, u64>,
    /// The merge commits this repository already holds, keyed by the PAIR of
    /// commits that were merged — `(source commit id, destination commit id)` —
    /// each naming the commit that merge wrote.
    ///
    /// Making a merge is a write (`CreateUnreferencedMergeCommit` and the branch
    /// and pull-request merge handlers) and reading one back is `GetMergeCommit`,
    /// so the merge commit a merge wrote is RECORDED here rather than recomputed
    /// from history: a squash merge's commit records the destination parent
    /// alone, so nothing in the commit object itself says which source it
    /// collapsed. A fast-forward merge writes no merge commit at all, so it
    /// records none.
    ///
    /// Bounded by the commit projection: an entry whose merge commit or whose
    /// merged pair the repository no longer holds is released by
    /// [`Repository::release_stale_merge_commits`], which
    /// [`Repository::prune_history`] runs.
    pub merge_commits: BTreeMap<(String, String), String>,
    /// The trigger configuration `PutRepositoryTriggers` last wrote for this
    /// repository, or `None` while no caller has written one. The triggers and
    /// the id of the write that stored them are held TOGETHER
    /// ([`TriggerConfiguration`]), so the id a write returned and the triggers
    /// it stored cannot disagree.
    pub trigger_configuration: Option<TriggerConfiguration>,
}

/// One repository's trigger configuration: the triggers the last
/// `PutRepositoryTriggers` stored, and the `configurationId` that write minted
/// for them.
///
/// The operation "replaces all triggers for a repository", so a configuration is
/// written WHOLE rather than merged, and a write that stores no trigger at all
/// (an empty list) still establishes a configuration — which is how a caller
/// deletes every trigger.
#[derive(Debug, Clone)]
pub struct TriggerConfiguration {
    pub configuration_id: String,
    pub triggers: Vec<RepositoryTrigger>,
}

/// A branch: its name and the commit it points at (its tip).
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub commit_id: String,
}

/// A commit. `files` is the FULL tree snapshot at this commit, so reading a file
/// at any commit specifier is a lookup rather than a replay of history.
#[derive(Debug, Clone)]
pub struct Commit {
    pub commit_id: String,
    pub tree_id: String,
    pub parents: Vec<String>,
    pub message: Option<String>,
    pub author: UserInfo,
    pub committer: UserInfo,
    pub files: BTreeMap<String, FileEntry>,
    /// Write order within the repository: the lowest sequence is the oldest
    /// commit, which is what [`Repository::prune_history`] drops first.
    pub sequence: u64,
}

/// One file in a commit's tree.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub blob_id: String,
    pub file_mode: String,
    pub size: i64,
}

/// The host a URL this emulator announces names when the asking request carries
/// no address at all — only an in-process caller that builds a path-only URI with
/// no `Host` does. Loopback is where the emulator binds by default
/// (`EMULATOR_BIND_ADDRESS`), so the announced URL still names this emulator
/// rather than a host it does not serve; it is the fallback
/// `service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader` uses for the
/// same question about its own playback URL.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The endpoint a client reached this emulator at: the scheme and authority
/// every URL the emulator announces for itself has to name.
///
/// CodeCommit's `cloneUrlHttp` is the address the repository's git transport
/// answers on, so in an emulator it is the address THIS emulator answers on —
/// the endpoint the request arrived at — and not the public AWS git host, which
/// is a different server entirely and would send a client that follows the URL
/// off the emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingEndpoint {
    pub scheme: String,
    pub authority: String,
    /// The `host:port` this emulator serves the GIT transport at, when it has an
    /// address of its own for it ([`git_endpoint`]): the authority `cloneUrlHttp`
    /// names, under the scheme that address is served in the clear on.
    ///
    /// A clone URL is dialled by a stock git client with the trust material it
    /// already has, and the emulator's own front door presents a certificate no
    /// default trust store accepts — so the git transport is announced on the
    /// address opened for it rather than on the one the asking request arrived at.
    /// `None` on an engine that vends no listener, where the announced URL falls
    /// back to the endpoint the request arrived at: the address this emulator is
    /// reached on is still the only one it answers git at.
    pub git_authority: Option<String>,
}

impl ServingEndpoint {
    /// The endpoint this request arrived at: the `Host` header (or, on HTTP/2,
    /// the URI authority), under the scheme the request names.
    ///
    /// A request that carries neither — only an in-process caller that builds a
    /// path-only URI with no `Host` — falls back to [`LOOPBACK_HOST`], so every
    /// URL this service announces names an address this emulator serves and never
    /// one it does not. Each source is read on its own, in that order, and the
    /// FIRST one that names an address wins: a `Host` header carrying nothing
    /// leaves the URI's authority still to be read.
    pub fn from_request(parts: &::http::request::Parts) -> Self {
        let authority = [
            parts
                .headers
                .get(::http::header::HOST)
                .and_then(|host| host.to_str().ok()),
            parts.uri.authority().map(::http::uri::Authority::as_str),
        ]
        .into_iter()
        .flatten()
        .find(|authority| !authority.is_empty())
        .unwrap_or(LOOPBACK_HOST)
        .to_string();
        let scheme = parts
            .uri
            .scheme_str()
            .or_else(|| {
                parts
                    .headers
                    .get("x-forwarded-proto")
                    .and_then(|proto| proto.to_str().ok())
            })
            // The emulator's own listener serves HTTPS, which is also the only
            // scheme CodeCommit publishes a clone URL for.
            .unwrap_or("https")
            .to_string();
        Self {
            scheme,
            authority,
            git_authority: None,
        }
    }

    /// The same endpoint, announcing the git transport at the address this emulator
    /// opened for it ([`git_endpoint::ensure_git_endpoint`]). `None` leaves the
    /// announced clone URL naming the endpoint the request arrived at.
    #[must_use]
    pub fn serving_git_at(mut self, git_authority: Option<String>) -> Self {
        self.git_authority = git_authority;
        self
    }

    /// The HTTPS clone URL for a repository: the git smart-HTTP endpoint this
    /// emulator serves for it (see [`crate::service::git_http`]).
    ///
    /// The ADDRESS is the one that transport is served at — the git listener's when
    /// this emulator opened one, and the endpoint the asking request arrived at
    /// otherwise — because a client hands this value straight to git, which dials it
    /// as it stands (see [`Self::git_authority`]).
    pub fn clone_url_http(&self, name: &str) -> String {
        match &self.git_authority {
            Some(authority) => format!(
                "{}://{authority}/v1/repos/{name}",
                git_endpoint::GIT_SCHEME
            ),
            None => format!("{}://{}/v1/repos/{name}", self.scheme, self.authority),
        }
    }

    /// The SSH clone URL for a repository, naming the same host.
    ///
    /// The emulator serves no SSH transport — there is no SSH listener to bind —
    /// so this URL names a host that answers git over HTTPS only. It still names
    /// the emulator rather than the public AWS host, because a client that
    /// follows it must fail against the emulator instead of being sent to a real
    /// AWS endpoint where its credentials do not exist. The port is omitted: a
    /// port in the authority is the HTTP listener's, which an SSH URL must not
    /// claim.
    pub fn clone_url_ssh(&self, name: &str) -> String {
        format!("ssh://{}/v1/repos/{name}", self.host())
    }

    /// The authority with any `:port` removed.
    fn host(&self) -> &str {
        let after_userinfo = self
            .authority
            .rsplit_once('@')
            .map_or(self.authority.as_str(), |(_, host)| host);
        if after_userinfo.starts_with('[') {
            // An IPv6 literal keeps its brackets; only a port after them is cut.
            return after_userinfo
                .rsplit_once("]:")
                .map_or(after_userinfo, |(host, _)| &after_userinfo[..host.len() + 1]);
        }
        after_userinfo
            .rsplit_once(':')
            .map_or(after_userinfo, |(host, _)| host)
    }
}

impl Commit {
    /// This commit as the modeled `Commit` shape — the one place that projection
    /// is written, so every operation that reports a stored commit agrees about
    /// it, exactly as [`Repository::metadata`] does for a repository.
    ///
    /// `parents` is always present, empty for a root commit, because the model's
    /// `ParentList` is a list of the full ids of the commits this one was built
    /// on. `additionalData` carries whatever a git host records beyond the
    /// modelled members; a commit object this service stores records nothing
    /// beyond them, so it is the EMPTY string — a caller can then tell "no other
    /// data" from a member a reader did not report at all. The author's and the
    /// committer's name, email and date are reported the same way
    /// ([`identity_info`]).
    pub fn info(&self) -> crate::ops::types::Commit {
        crate::ops::types::Commit {
            commit_id: Some(self.commit_id.clone()),
            tree_id: Some(self.tree_id.clone()),
            parents: Some(self.parents.clone()),
            message: self.message.clone(),
            author: Some(identity_info(&self.author)),
            committer: Some(identity_info(&self.committer)),
            additional_data: Some(String::new()),
        }
    }

    /// The id of the blob this commit's tree holds at `path`, or `None` where the
    /// tree does not hold that path at all.
    pub fn blob_at(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|entry| entry.blob_id.as_str())
    }
}

/// The comparison a comment was posted on: the two commits that establish its
/// directionality, and the blobs of the commented file in each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedCommit {
    pub before_commit_id: Option<String>,
    pub after_commit_id: String,
    pub before_blob_id: Option<String>,
    pub after_blob_id: Option<String>,
}

/// One comment, as posted. `comparison` is what the comment is filed under, so a
/// read of one comparison reports exactly the comments posted on it.
#[derive(Debug, Clone)]
pub struct CommentRecord {
    pub comment_id: String,
    pub content: String,
    /// The content the comment was POSTED with, which an idempotency token is
    /// judged against: a token replay repeats the request the token was first
    /// accepted for, so a later edit of the comment must not turn that replay
    /// into a parameter mismatch.
    pub posted_content: String,
    /// The comment this one replies to; `None` for the root of a thread.
    pub in_reply_to: Option<String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub author_arn: String,
    pub deleted: bool,
    pub client_request_token: Option<String>,
    /// The pull request this comment was posted ON, for a comment posted on one;
    /// `None` for a comment posted on a commit comparison alone. A pull request
    /// comment is filed under a comparison too — the two commits the pull request
    /// is between — so this is what tells the two kinds of comment apart, and a
    /// read of a comparison's comments reports only the comparison's own.
    pub pull_request_id: Option<u64>,
    pub comparison: ComparedCommit,
    pub location: Option<Location>,
    /// The emoji reactions left on this comment — each the NAME of a reaction
    /// the service's catalog holds ([`Reaction::name`]) — keyed by the ARN of the
    /// principal that left it, so a principal holds at most one reaction
    /// per comment ("Reactions are limited to one reaction per user for each
    /// individual comment ID") and `callerReactions` is that principal's own
    /// entry. Bounded by [`MAX_REACTIONS_PER_COMMENT`] principals.
    pub reactions: BTreeMap<String, String>,
    /// Post order within the repository: the lowest sequence is the oldest
    /// comment, and that is the order comments are reported in.
    pub sequence: u64,
}

impl CommentRecord {
    /// This comment as the modeled `Comment` shape, for the caller that is
    /// asking — the one place that projection is written, so no two readers of a
    /// stored comment can disagree about it. `callerReactions` is that
    /// principal's own reaction, the one member of a comment that depends on who
    /// is asking.
    pub fn comment_as(&self, caller_arn: &str) -> Comment {
        // `reactionCounts` is how many principals hold each reaction, counted
        // from the stored map rather than tallied anywhere else. It is "a string
        // to integer map that represents the number of individual users who have
        // responded to a comment with the specified reactions": a comment nobody
        // has reacted to has responded-to-with nothing, which is the EMPTY map
        // rather than no map at all.
        let mut reaction_counts: HashMap<String, i32> = HashMap::new();
        for reaction in self.reactions.values() {
            *reaction_counts.entry(reaction.clone()).or_insert(0) += 1;
        }
        Comment {
            comment_id: Some(self.comment_id.clone()),
            // A deleted comment keeps its record but reports no content, which is
            // what `DeleteCommentContent` documents.
            content: (!self.deleted).then(|| self.content.clone()),
            in_reply_to: self.in_reply_to.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            author_arn: Some(self.author_arn.clone()),
            deleted: self.deleted,
            client_request_token: self.client_request_token.clone(),
            caller_reactions: self
                .reactions
                .get(caller_arn)
                .map(|reaction| vec![reaction.clone()]),
            reaction_counts: Some(reaction_counts),
        }
    }
}

/// Every comment of one repository by the comment it replies to: the reply edges
/// of that repository's whole comment map, built ONCE per request.
///
/// [`comment_thread`] walks these edges per root, so without this index each step
/// would rescan the whole map and rendering one page would cost roots x comments.
/// The edges live here rather than in one reader because a reply is reported
/// inside its root's thread by every read of a comment thread — the comparison's
/// and the pull request's alike — so both walk the same stored edges.
pub fn reply_index(repository: &Repository) -> HashMap<&str, Vec<&CommentRecord>> {
    let mut index: HashMap<&str, Vec<&CommentRecord>> = HashMap::new();
    for comment in repository.comments.values() {
        if let Some(parent) = comment.in_reply_to.as_deref() {
            index.entry(parent).or_default().push(comment);
        }
    }
    for replies in index.values_mut() {
        replies.sort_by_key(|comment| comment.sequence);
    }
    index
}

/// One comment thread: the root comment, then the comments replying to it, each
/// level in the order it was posted. A comment and its replies are one entry of a
/// comment read, which is why they are collected together.
///
/// Each comment is expanded AT MOST ONCE, tracked in `expanded`: the reply edges
/// are stored data, so a record whose `in_reply_to` named itself or an ancestor
/// would otherwise make this walk re-expand the same records for ever — growing
/// `members` without end while the store lock is held, which is a hang of the whole
/// service rather than of one request. Visiting each record once also bounds the
/// thread by the repository's comment count, itself bounded by
/// [`MAX_COMMENTS_PER_REPOSITORY`].
pub fn comment_thread<'a>(
    replies: &HashMap<&'a str, Vec<&'a CommentRecord>>,
    root: &'a CommentRecord,
) -> Vec<&'a CommentRecord> {
    let mut members: Vec<&CommentRecord> = vec![root];
    let mut expanded: BTreeSet<&str> = BTreeSet::from([root.comment_id.as_str()]);
    let mut next = 0usize;
    while next < members.len() {
        let parent = members[next].comment_id.as_str();
        next += 1;
        for reply in replies.get(parent).into_iter().flatten() {
            if expanded.insert(reply.comment_id.as_str()) {
                members.push(reply);
            }
        }
    }
    members
}

/// A location's members as a comparable key: two comments sit in the same place in
/// a comparison only when all three members agree. The modeled `Location` shape
/// derives no `PartialEq`, so this is how two of them are compared.
pub fn location_key(location: Option<&Location>) -> (Option<&str>, Option<i64>, Option<&str>) {
    match location {
        None => (None, None, None),
        Some(location) => (
            location.file_path.as_deref(),
            location.file_position,
            location.relative_file_version.as_deref(),
        ),
    }
}

/// How many lines a blob's bytes hold: the number of line separators, plus the
/// trailing line when the content does not end in one. This is what bounds the
/// `filePosition` a comment can name in that version of a file.
pub fn line_count(content: &[u8]) -> i64 {
    if content.is_empty() {
        return 0;
    }
    let separators = content.iter().filter(|byte| **byte == b'\n').count();
    let trailing = usize::from(!content.ends_with(b"\n"));
    i64::try_from(separators + trailing).unwrap_or(i64::MAX)
}

/// Why the content a comment is posted with cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentContentProblem {
    /// Nothing was written: the operations model
    /// `CommentContentRequiredException` for this.
    Required,
    /// Past [`MAX_COMMENT_CHARACTERS`]:
    /// `CommentContentSizeLimitExceededException`.
    TooLarge,
}

impl CommentContentProblem {
    /// What the refusal says. Worded here once, so every operation that posts a
    /// comment answers the same text for the same code.
    pub fn message(self) -> String {
        match self {
            Self::Required => COMMENT_CONTENT_REQUIRED_MESSAGE.to_string(),
            Self::TooLarge => format!(
                "the comment is too large: comments are limited to {MAX_COMMENT_CHARACTERS} characters"
            ),
        }
    }
}

/// Is this content a comment this service accepts? The model puts no length bound
/// on `Content`, so both bounds are checked here — the one place every operation
/// that posts a comment classifies its content.
pub fn checked_comment_content(content: &str) -> Result<(), CommentContentProblem> {
    if content.is_empty() {
        return Err(CommentContentProblem::Required);
    }
    if content.chars().count() > MAX_COMMENT_CHARACTERS {
        return Err(CommentContentProblem::TooLarge);
    }
    Ok(())
}

/// Why the `location` a comment names is not a place in a comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentLocationProblem {
    /// The location names a position or a version but no file at all:
    /// `InvalidFileLocationException`.
    FileLocation,
    /// `PathRequiredException`.
    PathRequired,
    /// `InvalidPathException`, carrying the path as the request spelled it.
    InvalidPath(String),
    /// A `relativeFileVersion` outside [`RELATIVE_FILE_VERSION_BEFORE`] and
    /// [`RELATIVE_FILE_VERSION_AFTER`]: `InvalidRelativeFileVersionEnumException`.
    RelativeFileVersion(String),
    /// A position below the first line: `InvalidFilePositionException`.
    FilePosition,
}

impl CommentLocationProblem {
    /// What the refusal says, worded here once for every operation that places a
    /// comment.
    pub fn message(&self) -> String {
        match self {
            Self::FileLocation => {
                "the location of the file is not valid: it names no file path".to_string()
            }
            Self::PathRequired => "a file path is required".to_string(),
            Self::InvalidPath(file_path) => format!("the path {file_path} is not valid"),
            Self::RelativeFileVersion(version) => format!(
                "the relative file version {version} is not valid: expected {RELATIVE_FILE_VERSION_BEFORE} or {RELATIVE_FILE_VERSION_AFTER}"
            ),
            Self::FilePosition => {
                "the position is not valid: a line number starts at 1".to_string()
            }
        }
    }
}

/// The place a comment is posted at, as this service HOLDS it, or why the request
/// names no such place.
///
/// The file path is reduced to the form a git tree holds (as `GetFile` does), so
/// the same file is the same location however it is spelled. A location naming
/// nothing at all places the comment nowhere, so it is the same as leaving the
/// optional member out. Every operation that posts a comment normalizes its
/// location here, so they cannot disagree about where a comment sits.
pub fn normalized_comment_location(
    location: Option<Location>,
) -> Result<Option<Location>, CommentLocationProblem> {
    let mut location =
        location.filter(|location| location_key(Some(location)) != (None, None, None));
    let Some(place) = location.as_mut() else {
        return Ok(None);
    };
    if place.file_path.is_none()
        && (place.file_position.is_some() || place.relative_file_version.is_some())
    {
        return Err(CommentLocationProblem::FileLocation);
    }
    if let Some(file_path) = place.file_path.as_deref() {
        place.file_path = Some(match normalize_path(file_path) {
            Ok(path) => path,
            Err(PathProblem::Missing) => return Err(CommentLocationProblem::PathRequired),
            Err(PathProblem::Invalid) => {
                return Err(CommentLocationProblem::InvalidPath(file_path.to_string()))
            }
        });
    }
    // `RelativeFileVersionEnum` is an enum, so a value outside it is already
    // refused at the boundary; this keeps the invariant explicit for every caller.
    if let Some(version) = place.relative_file_version.as_deref() {
        if version != RELATIVE_FILE_VERSION_BEFORE && version != RELATIVE_FILE_VERSION_AFTER {
            return Err(CommentLocationProblem::RelativeFileVersion(
                version.to_string(),
            ));
        }
    }
    // A line number starts at 1, so a position below that names no line.
    if place.file_position.is_some_and(|position| position < 1) {
        return Err(CommentLocationProblem::FilePosition);
    }
    Ok(location)
}

/// Why a comment cannot be anchored in the comparison its request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentAnchorProblem {
    /// The commented version of the comparison holds no such file:
    /// `PathDoesNotExistException`.
    PathDoesNotExist { file_path: String, version: String },
    /// The position names no line of the commented version of the file:
    /// `InvalidFilePositionException`.
    FilePosition {
        file_path: String,
        version: String,
        position: i64,
        lines: i64,
    },
    /// The before version was asked for in a comparison that has no before
    /// commit: `InvalidRelativeFileVersionEnumException`.
    NoBeforeVersion,
}

impl CommentAnchorProblem {
    /// What the refusal says, worded here once for every operation that anchors a
    /// comment.
    pub fn message(&self) -> String {
        match self {
            Self::PathDoesNotExist { file_path, version } => format!(
                "the path {file_path} does not exist in the {version} version of the comparison"
            ),
            Self::FilePosition {
                file_path,
                version,
                position,
                lines,
            } => format!(
                "the position {position} is not valid: {file_path} has {lines} line(s) in the \
                 {version} version of the comparison"
            ),
            Self::NoBeforeVersion => format!(
                "the relative file version {RELATIVE_FILE_VERSION_BEFORE} is not valid for a \
                 comparison with no before commit"
            ),
        }
    }
}

/// The blobs a comment is anchored to, as `(beforeBlobId, afterBlobId)`: the
/// commented file's blob in each side of the comparison, read out of the two
/// commits' stored tree snapshots.
///
/// A comment that names no file is anchored to no blob, so both are absent. The
/// version the comment is ON — `relativeFileVersion`, which defaults to the after
/// side of the comparison — is the one whose stored blob has to hold the path, and
/// whose own line count bounds the position the location may name ("Make sure that
/// the line number exists in the version of the file you want to comment on").
///
/// Both commits are assumed to be commits this repository holds: a caller checks
/// that first, because the error it answers for a commit the repository never had
/// is its own.
pub fn comment_anchor(
    repository: &Repository,
    before_commit_id: Option<&str>,
    after_commit_id: &str,
    location: Option<&Location>,
) -> Result<(Option<String>, Option<String>), CommentAnchorProblem> {
    let Some(file_path) = location.and_then(|location| location.file_path.as_deref()) else {
        return Ok((None, None));
    };
    let entry_in = |commit_id: &str| {
        repository
            .commits
            .get(commit_id)
            .and_then(|commit| commit.files.get(file_path))
            .cloned()
    };
    let before_entry = before_commit_id.and_then(entry_in);
    let after_entry = entry_in(after_commit_id);

    let version = location
        .and_then(|location| location.relative_file_version.clone())
        .unwrap_or_else(|| RELATIVE_FILE_VERSION_AFTER.to_string());
    let commented = if version == RELATIVE_FILE_VERSION_BEFORE {
        if before_commit_id.is_none() {
            // There is no before side to comment on.
            return Err(CommentAnchorProblem::NoBeforeVersion);
        }
        before_entry.clone()
    } else {
        after_entry.clone()
    };
    let commented = commented.ok_or_else(|| CommentAnchorProblem::PathDoesNotExist {
        file_path: file_path.to_string(),
        version: version.clone(),
    })?;
    if let Some(position) = location.and_then(|location| location.file_position) {
        let lines = repository.blob(&commented.blob_id).map_or(0, line_count);
        if position > lines {
            return Err(CommentAnchorProblem::FilePosition {
                file_path: file_path.to_string(),
                version,
                position,
                lines,
            });
        }
    }
    Ok((
        before_entry.map(|entry| entry.blob_id),
        after_entry.map(|entry| entry.blob_id),
    ))
}

/// One approval rule template and everything CodeCommit reports about it.
///
/// Which repositories it is associated with is NOT held here: that lives once, on
/// each repository's [`Repository::associated_templates`].
#[derive(Debug, Clone)]
pub struct ApprovalRuleTemplateRecord {
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The approval rule document in its canonical form — see
    /// [`canonical_json_document`]. It is canonicalized once, here, on the way in,
    /// so every reader of a stored template reports the same bytes and
    /// `ruleContentSha256` is the digest of those bytes.
    pub content: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// The ARN of the identity that made the most recent change, as CodeCommit
    /// reports it in `lastModifiedUser`.
    pub last_modified_user: String,
}

impl ApprovalRuleTemplateRecord {
    /// Build a template, deriving every generated field from the name, the content
    /// and the creation instant.
    pub fn new(
        name: &str,
        content: String,
        description: Option<String>,
        last_modified_user: String,
        created: SystemTime,
    ) -> Self {
        Self {
            template_id: approval_rule_template_id(name, created),
            name: name.to_string(),
            description: normalized_template_description(description),
            // The document is held canonicalized, never as the caller happened to
            // format it: whatever the submitted whitespace, the stored content,
            // `ruleContentSha256` and every later read agree.
            content: canonical_json_document(&content),
            creation_date: created,
            last_modified_date: created,
            last_modified_user,
        }
    }

    /// Set this template's description to the one a request carried, and record
    /// who changed it when.
    ///
    /// The description is normalized by [`normalized_template_description`], so a
    /// template never holds an empty one however it was set, and the change moves
    /// `lastModifiedDate` and `lastModifiedUser` onto this call: both report the
    /// most recent change to the template, not its creation.
    pub fn set_description(
        &mut self,
        description: Option<String>,
        modified_by: String,
        modified: SystemTime,
    ) {
        self.description = normalized_template_description(description);
        self.last_modified_user = modified_by;
        self.last_modified_date = modified;
    }

    /// This template as the modeled `ApprovalRuleTemplate` shape — the one place
    /// that projection is written, so `CreateApprovalRuleTemplate`,
    /// `GetApprovalRuleTemplate` and any later updater cannot disagree about a
    /// stored template.
    pub fn to_modeled(&self) -> ApprovalRuleTemplate {
        ApprovalRuleTemplate {
            approval_rule_template_id: Some(self.template_id.clone()),
            approval_rule_template_name: Some(self.name.clone()),
            approval_rule_template_description: self.description.clone(),
            approval_rule_template_content: Some(self.content.clone()),
            rule_content_sha256: Some(content_sha256(&self.content)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_user: Some(self.last_modified_user.clone()),
        }
    }

    /// Replace this template's content, recording who changed it and when.
    ///
    /// The one place a stored template's content is overwritten, so every updater
    /// attributes the change the same way: the content is canonicalized exactly as a
    /// created template's is ([`canonical_json_document`]) — an update can therefore
    /// not leave a template in a serialization a create would never have stored —
    /// and with it the `ruleContentSha256` derived from it, while the template's id,
    /// name, description and creation date are untouched.
    pub fn replace_content(&mut self, content: String, by: String, at: SystemTime) {
        self.content = canonical_json_document(&content);
        self.last_modified_user = by;
        self.last_modified_date = at;
    }

    /// Give this template another name, recording who changed it and when.
    ///
    /// The name is the only thing that moves: the template's id is its identity and
    /// is NOT re-derived from the new name, and its description, content, content
    /// digest and creation date carry over — so the template a caller renames is the
    /// same template, reachable under the new name. Which name the map keys it under
    /// is the store's part of the move
    /// ([`ComAmazonawsCodecommitCodecommit20150413State::rename_approval_rule_template`]).
    pub fn rename(&mut self, name: &str, by: String, at: SystemTime) {
        self.name = name.to_string();
        self.last_modified_user = by;
        self.last_modified_date = at;
    }
}

/// One pull request and everything CodeCommit reports about it.
///
/// A pull request lives on the repository its target names
/// ([`Repository::pull_requests`]), because that is what bounds its lifetime: a
/// deleted repository takes its pull requests with it.
#[derive(Debug, Clone)]
pub struct PullRequestRecord {
    /// The numeric `pullRequestId`, unique per account. Reported as its decimal
    /// text, which is what [`PullRequestRecord::to_modeled`] renders.
    pub pull_request_id: u64,
    pub title: String,
    pub description: Option<String>,
    pub creation_date: SystemTime,
    /// "The day and time of the last user or system activity on the pull
    /// request": the creation instant for a pull request nothing has touched
    /// yet, and the instant of each later change.
    pub last_activity_date: SystemTime,
    /// [`PULL_REQUEST_STATUS_OPEN`] until something closes it; the model admits
    /// no other transition.
    pub status: String,
    pub author_arn: String,
    pub client_request_token: Option<String>,
    /// The targets of the pull request. CodeCommit admits exactly one.
    pub targets: Vec<PullRequestTargetRecord>,
    /// The approval rules applied to this pull request, in the order they were
    /// applied: the ones derived from the repository's associated approval rule
    /// templates when it was created, and any rule added to it later.
    pub approval_rules: Vec<ApprovalRuleRecord>,
    /// The approval state each reviewer has recorded on this pull request, keyed
    /// by that reviewer's identity ARN and holding one of
    /// [`APPROVAL_STATE_APPROVE`] / [`APPROVAL_STATE_REVOKE`]. What an approval
    /// rule's conditions are counted against
    /// ([`PullRequestRecord::approval_rules_not_satisfied`]), so a rule is met by
    /// the approvals the pull request HOLDS rather than by anything a request
    /// says.
    pub approvals: BTreeMap<String, String>,
    /// The identity that set the approval rules of this pull request aside
    /// ([`OVERRIDE_STATUS_OVERRIDE`]), which is what lets it be merged while a
    /// rule still states conditions its approvals do not meet, and `None` while
    /// its rules' requirements apply. ONE field for both facts, so the override a
    /// merge reads and the overrider a reader reports cannot disagree: holding the
    /// requirements again ([`OVERRIDE_STATUS_REVOKE`]) leaves none stored, which
    /// is what the model documents for `overrideStatus` — "REVOKE status is not
    /// stored".
    pub approval_rules_overridden_by: Option<String>,
    /// How many times the reviewable state of this pull request has moved. It
    /// is what distinguishes one `revisionId` from the next — see
    /// [`PullRequestRecord::revision_id`] — so a caller that holds a revision id
    /// can tell whether it still names the revision it read.
    pub revision: u64,
    /// This pull request's event log, OLDEST FIRST: one entry per change made
    /// to the pull request, appended by the operation that made it through
    /// [`PullRequestRecord::record_event`]. Append-only, so the stored order IS
    /// the order the changes happened, which is the order
    /// `DescribePullRequestEvents` reports them in. Bounded by
    /// [`MAX_PULL_REQUEST_EVENTS_RETAINED`].
    pub events: Vec<PullRequestEventRecord>,
}

impl PullRequestRecord {
    /// The approval rule this pull request holds under this name, if it holds one.
    ///
    /// An `approvalRuleName` is unique within one pull request, so at most one rule
    /// can match, and both the operation that adds a rule and the one that deletes
    /// it ask here rather than each scanning its own way.
    pub fn approval_rule(&self, name: &str) -> Option<&ApprovalRuleRecord> {
        self.approval_rules
            .iter()
            .find(|rule| rule.approval_rule_name == name)
    }

    /// The reviewers whose LAST recorded approval state on this pull request is
    /// an approval — the approvals an approval rule's conditions are counted
    /// against.
    pub fn approvers(&self) -> BTreeSet<&str> {
        self.approvals
            .iter()
            .filter(|(_, state)| state.as_str() == APPROVAL_STATE_APPROVE)
            .map(|(reviewer, _)| reviewer.as_str())
            .collect()
    }

    /// The approvals this pull request holds, as the modeled `Approval` shapes, in
    /// reviewer order.
    ///
    /// An approval is one reviewer holding [`APPROVAL_STATE_APPROVE`], so only the
    /// approvers are reported: the model documents the state an `Approval` carries
    /// as "The state of the approval, APPROVE or REVOKE. REVOKE states are not
    /// stored", and a reviewer whose last recorded state is a revoke holds no
    /// approval to report. Built from the very set an approval rule's conditions
    /// are counted against ([`Self::approvers`]), so a reader of the approvals and
    /// a merge of the pull request cannot disagree about which ones it holds.
    pub fn approvals_to_modeled(&self) -> Vec<Approval> {
        self.approvers()
            .into_iter()
            .map(|reviewer| Approval {
                user_arn: Some(reviewer.to_string()),
                approval_state: Some(APPROVAL_STATE_APPROVE.to_string()),
            })
            .collect()
    }

    /// The approval state this pull request holds for one reviewer.
    ///
    /// An approval the pull request does not hold IS a non-approval, so a reviewer
    /// it has recorded nothing for is in [`APPROVAL_STATE_REVOKE`]: that is what
    /// makes setting a reviewer's state to it a request that asks for the state
    /// already held rather than a change.
    pub fn approval_state_of(&self, reviewer: &str) -> &str {
        self.approvals
            .get(reviewer)
            .map_or(APPROVAL_STATE_REVOKE, String::as_str)
    }

    /// Does `revision_id` name the CURRENT revision of this pull request?
    ///
    /// The question every operation asks of a request made against a revision —
    /// an older revision is one the pull request has moved past, which is what
    /// `RevisionNotCurrentException` reports.
    pub fn is_current_revision(&self, revision_id: &str) -> bool {
        self.revision_id() == revision_id
    }

    /// The names of the approval rules applied to this pull request, split into
    /// the ones whose conditions its approvals MEET and the ones they do not, each
    /// in the order the rules were applied.
    ///
    /// Each rule is read the one way this crate reads an approval rule document
    /// ([`approval_rule_satisfied`]), and the split is made in this ONE place, so
    /// the rules an evaluation of this pull request reports as met are exactly the
    /// ones it does not report as unmet, and exactly the ones a merge is not
    /// refused for.
    pub fn approval_rules_by_condition(&self) -> (Vec<String>, Vec<String>) {
        let approvers = self.approvers();
        let mut satisfied = Vec::new();
        let mut not_satisfied = Vec::new();
        for rule in &self.approval_rules {
            if approval_rule_satisfied(&rule.content, &approvers) {
                satisfied.push(rule.approval_rule_name.clone());
            } else {
                not_satisfied.push(rule.approval_rule_name.clone());
            }
        }
        (satisfied, not_satisfied)
    }

    /// The names of the approval rules applied to this pull request whose
    /// conditions its approvals do not meet, in the order the rules were applied.
    ///
    /// Each rule is read the one way this crate reads an approval rule document
    /// ([`approval_rule_satisfied`]), so the rules a merge is refused for are the
    /// same ones an evaluation of this pull request reports.
    pub fn approval_rules_not_satisfied(&self) -> Vec<String> {
        self.approval_rules_by_condition().1
    }

    /// Does every approval rule applied to this pull request state conditions its
    /// approvals meet — or have its rules been overridden?
    ///
    /// The question a merge asks before it writes anything: "the pull request
    /// cannot be merged because one or more approval rules applied to the pull
    /// request have conditions that have not been met", and an override is what
    /// sets those conditions aside.
    pub fn approval_rules_satisfied(&self) -> bool {
        self.approval_rules_overridden() || self.approval_rules_not_satisfied().is_empty()
    }

    /// Does this pull request already record a merge that was MADE?
    ///
    /// A merge is what closes a pull request ("if the merge is successful, it closes
    /// the pull request"), so a closed pull request whose target carries the merge was
    /// closed BY it: there is no second attempt to make, and the stored merge — the
    /// commit it left, who made it and with which strategy — is the answer. Every
    /// merge handler asks here before it refuses a closed pull request or writes
    /// anything, so a repeated merge of one pull request cannot be answered one way by
    /// one strategy and another way by the next.
    pub fn is_merged(&self) -> bool {
        self.targets.iter().any(|target| target.merged)
    }

    /// Are the approval rules of this pull request currently set aside?
    ///
    /// Asked by every reader of the override and by a merge of the pull request, so
    /// the one stored fact ([`Self::approval_rules_overridden_by`]) answers them all.
    pub fn approval_rules_overridden(&self) -> bool {
        self.approval_rules_overridden_by.is_some()
    }

    /// The system-generated `revisionId` of this pull request's CURRENT
    /// revision: a digest over the pull request's identity, how many times its
    /// reviewable state has moved, and the tips each target is between.
    ///
    /// Derived rather than stored, so it cannot disagree with the state it
    /// names: it is stable for as long as that state is, and a change to it (a
    /// source branch moving, a revision of the pull request) gives the next
    /// read a different one. The digest form is the hex SHA-256 this crate
    /// already reports for content signatures ([`content_sha256`]).
    pub fn revision_id(&self) -> String {
        let tips: Vec<String> = self
            .targets
            .iter()
            .map(|target| {
                format!(
                    "{}/{}/{}/{}/{}",
                    target.repository_name,
                    target.source_reference,
                    target.source_commit,
                    target.destination_reference,
                    target.destination_commit
                )
            })
            .collect();
        content_sha256(&format!(
            "codecommit/{}/pull-request/{}/{}/{}",
            ::engine::emulator_account_id(),
            self.pull_request_id,
            self.revision,
            tips.join(",")
        ))
    }

    /// Append one entry to this pull request's event log: the change `detail`
    /// describes, made by `actor_arn` at `at`.
    ///
    /// The ONE place an event is appended, so every operation that changes a
    /// pull request records it the same way and `DescribePullRequestEvents`
    /// cannot report a change no operation made. The stored instant is never
    /// earlier than the previous entry's, so the log stays in the order the
    /// changes happened whatever the clock does between two calls, and
    /// `lastActivityDate` moves onto it because an entry in this log IS "user or
    /// system activity on the pull request".
    pub fn record_event(
        &mut self,
        at: SystemTime,
        actor_arn: Option<String>,
        detail: PullRequestEventDetail,
    ) {
        let event_date = match self.events.last() {
            Some(previous) => at.max(previous.event_date),
            None => at,
        };
        self.events.push(PullRequestEventRecord {
            event_date,
            actor_arn,
            detail,
        });
        // The emulator's own retention: the oldest entries past the bound are
        // released, exactly as the oldest commits are by `prune_history`.
        let held = self.events.len();
        if held > MAX_PULL_REQUEST_EVENTS_RETAINED {
            self.events.drain(..held - MAX_PULL_REQUEST_EVENTS_RETAINED);
        }
        self.last_activity_date = event_date;
    }

    /// This pull request as the modeled `PullRequest` shape — the one place that
    /// projection is written, so `CreatePullRequest`, `GetPullRequest` and any
    /// later updater cannot disagree about a stored pull request.
    pub fn to_modeled(&self) -> PullRequest {
        PullRequest {
            pull_request_id: Some(self.pull_request_id.to_string()),
            title: Some(self.title.clone()),
            description: self.description.clone(),
            last_activity_date: Some(to_date_time(self.last_activity_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            pull_request_status: Some(self.status.clone()),
            author_arn: Some(self.author_arn.clone()),
            pull_request_targets: Some(
                self.targets
                    .iter()
                    .map(PullRequestTargetRecord::to_modeled)
                    .collect(),
            ),
            client_request_token: self.client_request_token.clone(),
            revision_id: Some(self.revision_id()),
            approval_rules: Some(
                self.approval_rules
                    .iter()
                    .map(ApprovalRuleRecord::to_modeled)
                    .collect(),
            ),
        }
    }
}

/// One target of a pull request: the two references it is between, the tips they
/// pointed at, their merge base, and how the merge stands.
#[derive(Debug, Clone)]
pub struct PullRequestTargetRecord {
    pub repository_name: String,
    /// The FULLY QUALIFIED reference (`refs/heads/<branch>`), which is the form
    /// CodeCommit reports whichever form the request named — see
    /// [`pull_request_reference`].
    pub source_reference: String,
    pub destination_reference: String,
    /// The tip of the source branch when the pull request was created. "If the
    /// pull request branch is updated by a push while the pull request is open,
    /// the commit ID changes to reflect the new tip of the branch."
    pub source_commit: String,
    pub destination_commit: String,
    /// "The commit ID of the most recent commit that the source branch and the
    /// destination branch have in common" — absent for two histories that share
    /// no commit at all.
    pub merge_base: Option<String>,
    pub merged: bool,
    pub merged_by: Option<String>,
    pub merge_commit_id: Option<String>,
    pub merge_option: Option<String>,
}

impl PullRequestTargetRecord {
    /// This target as the modeled `PullRequestTarget` shape.
    pub fn to_modeled(&self) -> PullRequestTarget {
        PullRequestTarget {
            repository_name: Some(self.repository_name.clone()),
            source_reference: Some(self.source_reference.clone()),
            destination_reference: Some(self.destination_reference.clone()),
            destination_commit: Some(self.destination_commit.clone()),
            source_commit: Some(self.source_commit.clone()),
            merge_base: self.merge_base.clone(),
            // "Returns metadata about the state of the merge, including whether
            // the merge has been made": a target that has not been merged
            // reports that, rather than reporting nothing.
            merge_metadata: Some(MergeMetadata {
                is_merged: self.merged,
                merged_by: self.merged_by.clone(),
                merge_commit_id: self.merge_commit_id.clone(),
                merge_option: self.merge_option.clone(),
            }),
        }
    }
}

/// One approval rule applied to a pull request.
#[derive(Debug, Clone)]
pub struct ApprovalRuleRecord {
    pub approval_rule_id: String,
    pub approval_rule_name: String,
    /// The approval rule document, canonicalized exactly as a template's
    /// content is ([`canonical_json_document`]), so `ruleContentSha256` is the
    /// digest of the bytes every reader reports.
    pub content: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub last_modified_user: String,
    /// The approval rule template this rule was derived from, for a rule a
    /// template applied; absent for one created on the pull request itself.
    pub origin_template: Option<(String, String)>,
}

impl ApprovalRuleRecord {
    /// This rule as the modeled `ApprovalRule` shape.
    pub fn to_modeled(&self) -> ApprovalRule {
        ApprovalRule {
            approval_rule_id: Some(self.approval_rule_id.clone()),
            approval_rule_name: Some(self.approval_rule_name.clone()),
            approval_rule_content: Some(self.content.clone()),
            rule_content_sha256: Some(content_sha256(&self.content)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_user: Some(self.last_modified_user.clone()),
            origin_approval_rule_template: self.origin_template.as_ref().map(|(id, name)| {
                OriginApprovalRuleTemplate {
                    approval_rule_template_id: Some(id.clone()),
                    approval_rule_template_name: Some(name.clone()),
                }
            }),
        }
    }
}

/// One entry in a pull request's event log: WHEN the pull request changed, WHO
/// changed it, and WHAT the change was.
///
/// The log lives on the pull request ([`PullRequestRecord::events`]) and is
/// written by the operations that change it, so `DescribePullRequestEvents`
/// reports what happened to the stored pull request rather than a history
/// reconstructed on the request path. The event's TYPE is derived from the
/// change it carries ([`PullRequestEventDetail::event_type`]), so an entry
/// cannot report one kind of change and carry another's metadata.
#[derive(Debug, Clone)]
pub struct PullRequestEventRecord {
    /// "The day and time of the pull request event, in timestamp format."
    pub event_date: SystemTime,
    /// "The Amazon Resource Name (ARN) of the user whose actions resulted in the
    /// event" — the calling identity ([`caller_identity_arn`]) for a change made
    /// through a modeled operation, and NONE for one whose transport carries no
    /// caller identity to attribute it to (a git push).
    pub actor_arn: Option<String>,
    pub detail: PullRequestEventDetail,
}

/// What changed about the pull request, one variant per `PullRequestEventType`
/// the model declares, each carrying exactly the metadata that type reports.
#[derive(Debug, Clone)]
pub enum PullRequestEventDetail {
    /// The pull request was opened: "information about the source and
    /// destination branches for the pull request".
    Created {
        repository_name: String,
        source_commit_id: String,
        destination_commit_id: String,
        merge_base: Option<String>,
    },
    /// The pull request's `pullRequestStatus` moved, to the status carried here.
    StatusChanged { status: String },
    /// The source branch moved while the pull request was open, from
    /// `before_commit_id` to `after_commit_id`.
    SourceReferenceUpdated {
        repository_name: String,
        before_commit_id: String,
        after_commit_id: String,
        merge_base: Option<String>,
    },
    /// The merge state of a target changed — what it was merged by and into.
    MergeStateChanged {
        repository_name: String,
        destination_reference: String,
        merged: bool,
        merged_by: Option<String>,
        merge_commit_id: Option<String>,
        merge_option: Option<String>,
    },
    /// An approval rule was created on, updated on, or deleted from the pull
    /// request. One payload for the three, because the rule an approval-rule
    /// event names is reported the same way whichever of them happened.
    ApprovalRuleCreated(ApprovalRuleEvent),
    ApprovalRuleUpdated(ApprovalRuleEvent),
    ApprovalRuleDeleted(ApprovalRuleEvent),
    /// The approval rules of the pull request were overridden (or the override
    /// was revoked), at the revision carried here.
    ApprovalRuleOverridden {
        revision_id: String,
        override_status: String,
    },
    /// A reviewer's approval state changed, at the revision carried here.
    ApprovalStateChanged {
        revision_id: String,
        approval_status: String,
    },
}

/// The approval rule an approval-rule event names, as that event reports it.
#[derive(Debug, Clone)]
pub struct ApprovalRuleEvent {
    pub approval_rule_id: String,
    pub approval_rule_name: String,
    pub approval_rule_content: String,
}

impl PullRequestEventDetail {
    /// The `pullRequestEventType` this change is reported under: the model's own
    /// value for the kind of change it is.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => PULL_REQUEST_EVENT_CREATED,
            Self::StatusChanged { .. } => PULL_REQUEST_EVENT_STATUS_CHANGED,
            Self::SourceReferenceUpdated { .. } => PULL_REQUEST_EVENT_SOURCE_REFERENCE_UPDATED,
            Self::MergeStateChanged { .. } => PULL_REQUEST_EVENT_MERGE_STATE_CHANGED,
            Self::ApprovalRuleCreated(_) => PULL_REQUEST_EVENT_APPROVAL_RULE_CREATED,
            Self::ApprovalRuleUpdated(_) => PULL_REQUEST_EVENT_APPROVAL_RULE_UPDATED,
            Self::ApprovalRuleDeleted(_) => PULL_REQUEST_EVENT_APPROVAL_RULE_DELETED,
            Self::ApprovalRuleOverridden { .. } => PULL_REQUEST_EVENT_APPROVAL_RULE_OVERRIDDEN,
            Self::ApprovalStateChanged { .. } => PULL_REQUEST_EVENT_APPROVAL_STATE_CHANGED,
        }
    }
}

impl PullRequestEventRecord {
    /// The `pullRequestEventType` this entry is reported under.
    pub fn event_type(&self) -> &'static str {
        self.detail.event_type()
    }

    /// This entry as the modeled `PullRequestEvent` shape, for the pull request
    /// whose log holds it — the one place that projection is written, so an
    /// event carries the metadata member of its own type and no other.
    pub fn to_modeled(&self, pull_request_id: u64) -> PullRequestEvent {
        let mut event = PullRequestEvent {
            pull_request_id: Some(pull_request_id.to_string()),
            event_date: Some(to_date_time(self.event_date)),
            pull_request_event_type: Some(self.event_type().to_string()),
            actor_arn: self.actor_arn.clone(),
            ..PullRequestEvent::default()
        };
        match &self.detail {
            PullRequestEventDetail::Created {
                repository_name,
                source_commit_id,
                destination_commit_id,
                merge_base,
            } => {
                event.pull_request_created_event_metadata = Some(PullRequestCreatedEventMetadata {
                    repository_name: Some(repository_name.clone()),
                    source_commit_id: Some(source_commit_id.clone()),
                    destination_commit_id: Some(destination_commit_id.clone()),
                    merge_base: merge_base.clone(),
                });
            }
            PullRequestEventDetail::StatusChanged { status } => {
                event.pull_request_status_changed_event_metadata =
                    Some(PullRequestStatusChangedEventMetadata {
                        pull_request_status: Some(status.clone()),
                    });
            }
            PullRequestEventDetail::SourceReferenceUpdated {
                repository_name,
                before_commit_id,
                after_commit_id,
                merge_base,
            } => {
                event.pull_request_source_reference_updated_event_metadata =
                    Some(PullRequestSourceReferenceUpdatedEventMetadata {
                        repository_name: Some(repository_name.clone()),
                        before_commit_id: Some(before_commit_id.clone()),
                        after_commit_id: Some(after_commit_id.clone()),
                        merge_base: merge_base.clone(),
                    });
            }
            PullRequestEventDetail::MergeStateChanged {
                repository_name,
                destination_reference,
                merged,
                merged_by,
                merge_commit_id,
                merge_option,
            } => {
                event.pull_request_merged_state_changed_event_metadata =
                    Some(PullRequestMergedStateChangedEventMetadata {
                        repository_name: Some(repository_name.clone()),
                        destination_reference: Some(destination_reference.clone()),
                        merge_metadata: Some(MergeMetadata {
                            is_merged: *merged,
                            merged_by: merged_by.clone(),
                            merge_commit_id: merge_commit_id.clone(),
                            merge_option: merge_option.clone(),
                        }),
                    });
            }
            PullRequestEventDetail::ApprovalRuleCreated(rule)
            | PullRequestEventDetail::ApprovalRuleUpdated(rule)
            | PullRequestEventDetail::ApprovalRuleDeleted(rule) => {
                event.approval_rule_event_metadata = Some(ApprovalRuleEventMetadata {
                    approval_rule_name: Some(rule.approval_rule_name.clone()),
                    approval_rule_id: Some(rule.approval_rule_id.clone()),
                    approval_rule_content: Some(rule.approval_rule_content.clone()),
                });
            }
            PullRequestEventDetail::ApprovalRuleOverridden {
                revision_id,
                override_status,
            } => {
                event.approval_rule_overridden_event_metadata =
                    Some(ApprovalRuleOverriddenEventMetadata {
                        revision_id: Some(revision_id.clone()),
                        override_status: Some(override_status.clone()),
                    });
            }
            PullRequestEventDetail::ApprovalStateChanged {
                revision_id,
                approval_status,
            } => {
                event.approval_state_changed_event_metadata =
                    Some(ApprovalStateChangedEventMetadata {
                        revision_id: Some(revision_id.clone()),
                        approval_status: Some(approval_status.clone()),
                    });
            }
        }
        event
    }
}

/// The `lastModifiedUser` CodeCommit attributes an approval rule it applied
/// ITSELF to: the service principal, because a rule that an associated approval
/// rule template put on a new pull request is written by the service rather than
/// by the caller who opened the pull request (the recorded behaviour the corpus
/// names "approval-rule-template-auto-attaches-to-new-pull-requests", whose
/// recorded `lastModifiedUser` for such a rule is this principal and not the
/// creating caller's ARN). A rule a caller creates on a pull request names that
/// caller instead.
pub const SERVICE_PRINCIPAL: &str = "codecommit.amazonaws.com";

/// An approval rule's opaque `approvalRuleId`: the same kind of version-8 UUID
/// as every other id this service mints, over the account, the pull request the
/// rule is on, the rule's name and the instant it was applied.
pub fn approval_rule_id(pull_request_id: u64, name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/pull-request/{pull_request_id}/approval-rule/{name}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// How many approval rules this emulator holds on one pull request.
///
/// `CreatePullRequestApprovalRule` models `NumberOfRulesExceededException` ("The
/// approval rule cannot be added. The pull request has the maximum number of
/// approval rules associated with it"), so a bound has to exist; this is the one
/// this emulator holds, and it is the number the CodeCommit quota for approval
/// rules on a single pull request publishes.
pub const MAX_APPROVAL_RULES_PER_PULL_REQUEST: usize = 30;

/// `ApprovalRuleNameRequiredException`'s message: a FIXED sentence, worded here
/// once for every operation that takes an `approvalRuleName`.
pub const APPROVAL_RULE_NAME_REQUIRED_MESSAGE: &str =
    "An approval rule name is required, but was not specified.";

/// `ApprovalRuleNameAlreadyExistsException`'s message, built from the name the
/// request asked for: an approval rule name is unique within one pull request.
pub fn approval_rule_name_already_exists_message(name: &str) -> String {
    format!(
        "An approval rule with the name {name} already exists for this pull request. Approval \
         rule names must be unique within a pull request."
    )
}

/// `NumberOfRulesExceededException`'s message, stating the bound this emulator
/// holds ([`MAX_APPROVAL_RULES_PER_PULL_REQUEST`]).
pub fn number_of_rules_exceeded_message() -> String {
    format!(
        "The approval rule cannot be added. The pull request has the maximum number of approval \
         rules associated with it. The maximum number of approval rules for a pull request is {}.",
        MAX_APPROVAL_RULES_PER_PULL_REQUEST
    )
}

/// `CannotDeleteApprovalRuleFromTemplateException`'s message: a FIXED sentence,
/// naming nothing from the request — the wording CodeCommit answers that code with
/// (the model's documentation for the shape states the same sentence with its last
/// two phrases in the other order).
pub const CANNOT_DELETE_APPROVAL_RULE_FROM_TEMPLATE_MESSAGE: &str =
    "The approval rule cannot be deleted from the pull request because it was created by an \
     approval rule template and automatically applied to the pull request.";

/// `PullRequestAlreadyClosedException`'s message: the sentence the model documents
/// for the shape, worded here once for every operation that refuses to change a
/// pull request that is no longer open and has no sentence of its OWN.
///
/// CodeCommit words this code per the change the caller attempted, so an operation
/// whose refusal the service words differently carries its own message instead of
/// this one — see [`pull_request_text_cannot_be_changed_message`] for the
/// title/description path and
/// [`PULL_REQUEST_APPROVAL_OVERRIDE_CANNOT_BE_CHANGED_MESSAGE`] for the
/// approval-rule override path.
pub const PULL_REQUEST_ALREADY_CLOSED_MESSAGE: &str =
    "The pull request status cannot be updated because it is already closed.";

/// `PullRequestAlreadyClosedException`'s message for an attempt to MERGE a pull
/// request that is no longer open.
///
/// CodeCommit words that code differently there too: it states the state the pull
/// request is in and then that such a pull request is not merged — a caller
/// attempting a merge is not told a status update failed. FIXED text naming nothing
/// from the request, worded here once for the three operations that merge a pull
/// request.
pub const PULL_REQUEST_CLOSED_CANNOT_BE_MERGED_MESSAGE: &str =
    "The specified pull request is marked as Closed. A closed pull request cannot be merged.";

/// `PullRequestAlreadyClosedException`'s message for an update of a pull
/// request's TEXT — its title or its description.
///
/// CodeCommit words that code differently there: it names the state the pull
/// request is IN (a [`PULL_REQUEST_STATUS_OPEN`] / [`PULL_REQUEST_STATUS_CLOSED`]
/// value, read from the stored record rather than from the request) as the reason
/// the change cannot be made, and then which changes a closed pull request no
/// longer takes. Worded here once for both operations that replace a pull
/// request's text.
pub fn pull_request_text_cannot_be_changed_message(status: &str) -> String {
    format!(
        "The change cannot be made because the state of the pull request is {status}. You cannot \
         update the title or description of a pull request after it has been closed."
    )
}

/// `PullRequestAlreadyClosedException`'s message for a request to SET ASIDE a pull
/// request's approval rule requirements, or to hold them again.
///
/// CodeCommit words that code differently there too: it states that the pull
/// request is already merged or closed, that the approval status of such a pull
/// request is not overridden, and that the request is to be modified. FIXED text
/// naming nothing from the request or from the stored record — neither the pull
/// request, nor the revision, nor either status — so the whole sentence set is the
/// message.
pub const PULL_REQUEST_APPROVAL_OVERRIDE_CANNOT_BE_CHANGED_MESSAGE: &str =
    "This pull request is already merged or closed. You cannot override the approval status of a \
     merged or closed pull request. Modify your request, and then try again.";

/// `SourceAndDestinationAreSameException`'s message: the FIXED sentence pair the
/// service answers that code with, covering BOTH conditions it is raised for —
/// the two references naming the same branch, and two differently named branches
/// whose tips are the same commit — and carrying nothing from the request.
///
/// It is not the error shape's documented sentence, which speaks only of the two
/// branches being the same and so asserts something untrue of a request whose
/// branches differ in name but not in tip. Worded here once for every operation
/// that compares a source against a destination.
pub const SOURCE_AND_DESTINATION_ARE_SAME_MESSAGE: &str =
    "The source branch name and the destination branch name are either the same, or the tips of \
     both branches are identical. Make sure that your source branch and destination branch have \
     unique names and that the tips of the branches are different.";

/// `TipOfSourceReferenceIsDifferentException`'s message: the sentences the model
/// documents for the shape, echoing nothing from the request. Worded here once
/// for every operation that merges a pull request whose source reference no
/// longer holds the tip the request was made against.
pub const TIP_OF_SOURCE_REFERENCE_IS_DIFFERENT_MESSAGE: &str =
    "The tip of the source branch in the destination repository does not match the tip of the \
     source branch specified in your request. The pull request might have been updated. Make sure \
     that you have the latest changes.";

/// `ManualMergeRequiredException`'s message: the FIXED sentence CodeCommit answers
/// that code with, carrying nothing from the request — no repository, no branch, no
/// commit id and no conflicting path.
///
/// It is not the error shape's documented sentence, which is prose about the error
/// class and speaks of a pull request even where the merge was between two branches.
/// The service answers this one sentence on every path that raises the code — a
/// merge whose conflicts no strategy and no resolution settled, and a fast-forward
/// between two tips neither of which contains the other — so it is worded here once
/// for all of them.
pub const MANUAL_MERGE_REQUIRED_MESSAGE: &str =
    "Current branches cannot be merged, please try to merge using a Git client locally.";

/// `ConcurrentReferenceUpdateException`'s message: the sentences the model
/// documents for the shape, echoing nothing from the request — what a merge
/// answers when the branch it would be applied to is no longer the reference the
/// merge was evaluated for.
pub const CONCURRENT_REFERENCE_UPDATE_MESSAGE: &str =
    "The merge cannot be completed because the target branch has been modified. Another user might \
     have modified the target branch while the merge was in progress. Wait a few minutes, and then \
     try again.";

/// `PullRequestApprovalRulesNotSatisfiedException`'s message: the FIXED sentence
/// pair CodeCommit answers that code with — the failure, then where the rules that
/// still have requirements are reported. Echoes nothing from the request, and is
/// worded here once for every operation that refuses to merge a pull request whose
/// approval rules still state requirements its approvals do not meet. (The model's
/// documentation for the shape states the same refusal in other words; the service's
/// own sentences are these.)
pub const PULL_REQUEST_APPROVAL_RULES_NOT_SATISFIED_MESSAGE: &str =
    "You cannot merge the pull request because the requirements for one or more approval rules \
     have not been met. For details about the approval rules that still have requirements, use \
     EvaluatePullRequestApprovalRules.";

/// `approvalState` values the model's `ApprovalState` declares
/// (`smithy_query(query="shape",shape="ApprovalState")`): the two states one
/// reviewer's approval of a pull request can hold.
pub const APPROVAL_STATE_APPROVE: &str = "APPROVE";
pub const APPROVAL_STATE_REVOKE: &str = "REVOKE";

/// How many reviewers' approval states one pull request holds before
/// `UpdatePullRequestApprovalState` refuses to record another.
///
/// The emulator's OWN bound, not an AWS limit: an approval state is keyed by the
/// identity that recorded it and a caller signs as whatever identity it likes, so
/// this is what keeps one pull request's stored size bounded. A reviewer the pull
/// request already holds a state for changes it whatever the bound, and every
/// record it holds is released by `DeleteRepository`.
pub const MAX_APPROVALS_PER_PULL_REQUEST: usize = 100;

/// `MaximumNumberOfApprovalsExceededException`'s message: the sentence the model
/// documents for the shape, then the bound this emulator enforces — worded the way
/// [`number_of_rules_exceeded_message`] states the bound on approval rules.
pub fn maximum_number_of_approvals_exceeded_message() -> String {
    format!(
        "The number of approvals required for the approval rule exceeds the maximum number \
         allowed. The maximum number of approvals for a pull request is {MAX_APPROVALS_PER_PULL_REQUEST}."
    )
}

/// `overrideStatus` values the model's `OverrideStatus` declares
/// (`smithy_query(query="shape",shape="OverrideStatus")`): whether the approval
/// rules of a pull request are set aside or held again.
pub const OVERRIDE_STATUS_OVERRIDE: &str = "OVERRIDE";
pub const OVERRIDE_STATUS_REVOKE: &str = "REVOKE";

/// `OverrideStatusRequiredException`'s message: the sentence pair the model
/// documents for the shape — the requirement, then the values that are accepted —
/// worded here once for every operation that takes an `overrideStatus`.
pub const OVERRIDE_STATUS_REQUIRED_MESSAGE: &str = "An override status is required, but no value \
                                                    was provided. Valid values include OVERRIDE \
                                                    and REVOKE.";

/// `InvalidOverrideStatusException`'s message: the sentence CodeCommit answers that
/// class with — the failure, then the values that are accepted — echoing nothing
/// from the request.
pub const INVALID_OVERRIDE_STATUS_MESSAGE: &str =
    "The provided override status value is not valid. Valid values are: [OVERRIDE, REVOKE].";

/// `OverrideAlreadySetException`'s message: a FIXED sentence, the one the model
/// documents for the shape, echoing nothing from the request.
pub const OVERRIDE_ALREADY_SET_MESSAGE: &str =
    "The pull request has already had its approval rules set to override.";

/// `ApprovalStateRequiredException`'s message: a FIXED sentence, the one the model
/// documents for the shape, worded here once for every operation that takes an
/// `approvalState`.
pub const APPROVAL_STATE_REQUIRED_MESSAGE: &str =
    "An approval state is required, but was not specified.";

/// `InvalidApprovalStateException`'s message: the sentence CodeCommit answers that
/// class with — the requirement, then the values that are accepted — echoing nothing
/// from the request.
pub const INVALID_APPROVAL_STATE_MESSAGE: &str =
    "An approval state is required, but the supplied value is not valid. Valid values for \
     approval states are: [APPROVE, REVOKE].";

/// `PullRequestCannotBeApprovedByAuthorException`'s message: the sentence pair the
/// model documents for the shape, echoing nothing from the request.
pub const PULL_REQUEST_CANNOT_BE_APPROVED_BY_AUTHOR_MESSAGE: &str =
    "The approval cannot be applied because the user approving the pull request matches the user \
     who created the pull request. You cannot approve a pull request that you created.";

/// `RevisionIdRequiredException`'s message: a FIXED sentence, for a request that
/// carried no `revisionId` member at all — the case the request boundary
/// (`crate::service`) refuses before any handler reads it. A member that IS carried
/// but names no revision is `InvalidRevisionIdException`
/// ([`invalid_revision_id_message`]).
pub const REVISION_ID_REQUIRED_MESSAGE: &str = "A revision ID is required, but was not provided.";

/// `InvalidRevisionIdException`'s message for a value that is not the FORM a
/// revision id takes: the requirement, then the length a revision id holds — the 64
/// hex characters `REVISION_ID_CHARACTERS` enforces — and then where the current one
/// is read from. It names no value, which a revision id's wording leaves out.
pub const INVALID_REVISION_ID_MESSAGE: &str =
    "A revision ID is required, but the value is not valid. Revision IDs are 64 characters in \
     length. To retrieve a valid revision ID, use GetPullRequest.";

/// `RevisionNotCurrentException`'s message: the sentence pair the model documents
/// for the shape, echoing nothing from the request. Worded here once for every
/// operation that refuses a request made against a revision the pull request has
/// moved past.
pub const REVISION_NOT_CURRENT_MESSAGE: &str =
    "The revision ID provided in the request does not match the current revision ID. Use \
     GetPullRequest to retrieve the current revision ID.";

/// `InvalidRevisionIdException`'s message for a `revisionId` that was CARRIED but
/// holds no characters at all: a value that is empty names no revision, so it is
/// refused as a revision id that is not valid, with the sentence CodeCommit words
/// for an empty or null one.
pub const EMPTY_REVISION_ID_MESSAGE: &str =
    "A revision ID is required, but the value is either empty or null.";

/// Why a `revisionId` cannot name a revision of a pull request. Both outcomes are
/// `InvalidRevisionIdException` — a carried value that addresses no revision is not
/// valid, whether it is empty or of the wrong form — and they differ only in the
/// sentence that error carries ([`invalid_revision_id_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionIdProblem {
    /// The id was carried but holds no characters.
    Empty,
    /// The id is not the form a revision id takes.
    Invalid,
}

/// The sentence `InvalidRevisionIdException` carries for a value naming no
/// revision: the empty case states that the value is empty or null, every other
/// case states that it is not valid and where the current one is read from.
pub fn invalid_revision_id_message(problem: RevisionIdProblem) -> &'static str {
    match problem {
        RevisionIdProblem::Empty => EMPTY_REVISION_ID_MESSAGE,
        RevisionIdProblem::Invalid => INVALID_REVISION_ID_MESSAGE,
    }
}

/// The revision a `revisionId` names, in the form a stored revision id is reported
/// in, or why it names none.
///
/// A revision id is the digest this crate mints for a pull request's current
/// revision ([`PullRequestRecord::revision_id`]), so a value that is not a digest of
/// that form names no revision of any pull request in any account and is refused as
/// invalid rather than reported as not current. Hex is case insensitive, so an id
/// handed back in either case names the same revision. A value that holds no
/// characters names none either, and is reported apart only so the refusal can say
/// so ([`invalid_revision_id_message`]). Every operation that takes a `revisionId`
/// classifies it here, so they cannot disagree about which ids are addressable.
pub fn normalize_revision_id(raw: &str) -> Result<String, RevisionIdProblem> {
    if raw.is_empty() {
        return Err(RevisionIdProblem::Empty);
    }
    let trimmed = raw.trim();
    if trimmed.len() != REVISION_ID_CHARACTERS
        || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RevisionIdProblem::Invalid);
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// How many characters a revision id holds: the hex SHA-256
/// [`PullRequestRecord::revision_id`] mints.
const REVISION_ID_CHARACTERS: usize = 64;

/// Is this approval rule document's set of conditions met by `approvers`?
///
/// An approval rule document carries one or more `Approvers` statements (the shape
/// [`is_approval_rule_template_content`] accepts), each stating how many approvals
/// it needs and, where present, the pool of identities whose approvals count
/// towards it: a statement is met once at least that many of the pull request's
/// approvers are in its pool, a statement that names no pool counts every
/// approver, and the rule is met only when every one of its statements is.
///
/// A document this service cannot read states conditions that cannot be counted at
/// all, so it is reported UNMET rather than silently met.
fn approval_rule_satisfied(content: &str, approvers: &BTreeSet<&str>) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(content) else {
        return false;
    };
    let Some(statements) = document
        .get("Statements")
        .and_then(::serde_json::Value::as_array)
    else {
        return false;
    };
    if statements.is_empty() {
        return false;
    }
    statements.iter().all(|statement| {
        let Some(needed) = statement
            .get("NumberOfApprovalsNeeded")
            .and_then(::serde_json::Value::as_i64)
        else {
            return false;
        };
        let pool = statement
            .get("ApprovalPoolMembers")
            .and_then(::serde_json::Value::as_array);
        let counted = approvers
            .iter()
            .filter(|approver| match pool {
                Some(pool) => pool
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .any(|member| approval_pool_includes(member, approver)),
                None => true,
            })
            .count();
        i64::try_from(counted).unwrap_or(i64::MAX) >= needed
    })
}

/// Does one `ApprovalPoolMembers` entry name this identity?
///
/// An entry is an identity the approvals of which count towards the statement,
/// written as that identity's ARN, in which `*` stands for any sequence of
/// characters — the wildcard CodeCommit's approval rule template schema allows in
/// a pool member, so one entry can name every session of an assumed role.
///
/// Every slice taken below is at a boundary the check above it established — a
/// prefix stripped, a suffix confirmed, an occurrence found — so no caller-supplied
/// entry can index outside the identity it is matched against.
fn approval_pool_includes(member: &str, identity: &str) -> bool {
    let segments: Vec<&str> = member.split('*').collect();
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return false;
    };
    if segments.len() == 1 {
        // No wildcard at all: the entry names exactly one identity.
        return member == identity;
    }
    // The text before the FIRST wildcard is anchored at the start of the identity,
    // and the text after the LAST one at its end.
    let Some(rest) = identity.strip_prefix(*first) else {
        return false;
    };
    if !rest.ends_with(*last) {
        return false;
    }
    let mut rest = &rest[..rest.len() - last.len()];
    // Every segment BETWEEN two wildcards matches the next occurrence of it,
    // leaving the rest of the identity for the segments after it.
    for segment in &segments[1..segments.len() - 1] {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    true
}

/// Does an approval rule template's content apply to a pull request whose
/// destination is this fully qualified reference?
///
/// A template document may carry `DestinationReferences` (the member
/// [`is_approval_rule_template_content`] accepts): CodeCommit's approval rule
/// template schema uses it to scope the template to pull requests merging into
/// those branches, so a template that lists any applies only to a destination it
/// lists, and one that lists none applies to every pull request in the
/// repositories it is associated with. A listed entry matches either spelling of
/// the reference, since the two name the same branch.
pub fn template_applies_to_destination(content: &str, destination_reference: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(content) else {
        return false;
    };
    let Some(references) = document
        .get("DestinationReferences")
        .and_then(::serde_json::Value::as_array)
    else {
        return true;
    };
    if references.is_empty() {
        return true;
    }
    let branch = destination_reference
        .strip_prefix(HEADS_PREFIX)
        .unwrap_or(destination_reference);
    references.iter().filter_map(::serde_json::Value::as_str).any(|listed| {
        listed == destination_reference
            || listed.strip_prefix(HEADS_PREFIX).unwrap_or(listed) == branch
    })
}

/// Why a `pullRequestId` cannot name a pull request. Both outcomes are
/// `InvalidPullRequestIdException` — a carried value that addresses no pull request
/// is not valid, whether it is empty or of the wrong form — and they differ only in
/// the sentence that error carries ([`invalid_pull_request_id_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestIdProblem {
    /// The id was carried but holds no characters.
    Empty,
    /// The id is not the form a pull request id takes.
    Invalid,
}

/// The pull request a `pullRequestId` names, or why it names none.
///
/// CodeCommit ids a pull request with a decimal integer, so a value that is not
/// one addresses no pull request in any account: a sign, a fractional part or
/// surrounding space all make it a value no id was ever minted as. A value that
/// holds no characters names none either, and is reported apart only so the
/// refusal can say so ([`invalid_pull_request_id_message`]). Every operation that
/// takes a `pullRequestId` classifies it here, so they cannot disagree about which
/// ids are addressable.
pub fn normalize_pull_request_id(raw: &str) -> Result<u64, PullRequestIdProblem> {
    if raw.is_empty() {
        return Err(PullRequestIdProblem::Empty);
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PullRequestIdProblem::Invalid);
    }
    raw.parse::<u64>()
        .map_err(|_| PullRequestIdProblem::Invalid)
}

/// `PullRequestDoesNotExistException`'s message: a FIXED sentence pair that states
/// the failure and then where a caller reads the ids that do exist. It names NO
/// value — not the id the request asked for, and not the repository — so a caller
/// learns which ids are addressable only by listing them.
///
/// The one place this crate words that code, for the same reason
/// [`repository_does_not_exist_message`] is the one place it words its own.
pub const PULL_REQUEST_DOES_NOT_EXIST_MESSAGE: &str =
    "The pull request ID does not exist. Verify that you provided a valid pull request ID, and \
     then try your operation again. To get a list of pull request IDs, use ListPullRequests.";

/// `InvalidPullRequestIdException`'s message, worded around the value the request
/// carried: the refusal names the offending id — the one piece of the failure a
/// caller cannot already see — and then where the valid ones are listed. A value
/// holding no characters is named the same way, with the clause that states it was
/// empty or null.
///
/// The one place this crate words that code, for the same reason
/// [`pull_request_does_not_exist_message`] is the one place it words its own: every
/// operation taking a `pullRequestId` refuses a value addressing no pull request
/// identically.
pub fn invalid_pull_request_id_message(raw: &str, problem: PullRequestIdProblem) -> String {
    let fault = match problem {
        PullRequestIdProblem::Empty => "is empty or null and therefore not valid",
        PullRequestIdProblem::Invalid => "is not valid",
    };
    format!(
        "The following pull request ID {fault}: {raw}. To view a list of valid pull request IDs \
         for this repository, use ListPullRequests."
    )
}

/// `InvalidPullRequestIdException`'s message where the request carried no
/// `pullRequestId` VALUE to name — a member that is not a string at all, refused at
/// the request boundary before any handler reads it: a FIXED sentence pair that
/// states the failure and then the rule.
pub const INVALID_PULL_REQUEST_ID_MESSAGE: &str =
    "The pull request ID is not valid. Make sure that you have provided the full ID and that the \
     pull request is in the specified repository, and then try again.";

/// `PullRequestIdRequiredException`'s message: a FIXED sentence, for a request that
/// carried no `pullRequestId` member at all — the case the request boundary
/// (`crate::service`) refuses before any handler reads it. A member that IS carried
/// but names no pull request is `InvalidPullRequestIdException`
/// ([`invalid_pull_request_id_message`]).
pub const PULL_REQUEST_ID_REQUIRED_MESSAGE: &str =
    "A pull request ID is required, but none was provided.";

/// `TitleRequiredException`'s message: a FIXED sentence, worded here once for
/// every operation that takes a pull request `title` — the one that opens a pull
/// request and the one that retitles it answer a missing title identically.
pub const TITLE_REQUIRED_MESSAGE: &str = "A pull request title is required.";

/// `RepositoryNotAssociatedWithPullRequestException`'s message: the sentence pair
/// the model states for that error shape, echoing nothing from the request.
///
/// The one place this crate words that code, for the same reason
/// [`pull_request_does_not_exist_message`] is the one place it words its own: a
/// pull request that exists but belongs to another repository is refused the same
/// way by every operation that takes both a `repositoryName` and a
/// `pullRequestId`.
pub const REPOSITORY_NOT_ASSOCIATED_WITH_PULL_REQUEST_MESSAGE: &str =
    "The repository does not contain any pull requests with that pull request ID. Use \
     GetPullRequest to verify the correct repository name for the pull request ID.";

/// `ReferenceNameRequiredException`'s message: a FIXED sentence.
///
/// The one place this crate words that code, for the same reason
/// [`repository_does_not_exist_message`] is the one place it words its own: the
/// request boundary (`crate::service`) and every handler that classifies a
/// reference answer the same text.
pub const REFERENCE_NAME_REQUIRED_MESSAGE: &str =
    "A reference name is required, but none was provided.";

/// `InvalidReferenceNameException`'s message: the sentence pair the model states
/// for that error shape, echoing nothing from the request.
pub const INVALID_REFERENCE_NAME_MESSAGE: &str =
    "The specified reference name format is not valid. Reference names must conform to the Git \
     references format (for example, refs/heads/main).";

/// `ReferenceTypeNotSupportedException`'s message: a FIXED sentence.
pub const REFERENCE_TYPE_NOT_SUPPORTED_MESSAGE: &str =
    "The specified reference is not a supported type.";

/// `ReferenceDoesNotExistException`'s message: a FIXED sentence that names
/// neither the reference nor the repository.
pub const REFERENCE_DOES_NOT_EXIST_MESSAGE: &str = "The specified reference does not exist.";

/// An approval rule template's description as CodeCommit HOLDS it, whatever a
/// request carried.
///
/// A template holds no empty description: one set to an empty string simply has
/// none, so `approvalRuleTemplateDescription` is ABSENT on the answer and on every
/// later read rather than reported as an empty string (the recorded behaviour the
/// corpus names
/// "approval-rule-template-empty-description-normalized-to-absent"). Every write
/// of a description — the create and every update — goes through here, so they
/// cannot disagree about what an empty one means.
pub fn normalized_template_description(description: Option<String>) -> Option<String> {
    description.filter(|text| !text.is_empty())
}

/// The SHA-256 hash signature CodeCommit reports for a template's content
/// (`ruleContentSha256`): the digest of the content bytes, hex encoded. Those bytes
/// are the canonical ones ([`canonical_json_document`]), so the signature of a
/// document does not depend on how its submitter formatted it.
pub fn content_sha256(content: &str) -> String {
    ::hex::encode(Sha256::digest(content.as_bytes()))
}

/// A JSON document CodeCommit stores and hands back, in its canonical form:
/// minified — every byte of RFC 8259 insignificant whitespace (space, tab, CR, LF
/// between tokens) removed — with the member order, the string literals and the
/// number literals exactly as submitted. A service that accepts a JSON document and
/// reports it back does not echo the submitted formatting: a caller that stores the
/// returned text, diffs it against a later read, or hashes it to detect drift sees a
/// change only when the document itself changed.
///
/// Whitespace is dropped by scanning the text rather than by re-serializing the
/// parsed value, because a re-serialization would sort the members (`serde_json`'s
/// `Map` is a `BTreeMap` unless its `preserve_order` feature is on) and renormalise
/// number literals (`1e3` to `1000.0`), both of which this canonical form preserves.
/// Whitespace inside a string literal is part of the value, so it survives, escapes
/// included.
///
/// Callers hand this a document `serde_json` has already accepted (for approval rule
/// template content, [`is_approval_rule_template_content`]); given text that
/// is not JSON it is still total, simply stripping whitespace outside quotes.
///
/// Two sibling services canonicalize a document of their own:
/// `service-com-amazonaws-rum-rum`'s `canonical_policy_document` scans the text the
/// same way, and `service-com-amazonaws-schemas-schemas`'s `canonical_json`
/// re-encodes through `serde_transcode`. The re-encode route is not used here
/// because, as that function's own doc comment records, it keeps a number's VALUE
/// but not its literal (`1e3` comes back `1000.0`), and this canonical form keeps
/// the literal a caller submitted.
pub fn canonical_json_document(document: &str) -> String {
    let mut canonical = String::with_capacity(document.len());
    // Where the scan is: inside a string literal, and whether the previous
    // character was the backslash of an escape (so an escaped quote does not close
    // the literal).
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            canonical.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                canonical.push(character);
            }
            // The four insignificant whitespace characters JSON defines. Everything
            // else outside a string literal is structure or a bare literal, and is
            // kept exactly as it came in.
            ' ' | '\t' | '\r' | '\n' => {}
            _ => canonical.push(character),
        }
    }
    canonical
}

/// Could `value` be a `RuleContentSha256` at all — the hex form [`content_sha256`]
/// reports, 64 hex characters for a 256-bit digest?
///
/// A value outside that form is not a signature of any content, which is what the
/// operations report as `InvalidRuleContentSha256Exception`.
pub fn is_content_sha256(value: &str) -> bool {
    value.len() == Sha256::output_size() * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The repository's opaque id: an RFC 9562 version-8 (custom) UUID whose 122
/// implementation-defined bits are a digest of the account, the repository name and
/// the creation instant. It is therefore stable for the life of the repository and
/// distinct for a repository of the same name created later.
fn repository_id(name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/{name}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// The id of a repository's tagging record — the id reported beside the tags
/// themselves: the same kind of version-8 UUID as every other opaque id this
/// service mints, over the account, the repository name and the instant the
/// repository was created, so it is stable for the life of the repository and
/// distinct for a repository of the same name created later.
fn tagging_id(name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/{name}/{nanos}/tags",
        ::engine::emulator_account_id()
    ))
}

/// An approval rule template's opaque `approvalRuleTemplateId`: the same kind of
/// version-8 UUID, over the account, the template name and the instant it was
/// created. It is therefore stable for the life of the template and distinct for a
/// template of the same name created later.
fn approval_rule_template_id(name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/approval-rule-template/{name}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// The widths of the dash-separated groups of a UUID in its canonical textual
/// form, 8-4-4-4-12.
const UUID_GROUP_WIDTHS: &[usize] = &[8, 4, 4, 4, 12];

/// Is this a UUID in its canonical textual form — [`UUID_GROUP_WIDTHS`] groups of
/// hexadecimal digits joined by dashes?
fn is_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == UUID_GROUP_WIDTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_WIDTHS)
            .all(|(group, width)| {
                group.len() == *width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// The separator between the two halves of a comment id.
const COMMENT_ID_SEPARATOR: char = ':';

/// Is this a syntactically valid comment id — the FORM CodeCommit mints a comment
/// id as, a pair of UUIDs joined by [`COMMENT_ID_SEPARATOR`]?
///
/// Only the form is judged here: whether an id of this form names a comment this
/// service holds is a separate question, answered by looking the comment up, which
/// is what keeps `InvalidCommentIdException` ("Make sure that you have provided the
/// full comment ID" — the half of an id, or something that is no id at all) apart
/// from `CommentDoesNotExistException` (a full id naming no comment). An id missing
/// one of the two halves is therefore malformed rather than absent.
pub fn is_comment_id(candidate: &str) -> bool {
    candidate
        .split_once(COMMENT_ID_SEPARATOR)
        .is_some_and(|(first, second)| is_uuid(first) && is_uuid(second))
}

/// Why a `commentId` cannot name a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentIdProblem {
    /// The id is absent or blank: the operations model `CommentIdRequiredException`
    /// for this.
    Missing,
    /// The id is not the form a comment id takes: `InvalidCommentIdException`.
    Invalid,
}

/// The comment id a request names, with surrounding space removed, or why it
/// names none. Every operation that takes a `commentId` classifies it here, so
/// they cannot disagree about which ids are addressable.
pub fn normalize_comment_id(raw: &str) -> Result<&str, CommentIdProblem> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CommentIdProblem::Missing);
    }
    if !is_comment_id(trimmed) {
        return Err(CommentIdProblem::Invalid);
    }
    Ok(trimmed)
}

/// Which of `CommentDeletedException`'s two fixed templates a refusal carries.
///
/// The exception is raised by several operations, and CodeCommit does not word
/// them all alike: what the caller ASKED decides the text, so the template is
/// chosen here by the kind of call rather than shared by every raiser. Neither
/// template carries a value from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentDeletedRefusal {
    /// The exception's own sentence pair, with no lead clause: the recorded
    /// answer where the call only asks ABOUT the comment whose content is gone
    /// (`GetComment`), or about something that hangs off it rather than the
    /// comment itself (`PutCommentReaction`, for which no lead clause is
    /// recorded).
    AlreadyDeleted,
    /// That pair led by "You cannot delete this comment.": the recorded answer
    /// where the call asks to EDIT OR DELETE the comment itself, which is the
    /// pair of calls the exception's own documented sentence names ("You cannot
    /// edit or delete a deleted comment") — a repeated `DeleteCommentContent`
    /// and an `UpdateComment`.
    EditOrDeleteRefused,
}

/// The message `CommentDeletedException` carries for `refusal`: a FIXED sentence
/// carrying no value from the request, which is why each template is a constant
/// rather than built per call.
///
/// CodeCommit answers a call against a comment whose content has been deleted with
/// "The specified comment has already been marked as deleted. Verify that you
/// specified a valid comment ID, and then try again.", led — where the call asked
/// to edit or delete that comment — by "You cannot delete this comment.". Both
/// templates are stated once here, so two operations can differ only by naming a
/// different [`CommentDeletedRefusal`] and never by wording the same template
/// twice.
///
/// The sentences of each template are separated by TWO spaces, which is part of
/// the recorded text rather than formatting latitude — hence the explicit
/// `concat!` rather than a line-continued literal, whose continuation would
/// swallow them.
pub fn comment_deleted_message(refusal: CommentDeletedRefusal) -> &'static str {
    const ALREADY_DELETED: &str = concat!(
        "The specified comment has already been marked as deleted.",
        "  ",
        "Verify that you specified a valid comment ID, and then try again.",
    );
    const EDIT_OR_DELETE_REFUSED: &str = concat!(
        "You cannot delete this comment.",
        "  ",
        "The specified comment has already been marked as deleted.",
        "  ",
        "Verify that you specified a valid comment ID, and then try again.",
    );
    match refusal {
        CommentDeletedRefusal::AlreadyDeleted => ALREADY_DELETED,
        CommentDeletedRefusal::EditOrDeleteRefused => EDIT_OR_DELETE_REFUSED,
    }
}

/// A comment's opaque `commentId`: the form [`is_comment_id`] judges, a pair of
/// version-8 UUIDs joined by [`COMMENT_ID_SEPARATOR`], each over the account, the
/// repository, the comment's post order and the instant it was posted. It is
/// therefore stable for the life of the comment and distinct from every other
/// comment's, and the minter and the validator agree about the form by
/// construction. The two halves are two digests of the SAME comment's seed (the
/// suffixes only keep them apart), since nothing about a comment is recorded as
/// living in one half rather than the other.
pub fn comment_id(repository: &str, sequence: u64, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    let seed = format!(
        "codecommit/{}/{repository}/comment/{sequence}/{nanos}",
        ::engine::emulator_account_id()
    );
    let first = uuid_v8(&format!("{seed}/first"));
    let second = uuid_v8(&format!("{seed}/second"));
    format!("{first}{COMMENT_ID_SEPARATOR}{second}")
}

/// The message `CommentDoesNotExistException` carries about `comment_id`: the
/// FIXED sentence "The following comment ID does not exist: " and then the id the
/// request named, which is the only variable part.
///
/// Every operation that looks a comment up refuses a full id naming no comment
/// with this template, so none of them can word it differently.
pub fn comment_does_not_exist_message(comment_id: &str) -> String {
    format!("The following comment ID does not exist: {comment_id}")
}

fn nanos_since_epoch(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// An RFC 9562 version-8 (custom) UUID whose 122 implementation-defined bits are a
/// digest of `seed`, in the canonical 8-4-4-4-12 textual form.
fn uuid_v8(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 8 in the version nibble, and the RFC's variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Does this repository name carry the prohibited `.git` suffix?
///
/// CodeCommit documents it on every operation that takes a name for a repository
/// it is about to hold (`CreateRepository`, `UpdateRepositoryName`): "The suffix
/// .git is prohibited." Every other restriction on `RepositoryName` is the
/// modelled length and pattern, which the boundary validator refuses before a
/// handler runs, so this is the one name rule a handler is left to apply. Git
/// repository names are compared case-insensitively on this suffix, so `.GIT` is
/// prohibited too.
pub fn has_prohibited_git_suffix(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".git")
}

/// The number of hexadecimal digits after the `mrk-` prefix of the id KMS mints
/// for a multi-region key.
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// Is this a KMS key id — a UUID for a single-region key, as every KMS `KeyId`
/// example is (`1234abcd-12ab-34cd-56ef-1234567890ab`), or `mrk-` and
/// [`MULTI_REGION_KEY_ID_DIGITS`] hexadecimal digits for a multi-region key?
///
/// This emulator resolves no KMS key, so only the FORM of the selector is
/// judged — but that form is the id's two shapes exactly, read through the same
/// [`is_uuid`] every other UUID-shaped selector in this module is read through.
fn is_kms_key_id(candidate: &str) -> bool {
    if let Some(digits) = candidate.strip_prefix("mrk-") {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    is_uuid(candidate)
}

/// Is this a KMS alias path — `alias/` followed by a non-empty name of the
/// characters `KmsKeyId`'s modelled pattern `^[a-zA-Z0-9:/_-]+$` allows?
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix("alias/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// Is this the full ARN of a KMS key or alias
/// (`arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`)?
///
/// Read with the same six-field `splitn` this module's
/// [`repository_name_of_resource_arn`] uses, so one file has one way of reading an
/// ARN; this adds only which vendor and resource the value has to name.
fn is_kms_arn(candidate: &str) -> bool {
    let [prefix, partition, vendor, region, account, resource] =
        candidate.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return false;
    };
    prefix == "arn"
        && !partition.is_empty()
        && vendor == "kms"
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix("key/").is_some_and(is_kms_key_id))
}

/// Does `key_id` select a KMS key at all?
///
/// `KmsKeyId`'s documentation sends a caller to KMS for the acceptable values —
/// "For more information about acceptable values for keyID, see KeyId in the
/// Decrypt API description in the Key Management Service API Reference"
/// (`smithy_query(query="doc",shape="UpdateRepositoryEncryptionKeyInput",member="kmsKeyId")`)
/// — and that description lists four forms: a key id, a key ARN, an alias name
/// and an alias ARN. The modelled pattern `^[a-zA-Z0-9:/_-]+$` admits text that
/// is none of them, so this is the rule a handler is left to apply; a value
/// outside every form names no key, and the operation answers the modelled
/// `EncryptionKeyInvalidIdException`.
pub fn is_kms_key_selector(key_id: &str) -> bool {
    is_kms_alias(key_id) || is_kms_key_id(key_id) || is_kms_arn(key_id)
}

/// The message `EncryptionKeyInvalidIdException` carries: one code, one
/// sentence, worded in a single place so the two operations that take a
/// `kmsKeyId` cannot word the same refusal differently. The only variable part
/// is the selector the request named.
pub fn invalid_encryption_key_id_message(key_id: &str) -> String {
    format!(
        "the encryption key id {key_id} is not valid: it names neither a key id, a key ARN, an alias nor an alias ARN"
    )
}

/// The Amazon Web Services managed key a repository is encrypted under when the
/// request names no key of its own, in the alias form KMS names that key by:
/// `kmsKeyId`'s own documentation states "If no key is specified, the default
/// aws/codecommit Amazon Web Services managed key is used"
/// (`smithy_query(query="doc",shape="CreateRepositoryInput",member="kmsKeyId")`).
pub const DEFAULT_ENCRYPTION_KEY_ALIAS: &str = "alias/aws/codecommit";

/// The ONE form a repository's encryption key is held and reported in, for every
/// selector that names a key ([`is_kms_key_selector`]).
///
/// KMS's `KeyId` admits four selectors for the same key, and of them only the
/// bare key id leaves the key's account and region unsaid; a key id therefore
/// resolves to the ARN of that key in the repository's own account and region,
/// the identifier KMS reports a key by, while a selector that already names a
/// key or an alias in full is kept as the caller set it.
pub fn canonical_encryption_key(selector: &str, region: &str) -> String {
    if is_kms_key_id(selector) {
        return ::engine::emulator_arn("kms", region, &format!("key/{selector}"));
    }
    selector.to_string()
}

/// The region this request is addressed to: the SigV4 credential scope's region,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The session registry in the SAME shared engine: where the identity a temporary
/// credential was minted for is held.
///
/// The STS service's OWN accessor is called rather than copied, so this service
/// resolves a caller from the very records `sts:AssumeRole` writes and
/// `sts:GetCallerIdentity` reads. `None` means the engine carries no such store — a
/// router that mounts this service without STS, where no session can exist at all.
fn session_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<sts_state::StsState>> {
    sts_state::service_state(engine)
}

/// The identity that made this request, as an ARN: what an operation reports as
/// the author of something the caller created, and the reviewer an approval is
/// recorded for.
///
/// A request speaks for the principal its CREDENTIAL was minted for, so who that is
/// is asked of the session registry ([`session_registry`]) through STS's own
/// resolution of a request's caller rather than a second reading of it: a request
/// signed with the temporary credentials of an assumed role speaks for that role
/// SESSION, so the identity is the `assumed-role` ARN STS recorded for it — which is
/// the spelling an approval rule's `ApprovalPoolMembers` entry names and a later
/// reader reports — rather than an identity invented from the key's text.
///
/// A key that names no session is a LONG-TERM key, and the identity such a key
/// speaks for is the IAM user the account holds it for
/// ([`long_term_caller_identity`]) — the same resolution STS reports for it. A
/// session whose expiry the clock has passed resolves there too: STS refuses such a
/// credential, and the same call materialises the expiry, deleting the credential
/// from the store so the next request presenting it does not verify at all.
///
/// Every answer therefore comes from ONE resolution of the access key id — the one
/// `sts:GetCallerIdentity` answers that same request with — so an ARN this service
/// records (a comment's author, a reaction's owner, an approval's reviewer) is a value
/// the caller can compare with its own identity, and never an identity invented here.
pub fn caller_identity_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let long_term = long_term_caller_identity(engine, &access_key_id);
    let Some(registry) = session_registry(engine) else {
        return long_term.arn;
    };
    let resolved = sts_state::resolve_request_caller(
        &mut registry.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term.clone(),
    )
    .map(|caller| caller.identity);
    resolved.unwrap_or(long_term).arn
}

/// The identity a LONG-TERM access key speaks for: STS's OWN resolution of such a
/// key against the account's IAM records ([`sts_state::long_term_identity`]) — the
/// user the account holds the key for, and, for a key no IAM record names, the
/// account's own principal, which is the identity the key pair this emulator
/// publishes at start speaks for.
///
/// The resolution is called rather than reproduced, and no answer of its own is
/// substituted for the one it gives: a caller reading its identity through
/// `sts:GetCallerIdentity` and a caller reading an ARN this service recorded for the
/// same credential have to see one principal, which they cannot if this service
/// derives an identity from the key id's text. Two callers that have to be two
/// principals are two IAM users of the account, each holding its own access key —
/// the records this resolution reads — which is the position two callers are in on
/// AWS.
fn long_term_caller_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
) -> sts_state::Identity {
    let iam = sts_state::iam_state(engine);
    let guard = iam.as_ref().map(|iam| iam.lock());
    sts_state::long_term_identity(guard.as_deref(), access_key_id)
}

/// A `SystemTime` as the modeled timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// A git identity date: unix seconds plus a GMT offset, the form git (and so
/// CodeCommit's `UserInfo.date`) uses.
pub fn git_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs} +0000")
}

/// Is this a syntactically valid git object id (40 hex characters)?
pub fn is_object_id(candidate: &str) -> bool {
    git_object::is_object_id(candidate)
}

/// Can this commit specifier name a commit at all? `CommitName` is "the branch,
/// tag, HEAD, or other fully qualified reference used to identify a commit (for
/// example, a branch name or a full commit ID)", so a git object id and any git
/// reference name can; anything git itself would refuse as a reference name
/// cannot, and that is what the merge operations' `Invalid…CommitException` /
/// `Invalid…CommitSpecifierException` report.
///
/// One reading for every operation that takes a `sourceCommitSpecifier` /
/// `destinationCommitSpecifier`, so two of them cannot disagree about which
/// specifier is addressable at all.
pub fn names_a_commit(specifier: &str) -> bool {
    is_object_id(specifier) || is_valid_branch_name(specifier)
}

/// The longest reference name this service accepts.
///
/// `BranchName`'s modelled `length`
/// (`smithy_query(query="operation",operation="CreateBranch")` reports
/// `len 1..256`): a reference longer than this is one the control plane could not
/// address, so neither face of the service writes it.
pub const MAX_REF_NAME: usize = 256;

/// Can `refs/heads/<name>` be a git reference? CodeCommit is a git host, so a
/// branch name is a reference name: the rules below are git's own
/// (`git check-ref-format`), which is what makes a name a repository can hold
/// tellable from one it cannot.
///
/// The model constrains only the length of `BranchName`
/// (`smithy_query(query="operation",operation="CreateBranch")` reports
/// `len 1..256`), so every other malformed name arrives at a handler and is
/// refused there with `InvalidBranchNameException`. A caller that is not held to
/// the modelled length by the boundary validator — a git push, whose ref name is
/// not a modelled member — goes through [`is_valid_ref_name`], which adds it.
pub fn is_valid_branch_name(name: &str) -> bool {
    // git refuses a bare "@" as a reference name, a name that ends in "/" or
    // ".", the sequences "..", "//" and "@{", and every ASCII control
    // character as well as space ~ ^ : ? * [ and backslash.
    if name.is_empty() || name == "@" {
        return false;
    }
    if name.ends_with('/') || name.ends_with('.') {
        return false;
    }
    if name.contains("..") || name.contains("//") || name.contains("@{") {
        return false;
    }
    if name.bytes().any(|byte| {
        byte.is_ascii_control()
            || matches!(
                byte,
                b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'
            )
    }) {
        return false;
    }
    // No slash-separated component may be empty, start with "." or end in
    // ".lock" — the leading "/" case is the first component being empty.
    name.split('/').all(|component| {
        !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
    })
}

/// Can this be the short name of a reference a WRITE may create, in either
/// namespace (`refs/heads/<name>`, `refs/tags/<name>`)?
///
/// [`is_valid_branch_name`]'s rule — git's own `check-ref-format` — plus
/// [`MAX_REF_NAME`]. One implementation for both faces of the service: a name a
/// git push can create is exactly a name `CreateBranch` accepts, so the two
/// cannot disagree about which references a repository may hold. Smithy counts a
/// string's length in Unicode code points, as the boundary validator does for the
/// modelled member.
pub fn is_valid_ref_name(name: &str) -> bool {
    name.chars().count() <= MAX_REF_NAME && is_valid_branch_name(name)
}

/// Why a reference a pull request target names cannot address a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceProblem {
    /// The reference is absent or blank: the operations model
    /// `ReferenceNameRequiredException` for this.
    Missing,
    /// The reference is not a name a git reference can take:
    /// `InvalidReferenceNameException`.
    Invalid,
    /// The reference is well formed but names something other than a branch — a
    /// tag, or another `refs/` namespace: `ReferenceTypeNotSupportedException`.
    UnsupportedType,
}

/// The BRANCH a pull request target's reference names, or why it names none.
///
/// A pull request is between two branches: `ReferenceName` documents the member
/// as "The branch of the repository …", and
/// `ReferenceTypeNotSupportedException` is what CodeCommit answers for a
/// reference of any other kind. CodeCommit is a git host, so both the short
/// branch name and the fully qualified `refs/heads/<branch>` name the same
/// branch — the form the request used is not what is stored, since
/// `PullRequestTarget` reports the qualified form either way (see
/// [`qualified_reference`]).
///
/// The one place this crate classifies such a reference, so the operations that
/// take one cannot disagree about which references address a branch.
pub fn pull_request_reference(raw: &str) -> Result<&str, ReferenceProblem> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ReferenceProblem::Missing);
    }
    // A reference in another namespace than `refs/heads/` is a reference of a
    // type a pull request does not take, rather than a malformed one.
    if let Some(name) = trimmed.strip_prefix("refs/") {
        let Some(branch) = name.strip_prefix("heads/") else {
            return Err(ReferenceProblem::UnsupportedType);
        };
        return if is_valid_ref_name(branch) {
            Ok(branch)
        } else {
            Err(ReferenceProblem::Invalid)
        };
    }
    if is_valid_ref_name(trimmed) {
        Ok(trimmed)
    } else {
        Err(ReferenceProblem::Invalid)
    }
}

/// The fully qualified reference of a branch: `refs/heads/<branch>`, the form
/// `PullRequestTarget`'s `sourceReference` and `destinationReference` report.
pub fn qualified_reference(branch: &str) -> String {
    format!("{HEADS_PREFIX}{branch}")
}

/// The folder a path lives in: everything before its last `/`, and the empty string
/// for a path at the root of the tree. The ONE place this crate keys a tree snapshot
/// by folder, so no reading of CodeCommit's folder rule can key it differently.
pub fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The total content size of the folder a path lives in, over the sizes a tree
/// snapshot records for that folder's immediate entries.
///
/// A caller charging EVERY folder of a tree reads them in one pass instead
/// ([`folder_content_bytes_by_folder`]); this is the per-path question, for a
/// write that touches one folder.
pub fn folder_content_bytes(files: &BTreeMap<String, FileEntry>, path: &str) -> i64 {
    let folder = folder_of(path);
    files
        .iter()
        .filter(|(other, _)| folder_of(other) == folder)
        .map(|(_, entry)| entry.size)
        .fold(0i64, i64::saturating_add)
}

/// The total content size of EVERY folder of a tree snapshot, keyed by that folder
/// ([`folder_of`], so the root folder is the empty key).
///
/// [`folder_content_bytes`] for the whole tree in ONE pass, for a caller that charges
/// every folder a write touches rather than the one folder a single path lives in —
/// the same sizes, summed the same saturating way, so the two cannot drift.
pub fn folder_content_bytes_by_folder(files: &BTreeMap<String, FileEntry>) -> BTreeMap<&str, i64> {
    let mut folders: BTreeMap<&str, i64> = BTreeMap::new();
    for (path, entry) in files {
        let total = folders.entry(folder_of(path)).or_insert(0);
        *total = total.saturating_add(entry.size);
    }
    folders
}

/// The `blobId` of a file's bytes.
pub fn blob_id(content: &[u8]) -> String {
    git_object::object_id(Kind::Blob, content)
}

/// The git mode of a modeled `fileMode`: the numeric file mode permission code a
/// git tree entry carries, which is what `BlobMetadata.mode` reports (`100644`
/// read/write, `100755` read/write/execute, `120000` a symlink — see
/// `smithy_query(query="doc",shape="BlobMetadata",member="mode")`).
pub fn git_mode(file_mode: &str) -> &'static str {
    match file_mode {
        FILE_MODE_EXECUTABLE => git_object::MODE_EXECUTABLE,
        FILE_MODE_SYMLINK => git_object::MODE_SYMLINK,
        // NORMAL, and anything the enum does not declare, is a regular file.
        _ => git_object::MODE_FILE,
    }
}

/// The modeled `fileMode` a git tree entry mode names.
pub fn file_mode_of(mode: &str) -> &'static str {
    match mode {
        git_object::MODE_EXECUTABLE => FILE_MODE_EXECUTABLE,
        git_object::MODE_SYMLINK => FILE_MODE_SYMLINK,
        _ => FILE_MODE_NORMAL,
    }
}

/// The tree objects a whole snapshot is made of: the root's name, and every tree
/// object (root and nested) the snapshot needs, exactly as git would write them.
///
/// `None` when an entry's `blobId` is not an object name, which only a snapshot
/// built outside this module could carry: every writer here puts an id
/// [`Repository::insert_blob`] reported.
pub fn tree_objects(files: &BTreeMap<String, FileEntry>) -> Option<(String, Vec<Object>)> {
    let mut objects: Vec<Object> = Vec::new();
    let root = build_tree(files, &mut objects, 0)?;
    Some((root, objects))
}

/// The `treeId` of a whole tree snapshot, or `None` when an entry's `blobId` is
/// not an object name.
pub fn tree_id(files: &BTreeMap<String, FileEntry>) -> Option<String> {
    tree_objects(files).map(|(root, _)| root)
}

fn build_tree(
    files: &BTreeMap<String, FileEntry>,
    objects: &mut Vec<Object>,
    depth: usize,
) -> Option<String> {
    // One level of recursion per path segment, so the depth of this walk is bounded
    // where it happens — exactly as the opposite walk, [`Repository::flatten_tree_into`],
    // bounds itself. Every `filePath` a request supplies is already held to
    // [`MAX_PATH_SEGMENTS`] by [`normalize_path`], but this function is public and a
    // snapshot can also be assembled from a pushed packfile, so a snapshot whose keys
    // nest deeper than that is refused here rather than recursed into: a pathological
    // path cannot exhaust the stack (which in Rust aborts the process), it only fails
    // its own request.
    if depth >= MAX_PATH_SEGMENTS {
        return None;
    }
    let mut blobs: BTreeMap<&str, &FileEntry> = BTreeMap::new();
    let mut dirs: BTreeMap<&str, BTreeMap<String, FileEntry>> = BTreeMap::new();
    for (path, entry) in files {
        match path.split_once('/') {
            None => {
                blobs.insert(path.as_str(), entry);
            }
            Some((head, rest)) => {
                dirs.entry(head)
                    .or_default()
                    .insert(rest.to_string(), entry.clone());
            }
        }
    }

    let mut entries: Vec<TreeEntry> = Vec::new();
    for (name, entry) in blobs {
        entries.push(TreeEntry {
            mode: git_mode(&entry.file_mode).to_string(),
            name: name.to_string(),
            id: entry.blob_id.clone(),
        });
    }
    for (name, subtree) in &dirs {
        entries.push(TreeEntry {
            mode: git_object::MODE_TREE.to_string(),
            name: (*name).to_string(),
            id: build_tree(subtree, objects, depth + 1)?,
        });
    }
    let tree = git_object::encode_tree(entries)?;
    let id = tree.id();
    objects.push(tree);
    Some(id)
}

/// The `commitId` of a commit: the name of the git commit object over its tree,
/// parents, author, committer and message.
pub fn commit_id(
    tree: &str,
    parents: &[String],
    author: &UserInfo,
    committer: &UserInfo,
    message: Option<&str>,
) -> String {
    git_object::encode_commit(&git_object::CommitFields {
        tree: tree.to_string(),
        parents: parents.to_vec(),
        author: git_identity(author),
        committer: git_identity(committer),
        message: message.unwrap_or_default().to_string(),
    })
    .id()
}

fn git_identity(user: &UserInfo) -> String {
    git_object::join_identity(
        user.name.as_deref().unwrap_or(""),
        user.email.as_deref().unwrap_or(""),
        user.date.as_deref().unwrap_or(EPOCH_GIT_DATE),
    )
}

/// The git date of a commit whose stored identity carries none: the epoch at GMT,
/// the form [`git_date`] writes.
const EPOCH_GIT_DATE: &str = "0 +0000";

/// A stored commit identity as the modeled `UserInfo`.
///
/// A git identity line always carries a name field, an email field and a date, so
/// all three are REPORTED — empty where the stored line records nothing there —
/// rather than omitted: the members are part of the shape a reader of a commit
/// sees, not a function of what the write happened to name.
fn identity_info(user: &UserInfo) -> UserInfo {
    UserInfo {
        name: Some(user.name.clone().unwrap_or_default()),
        email: Some(user.email.clone().unwrap_or_default()),
        date: Some(
            user.date
                .clone()
                .unwrap_or_else(|| EPOCH_GIT_DATE.to_string()),
        ),
    }
}

/// The NAME of a principal: the last segment of the resource part of its ARN —
/// the IAM user name of `.../user/<name>`, the session name of an assumed-role
/// principal, `root` for an account's own principal.
///
/// A git identity line records a person's name, not an account's address for
/// them, and CodeCommit fills an identity member the request left out with the
/// name of the calling principal — the value a git client shows for the commit,
/// and the value a reader comparing a commit's author against a user name can
/// match. A value that is no ARN at all has no resource part and stands for
/// itself.
pub fn identity_name(principal: &str) -> &str {
    let resource = principal
        .strip_prefix("arn:")
        .and_then(|rest| rest.splitn(5, ':').nth(4))
        .unwrap_or(principal);
    resource
        .split(['/', ':'])
        .next_back()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(principal)
}

/// The identity a commit this service writes is attributed to, from the identity
/// members the request carried and the principal that made the call.
///
/// "If no name is specified, the user's ARN is used as the author name and
/// committer name" (`DeleteFileInput`'s `name`, the member the model states the
/// rule on): a commit is never authored by nobody, so an identity member the
/// request leaves out is filled from the calling principal — the only identity of
/// a caller this service holds. What is recorded for it is that principal's NAME
/// ([`identity_name`]), the git identity field a commit carries, rather than the
/// whole ARN naming it. An operation whose model says a member of ITS OWN is left
/// blank instead passes that blank explicitly.
///
/// `caller` is the principal the request was signed by ([`caller_identity_arn`]),
/// and `at` the instant the commit is written.
pub fn commit_identity(
    name: Option<String>,
    email: Option<String>,
    caller: &str,
    at: SystemTime,
) -> UserInfo {
    let caller_name = identity_name(caller);
    UserInfo {
        // A name of nothing but whitespace specifies no name at all: it would
        // leave the commit's identity line naming nobody.
        name: Some(
            name.filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| caller_name.to_string()),
        ),
        email: Some(email.unwrap_or_else(|| caller_name.to_string())),
        date: Some(git_date(at)),
    }
}

/// Why a `filePath` cannot address a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathProblem {
    /// The path is absent, or empty once leading separators are removed: the
    /// operations model `PathRequiredException` for this.
    Missing,
    /// The path cannot name a file in a tree: `InvalidPathException`.
    Invalid,
}

/// The canonical form of a `filePath`: leading separators removed, so
/// `"/README.md"` and `"README.md"` address the same file, as they do in a git
/// tree where a path is always relative to the root.
pub fn normalize_path(raw: &str) -> Result<String, PathProblem> {
    let trimmed = raw.trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(PathProblem::Missing);
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() > MAX_PATH_SEGMENTS {
        return Err(PathProblem::Invalid);
    }
    // An empty, "." or ".." segment names no entry in a tree, and a NUL cannot
    // appear in a git tree entry name.
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == ".." )
        || trimmed.contains('\0')
    {
        return Err(PathProblem::Invalid);
    }
    Ok(trimmed.to_string())
}

/// Why a paginated list request cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageProblem {
    /// `maxResults` is outside the published range: the list operations model
    /// `InvalidMaxResultsException` for this. Carries the refusal message.
    InvalidMaxResults(String),
    /// `nextToken` is not a token this service issued: the list operations model
    /// `InvalidContinuationTokenException` for this. Carries the refusal message.
    InvalidContinuationToken(String),
}

/// One page of an ordered sequence of names, and the token that resumes after it.
///
/// The single pagination rule for this service's `maxResults`/`nextToken`
/// operations: `maxResults` must lie in [`MIN_MAX_RESULTS`]..=[`MAX_MAX_RESULTS`]
/// (absent means [`DEFAULT_MAX_RESULTS`]), `nextToken` must be a token this service
/// issued FOR THIS LISTING (see [`page_token`]) that still addresses a position in
/// the sequence, and the answer carries a token only when a further page exists.
///
/// `scope` names the listing the token belongs to — the operation, plus whatever
/// else picks the sequence out (the repository a per-repository listing reads), so
/// a token issued for one listing is not a token for another.
pub fn page_of(
    scope: &str,
    ordered: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<String>, Option<String>), PageProblem> {
    page_of_at_most(
        scope,
        ordered,
        next_token,
        max_results,
        MAX_MAX_RESULTS,
        DEFAULT_MAX_RESULTS,
    )
}

/// The same one page, for an operation that publishes its OWN `maxResults`
/// maximum and default rather than the range the list operations share, or that
/// pages items of its own rather than names.
///
/// The pagination rule is [`page_of`]'s in every other respect — the token is one
/// this service issued for THIS listing, and the answer carries a token only when
/// a further page exists — so an operation whose documentation states a different
/// page size (`DescribePullRequestEvents`'s
/// [`PULL_REQUEST_EVENTS_PAGE_MAX`]), or whose paginated member is not a list of
/// names at all (`DescribeMergeConflicts`'s merge hunks), pages through the same
/// code rather than a second copy of it.
pub fn page_of_at_most<T: Clone>(
    scope: &str,
    ordered: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    page_maximum: i32,
    page_default: i32,
) -> Result<(Vec<T>, Option<String>), PageProblem> {
    let limit = match max_results {
        None => page_default,
        Some(requested) if (MIN_MAX_RESULTS..=page_maximum).contains(&requested) => requested,
        // `InvalidMaxResultsException`'s message comes from the one place this crate
        // words that code: the accepted range, with nothing from the request.
        Some(_) => {
            return Err(PageProblem::InvalidMaxResults(invalid_max_results_message(
                MIN_MAX_RESULTS,
                page_maximum,
            )));
        }
    };
    let start = match next_token {
        None => 0usize,
        Some(token) => page_offset(scope, token)
            // A token this service issued always names a position the sequence
            // still holds: one is only ever issued while a further page exists.
            .filter(|offset| *offset < ordered.len())
            .ok_or_else(|| {
                // `InvalidContinuationTokenException`'s message comes from the one
                // place this crate words that code: the template whose slot is the
                // token the request sent.
                PageProblem::InvalidContinuationToken(invalid_continuation_token_message(token))
            })?,
    };
    let end = ordered
        .len()
        .min(start.saturating_add(usize::try_from(limit).unwrap_or(0)));
    Ok((
        ordered[start..end].to_vec(),
        (end < ordered.len()).then(|| page_token(scope, end)),
    ))
}

/// The marker every continuation token this service issues carries, so a decimal
/// offset a caller invents is not mistaken for a token the service handed out.
const PAGE_TOKEN_MARKER: &str = "codecommit-page:";

/// The continuation token that resumes the `scope` listing at `offset`.
///
/// Hex-encoded so the token is opaque to the caller and carries no structure a
/// caller can act on. It is not integrity-protected and claims nothing: a caller
/// that reverses it can only name a position in the SAME listing, which is what
/// [`page_of`] already bounds.
fn page_token(scope: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_MARKER}{scope}:{offset}"))
}

/// The position a continuation token names in the `scope` listing, or `None` for
/// anything this service did not issue for that listing. Every step is fallible on
/// purpose: the token is caller-supplied.
fn page_offset(scope: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("{PAGE_TOKEN_MARKER}{scope}:"))?
        .parse::<usize>()
        .ok()
}

/// The approval-rule-template content schema version CodeCommit publishes for the
/// `approvalRuleTemplateContent` document (the `Version` member of every template
/// body in the CodeCommit User Guide's approval rule template examples).
pub const APPROVAL_RULE_TEMPLATE_VERSION: &str = "2018-11-08";

/// The only `Statements[].Type` CodeCommit's approval rule template schema defines.
pub const APPROVAL_RULE_STATEMENT_TYPE: &str = "Approvers";

/// Is this `approvalRuleTemplateContent` a valid approval rule document?
///
/// CodeCommit's template content is a JSON document, not free text: an object with
/// a `Version` string, a non-empty `Statements` array whose every entry is an
/// `Approvers` statement carrying a positive `NumberOfApprovalsNeeded` and, where
/// present, an `ApprovalPoolMembers` array of strings, plus an optional
/// `DestinationReferences` array of strings (the structure the CodeCommit User
/// Guide documents for an approval rule template).
///
/// The answer is a BOOLEAN, not a reason: the operations that report a rejected
/// document answer the fixed sentence for the thing the caller submitted
/// ([`INVALID_APPROVAL_RULE_CONTENT_MESSAGE`] or
/// [`INVALID_APPROVAL_RULE_TEMPLATE_CONTENT_MESSAGE`]), and the one diagnostic
/// a caller must never be handed is the JSON reader's own ("key must be a string at
/// line 1 column 3"), whose offsets belong to whichever deserializer this emulator
/// links. Returning nothing to interpolate is what keeps it out of the wire.
pub fn is_approval_rule_template_content(content: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(content) else {
        return false;
    };
    let Some(object) = document.as_object() else {
        return false;
    };

    if object.get("Version").and_then(::serde_json::Value::as_str)
        != Some(APPROVAL_RULE_TEMPLATE_VERSION)
    {
        return false;
    }

    let Some(statements) = object
        .get("Statements")
        .and_then(::serde_json::Value::as_array)
    else {
        return false;
    };
    if statements.is_empty() {
        return false;
    }
    for statement in statements {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        if statement.get("Type").and_then(::serde_json::Value::as_str)
            != Some(APPROVAL_RULE_STATEMENT_TYPE)
        {
            return false;
        }
        match statement
            .get("NumberOfApprovalsNeeded")
            .and_then(::serde_json::Value::as_i64)
        {
            Some(needed) if needed >= 1 => {}
            _ => return false,
        }
        if let Some(members) = statement.get("ApprovalPoolMembers") {
            match members.as_array() {
                Some(members) if members.iter().all(::serde_json::Value::is_string) => {}
                _ => return false,
            }
        }
    }

    if let Some(references) = object.get("DestinationReferences") {
        match references.as_array() {
            Some(references) if references.iter().all(::serde_json::Value::is_string) => {}
            _ => return false,
        }
    }
    true
}

/// The canonical form of a `folderPath`: leading and trailing separators removed,
/// so `"/src/"`, `"/src"` and `"src"` all address the same folder, and an empty
/// path (or `"/"`) addresses the repository root — the tree itself, which always
/// exists in a commit. Every other form is checked exactly as a file path is, so
/// a folder and a file in it are addressed by the same path grammar.
pub fn normalize_folder_path(raw: &str) -> Result<String, PathProblem> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        // The root folder: the whole tree, named by no segment at all.
        return Ok(String::new());
    }
    normalize_path(trimmed)
}

/// The entries of a tree snapshot that live under `folder`, keyed by their path
/// RELATIVE to that folder. `folder` is `""` for the repository root, whose
/// relative paths are the snapshot's own paths.
///
/// The result is itself a tree snapshot, so [`tree_id`] over it is the git tree
/// object name of that folder, and calling this again with one of its own folder
/// names descends another level.
pub fn subtree(files: &BTreeMap<String, FileEntry>, folder: &str) -> BTreeMap<String, FileEntry> {
    if folder.is_empty() {
        return files.clone();
    }
    let prefix = format!("{folder}/");
    files
        .iter()
        .filter_map(|(path, entry)| {
            path.strip_prefix(&prefix)
                .map(|relative| (relative.to_string(), entry.clone()))
        })
        .collect()
}

/// Every folder a path lives under, from its immediate parent outwards:
/// `"a/b/c.txt"` lives under `"a/b"` and `"a"`. Empty for a path at the root of
/// the tree, which is not a folder a deletion can empty.
pub fn ancestor_folders(path: &str) -> Vec<&str> {
    let mut folders = Vec::new();
    let mut rest = path;
    while let Some((folder, _)) = rest.rsplit_once('/') {
        folders.push(folder);
        rest = folder;
    }
    folders
}

/// The tree entry the kept-folder placeholder is stored as: the EMPTY blob, in
/// `NORMAL` mode. The caller stores that blob ([`Repository::insert_blob`]) so the
/// snapshot's entry names an object the store holds.
pub fn keep_empty_folders_entry() -> FileEntry {
    FileEntry {
        blob_id: blob_id(&[]),
        file_mode: FILE_MODE_NORMAL.to_string(),
        size: 0,
    }
}

/// Keep the folder structure a commit's deletions emptied, as `keepEmptyFolders`
/// asks: "If a file is the only object in the folder or directory, specifies
/// whether to delete the folder or directory that contains the file. By default,
/// empty folders are deleted" — and, for `CreateCommit`, "If true, a .gitkeep file
/// is created for empty folders."
///
/// Nothing else recreates a folder: a git tree holds no empty subtree. A deletion
/// can empty a whole chain of folders (`"a/b/c.txt"` empties `"a/b"` and then
/// `"a"`), so every ANCESTOR folder of a deleted path is considered, deepest
/// first: the placeholder written in the deepest emptied folder is what keeps each
/// folder above it non-empty, so the whole structure is kept with one placeholder.
///
/// Shared by every operation that takes `keepEmptyFolders` over a tree snapshot,
/// so no two of them can disagree about which folders a deletion keeps. Returns
/// the placeholder paths written, each stored as [`keep_empty_folders_entry`].
pub fn keep_empty_folders<'a>(
    files: &mut BTreeMap<String, FileEntry>,
    deleted_paths: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut candidates: Vec<&str> = deleted_paths
        .into_iter()
        .flat_map(ancestor_folders)
        .collect();
    candidates
        .sort_unstable_by_key(|folder| (std::cmp::Reverse(folder.matches('/').count()), *folder));
    candidates.dedup();
    let mut written: Vec<String> = Vec::new();
    for folder in candidates {
        // A name the tree now holds as a FILE is not a folder to keep: a git tree
        // holds one or the other under a name, never both, so a placeholder there
        // would build a snapshot no tree can encode. The file the write put there
        // is what the folder's name now holds.
        if files.contains_key(folder) {
            continue;
        }
        let prefix = format!("{folder}/");
        if files.keys().any(|path| path.starts_with(&prefix)) {
            continue;
        }
        let path = format!("{prefix}{KEEP_EMPTY_FOLDERS_PLACEHOLDER}");
        files.insert(path.clone(), keep_empty_folders_entry());
        written.push(path);
    }
    written
}

/// How many triggers one repository's configuration holds.
///
/// The model declares `MaximumRepositoryTriggersExceededException` ("The number
/// of triggers allowed for the repository was exceeded"), so a bound has to
/// exist; this is the one this emulator holds, and it is the number the
/// CodeCommit quota for triggers on a single repository publishes.
pub const MAX_TRIGGERS_PER_REPOSITORY: usize = 10;

/// How many branches ONE trigger may name.
///
/// Likewise the bound `MaximumBranchesExceededException` ("The number of
/// branches for the trigger was exceeded") exists for, at the number the
/// CodeCommit quota for branches in a single trigger publishes. A trigger that
/// names NO branch is not at this bound: an empty list "applies to all
/// branches" (`smithy_query(query="doc",shape="RepositoryTrigger",member="branches")`).
pub const MAX_BRANCHES_PER_TRIGGER: usize = 10;

/// The longest trigger name, and the longest `customData`, this emulator stores.
///
/// `RepositoryTriggerName` and `RepositoryTriggerCustomData` carry no modelled
/// length (`smithy_query(query="shape",shape="RepositoryTriggerName")`), and both
/// are caller-supplied text kept for the life of the repository, so these are the
/// emulator's OWN bounds on what one configuration can cost. A value past either
/// is refused as the modelled "not valid" error for that member, and every record
/// the configuration holds is released by `DeleteRepository`.
pub const MAX_TRIGGER_NAME_CHARACTERS: usize = 100;
pub const MAX_TRIGGER_CUSTOM_DATA_CHARACTERS: usize = 1_000;

/// The one `RepositoryTriggerEventEnum` value that stands for every repository
/// event. The model documents that it cannot be combined: "The valid value
/// \"all\" cannot be used with any other values."
/// (`smithy_query(query="doc",shape="RepositoryTrigger",member="events")`).
pub const REPOSITORY_TRIGGER_EVENT_ALL: &str = "all";

/// The ARN namespaces a trigger destination may name: a repository trigger
/// "runs actions in another service, such as sending a notification through
/// Amazon SNS" (`RepositoryTrigger.events`), and the two targets CodeCommit
/// publishes for a trigger are an Amazon SNS topic and a Lambda function. An ARN
/// of any other service is not a destination a trigger can run.
const TRIGGER_DESTINATION_NAMESPACES: &[&str] = &["sns", "lambda"];

/// `RepositoryTriggersListRequiredException`'s message: the shape's own
/// documented sentence, for the one refusal about the trigger LIST rather than
/// about a trigger in it — the list is `@required`, so this is what a request
/// that carries none is answered with.
pub const REPOSITORY_TRIGGERS_LIST_REQUIRED_MESSAGE: &str =
    "The list of triggers for the repository is required, but was not specified.";

/// `InvalidRepositoryTriggerDestinationArnException`'s message: the text the
/// service itself answers a destination that is not an ARN with, which is
/// entirely FIXED — it names neither the offending value, nor the trigger, nor
/// the repository, so nothing about it is read off the request.
///
/// It is not the error shape's documented sentence: that sentence is prose about
/// the error class for a reader of the API reference, and a client branching on
/// the message a trigger refusal carries matches this one.
pub const TRIGGER_DESTINATION_ARN_MALFORMED_MESSAGE: &str =
    "Repository trigger destination arn is incorrectly formatted";

/// Why a trigger in a `RepositoryTriggersList` cannot be stored for a
/// repository. Each variant is the modelled error the trigger operations answer
/// for it, and [`TriggerProblem::message`] is the text that comes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerProblem {
    /// The configuration holds more triggers than one repository may:
    /// `MaximumRepositoryTriggersExceededException`.
    TooManyTriggers,
    /// A trigger carries no name at all: `RepositoryTriggerNameRequiredException`.
    NameMissing,
    /// A name past [`MAX_TRIGGER_NAME_CHARACTERS`], or one carrying a control
    /// character: `InvalidRepositoryTriggerNameException`.
    NameInvalid,
    /// Two triggers in one configuration carry the same name, which would leave
    /// the repository with two triggers one name addresses:
    /// `InvalidRepositoryTriggerNameException`.
    NameNotUnique,
    /// A trigger carries no destination:
    /// `RepositoryTriggerDestinationArnRequiredException`.
    DestinationArnMissing,
    /// The destination is not the ARN of a service a trigger can run:
    /// `InvalidRepositoryTriggerDestinationArnException`.
    DestinationArnInvalid,
    /// The destination is in another Region than the repository:
    /// `InvalidRepositoryTriggerRegionException`.
    DestinationRegion,
    /// A trigger names no event, so nothing would ever run it:
    /// `RepositoryTriggerEventsListRequiredException`.
    EventsMissing,
    /// An event outside `RepositoryTriggerEventEnum`, or one named twice:
    /// `InvalidRepositoryTriggerEventsException`.
    EventsInvalid,
    /// [`REPOSITORY_TRIGGER_EVENT_ALL`] named beside another event type, which the
    /// one value standing for every event cannot be: the same
    /// `InvalidRepositoryTriggerEventsException`, worded for the combination
    /// rather than for an event name the service does not recognise.
    EventsAllCombined,
    /// A trigger names more branches than one trigger may:
    /// `MaximumBranchesExceededException`.
    TooManyBranches,
    /// A branch name a repository cannot hold:
    /// `InvalidRepositoryTriggerBranchNameException`.
    BranchNameInvalid,
    /// Custom data past [`MAX_TRIGGER_CUSTOM_DATA_CHARACTERS`], or carrying a
    /// control character: `InvalidRepositoryTriggerCustomDataException`.
    CustomDataInvalid,
}

impl TriggerProblem {
    /// The message the error for this problem carries.
    ///
    /// The text is the one the service answers for that error CLASS, stated here
    /// once so the operations that take a `RepositoryTriggersList` cannot word
    /// the same code differently — a malformed destination ARN carries
    /// [`TRIGGER_DESTINATION_ARN_MALFORMED_MESSAGE`], which is fixed text with no
    /// part of it read off the request. For the problems whose own wording is not
    /// recorded anywhere this crate can read it, the text is the documented
    /// sentence of the error shape the problem names (the `//! ## Errors` list in
    /// `src/ops/put_repository_triggers.rs`); the two bounded problems state the
    /// bound they tripped over, as this crate's other quota refusals do.
    pub fn message(self) -> String {
        match self {
            Self::TooManyTriggers => format!(
                "The number of triggers allowed for the repository was exceeded. The maximum \
                 number of triggers for a repository is {MAX_TRIGGERS_PER_REPOSITORY}."
            ),
            Self::NameMissing => {
                "A name for the trigger is required, but was not specified.".to_string()
            }
            Self::NameInvalid => "The name of the trigger is not valid.".to_string(),
            Self::NameNotUnique => "The name of the trigger is not valid. Trigger names must be \
                                    unique within a repository."
                .to_string(),
            Self::DestinationArnMissing => "A destination ARN for the target service for the \
                                            trigger is required, but was not specified."
                .to_string(),
            Self::DestinationArnInvalid => {
                TRIGGER_DESTINATION_ARN_MALFORMED_MESSAGE.to_string()
            }
            Self::DestinationRegion => {
                "The Amazon Web Services Region for the trigger target does not match the Amazon \
                 Web Services Region for the repository. Triggers must be created in the same \
                 Amazon Web Services Region as the target for the trigger."
                    .to_string()
            }
            Self::EventsMissing => {
                "At least one event for the trigger is required, but was not specified."
                    .to_string()
            }
            Self::EventsInvalid => "One or more events specified for the trigger is not valid. \
                                    Check to make sure that all events specified match the \
                                    requirements for allowed events."
                .to_string(),
            // The two reasons an events list is refused are distinguishable only by
            // this text, so the combination has the service's own sentence for it
            // rather than the one that reports an unrecognised event name. It ends
            // without a period, as the service writes it.
            Self::EventsAllCombined => format!(
                "Repository trigger events cannot contain '{REPOSITORY_TRIGGER_EVENT_ALL}' and \
                 additional event types simultaneously"
            ),
            Self::TooManyBranches => format!(
                "The number of branches for the trigger was exceeded. The maximum number of \
                 branches for a trigger is {MAX_BRANCHES_PER_TRIGGER}."
            ),
            Self::BranchNameInvalid => {
                "One or more branch names specified for the trigger is not valid.".to_string()
            }
            Self::CustomDataInvalid => {
                "The custom data provided for the trigger is not valid.".to_string()
            }
        }
    }
}

/// The Region the ARN of a trigger destination names, or `None` where the value
/// is not the ARN of a service a repository trigger can target.
///
/// An ARN is six colon-separated fields, `arn:<partition>:<vendor>:<region>:
/// <account>:<resource>`; a Lambda function's resource portion carries further
/// colons, which `splitn` leaves in the last field. Region and account are
/// checked for FORM only, and the destination itself is not resolved: which
/// topics and functions exist is not something this service holds.
fn trigger_destination_region(raw: &str) -> Option<&str> {
    let [prefix, partition, vendor, region, account, resource] =
        raw.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return None;
    };
    if prefix != "arn"
        || partition.is_empty()
        // An ARN namespace is a fixed lowercase token, so a value differing in
        // case names another vendor.
        || !TRIGGER_DESTINATION_NAMESPACES.contains(&vendor)
        || region.is_empty()
        || account.len() != ACCOUNT_ID_DIGITS
        || !account.bytes().all(|byte| byte.is_ascii_digit())
        || resource.is_empty()
    {
        return None;
    }
    Some(region)
}

/// What is wrong with a `RepositoryTriggersList` sent for a repository in
/// `repository_region`, or `None` where every trigger in it can be stored.
///
/// The ONE place this crate judges a trigger configuration: `PutRepositoryTriggers`
/// and `TestRepositoryTriggers` take the same list and model the same errors for
/// it, so neither can accept a configuration the other refuses. Each trigger is
/// judged in the order its members are modelled, and the first problem found is
/// the one answered — a configuration is stored whole or not at all.
pub fn trigger_configuration_problem(
    triggers: &[RepositoryTrigger],
    repository_region: &str,
) -> Option<TriggerProblem> {
    if triggers.len() > MAX_TRIGGERS_PER_REPOSITORY {
        return Some(TriggerProblem::TooManyTriggers);
    }
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for trigger in triggers {
        if trigger.name.trim().is_empty() {
            return Some(TriggerProblem::NameMissing);
        }
        if trigger.name.chars().count() > MAX_TRIGGER_NAME_CHARACTERS
            || trigger.name.chars().any(char::is_control)
        {
            return Some(TriggerProblem::NameInvalid);
        }
        if !named.insert(trigger.name.as_str()) {
            return Some(TriggerProblem::NameNotUnique);
        }
        if trigger.destination_arn.trim().is_empty() {
            return Some(TriggerProblem::DestinationArnMissing);
        }
        match trigger_destination_region(&trigger.destination_arn) {
            None => return Some(TriggerProblem::DestinationArnInvalid),
            // "Triggers must be created in the same Amazon Web Services Region
            // as the target for the trigger", which is the Region the repository
            // itself lives in.
            Some(region) if region != repository_region => {
                return Some(TriggerProblem::DestinationRegion);
            }
            Some(_) => {}
        }
        if trigger.events.is_empty() {
            return Some(TriggerProblem::EventsMissing);
        }
        let names_all = trigger
            .events
            .iter()
            .any(|event| event == REPOSITORY_TRIGGER_EVENT_ALL);
        // Each event a trigger names is one of the modelled repository events, and
        // names it ONCE: a list that repeats a value names the same event twice,
        // which also keeps what one trigger stores bounded by the enum's size.
        let mut distinct: BTreeSet<&str> = BTreeSet::new();
        if trigger.events.iter().any(|event| {
            !crate::ops::types::REPOSITORY_TRIGGER_EVENT_ENUM_VALUES.contains(&event.as_str())
                || !distinct.insert(event.as_str())
        }) {
            return Some(TriggerProblem::EventsInvalid);
        }
        // "The valid value \"all\" cannot be used with any other values": a list
        // naming it beside another event type is refused for the COMBINATION, which
        // the service words differently from an event it does not recognise.
        if names_all && trigger.events.len() > 1 {
            return Some(TriggerProblem::EventsAllCombined);
        }
        let branches = trigger.branches.as_deref().unwrap_or_default();
        if branches.len() > MAX_BRANCHES_PER_TRIGGER {
            return Some(TriggerProblem::TooManyBranches);
        }
        // A trigger names branches of the repository it is on, so a name it
        // names is one a repository could hold: git's own reference-name rule,
        // the same one `CreateBranch` holds a name to.
        if branches.iter().any(|branch| !is_valid_branch_name(branch)) {
            return Some(TriggerProblem::BranchNameInvalid);
        }
        if trigger.custom_data.as_ref().is_some_and(|data| {
            data.chars().count() > MAX_TRIGGER_CUSTOM_DATA_CHARACTERS
                || data.chars().any(char::is_control)
        }) {
            return Some(TriggerProblem::CustomDataInvalid);
        }
    }
    None
}

/// A trigger configuration's `configurationId`: the same kind of version-8 UUID
/// as every other id this service mints, over the account, the repository the
/// configuration is on and the instant it was written. A later write of the same
/// repository's configuration therefore mints a different id, which is what
/// "the system-generated unique ID for the create or update operation" is.
pub fn trigger_configuration_id(repository: &str, written: SystemTime) -> String {
    let nanos = nanos_since_epoch(written);
    uuid_v8(&format!(
        "codecommit/{}/{repository}/triggers/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodecommitCodecommit20150413State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodecommitCodecommit20150413State>)
}
