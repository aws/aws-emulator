// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2 service.
//!
//! Every record below belongs to ONE region: campaigns and Connect instance
//! configurations are regional resources, reachable only through the endpoint of the
//! region they were created in, so the store keeps one set of records per
//! credential-scope region ([`request_region`]) and every lookup, listing and write
//! takes the region it is serving and resolves inside that region alone.
//!
//! The campaign store is the single source of truth for the campaign lifecycle:
//! `CreateCampaign` writes a record here, `DescribeCampaign` / `ListCampaigns` /
//! `GetCampaignState` project it, `Start`/`Pause`/`Resume`/`StopCampaign` move its
//! [`CampaignState`], and `DeleteCampaign` removes it. No handler keeps a copy of
//! its own, so every response describes what the store holds.
//!
//! A campaign's state is owed to the CLOCK as well as to those calls: a campaign
//! that has entered its run is `Completed` once its schedule's `endTime` has
//! passed, which [`StoredCampaign::state_at`] derives from the stored schedule on
//! every read, so no reader reports a state the schedule has already moved past.
//!
//! The instance-onboarding store beside it is the single source of truth for which
//! Connect instances are onboarded to Campaigns: `StartInstanceOnboardingJob` writes a
//! record here with the encryption config it was given,
//! `GetInstanceOnboardingJobStatus` reports the status that record is at,
//! `GetConnectInstanceConfig` projects the config it holds, and `CreateCampaign`
//! consults it before minting a campaign — an instance with no finished onboarding job
//! is not a campaign target. The instance-wide communication limits live on that same
//! record: `PutInstanceCommunicationLimits` writes them and
//! `GetInstanceCommunicationLimits` reports them, so neither keeps a copy of its own.
//!
//! An onboarding job is owed to the CLOCK like a campaign's state, and it settles on
//! the outcome the call that started it determined: the key its encryption config names
//! is resolved in the KMS store ([`onboarding_encryption_failure`]) when the job starts,
//! so a job the service cannot finish settles `FAILED` carrying the
//! `InstanceOnboardingJobFailureCode` of the dependency that stopped it, which is the
//! record `DeleteInstanceOnboardingJob` clears.
//!
//! The integration store is the single source of truth for what an onboarded Connect
//! instance is integrated with: `PutConnectInstanceIntegration` writes one record per
//! integration the instance holds, keyed by the kind and resource ARN that identify
//! it, `ListConnectInstanceIntegrations` projects those records and
//! `DeleteConnectInstanceIntegration` removes the one its identifier names.
//!
//! `DeleteConnectInstanceConfig` is the other end of onboarding: it removes an
//! instance's record from the onboarding store together with everything held on that
//! configuration — its communication limits and its integrations — so the instance is
//! no longer onboarded and no reader reports a configuration for it. The campaigns of
//! the instance go with it only when the request's campaign deletion policy says so.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::SystemTime;

use crate::ops::types::{
    Campaign, CampaignSummary, ChannelSubtypeConfig, ChannelSubtypeParameters, CommunicationLimits,
    CommunicationLimitsConfig, CommunicationTimeConfig, CustomerProfilesIntegrationConfig,
    CustomerProfilesIntegrationIdentifier, CustomerProfilesIntegrationSummary,
    EmailChannelSubtypeParameters, EncryptionConfig, EntryLimitsConfig, EventTrigger,
    EventTriggerContext, InstanceCommunicationLimitsConfig, InstanceConfig, InstanceOnboardingJobStatus,
    IntegrationConfig, IntegrationIdentifier, IntegrationSummary, InternalServerException,
    LambdaIntegrationConfig, LambdaIntegrationIdentifier, LambdaIntegrationSummary,
    OpenHours, QConnectIntegrationConfig, QConnectIntegrationIdentifier, QConnectIntegrationSummary,
    Schedule, SmsChannelSubtypeParameters, Source, TelephonyChannelSubtypeParameters, TimeWindow,
    WhatsAppChannelSubtypeParameters, DAY_OF_WEEK_VALUES, EVENT_TYPE_VALUES,
};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, as the
/// Smithy model's `aws.api#service` trait declares it.
pub const ARN_NAMESPACE: &str = "connect-campaigns";

/// The region an ARN is rooted in when a request carries no SigV4 credential
/// scope to read one from (an anonymous or host-routed call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for: the SigV4 credential scope's region,
/// which is the region every ARN minted while serving it belongs to.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a campaign: `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`.
///
/// Built through [`emulator_server::emulator_arn`] so the account segment is the
/// emulator's canonical placeholder account rather than a literal.
pub fn campaign_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("campaign/{id}"))
}

/// The resource-type segment of the only ARN family this service's tag operations
/// recognise: a campaign, the one taggable resource its model mints an ARN for
/// (`CampaignArn`, built by [`campaign_arn`]).
pub const CAMPAIGN_RESOURCE_TYPE: &str = "campaign";

/// The wire error code ConnectCampaignsV2 answers a tag operation with when the ARN is
/// well formed but names no resource this account holds, at HTTP 404.
///
/// The recorded responses carry this code, which is NOT the name of the model's
/// corresponding shape (`ResourceNotFoundException`); a client branches on the code it
/// is sent, so the emulator sends this one through
/// [`crate::scaffolded::Scaffolded::wire_coded`].
pub const TAG_NOT_FOUND_CODE: &str = "NotFoundException";

/// The wire error code ConnectCampaignsV2 answers a tag operation with when the ARN's
/// resource path is not one of its own resources, at HTTP 400 — refused before any
/// existence check. Recorded in the same way as [`TAG_NOT_FOUND_CODE`], and not the
/// name of the model's corresponding shape (`ValidationException`).
pub const TAG_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The refusal a tag operation answers for an ARN whose resource path is not one of
/// this service's own: HTTP 400 under [`TAG_BAD_REQUEST_CODE`], with the sentence the
/// recorded responses carry. Reached through [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::tagged_campaign`],
/// which is what decides when it is answered.
fn tag_arn_bad_request<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::wire_coded(
        TAG_BAD_REQUEST_CODE,
        400,
        "Invalid input resource arn",
    )
}

/// The refusal a tag operation answers for a well-formed campaign ARN the signed-for
/// region stores nothing under: HTTP 404 under [`TAG_NOT_FOUND_CODE`], with the sentence
/// the recorded responses carry.
fn tag_arn_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::wire_coded(TAG_NOT_FOUND_CODE, 404, "Resource was not found")
}

/// The most tags one resource holds.
///
/// Source: `TagKeyList`, the shape `UntagResource$tagKeys` takes, whose `@length` is
/// 0..50 — a resource whose every tag can be removed in one call holds at most 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The shortest tag key this service accepts.
///
/// Source: the `@length` of `TagKey`, the shape every tag map's key and every tag-key
/// list's member targets, whose minimum is 1.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// The longest tag key this service accepts, from the same `@length`, whose maximum
/// is 128.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// `TagKey`'s modelled `@pattern`, quoted exactly as the model states it so a refusal
/// names the expression the key failed.
pub const TAG_KEY_PATTERN: &str = "^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The key prefix [`TAG_KEY_PATTERN`]'s leading negative lookahead excludes: a
/// namespace the service reserves for keys of its own, so a caller cannot supply one.
const TAG_KEY_RESERVED_PREFIX: &str = "aws:";

/// Whether every character of `key` is one [`TAG_KEY_PATTERN`]'s character class
/// admits: an ASCII letter, `_`, or a character of the `+`..`=` range that class
/// spells (`+ , - . / 0-9 : ; < =`).
fn tag_key_characters_allowed(key: &str) -> bool {
    key.chars()
        .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '_' | '+'..='='))
}

/// The longest caller-supplied text a refusal message echoes, pinned to the boundary
/// validator's own ceiling (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`), so a huge request member cannot be
/// amplified into a huge response.
///
/// The framework's own eliding helper is private and lives under the agent-immutable
/// `vendor/` tree, so it cannot be called from here; the ceiling is pinned to the same
/// figure instead.
const MAX_ECHOED_VALUE: usize = 256;

/// The most clauses one refusal message carries, pinned to the boundary validator's
/// own ceiling (`MAX_VIOLATIONS`): a request may carry arbitrarily many tag keys, and
/// a message that named every bad one would grow with the request.
const MAX_REPORTED_VIOLATIONS: usize = 100;

/// `text` as a refusal message may carry it: itself when short, a
/// character-boundary-safe prefix with an elision when not.
fn elided(text: &str) -> String {
    if text.len() <= MAX_ECHOED_VALUE {
        return text.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &text[..end], text.len())
}

/// Why `key` is not a key `TagKey` admits, or `None` when it is one.
///
/// The one place the key shape's `@length` and `@pattern` are read, so a key in a tag
/// MAP and a key in a tag-key LIST are held to the same bounds.
fn tag_key_violation_kind(key: &str) -> Option<::smithy_server_runtime::ViolationKind> {
    let length = key.chars().count();
    if length < TAG_KEY_MIN_LENGTH {
        Some(::smithy_server_runtime::ViolationKind::MinLength(
            TAG_KEY_MIN_LENGTH as i64,
        ))
    } else if length > TAG_KEY_MAX_LENGTH {
        Some(::smithy_server_runtime::ViolationKind::MaxLength(
            TAG_KEY_MAX_LENGTH as i64,
        ))
    } else if key.starts_with(TAG_KEY_RESERVED_PREFIX) || !tag_key_characters_allowed(key) {
        Some(::smithy_server_runtime::ViolationKind::Pattern(
            TAG_KEY_PATTERN.to_string(),
        ))
    } else {
        None
    }
}

/// Why the keys of the MAP at `member` are a bad request, each judged by
/// `violation_kind`, or `None` when that finds nothing wrong with any of them.
///
/// A map's KEY never reaches the boundary walk: it captures an entry's key as the path
/// segment of that entry's value rather than validating it as a leaf of its own. So
/// every key shape this service has to weigh itself goes through here, which is the one
/// place the shape of such a refusal is decided — which keys are reported, in which
/// order, how an offending key is named and how much of it is echoed. The sentence is
/// built from the framework's own [`::smithy_server_runtime::Violation`] and rendered by
/// [`crate::service::validation::stated`] — the one place that wording
/// lives — so a handler refusal and a boundary refusal read identically.
fn map_keys_violation<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
    violation_kind: impl Fn(&str) -> Option<::smithy_server_runtime::ViolationKind>,
) -> Option<String> {
    // Sorted and deduplicated: a `HashMap` iterates in an arbitrary order, and the
    // same request must be refused with the same sentence every time.
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.dedup();

    let violations: Vec<::smithy_server_runtime::Violation> = keys
        .into_iter()
        .filter_map(|key| {
            let kind = violation_kind(key)?;
            // The key embeds caller-supplied text, so it is elided BEFORE the path is
            // composed from it: eliding the composed path instead would cut the
            // trailing collection step off a path a huge key had filled.
            let echoed = elided(key);
            Some(::smithy_server_runtime::Violation {
                // The path names the offending key, the spelling the boundary walk's
                // own path builder gives a map entry.
                path: format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::entry(&echoed)
                ),
                kind,
                // `Violation::value` must arrive ALREADY elided: the message
                // interpolates it verbatim.
                value: Some(echoed),
                sensitive: false,
            })
        })
        .take(MAX_REPORTED_VIOLATIONS)
        .collect();

    crate::service::validation::stated(&violations)
}

/// Why the tag keys of the MAP at `member` are a bad request, or `None` when every one
/// of them is a key `TagKey` admits.
///
/// Tag keys reach a handler UNCHECKED, for two reasons that compound: the boundary
/// walk captures a map's key as the path segment of that entry's value rather than
/// validating it as a leaf of its own, and `TagKey`'s `@pattern` carries a leading
/// negative lookahead the pattern engine cannot compile, which it then treats as
/// matching. So this is the one place the key shape's `@length` and `@pattern` are
/// enforced, shared by every operation that takes a tag map; [`map_keys_violation`]
/// turns what it finds into the refusal.
pub fn tag_keys_violation<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    map_keys_violation(member, keys, tag_key_violation_kind)
}

/// Why the tag keys of the LIST at `member` are a bad request, or `None` when every one
/// of them is a key `TagKey` admits.
///
/// Same bounds as [`tag_keys_violation`], read from the same place, differing only in
/// how an offending element is named: a list member identifies itself by its 1-based
/// POSITION (`<member>.<position>.member`), the spelling the boundary walk's own path
/// builder produces, so a refusal points the caller at the element it sent. A key the
/// `@pattern` rejects reaches a handler even in a list, because the pattern's leading
/// negative lookahead is one the pattern engine cannot compile and so treats as
/// matching.
pub fn tag_key_list_violation(member: &str, keys: &[String]) -> Option<String> {
    let violations: Vec<::smithy_server_runtime::Violation> = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| {
            let kind = tag_key_violation_kind(key)?;
            Some(::smithy_server_runtime::Violation {
                path: format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::element(index + 1)
                ),
                kind,
                // `Violation::value` must arrive ALREADY elided: the message
                // interpolates it verbatim.
                value: Some(elided(key)),
                sensitive: false,
            })
        })
        .take(MAX_REPORTED_VIOLATIONS)
        .collect();

    crate::service::validation::stated(&violations)
}

/// Why leaving the resource at `member` holding `count` tags is a bad request, or
/// `None` when that many is within [`MAX_TAGS_PER_RESOURCE`].
///
/// The one place that ceiling is enforced, so every path that can leave a resource
/// holding tags — the create that is born with a tag map, the merge that adds to one —
/// refuses at the same count and with the same sentence, and no resource can come to
/// hold a map no tag call could have produced.
pub fn tag_count_violation(member: &str, count: usize) -> Option<String> {
    if count <= MAX_TAGS_PER_RESOURCE {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::MaxLength(MAX_TAGS_PER_RESOURCE as i64),
        // The `<n> entries` form the framework displays a map's own length with.
        value: Some(format!("{count} entries")),
        sensitive: false,
    };
    crate::service::validation::stated(std::slice::from_ref(&violation))
}

/// The campaign id an `/v2/tags/{arn}` ARN names, or `None` when `arn` does not name
/// a connect-campaigns campaign at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; the resource
/// portion may itself contain `:`, so only the first five fields are split off. The
/// ARN names a campaign when its service field is this service's ARN namespace and its
/// resource portion is `campaign/<id>` with a non-empty id. Anything else — another
/// service's ARN, an unknown resource type, a bare resource with no id — is a resource
/// path this service cannot resolve, which its tag operations refuse as a bad request
/// BEFORE asking whether any resource exists.
pub fn campaign_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let ("arn", Some(_partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next()?,
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if service != ARN_NAMESPACE {
        return None;
    }
    let (resource_type, id) = resource.split_once('/')?;
    (resource_type == CAMPAIGN_RESOURCE_TYPE && !id.is_empty()).then_some(id)
}

/// The most entries a `communicationLimitsList` carries.
///
/// Source: the `@length` of `CommunicationLimits$communicationLimitsList`, whose
/// maximum is 2.
pub const MAX_COMMUNICATION_LIMITS: usize = 2;

/// Why a communication-limits config is a bad request, or `None` when it is within
/// the model's bounds.
///
/// The one place this service checks a `CommunicationLimits` union against the
/// `@length` its list member carries, so every operation that accepts one
/// (`CreateCampaign`, `UpdateCampaignCommunicationLimits`,
/// `PutInstanceCommunicationLimits`) refuses an over-long list the same way. `member`
/// is the request path the refusal names, so the message is built from the request
/// rather than fixed text, and the sentence itself is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// [`crate::service::validation::stated`] — the one place that wording
/// lives — so this handler refusal and a boundary refusal of the same `@length` read
/// identically.
///
/// The list's LENGTH is the whole of what is weighed here, and that is not a partial
/// compensation: the ELEMENTS arrive already checked. The walk descends through the
/// union member into each `CommunicationLimit` and judges it against that structure's
/// generated schema, whose member schemas carry the model's own constraints — the
/// `@range` of `maxCountPerRecipient` and of `frequency`, and the
/// `CommunicationLimitTimeUnit` catalog of `unit` — reporting a failure at
/// `<member>.<position>.member.<name>`. Only the union MEMBER's own traits are missing
/// from that walk (a union's generated member schemas carry none), which is exactly the
/// `@length` below. Pinned end to end by `tests/all/communication_limit_range.rs`.
pub fn communication_limits_violation(
    member: &str,
    limits: Option<&CommunicationLimits>,
) -> Option<String> {
    let count = match limits? {
        CommunicationLimits::communicationLimitsList(list) => list.len(),
    };
    if count <= MAX_COMMUNICATION_LIMITS {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::MaxLength(MAX_COMMUNICATION_LIMITS as i64),
        // The `<n> items` form the framework displays a LIST's own length with (a map's
        // is `<n> entries`, which is what [`tag_count_violation`] shows).
        value: Some(format!("{count} items")),
        sensitive: false,
    };
    crate::service::validation::stated(std::slice::from_ref(&violation))
}

/// Why the keys of the MAP at `member` are a bad request, or `None` when every one of
/// them is a value the key shape's catalog (`allowed`) holds.
///
/// The one place an enum-keyed map's value set is enforced, shared by every operation
/// that accepts such a map — a key the walk never validates, refused through
/// [`map_keys_violation`] exactly as a tag key is.
pub fn enum_map_keys_violation<'k>(
    member: &str,
    allowed: &[&str],
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    map_keys_violation(member, keys, |key| {
        (!allowed.contains(&key)).then(|| {
            ::smithy_server_runtime::ViolationKind::EnumValue(
                allowed.iter().map(|value| (*value).to_string()).collect(),
            )
        })
    })
}

/// Why the SCALAR at `member` is a bad request, or `None` when the request leaves it
/// out or it holds a value the member's own catalog (`allowed`) holds.
///
/// The scalar counterpart of [`enum_map_keys_violation`], and with it the one place
/// this service weighs a value against an enum catalog itself: every operation that
/// keeps its own contract for an enum member — the `channelSubtype` of the
/// channel-subtype-config deletion, the `config` of the communication-limits and
/// communication-time deletions, the `campaignDeletionPolicy` of the instance-config
/// deletion and the encryption type of an onboarding job's encryption config — refuses
/// an off-catalog value through here, so the one rule is worded one way however many
/// operations hold to it, whether the member is required or optional: a member the
/// request never sent is nothing to weigh.
///
/// A backstop rather than the first line of defence: each of those members targets a
/// modelled enum, so the boundary walk refuses an off-catalog value before a handler
/// runs. Because the sentence here is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// [`crate::service::validation::stated`] — the one place that wording
/// lives — the two layers refuse the same value with the same sentence, so which of
/// them answered is not something a caller can tell.
pub fn enum_value_violation(member: &str, allowed: &[&str], value: Option<&str>) -> Option<String> {
    let value = value?;
    if allowed.contains(&value) {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::EnumValue(
            allowed.iter().map(|allowed| (*allowed).to_string()).collect(),
        ),
        // `Violation::value` must arrive ALREADY elided: the message interpolates it
        // verbatim.
        value: Some(elided(value)),
        sensitive: false,
    };
    crate::service::validation::stated(std::slice::from_ref(&violation))
}

/// Why a communication-time config is a bad request, or `None` when every day its
/// windows open is one the model's `DayOfWeek` catalog holds (`DAY_OF_WEEK_VALUES`
/// in [`crate::ops::types`]).
///
/// The one place this service checks a `CommunicationTimeConfig`'s `dailyHours`
/// keys, so every operation that accepts one (`CreateCampaign`,
/// `UpdateCampaignCommunicationTime`) refuses a day naming no weekday the same way.
/// A map key is one the boundary walk never validates, so without this check a
/// campaign could be stored opening a day the model does not model; the catalog
/// itself is weighed by [`enum_map_keys_violation`], shared with every other
/// enum-keyed map this service accepts. `member` is the request path the refusal
/// names, so the message is built from the request rather than fixed text.
///
/// A key is the ONLY leaf of a window this has to weigh: everything else behind the
/// `OpenHours` and `RestrictedPeriods` unions is a structure, and the boundary walk
/// descends into one — a union's own member schema carries no constraint traits, but
/// the generated member schemas of the structure behind it do, so a `TimeRange` time
/// and a `RestrictedPeriod` date are already held to the `@pattern` of the shape they
/// target by the time a handler runs. `tests/all/communication_time_window_constraints.rs`
/// pins that end to end, so a codegen change that stopped enforcing them fails there.
pub fn communication_time_violation(
    member: &str,
    config: &CommunicationTimeConfig,
) -> Option<String> {
    // Every per-channel window `CommunicationTimeConfig` carries, under the member
    // name a refusal reports it as.
    let windows: [(&str, Option<&TimeWindow>); 4] = [
        ("telephony", config.telephony.as_ref()),
        ("sms", config.sms.as_ref()),
        ("email", config.email.as_ref()),
        ("whatsApp", config.whats_app.as_ref()),
    ];
    for (channel, window) in windows {
        let Some(window) = window else {
            continue;
        };
        let OpenHours::dailyHours(daily_hours) = &window.open_hours;
        if let Some(message) = enum_map_keys_violation(
            &format!("{member}.{channel}.openHours.dailyHours"),
            DAY_OF_WEEK_VALUES,
            daily_hours.keys().map(String::as_str),
        ) {
            return Some(message);
        }
    }
    None
}

