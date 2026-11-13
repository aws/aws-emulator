// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice service.
//!
//! The service owns one resource, the asynchronous data-automation invocation
//! (`AutomationJobResource` in the model). `InvokeDataAutomationAsync` mints one and
//! persists it here; `GetDataAutomationStatus`, `TagResource`, `ListTagsForResource`
//! and `UntagResource` all resolve a caller-supplied ARN against
//! [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::invocations`]
//! and compute their answer from the stored record.
//!
//! A submission also REFERENCES resources this service does not own — the
//! data-automation project, or the blueprints, that say what to extract. Those belong to
//! the data-automation build-time service, which is emulated in this workspace, so
//! [`unresolved_reference_refusal`] resolves such a reference against THAT service's
//! store rather than recording it as opaque text.
//!
//! The ASSET a submission points at is not owned here either: it is an object of Amazon
//! S3, which this emulator also serves, so the job's work — which begins by reading that
//! asset — resolves it through the framework's in-process callback
//! ([`resolve_input_asset`]), the job's terminal outcome is settled on what that
//! resolution established, and what the job extracted from the asset is written back into
//! the output location the submission named ([`job_output`]) before that outcome is
//! reported.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state as build_time;

/// What reading an asset's bytes establishes about it, and the text extracted from it —
/// which is what a finished job's output is written from.
pub mod asset_extraction;

/// Whether the bytes of an asset are an image this service can process, which is what a
/// job pointed at an image settles its outcome on.
pub mod input_image;

/// The EventBridge events a job's lifecycle publishes for a submission that asked for
/// them.
pub mod job_events;

/// What a finished job writes into the output location its submission named, and the
/// settle that delivers it before the job reports itself finished.
pub mod job_output;

use crate::ops::types::{
    AssetProcessingConfiguration, Blueprint, DataAutomationConfiguration, EncryptionConfiguration,
    InternalServerException, NotificationConfiguration, ResourceNotFoundException, Tag,
    TimestampSegment, VideoSegmentConfiguration,
};

/// The region every ARN this service mints falls back to when the request carries no
/// region an ARN can be minted in: an unsigned, host-routed request with no SigV4
/// credential scope to read one from, or a scope whose region is outside the label
/// grammar [`arn_region`] holds it to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `arnNamespace` (`aws.api#service`): every resource ARN is under
/// `arn:aws:bedrock:…`, which is also what the modelled `InvocationArn` pattern
/// requires.
const ARN_NAMESPACE: &str = "bedrock";

/// The resource portion prefix of an invocation ARN, as the modelled `InvocationArn` /
/// `TaggableResourceArn` pattern spells it
/// (`…:data-automation-invocation/[a-zA-Z0-9-_]+`).
const INVOCATION_RESOURCE_PREFIX: &str = "data-automation-invocation/";

/// How long a freshly submitted job stays `Created` before the clock owes it
/// `InProgress`.
///
/// AWS publishes no latency for this transition, so the span is CHOSEN: it sits inside
/// the emulator's 250–750 ms window for a clock-owed transition, so a caller that polls
/// immediately after `InvokeDataAutomationAsync` still observes the non-terminal
/// `Created` state rather than a job that was never pending.
pub const CREATED_FOR: Duration = Duration::from_millis(300);

/// How long a job stays `InProgress` before the clock owes it `Success`.
///
/// Chosen on the same basis as [`CREATED_FOR`]: AWS documents no duration for data
/// automation, and a real job's runtime depends on the asset, so this is the emulator's
/// own span inside the 250–750 ms window.
pub const IN_PROGRESS_FOR: Duration = Duration::from_millis(300);

/// The modelled `TagList` bound (`@length(min: 0, max: 200)`): the most tags one
/// resource can carry.
pub const MAX_TAGS: usize = 200;

/// The modelled `TagKey` pattern, verbatim (`smithy_query` shape `TagKey`).
///
/// Stated here because this crate has to apply it itself rather than leave it to the
/// framework's boundary walk: see [`tag_key_is_well_formed`].
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";

/// The tag-key prefix the modelled `TagKey` pattern excludes with its negative
/// lookahead — the namespace AWS reserves for its own tags. Stated for the refusal
/// documentation; the pattern itself is what [`tag_key_is_well_formed`] applies.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The modelled [`TAG_KEY_PATTERN`] with its leading look-around removed and NOTHING else
/// changed — the declared anchors and the declared character class — compiled once.
///
/// The expression is derived from the declared source text rather than written out a
/// second time, and the part removed is built from [`RESERVED_TAG_KEY_PREFIX`], so the
/// class this matches cannot drift from the class the model declares and the prefix
/// [`tag_key_is_well_formed`] tests cannot drift from the one the pattern names.
///
/// The engine is the same linear-time one the framework's boundary walk compiles a
/// `@pattern` with, so a match is bounded by the length of the value and nothing has to
/// bound the matcher itself. Compilation is fallible and its input is a compile-time
/// constant, never request text, but a pattern that failed to compile is dropped rather
/// than unwrapped: nothing here panics, on any input.
static TAG_KEY_CLASS_REGEX: LazyLock<Option<::regex::Regex>> = LazyLock::new(|| {
    let look_around = format!("(?!{RESERVED_TAG_KEY_PREFIX})");
    let class_only = TAG_KEY_PATTERN.replacen(&look_around, "", 1);
    // Both operands are compile-time constants, so this holds or it never holds: it says
    // the declared pattern still opens with the look-around over the reserved prefix, and
    // so that the expression below is the declared one minus exactly that look-around.
    debug_assert!(
        class_only != TAG_KEY_PATTERN,
        "the modelled TagKey pattern no longer opens with the reserved-prefix look-around"
    );
    match ::regex::Regex::new(&class_only) {
        Ok(compiled) => Some(compiled),
        Err(error) => {
            // Reachable only if the declared pattern stops decomposing this way, and a
            // rule that is not applied must not read as a value that satisfied it.
            ::tracing::warn!(
                pattern = TAG_KEY_PATTERN,
                %error,
                "the modelled TagKey character class is not being applied",
            );
            None
        }
    }
});

/// Whether `key` satisfies the modelled [`TAG_KEY_PATTERN`].
///
/// This one modelled constraint cannot be left to the framework's boundary walk: the
/// pattern opens with the negative look-around `(?!aws:)`, which the walk's regex engine
/// cannot compile, and a `@pattern` that fails to compile is treated as matching EVERY
/// value — which drops the character-class half of the pattern along with the look-around
/// (`vendor/smithy-schema-server/src/constraints.rs`). So the pattern is applied here, by
/// the three operations that accept a tag key (`InvokeDataAutomationAsync` and
/// `TagResource` through `tags`, `UntagResource` through `tagKeys`).
///
/// Both halves are applied as declared, neither approximated. The look-around sits
/// immediately behind the pattern's `^`, so "no match begins at position 0" is exactly
/// "the key does not begin with [`RESERVED_TAG_KEY_PREFIX`]" — a prefix test, and nothing
/// weaker. Everything else, anchors and class alike, is matched by
/// [`TAG_KEY_CLASS_REGEX`] from the declared source text: reading the class off `char`'s
/// properties instead cannot express it, because `char::is_alphabetic` is the Unicode
/// `Alphabetic` property — `\p{L}` plus `Nl` plus `Other_Alphabetic` (combining marks) —
/// so a key carrying such a mark would be admitted where the class names only `L`, `Z`,
/// `N` and eight literal characters. The refusal quotes this same source text
/// ([`pattern_requirement`]), so what a clause states and what is applied are one thing.
///
/// A class that holds no compiled form is treated as matching, which is what the boundary
/// walk does with a pattern it cannot compile: a value is never refused because of a model
/// pattern nothing could evaluate.
pub fn tag_key_is_well_formed(key: &str) -> bool {
    !key.starts_with(RESERVED_TAG_KEY_PREFIX)
        && TAG_KEY_CLASS_REGEX
            .as_ref()
            .is_none_or(|compiled| compiled.is_match(key))
}

