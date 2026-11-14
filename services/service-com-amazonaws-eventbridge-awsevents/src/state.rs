// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eventbridge-awsevents service.
//!
//! EventBridge's control plane is two nested resources: an event bus (named
//! within one Region — an event bus is a regional resource, so the bus store is
//! keyed by [`BusKey`] and every Region holds the account's `default` bus) and
//! the rules bound to it. A rule owns an ordered list of targets. `PutRule` creates or replaces a
//! rule, `PutTargets`/`RemoveTargets` upsert and remove its targets by `Id`,
//! `DescribeRule`/`ListRules`/`ListTargetsByRule` report them, and `DeleteRule`
//! removes one. A bus also carries its own mutable configuration, which
//! `CreateEventBus` and `UpdateEventBus` write and `DescribeEventBus` reports,
//! and its resource policy ([`BusPolicy`]): the statements `PutPermission`
//! installs and `RemovePermission` revokes. `PutEvents` is the data plane: it
//! routes a published event to
//! every enabled rule on the entry's bus whose `EventPattern` matches, and
//! appends what it routed to [`ComAmazonawsEventbridgeAwseventsState::delivered`]
//! so the delivery is observable state rather than a discarded side effect; what
//! came of handing that event to each target of each matched rule is appended to
//! [`ComAmazonawsEventbridgeAwseventsState::invocations`]. Whether EventBridge
//! may invoke a target at all is decided against the account's OWN IAM records
//! ([`delivery_refusal`]) and, for a publication onto an event bus, against that
//! bus's own resource policy ([`publication_refusal`]) — and a publication a
//! CLIENT makes against that policy together with the calling principal's own
//! identity-based policies ([`authorization::publication_denial`]). Each match is counted on
//! the matched rule's own `MatchedEvents` and `TriggeredRules` metrics
//! ([`count_rule_matches`]) and what came of each
//! handover is counted on the
//! rule's own `Invocations` and `SuccessfulInvocationAttempts`, or on its
//! `FailedInvocations`, in the CloudWatch store
//! ([`count_invocation`], [`count_failed_invocation`]) — both reached through the
//! same shared engine this service's store lives in, so there is one copy of each
//! fact.
//!
//! Beside the bus tree, EventBridge owns the CONNECTION resource
//! ([`ConnectionRecord`]): the authorization type and credentials
//! `CreateConnection` writes, `DescribeConnection` reports, `DeauthorizeConnection`
//! strips and `DeleteConnection` removes. A connection answered in a non-terminal
//! state reaches its next one on its own, [`CONNECTION_TRANSITION`] later
//! ([`ComAmazonawsEventbridgeAwseventsState::settle_connections`]).
//!
//! Two resources hang off those: the ARCHIVE ([`ArchiveRecord`]), bound to one
//! event bus, which `CreateArchive` writes, `UpdateArchive` changes,
//! `DescribeArchive` and `ListArchives` report, `DeleteArchive` removes and
//! `PutEvents` captures matching events into, and the API DESTINATION
//! ([`ApiDestinationRecord`]), the HTTP endpoint `CreateApiDestination` writes
//! and `DescribeApiDestination` and `ListApiDestinations` report, whose state
//! follows the connection it invokes through
//! ([`ComAmazonawsEventbridgeAwseventsState::api_destination_state`]).
//!
//! It also owns the GLOBAL ENDPOINT ([`EndpointRecord`]), the pair of event
//! buses `CreateEndpoint` writes, `DescribeEndpoint` reports and
//! `DeleteEndpoint` removes: an endpoint answered in a non-terminal state
//! reaches its next one on its own, [`ENDPOINT_TRANSITION`] later
//! ([`ComAmazonawsEventbridgeAwseventsState::settle_endpoints`]).
//!
//! It also owns the PARTNER EVENT SOURCE ([`PartnerEventSourceRecord`]), the one
//! resource two accounts share: an SaaS partner authors it in a namespace it is
//! the registered partner of ([`not_entitled_to_namespace`] refuses any other
//! caller) with `CreatePartnerEventSource`, reads it back with
//! `DescribePartnerEventSource`, `ListPartnerEventSources` and
//! `ListPartnerEventSourceAccounts`, publishes to it with `PutPartnerEvents` and
//! removes it with `DeletePartnerEventSource`; the customer the source was
//! offered to reads it with `DescribeEventSource`, matches it with a bus through
//! `CreateEventBus`, and turns its ingestion on and off with
//! `ActivateEventSource` and `DeactivateEventSource`. The state such a source
//! reports is derived from the record and that bus
//! ([`ComAmazonawsEventbridgeAwseventsState::event_source_state`]), and a source
//! nobody matched within [`PARTNER_EVENT_SOURCE_EXPIRY`] of entering `PENDING`
//! expires on its own
//! ([`ComAmazonawsEventbridgeAwseventsState::settle_partner_sources`]).
//!
//! Everything a handler of this service reads or writes lives below; the maps
//! are ordered so enumeration is deterministic, which is what lets the list
//! operations resume from an opaque page token.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ApiDestination as ApiDestinationSummary, Archive as ArchiveSummary, Connection,
    ConnectionAuthResponseParameters, ConnectionBodyParameter, ConnectionHeaderParameter,
    ConnectionHttpParameters, ConnectionQueryStringParameter,
    DeadLetterConfig,
    DescribeApiDestinationOutput, DescribeArchiveOutput, DescribeConnectionOutput,
    DescribeEndpointOutput, DescribeReplayOutput, Endpoint as EndpointSummary, EndpointEventBus,
    LogConfig, PartnerEventSourceAccount, Replay as ReplaySummary, ReplayDestination,
    ReplicationConfig, RoutingConfig, Rule as RuleSummary, Tag, Target, UpdateEndpointOutput,
    INCLUDE_DETAIL_VALUES, LEVEL_VALUES, RULE_STATE_VALUES,
};

/// The event bus a request lands on when it names none.
///
/// AWS creates this bus with the account ("Events generated by Amazon Web
/// Services services go to your account's default event bus"), so it exists
/// before any call is made and `CreateEventBus` is not needed to use it.
pub const DEFAULT_EVENT_BUS_NAME: &str = "default";

/// The region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one.
pub const FALLBACK_REGION: &str = "us-east-1";

/// This service's ARN namespace (`OPERATIONS.md`, "Wire protocol").
pub const ARN_NAMESPACE: &str = "events";

/// The ARN namespace of the Key Management Service, whose keys an event bus is
/// encrypted with ([`kms_key_identifier`]).
pub const KMS_ARN_NAMESPACE: &str = "kms";

/// The `RuleState` of a rule EventBridge matches published events against.
pub const RULE_STATE_ENABLED: &str = "ENABLED";

/// The `RuleState` of a rule EventBridge matches no published event against:
/// "DISABLED: ... EventBridge does not match any events against the rule"
/// (`PutRule$State`).
pub const RULE_STATE_DISABLED: &str = "DISABLED";

/// The `RuleState` a rule takes when `PutRule` omits `State`: "Rules are
/// enabled by default, or based on value of the state" (`PutRule`).
pub const DEFAULT_RULE_STATE: &str = RULE_STATE_ENABLED;

/// What a handler reports when the engine it was built on carries no store for
/// this service — a server fault, never a bad request.
pub const STORE_UNAVAILABLE: &str = "the eventbridge state store is unavailable";

/// What `RemovePermission` reports for a `StatementId` the bus's resource policy
/// carries no statement for.
///
/// EventBridge's own wording for that refusal, which names no statement id: the
/// id is in the request the caller sent.
pub const NO_SUCH_POLICY_STATEMENT: &str = "Statement with the provided id does not exist.";

/// What EventBridge reports for a request that would take a resource COUNT past
/// the quota that bounds it.
///
/// `LimitExceededException` is "the request failed because it attempted to
/// create resource beyond the allowed service quota", and the message the
/// service answers it with is this FIXED sentence: it names neither the resource
/// the request was about nor the number that bounds it, so a quota a caller
/// cannot raise from the API is not disclosed in the refusal. Stated once, so
/// every count quota of this service — targets on a rule, rules on a bus, buses,
/// connections, archives, replays, api destinations, global endpoints, partner
/// event sources — is refused in the same words.
///
/// A refusal about one submitted VALUE being outside a bound is NOT this: that
/// is a parameter diagnostic naming the member and the value
/// ([`parameter_violation`]), which is a different class of refusal even where it
/// travels on the same modelled error.
pub const QUOTA_EXCEEDED: &str = "The requested resource exceeds the maximum number allowed.";

/// Largest page a list operation returns, and the page size it uses when the
/// request names no `Limit`.
///
/// `LimitMax100` is modelled `range 1..100`, so no client can ask for a bigger
/// page; applying the same maximum to a request that omits `Limit` means one
/// call can never serialize more than this many records.
pub const MAX_LIST_PAGE: usize = 100;

/// Largest number of event buses one account holds IN ONE REGION, that Region's
/// `default` bus included.
///
/// The EventBridge service quota "Event buses per account" (default 100), which
/// like every EventBridge quota is per account and Region — and an event bus is
/// a regional resource ([`BusKey`]). It is also what bounds the store: a
/// `CreateEventBus` that would ADD a bus past it answers the modelled
/// `LimitExceededException` instead of growing memory without limit.
pub const MAX_EVENT_BUSES: usize = 100;

/// Largest number of REGIONS this store holds event buses in.
///
/// This emulator's OWN bound, not a quota AWS publishes: the Region half of a
/// bus's identity ([`BusKey`]) is read off the caller's own credential scope
/// ([`request_region`]), so the per-Region quotas above bound each Region's
/// buses and rules but say nothing about how many Regions a caller may open —
/// and this store holds them all in memory. It is far above the number of
/// Regions AWS has, so no client addressing real Regions reaches it, and a
/// `CreateEventBus` past it answers the modelled `LimitExceededException`
/// ([`ComAmazonawsEventbridgeAwseventsState::may_hold_bus_in`]).
pub const MAX_BUS_REGIONS: usize = 64;

/// Largest number of rules one event bus holds.
///
/// The EventBridge service quota "Rules per event bus" (default 300). It is
/// also what bounds the store: a `PutRule` that would ADD a rule past it
/// answers the modelled `LimitExceededException` instead of growing memory
/// without limit. A rule already stored stays writable.
pub const MAX_RULES_PER_BUS: usize = 300;

/// Largest number of targets one rule holds.
///
/// The EventBridge service quota "Targets per rule" (5, a hard limit). The
/// modelled `TargetList` bound of 1..100 constrains ONE request's list, not the
/// rule's total, so this is checked against the rule after the upsert is
/// resolved.
pub const MAX_TARGETS_PER_RULE: usize = 5;

/// Largest number of tags one taggable resource of this service carries.
///
/// "You can associate as many as 50 tags with a resource" (`TagResource`). The
/// modelled `TagList` carries no `@length`, so nothing refuses a longer list
/// before a handler runs; it is also what bounds the store, since a tag write
/// past it is refused rather than growing a record without limit. It is charged
/// on the RESULT of a write, which is what the limit is about: re-setting a key
/// the resource already carries does not grow its tag set.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `version` every EventBridge event envelope carries.
pub const EVENT_VERSION: &str = "0";

/// Largest TOTAL entry size one `PutEvents` request may carry, in bytes —
/// INCLUSIVE: a request measuring exactly this is within the limit.
///
/// "You can batch multiple event entries into one request for efficiency.
/// However, the total entry size must be less than 1MB" (`PutEvents`), where
/// the megabyte the limit is published in is 2^20 bytes: entries totalling
/// exactly 1,048,576 bytes are the most AWS accepts, and only MORE than that is
/// refused. The charge is the one AWS documents under "Calculating PutEvents
/// event entry size" (see [`put_events_entry_size`]), summed over the request's
/// entries — which for a single-entry request is that entry's own size. A
/// request past it is refused as a WHOLE (see [`REQUEST_SIZE_EXCEEDED`]),
/// which is also what bounds the text the event-pattern matcher walks.
pub const MAX_ENTRY_SIZE_BYTES: usize = 1_048_576;

/// How many routed events the routing log keeps.
/// This is the emulator's OWN bound on its store, not a claim about any limit
/// AWS publishes: `PutEvents` appends one record per accepted entry and nothing
/// in the API deletes one, so without a bound a caller could grow the process
/// heap without end. The log is a ring: the oldest record is evicted before a
/// new one is appended, so the most recent [`MAX_DELIVERED_EVENTS`] routed
/// events are always observable.
pub const MAX_DELIVERED_EVENTS: usize = 10_000;

/// How many bytes of caller-supplied event text the routing log keeps.
///
/// A second bound on the same ring, beside [`MAX_DELIVERED_EVENTS`], because a
/// record count alone does not bound memory: one entry's `Detail` may be just
/// under [`MAX_ENTRY_SIZE_BYTES`], so a count-only cap would admit gigabytes.
/// Both bounds are enforced by [`ComAmazonawsEventbridgeAwseventsState::record_delivery`],
/// which evicts oldest-first until each holds.
pub const MAX_DELIVERED_BYTES: usize = 8_000_000;

/// How many bytes of text the event-pattern matcher may walk for ONE
/// `PutEvents` entry, across every rule that entry is matched against.
///
/// The emulator's OWN bound, not a limit AWS publishes. Each factor of the
/// routing work is bounded on its own — at most 10 entries per request
/// (`Entries` is modelled `length 1..10`), [`MAX_RULES_PER_BUS`] rules per bus,
/// [`MAX_ENTRY_SIZE_BYTES`] per event and 4096 pattern characters — but their
/// PRODUCT is not, so one request could otherwise occupy a core for far longer
/// than any real one needs. A budget per entry caps the whole request at this
/// value times the modelled maximum number of entries. It is set far above any
/// realistic event-and-rules combination (a few-kilobyte event against a full
/// bus of 300 rules costs about a megabyte of walking), and an entry that
/// exhausts it is REFUSED with a message saying so rather than silently
/// reported as matching nothing.
pub const MAX_MATCH_BYTES_PER_ENTRY: u64 = 32_000_000;

/// The `Version` an event-bus resource policy document carries.
///
/// IAM's current policy language version, which is what EventBridge assembles a
/// bus policy under; a policy document supplied whole by a caller keeps its own
/// `Version` ([`EventBus::install_policy_document`]).
pub const POLICY_VERSION: &str = "2012-10-17";

/// The `Effect` a permission grant installs.
///
/// `PutPermission` "permits the specified Amazon Web Services account or Amazon
/// Web Services organization to put events to the specified event bus" — a
/// grant, so the statement it installs allows.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";

/// Largest an event bus's resource policy may be, in bytes.
///
/// "The permission policy on the event bus cannot exceed 10 KB in size"
/// (`PutPermission`'s own documentation), so a statement that would push the
/// assembled document past it is refused with the modelled
/// `PolicyLengthExceededException` instead of being stored.
pub const MAX_POLICY_BYTES: usize = 10 * 1024;

/// The size AWS documents for one `PutEvents` entry ("Calculating PutEvents
/// event entry size"): 14 bytes when `Time` is set, plus the UTF-8 length of
/// `Source`, `DetailType` and `Detail`, plus the length of each resource ARN.
pub fn put_events_entry_size(
    time_present: bool,
    source: Option<&str>,
    detail_type: Option<&str>,
    detail: Option<&str>,
    resources: &[String],
) -> usize {
    let mut size = usize::from(time_present) * 14;
    for text in [source, detail_type, detail] {
        size += text.map_or(0, str::len);
    }
    size + resources.iter().map(String::len).sum::<usize>()
}

/// What a refusal states for a request whose entries total more than
/// [`MAX_ENTRY_SIZE_BYTES`].
///
/// The bound is on the REQUEST ("the total entry size"), so the refusal is of
/// the whole call. It is a CONSTANT of the condition — the verdict is on the
/// total of the request's entries, not on anything in any one of them — so it
/// carries none of the request's values.
pub const REQUEST_SIZE_EXCEEDED: &str =
    "Total size of the entries in the request is over the limit.";

/// One target of a rule, as `PutTargets` stored it.
///
/// `raw` keeps the whole modelled `Target`, so `ListTargetsByRule` reports back
/// exactly the structure that was stored — every member, not the handful the
/// routing log needs. The flat fields beside it are the ones routing reads.
#[derive(Debug, Clone)]
pub struct StoredTarget {
    pub id: String,
    pub arn: String,
    pub role_arn: Option<String>,
    pub input: Option<String>,
    pub input_path: Option<String>,
    pub raw: Target,
}

impl StoredTarget {
    /// The stored target, projected onto the modelled shape.
    pub fn to_target(&self) -> Target {
        self.raw.clone()
    }
}

/// A rule as this emulator persists it.
///
/// `arn` is minted ONCE, when the rule is first created, and never recomputed:
/// `PutRule` is create-or-update, so a second call with the same name must hand
/// back the same `RuleArn` it handed back the first time.
#[derive(Debug, Clone)]
pub struct RuleRecord {
    pub name: String,
    pub arn: String,
    pub event_pattern: Option<String>,
    pub schedule_expression: Option<String>,
    /// One of `RuleState`'s values (`RULE_STATE_VALUES`).
    pub state: String,
    pub description: Option<String>,
    pub role_arn: Option<String>,
    /// Set only for a rule an Amazon Web Services service created on the
    /// account's behalf; nothing in this emulator creates one, so it is `None`.
    pub managed_by: Option<String>,
    /// The account the rule was created by, which is this emulator's account.
    pub created_by: Option<String>,
    pub event_bus_name: String,
    /// Insertion-ordered, which is the order `ListTargetsByRule` pages through;
    /// upsert is keyed by [`StoredTarget::id`].
    pub targets: Vec<StoredTarget>,
    /// The tags this rule carries, in the order their keys were first written,
    /// so a tag read reports back exactly what the tag writes left. "In
    /// EventBridge, rules and event buses can be tagged" (`TagResource`), so a
    /// rule holds its own tag set beside the bus's ([`EventBus::tags`]).
    pub tags: Vec<Tag>,
}

impl RuleRecord {
    /// The `Rule` summary `ListRules` reports: every member read back from the
    /// stored rule.
    pub fn to_summary(&self) -> RuleSummary {
        RuleSummary {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            event_pattern: self.event_pattern.clone(),
            state: Some(self.state.clone()),
            description: self.description.clone(),
            schedule_expression: self.schedule_expression.clone(),
            role_arn: self.role_arn.clone(),
            managed_by: self.managed_by.clone(),
            event_bus_name: Some(self.event_bus_name.clone()),
        }
    }

    /// Whether this rule matches published events at all.
    ///
    /// `DISABLED` means "EventBridge does not match any events against the
    /// rule" (`PutRule`'s `State`); a rule carrying only a `ScheduleExpression`
    /// self-triggers on its schedule and is never matched by a published event.
    pub fn matches_published_events(&self) -> bool {
        self.state != RULE_STATE_DISABLED && self.event_pattern.is_some()
    }

    /// The position of the target `id` names, for upsert and removal.
    pub fn target_position(&self, id: &str) -> Option<usize> {
        self.targets.iter().position(|target| target.id == id)
    }
}

/// One statement of an event bus's resource policy.
///
/// `body` is the statement as it will be serialized into the policy document
/// `DescribeEventBus` reports, so the document is assembled by concatenation and
/// no statement is re-rendered on a read. `sid` is the statement's `Sid`, which
/// is what `PutPermission` and `RemovePermission` address a statement by ("If
/// you later want to revoke the permission for this external account, specify
/// this StatementId when you run RemovePermission. Each StatementId must be
/// unique") — `None` for a statement that arrived inside a whole policy document
/// carrying no `Sid`, which is therefore addressable only by replacing the
/// document.
#[derive(Debug, Clone)]
pub struct PolicyStatement {
    pub sid: Option<String>,
    pub body: String,
}

/// What identifies one event bus in this store: the Region it lives in, and its
/// name within that Region.
///
/// An event bus is a REGIONAL resource — its ARN carries the Region
/// ([`event_bus_arn`]) and a global endpoint is built from "a primary and
/// secondary Region with event buses in each Region" whose names "must be
/// identical in each Region" ([`endpoint_event_bus_violation`]) — so the same
/// name in two Regions is two separate buses and a name is unique only WITHIN a
/// Region. The Region comes first in the ordering, so the buses of one Region
/// enumerate in name order and a list operation's page token stays a position in
/// that order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BusKey {
    pub region: String,
    pub name: String,
}

impl BusKey {
    /// The key of the bus `name` names in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// An event bus and the rules bound to it.
///
/// The bus's ARN is NOT stored: an ARN's region segment comes from the request
/// that is being answered ([`request_region`]), which is also the Region half of
/// the bus's identity ([`BusKey`]), and the account's `default` bus exists in
/// every Region before any request is made, so a reader derives it with
/// [`event_bus_arn`] rather than reading a region frozen at startup.
#[derive(Debug, Clone)]
pub struct EventBus {
    pub name: String,
    pub description: Option<String>,
    /// When the call that brought this bus into existence ran — the instant
    /// `CreationTime` reports, and `None` for a bus no call created.
    ///
    /// The account's `default` bus is not created by anybody: it "receives
    /// events from Amazon Web Services services" and is there before the first
    /// request, so there is no creation instant to report for it and
    /// `CreationTime` stays absent. Every bus `CreateEventBus` wrote carries
    /// the instant that call ran.
    pub created: Option<SystemTime>,
    /// When the bus was last changed: `None` until something changes it, then
    /// the instant of that change. Starts equal to [`Self::created`] for a bus a
    /// call created, so `LastModifiedTime` is never earlier than
    /// `CreationTime`.
    pub last_modified: Option<SystemTime>,
    /// The customer managed key this bus encrypts events with, as the ARN of the
    /// KMS resource the write named ([`kms_key_identifier`]). "If you do not
    /// specify a customer managed key identifier, EventBridge uses an Amazon Web
    /// Services owned key" (`CreateEventBus`'s `KmsKeyIdentifier`), and an owned
    /// key is not reported back, so an absent member stays absent.
    pub kms_key_identifier: Option<String>,
    /// The dead-letter queue configuration, as it was stored.
    pub dead_letter_config: Option<DeadLetterConfig>,
    /// The logging configuration, as it was stored.
    pub log_config: Option<LogConfig>,
    /// The partner event source this bus is matched with
    /// ([`PartnerEventSourceRecord`]), set only for a partner event bus — the one
    /// `CreateEventBus` was given an `EventSourceName` for.
    pub event_source_name: Option<String>,
    /// The tags `CreateEventBus` was given, in the order they arrived, so the
    /// tagging operations report back exactly what was stored.
    pub tags: Vec<Tag>,
    /// This bus's resource policy, as `PutPermission` installed it. Empty for a
    /// bus nothing has granted a permission on, which is why `DescribeEventBus`
    /// reports no `Policy` for one.
    pub policy: BusPolicy,
    /// Rules by name, so enumeration is deterministic and `ListRules` can
    /// resume after a page token.
    pub rules: BTreeMap<String, RuleRecord>,
}

impl EventBus {
    /// A bus named `name`, created at `created` — the starting point
    /// `CreateEventBus` fills in from its request.
    ///
    /// `created` is `None` for a bus no call created (the account's `default`
    /// bus), and that bus therefore reports neither timestamp until something
    /// changes it.
    pub fn new(name: &str, created: Option<SystemTime>) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            created,
            last_modified: created,
            kms_key_identifier: None,
            dead_letter_config: None,
            log_config: None,
            event_source_name: None,
            tags: Vec::new(),
            policy: BusPolicy::default(),
            rules: BTreeMap::new(),
        }
    }

    /// Every rule of this bus a published event can match, with its
    /// `EventPattern` parsed ONCE: the snapshot a publisher takes under the
    /// store's lock so the matching itself can run with no lock held.
    ///
    /// A rule no published event can match is left out
    /// ([`RuleRecord::matches_published_events`]), as is one whose pattern no
    /// longer parses.
    pub fn matchable_rules(&self) -> Vec<MatchableRule> {
        self.rules
            .values()
            .filter(|rule| rule.matches_published_events())
            .filter_map(|rule| {
                Some(MatchableRule {
                    name: rule.name.clone(),
                    pattern: event_pattern::parse(rule.event_pattern.as_deref()?).ok()?,
                    target_arns: rule.targets.iter().map(|target| target.arn.clone()).collect(),
                })
            })
            .collect()
    }

    /// The `CreationTime` this bus reports: absent for a bus no call created, so
    /// a reader can tell the account's implicit bus from one a principal made.
    pub fn creation_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.created.map(to_wire_timestamp)
    }

    /// The `LastModifiedTime` this bus reports: absent while nothing has set it
    /// (see [`Self::created`]).
    pub fn last_modified_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.last_modified.map(to_wire_timestamp)
    }

    /// Record that this bus was changed at `at`, which is what
    /// `LastModifiedTime` reports from then on.
    pub fn touch(&mut self, at: SystemTime) {
        self.last_modified = Some(at);
    }
}