// =============================================================================
// The ARNs this service's unions carry
// =============================================================================
//
// A constrained member reaches a handler already checked — unless the UNION MEMBER
// SCHEMA is the only thing that could have carried the constraint, because the
// generated member schemas of a union carry no constraint traits and so the
// centralized walk has nothing to enforce there. That is the case for a union member
// that is a bare STRING, and for one whose own `@length` bounds a list (the reason
// [`communication_limits_violation`] above exists). Where the union member is a
// STRUCTURE, a LIST of structures or a MAP of them, the walk descends and judges each
// structure against its own generated schema, whose member schemas DO carry the
// model's `@length`/`@pattern`, so those leaves arrive checked — a map KEY stays the
// exception, which is what [`communication_time_violation`] above weighs. The value
// behind the union member is judged HERE by that very walk, started one level down:
// `aws_smithy_schema_server::constraints::validate` over a structure behind a union
// member, and the walk's `ValidatingSerializer` over the one union member that is a
// bare string. Same rules and same wording as the boundary, so no record can come to
// hold — and no read project — an ARN the model's own shape forbids.
//
// `ChannelSubtypeConfig` is the union-borne case that rule settles with NO check here,
// which is why no helper for it stands beside the ones above: every member of the four
// `outboundMode` unions, and of the `PacingStrategy` union one level deeper, is a
// STRUCTURE carrying no constraint trait on the union member itself, so the centralized
// walk descends to every leaf behind them — the bandwidth-allocation, target-rate and
// timeout ranges, the pacing-strategy list's `@length`, the agent-action and
// connection-start-point catalogs, the evaluation window's `@pattern` and the
// `@required` members at each depth — and the operations that accept such a config
// (`CreateCampaign`, `UpdateCampaignChannelSubtypeConfig`) are handed one already
// checked. `tests/all/channel_subtype_config_is_one_rule.rs` pins that end to end
// through both of them, so a codegen change that stopped descending past a union
// member fails there rather than letting a campaign come to hold a config the model's
// own shape forbids.

/// The modelled `Arn` rules, from `smithy_query(query="shape", shape="Arn")`:
/// `@length(min: 20, max: 500)` with the ARN `@pattern`.
///
/// Restated here for `Source$customerProfilesSegmentArn` alone — the one ARN behind a
/// union that is a BARE STRING, so there is no generated structure schema carrying
/// those traits to walk instead, and [`SEGMENT_ARN_SCHEMA`] gives them back. Pinned to
/// the model by
/// `tests/all/union_member_arn_constraints.rs::the_arn_rules_this_crate_restates_are_the_models`,
/// which reads these three values out of `smithy-model/` and fails if they drift.
pub const ARN_MIN_LEN: i64 = 20;
/// See [`ARN_MIN_LEN`].
pub const ARN_MAX_LEN: i64 = 500;
/// See [`ARN_MIN_LEN`].
pub const ARN_PATTERN: &str =
    r"^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-z]{2}-[a-z]+-\d{1,2}:[a-zA-Z0-9-]+:[^:]+(?:/[^:]+)*(?:/[^:]+)?(?:\:[^:]+)?$";

/// `Arn`'s constraints as the trait map the framework's constraint walk reads them
/// from, built from [`ARN_MIN_LEN`] and its siblings so the rules enforced and the
/// rules the refusal states are one statement.
static SEGMENT_ARN_TRAITS: LazyLock<::aws_smithy_schema::TraitMap> = LazyLock::new(|| {
    let mut traits = ::aws_smithy_schema::TraitMap::new();
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::LengthTrait::new(
            Some(ARN_MIN_LEN),
            Some(ARN_MAX_LEN),
        ),
    ));
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::PatternTrait::new(ARN_PATTERN),
    ));
    traits
});

/// The member schema the walk judges `Source$customerProfilesSegmentArn` against: one
/// string member carrying `Arn`'s own constraints, since the union's generated member
/// schema carries none.
static SEGMENT_ARN_SCHEMA: LazyLock<::aws_smithy_schema::Schema> = LazyLock::new(|| {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.connectcampaignsv2#Source$customerProfilesSegmentArn",
            "com.amazonaws.connectcampaignsv2",
            "customerProfilesSegmentArn",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "customerProfilesSegmentArn",
        0,
    )
    .with_traits(&SEGMENT_ARN_TRAITS)
});

/// `violations` as the refusal message a handler answers with, every path prefixed by
/// `member` — the request path the walk below was started beneath, so the clause names
/// the member as the CALLER spelled it rather than as the sub-walk saw it.
///
/// Rendered by [`crate::service::validation::stated`] — the one place that
/// wording lives — so a refusal from inside a handler reads exactly like one from the
/// boundary.
fn union_member_refusal(
    member: &str,
    mut violations: Vec<::smithy_server_runtime::Violation>,
) -> Option<String> {
    violations.truncate(MAX_REPORTED_VIOLATIONS);
    for violation in &mut violations {
        violation.path = format!("{member}.{}", violation.path);
    }
    crate::service::validation::stated(&violations)
}

/// The refusal for the bare string `value` that arrived at `member`, judged against
/// the member `schema` by the framework's own walk, or `None` when the shape admits
/// it.
fn string_member_refusal(
    member: &str,
    schema: &::aws_smithy_schema::Schema,
    value: &str,
) -> Option<String> {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    // Infallible: the validator accumulates violations instead of failing.
    let _ = validator.write_string(schema, value);
    union_member_refusal(member, validator.into_violations())
}

/// The refusal for the structure that arrived at `member`, judged against its own
/// generated `schema` — whose member schemas carry every constraint the model
/// declares on them — or `None` when it satisfies all of them.
fn structure_refusal(
    member: &str,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
) -> Option<String> {
    union_member_refusal(
        member,
        ::aws_smithy_schema_server::constraints::validate(value, schema),
    )
}

/// Why the `Source` at `member` is a bad request, or `None` when the value the union
/// member in force carries satisfies every constraint its shape declares.
///
/// The one place a campaign's source is judged, so every operation that sets one
/// (`CreateCampaign`, `UpdateCampaignSource`) refuses the same value the same way and
/// no stored campaign projects a source the model forbids. `member` is the request
/// path the refusal names, so the message is built from the request rather than fixed
/// text.
pub fn source_violation(member: &str, source: &Source) -> Option<String> {
    match source {
        // A bare string behind the union: judged against the member schema that
        // carries `Arn`'s own constraints.
        Source::customerProfilesSegmentArn(arn) => {
            string_member_refusal(member, &SEGMENT_ARN_SCHEMA, arn)
        }
        // A structure behind the union: its generated member schemas carry the
        // model's constraints, so the walk judges it as the boundary would have —
        // including the optional domain ARN, absent from a trigger that carries none.
        Source::eventTrigger(trigger) => structure_refusal(
            &format!("{member}.eventTrigger"),
            trigger,
            &EventTrigger::SCHEMA,
        ),
    }
}

/// Why the `IntegrationConfig` at `member` is a bad request, or `None` when the
/// structure the union member in force carries satisfies every constraint its shape
/// declares — `Arn` for the Customer Profiles domain and the Q Connect knowledge
/// base, `LambdaArn` for the Lambda function, and the `EventType` catalog for the keys
/// of the Customer Profiles `objectTypeNames` map.
///
/// A structure's leaves are judged by the framework's own walk, which never validates
/// a map's KEY, so the catalog an enum-keyed map is drawn from is weighed beside it
/// through [`enum_map_keys_violation`] — without which an instance's integration could
/// come to hold, and `ListConnectInstanceIntegrations` to report, a key the summary's
/// own key shape does not admit.
pub fn integration_config_violation(member: &str, config: &IntegrationConfig) -> Option<String> {
    match config {
        IntegrationConfig::customerProfiles(profiles) => structure_refusal(
            &format!("{member}.customerProfiles"),
            profiles,
            &CustomerProfilesIntegrationConfig::SCHEMA,
        )
        .or_else(|| {
            enum_map_keys_violation(
                &format!("{member}.customerProfiles.objectTypeNames"),
                EVENT_TYPE_VALUES,
                profiles.object_type_names.keys().map(String::as_str),
            )
        }),
        IntegrationConfig::qConnect(q_connect) => structure_refusal(
            &format!("{member}.qConnect"),
            q_connect,
            &QConnectIntegrationConfig::SCHEMA,
        ),
        IntegrationConfig::Lambda(lambda) => structure_refusal(
            &format!("{member}.lambda"),
            lambda,
            &LambdaIntegrationConfig::SCHEMA,
        ),
    }
}

/// Why the `IntegrationIdentifier` at `member` is a bad request, or `None` when the
/// structure the union member in force carries satisfies every constraint its shape
/// declares.
///
/// The same rules the config is judged by, read from the same schemas: an identifier
/// names the integration a config created, so a value one refuses cannot name an
/// integration the other could have stored.
///
/// Weighed before the instance or the record the identifier names is looked up, so a
/// value no integration could have been stored under is the bad request its shape
/// makes it rather than a missing resource — pinned by
/// `tests/all/integration_identifier_precedes_lookup.rs`.
pub fn integration_identifier_violation(
    member: &str,
    identifier: &IntegrationIdentifier,
) -> Option<String> {
    match identifier {
        IntegrationIdentifier::customerProfiles(profiles) => structure_refusal(
            &format!("{member}.customerProfiles"),
            profiles,
            &CustomerProfilesIntegrationIdentifier::SCHEMA,
        ),
        IntegrationIdentifier::qConnect(q_connect) => structure_refusal(
            &format!("{member}.qConnect"),
            q_connect,
            &QConnectIntegrationIdentifier::SCHEMA,
        ),
        IntegrationIdentifier::Lambda(lambda) => structure_refusal(
            &format!("{member}.lambda"),
            lambda,
            &LambdaIntegrationIdentifier::SCHEMA,
        ),
    }
}

/// The modelled `AttributeName` rules, from
/// `smithy_query(query="shape", shape="AttributeName")`: `@length(min: 0, max: 32767)`
/// with that shape's `@pattern`.
///
/// Restated here because `AttributeName` is the KEY shape of a map (`Attributes`), and
/// a key is the one leaf no generated schema carries the constraints of: the codegen
/// hangs the VALUE shape's traits off the map member and describes the key nowhere, so
/// there is no schema to walk instead. Pinned to the model by
/// `tests/all/attribute_map_keys.rs::the_attribute_key_rules_this_crate_restates_are_the_models`,
/// which reads these three values out of `smithy-model/` and fails if they drift.
pub const ATTRIBUTE_NAME_MIN_LEN: i64 = 0;
/// See [`ATTRIBUTE_NAME_MIN_LEN`].
pub const ATTRIBUTE_NAME_MAX_LEN: i64 = 32767;
/// See [`ATTRIBUTE_NAME_MIN_LEN`].
pub const ATTRIBUTE_NAME_PATTERN: &str = r"^[a-zA-Z0-9\-_]+$";

/// `AttributeName`'s constraints as the trait map the framework's constraint walk reads
/// them from, built from [`ATTRIBUTE_NAME_MIN_LEN`] and its siblings so the rules
/// enforced and the rules a refusal states are one statement.
static ATTRIBUTE_NAME_TRAITS: LazyLock<::aws_smithy_schema::TraitMap> = LazyLock::new(|| {
    let mut traits = ::aws_smithy_schema::TraitMap::new();
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::LengthTrait::new(
            Some(ATTRIBUTE_NAME_MIN_LEN),
            Some(ATTRIBUTE_NAME_MAX_LEN),
        ),
    ));
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::PatternTrait::new(ATTRIBUTE_NAME_PATTERN),
    ));
    traits
});

/// The schema an attribute key is judged against: one string carrying
/// `AttributeName`'s own constraints, since the map member's generated schema carries
/// the value shape's and not the key's.
static ATTRIBUTE_NAME_SCHEMA: LazyLock<::aws_smithy_schema::Schema> = LazyLock::new(|| {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.connectcampaignsv2#Attributes$key",
            "com.amazonaws.connectcampaignsv2",
            "key",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "key",
        0,
    )
    .with_traits(&ATTRIBUTE_NAME_TRAITS)
});

/// Why `key` is not a value the string shape behind `schema` admits, or `None` when it
/// is one.
///
/// Judged by the framework's own leaf walk, so a key is weighed by the very code the
/// boundary weighs a constrained string member with, in the same order (`@length`
/// before `@pattern`); the first requirement it breaks is the one reported, as the
/// per-key clause of [`map_keys_violation`] carries exactly one.
fn string_key_violation_kind(
    schema: &::aws_smithy_schema::Schema,
    key: &str,
) -> Option<::smithy_server_runtime::ViolationKind> {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    validator.set_fail_fast(true);
    // Infallible: the validator accumulates violations instead of failing.
    let _ = validator.write_string(schema, key);
    validator
        .into_violations()
        .into_iter()
        .next()
        .map(|violation| violation.kind)
}

/// Why the keys of the attribute MAP at `member` are a bad request, or `None` when
/// every one of them is a key `AttributeName` admits.
///
/// The one place a map key whose shape declares a `@length` and a `@pattern` is
/// enforced, shared by every such map this service accepts — a telephony request's
/// `attributes` and the `templateParameters` of its SMS, email and WhatsApp siblings,
/// all four keyed by `AttributeName`. A map key never reaches the boundary walk, so
/// without this a request could name an attribute the key shape forbids and still be
/// taken; [`map_keys_violation`] turns what this finds into the refusal, so it reads
/// exactly like the enum-keyed and tag-keyed refusals beside it.
pub fn attribute_keys_violation<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    map_keys_violation(member, keys, |key| {
        string_key_violation_kind(&ATTRIBUTE_NAME_SCHEMA, key)
    })
}

/// Why the `ChannelSubtypeParameters` at `member` are a bad request, or `None` when
/// the parameters of the channel in force satisfy every constraint their shape
/// declares — the source-number and template ARNs among them, and the keys of the
/// attribute map every channel carries.
///
/// A structure's leaves are judged by the framework's own walk, which never validates
/// a map's KEY, so the key shape of the `Attributes` map behind each channel is weighed
/// beside them through [`attribute_keys_violation`] — without which a request could
/// name an attribute `AttributeName` does not admit and still be taken into a
/// campaign's buffer.
pub fn channel_subtype_parameters_violation(
    member: &str,
    parameters: &ChannelSubtypeParameters,
) -> Option<String> {
    match parameters {
        ChannelSubtypeParameters::Telephony(telephony) => structure_refusal(
            &format!("{member}.telephony"),
            telephony,
            &TelephonyChannelSubtypeParameters::SCHEMA,
        )
        .or_else(|| {
            attribute_keys_violation(
                &format!("{member}.telephony.attributes"),
                telephony.attributes.keys().map(String::as_str),
            )
        }),
        ChannelSubtypeParameters::Sms(sms) => structure_refusal(
            &format!("{member}.sms"),
            sms,
            &SmsChannelSubtypeParameters::SCHEMA,
        )
        .or_else(|| {
            attribute_keys_violation(
                &format!("{member}.sms.templateParameters"),
                sms.template_parameters.keys().map(String::as_str),
            )
        }),
        ChannelSubtypeParameters::Email(email) => structure_refusal(
            &format!("{member}.email"),
            email,
            &EmailChannelSubtypeParameters::SCHEMA,
        )
        .or_else(|| {
            attribute_keys_violation(
                &format!("{member}.email.templateParameters"),
                email.template_parameters.keys().map(String::as_str),
            )
        }),
        ChannelSubtypeParameters::whatsApp(whats_app) => structure_refusal(
            &format!("{member}.whatsApp"),
            whats_app,
            &WhatsAppChannelSubtypeParameters::SCHEMA,
        )
        .or_else(|| {
            attribute_keys_violation(
                &format!("{member}.whatsApp.templateParameters"),
                whats_app.template_parameters.keys().map(String::as_str),
            )
        }),
    }
}

/// The `ChannelSubtype` value a request's `channelSubtypeParameters` asks to reach
/// someone on.
///
/// The union's four members are the four channels, and the values are the model's own
/// `ChannelSubtype` catalog (`CHANNEL_SUBTYPE_VALUES` in [`crate::ops::types`]) — the
/// same catalog [`StoredCampaign::channel_subtypes`] reports a campaign's configuration
/// in, so the two can be compared directly.
///
/// The one place the union is read as a channel name: both the check a batch makes
/// against the campaign's configuration and [`StoredOutboundRequest::channel_subtype`]
/// go through it, so a stored request's channel is derived from the parameters it was
/// submitted with rather than kept as a second copy of them.
pub fn requested_channel_subtype(parameters: &ChannelSubtypeParameters) -> &'static str {
    match parameters {
        ChannelSubtypeParameters::Telephony(_) => "TELEPHONY",
        ChannelSubtypeParameters::Sms(_) => "SMS",
        ChannelSubtypeParameters::Email(_) => "EMAIL",
        ChannelSubtypeParameters::whatsApp(_) => "WHATSAPP",
    }
}

/// The listing a cursor belongs to: the operation that minted it, together with the
/// selectors that chose the result set it was computed over.
///
/// A cursor is a position in ONE listing's stored order, so it is a resume point for
/// that listing and for nothing else. The sequence values the paged lists of this
/// service page by are minted from counters that advance independently of each other
/// ([`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State`]), and a
/// listing scoped to one region, instance or filter reports a different set from a
/// listing scoped to another, so the same bare sequence value names an unrelated
/// position in each. Carrying the scope in the token and requiring it back is what
/// lets a list tell a cursor of its own from one it never issued — the second being
/// an [`UnissuedCursor`] refusal rather than a page silently cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorScope(String);

impl CursorScope {
    /// The listing `operation` makes over the result set `selectors` chooses — the
    /// region it is scoped to and whatever path or filter members narrow it further,
    /// in a fixed order.
    ///
    /// Each selector goes in length-prefixed, so no two different selector lists can
    /// spell one scope: a listing filtered by an instance and an unfiltered one over
    /// the same region stay apart, as do two instance ids whose concatenations agree.
    pub fn new<'s>(operation: &str, selectors: impl IntoIterator<Item = &'s str>) -> Self {
        let mut scope = operation.to_string();
        for selector in selectors {
            scope.push_str(&format!("/{}:{selector}", selector.len()));
        }
        Self(scope)
    }

    /// The fixed-size tag a token carries this scope as: the leading
    /// [`CURSOR_SCOPE_TAG_BYTES`] bytes of the scope's digest, hex-encoded.
    ///
    /// A digest rather than the scope itself keeps the token opaque and its length
    /// independent of how long the names the listing was scoped by are, so a cursor
    /// stays well inside the bound `NextToken` declares however it was scoped.
    fn tag(&self) -> String {
        use ::sha2::Digest as _;

        let digest = ::sha2::Sha256::digest(self.0.as_bytes());
        ::hex::encode(&digest[..CURSOR_SCOPE_TAG_BYTES])
    }
}

/// How much of a [`CursorScope`]'s digest a token carries: enough that two listings
/// of this service cannot be confused for one another.
const CURSOR_SCOPE_TAG_BYTES: usize = 16;

/// Encode a cursor-paged list's cursor: the listing that minted it ([`CursorScope`])
/// and the sequence number of the last record on the page just returned,
/// hex-encoded so the token is opaque rather than a readable position.
///
/// The position is a sequence number rather than an id so that a record deleted
/// between two pages does not invalidate a token already handed out: the next
/// page is simply everything stored after that point. The scope beside it is what
/// makes the token a resume point for the listing that handed it out alone.
pub fn encode_next_token(scope: &CursorScope, last_sequence: u64) -> String {
    ::hex::encode(format!("{}:{last_sequence}", scope.tag()))
}

