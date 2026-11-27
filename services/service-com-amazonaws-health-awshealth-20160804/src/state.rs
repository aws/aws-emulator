// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-health-awshealth-20160804 service.
//!
//! AWS Health is a READ-ONLY API: it has no `Create*`/`Put*` operation, so the
//! resources every `Describe*` reports on are the events AWS itself publishes. The
//! emulator therefore installs a small, deterministic event catalog when this state
//! is first constructed ([`Default`]), and every operation computes its answer by
//! filtering/aggregating THAT catalog. No handler holds a canned response.
//!
//! This module owns the three things more than one handler needs:
//!
//! * the stored records ([`HealthEvent`], [`HealthEntity`]) and their projections
//!   onto the modelled wire shapes, so the `Event` `DescribeEvents` returns and the
//!   one `DescribeEventDetails` returns are built by the same function from the same
//!   record;
//! * filter evaluation ([`ComAmazonawsHealthAwshealth20160804State::matching_events`],
//!   [`ComAmazonawsHealthAwshealth20160804State::matching_entities`]); and
//! * pagination ([`paginate`]) with server-issued, verifiable opaque tokens.
//!
//! It also owns the one ACCOUNT-level fact the whole API is gated on: the Amazon Web
//! Services Support plan the account holds
//! ([`ComAmazonawsHealthAwshealth20160804State::support_plan`]), which
//! [`ComAmazonawsHealthAwshealth20160804State::health_api_allowed`] answers against
//! the plans this service's documentation names ([`QUALIFYING_SUPPORT_PLANS`]) and
//! `crate::service` reads ahead of every operation.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AffectedEntity, DateTimeRange, EntityFilter, Event, EventFilter, EventType, EventTypeFilter,
};

/// The only locale AWS Health serves. The modelled `locale` doc states "English
/// (en) is the default and the only supported value at this time", so any other
/// value is answered with `UnsupportedLocale`.
pub const SUPPORTED_LOCALE: &str = "en";

/// The Amazon Web Services Support plans an account may call the Health API with.
///
/// Source: this service's own documentation
/// (`smithy_query(query="doc",shape="AWSHealth_20160804")`): "You must have a
/// Business, Enterprise On-Ramp, or Enterprise Support plan from Amazon Web Services
/// Support to use the Health API. If you call the Health API from an Amazon Web
/// Services account that doesn't have a Business, Enterprise On-Ramp, or Enterprise
/// Support plan, you receive a `SubscriptionRequiredException` error." Every plan the
/// sentence does not name — Basic and Developer — does NOT qualify.
///
/// The last two entries are the CURRENT names of those same premium tiers, which the
/// Support API's documentation spells out while the Health documentation above still
/// names only the legacy ones: "You must have an Amazon Web Services Business
/// Support+, Amazon Web Services Enterprise Support, or Amazon Web Services Unified
/// Operations plan … or, if you haven't transitioned to one of these plans, … a
/// Business, Enterprise On-Ramp, or Enterprise Support plan." An account is on one
/// plan, so a tier that entitles it to the Support API must not be refused here under
/// its new name alone (see the peer gate's `state::QUALIFYING_SUPPORT_PLANS` in
/// `service-com-amazonaws-support-awssupport-20130415`).
///
/// Entries are in the form [`normalize_support_plan`] produces, so a caller compares
/// through [`plan_qualifies`] rather than against this list directly.
pub const QUALIFYING_SUPPORT_PLANS: &[&str] = &[
    "business",
    "businesssupport",
    "enterprise",
    "enterpriseonramp",
    "enterpriseonrampsupport",
    "enterprisesupport",
    "businesssupport+",
    "unifiedoperations",
];

/// The environment variable an operator uses to tell this emulated account which
/// Amazon Web Services Support plan it holds. The Health API models no operation that
/// changes one (`smithy_query(query="digest")` lists none — a plan is bought outside
/// this API), so the only way one reaches the emulator is out of band, and it is read
/// once when this service's store is created (`SUPPORT_PLAN="enterprise"`).
///
/// The name is the one the Support API's own gate reads, so ONE operator input decides
/// the account's plan for every API that is gated on it rather than each service
/// inventing its own switch.
///
/// UNSET means the account holds no Support plan, which is what a new Amazon Web
/// Services account has — and then every operation of this API answers the documented
/// `SubscriptionRequiredException` refusal rather than running.
pub const SUPPORT_PLAN_ENV: &str = "SUPPORT_PLAN";

/// A plan name reduced to what identifies the plan: ASCII lowercase, with every
/// separator and the vendor prefix the documentation writes dropped. So
/// `"Enterprise On-Ramp"`, `"enterprise_on_ramp"` and `"AWS Enterprise On-Ramp"` are
/// one plan, while `"Business Support+"` keeps the `+` that names it.
pub fn normalize_support_plan(plan: &str) -> String {
    let squeezed: String = plan
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let identifying = squeezed
        .strip_prefix("amazonwebservices")
        .or_else(|| squeezed.strip_prefix("aws"))
        .unwrap_or(squeezed.as_str());
    identifying.to_string()
}

/// Whether `plan` names one of the [`QUALIFYING_SUPPORT_PLANS`], i.e. whether an
/// account holding it may call this API at all.
pub fn plan_qualifies(plan: &str) -> bool {
    QUALIFYING_SUPPORT_PLANS.contains(&normalize_support_plan(plan).as_str())
}

/// Whether `locale` is one this service can answer in.
///
/// Absent means "the default", which is the supported locale.
pub fn locale_supported(locale: Option<&str>) -> bool {
    locale.is_none_or(|l| l.eq_ignore_ascii_case(SUPPORTED_LOCALE))
}

// ---------------------------------------------------------------------------
// Tag tracking
// ---------------------------------------------------------------------------

