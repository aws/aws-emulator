// SPDX-License-Identifier: CC0-1.0

//! Delivering a matched event to a CLOUDWATCH LOGS LOG GROUP target: the log
//! event a rule writes into the group, what the documents in force over that
//! group admit of the write, and how a write the group would not take is
//! classified and retried.
//!
//! It is one of the target kinds whose delivery is RETRIED here — a log group
//! that does not hold the event YET may hold it later, and "retry attempts
//! continue until either the maximum number of attempts is made or until the
//! duration of the `MaximumEventAgeInSeconds` is met"
//! ([`crate::state::retry_bounds`] decides which of the two ends a delivery) —
//! so it carries the retry bookkeeping the dead letter reports, which is why it
//! sits beside the rest of [`super`] rather than inside it.

use super::*;

/// Deliver one routed event to the CloudWatch Logs log group `arn` names: write
/// ONE log event into it, carrying what the target receives.
///
/// The rule AWS follows: a rule target whose `Arn` names a log group is
/// delivered to by WRITING the event into that group — one log event per matched
/// event — rather than by calling an API of it, and the write is
/// `logs:PutLogEvents` into a stream of that group made as this service's own
/// principal, so the documents in force over the GROUP are what admit it
/// ([`log_group_grants`]). What the log event CARRIES is the text the
/// target receives, read as the log record it is
/// ([`crate::state::log_group_target::log_record`]): the whole envelope under the
/// envelope's own instant for a target taking the event as published, and the
/// `timestamp` and `message` a reshaped input names for one that asked for
/// something else.
///
/// The write is made of TWO calls, each needing its own grant: a log event can
/// only be put into a stream that exists, so the stream is opened first. The two
/// are decided separately, because they fail differently — a delivery whose put
/// is not admitted writes nothing at all, while one whose stream OPEN is not
/// admitted opens no stream and is left to the put, which succeeds into a stream
/// the group already holds and fails for one it does not. A stream already open
/// is the ordinary answer for the second and every later delivery, so that
/// refusal alone is not a failed delivery. The stream is named for the RULE whose
/// delivery writes it: no recorded answer states the name AWS gives it, and
/// naming it after the rule keeps every delivery of one rule in one stream, in
/// the order the deliveries were made, where a reader of the group sees them.
///
/// A write the group would not take is reported in EventBridge's OWN vocabulary
/// ([`rejected_write`]), never in the shape name or the sentence CloudWatch Logs
/// answered it with, and one the group has not taken YET is retried first
/// ([`crate::state::retry_bounds`]) — the attempts and the retries the delivery
/// made, reported in `spent`.
///
/// `stores` is where the ROLE the target names is read from, because each of the
/// two calls the write is made of is granted by that role on its own (see
/// [`log_group_grants`]).
#[allow(clippy::too_many_arguments)]
pub(super) async fn deliver_log_event(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    stores: &Stores<'_>,
    region: &str,
    arn: &str,
    action: &str,
    handover: &Handover<'_>,
    body: &str,
    spent: &mut Spent,
) -> crate::state::InvocationOutcome {
    let Some(group) = crate::state::log_group_target::log_group_name(arn) else {
        return crate::state::InvocationOutcome::NotAttempted(
            "the target ARN names no log group".to_string(),
        );
    };
    let reshaped = handover.target.reshapes();
    let record = match crate::state::log_group_target::log_record(body, reshaped) {
        Ok(record) => record,
        // The text the target asked for is no log event, so the put this
        // delivery would make is one CloudWatch Logs will not take — and will
        // not take however often it is made, since nothing about the text
        // changes: the delivery was attempted and wrote nothing, which is a
        // FAILED invocation rather than one nobody made, and it is reported as
        // the putLogEvents call that could not be completed
        // ([`rejected_write`]). The detail of WHAT was missing is not a sentence
        // the service words, so it goes where this emulator's own detail goes.
        Err(reason) => {
            ::tracing::warn!(
                log_group = %arn,
                rule = %handover.rule,
                detail = %reason,
                "an EventBridge delivery wrote no log event: the text the target receives is \
                 no log record"
            );
            return rejected_write(Refused::ForGood, group);
        }
    };
    let event = match ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
        .timestamp(record.timestamp)
        .message(&record.message)
        .build()
    {
        Ok(event) => event,
        Err(error) => {
            return crate::state::InvocationOutcome::NotAttempted(error.to_string());
        }
    };
    // One client for every call this delivery makes on the group: the handle is a
    // copy over the shared in-process transport, for the reason the queue branch
    // of [`TargetKind::invoke`] gives.
    let client = clients.logs(callback, region).clone();
    let stream = handover.rule;
    let stream_arn = crate::state::log_group_target::log_stream_arn(arn, stream);
    let grants = log_group_grants(&client, stores, arn, &stream_arn, handover).await;
    if !grants.write {
        ::tracing::warn!(
            log_group = %arn,
            rule = %handover.rule,
            "an EventBridge delivery was withheld: no resource policy in force over the log \
             group allows this service to write to it"
        );
        let (code, message) = log_group_write_refusal(&stream_arn, action);
        return crate::state::InvocationOutcome::Failed { code, message };
    }
    if grants.open {
        let opened = client
            .create_log_stream()
            .log_group_name(group)
            .log_stream_name(stream)
            .send()
            .await;
        if let Err(error) = &opened {
            let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error);
            if code != Some(LOG_STREAM_ALREADY_OPEN_CODE) {
                return outcome(opened);
            }
        }
    }
    // The put is RETRIED while the group might still take the event: a log group
    // does not hold an event whose instant is ahead of the window it ingests, and
    // the clock reaching that instant is what makes the same put succeed — so the
    // bounds the target's own `RetryPolicy` puts on a retried delivery decide how
    // long this one goes on ([`crate::state::retry_bounds`]).
    let bounds = crate::state::retry_bounds::RetryBounds::of(
        handover.target.maximum_retry_attempts,
        handover.target.maximum_event_age_in_seconds,
    );
    // How old the EVENT already is where this delivery begins, which is what the
    // age bound is read against, as the api-destination branch reads it.
    let age = ::std::time::SystemTime::now()
        .duration_since(handover.published)
        .unwrap_or_default();
    // How many attempts were made; one more than the retries, which is what the
    // retry bounds are counted in.
    let mut attempted: i32 = 0;
    // WHICH of the target's bounds ended the delivery, left `None` by a bound of
    // this emulator's own: a condition the target's retry policy did not produce
    // must not be reported as one it did.
    let mut exhausted: Option<&'static str> = None;
    loop {
        // One yield per attempt, so a retried delivery stays interruptible.
        ::tokio::task::yield_now().await;
        attempted += 1;
        // Reported as the attempt is MADE, so the rule's attempt counter counts
        // the attempts this delivery really made whatever it ends as
        // ([`super::count_invocation`]).
        spent.attempts = attempted;
        let answered = client
            .put_log_events()
            .log_group_name(group)
            .log_stream_name(stream)
            .log_events(event.clone())
            .send()
            .await;
        let answer = match answered {
            Ok(answer) => answer,
            // The service ANSWERED a fault, which is a refusal of the CALL
            // rather than of the event's instant: classified into the condition
            // EventBridge reports for it and carrying that answer, the way every
            // target service's own answer is ([`outcome`]).
            Err(error) => return outcome::<(), _>(Err(error)),
        };
        // A put is answered with "the rejected events" when CloudWatch Logs would
        // not hold one of them (`PutLogEvents$rejectedLogEventsInfo`). This
        // delivery carries ONE event, so a rejection naming it is a delivery that
        // wrote nothing — reported as the failure it is rather than as the 200 the
        // call came back with.
        let Some(refused) = window_refusal(answer.rejected_log_events_info()) else {
            return crate::state::InvocationOutcome::Invoked;
        };
        if refused == Refused::ForGood {
            // No later put can land on an instant the group has already left
            // behind, so the delivery fails on this attempt. Retry accounting is
            // reported only for a delivery that was RETRIED: a refusal on the
            // first attempt retried nothing.
            spent.retries = (attempted > 1).then(|| Retries {
                attempts: attempted - 1,
                exhausted: None,
            });
            return rejected_write(refused, group);
        }
        // Both of the target's bounds are asked around EVERY retryable failure,
        // so the delivery stops on whichever is met first and the dead letter
        // names it.
        let made = attempted - 1;
        if let Some(condition) = bounds.exhausted_after(made, age) {
            exhausted = Some(condition);
            break;
        }
        // This emulator's own bound on the real calls one delivery makes, for the
        // reason the api-destination branch gives: it is derived from the widest
        // age bound the model admits, so a target within the model is stopped by
        // one of its own bounds above and this is the guard rather than the rule.
        if made >= crate::state::retry_bounds::MAX_RETRIES_PER_DELIVERY {
            ::tracing::warn!(
                log_group = %arn,
                bound = crate::state::retry_bounds::MAX_RETRIES_PER_DELIVERY,
                "this delivery reached the retries one makes in this emulator; neither bound of \
                 the target's retry policy was met"
            );
            break;
        }
    }
    spent.retries = Some(Retries {
        attempts: (attempted - 1).max(0),
        exhausted,
    });
    rejected_write(Refused::NotYet, group)
}

