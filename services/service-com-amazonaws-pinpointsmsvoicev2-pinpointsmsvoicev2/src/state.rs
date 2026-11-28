// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2 service.
//!
//! The origination-identity + send-text slice of AWS End User Messaging SMS
//! (`sms-voice`) is persisted here: configuration sets, requested phone numbers, protect
//! configurations and the messages that were actually sent. Every implemented handler
//! computes its whole response from these records, so a create is observable through the
//! matching describe/release and a release is observable as an absence.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{EventDestination, Tag, ValidationExceptionField};

/// This service's SigV4 signing name, which is also its ARN namespace.
///
/// From the model's `awsService` identity (`OPERATIONS.md` "Wire protocol": sigv4
/// signing name / ARN namespace `sms-voice`).
pub const SIGNING_NAME: &str = "sms-voice";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The opt-out list a phone number is associated with when the request names none.
///
/// Must satisfy `OptOutListName` (len 1..64, `^[A-Za-z0-9_-]+$`), which
/// `PhoneNumberInformation$OptOutListName` declares `@required`.
pub const DEFAULT_OPT_OUT_LIST_NAME: &str = "Default";

/// The lease price reported for every minted number. Real per-country, per-type
/// pricing is not emulatable, so one chosen value is reported for all of them; it is
/// a US-dollar amount, the unit `MonthlyLeasingPrice` documents.
pub const MONTHLY_LEASING_PRICE: &str = "0.00";

/// The `NumberStatus` a number this emulator leases is in: provisioning is synchronous
/// here, so the lease is handed back in the terminal state and no later transition is owed.
pub const ACTIVE_NUMBER_STATUS: &str = "ACTIVE";

/// The attributes of the origination identity this service leases for an account that
/// holds none of its own, so a send that names no `OriginationIdentity` has one to resolve
/// ([`ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State::default_origination_identity`]).
///
/// Each value is one its own modelled enum declares: `IsoCountryCode` (`^[A-Z]{2}$`),
/// `MessageType`, `NumberCapability` and `RequestableNumberType`
/// (`smithy_query(query="shape",shape="MessageType,NumberCapability,RequestableNumberType")`).
/// Which of those values it is is this emulator's choice, as the synthetic number itself is.
pub const DEFAULT_ORIGINATION_ISO_COUNTRY_CODE: &str = "US";
/// See [`DEFAULT_ORIGINATION_ISO_COUNTRY_CODE`].
pub const DEFAULT_ORIGINATION_MESSAGE_TYPE: &str = "TRANSACTIONAL";
/// See [`DEFAULT_ORIGINATION_ISO_COUNTRY_CODE`]. A text message is sent over the number's
/// SMS capability, so that is the capability the identity carries.
pub const DEFAULT_ORIGINATION_NUMBER_CAPABILITY: &str = "SMS";
/// See [`DEFAULT_ORIGINATION_ISO_COUNTRY_CODE`].
pub const DEFAULT_ORIGINATION_NUMBER_TYPE: &str = "LONG_CODE";

/// The `ClientToken` idempotency key is namespaced by operation, so the same token on
/// two different creates is two different requests. These are the operations that mint a
/// resource under a token, named here because a delete/release must also forget the
/// token that created the resource it removes.
pub const CREATE_CONFIGURATION_SET: &str = "CreateConfigurationSet";
/// See [`CREATE_CONFIGURATION_SET`].
pub const REQUEST_PHONE_NUMBER: &str = "RequestPhoneNumber";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_EVENT_DESTINATION: &str = "CreateEventDestination";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_PROTECT_CONFIGURATION: &str = "CreateProtectConfiguration";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_OPT_OUT_LIST: &str = "CreateOptOutList";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_RCS_AGENT: &str = "CreateRcsAgent";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_REGISTRATION: &str = "CreateRegistration";
/// See [`CREATE_CONFIGURATION_SET`].
pub const CREATE_REGISTRATION_ATTACHMENT: &str = "CreateRegistrationAttachment";

/// What a `ClientToken` for an event-destination create points at: a destination is named
/// only within one configuration set, so the set's name is part of the key. Neither name
/// can hold a `/` (both are `^[A-Za-z0-9_-]+$`), so the two parts cannot run together.
pub fn event_destination_resource(
    configuration_set_name: &str,
    event_destination_name: &str,
) -> String {
    format!("{configuration_set_name}/{event_destination_name}")
}

// ---------------------------------------------------------------------------------
// Bounds. Every store below grows on a caller-driven path, so each one has a maximum:
// without one, a loop of requests grows the emulator's heap until the host is out of
// memory and only a restart releases it. The two resource maps are bounded by an
// account quota, refused with the modelled `ServiceQuotaExceededException`; the two
// bookkeeping stores are bounded by eviction of their oldest entry, because a caller
// cannot delete from them.
// ---------------------------------------------------------------------------------

/// The most configuration sets one account may hold. Reaching it is answered with
/// `ServiceQuotaExceededException` / `reason = CONFIGURATION_SETS_PER_ACCOUNT`, the
/// quota the model names for this resource. The number itself is this emulator's
/// choice: AWS publishes an adjustable per-account quota, not a fixed one.
pub const MAX_CONFIGURATION_SETS_PER_ACCOUNT: usize = 1_000;

/// The most protect configurations one account may hold. Reaching it is answered with
/// `ServiceQuotaExceededException` / `reason = PROTECT_CONFIGURATIONS_PER_ACCOUNT`, the
/// quota the model names for this resource. Chosen for the same reason as
/// [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_PROTECT_CONFIGURATIONS_PER_ACCOUNT: usize = 1_000;

/// The most phone numbers one account may lease, answered with
/// `ServiceQuotaExceededException` / `reason = PHONE_NUMBERS_PER_ACCOUNT`. Chosen for
/// the same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_PHONE_NUMBERS_PER_ACCOUNT: usize = 1_000;

/// The most opt-out lists one account may hold, answered with
/// `ServiceQuotaExceededException` / `reason = OPT_OUT_LISTS_PER_ACCOUNT`. Chosen for the
/// same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_OPT_OUT_LISTS_PER_ACCOUNT: usize = 1_000;

