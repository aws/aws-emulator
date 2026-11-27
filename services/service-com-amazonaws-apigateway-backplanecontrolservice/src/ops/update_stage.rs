// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStage
//! IMPLEMENTED: applies the request's `patchOperations` to the stored `Stage` record of
//! `restApiId`/`stageName`, in request order, and reports the updated stage — so a later
//! `GetStage` reads back exactly what the patch set.
//!
//! The whole list is applied to a COPY of the record and written back only once every op
//! has succeeded, so a list carrying one unsupported op leaves the stored stage as it was
//! rather than half-patched. The one refusal that is NOT a refusal of the request but of the
//! stage the request LEAVES — the duplicate method settings [`duplicate_method_settings`]
//! words — keeps what the list applied, which is how a stage comes to carry the two entries
//! that sentence asks the caller to remove one of.
//!
//! Enabling, resizing or disabling the stage's cache cluster is a transition the CLOCK
//! owes: the due instant is stored on the record (see [`crate::state::StageEntry`]) and
//! `cacheClusterStatus` is computed from it as of the instant this call is answered, so
//! THIS answer reports the `CREATE_IN_PROGRESS` or `DELETE_IN_PROGRESS` it accepted and a
//! later call reports what the clock has since made of it.
//!
//! Turning a method's EXECUTION logging on is gated on the account rather than on the
//! stage: API Gateway writes those log events under the account-level CloudWatch Logs
//! role, so a patch that sets a logging level other than off, or turns the full
//! request/response data trace on, is refused while the account singleton carries no role
//! — see [`ACCOUNT_CLOUDWATCH_ROLE_REQUIRED`].
//!
//! What an `op` means, how a `path` is read and how a single-valued member, a map
//! member or a bounded number is edited are the service's shared patch semantics
//! (`crate::state`'s patch section); this file only maps each supported path to the
//! member of a stage it names.
//!
//! Two paths of this surface are not a plain member of the `Stage` shape:
//!
//! * the per-method settings. A method-setting path is the reference tokens naming the method
//!   followed by the setting it writes (`/~1pets/GET/metrics/enabled`), and the KEY the stage
//!   holds that method's settings under is the method path those leading tokens SPELL —
//!   `{resource_path}/{http_method}` or `*/*` as `Stage$methodSettings` documents it, written
//!   exactly as the request wrote it, so a client reads a setting back under the path it
//!   patched. The resource half carries its separators escaped `~1` the way
//!   `PatchOperation$path` requires of a path name (`~1pets/GET`), with its LEADING separator
//!   either escaped too or left to the pointer's own (`//pets/GET/…`, the spelling
//!   `MethodSetting$resourcePath` states a path in), and a pointer that escapes the separator
//!   before the method too names the method in ONE token, whose LAST escape is that separator,
//!   so it is keyed as the method path that token SPELLS (`~1pets/GET`) — the same key the
//!   pointer that gives the method its own token writes. Two spellings of the RESOURCE half are
//!   two entries, and a stage carrying both cannot be updated — see
//!   [`apply_method_setting_patch`], [`method_path_spellings`], [`duplicate_method_settings`]
//!   and [`crate::state::method_of_settings_key`], which is what a key DENOTES and what the
//!   data plane resolves a served call's settings by.
//! * a pointer that is no member of the `Stage` shape is read as such a method path, so
//!   it is refused as one — see [`STAGE_INVALID_PATCH_PATH`].
//! * `accessLogSettings` and `canarySettings`, structures whose own members are
//!   addressed the way a top-level member is, and which a `remove` of the structure
//!   itself turns off.
//!
//! A `copy` is the canary PROMOTION rather than a patch of a member, so its legal
//! `(path, from)` pairs are a grammar of their own — see [`CANARY_PROMOTIONS`].
//!
//! An `add` names one entry of a collection the stage keys by name and nothing else: every
//! other pointer of a stage names a member that is already there, which is `replace`d, so
//! an `add` of one is refused — see [`add_names_an_entry`]. A `remove` takes away the parts
//! of a stage that can go away — a method's settings, a nested structure, an entry of a
//! collection the stage keys by name — and the one pointer TEMPLATE it publishes is the
//! method path, so a pointer that can name none of those parts is refused with that template
//! and leaves the stage as it was; a pointer that names a method and no settings of it is
//! read against the settings the stage carries — see [`remove_names_no_part_of_a_stage`] and
//! [`remove_method_setting`].
//! A patch `value` for a boolean member is READ rather than validated: see [`stage_bool`].
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStage`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")
//! ```
//!
//! Changes information about a Stage resource.
//!
//! ## Input
//!
//! * `patchOperations` (ListOfPatchOperation) — For more information about supported patch
//!   operations, see Patch Operations.
//! * `restApiId` (String) **required** — The string identifier of the associated RestApi.
//! * `stageName` (String) **required** — The name of the Stage resource to change information
//!   about.
//!
//! ## Output
//!
//! * `accessLogSettings` (AccessLogSettings) — Settings for logging access in this stage.
//! * `cacheClusterEnabled` (Boolean, default false) **required** — Specifies whether a cache
//!   cluster is enabled for the stage. To activate a method-level cache, set CachingEnabled
//!   to true for a method.
//! * `cacheClusterSize` (CacheClusterSize, one of: 0.5 | 1.6 | 118 | 13.5 | 237 | 28.4 | 58.2
//!   | 6.1) — The stage's cache capacity in GB. For more information about choosing a cache
//!   size, see Enabling API caching to enhance responsiveness.
//! * `cacheClusterStatus` (CacheClusterStatus, one of: AVAILABLE | CREATE_IN_PROGRESS |
//!   DELETE_IN_PROGRESS | FLUSH_IN_PROGRESS | NOT_AVAILABLE) — The status of the cache
//!   cluster for the stage, if enabled.
//! * `canarySettings` (CanarySettings) — Settings for the canary deployment in this stage.
//! * `clientCertificateId` (String) — The identifier of a client certificate for an API
//!   stage.
//! * `createdDate` (Timestamp) — The timestamp when the stage was created.
//! * `deploymentId` (String) — The identifier of the Deployment that the stage points to.
//! * `description` (String) — The stage's description.
//! * `documentationVersion` (String) — The version of the associated API documentation.
//! * `lastUpdatedDate` (Timestamp) — The timestamp when the stage last updated.
//! * `methodSettings` (MapOfMethodSettings) — A map that defines the method settings for a
//!   Stage resource. Keys (designated as /{method_setting_key below) are method paths defined
//!   as {resource_path}/{http_method} for an individual method override, or /\*/\* for
//!   overriding all methods in the stage.
//! * `stageName` (String) — The name of the stage is the first path segment in the Uniform
//!   Resource Identifier (URI) of a call to API Gateway. Stage names can only contain
//!   alphanumeric characters, hyphens, and underscores. Maximum length is 128 characters.
//! * `tags` (MapOfStringToString) — The collection of tags. Each tag element is associated
//!   with a given resource.
//! * `tracingEnabled` (Boolean, default false) **required** — Specifies whether active
//!   tracing with X-ray is enabled for the Stage.
//! * `variables` (MapOfStringToString) — A map that defines the stage variables for a Stage
//!   resource. Variable names can have alphanumeric and underscore characters, and the values
//!   must match [A-Za-z0-9-._~:/?#&=,]+.
//! * `webAclArn` (String) — The ARN of the WebAcl associated with the Stage.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The submitted request is not valid, for example, the
//!   input is incomplete or incorrect. See the accompanying error message for details.
//! * `ConflictException` (HTTP 409) — The request configuration has conflicts. For details,
//!   see the accompanying error message.
//! * `LimitExceededException` (HTTP 429) — The request exceeded the rate limit. Retry after
//!   the specified time period.
//! * `NotFoundException` (HTTP 404) — The requested resource is not found. Make sure that the
//!   request URI is correct.
//! * `TooManyRequestsException` (HTTP 429) — The request has reached its throttling limit.
//!   Retry after the specified time period.
//! * `UnauthorizedException` (HTTP 401) — The request is denied because the caller has
//!   insufficient permissions.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{
    patch_enum_string_refused_with, patch_f64_checked,
    patch_f64_refused_with, patch_i32_refused_with,
    patch_optional_string, patch_string_map, same_method, Patch, PatchFailure, PatchOpKind,
    StageEntry, EVERY_RESOURCE, MAX_COLLECTION_ENTRIES, MAX_STRING_CHARS,
};

// The `path` values `UpdateStage` accepts are the ones `apply_patch` below has an arm
// for: one per updateable member of the `Stage` shape, the `canarySettings` and
// `accessLogSettings` sub-members and the per-method settings included. `stageName`,
// `createdDate`, `lastUpdatedDate` and `cacheClusterStatus` are assigned by the service,
// `tags` are changed through `TagResource`/`UntagResource` and `webAclArn` by associating
// a WebACL through AWS WAF, so none of them is patchable.

/// The pointer a canary promotion copies the deployment id FROM: "to promote a canary
/// deployment, you copy the canary deployment ID to the affiliated deployment ID by
/// calling a PATCH request on a Stage resource with "op":"copy",
/// "from":"/canarySettings/deploymentId" and "path":"/deploymentId""
/// (`PatchOperation$from`).
const CANARY_DEPLOYMENT_POINTER: &str = "/canarySettings/deploymentId";

/// The pointer a canary promotion copies the stage variable overrides FROM: the canary's
/// own `stageVariableOverrides`, "the stage variable overrides used for the canary release
/// deployment" (`CanarySettings$stageVariableOverrides`), which promoting the canary makes
/// the stage's own variables.
const CANARY_OVERRIDES_POINTER: &str = "/canarySettings/stageVariableOverrides";

/// The `copy` grammar of a stage: each pair is a `path` a `copy` may name beside the ONE
/// pointer that `copy`'s `from` may read it out of.
///
/// A `copy` on a stage is the canary promotion — the canary's deployment becomes the
/// stage's, and the canary's stage variable overrides become the stage's variables — so
/// the two sides are a PAIR rather than two independent sets: a `copy` whose path, whose
/// `from`, or whose combination of the two is not one of these pairs applies nothing, and
/// is refused with the sentence [`invalid_copy_operation`] states, which enumerates each
/// side of this table.
const CANARY_PROMOTIONS: &[(&str, &str)] = &[
    ("/deploymentId", CANARY_DEPLOYMENT_POINTER),
    ("/variables", CANARY_OVERRIDES_POINTER),
];