/// Decode a cursor [`encode_next_token`] minted for `scope`, or `None` when the
/// token was not produced by it for THIS listing — a token this codec never minted,
/// a token it minted for another listing, and a re-spelling of one of its own tokens
/// alike.
///
/// A position is resolved only from the token [`encode_next_token`] would itself
/// write for it, which is the only spelling of that position this service ever hands
/// out: reading the position back out of the token is not enough, because the readers
/// this walks through each take spellings the minting side cannot write — `u64`'s
/// `FromStr` takes a leading `+` and any number of leading zeros, while
/// `::hex::decode` takes the upper-case digits `::hex::encode` never emits — and a
/// position resolved from one of those would resume a caller mid-list from a cursor
/// this service never issued, silently dropping every record before it instead of
/// refusing the token.
pub fn decode_next_token(scope: &CursorScope, token: &str) -> Option<u64> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (tag, position) = decoded.split_once(':')?;
    if tag != scope.tag() {
        return None;
    }
    let sequence: u64 = position.parse().ok()?;
    (encode_next_token(scope, sequence) == token).then_some(sequence)
}

/// The page size a cursor-paged list uses when the caller names no `maxResults`:
/// the upper bound the model declares for it (`MaxResults`, range 1..50), so an
/// unpaged call sees as much as one page can carry.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// A `nextToken` a caller sent that [`encode_next_token`] did not mint for THIS
/// listing, which a paged list answers as a `ValidationException` carrying
/// [`Self::message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnissuedCursor;

impl UnissuedCursor {
    /// The sentence a paged list refuses a token this service did not issue with.
    /// The one place that text lives, so two lists cannot word the same refusal
    /// differently.
    pub fn message(self) -> String {
        "nextToken is not a cursor this service issued".to_string()
    }
}

/// One page of a cursor-paged list: the window of items it reports and the token
/// the call after it resumes from.
#[derive(Debug)]
pub struct CursorPage<'a, T> {
    /// The items on this page, in the stored order they were given in.
    pub page: &'a [T],
    /// The cursor the next page starts after, absent when this page reached the
    /// end of the items — which is how a client knows to stop paging.
    pub next_token: Option<String>,
}

/// The window `next_token` and `max_results` name over `items`, already in stored
/// order, each item's cursor position read by `sequence_of`, for the listing
/// `scope` names.
///
/// The one place the rule the opaque cursor is built around lives, beside the codec
/// that mints it ([`encode_next_token`] / [`decode_next_token`]), so every paged
/// list of this service pages identically: the page starts at the first item whose
/// sequence is past the cursor, runs for `max_results` items (or
/// [`DEFAULT_PAGE_SIZE`] when the caller named none) clamped to what is left, and
/// hands back a token — the sequence of the item it ended at, stamped with `scope` —
/// only while items remain beyond it.
///
/// A cursor names a position in the stored order of ONE listing rather than a
/// record, so an item removed between two pages does not invalidate a token already
/// handed out, while a token minted for any other listing resolves to no position
/// here at all — which is [`UnissuedCursor`] rather than a page, exactly as a token
/// this codec never minted is.
pub fn cursor_page<'a, T>(
    items: &'a [T],
    sequence_of: impl Fn(&T) -> u64,
    scope: &CursorScope,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<CursorPage<'a, T>, UnissuedCursor> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let after = decode_next_token(scope, token).ok_or(UnissuedCursor)?;
            items
                .iter()
                .position(|item| sequence_of(item) > after)
                .unwrap_or(items.len())
        }
    };
    let page_size = max_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE);
    let end = start.saturating_add(page_size).min(items.len());
    let page = &items[start..end];
    Ok(CursorPage {
        next_token: if end < items.len() {
            page.last()
                .map(|item| encode_next_token(scope, sequence_of(item)))
        } else {
            None
        },
        page,
    })
}

/// `at` as the `(seconds, nanoseconds)` pair an `::aws_smithy_types::DateTime`
/// reports an instant as, so a wall-clock instant and a modelled timestamp compare
/// exactly rather than through a lossy float.
fn instant_parts(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Whether the instant `deadline` has passed at `now` — compared exactly, through
/// [`instant_parts`], so the comparison is not rounded to whole seconds.
///
/// The one place a deadline a caller sent is weighed against the clock, so every
/// reader of a stored deadline draws the line at the same instant.
fn deadline_passed(deadline: ::aws_smithy_types::DateTime, now: SystemTime) -> bool {
    instant_parts(::aws_smithy_types::DateTime::from(now)) >= instant_parts(deadline)
}

/// The lifecycle state of a stored campaign.
///
/// The variants are exactly the `CampaignState` enum the Smithy model declares —
/// see `CAMPAIGN_STATE_VALUES` in [`crate::ops::types`], which
/// [`CampaignState::as_wire`] renders a value of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignState {
    Completed,
    Failed,
    Initialized,
    Paused,
    Running,
    Stopped,
}

impl CampaignState {
    /// The wire value a response carries for this state.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Initialized => "Initialized",
            Self::Paused => "Paused",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
        }
    }

    /// The UPPER-CASE spelling of this state, which is how a refusal that names the
    /// state a campaign is at spells it (see [`campaign_invalid_state_message`]) — the
    /// same catalog value as [`Self::as_wire`], raised rather than restated, so the two
    /// cannot drift as the catalog grows.
    pub fn as_upper(self) -> String {
        self.as_wire().to_ascii_uppercase()
    }

    /// The state this one moves to under `action`, or `None` when `action` is not
    /// legal from here (which the caller answers with the state-conflict refusal
    /// [`campaign_invalid_state_message`] words).
    ///
    /// Start is the only action a campaign that has never entered its run takes:
    /// every other verb acts on a run already under way, so Pause (which suspends
    /// a running campaign), Resume (which puts a suspended one back to work, legal
    /// from `Paused` and nowhere else) and Stop (which ends a run before it
    /// finishes, legal from `Running` and `Paused`) are all refused from
    /// `Initialized`. Nothing moves out of a TERMINAL state, and `Stopped` is one of
    /// them: stopping ends the campaign's run for good, so it is not startable again,
    /// as `Completed` and `Failed` are not. No action is a no-op either: repeating one
    /// is refused rather than silently accepted.
    ///
    /// A call is judged against the state the campaign is at NOW, which the clock
    /// has a say in — see [`StoredCampaign::state_at`], the one place a caller reads
    /// a campaign's state from.
    pub fn after(self, action: CampaignAction) -> Option<Self> {
        match (self, action) {
            (Self::Initialized | Self::Paused, CampaignAction::Start) => Some(Self::Running),
            (Self::Running, CampaignAction::Pause) => Some(Self::Paused),
            (Self::Paused, CampaignAction::Resume) => Some(Self::Running),
            (Self::Running | Self::Paused, CampaignAction::Stop) => Some(Self::Stopped),
            _ => None,
        }
    }

    /// Whether a campaign in this state still takes a change to its configuration
    /// (the flow it is associated with, its source, its schedule, its limits).
    ///
    /// `Completed` and `Failed` are the terminal states — [`Self::after`] moves
    /// nothing out of either — so a campaign in one of them will never run again and
    /// its configuration can no longer affect anything; a configuration change is
    /// refused there with `InvalidCampaignStateException`, which is exactly the error
    /// the model declares for every campaign-configuration operation
    /// (`UpdateCampaignFlowAssociation`, `UpdateCampaignSource`, …) and does NOT
    /// declare for `UpdateCampaignName`, the one update that is independent of how
    /// the campaign runs. Every non-terminal state takes the change.
    ///
    /// Shared by the configuration operations so two of them cannot disagree about
    /// which states are still configurable.
    pub fn accepts_configuration_change(self) -> bool {
        !matches!(self, Self::Completed | Self::Failed)
    }

    /// Whether a campaign in this state takes a batch of outbound requests into one of
    /// its buffers.
    ///
    /// A campaign reaches out to anyone only while its run is actually under way, which
    /// is `Running`: a campaign that has not started yet, one suspended, one whose run
    /// was ended and one in a terminal state are all placing nothing, so a batch
    /// submitted to any of them is refused as the state conflict it is (see
    /// [`campaign_invalid_state_message`]) rather than buffered for a run that is not
    /// happening.
    ///
    /// Shared by the batch operations through [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::dialable_campaign`],
    /// so two of them cannot disagree about which states take requests.
    pub fn takes_outbound_requests(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A lifecycle action a `StartCampaign` / `PauseCampaign` / `ResumeCampaign` /
/// `StopCampaign` call asks for, as [`CampaignState::after`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignAction {
    Start,
    Pause,
    Resume,
    Stop,
}

/// A configuration structure every one of whose modelled members is optional, so a
/// request can carry one that configures nothing at all.
///
/// The campaign record represents "configured for nothing" by the ABSENCE of the
/// member it would hold, so one such structure is never stored: every writer asks
/// [`configured`] what to store, and no reader has to tell a structure holding no
/// member from a missing one.
pub trait OptionalMembersOnly {
    /// Whether this structure holds none of its modelled members.
    fn holds_no_member(&self) -> bool;
}

impl OptionalMembersOnly for ChannelSubtypeConfig {
    /// Every member of `ChannelSubtypeConfig` is a channel block, and the model marks
    /// none of them `@required`, so a config holding no block is configured for no
    /// channel.
    fn holds_no_member(&self) -> bool {
        self.email.is_none()
            && self.sms.is_none()
            && self.telephony.is_none()
            && self.whats_app.is_none()
    }
}

impl OptionalMembersOnly for CommunicationLimitsConfig {
    /// `allChannelSubtypes` and `instanceLimitsHandling` are the whole of
    /// `CommunicationLimitsConfig`, and the model marks neither `@required`, so an
    /// override holding neither limits nothing.
    fn holds_no_member(&self) -> bool {
        self.all_channel_subtypes.is_none() && self.instance_limits_handling.is_none()
    }
}

/// `config` as a record holds it: a structure holding no member at all holds nothing,
/// so the record carries no member rather than an empty structure.
///
/// The one representation of "configured for nothing" on this service's records, so the
/// write paths and the removal paths of one member cannot disagree about it.
pub fn configured<C: OptionalMembersOnly>(config: C) -> Option<C> {
    if config.holds_no_member() {
        None
    } else {
        Some(config)
    }
}

/// A campaign as stored: everything `CreateCampaign` was given, plus the server's
/// own id, ARN, lifecycle state and creation instant.
#[derive(Debug, Clone)]
pub struct StoredCampaign {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub connect_instance_id: String,
    /// Absent when the campaign is configured for no channel at all — the record's one
    /// representation of that, which [`configured`] is what every writer gets it from.
    pub channel_subtype_config: Option<ChannelSubtypeConfig>,
    /// The modelled `type` member (`ExternalCampaignType`); `type` is a keyword.
    pub campaign_type: Option<String>,
    pub source: Option<Source>,
    pub connect_campaign_flow_arn: Option<String>,
    pub schedule: Option<Schedule>,
    pub entry_limits_config: Option<EntryLimitsConfig>,
    pub communication_time_config: Option<CommunicationTimeConfig>,
    /// Absent when the campaign overrides no communication limit at all — the same
    /// representation [`Self::channel_subtype_config`] carries, from [`configured`].
    pub communication_limits_override: Option<CommunicationLimitsConfig>,
    /// Ordered so the tag map a response carries is built deterministically.
    pub tags: BTreeMap<String, String>,
    /// The state the lifecycle CALLS left this campaign in
    /// (`Start`/`Pause`/`Resume`/`StopCampaign`, through [`CampaignState::after`]).
    ///
    /// NOT what a reader reports: the campaign's state is owed to the clock as well
    /// as to those calls, so every reader asks [`Self::state_at`] for the state at
    /// the instant it is serving.
    pub lifecycle_state: CampaignState,
    /// When this campaign was created. Part of the record, not of any response:
    /// no modelled member in this slice carries it.
    pub created_at: std::time::SystemTime,
    /// When this campaign's definition or configuration was last WRITTEN, which
    /// `Campaign$lastModifiedTime` and `CampaignSummary$lastModifiedTime` both
    /// project (through [`Self::last_modified_time_for_response`]).
    ///
    /// It starts at [`Self::created_at`] — the create is the write that put this
    /// campaign's definition and configuration in the store — and moves only when a
    /// later call actually writes one of them, through the one writer of it,
    /// [`Self::mark_modified`]. A refused call, and a call that writes nothing, leave
    /// it exactly as it was.
    pub last_modified_time: std::time::SystemTime,
    /// The outbound requests this campaign has pending, keyed by the `clientToken`
    /// each was submitted under — the idempotency token `PutOutboundRequestBatch`
    /// ("This API is idempotent", per the model's own documentation) resolves a
    /// repeated submission through, so the same token is answered with the dial
    /// request id it was first given rather than a second one.
    ///
    /// A request leaves this buffer once its `expirationTime` has passed, which is
    /// owed to the clock alone ([`Self::expire_outbound_requests`]): an expired
    /// request is no longer dialable, so it neither occupies a buffer slot nor keeps
    /// answering for its token.
    pub outbound_requests: BTreeMap<String, StoredOutboundRequest>,
    /// The profile outbound requests this campaign has pending, keyed by the
    /// `clientToken` each was submitted under — the idempotency token
    /// `PutProfileOutboundRequestBatch` ("This API is idempotent", per the model's own
    /// documentation) resolves a repeated submission through, so the same token is
    /// answered with the `ProfileOutboundRequestId` it was first given rather than a
    /// second one.
    ///
    /// A buffer of its own beside [`Self::outbound_requests`], because the two carry
    /// different resources: a profile request names a Customer Profiles profile and its
    /// expiration is OPTIONAL, where a dial request names a channel and must carry one.
    /// A request whose `expirationTime` has passed leaves this buffer for the same
    /// reason ([`Self::expire_profile_outbound_requests`]): it can no longer be placed,
    /// so it neither occupies a buffer slot nor keeps answering for its token.
    pub profile_outbound_requests: BTreeMap<String, StoredProfileOutboundRequest>,
}

/// An outbound request a campaign has taken, as stored.
#[derive(Debug, Clone)]
pub struct StoredOutboundRequest {
    /// The `DialRequestId` this request was given. Minted from the campaign's own
    /// counter, never from anything in the request, and within `DialRequestId`'s
    /// pattern `^[-_.a-zA-Z0-9]+$` and its 1..256 length bound.
    pub id: String,
    /// The `channelSubtypeParameters` the request carried, whole: who this request is
    /// to reach and with what — a telephony request's `destinationPhoneNumber`,
    /// `attributes`, `connectSourcePhoneNumber`, `answerMachineDetectionConfig` and
    /// `ringTimeout`, an email request's addresses, an SMS or WhatsApp request's
    /// template and its parameters. Part of the record, not of any response — no
    /// modelled member of this operation's output carries them — so a later reader of
    /// the buffer has what was submitted, which is what makes the buffer a record of
    /// the campaign's pending requests rather than of their count alone.
    ///
    /// The channel the request is for is the union's own member, read back through
    /// [`Self::channel_subtype`] rather than stored beside it.
    pub channel_subtype_parameters: ChannelSubtypeParameters,
    /// The `expirationTime` the caller sent: the instant after which this request is
    /// no longer dialable, which is why `OutboundRequest$expirationTime` is required.
    /// A record past it is no longer pending (see [`Self::is_expired_at`]).
    pub expires_at: ::aws_smithy_types::DateTime,
}

impl StoredOutboundRequest {
    /// The `ChannelSubtype` this request was submitted for, derived from the
    /// parameters it carried by [`requested_channel_subtype`].
    pub fn channel_subtype(&self) -> &'static str {
        requested_channel_subtype(&self.channel_subtype_parameters)
    }

    /// Whether this request's `expirationTime` has passed at `now`, so it is no
    /// longer dialable and no longer one of the campaign's pending requests.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        deadline_passed(self.expires_at, now)
    }
}

/// A profile outbound request a campaign has taken, as stored.
#[derive(Debug, Clone)]
pub struct StoredProfileOutboundRequest {
    /// The `ProfileOutboundRequestId` this request was given. Minted from the store's
    /// own counter, never from anything in the request, and within
    /// `ProfileOutboundRequestId`'s pattern `^[a-zA-Z0-9_\-.]*$` and its 0..256 length
    /// bound.
    pub id: String,
    /// The `profileId` the request was submitted for: the Customer Profiles profile the
    /// campaign is to reach.
    pub profile_id: String,
    /// The `expirationTime` the caller sent, when it sent one: the instant after which
    /// this request can no longer be placed. `ProfileOutboundRequest$expirationTime` is
    /// OPTIONAL, so a request that carries none never expires and stays pending until
    /// the campaign goes (see [`Self::is_expired_at`]).
    pub expires_at: Option<::aws_smithy_types::DateTime>,
    /// The `eventTriggerContext` the request carried, when it carried one, whole: the
    /// `sourceEvent` this request was triggered by AND the `channelContext` it was
    /// triggered from (a web notification's session and browser). Part of the record,
    /// not of any response — no modelled member of this operation's output carries it —
    /// so a later reader of the buffer has what was submitted.
    pub event_trigger_context: Option<EventTriggerContext>,
}

impl StoredProfileOutboundRequest {
    /// Whether this request's `expirationTime` has passed at `now`, so it can no longer
    /// be placed and is no longer one of the campaign's pending profile requests. A
    /// request submitted without an expiration has no instant to pass, so it never
    /// expires.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        self.expires_at
            .is_some_and(|expires_at| deadline_passed(expires_at, now))
    }
}

/// What a campaign's store did with one outbound request of a batch, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::take_outbound_request`]
/// and
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::take_profile_outbound_request`]
/// both report it: the two batch operations decide a request's fate the same way, so
/// they share one outcome rather than each carrying a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundRequestOutcome {
    /// The request is in the campaign's buffer under this id (a `DialRequestId` or a
    /// `ProfileOutboundRequestId`) — either freshly minted, or the id an earlier
    /// submission under the same `clientToken` was given, in which case this repeat
    /// stored nothing new.
    Taken(String),
    /// The campaign's buffer already holds [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`]
    /// pending requests, so nothing was stored.
    BufferFull,
    /// The request's `expirationTime` has already passed, so there is no call left to
    /// place for it: nothing was stored and no id was minted.
    Expired,
}

impl StoredCampaign {
    /// Record that the call running now has written this campaign's definition or its
    /// configuration, at `now`.
    ///
    /// The ONE writer of [`Self::last_modified_time`], so every operation that changes
    /// what a reader projects stamps the record the same way and no two of them can
    /// disagree about when it was last modified. It is called from the record's own
    /// writers below, which is what keeps a refusal, and a call that writes nothing,
    /// from stamping anything: the stamp moves with the write, not with the call.
    ///
    /// The campaign's lifecycle state, its tag map and its outbound-request buffers are
    /// neither its definition nor its configuration — the state is what
    /// `GetCampaignState` reports and the lifecycle calls move, the tags are written by
    /// the tagging operations, and the buffers hold submitted work — so none of their
    /// writers comes through here.
    pub fn mark_modified(&mut self, now: SystemTime) {
        self.last_modified_time = now;
    }

    /// This campaign's `lastModifiedTime` as a response carries it, at the precision
    /// this service keeps a `date-time` to ([`millisecond_precision`]).
    ///
    /// Always present: the member is optional because the real service holds no stamp
    /// for a campaign created before it existed, and every campaign in this store was
    /// stamped by the create that minted it.
    pub fn last_modified_time_for_response(&self) -> Option<::aws_smithy_types::DateTime> {
        Some(millisecond_precision(::aws_smithy_types::DateTime::from(
            self.last_modified_time,
        )))
    }