/// The most RCS agents one account may hold, answered with
/// `ServiceQuotaExceededException` / `reason = RCS_AGENTS_PER_ACCOUNT`. Chosen for the
/// same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_RCS_AGENTS_PER_ACCOUNT: usize = 1_000;

/// The most registrations one account may hold, answered with
/// `ServiceQuotaExceededException` / `reason = REGISTRATIONS_PER_ACCOUNT`. Chosen for the
/// same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_REGISTRATIONS_PER_ACCOUNT: usize = 1_000;

/// The most registration attachments one account may hold, answered with
/// `ServiceQuotaExceededException` / `reason = REGISTRATION_ATTACHMENTS_PER_ACCOUNT`.
/// Chosen for the same reason as [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`].
pub const MAX_REGISTRATION_ATTACHMENTS_PER_ACCOUNT: usize = 1_000;

/// The most resource-based policies one account may hold at once. One policy is attached
/// per resource, so the store cannot outgrow the resources it points at by more than this;
/// past it `PutResourcePolicy` is refused, so a loop of puts cannot grow the emulator's
/// heap without limit. `PutResourcePolicy` models no `ServiceQuotaExceededException`, so
/// the refusal is its `ValidationException`.
pub const MAX_RESOURCE_POLICIES_PER_ACCOUNT: usize = 1_000;

/// The most event destinations one configuration set may hold, answered with
/// `ServiceQuotaExceededException` / `reason = EVENT_DESTINATIONS_PER_CONFIGURATION_SET`.
///
/// Unlike the two quotas above this one is AWS's published, fixed bound, which
/// `CreateEventDestination` documents: "Each configuration set can contain between 0 and 5
/// event destinations."
pub const MAX_EVENT_DESTINATIONS_PER_CONFIGURATION_SET: usize = 5;

/// How many of the most recent sends the message log retains. The log is a diagnostic
/// trail with no operation that reads or trims it, and each record holds caller-sized
/// text, so it is a ring: the oldest record is dropped to make room for a new one.
pub const MAX_RETAINED_MESSAGES: usize = 1_000;

/// How many `ClientToken` entries are retained. An entry is dropped when the resource
/// it created is deleted or released; past this many live entries the oldest is dropped,
/// which can only make a replayed create act as a first create.
pub const MAX_RETAINED_CLIENT_TOKENS: usize = 1_000;

/// The region to stamp into ARNs for this request: the SigV4 credential scope's
/// region, or [`DEFAULT_REGION`] when the request carries no scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A wall-clock instant as the modelled `Timestamp` type.
///
/// Truncated to whole seconds: every `Timestamp` this service reports is documented as
/// a time "in UNIX epoch time format" (e.g.
/// `DeleteConfigurationSetResult$CreatedTimestamp`, `PhoneNumberInformation$CreatedTimestamp`),
/// which is the granularity the records are stamped at, so no member carries a
/// sub-second remainder of the host clock. Truncation is applied here, on the one
/// helper every stamped member goes through, so the whole service reports one
/// granularity; the stored instant is unchanged, so repeated reads of one record keep
/// answering the same value.
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
    )
}

/// A page cursor: the offset into a paginated read's ordered result set at which the next
/// page starts, behind the prefix the operation that issues it names.
///
/// `NextToken` is modelled only as `len 1..1024, pattern ^.+$`, so the shape of the cursor
/// is this service's own. Every paginated read mints and parses its cursor through
/// [`page_cursor`] / [`page_start`], so the two cannot drift apart and a third one cannot
/// grow a third encoding.
pub fn page_cursor(prefix: &str, offset: usize) -> String {
    format!("{prefix}{offset:08x}")
}

/// The `ValidationExceptionReason` a refused cursor carries.
pub const INVALID_NEXT_TOKEN_REASON: &str = "INVALID_NEXT_TOKEN";

/// The input member a refused cursor names in `ValidationException$Fields`.
pub const NEXT_TOKEN_FIELD: &str = "NextToken";

/// Why a `NextToken` was refused: the wording a paginated read lifts into its own modelled
/// `ValidationException`, as the exception's `Message` and as its field entry's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextTokenRejection {
    /// The exception's `Message`.
    pub message: &'static str,
    /// The `ValidationExceptionField$Message` for `NextToken`.
    pub field_message: &'static str,
}

/// The refusal of a token this service did not issue.
pub const NEXT_TOKEN_NOT_ISSUED: NextTokenRejection = NextTokenRejection {
    message: "NextToken is not a cursor this service issued",
    field_message: "not a cursor this service issued",
};

/// The refusal of a cursor whose offset is past the end of the result set it is replayed
/// against.
pub const NEXT_TOKEN_PAST_END: NextTokenRejection = NextTokenRejection {
    message: "NextToken points past the end of the result set",
    field_message: "points past the end of the result set",
};

/// The offset one page starts at: `0` when the request carries no cursor, the offset a
/// cursor this operation issued encodes, or a rejection to refuse.
///
/// The prefix is what makes a token self-identifying, so a token a client made up is
/// refused whatever offset it encodes rather than silently driving paging.
pub fn page_start(
    prefix: &str,
    token: Option<&str>,
    total: usize,
) -> Result<usize, NextTokenRejection> {
    let Some(token) = token else {
        return Ok(0);
    };
    let start = token
        .strip_prefix(prefix)
        .and_then(|offset| usize::from_str_radix(offset, 16).ok())
        .ok_or(NEXT_TOKEN_NOT_ISSUED)?;
    if start > total {
        return Err(NEXT_TOKEN_PAST_END);
    }
    Ok(start)
}

/// Where one page ends: `MaxResults` records from `start`, or the whole remainder when the
/// request names no maximum.
///
/// A `MaxResults` that is not a usable page size — zero, or a value no page size can hold,
/// both of which the modelled `range 1..100` already refuses before a handler runs — is read
/// as "no maximum" rather than as an empty page, so no cursor can be minted for a page that
/// holds nothing and paging always advances.
pub fn page_end(start: usize, max_results: Option<i32>, total: usize) -> usize {
    let page_size = max_results
        .filter(|max| *max > 0)
        .map_or(total, |max| usize::try_from(max).unwrap_or(total));
    start.saturating_add(page_size).min(total)
}