/// What EventBridge reports for a log-group delivery whose `PutLogEvents` wrote
/// no log event, by whether the group could ever have taken it.
///
/// The condition is EventBridge's own classification and never the shape name of
/// the answer the write came back with ([`delivery_condition`] states the same
/// rule for the answers a target's service makes): a write the group refused for
/// good is a refusal of the event no later attempt changes, which is the
/// condition a call the service would not take the parameters of carries
/// ([`crate::state::INVALID_PARAMETER`]) — while one it has not taken YET is a
/// batch whose events were rejected by a call that otherwise succeeded, retried
/// first and reported as
/// [`crate::state::EVENTS_IN_BATCH_REQUEST_REJECTED`].
///
/// The sentence is the one EventBridge words for the call it could not complete
/// ([`put_log_events_failure`]), the same for both.
fn rejected_write(refused: Refused, group: &str) -> crate::state::InvocationOutcome {
    let code = match refused {
        Refused::ForGood => crate::state::INVALID_PARAMETER,
        Refused::NotYet => crate::state::EVENTS_IN_BATCH_REQUEST_REJECTED,
    };
    crate::state::InvocationOutcome::Failed {
        code: code.to_string(),
        message: put_log_events_failure(group),
    }
}

/// The sentence EventBridge reports for a log-group target whose `PutLogEvents`
/// wrote no log event.
///
/// AWS's own wording, whose fixed text is "Could not complete putLogEvents call
/// for " and the trailing "."; the one variable part is the NAME of the log group
/// the target's ARN names ([`crate::state::log_group_target::log_group_name`]), so
/// a consumer of a dead-letter queue reads which group refused the write back out
/// of the message.
fn put_log_events_failure(group: &str) -> String {
    format!("Could not complete putLogEvents call for {group}.")
}