    /// The stored tags as a response `TagMap`, absent when the campaign carries
    /// none (the member is optional, and AWS omits an empty map).
    pub fn tags_for_response(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        }
    }

    /// Whether this campaign's scheduled run window has closed at `now`.
    ///
    /// A campaign carries a window only when it carries a `Schedule`, whose
    /// `endTime` is `@required` — so a scheduled campaign always knows the instant
    /// its window closes, and an unscheduled one has no window to close.
    pub fn run_window_closed_at(&self, now: SystemTime) -> bool {
        let now = instant_parts(::aws_smithy_types::DateTime::from(now));
        self.schedule
            .as_ref()
            .is_some_and(|schedule| now >= instant_parts(schedule.end_time))
    }

    /// This campaign's lifecycle state at `now`.
    ///
    /// The state is owed to the clock as well as to the lifecycle calls: a campaign
    /// that has entered its run — `Running` or `Paused`, the states a
    /// `StartCampaign` leads to — has run to the end of its schedule once that
    /// schedule's `endTime` has passed, and a run that has reached its end is the
    /// terminal `Completed`. A campaign that never entered a run (`Initialized`) or
    /// whose run a caller ended (`Stopped`) is left where the calls put it, and a
    /// terminal state moves nowhere.
    ///
    /// A pure read of the stored schedule, so reporting the state writes nothing and
    /// two callers reading at the same instant see the same value — the same way
    /// [`StoredInstanceOnboarding::status_at`] reports an onboarding job's status.
    pub fn state_at(&self, now: SystemTime) -> CampaignState {
        match self.lifecycle_state {
            CampaignState::Running | CampaignState::Paused if self.run_window_closed_at(now) => {
                CampaignState::Completed
            }
            stored => stored,
        }
    }

    /// Drop every request this campaign's buffer holds whose `expirationTime` has
    /// passed at `now`, and report how many went.
    ///
    /// The buffer is a buffer of PENDING requests: a request past its expiration is
    /// no longer dialable, so it is no longer pending — it holds neither one of the
    /// [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`] slots nor its own `clientToken`. The
    /// removal is owed to the CLOCK, not to the call that materialises it: it was due
    /// at the expiration instant whether or not anyone called afterwards.
    pub fn expire_outbound_requests(&mut self, now: SystemTime) -> usize {
        let before = self.outbound_requests.len();
        self.outbound_requests
            .retain(|_, request| !request.is_expired_at(now));
        before - self.outbound_requests.len()
    }

    /// The requests this campaign still has pending at `now` — everything in its
    /// buffer that has not passed its `expirationTime`. A pure read, so a reader
    /// counting the buffer writes nothing.
    pub fn pending_outbound_requests(&self, now: SystemTime) -> usize {
        self.outbound_requests
            .values()
            .filter(|request| !request.is_expired_at(now))
            .count()
    }

    /// Drop every profile outbound request this campaign's buffer holds whose
    /// `expirationTime` has passed at `now`, and report how many went.
    ///
    /// The same rule as [`Self::expire_outbound_requests`], on the buffer of profile
    /// requests: a request past its expiration can no longer be placed, so it is no
    /// longer pending and holds neither one of the
    /// [`MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN`] slots nor its own `clientToken`. The
    /// removal is owed to the CLOCK, not to the call that materialises it. A request
    /// submitted without an expiration never becomes due.
    pub fn expire_profile_outbound_requests(&mut self, now: SystemTime) -> usize {
        let before = self.profile_outbound_requests.len();
        self.profile_outbound_requests
            .retain(|_, request| !request.is_expired_at(now));
        before - self.profile_outbound_requests.len()
    }

    /// The profile outbound requests this campaign still has pending at `now` —
    /// everything in that buffer which has not passed its `expirationTime`. A pure
    /// read, so a reader counting the buffer writes nothing.
    pub fn pending_profile_outbound_requests(&self, now: SystemTime) -> usize {
        self.profile_outbound_requests
            .values()
            .filter(|request| !request.is_expired_at(now))
            .count()
    }

    /// The channel subtypes this campaign is configured for, as the model's
    /// `ChannelSubtype` values.
    ///
    /// Derived from the stored `channelSubtypeConfig`: one entry per channel block
    /// present. The values AND their order come from the model's own catalog,
    /// `CHANNEL_SUBTYPE_VALUES` in [`crate::ops::types`], so the list is
    /// deterministic and satisfies `ChannelSubtypeList`'s `@uniqueItems`.
    pub fn channel_subtypes(&self) -> Vec<String> {
        let Some(config) = self.channel_subtype_config.as_ref() else {
            return Vec::new();
        };
        crate::ops::types::CHANNEL_SUBTYPE_VALUES
            .iter()
            .filter(|subtype| match **subtype {
                "EMAIL" => config.email.is_some(),
                "SMS" => config.sms.is_some(),
                "TELEPHONY" => config.telephony.is_some(),
                "WHATSAPP" => config.whats_app.is_some(),
                // A catalog value with no block on `ChannelSubtypeConfig` is a
                // channel this emulator cannot be configured for, so it is not
                // reported as configured.
                _ => false,
            })
            .map(|subtype| (*subtype).to_string())
            .collect()
    }

    /// Replace this campaign's channel subtype config with `config`.
    ///
    /// The config is ONE structure, so this is a replacement and not a merge: a channel
    /// `config` leaves out is a channel the campaign is no longer configured for. A
    /// config holding no channel block at all configures no channel, which the record
    /// carries as no `channelSubtypeConfig` rather than as an empty one — the same
    /// representation [`Self::clear_channel_subtype_config`] leaves behind when the last
    /// block goes, so neither writer of the member can leave a state the other never
    /// would.
    pub fn set_channel_subtype_config(&mut self, config: ChannelSubtypeConfig, now: SystemTime) {
        self.channel_subtype_config = configured(config);
        self.mark_modified(now);
    }

    /// Replace this campaign's communication limits override with `config`.
    ///
    /// The same replacement-and-not-merge rule as [`Self::set_channel_subtype_config`],
    /// in the same one representation: an override holding neither of its members
    /// overrides nothing, so the record carries no `communicationLimitsOverride` rather
    /// than an empty one — what [`Self::clear_communication_limits`] leaves behind when
    /// the last member goes.
    pub fn set_communication_limits_override(
        &mut self,
        config: CommunicationLimitsConfig,
        now: SystemTime,
    ) {
        self.communication_limits_override = configured(config);
        self.mark_modified(now);
    }

    /// Replace this campaign's communication-time config with `config`.
    ///
    /// The same replacement-and-not-merge rule as [`Self::set_channel_subtype_config`]:
    /// the config is ONE structure, whose `localTimeZoneConfig` is `@required`, so a
    /// channel the new config leaves out is a channel the campaign holds no window for.
    pub fn set_communication_time_config(
        &mut self,
        config: CommunicationTimeConfig,
        now: SystemTime,
    ) {
        self.communication_time_config = Some(config);
        self.mark_modified(now);
    }

    /// Replace this campaign's entry-limits config with `config`.
    pub fn set_entry_limits_config(&mut self, config: EntryLimitsConfig, now: SystemTime) {
        self.entry_limits_config = Some(config);
        self.mark_modified(now);
    }

    /// Replace this campaign's run window with `schedule`, at the precision this
    /// service keeps a `date-time` to ([`stored_schedule`]).
    pub fn set_schedule(&mut self, schedule: Schedule, now: SystemTime) {
        self.schedule = Some(stored_schedule(schedule));
        self.mark_modified(now);
    }

    /// Replace what this campaign is sourced from with `source`.
    ///
    /// The source is one union, so this is a replacement and not a merge: exactly one
    /// member is in force, and the member the new value does not name is one the
    /// campaign is no longer sourced through.
    pub fn set_source(&mut self, source: Source, now: SystemTime) {
        self.source = Some(source);
        self.mark_modified(now);
    }

    /// Associate this campaign with the Connect campaign flow `arn` names, replacing
    /// whatever flow it was associated with.
    pub fn set_connect_campaign_flow_arn(&mut self, arn: String, now: SystemTime) {
        self.connect_campaign_flow_arn = Some(arn);
        self.mark_modified(now);
    }

    /// Remove this campaign's entry-limits config, so no reader projects one.
    ///
    /// A campaign that held none is left exactly as it was — nothing at all is written,
    /// the stamp included — which is what makes the removal idempotent, as the model
    /// declares.
    pub fn clear_entry_limits(&mut self, now: SystemTime) {
        if self.entry_limits_config.take().is_some() {
            self.mark_modified(now);
        }
    }

    /// Remove the channel-subtype config block this campaign holds for the channel
    /// `channel_subtype` names.
    ///
    /// `channel_subtype` is a `ChannelSubtype` value — the catalog
    /// `CHANNEL_SUBTYPE_VALUES` in [`crate::ops::types`], which the caller refuses a
    /// value outside of before reaching here, so a value naming no channel removes
    /// nothing.
    ///
    /// Only the named channel's block goes: every other channel keeps the block it
    /// was configured with, so [`Self::channel_subtypes`] still reports those. A
    /// campaign that held no block for the named channel, and one carrying no channel
    /// subtype config at all, are left exactly as they were, which is what makes the
    /// operation idempotent, as the model declares.
    ///
    /// A campaign always keeps at least ONE configured channel subtype, so a removal
    /// that would leave it configured for no channel at all is refused
    /// ([`Err`]) and NOTHING is written: the record cannot come to hold the empty
    /// channel-subtype config that would leave every later read of it describing a
    /// campaign that can reach nobody. The emptiness test is the one every writer of the
    /// member decides by ([`OptionalMembersOnly`]).
    pub fn clear_channel_subtype_config(
        &mut self,
        channel_subtype: &str,
        now: SystemTime,
    ) -> Result<(), LastChannelSubtype> {
        if self.channel_subtype_config.is_none() {
            return Ok(());
        }
        // Judged BEFORE anything is taken out, so a refusal leaves the config exactly
        // as it was: the campaign keeps this block when it is the only one it holds.
        if self.channel_subtypes() == [channel_subtype] {
            return Err(LastChannelSubtype);
        }
        let Some(config) = self.channel_subtype_config.as_mut() else {
            return Ok(());
        };
        let removed = match channel_subtype {
            "EMAIL" => config.email.take().is_some(),
            "SMS" => config.sms.take().is_some(),
            "TELEPHONY" => config.telephony.take().is_some(),
            "WHATSAPP" => config.whats_app.take().is_some(),
            // A catalog value with no block on `ChannelSubtypeConfig` is a channel
            // this emulator cannot be configured for, so it holds nothing to remove.
            _ => false,
        };
        // The guard above leaves a removal with a channel still configured, so this
        // keeps the one representation of "configured for no channel" the writers of
        // the member share rather than ever being reached.
        if config.holds_no_member() {
            self.channel_subtype_config = None;
        }
        // Only a removal writes anything, so a call that found no block for the named
        // channel leaves the record — the stamp included — exactly as it was.
        if removed {
            self.mark_modified(now);
        }
        Ok(())
    }

    /// Remove the communication-time window this campaign holds for the channel
    /// `config_type` names.
    ///
    /// `config_type` is a `CommunicationTimeConfigType` value — the catalog
    /// `COMMUNICATION_TIME_CONFIG_TYPE_VALUES` in [`crate::ops::types`], which the
    /// caller refuses a value outside of before reaching here, so a value naming no
    /// channel removes nothing.
    ///
    /// Only the named channel's window goes. `CommunicationTimeConfig`'s
    /// `localTimeZoneConfig` is `@required`, so the config itself stays on the record
    /// with the time zone it was configured with and the other channels keep their
    /// windows; a campaign carrying no communication-time config at all holds no
    /// window for any channel, so there is nothing to remove and nothing is written.
    pub fn clear_communication_time_window(&mut self, config_type: &str, now: SystemTime) {
        let Some(config) = self.communication_time_config.as_mut() else {
            return;
        };
        let window = match config_type {
            "EMAIL" => &mut config.email,
            "SMS" => &mut config.sms,
            "TELEPHONY" => &mut config.telephony,
            "WHATSAPP" => &mut config.whats_app,
            // A catalog value with no window on `CommunicationTimeConfig` is a
            // channel this emulator cannot be configured for, so it holds nothing to
            // remove.
            _ => return,
        };
        // Only a removal writes anything, so a call that found no window for the named
        // channel leaves the record — the stamp included — exactly as it was.
        if window.take().is_some() {
            self.mark_modified(now);
        }
    }

    /// Remove the communication-limits block this campaign's override holds for the
    /// config `config_type` names.
    ///
    /// `config_type` is a `CommunicationLimitsConfigType` value — the catalog
    /// `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES` in [`crate::ops::types`], whose one
    /// entry `ALL_CHANNEL_SUBTYPES` is the `CommunicationLimitsConfig$allChannelSubtypes`
    /// block; the caller refuses a value outside that catalog before reaching here, so a
    /// value naming no block removes nothing.
    ///
    /// Only the named block goes: `instanceLimitsHandling` is not a config type any
    /// request can name, so a campaign that set it keeps it. An override a REMOVAL left
    /// carrying no member at all carries nothing, so the record holds no override rather
    /// than an empty one. A campaign with no override to begin with, and one whose
    /// override holds no block for the named config, are left exactly as they were —
    /// which is what makes the operation idempotent, as the model declares.
    pub fn clear_communication_limits(&mut self, config_type: &str, now: SystemTime) {
        let Some(config) = self.communication_limits_override.as_mut() else {
            return;
        };
        let removed = match config_type {
            "ALL_CHANNEL_SUBTYPES" => config.all_channel_subtypes.take().is_some(),
            // A catalog value with no block on `CommunicationLimitsConfig` is a config
            // this emulator cannot be configured for, so it holds nothing to remove.
            _ => false,
        };
        // Only a removal collapses the override: a call that removed nothing writes
        // nothing at all — and the emptiness test is the shared one the sibling
        // `clear_channel_subtype_config` and both update paths decide by.
        if removed && config.holds_no_member() {
            self.communication_limits_override = None;
        }
        // ...and only a removal stamps the record, so a call that removed nothing
        // leaves the campaign's last-modified instant where it was.
        if removed {
            self.mark_modified(now);
        }
    }

    /// This record as the modelled `Campaign` shape `DescribeCampaign` returns.
    pub fn to_campaign(&self) -> Campaign {
        Campaign {
            id: self.id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            connect_instance_id: self.connect_instance_id.clone(),
            channel_subtype_config: self.channel_subtype_config.clone(),
            r#type: self.campaign_type.clone(),
            source: self.source.clone(),
            connect_campaign_flow_arn: self.connect_campaign_flow_arn.clone(),
            schedule: self.schedule.clone(),
            entry_limits_config: self.entry_limits_config.clone(),
            communication_time_config: self.communication_time_config.clone(),
            communication_limits_override: self.communication_limits_override.clone(),
            last_modified_time: self.last_modified_time_for_response(),
            tags: self.tags_for_response(),
        }
    }

    /// This record as the modelled `CampaignSummary` shape `ListCampaigns`
    /// returns.
    pub fn to_summary(&self) -> CampaignSummary {
        CampaignSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            connect_instance_id: self.connect_instance_id.clone(),
            channel_subtypes: self.channel_subtypes(),
            r#type: self.campaign_type.clone(),
            schedule: self.schedule.clone(),
            entry_limits_config: self.entry_limits_config.clone(),
            connect_campaign_flow_arn: self.connect_campaign_flow_arn.clone(),
            last_modified_time: self.last_modified_time_for_response(),
        }
    }
}

/// Record one state-changing Connect-instance operation — onboarding a instance, or
/// configuring one: who asked (the access key id the request presented), which
/// Connect instance it named and how the call ended.
///
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields. Called on EVERY path such a call returns through
/// — the accepted one and each refusal, named by the modelled error the refusal
/// answers with — so an attempt to change an instance is as visible when it was
/// refused as when it succeeded. Only wire metadata and the request's instance id
/// go in: never a stored encryption, integration or communication-limits config, a
/// key or resource ARN or any credential material.
pub fn audit_onboarding_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    connect_instance_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::emulator_server::extract_akid(&parts.headers),
        connect_instance_id,
        "connect instance onboarding changed"
    );
}

/// The modelled server error a state-changing Connect-instance call answers with when
/// a store it has to read cannot be reached, recorded through
/// [`audit_onboarding_change`] on the way out.
///
/// The one place that pairing is made, so the server-fault exit of such a call cannot
/// answer without leaving the record every one of its exits owes: the payload the
/// caller sees and the record the trail keeps come out of the same call. `detail` is
/// the text the exception carries — which store the handler could not reach — and goes
/// nowhere near the record, which carries only wire metadata and the instance id the
/// request named.
pub fn instance_server_fault(
    parts: &::http::request::Parts,
    operation: &'static str,
    connect_instance_id: &str,
    detail: &str,
) -> InternalServerException {
    // The outcome is named by the modelled error the refusal answers with, exactly as
    // the request-fault exits name theirs.
    audit_onboarding_change(parts, operation, connect_instance_id, "InternalServerException");
    InternalServerException {
        message: detail.to_string(),
        x_amz_error_type: None,
    }
}

/// The `InstanceOnboardingJobStatusCode` value a job that has not finished
/// onboarding the instance yet reports, as the model's catalog declares it (see
/// `INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES` in [`crate::ops::types`]).
pub const ONBOARDING_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `InstanceOnboardingJobStatusCode` value a finished onboarding job reports,
/// from the same catalog.
pub const ONBOARDING_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `InstanceOnboardingJobStatusCode` value a job that could not onboard the
/// instance reports, from the same catalog. It is the ONLY status
/// `DeleteInstanceOnboardingJob` accepts: "Instance onboarding job is not in failed
/// status. Only failed job can be deleted."
///
/// A job carries it exactly while its record names the
/// `InstanceOnboardingJobFailureCode` it ended on
/// ([`StoredInstanceOnboarding::failure_code`]), which is what the onboarding's own
/// dependency check writes: a job the service cannot finish settles here instead of at
/// [`ONBOARDING_STATUS_SUCCEEDED`], and it is that record a delete clears.
pub const ONBOARDING_STATUS_FAILED: &str = "FAILED";

/// The `InstanceOnboardingJobFailureCode` an onboarding job ends on when the KMS key
/// its `encryptionConfig` names resolves to no key the account holds in the request's
/// region, as the model's catalog declares it (see
/// `INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES` in [`crate::ops::types`]).
pub const ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND: &str = "KMS_KEY_NOT_FOUND";

/// The `InstanceOnboardingJobFailureCode` an onboarding job ends on when the key its
/// `encryptionConfig` names IS held but the service cannot use it to encrypt the
/// instance's campaign data, from the same catalog.
///
/// It is the catalog's only KMS code for a key that was resolved and still could not be
/// used, the other being [`ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND`] for one that resolves
/// to nothing at all.
pub const ONBOARDING_FAILURE_KMS_ACCESS_DENIED: &str = "KMS_ACCESS_DENIED";

/// Why `DeleteInstanceOnboardingJob` refuses to delete a job that has not failed.
///
/// A fixed service string: it names no instance and carries nothing from the
/// request, so every refusal of this kind reads the same, as the recorded responses
/// do.
pub const ONBOARDING_JOB_NOT_FAILED_MESSAGE: &str =
    "Instance onboarding job is not in failed status. Only failed job can be deleted.";

/// What this service answers when a Connect instance has no onboarding job to report
/// on or to delete: the region holds no record of one, because the instance was never
/// onboarded there or its job has since been deleted.
///
/// Composed from the request AND from who is asking: an onboarding record belongs to
/// one AWS account's instance, so the refusal names the account the call was made in
/// ([`::engine::emulator_account_id`], the one account every emulated call is made
/// in) beside the instance it asked about — which is how a caller tells "this
/// instance was never onboarded in THIS account" from a cross-account mistake. No
/// trailing period: the sentence ends on the id.
///
/// The one text the two operations whose own resource IS that job
/// (`GetInstanceOnboardingJobStatus` and `DeleteInstanceOnboardingJob`) share, so the
/// two cannot disagree about a request they both refuse.
pub fn onboarding_job_not_found_message(connect_instance_id: &str) -> String {
    format!(
        "Instance onboarding record does not exist for aws account id {} and instance id {connect_instance_id}",
        ::engine::emulator_account_id()
    )
}

/// What this service answers when a campaign's own `CampaignState` does not permit
/// the action a request asks of it — a lifecycle transition
/// ([`CampaignState::after`]) or a batch of outbound requests
/// ([`CampaignState::takes_outbound_requests`]) — at HTTP 409 under the modelled
/// `ConflictException`.
///
/// Composed from the request and the store: the campaign id the request addressed and
/// the state the campaign is at NOW, spelled as [`CampaignState::as_upper`] spells it.
/// The one text every state gate of a campaign action shares, so two of them cannot
/// disagree about a request they both refuse, and a caller that parses the state out of
/// the refusal to decide whether to start or to resume reads the same token whichever
/// call it made.
pub fn campaign_invalid_state_message(id: &str, state: CampaignState) -> String {
    format!("Campaign: {id} is in invalid state: {}", state.as_upper())
}

