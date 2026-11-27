// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeguruprofiler-codeguruprofiler service.
//!
//! One map of profiling groups, keyed by name — the service's only resource. Every
//! handler computes its response from what is stored here; nothing is canned.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AggregatedProfileTime, AgentOrchestrationConfig, Anomaly, AnomalyInstance, Channel,
    FindingsReportSummary, Metric, NotificationConfiguration, ProfilingGroupDescription,
    ProfilingStatus, UserFeedback,
};

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as
/// `OPERATIONS.md` / the model's `aws.api#service` records it.
pub const ARN_NAMESPACE: &str = "codeguru-profiler";

/// `computePlatform` when `CreateProfilingGroup` does not specify one. The model's
/// doc for that member: "If not specified, Default is used."
pub const DEFAULT_COMPUTE_PLATFORM: &str = "Default";

/// The aggregation periods this service offers, as (the modelled `AggregationPeriod`
/// value, the period's length in WHOLE SECONDS): the three values the model's
/// `AggregationPeriod` enum declares (PT5M, PT1H, P1D), with the lengths the
/// `GetProfile` doc states — "Aggregated profiles are available with aggregation
/// periods of 5 minutes, 1 hour, and 1 day, aligned to UTC."
///
/// Listed from the MOST GRANULAR window to the coarsest, so the first entry is the finest
/// period a posted profile is collected into ([`FINEST_AGGREGATION_PERIOD`]).
pub const AGGREGATION_PERIODS: &[(&str, i64)] = &[
    ("PT5M", 5 * 60),
    ("PT1H", 60 * 60),
    ("P1D", 24 * 60 * 60),
];

/// The MOST GRANULAR aggregation period this service aggregates at, as (the modelled
/// `AggregationPeriod` value, its length in whole seconds): the first entry of
/// [`AGGREGATION_PERIODS`], which lists them shortest window first. This is the finest
/// window a posted profile can be collected into — and so the finest resolution a time
/// series over those profiles can have.
pub const FINEST_AGGREGATION_PERIOD: (&str, i64) = AGGREGATION_PERIODS[0];

/// The length in seconds of a modelled `AggregationPeriod` value, or `None` when the
/// string is not one of [`AGGREGATION_PERIODS`], which covers every value the
/// generated `AGGREGATION_PERIOD_VALUES` lists.
pub fn aggregation_period_seconds(period: &str) -> Option<i64> {
    AGGREGATION_PERIODS
        .iter()
        .find(|(name, _)| *name == period)
        .map(|(_, seconds)| *seconds)
}

/// The span the ISO 8601 duration `period` names, or `None` when it names none —
/// [`::service_common::iso8601_span::iso8601_span`], the one reading of such a value, so
/// no two services disagree about how long a duration is.
///
/// This reads a FREE-FORM duration, which is what `GetProfile$period` targets (`Period`:
/// a string of length 1..64, whose own doc gives `P1DT1H1M1S` as an example) — as opposed
/// to [`aggregation_period_seconds`], which reads the `AggregationPeriod` ENUM that
/// `ListProfileTimes$period` targets.
///
/// A duration of zero length IS a span and is returned as one: a window of zero width is
/// refused by [`validate_interval`], with the sentence that rule states, rather than
/// reported here as a duration that could not be read.
pub fn iso8601_period(period: &str) -> Option<Duration> {
    ::service_common::iso8601_span::iso8601_span(period)
}

/// The message a read refused for a `period` that names no fixed span carries: the rule
/// the member states ("Specify using the ISO 8601 format. For example, P1DT1H1M1S") and
/// the value the request sent, so the caller sees what to change.
pub fn period_validation_message(period: &str) -> String {
    format!(
        "period must name a fixed span of time in the ISO 8601 duration format, for \
         example P1DT1H1M1S, got {period}"
    )
}

/// What a read says when a request names ALL THREE of the members a window is expressed
/// with: the trio over-determines the range, so the request is refused rather than served
/// about one reading of it. The `GetProfile` doc states the rule — "Specify the time range
/// for the requested aggregated profile using 1 or 2 of the following parameters:
/// startTime, endTime, period. … If you specify all 3 parameters, an exception is thrown."
pub const OVER_DETERMINED_WINDOW_MESSAGE: &str =
    "specify 1 or 2 of startTime, endTime and period, not all 3";

/// The time range a request names with the three members this service's reads express a
/// window with, as [`requested_window`] read them.
///
/// Either bound may be OPEN: the trio names a window with 1 or 2 of its members, so a
/// request that named only one of them leaves the other side to the operation reading it.
/// `span` is the length the request named, kept beside the bounds for an operation that
/// completes an open bound with it.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestedWindow {
    /// `startTime`, or the start the named `endTime` and length imply.
    pub start: Option<DateTime>,
    /// `endTime`, or the end the named `startTime` and length imply.
    pub end: Option<DateTime>,
    /// The length `period` named, or `None` when the request named no length.
    pub span: Option<Duration>,
}

/// The window a request names with `startTime`, `endTime` and a `period` giving the
/// window's LENGTH — this service's one reading of that trio, for every operation that
/// takes a time range expressed with it.
///
/// The rule, as the `GetProfile` doc states it: "Specify the time range for the requested
/// aggregated profile using 1 or 2 of the following parameters: startTime, endTime,
/// period. … If you specify all 3 parameters, an exception is thrown." So a length
/// completes whichever single bound was given, all three together are refused
/// ([`OVER_DETERMINED_WINDOW_MESSAGE`]), and a length on its own leaves both bounds open —
/// it says how LONG the window is, not where it sits.
///
/// `Err` carries the sentence this service states for the refusal, for each handler to
/// lift into its own modelled client-input error (HTTP 400): a `period` naming no fixed
/// span ([`period_validation_message`]), a request naming all three members, or a
/// COMPLETED pair of bounds that is no interval ([`validate_interval`], so a zero-length
/// `period` is refused as the zero-width window it names).
///
/// WHICH single member a request may name on its own is the calling operation's rule, not
/// this one: `GetProfile`'s two bound members each document a companion they require, while
/// `BatchGetFrameMetricData`'s document none and that handler completes an open side
/// itself. So this read states only what the trio MEANS, and an operation whose members
/// carry such a rule applies it before asking.
pub fn requested_window(
    start_time: Option<DateTime>,
    end_time: Option<DateTime>,
    period: Option<&str>,
) -> Result<RequestedWindow, String> {
    // `period` here is the free-form `Period` string — the LENGTH of the window, stated as
    // an ISO 8601 duration — and not the `AggregationPeriod` enum a resolution is named
    // by, so the one read of it is [`iso8601_period`]. Only a string that names no fixed
    // span is refused.
    let span = match period {
        None => None,
        Some(period) => {
            Some(iso8601_period(period).ok_or_else(|| period_validation_message(period))?)
        }
    };

    let (start, end) = match (start_time, end_time, span) {
        (Some(_), Some(_), Some(_)) => {
            return Err(OVER_DETERMINED_WINDOW_MESSAGE.to_string());
        }
        (Some(start), None, Some(span)) => (Some(start), Some(shifted(start, span_nanos(span)))),
        (None, Some(end), Some(span)) => (Some(shifted(end, -span_nanos(span))), Some(end)),
        // A length alone (and no member at all) names no bound.
        (start, end, _) => (start, end),
    };

    // A window that names BOTH bounds names an interval, and a pair of bounds is one only
    // when the start is strictly before the end. It is checked on the COMPLETED window, so
    // a length of zero is refused as the zero-width window it names rather than served as
    // a window at all.
    if let (Some(start), Some(end)) = (start, end) {
        validate_interval(start, end)?;
    }
    Ok(RequestedWindow { start, end, span })
}

/// `span` as whole nanoseconds, saturating — the sign is the caller's, which is how the
/// same span completes a window forwards from a start and backwards from an end.
pub fn span_nanos(span: Duration) -> i128 {
    i128::try_from(span.as_nanos()).unwrap_or(i128::MAX)
}

/// One instant as whole nanoseconds since the epoch, exactly — the unit this service does
/// its instant arithmetic in, since a `DateTime` carries nanoseconds and an `f64` of
/// current epoch seconds cannot represent them ([`instant_key`] orders instants for the
/// same reason).
///
/// The difference of two of these is how LONG a range is, in the one unit a window length
/// ([`span_nanos`]) is stated in, so a length and a range are compared without either
/// being rounded to whole seconds.
pub fn epoch_nanos(at: DateTime) -> i128 {
    const NANOS_PER_SECOND: i128 = 1_000_000_000;
    i128::from(at.secs()) * NANOS_PER_SECOND + i128::from(at.subsec_nanos())
}

/// `at` moved by `nanos` nanoseconds, exactly: a window length is an arbitrary ISO 8601
/// duration, so a bound it completes keeps the sub-second part of both the instant and
/// the span rather than rounding either to whole seconds.
pub fn shifted(at: DateTime, nanos: i128) -> DateTime {
    const NANOS_PER_SECOND: i128 = 1_000_000_000;
    let total = epoch_nanos(at) + nanos;
    // `div_euclid`/`rem_euclid` floor towards the past on both sides of the epoch, so the
    // remainder is always in `0..NANOS_PER_SECOND` and is always the in-range sub-second
    // part of the instant.
    let seconds = total.div_euclid(NANOS_PER_SECOND);
    let subsec = total.rem_euclid(NANOS_PER_SECOND);
    DateTime::from_secs_and_nanos(
        i64::try_from(seconds).unwrap_or(if seconds.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }),
        u32::try_from(subsec).unwrap_or(0),
    )
}

/// The start of the `period_seconds`-long aggregation window, aligned to the UTC
/// epoch, that contains the instant `at`. Alignment to UTC is what the `GetProfile`
/// doc states for every aggregation period, and the epoch is a whole number of
/// 5-minute, 1-hour and 1-day periods past UTC midnight, so flooring against it aligns
/// each of the three the same way.
///
/// The modelled `AggregationPeriod` values are whole numbers of seconds long, so this is
/// that one alignment ([`aligned_window_start`]) named in the unit they are stated in. A
/// period that is not positive names no window, so the instant is its own window start.
pub fn aggregation_window_start(at: DateTime, period_seconds: i64) -> DateTime {
    aligned_window_start(
        at,
        Duration::from_secs(u64::try_from(period_seconds).unwrap_or(0)),
    )
}

/// The start of the `span`-long window, aligned to the UTC epoch, that contains the
/// instant `at` — [`aggregation_window_start`] at the resolution a FREE-FORM window
/// length is stated in, since such a length may carry a sub-second part
/// ([`iso8601_period`]) and must then align exactly rather than being rounded to whole
/// seconds.
///
/// Exact nanoseconds, like [`instant_key`], rather than `f64` epoch seconds, whose
/// granularity at current instants is coarser than a microsecond; `div_euclid` floors
/// towards the past on both sides of the epoch, so the window containing a pre-epoch
/// instant starts before it like any other. A span that is not positive names no window,
/// so the instant is its own window start.
pub fn aligned_window_start(at: DateTime, span: Duration) -> DateTime {
    let length = span_nanos(span);
    if length <= 0 {
        return at;
    }
    let total = epoch_nanos(at);
    // Measured from the epoch, which is what "aligned to UTC" means for a window length,
    // and converted back into an instant by the one reader of that arithmetic.
    shifted(DateTime::from_secs(0), total.div_euclid(length) * length)
}

/// The SHORTEST aggregation period this service aggregates at, in whole seconds: the
/// length of [`FINEST_AGGREGATION_PERIOD`].
pub fn finest_aggregation_period_seconds() -> i64 {
    FINEST_AGGREGATION_PERIOD.1
}

/// How long the service's pass that collects submitted profiles into the aggregated
/// profile of a window takes, in milliseconds — measured from the moment there is
/// anything to collect ([`aggregated_at`]).
///
/// Collecting posted agent profiles into an aggregated profile is work the service does
/// after the fact, not part of answering the post, so the aggregate a window owes falls
/// due a span after the pass over it can start rather than at the instant of the post.
/// The span is this emulator's own choice (the real service's is neither modelled nor
/// documented) and is deliberately NON-ZERO, so a caller that posts and reads straight
/// away still sees the pending-aggregation state.
pub const AGGREGATION_PASS_MILLIS: i64 = 3_000;

/// When the aggregated profile OF THE AGGREGATION PERIOD `period_seconds` LONG that
/// covers the data starting at `data_start`, submitted at `received_at`, becomes
/// available: [`AGGREGATION_PASS_MILLIS`] after the LATER of that period's window end
/// and the instant the submission arrived.
///
/// Two things have to have happened before a window's aggregated profile can hold a
/// submission, so the pass runs after both of them. The window must have CLOSED — while
/// it is open the service is still collecting into it — and the window is the one of the
/// period being asked about, so the aggregate of an hourly or daily window holding the
/// data falls due when THAT window closes, however many finer windows inside it have
/// closed already. And the submission must have ARRIVED: a pass that ran before it
/// arrived cannot have collected it, so a submission is never part of an aggregated
/// profile at the instant it is accepted, however long ago the window its data covers
/// closed. Receiving a profile and holding an aggregated profile of its window are
/// different states, and this span is what separates them — what a caller polling for the
/// aggregate waits out.
///
/// A period that is not positive names no window ([`aggregation_window_start`]), so the
/// instant itself stands in for the end.
pub fn aggregated_at(
    data_start: DateTime,
    received_at: DateTime,
    period_seconds: i64,
) -> DateTime {
    const NANOS_PER_MILLI: i128 = 1_000_000;
    let window_end = aggregation_window_start(data_start, period_seconds)
        .secs()
        .saturating_add(period_seconds.max(0));
    // The instant the pass over this submission's window can start: both the window's
    // close and the submission's arrival are behind it. Nanoseconds, like every other
    // instant comparison here ([`instant_key`]), so a receipt a fraction of a second
    // after the window closed is not rounded back onto it.
    let collectable_from = epoch_nanos(DateTime::from_secs(window_end))
        .max(epoch_nanos(received_at));
    shifted(
        DateTime::from_secs(0),
        collectable_from + i128::from(AGGREGATION_PASS_MILLIS) * NANOS_PER_MILLI,
    )
}

/// How many posted profiles one group retains. An emulator-chosen bound, not an AWS
/// duration: the real service expires profiles over time (the `GetProfile` doc:
/// "Because aggregated profiles expire over time GetProfile is not idempotent"), and
/// an in-memory emulator must bound what a caller can make it hold, so
/// [`ProfilingGroup::record_profile`] evicts the oldest profiles once a group holds
/// more than this.
pub const MAX_RETAINED_PROFILES_PER_GROUP: usize = 16;

/// How many profile BYTES one group retains, so a few large payloads are bounded as
/// well as many small ones. Emulator-chosen, for the same reason as
/// [`MAX_RETAINED_PROFILES_PER_GROUP`]; the newest profile is always kept even when it
/// alone is larger than this.
pub const MAX_RETAINED_PROFILE_BYTES_PER_GROUP: usize = 4 * 1024 * 1024;