/// One rule of a bus, ready to match a published event: its name, its parsed
/// `EventPattern`, and the `Arn` of every target an event matching it is
/// delivered to.
///
/// Taken from the stored rule ([`EventBus::matchable_rules`]) so routing never
/// re-parses a pattern and never holds the store's lock while matching.
#[derive(Debug, Clone)]
pub struct MatchableRule {
    pub name: String,
    pattern: ::serde_json::Map<String, ::serde_json::Value>,
    pub target_arns: Vec<String>,
}

impl MatchableRule {
    /// Whether `envelope` matches this rule's pattern, spending `budget` — which
    /// is what bounds the comparison work one event can demand across every rule
    /// of its bus (see [`event_pattern::Budget`]).
    pub fn matches(
        &self,
        envelope: &::serde_json::Value,
        budget: &mut event_pattern::Budget,
    ) -> bool {
        event_pattern::matches_parsed(&self.pattern, envelope, budget)
    }
}

/// An event bus's resource policy.
///
/// Two shapes of the same policy, because `PutPermission` installs one in two
/// ways: a grant it describes member by member (`StatementId`, `Action`,
/// `Principal`, `Condition`), which this service assembles into a statement, or
/// a whole `Policy` document the caller wrote. A document a caller wrote is
/// reported back BYTE FOR BYTE as it was installed — re-rendering it would hand
/// a read-modify-write client back a document it did not write — while a policy
/// assembled from grants is rendered from the statements
/// ([`Self::document`]).
#[derive(Debug, Clone, Default)]
pub struct BusPolicy {
    /// The statements of this policy, in the order they were installed. Also
    /// filled in from a document a caller supplied whole, so a later grant
    /// addressing a `Sid` of that document finds it.
    pub statements: Vec<PolicyStatement>,
    /// The document as the caller supplied it, when the whole policy arrived as
    /// one; `None` once a grant has changed the policy, because the stored
    /// statements are then no longer the ones that text describes.
    pub supplied: Option<String>,
    /// The policy language `Version`, from a supplied document or
    /// [`POLICY_VERSION`] for a policy assembled from grants.
    pub version: Option<String>,
}

impl BusPolicy {
    /// This policy as the JSON document `DescribeEventBus` reports in `Policy`,
    /// or `None` when nothing has been installed.
    ///
    /// "The policy that enables the external account to send events to your
    /// account" (`DescribeEventBus`'s `Policy`) is an IAM resource policy: a
    /// `Version` and the statements installed on the bus. A bus with no
    /// statement has no policy at all, so the member stays ABSENT rather than
    /// reporting an empty document.
    pub fn document(&self) -> Option<String> {
        if let Some(supplied) = &self.supplied {
            return Some(supplied.clone());
        }
        if self.statements.is_empty() {
            return None;
        }
        let body = self
            .statements
            .iter()
            .map(|statement| statement.body.as_str())
            .collect::<Vec<_>>()
            .join(",");
        Some(format!(
            "{{\"Version\":{},\"Statement\":[{body}]}}",
            json_text(self.version.as_deref().unwrap_or(POLICY_VERSION))
        ))
    }

    /// Add `statement` to this policy, REPLACING the one carrying the same `Sid`
    /// in place when there is one.
    ///
    /// A `StatementId` identifies the grant ("Each StatementId must be
    /// unique"), so granting again under an id already installed changes that
    /// one grant and leaves every other statement, and their order, alone.
    pub fn install_statement(&mut self, statement: PolicyStatement) {
        let existing = statement.sid.as_deref().and_then(|sid| {
            self.statements
                .iter()
                .position(|held| held.sid.as_deref() == Some(sid))
        });
        match existing {
            Some(at) => self.statements[at] = statement,
            None => self.statements.push(statement),
        }
        // The supplied text no longer describes these statements.
        self.supplied = None;
    }

    /// Remove the statement carrying `sid`, reporting whether one was held.
    ///
    /// A statement that arrived inside a document a caller supplied whole is
    /// removable too, by the `Sid` that document gave it; the remaining
    /// statements are then reported as this service renders them, because the
    /// supplied text no longer describes the policy.
    pub fn remove_statement(&mut self, sid: &str) -> bool {
        let before = self.statements.len();
        self.statements
            .retain(|held| held.sid.as_deref() != Some(sid));
        let removed = self.statements.len() != before;
        if removed {
            self.supplied = None;
        }
        removed
    }
}

/// `value` as a JSON string literal, escaped.
///
/// The policy document is assembled member by member rather than through a
/// `serde_json` object, because a policy statement's members have a published
/// ORDER (`Sid`, `Effect`, `Principal`, `Action`, `Resource`, `Condition`) and
/// this crate's `serde_json` orders an object's keys alphabetically. Every value
/// that goes into it still passes through `serde_json`, so escaping is never
/// hand-rolled.
fn json_text(value: &str) -> String {
    ::serde_json::Value::String(value.to_string()).to_string()
}

/// The policy statement a permission grant installs on an event bus.
///
/// `PutPermission` grants `action` on the bus `resource` names to `principal`,
/// so the statement is that grant written in the IAM policy language: the
/// request's `StatementId` as the `Sid`, [`POLICY_EFFECT_ALLOW`], the principal
/// under `Principal`, the action and the bus ARN as the `Resource`, in that
/// member order. The principal is written AS THE REQUEST GAVE IT — "the
/// 12-digit Amazon Web Services account ID that you are permitting to put events
/// to your default event bus. Specify "*" to permit any account" — so an account
/// id stays an account id (the policy language also admits that account's
/// root-user ARN, but EventBridge installs what it was given) and `*` stays the
/// wildcard. The WILDCARD is rendered the way the policy language writes "every
/// principal", the bare string `"*"` ([`wildcard_principal`]); a named account is
/// an Amazon Web Services principal and is rendered under `AWS`. `condition` is
/// `Condition`'s `Type`, `Key` and `Value`, which "must contain Type, Key, and
/// Value fields" and render as the IAM condition block
/// `{<Type>: {<Key>: <Value>}}`.
pub fn permission_statement(
    sid: &str,
    action: &str,
    principal: &str,
    resource: &str,
    condition: Option<(&str, &str, &str)>,
) -> PolicyStatement {
    let mut body = format!(
        "{{\"Sid\":{},\"Effect\":{},\"Principal\":{},\"Action\":{},\"Resource\":{}",
        json_text(sid),
        json_text(POLICY_EFFECT_ALLOW),
        rendered_principal(principal),
        json_text(action),
        json_text(resource),
    );
    if let Some((kind, key, value)) = condition {
        body.push_str(&format!(
            ",\"Condition\":{{{}:{{{}:{}}}}}",
            json_text(kind),
            json_text(key),
            json_text(value),
        ));
    }
    body.push('}');
    PolicyStatement {
        sid: Some(sid.to_string()),
        body,
    }
}

/// The principal `*` names every principal there is.
///
/// `PutPermission$Principal` is "the 12-digit Amazon Web Services account ID
/// that you are permitting ... Specify "*" to permit any account to put events",
/// and its modelled pattern `^(\d{12}|\*)$` admits exactly those two forms.
pub const WILDCARD_PRINCIPAL: &str = "*";

/// `principal` as a policy statement's `Principal` member.
///
/// The policy language writes "every principal" as the bare string `"*"` and a
/// NAMED principal as an object keyed by the kind of principal it is, so an
/// account id is an Amazon Web Services principal under `AWS`
/// (`{"AWS":"<account>"}`) while the wildcard is the string itself. A document a
/// client reads back is therefore the one the policy language defines, which is
/// what lets policy diffing compare it with the document the client submitted.
fn rendered_principal(principal: &str) -> String {
    if principal == WILDCARD_PRINCIPAL {
        return json_text(principal);
    }
    format!("{{\"AWS\":{}}}", json_text(principal))
}

/// The policy a caller supplied as one whole document, or why it is not a
/// policy document.
///
/// A policy document is an object carrying `Statement`, either one statement
/// object or a list of them. The TEXT is kept as it arrived, so
/// `DescribeEventBus` reports back exactly the document that was installed;
/// each statement is also kept beside it, with its own `Sid`, so a later grant
/// naming that id addresses it.
pub fn parse_policy_document(document: &str) -> Result<BusPolicy, String> {
    let parsed: ::serde_json::Value = ::serde_json::from_str(document)
        .map_err(|error| format!("it is not a JSON document ({error})"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| "a policy document is a JSON object".to_string())?;
    let statements = match object.get("Statement") {
        Some(::serde_json::Value::Array(items)) => items.clone(),
        Some(item @ ::serde_json::Value::Object(_)) => vec![item.clone()],
        _ => return Err("a policy document carries a Statement member".to_string()),
    };
    let mut parsed_statements = Vec::with_capacity(statements.len());
    for statement in statements {
        if !statement.is_object() {
            return Err("every statement of a policy document is a JSON object".to_string());
        }
        parsed_statements.push(PolicyStatement {
            sid: statement
                .get("Sid")
                .and_then(::serde_json::Value::as_str)
                .map(str::to_string),
            body: statement.to_string(),
        });
    }
    Ok(BusPolicy {
        statements: parsed_statements,
        supplied: Some(document.to_string()),
        version: object
            .get("Version")
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// One published event and where it was routed: the routing log `PutEvents`
/// appends to, so a caller can observe that an event reached a rule's targets.
#[derive(Debug, Clone)]
pub struct DeliveredEvent {
    pub event_id: String,
    pub bus: String,
    pub source: Option<String>,
    pub detail_type: Option<String>,
    pub detail: Option<String>,
    pub resources: Vec<String>,
    pub time: SystemTime,
    /// The names of the rules on `bus` whose `EventPattern` matched, in the
    /// bus's rule order.
    pub matched_rules: Vec<String>,
    /// The `Arn` of every target of every matched rule, in rule then target
    /// order — what the event was actually delivered to.
    pub target_arns: Vec<String>,
}

impl DeliveredEvent {
    /// Roughly how much caller-supplied text this record holds, which is what
    /// [`MAX_DELIVERED_BYTES`] bounds.
    fn retained_bytes(&self) -> usize {
        let text = [
            self.source.as_deref(),
            self.detail_type.as_deref(),
            self.detail.as_deref(),
        ]
        .iter()
        .map(|member| member.map_or(0, str::len))
        .sum::<usize>();
        text + self.event_id.len()
            + self.bus.len()
            + self.resources.iter().map(String::len).sum::<usize>()
            + self.matched_rules.iter().map(String::len).sum::<usize>()
            + self.target_arns.iter().map(String::len).sum::<usize>()
    }
}

/// What came of handing ONE routed event to ONE target of a matched rule.
///
/// [`DeliveredEvent`] says WHERE an event was routed; this says what happened
/// when it was handed over, which is a different fact and the one a client
/// observes through the target itself (a message on the queue, an execution on
/// the state machine) or through the dead-letter queue when the handover failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationOutcome {
    /// The target's own API accepted the event.
    Invoked,
    /// Nothing was handed over, and nothing was invented in its place: a target
    /// kind this emulator has no invocation for, a target asking for an input
    /// transformation, or a target service that answered `NotImplemented`.
    /// `reason` says which.
    NotAttempted(String),
    /// The invocation was attempted and refused. `code` is the condition
    /// EventBridge reports for it, `message` the detail.
    Failed { code: String, message: String },
}

/// One handover of a routed event to one target, and what came of it.
#[derive(Debug, Clone)]
pub struct TargetInvocation {
    pub event_id: String,
    /// The rule whose pattern matched, which with its bus is what the rule's
    /// invocation metrics are dimensioned by.
    pub rule: String,
    pub target_id: String,
    pub target_arn: String,
    pub outcome: InvocationOutcome,
}

/// The service principal EventBridge assumes a rule target's `RoleArn` as.
///
/// `Target$RoleArn`: "The Amazon Resource Name (ARN) of the IAM role to be used
/// for this target when the rule is triggered" — a role EventBridge is to act as
/// has to trust this principal for `sts:AssumeRole`.
pub const SERVICE_PRINCIPAL: &str = "events.amazonaws.com";

/// The condition EventBridge reports for an event it could not deliver because
/// it could not act as the role the target names at all: the role's trust policy
/// does not let [`SERVICE_PRINCIPAL`] assume it, so the delivery failed at the
/// assume-role step, before any target API call was attempted.
pub const FAILED_TO_ASSUME_ROLE: &str = "FAILED_TO_ASSUME_ROLE";

/// The condition EventBridge reports for an event whose target invocation was
/// refused for want of a permission: the role could be assumed, and the policies
/// in force on it do not allow the API call the target's invocation makes.
pub const NO_PERMISSIONS: &str = "NO_PERMISSIONS";

/// The condition EventBridge reports for a delivery that failed for a reason its
/// classification does not place: the target's service answered something that is
/// neither a refusal of the identity ([`NO_PERMISSIONS`]) nor a resource that is
/// not there ([`NO_RESOURCE`]), so what there is to go on is that answer itself,
/// which the dead letter's `ERROR_MESSAGE` carries beside this condition.
pub const UNKNOWN_CONDITION: &str = "UNKNOWN";

/// The condition EventBridge reports for a delivery whose own input was not one a
/// target may be handed: what it had to send is not a JSON document, so no call
/// was made with it.
pub const INVALID_JSON: &str = "INVALID_JSON";

/// What that delivery reports beside the condition: AWS's own sentence for an
/// input a target may not be handed, which carries nothing of the request —
/// "Invalid input for target."
///
/// Fixed text, with no variable part: the rendered input itself is not reported,
/// because it is built from the event and a dead letter carries the event
/// already.
pub const INPUT_IS_NOT_JSON: &str = "Invalid input for target.";

/// The CloudWatch namespace EventBridge's own metrics live in.
pub const METRIC_NAMESPACE: &str = "AWS/Events";

/// The EventBridge metric counting events a rule could not deliver to a target.
pub const FAILED_INVOCATIONS_METRIC: &str = "FailedInvocations";

/// The EventBridge metric counting the target invocations a rule made.
pub const INVOCATIONS_METRIC: &str = "Invocations";

/// The EventBridge metric counting the invocation attempts a rule's target
/// ACCEPTED: one per handover whose target API call went through, which is the
/// counterpart of [`FAILED_INVOCATIONS_METRIC`] on the attempt series and the
/// only way a client can tell "delivery succeeded" from "nothing is counted".
pub const SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC: &str = "SuccessfulInvocationAttempts";

/// The EventBridge metric counting the events a rule's `EventPattern` matched.
pub const MATCHED_EVENTS_METRIC: &str = "MatchedEvents";

/// The EventBridge metric counting the times a rule ran because an event matched
/// it.
pub const TRIGGERED_RULES_METRIC: &str = "TriggeredRules";

/// The dimension every per-rule metric is reported per: the rule the count
/// belongs to.
pub const RULE_NAME_DIMENSION: &str = "RuleName";

/// The dimension that narrows a rule name to ONE rule: the event bus the rule is
/// on — a rule's identity is the bus it belongs to plus its name (`rule_arn`),
/// so two buses may each hold a rule of the same name.
pub const EVENT_BUS_NAME_DIMENSION: &str = "EventBusName";

/// The `StandardUnit` a count of invocations is published in.
pub const METRIC_UNIT_COUNT: &str = "Count";

/// The `StorageResolution` a standard-resolution metric is published at, in
/// seconds: "the default is 60" (`MetricDatum$StorageResolution`).
pub const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The most rule matches ONE request counts on the AWS/Events metrics.
///
/// This emulator's OWN bound, like [`MAX_DELIVERED_EVENTS`]: what counting costs
/// is the (events x matched rules) PRODUCT, and both factors come from the
/// request, so the product itself is capped rather than each factor. A request
/// that reaches it counts no further match and says so once in the log
/// ([`count_rule_matches`]).
pub const MAX_COUNTED_MATCHES_PER_REQUEST: usize = 1_000;

/// Largest number of target invocations the log holds.
///
/// This emulator's OWN bound, like [`MAX_DELIVERED_EVENTS`]: the log is a ring,
/// so a long-running engine cannot grow it without limit. Each record holds only
/// identifiers and a refusal message, never the event text.
pub const MAX_TARGET_INVOCATIONS: usize = 10_000;

/// The `ConnectionState` of a connection whose creation is still in flight: one
/// of `CONNECTION_STATE_VALUES`, so `ListConnections` admits it as a filter. No
/// connection this emulator writes rests in it — credentials that ARE the
/// authorization settle it in [`CONNECTION_STATE_AUTHORIZED`] as they are
/// accepted, and one still to obtain a token is [`CONNECTION_STATE_AUTHORIZING`].
pub const CONNECTION_STATE_CREATING: &str = "CREATING";

/// The `ConnectionState` a connection whose credentials are in place reaches.
pub const CONNECTION_STATE_AUTHORIZED: &str = "AUTHORIZED";

/// The `ConnectionState` `DeauthorizeConnection` moves a connection into while
/// its authorization parameters are being removed.
pub const CONNECTION_STATE_DEAUTHORIZING: &str = "DEAUTHORIZING";

/// The `ConnectionState` a deauthorized connection settles in.
pub const CONNECTION_STATE_DEAUTHORIZED: &str = "DEAUTHORIZED";

/// Why a connection whose authorization parameters `DeauthorizeConnection` took
/// away holds none, as its `StateReason`: fixed wording with no variable part.
pub const DEAUTHORIZED_BY_REQUEST: &str = "The authorization parameters were removed from this \
                                           connection by a DeauthorizeConnection operation.";

/// The `ConnectionState` `UpdateConnection` moves a connection into while its
/// new settings are being applied.
pub const CONNECTION_STATE_UPDATING: &str = "UPDATING";

/// The `ConnectionState` `DeleteConnection` moves a connection into while it is
/// being removed.
pub const CONNECTION_STATE_DELETING: &str = "DELETING";

/// The `ConnectionState` a connection is in while EventBridge is obtaining the
/// token it needs before it can authorize anything.
pub const CONNECTION_STATE_AUTHORIZING: &str = "AUTHORIZING";

/// The `ConnectionAuthorizationType` whose credentials are not the
/// authorization: EventBridge exchanges them for a token at the connection's
/// authorization endpoint ("OAUTH tokens are refreshed when a 401 or 407
/// response is returned", `CreateConnection$AuthorizationType`).
pub const OAUTH_CLIENT_CREDENTIALS_AUTHORIZATION: &str = "OAUTH_CLIENT_CREDENTIALS";

/// Whether a connection of `authorization_type` has to obtain a token before it
/// can authorize an invocation.
///
/// `API_KEY` and `BASIC` credentials ARE the authorization — EventBridge sends
/// them with the invocation — so a connection holding them is authorized as soon
/// as it is written. `OAUTH_CLIENT_CREDENTIALS` credentials are exchanged for a
/// token at the connection's own authorization endpoint first, so such a
/// connection is not authorized until that exchange has happened
/// ([`CONNECTION_STATE_AUTHORIZING`] is the state it is in until then).
pub fn needs_authorization_exchange(authorization_type: &str) -> bool {
    authorization_type == OAUTH_CLIENT_CREDENTIALS_AUTHORIZATION
}

/// How long ONE authorization exchange may wait on a connection's authorization
/// endpoint.
///
/// This emulator's OWN bound, not a duration AWS publishes: the exchange is made
/// through the in-process transport, and a request that never answers must not
/// hold the call that started it open.
pub const AUTHORIZATION_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

/// The HTTP method an authorization exchange uses when the connection names
/// none: a token request posts its client credentials.
pub const AUTHORIZATION_EXCHANGE_METHOD: &str = "POST";

/// What a connection reports as its `StateReason` when it holds no
/// authorization endpoint to obtain a token from at all — which is what a
/// connection `DeauthorizeConnection` stripped is left with.
pub const NO_AUTHORIZATION_ENDPOINT: &str =
    "The connection names no authorization endpoint to obtain a token from.";

/// Obtain the token an OAuth connection needs: the client-credentials grant at
/// the connection's own authorization endpoint
/// ([`oauth::client_credentials_token`]), answering the access token it issued
/// and `Err(reason)` — the `StateReason` the connection then reports — when it
/// issued none.
///
/// The rule AWS follows: an `OAUTH_CLIENT_CREDENTIALS` connection's client
/// credentials are exchanged for a token at its `AuthorizationEndpoint` with its
/// `HttpMethod`, and the connection can authorize an invocation only once that
/// exchange has succeeded; an endpoint that answers anything else, or nothing at
/// all, leaves the connection DEAUTHORIZED, which in turn leaves every API
/// destination invoking through it INACTIVE
/// ([`ComAmazonawsEventbridgeAwseventsState::api_destination_state`]).
///
/// `credential` is the credential half of the connection's parameters, which is
/// where the client SECRET the grant has to carry lives: the half a read reports
/// withholds it ([`reported_http_parameters`]), so the exchange is never made
/// from that half.
pub async fn authorization_exchange(
    transport: &::aws_smithy_runtime_api::client::http::SharedHttpConnector,
    credential: Option<&oauth::OAuthCredential>,
) -> Result<String, String> {
    // A connection left holding no authorization endpoint — one
    // `DeauthorizeConnection` stripped — has nothing to obtain a token from.
    let Some(credential) = credential else {
        return Err(NO_AUTHORIZATION_ENDPOINT.to_string());
    };
    oauth::client_credentials_token(transport, credential).await
}

/// What a connection reports as its `StateReason` when EventBridge could not
/// reach the endpoint it had to call: "Could not connect to the specified
/// endpoint."
///
/// The wording EventBridge states for an endpoint nothing answered at, whatever
/// the endpoint was.
pub const ENDPOINT_NOT_REACHED: &str = "Could not connect to the specified endpoint.";

/// The error types that are the TRANSPORT's own refusal rather than an answer
/// from an endpoint: this emulator serves nothing at that URL, or more than one
/// of its services claims it. Either way the request reached no endpoint.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// How much of a non-2xx answer is read to tell the transport's own refusal from
/// an endpoint's answer. The envelope is two short fields; anything larger is an
/// answer from something that served the request, so it is not read at all.
const MAX_REFUSAL_ENVELOPE_BYTES: usize = 4 * 1024;

/// Whether `body` is the in-process transport's OWN refusal of the request —
/// this emulator serves nothing at the endpoint that was called — rather than an
/// answer from an endpoint that served it.
///
/// The refusal is told by the error type its envelope carries
/// ([`TRANSPORT_REFUSALS`]); the bridge the request crosses keeps the body but
/// not the headers, so the envelope is where that type can be read at all.
/// Written once, so every request this service makes on the account's behalf —
/// an API destination invocation, a connection's authorization exchange — tells
/// "nothing answered" from "an endpoint answered" the same way.
pub fn is_transport_refusal(body: &[u8]) -> bool {
    if body.len() > MAX_REFUSAL_ENVELOPE_BYTES {
        return false;
    }
    let Ok(envelope) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
        return false;
    };
    let Some(error_type) = envelope.get("__type").and_then(::serde_json::Value::as_str) else {
        return false;
    };
    // The type may carry a shape namespace; the code is its last segment.
    let code = error_type.rsplit('#').next().unwrap_or(error_type);
    TRANSPORT_REFUSALS.contains(&code)
}

/// The scheme an authorization endpoint carries: `AuthorizationEndpoint` is an
/// `HttpsEndpoint`, so the exchange is made over TLS.
const HTTPS_SCHEME: &str = "https://";

/// What a refusal states about an endpoint member whose value is not an HTTPS
/// URL: the endpoint AS SUBMITTED, in single quotes, and what a valid value is.
///
/// EventBridge words every such refusal the same way, whichever member carried
/// the endpoint, so it is written once here rather than at the member that
/// checks it. The only variable part is the value the request supplied.
fn invalid_https_endpoint(endpoint: &str) -> String {
    format!("Endpoint '{endpoint}' is invalid, please provide a valid HTTPS endpoint URL.")
}

/// Why `endpoint` is not an authorization endpoint EventBridge accepts, or
/// `None` when it is.
///
/// The member is modelled as an `HttpsEndpoint` ("the URL to the authorization
/// endpoint when OAuth is specified as the authorization type"), and its
/// modelled pattern admits any URL characters, so the scheme is checked here: a
/// value that is not an `https` URL is refused rather than stored as an endpoint
/// no token request could be made to.
///
/// The refusal names the member whose value was rejected —
/// `AuthorizationEndpoint`, the leaf, not the `AuthParameters` block that
/// carries it — so a client that surfaces the rejected parameter points a user
/// at the field it has to change.
///
/// This is a FIDELITY check on a submitted value, and nothing else: it is not
/// what keeps a stored endpoint from reaching a host. An endpoint this store
/// holds is only ever dispatched through the in-process emulator transport
/// ([`smithy_server_runtime::OperationContext::callback`]'s connector, which is
/// the emulator's own router), so it reaches only what this emulator serves
/// itself — no socket is opened and no name is resolved, whatever a caller
/// stored, and an endpoint nothing here serves answers the transport's own
/// refusal ([`is_transport_refusal`]). An `https` value is therefore accepted on
/// the same terms AWS accepts it, without the destination being reachable.
pub fn authorization_endpoint_violation(endpoint: &str) -> Option<String> {
    let scheme_matches = endpoint
        .get(..HTTPS_SCHEME.len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(HTTPS_SCHEME));
    (!scheme_matches)
        .then(|| parameter_violation("AuthorizationEndpoint", &invalid_https_endpoint(endpoint)))
}

/// Record what a connection's authorization exchange came to: the state the
/// clock owes the connection `name` is what that exchange answered, and the
/// token it issued is what the connection authorizes its invocations with.
///
/// The second half of the write a connection of an authorization type that needs
/// a token makes ([`authorization_exchange`]). It is applied after the exchange
/// so no store lock is held across it: while the exchange is outstanding the
/// connection is owed the state of one that obtained no token, and this replaces
/// that with the outcome — keeping the instant the transition was already due,
/// or falling due at once when the clock has already moved the connection on.
/// A connection that is gone, or on its way out, is left alone: a delete already
/// decided its last state.
pub fn record_authorization(
    state: &::engine::ServiceState<ComAmazonawsEventbridgeAwseventsState>,
    name: &str,
    authorized: Result<String, String>,
) {
    let (transition, token) = match authorized {
        Ok(token) => (
            ConnectionTransition::to(CONNECTION_STATE_AUTHORIZED),
            Some(token),
        ),
        Err(reason) => (
            ConnectionTransition::to_because(CONNECTION_STATE_DEAUTHORIZED, reason),
            None,
        ),
    };
    let mut store = state.lock();
    let Some(connection) = store.connection_mut(name) else {
        return;
    };
    if connection.is_being_removed() {
        return;
    }
    // The token the exchange issued lives in the connection's secret, which no
    // read reports and every invocation through the connection carries; an
    // exchange that issued none leaves the connection holding none, so no
    // invocation through it is authorized.
    connection.secret.oauth_token = token;
    match connection.pending.as_mut() {
        Some(pending) => pending.to = transition,
        None => {
            connection.pending = Some(PendingTransition {
                due: SystemTime::now(),
                to: transition,
            })
        }
    }
}

/// How long after the call that starts it a connection's next state becomes due.
///
/// A connection AWS answers in a non-terminal state (`CREATING`, `DEAUTHORIZING`,
/// `DELETING`) reaches its next state a moment later, not instantly, so a caller
/// acting straight after the call still observes the transient state. AWS
/// publishes no duration for it; this is the emulator's own span, chosen inside
/// the 250–750 ms window the project fixes for a clock-owed transition, and it is
/// stored as a DUE INSTANT rather than waited for.
pub const CONNECTION_TRANSITION: Duration = Duration::from_millis(400);

/// Largest number of connections this store holds.
///
/// This emulator's OWN bound, not a quota AWS publishes: a `CreateConnection`
/// past it answers the modelled `LimitExceededException` instead of growing
/// memory without limit. It is far above any realistic fixture's needs.
pub const MAX_CONNECTIONS: usize = 1_000;

/// The next state a connection's clock owes it.
#[derive(Debug, Clone)]
pub enum ConnectionTransition {
    /// The connection moves to this `ConnectionState`, carrying the
    /// `StateReason` that state has — `None` for a state that needs none, which
    /// clears whatever reason the connection reported before.
    To {
        state: String,
        reason: Option<String>,
    },
    /// The connection is removed from the store.
    Removed,
}

impl ConnectionTransition {
    /// A move to `state` with no reason to report.
    pub fn to(state: &str) -> Self {
        Self::To {
            state: state.to_string(),
            reason: None,
        }
    }

    /// A move to `state` the connection reports `reason` for.
    pub fn to_because(state: &str, reason: String) -> Self {
        Self::To {
            state: state.to_string(),
            reason: Some(reason),
        }
    }
}

/// The transition a connection's clock owes, and when it falls due.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub to: ConnectionTransition,
}

/// A connection as this emulator persists it.
///
/// "A connection defines the authorization type and credentials to use for
/// authorization with an API destination HTTP endpoint" (`CreateConnection`). The
/// credentials the request carried are NOT kept on the record: EventBridge puts
/// them in a secret and reports only the non-secret half of the parameters back
/// ([`ConnectionAuthResponseParameters`] carries a user name, an API key NAME and
/// a client id, never a password, key value or client secret), so this record
/// holds exactly what a read may report.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    pub name: String,
    /// Minted ONCE, when the connection is created, and never recomputed: the
    /// ARN carries a unique id segment, so a connection deleted and recreated
    /// under one name has a different ARN.
    pub arn: String,
    /// The secret the connection's credentials are held in, named after the
    /// connection itself. The secret VALUE is never reported, and no caller can
    /// read it through this service.
    pub secret_arn: String,
    pub description: Option<String>,
    /// One of `ConnectionAuthorizationType`'s values.
    pub authorization_type: String,
    /// The authorization parameters as a read reports them: every secret member
    /// the request carried is withheld.
    pub auth_parameters: ConnectionAuthResponseParameters,
    /// The credential half of those parameters — what the secret `secret_arn`
    /// names holds, and what an INVOCATION through this connection carries
    /// ([`invocation::ConnectionSecret`]). No operation of this service reports
    /// it.
    pub secret: invocation::ConnectionSecret,
    /// The resource configuration a connection to a PRIVATE api names
    /// (`CreateConnection$InvocationConnectivityParameters`), as the request gave
    /// it.
    ///
    /// It is stored but NOT reported: `DescribeConnection`'s connectivity
    /// parameters carry the ARN of the resource ASSOCIATION EventBridge made
    /// with that configuration beside the configuration's own ARN, and this
    /// emulator makes no such association — so there is none to name, and a read
    /// reports no connectivity parameters rather than an ARN that names
    /// something else.
    pub invocation_resource_configuration_arn: Option<String>,
    pub kms_key_identifier: Option<String>,
    /// One of `ConnectionState`'s values (`CONNECTION_STATE_VALUES`).
    pub state: String,
    pub state_reason: Option<String>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    pub last_authorized: Option<SystemTime>,
    /// The transition the clock owes this connection, or `None` once it has
    /// settled.
    pub pending: Option<PendingTransition>,
}