/// What a well-formed campaign id that names no campaign is refused with, where the
/// operation that was called states the lookup WITHOUT the id it was asked about.
///
/// This service words that one refusal in three sentences, and which of them an
/// operation answers is a fact of that operation's own wire rather than of the store:
/// the lookup sentence without the id (this one), the same lookup sentence NAMING the id
/// ([`campaign_id_not_found_message`]), and the data/lifecycle plane's own sentence about
/// the campaign ([`campaign_absent_message`]). The three live here together so an
/// operation picks one rather than spelling its own, and so the set of them is one thing
/// to read.
///
/// A FIXED service string: it carries nothing from the request, so every refusal of this
/// kind reads the same and the response is deterministic.
pub const CAMPAIGN_NOT_FOUND_MESSAGE: &str = "CampaignId not found.";

/// The same lookup sentence as [`CAMPAIGN_NOT_FOUND_MESSAGE`], NAMING the campaign id
/// the request addressed and ended with a period.
///
/// Composed from the request: a caller that addressed the wrong id learns WHICH id was
/// missing, which is the whole purpose of the id in this message. The one text every
/// operation whose wire states it shares — the rename, the configuration changes that
/// state it, and the deletion of the campaign itself — so two of them cannot disagree.
pub fn campaign_id_not_found_message(id: &str) -> String {
    format!("CampaignId not found {id}.")
}

/// What the operations that ACT on a campaign — its lifecycle transitions, the state it
/// is at and the outbound requests submitted against it — answer when the id names no
/// campaign: the campaign is reported absent rather than the id reported unresolved.
///
/// Composed from the request, so the refusal names the campaign the caller asked about.
/// The one text that plane shares, so two of its operations cannot disagree about a
/// request they both refuse.
pub fn campaign_absent_message(id: &str) -> String {
    format!("Campaign: {id} doesn't exist")
}

/// What this service answers when a request addresses a Connect instance that is not
/// onboarded to Campaigns in the region it was signed for: the instance is reported as
/// not onboarded, which is what a caller has to change before the call means anything.
///
/// Two operations' wire states it: the campaign LISTING, whose instance filter is
/// resolved before anything is listed (so an instance with no campaigns and one that is
/// no campaign target at all are not answered alike, at HTTP 400 under the modelled
/// `ValidationException`), and the removal of an instance's CONFIGURATION, which has
/// nothing to remove for such an instance (at HTTP 404 under the modelled
/// `ResourceNotFoundException`).
///
/// A FIXED service string: it names no instance and carries nothing from the request,
/// so every refusal of this kind reads the same.
pub const INSTANCE_NOT_ONBOARDED_MESSAGE: &str = "Instance is not onboarded";

/// What `DeleteConnectInstanceConfig` answers when the instance whose configuration it
/// was asked to remove still owns at least one campaign and the request named no
/// `campaignDeletionPolicy`, at HTTP 409 under the modelled `InvalidStateException`:
/// the campaigns of an instance depend on its configuration, so the un-forced delete is
/// blocked rather than leaving them pointing at a configuration that is gone.
///
/// A FIXED service string: it names neither the instance nor how many campaigns hold
/// it, so every refusal of this kind reads the same.
pub const INSTANCE_HAS_CAMPAIGNS_MESSAGE: &str = "This instance still has campaigns";

/// What a `DeleteConnectInstanceConfig` request's `campaignDeletionPolicy` decides for
/// the campaigns of the instance whose configuration it removes.
///
/// The member is optional, and its ABSENCE is not one of the policies: a caller that
/// has not said what is to become of the instance's campaigns gets the delete refused
/// while there are any ([`INSTANCE_HAS_CAMPAIGNS_MESSAGE`]) rather than one of the two
/// outcomes chosen for it — which is why the parameter exists at all. A caller that
/// names a policy has decided, so the delete goes through either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignDeletionPolicy {
    /// The request named no policy.
    Unspecified,
    /// [`CAMPAIGN_DELETION_POLICY_RETAIN_ALL`]: the configuration goes, the campaigns
    /// stay exactly as they are.
    RetainAll,
    /// [`CAMPAIGN_DELETION_POLICY_DELETE_ALL`]: the campaigns go with the
    /// configuration.
    DeleteAll,
}

impl CampaignDeletionPolicy {
    /// The policy a request carrying `policy` asks for — the value of its
    /// `campaignDeletionPolicy` member, already weighed against the model's catalog by
    /// the caller, and `None` when the request carries none.
    ///
    /// Each catalog value is named: a value outside the catalog never reaches here (the
    /// boundary walk and the handler's own check refuse it first), so one that did is this
    /// service's own mistake rather than a caller's, and the conservative reading of it is
    /// the one a request that decided nothing gets.
    pub fn of_request(policy: Option<&str>) -> Self {
        match policy {
            None => Self::Unspecified,
            Some(CAMPAIGN_DELETION_POLICY_DELETE_ALL) => Self::DeleteAll,
            Some(CAMPAIGN_DELETION_POLICY_RETAIN_ALL) => Self::RetainAll,
            Some(_) => Self::Unspecified,
        }
    }

    /// Whether the campaigns of the instance go with its configuration.
    fn deletes_campaigns(self) -> bool {
        matches!(self, Self::DeleteAll)
    }

    /// Whether a campaign the instance still owns BLOCKS the delete: only a request
    /// that named no policy at all leaves that decision unmade.
    fn blocked_by_campaigns(self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// What `DeleteCampaignChannelSubtypeConfig` answers when the channel subtype it was
/// asked to remove is the only one the campaign has configured, at HTTP 400 under the
/// modelled `ValidationException`: a campaign always keeps at least one channel
/// subtype, so the removal that would leave it with none is refused and nothing is
/// written.
///
/// A FIXED service string: it names neither the campaign nor the channel, so every
/// refusal of this kind reads the same.
pub const LAST_CHANNEL_SUBTYPE_MESSAGE: &str = "At least one channel subtype needs to be configured.";

/// The shortest campaign id this service accepts.
///
/// Source: the `@length` of `CampaignId`, the shape every campaign-id member and
/// `CampaignIdList`'s member target, whose minimum is 1 and maximum 256
/// (`smithy_query(query="shape", shape="CampaignId")`).
pub const CAMPAIGN_ID_MIN_LENGTH: i64 = 1;
/// See [`CAMPAIGN_ID_MIN_LENGTH`].
pub const CAMPAIGN_ID_MAX_LENGTH: i64 = 256;
/// `CampaignId`'s modelled `@pattern`, quoted exactly as the model states it.
pub const CAMPAIGN_ID_PATTERN: &str = "^[-:/a-zA-Z0-9]+$";

/// `timestamp` at the precision this service keeps a `date-time` instant to:
/// MILLISECONDS, with the sub-millisecond remainder of a submitted value dropped.
///
/// The service stores and echoes a caller-supplied instant to the millisecond, so a
/// value submitted with a finer remainder comes back without it: truncating on the way
/// IN is what makes that so for every later read of the record, rather than leaving two
/// readers to round the same stored value differently. Truncation and not rounding — the
/// instant a value names never moves forward.
///
/// The one place that precision lives, so every writer of a `date-time` member
/// (a campaign's schedule window, an outbound request's expiration) keeps records to
/// the same one.
pub fn millisecond_precision(
    timestamp: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    const NANOS_PER_MILLI: u32 = 1_000_000;
    let nanos = timestamp.subsec_nanos() / NANOS_PER_MILLI * NANOS_PER_MILLI;
    ::aws_smithy_types::DateTime::from_secs_and_nanos(timestamp.secs(), nanos)
}

/// `schedule` as the campaign record holds it: its run window at the precision
/// [`millisecond_precision`] keeps a `date-time` to.
///
/// The one place a schedule is prepared for the store, shared by `CreateCampaign` and
/// `UpdateCampaignSchedule`, so a campaign born with a window and one given a new one
/// hold it at the same precision and `DescribeCampaign` projects it the same way
/// whichever call wrote it.
pub fn stored_schedule(schedule: Schedule) -> Schedule {
    Schedule {
        start_time: millisecond_precision(schedule.start_time),
        end_time: millisecond_precision(schedule.end_time),
        refresh_frequency: schedule.refresh_frequency,
    }
}

/// What this service answers when a write is refused because the campaign name it
/// asks for is already held by another campaign of the same Connect instance, at
/// HTTP 409.
///
/// A FIXED service string: the modelled `ConflictException` carries only a
/// `message` and declares no per-resource template, and the recorded refusal of a
/// name collision is this one sentence with nothing taken from the request or the
/// store — no campaign name, no instance id, no campaign id — so every uniqueness
/// conflict reads the same. The one text every writer of a campaign name shares
/// (`CreateCampaign` and `UpdateCampaignName`), so two of them cannot disagree
/// about the invariant they both uphold (see
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::campaign_named`]).
pub const CAMPAIGN_NAME_TAKEN_MESSAGE: &str =
    "Failed due to conflict in current state of the resource.";

/// The `CampaignDeletionPolicy` value that has `DeleteConnectInstanceConfig` remove
/// the instance's existing campaigns along with its configuration, as the model's
/// catalog declares it (see `CAMPAIGN_DELETION_POLICY_VALUES` in
/// [`crate::ops::types`]).
pub const CAMPAIGN_DELETION_POLICY_DELETE_ALL: &str = "DELETE_ALL";

/// The `CampaignDeletionPolicy` value that retains the instance's existing
/// campaigns, from the same catalog: the policy that enacts nothing on them, and so
/// also how a request that names no policy at all is served — the conservative
/// reading, since a caller that did not ask for the campaigns to go keeps them.
pub const CAMPAIGN_DELETION_POLICY_RETAIN_ALL: &str = "RETAIN_ALL";

/// What `StartInstanceOnboardingJob` answers when the account's Amazon Connect registry
/// holds no instance under the id a request names: the instance id itself is reported
/// unresolved, naming it.
///
/// Composed from the request, so the refusal names the instance the caller asked about, as
/// the recorded response of this class does. The one operation whose wire states this
/// sentence today — the onboarding start; the removal of an instance's CONFIGURATION
/// reports the instance as not onboarded ([`INSTANCE_NOT_ONBOARDED_MESSAGE`]) and the
/// operations that merely need a usable instance report its usability
/// ([`INSTANCE_NOT_FOUND_MESSAGE`]) — so a second operation whose wire shows this sentence
/// states it from here rather than spelling its own.
pub fn instance_id_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceId not found {connect_instance_id}.")
}

/// What this service answers when the resource a request addresses depends on the
/// Campaigns configuration of a Connect instance the request's region holds none for:
/// there is no instance configuration to report, to configure, or to hang the addressed
/// resource off.
///
/// Composed from the request, so the refusal names the instance the caller asked
/// about, as the recorded responses of this class do. The one text shared by every
/// operation whose wire states it — the READ of `/v2/connect-instance/{id}/config`
/// (`GetConnectInstanceConfig`) and the two that write an instance's integrations
/// (`PutConnectInstanceIntegration` and `DeleteConnectInstanceIntegration`), which hang
/// off that same stored configuration — so two of them cannot disagree about an
/// instance this service holds no configuration for.
///
/// The REMOVAL of that configuration (`DeleteConnectInstanceConfig`) states its own
/// sentence instead, reporting the instance as not onboarded
/// ([`INSTANCE_NOT_ONBOARDED_MESSAGE`]), and the operations whose wire states the
/// instance's USABILITY answer [`INSTANCE_NOT_FOUND_MESSAGE`], which is the text
/// recorded for those — the two on the instance's communication limits among them, even
/// though those limits are a member of this same stored configuration (see
/// [`StoredInstanceOnboarding::communication_limits`]).
pub fn instance_config_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceConfig not found {connect_instance_id}.")
}

/// How long after the `StartInstanceOnboardingJob` call that started it an
/// onboarding job is due to have settled — on `SUCCEEDED`, or on the
/// `InstanceOnboardingJobFailureCode` its dependency check found.
///
/// AWS documents no duration for the job, so this is the emulator's own: long
/// enough that a caller which polls immediately still observes `IN_PROGRESS` (the
/// transient state the real job has), short enough that a poller reaches the
/// settled status straight away. Nothing sleeps for it — the due instant is stored
/// and the status is read off the clock.
pub const ONBOARDING_JOB_DURATION: std::time::Duration = std::time::Duration::from_millis(500);

/// The onboarding job of one Connect instance: the encryption config the
/// `StartInstanceOnboardingJob` call persisted, the instant its job is due to
/// have finished, what became of it, and the instance-wide configuration set on it
/// since.
#[derive(Debug, Clone)]
pub struct StoredInstanceOnboarding {
    pub connect_instance_id: String,
    /// What the instance's campaign data is encrypted with, exactly as the call
    /// that started the job supplied it.
    pub encryption_config: EncryptionConfig,
    /// The instant this job settles: the start instant plus
    /// [`ONBOARDING_JOB_DURATION`]. Before it the job reports `IN_PROGRESS`.
    pub settles_at: std::time::SystemTime,
    /// The `InstanceOnboardingJobFailureCode` this job settles on, or `None` for a job
    /// that settles `SUCCEEDED`.
    ///
    /// It is what makes a record's job a FAILED one: [`Self::status_at`] reports that
    /// status exactly while this member is set and the job has settled, and
    /// [`Self::to_job_status`] reports the code beside it, so a job's status and its
    /// failure code cannot disagree. The call that STARTS the job decides it — that is
    /// when the service resolves the dependencies the onboarding needs (see
    /// [`onboarding_encryption_failure`]) — so the outcome a reader reports is the
    /// outcome of the job that ran and not of whatever those dependencies look like
    /// when somebody happens to poll.
    pub failure_code: Option<String>,
    /// The instance-wide communication limits `PutInstanceCommunicationLimits`
    /// persisted for this instance, exactly as that call supplied them, and `None`
    /// while no call has set any. `GetInstanceCommunicationLimits` reports this
    /// member and nothing else, so the two cannot disagree. It lives on the
    /// instance's record, so an instance whose record goes takes its limits with it.
    pub communication_limits: Option<InstanceCommunicationLimitsConfig>,
}

impl StoredInstanceOnboarding {
    /// A job just started for `connect_instance_id` at `now`, due to settle one
    /// [`ONBOARDING_JOB_DURATION`] later on `failure_code`: `None` for a job whose
    /// dependencies all held, so it settles `SUCCEEDED`, and otherwise the
    /// `InstanceOnboardingJobFailureCode` the one that did not held it up.
    pub fn starting(
        connect_instance_id: String,
        encryption_config: EncryptionConfig,
        failure_code: Option<String>,
        now: std::time::SystemTime,
    ) -> Self {
        Self {
            connect_instance_id,
            encryption_config,
            settles_at: now + ONBOARDING_JOB_DURATION,
            failure_code,
            communication_limits: None,
        }
    }

    /// The job's status at `now`: `IN_PROGRESS` until the clock has passed the instant
    /// it settles at, and from then on the outcome the record carries — `FAILED` when
    /// it names the code the job ended on, `SUCCEEDED` when it names none.
    ///
    /// The span is never zero either way, so a caller that polls straight after the
    /// start call still sees the transient status a real job has before it fails.
    ///
    /// A pure read of the stored instant and outcome, so reporting the status writes
    /// nothing and two callers reading at the same moment see the same value.
    pub fn status_at(&self, now: std::time::SystemTime) -> &'static str {
        if now < self.settles_at {
            ONBOARDING_STATUS_IN_PROGRESS
        } else if self.failure_code.is_some() {
            ONBOARDING_STATUS_FAILED
        } else {
            ONBOARDING_STATUS_SUCCEEDED
        }
    }

    /// This record as the modelled `InstanceOnboardingJobStatus` shape the
    /// onboarding operations return.
    ///
    /// `failureCode` is the code the record names, reported exactly while the job is at
    /// `FAILED`: a job that has not settled yet has not failed, so it carries no code —
    /// the model declares the member for the failure of a job, not for its prospects.
    pub fn to_job_status(&self, now: std::time::SystemTime) -> InstanceOnboardingJobStatus {
        let status = self.status_at(now);
        InstanceOnboardingJobStatus {
            connect_instance_id: self.connect_instance_id.clone(),
            status: status.to_string(),
            failure_code: (status == ONBOARDING_STATUS_FAILED)
                .then(|| self.failure_code.clone())
                .flatten(),
        }
    }

    /// This record as the modelled `InstanceConfig` shape `GetConnectInstanceConfig`
    /// returns: the instance, the encryption config this record persisted, and the
    /// account's Campaigns service-linked role (see [`service_linked_role_arn`]).
    ///
    /// A config exists once the instance's onboarding job has finished, so the caller
    /// checks [`Self::status_at`] before projecting one.
    pub fn to_config(&self) -> InstanceConfig {
        InstanceConfig {
            connect_instance_id: self.connect_instance_id.clone(),
            service_linked_role_arn: service_linked_role_arn(),
            encryption_config: self.encryption_config.clone(),
        }
    }
}

/// The most campaigns this emulator's store holds, after which `CreateCampaign`
/// refuses with the modelled `ServiceQuotaExceededException` (HTTP 402) — the
/// error the model declares for "Request would cause a service quota to be
/// exceeded".
///
/// The store grows one record per `CreateCampaign` and shrinks only on an
/// explicit `DeleteCampaign`, so without a ceiling a caller could repeat the
/// create until the process runs out of memory. AWS's own per-instance campaign
/// quota is an adjustable Service Quota, so no particular figure would be
/// faithful to it; this one is the emulator's own, set far above any plausible
/// workload and enforced through the error AWS answers a quota breach with.
pub const MAX_CAMPAIGNS: usize = 1_000;

/// The most outbound requests one campaign's store holds in a buffer, after which a
/// further request in a batch is reported as a per-request failure carrying the code
/// the model declares for a campaign that cannot take another request right now:
/// `FailureCode` `BufferLimitExceeded` for a dial request, and
/// `ProfileOutboundRequestFailureCode` `Conflict` for a profile request, whose catalog
/// carries no buffer code and whose nearest entry is the one for a refusal the
/// resource's own state causes.
///
/// A campaign's buffer grows one record per accepted request and shrinks only as
/// requests pass their `expirationTime`, so without a ceiling a caller could submit
/// batches of long-lived requests until the process runs out of memory. The figure is
/// the emulator's own, set far above any plausible workload. It counts the PENDING
/// requests: an expired one is not in the buffer any more (see
/// [`StoredCampaign::expire_outbound_requests`] and
/// [`StoredCampaign::expire_profile_outbound_requests`]). Each buffer is counted on
/// its own, since each holds its own resource.
pub const MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN: usize = 10_000;

/// The most Connect instances this emulator's store holds onboarding records for,
/// after which `StartInstanceOnboardingJob` refuses to take another with the
/// modelled `ConflictException` (HTTP 409) — of the errors that operation declares,
/// the one that fits a refusal the service's own state and not the request causes.
/// A later fix that models a real onboarding quota should answer it the same way.
///
/// The store grows one record per newly onboarded instance, so without a ceiling a
/// caller could repeat the call with fresh instance ids until the process runs out
/// of memory. AWS publishes no such limit, so this is the emulator's own, set far
/// above any plausible workload.
pub const MAX_ONBOARDED_INSTANCES: usize = 1_000;

/// The most regions this store holds records for at once, counted per region and not
/// per record: past it a call that would give a further region its first resource is
/// refused exactly as a full store is, and nothing is written.
///
/// The region of a request is its SigV4 credential scope, which the caller chooses,
/// so without a ceiling a caller could repeat a write under fresh region names until
/// the process runs out of memory, each one carrying its own set of the stores above.
/// A partition's regions are a fixed, published set; AWS publishes no limit of this
/// kind, so the figure is the emulator's own, set well above the largest partition.
pub const MAX_REGIONS: usize = 64;

/// The service-linked role every onboarded instance's config names.
///
/// `InstanceConfig$serviceLinkedRoleArn` is required, and AWS fills it with the
/// Campaigns service-linked role of the caller's own account — one role per account,
/// not per instance — so it is built from the emulator's canonical account rather than
/// from anything in the request. IAM is global, hence the empty region segment.
pub fn service_linked_role_arn() -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        "role/aws-service-role/connect-campaigns.amazonaws.com/AWSServiceRoleForAmazonConnectCampaigns",
    )
}

/// What this service answers when a request whose own resource is NOT the instance's
/// configuration names a Connect instance that is not usable by Campaigns: one the
/// account's Amazon Connect registry does not hold, or one that no finished onboarding
/// job has enabled for Campaigns in the request's region.
///
/// A FIXED service string — it names no instance and carries nothing from the request,
/// which is what the recorded responses of this class carry — so every refusal of this
/// kind reads the same and the response is deterministic. The one text shared by the
/// operations whose wire states the instance's usability (creating a campaign, listing
/// an instance's integrations, and the two on the instance's communication limits), so
/// two of them cannot disagree about a request they both refuse.
///
/// Each of the other instance-scoped sentences is stated by its own operations: the
/// ones whose wire states the instance's CONFIGURATION answer
/// [`instance_config_not_found_message`], the onboarding START reports the instance id
/// itself as unresolved ([`instance_id_not_found_message`]), and the campaign listing
/// and the removal of an instance's configuration report the instance as not onboarded
/// ([`INSTANCE_NOT_ONBOARDED_MESSAGE`]).
///
/// The sentence itself is the shared gate's
/// ([`::service_common::connect_instance_registry::INSTANCE_NOT_USABLE_MESSAGE`]), stated
/// once for both ConnectCampaigns API versions, and named here because this service's own
/// `ResourceNotFoundException` (HTTP 404) is what carries it.
pub const INSTANCE_NOT_FOUND_MESSAGE: &str =
    ::service_common::connect_instance_registry::INSTANCE_NOT_USABLE_MESSAGE;

/// What a request's Connect instance being unusable on REGISTRY grounds is, for the
/// operations that merely require a usable instance (see
/// [`INSTANCE_NOT_FOUND_MESSAGE`]): the shared gate's two outcomes
/// ([`::service_common::connect_instance_registry`]), which both ConnectCampaigns API
/// versions read the registry for and each answers with the error ITS own model names.
pub use ::service_common::connect_instance_registry::ConnectInstanceRefusal;

/// What this service answers when the Amazon Connect instance registry — another
/// service's store, reached through the shared engine — cannot be read at all, so the
/// one text is shared by every operation that resolves an instance against it.
pub const INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE: &str =
    ::service_common::connect_instance_registry::REGISTRY_UNAVAILABLE_MESSAGE;

/// The registry half of the usability class [`INSTANCE_NOT_FOUND_MESSAGE`] describes:
/// `None` when `connect_instance_id` names an instance the account holds, and
/// otherwise which refusal the caller owes.
///
/// The shared gate ([`::service_common::connect_instance_registry`]) over the registry
/// owner's own read: Amazon Connect mints and drops every instance id — `CreateInstance`
/// is the only call that mints one and `DeleteInstance` the only one that drops it — so
/// the answer comes from THAT service's store through the one shared engine
/// (`::service_com_amazonaws_connect_amazonconnectservice::state::registry_holds_instance`)
/// rather than being kept a second time here: an instance created or deleted over the
/// Connect API is onboardable, or not, with no propagation step.
///
/// The one place that half is decided, so the operations that merely require a usable
/// instance cannot disagree about a request they all refuse. The other half — whether
/// a finished onboarding job has enabled the instance for Campaigns in the request's
/// region — is this service's own store's answer
/// ([`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::is_onboarded`]),
/// and both refuse with the same text.
///
/// LOCK ORDER: the registry read takes the Connect store's lock, holds no other while it
/// does, and hands back an owned value rather than a guard — so this is called with NO
/// guard of this service's store held, which is why a handler asks it BEFORE locking its
/// own store.
pub fn connect_instance_registry_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> Option<ConnectInstanceRefusal> {
    ::service_common::connect_instance_registry::connect_instance_refusal(
        ::service_com_amazonaws_connect_amazonconnectservice::state::registry_holds_instance(
            engine,
            connect_instance_id,
        ),
    )
}

/// The `InstanceOnboardingJobFailureCode` an onboarding job started with
/// `encryption_config` in `region` settles on, or `None` for one whose encryption key
/// the service can use \u2014 which is every job whose config asks for the service's own
/// key, since such a job needs no key of the caller's at all.
///
/// Onboarding an instance puts its campaign data on the KMS key the config names:
/// "if enabled, a KMS key id needs to be provided"
/// (`smithy_query(query="shape",shape="EncryptionConfig")`), so a key the account does
/// not hold in the request's region, or holds in a form the service cannot encrypt
/// with, is a job the service cannot finish. AWS owns that key in KMS \u2014 `CreateKey` is
/// the only call that mints one and `ScheduleKeyDeletion` the only one that takes it
/// away \u2014 so the answer is read from THAT service's own store through the one shared
/// engine, in every identifier form KMS itself accepts, rather than guessed from the
/// text of the ARN.
///
/// The two codes are the two the catalog declares about a KMS key: the key resolves to
/// nothing ([`ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND`]), or it resolves and the service
/// still cannot use it to encrypt ([`ONBOARDING_FAILURE_KMS_ACCESS_DENIED`]) \u2014 a key
/// disabled, awaiting deletion or awaiting its material, one whose usage is not
/// encryption, and an identifier whose account segment names no account at all, which
/// KMS refuses for want of permission before it looks anything up.
///
/// `None` when KMS's store cannot be read at all (the engine is not the shared
/// in-memory one): nothing has then been attributed to the key, and a condition nobody
/// attributed to the key fails no job.
///
/// LOCK ORDER: this takes KMS's store lock, holds no other while it does, and hands
/// back an owned value \u2014 so it is called with NO guard of this service's store held,
/// which is why a handler asks it BEFORE locking its own.
pub fn onboarding_encryption_failure<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    encryption_config: &EncryptionConfig,
) -> Option<String> {
    // A config that is not on a customer key names no key to resolve: the service
    // encrypts with its own, which is always there.
    if !encryption_config.enabled {
        return None;
    }
    let key_arn = encryption_config.key_arn.as_deref()?;
    let state = ::service_com_amazonaws_kms_trentservice::state::service_state(engine)?;
    let store = state.lock();
    match store.resolve(region, key_arn) {
        // A key the service can use in a cryptographic operation right now, and whose
        // usage is the symmetric encryption a data key is generated under, is a key the
        // onboarding can put the instance's data on.
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::Found(key)
            if key.is_usable() && key.is_symmetric_encryption() =>
        {
            None
        }
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::Found(_) => {
            Some(ONBOARDING_FAILURE_KMS_ACCESS_DENIED.to_string())
        }
        // An identifier KMS cannot read as one of its own names no key either, so it is
        // reported as the key that was not found rather than as a key held back.
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::NotFound
        | ::service_com_amazonaws_kms_trentservice::state::KeyRef::MalformedArn => {
            Some(ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND.to_string())
        }
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::UnauthorizedAccount => {
            Some(ONBOARDING_FAILURE_KMS_ACCESS_DENIED.to_string())
        }
    }
}

/// The most integrations one Connect instance's store holds, after which a further
/// put is refused rather than taken.
///
/// AWS publishes no limit on the integrations of a Connect instance, so this figure
/// is the emulator's own: without a ceiling a caller could put integrations until the
/// process runs out of memory. It is set far above any plausible workload — the model
/// declares three integration kinds, and one instance integrates few resources of
/// each.
pub const MAX_INTEGRATIONS_PER_INSTANCE: usize = 100;

/// The most Connect instances one region's integration store holds records for at
/// once, counted per instance and not per integration: past it a put that would give a
/// FURTHER instance its first integration is refused exactly as an instance already at
/// [`MAX_INTEGRATIONS_PER_INSTANCE`] is, and nothing is written.
///
/// The instance id a put carries is the caller's, so the per-instance ceiling alone
/// bounds no growth: without a ceiling on the KEYS a caller could repeat the put under
/// fresh ids until the process runs out of memory. It is [`MAX_ONBOARDED_INSTANCES`],
/// the most instances one region holds onboarding records for, because only an
/// onboarded instance has integrations to hold.
pub const MAX_INTEGRATION_INSTANCES: usize = MAX_ONBOARDED_INSTANCES;

/// Which of the three integrations the `IntegrationConfig`, `IntegrationIdentifier`
/// and `IntegrationSummary` unions each declare a member for a record is.
///
/// The kind and the ARN of the integrated resource together identify one integration
/// of one Connect instance: that pair is exactly what the matching
/// `IntegrationIdentifier` member carries (`customerProfiles` a domain ARN, `qConnect`
/// a knowledge-base ARN, `lambda` a function ARN), so it is the key an integration is
/// stored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationKind {
    CustomerProfiles,
    QConnect,
    Lambda,
}

impl IntegrationKind {
    /// The union member this kind is carried in, as the model names it — the key a
    /// request or response body carries the integration under.
    pub fn member_name(self) -> &'static str {
        match self {
            Self::CustomerProfiles => "customerProfiles",
            Self::QConnect => "qConnect",
            Self::Lambda => "lambda",
        }
    }
}

/// The identity of the integration `config` configures: its kind, and the ARN of the
/// resource it integrates (see [`IntegrationKind`]).
///
/// The one place that pair is read off a config, so the key a put stores under and
/// the key a reader matches on cannot disagree.
pub fn integration_key(config: &IntegrationConfig) -> (IntegrationKind, &str) {
    match config {
        IntegrationConfig::customerProfiles(config) => (
            IntegrationKind::CustomerProfiles,
            config.domain_arn.as_str(),
        ),
        IntegrationConfig::qConnect(config) => (
            IntegrationKind::QConnect,
            config.knowledge_base_arn.as_str(),
        ),
        IntegrationConfig::Lambda(config) => {
            (IntegrationKind::Lambda, config.function_arn.as_str())
        }
    }
}

/// The identity `identifier` names: the kind of integration, and the ARN of the
/// resource it integrates (see [`IntegrationKind`]).
///
/// The `IntegrationIdentifier` counterpart of [`integration_key`]: the model declares
/// the same kind-and-ARN pair in both the config a put carries and the identifier a
/// delete carries, and reading it in one place is what keeps the key a put stores
/// under and the key a delete resolves from disagreeing.
pub fn integration_identifier_key(
    identifier: &IntegrationIdentifier,
) -> (IntegrationKind, &str) {
    match identifier {
        IntegrationIdentifier::customerProfiles(identifier) => (
            IntegrationKind::CustomerProfiles,
            identifier.domain_arn.as_str(),
        ),
        IntegrationIdentifier::qConnect(identifier) => (
            IntegrationKind::QConnect,
            identifier.knowledge_base_arn.as_str(),
        ),
        IntegrationIdentifier::Lambda(identifier) => {
            (IntegrationKind::Lambda, identifier.function_arn.as_str())
        }
    }
}

/// One integration of one Connect instance: the config the
/// `PutConnectInstanceIntegration` call persisted, and where in the store's order it
/// sits.
#[derive(Debug, Clone)]
pub struct StoredIntegration {
    /// The store-wide sequence number this integration was first put at — the value
    /// a `ListConnectInstanceIntegrations` cursor names. Updating an integration
    /// keeps it, so a put does not reorder a page a caller is part way through.
    pub sequence: u64,
    /// The integration config, exactly as the call that put it supplied it.
    pub config: IntegrationConfig,
}

impl StoredIntegration {
    /// Which integration this is: the kind and resource ARN it is stored under (see
    /// [`integration_key`]).
    pub fn key(&self) -> (IntegrationKind, &str) {
        integration_key(&self.config)
    }

    /// This record as the modelled `IntegrationSummary` `ListConnectInstanceIntegrations`
    /// returns: the same union member as the stored config, carrying the values that
    /// config holds.
    pub fn to_summary(&self) -> IntegrationSummary {
        match &self.config {
            IntegrationConfig::customerProfiles(config) => {
                IntegrationSummary::customerProfiles(CustomerProfilesIntegrationSummary {
                    domain_arn: config.domain_arn.clone(),
                    object_type_names: config.object_type_names.clone(),
                })
            }
            IntegrationConfig::qConnect(config) => {
                IntegrationSummary::qConnect(QConnectIntegrationSummary {
                    knowledge_base_arn: config.knowledge_base_arn.clone(),
                })
            }
            IntegrationConfig::Lambda(config) => {
                IntegrationSummary::Lambda(LambdaIntegrationSummary {
                    function_arn: config.function_arn.clone(),
                })
            }
        }
    }
}

/// Which campaign a call is about: the region whose endpoint holds it, and its id
/// within that region.
///
/// A campaign is identified by the PAIR — the same id in two regions names two
/// different resources — so the methods that take a campaign's id alongside several
/// values of its own take this instead of two loose strings.
#[derive(Debug, Clone, Copy)]
pub struct CampaignRef<'a> {
    pub region: &'a str,
    pub id: &'a str,
}

/// Why the store refused to take a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::insert_new_campaign`]
/// reports it. Nothing is stored in any case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertRejected {
    /// The store already holds [`MAX_CAMPAIGNS`] campaigns. The caller answers
    /// this with the modelled `ServiceQuotaExceededException` (HTTP 402).
    StoreFull,
    /// A campaign is already stored under that id, so taking this one would
    /// discard the existing record. A freshly minted id cannot collide, so the
    /// caller answers this as a server fault rather than a client error.
    IdTaken,
    /// The Connect instance this campaign names already holds a campaign of that
    /// name, and a campaign name identifies a campaign within its instance (see
    /// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::campaign_named`]).
    /// The caller answers this with the modelled `ConflictException` (HTTP 409).
    NameTaken,
}

