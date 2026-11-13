// SPDX-License-Identifier: CC0-1.0

//! The EventBridge envelope a change to a pull request is PUBLISHED as.
//!
//! CodeCommit does not only record what happened to a pull request for a reader to
//! poll ([`PullRequestRecord::events`]): every such change is also published onto the
//! account's DEFAULT event bus, with no opt-in and no trigger configuration, under the
//! `source` this service owns ([`EVENT_SOURCE`]) and the one `detail-type` a pull
//! request's lifecycle is carried by ([`PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE`]). The
//! envelope names the REPOSITORY's ARN as the single entry of `resources`, so a rule
//! whose pattern pins `resources` to one repository's ARN is delivered that
//! repository's pull-request events and no other repository's; `detail` names the
//! change in `event` and carries the pull request as the API reports it. A client that
//! drives review automation — a reviewer notifier, a status check, an audit trail —
//! therefore observes a pull request being opened, its source branch moving and its
//! status changing without polling CodeCommit again.
//!
//! One envelope per entry in the pull request's own event log, rendered from the
//! STORED pull request and the stored repository ([`PullRequestStateChange::of`]), so
//! every value in it is one a later `GetPullRequest` or `DescribePullRequestEvents`
//! reports and no change can be published that the pull request does not record. The
//! change the entry carries supplies the members that are about the change itself (the
//! tips a creation was opened between, the tip a source-branch move landed on, the
//! status a status change moved to), so each envelope states what happened at the
//! moment it happened.
//!
//! Publication happens only AFTER the write that recorded the change was accepted: a
//! refused call records nothing, so it publishes nothing. It is handed to a task of its
//! own ([`publish_all_detached`]) rather than awaited while the handler still holds
//! this service's store guard, because delivery to a rule's targets is not part of the
//! answer the write gives its caller. EventBridge is reached the way every emulated
//! service reaches another — the AWS SDK over the framework's in-process callback,
//! through the ONE client construction [`::service_common::eventbridge_client`] — so
//! nothing here opens a socket. The only thing the address is built from is the Region
//! the repository lives in, so no part of a caller's request is a destination.
//! Publication is BEST EFFORT: an emulator with no EventBridge mounted has nowhere to
//! publish, so a failure is traced and the operation's own answer stands.
//!
//! The one write path that records a change and publishes none is the git transport's
//! push ([`crate::service::git_http`]): the framework hook it is served through is
//! synchronous and is handed no callback to publish over, so a push's pull-request
//! changes are recorded in the log and left unpublished rather than published from a
//! channel this crate does not have.

use super::*;

/// The `source` of every event CodeCommit publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`,
/// and this service's namespace is the one its ARNs carry
/// (`smithy_query(query="service_info", model_path="services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json")`:
/// `arnNamespace` `codecommit`).
pub const EVENT_SOURCE: &str = "aws.codecommit";

/// The `detail-type` every change to a PULL REQUEST is published under: one type for
/// the whole lifecycle, with the change itself named by `detail.event`.
pub const PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE: &str = "CodeCommit Pull Request State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is
/// the bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// `detail.event` values: the name each kind of recorded change is published under.
///
/// One per variant of [`PullRequestEventDetail`], so a change the pull request's log
/// records is published as the event of its own kind and the mapping cannot drift from
/// the log.
pub const EVENT_PULL_REQUEST_CREATED: &str = "pullRequestCreated";
pub const EVENT_PULL_REQUEST_STATUS_CHANGED: &str = "pullRequestStatusChanged";
pub const EVENT_PULL_REQUEST_SOURCE_BRANCH_UPDATED: &str = "pullRequestSourceBranchUpdated";
pub const EVENT_PULL_REQUEST_MERGE_STATUS_UPDATED: &str = "pullRequestMergeStatusUpdated";
pub const EVENT_PULL_REQUEST_APPROVAL_RULE_CREATED: &str = "pullRequestApprovalRuleCreated";
pub const EVENT_PULL_REQUEST_APPROVAL_RULE_UPDATED: &str = "pullRequestApprovalRuleUpdated";
pub const EVENT_PULL_REQUEST_APPROVAL_RULE_DELETED: &str = "pullRequestApprovalRuleDeleted";
pub const EVENT_PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN: &str = "pullRequestApprovalRuleOverridden";
pub const EVENT_PULL_REQUEST_APPROVAL_STATE_CHANGED: &str = "pullRequestApprovalStateChanged";

