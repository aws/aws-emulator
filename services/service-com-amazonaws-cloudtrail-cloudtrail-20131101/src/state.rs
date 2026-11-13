// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudtrail-cloudtrail-20131101 service.
//!
//! This module owns the trail registry — the service's primary resource — plus the
//! rules every trail operation shares: how a caller-supplied identifier (a bare name
//! or a trail ARN) resolves to a stored trail, which trails a request's Region can
//! see, and how a stored trail projects onto the modelled `Trail` / `TrailInfo`
//! shapes. It lives here, not in one handler, because seven operations apply the same
//! rules and two copies of them would drift.
//!
//! It also owns this service's half of the account's MANAGEMENT-EVENT log — the audit
//! record `LookupEvents` serves. The log itself is the shared
//! [`service_common::management_events::ManagementEventLog`], because CloudTrail records
//! the management calls of EVERY service and only the service that served a call knows
//! it happened; what lives here is how this service REACHES that log
//! ([`management_event_log`]), which both the audit leg that records a call
//! (`crate::service::management_events`) and the handler that reports one read it through.
//!
//! It owns the CloudTrail Lake registries on the same terms. An event data store
//! and the queries run against it are named by a dozen operations (`StartQuery`,
//! `DescribeQuery`, `CancelQuery`, `ListQueries`, `GetQueryResults`, the event
//! data store lifecycle), each of which has to resolve the same identifier forms
//! — an ARN or the bare id suffix of one — the same way. A dashboard identifier
//! is a bare name or a dashboard ARN, the refresh-schedule and widget rules are
//! the ones the model documents, and both are applied by every dashboard
//! operation rather than copied into each.
//!
//! An event data store's own configuration — the retention bounds and defaults that
//! follow from its billing mode, and the `CREATED` -> `ENABLED` transition its clock
//! owes — lives here beside that registry, for the same reason.
//!
//! It owns the **resource-based policy** registry too — the policy document attached
//! to one CloudTrail resource — plus the rules every resource-policy operation
//! shares: how a `ResourceArn` is read, which resource types carry a policy at all,
//! which stored resource an ARN resolves to, and what makes a policy document valid.
//!
//! It owns the CloudTrail Lake **channel** registry on the same terms again: a channel
//! identifier is a channel ARN or the bare id the service minted for it, and the refusal
//! wording for an `arn:` value that is not a channel ARN, or for an identifier that names
//! no channel, lives here so every channel operation refuses alike.
//!
//! The **tagging** rules live here for the same reason: a tagging call addresses its
//! resource by ARN, and how that ARN resolves to a stored resource's tag set — plus
//! how a malformed one and one naming nothing are refused — is shared by every
//! tagging operation.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime};

use service_common::data_events::{DataEventFieldSelector, DataEventSelector};
use service_common::management_events::{
    DeliveringTrail, ManagementEventClass, ManagementEventLog, RecordedSession,
    SessionAuditDirectory,
};

use crate::ops::types::{
    AdvancedEventSelector, AggregationConfiguration, ContextKeySelector, EventDataStore,
    EventSelector, ImportSource, ImportStatistics, ImportsListItem, InsightSelector, PartitionKey,
    RefreshSchedule, RequestWidget, Tag, Trail, TrailInfo, Widget, INSIGHT_TYPE_VALUES,
};

/// The Region a request is served in when it carries no SigV4 credential scope to
/// read one from. It matches the Region the emulator writes into the AWS shared
/// config it hands clients, so an unsigned call and a default-configured SDK call
/// land in the same Region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`awsService.arnNamespace` in the Smithy model).
pub const SERVICE_NAMESPACE: &str = "cloudtrail";

/// The resource prefix of a trail ARN: `arn:aws:cloudtrail:<region>:<account>:trail/<name>`.
const TRAIL_RESOURCE_PREFIX: &str = "trail/";

/// Trail-name length bounds, from the model doc for `CreateTrailRequest$Name`
/// ("Be between 3 and 128 characters").
pub const TRAIL_NAME_MIN_LEN: usize = 3;
/// See [`TRAIL_NAME_MIN_LEN`].
pub const TRAIL_NAME_MAX_LEN: usize = 128;

/// From the model doc for `CreateTrailRequest$S3KeyPrefix`: "The maximum length is
/// 200 characters."
pub const S3_KEY_PREFIX_MAX_LEN: usize = 200;

/// From the model doc for `CreateTrailRequest$SnsTopicName`: "The maximum length is
/// 256 characters."
pub const SNS_TOPIC_NAME_MAX_LEN: usize = 256;

/// How CloudTrail words a page-size limit that is outside its bounds: the member the
/// request carried the limit at, and the bound that limit broke.
///
/// `InvalidMaxResultsException` is "thrown if the limit specified is not valid", and the
/// sentence is built from the request — the member and the broken bound — so it cannot
/// drift from the model's range. One function for the whole service, read by both places
/// such a limit is refused from: the pre-dispatch seam in `crate::service`, for the
/// operations whose declared page size the boundary walk checks, and the handler that
/// checks the bound itself (`crate::ops::lookup_events`).
pub fn limit_not_valid_message(member: &str, bound: &str) -> String {
    format!("The limit at '{member}' is not valid: it must have value {bound}.")
}

/// How CloudTrail words a parameter whose value is outside the set that parameter admits:
/// `Value <value> for <Member> is invalid.`
///
/// The fixed text is `Value ` and ` for <Member> is invalid.`, with two parts taken from the
/// request: the value exactly as it was sent — bare, never quoted or escaped — and the
/// parameter that carried it, named by its API path relative to the shape it sits in
/// (`ReadWriteType`, `DataResources.Type`). It is one function for the whole service so that
/// every such refusal is worded identically, wherever the value set comes from: a modelled
/// enum the boundary walk checks (`crate::service`) or a catalogue a handler checks against
/// itself.
pub fn value_not_valid_message(member: &str, value: &str) -> String {
    format!("Value {value} for {member} is invalid.")
}

/// An instant as whole milliseconds since the epoch — the resolution this service compares a
/// recorded time and a caller-supplied window bound at.
pub fn millis_of(at: SystemTime) -> i64 {
    datetime_millis(&::aws_smithy_types::DateTime::from(at))
}

/// A caller-supplied timestamp as whole milliseconds since the epoch ([`millis_of`]).
pub fn datetime_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// From the model doc for `TagsLimitExceededException`: "The number of tags per
/// trail, event data store, dashboard, or channel has exceeded the permitted amount.
/// Currently, the limit is 50."
pub const TAGS_PER_RESOURCE_LIMIT: usize = 50;

/// How CloudTrail words `TagsLimitExceededException`: the RULE it refuses by — a
/// resultant tag set larger than a resource may carry — and nothing of the request.
///
/// The recorded refusal of a tag write whose merged set would pass the limit is "The
/// resultant tag set must not have more than 50 user tags.", and the limit in it is
/// [`TAGS_PER_RESOURCE_LIMIT`] so the sentence cannot drift from the bound the
/// operations enforce. It is the ONE wording of that refusal, read by every operation
/// that writes tags — the tagging pair and the creates that seed tags — because the
/// rule they enforce is the same one.
pub fn tags_limit_exceeded_message() -> String {
    format!("The resultant tag set must not have more than {TAGS_PER_RESOURCE_LIMIT} user tags.")
}

/// How many trails the registry holds before `CreateTrail` refuses with the
/// modelled `MaximumNumberOfTrailsExceededException`.
///
/// CloudTrail publishes an adjustable per-Region trail quota, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's limit.
/// It bounds the WHOLE registry, not one Region's share of it, because the Region a
/// trail is homed in comes from the request: a per-Region bound would still let a
/// caller grow the store without end by varying the Region it signs for. Every
/// record it holds is released by `DeleteTrail`, and it sits far above any workflow
/// an emulator serves.
pub const TRAIL_QUOTA: usize = 1_000;

/// How `ReadWriteType` reads when an event selector leaves it out, from the model doc
/// for `EventSelector$ReadWriteType`: "By default, the value is `All`." It is one of
/// [`crate::ops::types::READ_WRITE_TYPE_VALUES`].
pub const DEFAULT_READ_WRITE_TYPE: &str = "All";

/// How `IncludeManagementEvents` reads when an event selector leaves it out, from the
/// model doc for `EventSelector$IncludeManagementEvents`: "By default, the value is
/// `true`."
pub const DEFAULT_INCLUDE_MANAGEMENT_EVENTS: bool = true;

/// The resource types a BASIC event selector's `DataResource` may name, from the model
/// doc for `DataResource$Type`: "You can specify the following basic event selector
/// resource types: AWS::DynamoDB::Table, AWS::Lambda::Function, AWS::S3::Object".
/// Every other resource type is reached through advanced event selectors.
pub const BASIC_DATA_RESOURCE_TYPES: [&str; 3] = [
    "AWS::DynamoDB::Table",
    "AWS::Lambda::Function",
    "AWS::S3::Object",
];

/// The management event sources `ExcludeManagementEventSources` may name, from the
/// model doc for `EventSelector$ExcludeManagementEventSources`: "the list can be empty
/// (disables the filter), or it can filter out Key Management Service or Amazon RDS
/// Data API events by containing `kms.amazonaws.com` or `rdsdata.amazonaws.com`".
pub const EXCLUDABLE_MANAGEMENT_EVENT_SOURCES: [&str; 2] =
    ["kms.amazonaws.com", "rdsdata.amazonaws.com"];

/// How many basic event selectors one trail may carry, from the model doc for
/// `InvalidEventSelectorsException`: "A trail can have up to 5 event selectors."
pub const EVENT_SELECTORS_PER_TRAIL_LIMIT: usize = 5;

/// How many data resources one trail may carry across all of its basic event
/// selectors, from the model doc for `InvalidEventSelectorsException`: "A trail is
/// limited to 250 data resources. These data resources can be distributed across event
/// selectors, but the overall total cannot exceed 250."
pub const DATA_RESOURCES_PER_TRAIL_LIMIT: usize = 250;

/// How many condition values one trail may carry across all of its advanced event
/// selectors, from the model doc for `PutEventSelectors`: "You can add advanced event
/// selectors, and conditions for your advanced event selectors, up to a maximum of 500
/// values for all conditions and selectors on a trail."
pub const ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT: usize = 500;

/// How many advanced event selectors ONE event data store may carry, from the model
/// doc for `CreateEventDataStoreRequest$AdvancedEventSelectors` (and, in the same
/// words, `UpdateEventDataStoreRequest$AdvancedEventSelectors`): "You can configure up
/// to five advanced event selectors for each event data store."
pub const ADVANCED_SELECTORS_PER_EVENT_DATA_STORE_LIMIT: usize = 5;

/// The one field every advanced event selector has to select on.
///
/// An advanced event selector says WHICH CATEGORY of events it selects before it says
/// anything else about them — the field decides which other fields are even legal, and
/// the model's own doc for `AdvancedFieldSelector$Equals` names it among the fields that
/// accept only that operator. A selector set that never states it selects no category of
/// event at all, which is the modelled `InvalidEventSelectorsException` ("the combination
/// of event selectors or advanced event selectors and data resources is not valid").
pub const EVENT_CATEGORY_SELECTOR_FIELD: &str = "eventCategory";

/// The advanced-selector fields that accept the `Equals` operator and no other, from the
/// model doc for `AdvancedFieldSelector$Equals`: "This is the only valid operator that
/// you can use with the `readOnly`, `eventCategory`, and `resources.type` fields."
pub const EQUALS_ONLY_SELECTOR_FIELDS: [&str; 3] =
    ["readOnly", EVENT_CATEGORY_SELECTOR_FIELD, "resources.type"];

/// How many trails one `ListTrails` page carries. `ListTrails` models no
/// `MaxResults`, so the page size is the service's to choose; the remainder is
/// reached through `NextToken`.
pub const LIST_TRAILS_PAGE_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Insights selectors
//
// The rules the Insights-selector pair (`PutInsightSelectors` /
// `GetInsightSelectors`) shares: which event category an Insights selector
// defaults to, how the `InsightType` value set is worded in a refusal, and how
// an event data store states which category of events it collects.
// ---------------------------------------------------------------------------

/// The event category an Insights selector that names none applies to, from the
/// model doc for `InsightSelector$EventCategories`: "If EventCategories is not
/// provided, the specified Insights types are enabled on management API calls by
/// default." It is one of [`crate::ops::types::SOURCE_EVENT_CATEGORY_VALUES`].
pub const DEFAULT_INSIGHTS_EVENT_CATEGORY: &str = "Management";

/// The event category of the data events Insights can be enabled on, the other
/// value of the model's `SourceEventCategory` enum
/// ([`crate::ops::types::SOURCE_EVENT_CATEGORY_VALUES`]).
pub const DATA_INSIGHTS_EVENT_CATEGORY: &str = "Data";

/// The `AdvancedFieldSelector$Field` an event data store selects a category of
/// events on. "You can call GetEventDataStore on an event data store to check
/// whether the event data store logs management events" (model doc for
/// `PutInsightSelectors`) — which categories a store collects is what its
/// advanced event selectors state, and they state it on this field.
pub const EVENT_CATEGORY_FIELD: &str = "eventCategory";

/// The `eventCategory` an event data store that logs Insights events selects on —
/// the destination store of `PutInsightSelectors$InsightsDestination`, which "must
/// log Insights events" (model doc for `InvalidInsightSelectorsException`).
pub const INSIGHT_EVENT_CATEGORY: &str = "Insight";

/// The Insight types the model's `InsightType` enum carries, worded as a refusal
/// lists them.
///
/// The enum carries exactly two values
/// ([`crate::ops::types::INSIGHT_TYPE_VALUES`]), so the set reads as "A and B";
/// read off the model rather than written out, so it cannot drift from it.
pub fn insight_type_value_set() -> String {
    INSIGHT_TYPE_VALUES.join(" and ")
}

/// The `InvalidInsightSelectorsException` text for an `InsightType` the model's
/// enum does not carry: "this exception is thrown when … the specified
/// `InsightType` in the `InsightSelectors` statement is not valid. Valid values
/// for InsightType are ApiCallRateInsight and ApiErrorRateInsight."
///
/// Built from the request — it names the value the caller sent and the set it is
/// not in — so a caller can fix the call from the refusal alone. The value set is
/// off the generated schema (`skip_validation` on `InsightSelector`), so this is
/// the ONE place that refusal is worded, for every operation that carries an
/// `InsightSelectors` statement.
pub fn invalid_insight_type_message(value: &str) -> String {
    format!(
        "The provided value is not a valid Insights type: {value}. Valid Insight types are: {}. \
         Verify your input and then try again.",
        insight_type_value_set()
    )
}

/// Characters a trail name may contain besides ASCII letters and digits, from the
/// model doc for `CreateTrailRequest$Name`: "periods (.), underscores (_), or
/// dashes (-)".
const TRAIL_NAME_PUNCTUATION: [char; 3] = ['.', '_', '-'];

/// The logging state of one trail.
///
/// `start_logging_time` / `stop_logging_time` are the instants the trail last
/// transitioned into and out of logging, which is what `GetTrailStatus` reports.
#[derive(Debug, Default, Clone)]
pub struct TrailStatus {
    pub is_logging: bool,
    pub start_logging_time: Option<SystemTime>,
    pub stop_logging_time: Option<SystemTime>,
}

/// The event selectors configured on one trail.
///
/// The two kinds are mutually exclusive — the model doc for `PutEventSelectors` says
/// "You can use either AdvancedEventSelectors or EventSelectors, but not both. If you
/// apply AdvancedEventSelectors to a trail, any existing EventSelectors are
/// overwritten" — so the store holds ONE of them rather than two fields that could
/// disagree.
#[derive(Debug, Clone)]
pub enum EventSelectorConfig {
    /// Basic event selectors, as `PutEventSelectors$EventSelectors` carries them.
    Basic(Vec<EventSelector>),
    /// Advanced event selectors, as `PutEventSelectors$AdvancedEventSelectors`
    /// carries them.
    Advanced(Vec<AdvancedEventSelector>),
}

/// The basic event selector a trail logs by, until something configures it.
///
/// From the model doc for `EventSelector`: "By default, trails created without
/// specific event selectors will be configured to log all read and write management
/// events, and no data events" — so `ReadWriteType` is `All`,
/// `IncludeManagementEvents` is true, and neither data resources nor excluded
/// management event sources are in effect.
pub fn default_event_selector() -> EventSelector {
    EventSelector {
        read_write_type: Some(DEFAULT_READ_WRITE_TYPE.to_string()),
        include_management_events: Some(DEFAULT_INCLUDE_MANAGEMENT_EVENTS),
        data_resources: Some(Vec::new()),
        exclude_management_event_sources: Some(Vec::new()),
    }
}

/// One persisted trail: everything `CreateTrail` was given, plus the values the
/// service mints (the trail ARN, the SNS topic ARN, the home Region) and the
/// logging state the Start/StopLogging pair moves.
#[derive(Debug, Default, Clone)]
pub struct TrailRecord {
    pub name: String,
    pub trail_arn: String,
    pub s3_bucket_name: String,
    pub s3_key_prefix: Option<String>,
    pub sns_topic_name: Option<String>,
    pub sns_topic_arn: Option<String>,
    pub include_global_service_events: bool,
    pub is_multi_region_trail: bool,
    pub log_file_validation_enabled: bool,
    pub cloud_watch_logs_log_group_arn: Option<String>,
    pub cloud_watch_logs_role_arn: Option<String>,
    pub kms_key_id: Option<String>,
    pub is_organization_trail: bool,
    pub recursive_logging: bool,
    pub home_region: String,
    /// What `PutEventSelectors` configured, or `None` for a trail nothing has
    /// configured — which logs by [`default_event_selector`] and reports
    /// `HasCustomEventSelectors` false.
    pub event_selectors: Option<EventSelectorConfig>,
    /// The Insights selectors `PutInsightSelectors` configured on this trail and
    /// `GetInsightSelectors` reports.
    ///
    /// EMPTY means Insights event logging is OFF, which is also how a caller turns
    /// it off: "You also use PutInsightSelectors to turn off Insights event logging,
    /// by passing an empty list of Insights types" (model doc for
    /// `PutInsightSelectors`). A trail is created with Insights off.
    pub insight_selectors: Vec<InsightSelector>,
    /// The event aggregation this trail is configured with, as
    /// `PutEventConfiguration` sets it and `GetEventConfiguration` reports it.
    /// Empty for a trail nobody has configured aggregation for; the model bounds
    /// `AggregationConfigurations` to 0..=1 entries.
    pub aggregation_configurations: Vec<AggregationConfiguration>,
    pub tags: Vec<Tag>,
    pub status: TrailStatus,
}

impl TrailRecord {
    /// Project onto the modelled `Trail` shape, as `GetTrail` and `DescribeTrails`
    /// report it.
    pub fn to_trail(&self) -> Trail {
        Trail {
            name: Some(self.name.clone()),
            s3_bucket_name: Some(self.s3_bucket_name.clone()),
            s3_key_prefix: self.s3_key_prefix.clone(),
            // The model deprecates this member — "This field is no longer in use.
            // Use SnsTopicARN." — so only the ARN is reported.
            sns_topic_name: None,
            sns_topic_arn: self.sns_topic_arn.clone(),
            include_global_service_events: Some(self.include_global_service_events),
            is_multi_region_trail: Some(self.is_multi_region_trail),
            home_region: Some(self.home_region.clone()),
            trail_arn: Some(self.trail_arn.clone()),
            log_file_validation_enabled: Some(self.log_file_validation_enabled),
            cloud_watch_logs_log_group_arn: self.cloud_watch_logs_log_group_arn.clone(),
            cloud_watch_logs_role_arn: self.cloud_watch_logs_role_arn.clone(),
            kms_key_id: self.kms_key_id.clone(),
            // "Specifies if the trail has custom event selectors" — true exactly
            // when something configured them, so it is derived from the stored
            // configuration rather than tracked beside it.
            has_custom_event_selectors: Some(self.event_selectors.is_some()),
            // "Specifies whether a trail has Insight types specified in an
            // InsightSelector list" — true exactly when a selector list is
            // configured, so it is derived from the stored selectors rather than
            // tracked beside them.
            has_insight_selectors: Some(!self.insight_selectors.is_empty()),
            is_organization_trail: Some(self.is_organization_trail),
            recursive_logging: Some(self.recursive_logging),
        }
    }

    /// Project onto the modelled `TrailInfo` shape, as `ListTrails` reports it.
    pub fn to_trail_info(&self) -> TrailInfo {
        TrailInfo {
            trail_arn: Some(self.trail_arn.clone()),
            name: Some(self.name.clone()),
            home_region: Some(self.home_region.clone()),
        }
    }

    /// The event selectors `GetEventSelectors` and `PutEventSelectors` report for this
    /// trail, as the `(EventSelectors, AdvancedEventSelectors)` pair of their output.
    ///
    /// Exactly one of the two is reported, because a trail carries one kind: the
    /// advanced selectors when advanced selectors are configured, otherwise the basic
    /// ones — which for a trail nothing has configured is the single
    /// [`default_event_selector`].
    pub fn reported_event_selectors(
        &self,
    ) -> (Option<Vec<EventSelector>>, Option<Vec<AdvancedEventSelector>>) {
        match &self.event_selectors {
            Some(EventSelectorConfig::Basic(selectors)) => (Some(selectors.clone()), None),
            Some(EventSelectorConfig::Advanced(selectors)) => (None, Some(selectors.clone())),
            None => (Some(vec![default_event_selector()]), None),
        }
    }