/// Why the store refused to rename a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::rename_campaign`]
/// reports it. The stored name is unchanged in either case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameRejected {
    /// The region holds no campaign under that id, so there is nothing to rename.
    /// The caller answers this with the modelled `ResourceNotFoundException`
    /// (HTTP 404).
    NotFound,
    /// Another campaign of the same Connect instance already holds that name, and a
    /// campaign name identifies a campaign within its instance (see
    /// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::campaign_named`]).
    /// Answered with the modelled `ConflictException` (HTTP 409), whose message
    /// ([`CAMPAIGN_NAME_TAKEN_MESSAGE`]) names neither the instance nor the name,
    /// so the rejection carries nothing beyond which invariant stopped the write.
    NameTaken,
}

/// Why the store refused to remove a campaign's channel-subtype config block, as
/// [`StoredCampaign::clear_channel_subtype_config`] reports it: the block named is the
/// only one the campaign holds, and a campaign always keeps at least one configured
/// channel subtype. Nothing is written.
///
/// The caller answers this with the modelled `ValidationException` (HTTP 400) carrying
/// [`LAST_CHANNEL_SUBTYPE_MESSAGE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastChannelSubtype;

/// Why the store refused a lifecycle action on a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::act_on_campaign`]
/// reports it. The stored lifecycle state is unchanged in either case.
///
/// [`Self::message`] carries the sentence the refusal is worded with, so the four
/// lifecycle operations refuse alike while each maps the rejection to the error
/// variant its own model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleRejected {
    /// The region holds no campaign under that id, so there is no lifecycle to move.
    /// Carries the id judged, so [`Self::message`] can name the campaign the request
    /// addressed. The caller answers this with the modelled `ResourceNotFoundException`
    /// (HTTP 404).
    NotFound { id: String },
    /// The campaign is at `state` at the instant the call was judged and the action
    /// asked for is not legal from there (see [`CampaignState::after`]). Carries the
    /// id judged, so [`Self::message`] can name the campaign the request did. The
    /// caller answers this as the state conflict it is: the modelled
    /// `ConflictException` (HTTP 409) carrying [`campaign_invalid_state_message`].
    IllegalAction { id: String, state: CampaignState },
}

impl LifecycleRejected {
    /// The sentence this refusal is reported with.
    pub fn message(&self) -> String {
        match self {
            // The lifecycle plane reports the campaign absent, naming it: see
            // [`campaign_absent_message`] for the three sentences this service words a
            // campaign-id lookup with and which plane states which.
            Self::NotFound { id } => campaign_absent_message(id),
            Self::IllegalAction { id, state } => campaign_invalid_state_message(id, *state),
        }
    }
}

/// Why the store refused a configuration change to a campaign, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::configurable_campaign`]
/// reports it. Nothing is written in either case.
///
/// [`Self::message`] carries the sentence a STATE refusal is worded with, so every
/// campaign-configuration operation reports an unconfigurable campaign alike while each
/// maps the rejection to the error variant its own model declares. The NOT-FOUND sentence
/// is the calling operation's and not this enum's: AWS words that refusal per operation —
/// six of these operations name the id ([`campaign_id_not_found_message`]) and the rest
/// state the id-less lookup ([`CAMPAIGN_NOT_FOUND_MESSAGE`]) — and which one an operation
/// states is a fact of its own wire, which the store cannot know. So each handler states
/// its own on this arm, picking one of the sentences `crate::state` holds rather than
/// spelling it, and [`Self::message`] answers the id-less one for the operations whose wire
/// states that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationRejected {
    /// The region holds no campaign under that id, so there is no configuration to
    /// change. The caller answers this with the modelled `ResourceNotFoundException`
    /// (HTTP 404), worded with the sentence ITS own wire states (see the type's own
    /// documentation above).
    NotFound,
    /// The campaign is at `state` at the instant the call was judged and that state
    /// takes no further configuration change (see
    /// [`CampaignState::accepts_configuration_change`]). The caller answers this with
    /// the modelled `InvalidCampaignStateException` (HTTP 409), whose `state` member
    /// carries `state.as_wire()`.
    NotConfigurable { state: CampaignState },
}

impl ConfigurationRejected {
    /// The sentence this refusal is reported with: the state clause for a campaign that
    /// takes no further change, and the id-less lookup sentence for an absent campaign —
    /// which is what the operations whose wire states that one answer, the rest stating
    /// their own (see the type's own documentation).
    pub fn message(self) -> String {
        match self {
            Self::NotFound => CAMPAIGN_NOT_FOUND_MESSAGE.to_string(),
            Self::NotConfigurable { state } => {
                format!("A campaign in state {} cannot be reconfigured", state.as_wire())
            }
        }
    }
}

/// Why the store refused to take an integration into a Connect instance, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::put_instance_integration`]
/// reports it. Nothing is written in either case.
///
/// Two ceilings, kept apart because each is a different thing being full, and
/// [`Self::message`] carries the sentence that names the one actually hit — so the
/// refusal never describes a state the store is not in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationPutRefused {
    /// The instance already holds [`MAX_INTEGRATIONS_PER_INSTANCE`] integrations.
    InstanceFull,
    /// The store has no room for a FURTHER instance's integrations: the region already
    /// holds them for [`MAX_INTEGRATION_INSTANCES`] instances, or the store already
    /// holds [`MAX_REGIONS`] regions and this would be a new one's first.
    StoreFull,
}

impl IntegrationPutRefused {
    /// The sentence this refusal is reported with.
    pub fn message(self) -> String {
        match self {
            Self::InstanceFull => format!(
                "The maximum number of integrations ({MAX_INTEGRATIONS_PER_INSTANCE}) are \
                 already configured for this Connect instance"
            ),
            Self::StoreFull => format!(
                "The maximum number of Connect instances ({MAX_INTEGRATION_INSTANCES}) already \
                 have integrations configured"
            ),
        }
    }
}