/// The requirement clause this service states for a `@pattern` violation: the declared
/// pattern WITHOUT its outer `^`/`$` anchors, which a Smithy `@pattern` does not need
/// (the trait is a find, not a full match).
///
/// The anchor stripping itself is `service_common::validation::unanchored` — the staged,
/// shared home for that rendering, which drops the pair only when it wraps the WHOLE
/// expression and is not an escaped `$`, so an alternation that anchors each branch
/// (`^a$|^b$`) is left alone rather than mangled.
///
/// PENDING MIGRATION: the REQUIREMENT half below (the sentence per `ViolationKind`, with
/// [`violation_clause`], [`validation_envelope`] and `service::clause_for_violation`) is
/// this service's inlined copy of a service-independent validation-wording rule that
/// `services/common/src/validation.rs` stages but does not state yet — its "Still to
/// migrate here" section lists it as not yet there, so there is no shared helper to call
/// for it today. The fix that owns the migration (~24 crates carry the same copy) moves
/// those rules there and calls them from here. The note is left here, as that module
/// asks, because this change does not own that migration.
pub fn pattern_requirement(pattern: &str) -> String {
    let unanchored = service_common::validation::unanchored(pattern);
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// One clause of a refusal, as this service words it: the member it names and the
/// requirement that member failed, with the offending value NOT echoed.
///
/// Pending the migration noted on [`pattern_requirement`].
///
/// `path` is a path spelled by the framework's own path rule
/// ([`::aws_smithy_schema_server::constraints::path`]) — the module BOTH halves of this
/// service's validation build their paths through, so a position the walk reports and the
/// same position a handler reports are spelled identically.
pub fn violation_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The whole `ValidationException` message around one or more [`violation_clause`]s, in
/// the AWS envelope (`1 validation error detected: …` / `N validation errors detected:
/// …; …`).
///
/// The envelope itself is a service dialect, but the clause halves it joins are not:
/// pending the migration noted on [`pattern_requirement`].
///
/// The ONE place this service's refusal envelope is written: the boundary walk's
/// refusals reach it through `service::override_refusal`, and a rule the walk cannot
/// express (the `TagKey` lookahead) reaches it through [`malformed_tag_key_message`], so
/// one logical failure cannot read two different ways depending on which side caught it.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The `ValidationException` message for the tag keys outside [`TAG_KEY_PATTERN`] a
/// request carried, one clause per offending key at the input path `paths` names it at.
///
/// Every offender is stated, not just the first, because that is what the boundary walk
/// does with the constraints it CAN apply: it descends the whole list and accumulates a
/// violation per element, and [`validation_envelope`] joins them. The count is bounded by
/// the list itself (`@length(max: 200)`, [`MAX_TAGS`]).
pub fn malformed_tag_key_message(paths: &[String]) -> String {
    let requirement = pattern_requirement(TAG_KEY_PATTERN);
    let clauses: Vec<String> = paths
        .iter()
        .map(|path| violation_clause(path, &requirement))
        .collect();
    validation_envelope(&clauses)
}

/// The Smithy member name of the list of whole tags, which both operations that accept
/// whole tags submit their tags at (`smithy_query` operation `TagResource` /
/// `InvokeDataAutomationAsync`).
const TAGS_MEMBER: &str = "tags";

/// The Smithy member name of the key within one `Tag` (`smithy_query` shape `Tag`).
const TAG_KEY_MEMBER: &str = "key";

/// The Smithy member name of the list of bare tag keys (`smithy_query` operation
/// `UntagResource`).
const TAG_KEYS_MEMBER: &str = "tagKeys";

/// The input path the `key` of the `tags` element at 1-based `position` occupies, built
/// with the framework's own path rule ([`::aws_smithy_schema_server::constraints::path`]):
/// the list member, the element's position and the step into it, then the member within
/// that element.
fn tags_member_key_path(position: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member(TAGS_MEMBER),
        path::element(position),
        path::member(TAG_KEY_MEMBER),
    ])
}

/// The input path the `tagKeys` element at 1-based `position` occupies: the list holds the
/// keys themselves, so the path stops at the step into that element.
fn tag_keys_member_path(position: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[path::member(TAG_KEYS_MEMBER), path::element(position)])
}

/// The refusal a sequence of tag keys earns, or `None` when every one of them satisfies
/// [`TAG_KEY_PATTERN`]; `path_of` spells the input path of the element at a 1-based
/// position.
///
/// The scan itself — WHICH keys are looked at, and what a malformed one reads as — lives
/// here once, so the operations that accept a tag key cannot come to disagree about the
/// rule (whether an empty list is refusable, whether the first offender or every offender
/// is reported, how the element is named).
fn malformed_tag_key_refusal_at<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    path_of: impl Fn(usize) -> String,
) -> Option<String> {
    let paths: Vec<String> = keys
        .into_iter()
        .enumerate()
        .filter(|(_, key)| !tag_key_is_well_formed(key))
        .map(|(index, _)| path_of(index + 1))
        .collect();
    (!paths.is_empty()).then(|| malformed_tag_key_message(&paths))
}

/// The refusal a `tags` list earns, or `None` when every key in it is well formed.
///
/// The single home of the rule for the operations that accept whole tags
/// (`InvokeDataAutomationAsync` and `TagResource`), which differ only in whether the
/// member is optional — an absent list is an empty slice, and carries no key to refuse.
pub fn malformed_tag_refusal(tags: &[Tag]) -> Option<String> {
    malformed_tag_key_refusal_at(tags.iter().map(|tag| tag.key.as_str()), tags_member_key_path)
}

/// The refusal a `tagKeys` list earns, or `None` when every key in it is well formed:
/// the same rule as [`malformed_tag_refusal`], for the operation that carries bare keys
/// (`UntagResource`) and so names a different input path.
pub fn malformed_tag_key_refusal(tag_keys: &[String]) -> Option<String> {
    malformed_tag_key_refusal_at(tag_keys.iter().map(String::as_str), tag_keys_member_path)
}

/// Why a submission that names neither a data-automation project nor an inline blueprint
/// is refused.
///
/// This service's own fixed string for that refusal: it carries NOTHING from the
/// request, so reproducing it is fidelity rather than fitting one recorded call. What a
/// client branches on is the HTTP 400 and the `ValidationException` code.
pub const MISSING_PROJECT_OR_BLUEPRINTS: &str =
    "At least one of project or inline blueprints must be provided in the request.";

/// Why a submission that names BOTH a data-automation project and an inline blueprint is
/// refused. A fixed string on the same footing as [`MISSING_PROJECT_OR_BLUEPRINTS`].
///
/// NO RECORDING covers this half of the rule: the corpus only records the "names
/// neither" refusal. It is stated because the two members describe two different
/// extraction targets and the service has no documented tie-break, but a later recording
/// showing AWS accepting both should settle it — and would change only this branch of
/// [`extraction_target_refusal`], not the "names neither" one the corpus pins.
pub const BOTH_PROJECT_AND_BLUEPRINTS: &str =
    "Only one of project or inline blueprints can be provided in the request.";

/// The refusal a submission's extraction members earn, or `None` when they are
/// acceptable.
///
/// A data-automation submission has to say WHAT to extract, and it says so in exactly
/// ONE of two places: a data-automation project
/// (`dataAutomationConfiguration.dataAutomationProjectArn`) or an inline `blueprints`
/// list. Naming neither describes no extraction; naming both describes two, and the
/// service does not choose between them. Both members are individually optional in the
/// model — no Smithy trait expresses "exactly one of" — so this cross-member rule is the
/// service's, and it is single-homed here because BOTH invoke operations
/// (`InvokeDataAutomation` and `InvokeDataAutomationAsync`) apply it identically.
///
/// `blueprints` carries `@length(min: 1)`, so an empty list is already refused by the
/// boundary walk; it counts here as "no inline blueprint" so the rule holds whatever
/// reaches a handler.
pub fn extraction_target_refusal(
    data_automation_configuration: Option<&DataAutomationConfiguration>,
    blueprints: Option<&[Blueprint]>,
) -> Option<&'static str> {
    let names_project = data_automation_configuration
        .is_some_and(|configuration| !configuration.data_automation_project_arn.is_empty());
    let names_blueprint = blueprints.is_some_and(|blueprints| !blueprints.is_empty());
    match (names_project, names_blueprint) {
        (false, false) => Some(MISSING_PROJECT_OR_BLUEPRINTS),
        (true, true) => Some(BOTH_PROJECT_AND_BLUEPRINTS),
        _ => None,
    }
}

/// The account field the modelled project and blueprint ARN patterns admit beside a
/// 12-digit account id (`…:(aws|[0-9]{12}):…`): the literal `aws`, which marks a resource
/// AWS ITSELF owns — the SERVICE-owned catalog of the build-time service's
/// `ResourceOwner` enum — rather than one an account created.
const SERVICE_OWNED_ACCOUNT: &str = "aws";