/// The `BadRequestException` message for an `op` a stage cannot apply to the pointer that
/// `op` named: the op and the path, both quoted back exactly as the request spelled them,
/// and nothing else.
///
/// This is the stage's sentence for an op that NAMES a pointer it cannot be applied to —
/// as opposed to one the resource takes no pointer for at all, which is
/// [`STAGE_INVALID_PATCH_OPERATION`] — and the `copy` grammar's refusal below opens with
/// the same text, so the two are built from this one template.
fn invalid_operation(op: PatchOpKind, path: &str) -> String {
    format!("Invalid {} operation with path: {path}", op.as_str())
}

/// The `BadRequestException` message for a `copy` outside the stage's copy grammar.
///
/// Fixed text states the two legal sets — the paths a `copy` may name and the pointers one
/// may read from, both constants of the grammar [`CANARY_PROMOTIONS`] holds, listed from
/// that one table so the sentence cannot offer a pair the stage would refuse — and the only
/// variable parts are the rejected operation's own `path` and `from`, quoted back exactly
/// as the request spelled them.
fn invalid_copy_operation(path: &str, from: &str) -> String {
    let paths: Vec<&str> = CANARY_PROMOTIONS.iter().map(|(path, _)| *path).collect();
    let sources: Vec<&str> = CANARY_PROMOTIONS.iter().map(|(_, from)| *from).collect();
    format!(
        "{} and from {from}. Valid copy:path are [{}] and valid copy:from are [{}]",
        invalid_operation(PatchOpKind::Copy, path),
        paths.join(", "),
        sources.join(", ")
    )
}

/// The `loggingLevel` that is execution logging turned OFF: the one value of the
/// documented set at which a method emits no execution log events at all.
const LOGGING_LEVEL_OFF: &str = "OFF";

/// The values `MethodSetting$loggingLevel` documents: "Valid values are OFF, ERROR, and
/// INFO." The member is a plain string on the wire, so its value set is checked here.
const LOGGING_LEVEL_VALUES: &[&str] = &[LOGGING_LEVEL_OFF, "ERROR", "INFO"];

/// The `BadRequestException` message for a `loggingLevel` outside the documented set.
///
/// API Gateway answers this condition with its own sentence for the SETTING rather than
/// with the generic out-of-set sentence a patch path earns: fixed text naming the setting
/// that was refused, and then the one variable part — the level the request asked for,
/// exactly as it spelled it.
fn invalid_logging_level(level: &str) -> String {
    format!("Invalid logging level specified: {level}")
}

/// The `BadRequestException` message API Gateway refuses a method setting's
/// `unauthorizedCacheControlHeaderStrategy` outside its declared value set with.
///
/// The strategy says "how to handle unauthorized requests for cache invalidation"
/// (`MethodSetting$unauthorizedCacheControlHeaderStrategy`), whose values the modelled
/// `UnauthorizedCacheControlHeaderStrategy` enum declares
/// ([`UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES`]) — a patch `value` is a bare string
/// on the wire, so the set is checked where the setting is written. API Gateway answers a
/// value outside it with its own sentence for the SETTING: entirely fixed text, naming
/// neither the rejected value, nor the method-setting path that carried it, nor the
/// strategies it would have taken — the wording the method settings' other refusals of a
/// value use ([`INVALID_THROTTLING_SETTING`]), rather than the generic out-of-set sentence a
/// patch path earns.
const INVALID_UNAUTHORIZED_CACHE_CONTROL_STRATEGY: &str =
    "Invalid unauthorized cache control header strategy";

/// The `BadRequestException` message API Gateway refuses a configuration that turns a
/// stage's execution logging on with while the account has no CloudWatch Logs role.
///
/// API Gateway writes a method's execution logs under the account-level role
/// (`Account$cloudwatchRoleArn`, "the ARN of an Amazon CloudWatch role for the current
/// Account"), so a stage cannot be configured to emit them before the account has one.
/// Entirely fixed text for this condition: it names neither the stage, the method scope
/// nor the setting the request carried.
const ACCOUNT_CLOUDWATCH_ROLE_REQUIRED: &str =
    "CloudWatch Logs role ARN must be set in account settings to enable logging";

/// Whether this patch of the method setting `setting` turns a method's EXECUTION logging
/// ON — the condition the account-level CloudWatch Logs role is a precondition of.
///
/// Execution logging is on for a method scope when its level is one of the documented
/// levels other than [`LOGGING_LEVEL_OFF`], and the full request/response data trace is
/// execution logging too, so either setting being turned on by an `add` or a `replace` is
/// a configuration that needs the role. Turning either off, removing it, and every other
/// method setting (metrics, throttling, caching) emit no execution logs and so are not
/// gated. Each value is read exactly as the applier below reads it, so a value the
/// setting would refuse anyway is left to its own refusal.
fn enables_execution_logging(patch: &Patch, setting: &str) -> bool {
    if !matches!(patch.op, PatchOpKind::Add | PatchOpKind::Replace) {
        return false;
    }
    match setting {
        "logging/loglevel" => patch.value.as_deref().is_some_and(|level| {
            level != LOGGING_LEVEL_OFF && LOGGING_LEVEL_VALUES.contains(&level)
        }),
        "logging/dataTrace" => stage_bool(patch).unwrap_or(false),
        _ => false,
    }
}

/// The boolean a stage's patch `value` spells.
///
/// A patch `value` is a bare string on the wire ("applicable for the add or replace
/// operation", `PatchOperation$value`), and a boolean member of a stage always carries one
/// of its two values, so reading one leaves no third answer for a text that spells
/// neither: the member is TRUE when the text spells `true` and FALSE for every other text,
/// which is what a recorded `cacheClusterEnabled` patch carrying a word shows API Gateway
/// answering — 200, with the flag reported off. Case and surrounding space are not part of
/// the spelling.
fn stage_bool(patch: &Patch) -> Result<bool, PatchFailure> {
    Ok(patch.require_value()?.trim().eq_ignore_ascii_case("true"))
}

/// Apply `patch` to a boolean member of a stage, reading the value the way
/// [`stage_bool`] reads one. `remove` is refused, as it is for every boolean member the
/// model declares with a default: such a member always has one of its two values.
fn patch_stage_bool(current: &mut bool, patch: &Patch) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = stage_bool(patch)?,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// The range of `MethodSetting$cacheTtlInSeconds`: API Gateway documents a default TTL of
/// 300 seconds and a maximum of 3600 seconds for a method cache, and 0 ("the higher the
/// TTL, the longer the response will be cached") disables caching.
///
/// `pub(crate)` for the DATA plane, which honours the same bound when it holds a response
/// for a stored TTL ([`crate::service::invoke`]): one range, so the setting a patch may
/// write and the span an entry may be held for cannot disagree.
pub(crate) const CACHE_TTL_RANGE: ::std::ops::RangeInclusive<i32> = 0..=3600;

/// The `BadRequestException` message for a `cacheTtlInSeconds` that is not a whole number
/// of seconds inside [`CACHE_TTL_RANGE`].
///
/// API Gateway answers this condition with its own sentence for the SETTING — the
/// time-to-live of a cached response — rather than with the generic bounds sentence a
/// patch path earns, and it states the rule a value has to meet: an integer no greater
/// than the documented maximum, which is read off that range so the sentence cannot name a
/// maximum the setting would refuse.
fn invalid_cache_ttl() -> String {
    format!(
        "Invalid time-to-live setting, must be an integer no greater than {}",
        CACHE_TTL_RANGE.end()
    )
}

/// Apply `patch` to a method setting's `cacheTtlInSeconds`: EVERY value the setting cannot
/// take — a text that is no integer as much as an integer outside [`CACHE_TTL_RANGE`] — is
/// refused with [`invalid_cache_ttl`], the one sentence API Gateway states that rule in.
/// `remove` is not reached here: a `remove` of a method setting is applied against the
/// settings the stage already carries.
fn patch_cache_ttl(current: &mut i32, patch: &Patch) -> Result<(), PatchFailure> {
    patch_i32_refused_with(
        current,
        patch,
        |seconds| CACHE_TTL_RANGE.contains(&seconds),
        invalid_cache_ttl,
    )
}

/// The `BadRequestException` message API Gateway refuses a method setting's THROTTLE limit
/// it cannot meter with.
///
/// A method's throttle is one setting of a stage — a steady-state rate and a burst
/// (`/{resourcePath}/{httpMethod}/throttling/rateLimit` and `…/burstLimit`) — and API
/// Gateway answers every value either limit cannot take with this one sentence: entirely
/// fixed text, naming neither the limit, nor the method-setting path the value arrived on,
/// nor the value itself. The limits a stage's method setting takes are the ones
/// [`crate::state::accepts_method_throttle_burst`] and
/// [`crate::state::accepts_method_throttle_rate`] state: a count and a rate of requests, so a
/// rate has to be a finite number and neither can be negative — except for the one value that
/// asks for NO throttle on the method ([`crate::state::UNTHROTTLED_LIMIT`]), which API Gateway
/// takes and reports back. API Gateway publishes DEFAULT account-level throttle limits that an
/// account can have raised, so no maximum is a contract and only those rules are refusals.
const INVALID_THROTTLING_SETTING: &str = "Invalid throttling setting";

/// The context variables an access log format may carry the REQUEST IDENTIFIER as, one of
/// which every format has to name.
///
/// `AccessLogSettings$format` states the requirement — "The format must include at least
/// `$context.requestId`" — and API Gateway accepts EITHER identifier of the request in its
/// place: `$context.extendedRequestId` is the same request's id under the member API
/// Gateway reports it as on the response, so a format naming it identifies the request the
/// line is about just as well. A recorded refusal of a format naming neither enumerates
/// both, which is what [`access_log_format_refused`] words: a caller told to add one
/// of two variables can satisfy the rule with either, where one told to add a single
/// variable may apply a fix that is still refused.
const ACCESS_LOG_REQUEST_IDENTIFIERS: &[&str] =
    &["$context.requestId", "$context.extendedRequestId"];