/// The resource name carried by a `<Thing>NameOrArn` / `<Thing>IdOrArn` member: the
/// last `/`-separated segment of an ARN, or the value itself when it is a bare name.
pub fn name_from_name_or_arn(value: &str) -> &str {
    if value.starts_with("arn:") {
        value.split('/').next_back().unwrap_or(value)
    } else {
        value
    }
}

/// The message a `ConflictException` carries: this service words it as a restatement of
/// the exception's own structured members, so a caller reading only `Message` is told the
/// same `Reason`, `ResourceType` and `ResourceId` the body carries.
///
/// `reason` is a `ConflictExceptionReason`, `resource_type` a `ResourceType`.
pub fn conflict_message(reason: &str, resource_type: &str, resource_id: &str) -> String {
    format!(
        "Conflict Occurred - Reason=\"{reason}\" ResourceType=\"{resource_type}\" \
         ResourceId=\"{resource_id}\""
    )
}

/// The message the refusal of a replayed `ClientToken` carries. A token already used for
/// a DIFFERENT request names no resource, so the conflict is reported with this message
/// alone and none of `ConflictException`'s resource members.
pub const CLIENT_TOKEN_CONFLICT_MESSAGE: &str = "Token used for different request";

/// The `ProtectStatus` every country of every capability carries in a protect
/// configuration no caller has changed.
///
/// `CreateProtectConfiguration` documents it: "By default all country rule sets for each
/// capability are set to ALLOW."
pub const DEFAULT_PROTECT_STATUS: &str = "ALLOW";

/// The destination countries a protect configuration's country rule set covers: the
/// published ISO 3166-1 alpha-2 codes, from the `rust_iso3166` crate's table rather than
/// a list restated here.
///
/// `ProtectConfigurationCountryRuleSet` is keyed by `IsoCountryCode`
/// (`smithy_query(query="shape",shape="ProtectConfigurationCountryRuleSet")`: `len 2..2`,
/// `^[A-Z]{2}$`) and holds at most 300 entries, which the published table fits; the
/// documentation for the map calls the key "the two-letter ISO country code".
pub fn country_rule_set_countries() -> &'static [&'static str] {
    ISO_COUNTRY_CODES
}

/// See [`country_rule_set_countries`]. A `static` so the slice is borrowed from the
/// crate's own table rather than from a temporary.
static ISO_COUNTRY_CODES: &[&str] = ::rust_iso3166::ALL_ALPHA2;

/// Whether `country` is one of the two-letter country codes
/// [`country_rule_set_countries`] lists.
///
/// The codes are published as two UPPERCASE letters, so the lookup is case-sensitive: a
/// pair of letters that is not an assigned code names no country, whatever the modelled
/// `^[A-Z]{2}$` pattern allows.
pub fn is_country_rule_set_country(country: &str) -> bool {
    country.chars().all(|letter| letter.is_ascii_uppercase())
        && ::rust_iso3166::from_alpha2(country).is_some()
}

/// The `ValidationExceptionReason` a parameter this service cannot read the value of
/// earns.
pub const INVALID_PARAMETER_REASON: &str = "INVALID_PARAMETER";

/// The `ValidationExceptionReason` a member whose value this service validated and
/// rejected earns, whether the rule it broke is one the model declares (the boundary
/// walk in `crate::service::validation`) or one only a handler can check.
pub const FIELD_VALIDATION_FAILED_REASON: &str = "FIELD_VALIDATION_FAILED";

/// The sentence this service refuses a request's parameters with: fixed text up to the
/// colon, then the bracketed, single-quoted WIRE names of the members it rejected, in the
/// order they are given.
///
/// Stated once for the whole service, so the boundary refusal and a handler's own refusal
/// of a value no modelled constraint can describe word the same thing. No submitted value
/// is echoed: the sentence names the member, never its contents.
pub fn invalid_parameters_message(wire_names: &[&str]) -> String {
    let named: Vec<String> = wire_names.iter().map(|name| format!("'{name}'")).collect();
    format!(
        "The following parameter is null, empty or invalid: [{}]",
        named.join(", ")
    )
}

/// The message a `ValidationException` carries when this service could not READ a
/// parameter's value: like the conflict and not-found messages, a restatement of the
/// exception's own structured members, here its `Reason` and the members its `Fields`
/// name.
///
/// Distinct from [`invalid_parameters_message`], which states a value this service read
/// and then could not use. `reason` is a `ValidationExceptionReason`.
pub fn validation_error_message(reason: &str, fields: &str) -> String {
    format!("Validation Error Occurred - Reason=\"{reason}\" Fields=\"{fields}\"")
}

/// The request members that each name one KIND of event destination, in the order the
/// event-destination operations document them.
///
/// Stated once for the whole service: an event destination publishes to one place, so the
/// create needs exactly one of these members and the update at most one, and both have to
/// name the same set — two copies of it drift the moment the model declares a fourth kind.
pub const EVENT_DESTINATION_MEMBERS: [&str; 3] = [
    "CloudWatchLogsDestination",
    "KinesisFirehoseDestination",
    "SnsDestination",
];

/// Which of [`EVENT_DESTINATION_MEMBERS`] are present, in that order, from one presence
/// flag per member — so every reader of "which kind of destination is this" asks it here
/// rather than spelling the three members a second time.
pub fn named_event_destination_members(present: [bool; 3]) -> Vec<&'static str> {
    EVENT_DESTINATION_MEMBERS
        .iter()
        .zip(present)
        .filter_map(|(member, given)| given.then_some(*member))
        .collect()
}

/// The `Fields` entry each member of `named` earns on a refusal of the event-destination
/// member set: every one of them failed the same requirement, which is that exactly one
/// kind of destination is named.
pub fn event_destination_fields(named: &[&str]) -> Vec<ValidationExceptionField> {
    named
        .iter()
        .map(|member| ValidationExceptionField {
            name: (*member).to_string(),
            message: ONE_EVENT_DESTINATION_REQUIREMENT.to_string(),
        })
        .collect()
}

/// The requirement every `Fields` entry of such a refusal states.
const ONE_EVENT_DESTINATION_REQUIREMENT: &str = "exactly one event destination is required";

