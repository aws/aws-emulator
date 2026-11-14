// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-secretsmanager-secretsmanager service.
//!
//! The shape of the resource this service stores: a secret is identified by its
//! name WITHIN A REGION — the namespace is per Region, so the same name in
//! another Region is another secret, and the replicas of a secret are secrets of
//! their own in the Regions it is replicated to (see [`replication`]) — carries
//! an ARN that ends in the name plus a hyphen and six random characters, and owns
//! an insertion-ordered list of versions. The staging labels (`AWSCURRENT`,
//! `AWSPREVIOUS`, and any label a caller names) live on the versions, and a label
//! is attached to at most one version of a secret at a time; a version that
//! carries no label is deprecated and is left out of `VersionIdsToStages` /
//! `SecretVersionsToStages`.
//!
//! A secret also carries the rotation configuration `RotateSecret` set for it —
//! the rotation function, the rules, the external-rotation metadata and the
//! instant the schedule next makes a rotation due — and, while a rotation it
//! started has not finished, the version id that rotation staked `AWSPENDING`
//! on before its rotation function created that version.
//!
//! Everything a handler needs to resolve, mutate or project a secret is a method
//! here, so the per-operation files hold only that operation's own rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::ops::types::{
    ExternalSecretRotationMetadataItem, Filter, ReplicationStatusType, RotationRulesType, Tag,
};

/// The envelope a version's value is stored under, and the KMS calls that seal
/// and open it: the one place this service encrypts and decrypts a secret value,
/// so every read and every write of one goes through the secret's own key.
pub mod envelope;

/// Whether IAM lets a request to this service perform the action it asks for: the
/// one gate every operation of this service consults before it reads or writes
/// anything.
pub mod authorization;

/// The IAM condition keys a request and the secret it addresses populate, which the
/// gate above decides a statement's `Condition` against.
pub mod condition_keys;

/// Serving the retrieval of a secret another AWS service makes on a ROLE's behalf, for the
/// workload that service runs: the same gate, asked about that role rather than about a
/// signed request's caller.
pub mod secret_retrieval;

/// Rotating a secret: whether this service may invoke the rotation function, and the four
/// steps it drives that function through once a rotation has started.
pub mod rotation;

/// Cross-region replication: how a replica of a secret becomes a secret of its own
/// in its Region, how its status settles, and how it is torn down or promoted.
pub mod replication;
pub use replication::*;

/// The layout this service reports an attached resource policy document in, which is
/// the shared rendering every emulated service that attaches an IAM policy reports.
pub mod policy_rendering;
pub use policy_rendering::*;

/// The secrets ANOTHER Amazon Web Services service creates and manages for one of its own
/// resources: the reserved name they carry, the owning service they report, and how a
/// request of that service is told apart from a caller's.
pub mod service_owned;

/// The staging label on the current value of a secret.
pub const STAGE_CURRENT: &str = "AWSCURRENT";

/// The staging label that moves to the version `AWSCURRENT` was taken from.
pub const STAGE_PREVIOUS: &str = "AWSPREVIOUS";

/// The staging label a rotation stages the new secret value under: "AWSPENDING,
/// which indicates the version of the secret that contains new secret
/// information that will become the next current version when rotation
/// finishes" (`DescribeSecretResponse$VersionIdsToStages`, this service's
/// Smithy model).
pub const STAGE_PENDING: &str = "AWSPENDING";

/// Whether `stage` is one of the staging labels the service itself moves —
/// [`STAGE_CURRENT`], [`STAGE_PREVIOUS`] and [`STAGE_PENDING`], the three the
/// model documents — rather than one a caller named.
pub fn is_reserved_stage(stage: &str) -> bool {
    matches!(stage, STAGE_CURRENT | STAGE_PREVIOUS | STAGE_PENDING)
}

/// Whether a version claiming the staging labels `claimed` cancels a pending
/// one.
///
/// `AWSPENDING` "indicates the version of the secret that contains new secret
/// information that will become the next current version when rotation
/// finishes" (`DescribeSecretResponse$VersionIdsToStages`, this service's Smithy
/// model), so the label names a candidate for the CURRENT value. Minting a new
/// current version decides that value without that candidate, which leaves
/// nothing for the label to name: the candidate is cancelled, and the version it
/// sat on carries no label at all. A write that re-stages `AWSPENDING` on the
/// version it mints names its own candidate instead, so it cancels nothing.
fn cancels_pending_version(claimed: &[String]) -> bool {
    claimed.iter().any(|stage| stage == STAGE_CURRENT)
        && !claimed.iter().any(|stage| stage == STAGE_PENDING)
}

/// Secrets Manager appends "a hyphen and six random characters after the secret
/// name at the end of the ARN" (`CreateSecretRequest$Name`, this service's
/// Smithy model).
pub const ARN_SUFFIX_LEN: usize = 6;

/// The recovery window `DeleteSecret` applies when the caller names none: "If
/// you don't use either, then by default Secrets Manager uses a 30 day recovery
/// window." (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy
/// model.)
pub const DEFAULT_RECOVERY_WINDOW_DAYS: i64 = 30;

/// "The number of days from 7 to 30 that Secrets Manager waits before
/// permanently deleting the secret." (`DeleteSecretRequest$RecoveryWindowInDays`,
/// this service's Smithy model.)
pub const MIN_RECOVERY_WINDOW_DAYS: i64 = 7;

/// The upper bound of the same documented range.
pub const MAX_RECOVERY_WINDOW_DAYS: i64 = 30;

/// How long a `ForceDeleteWithoutRecovery` deletion, which skips the recovery
/// window, stays marked for deletion before the store owes its permanent
/// removal.
///
/// "Secrets Manager performs the actual deletion with an asynchronous background
/// process, so there might be a short delay before the secret is permanently
/// deleted." (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's
/// Smithy model.) AWS documents no duration for that delay, so this span is the
/// emulator's OWN: long enough that a caller acting straight after the call
/// still observes the secret marked for deletion — and is refused by its state
/// rather than told it does not exist — short enough that a client retrying a
/// create of the same name does not wait on a human timescale.
pub const FORCE_DELETE_PURGE_DELAY: Duration = Duration::from_millis(500);

/// The message `ResourceNotFoundException` carries when a `SecretId` names no
/// secret this service holds.
///
/// `ResourceNotFoundException` is documented as "Secrets Manager can't find the
/// resource that you asked for." (this service's Smithy model), and the wire
/// message Secrets Manager answers with is this one constant sentence: it names
/// no identifier, so the request's `SecretId` is never reflected back into the
/// error body. Every operation that resolves a `SecretId` answers with THIS
/// constant, so one secret-not-found error reads the same whichever operation
/// raised it.
pub const SECRET_NOT_FOUND_MESSAGE: &str = "Secrets Manager can't find the specified secret.";

/// The message `InvalidNextTokenException` carries for a pagination token this
/// service cannot resume from.
///
/// `InvalidNextTokenException` is documented as "The `NextToken` value is
/// invalid." (this service's Smithy model); the wire message is this constant
/// sentence, and it likewise carries no part of the request. Every paginated
/// operation answers with THIS constant.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The pagination token is no longer valid.";

/// The message `InvalidRequestException` carries for a write that would create a
/// secret version without naming the version's id.
///
/// A write that stores a value creates a version, and "this parameter specifies
/// the unique identifier for the new version. If you use the Amazon Web Services
/// CLI or one of the Amazon Web Services SDKs to call this operation, then you
/// can leave this parameter empty. The CLI or SDK generates a random UUID for you
/// and includes it as the value for this parameter in the request. If you
/// generate a raw HTTP request to the Secrets Manager service endpoint, then you
/// must generate a ClientRequestToken and include it in the request."
/// (`CreateSecretRequest$ClientRequestToken`, and the same text on
/// `PutSecretValueRequest$ClientRequestToken` and
/// `UpdateSecretRequest$ClientRequestToken`, this service's Smithy model.) The
/// service sees only the request, so a token the client never generated is a
/// missing one: the version is refused rather than staked under an id the caller
/// never chose and cannot retry with. The wire message is this one constant
/// sentence, which interpolates nothing from the request or from state.
pub const MISSING_CLIENT_REQUEST_TOKEN_MESSAGE: &str =
    "You must provide a ClientRequestToken value. We recommend a UUID-type value.";

/// The message `InvalidRequestException` carries for an operation a caller
/// cannot perform on a secret that is inside its recovery window: the secret
/// still exists ("The secret is scheduled for deletion.",
/// `InvalidRequestException`, this service's Smithy model), so it is refused by
/// state rather than reported as missing.
pub const MARKED_FOR_DELETION_MESSAGE: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The message `InvalidRequestException` carries for a `CreateSecret` whose
/// `Name` is held by a secret inside its recovery window.
///
/// The name is still taken — "you cannot access the encrypted secret
/// information" of a secret scheduled for deletion, and the name cannot be
/// reused until the deletion completes or is cancelled — so the create is
/// refused by that secret's state. The sentence is FIXED: it states that a
/// secret with this name is already scheduled for deletion and interpolates
/// nothing, so the caller's own identifier is never reflected back into the
/// error body.
pub const NAME_SCHEDULED_FOR_DELETION_MESSAGE: &str = "You can't create this secret because a \
     secret with this name is already scheduled for deletion.";

/// The message a write that carries BOTH secret values is refused with.
///
/// "Either `SecretString` or `SecretBinary` must have a value, but not both."
/// (`CreateSecretRequest$SecretString`, and the same text on
/// `PutSecretValueRequest$SecretString` and `UpdateSecretRequest$SecretString`,
/// this service's Smithy model.) One fixed sentence, naming the two KINDS of
/// value rather than the members that carried them and interpolating neither
/// value: both are `@sensitive`. Every operation that can be handed both answers
/// with THIS constant.
pub const BOTH_SECRET_VALUES_MESSAGE: &str =
    "You can't specify both a binary secret value and a string secret value in the same secret.";

/// The message a request naming a `Name` this service cannot use is refused
/// with.
///
/// A name is the identity a secret is created under and the literal its ARN
/// carries, so a name outside what the service accepts — a character the
/// documented set does not list ("The secret name can contain ASCII letters,
/// numbers, and the following characters: /_+=.@-", `CreateSecretRequest$Name`,
/// this service's Smithy model), or a length outside `NameType`'s bounds —
/// names nothing this service can create. ONE fixed sentence states the rule for
/// every such name: it lists the character set and interpolates no part of the
/// request, so a rejected name is never echoed back. The set it states includes
/// the `!` that separates the reserved `<service>!…` form another AWS service's
/// own create may carry ([`service_owned`]).
pub const INVALID_SECRET_NAME_MESSAGE: &str = "Invalid name. Must be a valid name containing \
     alphanumeric characters, or any of the following: -/_+=.@!";

/// The message `GetSecretValue` refuses a request whose `VersionId` and
/// `VersionStage` name different versions with.
///
/// "If you include both this parameter and `VersionId`, the two parameters must
/// refer to the same secret version." (`GetSecretValueRequest$VersionStage`,
/// this service's Smithy model.) Both halves resolve — the version exists and
/// the label is attached to another version — so the request is not a miss but a
/// contradiction, and it is refused for the state of the secret rather than
/// reported as a version that is gone. The sentence is fixed and carries neither
/// identifier.
pub const VERSION_ID_AND_STAGE_DISAGREE_MESSAGE: &str =
    "You provided a VersionStage that is not associated to the provided VersionId.";

/// The message a request that writes a tag in the namespace AWS reserves for its
/// own is refused with.
///
/// A tag whose key starts with [`RESERVED_TAG_PREFIX`] is AWS's own ("Service
/// quotas for Tagging", Amazon Web Services General Reference, referenced from
/// `TagResource`'s documentation in this service's Smithy model): a caller may
/// neither create nor remove one. The sentence is fixed apart from the tag key
/// the request named, which is what tells a caller which of the keys it sent is
/// the reserved one.
pub fn system_tag_message(key: &str) -> String {
    format!("You can't modify system tags. Tag key: '{key}'.")
}

/// The order this service states `FilterNameStringType`'s admitted values in when it
/// refuses a value outside them.
///
/// The VALUES are the model's own, generated from it
/// ([`crate::ops::types::FILTER_NAME_STRING_TYPE_VALUES`]), so nothing here restates the
/// catalogue; the ORDER is the one Secrets Manager's own refusal of an unknown filter key
/// states them in, which is neither the order the model declares them in nor the sorted one
/// (recorded in
/// `issues/things-that-do-not-work/20261103T043509Z-secretsmanager-secrets-manager-version-selector-pending-deletion-and-rotation.md`,
/// whose cause 6 pins this ordering against a recorded response). A value set is a SET, so
/// the order a message states it in belongs to the service rather than to the model, and it
/// is fixed text — no member of the request and nothing in the store varies it — which is
/// why it lives here beside this service's other refusal wording, as the one copy every
/// refusal states the set from.
pub const STATED_FILTER_NAME_VALUE_SET: [&str; 7] = [
    "all",
    "owning-service",
    "name",
    "tag-key",
    "description",
    "tag-value",
    "primary-region",
];

/// Every enum value set this service states an order of its own for: one row per
/// enumerated type, the MODEL's values (the generated catalogue, matched as a set) beside
/// the service's ordering of them. A type absent from this table is stated as the violation
/// presents it, because this service has shown no other order for it. The lookup and the
/// restating are [`service_common::validation_message`]'s.
pub const STATED_VALUE_SETS: &service_common::validation_message::StatedEnumValueSets<'static> =
    &[(
        crate::ops::types::FILTER_NAME_STRING_TYPE_VALUES,
        &STATED_FILTER_NAME_VALUE_SET,
    )];

/// The message a tag list that names one key twice is refused with.
///
/// "Each tag is a key and value pair" and "Secrets Manager tag key names are case
/// sensitive. A tag with the key \"ABC\" is a different tag from one with key \"abc\"."
/// (`CreateSecretRequest$Tags`, this service's Smithy model): a key identifies one tag, so
/// a list repeating a key asks for two values for one tag and names no single tag set to
/// attach. Keys are compared exactly, as that documentation requires. ONE fixed sentence,
/// which names neither the repeated key nor anything else from the request — the wording is
/// the one a recorded refusal of a duplicate tag key carries, which is why it reads as a
/// bare clause with no closing period.
pub const DUPLICATE_TAG_KEYS_MESSAGE: &str = "Duplicate tag keys in request";

/// Whether `tags` names any key more than once.
///
/// A tag with no key names nothing to attach and is refused on its own, so a keyless tag is
/// not compared here.
pub fn repeats_a_tag_key(tags: &[Tag]) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    tags.iter()
        .filter_map(|tag| tag.key.as_deref())
        .any(|key| !seen.insert(key))
}

/// The message a write that would leave a secret carrying more tags than the
/// tagging quota allows is refused with (see [`TAGS_PER_SECRET`]).
///
/// One fixed sentence: it states that the request carried more tags than the
/// service accepts, and interpolates neither the quota nor any tag the request
/// named. Every operation that attaches or removes tags answers with THIS
/// constant, so one quota refusal reads the same whichever raised it.
pub const TOO_MANY_TAGS_MESSAGE: &str = "The request included too many tags.";

/// The message a write that does not state the one value to store is refused
/// with.
///
/// "Either `SecretString` or `SecretBinary` must have a value"
/// (`PutSecretValueRequest$SecretString`, this service's Smithy model), so a
/// write that creates a version carrying neither names nothing to store. One
/// fixed sentence, naming the two members a caller may choose between.
pub const NO_SECRET_VALUE_MESSAGE: &str =
    "You must provide either SecretString or SecretBinary.";

/// The `InvalidRequestException` message `RotateSecret` refuses a call with when the secret
/// has no Lambda rotation function to invoke: the documented cause "You tried to enable
/// rotation on a secret that doesn't already have a Lambda function ARN configured and you
/// didn't include such an ARN as a parameter in this call." (`InvalidRequestException`, this
/// service's Smithy model).
///
/// One fixed sentence, interpolating no secret, no ARN and no parameter value. It is the
/// answer for every way a call leaves the secret without one: a call that names no
/// `RotationLambdaARN` for a secret that stores none, and a call whose `RotationLambdaARN`
/// names no Lambda function at all — a value that is not a function's ARN associates no
/// rotation function with the secret.
pub const NO_ROTATION_FUNCTION_MESSAGE: &str =
    "No Lambda rotation function ARN is associated with this secret.";

/// The `InvalidRequestException` message `RotateSecret` refuses a call with while a
/// rotation of that secret has not finished: "If the AWSPENDING staging label is present
/// but not attached to the same version as AWSCURRENT, then any later invocation of
/// RotateSecret assumes that a previous rotation request is still in progress and returns
/// an error." (`RotateSecret`, this service's Smithy model).
///
/// One fixed sentence, carrying no version id, no secret and nothing else from the request:
/// what it reports is that a rotation of this secret is not complete and that the service
/// will try it again.
pub const ROTATION_IN_PROGRESS_MESSAGE: &str =
    "A previous rotation isn't complete. That rotation will be reattempted.";

