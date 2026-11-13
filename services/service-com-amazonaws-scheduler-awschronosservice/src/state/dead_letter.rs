// SPDX-License-Identifier: CC0-1.0

//! What becomes of an invocation that did NOT reach its target: how many further
//! attempts the schedule's own retry policy admits, and what the dead-letter route
//! the last of them takes carries.
//!
//! EventBridge Scheduler does not drop a failed invocation. `Target$RetryPolicy`
//! "includes information about the retry policy settings, including the maximum age
//! of an event, and the maximum number of times EventBridge Scheduler will try to
//! deliver the event to a target", and `MaximumRetryAttempts` is "the maximum number
//! of retry attempts to make before the request fails. Retry attempts with
//! exponential backoff continue until either the maximum number of attempts is made
//! or until the duration of the `MaximumEventAgeInSeconds` is reached" — so a
//! delivery that failed is made again inside BOTH of those bounds
//! ([`RetryBounds`]), and the attempt that exhausts either of them is the
//! invocation's last.
//!
//! What then becomes of that invocation is `Target$DeadLetterConfig`: "an object
//! that contains information about an Amazon SQS queue that EventBridge Scheduler
//! uses as a dead-letter queue for your schedule. If specified, EventBridge
//! Scheduler delivers failed events that could not be successfully delivered to a
//! target to the queue." So the event that could not be delivered is sent to that
//! queue, carrying the request the attempt made of the target's own API — which the
//! invocation half builds, since it is the half that makes that call — and, beside it,
//! the detail of the failure that ended the invocation ([`Report`]). A schedule that
//! names no such queue leaves nothing behind but this emulator's own log: the
//! service has nowhere to report the failure to.
//!
//! Which failures are RETRIED at all is what the target answered: a throttled call
//! and a target's own server-side failure are made again, while a refusal of the
//! request itself — the permissions the invocation is made under withhold the call,
//! the target does not exist, the request is not one the target accepts — fails the
//! same way however often it is repeated, so such an attempt is the invocation's
//! last whatever the retry policy admits ([`failure_of`]).

use std::time::Duration;

use crate::ops::types::RetryPolicy;

/// How many retries an invocation of a schedule that states no
/// `MaximumRetryAttempts` makes.
///
/// Every schedule has a retry policy, whether or not it states one, and the figure
/// AWS applies to a schedule that states none is the largest the member admits
/// (`RetryPolicy$MaximumRetryAttempts`, modelled `range 0..185`).
pub(crate) const DEFAULT_MAXIMUM_RETRY_ATTEMPTS: u32 = 185;

/// How long retries continue for an invocation of a schedule that states no
/// `MaximumEventAgeInSeconds`: likewise the largest that member admits
/// (modelled `range 60..86400`), which is 24 hours.
pub(crate) const DEFAULT_MAXIMUM_EVENT_AGE: Duration = Duration::from_secs(86_400);

/// How long after a failed attempt the next attempt falls due.
///
/// AWS makes its retries "with exponential backoff" and publishes no schedule for
/// them, so this span is CHOSEN: short enough that an invocation whose schedule
/// states a few attempts reaches its dead-letter route while a client is still
/// watching for it, and long enough that a default policy's attempts span more than
/// the shortest `MaximumEventAgeInSeconds` a schedule may state — so the age bound
/// still ends a series of retries the attempt count alone would not. Nothing sleeps
/// for it: the next attempt's instant is stored and the clock performs the attempt
/// once it has already fallen due.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// The error code reported for a failure whose answer carried none.
///
/// AWS's own general-purpose code for a request that failed on something nothing
/// more specific was established about: "InternalFailure — the request processing
/// has failed because of an unknown error, exception or failure".
const UNREADABLE_FAILURE_CODE: &str = "InternalFailure";

/// The error code of a call the invocation's own permissions withheld, for a service
/// whose protocol spells AWS's authorization refusal in its modern form, which is the
/// one AWS's authorization layer answers such a call with.
pub(crate) const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The same refusal as answered by a QUERY-COMPATIBLE service, Amazon SQS among them
/// (`awsQueryCompatible`): such a service answers with the legacy Query code, which is
/// what an SDK surfaces for it, and AWS's Query code for an unauthorized call is this.
pub(crate) const QUERY_ACCESS_DENIED_CODE: &str = "AccessDenied";

/// The refusal Amazon SNS answers an unauthorized call with, which that service MODELS
/// rather than leaving to AWS's authorization layer: `AuthorizationErrorException`,
/// whose `awsQueryError` code — the code an SDK surfaces — is this, under HTTP 403.
pub(crate) const AUTHORIZATION_ERROR_CODE: &str = "AuthorizationError";