/// The message a request naming SEVERAL event destinations is refused with: it does not say
/// where events go. Worded once, so the create and the update state it the same way.
pub fn too_many_event_destinations_message(named: &[&str]) -> String {
    format!(
        "only one event destination can be provided, but {} were provided",
        named.join(", ")
    )
}

/// The message a request that must name one event destination and names NONE is refused
/// with: it has nothing to log to.
pub fn no_event_destination_message() -> String {
    format!(
        "a value must be provided for a single event destination using one of {}",
        EVENT_DESTINATION_MEMBERS.join(", ")
    )
}

/// The message a `ResourceNotFoundException` carries: like the conflict message, this
/// service words it as a restatement of the exception's own structured members.
///
/// `resource_type` is a `ResourceType`.
pub fn resource_not_found_message(resource_type: &str, resource_id: &str) -> String {
    format!("Resource Not Found - ResourceType=\"{resource_type}\" ResourceId=\"{resource_id}\"")
}

/// A `MessageType` as a configuration-set record holds it: initial capital, the rest
/// lower (`Transactional`), which is the capitalization every read of a configuration
/// set's stored default message type reports, whichever case the request that set it
/// used. `SetDefaultMessageType`'s own response echoes the request's spelling, so the
/// canonical form is applied where the record is WRITTEN, not where it is read.
pub fn canonical_message_type(message_type: &str) -> String {
    let mut characters = message_type.chars();
    match characters.next() {
        Some(first) => format!(
            "{}{}",
            first.to_uppercase(),
            characters.as_str().to_lowercase()
        ),
        None => String::new(),
    }
}

/// A stored configuration set.
#[derive(Debug, Clone)]
pub struct ConfigurationSet {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
    pub default_message_type: Option<String>,
    pub default_sender_id: Option<String>,
    pub default_message_feedback_enabled: bool,
    pub event_destinations: Vec<EventDestination>,
    /// The `ProtectConfigurationId` of the protect configuration associated with this
    /// set, while one is associated, and `None` while none is — which is why every
    /// read of a set reports the member only some of the time.
    ///
    /// One id rather than a list: `AssociateProtectConfiguration` documents that a
    /// configuration set can only be associated with one protect configuration at a
    /// time and that associating replaces the set's current one, so the link is a single
    /// optional id held on the set that owns it. The id is a key into
    /// [`Self::protect_configurations`](ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State::protect_configurations),
    /// so the associated configuration's ARN is read from that record rather than stored
    /// twice.
    pub protect_configuration_id: Option<String>,
}

impl ConfigurationSet {
    /// The stored event destination this set holds under `name`, if it holds one. A
    /// destination's name identifies it within its set, so at most one record matches.
    pub fn event_destination(&self, name: &str) -> Option<&EventDestination> {
        self.event_destinations
            .iter()
            .find(|destination| destination.event_destination_name == name)
    }

    /// The record's value for one `ConfigurationSetFilterName`, or `None` when the record
    /// carries no value for that attribute (so no filter value can match it).
    ///
    /// The names are exactly the model's `ConfigurationSetFilterName` enum, re-exported as
    /// [`crate::ops::types::CONFIGURATION_SET_FILTER_NAME_VALUES`];
    /// `event-destination-name` and `matching-event-types` are multi-valued, because one
    /// set holds a list of event destinations and each of those a list of event types.
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        match name {
            "event-destination-name" => Some(
                self.event_destinations
                    .iter()
                    .map(|destination| destination.event_destination_name.clone())
                    .collect(),
            ),
            "matching-event-types" => Some(
                self.event_destinations
                    .iter()
                    .flat_map(|destination| destination.matching_event_types.iter().cloned())
                    .collect(),
            ),
            // The record holds the canonical capitalization of the message type
            // ([`canonical_message_type`]), while a request filters with the spelling the
            // model's `MessageType` enum declares (`TRANSACTIONAL`), so both forms of the
            // ONE stored value are offered and either selects this set.
            "default-message-type" => self.default_message_type.as_ref().map(|value| {
                let declared = value.to_uppercase();
                if declared == *value {
                    vec![declared]
                } else {
                    vec![value.clone(), declared]
                }
            }),
            "default-sender-id" => self.default_sender_id.clone().map(|value| vec![value]),
            "default-message-feedback-enabled" => {
                Some(vec![self.default_message_feedback_enabled.to_string()])
            }
            // The association `AssociateProtectConfiguration` recorded and
            // `DisassociateProtectConfiguration` clears. A set with no protect
            // configuration carries no value for this attribute, so no filter value
            // selects it.
            "protect-configuration-id" => {
                self.protect_configuration_id.clone().map(|id| vec![id])
            }
            _ => None,
        }
    }
}

/// A stored origination phone number.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    pub id: String,
    pub arn: String,
    /// E.164, as `PhoneNumber` (`^\+?[1-9][0-9]{1,18}$`) requires.
    pub phone_number: String,
    pub status: String,
    pub iso_country_code: String,
    pub message_type: String,
    pub number_capabilities: Vec<String>,
    pub number_type: String,
    pub monthly_leasing_price: String,
    pub two_way_enabled: bool,
    pub two_way_channel_arn: Option<String>,
    pub two_way_channel_role: Option<String>,
    pub self_managed_opt_outs_enabled: bool,
    pub opt_out_list_name: String,
    pub international_sending_enabled: bool,
    pub deletion_protection_enabled: bool,
    pub pool_id: Option<String>,
    pub registration_id: Option<String>,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
}

impl PhoneNumberRecord {
    /// The record's value for one `PhoneNumberFilterName`, or `None` when the record
    /// carries no value for that attribute (so no filter value can match it).
    ///
    /// The names are exactly the model's `PhoneNumberFilterName` enum, re-exported as
    /// [`crate::ops::types::PHONE_NUMBER_FILTER_NAME_VALUES`]; `number-capability` is
    /// multi-valued, which is why this answers a list.
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        let single = |value: String| Some(vec![value]);
        match name {
            "status" => single(self.status.clone()),
            "iso-country-code" => single(self.iso_country_code.clone()),
            "message-type" => single(self.message_type.clone()),
            "number-capability" => Some(self.number_capabilities.clone()),
            "number-type" => single(self.number_type.clone()),
            "two-way-enabled" => single(self.two_way_enabled.to_string()),
            "self-managed-opt-outs-enabled" => single(self.self_managed_opt_outs_enabled.to_string()),
            "opt-out-list-name" => single(self.opt_out_list_name.clone()),
            "deletion-protection-enabled" => single(self.deletion_protection_enabled.to_string()),
            "two-way-channel-arn" => self.two_way_channel_arn.clone().map(|arn| vec![arn]),
            _ => None,
        }
    }
}