    /// Whether a request made in `region` sees this trail.
    ///
    /// A trail always shows in its home Region. A multi-Region trail also has a
    /// shadow copy in every other Region, which `DescribeTrails` includes unless the
    /// caller sets `includeShadowTrails` to false.
    pub fn visible_from(&self, region: &str, include_shadow_trails: bool) -> bool {
        self.home_region == region || (include_shadow_trails && self.is_multi_region_trail)
    }
}

/// Why a caller-supplied trail identifier could not be read at all.
///
/// Each variant names the modelled error the handler answers with, so every
/// operation refuses the same malformed identifier the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierRejection {
    /// Shaped like an ARN but not a CloudTrail trail ARN ⇒
    /// `CloudTrailARNInvalidException` (HTTP 400).
    InvalidArn(String),
    /// A bare name outside the documented trail-name rules ⇒
    /// `InvalidTrailNameException` (HTTP 400).
    InvalidName(String),
}

impl IdentifierRejection {
    /// The refusal text: built from the request, naming the value and the rule it
    /// broke, so a caller can fix the request from it alone.
    pub fn message(self) -> String {
        match self {
            Self::InvalidArn(message) | Self::InvalidName(message) => message,
        }
    }
}

/// A caller-supplied trail identifier, in either of the two forms every trail
/// operation accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrailIdentifier {
    /// A bare trail name, which names a trail in the caller's own Region.
    Name(String),
    /// A trail ARN, which also names the Region the trail is homed in.
    Arn { region: String, name: String },
}

impl TrailIdentifier {
    /// The trail name this identifier names — the registry's key.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Arn { name, .. } => name,
        }
    }

    /// The home Region this identifier pins the trail to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }
}

/// The Region this request is served in: the SigV4 credential scope's Region,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the trail called `name` homed in `region`.
pub fn trail_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{TRAIL_RESOURCE_PREFIX}{name}"),
    )
}

/// The ARN of the SNS topic `CreateTrail` was given.
///
/// `SnsTopicName` is documented as "the name or ARN of the Amazon SNS topic", so an
/// ARN is kept as given and a bare name is completed into one.
pub fn sns_topic_arn(region: &str, topic: &str) -> String {
    if topic.starts_with("arn:") {
        topic.to_string()
    } else {
        ::engine::emulator_arn("sns", region, topic)
    }
}

/// The refusal text for a well-formed identifier that names no trail, shared by
/// every operation that answers `TrailNotFoundException`.
///
/// It names the trail by the `subject` the calling operation states — see
/// [`trail_not_found_arn_message`] for which operations state which — and the account
/// the lookup ran in, because a trail name is scoped to one account and Region, so "no
/// trail of this name here" is the whole refusal.
///
/// Called with the request's OWN identifier member, as received, by the operations that
/// take the member as a trail name to look up rather than resolving it to one Region's
/// trail: `GetTrail`, `GetInsightSelectors`, `GetEventConfiguration` and
/// `PutEventConfiguration`, each pinned by its own recorded refusal, plus the tag
/// writes. Those never pass a value SYNTHESIZED from the identifier — an ARN the
/// service expanded a bare name into — because the recorded refusal of each carries
/// the bare name the request sent.
pub fn unknown_trail_message(identifier: &str) -> String {
    format!(
        "Unknown trail: {identifier} for the user: {}",
        ::engine::emulator_account_id()
    )
}

/// Check a bare trail name against the rules the model documents for
/// `CreateTrailRequest$Name`:
///
/// * between [`TRAIL_NAME_MIN_LEN`] and [`TRAIL_NAME_MAX_LEN`] characters,
/// * starts and ends with a letter or digit,
/// * no two adjacent periods, underscores or dashes,
/// * only ASCII letters, digits, periods, underscores or dashes,
/// * not in IP-address format.
///
/// A refusal names the ONE rule the name broke — and, where that rule is a length
/// bound, the bound and the name's own length — rather than restating every rule, so
/// the caller can fix the name from the refusal alone. The rules are checked in the
/// order they are listed above, so a name that breaks several is refused for the
/// first.
///
/// That order is the order the real service applies them in, not an arbitrary one:
/// a name whose last character is neither a letter nor a digit is refused for the
/// terminal-character rule even when that character is also outside the charset
/// (recorded: `bad name!` answers "Trail name must end with a letter or number.",
/// not the charset sentence), and a name shorter than the minimum is refused for
/// its length whatever else it breaks. So the charset rule — which every
/// ill-formed name also breaks — is checked LAST of the character rules, and only
/// a name that satisfies the edge and adjacency rules can be refused for it.
pub fn validate_trail_name(name: &str) -> Result<(), IdentifierRejection> {
    let reject = |message: String| Err(IdentifierRejection::InvalidName(message));

    let length = name.chars().count();
    if length < TRAIL_NAME_MIN_LEN {
        return reject(format!(
            "Trail name too short. Minimum allowed length: {TRAIL_NAME_MIN_LEN} characters. \
             Specified name length: {length} characters."
        ));
    }
    if length > TRAIL_NAME_MAX_LEN {
        return reject(format!(
            "Trail name too long. Maximum allowed length: {TRAIL_NAME_MAX_LEN} characters. \
             Specified name length: {length} characters."
        ));
    }
    // The two edges are separate rules, so a refusal names the edge that broke.
    if name
        .chars()
        .next()
        .is_some_and(|c| !c.is_ascii_alphanumeric())
    {
        return reject("Trail name must start with a letter or number.".to_string());
    }
    if name
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_ascii_alphanumeric())
    {
        return reject("Trail name must end with a letter or number.".to_string());
    }
    let punctuation = |c: &char| TRAIL_NAME_PUNCTUATION.contains(c);
    if name
        .chars()
        .zip(name.chars().skip(1))
        .any(|(a, b)| punctuation(&a) && punctuation(&b))
    {
        return reject(
            "Trail name or ARN cannot have adjacent periods (.), hyphens (-), or \
             underscores (_)."
                .to_string(),
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || TRAIL_NAME_PUNCTUATION.contains(&c))
    {
        return reject(
            "Trail name or ARN can only contain ASCII letters (a-z, A-Z), numbers (0-9), \
             periods (.), underscores (_), or dashes (-)."
                .to_string(),
        );
    }
    // "Not be in IP address format (for example, 192.168.5.4)" — a dotted quad is
    // exactly what `Ipv4Addr`'s parser accepts, and a name cannot hold the colons an
    // IPv6 literal needs.
    if name.parse::<Ipv4Addr>().is_ok() {
        return reject("Trail names must not be formatted as an IP address.".to_string());
    }
    Ok(())
}

/// Read a caller-supplied identifier as either a trail ARN or a bare trail name.
///
/// A value that starts with `arn:` is read as an ARN — so a malformed ARN is
/// refused as one rather than as a bad name — and anything else as a bare name.
pub fn parse_trail_identifier(value: &str) -> Result<TrailIdentifier, IdentifierRejection> {
    if value.starts_with("arn:") {
        return parse_trail_arn(value);
    }
    validate_trail_name(value)?;
    Ok(TrailIdentifier::Name(value.to_string()))
}

/// The Region, account and resource segments of a CloudTrail resource ARN —
/// `arn:<partition>:cloudtrail:<region>:<account>:<resource>` — or `None` when `value`
/// is not one.
///
/// The ARN itself is read through [`::service_common::arn`], this tree's ONE parse of a
/// caller-supplied resource ARN — the same one [`parse_taggable_arn`] uses — so no
/// parser in this module can disagree with another, or with another service, about what
/// an ARN is. What this adds is CloudTrail's own part: every taggable or addressable
/// CloudTrail resource lives in a Region and belongs to an account, so an ARN leaving
/// either field empty names none of them, unlike the partition-global ARNs the shared
/// parse deliberately admits. What the RESOURCE segment must look like stays the
/// caller's rule, because it differs per resource type.
pub fn cloudtrail_arn_fields(value: &str) -> Option<(&str, &str, &str)> {
    let arn = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
    (!arn.region.is_empty() && !arn.account.is_empty())
        .then_some((arn.region, arn.account, arn.resource))
}

/// Read `arn:<partition>:cloudtrail:<region>:<account>:trail/<name>`.
fn parse_trail_arn(value: &str) -> Result<TrailIdentifier, IdentifierRejection> {
    // The refusal names the ARN the caller supplied, which is the one thing the
    // caller has to change — the way the service words it: "The ARN <arn> is
    // invalid."
    let reject = || {
        Err(IdentifierRejection::InvalidArn(format!(
            "The ARN {value} is invalid."
        )))
    };

    let Some((region, _account, resource)) = cloudtrail_arn_fields(value) else {
        return reject();
    };
    let Some(name) = resource.strip_prefix(TRAIL_RESOURCE_PREFIX) else {
        return reject();
    };
    if validate_trail_name(name).is_err() {
        return reject();
    }
    Ok(TrailIdentifier::Arn {
        region: region.to_string(),
        name: name.to_string(),
    })
}

/// The HTTP status every trail not-found refusal answers with.
///
/// The model annotates `TrailNotFoundException` `@httpError(404)`, but CloudTrail
/// answers 400 for it on the wire: the recorded responses of `GetTrail`,
/// `GetTrailStatus`, `DeleteTrail`, `StartLogging` and `StopLogging` on a trail that
/// was never created are HTTP 400 with `__type` `TrailNotFoundException`. The error
/// CODE and the body stay the model's; only the status is stated here, through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`]. It is the same deviation
/// `EventDataStoreNotFoundException` and `QueryIdNotFoundException` already carry in
/// `ops::describe_query`.
pub const TRAIL_NOT_FOUND_STATUS: u16 = 400;

/// The refusal text that names the trail by the ARN the identifier RESOLVED to —
/// the subject the operations that address ONE Region's trail to drive or configure it
/// carry, whichever form the caller wrote.
///
/// A caller who passed a bare name is told which Region's trail was looked for, which
/// is what the recorded refusals of `GetTrailStatus`, `DeleteTrail`, `StartLogging`,
/// `StopLogging`, `UpdateTrail`, `GetEventSelectors` and `PutEventSelectors` carry: a
/// bare name in the request, the fully-qualified trail ARN in the refusal. The
/// operations whose own recording carries the identifier as RECEIVED instead take it
/// from the request member ([`unknown_trail_message`]) and never come here.
pub fn trail_not_found_arn_message(region: &str, name: &str) -> String {
    unknown_trail_message(&trail_arn(region, name))
}

/// The refusal text for `TrailAlreadyExistsException`: `CreateTrail`'s duplicate-name
/// refusal, which names the trail the request asked for and the account that already
/// holds one of that name.
///
/// Request-derived (the `Name` the caller sent) plus the caller's account, the way
/// the recorded response words it — the Region is NOT in it, even though a trail name
/// is scoped to one Region, because "for customer: <account>" is the subject the
/// service states.
pub fn trail_already_exists_message(name: &str) -> String {
    format!(
        "Trail {name} already exists for customer: {}",
        ::engine::emulator_account_id()
    )
}

/// The HTTP status a destination-bucket not-found refusal answers with.
///
/// The model annotates `S3BucketDoesNotExistException` `@httpError(404)`, but
/// CloudTrail answers 400 for it on the wire: the recorded response of `CreateTrail`
/// against a bucket that was never created is HTTP 400 with `__type`
/// `S3BucketDoesNotExistException`. The error CODE and the body stay the model's;
/// only the status is stated here, through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`] — the same deviation
/// [`TRAIL_NOT_FOUND_STATUS`] already carries.
pub const S3_BUCKET_DOES_NOT_EXIST_STATUS: u16 = 400;

/// The refusal text for `S3BucketDoesNotExistException`: the destination bucket the
/// request named, inside the fixed sentence the recorded response carries.
///
/// Request-derived — the `S3BucketName` the caller sent, verbatim — because the
/// caller fixes the call by naming a bucket that exists or by creating that one.
pub fn s3_bucket_does_not_exist_message(bucket: &str) -> String {
    format!("S3 bucket {bucket} does not exist!")
}

/// The refusal text for `InvalidS3BucketNameException`: the name the request gave,
/// inside the fixed sentence the recorded response carries.
///
/// A name outside the Amazon S3 naming rules
/// ([`service_common::s3_bucket_name::is_well_formed`]) names no bucket anywhere, so it
/// is refused as a malformed PARAMETER before any store is asked about it — which is
/// the error the model declares for it ("thrown when the provided S3 bucket name is not
/// valid"), and not the destination-does-not-exist one.
pub fn invalid_s3_bucket_name_message(bucket: &str) -> String {
    format!("S3 bucket name {bucket} is not valid")
}

/// Whether the account's one object store holds a bucket named `bucket`: whether the
/// log-delivery destination a trail request names resolves to anything.
///
/// A trail delivers its log files INTO a bucket, so the destination has to be a bucket
/// that exists — and the buckets of this emulated account live in the Amazon S3
/// service's own store, not in a copy of them here, so that store is where the name is
/// resolved. Both operations that set a trail's destination (`CreateTrail` and
/// `UpdateTrail`) read it through here, so neither can disagree with the other about
/// which destinations exist.
///
/// `None` when that store cannot be reached at all — an engine this emulator's object
/// store is not mounted on — which is a fact about the engine and not about the bucket,
/// so a caller is served rather than refused on a resolution that never happened.
///
/// LOCK ORDER: it takes the object store's lock, holds no other while it does and hands
/// back an owned value, so it must be called with NO guard of this service's own store
/// held. That keeps the cross-service access one-directional and a lock cycle between
/// the two stores unreachable.
pub fn destination_bucket_exists<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    Some(store.bucket(bucket).is_some())
}

/// The refusal text for `InsightNotEnabledException`, shared by every operation that
/// reports or reads Insights events on a resource that has none enabled
/// (`GetInsightSelectors` and the Insights data operations).
///
/// Request-derived (the ARN of the resource the call addressed) plus the fixed tail
/// the recorded response carries, which tells the caller what to change. `kind` is the
/// noun the subject opens with: `Trail` for a trail, which is the form the recorded
/// refusal carries, and [`EVENT_DATA_STORE_SUBJECT`] for an event data store, whose
/// own wording no recording pins — so it states the same rule about the resource it
/// was actually given.
pub fn insight_not_enabled_message(kind: &str, resource_arn: &str) -> String {
    format!(
        "{kind} {resource_arn} does not have Insights enabled. Edit the trail settings to \
         enable Insights, and then try the operation again."
    )
}

/// The subject noun of a refusal about a trail: `Trail <arn>`.
pub const TRAIL_SUBJECT: &str = "Trail";

/// The subject noun of a refusal about an event data store.
pub const EVENT_DATA_STORE_SUBJECT: &str = "Event data store";

/// Render an instant the way the modelled string timestamp members carry it:
/// ISO 8601 at WHOLE-SECOND resolution, `2026-10-21T21:38:12Z`.
///
/// The resolution is part of the form, not an accident of the clock — the instants AWS
/// reports in these members are `%Y-%m-%dT%H:%M:%SZ`, which is the pattern a consumer
/// parses them with — so the instant is TRUNCATED to the second rather than rounded, and a
/// rendered time never names an instant later than the one it reports.
pub fn iso8601(at: SystemTime) -> Option<String> {
    let whole_seconds = at
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| ::std::time::UNIX_EPOCH + Duration::from_secs(since.as_secs()))
        .unwrap_or(at);
    ::aws_smithy_types::DateTime::from(whole_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// How many result rows one `GetQueryResults` page carries when the caller names no
/// `MaxQueryResults`. The model bounds `MaxQueryResults` at
/// `range 1..1000`, so the whole page a caller may ask for is the largest one the
/// service will serve; a caller that wants smaller pages says so.
pub const GET_QUERY_RESULTS_MAX_PAGE_SIZE: usize = 1_000;

/// One row of a query result: its columns in order, each a (name, value) pair.
///
/// The modelled `QueryResultRow` is a LIST of single-entry maps rather than one map,
/// which is what keeps a row's columns ordered on the wire — so a row is stored
/// ordered here too.
pub type QueryResultRowRecord = Vec<(String, String)>;

/// How many import failures one `ListImportFailures` page carries when the request
/// names no `MaxResults`.
///
/// The model bounds `ListImportFailuresRequest$MaxResults` at
/// [`LIST_IMPORT_FAILURES_MAX_RESULTS_MIN`]..=[`LIST_IMPORT_FAILURES_MAX_RESULTS_MAX`]
/// and documents no default, so the default page size is the service's to choose;
/// whatever a page leaves over is reached through `NextToken`.
pub const LIST_IMPORT_FAILURES_DEFAULT_MAX_RESULTS: usize = 100;

/// The smallest `MaxResults` the model accepts, from the `range` trait on
/// `ListImportFailuresMaxResultsCount` (1..1000).
pub const LIST_IMPORT_FAILURES_MAX_RESULTS_MIN: usize = 1;
/// See [`LIST_IMPORT_FAILURES_MAX_RESULTS_MIN`].
pub const LIST_IMPORT_FAILURES_MAX_RESULTS_MAX: usize = 1000;

/// One failure an import recorded for a single source location, as
/// `ListImportFailures` reports it.
///
/// `status` carries one of the values the model's `ImportFailureStatus` enum
/// accepts (`crate::ops::types::IMPORT_FAILURE_STATUS_VALUES`).
#[derive(Debug, Clone)]
pub struct ImportFailureRecord {
    pub location: String,
    pub status: String,
    pub error_type: String,
    pub error_message: String,
    pub last_updated_time: SystemTime,
}

impl ImportFailureRecord {
    /// Project onto the modelled `ImportFailureListItem` shape, as
    /// `ListImportFailures` reports it. The shape is named in full rather than
    /// imported: this module's `use crate::ops::types::{..}` list is edited by every
    /// operation that lands, and a one-line addition there conflicts with all of them.
    pub fn to_failure_list_item(&self) -> crate::ops::types::ImportFailureListItem {
        crate::ops::types::ImportFailureListItem {
            location: Some(self.location.clone()),
            status: Some(self.status.clone()),
            error_type: Some(self.error_type.clone()),
            error_message: Some(self.error_message.clone()),
            last_updated_time: Some(::aws_smithy_types::DateTime::from(self.last_updated_time)),
        }
    }
}

// ---------------------------------------------------------------------------
// Event data store lifecycle (CloudTrail Lake)
// ---------------------------------------------------------------------------

/// The shortest retention period an event data store may carry, from the model's
/// `@range` on `com.amazonaws.cloudtrail#RetentionPeriod` (7..3653).
pub const RETENTION_PERIOD_MIN_DAYS: i32 = 7;

/// From the model doc for `CreateEventDataStoreRequest$AdvancedEventSelectors`:
/// "You can configure up to five advanced event selectors for each event data
/// store." Exceeding it is the modelled `InvalidEventSelectorsException`.
pub const ADVANCED_EVENT_SELECTOR_LIMIT: usize = 5;

/// The default and maximum retention period of `billing_mode`, in days.
///
/// From the model doc for `CreateEventDataStoreRequest$BillingMode`:
/// "`EXTENDABLE_RETENTION_PRICING` - ... a flexible retention period of up to 3653
/// days (about 10 years). The default retention period for this billing mode is 366
/// days." / "`FIXED_RETENTION_PRICING` - ... need a retention period of up to 2557
/// days (about 7 years). The default retention period for this billing mode is 2557
/// days."
pub fn retention_bounds(billing_mode: EventDataStoreBillingMode) -> (i32, i32) {
    match billing_mode {
        EventDataStoreBillingMode::ExtendableRetentionPricing => (366, 3653),
        EventDataStoreBillingMode::FixedRetentionPricing => (2557, 2557),
    }
}

/// How long a newly created event data store stays in `CREATED` before the clock owes
/// it the status it was created for.
///
/// AWS publishes no span for it, so this is the emulator's own: short enough that a
/// workflow is not slowed, long enough that a caller reading the store straight back
/// still sees the transient `CREATED` the real service reports.
pub const EVENT_DATA_STORE_PROVISIONING: Duration = Duration::from_millis(500);

/// How long a store stays in `PENDING_DELETION` before it is really gone, from the
/// model doc for `DeleteEventDataStore`: "the event data store enters a
/// PENDING_DELETION state, and is automatically deleted after a wait period of seven
/// days."
pub const EVENT_DATA_STORE_DELETION_WAIT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many event data stores the registry holds before `CreateEventDataStore`
/// refuses with the modelled `EventDataStoreMaxLimitExceededException`.
///
/// CloudTrail publishes an adjustable per-Region quota, and this emulator models
/// neither Service Quotas nor an account's adjustments — so this is the emulator's
/// OWN bound on the store rather than a claim about any account's limit. It bounds
/// the WHOLE registry, because the Region a store is created in comes from the
/// request: a per-Region bound would still let a caller grow the store without end
/// by varying the Region it signs for. It sits far above any workflow an emulator
/// serves.
pub const EVENT_DATA_STORE_QUOTA: usize = 1_000;

/// The fully specified KMS key ARN a `KmsKeyId` names.
///
/// The request takes "an alias name prefixed by alias/, a fully specified ARN to an
/// alias, a fully specified ARN to a key, or a globally unique identifier" — the forms
/// differ only in how they SPELL one key, so two spellings of the same key name the
/// same key. Completing a bare value into the ARN it names is what lets a caller that
/// re-states a store's key in another accepted form be told it changed nothing.
pub fn kms_key_arn(region: &str, key: &str) -> String {
    if key.starts_with("arn:") {
        key.to_string()
    } else if key.starts_with("alias/") {
        ::engine::emulator_arn("kms", region, key)
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{key}"))
    }
}

/// The `ImportStatus` values the model's enum declares
/// (`smithy_query(query="shape",shape="ImportStatus")`): `INITIALIZING | IN_PROGRESS |
/// FAILED | STOPPED | COMPLETED`.
pub const IMPORT_STATUS_INITIALIZING: &str = "INITIALIZING";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_STOPPED: &str = "STOPPED";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_COMPLETED: &str = "COMPLETED";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_FAILED: &str = "FAILED";

/// The statuses an import can still leave. The other three — `COMPLETED`, `FAILED`
/// and `STOPPED` — are where an import ends, so nothing moves them.
pub const RUNNING_IMPORT_STATUSES: [&str; 2] =
    [IMPORT_STATUS_INITIALIZING, IMPORT_STATUS_IN_PROGRESS];

/// How long after `StartImport` the clock owes an import its `INITIALIZING` ->
/// `IN_PROGRESS` transition.
///
/// CloudTrail starts an import asynchronously — `StartImport` answers
/// `INITIALIZING` — so the move to `IN_PROGRESS` is a transition the clock makes
/// due rather than one a later call performs. The span is the emulator's OWN
/// choice (CloudTrail documents no time for it), and it is deliberately non-zero so
/// a caller reading the import straight after starting it still sees
/// `INITIALIZING`.
pub const IMPORT_INGESTION_STARTS_AFTER: Duration = Duration::from_millis(500);

/// How many import jobs the registry holds before `StartImport` refuses.
///
/// It bounds the WHOLE registry, not one Region's share of it, for the reason
/// [`TRAIL_QUOTA`] states: the Region an import is created in comes from the
/// request, so a per-Region bound would still let a caller grow the store without
/// end. `StartImport` models no quota error, so the refusal is the modelled
/// `OperationNotPermittedException` ("this exception is thrown when the requested
/// operation is not permitted"), and the bound sits far above any workflow an
/// emulator serves.
pub const IMPORT_QUOTA: usize = 1_000;

/// The HTTP status this service answers `ImportNotFoundException` with.
///
/// The model annotates the shape `@httpError(404)`
/// (`smithy_query(query="shape",shape="ImportNotFoundException")`), but `@httpError`
/// is an HTTP-BINDING trait and the recorded CloudTrail responses to an import
/// operation addressing an unknown import id carry HTTP **400** with
/// `__type: ImportNotFoundException` — the status a client's retry classifier sees is
/// the service's, not the model's. A handler answers it through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line.
pub const IMPORT_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// The refusal text for an import id that names no import.
///
/// Built from the request so a caller can tell WHICH id was not found, in the wording
/// the recorded CloudTrail responses use ("The import job <id> was not found.").
pub fn import_not_found_message(import_id: &str) -> String {
    format!("The import job {import_id} was not found.")
}

/// One persisted import job: the request `StartImport` was given, the status the job
/// is in, and the two instants the service stamps on it.
///
/// `statistics` is `None` while nothing has been imported: this emulator reads no S3
/// bucket, so an import reports no counts rather than counts nothing produced.
#[derive(Debug, Clone)]
pub struct ImportRecord {
    pub import_id: String,
    /// The destination event data store ARNs (`ImportDestinations`, length 1..1).
    pub destinations: Vec<String>,
    pub import_source: Option<ImportSource>,
    pub start_event_time: Option<SystemTime>,
    pub end_event_time: Option<SystemTime>,
    /// One of the [`IMPORT_STATUS_INITIALIZING`] family.
    pub status: String,
    pub created_timestamp: SystemTime,
    pub updated_timestamp: SystemTime,
    /// When `INITIALIZING` is due to become `IN_PROGRESS`; see
    /// [`ImportRecord::settle`].
    pub ingests_at: SystemTime,
    pub statistics: Option<ImportStatistics>,
}

impl ImportRecord {
    /// Whether this import is still running, so a stop has something to stop.
    pub fn is_running(&self) -> bool {
        RUNNING_IMPORT_STATUSES.contains(&self.status.as_str())
    }

    /// Materialise the `INITIALIZING` -> `IN_PROGRESS` transition the clock already
    /// owes.
    ///
    /// A reporting operation may call this: the transition was due at `ingests_at`
    /// whether or not anybody read the import, so applying it changes nothing a
    /// later caller would otherwise have seen — which is also why the stamped
    /// `UpdatedTimestamp` is the instant the move was DUE, not the instant it was
    /// observed.
    ///
    /// Nothing moves an import out of `IN_PROGRESS` on its own: this emulator reads
    /// no S3 bucket, so no events are imported and no status may claim the import
    /// finished importing them. A caller ends a running import with `StopImport`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == IMPORT_STATUS_INITIALIZING && now >= self.ingests_at {
            self.status = IMPORT_STATUS_IN_PROGRESS.to_string();
            self.updated_timestamp = self.ingests_at;
        }
    }

    /// Project onto the modelled `ImportsListItem` shape, as `ListImports` reports
    /// it: the five members that shape carries, each from the stored record.
    pub fn to_imports_list_item(&self) -> ImportsListItem {
        ImportsListItem {
            import_id: Some(self.import_id.clone()),
            import_status: Some(self.status.clone()),
            destinations: Some(self.destinations.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_timestamp)),
            updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.updated_timestamp)),
        }
    }

    /// Stop this import.
    ///
    /// A running import moves to `STOPPED` and its `UpdatedTimestamp` becomes `at`.
    /// An import that already reached a terminal status is left exactly as it is —
    /// there is nothing left to stop — so a repeated stop reports the same record.
    pub fn stop(&mut self, at: SystemTime) {
        if self.is_running() {
            self.status = IMPORT_STATUS_STOPPED.to_string();
            self.updated_timestamp = at;
        }
    }
}