/// How many profile BYTES the whole store retains, across every group. This is the
/// bound on what the service can be made to hold in total: the per-group caps alone
/// would still scale with the number of groups, so
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`] evicts the
/// oldest profile anywhere in the store until the total fits (never the profile just
/// posted). Emulator-chosen.
pub const MAX_RETAINED_PROFILE_BYTES: usize = 64 * 1024 * 1024;

/// How many profiling groups this emulator holds. Emulator-chosen, and reported with
/// the operation's own modelled `ServiceQuotaExceededException`, which says exactly
/// this: "You have exceeded your service quota. To perform the requested action,
/// remove some of the relevant resources, or use Service Quotas to request a service
/// quota increase." (The real service also caps profiling groups per account.)
pub const MAX_PROFILING_GROUPS: usize = 256;

/// The most tags one resource holds. AWS's general tagging limit, not an emulator
/// choice — "Maximum number of tags per resource: 50" in the AWS tagging quotas the
/// CodeGuru Profiler tagging operations follow — so a `TagResource` (or a create) that
/// would leave a group past it is refused with the operation's modelled
/// `ValidationException` instead of growing the store. `UntagResource` frees the budget
/// again, so this is a bound on what one resource holds, not on how often it is
/// re-tagged.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag KEY, in characters, and the shortest: AWS's tagging limits give a
/// key 1..128 characters. The model constrains `TagsMap`'s key with nothing
/// (`smithy_query(query="shape",shape="TagsMap")` -> `{key: String, value: String}`),
/// so the boundary validator clamps neither and [`validate_tags`] is what holds the
/// published range.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag VALUE, in characters: AWS's tagging limits give a value 0..256
/// characters (an empty value is a valid one). Same reason as [`MAX_TAG_KEY_CHARS`] for
/// enforcing it here.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The page size a listing uses when the caller gives no `maxResults`, so one
/// unparameterised request cannot materialise a response that grows with the number of
/// stored elements. Inside the modelled `maxResults` range (1..1000).
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// How many frame metrics ONE `BatchGetFrameMetricData` request asks about. An
/// emulator-chosen bound, for the same reason as [`DEFAULT_LIST_PAGE_SIZE`]: that
/// operation answers with one value per requested metric per time step, so the response
/// grows with the PRODUCT of the two and a small request must not be able to materialise
/// an arbitrarily large one. The model's `FrameMetrics` list declares no maximum length
/// of its own, so a longer list is refused with that operation's modelled
/// `ValidationException`, worded the way every other over-long member of this service is
/// ([`validate_tags`]).
pub const MAX_FRAME_METRICS_PER_REQUEST: usize = 100;

/// How many thread states ONE requested frame metric names. Emulator-chosen, beside
/// [`MAX_FRAME_METRICS_PER_REQUEST`] and for the same reason: deriving a metric's value
/// tests the counts a stored profile document records against the states that metric asks
/// about, so the work of reading one request's documents grows with this list as well as
/// with the number of metrics. The model's `ThreadStates` list declares no maximum of its
/// own, and the runtime thread states a profile records are a small fixed set, so a longer
/// list is refused with the operation's modelled `ValidationException` in the same wording
/// ([`validate_tags`]).
pub const MAX_THREAD_STATES_PER_METRIC: usize = 32;

/// How many anomaly-notification channels one profiling group holds. NOT an
/// emulator-chosen bound: the model's `Channels` list carries `@length(min: 1, max: 2)`
/// both on the `AddNotificationChannels` input and on the `NotificationConfiguration`
/// every notification operation returns, and `AddNotificationChannels`' own doc says
/// "Add up to 2 anomaly notifications channels for a profiling group". An add that
/// would push a group past this is refused with that operation's modelled
/// `ServiceQuotaExceededException`.
pub const MAX_NOTIFICATION_CHANNELS_PER_GROUP: usize = 2;

/// The `AggregationPeriod` of a DAILY profile, as the model's `AggregationPeriod` enum
/// spells it (its values are "P1D", "PT1H", "PT5M" — see `AGGREGATION_PERIOD_VALUES`).
/// A findings report over a `P1D` profile is what `dailyReportsOnly` selects.
pub const DAILY_AGGREGATION_PERIOD: &str = "P1D";

/// How many findings reports one profiling group retains, so the report store is
/// bounded the way the profile store is. Emulator-chosen, like
/// [`MAX_RETAINED_PROFILES_PER_GROUP`]; the oldest analysed window is dropped first.
pub const MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP: usize = 64;

/// How many detected anomalies one profiling group retains, so the anomaly catalogue is
/// bounded the way the findings-report store is. Emulator-chosen, like
/// [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`]; the oldest recorded anomaly is dropped
/// first by
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_detected_anomaly`].
pub const MAX_RETAINED_ANOMALIES_PER_GROUP: usize = 64;

/// How many `PostAgentProfile` idempotency tokens one group remembers. Emulator-chosen,
/// and deliberately larger than [`MAX_RETAINED_PROFILES_PER_GROUP`] so every retained
/// profile's token is still recognised as a replay; the oldest token is forgotten
/// first, so caller-chosen token strings cannot accumulate without limit.
pub const MAX_RETAINED_PROFILE_TOKENS_PER_GROUP: usize = 64;

/// How many anomaly-feedback submissions one profiling group retains, one per anomaly
/// instance it has been given feedback about. Emulator-chosen, like
/// [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`]: an anomaly instance id is caller-supplied,
/// so an in-memory store must bound how many distinct ones it accumulates; the group's
/// least recently submitted feedback is dropped first by
/// [`ProfilingGroup::submit_anomaly_feedback`].
pub const MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP: usize = 64;

/// How many agent reports one profiling group retains, one per profiling instance that
/// has reported to it. Emulator-chosen, like
/// [`MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP`]: the instance a report identifies itself
/// with is agent-supplied, so an in-memory store must bound how many distinct ones it
/// accumulates; the group's least recently reporting instance is dropped first by
/// [`ProfilingGroup::record_agent_report`].
pub const MAX_RETAINED_AGENT_REPORTS_PER_GROUP: usize = 64;

/// How many reported BYTES one group retains across the agent reports it holds, so a few
/// large reports are bounded as well as many small ones. Emulator-chosen, for the same
/// reason as [`MAX_RETAINED_AGENT_REPORTS_PER_GROUP`]: the metadata a report carries is
/// agent-supplied and the model constrains neither its field count nor its value lengths.
/// The report just made is always kept even when it alone is larger than this, the way
/// [`MAX_RETAINED_PROFILE_BYTES_PER_GROUP`] keeps the newest profile.
pub const MAX_RETAINED_AGENT_REPORT_BYTES_PER_GROUP: usize = 64 * 1024;

/// The hex-digit group lengths a `RevisionId` carries, as the model's `RevisionId`
/// pattern (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`) spells them
/// out: 32 lowercase hex characters in five hyphen-separated groups.
pub const REVISION_ID_GROUPS: &[usize] = &[8, 4, 4, 4, 12];

/// A `RevisionId` identifying one policy write: 32 lowercase hex characters grouped the
/// way [`REVISION_ID_GROUPS`] records the modelled pattern, which is the form the
/// `PutPermission` doc names ("A universally unique identifier (UUID) for the revision
/// of the policy").
///
/// Derived with SHA-256 (the `sha2` crate) from the write itself — the store's own
/// monotonically increasing mint count, the instant of the write, and the group and
/// action group being written. The mint count is what makes two writes of the same
/// grants carry different revisions, and it is store state a caller never sees, so a
/// revision cannot be computed from the request that produced it.
fn revision_id(mint: u64, at: DateTime, group_name: &str, action_group: &str) -> String {
    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, mint.to_be_bytes());
    ::sha2::Digest::update(&mut digest, at.secs().to_be_bytes());
    ::sha2::Digest::update(&mut digest, at.subsec_nanos().to_be_bytes());
    ::sha2::Digest::update(&mut digest, (group_name.len() as u64).to_be_bytes());
    ::sha2::Digest::update(&mut digest, group_name.as_bytes());
    ::sha2::Digest::update(&mut digest, action_group.as_bytes());
    let hex = ::hex::encode(::sha2::Digest::finalize(digest));

    // The first 32 hex characters of the digest, cut into the modelled groups.
    let mut remaining = &hex[..REVISION_ID_GROUPS.iter().sum::<usize>()];
    let mut revision = String::with_capacity(remaining.len() + REVISION_ID_GROUPS.len() - 1);
    for (index, group) in REVISION_ID_GROUPS.iter().enumerate() {
        if index > 0 {
            revision.push('-');
        }
        let (head, tail) = remaining.split_at(*group);
        revision.push_str(head);
        remaining = tail;
    }
    revision
}

/// The IAM policy-language version a resource-based policy document carries. The
/// version the service's own resource-policy document uses (the AWS CodeGuru Profiler
/// User Guide, "Resource-based policies in CodeGuru Profiler").
pub const POLICY_VERSION: &str = "2012-10-17";

/// The one action group a resource-based policy can grant, as the model's `ActionGroup`
/// enum declares it (`smithy_query(query="shape",shape="ActionGroup")`).
pub const ACTION_GROUP_AGENT_PERMISSIONS: &str = "agentPermissions";

/// Every action group a resource-based policy can hold a statement for: the values the
/// model's `ActionGroup` enum declares, as the generated
/// [`crate::ops::types::ACTION_GROUP_VALUES`] lists them (one, `agentPermissions`) — the
/// handlers refuse anything outside that list, so there is ONE list of them in this
/// crate and the bound below follows the model rather than a copy of it.
pub const MAX_POLICY_STATEMENTS: usize = crate::ops::types::ACTION_GROUP_VALUES.len();

/// The most principals one statement grants to: the maximum length the model's
/// `Principals` list declares (`smithy_query(query="shape",shape="Principals")`:
/// `length 1..50`). The validating request walk already refuses a longer list at the
/// boundary; the store holds to the same bound so no writer can grow a statement past
/// what the model allows.
pub const MAX_PRINCIPALS_PER_STATEMENT: usize = 50;

/// The API actions an action group grants, as IAM action strings
/// (`<arn namespace>:<operation>`). The model's `PutPermission` doc for `actionGroup`:
/// "One action group is supported, agentPermissions, which grants permission to perform
/// actions required by the profiling agent, ConfigureAgent and PostAgentProfile
/// permissions." An action group this service does not know grants nothing, so it
/// renders no actions rather than inventing some.
pub fn action_group_actions(action_group: &str) -> Vec<String> {
    match action_group {
        ACTION_GROUP_AGENT_PERMISSIONS => vec![
            format!("{ARN_NAMESPACE}:ConfigureAgent"),
            format!("{ARN_NAMESPACE}:PostAgentProfile"),
        ],
        _ => Vec::new(),
    }
}

/// Every action group a resource-based policy can hold a statement for, as ONE list for
/// both the predicate and the sentence below: the values the model's `ActionGroup` enum
/// declares ([`crate::ops::types::ACTION_GROUP_VALUES`]), kept to those whose permissions
/// this service can name ([`action_group_actions`]). Derived from those two, so the set a
/// write is checked against and the set its refusal advertises can never disagree.
pub fn grantable_action_groups() -> Vec<&'static str> {
    crate::ops::types::ACTION_GROUP_VALUES
        .iter()
        .copied()
        .filter(|action_group| !action_group_actions(action_group).is_empty())
        .collect()
}

/// This service's ACTION-GROUP rule, in one place for every operation that writes a
/// resource-based policy — the way [`validate_interval`] holds the interval rule and
/// [`validate_tags`] the tag rules: a policy holds a statement only for an action group
/// whose permissions the service can name ([`grantable_action_groups`]), so a write
/// naming any other is refused before anything is stored, rather than persisting a
/// statement no reader can interpret.
///
/// `Ok(())` for a grantable action group; otherwise the sentence this refusal states,
/// which names the allowed set and the value the request carried, for each handler to
/// lift into its own modelled `ValidationException` (HTTP 400).
pub fn validate_action_group(action_group: &str) -> Result<(), String> {
    let grantable = grantable_action_groups();
    if grantable.contains(&action_group) {
        return Ok(());
    }
    Err(format!(
        "actionGroup must be one of {}, got {action_group}",
        grantable.join(", ")
    ))
}

/// A profiling group's resource-based policy: which principals hold which action
/// group, and the revision that identifies the policy's current content.
///
/// The policy is stored as the grants themselves, never as a document string, so every
/// reader ([`ResourcePolicy::document`]) renders the SAME document from the SAME grants
/// and the group's own stored ARN.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// `actionGroup` -> the principal ARNs that hold it, in the order they were
    /// granted. A `BTreeMap` so the rendered document's statement order is stable
    /// across reads.
    pub statements: BTreeMap<String, Vec<String>>,
    /// The policy's current revision, as the modelled `RevisionId`
    /// (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`). Rotated by
    /// whoever changes the grants, never by a read.
    pub revision_id: String,
}

impl ResourcePolicy {
    /// A statement-less policy at `revision_id` — the state a policy is built up from
    /// by the first write, which then grants it at least one statement. A STORED policy
    /// always holds at least one statement: a removal that takes the last one deletes
    /// the policy rather than storing it empty
    /// ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`]), so
    /// this is a transient value on the way to a grant, never a stored state.
    ///
    /// This constructor and the two writers below are what the permission handlers'
    /// store writers —
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`] and
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`] — build
    /// a policy with, so every operation that returns `policy` renders the same
    /// document from the same grants.
    pub fn new(revision_id: impl Into<String>) -> Self {
        Self {
            statements: BTreeMap::new(),
            revision_id: revision_id.into(),
        }
    }

    /// Grant `action_group` to exactly `principals`, REPLACING whatever principal set
    /// the action group held. The revision is NOT touched here: a writer that changes
    /// the grants rotates it once, with [`ResourcePolicy::set_revision`], however many
    /// action groups the one call rewrote.
    pub fn grant(&mut self, action_group: &str, principals: Vec<String>) {
        self.statements
            .insert(action_group.to_string(), principals);
    }

    /// Record `revision_id` as the policy's current revision. Only a change to the
    /// grants rotates it; a read reports what is stored.
    pub fn set_revision(&mut self, revision_id: impl Into<String>) {
        self.revision_id = revision_id.into();
    }

    /// Drop `action_group`'s grant, reporting whether the policy held one. The
    /// revision is not touched here, for the same reason as
    /// [`ResourcePolicy::grant`]: the writer rotates it once for the change.
    pub fn revoke(&mut self, action_group: &str) -> bool {
        self.statements.remove(action_group).is_some()
    }

    /// Whether revoking `action_group` would leave the policy holding no statement at
    /// all — what the writer that revokes a statement asks before deciding whether
    /// there is still a policy to store, and so a revision to be at. True only for the
    /// policy's LAST remaining statement.
    pub fn is_last_statement(&self, action_group: &str) -> bool {
        self.statements.len() == 1 && self.statements.contains_key(action_group)
    }

    /// The JSON-formatted resource-based policy document, as every operation that
    /// returns `policy` reports it: one `Allow` statement per stored action group,
    /// carrying that action group's statement id ([`action_group_statement_id`]), the
    /// principals that hold it, the actions the action group grants and `resource_arn` —
    /// the profiling group's own stored ARN.
    ///
    /// The document is an IAM policy document, so it is written the way an IAM policy
    /// document is written: a single principal is the ARN itself rather than a
    /// one-element list (both forms are accepted by a policy reader, and the single-ARN
    /// grant is reported in the scalar form), and the whole document is laid out by
    /// [`PolicyDocumentFormatter`]. The member ORDER is fixed by the structures below
    /// rather than by a `serde_json::Map`, whose iteration order depends on whether
    /// something in the build enabled the crate's `preserve_order` feature.
    pub fn document(&self, resource_arn: &str) -> String {
        let document = PolicyDocumentWire {
            version: POLICY_VERSION,
            statement: self
                .statements
                .iter()
                .map(|(action_group, principals)| PolicyStatementWire {
                    sid: action_group_statement_id(action_group),
                    effect: POLICY_EFFECT_ALLOW,
                    principal: PolicyPrincipalWire {
                        aws: PolicyPrincipalsWire::of(principals),
                    },
                    action: action_group_actions(action_group),
                    resource: resource_arn,
                })
                .collect(),
        };
        render_policy_document(&document)
    }
}

/// The `Effect` a stored grant carries: a resource-based policy holds the grants that
/// were made, so every statement it renders allows them.
const POLICY_EFFECT_ALLOW: &str = "Allow";

/// The suffix a policy statement's `Sid` carries past the action group it states: the
/// statement id of an action group's statement is the action group's own name plus
/// this, which is how a client locates its statement in the document again.
pub const STATEMENT_SID_SUFFIX: &str = "-statement";

/// The `Sid` the statement for `action_group` carries — the action group's statement
/// id. Applies to EVERY action group, so a document rendered for one the catalog does
/// not know still names its statement the same way.
pub fn action_group_statement_id(action_group: &str) -> String {
    format!("{action_group}{STATEMENT_SID_SUFFIX}")
}

/// One policy document, as the wire renders it. The field ORDER is the order the
/// document's members are written in, which is why these are structures and not
/// `serde_json::Value` maps.
#[derive(Debug, ::serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyDocumentWire<'a> {
    version: &'a str,
    statement: Vec<PolicyStatementWire<'a>>,
}

/// One `Allow` statement of a policy document: one stored action group's grant.
#[derive(Debug, ::serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyStatementWire<'a> {
    sid: String,
    effect: &'a str,
    principal: PolicyPrincipalWire,
    action: Vec<String>,
    resource: &'a str,
}

/// A statement's `Principal` block. `AWS` is spelled by the policy language, not by
/// Rust's casing rules, so it is renamed rather than derived.
#[derive(Debug, ::serde::Serialize)]
struct PolicyPrincipalWire {
    #[serde(rename = "AWS")]
    aws: PolicyPrincipalsWire,
}