/// Whether `arn` names a resource an ACCOUNT owns, which is the only kind a reference can
/// be resolved against: the build-time store holds exactly what was created through that
/// service, so a SERVICE-owned resource (account field [`SERVICE_OWNED_ACCOUNT`]) is a
/// catalog entry no store here holds and nothing can be concluded from its absence —
/// refusing it would refuse a reference AWS resolves. Read through the shared ARN parse,
/// so this service and its siblings agree on what an ARN's account field is.
fn names_account_owned_resource(arn: &str) -> bool {
    ::service_common::arn::parse(arn).is_some_and(|parsed| parsed.account != SERVICE_OWNED_ACCOUNT)
}

/// Why a submission naming a data-automation project no such account holds is refused.
///
/// Worded as this service words the sibling refusal for the one resource it owns itself
/// ([`INVOCATION_NOT_FOUND_MESSAGE`]): the resource kind and the member that names it,
/// with the caller's ARN NOT echoed back.
pub const PROJECT_NOT_FOUND_MESSAGE: &str = "Data Automation Project ARN not found.";

/// Why a submission naming a blueprint no such account holds is refused — the same
/// wording rule as [`PROJECT_NOT_FOUND_MESSAGE`], for the other referenced resource.
pub const BLUEPRINT_NOT_FOUND_MESSAGE: &str = "Blueprint ARN not found.";

/// The refusal a submission's REFERENCED resources earn, or `None` when every one of them
/// resolves.
///
/// A submission names what to extract by reference: a data-automation project
/// (`dataAutomationConfiguration.dataAutomationProjectArn`) or inline `blueprints`. Both
/// resources belong to the data-automation build-time service, which has to be read
/// before a job can start — the project's output configuration, or the blueprint's
/// schema, IS the extraction the job performs — so a reference that resolves to nothing
/// describes no job. The model gives this operation no `ResourceNotFoundException`, so
/// the refusal is its `ValidationException` (HTTP 400), raised before anything is minted
/// and so leaving no record behind.
///
/// Resolution reads the owning service's own store through its public
/// `state::service_state`, the way a sibling crate reads another service's state
/// elsewhere in this workspace, so the runtime and the build-time service answer about
/// one set of projects and blueprints rather than two. It is called BEFORE this service's
/// own store guard is taken, so only one engine lock is ever held at a time.
///
/// Two references are not resolved, and neither can be concluded from absence. One is an
/// ARN whose account field is AWS's own ([`names_account_owned_resource`]). The other is a
/// value that is not a resource ARN at all: the framework's boundary walk enforces the
/// modelled `@pattern` on both members before a handler runs, so such a value is already
/// refused as a constraint violation and never reaches here.
///
/// `None` from the owning store means this engine holds no store of that type to read at
/// all, which is only the case for an engine kind this emulator does not mount; on the
/// shared in-memory engine the store is always readable, and an empty one resolves
/// nothing, so every account-owned reference is refused until something creates it.
pub fn unresolved_reference_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    data_automation_configuration: Option<&DataAutomationConfiguration>,
    blueprints: Option<&[Blueprint]>,
) -> Option<&'static str> {
    let control_plane = build_time::service_state(engine)?;
    let store = control_plane.lock();

    let unknown_project = data_automation_configuration
        .map(|configuration| configuration.data_automation_project_arn.as_str())
        .filter(|arn| names_account_owned_resource(arn))
        .is_some_and(|arn| !store.projects.contains_key(arn));
    if unknown_project {
        return Some(PROJECT_NOT_FOUND_MESSAGE);
    }

    let unknown_blueprint = blueprints.unwrap_or_default().iter().any(|blueprint| {
        names_account_owned_resource(&blueprint.blueprint_arn)
            && !store.blueprints.contains_key(&blueprint.blueprint_arn)
    });
    if unknown_blueprint {
        return Some(BLUEPRINT_NOT_FOUND_MESSAGE);
    }

    None
}

/// The timestamp segment a submission delimited, or `None` when it delimited no segment.
///
/// The member is four optional levels deep
/// (`assetProcessingConfiguration.video.segmentConfiguration.timestampSegment`, with the
/// third level a union), so the walk down to it lives here once rather than in every
/// caller that wants the requested processing scope.
pub fn requested_timestamp_segment(
    asset_processing_configuration: Option<&AssetProcessingConfiguration>,
) -> Option<&TimestampSegment> {
    match asset_processing_configuration?
        .video
        .as_ref()?
        .segment_configuration
        .as_ref()?
    {
        VideoSegmentConfiguration::timestampSegment(segment) => Some(segment),
    }
}

/// Why a submission that delimits a segment whose end does not follow its start is
/// refused.
///
/// This service's own fixed string, on the same footing as
/// [`MISSING_PROJECT_OR_BLUEPRINTS`]: it carries NOTHING from the request, and what a
/// client branches on is the HTTP 400 and the `ValidationException` code.
pub const INCONSISTENT_TIMESTAMP_SEGMENT: &str =
    "endTimeMillis must be greater than startTimeMillis in the requested timestamp segment.";

/// The refusal a submission's requested processing scope earns, or `None` when it is
/// acceptable.
///
/// A timestamp segment delimits a half-open interval of the asset, so its end has to lie
/// after its start; a segment whose `endTimeMillis` does not follow its `startTimeMillis`
/// delimits nothing the service could process. The model bounds each member on its own
/// (`startTimeMillis` `@range(min: 0)`, `endTimeMillis` `@range(min: 300000)`) and no
/// Smithy trait relates one member to another, so this cross-member rule is the
/// service's — stated here, beside the other one
/// ([`extraction_target_refusal`]), and applied before anything is minted so a refused
/// submission leaves no record behind.
pub fn timestamp_segment_refusal(
    asset_processing_configuration: Option<&AssetProcessingConfiguration>,
) -> Option<&'static str> {
    let segment = requested_timestamp_segment(asset_processing_configuration)?;
    (segment.end_time_millis <= segment.start_time_millis).then_some(INCONSISTENT_TIMESTAMP_SEGMENT)
}

/// What this service answers when a caller-supplied ARN names no retained invocation —
/// the one message behind `ResourceNotFoundException` for every operation here that
/// resolves an ARN (`GetDataAutomationStatus`, `TagResource`, `ListTagsForResource`,
/// `UntagResource`).
///
/// It is the service's own fixed string and carries NOTHING from the request: the ARN
/// the caller sent is not interpolated, because this service does not echo it back. Kept
/// in one place so the four readers cannot drift apart — through
/// [`unknown_invocation_refusal`], which is the only thing that reads it.
pub const INVOCATION_NOT_FOUND_MESSAGE: &str = "Data Automation Invocation ARN not found.";

/// The whole refusal a caller-supplied ARN naming no retained invocation earns: the
/// `ResourceNotFoundException` shape carrying [`INVOCATION_NOT_FOUND_MESSAGE`], answered
/// with this service's recorded client-error status rather than the status the shape's
/// advisory `smithy.api#httpError` trait declares (see
/// [`crate::scaffolded::Scaffolded::ClientError`]).
///
/// `not_found` is the calling operation's own `ResourceNotFoundException` variant — the
/// shape itself is the one generated struct every operation here shares, so the variant
/// constructor is all that differs between callers. EVERYTHING else about the refusal
/// lives here once: the shape, the message, and the STATUS DECISION the status-override
/// channel documents as evidence-gated. That is the same reason the tag-key rule is
/// single-homed in [`malformed_tag_key_refusal`]: the operations that resolve an ARN
/// cannot come to disagree about what one naming nothing answers.
pub fn unknown_invocation_refusal<E>(
    not_found: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::client_error(not_found(ResourceNotFoundException {
        message: Some(INVOCATION_NOT_FOUND_MESSAGE.to_string()),
    }))
}

/// What this service answers when a tag edit would take one invocation past [`MAX_TAGS`]
/// — the message behind `ServiceQuotaExceededException` for `TagResource`.
///
/// AWS's own wording for this error class, which is ENTIRELY fixed text: it names no
/// resource, no count and no limit, so there is no variable part to fill and nothing the
/// caller sent can reach it. The quota is cumulative per resource, so one sentence covers
/// both halves of it — a single request carrying too many tags, and a request whose tags
/// would take the resource past the bound.
pub const TAG_QUOTA_MESSAGE: &str = "The number of the provided tags is beyond the limit, \
                                     or the number of total tags you are trying to attach \
                                     to the specified resource exceeds the limit.";