/// The modelled `TagKeyStatus` of a configuration that has finished propagating, and
/// of one that has not (`ops::types::TAG_KEY_STATUS_VALUES`).
pub const TAG_KEY_STATUS_ACTIVE: &str = "ACTIVE";
pub const TAG_KEY_STATUS_PENDING: &str = "PENDING";

/// The one `AccountTagType` the model declares, carried by every tag `DescribeTags`
/// reports (`ops::types::ACCOUNT_TAG_TYPE_VALUES`).
pub const ACCOUNT_RESOURCE_TAG: &str = "ACCOUNT_RESOURCE_TAG";

/// The two `OrganizationTagType` values the model declares
/// (`ops::types::ORGANIZATION_TAG_TYPE_VALUES`): a tag attached to an affected
/// resource, and one attached to the account that owns it.
pub const ORGANIZATION_RESOURCE_TAG: &str = "ORGANIZATION_RESOURCE_TAG";
pub const ORGANIZATION_ACCOUNT_TAG: &str = "ORGANIZATION_ACCOUNT_TAG";

/// How long a tag-key configuration takes to finish propagating, after which the key
/// reports [`TAG_KEY_STATUS_ACTIVE`] instead of [`TAG_KEY_STATUS_PENDING`].
///
/// The model declares `PENDING` as a `TagKeyStatus`, so a freshly configured key is
/// not immediately active; neither the model nor the documentation fixes how long that
/// takes, so this span is the emulator's choice, short enough for a caller to wait out
/// and long enough that a caller acting straight after `ConfigureTags` still sees the
/// transient status.
const TAG_KEY_ACTIVATION_SPAN: Duration = Duration::from_secs(3);

/// One tag key configured for Health event tracking.
///
/// `ConfigureTags` owns the account's list of these and `ConfigureTagsForOrganization`
/// the organization's. The keys in a list are the only ones the tags Health reports are
/// drawn from, which is what both operations' documentation states: "Only configured
/// tag keys will appear in Health event data."
#[derive(Debug, Clone)]
pub struct ConfiguredTagKey {
    pub key: String,
    pub enabled_for_event_bridge: bool,
    /// The instant this key's configuration finishes propagating. STORED, never slept
    /// on: [`Self::status`] derives the modelled status from it, so the key moves from
    /// `PENDING` to `ACTIVE` when the clock says so and no read has to write for it.
    active_at: Instant,
}

impl ConfiguredTagKey {
    /// A key configured now, so its propagation is due one
    /// [`TAG_KEY_ACTIVATION_SPAN`] from this call.
    fn new(key: String, enabled_for_event_bridge: bool) -> Self {
        Self {
            key,
            enabled_for_event_bridge,
            active_at: Instant::now() + TAG_KEY_ACTIVATION_SPAN,
        }
    }

    /// This key's modelled `TagKeyStatus`.
    pub fn status(&self) -> &'static str {
        if Instant::now() >= self.active_at {
            TAG_KEY_STATUS_ACTIVE
        } else {
            TAG_KEY_STATUS_PENDING
        }
    }
}

/// Replace `current` with exactly the keys `requested` names, in the order requested.
///
/// The rule both `Configure*` operations state — "This operation replaces the existing
/// configuration with the specified tag keys" — so a key the request omits stops being
/// tracked. A key that was already configured keeps the propagation instant it already
/// had, which is what makes the `@idempotent` call idempotent: re-sending an unchanged
/// configuration neither restarts its clock nor moves an `ACTIVE` key back to
/// `PENDING`. A configuration is a SET of keys, so a key named twice is one entry and
/// the last `enabledForEventBridge` it was given wins.
pub fn replace_tag_keys<'a>(
    current: &mut Vec<ConfiguredTagKey>,
    requested: impl IntoIterator<Item = (&'a str, bool)>,
) {
    let mut replaced: Vec<ConfiguredTagKey> = Vec::new();
    for (key, enabled_for_event_bridge) in requested {
        if let Some(already) = replaced.iter_mut().find(|entry| entry.key == key) {
            already.enabled_for_event_bridge = enabled_for_event_bridge;
            continue;
        }
        let mut entry = current
            .iter()
            .find(|entry| entry.key == key)
            .cloned()
            .unwrap_or_else(|| ConfiguredTagKey::new(key.to_string(), enabled_for_event_bridge));
        entry.enabled_for_event_bridge = enabled_for_event_bridge;
        replaced.push(entry);
    }
    *current = replaced;
}

/// The key names a configuration tracks.
pub fn tracked_keys(configured: &[ConfiguredTagKey]) -> BTreeSet<&str> {
    configured.iter().map(|entry| entry.key.as_str()).collect()
}