/// The principals a statement grants to: an IAM policy document writes ONE principal
/// as the ARN itself and several as a list, so that is what is rendered.
#[derive(Debug, ::serde::Serialize)]
#[serde(untagged)]
enum PolicyPrincipalsWire {
    /// Exactly one principal: the ARN as a string scalar.
    One(String),
    /// Several (or, for a statement that somehow holds none, zero) principals.
    Several(Vec<String>),
}

impl PolicyPrincipalsWire {
    /// The rendered form of a stored principal set.
    fn of(principals: &[String]) -> Self {
        match principals {
            [only] => Self::One(only.clone()),
            several => Self::Several(several.to_vec()),
        }
    }
}

/// The JSON layout a resource-based policy document is written in: two-space
/// indentation, `" : "` between a member name and its value, and an array kept on one
/// line with a single space inside its brackets — the layout the policy documents this
/// surface returns carry.
///
/// This is `serde_json`'s OWN writer, configured through the one seam that fixes
/// layout ([`::serde_json::ser::Formatter`]): string escaping, number formatting and
/// UTF-8 handling all stay with `serde_json`.
///
/// Only the OBJECT hooks indent, because an array is written inline — which is the
/// layout, and which also means an array nested DIRECTLY inside another array would be
/// written `[ [ … ] ]` on one line, and an empty array as `[ ]`. The empty form is
/// reachable (an action group outside the catalog grants no actions, so its `Action` is
/// empty) and is written that way deliberately; an array directly inside an array is
/// not a shape any member of a policy document takes, and a member that ever took it
/// would have to be laid out here deliberately.
#[derive(Debug, Default)]
struct PolicyDocumentFormatter {
    /// How many OBJECTS deep the writer is — what the indentation counts. An array is
    /// written on one line, so it adds no depth.
    depth: usize,
    /// Whether the object open at each depth has written a member yet, innermost last:
    /// an object that wrote none closes as `{ }` rather than across two lines.
    object_wrote_member: Vec<bool>,
}

impl PolicyDocumentFormatter {
    /// A line break, then this depth's indentation.
    fn newline_indent<W>(&self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b"\n")?;
        for _ in 0..self.depth {
            writer.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl ::serde_json::ser::Formatter for PolicyDocumentFormatter {
    fn begin_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth += 1;
        self.object_wrote_member.push(false);
        writer.write_all(b"{")
    }

    fn end_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth = self.depth.saturating_sub(1);
        if self.object_wrote_member.pop().unwrap_or(false) {
            self.newline_indent(writer)?;
        } else {
            writer.write_all(b" ")?;
        }
        writer.write_all(b"}")
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if !first {
            writer.write_all(b",")?;
        }
        if let Some(wrote) = self.object_wrote_member.last_mut() {
            *wrote = true;
        }
        self.newline_indent(writer)
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b" : ")
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if !first {
            writer.write_all(b",")?;
        }
        writer.write_all(b" ")
    }

    fn end_array<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b" ]")
    }
}

/// `document`, written out with [`PolicyDocumentFormatter`].
fn render_policy_document(document: &PolicyDocumentWire<'_>) -> String {
    let mut buffer: Vec<u8> = Vec::new();
    let mut serializer =
        ::serde_json::Serializer::with_formatter(&mut buffer, PolicyDocumentFormatter::default());
    // Infallible: the only fallible steps are the write — into an in-memory buffer —
    // and the two values `serde_json` refuses (a non-string map key, a non-finite
    // number), neither of which a policy document holds.
    ::serde::Serialize::serialize(document, &mut serializer)
        .expect("a policy document writes into an in-memory buffer");
    String::from_utf8(buffer).expect("serde_json writes UTF-8")
}

/// One agent profile as `PostAgentProfile` uploaded it. The bytes are kept verbatim:
/// the agent profile is an opaque payload (Amazon Ion or JSON), and what an aggregated
/// profile states about the data in it is derived from the record below rather than
/// copied out of the upload.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub bytes: Vec<u8>,
    pub content_type: String,
    /// When the service RECEIVED this upload — what
    /// `profilingStatus.latestAgentProfileReportedAt` reports, and nothing else.
    pub received_at: DateTime,
    /// The start of the PERIOD this profile's data covers: the instant whose aligned
    /// window the data belongs to.
    ///
    /// An agent collects samples over a period and uploads the profile of that period
    /// afterwards, so the window a profile is collected into is the one its DATA covers
    /// and not the one the upload happened to arrive in: the aggregated profile of a
    /// window "contains profiles posted by the agent starting at this time for an
    /// aggregation period specified by the period property"
    /// (`smithy_query(query="doc", shape="AggregatedProfileTime", member="start")`) —
    /// the data's period, which is why every window this service derives for a profile
    /// ([`ProfilingGroup::aggregated_profile_starts`],
    /// [`ProfilingGroup::aggregated_profiles_in`],
    /// [`ProfilingGroup::latest_aggregated_window`], [`AgentProfile::is_aggregated`])
    /// is derived from this instant.
    ///
    /// It is the start the submitted document states for itself, and the receipt instant
    /// for a profile whose document states no instant there — a document states its own
    /// period, so a profile that says nothing about it is placed where it arrived.
    pub period_start: DateTime,
}

impl AgentProfile {
    /// When this posted profile has been collected into the aggregated profile of the
    /// aggregation period `period_seconds` long — the instant [`aggregated_at`] places for
    /// the window its data covers and the instant it arrived.
    pub fn aggregated_at(&self, period_seconds: i64) -> DateTime {
        aggregated_at(self.period_start, self.received_at, period_seconds)
    }

    /// Whether, as of `at`, this posted profile is part of the aggregated profile of the
    /// aggregation period `period_seconds` long.
    ///
    /// The operations that report AGGREGATED profiles (`GetProfile`, `ListProfileTimes`)
    /// count a posted profile only once this holds: until then the group has received
    /// the profile — `profilingStatus.latestAgentProfileReportedAt` says so — and no
    /// aggregated profile of that period covers it yet. The pass that collects it runs
    /// after the window its data covers has closed AND after the submission itself
    /// arrived ([`aggregated_at`]), so a profile is never already aggregated at the
    /// instant it is posted, however long ago its window closed. The period is the one the
    /// caller is reporting about, so a window that is still open counts no profile it
    /// holds whatever finer windows inside it have closed; a reader of a FREE-FORM range,
    /// which names no aggregation period, asks about the finest window a profile is
    /// collected into ([`finest_aggregation_period_seconds`]).
    pub fn is_aggregated(&self, period_seconds: i64, at: DateTime) -> bool {
        instant_key(self.aggregated_at(period_seconds)) <= instant_key(at)
    }
}

/// The state ONE profiling instance reported with its latest `ConfigureAgent` call.
///
/// That operation's doc makes reporting as much of it as configuring — "Used by profiler
/// agents to report their current state and to receive remote configuration updates" — so
/// what an agent reports about itself is stored against the instance that reported it:
/// `fleetInstanceId` is "a universally unique identifier (UUID) for a profiling instance"
/// and `metadata` is "metadata captured about the compute platform the agent is running
/// on. It includes information about sampling and reporting".
///
/// No operation reports a stored report back: the modelled `AgentConfiguration` carries
/// the service's decision FOR an agent, not the agent's own account of itself, and the
/// model declares no shape that returns the latter. So this is the record a group holds
/// about each instance that has reported to it, read through
/// [`ProfilingGroup::agent_report`] — the same standing as the stored
/// [`AnomalyFeedback`] a submission leaves behind.
#[derive(Debug, Clone)]
pub struct AgentReport {
    /// The `FleetInstanceId` the report named, or `None` when the agent named none (the
    /// member is optional). This is the identity the report is filed under, so a later
    /// report from the same instance supersedes this one.
    pub fleet_instance_id: Option<String>,
    /// The `Metadata` the report carried, keyed by the modelled `MetadataField`, and
    /// empty when the report carried none — a report that names no metadata reports that
    /// it has none, so it does not inherit what the instance sent before.
    pub metadata: HashMap<String, String>,
    /// When this instance reported. The most recent such instant over every instance is
    /// what the group's `profilingStatus.latestAgentOrchestratedAt` reports.
    pub reported_at: DateTime,
}

impl AgentReport {
    /// How many bytes of reported text this record holds — what
    /// [`MAX_RETAINED_AGENT_REPORT_BYTES_PER_GROUP`] bounds across a group's reports.
    fn reported_bytes(&self) -> usize {
        let metadata: usize = self
            .metadata
            .iter()
            .map(|(field, value)| field.len().saturating_add(value.len()))
            .sum();
        self.fleet_instance_id
            .as_ref()
            .map_or(0, String::len)
            .saturating_add(metadata)
    }
}

/// One anomaly-notification channel of a profiling group, as
/// `AddNotificationChannels` stored it.
///
/// `id` is a plain `String` rather than the modelled `Option<String>`: a STORED channel
/// always has an id — the one the request named, or the random UUID the model's
/// `Channel$id` doc says is used "when adding a channel to the notification
/// configuration if not specified in the request" — and
/// `RemoveNotificationChannel` addresses a channel by it.
#[derive(Debug, Clone)]
pub struct NotificationChannel {
    pub id: String,
    pub uri: String,
    pub event_publishers: Vec<String>,
}

impl NotificationChannel {
    /// This channel as the modelled shape every notification operation reports.
    pub fn to_modeled(&self) -> Channel {
        Channel {
            id: Some(self.id.clone()),
            uri: self.uri.clone(),
            event_publishers: self.event_publishers.clone(),
        }
    }
}

/// One user feedback submission about one anomaly instance, as `SubmitFeedback` filed it
/// against a profiling group.
///
/// `anomaly_instance_id` is the key the submission is filed under, so the latest
/// submission about one anomaly instance is the one this group holds — the real service
/// reports it on that instance's `userFeedback`, whose modelled `UserFeedback` carries a
/// single `type`, i.e. ONE feedback per instance rather than a history.
///
/// `comment` keeps the `@optional` member as sent (absent when the request carried none);
/// the modelled `UserFeedback` has no member for it, so it is stored and never reported.
#[derive(Debug, Clone)]
pub struct AnomalyFeedback {
    /// The `AnomalyInstanceId` the feedback is about — the request's own, whose modelled
    /// pattern (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`) the
    /// boundary walk has already enforced by the time it is stored.
    pub anomaly_instance_id: String,
    /// The modelled `FeedbackType` submitted: `Positive` or `Negative`.
    pub feedback_type: String,
    /// The optional free-text comment the submission carried.
    pub comment: Option<String>,
    /// When this submission was filed.
    pub submitted_at: DateTime,
}

/// One INSTANCE of a detected anomaly — "the specific duration in which the metric is
/// flagged as anomalous", as the modelled `AnomalyInstance` documents it — as a group's
/// anomaly catalogue holds it.
///
/// `id` is what `SubmitFeedback` addresses feedback to and what
/// [`ProfilingGroup::anomaly_instance`] resolves that `anomalyInstanceId` against. The
/// `userFeedback` the modelled instance reports is NOT held here but read from the group's
/// own [`AnomalyFeedback`], so a submission and the instance it is about cannot drift
/// apart.
#[derive(Debug, Clone)]
pub struct DetectedAnomalyInstance {
    /// The instance's UUID: "The universally unique identifier (UUID) of an instance of an
    /// anomaly in a metric", in the shape the `AnomalyInstanceId` a submission names
    /// carries (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`).
    pub id: String,
    /// Start of the period over which the metric was flagged as anomalous.
    pub start_time: DateTime,
    /// End of that period, or `None` while it has no recorded end (the modelled member is
    /// optional).
    pub end_time: Option<DateTime>,
}

impl DetectedAnomalyInstance {
    /// Whether the period this instance covers overlaps the closed interval
    /// `[start, end]` a `GetRecommendations` request asks the analysis about — the
    /// "requested period" the modelled `Anomaly$instances` is documented to list the
    /// instances "during".
    ///
    /// Both bound tests go through [`instant_key`], the way a findings report's window
    /// test does ([`FindingsReport::overlaps`]); an instance with no recorded end is still
    /// open, so it overlaps every interval that does not end before it began.
    pub fn overlaps(&self, start: DateTime, end: DateTime) -> bool {
        let ends_at_or_after_start = self
            .end_time
            .is_none_or(|at| instant_key(at) >= instant_key(start));
        ends_at_or_after_start && instant_key(self.start_time) <= instant_key(end)
    }
}

/// One anomaly a recommendation analysis detected in a profiling group's profiles, as that
/// group's catalogue holds it: the metric it was detected in, why it was flagged, and the
/// instances — the periods — it was anomalous over.
///
/// Anomalies are STORED, never derived from a request: this emulator runs no anomaly
/// detector over the agent profiles it holds (see `crate::ops::get_recommendations`), so
/// the only way one enters the store is
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_detected_anomaly`] — a
/// group no analysis has been recorded against holds none, which is why
/// `GetRecommendations` answers it with an empty `anomalies` list instead of an invented
/// finding, and why `SubmitFeedback` resolves an `anomalyInstanceId` against it rather
/// than accepting whatever id arrives.
#[derive(Debug, Clone)]
pub struct DetectedAnomaly {
    /// `Anomaly.metric.frameName`: "The name of the method that appears as a frame in any
    /// stack in a profile."
    pub frame_name: String,
    /// `Anomaly.metric.type`, one of the modelled `MetricType` values
    /// ([`crate::ops::types::METRIC_TYPE_VALUES`]).
    pub metric_type: String,
    /// `Anomaly.metric.threadStates`: the application runtime thread states the metric
    /// value was calculated over.
    pub thread_states: Vec<String>,
    /// `Anomaly.reason`: "The reason for which metric was flagged as anomalous."
    pub reason: String,
    /// The periods this anomaly was detected over, earliest recorded FIRST.
    pub instances: Vec<DetectedAnomalyInstance>,
}

/// One recommendation ("findings") report: the result of analysing ONE profile window
/// of one group, which `ListFindingsReports` and `GetFindingsReportAccountSummary`
/// summarise as a `FindingsReportSummary`.
///
/// Reports are STORED, never derived from a request: this emulator runs no
/// recommendation analysis of its own, so the only way one enters the store is
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_findings_report`] — a
/// group that has had no analysis recorded against it holds no reports, and
/// `ListFindingsReports` answers it with an empty list rather than with invented
/// findings.
#[derive(Debug, Clone)]
pub struct FindingsReport {
    /// `FindingsReportSummary.id`: the report's UUID (the member's modelled pattern is
    /// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`).
    pub id: String,
    /// Start of the profile window the analysis covered.
    pub profile_start_time: DateTime,
    /// End of the profile window the analysis covered.
    pub profile_end_time: DateTime,
    /// `FindingsReportSummary.totalNumberOfFindings`: "The total number of different
    /// recommendations that were found by the analysis."
    pub total_number_of_findings: i32,
    /// The `AggregationPeriod` of the profile analysed — what `dailyReportsOnly`
    /// selects on. `FindingsReportSummary` has no member for it, so it is stored and
    /// never reported.
    pub aggregation_period: String,
}

impl FindingsReport {
    /// The `FindingsReportSummary` a list operation reports for this stored report.
    /// `profilingGroupName` comes from the stored group, not from the request that is
    /// asking for it.
    pub fn summary(&self, group_name: &str) -> FindingsReportSummary {
        FindingsReportSummary {
            id: Some(self.id.clone()),
            profiling_group_name: Some(group_name.to_string()),
            profile_start_time: Some(self.profile_start_time),
            profile_end_time: Some(self.profile_end_time),
            total_number_of_findings: Some(self.total_number_of_findings),
        }
    }

    /// Whether this is a report over a daily profile — the reports
    /// `dailyReportsOnly=true` restricts a listing to.
    pub fn is_daily(&self) -> bool {
        self.aggregation_period == DAILY_AGGREGATION_PERIOD
    }

    /// Whether the profile window this report analysed overlaps the closed interval
    /// `[start, end]` — the time range a listing asks about.
    ///
    /// Both bound tests go through [`instant_key`], so a window that ends a fraction of
    /// a microsecond before the range starts (or begins a fraction of a microsecond
    /// after it ends) is excluded rather than collapsing onto the bound.
    pub fn overlaps(&self, start: DateTime, end: DateTime) -> bool {
        instant_key(self.profile_end_time) >= instant_key(start)
            && instant_key(self.profile_start_time) <= instant_key(end)
    }
}