/// The import registry key: an import id and the Region the import was created in.
///
/// CloudTrail is a regional service — an import belongs to the Region its destination
/// event data store lives in — so an import is reachable only from that Region, the
/// same way [`TrailKey`] scopes a trail name. The id comes FIRST so the `BTreeMap`
/// iterates id-ordered, which is the order `ListImports` pages through.
pub type ImportKey = (String, String);

/// The first tag key that appears more than once in `tags`, if any.
///
/// `InvalidTagParameterException` covers duplicate tags, and a tag set is a map on
/// the wire even though the model carries it as a list — so the rule holds for every
/// taggable resource, not just one of them.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen = ::std::collections::BTreeSet::new();
    tags.iter()
        .find(|tag| !seen.insert(tag.key.as_str()))
        .map(|tag| tag.key.as_str())
}

/// The registry key: a trail name and the Region the trail is homed in.
///
/// AWS scopes a trail name to the Region it was created in, so the name alone is
/// not a key: two Regions may each hold a trail of the same name, and each is
/// reachable only from its own Region (or by its own ARN). The name comes FIRST so
/// the `BTreeMap` iterates name-ordered, which is the order `DescribeTrails` and
/// `ListTrails` report and the order `ListTrails` pages through.
pub type TrailKey = (String, String);

/// The HTTP status this service answers `EventDataStoreNotFoundException` with.
///
/// The model annotates the shape `@httpError(404)`
/// (`smithy_query(query="shape",shape="EventDataStoreNotFoundException")`), but
/// `@httpError` is an HTTP-BINDING trait and the recorded CloudTrail responses to a
/// call addressing an unknown event data store carry HTTP **400** with
/// `__type: EventDataStoreNotFoundException` — the status a client's retry classifier
/// sees is the service's, not the model's. It lives here, beside the refusal text, so
/// every handler that answers this error answers it with one status rather than a
/// hand-copied constant per operation; a handler applies it through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line.
pub const EVENT_DATA_STORE_NOT_FOUND_STATUS: u16 = 400;

/// The refusal text for `EventDataStoreNotFoundException`.
///
/// The model annotates CloudTrail's not-found errors — `ChannelNotFoundException`,
/// `EventDataStoreNotFoundException`, `QueryIdNotFoundException` — `@httpError(404)`,
/// but CloudTrail answers 400 for them on the wire: the recorded responses for
/// `GetChannel`, `GetEventDataStore` and `DescribeQuery` are all `HTTP 400` with the
/// model's own `__type`. The error CODE stays the model's; only the status is stated
/// here, and an operation applies it through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`]. It lives here so the four
/// operations that refuse a not-found cannot drift apart on the status.
pub const NOT_FOUND_STATUS: u16 = 400;

/// The refusal text for `EventDataStoreNotFoundException` when the request named no
/// event data store — the store the caller asked for is exactly what could not be
/// found, which is how the service words it.
pub const EVENT_DATA_STORE_NOT_FOUND_MESSAGE: &str = "The event data store was not found.";

/// The refusal text for `EventDataStoreNotFoundException` on a call that names ONE
/// store, as the recorded response of the real service words it: it names the ARN
/// the request addressed, which for a bare id is the one that id completes to in
/// the caller's Region.
pub fn event_data_store_not_found_message(arn: &str) -> String {
    format!("The event data store {arn} was not found.")
}

/// The refusal text for `QueryIdNotFoundException`: it names the member the
/// request supplied and its value, the way the service words it — "The QueryId
/// value <id> does not exist or is not in the same region."
pub fn query_not_found_message(member: &str, value: &str) -> String {
    format!("The {member} value {value} does not exist or is not in the same region.")
}

/// Mint the page token that resumes a paged read of `resource_id` at `offset`.
///
/// Every paged operation of this service hands out the same shape of token — the
/// resource the page belongs to, then the row the next page resumes at — so the
/// encoding lives here once rather than beside each handler. The `separator` is the
/// operation's own, because a token form it has already handed out is part of that
/// operation's wire contract; a resource id is a `UUID` (`^[a-f0-9\-]+$`) and so holds
/// neither separator, which is what keeps the two halves apart.
pub fn encode_page_token(resource_id: &str, offset: usize, separator: char) -> String {
    format!("{resource_id}{separator}{offset}")
}

/// Read a caller-supplied page token back as the offset it resumes at, or `None` when
/// it is not a token minted for `resource_id`: no separator, another resource's token,
/// or an offset that is not a row number. The model documents
/// `InvalidNextTokenException` as "a token that is not valid, or a token that was
/// previously used in a request with different parameters", which is the refusal a
/// `None` answers with; whether an offset is still within the rows that remain is the
/// calling operation's own check, since only it knows how many there are.
pub fn decode_page_token(token: &str, resource_id: &str, separator: char) -> Option<usize> {
    let (minted_for, offset) = token.rsplit_once(separator)?;
    if minted_for != resource_id {
        return None;
    }
    offset.parse::<usize>().ok()
}

/// The resource prefix of an event data store ARN:
/// `arn:aws:cloudtrail:<region>:<account>:eventdatastore/<id>`.
const EVENT_DATA_STORE_RESOURCE_PREFIX: &str = "eventdatastore/";

/// The status of an event data store: one variant per value of the model's
/// `EventDataStoreStatus` enum (`ops::types::EVENT_DATA_STORE_STATUS_VALUES`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EventDataStoreState {
    Created,
    #[default]
    Enabled,
    PendingDeletion,
    StartingIngestion,
    StoppingIngestion,
    StoppedIngestion,
}

impl EventDataStoreState {
    /// The value this state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Enabled => "ENABLED",
            Self::PendingDeletion => "PENDING_DELETION",
            Self::StartingIngestion => "STARTING_INGESTION",
            Self::StoppingIngestion => "STOPPING_INGESTION",
            Self::StoppedIngestion => "STOPPED_INGESTION",
        }
    }

    /// Whether the store is still an active lake at all.
    ///
    /// A store awaiting deletion is not, which is what the modelled
    /// `InactiveEventDataStoreException` ("The event data store is inactive")
    /// reports. A store that has stopped ingesting new events is still active: it
    /// serves queries over what it already holds, and ingestion is restarted with
    /// `StartEventDataStoreIngestion` — so only `PENDING_DELETION` is inactive.
    pub fn is_active(self) -> bool {
        !matches!(self, Self::PendingDeletion)
    }

    /// Whether a query operation may name this store — exactly the stores that are
    /// still active ([`EventDataStoreState::is_active`]).
    pub fn is_queryable(self) -> bool {
        self.is_active()
    }

    /// The status an in-flight ingestion transition settles into once the clock
    /// has run, or `None` for a status the clock owes nothing on.
    ///
    /// The model's `EventDataStoreStatus` enum carries `STARTING_INGESTION` and
    /// `STOPPING_INGESTION` precisely because the Start/Stop ingestion pair is
    /// asynchronous: a store told to stop reads `STOPPING_INGESTION` until
    /// ingestion has stopped, and `STOPPED_INGESTION` after. Every other value
    /// is terminal — the clock moves none of them on its own.
    pub fn settled(self) -> Option<Self> {
        match self {
            Self::StartingIngestion => Some(Self::Enabled),
            Self::StoppingIngestion => Some(Self::StoppedIngestion),
            Self::Created | Self::Enabled | Self::PendingDeletion | Self::StoppedIngestion => None,
        }
    }
}

/// The event categories whose ingestion can be stopped and started.
///
/// From the model doc for `StopEventDataStoreIngestion` (and, in the same words,
/// `StartEventDataStoreIngestion`): "To stop ingestion, the event data store
/// `Status` must be `ENABLED` and the `eventCategory` must be `Management`,
/// `Data`, `NetworkActivity`, or `ConfigurationItem`." A store collecting any
/// other category (CloudTrail Insights events, Audit Manager evidence, events
/// from outside Amazon Web Services) ingests nothing a caller can pause, which is
/// what the modelled `InvalidEventDataStoreCategoryException` reports.
pub const INGESTION_CONTROLLABLE_EVENT_CATEGORIES: &[&str] =
    &["Management", "Data", "NetworkActivity", "ConfigurationItem"];

/// How long an event data store reads a non-terminal status — `CREATED` after
/// `CreateEventDataStore`, `STARTING_INGESTION` / `STOPPING_INGESTION` after the
/// Start/Stop ingestion pair — before the clock settles it into the status it is
/// heading for (`ENABLED` / `STOPPED_INGESTION`).
///
/// The model's `EventDataStoreStatus` enum carries every one of those in-flight
/// values, so both transitions ARE asynchronous. CloudTrail publishes no duration
/// for either, so this span is the emulator's own: short enough not to stall a
/// workflow, non-zero so a caller that reads the store straight after the call
/// still sees the transient status — and so the state errors that transient
/// status carries are reachable.
pub const INGESTION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The billing mode of an event data store: one variant per value of the model's
/// `BillingMode` enum.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EventDataStoreBillingMode {
    /// The model doc for `CreateEventDataStoreRequest$BillingMode`: "The default
    /// value is EXTENDABLE_RETENTION_PRICING."
    #[default]
    ExtendableRetentionPricing,
    FixedRetentionPricing,
}

impl EventDataStoreBillingMode {
    /// The value this billing mode carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ExtendableRetentionPricing => "EXTENDABLE_RETENTION_PRICING",
            Self::FixedRetentionPricing => "FIXED_RETENTION_PRICING",
        }
    }

    /// Read a billing mode a request supplied, or `None` for a value the model's
    /// `BillingMode` enum does not carry.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "EXTENDABLE_RETENTION_PRICING" => Some(Self::ExtendableRetentionPricing),
            "FIXED_RETENTION_PRICING" => Some(Self::FixedRetentionPricing),
            _ => None,
        }
    }
}

/// The Lake query federation state of an event data store: one variant per value
/// of the model's `FederationStatus` enum.
///
/// `Disabled` is the default because a store federates only once
/// `EnableFederation` is called for it: "The status is ENABLED if Lake query
/// federation is enabled, or DISABLED if Lake query federation is disabled"
/// (model doc for `GetEventDataStoreResponse$FederationStatus`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FederationState {
    Enabling,
    Enabled,
    Disabling,
    #[default]
    Disabled,
}

impl FederationState {
    /// The value this federation state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Enabling => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling => "DISABLING",
            Self::Disabled => "DISABLED",
        }
    }

    /// Whether Lake query federation is on, or on its way on — the two states
    /// `DisableFederation` has something to turn off.
    pub fn is_on(self) -> bool {
        matches!(self, Self::Enabling | Self::Enabled)
    }

    /// The state this one settles into once the transition it names has run, or
    /// `None` for a state the clock owes nothing on.
    ///
    /// `ENABLING` and `DISABLING` exist in the model precisely because the
    /// Enable/Disable pair is asynchronous; `ENABLED` and `DISABLED` are terminal.
    pub fn settled(self) -> Option<Self> {
        match self {
            Self::Enabling => Some(Self::Enabled),
            Self::Disabling => Some(Self::Disabled),
            Self::Enabled | Self::Disabled => None,
        }
    }
}

/// The maximum size an event stored in an event data store may have: one variant
/// per value of the model's `MaxEventSize` enum
/// (`ops::types::MAX_EVENT_SIZE_VALUES`).
///
/// `Standard` is the default because `Large` is reachable only by asking for it:
/// "If you are using context key selectors, MaxEventSize must be set to Large"
/// (model doc for `PutEventConfigurationRequest$MaxEventSize`), so a store nobody
/// has configured carries the smaller limit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MaxEventSizeSetting {
    #[default]
    Standard,
    Large,
}

impl MaxEventSizeSetting {
    /// The value this setting carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Large => "Large",
        }
    }

    /// Read a wire value of the model's `MaxEventSize` enum.
    ///
    /// `None` is a value outside the enum, which the boundary validator refuses
    /// before a handler sees it.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Standard" => Some(Self::Standard),
            "Large" => Some(Self::Large),
            _ => None,
        }
    }
}

/// How long an event data store reads `ENABLING` / `DISABLING` before the clock
/// settles it into `ENABLED` / `DISABLED`.
///
/// The model's `FederationStatus` enum carries both in-flight values, so
/// `EnableFederation` and `DisableFederation` ARE asynchronous. CloudTrail
/// publishes no duration for either, so this span is the emulator's own: short
/// enough not to stall a workflow, non-zero so a caller that reads the store
/// straight after the call still sees the transient status.
pub const FEDERATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// One persisted CloudTrail Lake event data store.
///
/// The ARN is not stored: it is a function of the Region and the id
/// ([`event_data_store_arn`]), so deriving it keeps one source of truth. Every
/// other member a read reports IS stored, so a read computes its whole answer from
/// this record rather than from the request that asked for it.
#[derive(Debug, Default, Clone)]
pub struct EventDataStoreRecord {
    /// The ARN's resource suffix — the identifier every operation accepts in
    /// place of the full ARN.
    pub id: String,
    pub name: String,
    /// The Region the store lives in; it is reachable only from there.
    pub region: String,
    pub status: EventDataStoreState,
    /// The status the CLOCK owes this store, and the instant it comes due: a store
    /// AWS reports in a non-terminal status reaches its next one with no further
    /// call. Materialised by [`EventDataStoreRecord::settle`].
    pub pending_status: Option<(SystemTime, EventDataStoreState)>,
    /// When an in-flight ingestion `status` (`STARTING_INGESTION` /
    /// `STOPPING_INGESTION`) is due to settle into the status it is heading for;
    /// `None` once it has, and for a store whose ingestion is not moving.
    pub ingestion_settles_at: Option<SystemTime>,
    /// The account the store belongs to, or `None` for the emulator's own
    /// account. A cross-account query echoes it back as
    /// `EventDataStoreOwnerAccountId`.
    pub owner_account_id: Option<String>,
    pub created_at: Option<SystemTime>,
    /// When the store was last changed. "UpdatedTimestamp is always either the
    /// same or newer than the time shown in CreatedTimestamp" (model doc for
    /// `GetEventDataStoreResponse$UpdatedTimestamp`), and `None` for a store
    /// nothing has changed since it was created.
    pub updated_at: Option<SystemTime>,
    /// The selectors that choose which events the store collects.
    pub advanced_event_selectors: Vec<AdvancedEventSelector>,
    pub multi_region_enabled: bool,
    pub organization_enabled: bool,
    /// In days; the model bounds `RetentionPeriod` to 7..=3653. `None` for a store
    /// whose retention nothing has stated.
    pub retention_period: Option<i32>,
    pub termination_protection_enabled: bool,
    pub kms_key_id: Option<String>,
    pub billing_mode: EventDataStoreBillingMode,
    pub federation_status: FederationState,
    /// When an in-flight `federation_status` (`ENABLING` / `DISABLING`) is due to
    /// settle into the state it is heading for; `None` once it has, and for a
    /// store whose federation is not moving.
    pub federation_settles_at: Option<SystemTime>,
    /// The role Lake query federation accesses the store's resources through; set
    /// only while federation is on.
    pub federation_role_arn: Option<String>,
    /// The keys CloudTrail Lake partitions the store's event data by. The model
    /// bounds `PartitionKeyList` to 0..=2 entries.
    pub partition_keys: Vec<PartitionKey>,
    /// The largest event the store accepts, as `PutEventConfiguration` sets it
    /// and `GetEventConfiguration` reports it.
    pub max_event_size: MaxEventSizeSetting,
    /// The context keys CloudTrail includes in this store's enriched events, as
    /// `PutEventConfiguration` sets them and `GetEventConfiguration` reports
    /// them. Empty for a store nobody has configured; the model bounds
    /// `ContextKeySelectors` to 0..=2 entries.
    pub context_key_selectors: Vec<ContextKeySelector>,
    /// The store's tag set, as `CreateEventDataStore` seeds it, `AddTags` merges into
    /// it, `RemoveTags` drops keys from it and `ListTags` reports it.
    pub tags: Vec<Tag>,
    /// When the `PENDING_DELETION` wait period elapses and the store is gone, set
    /// by `DeleteEventDataStore`.
    pub deleted_at: Option<SystemTime>,
    /// The Insights selectors `PutInsightSelectors` configured on this store and
    /// `GetInsightSelectors` reports. EMPTY means Insights event logging is off,
    /// on the same terms as [`TrailRecord::insight_selectors`].
    pub insight_selectors: Vec<InsightSelector>,
    /// The ARN of the store the Insights events this store generates are logged
    /// to (`PutInsightSelectors$InsightsDestination`); `None` while Insights are
    /// off, which is the only state in which no destination is configured.
    pub insights_destination: Option<String>,
}