/// How API Gateway refuses an access log `format` that identifies the request by neither of
/// [`ACCESS_LOG_REQUEST_IDENTIFIERS`].
///
/// The sentence is the service's own, fixed text — "Access Log format must include either
/// &lt;identifiers&gt; : " — followed by the REJECTED format between single quotes, which is
/// the only variable part of it: the value the patch operation carried, written out exactly
/// as the caller wrote it so the caller can see which of its formats was refused. The
/// variables are named from [`ACCESS_LOG_REQUEST_IDENTIFIERS`] rather than written out, so
/// the sentence and the check can never name different ones.
fn access_log_format_refused(format: &str) -> String {
    format!(
        "Access Log format must include either {} : '{format}'",
        ACCESS_LOG_REQUEST_IDENTIFIERS.join(" or ")
    )
}

/// Whether `format` names one of [`ACCESS_LOG_REQUEST_IDENTIFIERS`] as a REFERENCE rather
/// than as the prefix of a longer name.
///
/// A `$context` reference ends where its member name ends, so a format naming
/// `$context.requestIdentity` names a different member and satisfies nothing: an occurrence
/// counts only when what follows it is not another character a member name may carry (a
/// letter, a digit or an underscore), which is also true at the very end of the format.
fn identifies_the_request(format: &str) -> bool {
    ACCESS_LOG_REQUEST_IDENTIFIERS.iter().any(|variable| {
        format.match_indices(variable).any(|(at, _)| {
            format[at + variable.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
        })
    })
}

/// The sentence API Gateway refuses an `op` a stage supports no path for with.
///
/// The ops a stage takes are `replace` (its single-valued members), `remove` (its optional
/// members and its nested structures) and `copy` (the canary promotion
/// [`CANARY_DEPLOYMENT_POINTER`] documents), and a recorded refusal enumerates exactly
/// those three, worded differently from the sentence the resources that take
/// `add`/`remove`/`replace` use ([`crate::state::INVALID_PATCH_OPERATION`]).
const STAGE_INVALID_PATCH_OPERATION: &str =
    "Invalid patch operation specified. Must be one of: 'replace', 'remove', 'copy'";

/// The pointers a STAGE publishes, in the order API Gateway's own refusal enumerates
/// them: the stage's own members, the two pointers of its access log settings, the
/// per-method settings spelled as the TEMPLATE the service publishes
/// (`/{resourcePath}/{httpMethod}/<group>/<setting>`, placeholders and all) and again for
/// the `/*/*` scope that stands for every method of the stage, and the collection it keys
/// by name.
///
/// This is FIXED TEXT of the refusal below, not a table the dispatch reads: which pointer a
/// stage applies is each arm of [`apply_stage_patch`] and of [`apply_method_setting_patch`],
/// and what the SERVICE prints is reproduced here as it prints it — quirks included. Two
/// entries of the recorded sentence run several pointers TOGETHER, with no separator at all
/// in the `{resourcePath}` family and a single space in the `/*/*` one, so each of those is
/// one element of this list rather than three; and the list carries no `/documentationVersion`
/// and no `/canarySettings` pointer, which this operation applies all the same
/// ([`apply_canary_patch`]) — the printed list and the dispatch are separate things.
/// and the enumeration is what the SERVICE prints.
///
/// Asserted end to end from a second statement of this text in
/// `tests/all/patch_entry_precedence.rs`, which keeps the assertion independent of this
/// constant; the two are edited together.
const STAGE_PATCH_PATHS: &[&str] = &[
    "/deploymentId",
    "/description",
    "/cacheClusterEnabled",
    "/cacheClusterSize",
    "/clientCertificateId",
    "/accessLogSettings",
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
    "/{resourcePath}/{httpMethod}/metrics/enabled",
    "/{resourcePath}/{httpMethod}/logging/dataTrace",
    "/{resourcePath}/{httpMethod}/logging/loglevel",
    concat!(
        "/{resourcePath}/{httpMethod}/throttling/burstLimit",
        "/{resourcePath}/{httpMethod}/throttling/rateLimit",
        "/{resourcePath}/{httpMethod}/caching/ttlInSeconds",
    ),
    "/{resourcePath}/{httpMethod}/caching/enabled",
    "/{resourcePath}/{httpMethod}/caching/dataEncrypted",
    "/{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl",
    "/{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy",
    "/*/*/metrics/enabled",
    "/*/*/logging/dataTrace",
    "/*/*/logging/loglevel",
    concat!(
        "/*/*/throttling/burstLimit",
        " /*/*/throttling/rateLimit",
        " /*/*/caching/ttlInSeconds",
    ),
    "/*/*/caching/enabled",
    "/*/*/caching/dataEncrypted",
    "/*/*/caching/requireAuthorizationForCacheControl",
    "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
    "/variables/{variable_name}",
    "/tracingEnabled",
];

/// How a stage words a refusal of a patch path it cannot apply: every pointer that is no
/// member of the `Stage` shape is read as a per-method setting's path, so the sentence
/// names the rejected path as such and then enumerates [`STAGE_PATCH_PATHS`] — the one
/// set, whichever `op` named the path.
const STAGE_INVALID_PATCH_PATH: crate::state::InvalidPathRefusal =
    crate::state::InvalidPathRefusal::MethodSettingPath(STAGE_PATCH_PATHS);

/// The one pointer TEMPLATE a stage publishes for a `remove`: a per-method settings path,
/// `{resource_path}/{http_method}` as `Stage$methodSettings` states one, with the setting
/// the `remove` takes away written after it.
///
/// A stage's own single-valued members are not among them — a stage reports every one of
/// them, and a caller sets one to another value through the `replace` the pointer publishes
/// instead — and neither is any other pointer that is no part of a stage a `remove` can take
/// AWAY ([`remove_names_no_part_of_a_stage`]). Asserted end to end from a second statement of
/// this text in `tests/all/stage_remove_patch_grammar.rs`.
const METHOD_SETTINGS_PATH_TEMPLATE: &str = "/{resourcePath}/{httpMethod}";

/// The one-token pointers of a stage a `remove` DOES name something to take away at: the
/// nested settings structures, which a `remove` of the structure itself turns off
/// ([`apply_access_log_patch`], [`apply_canary_patch`]).
///
/// Every other pointer a `remove` can apply names a method's settings, an entry of a
/// collection the stage keys by name, or a member of one of these structures — all of which
/// carry more reference tokens than the method path itself does ([`METHOD_PATH_TOKENS`]), so
/// the removable parts of a stage that ONE token can name are exactly these two.
const REMOVABLE_STAGE_STRUCTURES: &[&str] = &["accessLogSettings", "canarySettings"];

/// Whether this `remove` names no part of a stage it could take AWAY.
///
/// A `remove` takes a part of the stage away, and the parts that can go away are a method's
/// settings, the two structures above and the entries of the collections a stage keys by
/// name. A pointer of a SINGLE reference token that is neither of those structures names
/// none of them: it cannot spell a method path, which takes at least
/// [`METHOD_PATH_TOKENS`] tokens, and every single-valued member of the `Stage` shape is
/// such a pointer — so a `remove` of one is refused rather than clearing the member, and
/// the stage is left exactly as it was.
fn remove_names_no_part_of_a_stage(patch: &Patch) -> bool {
    matches!(patch.op, PatchOpKind::Remove)
        && patch.tokens.len() < METHOD_PATH_TOKENS
        && !REMOVABLE_STAGE_STRUCTURES.contains(&patch.member())
}

/// How API Gateway refuses such a `remove`: the shared frame that quotes the rejected path
/// and the `op` that named it, and then the one template a stage publishes for that op
/// ([`METHOD_SETTINGS_PATH_TEMPLATE`]) — unbracketed, which is how the service prints a set
/// of one.
fn remove_outside_the_method_settings_grammar(patch: &Patch) -> PatchFailure {
    PatchFailure::BadRequest(crate::state::invalid_patch_path_for_op(
        &patch.path,
        patch.op,
        METHOD_SETTINGS_PATH_TEMPLATE,
    ))
}

/// The pointers a patch may name inside a stage's `accessLogSettings`, spelled from the
/// stage root: the two members the `AccessLogSettings` shape declares
/// (`smithy_query(query="shape", shape="AccessLogSettings")` — `destinationArn` and
/// `format`), which are the arms [`apply_access_log_patch`] has, in the order API
/// Gateway's own refusal enumerates them.
const ACCESS_LOG_PATHS: &[&str] = &[
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
];

/// How a refusal of a pointer inside `accessLogSettings` is worded: by the STRUCTURE, not
/// by the stage — the member's name and the pointers above, with nothing of the rejected
/// path, which is the sentence API Gateway answers such a pointer with. The three ops a
/// stage takes on this structure enumerate the one list; `move`, `copy` and `test` name
/// nothing inside it and are refused as ops.
const ACCESS_LOG_REFUSAL: crate::state::InvalidPathRefusal =
    crate::state::InvalidPathRefusal::NestedStructure {
        member: "accessLogSettings",
        paths: crate::state::SupportedPatchPaths::for_every_op(ACCESS_LOG_PATHS),
    };

/// The `cacheTtlInSeconds` a method-setting entry carries when the patch does not name
/// it: "The default TTL value for API caching is 300 seconds."
const DEFAULT_CACHE_TTL_SECONDS: i32 = 300;

/// The `throttlingRateLimit` a method-setting entry carries when the patch does not name
/// it: API Gateway's default account-level steady-state request rate of 10,000 requests
/// per second.
const DEFAULT_THROTTLING_RATE_LIMIT: f64 = 10_000.0;

/// The `throttlingBurstLimit` a method-setting entry carries when the patch does not name
/// it: API Gateway's default account-level burst of 5,000 requests.
const DEFAULT_THROTTLING_BURST_LIMIT: i32 = 5_000;

/// The method-setting entry a patch creates for a method path that has none yet.
///
/// A setting the patch does not name is not absent on a stage AWS has method settings
/// for: it carries the service's own default — the documented account-level throttle,
/// the documented default cache TTL, and cache invalidation requiring authorization and
/// answering an unauthorized request with the response header. `loggingLevel` is the one
/// member the model declares without a default, so it stays absent until a patch sets it.
fn default_method_setting() -> MethodSetting {
    MethodSetting {
        metrics_enabled: false,
        logging_level: None,
        data_trace_enabled: false,
        throttling_burst_limit: DEFAULT_THROTTLING_BURST_LIMIT,
        throttling_rate_limit: DEFAULT_THROTTLING_RATE_LIMIT,
        caching_enabled: false,
        cache_ttl_in_seconds: DEFAULT_CACHE_TTL_SECONDS,
        cache_data_encrypted: false,
        require_authorization_for_cache_control: true,
        unauthorized_cache_control_header_strategy: Some(
            "SUCCEED_WITH_RESPONSE_HEADER".to_string(),
        ),
    }
}

/// Refuse a deployment id this API has no deployment for: a stage, and a canary of it,
/// serves a deployment of its own API — the same rule `CreateStage` applies.
///
/// A patch VALUE that names no record is a refusal of the REQUEST
/// (`BadRequestException`), not of the resource the URI addresses: the stage and the API
/// this patch was sent to both exist, so the caller has to fix the value rather than
/// abandon the stage. The not-found class, and the identifier sentence
/// ([`crate::state::NO_SUCH_DEPLOYMENT`]), stay for the identifiers in the URI.
fn require_deployment(
    deployments: &BTreeSet<String>,
    deployment_id: &str,
) -> Result<(), PatchFailure> {
    if deployments.contains(deployment_id) {
        return Ok(());
    }
    Err(PatchFailure::BadRequest(
        crate::state::DEPLOYMENT_ID_DOES_NOT_EXIST.to_string(),
    ))
}

/// The stored state a stage's patch is applied against, beside the record itself: what a
/// patch VALUE naming another record is checked against, and the account-level
/// precondition of a stage's execution logging. Read from the store once per request, so
/// every op of one `patchOperations` list is applied against the same state.
struct StagePatchContext<'a> {
    /// The ids of the API's stored deployments: a stage, and a canary of it, serves a
    /// deployment of its own API.
    deployments: &'a BTreeSet<String>,
    /// The ids of the account's client certificates: a stage presents one of them to its
    /// integration endpoint.
    client_certificates: &'a BTreeSet<String>,
    /// Whether the ACCOUNT has a CloudWatch Logs role, which a patch turning execution
    /// logging on requires (see [`ACCOUNT_CLOUDWATCH_ROLE_REQUIRED`]).
    logging_role_configured: bool,
    /// The instant this update is ANSWERED at: when a patch of the cache cluster accepts
    /// the provisioning or the teardown it asks for, so the due instant every op of one
    /// list records is the same one this answer is rendered as of.
    accepted_at: ::std::time::Instant,
}