/// A profiling group, as Create/ConfigureAgent/PostAgentProfile left it.
#[derive(Debug, Clone)]
pub struct ProfilingGroup {
    pub name: String,
    pub arn: String,
    /// The `clientToken` of the `CreateProfilingGroup` call that created this group: its
    /// idempotency record, so a retry of THAT create replays this group instead of
    /// conflicting with it ([`ProfilingGroup::was_created_by`]).
    ///
    /// It lives ON the group, which is what makes the record outlive every later create:
    /// a create writes only the record of the group it added, so no call can rewrite or
    /// drop another create's record, and deleting the group takes its record with it —
    /// the token of a group that no longer exists replays nothing.
    pub create_token: String,
    pub compute_platform: String,
    /// `agentOrchestrationConfig.profilingEnabled`.
    pub profiling_enabled: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: HashMap<String, String>,
    /// `profilingStatus.latestAgentOrchestratedAt` — set by `ConfigureAgent`.
    pub latest_agent_orchestrated_at: Option<DateTime>,
    /// `profilingStatus.latestAgentProfileReportedAt` — set by `PostAgentProfile`.
    pub latest_agent_profile_reported_at: Option<DateTime>,
    /// Posted agent profiles, oldest FIRST — a `VecDeque`, so the oldest-first
    /// eviction the retention bound performs is structural and O(1). Bounded: at most
    /// [`MAX_RETAINED_PROFILES_PER_GROUP`] profiles and
    /// [`MAX_RETAINED_PROFILE_BYTES_PER_GROUP`] bytes are retained per group, and the
    /// store-wide [`MAX_RETAINED_PROFILE_BYTES`] budget bounds every group together —
    /// write them only through
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`], which is
    /// what applies those bounds.
    pub profiles: VecDeque<AgentProfile>,
    /// `PostAgentProfile` idempotency tokens already honoured for this group, oldest
    /// first. Bounded at [`MAX_RETAINED_PROFILE_TOKENS_PER_GROUP`] by
    /// [`ProfilingGroup::remember_profile_token`], which is the only writer.
    pub seen_profile_tokens: VecDeque<String>,
    /// This group's anomaly-notification channels, in the order they were added —
    /// what `GetNotificationConfiguration` reports, `AddNotificationChannels` stores
    /// into and `RemoveNotificationChannel` deletes from. At most one channel per
    /// `uri`, and bounded at [`MAX_NOTIFICATION_CHANNELS_PER_GROUP`], by
    /// [`ProfilingGroup::store_notification_channels`], which is the only writer.
    pub notification_channels: Vec<NotificationChannel>,
    /// The group's resource-based policy, or `None` while it has none — a group is
    /// created without one, and the permission writers
    /// ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`] and
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`], which
    /// are what enforce the revision check) attach and clear it. It is `Some` exactly
    /// while the policy holds at least one statement, since the removal of the last
    /// one deletes the policy. It lives ON the group so deleting the group takes its
    /// policy with it and a re-created group starts with none. Read it through
    /// [`ProfilingGroup::resource_policy`].
    pub policy: Option<ResourcePolicy>,
    /// The recommendation reports recorded against this group, oldest recorded FIRST.
    /// Bounded at [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`] by
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_findings_report`],
    /// which is the only writer.
    pub findings_reports: VecDeque<FindingsReport>,
    /// The anomalies a recommendation analysis has detected in this group's profiles,
    /// oldest recorded FIRST. Bounded at [`MAX_RETAINED_ANOMALIES_PER_GROUP`] by
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_detected_anomaly`],
    /// which is the only writer. It is the catalogue `GetRecommendations` reports from
    /// ([`ProfilingGroup::anomalies_in`]) and the one `SubmitFeedback` resolves an
    /// `anomalyInstanceId` against ([`ProfilingGroup::anomaly_instance`]), so those two
    /// operations agree about which anomaly instances exist. It lives ON the group — an
    /// anomaly is detected in THIS group's profiles — so deleting the group takes its
    /// anomalies with it and a re-created group starts with none.
    pub detected_anomalies: VecDeque<DetectedAnomaly>,
    /// The anomaly feedback submitted against this group, least recently submitted
    /// FIRST and at most one entry per anomaly instance. Bounded at
    /// [`MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP`] by
    /// [`ProfilingGroup::submit_anomaly_feedback`], which is the only writer. It lives ON
    /// the group — the feedback is about an anomaly of THIS group's analysis — so
    /// deleting the group takes it with it and a re-created group starts with none.
    pub anomaly_feedback: VecDeque<AnomalyFeedback>,
    /// What each profiling instance reported about itself with its latest
    /// `ConfigureAgent` call, least recently reporting instance FIRST and at most one
    /// record per instance. Bounded at [`MAX_RETAINED_AGENT_REPORTS_PER_GROUP`] by
    /// [`ProfilingGroup::record_agent_report`], which is the only writer. It lives ON the
    /// group — an agent reports to the group it is profiling — so deleting the group takes
    /// its reports with it and a re-created group starts with none.
    pub agent_reports: VecDeque<AgentReport>,
}

impl ProfilingGroup {
    /// Whether this group is the one the create carrying `client_token` created — i.e.
    /// whether a create presenting that token is a RETRY of the create that stored this
    /// group, which `CreateProfilingGroup` replays rather than refusing.
    pub fn was_created_by(&self, client_token: &str) -> bool {
        self.create_token == client_token
    }

    /// The `ProfilingGroupDescription` every READ of a stored profiling group answers with
    /// — `DescribeProfilingGroup`, `UpdateProfilingGroup` and the described form of
    /// `ListProfilingGroups` — built from what is stored, never from the request that is
    /// asking for it.
    ///
    /// These paths report the `profilingStatus` CONTAINER even while the group has no
    /// activity to report in it (its three members are each absent until the activity they
    /// name happens); the CREATE path is the one that omits the container, see
    /// [`ProfilingGroup::created_description`].
    ///
    /// `at` is the instant the description is reported as of: part of the status
    /// ([`ProfilingGroup::latest_aggregated_profile`]) is a fact the CLOCK makes true, so
    /// every read states which instant it answers for instead of each path reading its own.
    pub fn description(&self, at: DateTime) -> ProfilingGroupDescription {
        ProfilingGroupDescription {
            name: Some(self.name.clone()),
            agent_orchestration_config: Some(AgentOrchestrationConfig {
                profiling_enabled: self.profiling_enabled,
            }),
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            profiling_status: Some(self.profiling_status(at)),
            compute_platform: Some(self.compute_platform.clone()),
            tags: Some(self.tags.clone()),
        }
    }

    /// The description `CreateProfilingGroup` answers with: [`description`] with
    /// `profilingStatus` ABSENT while the group has no profiling activity.
    ///
    /// `profilingStatus` is not `@required`, and each of its members reports one thing that
    /// has HAPPENED to the group — the last time an agent pinged back, the last time a
    /// profile was received, the most recent aggregated profile. A create reports only what
    /// the create established, and nothing can have been reported against a group that is
    /// only now coming into existence, so the member is left out of that payload entirely
    /// rather than written as an empty object: the way a client tells "nothing has ever
    /// been reported" from "activity, reported here" is whether the key is there at all,
    /// and an empty object answers that question wrongly. A create replayed on its
    /// `clientToken` against a group that HAS since reported carries the status, because by
    /// then there is something to report.
    ///
    /// [`description`]: ProfilingGroup::description
    pub fn created_description(&self, at: DateTime) -> ProfilingGroupDescription {
        ProfilingGroupDescription {
            profiling_status: self
                .has_profiling_activity(at)
                .then(|| self.profiling_status(at)),
            ..self.description(at)
        }
    }

    /// The group's `profilingStatus` as of `at`: each member present only once the activity
    /// it names has happened.
    fn profiling_status(&self, at: DateTime) -> ProfilingStatus {
        ProfilingStatus {
            latest_agent_profile_reported_at: self.latest_agent_profile_reported_at,
            latest_aggregated_profile: self.latest_aggregated_profile(at),
            latest_agent_orchestrated_at: self.latest_agent_orchestrated_at,
        }
    }

    /// The MOST RECENT aggregated profile this group holds as of `at`, as the
    /// `AggregatedProfileTime` pair (`start`, `period`) the status reports it by — or
    /// `None` while no aggregated profile covers any profile the group holds.
    ///
    /// A posted profile belongs to one aligned window of EVERY period the service
    /// aggregates at ([`AGGREGATION_PERIODS`]), and
    /// [`ProfilingGroup::aggregated_profile_starts`] is what derives those windows for one
    /// period — each gated by that period's OWN aggregation, so a window still collecting
    /// is among no period's aggregated profiles. The most recent of those aggregated
    /// profiles is the one whose aggregation period STARTS latest, and — when two periods
    /// start their window at the same instant, which happens where their alignments
    /// coincide — the narrowest of them, i.e. the smallest span of time whose aggregate
    /// covers the group's latest aggregated profile.
    pub fn latest_aggregated_profile(&self, at: DateTime) -> Option<AggregatedProfileTime> {
        AGGREGATION_PERIODS
            .iter()
            .filter_map(|(period, period_seconds)| {
                self.aggregated_profile_starts(*period_seconds, at)
                    .last()
                    .map(|start| (*start, *period_seconds, *period))
            })
            .max_by_key(|(start, period_seconds, _)| {
                (instant_key(*start), ::std::cmp::Reverse(*period_seconds))
            })
            .map(|(start, _, period)| AggregatedProfileTime {
                start: Some(start),
                period: Some(period.to_string()),
            })
    }

    /// Whether anything has ever been reported about this group's profiling as of `at`: an
    /// agent ping (`ConfigureAgent`), a posted profile (`PostAgentProfile`) or an aggregated
    /// profile. This is what decides whether a create's payload carries a `profilingStatus`
    /// at all.
    fn has_profiling_activity(&self, at: DateTime) -> bool {
        let status = self.profiling_status(at);
        status.latest_agent_profile_reported_at.is_some()
            || status.latest_agent_orchestrated_at.is_some()
            || status.latest_aggregated_profile.is_some()
    }

