// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdashboards-awsbcmdashboardsservice service.
//!
//! The dashboard records every dashboard operation reads and writes live here, so
//! `CreateDashboard`, `GetDashboard`, `ListDashboards`, `UpdateDashboard` and
//! `DeleteDashboard` all see one copy of each resource. The helpers below (ARN and
//! widget-id minting, the `DashboardReference` projection, the tag resolution the
//! tagging operations share, the pagination cursor codec) are shared by those handlers
//! rather than duplicated per operation.
//!
//! # An execution of a scheduled report is a delivery
//!
//! A scheduled report's whole purpose is a notification on the account's default Amazon
//! EventBridge event bus: `HealthStatus$statusCode` documents `UNHEALTHY` as "the
//! scheduled report is unable to deliver the notification to the default Amazon
//! EventBridge EventBus in your account". So an execution is not internal bookkeeping —
//! it hands one event to that bus ([`deliver_execution_notification`]), and the health
//! the execution establishes reports whether that delivery succeeded
//! ([`undelivered_notification_health`]). EventBridge is reached the way any emulated
//! service reaches another: over the in-process callback, so nothing is dialled and this
//! crate names no other service crate.
//!
//! # A scheduled report executes AS a role
//!
//! The report also has to be able to run at all: the model documents its
//! `scheduledReportExecutionRoleArn` as the role this service "will assume while executing
//! the scheduled report", and `HEALTHY` as the report having "all required permissions to
//! execute". So every resolution of a report's health asks the account's IAM records what
//! they show about that role ([`execution_role`]): it reports
//! `EXECUTION_ROLE_ASSUME_FAILED` while the role no longer admits this service, and
//! `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS` while the role it can assume may not do the
//! work an execution does under it.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    CostAndUsageQuery, DashboardReference, DateTimeRange, DateTimeValue, DisplayConfig,
    GraphDisplayConfig, GroupDefinition, HealthStatus, QueryParameters, ReservationUtilizationQuery,
    ResourceTag, SavingsPlansCoverageQuery, ScheduleConfig, SchedulePeriod, ScheduledReport,
    ScheduledReportSummary, TableDisplayConfigStruct, Widget, WidgetConfig,
};

/// What the account's IAM shows about the role a scheduled report executes as, which the
/// health derivation below is one half of.
pub mod execution_role;

pub use execution_role::ExecutionRoleFaults;

/// The only `DashboardType` the model declares (`DASHBOARD_TYPE_VALUES`), and the
/// type every dashboard a client creates through this API carries.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";

/// The `DashboardType` the dashboards Amazon Web Services publishes itself carry.
///
/// The model's `DashboardType` enum lists `CUSTOM` alone
/// (`smithy_query(query="shape",shape="DashboardType")`), while every reference in the
/// AWS-managed catalog this service answers a brand-new account with carries
/// `AWS_MANAGED`. A dashboard's `type` travels as a string on the wire, so the value is
/// representable: it is stored on the record and projected like any other, rather than
/// derived per request.
pub const DASHBOARD_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The `policyDocument` a dashboard with no resource-based policy attached reads back:
/// the empty JSON document.
///
/// `GetResourcePolicy$policyDocument` is @required and reports a policy DOCUMENT, which
/// a caller feeds straight to a JSON parser — so "no policy" is the document that holds
/// no statement, which parses to an empty policy. The empty STRING is not a JSON document
/// at all and fails that parse, even though `GenericString`'s modelled length (0..1024)
/// would admit it.
pub const EMPTY_RESOURCE_POLICY_DOCUMENT: &str = "{}";

/// The `ScheduleState` (`SCHEDULE_STATE_VALUES`) a schedule carries when the request
/// left `scheduleConfig.state` unset: a report whose state nobody chose runs on its
/// schedule, which is what the recorded unit
/// `bcm-dashboards.scheduled-report-default-schedule-state-enabled` reads back.
pub const SCHEDULE_STATE_ENABLED: &str = "ENABLED";

/// The `HealthStatusCode` (`HEALTH_STATUS_CODE_VALUES`) a scheduled report reports
/// while nothing has refreshed its health: the model documents `HEALTHY` as "the
/// scheduled report is configured properly and has all required permissions to
/// execute", which is what a report this service accepted at create time is.
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";

/// The other `HealthStatusCode` (`HEALTH_STATUS_CODE_VALUES`): the model documents
/// `UNHEALTHY` as "the scheduled report is unable to deliver the notification … and
/// your action is needed", with the reasons in `statusReasons`.
pub const HEALTH_STATUS_UNHEALTHY: &str = "UNHEALTHY";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `dashboardArn` names no dashboard this account stores any more — the dashboard the
/// report renders was deleted after the report was created.
pub const STATUS_REASON_DASHBOARD_NOT_FOUND: &str = "DASHBOARD_NOT_FOUND";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `widgetIds` name a widget its dashboard does not hold — the widget was removed from
/// the dashboard after the report was created.
pub const STATUS_REASON_WIDGET_ID_NOT_FOUND: &str = "WIDGET_ID_NOT_FOUND";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `scheduledReportExecutionRoleArn` names a role this service can no longer assume — the
/// role's trust policy stopped admitting this service after the report was created.
pub const STATUS_REASON_EXECUTION_ROLE_ASSUME_FAILED: &str = "EXECUTION_ROLE_ASSUME_FAILED";

/// The `StatusReason` (`STATUS_REASON_VALUES`) for a scheduled report whose
/// `scheduledReportExecutionRoleArn` names a role this service can assume but whose
/// policies deny it the work an execution does under that role — the role's permissions
/// stopped covering the execution after the report was created.
pub const STATUS_REASON_EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS: &str =
    "EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS";

/// The `StatusReason` values this store can DERIVE from the resources a scheduled
/// report's configuration points at, so resolving a report's health re-derives them
/// from what the store holds now instead of repeating what an earlier refresh found.
/// Every other reason a refresh established is carried forward from the record, because
/// nothing in the store can re-establish it.
const DERIVED_STATUS_REASONS: [&str; 4] = [
    STATUS_REASON_EXECUTION_ROLE_ASSUME_FAILED,
    STATUS_REASON_EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS,
    STATUS_REASON_DASHBOARD_NOT_FOUND,
    STATUS_REASON_WIDGET_ID_NOT_FOUND,
];

/// The `StatusReason` (`STATUS_REASON_VALUES`) for an execution whose notification the
/// account's default event bus did not take: the report's configuration resolved, so the
/// failure is neither a missing dashboard nor a missing widget — it is the delivery
/// itself, which is what this value names.
pub const STATUS_REASON_INTERNAL_FAILURE: &str = "INTERNAL_FAILURE";

/// How long the `clientToken` of an `ExecuteScheduledReport` call keeps answering with
/// that call's outcome instead of triggering a second execution.
///
/// CITED, not chosen: the model documents the window on the operation itself — "If a
/// `clientToken` is provided, the service uses it for idempotency. Requests with the
/// same client token will not trigger a new execution within the same minute."
/// (`smithy_query(query="doc",shape="ExecuteScheduledReport")`). One minute.
pub const EXECUTION_IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(60);

/// The prefix a pagination cursor carries, so a token this service did not mint is
/// rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";


/// The most dashboards this emulator stores for the account before `CreateDashboard`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's dashboard quota is not stated in the model, so this bound is
/// CHOSEN, not cited: it is set far above any plausible client's dashboard count, so
/// normal use never meets it, while the store a caller can grow stays bounded.
pub const MAX_DASHBOARDS: usize = 1_000;

/// The most tags this emulator stores on ONE dashboard or scheduled report. A merge
/// that would carry a resource past it is refused by `TagResource` with its modelled
/// `ValidationException`, storing nothing.
///
/// The model states no per-resource tag quota for this service, so the ceiling is
/// CHOSEN, not cited — and chosen to be exactly `ResourceTagList`'s own modelled maximum
/// (length 0..200, `smithy_query(query="shape",shape="ResourceTagList")`): a resource
/// holds at most as many tags as one request may carry. That is what bounds the store a
/// caller can grow by repeating `TagResource` with fresh keys, and with it the linear
/// scan a merge runs while the state lock is held. The create-time tag path needs no
/// second check for the same reason: `CreateDashboard`'s and `CreateScheduledReport`'s
/// `resourceTags` are that same 0..200 list, so the boundary validator already refuses a
/// create that would enter the store above this ceiling.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix Amazon Web Services reserves for the tags it generates itself.
///
/// AWS's tagging rules state that the `aws:` prefix is reserved for AWS use and that a
/// caller can neither create nor edit nor delete a tag key carrying it, so every path
/// on this surface that takes a caller-supplied tag key refuses one in this namespace
/// instead of storing or removing it ([`check_tag_keys_not_reserved`]). The model cannot
/// express the rule — `ResourceTagKey`'s pattern is `^[\S\s]*$`
/// (`smithy_query(query="shape",shape="ResourceTagKey")`) — so the handlers enforce it.
/// The comparison is case-insensitive, because the prefix is reserved however it is
/// cased.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The most characters an error message of this service may carry.
///
/// Every error shape on this surface declares its `message` against `GenericString`,
/// whose `@length` is `min 0, max 1024`
/// (`smithy_query(query="shape",shape="GenericString,ValidationException")`). The members a
/// message quotes are bounded far more loosely than that — one `resourceTags` list carries
/// up to 200 keys of up to 128 characters each, and a `ResourceArn` up to 2048 — so a
/// message built by quoting them is bounded HERE, by
/// [`within_message_bound`], before it goes on the wire. A client that sized or stored a
/// message against the maximum its own copy of the model declares then never sees a value
/// the model said could not occur.
pub const MAX_MESSAGE_LENGTH: usize = 1024;

/// What a message carries in place of the text [`MAX_MESSAGE_LENGTH`] left no room for.
const MESSAGE_ELISION: &str = "...";

/// How a message separates the values it quotes from a modelled list.
const MESSAGE_LIST_SEPARATOR: &str = ", ";

/// How the TAGGING path words a `ResourceArn` that resolves to no stored resource.
///
/// A constant, carrying nothing from the request — which is what the recorded
/// `ListTagsForResource` not-found answer says, verbatim. Both tagging operations that
/// resolve an ARN through this state's `resource_tags` / `resource_tags_mut`
/// word that one refusal the same way; the ARN itself travels in the answer's `resourceId`
/// member, not in the text (see `crate::scaffolded::Scaffolded::resource_not_found`).
pub const TAGGING_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// One stored dashboard.
///
/// `widgets` holds the modelled [`Widget`] shape verbatim — including the widget
/// ids this service minted and the `width`/`height`/`horizontalOffset` values the
/// model's defaults filled in — so `GetDashboard` reads back exactly what was
/// stored rather than re-deriving anything from the request.
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// `DashboardType` as it goes on the wire; always [`DASHBOARD_TYPE_CUSTOM`]
    /// for a dashboard created through `CreateDashboard`.
    pub dashboard_type: String,
    pub widgets: Vec<Widget>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags supplied at create time, read back by `ListTagsForResource` and
    /// narrowed by `UntagResource`, in the order they were stored.
    pub resource_tags: Vec<ResourceTag>,
    /// The resource-based policy document attached to this dashboard, or `None`
    /// while no policy is attached — the two are distinct, and `GetResourcePolicy`
    /// projects the stored document rather than deriving one from the request.
    ///
    /// This service's model declares no operation that attaches a policy (there is
    /// no `PutResourcePolicy` among its 15 operations), so a dashboard created
    /// through this API keeps `None` and reads back
    /// [`EMPTY_RESOURCE_POLICY_DOCUMENT`]. The field is the single place a policy
    /// would live once an operation can write one, so the reader needs no second map
    /// beside the dashboards.
    pub resource_policy: Option<String>,
}

impl Dashboard {
    /// This dashboard projected onto the `DashboardReference` shape `ListDashboards`
    /// returns: the stored record's own fields, never request input.
    pub fn reference(&self) -> DashboardReference {
        DashboardReference {
            arn: self.arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.dashboard_type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The resource-id prefix of every AWS-managed dashboard's ARN, which is what tells one
/// apart from a dashboard a caller created: the catalog's ARNs are
/// `arn:aws:bcm-dashboards::<account>:dashboard/managed-<32 hex>`, while
/// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::mint_dashboard_arn`] mints a
/// bare 32-hex resource id. Both spellings satisfy `DashboardArn`'s `[-a-z0-9]+` resource
/// segment.
const MANAGED_DASHBOARD_ID_PREFIX: &str = "managed-";

/// When the AWS-managed catalog this service seeds was published, and when it was last
/// revised, as seconds since the Unix epoch.
///
/// The catalog is Amazon Web Services' own data, not the account's: its dashboards exist
/// before a caller writes anything, so their `createdAt`/`updatedAt` are the instants AWS
/// published and last revised the catalog rather than anything this emulator's clock
/// produced — which is why they are stored constants and are stable across reads and
/// across restarts. A dashboard a CALLER creates is stamped from the clock
/// (`CreateDashboard`), as its own record's instants.
const MANAGED_CATALOG_CREATED_AT: u64 = 1_786_644_000;
const MANAGED_CATALOG_UPDATED_AT: u64 = 1_787_079_600;

/// The `DateTimeType` (`DATE_TIME_TYPE_VALUES`) a widget's time range uses when its
/// bounds are offsets from the instant the widget is rendered rather than fixed dates.
const DATE_TIME_TYPE_RELATIVE: &str = "RELATIVE";

/// The `DateTimeType` (`DATE_TIME_TYPE_VALUES`) a time range uses when its bounds are
/// fixed dates rather than offsets.
const DATE_TIME_TYPE_ABSOLUTE: &str = "ABSOLUTE";

/// The `Granularity` (`GRANULARITY_VALUES`) every widget in the AWS-managed catalog
/// aggregates at: a month, which is the period a bill covers.
const GRANULARITY_MONTHLY: &str = "MONTHLY";

/// The `GroupDefinitionType` (`GROUP_DEFINITION_TYPE_VALUES`) a grouping by a cost
/// dimension carries — also the member's modelled `@default`.
const GROUP_DEFINITION_TYPE_DIMENSION: &str = "DIMENSION";

/// A widget time range running from `start` to `end`, both relative offsets (ISO 8601
/// durations, as `DateTimeValue$value` carries them for a `RELATIVE` bound).
fn relative_range(start: &str, end: &str) -> DateTimeRange {
    DateTimeRange {
        start_time: DateTimeValue {
            r#type: DATE_TIME_TYPE_RELATIVE.to_string(),
            value: start.to_string(),
        },
        end_time: DateTimeValue {
            r#type: DATE_TIME_TYPE_RELATIVE.to_string(),
            value: end.to_string(),
        },
    }
}

/// A `DisplayConfig` drawing one metric as `visual_type` (a `VisualType`:
/// `VISUAL_TYPE_VALUES` are `LINE`, `BAR`, `STACK`).
fn graph_display(metric: &str, visual_type: &str) -> DisplayConfig {
    DisplayConfig::Graph(HashMap::from([(
        metric.to_string(),
        GraphDisplayConfig {
            visual_type: visual_type.to_string(),
        },
    )]))
}

/// A `DisplayConfig` drawing the query's result as a table.
fn table_display() -> DisplayConfig {
    DisplayConfig::Table(TableDisplayConfigStruct {})
}

/// One widget of the AWS-managed catalog: the members a caller's widget carries too,
/// with the model's declared `width`/`height`/`horizontalOffset` defaults (4, 7, 0)
/// unless the catalog entry states its own.
struct ManagedWidget {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    width: i32,
    height: i32,
    configs: Vec<WidgetConfig>,
}

impl ManagedWidget {
    /// This catalog widget as the modelled [`Widget`] every reader projects.
    fn widget(self) -> Widget {
        Widget {
            id: Some(self.id.to_string()),
            title: self.title.to_string(),
            description: Some(self.description.to_string()),
            width: self.width,
            height: self.height,
            horizontal_offset: 0,
            configs: self.configs,
        }
    }
}

/// The widgets of the `Cost Overview and Trends` catalog entry.
fn cost_overview_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "a18b58807f3934dbcfebd21ad359afaa",
        title: "Total Current And Forecasted Cost",
        description: "Historical monthly unblended cost with forecasted cost.",
        width: 2,
        height: 4,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::costAndUsage(CostAndUsageQuery {
                metrics: vec!["UnblendedCost".to_string()],
                time_range: relative_range("-P12M", "P4M"),
                granularity: GRANULARITY_MONTHLY.to_string(),
                group_by: Some(Vec::new()),
                filter: None,
            }),
            display_config: graph_display("UnblendedCost", "BAR"),
        }],
    }
    .widget()]
}