impl EventDataStoreRecord {
    /// The account that owns this store: the one it was created for, defaulting
    /// to the emulator's own account.
    pub fn owner(&self) -> &str {
        match self.owner_account_id.as_deref() {
            Some(account_id) => account_id,
            None => ::engine::emulator_account_id(),
        }
    }

    /// This store's ARN, derived from the Region it lives in and its id.
    pub fn arn(&self) -> String {
        event_data_store_arn(&self.region, &self.id)
    }

    /// Take the status the clock already owes this store, if its instant has passed.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some((due, next)) = self.pending_status {
            if now >= due {
                self.status = next;
                self.pending_status = None;
            }
        }
    }

    /// Whether this store's seven-day `PENDING_DELETION` wait has elapsed, so a
    /// caller would no longer find it at all.
    pub fn deletion_due(&self, now: SystemTime) -> bool {
        self.deleted_at.is_some_and(|at| now >= at)
    }

    /// Whether this store collects events of `category`: one of its advanced event
    /// selectors selects that value on the [`EVENT_CATEGORY_FIELD`].
    ///
    /// That is what "the event data store logs management events" and "the
    /// destination event data store … must log Insights events" (model doc for
    /// `InvalidInsightSelectorsException`) are read against, so both are decided
    /// from the store's own selectors rather than from the request asking about it.
    pub fn logs_event_category(&self, category: &str) -> bool {
        self.advanced_event_selectors.iter().any(|selector| {
            selector.field_selectors.iter().any(|field_selector| {
                field_selector.field == EVENT_CATEGORY_FIELD
                    && field_selector
                        .equals
                        .iter()
                        .flatten()
                        .any(|value| value == category)
            })
        })
    }

    /// Materialise the federation transition the clock already owes: `ENABLING`
    /// into `ENABLED`, `DISABLING` into `DISABLED`.
    ///
    /// A reporting operation reaches this through
    /// [`ComAmazonawsCloudtrailCloudtrail20131101State::settle_event_data_stores`],
    /// the one pass that applies every transition a store is owed: the transition
    /// was due at
    /// `federation_settles_at` whether or not anybody read the store, so applying
    /// it changes nothing a later caller would otherwise have seen. Federation
    /// reaching `DISABLED` releases the role it ran under — that role is held
    /// "only while federation is on".
    fn settle_federation(&mut self, now: SystemTime) {
        let Some(due) = self.federation_settles_at else {
            return;
        };
        if now < due {
            return;
        }
        self.federation_settles_at = None;
        if let Some(settled) = self.federation_status.settled() {
            self.federation_status = settled;
            if !settled.is_on() {
                self.federation_role_arn = None;
            }
        }
    }

    /// Materialise the ingestion transition the clock already owes:
    /// `STARTING_INGESTION` into `ENABLED`, `STOPPING_INGESTION` into
    /// `STOPPED_INGESTION`.
    ///
    /// A reporting operation reaches this through
    /// [`ComAmazonawsCloudtrailCloudtrail20131101State::settle_event_data_stores`]:
    /// the transition was due at
    /// `ingestion_settles_at` whether or not anybody read the store, so applying
    /// it changes nothing a later caller would otherwise have seen.
    fn settle_ingestion(&mut self, now: SystemTime) {
        let Some(due) = self.ingestion_settles_at else {
            return;
        };
        if now < due {
            return;
        }
        self.ingestion_settles_at = None;
        if let Some(settled) = self.status.settled() {
            self.status = settled;
        }
    }

    /// Project onto the modelled `EventDataStore` shape, as `ListEventDataStores`
    /// reports it.
    ///
    /// Only the ARN and the name: the model deprecates every other member of that
    /// shape with "<member> is no longer returned by ListEventDataStores", so a
    /// listing reports the two that identify a store and nothing else. The values a
    /// store is configured with are read through `GetEventDataStore`.
    pub fn to_list_entry(&self) -> EventDataStore {
        EventDataStore {
            event_data_store_arn: Some(self.arn()),
            name: Some(self.name.clone()),
            ..EventDataStore::default()
        }
    }

    /// Whether ingestion can be stopped or started on this store at all: it
    /// collects one of the [`INGESTION_CONTROLLABLE_EVENT_CATEGORIES`].
    ///
    /// Decided from the store's OWN advanced event selectors — the only place an
    /// event data store states which category of events it collects — so the
    /// answer is the same for every caller that addresses it.
    pub fn ingestion_is_controllable(&self) -> bool {
        INGESTION_CONTROLLABLE_EVENT_CATEGORIES
            .iter()
            .any(|category| self.logs_event_category(category))
    }

    /// The event categories this store collects, in the order its selectors state
    /// them — what a refusal about the store's category names.
    pub fn event_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for selector in &self.advanced_event_selectors {
            for field_selector in &selector.field_selectors {
                if field_selector.field != EVENT_CATEGORY_FIELD {
                    continue;
                }
                for value in field_selector.equals.iter().flatten() {
                    if !categories.contains(value) {
                        categories.push(value.clone());
                    }
                }
            }
        }
        categories
    }
}

/// A Lake query's lifecycle state: one variant per value of the model's
/// `QueryStatus` enum (`ops::types::QUERY_STATUS_VALUES`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    #[default]
    Queued,
    Running,
    Finished,
    Failed,
    Cancelled,
    TimedOut,
}

impl QueryState {
    /// The value this state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Finished => "FINISHED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::TimedOut => "TIMED_OUT",
        }
    }

    /// Whether the run has stopped moving: no later call changes this state.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

/// How long a Lake query reads a non-terminal `QueryStatus` before the clock
/// settles it into the terminal one.
///
/// The model's `QueryStatus` enum carries `QUEUED` and `RUNNING`, so a query IS
/// asynchronous: `StartQuery` answers with an id while the run is still queued.
/// CloudTrail publishes no duration for a run, so this span is the emulator's
/// own: short enough not to stall a workflow, non-zero so a caller that
/// describes the query straight after starting it still sees `QUEUED`.
pub const QUERY_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How many runs of one Region may be in flight at once, from the model doc for
/// `MaxConcurrentQueriesException`: "You are already running the maximum number
/// of concurrent queries. The maximum number of concurrent queries is 10."
pub const MAX_CONCURRENT_QUERIES: usize = 10;

/// How many query runs the registry keeps before `StartQuery` drops the oldest
/// finished ones.
///
/// Every `StartQuery` adds a record and no operation removes one, so the
/// registry would grow for the life of the process. CloudTrail publishes a
/// 7-day query history rather than a count, so this is the emulator's OWN bound
/// on the store, not a claim about the service: it sits far above any workflow
/// an emulator serves, and only runs that have already reached a terminal state
/// are dropped, oldest first, so no in-flight run is ever lost.
pub const QUERY_HISTORY_LIMIT: usize = 1_000;

/// The `DeliveryStatus` value a run whose results were asked for in S3 carries
/// while the delivery has not completed, from the model's `DeliveryStatus` enum
/// (`ops::types::DELIVERY_STATUS_VALUES`).
///
/// It is where a delivery this emulator accepted STAYS: writing the result file
/// would mean writing to an S3 bucket, which no part of this service does, so
/// reporting `SUCCESS` would claim a delivery that never happened.
pub const DELIVERY_STATUS_PENDING: &str = "PENDING";

/// Where a query's results were delivered.
///
/// The URI and the delivery status are one value because the model reports them
/// as a pair — "If the query results were delivered to an S3 bucket, the response
/// also provides the S3 URI and the delivery status" — so neither can be stored
/// without the other.
#[derive(Debug, Clone)]
pub struct QueryDelivery {
    pub s3_uri: String,
    /// One of the model's `DeliveryStatus` values
    /// (`ops::types::DELIVERY_STATUS_VALUES`).
    pub status: String,
}

/// What a query run accumulated, as `DescribeQuery` reports it.
#[derive(Debug, Default, Clone)]
pub struct QueryStatisticsRecord {
    pub events_matched: i64,
    pub events_scanned: i64,
    pub bytes_scanned: i64,
    /// `None` until the run ends: a run time exists only once there is a run to
    /// have taken it.
    pub execution_time_in_millis: Option<i32>,
}

/// One persisted Lake query — everything `StartQuery` was given plus what the run
/// produced, which is what `DescribeQuery` reports.
#[derive(Debug, Default, Clone)]
pub struct QueryRecord {
    pub query_id: String,
    /// The id of the event data store the query ran on.
    pub event_data_store_id: String,
    /// The Region the query ran in; it is reachable only from there.
    pub region: String,
    pub query_string: String,
    /// The query template alias this run was started under, if any.
    pub query_alias: Option<String>,
    /// The dashboard refresh this run belongs to, if any.
    pub refresh_id: Option<String>,
    /// The natural-language prompt a generated query came from, if any.
    pub prompt: Option<String>,
    pub status: QueryState,
    pub statistics: QueryStatisticsRecord,
    pub delivery: Option<QueryDelivery>,
    /// Every result row the run produced, in result order — the rows
    /// `GetQueryResults` pages through.
    pub result_rows: Vec<QueryResultRowRecord>,
    /// Why the run failed; set only by a run that ended in failure.
    pub error_message: Option<String>,
    /// The account owning the event data store, for a cross-account query.
    pub event_data_store_owner_account_id: Option<String>,
    /// When `StartQuery` minted the query: the `CreationTime` of the statistics
    /// `DescribeQuery` reports.
    pub created_at: Option<SystemTime>,
    /// When a non-terminal run is due to reach its terminal state; `None` once it
    /// has, and for a run the clock owes nothing.
    pub settles_at: Option<SystemTime>,
}

impl QueryRecord {
    /// The account that owns the event data store this query ran on: the one the
    /// run recorded, defaulting to the emulator's own account.
    pub fn owner(&self) -> &str {
        match self.event_data_store_owner_account_id.as_deref() {
            Some(account_id) => account_id,
            None => ::engine::emulator_account_id(),
        }
    }

    /// Whether a call made in `region` naming `event_data_store_id` (when it names
    /// one) addresses this query.
    pub fn addressed_by(&self, region: &str, event_data_store_id: Option<&str>) -> bool {
        self.region == region
            && event_data_store_id.is_none_or(|store| self.event_data_store_id == store)
    }

    /// The `page_size` result rows that follow `offset`, projected onto the
    /// modelled `QueryResultRows` shape: a list of rows, each a list of one-entry
    /// column maps, which is how a row keeps its column order on the wire.
    pub fn result_rows_page(
        &self,
        offset: usize,
        page_size: usize,
    ) -> Vec<Vec<::std::collections::HashMap<String, String>>> {
        self.result_rows
            .iter()
            .skip(offset)
            .take(page_size)
            .map(|row| {
                row.iter()
                    .map(|(column, value)| {
                        ::std::collections::HashMap::from([(column.clone(), value.clone())])
                    })
                    .collect()
            })
            .collect()
    }

    /// Materialise the transition the clock already owes this run: a run that is
    /// still `QUEUED` or `RUNNING` at [`Self::settles_at`] has ended, and ended
    /// `FINISHED` — the statement parsed and the store it reads resolved before
    /// the run was ever queued, so there is nothing left for it to fail on.
    ///
    /// A reporting operation may call this: the run ended at `settles_at` whether
    /// or not anybody looked, so applying it changes nothing a later caller would
    /// otherwise have seen.
    ///
    /// The statistics it leaves are the run's own: this emulator ingests no
    /// CloudTrail events, so the store the query read held none and the run
    /// scanned and matched nothing. The run time is the span the run took — from
    /// when it was queued to when it ended — and not the time until somebody
    /// asked about it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if now < due {
            return;
        }
        self.settles_at = None;
        if self.status.is_terminal() {
            return;
        }
        self.status = QueryState::Finished;
        self.statistics.execution_time_in_millis = self
            .created_at
            .and_then(|started| due.duration_since(started).ok())
            .map(|took| i32::try_from(took.as_millis()).unwrap_or(i32::MAX));
    }
}

/// Why a caller-supplied event data store identifier could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventDataStoreRejection {
    /// Readable as neither an event data store ARN nor the bare id suffix of one
    /// ⇒ `EventDataStoreARNInvalidException` (HTTP 400). Carries the value as the
    /// request gave it, which is what the refusal names.
    InvalidArn(String),
}

impl EventDataStoreRejection {
    /// The refusal text: built from the request, naming the ARN it addressed, so a
    /// caller can fix the request from it alone. See
    /// [`event_data_store_arn_invalid_message`] for the wording and why a bare id
    /// needs the Region the call was made in.
    pub fn message(self, request_region: &str) -> String {
        match self {
            Self::InvalidArn(value) => {
                event_data_store_arn_invalid_message(&value, request_region)
            }
        }
    }
}

/// A caller-supplied event data store identifier, in either of the two forms the
/// model documents: "The ARN (or the ID suffix of the ARN) of an event data store".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDataStoreIdentifier {
    /// The store id — the ARN's resource suffix.
    pub id: String,
    /// The Region the ARN pins the store to; `None` for a bare id, which names a
    /// store in the caller's own Region.
    pub region: Option<String>,
}

impl EventDataStoreIdentifier {
    /// The Region this identifier addresses a store in: the ARN's own Region, or
    /// the Region the call was made in for a bare id.
    pub fn addressed_region<'a>(&'a self, request_region: &'a str) -> &'a str {
        self.region.as_deref().unwrap_or(request_region)
    }

    /// The whole ARN this identifier addresses — the ARN as given, or the one a
    /// bare id completes to in the caller's Region.
    pub fn arn(&self, request_region: &str) -> String {
        event_data_store_arn(self.addressed_region(request_region), &self.id)
    }
}

/// The ARN of the event data store `id` in `region`.
pub fn event_data_store_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{EVENT_DATA_STORE_RESOURCE_PREFIX}{id}"),
    )
}

/// Read a caller-supplied event data store identifier as either an event data
/// store ARN or the bare id suffix of one.
///
/// A value carrying a `:` is read as an ARN — so a malformed ARN is refused as one
/// rather than taken for an id — and anything else as a bare id. Either way the id
/// has to BE one this service could have minted ([`is_event_data_store_id`]) when it
/// is written WITHOUT an ARN: the recorded answer to `UpdateEventDataStore` naming
/// `not-an-arn` is `EventDataStoreARNInvalidException` over the ARN that value
/// completes to, not a report that the store is absent. An ARN's resource suffix is
/// the store the ARN names, so for that form whether a store of that id exists is
/// the next question and not a question about the ARN's form.
pub fn parse_event_data_store_identifier(
    value: &str,
) -> Result<EventDataStoreIdentifier, EventDataStoreRejection> {
    let reject = || Err(EventDataStoreRejection::InvalidArn(value.to_string()));
    if !value.contains(':') {
        // The id suffix form, read by this service's ONE reading of it.
        let Some(id) = read_event_data_store_id_suffix(value) else {
            return reject();
        };
        return Ok(EventDataStoreIdentifier {
            id: id.to_string(),
            region: None,
        });
    }

    let Some((region, _account, resource)) = cloudtrail_arn_fields(value) else {
        return reject();
    };
    // The shared parse keeps every colon the resource field carries, because an ARN of
    // the `<type>:<id>` form is as well formed as one of the `<type>/<id>` form. An
    // event data store ARN is only ever the slash form ("the ARN (or the ID suffix of
    // the ARN) of an event data store", whose published format is
    // `…:eventdatastore/<id>`), so a further colon here is a seventh field this
    // identifier cannot carry.
    if resource.contains(':') {
        return reject();
    }
    match resource.strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX) {
        Some(id) if !id.is_empty() => Ok(EventDataStoreIdentifier {
            id: id.to_string(),
            region: Some(region.to_string()),
        }),
        _ => reject(),
    }
}

/// Read the BARE id form of an event data store identifier — "the ID suffix of the
/// ARN" — with or without the `eventdatastore/` resource prefix the whole ARN carries,
/// and only when the suffix is an id this service could have minted
/// ([`is_event_data_store_id`]). `None` is a bare value in no id form at all.
///
/// The ONE reading of that form: [`parse_event_data_store_identifier`] reads it for the
/// members documented as "the ARN (or the ID suffix of the ARN) of an event data store",
/// and the channel destination reader ([`crate::state::channels`]) reads it for a
/// `Destination$Location` — whose refusal class for a value in NO identifier form is its
/// own — so the resource prefix and the letter case cannot drift between the two.
pub fn read_event_data_store_id_suffix(value: &str) -> Option<&str> {
    let id = value
        .strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX)
        .unwrap_or(value);
    is_event_data_store_id(id).then_some(id)
}

/// Whether `value` is an id this service could have minted for an event data
/// store: the UUID that is the resource suffix of its ARN
/// (`eventdatastore/1a0abbc2-dfa0-46e6-885a-5ce09f2ee3ee`).
///
/// Only the lower-case hyphenated form counts, which is [`is_minted_uuid_id`] — this
/// service's ONE reading of that form, shared with every other resource whose id it
/// mints as a UUID, so two predicates cannot drift on letter case. `Uuid::try_parse`
/// also accepts the simple, braced and URN forms and upper-case hex, and none of
/// those is an id `CreateEventDataStore` ever handed out: the modelled `UUID` shape
/// every store id member targets is `len 36..36`, `pattern ^[a-f0-9\-]+$`
/// (`smithy_query(query="shape", shape="UUID")`).
pub fn is_event_data_store_id(value: &str) -> bool {
    is_minted_uuid_id(value)
}

/// The text an unreadable event data store identifier is refused with, worded the
/// way the real service words it: "The event data store ARN <arn> format is not
/// valid."
///
/// It names the ARN the request addressed rather than the characters it typed:
/// a value that is ARN-shaped is named as given, and a bare id is completed into
/// the ARN it names in the Region the call was made in — which is why the Region
/// is a parameter. Recorded CloudTrail responses carry both forms: an identifier
/// that is not a CloudTrail ARN is echoed whole, and a bare id that cannot be read
/// comes back as `arn:aws:cloudtrail:<request-region>:<account>:eventdatastore/<id>`.
pub fn event_data_store_arn_invalid_message(value: &str, request_region: &str) -> String {
    let addressed = if value.contains(':') {
        value.to_string()
    } else {
        let id = value
            .strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX)
            .unwrap_or(value);
        event_data_store_arn(request_region, id)
    };
    format!("The event data store ARN {addressed} format is not valid.")
}

// ---------------------------------------------------------------------------
// Event configuration (`PutEventConfiguration` / `GetEventConfiguration`)
// ---------------------------------------------------------------------------

/// Which resource an event-configuration call addresses.
///
/// `TrailName` and `EventDataStore` are both optional in the model and name
/// different resources, so the two operations that carry them —
/// `PutEventConfiguration` and `GetEventConfiguration` — read the pair the same
/// way: exactly one of them addresses the call's resource.
///
/// The Insights-selector pair (`PutInsightSelectors` / `GetInsightSelectors`)
/// carries the same pair under the same rule — "You cannot use this parameter
/// with the TrailName parameter" (model doc for
/// `PutInsightSelectorsRequest$EventDataStore`) — and reads it through here too,
/// so one statement of the rule serves every operation that addresses either a
/// trail or an event data store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventConfigurationTarget {
    /// The call addresses a trail, by name or by trail ARN.
    Trail(String),
    /// The call addresses an event data store, by ARN or by the id suffix of
    /// one.
    EventDataStore(String),
}

/// Why a request names no single event-configuration resource.
///
/// Both halves of the rule — naming neither resource and naming both — are the
/// modelled `InvalidParameterCombinationException` ("This exception is thrown
/// when the combination of parameters provided is not valid"), worded the way the
/// recorded response of the real service words it, so the rejection carries only
/// that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventConfigurationTargetRejection(pub String);

impl EventConfigurationTargetRejection {
    /// The refusal text: it states the rule the request broke.
    pub fn message(self) -> String {
        self.0
    }
}

/// The refusal text for a call that does not address exactly one resource, as the
/// recorded response of the real service words it.
pub const NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE: &str =
    "You must specify either a trail or an event data store.";