impl ConnectionRecord {
    /// Whether this connection is already on its way OUT: the transition the
    /// clock owes it REMOVES it, so there is no later state for another call to
    /// put it in.
    ///
    /// This is the one conflict a write of the connection slice refuses. Every
    /// other non-terminal state (`CREATING`, `DEAUTHORIZING`) is a connection
    /// that still exists and can be written: a client that creates a connection
    /// and deletes it at once is not refused.
    pub fn is_being_removed(&self) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            matches!(pending.to, ConnectionTransition::Removed)
        })
    }

    /// The `ConnectionState` this connection has SETTLED on: the state its clock
    /// already owes it while a transition is pending, and its current state once
    /// it has settled — `None` when the transition the clock owes it REMOVES the
    /// connection, so there is no state it comes to rest in at all.
    ///
    /// `CREATING`, `UPDATING` and `AUTHORIZING` say a change is in flight; they
    /// do not say whether the connection ends up holding authorization. The state
    /// the pending transition names is that outcome, already decided by the
    /// credentials the connection holds — or by the token exchange that answered
    /// ([`record_authorization`]) — and merely not materialised yet
    /// ([`ComAmazonawsEventbridgeAwseventsState::settle_connections`] is what
    /// materialises it). A reader that has no in-flight value of its own to
    /// report reads off this rather than off the transient
    /// ([`ComAmazonawsEventbridgeAwseventsState::api_destination_state`]).
    pub fn settled_state(&self) -> Option<&str> {
        match self.pending.as_ref().map(|pending| &pending.to) {
            Some(ConnectionTransition::Removed) => None,
            Some(ConnectionTransition::To { state, .. }) => Some(state.as_str()),
            None => Some(self.state.as_str()),
        }
    }

    /// The `Connection` summary `ListConnections` reports: the members of the
    /// stored record the summary shape carries, read back from it — so a listing
    /// and `DescribeConnection` cannot disagree about any of them.
    pub fn to_summary(&self) -> Connection {
        Connection {
            connection_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            connection_state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            authorization_type: Some(self.authorization_type.clone()),
            creation_time: Some(to_wire_timestamp(self.created)),
            last_modified_time: Some(to_wire_timestamp(self.last_modified)),
            last_authorized_time: self.last_authorized.map(to_wire_timestamp),
        }
    }

    /// Every member `DescribeConnection` reports, read back from the stored
    /// record — so a read and the write that created the connection cannot
    /// disagree about any of them.
    pub fn to_describe_output(&self) -> DescribeConnectionOutput {
        DescribeConnectionOutput {
            connection_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            // See `invocation_resource_configuration_arn`: this emulator makes
            // no resource association, so there is none to report.
            invocation_connectivity_parameters: None,
            connection_state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            authorization_type: Some(self.authorization_type.clone()),
            secret_arn: Some(self.secret_arn.clone()),
            kms_key_identifier: self.kms_key_identifier.clone(),
            auth_parameters: Some(self.auth_parameters.clone()),
            creation_time: Some(to_wire_timestamp(self.created)),
            last_modified_time: Some(to_wire_timestamp(self.last_modified)),
            last_authorized_time: self.last_authorized.map(to_wire_timestamp),
        }
    }
}

/// `parameters` as a READ reports them: every value the write flagged SECRET is
/// withheld.
///
/// "Specifies whether the value is a secret" (`IsValueSecret`, a member of each
/// of the header, query-string and body parameters a connection sends with every
/// invocation). A value declared secret lives in the connection's secret, which
/// `SecretArn` names and no operation of this service reads back, so it is not
/// part of what a read may report — exactly as the response authorization shapes
/// carry a user name, an API key NAME and a client id and no password, key value
/// or client secret. The key and the flag ARE reported: they say which parameter
/// the connection sends and that its value is held in that secret. A value the
/// write did NOT flag secret is reported as it was given.
///
/// Written once and applied by every write of a connection, so no secret the
/// request carried is ever stored on the record a read serializes.
pub fn reported_http_parameters(parameters: &ConnectionHttpParameters) -> ConnectionHttpParameters {
    let withheld = |value: &Option<String>, secret: bool| -> Option<String> {
        if secret {
            None
        } else {
            value.clone()
        }
    };
    ConnectionHttpParameters {
        header_parameters: parameters.header_parameters.as_ref().map(|list| {
            list.iter()
                .map(|parameter| ConnectionHeaderParameter {
                    key: parameter.key.clone(),
                    value: withheld(&parameter.value, parameter.is_value_secret),
                    is_value_secret: parameter.is_value_secret,
                })
                .collect()
        }),
        query_string_parameters: parameters.query_string_parameters.as_ref().map(|list| {
            list.iter()
                .map(|parameter| ConnectionQueryStringParameter {
                    key: parameter.key.clone(),
                    value: withheld(&parameter.value, parameter.is_value_secret),
                    is_value_secret: parameter.is_value_secret,
                })
                .collect()
        }),
        body_parameters: parameters.body_parameters.as_ref().map(|list| {
            list.iter()
                .map(|parameter| ConnectionBodyParameter {
                    key: parameter.key.clone(),
                    value: withheld(&parameter.value, parameter.is_value_secret),
                    is_value_secret: parameter.is_value_secret,
                })
                .collect()
        }),
    }
}

/// The settled `ArchiveState` of an archive that is capturing events.
///
/// An archive is provisioned by the call that writes it: `CreateArchive` and
/// `UpdateArchive` both answer the archive in this state, and every reader
/// (`DescribeArchive`, `ListArchives`) reports the state stored for the archive,
/// so a client polling for a terminal `ArchiveState` is already done when the
/// write returns. "Incoming events might not immediately start being sent to the
/// archive" (`CreateArchive`) is about ingestion latency, not about a transient
/// state a reader observes.
pub const ARCHIVE_STATE_ENABLED: &str = "ENABLED";

/// Largest number of archives this store holds.
///
/// This emulator's OWN bound, like [`MAX_CONNECTIONS`]: a `CreateArchive` past it
/// answers the modelled `LimitExceededException` instead of growing memory
/// without limit. It is far above any realistic fixture's needs.
pub const MAX_ARCHIVES: usize = 100;

/// How many captured events one archive keeps.
///
/// The emulator's OWN bound, for the reason [`MAX_DELIVERED_EVENTS`] gives: an
/// archive is appended to by every matching `PutEvents` entry and nothing in the
/// API deletes one event, so without a bound a caller could grow the process
/// heap without end. The store is a ring, oldest evicted first.
pub const MAX_ARCHIVED_EVENTS: usize = 10_000;

/// How many bytes of captured event text one archive keeps, beside
/// [`MAX_ARCHIVED_EVENTS`] — a record count alone does not bound memory, for the
/// reason [`MAX_DELIVERED_BYTES`] gives.
pub const MAX_ARCHIVED_BYTES: usize = 8_000_000;

/// How many seconds a day of `RetentionDays` is.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// One event an archive captured, as the archive holds it.
///
/// The whole envelope is kept, because that is what a replay of the archive
/// re-publishes, and its byte length is what the archive reports as `SizeBytes`.
#[derive(Debug, Clone)]
pub struct ArchivedEvent {
    /// The `EventId` `PutEvents` minted for the event.
    pub event_id: String,
    /// The event envelope, as it was routed.
    pub envelope: String,
    /// The event's own time, which is what retention is measured from.
    pub time: SystemTime,
}

/// An archive as this emulator persists it.
///
/// "Creates an archive of events with the specified settings" (`CreateArchive`):
/// the archive is bound to ONE event bus (`EventSourceArn`), optionally filters
/// what it captures by `EventPattern` ("If you do not specify a pattern to filter
/// events sent to the archive, all events are sent to the archive"), and holds
/// what it captured so `DescribeArchive`'s `EventCount` and `SizeBytes` are read
/// off the events themselves rather than asserted.
#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub name: String,
    /// Minted ONCE, when the archive is created, and never recomputed.
    pub arn: String,
    /// The `EventBusArn` the request named, reported back as given.
    pub event_source_arn: String,
    /// The name of the bus that ARN named, which is what capture resolves on.
    pub event_bus_name: String,
    pub description: Option<String>,
    pub event_pattern: Option<String>,
    pub retention_days: Option<i32>,
    pub kms_key_identifier: Option<String>,
    /// One of `ArchiveState`'s values.
    pub state: String,
    pub state_reason: Option<String>,
    pub created: SystemTime,
    /// What the archive captured, oldest first. A ring bounded by
    /// [`MAX_ARCHIVED_EVENTS`] and [`MAX_ARCHIVED_BYTES`] — see
    /// [`Self::capture`], the one place it grows.
    pub events: VecDeque<ArchivedEvent>,
    /// How many bytes the captured envelopes hold, kept in step with the ring so
    /// the byte bound costs nothing to check.
    captured_bytes: usize,
}

impl ArchiveRecord {
    /// A newly created archive on `event_bus_name`: settled in
    /// [`ARCHIVE_STATE_ENABLED`] and capturing from this instant on, holding
    /// nothing yet. The optional members the request carried (`Description`,
    /// `EventPattern`, `RetentionDays`, `KmsKeyIdentifier`) are written onto the
    /// record by its creator.
    pub fn new(
        name: &str,
        arn: String,
        event_source_arn: String,
        event_bus_name: &str,
        created: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn,
            event_source_arn,
            event_bus_name: event_bus_name.to_string(),
            description: None,
            event_pattern: None,
            retention_days: None,
            kms_key_identifier: None,
            state: ARCHIVE_STATE_ENABLED.to_string(),
            state_reason: None,
            created,
            events: VecDeque::new(),
            captured_bytes: 0,
        }
    }

    /// Whether this archive captures published events at all.
    ///
    /// Only an archive in [`ARCHIVE_STATE_ENABLED`] keeps what its bus routes; a
    /// `DISABLED` or failed archive keeps nothing. An `UpdateArchive` does not
    /// interrupt capture: it changes WHAT the archive keeps rather than stopping
    /// it keeping anything, so an archive captures by the pattern the update
    /// wrote from the instant that update returns.
    pub fn captures_events(&self) -> bool {
        self.state == ARCHIVE_STATE_ENABLED
    }

    /// Append one captured event, evicting the oldest first so the archive never
    /// holds more than [`MAX_ARCHIVED_EVENTS`] events or [`MAX_ARCHIVED_BYTES`]
    /// bytes of event text. The ONE place the ring grows.
    pub fn capture(&mut self, event: ArchivedEvent) {
        let size = event.envelope.len();
        while self.events.len() >= MAX_ARCHIVED_EVENTS
            || (!self.events.is_empty()
                && self.captured_bytes.saturating_add(size) > MAX_ARCHIVED_BYTES)
        {
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.captured_bytes = self.captured_bytes.saturating_sub(evicted.envelope.len());
        }
        self.captured_bytes = self.captured_bytes.saturating_add(size);
        self.events.push_back(event);
    }

    /// Drop every captured event the archive's `RetentionDays` has already
    /// expired at `now`, and answer how many were dropped.
    ///
    /// "The number of days to retain events for. Default value is 0. If set to 0,
    /// events are retained indefinitely" (`CreateArchive$RetentionDays`), so a
    /// retention of zero or none expires nothing.
    pub fn expire_captured(&mut self, now: SystemTime) -> usize {
        let days = self.retention_days.unwrap_or_default();
        let Ok(days) = u64::try_from(days) else {
            return 0;
        };
        if days == 0 {
            return 0;
        }
        let retention = Duration::from_secs(days * SECONDS_PER_DAY);
        let mut dropped = 0usize;
        while let Some(oldest) = self.events.front() {
            let expired = now
                .duration_since(oldest.time)
                .is_ok_and(|age| age > retention);
            if !expired {
                break;
            }
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.captured_bytes = self.captured_bytes.saturating_sub(evicted.envelope.len());
            dropped += 1;
        }
        dropped
    }

    /// Every member `DescribeArchive` reports, read back from the stored record —
    /// `EventCount` and `SizeBytes` from the captured events themselves, so a
    /// read and what the archive holds cannot disagree.
    pub fn to_describe_output(&self) -> DescribeArchiveOutput {
        DescribeArchiveOutput {
            archive_arn: Some(self.arn.clone()),
            archive_name: Some(self.name.clone()),
            event_source_arn: Some(self.event_source_arn.clone()),
            description: self.description.clone(),
            event_pattern: self.event_pattern.clone(),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            kms_key_identifier: self.kms_key_identifier.clone(),
            retention_days: self.retention_days,
            size_bytes: i64::try_from(self.captured_bytes).unwrap_or(i64::MAX),
            event_count: i64::try_from(self.events.len()).unwrap_or(i64::MAX),
            creation_time: Some(to_wire_timestamp(self.created)),
        }
    }

    /// The `Archive` summary a LISTING reports, projected from the SAME stored
    /// record [`Self::to_describe_output`] reports, so a listing and a read of one
    /// archive cannot disagree. The summary shape carries no `Description`,
    /// `EventPattern` or `KmsKeyIdentifier` — those are read with
    /// `DescribeArchive`.
    pub fn to_summary(&self) -> ArchiveSummary {
        ArchiveSummary {
            archive_name: Some(self.name.clone()),
            event_source_arn: Some(self.event_source_arn.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            retention_days: self.retention_days,
            size_bytes: i64::try_from(self.captured_bytes).unwrap_or(i64::MAX),
            event_count: i64::try_from(self.events.len()).unwrap_or(i64::MAX),
            creation_time: Some(to_wire_timestamp(self.created)),
        }
    }
}