/// Apply one patch to a COPY of a stored stage.
///
/// An `add` is applied only where it names something to add ([`add_names_an_entry`]); on
/// every other pointer of a stage it is refused as the OPERATION it is, naming the path it
/// rode in on ([`invalid_operation`]) — including a pointer this operation has no applier
/// for at all ([`names_a_pointer_of_a_stage`]), for which an `add` puts nothing anywhere
/// whether or not the stage could address it. What is judged ahead of that refusal is the
/// VALUE: a value the member the pointer names would refuse is refused on its own terms
/// whichever op carried it, so such an `add` is run against a throwaway copy of the record
/// and refused as an op only once that copy took it.
fn apply_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    context: &StagePatchContext<'_>,
) -> Result<(), PatchFailure> {
    if matches!(patch.op, PatchOpKind::Add) && !add_names_an_entry(patch) {
        if names_a_pointer_of_a_stage(patch) {
            apply_stage_patch(&mut entry.clone(), patch, context)?;
        }
        return Err(PatchFailure::BadRequest(invalid_operation(
            patch.op,
            &patch.path,
        )));
    }
    apply_stage_patch(entry, patch, context)
}

/// The members of the `Stage` shape this operation applies a patch to — the arms
/// [`apply_stage_patch`] dispatches on.
///
/// Read to answer an `add` WITHOUT applying anything: a pointer whose first reference token
/// is none of these, and which names no method setting either ([`method_setting`]), is one
/// this operation has no applier for, so there is no member to judge the op's `value`
/// against and the `add` is refused as the operation. `stageName`, `cacheClusterStatus` and
/// the timestamps are members of the shape that no patch writes, so they are not among them.
const STAGE_PATCHED_MEMBERS: &[&str] = &[
    "deploymentId",
    "description",
    "documentationVersion",
    "clientCertificateId",
    "tracingEnabled",
    "cacheClusterEnabled",
    "cacheClusterSize",
    "variables",
    "canarySettings",
    "accessLogSettings",
];

/// Whether this pointer names a part of a stage this operation has an applier for: one of
/// [`STAGE_PATCHED_MEMBERS`], or a setting of a method ([`method_setting`]).
fn names_a_pointer_of_a_stage(patch: &Patch) -> bool {
    STAGE_PATCHED_MEMBERS.contains(&patch.member()) || method_setting(patch).is_some()
}

/// Whether an `add` names a pointer of a stage it applies to.
///
/// `add` is the op that puts something THERE that was not: on a stage it names one entry
/// of a collection the stage keys by name — a stage variable, one of the canary's variable
/// overrides, or a method's setting, which the stage carries no entry for until a patch
/// names it. Every other pointer of a stage names a single-valued member the stage already
/// has, which is `replace`d, so an `add` of one puts nothing anywhere and is refused.
///
/// Stated positively, from the three collections only, so it cannot drift from the
/// dispatch below: a pointer that names none of them names no entry.
fn add_names_an_entry(patch: &Patch) -> bool {
    match patch.member() {
        "variables" => patch.tokens.len() > 1,
        "canarySettings" => {
            patch.tokens.len() > 2
                && patch
                    .tokens
                    .get(1)
                    .is_some_and(|member| member == "stageVariableOverrides")
        }
        // A method's settings, which the stage holds no entry for until one is added. A
        // single-valued member of the `Stage` shape names no setting, so it answers here
        // too.
        _ => method_setting(patch).is_some(),
    }
}

/// Apply one patch that named an op and a pointer the stage takes, to the member of the
/// record that pointer names.
fn apply_stage_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    context: &StagePatchContext<'_>,
) -> Result<(), PatchFailure> {
    // A `copy` is the canary promotion, whose legal (path, from) pairs are the stage's
    // copy grammar — judged for the op as a whole rather than per member, because the
    // refusal names both sides of the pair the request sent.
    if matches!(patch.op, PatchOpKind::Copy) {
        return apply_canary_promotion(entry, patch, context.deployments);
    }
    // A `remove` applies only where the stage has a part to take away: judged for the
    // pointer as a whole, and before any member of the record is edited, so a `remove` of a
    // member the stage reports is refused rather than clearing it.
    if remove_names_no_part_of_a_stage(patch) {
        return Err(remove_outside_the_method_settings_grammar(patch));
    }
    match patch.member() {
        "deploymentId" => apply_deployment_id(entry, patch, context.deployments),
        "description" => {
            crate::state::patch_description(&mut entry.description, patch, MAX_STRING_CHARS)
        }
        "documentationVersion" => {
            patch_optional_string(&mut entry.documentation_version, patch, MAX_STRING_CHARS)
        }
        "clientCertificateId" => {
            patch_optional_string(&mut entry.client_certificate_id, patch, MAX_STRING_CHARS)?;
            // "The identifier of a client certificate for an API stage"
            // (`Stage$clientCertificateId`): the stage presents that certificate to its
            // integration endpoint, so a value has to name one the ACCOUNT has — refused
            // as a bad REQUEST for the reason [`require_deployment`] gives.
            //
            // An EMPTY value names no certificate and asks for none: it RELEASES the
            // association, as a `remove` of the member does, so it is the one value that is
            // not looked up. That is what lets a caller stop a stage presenting a
            // certificate — and what makes the certificate deletable again — through the
            // same `replace` that assigned it.
            if entry
                .client_certificate_id
                .as_deref()
                .is_some_and(str::is_empty)
            {
                entry.client_certificate_id = None;
            }
            if let Some(certificate) = &entry.client_certificate_id {
                if !context.client_certificates.contains(certificate) {
                    return Err(PatchFailure::BadRequest(
                        crate::state::NO_SUCH_CLIENT_CERTIFICATE.to_string(),
                    ));
                }
            }
            Ok(())
        }
        "tracingEnabled" => patch_stage_bool(&mut entry.tracing_enabled, patch),
        "cacheClusterEnabled" => apply_cache_cluster_enabled(entry, patch, context.accepted_at),
        "cacheClusterSize" => apply_cache_cluster_size(entry, patch, context.accepted_at),
        "variables" => {
            let name = patch.map_key(1)?;
            // The name and the value go through the one check `CreateStage` enforces for
            // a stage variable, so a variable this service refuses on a create cannot be
            // written through a patch.
            if matches!(patch.op, PatchOpKind::Add | PatchOpKind::Replace) {
                crate::state::bound_stage_variable(&name, patch.require_value()?)
                    .map_err(PatchFailure::BadRequest)?;
            }
            // The member the patch addresses exists once a request names it: a stage
            // whose variables were never set carries NO map at all, and setting (or
            // dropping) an entry is what gives it one — which is what keeps a stage
            // configured with an empty map distinct from one configured with none.
            patch_string_map(
                entry.variables.get_or_insert_with(BTreeMap::new),
                patch,
                name,
                MAX_COLLECTION_ENTRIES,
                MAX_STRING_CHARS,
            )
        }
        "canarySettings" => apply_canary_patch(entry, patch, context.deployments),
        "accessLogSettings" => apply_access_log_patch(entry, patch),
        // Everything else is addressed as a method path rather than as a member of the
        // stage: `/{resource_path}/{http_method}/<setting>`.
        _ => apply_method_setting_patch(entry, patch, context.logging_role_configured),
    }
}