/// Read the resource an event-configuration call addresses from the `TrailName`
/// / `EventDataStore` pair.
pub fn event_configuration_target(
    trail_name: Option<&str>,
    event_data_store: Option<&str>,
) -> Result<EventConfigurationTarget, EventConfigurationTargetRejection> {
    match (trail_name, event_data_store) {
        (Some(name), None) => Ok(EventConfigurationTarget::Trail(name.to_string())),
        (None, Some(store)) => Ok(EventConfigurationTarget::EventDataStore(store.to_string())),
        // Neither resource leaves the call with nothing to address; both address
        // two resources of different kinds. Either way the pair cannot hold
        // together.
        (Some(_), Some(_)) | (None, None) => Err(EventConfigurationTargetRejection(
            NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE.to_string(),
        )),
    }
}

/// The refusal text for a `PutEventConfiguration` call that addresses an event
/// data store without stating the configuration to apply to it, as the recorded
/// response of the real service words it.
pub const MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE: &str =
    "You must specify the maximum event size and context key selectors.";

/// The refusal text for a `PutEventConfiguration` call that addresses a trail
/// without stating the configuration to apply to it. `AggregationConfigurations`
/// is the only trail-level setting this operation carries ("The list of
/// aggregation configurations that you want to configure for the trail").
pub const MISSING_TRAIL_CONFIGURATION_MESSAGE: &str =
    "You must specify the aggregation configurations.";

/// The CloudTrail Lake **channel** registry's own rules: how a channel identifier is
/// read and how a channel operation refuses one. They live in their own module beside
/// the registry they resolve against, the way `src/service/rpc_target.rs` holds the
/// target reading `src/service.rs` applies.
pub mod channels;

/// The rules an ADVANCED event selector set is held to — which fields each event
/// category admits, how many values `eventCategory` carries, the resource types a
/// `resources.type` condition may name — and the sentence each refusal carries. In its
/// own module for the same reason [`channels`] is: all three operations that store such
/// a set (`PutEventSelectors`, `CreateEventDataStore`, `UpdateEventDataStore`) apply it.
pub mod advanced_event_selectors;

/// The resources a trail REFERENCES — its destination bucket, its notification topic,
/// its encryption key and its CloudWatch Logs destination — resolved in the services
/// that own them, with the refusal text each resolution feeds. In its own module for the
/// same reason [`channels`] is: both operations that configure a trail apply it.
pub mod referenced_resources;

/// How a dashboard widget's `QueryStatement` is read, how many widgets one request may
/// carry, and the sentence a statement CloudTrail Lake cannot run is refused with. In
/// its own module beside the registry it guards, for the same reason [`channels`] is:
/// both dashboard operations apply it.
pub mod widget_query;

/// Per-service state for com-amazonaws-cloudtrail-cloudtrail-20131101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudtrailCloudtrail20131101State {
    /// How many dashboard refreshes this process has minted an id for;
    /// `StartDashboardRefresh` counts refresh ids off this.
    pub refreshes_started: u64,
    /// Every trail in the account, keyed by [`TrailKey`].
    pub trails: BTreeMap<TrailKey, TrailRecord>,
    /// Every import job in the account, keyed by [`ImportKey`]. `StartImport` adds a
    /// record; `StopImport` moves a running one to `STOPPED`.
    ///
    /// Bounded by [`IMPORT_QUOTA`], which `StartImport` — the only operation that
    /// adds a record — refuses past, on the terms [`TRAIL_QUOTA`] and
    /// [`DASHBOARD_QUOTA`] state.
    pub imports: BTreeMap<ImportKey, ImportRecord>,
    /// The failures each import recorded, in the order it recorded them — the order
    /// `ListImportFailures` reports and pages through — under the same [`ImportKey`]
    /// as the import itself.
    ///
    /// A one-to-many child collection rather than a `Vec` inside [`ImportRecord`]: an
    /// import's own members and the failures it recorded are read by different
    /// operations (`GetImport`/`ListImports`/`StopImport` against
    /// `ListImportFailures`), and keeping them apart means an import carries exactly
    /// one record of itself here while the page a listing serves is still computed
    /// from this one store.
    pub import_failures: BTreeMap<ImportKey, Vec<ImportFailureRecord>>,
    /// Every CloudTrail Lake event data store, keyed by its id (the resource
    /// suffix of its ARN), which is unique across Regions.
    pub event_data_stores: BTreeMap<String, EventDataStoreRecord>,
    /// Every Lake query run, keyed by its query id.
    pub queries: BTreeMap<String, QueryRecord>,
    /// Every CloudTrail Lake dashboard in the account, keyed by [`DashboardKey`].
    pub dashboards: BTreeMap<DashboardKey, DashboardRecord>,
    /// Every CloudTrail Lake channel in the account, keyed by [`channels::ChannelKey`].
    pub channels: BTreeMap<channels::ChannelKey, channels::ChannelRecord>,
    /// The resource-based policy attached to each resource that has one, keyed by
    /// the canonical ARN of that resource ([`PolicyResourceArn::canonical`]).
    ///
    /// Bounded by the resource registries above rather than by a quota of its own: a
    /// policy is only ever written for a resource that exists
    /// ([`ComAmazonawsCloudtrailCloudtrail20131101State::resolve_policy_resource`]),
    /// and the modelled `ResourcePolicy` length caps each document at 8192
    /// characters.
    ///
    /// An entry outlives nothing: a policy belongs to the resource it is attached to,
    /// so an operation that DELETES one of those resources drops that resource's entry
    /// here too — otherwise a resource later created with the same name in the same
    /// Region would inherit a policy nobody attached to it. `DeleteDashboard` does;
    /// `DeleteEventDataStore` and `DeleteChannel` own that for their registries.
    pub resource_policies: BTreeMap<String, String>,
}

/// What a reporting operation found for the identifier it was given: the registry
/// key the identifier addressed, and the trail stored under it, if any.
#[derive(Debug)]
pub struct AddressedTrail<'a> {
    /// The name and Region the identifier addressed — the ARN's own Region for a
    /// trail ARN, the request's Region for a bare name.
    pub key: TrailKey,
    /// The trail stored under [`Self::key`]; `None` is a well-formed identifier
    /// naming no trail, which is `TrailNotFoundException`.
    pub record: Option<&'a TrailRecord>,
}

impl AddressedTrail<'_> {
    /// The refusal text for an identifier that names no trail: the ARN the
    /// identifier resolved to, plus the account the lookup ran in
    /// ([`trail_not_found_arn_message`]).
    pub fn not_found_message(&self) -> String {
        let (name, region) = &self.key;
        trail_not_found_arn_message(region, name)
    }
}

/// What a mutating operation found for the identifier it was given.
///
/// The three failing variants are what keeps `TrailNotFoundException` (no such
/// trail anywhere) apart from `InvalidHomeRegionException` (the trail exists, but
/// not in the Region this call can drive it from).
pub enum TrailForMutation<'a> {
    /// The trail, homed in the caller's own Region.
    Found(&'a mut TrailRecord),
    /// The identifier pinned a Region other than the caller's — a trail ARN from
    /// another Region, which is how a shadow trail is addressed.
    AddressedInAnotherRegion { name: String, addressed: String },
    /// A trail of this name exists, but it is homed in another Region.
    HomedInAnotherRegion { name: String, home_region: String },
    /// No trail of this name in any Region. Carries the name the identifier
    /// resolved to, which the not-found refusal names as that trail's ARN (see
    /// [`trail_not_found_arn_message`]).
    Absent { name: String },
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The registry key a caller-supplied identifier addresses.
    ///
    /// An identifier names the trail of that name in the Region the identifier
    /// pins: the ARN's own Region, or — for a bare name — `request_region`, the
    /// Region the call was made in. That is why the status of a shadow trail (a
    /// multi-Region trail's replica in another Region) is reachable only by ARN.
    pub fn addressed_key(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<TrailKey, IdentifierRejection> {
        let parsed = parse_trail_identifier(identifier)?;
        Ok((
            parsed.name().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        ))
    }

    /// Resolve a caller-supplied identifier to a stored trail.
    ///
    /// `Err` is a malformed identifier (refuse the request), `Ok(None)` a
    /// well-formed one naming no trail (`TrailNotFoundException`).
    pub fn resolve(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<Option<&TrailRecord>, IdentifierRejection> {
        let key = self.addressed_key(identifier, request_region)?;
        Ok(self.trails.get(&key))
    }

    /// Resolve a caller-supplied identifier the way [`Self::resolve`] does, keeping
    /// the key it ADDRESSED beside the trail stored under it.
    ///
    /// A reporting operation whose not-found refusal names the trail by its resolved
    /// ARN needs both: the record when there is one, and the ARN the identifier
    /// resolved to when there is not
    /// ([`AddressedTrail::not_found_message`]).
    pub fn resolve_addressed(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<AddressedTrail<'_>, IdentifierRejection> {
        let key = self.addressed_key(identifier, request_region)?;
        let record = self.trails.get(&key);
        Ok(AddressedTrail { key, record })
    }

    /// Every Region holding a trail of this name, in Region order.
    pub fn home_regions_of(&self, name: &str) -> Vec<String> {
        self.trails
            .range((name.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == name)
            .map(|((_, region), _)| region.clone())
            .collect()
    }

    /// Resolve a caller-supplied identifier for an operation that CHANGES the
    /// trail, which runs only in the trail's home Region.
    pub fn resolve_for_mutation(
        &mut self,
        identifier: &str,
        request_region: &str,
    ) -> Result<TrailForMutation<'_>, IdentifierRejection> {
        let (name, addressed) = self.addressed_key(identifier, request_region)?;
        if addressed != request_region {
            return Ok(TrailForMutation::AddressedInAnotherRegion { name, addressed });
        }
        // Read where the name lives BEFORE taking the mutable borrow, so the
        // lookup below is the only one.
        let elsewhere = self.home_regions_of(&name);
        match self.trails.get_mut(&(name.clone(), addressed)) {
            Some(record) => Ok(TrailForMutation::Found(record)),
            None => Ok(match elsewhere.into_iter().next() {
                Some(home_region) => TrailForMutation::HomedInAnotherRegion { name, home_region },
                None => TrailForMutation::Absent { name },
            }),
        }
    }

    /// Materialise every change the CLOCK already owes the event-data-store
    /// registry: a store whose provisioning span has elapsed takes the status it was
    /// created for, one whose seven-day `PENDING_DELETION` wait has elapsed is gone,
    /// and the Lake queries that ran on a store THIS pass removed go with it — a
    /// query is reachable only through the store it ran on, so nothing can address
    /// those records again. Every event-data-store lifecycle operation calls this
    /// before it reads, so what it reports is what a caller would see even if nobody
    /// had called.
    pub fn settle_event_data_stores(&mut self, now: SystemTime) {
        // The stores this pass removes, and with them the queries that ran on them:
        // only those, so a query whose store the registry never held (a seeded one,
        // say) is left alone — the clock owes nothing about it.
        let removed: ::std::collections::HashSet<String> = self
            .event_data_stores
            .iter()
            .filter(|(_, record)| record.deletion_due(now))
            .map(|(id, _)| id.clone())
            .collect();
        self.event_data_stores
            .retain(|id, _| !removed.contains(id));
        for record in self.event_data_stores.values_mut() {
            record.settle(now);
            // Every transition the clock owes a store is materialised in ONE place,
            // so a precondition cannot read a stale federation or ingestion status.
            record.settle_federation(now);
            record.settle_ingestion(now);
        }
        // This is also the ONLY eviction path the query registry has: no operation
        // inserts into it yet (`StartQuery` is still a scaffold stub, with its own
        // filed issue), so a per-registry bound belongs with the handler that will.
        if !removed.is_empty() {
            self.queries
                .retain(|_, query| !removed.contains(&query.event_data_store_id));
        }
    }

    /// The import `import_id` names in `region`, for an operation that only reports
    /// it.
    pub fn import(&self, import_id: &str, region: &str) -> Option<&ImportRecord> {
        self.imports
            .get(&(import_id.to_string(), region.to_string()))
    }

    /// The import `import_id` names in `region`, for an operation that CHANGES it.
    ///
    /// `None` is a well-formed id naming no import the caller's Region holds, which
    /// every import operation answers with `ImportNotFoundException`.
    pub fn import_mut(&mut self, import_id: &str, region: &str) -> Option<&mut ImportRecord> {
        self.imports
            .get_mut(&(import_id.to_string(), region.to_string()))
    }

    /// The failures the import `import_id` names in `region` recorded, in the order
    /// it recorded them.
    ///
    /// An id naming no import of the caller's Region, and an import that recorded
    /// nothing, both hold no failures: `ListImportFailures` models no not-found error,
    /// so both are one empty page.
    pub fn import_failures(&self, import_id: &str, region: &str) -> &[ImportFailureRecord] {
        self.import_failures
            .get(&(import_id.to_string(), region.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The import still running in `region`, if there is one — what a NEW import
    /// is refused for ("This exception is thrown when you start a new import and a
    /// previous import is still in progress", the model doc for
    /// `AccountHasOngoingImportException`).
    ///
    /// Scoped to one Region because an import belongs to the Region it was created
    /// in ([`ImportKey`]): a job in another Region is not in progress here. Only
    /// `INITIALIZING` and `IN_PROGRESS` count, so a stopped or finished import
    /// blocks nothing.
    pub fn ongoing_import_in(&self, region: &str) -> Option<&ImportRecord> {
        self.imports
            .iter()
            .filter(|((_, import_region), _)| import_region.as_str() == region)
            .map(|(_, record)| record)
            .find(|record| record.is_running())
    }

    /// Resolve a caller-supplied event data store identifier (an ARN or the bare
    /// id suffix of one) to a stored event data store.
    ///
    /// `Err` is an unreadable identifier (refuse with
    /// `EventDataStoreARNInvalidException`), `Ok(None)` a readable one naming no
    /// store in the Region it addresses (`EventDataStoreNotFoundException`): a
    /// store is reachable only from the Region it lives in, whether it is named by
    /// a bare id (the caller's Region) or by an ARN (the ARN's Region).
    pub fn event_data_store(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<Option<&EventDataStoreRecord>, EventDataStoreRejection> {
        let parsed = parse_event_data_store_identifier(identifier)?;
        Ok(self.addressed_event_data_store(&parsed, request_region))
    }

    /// The event data store an already-read identifier addresses, if the registry
    /// holds it — the lookup half of [`Self::event_data_store`], for a caller that
    /// has read the identifier itself (to word a refusal with the ARN it
    /// addresses, say).
    pub fn addressed_event_data_store(
        &self,
        identifier: &EventDataStoreIdentifier,
        request_region: &str,
    ) -> Option<&EventDataStoreRecord> {
        let addressed = identifier.addressed_region(request_region);
        self.event_data_stores
            .get(&identifier.id)
            .filter(|record| record.region == addressed)
    }

    /// [`Self::addressed_event_data_store`] for a caller that CHANGES the store —
    /// or that materialises a transition the clock already owed it: its status move
    /// and its federation transition, each of which was due at the stored instant
    /// whether or not anybody called. The whole registry's owed changes are applied
    /// in one place, [`ComAmazonawsCloudtrailCloudtrail20131101State::settle_event_data_stores`].
    ///
    /// The addressing rule is the same one: a store is reachable only from the
    /// Region it lives in, whether it is named by a bare id or by an ARN.
    pub fn addressed_event_data_store_mut(
        &mut self,
        identifier: &EventDataStoreIdentifier,
        request_region: &str,
        now: SystemTime,
    ) -> Option<&mut EventDataStoreRecord> {
        let addressed = identifier.addressed_region(request_region).to_string();
        let record = self
            .event_data_stores
            .get_mut(&identifier.id)
            .filter(|record| record.region == addressed)?;
        record.settle(now);
        record.settle_federation(now);
        Some(record)
    }

    /// The query `query_id` names, as a call made in `request_region` and naming
    /// `event_data_store_id` (when it names one) addresses it.
    pub fn query(
        &self,
        query_id: &str,
        request_region: &str,
        event_data_store_id: Option<&str>,
    ) -> Option<&QueryRecord> {
        self.queries
            .get(query_id)
            .filter(|record| record.addressed_by(request_region, event_data_store_id))
    }

    /// The LAST query run for `alias` — what `DescribeQuery` reports when it is
    /// given a `QueryAlias` instead of a `QueryId` — narrowed to one dashboard
    /// refresh when `refresh_id` is given.
    ///
    /// "Last" is by creation time, so the newest run of the template wins.
    pub fn last_query_for_alias(
        &self,
        alias: &str,
        refresh_id: Option<&str>,
        request_region: &str,
        event_data_store_id: Option<&str>,
    ) -> Option<&QueryRecord> {
        self.queries
            .values()
            .filter(|record| record.addressed_by(request_region, event_data_store_id))
            .filter(|record| record.query_alias.as_deref() == Some(alias))
            .filter(|record| {
                refresh_id.is_none_or(|refresh| record.refresh_id.as_deref() == Some(refresh))
            })
            .max_by_key(|record| record.created_at)
    }

    /// Materialise every query transition the clock already owes (see
    /// [`QueryRecord::settle`]), so what the registry holds is what the runs in
    /// it have actually reached by `now`.
    pub fn settle_queries(&mut self, now: SystemTime) {
        for record in self.queries.values_mut() {
            record.settle(now);
        }
    }

    /// How many runs in `region` have not reached a terminal state — the
    /// concurrent queries [`MAX_CONCURRENT_QUERIES`] bounds.
    ///
    /// Call [`Self::settle_queries`] first: a run whose settle instant has passed
    /// is no longer in flight, whether or not anybody has looked at it.
    pub fn queries_in_flight(&self, region: &str) -> usize {
        self.queries
            .values()
            .filter(|record| record.region == region && !record.status.is_terminal())
            .count()
    }

    /// Hold the registry to `limit` records by dropping the oldest runs that have
    /// already reached a terminal state.
    ///
    /// A run still in flight is never dropped, so this can leave the registry
    /// above `limit` — [`MAX_CONCURRENT_QUERIES`] is what bounds those.
    pub fn trim_query_history(&mut self, limit: usize) {
        let over = self.queries.len().saturating_sub(limit);
        if over == 0 {
            return;
        }
        // Oldest first, so the runs dropped are the ones whose history is
        // furthest behind; a record carrying no creation time sorts oldest.
        let mut ended: Vec<(Option<SystemTime>, String)> = self
            .queries
            .values()
            .filter(|record| record.status.is_terminal())
            .map(|record| (record.created_at, record.query_id.clone()))
            .collect();
        ended.sort_unstable();
        for (_, query_id) in ended.into_iter().take(over) {
            self.queries.remove(&query_id);
        }
    }

    /// The query template `alias` names: the statement of a stored dashboard
    /// widget carrying that alias, in `region`.
    ///
    /// "The QueryAlias and QueryParameters parameters are used only for the
    /// queries that populate the CloudTrail Lake dashboards" (model doc for
    /// `StartQuery`), so a dashboard's widgets are where an alias resolves — a
    /// widget's `QueryAlias` names the CloudTrail-managed query it draws.
    pub fn query_template(&self, alias: &str, region: &str) -> Option<&str> {
        self.dashboards
            .values()
            .filter(|dashboard| dashboard.home_region == region)
            .flat_map(|dashboard| dashboard.widgets.iter())
            .find(|widget| widget.query_alias.as_deref() == Some(alias))
            .and_then(|widget| widget.query_statement.as_deref())
    }
}

// ---------------------------------------------------------------------------
// CloudTrail Lake dashboards
// ---------------------------------------------------------------------------

/// The resource prefix of a dashboard ARN:
/// `arn:aws:cloudtrail:<region>:<account>:dashboard/<name>`.
const DASHBOARD_RESOURCE_PREFIX: &str = "dashboard/";

/// Dashboard-name length bounds, from the `DashboardName` length constraint in the
/// model (`smithy_query(query="operation", operation="CreateDashboard")` reports
/// `Name` as `len 3..128, pattern ^[a-zA-Z0-9_\-]+$`).
pub const DASHBOARD_NAME_MIN_LEN: usize = 3;
/// See [`DASHBOARD_NAME_MIN_LEN`].
pub const DASHBOARD_NAME_MAX_LEN: usize = 128;

/// The one name that names the CloudTrail-managed Highlights dashboard, from the
/// model doc for `CreateDashboardRequest$Name`: "To create the Highlights dashboard,
/// the name must be AWSCloudTrail-Highlights."
pub const HIGHLIGHTS_DASHBOARD_NAME: &str = "AWSCloudTrail-Highlights";

/// `DashboardType` as the model enumerates it (`MANAGED | CUSTOM`): the dashboard a
/// caller composes out of its own widgets.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";
/// See [`DASHBOARD_TYPE_CUSTOM`]: the Highlights dashboard, which CloudTrail owns.
pub const DASHBOARD_TYPE_MANAGED: &str = "MANAGED";

/// `DashboardStatus` as the model enumerates it (`CREATING | CREATED | UPDATING |
/// UPDATED | DELETING`): a dashboard CloudTrail is still loading.
pub const DASHBOARD_STATUS_CREATING: &str = "CREATING";
/// See [`DASHBOARD_STATUS_CREATING`]: a dashboard that has finished loading.
pub const DASHBOARD_STATUS_CREATED: &str = "CREATED";
/// See [`DASHBOARD_STATUS_CREATING`]: a dashboard whose last write was an update.
///
/// It is the status a dashboard reports from the moment an update returns for it: a
/// read that follows a successful update observes the dashboard already updated, which
/// is what the recorded read straight after an update reports. The model's `UPDATING`
/// is the status CloudTrail reports while an update is still in flight, and an update
/// this service has answered is not — nothing here claims a write it already finished
/// is still running. `DELETING` likewise belongs to a dashboard on its way out, and
/// `DeleteDashboard` releases the record rather than holding it in that status.
pub const DASHBOARD_STATUS_UPDATED: &str = "UPDATED";

/// Whether a dashboard created without a `TerminationProtectionEnabled` member is
/// protected from deletion.
///
/// CloudTrail protects a new dashboard unless the caller opts out: a create that
/// states nothing is answered — and read back — with the protection ON, which is what
/// the recorded create of a dashboard carrying no such member reports. A request that
/// states the member keeps the value it stated, in either direction, and
/// `UpdateDashboard` is what changes it afterwards.
pub const DASHBOARD_TERMINATION_PROTECTION_DEFAULT: bool = true;

/// From the `CreateDashboard` model doc: "You can add up to 10 widgets to a custom
/// dashboard." The check both dashboard writers apply is
/// [`widget_query::widgets_beyond_dashboard_limit_message`].
pub const CUSTOM_DASHBOARD_WIDGET_LIMIT: usize = 10;

/// How many dashboards the registry holds before `CreateDashboard` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// CloudTrail publishes an adjustable per-account dashboard quota, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's limit.
/// Every record it holds is released by `DeleteDashboard`, and it sits far above any
/// workflow an emulator serves.
pub const DASHBOARD_QUOTA: usize = 1_000;

/// The largest page `ListDashboards` reports, which is also the page size a request
/// that names no `MaxResults` takes.
///
/// It is the modelled maximum of `ListDashboardsMaxResultsCount`
/// (`smithy_query(query="operation", operation="ListDashboards")` reports `MaxResults`
/// as `range 1..1000`), so a caller asking for more than the model admits is refused at
/// the boundary rather than here, and a caller asking for nothing gets the whole
/// Region's dashboards in one page unless the registry holds more than the model lets
/// one page carry.
pub const LIST_DASHBOARDS_MAX_PAGE_SIZE: usize = 1_000;

/// How long a new dashboard reads [`DASHBOARD_STATUS_CREATING`] before the clock
/// settles it into [`DASHBOARD_STATUS_CREATED`].
///
/// The model carries a `CREATING` status and a `ConflictException` documented as
/// "the specified resource is not ready for an operation … before CloudTrail has
/// time to fully load the resource", so a new dashboard IS loaded asynchronously.
/// CloudTrail publishes no duration for that load, so this span is the emulator's
/// own: short enough not to stall a workflow, non-zero so a caller that reads the
/// dashboard straight after creating it still sees the transient status. An UPDATE
/// owes no such span: a dashboard an update has returned for already reads
/// [`DASHBOARD_STATUS_UPDATED`] (see [`DashboardRecord::record_update`]).
pub const DASHBOARD_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// `RefreshScheduleFrequencyUnit` as the model enumerates it (`HOURS | DAYS`).
pub const REFRESH_FREQUENCY_UNIT_HOURS: &str = "HOURS";
/// See [`REFRESH_FREQUENCY_UNIT_HOURS`].
pub const REFRESH_FREQUENCY_UNIT_DAYS: &str = "DAYS";

/// From the model doc for `RefreshScheduleFrequency$Value`: "For custom dashboards,
/// the following values are valid when the unit is HOURS: 1, 6, 12, 24".
pub const REFRESH_FREQUENCY_HOURS_VALUES: [i32; 4] = [1, 6, 12, 24];

/// From the same doc: "For custom dashboards, the only valid value when the unit is
/// DAYS is 1."
pub const REFRESH_FREQUENCY_DAYS_VALUE: i32 = 1;

/// From the model doc for `CreateDashboardRequest$RefreshSchedule`: "To create the
/// Highlights dashboard, you must set a refresh schedule and set the Status to
/// ENABLED. The Unit for the refresh schedule must be HOURS and the Value must be 6."
pub const HIGHLIGHTS_REFRESH_FREQUENCY_VALUE: i32 = 6;

/// `RefreshScheduleStatus` as the model enumerates it (`ENABLED | DISABLED`).
pub const REFRESH_SCHEDULE_ENABLED: &str = "ENABLED";

/// From the model doc for `RefreshSchedule$TimeOfDay`: "The time of day in UTC to
/// run the schedule; for hourly only refer to minutes; default is 00:00."
pub const DEFAULT_REFRESH_TIME_OF_DAY: &str = "00:00";

/// The refusal text a dashboard operation answers with when the identifier it was
/// given names no stored dashboard.
///
/// It is a FIXED service message — CloudTrail reports no part of the request in it,
/// and it is the text the recorded `GetDashboard` miss carries — so every dashboard
/// operation answers a miss with the same words rather than wording its own.
pub const DASHBOARD_NOT_FOUND_MESSAGE: &str = "No matching dashboard found.";

/// The refusal text a dashboard operation answers with when the dashboard it
/// addressed has termination protection enabled ("You cannot delete a dashboard that
/// has termination protection enabled").
///
/// It is a FIXED service message — it carries no dashboard name, ARN, account or
/// Region, and it is the text the recorded refusal of a delete against a protected
/// dashboard carries — so a dashboard operation blocked by that protection answers
/// with these words rather than wording its own.
pub const DASHBOARD_TERMINATION_PROTECTED_MESSAGE: &str =
    "This dashboard has termination protection enabled.";

/// The HTTP status this service answers a dashboard `ConflictException` with.
///
/// The model annotates the shape `@httpError(409)`
/// (`smithy_query(query="shape", shape="ConflictException")`), but `@httpError` is an
/// HTTP-BINDING trait and the recorded response carrying `__type: ConflictException` —
/// a second create of a dashboard name already taken,
/// `cloudtrail-dashboard-error-paths#task#2` — carries HTTP **400**: the status a
/// client's retry classifier sees is the service's, not the model's, and a service
/// renders one error shape one way. It is answered through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line — the same deviation
/// [`TRAIL_NOT_FOUND_STATUS`] and [`IMPORT_NOT_FOUND_HTTP_STATUS`] already carry for
/// this service. Stated here rather than in one handler so every dashboard operation
/// that refuses with `ConflictException` answers at the same status.
pub const DASHBOARD_CONFLICT_HTTP_STATUS: u16 = 400;

/// The registry key: a dashboard name and the Region the dashboard lives in.
///
/// A dashboard ARN carries a Region, so a dashboard is a regional resource and its
/// name alone is not a key. The name comes FIRST so the `BTreeMap` iterates
/// name-ordered, which is the order `ListDashboards` pages through.
pub type DashboardKey = (String, String);

/// One persisted CloudTrail Lake dashboard: everything the create/update request was
/// given, plus the values the service mints (the ARN, the type, the status and the
/// timestamps).
#[derive(Debug, Clone)]
pub struct DashboardRecord {
    pub name: String,
    pub dashboard_arn: String,
    /// [`DASHBOARD_TYPE_CUSTOM`] or [`DASHBOARD_TYPE_MANAGED`].
    pub dashboard_type: String,
    /// A `DashboardStatus` value; see [`DashboardRecord::settle`].
    pub status: String,
    /// When `CREATING` is due to settle into `CREATED`; see
    /// [`DashboardRecord::settle`].
    pub settles_at: SystemTime,
    pub widgets: Vec<Widget>,
    pub refresh_schedule: Option<RefreshSchedule>,
    pub termination_protection_enabled: bool,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub home_region: String,
    /// The id of the refresh `StartDashboardRefresh` last began for this dashboard,
    /// and `None` until one has been started.
    pub last_refresh_id: Option<String>,
    /// Why the dashboard's last refresh failed. Nothing sets it: a refresh is started
    /// and recorded, but no Lake query runs for it (`StartQuery` is still a scaffold),
    /// so no refresh of this emulator's own fails.
    pub last_refresh_failure_reason: Option<String>,
}

impl DashboardRecord {
    /// Materialise the `CREATING` -> `CREATED` transition the clock already owes.
    ///
    /// A reporting operation may call this: the transition was due at `settles_at`
    /// whether or not anybody read the dashboard, so applying it changes nothing a
    /// later caller would otherwise have seen. A dashboard whose last write was an
    /// update is owed nothing — that write settled its own status (see
    /// [`Self::record_update`]) — so this leaves it where it stands.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == DASHBOARD_STATUS_CREATING && now >= self.settles_at {
            self.status = DASHBOARD_STATUS_CREATED.to_string();
        }
    }

    /// Record that this dashboard has just been updated: it was updated now, and the
    /// status it reports is [`DASHBOARD_STATUS_UPDATED`].
    ///
    /// The updated status is reported from the moment the update returns — a read
    /// that follows a successful update observes the dashboard already updated, which
    /// is what the recorded read straight after an update reports — so an update owes
    /// the clock nothing and leaves no transition behind for [`Self::settle`].
    ///
    /// Only a call the service documents as updating the dashboard may do this: it is
    /// the write half of the same status machine [`Self::settle`] reads, so the
    /// dashboard's configuration writer and its readers cannot drift.
    pub fn record_update(&mut self, now: SystemTime) {
        self.updated_at = now;
        self.status = DASHBOARD_STATUS_UPDATED.to_string();
    }

    /// Whether this is the CloudTrail-managed Highlights dashboard.
    pub fn is_managed(&self) -> bool {
        self.dashboard_type == DASHBOARD_TYPE_MANAGED
    }
}

/// A caller-supplied dashboard identifier, in either of the two forms the model
/// documents for `GetDashboardRequest$DashboardId` ("The name or ARN for the
/// dashboard").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardIdentifier {
    /// A bare dashboard name, which names a dashboard in the caller's own Region.
    Name(String),
    /// A dashboard ARN, which also names the Region the dashboard lives in.
    Arn { region: String, name: String },
}

impl DashboardIdentifier {
    /// The dashboard name this identifier names — the registry's key.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Arn { name, .. } => name,
        }
    }

    /// The Region this identifier pins the dashboard to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }
}

/// The ARN of the dashboard called `name` in `region`.
pub fn dashboard_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{DASHBOARD_RESOURCE_PREFIX}{name}"),
    )
}