/// The `ApiDestinationState` of a destination whose connection can authorize an
/// invocation.
pub const API_DESTINATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ApiDestinationState` of a destination whose connection cannot.
pub const API_DESTINATION_STATE_INACTIVE: &str = "INACTIVE";

/// The `ApiDestinationState` a destination takes from the `ConnectionState` of
/// the connection it invokes through.
///
/// "The destination endpoint must support the authorization type specified for
/// the connection" (`CreateApiDestination$ConnectionArn`): a destination can be
/// invoked only while its connection holds the authorization that invocation
/// carries, so a connection holding one leaves the destination `ACTIVE` and one
/// holding none makes it `INACTIVE`. Written once, so the write that creates a
/// destination, the write that repoints it and the connection transition that
/// settles under it all read one derivation.
pub fn api_destination_state_of(connection_state: &str) -> String {
    if matches!(
        connection_state,
        CONNECTION_STATE_AUTHORIZED | CONNECTION_STATE_ACTIVE
    ) {
        API_DESTINATION_STATE_ACTIVE.to_string()
    } else {
        API_DESTINATION_STATE_INACTIVE.to_string()
    }
}

/// The `ConnectionState` of a connection that is in use and authorized.
pub const CONNECTION_STATE_ACTIVE: &str = "ACTIVE";

/// The `InvocationRateLimitPerSecond` an API destination takes when the request
/// names none.
///
/// The EventBridge quota "Invocation rate limit per second" for an API
/// destination (default 300), which is also the default
/// `CreateApiDestination$InvocationRateLimitPerSecond` the service applies, so a
/// read reports the rate the destination is actually invoked at.
pub const DEFAULT_API_DESTINATION_RATE_LIMIT: i32 = 300;

/// Largest `InvocationRateLimitPerSecond` an API destination may be created with.
///
/// The same EventBridge quota, which is an account LIMIT as well as the default:
/// a create asking to be invoked faster than the account may invoke is refused
/// with the modelled `LimitExceededException` rather than stored as a rate
/// nothing would honour.
pub const MAX_API_DESTINATION_RATE_LIMIT: i32 = DEFAULT_API_DESTINATION_RATE_LIMIT;

/// Largest number of API destinations this store holds.
///
/// This emulator's OWN bound, like [`MAX_CONNECTIONS`]: a create past it answers
/// the modelled `LimitExceededException` instead of growing memory without limit.
pub const MAX_API_DESTINATIONS: usize = 100;

/// An API destination as this emulator persists it.
///
/// "An API destination ... is an HTTP invocation endpoint configured as a target
/// for events" (`CreateApiDestination`): the endpoint, the method and the
/// connection EventBridge authorizes the invocation with, beside the
/// `ApiDestinationState` that says whether an invocation can be authorized at
/// all ([`ComAmazonawsEventbridgeAwseventsState::api_destination_state`]).
#[derive(Debug, Clone)]
pub struct ApiDestinationRecord {
    pub name: String,
    /// Minted ONCE, when the destination is created, and never recomputed.
    pub arn: String,
    pub description: Option<String>,
    /// The `ConnectionArn` the request named, which is also what resolves the
    /// connection whose authorization the state is taken from.
    pub connection_arn: String,
    pub invocation_endpoint: String,
    /// One of `ApiDestinationHttpMethod`'s values.
    pub http_method: String,
    /// The rate the destination is invoked at, defaulted to
    /// [`DEFAULT_API_DESTINATION_RATE_LIMIT`] when the request named none.
    pub invocation_rate_limit_per_second: i32,
    /// One of `ApiDestinationState`'s values, taken from the connection's own
    /// authorization ([`api_destination_state_of`]) by the write that created or
    /// repointed this destination and by the connection transition that settles
    /// under it
    /// ([`ComAmazonawsEventbridgeAwseventsState::settle_connections`]) — so a
    /// destination whose connection is DELETED from under it keeps the state it
    /// had, the connection reference merely dangling.
    pub state: String,
    pub created: SystemTime,
    pub last_modified: SystemTime,
}

/// `arn:aws:events:<region>:<account>:archive/<name>`.
///
/// `ArchiveArn`'s modelled pattern is
/// `^arn:aws([a-z]|\-)*:events:([a-z]|\d|\-)*:([0-9]{12})?:.+\/.+$`, so the
/// resource is the archive's own name under this service's `archive/` type.
pub fn archive_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("archive/{name}"))
}

/// The name of the archive an `ArchiveArn` names, or `None` when the text is not
/// an archive ARN at all.
///
/// The counterpart of [`archive_arn`], and the ONE place an `ArchiveArn` is read
/// back: the name is the whole remainder after the `archive/` segment, so an ARN
/// this service minted resolves to the archive it was minted for.
pub fn archive_name_in_arn(arn: &str) -> Option<&str> {
    let (prefix, name) = arn.split_once(":archive/")?;
    (prefix.starts_with("arn:") && !name.is_empty()).then_some(name)
}

/// `arn:aws:events:<region>:<account>:replay/<name>`.
///
/// `ReplayArn`'s modelled pattern is
/// `^arn:aws([a-z]|\-)*:events:([a-z]|\d|\-)*:([0-9]{12})?:.+\/[\.\-_A-Za-z0-9]+$`,
/// so the resource is the replay's own name under this service's `replay/` type.
pub fn replay_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("replay/{name}"))
}

/// `arn:aws:events:<region>:<account>:api-destination/<name>/<id>`.
///
/// `ApiDestinationArn`'s modelled pattern is
/// `^arn:aws([a-z]|\-)*:events:([a-z]|\d|\-)*:([0-9]{12})?:api-destination\/[\.\-_A-Za-z0-9]+\/[\-A-Za-z0-9]+$`,
/// so the resource carries the destination's NAME and then a unique id segment,
/// exactly as a connection's ARN does ([`connection_arn`]).
pub fn api_destination_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("api-destination/{name}/{id}"),
    )
}

/// The name of the API destination an `ApiDestinationArn` names, or `None` when
/// the text is not one at all.
///
/// The counterpart of [`api_destination_arn`], and the ONE place an
/// `ApiDestinationArn` is read back: the name is the segment between the
/// `api-destination/` resource type and the unique id that follows it, which is
/// what [`ComAmazonawsEventbridgeAwseventsState::api_destination`] holds the
/// record under.
pub fn api_destination_name_in_arn(arn: &str) -> Option<&str> {
    let (prefix, resource) = arn.split_once(":api-destination/")?;
    let (name, id) = resource.split_once('/')?;
    (prefix.starts_with("arn:") && !name.is_empty() && !id.is_empty()).then_some(name)
}

/// The name of the event bus an `EventBusArn` names, or `None` when the text is
/// not an event-bus ARN at all.
///
/// Unlike [`effective_bus_name`], which accepts a NAME or an ARN and defaults to
/// the account's `default` bus, this reads a member modelled as an `EventBusArn`
/// — so text that names no event bus is a bad request rather than a silent read
/// of the default bus. The name is the whole remainder after the `event-bus/`
/// segment, for the reason [`effective_bus_name`] gives.
pub fn event_bus_name_in_arn(arn: &str) -> Option<&str> {
    let (prefix, name) = arn.split_once(":event-bus/")?;
    (prefix.starts_with("arn:") && !name.is_empty()).then_some(name)
}

/// The prefix every partner event source name carries.
///
/// `EventSourceName` is modelled `pattern ^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$`,
/// so a name is this prefix followed by at least two `/`-separated segments:
/// "partner event source names follow this format:
/// `partner_name/event_namespace/event_name`" (`CreatePartnerEventSource`).
pub const PARTNER_EVENT_SOURCE_PREFIX: &str = "aws.partner/";

/// `EventSourceState`'s value for a source whose matching event bus does not
/// exist yet, or exists but is deactivated: "If it is PENDING, either you
/// haven't yet created a matching event bus, or that event bus is deactivated"
/// (`DescribeEventSource$State`).
pub const EVENT_SOURCE_STATE_PENDING: &str = "PENDING";

/// `EventSourceState`'s value for a source whose matching event bus exists and
/// is receiving events: "If it is ACTIVE, you have already created a matching
/// event bus for this event source, and that event bus is active"
/// (`DescribeEventSource$State`).
pub const EVENT_SOURCE_STATE_ACTIVE: &str = "ACTIVE";

/// `EventSourceState`'s value for a source the partner has deleted while the
/// customer's matching bus still exists: "If it is DELETED, you have created a
/// matching event bus, but the event source has since been deleted"
/// (`DescribeEventSource$State`).
pub const EVENT_SOURCE_STATE_DELETED: &str = "DELETED";

/// How long a partner event source may stay `PENDING` before it is gone.
///
/// "When you deactivate a partner event source, the source goes into PENDING
/// state. If it remains in PENDING state for more than two weeks, it is deleted"
/// (`DeactivateEventSource`), and `ExpirationTime` is "the date and time that the
/// event source will expire, if the Amazon Web Services account doesn't create a
/// matching event bus for it" (`PartnerEventSourceAccount$ExpirationTime`) — the
/// same window, counted from the instant the source entered `PENDING`. It is
/// stored as a DUE INSTANT and never waited for.
pub const PARTNER_EVENT_SOURCE_EXPIRY: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Largest number of partner event sources this store holds.
///
/// This emulator's OWN bound, not a quota AWS publishes: a
/// `CreatePartnerEventSource` past it answers the modelled
/// `LimitExceededException` instead of growing memory without limit. It is far
/// above any realistic fixture's needs.
pub const MAX_PARTNER_EVENT_SOURCES: usize = 1_000;

/// A partner event source as this emulator persists it.
///
/// "A partner event source is created by an SaaS partner. If a customer creates a
/// partner event bus that matches this event source, that Amazon Web Services
/// account can receive events from the partner's applications or services"
/// ([`crate::ops::types::PartnerEventSource`]). The source is a SHARED resource:
/// the partner authors it ([`ComAmazonawsEventbridgeAwseventsState::partner_sources`])
/// and exactly one customer account may match it with a bus — "each partner event
/// source can be used by one Amazon Web Services account to create a matching
/// partner event bus in that Amazon Web Services account"
/// (`CreatePartnerEventSource`) — which is why [`Self::account`] is one account
/// and not a set.
///
/// The reported `EventSourceState` is DERIVED from this record and the bus slice
/// ([`ComAmazonawsEventbridgeAwseventsState::event_source_state`]) rather than
/// stored, so a source and the bus matched with it can never disagree about it.
#[derive(Debug, Clone)]
pub struct PartnerEventSourceRecord {
    /// The whole source name, `aws.partner/<partner_name>/<namespace>/<event>`.
    pub name: String,
    /// "The Amazon Web Services account ID that is permitted to create a
    /// matching partner event bus for this partner event source"
    /// (`CreatePartnerEventSource$Account`).
    pub account: String,
    pub created: SystemTime,
    /// When this source entered `PENDING`, which is what its expiry is counted
    /// from ([`PARTNER_EVENT_SOURCE_EXPIRY`]); `None` once the matching bus is
    /// receiving events, because a matched source does not expire.
    pub pending_since: Option<SystemTime>,
    /// Whether the matching bus is receiving events from this source — set when
    /// the customer creates the matching bus or calls `ActivateEventSource`, and
    /// cleared by `DeactivateEventSource`.
    pub activated: bool,
    /// Whether the PARTNER has deleted the source while the customer's matching
    /// bus still exists, which is the `DELETED` state the customer reads.
    pub deleted: bool,
}

impl PartnerEventSourceRecord {
    /// A source named `name`, offered to `account` at `created`: what
    /// `CreatePartnerEventSource` stores. It starts `PENDING` — the customer has
    /// not matched it with a bus yet — so its expiry clock starts now.
    pub fn new(name: &str, account: &str, created: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            account: account.to_string(),
            created,
            pending_since: Some(created),
            activated: false,
            deleted: false,
        }
    }

    /// "The date and time that the event source will expire, if the Amazon Web
    /// Services account doesn't create a matching event bus for it" — the end of
    /// the [`PARTNER_EVENT_SOURCE_EXPIRY`] window this source entered `PENDING`
    /// at, and `None` for one that is not waiting to be matched.
    pub fn expiration(&self) -> Option<SystemTime> {
        self.pending_since
            .map(|since| since + PARTNER_EVENT_SOURCE_EXPIRY)
    }

    /// "The name of the SaaS partner that created the event source"
    /// (`DescribeEventSource$CreatedBy`): the `partner_name` segment of the
    /// source's own name, which is "determined during partner registration, and
    /// identifies the partner to Amazon Web Services customers"
    /// (`CreatePartnerEventSource`).
    pub fn created_by(&self) -> Option<&str> {
        partner_name(&self.name)
    }
}

/// The `partner_name` segment of a partner event source name or name prefix:
/// the first segment after [`PARTNER_EVENT_SOURCE_PREFIX`], or `None` for a
/// value that is not in the partner namespace at all.
pub fn partner_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(PARTNER_EVENT_SOURCE_PREFIX)?;
    let partner = rest.split('/').next().unwrap_or(rest);
    (!partner.is_empty()).then_some(partner)
}

/// The partner NAMESPACE a source name or name prefix lies in,
/// `aws.partner/<partner_name>`.
///
/// It is the unit an SaaS partner is entitled to author in: every name under one
/// `partner_name` belongs to that one partner, so the namespace — not the whole
/// source name — is what an authoring call is checked against
/// ([`ComAmazonawsEventbridgeAwseventsState::may_author_partner_namespace`]).
pub fn partner_namespace(name: &str) -> Option<String> {
    partner_name(name).map(|partner| format!("{PARTNER_EVENT_SOURCE_PREFIX}{partner}"))
}

/// What an authoring operation of the partner slice reports to a caller that is
/// not the SaaS partner of the namespace it named.
///
/// Authoring a partner event source is "called by an SaaS partner … this
/// operation is not used by Amazon Web Services customers"
/// (`CreatePartnerEventSource`), and the partner of a namespace is registered
/// with Amazon Web Services outside this API, so a caller holding no
/// registration for the namespace it named is refused rather than told whether
/// the source exists. The message is built from the request: it names that
/// namespace, which is the one thing the caller has to change.
pub fn not_entitled_to_namespace(namespace: &str) -> String {
    format!("Caller is not whitelisted for the event source partner namespace {namespace}")
}

/// What any reader reports for a name that names no partner event source: the
/// missing SOURCE by name, built from the request, in the same words every
/// reader of this resource kind uses (see [`no_such_rule`]).
pub fn no_such_event_source(name: &str) -> String {
    format!("Event source {name} does not exist.")
}

/// What the operations that act on a source THROUGH its matching bus
/// (`ActivateEventSource`, `DeactivateEventSource`) report when either of the
/// two is missing.
///
/// Both have to exist for the call to mean anything — activating a source is
/// what makes "your matching event bus start receiving events from the event
/// source" (`ActivateEventSource`) — and the refusal names both rather than
/// telling the caller which of them it was: the caller's next step, creating the
/// pair, is the same either way.
pub fn no_such_event_source_or_bus(name: &str) -> String {
    format!("Event source {name} or event bus {name} does not exist.")
}

/// `arn:aws:events:<region>:<account>:event-source/<name>`.
///
/// `DescribeEventSource$Arn` and `CreatePartnerEventSource$EventSourceArn` are
/// modelled as plain strings, so the ARN is built the way every other ARN of
/// this service is ([`event_bus_arn`]): this service's namespace, the region the
/// request was signed for, and the resource type that names the resource kind.
pub fn event_source_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("event-source/{name}"))
}

/// The `EndpointState` a created global endpoint is answered in.
///
/// "Creates a global endpoint" (`CreateEndpoint`) starts work that finishes on
/// its own — the endpoint's own URL has to be published before events can be
/// routed through it — so the create answers a non-terminal state and a later
/// read reports the endpoint serving.
pub const ENDPOINT_STATE_CREATING: &str = "CREATING";

/// The `EndpointState` of a global endpoint that is serving.
pub const ENDPOINT_STATE_ACTIVE: &str = "ACTIVE";

/// The `EndpointState` a global endpoint carries while a write to it is being
/// applied.
///
/// `EndpointState` holds `UPDATING` beside `ACTIVE`, and
/// `UpdateEndpoint$State` is "the state of the endpoint you updated in this
/// request": the configuration a write names has to reach the Regions the
/// endpoint spans before the endpoint is serving it, so the write answers this
/// non-terminal state and a later read reports the endpoint serving again.
pub const ENDPOINT_STATE_UPDATING: &str = "UPDATING";

/// How long after a write a global endpoint's [`ENDPOINT_STATE_ACTIVE`] state
/// becomes due.
///
/// AWS publishes no duration for it, so this is the emulator's OWN span, chosen
/// inside the window the project fixes for a clock-owed transition, and it is
/// stored as a DUE INSTANT rather than waited for: a caller reading straight
/// after the write still observes [`ENDPOINT_STATE_CREATING`] or
/// [`ENDPOINT_STATE_UPDATING`].
pub const ENDPOINT_TRANSITION: Duration = Duration::from_millis(400);

/// Largest number of global endpoints this store holds.
///
/// This emulator's OWN bound, like [`MAX_ARCHIVES`]: a create past it answers
/// the modelled `LimitExceededException` instead of growing memory without
/// limit. It is far above any realistic fixture's needs.
pub const MAX_ENDPOINTS: usize = 100;

/// `ReplicationState`'s value for an endpoint that replicates every event to
/// the secondary Region.
pub const REPLICATION_STATE_ENABLED: &str = "ENABLED";

/// `ReplicationState`'s value for an endpoint that replicates nothing.
pub const REPLICATION_STATE_DISABLED: &str = "DISABLED";

/// The `ReplicationState` an endpoint takes when the request names none: "The
/// default state is ENABLED which means you must supply a RoleArn"
/// (`CreateEndpoint$ReplicationConfig`). It is what a read reports, so the
/// endpoint reports the replication actually in force rather than nothing.
pub const DEFAULT_REPLICATION_STATE: &str = REPLICATION_STATE_ENABLED;

/// What a writer of a global endpoint reports for a configuration that would
/// leave event replication on with no role to replicate as.
///
/// "The default state is ENABLED which means you must supply a RoleArn. If you
/// don't have a RoleArn or you don't want event replication enabled, set the
/// state to DISABLED" (`CreateEndpoint$ReplicationConfig`): replication runs AS
/// that role, so a configuration that names none is refused. The sentence
/// EventBridge answers it with is entirely FIXED — it names neither the member
/// nor the state, so nothing of the request appears in it — and it is stated
/// once here, for both writers of an endpoint's replication configuration.
pub const REPLICATION_NEEDS_A_ROLE: &str = "When replication is enabled, role cannot be empty.";

/// The domain a global endpoint's URL is published under.
///
/// "The URL subdomain of the endpoint. For example, if the URL for Endpoint is
/// `https://abcde.veo.endpoints.event.amazonaws.com`, then the EndpointId is
/// `abcde.veo`" (`Endpoint$EndpointId`): the URL is the endpoint's own id
/// followed by this domain, which is why [`endpoint_url`] builds one from the
/// other instead of storing both independently.
pub const ENDPOINT_URL_DOMAIN: &str = "endpoints.event.amazonaws.com";

/// `arn:aws:events:<region>:<account>:endpoint/<name>`.
///
/// `EndpointArn`'s modelled pattern is
/// `^arn:aws([a-z]|\-)*:events:([a-z]|\d|\-)*:([0-9]{12})?:endpoint\/[/\.\-_A-Za-z0-9]+$`,
/// so the resource is the endpoint's own name under this service's `endpoint/`
/// type.
pub fn endpoint_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("endpoint/{name}"))
}

/// The `EndpointId` of a global endpoint, built from one id of this store's own
/// mint ([`ComAmazonawsEventbridgeAwseventsState::next_unique_id`]).
///
/// `EndpointId` is modelled `pattern ^[A-Za-z0-9\-]+[\.][A-Za-z0-9\-]+$` and
/// documented with the example `abcde.veo` — two dot-separated segments — while
/// the URL built from it ([`endpoint_url`]) is modelled
/// `^(https://)?[\.\-a-z0-9]+$`, so both segments are lowercase. The id is the
/// subdomain EventBridge publishes the endpoint at, so it is minted per
/// endpoint and never derived from the endpoint's name: two endpoints of one run
/// cannot share a subdomain.
///
/// The URL is how events reach the endpoint, so a subdomain two endpoints shared
/// would make both unroutable. The id is therefore built from BOTH ends of the
/// minted value: the first segment from its leading field, which the creation
/// instant supplies, so ids of separate runs differ; the second from its
/// trailing field, which is the mint's own per-store counter, so no two
/// endpoints of one run can be handed the same subdomain however close together
/// they were created. Each field is kept well inside `EndpointId`'s modelled
/// `length 1..50`.
pub fn endpoint_id(unique: &str) -> String {
    /// How many characters of the instant-derived field the subdomain carries.
    const SUBDOMAIN_WIDTH: usize = 6;
    /// Smallest number of characters the counter-derived segment carries, so a
    /// low counter still reads as a subdomain rather than a single digit.
    const COUNTER_WIDTH: usize = 3;

    let field = |text: &str| -> String {
        text.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_lowercase())
            .collect()
    };
    let mut fields = unique.split('-');
    let instant = field(fields.next().unwrap_or_default());
    // The counter is the LAST field the mint formats, and the one that differs
    // between two ids of a single run.
    let counter = field(fields.next_back().unwrap_or_default());

    // A field shorter than its segment is padded, so the value satisfies the
    // modelled pattern whatever the mint handed back; the counter keeps every
    // character that distinguishes it, with only its leading padding dropped.
    let subdomain: String = format!("{instant:0<SUBDOMAIN_WIDTH$}")
        .chars()
        .take(SUBDOMAIN_WIDTH)
        .collect();
    let trimmed = counter.trim_start_matches('0');
    let suffix = format!("{trimmed:0>COUNTER_WIDTH$}");
    format!("{subdomain}.{suffix}")
}

/// The URL a global endpoint is reached at: its own [`endpoint_id`] under
/// [`ENDPOINT_URL_DOMAIN`].
pub fn endpoint_url(endpoint_id: &str) -> String {
    format!("https://{endpoint_id}.{ENDPOINT_URL_DOMAIN}")
}

/// What any operation addressing a name that names no global endpoint reports:
/// the missing ENDPOINT by name AND the Region it was looked for in, both built
/// from the request ([`endpoint_region`] resolves the Region).
///
/// Like `no_such_archive`, [`no_such_replay`], [`no_such_event_bus`],
/// [`no_such_event_source`] and [`no_such_rule`], it names the resource kind and
/// then the identifier the caller passed; unlike them it also names the Region,
/// because a global endpoint is homed in ONE Region and the same name can be
/// absent here and present there — so the refusal says WHERE it looked.
pub fn no_such_endpoint(name: &str, region: &str) -> String {
    format!("Endpoint {name} does not exist in region {region}.")
}

/// The Region a global-endpoint request is about: the `HomeRegion` it named, or
/// the Region the request itself was addressed to when it named none.
///
/// A global endpoint is homed in ONE Region — "the primary Region of the
/// endpoint you want to get information about" (`DescribeEndpoint$HomeRegion`) —
/// so the Region searched is the one the caller named, and otherwise the one the
/// call was signed for ([`request_region`]). It is the Region
/// [`no_such_endpoint`] reports, so the refusal names the Region actually
/// searched. An operation whose input carries no `HomeRegion` member passes
/// `None`.
pub fn endpoint_region(home_region: Option<&str>, parts: &::http::request::Parts) -> String {
    home_region.map_or_else(|| request_region(parts), str::to_string)
}

/// The Region field of an ARN, or `None` when the text carries none.
///
/// The ARN fields are `arn:<partition>:<service>:<region>:<account>:<resource>`
/// ([`is_arn`]), and the Region is the one an endpoint's event bus is reached in
/// ([`endpoint_event_bus_violation`]).
pub fn arn_region(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next() != Some("arn") {
        return None;
    }
    let (_partition, _service) = (fields.next()?, fields.next()?);
    let region = fields.next()?;
    (!region.is_empty()).then_some(region)
}

/// Why an endpoint request's `RoutingConfig` is not a failover policy
/// EventBridge accepts for an endpoint whose primary Region is
/// `primary_region`, or `None` when it is one.
///
/// "The secondary Region that processes events when failover is triggered or
/// replication is enabled" (`Secondary`) — the secondary Region is the OTHER
/// Region of the pair, so a `Route` naming the endpoint's own primary Region
/// describes a failover that fails over to where it already is, and is refused
/// rather than stored. The route's own form is modelled (`Route` is len 9..20
/// with pattern `^[\-a-z0-9]+$`), so what is left to decide here is only which
/// Region it names relative to the endpoint's primary one.
///
/// This is the first thing an endpoint write is judged on, before the bus pair
/// ([`endpoint_event_bus_violation`]): the two members describe the SAME pair of
/// Regions, so where both are wrong the caller reads the refusal about the
/// Region it stated itself.
///
/// The message is built from the request and names the member that was rejected,
/// the way this service's other parameter refusals do ([`parameter_violation`]).
pub fn routing_config_violation(routing: &RoutingConfig, primary_region: &str) -> Option<String> {
    (routing.failover_config.secondary.route == primary_region).then(|| {
        parameter_violation(
            "Route",
            "Provided route for the secondary region cannot be the same as the primary region.",
        )
    })
}

/// Why an endpoint request's `EventBuses` name no pair of buses an endpoint can
/// route between, or `None` when they do.
///
/// "You define a primary and secondary Region with event buses in each Region"
/// and "the names of the event buses must be identical in each Region"
/// (`CreateEndpoint`): the pair is ONE bus name present in TWO Regions, so a
/// pair naming two differently named buses, or the same Region twice, is refused
/// rather than stored as a routing pair no event could ever follow. The list's
/// length and each member's ARN form are modelled (`EndpointEventBusList` is
/// len 2..2 and `EndpointEventBus$EventBusArn` carries a pattern), so a pair
/// reaching here holds exactly two well-formed bus ARNs.
///
/// Whether each bus EXISTS is not decided here, and cannot be: one of the two is
/// in another Region by the rule above, and this store holds the buses of the
/// Region its requests are signed for.
///
/// The message is built from the request: it names the member that was rejected
/// and the requirement it failed, the way this service's other parameter
/// refusals do ([`parameter_violation`]).
pub fn endpoint_event_bus_violation(buses: &[EndpointEventBus]) -> Option<String> {
    let [first, second] = buses else {
        return None;
    };
    let bus_name = |bus: &EndpointEventBus| -> Option<String> {
        event_bus_name_in_arn(&bus.event_bus_arn).map(str::to_string)
    };
    let bus_region =
        |bus: &EndpointEventBus| -> Option<String> { arn_region(&bus.event_bus_arn).map(str::to_string) };
    if bus_name(first) != bus_name(second) {
        return Some(parameter_violation(
            "EventBuses",
            "The names of the event buses must be identical in each Region.",
        ));
    }
    if bus_region(first) == bus_region(second) {
        return Some(parameter_violation(
            "EventBuses",
            "The event buses must be in two different Regions.",
        ));
    }
    None
}

/// A global endpoint as this emulator persists it.
///
/// "A global endpoint used to improve your application's availability by making
/// it regional-fault tolerant" ([`crate::ops::types::Endpoint`]): the record
/// holds the pair of event buses events are routed between, the routing policy
/// that decides which of the two is used, whether events are replicated to the
/// secondary Region and the role that replication runs as, plus the identity
/// EventBridge publishes the endpoint at — its ARN, its subdomain and the URL
/// built from that subdomain.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    pub name: String,
    /// Minted ONCE, when the endpoint is created, and never recomputed.
    pub arn: String,
    pub description: Option<String>,
    /// The Region the endpoint was created in, which is its PRIMARY Region:
    /// "The primary Region of the endpoint you want to get information about"
    /// (`DescribeEndpoint$HomeRegion`), so a reader naming another Region's
    /// endpoint reads nothing
    /// ([`ComAmazonawsEventbridgeAwseventsState::endpoint_in_home_region`]).
    pub home_region: String,
    /// "Configure the routing policy, including the health check and secondary
    /// Region" (`CreateEndpoint$RoutingConfig`), reported back as given.
    pub routing_config: RoutingConfig,
    /// One of `ReplicationState`'s values, defaulted to
    /// [`DEFAULT_REPLICATION_STATE`] when the request named none.
    pub replication_state: String,
    pub event_buses: Vec<EndpointEventBus>,
    /// "The ARN of the role used for replication" (`CreateEndpoint$RoleArn`);
    /// `None` only for an endpoint whose replication is disabled.
    pub role_arn: Option<String>,
    /// The subdomain EventBridge publishes this endpoint at ([`endpoint_id`]),
    /// minted once with the endpoint.
    pub endpoint_id: String,
    /// One of `EndpointState`'s values.
    pub state: String,
    pub state_reason: Option<String>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    /// When [`ENDPOINT_STATE_ACTIVE`] falls due, or `None` once the endpoint has
    /// reached the state it stays in until a caller changes it.
    pub active_due: Option<SystemTime>,
}

impl EndpointRecord {
    /// "Whether replication is enabled or disabled for the endpoint"
    /// (`DescribeEndpoint$ReplicationConfig`), built from the stored state so a
    /// read reports the replication actually in force — including the default
    /// the request left out.
    pub fn replication_config(&self) -> ReplicationConfig {
        ReplicationConfig {
            state: Some(self.replication_state.clone()),
        }
    }

    /// "The URL of the endpoint" (`DescribeEndpoint$EndpointUrl`), built from
    /// the stored subdomain so the URL and the `EndpointId` cannot disagree.
    pub fn url(&self) -> String {
        endpoint_url(&self.endpoint_id)
    }