/// The `InvalidParameterException` message `StopReplicationToReplica` refuses a
/// `SecretId` with that, in the Region the request is signed for, names a secret
/// which is NOT a replica — a replication primary, or a standalone secret that
/// has no replicas at all.
///
/// Secrets Manager words this refusal as ONE fixed sentence naming the class of
/// secret the call is not permitted on, and nothing else: no secret name, ARN or
/// Region from the request or from the store is interpolated into it, and nothing
/// is appended about where the call should instead have been made (a standalone
/// secret has no replica Region to redirect a caller to). Every secret refused
/// for not being a replica here — holding replicas or not — carries exactly this
/// text.
pub const NOT_A_REPLICA_MESSAGE: &str = "Operation not permitted on a primary/standalone secret.";

/// The `InvalidParameterException` message for a `RecoveryWindowInDays` outside
/// the documented range, stating that constraint: "The number of days from 7 to
/// 30 that Secrets Manager waits before permanently deleting the secret."
/// (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy model).
/// The bounds come from the same constants the validation enforces, and the
/// rejected value is not echoed back.
pub fn recovery_window_out_of_range_message() -> String {
    format!(
        "The RecoveryWindowInDays value must be between {MIN_RECOVERY_WINDOW_DAYS} and \
         {MAX_RECOVERY_WINDOW_DAYS} days (inclusive)."
    )
}

/// The message `LimitExceededException` carries for an attachment that would
/// take a version past the staging-label cap (see [`STAGES_PER_VERSION`]).
///
/// `LimitExceededException` is documented as "The request failed because it
/// would exceed one of the Secrets Manager quotas." (this service's Smithy
/// model); the wire message is this one constant sentence, which states the
/// quota rather than echoing the label or the version the request named.
pub const STAGE_LIMIT_MESSAGE: &str =
    "You exceeded the maximum number of staging labels allowed on a secret.";

/// The `ResourceNotFoundException` message for a `VersionId` the named secret
/// holds no version under, for an operation that moves a staging label onto that
/// version rather than reading a value.
///
/// The secret itself was found, so the missing thing is the version the request
/// named — and the caller cannot tell which of the ids it sent was wrong unless
/// the message names it.
pub fn version_not_found_message(version_id: &str) -> String {
    format!("Secrets Manager can't find the specified secret version: {version_id}")
}

/// The `ResourceNotFoundException` message for a READ of a secret value that
/// resolves the value by `VersionId` and finds no such version.
///
/// A read of a secret value reports the VALUE it could not find, naming the
/// request member it resolved by and that member's value — the same sentence
/// shape as the staging-label form below, whose fixed text differs only in the
/// member it names ([`stage_not_found_message`]). The only variable part is the
/// `VersionId` echoed from the request.
pub fn value_version_not_found_message(version_id: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for VersionId: {version_id}")
}

/// The `ResourceNotFoundException` message for a READ of a secret value that
/// resolves the value by staging label and finds no version carrying it.
///
/// The secret was found and the label was not, so the message names the label
/// the request asked for. Every operation that resolves a value BY STAGE — the
/// single read and the batched one — answers with THIS sentence.
pub fn stage_not_found_message(stage: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for staging label: {stage}")
}

/// The `ResourceExistsException` message for a `ClientRequestToken` that already
/// names a version of this secret holding a DIFFERENT value.
///
/// The token is the new version's id and an `@idempotencyToken`, so the same
/// token with the same value is the same request; the same token with another
/// value would modify an existing version, which no operation of this service
/// does. The message is built from the request's own token and the secret's ARN,
/// which is what identifies which of a batch of idempotent writes collided.
pub fn token_in_use_message(token: &str, arn: &str) -> String {
    format!(
        "You can't use ClientRequestToken {token} because that value is already in use for a \
         version of secret {arn}."
    )
}

/// The `LimitExceededException` message for a write that would leave a secret
/// holding more versions than this store keeps for one secret (see
/// [`VERSIONS_PER_SECRET`]).
pub fn version_quota_message() -> String {
    format!(
        "The request failed because it would exceed the maximum of {VERSIONS_PER_SECRET} versions \
         this emulator stores for one secret."
    )
}

/// Seconds in a day, for turning a recovery window into an instant.
const SECONDS_PER_DAY: u64 = 86_400;

/// Seconds in an hour, for turning a `rate(n hours)` rotation interval into a
/// duration.
const SECONDS_PER_HOUR: u64 = 3_600;

/// The shortest rotation interval a schedule expression may ask for: "You can
/// rotate a secret as often as every four hours"
/// (`RotationRulesType$ScheduleExpression`, this service's Smithy model).
pub const MIN_ROTATION_INTERVAL_HOURS: u64 = 4;

/// The longest rotation interval a schedule may ask for, in days.
///
/// The EMULATOR's own bound, not a documented AWS one: an interval has to be
/// addable to an instant, so a magnitude that cannot be represented is refused
/// where the caller is told why rather than carried into instant arithmetic. Its
/// value is the upper bound of the range the model puts on the other way of
/// naming the same interval, `AutomaticallyAfterDays`
/// (`AutomaticallyRotateAfterDaysType`, range 1..1000, this service's Smithy
/// model), so the two ways of asking for one interval accept the same span.
pub const MAX_ROTATION_INTERVAL_DAYS: u64 = 1_000;

/// The units a `rate()` rotation interval is expressed in, and the seconds each
/// one counts: "Secrets Manager rate() expressions represent the interval in
/// hours or days that you want to rotate your secret, for example rate(12
/// hours) or rate(10 days)" (`RotationRulesType$ScheduleExpression`, this
/// service's Smithy model). Both the singular and the plural are accepted, as
/// the documented examples use the plural and a rate of one is singular.
const RATE_UNITS: &[(&str, u64)] = &[
    ("hour", SECONDS_PER_HOUR),
    ("hours", SECONDS_PER_HOUR),
    ("day", SECONDS_PER_DAY),
    ("days", SECONDS_PER_DAY),
];

/// How many fields the documented cron form carries: "A cron() or rate()
/// expression that defines the schedule for rotating your secret", in the
/// six-field form `cron(minutes hours day-of-month month day-of-week year)`
/// (`RotationRulesType$ScheduleExpression`, this service's Smithy model).
const CRON_FIELDS: usize = 6;

/// What a schedule expression that is not one of the two documented forms is
/// refused with.
pub const SCHEDULE_EXPRESSION_SYNTAX: &str = "ScheduleExpression must be a rate() or cron() \
     expression, for example rate(10 days) or cron(0 8 1 * ? *).";

/// What a rotation `Duration` with no schedule to start it from is refused with.
///
/// "The window starts according to the `ScheduleExpression`."
/// (`RotationRulesType$Duration`, this service's Smithy model.) A duration is the
/// LENGTH of the window a scheduled rotation may happen in, so rules that name a
/// duration and no schedule — neither a `ScheduleExpression` nor the
/// `AutomaticallyAfterDays` that stands for one — describe a window with no
/// start, and the parameter is refused rather than stored.
///
/// The CLASS and the status of this refusal are AWS's (`InvalidParameterException`, HTTP
/// 400, as a recorded refusal of these rules shows); the SENTENCE states the documented
/// rule above and is this emulator's own wording of it, since no recording of this service
/// discloses the text — it interpolates nothing from the request either way.
pub const DURATION_WITHOUT_SCHEDULE: &str =
    "You can't specify a rotation Duration without a rotation schedule.";

/// The schedule the rules in `rules` rotate the secret on, or the
/// `InvalidParameterException` message for rules that cannot hold together.
///
/// `Ok(None)` is rules this service computes no due instant from: rules that
/// name no schedule at all (an empty structure), and a `cron()` expression in the
/// service's own wildcard dialect — see [`SERVICE_CRON_WILDCARDS`].
pub fn parse_rotation_rules(
    rules: &RotationRulesType,
) -> Result<Option<RotationSchedule>, String> {
    // "In RotateSecret, you can set the rotation schedule in RotationRules with
    // AutomaticallyAfterDays or ScheduleExpression, but not both."
    if rules.automatically_after_days.is_some() && rules.schedule_expression.is_some() {
        return Err("You can specify either AutomaticallyAfterDays or ScheduleExpression in \
                    RotationRules, but not both."
            .to_string());
    }
    // A window needs a start, and the schedule is what states one.
    if rules.duration.is_some()
        && rules.automatically_after_days.is_none()
        && rules.schedule_expression.is_none()
    {
        return Err(DURATION_WITHOUT_SCHEDULE.to_string());
    }
    if let Some(days) = rules.automatically_after_days {
        return Ok(Some(RotationSchedule::Every(interval_of(
            u64::try_from(days).unwrap_or_default(),
            SECONDS_PER_DAY,
            "AutomaticallyAfterDays",
        )?)));
    }
    match rules.schedule_expression.as_deref() {
        Some(expression) => parse_schedule_expression(expression),
        None => Ok(None),
    }
}

/// The schedule a secret's rotation rules state, which is what answers when that
/// secret's next rotation is due.
///
/// The two ways `RotationRulesType` states one: an INTERVAL
/// (`AutomaticallyAfterDays`, or a `rate()` expression), where "Secrets Manager
/// calculates the next rotation date based on the previous rotation"; and a
/// CALENDAR (a `cron()` expression), which names the occurrences themselves —
/// "Secrets Manager rotation schedules use UTC time zone"
/// (`RotationRulesType$AutomaticallyAfterDays` / `$ScheduleExpression`, this
/// service's Smithy model).
#[derive(Debug, Clone)]
pub enum RotationSchedule {
    /// One rotation due every span of this length, counted from the rotation
    /// before it.
    Every(Duration),
    /// One rotation due at each instant the expression names on the UTC
    /// calendar.
    Calendar(Box<::cron::Schedule>),
}

impl RotationSchedule {
    /// The next instant this schedule makes a rotation due, after `anchor`.
    ///
    /// For an interval that is one interval past `anchor`, the instant the
    /// previous rotation (or the configuration of the schedule) puts the next
    /// one at. For a calendar it is the first occurrence the expression names
    /// after `anchor`, read in UTC and so independent of where this emulator
    /// runs — the instant the rotation window opens, which is what `Duration`
    /// measures that window from ("The window starts according to the
    /// ScheduleExpression", `RotationRulesType$Duration`, this service's Smithy
    /// model).
    ///
    /// `None` when this schedule names no instant at all ahead of `anchor`: an
    /// interval no clock can represent past it, and a `cron()` expression whose
    /// year field is already behind it, which names no further occurrence.
    #[must_use]
    pub fn next_after(&self, anchor: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Every(interval) => anchor.checked_add(*interval),
            Self::Calendar(occurrences) => {
                let from: ::chrono::DateTime<::chrono::Utc> = anchor.into();
                occurrences.after(&from).next().map(SystemTime::from)
            }
        }
    }
}

/// The schedule a `ScheduleExpression` names.
///
/// A `rate()` expression names an interval outright. A `cron()` expression names
/// occurrences on the calendar, in the six-field form the service documents, and
/// is evaluated in UTC: a `?` ("no specific value") in the day-of-month or
/// day-of-week field is the same statement as `*` in the grammar the evaluator
/// reads, and that grammar carries a seconds field this form does not, so a
/// literal `0` seconds is prepended — the expression names instants on the
/// minute. A field in the service's own wildcard dialect is read on the
/// service's grammar and named no occurrence by this evaluator
/// ([`cron_occurrences`]).
fn parse_schedule_expression(expression: &str) -> Result<Option<RotationSchedule>, String> {
    let expression = expression.trim();
    let body = |keyword: &str| {
        expression
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
    };
    if let Some(rate) = body("rate") {
        let mut parts = rate.split_whitespace();
        let (Some(value), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(SCHEDULE_EXPRESSION_SYNTAX.to_string());
        };
        let value: u64 = value
            .parse()
            .map_err(|_| SCHEDULE_EXPRESSION_SYNTAX.to_string())?;
        let seconds = RATE_UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, seconds)| *seconds)
            .ok_or_else(|| SCHEDULE_EXPRESSION_SYNTAX.to_string())?;
        return Ok(Some(RotationSchedule::Every(interval_of(
            value,
            seconds,
            "ScheduleExpression",
        )?)));
    }
    if let Some(body) = body("cron") {
        let fields: Vec<&str> = body.split_whitespace().collect();
        if fields.len() != CRON_FIELDS {
            return Err(format!(
                "ScheduleExpression must carry a cron expression of {CRON_FIELDS} fields \
                 (minutes hours day-of-month month day-of-week year), not {}.",
                fields.len()
            ));
        }
        return Ok(cron_occurrences(&fields)?
            .map(|occurrences| RotationSchedule::Calendar(Box::new(occurrences))));
    }
    Err(SCHEDULE_EXPRESSION_SYNTAX.to_string())
}

/// The indexes of the day-of-month and day-of-week fields, the two in which `?`
/// states "no specific value".
const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

/// The field value stating "no specific value", which is the same statement `*`
/// makes in the grammar the occurrences are evaluated on.
const NO_SPECIFIC_VALUE: &str = "?";

/// The seconds field that grammar carries and the documented six-field form does
/// not: an occurrence the documented form names falls on the minute.
const ON_THE_MINUTE: &str = "0";

/// The field values a normalised expression carries for every field but the one
/// being judged, so a field is read in the place it stands and not against the
/// rest of the expression.
const NEUTRAL_CRON_FIELDS: [&str; CRON_FIELDS + 1] = ["0", "0", "0", "*", "*", "*", "*"];

/// The wildcards the schedule-expression reference documents on top of the ones
/// the evaluated grammar reads: `L` (last), `W` (nearest weekday) and `#` (nth
/// weekday of the month). A field carrying one of them states occurrences that
/// grammar cannot read at all, so it is read on the SERVICE's grammar rather than
/// refused on the evaluator's.
const SERVICE_CRON_WILDCARDS: [char; 3] = ['L', 'W', '#'];

/// The occurrences the documented six cron `fields` name, the refusal for fields
/// that name none, or `Ok(None)` for an expression in the service's own wildcard
/// dialect — one this service stores and reports back, and computes no occurrence
/// from ([`SERVICE_CRON_WILDCARDS`]).
///
/// Every field is judged on the EVALUATED grammar first, so a value that grammar
/// reads is read there whatever letters it is spelled with — a month or weekday
/// name carrying one of the dialect's letters (`JUL`, `WED`, `MON-WED`) is an
/// ordinary named field, not a wildcard. Only a field that grammar cannot read at
/// all is put to the service's own dialect, and a field neither reads is refused.
fn cron_occurrences(fields: &[&str]) -> Result<Option<::cron::Schedule>, String> {
    let mut service_dialect = false;
    for (index, field) in fields.iter().enumerate() {
        if cron_field_reads(index, field) {
            continue;
        }
        if !names_service_wildcard(field) {
            return Err(SCHEDULE_EXPRESSION_SYNTAX.to_string());
        }
        service_dialect = true;
    }
    // An expression whose own dialect that grammar does not read is not evaluated
    // as a whole either: it is stored and reported back, and names no instant
    // this service computes.
    if service_dialect {
        return Ok(None);
    }
    normalised_cron(fields)
        .parse()
        .map(Some)
        .map_err(|_| SCHEDULE_EXPRESSION_SYNTAX.to_string())
}

/// Whether `field` carries one of the wildcards this service's cron dialect
/// documents and the evaluated grammar does not read.
///
/// Asked only of a field that grammar could not read, and case-insensitively: the
/// dialect's letters are documented in capitals, and a field is the same
/// statement however it is cased.
fn names_service_wildcard(field: &str) -> bool {
    field
        .chars()
        .any(|carried| SERVICE_CRON_WILDCARDS.contains(&carried.to_ascii_uppercase()))
}

/// Whether `field` states something the evaluated grammar reads in the place
/// `index` names, judged with every other field neutral.
fn cron_field_reads(index: usize, field: &str) -> bool {
    let mut expression = NEUTRAL_CRON_FIELDS;
    expression[index + 1] = cron_field_as_evaluated(index, field);
    expression
        .join(" ")
        .parse::<::cron::Schedule>()
        .is_ok()
}

/// The six documented fields as the expression the occurrences are evaluated on.
fn normalised_cron(fields: &[&str]) -> String {
    let mut expression = String::from(ON_THE_MINUTE);
    for (index, field) in fields.iter().enumerate() {
        expression.push(' ');
        expression.push_str(cron_field_as_evaluated(index, field));
    }
    expression
}

/// One field as that grammar reads it: `?` is the `*` it spells the same
/// statement with, in the two fields `?` is a statement in.
fn cron_field_as_evaluated(index: usize, field: &str) -> &str {
    if field == NO_SPECIFIC_VALUE && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
        "*"
    } else {
        field
    }
}