/// Whether `name` satisfies the modelled `DashboardName` constraints: between
/// [`DASHBOARD_NAME_MIN_LEN`] and [`DASHBOARD_NAME_MAX_LEN`] characters of
/// `^[a-zA-Z0-9_\-]+$`.
pub fn is_valid_dashboard_name(name: &str) -> bool {
    let length = name.chars().count();
    (DASHBOARD_NAME_MIN_LEN..=DASHBOARD_NAME_MAX_LEN).contains(&length)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Read a caller-supplied dashboard identifier as either a dashboard ARN or a bare
/// dashboard name. `None` is an identifier that can name no dashboard at all.
///
/// A value that starts with `arn:` is read as an ARN — the modelled `DashboardArn`
/// pattern admits both forms — and anything else as a bare name.
pub fn parse_dashboard_identifier(value: &str) -> Option<DashboardIdentifier> {
    if value.starts_with("arn:") {
        let (region, _account, resource) = cloudtrail_arn_fields(value)?;
        let name = resource.strip_prefix(DASHBOARD_RESOURCE_PREFIX)?;
        if !is_valid_dashboard_name(name) {
            return None;
        }
        return Some(DashboardIdentifier::Arn {
            region: region.to_string(),
            name: name.to_string(),
        });
    }
    if !is_valid_dashboard_name(value) {
        return None;
    }
    Some(DashboardIdentifier::Name(value.to_string()))
}

/// Check a caller-supplied refresh schedule against the frequencies the model
/// documents, and hand back the schedule as it will be STORED — which is the request's
/// own schedule with `TimeOfDay` defaulted to [`DEFAULT_REFRESH_TIME_OF_DAY`].
///
/// `Err` carries the refusal text, built from the request so a caller can fix it from
/// the message alone. Every dashboard operation that takes a `RefreshSchedule` applies
/// this, so the create and the update paths cannot drift.
pub fn normalize_refresh_schedule(schedule: &RefreshSchedule) -> Result<RefreshSchedule, String> {
    let time_of_day = schedule
        .time_of_day
        .clone()
        .unwrap_or_else(|| DEFAULT_REFRESH_TIME_OF_DAY.to_string());
    // The modelled pattern `^[0-9]{2}:[0-9]{2}$` fixes the shape; what it cannot say
    // is that the two fields are a UTC time of day.
    let (hours, minutes) = time_of_day
        .split_once(':')
        .ok_or_else(|| time_of_day_rejection(&time_of_day))?;
    let valid_time = match (hours.parse::<u32>(), minutes.parse::<u32>()) {
        (Ok(hours), Ok(minutes)) => hours < 24 && minutes < 60,
        _ => false,
    };
    if !valid_time {
        return Err(time_of_day_rejection(&time_of_day));
    }

    if let Some(frequency) = schedule.frequency.as_ref() {
        let (Some(unit), Some(value)) = (frequency.unit.as_deref(), frequency.value) else {
            return Err(
                "RefreshSchedule.Frequency must carry both Unit and Value: a refresh \
                 frequency is a unit and a value together"
                    .to_string(),
            );
        };
        let valid_value = match unit {
            REFRESH_FREQUENCY_UNIT_HOURS => REFRESH_FREQUENCY_HOURS_VALUES.contains(&value),
            REFRESH_FREQUENCY_UNIT_DAYS => value == REFRESH_FREQUENCY_DAYS_VALUE,
            // `RefreshScheduleFrequencyUnit` is a modelled enum, so the boundary
            // validator has already refused anything else.
            _ => false,
        };
        if !valid_value {
            return Err(format!(
                "RefreshSchedule.Frequency.Value {value} is not valid for Unit {unit}: the \
                 valid values are {:?} for {REFRESH_FREQUENCY_UNIT_HOURS} and \
                 {REFRESH_FREQUENCY_DAYS_VALUE} for {REFRESH_FREQUENCY_UNIT_DAYS}",
                REFRESH_FREQUENCY_HOURS_VALUES
            ));
        }
    }

    Ok(RefreshSchedule {
        frequency: schedule.frequency.clone(),
        status: schedule.status.clone(),
        time_of_day: Some(time_of_day),
    })
}

/// The refusal text for a `TimeOfDay` that is not a UTC time of day.
fn time_of_day_rejection(time_of_day: &str) -> String {
    format!(
        "RefreshSchedule.TimeOfDay '{time_of_day}' is not valid: the time of day is UTC \
         HH:MM, with HH between 00 and 23 and MM between 00 and 59"
    )
}

/// Whether `schedule` is the one the Highlights dashboard requires: enabled, and
/// refreshing every [`HIGHLIGHTS_REFRESH_FREQUENCY_VALUE`] hours.
pub fn is_highlights_refresh_schedule(schedule: &RefreshSchedule) -> bool {
    let enabled = schedule.status.as_deref() == Some(REFRESH_SCHEDULE_ENABLED);
    let frequency = schedule.frequency.as_ref().is_some_and(|frequency| {
        frequency.unit.as_deref() == Some(REFRESH_FREQUENCY_UNIT_HOURS)
            && frequency.value == Some(HIGHLIGHTS_REFRESH_FREQUENCY_VALUE)
    });
    enabled && frequency
}

/// Project a requested widget onto the modelled `Widget` shape, as a dashboard
/// reports it back.
///
/// `QueryAlias` names a CloudTrail-managed query; a widget the caller composed
/// carries its own `QueryStatement` instead, so no alias is minted for it.
pub fn request_widget_to_widget(widget: RequestWidget) -> Widget {
    Widget {
        query_alias: None,
        query_statement: Some(widget.query_statement),
        query_parameters: widget.query_parameters,
        view_properties: Some(widget.view_properties),
    }
}

// ---------------------------------------------------------------------------
// Pagination: the one page token this service's listings issue
// ---------------------------------------------------------------------------

/// The page token a paged listing hands back, and how it reads one again.
///
/// Stated here rather than per operation, for the reason this module exists:
/// `InvalidNextTokenException` is documented as "a token that is not valid, or a
/// token that was previously used in a request with different parameters", and a
/// listing applies that rule to the same two parameters — the page size the token was
/// issued for and the Region it was issued in — so a second copy of it would drift
/// from the first. Each operation keeps only what is its own: the shape of a resume
/// key it could have minted, and the modelled error it refuses with.
///
/// `ListChannels` and `ListEventDataStores` read their tokens through here. The other
/// listings of this service still carry a token format of their own
/// (`ops::list_imports`, `ops::list_trails`, `ops::list_dashboards`), each shaped by
/// the filters it pages with; folding those in belongs with whichever of those
/// operations is worked next, and this is the reader they fold into.
pub mod page_token {
    /// The separator between the token's fields. Neither a Region, a page size, nor
    /// an id this service mints carries one, so the split is unambiguous.
    const FIELD_SEPARATOR: char = '|';

    /// The token for resuming at `resume_at`: the hex of
    /// `<page size>|<region>|<resume key>`.
    ///
    /// Hex-coded because a `PaginationToken` is opaque to the caller — it is only
    /// ever handed straight back — and because the coding keeps the token inside the
    /// member's modelled `@length 4..1000`: the shortest payload is already 4 bytes,
    /// and the longest (a resource id, a Region, at most four digits of page size and
    /// the separators) codes to well under 1000 characters.
    pub fn encode(page_size: usize, region: &str, resume_at: &str) -> String {
        ::hex::encode(format!(
            "{page_size}{FIELD_SEPARATOR}{region}{FIELD_SEPARATOR}{resume_at}"
        ))
    }

    /// The resume key a token that `operation` issued for THIS walk carries, where
    /// `is_minted_key` is what that operation's resume keys look like.
    ///
    /// `Err` is the refusal text for the caller's own
    /// `InvalidNextTokenException`, naming the way the token does not belong to this
    /// walk, so a caller can fix the request from it alone.
    pub fn decode(
        token: &str,
        page_size: usize,
        region: &str,
        operation: &str,
        is_minted_key: impl Fn(&str) -> bool,
    ) -> Result<String, String> {
        let invalid = |reason: String| format!("Invalid NextToken '{token}': {reason}");
        let unissued = || invalid(format!("it was not issued by {operation}"));

        let decoded = ::hex::decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(unissued)?;
        let (issued_for, rest) = decoded.split_once(FIELD_SEPARATOR).ok_or_else(unissued)?;
        let (issued_in, resume_at) = rest.split_once(FIELD_SEPARATOR).ok_or_else(unissued)?;
        if issued_for.parse::<usize>() != Ok(page_size) {
            return Err(invalid(format!(
                "it was issued for a different MaxResults than the {page_size} this request \
                 asks for"
            )));
        }
        if issued_in != region {
            return Err(invalid(format!(
                "it was issued for another Region than {region}"
            )));
        }
        // Every token a listing issues resumes at a key the service minted, so
        // anything else is a position no page of this walk ever stopped at. A
        // resource released since the page was issued is NOT refused here: the walk
        // resumes where it stopped whether or not that resource is still stored.
        if !is_minted_key(resume_at) {
            return Err(unissued());
        }
        Ok(resume_at.to_string())
    }
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The registry key a caller-supplied dashboard identifier addresses: the
    /// dashboard of that name in the Region the identifier pins — the ARN's own
    /// Region, or, for a bare name, the Region the call was made in.
    ///
    /// `None` is an identifier that can name no dashboard at all.
    pub fn addressed_dashboard_key(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Option<DashboardKey> {
        let parsed = parse_dashboard_identifier(identifier)?;
        Some((
            parsed.name().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        ))
    }

    /// Resolve a caller-supplied dashboard identifier to the stored dashboard,
    /// materialising the transition its clock already owes.
    pub fn resolve_dashboard_mut(
        &mut self,
        identifier: &str,
        request_region: &str,
        now: SystemTime,
    ) -> Option<&mut DashboardRecord> {
        let key = self.addressed_dashboard_key(identifier, request_region)?;
        let record = self.dashboards.get_mut(&key)?;
        record.settle(now);
        Some(record)
    }

    /// Whether a store in `region` other than `except` already carries `name`.
    ///
    /// An event data store name is unique per Region, which is what
    /// `EventDataStoreAlreadyExistsException` reports; `except` is the id of the
    /// store being renamed, which never conflicts with itself.
    pub fn event_data_store_name_taken(
        &self,
        name: &str,
        region: &str,
        except: Option<&str>,
    ) -> bool {
        self.event_data_stores.values().any(|record| {
            record.name == name
                && record.region == region
                && except.is_none_or(|id| record.id != id)
        })
    }
}

// ---------------------------------------------------------------------------
// Pagination: how many event data stores one listing page carries
// ---------------------------------------------------------------------------

/// How many event data stores one `ListEventDataStores` page carries when the
/// request names no `MaxResults`.
///
/// The model bounds `MaxResults` at 1..1000 but publishes no default, so the default
/// page size is the service's to choose; the remainder is reached through
/// `NextToken`.
pub const LIST_EVENT_DATA_STORES_PAGE_SIZE: usize = 50;

/// The largest `ListEventDataStores` page the model admits, from the `@range 1..1000`
/// on `ListEventDataStoresMaxResultsCount`. The boundary refusal already answers a
/// request outside that bound with the modelled `InvalidMaxResultsException`; the
/// handler clamps to it so the walk stays bounded whatever reaches it.
pub const LIST_EVENT_DATA_STORES_MAX_PAGE_SIZE: usize = 1_000;

/// The smallest page that bound admits — the other end of the same `@range`. A page
/// of zero would carry nothing and still mint a token, so this is the floor the
/// handler clamps to.
pub const LIST_EVENT_DATA_STORES_MIN_PAGE_SIZE: usize = 1;

// ---------------------------------------------------------------------------
// Resource-based policies
// ---------------------------------------------------------------------------

/// The resource prefix of a channel ARN:
/// `arn:aws:cloudtrail:<region>:<account>:channel/<channel-id>`, as the model doc for
/// `ResourceArn` and for `AddTagsRequest$ResourceId` both give it ("Example channel
/// ARN format: arn:aws:cloudtrail:us-east-2:123456789012:channel/01234567890"). The
/// event data store and dashboard prefixes the same docs give are already defined
/// above, with the registries they address.
const CHANNEL_RESOURCE_PREFIX: &str = "channel/";

/// What kind of CloudTrail resource an ARN names.
///
/// The three kinds a resource-based policy attaches to are the ones the model doc
/// for `ResourceArn` names: "the CloudTrail event data store, dashboard, or channel
/// attached to the resource-based policy". [`ResourceKind::Trail`] is here because a
/// trail ARN is a well-formed CloudTrail resource ARN that carries no resource-based
/// policy, and that is a different refusal from an ARN that is not well formed at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    EventDataStore,
    Dashboard,
    Channel,
    Trail,
}

impl ResourceKind {
    /// Every kind, so a caller can walk them rather than restate the list.
    pub const ALL: [Self; 4] = [
        Self::EventDataStore,
        Self::Dashboard,
        Self::Channel,
        Self::Trail,
    ];

    /// The `<type>/` prefix this kind's ARN carries in its resource segment.
    pub fn resource_prefix(self) -> &'static str {
        match self {
            Self::EventDataStore => EVENT_DATA_STORE_RESOURCE_PREFIX,
            Self::Dashboard => DASHBOARD_RESOURCE_PREFIX,
            Self::Channel => CHANNEL_RESOURCE_PREFIX,
            Self::Trail => TRAIL_RESOURCE_PREFIX,
        }
    }

    /// The resource type as an ARN spells it, without the trailing separator.
    pub fn resource_type(self) -> &'static str {
        self.resource_prefix().trim_end_matches('/')
    }

    /// The CloudFormation-style type name CloudTrail reports for this kind of resource in
    /// an event's `Resources`, which a lookup by `ResourceType` filters on.
    pub fn audited_resource_type(self) -> &'static str {
        match self {
            Self::Trail => TRAIL_RESOURCE_TYPE,
            Self::EventDataStore => "AWS::CloudTrail::EventDataStore",
            Self::Dashboard => "AWS::CloudTrail::Dashboard",
            Self::Channel => "AWS::CloudTrail::Channel",
        }
    }

    /// How a refusal names this kind of resource in prose.
    ///
    /// CloudTrail's resource-policy surface words a resource it could not find per
    /// resource kind, and the recorded answers word two of them differently: an event
    /// data store's and a channel's sentence name the ARN they were given ("The event
    /// data store <arn> was not found."), a dashboard's names no identifier at all
    /// ("The dashboard was not found."). [`ResourceKind::not_found_message`] is where
    /// that split lives. It is NOT
    /// [`EVENT_DATA_STORE_NOT_FOUND_MESSAGE`]: that is the text of a different
    /// exception on a different surface (`EventDataStoreNotFoundException`, which the
    /// Lake query operations answer), which states no identifier.
    pub fn described(self) -> &'static str {
        match self {
            Self::EventDataStore => "event data store",
            Self::Dashboard => "dashboard",
            Self::Channel => "channel",
            Self::Trail => "trail",
        }
    }

    /// The refusal text for a resource of this kind that `value` named and that does
    /// not exist.
    ///
    /// See [`ResourceKind::described`]: a dashboard's sentence carries no identifier,
    /// every other kind's names the ARN the request supplied. An event data store's
    /// and a channel's are the ones this crate already states for those kinds
    /// ([`event_data_store_not_found_message`],
    /// [`channels::channel_not_found_message`]), so the surfaces cannot drift.
    /// The dashboard's is NOT [`DASHBOARD_NOT_FOUND_MESSAGE`]: the dashboard
    /// operations were recorded answering "No matching dashboard found." and this
    /// surface "The dashboard was not found.", so the two are deliberately not one
    /// sentence.
    pub fn not_found_message(self, value: &str) -> String {
        match self {
            Self::EventDataStore => event_data_store_not_found_message(value),
            Self::Channel => channels::channel_not_found_message(value),
            Self::Dashboard => format!("The {} was not found.", self.described()),
            // Spelled out rather than `_`, so a resource kind added later is a
            // compile error here rather than silently taking this wording.
            Self::Trail => format!("The {} {value} was not found.", self.described()),
        }
    }

    /// Whether a resource-based policy can be attached to this kind of resource:
    /// true for the three kinds the `ResourceArn` doc names, false for every other
    /// CloudTrail resource — which is what `ResourceTypeNotSupportedException`
    /// ("the specified resource type is not supported by CloudTrail") refuses.
    pub fn takes_resource_policy(self) -> bool {
        match self {
            Self::EventDataStore | Self::Dashboard | Self::Channel => true,
            Self::Trail => false,
        }
    }
}