    /// Every member a listing reports for this endpoint
    /// ([`crate::ops::types::Endpoint`]), read back from the same stored record
    /// the single-endpoint read reports, so a listing and a read cannot
    /// disagree.
    pub fn to_summary(&self) -> EndpointSummary {
        EndpointSummary {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            routing_config: Some(self.routing_config.clone()),
            replication_config: Some(self.replication_config()),
            event_buses: Some(self.event_buses.clone()),
            role_arn: self.role_arn.clone(),
            endpoint_id: Some(self.endpoint_id.clone()),
            endpoint_url: Some(self.url()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            creation_time: Some(to_wire_timestamp(self.created)),
            last_modified_time: Some(to_wire_timestamp(self.last_modified)),
        }
    }

    /// Every member `UpdateEndpoint` reports, read back from the stored record,
    /// so the write's answer and a later read cannot disagree.
    pub fn to_update_output(&self) -> UpdateEndpointOutput {
        UpdateEndpointOutput {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            routing_config: Some(self.routing_config.clone()),
            replication_config: Some(self.replication_config()),
            event_buses: Some(self.event_buses.clone()),
            role_arn: self.role_arn.clone(),
            endpoint_id: Some(self.endpoint_id.clone()),
            endpoint_url: Some(self.url()),
            state: Some(self.state.clone()),
        }
    }

    /// Every member `DescribeEndpoint` reports, read back from the stored
    /// record, so a read and what the create did cannot disagree.
    pub fn to_describe_output(&self) -> DescribeEndpointOutput {
        DescribeEndpointOutput {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            routing_config: Some(self.routing_config.clone()),
            replication_config: Some(self.replication_config()),
            event_buses: Some(self.event_buses.clone()),
            role_arn: self.role_arn.clone(),
            endpoint_id: Some(self.endpoint_id.clone()),
            endpoint_url: Some(self.url()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            creation_time: Some(to_wire_timestamp(self.created)),
            last_modified_time: Some(to_wire_timestamp(self.last_modified)),
        }
    }
}

/// Per-service state for com-amazonaws-eventbridge-awsevents. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEventbridgeAwseventsState {
    /// Every event bus, by Region and name ([`BusKey`]): a bus name is unique
    /// within one Region, and the same name in another Region is another bus.
    /// A Region's `default` bus is recorded here the first time something
    /// WRITES a bus of that Region; until then it is the implicit one below, so
    /// no read has to write.
    pub buses: BTreeMap<BusKey, EventBus>,
    /// The `default` bus of a Region nothing has written a bus in yet: the bus
    /// the account has in every Region before any call is made, holding no
    /// creation instant, no configuration and no rule. One canonical record,
    /// never handed out mutably — see
    /// [`ComAmazonawsEventbridgeAwseventsState::implicit_default_bus_in`].
    implicit_default_bus: EventBus,
    /// Every connection, by name — the resource `CreateConnection` writes and
    /// `DescribeConnection` reports.
    pub connections: BTreeMap<String, ConnectionRecord>,
    /// Every archive, by name — the resource `CreateArchive` writes,
    /// `DescribeArchive` reports and `PutEvents` captures matching events into.
    pub archives: BTreeMap<String, ArchiveRecord>,
    /// Every replay, by name — the job `StartReplay` writes and
    /// `DescribeReplay` reports. Ordered by name, so enumeration is
    /// deterministic.
    pub replays: BTreeMap<String, ReplayRecord>,
    /// Every API destination, by name — the resource `CreateApiDestination`
    /// writes and `DescribeApiDestination` reports.
    pub api_destinations: BTreeMap<String, ApiDestinationRecord>,
    /// Every global endpoint, by name — the resource `CreateEndpoint` writes
    /// and `DescribeEndpoint` reports. Ordered by name, so enumeration is
    /// deterministic.
    pub endpoints: BTreeMap<String, EndpointRecord>,
    /// Every partner event source, by name — the resource
    /// `CreatePartnerEventSource` writes, `DescribePartnerEventSource` and
    /// `DescribeEventSource` report, and `PutPartnerEvents` publishes to. Ordered
    /// by name, so enumeration is deterministic and the list operations can
    /// resume from a page token.
    pub partner_sources: BTreeMap<String, PartnerEventSourceRecord>,
    /// The partner namespaces ([`partner_namespace`]) this account is the
    /// registered SaaS partner of, and may therefore author partner event
    /// sources in.
    ///
    /// Empty by default, and no operation of this API adds to it: a partner's
    /// `partner_name` "is determined during partner registration"
    /// (`CreatePartnerEventSource`), which happens outside EventBridge, so an
    /// ordinary account holds no registration and every authoring call it makes
    /// is refused ([`not_entitled_to_namespace`]). A deployment that stands in
    /// for an onboarded SaaS partner installs the registration it was given with
    /// [`Self::register_partner_namespace`], and the whole partner slice then
    /// works off it.
    pub partner_namespaces: BTreeSet<String>,
    /// The routing log: what `PutEvents` accepted and where it went, newest
    /// last. A ring bounded by [`MAX_DELIVERED_EVENTS`] — see
    /// [`Self::record_delivery`], the one place it grows.
    pub delivered: VecDeque<DeliveredEvent>,
    /// The invocation log: what came of handing each routed event to each
    /// target of each matched rule, newest last. A ring bounded by
    /// [`MAX_TARGET_INVOCATIONS`] — see [`Self::record_invocation`], the one
    /// place it grows.
    pub invocations: VecDeque<TargetInvocation>,
    /// How many bytes the records in `delivered` hold, kept in step with the
    /// ring so the byte bound costs nothing to check.
    delivered_bytes: usize,
    /// Monotonic counter behind [`Self::next_event_id`]; never reused in a run.
    event_seq: u64,
    /// What the schedule clock has already done for the rules of this store that
    /// fire on a `ScheduleExpression`, and which clock is driving them
    /// ([`schedule`]).
    pub schedules: schedule::Schedules,
}

/// The store comes up holding no bus RECORD at all: the account's `default` bus
/// exists in every Region before any call is made, and which Regions those are
/// is not known until a request names one, so a Region's default bus is the
/// implicit one until something writes a bus of that Region — the whole rule
/// slice therefore works without `CreateEventBus`, in every Region, and no read
/// writes anything to make that so.
///
/// No call creates it, so it holds no creation instant ([`EventBus::created`]):
/// the moment this store was built, or the moment a Region was first addressed,
/// is not a creation anybody performed and is never reported as one.
impl Default for ComAmazonawsEventbridgeAwseventsState {
    fn default() -> Self {
        Self {
            buses: BTreeMap::new(),
            implicit_default_bus: EventBus::new(DEFAULT_EVENT_BUS_NAME, None),
            connections: BTreeMap::new(),
            archives: BTreeMap::new(),
            replays: BTreeMap::new(),
            api_destinations: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            partner_sources: BTreeMap::new(),
            partner_namespaces: BTreeSet::new(),
            delivered: VecDeque::new(),
            invocations: VecDeque::new(),
            delivered_bytes: 0,
            event_seq: 0,
            schedules: schedule::Schedules::default(),
        }
    }
}

/// The store's own accessors and the clock-owed transitions a read may finish
/// live beside the records, in their own file.
mod store;


/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEventbridgeAwseventsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEventbridgeAwseventsState>)
}

/// The region this request was signed for, which is the region its ARNs carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| FALLBACK_REGION.to_string(), |scope| scope.region)
}

/// This emulator's account id — the account that owns every resource it mints,
/// and so the `CreatedBy` a rule reports.
pub fn account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`.
pub fn event_bus_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("event-bus/{name}"))
}

/// `arn:aws:events:<region>:<account>:rule/<bus-if-not-default>/<name>`.
///
/// A rule on the `default` bus carries no bus segment, which is the form
/// EventBridge mints; a rule on any other bus names that bus, so two rules of
/// the same name on different buses have different ARNs.
pub fn rule_arn(region: &str, event_bus_name: &str, rule_name: &str) -> String {
    let resource = if event_bus_name == DEFAULT_EVENT_BUS_NAME {
        format!("rule/{rule_name}")
    } else {
        format!("rule/{event_bus_name}/{rule_name}")
    };
    ::engine::emulator_arn(ARN_NAMESPACE, region, &resource)
}

/// The ARN namespace of AWS Secrets Manager, where a connection's credentials
/// are held ([`connection_secret_arn`]).
pub const SECRETS_MANAGER_ARN_NAMESPACE: &str = "secretsmanager";

/// `arn:aws:events:<region>:<account>:connection/<name>/<id>`.
///
/// `ConnectionArn`'s modelled pattern is
/// `^arn:aws([a-z]|\-)*:events:([a-z]|\d|\-)*:([0-9]{12})?:connection\/[\.\-_A-Za-z0-9]+\/[\-A-Za-z0-9]+$`,
/// so the resource carries the connection's NAME and then a unique id segment of
/// hyphens and alphanumerics ([`ComAmazonawsEventbridgeAwseventsState::next_unique_id`]
/// mints one).
pub fn connection_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("connection/{name}/{id}"))
}

/// The ARN of the secret a connection's credentials are held in.
///
/// `DescribeConnection$SecretArn` is "the ARN of the secret created from the
/// authorization parameters specified for the connection", and its modelled
/// pattern is
/// `^arn:aws([a-z]|\-)*:secretsmanager:([a-z]|\d|\-)*:([0-9]{12})?:secret:[\/_+=\.@\-A-Za-z0-9]+$`
/// — so the secret is named after the connection it belongs to, under this
/// service's own `events/connection/` prefix. The secret's VALUE is held on the
/// connection record's behalf by this service and is never reported: no operation
/// of this service hands a connection's credentials back.
pub fn connection_secret_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SECRETS_MANAGER_ARN_NAMESPACE,
        region,
        &format!("secret:events/connection/{name}/{id}"),
    )
}

/// The IAM store a target's `RoleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so EventBridge reads the very records
/// `CreateRole`, `PutRolePolicy` and `AttachRolePolicy` wrote rather than a
/// second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles
/// live. `None` means the engine carries no such store — a deployment with no
/// IAM to ask, which is evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The CloudWatch store EventBridge's own metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME
/// shared engine this service's store lives in, so what EventBridge counts is
/// what `ListMetrics` and `GetMetricStatistics` report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine. `None` means the engine
/// carries no such store — a deployment with no CloudWatch to count on.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The identity of ONE CloudWatch series, as CloudWatch's own store keys it: the
/// namespace, the metric name and the dimension set together.
pub type MetricSeriesKey =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::MetricKey;

/// The Glue store a rule's WORKFLOW target is delivered to.
///
/// It is the Glue service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so a delivery is counted against the
/// very EVENT trigger `CreateTrigger` wrote and the run it starts is the one
/// `GetWorkflowRuns` reports. Glue publishes no API for the ingress EventBridge
/// uses — the action a target's role carries is `glue:notifyEvent`, which no
/// operation of the Glue model serves — so the delivery reaches that store
/// directly, the way this service's own metrics reach CloudWatch's
/// ([`count_rule_metric`]).
pub type GlueState = ::service_com_amazonaws_glue_awsglue::ComAmazonawsGlueAwsglueState;

/// The Glue store in the SAME shared engine, which is where a workflow and its
/// triggers live. `None` means the engine carries no such store — a deployment
/// with no Glue to deliver to, which is evidence of nothing.
pub fn glue_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<GlueState>> {
    ::service_com_amazonaws_glue_awsglue::state::service_state(engine)
}

/// Count ONE undeliverable event on the rule's `FailedInvocations` metric.
///
/// "FailedInvocations: the number of invocations that failed permanently"
/// (EventBridge's published metrics).
pub fn count_failed_invocation(cloudwatch: &mut CloudWatchState, bus: &str, rule: &str) {
    count_handover(cloudwatch, FAILED_INVOCATIONS_METRIC, bus, rule);
}

/// Count ONE invocation a rule's target accepted, on both counters an accepted
/// handover is reported on: the rule's `Invocations` and its
/// `SuccessfulInvocationAttempts`.
///
/// "Invocations: the number of times a target is invoked for a rule in response
/// to an event" and "SuccessfulInvocationAttempts: the number of invocation
/// attempts that were successful" (EventBridge's published metrics) are both
/// facts about the SAME accepted handover — the target's API call was made and
/// the target took the event — so one acceptance counts once on each, and
/// neither is counted for a handover that was refused ([`count_failed_invocation`])
/// or never attempted. Counting both is what lets a client read delivery success
/// off either series rather than inferring it from the absence of a failure.
pub fn count_invocation(cloudwatch: &mut CloudWatchState, bus: &str, rule: &str) {
    count_handover(cloudwatch, INVOCATIONS_METRIC, bus, rule);
    count_handover(cloudwatch, SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, bus, rule);
}

/// Count what came of ONE handover on `metric` for the rule `rule` of `bus`.
///
/// A handover is counted on its own, under a guard its caller holds for that one
/// write, so the series it wrote is not carried out for a batched alarm pass the
/// way a round of matches is ([`count_rule_matches`]): an alarm over these two
/// counters keeps the evaluation instant it already carries, which is what these
/// counters have done since they were introduced and is left unchanged here.
fn count_handover(cloudwatch: &mut CloudWatchState, metric: &str, bus: &str, rule: &str) {
    let mut written = ::std::collections::BTreeSet::new();
    count_rule_metric(cloudwatch, metric, bus, rule, &mut written);
}

/// Count ONE event a rule's `EventPattern` matched on that rule's own
/// `MatchedEvents` and `TriggeredRules` metrics.
///
/// The MATCH is what these two count, not what became of it: "MatchedEvents: the
/// number of events that matched with any rule" and "TriggeredRules: the number
/// of rules that have run" (EventBridge's published metrics) are both facts about
/// routing, so a rule carrying no target at all, and one whose every delivery was
/// refused, each count the events their pattern selected — which is what tells a
/// rule that stopped matching apart from a target that stopped accepting
/// ([`count_invocation`] / [`count_failed_invocation`]).
pub fn count_rule_match(
    cloudwatch: &mut CloudWatchState,
    bus: &str,
    rule: &str,
    written: &mut ::std::collections::BTreeSet<MetricSeriesKey>,
) {
    count_rule_metric(cloudwatch, MATCHED_EVENTS_METRIC, bus, rule, written);
    count_rule_metric(cloudwatch, TRIGGERED_RULES_METRIC, bus, rule, written);
}

/// Count every match the routing log `deliveries` records, on the metrics of the
/// rule each one matched ([`count_rule_match`]).
///
/// The one place routing becomes telemetry, so every path that publishes an event
/// onto a bus — a client's publication, a partner's, a forward onto another bus,
/// an archive replayed back onto one — counts its matches the same way and under
/// the bus the matched rule belongs to. `None` is a deployment with no CloudWatch
/// mounted, which is nothing to count on.
///
/// What one call does is bounded and interruptible, because the work is the
/// (events x matched rules) PRODUCT and both factors come from the request: at
/// most [`MAX_COUNTED_MATCHES_PER_REQUEST`] matches are counted, the CloudWatch
/// guard is taken and released per EVENT rather than held across the whole
/// product, and the loop yields between events so a cancellation or a shutdown
/// signal has somewhere to land — as the replay loop that calls this does for the
/// same reason. The alarms watching what was written are made due in ONE pass
/// over the DISTINCT series at the end, rather than once per datapoint: a series
/// is the same series however many events landed in it.
pub async fn count_rule_matches(
    cloudwatch: Option<&::engine::ServiceState<CloudWatchState>>,
    deliveries: &[DeliveredEvent],
) {
    let Some(cloudwatch) = cloudwatch else {
        return;
    };
    let mut written: ::std::collections::BTreeSet<MetricSeriesKey> =
        ::std::collections::BTreeSet::new();
    let mut budget = MAX_COUNTED_MATCHES_PER_REQUEST;
    for delivery in deliveries {
        if delivery.matched_rules.is_empty() {
            continue;
        }
        // One yield per event, so counting the matches of a whole batch stays
        // interruptible however many events and rules the request matched.
        ::tokio::task::yield_now().await;
        let counted = delivery.matched_rules.len().min(budget);
        // Said where the matches are DROPPED rather than on a later turn of this
        // loop, which an exhausted budget may never reach: a count nobody can see
        // is a count a client would read as a rule that did not match.
        if counted < delivery.matched_rules.len() {
            ::tracing::warn!(
                bus = %delivery.bus,
                event_id = %delivery.event_id,
                bound = MAX_COUNTED_MATCHES_PER_REQUEST,
                dropped = delivery.matched_rules.len() - counted,
                "this request reached the emulator's bound on the rule matches one \
                 request counts; the matches past it are not counted"
            );
        }
        if counted == 0 {
            break;
        }
        budget -= counted;
        let mut store = cloudwatch.lock();
        for rule in delivery.matched_rules.iter().take(counted) {
            count_rule_match(&mut store, &delivery.bus, rule, &mut written);
        }
        drop(store);
    }
    if written.is_empty() {
        return;
    }
    // An alarm watching one of these series is owed its evaluation from this
    // write on, exactly as it is for a datapoint a client published: an alarm
    // cannot tell a service's own metric from one `PutMetricData` ingested, so a
    // rule that stopped matching is what takes such an alarm out of
    // `INSUFFICIENT_DATA`.
    let watched: Vec<_> = written
        .into_iter()
        .map(
            |key| ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::WrittenSeries {
                key,
                units: vec![METRIC_UNIT_COUNT.to_string()],
            },
        )
        .collect();
    let at_secs =
        ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::now_secs();
    cloudwatch.lock().make_alarms_due(&watched, at_secs);
}