    /// Store a posted profile and record its receipt time, within this group's
    /// retention bound: once the group holds more than
    /// [`MAX_RETAINED_PROFILES_PER_GROUP`] profiles or
    /// [`MAX_RETAINED_PROFILE_BYTES_PER_GROUP`] bytes, the oldest are dropped until it
    /// does not. The profile just posted is never the one evicted, so a post is always
    /// readable back. Call it through
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`], which also
    /// applies the store-wide byte budget.
    fn record_profile(&mut self, profile: AgentProfile) {
        self.latest_agent_profile_reported_at = Some(profile.received_at);
        self.profiles.push_back(profile);
        let mut retained_bytes: usize = self
            .profiles
            .iter()
            .map(|profile| profile.bytes.len())
            .sum();
        while self.profiles.len() > 1
            && (self.profiles.len() > MAX_RETAINED_PROFILES_PER_GROUP
                || retained_bytes > MAX_RETAINED_PROFILE_BYTES_PER_GROUP)
        {
            match self.profiles.pop_front() {
                Some(evicted) => retained_bytes -= evicted.bytes.len(),
                None => break,
            }
        }
    }

    /// Remember a `PostAgentProfile` idempotency token, within
    /// [`MAX_RETAINED_PROFILE_TOKENS_PER_GROUP`]: `true` when the token is new (so the
    /// post must be stored), `false` when it is a replay of one still remembered.
    fn remember_profile_token(&mut self, token: &str) -> bool {
        if self.seen_profile_tokens.iter().any(|seen| seen == token) {
            return false;
        }
        while self.seen_profile_tokens.len() >= MAX_RETAINED_PROFILE_TOKENS_PER_GROUP {
            self.seen_profile_tokens.pop_front();
        }
        self.seen_profile_tokens.push_back(token.to_string());
        true
    }

    /// The notification configuration every notification operation answers with,
    /// built from the channels stored on THIS group.
    ///
    /// `channels` is always present, as an empty list when the group has none: that is
    /// what distinguishes a group whose notifications nobody has configured from one
    /// whose configuration is unknown (which is a 404 instead).
    pub fn notification_configuration(&self) -> NotificationConfiguration {
        NotificationConfiguration {
            channels: Some(
                self.notification_channels
                    .iter()
                    .map(NotificationChannel::to_modeled)
                    .collect(),
            ),
        }
    }

    /// Whether this group already holds a channel with `id`.
    pub fn has_notification_channel(&self, id: &str) -> bool {
        self.notification_channels
            .iter()
            .any(|channel| channel.id == id)
    }

    /// The channel this group holds for `uri`, or `None` when no stored channel reports
    /// to it.
    ///
    /// A `uri` identifies at most one channel of a configuration: it is the destination
    /// the group's anomaly notifications are published to, and a configuration holding
    /// two channels for one destination would publish every notification to it twice.
    pub fn notification_channel_for_uri(&self, uri: &str) -> Option<&NotificationChannel> {
        self.notification_channels
            .iter()
            .find(|channel| channel.uri == uri)
    }

    /// Whether this group already holds a channel reporting to `uri`.
    pub fn has_notification_channel_for_uri(&self, uri: &str) -> bool {
        self.notification_channel_for_uri(uri).is_some()
    }

    /// Store a batch of notification channels, ALL or nothing: `false` — and nothing
    /// stored — unless every channel fits.
    ///
    /// A channel whose `uri` a stored channel (or an earlier channel of the same batch)
    /// already reports to REPLACES that channel in place, keeping its position and
    /// taking the id, uri and publishers of the replacement: one destination holds one
    /// channel, so storing a destination the group already has is idempotent at the uri
    /// level and consumes none of the group's channel capacity. Every other channel is
    /// appended.
    ///
    /// It is refused when a channel's id names a DIFFERENT stored channel (one for
    /// another uri) or repeats an id of another channel of the batch for another uri, or
    /// when the channels that are net new to this group would push it past
    /// [`MAX_NOTIFICATION_CHANNELS_PER_GROUP`]. The only writer, so the stored
    /// configuration never leaves what the model's `Channels` list allows, never holds
    /// two channels for one uri, and a refused store is never a partial one.
    pub fn store_notification_channels(&mut self, channels: Vec<NotificationChannel>) -> bool {
        // What the configuration's uris would be once the whole batch is stored: a uri
        // already covered replaces, so only a uri nobody covers yet grows the group.
        let mut uris: Vec<&str> = self
            .notification_channels
            .iter()
            .map(|channel| channel.uri.as_str())
            .collect();
        for channel in &channels {
            if !uris.contains(&channel.uri.as_str()) {
                uris.push(channel.uri.as_str());
            }
        }
        if uris.len() > MAX_NOTIFICATION_CHANNELS_PER_GROUP {
            return false;
        }
        let repeated = channels.iter().enumerate().any(|(index, channel)| {
            self.notification_channels.iter().any(|stored| {
                stored.id == channel.id && stored.uri != channel.uri
            }) || channels[..index]
                .iter()
                .any(|earlier| earlier.id == channel.id && earlier.uri != channel.uri)
        });
        if repeated {
            return false;
        }
        for channel in channels {
            match self
                .notification_channels
                .iter_mut()
                .find(|stored| stored.uri == channel.uri)
            {
                Some(stored) => *stored = channel,
                None => self.notification_channels.push(channel),
            }
        }
        true
    }

    /// Remove the channel with `id`, answering whether there was one.
    pub fn remove_notification_channel(&mut self, id: &str) -> bool {
        let before = self.notification_channels.len();
        self.notification_channels
            .retain(|channel| channel.id != id);
        self.notification_channels.len() != before
    }

    /// The anomaly instance this group's catalogue holds under `anomaly_instance_id`, or
    /// `None` when no anomaly detected in this group has an instance with that id.
    ///
    /// This is the lookup `SubmitFeedback` resolves its `anomalyInstanceId` against: an
    /// id the group's analysis does not know names no instance to file feedback about,
    /// whatever period a reader asked about, because feedback is submitted about the
    /// INSTANCE and not about a window.
    pub fn anomaly_instance(&self, anomaly_instance_id: &str) -> Option<&DetectedAnomalyInstance> {
        self.detected_anomalies
            .iter()
            .flat_map(|anomaly| anomaly.instances.iter())
            .find(|instance| instance.id == anomaly_instance_id)
    }

    /// The anomalies of this group's analysis that the closed interval `[start, end]`
    /// covers, as the modelled `Anomaly` `GetRecommendations` reports them, oldest
    /// recorded first.
    ///
    /// An anomaly is reported with exactly the instances whose own period overlaps the
    /// requested one — `Anomaly$instances` is "a list of the instances of the detected
    /// anomalies during the requested period" — so an anomaly none of whose instances the
    /// interval covers is not reported at all. Each reported instance carries the
    /// `userFeedback` this group holds about it ([`Self::feedback_about`]), which is where
    /// a `SubmitFeedback` submission becomes readable.
    pub fn anomalies_in(&self, start: DateTime, end: DateTime) -> Vec<Anomaly> {
        self.detected_anomalies
            .iter()
            .filter_map(|anomaly| {
                let instances: Vec<AnomalyInstance> = anomaly
                    .instances
                    .iter()
                    .filter(|instance| instance.overlaps(start, end))
                    .map(|instance| AnomalyInstance {
                        id: instance.id.clone(),
                        start_time: instance.start_time,
                        end_time: instance.end_time,
                        user_feedback: self.feedback_about(&instance.id).map(|feedback| {
                            UserFeedback {
                                r#type: feedback.feedback_type.clone(),
                            }
                        }),
                    })
                    .collect();
                (!instances.is_empty()).then(|| Anomaly {
                    metric: Metric {
                        frame_name: anomaly.frame_name.clone(),
                        r#type: anomaly.metric_type.clone(),
                        thread_states: anomaly.thread_states.clone(),
                    },
                    reason: anomaly.reason.clone(),
                    instances,
                })
            })
            .collect()
    }

    /// The feedback this group holds about one anomaly instance, or `None` when none has
    /// been submitted about it (or the submission has since been evicted).
    ///
    /// What reports it back is [`Self::anomalies_in`], which carries it on the modelled
    /// `AnomalyInstance`'s `userFeedback` — so a submission about an instance of an
    /// anomaly recorded against this group is read back through `GetRecommendations`.
    pub fn feedback_about(&self, anomaly_instance_id: &str) -> Option<&AnomalyFeedback> {
        self.anomaly_feedback
            .iter()
            .find(|feedback| feedback.anomaly_instance_id == anomaly_instance_id)
    }

    /// File `feedback` against this group, superseding what it held about that anomaly
    /// instance.
    ///
    /// One anomaly instance carries one feedback — the modelled `UserFeedback` reported on
    /// an `AnomalyInstance` has a single `type` — so a further submission about the same
    /// instance replaces the one stored rather than accumulating beside it, and becomes
    /// this group's most recent submission. The only writer, so the group never holds two
    /// submissions about one instance, nor more than
    /// [`MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP`] of them: once past that bound the least
    /// recently submitted is dropped, never the submission just filed.
    pub fn submit_anomaly_feedback(&mut self, feedback: AnomalyFeedback) {
        if let Some(at) = self
            .anomaly_feedback
            .iter()
            .position(|stored| stored.anomaly_instance_id == feedback.anomaly_instance_id)
        {
            self.anomaly_feedback.remove(at);
        }
        self.anomaly_feedback.push_back(feedback);
        while self.anomaly_feedback.len() > MAX_RETAINED_ANOMALY_FEEDBACK_PER_GROUP {
            self.anomaly_feedback.pop_front();
        }
    }

    /// What the profiling instance identified by `fleet_instance_id` reported about itself
    /// most recently, or `None` when this group holds no report under that identity (or it
    /// has since been evicted). `None` asks about the reports that identified no instance.
    pub fn agent_report(&self, fleet_instance_id: Option<&str>) -> Option<&AgentReport> {
        self.agent_reports
            .iter()
            .find(|report| report.fleet_instance_id.as_deref() == fleet_instance_id)
    }

    /// Record what one profiling instance reports about itself, and the instant it
    /// reported — the state half of a `ConfigureAgent` call.
    ///
    /// A report states the instance's CURRENT state, so a further report from the same
    /// instance REPLACES the record held for it rather than accumulating beside it, and
    /// becomes this group's most recent report; a report that identifies no instance is
    /// one such identity of its own. `latestAgentOrchestratedAt` — "the date and time when
    /// the profiling agent most recently pinged back" — is the instant of the most recent
    /// report from any instance, so it moves with every report. The only writer, so the
    /// group never holds two records for one instance, nor more than
    /// [`MAX_RETAINED_AGENT_REPORTS_PER_GROUP`] of them or
    /// [`MAX_RETAINED_AGENT_REPORT_BYTES_PER_GROUP`] of reported text: once past either
    /// bound the least recently reporting instance is dropped first, never the report just
    /// made.
    pub fn record_agent_report(&mut self, report: AgentReport) {
        if let Some(at) = self
            .agent_reports
            .iter()
            .position(|stored| stored.fleet_instance_id == report.fleet_instance_id)
        {
            self.agent_reports.remove(at);
        }
        self.latest_agent_orchestrated_at = Some(report.reported_at);
        self.agent_reports.push_back(report);
        let mut retained_bytes: usize = self
            .agent_reports
            .iter()
            .map(AgentReport::reported_bytes)
            .sum();
        while self.agent_reports.len() > 1
            && (self.agent_reports.len() > MAX_RETAINED_AGENT_REPORTS_PER_GROUP
                || retained_bytes > MAX_RETAINED_AGENT_REPORT_BYTES_PER_GROUP)
        {
            match self.agent_reports.pop_front() {
                Some(evicted) => retained_bytes -= evicted.reported_bytes(),
                None => break,
            }
        }
    }

    /// The start instants of the aggregated profiles this group holds for an
    /// aggregation period `period_seconds` long, as of `at`, ASCENDING and distinct.
    ///
    /// The `GetProfile` doc fixes the rule: "Amazon CodeGuru Profiler collects posted
    /// agent profiles for a profiling group into aggregated profiles", at periods
    /// "aligned to UTC". So each posted profile belongs to the one aligned window that
    /// contains the period its data covers ([`AgentProfile::period_start`]), and every
    /// window holding at least one posted profile
    /// whose aggregation IN THAT WINDOW is due is exactly one available aggregated
    /// profile, reported by its window start. A window the service is still collecting
    /// into — one that has not closed, or whose aggregation pass is not through yet
    /// ([`AgentProfile::is_aggregated`] for this very period) — is covered by no
    /// aggregated profile, so it contributes no profile time however many profiles it
    /// holds and however long their finer windows have been closed.
    pub fn aggregated_profile_starts(&self, period_seconds: i64, at: DateTime) -> Vec<DateTime> {
        let mut starts: Vec<i64> = self
            .profiles
            .iter()
            .filter(|profile| profile.is_aggregated(period_seconds, at))
            .map(|profile| aggregation_window_start(profile.period_start, period_seconds).secs())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        starts.into_iter().map(DateTime::from_secs).collect()
    }

    /// Every profile this group retains whose DATA falls in the HALF-OPEN window
    /// `[start, end)` ([`AgentProfile::period_start`]), oldest first.
    ///
    /// Half-open at the end, like every other window this service steps a time axis with
    /// ([`aggregated_profile_starts`] reports one aggregated profile per aligned window),
    /// so back-to-back windows hold each profile exactly once. Aggregated or not: a
    /// caller that reports only AGGREGATED data asks [`AgentProfile::is_aggregated`],
    /// while a caller that has to tell "no data in this window" from "data not collected
    /// yet" needs to see both.
    ///
    /// Instants are compared through [`instant_key`], so data a fraction of a
    /// microsecond inside a bound still falls on the right side of it.
    ///
    /// [`aggregated_profile_starts`]: ProfilingGroup::aggregated_profile_starts
    pub fn profiles_in(&self, start: DateTime, end: DateTime) -> Vec<&AgentProfile> {
        let (lower, upper) = (instant_key(start), instant_key(end));
        self.profiles
            .iter()
            .filter(|profile| {
                let at = instant_key(profile.period_start);
                at >= lower && at < upper
            })
            .collect()
    }

    /// This group's resource-based policy, or `None` when it has none.
    ///
    /// A policy exists exactly while it holds at least one statement: it comes into
    /// being with the write that grants the first one, and the removal that takes the
    /// last one takes the policy with it
    /// ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`]), so
    /// the group is policy-less again — the same state it was created in. Deleting the
    /// group also takes whatever policy it held.
    pub fn resource_policy(&self) -> Option<&ResourcePolicy> {
        self.policy.as_ref()
    }

    /// The span the AGGREGATED profiles this group retains inside `[start, end]` cover
    /// as of `at`, as (earliest data start, latest data start), or `None` when it
    /// retains no such profile in that window. Both bounds are inclusive, and the answer
    /// is derived from the periods the stored profiles cover
    /// ([`AgentProfile::period_start`]) and `at` alone — this is what
    /// `GetRecommendations` reports as the span its analysis is about.
    ///
    /// A profile whose aggregation is not due yet ([`AgentProfile::is_aggregated`]) is in
    /// no aggregated profile, so it is in no analysed span either — the same gate
    /// [`ProfilingGroup::aggregated_profiles_in`] and
    /// [`ProfilingGroup::aggregated_profile_starts`] apply, so one instant's reads of a
    /// group's profile data agree about which profiles exist. The range a request names
    /// here is FREE-FORM — it names no aggregation period — so the window a profile is
    /// asked about in is the finest one it is collected into
    /// ([`finest_aggregation_period_seconds`]), the resolution the data of an arbitrary
    /// range is assembled from.
    ///
    /// Instants are compared through [`instant_key`], so two instants a fraction of
    /// a microsecond apart still order exactly.
    pub fn profile_span_in(
        &self,
        start: DateTime,
        end: DateTime,
        at: DateTime,
    ) -> Option<(DateTime, DateTime)> {
        let (lower, upper) = (instant_key(start), instant_key(end));
        let period_seconds = finest_aggregation_period_seconds();
        let mut span: Option<(DateTime, DateTime)> = None;
        for profile in &self.profiles {
            let covered = instant_key(profile.period_start);
            if covered < lower || covered > upper || !profile.is_aggregated(period_seconds, at) {
                continue;
            }
            span = Some(match span {
                None => (profile.period_start, profile.period_start),
                Some((earliest, latest)) => (
                    if covered < instant_key(earliest) {
                        profile.period_start
                    } else {
                        earliest
                    },
                    if covered > instant_key(latest) {
                        profile.period_start
                    } else {
                        latest
                    },
                ),
            });
        }
        span
    }

    /// This group's recorded findings reports that `keep` selects, in the stable order
    /// every report listing reports them in: by analysed window, oldest first, then by
    /// report id, so paging a listing visits every report exactly once however the
    /// store was written.
    fn ordered_findings_reports(
        &self,
        keep: impl Fn(&FindingsReport) -> bool,
    ) -> Vec<&FindingsReport> {
        let mut reports: Vec<&FindingsReport> = self
            .findings_reports
            .iter()
            .filter(|report| keep(report))
            .collect();
        // Instants are ordered through `instant_key`, exact at nanosecond resolution, so
        // two analysed windows starting a fraction of a microsecond apart still order by
        // window rather than falling through to the id tie-break.
        reports.sort_by(|left, right| {
            instant_key(left.profile_start_time)
                .cmp(&instant_key(right.profile_start_time))
                .then_with(|| left.id.cmp(&right.id))
        });
        reports
    }

    /// This group's recorded findings reports that a listing of `[start, end]` reports:
    /// those whose analysed profile window overlaps that range, and — when
    /// `daily_only` — only those over a daily (`P1D`) profile, in
    /// [`ProfilingGroup::ordered_findings_reports`]'s stable order.
    pub fn findings_reports_in(
        &self,
        start: DateTime,
        end: DateTime,
        daily_only: bool,
    ) -> Vec<&FindingsReport> {
        self.ordered_findings_reports(|report| {
            report.overlaps(start, end) && (!daily_only || report.is_daily())
        })
    }

    /// EVERY findings report recorded against this group, in the same stable order,
    /// restricted to reports over daily (`P1D`) profiles when `daily_only`. The
    /// account-wide summary (`GetFindingsReportAccountSummary`) asks about no time
    /// range, so it reports a group's reports whatever window they analysed.
    pub fn findings_reports(&self, daily_only: bool) -> Vec<&FindingsReport> {
        self.ordered_findings_reports(|report| !daily_only || report.is_daily())
    }

    /// EVERY AGGREGATED profile whose DATA falls in `[start, end]` (either bound
    /// absent means unbounded) as of `at`, oldest first.
    ///
    /// All of them, because the aggregated profile of a range is what the profiles of
    /// that range were collected INTO: "Amazon CodeGuru Profiler collects posted agent
    /// profiles for a profiling group into aggregated profiles", and an aggregation
    /// period's profile "contains profiles posted by the agent starting at this time for
    /// an aggregation period specified by the period property". So a reader of the
    /// aggregated data of a range reads every profile the range holds — the same set
    /// [`ProfilingGroup::aggregated_profile_starts`] collapses into one profile time per
    /// aligned window and [`ProfilingGroup::profiles_in`] hands a step's reader — rather
    /// than one profile picked out of them.
    ///
    /// A profile the service has received but not yet collected into an aggregated
    /// profile ([`AgentProfile::is_aggregated`]) is in no aggregated profile, so it is in
    /// nothing this reports: the operations that report aggregated data report the same
    /// absence for it. The bounds here name a FREE-FORM range and no aggregation period,
    /// so the window a profile is asked about in is the finest one it is collected into
    /// ([`finest_aggregation_period_seconds`]) — the resolution the data of an arbitrary
    /// range is assembled from, as opposed to
    /// [`ProfilingGroup::aggregated_profile_starts`], which is asked about one period's
    /// own windows and gates each by that period.
    ///
    /// Both inclusive-bound tests go through [`instant_key`], so data a
    /// fraction of a microsecond outside the requested window is excluded rather than
    /// comparing equal to the bound.
    pub fn aggregated_profiles_in(
        &self,
        start: Option<DateTime>,
        end: Option<DateTime>,
        at: DateTime,
    ) -> Vec<&AgentProfile> {
        let (lower, upper) = (start.map(instant_key), end.map(instant_key));
        let period_seconds = finest_aggregation_period_seconds();
        self.profiles
            .iter()
            .filter(|profile| {
                let covered = instant_key(profile.period_start);
                profile.is_aggregated(period_seconds, at)
                    && lower.is_none_or(|s| covered >= s)
                    && upper.is_none_or(|e| covered <= e)
            })
            .collect()
    }

    /// This group's LATEST aggregated profile of a `span`-long aggregation period as of
    /// `at`, as (the start of the aligned window it covers, the profiles that window
    /// holds, oldest first) — or `None` when the group holds no aggregated profile at
    /// all.
    ///
    /// This is what a request that names a window's LENGTH and neither of its bounds asks
    /// for: a length says how long the aggregated profile wanted is, not where it sits, and
    /// the `GetProfile` doc fixes which one is then served — "If you specify only period,
    /// the latest aggregated profile is returned." A posted profile belongs to the one
    /// window of that length containing the period its data covers
    /// ([`aligned_window_start`] over [`AgentProfile::period_start`] — the same
    /// alignment [`ProfilingGroup::aggregated_profile_starts`] reports a profile time by),
    /// so the latest aggregated profile is the one of the LAST such window, and not the
    /// profiles of every window the group retains. The window start is answered beside
    /// them because an aggregated profile states the window it covers, which is that
    /// window and not the data of any one profile in it.
    ///
    /// Windows are compared through [`instant_key`], exactly, so two windows a fraction of
    /// a microsecond apart — which a sub-second length can name — still order by window.
    pub fn latest_aggregated_window(
        &self,
        span: Duration,
        at: DateTime,
    ) -> Option<(DateTime, Vec<&AgentProfile>)> {
        let aggregated = self.aggregated_profiles_in(None, None, at);
        let window = |profile: &AgentProfile| aligned_window_start(profile.period_start, span);
        let latest = aggregated
            .iter()
            .map(|profile| instant_key(window(profile)))
            .max()?;
        let held: Vec<&AgentProfile> = aggregated
            .into_iter()
            .filter(|profile| instant_key(window(profile)) == latest)
            .collect();
        let start = held.first().map(|profile| window(profile))?;
        Some((start, held))
    }
}

/// Per-service state for com-amazonaws-codeguruprofiler-codeguruprofiler. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeguruprofilerCodeguruprofilerState {
    /// `profilingGroupName` -> the group.
    pub groups: HashMap<String, ProfilingGroup>,
    /// How many policy revisions this store has minted. Bumped by
    /// [`Self::mint_revision_id`], the only minter, and mixed into every revision it
    /// derives, so two writes of the same grants never carry the same revision.
    pub revisions_minted: u64,
    /// How many notification-channel ids this store has minted. Bumped by
    /// [`Self::mint_channel_id`], the only minter, and mixed into every id it derives,
    /// so two channels added in the same instant never carry the same id.
    pub channel_ids_minted: u64,
}