/// A CloudTrail resource ARN, as a resource-policy operation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResourceArn {
    pub kind: ResourceKind,
    /// The Region the ARN pins the resource to.
    pub region: String,
    /// The resource segment after the `<type>/` prefix: a dashboard's name, an event
    /// data store's or a channel's id.
    pub id: String,
}

impl PolicyResourceArn {
    /// This resource's canonical ARN — the one the emulator mints for it, which is
    /// the key its policy is stored under.
    ///
    /// Reading an ARN back out canonically is what keeps two spellings of one
    /// resource (another partition, or a caller quoting a real account id) from
    /// addressing two different policies, exactly as a dashboard lookup keys on the
    /// name and the Region rather than on the ARN's account segment.
    pub fn canonical(&self) -> String {
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            &self.region,
            &format!("{}{}", self.kind.resource_prefix(), self.id),
        )
    }
}

/// Read `arn:<partition>:cloudtrail:<region>:<account>:<type>/<id>`, the ARN form the
/// model documents for every resource a resource-based policy can name.
///
/// The six-field split is NOT re-derived here: it is
/// [`service_common::arn::parse_of`], the one caller-supplied-ARN parse, narrowed to
/// this service's vendor. What this layers on top of it is CloudTrail's own: a
/// resource is homed in a Region and an account (the shared parse admits the
/// partition-global and account-less forms AWS writes for other services, which no
/// CloudTrail resource has), and the resource field names one of this service's
/// resource kinds with a non-empty id.
///
/// `None` is a value that is not a CloudTrail resource ARN at all: a bare name, an
/// ARN of another service, one missing a field, or one whose resource segment names
/// no CloudTrail resource type.
///
/// SHARES ITS SUBJECT WITH [`parse_taggable_arn`], which classifies the same four
/// CloudTrail resource kinds for the TAGGING surface. The two are kept apart because
/// what each surface does with a kind differs beyond the wording: a tagging ARN has to
/// satisfy the addressed resource's own naming rules before it can be refused as the
/// right kind of invalid ARN (`ChannelARNInvalidException` versus
/// `CloudTrailARNInvalidException`), while a policy ARN is classified only to pick
/// between `ResourceARNNotValidException`, `ResourceTypeNotSupportedException` and
/// `ResourceNotFoundException` and is then resolved through the registries. Both read
/// the ARN itself through [`::service_common::arn`], so neither re-derives the split;
/// a change to WHICH resource types this service has belongs in both, and the fix that
/// merges them states the shared classification once and layers each surface's
/// rejection mapping on top.
pub fn parse_resource_arn(value: &str) -> Option<PolicyResourceArn> {
    let parsed = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
    if parsed.region.is_empty() || parsed.account.is_empty() {
        return None;
    }
    let kind = ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.resource_type() == parsed.resource_type())?;
    // The shared reader takes the id out of either form AWS writes a resource in
    // (`<type>/<id>` and `<type>:<id>`); CloudTrail writes the slash form, which is
    // the one the canonical ARN this resolves to is built in.
    let id = ::service_common::arn::resource_id_of_type(value, kind.resource_type())?;
    if id.is_empty() {
        return None;
    }
    Some(PolicyResourceArn {
        kind,
        region: parsed.region.to_string(),
        id: id.to_string(),
    })
}

/// Why a resource-policy operation could not reach the resource its `ResourceArn`
/// names.
///
/// Each variant names the modelled error every resource-policy operation answers it
/// with, so the three operations cannot drift in how they refuse one ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyResourceRejection {
    /// Not a well-formed CloudTrail resource ARN ⇒ `ResourceARNNotValidException`
    /// (HTTP 400).
    ArnNotValid(String),
    /// A well-formed CloudTrail ARN naming a resource type that carries no
    /// resource-based policy ⇒ `ResourceTypeNotSupportedException` (HTTP 400).
    TypeNotSupported(String),
    /// A well-formed ARN of a supported type that names no stored resource ⇒
    /// `ResourceNotFoundException` (HTTP 400).
    ResourceNotFound(String),
}

impl PolicyResourceRejection {
    /// The refusal text: built from the request, naming the value and the rule it
    /// broke, so a caller can fix the request from it alone.
    ///
    /// Borrowed, so a handler reads the text and matches on the variant without
    /// cloning the rejection.
    pub fn message(&self) -> &str {
        match self {
            Self::ArnNotValid(message)
            | Self::TypeNotSupported(message)
            | Self::ResourceNotFound(message) => message,
        }
    }
}

/// The refusal text for `ResourcePolicyNotFoundException`: a resource that exists
/// and carries no resource-based policy.
///
/// A FIXED service sentence — the recorded refusal names neither the resource nor
/// anything else from the request — and the one sentence every operation that looks a
/// policy up answers with, the read that reports one and the delete that detaches one
/// alike, so the two cannot word the same miss differently.
pub const RESOURCE_POLICY_NOT_FOUND_MESSAGE: &str =
    "There is no policy attached to the given resource.";

/// The refusal text for `ConflictException`: a resource another operation is
/// modifying, which no resource-policy WRITE can change.
///
/// From the model doc for the error — "the specified resource is not ready for an
/// operation … If this exception occurs, wait a few minutes, and then try the
/// operation again" — and shared by every resource-policy write for that reason.
pub fn policy_resource_not_ready_message(resource_arn: &str) -> String {
    format!(
        "Resource {resource_arn} is not ready for this operation; wait a few minutes and \
         then try the operation again"
    )
}

/// The resource a resource-policy operation addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResource {
    /// The canonical ARN of the resource, which is the registry key its policy is
    /// stored under and the ARN the operation reports back.
    pub arn: String,
    /// Whether the resource can take a policy WRITE. A resource another operation is
    /// modifying — an event data store on its way out — is what `ConflictException`
    /// ("the specified resource is not ready for an operation … because another
    /// operation is modifying the resource") refuses a write to; a read reports its
    /// policy regardless. A resource whose create has returned is ready.
    pub ready: bool,
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// Resolve a caller-supplied `ResourceArn` to the resource whose policy it
    /// addresses, materialising any transition that resource's clock already owes.
    ///
    /// The three refusals are ordered the way the ARN is read: an ARN that is not a
    /// CloudTrail resource ARN cannot name a resource type, and a resource type that
    /// carries no policy cannot be looked up.
    pub fn resolve_policy_resource(
        &mut self,
        value: &str,
        now: SystemTime,
    ) -> Result<PolicyResource, PolicyResourceRejection> {
        let parsed = parse_resource_arn(value).ok_or_else(|| {
            // The recorded refusal names the value and nothing else: "The format of
            // the resource ARN <value> is not valid." — built from the request, so a
            // caller sees which value it was.
            PolicyResourceRejection::ArnNotValid(format!(
                "The format of the resource ARN {value} is not valid."
            ))
        })?;
        if !parsed.kind.takes_resource_policy() {
            // The refusal names the resource type the ARN carried, in the sentence
            // the service answers an unsupported type with — built from the request,
            // so a caller sees which type it addressed.
            return Err(PolicyResourceRejection::TypeNotSupported(format!(
                "Resource policies are not supported for the resource: {}.",
                parsed.kind.resource_type()
            )));
        }

        let ready = match parsed.kind {
            // Through the dashboard registry's own reader, so the key shape and the
            // transition the clock owes are stated once. The ARN's own Region is the
            // Region the identifier is read in, and its resource id is the
            // dashboard's name.
            //
            // A stored dashboard is READY, whatever `DashboardStatus` it reports: a
            // dashboard `CreateDashboard` has returned takes a resource-based policy
            // straight away, which is what the recorded attach made against a
            // just-created dashboard shows. The status a dashboard reports while it
            // finishes loading refuses nothing.
            ResourceKind::Dashboard => self
                .resolve_dashboard_mut(&parsed.id, &parsed.region, now)
                .map(|_| true),
            // An event data store id is unique across Regions, and a store is
            // reachable only from the Region it lives in — so an ARN pinning
            // another Region names no store.
            ResourceKind::EventDataStore => self
                .event_data_stores
                .get(&parsed.id)
                .filter(|record| record.region == parsed.region)
                // `EventDataStoreStatus` carries no "still loading" state, so a
                // stored store is ready for a write unless it is on its way out:
                // a store awaiting deletion is the resource `ConflictException`
                // ("another operation is modifying the resource") covers.
                .map(|record| record.status.is_queryable()),
            // A channel lives in the Region its ARN carries, keyed by the id
            // CloudTrail minted for it ([`channels::ChannelKey`]). A channel record
            // carries no transient status, so a stored channel is ready to take a
            // policy.
            ResourceKind::Channel => self
                .channels
                .get(&(parsed.id.clone(), parsed.region.clone()))
                .map(|_| true),
            // Refused above by `takes_resource_policy`; answered rather than
            // panicked so the two rules cannot disagree at runtime.
            ResourceKind::Trail => None,
        };
        let Some(ready) = ready else {
            return Err(PolicyResourceRejection::ResourceNotFound(
                parsed.kind.not_found_message(value),
            ));
        };

        Ok(PolicyResource {
            arn: parsed.canonical(),
            ready,
        })
    }
}

/// The policy elements a resource-based policy statement is checked against, from
/// the IAM JSON policy grammar: a statement states an `Effect` of `Allow` or `Deny`,
/// an `Action` (or `NotAction`), and — because this is a policy attached TO a
/// resource rather than to an identity — a `Principal` (or `NotPrincipal`).
const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// The refusal text for `ResourcePolicyNotValidException`.
///
/// A FIXED service sentence: the recorded refusal of a document that is not a policy
/// states nothing from the request in it — not the part of the grammar the document
/// broke and not the document itself — so every way a document can fail this check is
/// answered with these words.
pub const RESOURCE_POLICY_NOT_VALID_MESSAGE: &str = "The given resource policy is not valid.";

/// Whether a caller-supplied resource-based policy document IS a policy.
///
/// `false` is the refusal `ResourcePolicyNotValidException` covers, which the model
/// documents for a policy that "has syntax errors, or contains a principal that is
/// not valid" — so the syntax of the JSON policy document and the presence of a
/// usable principal are what this checks. The refusal itself states one fixed
/// sentence ([`RESOURCE_POLICY_NOT_VALID_MESSAGE`]), so what each rule rejects is
/// recorded in the comments here rather than in a message.
///
/// MIGRATION NOTE: the GRAMMAR this walks is one IAM rule, and three other services
/// already walk it against their own recorded wording —
/// `service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`,
/// `service-com-amazonaws-s3files-s3files` and `service-com-amazonaws-rum-rum`, each
/// in its own `src/state.rs`. `services/common` carries no shared grammar check yet
/// (its `iam_authorization` / `iam_trust_policy` EVALUATE a policy rather than
/// validate one, and the latter's parse is `pub(crate)`), and each copy's messages
/// are its service's dialect, so the structural fix is a shared
/// `services/common` policy-grammar module that takes the per-service wording from
/// its caller and that every copy — this one included — then calls. Until that lands
/// the document is still parsed by `serde_json` rather than by a hand-rolled parser,
/// and only the element checks live here.
pub fn is_valid_resource_policy(document: &str) -> bool {
    // A document that does not parse has the "syntax errors" the model names.
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    // A policy document is a JSON object carrying a Statement element.
    let Some(members) = parsed.as_object() else {
        return false;
    };

    let Some(statement) = members.get("Statement") else {
        return false;
    };
    let statements: Vec<&::serde_json::Value> = match statement {
        // The Statement element is a statement or an array of statements, and
        // nothing else.
        ::serde_json::Value::Array(items) => items.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => return false,
    };
    // A policy document states at least one statement.
    if statements.is_empty() {
        return false;
    }

    statements.iter().all(|statement| {
        let Some(elements) = statement.as_object() else {
            return false;
        };

        // Each statement states an Effect the grammar admits, …
        let effect = elements.get("Effect").and_then(::serde_json::Value::as_str);
        if !effect.is_some_and(|effect| POLICY_EFFECTS.contains(&effect)) {
            return false;
        }
        // … an Action or a NotAction, …
        if !elements.contains_key("Action") && !elements.contains_key("NotAction") {
            return false;
        }
        // … and — because this is a policy attached TO a resource — a Principal (or
        // NotPrincipal) naming somebody the policy can grant to.
        elements
            .get("Principal")
            .or_else(|| elements.get("NotPrincipal"))
            .is_some_and(is_valid_principal)
    })
}

/// Whether a statement's `Principal` element names a principal at all: the `*`
/// wildcard, or a map of principal type (`AWS`, `Service`, …) to one identifier or a
/// non-empty list of them, as the IAM JSON policy `Principal` element documents.
fn is_valid_principal(principal: &::serde_json::Value) -> bool {
    let named = |value: &::serde_json::Value| {
        value
            .as_str()
            .is_some_and(|identifier| !identifier.trim().is_empty())
    };
    match principal {
        ::serde_json::Value::String(_) => named(principal),
        ::serde_json::Value::Object(types) => {
            !types.is_empty()
                && types.values().all(|identifiers| match identifiers {
                    ::serde_json::Value::Array(items) => {
                        !items.is_empty() && items.iter().all(named)
                    }
                    other => named(other),
                })
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tagging: the resource a tagging call addresses by ARN
// ---------------------------------------------------------------------------

/// The HTTP status CloudTrail answers a tagging refusal with.
///
/// Every tagging refusal in the recorded corpus is HTTP 400, including the
/// `*NotFoundException` shapes the model marks `@httpError(404)`:
/// `cloudtrail-channel-notfound-error-paths#task#5` records `RemoveTags` answering a
/// channel ARN that names no channel with HTTP 400 and the error code
/// `ChannelNotFoundException`. A client branches on the status it is SENT, so the
/// emulator sends the one the service sends; the `*NotFoundException` shapes of the
/// tagging operations are the only place the model's status and the wire's disagree.
pub const TAGGING_REFUSAL_STATUS: u16 = 400;

/// How many ARNs one `ListTags` call may name, from the model doc for
/// `ListTagsRequest$ResourceIdList`: "The list has a limit of 20 ARNs."
pub const LIST_TAGS_RESOURCE_LIMIT: usize = 20;

/// A well-formed tagging ARN: which kind of resource it names, the Region that
/// resource lives in, and its name or id.
///
/// The four kinds are the ones the model doc for `AddTagsRequest$ResourceId` /
/// `RemoveTagsRequest$ResourceId` lists — "the ARN of the trail, event data store,
/// dashboard, or channel" — each with its own example ARN format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableArn {
    Trail { region: String, name: String },
    Dashboard { region: String, name: String },
    Channel { region: String, id: String },
    EventDataStore { region: String, id: String },
}

impl TaggableArn {
    /// The noun CloudTrail names this kind of resource by in a refusal, as the
    /// recorded `ChannelNotFoundException` message words it ("The channel `<arn>` was
    /// not found.").
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Trail { .. } => "trail",
            Self::Dashboard { .. } => "dashboard",
            Self::Channel { .. } => "channel",
            Self::EventDataStore { .. } => "event data store",
        }
    }

    /// The refusal text for an ARN that is well formed but names no stored resource,
    /// as a tag WRITE (`AddTags` / `RemoveTags`) words it.
    ///
    /// CloudTrail words each kind its own way, and the recorded corpus pins three of
    /// the four: a trail by its NAME and the account the call is made for ("Unknown
    /// trail: `<name>` for the user: `<account>`"), a channel by the ARN the caller
    /// passed ("The channel `<arn>` was not found."), and a dashboard by one sentence
    /// naming nothing at all ("No matching dashboard found."). An event data store,
    /// which the corpus does not pin, follows the channel's wording — the kind it
    /// shares its ARN-addressed not-found shape with.
    pub fn not_found_message(&self, resource_id: &str) -> String {
        match self {
            Self::Trail { name, .. } => self.not_found_message_naming(name),
            _ => self.not_found_message_naming(resource_id),
        }
    }

    /// The same refusal as [`Self::not_found_message`], as `ListTags` words it.
    ///
    /// The ONE difference the corpus records between the two paths is the subject of
    /// a trail's wording: a tag write names the trail's name, `ListTags` the whole
    /// ARN the request passed.
    pub fn listed_not_found_message(&self, resource_id: &str) -> String {
        self.not_found_message_naming(resource_id)
    }

    /// The per-kind wording, naming `subject`.
    fn not_found_message_naming(&self, subject: &str) -> String {
        match self {
            // The one wording every operation that cannot find a trail answers with,
            // and likewise for a dashboard and an event data store.
            Self::Trail { .. } => unknown_trail_message(subject),
            Self::Dashboard { .. } => DASHBOARD_NOT_FOUND_MESSAGE.to_string(),
            Self::EventDataStore { .. } => event_data_store_not_found_message(subject),
            // A channel's not-found wording is the one every channel operation
            // answers with: one helper, so the tagging and the channel paths cannot
            // drift.
            Self::Channel { .. } => channels::channel_not_found_message(subject),
        }
    }
}