/// Why the store refused to take a batch of outbound requests into a campaign's
/// buffer, as
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::dialable_campaign`]
/// reports it. Nothing is written in either case.
///
/// [`Self::message`] carries the sentence the refusal is worded with, so both batch
/// operations refuse alike while each maps the rejection to the error variant its own
/// model declares and keeps its own per-request failure catalog. A campaign the region
/// does not hold is reported absent by the sentence the plane that ACTS on a campaign
/// states ([`campaign_absent_message`]), which both batch operations answer with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialRejected {
    /// The region holds no campaign under that id, so there is no buffer to take the
    /// batch into. Carries the id judged, so [`Self::message`] can name the campaign the
    /// request addressed. The caller answers this with the modelled
    /// `ResourceNotFoundException` (HTTP 404).
    NotFound { id: String },
    /// The campaign is at `state` at the instant the call was judged and that state
    /// takes no outbound requests (see [`CampaignState::takes_outbound_requests`]).
    /// Carries the id judged, so [`Self::message`] can name the campaign. The caller
    /// answers this as the state conflict it is: the modelled `ConflictException`
    /// (HTTP 409) carrying [`campaign_invalid_state_message`].
    NotDialable { id: String, state: CampaignState },
}

impl DialRejected {
    /// The sentence this refusal is reported with.
    pub fn message(&self) -> String {
        match self {
            Self::NotFound { id } => campaign_absent_message(id),
            Self::NotDialable { id, state } => campaign_invalid_state_message(id, *state),
        }
    }
}

/// What a batch operation answers when the campaign it resolved is no longer in the
/// store by the time a request of the batch is taken into its buffer: a server fault,
/// since the resolve and the walk happen under the one lock the handler holds, so the
/// condition is answered rather than panicked on.
///
/// Composed from the request, so the fault names the campaign the batch was for. The
/// one text both batch operations report it with, so neither can drift from the other.
pub fn campaign_vanished_message(id: &str) -> String {
    format!("campaign {id} went missing while its batch was being taken")
}

/// The records of ONE region: everything a client talking to that region's endpoint
/// can see or change.
///
/// `campaigns` answers a `{id}` lookup in O(1); `order` carries the creation order as
/// `(sequence number, id)` pairs, which the methods below keep in step with the
/// map, so `ListCampaigns` paginates deterministically and its `nextToken` can
/// name the sequence number the last page ended at. Growth is bounded:
/// [`MAX_CAMPAIGNS`] records, enforced by [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::insert_new_campaign`].
///
/// `instance_onboarding` is the second map: one onboarding record per Connect
/// instance, keyed by its instance id, bounded at [`MAX_ONBOARDED_INSTANCES`] by
/// [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::start_instance_onboarding`].
/// It is the one place "is this instance a campaign target?" is answered, so
/// `CreateCampaign`'s gate and `GetConnectInstanceConfig` /
/// `GetInstanceOnboardingJobStatus` cannot disagree.
///
/// `instance_integrations` is the third: the integrations of each Connect instance,
/// which `PutConnectInstanceIntegration` writes and
/// `ListConnectInstanceIntegrations` projects, so the list reports what the puts
/// stored and nothing else.
#[derive(Debug, Default)]
struct RegionStore {
    campaigns: HashMap<String, StoredCampaign>,
    order: Vec<(u64, String)>,
    instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
    instance_integrations: HashMap<String, Vec<StoredIntegration>>,
}

/// Per-service state for com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2. A handler reaches it through [`service_state`].
///
/// A campaign and a Connect instance configuration are REGIONAL resources, reachable
/// only through the endpoint of the region they were created in — which the ARN this
/// service mints states ([`campaign_arn`]) — so the records live in one
/// [`RegionStore`] per region, keyed by the credential-scope region of the request
/// that wrote them ([`request_region`]). Every method below takes that region and
/// resolves inside its store alone: a resource another region holds is absent from a
/// page here and its id resolves to nothing, so no caller sees or changes a resource
/// its endpoint does not serve. A region nothing has been written to yet has no entry,
/// which every reader answers exactly as it answers an empty one, so reading a region
/// never creates it.
///
/// The sequence counters stay store-wide: an id or cursor value is minted from one
/// counter whatever region takes it, so no two campaigns, dial requests, profile
/// requests or integrations anywhere in the store share one.
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    /// The records of each region, keyed by the credential-scope region they belong
    /// to. Bounded at [`MAX_REGIONS`] regions, enforced by [`Self::region_mut`] — the
    /// one path that gives a region its first record.
    regions: HashMap<String, RegionStore>,
    next_sequence: u64,
    next_dial_sequence: u64,
    /// The counter the `ProfileOutboundRequestId`s are minted from, store-wide so no
    /// two profile outbound requests share an id (see
    /// [`Self::take_profile_outbound_request`]).
    next_profile_request_sequence: u64,
    next_integration_sequence: u64,
}

impl ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State {
    /// The records `region` holds, or `None` when nothing has been written to that
    /// region yet.
    ///
    /// The read path: a region with no entry holds no resources, which every reader
    /// answers exactly as it answers an empty store, so a read never adds an entry and
    /// never changes what a later caller sees.
    fn region(&self, region: &str) -> Option<&RegionStore> {
        self.regions.get(region)
    }

    /// The records `region` holds, for a WRITER — created empty when that region has
    /// none yet, since the write is what gives the region its first resource.
    ///
    /// `None` when the region has no records and the store already holds
    /// [`MAX_REGIONS`] regions: the caller chooses the region it signs for, so a new
    /// region is taken only while there is room for one, and a refusal creates
    /// nothing. A region already in the store is always handed back.
    fn region_mut(&mut self, region: &str) -> Option<&mut RegionStore> {
        if !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS {
            return None;
        }
        Some(self.regions.entry(region.to_string()).or_default())
    }

    /// The id the next campaign this store takes will carry: its creation
    /// sequence number, zero-padded to a fixed width.
    ///
    /// Minted from the store's own counter, never from anything in the request,
    /// and unique because the counter never repeats. The text is within
    /// `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256 length bound.
    /// Call it holding the guard you then insert through, so two concurrent
    /// creates cannot be handed the same id.
    pub fn next_campaign_id(&self) -> String {
        format!("{:012}", self.next_sequence)
    }

    /// Store `campaign` at the end of the creation order and hand back the record
    /// now in the store — so a handler builds its response from the store rather
    /// than from the request it just read.
    ///
    /// Refuses, storing nothing, when the campaign's Connect instance already
    /// holds a campaign of that name, when the store is full, or when the id is
    /// already taken (see [`InsertRejected`]). The checks and the insert happen
    /// under the one lock the caller holds, so concurrent creates cannot both pass
    /// them — of two creates racing on one name, exactly one stores a campaign.
    ///
    /// The configuration the record carries is stored the way every other writer of it
    /// stores it: see [`configured`].
    pub fn insert_new_campaign(
        &mut self,
        region: &str,
        campaign: StoredCampaign,
    ) -> Result<StoredCampaign, InsertRejected> {
        // A record enters the store in the one representation of "configured for
        // nothing" every other writer of these members uses: a config the create was
        // given that holds no member at all configures nothing, so the record carries
        // no member rather than an empty structure. This is the one place that holds
        // for every create, so a campaign cannot be born carrying a config state the
        // update and removal paths would never leave behind.
        let mut campaign = campaign;
        // Normalising the config the create was given is part of that same create, so
        // it stamps the record at the create's own instant rather than moving the
        // campaign's last-modified instant past its creation.
        let created_at = campaign.created_at;
        if let Some(config) = campaign.channel_subtype_config.take() {
            campaign.set_channel_subtype_config(config, created_at);
        }
        if let Some(config) = campaign.communication_limits_override.take() {
            campaign.set_communication_limits_override(config, created_at);
        }
        // A campaign name identifies a campaign within its Connect instance, so a
        // record whose (instance, name) pair is already held is refused before any
        // capacity question: the resource it names exists, and taking a second one
        // would leave the instance with two campaigns answering to one name. This is
        // the one place that invariant is enforced, so every writer of a campaign
        // name upholds it.
        if self
            .campaign_named(region, &campaign.connect_instance_id, &campaign.name)
            .is_some()
        {
            return Err(InsertRejected::NameTaken);
        }
        let sequence = self.next_sequence;
        let outcome = {
            // Past the region ceiling there is no room for a region's first campaign,
            // which the caller answers exactly as it answers a full store.
            let Some(store) = self.region_mut(region) else {
                return Err(InsertRejected::StoreFull);
            };
            let full = store.campaigns.len() >= MAX_CAMPAIGNS;
            let id = campaign.id.clone();
            match store.campaigns.entry(id.clone()) {
                // Taking this campaign would discard the one already under that id.
                std::collections::hash_map::Entry::Occupied(_) => Err(InsertRejected::IdTaken),
                std::collections::hash_map::Entry::Vacant(_) if full => {
                    Err(InsertRejected::StoreFull)
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let stored = slot.insert(campaign).clone();
                    // Only now, with the record actually in the map, does the order
                    // gain an entry and a sequence number — so "a refusal stores
                    // nothing" holds structurally on every arm above.
                    store.order.push((sequence, id));
                    Ok(stored)
                }
            }
        };
        if outcome.is_ok() {
            self.next_sequence = sequence.saturating_add(1);
        }
        outcome
    }

    /// The campaign `region` stores under `id`, or `None` when that region stores
    /// none — including when another region stores one under that id, which is a
    /// different resource and not this endpoint's to answer for.
    pub fn campaign(&self, region: &str, id: &str) -> Option<&StoredCampaign> {
        self.region(region)?.campaigns.get(id)
    }

    /// The campaign `connect_instance_id` holds under `name`, or `None` when that
    /// instance holds none.
    ///
    /// A campaign name is scoped to its Connect instance: the same name under two
    /// instances names two campaigns, and the comparison is exact — a name differing
    /// in case or in surrounding whitespace is a different name, since the model puts
    /// no normalization on `CampaignName`.
    ///
    /// Walked in creation order, so the record answered for is the oldest one, not
    /// whichever the hash map happens to yield first.
    pub fn campaign_named(
        &self,
        region: &str,
        connect_instance_id: &str,
        name: &str,
    ) -> Option<&StoredCampaign> {
        self.campaigns_in_order(region)
            .map(|(_, campaign)| campaign)
            .find(|campaign| {
                campaign.connect_instance_id == connect_instance_id && campaign.name == name
            })
    }

    /// The campaign `region` stores under `id`, for a handler that moves its state.
    /// Resolved exactly as [`Self::campaign`] resolves it, so a lifecycle write
    /// reaches only the region it was signed for.
    pub fn campaign_mut(&mut self, region: &str, id: &str) -> Option<&mut StoredCampaign> {
        self.regions.get_mut(region)?.campaigns.get_mut(id)
    }

    /// Rename the campaign `campaign` names to `name`, upholding the invariant that
    /// a campaign name identifies a campaign within its Connect instance (see
    /// [`Self::campaign_named`]).
    ///
    /// The lookup, the uniqueness check and the write happen under the one lock the
    /// caller holds, so two concurrent renames onto one name cannot both pass it.
    /// Renaming a campaign to the name it already carries is accepted and changes
    /// nothing — the campaign is not in conflict with itself — which is what makes
    /// the rename idempotent. A refusal ([`RenameRejected`]) writes nothing.
    pub fn rename_campaign(
        &mut self,
        campaign: CampaignRef<'_>,
        name: &str,
        now: SystemTime,
    ) -> Result<(), RenameRejected> {
        let Some(stored) = self.campaign(campaign.region, campaign.id) else {
            return Err(RenameRejected::NotFound);
        };
        if stored.name == name {
            return Ok(());
        }
        let connect_instance_id = stored.connect_instance_id.clone();
        if self
            .campaign_named(campaign.region, &connect_instance_id, name)
            .is_some()
        {
            return Err(RenameRejected::NameTaken);
        }
        match self.campaign_mut(campaign.region, campaign.id) {
            Some(stored) => {
                stored.name = name.to_string();
                // The name is part of the campaign's definition, so the write stamps
                // the record; the rename onto the name it already carries returned
                // above, having written nothing at all.
                stored.mark_modified(now);
                Ok(())
            }
            // The lookup above succeeded under this same borrow, so this arm is
            // unreachable; it is reported rather than ignored so that an `Ok` from
            // this method always means the name was written.
            None => Err(RenameRejected::NotFound),
        }
    }

    /// Move the campaign `campaign` names through `action`, judging the call against
    /// the state the campaign is at `now`.
    ///
    /// The one place the lifecycle rule of this service lives, so
    /// `StartCampaign`, `PauseCampaign`, `ResumeCampaign` and `StopCampaign` cannot
    /// disagree about it: the id is resolved in the region the request was signed for
    /// (a campaign is reachable only through the endpoint of the region it was created
    /// in, so a lifecycle write never reaches another region's campaign), the state
    /// judged is the CLOCK-derived one ([`StoredCampaign::state_at`] — a campaign whose
    /// scheduled run window has closed is `Completed`, which no action moves out of),
    /// the move is the one [`CampaignState::after`] names, and the state it moves to is
    /// written and handed back. A refusal ([`LifecycleRejected`]) writes nothing, and
    /// names the state it judged so the caller's refusal can report it.
    ///
    /// The lookup, the check and the write happen under the one lock the caller holds,
    /// so two concurrent lifecycle calls cannot both pass the check from one state.
    pub fn act_on_campaign(
        &mut self,
        campaign: CampaignRef<'_>,
        action: CampaignAction,
        now: SystemTime,
    ) -> Result<CampaignState, LifecycleRejected> {
        let Some(stored) = self.campaign_mut(campaign.region, campaign.id) else {
            return Err(LifecycleRejected::NotFound {
                id: campaign.id.to_string(),
            });
        };
        let current = stored.state_at(now);
        let Some(next) = current.after(action) else {
            return Err(LifecycleRejected::IllegalAction {
                id: campaign.id.to_string(),
                state: current,
            });
        };
        stored.lifecycle_state = next;
        Ok(next)
    }

    /// The campaign `campaign` names, for a handler about to change its
    /// CONFIGURATION — or the refusal that handler answers instead, judged against the
    /// state the campaign is at `now`.
    ///
    /// The one place the configuration-change rule of this service lives, so
    /// `UpdateCampaignSource`, `UpdateCampaignSchedule`,
    /// `UpdateCampaignFlowAssociation`, the communication-limits, communication-time
    /// and entry-limits updates and their deletions cannot disagree about it: the id is
    /// resolved in the region the request was signed for (a campaign is reachable only
    /// through the endpoint of the region it was created in, so a configuration write
    /// never reaches another region's campaign), and the state judged is the
    /// CLOCK-derived one ([`StoredCampaign::state_at`] — a campaign whose scheduled run
    /// window has closed is `Completed`, which takes no further configuration change)
    /// rather than the one the last lifecycle call wrote.
    ///
    /// A refusal ([`ConfigurationRejected`]) hands back no record, so nothing is
    /// written, and names the state it judged so the caller's refusal can report it.
    /// What belongs to the CALLING operation is only its own write and the mapping from
    /// a rejection to the error variant its model declares.
    ///
    /// The lookup, the check and the write happen under the one lock the caller holds,
    /// so a concurrent delete of the campaign cannot leave a configuration change on a
    /// record no longer in the store.
    pub fn configurable_campaign(
        &mut self,
        campaign: CampaignRef<'_>,
        now: SystemTime,
    ) -> Result<&mut StoredCampaign, ConfigurationRejected> {
        let Some(stored) = self.campaign_mut(campaign.region, campaign.id) else {
            return Err(ConfigurationRejected::NotFound);
        };
        let current = stored.state_at(now);
        if !current.accepts_configuration_change() {
            return Err(ConfigurationRejected::NotConfigurable { state: current });
        }
        Ok(stored)
    }

    /// The campaign `campaign` names, for a handler about to take a batch of outbound
    /// requests into one of its buffers — or the refusal that handler answers instead,
    /// judged against the state the campaign is at `now`.
    ///
    /// The one place the dialability rule of this service lives, so
    /// `PutOutboundRequestBatch` and `PutProfileOutboundRequestBatch` cannot disagree
    /// about it: the id is resolved in the region the request was signed for (a campaign
    /// is reachable only through the endpoint of the region it was created in, so a
    /// batch never reaches another region's campaign), and the state judged is the
    /// CLOCK-derived one ([`StoredCampaign::state_at`] — a campaign whose scheduled run
    /// window has closed is `Completed`, which takes no requests) rather than the one
    /// the last lifecycle call wrote.
    ///
    /// A refusal ([`DialRejected`]) hands back no record, so nothing is written, and
    /// names the state it judged so the caller's refusal can report it. What belongs to
    /// the CALLING operation is only which buffer it writes, the per-request failure
    /// catalog it reports through, and the mapping from a rejection to the error variant
    /// its model declares.
    ///
    /// The resolve and the per-request walk that follows happen under the one lock the
    /// caller holds, so a concurrent lifecycle call or delete cannot leave a request
    /// buffered on a campaign that no longer takes one.
    pub fn dialable_campaign(
        &self,
        campaign: CampaignRef<'_>,
        now: SystemTime,
    ) -> Result<&StoredCampaign, DialRejected> {
        let Some(stored) = self.campaign(campaign.region, campaign.id) else {
            return Err(DialRejected::NotFound {
                id: campaign.id.to_string(),
            });
        };
        let current = stored.state_at(now);
        if !current.takes_outbound_requests() {
            return Err(DialRejected::NotDialable {
                id: campaign.id.to_string(),
                state: current,
            });
        }
        Ok(stored)
    }

    /// Remove the campaign `region` stores under `id`, returning it when it was there.
    pub fn remove_campaign(&mut self, region: &str, id: &str) -> Option<StoredCampaign> {
        let store = self.regions.get_mut(region)?;
        let removed = store.campaigns.remove(id)?;
        store.order.retain(|(_, stored)| stored != id);
        Some(removed)
    }

    /// The campaign of `region` whose ARN is `arn`, or `None` when that region stores
    /// no campaign carrying it.
    ///
    /// Resolved in the region the request was signed for, and then the whole ARN is
    /// compared, not just the id it ends with: an ARN names a partition, a region and
    /// an account as well as a resource, so one naming a different region or account
    /// names a different resource and resolves to nothing here.
    pub fn campaign_by_arn(&self, region: &str, arn: &str) -> Option<&StoredCampaign> {
        let id = campaign_id_from_arn(arn)?;
        self.campaign(region, id)
            .filter(|stored| stored.arn == arn)
    }

    /// The campaign whose ARN is `arn`, for a handler that writes to it. Resolved
    /// exactly as [`Self::campaign_by_arn`] resolves it.
    pub fn campaign_by_arn_mut(&mut self, region: &str, arn: &str) -> Option<&mut StoredCampaign> {
        let id = campaign_id_from_arn(arn)?.to_string();
        self.campaign_mut(region, &id)
            .filter(|stored| stored.arn == arn)
    }

    /// The campaign a tag operation's `arn` names in `region`, or the refusal that
    /// operation answers instead.
    ///
    /// The one place the resolve-or-refuse step of `/v2/tags/{arn}` lives, so every tag
    /// operation refuses an ARN it cannot resolve at the same point, with the same wire
    /// code, status and sentence: an ARN whose resource path is not one of this
    /// service's own is a bad request, decided before any existence check, and a
    /// campaign ARN this region stores nothing under is a not-found. Generic over the
    /// calling operation's modelled error enum, which the refusal does not name — both
    /// codes are the RECORDED wire codes rather than shape names (see
    /// [`TAG_BAD_REQUEST_CODE`], [`TAG_NOT_FOUND_CODE`] and
    /// [`crate::scaffolded::WireCoded`]).
    pub fn tagged_campaign<E>(
        &self,
        region: &str,
        arn: &str,
    ) -> Result<&StoredCampaign, crate::scaffolded::Scaffolded<E>> {
        if campaign_id_from_arn(arn).is_none() {
            return Err(tag_arn_bad_request());
        }
        self.campaign_by_arn(region, arn)
            .ok_or_else(tag_arn_not_found)
    }