/// Promote the canary: apply the one `copy` of the stage's copy grammar
/// ([`CANARY_PROMOTIONS`]) the request sent, or refuse a `copy` outside it.
fn apply_canary_promotion(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    let from = patch.from.as_deref().unwrap_or_default();
    let promoted = CANARY_PROMOTIONS
        .iter()
        .find(|(path, source)| *path == patch.path && *source == from)
        .map(|(path, _)| *path)
        .ok_or_else(|| PatchFailure::BadRequest(invalid_copy_operation(&patch.path, from)))?;
    let canary = entry.canary_settings.clone();
    if promoted == "/deploymentId" {
        // The deployment the canary SERVES becomes the one the stage serves, so it has to
        // be a deployment of this API, exactly as a `replace` of the member does. A canary
        // that is configured but carries no deployment of its own serves the stage's
        // ([`crate::ops::create_stage::stage_body`] reports that), so promoting it leaves
        // the stage serving what it already serves; a stage with no canary at all has
        // nothing to promote.
        let deployment_id = canary
            .as_ref()
            .map(|canary| {
                canary
                    .deployment_id
                    .clone()
                    .unwrap_or_else(|| entry.deployment_id.clone())
            })
            .ok_or_else(|| {
                PatchFailure::BadRequest(format!(
                    "{CANARY_DEPLOYMENT_POINTER} names no deployment on this stage, so there is \
                     nothing to copy to {}",
                    patch.path
                ))
            })?;
        require_deployment(deployments, &deployment_id)?;
        entry.deployment_id = deployment_id;
        return Ok(());
    }
    // `/variables`: the overrides the canary served are promoted ONTO the stage's own
    // variables — an overridden name takes the canary's value and a name the canary did
    // not override keeps the stage's — so a canary that overrode nothing promotes nothing.
    // The result is bounded like every other caller-driven collection.
    let mut variables = entry.variables.clone().unwrap_or_default();
    variables.extend(
        canary
            .and_then(|canary| canary.stage_variable_overrides)
            .unwrap_or_default(),
    );
    crate::state::bound_entries("variables", variables.len(), MAX_COLLECTION_ENTRIES)
        .map_err(PatchFailure::BadRequest)?;
    // A promotion that merges nothing onto a stage whose variables were never set leaves
    // it with none: the member is reported for a stage that HAS a map, and this copy
    // gives it one only where the canary or the stage itself carried entries.
    if entry.variables.is_some() || !variables.is_empty() {
        entry.variables = Some(variables);
    }
    Ok(())
}

/// Point the stage at another deployment from the op's `value`. A canary promotion reads
/// the canary's own deployment id instead and is applied by [`apply_canary_promotion`].
fn apply_deployment_id(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    let deployment_id = match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => patch.require_value()?.to_string(),
        // A stage always points at a deployment, so the member cannot be removed.
        _ => return Err(patch.unsupported_op()),
    };
    if deployment_id.is_empty() {
        return Err(PatchFailure::BadRequest(format!(
            "{} must not be empty",
            patch.path
        )));
    }
    require_deployment(deployments, &deployment_id)?;
    entry.deployment_id = deployment_id;
    Ok(())
}

/// Enable or disable the stage's cache cluster, recording the instant the transition the
/// clock now owes is due — `accepted_at` being the instant this update is answered at.
fn apply_cache_cluster_enabled(
    entry: &mut StageEntry,
    patch: &Patch,
    accepted_at: ::std::time::Instant,
) -> Result<(), PatchFailure> {
    let was_enabled = entry.cache_cluster_enabled;
    patch_stage_bool(&mut entry.cache_cluster_enabled, patch)?;
    if entry.cache_cluster_enabled == was_enabled {
        // Asking for what the stage already has provisions nothing, so no transition is
        // owed and an outstanding one is left alone.
        return Ok(());
    }
    let now = accepted_at;
    // A cluster that is provisioned or torn down holds nothing: the entries of the one
    // that was there do not survive it (`StageEntry::response_cache`).
    entry.flush_response_cache();
    if entry.cache_cluster_enabled {
        entry.cache_cluster_ready_at = Some(now + crate::state::CACHE_CLUSTER_CREATE_SPAN);
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = None;
    } else {
        // The cluster is torn down over its span; a flush of the cluster that is going
        // away is no longer outstanding.
        entry.cache_cluster_ready_at = None;
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = Some(now + crate::state::CACHE_CLUSTER_DELETE_SPAN);
    }
    Ok(())
}

/// Change the stage's cache capacity. Re-sizing a cluster the stage HAS provisions a
/// cluster of the new capacity, which is a creation the clock owes; a size set on a stage
/// with no cluster is stored and provisions nothing.
///
/// The capacity is stored exactly as the request spells it. A patch `value` is a bare
/// string on the wire (`PatchOperation$value`), so the published catalogue of capacities
/// the `CacheClusterSize` enum declares constrains the MODELLED member — where
/// `CreateStage` or `CreateDeployment` carries one and the request-validation layer
/// checks it — and not this member's stored string, which every reader of the stage
/// echoes back as it was set.
fn apply_cache_cluster_size(
    entry: &mut StageEntry,
    patch: &Patch,
    accepted_at: ::std::time::Instant,
) -> Result<(), PatchFailure> {
    let before = entry.cache_cluster_size.clone();
    patch_optional_string(&mut entry.cache_cluster_size, patch, MAX_STRING_CHARS)?;
    if entry.cache_cluster_enabled && entry.cache_cluster_size != before {
        entry.cache_cluster_ready_at = Some(accepted_at + crate::state::CACHE_CLUSTER_CREATE_SPAN);
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = None;
        // A cluster of the new capacity comes up holding nothing, as a provisioned one
        // does in [`apply_cache_cluster_enabled`].
        entry.flush_response_cache();
    }
    Ok(())
}

/// Apply one patch that names the `canarySettings` structure or reaches into it.
fn apply_canary_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    // The path naming the structure itself removes the whole canary configuration; there
    // is no value a single op could set every member of it to.
    if patch.tokens.len() == 1 {
        return match patch.op {
            PatchOpKind::Remove => {
                entry.canary_settings = None;
                Ok(())
            }
            _ => Err(patch.unsupported_path()),
        };
    }
    let mut canary = entry.canary_settings.clone().unwrap_or_default();
    // The structure's own members are addressed the way a top-level member is, with the
    // `canarySettings` token dropped.
    let nested = patch.rebased(1);
    match nested.member() {
        "percentTraffic" => {
            patch_f64_checked(
                &mut canary.percent_traffic,
                &nested,
                crate::state::accepted_percent_traffic,
            )?;
        }
        "useStageCache" => patch_stage_bool(&mut canary.use_stage_cache, &nested)?,
        "deploymentId" => {
            patch_optional_string(&mut canary.deployment_id, &nested, MAX_STRING_CHARS)?;
            if let Some(deployment_id) = &canary.deployment_id {
                require_deployment(deployments, deployment_id)?;
            }
        }
        "stageVariableOverrides" => {
            let name = nested.map_key(1)?;
            // A canary's override IS a stage variable, held to the same rule as the
            // stage's own map.
            if matches!(nested.op, PatchOpKind::Add | PatchOpKind::Replace) {
                crate::state::bound_stage_variable(&name, nested.require_value()?)
                    .map_err(PatchFailure::BadRequest)?;
            }
            let mut overrides: BTreeMap<String, String> = canary
                .stage_variable_overrides
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            patch_string_map(
                &mut overrides,
                &nested,
                name,
                MAX_COLLECTION_ENTRIES,
                MAX_STRING_CHARS,
            )?;
            canary.stage_variable_overrides =
                (!overrides.is_empty()).then(|| overrides.into_iter().collect());
        }
        _ => return Err(patch.unsupported_path()),
    }
    entry.canary_settings = Some(canary);
    Ok(())
}

/// Apply one patch that names the `accessLogSettings` structure or reaches into it.
fn apply_access_log_patch(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    // Re-labelled once, here: every way a pointer into this structure can fail to apply —
    // the structure named as a whole by an op that cannot address it that way, a
    // sub-member it does not have, a pointer reaching past one it does — is answered with
    // the structure's own sentence (see [`ACCESS_LOG_REFUSAL`]), which the nested patch
    // below inherits.
    let patch = &patch.refused_with(ACCESS_LOG_REFUSAL);
    if patch.tokens.len() == 1 {
        return match patch.op {
            PatchOpKind::Remove => {
                entry.access_log_settings = None;
                Ok(())
            }
            _ => Err(patch.unsupported_path()),
        };
    }
    let mut settings = entry.access_log_settings.clone().unwrap_or_default();
    let nested = patch.rebased(1);
    match nested.member() {
        "destinationArn" => {
            patch_optional_string(&mut settings.destination_arn, &nested, MAX_STRING_CHARS)?;
            // "The Amazon Resource Name (ARN) of the CloudWatch Logs log group or Kinesis
            // Data Firehose delivery stream to receive access logs"
            // (`AccessLogSettings$destinationArn`): the destination is named by an ARN, so
            // a value that is none is refused the way this service refuses an unreadable
            // ARN on any member.
            if let Some(arn) = &settings.destination_arn {
                crate::state::require_arn(arn).map_err(PatchFailure::BadRequest)?;
            }
        }
        "format" => {
            // The format is not a part of a stage a `remove` takes away on its own: it is
            // "a single line format of the access logs of data" the destination receives
            // (`AccessLogSettings$format`), so it goes with the settings rather than
            // separately, and the pointer is read the way every pointer that is no
            // removable part of a stage is — as a method setting's, see
            // [`remove_method_setting`]. A pointer reaching PAST the format names nothing
            // of this structure whatever the op, so it keeps the structure's own refusal.
            if matches!(patch.op, PatchOpKind::Remove) && nested.tokens.len() == 1 {
                return remove_method_setting(entry, patch);
            }
            patch_optional_string(&mut settings.format, &nested, MAX_STRING_CHARS)?;
            // From `AccessLogSettings$format`: "The format must include at least
            // $context.requestId." EITHER identifier of the request satisfies it
            // ([`ACCESS_LOG_REQUEST_IDENTIFIERS`]), named as a reference rather than as the
            // prefix of a longer member ([`identifies_the_request`]), and the refusal names
            // them all ([`access_log_format_refused`]), so the caller is told the whole rule
            // rather than one half of it.
            if let Some(format) = &settings.format {
                if !identifies_the_request(format) {
                    return Err(PatchFailure::BadRequest(access_log_format_refused(format)));
                }
            }
        }
        _ => return Err(patch.unsupported_path()),
    }
    entry.access_log_settings = Some(settings);
    Ok(())
}