/// The widgets of a catalog entry that reports cost and usage grouped by one
/// `Dimension` (`DIMENSION_VALUES`) — the shape the `Compute` and `Database` entries
/// take, each over its own dimension.
fn cost_by_dimension_widgets(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    dimension: &'static str,
) -> Vec<Widget> {
    vec![ManagedWidget {
        id,
        title,
        description,
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::costAndUsage(CostAndUsageQuery {
                metrics: vec!["UnblendedCost".to_string()],
                time_range: relative_range("-P6M", "P1M"),
                granularity: GRANULARITY_MONTHLY.to_string(),
                group_by: Some(vec![GroupDefinition {
                    key: dimension.to_string(),
                    r#type: GROUP_DEFINITION_TYPE_DIMENSION.to_string(),
                }]),
                filter: None,
            }),
            display_config: graph_display("UnblendedCost", "STACK"),
        }],
    }
    .widget()]
}

/// The widgets of the `Reservations` catalog entry: how much of the account's
/// reservations were used.
fn reservation_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "c5a7d0b3e19f4c82a6d4b8e01f3c7a95",
        title: "Reservation Utilization",
        description: "Monthly utilization of your Reserved Instances.",
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::reservationUtilization(
                ReservationUtilizationQuery {
                    time_range: relative_range("-P6M", "P0M"),
                    group_by: None,
                    granularity: Some(GRANULARITY_MONTHLY.to_string()),
                    filter: None,
                },
            ),
            display_config: table_display(),
        }],
    }
    .widget()]
}

/// The widgets of the `Savings Plans` catalog entry: how much of the account's eligible
/// spend the Savings Plans it holds covered.
fn savings_plans_widgets() -> Vec<Widget> {
    vec![ManagedWidget {
        id: "d6b8e1c4f20a5d93b7e5c9f12a4d8b06",
        title: "Savings Plans Coverage",
        description: "Monthly share of eligible spend covered by your Savings Plans.",
        width: 4,
        height: 7,
        configs: vec![WidgetConfig {
            query_parameters: QueryParameters::savingsPlansCoverage(SavingsPlansCoverageQuery {
                time_range: relative_range("-P6M", "P0M"),
                metrics: Some(vec!["SpendCoveredBySavingsPlans".to_string()]),
                granularity: Some(GRANULARITY_MONTHLY.to_string()),
                group_by: None,
                filter: None,
            }),
            display_config: table_display(),
        }],
    }
    .widget()]
}

/// One stored dashboard of the AWS-managed catalog, built the same way a created one is:
/// the record carries its own ARN, name, description, type and widgets, and every reader
/// projects it from the store.
fn managed_dashboard(
    resource_id: &str,
    name: &str,
    description: &str,
    widgets: Vec<Widget>,
) -> Dashboard {
    Dashboard {
        arn: ::engine::emulator_arn(
            ARN_NAMESPACE,
            "",
            &format!("{DASHBOARD_RESOURCE_TYPE}/{MANAGED_DASHBOARD_ID_PREFIX}{resource_id}"),
        ),
        name: name.to_string(),
        description: Some(description.to_string()),
        dashboard_type: DASHBOARD_TYPE_AWS_MANAGED.to_string(),
        widgets,
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_CATALOG_CREATED_AT),
        updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_CATALOG_UPDATED_AT),
        // The catalog carries no tags and no resource-based policy: a tag is an
        // account's own metadata, and this model declares no operation that attaches a
        // policy at all.
        resource_tags: Vec::new(),
        resource_policy: None,
    }
}

/// The AWS-managed dashboard catalog every account already holds, keyed by ARN exactly
/// as a created dashboard is.
///
/// These dashboards are the service's OWN data rather than the account's: an account that
/// has created nothing still lists them, and `GetDashboard` reads one back with its
/// widgets, which is the first call a console-like client makes. They are therefore
/// SEEDED into the store ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::default`])
/// and read back through the same map, the same projection and the same pagination as a
/// dashboard a caller created — no handler branches on whether a dashboard is managed, and
/// nothing about the catalog is derived from a request.
///
/// `pub` so a test can state the catalog a listing must report alongside its own
/// dashboards from the one place that defines it.
pub fn managed_dashboard_catalog() -> BTreeMap<String, Dashboard> {
    [
        managed_dashboard(
            "4dc5619130f2463687fc45e003e4b930",
            "Cost Overview and Trends",
            "High-level view of your AWS costs, usage trends, and forecasts",
            cost_overview_widgets(),
        ),
        managed_dashboard(
            "6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d",
            "Compute",
            "Cost and usage of your compute services, including Amazon EC2, AWS Lambda and \
             AWS Fargate",
            cost_by_dimension_widgets(
                "b29c69918a0f45e3d7fce32be46a0bb6",
                "Compute Cost By Instance Type",
                "Monthly unblended cost of your compute usage, grouped by instance type.",
                "INSTANCE_TYPE",
            ),
        ),
        managed_dashboard(
            "7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e",
            "Database",
            "Cost and usage of your database services, including Amazon RDS, Amazon DynamoDB \
             and Amazon ElastiCache",
            cost_by_dimension_widgets(
                "e7c9f2a5b38d4e06c1a9f7b2d5e8c304",
                "Database Cost By Engine",
                "Monthly unblended cost of your database usage, grouped by database engine.",
                "DATABASE_ENGINE",
            ),
        ),
        managed_dashboard(
            "8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f",
            "Reservations",
            "Utilization and coverage of the Reserved Instances in your account",
            reservation_widgets(),
        ),
        managed_dashboard(
            "9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a",
            "Savings Plans",
            "Utilization and coverage of the Savings Plans in your account",
            savings_plans_widgets(),
        ),
    ]
    .into_iter()
    .map(|dashboard| (dashboard.arn.clone(), dashboard))
    .collect()
}

/// The most scheduled reports this emulator stores for the account before
/// `CreateScheduledReport` answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's scheduled-report quota is not stated in the model, so this
/// bound is CHOSEN, not cited: it is set far above any plausible client's report
/// count, so normal use never meets it, while the store a caller can grow stays
/// bounded.
pub const MAX_SCHEDULED_REPORTS: usize = 1_000;

/// One stored scheduled report.
///
/// Holds the modelled members verbatim — including the `scheduleConfig` with its
/// `state` resolved — so every reader projects the stored record rather than
/// re-deriving anything from a request. `last_execution_at` and `health_status` stay
/// unset until an execution of this report sets them: nothing has run at create time.
/// A reader never projects `health_status` directly — the health every view reports is
/// resolved from the whole store by
/// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`],
/// because it depends on the resources this record points at and not on this record
/// alone.
#[derive(Debug, Clone)]
pub struct ScheduledReportRecord {
    pub arn: String,
    pub name: String,
    pub dashboard_arn: String,
    pub execution_role_arn: String,
    pub schedule_config: ScheduleConfig,
    pub description: Option<String>,
    /// `None` when the request named no widget, `Some(list)` otherwise — the two are
    /// distinct on the wire, and `UpdateScheduledReport` has to be able to reset the
    /// member to an empty list.
    pub widget_ids: Option<Vec<String>>,
    pub widget_date_range_override: Option<DateTimeRange>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub last_execution_at: Option<SystemTime>,
    /// The health this report's last refresh established, unset until one has: the
    /// readers project
    /// [`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`],
    /// never this member.
    pub health_status: Option<HealthStatus>,
    /// The tags supplied at create time, read back by `ListTagsForResource` and
    /// narrowed by `UntagResource`, in the order they were stored.
    pub resource_tags: Vec<ResourceTag>,
}

impl ScheduledReportRecord {
    /// The `ScheduleState` this report's schedule is in: the stored value, or
    /// [`SCHEDULE_STATE_ENABLED`] for a record stored without one. `state` is resolved
    /// at write time ([`resolve_schedule_config`] on a create,
    /// [`updated_schedule_config`] on an update), so the fallback only covers a record
    /// written without one; resolving it in ONE place ([`schedule_config_state`]) is
    /// what lets every reader and `ExecuteScheduledReport` agree on whether a report's
    /// schedule is enabled.
    pub fn schedule_state(&self) -> &str {
        schedule_config_state(&self.schedule_config)
    }

    /// This record projected onto the modelled `ScheduledReport` shape that
    /// `GetScheduledReport` (and the other readers) return: the stored record's own
    /// fields, never request input.
    ///
    /// `healthStatus` is optional on this shape and `@required` on the summary, but it
    /// is the same property of the same record either way, so the detail view reports
    /// exactly the `health` its caller resolved rather than reporting less than the
    /// listing does. The resolution is the store's
    /// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`]),
    /// never a projection's own.
    pub fn report(&self, health: HealthStatus) -> ScheduledReport {
        ScheduledReport {
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            dashboard_arn: self.dashboard_arn.clone(),
            scheduled_report_execution_role_arn: self.execution_role_arn.clone(),
            schedule_config: self.schedule_config.clone(),
            description: self.description.clone(),
            widget_ids: self.widget_ids.clone(),
            widget_date_range_override: self.widget_date_range_override.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            last_execution_at: self.last_execution_at.map(::aws_smithy_types::DateTime::from),
            health_status: Some(health),
        }
    }

    /// This record projected onto the modelled `ScheduledReportSummary` shape that
    /// `ListScheduledReports` pages over: the stored record's own fields, never
    /// request input.
    ///
    /// Two members the summary declares `@required` are optional on the stored
    /// record, so each is resolved here rather than by every reader:
    ///
    /// * `scheduleExpression` is optional inside `ScheduleConfig`, and a
    ///   `GenericString` of length 0..1024 — so a schedule stored without an
    ///   expression summarises as the empty string rather than inventing one.
    /// * `healthStatus` is optional on the stored record until something refreshes it,
    ///   and is the `health` its caller resolved from the store
    ///   ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reported_scheduled_report_health`])
    ///   — the same resolution the detail view uses, so both views of one record report
    ///   one health.
    pub fn summary(&self, health: HealthStatus) -> ScheduledReportSummary {
        ScheduledReportSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            dashboard_arn: self.dashboard_arn.clone(),
            schedule_expression: self
                .schedule_config
                .schedule_expression
                .clone()
                .unwrap_or_default(),
            state: self.schedule_state().to_string(),
            health_status: health,
            schedule_expression_time_zone: self
                .schedule_config
                .schedule_expression_time_zone
                .clone(),
            widget_ids: self.widget_ids.clone(),
        }
    }
}

/// What the `CreateScheduledReport` call that claimed a `clientToken` has done with it.
///
/// The member is modelled `@idempotencyToken`, so the token is the caller's key for ONE
/// create: the claim is taken by the call that commits to creating
/// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reserve_scheduled_report_creation`])
/// and settled with the ARN of the report that call stored
/// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::settle_scheduled_report_creation`]),
/// so one token answers for exactly one report for as long as that report is stored.
///
/// A token a create has claimed is SPENT: every later call presenting it is refused with
/// the modelled `ConflictException` — what the model documents it for, "attempting to
/// create a resource that already exists or is being created" — and creates nothing. Which
/// of the two conflicts it is is what this record says
/// ([`Self::conflict_message`]):
///
/// * [`Self::Created`] — the create holding the claim stored its report, so the resource
///   ALREADY EXISTS under the ARN the claim names, whatever the later call's own request
///   asks for. The conflict is terminal: the caller picks another token, or updates the
///   report this one names.
/// * [`Self::InFlight`] — the create holding the claim has not stored its report yet, so the
///   resource IS BEING CREATED. That one is transient: the caller may present the same token
///   again.
#[derive(Debug, Clone)]
pub enum ScheduledReportCreation {
    /// A create has claimed the token and has not stored its report yet.
    InFlight,
    /// The create that claimed the token stored its report under this ARN.
    Created { arn: String },
}

impl ScheduledReportCreation {
    /// The ARN of the report the create holding this claim stored, or `None` while that
    /// create is still in flight and has stored nothing to name.
    pub fn created_arn(&self) -> Option<&str> {
        match self {
            Self::InFlight => None,
            Self::Created { arn } => Some(arn.as_str()),
        }
    }

    /// The `ConflictException` message a `CreateScheduledReport` call presenting this
    /// claim's `clientToken` is refused with.
    ///
    /// The service words the refusal from the state of the create that already claimed
    /// the token, and the wording is part of the contract: a client decides from it
    /// whether to back off and present the same token again (the creation is still in
    /// flight, so the conflict is transient) or to pick another token (the report exists,
    /// so the conflict is terminal). Both texts are therefore FIXED; the only value
    /// either of them carries is the ARN of the report already created, which the caller
    /// may act on. The presented token is deliberately absent from both: it is the
    /// caller's idempotency key, and an error body is not where it belongs.
    pub fn conflict_message(&self) -> String {
        match self {
            Self::InFlight => "A scheduled report creation is already in progress for the \
                 provided clientToken. Please retry after some time."
                .to_string(),
            Self::Created { arn } => format!(
                "A scheduled report with the provided clientToken already exists (ARN: {arn}). \
                 Use a different clientToken to create a new scheduled report, or use the \
                 existing report's ARN to update it."
            ),
        }
    }
}

/// The outcome one `ExecuteScheduledReport` call recorded under the report it executed
/// and the `clientToken` it presented, so a RETRY of that same call inside
/// [`EXECUTION_IDEMPOTENCY_WINDOW`] answers that same outcome rather than triggering a
/// second execution.
///
/// The record is keyed by the pair, never by the token alone: the token suppresses a
/// repeat of the request that carried it, and the request names the report, so the same
/// token presented for another report is a different request and is decided on its own
/// merits.
///
/// It is written BEFORE the execution's delivery is attempted
/// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reserve_execution_token`]),
/// holding the outcome the call is already committed to at that point — whether it
/// triggers an execution, and the health its stored configuration establishes — and the
/// health is then settled with what the delivery established
/// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::settle_execution_token`]).
/// `executed_at` is the instant of the call that reserved it, which is both what the
/// expiry measures the idempotency window from and what identifies the reservation to
/// settle.
#[derive(Debug, Clone)]
pub struct ScheduledReportExecution {
    pub executed_at: SystemTime,
    pub execution_triggered: bool,
    pub health_status: HealthStatus,
}