/// [`TAG_QUOTA_MESSAGE`], as the owned `String` the modelled error member holds.
pub fn tag_quota_message() -> String {
    TAG_QUOTA_MESSAGE.to_string()
}

/// The account field of an ARN that marks a resource AWS itself owns rather than one an
/// account created — see [`SERVICE_OWNED_ACCOUNT`], which this is the authorization side
/// of: an AWS-owned profile belongs to no account, so no caller is foreign to it.
///
/// What a caller IS authorized against is its own account: the modelled
/// `DataAutomationProfileArn` pattern admits `(aws|[0-9]{12})` in that field, so an ARN
/// naming any other twelve-digit account is well formed and has to be refused on
/// authorization rather than on shape.
///
/// The sentence itself is the shared one every service of this workspace refuses such a
/// request with (`service_common::iam_authorization`), so this service states only WHICH
/// principal it names: the account this emulator serves, which is the account a caller's
/// credentials speak for and the account every ARN it mints carries.
pub fn access_denied_message() -> String {
    ::service_common::iam_authorization::not_authorized_to_access_resource(
        ::emulator_server::emulator_account_id(),
    )
}

/// The refusal a submission's `dataAutomationProfileArn` earns when it names a profile of
/// an account other than the caller's own, or `None` when the caller may use it.
///
/// A submission RUNS under the data-automation profile it names, so the caller has to be
/// allowed to use that profile: a profile scoped to a foreign account never is, whatever
/// the rest of the ARN says, and the refusal is decided from the ARN's account field
/// alone — before anything is minted or persisted, so a denied submission leaves no
/// record and no job behind. The two cases that are NOT foreign are the account this
/// emulator serves, and AWS's own [`SERVICE_OWNED_ACCOUNT`] (a profile AWS publishes for
/// every account to use). A value whose account field cannot be read is left to the
/// framework's `@pattern` walk, which refuses it as a constraint violation.
///
/// The message is AWS's denial sentence ([`access_denied_message`]): it names the CALLER,
/// never the foreign account the request carried, so a caller cannot learn from it
/// whether a resource exists in an account that is not its own.
pub fn cross_account_profile_refusal(data_automation_profile_arn: &str) -> Option<String> {
    let account = ::service_common::arn::parse(data_automation_profile_arn)?.account;
    let authorized =
        account == SERVICE_OWNED_ACCOUNT || account == ::emulator_server::emulator_account_id();
    (!authorized).then(access_denied_message)
}

/// What this service answers when it cannot admit one more invocation — the message
/// behind `ServiceQuotaExceededException` for the two invoke operations.
///
/// Carries only [`MAX_INVOCATIONS`], this service's own constant, and NOTHING taken from
/// the request. NO RECORDING covers this refusal — the bound itself is the emulator's —
/// so unlike [`TAG_QUOTA_MESSAGE`], whose wording is AWS's own for that error class, this
/// sentence is the service's own and states the bound it enforces.
pub fn invocation_quota_message() -> String {
    format!("Cannot have more than {MAX_INVOCATIONS} data automation invocations in progress")
}

/// The most invocation records this emulator retains at once.
///
/// Every `InvokeDataAutomationAsync` call persists one record on a request-reachable
/// path, so without a ceiling the store would grow with traffic forever. The capacity is
/// the emulator's own (AWS publishes no such quota), and it is enforced by
/// [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::admit_invocation`],
/// the one admission path, which evicts finished jobs to make room and refuses the
/// submission when it cannot.
pub const MAX_INVOCATIONS: usize = 1_000;

/// What asking this emulator's own Amazon S3 about the asset a submission named
/// established — the fact the job's terminal outcome is settled on
/// ([`Invocation::advance_to`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAsset {
    /// The job may take the asset on: the object store answered for it and what it holds
    /// is processable, or NOTHING could be established about it — no object store on this
    /// emulator to ask, or an answer that was not the store reporting an absent object —
    /// so no job is failed on an assumption.
    Available,
    /// The object store ITSELF answered that the location the submission named holds no
    /// object, or that location addresses no object at all, so the job has nothing to
    /// read and the work it was accepted for cannot be carried out.
    Absent,
    /// The object store served the asset, and what it holds is an IMAGE this service
    /// cannot process: bytes that are not an image in a format it reads, or an image
    /// outside the dimensions it processes ([`input_image::ImageVerdict`]). The job can
    /// read its asset but not do the work it was accepted for.
    InvalidImage,
}

/// Why a job pointed at an asset the object store holds none of failed — its
/// `errorMessage`.
///
/// This service's own fixed string, on the same footing as
/// [`MISSING_PROJECT_OR_BLUEPRINTS`]: it carries NOTHING from the request, and what a
/// client branches on is the `ClientError` status and the two error members being there
/// at all. NO RECORDING covers a failed data-automation job whose asset was absent, so a
/// later one showing how AWS words this failure settles the wording — and would change
/// only this constant. The failure whose wording IS recorded has its own
/// ([`input_image::INVALID_IMAGE_MESSAGE`]).
pub const INPUT_ASSET_ABSENT_MESSAGE: &str =
    "The input asset could not be read from the Amazon S3 location the submission named.";

/// The `errorType` of a job failed because its asset was not there.
///
/// NO RECORDING covers this failure class, so the type reported is the failure CLASS the
/// status already names, which is all that can be said about it without inventing an AWS
/// vocabulary. The class that IS recorded carries AWS's own type
/// ([`input_image::INVALID_IMAGE_ERROR_TYPE`]).
pub const INPUT_ASSET_ABSENT_ERROR_TYPE: &str = "ClientError";

/// The Amazon S3 error codes in which S3 reports that an object a request names is NOT
/// THERE: a bucket it does not hold (`NoSuchBucket`), a key that bucket does not hold
/// (`NoSuchKey`), and the code `HeadObject` models for either (`NotFound`, which a HEAD
/// carries no body to spell out).
///
/// The code is required TOGETHER WITH the 404 status ([`object_is_absent`]), never the
/// status alone, because a status is not evidence that S3 answered at all: a request that
/// reached no S3 is a 404 too, and that is this emulator failing to ask rather than a
/// verdict about the asset. The list is deliberately CLOSED and holds only the codes that
/// say the object is absent — an answer reporting something ELSE about it establishes
/// nothing and fails no job ([`InputAsset::Available`]).
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NotFound"];

/// Whether S3's answer to a read is S3 ITSELF saying it holds nothing at that address:
/// HTTP 404 AND one of the codes it sends for a missing object ([`ABSENT_OBJECT_CODES`]).
fn object_is_absent<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_sdk_s3::error::ProvideErrorMetadata,
{
    use ::aws_sdk_s3::error::ProvideErrorMetadata;

    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
        && error
            .code()
            .is_some_and(|code| ABSENT_OBJECT_CODES.contains(&code))
}

/// The Amazon S3 object an `s3://` URI addresses: its bucket and its key.
///
/// The modelled `S3Uri` pattern admits a URI naming a bucket with nothing under it
/// (`^s3://<bucket>(/…)?$`), and `inputConfiguration.s3Uri` names the ASSET to process —
/// one object — so the first path segment is the bucket and everything after it is the
/// key, and a URI carrying no key addresses a bucket rather than an asset.
///
/// `None` for a value that addresses no object of the store: no bucket, or no key under
/// it.
fn s3_object_address(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = s3_bucket_and_tail(uri)?;
    (!key.is_empty()).then_some((bucket, key))
}