/// A stored protect configuration.
///
/// The resource carries no name of its own: a caller labels it with a `Name` tag, so the
/// only identity a record has is its minted `ProtectConfigurationId` and the ARN built
/// from it.
#[derive(Debug, Clone)]
pub struct ProtectConfiguration {
    pub id: String,
    pub arn: String,
    pub created_timestamp: SystemTime,
    /// True while this configuration is the account default one.
    pub account_default: bool,
    pub deletion_protection_enabled: bool,
    pub tags: Vec<Tag>,
    /// The `ProtectStatus` this configuration holds per destination country, keyed by
    /// `NumberCapability` and then by ISO country code — and only for the countries a
    /// caller has actually set, every other country carrying
    /// [`DEFAULT_PROTECT_STATUS`]. The rule set is per capability, so a status written
    /// for one capability leaves the others as they were.
    ///
    /// Bounded by the model: at most one entry per `NumberCapability` value, each holding
    /// at most one entry per country in [`country_rule_set_countries`], since a status is
    /// written only for a country code this service recognizes.
    pub country_rule_sets: BTreeMap<String, BTreeMap<String, String>>,
}

impl ProtectConfiguration {
    /// The record's value for one `ProtectConfigurationFilterName`, or `None` when the
    /// record carries no value for that attribute (so no filter value can match it).
    ///
    /// The names are exactly the model's `ProtectConfigurationFilterName` enum,
    /// re-exported as
    /// [`crate::ops::types::PROTECT_CONFIGURATION_FILTER_NAME_VALUES`]; both are
    /// single-valued booleans, which a request filters for as `"true"` / `"false"`.
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        match name {
            "account-default" => Some(vec![self.account_default.to_string()]),
            "deletion-protection-enabled" => {
                Some(vec![self.deletion_protection_enabled.to_string()])
            }
            _ => None,
        }
    }

    /// The whole `ProtectConfigurationCountryRuleSet` for one `NumberCapability`: every
    /// country this service recognizes, with the status this record holds for it under
    /// that capability and [`DEFAULT_PROTECT_STATUS`] for the ones it holds none for.
    ///
    /// A `BTreeMap`, so one rule set is reported in a stable country order.
    pub fn country_rule_set(&self, number_capability: &str) -> BTreeMap<String, String> {
        let written = self.country_rule_sets.get(number_capability);
        country_rule_set_countries()
            .iter()
            .map(|country| {
                let status = written
                    .and_then(|statuses| statuses.get(*country))
                    .map_or(DEFAULT_PROTECT_STATUS, String::as_str);
                ((*country).to_string(), status.to_string())
            })
            .collect()
    }

    /// Hold `protect_status` for one country under one `NumberCapability`, replacing the
    /// status that capability held for that country.
    pub fn set_country_protect_status(
        &mut self,
        number_capability: &str,
        country: &str,
        protect_status: &str,
    ) {
        self.country_rule_sets
            .entry(number_capability.to_string())
            .or_default()
            .insert(country.to_string(), protect_status.to_string());
    }
}

/// A stored opt-out list.
///
/// The caller NAMES it — `CreateOptOutList` takes the `OptOutListName` and every later
/// call addresses the list by that name or by the ARN built around it — so the name is the
/// record's whole identity.
#[derive(Debug, Clone)]
pub struct OptOutList {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
}

/// A stored RCS agent.
///
/// The resource carries no caller-supplied name: `CreateRcsAgent` takes the agent's
/// configuration and answers the `RcsAgentId` this service minted with the ARN built
/// around it, which is what every later call addresses one by.
#[derive(Debug, Clone)]
pub struct RcsAgent {
    pub id: String,
    pub arn: String,
    /// An `RcsAgentStatus`. A created agent is `CREATED`: it leaves that status only when
    /// the caller submits the registration that launches it, which is a caller's action
    /// rather than something the clock owes.
    pub status: String,
    pub created_timestamp: SystemTime,
    pub deletion_protection_enabled: bool,
    pub opt_out_list_name: Option<String>,
    pub self_managed_opt_outs_enabled: bool,
    pub two_way_enabled: bool,
    pub two_way_channel_arn: Option<String>,
    pub two_way_channel_role: Option<String>,
    pub two_way_rcs_events_enabled: Vec<String>,
    pub tags: Vec<Tag>,
}

impl RcsAgent {
    /// The record's value for one `RcsAgentFilterName`, or `None` when the record carries
    /// no value for that attribute (so no filter value can match it).
    ///
    /// The names are exactly the model's `RcsAgentFilterName` enum, re-exported as
    /// [`crate::ops::types::RCS_AGENT_FILTER_NAME_VALUES`].
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        let single = |value: String| Some(vec![value]);
        match name {
            "status" => single(self.status.clone()),
            "two-way-enabled" => single(self.two_way_enabled.to_string()),
            "self-managed-opt-outs-enabled" => {
                single(self.self_managed_opt_outs_enabled.to_string())
            }
            "deletion-protection-enabled" => single(self.deletion_protection_enabled.to_string()),
            "opt-out-list-name" => self.opt_out_list_name.clone().map(|value| vec![value]),
            "two-way-channel-arn" => self.two_way_channel_arn.clone().map(|arn| vec![arn]),
            _ => None,
        }
    }
}

/// A stored registration.
///
/// `CreateRegistration` takes only the registration's TYPE and answers the
/// `RegistrationId` this service minted with the ARN built around it; the form's field
/// values are written afterwards, one call at a time.
#[derive(Debug, Clone)]
pub struct Registration {
    pub id: String,
    pub arn: String,
    pub registration_type: String,
    /// A `RegistrationStatus`. A created registration is `CREATED` — "Your registration is
    /// created but not submitted" — and leaves that status only when the caller submits a
    /// version of it, which is a caller's action rather than something the clock owes.
    pub status: String,
    /// The version the form's values are being written into, which a created registration
    /// starts at 1: `CreateRegistration` answers a `CurrentVersionNumber` and the member's
    /// own `range 1..100000` has no zero.
    pub current_version_number: i64,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
}