/// Per-service state for com-amazonaws-bcmdashboards-awsbcmdashboardsservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Every stored dashboard, keyed by its ARN. A `BTreeMap` so `ListDashboards`
    /// walks the dashboards in one stable order and a cursor taken on one page
    /// resumes at the right place on the next.
    pub dashboards: BTreeMap<String, Dashboard>,
    /// Monotonic counter behind every minted dashboard resource id, so no two
    /// dashboards — including one created after another was deleted — share an ARN.
    pub next_dashboard_seq: u64,
    /// Monotonic counter behind every minted widget id.
    pub next_widget_seq: u64,
    /// Every stored scheduled report, keyed by its ARN. A `BTreeMap` for the same
    /// reason the dashboards are in one: `ListScheduledReports` needs one stable
    /// order to page over.
    pub scheduled_reports: BTreeMap<String, ScheduledReportRecord>,
    /// Monotonic counter behind every minted scheduled-report resource id.
    pub next_scheduled_report_seq: u64,
    /// The `clientToken` of every `CreateScheduledReport` that has claimed one, mapped to
    /// what the create holding that claim has done ([`ScheduledReportCreation`]). A token
    /// is the caller's key for ONE create: the call that commits to creating claims it
    /// ([`Self::reserve_scheduled_report_creation`]) and settles the claim with the ARN of
    /// the report it stored and the content of the request that stored it
    /// ([`Self::settle_scheduled_report_creation`]), so a repeat of that request under the
    /// same token is answered the ARN the first call minted
    /// ([`Self::replayed_scheduled_report_creation`]) and a token can never mint a second
    /// report: a DIFFERENT request under a settled claim is refused with the modelled
    /// `ConflictException` in the name of the report the token already created, and any
    /// request under an unsettled one is refused with the same error's other documented
    /// case, the report being created. A call that is refused before it stores anything
    /// releases its claim ([`Self::release_scheduled_report_creation`]), leaving the token
    /// free. A settled claim whose report is no longer stored is dropped
    /// ([`Self::forget_creations_of_removed_reports`]), which is what keeps the map
    /// bounded by [`MAX_SCHEDULED_REPORTS`] — one record per stored report at most — and
    /// what frees a token whose report a caller deleted.
    pub scheduled_report_client_tokens: BTreeMap<String, ScheduledReportCreation>,
    /// Every `ExecuteScheduledReport` call that has triggered (or declined to trigger)
    /// an execution under a `clientToken`, keyed by the pair (the executed report's ARN,
    /// that token) and mapped to that call's outcome — the scope an idempotency token
    /// has, which is the request it was presented with, not every request sharing its
    /// value. An entry is RESERVED by the call that executes, under the same guard that
    /// read it ([`Self::reserve_execution_token`]), and settled with the health its
    /// delivery established ([`Self::settle_execution_token`]), so a duplicate still in
    /// flight is suppressed exactly as one arriving afterwards is. An entry lives for
    /// [`EXECUTION_IDEMPOTENCY_WINDOW`] from the call that wrote it and is dropped once
    /// the clock has closed its window
    /// ([`Self::expire_execution_tokens`]), so the map a caller can grow stays bounded by
    /// the calls of one minute.
    pub scheduled_report_executions: BTreeMap<(String, String), ScheduledReportExecution>,
}

/// A fresh store already holds the AWS-managed dashboard catalog
/// ([`managed_dashboard_catalog`]) and nothing else: those dashboards are the service's
/// own, so they exist before any caller writes anything, which is what `ListDashboards`
/// reports on a brand-new account and what `GetDashboard` reads back. Everything else
/// starts empty — an account owns no custom dashboard and no scheduled report until it
/// creates one.
impl Default for ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    fn default() -> Self {
        Self {
            dashboards: managed_dashboard_catalog(),
            next_dashboard_seq: 0,
            next_widget_seq: 0,
            scheduled_reports: BTreeMap::new(),
            next_scheduled_report_seq: 0,
            scheduled_report_client_tokens: BTreeMap::new(),
            scheduled_report_executions: BTreeMap::new(),
        }
    }
}

/// The ARN namespace this service's ARNs carry, as the model's `aws.api#service`
/// trait declares it (`arnNamespace: bcm-dashboards`).
const ARN_NAMESPACE: &str = "bcm-dashboards";

/// The ARN resource-type segment of a dashboard ARN, as `DashboardArn`'s pattern
/// declares it. Also the resource type a `ListDashboards` cursor's ARN names.
pub const DASHBOARD_RESOURCE_TYPE: &str = "dashboard";

/// The ARN resource-type segment of a scheduled-report ARN, as `ScheduledReportArn`'s
/// pattern declares it. Also the resource type a `ListScheduledReports` cursor's ARN
/// names.
pub const SCHEDULED_REPORT_RESOURCE_TYPE: &str = "scheduled-report";

/// Advance `counter` and render its new value as a resource id: 32 lowercase hex
/// characters, which satisfies both the `[-a-z0-9]+` resource segment the ARN patterns
/// admit and `WidgetId`'s stricter `^[0-9a-f]{32}$`.
///
/// The counter is monotonic, so no two ids this store mints for one resource type are
/// equal — including an id minted after the resource holding the previous one was
/// deleted. `wrapping_add` keeps the bump panic-free in a debug build; a `u64` counter
/// cannot be driven to its ceiling by API calls.
fn next_resource_id(counter: &mut u64) -> String {
    *counter = counter.wrapping_add(1);
    format!("{:032x}", *counter)
}

/// Mint a fresh, unique ARN for one resource type: the next id from `counter` under
/// `resource_type`, with the emulator's canonical account id supplied by
/// `engine::emulator_arn`.
///
/// `bcm-dashboards` ARNs carry no region (the model's ARN patterns have an empty region
/// segment), so the region passed is empty. Every resource type this store mints an ARN
/// for goes through here, so the id width, the region and the account all stay the same
/// shape across types.
fn mint_arn(counter: &mut u64, resource_type: &str) -> String {
    let resource_id = next_resource_id(counter);
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        "",
        &format!("{resource_type}/{resource_id}"),
    )
}

impl ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState {
    /// Mint a fresh, unique dashboard ARN, as [`mint_arn`] mints every resource ARN
    /// this store hands out: a 32-hex-character resource id under the `dashboard`
    /// resource type, which satisfies the `DashboardArn` pattern's `[-a-z0-9]+`
    /// resource segment.
    pub fn mint_dashboard_arn(&mut self) -> String {
        mint_arn(&mut self.next_dashboard_seq, DASHBOARD_RESOURCE_TYPE)
    }

    /// The ARN the dashboard named `name` is stored under, or `None` when no stored
    /// dashboard carries that name.
    ///
    /// A dashboard's name identifies it within the account — `CreateDashboard`'s `name`
    /// is documented as "The name of the dashboard. The name must be unique within your
    /// account." (`smithy_query(query="doc",shape="CreateDashboardRequest",member="name")`)
    /// — so every operation that ASSIGNS a name to a dashboard resolves the name here
    /// before it writes, rather than each one scanning the map itself. The comparison is
    /// exact: the model states no case folding, so two names that differ only in case are
    /// two names.
    pub fn dashboard_arn_by_name(&self, name: &str) -> Option<&str> {
        self.dashboards
            .values()
            .find(|dashboard| dashboard.name == name)
            .map(|dashboard| dashboard.arn.as_str())
    }