/// Append one observation of the value 1 to `metric` for the rule `rule` of the
/// bus `bus`.
///
/// The series live in the `AWS/Events` namespace, and the observation is appended
/// to EACH of the dimension sets THAT counter is published under
/// ([`rule_dimension_sets`]), because "CloudWatch treats each unique combination
/// of dimensions as a separate metric" (`GetMetricStatistics$Dimensions`): a
/// dimension set nothing is appended to is a series a reader finds no datapoint
/// for at all, which is what EventBridge publishing one counter under one
/// dimension set and another under two LOOKS like to a client.
///
/// The datapoint is appended to the CloudWatch store directly rather than sent
/// through `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the
/// services that own them ("you should not specify a namespace that begins with
/// `AWS/`", `PutMetricData$Namespace`), so a service's own metric never travels
/// over that API. It goes into the very store `ListMetrics` and
/// `GetMetricStatistics` read, through CloudWatch's own bounded append, so one
/// metric and its reader cannot disagree.
///
/// Every series this appended to is collected into `written` — the identity only,
/// so one series the whole request wrote is one entry — which is what lets a
/// caller make the alarms watching them due in one pass instead of per datapoint.
fn count_rule_metric(
    cloudwatch: &mut CloudWatchState,
    metric: &str,
    bus: &str,
    rule: &str,
    written: &mut ::std::collections::BTreeSet<MetricSeriesKey>,
) {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as metrics;

    for dimensions in rule_dimension_sets(metric, bus, rule) {
        let key = metrics::MetricKey {
            namespace: METRIC_NAMESPACE.to_string(),
            metric_name: metric.to_string(),
            dimensions,
        };
        // One occurrence: a single observation of the value 1, at standard
        // (60-second) storage resolution, counted in `Count` — the unit the
        // metric is published in.
        let datum = metrics::StoredDatum {
            timestamp_secs: metrics::now_secs(),
            sample_count: 1.0,
            sum: 1.0,
            minimum: 1.0,
            maximum: 1.0,
            unit: Some(METRIC_UNIT_COUNT.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if cloudwatch.append_datapoints(key.clone(), vec![datum]).is_err() {
            ::tracing::warn!(
                %bus,
                %rule,
                "the CloudWatch store is at capacity; an EventBridge occurrence \
                 could not be counted on {metric}"
            );
            continue;
        }
        written.insert(key);
    }
}

/// The counters EventBridge reports for a rule ONLY under the rule's whole
/// identity — the event bus the rule belongs to together with the rule's name.
///
/// These two count what the bus's ROUTING TABLE did with an event, so the bus is
/// part of what the count is about and the rule name on its own is not a series
/// EventBridge publishes them under: a reader naming the rule name alone finds no
/// datapoint for them, not a zero.
const ROUTING_COUNTERS: [&str; 2] = [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC];

/// The dimension sets ONE per-rule series of `metric` is published under, which
/// is a property of the COUNTER rather than of the rule.
///
/// The rule's whole identity — the bus it belongs to and its name — is a series
/// every per-rule counter is published under, and it is what tells two buses'
/// rules of the same name apart. The rule name ON ITS OWN is a second series, and
/// EventBridge publishes the counters of a HANDOVER (what became of a delivery)
/// under it as well, while the counters of the MATCH ([`ROUTING_COUNTERS`]) are
/// published under the whole identity only. They are separate metrics, so a
/// counter published under both is appended to both rather than one standing in
/// for the other.
fn rule_dimension_sets(
    metric: &str,
    bus: &str,
    rule: &str,
) -> Vec<::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::DimensionSet> {
    let whole_identity = [
        (EVENT_BUS_NAME_DIMENSION.to_string(), bus.to_string()),
        (RULE_NAME_DIMENSION.to_string(), rule.to_string()),
    ]
    .into_iter()
    .collect();
    if ROUTING_COUNTERS.contains(&metric) {
        return vec![whole_identity];
    }
    let by_rule_name = [(RULE_NAME_DIMENSION.to_string(), rule.to_string())]
        .into_iter()
        .collect();
    vec![whole_identity, by_rule_name]
}

/// The most policy documents the delivery gate reads for ONE role.
///
/// A role carries its inline policies plus at most the IAM store's own ceiling of
/// attached managed policies. A role whose documents outnumber this is one the
/// gate does not finish reading, and an unread document might be the one that
/// carries the grant — so nothing is shown about such a role and the delivery
/// goes ahead.
pub const MAX_ROLE_DOCUMENTS: usize = 32;

/// Why EventBridge cannot deliver a matched event to a target under the role the
/// target names, or `None` when nothing of the kind can be shown.
///
/// A rule's target is invoked by EventBridge ACTING AS the target's `RoleArn`, so
/// two things have to hold before the invocation is made: the role must let
/// [`SERVICE_PRINCIPAL`] assume it, and the policies in force on it must not
/// withhold the API call the target's invocation makes. Both are decided against
/// the account's OWN IAM records — the ones `CreateRole`/`PutRolePolicy` wrote —
/// and through the shared readers every service that resolves a caller-supplied
/// role uses, so no two services answer differently.
///
/// The condition this answers names the STEP that failed, which is what a
/// dead-letter consumer triages on: a role that cannot be assumed at all is
/// [`FAILED_TO_ASSUME_ROLE`], and a role that can be assumed but whose policies
/// withhold the target's API call is [`NO_PERMISSIONS`].
///
/// The emulator refuses only what it can SHOW, so each of these is evidence of
/// nothing and lets the delivery go ahead: a target naming no role at all (SQS,
/// SNS and Lambda targets are reached through the resource's own policy instead),
/// an engine with no IAM store, a `RoleArn` the store holds no role for (it may
/// belong to another account), a trust document that cannot be read as a policy,
/// and a role carrying more documents than [`MAX_ROLE_DOCUMENTS`].
pub fn delivery_refusal(
    role_arn: Option<&str>,
    rule_arn: &str,
    attempt: &DeliveryAttempt<'_>,
    iam: &IamState,
) -> Option<(String, String)> {
    let role_arn = role_arn?;
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    // The trust policy first: a role EventBridge cannot assume never gets as far
    // as its permissions. The step is stated on its own in
    // [`role_assumption::assume_role_refusal`], because it is about the role
    // alone — which API call would have followed does not enter into it — so a
    // handover whose target call this emulator does not make is decided by that
    // step and nothing else. It is decided for the RULE that is invoking, whose
    // ARN is what a trust policy's own condition is scoped by.
    if let Some(refusal) = role_assumption::assume_role_refusal(Some(role_arn), rule_arn, iam) {
        return Some(refusal);
    }
    let documents = role_documents(role_arn, iam)?;
    let (action, resource) = (attempt.action, attempt.resource);
    let request = ::service_common::iam_authorization::Request::new(action, resource, &[]);
    let decision = ::service_common::iam_authorization::identity_decision(&documents, &request);
    if decision == ::service_common::iam_authorization::Decision::Allowed {
        return None;
    }
    // Nothing stated is an implicit deny in IAM, and the documents in force on
    // the role ARE the authority on what it may do: a role whose policies say
    // nothing about the call is not allowed to make it. Which of the two it was
    // is what the sentence's closing clause states, because they ask for
    // different remedies.
    let reason = if decision == ::service_common::iam_authorization::Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    Some((
        NO_PERMISSIONS.to_string(),
        not_authorized(
            &assumed_role_principal(role_arn, &role.role_name, attempt),
            action,
            resource,
            &reason,
        ),
    ))
}

/// Why the event bus whose resource policy is `policy` does not accept a
/// publication made as `role_arn`, or `None` when that policy shows nothing
/// against it.
///
/// A publication onto an event bus is authorized against THAT bus's resource
/// policy — "the policy that enables the external account to send events to
/// your account" (`DescribeEventBus$Policy`) — under the identity the
/// publication is made as, so a statement of it that explicitly denies that
/// identity the publication refuses it, whatever the identity's own policies
/// allow: an explicit deny wins over every allow, which is the ONE evaluation
/// in [`::service_common::iam_authorization`]. The condition is
/// [`NO_PERMISSIONS`], the same one a refusal for want of a permission carries
/// anywhere else ([`delivery_refusal`]).
///
/// Nothing ELSE is refused here, for the reason [`delivery_refusal`] gives: the
/// emulator refuses only what it can show. Within one account "the
/// resource-based policy and the identity-based policy are evaluated together,
/// and access is granted if either allows it", so a policy that states nothing
/// about the publication withholds nothing and the identity half of the same
/// decision stays [`delivery_refusal`]'s.
///
/// `policy` is the bus's policy document ([`BusPolicy::document`]), and `None`
/// — a bus nothing has installed a statement on — shows nothing either.
/// `context` is the publication's own condition context
/// ([`publication_condition_context`]), so a statement scoped to one event's
/// `Source` decides that event and no other.
///
/// The decision is the SAME-ACCOUNT one, which is what this emulator's single
/// account can hold: a principal of ANOTHER account, for which AWS requires an
/// explicit resource-policy allow rather than merely no deny, is out of scope
/// here and is left to the change that gives this emulator a second account.
pub fn publication_refusal(
    policy: Option<&str>,
    role_arn: Option<&str>,
    attempt: &DeliveryAttempt<'_>,
    context: &[(String, String)],
) -> Option<(String, String)> {
    let document = policy?;
    let principals = publication_principals(role_arn, attempt);
    let (action, resource) = (attempt.action, attempt.resource);
    let request = ::service_common::iam_authorization::Request::new(action, resource, &principals)
        .with_context(context)
        .with_published_keys(&PUBLICATION_CONDITION_KEYS);
    if ::service_common::iam_authorization::resource_decision(document, &request)
        != ::service_common::iam_authorization::Decision::Denied
    {
        return None;
    }
    Some((
        NO_PERMISSIONS.to_string(),
        explicit_resource_policy_deny(
            &publication_principal(role_arn, attempt),
            action,
            resource,
        ),
    ))
}

/// The condition key naming the resource a cross-service call was made ON BEHALF
/// OF — the rule whose target is being invoked — which is what a target's own
/// resource policy scopes its grant to.
pub const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition key naming the ACCOUNT that resource belongs to.
pub const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// Every condition key ONE target invocation carries a value for.
///
/// They are DECLARED on the request
/// ([`::service_common::iam_authorization::Request::with_published_keys`]),
/// because this invocation knows each of them exactly: a statement asking
/// whether one is PRESENT — rather than what it equals — is therefore decided
/// rather than left undecided.
const TARGET_INVOCATION_CONDITION_KEYS: [&str; 2] =
    [SOURCE_ARN_CONDITION_KEY, SOURCE_ACCOUNT_CONDITION_KEY];

/// The condition context ONE invocation of a rule's target carries: the ARN of
/// the rule the invocation is made for, and the account that rule belongs to.
///
/// The account is the RULE's own, read off its ARN through the one shared parse,
/// so a rule of another account is reported under that account rather than under
/// this emulator's.
fn target_invocation_condition_context(rule_arn: &str) -> Vec<(String, String)> {
    let account = ::service_common::arn::parse(rule_arn)
        .map(|arn| arn.account.to_string())
        .unwrap_or_default();
    vec![
        (SOURCE_ARN_CONDITION_KEY.to_string(), rule_arn.to_string()),
        (SOURCE_ACCOUNT_CONDITION_KEY.to_string(), account),
    ]
}

/// Whether the TARGET whose own resource policy is `policy` admits the invocation
/// `attempt` describes.
///
/// "For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
/// relies on resource-based policies": such a target is invoked as this
/// service's own principal ([`SERVICE_PRINCIPAL`]) rather than as the target
/// owner or as a role, and a service principal holds no identity-based policies
/// — so the TARGET's document is the only one in force and it alone decides the
/// invocation. In IAM's own order: an explicit `Deny` about the invocation
/// refuses it whatever any grant says, and otherwise the invocation needs an
/// `Allow` about it — a target carrying no policy at all, or one whose
/// statements are about something else, has granted nothing, which is IAM's
/// implicit deny. That is the opposite reading from a bus's own policy
/// ([`publication_refusal`]), where the publisher is an identity whose own
/// documents may grant the call; here there is no second document to grant it.
///
/// The decision carries the keys such a grant is scoped by
/// ([`target_invocation_condition_context`]), so a statement conditioned on the
/// invoking rule is decided for the rule being invoked and no other: a grant
/// scoped to one rule's ARN authorizes that rule's deliveries alone.
///
/// WHICH of the two refusals it was is deliberately not answered: the target's
/// service does not tell EventBridge either, and what EventBridge reports for a
/// delivery it could not make is the answer that service gave it (see
/// [`queue_access_refusal`]).
pub fn target_policy_admits(
    policy: Option<&str>,
    attempt: &DeliveryAttempt<'_>,
    rule_arn: &str,
) -> bool {
    policy.is_some_and(|document| {
        target_policy_decision(document, attempt, rule_arn)
            == ::service_common::iam_authorization::Decision::Allowed
    })
}

/// What the TARGET's document `policy` states about the invocation `attempt`
/// describes, in IAM's own three answers.
///
/// The decision [`target_policy_admits`] reads, answered rather than collapsed to
/// a grant for the target over which SEVERAL documents are in force — a log
/// group's own resource policy and the account's are both over a write into that
/// group ([`log_group_target::documents_admit`]) — because IAM's order spans the
/// whole set: an explicit `Deny` in ANY document refuses the invocation whatever
/// another grants, which one document's verdict cannot say.
pub fn target_policy_decision(
    policy: &str,
    attempt: &DeliveryAttempt<'_>,
    rule_arn: &str,
) -> ::service_common::iam_authorization::Decision {
    let (action, resource) = (attempt.action, attempt.resource);
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let context = target_invocation_condition_context(rule_arn);
    let request = ::service_common::iam_authorization::Request::new(action, resource, &principals)
        .with_context(&context)
        .with_published_keys(&TARGET_INVOCATION_CONDITION_KEYS);
    ::service_common::iam_authorization::resource_decision(policy, &request)
}

/// The condition EventBridge reports for a delivery whose target was not there
/// for it to hand the event to.
///
/// Distinct from [`NO_PERMISSIONS`], which is about the identity the call was
/// made as: this one is about the RESOURCE, and it is what a target service
/// answering "no such resource" earns. The two are the service's OWN
/// classification of a failed delivery and never the target service's error code,
/// which the dead letter reports in its `ERROR_MESSAGE` beside the condition.
pub const NO_RESOURCE: &str = "NO_RESOURCE";

/// The fixed text Amazon SQS answers a caller that may not address the queue it
/// named, as that service words an authorization failure: the principal, the
/// action and the queue, and the document that withheld the grant. Read from the
/// SHARED sentence (`::service_common::iam_authorization`), so the refusal a
/// delivery reports and the one Amazon SQS answers a client with cannot drift
/// apart.
///
/// The action is named the way that service names it in a refusal — case-folded
/// (`service-com-amazonaws-sqs-amazonsqs`'s `state::authorization`) — and the
/// answer carries the AWS-common `AccessDenied` with HTTP 403, because Amazon
/// SQS's model declares no shape for an authorization failure.
const SQS_SEND_MESSAGE_ACTION_NAME: &str = "sqs:sendmessage";

/// The service name Amazon SQS's own error metadata carries. `pub(crate)`
/// because every refusal this service reports of a call on a queue carries that
/// block — the policy refusal here and the encrypted-queue one in
/// `crate::ops::put_events` — so the name is written once.
pub(crate) const SQS_SERVICE_NAME: &str = "AmazonSQS";

/// The error code and status that answer carries.
const SQS_ACCESS_DENIED_CODE: &str = "AccessDenied";
const SQS_ACCESS_DENIED_STATUS: u16 = 403;

/// The condition and the sentence EventBridge reports for a queue target it could
/// not hand an event to, because the queue granted this service nothing.
///
/// The queue's own policy is what governs the delivery
/// ([`target_policy_admits`]), and what the delivery SEES when that document
/// grants nothing is Amazon SQS's own refusal of the `sqs:SendMessage` — the
/// authorization sentence naming the principal the call was made as, the action
/// and the queue, followed by the metadata block an AWS error carries. So the
/// condition names the PERMISSION ([`NO_PERMISSIONS`]), which is what the
/// delivery failed for: the identity it was made as was not allowed the call,
/// and the resource was there all along ([`NO_RESOURCE`] is the condition of a
/// target that is NOT).
///
/// The request id is MINTED here and nowhere else: it identifies the one refused
/// call, so it is drawn fresh rather than derived from the event or the target,
/// which is what makes two refused deliveries distinguishable in a dead-letter
/// queue. It comes from the tree's one mint of an AWS request id
/// (`::service_common::query_error_envelope::new_request_id`), so this service
/// cannot report one in a form no other service reports.
pub fn queue_access_refusal(queue_arn: &str) -> (String, String) {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    (
        NO_PERMISSIONS.to_string(),
        format!(
            "{} (Service: {SQS_SERVICE_NAME}; Status Code: {SQS_ACCESS_DENIED_STATUS}; Error \
             Code: {SQS_ACCESS_DENIED_CODE}; Request ID: {request_id}; Proxy: null)",
            ::service_common::iam_authorization::not_authorized_by_resource_policy(
                SERVICE_PRINCIPAL,
                SQS_SEND_MESSAGE_ACTION_NAME,
                queue_arn,
            )
        ),
    )
}

/// The IAM action publishing an event onto an event bus performs — the action an
/// event bus's resource policy grants or denies a publisher, and the one
/// `PutPermission` installs a grant for.
pub const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// `events:source`: the condition key carrying the `Source` of the event being
/// published.
pub const SOURCE_CONDITION_KEY: &str = "events:source";

/// `events:detail-type`: the condition key carrying the `DetailType` of the
/// event being published.
pub const DETAIL_TYPE_CONDITION_KEY: &str = "events:detail-type";

/// `events:detail.service`.
pub const DETAIL_SERVICE_CONDITION_KEY: &str = "events:detail.service";

/// `events:detail.eventTypeCode`.
pub const DETAIL_EVENT_TYPE_CODE_CONDITION_KEY: &str = "events:detail.eventTypeCode";

/// `events:detail.userIdentity.principalId`.
pub const DETAIL_PRINCIPAL_ID_CONDITION_KEY: &str = "events:detail.userIdentity.principalId";

/// The condition keys read out of the published event's `Detail` document, each
/// with the member path inside that document whose value it carries.
///
/// The key names the path: `events:detail.service` is `Detail`'s `service`
/// member and `events:detail.userIdentity.principalId` the `principalId` member
/// of its `userIdentity` object, so the table says in one place what each key
/// reads.
const DETAIL_CONDITION_KEYS: [(&str, &[&str]); 3] = [
    (DETAIL_SERVICE_CONDITION_KEY, &["service"]),
    (DETAIL_EVENT_TYPE_CODE_CONDITION_KEY, &["eventTypeCode"]),
    (
        DETAIL_PRINCIPAL_ID_CONDITION_KEY,
        &["userIdentity", "principalId"],
    ),
];

/// Every condition key a publication onto an event bus carries a value for — the
/// EVENT-CONTENT keys AWS publishes for `events:PutEvents`, which is what a
/// statement about that action can be scoped by.
///
/// Each reads one member of the event being published: the entry's `Source`
/// ([`SOURCE_CONDITION_KEY`]) and `DetailType`
/// ([`DETAIL_TYPE_CONDITION_KEY`]), and the three members of its `Detail`
/// document [`DETAIL_CONDITION_KEYS`] names. They are DECLARED on the request
/// ([`::service_common::iam_authorization::Request::with_published_keys`]), so a
/// key this list carries and the event provides no value for is a key the
/// publication really has no value for rather than one this emulator cannot
/// decide — which is what lets a clause written with IAM's `IfExists` suffix, or
/// a `Null` clause, be answered for it.
pub const PUBLICATION_CONDITION_KEYS: [&str; 5] = [
    SOURCE_CONDITION_KEY,
    DETAIL_TYPE_CONDITION_KEY,
    DETAIL_SERVICE_CONDITION_KEY,
    DETAIL_EVENT_TYPE_CODE_CONDITION_KEY,
    DETAIL_PRINCIPAL_ID_CONDITION_KEY,
];

/// The condition context ONE publication carries: a `(key, value)` pair for
/// every key of [`PUBLICATION_CONDITION_KEYS`] the event provides a value for.
///
/// `detail` is the event's `Detail` as a parse tree ([`parse_detail`], or the
/// `detail` member of an envelope being re-published), because the keys under it
/// are members of that document rather than text of the request. Only a STRING
/// member contributes a value: a member of any other JSON type names no text a
/// string comparison could read.
#[must_use]
pub fn publication_condition_context(
    source: Option<&str>,
    detail_type: Option<&str>,
    detail: Option<&::serde_json::Value>,
) -> Vec<(String, String)> {
    let mut context: Vec<(String, String)> = [
        (SOURCE_CONDITION_KEY, source),
        (DETAIL_TYPE_CONDITION_KEY, detail_type),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key.to_string(), value.to_string())))
    .collect();
    for (key, path) in DETAIL_CONDITION_KEYS {
        let member = path
            .iter()
            .fold(detail, |at, step| at.and_then(|value| value.get(step)));
        if let Some(text) = member.and_then(::serde_json::Value::as_str) {
            context.push((key.to_string(), text.to_string()));
        }
    }
    context
}

/// The condition context of a publication made from an event ENVELOPE rather
/// than from a request entry — the shape a bus-to-bus forward re-publishes, whose
/// members [`event_envelope`] names.
#[must_use]
pub fn envelope_publication_context(
    envelope: &::serde_json::Map<String, ::serde_json::Value>,
) -> Vec<(String, String)> {
    let text = |field: &str| {
        envelope
            .get(field)
            .and_then(::serde_json::Value::as_str)
    };
    publication_condition_context(text("source"), text("detail-type"), envelope.get("detail"))
}

/// The sentence AWS refuses a request with when the resource it names carries a
/// statement that explicitly DENIES the calling principal the action: the
/// principal that asked, the action it asked for, the resource it asked on, and
/// the clause naming the kind of statement the deny came from.
///
/// The counterpart of the shared sentence for a resource policy that merely
/// grants nothing
/// (`::service_common::iam_authorization::not_authorized_by_resource_policy`),
/// stated once here so both publication gates of this service — the one in front
/// of a client's `PutEvents` ([`authorization::publication_denial`]) and the one
/// in front of a bus-to-bus forward ([`publication_refusal`]) — word a deny
/// identically.
fn explicit_resource_policy_deny(principal: &str, action: &str, resource: &str) -> String {
    not_authorized(
        principal,
        action,
        resource,
        "with an explicit deny in a resource-based policy",
    )
}

/// The sentence AWS refuses a call with when the policies in force do not permit
/// it: who was refused, which action, on which resource, and the clause naming
/// the document that withheld the grant.
///
/// Every authorization refusal of this service is built here — the ones a
/// delivery earns ([`delivery_refusal`]), the one a bus's own policy states
/// ([`explicit_resource_policy_deny`]) and the one a client's publication earns
/// ([`authorization::publication_denial`]) — so the three cannot drift in
/// wording. Only the closing clause differs between them, because the three ask
/// for different remedies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         {reason}"
    )
}

/// The IAM principal a request that presents the account's OWN published
/// credential speaks for — the one `sts:GetCallerIdentity` reports for it — which
/// is the identity such a publication is authorized as
/// ([`authorization::request_caller`] resolves the general case, for every
/// credential) and the one an authorization refusal of it names.
///
/// Read from the shared definition, so this service refuses in the name of the
/// same principal every other service of this emulator names.
#[must_use]
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// Whether the policies in force let a CLIENT's publication onto an event bus go
/// ahead, and who that client is: a client's `PutEvents` is the action
/// [`PUT_EVENTS_ACTION`] on the bus its entry names, so it is authorized against
/// the calling principal's identity-based policies and that bus's own resource
/// policy together ([`authorization::publication_denial`]) — where a bus-to-bus
/// forward, made as a target's role rather than as a client,
/// is [`publication_refusal`].
pub mod authorization;

/// Whether the policies in force let the calling principal CHANGE a rule: a rule
/// mutator is the action `events:<OperationName>` on the rule's own ARN, decided
/// against the caller's identity-based policies with the TARGET rule's tags
/// published as `aws:ResourceTag/<key>` and the request keys the CALL itself
/// carries beside them — the ARN of each target a target-attaching call attaches,
/// as `events:TargetArn`
/// ([`rule_authorization::rule_mutation_denial`],
/// [`rule_authorization::target_attachment_denial`]).
pub mod rule_authorization;

/// Every spelling of the identity a publication is made as that a resource
/// policy may name it by: the role itself, the SESSION of that role the
/// publication is made under ([`assumed_role_principal`]) and the spellings of
/// the account the role belongs to.
///
/// The account's own spellings are included because this decides a DENY, and a
/// statement that denies an account refuses each of its principals
/// (`::service_common::iam_authorization::account_principals`); a GRANT would be
/// decided with the principal's spellings alone. A publication under NO role is
/// made by EventBridge itself, so the only identity to name is its service
/// principal — which a statement denying every principal still names.
fn publication_principals(role_arn: Option<&str>, attempt: &DeliveryAttempt<'_>) -> Vec<String> {
    let Some(role_arn) = role_arn else {
        return vec![SERVICE_PRINCIPAL.to_string()];
    };
    let mut principals = vec![
        role_arn.to_string(),
        publication_principal(Some(role_arn), attempt),
    ];
    principals.extend(::service_common::iam_authorization::account_principals(
        role_arn,
    ));
    principals
}

/// The identity a refused publication is REPORTED as: the session of the role it
/// was made under, or EventBridge's own service principal for one made under no
/// role at all.
fn publication_principal(role_arn: Option<&str>, attempt: &DeliveryAttempt<'_>) -> String {
    let Some(role_arn) = role_arn else {
        return SERVICE_PRINCIPAL.to_string();
    };
    // The role's NAME comes from its ARN through the shared reading of one
    // (`::service_common::iam_role`), so the session principal is spelled the
    // same way whether or not this account holds a record of the role.
    let role_name = ::service_common::iam_role::role_name(role_arn).unwrap_or_default();
    assumed_role_principal(role_arn, role_name, attempt)
}

/// Which invocation the delivery gate is deciding: the API call EventBridge is
/// about to make, on which resource, for which rule target.
///
/// The rule and the target are part of it because the identity the call is made
/// as is a SESSION of the target's role, and a session is opened per rule target
/// ([`target_role_session_name`]).
#[derive(Debug, Clone, Copy)]
pub struct DeliveryAttempt<'a> {
    /// The rule whose target is being invoked.
    pub rule: &'a str,
    /// `Target$Id`: the id of that target within the rule.
    pub target_id: &'a str,
    /// The IAM action the invocation performs.
    pub action: &'a str,
    /// The resource it performs it on: the target's ARN.
    pub resource: &'a str,
}

/// The prefix the session name of a target invocation carries, naming the
/// service that opened the session.
const SESSION_NAME_PREFIX: &str = "AWSEvents_";

/// Longest role session name, in characters.
///
/// `sts:AssumeRole`'s `RoleSessionName` is bounded at 64 characters, so a session
/// name assembled from a rule name and a target id — each of which may be that
/// long on its own — is clipped to what the session can carry.
const MAX_SESSION_NAME_CHARS: usize = 64;

/// The name of the role SESSION a rule's target is invoked under.
///
/// A role is never acted as directly: it is acted as through a session, and the
/// session a target invocation runs under is named for the service that opened it
/// and the rule target it was opened for, so a refusal that names the session
/// principal says which target of which rule was being invoked.
pub fn target_role_session_name(rule: &str, target_id: &str) -> String {
    let name = format!("{SESSION_NAME_PREFIX}{rule}_{target_id}");
    name.chars().take(MAX_SESSION_NAME_CHARS).collect()
}

/// The principal EventBridge makes a role-governed target's API call as: a
/// session of the role the target names
/// (`arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`),
/// which is the identity an authorization refusal is about.
///
/// The partition and the account are the role's own, read off its ARN through the
/// ONE ARN parse, so a role of another partition or account is named as that
/// role's session rather than as this emulator's.
fn assumed_role_principal(role_arn: &str, role_name: &str, attempt: &DeliveryAttempt<'_>) -> String {
    let parsed = ::service_common::arn::parse(role_arn);
    let partition = parsed.map_or("aws", |arn| arn.partition);
    let account = parsed.map_or("", |arn| arn.account);
    format!(
        "arn:{partition}:sts::{account}:assumed-role/{role_name}/{}",
        target_role_session_name(attempt.rule, attempt.target_id)
    )
}

/// The identity-based policy documents in force on the role `role_arn` names: its
/// inline role policies plus the documents of the customer-managed policies
/// attached to it.
///
/// Called with the IAM guard held, and it CLONES what it reads, so the deciding
/// walk holds no IAM guard.
///
/// `None` when nothing can be SHOWN about the role's permissions — the store
/// holds no such role, one of its attachments is an AWS-managed policy whose
/// document this account does not hold, or it carries more documents than
/// [`MAX_ROLE_DOCUMENTS`].
///
/// The same IAM-STORE LOOKUP lives in
/// `service-com-amazonaws-fis-faultinjectionsimulator/src/state/action_permissions.rs`
/// and in Transfer Family's and CloudWatch RUM's authorization modules, for the
/// reason that module records: lifting it beside the shared evaluator would make
/// `services/common` depend on the IAM SERVICE crate, which is a change to every
/// service rather than to this one. What is NOT duplicated is the rule itself —
/// the trust reading and the policy evaluation are
/// [`::service_common::iam_trust_policy`] and
/// [`::service_common::iam_authorization`], called here, so two services handed
/// the same documents cannot answer differently.
pub fn role_documents(role_arn: &str, iam: &IamState) -> Option<Vec<String>> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let role_key = iam_store::name_key(&role.role_name);
    let mut documents: Vec<String> = iam
        .role_inline_policies
        .get(&role_key)
        .into_iter()
        .flat_map(|policies| policies.values())
        .map(|policy| policy.policy_document.clone())
        .collect();
    for arn in iam
        .role_attached_policies
        .get(&role_key)
        .into_iter()
        .flatten()
    {
        match default_policy_document(iam, arn) {
            Some(document) => documents.push(document),
            // An AWS-managed policy might be the one that carries the grant, and
            // this account cannot read it: nothing can be shown about this role.
            // Which ARNs those are is IAM's OWN reading, called rather than
            // re-derived.
            None if iam_store::is_aws_managed_policy_arn(arn) => return None,
            None => {}
        }
    }
    // More documents than this gate reads: an unread one might carry the grant.
    if documents.len() > MAX_ROLE_DOCUMENTS {
        return None;
    }
    Some(documents)
}

/// The default version's document of the managed policy `arn` names, or `None`
/// when the account holds no such policy or no document for its default version.
fn default_policy_document(iam: &IamState, arn: &str) -> Option<String> {
    let managed = iam.policy_by_arn(arn)?;
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|default| managed.versions.get(default))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

/// The bus name a request addresses.
///
/// `EventBusNameOrArn` admits either a bare name or an event-bus ARN — its
/// modelled pattern spells both out:
/// `^(arn:aws[\w-]*:events:[a-z]+-[a-z]+-[\w-]+:[0-9]{12}:event-bus\/)?[/\.\-_A-Za-z0-9]+$`
/// — so an ARN is reduced to everything after its `event-bus/` resource-type
/// segment. That is the whole remainder, NOT the text after the ARN's last `/`: a
/// bus name may itself contain `/` (`EventBusName`'s pattern admits it, and a
/// partner event bus name such as `aws.partner/example.com/1234/stream` is built
/// from them), and cutting at the last `/` would address a different bus. An
/// absent or empty member means the account's `default` bus.
pub fn effective_bus_name(event_bus_name_or_arn: Option<&str>) -> String {
    let named = event_bus_name_or_arn
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_EVENT_BUS_NAME);
    match named.split_once(":event-bus/") {
        Some((prefix, name)) if prefix.starts_with("arn:") && !name.is_empty() => name.to_string(),
        _ => named.to_string(),
    }
}

/// Whether `value` names one of `RuleState`'s values.
pub fn is_rule_state(value: &str) -> bool {
    RULE_STATE_VALUES.contains(&value)
}

/// A resource of this service a tag can be written to, as an ARN names one.
///
/// "In EventBridge, rules and event buses can be tagged" (`TagResource`), and
/// those are exactly the two resource kinds this emulator holds, so an ARN
/// resolves to one of them or to nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `arn:aws:events:<region>:<account>:event-bus/<name>`.
    EventBus { region: String, name: String },
    /// `arn:aws:events:<region>:<account>:rule/<bus-if-not-default>/<name>`.
    Rule {
        region: String,
        event_bus_name: String,
        name: String,
    },
}