/// Whether `value` has the shape of a resource id this service MINTED: the canonical
/// lowercase hyphenated rendering of a UUID, the one form every minted-UUID id of
/// this service takes.
///
/// Stated once for every such resource, because one reader of it now serves them all:
/// a page token's resume key is checked through [`page_token::decode`], and two
/// predicates that disagreed (on letter case, say) would make one listing accept a
/// resume position the other refuses.
///
/// Only a caller that needs to know whether an id COULD be one this service handed
/// out uses this. Resolving an identifier does not: an id of any other shape simply
/// names no resource. The canonical spelling is the test rather than "parses as a
/// UUID", because the id is also the registry key a walk resumes at, and another
/// spelling of the same UUID would seek to a key the registry does not order there.
pub fn is_minted_uuid_id(value: &str) -> bool {
    ::uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

/// Why a tagging `ResourceId` can name no taggable resource.
///
/// Each variant names the modelled error the handler answers with, so every tagging
/// operation refuses the same malformed ARN the same way. The kind-specific ARN
/// errors exist because the model declares them: a channel ARN is refused as a
/// channel ARN, an event data store ARN as an event data store ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableArnRejection {
    /// Written as a channel ARN but not a well-formed one ⇒
    /// `ChannelARNInvalidException`.
    ChannelArn(String),
    /// Written as an event data store ARN but not a well-formed one ⇒
    /// `EventDataStoreARNInvalidException`.
    EventDataStoreArn(String),
    /// Not a CloudTrail resource ARN at all ⇒ `CloudTrailARNInvalidException`.
    CloudTrailArn(String),
    /// A CloudTrail ARN naming a resource type that carries no tags ⇒
    /// `ResourceTypeNotSupportedException`.
    UnsupportedResourceType { resource_id: String, kind: String },
}

impl TaggableArnRejection {
    /// The refusal text: built from the request, naming the ARN it refused, as the
    /// recorded corpus words it — `RemoveTags` answers an ARN whose resource segment
    /// names no taggable resource with "The ARN `<value>` is invalid." The
    /// kind-specific shapes word it the same way, naming their kind, and an
    /// unsupported resource type names the type it read.
    pub fn message(&self) -> String {
        match self {
            Self::ChannelArn(resource_id) => format!("The channel ARN {resource_id} is invalid."),
            Self::EventDataStoreArn(resource_id) => {
                format!("The event data store ARN {resource_id} is invalid.")
            }
            Self::CloudTrailArn(resource_id) => format!("The ARN {resource_id} is invalid."),
            Self::UnsupportedResourceType { resource_id, kind } => format!(
                "The resource type {kind} in the ARN {resource_id} is not supported by \
                 CloudTrail: tags are carried by a trail, an event data store, a dashboard \
                 or a channel."
            ),
        }
    }
}

/// The character set a CloudTrail resource ARN is written from.
///
/// The model constrains the ARN of three of the four taggable kinds — `ChannelArn`,
/// `EventDataStoreArn` and `DashboardArn`, each `^[a-zA-Z0-9._/\-:]+$`
/// (`smithy_query(query="shape", shape="ChannelArn,EventDataStoreArn,DashboardArn")`) —
/// so that set is what a CloudTrail ARN is spelled with, whatever resource type is
/// written inside it. A tagging `ResourceId` carries no `@pattern` of its own (it
/// targets a plain `String`), so nothing has applied this before
/// [`parse_taggable_arn`] does.
const ARN_PUNCTUATION: [char; 5] = ['.', '_', '/', '-', ':'];

/// Whether `value` is written only from the characters a CloudTrail resource ARN is
/// spelled with ([`ARN_PUNCTUATION`] plus ASCII letters and digits).
fn is_arn_spelling(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ARN_PUNCTUATION.contains(&c))
}

/// Read a tagging `ResourceId` as the ARN of one of the four taggable resources.
///
/// The member is documented as "the ARN of the trail, event data store, dashboard, or
/// channel", so a value that is not such an ARN names no resource and is refused
/// rather than read as a bare name. The refusal names the kind of ARN the caller
/// MEANT — the resource type written in the value — which stays readable even when
/// the rest of the ARN is not.
///
/// The ARN itself is read through [`::service_common::arn`], the one parse of a
/// caller-supplied resource ARN in this tree, so the resource field keeps the colons
/// the model's `ChannelArn` / `EventDataStoreArn` pattern (`^[a-zA-Z0-9._/\-:]+$`)
/// admits and the `<type>/<id>` and `<type>:<id>` forms are read the same way. What
/// this function adds is this service's own part: that the value is spelled like an ARN
/// at all ([`is_arn_spelling`]), WHICH of the four kinds the ARN names, and that kind's
/// naming rules.
///
/// It is also why a tagging call does not read its identifier through
/// [`parse_trail_identifier`] or [`parse_event_data_store_identifier`]: those accept
/// the bare name / bare id form their OWN operations document, and each already knows
/// which kind of resource it is looking for, while a tagging `ResourceId` has to be
/// classified by kind before it can be read at all.
///
/// [`parse_resource_arn`] classifies the same four kinds for the resource-POLICY
/// surface; see the note there for what keeps the two apart and what merging them
/// would take.
pub fn parse_taggable_arn(value: &str) -> Result<TaggableArn, TaggableArnRejection> {
    // A value carrying a character no CloudTrail resource ARN is spelled with is not
    // one, whatever resource type is written inside it, so it is refused as an ARN
    // before its kind is read: the kind-specific errors below answer for a value that
    // IS written as that kind's ARN, and reading a kind out of a value that is no ARN
    // would let a resource id the model's ARN pattern cannot hold be looked up as if
    // it named something.
    if !is_arn_spelling(value) {
        return Err(TaggableArnRejection::CloudTrailArn(value.to_string()));
    }

    let reject = || {
        if value.contains(channels::CHANNEL_RESOURCE_PREFIX) {
            TaggableArnRejection::ChannelArn(value.to_string())
        } else if value.contains(EVENT_DATA_STORE_RESOURCE_PREFIX) {
            TaggableArnRejection::EventDataStoreArn(value.to_string())
        } else {
            TaggableArnRejection::CloudTrailArn(value.to_string())
        }
    };

    let Some(arn) = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE) else {
        return Err(reject());
    };
    // Every taggable CloudTrail resource lives in a Region and belongs to an account,
    // so an ARN leaving either field empty names none of them — unlike the
    // partition-global ARNs the shared parse deliberately admits.
    if arn.region.is_empty() || arn.account.is_empty() {
        return Err(reject());
    }
    let resource_type = arn.resource_type();
    let Some(id) = ::service_common::arn::resource_id_of_type(value, resource_type) else {
        return Err(reject());
    };
    if id.is_empty() {
        return Err(reject());
    }

    let region = arn.region.to_string();
    // The resource type is the segment the four example ARN formats differ in; each
    // one's id has to satisfy that resource's own naming rules.
    match resource_type {
        "trail" => {
            validate_trail_name(id).map_err(|_| reject())?;
            Ok(TaggableArn::Trail {
                region,
                name: id.to_string(),
            })
        }
        "dashboard" => {
            if !is_valid_dashboard_name(id) {
                return Err(reject());
            }
            Ok(TaggableArn::Dashboard {
                region,
                name: id.to_string(),
            })
        }
        // A channel id and an event data store id are opaque to this service: the
        // model constrains the WHOLE ARN (`ChannelArn` / `EventDataStoreArn`, length
        // 3..256 of `^[a-zA-Z0-9._/\-:]+$`), whose character set every value reaching
        // here already satisfies (see `is_arn_spelling` above), and says nothing
        // further about the id itself.
        "channel" => Ok(TaggableArn::Channel {
            region,
            id: id.to_string(),
        }),
        "eventdatastore" => Ok(TaggableArn::EventDataStore {
            region,
            id: id.to_string(),
        }),
        kind => Err(TaggableArnRejection::UnsupportedResourceType {
            resource_id: value.to_string(),
            kind: kind.to_string(),
        }),
    }
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The tag set stored for the resource `arn` names, or `None` when no stored
    /// resource answers to it.
    ///
    /// Each of the four kinds resolves in the registry that HOLDS that kind, and `None`
    /// is that registry holding no such record — never a kind answered without reading
    /// it. An event data store and a channel are reachable only from the Region they live
    /// in, which is the Region their ARN pins — the same rule [`Self::event_data_store`]
    /// and [`Self::addressed_channel`] apply. A channel the registry does not hold (no
    /// channel carries that id in that Region) answers `None`, which its caller reports
    /// as `ChannelNotFoundException`.
    pub fn resource_tags(&self, arn: &TaggableArn) -> Option<&[Tag]> {
        match arn {
            TaggableArn::Trail { region, name } => self
                .trails
                .get(&(name.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
            TaggableArn::Dashboard { region, name } => self
                .dashboards
                .get(&(name.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
            TaggableArn::EventDataStore { region, id } => self
                .event_data_stores
                .get(id)
                .filter(|record| record.region == *region)
                .map(|record| record.tags.as_slice()),
            // The channel registry is keyed by the id and Region a channel ARN pins
            // ([`channels::ChannelKey`]), which is exactly what a tagging ARN
            // carries — the same two fields a trail's or a store's ARN is looked up
            // by here.
            TaggableArn::Channel { region, id } => self
                .channels
                .get(&(id.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
        }
    }

    /// The tag set of the resource `arn` names, to change in place. See
    /// [`Self::resource_tags`] for what `None` means.
    pub fn resource_tags_mut(&mut self, arn: &TaggableArn) -> Option<&mut Vec<Tag>> {
        match arn {
            TaggableArn::Trail { region, name } => self
                .trails
                .get_mut(&(name.clone(), region.clone()))
                .map(|record| &mut record.tags),
            TaggableArn::Dashboard { region, name } => self
                .dashboards
                .get_mut(&(name.clone(), region.clone()))
                .map(|record| &mut record.tags),
            TaggableArn::EventDataStore { region, id } => self
                .event_data_stores
                .get_mut(id)
                .filter(|record| record.region == *region)
                .map(|record| &mut record.tags),
            TaggableArn::Channel { region, id } => self
                .channels
                .get_mut(&(id.clone(), region.clone()))
                .map(|record| &mut record.tags),
        }
    }
}

/// Merge `added` into `existing` the way `AddTags` documents it: "Overwrites an
/// existing tag's value when a new value is specified for an existing tag key", and a
/// key not already on the resource is appended.
///
/// The merged set is handed back rather than written in place, so a caller can hold it
/// against the per-resource limit before it becomes the resource's tag set.
pub fn tags_merged(existing: &[Tag], added: Vec<Tag>) -> Vec<Tag> {
    let mut merged = existing.to_vec();
    for tag in added {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => merged.push(tag),
        }
    }
    merged
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudtrailCloudtrail20131101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudtrailCloudtrail20131101State>)
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type — [`service_common::management_events::ManagementEventLog`] — and not a
/// field of this service's own state: CloudTrail reports the management calls of every
/// service, and the service that served a call is the only one that knows it happened,
/// so recorder and reporter must reach one log rather than two copies of it.
///
/// `None` is answered differently on the two sides, deliberately: a READER (`LookupEvents`)
/// cannot report what it cannot read, so it refuses with the modelled
/// `UnsupportedOperationException`, while a RECORDER still serves the caller's write and
/// only traces a warning — refusing a write because the audit log is unreachable would be
/// a worse answer than serving it unrecorded.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The account's SESSION directory in that same shared engine: the sessions STS minted,
/// each keyed by the access key id it was minted under, which is what lets a record of a
/// call made with one name the session it was made by.
///
/// A SHARED type, for the same reason as the log above: the service that MINTS a session
/// and the services that record the calls made WITH it are not the same service.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<SessionAuditDirectory>)
}

/// The session the credential a request was signed with belongs to, for a request signed
/// with one: what the record of this call carries as its `userIdentity.sessionContext`.
///
/// `None` for a long-term credential, for an unsigned request, and for an engine that
/// carries no directory — each of which is a call made by no session, which is what leaves
/// the block out of the record.
pub fn recorded_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: Option<&str>,
) -> Option<RecordedSession> {
    let access_key_id = access_key_id?;
    session_audit_directory(engine)
        .and_then(|directory| directory.lock().session(access_key_id).cloned())
}

/// This service's `eventSource` on an audit record: CloudTrail's endpoint prefix
/// (`awsService.endpointPrefix` in the model) in the `<prefix>.amazonaws.com` form
/// CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// The CloudFormation-style type name of the resource a trail event refers to, as
/// CloudTrail reports it in an event's `Resources`.
pub const TRAIL_RESOURCE_TYPE: &str = "AWS::CloudTrail::Trail";

/// Project this account's LOGGING trails onto the shared management-event log, so the
/// services whose calls that log records know whether CloudTrail is fanning them out onto
/// the account's default event bus.
///
/// Called by every operation that changes WHICH calls a trail logs — the create, the
/// reconfiguration, the delete and the Start/StopLogging pair — after its own write, so the
/// projection is never stale between two of them. It is a projection and not a copy: a
/// trail's own record stays here, and what travels is the delivery decision
/// ([`service_common::management_events::DeliveringTrail`]), because the service that
/// records a call cannot read this store and the fan-out is decided on the recording path.
///
/// Best effort: an engine carrying no log has nobody to tell, which is the same engine on
/// which nothing is recorded either.
pub fn publish_logging_trails<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard is taken and dropped inside this statement: the projection is built here so
    // no other store's lock is held while this one is.
    let logging: Vec<DeliveringTrail> = state
        .lock()
        .trails
        .values()
        .filter(|trail| trail.status.is_logging)
        .map(delivering_trail)
        .collect();
    let Some(log) = management_event_log(engine) else {
        return;
    };
    log.lock().trails_delivering(logging);
}

/// Refresh the logging-trail projection ([`publish_logging_trails`]) when the call that may
/// have changed it RETURNS — on every path out of it, the refusals included, because
/// recomputing the projection from the registry is the same work either way.
///
/// Bound as the FIRST statement of such a handler, so it is dropped LAST: the registry's own
/// guard is taken later in the handler and released before this runs, which is what keeps
/// the two stores from ever being locked at once.
pub struct LoggingTrailsRefreshed<'a, Eng: Send + Sync + 'static> {
    engine: &'a Eng,
}

impl<'a, Eng: Send + Sync + 'static> LoggingTrailsRefreshed<'a, Eng> {
    /// Refresh `engine`'s projection when the returned value is dropped.
    pub fn on_return(engine: &'a Eng) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> Drop for LoggingTrailsRefreshed<'_, Eng> {
    fn drop(&mut self) {
        publish_logging_trails(self.engine);
    }
}

/// One logging trail as the fan-out reads it: where it DELIVERS, where it logs, and which
/// management events its selectors admit.
fn delivering_trail(trail: &TrailRecord) -> DeliveringTrail {
    let (management_events, event_class) = logged_management_events(trail);
    DeliveringTrail {
        trail_arn: trail.trail_arn.clone(),
        home_region: trail.home_region.clone(),
        s3_bucket_name: trail.s3_bucket_name.clone(),
        s3_key_prefix: trail.s3_key_prefix.clone(),
        multi_region: trail.is_multi_region_trail,
        global_service_events: trail.include_global_service_events,
        management_events,
        event_class,
        data_event_selectors: logged_data_event_selectors(trail),
    }
}

/// The selectors of `trail` a DATA event is matched against: one per ADVANCED event selector
/// the trail carries.
///
/// Data events are logged only where a selector says so — "trails created without specific
/// event selectors will be configured to log all read and write management events, and no
/// data events" (`EventSelector`) — and a data-event class is named by the
/// `resources.type` field of an ADVANCED selector, which is why a trail carrying none, or
/// carrying BASIC selectors (whose `DataResources` name the three legacy resource types,
/// served by the data planes that own them), contributes nothing here.
///
/// Every field selector is carried through as the client wrote it, so the match is made
/// against the record ([`service_common::data_events::DataEventSelector`]) rather than
/// against a projection that might admit an event the selector excludes.
fn logged_data_event_selectors(trail: &TrailRecord) -> Vec<DataEventSelector> {
    let Some(EventSelectorConfig::Advanced(selectors)) = &trail.event_selectors else {
        return Vec::new();
    };
    selectors
        .iter()
        .filter(|selector| !selects_management_category(selector))
        .map(|selector| DataEventSelector {
            field_selectors: selector
                .field_selectors
                .iter()
                .map(|field| DataEventFieldSelector {
                    field: field.field.clone(),
                    equals: field.equals.clone().unwrap_or_default(),
                    starts_with: field.starts_with.clone().unwrap_or_default(),
                    ends_with: field.ends_with.clone().unwrap_or_default(),
                    not_equals: field.not_equals.clone().unwrap_or_default(),
                    not_starts_with: field.not_starts_with.clone().unwrap_or_default(),
                    not_ends_with: field.not_ends_with.clone().unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}

/// Whether `trail`'s selectors log management events at all, and which read/write class of
/// them.
///
/// The three cases are the three a trail can be in:
///
/// * NOTHING configured — "trails created without specific event selectors will be
///   configured to log all read and write management events" ([`default_event_selector`]),
///   so every management call is logged;
/// * BASIC selectors — a selector logs management events when its
///   `IncludeManagementEvents` is set, in the class its `ReadWriteType` names, so the trail
///   logs what any one of its selectors admits;
/// * ADVANCED selectors — a selector admits management events when its `eventCategory`
///   field equals `Management`, and narrows the class when it also pins `readOnly`.
fn logged_management_events(trail: &TrailRecord) -> (bool, ManagementEventClass) {
    let selectors = match &trail.event_selectors {
        None => return (DEFAULT_INCLUDE_MANAGEMENT_EVENTS, ManagementEventClass::All),
        Some(EventSelectorConfig::Basic(selectors)) => {
            let logging: Vec<&EventSelector> = selectors
                .iter()
                .filter(|selector| selector.include_management_events.unwrap_or(false))
                .collect();
            let class = logging
                .iter()
                .map(|selector| {
                    ManagementEventClass::from_read_write_type(selector.read_write_type.as_deref())
                })
                .fold(None, widest_class);
            return (!logging.is_empty(), class.unwrap_or_default());
        }
        Some(EventSelectorConfig::Advanced(selectors)) => selectors,
    };
    let class = selectors
        .iter()
        .filter(|selector| selects_management_category(selector))
        .map(advanced_event_class)
        .fold(None, widest_class);
    (class.is_some(), class.unwrap_or_default())
}

/// The class two selectors log between them: a trail logging both read-only and write-only
/// management events logs all of them.
fn widest_class(
    carried: Option<ManagementEventClass>,
    class: ManagementEventClass,
) -> Option<ManagementEventClass> {
    match carried {
        None => Some(class),
        Some(carried) if carried == class => Some(carried),
        Some(_) => Some(ManagementEventClass::All),
    }
}

/// Whether an advanced selector admits the `Management` event category — the field every
/// advanced selector pins, and the one that says whether a selector is about management
/// events at all.
fn selects_management_category(selector: &AdvancedEventSelector) -> bool {
    selector.field_selectors.iter().any(|field| {
        field.field == ADVANCED_SELECTOR_EVENT_CATEGORY_FIELD
            && field
                .equals
                .as_ref()
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value == ::service_common::management_events::EVENT_CATEGORY_MANAGEMENT)
                })
    })
}

/// The read/write class an advanced selector logs: the one its `readOnly` field pins, or
/// both when it pins none.
fn advanced_event_class(selector: &AdvancedEventSelector) -> ManagementEventClass {
    for field in &selector.field_selectors {
        if field.field != ADVANCED_SELECTOR_READ_ONLY_FIELD {
            continue;
        }
        let Some(values) = field.equals.as_ref() else {
            continue;
        };
        return match values.first().map(String::as_str) {
            Some("true") => ManagementEventClass::ReadOnly,
            Some("false") => ManagementEventClass::WriteOnly,
            _ => ManagementEventClass::All,
        };
    }
    ManagementEventClass::All
}

/// The advanced-selector field that names an event's category
/// (`AdvancedFieldSelector$Field`: "eventCategory").
const ADVANCED_SELECTOR_EVENT_CATEGORY_FIELD: &str = "eventCategory";

/// The advanced-selector field that names an event's read/write kind
/// (`AdvancedFieldSelector$Field`: "readOnly").
const ADVANCED_SELECTOR_READ_ONLY_FIELD: &str = "readOnly";