/// The pairs of `tags` whose key is one `tracked` names, deduplicated and ordered by
/// key then value, so a paginated walk over them never repeats or skips a row.
fn tracked_pairs<'a>(
    tags: impl IntoIterator<Item = (&'a String, &'a String)>,
    tracked: &BTreeSet<&str>,
) -> Vec<(String, String)> {
    let mut pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (key, value) in tags {
        if tracked.contains(key.as_str()) {
            pairs.insert((key.as_str(), value.as_str()));
        }
    }
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// One stored Health event — the record every `Describe*` reports on.
#[derive(Debug, Clone)]
pub struct HealthEvent {
    pub arn: String,
    pub service: String,
    pub event_type_code: String,
    pub event_type_category: String,
    pub region: String,
    pub availability_zone: Option<String>,
    /// Epoch seconds; rendered as the modelled `timestamp` on the wire.
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub last_updated_time: i64,
    /// One of `eventStatusCode`: `open` | `closed` | `upcoming`.
    pub status_code: String,
    /// One of `eventScopeCode`: `PUBLIC` | `ACCOUNT_SPECIFIC` | `NONE`.
    pub event_scope_code: String,
    /// One of `EventActionability`.
    pub actionability: String,
    /// A subset of `EventPersona`.
    pub personas: Vec<String>,
    /// `EventDescription.latestDescription`, returned only by `DescribeEventDetails`.
    pub description: String,
    /// `EventDetails.eventMetadata`, returned only by `DescribeEventDetails`.
    pub event_metadata: BTreeMap<String, String>,
    /// The accounts an `ACCOUNT_SPECIFIC` event affects; empty for a `PUBLIC` one.
    ///
    /// Stored because `eventScopeCode` only means anything alongside it (the
    /// `eventScopeCode` doc: "If the value is `ACCOUNT_SPECIFIC`, then the
    /// `affectedAccounts` value lists the affected accounts in your organization").
    /// `DescribeAffectedAccountsForOrganization`, the operation that reports it, is
    /// still a scaffold 501 and has its own gap issue; none of the six operations
    /// implemented here returns it.
    pub affected_accounts: Vec<String>,
}

impl HealthEvent {
    /// This record projected onto the modelled `Event` summary shape.
    ///
    /// The single projection used by BOTH `DescribeEvents` and
    /// `DescribeEventDetails`, so the summary a client lists and the one it then
    /// fetches by ARN are necessarily identical.
    pub fn to_event(&self) -> Event {
        Event {
            arn: Some(self.arn.clone()),
            service: Some(self.service.clone()),
            event_type_code: Some(self.event_type_code.clone()),
            event_type_category: Some(self.event_type_category.clone()),
            region: Some(self.region.clone()),
            availability_zone: self.availability_zone.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from_secs(self.start_time)),
            end_time: self
                .end_time
                .map(::aws_smithy_types::DateTime::from_secs),
            last_updated_time: Some(::aws_smithy_types::DateTime::from_secs(
                self.last_updated_time,
            )),
            status_code: Some(self.status_code.clone()),
            event_scope_code: Some(self.event_scope_code.clone()),
            actionability: Some(self.actionability.clone()),
            personas: Some(self.personas.clone()),
        }
    }

    /// The `EventType` triple (plus its classification) this event belongs to.
    pub fn to_event_type(&self) -> EventType {
        EventType {
            service: Some(self.service.clone()),
            code: Some(self.event_type_code.clone()),
            category: Some(self.event_type_category.clone()),
            actionability: Some(self.actionability.clone()),
            personas: Some(self.personas.clone()),
        }
    }
}

/// One stored entity affected by a stored event.
#[derive(Debug, Clone)]
pub struct HealthEntity {
    pub entity_arn: String,
    pub event_arn: String,
    pub entity_value: String,
    pub entity_url: Option<String>,
    pub aws_account_id: String,
    pub last_updated_time: i64,
    /// One of `entityStatusCode`: `IMPAIRED` | `UNIMPAIRED` | `UNKNOWN` | `PENDING` | `RESOLVED`.
    pub status_code: String,
    /// The resource tags attached to this entity — every one of them, whether or not
    /// the account currently tracks its key. Which of them a caller SEES is decided
    /// where the entity is projected ([`Self::to_affected_entity`]), so configuring a
    /// tag key reveals the pairs that were always attached rather than inventing them.
    pub resource_tags: BTreeMap<String, String>,
    pub entity_metadata: BTreeMap<String, String>,
}