    /// The campaign a tag operation's `arn` names, for a handler that writes to it, or
    /// the refusal it answers instead. Resolved exactly as [`Self::tagged_campaign`]
    /// resolves it.
    pub fn tagged_campaign_mut<E>(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Result<&mut StoredCampaign, crate::scaffolded::Scaffolded<E>> {
        if campaign_id_from_arn(arn).is_none() {
            return Err(tag_arn_bad_request());
        }
        self.campaign_by_arn_mut(region, arn)
            .ok_or_else(tag_arn_not_found)
    }

    /// The id the next outbound request this store takes will carry: a store-wide
    /// sequence number, so no two dial requests of any campaign share an id.
    ///
    /// Minted from the store's own counter, never from anything in the request. The
    /// text is within `DialRequestId`'s pattern `^[-_.a-zA-Z0-9]+$` and its 1..256
    /// length bound.
    fn next_dial_request_id(&mut self) -> String {
        let sequence = self.next_dial_sequence;
        self.next_dial_sequence = self.next_dial_sequence.saturating_add(1);
        format!("dial-{sequence:012}")
    }

    /// Take the outbound request submitted under `client_token` into the buffer of
    /// the campaign `campaign` names, and report what became of it (see
    /// [`OutboundRequestOutcome`]). `None` when that campaign's region stores no
    /// campaign under that id.
    ///
    /// A `client_token` the campaign has already taken stores nothing and reports
    /// the id it was given then, which is what makes a repeated batch idempotent.
    ///
    /// `expires_at` is weighed against `now` first: a request whose expiration has
    /// already passed can never be dialled, so it is not taken at all, mints no id
    /// and occupies no slot. The expiries the clock already owes the campaign's
    /// buffer are materialised at the same instant
    /// ([`StoredCampaign::expire_outbound_requests`]), so a slot and a `clientToken`
    /// are both held only while their request is still pending.
    ///
    /// A request that IS taken is stored as it was submitted:
    /// `channel_subtype_parameters` goes into the record whole (see
    /// [`StoredOutboundRequest`]), so the buffer says what each pending request is for
    /// and not merely that one is pending.
    pub fn take_outbound_request(
        &mut self,
        campaign: CampaignRef<'_>,
        client_token: &str,
        channel_subtype_parameters: &ChannelSubtypeParameters,
        expires_at: ::aws_smithy_types::DateTime,
        now: SystemTime,
    ) -> Option<OutboundRequestOutcome> {
        {
            let stored = self.campaign_mut(campaign.region, campaign.id)?;
            stored.expire_outbound_requests(now);
            if deadline_passed(expires_at, now) {
                return Some(OutboundRequestOutcome::Expired);
            }
            if let Some(taken) = stored.outbound_requests.get(client_token) {
                return Some(OutboundRequestOutcome::Taken(taken.id.clone()));
            }
            if stored.outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN {
                return Some(OutboundRequestOutcome::BufferFull);
            }
        }
        // Only now, past every refusal, is an id minted — so a refused request
        // consumes no id and leaves no gap in the sequence.
        let id = self.next_dial_request_id();
        let stored = self.campaign_mut(campaign.region, campaign.id)?;
        stored.outbound_requests.insert(
            client_token.to_string(),
            StoredOutboundRequest {
                id: id.clone(),
                channel_subtype_parameters: channel_subtype_parameters.clone(),
                expires_at,
            },
        );
        Some(OutboundRequestOutcome::Taken(id))
    }

    /// The id the next profile outbound request this store takes will carry: a
    /// store-wide sequence number, so no two profile requests of any campaign share an
    /// id, and none shares one with a dial request.
    ///
    /// Minted from the store's own counter, never from anything in the request. The
    /// text is within `ProfileOutboundRequestId`'s pattern `^[a-zA-Z0-9_\-.]*$` and its
    /// 0..256 length bound.
    fn next_profile_outbound_request_id(&mut self) -> String {
        let sequence = self.next_profile_request_sequence;
        self.next_profile_request_sequence = self.next_profile_request_sequence.saturating_add(1);
        format!("profile-{sequence:012}")
    }

    /// Take the profile outbound request submitted under `client_token` into the
    /// profile buffer of the campaign `campaign` names, and report what became of it
    /// (see [`OutboundRequestOutcome`]). `None` when that campaign's region stores no
    /// campaign under that id.
    ///
    /// Decided exactly as [`Self::take_outbound_request`] decides a dial request, on
    /// the buffer of profile requests: a `client_token` the campaign has already taken
    /// stores nothing and reports the id it was given then (what makes a repeated batch
    /// idempotent), an `expires_at` the clock has passed is not taken at all, mints no
    /// id and occupies no slot, and the expiries the clock already owes the buffer are
    /// materialised at the same instant
    /// ([`StoredCampaign::expire_profile_outbound_requests`]).
    ///
    /// `expires_at` is optional, as `ProfileOutboundRequest$expirationTime` is: a
    /// request that carries none has no instant to have passed, so it is always taken.
    ///
    /// A request that IS taken is stored as it was submitted: the `profile_id` it is to
    /// reach and the whole `event_trigger_context` it carried go into the record (see
    /// [`StoredProfileOutboundRequest`]).
    pub fn take_profile_outbound_request(
        &mut self,
        campaign: CampaignRef<'_>,
        client_token: &str,
        profile_id: &str,
        expires_at: Option<::aws_smithy_types::DateTime>,
        event_trigger_context: Option<&EventTriggerContext>,
        now: SystemTime,
    ) -> Option<OutboundRequestOutcome> {
        {
            let stored = self.campaign_mut(campaign.region, campaign.id)?;
            stored.expire_profile_outbound_requests(now);
            if expires_at.is_some_and(|expires_at| deadline_passed(expires_at, now)) {
                return Some(OutboundRequestOutcome::Expired);
            }
            if let Some(taken) = stored.profile_outbound_requests.get(client_token) {
                return Some(OutboundRequestOutcome::Taken(taken.id.clone()));
            }
            if stored.profile_outbound_requests.len() >= MAX_OUTBOUND_REQUESTS_PER_CAMPAIGN {
                return Some(OutboundRequestOutcome::BufferFull);
            }
        }
        // Only now, past every refusal, is an id minted — so a refused request
        // consumes no id and leaves no gap in the sequence.
        let id = self.next_profile_outbound_request_id();
        let stored = self.campaign_mut(campaign.region, campaign.id)?;
        stored.profile_outbound_requests.insert(
            client_token.to_string(),
            StoredProfileOutboundRequest {
                id: id.clone(),
                profile_id: profile_id.to_string(),
                expires_at,
                event_trigger_context: event_trigger_context.cloned(),
            },
        );
        Some(OutboundRequestOutcome::Taken(id))
    }

    /// The onboarding record `region` holds for `connect_instance_id` when that
    /// instance is onboarded to Campaigns there at `now` — a record exists in that
    /// region AND its job has reached `SUCCEEDED` — and `None` otherwise. An instance
    /// whose job is still `IN_PROGRESS` is not a campaign target yet, one whose job
    /// settled `FAILED` never became one, and neither is one onboarded only in another
    /// region.
    ///
    /// The one place "is this instance onboarded?" is decided, so `CreateCampaign`'s
    /// gate ([`Self::is_onboarded`]) and `GetConnectInstanceConfig`'s projection
    /// cannot disagree. A pure read of the stored due instant (see
    /// [`StoredInstanceOnboarding::status_at`]), so neither writes anything.
    pub fn onboarded_instance(
        &self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding(region, connect_instance_id)
            .filter(|record| record.status_at(now) == ONBOARDING_STATUS_SUCCEEDED)
    }

    /// Whether `region` has `connect_instance_id` onboarded to Campaigns at `now`, as
    /// [`Self::onboarded_instance`] decides it.
    pub fn is_onboarded(&self, region: &str, connect_instance_id: &str, now: SystemTime) -> bool {
        self.onboarded_instance(region, connect_instance_id, now)
            .is_some()
    }

    /// The record [`Self::onboarded_instance`] resolves, mutably — for a caller that
    /// configures the onboarded instance. The same predicate decides it, so an
    /// instance that is not a campaign target yet is not configurable either.
    fn onboarded_instance_mut(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> Option<&mut StoredInstanceOnboarding> {
        self.regions
            .get_mut(region)?
            .instance_onboarding
            .get_mut(connect_instance_id)
            .filter(|record| record.status_at(now) == ONBOARDING_STATUS_SUCCEEDED)
    }

    /// Replace the instance-wide communication limits `region` holds for
    /// `connect_instance_id` with `config`, and report whether the store took it.
    ///
    /// `false` means the instance is not onboarded in that region at `now`, so there
    /// is no instance configuration to put the limits on and nothing was written.
    /// Writing the whole config rather than merging into it is what makes the put
    /// idempotent: the same call twice leaves the same config stored.
    pub fn put_instance_communication_limits(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        config: InstanceCommunicationLimitsConfig,
        now: SystemTime,
    ) -> bool {
        match self.onboarded_instance_mut(region, connect_instance_id, now) {
            Some(record) => {
                record.communication_limits = Some(config);
                true
            }
            None => false,
        }
    }

    /// Every campaign `region` stores, in creation order, each with its creation
    /// sequence number — the value a `ListCampaigns` cursor is built from. A region
    /// that stores none yields nothing.
    pub fn campaigns_in_order<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = (u64, &'a StoredCampaign)> + 'a {
        self.region(region).into_iter().flat_map(|store| {
            store
                .order
                .iter()
                .filter_map(|(sequence, id)| store.campaigns.get(id).map(|c| (*sequence, c)))
        })
    }

    /// Start the onboarding job `region` holds for `connect_instance_id` with
    /// `encryption_config`, settling on `failure_code` (see
    /// [`StoredInstanceOnboarding::failure_code`]), and hand back the record now in
    /// the store.
    ///
    /// An instance with no record in that region gets a job that starts at `now` —
    /// including one already onboarded in another region, whose record is another
    /// region's resource. An instance that already has one there keeps that job —
    /// repeating the call does not re-onboard an instance, which is what makes the
    /// operation idempotent, and so does not re-run the job's dependency check either —
    /// and takes the request's encryption config, since the call carries the instance's
    /// config.
    ///
    /// `None` means the region already holds [`MAX_ONBOARDED_INSTANCES`] instances
    /// and nothing was written.
    pub fn start_instance_onboarding(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        encryption_config: EncryptionConfig,
        failure_code: Option<String>,
        now: std::time::SystemTime,
    ) -> Option<StoredInstanceOnboarding> {
        // Past the region ceiling there is no room for a region's first onboarding
        // record, which the caller answers exactly as it answers a full store.
        let store = self.region_mut(region)?;
        if let Some(existing) = store.instance_onboarding.get_mut(connect_instance_id) {
            existing.encryption_config = encryption_config;
            return Some(existing.clone());
        }
        if store.instance_onboarding.len() >= MAX_ONBOARDED_INSTANCES {
            return None;
        }
        let record = StoredInstanceOnboarding::starting(
            connect_instance_id.to_string(),
            encryption_config,
            failure_code,
            now,
        );
        store
            .instance_onboarding
            .insert(connect_instance_id.to_string(), record.clone());
        Some(record)
    }

    /// The onboarding record `region` holds for `connect_instance_id`, or `None` when
    /// the instance has never been onboarded there.
    pub fn instance_onboarding(
        &self,
        region: &str,
        connect_instance_id: &str,
    ) -> Option<&StoredInstanceOnboarding> {
        self.region(region)?
            .instance_onboarding
            .get(connect_instance_id)
    }

    /// Delete the onboarding job `region` holds for `connect_instance_id`, and report
    /// what became of the request (see [`DeleteOnboardingOutcome`]).
    ///
    /// Only a job that has FAILED may be deleted: a job in any other status keeps
    /// its record, so the status a later `GetInstanceOnboardingJobStatus` reports
    /// and the config `GetConnectInstanceConfig` projects are untouched by a refused
    /// delete. The status is read off the stored due instant at `now`, the same way
    /// every other reader of the record reads it.
    pub fn delete_instance_onboarding(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> DeleteOnboardingOutcome {
        let Some(record) = self.instance_onboarding(region, connect_instance_id) else {
            return DeleteOnboardingOutcome::NoSuchJob;
        };
        if record.status_at(now) != ONBOARDING_STATUS_FAILED {
            return DeleteOnboardingOutcome::NotFailed;
        }
        if let Some(store) = self.regions.get_mut(region) {
            store.instance_onboarding.remove(connect_instance_id);
        }
        DeleteOnboardingOutcome::Deleted
    }

    /// Remove every campaign `region` stores for `connect_instance_id`, in creation
    /// order, leaving every other instance's campaigns — and every other region's —
    /// where they are.
    ///
    /// Each record goes through [`Self::remove_campaign`], so the creation order
    /// loses exactly the entries the map does and a later page walks what is left.
    fn remove_instance_campaigns(&mut self, region: &str, connect_instance_id: &str) {
        let ids: Vec<String> = self
            .campaigns_in_order(region)
            .filter(|(_, campaign)| campaign.connect_instance_id == connect_instance_id)
            .map(|(_, campaign)| campaign.id.clone())
            .collect();
        for id in &ids {
            self.remove_campaign(region, id);
        }
    }

    /// Delete the instance configuration `region` holds for `connect_instance_id`,
    /// and report whether there was one to delete.
    ///
    /// `false` means the instance holds no configuration in that region at `now` — it
    /// was never onboarded there, its onboarding job has not settled or settled
    /// `FAILED`, or its configuration is already gone — so nothing was written, and the
    /// caller answers as not found. A failed job is cleared by
    /// `DeleteInstanceOnboardingJob`, the operation whose own resource that job is.
    ///
    /// Deleting the configuration takes with it everything held ON that
    /// configuration: the instance's onboarding record (so the instance is no longer
    /// a campaign target and no reader reports a config for it), the instance-wide
    /// communication limits that record carries, and the instance's integrations —
    /// which belong to the configured instance, so an instance onboarded again later
    /// starts out holding none.
    ///
    /// `policy` is what the request's `campaignDeletionPolicy` decided for the
    /// campaigns of the instance (see [`CampaignDeletionPolicy`]):
    /// [`CampaignDeletionPolicy::DeleteAll`] removes them with the configuration and
    /// [`CampaignDeletionPolicy::RetainAll`] retains them exactly as they are.
    ///
    /// An instance that still OWNS a campaign, where the request named NO policy for
    /// those campaigns, keeps its configuration:
    /// [`InstanceConfigDeletion::HeldByCampaigns`] is the whole answer and nothing is
    /// written, since a campaign depends on the configuration of the instance it
    /// belongs to and removing it would leave that campaign pointing at a
    /// configuration no reader can resolve. Once its last campaign is deleted, or the
    /// request says what is to become of them, the identical call goes through.
    pub fn delete_instance_config(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        policy: CampaignDeletionPolicy,
        now: SystemTime,
    ) -> InstanceConfigDeletion {
        if self
            .onboarded_instance(region, connect_instance_id, now)
            .is_none()
        {
            return InstanceConfigDeletion::NotConfigured;
        }
        if policy.blocked_by_campaigns()
            && self.instance_campaign_count(region, connect_instance_id) > 0
        {
            return InstanceConfigDeletion::HeldByCampaigns;
        }
        if policy.deletes_campaigns() {
            self.remove_instance_campaigns(region, connect_instance_id);
        }
        // The region holds the record the check above resolved, so it is in the map.
        let Some(store) = self.regions.get_mut(region) else {
            return InstanceConfigDeletion::NotConfigured;
        };
        store.instance_onboarding.remove(connect_instance_id);
        store.instance_integrations.remove(connect_instance_id);
        InstanceConfigDeletion::Deleted
    }

    /// How many campaigns `region` stores for `connect_instance_id`.
    ///
    /// The one place "does this instance still own campaigns?" is answered, read off
    /// the campaign store itself ([`Self::campaigns_in_order`]) rather than from a
    /// count kept beside it, so it cannot drift from the records a listing projects.
    pub fn instance_campaign_count(&self, region: &str, connect_instance_id: &str) -> usize {
        self.campaigns_in_order(region)
            .filter(|(_, campaign)| campaign.connect_instance_id == connect_instance_id)
            .count()
    }

    /// Put `config` as an integration `region` holds for `connect_instance_id`, and
    /// hand back the record now in the store — so a handler reports what the store
    /// holds rather than what the request carried.
    ///
    /// An integration already stored under the same identity (see
    /// [`integration_key`]) takes the new config and KEEPS its sequence number: that
    /// is what makes the call an update of that integration rather than a second one
    /// beside it. A config under an identity the instance does not hold is appended.
    ///
    /// `Err` means nothing was written, and which ceiling stopped it (see
    /// [`IntegrationPutRefused`]): a refusal consumes no sequence number and leaves no
    /// empty entry behind.
    pub fn put_instance_integration(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        config: IntegrationConfig,
    ) -> Result<StoredIntegration, IntegrationPutRefused> {
        let (kind, arn) = {
            let (kind, arn) = integration_key(&config);
            (kind, arn.to_string())
        };
        let sequence = self.next_integration_sequence;
        // Past the region ceiling there is no room for a region's first integration,
        // which the caller answers exactly as it answers a store whose key space is
        // full — in both the store, and not the request, is what stops the put.
        let store = self
            .region_mut(region)
            .ok_or(IntegrationPutRefused::StoreFull)?;
        let held = store
            .instance_integrations
            .get(connect_instance_id)
            .map_or(0, Vec::len);
        if let Some(existing) = store
            .instance_integrations
            .get_mut(connect_instance_id)
            .and_then(|integrations| {
                integrations
                    .iter_mut()
                    .find(|stored| stored.key() == (kind, arn.as_str()))
            })
        {
            existing.config = config;
            return Ok(existing.clone());
        }
        if held >= MAX_INTEGRATIONS_PER_INSTANCE {
            return Err(IntegrationPutRefused::InstanceFull);
        }
        // The KEY space is bounded as well as each instance's list, so the store cannot
        // grow through fresh instance ids: past the ceiling a put that would give a
        // further instance its first integration is refused and writes nothing, which
        // is why capacity is weighed BEFORE the entry is created (as `region_mut` does).
        if !store.instance_integrations.contains_key(connect_instance_id)
            && store.instance_integrations.len() >= MAX_INTEGRATION_INSTANCES
        {
            return Err(IntegrationPutRefused::StoreFull);
        }
        // Only now, past the refusal, is a sequence number taken — so the cursor
        // space has no gaps a refused put left behind.
        let record = StoredIntegration { sequence, config };
        store
            .instance_integrations
            .entry(connect_instance_id.to_string())
            .or_default()
            .push(record.clone());
        self.next_integration_sequence = sequence.saturating_add(1);
        Ok(record)
    }

    /// Every integration `region` holds for `connect_instance_id`, in the order they
    /// were first put.
    ///
    /// An instance that holds none yields nothing, which is not the same as an
    /// instance that is not onboarded: whether the instance exists at all is
    /// [`Self::is_onboarded`]'s answer, not this one's.
    pub fn instance_integrations<'a>(
        &'a self,
        region: &str,
        connect_instance_id: &str,
    ) -> impl Iterator<Item = &'a StoredIntegration> + 'a {
        self.region(region)
            .and_then(|store| store.instance_integrations.get(connect_instance_id))
            .into_iter()
            .flatten()
    }

    /// Remove the integration `region` holds for `connect_instance_id` that
    /// `identifier` names (see [`integration_identifier_key`]), and report whether one
    /// was removed.
    ///
    /// `false` means the instance holds no integration under that identity and nothing
    /// was written, which is what the caller answers as not found. The integrations the
    /// instance keeps hold the sequence numbers they were first put at, so removing one
    /// does not renumber the rest or reorder a page a caller is part way through.
    pub fn delete_instance_integration(
        &mut self,
        region: &str,
        connect_instance_id: &str,
        identifier: &IntegrationIdentifier,
    ) -> bool {
        let key = integration_identifier_key(identifier);
        let Some(integrations) = self
            .regions
            .get_mut(region)
            .and_then(|store| store.instance_integrations.get_mut(connect_instance_id))
        else {
            return false;
        };
        let Some(at) = integrations.iter().position(|stored| stored.key() == key) else {
            return false;
        };
        integrations.remove(at);
        true
    }
}

/// What [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::delete_instance_config`]
/// did with a delete request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceConfigDeletion {
    /// The configuration, and everything held on it, is gone.
    Deleted,
    /// The region holds no configuration for the instance at the instant the call was
    /// judged, so there was none to delete. The caller answers this with the modelled
    /// `ResourceNotFoundException` (HTTP 404) carrying
    /// [`INSTANCE_NOT_ONBOARDED_MESSAGE`], the sentence that operation's own wire states
    /// for an instance it holds no finished onboarding for.
    NotConfigured,
    /// The instance still owns at least one campaign and the request asked for no
    /// campaign deletion, so the configuration those campaigns depend on is left in
    /// place. The caller answers this with the modelled `InvalidStateException`
    /// (HTTP 409) carrying [`INSTANCE_HAS_CAMPAIGNS_MESSAGE`].
    HeldByCampaigns,
}

/// What [`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::delete_instance_onboarding`]
/// did with a delete request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOnboardingOutcome {
    /// The record was FAILED and is gone.
    Deleted,
    /// The instance has no onboarding record at all.
    NoSuchJob,
    /// The record is in a status other than FAILED and was left untouched.
    NotFailed,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>)
}