impl Registration {
    /// The record's value for one `RegistrationFilterName`, or `None` when the record
    /// carries no value for that attribute.
    ///
    /// The names are exactly the model's `RegistrationFilterName` enum, re-exported as
    /// [`crate::ops::types::REGISTRATION_FILTER_NAME_VALUES`].
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        match name {
            "registration-type" => Some(vec![self.registration_type.clone()]),
            "registration-status" => Some(vec![self.status.clone()]),
            _ => None,
        }
    }
}

/// How long after the create a registration attachment's upload is reported as finished.
///
/// `CreateRegistrationAttachment` answers `UPLOAD_IN_PROGRESS` — the attachment is being
/// uploaded — and the attachment reaches `UPLOAD_COMPLETE` without anyone asking for it, so
/// the instant that transition falls due is stored on the record and a read materialises it
/// ([`ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State::settle_attachment_uploads`]).
/// Nothing sleeps for it: the span is this emulator's own and is short enough that a caller
/// polling the attachment sees both statuses.
pub const ATTACHMENT_UPLOAD_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The `AttachmentStatus` a created attachment is in, and the one its upload finishes in.
pub const ATTACHMENT_UPLOAD_IN_PROGRESS: &str = "UPLOAD_IN_PROGRESS";
/// See [`ATTACHMENT_UPLOAD_IN_PROGRESS`].
pub const ATTACHMENT_UPLOAD_COMPLETE: &str = "UPLOAD_COMPLETE";

/// A stored registration attachment: the supporting document a registration form refers to.
#[derive(Debug, Clone)]
pub struct RegistrationAttachment {
    pub id: String,
    pub arn: String,
    /// An `AttachmentStatus`, which starts at [`ATTACHMENT_UPLOAD_IN_PROGRESS`].
    pub status: String,
    /// The URL the document was supplied from, when the create named one rather than
    /// carrying the body inline.
    pub attachment_url: Option<String>,
    /// When [`ATTACHMENT_UPLOAD_IN_PROGRESS`] becomes [`ATTACHMENT_UPLOAD_COMPLETE`]:
    /// [`ATTACHMENT_UPLOAD_SPAN`] after the create.
    pub upload_complete_at: SystemTime,
    pub tags: Vec<Tag>,
    pub created_timestamp: SystemTime,
}

impl RegistrationAttachment {
    /// The record's value for one `RegistrationAttachmentFilterName`, or `None` when the
    /// record carries no value for that attribute.
    ///
    /// The one name is the model's `RegistrationAttachmentFilterName` enum, re-exported as
    /// [`crate::ops::types::REGISTRATION_ATTACHMENT_FILTER_NAME_VALUES`].
    pub fn filterable(&self, name: &str) -> Option<Vec<String>> {
        match name {
            "attachment-status" => Some(vec![self.status.clone()]),
            _ => None,
        }
    }
}

/// The policy document `GetResourcePolicy` reports for a resource that carries NO
/// resource-based policy: the empty JSON object, which is what AWS answers once a policy
/// has been deleted.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// A stored resource-based policy, keyed by the ARN of the resource it is attached to.
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    pub resource_arn: String,
    pub policy: String,
    pub created_timestamp: SystemTime,
}

/// A resource this service holds, as the `ResourceType`/id pair its own exceptions name it
/// by — what [`addressed_resource`] reads off an ARN and
/// [`ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State::resource_at`] answers for one
/// this account really holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressedResource {
    /// One of the model's `ResourceType` values (`configuration-set`, `opt-out-list`, …).
    pub resource_type: String,
    /// The resource's own name or minted id, which is the ARN's last path segment.
    pub resource_id: String,
}