    /// The tags persisted on the resource `resource_arn` names, in the order they
    /// were stored, or `None` when this service holds no resource under that ARN.
    ///
    /// The ARN is the only key a tagging operation resolves a resource by, so every
    /// one of them resolves it here rather than reaching into the resource maps
    /// itself. `ResourceArn`'s pattern admits both resource types this service
    /// stores — `dashboard/...` and `scheduled-report/...` — so both are looked up
    /// here; an ARN neither map holds resolves to `None` and the caller answers it with
    /// `crate::scaffolded::Scaffolded::resource_not_found`.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&[ResourceTag]> {
        if let Some(dashboard) = self.dashboards.get(resource_arn) {
            return Some(dashboard.resource_tags.as_slice());
        }
        self.scheduled_reports
            .get(resource_arn)
            .map(|report| report.resource_tags.as_slice())
    }

    /// The tags persisted on the resource `resource_arn` names, for a handler that
    /// CHANGES them, or `None` when this service holds no resource under that ARN.
    ///
    /// Resolves the ARN exactly as [`Self::resource_tags`] does — both resource types
    /// `ResourceArn` admits, in the same order — so a tagging write and a tagging read
    /// always agree on which resource an ARN names and share one copy of its tags.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<ResourceTag>> {
        if let Some(dashboard) = self.dashboards.get_mut(resource_arn) {
            return Some(&mut dashboard.resource_tags);
        }
        self.scheduled_reports
            .get_mut(resource_arn)
            .map(|report| &mut report.resource_tags)
    }

    /// The resource-based policy document attached to the dashboard `resource_arn`
    /// names, or `None` when this service holds no dashboard under that ARN (the
    /// caller answers that with `crate::scaffolded::Scaffolded::resource_not_found`).
    ///
    /// A dashboard with no policy attached resolves to
    /// [`EMPTY_RESOURCE_POLICY_DOCUMENT`]: a missing policy is not a missing resource,
    /// and `policyDocument` reports a POLICY DOCUMENT, so the absence of one is the
    /// document that grants nothing rather than a value that is not a document at all.
    /// A stored document that holds no characters says the same thing, so it resolves
    /// the same way.
    pub fn dashboard_resource_policy(&self, resource_arn: &str) -> Option<&str> {
        self.dashboards.get(resource_arn).map(|dashboard| {
            dashboard
                .resource_policy
                .as_deref()
                .filter(|document| !document.is_empty())
                .unwrap_or(EMPTY_RESOURCE_POLICY_DOCUMENT)
        })
    }

    /// Mint a fresh widget id: the same [`next_resource_id`] rendering the ARN minters
    /// use, which is what `WidgetId`'s `^[0-9a-f]{32}$` pattern requires.
    pub fn mint_widget_id(&mut self) -> String {
        next_resource_id(&mut self.next_widget_seq)
    }

    /// Mint a fresh, unique scheduled-report ARN through the same [`mint_arn`] the
    /// dashboard minter uses, under the `scheduled-report` resource type — whose
    /// `ScheduledReportArn` pattern admits the same `[-a-z0-9]+` resource segment.
    pub fn mint_scheduled_report_arn(&mut self) -> String {
        mint_arn(
            &mut self.next_scheduled_report_seq,
            SCHEDULED_REPORT_RESOURCE_TYPE,
        )
    }

    /// The widgets to store for a create or update: the supplied widgets with an
    /// id minted for every one that arrived without one. `width`, `height` and
    /// `horizontalOffset` already carry the model's declared defaults (4, 7, 0),
    /// which the framework fills in before a handler runs.
    ///
    /// A minted id is one no widget of this list already carries: `Widget$id` is the unique
    /// identifier of a widget, which a client may supply itself, so a supplied id that the
    /// counter would have reached next is skipped rather than minted a second time — the
    /// same invariant [`widget_list_refusal`] holds the supplied ids to.
    pub fn adopt_widgets(&mut self, widgets: Vec<Widget>) -> Vec<Widget> {
        let mut taken: BTreeSet<String> = widgets
            .iter()
            .filter_map(|widget| widget.id.clone())
            .collect();
        widgets
            .into_iter()
            .map(|mut widget| {
                if widget.id.as_deref().is_none_or(str::is_empty) {
                    let mut minted = self.mint_widget_id();
                    while !taken.insert(minted.clone()) {
                        minted = self.mint_widget_id();
                    }
                    widget.id = Some(minted);
                }
                widget
            })
            .collect()
    }

    /// The reasons the resources `report`'s stored configuration points at do not resolve,
    /// in the order the model lists them (`smithy_query(query="shape",shape="StatusReason")`)
    /// and at most once each:
    ///
    /// * the execution role it names is one this service can no longer assume
    ///   ([`STATUS_REASON_EXECUTION_ROLE_ASSUME_FAILED`], as `role_faults` — the account's
    ///   IAM records — reports it),
    /// * or, where it can be assumed, one whose policies deny it the work an execution does
    ///   under it ([`STATUS_REASON_EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS`]) — the two name
    ///   different failures of one role, so at most one of them is reported for it,
    /// * the dashboard its `dashboardArn` names is no longer stored
    ///   ([`STATUS_REASON_DASHBOARD_NOT_FOUND`]), and
    /// * an id in its `widgetIds` is no longer a widget of that dashboard
    ///   ([`STATUS_REASON_WIDGET_ID_NOT_FOUND`]).
    ///
    /// Derived from what the stores hold AT THE CALL, never from a request and never from
    /// what an earlier refresh found: the role, the dashboard and the widgets a report
    /// points at can be deleted, replaced, re-pointed, re-trusted or re-permissioned after
    /// any refresh, so the reasons that name them are re-derived every time the health is
    /// resolved.
    ///
    /// The remaining `StatusReason` values name failures of resources neither store holds
    /// anything about (the cost data source, the dashboard's own access control), which is
    /// why nothing here claims them.
    fn unresolved_configuration_reasons(
        &self,
        report: &ScheduledReportRecord,
        role_faults: &ExecutionRoleFaults,
    ) -> Vec<String> {
        let mut status_reasons: Vec<String> = Vec::new();
        if role_faults.assume_failed(&report.execution_role_arn) {
            status_reasons.push(STATUS_REASON_EXECUTION_ROLE_ASSUME_FAILED.to_string());
        } else if role_faults.insufficient_permissions(&report.execution_role_arn) {
            status_reasons
                .push(STATUS_REASON_EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS.to_string());
        }
        match self.dashboards.get(&report.dashboard_arn) {
            None => status_reasons.push(STATUS_REASON_DASHBOARD_NOT_FOUND.to_string()),
            Some(dashboard) => {
                let present: BTreeSet<&str> = dashboard
                    .widgets
                    .iter()
                    .filter_map(|widget| widget.id.as_deref())
                    .collect();
                let names_a_missing_widget = report
                    .widget_ids
                    .iter()
                    .flatten()
                    .any(|id| !present.contains(id.as_str()));
                if names_a_missing_widget {
                    status_reasons.push(STATUS_REASON_WIDGET_ID_NOT_FOUND.to_string());
                }
            }
        }
        status_reasons
    }

    /// The health of `report` as this store establishes it at `now`, for the refresh that
    /// is running.
    ///
    /// The model documents `HEALTHY` as "the scheduled report is configured properly and
    /// has all required permissions to execute" and `UNHEALTHY` as the report being
    /// unable to deliver, with the reasons in `statusReasons`
    /// (`smithy_query(query="shape",shape="HealthStatus")`). So the status is DERIVED
    /// from what the stores hold, never from a request: a report is healthy exactly
    /// while every resource its configuration points at still resolves — the role it
    /// executes as included
    /// ([`Self::unresolved_configuration_reasons`]).
    ///
    /// `lastRefreshedAt` is `now` because this call is the refresh.
    pub fn scheduled_report_health(
        &self,
        report: &ScheduledReportRecord,
        role_faults: &ExecutionRoleFaults,
        now: SystemTime,
    ) -> HealthStatus {
        let status_reasons = self.unresolved_configuration_reasons(report, role_faults);
        // `lastRefreshedAt` is `now`: this call IS the refresh.
        health_status(status_reasons, Some(::aws_smithy_types::DateTime::from(now)))
    }

    /// The health EVERY view of `report` reports: `HealthStatus$statusCode` with the
    /// reasons behind it, resolved from this store.
    ///
    /// Health is a property of the RESOURCE and of the resources it points at, so it is
    /// resolved here — once, from the whole store — rather than projected out of the
    /// record by each view: no two views of one report can disagree about it, and none
    /// reports a status the store itself contradicts.
    ///
    /// * The reasons that name resources a store holds are re-derived from them
    ///   ([`Self::unresolved_configuration_reasons`]), so a report whose dashboard was
    ///   deleted — or whose named widget was removed from it, or whose execution role the
    ///   account's IAM no longer lets this service assume or use — is reported `UNHEALTHY`
    ///   with those reasons whether or not anything has ever refreshed the record, and
    ///   reported healthy again as soon as the resource resolves once more.
    /// * A reason the last refresh established that this store cannot re-derive (a
    ///   notification the account's default event bus did not take,
    ///   [`STATUS_REASON_INTERNAL_FAILURE`]) is carried forward from the record: the
    ///   refresh is the only evidence of it.
    /// * `lastRefreshedAt` is documented as "the timestamp when the health status was
    ///   last refreshed", so it is the instant of the record's last refresh and stays
    ///   absent until one has happened — resolving the health for a reader is not a
    ///   refresh, and this call writes nothing.
    ///
    /// A record with no refresh and every resource resolving is therefore
    /// [`HEALTH_STATUS_HEALTHY`] with no reasons and no refresh instant — the model's
    /// "configured properly and has all required permissions to execute", which is what
    /// a report this service accepted and whose resources still resolve is.
    pub fn reported_scheduled_report_health(
        &self,
        report: &ScheduledReportRecord,
        role_faults: &ExecutionRoleFaults,
    ) -> HealthStatus {
        let mut status_reasons = self.unresolved_configuration_reasons(report, role_faults);
        let refreshed = report.health_status.as_ref();
        let refreshed_reasons = refreshed
            .and_then(|health| health.status_reasons.as_ref())
            .into_iter()
            .flatten();
        for reason in refreshed_reasons {
            if !DERIVED_STATUS_REASONS.contains(&reason.as_str())
                && !status_reasons.contains(reason)
            {
                status_reasons.push(reason.clone());
            }
        }

        health_status(
            status_reasons,
            refreshed.and_then(|health| health.last_refreshed_at),
        )
    }

    /// `report` projected onto the modelled `ScheduledReport` the detail readers answer,
    /// with the health this store resolves for it.
    pub fn scheduled_report_detail(
        &self,
        report: &ScheduledReportRecord,
        role_faults: &ExecutionRoleFaults,
    ) -> ScheduledReport {
        report.report(self.reported_scheduled_report_health(report, role_faults))
    }

    /// `report` projected onto the modelled `ScheduledReportSummary` the listing pages
    /// over, with the health this store resolves for it — the SAME resolution the detail
    /// view gets.
    pub fn scheduled_report_summary(
        &self,
        report: &ScheduledReportRecord,
        role_faults: &ExecutionRoleFaults,
    ) -> ScheduledReportSummary {
        report.summary(self.reported_scheduled_report_health(report, role_faults))
    }

    /// Drop every settled create claim whose scheduled report this service no longer
    /// stores.
    ///
    /// A settled claim exists to refuse a later create presenting its token with the ARN
    /// of the report that token already created; once that report is gone there is no
    /// such report to name, so the claim would otherwise refuse a create in the name of
    /// an ARN nothing resolves to. The removal of the report is what makes the claim due
    /// for dropping — it was owed at that moment, whether or not anybody called
    /// afterwards — and dropping it is also what bounds the map by the reports actually
    /// stored ([`MAX_SCHEDULED_REPORTS`]) rather than by every token ever presented.
    ///
    /// A claim that is still IN FLIGHT names no report yet, so it has nothing removed to
    /// forget: it is held until the create holding it settles or releases it.
    pub fn forget_creations_of_removed_reports(&mut self) {
        let reports = &self.scheduled_reports;
        self.scheduled_report_client_tokens
            .retain(|_, creation| {
                creation
                    .created_arn()
                    .is_none_or(|arn| reports.contains_key(arn))
            });
    }

    /// Claim `token` for the `CreateScheduledReport` call that is about to create, or —
    /// when another create already claimed it — answer that claim and leave it alone.
    ///
    /// The lookup and the claim are ONE operation under the caller's single guard, which
    /// is what makes the token suppress a duplicate that is IN FLIGHT and not only one
    /// that arrives after the report is stored: the claim is taken before the report
    /// exists and the store is not held from the claim to the write, so a token recorded
    /// only once the report was stored would leave a window in which a duplicate finds
    /// the token free and creates a second report. Of several calls presenting one token
    /// exactly one is therefore answered `None` and goes on to create; every other is
    /// answered the claim it found, whose state words the refusal it is given
    /// ([`ScheduledReportCreation::conflict_message`]).
    pub fn reserve_scheduled_report_creation(
        &mut self,
        token: String,
    ) -> Option<ScheduledReportCreation> {
        match self.scheduled_report_client_tokens.entry(token) {
            Entry::Occupied(claimed) => Some(claimed.get().clone()),
            Entry::Vacant(free) => {
                free.insert(ScheduledReportCreation::InFlight);
                None
            }
        }
    }

    /// Settle the claim `token` holds with `arn`, the report the call holding it stored:
    /// from here every call presenting that token is refused in the name of that report.
    ///
    /// Only an unsettled claim is written: a claim another call settled under the same
    /// token — which it can only have taken once this one released its own — is left as
    /// that call settled it.
    pub fn settle_scheduled_report_creation(&mut self, token: &str, arn: &str) {
        if let Some(claim) = self.scheduled_report_client_tokens.get_mut(token) {
            if claim.created_arn().is_none() {
                *claim = ScheduledReportCreation::Created {
                    arn: arn.to_string(),
                };
            }
        }
    }

    /// Release the IN-FLIGHT claim `token` holds, because the call that took it was
    /// refused and stored nothing: a refused request creates no report, so its token
    /// stays free for the request that corrects it. A settled claim is left alone — the
    /// report it names exists, and the token that created it is spent.
    pub fn release_scheduled_report_creation(&mut self, token: &str) {
        if self
            .scheduled_report_client_tokens
            .get(token)
            .is_some_and(|claim| claim.created_arn().is_none())
        {
            self.scheduled_report_client_tokens.remove(token);
        }
    }

    /// Drop every execution idempotency record whose [`EXECUTION_IDEMPOTENCY_WINDOW`]
    /// the clock has already closed at `now`.
    ///
    /// This is the one write an `ExecuteScheduledReport` call makes before it decides
    /// anything: the expiry was owed at `now` whether or not anybody called, so
    /// materialising it changes nothing a later caller could otherwise have seen — it
    /// only stops an expired token from being read as a retry.
    pub fn expire_execution_tokens(&mut self, now: SystemTime) {
        self.scheduled_report_executions.retain(|_, execution| {
            // A clock that went backwards yields `Err` here; treat that as "no time has
            // passed" and keep the record rather than dropping it early.
            now.duration_since(execution.executed_at)
                .unwrap_or_default()
                < EXECUTION_IDEMPOTENCY_WINDOW
        });
    }

    /// The outcome already recorded under `key`, or — when nothing is recorded under it —
    /// `None`, with `reservation` recorded under it as this call's outcome.
    ///
    /// The lookup and the write are ONE operation under the caller's single guard, which
    /// is what makes an idempotency token suppress a duplicate that is IN FLIGHT and not
    /// only one that arrives afterwards: an execution reaches the event bus with the store
    /// UNLOCKED (a lock is not held across an await), so a token recorded only once that
    /// delivery has answered leaves a window in which a duplicate finds nothing recorded,
    /// triggers its own execution and delivers its own notification. Of several calls
    /// presenting one token for one report exactly one is therefore answered `None` and
    /// goes on to execute; every other is answered the outcome that call reserved.
    pub fn reserve_execution_token(
        &mut self,
        key: (String, String),
        reservation: ScheduledReportExecution,
    ) -> Option<ScheduledReportExecution> {
        match self.scheduled_report_executions.entry(key) {
            Entry::Occupied(recorded) => Some(recorded.get().clone()),
            Entry::Vacant(free) => {
                free.insert(reservation);
                None
            }
        }
    }

    /// Settle the reservation `key` holds for the call that took it at `executed_at` with
    /// `health_status` — the health that call's delivery established — so a later retry
    /// replays the outcome the call ended with rather than the one it was committed to
    /// before the delivery.
    ///
    /// Only that call's own reservation is settled: a record another call wrote under the
    /// same key once this one's window had closed is left as that call recorded it, and a
    /// reservation the expiry has already dropped is not written back.
    pub fn settle_execution_token(
        &mut self,
        key: &(String, String),
        executed_at: SystemTime,
        health_status: &HealthStatus,
    ) {
        if let Some(reserved) = self.scheduled_report_executions.get_mut(key) {
            if reserved.executed_at == executed_at {
                reserved.health_status = health_status.clone();
            }
        }
    }
}

/// The tracing target every audit record below is emitted on, so a host can route or
/// filter the audit stream on its own. Keep it equal to the `target:` literal in
/// [`record_state_change`] — the macro needs a literal there.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the audit record a state-changing operation owes: when it happened, where the
/// request came from, which access key id it presented, which operation ran, which
/// resource it touched and how it came out.
///
/// Lives here rather than in one operation's file because every state-changing
/// dashboard operation emits the same record. Only the presented access key id is
/// recorded — never a secret, and never the emulator's own credentials. Reporting
/// operations do not call this: they change nothing to account for.
pub fn record_state_change(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    outcome: &str,
) {
    let headers = &ctx.parts().headers;
    let source = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    let timestamp_ms = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| {
            u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
        });
    ::tracing::info!(
        target: "emulator::audit",
        timestamp_ms,
        source,
        akid = ::engine::extract_akid(headers),
        operation,
        resource,
        outcome,
        "state-changing bcm-dashboards operation"
    );
}

/// How every operation that ASSIGNS a dashboard name words the refusal of a name the
/// account already holds.
///
/// Built from the request, the way the boundary validator words a refusal: it names the
/// member that broke the constraint, the value it carried and the constraint itself. One
/// helper so `CreateDashboard` and `UpdateDashboard` cannot drift apart on the wording of
/// one rule.
pub fn duplicate_dashboard_name_message(name: &str) -> String {
    format!(
        "name: a dashboard named '{name}' already exists in this account; \
         the name of a dashboard must be unique within your account"
    )
}

/// Whether `key` is a tag key Amazon Web Services reserves for itself (see
/// [`RESERVED_TAG_KEY_PREFIX`]).
///
/// `str::get` rather than a byte-range slice (`key[..4]`), which PANICS when the key
/// carries a multi-byte character straddling that byte index — `ResourceTagKey`'s
/// modelled pattern is `^[\S\s]*$`, which admits any character at all, so such a key
/// passes boundary validation and reaches this helper. `get` answers `None` for a key
/// shorter than the prefix or one whose boundary falls mid-character, which is exactly
/// "not reserved".
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `message` within [`MAX_MESSAGE_LENGTH`] characters: itself when it already fits, else
/// its longest prefix that leaves room for [`MESSAGE_ELISION`] — cut between characters,
/// never inside one — followed by that marker.
///
/// The last step of every message this service builds by quoting a value the request
/// carried, because those members admit far more text than the message member that would
/// have to hold it (see [`MAX_MESSAGE_LENGTH`]). Characters, not bytes: `@length` on a
/// string counts characters, which is what the boundary validator charges a value by.
pub fn within_message_bound(message: impl Into<String>) -> String {
    elided_within(&message.into(), MAX_MESSAGE_LENGTH)
}

/// `text` within `budget` characters: itself when it already fits, else its longest prefix
/// that leaves room for [`MESSAGE_ELISION`] — cut between characters, never inside one —
/// followed by that marker.
///
/// [`within_message_bound`] applied to the WHOLE message is this with the message member's
/// own maximum as the budget; a message that quotes a request value in the MIDDLE of its
/// clause applies it to that value instead, with the room the clause leaves as the budget,
/// so the rule the caller has to read survives the bound rather than being cut off after
/// the value. The result never exceeds `budget`: a budget with no room for the marker
/// itself keeps no text at all.
fn elided_within(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let marker = MESSAGE_ELISION.chars().count();
    let mut bounded: String = text.chars().take(budget.saturating_sub(marker)).collect();
    if budget >= marker {
        bounded.push_str(MESSAGE_ELISION);
    }
    bounded
}

/// How a message accounts for the `omitted` values a bound left no room for — nothing at
/// all when it listed every one of them.
fn omitted_values_notice(omitted: usize) -> String {
    if omitted == 0 {
        return String::new();
    }
    format!(" and {omitted} more")
}

/// `values` quoted into a message that has `budget` characters left for them: as many as
/// fit, joined with [`MESSAGE_LIST_SEPARATOR`], and a COUNT of the ones that do not.
///
/// A message quoting a modelled list has to stay inside the bound of the member carrying
/// it however many entries the request named, and the two ways to get there are not
/// equal: cutting the rendered text would end the message inside a value, which reads as a
/// value the caller never sent, while stopping at a whole value and counting the rest
/// leaves every quoted value exact and still says how many more the request carried. The
/// room the closing count needs is reserved before each value is admitted, so the result
/// never exceeds `budget` — with every value quoted when the whole list fits.
fn values_within(values: &[&str], budget: usize) -> String {
    let mut quoted = String::new();
    let mut listed = 0usize;
    for (index, value) in values.iter().enumerate() {
        let separator = if index == 0 { "" } else { MESSAGE_LIST_SEPARATOR };
        // What this function would close with if the NEXT value did not fit, which is
        // exactly the notice it does close with when it stops at that value.
        let notice = omitted_values_notice(values.len() - index - 1);
        let needed = quoted.chars().count()
            + separator.chars().count()
            + value.chars().count()
            + notice.chars().count();
        if needed > budget {
            break;
        }
        quoted.push_str(separator);
        quoted.push_str(value);
        listed += 1;
    }
    let notice = omitted_values_notice(values.len() - listed);
    if quoted.is_empty() {
        // No value fit at all, so the count is the whole of what this function has to
        // say and it is rendered without the separating space it would have carried
        // after a value. Out of reach of either caller today — the clause each of them
        // builds leaves room for several values at their modelled maximum length — and
        // kept correct rather than left to produce a sentence with a hole in it.
        return notice.trim_start().to_string();
    }
    quoted.push_str(&notice);
    quoted
}