/// `value` units of `seconds` as a rotation interval, refused when it falls
/// outside the interval the service's own rules allow: no more often than every
/// four hours, and no longer than the maximum `AutomaticallyAfterDays` names.
///
/// `member` is the member the refusal names, so the caller is told which half of
/// `RotationRules` it has to fix.
fn interval_of(value: u64, seconds: u64, member: &str) -> Result<Duration, String> {
    let total = value.saturating_mul(seconds);
    if total < MIN_ROTATION_INTERVAL_HOURS * SECONDS_PER_HOUR {
        return Err(format!(
            "{member} must schedule a rotation no more often than every \
             {MIN_ROTATION_INTERVAL_HOURS} hours."
        ));
    }
    if total > MAX_ROTATION_INTERVAL_DAYS * SECONDS_PER_DAY {
        return Err(format!(
            "{member} must not schedule a rotation further apart than every \
             {MAX_ROTATION_INTERVAL_DAYS} days."
        ));
    }
    Ok(Duration::from_secs(total))
}

/// How many secrets the store holds before `CreateSecret` refuses with the
/// modelled `LimitExceededException`.
///
/// Secrets Manager publishes an adjustable per-Region secret quota, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, not a claim about any account's
/// limit. It bounds the WHOLE store rather than one region's share of it: the
/// region a secret is homed in comes from the request, so a per-region bound
/// would still let a caller grow the store without end by varying the region it
/// signs for. Every record it holds is released by `DeleteSecret`, and it sits
/// far above any workflow an emulator serves.
pub const SECRET_QUOTA: usize = 1_000;

/// How many versions one secret holds before a further `PutSecretValue` has to
/// make room by evicting a deprecated one.
///
/// Secrets Manager removes a version once it carries no staging label, so
/// nothing a caller can reach by stage depends on a deprecated version
/// surviving; keeping a bounded tail of them is what lets a deprecated
/// `VersionId` still be read for a while, as the real service does. The bound is
/// the emulator's own, and it is what stops a loop of `PutSecretValue` calls
/// against one secret from growing the store without end.
pub const VERSIONS_PER_SECRET: usize = 100;

/// How many staging labels of its OWN a caller attaches to one version of a
/// secret before a further attachment is refused.
///
/// `SecretVersionStagesType` — the list of "all of the staging labels currently
/// attached to this version of the secret" — is modelled with a maximum length
/// of 20 (this service's Smithy model), so twenty is what one version carries.
/// The labels the service itself moves (`AWSCURRENT`, `AWSPREVIOUS`,
/// `AWSPENDING`) are not counted against it: they are attached by rotation and
/// by every write of a value, so counting them would refuse a caller's
/// twentieth label on a version that is simply current.
pub const STAGES_PER_VERSION: usize = 20;

/// How many secrets one batched read resolves, and the page size it falls back
/// to when the caller names none.
///
/// `BatchGetSecretValue` "retrieves the contents of the encrypted fields
/// SecretString or SecretBinary for up to 20 secrets", `SecretIdListType` is
/// modelled with a maximum length of 20 and `MaxResultsBatchType` with a range
/// of 1..20 (this service's Smithy model), so twenty secrets is one batch
/// whichever way the caller selects them.
pub const SECRETS_PER_BATCH: usize = 20;

/// The region a resource is placed in when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many tags one secret carries before a further `TagResource` is refused.
///
/// The AWS tagging quota is 50 tags per resource ("Service quotas for Tagging",
/// Amazon Web Services General Reference, referenced from `TagResource`'s own
/// documentation in this service's Smithy model). It is what stops a loop of
/// `TagResource` calls against one secret from growing the store without end.
pub const TAGS_PER_SECRET: usize = 50;

/// The prefix AWS reserves for its own tags: a caller may neither create nor
/// modify a tag whose key starts with it ("Service quotas for Tagging", Amazon
/// Web Services General Reference).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// How many replica Regions one secret holds before a further replication
/// request is refused, and the longest `AddReplicaRegions` list a request may
/// carry.
///
/// A replica lives in an AWS Region, and the set of Regions is finite and
/// published — but `RegionType` is only a length-bounded pattern
/// (`^([a-z]+-)+\d+$`), and this emulator validates no Region catalogue, so a
/// caller could otherwise mint unbounded distinct Region strings and grow one
/// secret's replica list without end. This bound is therefore the emulator's
/// OWN, chosen above the number of Regions AWS publishes, and it is what keeps
/// the resident state (and the per-request walk in [`settle_replicas`]) finite.
/// `AddReplicaRegionListType` declares no maximum length, so the request's own
/// list is bounded by the same constant before anything is counted or stored.
///
/// [`settle_replicas`]: Secret::settle_replicas
pub const REPLICAS_PER_SECRET: usize = 64;

/// The KMS key a replica is encrypted with when the request names none: "The
/// ARN, key ID, or alias of the KMS key to encrypt the secret. If you don't
/// include this field, Secrets Manager uses `aws/secretsmanager`."
/// (`ReplicaRegionType$KmsKeyId`, this service's Smithy model.) The alias form
/// is what the service reports back in a `ReplicationStatusType`.
pub const DEFAULT_KMS_KEY_ALIAS: &str = "alias/aws/secretsmanager";

/// The key id Secrets Manager reports for a secret version encrypted under the
/// account's default Secrets Manager key.
///
/// "If you don't specify this value, then Secrets Manager uses the key
/// `aws/secretsmanager`." (`CreateSecretRequest$KmsKeyId`, this service's
/// Smithy model.) Where the service reports the keys a version is encrypted
/// under (`SecretVersionsListEntry$KmsKeyIds`) it does not name that key by id
/// or by alias: it names it with this one token, so a caller can tell a version
/// on the account default key from one on a key of the secret's own.
pub const DEFAULT_ENCRYPTION_KEY_ID: &str = "DefaultEncryptionKey";

/// Whether `kms_key_id` names the account's default Secrets Manager key —
/// [`DEFAULT_KMS_KEY_ALIAS`] — rather than a key of the secret's own.
///
/// The alias prefix is optional in a request ("An alias is always prefixed by
/// `alias/`", `CreateSecretRequest$KmsKeyId`), so both forms of that one alias
/// name the same key, and a secret that names it is on the account default key
/// exactly as one that names no key at all is.
pub fn names_default_kms_key(kms_key_id: &str) -> bool {
    let unprefixed = |key: &str| key.strip_prefix("alias/").unwrap_or(key).to_string();
    unprefixed(kms_key_id) == unprefixed(DEFAULT_KMS_KEY_ALIAS)
}