/// The resource an ARN of this service ADDRESSES, read off the ARN alone: the
/// `ResourceType` its resource field carries and the name or id inside it.
///
/// An ARN of this service spells the resource as `<kind>/<id>` in its last field
/// (`arn:aws:sms-voice:<region>:<account>:configuration-set/<name>`), so that is what is
/// read. `None` for text carrying no such field — an ARN this service cannot read at all,
/// which is its `INVALID_ARN` rather than a resource it does not hold.
pub fn addressed_resource(resource_arn: &str) -> Option<AddressedResource> {
    let (resource_type, resource_id) = resource_arn.splitn(6, ':').nth(5)?.split_once('/')?;
    (!resource_type.is_empty() && !resource_id.is_empty()).then(|| AddressedResource {
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// A message this emulator accepted for sending.
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub message_id: String,
    pub destination_phone_number: String,
    pub origination_identity: String,
    pub message_body: Option<String>,
    pub message_type: Option<String>,
    pub configuration_set_name: Option<String>,
    pub context: HashMap<String, String>,
    pub dry_run: bool,
    pub sent_at: SystemTime,
}

/// Per-service state for com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2. A handler reaches it through [`service_state`].
///
/// The two resource maps are `BTreeMap`s so a list/describe walks them in a stable
/// order; `DescribePhoneNumbers` pages through that order, and a page boundary that
/// moved between calls would skip or repeat records.
///
/// Every field that grows on a caller-driven path is bounded — see the `MAX_*`
/// constants above. The two bookkeeping stores are private so the only way to grow them
/// is through [`record_message`](Self::record_message) /
/// [`remember_client_token`](Self::remember_client_token), which enforce their caps.
#[derive(Debug, Default)]
pub struct ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State {
    /// Keyed by `ConfigurationSetName`, which AWS makes unique per account. Bounded by
    /// [`MAX_CONFIGURATION_SETS_PER_ACCOUNT`], enforced by `CreateConfigurationSet`.
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// Keyed by `PhoneNumberId`. Bounded by [`MAX_PHONE_NUMBERS_PER_ACCOUNT`], enforced
    /// by `RequestPhoneNumber`.
    pub phone_numbers: BTreeMap<String, PhoneNumberRecord>,
    /// Keyed by `ProtectConfigurationId`. Bounded by
    /// [`MAX_PROTECT_CONFIGURATIONS_PER_ACCOUNT`], enforced by
    /// `CreateProtectConfiguration`.
    pub protect_configurations: BTreeMap<String, ProtectConfiguration>,
    /// Keyed by `OptOutListName`, which AWS makes unique per account. Bounded by
    /// [`MAX_OPT_OUT_LISTS_PER_ACCOUNT`], enforced by `CreateOptOutList`.
    pub opt_out_lists: BTreeMap<String, OptOutList>,
    /// Keyed by `RcsAgentId`. Bounded by [`MAX_RCS_AGENTS_PER_ACCOUNT`], enforced by
    /// `CreateRcsAgent`.
    pub rcs_agents: BTreeMap<String, RcsAgent>,
    /// Keyed by `RegistrationId`. Bounded by [`MAX_REGISTRATIONS_PER_ACCOUNT`], enforced
    /// by `CreateRegistration`.
    pub registrations: BTreeMap<String, Registration>,
    /// Keyed by `RegistrationAttachmentId`. Bounded by
    /// [`MAX_REGISTRATION_ATTACHMENTS_PER_ACCOUNT`], enforced by
    /// `CreateRegistrationAttachment`.
    pub registration_attachments: BTreeMap<String, RegistrationAttachment>,
    /// The resource-based policies this account holds, keyed by the ARN of the resource
    /// each is attached to. Bounded by [`MAX_RESOURCE_POLICIES_PER_ACCOUNT`], enforced by
    /// `PutResourcePolicy`.
    pub resource_policies: BTreeMap<String, ResourcePolicyRecord>,
    /// The most recent accepted sends, oldest first, dry runs included (`dry_run` tells
    /// them apart). A ring bounded by [`MAX_RETAINED_MESSAGES`].
    messages: VecDeque<SentMessage>,
    /// `"<Operation>:<ClientToken>"` -> what that token already created, so a replayed
    /// request with the same token creates nothing further. Bounded by
    /// [`MAX_RETAINED_CLIENT_TOKENS`].
    client_tokens: HashMap<String, ClientTokenRecord>,
    /// The keys of [`Self::client_tokens`] in insertion order, so the oldest entry is the
    /// one evicted when the cap is reached.
    client_token_order: VecDeque<String>,
    /// Monotonic, and the only source of minted ids: phone number ids, synthetic E.164
    /// numbers and message ids all draw from it, so no two of them collide.
    pub sequence: u64,
}

impl ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State {
    /// The next value of the monotonic id counter.
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// Lease one phone number into the store and answer its `PhoneNumberId`.
    ///
    /// The ONE place a lease is minted: the id, the ARN and the synthetic E.164 number all
    /// come from the monotonic [`Self::sequence`], so no two leases collide, and every
    /// member a fresh lease does not get from its request starts at the value the model
    /// declares as that member's default. A caller-driven lease overwrites the optional
    /// members on the stored record afterwards.
    pub fn lease_phone_number(
        &mut self,
        region: &str,
        iso_country_code: String,
        message_type: String,
        number_capabilities: Vec<String>,
        number_type: String,
    ) -> String {
        let sequence = self.next_sequence();
        let id = format!("phone-{sequence:032x}");
        let record = PhoneNumberRecord {
            arn: ::emulator_server::emulator_arn(
                SIGNING_NAME,
                region,
                &format!("phone-number/{id}"),
            ),
            id: id.clone(),
            // A synthetic number from one reserved range, minted from the monotonic
            // sequence so no two leases collide. It satisfies `PhoneNumber`
            // (`^\+?[1-9][0-9]{1,18}$`); the emulator does not claim country-correct
            // numbering, because no numbering plan is available to it.
            phone_number: format!("+1555{sequence:07}"),
            // The emulator provisions synchronously, so the number is handed back in the
            // terminal `ACTIVE` state and no later transition is owed.
            status: ACTIVE_NUMBER_STATUS.to_string(),
            iso_country_code,
            message_type,
            number_capabilities,
            number_type,
            monthly_leasing_price: MONTHLY_LEASING_PRICE.to_string(),
            // Two-way messaging and self-managed opt-outs are off on a fresh lease; the
            // model declares `false` as the default of each.
            two_way_enabled: false,
            two_way_channel_arn: None,
            two_way_channel_role: None,
            self_managed_opt_outs_enabled: false,
            opt_out_list_name: DEFAULT_OPT_OUT_LIST_NAME.to_string(),
            international_sending_enabled: false,
            deletion_protection_enabled: false,
            pool_id: None,
            registration_id: None,
            tags: Vec::new(),
            created_timestamp: SystemTime::now(),
        };
        self.phone_numbers.insert(id.clone(), record);
        id
    }

    /// The origination identity a send uses when the request names none: the
    /// `PhoneNumberId` of the first number this account leases, in the store's stable
    /// order, leasing one for the account when it holds none.
    ///
    /// The identity is the service's to resolve, not the caller's to supply, so there is
    /// always one: an account that has leased nothing is given a number here, and because
    /// it is a lease like any other it is reported by `DescribePhoneNumbers` and resolves
    /// through [`Self::find_phone_number`] exactly as a requested number does. A send
    /// grows the lease store only while it holds nothing, so repeated sends provision at
    /// most this one number however many times they are made.
    pub fn default_origination_identity(&mut self, region: &str) -> String {
        if let Some(record) = self.phone_numbers.values().next() {
            return record.id.clone();
        }
        self.lease_phone_number(
            region,
            DEFAULT_ORIGINATION_ISO_COUNTRY_CODE.to_string(),
            DEFAULT_ORIGINATION_MESSAGE_TYPE.to_string(),
            vec![DEFAULT_ORIGINATION_NUMBER_CAPABILITY.to_string()],
            DEFAULT_ORIGINATION_NUMBER_TYPE.to_string(),
        )
    }

    /// Look a stored number up by any of the origination identities that name it: its
    /// `PhoneNumberId`, its `PhoneNumberArn`, or its E.164 `PhoneNumber` (with or
    /// without the leading `+`, both of which `PhoneNumber` accepts).
    pub fn find_phone_number(&self, identity: &str) -> Option<&PhoneNumberRecord> {
        if let Some(record) = self.phone_numbers.get(identity) {
            return Some(record);
        }
        self.phone_numbers.values().find(|record| {
            record.arn == identity
                || record.phone_number == identity
                || record.phone_number.strip_prefix('+') == Some(identity)
        })
    }

    /// The retained sends, oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &SentMessage> + '_ {
        self.messages.iter()
    }

    /// The resource one ARN of this service addresses AND this account holds, as the
    /// `ResourceType`/id pair its exceptions name it by, or `None` when it holds none.
    ///
    /// The ARN says which kind of resource it names ([`addressed_resource`]), so the kind
    /// says which store to look the id up in — and a kind this emulator holds no store for,
    /// or an id no store holds, is no resource at all. That is what makes a policy call
    /// naming a resource nobody created a not-found rather than a stored policy pointing at
    /// nothing.
    pub fn resource_at(&self, resource_arn: &str) -> Option<AddressedResource> {
        let addressed = addressed_resource(resource_arn)?;
        let id = addressed.resource_id.as_str();
        let held = match addressed.resource_type.as_str() {
            "configuration-set" => self.configuration_sets.contains_key(id),
            "opt-out-list" => self.opt_out_lists.contains_key(id),
            "protect-configuration" => self.protect_configurations.contains_key(id),
            "phone-number" => self.phone_numbers.contains_key(id),
            "rcs-agent" => self.rcs_agents.contains_key(id),
            "registration" => self.registrations.contains_key(id),
            "registration-attachment" => self.registration_attachments.contains_key(id),
            _ => false,
        };
        held.then_some(addressed)
    }

    /// The tags the resource one ARN of this service addresses carries, or `None` when this
    /// account holds no such resource.
    ///
    /// The ONE reading of that question: every record that carries tags is reached through
    /// the kind the ARN names ([`addressed_resource`]), so a tag read and a policy call
    /// resolve an ARN exactly the same way. A kind whose records carry no tag set at all
    /// answers the empty list, which is what a resource with no tags carries anyway.
    pub fn tags_at(&self, resource_arn: &str) -> Option<Vec<Tag>> {
        let addressed = addressed_resource(resource_arn)?;
        let id = addressed.resource_id.as_str();
        match addressed.resource_type.as_str() {
            "configuration-set" => self
                .configuration_sets
                .get(id)
                .map(|record| record.tags.clone()),
            "opt-out-list" => self.opt_out_lists.get(id).map(|record| record.tags.clone()),
            "protect-configuration" => self
                .protect_configurations
                .get(id)
                .map(|record| record.tags.clone()),
            "phone-number" => self.phone_numbers.get(id).map(|record| record.tags.clone()),
            "rcs-agent" => self.rcs_agents.get(id).map(|record| record.tags.clone()),
            "registration" => self.registrations.get(id).map(|record| record.tags.clone()),
            "registration-attachment" => self
                .registration_attachments
                .get(id)
                .map(|record| record.tags.clone()),
            _ => None,
        }
    }

    /// Move every registration attachment whose upload is already DUE to
    /// [`ATTACHMENT_UPLOAD_COMPLETE`], and answer nothing: the transition is the clock's,
    /// owed [`ATTACHMENT_UPLOAD_SPAN`] after the create whether or not anybody asks, so a
    /// read materialises what is already owed rather than deciding anything.
    pub fn settle_attachment_uploads(&mut self, now: SystemTime) {
        for attachment in self.registration_attachments.values_mut() {
            if attachment.status == ATTACHMENT_UPLOAD_IN_PROGRESS
                && now >= attachment.upload_complete_at
            {
                attachment.status = ATTACHMENT_UPLOAD_COMPLETE.to_string();
            }
        }
    }

    /// Record one accepted send, dropping the oldest retained send when the ring is full.
    ///
    /// This is the ONLY way to grow the message log, and the cap dominates its growth:
    /// after [`MAX_RETAINED_MESSAGES`] sends the log's size stops changing.
    pub fn record_message(&mut self, message: SentMessage) {
        while self.messages.len() >= MAX_RETAINED_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
    }

    /// What one `ClientToken` already created, if that entry is retained.
    pub fn client_token_record(&self, operation: &str, token: &str) -> Option<&ClientTokenRecord> {
        self.client_tokens.get(&client_token_key(operation, token))
    }

    /// Remember that `operation` created `resource` under `token` for the request
    /// `fingerprint`, dropping the oldest retained entry when the cap is reached.
    ///
    /// This is the ONLY way to grow the token store, and the cap dominates its growth.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        token: &str,
        resource: &str,
        fingerprint: u64,
    ) {
        let key = client_token_key(operation, token);
        let record = ClientTokenRecord {
            resource: resource.to_string(),
            fingerprint,
        };
        if self.client_tokens.insert(key.clone(), record).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_RETAINED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency entries that point at a resource that no longer exists, so a
    /// delete/release releases the bookkeeping it created too.
    pub fn forget_client_tokens_for(&mut self, operation: &str, resource: &str) {
        let prefix = format!("{operation}:");
        let stale: Vec<String> = self
            .client_tokens
            .iter()
            .filter(|(key, record)| key.starts_with(&prefix) && record.resource == resource)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            self.client_tokens.remove(&key);
            self.client_token_order.retain(|retained| *retained != key);
        }
    }
}

/// The idempotency store's key: a `ClientToken` namespaced by the operation that used
/// it, so the same token on two different creates is two different requests.
fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// What one `ClientToken` already created, and the request it was first used with.
///
/// AWS replays an idempotency token only for an IDENTICAL request, so the fingerprint is
/// kept beside the resource: a replay that changes any of the request's members is a
/// different request and must be refused rather than answered with this resource.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The id (phone number) or name (configuration set) of what was created.
    pub resource: String,
    /// [`request_fingerprint`] of the request the token was first used with.
    pub fingerprint: u64,
}

/// A fingerprint of the request members a `ClientToken` is scoped to, so a replay can be
/// told apart from a different request reusing the token.
pub fn request_fingerprint(members: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    members.len().hash(&mut hasher);
    for member in members {
        member.hash(&mut hasher);
    }
    hasher.finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State>)
}