/// `Ok(())` when no key in `keys` is in the reserved namespace, or the refusal message
/// for the ones that are — the text a `ValidationException` carries.
///
/// One helper so every operation that takes caller-supplied tag keys —
/// `CreateDashboard` and `CreateScheduledReport` at create time, `TagResource` and
/// `UntagResource` afterwards — refuses that one namespace the same way and cannot drift
/// apart. The message is built from the request, the way the boundary validator words a
/// refusal: it names the member that broke the rule, the offending keys and the rule
/// itself. The offenders are sorted and deduplicated so one request that names a key
/// twice reports it once and the text does not depend on iteration order.
///
/// The keys are quoted within what [`MAX_MESSAGE_LENGTH`] leaves after the clause itself
/// ([`values_within`]), because the lists this rule is applied to are bounded by their own
/// models and not by the message member's: `resourceTags` and `resourceTagKeys` each carry
/// up to 200 keys of up to 128 characters, so quoting all of them unconditionally puts a
/// message on the wire some twenty-five times the maximum the model declares for it.
pub fn check_tag_keys_not_reserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    let clause = format!(
        "{member}: the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved for tags Amazon Web \
         Services generates and cannot be set or removed by a caller, so these keys are not \
         allowed: "
    );
    let budget = MAX_MESSAGE_LENGTH.saturating_sub(clause.chars().count());
    Err(within_message_bound(format!(
        "{clause}{}",
        values_within(&reserved, budget)
    )))
}

/// The tag list to STORE when `incoming` is applied to the resource that currently
/// carries `stored`: the tags resolved by tag key, so one key holds one value.
///
/// A resource's tags are a map keyed by tag key, while `ResourceTagList` is a plain
/// list the model lets repeat a key (length 0..200, no `@uniqueItems`,
/// `smithy_query(query="shape",shape="ResourceTagList")`), so every path that accepts
/// caller-supplied tags resolves that list the same way here rather than each one
/// deciding for itself:
///
/// * a key the resource already carries takes the applied value IN PLACE, keeping its
///   position in the stored list, so the order a tagging read reports stays stable;
/// * a key it does not carry yet is appended in the order `incoming` first names it;
/// * a key the request repeats settles on its LAST value — the same answer as applying
///   those entries one request at a time;
/// * a key `incoming` does not name keeps its stored value.
///
/// Create-time tagging passes an empty `stored`, which is the same rule with nothing to
/// update: the request's own repeated keys are resolved, so no resource ever enters the
/// store carrying one key twice — a state no later tagging call could produce, and one
/// that would make a read report two values for one key and an update reach only the
/// first copy.
pub fn merge_resource_tags(
    stored: Vec<ResourceTag>,
    incoming: impl IntoIterator<Item = ResourceTag>,
) -> Vec<ResourceTag> {
    let mut merged = stored;
    for tag in incoming {
        match merged.iter_mut().find(|stored| stored.key == tag.key) {
            Some(stored) => stored.value = tag.value,
            None => merged.push(tag),
        }
    }
    merged
}

/// The `ScheduleState` a stored `scheduleConfig` is in: the value it carries, or
/// [`SCHEDULE_STATE_ENABLED`] for a config that carries none. `state` is resolved at
/// write time, so the fallback only covers a config written without one; keeping the
/// fallback in ONE place is what lets every reader, every writer and
/// `ExecuteScheduledReport` agree on whether a schedule is enabled.
pub fn schedule_config_state(config: &ScheduleConfig) -> &str {
    config
        .state
        .as_deref()
        .filter(|state| !state.is_empty())
        .unwrap_or(SCHEDULE_STATE_ENABLED)
}

/// The `schedulePeriod` to STORE for a write made at `now`: the submitted period with
/// each bound its own member documents a service-side default for resolved to that
/// default, so the period a reader gets back is the one the schedule actually runs over.
///
/// Both bounds of `SchedulePeriod` document one — "The start time of the schedule period.
/// If not specified, defaults to the time of the create or update request" and "The end
/// time of the schedule period. If not specified, defaults to 3 years from the time of the
/// create or update request" (`smithy_query(query="shape",shape="SchedulePeriod")`) — and a
/// documented service-side default is resolved by the write that stores the value, not
/// guessed by each reader: a member a request left unset reads back carrying the value the
/// service chose for it.
///
/// A period the request omits ENTIRELY is the same case: every bound of it is unspecified,
/// so the stored schedule carries the period both defaults spell out. The end default is
/// the same instant as the ceiling the same member states on a SUBMITTED end time
/// ([`schedule_period_end_time_ceiling`]) — "3 years from the time of the create or update
/// request" and "3 years from the current time" are one instant — so the period this
/// resolves is one the rule admits.
pub fn resolved_schedule_period(submitted: Option<SchedulePeriod>, now: SystemTime) -> SchedulePeriod {
    let submitted = submitted.unwrap_or_default();
    SchedulePeriod {
        start_time: Some(
            submitted
                .start_time
                .unwrap_or_else(|| ::aws_smithy_types::DateTime::from(now)),
        ),
        end_time: Some(submitted.end_time.unwrap_or_else(|| {
            ::aws_smithy_types::DateTime::from(schedule_period_end_time_ceiling(now))
        })),
    }
}

/// The `scheduleConfig` to store for a CREATE made at `now`: the supplied config with
/// every member that carries a documented service-side default resolved to it, so every
/// reader sees a schedule whose values are decided once, at write time, rather than each
/// reader guessing.
///
/// Two members document one:
///
/// * `state` defaults to [`SCHEDULE_STATE_ENABLED`] — `ScheduledReportInput$scheduleConfig`
///   is the member documented "If the schedule state is not specified, it defaults to
///   ENABLED" (`smithy_query(query="shape",shape="ScheduledReportInput")`), which belongs to
///   the create input alone, so an update resolves its config through
///   [`updated_schedule_config`] instead;
/// * `schedulePeriod`'s two bounds default to the time of this request and to 3 years on
///   from it ([`resolved_schedule_period`]), a sentence written for the create request and
///   the update request alike, so both write paths resolve the period the same way.
pub fn resolve_schedule_config(mut config: ScheduleConfig, now: SystemTime) -> ScheduleConfig {
    config.state = Some(schedule_config_state(&config).to_string());
    config.schedule_period = Some(resolved_schedule_period(config.schedule_period, now));
    config
}

/// The `scheduleConfig` to store for an UPDATE of a report whose stored config is
/// `stored`: the STORED config with each property the submitted one names written over
/// it, property by property.
///
/// An update applies only the properties its request names — "Only the parameters
/// included in the request are updated; all other properties remain unchanged"
/// (`smithy_query(query="doc",shape="UpdateScheduledReport")`) — and a property of a
/// submitted STRUCTURE is named only when that structure carries it, so a submitted
/// `scheduleConfig` is merged into the stored one rather than swapped for it: the
/// expression, the time zone, the active period and the state each move only when the
/// request carries that member. This service spells out how a caller unsets or re-defaults
/// an optional property — an explicit flag on the request (`clearWidgetIds`,
/// `clearWidgetDateRangeOverride`) — and `UpdateScheduledReportRequest` models no such flag
/// for any member of `ScheduleConfig` (`smithy_query(query="operation",
/// operation="UpdateScheduledReport")`), so an absent member is never an instruction to
/// drop or re-default a stored one: a caller re-cronning a report keeps the time zone and
/// the period it configured, and cannot resume a report it paused
/// ([`SCHEDULE_STATE_ENABLED`] is documented on the CREATE member alone,
/// [`resolve_schedule_config`]).
///
/// The merge stops at the members of `ScheduleConfig`: inside a submitted
/// `schedulePeriod`, an omitted bound is not "unchanged" but the default its own member
/// documents ("defaults to the time of the create or update request" / "defaults to 3 years
/// from the time of the create or update request",
/// `smithy_query(query="shape",shape="SchedulePeriod")`), so a period the request carries
/// is one whole parameter: it replaces the stored period, with its own bounds resolved at
/// `now`, the instant of THIS update ([`resolved_schedule_period`]).
pub fn updated_schedule_config(
    stored: &ScheduleConfig,
    submitted: ScheduleConfig,
    now: SystemTime,
) -> ScheduleConfig {
    // Destructured rather than mutated, so a member added to `ScheduleConfig` later
    // cannot slip through this merge unconsidered: the build stops here until it is.
    let ScheduleConfig {
        schedule_expression,
        schedule_expression_time_zone,
        schedule_period,
        state,
    } = submitted;
    ScheduleConfig {
        schedule_expression: schedule_expression.or_else(|| stored.schedule_expression.clone()),
        schedule_expression_time_zone: schedule_expression_time_zone
            .or_else(|| stored.schedule_expression_time_zone.clone()),
        schedule_period: schedule_period
            .map(|period| resolved_schedule_period(Some(period), now))
            .or_else(|| stored.schedule_period.clone()),
        // `state` is the one member whose target is an enum, so an empty string is no
        // value of it at all and counts as unnamed; the other three are strings and a
        // structure the model admits as submitted.
        state: Some(
            state
                .filter(|state| !state.is_empty())
                .unwrap_or_else(|| schedule_config_state(stored).to_string()),
        ),
    }
}

/// How far past the write request that submits it the END of a schedule period may reach,
/// in calendar months.
///
/// `SchedulePeriod$endTime` is documented "The maximum allowed value is 3 years from the
/// current time. Setting an end time beyond this limit returns a `ValidationException`"
/// (`smithy_query(query="shape",shape="SchedulePeriod")`), and that sentence is written for
/// both write paths — the member it is on defaults to "3 years from the time of the create
/// or update request" — so every write that submits a period is held to it.
pub const SCHEDULE_PERIOD_MAX_END_TIME_MONTHS: u32 = 36;

/// The latest `endTime` a schedule period submitted at `now` may carry:
/// [`SCHEDULE_PERIOD_MAX_END_TIME_MONTHS`] calendar months on from `now`.
///
/// Calendar arithmetic rather than a span of seconds, because the ceiling is stated in
/// years and years differ in length — a span of 365-day years would refuse an end time the
/// service admits whenever a leap day falls inside the window. The 29th of February maps
/// to the 28th in a year that has no 29th, which is what `chrono`'s month arithmetic does.
pub fn schedule_period_end_time_ceiling(now: SystemTime) -> SystemTime {
    let now_utc = ::chrono::DateTime::<::chrono::Utc>::from(now);
    let months = ::chrono::Months::new(SCHEDULE_PERIOD_MAX_END_TIME_MONTHS);
    SystemTime::from(now_utc.checked_add_months(months).unwrap_or(now_utc))
}

/// Render the instant `value` as the RFC 3339 instant it spells, or as its epoch seconds
/// for a value no calendar rendering can hold.
///
/// The one instant rendering of this crate: what a refusal names a submitted instant with,
/// and what an event's `detail` carries ([`rfc3339`]).
fn instant_text(value: &::aws_smithy_types::DateTime) -> String {
    value
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| value.secs().to_string())
}

/// How far BEFORE the write request that submits it the START of a schedule period may
/// sit.
///
/// `SchedulePeriod$startTime` is documented "The start time cannot be more than 5 minutes
/// before the time of the request" (`smithy_query(query="shape",shape="SchedulePeriod")`),
/// and that sentence, like the ceiling on the period's end, is written for both write paths
/// — the member it is on defaults to "the time of the create or update request".
pub const SCHEDULE_PERIOD_MAX_START_TIME_LAG: Duration = Duration::from_secs(5 * 60);

/// The earliest `startTime` a schedule period submitted at `now` may carry:
/// [`SCHEDULE_PERIOD_MAX_START_TIME_LAG`] before `now`.
pub fn schedule_period_earliest_start_time(now: SystemTime) -> SystemTime {
    now.checked_sub(SCHEDULE_PERIOD_MAX_START_TIME_LAG)
        .unwrap_or(now)
}

/// Whether the instant `earlier` is at or before the instant `later`.
fn at_or_before(
    earlier: &::aws_smithy_types::DateTime,
    later: &::aws_smithy_types::DateTime,
) -> bool {
    (earlier.secs(), earlier.subsec_nanos()) <= (later.secs(), later.subsec_nanos())
}

/// How this service refuses the `schedulePeriod` of the `scheduleConfig` a write submits at
/// the member path `path`, made at `now`, or `None` when the period is one it can store.
///
/// The two rules the model states on the period's bounds, each measured from the request
/// that submits it:
///
/// * the `startTime` may be at most [`SCHEDULE_PERIOD_MAX_START_TIME_LAG`] before it
///   ([`schedule_period_earliest_start_time`]) — "The start time cannot be more than 5
///   minutes before the time of the request";
/// * the `endTime` may be at most [`SCHEDULE_PERIOD_MAX_END_TIME_MONTHS`] calendar months
///   past it ([`schedule_period_end_time_ceiling`]) — "The maximum allowed value is 3 years
///   from the current time. Setting an end time beyond this limit returns a
///   `ValidationException`".
///
/// Each is written for the create request and the update request alike, and a period
/// outside either is refused with the modelled `ValidationException`. Both bounds
/// themselves are admitted: one is the maximum ALLOWED value and the other the earliest a
/// start time may sit. A period that names a bound not at all breaks neither rule — the
/// member is optional, and an absent one is the documented default this service resolves at
/// write time ([`resolved_schedule_period`]) rather than a value to check. A request that
/// breaks both is refused for its `startTime`, the first member of the structure.
///
/// One helper for every write that submits a schedule, so the create path and the update
/// path cannot drift apart on the rules. The refusal quotes the instant the request carried
/// within the maximum `ValidationException$message` declares ([`MAX_MESSAGE_LENGTH`]).
pub fn schedule_period_refusal(
    path: &str,
    config: &ScheduleConfig,
    now: SystemTime,
) -> Option<String> {
    let period = config.schedule_period.as_ref()?;
    if let Some(start_time) = period.start_time.as_ref() {
        let earliest =
            ::aws_smithy_types::DateTime::from(schedule_period_earliest_start_time(now));
        if !at_or_before(&earliest, start_time) {
            return Some(within_message_bound(format!(
                "{path}.schedulePeriod.startTime: '{}' is before the earliest allowed value; \
                 the start time of a schedule period may be at most 5 minutes before the time \
                 of the request",
                instant_text(start_time)
            )));
        }
    }
    let end_time = period.end_time.as_ref()?;
    let ceiling = ::aws_smithy_types::DateTime::from(schedule_period_end_time_ceiling(now));
    if at_or_before(end_time, &ceiling) {
        return None;
    }
    Some(within_message_bound(format!(
        "{path}.schedulePeriod.endTime: '{}' is beyond the maximum allowed value; the end \
         time of a schedule period may be at most 3 years from the time of the request",
        instant_text(end_time)
    )))
}

/// How a schedule expression this service stores OPENS, as the member that carries it
/// writes the form: `ScheduleConfig$scheduleExpression` is documented "This value must be a
/// cron expression consisting of six fields separated by white spaces:
/// `cron(minutes hours day_of_month month day_of_week year)`"
/// (`smithy_query(query="shape",shape="ScheduleConfig")`).
const SCHEDULE_EXPRESSION_CRON_PREFIX: &str = "cron(";

/// How that same expression CLOSES.
const SCHEDULE_EXPRESSION_CRON_SUFFIX: &str = ")";

/// How many white-space separated fields the documented form carries between those two:
/// the minutes, the hours, the day of the month, the month, the day of the week and the
/// year.
pub const SCHEDULE_EXPRESSION_CRON_FIELDS: usize = 6;