/// Each method setting's published patch name — its group and its own name — beside the
/// `MethodSetting` member that names the SAME setting in ONE reference token
/// (`smithy_query(query="shape", shape="MethodSetting")`).
///
/// A patch may address a setting either way: the grouped name is what API Gateway's
/// patch-operations table and console use, and the member name is what a recorded AWS
/// answer shows the service accepting (`/~1<resource>/<METHOD>/metricsEnabled` is
/// answered with the stage's settings, not a refusal). Listed once, so the two spellings
/// of one setting can never write different members.
const METHOD_SETTING_NAMES: &[(&str, &str)] = &[
    ("metrics/enabled", "metricsEnabled"),
    ("logging/loglevel", "loggingLevel"),
    ("logging/dataTrace", "dataTraceEnabled"),
    ("throttling/burstLimit", "throttlingBurstLimit"),
    ("throttling/rateLimit", "throttlingRateLimit"),
    ("caching/enabled", "cachingEnabled"),
    ("caching/ttlInSeconds", "cacheTtlInSeconds"),
    ("caching/dataEncrypted", "cacheDataEncrypted"),
    (
        "caching/requireAuthorizationForCacheControl",
        "requireAuthorizationForCacheControl",
    ),
    (
        "caching/unauthorizedCacheControlHeaderStrategy",
        "unauthorizedCacheControlHeaderStrategy",
    ),
];

/// How many reference tokens of a method-setting path name the METHOD when the pointer
/// spells the resource path and the HTTP method separately: "method paths defined as
/// {resource_path}/{http_method} for an individual method override, or /\*/\* for
/// overriding all methods in the stage" (`Stage$methodSettings`), with every `/` INSIDE
/// the resource path escaped `~1` in the POINTER the way a JSON Pointer requires.
const METHOD_PATH_TOKENS: usize = 2;

/// How many reference tokens name the method when the resource path's LEADING separator is
/// the pointer's OWN rather than an escape (`//pets/GET/…` is `/` + `/pets` + `/GET`): that
/// separator opens an empty first reference token, which belongs to the resource half, so the
/// method path is one token longer. See [`method_path_spellings`].
const PREFIXED_METHOD_PATH_TOKENS: usize = METHOD_PATH_TOKENS + 1;

/// How many leading reference tokens of THIS pointer may name the method, in the order they
/// are read.
///
/// A resource path "begins with a '/'" (`Resource$path`), and a pointer may leave that one
/// leading separator to its own rather than escaping it — the spelling
/// `MethodSetting$resourcePath` states a path in — which opens an EMPTY first reference token
/// the resource half carries. The root resource is then named by the method path's two tokens
/// (`//GET/…`) and every deeper resource path by three (`//pets/GET/…`), so both are read,
/// the shorter one first. That leading separator is the only unescaped one a resource half may
/// carry: every separator INSIDE the resource path is escaped `~1`, so no other pointer names
/// a method in more than [`METHOD_PATH_TOKENS`] tokens.
fn method_path_spellings(patch: &Patch) -> &'static [usize] {
    if patch.tokens.first().is_some_and(String::is_empty) {
        return &[METHOD_PATH_TOKENS, PREFIXED_METHOD_PATH_TOKENS];
    }
    &[METHOD_PATH_TOKENS]
}

/// The `methodSettings` key a patch path names and the setting it writes, or `None` for a
/// path that names no setting of a method.
///
/// A pointer spells the method in one of the two ways API Gateway reads:
///
/// * as its own reference tokens, `/{resource_path}/{http_method}/…` — the separator
///   before the method is a real one and every separator INSIDE the resource path is
///   escaped, its leading one either escaped too (`/~1pets/GET/…`) or left to the pointer's
///   own leading separator (`//pets/GET/…`, the spelling `MethodSetting$resourcePath` states
///   a path in; see [`method_path_spellings`]) — and then the setting, in either spelling
///   [`METHOD_SETTING_NAMES`] lists;
/// * as ONE token carrying the whole method path, `/{escaped_method_path}/{setting}`, and
///   then the setting's own member name. That token escapes the separator before the method
///   as well, so the method is read out of it by decoding that one escape.
///
/// The key is the method path the pointer SPELLED: the tokens before the setting, written
/// exactly as the request wrote them ([`addressed_method_path`]) — which is the key API
/// Gateway reports the entry under, so a client reads a setting back under the path it
/// patched. A one-token method path spells the SAME key as the pointer that gives the method
/// its own token, since the only separator a key writes literally is the one before the method
/// ([`crate::state::spelled_method_path`]). Two pointers that spell the RESOURCE half
/// differently do name two entries, and a stage cannot be updated while it carries both
/// ([`duplicate_method_settings`]).
///
/// A pointer that is neither — a one-token method path followed by a GROUPED setting name,
/// a tail that names no setting, a path that stops at the method — names no setting of a
/// method, and so never becomes a `methodSettings` entry.
///
/// In the two-token spelling the first reference token IS the resource half, whatever it
/// spells, so nothing is asked of it beyond the SETTING that has to follow the method: a
/// resource path whose inner separator went unescaped (`/pets/{petId}/GET/metrics/enabled`)
/// pushes the verb into the tail, which then names no setting, and is refused by that
/// question alone.
fn method_setting(patch: &Patch) -> Option<(String, &'static str)> {
    for method_tokens in method_path_spellings(patch).iter().copied() {
        if let Some(tail) = patch
            .tokens
            .get(method_tokens..)
            .filter(|tail| !tail.is_empty())
        {
            if let Some(setting) = named_setting(&tail.join("/")) {
                return Some((addressed_method_path(patch, method_tokens)?, setting));
            }
        }
    }
    let [method_path, setting] = patch.tokens.as_slice() else {
        return None;
    };
    // The grouped spelling of a setting is two reference tokens, so a one-token method
    // path can only be followed by the setting's member name — which is the only spelling
    // one token can match. The answer is the published name either way.
    let setting = named_setting(setting)?;
    // Decoded, that one token has to BE a method path: a resource path, and then the HTTP
    // method after the `/` the escape stood for. The KEY is the method path the token SPELLED
    // all the same — its last escape written as the literal separator a key carries, the
    // resource half as the request wrote it ([`crate::state::spelled_method_path`]) — so this
    // spelling and the one that gives the method its own token name the ONE entry.
    let (resource_path, _) = method_path.rsplit_once('/')?;
    if !names_a_resource(resource_path) {
        return None;
    }
    let spelled = crate::state::spelled_method_path(written_tokens(patch).first()?)?;
    Some((spelled, setting))
}

/// Whether `resource_path` is the resource half of a method path spelled WHOLE inside one
/// reference token: a RESOURCE path, which "begins with a '/'" as `Resource$path` spells one
/// — so the pointer escapes that `/` as `~1` — or the token that stands for every resource
/// of the stage.
///
/// Asked of the ONE-token spelling alone: there, nothing but the token itself says where the
/// resource path stops and the HTTP method begins, so a token whose head is no resource path
/// names no method. The two-token spelling supplies its own separators and needs no such
/// question ([`method_setting`]).
fn names_a_resource(resource_path: &str) -> bool {
    resource_path.starts_with('/') || resource_path == EVERY_RESOURCE
}

/// The PUBLISHED name of the setting `tail` spells, in either spelling, or `None` when it
/// names no setting.
fn named_setting(tail: &str) -> Option<&'static str> {
    // The log level is the one setting with two spellings of its published name in
    // circulation, so `loglevel` and `logLevel` are normalised to the published one.
    match tail {
        "logging/logLevel" => Some("logging/loglevel"),
        tail => METHOD_SETTING_NAMES
            .iter()
            .find(|(published, member)| *published == tail || *member == tail)
            .map(|(published, _)| *published),
    }
}

/// The `methodSettings` key the leading `method_tokens` reference TOKENS of this patch's path
/// name: the method path AS THE POINTER SPELLED IT, each token exactly as the request wrote
/// it. `None` for a path carrying fewer tokens than that.
///
/// The tokens are taken from the path itself rather than from `patch.tokens`, which is the
/// same pointer DECODED: a key is what the pointer SAID (`~1pets/GET`), while the decoded
/// tokens are what it MEANT (the resource path `/pets`, which is the method the key denotes —
/// [`crate::state::method_of_settings_key`]).
fn addressed_method_path(patch: &Patch, method_tokens: usize) -> Option<String> {
    Some(written_tokens(patch).get(..method_tokens)?.join("/"))
}

/// This pointer's reference tokens as the REQUEST WROTE them, escapes and all: the path with
/// its leading separator dropped, split on every separator, which is how a JSON Pointer is
/// read apart before its tokens are decoded ([`crate::state::Patch`]).
fn written_tokens(patch: &Patch) -> Vec<&str> {
    patch
        .path
        .strip_prefix('/')
        .map(|rest| rest.split('/').collect())
        .unwrap_or_default()
}

/// The `BadRequestException` message API Gateway refuses a `remove` of a method setting
/// the stage carries none of with.
///
/// A `remove` names a setting that goes AWAY, so the stage has to carry settings for the
/// method the path addresses. The only variable part is the setting path the request
/// sent, with the pointer's leading `/` dropped; the rest, the trailing space included,
/// is the service's own fixed text.
fn cannot_remove_method_setting(setting: &str) -> String {
    format!(
        "Cannot remove method setting {setting} because there is no method setting for this \
         method "
    )
}