/// The bucket an `s3://` URI names and everything after it, which may be empty.
///
/// The ONE place this tree's `s3://` form is split, so the two things a submission
/// addresses with it cannot drift apart: the caller decides what an empty tail means — no
/// asset for [`s3_object_address`], the bucket root for
/// [`job_output::output_location`] — and the splitting itself is stated once.
///
/// `None` for a value that is not an `s3://` URI, or that names no bucket.
fn s3_bucket_and_tail(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.trim().strip_prefix("s3://")?;
    let (bucket, tail) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then_some((bucket, tail))
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The ONE client this service reaches object storage with. The callback is the
/// framework's documented seam for calling another emulated service: it routes in
/// process, so no network client is configured here and this crate names no other
/// service's crate. The `endpoint_url` below is therefore an ADDRESS the in-process
/// router reads, never a host any resolver looks up — and the one value that reaches its
/// authority is narrowed by [`arn_region`], the same grammar a minted ARN's region
/// segment is held to, so a region outside it falls back to [`DEFAULT_REGION`] exactly as
/// a request carrying no credential scope does.
///
/// The credentials are the emulator's own: they only sign this operation's in-process
/// calls and are never put in a response or a log.
///
/// `callback` is the handle a handler reaches through
/// [`smithy_server_runtime::OperationContext::callback`]; it is taken directly rather
/// than through the request context so the job's own asset read can also be made once the
/// call that submitted it has answered ([`job_events::settle_when_due`]).
fn s3_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let region = arn_region(region);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The most of one asset this service reads to establish what it holds.
///
/// The emulator's OWN ceiling, not an AWS quota: the classification only needs the
/// asset's header ([`input_image::image_verdict`]), so an object this emulator would have
/// to hold in memory beyond this is not read at all and nothing is established about it —
/// which fails no job. It sits well above the formats whose header this reads.
pub const MAX_CLASSIFIED_ASSET_BYTES: i64 = 8 * 1024 * 1024;

/// Resolve the asset a submission named against this emulator's own Amazon S3 and report
/// what that established.
///
/// A submission points at an asset already in object storage, and the job's work starts
/// by READING it, so the resolution happens when the job runs rather than when the
/// submission is accepted: it is called by whatever settles the terminal transition the
/// clock owes ([`Invocation::terminal_transition_due`]), and its answer is what that
/// transition settles the outcome on. An asset created after the submission but before
/// the job finishes is therefore there for the job, exactly as it would be in the real
/// service.
///
/// The verdict is read from S3's own absent-object answer and never from an HTTP status
/// alone ([`object_is_absent`]): with no Amazon S3 mounted to ask, nothing is known about
/// the asset, so the job completes — a failure has to be GROUNDED in what the object
/// store answered, never assumed. The one verdict that needs no answer is a URI that
/// addresses no object at all ([`s3_object_address`]): it names no asset for any store to
/// hold.
///
/// TWO reads: a `HeadObject` establishes that the asset IS there and how large the store
/// says it is, and then — only for an object inside [`MAX_CLASSIFIED_ASSET_BYTES`] — a
/// `GetObject` reads it, so the job can establish whether what it was pointed at is
/// something it can process at all ([`input_image::image_verdict`]) and what it holds
/// ([`asset_extraction::read_asset`], the extraction a successful job's output is written
/// from). The bytes themselves are dropped with this call: what travels on is the reading.
pub async fn resolve_input_asset(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    s3_uri: &str,
) -> ResolvedAsset {
    use ::aws_sdk_s3::error::ProvideErrorMetadata;

    let Some((bucket, key)) = s3_object_address(s3_uri) else {
        return ResolvedAsset::of(InputAsset::Absent);
    };
    let client = s3_client(callback, region);
    let declared = match client.head_object().bucket(bucket).key(key).send().await {
        // The asset is there, and the store says how large it is.
        Ok(answer) => answer.content_length().unwrap_or_default(),
        Err(error) => {
            if object_is_absent(&error) {
                return ResolvedAsset::of(InputAsset::Absent);
            }
            // Any other answer is not S3 reporting an absent object, so it fails no job —
            // and it is worth a warning, because a deployment with no object store to ask
            // is one where this resolution does not run. The caller's own values are
            // rendered with their escaped `Debug` form, so a key carrying a newline
            // cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the input asset could not be resolved against this emulator's Amazon S3"
            );
            return ResolvedAsset::of(InputAsset::Available);
        }
    };
    // Past this emulator's read ceiling nothing is established about the asset, so the
    // job is not failed on it — and nothing is extracted from it either.
    if declared > MAX_CLASSIFIED_ASSET_BYTES {
        return ResolvedAsset::of(InputAsset::Available);
    }
    let read = match client.get_object().bucket(bucket).key(key).send().await {
        Ok(read) => read,
        Err(error) => {
            // The asset was there a moment ago: whatever this answer is, it is not the
            // store reporting that the asset cannot be processed, so it fails no job.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the input asset's bytes could not be read from this emulator's Amazon S3"
            );
            return ResolvedAsset::of(InputAsset::Available);
        }
    };
    let Ok(body) = read.body.collect().await else {
        ::tracing::warn!(
            bucket = ?bucket,
            key = ?key,
            "the input asset's bytes did not arrive from this emulator's Amazon S3"
        );
        return ResolvedAsset::of(InputAsset::Available);
    };
    let bytes = body.into_bytes();
    let verdict = match input_image::image_verdict(&bytes) {
        // Either the asset is an image this service processes, or it is not an image at
        // all — nothing an image rule can establish about a document, a video or an audio
        // asset, so neither fails the job.
        input_image::ImageVerdict::Processable | input_image::ImageVerdict::NotAnImage => {
            InputAsset::Available
        }
        input_image::ImageVerdict::Invalid => InputAsset::InvalidImage,
    };
    ResolvedAsset {
        verdict,
        reading: asset_extraction::read_asset(&bytes).await,
    }
}

/// Everything one resolution of a job's input asset established: the FACT the job's
/// terminal outcome is settled on, and what reading the asset's bytes found in it.
///
/// The two travel together because they come from the one read: the outcome rests on the
/// verdict alone, and the output a successful job writes is written from the reading
/// ([`job_output`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    /// What the object store established about the asset.
    pub verdict: InputAsset,
    /// What the asset's own bytes hold, when they were read and something recognised them.
    pub reading: Option<asset_extraction::AssetReading>,
}

impl ResolvedAsset {
    /// A resolution that established only `verdict` — nothing was read out of the asset, so
    /// nothing about its content is claimed.
    #[must_use]
    pub fn of(verdict: InputAsset) -> Self {
        Self {
            verdict,
            reading: None,
        }
    }
}

/// The object key, relative to one job's own segment of the submitted output prefix, of
/// the metadata document that job's output is addressed through.
///
/// A client discovers a finished job's results by reading this document — the documented
/// flow is to poll the status and then read the job metadata the reported location names,
/// which is what indexes the per-asset result files — so the location
/// `GetDataAutomationStatus` reports is this object's
/// ([`Invocation::output_metadata_s3_uri`]) and not the bare prefix the submission
/// carried, which names no job.
const JOB_METADATA_OBJECT: &str = "job_metadata.json";

/// The `AutomationJobStatus` enum, as the model declares its values.
///
/// Held as an enum rather than a string so the clock-driven progression in
/// [`Invocation::advance_to`] cannot drift onto a value the model does not declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationJobStatus {
    /// The submission was accepted and the job has not started yet.
    Created,
    /// The job is running.
    InProgress,
    /// The job finished successfully.
    Success,
    /// The job failed for a reason attributable to the caller's request.
    ClientError,
    /// The job failed inside the service.
    ServiceError,
}

impl AutomationJobStatus {
    /// The wire value, exactly as `AUTOMATION_JOB_STATUS_VALUES` lists it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ClientError => "ClientError",
            Self::Created => "Created",
            Self::InProgress => "InProgress",
            Self::ServiceError => "ServiceError",
            Self::Success => "Success",
        }
    }
}