/// Whether `expression` is spelled the way the member carrying it documents: the
/// [`SCHEDULE_EXPRESSION_CRON_PREFIX`] wrapper around exactly
/// [`SCHEDULE_EXPRESSION_CRON_FIELDS`] fields separated by white space.
///
/// The fields are counted as the sentence words them — separated by white space — so a run
/// of blanks separates two fields rather than naming an empty one, and nothing here reads
/// INSIDE a field: which minutes, days and months a field may name is the cron dialect's
/// own business, which this service neither interprets nor runs.
fn names_a_cron_schedule(expression: &str) -> bool {
    expression
        .strip_prefix(SCHEDULE_EXPRESSION_CRON_PREFIX)
        .and_then(|rest| rest.strip_suffix(SCHEDULE_EXPRESSION_CRON_SUFFIX))
        .is_some_and(|fields| {
            fields.split_whitespace().count() == SCHEDULE_EXPRESSION_CRON_FIELDS
        })
}

/// How this service refuses the `scheduleExpression` of the `scheduleConfig` a write
/// submits at the member path `path`, or `None` when that expression is one it can store.
///
/// The member states the form of its own value — "This value must be a cron expression
/// consisting of six fields separated by white spaces:
/// `cron(minutes hours day_of_month month day_of_week year)`"
/// (`smithy_query(query="shape",shape="ScheduleConfig")`) — and the modelled shape behind
/// it is an unconstrained `GenericString`, so nothing at the boundary can hold a request to
/// that sentence: it is applied here, on every write that submits a schedule, with the
/// modelled `ValidationException`. A config that names no expression at all breaks no rule:
/// the member is optional, and an absent one is no value to hold to a form.
///
/// One helper for the create path and the update path alike, so the form a stored schedule
/// is in cannot depend on which call wrote it — a report could otherwise be created under
/// the rule and then updated out of it.
///
/// The refusal quotes the expression the request carried within the room the clause leaves
/// inside the maximum the member that has to carry the message declares
/// ([`MAX_MESSAGE_LENGTH`]), which the quoted value alone may reach.
pub fn schedule_expression_refusal(path: &str, config: &ScheduleConfig) -> Option<String> {
    let expression = config.schedule_expression.as_deref()?;
    if names_a_cron_schedule(expression) {
        return None;
    }
    let worded = |expression: &str| {
        format!(
            "{path}.scheduleExpression: '{expression}' is not a cron expression; the schedule \
             expression must be a cron expression consisting of six fields separated by white \
             spaces: cron(minutes hours day_of_month month day_of_week year)"
        )
    };
    let budget = MAX_MESSAGE_LENGTH.saturating_sub(worded("").chars().count());
    Some(within_message_bound(worded(&elided_within(
        expression, budget,
    ))))
}

/// How this service refuses the `widgetIds` a write submits at the member path `path` for a
/// report on `dashboard`, or `None` when every id it names is a widget of that dashboard.
///
/// `widgetIds` is documented "The list of widget identifiers to include in the scheduled
/// report. If not specified, all widgets in the dashboard are included"
/// (`smithy_query(query="shape",shape="ScheduledReportInput")`), so an id in it identifies
/// a widget OF the dashboard the report is rendered from: a report including a widget that
/// dashboard does not hold is a request for something the service cannot render, and it is
/// refused with the modelled `ValidationException` rather than stored. The empty list names
/// no widget, so it breaks no rule, and neither does an absent member — the documented "all
/// widgets in the dashboard" case.
///
/// Only the ids a REQUEST submits are held to this: the widgets of a stored dashboard are
/// replaced and deleted after the write that named them, which is what the derived
/// `WIDGET_ID_NOT_FOUND` health reason reports
/// ([`ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::unresolved_configuration_reasons`]),
/// and no rule applied to a submitted member can be a rule about a widget that went away
/// later.
///
/// The offenders are sorted and deduplicated, so the text does not depend on iteration
/// order, and quoted within the room the clause leaves inside the maximum the message
/// member declares ([`values_within`]).
pub fn widget_ids_refusal(
    path: &str,
    dashboard: &Dashboard,
    widget_ids: &[String],
) -> Option<String> {
    let present: BTreeSet<&str> = dashboard
        .widgets
        .iter()
        .filter_map(|widget| widget.id.as_deref())
        .collect();
    let mut unknown: Vec<&str> = widget_ids
        .iter()
        .map(String::as_str)
        .filter(|id| !present.contains(id))
        .collect();
    if unknown.is_empty() {
        return None;
    }
    unknown.sort_unstable();
    unknown.dedup();
    let worded = |dashboard_arn: &str, ids: &str| {
        format!(
            "{path}: the dashboard '{dashboard_arn}' has no widget with these identifiers: \
             {ids}"
        )
    };
    // A `DashboardArn` reaches 2048 characters where a `WidgetId` is 32 of them, so the ARN
    // is held to half the room the clause leaves and the ids get what it does not spend.
    let room = MAX_MESSAGE_LENGTH.saturating_sub(worded("", "").chars().count());
    let dashboard_arn = elided_within(&dashboard.arn, room / 2);
    let budget = room.saturating_sub(dashboard_arn.chars().count());
    Some(within_message_bound(worded(
        &dashboard_arn,
        &values_within(&unknown, budget),
    )))
}

/// The `@pattern` of `DashboardName` and `ScheduledReportName`, as the model writes it
/// (`smithy_query(query="shape",shape="DashboardName,ScheduledReportName")`): both shapes
/// declare this one expression.
pub const RESOURCE_NAME_PATTERN: &str = r"^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$";

/// The `@pattern` of `Description`, as the model writes it
/// (`smithy_query(query="shape",shape="Description")`) — the shape every `description`
/// member of this service targets, on a dashboard, a widget and a scheduled report alike.
pub const DESCRIPTION_PATTERN: &str = r"^(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*$";

/// The `@pattern` of `WidgetTitle`, as the model writes it
/// (`smithy_query(query="shape",shape="WidgetTitle")`).
pub const WIDGET_TITLE_PATTERN: &str =
    r"^(?!.* {2})[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]$";

/// A constrained string of this service whose modelled `@pattern` the boundary validator
/// does NOT apply, so the request reaches a handler unchecked against it.
///
/// Each of these expressions opens with the negative lookahead `(?!.* {2})` — no two
/// consecutive spaces anywhere in the value — and the boundary's regex engine compiles no
/// lookahead; a pattern it cannot compile is treated as matching
/// (`aws_smithy_schema_server::constraints`'s pattern check). So the three grammars are
/// applied HERE, by the same clause and envelope the boundary states for a pattern it can
/// compile ([`text_pattern_refusal`]), before the handler writes anything. The `@length`
/// bounds on the same members compile and are already enforced at the boundary, so they
/// are not repeated — which is also what bounds the value a clause echoes.
///
/// What is applied is the modelled expression itself, compiled by a look-around-capable
/// engine ([`COMPILED_PATTERNS`]), so each rule is stated once: the text a refusal quotes
/// is the text that judged the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstrainedText {
    /// A resource name: [`RESOURCE_NAME_PATTERN`].
    ResourceName,
    /// A description: [`DESCRIPTION_PATTERN`].
    Description,
    /// A widget title: [`WIDGET_TITLE_PATTERN`].
    WidgetTitle,
}

/// The step budget one match of a [`ConstrainedText`] grammar may spend.
///
/// The engine these expressions need is a BACKTRACKING one, so a match has no
/// linear-time guarantee in general and the work one match may do is bounded explicitly
/// here rather than left to the shape of an expression. Two properties of the inputs put
/// this budget out of reach: the expressions are MODEL CONSTANTS, never built from a
/// request, and the matched value is at most its member's `@length` maximum (200
/// characters for a description, 50 for a name or a title), which the boundary validator
/// enforces before any handler runs.
const PATTERN_BACKTRACK_LIMIT: usize = 1_000_000;

/// Each [`ConstrainedText`] grammar's modelled expression, compiled once and paired with
/// the grammar it belongs to — paired rather than indexed, so no ordering has to be kept
/// in step and a lookup cannot reach another grammar's expression.
///
/// The expression itself is what judges a value ([`ConstrainedText::admits`]), read from
/// the one place this crate states it ([`ConstrainedText::pattern`]) — the same text the
/// refusal publishes ([`text_pattern_refusal`]) — so the constraint this service applies
/// and the constraint it reports cannot drift apart: a model revision to a constant moves
/// both. Compilation is fallible and its input is a compile-time constant, but a pattern
/// that failed to compile is dropped rather than unwrapped: nothing here panics, on any
/// input.
static COMPILED_PATTERNS: ::std::sync::LazyLock<
    [(ConstrainedText, Option<::fancy_regex::Regex>); ConstrainedText::ALL.len()],
> = ::std::sync::LazyLock::new(|| {
    ConstrainedText::ALL.map(|text| {
        let compiled = ::fancy_regex::RegexBuilder::new(text.pattern())
            .backtrack_limit(PATTERN_BACKTRACK_LIMIT)
            .build()
            .ok();
        (text, compiled)
    })
});

impl ConstrainedText {
    /// Every variant, which is what [`COMPILED_PATTERNS`] holds a compiled expression for.
    const ALL: [Self; 3] = [Self::ResourceName, Self::Description, Self::WidgetTitle];

    /// This grammar's modelled expression, compiled; `None` for an expression no engine
    /// here could compile.
    fn compiled(self) -> Option<&'static ::fancy_regex::Regex> {
        COMPILED_PATTERNS
            .iter()
            .find(|(text, _)| *text == self)?
            .1
            .as_ref()
    }

    /// The modelled `@pattern` this grammar is, anchors included.
    pub fn pattern(self) -> &'static str {
        match self {
            Self::ResourceName => RESOURCE_NAME_PATTERN,
            Self::Description => DESCRIPTION_PATTERN,
            Self::WidgetTitle => WIDGET_TITLE_PATTERN,
        }
    }

    /// Whether `value` satisfies this grammar — the whole expression, lookahead included.
    ///
    /// The modelled expression is matched as written ([`Self::compiled`]), which is why
    /// the lookahead the boundary validator cannot compile is applied here.
    ///
    /// A value this crate cannot PROVE satisfies the expression it publishes does not
    /// satisfy it: a match that exhausts [`PATTERN_BACKTRACK_LIMIT`], and an expression
    /// that did not compile, both count as not admitted, so nothing is ever stored
    /// unchecked against a constraint the refusal advertises. Neither is reachable for
    /// these members — the expressions are constants that compile (every value the crate's
    /// tests submit under them is admitted) and the budget is out of reach of a value the
    /// `@length` bounds already cap.
    pub fn admits(self, value: &str) -> bool {
        self.compiled()
            .is_some_and(|compiled| compiled.is_match(value).unwrap_or(false))
    }
}

/// One text member of a request, by the path a refusal states it at, the value submitted
/// and the grammar it has to satisfy.
pub type TextMember<'a> = (String, &'a str, ConstrainedText);

/// How this service refuses the text members of `members` that break their modelled
/// `@pattern`, or `None` when every one of them satisfies it.
///
/// The refusal is the boundary's own: one clause per member that broke its pattern,
/// quoting the expression the service publishes — the model's text without the `^`/`$`
/// Smithy wraps it in, exactly as [`crate::service`]'s refusal seam quotes it — under the
/// framework's `N validation error(s) detected` envelope. So a value refused here is
/// refused in the same words as one the boundary could judge itself, and the caller
/// cannot tell which layer applied the pattern.
pub fn text_pattern_refusal<'a>(members: impl IntoIterator<Item = TextMember<'a>>) -> Option<String> {
    let violations: Vec<Violation> = members
        .into_iter()
        .filter(|(_, value, text)| !text.admits(value))
        .map(|(path, value, text)| Violation {
            path,
            kind: ViolationKind::Pattern(::service_common::validation::unanchored(text.pattern())),
            // The value the caller submitted, which a constraint clause echoes. Bounded
            // by the member's own `@length` maximum (50 for a name or a title, 200 for a
            // description), which the boundary already enforced.
            value: Some(value.to_string()),
            sensitive: false,
        })
        .collect();
    envelope(&violations)
}

/// The text members a write of a DASHBOARD submits, for [`text_pattern_refusal`]: the
/// dashboard's own name and description, and the title and description of every widget it
/// carries.
///
/// One list for both writes, so `CreateDashboard` and `UpdateDashboard` apply the same
/// grammars at the same member paths. A widget names its 1-based position the way the
/// framework spells a list element's path.
pub fn dashboard_text_members<'a>(
    name: Option<&'a str>,
    description: Option<&'a str>,
    widgets: &'a [Widget],
) -> Vec<TextMember<'a>> {
    let mut members: Vec<TextMember<'a>> = Vec::new();
    if let Some(name) = name {
        members.push(("name".to_string(), name, ConstrainedText::ResourceName));
    }
    if let Some(description) = description {
        members.push((
            "description".to_string(),
            description,
            ConstrainedText::Description,
        ));
    }
    for (index, widget) in widgets.iter().enumerate() {
        let position = ::aws_smithy_schema_server::constraints::path::element(index + 1);
        members.push((
            ::aws_smithy_schema_server::constraints::path::join(&[
                "widgets".to_string(),
                position.clone(),
                "title".to_string(),
            ]),
            widget.title.as_str(),
            ConstrainedText::WidgetTitle,
        ));
        if let Some(widget_description) = widget.description.as_deref() {
            members.push((
                ::aws_smithy_schema_server::constraints::path::join(&[
                    "widgets".to_string(),
                    position,
                    "description".to_string(),
                ]),
                widget_description,
                ConstrainedText::Description,
            ));
        }
    }
    members
}

/// The text members a write of a SCHEDULED REPORT submits, for [`text_pattern_refusal`]:
/// the report's name and description.
///
/// `prefix` is the member the two are nested under, or `None` when the request carries
/// them at its top level — `CreateScheduledReport` submits them inside `scheduledReport`
/// while `UpdateScheduledReport` carries them directly, and a clause states the path the
/// request used.
pub fn report_text_members<'a>(
    prefix: Option<&str>,
    name: Option<&'a str>,
    description: Option<&'a str>,
) -> Vec<TextMember<'a>> {
    let path = |member: &str| match prefix {
        Some(prefix) => format!("{prefix}.{member}"),
        None => member.to_string(),
    };
    let mut members: Vec<TextMember<'a>> = Vec::new();
    if let Some(name) = name {
        members.push((path("name"), name, ConstrainedText::ResourceName));
    }
    if let Some(description) = description {
        members.push((
            path("description"),
            description,
            ConstrainedText::Description,
        ));
    }
    members
}