/// What [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`] did with a
/// posted profile.
#[derive(Debug, PartialEq, Eq)]
pub enum PostedProfile {
    /// Stored against the group (within the retention bounds).
    Stored,
    /// A replay of a `profileToken` the group still remembers: nothing was stored.
    Replayed,
    /// There is no such profiling group.
    UnknownGroup,
}

/// Why a policy write did not match the policy's current revision — every case a
/// conflict, since each says the caller is writing against a revision that is not the
/// one stored.
///
/// Each case carries the revision ids the conflict is ABOUT: the one the request named,
/// where it named one, and the one the stored policy carries, where there is a stored
/// policy. A revision-conflict refusal is self-healing only if it hands the caller the
/// revision to retry against, so the ids travel with the mismatch rather than being
/// left for a second `GetPolicy` round trip (see [`revision_conflict_message`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionMismatch {
    /// A policy already exists, so the write must name the revision it updates:
    /// `PutPermission`'s doc, "Subsequent calls must provide a revisionId to specify
    /// which revision of the resource-based policy to add the permissions to."
    Missing {
        /// The revision the stored policy carries — the one the next write must name.
        current: String,
    },
    /// The write named a revision, but the group has no policy yet:
    /// `PutPermission`'s doc, "The first time you call PutPermission on a profiling
    /// group, do not specify a revisionId because it doesn't have a resource-based
    /// policy."
    Unexpected {
        /// The revision the request named, which identifies no policy.
        requested: String,
    },
    /// The write named a revision other than the current one — a stale revision.
    Stale {
        /// The revision the request named, which the policy has moved past.
        requested: String,
        /// The revision the stored policy carries now — the one to retry against.
        current: String,
    },
}

/// Why a policy write would have left a policy larger than the service holds — the
/// store's own bounds on a resource-based policy, named so the refusal can say which
/// one was reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyLimit {
    /// The grant named more principals than [`MAX_PRINCIPALS_PER_STATEMENT`].
    Principals { requested: usize, limit: usize },
    /// The write would have left the policy holding statements for more action groups
    /// than [`MAX_POLICY_STATEMENTS`].
    Statements { requested: usize, limit: usize },
}

/// What a policy write ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`]
/// or [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`]) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyWrite {
    /// Applied, and the group still holds a policy: the document that policy now
    /// carries, and the revision the write minted — the revision the next write to it
    /// must name.
    Written { policy: String, revision_id: String },
    /// Applied, and the group holds NO policy any more: the write took the policy's
    /// last statement, and a policy exists only while it holds one, so the policy was
    /// deleted. There is no document and no revision to report — nothing is stored for
    /// either, and no revision was minted, a deleted policy having none to be at.
    Deleted,
    /// There is no such profiling group.
    UnknownGroup,
    /// The group has no resource-based policy at all, so there is nothing to remove
    /// from — whether none was ever created, or the removal of its last statement
    /// deleted it.
    NoPolicy,
    /// The group HAS a resource-based policy, but it holds no statement for the named
    /// action group — a different state from [`PolicyWrite::NoPolicy`], and a different
    /// thing to say about it.
    NoStatement,
    /// The write was refused against the stored revision; nothing changed.
    RevisionMismatch(RevisionMismatch),
    /// The write was refused because it would have left the policy past one of the
    /// store's bounds; nothing changed.
    LimitExceeded(PolicyLimit),
}

impl ComAmazonawsCodeguruprofilerCodeguruprofilerState {
    /// The `RevisionId` the next policy write carries: the store's mint count is bumped
    /// first, so every revision this store hands out is distinct even when two writes
    /// grant exactly the same principals. The ONLY minter.
    fn mint_revision_id(&mut self, group_name: &str, action_group: &str) -> String {
        self.revisions_minted = self.revisions_minted.wrapping_add(1);
        revision_id(self.revisions_minted, now(), group_name, action_group)
    }

    /// The `ChannelId` the next notification channel the caller did not name one for
    /// carries: the store's own mint count is bumped first, so every id this store
    /// hands out is distinct even for two channels added in the same instant. The ONLY
    /// minter.
    pub fn mint_channel_id(&mut self, group_name: &str) -> String {
        self.channel_ids_minted = self.channel_ids_minted.wrapping_add(1);
        channel_id(self.channel_ids_minted, now(), group_name)
    }