/// Whether a write the log group would not take is one a LATER attempt could
/// still make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// For good: the group will not hold this event however often the put is
    /// made — an instant older than the window it ingests ("events older than 14
    /// days or preceding the log group's retention period are rejected"), which
    /// only gets older, or text that is no log event at all.
    ForGood,
    /// Not yet: the event's instant is further in the future than the window
    /// reaches ("events more than 2 hours in the future are rejected"), and the
    /// clock reaching it is what makes the same put land — so the delivery is
    /// retried.
    NotYet,
}

/// How the ONE event this delivery put was refused, or `None` for a put the group
/// took.
///
/// The members report runs by index — events BEFORE
/// `tooOldLogEventEndIndex`/`expiredLogEventEndIndex` and from
/// `tooNewLogEventStartIndex` on were not held — so for a batch of one they all
/// speak about index zero. A report naming the event in both runs at once is read
/// as [`Refused::ForGood`]: the refusal that no later attempt clears is the one
/// that decides the delivery.
pub(super) fn window_refusal(
    rejected: Option<&::aws_sdk_cloudwatchlogs::types::RejectedLogEventsInfo>,
) -> Option<Refused> {
    let rejected = rejected?;
    let for_good = rejected
        .too_old_log_event_end_index()
        .is_some_and(|end| end > 0)
        || rejected
            .expired_log_event_end_index()
            .is_some_and(|end| end > 0);
    if for_good {
        return Some(Refused::ForGood);
    }
    rejected
        .too_new_log_event_start_index()
        .is_some_and(|start| start == 0)
        .then_some(Refused::NotYet)
}