/// The instant the text of an `ABSOLUTE` [`DateTimeValue`] names, in seconds since the
/// epoch, or `None` when the text names no point in time at all.
///
/// The calendar is the runtime's own (`aws_smithy_types::DateTime`, the reader this service
/// already renders and reads its timestamps with) plus
/// [`service_common::rfc3339::split_utc_offset`] for the numeric zone that reader does not
/// take, so no date arithmetic and no leap-year rule is written here:
///
/// * a DATE alone names that date's first instant — the form the model's own example of an
///   absolute value takes ("2025-07-01");
/// * a value carrying a time of day names the instant it spells, in any of the zone forms
///   RFC 3339 admits, and with no zone it is read as UTC.
///
/// TOTAL over every string a caller can send: a value outside those forms — prose, a
/// calendar date that does not exist, a month without a day, epoch seconds — names no
/// instant, and the caller refuses it.
fn absolute_instant(value: &str) -> Option<i64> {
    let utc_instant = |civil: &str| -> Option<i64> {
        Some(
            ::aws_smithy_types::DateTime::from_str(
                civil,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .ok()?
            .secs(),
        )
    };
    if !value.contains(['T', 't']) {
        // A date with no time part: the first instant of that date.
        return utc_instant(&format!("{value}T00:00:00Z"));
    }
    match ::service_common::rfc3339::split_utc_offset(value) {
        Some((civil, offset)) => Some(utc_instant(&format!("{civil}Z"))? - offset),
        // No zone at all, so the civil time is the UTC one.
        None => utc_instant(&format!("{value}Z")),
    }
}

/// Whether `point` carries a `value` in the form its `type` names, which is what makes it
/// a point in time rather than text.
///
/// `DateTimeValue` is documented as "a point in time that can be specified as either an
/// absolute date (for example, \"2025-07-01\") or a relative time period using ISO 8601
/// duration format (for example, \"-P3M\" for three months ago)"
/// (`smithy_query(query="shape",shape="DateTimeValue")`), and its `type` says which of the
/// two a given value is. `value` targets an unconstrained string, so the form is read here:
/// an `ABSOLUTE` value has to name an instant ([`absolute_instant`]) and a `RELATIVE` one
/// has to be an ISO 8601 duration, read by the maintained parser every service of this
/// workspace reads one with (`service_common::iso8601_span::is_iso8601_duration`).
///
/// A `type` outside the enum is the BOUNDARY's refusal to word (`DateTimeType` is an
/// enumerated shape), and no form belongs to it, so this answers `true` for one and lets
/// that refusal stand.
fn names_a_point_in_time(point: &DateTimeValue) -> bool {
    match point.r#type.as_str() {
        DATE_TIME_TYPE_ABSOLUTE => absolute_instant(&point.value).is_some(),
        DATE_TIME_TYPE_RELATIVE => {
            ::service_common::iso8601_span::is_iso8601_duration(&point.value)
        }
        _ => true,
    }
}

/// How this service refuses the [`DateTimeValue`] at the member path `path` whose `value`
/// is not in the form its `type` names.
///
/// Worded from the request, like every other refusal this service builds: the member that
/// broke the rule, the value it carried and the form that `type` requires, named with the
/// model's own example of it.
///
/// The submitted value is quoted within what the clause itself leaves of
/// [`MAX_MESSAGE_LENGTH`] ([`elided_within`]), because `DateTimeValue$value` admits as much
/// text as the message member that has to carry it — both target `GenericString`, `@length
/// min 0, max 1024` (`smithy_query(query="shape",shape="DateTimeValue,GenericString")`) —
/// so quoting a maximal value whole beside the clause puts a message on the wire some two
/// hundred characters past the maximum the model declares for it. The value is the part
/// that gives way: the member path and the form `type` requires are what tell the caller
/// what to change.
fn malformed_date_time_value_message(path: &str, point: &DateTimeValue) -> String {
    let requirement = if point.r#type == DATE_TIME_TYPE_ABSOLUTE {
        "the value of an ABSOLUTE date/time value must be an absolute date, for example \
         '2025-07-01'"
    } else {
        "the value of a RELATIVE date/time value must be a relative time period in the ISO \
         8601 duration format, for example '-P3M' for three months ago"
    };
    let worded = |value: &str| {
        format!(
            "{path}.value: '{value}' is not a valid {} date/time value; {requirement}",
            point.r#type
        )
    };
    let budget = MAX_MESSAGE_LENGTH.saturating_sub(worded("").chars().count());
    within_message_bound(worded(&elided_within(&point.value, budget)))
}

/// How this service refuses the [`DateTimeRange`] at the member path `path`, or `None`
/// when that range defines a time period.
///
/// `DateTimeRange` is documented as "a time period with explicit start and end times for
/// data queries", with both bounds `@required`
/// (`smithy_query(query="shape",shape="DateTimeRange")`), so two rules are applied here
/// before a range is stored, in the order the request spells its members:
///
/// * each bound has to name a point in time in the form its `type` declares
///   ([`names_a_point_in_time`]);
/// * the period has to run forwards — the end may not come BEFORE the start. Only two
///   bounds that both name a FIXED point (both `ABSOLUTE`) are ordered here: a `RELATIVE`
///   bound names an offset resolved when the data is queried rather than a point this
///   service holds, so nothing at write time can say which of two of them comes first.
///   Equal bounds are a period of no length, which is a period and is accepted.
///
/// One helper for every write that submits a range, so a widget's `timeRange` and a
/// scheduled report's `widgetDateRangeOverride` cannot drift apart on either rule.
///
/// Both refusals quote the values the request carried within the maximum the member that
/// has to carry the message declares ([`MAX_MESSAGE_LENGTH`]), which is the same maximum
/// one of those values alone may reach.
pub fn date_time_range_refusal(path: &str, range: &DateTimeRange) -> Option<String> {
    for (member, point) in [
        ("startTime", &range.start_time),
        ("endTime", &range.end_time),
    ] {
        if !names_a_point_in_time(point) {
            return Some(malformed_date_time_value_message(
                &format!("{path}.{member}"),
                point,
            ));
        }
    }
    let both_absolute = range.start_time.r#type == DATE_TIME_TYPE_ABSOLUTE
        && range.end_time.r#type == DATE_TIME_TYPE_ABSOLUTE;
    if both_absolute {
        let start = absolute_instant(&range.start_time.value)?;
        let end = absolute_instant(&range.end_time.value)?;
        if end < start {
            let worded = |end: &str, start: &str| {
                format!(
                    "{path}: endTime '{end}' is earlier than startTime '{start}'; a date/time \
                     range defines a time period with explicit start and end times"
                )
            };
            // Two quoted values this time, so each is held to an equal share of the room
            // the clause leaves — the same rule as the malformed-value refusal above, and
            // needed for the same reason: either bound may carry a `GenericString` as long
            // as the message member itself.
            let share = MAX_MESSAGE_LENGTH.saturating_sub(worded("", "").chars().count()) / 2;
            return Some(within_message_bound(worded(
                &elided_within(&range.end_time.value, share),
                &elided_within(&range.start_time.value, share),
            )));
        }
    }
    None
}

/// The `MetricName` values a cost-and-usage query's `metrics` admits, in the order the
/// model lists them: `CostAndUsageQuery$metrics` is documented "Valid values for
/// CostAndUsageQuery metrics are AmortizedCost, BlendedCost, NetAmortizedCost,
/// NetUnblendedCost, NormalizedUsageAmount, UnblendedCost, and UsageQuantity"
/// (`smithy_query(query="shape",shape="CostAndUsageQuery")`).
const COST_AND_USAGE_METRICS: &[&str] = &[
    "AmortizedCost",
    "BlendedCost",
    "NetAmortizedCost",
    "NetUnblendedCost",
    "NormalizedUsageAmount",
    "UnblendedCost",
    "UsageQuantity",
];

/// The `MetricName` values a Savings Plans coverage query's `metrics` admits:
/// `SavingsPlansCoverageQuery$metrics` is documented "Valid value for
/// SavingsPlansCoverageQuery metrics is SpendCoveredBySavingsPlans"
/// (`smithy_query(query="shape",shape="SavingsPlansCoverageQuery")`).
const SAVINGS_PLANS_COVERAGE_METRICS: &[&str] = &["SpendCoveredBySavingsPlans"];

/// The `MetricName` values a reservation coverage query's `metrics` admits, in the order
/// the model lists them: `ReservationCoverageQuery$metrics` is documented "Valid values for
/// ReservationCoverageQuery metrics are Hour, Unit, and Cost"
/// (`smithy_query(query="shape",shape="ReservationCoverageQuery")`).
const RESERVATION_COVERAGE_METRICS: &[&str] = &["Hour", "Unit", "Cost"];

/// The `metrics` one widget query submitted, beside the values ITS OWN member documents as
/// valid.
struct SubmittedMetrics<'a> {
    /// The shape carrying the `metrics` member, as its documented rule names it — the name
    /// a refusal quotes the rule under.
    query_shape: &'static str,
    /// What the request asked for.
    submitted: &'a [String],
    /// The `MetricName` values that member admits.
    valid: &'static [&'static str],
}

/// One widget query as a dashboard write sees it: which member of the `QueryParameters`
/// union carries it, its time range, and the `metrics` it submitted when its member models
/// that list at all.
struct SubmittedQuery<'a> {
    member: &'static str,
    range: &'a DateTimeRange,
    metrics: Option<SubmittedMetrics<'a>>,
}

/// The submitted query one `QueryParameters` union member carries.
///
/// Every member of the union carries a `@required` `timeRange`
/// (`smithy_query(query="shape",shape="QueryParameters")`), so the rules a range has to
/// satisfy reach all five of them rather than only the cost-and-usage query. THREE of them
/// also model a `metrics` list, and each one documents its own set of valid `MetricName`
/// values; the two utilization queries model no `metrics` member at all, so there is no
/// metric of theirs to judge.
fn submitted_query(parameters: &QueryParameters) -> SubmittedQuery<'_> {
    match parameters {
        QueryParameters::costAndUsage(query) => SubmittedQuery {
            member: "costAndUsage",
            range: &query.time_range,
            metrics: Some(SubmittedMetrics {
                query_shape: "CostAndUsageQuery",
                submitted: &query.metrics,
                valid: COST_AND_USAGE_METRICS,
            }),
        },
        QueryParameters::savingsPlansCoverage(query) => SubmittedQuery {
            member: "savingsPlansCoverage",
            range: &query.time_range,
            metrics: Some(SubmittedMetrics {
                query_shape: "SavingsPlansCoverageQuery",
                submitted: query.metrics.as_deref().unwrap_or_default(),
                valid: SAVINGS_PLANS_COVERAGE_METRICS,
            }),
        },
        QueryParameters::savingsPlansUtilization(query) => SubmittedQuery {
            member: "savingsPlansUtilization",
            range: &query.time_range,
            metrics: None,
        },
        QueryParameters::reservationCoverage(query) => SubmittedQuery {
            member: "reservationCoverage",
            range: &query.time_range,
            metrics: Some(SubmittedMetrics {
                query_shape: "ReservationCoverageQuery",
                submitted: query.metrics.as_deref().unwrap_or_default(),
                valid: RESERVATION_COVERAGE_METRICS,
            }),
        },
        QueryParameters::reservationUtilization(query) => SubmittedQuery {
            member: "reservationUtilization",
            range: &query.time_range,
            metrics: None,
        },
    }
}

/// `values` as a documented list reads them: `A`, `A and B`, or `A, B, and C`.
fn listed_in_prose(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first, last] => format!("{first} and {last}"),
        [leading @ .., last] => format!(
            "{}{MESSAGE_LIST_SEPARATOR}and {last}",
            leading.join(MESSAGE_LIST_SEPARATOR)
        ),
    }
}

/// How a dashboard write refuses the metric at the member path `path`, which the query
/// carrying it does not admit.
///
/// Worded the way every other refusal of this service is: the member that broke the rule,
/// the value it carried, and the rule itself quoted from the member whose documentation
/// states it — so the clause a caller reads is the one their own copy of the model carries.
/// Nothing quoted here is free text: a `MetricName` is an enumerated value the boundary
/// validator already held to the eleven the enum declares, and the rest of the message is
/// a member path, so no part of it can reach [`MAX_MESSAGE_LENGTH`] the way a message
/// quoting a string member can.
fn metric_outside_query_message(
    path: &str,
    metric: &str,
    member: &str,
    metrics: &SubmittedMetrics<'_>,
) -> String {
    let shape = metrics.query_shape;
    let documented = if metrics.valid.len() == 1 {
        format!(
            "valid value for {shape} metrics is {}",
            listed_in_prose(metrics.valid)
        )
    } else {
        format!(
            "valid values for {shape} metrics are {}",
            listed_in_prose(metrics.valid)
        )
    };
    format!("{path}: '{metric}' is not a valid metric for a {member} query; {documented}")
}

/// How this service refuses the `widgets` a dashboard write submits, or `None` when every
/// widget in the list is one this service can store.
///
/// Three rules, applied before anything is written — no ARN minted, no widget id consumed —
/// so a stored dashboard never holds a widget configuration a reader would have to make
/// sense of:
///
/// * the time range of every query of every config is a time period
///   ([`date_time_range_refusal`]);
/// * every metric a query asks for is one THAT query admits
///   ([`metric_outside_query_message`]). `metrics` targets `MetricNames` everywhere it is
///   modelled, whose member is the `MetricName` enum holding the UNION of the three
///   documented sets (`smithy_query(query="shape",shape="MetricName,MetricNames")`), so the
///   boundary validator can only check membership in that union — which set of it applies
///   is decided by the union member carrying the query, and that is known only here;
/// * the ids in ONE list are DISTINCT. `Widget$id` is documented as "the unique identifier
///   for the widget" (`smithy_query(query="shape",shape="Widget")`), and a client may
///   supply it, so two widgets of one dashboard carrying one id would make every id-keyed
///   resolution of a widget ambiguous — a scheduled report's `widgetIds`, and the
///   `WIDGET_ID_NOT_FOUND` an execution derives from it.
///
/// The first two are judged per config, in the order the request spells its configs, so the
/// member a refusal names is the first one of the list that breaks a rule.
///
/// One helper for both dashboard writes, so `CreateDashboard` and `UpdateDashboard` apply
/// the same rules at the same member paths. A widget and a config name their 1-based
/// position the way the framework spells a list element's path.
pub fn widget_list_refusal(widgets: &[Widget]) -> Option<String> {
    let position = ::aws_smithy_schema_server::constraints::path::element;
    let widget_path = |index: usize| {
        ::aws_smithy_schema_server::constraints::path::join(&[
            "widgets".to_string(),
            position(index + 1),
        ])
    };
    for (index, widget) in widgets.iter().enumerate() {
        for (config_index, config) in widget.configs.iter().enumerate() {
            let query = submitted_query(&config.query_parameters);
            let query_member = query.member;
            let query_path = ::aws_smithy_schema_server::constraints::path::join(&[
                widget_path(index),
                "configs".to_string(),
                position(config_index + 1),
                "queryParameters".to_string(),
                query_member.to_string(),
            ]);
            let path = ::aws_smithy_schema_server::constraints::path::join(&[
                query_path.clone(),
                "timeRange".to_string(),
            ]);
            if let Some(message) = date_time_range_refusal(&path, query.range) {
                return Some(message);
            }
            let Some(metrics) = query.metrics else {
                continue;
            };
            for (metric_index, metric) in metrics.submitted.iter().enumerate() {
                if metrics.valid.contains(&metric.as_str()) {
                    continue;
                }
                let path = ::aws_smithy_schema_server::constraints::path::join(&[
                    query_path.clone(),
                    "metrics".to_string(),
                    position(metric_index + 1),
                ]);
                return Some(metric_outside_query_message(
                    &path,
                    metric,
                    query_member,
                    &metrics,
                ));
            }
        }
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (index, widget) in widgets.iter().enumerate() {
        let Some(id) = widget.id.as_deref().filter(|id| !id.is_empty()) else {
            // No id of its own: `adopt_widgets` mints one, and a minted id is fresh.
            continue;
        };
        if !seen.insert(id) {
            return Some(format!(
                "{}.id: '{id}' is already the id of another widget in this dashboard; the \
                 identifier of a widget must be unique within the dashboard",
                widget_path(index)
            ));
        }
    }
    None
}

/// Encode the ARN a listing page resumes at as an opaque `nextToken`.
///
/// One ARN has exactly ONE cursor: the prefix followed by the ARN's bytes in lower-case
/// hex. That makes this function the whole definition of the cursor format, which
/// [`decode_page_token`] reads a token back against.
pub fn encode_page_token(arn: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(arn))
}