/// The status a throttled call is answered with ("429 Too Many Requests"), which is
/// the one client-side status worth another attempt.
const TOO_MANY_REQUESTS: u16 = 429;

/// The lowest status that reports a failure of the SERVICE rather than of the
/// request, every one of which is worth another attempt.
const SERVER_ERROR_FLOOR: u16 = 500;

/// The SQS message attribute the dead-letter message reports the failing call's
/// error code in; the ones below carry the rest of the failure's detail.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
/// See [`ERROR_CODE_ATTRIBUTE`].
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";
/// See [`ERROR_CODE_ATTRIBUTE`].
const EXECUTION_ID_ATTRIBUTE: &str = "EXECUTION_ID";
/// See [`ERROR_CODE_ATTRIBUTE`].
const EXHAUSTED_RETRY_CONDITION_ATTRIBUTE: &str = "EXHAUSTED_RETRY_CONDITION";
/// See [`ERROR_CODE_ATTRIBUTE`].
const RETRY_ATTEMPTS_ATTRIBUTE: &str = "RETRY_ATTEMPTS";
/// See [`ERROR_CODE_ATTRIBUTE`].
const SCHEDULE_ARN_ATTRIBUTE: &str = "SCHEDULE_ARN";
/// See [`ERROR_CODE_ATTRIBUTE`].
const SCHEDULED_TIME_ATTRIBUTE: &str = "SCHEDULED_TIME";
/// See [`ERROR_CODE_ATTRIBUTE`].
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";

/// The `DataType` of a message attribute carrying text, as SQS names it.
const STRING_TYPE: &str = "String";

/// The `DataType` of a message attribute carrying a figure.
const NUMBER_TYPE: &str = "Number";

/// The member whose bound ended an invocation's retries, named as the retry policy
/// names it — which is what the dead-letter message reports the condition as.
const MAXIMUM_EVENT_AGE_MEMBER: &str = "MaximumEventAgeInSeconds";

/// Why one delivery attempt did not reach its target.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    /// The error code the failing call was answered with.
    pub(crate) code: String,
    /// The message that answer carried.
    pub(crate) message: String,
    /// Whether another attempt could come to anything else: a throttled or
    /// server-side failure could, a refusal of the request itself could not.
    pub(crate) retryable: bool,
}

/// What one delivery attempt came to.
#[derive(Debug)]
pub(crate) enum Attempt {
    /// The target's own API accepted the invocation.
    Delivered,
    /// The attempt was MADE and the target was not reached; the failure says why.
    Failed(Failure),
    /// Nothing was attempted, because this emulator does not invoke a target of that
    /// kind; the string says which. Nothing is reported as failed for it — no
    /// invocation was made to fail.
    NotAttempted(String),
}

/// Which of a retry policy's two bounds ended an invocation's attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exhaustion {
    /// "The maximum number of retry attempts to make before the request fails."
    MaximumRetryAttempts,
    /// "The maximum amount of time, in seconds, to continue to make retry attempts."
    MaximumEventAge,
}

/// The bounds one schedule's retry policy puts on an invocation's attempts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryBounds {
    /// How many RETRIES may be made, beyond the first attempt.
    pub(crate) retries: u32,
    /// How long retries continue, measured from the invocation's first attempt.
    pub(crate) age: Duration,
}

impl RetryBounds {
    /// The bounds `policy` states, each member it omits standing at its default.
    pub(crate) fn of(policy: Option<&RetryPolicy>) -> Self {
        let retries = policy
            .and_then(|policy| policy.maximum_retry_attempts)
            .and_then(|attempts| u32::try_from(attempts).ok())
            .unwrap_or(DEFAULT_MAXIMUM_RETRY_ATTEMPTS);
        let age = policy
            .and_then(|policy| policy.maximum_event_age_in_seconds)
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(DEFAULT_MAXIMUM_EVENT_AGE, Duration::from_secs);
        Self { retries, age }
    }

    /// Which bound a further attempt would exceed, or `None` while these bounds
    /// admit one.
    ///
    /// `retries_made` counts the attempts already made BEYOND the first, and
    /// `age_at_next` is how old the invocation would be when that further attempt
    /// was made: retries "continue until either the maximum number of attempts is
    /// made or until the duration of the `MaximumEventAgeInSeconds` is reached", so
    /// either figure reaching its bound ends them, and the attempt count is reported
    /// first because it is the one the policy states as a count of attempts.
    pub(crate) fn exhausted(&self, retries_made: u32, age_at_next: Duration) -> Option<Exhaustion> {
        if retries_made >= self.retries {
            return Some(Exhaustion::MaximumRetryAttempts);
        }
        if age_at_next >= self.age {
            return Some(Exhaustion::MaximumEventAge);
        }
        None
    }
}