/// The key this stage holds the settings of the method `key` denotes under: `key` itself when
/// the stage carries that entry, else the entry of the SAME method under another spelling.
///
/// A `remove` takes the settings of a METHOD away rather than those of a spelling — the
/// sentence API Gateway refuses one it has nothing to take away with says exactly that
/// ("there is no method setting for this method", [`cannot_remove_method_setting`]) — so a
/// pointer that spells the method differently from the one that wrote the entry still names
/// it. A WRITE keys the entry by its own spelling instead, which is what makes two spellings
/// two entries.
fn held_method_key(entry: &StageEntry, key: &str) -> Option<String> {
    if entry.method_settings.contains_key(key) {
        return Some(key.to_string());
    }
    entry
        .method_settings
        .keys()
        .find(|held| same_method(held, key))
        .cloned()
}

/// Take a method's settings away.
///
/// `methodSettings` is "a map that defines the method settings for a Stage resource"
/// (`Stage$methodSettings`), so a `remove` whose path ends in a setting puts that one
/// setting back to the value a stage carries for a setting nobody configured, and a path
/// naming the method alone takes its whole entry out of the map.
///
/// Either way the method has to HAVE an entry on this stage: there is nothing to take
/// away otherwise, and that is the condition [`cannot_remove_method_setting`] words.
/// Every pointer that is no removable part of a stage is read as a method setting's path,
/// so such a `remove` lands here too and is refused by the same sentence.
fn remove_method_setting(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let refuse = || {
        PatchFailure::BadRequest(cannot_remove_method_setting(
            patch.path.strip_prefix('/').unwrap_or(&patch.path),
        ))
    };
    let Some((key, setting)) = method_setting(patch) else {
        // The path names the method and nothing beyond it, so the whole entry goes —
        // whichever spelling it addressed that method by, since a `remove` names the method
        // rather than the spelling ([`held_method_key`]). A path that names neither a setting
        // of a method nor a method alone takes nothing away.
        let key = method_path_spellings(patch)
            .iter()
            .copied()
            .find(|tokens| *tokens == patch.tokens.len())
            .and_then(|tokens| addressed_method_path(patch, tokens))
            .and_then(|key| held_method_key(entry, &key))
            .ok_or_else(refuse)?;
        return entry
            .method_settings
            .remove(&key)
            .map(|_| ())
            .ok_or_else(refuse);
    };
    // The value a setting nobody configured carries, so a setting that was removed reads
    // back exactly as it does on a method the patch never named.
    let unconfigured = default_method_setting();
    let key = held_method_key(entry, &key).ok_or_else(refuse)?;
    let settings = entry.method_settings.get_mut(&key).ok_or_else(refuse)?;
    match setting {
        "metrics/enabled" => settings.metrics_enabled = unconfigured.metrics_enabled,
        "logging/loglevel" => settings.logging_level = unconfigured.logging_level,
        "logging/dataTrace" => settings.data_trace_enabled = unconfigured.data_trace_enabled,
        "throttling/burstLimit" => {
            settings.throttling_burst_limit = unconfigured.throttling_burst_limit;
        }
        "throttling/rateLimit" => {
            settings.throttling_rate_limit = unconfigured.throttling_rate_limit;
        }
        "caching/enabled" => settings.caching_enabled = unconfigured.caching_enabled,
        "caching/ttlInSeconds" => {
            settings.cache_ttl_in_seconds = unconfigured.cache_ttl_in_seconds;
        }
        "caching/dataEncrypted" => {
            settings.cache_data_encrypted = unconfigured.cache_data_encrypted;
        }
        "caching/requireAuthorizationForCacheControl" => {
            settings.require_authorization_for_cache_control =
                unconfigured.require_authorization_for_cache_control;
        }
        "caching/unauthorizedCacheControlHeaderStrategy" => {
            settings.unauthorized_cache_control_header_strategy =
                unconfigured.unauthorized_cache_control_header_strategy;
        }
        _ => return Err(patch.unsupported_path()),
    }
    Ok(())
}

/// The `BadRequestException` message API Gateway refuses an `UpdateStage` that would leave
/// the stage with the SAME method configured under two different method-path spellings with.
///
/// A stage keys its per-method settings by the method path a pointer WROTE
/// ([`method_setting`]), so two spellings of one method would be two entries whose settings
/// contradict each other on that method: API Gateway refuses the whole update instead of
/// folding them together or picking one, and asks the caller to take one of the two away. The
/// only variable parts are the colliding method paths, comma-separated — the escaped spelling
/// first ([`duplicate_method_paths`]) — the account the stage belongs to and the API the
/// request addressed; the rest is the service's own fixed text.
fn duplicate_method_settings(method_paths: &[String], rest_api_id: &str) -> String {
    format!(
        "Update Stage failed due to different method settings with the same resource path and \
         http method. Please remove one of the duplicate method settings for method paths: {} \
         for accountId: {} and restApiId: {rest_api_id}",
        method_paths.join(", "),
        ::emulator_server::emulator_account_id(),
    )
}

/// The method paths of `entry` that DUPLICATE one another — two spellings naming the one
/// method ([`crate::state::same_method`]) — and nothing when every method the stage carries
/// settings for is named one way.
///
/// The colliding paths are reported in DESCENDING order, which is the order a recorded
/// refusal of two spellings of one method prints them in: the resource half of a method path
/// is a path NAME, whose separators are escaped `~1`, and `~` sorts after every character a
/// path name carries, so the escaped spelling comes before one that left a separator literal.
fn duplicate_method_paths(entry: &StageEntry) -> Vec<String> {
    let keys: Vec<&str> = entry.method_settings.keys().map(String::as_str).collect();
    let mut duplicates: Vec<String> = Vec::new();
    for key in &keys {
        let collides = keys
            .iter()
            .any(|other| other != key && same_method(other, key));
        if collides && !duplicates.iter().any(|held| held == key) {
            duplicates.push((*key).to_string());
        }
    }
    duplicates.sort_by(|left, right| right.cmp(left));
    duplicates
}