/// The `@length` bounds a resource ARN of this service carries: `DashboardArn` and
/// `ScheduledReportArn` both declare 20..2048
/// (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
const RESOURCE_ARN_LENGTH: ::std::ops::RangeInclusive<usize> = 20..=2048;

/// The fixed opening of the partition field, which the ARN patterns spell
/// `aws[-a-z0-9]*`: the literal `aws` followed by a run of [`is_arn_label`]'s characters.
const ARN_PARTITION_PREFIX: &str = "aws";

/// How many digits an account field holds, as the ARN patterns' `[0-9]{12}` spells it.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The resource id that stands for every resource of a type, which the ARN patterns admit
/// beside a concrete id (`(\*|[-a-z0-9]+)`).
const ARN_RESOURCE_WILDCARD: &str = "*";

/// Whether `label` is a (possibly empty) run of the characters `[-a-z0-9]`, the one class
/// this service's ARN patterns admit in a partition's suffix and in a resource id.
fn is_arn_label(label: &str) -> bool {
    label
        .chars()
        .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

/// Whether `value` is an ARN of this service's `resource_type` resource, judged by that
/// resource's whole modelled ARN pattern — `DashboardArn`'s
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$` and
/// `ScheduledReportArn`'s, which differ only in the resource type
/// (`smithy_query(query="shape",shape="DashboardArn,ScheduledReportArn")`).
///
/// The expression is anchored at both ends, so the check is positional: six
/// colon-separated fields (a resource id of these patterns carries no colon of its own),
/// each judged against the pattern's requirement for THAT field — the literal `arn`, a
/// partition opening with `aws`, this service's ARN namespace, an empty region, twelve
/// digits of account, and the resource type followed by `/` and one resource id. A value
/// that satisfies the segments in some other arrangement is not an ARN of this resource.
///
/// Splitting `value` into those fields, and the resource field into its type and its id,
/// is not this service's business: both come from `service_common::arn`, the tree's one
/// reader of a resource ARN, which also settles the literal `arn` prefix and the
/// namespace. What is left here is only what THESE patterns require of each field.
pub fn names_resource_arn(value: &str, resource_type: &str) -> bool {
    if !RESOURCE_ARN_LENGTH.contains(&value.len()) {
        return false;
    }
    let Some(arn) = ::service_common::arn::parse_of(value, ARN_NAMESPACE) else {
        return false;
    };
    arn.partition
        .strip_prefix(ARN_PARTITION_PREFIX)
        .is_some_and(is_arn_label)
        && arn.region.is_empty()
        && arn.account.len() == ACCOUNT_ID_DIGITS
        && arn.account.chars().all(|digit| digit.is_ascii_digit())
        // These patterns spell the resource `<type>/<id>` and nothing else, so a resource
        // field carrying a colon — the `<type>:<id>` form AWS writes elsewhere, or a
        // further field behind the id — names no resource of this service.
        && !arn.resource.contains(':')
        && ::service_common::arn::resource_id_of_type(value, resource_type).is_some_and(
            |resource_id| {
                resource_id == ARN_RESOURCE_WILDCARD
                    || (!resource_id.is_empty() && is_arn_label(resource_id))
            },
        )
}

/// The ARN a `nextToken` resumes at, or `None` when the token is not one this
/// service minted for a listing over `resource_type`'s resource (the caller
/// answers that with a `ValidationException`).
///
/// A cursor this service minted always names an ARN of the resource the listing
/// walks. It is checked against that resource's whole ARN pattern
/// ([`names_resource_arn`]), so a token that decodes to anything else — a cursor minted
/// by a listing over the other resource type, or a hand-written body that merely carries
/// the pattern's segments somewhere in it — is rejected rather than read as a resume
/// point past every stored record.
///
/// Naming the right ARN is necessary but not sufficient: the token has to be the EXACT
/// string [`encode_page_token`] writes for that ARN. A cursor is one spelling per resume
/// point, so a body that decodes to the same bytes in some other spelling — the same hex
/// digits in upper or mixed case, which the decoder underneath accepts as readily as the
/// lower case the encoder emits — is a token this service could not have issued, and is
/// refused like any other.
pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    (token == encode_page_token(&arn) && names_resource_arn(&arn, resource_type)).then_some(arn)
}

/// One page of a listing over an ARN-keyed resource map: the records of the page,
/// already projected onto the shape the listing answers, and the cursor the next page
/// resumes at — present exactly when a record remains after the page.
#[derive(Debug)]
pub struct ListingPage<T> {
    /// The page's records, in ARN order.
    pub items: Vec<T>,
    /// The `nextToken` to answer, or `None` when the page exhausted the map.
    pub next_token: Option<String>,
}

/// Why a listing request names no page this service can serve. The listing turns it
/// into its own modelled `ValidationException` with [`ListingRefusal::message`], which
/// is the only part of the refusal that differs between listings.
#[derive(Debug)]
pub enum ListingRefusal {
    /// The request carried a `nextToken` this service did not mint for a listing over
    /// the resource type walked.
    UninterpretableToken,
    /// The request asked for a page of this many records, which is not a page any
    /// listing can answer.
    NonPositivePageSize(i32),
}

impl ListingRefusal {
    /// The `ValidationException` message for this refusal of `operation`'s listing.
    pub fn message(&self, operation: &str) -> String {
        match self {
            Self::UninterpretableToken => format!(
                "Member must be a nextToken returned by a previous {operation} call: nextToken"
            ),
            Self::NonPositivePageSize(max_results) => format!(
                "Member must have value greater than or equal to 1: maxResults {max_results}"
            ),
        }
    }
}

/// One page of a listing over `records`, an ARN-keyed map of this service's resources.
///
/// Every listing this service serves pages the same way, so the rule lives here once
/// rather than once per list handler: a `next_token` is the ARN to resume at, decoded
/// for the resource `resource_type` names and refused when it is not a cursor this
/// service minted for that resource; `max_results` is the page size, refused when it is not
/// at least one (a zero-item page that still carried a cursor would be a client loop
/// that never terminates); the walk starts at the cursor's ARN inclusive, or at the
/// first stored record, takes at most a page of records and projects each one with
/// `project`; and the cursor minted is the ARN of the first record AFTER the page, so
/// `next_token` is present exactly when one remains.
///
/// The token is decoded before the page size is read, so a request that is wrong in
/// both ways is answered for its token.
pub fn listing_page<'records, R, T>(
    records: &'records BTreeMap<String, R>,
    next_token: Option<&str>,
    max_results: i32,
    resource_type: &str,
    mut project: impl FnMut(&'records R) -> T,
) -> Result<ListingPage<T>, ListingRefusal> {
    let resume_from = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token, resource_type).ok_or(ListingRefusal::UninterpretableToken)?,
        ),
    };
    let page_size = match usize::try_from(max_results) {
        Ok(size) if size >= 1 => size,
        _ => return Err(ListingRefusal::NonPositivePageSize(max_results)),
    };
    // Borrowed bounds, so resuming copies no ARN.
    let start = match resume_from.as_deref() {
        Some(arn) => Bound::Included(arn),
        None => Bound::Unbounded,
    };
    let mut remaining = records.range::<str, _>((start, Bound::Unbounded));
    let items: Vec<T> = remaining
        .by_ref()
        .take(page_size)
        .map(|(_, record)| project(record))
        .collect();
    let next_token = remaining.next().map(|(arn, _)| encode_page_token(arn));
    Ok(ListingPage { items, next_token })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState>)
}

/// The event bus one execution of a scheduled report delivers its notification to.
///
/// "Events generated by Amazon Web Services services go to your account's default event
/// bus" (EventBridge's event-bus reference), and the default bus is named `default` —
/// which is also the bus the model names for this service's notification
/// (`HealthStatus$statusCode`: "the default Amazon Web Services EventBridge EventBus in
/// your account").
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `source` every event this service publishes carries.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`,
/// where the label is the service's own namespace — this service's
/// `aws.api#service$arnNamespace`, i.e. the `ARN_NAMESPACE` this module mints ARNs under.
/// Spelled as one literal because a `source` is a wire value a client writes an
/// `EventPattern` against.
pub const EVENT_SOURCE: &str = "aws.bcm-dashboards";

/// The `detail-type` of the notification one execution of a scheduled report delivers.
///
/// CHOSEN, not cited: the model documents THAT an execution delivers a notification to
/// the account's default event bus, and names neither a detail type nor a detail schema
/// for it. The value follows the form EventBridge's service events use — the service, the
/// resource and what happened to it — so a rule can match this notification and nothing
/// else.
pub const SCHEDULED_REPORT_EXECUTION_DETAIL_TYPE: &str =
    "BCM Dashboards Scheduled Report Execution";

/// The Region a notification is delivered in when the request carried no SigV4 credential
/// scope to read one from.
const DEFAULT_REGION: &str = "us-east-1";

/// The Region this request was signed for, which is the Region whose default event bus an
/// execution's notification goes to: the account's buses are regional, so the delivery
/// belongs to the Region the caller addressed and not to a Region frozen at startup.
///
/// This service's own ARNs carry no Region (the model's ARN patterns have an empty Region
/// segment), so the scope is the only thing that names one.
fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Render `at` as the RFC 3339 instant an event's `detail` carries, through the one
/// instant rendering this crate has ([`instant_text`]), so an instant in an event and an
/// instant in a refusal cannot drift apart on their format.
fn rfc3339(at: SystemTime) -> String {
    instant_text(&::aws_smithy_types::DateTime::from(at))
}

/// The `detail` of the notification one execution delivers: the report the execution ran,
/// read from the STORED record in the member names the modelled `ScheduledReport` shape
/// spells them, plus the instant the execution ran.
///
/// Nothing here is taken from the request or invented: a reader of this event and a
/// reader of `GetScheduledReport` see one record. `widgetIds` appears only when the
/// record carries the member, exactly as the readers report it.
pub fn scheduled_report_execution_detail(
    report: &ScheduledReportRecord,
    executed_at: SystemTime,
) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert("arn".to_string(), ::serde_json::json!(report.arn));
    detail.insert("name".to_string(), ::serde_json::json!(report.name));
    detail.insert(
        "dashboardArn".to_string(),
        ::serde_json::json!(report.dashboard_arn),
    );
    if let Some(widget_ids) = &report.widget_ids {
        detail.insert("widgetIds".to_string(), ::serde_json::json!(widget_ids));
    }
    detail.insert(
        "executionTime".to_string(),
        ::serde_json::json!(rfc3339(executed_at)),
    );
    ::serde_json::Value::Object(detail)
}

/// A `HealthStatus` carrying `status_reasons`, refreshed at `last_refreshed_at`.
///
/// The one place the model's two invariants on this shape hold, so no caller can report a
/// status that breaks them: the code is [`HEALTH_STATUS_HEALTHY`] exactly while there is
/// no reason — the model documents `HEALTHY` as "configured properly and has all required
/// permissions to execute" and `UNHEALTHY` as the report being unable to deliver, "the
/// reason for the unhealthy state is captured in the health status reasons" — and
/// `statusReasons` is documented "only present when the status is UNHEALTHY", so a healthy
/// status carries the member absent rather than an empty list
/// (`smithy_query(query="shape",shape="HealthStatus")`).
fn health_status(
    status_reasons: Vec<String>,
    last_refreshed_at: Option<::aws_smithy_types::DateTime>,
) -> HealthStatus {
    let healthy = status_reasons.is_empty();
    HealthStatus {
        status_code: if healthy {
            HEALTH_STATUS_HEALTHY.to_string()
        } else {
            HEALTH_STATUS_UNHEALTHY.to_string()
        },
        last_refreshed_at,
        status_reasons: (!healthy).then_some(status_reasons),
    }
}

/// The health of a report whose execution could NOT hand its notification to the
/// account's default event bus.
///
/// This is the state the model grounds `UNHEALTHY` in — "the scheduled report is unable
/// to deliver the notification to the default Amazon EventBridge EventBus in your
/// account" — with [`STATUS_REASON_INTERNAL_FAILURE`] as the reason, because the report's
/// own configuration resolved and the delivery is what failed. `lastRefreshedAt` is `now`
/// because the execution that attempted the delivery is the refresh.
pub fn undelivered_notification_health(now: SystemTime) -> HealthStatus {
    health_status(
        vec![STATUS_REASON_INTERNAL_FAILURE.to_string()],
        Some(::aws_smithy_types::DateTime::from(now)),
    )
}

/// Deliver the notification one execution of `report` owes to the account's default
/// EventBridge event bus, and answer whether the bus took it.
///
/// This is what an execution of a scheduled report IS: the model defines the unhealthy
/// state as the report being unable to deliver this notification, so a healthy execution
/// delivers it and a client that wires an EventBridge rule to a scheduled report sees it
/// arrive. The envelope names the two resources the notification is about (the report and
/// the dashboard it renders) in `resources`, carries the stored record as its `detail`
/// ([`scheduled_report_execution_detail`]) and is stamped with the instant the execution
/// ran rather than the instant the bus was reached — `PutEventsRequestEntry$Time` is "the
/// time stamp of the event, per RFC3339. If no time stamp is provided, the time stamp of
/// the PutEvents call is used."
///
/// EventBridge is reached through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), the way any emulated service
/// reaches another: the request never leaves this process, so nothing is dialled and this
/// crate names no other service crate. Call it with NO store guard held — the guard is a
/// lock, and a lock is not held across an await.
///
/// `false` means the notification was not delivered, which the caller reports as the
/// report's health: either EventBridge refused the entry (`FailedEntryCount` counts "the
/// number of failed entries") or there was no EventBridge in this emulator to take it. A
/// report whose notification nothing accepted is exactly the report the model calls
/// unhealthy, so the outcome is reported rather than assumed either way.
pub async fn deliver_execution_notification(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    report: &ScheduledReportRecord,
    executed_at: SystemTime,
) -> bool {
    let region = request_region(ctx);
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(region.clone()))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
        .event_bus_name(DEFAULT_EVENT_BUS)
        .source(EVENT_SOURCE)
        .detail_type(SCHEDULED_REPORT_EXECUTION_DETAIL_TYPE)
        .time(::aws_sdk_eventbridge::primitives::DateTime::from(executed_at))
        .resources(&report.arn)
        .resources(&report.dashboard_arn)
        .detail(scheduled_report_execution_detail(report, executed_at).to_string())
        .build();
    let answer = ::aws_sdk_eventbridge::Client::from_conf(config)
        .put_events()
        .entries(entry)
        .send()
        .await;
    match answer {
        Ok(output) => {
            // One entry was published, so one failed entry is this notification's.
            if output.failed_entry_count() == 0 {
                return true;
            }
            ::tracing::warn!(
                scheduled_report = %report.arn,
                entries = ?output.entries(),
                "the default event bus refused the scheduled report's execution notification"
            );
            false
        }
        Err(error) => {
            ::tracing::warn!(
                scheduled_report = %report.arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the scheduled report's execution notification could not be delivered to the \
                 default event bus"
            );
            false
        }
    }
}