/// One asynchronous data-automation invocation: everything the submit call accepted,
/// plus the job's own lifecycle.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The ARN this invocation is addressed by — the key it is stored under.
    pub invocation_arn: String,
    /// The opaque id in the ARN's resource portion.
    pub invocation_id: String,
    /// `inputConfiguration.s3Uri` as submitted.
    pub input_s3_uri: String,
    /// `inputConfiguration.assetProcessingConfiguration` as submitted: the processing
    /// scope this job was asked to work on, and `None` when the submission asked for the
    /// whole asset. Retained so the accepted job's scope is part of its record rather
    /// than being dropped at the handler boundary — reach the segment it delimits through
    /// [`Invocation::requested_timestamp_segment`].
    pub asset_processing_configuration: Option<AssetProcessingConfiguration>,
    /// `outputConfiguration.s3Uri` as submitted: the PREFIX this job's output is written
    /// under, which is what a submission names and what several jobs may share. It is
    /// NOT what a reader reports — that is this job's own metadata location under it
    /// ([`Invocation::output_metadata_s3_uri`]).
    pub output_s3_uri: String,
    /// `dataAutomationConfiguration.dataAutomationProjectArn`, when one was given.
    pub data_automation_project_arn: Option<String>,
    /// `dataAutomationConfiguration.stage` (a `DataAutomationStage` value).
    pub stage: Option<String>,
    /// `dataAutomationProfileArn` as submitted (required by the model).
    pub data_automation_profile_arn: String,
    /// `blueprints` as submitted; empty when the member was absent.
    pub blueprints: Vec<Blueprint>,
    /// `encryptionConfiguration` as submitted.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `notificationConfiguration` as submitted.
    pub notification_configuration: Option<NotificationConfiguration>,
    /// Where the job is in its lifecycle. Advanced only by [`Invocation::advance_to`].
    pub status: AutomationJobStatus,
    /// `errorType`, set only on a failure status, and only by the terminal transition
    /// that attributed the failure ([`Invocation::advance_to`]).
    pub error_type: Option<String>,
    /// `errorMessage`, set beside [`Invocation::error_type`] by the same transition and
    /// for the same reason.
    pub error_message: Option<String>,
    /// When the submission was accepted.
    pub job_submission_time: SystemTime,
    /// When the job reached a terminal status; `None` while it is still running.
    pub job_completion_time: Option<SystemTime>,
    /// The resource's tags, keyed by tag key — the map `TagResource` merges into and
    /// `UntagResource` removes from. A `BTreeMap` so `ListTagsForResource` answers in a
    /// stable order.
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` this job was submitted under, when the caller sent one.
    ///
    /// Retained as part of what the submission carried, and read by nothing: the token is
    /// accepted for request validation and is NOT an application-level deduplication key
    /// for this operation, so two submissions under one token are two independent jobs.
    pub client_token: Option<String>,
}

impl Invocation {
    /// A job accepted at `job_submission_time`, in its initial `Created` status, with
    /// every optional member absent. The caller fills in the optional members it was
    /// given.
    pub fn new(
        invocation_arn: String,
        invocation_id: String,
        input_s3_uri: String,
        output_s3_uri: String,
        data_automation_profile_arn: String,
        job_submission_time: SystemTime,
    ) -> Self {
        Self {
            invocation_arn,
            invocation_id,
            input_s3_uri,
            asset_processing_configuration: None,
            output_s3_uri,
            data_automation_project_arn: None,
            stage: None,
            data_automation_profile_arn,
            blueprints: Vec::new(),
            encryption_configuration: None,
            notification_configuration: None,
            status: AutomationJobStatus::Created,
            error_type: None,
            error_message: None,
            job_submission_time,
            job_completion_time: None,
            tags: BTreeMap::new(),
            client_token: None,
        }
    }

    /// Materialise every lifecycle transition the clock already owes this job at `now`,
    /// settling the terminal one on `asset` — what the job established about the asset it
    /// was asked to process.
    ///
    /// The transitions are owed by the passage of time since the submission that
    /// started them, not by anyone reading the job, so applying them from a reporting
    /// operation is what keeps repeated reads consistent: the first read that finds a
    /// transition due records it, and every later read sees the same status and the same
    /// `job_completion_time`. The completion instant stored is the instant the
    /// transition became DUE, not the instant it was observed, so the derived
    /// `jobDurationInSeconds` does not depend on when a caller happened to poll.
    ///
    /// A job's work begins with READING the asset `inputConfiguration.s3Uri` names and
    /// goes on to PROCESS what it read, so a job pointed at an asset the object store
    /// holds none of, or at one it cannot process, cannot carry out the work it was
    /// accepted for: it ends in the modelled failure status attributable to the request
    /// (`ClientError`) carrying the `errorType` of that failure class and the fixed
    /// `errorMessage` for it, and never in `Success`. A job whose asset IS there and
    /// processable, or about which nothing could be established ([`InputAsset`]),
    /// completes.
    ///
    /// What `Success` CLAIMS here: the invocation RECORD reached its terminal state, with
    /// the asset it was pointed at in place AND what the job extracted from that asset
    /// written into the submitted output location ([`job_output`]) — the standard output of
    /// the asset's segment and the job-metadata document that indexes it, both already in the
    /// object store by the time this transition is materialised, so a client that polls to
    /// `Success` and then reads the reported location finds the objects there. What is
    /// extracted is what this process can really read out of the asset
    /// ([`asset_extraction`]): nothing is synthesised, so a modality it extracts nothing from
    /// has its metadata written and no representation.
    pub fn advance_to(&mut self, now: SystemTime, asset: InputAsset) {
        if self.status == AutomationJobStatus::Created
            && now >= self.job_submission_time + CREATED_FOR
        {
            self.status = AutomationJobStatus::InProgress;
        }
        if self.status == AutomationJobStatus::InProgress {
            let completion_due = self.completion_due();
            if now >= completion_due {
                // Each failure class carries its OWN `errorType` and its own fixed
                // message, stated where that class is decided; nothing in either member
                // is spelled from the request.
                self.status = match asset {
                    InputAsset::Available => AutomationJobStatus::Success,
                    InputAsset::Absent => {
                        self.error_type = Some(INPUT_ASSET_ABSENT_ERROR_TYPE.to_string());
                        self.error_message = Some(INPUT_ASSET_ABSENT_MESSAGE.to_string());
                        AutomationJobStatus::ClientError
                    }
                    InputAsset::InvalidImage => {
                        self.error_type =
                            Some(input_image::INVALID_IMAGE_ERROR_TYPE.to_string());
                        self.error_message =
                            Some(input_image::INVALID_IMAGE_MESSAGE.to_string());
                        AutomationJobStatus::ClientError
                    }
                };
                self.job_completion_time = Some(completion_due);
            }
        }
    }

    /// Whether this submission asked for its lifecycle to be published to EventBridge:
    /// `notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled`.
    ///
    /// Both levels are `@required` inside the member that carries them, so a submission
    /// that sent no `notificationConfiguration` asked for nothing.
    pub fn event_bridge_enabled(&self) -> bool {
        self.notification_configuration
            .as_ref()
            .is_some_and(|configuration| {
                configuration.event_bridge_configuration.event_bridge_enabled
            })
    }

    /// When this job's terminal transition falls due: one `Created` span and one
    /// `InProgress` span after the submission was accepted
    /// ([`CREATED_FOR`] + [`IN_PROGRESS_FOR`]).
    pub fn completion_due(&self) -> SystemTime {
        self.job_submission_time + CREATED_FOR + IN_PROGRESS_FOR
    }

    /// Whether the clock already owes this job its terminal transition and that
    /// transition has not been materialised yet — so the outcome still has to be settled
    /// ([`Invocation::advance_to`]).
    ///
    /// What a reporting read asks BEFORE it goes looking for the job's asset: a job whose
    /// terminal transition is not due yet, and one that already reached a terminal status,
    /// both need no answer about the asset, so neither costs a read of the object store.
    pub fn terminal_transition_due(&self, now: SystemTime) -> bool {
        !self.is_terminal() && now >= self.completion_due()
    }

    /// Whether the job has reached a status it can never leave. Only a terminal job is
    /// a candidate for eviction: dropping one still running would lose a job a caller is
    /// waiting on.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            AutomationJobStatus::Success
                | AutomationJobStatus::ClientError
                | AutomationJobStatus::ServiceError
        )
    }

    /// Whether the job has reached a terminal status, or the clock has already made one
    /// due for it — the question eviction asks, which it must answer WITHOUT settling the
    /// outcome: which terminal status a finished job carries rests on what its asset
    /// established, and the admission path holds no answer about that.
    pub fn is_terminal_at(&self, now: SystemTime) -> bool {
        self.is_terminal() || self.terminal_transition_due(now)
    }

    /// Where this job's own metadata document sits: the submitted output prefix, this
    /// job's `invocationId` under it, and [`JOB_METADATA_OBJECT`].
    ///
    /// The prefix a submission names is shared by every job submitted under it, so one
    /// job's output lives under a segment of its OWN id and the metadata document at the
    /// top of that segment is what the job's results are addressed through. The two are
    /// joined by a single key separator, so a prefix written with a trailing separator, a
    /// prefix written without one, and a URI naming the bucket root all address the same
    /// location.
    ///
    /// Derived on every read and never stored: it is a function of two values the record
    /// already holds.
    pub fn output_metadata_s3_uri(&self) -> String {
        match job_output::output_location(&self.output_s3_uri) {
            // The one layout the job's own output is written under, so the location reported
            // and the location written cannot drift apart ([`job_output`]).
            Some((bucket, prefix)) => format!(
                "s3://{bucket}/{}",
                job_output::job_metadata_key(prefix, &self.invocation_id)
            ),
            // A submitted value that addresses no bucket names no location to write into —
            // the modelled `S3Uri` pattern does not admit one — so the two values the record
            // holds are joined as they stand and the reported location still names this job.
            None => format!(
                "{}/{}/{JOB_METADATA_OBJECT}",
                self.output_s3_uri.trim_end_matches('/'),
                self.invocation_id
            ),
        }
    }

    /// `jobDurationInSeconds`: whole seconds between the two stored timestamps, and
    /// `None` while the job has no completion time. Derived on every read, never stored.
    ///
    /// The emulated lifecycle spans ([`CREATED_FOR`] + [`IN_PROGRESS_FOR`]) total under a
    /// second, so a successful job here reports `0`: the value is consistent with the two
    /// timestamps it is derived from, and is not a duration to assert a magnitude on.
    pub fn job_duration_in_seconds(&self) -> Option<i32> {
        let completion = self.job_completion_time?;
        let elapsed = completion.duration_since(self.job_submission_time).ok()?;
        i32::try_from(elapsed.as_secs()).ok()
    }

    /// The timestamp segment this job was submitted with, or `None` when it was
    /// submitted without one (the whole asset).
    pub fn requested_timestamp_segment(&self) -> Option<&TimestampSegment> {
        requested_timestamp_segment(self.asset_processing_configuration.as_ref())
    }

    /// The tag map as the modelled `list<Tag>`, in tag-key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice. A handler reaches it through [`service_state`].
///
/// Every field is private, and the only way to add an invocation is
/// [`Self::admit_invocation`], which inserts one only after it has freed a slot. That is
/// what bounds this store: `invocations` can never exceed [`MAX_INVOCATIONS`], and the
/// queue of events waiting to be published is bounded in its own module
/// ([`job_events::MAX_PENDING_EVENTS`]). A handler reaches a record through
/// [`Self::resolve_invocation`] / [`Self::resolve_invocation_mut`] — which is also where
/// an ARN naming no record earns its refusal — or through the bare
/// [`Self::invocation`] / [`Self::invocation_mut`] lookups those are built on; none of
/// them can grow anything.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState {
    /// Every invocation this emulator has minted, keyed by its `invocationArn`. Bounded
    /// at [`MAX_INVOCATIONS`] by [`Self::admit_invocation`].
    invocations: HashMap<String, Invocation>,
    /// The lifecycle events of the jobs whose submission asked for them, waiting to be
    /// published onto the account's default EventBridge bus ([`job_events`]).
    pending_events: VecDeque<job_events::JobEvent>,
    /// How many settles started by a submission are in flight, bounded at
    /// [`job_events::MAX_SETTLES_IN_FLIGHT`] by [`Self::try_begin_settle`] — which is what
    /// stops a stream of notified submissions from having unbounded work outstanding.
    settles_in_flight: usize,
    /// How many invocations this store has ever minted an id for — the monotonic half of
    /// [`Self::mint_invocation_id`]. It counts mints, not retained records, so it keeps
    /// rising past an eviction and an id is never handed out twice.
    minted_invocations: u64,
}

impl ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState {
    /// Mint the resource id for one new invocation: the id of a data automation job,
    /// which this service names with a UUID in its canonical lower-case `8-4-4-4-12`
    /// form — the same id the job's output lands under
    /// ([`Invocation::output_metadata_s3_uri`]).
    ///
    /// The 16 bytes it is laid out from are a digest of this store's own monotonic mint
    /// count and the submission instant, and of NOTHING the request carries: two
    /// identical payloads submitted under different tokens therefore get different ids,
    /// the count means an id is never reused even after the record it named was retired,
    /// and the digest diffuses both inputs, so consecutive ids share no field rather than
    /// differing only in a counter. The version and variant bits and the hyphenated text
    /// come from [`::service_common::uuid_from_random_bytes`], this tree's one rendering
    /// of 16 opaque bytes as a UUID, so a client that parses the id out of the ARN gets a
    /// UUID; lower-case hex and hyphens keep it inside the modelled `InvocationArn`
    /// pattern's `[a-zA-Z0-9-_]+` resource segment, and its fixed 36-character width is
    /// what [`arn_region`] bounds the ARN's region segment against, so the whole ARN
    /// stays inside its `@length(1..128)`.
    pub fn mint_invocation_id(&mut self, now: SystemTime) -> String {
        use ::sha2::Digest as _;

        self.minted_invocations = self.minted_invocations.wrapping_add(1);
        let since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut digest = ::sha2::Sha256::new();
        digest.update(self.minted_invocations.to_be_bytes());
        digest.update(since_epoch.to_be_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest.finalize()[..16]);
        ::service_common::uuid_from_random_bytes(bytes)
    }

    /// The invocation `invocation_arn` names, for a reporting read.
    pub fn invocation(&self, invocation_arn: &str) -> Option<&Invocation> {
        self.invocations.get(invocation_arn)
    }

    /// Resolve `invocation_arn` for a reporting read, or the refusal it earns when this
    /// store retains no such invocation ([`unknown_invocation_refusal`]).
    ///
    /// The resolve-or-refuse step every ARN-keyed operation here starts with, so each one
    /// writes `?` over it rather than restating the refusal.
    pub fn resolve_invocation<E>(
        &self,
        invocation_arn: &str,
        not_found: impl FnOnce(ResourceNotFoundException) -> E,
    ) -> Result<&Invocation, crate::scaffolded::Scaffolded<E>> {
        self.invocation(invocation_arn)
            .ok_or_else(|| unknown_invocation_refusal(not_found))
    }

    /// [`Self::resolve_invocation`] for a caller that edits the record it resolves in
    /// place, so the two differ only in the borrow they hand back and not in the refusal.
    pub fn resolve_invocation_mut<E>(
        &mut self,
        invocation_arn: &str,
        not_found: impl FnOnce(ResourceNotFoundException) -> E,
    ) -> Result<&mut Invocation, crate::scaffolded::Scaffolded<E>> {
        self.invocation_mut(invocation_arn)
            .ok_or_else(|| unknown_invocation_refusal(not_found))
    }

    /// The invocation `invocation_arn` names, for a caller that edits the record in
    /// place (a tag edit, or materialising a transition the clock owes). Reaching an
    /// existing record can never grow the store.
    pub fn invocation_mut(&mut self, invocation_arn: &str) -> Option<&mut Invocation> {
        self.invocations.get_mut(invocation_arn)
    }

    /// How many invocations are retained; at most [`MAX_INVOCATIONS`].
    pub fn invocation_count(&self) -> usize {
        self.invocations.len()
    }

    /// Materialise the lifecycle transitions the clock owes the job `invocation_arn`
    /// names at `now`, settling the terminal one on `asset`, and QUEUE the event a
    /// transition this submission asked to be notified of earns.
    ///
    /// The one path that advances a job: it is what keeps the record and the events
    /// reporting it in step — a transition is queued by the very call that materialised
    /// it, exactly once, because the materialisation happens here and nowhere else
    /// ([`Invocation::advance_to`] is what it drives). `None` when this store retains no
    /// such invocation.
    pub fn settle_invocation(
        &mut self,
        invocation_arn: &str,
        now: SystemTime,
        asset: InputAsset,
    ) -> Option<&Invocation> {
        let invocation = self.invocations.get_mut(invocation_arn)?;
        let before = invocation.status;
        invocation.advance_to(now, asset);
        let moved_to_terminal = invocation.is_terminal() && before != invocation.status;
        let event = (moved_to_terminal && invocation.event_bridge_enabled())
            .then(|| job_events::JobEvent::of(invocation));
        if let Some(event) = event {
            self.queue_job_event(event);
        }
        self.invocations.get(invocation_arn)
    }

    /// Queue one lifecycle event for publication, dropping the oldest when the queue is
    /// full — see [`job_events::MAX_PENDING_EVENTS`] for why it is bounded.
    pub fn queue_job_event(&mut self, event: job_events::JobEvent) {
        while self.pending_events.len() >= job_events::MAX_PENDING_EVENTS {
            self.pending_events.pop_front();
            ::tracing::warn!(
                "the queue of data automation job events waiting to be published to \
                 EventBridge is full at {}; the oldest event was dropped",
                job_events::MAX_PENDING_EVENTS
            );
        }
        self.pending_events.push_back(event);
    }

    /// Take at most `limit` of the events not yet published, oldest first.
    ///
    /// Taking them out under the lock is what stops two concurrent calls from publishing
    /// one event twice.
    pub fn take_job_events(&mut self, limit: usize) -> Vec<job_events::JobEvent> {
        let taking = limit.min(self.pending_events.len());
        self.pending_events.drain(..taking).collect()
    }

    /// How many events are queued for publication — reporting only, for a reader that has
    /// to tell "nothing moved" from "nothing was published".
    pub fn pending_job_event_count(&self) -> usize {
        self.pending_events.len()
    }

    /// Take one of the slots for a settle a submission starts on its job's behalf, or
    /// `false` when [`job_events::MAX_SETTLES_IN_FLIGHT`] of them are already running.
    ///
    /// This is what bounds the work a stream of notified submissions can have in flight at
    /// once: a submission that cannot take a slot starts nothing, and its job's outcome is
    /// settled — and so published — by the next read that asks for it, exactly as a job
    /// nobody asked to be notified about is.
    pub fn try_begin_settle(&mut self) -> bool {
        if self.settles_in_flight >= job_events::MAX_SETTLES_IN_FLIGHT {
            return false;
        }
        self.settles_in_flight += 1;
        true
    }

    /// Give back the slot [`Self::try_begin_settle`] took, however the settle ended.
    pub fn finish_settle(&mut self) {
        self.settles_in_flight = self.settles_in_flight.saturating_sub(1);
    }

    /// How many such settles are in flight; at most
    /// [`job_events::MAX_SETTLES_IN_FLIGHT`].
    pub fn settles_in_flight(&self) -> usize {
        self.settles_in_flight
    }

    /// Admit one freshly minted invocation.
    ///
    /// This is the ONLY way an invocation is added to this store. It first frees a slot,
    /// bounding `invocations` at [`MAX_INVOCATIONS`]; `false` means every retained job
    /// is still running, so the caller must refuse the submission with the modelled
    /// quota error rather than drop work somebody is waiting on — and nothing at all is
    /// inserted, so a refused submission leaves no record behind.
    ///
    /// The record's `clientToken` is retained ON the record and indexed nowhere: this
    /// operation accepts the token for request validation and does NOT deduplicate on
    /// it, so a repeated token mints its own job and its own ARN exactly as a fresh one
    /// does.
    pub fn admit_invocation(&mut self, invocation: Invocation, now: SystemTime) -> bool {
        if !self.reserve_invocation_slot(now) {
            return false;
        }
        self.invocations
            .insert(invocation.invocation_arn.clone(), invocation);
        debug_assert!(
            self.invocations.len() <= MAX_INVOCATIONS,
            "the invocation map stays bounded by MAX_INVOCATIONS"
        );
        true
    }

    /// Make room for one more invocation, bounding the store at [`MAX_INVOCATIONS`].
    ///
    /// At capacity it evicts the longest-submitted job that has FINISHED — one already in
    /// a terminal status, or one whose terminal transition the clock has already made due
    /// ([`Invocation::is_terminal_at`]) — repeating until there is room. `false` means
    /// every retained job is still running. Nothing here MATERIALISES that transition:
    /// which terminal status a finished job carries rests on what its asset established,
    /// which only a settle that asks the object store can decide, so this
    /// admission path reads the clock without writing an outcome it cannot ground.
    /// Private: admission goes through [`Self::admit_invocation`], so freeing a slot and
    /// filling it cannot come apart.
    fn reserve_invocation_slot(&mut self, now: SystemTime) -> bool {
        while self.invocations.len() >= MAX_INVOCATIONS {
            let evictable = self
                .invocations
                .values()
                .filter(|invocation| invocation.is_terminal_at(now))
                .map(|invocation| {
                    (
                        invocation.job_submission_time,
                        invocation.invocation_arn.clone(),
                    )
                })
                .min_by_key(|(submitted, _)| *submitted);
            match evictable {
                Some((_, arn)) => self.forget_invocation(&arn),
                None => return false,
            }
        }
        true
    }

    /// Drop one invocation. A job nobody can resolve any more reports nothing, so the
    /// events it had queued are left to the drain that was going to publish them: a
    /// queued event carries the job's own id and is rendered from it
    /// ([`job_events::JobEvent`]), not from the record it came off.
    pub fn forget_invocation(&mut self, invocation_arn: &str) {
        self.invocations.remove(invocation_arn);
    }
}

/// The region segment an ARN this service MINTS may carry: `candidate` when it is a
/// well-formed AWS region label, else [`DEFAULT_REGION`].
///
/// A minted ARN is returned in — and read back through — members the model constrains
/// (`InvocationArn`, `@length(1..128)` and its pattern; `TaggableResourceArn`), so the
/// region segment spliced into it has to leave the whole value inside those bounds: an
/// ARN this service hands out but its own input members refuse would name a record no
/// caller could ever address. The label is therefore read through the shared
/// `service_common::arn::region_label` grammar — non-empty, at most 20 characters, and
/// composed only of `[a-z0-9-]`, the form every model's `*Arn` pattern states for the
/// field — which is what bounds the result: the fixed part of an invocation ARN
/// (`arn:aws:bedrock:` + account + [`INVOCATION_RESOURCE_PREFIX`]) plus the 36-character
/// UUID [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::mint_invocation_id`]
/// produces is 93 characters, so a region label within that grammar can never push the
/// ARN past 128. Anything wider names no region an endpoint has, so it is not spliced
/// into an ARN at all and the service's own region answers instead.
pub fn arn_region(candidate: &str) -> &str {
    ::service_common::arn::region_label(candidate).unwrap_or(DEFAULT_REGION)
}