/// The resource an `Arn` names, or `Err(reason)` when the text names no
/// taggable resource of this service at all.
///
/// The ONE place a tagging operation reads a `ResourceARN`, so `TagResource`,
/// `UntagResource` and `ListTagsForResource` resolve one ARN to one record. The
/// forms are the ones this service MINTS ([`event_bus_arn`], [`rule_arn`]): an
/// `events` ARN whose resource segment is `event-bus/<name>`, `rule/<name>` (a
/// rule on the account's `default` bus) or `rule/<bus>/<name>`. A rule name
/// carries no `/` (`RuleName`'s pattern `^[\.\-_A-Za-z0-9]+$`) while a bus name
/// may, so a rule ARN's name is the text after its LAST `/` and the bus is
/// everything between the resource type and it.
///
/// `Arn` is modelled as a plain 1..1600 string, so the boundary validator
/// passes any text: a value that is not an ARN, or names another service, or
/// names a resource kind this service does not tag, is a bad request — not a
/// missing resource. Whether the named resource EXISTS is a separate question,
/// answered by [`ComAmazonawsEventbridgeAwseventsState::tags_of`]; the Region
/// the ARN carries IS part of the identity this store keys a bus and a rule on
/// ([`BusKey`]), so it is read off the ARN here — and an ARN whose region
/// segment is empty (the modelled pattern admits one) names a resource in
/// `request_region`, the Region the call itself was signed for.
pub fn parse_tagged_resource(arn: &str, request_region: &str) -> Result<TaggedResource, String> {
    let invalid = || {
        format!("Parameter ResourceARN is not valid. Reason: {arn:?} is not the ARN of an EventBridge rule or event bus.")
    };
    if !is_arn(arn) {
        return Err(invalid());
    }
    let region = arn_region(arn).unwrap_or(request_region).to_string();
    let mut fields = arn.splitn(6, ':');
    let service = fields.nth(2).unwrap_or_default();
    let resource = fields.nth(2).unwrap_or_default();
    if service != ARN_NAMESPACE {
        return Err(invalid());
    }
    if let Some(name) = resource.strip_prefix("event-bus/") {
        if name.is_empty() {
            return Err(invalid());
        }
        return Ok(TaggedResource::EventBus {
            region,
            name: name.to_string(),
        });
    }
    if let Some(path) = resource.strip_prefix("rule/") {
        let (event_bus_name, name) = match path.rsplit_once('/') {
            Some((bus, name)) => (bus, name),
            None => (DEFAULT_EVENT_BUS_NAME, path),
        };
        if event_bus_name.is_empty() || name.is_empty() {
            return Err(invalid());
        }
        return Ok(TaggedResource::Rule {
            region,
            event_bus_name: event_bus_name.to_string(),
            name: name.to_string(),
        });
    }
    Err(invalid())
}

/// Write `submitted` into the tag set `stored`, or `Err(reason)` when the
/// request, or the set it would leave, carries more than
/// [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// "If you specify a new tag key, this tag is appended to the list of tags
/// associated with the resource. If you specify a tag key that is already
/// associated with the resource, the new tag value that you specify replaces
/// the previous value for that tag" (`TagResource`) — so this MERGES: a key the
/// request names takes the value it names, in the position that key already
/// held, and a key the request does not name keeps the value it had. Entries of
/// one request are applied in order, so a request repeating a key leaves the
/// value it names LAST, which is the same rule applied twice.
///
/// The bound is charged on the REQUEST's own list FIRST, before anything
/// caller-sized is built: `TagList` carries no modelled `@length`, so the
/// request's entry count is bounded only by the request body, while the merged
/// set always holds at least the request's distinct keys — so a request
/// carrying more entries than the quota can never be stored anyway, and
/// refusing it up front keeps every allocation and every comparison below
/// bounded by the quota rather than by the body. Then the merge runs through a
/// key→position index, so it is ONE pass over the request rather than a scan of
/// the merged set per entry. This is the same order `PutTargets` charges
/// [`MAX_TARGETS_PER_RULE`] in.
///
/// All-or-nothing: the bound is charged against the merged set before anything
/// is written, so a refusal leaves the resource carrying exactly the tags it
/// carried before.
/// A key in the namespace reserved for Amazon Web Services' own tags is refused
/// here too ([`request_validation::reserved_tag_key_violation`]), so no tag write
/// of this service can set one.
pub fn merge_tags(stored: &mut Vec<Tag>, submitted: Vec<Tag>) -> Result<(), String> {
    bound_tag_count(submitted.len())?;
    if let Some(reason) = request_validation::reserved_tag_key_violation(&submitted) {
        return Err(reason);
    }
    let mut merged = stored.clone();
    let mut positions: BTreeMap<String, usize> = merged
        .iter()
        .enumerate()
        .map(|(position, tag)| (tag.key.clone(), position))
        .collect();
    for tag in submitted {
        match positions.get(&tag.key) {
            Some(&position) => merged[position].value = tag.value,
            None => {
                positions.insert(tag.key.clone(), merged.len());
                merged.push(tag);
            }
        }
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(too_many_tags());
    }
    *stored = merged;
    Ok(())
}

/// Why one request's `Tags` list cannot be stored on any resource, or `Ok(())`
/// when it can.
///
/// A resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags and a merge leaves
/// at least the request's distinct keys, so a longer list is refused whatever
/// the resource already carries. A handler calls this BEFORE it takes the store
/// lock, so the work a request can ask for inside the critical section is
/// bounded by the quota rather than by the request body.
pub fn bound_tag_count(count: usize) -> Result<(), String> {
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(too_many_tags());
    }
    Ok(())
}

/// What a refusal states for a tag write past [`MAX_TAGS_PER_RESOURCE`].
///
/// A per-resource tag ceiling is a COUNT quota of this service like the targets
/// a rule holds or the rules a bus holds, so the refusal carries the one
/// sentence this service states for a count quota ([`QUOTA_EXCEEDED`]) rather
/// than prose of this emulator's own: it names neither the resource the request
/// was about nor the number that bounds it, so it reads the same whichever
/// resource was tagged and whichever write crossed the quota. What differs from
/// the other quotas is only the channel it travels on, because the tag writes
/// model no quota error and refuse on the bad-request channel instead.
///
/// The quota is charged on the set a write would LEAVE — "You can use the
/// TagResource action with a resource that already has tags ... the new tag
/// value that you specify replaces the previous value for that tag"
/// (`TagResource`), so re-setting a stored key leaves the set the size it was.
fn too_many_tags() -> String {
    QUOTA_EXCEEDED.to_string()
}

/// Remove from `stored` every tag whose key `keys` names, leaving the order of
/// the ones that stay.
///
/// "Removes one or more tags from the specified EventBridge resource"
/// (`UntagResource`): the request names keys, so a key the resource does not
/// carry removes nothing and is not an error — the resource ends up carrying no
/// tag under any key the request named, which is what was asked for.
///
/// `TagKeyList` carries no modelled `@length`, so the request's key count is
/// bounded only by the request body: the keys are collected into an ordered SET
/// once and each stored tag is then looked up in it, so the work inside the
/// store lock is one lookup per STORED tag (at most
/// [`MAX_TAGS_PER_RESOURCE`] of them) instead of a scan of the request's list
/// per stored tag.
pub fn remove_tags(stored: &mut Vec<Tag>, keys: &[String]) {
    let removed: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
    stored.retain(|tag| !removed.contains(tag.key.as_str()));
}

/// What any reader reports for a rule name that names no rule on the bus the
/// lookup ran against.
///
/// The message is built from the request and names the resource the way this
/// service names a missing rule: the RULE by name, and the EVENT BUS it was
/// sought on — a rule name is only unique within its bus, so without the bus a
/// caller cannot tell "no such rule anywhere" from "that rule lives on another
/// bus". Written once, so every operation that resolves a rule — a read, a
/// target write, a tagging call — refuses a missing one in the same words.
pub fn no_such_rule(event_bus_name: &str, rule_name: &str) -> String {
    format!("Rule {rule_name} does not exist on EventBus {event_bus_name}.")
}

/// The name of the rule EventBridge provisions for an archive on its source bus:
/// `Events-Archive-<archive name>`.
///
/// An archive is served by a MANAGED rule on the bus that feeds it — the rule
/// that puts the bus's matching events into the archive — so the archive's
/// dependency on its bus is a rule a client can see with `ListRules`, and the
/// rule is protected from customer writes like every other managed rule
/// ([`managed_rule_refusal`]).
pub fn archive_rule_name(archive_name: &str) -> String {
    format!("Events-Archive-{archive_name}")
}

/// The `EventPattern` the rule of an archive carries: what the archive matches,
/// and never a replayed event.
///
/// "If you do not specify a pattern to filter events sent to the archive, all
/// events are sent to the archive except replayed events. Replayed events are not
/// sent to an archive" (`CreateArchive`) — the exclusion holds whether the archive
/// filters or not, so the rule's pattern is the archive's own pattern with the
/// `replay-name` clause added to it. An archive that filters nothing therefore
/// carries the exclusion alone.
pub fn archive_rule_pattern(event_pattern: Option<&str>) -> String {
    let mut pattern = event_pattern
        .and_then(|text| {
            ::serde_json::from_str::<::serde_json::Map<String, ::serde_json::Value>>(text).ok()
        })
        .unwrap_or_default();
    pattern.insert(
        REPLAY_NAME_FIELD.to_string(),
        ::serde_json::json!([{ "exists": false }]),
    );
    ::serde_json::Value::Object(pattern).to_string()
}

/// The envelope field a REPLAYED event carries, and an event published by a
/// client does not: what tells the rule of an archive the two apart
/// ([`archive_rule_pattern`]).
pub const REPLAY_NAME_FIELD: &str = "replay-name";

/// How many levels the event envelope wraps a `Detail` document in: `detail` is
/// one member of the envelope object, so an envelope nests exactly one level
/// deeper than the document it carries ([`event_envelope`]).
const ENVELOPE_LEVELS: usize = 1;

/// The envelope a replay re-publishes for one archived event: the event exactly
/// as the archive holds it, carrying the [`REPLAY_NAME_FIELD`] of the replay that
/// re-published it.
///
/// "Each redelivered event carries the name of the replay that redelivered it" is
/// what the archive's own rule reads to keep a replayed event out of the archive
/// ([`archive_rule_pattern`]), so the field is added HERE — the one place a
/// replayed envelope is built — rather than by each caller. Nothing else of the
/// event changes: its `id`, its `time` and its `detail` are the archived ones,
/// because a replay re-publishes the event it archived rather than a new event
/// built from it.
///
/// `None` when the archived text is not a JSON object, which is text no rule
/// could be matched against; the caller replays nothing for it rather than
/// inventing an envelope. Reading the text DESCENDS into it, so the published
/// [`MAX_DETAIL_DEPTH`] is charged for it and the stack that depth needs is
/// provided — which is what [`parse_delivered_event`] is, so the bound on an
/// event's text is stated once and holds for a replayed envelope as it does for
/// a delivered one.
pub fn replayed_envelope(archived: &str, replay_name: &str) -> Option<::serde_json::Value> {
    let mut envelope = parse_delivered_event(archived)?;
    envelope.as_object_mut()?.insert(
        REPLAY_NAME_FIELD.to_string(),
        ::serde_json::Value::String(replay_name.to_string()),
    );
    Some(envelope)
}

/// The rule that serves `archive`, as this service provisions it on the archive's
/// source bus.
///
/// Its `EventPattern` MIRRORS the archive's ([`archive_rule_pattern`]), so what
/// the rule selects and what the archive keeps are the same thing, and its one
/// target is the archive the selected events go to.
pub fn archive_rule(region: &str, archive: &ArchiveRecord) -> RuleRecord {
    let name = archive_rule_name(&archive.name);
    // The one target of the rule is the ARCHIVE itself: that is where the events
    // the rule selects go. Its `Id` is the rule's own name, so the target a
    // client sees on the rule names the archive that owns it.
    let target = Target {
        id: name.clone(),
        arn: archive.arn.clone(),
        ..Target::default()
    };
    RuleRecord {
        arn: rule_arn(region, &archive.event_bus_name, &name),
        event_pattern: Some(archive_rule_pattern(archive.event_pattern.as_deref())),
        schedule_expression: None,
        state: RULE_STATE_ENABLED.to_string(),
        description: None,
        role_arn: None,
        // The rule belongs to this service, which created it on the account's
        // behalf — which is what makes it a managed rule.
        managed_by: Some(SERVICE_PRINCIPAL.to_string()),
        created_by: Some(account_id().to_string()),
        event_bus_name: archive.event_bus_name.clone(),
        targets: vec![StoredTarget {
            id: target.id.clone(),
            arn: target.arn.clone(),
            role_arn: None,
            input: None,
            input_path: None,
            raw: target,
        }],
        tags: Vec::new(),
        name,
    }
}

/// What any writer of a rule reports for a rule another Amazon Web Services
/// service owns.
///
/// "This rule was created by an Amazon Web Services service on behalf of your
/// account. It is managed by that service ... You cannot modify these managed
/// rules by using DisableRule, EnableRule, PutTargets, PutRule, TagResource, or
/// UntagResource" (`ManagedRuleException`). The message is built from the
/// request — the rule by name — and says what is true of it: it is a managed
/// rule. Written once, so every writer refuses a managed rule in the same words.
pub fn managed_rule_refusal(rule_name: &str) -> String {
    format!("{rule_name} is a managed rule.")
}

/// The same refusal, from the TAGGING operations.
///
/// `TagResource` and `UntagResource` are not writes OF the rule but writes of the
/// tags on it, and they say so: a tag operation on a managed rule is the thing
/// that is not allowed.
pub fn managed_rule_tag_refusal(rule_name: &str) -> String {
    format!("Tag related operations on the managed rule {rule_name} is not allowed.")
}

/// The same refusal, from the two operations whose `Force` parameter overrides
/// it.
///
/// "If you see this error in response to DeleteRule or RemoveTargets, you can use
/// the Force parameter in those calls to delete the rule or remove targets from
/// the rule" (`ManagedRuleException`), so those two name the way out; every other
/// writer has none to name.
pub fn managed_rule_refusal_force_overrides(rule_name: &str) -> String {
    format!(
        "{} Set 'force' parameter to true to override.",
        managed_rule_refusal(rule_name)
    )
}

/// The clause every refusal of a connection call opens with, for the call whose
/// own verb is `verb` ("create", "describe", "update", "delete",
/// "deauthorize").
///
/// EventBridge's connection refusals say first WHICH call could not be served
/// and then why, so the reason that follows is read against the operation that
/// failed rather than against the resource alone — the same form the
/// api-destination refusals of this service use
/// ([`api_destination_call_failed`]). Written once, so every operation on this
/// resource opens its refusals in the same words.
pub fn connection_call_failed(verb: &str) -> String {
    format!("Failed to {verb} the connection(s).")
}

/// One refusal of a connection call: the failing call
/// ([`connection_call_failed`]) and then `reason`.
pub fn connection_refusal(verb: &str, reason: &str) -> String {
    format!("{} {reason}", connection_call_failed(verb))
}

/// What a connection write states about `AuthParameters` that do not name
/// exactly one authorization type.
///
/// Each authorization type has exactly one member of the auth-parameters shape
/// that carries its credentials, so a set naming none of them and a set naming
/// several both fail one rule, and the service states that rule rather than
/// naming a member of the request. Written once, so `CreateConnection` and
/// `UpdateConnection` refuse it in the same words; the failing call's own clause
/// ([`connection_refusal`]) is stated before it.
pub const ONE_AUTHORIZATION_TYPE: &str =
    "Parameter AuthParameters is not valid. Reason: The value for AuthParameters must include \
     exactly one authorization type.";

/// What any refusal states about a name this store holds no connection for: the
/// missing CONNECTION by name, as the request named it.
///
/// The clause on its own, without the failing call's own prefix, because the
/// call that ran into a missing connection is not always a connection call: an
/// api-destination write resolving its `ConnectionArn` opens with ITS call
/// ([`api_destination_call_failed`]) and then states this.
pub fn connection_does_not_exist(name: &str) -> String {
    format!("Connection '{name}' does not exist.")
}

/// What a create reports for a name this store already holds a connection under:
/// the connection the request NAMED, quoted, and that one exists already.
///
/// The counterpart of [`connection_does_not_exist`], worded the way this
/// resource's refusals word an identity that is taken: the clause describes the
/// connection by the `Name` the request carried and states what is true of it.
/// It carries no verb of its own, because the failing call is stated before it
/// ([`connection_refusal`]).
pub fn connection_already_exists(name: &str) -> String {
    format!("A connection with name '{name}' already exists.")
}

/// What any operation on the connection resource reports for a name this store
/// holds no connection for: the failing call and then the missing connection.
///
/// Written once, for the reason [`no_such_rule`] gives: a read, an update, a
/// deauthorize and a delete of a connection that does not exist all refuse it in
/// the same words, differing only in the verb of the call that failed.
pub fn no_such_connection(verb: &str, name: &str) -> String {
    connection_refusal(verb, &connection_does_not_exist(name))
}

/// The name of the connection a `ConnectionArn` names, or `None` when the text is
/// not one of the ARNs this service mints ([`connection_arn`]).
pub fn connection_name_in_arn(arn: &str) -> Option<&str> {
    let (prefix, resource) = arn.split_once(":connection/")?;
    let (name, id) = resource.split_once('/')?;
    (prefix.starts_with("arn:") && !name.is_empty() && !id.is_empty()).then_some(name)
}

/// What any reader reports for a bus name that names no event bus: the missing
/// BUS by name, built from the request. Written once, for the reason
/// [`no_such_rule`] gives.
pub fn no_such_event_bus(bus_name: &str) -> String {
    format!("Event bus {bus_name} does not exist.")
}

/// What any reader reports for a name — or an `ArchiveArn` — that names no
/// archive: the missing ARCHIVE, built from the request. Written once, for the
/// reason [`no_such_rule`] gives: `DescribeArchive` and a replay resolving its
/// event source refuse a missing archive in the same words.
pub fn no_such_archive(archive_name: &str) -> String {
    format!("Archive {archive_name} does not exist.")
}

/// What any reader reports for a name that names no replay: the missing REPLAY,
/// built from the request. Written once, for the reason [`no_such_rule`] gives.
pub fn no_such_replay(replay_name: &str) -> String {
    format!("Replay {replay_name} does not exist.")
}

/// The clause every refusal of an API-destination call opens with, for the call
/// whose own verb is `verb` ("describe", "update", "create", "delete").
///
/// EventBridge's api-destination refusals say first WHICH call could not be
/// served and then why, so the reason that follows is read against the operation
/// that failed rather than against the resource alone. Written once, so every
/// operation on this resource opens its refusals in the same words.
pub fn api_destination_call_failed(verb: &str) -> String {
    format!("Failed to {verb} the api-destination(s).")
}

/// One refusal of an api-destination call: the failing call
/// ([`api_destination_call_failed`]) and then `reason`.
///
/// The counterpart of [`connection_refusal`], so both resources compose their
/// clause in ONE place — a call on this resource that ran into a missing
/// connection states its own call here and the connection clause
/// ([`connection_does_not_exist`]) as the reason.
pub fn api_destination_refusal(verb: &str, reason: &str) -> String {
    format!("{} {reason}", api_destination_call_failed(verb))
}

/// What any operation on the API-destination resource reports for a name this
/// store holds no destination for: the failing call
/// ([`api_destination_call_failed`]) and then the destination the request named.
///
/// Written once, for the reason [`no_such_rule`] gives: a read, an update and a
/// delete of a destination that does not exist all refuse it in the same words,
/// differing only in the verb of the call that failed.
pub fn no_such_api_destination(verb: &str, name: &str) -> String {
    api_destination_refusal(verb, &format!("An api-destination '{name}' does not exist."))
}

/// Why a replay cannot be cancelled in the state it is in.
///
/// "An error occurred because a replay can be canceled only when the state is
/// Running or Starting" (`IllegalStatusException`): the refusal names the replay
/// — read from the request's `ReplayName`, which is the replay's identity — and
/// says that the state it was found in is not one this call can act on, whichever
/// of them it is.
pub fn replay_not_cancellable(replay_name: &str) -> String {
    format!("Replay {replay_name} is not in a valid state for this operation.")
}

/// Why a `LogConfig` is not one EventBridge accepts, or `None` when it is.
///
/// A submitted log configuration has to name BOTH of `LogConfig`'s members:
/// `Level` is "the level of logging detail to include ... applies to all log
/// destinations for the event bus" and `IncludeDetail` is "whether EventBridge
/// include detailed event information in the records it generates", so a
/// configuration leaving either out states nothing about that half of it and is
/// refused naming the missing member rather than stored as a configuration half
/// of which nothing decided.
///
/// Each member is also a modelled enum — `IncludeDetail`
/// (`INCLUDE_DETAIL_VALUES`) and `Level` (`LEVEL_VALUES`) — and the boundary
/// validator does not narrow an enum-typed string to its values, so a value the
/// enum does not admit is refused here rather than stored and later reported
/// back as if EventBridge had accepted it. A value that was SUPPLIED is judged
/// first, so a caller that sent one the enum does not admit reads that about the
/// member it sent. Written once, so `CreateEventBus` and `UpdateEventBus` accept
/// exactly the same configurations.
pub fn log_config_violation(config: &LogConfig) -> Option<String> {
    let members = [
        ("IncludeDetail", config.include_detail.as_deref(), INCLUDE_DETAIL_VALUES),
        ("Level", config.level.as_deref(), LEVEL_VALUES),
    ];
    for (member, value, admitted) in members {
        if let Some(value) = value {
            if !admitted.contains(&value) {
                return Some(format!(
                    "Parameter LogConfig.{member} is not valid. Reason: {value:?} is not one of {}.",
                    admitted.join(" | ")
                ));
            }
        }
    }
    members
        .iter()
        .find(|(_, value, _)| value.is_none())
        .map(|(member, _, _)| required_member_violation(&format!("LogConfig.{member}")))
}

/// Whether `value` is a well-formed ARN: the six colon-separated fields
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a partition,
/// a service and a resource.
///
/// What a target points at is outside this service, so only the ARN's own shape
/// can be checked — and `TargetArn` is modelled as a plain 1..1600 string, so
/// the boundary validator does not check even that.
pub fn is_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// Why a `PutTargets` request's `Targets` list names no single state to reach,
/// or `None` when it does.
///
/// `Id` is the key a target is STORED under — "The ID of the target within the
/// specified rule. Use this ID to reference the target when updating the rule"
/// (`Target$Id`), and `PutTargets` "adds the specified targets to the specified
/// rule, or updates the targets if they are already associated with the rule" —
/// so two entries of ONE request carrying the same `Id` ask for two different
/// targets at one key, and no stored target set satisfies the request. The
/// whole request is refused, before any of its entries is applied, rather than
/// one of the two silently winning.
///
/// The message is built from the request: it names the parameter and the `Id`
/// that repeated, the way this service's other request-derived refusals do.
pub fn duplicate_target_id_violation<'ids>(
    ids: impl IntoIterator<Item = &'ids str>,
) -> Option<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Some(format!(
                "Parameter targets is not valid. Reason: More than one target with Id \
                 '{id}' is provided."
            ));
        }
    }
    None
}

/// Why one target of a `PutTargets` request cannot be installed as given, or
/// `None` when it can.
///
/// Every ARN-valued member of a `Target` has to hold an ARN: `Arn` is "The
/// Amazon Resource Name (ARN) of the target", `RoleArn` "the ARN of the IAM
/// role to be used for this target", and `DeadLetterConfig$Arn` the ARN of the
/// queue dead-letter events go to. None of the three is modelled with a
/// pattern — they are plain 1..1600 strings — so the boundary validator passes
/// any text, and a value that is not an ARN names no resource the rule could
/// ever invoke. The request is refused WHOLE, before any of its targets is
/// stored: a rule carrying a target that points nowhere would report back an
/// invocation target AWS never accepted.
///
/// The message is built from the request, naming the value that was rejected
/// and the form that was expected.
pub fn target_arn_violation(target: &Target) -> Option<String> {
    [
        Some(target.arn.as_str()),
        target.role_arn.as_deref(),
        target
            .dead_letter_config
            .as_ref()
            .and_then(|config| config.arn.as_deref()),
    ]
    .into_iter()
    .flatten()
    .find(|value| !is_arn(value))
    .map(invalid_arn_violation)
}

/// What a refusal states for an ARN-valued member holding text that is not an
/// ARN: the value that was rejected and the form that was expected.
///
/// Stated once, so every member of this service that has to hold an ARN is
/// refused in the same words — the ones [`target_arn_violation`] answers for a
/// target's own members and [`request_validation::dead_letter_arn_violation`]
/// for a dead-letter destination that names no service at all.
pub fn invalid_arn_violation(value: &str) -> String {
    format!("Parameter {value} is not valid. Reason: Provided Arn is not in correct format.")
}

/// Why one published event entry names no event, or `None` when it names one.
///
/// "Detail, DetailType, and Source are required for EventBridge to successfully
/// send an event to an event bus. If you include event entries in a request
/// that do not include each of those properties, EventBridge fails that entry.
/// If you submit a request in which none of the entries have each of these
/// properties, EventBridge fails the entire request"
/// (`PutEventsRequestEntry$Detail`). None of the three is modelled `@required`,
/// so nothing refuses an entry without them before a handler runs. They are
/// checked in the order the entry documents them, so a refusal names the first
/// property the entry does not provide.
///
/// `Detail` counts as provided when the entry CARRIES it, which is what the
/// documentation asks of a request ("do not include each of those properties");
/// whether its text is a JSON object is a separate check, with its own
/// per-entry error code. `Source` and `DetailType` identify the event, so text
/// that is only whitespace names nothing and is treated as absent.
pub fn missing_entry_member(
    source: Option<&str>,
    detail_type: Option<&str>,
    detail: Option<&str>,
) -> Option<&'static str> {
    let names_something = |text: Option<&str>| text.is_some_and(|text| !text.trim().is_empty());
    [
        ("Detail", detail.is_some()),
        ("DetailType", names_something(detail_type)),
        ("Source", names_something(source)),
    ]
    .into_iter()
    .find_map(|(member, provided)| (!provided).then_some(member))
}