/// The error code CloudWatch Logs answers a create of a stream the group already
/// holds, which for a group a rule has delivered to before is the ordinary answer
/// rather than a failure.
pub(super) const LOG_STREAM_ALREADY_OPEN_CODE: &str = "ResourceAlreadyExistsException";

/// What the documents in force over the log group `group_arn` names admit of the
/// write this delivery makes: opening the stream, and putting the event.
///
/// The SAME rule the queue, topic and function branches follow
/// ([`queue_delivery_refusal`]): EventBridge writes into a log group as its own
/// service principal [`crate::state::SERVICE_PRINCIPAL`], which holds no
/// identity-based policies, so the documents in force over the GROUP are the only
/// ones over this delivery and what they state decides it
/// ([`crate::state::log_group_target::documents_decision`]) — nothing is written
/// without a grant of
/// [`crate::state::log_group_target::PUT_LOG_EVENTS_ACTION`] to that principal on
/// the group's streams, an explicit deny in ANY of those documents refuses the
/// write whatever another grants, and a grant scoped by `aws:SourceArn` admits
/// the deliveries of the rule it names and no other. A group carrying no policy,
/// and one this account does not hold at all, are in the same position: no
/// document admits the write, so nothing is written and nothing reaches the
/// group's API.
///
/// The two calls the write is made of are decided SEPARATELY, because a document
/// may grant one and withhold the other
/// ([`crate::state::log_group_target::CREATE_LOG_STREAM_ACTION`] for the open),
/// and because they then fail differently (see [`deliver_log_event`]). Each is
/// decided on the group's STREAM
/// ([`crate::state::log_group_target::log_stream_arn`]), which is the resource
/// CloudWatch Logs names for the write, so a grant written over the group with
/// everything under it and one written over its streams — the two forms a client
/// writes for a logs delivery — both admit it. Each is also decided against the
/// ROLE the target names, which can grant that one call on its own
/// ([`super::role_grants_call`]): EventBridge writes into a group as its own
/// service principal when the target names no role and as that role when it names
/// one, so the role is asked per CALL rather than once for the delivery — a role
/// allowed the write and not the open opens no stream.
///
/// The documents are read through the group's own `DescribeResourcePolicies`, so
/// what decides a delivery is what a client's last `PutResourcePolicy` left: both
/// the policy attached to THAT group and the account's own are in force over it.
/// A read that does not ANSWER decides nothing here, so both calls are left to be
/// made and what the group's own API answers is what the delivery reports — the
/// reason [`queue_policy`] gives.
pub(super) async fn log_group_grants(
    client: &::aws_sdk_cloudwatchlogs::Client,
    stores: &Stores<'_>,
    group_arn: &str,
    stream_arn: &str,
    handover: &Handover<'_>,
) -> LogGroupGrants {
    let Some(documents) = log_group_policies(client, group_arn).await else {
        return LogGroupGrants {
            open: true,
            write: true,
        };
    };
    let admits = |action: &'static str| {
        let attempt = crate::state::DeliveryAttempt {
            rule: handover.rule,
            target_id: &handover.target.id,
            action,
            resource: stream_arn,
        };
        match crate::state::log_group_target::documents_decision(
            &documents,
            &attempt,
            handover.rule_arn,
        ) {
            ::service_common::iam_authorization::Decision::Allowed => true,
            // An explicit deny in a document in force over the group refuses the
            // call whatever the role the target names allows.
            ::service_common::iam_authorization::Decision::Denied => false,
            // Nothing in force over the group is about this call, so what can
            // grant it is the role the delivery is made as.
            ::service_common::iam_authorization::Decision::NotStated => super::role_grants_call(
                stores,
                handover.target.role_arn.as_deref(),
                handover,
                &attempt,
            ),
        }
    };
    LogGroupGrants {
        open: admits(crate::state::log_group_target::CREATE_LOG_STREAM_ACTION),
        write: admits(crate::state::log_group_target::PUT_LOG_EVENTS_ACTION),
    }
}