/// What the dead-letter queue is told about an invocation that failed terminally.
#[derive(Debug)]
pub(crate) struct Report<'a> {
    /// The schedule the invocation belongs to.
    pub(crate) schedule_arn: &'a str,
    /// The target it could not be delivered to.
    pub(crate) target_arn: &'a str,
    /// The invocation's own execution id.
    pub(crate) execution_id: &'a str,
    /// The occurrence's scheduled time, as the context attribute carries it.
    pub(crate) scheduled_time: &'a str,
    /// How many retries were made beyond the first attempt.
    pub(crate) retries: u32,
    /// Which retry bound ended the attempts, for an invocation whose last failure
    /// was retryable, and `None` for one whose failure was never worth retrying.
    pub(crate) exhausted: Option<Exhaustion>,
    /// The failure the last attempt came to.
    pub(crate) failure: &'a Failure,
}

/// One message attribute of a dead-letter message.
#[derive(Debug)]
pub(crate) struct Attribute {
    /// The name the attribute is carried under.
    pub(crate) name: &'static str,
    /// Its SQS `DataType`.
    pub(crate) data_type: &'static str,
    /// Its value.
    pub(crate) value: String,
}

/// The message attributes a dead-letter message carries beside the undelivered
/// payload: which schedule and target the invocation was for, which invocation it
/// was, the instant it was scheduled at, how many retries it took, and the failure
/// that ended it.
///
/// `EXHAUSTED_RETRY_CONDITION` is reported for the event-age bound alone: it names
/// the condition the retries ran OUT of time on, so an invocation whose attempts
/// were used up — and one whose failure was never retryable, which spent none of
/// either bound — carries no such attribute.
pub(crate) fn dead_letter_attributes(report: &Report<'_>) -> Vec<Attribute> {
    let text = |name: &'static str, value: &str| Attribute {
        name,
        data_type: STRING_TYPE,
        value: value.to_string(),
    };
    let mut attributes = vec![
        text(ERROR_CODE_ATTRIBUTE, &report.failure.code),
        text(ERROR_MESSAGE_ATTRIBUTE, &report.failure.message),
        text(EXECUTION_ID_ATTRIBUTE, report.execution_id),
        Attribute {
            name: RETRY_ATTEMPTS_ATTRIBUTE,
            data_type: NUMBER_TYPE,
            value: report.retries.to_string(),
        },
        text(SCHEDULE_ARN_ATTRIBUTE, report.schedule_arn),
        text(SCHEDULED_TIME_ATTRIBUTE, report.scheduled_time),
        text(TARGET_ARN_ATTRIBUTE, report.target_arn),
    ];
    if report.exhausted == Some(Exhaustion::MaximumEventAge) {
        attributes.push(text(
            EXHAUSTED_RETRY_CONDITION_ATTRIBUTE,
            MAXIMUM_EVENT_AGE_MEMBER,
        ));
    }
    attributes
}

/// The failure one answered call came to: the code and message the target's own
/// service reported, and whether another attempt is worth making.
///
/// A status of 429 and every status reporting a failure of the service are worth
/// another attempt; every other answer is a refusal of the request, which the next
/// attempt would be refused by in the same way. An attempt that was never ANSWERED
/// — nothing came back at all — establishes nothing about the request, so it is
/// retried rather than written off.
pub(crate) fn failure_of<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> Failure
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata
        + ::std::error::Error
        + 'static,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let answered = error
        .raw_response()
        .map(|response| response.status().as_u16());
    let service = error.as_service_error();
    Failure {
        code: service
            .and_then(ProvideErrorMetadata::code)
            .unwrap_or(UNREADABLE_FAILURE_CODE)
            .to_string(),
        message: service.and_then(ProvideErrorMetadata::message).map_or_else(
            // An answer carrying no message of its own is reported as the whole of
            // what came back, through the SDK's own rendering of a failure.
            || ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string(),
            ToString::to_string,
        ),
        retryable: answered
            .is_none_or(|status| status == TOO_MANY_REQUESTS || status >= SERVER_ERROR_FLOOR),
    }
}

/// The failure a call the invocation's own permissions withheld came to: the
/// authorization refusal the answering service gives such a call, under the `code` that
/// service answers it with, which no further attempt would be answered differently.
pub(crate) fn withheld(code: &str, message: String) -> Failure {
    Failure {
        code: code.to_string(),
        message,
        retryable: false,
    }
}