impl HealthEntity {
    /// This record projected onto the modelled `AffectedEntity` shape, reporting only
    /// the resource tags whose key is one of the `tracked` ones.
    pub fn to_affected_entity(&self, tracked: &BTreeSet<&str>) -> AffectedEntity {
        let resource_tags: HashMap<String, String> = self
            .resource_tags
            .iter()
            .filter(|(key, _)| tracked.contains(key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        AffectedEntity {
            entity_arn: Some(self.entity_arn.clone()),
            event_arn: Some(self.event_arn.clone()),
            entity_value: Some(self.entity_value.clone()),
            entity_url: self.entity_url.clone(),
            aws_account_id: Some(self.aws_account_id.clone()),
            last_updated_time: Some(::aws_smithy_types::DateTime::from_secs(
                self.last_updated_time,
            )),
            status_code: Some(self.status_code.clone()),
            // Absent rather than empty when the account tracks none of this entity's
            // tag keys: there is no pair to report.
            resource_tags: (!resource_tags.is_empty()).then_some(resource_tags),
            // The `accountTags` doc: "This field is returned only by
            // DescribeAffectedEntitiesForOrganization and is not populated by
            // DescribeAffectedEntities." That organization reader is still a scaffold
            // with its own issue, and this projection serves the account reader, so
            // the member stays absent here.
            account_tags: None,
            entity_metadata: Some(
                self.entity_metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        }
    }
}

/// Per-service state for com-amazonaws-health-awshealth-20160804. A handler reaches it through [`service_state`].
///
/// `events` is keyed by event ARN and `entities` by the ARN of the event they
/// belong to, so every operation reads ONE copy of each record.
#[derive(Debug)]
pub struct ComAmazonawsHealthAwshealth20160804State {
    pub events: BTreeMap<String, HealthEvent>,
    pub entities: BTreeMap<String, Vec<HealthEntity>>,
    /// The Amazon Web Services Support plan this account holds, as
    /// [`SUPPORT_PLAN_ENV`] named it at start-up — `None` when nothing named one,
    /// which is an account with no Support plan.
    ///
    /// It is a field rather than a constant because the subscription is a property of
    /// the ACCOUNT and nothing in THIS API changes it. One field serves every caller
    /// because this emulator serves ONE account — the canonical emulator account every
    /// stored record is built from. [`Self::health_api_allowed`] is what the dispatch
    /// path reads.
    pub support_plan: Option<String>,
    /// The resource tag keys this account has configured for Health event tracking,
    /// as `ConfigureTags` last left them. EMPTY by default, which is what a new
    /// account has: nothing has configured a key yet, so no tag appears in the event
    /// data this API reports.
    pub resource_tag_keys: Vec<ConfiguredTagKey>,
    /// The same configuration at organization scope, as
    /// `ConfigureTagsForOrganization` last left it. A separate list because the two
    /// operations configure two scopes: one account's tracking and the
    /// organization's.
    pub organization_tag_keys: Vec<ConfiguredTagKey>,
    /// The tags attached to the ACCOUNT the affected resources belong to — the
    /// `ORGANIZATION_ACCOUNT_TAG` half of what `DescribeTagsForOrganization` reports.
    ///
    /// Seeded alongside the event catalog and for the same reason: this API reports an
    /// account's tags, it models no operation that sets one (a tag on an account is
    /// written where accounts are managed), so the only faithful stored value is a
    /// deterministic one.
    pub account_tags: BTreeMap<String, String>,
}

impl Default for ComAmazonawsHealthAwshealth20160804State {
    fn default() -> Self {
        let mut state = Self {
            events: BTreeMap::new(),
            entities: BTreeMap::new(),
            support_plan: ::std::env::var(SUPPORT_PLAN_ENV).ok(),
            resource_tag_keys: Vec::new(),
            organization_tag_keys: Vec::new(),
            account_tags: BTreeMap::new(),
        };
        state.seed();
        state
    }
}

/// One row of the seeded entity table: (index of the event in the catalog above, the
/// entity's value, its `entityStatusCode`, its tags).
type SeedEntityRow<'a> = (usize, &'a str, &'a str, &'a [(&'a str, &'a str)]);

/// The instant the seeded catalog is anchored on: 2026-01-01T00:00:00Z in epoch
/// seconds. A fixed anchor keeps the catalog identical on every run, so the
/// timestamps a client reads back are stable across calls and processes.
const SEED_EPOCH: i64 = 1_767_225_600;

impl ComAmazonawsHealthAwshealth20160804State {
    /// Whether this account may call the Health API at all: whether the plan it holds
    /// is one of the [`QUALIFYING_SUPPORT_PLANS`]. False for an account with no plan
    /// recorded, and false for one holding a plan that does not qualify (Basic,
    /// Developer) — the account-level gate that fronts every operation of this
    /// service, which `crate::service` reads before any handler runs.
    pub fn health_api_allowed(&self) -> bool {
        self.support_plan.as_deref().is_some_and(plan_qualifies)
    }

    /// Install the deterministic event catalog.
    ///
    /// Health publishes events; a client cannot create one. Four events across two
    /// services, two regions and three `eventTypeCategory` values, with affected
    /// entities on the account-specific ones, give every `Describe*` operation
    /// something real to filter and aggregate.
    fn seed(&mut self) {
        let catalog = [
            HealthEvent {
                arn: event_arn(
                    "us-east-1",
                    "EC2",
                    "AWS_EC2_OPERATIONAL_ISSUE",
                    "AWS_EC2_OPERATIONAL_ISSUE_7A5C3B21",
                ),
                service: "EC2".to_string(),
                event_type_code: "AWS_EC2_OPERATIONAL_ISSUE".to_string(),
                event_type_category: "issue".to_string(),
                region: "us-east-1".to_string(),
                availability_zone: Some("us-east-1a".to_string()),
                start_time: SEED_EPOCH,
                end_time: Some(SEED_EPOCH + 7_200),
                last_updated_time: SEED_EPOCH + 7_200,
                status_code: "closed".to_string(),
                event_scope_code: "PUBLIC".to_string(),
                actionability: "INFORMATIONAL".to_string(),
                personas: vec!["OPERATIONS".to_string()],
                description: "Between 00:00 and 02:00 UTC we experienced increased \
                               API error rates for Amazon EC2 in the US-EAST-1 Region. \
                               The issue has been resolved."
                    .to_string(),
                event_metadata: BTreeMap::new(),
                affected_accounts: Vec::new(),
            },
            HealthEvent {
                arn: event_arn(
                    "us-east-1",
                    "EC2",
                    "AWS_EC2_INSTANCE_SCHEDULED_MAINTENANCE",
                    "AWS_EC2_INSTANCE_SCHEDULED_MAINTENANCE_3F0D91AC",
                ),
                service: "EC2".to_string(),
                event_type_code: "AWS_EC2_INSTANCE_SCHEDULED_MAINTENANCE".to_string(),
                event_type_category: "scheduledChange".to_string(),
                region: "us-east-1".to_string(),
                availability_zone: None,
                start_time: SEED_EPOCH + 86_400,
                end_time: Some(SEED_EPOCH + 93_600),
                last_updated_time: SEED_EPOCH + 10_800,
                status_code: "upcoming".to_string(),
                event_scope_code: "ACCOUNT_SPECIFIC".to_string(),
                actionability: "ACTION_REQUIRED".to_string(),
                personas: vec!["OPERATIONS".to_string()],
                description: "One or more of your Amazon EC2 instances is scheduled \
                               for maintenance. Reboot the listed instances before the \
                               scheduled window to control when they are restarted."
                    .to_string(),
                event_metadata: BTreeMap::from([(
                    "maintenanceWindow".to_string(),
                    "2026-01-02 00:00:00 UTC - 2026-01-02 02:00:00 UTC".to_string(),
                )]),
                affected_accounts: vec![::engine::emulator_account_id().to_string()],
            },
            HealthEvent {
                arn: event_arn(
                    "us-west-2",
                    "RDS",
                    "AWS_RDS_OPERATIONAL_ISSUE",
                    "AWS_RDS_OPERATIONAL_ISSUE_5C8E47FD",
                ),
                service: "RDS".to_string(),
                event_type_code: "AWS_RDS_OPERATIONAL_ISSUE".to_string(),
                event_type_category: "issue".to_string(),
                region: "us-west-2".to_string(),
                availability_zone: None,
                start_time: SEED_EPOCH + 3_600,
                end_time: None,
                last_updated_time: SEED_EPOCH + 14_400,
                status_code: "open".to_string(),
                event_scope_code: "ACCOUNT_SPECIFIC".to_string(),
                actionability: "ACTION_MAY_BE_REQUIRED".to_string(),
                personas: vec!["OPERATIONS".to_string()],
                description: "We are investigating elevated connection failures for \
                               some Amazon RDS instances in the US-WEST-2 Region."
                    .to_string(),
                event_metadata: BTreeMap::new(),
                affected_accounts: vec![::engine::emulator_account_id().to_string()],
            },
            HealthEvent {
                arn: event_arn(
                    "us-east-1",
                    "BILLING",
                    "AWS_BILLING_NOTICE",
                    "AWS_BILLING_NOTICE_1D4F6B90",
                ),
                service: "BILLING".to_string(),
                event_type_code: "AWS_BILLING_NOTICE".to_string(),
                event_type_category: "accountNotification".to_string(),
                region: "us-east-1".to_string(),
                availability_zone: None,
                start_time: SEED_EPOCH - 86_400,
                end_time: None,
                last_updated_time: SEED_EPOCH + 1_800,
                status_code: "open".to_string(),
                event_scope_code: "ACCOUNT_SPECIFIC".to_string(),
                actionability: "INFORMATIONAL".to_string(),
                personas: vec!["BILLING".to_string()],
                description: "Your invoice for the previous month is now available in \
                               the Billing and Cost Management console."
                    .to_string(),
                event_metadata: BTreeMap::new(),
                affected_accounts: vec![::engine::emulator_account_id().to_string()],
            },
        ];

        // The affected entities of each seeded event, as (event index, entity value,
        // entityStatusCode, resource tags). A PUBLIC event whose blast radius is wider
        // than any one resource still carries one entity saying so, which is what the
        // DescribeAffectedEntities doc describes. The tag rows span two keys with
        // overlapping and distinct values, and entities carrying one, both or neither,
        // so the tag filter and the tag readers have something real to narrow.
        let entity_rows: &[SeedEntityRow] = &[
            (0, "us-east-1", "UNKNOWN", &[]),
            (
                1,
                "i-0a1b2c3d4e5f60718",
                "IMPAIRED",
                &[("Name", "web-1"), ("Environment", "production")],
            ),
            (
                1,
                "i-0a1b2c3d4e5f60719",
                "PENDING",
                &[("Name", "web-2"), ("Environment", "staging")],
            ),
            (1, "i-0a1b2c3d4e5f6071a", "RESOLVED", &[("Name", "web-3")]),
            (
                2,
                "database-primary",
                "IMPAIRED",
                &[("Environment", "production")],
            ),
            (2, "database-replica", "UNIMPAIRED", &[]),
            (
                3,
                ::engine::emulator_account_id(),
                "RESOLVED",
                &[] as &[(&str, &str)],
            ),
        ];

        for (event_index, entity_value, status_code, tags) in entity_rows {
            let event = &catalog[*event_index];
            let entity_id = entity_id_for(&event.arn, entity_value);
            self.entities
                .entry(event.arn.clone())
                .or_default()
                .push(HealthEntity {
                    entity_arn: ::engine::emulator_arn(
                        "health",
                        &event.region,
                        &format!("entity/{entity_id}"),
                    ),
                    event_arn: event.arn.clone(),
                    entity_value: (*entity_value).to_string(),
                    entity_url: None,
                    aws_account_id: ::engine::emulator_account_id().to_string(),
                    last_updated_time: event.last_updated_time,
                    status_code: (*status_code).to_string(),
                    resource_tags: tags
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                    entity_metadata: BTreeMap::new(),
                });
        }

        // The tags of the account those entities belong to. Keys distinct from the
        // resource ones above, so the `OrganizationTagType` a row carries is decided
        // by where the tag is attached and not by its key.
        self.account_tags = BTreeMap::from([
            ("CostCenter".to_string(), "cc-1042".to_string()),
            ("ManagedBy".to_string(), "platform-team".to_string()),
        ]);

        for event in catalog {
            self.events.insert(event.arn.clone(), event);
        }
    }

    /// Every stored event matching `filter`, newest first.
    ///
    /// The order is the one the `DescribeEvents` doc states — "Results are sorted by
    /// lastModifiedTime, starting with the most recent event" — with the event ARN as
    /// a tie-break so the sequence is total and a page boundary is stable.
    pub fn matching_events(&self, filter: Option<&EventFilter>) -> Vec<&HealthEvent> {
        let mut matched: Vec<&HealthEvent> = self
            .events
            .values()
            .filter(|event| self.event_matches(event, filter))
            .collect();
        matched.sort_by(|a, b| {
            b.last_updated_time
                .cmp(&a.last_updated_time)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        matched
    }

    /// Whether one stored event satisfies every clause `filter` sets.
    ///
    /// An absent filter, an absent member and an empty list all mean "no constraint";
    /// a populated list is satisfied when the event's value is one of its entries.
    fn event_matches(&self, event: &HealthEvent, filter: Option<&EventFilter>) -> bool {
        let Some(filter) = filter else { return true };

        if !matches_one_of(&filter.event_arns, &event.arn)
            || !matches_one_of(&filter.event_type_codes, &event.event_type_code)
            || !matches_one_of(&filter.services, &event.service)
            || !matches_one_of(&filter.regions, &event.region)
            || !matches_one_of(&filter.event_type_categories, &event.event_type_category)
            || !matches_one_of(&filter.event_status_codes, &event.status_code)
            || !matches_one_of(&filter.actionabilities, &event.actionability)
        {
            return false;
        }

        // An absent optional member cannot satisfy a filter that constrains it.
        if !is_unconstrained(&filter.availability_zones) {
            match &event.availability_zone {
                Some(az) if matches_one_of(&filter.availability_zones, az) => {}
                _ => return false,
            }
        }

        if !is_unconstrained(&filter.personas)
            && !event
                .personas
                .iter()
                .any(|persona| matches_one_of(&filter.personas, persona))
        {
            return false;
        }

        if !within_any_range(&filter.start_times, Some(event.start_time))
            || !within_any_range(&filter.end_times, event.end_time)
            || !within_any_range(&filter.last_updated_times, Some(event.last_updated_time))
        {
            return false;
        }

        // The entity-shaped clauses of EventFilter select an event by the entities it
        // affects, so they are answered from the same stored entity list
        // DescribeAffectedEntities reads.
        let entities = self.entities_for(&event.arn);
        if !is_unconstrained(&filter.entity_arns)
            && !entities
                .iter()
                .any(|entity| matches_one_of(&filter.entity_arns, &entity.entity_arn))
        {
            return false;
        }
        if !is_unconstrained(&filter.entity_values)
            && !entities
                .iter()
                .any(|entity| matches_one_of(&filter.entity_values, &entity.entity_value))
        {
            return false;
        }
        if !tag_filter_unconstrained(&filter.tags)
            && !entities.iter().any(|entity| {
                entity_matches_tag_filter(entity, filter.tags.as_ref(), &self.tracked_resource_tag_keys())
            })
        {
            return false;
        }

        true
    }

    /// The stored entities of one event, in catalog order.
    pub fn entities_for(&self, event_arn: &str) -> &[HealthEntity] {
        self.entities
            .get(event_arn)
            .map_or(&[][..], |entities| entities.as_slice())
    }

    /// Every stored entity of the events `filter` names, narrowed by the filter's
    /// remaining clauses, in a TOTAL order that does not depend on the order the
    /// request listed those events.
    ///
    /// Request-order independence is what makes the page token sound: the token's
    /// request digest hashes list members order-independently (so two spellings of one
    /// filter continue the same walk), so the result this offset indexes into must be
    /// ordered the same way for both spellings, or a continued walk could repeat or
    /// omit a row.
    pub fn matching_entities(&self, filter: &EntityFilter) -> Vec<&HealthEntity> {
        let mut matched: Vec<&HealthEntity> = Vec::new();
        let tracked = self.tracked_resource_tag_keys();
        let mut visited: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for event_arn in &filter.event_arns {
            // Each named event contributes its entities once, so repeating an ARN in
            // the request does not duplicate rows.
            if !visited.insert(event_arn.as_str()) {
                continue;
            }
            for entity in self.entities_for(event_arn) {
                if matches_one_of(&filter.entity_arns, &entity.entity_arn)
                    && matches_one_of(&filter.entity_values, &entity.entity_value)
                    && matches_one_of(&filter.status_codes, &entity.status_code)
                    && within_any_range(
                        &filter.last_updated_times,
                        Some(entity.last_updated_time),
                    )
                    && entity_matches_tag_filter(entity, filter.tags.as_ref(), &tracked)
                {
                    matched.push(entity);
                }
            }
        }
        matched.sort_by(|a, b| {
            a.event_arn
                .cmp(&b.event_arn)
                .then_with(|| a.entity_arn.cmp(&b.entity_arn))
        });
        matched
    }

    /// The distinct event types present in the catalog, narrowed by `filter`.
    ///
    /// Derived from the stored events, never from a second list, so an event type is
    /// reported exactly when some event of that type exists.
    pub fn matching_event_types(&self, filter: Option<&EventTypeFilter>) -> Vec<EventType> {
        let mut keys: Vec<(String, String, String)> = Vec::new();
        let mut types: Vec<EventType> = Vec::new();
        for event in self.events.values() {
            if let Some(filter) = filter {
                if !matches_one_of(&filter.services, &event.service)
                    || !matches_one_of(&filter.event_type_codes, &event.event_type_code)
                    || !matches_one_of(&filter.event_type_categories, &event.event_type_category)
                    || !matches_one_of(&filter.actionabilities, &event.actionability)
                {
                    continue;
                }
                if !is_unconstrained(&filter.personas)
                    && !event
                        .personas
                        .iter()
                        .any(|persona| matches_one_of(&filter.personas, persona))
                {
                    continue;
                }
            }
            let key = (
                event.service.clone(),
                event.event_type_code.clone(),
                event.event_type_category.clone(),
            );
            if keys.contains(&key) {
                continue;
            }
            keys.push(key);
            types.push(event.to_event_type());
        }
        // A total order on the (service, code, category) triple, so a paginated walk
        // of the catalog's event types never repeats or skips one.
        types.sort_by(|a, b| {
            a.service
                .cmp(&b.service)
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.category.cmp(&b.category))
        });
        types
    }

    /// The resource tag keys this account tracks, as `ConfigureTags` left them.
    pub fn tracked_resource_tag_keys(&self) -> BTreeSet<&str> {
        tracked_keys(&self.resource_tag_keys)
    }

    /// The tag keys the organization tracks, as `ConfigureTagsForOrganization` left
    /// them.
    pub fn tracked_organization_tag_keys(&self) -> BTreeSet<&str> {
        tracked_keys(&self.organization_tag_keys)
    }

    /// The distinct tracked resource tag pairs attached to the stored entities — "the
    /// actual tag key-value pairs attached to affected entities", as the
    /// `DescribeTags` doc puts it — ordered by key then value.
    ///
    /// Derived from the SAME stored entities `DescribeAffectedEntities` reports, so a
    /// pair is listed exactly when some affected entity carries it and the account
    /// tracks its key. One pair appears once however many entities carry it: it is one
    /// tag of this account.
    pub fn resource_tag_pairs(&self, tracked: &BTreeSet<&str>) -> Vec<(String, String)> {
        tracked_pairs(
            self.entities
                .values()
                .flatten()
                .flat_map(|entity| entity.resource_tags.iter()),
            tracked,
        )
    }

    /// The tracked tag pairs attached to the account itself, ordered by key then
    /// value.
    pub fn account_tag_pairs(&self, tracked: &BTreeSet<&str>) -> Vec<(String, String)> {
        tracked_pairs(self.account_tags.iter(), tracked)
    }
}

/// Whether `entity` satisfies a `tags` filter member.
///
/// The rule every `tags` filter member of this service documents: the member is a map
/// of tag key to the values that key accepts, "a tag matches when its key is in the map
/// and its value is one of the values listed for that key", the comparison is case
/// sensitive, and "the filter matches when at least one key in the map matches". An
/// absent or empty map constrains nothing. Only a TRACKED key can match: an untracked
/// key is not part of the event data the filter runs over, so it is not a tag the
/// entity has as far as this API is concerned.
pub fn entity_matches_tag_filter(
    entity: &HealthEntity,
    filter: Option<&HashMap<String, Vec<String>>>,
    tracked: &BTreeSet<&str>,
) -> bool {
    let Some(filter) = filter else { return true };
    if filter.is_empty() {
        return true;
    }
    filter.iter().any(|(key, accepted)| {
        tracked.contains(key.as_str())
            && entity
                .resource_tags
                .get(key)
                .is_some_and(|stored| accepted.iter().any(|value| value == stored))
    })
}

/// Whether a `tags` filter member constrains nothing: absent, or present but empty.
pub fn tag_filter_unconstrained(filter: &Option<HashMap<String, Vec<String>>>) -> bool {
    filter.as_ref().is_none_or(HashMap::is_empty)
}

/// Whether a filter member constrains nothing: absent, or present but empty.
fn is_unconstrained<T>(list: &Option<Vec<T>>) -> bool {
    list.as_ref().is_none_or(|values| values.is_empty())
}

/// Whether `value` satisfies a filter member: trivially when the member
/// constrains nothing, else by being one of its entries.
pub fn matches_one_of(list: &Option<Vec<String>>, value: &str) -> bool {
    match list {
        None => true,
        Some(values) if values.is_empty() => true,
        Some(values) => values.iter().any(|candidate| candidate == value),
    }
}

/// Whether `instant` falls inside at least one of the requested ranges. An absent
/// instant cannot be inside any, so an event missing the timestamp a range
/// constrains does not match.
fn within_any_range(ranges: &Option<Vec<DateTimeRange>>, instant: Option<i64>) -> bool {
    if is_unconstrained(ranges) {
        return true;
    }
    let Some(instant) = instant else { return false };
    ranges.as_ref().is_some_and(|ranges| {
        ranges.iter().any(|range| {
            range.from.as_ref().is_none_or(|from| instant >= from.secs())
                && range.to.as_ref().is_none_or(|to| instant <= to.secs())
        })
    })
}

/// A Health event ARN in the format the `eventArn` member documents:
/// `arn:aws:health:<event-region>::event/<SERVICE>/<EVENT_TYPE_CODE>/<EVENT_TYPE_PLUS_ID>`.
///
/// Health event ARNs carry NO account segment (the event belongs to AWS, not to an
/// account), which is why this is not built through `engine::emulator_arn`.
fn event_arn(region: &str, service: &str, event_type_code: &str, event_id: &str) -> String {
    format!("arn:aws:health:{region}::event/{service}/{event_type_code}/{event_id}")
}

/// A stable, opaque entity id derived from the event ARN and the entity value, so
/// the entity ARN a client reads is the same on every call and distinct per entity.
fn entity_id_for(event_arn: &str, entity_value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_arn.as_bytes());
    hasher.update(b"\0");
    hasher.update(entity_value.as_bytes());
    let digest = hasher.finalize();
    // 20 base-16 characters: the same LENGTH as the `AVh5GGT7ul1arKr1sE1K` id the
    // `entityArn` doc shows, though that example is mixed-case base62 rather than hex.
    // The `entityArn` member is an unconstrained string (`^.{0,1600}$`), so only the
    // id's stability and uniqueness matter, not its alphabet.
    hex::encode(&digest[..10])
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Domain separator for the page-token authenticator below. A token is only
/// accepted when it re-derives under this key, so a value the server never issued
/// (a client-invented or corrupted token) is refused.
const PAGE_TOKEN_KEY: &[u8] = b"com.amazonaws.health/pagination-token/v1";

/// The authenticator over a page's (operation, request, offset) triple.
fn page_token_mac(operation: &str, request_digest: &str, offset: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PAGE_TOKEN_KEY);
    hasher.update(operation.as_bytes());
    hasher.update(b"\0");
    hasher.update(request_digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(offset.to_le_bytes());
    hex::encode(&hasher.finalize()[..12])
}

/// The opaque `nextToken` for the page starting at `offset`.
///
/// It matches the modelled `^[a-zA-Z0-9=/+_.-]{4,10000}$` pattern and is bound to
/// the operation and to the request it continues, so replaying it against another
/// operation or another filter is refused rather than silently answered from the
/// wrong result set.
pub fn issue_page_token(operation: &str, request_digest: &str, offset: usize) -> String {
    format!(
        "{offset:x}.{}",
        page_token_mac(operation, request_digest, offset)
    )
}

/// The offset a token names, or `None` when this operation and request did not
/// issue it.
pub fn decode_page_token(token: &str, operation: &str, request_digest: &str) -> Option<usize> {
    let (offset_hex, mac) = token.split_once('.')?;
    let offset = usize::from_str_radix(offset_hex, 16).ok()?;
    (page_token_mac(operation, request_digest, offset) == mac).then_some(offset)
}

/// The requested page of `items`, plus the token for the page after it.
///
/// `None` means the supplied `next_token` is not one this operation issued for this
/// request — the caller answers `InvalidPaginationToken`. The returned token is
/// absent on the last page, which is how a client knows the walk is done.
pub fn paginate<T: Clone>(
    items: &[T],
    operation: &str,
    request_digest: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
    default_page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    let offset = match next_token {
        None => 0,
        Some(token) => decode_page_token(token, operation, request_digest)?,
    };
    if offset > items.len() {
        return None;
    }
    let page_size = max_results.map_or(default_page_size, |requested| {
        // Every operation that paginates here models `maxResults` with a `range`
        // whose minimum is at least 1, and the framework refuses a value outside it
        // BEFORE this handler runs — so a non-positive `requested` is unreachable.
        // The fallbacks below keep that unreachable case a well-formed page rather
        // than an empty one that would make a walk loop forever.
        debug_assert!(
            requested >= 1,
            "maxResults below the modelled minimum reached the handler: {requested}"
        );
        usize::try_from(requested).unwrap_or(default_page_size).max(1)
    });
    let end = offset.saturating_add(page_size).min(items.len());
    let next = (end < items.len()).then(|| issue_page_token(operation, request_digest, end));
    Some((items[offset..end].to_vec(), next))
}

/// Accumulates the canonical rendering of a request's non-pagination members, whose
/// digest binds a page token to the request that produced it.
pub struct RequestDigest(Sha256);

impl RequestDigest {
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    /// A scalar member (an aggregate field, a locale).
    pub fn scalar(mut self, name: &str, value: Option<&str>) -> Self {
        self.0.update(name.as_bytes());
        self.0.update(b"=");
        self.0.update(value.unwrap_or("\u{1}").as_bytes());
        self.0.update(b";");
        self
    }

    /// A list member. Hashed order-independently, so two spellings of one filter
    /// continue the same walk.
    pub fn list(mut self, name: &str, values: Option<&[String]>) -> Self {
        self.0.update(name.as_bytes());
        self.0.update(b"=[");
        if let Some(values) = values {
            let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            for value in sorted {
                self.0.update(value.as_bytes());
                self.0.update(b",");
            }
        }
        self.0.update(b"];");
        self
    }

    /// A list-of-timestamp-ranges member.
    pub fn ranges(self, name: &str, values: Option<&[DateTimeRange]>) -> Self {
        let mut rendered: Vec<String> = values
            .unwrap_or(&[])
            .iter()
            .map(|range| {
                format!(
                    "{}-{}",
                    range.from.as_ref().map_or(i64::MIN, |t| t.secs()),
                    range.to.as_ref().map_or(i64::MAX, |t| t.secs())
                )
            })
            .collect();
        rendered.sort_unstable();
        self.list(name, Some(&rendered))
    }

    /// A `tags` filter member: a map of tag key to the values that key accepts.
    /// Hashed order-independently in both the map and each value list, so two
    /// spellings of one filter continue the same walk.
    pub fn tag_filter(self, name: &str, values: Option<&HashMap<String, Vec<String>>>) -> Self {
        let mut rendered: Vec<String> = values
            .map(|filter| {
                filter
                    .iter()
                    .map(|(key, accepted)| {
                        let mut sorted: Vec<&str> =
                            accepted.iter().map(String::as_str).collect();
                        sorted.sort_unstable();
                        format!("{key}={}", sorted.join(","))
                    })
                    .collect()
            })
            .unwrap_or_default();
        rendered.sort_unstable();
        self.list(name, Some(&rendered))
    }

    pub fn finish(self) -> String {
        hex::encode(&self.0.finalize()[..16])
    }
}

impl Default for RequestDigest {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RequestDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestDigest").finish_non_exhaustive()
    }
}

/// The canonical digest of an `EventFilter`-shaped request, shared by
/// `DescribeEvents` and `DescribeEventAggregates`.
pub fn event_filter_digest(filter: Option<&EventFilter>) -> RequestDigest {
    let empty = EventFilter::default();
    let filter = filter.unwrap_or(&empty);
    RequestDigest::new()
        .list("actionabilities", filter.actionabilities.as_deref())
        .list("eventArns", filter.event_arns.as_deref())
        .list("eventTypeCodes", filter.event_type_codes.as_deref())
        .list("services", filter.services.as_deref())
        .list("regions", filter.regions.as_deref())
        .list("availabilityZones", filter.availability_zones.as_deref())
        .ranges("startTimes", filter.start_times.as_deref())
        .ranges("endTimes", filter.end_times.as_deref())
        .ranges("lastUpdatedTimes", filter.last_updated_times.as_deref())
        .list("entityArns", filter.entity_arns.as_deref())
        .list("entityValues", filter.entity_values.as_deref())
        .list("eventTypeCategories", filter.event_type_categories.as_deref())
        .tag_filter("tags", filter.tags.as_ref())
        .list("eventStatusCodes", filter.event_status_codes.as_deref())
        .list("personas", filter.personas.as_deref())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsHealthAwshealth20160804State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsHealthAwshealth20160804State>)
}

/// This service's store, or the failure a handler answers when the engine it was
/// built on is not one this crate can reach.
///
/// Unreachable in the assembled emulator — the router builds a single
/// `engine::InMemoryEngine` and hands it to every service — so this is the
/// "this build cannot serve the operation at all" case, which is exactly what the
/// 501 channel states. None of these operations models a server-side error, so
/// there is no modelled shape to answer with instead.
pub fn require_state<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    operation: &str,
) -> Result<
    ::engine::ServiceState<ComAmazonawsHealthAwshealth20160804State>,
    crate::scaffolded::Scaffolded<E>,
> {
    service_state(engine).ok_or_else(|| crate::scaffolded::Scaffolded::not_implemented(operation))
}