/// Which of the two calls a log-group delivery is made of the documents in force
/// over that group admit.
pub(super) struct LogGroupGrants {
    /// `logs:CreateLogStream` on the group's streams: the open of the stream the
    /// event is written into.
    pub(super) open: bool,
    /// `logs:PutLogEvents` on them: the write itself.
    pub(super) write: bool,
}

/// Every resource-policy document in force over the log group `group_arn` names:
/// the one attached to that group and the account's own — `None` when the service
/// did not answer either read at all, which is a delivery nothing is known about
/// rather than one no document admits.
pub(super) async fn log_group_policies(
    client: &::aws_sdk_cloudwatchlogs::Client,
    group_arn: &str,
) -> Option<Vec<String>> {
    // The group's OWN policy, which is what a `PutResourcePolicy` naming a
    // `resourceArn` leaves, and then the account's — "when not specified,
    // defaults to ACCOUNT" (`DescribeResourcePolicies$policyScope`), which is
    // what a put naming no resource leaves.
    let attached = client
        .describe_resource_policies()
        .resource_arn(group_arn)
        .send()
        .await;
    let account_wide = client.describe_resource_policies().send().await;
    let mut documents: Vec<String> = Vec::new();
    for answered in [attached, account_wide] {
        match answered {
            Ok(answer) => documents.extend(
                answer
                    .resource_policies()
                    .iter()
                    .filter_map(|policy| policy.policy_document().map(ToString::to_string)),
            ),
            Err(error) => {
                ::tracing::warn!(
                    log_group = %group_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the resource policies in force over an EventBridge target log group could \
                     not be read"
                );
                return None;
            }
        }
    }
    Some(documents)
}

/// The service name Amazon CloudWatch Logs' own error metadata carries.
pub(super) const LOGS_SERVICE_NAME: &str = "AWSLogs";

/// The error code CloudWatch Logs answers a caller its resource policies do not
/// permit, and the status that answer carries:
/// `smithy_query(query="shape", shape="AccessDeniedException",
/// model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json")`
/// — "you don't have sufficient permissions to perform this action", `httpError`
/// 400.
pub(super) const LOGS_ACCESS_DENIED_CODE: &str = "AccessDeniedException";
pub(super) const LOGS_ACCESS_DENIED_STATUS: u16 = 400;

/// The condition and the sentence EventBridge reports for a log-group target it
/// could not write to, because no document in force over that group granted this
/// service the write.
///
/// What the delivery SEES is CloudWatch Logs' own refusal of the
/// `logs:PutLogEvents`: the authorization sentence naming the principal the call
/// was made as, the action and the resource, followed by the metadata block an
/// AWS error carries. The condition is the class that answer's own code belongs to
/// ([`super::answered_refusal`]), which names the PERMISSION
/// ([`crate::state::NO_PERMISSIONS`]) because that is what the answer is about,
/// as a topic's and a function's do.
///
/// The request id is MINTED there, for the reason
/// [`super::answered_refusal`] gives.
pub(super) fn log_group_write_refusal(stream_arn: &str, action: &str) -> (String, String) {
    super::answered_refusal(
        &::service_common::iam_authorization::not_authorized_by_resource_policy(
            crate::state::SERVICE_PRINCIPAL,
            action,
            stream_arn,
        ),
        LOGS_SERVICE_NAME,
        LOGS_ACCESS_DENIED_STATUS,
        LOGS_ACCESS_DENIED_CODE,
    )
}