/// What a refusal states about an input member this service will not accept:
/// the member it is about and the reason it is not accepted.
///
/// The FORM every parameter-validation refusal of this service uses, whether it
/// leaves as a top-level error or on a batch entry's own row, so a caller reads
/// the same shape for every rejected member.
pub fn parameter_violation(member: &str, reason: &str) -> String {
    format!("{} Reason: {reason}", parameter_not_valid(member))
}

/// The subject half of that refusal on its own: the member is not valid, and
/// nothing about why.
///
/// Some refusals of this service state only this — the member the request got
/// wrong, with no reason clause — because the service itself discloses no reason
/// for them ([`request_validation::schedule_expression_violation`]). The two
/// forms share their opening, so a caller that looks for the member it has to
/// change finds it in the same place either way.
pub fn parameter_not_valid(member: &str) -> String {
    format!("Parameter {member} is not valid.")
}

/// What a refusal states for an entry member the entry does not provide.
pub fn required_member_violation(member: &str) -> String {
    parameter_violation(member, &format!("{member} is a required argument."))
}

/// Largest `DetailType` one published event entry may carry, in characters.
///
/// "Free-form string, with a maximum of 128 characters, used to decide what
/// fields to expect in the event detail" (`PutEventsRequestEntry$DetailType`).
/// The member's target shape is a plain `String` carrying no `@length`, so the
/// boundary validator passes a longer value and the documented limit has to be
/// enforced per entry — see [`entry_limit_violation`].
pub const MAX_DETAIL_TYPE_CHARS: usize = 128;

/// Why one published event entry is outside a limit AWS DOCUMENTS for an entry
/// member, or `None` when it is within every one of them.
///
/// These are the limits the entry's own documentation states while the model
/// expresses none of them, so nothing refuses them before a handler runs. An
/// entry violating one is not published: it is reported as a FAILED entry (the
/// per-entry `InvalidArgument`, "A specified parameter is not valid"), not as a
/// refusal of the whole request, because `PutEvents` reports one outcome per
/// entry.
///
/// Today that is `DetailType`'s [`MAX_DETAIL_TYPE_CHARS`]; `Source` and
/// `Detail` state no maximum of their own, and the size of the entry as a whole
/// is [`MAX_ENTRY_SIZE_BYTES`], checked separately by
/// [`put_events_entry_size`].
pub fn entry_limit_violation(detail_type: Option<&str>) -> Option<String> {
    let detail_type = detail_type?;
    (detail_type.chars().count() > MAX_DETAIL_TYPE_CHARS).then(|| {
        format!(
            "Parameter DetailType is not valid. Reason: DetailType can't have more than \
             {MAX_DETAIL_TYPE_CHARS} characters."
        )
    })
}

/// How deeply one published event entry's `Detail` may nest JSON containers.
///
/// "PutEvents will only process nested JSON up to 1000 levels deep" (this
/// service's `PutEvents` documentation): the published bound is on the DOCUMENT
/// the entry carries, not on the parser that happens to read it, so it is
/// stated here and checked by [`validate_detail`] — the `serde_json` parse-depth
/// default is an order of magnitude below it and refusing at that default would
/// reject documents AWS publishes.
pub const MAX_DETAIL_DEPTH: usize = 1000;

/// Why one entry's `Detail` cannot be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailRefusal {
    /// Not a JSON object: `Detail` is "a valid JSON object"
    /// (`PutEventsRequestEntry$Detail`), and a pattern reaches into it, so
    /// anything else names no event document.
    NotAnObject,
    /// Nests deeper than [`MAX_DETAIL_DEPTH`] containers; carries the depth
    /// that was measured, so the refusal states what it found.
    TooDeep(usize),
}

impl DetailRefusal {
    /// What a refused entry's `ErrorMessage` states. The condition is a
    /// property of the document the entry carried, so the wording names the
    /// bound it passed rather than echoing the document back.
    pub fn message(&self) -> String {
        match self {
            Self::NotAnObject => "Detail is malformed.".to_string(),
            Self::TooDeep(depth) => format!(
                "Detail nests JSON {depth} levels deep, which exceeds the maximum nesting depth \
                 of {MAX_DETAIL_DEPTH} levels."
            ),
        }
    }
}

/// How many levels of JSON containers `text` nests at its deepest point.
///
/// Measured by walking the bytes, not by recursing, so it answers for a
/// document far deeper than anything that may be parsed — which is what lets
/// [`MAX_DETAIL_DEPTH`] be enforced BEFORE a parser descends into it. Only the
/// container delimiters matter, and a delimiter inside a JSON string is text
/// rather than structure, so strings (and their escapes) are skipped. A closing
/// delimiter with nothing open is ignored: such text is not JSON at all and the
/// read reports that.
pub fn json_nesting_depth(text: &str) -> usize {
    let mut open = 0usize;
    let mut deepest = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                open += 1;
                deepest = deepest.max(open);
            }
            b'}' | b']' => open = open.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// Whether one entry's `Detail` is a publishable event document.
///
/// Both of its checks are made WITHOUT descending into the document: the depth
/// comes from [`json_nesting_depth`]'s byte walk, and the form comes from
/// reading the text as one JSON object whose member values are left as raw text,
/// which `serde_json` does with an explicit stack rather than by recursion. So
/// an entry is judged on whatever stack it arrives on, however deeply it nests,
/// and a document past the published [`MAX_DETAIL_DEPTH`] is refused here
/// without ever being read — only building the parse tree ([`parse_detail`])
/// costs depth, and nothing reaches that until this has passed.
///
/// The depth charge comes FIRST, so text that is both unbalanced and deep in
/// delimiters is reported as past the bound rather than as malformed — the
/// delimiter walk is what decides whether the text may be read at all, and both
/// answers are the same per-entry refusal to a caller.
pub fn validate_detail(detail: &str) -> Result<(), DetailRefusal> {
    let depth = json_nesting_depth(detail);
    if depth > MAX_DETAIL_DEPTH {
        return Err(DetailRefusal::TooDeep(depth));
    }
    let mut reader = ::serde_json::Deserializer::from_str(detail);
    // The document's own depth bound is the one checked above, so the parser's
    // default must not refuse first.
    reader.disable_recursion_limit();
    // Read as a stream of documents, which is how the parser is asked for ONE
    // value and then for the end of the input: text carrying a second document,
    // or trailing bytes that are not whitespace, is not one JSON document. The
    // member values stay raw, so nothing below the top level is descended into.
    type RawMembers = ::std::collections::BTreeMap<String, Box<::serde_json::value::RawValue>>;
    let mut documents = reader.into_iter::<RawMembers>();
    if !matches!(documents.next(), Some(Ok(_))) {
        return Err(DetailRefusal::NotAnObject);
    }
    if documents.next().is_some() {
        return Err(DetailRefusal::NotAnObject);
    }
    Ok(())
}

/// How deeply a JSON document may nest before its parse tree is built on a
/// stack of this module's own sizing.
///
/// Building a [`::serde_json::Value`] DESCENDS one frame per container level, and
/// a frame of that parse costs on the order of two kilobytes, so a document near
/// [`MAX_DETAIL_DEPTH`] needs more stack than a thread serving a request is
/// guaranteed to have. Up to this depth — where every realistic document sits —
/// the tree is built in place; past it the build moves to a thread whose stack is
/// sized for the published bound ([`DEEP_PARSE_STACK_BYTES`]). So how deeply a
/// document nests decides HOW the tree is built and never whether the process
/// survives building it.
const IN_PLACE_PARSE_DEPTH: usize = 64;

/// The stack a deep parse ([`parse_json_document`]) is given: a generous charge
/// per level of [`MAX_DETAIL_DEPTH`], so the deepest document this service
/// publishes is read with room to spare.
const DEEP_PARSE_STACK_BYTES: usize = 8 * 1024 * MAX_DETAIL_DEPTH;

/// The name the deep parse's thread carries, so a stack trace taken during one
/// says which work it belongs to.
const DEEP_PARSE_THREAD_NAME: &str = "eventbridge-json-parse";

/// `text` read as EXACTLY one JSON document, with the parser's own depth default
/// lifted — whatever stack this runs on is the stack the parse descends into, so
/// every caller comes through [`parse_json_document`], which provides one.
///
/// `None` for text that is not one JSON document: a second document, or trailing
/// bytes that are not whitespace, is not one.
fn read_one_json_document(text: &str) -> Option<::serde_json::Value> {
    let mut reader = ::serde_json::Deserializer::from_str(text);
    reader.disable_recursion_limit();
    let mut documents = reader.into_iter::<::serde_json::Value>();
    let Some(Ok(value)) = documents.next() else {
        return None;
    };
    documents.next().is_none().then_some(value)
}

/// One JSON document of `depth` levels ([`json_nesting_depth`]) as a parse tree,
/// read on a stack sized for that depth.
///
/// The ONE place this service builds a tree out of text a caller supplied. A
/// document no deeper than [`IN_PLACE_PARSE_DEPTH`] is read in place; a deeper
/// one is read on a scoped thread whose stack is sized for the published bound,
/// which is joined before this returns, so nothing is left running behind the
/// request. A scoped thread rather than a blocking-pool task: the SIZED stack is
/// the whole point, and a pool task takes the runtime's size, which no caller
/// here can set. A parse that panics, or a thread this process cannot start,
/// answers `None` — the same answer unreadable text gets.
fn parse_json_document(text: &str, depth: usize) -> Option<::serde_json::Value> {
    if depth <= IN_PLACE_PARSE_DEPTH {
        return read_one_json_document(text);
    }
    ::std::thread::scope(|scope| {
        ::std::thread::Builder::new()
            .name(DEEP_PARSE_THREAD_NAME.to_string())
            .stack_size(DEEP_PARSE_STACK_BYTES)
            .spawn_scoped(scope, || read_one_json_document(text))
            .ok()?
            .join()
            .ok()?
    })
}

/// One entry's `Detail` as the parse tree a rule's `EventPattern` is matched
/// against, or `None` when it is not a publishable document at all.
///
/// Building the tree is the one read of a `Detail` that DESCENDS into it, one
/// frame per level, so it is as deep as the document nests. The published
/// [`MAX_DETAIL_DEPTH`] is therefore charged HERE too, with the same byte walk
/// [`validate_detail`] uses and before the parser is handed anything: the bound
/// on the recursion is a property of this function rather than of the order its
/// callers happen to check things in, so no caller can reach the parser with a
/// document past it. The stack that depth needs is PROVIDED rather than assumed
/// ([`parse_json_document`]), so a document at the bound is read on a stack sized
/// for it instead of on whatever the request arrived on.
pub fn parse_detail(detail: &str) -> Option<::serde_json::Value> {
    let depth = json_nesting_depth(detail);
    if depth > MAX_DETAIL_DEPTH {
        return None;
    }
    let value = parse_json_document(detail, depth)?;
    value.is_object().then_some(value)
}

/// The text a target was DELIVERED, as the parse tree a delivery reads it from.
///
/// A Kinesis data stream target's `PartitionKeyPath` selects a value out of the
/// event the target is handed, so the delivery has to read that text back as a
/// document. Any JSON document is one — the handover may carry the target's own
/// `Input` rather than the event envelope — so the form is not narrowed here;
/// `None` for text that is not a single JSON document, or that nests past the
/// published bound, allowing for the level the envelope wraps a `Detail` in.
///
/// The depth is charged with the same byte walk [`parse_detail`] uses and before
/// the parser is handed anything, and the stack that depth needs is provided
/// ([`parse_json_document`]) — for the reasons [`parse_detail`] states.
pub fn parse_delivered_event(text: &str) -> Option<::serde_json::Value> {
    let depth = json_nesting_depth(text);
    if depth > MAX_DETAIL_DEPTH + ENVELOPE_LEVELS {
        return None;
    }
    parse_json_document(text, depth)
}

/// One clause of a `ValidationException` message: the value that was rejected,
/// the member path it sat at, and the requirement it failed.
///
/// `value` is `None` for a member that was ABSENT, reported as the null it was
/// rather than as a rendering of something the caller sent.
///
/// This and the three helpers below are the wording of a constraint refusal,
/// which `crate::service`'s `override_refusal` renders and a handler enforcing
/// an unmodelled constraint itself can reach, so a refusal found at the
/// boundary and one found in a handler read identically.
///
/// NOTE: the subject half, the requirement half and [`unanchored`] are the same
/// for every service and belong in `services/common/src/validation.rs` — whose
/// module docs record that migration as a change of its own. They are inlined
/// here, as they are in the ~24 other service crates that state them
/// (`grep -rln 'fn unanchored' services/*/src`), and are to be MOVED there
/// rather than copied again.
pub fn constraint_clause(member_path: &str, value: Option<&str>, requirement: &str) -> String {
    let subject = match value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value null".to_string(),
    };
    format!("{subject} at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The requirement clause for a `@pattern` violation: the expression the value
/// had to satisfy, rendered WITHOUT the anchors the model spells it with (see
/// [`unanchored`]).
pub fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// The `ValidationException` message over one or more clauses: how many
/// constraints were violated, then the clauses themselves.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with: the
/// requirement reported is the expression a value must satisfy, not the
/// anchoring the model uses to make the match total.
///
/// Only the expression's OUTER anchors go: an anchor inside the expression (one
/// per branch of an alternation, say) is part of its own structure, and
/// dropping it would change which strings the reported expression describes. A
/// `$` escaped by a backslash is a literal dollar sign rather than the end
/// anchor, so it stays.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The identifier an event bus's customer managed key is reported back under:
/// the ARN of the KMS resource the identifier names.
///
/// "The identifier can be the key Amazon Resource Name (ARN), KeyId, key alias,
/// or key alias ARN" (`CreateEventBus`'s `KmsKeyIdentifier`) — four spellings of
/// one key, of which only the ARN says which account and region the key is in.
/// EventBridge reports the key it stored, so an identifier that is not already
/// an ARN is completed into one in the caller's region and this account, and an
/// identifier that IS an ARN is reported as it stands. A bare alias (`alias/x`)
/// already names its KMS resource type; anything else is a key id.
pub fn kms_key_identifier(region: &str, identifier: &str) -> String {
    if identifier.is_empty() || identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    let resource = if identifier.starts_with("alias/") {
        identifier.to_string()
    } else {
        format!("key/{identifier}")
    };
    ::engine::emulator_arn(KMS_ARN_NAMESPACE, region, &resource)
}

/// A server clock reading as the wire `DateTime` a timestamp member carries.
pub fn to_wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// `at` as the RFC 3339 instant the `time` field of an event envelope carries.
///
/// The envelope's `time` is an instant at WHOLE-SECOND precision closed by `Z`
/// (`2026-01-01T00:00:00Z`): every event AWS delivers carries it in that one form,
/// which is why a consumer reads it with that one format. A server clock reading is
/// finer than that, so the sub-second remainder is DROPPED before the instant is
/// formatted — the field names the second the event was published in, and a
/// fractional part no delivered envelope carries is not written into it.
pub fn to_event_time(at: SystemTime) -> String {
    // Truncating the instant, rather than trimming the text, is what keeps the form
    // the same for every reading: a clock reading whose remainder happens to be zero
    // formats identically to one whose does not.
    ::aws_smithy_types::DateTime::from_secs(to_wire_timestamp(at).secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Encode a list page token for the last key returned.
///
/// The token is opaque to a client but decodable here, so the next page resumes
/// immediately after that key in the ordered collection — not at an index into
/// a list that may have changed between calls.
pub fn encode_page_token(after: &str) -> String {
    ::hex::encode(::serde_json::json!({ "a": after }).to_string())
}

/// What every list operation refuses a `NextToken` it cannot resume from with.
///
/// The service states the parameter that was rejected and nothing else — no
/// reason clause and no echo of the token — so a token this service never
/// minted and a well-formed one whose position is gone are refused in exactly
/// these words. Written once, so the twelve paginated listings cannot word it
/// differently.
pub const INVALID_NEXT_TOKEN: &str = "Parameter NextToken is not valid.";

/// Decode a list page token. `None` when it is not one this service minted,
/// which the caller turns into a refusal rather than an empty page.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    Some(value.get("a")?.as_str()?.to_string())
}

/// The TRACE HEADER an entry publishes, when the text IS one — and `None` when
/// it is not, so nothing is carried to the entry's targets in its place.
///
/// `PutEventsRequestEntry$TraceHeader` is "an X-Ray trace header, which is an
/// http header (`X-Amzn-Trace-Id`) that contains the trace-id associated with the
/// event", so what counts as one is X-Ray's own reading of it, asked here
/// through the ONE shared one (`service_common::xray_trace`). Text that names no
/// trace is not a trace header and the delivery carries none — the entry is
/// published all the same, because the header says nothing about whether the
/// event is publishable.
pub fn published_trace_header(text: &str) -> Option<&str> {
    ::service_common::xray_trace::tracing_header(text)
}

/// The event envelope EventBridge matches an `EventPattern` against.
///
/// An event pattern names the envelope's fields (`source`, `detail-type`,
/// `resources`, `detail`, …), so routing has to build the envelope before it
/// can evaluate a pattern. `detail` is the entry's `Detail` parsed as JSON,
/// because a pattern reaches INTO it. Only the fields the real envelope carries
/// are present, so a pattern that matches here matches in AWS too.
///
/// Building it DESCENDS into `detail` ([`parse_detail`]), which charges the
/// published nesting bound itself, so an unpublishable document leaves `detail`
/// an empty object rather than reaching the parser.
pub fn event_envelope(
    event_id: &str,
    region: &str,
    source: Option<&str>,
    detail_type: Option<&str>,
    detail: Option<&str>,
    resources: &[String],
    time: SystemTime,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "version": EVENT_VERSION,
        "id": event_id,
        "detail-type": detail_type,
        "source": source,
        "account": account_id(),
        "time": to_event_time(time),
        "region": region,
        "resources": resources,
        "detail": detail
            .and_then(parse_detail)
            .unwrap_or_else(|| ::serde_json::json!({})),
    })
}

/// The fields an event a pattern is tested against must carry.
///
/// "The JSON must follow the format specified in Amazon Web Services Events, and
/// the following fields are mandatory: id account source time region resources
/// detail-type" (`TestEventPattern$Event`).
///
/// `resources` is the one member of that list whose ABSENCE the service does not
/// refuse: an event carries no resource ARNs far more often than it carries some,
/// so an envelope that omits the member is read as one listing none (the empty
/// list a published event gets, see [`event_envelope`]) and a `resources` clause
/// in the pattern matches nothing \u2014 which is the same verdict the member spelled
/// out as `[]` gives. Every other member above decides a clause a pattern can
/// name, so an envelope missing one of them could not be matched at all.
pub const REQUIRED_EVENT_FIELDS: [&str; 6] = [
    "id",
    "account",
    "source",
    "time",
    "region",
    "detail-type",
];

/// The whole message a refusal of an unreadable `Event` member states.
///
/// EventBridge names the member and stops there for text it could not read as an
/// event at all — a document that is not an envelope is not diagnosed further.
pub const EVENT_NOT_VALID: &str = "Parameter Event is not valid.";

/// What the refusal DOES state about an envelope whose `time` is not an instant.
///
/// `time` is the one envelope member whose VALUE the service reads rather than
/// merely requires — a pattern's `time` clause is compared as an instant — so a
/// value it cannot read as one is named for what it is.
const EVENT_TIME_NOT_VALID: &str = "Invalid time.";

/// An event envelope a caller supplied, parsed, or `Err(message)` when the text
/// is not an event EventBridge matches a pattern against.
///
/// An envelope is not a pattern, so a caller refuses it as a bad request MEMBER
/// rather than as a bad event pattern, and what comes back is the WHOLE message
/// that refusal carries. Checked here rather than in a handler so every path that
/// takes an envelope FROM A CALLER reads the same rules: the mandatory fields
/// ([`REQUIRED_EVENT_FIELDS`]) have to be present, and `time` has to be an
/// instant that parses — an envelope whose time does not would match a pattern's
/// `time` clause by luck rather than by comparison.
pub fn parse_event_envelope(event: &str) -> Result<::serde_json::Value, String> {
    let parsed: ::serde_json::Value =
        ::serde_json::from_str(event).map_err(|_| EVENT_NOT_VALID.to_string())?;
    let Some(fields) = parsed.as_object() else {
        return Err(EVENT_NOT_VALID.to_string());
    };
    for field in REQUIRED_EVENT_FIELDS {
        if !fields.contains_key(field) {
            return Err(EVENT_NOT_VALID.to_string());
        }
    }
    let parses = fields
        .get("time")
        .and_then(::serde_json::Value::as_str)
        .is_some_and(|text| {
            ::aws_smithy_types::DateTime::from_str(
                text,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok()
        });
    if !parses {
        return Err(parameter_violation("Event", EVENT_TIME_NOT_VALID));
    }
    Ok(parsed)
}

/// EventBridge's INPUT TRANSFORMATION: what a rule's target receives when the
/// target does not take the whole event envelope.
///
/// Two members of `Target` reshape the event before it is handed over, and both
/// are read here so the rule holds once for every target kind:
///
/// * `InputPath` — "the value of the JSONPath that is used for extracting part
///   of the matched event when passing it to the target", so the target receives
///   the JSON text of what that path selects instead of the envelope;
/// * `InputTransformer` — `InputPathsMap`, "a map of JSON paths to be extracted
///   from the event", together with `InputTemplate`, the text in which "you
///   specify placeholders that will be filled with the values of the keys from
///   `InputPathsMap`", each placeholder written `<key>`. The template is
///   delivered as it renders: one that opens a JSON value carries its
///   placeholders "in quotes or out", and any other template is text
///   (`"<instance> is in state <status>"`).
///
/// A path is evaluated through the ONE bounded reader of a caller-supplied JSON
/// path (`service_common::json_reference_path`), in its SELECTION form: a path
/// may name one place (`$.detail.order`, `$.detail.items[0]`) or, through a
/// trailing slice, the list of elements it spans (`$.detail.items[1:3]`). A path
/// that names NO place of the event selects nothing, which an `InputTransformer`
/// placeholder is filled with the EMPTY STRING for — told apart from a place
/// that is there and holds `null`, which renders as `null`.
pub mod input_transform;

/// EventBridge's event-pattern language: validate a pattern, and decide whether
/// an event envelope matches one.
///
/// The language is EventBridge's own — no published grammar outside the service
/// and no crates.io crate implements it — so it is interpreted here over
/// `serde_json`'s parse tree rather than re-parsed by hand. Written once, so
/// `PutRule`'s validation, `PutEvents`' routing and (later) `TestEventPattern`
/// all read the same rules.
///
/// The operators are the ones the EventBridge event-pattern reference defines:
/// exact values, `prefix`, `suffix`, `equals-ignore-case`, `wildcard`,
/// `anything-but`, `numeric`, `cidr`, `exists`, `$or`, and nested objects.
pub mod event_pattern;

/// What an invocation of an API destination CARRIES: the credentials the
/// connection holds and the parameters the connection and the target each
/// contribute to the request.
pub mod invocation;

/// The LOG RECORD a delivery to a CloudWatch Logs log-group target writes: the
/// group the target's ARN names, the stream the write opens, and the
/// `{timestamp, message}` pair the delivered text is written as.
///
/// A log-group target is delivered to by WRITING into the group rather than by
/// invoking an API of it, so what the delivered text becomes is a reading of
/// that text and belongs beside the other readings of a delivery
/// ([`input_transform`]) rather than in the handler that makes the write.
pub mod log_group_target;

/// The SCHEDULE a rule fires on: when a rule carrying a `ScheduleExpression` is
/// due, and what firing one hands its targets.
///
/// "Rules with `ScheduleExpressions` self-trigger based on the given schedule"
/// (`PutRule`), so nothing a client does makes such a rule fire: its own schedule
/// does, which is what that module performs.
pub mod schedule;

/// The customer managed key a TARGET is encrypted with, and whether this service
/// may have a data key generated under it — the second document in force over a
/// delivery to such a target, beside the target's own access policy.
pub mod target_encryption;

/// The CLIENT-CREDENTIALS grant an `OAUTH_CLIENT_CREDENTIALS` connection
/// authorizes with: the token request EventBridge makes at the connection's
/// authorization endpoint, and the access token every invocation through the
/// connection then carries.
pub mod oauth;

/// Whether EventBridge may ASSUME the role a rule's target names — the step
/// every target invocation begins with, and the one that decides a handover
/// whose own API call never gets made.
pub mod role_assumption;

/// The submitted VALUES this service refuses: the reserved namespaces, the
/// service a referenced resource has to belong to, and the members another
/// member makes required — each stated once, for every operation that writes it.
pub mod request_validation;

/// Whether the caller may HAND this service the IAM role a request names — the
/// `iam:PassRole` pass every operation that takes a role ARN makes.
pub mod pass_role;

/// The customer managed key an EVENT BUS is configured with, and whether that
/// key's own policy lets this service use it — asked by every writer of a bus's
/// `KmsKeyIdentifier`.
pub mod bus_encryption;

mod replays;
pub use replays::*;