impl PullRequestEventDetail {
    /// The `detail.event` this change is published as.
    pub fn published_event(&self) -> &'static str {
        match self {
            Self::Created { .. } => EVENT_PULL_REQUEST_CREATED,
            Self::StatusChanged { .. } => EVENT_PULL_REQUEST_STATUS_CHANGED,
            Self::SourceReferenceUpdated { .. } => EVENT_PULL_REQUEST_SOURCE_BRANCH_UPDATED,
            Self::MergeStateChanged { .. } => EVENT_PULL_REQUEST_MERGE_STATUS_UPDATED,
            Self::ApprovalRuleCreated(_) => EVENT_PULL_REQUEST_APPROVAL_RULE_CREATED,
            Self::ApprovalRuleUpdated(_) => EVENT_PULL_REQUEST_APPROVAL_RULE_UPDATED,
            Self::ApprovalRuleDeleted(_) => EVENT_PULL_REQUEST_APPROVAL_RULE_DELETED,
            Self::ApprovalRuleOverridden { .. } => EVENT_PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN,
            Self::ApprovalStateChanged { .. } => EVENT_PULL_REQUEST_APPROVAL_STATE_CHANGED,
        }
    }
}

/// One pull-request change, rendered from stored state and waiting to be published.
#[derive(Debug, Clone)]
#[must_use = "a recorded pull-request change is published onto the default event bus"]
pub struct PullRequestStateChange {
    /// The repository's ARN, which the envelope carries as the ONE entry of
    /// `resources`: the event names the resource it is about, which is what a rule
    /// filtering on `resources` reads.
    pub repository_arn: String,
    /// The Region the event is published in: the Region the repository lives in.
    pub region: String,
    /// WHEN the pull request changed, in epoch milliseconds — the instant the log
    /// entry carries, not the instant the publication was made.
    pub at: i64,
    /// The rendered `detail` of the event.
    pub detail: ::serde_json::Value,
}

impl PullRequestStateChange {
    /// The envelope for `entry`, one entry of `pull_request`'s log, in `repository`.
    ///
    /// Every member is read off the stored records: which pull request, in which
    /// repository, who opened it and who made this change, the references and tips it
    /// is between, the revision its reviewable state now stands at, and the status it
    /// holds. The members that are about the CHANGE come from the entry itself, so a
    /// creation reports the tips it was opened between, a source-branch move reports
    /// the tip it landed on, and a status change reports the status it moved to,
    /// whatever has happened to the pull request since.
    pub fn of(
        repository: &Repository,
        pull_request: &PullRequestRecord,
        entry: &PullRequestEventRecord,
    ) -> Self {
        let target = pull_request.targets.first();
        let mut detail = ::serde_json::Map::new();
        let mut put = |member: &str, value: &str| {
            detail.insert(
                member.to_string(),
                ::serde_json::Value::String(value.to_string()),
            );
        };
        put("event", entry.detail.published_event());
        put("pullRequestId", &pull_request.pull_request_id.to_string());
        put("title", &pull_request.title);
        put("author", &pull_request.author_arn);
        put("pullRequestStatus", &pull_request.status);
        put("revisionId", &pull_request.revision_id());
        if let Some(actor) = entry.actor_arn.as_deref() {
            put("callerUserArn", actor);
        }
        if let Some(target) = target {
            put("sourceReference", &target.source_reference);
            put("destinationReference", &target.destination_reference);
            put("sourceCommit", &target.source_commit);
            put("destinationCommit", &target.destination_commit);
            if let Some(merge_base) = target.merge_base.as_deref() {
                put("mergeBase", merge_base);
            }
        }
        // What the change ITSELF says, over what the pull request now stands at.
        match &entry.detail {
            PullRequestEventDetail::Created {
                source_commit_id,
                destination_commit_id,
                merge_base,
                ..
            } => {
                put("sourceCommit", source_commit_id);
                put("destinationCommit", destination_commit_id);
                if let Some(merge_base) = merge_base.as_deref() {
                    put("mergeBase", merge_base);
                }
            }
            PullRequestEventDetail::SourceReferenceUpdated {
                before_commit_id,
                after_commit_id,
                merge_base,
                ..
            } => {
                put("beforeCommitId", before_commit_id);
                put("sourceCommit", after_commit_id);
                if let Some(merge_base) = merge_base.as_deref() {
                    put("mergeBase", merge_base);
                }
            }
            PullRequestEventDetail::StatusChanged { status } => {
                put("pullRequestStatus", status);
            }
            PullRequestEventDetail::MergeStateChanged {
                destination_reference,
                merge_commit_id,
                merge_option,
                ..
            } => {
                put("destinationReference", destination_reference);
                if let Some(merge_commit_id) = merge_commit_id.as_deref() {
                    put("mergeCommitId", merge_commit_id);
                }
                if let Some(merge_option) = merge_option.as_deref() {
                    put("mergeOption", merge_option);
                }
            }
            PullRequestEventDetail::ApprovalRuleCreated(rule)
            | PullRequestEventDetail::ApprovalRuleUpdated(rule)
            | PullRequestEventDetail::ApprovalRuleDeleted(rule) => {
                put("approvalRuleId", &rule.approval_rule_id);
                put("approvalRuleName", &rule.approval_rule_name);
            }
            PullRequestEventDetail::ApprovalRuleOverridden { override_status, .. } => {
                put("overrideStatus", override_status);
            }
            PullRequestEventDetail::ApprovalStateChanged {
                approval_status, ..
            } => {
                put("approvalStatus", approval_status);
            }
        }
        // The repository names the change is in, as a LIST: a pull request is between
        // targets, and a consumer filters on the name being among them.
        let repositories: Vec<::serde_json::Value> = pull_request
            .targets
            .iter()
            .map(|target| ::serde_json::Value::String(target.repository_name.clone()))
            .collect();
        detail.insert(
            "repositoryNames".to_string(),
            ::serde_json::Value::Array(repositories),
        );
        Self {
            repository_arn: repository.arn.clone(),
            region: repository.region.clone(),
            at: to_date_time(entry.event_date)
                .to_millis()
                .unwrap_or_default(),
            detail: ::serde_json::Value::Object(detail),
        }
    }