/// Apply one patch addressed as a METHOD path:
/// `/{resource_path}/{http_method}/{group}/{setting}`, or `/*/*/{group}/{setting}` for
/// every method of the stage.
///
/// Which method the pointer names, which setting it writes and the `methodSettings` key
/// the stage reports it under are [`method_setting`]'s; a pointer that names no setting of
/// a method is refused as a path.
fn apply_method_setting_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    logging_role_configured: bool,
) -> Result<(), PatchFailure> {
    // A `remove` takes settings AWAY rather than configuring one, so it is read against
    // the settings the stage already carries — see [`remove_method_setting`].
    if matches!(patch.op, PatchOpKind::Remove) {
        return remove_method_setting(entry, patch);
    }
    let Some((method_path, setting)) = method_setting(patch) else {
        return Err(patch.unsupported_path());
    };
    // The account-level logging role is a precondition of the CONFIGURATION, checked
    // before any entry of `methodSettings` is touched: a stage cannot be set to emit
    // execution logs the account has granted API Gateway no role to write.
    if !logging_role_configured && enables_execution_logging(patch, setting) {
        return Err(PatchFailure::BadRequest(
            ACCOUNT_CLOUDWATCH_ROLE_REQUIRED.to_string(),
        ));
    }
    if !entry.method_settings.contains_key(&method_path) {
        crate::state::bound_entries(
            "methodSettings",
            entry.method_settings.len() + 1,
            MAX_COLLECTION_ENTRIES,
        )
        .map_err(PatchFailure::BadRequest)?;
    }
    let settings = entry
        .method_settings
        .entry(method_path)
        .or_insert_with(default_method_setting);
    // One setting is one reference token, so the op is applied to the last token the
    // path carries.
    let nested = patch.rebased(patch.tokens.len() - 1);
    match setting {
        "metrics/enabled" => patch_stage_bool(&mut settings.metrics_enabled, &nested),
        "logging/loglevel" => patch_enum_string_refused_with(
            &mut settings.logging_level,
            &nested,
            LOGGING_LEVEL_VALUES,
            invalid_logging_level,
        ),
        "logging/dataTrace" => patch_stage_bool(&mut settings.data_trace_enabled, &nested),
        "throttling/burstLimit" => patch_i32_refused_with(
            &mut settings.throttling_burst_limit,
            &nested,
            crate::state::accepts_method_throttle_burst,
            || INVALID_THROTTLING_SETTING.to_string(),
        ),
        "throttling/rateLimit" => patch_f64_refused_with(
            &mut settings.throttling_rate_limit,
            &nested,
            crate::state::accepts_method_throttle_rate,
            || INVALID_THROTTLING_SETTING.to_string(),
        ),
        "caching/enabled" => patch_stage_bool(&mut settings.caching_enabled, &nested),
        "caching/ttlInSeconds" => patch_cache_ttl(&mut settings.cache_ttl_in_seconds, &nested),
        "caching/dataEncrypted" => patch_stage_bool(&mut settings.cache_data_encrypted, &nested),
        "caching/requireAuthorizationForCacheControl" => patch_stage_bool(
            &mut settings.require_authorization_for_cache_control,
            &nested,
        ),
        "caching/unauthorizedCacheControlHeaderStrategy" => patch_enum_string_refused_with(
            &mut settings.unauthorized_cache_control_header_strategy,
            &nested,
            UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES,
            |_| INVALID_UNAUTHORIZED_CACHE_CONTROL_STRATEGY.to_string(),
        ),
        _ => Err(patch.unsupported_path()),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateStageError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStageError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.apigateway#UpdateStage")]
#[derive(Debug)]
pub enum UpdateStageErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stage::UpdateStageError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStageError = crate::scaffolded::Scaffolded<UpdateStageErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStageErrorModeled> for UpdateStageError {
    fn from(modeled: UpdateStageErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStage` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStage<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateStage<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStage<Eng>
{
    type Input = UpdateStageInput;
    type Output = UpdateStageOutput;
    type Error = UpdateStageError;

    const NAME: &'static str = "UpdateStage";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STAGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STAGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // The list is read BEFORE the store lock is taken, so a malformed patch never
        // reaches the records at all.
        let patches = crate::state::parse_patches_refused_with_op(
            input.patch_operations.as_ref(),
            STAGE_INVALID_PATCH_PATH,
            crate::state::UnsupportedOpRefusal::Ops(STAGE_INVALID_PATCH_OPERATION),
        )
        .map_err(patch_failure)?;

        let now = crate::state::now();
        // The instant this call is ANSWERED at: a patch of the cache cluster accepts the
        // provisioning or the teardown it asks for at this instant, and the answer is
        // rendered as of it (see `crate::state::StageEntry::cache_cluster_status_at`).
        let accepted_at = ::std::time::Instant::now();
        // The store is locked for THIS block alone, which is the whole state change: what it
        // answers is owned, so the group the stage publishes is created below with no guard
        // held — a write into CloudWatch Logs is a call into another service.
        let (body, region, publishing_role) = {
            let mut store = state.lock();
            // The `(restApiId, stageName)` pair is resolved as ONE lookup for the stage the
            // URI addresses, so an identifier no stored API answers to is reported as the
            // stage.
            //
            // Patched on a COPY: a list whose later op is refused leaves the stored stage
            // untouched, rather than keeping the ops before it. The one refusal that is not
            // of the REQUEST but of the stage the request leaves — the duplicate method
            // settings below — keeps what the list applied, which is how a stage comes to
            // carry two spellings of one method at all.
            let (api, stage) = store
                .api_and_stage(&input.rest_api_id, &input.stage_name)
                .map_err(|message| {
                    UpdateStageErrorModeled::NotFoundException(NotFoundException {
                        message: Some(message),
                    })
                })?;
            let mut updated = stage.clone();
            let deployments: BTreeSet<String> = api.deployments.keys().cloned().collect();
            // The account singleton's CloudWatch Logs role, which a patch that turns this
            // stage's execution logging on is a configuration of: a role set to no ARN at all
            // is an account with none, exactly as `GetAccount` reports it.
            let logging_role_configured = store
                .account
                .cloudwatch_role_arn
                .as_deref()
                .is_some_and(|arn| !arn.trim().is_empty());
            // That same role as the PRINCIPAL the group API Gateway publishes is created by,
            // read here under the one guard this block takes and judged below with none held.
            let store_account_role = store.account.cloudwatch_role_arn.clone();
            // The account's client certificates, one of which a stage presents to its
            // integration endpoint.
            let client_certificates: BTreeSet<String> =
                store.client_certificates.keys().cloned().collect();
            let context = StagePatchContext {
                deployments: &deployments,
                client_certificates: &client_certificates,
                logging_role_configured,
                accepted_at,
            };

            for patch in &patches {
                apply_patch(&mut updated, patch, &context).map_err(patch_failure)?;
            }
            // The stage this patch list LEAVES: one method's settings under two spellings of
            // its method path, which a stage cannot be updated while it carries — see
            // [`duplicate_method_settings`]. Judged on the whole list's result, so a list
            // that spells a method twice and then takes one of the two away leaves no
            // duplicate and is applied; what it leaves STAYS, which is what gives the caller
            // the two entries the sentence asks it to remove one of, and what makes every
            // later update of this stage answer the same sentence until one of them is gone.
            //
            // A refused update is no successful update: it moves `lastUpdatedDate` no more
            // than it reports a stage, and it publishes no execution log group — the stage's
            // own group is published by the update that is ANSWERED with the configuration
            // ([`provisioned_execution_log_group`]), which is the next one this stage takes.
            let duplicates = duplicate_method_paths(&updated);
            if !duplicates.is_empty() {
                let message = duplicate_method_settings(&duplicates, &input.rest_api_id);
                let api = store
                    .rest_apis
                    .get_mut(&input.rest_api_id)
                    .ok_or_else(crate::state::state_unavailable)?;
                api.stages.insert(input.stage_name.clone(), updated);
                return Err(patch_failure(PatchFailure::BadRequest(message)));
            }
            // A successful patch is when the record last changed. An empty list changes
            // nothing, so it moves nothing.
            if !patches.is_empty() {
                updated.last_updated_date = Some(now);
            }

            let api = store
                .rest_apis
                .get_mut(&input.rest_api_id)
                .ok_or_else(crate::state::state_unavailable)?;
            // Rendered through the one `Stage` body this crate builds, from the record about
            // to be stored, so the update reports the stage exactly as `GetStage` will read
            // it back.
            let body = super::create_stage::stage_body_as_of(&updated, None, accepted_at);
            let region = api.arn_region().to_string();
            // Whether the stage this patch leaves behind has its EXECUTION logging on, which
            // is what API Gateway publishes the stage's own log group for
            // ([`provisioned_execution_log_group`]). Read off the record about to be stored,
            // so it is the configuration now in force rather than what any one patch asked
            // for — and answered as the ROLE the group would be published as, which is the
            // principal that create is made by.
            let publishing_role = if logging_role_configured && stage_logs_executions(&updated) {
                store_account_role
            } else {
                None
            };
            api.stages.insert(input.stage_name.clone(), updated);
            (body, region, publishing_role)
        };
        let output = UpdateStageOutput {
            deployment_id: body.deployment_id,
            client_certificate_id: body.client_certificate_id,
            stage_name: body.stage_name,
            description: body.description,
            cache_cluster_enabled: body.cache_cluster_enabled,
            cache_cluster_size: body.cache_cluster_size,
            cache_cluster_status: body.cache_cluster_status,
            method_settings: body.method_settings,
            variables: body.variables,
            documentation_version: body.documentation_version,
            access_log_settings: body.access_log_settings,
            canary_settings: body.canary_settings,
            tracing_enabled: body.tracing_enabled,
            web_acl_arn: None,
            tags: body.tags,
            created_date: body.created_date,
            last_updated_date: body.last_updated_date,
        };
        // The stage's OWN execution log group, for a stage whose execution logging this
        // configuration leaves on: a group API Gateway publishes rather than one it was
        // pointed at, so it is there before the stage serves anything.
        if let Some(role_arn) = &publishing_role {
            provisioned_execution_log_group(
                &*self.engine,
                ctx,
                &region,
                &input.rest_api_id,
                &input.stage_name,
                role_arn,
            )
            .await;
        }
        // "The ARN of the WebAcl associated with the Stage": WAFv2's own record, read
        // with this service's store released, so the update reports the web ACL in front
        // of the stage exactly as `GetStage` reads it back.
        let output = UpdateStageOutput {
            web_acl_arn: crate::state::associated_web_acl_arn(
                &*self.engine,
                &region,
                &input.rest_api_id,
                &input.stage_name,
            ),
            ..output
        };
        // The access record for a state-changing operation: who, what, and the outcome.
        ::tracing::info!(
            target: crate::state::AUDIT_TARGET,
            operation = Self::NAME,
            caller = ?crate::state::request_caller(ctx.parts()),
            rest_api_id = ?input.rest_api_id,
            stage_name = ?input.stage_name,
            patch_operations = patches.len(),
            outcome = "updated",
            "stage updated",
        );
        Ok(output)
    }
}

/// Whether the stage `stage` carries a method setting that turns its EXECUTION logging on.
///
/// `MethodSetting$loggingLevel` is the setting ("valid values are `OFF`, `ERROR`, and
/// `INFO`"), and a stage's execution log is configured per METHOD scope, so the stage's log is
/// on as soon as ANY scope it carries — `*/*` as much as one method — is at a level that
/// writes a trace. The reading of "this level writes a trace" is the data plane's own
/// ([`crate::service::invoke::stage_logs::logs_executions`]), called rather than restated, so
/// the group this operation publishes and the lines that go into it cannot disagree about
/// which configuration turns the log on.
fn stage_logs_executions(stage: &crate::state::StageEntry) -> bool {
    stage.method_settings.values().any(|setting| {
        crate::service::invoke::stage_logs::logs_executions(setting.logging_level.as_deref())
    })
}

/// Bring the stage's own EXECUTION LOG GROUP into existence, for a stage whose execution
/// logging is on.
///
/// `API-Gateway-Execution-Logs_{rest-api-id}/{stage-name}` is a group API Gateway publishes
/// under its OWN name for every stage it writes an execution log for — unlike an access-log
/// `destinationArn`, which names a group of the account's — so it exists from the moment the
/// configuration that turns that log on is accepted, before the stage has served any request:
/// a client reads the group to confirm logging is on, and attaches a retention policy or a
/// subscription filter to it, without having to send traffic first. The group is created where
/// CloudWatch Logs does not already hold it, and the traces the stage goes on to deliver land
/// in that same group ([`crate::service::invoke::stage_logs`]).
///
/// API Gateway writes the account's logs as the account's own CloudWatch role
/// (`Account$cloudwatchRoleArn`), which is what every patch that turns this logging on
/// requires ([`ACCOUNT_CLOUDWATCH_ROLE_REQUIRED`]), so an account with no role has nothing for
/// API Gateway to publish the group as and none is created. The create is a call made BY that
/// role, so a role whose policies say no about creating this group publishes no group either
/// ([`crate::state::log_delivery_role`]) — and the caller is answered all the same.
///
/// Best effort, and called with no store lock held: the stage has already been updated, and
/// the caller is answered whatever CloudWatch Logs does with the create.
async fn provisioned_execution_log_group<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    api_id: &str,
    stage_name: &str,
    role_arn: &str,
) {
    let group = crate::service::invoke::stage_logs::execution_log_group(api_id, stage_name);
    if crate::state::log_delivery_role::group_creation_denied(engine, role_arn, region, &group) {
        return;
    }
    ::service_common::cloudwatch_logs::provisioned(ctx, region, &group).await;
}

/// The modelled failure for a patch this operation cannot apply.
fn patch_failure(failure: PatchFailure) -> UpdateStageError {
    match failure {
        PatchFailure::BadRequest(message) => {
            UpdateStageErrorModeled::BadRequestException(BadRequestException {
                message: Some(message),
            })
        }
        PatchFailure::NotFound(message) => {
            UpdateStageErrorModeled::NotFoundException(NotFoundException {
                message: Some(message),
            })
        }
        PatchFailure::Conflict(message) => {
            UpdateStageErrorModeled::ConflictException(ConflictException {
                message: Some(message),
            })
        }
    }
    .into()
}
