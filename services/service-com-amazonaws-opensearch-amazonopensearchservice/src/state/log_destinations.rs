// SPDX-License-Identifier: CC0-1.0

//! Whether the destination an enabled log-publishing option names is one the service can
//! really publish to — the ONE statement of that rule, so every call that takes a
//! `LogPublishingOptions` map judges it alike.
//!
//! A `LogPublishingOption` is "whether the log should be published" plus "the ARN of the
//! CloudWatch Logs group to publish logs to"
//! (`smithy_query(query="shape",shape="LogPublishingOption")`). Two things about that group
//! decide whether the delivery it describes can ever happen, and neither is a constraint
//! the model can carry — the member is a string — so both are checked here, in this order:
//!
//! 1. the group EXISTS: an option enabled against a log group nobody created publishes to
//!    nothing ([`nonexistent_destination`]);
//! 2. the group LETS THIS SERVICE WRITE TO IT: a domain's logs are delivered by the service
//!    itself, under its own service principal ([`SERVICE_PRINCIPAL`]), so the records only
//!    arrive where that principal may open a stream and put events ([`DELIVERY_ACTIONS`]) —
//!    which is what the group's CloudWatch Logs resource policy grants or withholds
//!    ([`ungranted_destination`]).
//!
//! Either way the request is refused with the operation's `ValidationException` and nothing
//! is stored. [`super::log_publishing_refusal`] is the step before both: an option that
//! names no log group at all.
//!
//! The group is resolved in the CloudWatch Logs service's own store, reached through the
//! SAME shared engine this service's handlers reach their own store through — the store
//! `logs:CreateLogGroup` writes and `logs:DescribeLogGroups` reports, and the store
//! `logs:PutResourcePolicy` attaches a policy in — the way
//! [`super::cognito_authentication`] resolves a user pool. That store belongs to the
//! ENGINE rather than to a mounted service, so which services the emulator happens to
//! mount does not change what this check resolves.
//!
//! What a caller SEES for a destination that does not exist is the service's own fixed
//! sentence for a log group it cannot resolve. That sentence carries no part of the request,
//! because WHICH group is missing is not what it states — it states that the destination the
//! request named does not exist, which is the one thing a caller has to act on. The refusal
//! for a group that IS there and grants nothing names that group instead, because the policy
//! to add is the policy OF one group.
//!
//! The policy is evaluated by the shared IAM evaluation
//! (`service_common::iam_authorization`), so this service and every other one handed the
//! same document decide alike, and a grant is only read as WITHHELD where the documents say
//! so ([`withholds_delivery`]).

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as cloudwatch_logs;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::{LogPublishingOption, ValidationException};

/// The suffix the WILDCARD form of a log group's ARN carries (`LogGroup$arn`, which
/// names the group's log streams); the plain form (`LogGroup$logGroupArn`) carries none.
/// A caller may hand over either, so the suffix is dropped before the ARN is resolved.
const LOG_GROUP_STREAMS_SUFFIX: &str = ":*";

/// The log group an option publishes to, when its `CloudWatchLogsLogGroupArn` names one:
/// the ARN without the wildcard suffix either form may carry, and the group's own name.
///
/// WHICH values name a log group, and WHICH part of one is the group's name, are the
/// CloudWatch Logs service's own readings of its own ARN — so they are read with its own
/// helpers rather than parsed a second time here, and the two crates cannot drift on what
/// counts as a log group's ARN.
///
/// `None` for a value that is not the ARN of a CloudWatch Logs log group — an option
/// carrying one of those names no destination at all, which is
/// [`super::log_publishing_refusal`]'s refusal rather than this module's.
///
/// `pub(crate)` because the DELIVERY into that group reads the destination the same way
/// ([`super::log_publishing`]): one reading of what an option's ARN names, so what the
/// create resolved is what a record is published to.
pub(crate) fn named_log_group(arn: &str) -> Option<(&str, &str)> {
    if !cloudwatch_logs::is_log_group_arn(arn) {
        return None;
    }
    let name = cloudwatch_logs::log_group_name_from_identifier(arn);
    let arn = arn.strip_suffix(LOG_GROUP_STREAMS_SUFFIX).unwrap_or(arn);
    // A group's name is what identifies it, so an ARN carrying none names no group.
    (!name.is_empty()).then_some((arn, name))
}

/// Whether `arn` names a CloudWatch Logs log group AT ALL — the shape half of the rule,
/// so the one place that reads a destination ARN is this module.
///
/// A value that is not a log group's ARN names no destination, which is the refusal
/// [`super::log_publishing_refusal`] answers an option publishing to nowhere with.
pub fn names_a_log_group(arn: &str) -> bool {
    named_log_group(arn).is_some()
}

/// What the service answers an enabled option whose destination does not exist.
///
/// A log group nobody created is no destination at all, and the service says so in a fixed
/// sentence: the refusal states the CONDITION that held and carries no value of the request
/// — not the group's name, not the log type — so it reads the same for every destination
/// that resolves to nothing. It is the refusal for a group this account and Region does not
/// hold, whichever field of the ARN it is that misses them.
pub fn nonexistent_destination() -> ValidationException {
    ValidationException {
        message: Some("The specified CloudWatch Logs log group doesn't exist.".to_string()),
    }
}

/// The service principal Amazon OpenSearch Service publishes a domain's logs under, and so
/// the principal a destination log group's resource policy has to grant: the records are
/// delivered BY the service rather than by the caller that configured the publishing, which
/// is why the group's own policy is what admits them.
pub const SERVICE_PRINCIPAL: &str = "es.amazonaws.com";