/// `ReplicationStatusType$Status` while the replica is being created
/// (`StatusType`, this service's Smithy model).
pub const REPLICA_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ReplicationStatusType$Status` once the replica exists.
pub const REPLICA_STATUS_IN_SYNC: &str = "InSync";

/// `ReplicationStatusType$Status` for a replica the service could not create
/// (`StatusType`, this service's Smithy model).
pub const REPLICA_STATUS_FAILED: &str = "Failed";

/// How long a fresh replica stays `InProgress` before the clock owes it the move
/// to `InSync`.
///
/// Secrets Manager documents no duration for creating a replica, so this span is
/// the emulator's OWN: long enough that a caller polling straight after
/// `ReplicateSecretToRegions` still observes `InProgress`, short enough that a
/// client waiting for `InSync` does not wait on a human timescale.
pub const REPLICA_SYNC_DELAY: Duration = Duration::from_millis(750);

/// The characters a secret name may carry besides ASCII letters and digits:
/// "The secret name can contain ASCII letters, numbers, and the following
/// characters: /_+=.@-" (`CreateSecretRequest$Name`, this service's Smithy
/// model). The model declares no pattern for `NameType`, so the documented
/// charset is enforced in the handler.
pub const SECRET_NAME_EXTRA_CHARS: &str = "/_+=.@-";

/// Whether `name` is a secret name this service accepts.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SECRET_NAME_EXTRA_CHARS.contains(c))
}

/// Whether a secret satisfies one `Filter`.
///
/// "You can prefix your search value with an exclamation mark (`!`) in order to
/// perform negation filters" (`Filter$Values`, this service's Smithy model), so a
/// filter's values are of two kinds and they do different work: the plain ones
/// are alternatives (any one matching is enough), and every negated one EXCLUDES
/// the secrets it matches, whatever the others select. A filter that carries only
/// negated values selects everything they do not exclude; separate filters all
/// have to hold.
///
/// The attributes are read from the view ([`SecretView`]), so a replica is
/// selected by the content it serves — the primary's — under its own name and
/// Region.
///
/// Every operation that selects secrets by filter — the listing and the batched
/// read, which share one `Filter` shape — selects with THIS function, so the two
/// cannot disagree about what a filter means.
pub fn matches_filter(view: SecretView<'_>, filter: &Filter) -> bool {
    let Some(key) = filter.key.as_deref() else {
        return true;
    };
    let values = filter.values.as_deref().unwrap_or(&[]);
    if values.is_empty() {
        return true;
    }
    let (negated, selecting): (Vec<&String>, Vec<&String>) =
        values.iter().partition(|value| value.starts_with('!'));
    let excluded = negated.iter().any(|value| {
        matches_filter_value(view, key, value.strip_prefix('!').unwrap_or(value))
    });
    if excluded {
        return false;
    }
    selecting.is_empty()
        || selecting
            .iter()
            .any(|value| matches_filter_value(view, key, value))
}

/// Whether a secret matches one filter value.
///
/// The per-key rules come from `Filter$Key` in this service's model: `name`,
/// `tag-key`, `tag-value`, `primary-region` and `owning-service` are
/// case-sensitive prefix matches, `description` is a prefix match that is not
/// case-sensitive, and `all` "breaks the filter value string into words and then
/// searches all attributes for matches", not case-sensitive.
///
/// The two keys that search TEXT — `description` and `all` — match a prefix of
/// any WORD of what they search ([`words`]), not of the whole attribute alone: a
/// description is a sentence, and the word is the unit a search of one selects by.
/// `all` searches the words of every attribute, and — "breaks the filter value
/// string into words" — all of the words of its own value have to be found.
fn matches_filter_value(view: SecretView<'_>, key: &str, value: &str) -> bool {
    let SecretView { secret, content } = view;
    match key {
        "name" => secret.name.starts_with(value),
        "description" => content
            .description
            .as_deref()
            .is_some_and(|description| matches_text(description, value)),
        "tag-key" => content
            .tags
            .iter()
            .filter_map(|tag| tag.key.as_deref())
            .any(|tag_key| tag_key.starts_with(value)),
        "tag-value" => content
            .tags
            .iter()
            .filter_map(|tag| tag.value.as_deref())
            .any(|tag_value| tag_value.starts_with(value)),
        // The Region the secret's primary is in, which for a replica is another
        // Region than its own and for every other secret is its own.
        "primary-region" => secret
            .replica_of
            .as_deref()
            .unwrap_or(&secret.region)
            .starts_with(value),
        "owning-service" => content
            .owning_service
            .as_deref()
            .is_some_and(|owner| owner.starts_with(value)),
        "all" => {
            let attributes = searchable_attributes(view);
            value.split_whitespace().all(|word| {
                attributes
                    .iter()
                    .any(|attribute| matches_text(attribute, word))
            })
        }
        // A key the model does not name matches nothing.
        _ => false,
    }
}

/// Whether `value` is a prefix of `text` or of any word of it, ignoring case —
/// what the filter keys that search text match by.
fn matches_text(text: &str, value: &str) -> bool {
    starts_with_ignoring_case(text, value)
        || words(text).any(|word| starts_with_ignoring_case(word, value))
}

/// The words of a searchable attribute: its maximal runs of alphanumeric
/// characters, so the words of a sentence, of a path-like secret name and of a
/// hyphenated tag value are all reached.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// The attributes the `all` filter key searches.
fn searchable_attributes(view: SecretView<'_>) -> Vec<&str> {
    let SecretView { secret, content } = view;
    let mut attributes = vec![secret.name.as_str(), secret.region.as_str()];
    attributes.extend(content.description.as_deref());
    attributes.extend(content.owning_service.as_deref());
    for tag in &content.tags {
        attributes.extend(tag.key.as_deref());
        attributes.extend(tag.value.as_deref());
    }
    attributes
}

/// A prefix match that ignores ASCII case.
///
/// Compares BYTES rather than slicing the haystack at `prefix.len()`: both
/// operands are request-derived, so a byte-indexed slice would panic whenever
/// that offset fell inside a multi-byte character of the haystack. Case folding
/// is ASCII-only, as `eq_ignore_ascii_case` makes it: a non-ASCII byte has to
/// match exactly, which is the most the model's "not case-sensitive" pins down.
fn starts_with_ignoring_case(haystack: &str, prefix: &str) -> bool {
    let (haystack, prefix) = (haystack.as_bytes(), prefix.as_bytes());
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The wildcard a policy statement names when it grants access to every
/// principal.
pub const POLICY_WILDCARD: &str = "*";

/// What `ValidateResourcePolicy` refuses a document that fails the syntax check
/// with: that operation "[c]hecks for correct syntax in a policy" (this
/// service's Smithy model) and reports the failure as this one constant
/// sentence, naming no part of the document.
pub const VALIDATE_SYNTAX_ERROR_MESSAGE: &str = "This resource policy contains a syntax error.";

/// What a resource policy document that is not a policy document is refused
/// with: `MalformedPolicyDocumentException` is "The resource policy has syntax
/// errors." (this service's Smithy model).
///
/// One fixed sentence, naming no part of the document and no secret, and the
/// SAME sentence wherever the service speaks about that condition: the
/// `MalformedPolicyDocumentException` an attach is refused with, and the message
/// the syntax check reports on the validation channel
/// ([`VALIDATE_SYNTAX_ERROR_MESSAGE`]) — so a caller using
/// `ValidateResourcePolicy` as a pre-flight guard reads the very text the
/// refusal will carry.
pub const MALFORMED_POLICY_MESSAGE: &str = VALIDATE_SYNTAX_ERROR_MESSAGE;

/// What this service says about a resource policy that grants public access to a
/// secret: `PublicPolicyException` is "The BlockPublicPolicy parameter is set to
/// true, and the resource policy did not prevent broad access to the secret."
/// (this service's Smithy model).
///
/// One fixed sentence, naming no part of the document, no secret and no
/// principal, and the SAME sentence wherever the service speaks about that
/// condition: the `PublicPolicyException` a `BlockPublicPolicy` request is
/// refused with, and the message the broad-access check reports on the
/// validation channel ([`BROAD_ACCESS_CHECK_MESSAGE`]).
pub const PUBLIC_POLICY_MESSAGE: &str =
    "You can not attach a resource policy that grants Public access to your secret.";

/// The policy elements a statement can name a principal under. A principal is
/// either the bare wildcard string or a map from principal type to one or more
/// identifiers, so the values of every type are what a wildcard can appear in.
const PRINCIPAL_ELEMENTS: &[&str] = &["Principal", "NotPrincipal"];

/// Parse a resource policy document, or answer the
/// `MalformedPolicyDocumentException` message for one that is not a policy
/// document.
///
/// The grammar enforced is the IAM policy-document one: the document is a JSON
/// object, its `Statement` element is one statement or a non-empty list of
/// them, and every statement is an object carrying an `Effect` of `Allow` or
/// `Deny`. Everything else a statement may carry is passed through untouched,
/// so a document this emulator does not interpret is still reported whole.
pub fn parse_resource_policy(document: &str) -> Result<Value, String> {
    let parsed: Value =
        ::serde_json::from_str(document).map_err(|_| MALFORMED_POLICY_MESSAGE.to_string())?;
    if !parsed.is_object() {
        return Err(MALFORMED_POLICY_MESSAGE.to_string());
    }
    let statements = policy_statements(&parsed).ok_or_else(|| MALFORMED_POLICY_MESSAGE.to_string())?;
    if statements.is_empty() {
        return Err(MALFORMED_POLICY_MESSAGE.to_string());
    }
    for statement in &statements {
        if statement_effect(statement).is_none() {
            return Err(MALFORMED_POLICY_MESSAGE.to_string());
        }
    }
    Ok(parsed)
}

/// The statements of a policy document: the single statement a `Statement`
/// object is, or the list one holds. `None` for a document whose `Statement` is
/// missing or is neither.
fn policy_statements(policy: &Value) -> Option<Vec<&Value>> {
    match policy.get("Statement")? {
        Value::Object(_) => Some(vec![&policy["Statement"]]),
        Value::Array(items) => items
            .iter()
            .all(Value::is_object)
            .then(|| items.iter().collect()),
        _ => None,
    }
}

/// A statement's effect, lower-cased, or `None` when it names neither of the
/// two the grammar allows.
fn statement_effect(statement: &Value) -> Option<String> {
    let effect = statement.get("Effect")?.as_str()?.to_ascii_lowercase();
    matches!(effect.as_str(), "allow" | "deny").then_some(effect)
}

/// Whether a policy document grants broad access to the secret, which is what
/// a request that blocks public policies refuses: "Specifies whether to block
/// resource-based policies that allow broad access to the secret, for example
/// those that use a wildcard for the principal"
/// (`PutResourcePolicyRequest$BlockPublicPolicy`, this service's Smithy model).
///
/// Broad access is an ALLOW statement whose principal is the wildcard — the
/// bare `"*"`, or `"*"` among the identifiers of any principal type it maps —
/// AND whose `Condition` does not bind the caller to one this account names
/// ([`statement_conditions_bind_the_caller`]). A policy whose every allow
/// statement names a concrete principal, and one that only denies, grant no
/// broad access however many statements they carry.
pub fn policy_grants_public_access(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            statement_effect(statement).as_deref() == Some("allow")
                && names_wildcard_principal(statement)
                && !statement_conditions_bind_the_caller(statement)
        })
}

/// Condition keys that bind a request to a caller something names — one principal,
/// one account, one organization, one calling service or one network path.
///
/// A wildcard principal under a clause over one of these reaches exactly the callers
/// that key admits rather than everybody, which is why it is not broad access: the
/// statement a `Condition` like this carries is the documented way to open a secret to
/// an account or an organization without opening it to the world. The names are AWS's
/// global condition context keys for the principal making the request and for the
/// service or resource making it on the principal's behalf (IAM User Guide, "AWS global
/// condition context keys"), compared case-insensitively as IAM compares key names.
///
/// The `aws:Resource…` family is deliberately absent: those keys describe the account
/// that OWNS the secret rather than the caller, so a clause over one of them holds for
/// every caller.
const CALLER_BINDING_CONDITION_KEYS: &[&str] = &[
    "aws:PrincipalAccount",
    "aws:PrincipalArn",
    "aws:PrincipalOrgID",
    "aws:PrincipalOrgPaths",
    "aws:PrincipalIsAWSService",
    "aws:PrincipalServiceName",
    "aws:userid",
    "aws:username",
    "aws:SourceAccount",
    "aws:SourceArn",
    "aws:SourceOrgID",
    "aws:SourceOrgPaths",
    "aws:SourceOwner",
    "aws:SourceVpc",
    "aws:SourceVpce",
    "aws:SourceIp",
    "aws:VpcSourceIp",
];

/// Condition key FAMILIES that bind the caller, each carrying a suffix naming one tag
/// of the principal (`aws:PrincipalTag/<tag-key>`), from the same documentation.
const CALLER_BINDING_CONDITION_KEY_FAMILIES: &[&str] = &["aws:PrincipalTag/"];

/// The IAM operators whose comparison says what a request must NOT be, and the suffix
/// that makes a clause hold when the key is ABSENT: neither binds a caller, because the
/// caller a wildcard principal opens the secret to is exactly the one that carries no
/// value for the key.
///
/// `Not` is the spelling IAM gives every negated comparison (`StringNotEquals`,
/// `ArnNotLike`, `StringNotEqualsIgnoreCase`, …), which is the same reading the shared
/// evaluation of a policy makes of an operator name
/// ([`::service_common::iam_authorization`]).
const NEGATED_OPERATOR_INFIX: &str = "Not";
const IF_EXISTS_SUFFIX: &str = "IfExists";

/// The set qualifiers an operator may carry, which name how MANY of a multi-valued key's
/// values the comparison has to hold for rather than what the comparison is: a clause's
/// operator is read with the qualifier stripped off ([`clause_binds_the_caller`]).
const SET_QUALIFIER_SEPARATOR: char = ':';

/// The `Null` operator, which compares PRESENCE: `"false"` demands the key be there,
/// which binds the caller to one that carries it, while `"true"` demands the opposite
/// and binds nobody.
const NULL_OPERATOR: &str = "Null";
const NULL_KEY_MUST_EXIST: &str = "false";

/// Whether a statement's `Condition` binds the request to a caller something names, so
/// its wildcard principal does not reach the public.
///
/// A `Condition` is a conjunction, so ONE clause that binds the caller is enough. A
/// clause binds when its key is one of [`CALLER_BINDING_CONDITION_KEYS`] (or a family of
/// [`CALLER_BINDING_CONDITION_KEY_FAMILIES`]), its operator is a positive comparison, and
/// EVERY value it accepts names something concrete — a value carrying an IAM wildcard
/// admits callers nothing named, so a clause accepting one is no narrower than the
/// wildcard principal itself.
fn statement_conditions_bind_the_caller(statement: &Value) -> bool {
    let Some(Value::Object(clauses)) = statement.get("Condition") else {
        return false;
    };
    clauses.iter().any(|(operator, compared)| {
        let Value::Object(keys) = compared else {
            return false;
        };
        keys.iter()
            .any(|(key, values)| clause_binds_the_caller(operator, key, values))
    })
}

/// Whether one `(operator, key, values)` clause binds the caller.
fn clause_binds_the_caller(operator: &str, key: &str, values: &Value) -> bool {
    if !binds_the_caller(key) {
        return false;
    }
    // The comparison is what follows the set qualifier, so it is the LAST
    // `:`-separated segment of the clause's name (`ForAnyValue:StringNotEquals` compares
    // with `StringNotEquals`); an operator carrying no qualifier is that segment itself.
    let comparison = operator
        .rsplit(SET_QUALIFIER_SEPARATOR)
        .next()
        .unwrap_or(operator);
    if comparison.contains(NEGATED_OPERATOR_INFIX) || comparison.ends_with(IF_EXISTS_SUFFIX) {
        return false;
    }
    let compared = condition_values(values);
    if compared.is_empty() {
        return false;
    }
    if comparison == NULL_OPERATOR {
        return compared
            .iter()
            .all(|value| value.eq_ignore_ascii_case(NULL_KEY_MUST_EXIST));
    }
    compared
        .iter()
        .all(|value| !names_every_caller(key, value))
}

/// Whether a condition key is one this service reads as binding the caller.
fn binds_the_caller(key: &str) -> bool {
    CALLER_BINDING_CONDITION_KEYS
        .iter()
        .any(|binding| key.eq_ignore_ascii_case(binding))
        || CALLER_BINDING_CONDITION_KEY_FAMILIES.iter().any(|family| {
            key.len() > family.len()
                && key
                    .as_bytes()
                    .get(..family.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(family.as_bytes()))
        })
}

/// The values one condition clause compares against: the single value it names, or the
/// list it holds. A value that is not a string is rendered as the document wrote it, so
/// the flag and number forms a document may use are compared as text.
fn condition_values(values: &Value) -> Vec<String> {
    match values {
        Value::Array(items) => items.iter().flat_map(condition_values).collect(),
        Value::String(value) => vec![value.clone()],
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

/// Whether a condition value is no narrower than the wildcard principal, judged on the kind
/// of value the KEY carries.
///
/// Three kinds are judged on their own terms, which is the reading the tree's other
/// public-access judgement makes of the same clause
/// (`service-com-amazonaws-accessanalyzer-accessanalyzer`'s `CheckNoPublicAccess`):
///
/// * a FLAG key narrows only in the polarity that excludes nobody else
///   ([`BINDING_FLAG_VALUES`]) — the opposite polarity excludes one kind of caller and leaves
///   every other, the anonymous one included;
/// * a NETWORK ADDRESS narrows unless its CIDR covers every address
///   ([`NETWORK_ADDRESS_KEYS`]), since `0.0.0.0/0` admits every caller there is;
/// * every other value narrows only when it names something concrete, so an IAM wildcard
///   anywhere in it, and the empty value, match callers nothing named.
fn names_every_caller(key: &str, value: &str) -> bool {
    let value = value.trim();
    if let Some((_, binding)) = BINDING_FLAG_VALUES
        .iter()
        .find(|(flag, _)| key.eq_ignore_ascii_case(flag))
    {
        return !value.eq_ignore_ascii_case(binding);
    }
    if NETWORK_ADDRESS_KEYS
        .iter()
        .any(|network| key.eq_ignore_ascii_case(network))
        && covers_every_address(value)
    {
        return true;
    }
    value.is_empty() || value.contains('*') || value.contains('?')
}

/// The caller-binding keys whose value is a FLAG, with the value that actually binds a
/// caller: `aws:PrincipalIsAWSService` binds only when it demands the caller BE an AWS
/// service, since demanding it not be one leaves every other caller admitted.
const BINDING_FLAG_VALUES: &[(&str, &str)] = &[("aws:PrincipalIsAWSService", "true")];

/// The caller-binding keys whose value is a network address, so a CIDR covering every
/// address binds nobody (IAM's global condition keys: `aws:SourceIp`, `aws:VpcSourceIp`).
const NETWORK_ADDRESS_KEYS: &[&str] = &["aws:SourceIp", "aws:VpcSourceIp"];

/// What separates a network address from its CIDR prefix length, and the prefix length that
/// covers EVERY address of either family (`0.0.0.0/0`, `::/0`, …) — judged by the parsed
/// length rather than by how the address is spelled. A value carrying no prefix length names
/// one address, which binds.
const CIDR_PREFIX_SEPARATOR: char = '/';
const EVERY_ADDRESS_PREFIX_LENGTH: u8 = 0;

/// Whether a network-address condition value covers every address.
fn covers_every_address(value: &str) -> bool {
    value
        .rsplit_once(CIDR_PREFIX_SEPARATOR)
        .and_then(|(_, prefix_length)| prefix_length.trim().parse::<u8>().ok())
        .is_some_and(|prefix_length| prefix_length == EVERY_ADDRESS_PREFIX_LENGTH)
}

/// Whether a statement names the wildcard principal.
fn names_wildcard_principal(statement: &Value) -> bool {
    PRINCIPAL_ELEMENTS
        .iter()
        .filter_map(|element| statement.get(element))
        .any(is_wildcard_principal)
}

/// The name reported for the check that a policy does not open the secret to
/// every principal: "ensure your resource policy does not allow broad access to
/// your secret, for example policies that use a wildcard for the principal"
/// (`ValidateResourcePolicy`, this service's Smithy model).
/// The service reports this check under the name of the request flag that
/// blocks such a policy outright, `BlockPublicPolicy`.
pub const BROAD_ACCESS_CHECK_NAME: &str = "BlockPublicPolicyCheck";

/// What that check reports: the one sentence this service says about a policy
/// granting public access to a secret, the same on the reporting channel as on
/// the refusing one ([`PUBLIC_POLICY_MESSAGE`]), so a caller using this
/// operation as a pre-flight guard reads the very text the refusal will carry.
pub const BROAD_ACCESS_CHECK_MESSAGE: &str = PUBLIC_POLICY_MESSAGE;

/// The name reported for the check that a policy leaves somebody able to reach
/// the secret: "Verifies the policy does not lock out a caller"
/// (`ValidateResourcePolicy`, this service's Smithy model).
pub const LOCKS_OUT_CALLER_CHECK_NAME: &str = "LockOutCheck";

/// What that check reports.
pub const LOCKS_OUT_CALLER_CHECK_MESSAGE: &str =
    "The resource policy locks out callers: a statement denies every principal access to the \
     secret.";

/// Every check a policy document fails, as `(check name, message)` pairs in the
/// order the operation documents its checks.
///
/// The document has already satisfied the syntax check by being parsed by
/// [`parse_resource_policy`]; what is left is the broad-access check and the
/// lock-out check, each a property of the statements the document carries.
pub fn resource_policy_check_failures(policy: &Value) -> Vec<(&'static str, &'static str)> {
    let mut failures = Vec::new();
    if policy_grants_public_access(policy) {
        failures.push((BROAD_ACCESS_CHECK_NAME, BROAD_ACCESS_CHECK_MESSAGE));
    }
    if policy_locks_out_callers(policy) {
        failures.push((LOCKS_OUT_CALLER_CHECK_NAME, LOCKS_OUT_CALLER_CHECK_MESSAGE));
    }
    failures
}

/// What a document naming a principal that does not exist is refused with: like
/// the syntax refusal, one constant sentence that names no part of the document.
pub const VALIDATE_UNSUPPORTED_PRINCIPAL_MESSAGE: &str =
    "This resource policy contains an unsupported principal.";

/// Whether a resource policy document names a principal this service cannot
/// resolve to an identity that exists.
///
/// A RESOURCE-based policy statement names the identities it applies to, and
/// each of them has to be one that EXISTS: a document whose statement names
/// none, or names an identity that cannot exist, is refused rather than reported
/// on. This emulator serves exactly one account
/// (`engine::emulator_account_id`), so an identity belonging to any other
/// account cannot exist here, and neither can an identifier that is neither an
/// account id nor an ARN. The wildcard names every principal instead of one
/// identity, and a principal under any other type (a service, a federated
/// provider, a canonical user) names no account, so neither is resolved here.
pub fn names_unsupported_policy_principal(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            let elements: Vec<&Value> = PRINCIPAL_ELEMENTS
                .iter()
                .filter_map(|element| statement.get(element))
                .collect();
            elements.is_empty()
                || elements
                    .into_iter()
                    .flat_map(account_principals)
                    .any(|principal| !names_this_account(&principal))
        })
}

/// Every identifier of one `Principal` element that names an identity by
/// account: the bare form and the identifiers under the `AWS` type.
fn account_principals(principal: &Value) -> Vec<String> {
    match principal {
        Value::String(value) => vec![value.clone()],
        Value::Array(items) => items.iter().flat_map(account_principals).collect(),
        Value::Object(types) => types
            .get(AWS_PRINCIPAL_TYPE)
            .map(account_principals)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The principal type whose identifiers name an identity inside an account, as
/// an account id or as an ARN.
const AWS_PRINCIPAL_TYPE: &str = "AWS";

/// Whether one `AWS` principal identifier names an identity of the account this
/// service serves. The wildcard names every principal rather than one identity,
/// so it is resolvable whatever the account.
fn names_this_account(principal: &str) -> bool {
    if principal == POLICY_WILDCARD {
        return true;
    }
    principal_account(principal).is_some_and(|account| account == ::engine::emulator_account_id())
}

/// The account an `AWS` principal identifier belongs to: the identifier itself
/// when it is an account id, the account field when it is an ARN, and `None` for
/// an identifier that is neither.
fn principal_account(principal: &str) -> Option<&str> {
    if is_account_id(principal) {
        return Some(principal);
    }
    let account = principal.strip_prefix("arn:")?.split(':').nth(3)?;
    is_account_id(account).then_some(account)
}

/// The number of digits of an Amazon Web Services account id.
const ACCOUNT_ID_LEN: usize = 12;

/// Whether a value is an account id: exactly twelve digits.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether a policy document locks every caller out of the secret: a DENY
/// statement whose `Principal` is the wildcard, carrying no `Condition` that
/// narrows which requests it denies, so the deny reaches every caller including
/// the secret's own account.
///
/// A deny that names concrete principals, or one whose reach a condition
/// narrows, leaves callers outside it able to reach the secret and locks nobody
/// out.
pub fn policy_locks_out_callers(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            statement_effect(statement).as_deref() == Some("deny")
                && statement
                    .get("Principal")
                    .is_some_and(is_wildcard_principal)
                && statement.get("Condition").is_none()
        })
}

/// Whether one `Principal` element is the wildcard: the bare string, or a map
/// from principal type to identifiers one of which is the wildcard.
fn is_wildcard_principal(principal: &Value) -> bool {
    match principal {
        Value::String(value) => value == POLICY_WILDCARD,
        Value::Array(items) => items.iter().any(is_wildcard_principal),
        Value::Object(types) => types.values().any(is_wildcard_principal),
        _ => false,
    }
}

/// The region this request is addressed to, taken from its SigV4 credential
/// scope so an ARN reports the region the caller actually signed for.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Render a stored instant as the wire timestamp type.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// `at` truncated to midnight UTC, the resolution the service reports an access
/// date at: "Note that the resolution of this field is at the date level and
/// does not include the time." (`SecretVersionsListEntry$LastAccessedDate`,
/// this service's Smithy model.)
pub fn start_of_day(at: SystemTime) -> SystemTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    UNIX_EPOCH + Duration::from_secs(since_epoch - since_epoch % SECONDS_PER_DAY)
}

/// The instant a recovery window of `days` started at `from` elapses.
///
/// TOTAL for any value a request can carry. `DeleteSecret` already refuses a
/// window outside the documented 7..30 range with the modelled
/// `InvalidParameterException`, so the clamp here is defence in depth; it keeps
/// the multiplication and the panicking `SystemTime + Duration` out of reach of
/// any caller, whatever a future caller of this function passes.
pub fn after_days(from: SystemTime, days: i64) -> SystemTime {
    let days = days.clamp(MIN_RECOVERY_WINDOW_DAYS, MAX_RECOVERY_WINDOW_DAYS);
    let window = ::std::time::Duration::from_secs(
        u64::try_from(days)
            .unwrap_or(0)
            .saturating_mul(SECONDS_PER_DAY),
    );
    from.checked_add(window).unwrap_or(from)
}

/// A fresh version id. A UUID satisfies `SecretVersionIdType`'s modelled length
/// bound of 32..64 characters.
pub fn new_version_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The six random characters that end a secret's ARN.
pub fn new_arn_suffix() -> String {
    // The alphabet Secrets Manager draws the ARN suffix from: ASCII letters and
    // digits.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    ::uuid::Uuid::new_v4().as_bytes()[..ARN_SUFFIX_LEN]
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// Encode a pagination cursor — the name of the first secret of the NEXT page —
/// as the opaque `NextToken` a client hands back unchanged.
pub fn encode_next_token(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// Read a cursor produced by [`encode_next_token`]. `None` for a token this
/// service did not mint, which the caller answers with
/// `InvalidNextTokenException`.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// A tag as stored: both halves are optional, exactly as the modelled `Tag`
/// shape has them, so a tag round-trips through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl StoredTag {
    /// The stored form of a modelled `Tag`.
    pub fn from_modeled(tag: &Tag) -> Self {
        Self {
            key: tag.key.clone(),
            value: tag.value.clone(),
        }
    }
}

/// One `ExternalSecretRotationMetadata` item as stored: the same optional
/// key/value pair the modelled `ExternalSecretRotationMetadataItem` has, so the
/// metadata a partner specified round-trips through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredKeyValue {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl StoredKeyValue {
    /// The stored form of a modelled metadata item.
    pub fn from_modeled(item: &ExternalSecretRotationMetadataItem) -> Self {
        Self {
            key: item.key.clone(),
            value: item.value.clone(),
        }
    }

    /// The metadata item for the wire.
    pub fn to_modeled(&self) -> ExternalSecretRotationMetadataItem {
        ExternalSecretRotationMetadataItem {
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }
}

/// A secret's rotation schedule as stored: every member of the modelled
/// `RotationRulesType`, so the rules round-trip through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredRotationRules {
    pub automatically_after_days: Option<i64>,
    pub duration: Option<String>,
    pub schedule_expression: Option<String>,
}

impl StoredRotationRules {
    /// The stored form of a modelled `RotationRulesType`.
    pub fn from_modeled(rules: &RotationRulesType) -> Self {
        Self {
            automatically_after_days: rules.automatically_after_days,
            duration: rules.duration.clone(),
            schedule_expression: rules.schedule_expression.clone(),
        }
    }

    /// The rules for the wire.
    pub fn to_modeled(&self) -> RotationRulesType {
        RotationRulesType {
            automatically_after_days: self.automatically_after_days,
            duration: self.duration.clone(),
            schedule_expression: self.schedule_expression.clone(),
        }
    }
}

/// One replica of a secret: the Region it is in, the KMS key it is encrypted
/// with, and the `StatusType` its creation has reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replica {
    pub region: String,
    pub kms_key_id: String,
    /// One of [`REPLICA_STATUS_IN_PROGRESS`], [`REPLICA_STATUS_IN_SYNC`] or
    /// [`REPLICA_STATUS_FAILED`].
    pub status: String,
    /// "Status message such as 'Secret with this name already exists in this
    /// region'" (`ReplicationStatusType$StatusMessage`, this service's Smithy
    /// model): what the status it accompanies says about this replica — why a
    /// `Failed` one could not be created, and that replication succeeded for one
    /// that is `InSync`. A replica whose creation is still `InProgress` has
    /// nothing to report yet.
    pub status_message: Option<String>,
    /// The instant the `InProgress` -> `InSync` move the clock owes this replica
    /// becomes due; `None` once it has been made, and for a replica that failed —
    /// there is nothing left for the clock to owe it. Stored rather than slept on,
    /// so nothing blocks a request.
    pub in_sync_due: Option<SystemTime>,
    /// "The date that the secret was last accessed in the Region"
    /// (`ReplicationStatusType$LastAccessedDate`): stamped by a read of the
    /// replica's own value in its own Region, and absent while nothing has read
    /// it there.
    pub last_accessed_date: Option<SystemTime>,
}

impl Replica {
    /// A replica whose creation has just started in `region`.
    pub(crate) fn creating(region: String, kms_key_id: String, at: SystemTime) -> Self {
        Self {
            region,
            kms_key_id,
            status: REPLICA_STATUS_IN_PROGRESS.to_string(),
            status_message: None,
            in_sync_due: at.checked_add(REPLICA_SYNC_DELAY),
            last_accessed_date: None,
        }
    }

    /// A replica the service could not create, carrying the reason it could not.
    pub(crate) fn failed(region: String, kms_key_id: String, reason: String) -> Self {
        Self {
            region,
            kms_key_id,
            status: REPLICA_STATUS_FAILED.to_string(),
            status_message: Some(reason),
            in_sync_due: None,
            last_accessed_date: None,
        }
    }

    /// Whether this replica exists as a secret in its Region: one whose creation
    /// failed does not, and one still being created does not yet.
    pub fn is_live(&self) -> bool {
        self.status != REPLICA_STATUS_FAILED
    }

    /// Materialise the move to `InSync` once the clock has made it due. Returns
    /// whether anything changed, so a caller can tell a settled replica from an
    /// untouched one.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.in_sync_due.is_none_or(|due| due > now) {
            return false;
        }
        self.status = REPLICA_STATUS_IN_SYNC.to_string();
        self.status_message = Some(REPLICATION_SUCCEEDED_MESSAGE.to_string());
        self.in_sync_due = None;
        true
    }
}

/// How many tags a secret already carrying `existing` would carry once `tags`
/// were attached, which is what [`TAGS_PER_SECRET`] bounds: a key already
/// attached is replaced, not added, so it does not grow the list.
///
/// Linear in both lists: the keys go into a set rather than being rescanned per
/// input tag, so counting a long request list costs no more than reading it.
pub fn tag_count_after_attaching(existing: &[StoredTag], tags: &[Tag]) -> usize {
    let mut keys: BTreeSet<Option<&String>> = existing.iter().map(|tag| tag.key.as_ref()).collect();
    keys.extend(tags.iter().map(|tag| tag.key.as_ref()));
    keys.len()
}

/// How many replicas a secret already holding `existing` would hold once
/// `regions` were replicated to, which is what [`REPLICAS_PER_SECRET`] bounds: a
/// Region already replicated to is replaced, not added.
pub fn replica_count_after_adding(existing: &[Replica], regions: &[&str]) -> usize {
    let mut seen: BTreeSet<&str> = existing
        .iter()
        .map(|replica| replica.region.as_str())
        .collect();
    seen.extend(regions.iter().copied());
    seen.len()
}

/// One version of a secret: its value, and the staging labels pointing at it.
#[derive(Debug, Clone)]
pub struct SecretVersion {
    pub version_id: String,
    /// The version's value, under the key the secret names: sealed under a data
    /// key a customer managed key wrapped, or held as sent for a secret on the
    /// service-managed key. See [`envelope`].
    pub value: envelope::StoredValue,
    pub version_stages: Vec<String>,
    pub created_date: SystemTime,
    /// The date this version's value was last retrieved, at the resolution the
    /// service reports it (see [`Secret::record_access`]); `None` while it has
    /// never been retrieved.
    pub last_accessed_date: Option<SystemTime>,
}

impl SecretVersion {
    /// Whether this version carries `stage`.
    pub fn has_stage(&self, stage: &str) -> bool {
        self.version_stages.iter().any(|held| held == stage)
    }

    /// Whether this version already holds exactly the value in a request — the
    /// test an `@idempotencyToken` replay turns on.
    pub fn holds_same_value(&self, value: &envelope::SecretValue) -> bool {
        self.value.holds(value)
    }

    /// The version's staging labels for the wire, omitted while it carries none
    /// (`SecretVersionStagesType` is modelled with a minimum length of 1).
    pub fn stages_out(&self) -> Option<Vec<String>> {
        (!self.version_stages.is_empty()).then(|| self.stages_in_report_order())
    }

    /// The version's staging labels in the one order the service reports them:
    /// the labels it moves itself first, in the order its own lifecycle names
    /// them ([`STAGE_CURRENT`], then [`STAGE_PENDING`], then
    /// [`STAGE_PREVIOUS`]), and then the labels a caller attached, in the order
    /// they were attached.
    ///
    /// A version can carry several labels at once — "the AWSPENDING staging
    /// label might be attached to the same version as the AWSCURRENT version"
    /// (`RotateSecret`, this service's Smithy model) — and what it reports is
    /// the same list however the labels got there, rather than the order in
    /// which they happened to be moved onto it.
    pub fn stages_in_report_order(&self) -> Vec<String> {
        let mut ordered: Vec<String> = [STAGE_CURRENT, STAGE_PENDING, STAGE_PREVIOUS]
            .into_iter()
            .filter(|stage| self.has_stage(stage))
            .map(str::to_string)
            .collect();
        ordered.extend(
            self.version_stages
                .iter()
                .filter(|stage| !is_reserved_stage(stage))
                .cloned(),
        );
        ordered
    }

    /// How many of the staging labels on this version a CALLER named: the ones
    /// the service moves itself are left out, as [`is_reserved_stage`] decides.
    /// This is the count [`STAGES_PER_VERSION`] bounds.
    pub fn custom_stage_count(&self) -> usize {
        self.version_stages
            .iter()
            .filter(|stage| !is_reserved_stage(stage))
            .count()
    }
}

/// A stored secret: its metadata and every version it still holds.
#[derive(Debug, Clone)]
pub struct Secret {
    pub name: String,
    pub arn: String,
    pub region: String,
    pub description: Option<String>,
    pub kms_key_id: Option<String>,
    /// The `Type` of an externally-held secret; `None` for a secret this
    /// service stores itself.
    pub secret_type: Option<String>,
    /// The service that created the secret, for a service-linked secret;
    /// `None` for one a caller created.
    pub owning_service: Option<String>,
    pub tags: Vec<StoredTag>,
    pub created_date: SystemTime,
    pub last_changed_date: SystemTime,
    pub last_accessed_date: Option<SystemTime>,
    /// The instant the deletion of this secret was REQUESTED; `Some(..)` means
    /// it is scheduled for deletion. This is the `DeletedDate` the service
    /// reports: "The date and time the deletion of the secret occurred. Not
    /// present on active secrets. The secret can be recovered until the number
    /// of days in the recovery window has passed"
    /// (`SecretListEntry$DeletedDate`, this service's Smithy model), so the
    /// recovery window runs FROM this instant rather than ending at it.
    pub deleted_date: Option<SystemTime>,
    /// The instant the recovery window of the scheduled deletion elapses: "the
    /// date and time after which this secret Secrets Manager can permanently
    /// delete this secret, and it can no longer be restored"
    /// (`DeleteSecretResponse$DeletionDate`, this service's Smithy model). It is
    /// the deletion request plus the window for a windowed deletion, and the
    /// request instant itself for a `ForceDeleteWithoutRecovery` one, which
    /// skips the window.
    pub deletion_date: Option<SystemTime>,
    /// The instant this store drops the secret for good, which [`expire_due`]
    /// materialises. It is the end of the recovery window for a windowed
    /// deletion, and a short span after the call for a
    /// `ForceDeleteWithoutRecovery` one: "Secrets Manager performs the actual
    /// deletion with an asynchronous background process, so there might be a
    /// short delay before the secret is permanently deleted."
    /// (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's Smithy
    /// model.) Until it falls due the secret still exists, marked for deletion.
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub purge_at: Option<SystemTime>,
    pub rotation_enabled: bool,
    /// The ARN of the Lambda rotation function `RotateSecret` configured for
    /// this secret; `None` for a secret no rotation has been configured for.
    pub rotation_lambda_arn: Option<String>,
    /// The rotation schedule `RotateSecret` configured.
    pub rotation_rules: Option<StoredRotationRules>,
    /// "The metadata needed to successfully rotate a managed external secret",
    /// as the partner specified it.
    pub external_rotation_metadata: Vec<StoredKeyValue>,
    /// The role that lets Secrets Manager rotate a secret a third-party partner
    /// holds.
    pub external_rotation_role_arn: Option<String>,
    /// The instant the last rotation of this secret COMPLETED; `None` while no
    /// rotation has ever completed, which is what a started rotation that has
    /// not finished leaves it at.
    pub last_rotated_date: Option<SystemTime>,
    /// The instant the rotation schedule next makes a rotation due.
    pub next_rotation_date: Option<SystemTime>,
    /// The version id a started rotation has staked `AWSPENDING` on, while the
    /// rotation function has not created that version yet: "During rotation,
    /// Secrets Manager creates an AWSPENDING version ID before creating the new
    /// secret version" (`DescribeSecretResponse$VersionIdsToStages`, this
    /// service's Smithy model).
    pub pending_rotation_version_id: Option<String>,
    /// The replicas of this secret, in the order their Regions were first
    /// replicated to. Empty for a secret that does not participate in
    /// cross-region replication.
    pub replicas: Vec<Replica>,
    /// The Region holding the PRIMARY this secret is a replica of; `None` for a
    /// primary or a standalone secret.
    ///
    /// A replica is "a separate secret in the Region" it was replicated to
    /// (`ReplicateSecretToRegions`, this service's Smithy model): it carries its
    /// own region-qualified ARN and is read in its own Region, and while it
    /// tracks the primary its content is the primary's — so every write goes to
    /// the primary, and `StopReplicationToReplica` is what makes it a secret of
    /// its own.
    pub replica_of: Option<String>,
    /// The ARN this secret's stored version values were sealed under, when it is
    /// not this secret's own: the values of a replica, and of a secret promoted
    /// from one, were sealed for the primary's ARN, which is the encryption
    /// context they can only be opened under (see [`envelope::ValueContext`]).
    pub sealed_under: Option<String>,
    /// The resource-based permission policy document attached to this secret, in
    /// the layout this service reports a policy in (see
    /// [`attached_resource_policy`]); `None` for a secret no policy is attached
    /// to. `GetResourcePolicy` "retrieves the JSON text of the resource-based
    /// policy document attached to the secret" (this service's Smithy model), and
    /// the JSON text it retrieves is the rendering of the document this service
    /// parsed rather than the caller's bytes.
    pub resource_policy: Option<String>,
    /// Versions in the order they were created.
    pub versions: Vec<SecretVersion>,
}

impl Secret {
    /// A new secret with no versions, owned by `region`.
    pub fn new(name: String, arn: String, region: String, at: SystemTime) -> Self {
        Self {
            name,
            arn,
            region,
            description: None,
            kms_key_id: None,
            secret_type: None,
            owning_service: None,
            tags: Vec::new(),
            created_date: at,
            last_changed_date: at,
            last_accessed_date: None,
            deleted_date: None,
            deletion_date: None,
            purge_at: None,
            rotation_enabled: false,
            rotation_lambda_arn: None,
            rotation_rules: None,
            external_rotation_metadata: Vec::new(),
            external_rotation_role_arn: None,
            last_rotated_date: None,
            next_rotation_date: None,
            pending_rotation_version_id: None,
            replicas: Vec::new(),
            replica_of: None,
            sealed_under: None,
            resource_policy: None,
            versions: Vec::new(),
        }
    }

    /// Whether a `DeleteSecret` call has scheduled this secret for deletion.
    pub fn scheduled_for_deletion(&self) -> bool {
        self.deleted_date.is_some()
    }

    /// Mark this secret for deletion: `requested_at` is the `DeletedDate` the
    /// service reports — the instant the deletion was asked for — `deletion_date`
    /// the end of the recovery window it reports as `DeletionDate`, and
    /// `purge_at` the instant the store owes its permanent removal.
    ///
    /// Scheduling a deletion also turns ROTATION off: a secret on its way out is
    /// not rotated, and `RotationEnabled` is "whether automatic rotation is turned
    /// on for this secret" (`DescribeSecretResponse$RotationEnabled`, this
    /// service's Smithy model), which it no longer is. The rotation
    /// CONFIGURATION is kept, so a restored secret can be put back on a schedule
    /// by `RotateSecret` — "to turn on automatic rotation again, call
    /// RotateSecret" — but nothing turns it back on by itself, which is why
    /// [`cancel_deletion`] leaves it off.
    ///
    /// Re-scheduling a secret that is already marked replaces all three, so a
    /// second deletion request measures its window from itself.
    ///
    /// [`cancel_deletion`]: Secret::cancel_deletion
    pub fn schedule_deletion(
        &mut self,
        requested_at: SystemTime,
        deletion_date: SystemTime,
        purge_at: SystemTime,
    ) {
        self.deleted_date = Some(requested_at);
        self.deletion_date = Some(deletion_date);
        self.purge_at = Some(purge_at);
        self.rotation_enabled = false;
        self.next_rotation_date = None;
    }

    /// Whether a scheduled deletion of this secret can still be cancelled.
    ///
    /// `DeletionDate` is "the date and time after which this secret Secrets
    /// Manager can permanently delete this secret, and it can no longer be
    /// restored" (`DeleteSecretResponse$DeletionDate`, this service's Smithy
    /// model), so a deletion is cancellable exactly while that instant is still
    /// ahead. A deletion that skipped the recovery window stamps that instant at
    /// the call itself, so there is no span in which it can be cancelled.
    pub fn deletion_cancellable(&self, now: SystemTime) -> bool {
        self.deletion_date.is_some_and(|at| at > now)
    }

    /// Cancel a scheduled deletion: drop the `DeletionDate` stamp and with it
    /// the permanent removal the store owed, leaving the secret's versions,
    /// staging labels and every other attribute as they were.
    ///
    /// Rotation stays OFF: scheduling the deletion turned it off (see
    /// [`schedule_deletion`]), and `RestoreSecret` "cancels the scheduled
    /// deletion" (this service's Smithy model) — nothing more, so a caller that
    /// wants the secret rotated again asks for that itself.
    ///
    /// [`schedule_deletion`]: Secret::schedule_deletion
    pub fn cancel_deletion(&mut self, at: SystemTime) {
        self.deleted_date = None;
        self.deletion_date = None;
        self.purge_at = None;
        self.last_changed_date = at;
    }

    /// The version with this id, deprecated ones included.
    pub fn version(&self, version_id: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.version_id == version_id)
    }

    /// The keys every version of this secret is encrypted under: the key the
    /// secret's configuration NAMES, exactly as the secret holds it — the value
    /// `DescribeSecret` reports as its `KmsKeyId`, in whichever of the forms
    /// `KmsKeyId` accepts it was given ("the ARN, key ID, or alias of the KMS
    /// key", `CreateSecretRequest$KmsKeyId`, this service's Smithy model) — and
    /// [`DEFAULT_ENCRYPTION_KEY_ID`] only for a secret whose configuration names
    /// NO key, which is the one case the service has no identifier of the
    /// caller's to report. This is `SecretVersionsListEntry$KmsKeyIds`, "the KMS
    /// keys used to encrypt the secret version", and it is never empty: every
    /// version is encrypted under some key.
    ///
    /// So a secret pointed at the Amazon Web Services managed key BY NAME reports
    /// that name, and only one that was never given a key at all reports the
    /// token — which is what lets a caller tell the two apart after an update
    /// that re-keys a secret onto the managed key.
    pub fn encryption_key_ids(&self) -> Vec<String> {
        match self.kms_key_id.as_deref().filter(|key| !key.is_empty()) {
            Some(key) => vec![key.to_string()],
            None => vec![DEFAULT_ENCRYPTION_KEY_ID.to_string()],
        }
    }

    /// Record that the value of the version `version_id` was retrieved at `at`.
    ///
    /// `LastAccessedDate` is "the date that the secret was last accessed in the
    /// Region. This field is omitted if the secret has never been retrieved in
    /// the Region." (`DescribeSecretResponse$LastAccessedDate`, this service's
    /// Smithy model), so it is retrieving a value that sets it — on the version
    /// whose value was read and on the secret that holds it. It is stored at
    /// the resolution the service reports it at (see [`start_of_day`]), so
    /// repeated retrievals on one day leave it where it is.
    ///
    /// Nothing else about the secret moves: a retrieval changes no value, no
    /// staging label and no attribute, so `LastChangedDate` stays put.
    pub fn record_access(&mut self, version_id: &str, at: SystemTime) {
        let date = start_of_day(at);
        if let Some(version) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == version_id)
        {
            version.last_accessed_date = Some(date);
        }
        self.last_accessed_date = Some(date);
    }

    /// The one version carrying `stage`.
    pub fn version_with_stage(&self, stage: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.has_stage(stage))
    }

    /// The NAMES of the fields this secret's CURRENT value states
    /// ([`envelope::StoredValue::stated_field_names`]), which is the value a retrieval
    /// that names no version gets — "AWSCURRENT, which Secrets Manager creates for the
    /// latest version" — and so the one a service pointed at this secret reads.
    ///
    /// A secret holding no current version at all states no fields, which is an answer: a
    /// secret can be created without a value ("SecretString … Either SecretString or
    /// SecretBinary must have a value, but not both" is stated of a value-carrying
    /// request, and `CreateSecret` accepts neither member), and such a secret stores no
    /// field. `None` means the fields cannot be read here — a value sealed under a
    /// customer managed key — so a caller reports nothing about them.
    ///
    /// Only names: what the fields HOLD stays in this store (see
    /// [`envelope::StoredValue::stated_field_names`]).
    pub fn stated_field_names(&self) -> Option<Vec<String>> {
        match self.version_with_stage(STAGE_CURRENT) {
            Some(version) => version.value.stated_field_names(),
            None => Some(Vec::new()),
        }
    }

    /// The stored value of every version that carries one of the staging labels
    /// the service itself moves, by version id.
    ///
    /// These are the versions a change of the secret's key moves to that key:
    /// `KmsKeyId` is "the KMS key that Secrets Manager uses to encrypt new
    /// secret versions as well as any existing versions with the staging labels
    /// AWSCURRENT, AWSPENDING, or AWSPREVIOUS" (`UpdateSecretRequest$KmsKeyId`,
    /// this service's Smithy model) — a deprecated version carries none of them
    /// and keeps the key it was sealed under.
    pub fn staged_version_values(&self) -> Vec<(String, envelope::StoredValue)> {
        self.versions
            .iter()
            .filter(|version| {
                version
                    .version_stages
                    .iter()
                    .any(|held| is_reserved_stage(held))
            })
            .map(|version| (version.version_id.clone(), version.value.clone()))
            .collect()
    }

    /// Replace the stored value of the version `version_id` with `value`, the
    /// same plaintext sealed under the key the secret is now on.
    ///
    /// Only a re-encryption reaches here: a version's VALUE is never otherwise
    /// modified ("You can't modify an existing version, you can only create a
    /// new version", `UpdateSecret`, this service's Smithy model), so what this
    /// changes is the key the bytes are held under and nothing a caller reads.
    /// A version that has since gone is left alone.
    pub fn reseal_version(&mut self, version_id: &str, value: envelope::StoredValue) {
        if let Some(version) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == version_id)
        {
            version.value = value;
        }
    }

    /// Every version that still carries a staging label, mapped to its labels.
    /// A version with no label is deprecated and is left out, which is what
    /// makes this map the live staging state rather than a version history.
    ///
    /// A rotation that has staked `AWSPENDING` on a version its rotation
    /// function has not created yet is in the map too, under the id it staked:
    /// "During rotation, Secrets Manager creates an AWSPENDING version ID
    /// before creating the new secret version. To check if a secret version
    /// exists, call GetSecretValue"
    /// (`DescribeSecretResponse$VersionIdsToStages`, this service's Smithy
    /// model) — so the label is reported before the version it will name is
    /// readable.
    pub fn versions_to_stages(&self) -> Option<HashMap<String, Vec<String>>> {
        let mut map: HashMap<String, Vec<String>> = self
            .versions
            .iter()
            .filter(|v| !v.version_stages.is_empty())
            .map(|v| (v.version_id.clone(), v.stages_in_report_order()))
            .collect();
        if let Some(reserved) = self.reserved_pending_version_id() {
            map.insert(reserved.to_string(), vec![STAGE_PENDING.to_string()]);
        }
        (!map.is_empty()).then_some(map)
    }

    /// The version id a started rotation staked `AWSPENDING` on and whose
    /// version does not exist yet; `None` once the rotation function has created
    /// it, which is when the label lives on the version itself.
    pub fn reserved_pending_version_id(&self) -> Option<&str> {
        self.pending_rotation_version_id
            .as_deref()
            .filter(|id| self.version(id).is_none())
    }

    /// The version id `AWSPENDING` names: the version carrying the label, or the
    /// id a started rotation reserved for the version its rotation function has
    /// not created yet.
    pub fn pending_version_id(&self) -> Option<&str> {
        self.version_with_stage(STAGE_PENDING)
            .map(|version| version.version_id.as_str())
            .or_else(|| self.reserved_pending_version_id())
    }

    /// Whether a rotation of this secret is still in progress.
    ///
    /// "When rotation is successful, the AWSPENDING staging label might be
    /// attached to the same version as the AWSCURRENT version, or it might not
    /// be attached to any version. If the AWSPENDING staging label is present
    /// but not attached to the same version as AWSCURRENT, then any later
    /// invocation of RotateSecret assumes that a previous rotation request is
    /// still in progress" (`RotateSecret`, this service's Smithy model).
    pub fn rotation_in_progress(&self) -> bool {
        match self.pending_version_id() {
            Some(pending) => {
                self.version_with_stage(STAGE_CURRENT)
                    .map(|current| current.version_id.as_str())
                    != Some(pending)
            }
            None => false,
        }
    }

    /// The schedule the stored rules rotate the secret on, for computing the
    /// next due instant. Rules are validated before they are stored, so rules
    /// that no longer parse, and rules naming no schedule at all, name none.
    pub fn rotation_schedule(&self) -> Option<RotationSchedule> {
        parse_rotation_rules(&self.rotation_rules.as_ref()?.to_modeled())
            .ok()
            .flatten()
    }

    /// Stamp the instant the stored schedule next makes a rotation due, measured
    /// from `anchor` — the rotation (or the configuration of the schedule) the
    /// next one follows.
    ///
    /// "In DescribeSecret and ListSecrets, this value is calculated from the
    /// rotation schedule after every successful rotation"
    /// (`RotationRulesType$AutomaticallyAfterDays`, this service's Smithy
    /// model), so every event that re-anchors the schedule comes through here
    /// rather than each computing an instant of its own.
    ///
    /// Nothing is due while rotation is off: "If the secret isn't configured for
    /// rotation or rotation has been disabled, Secrets Manager returns null"
    /// (`DescribeSecretResponse$NextRotationDate`).
    fn reschedule_next_rotation(&mut self, anchor: SystemTime) {
        self.next_rotation_date = self
            .rotation_enabled
            .then(|| self.rotation_schedule())
            .flatten()
            .and_then(|schedule| schedule.next_after(anchor));
    }

    /// The stored rotation rules for the wire.
    pub fn rotation_rules_out(&self) -> Option<RotationRulesType> {
        self.rotation_rules
            .as_ref()
            .map(StoredRotationRules::to_modeled)
    }

    /// The stored external rotation metadata for the wire, omitted while the
    /// secret carries none.
    pub fn external_rotation_metadata_out(
        &self,
    ) -> Option<Vec<ExternalSecretRotationMetadataItem>> {
        (!self.external_rotation_metadata.is_empty()).then(|| {
            self.external_rotation_metadata
                .iter()
                .map(StoredKeyValue::to_modeled)
                .collect()
        })
    }

    /// The secret's tags for the wire, omitted while it carries none.
    pub fn tags_out(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|tag| Tag {
                    key: tag.key.clone(),
                    value: tag.value.clone(),
                })
                .collect()
        })
    }

    /// Attach `tags` to the secret, appending the keys it does not carry yet and
    /// replacing the value of each key it does.
    ///
    /// `TagResource` "appends tags to the existing list of tags", and a tag's
    /// key is its identity ("The key identifier, or name, of the tag",
    /// `Tag$Key`), so a key that is already attached is updated in place rather
    /// than attached twice. Tagging modifies the secret's metadata, which is
    /// what `LastChangedDate` reports ("The last date and time that this secret
    /// was modified in any way").
    pub fn attach_tags(&mut self, tags: &[Tag], at: SystemTime) {
        for tag in tags {
            let stored = StoredTag::from_modeled(tag);
            match self
                .tags
                .iter_mut()
                .find(|existing| existing.key == stored.key)
            {
                Some(existing) => existing.value = stored.value,
                None => self.tags.push(stored),
            }
        }
        self.last_changed_date = at;
    }

    /// Detach every tag whose key is named, leaving the rest in place.
    ///
    /// `UntagResource` "is idempotent. If a requested tag is not attached to the
    /// secret, no error is returned and the secret metadata is unchanged", so a
    /// key that matches nothing is not an error — and when nothing matches,
    /// `LastChangedDate` does not move either.
    pub fn detach_tags(&mut self, keys: &[String], at: SystemTime) {
        let before = self.tags.len();
        self.tags
            .retain(|tag| tag.key.as_ref().is_none_or(|key| !keys.contains(key)));
        if self.tags.len() != before {
            self.last_changed_date = at;
        }
    }

    /// How many tags this secret would carry once `tags` were attached, which is
    /// what [`TAGS_PER_SECRET`] bounds.
    pub fn tag_count_after(&self, tags: &[Tag]) -> usize {
        tag_count_after_attaching(&self.tags, tags)
    }

    /// Attach `policy` to the secret, replacing whatever policy it carried.
    ///
    /// `PutResourcePolicy` "attaches a resource-based permission policy to a
    /// secret" and one secret carries at most one such policy, so a second call
    /// replaces the document rather than adding to it. Attaching one modifies
    /// the secret, which is what `LastChangedDate` reports ("The last date and
    /// time that this secret was modified in any way", this service's Smithy
    /// model).
    pub fn attach_resource_policy(&mut self, policy: String, at: SystemTime) {
        self.resource_policy = Some(policy);
        self.last_changed_date = at;
    }

    /// Detach the resource policy, reporting whether the secret carried one.
    ///
    /// `DeleteResourcePolicy` "deletes the resource-based permission policy
    /// attached to the secret" (this service's Smithy model) and models no
    /// error for a secret that carries none, so detaching nothing succeeds —
    /// and, having changed nothing, leaves `LastChangedDate` where it was.
    pub fn detach_resource_policy(&mut self, at: SystemTime) -> bool {
        let had = self.resource_policy.take().is_some();
        if had {
            self.last_changed_date = at;
        }
        had
    }

    /// The replica held for `region`, if any.
    pub fn replica(&self, region: &str) -> Option<&Replica> {
        self.replicas.iter().find(|replica| replica.region == region)
    }

    /// How many replicas this secret would hold once `regions` were replicated
    /// to, which is what [`REPLICAS_PER_SECRET`] bounds.
    pub fn replica_count_after(&self, regions: &[&str]) -> usize {
        replica_count_after_adding(&self.replicas, regions)
    }

    /// Record a replication attempt into `region`.
    ///
    /// A replica the service could create starts `InProgress`, which the clock
    /// moves to `InSync` (see [`settle_replicas`]); one it could not — `reason`
    /// states why — is recorded `Failed` carrying that reason, which is what a
    /// client reads to decide whether the replica is usable. A Region that
    /// already holds a replica OF THIS SECRET is replicated again, which starts
    /// its creation over rather than adding a second entry for the same Region.
    ///
    /// [`settle_replicas`]: Secret::settle_replicas
    pub fn replicate_to(
        &mut self,
        region: &str,
        kms_key_id: Option<&str>,
        reason: Option<String>,
        at: SystemTime,
    ) {
        let key = kms_key_id
            .filter(|key| !key.is_empty())
            .unwrap_or(DEFAULT_KMS_KEY_ALIAS)
            .to_string();
        let fresh = match reason {
            Some(reason) => Replica::failed(region.to_string(), key, reason),
            None => Replica::creating(region.to_string(), key, at),
        };
        match self
            .replicas
            .iter_mut()
            .find(|replica| replica.region == region)
        {
            Some(existing) => *existing = fresh,
            None => self.replicas.push(fresh),
        }
    }

    /// Stop replicating to `region`, reporting whether a replica was held there.
    pub fn remove_replica(&mut self, region: &str) -> bool {
        let before = self.replicas.len();
        self.replicas.retain(|replica| replica.region != region);
        self.replicas.len() != before
    }

    /// Materialise every replica transition the clock has already made due,
    /// reporting whether any moved. This is the move from `InProgress` to
    /// `InSync` that was owed at this instant whether or not anybody called, so
    /// a reporting operation may perform it before it answers.
    pub fn settle_replicas(&mut self, now: SystemTime) -> bool {
        let mut settled = false;
        for replica in &mut self.replicas {
            settled |= replica.settle(now);
        }
        settled
    }

    /// The secret's replication view for the wire — one `ReplicationStatusType`
    /// per replica — omitted while the secret has no replicas.
    pub fn replication_status_out(&self) -> Option<Vec<ReplicationStatusType>> {
        (!self.replicas.is_empty()).then(|| self.replication_status())
    }

    /// The same view, as a list that is EMPTY rather than absent for a secret
    /// with no replicas — what `RemoveRegionsFromReplication` answers with,
    /// since its `ReplicationStatus` is "the status of replicas for this secret
    /// after you remove Regions" and "none left" is that status.
    pub fn replication_status(&self) -> Vec<ReplicationStatusType> {
        self.replicas
            .iter()
            .map(|replica| ReplicationStatusType {
                region: Some(replica.region.clone()),
                kms_key_id: Some(replica.kms_key_id.clone()),
                status: Some(replica.status.clone()),
                status_message: replica.status_message.clone(),
                last_accessed_date: replica.last_accessed_date.map(timestamp),
            })
            .collect()
    }

    /// Whether this secret is a replica of a primary in another Region.
    pub fn is_replica(&self) -> bool {
        self.replica_of.is_some()
    }

    /// The ARN this secret's version values were sealed under, which is the
    /// encryption context they are opened under: its own, unless they were
    /// mirrored from a primary (see [`Secret::sealed_under`]).
    pub fn value_context_arn(&self) -> &str {
        self.sealed_under.as_deref().unwrap_or(&self.arn)
    }

    /// The secret's `PrimaryRegion` for the wire.
    ///
    /// `PrimaryRegion` is part of the replication view — "The Region the secret
    /// is in. If a secret is replicated to other Regions, the replicas are
    /// listed in ReplicationStatus" — and Secrets Manager "only returns fields
    /// that have a value in the response", so a secret that takes no part in
    /// replication has no primary Region to report and the member is omitted. A
    /// replica reports the Region its primary is in; a primary reports its own,
    /// from the moment it has a replica until the last one is removed.
    pub fn primary_region_out(&self) -> Option<String> {
        self.replica_of
            .clone()
            .or_else(|| (!self.replicas.is_empty()).then(|| self.region.clone()))
    }

    /// Whether another version fits under [`VERSIONS_PER_SECRET`], given the
    /// staging labels the new version would claim.
    ///
    /// Below the bound there is always room. At it, a version every one of whose
    /// labels this put takes away is left deprecated, and [`put_version`] evicts
    /// it to make room — except the version `AWSCURRENT` is taken from, which
    /// [`put_version`] relabels `AWSPREVIOUS` instead of deprecating, so it
    /// frees nothing. With no version to free — a caller naming a fresh label
    /// every time — the caller is refused with the modelled quota error rather
    /// than growing the store.
    ///
    /// A put that cancels a pending version ([`cancels_pending_version`]) takes
    /// `AWSPENDING` away too, so it frees a version that carried only that
    /// label: the two have to agree about what the put takes away, or a write
    /// that does make room is refused by the quota.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn has_room_for_version(&self, claimed: &[String]) -> bool {
        if self.versions.len() < VERSIONS_PER_SECRET {
            return true;
        }
        let demotes = claimed.iter().any(|stage| stage == STAGE_CURRENT)
            && !claimed.iter().any(|stage| stage == STAGE_PREVIOUS);
        let cancels_pending = cancels_pending_version(claimed);
        self.versions.iter().any(|existing| {
            let freed = existing.version_stages.iter().all(|held| {
                claimed.contains(held) || (cancels_pending && held == STAGE_PENDING)
            });
            freed && !(demotes && existing.has_stage(STAGE_CURRENT))
        })
    }

    /// Add `version` and move the staging labels it claims onto it.
    ///
    /// A label belongs to one version at a time, so every label the new version
    /// claims is taken off whichever version held it; and when the new version
    /// claims `AWSCURRENT`, `AWSPREVIOUS` moves to the version `AWSCURRENT` was
    /// taken from — unless the caller placed `AWSPREVIOUS` on the new version
    /// itself, which would otherwise leave the label on two versions. A version
    /// left with no label stays stored (it is still addressable by `VersionId`)
    /// but is deprecated, and is the first thing evicted once the secret holds
    /// more than [`VERSIONS_PER_SECRET`] versions.
    ///
    /// A new version that takes `AWSCURRENT` without re-staging `AWSPENDING`
    /// also takes `AWSPENDING` off whichever version held it, and releases a
    /// rotation's reservation of it: see [`cancels_pending_version`].
    pub fn put_version(&mut self, version: SecretVersion, at: SystemTime) {
        let claimed = version.version_stages.clone();
        // Minting the current value from somewhere other than the pending
        // candidate cancels that candidate, so the version it sat on is left
        // carrying no staging label — its value stays addressable by
        // `VersionId` like any deprecated version's.
        if cancels_pending_version(&claimed) {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PENDING);
            }
            self.pending_rotation_version_id = None;
        }
        // A rotation reserves the id of the version its rotation function will
        // create and stakes `AWSPENDING` on it, so a version arriving under that
        // id — or any version claiming that label — is that label moving onto a
        // version that now exists: the reservation has done its job, and
        // dropping it is what keeps `AWSPENDING` on one version at a time.
        if self.pending_rotation_version_id.as_deref() == Some(version.version_id.as_str())
            || claimed.iter().any(|stage| stage == STAGE_PENDING)
        {
            self.pending_rotation_version_id = None;
        }
        let demoted = if claimed.iter().any(|stage| stage == STAGE_CURRENT) {
            self.version_with_stage(STAGE_CURRENT)
                .map(|current| current.version_id.clone())
        } else {
            None
        };
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| !claimed.contains(held));
        }
        if let Some(demoted) = demoted.filter(|_| !claimed.iter().any(|s| s == STAGE_PREVIOUS)) {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PREVIOUS);
            }
            if let Some(existing) = self
                .versions
                .iter_mut()
                .find(|v| v.version_id == demoted)
            {
                existing.version_stages.push(STAGE_PREVIOUS.to_string());
            }
        }
        self.versions.push(version);
        self.last_changed_date = at;
        // Keep the secret bounded: evict the oldest deprecated versions, the
        // ones no staging label reaches, until the count is back under the
        // bound. A labelled version is never evicted, which is why a put that
        // frees none is refused by `has_room_for_version` before it gets here.
        while self.versions.len() > VERSIONS_PER_SECRET {
            let oldest_deprecated = self
                .versions
                .iter()
                .position(|existing| existing.version_stages.is_empty());
            match oldest_deprecated {
                Some(index) => {
                    self.versions.remove(index);
                }
                None => break,
            }
        }
        // "Manually updating the secret value by calling PutSecretValue or
        // UpdateSecret is considered a valid rotation"
        // (`RotationRulesType$AutomaticallyAfterDays`, this service's Smithy
        // model): a version that takes `AWSCURRENT` is the secret's value having
        // been replaced, so the schedule is measured from THAT rather than from
        // whenever rotation was configured. A version staged under any other
        // label is not the current value and re-anchors nothing.
        if claimed.iter().any(|stage| stage == STAGE_CURRENT) {
            self.reschedule_next_rotation(at);
        }
    }

    /// The version id `stage` currently names: the version carrying the label,
    /// and for `AWSPENDING` the id a started rotation reserved for the version
    /// its rotation function has not created yet — the reservation IS the label's
    /// current holder until that version arrives.
    pub fn stage_holder(&self, stage: &str) -> Option<&str> {
        if stage == STAGE_PENDING {
            return self.pending_version_id();
        }
        self.version_with_stage(stage)
            .map(|version| version.version_id.as_str())
    }

    /// Attach `stage` to the version `move_to` already holds.
    ///
    /// "Each staging label can be attached to only one version at a time. To add
    /// a staging label to a version when it is already attached to another
    /// version, Secrets Manager first removes it from the other version first
    /// and then attaches it to this one", and "Whenever you move AWSCURRENT,
    /// Secrets Manager automatically moves the label AWSPREVIOUS to the version
    /// that AWSCURRENT was removed from" (`UpdateSecretVersionStage`, this
    /// service's Smithy model) — so the label ends up on exactly one version,
    /// and a move of the current label leaves the previous one behind it.
    ///
    /// A move of `AWSPENDING` onto a version that exists also drops a rotation's
    /// reservation of that label, for the reason [`put_version`] drops it: the
    /// reservation stands only while the label names a version the store does
    /// not hold.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn attach_stage(&mut self, stage: &str, move_to: &str, at: SystemTime) {
        let demoted = if stage == STAGE_CURRENT {
            self.version_with_stage(STAGE_CURRENT)
                .map(|current| current.version_id.clone())
                .filter(|current| current != move_to)
        } else {
            None
        };
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| held != stage);
        }
        if let Some(demoted) = demoted {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PREVIOUS);
            }
            if let Some(existing) = self
                .versions
                .iter_mut()
                .find(|version| version.version_id == demoted)
            {
                existing.version_stages.push(STAGE_PREVIOUS.to_string());
            }
        }
        if let Some(existing) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == move_to)
        {
            existing.version_stages.push(stage.to_string());
        }
        if stage == STAGE_PENDING {
            self.pending_rotation_version_id = None;
        }
        self.last_changed_date = at;
    }

    /// Take `stage` off the version `remove_from`.
    ///
    /// "If this action results in the last label being removed from a version,
    /// then the version is considered to be 'deprecated' and can be deleted by
    /// Secrets Manager" (`UpdateSecretVersionStage`, this service's Smithy
    /// model), so the version stays stored and simply carries no label.
    ///
    /// Removing `AWSPENDING` from the id a cancelled rotation reserved releases
    /// that reservation, which is the clean-up the model sends a caller to this
    /// operation for.
    pub fn detach_stage(&mut self, stage: &str, remove_from: &str, at: SystemTime) {
        if let Some(existing) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == remove_from)
        {
            existing.version_stages.retain(|held| held != stage);
        }
        if stage == STAGE_PENDING
            && self.pending_rotation_version_id.as_deref() == Some(remove_from)
        {
            self.pending_rotation_version_id = None;
        }
        self.last_changed_date = at;
    }

    /// Apply the rotation configuration a `RotateSecret` call carries.
    ///
    /// A member the call does not name keeps the value already stored: "If you
    /// include the configuration parameters, the operation sets the values for
    /// the secret and then immediately starts a rotation. If you don't include
    /// the configuration parameters, the operation starts a rotation with the
    /// values already stored in the secret" (`RotateSecret`, this service's
    /// Smithy model). Configuring rotation is what enables it, and the next due
    /// instant is recomputed from whichever schedule the secret now holds.
    pub fn configure_rotation(&mut self, config: RotationConfig, at: SystemTime) {
        if let Some(lambda_arn) = config.lambda_arn {
            self.rotation_lambda_arn = Some(lambda_arn);
        }
        if let Some(rules) = config.rules {
            self.rotation_rules = Some(rules);
        }
        if let Some(metadata) = config.metadata {
            self.external_rotation_metadata = metadata;
        }
        if let Some(role_arn) = config.role_arn {
            self.external_rotation_role_arn = Some(role_arn);
        }
        self.rotation_enabled = true;
        self.reschedule_next_rotation(at);
        self.last_changed_date = at;
    }

    /// Stake `AWSPENDING` on `version_id`, the id the rotation that is starting
    /// will create its new version of the secret under.
    ///
    /// A staging label names at most one version of a secret at a time, so the
    /// label is taken off whichever version holds it — which is how a rotation
    /// that starts where the previous one left `AWSPENDING` on the `AWSCURRENT`
    /// version (the documented "rotation succeeded" shape) still leaves the
    /// label naming one version.
    pub fn start_rotation(&mut self, version_id: String, at: SystemTime) {
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| held != STAGE_PENDING);
        }
        self.pending_rotation_version_id = Some(version_id);
        self.last_changed_date = at;
    }

    /// Record that the rotation that was in flight COMPLETED, at `at`.
    ///
    /// "The last date and time that Secrets Manager rotated the secret. If the secret isn't
    /// configured for rotation or was never rotated, Secrets Manager returns null."
    /// (`DescribeSecretResponse$LastRotatedDate`, this service's Smithy model) — so the
    /// instant the rotation finished is what this stamps, and the schedule's next due
    /// instant is measured from it: `NextRotationDate` is "the next rotation is scheduled to
    /// occur on or before this date", which for a schedule stated as an interval is that
    /// interval after the rotation that just happened.
    ///
    /// Only the stamps move. Which version holds which staging label is the rotation
    /// FUNCTION's own work, written through this service's operations while the rotation
    /// ran, so nothing here moves a label or mints a version.
    pub fn finish_rotation(&mut self, at: SystemTime) {
        self.last_rotated_date = Some(at);
        self.reschedule_next_rotation(at);
        self.last_changed_date = at;
    }

    /// Turn automatic rotation off and cancel a rotation that is still in
    /// progress, reporting the version id that rotation staked `AWSPENDING` on
    /// (`None` when no rotation was in progress).
    ///
    /// "Turns off automatic rotation, and if a rotation is currently in
    /// progress, cancels the rotation" (`CancelRotateSecret`, this service's
    /// Smithy model), so the schedule stops making rotations due. The rotation
    /// configuration itself is KEPT: "To turn on automatic rotation again, call
    /// RotateSecret", which rotates "with the values already stored in the
    /// secret" when the call names none.
    ///
    /// The staging labels the cancelled rotation left are kept too: "If you
    /// cancel a rotation in progress, it can leave the VersionStage labels in an
    /// unexpected state. You might need to remove the staging label AWSPENDING
    /// from the partially created version. ... Failing to clean up a cancelled
    /// rotation can block you from starting future rotations." — the caller
    /// cleans them up with `UpdateSecretVersionStage`, which is also why the id
    /// is reported back here.
    pub fn cancel_rotation(&mut self, at: SystemTime) -> Option<String> {
        let cancelled = self
            .rotation_in_progress()
            .then(|| self.pending_version_id().map(str::to_string))
            .flatten();
        self.rotation_enabled = false;
        self.next_rotation_date = None;
        self.last_changed_date = at;
        cancelled
    }
}

/// The rotation configuration one `RotateSecret` call carries: a member it does
/// not name is `None` and leaves what the secret already stores in place.
#[derive(Debug, Clone, Default)]
pub struct RotationConfig {
    pub lambda_arn: Option<String>,
    pub rules: Option<StoredRotationRules>,
    pub metadata: Option<Vec<StoredKeyValue>>,
    pub role_arn: Option<String>,
}

/// Where one secret lives: the Region it is homed in and the name it carries
/// there.
///
/// The secret NAMESPACE is per Region — "ReplicateSecretToRegions creates a
/// separate secret in each Region", and a replica "is the same as the original
/// primary secret except the Region is changed"
/// (`StopReplicationToReplicaResponse$ARN`, this service's Smithy model) — so one
/// name identifies at most one secret IN A REGION, and the same name in another
/// Region is another secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretKey {
    pub region: String,
    pub name: String,
}

impl SecretKey {
    /// The key of the secret named `name` in `region`.
    pub fn new(region: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            name: name.into(),
        }
    }
}

/// Per-service state for com-amazonaws-secretsmanager-secretsmanager. A handler reaches it through [`service_state`].
///
/// Secrets are held by Region and name ([`SecretKey`]), which is the namespace
/// the service itself has: a `SecretId` resolves only within the Region the
/// request was signed for, so the same name in two Regions is two secrets — the
/// primary and its replica among them. The quota in [`SECRET_QUOTA`] bounds the
/// whole store.
#[derive(Debug, Default)]
pub struct ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Secrets by Region and name, in that order, so `ListSecrets` pages one
    /// Region deterministically in name order.
    pub secrets: BTreeMap<SecretKey, Secret>,
    /// Full ARN to the secret it names, so a `SecretId` is accepted in either
    /// form.
    pub arn_index: HashMap<String, SecretKey>,
}

impl ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Store a secret and index its ARN.
    ///
    /// Replacing the secret at a key drops the ARN index entry of the secret it
    /// replaces, so no ARN that names nothing is left resolvable.
    pub fn insert(&mut self, secret: Secret) {
        let key = SecretKey::new(secret.region.clone(), secret.name.clone());
        self.arn_index.insert(secret.arn.clone(), key.clone());
        if let Some(replaced) = self.secrets.insert(key.clone(), secret) {
            let still_current = self
                .secrets
                .get(&key)
                .is_some_and(|current| current.arn == replaced.arn);
            if !still_current {
                self.arn_index.remove(&replaced.arn);
            }
        }
    }

    /// Remove the secret at `key`, dropping its ARN index entry with it.
    pub fn remove(&mut self, key: &SecretKey) -> Option<Secret> {
        let removed = self.secrets.remove(key)?;
        self.arn_index.remove(&removed.arn);
        Some(removed)
    }

    /// The secret a `SecretId` names in `region`: the bare secret name, the full
    /// ARN, or the partial ARN (the ARN without the trailing hyphen and six
    /// random characters, which a caller that never saw the full ARN can still
    /// build).
    ///
    /// Resolution never leaves `region`: a request is addressed to one Region's
    /// endpoint, and the secret it names is the one that Region holds — an ARN
    /// naming another Region's secret names nothing here.
    pub fn resolve(&self, region: &str, secret_id: &str) -> Option<SecretKey> {
        let by_name = SecretKey::new(region, secret_id);
        if self.secrets.contains_key(&by_name) {
            return Some(by_name);
        }
        if let Some(key) = self.arn_index.get(secret_id) {
            return (key.region == region).then(|| key.clone());
        }
        // The partial-ARN scan walks the name-ordered map, not the ARN hash
        // map, so the match it picks does not depend on hash iteration order.
        self.secrets
            .values()
            .find(|secret| secret.region == region && partial_arn(&secret.arn) == secret_id)
            .map(|secret| SecretKey::new(secret.region.clone(), secret.name.clone()))
    }

    /// The secret a `SecretId` names in `region`.
    pub fn get(&self, region: &str, secret_id: &str) -> Option<&Secret> {
        let key = self.resolve(region, secret_id)?;
        self.secrets.get(&key)
    }

    /// The secret a `SecretId` names in `region`, for mutation.
    pub fn get_mut(&mut self, region: &str, secret_id: &str) -> Option<&mut Secret> {
        let key = self.resolve(region, secret_id)?;
        self.secrets.get_mut(&key)
    }

    /// The secret named `name` in `region`, by name alone.
    pub fn named(&self, region: &str, name: &str) -> Option<&Secret> {
        self.secrets.get(&SecretKey::new(region, name))
    }

    /// Every secret `region` holds, in name order.
    pub fn in_region<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Secret> + 'a {
        self.secrets
            .values()
            .filter(move |secret| secret.region == region)
    }

    /// Record that a read in the secret's own Region retrieved the value of the
    /// version `version_id`.
    ///
    /// A read of a REPLICA is a read of the secret in the replica's Region, which
    /// is what the primary's replication view reports
    /// (`ReplicationStatusType$LastAccessedDate`), so the stamp lands on both.
    pub fn record_access(&mut self, key: &SecretKey, version_id: &str, at: SystemTime) {
        let Some(secret) = self.secrets.get_mut(key) else {
            return;
        };
        secret.record_access(version_id, at);
        let Some(primary_region) = secret.replica_of.clone() else {
            return;
        };
        let primary_key = SecretKey::new(primary_region, key.name.clone());
        if let Some(primary) = self.secrets.get_mut(&primary_key) {
            if let Some(replica) = primary
                .replicas
                .iter_mut()
                .find(|replica| replica.region == key.region)
            {
                replica.last_accessed_date = Some(start_of_day(at));
            }
        }
    }

    /// Permanently remove every secret whose deletion the clock has already made
    /// due — the end of its recovery window, or the short asynchronous delay a
    /// `ForceDeleteWithoutRecovery` deletion skips the window for. This is the
    /// deletion the clock has already made due — it was owed at this instant
    /// whether or not anybody called — so a reporting operation materialises it
    /// before it answers, and never reports a secret the service would already
    /// have deleted.
    pub fn expire_due(&mut self, now: SystemTime) {
        let due: Vec<SecretKey> = self
            .secrets
            .iter()
            .filter(|(_, secret)| secret.purge_at.is_some_and(|at| at <= now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            self.remove(&key);
        }
    }

    /// Materialise every replica transition the clock has already made due,
    /// across every secret the store holds. Like [`expire_due`], this is a change
    /// that was owed at this instant whether or not anybody called, so an
    /// operation that reports a replication view performs it before it answers
    /// and never reports a replica as still being created once its span has
    /// elapsed.
    ///
    /// Nothing is propagated here: a replica holds no copy of its primary to
    /// bring up to date (see [`replication`]).
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub fn settle_replicas_due(&mut self, now: SystemTime) {
        for secret in self.secrets.values_mut() {
            secret.settle_replicas(now);
        }
    }
}

/// An ARN with the trailing hyphen and six random characters removed; the ARN
/// unchanged when it does not end that way.
fn partial_arn(arn: &str) -> &str {
    match arn.len().checked_sub(ARN_SUFFIX_LEN + 1) {
        Some(cut) if arn.as_bytes().get(cut) == Some(&b'-') => &arn[..cut],
        _ => arn,
    }
}

/// The secret NAME a `SecretId` spells: "the ARN or name of the secret"
/// (`DeleteSecretRequest$SecretId`, this service's Smithy model), so an id that is
/// an ARN spells the name in its resource field — without the hyphen and six
/// random characters a secret's ARN ends in, which belong to the secret rather
/// than to its name.
///
/// This is the name of the secret the id NAMES, whether or not this store holds
/// one: it is what an operation that answers for a secret it does not hold —
/// `DeleteSecret` with `ForceDeleteWithoutRecovery` — reports as the `Name`.
#[must_use]
pub fn secret_name_in(secret_id: &str) -> &str {
    let Some(resource) = secret_id
        .strip_prefix("arn:")
        .and_then(|rest| rest.split_once("secret:"))
        .map(|(_, resource)| resource)
    else {
        return secret_id;
    };
    let stripped = partial_arn(resource);
    // Only a suffix of the six characters the service draws from letters and
    // digits is one of its own; a name that ends in a hyphen and six other
    // characters keeps them.
    if stripped.len() < resource.len()
        && resource[stripped.len() + 1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return stripped;
    }
    resource
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecretsmanagerSecretsmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecretsmanagerSecretsmanagerState>)
}

#[cfg(test)]
mod tests {
    use ::chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

    use super::*;

    /// What a row expects the next due instant to be, as the TEST derives it —
    /// independently of the schedule evaluation under test.
    enum Expected {
        /// This span past the instant the schedule is measured from.
        After(Duration),
        /// The first instant past it, on the minute, that this predicate holds
        /// for — the test's own reading of the calendar the expression names.
        FirstMatching(fn(&DateTime<Utc>) -> bool),
    }

    impl Expected {
        /// The next due instant expected when the schedule is measured from
        /// `anchor`.
        fn at(&self, anchor: SystemTime) -> SystemTime {
            match self {
                Self::After(span) => anchor + *span,
                Self::FirstMatching(holds) => {
                    let from: DateTime<Utc> = anchor.into();
                    // The expression names instants on the minute, so the search
                    // starts at the first whole minute strictly past `anchor` and
                    // steps a minute at a time. Six weeks of minutes is past
                    // every calendar these rows name.
                    let mut at = from
                        .with_second(0)
                        .and_then(|at| at.with_nanosecond(0))
                        .expect("a whole minute")
                        + ::chrono::Duration::minutes(1);
                    for _ in 0..60_480 {
                        if holds(&at) {
                            return at.into();
                        }
                        at += ::chrono::Duration::minutes(1);
                    }
                    panic!("no instant matching this row's calendar inside six weeks");
                }
            }
        }
    }

    /// A secret with no versions, to configure rotation on.
    fn secret_at(at: SystemTime) -> Secret {
        Secret::new(
            "db-credentials".to_string(),
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:db-credentials-AbCdEf"
                .to_string(),
            "us-west-2".to_string(),
            at,
        )
    }

    /// One version of a secret, carrying `stages`.
    fn version(version_id: &str, stages: &[&str], at: SystemTime) -> SecretVersion {
        let value = envelope::SecretValue::from_request(Some(&"a-value".to_string()), None)
            .expect("a value the request carried");
        SecretVersion {
            version_id: version_id.to_string(),
            value: envelope::StoredValue::as_sent(&value),
            version_stages: stages.iter().map(|stage| (*stage).to_string()).collect(),
            created_date: at,
            last_accessed_date: None,
        }
    }

    /// The rotation configuration carrying `rules`.
    fn config(rules: StoredRotationRules) -> RotationConfig {
        RotationConfig {
            lambda_arn: Some(
                "arn:aws:lambda:us-west-2:000000000000:function:rotator".to_string(),
            ),
            rules: Some(rules),
            metadata: None,
            role_arn: None,
        }
    }

    /// Rules naming a schedule by `AutomaticallyAfterDays`.
    fn after_days(days: i64) -> StoredRotationRules {
        StoredRotationRules {
            automatically_after_days: Some(days),
            ..StoredRotationRules::default()
        }
    }

    /// Rules naming a schedule by `ScheduleExpression`, with an optional
    /// rotation-window `Duration`.
    fn expression(schedule: &str, duration: Option<&str>) -> StoredRotationRules {
        StoredRotationRules {
            schedule_expression: Some(schedule.to_string()),
            duration: duration.map(str::to_string),
            ..StoredRotationRules::default()
        }
    }

    /// An instant a fixed number of seconds past the epoch, so every row runs on
    /// a calendar the test states rather than on the host's clock.
    fn instant(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// RULE: the next due instant is the first instant the stored schedule names
    /// after the event it is measured from — the call that configured rotation,
    /// then every later rotation of the secret's value — and it is cleared while
    /// rotation is off.
    ///
    /// The rows differ along how the schedule is stated (an interval in days, a
    /// `rate()` interval, a `cron()` calendar by weekday and by day of month)
    /// and so along what re-anchoring it moves the instant to. Every instant is
    /// the test's own, which is what makes the ANCHOR observable: each event
    /// below falls on a different day of a stated calendar.
    #[test]
    fn the_next_rotation_date_is_measured_from_the_last_rotation_of_the_value() {
        // 2023-11-14T22:13:20Z, a Tuesday, and the events that follow it.
        let configured = instant(1_700_000_000);
        let rotated = configured + Duration::from_secs(10 * SECONDS_PER_DAY);
        let written = rotated + Duration::from_secs(20 * SECONDS_PER_DAY);
        let staged = written + Duration::from_secs(SECONDS_PER_DAY);

        // (case, the stored rules, what the next due instant is measured as)
        let rows: Vec<(&str, StoredRotationRules, Expected)> = vec![
            (
                "an interval in days",
                after_days(30),
                Expected::After(Duration::from_secs(30 * SECONDS_PER_DAY)),
            ),
            (
                "a rate in hours",
                expression("rate(12 hours)", None),
                Expected::After(Duration::from_secs(12 * SECONDS_PER_HOUR)),
            ),
            (
                "a cron window on one weekday",
                expression("cron(0 16 ? * SAT *)", Some("3h")),
                Expected::FirstMatching(|at| {
                    at.weekday() == Weekday::Sat && at.hour() == 16 && at.minute() == 0
                }),
            ),
            (
                "a cron schedule on one day of the month",
                expression("cron(45 6 15 * ? *)", None),
                Expected::FirstMatching(|at| {
                    at.day() == 15 && at.hour() == 6 && at.minute() == 45
                }),
            ),
            (
                "a cron schedule every day",
                expression("cron(0 8 * * ? *)", None),
                Expected::FirstMatching(|at| at.hour() == 8 && at.minute() == 0),
            ),
        ];

        for (case, rules, expected) in rows {
            let mut secret = secret_at(configured);
            secret.configure_rotation(config(rules), configured);
            assert_eq!(
                secret.next_rotation_date,
                Some(expected.at(configured)),
                "case {case:?}: expected the schedule measured from the call that configured \
                 it, got {:?}",
                secret.next_rotation_date
            );

            // A rotation that COMPLETED re-anchors the schedule to itself.
            secret.finish_rotation(rotated);
            assert_eq!(
                secret.last_rotated_date,
                Some(rotated),
                "case {case:?}: expected the completed rotation to be stamped, got {:?}",
                secret.last_rotated_date
            );
            assert_eq!(
                secret.next_rotation_date,
                Some(expected.at(rotated)),
                "case {case:?}: expected the schedule measured from the completed rotation, got \
                 {:?}",
                secret.next_rotation_date
            );

            // A write that becomes the CURRENT value is a rotation of the value,
            // so the schedule follows it — and it is not a rotation this service
            // performed, so the completed-rotation stamp stays where it was.
            secret.put_version(version("manual-write", &[STAGE_CURRENT], written), written);
            assert_eq!(
                secret.next_rotation_date,
                Some(expected.at(written)),
                "case {case:?}: expected the schedule measured from the manual write, got {:?}",
                secret.next_rotation_date
            );
            assert_eq!(
                secret.last_rotated_date,
                Some(rotated),
                "case {case:?}: expected the manual write to leave LastRotatedDate alone, got \
                 {:?}",
                secret.last_rotated_date
            );

            // A version staged for a rotation that has not finished is not the
            // current value, so it re-anchors nothing.
            secret.put_version(version("staged-write", &[STAGE_PENDING], staged), staged);
            assert_eq!(
                secret.next_rotation_date,
                Some(expected.at(written)),
                "case {case:?}: expected a staged write to leave the schedule where it was, got \
                 {:?}",
                secret.next_rotation_date
            );

            // Nothing is due while rotation is off, and a later write does not
            // put the secret back on its schedule.
            secret.cancel_rotation(staged);
            assert_eq!(
                secret.next_rotation_date, None,
                "case {case:?}: expected no rotation due once rotation is off, got {:?}",
                secret.next_rotation_date
            );
            secret.put_version(
                version("write-while-off", &[STAGE_CURRENT], staged),
                staged,
            );
            assert_eq!(
                secret.next_rotation_date, None,
                "case {case:?}: expected a write while rotation is off to schedule nothing, got \
                 {:?}",
                secret.next_rotation_date
            );
        }
    }

    /// RULE: a `cron()` expression names occurrences on the UTC calendar, so the
    /// schedule it states answers with the first occurrence past the instant it
    /// is measured from — and with none at all once the expression names no
    /// further occurrence.
    ///
    /// The rows differ along which field carries the restriction: a month name,
    /// a list, a step, a range, a day of the week by number and by name (the
    /// field is numbered from Sunday, as the schedule-expression reference this
    /// member's documentation links names it), a month and a weekday whose names
    /// carry one of the letters the service's own wildcard dialect uses, and a
    /// year already behind the clock.
    #[test]
    fn a_cron_expression_names_occurrences_on_the_utc_calendar() {
        // 2024-02-29T12:00:00Z, a Thursday in a leap year.
        let anchor = instant(1_709_208_000);
        // (case, the expression, the instant it next makes a rotation due)
        let rows: Vec<(&str, &str, Option<SystemTime>)> = vec![
            (
                "a month named in words",
                "cron(0 9 1 MAR ? *)",
                // 2024-03-01T09:00:00Z
                Some(instant(1_709_283_600)),
            ),
            (
                "a list of hours",
                "cron(0 13,18 * * ? *)",
                // 2024-02-29T13:00:00Z
                Some(instant(1_709_211_600)),
            ),
            (
                "a step over the hours",
                "cron(0 */6 * * ? *)",
                // 2024-02-29T18:00:00Z
                Some(instant(1_709_229_600)),
            ),
            (
                "a range of days of the month",
                "cron(30 0 1-5 * ? *)",
                // 2024-03-01T00:30:00Z
                Some(instant(1_709_253_000)),
            ),
            (
                "a year already behind the clock",
                "cron(0 10 1 1 ? 2000)",
                None,
            ),
            (
                "a day of the week by number, where 1 is Sunday",
                "cron(0 12 ? * 1 *)",
                // 2024-03-03T12:00:00Z, the Sunday after the anchor's Thursday.
                Some(instant(1_709_467_200)),
            ),
            (
                "a day of the week by name",
                "cron(0 12 ? * SUN *)",
                // The same Sunday: the two spellings name one day.
                Some(instant(1_709_467_200)),
            ),
            (
                "a month whose name carries one of the dialect's letters",
                "cron(0 9 1 JUL ? *)",
                // 2024-07-01T09:00:00Z: `JUL` is an ordinary month name, not an
                // `L` wildcard.
                Some(instant(1_719_824_400)),
            ),
            (
                "a day of the week whose name carries one of them",
                "cron(0 12 ? * WED *)",
                // 2024-03-06T12:00:00Z, the Wednesday after the anchor's
                // Thursday: `WED` is an ordinary weekday name, not a `W`
                // wildcard.
                Some(instant(1_709_726_400)),
            ),
            (
                "a range of weekday names carrying one of them",
                "cron(0 12 ? * MON-WED *)",
                // The Monday after the anchor, 2024-03-04T12:00:00Z.
                Some(instant(1_709_553_600)),
            ),
            (
                "the same weekday name in lower case",
                "cron(0 12 ? * wed *)",
                // The same Wednesday: a field is the same statement however it
                // is cased.
                Some(instant(1_709_726_400)),
            ),
        ];

        for (case, expression, due) in rows {
            let schedule = parse_rotation_rules(&schedule_rules(expression))
                .unwrap_or_else(|refusal| {
                    panic!("case {case:?}: expected {expression} to state a schedule, got {refusal}")
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected {expression} to state a schedule")
                });
            let next = schedule.next_after(anchor);
            assert_eq!(
                next, due,
                "case {case:?}: expected {expression} to make a rotation due at {due:?} after \
                 {anchor:?}, got {next:?}"
            );
        }
    }

    /// RULE: a `cron()` expression in the wildcard dialect the service's own
    /// schedule-expression reference documents — `L` (last), `W` (nearest
    /// weekday) and `#` (nth weekday of the month) — is read on the SERVICE's
    /// grammar: the rules are accepted and stored, and this service computes no
    /// occurrence from them, rather than the expression being refused for a
    /// grammar the service does not state.
    ///
    /// The rows differ along which wildcard the expression carries and which
    /// field carries it; the refusing rows show that a field outside the range
    /// its place allows is still refused, wildcard or no wildcard.
    #[test]
    fn the_services_own_cron_wildcards_are_read_on_its_own_grammar() {
        // (case, an expression in the service's own dialect)
        let dialect: Vec<(&str, &str)> = vec![
            ("the last day of the month", "cron(0 16 L * ? *)"),
            ("the nearest weekday to a day of the month", "cron(0 8 15W * ? *)"),
            ("the last weekday of the month", "cron(0 8 LW * ? *)"),
            ("the third Friday of the month", "cron(0 10 ? * 6#3 *)"),
            ("the last Friday of the month", "cron(0 10 ? * 6L *)"),
        ];
        for (case, expression) in dialect {
            let schedule = parse_rotation_rules(&schedule_rules(expression)).unwrap_or_else(
                |refusal| panic!("case {case:?}: expected {expression} to be read, got {refusal}"),
            );
            assert!(
                schedule.is_none(),
                "case {case:?}: expected no occurrence computed for {expression}"
            );
        }

        // (case, an expression no grammar reads in the place the field stands)
        let refused: Vec<(&str, &str)> = vec![
            ("an hour past the end of the day", "cron(0 24 1 * ? *)"),
            (
                "an hour past the end of the day beside one of those wildcards",
                "cron(0 24 L * ? *)",
            ),
            ("a month that is not one of the twelve", "cron(0 8 1 13 ? *)"),
        ];
        for (case, expression) in refused {
            let refusal = parse_rotation_rules(&schedule_rules(expression));
            assert!(
                refusal.is_err(),
                "case {case:?}: expected {expression} to be refused, got {:?}",
                refusal.map(|schedule| schedule.is_some())
            );
        }
    }

    /// Rotation rules naming a schedule by `ScheduleExpression`, for the wire.
    fn schedule_rules(expression: &str) -> RotationRulesType {
        RotationRulesType {
            automatically_after_days: None,
            duration: None,
            schedule_expression: Some(expression.to_string()),
        }
    }
}