/// The failure a RAW answer came to, for an invocation made without a typed client:
/// the error code and the message that answer itself carries.
///
/// A universal target's invocation is the invoked operation's own call
/// ([`super::universal_target`]), made over the in-process transport rather than
/// through one service's generated client, so what the answer reports is read off the
/// answer: an AWS JSON refusal names its error shape in the `x-amzn-errortype` header
/// and in the body's `__type` discriminator — read through the one reading of those two
/// places this tree has ([`::service_common::error_type_locator`]) — and carries its
/// message under either spelling the protocol allows. An answer naming no class at all
/// is reported under [`UNREADABLE_FAILURE_CODE`], and one carrying no message of its own
/// is reported as the text that came back.
///
/// Whether another attempt is worth making is the STATUS's, exactly as it is for a
/// typed client's failure ([`failure_of`]).
pub(crate) fn failure_of_answer(status: u16, error_type: Option<&str>, payload: &[u8]) -> Failure {
    use ::service_common::error_type_locator::{shape_named, SHAPE_DISCRIMINATOR};

    let document: Option<::serde_json::Value> = ::serde_json::from_slice(payload).ok();
    let named = error_type.map(ToString::to_string).or_else(|| {
        document
            .as_ref()?
            .get(SHAPE_DISCRIMINATOR)
            .and_then(::serde_json::Value::as_str)
            .map(ToString::to_string)
    });
    // The header may carry a URL after the class, which is no part of the name, and
    // either place may qualify the shape by its namespace.
    let code = named
        .as_deref()
        .map(|named| shape_named(named.split(':').next().unwrap_or(named)).trim())
        .filter(|code| !code.is_empty())
        .unwrap_or(UNREADABLE_FAILURE_CODE)
        .to_string();
    let message = document
        .as_ref()
        .and_then(|document| {
            ["message", "Message"]
                .iter()
                .find_map(|member| document.get(*member).and_then(::serde_json::Value::as_str))
        })
        .map_or_else(
            || String::from_utf8_lossy(payload).to_string(),
            ToString::to_string,
        );
    Failure {
        code,
        message,
        retryable: status == TOO_MANY_REQUESTS || status >= SERVER_ERROR_FLOOR,
    }
}