    /// Grant `action_group`'s permissions to exactly `principals` on `group_name`,
    /// under optimistic locking on `revision_id`.
    ///
    /// The principal set REPLACES whatever that action group last granted (the write
    /// is a `PUT` of the action group's statement, not a merge), duplicate principals
    /// in one request collapse to one, and a successful write mints a new revision —
    /// so the revision a caller holds is good for exactly one write.
    ///
    /// Bounded like every other store in this module: a grant to more than
    /// [`MAX_PRINCIPALS_PER_STATEMENT`] principals, or one that would leave the policy
    /// holding more than [`MAX_POLICY_STATEMENTS`] statements, is refused with
    /// [`PolicyWrite::LimitExceeded`] and changes nothing, so no sequence of writes can
    /// grow one policy without limit.
    pub fn put_permission(
        &mut self,
        group_name: &str,
        action_group: &str,
        principals: &[String],
        revision_id: Option<&str>,
    ) -> PolicyWrite {
        let Some(group) = self.groups.get(group_name) else {
            return PolicyWrite::UnknownGroup;
        };
        if principals.len() > MAX_PRINCIPALS_PER_STATEMENT {
            return PolicyWrite::LimitExceeded(PolicyLimit::Principals {
                requested: principals.len(),
                limit: MAX_PRINCIPALS_PER_STATEMENT,
            });
        }
        // The `revisionId` rule of this surface, from the member's own doc: "Do not
        // specify this when you add permissions to a profiling group for the first
        // time. If a policy already exists on the profiling group, you must specify the
        // revisionId." A group holds a policy exactly while that policy holds a
        // statement (`ProfilingGroup::resource_policy`), so a write against a group that
        // has none is a FIRST write — including the first grant after a removal took the
        // previous policy's last statement.
        match (group.resource_policy(), revision_id) {
            (None, Some(named)) => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Unexpected {
                    requested: named.to_string(),
                });
            }
            (Some(policy), None) => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Missing {
                    current: policy.revision_id.clone(),
                });
            }
            (Some(policy), Some(named)) if policy.revision_id != named => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Stale {
                    requested: named.to_string(),
                    current: policy.revision_id.clone(),
                });
            }
            _ => {}
        }

        // How many statements the policy would hold once this one is written: rewriting
        // a statement the policy already holds adds none.
        let would_hold = group
            .resource_policy()
            .map(|policy| {
                policy.statements.len() + usize::from(!policy.statements.contains_key(action_group))
            })
            .unwrap_or(1);
        if would_hold > MAX_POLICY_STATEMENTS {
            return PolicyWrite::LimitExceeded(PolicyLimit::Statements {
                requested: would_hold,
                limit: MAX_POLICY_STATEMENTS,
            });
        }

        // A grant is a SET of principals: the request's order is kept, and a principal
        // it names twice is granted once. The `seen` set is what makes that one pass
        // over the list rather than one scan of the kept principals per principal.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut granted: Vec<String> = Vec::with_capacity(principals.len());
        for principal in principals {
            if seen.insert(principal.as_str()) {
                granted.push(principal.clone());
            }
        }

        // The write is applied from here on, so this is where the revision it carries
        // is minted.
        let revision_id = self.mint_revision_id(group_name, action_group);
        let Some(group) = self.groups.get_mut(group_name) else {
            // Unreachable: the group was resolved above, under the same `&mut self`.
            return PolicyWrite::UnknownGroup;
        };
        let arn = group.arn.clone();
        let mut policy = group
            .policy
            .take()
            .unwrap_or_else(|| ResourcePolicy::new(&revision_id));
        policy.grant(action_group, granted);
        policy.set_revision(&revision_id);
        let document = policy.document(&arn);
        group.policy = Some(policy);
        PolicyWrite::Written {
            policy: document,
            revision_id,
        }
    }

    /// Remove `action_group`'s statement from `group_name`'s policy, under the same
    /// optimistic locking as [`Self::put_permission`]: the revision is checked first,
    /// so a stale revision is answered as a conflict whatever else the request names.
    ///
    /// A removal that leaves a statement behind is [`PolicyWrite::Written`]: the policy
    /// as it then stands and the revision the change minted.
    ///
    /// A resource-based policy exists only while it holds at least one statement, so a
    /// removal that takes the LAST statement DELETES the policy and is
    /// [`PolicyWrite::Deleted`]: the group is policy-less again, exactly as it was
    /// before the first grant. There is then no policy to render a document of and none
    /// to carry a revision, so none is minted either — the next grant on the group is a
    /// FIRST write again, and every other policy-scoped call finds no policy.
    pub fn remove_permission(
        &mut self,
        group_name: &str,
        action_group: &str,
        revision_id: &str,
    ) -> PolicyWrite {
        // The checks first, under a shared borrow, so the revision is minted only for a
        // removal that is actually applied.
        let Some(group) = self.groups.get(group_name) else {
            return PolicyWrite::UnknownGroup;
        };
        // No write has created a policy on this group, so there is nothing to remove
        // from.
        let Some(policy) = group.resource_policy() else {
            return PolicyWrite::NoPolicy;
        };
        if policy.revision_id != revision_id {
            return PolicyWrite::RevisionMismatch(RevisionMismatch::Stale {
                requested: revision_id.to_string(),
                current: policy.revision_id.clone(),
            });
        }
        if !policy.statements.contains_key(action_group) {
            return PolicyWrite::NoStatement;
        }

        // A statement-less policy is no policy: the removal that takes the last
        // statement deletes the group's policy, so the group goes back to the
        // policy-less state it was created in. There is then no policy to be at a
        // revision, so a removal that deletes one mints none.
        let revision_id = (!policy.is_last_statement(action_group))
            .then(|| self.mint_revision_id(group_name, action_group));
        let Some(group) = self.groups.get_mut(group_name) else {
            // Unreachable: the group was resolved above, under the same `&mut self`.
            return PolicyWrite::UnknownGroup;
        };
        let Some(mut policy) = group.policy.take() else {
            // Unreachable for the same reason: the policy was read above.
            return PolicyWrite::NoPolicy;
        };
        policy.revoke(action_group);
        // Only a policy that still holds a statement is stored again, and only such a
        // policy has a document and a revision to report.
        let Some(revision_id) = revision_id else {
            return PolicyWrite::Deleted;
        };
        policy.set_revision(&revision_id);
        let document = policy.document(&group.arn);
        group.policy = Some(policy);
        PolicyWrite::Written {
            policy: document,
            revision_id,
        }
    }

    /// Profiling-group names in a stable (lexicographic) order, so paging over
    /// `ListProfilingGroups` visits every group exactly once.
    pub fn sorted_group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.groups.keys().cloned().collect();
        names.sort();
        names
    }

    /// Every findings report recorded anywhere in this store, as the
    /// `FindingsReportSummary` the account-wide summary
    /// (`GetFindingsReportAccountSummary`) reports — restricted to reports over daily
    /// (`P1D`) profiles when `daily_only`.
    ///
    /// Stable order: by profiling-group name (as [`Self::sorted_group_names`] orders
    /// it), then within a group by analysed window and report id (as
    /// [`ProfilingGroup::findings_reports`] orders it), so paging over the summary
    /// visits every report exactly once. A group with no recorded analysis contributes
    /// nothing: the emulator runs no recommendation analysis, so the reports summarised
    /// here are only those [`Self::record_findings_report`] stored.
    pub fn account_findings_report_summaries(&self, daily_only: bool) -> Vec<FindingsReportSummary> {
        self.sorted_group_names()
            .iter()
            .filter_map(|name| self.groups.get(name))
            .flat_map(|group| {
                group
                    .findings_reports(daily_only)
                    .into_iter()
                    // The group name reported is the STORED group's.
                    .map(|report| report.summary(&group.name))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The stored group an ARN identifies, or `None` when this store holds no resource
    /// with that ARN. An ARN identifies ONE resource, so the stored ARN must match the
    /// one asked about in full: a group of the same name in another region, partition or
    /// account is a different resource and is not found here.
    ///
    /// Pair it with [`profiling_group_name_from_arn`], which separates "not a
    /// profiling-group ARN at all" (a bad request) from "no such profiling group"
    /// (a 404).
    pub fn group_by_arn(&self, arn: &str) -> Option<&ProfilingGroup> {
        let group = self.groups.get(profiling_group_name_from_arn(arn)?)?;
        (group.arn == arn).then_some(group)
    }

    /// The same resolution as [`group_by_arn`](Self::group_by_arn), for an operation
    /// that WRITES the resource it resolved. The rule lives in `group_by_arn` alone and
    /// is applied here through it, so the two cannot drift; only the borrow differs,
    /// which is why a reporting operation takes the shared one and physically cannot
    /// write.
    pub fn group_by_arn_mut(&mut self, arn: &str) -> Option<&mut ProfilingGroup> {
        let name = self.group_by_arn(arn)?.name.clone();
        self.groups.get_mut(&name)
    }

    /// Store a posted agent profile, honouring `profile_token` as an idempotency token
    /// and every retention bound: the per-group count and byte caps, and the
    /// store-wide [`MAX_RETAINED_PROFILE_BYTES`] budget, which evicts the oldest
    /// profile anywhere in the store — never the one just posted — until the total
    /// fits. The only way a handler stores a profile.
    pub fn post_profile(
        &mut self,
        group_name: &str,
        profile_token: Option<&str>,
        profile: AgentProfile,
    ) -> PostedProfile {
        let Some(group) = self.groups.get_mut(group_name) else {
            return PostedProfile::UnknownGroup;
        };
        if let Some(token) = profile_token {
            if !group.remember_profile_token(token) {
                return PostedProfile::Replayed;
            }
        }
        group.record_profile(profile);

        while self.retained_profile_bytes() > MAX_RETAINED_PROFILE_BYTES {
            let Some((name, index)) = self.oldest_evictable_profile(group_name) else {
                break;
            };
            if let Some(group) = self.groups.get_mut(&name) {
                group.profiles.remove(index);
            }
        }
        PostedProfile::Stored
    }

    /// Record one recommendation report against `group_name`, within
    /// [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`] (the oldest recorded report is
    /// dropped first, so the report store cannot grow without limit). `false` when
    /// there is no such group.
    ///
    /// The ONLY way a findings report enters this store. The emulator performs no
    /// recommendation analysis of profiles — nothing in it invents findings — so a
    /// group holds reports only where analysis was recorded for it, which is why
    /// `ListFindingsReports` answers a group that has had none with an empty list.
    pub fn record_findings_report(&mut self, group_name: &str, report: FindingsReport) -> bool {
        let Some(group) = self.groups.get_mut(group_name) else {
            return false;
        };
        while group.findings_reports.len() >= MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP {
            group.findings_reports.pop_front();
        }
        group.findings_reports.push_back(report);
        true
    }

    /// Record one detected anomaly against `group_name`, within
    /// [`MAX_RETAINED_ANOMALIES_PER_GROUP`] (the oldest recorded anomaly is dropped
    /// first, so the catalogue cannot grow without limit). `false` when there is no such
    /// group.
    ///
    /// The ONLY way an anomaly enters this store. The emulator runs no anomaly detector
    /// over the profiles it holds — nothing in it invents a finding — so a group holds
    /// anomalies only where an analysis was recorded for it, which is why
    /// `GetRecommendations` answers a group that has had none with an empty `anomalies`
    /// list and `SubmitFeedback` refuses every `anomalyInstanceId` submitted against it.
    pub fn record_detected_anomaly(&mut self, group_name: &str, anomaly: DetectedAnomaly) -> bool {
        let Some(group) = self.groups.get_mut(group_name) else {
            return false;
        };
        while group.detected_anomalies.len() >= MAX_RETAINED_ANOMALIES_PER_GROUP {
            group.detected_anomalies.pop_front();
        }
        group.detected_anomalies.push_back(anomaly);
        true
    }

    /// Profile bytes retained across every group — what
    /// [`MAX_RETAINED_PROFILE_BYTES`] bounds.
    pub fn retained_profile_bytes(&self) -> usize {
        self.groups
            .values()
            .flat_map(|group| group.profiles.iter())
            .map(|profile| profile.bytes.len())
            .sum()
    }

    /// The oldest retained profile anywhere in the store, as (group name, index),
    /// skipping the newest profile of `keep_newest_of` so a post is never evicted by
    /// its own budget pass.
    ///
    /// Receipt times are ordered through [`instant_key`], exact at nanosecond
    /// resolution, so the oldest of two profiles received a fraction of a microsecond
    /// apart is the one that really is older.
    fn oldest_evictable_profile(&self, keep_newest_of: &str) -> Option<(String, usize)> {
        let mut oldest: Option<(String, usize, (i64, u32))> = None;
        for (name, group) in &self.groups {
            let protected = if name == keep_newest_of {
                group.profiles.len().saturating_sub(1)
            } else {
                usize::MAX
            };
            for (index, profile) in group.profiles.iter().enumerate() {
                if index == protected {
                    continue;
                }
                let at = instant_key(profile.received_at);
                if oldest
                    .as_ref()
                    .is_none_or(|(_, _, candidate)| at < *candidate)
                {
                    oldest = Some((name.clone(), index, at));
                }
            }
        }
        oldest.map(|(name, index, _)| (name, index))
    }
}

/// The message a bound-refused policy write carries, for the `ValidationException` the
/// write answers with: which bound the write would have passed, and by how much. The
/// bounds themselves are the store's ([`MAX_PRINCIPALS_PER_STATEMENT`] is the modelled
/// `Principals` maximum, [`MAX_POLICY_STATEMENTS`] the number of modelled action
/// groups), so the text states the member and the constraint the request missed.
pub fn policy_limit_message(group_name: &str, limit: &PolicyLimit) -> String {
    match limit {
        PolicyLimit::Principals { requested, limit } => format!(
            "principals must name at most {limit} roles or users, got {requested}"
        ),
        PolicyLimit::Statements { requested, limit } => format!(
            "the resource-based policy of profiling group {group_name} holds at most \
             {limit} action group statements, and this write would leave it holding \
             {requested}"
        ),
    }
}

/// Why a revision-checked policy write was refused, for the `ConflictException` the
/// write answers with. The status (409) and the code are the contract; this text is
/// built from the request and the stored resource, so it STATES both revisions the
/// conflict is about: the revision the request named (where it named one) and the
/// revision the stored policy carries (where there is one) — which is the revision the
/// caller retries against, so a conflict resolves without a second `GetPolicy`.
///
/// Only the `Stale` sentence is a RECORDED one. The other two arms are this emulator's
/// own wording around the same two facts: a recording that shows how the service words
/// them is the authority over this text, not a regression against it.
pub fn revision_conflict_message(group_name: &str, mismatch: &RevisionMismatch) -> String {
    match mismatch {
        RevisionMismatch::Missing { current } => format!(
            "profiling group {group_name} already has a resource-based policy; name its \
             current revision to write to it, RevisionId: {current}"
        ),
        RevisionMismatch::Unexpected { requested } => format!(
            "profiling group {group_name} has no resource-based policy, so the revision: \
             {requested} names none; do not name a revisionId when creating one"
        ),
        RevisionMismatch::Stale { requested, current } => format!(
            "Policy has been updated since the revision: {requested}, new RevisionId: \
             {current}"
        ),
    }
}

/// The `ResourceNotFoundException` message a policy-scoped operation answers a group
/// that EXISTS but has no resource-based policy with: the group as the request named it,
/// and the account the lookup was scoped to — a profiling-group name identifies a group
/// only within one account, so the account is part of saying whose view of the group had
/// no policy.
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries. This
/// is distinct from [`group_not_found_message`], which answers a group that is not there
/// at all.
pub fn policy_not_found_message(group_name: &str) -> String {
    format!(
        "No policy attached to profiling group name: {group_name} in accountId: {}; ",
        ::engine::emulator_account_id()
    )
}

/// The message a 404 about missing profile DATA carries — a FIXED service text, with no
/// substitution from the request at all. Both "no such group" and "the group holds no
/// profile in this range" are `ResourceNotFoundException` (404), so the message is the
/// only thing a caller can tell them apart by: the group-missing refusals name the group
/// ([`group_not_found_message`], [`profiling_group_record_not_found_message`]) and the
/// no-data refusal states this constant instead.
pub const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// How CodeGuru Profiler words a refusal about the profiling-group RECORD ITSELF — the
/// collision a create hits and the absence a delete hits: the group is named together
/// with the account it would live in, because a profiling-group name identifies a group
/// only WITHIN one account, so the account is part of saying which group collided or was
/// missing. `outcome` completes the sentence ("already exists.", "not found.").
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries.
fn profiling_group_record_message(name: &str, outcome: &str) -> String {
    format!(
        "ProfilingGroup (name: {name}, accountId: {}) {outcome}",
        ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a create whose `profilingGroupName` is already
/// taken: the record-level form of [`profiling_group_record_message`].
pub fn profiling_group_already_exists_message(name: &str) -> String {
    profiling_group_record_message(name, "already exists.")
}

/// The `ResourceNotFoundException` message for a delete of a group that is not there:
/// the record-level form of [`profiling_group_record_message`].
pub fn profiling_group_record_not_found_message(name: &str) -> String {
    profiling_group_record_message(name, "not found.")
}

/// The `ResourceNotFoundException` message every OTHER group-scoped operation answers a
/// name it cannot resolve with: the service states that it cannot carry out the operation
/// asked of it, naming the group that was not there. The apostrophe is the typographic
/// one (U+2019) the service's own text uses, not an ASCII `'`.
///
/// This is the refusal of an operation ON a group, as distinct from a refusal about the
/// group RECORD (create's conflict, delete's absence), which names the account too —
/// see [`profiling_group_record_not_found_message`].
///
/// It is the sentence of the operations whose own wording for that case is NOT recorded,
/// so it is not what EVERY group-scoped refusal states: the operations with a recorded
/// form of their own state that instead, each documented on its own helper —
/// [`policy_group_not_found_message`] (`GetPolicy`),
/// [`notification_group_not_found_message`] (`GetNotificationConfiguration`),
/// [`update_group_not_found_message`] (`UpdateProfilingGroup`) and
/// [`agent_group_not_found_message`] (the agent plane). A handler that calls this
/// function therefore states no rule of its own: this doc is the rule, and which sentence
/// an operation answers with is decided here.
pub fn group_not_found_message(name: &str) -> String {
    format!("CodeGuru Profiler can\u{2019}t complete the operation because {name} was not found.")
}

/// How the POLICY plane says that the profiling group a request names does not exist:
/// `GetPolicy` states the LOOKUP it performed — the account the lookup was scoped to and
/// the name it looked for — rather than the sentence every other group-scoped operation
/// states ([`group_not_found_message`]).
///
/// Why this operation has a form of its own: both "no such group" and "the group has no
/// resource-based policy" are 404 `ResourceNotFoundException` here, so the message is the
/// only thing a caller can tell them apart by — this one says the GROUP lookup found
/// nothing, [`policy_not_found_message`] says the group was found and holds no policy.
///
/// `PutPermission` and `RemovePermission` keep [`group_not_found_message`] for a group
/// that is not there: their own wording for that case is not recorded anywhere, and a
/// per-operation sentence this service has never been observed to emit would be invented
/// rather than emulated.
pub fn policy_group_not_found_message(name: &str) -> String {
    format!(
        "No profiling group with accountId: {}; name: {name} was found.",
        ::engine::emulator_account_id()
    )
}

/// How the NOTIFICATION surface says that the profiling group a request names does not
/// exist: `GetNotificationConfiguration` states the LOOKUP it performed — the account the
/// lookup was scoped to and the name it looked for — rather than the sentence every other
/// group-scoped operation states ([`group_not_found_message`]).
///
/// `AddNotificationChannels` and `RemoveNotificationChannel` keep
/// [`group_not_found_message`] for a group that is not there, for the reason
/// [`policy_group_not_found_message`] is `GetPolicy`'s alone: their own wording for that
/// case is not recorded anywhere, and a per-operation sentence this service has never
/// been observed to emit would be invented rather than emulated.
pub fn notification_group_not_found_message(name: &str) -> String {
    format!(
        "Profiling group not found with accountId: {}, name: {name}.",
        ::engine::emulator_account_id()
    )
}

/// How the NOTIFICATION surface says that the profiling group a request names holds no
/// channel with the `channelId` the request addressed: it states the LOOKUP it performed —
/// the channel id asked about, the group it was looked for in, and the account that group
/// belongs to.
///
/// This refusal is the operation's `ValidationException` (400), NOT a 404: a notification
/// request addresses the profiling GROUP, and the channel id is a parameter of it, so an
/// id the group's stored configuration does not hold is an invalid parameter value, while
/// `ResourceNotFoundException` stays reserved for the group itself not existing
/// ([`notification_group_not_found_message`], [`group_not_found_message`]).
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries.
pub fn notification_channel_not_found_message(channel_id: &str, group_name: &str) -> String {
    format!(
        "Channel not found with channelId: {channel_id}, profilingGroupName: {group_name} \
         and AccountId: {}",
        ::engine::emulator_account_id()
    )
}

/// How the NOTIFICATION surface refuses a channel `uri` that is not an Amazon SNS topic
/// ARN: it states that channel validation failed and names the topic ARN it could not
/// read, exactly as the request sent it.
///
/// `Channel$uri`'s doc fixes what a channel uri must be — "Unique arn of the resource to
/// be used for notifications. We support a valid SNS topic arn as a channel uri" — and
/// anything else is the operation's `ValidationException` (400), refused by
/// [`is_sns_topic_arn`]. The rejected uri is the only variable part: the sentence names
/// neither the member path it arrived in nor a grammar, so a client reads the value it
/// sent back out of it and nothing else.
pub fn invalid_channel_uri_message(uri: &str) -> String {
    format!("Channel Validation failed: Invalid topic ARN: {uri}")
}

/// How the ANALYSIS surface says that the `anomalyInstanceId` a `SubmitFeedback` request
/// names is not an anomaly instance of the caller's account: it states the lookup it
/// performed — the id asked about and the account the anomalies were looked for in.
///
/// Feedback is submitted about an anomaly instance of an analysis, so an id no anomaly of
/// the named group has is an invalid parameter value — the operation's
/// `ValidationException` (400), "The parameter is not valid." —  while
/// `ResourceNotFoundException` (404) stays reserved for the profiling group itself not
/// existing ([`group_not_found_message`]).
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries.
pub fn anomaly_instance_not_found_message(anomaly_instance_id: &str) -> String {
    format!(
        "Anomaly with id: {anomaly_instance_id} not found for accountId: {}",
        ::engine::emulator_account_id()
    )
}

/// How `UpdateProfilingGroup` says that the profiling group a request names does not
/// exist: it states the lookup it performed — the account it was scoped to and the
/// `profilingGroupName` member it looked the group up by — and the operation that could
/// not be carried out, rather than the sentence every other group-scoped operation
/// states ([`group_not_found_message`]).
///
/// Why this operation has a form of its own: a client that probes a name before writing
/// to it reads the message to tell WHICH lookup failed, and an update's refusal has to
/// be distinguishable from the same name's read refusals (`DescribeProfilingGroup`'s
/// sentence, `GetPolicy`'s in [`policy_group_not_found_message`]).
pub fn update_group_not_found_message(name: &str) -> String {
    format!(
        "Unable to find profiling group with accountId: {}; profilingGroupName: {name} \
         during update",
        ::engine::emulator_account_id()
    )
}

/// What the ARN-addressed TAGGING surface (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) says about a `resourceArn` that resolves to no live resource:
/// a FIXED sentence about the resource, naming nothing from the request.
///
/// That surface is generic over resource types — it addresses whatever a resource ARN
/// names, not a profiling group by name — so it reports the absence in its own terms,
/// with its own error code ([`crate::scaffolded::Scaffolded::not_found`]'s
/// `NotFoundException`) rather than the `ResourceNotFoundException` the
/// profiling-group operations answer with.
pub const TAGGING_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// Why a tag map that carries no entry is refused: it names the member the map arrived
/// in and the constraint it broke (non-empty), so the caller can tell a well-formed
/// request carrying an empty map from a body the parser could not read.
///
/// `TagsMap` has no modelled length bound, so the boundary validator passes an empty
/// map and this is the operation's own check.
pub fn empty_tag_map_message(member: &str) -> String {
    format!("Invalid request input, expected non-empty {member} map")
}

/// The resource-type prefix a profiling-group ARN carries in its resource field, as
/// [`profiling_group_arn`] mints it: `profilingGroup/<name>`.
pub const PROFILING_GROUP_RESOURCE_PREFIX: &str = "profilingGroup/";

/// The profiling-group name a `ProfilingGroupArn` names, or `None` when the string is
/// not a profiling-group ARN of this service.
///
/// `ProfilingGroupArn` carries an `aws.api#arnReference` to
/// `AWS::CodeGuruProfiler::ProfilingGroup` and no length or pattern constraint, so the
/// boundary validator accepts any string and the SHAPE of the ARN is the handler's to
/// check. What an ARN IS — the six fields, the literal `arn`, the non-empty partition,
/// vendor and resource — is read by the tree's one parse,
/// [`service_common::arn::parse_of`], narrowed to this service's
/// [`ARN_NAMESPACE`]; only the part that is specific to this service is decided here:
/// its resources are the profiling groups, so a resource field that is not a
/// `profilingGroup/<name>` names nothing a tagging operation can act on.
pub fn profiling_group_name_from_arn(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    let name = parsed
        .resource
        .strip_prefix(PROFILING_GROUP_RESOURCE_PREFIX)?;
    // A profiling-group name is one path segment (the modelled `ProfilingGroupName`
    // pattern is `^[\w-]+$`), so an empty name or a further `/` is not one of this
    // service's resources. `parse_of` keeps the resource field's own colons, so this
    // check is on the name as the ARN carries it.
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name)
}

/// One instant as an EXACTLY comparable key: whole seconds and sub-second nanoseconds.
///
/// `DateTime` carries nanosecond resolution, which an `f64` of current epoch seconds
/// cannot represent (its granularity there is coarser than a microsecond), so two
/// instants a fraction of a microsecond apart would compare equal. Ordering and
/// inclusive-bound tests on stored timestamps go through this instead.
pub fn instant_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// One instant in the ISO 8601 `date-time` form this service's own docs state its time
/// members in ("This is specified using the ISO 8601 format. For example,
/// 2020-06-01T13:15:02.001Z", `smithy_query(query="doc",
/// shape="com.amazonaws.codeguruprofiler#GetRecommendationsRequest", member="startTime")`),
/// so a refusal naming a bound states it the way the request carried it.
///
/// The fractional-second field of that form is the MILLISECOND field the documented
/// example spells out: three digits, zero-padded, present on every instant — so a whole
/// second is named `...:02.000Z`, half a second `...:02.500Z` and 660 ms `...:02.660Z`.
/// The library's RFC-3339 writer emits the SHORTEST fraction that names the instant
/// instead (nothing at all for a whole second, `.5` for half of one, `.66` for 660 ms), so
/// the field is written here: the whole second goes through the library, which owns the
/// calendar, and the fixed-width field is appended to it. `subsec_nanos` counts forward
/// from `secs` on either side of the epoch, so that split holds before 1970 too.
///
/// An instant carrying a finer fraction than the field holds is named at the field's own
/// resolution — the millisecond it falls in — because the form carries no digit for a
/// fraction of a millisecond. The interval rule itself still COMPARES bounds at the full
/// nanosecond resolution they were sent in ([`instant_key`]); only the rendering is at the
/// documented width.
///
/// Falls back to epoch seconds if the instant cannot be rendered, so a refusal is never
/// lost to a formatting failure.
pub fn iso8601(at: DateTime) -> String {
    const NANOS_PER_MILLISECOND: u32 = 1_000_000;
    match DateTime::from_secs(at.secs()).fmt(::aws_smithy_types::date_time::Format::DateTime) {
        // The whole-second instant carries no fraction of its own, so its trailing zone
        // designator is exactly where the millisecond field goes.
        Ok(whole_second) => format!(
            "{}.{:03}Z",
            whole_second.strip_suffix('Z').unwrap_or(&whole_second),
            at.subsec_nanos() / NANOS_PER_MILLISECOND
        ),
        Err(_) => format!("{} (epoch seconds)", at.as_secs_f64()),
    }
}

/// One instant from the epoch MILLISECONDS `millis`, or `None` when that number names no
/// instant this service can hold.
///
/// The `start` an agent profile document states for the period it covers is a number of
/// milliseconds since the epoch — which is how a submitted profile states its own period,
/// as opposed to the ISO 8601 `date-time` form the modelled timestamp MEMBERS of this
/// service carry ([`iso8601`]) — so the one reading of such a number lives here.
///
/// The number is read as the document states it, so a fraction of a millisecond is kept
/// as part of the instant: it is the epoch shifted by that many nanoseconds, through the
/// one piece of instant arithmetic this module has ([`shifted`]), which floors towards the
/// past on both sides of the epoch — so a negative number names the instant before it. A
/// number no instant can be shifted by names none.
pub fn instant_from_epoch_millis(millis: f64) -> Option<DateTime> {
    const NANOS_PER_MILLISECOND: f64 = 1_000_000.0;
    let nanos = (millis * NANOS_PER_MILLISECOND).round();
    if !nanos.is_finite() || nanos < i128::MIN as f64 || nanos > i128::MAX as f64 {
        return None;
    }
    Some(shifted(DateTime::from_secs(0), nanos as i128))
}

/// This service's INTERVAL rule, in one place for every operation that takes a time
/// range: a pair of bounds names an interval only when its start is STRICTLY before its
/// end, so a reversed pair and a zero-width pair are both refused — before any lookup,
/// so a request that names no interval never reports about one.
///
/// `Ok(())` when `start` is before `end`; otherwise the message, which states the rule
/// and names the two offending bounds as the request gave them (the service's own
/// sentence for this refusal), for each handler to lift into its own modelled
/// `ValidationException` (HTTP 400).
///
/// The bounds are compared through [`instant_key`], exactly at nanosecond resolution,
/// so an interval narrower than a microsecond is still ordered correctly rather than
/// collapsing to a zero-width one through `f64` epoch seconds.
pub fn validate_interval(start: DateTime, end: DateTime) -> Result<(), String> {
    if instant_key(start) < instant_key(end) {
        return Ok(());
    }
    Err(format!(
        "Interval start time ({}) must be before end time ({})",
        iso8601(start),
        iso8601(end)
    ))
}

/// What a read says when the time range a request names is LONGER than the maximum the
/// operation's own doc allows: what that doc calls the range (`subject`), the limit in the
/// ISO 8601 duration form the doc names it by (`limit`), and the range the request ended
/// up naming (`named`) — after any open side was completed — so the caller can fix the
/// request from the message alone.
pub fn window_too_long_message(subject: &str, limit: &str, named: &str) -> String {
    format!("{subject} must be at most {limit} long, got {named}")
}

/// This service's WINDOW-LENGTH rule for a window whose BOTH bounds are known: the range
/// `[start, end]` is refused when it is longer than `maximum`, naming both bounds the way
/// the request would have spelled them ([`iso8601`]).
///
/// For an operation that completes an open side itself before bounding the result;
/// [`validate_window_length`] applies the same rule to the trio a request names a window
/// with. Bounds are compared through [`instant_key`], exactly at nanosecond resolution, so
/// a range a nanosecond over the maximum is refused rather than collapsing onto it through
/// `f64` epoch seconds.
pub fn validate_bounds_length(
    start: DateTime,
    end: DateTime,
    maximum: Duration,
    limit: &str,
    subject: &str,
) -> Result<(), String> {
    let longest = shifted(start, span_nanos(maximum));
    if instant_key(end) > instant_key(longest) {
        return Err(window_too_long_message(
            subject,
            limit,
            &format!("{} to {}", iso8601(start), iso8601(end)),
        ));
    }
    Ok(())
}

/// This service's WINDOW-LENGTH rule, in one place for every operation whose doc bounds
/// how LONG a time range it answers over may be — the way [`validate_interval`] holds the
/// interval rule: a range longer than `maximum` is refused, however the request named it.
///
/// A window is named with 1 or 2 of `startTime`, `endTime` and `period`
/// ([`requested_window`]), so the length a request states is either the distance between
/// the two bounds it ended up with — the pair it gave, or the pair a `period` completed
/// from a single bound — or, where it named no pair at all, the `period` on its own, which
/// says how long the range is without saying where it sits. Both are bounded here; a
/// request that named no length at all leaves nothing to bound.
///
/// `Err` carries the sentence this refusal states ([`window_too_long_message`]), for each
/// handler to lift into its own modelled client-input error (HTTP 400).
pub fn validate_window_length(
    window: &RequestedWindow,
    period: Option<&str>,
    maximum: Duration,
    limit: &str,
    subject: &str,
) -> Result<(), String> {
    match (window.start, window.end) {
        (Some(start), Some(end)) => validate_bounds_length(start, end, maximum, limit, subject),
        // No pair of bounds, so the only length the request stated is the `period` itself,
        // reported back as the request spelled it.
        _ => match (window.span, period) {
            (Some(span), Some(period)) if span > maximum => {
                Err(window_too_long_message(subject, limit, period))
            }
            _ => Ok(()),
        },
    }
}

/// What every listing of this service says when the `nextToken` it was handed is not a
/// cursor it could have issued: it names no position in the listing, so the request is
/// refused rather than answered with a page. Each handler lifts this sentence into its
/// OWN modelled client-input error (HTTP 400), so the code a client matches on is still
/// the one that operation declares.
const UNISSUED_TOKEN_MESSAGE: &str = "nextToken is not a token this service issued";

/// One page of an already-ordered listing, as [`ListCursor::page_of`] computes it.
#[derive(Debug)]
pub struct ListPage<'a, T> {
    /// The elements this page reports, in the listing's own order.
    pub items: &'a [T],
    /// The cursor the caller continues from, present only while elements remain past
    /// this page — so a listing that reached its end carries no token, the way the
    /// modelled `nextToken` members document ("This value is null when there are no
    /// more results to return").
    pub next_token: Option<String>,
}

/// This service's PAGINATION rule, in one place for every operation that reports an
/// already-ordered sequence a page at a time — the way [`validate_interval`] holds its
/// interval rule and [`validate_tags`] its tag rules.
///
/// The cursor is this service's own opaque token: the INDEX into the listing at which
/// the next page starts, which satisfies the modelled `PaginationToken` constraints
/// (len 1..64, `^[\w-]+$`) for a listing of any length. A token is issued only while
/// elements remain PAST the page it closes, so every index this service hands out names
/// an element of the listing: a token that does not read as an index, and one at or past
/// the end of the listing, are both tokens this service never issued and are refused
/// with [`UNISSUED_TOKEN_MESSAGE`]. A request that names NO token starts at the
/// beginning, which is the whole listing however short it is — including empty.
///
/// Parsing the token is separate from cutting the page so each handler can refuse a
/// malformed cursor BEFORE it resolves the resource the listing is about, the way it
/// refuses the rest of its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// The position in the listing this page starts at, as the `nextToken` named it;
    /// `None` when the request named no token, which starts at the beginning.
    start: Option<usize>,
}

impl ListCursor {
    /// The cursor `next_token` names, or the message for a token that is not one of
    /// this service's — which repeats the offending token, since the caller sent it.
    pub fn parse(next_token: Option<&str>) -> Result<Self, String> {
        let start = next_token
            .map(|token| {
                token
                    .parse::<usize>()
                    .map_err(|_| format!("{UNISSUED_TOKEN_MESSAGE}: {token}"))
            })
            .transpose()?;
        Ok(Self { start })
    }

    /// The page of `items` this cursor starts, at most `max_results` long, plus the
    /// token to continue from; or the message for a cursor this service never issued —
    /// one at or past the end of `items`.
    ///
    /// Without `max_results` the page is [`DEFAULT_LIST_PAGE_SIZE`] long, so one
    /// unparameterised request cannot materialise a response that grows with the number
    /// of stored elements. The boundary validator has already held `max_results` to the
    /// modelled range by the time a handler calls this; a page of zero elements would
    /// hand back a token forever, so a size below that range is floored at one rather
    /// than silently paging nothing.
    pub fn page_of<'a, T>(
        self,
        items: &'a [T],
        max_results: Option<i32>,
    ) -> Result<ListPage<'a, T>, String> {
        let start = match self.start {
            // A token is only ever issued with elements still after it, so a cursor at
            // or past the end of the listing is one this service never issued.
            Some(start) if start >= items.len() => {
                return Err(UNISSUED_TOKEN_MESSAGE.to_string());
            }
            Some(start) => start,
            None => 0usize,
        };
        let limit = max_results.map_or(DEFAULT_LIST_PAGE_SIZE, |max| {
            usize::try_from(max).unwrap_or(1).max(1)
        });
        let end = start.saturating_add(limit).min(items.len());
        Ok(ListPage {
            items: items.get(start..end).unwrap_or_default(),
            next_token: (end < items.len()).then(|| end.to_string()),
        })
    }
}