/// What the service does to a destination group when it publishes a log type to it: it
/// opens the stream a domain's node writes its records to, and it writes them. Both are
/// needed, so a policy granting only one of them lets no record arrive.
pub const DELIVERY_ACTIONS: [&str; 2] = ["logs:CreateLogStream", "logs:PutLogEvents"];

/// What the service answers an enabled option whose destination exists but grants the
/// service nothing: the sentence states the condition that held — the group's resource
/// access policy does not permit the service to create a log stream in it — and its one
/// variable part is the NAME of the group that policy belongs to, which is the thing a
/// caller has to act on.
pub fn ungranted_destination(log_group_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "The Resource Access Policy specified for the CloudWatch Logs log group \
             {log_group_name} does not grant sufficient permissions for Amazon OpenSearch \
             Service to create a log stream. Please check the Resource Access Policy."
        )),
    }
}

/// Whether the resource policies `documents` WITHHOLD `action` on the log group `arn` from
/// [`SERVICE_PRINCIPAL`] — the one reading a refusal of a destination is grounded in.
///
/// A grant may be written on the GROUP or on the group's log streams (the ARN's wildcard
/// form, which is the resource a `logs:CreateLogStream` is taken on), so both spellings are
/// asked and either one granting the action is a grant of it.
///
/// An explicit `Deny` withholds the action, and so does a set of documents that states
/// nothing about it at all: a policy silent about this principal grants it nothing, which is
/// exactly the case the service refuses. A statement that IS about the access and hinges on
/// a condition key this emulator carries no value for leaves the grant undecided, and an
/// undecided grant is no evidence that the real service would refuse the request — so
/// nothing is withheld on it.
fn withholds_delivery(documents: &[iam::PreparedDocument], action: &str, arn: &str) -> bool {
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let streams = format!("{arn}{LOG_GROUP_STREAMS_SUFFIX}");
    let mut allowed = false;
    let mut undecided = false;
    for resource in [arn, streams.as_str()] {
        let request = iam::Request::new(action, resource, &principals);
        for document in documents {
            let outcome = iam::resource_outcome_prepared(document, &request);
            match outcome.decision {
                Decision::Allowed => allowed = true,
                Decision::Denied => return true,
                Decision::NotStated => {
                    undecided = undecided || !outcome.required_context_keys.is_empty();
                }
            }
        }
    }
    !allowed && !undecided
}

/// Whether every enabled option in `options` publishes to a log group that EXISTS and that
/// lets this service deliver into it, and the refusal for the first destination that does
/// not otherwise ([`nonexistent_destination`], [`ungranted_destination`]).
///
/// EXISTENCE is decided for every destination first, so a request naming one group that is
/// not there and another that grants nothing is answered with the existence sentence: a
/// group nobody created is the earlier fault, and telling such a caller to attach a policy
/// would name a remedy that cannot be applied.
///
/// The log types are visited in order, so one request with two unresolvable destinations
/// is refused with the same sentence however the map was iterated. A DISABLED option
/// publishes nothing, so the destination it carries is not resolved — nothing would be
/// delivered to it.
///
/// `None` when the engine hands out no CloudWatch Logs store: a caller that cannot reach
/// the store cannot tell a missing group from a missing store, and refusing a
/// configuration on the strength of a store that was never readable would reject a
/// destination that does exist.
pub fn log_publishing_destination_refusal<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    options: impl IntoIterator<Item = (&'a String, &'a LogPublishingOption)>,
) -> Option<ValidationException> {
    let mut enabled: Vec<(&str, &str)> = options
        .into_iter()
        .filter(|(_, option)| option.enabled.unwrap_or(false))
        .filter_map(|(log_type, option)| {
            let arn = option.cloud_watch_logs_log_group_arn.as_deref()?;
            Some((log_type.as_str(), arn))
        })
        .collect();
    if enabled.is_empty() {
        return None;
    }
    enabled.sort_unstable();
    // The destinations the enabled options name, in log-type order: each one the ARN of the
    // group it publishes to and that group's own name. A value that is not a log group's
    // ARN is the other refusal's ([`names_a_log_group`]), so it is passed over here rather
    // than reported as a group that does not exist.
    let destinations: Vec<(&str, &str)> = enabled
        .iter()
        .filter_map(|(_, arn)| named_log_group(arn))
        .collect();
    let store = cloudwatch_logs::service_state(engine)?;
    let logs = store.lock();
    for (arn, name) in &destinations {
        // The group is resolved by the name it is stored under, and the record's own ARN
        // is what says WHICH group that is: a destination in another account, another
        // partition or another Region names a group this account does not hold here,
        // whatever a group of that name is called locally.
        if logs
            .log_groups
            .get(*name)
            .is_none_or(|group| group.log_group_arn != *arn)
        {
            return Some(nonexistent_destination());
        }
    }
    for (arn, name) in &destinations {
        let Some(group) = logs.log_groups.get(*name) else {
            continue;
        };
        // The policies in force for the group: the one attached to it, and the
        // account-scoped ones, which cover the log groups their own statements name. Read
        // once each, since every action is decided against the same set.
        let documents: Vec<iam::PreparedDocument> = group
            .resource_policy
            .iter()
            .map(|policy| policy.policy_document.as_str())
            .chain(
                logs.account_resource_policies
                    .values()
                    .map(|policy| policy.policy_document.as_str()),
            )
            .map(iam::PreparedDocument::read)
            .collect();
        if DELIVERY_ACTIONS
            .iter()
            .any(|action| withholds_delivery(&documents, action, arn))
        {
            return Some(ungranted_destination(name));
        }
    }
    None
}