    /// Publish this change onto the account's default event bus in the repository's
    /// Region.
    ///
    /// Call it with NO store guard held. Nothing about the answer changes what the
    /// caller of the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &::smithy_server_runtime::EmbeddedEmulator) {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EVENT_SOURCE)
            .detail_type(PULL_REQUEST_STATE_CHANGE_DETAIL_TYPE)
            // The event's `time` is WHEN the pull request changed, not when it was
            // published: `PutEventsRequestEntry$Time` is "the time stamp of the event,
            // per RFC3339. If no time stamp is provided, the time stamp of the
            // PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at,
            ))
            .resources(&self.repository_arn)
            .detail(self.detail.to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    repository = %self.repository_arn,
                    "a CodeCommit pull-request event was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    repository = %self.repository_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a CodeCommit pull-request event could not be published"
                );
            }
        }
    }
}

/// Publish `changes` in the order they were recorded.
///
/// The one place a handler hands over what its write recorded, so no caller of it
/// orders the publication of two changes of one call differently.
pub async fn publish_all(
    changes: Vec<PullRequestStateChange>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    for change in changes {
        change.publish(callback).await;
    }
}

/// How many pull-request publications run at once, over the whole store.
///
/// A write that records a change starts one publication, so a caller driving writes
/// drives tasks: this permit is what keeps the work in flight bounded however many
/// writes arrive, exactly as the rest of this crate bounds what one request can grow.
/// The task waits for a permit rather than dropping the event, because a change the
/// pull request records is a change a consumer is owed.
const CONCURRENT_PUBLICATIONS: usize = 16;

/// The permits those publications take, one per publication in flight.
static PUBLICATIONS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(CONCURRENT_PUBLICATIONS);

/// The same publication, on a task of its OWN, for a handler that still holds this
/// service's store guard.
///
/// EventBridge delivery is not part of the answer the caller of the write is given —
/// the event reaches a rule's targets after the call that caused it, which is what a
/// consumer polling for it observes — so the publication is handed to its own task
/// rather than awaited under the guard: a guard held across an await would make a
/// handler's future non-`Send`, and the write's answer waits for nothing that is not
/// part of it. Nothing to publish starts no task, and a task that starts takes one of
/// [`CONCURRENT_PUBLICATIONS`] permits before it publishes.
pub fn publish_all_detached(
    changes: Vec<PullRequestStateChange>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    if changes.is_empty() {
        return;
    }
    let callback = callback.clone();
    ::tokio::spawn(async move {
        // One permit per publication in flight; a closed semaphore is not something
        // this process creates, so a permit that cannot be taken leaves the events
        // untouched rather than published twice.
        let Ok(_permit) = PUBLICATIONS.acquire().await else {
            return;
        };
        publish_all(changes, &callback).await;
    });
}

impl Repository {
    /// The envelopes for the `newest` most recently recorded changes to the pull
    /// request `pull_request_id` names in this repository, OLDEST FIRST.
    ///
    /// What a handler that has just recorded its changes hands to [`publish_all`]: the
    /// changes are read back out of the pull request's own log, so what is published
    /// is what the log records. An id this repository does not hold, and a request for
    /// no changes, render nothing.
    #[must_use = "a recorded pull-request change is published onto the default event bus"]
    pub fn pull_request_changes(
        &self,
        pull_request_id: u64,
        newest: usize,
    ) -> Vec<PullRequestStateChange> {
        let Some(pull_request) = self.pull_requests.get(&pull_request_id) else {
            return Vec::new();
        };
        let held = pull_request.events.len();
        pull_request.events[held.saturating_sub(newest)..]
            .iter()
            .map(|entry| PullRequestStateChange::of(self, pull_request, entry))
            .collect()
    }
}

impl ComAmazonawsCodecommitCodecommit20150413State {
    /// The same envelopes, for a handler that reached the pull request by id alone
    /// ([`Self::pull_request_mut`]) and so does not name the repository holding it.
    #[must_use = "a recorded pull-request change is published onto the default event bus"]
    pub fn pull_request_changes(
        &self,
        pull_request_id: u64,
        newest: usize,
    ) -> Vec<PullRequestStateChange> {
        self.repositories
            .values()
            .find(|repository| repository.pull_requests.contains_key(&pull_request_id))
            .map(|repository| repository.pull_request_changes(pull_request_id, newest))
            .unwrap_or_default()
    }
}