/// The ARN of a profiling group:
/// `arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/<name>`.
pub fn profiling_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{PROFILING_GROUP_RESOURCE_PREFIX}{name}"),
    )
}

/// How the AGENT plane says that the profiling group a request names does not exist: its
/// message is account-qualified, naming the account the group would belong to as well as
/// the group — a different sentence from the one every other operation states
/// ([`group_not_found_message`]).
///
/// This is `ConfigureAgent`'s sentence. The agent plane is `ConfigureAgent` and
/// `PostAgentProfile` — the two operations the model's own `ActionGroup` enum names as the
/// agent APIs ("Permission group type for Agent APIs - ConfigureAgent, PostAgentProfile",
/// `smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ActionGroup")`) —
/// but a refusal's wording belongs to the operation that answers it, and
/// `PostAgentProfile`'s own is recorded, so it states that instead
/// ([`posted_profile_group_not_found_message`]). The
/// account is the emulator's canonical one, the same account every ARN this service mints
/// carries.
pub fn agent_group_not_found_message(name: &str) -> String {
    format!(
        "Profiling group for {}/{name} does not exist!",
        ::engine::emulator_account_id()
    )
}

/// How `PostAgentProfile` says that the profiling group a submission names does not
/// exist: it states the lookup it performed — the account the group was looked for in and
/// the `profilingGroupName` it was looked up by — in a sentence of its own.
///
/// A refusal's wording belongs to the OPERATION that answers it, so an operation whose
/// own text is recorded states that text and not another operation's: this is
/// `PostAgentProfile`'s, while `ConfigureAgent` — the other agent-plane operation — keeps
/// its own ([`agent_group_not_found_message`]). The account is the emulator's canonical
/// one, the same account every ARN this service mints carries, and the group name is the
/// request's; nothing else of the request is interpolated.
pub fn posted_profile_group_not_found_message(name: &str) -> String {
    format!(
        "No profiling group found in account {} with name {name}.",
        ::engine::emulator_account_id()
    )
}

/// Why a caller-supplied `resourceArn` that names no profiling group of this service is
/// refused on the INPUT: it names the member and the form the member must take, so the
/// caller can fix the request from the message alone.
pub fn resource_arn_validation_message(resource_arn: &str) -> String {
    format!(
        "Value '{resource_arn}' at 'resourceArn' failed to satisfy constraint: Member \
         must be the ARN of a CodeGuru Profiler profiling group, \
         arn:<partition>:{ARN_NAMESPACE}:<region>:<account>:{PROFILING_GROUP_RESOURCE_PREFIX}<name>"
    )
}

/// Why a `principals` element that is not the ARN of an IAM principal is refused on the
/// INPUT, worded like every other refusal this service states about a submitted value: the
/// value as submitted, the member's dotted path, and the requirement it failed, so the
/// caller can fix the request from the message alone.
///
/// `position` is the element's 0-based index in the submitted list; the path states it the
/// way the validating walk states a list element's — `<list>.<1-based position>.member` —
/// so a refusal about an element of a list reads the same whoever found it.
pub fn principal_validation_message(position: usize, principal: &str) -> String {
    format!(
        "Value '{principal}' at 'principals.{}.member' failed to satisfy constraint: \
         Member must be the ARN of an IAM principal (an account root, user, role, \
         federated user or assumed-role session), without wildcards",
        position + 1
    )
}

/// Refuse a caller-supplied tag map no AWS-tagged resource can hold: more than
/// [`MAX_TAGS_PER_RESOURCE`] entries, a key outside 1..[`MAX_TAG_KEY_CHARS`]
/// characters, or a value longer than [`MAX_TAG_VALUE_CHARS`].
///
/// `Ok(())` when every entry is inside those limits; otherwise the message naming the
/// member (`member`, the request member the map arrived in), the entry and the
/// constraint it failed, which each handler lifts into its own modelled
/// `ValidationException`. Every path that WRITES a resource's tags checks this BEFORE
/// storing anything, so a refused request leaves the stored map exactly as it was and
/// no request can grow the store past the published limits.
pub fn validate_tags(member: &str, tags: &HashMap<String, String>) -> Result<(), String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "Value at '{member}' failed to satisfy constraint: Member must have length \
             less than or equal to {MAX_TAGS_PER_RESOURCE}"
        ));
    }
    // Lexicographic order, so the entry a message blames is the same one on every run
    // whatever order the request's map was read in.
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    for key in keys {
        let key_chars = key.chars().count();
        if !(1..=MAX_TAG_KEY_CHARS).contains(&key_chars) {
            return Err(format!(
                "Value '{key}' at '{member}' failed to satisfy constraint: Map keys must \
                 have length between 1 and {MAX_TAG_KEY_CHARS}"
            ));
        }
        let value = &tags[key];
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return Err(format!(
                "Value at '{member}.{key}' failed to satisfy constraint: Member must have \
                 length less than or equal to {MAX_TAG_VALUE_CHARS}"
            ));
        }
    }
    Ok(())
}

/// How many tags `stored` would hold once `applied` has been merged into it: a key
/// `stored` already carries is re-tagged rather than added, so only a NEW key grows the
/// map. What [`MAX_TAGS_PER_RESOURCE`] is checked against before a merge is written.
pub fn merged_tag_count(
    stored: &HashMap<String, String>,
    applied: &HashMap<String, String>,
) -> usize {
    stored.len() + applied.keys().filter(|key| !stored.contains_key(*key)).count()
}

/// Why a merge that would leave a resource past [`MAX_TAGS_PER_RESOURCE`] is refused:
/// the member it arrived in, how many tags it would leave, and the limit. Beside
/// [`merged_tag_count`] and worded like [`validate_tags`], so every tag refusal of this
/// service speaks with one voice.
pub fn too_many_tags_message(member: &str, resulting: usize) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: applying these tags would \
         leave {resulting} tags on the resource, past the maximum of \
         {MAX_TAGS_PER_RESOURCE} tags per resource"
    )
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh notification-channel id for a channel the request did not name one for.
///
/// The model's `ChannelId` carries the SAME pattern as `RevisionId`
/// (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`) and `Channel$id`'s
/// doc says "A random UUID for channelId is used when adding a channel to the
/// notification configuration if not specified in the request" — so the id is derived
/// exactly the way this crate derives its other UUID-shaped id, through
/// [`revision_id`]: SHA-256 over the store's own monotonically increasing mint count,
/// the instant of the write and the group the channel is added to, cut into the five
/// lowercase-hex groups [`REVISION_ID_GROUPS`] records. The mint count is store state a
/// caller never sees, so a channel id cannot be computed from the request that produced
/// it, and two channels added in the same instant still differ.
fn channel_id(mint: u64, at: DateTime, group_name: &str) -> String {
    revision_id(mint, at, group_name, CHANNEL_ID_SCOPE)
}

/// What [`channel_id`] mixes into its digest in place of an action group, so a channel
/// id and a policy revision minted from the same count and instant differ.
const CHANNEL_ID_SCOPE: &str = "notificationChannel";

/// The ARN namespace (`aws.api#service$arnNamespace`) of Amazon SNS, the vendor field
/// of a topic ARN.
pub const SNS_ARN_NAMESPACE: &str = "sns";

/// Whether `uri` is an Amazon SNS topic ARN, which is what a channel uri must be:
/// `ChannelUri` is an `@arnReference` and its model doc says "Currently we only support
/// SNS channels and thus required to be an ARN".
///
/// The ARN itself is read by `service_common::arn`, the one caller-supplied-ARN parse
/// in this tree (six fields, a literal `arn`, a non-empty partition, vendor and
/// resource; region and account may be empty, as a partition-global ARN writes them);
/// only the vendor this service accepts is decided here.
pub fn is_sns_topic_arn(uri: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(uri, SNS_ARN_NAMESPACE)
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeguruprofilerCodeguruprofilerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>)
}