/// The ARN of an invocation with resource id `invocation_id` in `region`.
///
/// Built through `emulator_server::emulator_arn`, so the account segment is the one
/// canonical emulator account id. The resource id is never request-supplied — the only
/// caller mints it with
/// [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::mint_invocation_id`],
/// whose canonical UUID text is inside the modelled `InvocationArn` pattern's
/// `[a-zA-Z0-9-_]+` resource segment — and `region` is reduced to a region label by
/// [`arn_region`] HERE, at the one place an invocation ARN is built, so the result
/// satisfies the modelled pattern and its `@length(1..128)` whatever a caller passes.
pub fn invocation_arn(region: &str, invocation_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        arn_region(region),
        &format!("{INVOCATION_RESOURCE_PREFIX}{invocation_id}"),
    )
}

/// The region to mint an ARN in for this request: the SigV4 credential scope's region,
/// else [`DEFAULT_REGION`].
///
/// The scope region is accepted only when [`arn_region`] reports it as a region label an
/// ARN can carry; anything else — absent, empty, or outside that grammar — falls back to
/// [`DEFAULT_REGION`] rather than being spliced into an ARN. This is not a check that the
/// label names a region AWS has deployed: the emulator serves whatever region a caller
/// signs for, and the only property a minted ARN needs is that it stays inside the
/// members the model constrains it by.
pub fn request_region(parts: &::http::request::Parts) -> String {
    let signed =
        ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region);
    arn_region(signed.as_deref().unwrap_or_default()).to_string()
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine — which is what
/// [`store_or_unavailable_refusal`] does, so a handler reaches for that rather than for
/// this directly.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState>)
}