/// The failure an invocation whose call was never ANSWERED came to.
///
/// Nothing came back, so nothing is established about the request — exactly the case
/// [`failure_of`] retries an unanswered call for — and `detail` is what went wrong on
/// the way, so the dead-letter report says that rather than naming a refusal no service
/// gave.
pub(crate) fn unanswered(detail: &str) -> Failure {
    Failure {
        code: UNREADABLE_FAILURE_CODE.to_string(),
        message: detail.to_string(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A retry policy stating these members, as a schedule carries it.
    fn policy(retries: Option<i32>, age: Option<i32>) -> RetryPolicy {
        RetryPolicy {
            maximum_event_age_in_seconds: age,
            maximum_retry_attempts: retries,
        }
    }

    /// The bounds a policy states are the figures it carries, and a member it omits
    /// stands at the largest value the model admits for it. The rows differ along
    /// which member the policy states, and run each published bound of both members
    /// (`MaximumRetryAttempts` 0..185, `MaximumEventAgeInSeconds` 60..86400).
    #[test]
    fn a_policy_bounds_the_attempts_by_the_figures_it_states() {
        // (case, the policy, the retries it admits, its event age in seconds)
        let rows: [(&str, Option<RetryPolicy>, u32, u64); 7] = [
            ("no policy at all", None, 185, 86_400),
            ("a policy stating neither member", Some(policy(None, None)), 185, 86_400),
            ("no retry at all", Some(policy(Some(0), None)), 0, 86_400),
            ("a single retry", Some(policy(Some(1), None)), 1, 86_400),
            ("the most retries", Some(policy(Some(185), None)), 185, 86_400),
            ("the shortest event age", Some(policy(None, Some(60))), 185, 60),
            ("the longest event age", Some(policy(None, Some(86_400))), 185, 86_400),
        ];
        for (case, policy, retries, age) in rows {
            let bounds = RetryBounds::of(policy.as_ref());
            assert_eq!(
                bounds.retries, retries,
                "case {case:?}: expected {retries} retries, got {}",
                bounds.retries
            );
            assert_eq!(
                bounds.age,
                Duration::from_secs(age),
                "case {case:?}: expected an event age of {age}s, got {:?}",
                bounds.age
            );
        }
    }

    /// Either bound ends the attempts, and the one that ends them is the one the
    /// report names: the rows differ along which bound the invocation meets first,
    /// including the row where neither is met yet.
    #[test]
    fn either_bound_ends_the_attempts() {
        let bounds = RetryBounds {
            retries: 3,
            age: Duration::from_secs(60),
        };
        // (case, the retries already made, the age the next attempt would be at,
        //  which bound that exhausts)
        let rows: [(&str, u32, u64, Option<Exhaustion>); 6] = [
            ("the first failure of a policy admitting retries", 0, 1, None),
            ("a retry inside both bounds", 2, 30, None),
            (
                "the retry that reaches the attempt count",
                3,
                30,
                Some(Exhaustion::MaximumRetryAttempts),
            ),
            (
                "an attempt count past its bound",
                9,
                30,
                Some(Exhaustion::MaximumRetryAttempts),
            ),
            (
                "the attempt that reaches the event age",
                1,
                60,
                Some(Exhaustion::MaximumEventAge),
            ),
            (
                "an age past its bound",
                1,
                3_600,
                Some(Exhaustion::MaximumEventAge),
            ),
        ];
        for (case, retries_made, age, expected) in rows {
            let exhausted = bounds.exhausted(retries_made, Duration::from_secs(age));
            assert_eq!(
                exhausted, expected,
                "case {case:?}: expected {expected:?}, got {exhausted:?}"
            );
        }
    }

    /// The dead-letter message's attributes carry the invocation's own detail, and
    /// the exhausted-retry condition is reported for the event-age bound alone.
    #[test]
    fn a_dead_letter_message_carries_the_failures_detail() {
        let failure = Failure {
            code: "AWS.SimpleQueueService.NonExistentQueue".to_string(),
            message: "The specified queue does not exist.".to_string(),
            retryable: false,
        };
        // (case, which bound ended the attempts, whether the condition is reported)
        let rows: [(&str, Option<Exhaustion>, bool); 3] = [
            ("a failure that was never retryable", None, false),
            (
                "attempts used up",
                Some(Exhaustion::MaximumRetryAttempts),
                false,
            ),
            (
                "retries out of time",
                Some(Exhaustion::MaximumEventAge),
                true,
            ),
        ];
        for (case, exhausted, reported) in rows {
            let attributes = dead_letter_attributes(&Report {
                schedule_arn: "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly",
                target_arn: "arn:aws:sqs:eu-west-2:000000000000:orders",
                execution_id: "0f7b9c1e-0000-4000-8000-000000000001",
                scheduled_time: "2030-01-01T00:00:00Z",
                retries: 2,
                exhausted,
                failure: &failure,
            });
            let held = |name: &str| {
                attributes
                    .iter()
                    .find(|attribute| attribute.name == name)
                    .map(|attribute| attribute.value.clone())
            };
            for (name, expected) in [
                (ERROR_CODE_ATTRIBUTE, failure.code.clone()),
                (ERROR_MESSAGE_ATTRIBUTE, failure.message.clone()),
                (
                    EXECUTION_ID_ATTRIBUTE,
                    "0f7b9c1e-0000-4000-8000-000000000001".to_string(),
                ),
                (RETRY_ATTEMPTS_ATTRIBUTE, "2".to_string()),
                (
                    SCHEDULE_ARN_ATTRIBUTE,
                    "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly".to_string(),
                ),
                (SCHEDULED_TIME_ATTRIBUTE, "2030-01-01T00:00:00Z".to_string()),
                (
                    TARGET_ARN_ATTRIBUTE,
                    "arn:aws:sqs:eu-west-2:000000000000:orders".to_string(),
                ),
            ] {
                assert_eq!(
                    held(name),
                    Some(expected.clone()),
                    "case {case:?}: expected {name} to carry {expected:?}, got {:?}",
                    held(name)
                );
            }
            let condition = held(EXHAUSTED_RETRY_CONDITION_ATTRIBUTE);
            assert_eq!(
                condition.is_some(),
                reported,
                "case {case:?}: expected the exhausted-retry condition reported={reported}, got \
                 {condition:?}"
            );
            if reported {
                assert_eq!(
                    condition.as_deref(),
                    Some(MAXIMUM_EVENT_AGE_MEMBER),
                    "case {case:?}: expected the event-age member named, got {condition:?}"
                );
            }
            let numbers: Vec<&str> = attributes
                .iter()
                .filter(|attribute| attribute.data_type == NUMBER_TYPE)
                .map(|attribute| attribute.name)
                .collect();
            assert_eq!(
                numbers,
                vec![RETRY_ATTEMPTS_ATTRIBUTE],
                "case {case:?}: expected the retry count to be the one numeric attribute, got \
                 {numbers:?}"
            );
        }
    }
}