/// What this service answers when the engine it is mounted on holds no store of this
/// service's type — the one message behind `InternalServerException` for every operation
/// here.
///
/// It is the service's own fixed string and carries NOTHING from the request: the failure
/// is about the engine, not about anything the caller sent. Kept in one place so the five
/// handlers cannot drift apart — through [`store_or_unavailable_refusal`], which is the
/// only thing that reads it.
pub const STORE_UNAVAILABLE_MESSAGE: &str =
    "the data automation runtime store is unavailable on this engine";

/// This service's store, or the whole refusal its absence earns: the
/// `InternalServerException` shape carrying [`STORE_UNAVAILABLE_MESSAGE`], answered with
/// the status that shape's own `smithy.api#httpError` trait declares
/// ([`crate::scaffolded::Scaffolded::Modeled`]).
///
/// `internal_error` is the calling operation's own `InternalServerException` variant — the
/// shape itself is the one generated struct every operation here shares, so the variant
/// constructor is all that differs between callers. EVERYTHING else about the refusal
/// lives here once: the shape, the message and the status. That is the same reason the
/// ARN resolution refusal is single-homed in [`unknown_invocation_refusal`]: the
/// operations that reach this service's store cannot come to disagree about what one
/// engine holding no such store answers.
pub fn store_or_unavailable_refusal<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    internal_error: impl FnOnce(InternalServerException) -> E,
) -> Result<
    ::engine::ServiceState<ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState>,
    crate::scaffolded::Scaffolded<E>,
> {
    service_state(engine).ok_or_else(|| {
        crate::scaffolded::Scaffolded::Modeled(internal_error(InternalServerException {
            message: Some(STORE_UNAVAILABLE_MESSAGE.to_string()),
        }))
    })
}
