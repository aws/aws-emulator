// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rum-rum service.
//!
//! The app monitor is this service's primary resource: `CreateAppMonitor`,
//! `GetAppMonitor`, `ListAppMonitors`, `UpdateAppMonitor`, `DeleteAppMonitor`,
//! `ListTagsForResource` and the data-plane `PutRumEvents` all read and write the ONE
//! [`AppMonitorRecord`] map held here, so every one of them answers from persisted state
//! rather than from its own request. The helpers beside the map (id minting, the derived
//! ARN and CloudWatch Logs group name, the `ResourceArn` -> monitor resolution, the
//! documented defaults, the record -> wire-shape projections and the error builders) live
//! here for the same reason: every operation file shares them, and a second copy of any
//! of them would let two operations disagree about one resource.
//!
//! The metrics side of a monitor hangs off that same record: the extended-metrics
//! destinations `PutRumMetricsDestination` creates, and under each of them the metric
//! definitions `BatchCreateRumMetricDefinitions` creates,
//! `BatchGetRumMetricDefinitions` reports, `UpdateRumMetricDefinition` replaces and
//! `BatchDeleteRumMetricDefinitions` removes. Both
//! are keyed through the one [`metric_destination_key`], so no two of those operations can
//! disagree about which destination a request names.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessDeniedException, AppMonitor, AppMonitorConfiguration,
    AppMonitorSummary, ConflictException, CustomEvents, CwLog, DataStorage,
    DeobfuscationConfiguration, InternalServerException, InvalidPolicyRevisionIdException,
    JavaScriptSourceMaps, MalformedPolicyDocumentException, MetricDefinition,
    MetricDefinitionRequest, MetricDestinationSummary, PolicyNotFoundException,
    ResourceNotFoundException, ServiceQuotaExceededException, ValidationException,
    METRIC_DESTINATION_VALUES,
};

/// Who a request speaks for, and whether IAM lets it do what it asks: the authorization
/// evaluation every operation in this service runs before it reads or writes.
pub mod authorization;

/// Where the caller a batch of telemetry arrived from is, which RUM adds to every event's
/// metadata at ingestion.
pub mod caller_location;

/// The CloudWatch Logs side of `DataStorage.CwLog`: the permission turning log delivery
/// on requires, the vended log group RUM provisions when it is on, and the copies of
/// delivered telemetry it stores there.
pub mod cw_log_delivery;

/// The RUM event document: the enriched form a delivered `RumEvent` is stored in,
/// reported as and copied as, and the codec placing it on the millisecond scale a query's
/// `TimeRange` is expressed in.
pub mod rum_event;

/// The modelled constraints this service applies itself — the ones whose `@pattern` the
/// boundary walk cannot compile — and the words every constraint refusal of this service
/// is stated in.
pub mod validation;

/// This service's ARN namespace / sigv4 signing name (`OPERATIONS.md`,
/// `## Wire protocol`: both are `rum`).
pub const ARN_NAMESPACE: &str = "rum";

/// The `appmonitor` resource segment of an app-monitor ARN.
const APP_MONITOR_RESOURCE: &str = "appmonitor";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous, host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `State` of an app monitor this emulator creates — a member of
/// [`crate::ops::types::STATE_ENUM_VALUES`] (`CREATED`, `DELETING`, `ACTIVE`).
///
/// `CreateAppMonitor` is synchronous and the monitor it returns is immediately able to
/// accept `PutRumEvents`, so the resource is created in a terminal state with no
/// clock-owed transition pending. Recorded behaviour: the terminal state RUM reports for
/// a monitor that exists and is not being torn down is `CREATED` — on a freshly created
/// monitor and on one that has since been updated alike — so that is the single value the
/// detail and the summary projections both report.
pub const CREATED_STATE: &str = "CREATED";

/// The `AppMonitorPlatform` of an app monitor for a WEB application — the one platform
/// whose application has an internet domain at all, and so the one a monitor's domain
/// authority can describe.
///
/// From the model: `AppMonitorPlatform` admits `Web`, `Android` and `iOS`
/// ([`crate::ops::types::APP_MONITOR_PLATFORM_VALUES`]), documented "Web for web
/// applications, Android for Android applications, and iOS for IOS applications", while
/// `Domain` is "The top-level internet domain name for which your application has
/// administrative authority" — a property only a web application has.
pub const WEB_PLATFORM: &str = "Web";

/// `Platform` of an app monitor whose create request named none.
///
/// From the model: `CreateAppMonitorRequest$Platform` documents "If you omit this
/// parameter, the default is Web."
pub const DEFAULT_PLATFORM: &str = WEB_PLATFORM;

/// `JavaScriptSourceMaps$Status` of an app monitor whose create request named no
/// `DeobfuscationConfiguration`: deobfuscation is off until a request turns it on. A
/// member of [`crate::ops::types::DEOBFUSCATION_STATUS_VALUES`].
pub const DEFAULT_DEOBFUSCATION_STATUS: &str = "DISABLED";

/// The `JavaScriptSourceMaps$Status` that turns source-map unminification ON — the value
/// the member's documented requirement on `S3Uri` is conditioned on
/// ([`deobfuscation_configuration_refusal`]). A member of
/// [`crate::ops::types::DEOBFUSCATION_STATUS_VALUES`].
pub const ENABLED_DEOBFUSCATION_STATUS: &str = "ENABLED";

/// `CustomEvents$Status` of an app monitor whose request left the member unset — the
/// request that named no `CustomEvents` at all and the one that carried the structure
/// without a `Status` alike ([`settled_custom_events`]).
///
/// From the model: `CustomEvents$Status` documents "The default is for custom events to be
/// `DISABLED`", and `CreateAppMonitorRequest$CustomEvents` documents "If you omit this
/// parameter, custom events are DISABLED." A member of
/// [`crate::ops::types::CUSTOM_EVENTS_STATUS_VALUES`].
pub const DEFAULT_CUSTOM_EVENTS_STATUS: &str = "DISABLED";

/// `SessionSampleRate` of an app monitor whose request left the member unset — the request
/// that named no `AppMonitorConfiguration` at all and the one that carried the structure
/// without a rate alike ([`settled_app_monitor_configuration`]).
///
/// From the model: `AppMonitorConfiguration$SessionSampleRate` documents "If you omit this
/// parameter, the default of 0.1 is used, and 10% of sessions will be sampled", and
/// `CreateAppMonitorRequest$AppMonitorConfiguration` documents "If you omit this argument,
/// the sample rate used for RUM is set to 10% of the user sessions."
pub const DEFAULT_SESSION_SAMPLE_RATE: f64 = 0.1;

/// The value an UNSET `SessionSampleRate` arrives as: the member's modelled `@default` of
/// `0`, which the deserializer fills in for a configuration that leaves the member out.
///
/// The member is also the bottom of its own modelled `@range` (`0` to `1` inclusive), so
/// this is the lowest value that can reach a handler at all, and a rate at or below it is
/// the unset one ([`settled_app_monitor_configuration`]).
pub const UNSET_SESSION_SAMPLE_RATE: f64 = 0.0;

/// Prefix of the CloudWatch Logs group an app monitor copies telemetry into when
/// `CwLogEnabled` is set: RUM is a vended-logs service, so the group lives under
/// `/aws/vendedlogs/RUMService_`, followed by the app monitor's own name and then by a
/// suffix the SERVICE chooses for that monitor ([`cw_log_group_name`]).
///
/// Recorded behaviour: the group name RUM advertises is not reconstructible from the
/// request — the prefix and the monitor's name are followed by a per-monitor value only
/// the service knows — which is why the vended name is MINTED once
/// ([`ComAmazonawsRumRumState::mint_cw_log_group`]) and then held on the record
/// ([`AppMonitorRecord::vended_log_group`]) rather than recomputed on every read.
pub const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// How many characters of the minted token the vended log group's service-chosen suffix
/// carries ([`cw_log_group_name`]).
///
/// The model places no pattern and no length on `CwLog$CwLogGroup`, and the suffix's
/// literal text is not documented anywhere, so the WIDTH is this emulator's own: twelve
/// lowercase hex characters, which is wide enough that no caller can guess one monitor's
/// group from another's and narrow enough to leave the whole name inside a CloudWatch Logs
/// group name's 512-character bound for every `AppMonitorName` the model admits (1..255).
pub const CW_LOG_GROUP_SUFFIX_LEN: usize = 12;

/// The `resourceType` this service's `ConflictException` reports for an app monitor.
///
/// Recorded behavior: the duplicate-name conflict `CreateAppMonitor` answers names the
/// resource's type as RUM's own product noun, not the model's shape or resource name.
/// It is the ONLY error of this service that reports a `resourceType` at all — the
/// not-found refusals leave the member off the wire (see
/// [`crate::ops::types::ResourceNotFoundException`]).
pub const APP_MONITOR_RESOURCE_TYPE: &str = "RUM application";

/// The most app monitors this emulator holds at once IN ONE REGION.
///
/// `app_monitors` grows by one record per `CreateAppMonitor` request and shrinks only on
/// an explicit delete, so without a ceiling its entry count — and with it the heap this
/// emulator holds — is set by how many requests a caller makes. AWS bounds the same
/// thing with an adjustable app-monitor quota held per account AND region (every RUM
/// endpoint is regional, and a quota value is a per-region setting), whose value is not
/// in the model, so this number is the EMULATOR's own bound, counted the way AWS counts
/// its own — over the monitors of the region the request was signed for
/// ([`ComAmazonawsRumRumState::app_monitor_count`]) — while the refusal it produces is
/// the error `CreateAppMonitor` models for exactly this case:
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_APP_MONITORS: usize = 1000;

/// How many distinct regions this store keeps app monitors for.
///
/// [`MAX_APP_MONITORS`] alone would not bound the store: the region keying it is the
/// request's own SigV4 credential-scope region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh per-region bucket per
/// request. With it the store holds at most `MAX_REGIONS` buckets, and so at most
/// `MAX_REGIONS * MAX_APP_MONITORS` monitors in total (and
/// `MAX_REGIONS * MAX_APP_MONITORS * MAX_EVENTS_PER_APP_MONITOR` buffered events). AWS
/// operates on the order of 36 commercial regions, so this bound is this emulator's own
/// and is comfortably above the real surface; a create that would pass it answers the
/// same modelled `ServiceQuotaExceededException` (HTTP 402) as one that would exceed the
/// per-region allowance.
pub const MAX_REGIONS: usize = 64;

/// The most RUM events this emulator keeps per app monitor.
///
/// `PutRumEvents` appends to a monitor's buffer on every call, so the buffer needs a
/// ceiling to keep the emulator's heap bounded; past it the OLDEST event is dropped,
/// which is the same direction as the 30-day retention AWS documents for RUM-held data
/// (`CreateAppMonitorRequest$CwLogEnabled`: "Data collected by RUM is kept by RUM for 30
/// days and then deleted"). The count is this emulator's own bound, not AWS's.
pub const MAX_EVENTS_PER_APP_MONITOR: usize = 10_000;

/// The `MetricDestination` value naming the account's own CloudWatch metrics as the
/// destination of an app monitor's extended metrics — a member of
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`].
pub const CLOUD_WATCH_DESTINATION: &str = "CloudWatch";

/// The `MetricDestination` value naming a CloudWatch Evidently experiment as the
/// destination of an app monitor's extended metrics — a member of
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`].
pub const EVIDENTLY_DESTINATION: &str = "Evidently";

/// The most extended-metrics destinations this emulator keeps per app monitor.
///
/// `PutRumMetricsDestination` adds one record per distinct destination key, so without a
/// ceiling the heap an app monitor holds is set by how many distinct Evidently
/// experiment ARNs a caller names. AWS bounds the same thing with an adjustable
/// per-app-monitor quota whose value is not in the model, so this number is the
/// EMULATOR's own bound; `PutRumMetricsDestination` models no quota error, so the
/// refusal past it is the `ValidationException` it does model.
pub const MAX_METRIC_DESTINATIONS_PER_APP_MONITOR: usize = 100;

/// The most metric definitions one extended-metrics destination holds.
///
/// AWS's OWN documented limit, not a bound this emulator chose: the model's
/// `BatchCreateRumMetricDefinitions` documents "The maximum number of metric definitions
/// that one destination can contain is 2000." A definition that would take a destination
/// past it is refused with the `ServiceQuotaExceededException` (HTTP 402) that operation
/// models.
pub const MAX_METRIC_DEFINITIONS_PER_DESTINATION: usize = 2000;

/// The most metric definitions one batch call may name — the same bound for the batch that
/// creates them and the batch that deletes them.
///
/// Also AWS's own: "The maximum number of metric definitions that you can specify in one
/// BatchCreateRumMetricDefinitions operation is 200", and, for the delete, "The maximum
/// number of metric definitions that you can specify in one
/// BatchDeleteRumMetricDefinitions operation is 200." Neither
/// `MetricDefinitionsRequest` nor `MetricDefinitionIds` carries a modelled length, so the
/// handler is what refuses a longer list.
pub const MAX_METRIC_DEFINITIONS_PER_BATCH: usize = 200;

/// The namespace RUM publishes EXTENDED metrics to.
///
/// From the model (`MetricDefinitionRequest`): "For extended metrics that are sent to the
/// `AWS/RUM` namespace … The `Namespace` parameter must be omitted or set to `AWS/RUM`."
pub const EXTENDED_METRICS_NAMESPACE: &str = "AWS/RUM";

/// The metric-namespace prefix AWS reserves for its own metrics.
///
/// From the model (`MetricDefinitionRequest$Namespace`): "You cannot use any string that
/// starts with `AWS/` for your namespace."
pub const RESERVED_NAMESPACE_PREFIX: &str = "AWS/";

/// One entry of RUM's EXTENDED-metric catalog: the metric name, and the `ValueKey` and
/// `EventPattern` the model pins for it.
///
/// A definition published to the extended-metrics namespace is not free-form — the model
/// (`MetricDefinitionRequest`) says "Only certain combinations of values for `Name`,
/// `ValueKey`, and `EventPattern` are valid" and then lists one combination per name — so
/// the catalog IS the rule, and a definition naming an extended metric is judged against
/// the row that names it ([`EXTENDED_METRIC_CATALOG`]).
#[derive(Debug, Clone, Copy)]
pub struct ExtendedMetric {
    /// The `Name` this row pins a combination for.
    pub name: &'static str,
    /// The `ValueKey` the model requires for this name, or `None` where it requires none
    /// ("`ValueKey` must be null").
    pub value_key: Option<&'static str>,
    /// The JSON object the model requires this name's `EventPattern` to INCLUDE. The
    /// pattern may carry more than this — "the `EventPattern` can also include information
    /// used by the `DimensionKeys` field" — so inclusion, not equality, is the rule
    /// ([`Self::required_event_pattern`], [`event_pattern_includes`]).
    pub event_pattern: &'static str,
}

impl ExtendedMetric {
    /// The required pattern, parsed. `None` only if the literal above is not JSON, which
    /// would make this row state no requirement at all.
    pub fn required_event_pattern(&self) -> Option<::serde_json::Value> {
        ::serde_json::from_str(self.event_pattern).ok()
    }
}

/// Every extended metric RUM defines, with the `ValueKey`/`EventPattern` combination the
/// model pins for each.
///
/// Source: the model's `MetricDefinitionRequest` documentation, which lists one "If `Name`
/// is …, then `ValueKey` must be … and the `EventPattern` must include …" rule per name
/// (`smithy_query(query="doc", shape="MetricDefinitionRequest")`). The comparison
/// operators are written here as the characters they are, since the documentation carries
/// them HTML-escaped, and the two rows whose documented literal is missing a separator
/// (`NavigationToleratedTransaction`) or carries a stray closing brace (`Http4xxCount`,
/// `Http5xxCount`) are written as the JSON object those literals describe.
pub const EXTENDED_METRIC_CATALOG: &[ExtendedMetric] = &[
    ExtendedMetric {
        name: "PerformanceNavigationDuration",
        value_key: Some("event_details.duration"),
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"]}"#,
    },
    ExtendedMetric {
        name: "PerformanceResourceDuration",
        value_key: Some("event_details.duration"),
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_resource_event"]}"#,
    },
    ExtendedMetric {
        name: "NavigationSatisfiedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">",2000]}]}}"#,
    },
    ExtendedMetric {
        name: "NavigationToleratedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">=",2000,"<",8000]}]}}"#,
    },
    ExtendedMetric {
        name: "NavigationFrustratedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">=",8000]}]}}"#,
    },
    ExtendedMetric {
        name: "WebVitalsCumulativeLayoutShift",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.cumulative_layout_shift_event"]}"#,
    },
    ExtendedMetric {
        name: "WebVitalsFirstInputDelay",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.first_input_delay_event"]}"#,
    },
    ExtendedMetric {
        name: "WebVitalsLargestContentfulPaint",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.largest_contentful_paint_event"]}"#,
    },
    ExtendedMetric {
        name: "JsErrorCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.js_error_event"]}"#,
    },
    ExtendedMetric {
        name: "HttpErrorCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"]}"#,
    },
    ExtendedMetric {
        name: "SessionCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.session_start_event"]}"#,
    },
    ExtendedMetric {
        name: "PageViewCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.page_view_event"]}"#,
    },
    ExtendedMetric {
        name: "Http4xxCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"],"event_details":{"response":{"status":[{"numeric":[">=",400,"<",500]}]}}}"#,
    },
    ExtendedMetric {
        name: "Http5xxCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"],"event_details":{"response":{"status":[{"numeric":[">=",500,"<=",599]}]}}}"#,
    },
];

/// The catalog row for the extended metric named `name`, or `None` when the catalog lists
/// no metric under that name — which is what makes a definition a CUSTOM metric, the kind
/// whose name is free ("For custom metrics, you can specify any name that you like").
pub fn extended_metric(name: &str) -> Option<&'static ExtendedMetric> {
    EXTENDED_METRIC_CATALOG.iter().find(|entry| entry.name == name)
}

/// Every `DimensionKeys` entry an EXTENDED metric may carry: the event field path the
/// dimension's value is read out of, and the CloudWatch dimension name RUM publishes that
/// field under.
///
/// Source: the model's `MetricDefinitionRequest$DimensionKeys` documentation — "For
/// extended metrics, valid values for the entries in this field are the following",
/// followed by one `field path`/`dimension name` pair per entry
/// (`smithy_query(query="doc", shape="MetricDefinitionRequest", member="DimensionKeys")`),
/// in the order it lists them. An extended metric is RUM's own metric, so the dimensions
/// it may be broken down by are the ones RUM itself computes: the pairing is the rule, not
/// a default a request can rename.
///
/// The crate's tests transcribe the same list independently from the model
/// (`EXTENDED_DIMENSION_KEYS` in `tests/all/metric_definitions.rs`), so they state the
/// expectation rather than agreeing with this constant by construction; the two are
/// expected to hold the same entries.
pub const EXTENDED_DIMENSION_ENTRIES: &[(&str, &str)] = &[
    ("metadata.pageId", "PageId"),
    ("metadata.browserName", "BrowserName"),
    ("metadata.deviceType", "DeviceType"),
    ("metadata.osName", "OSName"),
    ("metadata.countryCode", "CountryCode"),
    ("event_details.fileType", "FileType"),
];

/// The dimension name [`EXTENDED_DIMENSION_ENTRIES`] pins for the field path `field`, or
/// `None` when it lists no entry keyed by that path — which is what makes the entry one an
/// extended metric may not carry.
pub fn extended_dimension_name(field: &str) -> Option<&'static str> {
    EXTENDED_DIMENSION_ENTRIES
        .iter()
        .find(|(path, _)| *path == field)
        .map(|(_, dimension)| *dimension)
}

/// Every field path an extended metric's `DimensionKeys` may be keyed by, as a refusal
/// names them.
fn extended_dimension_fields() -> String {
    EXTENDED_DIMENSION_ENTRIES
        .iter()
        .map(|(field, _)| *field)
        .collect::<Vec<&str>>()
        .join(", ")
}

/// Every field a RUM event carries, which is every FIRST-LEVEL key a custom metric's
/// `ValueKey`, `EventPattern` and `DimensionKeys` may name.
///
/// Source: the model's `MetricDefinitionRequest` documentation, "Valid key values for
/// first-level keys in the `ValueKey`, `EventPattern`, and `DimensionKeys` fields"
/// (`smithy_query(query="doc", shape="MetricDefinitionRequest")`), in the order it lists
/// them.
pub const RUM_EVENT_FIELDS: &[&str] = &[
    "account_id",
    "application_Id",
    "application_version",
    "application_name",
    "batch_id",
    "event_details",
    "event_id",
    "event_interaction",
    "event_timestamp",
    "event_type",
    "event_version",
    "log_stream",
    "metadata",
    "sessionId",
    "user_details",
    "userId",
];

/// The event field carrying the details of the event an `EventPattern` matches — the one
/// whose presence the model pairs with [`EVENT_TYPE_FIELD`].
pub const EVENT_DETAILS_FIELD: &str = "event_details";

/// The event field carrying the type of the event an `EventPattern` matches, which decides
/// which details that event has.
pub const EVENT_TYPE_FIELD: &str = "event_type";

/// The most bytes a resource-policy document may carry.
///
/// From the model: `PutResourcePolicy` documents "The maximum size of the policy is 4
/// KB" and `PolicySizeLimitExceededException` documents "The policy document is too
/// large. The limit is 4 KB." — 4 KB is AWS's own documented limit, not a bound this
/// emulator chose, and a document past it is refused with that modelled error.
pub const MAX_POLICY_DOCUMENT_BYTES: usize = 4 * 1024;

/// The `Version` values an IAM JSON policy document may declare.
///
/// Source: the IAM JSON policy language reference — `Version` "specifies the language
/// syntax rules" and admits the current `2012-10-17` and the legacy `2008-10-17`; any
/// other value makes the document malformed.
pub const POLICY_DOCUMENT_VERSIONS: &[&str] = &["2008-10-17", "2012-10-17"];

/// The top-level elements an IAM JSON policy document may carry.
///
/// Source: the IAM JSON policy language reference — a policy document is a JSON object
/// whose elements are `Version`, `Id` and `Statement`.
pub const POLICY_DOCUMENT_ELEMENTS: &[&str] = &["Version", "Id", "Statement"];

/// The `Effect` values an IAM policy statement may carry.
///
/// Source: the IAM JSON policy language reference — `Effect` is required and is either
/// `Allow` or `Deny`, matched case-sensitively.
pub const POLICY_STATEMENT_EFFECTS: &[&str] = &["Allow", "Deny"];

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str = "the com-amazonaws-rum-rum store is not mounted on this engine";

/// One RUM event a `PutRumEvents` call appended to an app monitor's buffer.
///
/// The event is kept as the ONE document the service enriched it into when the batch was
/// accepted ([`rum_event::event_document`]): the event's own members beside the batch's
/// context and the fields only the service knows, under the field names RUM publishes for
/// a RUM event. That document is what `GetAppMonitorData` reports (`EventDataList` is a
/// list of `JsonValue`) and what the CloudWatch Logs copy carries, so the per-batch
/// context is held IN it rather than beside it and no reader has to re-derive it.
#[derive(Debug, Clone)]
pub struct StoredRumEvent {
    /// The RUM event document ([`rum_event::event_document`]).
    pub event_json: String,
}

/// The position one event was appended at inside its app monitor's buffer, counted per
/// monitor over the monitor's whole life.
///
/// It is the key that buffer is ordered by, and so the key a page of `GetAppMonitorData`
/// resumes after rather than a count of the events a page has returned: a rank names ONE
/// delivered event whatever happens to the buffer afterwards, so a page boundary cannot
/// drift from the sequence it indexes when the buffer evicts its oldest entries
/// ([`AppMonitorRecord::record_events`]) or when the query a later page is evaluated
/// against selects a different set of events.
pub type EventRank = u64;

/// The identity of one extended-metrics destination of an app monitor: its
/// `MetricDestination` and, for an Evidently destination, the ARN of the experiment that
/// receives the metrics (the empty string for a `CloudWatch` destination, which has no
/// ARN).
///
/// This pair is what `PutRumMetricsDestination` creates-or-updates and what
/// `DeleteRumMetricsDestination` names — both take exactly `Destination` plus the
/// optional `DestinationArn` — so it is the key the destinations map is held under.
pub type MetricDestinationKey = (String, String);

/// The position a metric definition was created at, counted per destination: the key its
/// destination's definitions are held under, and the only order `BatchGetRumMetricDefinitions`
/// enumerates them in.
///
/// A definition's own `MetricDefinitionId` is a minted value, so ordering by it is arbitrary
/// with respect to the order the definitions were created in and differs from run to run,
/// while the listing a caller reads correlates a destination's definitions BY POSITION. The
/// rank is therefore what the store is keyed by, assigned once by
/// [`AppMonitorRecord::insert_metric_definition`] and never changed afterwards.
pub type MetricDefinitionRank = u64;

/// The text an optional string member declares, with an absent, empty or whitespace-only
/// value all read as "nothing declared".
///
/// ONE reading of an optional string member of the metrics calls of this service, shared by
/// the rules that JUDGE a request and by the canonical form the request is STORED in, so the
/// two cannot disagree about which requests carry the same value: a member a rule read as
/// naming nothing must not be stored as naming something, or two spellings of "nothing"
/// become two resources. Surrounding whitespace is not part of a value a request names
/// something by, and a value with nothing left names nothing at all — exactly what leaving
/// the member out names.
///
/// Used for the optional members a value naming nothing can actually reach a handler
/// through: `DestinationArn`, `IamRoleArn` ([`declared_arn`]) and
/// `MetricDefinitionRequest$EventPattern`, which is modelled `@length(min: 0, max: 4000)`.
pub fn declared_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

/// The ARN a request declares, with an absent, empty or whitespace-only value all read
/// as "no ARN".
///
/// ONE reading of the optional ARN members of the metrics calls of this service — the
/// `DestinationArn` of `PutRumMetricsDestination`, `DeleteRumMetricsDestination`,
/// `BatchCreateRumMetricDefinitions`, `BatchDeleteRumMetricDefinitions`,
/// `BatchGetRumMetricDefinitions` and `UpdateRumMetricDefinition`, and the `IamRoleArn`
/// the put writes through — placed beside [`metric_destination_key`], the map key this
/// reading decides, so the put and the delete, or the batch create and the batch read,
/// cannot drift on which stored destination a request addresses.
///
/// Surrounding whitespace is not part of an ARN: `DestinationArn` is modelled
/// `@length(min: 0, max: 2048)` with a pattern Smithy matches ANYWHERE in the value, and
/// `IamRoleArn` carries no length bound at all, so a value with whitespace around the ARN
/// reaches a handler. Whatever spelling arrives, the ARN is what is left once that
/// whitespace is gone, and a value with nothing left names no experiment and no role: it
/// is the same input as an absent one, and must not become a second destination. That is
/// [`declared_text`], the one reading this service applies to such a member.
pub fn declared_arn(arn: Option<&str>) -> Option<&str> {
    declared_text(arn)
}

/// The [`MetricDestinationKey`] for `destination` and `destination_arn`, with an absent
/// or empty ARN normalised to the same key, so one destination cannot be stored twice
/// under two spellings of "no ARN" — the ARN is read through [`declared_arn`], the one
/// reading every caller of this key already applies, which is idempotent, so a key is the
/// same whether the caller normalised the ARN first or not.
pub fn metric_destination_key(
    destination: &str,
    destination_arn: Option<&str>,
) -> MetricDestinationKey {
    (
        destination.to_string(),
        declared_arn(destination_arn).unwrap_or_default().to_string(),
    )
}

/// One persisted extended-metrics destination of an app monitor.
///
/// Mirrors the model's `MetricDestinationSummary`, which is the only shape any operation
/// reports a destination through, with `Destination` held non-optional: a stored
/// destination always has one (it is `@required` on the put).
#[derive(Debug, Clone)]
pub struct MetricDestinationRecord {
    /// `Destination`, a member of [`crate::ops::types::METRIC_DESTINATION_VALUES`].
    pub destination: String,
    /// `DestinationArn`: the Evidently experiment receiving the metrics, and `None` for
    /// a `CloudWatch` destination, which the model says must carry no ARN.
    pub destination_arn: Option<String>,
    /// `IamRoleArn`: the role RUM assumes to write to that experiment. `None` for a
    /// `CloudWatch` destination, which the model says must carry no role.
    pub iam_role_arn: Option<String>,
}

impl MetricDestinationRecord {
    /// This record's identity in the destinations map.
    pub fn key(&self) -> MetricDestinationKey {
        metric_destination_key(&self.destination, self.destination_arn.as_deref())
    }

    /// Project the stored record onto the model's `MetricDestinationSummary` — the one
    /// conversion `ListRumMetricsDestinations` goes through, so what a read reports is
    /// exactly what the put wrote.
    ///
    /// EVERY member of the summary is materialised, not only the ones this destination
    /// kind carries a value for: a destination that has no ARN and no role (a
    /// `CloudWatch` one) reports `DestinationArn` and `IamRoleArn` as the EMPTY STRING
    /// rather than leaving them absent, which is the value recorded behaviour reports and
    /// which the model admits — `DestinationArn` is `@length(min: 0, max: 2048)`, and
    /// both members are documented as describing the Evidently case only ("If the
    /// destination is Evidently, this specifies the ARN of the Evidently experiment…",
    /// "This field appears only when the destination is Evidently"). So a client reading
    /// either member to decide whether a destination is Evidently-backed finds a defined
    /// empty value instead of a missing key.
    pub fn to_summary(&self) -> MetricDestinationSummary {
        MetricDestinationSummary {
            destination: Some(self.destination.clone()),
            destination_arn: Some(self.destination_arn.clone().unwrap_or_default()),
            iam_role_arn: Some(self.iam_role_arn.clone().unwrap_or_default()),
        }
    }
}

/// One persisted metric definition of an app monitor's extended-metrics destination.
///
/// Mirrors the model's `MetricDefinition` — the only shape any operation reports a
/// definition through — with the two members the service always knows held non-optional:
/// a stored definition always has the `MetricDefinitionId` the service minted for it and
/// the `Name` the request named (both `@required` there). The rest is kept exactly as the
/// request carried it, including a member the destination kind ignores, so what a read
/// reports is the definition that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDefinitionRecord {
    /// The minted `MetricDefinitionId`, the identity `UpdateRumMetricDefinition` and
    /// `BatchDeleteRumMetricDefinitions` address one definition by.
    pub metric_definition_id: String,
    /// `Name`: the metric's name, an extended metric's from RUM's catalog or any name at
    /// all for a custom metric.
    pub name: String,
    /// `ValueKey`: the event field the metric value is read from.
    pub value_key: Option<String>,
    /// `UnitLabel`: the CloudWatch unit the metric is recorded in.
    pub unit_label: Option<String>,
    /// `DimensionKeys`: field path -> CloudWatch dimension name. A `BTreeMap`, so one
    /// stored definition projects in ONE stable key order however the request spelled it.
    pub dimension_keys: Option<BTreeMap<String, String>>,
    /// `EventPattern`: the JSON pattern an event has to match to feed this metric.
    pub event_pattern: Option<String>,
    /// `Namespace`: the custom namespace a custom metric is published to.
    pub namespace: Option<String>,
}

impl MetricDefinitionRecord {
    /// The record `request` is stored as, under the id `metric_definition_id`.
    ///
    /// The ONE conversion from the request shape, shared by the create and the update, so
    /// a definition written by either carries exactly the same members — and so an update
    /// REPLACES the whole definition under the id it keeps (`UpdateRumMetricDefinition`
    /// takes "the new definition that you want to use for this metric", not a patch).
    ///
    /// It is also the CANONICAL form a definition's identity is judged in
    /// ([`Self::defines`], [`Self::same_definition`]), so a member whose value declares
    /// NOTHING is held exactly as an absent one: the `EventPattern` through
    /// [`declared_text`], the reading [`metric_definition_refusal`] judges that member by,
    /// and a `DimensionKeys` map with no entry as no map, which is what the rules that read
    /// it already treat it as. Otherwise two spellings of "nothing" that the rules admit as
    /// the SAME definition would be stored as two, each taking another slot of the
    /// destination's capacity and needing its own delete.
    ///
    /// The remaining optional members are kept exactly as the request carried them: a value
    /// declaring nothing cannot reach a handler through them, because `ValueKey`,
    /// `UnitLabel` and `Namespace` are each modelled `@length(min: 1, …)` — a value that
    /// is left is a value that names something, whatever it spells.
    pub fn from_request(metric_definition_id: &str, request: &MetricDefinitionRequest) -> Self {
        Self {
            metric_definition_id: metric_definition_id.to_string(),
            name: request.name.clone(),
            value_key: request.value_key.clone(),
            unit_label: request.unit_label.clone(),
            dimension_keys: request
                .dimension_keys
                .as_ref()
                .filter(|keys| !keys.is_empty())
                .map(|keys| {
                    keys.iter()
                        .map(|(field, dimension)| (field.clone(), dimension.clone()))
                        .collect()
                }),
            event_pattern: declared_text(request.event_pattern.as_deref()).map(str::to_string),
            namespace: request.namespace.clone(),
        }
    }

    /// Whether this stored definition is the definition `request` defines — i.e. whether a
    /// request asking for `request` is asking for the definition already stored here.
    ///
    /// A definition IS the metric it defines: the members a request carries. The
    /// `MetricDefinitionId` is the handle the service minted so later calls can address the
    /// definition, so it is NOT part of what distinguishes one definition from another, and
    /// the comparison runs the request through [`Self::from_request`] rather than member by
    /// member, so a request and a stored record are compared in the ONE canonical form a
    /// definition is stored in (which is also why a `DimensionKeys` map written in a
    /// different order is the same definition). The two members that tell most definitions
    /// apart — the metric's name and its namespace — are compared first, directly, so a
    /// scan over a destination's definitions canonicalises only the request it may actually
    /// match rather than once per stored definition.
    pub fn defines(&self, request: &MetricDefinitionRequest) -> bool {
        self.name == request.name
            && self.namespace == request.namespace
            && *self == Self::from_request(&self.metric_definition_id, request)
    }

    /// Whether two REQUESTED definitions define the same metric, judged in the same
    /// canonical form [`Self::defines`] judges a stored one in — for the caller that has to
    /// decide whether one request names the same definition twice.
    pub fn same_definition(
        left: &MetricDefinitionRequest,
        right: &MetricDefinitionRequest,
    ) -> bool {
        // One id for BOTH sides, so what is compared is the definitions themselves and not
        // the handle the service would mint for them.
        const UNMINTED: &str = "";
        left.name == right.name
            && left.namespace == right.namespace
            && Self::from_request(UNMINTED, left) == Self::from_request(UNMINTED, right)
    }

    /// Project the stored record onto the model's `MetricDefinition` — the one conversion
    /// `BatchGetRumMetricDefinitions` goes through, so every read reports one definition
    /// the same way.
    ///
    /// `Namespace` is reported as the namespace the metric is PUBLISHED to, which is what
    /// a caller reads it back for (to locate the metric in CloudWatch). A custom metric
    /// carries the namespace its request named; a definition that named none is an
    /// extended metric (the kind rule every operation accepting a definition judges by,
    /// [`metric_definition_refusal`]), and extended metrics are published to RUM's own
    /// vended namespace — so the member is settled to [`EXTENDED_METRICS_NAMESPACE`]
    /// rather than reported absent, which would leave "an extended metric" and "namespace
    /// unknown" spelled the same way.
    pub fn to_metric_definition(&self) -> MetricDefinition {
        MetricDefinition {
            metric_definition_id: self.metric_definition_id.clone(),
            name: self.name.clone(),
            value_key: self.value_key.clone(),
            unit_label: self.unit_label.clone(),
            dimension_keys: self.dimension_keys.as_ref().map(|keys| {
                keys.iter()
                    .map(|(field, dimension)| (field.clone(), dimension.clone()))
                    .collect()
            }),
            event_pattern: self.event_pattern.clone(),
            namespace: Some(
                self.namespace
                    .clone()
                    .unwrap_or_else(|| EXTENDED_METRICS_NAMESPACE.to_string()),
            ),
        }
    }
}

/// The resource-based policy attached to one app monitor.
///
/// `PutResourcePolicy` documents "Each app monitor can have one resource-based policy",
/// so a monitor holds at most one of these, and it lives ON the monitor's record: a
/// policy cannot outlive the resource whose access it controls, so `DeleteAppMonitor`
/// takes it away with the monitor. It is also what the authorization evaluation consults
/// beside the caller's identity-based policies ([`authorization`]).
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    /// `PolicyDocument` in the canonical form the service stores it in
    /// ([`canonical_policy_document`]): the submitted document's content, with the
    /// whitespace between its JSON tokens dropped. Everything a caller controls inside
    /// the document — its elements, their order, their values — is kept as written, so
    /// what a read reports is the policy that was written.
    pub document: String,
    /// This revision's number: 1 for the first policy revision an app monitor ever
    /// stores and one more than the previous revision for every later successful write,
    /// which is what makes a revision ORDERED — a client can tell which of two
    /// revisions it holds is the later one. Rendered on the wire by
    /// [`Self::revision_id`]; written only through
    /// [`AppMonitorRecord::store_resource_policy`], so no two revisions of one monitor's
    /// policy can share a number.
    pub revision: u64,
    /// Whether a revision id has been ASSIGNED to this policy by a caller, i.e. whether
    /// some accepted write of it provided a `PolicyRevisionId`.
    ///
    /// `PutResourcePolicyRequest$PolicyRevisionId` makes that assignment STICKY: "When
    /// you assign a policy revision ID, then later requests about that policy will be
    /// rejected with an InvalidPolicyRevisionIdException error if they don't provide the
    /// correct current revision ID" — so once this is true a put must carry the current
    /// revision id, and one that carries none is refused rather than overwriting the
    /// policy ([`AppMonitorRecord::policy_revision_refusal_on_current`]).
    ///
    /// It is the CALLER's assignment the member names, which is why this is tracked at
    /// all rather than read off the mere existence of a revision: the service mints a
    /// revision number for every write ([`Self::revision`]), and a caller that has never
    /// provided one goes on replacing its policy with a put that provides none, each
    /// write answering the next revision number.
    ///
    /// It is carried from revision to revision by
    /// [`AppMonitorRecord::store_resource_policy`], because the assignment is about the
    /// POLICY and not about the one write that made it; it goes away with the policy
    /// itself, since a monitor that carries none has no current revision id for a
    /// request to provide.
    pub revision_assigned: bool,
}

impl ResourcePolicyRecord {
    /// `PolicyRevisionId` as the wire carries it: the revision number in decimal.
    ///
    /// `PolicyRevisionId` is modelled as a 1..255-character string with no pattern, and
    /// the value is the SERVICE's — a caller only ever passes one back to make a
    /// conditional request. The decimal rendering of the revision number fits those
    /// bounds for every number a `u64` holds.
    pub fn revision_id(&self) -> String {
        self.revision.to_string()
    }
}

/// One persisted app monitor.
///
/// Mirrors the model's `AppMonitor` structure, but with the members the service always
/// knows held as non-optional: a stored monitor always has a name, an id, an ARN, both
/// timestamps, a state, a platform, a `CwLogEnabled` flag, a configuration and a custom
/// events setting. `Domain`/`DomainList` and `DeobfuscationConfiguration` stay optional
/// because a request may omit them.
#[derive(Debug, Clone)]
pub struct AppMonitorRecord {
    pub name: String,
    /// The region of the endpoint the `CreateAppMonitor` request that created this
    /// monitor was signed for — the region that OWNS it.
    ///
    /// RUM is a regional service: a monitor exists in exactly one region, its name only
    /// has to be unique within that region, and no request signed for another region can
    /// see it. So the region is part of the resource's identity and is the first half of
    /// the key the record is stored under
    /// ([`ComAmazonawsRumRumState::app_monitors`]), not just a segment of the derived
    /// [`Self::arn`].
    pub region: String,
    /// The minted `AppMonitorId`: a UUID, as `AppMonitorId`'s 36-character pattern asks.
    pub id: String,
    /// `arn:aws:rum:<region>:<account>:appmonitor/<name>`, derived once at create time.
    /// It is the identity the tagging operations name a resource by: `ListTagsForResource`
    /// and `UntagResource` resolve a `ResourceArn` against this ONE stored value (see
    /// [`ComAmazonawsRumRumState::app_monitor_by_arn`] /
    /// [`ComAmazonawsRumRumState::app_monitor_by_arn_mut`]) rather than minting their own,
    /// so a tag call and a read of the monitor cannot disagree about which resource is
    /// meant. (`TagResource` is still scaffolded.)
    pub arn: String,
    pub domain: Option<String>,
    pub domain_list: Option<Vec<String>>,
    /// `Created` as the modelled 19-character `ISOTimestampString`.
    pub created: String,
    /// `LastModified` as the modelled 19-character `ISOTimestampString`, or `None` while
    /// this monitor has only ever been created.
    ///
    /// The member is "the date and time of the most recent changes to this app monitor's
    /// configuration", so a monitor nothing has changed yet HAS no such instant and the
    /// member is absent from every projection of it; it is set by the operations that
    /// change the configuration. Its presence is therefore the answer to "has this
    /// monitor been modified since it was created?".
    pub last_modified: Option<String>,
    /// `State`, a member of [`crate::ops::types::STATE_ENUM_VALUES`].
    pub state: String,
    /// `Platform`, a member of [`crate::ops::types::APP_MONITOR_PLATFORM_VALUES`].
    pub platform: String,
    /// Whether RUM copies this monitor's telemetry into CloudWatch Logs; what
    /// `DataStorage.CwLog` reports.
    pub cw_log_enabled: bool,
    /// The vended CloudWatch Logs group name RUM chose for THIS monitor, minted once by
    /// the call that created it ([`ComAmazonawsRumRumState::mint_cw_log_group`]) and never
    /// recomputed.
    ///
    /// It is held on the record because the service-chosen part of the name is STATE and
    /// not a function of the request: a reader cannot rebuild it from the monitor's name,
    /// so the name a read advertises ([`Self::cw_log_group`]) is the stored one, which is
    /// also the one the handlers PROVISION in CloudWatch Logs and deliver copies to. That
    /// keeps `DataStorage.CwLog.CwLogGroup` stable across reads, equal to the group that
    /// actually exists, and distinct per monitor.
    ///
    /// Every monitor carries one, whatever [`Self::cw_log_enabled`] says: a monitor
    /// created with delivery off is given the name it will advertise if an update later
    /// turns delivery on, so toggling delivery off and on again addresses the one group
    /// RUM vended for this monitor rather than vending a second.
    pub vended_log_group: String,
    pub app_monitor_configuration: AppMonitorConfiguration,
    pub custom_events: CustomEvents,
    /// `DeobfuscationConfiguration`, materialised for every monitor: a create that named
    /// none stores [`default_deobfuscation_configuration`] rather than leaving the member
    /// unset, so a read always reports the structure and its status.
    pub deobfuscation_configuration: DeobfuscationConfiguration,
    /// This monitor's tags: the map `CreateAppMonitor` seeds and `UntagResource` removes
    /// keys from, what `GetAppMonitor` reports as `AppMonitor$Tags` and what
    /// `ListTagsForResource` reports as its own `Tags`. A `BTreeMap` so the stored record
    /// iterates in one stable key order.
    pub tags: BTreeMap<String, String>,
    /// Events delivered by `PutRumEvents`, oldest first. Bounded by
    /// [`MAX_EVENTS_PER_APP_MONITOR`]; append through [`Self::record_events`] and read
    /// through [`Self::events_from`], which pairs each event with its [`EventRank`].
    pub events: VecDeque<StoredRumEvent>,
    /// How many events this monitor has ever accepted: the source of the next
    /// [`EventRank`] ([`Self::record_events`]).
    ///
    /// It counts DELIVERIES, not the events the buffer currently holds, so the eviction
    /// that bounds the buffer does not lower it — a rank a reader resumed after must never
    /// come to name a later event than it did when it was issued.
    pub accepted_events: EventRank,
    /// The extended-metrics destinations `PutRumMetricsDestination` created or updated
    /// for this monitor, keyed by [`MetricDestinationKey`]. A `BTreeMap` so
    /// `ListRumMetricsDestinations` pages in a stable, resumable order. They live ON the
    /// monitor record because a destination belongs to one app monitor: deleting the
    /// monitor drops its destinations with it. Bounded by
    /// [`MAX_METRIC_DESTINATIONS_PER_APP_MONITOR`].
    pub metric_destinations: BTreeMap<MetricDestinationKey, MetricDestinationRecord>,
    /// The metric definitions stored for each of this monitor's extended-metrics
    /// destinations: keyed by the SAME [`MetricDestinationKey`] the destinations map uses,
    /// and inside that by the destination's own CREATION RANK — the position the
    /// definition was created at, counted per destination
    /// ([`Self::insert_metric_definition`]).
    ///
    /// The rank, and not the `MetricDefinitionId`, is the inner key because a listing of a
    /// destination's definitions enumerates them in the order they were CREATED: the id is
    /// a minted value whose ordering is arbitrary with respect to that, so a map keyed by
    /// it would report position N of the listing as a different definition on every run.
    /// A `BTreeMap` over the rank is that creation order, and it is resumable, so
    /// `BatchGetRumMetricDefinitions` pages through it with a range query.
    ///
    /// Definitions hang off the destination they were created under because that is the
    /// resource they belong to — "You must have already created a destination for the
    /// metrics before you send them" — so they go away with it
    /// ([`Self::remove_metric_destination`]) and with the monitor. Each inner map is
    /// bounded by [`MAX_METRIC_DEFINITIONS_PER_DESTINATION`].
    pub metric_definitions:
        BTreeMap<MetricDestinationKey, BTreeMap<MetricDefinitionRank, MetricDefinitionRecord>>,
    /// This monitor's one resource-based policy, or `None` while it carries none — the
    /// state `GetResourcePolicy` and `DeleteResourcePolicy` answer
    /// `PolicyNotFoundException` for, and what the authorization evaluation reads beside
    /// the caller's identity-based policies.
    pub resource_policy: Option<ResourcePolicyRecord>,
    /// How many resource-policy revisions this monitor has ever stored: the source of
    /// the next revision number ([`Self::store_resource_policy`]). It counts WRITES, not
    /// the policy currently held, so it is not reset by `DeleteResourcePolicy` — a
    /// revision id a caller read before a delete must never pass the conditional check
    /// of a policy stored after it.
    pub policy_revisions: u64,
}

impl AppMonitorRecord {
    /// The name of the CloudWatch Logs group this monitor's data is copied into, or
    /// `None` when `CwLogEnabled` is false — a monitor that stores no copy has no group
    /// (`CwLog$CwLogGroup` is "The name of the log group where the copies are stored").
    pub fn cw_log_group(&self) -> Option<String> {
        self.cw_log_enabled
            .then(|| self.vended_log_group.clone())
    }

    /// The internet domain this monitor's application has authority over, or `None` for a
    /// monitor that names none (a platform whose application is served from no internet
    /// domain — see [`platform_requires_domain`]).
    ///
    /// A monitor's domain authority is held in exactly ONE of the two modelled forms
    /// ("either the domain or the domain list", see [`domain_choice_refusal`]), so this is
    /// the one reading of that choice: `Domain` when the monitor was given one, and
    /// otherwise the first entry of `DomainList` — the first of the up-to-five domains
    /// `AppMonitorDomainList` admits, which is the one the stored record names first. It is
    /// what the data plane reports as the `domain` of the telemetry a batch delivered to
    /// this monitor whose client reported none itself (see [`rum_event`]).
    pub fn domain_authority(&self) -> Option<&str> {
        self.domain.as_deref().or_else(|| {
            self.domain_list
                .as_ref()
                .and_then(|domains| domains.first())
                .map(String::as_str)
        })
    }

    /// The resource policy document this monitor carries, for the authorization
    /// evaluation to consult.
    pub fn resource_policy_document(&self) -> Option<String> {
        self.resource_policy
            .as_ref()
            .map(|policy| policy.document.clone())
    }

    /// This monitor's tags as the model's `TagMap` — the ONE projection every operation
    /// that reports tags (`GetAppMonitor` inside `AppMonitor$Tags`,
    /// `ListTagsForResource` as its own `Tags` member) goes through, so two readers
    /// cannot report one monitor's tags differently.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Project the stored record onto the model's `AppMonitor` shape — the one
    /// conversion `GetAppMonitor` goes through, so what a read reports is exactly what
    /// the create and the updates wrote.
    pub fn to_app_monitor(&self) -> AppMonitor {
        AppMonitor {
            name: Some(self.name.clone()),
            domain: self.domain.clone(),
            domain_list: self.domain_list.clone(),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: self.last_modified.clone(),
            tags: Some(self.tag_map()),
            state: Some(self.state.clone()),
            app_monitor_configuration: Some(self.app_monitor_configuration.clone()),
            data_storage: Some(DataStorage {
                cw_log: Some(CwLog {
                    cw_log_enabled: Some(self.cw_log_enabled),
                    cw_log_group: self.cw_log_group(),
                }),
            }),
            custom_events: Some(self.custom_events.clone()),
            deobfuscation_configuration: Some(self.deobfuscation_configuration.clone()),
            platform: Some(self.platform.clone()),
        }
    }

    /// Project the stored record onto the model's `AppMonitorSummary` — the subset
    /// `ListAppMonitors` reports, read off the same record `GetAppMonitor` reads.
    pub fn to_summary(&self) -> AppMonitorSummary {
        AppMonitorSummary {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            created: Some(self.created.clone()),
            last_modified: self.last_modified.clone(),
            state: Some(self.state.clone()),
            platform: Some(self.platform.clone()),
        }
    }

    /// Remove the destination the `(Destination, DestinationArn)` pair
    /// `destination`/`destination_arn` names — the identity
    /// `DeleteRumMetricsDestination` addresses one by. `None` when this monitor holds no
    /// such destination, which the caller renders as `ResourceNotFoundException`. Keyed
    /// through [`metric_destination_key`], so the delete, the put and the listing cannot
    /// disagree about which destination a request means.
    pub fn remove_metric_destination(
        &mut self,
        destination: &str,
        destination_arn: Option<&str>,
    ) -> Option<MetricDestinationRecord> {
        let key = metric_destination_key(destination, destination_arn);
        // A metric definition belongs to the destination it was created under, so the
        // destination takes its definitions with it rather than leaving them stored under
        // a destination that no longer exists.
        self.metric_definitions.remove(&key);
        self.metric_destinations.remove(&key)
    }

    /// Whether this monitor has the extended-metrics destination the
    /// `(Destination, DestinationArn)` pair names.
    ///
    /// The precondition every metric-definition call shares: the model's
    /// `BatchCreateRumMetricDefinitions` documents "You must have already created a
    /// destination for the metrics before you send them", so a definition is created,
    /// read and updated under a destination `PutRumMetricsDestination` stored, and a pair
    /// naming none addresses no resource.
    pub fn has_metric_destination(&self, destination: &str, destination_arn: Option<&str>) -> bool {
        self.metric_destinations
            .contains_key(&metric_destination_key(destination, destination_arn))
    }

    /// The definitions stored for that destination, for a caller that only REPORTS them
    /// (`BatchGetRumMetricDefinitions`). `None` when the destination holds none.
    pub fn metric_definitions_for(
        &self,
        destination: &str,
        destination_arn: Option<&str>,
    ) -> Option<&BTreeMap<MetricDefinitionRank, MetricDefinitionRecord>> {
        self.metric_definitions
            .get(&metric_destination_key(destination, destination_arn))
    }

    /// How many definitions that destination holds — what
    /// [`MAX_METRIC_DEFINITIONS_PER_DESTINATION`] is checked against.
    pub fn metric_definition_count(
        &self,
        destination: &str,
        destination_arn: Option<&str>,
    ) -> usize {
        self.metric_definitions_for(destination, destination_arn)
            .map_or(0, BTreeMap::len)
    }

    /// The definition this destination ALREADY stores for `request`, or `None` while it
    /// stores none.
    ///
    /// What makes the create of a metric definition idempotent, as the model declares it:
    /// the identity of a definition is the definition itself ([`MetricDefinitionRecord::defines`]),
    /// so a request for a definition the destination already holds names that stored
    /// definition — under the id it already carries — rather than a second one. Keyed
    /// through the same [`metric_destination_key`] the create, the read and the update use,
    /// so the lookup cannot consult a different destination from the one a create writes to.
    /// Every writer of the map keeps that identity single-valued — the create resolves a
    /// requested definition through this lookup, and the update refuses a write that would
    /// store a definition another id of the destination already defines — so this answers
    /// THE definition of the requested metric, not one of several.
    pub fn metric_definition_defining(
        &self,
        destination: &str,
        destination_arn: Option<&str>,
        request: &MetricDefinitionRequest,
    ) -> Option<&MetricDefinitionRecord> {
        self.metric_definitions_for(destination, destination_arn)?
            .values()
            .find(|stored| stored.defines(request))
    }

    /// The ONE stored definition the `(Destination, DestinationArn, MetricDefinitionId)`
    /// triple names, for the caller that REPLACES it (`UpdateRumMetricDefinition`).
    /// `None` when that destination holds no definition under that id, which the caller
    /// renders as `ResourceNotFoundException`. Nothing is stored on the `None` path, so a
    /// refused update leaves the state exactly as it was.
    pub fn metric_definition_mut(
        &mut self,
        destination: &str,
        destination_arn: Option<&str>,
        metric_definition_id: &str,
    ) -> Option<&mut MetricDefinitionRecord> {
        // The map is keyed by creation rank, so a caller's id is resolved by the id each
        // record CARRIES — the same single-valued identity the map is written under, since
        // no two ids this state mints are equal.
        self.metric_definitions
            .get_mut(&metric_destination_key(destination, destination_arn))?
            .values_mut()
            .find(|stored| stored.metric_definition_id == metric_definition_id)
    }

    /// Remove the ONE definition the `(Destination, DestinationArn, MetricDefinitionId)`
    /// triple names, for the caller that DELETES it
    /// (`BatchDeleteRumMetricDefinitions`). `None` when that destination holds no
    /// definition under that id, which the caller reports as that id's own error while the
    /// rest of the batch is still deleted. Keyed through the same
    /// [`metric_destination_key`] the create and the read use, so a delete cannot address a
    /// different destination from the one a create stored the definition under.
    pub fn remove_metric_definition(
        &mut self,
        destination: &str,
        destination_arn: Option<&str>,
        metric_definition_id: &str,
    ) -> Option<MetricDefinitionRecord> {
        let held = self
            .metric_definitions
            .get_mut(&metric_destination_key(destination, destination_arn))?;
        // The rank the id is held at, read from the record's own id, then dropped: the
        // ranks of the definitions that remain are untouched, so a delete cannot move a
        // surviving definition's position in the listing.
        let rank = *held
            .iter()
            .find(|(_, stored)| stored.metric_definition_id == metric_definition_id)
            .map(|(rank, _)| rank)?;
        held.remove(&rank)
    }

    /// Store `record` as the NEWEST definition of that destination, at the next creation
    /// rank.
    ///
    /// The ONE writer of [`Self::metric_definitions`], so creation order is decided in one
    /// place: the rank is one past the highest the destination holds, which is past every
    /// definition already there whatever has been deleted meanwhile, and ranks already
    /// taken are never reused while their definition is held — so a definition's position
    /// in the listing is fixed when it is created and never moves afterwards.
    pub fn insert_metric_definition(
        &mut self,
        destination: &str,
        destination_arn: Option<&str>,
        record: MetricDefinitionRecord,
    ) {
        let held = self
            .metric_definitions
            .entry(metric_destination_key(destination, destination_arn))
            .or_default();
        let rank = held
            .keys()
            .next_back()
            .map_or(0, |highest| highest.saturating_add(1));
        held.insert(rank, record);
    }

    /// The revision id of this monitor's stored resource policy, or `None` while it
    /// carries no policy.
    pub fn policy_revision_id(&self) -> Option<String> {
        self.resource_policy
            .as_ref()
            .map(ResourcePolicyRecord::revision_id)
    }

    /// Whether a revision id has been ASSIGNED to the policy this monitor currently
    /// carries — see [`ResourcePolicyRecord::revision_assigned`]. False while the monitor
    /// carries no policy: there is then no current revision id for a request to provide.
    pub fn policy_revision_assigned(&self) -> bool {
        self.resource_policy
            .as_ref()
            .is_some_and(|policy| policy.revision_assigned)
    }

    /// Store `document` as this monitor's resource policy, as the NEXT revision of it,
    /// and report the record just written. `provided_revision_id` is the
    /// `PolicyRevisionId` the accepted request carried, if any.
    ///
    /// The ONE writer of [`Self::resource_policy`] and of [`Self::policy_revisions`], so
    /// every accepted `PutResourcePolicy` advances the revision by exactly one: the first
    /// policy a monitor stores is revision 1, the write that replaces it is revision 2,
    /// and so on. The document is stored in its canonical form
    /// ([`canonical_policy_document`]): its content, with the caller's insignificant
    /// whitespace dropped.
    ///
    /// A write that provided a revision id ASSIGNS one to the policy, and the assignment
    /// outlives that write: the new revision inherits it from the revision it replaces,
    /// so a caller that once made a conditional write keeps the protection the member
    /// documents for every later revision of the same policy.
    pub fn store_resource_policy(
        &mut self,
        document: &str,
        provided_revision_id: Option<&str>,
    ) -> &ResourcePolicyRecord {
        self.policy_revisions = self.policy_revisions.saturating_add(1);
        let stored = ResourcePolicyRecord {
            document: canonical_policy_document(document),
            revision: self.policy_revisions,
            revision_assigned: provided_revision_id.is_some() || self.policy_revision_assigned(),
        };
        self.resource_policy.insert(stored)
    }

    /// Why a request that must PROVIDE the correct current `PolicyRevisionId` cannot act
    /// on this monitor's policy, or `None` when it can.
    ///
    /// The rule `PutResourcePolicyRequest$PolicyRevisionId` states: "You can provide the
    /// revision ID of your existing policy to make mutating requests against that
    /// policy. When you assign a policy revision ID, then later requests about that
    /// policy will be rejected with an InvalidPolicyRevisionIdException error if they
    /// don't provide the correct current revision ID." The refusal is conditioned on the
    /// request not providing the CORRECT CURRENT id, so once a revision id has been
    /// assigned ([`Self::policy_revision_assigned`]) a request that provides none is
    /// refused exactly like one that provides a stale one: omitting the member is not a
    /// way around the condition, which is what makes the read-revision-then-write loop
    /// safe against a stale retry or a second writer.
    ///
    /// Until an id has been assigned a request may still name one, and is then
    /// conditional on it being the current one — including when there is no policy at
    /// all, where no id can be current.
    ///
    /// Both ways the condition can fail are the SAME refusal of the same operation, so
    /// both carry that operation's own fixed wording
    /// ([`put_policy_revision_mismatch`]).
    pub fn policy_revision_refusal_on_current(
        &self,
        provided: Option<&str>,
    ) -> Option<InvalidPolicyRevisionIdException> {
        let satisfied = match provided {
            Some(provided) => self.policy_revision_id().as_deref() == Some(provided),
            None => !self.policy_revision_assigned(),
        };
        (!satisfied).then(put_policy_revision_mismatch)
    }

    /// Why the `PolicyRevisionId` a request PROVIDES stops it acting on this monitor's
    /// policy, or `None` when it does not.
    ///
    /// The weaker of the two conditional rules, and the one
    /// `DeleteResourcePolicyRequest$PolicyRevisionId` states: "Provide a
    /// PolicyRevisionId to ensure an atomic delete operation. If the revision ID that you
    /// provide doesn't match the latest policy revision ID, the request will be rejected
    /// …". The refusal is conditioned on the id the request PROVIDES, so a request that
    /// provides none asks for no atomicity and is unconditional, whatever the policy's
    /// history.
    ///
    /// The refusal carries the delete's OWN wording
    /// ([`delete_policy_revision_mismatch`]), which the service words differently from the
    /// put's.
    pub fn policy_revision_refusal_on_provided(
        &self,
        provided: Option<&str>,
    ) -> Option<InvalidPolicyRevisionIdException> {
        let provided = provided?;
        (self.policy_revision_id().as_deref() != Some(provided))
            .then(delete_policy_revision_mismatch)
    }

    /// Append events to this monitor's buffer, dropping the oldest once the buffer is at
    /// [`MAX_EVENTS_PER_APP_MONITOR`].
    ///
    /// Each appended event takes the next [`EventRank`], so the ranks the buffer holds are
    /// the order the data plane delivered its events in and a rank is never reused — not
    /// even for the slot an eviction just freed.
    pub fn record_events(&mut self, events: impl IntoIterator<Item = StoredRumEvent>) {
        for event in events {
            while self.events.len() >= MAX_EVENTS_PER_APP_MONITOR {
                self.events.pop_front();
            }
            self.events.push_back(event);
            self.accepted_events = self.accepted_events.saturating_add(1);
        }
    }

    /// The [`EventRank`] of the oldest event this monitor's buffer still holds, which is
    /// also how many of its events have been evicted: the buffer keeps the LAST
    /// [`MAX_EVENTS_PER_APP_MONITOR`] events accepted, so every rank below this one names
    /// an event the monitor no longer holds.
    fn oldest_event_rank(&self) -> EventRank {
        self.accepted_events.saturating_sub(self.buffered_events())
    }

    /// How many events the buffer currently holds, as an [`EventRank`] can count them.
    fn buffered_events(&self) -> EventRank {
        EventRank::try_from(self.events.len()).unwrap_or(EventRank::MAX)
    }

    /// This monitor's buffered events with their ranks, oldest first, starting at the
    /// oldest held event that ranks strictly AFTER `resume_after` — or at the oldest held
    /// event when there is nothing to resume after.
    ///
    /// The one way a reader walks the buffer, so paging it is a RANGE over the ranks rather
    /// than a count of the entries earlier pages returned: every event is walked under the
    /// rank it was delivered at, so a resume point names the same place in the sequence
    /// however many events have been appended or evicted since it was handed out, and an
    /// event is walked at most once across the pages of one query.
    pub fn events_from(
        &self,
        resume_after: Option<EventRank>,
    ) -> impl Iterator<Item = (EventRank, &StoredRumEvent)> + '_ {
        let oldest = self.oldest_event_rank();
        // Every rank up to and including the one the resume point names has already been
        // walked or evicted, so the walk starts at the first entry held above it; a resume
        // point below what the buffer still holds leaves nothing of it to skip.
        let skipped = match resume_after {
            Some(after) => usize::try_from(after.saturating_add(1).saturating_sub(oldest))
                .unwrap_or(usize::MAX)
                .min(self.events.len()),
            None => 0,
        };
        let first = oldest.saturating_add(EventRank::try_from(skipped).unwrap_or(EventRank::MAX));
        self.events.range(skipped..).enumerate().map(move |(step, event)| {
            (
                first.saturating_add(EventRank::try_from(step).unwrap_or(EventRank::MAX)),
                event,
            )
        })
    }
}

/// Per-service state for com-amazonaws-rum-rum. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRumRumState {
    /// Every app monitor that currently exists, keyed by the region that owns it
    /// ([`AppMonitorRecord::region`]) and, under that, by its `Name` — which is unique
    /// per account AND region, since every RUM endpoint is regional and a monitor of one
    /// region is invisible from another. `BTreeMap`s so `ListAppMonitors` pages the one
    /// region it is signed for in a stable, resumable order. Each region's map is bounded
    /// by [`MAX_APP_MONITORS`]; read it through [`Self::app_monitor`] /
    /// [`Self::app_monitor_mut`] and write it through [`Self::insert_app_monitor`] /
    /// [`Self::remove_app_monitor`] so no two operations can disagree about which region
    /// owns a monitor and the id index below cannot drift from it.
    pub app_monitors: BTreeMap<String, BTreeMap<String, AppMonitorRecord>>,
    /// Secondary index `Id` -> `(region, Name)`, so the data-plane `PutRumEvents` — which
    /// names a monitor by its minted id, not its name — resolves one in O(1) without
    /// scanning. The region is part of the entry because it is part of the monitor's
    /// identity: an id is resolved WITHIN the region the request was signed for.
    pub monitor_ids: HashMap<String, (String, String)>,
    /// The names a create has CLAIMED but not stored yet, keyed by region exactly as
    /// `app_monitors` is — the holds [`hold_app_monitor_name`] takes.
    ///
    /// A create that provisions the monitor's vended CloudWatch Logs group has to release
    /// this store's lock to make that in-process call, so without a hold the name and the
    /// monitor slot it checked could be taken by a concurrent create while it is away, and
    /// the refusal would land after the group had already been created in another service.
    /// A claimed name is therefore counted exactly like a stored one — by the duplicate
    /// name conflict, by the per-region allowance and by the region-bucket bound — and is
    /// dropped again when the create either stores its record or gives up
    /// ([`AppMonitorHold`]), so a name is never claimed for longer than one call.
    app_monitor_holds: BTreeMap<String, BTreeSet<String>>,
    /// Per-state seed the minted ids are derived from, set on the first mint from the
    /// nanosecond clock so two runs of the emulator do not hand out the same ids. Only
    /// ever read by [`Self::mint_app_monitor_id`]; it is NOT an id itself.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state. Same note: not
    /// an id, and never reported.
    next_id_seq: u64,
}

impl ComAmazonawsRumRumState {
    /// Mint a fresh `AppMonitorId` that no stored monitor holds.
    ///
    /// The modelled shape is a UUID —
    /// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
    /// length 36..36 — so the id is 16 bytes of a SHA-256 digest of the clock-derived
    /// seed and the counter, laid out as a version-4, RFC 4122 variant UUID (the version
    /// and variant nibbles are forced, as a v4 generator does). The digest gives ids no
    /// caller can guess from a previous one, the seed keeps ids from different runs
    /// apart, the counter keeps ids within one run unique, and the loop re-draws rather
    /// than ever handing back an id already in use. The caller must store the record
    /// under the returned id while still holding the same state lock, or the id it
    /// returned names nothing.
    pub fn mint_app_monitor_id(&mut self) -> String {
        loop {
            let candidate = self.mint_id(APP_MONITOR_ID_DOMAIN);
            if !self.monitor_ids.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint the vended CloudWatch Logs group name for the app monitor named
    /// `app_monitor_name` — the ONE place a monitor's `CwLogGroup` is chosen.
    ///
    /// The name is the documented vended-logs prefix and the monitor's name, then a
    /// service-chosen suffix ([`cw_log_group_name`]): the hex characters of a token drawn
    /// from this state's own id machinery, so the suffix is a value only the service knows
    /// and no caller can derive from the request or from another monitor's group. The
    /// caller stores the result on the record it creates
    /// ([`AppMonitorRecord::vended_log_group`]); from then on the name is READ, never
    /// recomputed.
    ///
    /// Two monitors cannot be handed one name: the name embeds the monitor's own name,
    /// which is unique within the region that owns it, and each call advances this state's
    /// counter, so even two monitors of one name in different regions draw different
    /// suffixes.
    pub fn mint_cw_log_group(&mut self, app_monitor_name: &str) -> String {
        let token = self.mint_id(CW_LOG_GROUP_DOMAIN);
        let suffix: String = token
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(CW_LOG_GROUP_SUFFIX_LEN)
            .collect();
        cw_log_group_name(app_monitor_name, &suffix)
    }

    /// Mint a fresh `MetricDefinitionId`.
    ///
    /// `MetricDefinitionId` is modelled as a 1..255-character string with no pattern and
    /// the value is the SERVICE's — a caller only ever passes back one it was handed — so
    /// the id is drawn the way an app monitor's is: 16 bytes of a SHA-256 digest of this
    /// state's clock-derived seed and its monotonic counter, laid out as a version-4,
    /// RFC 4122 variant UUID. The counter advances on every mint and this id space is its
    /// own ([`METRIC_DEFINITION_ID_DOMAIN`]), so no two ids this state hands out — of this
    /// kind or of any other — are equal, and no caller can guess one from another. The
    /// caller must store the definition under the returned id while still holding the
    /// same state lock, or the id it returned names nothing.
    pub fn mint_metric_definition_id(&mut self) -> String {
        self.mint_id(METRIC_DEFINITION_ID_DOMAIN)
    }

    /// One id, drawn from the id space `domain`: the next value of this state's monotonic
    /// counter, digested with the seed the counter is drawn under
    /// ([`uuid_v4_shaped`]). The ONE place the seed is initialised and the counter is
    /// advanced, so two id spaces of this state cannot drift apart or draw the same
    /// `(seed, sequence)` pair twice.
    fn mint_id(&mut self, domain: &str) -> String {
        if self.id_seed == 0 {
            self.id_seed = nanos_since_epoch() as u64;
        }
        self.next_id_seq = self.next_id_seq.wrapping_add(1);
        uuid_v4_shaped(domain, self.id_seed, self.next_id_seq)
    }

    /// Store `record` in the region it names, keeping the `Id` -> `(region, Name)` index
    /// in step with the primary map.
    pub fn insert_app_monitor(&mut self, record: AppMonitorRecord) {
        self.monitor_ids.insert(
            record.id.clone(),
            (record.region.clone(), record.name.clone()),
        );
        self.app_monitors
            .entry(record.region.clone())
            .or_default()
            .insert(record.name.clone(), record);
    }

    /// The monitor named `name` in `region`, for a caller that only REPORTS it.
    ///
    /// The ONE name-addressed lookup of this service: a monitor belongs to the region its
    /// create was signed for, so a name is resolved within THAT region's monitors and a
    /// request signed for any other region finds nothing — which every caller renders as
    /// `ResourceNotFoundException`.
    pub fn app_monitor(&self, region: &str, name: &str) -> Option<&AppMonitorRecord> {
        self.app_monitors.get(region)?.get(name)
    }

    /// The same lookup for a caller that MUTATES the monitor it resolves, so a read and a
    /// write of one name can never disagree about which region owns it.
    pub fn app_monitor_mut(&mut self, region: &str, name: &str) -> Option<&mut AppMonitorRecord> {
        self.app_monitors.get_mut(region)?.get_mut(name)
    }

    /// Whether `region` already holds a monitor named `name` — the condition a create's
    /// duplicate-name conflict is taken on, and the reason a name only has to be unique
    /// within one region.
    pub fn contains_app_monitor(&self, region: &str, name: &str) -> bool {
        self.app_monitor(region, name).is_some()
    }

    /// Whether a create of `region` has already CLAIMED `name` and not stored it yet
    /// ([`Self::app_monitor_holds`]). The same duplicate-name conflict as a stored
    /// monitor: the name is in use by the call holding it until that call is done with it.
    fn app_monitor_name_claimed(&self, region: &str, name: &str) -> bool {
        self.app_monitor_holds
            .get(region)
            .is_some_and(|claimed| claimed.contains(name))
    }

    /// How many names creates of `region` are currently holding — monitors this store has
    /// promised a slot to, which the per-region allowance has to count alongside the
    /// stored ones.
    fn claimed_app_monitor_count(&self, region: &str) -> usize {
        self.app_monitor_holds
            .get(region)
            .map_or(0, ::std::collections::BTreeSet::len)
    }

    /// How many monitors `region` holds: what [`MAX_APP_MONITORS`] bounds, counted per
    /// region because that is the scope AWS's own app-monitor quota is held at.
    pub fn app_monitor_count(&self, region: &str) -> usize {
        self.app_monitors
            .get(region)
            .map_or(0, ::std::collections::BTreeMap::len)
    }

    /// Whether storing a monitor in `region` would open a new per-region bucket beyond
    /// [`MAX_REGIONS`]. The region comes from the request's own credential scope, so the
    /// number of buckets is capped as well as the monitors in each.
    ///
    /// A region a create is holding a name in counts as a bucket already: that create is
    /// going to store its monitor there, so the bound is read the same way whether the
    /// monitor is stored yet or still in flight.
    pub fn region_bucket_exhausted(&self, region: &str) -> bool {
        !self.region_bucket_open(region) && self.open_region_buckets() >= MAX_REGIONS
    }

    /// Whether `region` already has a bucket of this store — one holding a monitor, or one
    /// a create is holding a name in.
    fn region_bucket_open(&self, region: &str) -> bool {
        self.app_monitors.contains_key(region) || self.app_monitor_holds.contains_key(region)
    }

    /// How many regions have a bucket, counting each region once however it was opened.
    fn open_region_buckets(&self) -> usize {
        self.app_monitors.len()
            + self
                .app_monitor_holds
                .keys()
                .filter(|region| !self.app_monitors.contains_key(*region))
                .count()
    }

    /// Claim `name` in `region` for the create that is about to store it
    /// ([`hold_app_monitor_name`], the only caller, which has just decided the name is
    /// free).
    fn claim_app_monitor_name(&mut self, region: &str, name: &str) {
        self.app_monitor_holds
            .entry(region.to_string())
            .or_default()
            .insert(name.to_string());
    }

    /// Drop the claim on `name` in `region`, emptying the region's entry with it so the
    /// region-bucket count does not keep counting a bucket nothing is in. Called from
    /// [`AppMonitorHold`] and nowhere else, so a hold is released exactly once.
    fn release_app_monitor_name(&mut self, region: &str, name: &str) {
        if let Some(claimed) = self.app_monitor_holds.get_mut(region) {
            claimed.remove(name);
            if claimed.is_empty() {
                self.app_monitor_holds.remove(region);
            }
        }
    }

    /// The monitors `region` owns in `Name` order, resuming strictly after `after` when a
    /// page token named one — the range query `ListAppMonitors` pages over.
    ///
    /// A listing reports the monitors of the region it was signed for and no others, so
    /// the range is taken over that region's map alone: resuming still costs
    /// O(log n) + O(page), and a page boundary cannot drift from the map it indexes.
    pub fn app_monitors_from<'state>(
        &'state self,
        region: &str,
        after: Option<&'state str>,
    ) -> impl Iterator<Item = &'state AppMonitorRecord> {
        let start = match after {
            Some(after) => ::std::ops::Bound::Excluded(after),
            None => ::std::ops::Bound::Unbounded,
        };
        self.app_monitors
            .get(region)
            .into_iter()
            .flat_map(move |monitors| monitors.range::<str, _>((start, ::std::ops::Bound::Unbounded)))
            .map(|(_, record)| record)
    }

    /// Remove the monitor named `name` in `region` — with its tags and its event buffer,
    /// which live on the record — and drop its id from the index. `None` when that region
    /// holds no monitor of that name, which every caller renders as
    /// `ResourceNotFoundException`.
    pub fn remove_app_monitor(&mut self, region: &str, name: &str) -> Option<AppMonitorRecord> {
        let monitors = self.app_monitors.get_mut(region)?;
        let removed = monitors.remove(name)?;
        if monitors.is_empty() {
            self.app_monitors.remove(region);
        }
        self.monitor_ids.remove(&removed.id);
        Some(removed)
    }

    /// The monitor of `region` whose minted id is `id`, for a caller that mutates it (the
    /// data-plane `PutRumEvents`). The id index and the primary map are kept in step by
    /// the two writers above, so this is the ONE place the two-step lookup lives.
    ///
    /// The index entry carries the region that owns the monitor, so an id minted in
    /// another region resolves to nothing here: the data plane is as regional as the
    /// control plane.
    pub fn app_monitor_by_id_mut(&mut self, region: &str, id: &str) -> Option<&mut AppMonitorRecord> {
        let (owner, name) = self.monitor_ids.get(id)?.clone();
        (owner == region)
            .then(|| self.app_monitors.get_mut(&owner)?.get_mut(&name))
            .flatten()
    }

    /// The same lookup for a caller that only READS the monitor — the authorization
    /// evaluation, which needs the monitor's ARN and its resource policy before any write
    /// is decided on.
    pub fn app_monitor_by_id(&self, region: &str, id: &str) -> Option<&AppMonitorRecord> {
        let (owner, name) = self.monitor_ids.get(id)?;
        (owner == region)
            .then(|| self.app_monitors.get(owner)?.get(name))
            .flatten()
    }

    /// The monitor of `region` whose ARN is exactly `arn`, for a caller that mutates it
    /// (the tagging operations, which name a resource by `ResourceArn` and by nothing
    /// else).
    ///
    /// The search is over the monitors `region` owns, and the comparison is against
    /// [`AppMonitorRecord::arn`] in full — partition, service, region, account and
    /// resource segment — because that whole string is the resource's identity: an ARN
    /// that differs in any component names a DIFFERENT resource, which this emulator does
    /// not hold here, so the caller renders `None` as `ResourceNotFoundException` (see
    /// [`tagged_resource_not_found`]). Scoping the search to the signed region is what
    /// makes the ARN-addressed resolution agree with the name-addressed one: a monitor of
    /// another region is no more reachable by its ARN than by its name. App monitors are
    /// the only taggable RUM resource (`TagResource`: "Currently, the only resources that
    /// can be tagged app monitors"), so this one lookup covers every ARN a tag call can
    /// name.
    pub fn app_monitor_by_arn_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut AppMonitorRecord> {
        self.app_monitors
            .get_mut(region)?
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// The monitor of `region` whose ARN is exactly `arn`, for a caller that only REPORTS
    /// it (`ListTagsForResource`).
    ///
    /// The same identity comparison as [`Self::app_monitor_by_arn_mut`] — the two differ
    /// only in mutability, so a tag read and a tag write can never disagree about which
    /// resource one `ResourceArn` names.
    pub fn app_monitor_by_arn(&self, region: &str, arn: &str) -> Option<&AppMonitorRecord> {
        self.app_monitors
            .get(region)?
            .values()
            .find(|record| record.arn == arn)
    }
}

/// Why a create may not take the name and the monitor slot it asked for — the two
/// refusals `CreateAppMonitor` reads off this store, each already in the modelled shape
/// the operation answers it with ([`hold_app_monitor_name`]).
#[derive(Debug)]
pub enum AppMonitorHoldRefused {
    /// The region is already using that name: a stored monitor carries it, or a create in
    /// flight is holding it.
    NameTaken(ConflictException),
    /// Storing one more monitor in that region would pass an app-monitor ceiling — the
    /// per-region allowance or the store's total.
    QuotaExceeded(ServiceQuotaExceededException),
}

/// A create's claim on ONE name in ONE region, held from the moment the create's
/// store-read refusals are decided until the record is stored or the call gives up.
///
/// Released on drop, so a create that answers an error — or a request whose future is
/// dropped — hands the name back without a handler having to remember to, and
/// [`Self::release_under`] releases it under the lock the insert already holds, since the
/// state lock is not reentrant.
pub struct AppMonitorHold {
    state: ::engine::ServiceState<ComAmazonawsRumRumState>,
    region: String,
    name: String,
    /// Whether the claim is still in the store: cleared by [`Self::release_under`] so the
    /// drop that follows it does not take the lock a second time.
    claimed: bool,
}

impl AppMonitorHold {
    /// Release the claim through a guard the caller already holds — what the create calls
    /// under the very lock its insert runs in, so the name goes from claimed to stored
    /// with no instant in between where it is neither.
    pub fn release_under(mut self, store: &mut ComAmazonawsRumRumState) {
        store.release_app_monitor_name(&self.region, &self.name);
        self.claimed = false;
    }
}

impl ::std::fmt::Debug for AppMonitorHold {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("AppMonitorHold")
            .field("region", &self.region)
            .field("name", &self.name)
            .field("claimed", &self.claimed)
            .finish()
    }
}

impl Drop for AppMonitorHold {
    fn drop(&mut self) {
        if self.claimed {
            self.state
                .lock()
                .release_app_monitor_name(&self.region, &self.name);
        }
    }
}

/// Decide the refusals a create reads off this store — the duplicate name, the per-region
/// allowance and the region-bucket bound — and, when none of them applies, HOLD the name
/// and the slot the create just won.
///
/// The decision and the claim are taken under ONE lock, so they are final: the create can
/// then do the work that needs no lock (provisioning the monitor's vended CloudWatch Logs
/// group, which is an in-process call to another service and therefore an `.await`) knowing
/// that nothing it was promised can be taken away while it is away, and that its insert
/// needs no second opinion. That ordering is what keeps a side effect in another service
/// out of a call this service goes on to refuse: a create either holds its name before
/// anything is provisioned, or it is refused before anything is provisioned.
///
/// Call it with NO guard of this state held — it takes the lock itself.
pub fn hold_app_monitor_name(
    state: &::engine::ServiceState<ComAmazonawsRumRumState>,
    region: &str,
    name: &str,
) -> Result<AppMonitorHold, AppMonitorHoldRefused> {
    {
        let mut store = state.lock();
        // A name identifies an app monitor for every other operation IN THIS REGION, so a
        // second monitor of this region cannot take one that is in use there — while the
        // same name in another region names a different, independent monitor and is free.
        if store.contains_app_monitor(region, name) || store.app_monitor_name_claimed(region, name)
        {
            return Err(AppMonitorHoldRefused::NameTaken(app_monitor_already_exists(
                name,
            )));
        }
        // The per-region allowance bounds each region's monitors, counting the ones
        // creates in flight are holding slots for.
        if store.app_monitor_count(region) + store.claimed_app_monitor_count(region)
            >= MAX_APP_MONITORS
        {
            return Err(AppMonitorHoldRefused::QuotaExceeded(
                app_monitor_quota_exceeded(MAX_APP_MONITORS),
            ));
        }
        // And this bounds how many regions hold any, so the store's total stays finite
        // whatever region a caller signs for ([`MAX_REGIONS`]).
        if store.region_bucket_exhausted(region) {
            return Err(AppMonitorHoldRefused::QuotaExceeded(
                app_monitor_quota_exceeded(MAX_REGIONS * MAX_APP_MONITORS),
            ));
        }
        store.claim_app_monitor_name(region, name);
    }
    Ok(AppMonitorHold {
        state: state.clone(),
        region: region.to_string(),
        name: name.to_string(),
        claimed: true,
    })
}

/// The current nanosecond clock, the seed [`ComAmazonawsRumRumState::mint_app_monitor_id`]
/// draws from once per state.
fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// The id space [`ComAmazonawsRumRumState::mint_app_monitor_id`] draws from.
const APP_MONITOR_ID_DOMAIN: &str = "rum/appmonitor";

/// The id space [`ComAmazonawsRumRumState::mint_metric_definition_id`] draws from.
const METRIC_DEFINITION_ID_DOMAIN: &str = "rum/metricdefinition";

/// The value space [`ComAmazonawsRumRumState::mint_cw_log_group`] draws a vended log
/// group's service-chosen suffix from — its own, so a monitor's group suffix is never a
/// prefix of an id this state also handed out.
const CW_LOG_GROUP_DOMAIN: &str = "rum/cwloggroup";

/// One `(domain, seed, sequence)` triple as a canonical, lowercase, version-4-shaped
/// UUID string.
///
/// The 16 bytes come from a SHA-256 digest of the triple, then byte 6's high nibble is
/// set to `4` (the version) and byte 8's two high bits to `10` (the RFC 4122 variant),
/// which is exactly the layout a v4 generator writes. Distinct triples give distinct
/// ids: the digest is injective in practice and the sequence is strictly increasing.
/// `domain` keeps one kind of id from ever equalling another drawn from the same seed
/// and counter.
fn uuid_v4_shaped(domain: &str, seed: u64, sequence: u64) -> String {
    use sha2::{Digest as _, Sha256};

    let digest = Sha256::digest(format!("{domain}/{seed}/{sequence}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The ARN of the app monitor named `name` in `region`.
pub fn app_monitor_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APP_MONITOR_RESOURCE}/{name}"),
    )
}

/// The ARN of the account's whole app-monitor collection in `region`:
/// `arn:aws:rum:<region>:<account>:appmonitor/*`.
///
/// What an operation that acts on the collection rather than on one monitor
/// (`ListAppMonitors`) is authorized against, so a policy scoped to one monitor's ARN
/// does not also grant the listing.
pub fn app_monitor_collection_arn(region: &str) -> String {
    app_monitor_arn(region, "*")
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An instant as the modelled `ISOTimestampString`: `YYYY-MM-DDTHH:MM:SS`, exactly the
/// 19 characters that shape's `@length(min: 19, max: 19)` admits — RFC 3339 truncated to
/// whole seconds and with no zone suffix, which is how RUM renders `Created` and
/// `LastModified`.
pub fn iso_timestamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    let rendered = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let trimmed = rendered.strip_suffix('Z').unwrap_or(rendered.as_str());
    if trimmed.len() == 19 {
        trimmed.to_string()
    } else {
        // Only reachable for an instant RFC 3339 cannot render (a clock outside year
        // 0001..9999). The shape admits 19 characters and nothing else, so answer the
        // epoch rather than a string no client can parse.
        "1970-01-01T00:00:00".to_string()
    }
}

/// Now, as the modelled `ISOTimestampString`.
pub fn current_iso_timestamp() -> String {
    iso_timestamp(SystemTime::now())
}

/// `configuration` with every member it left unset settled onto the value the model
/// documents for that member when it is omitted.
///
/// The ONE conversion a configuration goes through before it is stored — the create path
/// and the update path share it — so a structure a request carried only part of is stored
/// with the same values as one it omitted entirely, and no read reports a monitor
/// configured with a value the service would never have chosen. Pass
/// `AppMonitorConfiguration::default()` for the request that named no configuration at
/// all: it leaves every member unset, so it settles to exactly the documented defaults.
///
/// `SessionSampleRate` is the one member of the structure the model documents a value for
/// when it is omitted ("If you omit this parameter, the default of 0.1 is used"): the rest
/// document none, so one a request left out stays absent. An omitted rate arrives as the
/// member's modelled `@default` ([`UNSET_SESSION_SAMPLE_RATE`]), which is also the bottom
/// of its modelled `@range`, so that is what "unset" is readable as here.
pub fn settled_app_monitor_configuration(
    mut configuration: AppMonitorConfiguration,
) -> AppMonitorConfiguration {
    if configuration.session_sample_rate <= UNSET_SESSION_SAMPLE_RATE {
        configuration.session_sample_rate = DEFAULT_SESSION_SAMPLE_RATE;
    }
    configuration
}

/// `custom_events` with its `Status` settled onto the documented default when the request
/// left the member unset — the same conversion for the request that named no
/// `CustomEvents` at all (pass `CustomEvents::default()`) and for the one that carried the
/// structure without a status, since "the default is for custom events to be `DISABLED`"
/// either way.
pub fn settled_custom_events(mut custom_events: CustomEvents) -> CustomEvents {
    if custom_events.status.is_none() {
        custom_events.status = Some(DEFAULT_CUSTOM_EVENTS_STATUS.to_string());
    }
    custom_events
}

/// The deobfuscation configuration an app monitor gets when its create request carried
/// none: source-map deobfuscation off, and no bucket to read maps from.
///
/// `DeobfuscationConfiguration` is `{ JavaScriptSourceMaps }` and
/// `JavaScriptSourceMaps$Status` is `@required` (a member of
/// [`crate::ops::types::DEOBFUSCATION_STATUS_VALUES`]), so there is no such thing as a
/// configuration that leaves the status open: a monitor created without one is created
/// with deobfuscation DISABLED, and recorded behaviour reports exactly that structure
/// rather than omitting the member. Which is also why a read never has to decide: the
/// record always holds one ([`AppMonitorRecord::deobfuscation_configuration`]).
///
/// There is nothing for a configuration the request DID carry to settle: the one member of
/// the structure documents no value for an omitted parameter, and the status inside it is
/// `@required`, so a request that names source maps always names their status
/// (`settled_app_monitor_configuration` and `settled_custom_events` cover the two members
/// that DO document one).
pub fn default_deobfuscation_configuration() -> DeobfuscationConfiguration {
    DeobfuscationConfiguration {
        java_script_source_maps: Some(JavaScriptSourceMaps {
            status: DEFAULT_DEOBFUSCATION_STATUS.to_string(),
            s3_uri: None,
        }),
    }
}

/// Why this request's `Domain`/`DomainList` pair cannot be applied, or `None` when it
/// can.
///
/// From the model: `CreateAppMonitorRequest$DomainList` documents "The CreateAppMonitor
/// requires either the domain or the domain list" and
/// `UpdateAppMonitorRequest$DomainList` documents "The UpdateAppMonitor allows either the
/// domain or the domain list" — a monitor's domain authority is held in exactly ONE of
/// the two forms. Neither constraint is expressible in the model, so every operation that
/// accepts the pair refuses it here with the `ValidationException` it declares.
/// `required` is true for the operation that must end up with one (a create of a monitor
/// whose platform HAS an internet domain — see [`platform_requires_domain`]), false for
/// one that may leave the stored choice alone (an update) or for a monitor that has no
/// domain to name.
pub fn domain_choice_refusal(
    domain: Option<&str>,
    domain_list: Option<&[String]>,
    required: bool,
) -> Option<String> {
    match (domain, domain_list) {
        (Some(_), Some(_)) => Some(
            "Domain and DomainList cannot both be specified; name either the domain or \
             the domain list."
                .to_string(),
        ),
        (None, None) if required => {
            Some("Either Domain or DomainList must be specified.".to_string())
        }
        _ => None,
    }
}

/// Whether an app monitor created for `platform` must name its domain authority.
///
/// From the model: `CreateAppMonitorRequest$DomainList` documents "The CreateAppMonitor
/// requires either the domain or the domain list", and `Domain` is "the top-level
/// internet domain name for which your application has administrative authority". An
/// application on a platform other than [`WEB_PLATFORM`] — an Android or an iOS
/// application — is not served from an internet domain, so there is no domain authority
/// for it to hold and the requirement applies to the web platform alone. Recorded
/// behaviour agrees: a monitor created for a mobile platform with neither member is
/// accepted and read back carrying neither.
///
/// The OTHER half of the rule is platform-independent: whichever platform a monitor is
/// for, its domain authority is held in at most one of the two forms, so naming both is
/// refused for every platform ([`domain_choice_refusal`]).
pub fn platform_requires_domain(platform: &str) -> bool {
    platform == WEB_PLATFORM
}

/// Why this `AppMonitorConfiguration` cannot be applied, or `None` when it can.
///
/// From the model: `AppMonitorConfiguration$IncludedPages` and `$ExcludedPages` both
/// document "You can't include both `ExcludedPages` and `IncludedPages` in the same
/// operation", a rule no modelled constraint expresses, so every operation that accepts
/// a configuration refuses it here with the `ValidationException` it declares.
pub fn app_monitor_configuration_refusal(config: &AppMonitorConfiguration) -> Option<String> {
    let includes = config.included_pages.as_ref().is_some_and(|p| !p.is_empty());
    let excludes = config.excluded_pages.as_ref().is_some_and(|p| !p.is_empty());
    (includes && excludes).then(|| {
        "AppMonitorConfiguration cannot carry both IncludedPages and ExcludedPages."
            .to_string()
    })
}

/// Why this `DeobfuscationConfiguration` cannot be applied, or `None` when it can.
///
/// From the model: `JavaScriptSourceMaps$S3Uri` is "The S3Uri of the bucket or folder that
/// stores the source map files. It is required if status is ENABLED." — source-map
/// unminification reads the maps out of a bucket, so a configuration that turns it on has
/// to say which bucket. `S3Uri` is an optional member carrying only a `@length` and a
/// `@pattern`, so an absent value satisfies every modelled constraint and the conditional
/// requirement is prose: every operation that accepts a deobfuscation configuration refuses
/// it here with the `ValidationException` it declares, before anything is written.
///
/// A configuration naming no `JavaScriptSourceMaps` turns nothing on, and one whose status
/// is not the enabling value needs no bucket — an `S3Uri` beside it is accepted, since the
/// model forbids it nowhere and a caller may keep the bucket it will enable later.
pub fn deobfuscation_configuration_refusal(
    configuration: &DeobfuscationConfiguration,
) -> Option<String> {
    let source_maps = configuration.java_script_source_maps.as_ref()?;
    let enabled = source_maps.status == ENABLED_DEOBFUSCATION_STATUS;
    let names_bucket = source_maps.s3_uri.as_ref().is_some_and(|uri| !uri.is_empty());
    (enabled && !names_bucket).then(|| {
        format!(
            "S3Uri is required when JavaScriptSourceMaps Status is \
             {ENABLED_DEOBFUSCATION_STATUS}."
        )
    })
}

/// Why this `Destination`/`DestinationArn` pair cannot be applied, or `None` when it can.
///
/// From the model: `PutRumMetricsDestinationRequest$DestinationArn` documents "Use this
/// parameter only if Destination is Evidently", the operation itself documents "If you
/// specify Evidently, you must also specify the ARN of the CloudWatchEvidently experiment
/// that is to be the destination", and `DeleteRumMetricsDestinationRequest$DestinationArn`
/// states both halves outright: "This parameter is required if Destination is Evidently.
/// If Destination is CloudWatch, do not use this parameter." So an Evidently destination
/// is identified by an experiment ARN and a CloudWatch one carries none. Neither half is
/// expressible as a modelled constraint, so every operation that accepts the pair refuses
/// it here with the `ValidationException` it declares.
///
/// The `Destination` value itself is checked against
/// [`crate::ops::types::METRIC_DESTINATION_VALUES`] first, so an operation stays correct
/// on a protocol whose boundary does not enforce the enum.
pub fn metric_destination_choice_refusal(
    destination: &str,
    destination_arn: Option<&str>,
) -> Option<String> {
    if !METRIC_DESTINATION_VALUES.contains(&destination) {
        return Some(format!(
            "Destination {destination} is not valid; valid values are {}.",
            METRIC_DESTINATION_VALUES.join(" and ")
        ));
    }
    match (destination, destination_arn) {
        (EVIDENTLY_DESTINATION, None) => Some(format!(
            "DestinationArn is required when Destination is {EVIDENTLY_DESTINATION}."
        )),
        (EVIDENTLY_DESTINATION, Some(_)) => None,
        (_, Some(_)) => Some(format!(
            "DestinationArn cannot be used when Destination is {destination}."
        )),
        (_, None) => None,
    }
}

/// Why this `Destination`/`IamRoleArn` pair cannot be applied, or `None` when it can.
///
/// From the model: `PutRumMetricsDestinationRequest$IamRoleArn` documents "This parameter
/// is required if Destination is Evidently. If Destination is CloudWatch, don't use this
/// parameter." — RUM needs a role to assume only when it writes to someone else's
/// experiment. Same reasoning as [`metric_destination_choice_refusal`]: the rule is prose
/// in the model, so the handler enforces it.
pub fn metric_destination_role_refusal(
    destination: &str,
    iam_role_arn: Option<&str>,
) -> Option<String> {
    match (destination, iam_role_arn) {
        (EVIDENTLY_DESTINATION, None) => Some(format!(
            "IamRoleArn is required when Destination is {EVIDENTLY_DESTINATION}."
        )),
        (EVIDENTLY_DESTINATION, Some(_)) => None,
        (_, Some(_)) => Some(format!(
            "IamRoleArn cannot be used when Destination is {destination}."
        )),
        (_, None) => None,
    }
}

/// `ResourceNotFoundException` for a name or id that names no stored app monitor.
///
/// The message is built from the request: the literal prefix `Resource not found `
/// followed by the value the request identified the monitor by, verbatim and with no
/// trailing period. That construction is RUM's own, and it is the same for every
/// operation that resolves an app monitor — recorded across this service's app-monitor
/// reads and writes, its metrics-destination calls and its resource-policy calls. The
/// body carries NOTHING else: `resourceName`/`resourceType` are off the wire for this
/// shape (see [`crate::ops::types::ResourceNotFoundException`]).
pub fn app_monitor_not_found(name_or_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{NOT_FOUND_MESSAGE} {name_or_id}"),
    }
}

/// The whole sentence this service reports for a resource that is not there, which the
/// app-monitor refusal above then names the requested monitor after.
///
/// It is also the message ON ITS OWN of a BATCH operation's per-item not-found error
/// (`BatchDeleteRumMetricDefinitionsError$ErrorMessage`): a per-item error reports the
/// error class's own fixed text, with no part of the request interpolated into it — the
/// item the text is about is already named by the error entry's own id member.
pub const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// `ResourceNotFoundException` for a destination the named app monitor does not have —
/// what the operations that address a destination's metric DEFINITIONS answer for a
/// `(Destination, DestinationArn)` pair the monitor holds no record under.
///
/// RECORDED: the message names the app monitor the request addressed and refers to the pair
/// as "the specified parameters" rather than echoing it, so the destination kind and the
/// experiment ARN are not part of the text. Like every other not-found body of this service
/// it carries `message` ALONE — `resourceName`/`resourceType` are off the wire for this
/// shape (see [`crate::ops::types::ResourceNotFoundException`]).
///
/// The operations that ADD metric definitions to a destination refuse the same missing
/// destination in their own words ([`adding_metrics_destination_not_found`]).
pub fn metric_destination_not_found(app_monitor_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No destination for Rum metric definitions exists on app monitor \
             {app_monitor_name}, for the specified parameters. Please try again."
        ),
    }
}

/// The dotted member path the framework's walk reports a violation inside the
/// `MetricDefinition` member at — the member the single-definition update carries a
/// definition by.
const METRIC_DEFINITION_PATH: &str = "metricDefinition";

/// The dotted member path the walk reports the `MetricDefinitions` LIST at — the member the
/// batch create carries its definitions by.
const METRIC_DEFINITIONS_PATH: &str = "metricDefinitions";

/// The path segment a definition's dimension map is reported under: the lowerCamel spelling
/// of `MetricDefinitionRequest$DimensionKeys`.
const DIMENSION_KEYS_MEMBER: &str = "dimensionKeys";

/// Why the `DimensionKeys` of the ONE definition a request carries break what
/// `DimensionKeysMap` declares, or `None` when every entry satisfies it.
///
/// For the operation that carries a definition by the `MetricDefinition` member; the batch
/// form is [`metric_definitions_dimension_keys_refusal`], and both apply the one set of
/// constraints ([`dimension_keys_violations`]) because those are the map's, declared once on
/// the shape every `DimensionKeys` member targets.
#[must_use]
pub fn dimension_keys_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
    dimension_keys_self_applied(definition).refusal()
}

/// The same violations, unworded, for the gate in front of the operation
/// ([`validation::SelfApplied`]): a request that breaks one of these AND a constraint the
/// framework's boundary walk applies is stated in ONE envelope, which the operation it
/// would have been refused by never gets to do.
#[must_use]
pub fn dimension_keys_self_applied(
    definition: &MetricDefinitionRequest,
) -> validation::SelfApplied {
    self_applied(dimension_keys_violations(METRIC_DEFINITION_PATH, definition))
}

/// The same, for every definition of the `MetricDefinitions` LIST a batch request carries,
/// each judged at the path the walk reports that element's members at.
///
/// A modelled constraint is the REQUEST's to satisfy and not one definition's: the boundary
/// walk refuses the WHOLE call for a constraint it can apply — a dimension name outside
/// `DimensionName`'s `@length` is refused that way today — so a constraint of the same map
/// that the walk cannot apply refuses the whole call too, rather than failing one entry of
/// the batch the way the prose rules in [`metric_definition_refusal`] do. Every offending
/// entry of every definition is stated, which is what the walk accumulating one violation
/// per leaf would have stated.
#[must_use]
pub fn metric_definitions_dimension_keys_refusal(
    definitions: &[MetricDefinitionRequest],
) -> Option<String> {
    metric_definitions_dimension_keys_self_applied(definitions).refusal()
}

/// The same violations, unworded, for the gate in front of the operation
/// ([`dimension_keys_self_applied`] states why).
#[must_use]
pub fn metric_definitions_dimension_keys_self_applied(
    definitions: &[MetricDefinitionRequest],
) -> validation::SelfApplied {
    let violations: Vec<::aws_smithy_schema_server::constraints::Violation> = definitions
        .iter()
        .enumerate()
        .flat_map(|(index, definition)| {
            let path = format!(
                "{METRIC_DEFINITIONS_PATH}.{}",
                ::aws_smithy_schema_server::constraints::path::element(index + 1)
            );
            dimension_keys_violations(&path, definition)
        })
        // The bound the framework's own walk collects to
        // (`service_common::validation::MAX_VIOLATIONS`): a request past it is refused with
        // the violations found so far, so neither the work this does nor the message it
        // states is sized by how many definitions a caller sent.
        .take(service_common::validation::MAX_VIOLATIONS)
        .collect();
    self_applied(violations)
}

/// Every constraint the entries of `definition`'s `DimensionKeys` break, at the paths the
/// framework's walk spells those positions as, where `path` is the path of the definition
/// itself.
///
/// BOTH shapes of `DimensionKeysMap` are applied here, for the two reasons the boundary walk
/// leaves them unapplied (`crate::state::validation`): the KEY's `@length`, because the walk
/// validates no map key at all, and the VALUE's `@length` and `@pattern`, whose pattern opens
/// with a negative look-ahead the walk's engine cannot compile — and a pattern it cannot
/// compile is treated as matching, so a dimension name outside it would otherwise be stored.
/// The CHECKS are [`validation`]'s, which is the one place this service applies a modelled
/// `@length` or `@pattern` itself.
///
/// The entries are judged in SORTED key order, and an entry's key before its value, which is
/// the order the walk writes an entry in — so which clause a refusal states first is fixed by
/// the request rather than by a `HashMap`'s iteration order.
fn dimension_keys_violations(
    path: &str,
    definition: &MetricDefinitionRequest,
) -> Vec<::aws_smithy_schema_server::constraints::Violation> {
    let mut entries: Vec<(&str, &str)> = definition
        .dimension_keys
        .iter()
        .flatten()
        .map(|(field, dimension)| (field.as_str(), dimension.as_str()))
        .collect();
    entries.sort_unstable();
    entries
        .into_iter()
        .flat_map(|(field, dimension)| {
            // An entry is reported at the key the CALLER chose: that is the segment the walk
            // spells a terminal map value with (`begin_collection_child` in
            // `vendor/smithy-schema-server/src/constraints.rs`), and it is the position both
            // halves of the entry hold.
            let entry = format!("{path}.{DIMENSION_KEYS_MEMBER}.{field}");
            [
                validation::string_violation(
                    validation::ConstrainedString::DimensionKey,
                    &entry,
                    field,
                ),
                validation::string_violation(
                    validation::ConstrainedString::DimensionName,
                    &entry,
                    dimension,
                ),
            ]
        })
        .flatten()
        .collect()
}

/// The violations the checks in this module find, as the structure BOTH callers take them
/// in ([`validation::SelfApplied`]): the handler that words them
/// (`validation::SelfApplied::refusal`, the one place a constraint refusal of this service is
/// worded), and the gate in front of the operation that states them beside the framework's
/// own (`crate::service`).
///
/// No rendered list travels with them: the checks that build them judge a MAP's entries or a
/// list of keys, and a clause is nested into a list member's own clause only for a violation
/// on an element of a list whose element shape declares constraints.
fn self_applied(
    violations: Vec<::aws_smithy_schema_server::constraints::Violation>,
) -> validation::SelfApplied {
    validation::SelfApplied {
        violations,
        lists: HashMap::new(),
    }
}

/// Why this `MetricDefinitionRequest` is not a metric definition this service can store,
/// or `None` when it is one.
///
/// The modelled constraints of the members the boundary walk reaches (their length, the
/// namespace's pattern) are refused before a handler runs, and the ones it leaves unapplied
/// — the `DimensionKeys` map's key and value shapes — are refused by
/// [`dimension_keys_refusal`] in front of this. What is left are the rules the model states
/// in PROSE, which is
/// why they live here: every operation that accepts a definition — the batch create, where
/// one bad definition fails alone, and the update, where it fails the call — judges a
/// definition exactly the same way.
///
/// * `Namespace` (from the model): "You cannot use any string that starts with `AWS/` for
///   your namespace", while an extended metric's namespace "must be omitted or set to
///   `AWS/RUM`" — so the reserved space is closed except for RUM's own namespace.
/// * The KIND of a definition, and the rules that follow from it (from the model): a
///   definition whose `Namespace` is omitted or `AWS/RUM` is an EXTENDED metric, so its
///   `Name` has to be one [`EXTENDED_METRIC_CATALOG`] lists and its `ValueKey` and
///   `EventPattern` have to be the combination that catalog pins for that name ("Only
///   certain combinations of values for `Name`, `ValueKey`, and `EventPattern` are
///   valid"). Every other definition is a CUSTOM metric, whose namespace "can't be omitted
///   and can't be `AWS/RUM`".
/// * `EventPattern` (from the model): "The pattern that defines the metric, specified as a
///   JSON object", so a pattern that is not one defines nothing.
/// * `DimensionKeys` (from the model): "For both extended metrics and custom metrics, all
///   dimensions listed in this field must also be included in `EventPattern`" — the
///   dimension's value is read out of the matched event, so the pattern has to name that
///   field, and a definition with dimensions but no pattern names none of them. For an
///   EXTENDED metric the ENTRIES themselves are pinned too, to the pairs
///   [`EXTENDED_DIMENSION_ENTRIES`] lists ([`extended_metric_refusal`]).
/// * The event fields a CUSTOM metric reads and the pattern it reads them by (from the
///   model): every first-level key its `ValueKey`, `EventPattern` and `DimensionKeys` name
///   has to be a field a RUM event carries, a pattern filtering on `event_details` has to
///   filter on `event_type` too, and the array a first-level key of its pattern carries holds
///   one value ([`custom_metric_refusal`]).
///
/// Which DESTINATION the request names is deliberately none of them, which is why this
/// judge takes the definition alone. The destination scoping of `DimensionKeys` ("Use this
/// field only if you are sending the metric to CloudWatch") bounds what the dimensions DO,
/// not whether a definition may declare them: the member a read reports the stored map back
/// through states the consequence outright — "The value of this field is used only if the
/// metric destination is `CloudWatch`. If the metric destination is `Evidently`, the value
/// of `DimensionKeys` is ignored" — which is a stored definition carrying the map under
/// either destination, and the operation's own documentation has extended metrics, the
/// metrics those additional dimensions belong to, going "to both Evidently and CloudWatch".
/// `UnitLabel` is scoped the same way and reported the same way. The `Destination` pairings
/// this service does refuse are the ones the model PROHIBITS outright — "If `Destination` is
/// `CloudWatch`, do not use this parameter" for `DestinationArn` and the same sentence for
/// `IamRoleArn` ([`metric_destination_choice_refusal`], [`metric_destination_role_refusal`])
/// — so a definition is judged identically whichever destination it is sent to, and a
/// member whose effect one destination ignores is still stored and reported as it was sent.
pub fn metric_definition_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
    let namespace = definition.namespace.as_deref();
    if let Some(namespace) = namespace {
        if namespace.starts_with(RESERVED_NAMESPACE_PREFIX)
            && namespace != EXTENDED_METRICS_NAMESPACE
        {
            return Some(format!(
                "Namespace {namespace} is reserved: a namespace starting with \
                 {RESERVED_NAMESPACE_PREFIX} cannot be used, other than \
                 {EXTENDED_METRICS_NAMESPACE} for extended metrics."
            ));
        }
    }

    // The pattern this definition declares, read through [`declared_text`] — the ONE
    // reading of the member, which the canonical stored form applies too, so a request
    // admitted here as naming no event is also STORED as naming none. The member is
    // modelled `@length(min: 0, max: 4000)`, so a value naming nothing reaches here.
    let declared = declared_text(definition.event_pattern.as_deref());
    let pattern = match declared {
        Some(text) => match ::serde_json::from_str::<::serde_json::Value>(text) {
            Ok(parsed) if parsed.is_object() => Some(parsed),
            Ok(_) => return Some("EventPattern must be a JSON object.".to_string()),
            Err(error) => return Some(format!("EventPattern is not valid JSON: {error}.")),
        },
        None => None,
    };

    // Which KIND of metric this definition is, and the rules of that kind. The namespace
    // decides: a definition published to RUM's own namespace, or to none at all, is an
    // extended metric, and one published to a namespace of the caller's own is a custom
    // metric.
    let extended_namespace = names_extended_namespace(definition);
    if extended_namespace {
        if let Some(message) = extended_metric_refusal(definition, pattern.as_ref()) {
            return Some(message);
        }
    } else if let Some(message) = custom_metric_refusal(definition, pattern.as_ref()) {
        return Some(message);
    }

    // Sorted, so the one dimension a refusal names is the same whichever order the
    // request's map iterated in.
    let mut dimensions: Vec<&str> = definition
        .dimension_keys
        .iter()
        .flatten()
        .map(|(field, _)| field.as_str())
        .collect();
    dimensions.sort_unstable();
    // A definition that lists no dimension has nothing left to check.
    let first = *dimensions.first()?;
    let Some(pattern) = pattern else {
        return Some(format!(
            "DimensionKeys names the field {first}, so an EventPattern including it is \
             required."
        ));
    };
    dimensions
        .into_iter()
        .find(|field| !event_pattern_names_field(&pattern, field))
        .map(|field| {
            format!("DimensionKeys names the field {field}, which the EventPattern does not include.")
        })
}

/// Why this definition is not the EXTENDED metric its namespace says it is, or `None` when
/// it is one.
///
/// Reached for a definition whose `Namespace` is omitted or [`EXTENDED_METRICS_NAMESPACE`]
/// — the namespace extended metrics are published to — so the model's extended-metric
/// rules are the ones in force: the name has to be one the catalog lists, the
/// `ValueKey`/`EventPattern` combination has to be the one that row pins, and every
/// `DimensionKeys` entry has to be one of the pairs the model lists for extended metrics
/// ("For extended metrics, valid values for the entries in this field are the following",
/// [`EXTENDED_DIMENSION_ENTRIES`]) — RUM computes those dimensions itself, so neither the
/// field path nor the name it is published under is the caller's to choose. A name the
/// catalog does NOT list is no extended metric at all, which invalidates the whole request
/// ([`extended_metric_name_refusal`]) rather than this definition alone.
fn extended_metric_refusal(
    definition: &MetricDefinitionRequest,
    pattern: Option<&::serde_json::Value>,
) -> Option<String> {
    let name = definition.name.as_str();
    let Some(entry) = extended_metric(name) else {
        // The name is not one RUM recognizes, which is the whole REQUEST's failure rather
        // than this definition's alone ([`extended_metric_name_refusal`]); the wording is
        // that one rule's, stated in one place, so the batch create's whole-request refusal
        // and the update's say the same thing.
        return Some(metric_name_not_valid(name));
    };

    // "Only certain combinations of values for Name, ValueKey, and EventPattern are
    // valid": the `ValueKey` of the row that names this metric, and nothing else.
    let declared_value_key = definition
        .value_key
        .as_deref()
        .filter(|value_key| !value_key.is_empty());
    if declared_value_key != entry.value_key {
        return Some(match entry.value_key {
            Some(required) => format!(
                "ValueKey must be {required} for the extended metric {name}."
            ),
            None => format!("ValueKey must be omitted for the extended metric {name}."),
        });
    }

    // "For extended metrics, valid values for the entries in this field are the
    // following": each `DimensionKeys` entry of an extended metric is one of the
    // `field path`/`dimension name` pairs the model lists, so a path it does not list
    // reads a dimension RUM does not compute for its own metrics, and a path it lists
    // carries only the dimension name pinned beside it. Sorted, so the entry a refusal
    // names is the same whichever order the request's map iterated in.
    let mut entries: Vec<(&str, &str)> = definition
        .dimension_keys
        .iter()
        .flatten()
        .map(|(field, dimension)| (field.as_str(), dimension.as_str()))
        .collect();
    entries.sort_unstable();
    for (field, dimension) in entries {
        match extended_dimension_name(field) {
            Some(required) if required == dimension => {}
            Some(required) => {
                return Some(format!(
                    "DimensionKeys maps the field {field} to the dimension {dimension}: the \
                     extended metric {name} publishes that field as {required}."
                ));
            }
            None => {
                return Some(format!(
                    "DimensionKeys names the field {field}, which is not a dimension the \
                     extended metric {name} can be broken down by: the valid field paths \
                     are {}.",
                    extended_dimension_fields()
                ));
            }
        }
    }

    // "When you define extended metrics, the metric definition is not valid if
    // EventPattern is omitted", and the pattern it carries has to INCLUDE the row's —
    // beyond which it may name whatever else the definition's dimensions need. A row whose
    // literal does not parse states no requirement, so there is nothing to refuse for.
    let required = entry.required_event_pattern()?;
    let Some(pattern) = pattern else {
        return Some(format!(
            "EventPattern is required for the extended metric {name}, and must include \
             {required}."
        ));
    };
    if !event_pattern_includes(pattern, &required) {
        return Some(format!(
            "EventPattern for the extended metric {name} must include {required}."
        ));
    }
    None
}

/// Whether `definition` is addressed to RUM's extended-metrics namespace, i.e. whether its
/// `Namespace` is [`EXTENDED_METRICS_NAMESPACE`] or omitted.
///
/// From the model (`MetricDefinitionRequest`): "For extended metrics that are sent to the
/// `AWS/RUM` namespace … The `Namespace` parameter must be omitted or set to `AWS/RUM`" —
/// so the two spellings name the same namespace and a definition carrying either is judged
/// by the extended-metric rules.
fn names_extended_namespace(definition: &MetricDefinitionRequest) -> bool {
    definition
        .namespace
        .as_deref()
        .is_none_or(|namespace| namespace == EXTENDED_METRICS_NAMESPACE)
}

/// The refusal for a `Name` RUM does not recognize as one of its extended metrics, worded
/// the way the service states an invalid metric value.
fn metric_name_not_valid(name: &str) -> String {
    format!("Metric value {name} is not valid.")
}

/// Why the WHOLE request carrying `definition` is not valid, or `None` when nothing about
/// its `Name` makes it so.
///
/// A definition addressed to RUM's extended-metrics namespace — named outright or left to
/// the omission the model equates with it ([`names_extended_namespace`]) — defines one of
/// RUM's OWN metrics, so its `Name` has to be one the catalog lists
/// ([`EXTENDED_METRIC_CATALOG`]): "You can use the `AWS/RUM` namespace only for extended
/// metrics", and the catalog is the set of extended metrics there are. A name outside it is
/// a value the request may not carry at all rather than a definition the service declines to
/// store, so it refuses the whole call with `ValidationException` and nothing is written —
/// which is why this is asked BEFORE any per-definition processing, by every operation that
/// accepts a definition.
///
/// A definition published to a namespace of the caller's own is a custom metric, which "can
/// specify any name that you like", so this says nothing about it.
#[must_use]
pub fn extended_metric_name_refusal(definition: &MetricDefinitionRequest) -> Option<String> {
    (names_extended_namespace(definition) && extended_metric(&definition.name).is_none())
        .then(|| metric_name_not_valid(&definition.name))
}

/// The same, for the `MetricDefinitions` LIST a batch request carries: the first definition
/// whose `Name` makes the request invalid, in request order.
///
/// One such definition invalidates the whole request, so the batch form reports the first
/// one rather than accumulating — there is one refusal to state.
#[must_use]
pub fn metric_definitions_name_refusal(
    definitions: &[MetricDefinitionRequest],
) -> Option<String> {
    definitions.iter().find_map(extended_metric_name_refusal)
}

/// Why this definition is not the CUSTOM metric its namespace says it is, or `None` when
/// it is one.
///
/// Reached for a definition published to a namespace of the caller's own, so the model's
/// CUSTOM-metric rules are the ones in force. A custom metric is computed from the RUM
/// events its pattern matches, which is what each of these rules follows from:
///
/// * "The values that you specify for `ValueKey`, `EventPattern`, and `DimensionKeys` must
///   be fields in RUM events, so all first-level keys in these fields must be one of the
///   keys in the list later in this section" — the keys that list holds are
///   [`RUM_EVENT_FIELDS`], and a key outside it reads a field no event has.
/// * "If `EventPattern` contains an `event_details` field, it must also contain an
///   `event_type`", restated as "For every non-empty `event_details`, there must be a
///   non-empty `event_type`" — the event's type is what decides which details it carries,
///   so details are filtered on only beside a type.
/// * "In `EventPattern`, any JSON array must contain only one value" — applied to the array
///   a FIRST-LEVEL key carries, which is as far as that sentence and the patterns the model
///   gives as valid examples agree: those examples match every first-level key against one
///   value and nest the multi-value matchers (a value list, a numeric range clause) below one
///   ([`single_value_array_violation`]).
fn custom_metric_refusal(
    definition: &MetricDefinitionRequest,
    pattern: Option<&::serde_json::Value>,
) -> Option<String> {
    // The first-level key of each member that names event fields, in the order the model
    // lists those members. A dotted path (`event_details.latency`) reads the field its
    // FIRST segment names; a pattern reads the fields its top-level members name; and a
    // dimension is keyed by the path its value is read from. Each member's keys are sorted,
    // so the key a refusal names is the same whichever order the request spelled them in.
    let mut named: Vec<(&str, Vec<&str>)> = Vec::new();
    if let Some(value_key) = definition
        .value_key
        .as_deref()
        .filter(|value_key| !value_key.is_empty())
    {
        named.push(("ValueKey", vec![first_level_key(value_key)]));
    }
    if let Some(members) = pattern.and_then(::serde_json::Value::as_object) {
        named.push((
            "EventPattern",
            members.keys().map(String::as_str).collect(),
        ));
    }
    let dimensions: Vec<&str> = definition
        .dimension_keys
        .iter()
        .flatten()
        .map(|(field, _)| first_level_key(field))
        .collect();
    if !dimensions.is_empty() {
        named.push(("DimensionKeys", dimensions));
    }
    for (member, mut keys) in named {
        keys.sort_unstable();
        if let Some(unknown) = keys
            .into_iter()
            .find(|key| !RUM_EVENT_FIELDS.contains(key))
        {
            return Some(format!(
                "{member} names the first-level key {unknown}, which is not a field of a RUM \
                 event: a custom metric can name only the fields a RUM event carries."
            ));
        }
    }

    // Everything left is a rule about the pattern, so a definition carrying none is done.
    let pattern = pattern?;
    if !pattern_member_is_empty(pattern.get(EVENT_DETAILS_FIELD))
        && pattern_member_is_empty(pattern.get(EVENT_TYPE_FIELD))
    {
        return Some(format!(
            "EventPattern filters on {EVENT_DETAILS_FIELD}, so it must also carry a non-empty \
             {EVENT_TYPE_FIELD}: the event's type is what decides which details it has."
        ));
    }
    single_value_array_violation(pattern).map(|(field, held)| {
        format!(
            "EventPattern matches the event field {field} against {held} values: the array a \
             first-level key of an EventPattern carries holds only one value."
        )
    })
}

/// The event field a dotted field path reads — its FIRST segment, which is the "first-level
/// key" the model's valid-key list is about.
fn first_level_key(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// Whether this member of an event pattern filters on nothing: one that is absent, or one
/// present but carrying no matcher at all.
///
/// The model states the `event_details`/`event_type` pairing in terms of emptiness ("For
/// every non-empty `event_details`, there must be a non-empty `event_type`"), so a member
/// that is there and empty is the same input as one left out.
fn pattern_member_is_empty(member: Option<&::serde_json::Value>) -> bool {
    match member {
        None | Some(::serde_json::Value::Null) => true,
        Some(::serde_json::Value::Object(members)) => members.is_empty(),
        Some(::serde_json::Value::Array(items)) => items.is_empty(),
        Some(::serde_json::Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

/// The first FIRST-LEVEL key of `pattern` whose value is an array not carrying exactly ONE
/// value, and how many values that array carries — or `None` when every such array carries
/// one.
///
/// The model states the rule as "In `EventPattern`, any JSON array must contain only one
/// value", and it is applied to the array a first-level key carries DIRECTLY, because that
/// is as far as the rule and the documented patterns agree: every pattern the model gives as
/// a valid example matches a first-level key against a single value, while the matchers
/// those examples NEST below one carry as many values as the matcher needs — a value list
/// (`"metadata": { "browserName": [ "Chrome", "Firefox" ] }`) and a numeric range clause
/// (`"event_details": { "duration": [{ "numeric": [ ">=", 2000, "<", 8000 ] }] }`), which
/// are also the forms the catalog's own required patterns are written in
/// ([`EXTENDED_METRIC_CATALOG`]). So a nested array is the matcher's own shape and is not
/// counted ([`MetricDefinitionRequest$EventPattern`]'s documentation is the source of both
/// examples).
///
/// The keys are judged in SORTED order, so the field a refusal names is the same whichever
/// order the request spelled the pattern's members in.
fn single_value_array_violation(pattern: &::serde_json::Value) -> Option<(String, usize)> {
    let mut offending: Vec<(&str, usize)> = pattern
        .as_object()?
        .iter()
        .filter_map(|(field, held)| match held {
            ::serde_json::Value::Array(values) if values.len() != 1 => {
                Some((field.as_str(), values.len()))
            }
            _ => None,
        })
        .collect();
    offending.sort_unstable();
    offending
        .first()
        .map(|(field, held)| ((*field).to_string(), *held))
}

/// Whether the event pattern `pattern` INCLUDES everything `required` asks for.
///
/// The model requires an extended metric's pattern to "include" the object it pins for that
/// metric rather than to equal it, since "the `EventPattern` can also include information
/// used by the `DimensionKeys` field" — so this is a containment test, applied structurally:
/// every member of a required object has to be present and itself included, every element
/// of a required array has to be matched by some element of the pattern's array (a pattern
/// lists the values it matches, so their order carries no meaning), and a required scalar
/// has to be that scalar. Two numbers are the same number whichever JSON form they were
/// written in.
pub fn event_pattern_includes(
    pattern: &::serde_json::Value,
    required: &::serde_json::Value,
) -> bool {
    match required {
        ::serde_json::Value::Object(members) => pattern.as_object().is_some_and(|present| {
            members.iter().all(|(member, wanted)| {
                present
                    .get(member)
                    .is_some_and(|held| event_pattern_includes(held, wanted))
            })
        }),
        ::serde_json::Value::Array(items) => pattern.as_array().is_some_and(|present| {
            items.iter().all(|wanted| {
                present
                    .iter()
                    .any(|held| event_pattern_includes(held, wanted))
            })
        }),
        ::serde_json::Value::Number(wanted) => pattern
            .as_f64()
            .zip(wanted.as_f64())
            .is_some_and(|(held, wanted)| held == wanted),
        scalar => pattern == scalar,
    }
}

/// Whether the event pattern `pattern` names the event field `path` — a dot-separated
/// field path, which is how a `DimensionKeys` key spells one (`metadata.browserName`,
/// `event_details.fileType`).
///
/// A pattern is nested JSON objects down to the matcher for one field, so the path is
/// resolved segment by segment: the field is named when every segment is a member of the
/// object the previous one resolved to.
fn event_pattern_names_field(pattern: &::serde_json::Value, path: &str) -> bool {
    let mut cursor = pattern;
    for segment in path.split('.') {
        match cursor.as_object().and_then(|members| members.get(segment)) {
            Some(next) => cursor = next,
            None => return false,
        }
    }
    true
}

/// `ResourceNotFoundException` for an operation that ADDS metric definitions to a
/// destination the named app monitor does not have.
///
/// The operations that send metrics to a destination — the batch create and the
/// single-definition update, both of which "add metric definitions … to destination" —
/// report the missing destination as a failure of that ADD, so the only part of the
/// request the message carries is the `Destination` it was asked to add the metrics to.
/// The app monitor is not named: it exists, and naming it would say the wrong resource is
/// the missing one.
pub fn adding_metrics_destination_not_found(destination: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Error adding metrics - Destination {destination} not found."),
    }
}

/// `InternalServerException` for an update that addresses no stored metric definition.
///
/// `UpdateRumMetricDefinition` replaces the definition one `MetricDefinitionId` names, and
/// the destination it replaces it under exists; an id that destination holds nothing under
/// leaves the service with no definition to carry the request's definition over onto, and
/// RUM reports that as a failure of the service's own add rather than as a resource the
/// caller named wrongly: HTTP 500, which the model declares `retryable`, so an SDK's
/// standard retry policy retries the call. The only variable part of the message is the
/// app monitor the request named.
pub fn metric_definitions_not_added(app_monitor_name: &str) -> InternalServerException {
    InternalServerException {
        message: format!(
            "Error adding metric definitions for app monitor {app_monitor_name} to destination."
        ),
        retry_after_seconds: None,
    }
}

/// `ServiceQuotaExceededException` for a destination already holding the most metric
/// definitions it may ([`MAX_METRIC_DEFINITIONS_PER_DESTINATION`]).
pub fn metric_definitions_quota_exceeded(destination: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "The {destination} metrics destination already contains the maximum number of \
             metric definitions ({MAX_METRIC_DEFINITIONS_PER_DESTINATION})."
        ),
    }
}

/// The name of the CloudWatch Logs group an app monitor's telemetry is copied into:
/// the vended-logs prefix, the app monitor's own name, and the suffix the service chose
/// for that monitor.
///
/// One definition of the SHAPE of the name, used by the mint
/// ([`ComAmazonawsRumRumState::mint_cw_log_group`]) whose result every reader of the name
/// then goes through — the record's own [`AppMonitorRecord::cw_log_group`] (what
/// `GetAppMonitor` reports) and the handlers that PROVISION the group in CloudWatch Logs
/// when delivery is turned on — so the name RUM advertises and the group it creates cannot
/// differ.
pub fn cw_log_group_name(app_monitor_name: &str, suffix: &str) -> String {
    format!("{CW_LOG_GROUP_PREFIX}{app_monitor_name}{suffix}")
}

/// The ARN of that log group, which is the resource the caller's
/// `logs:PutResourcePolicy` permission is evaluated against.
///
/// A CloudWatch Logs log-group ARN ends in `:*`, the form CloudWatch Logs itself reports
/// (`service-com-amazonaws-cloudwatchlogs-logs-20140328`'s `log_group_arn`).
pub fn cw_log_group_arn(region: &str, log_group_name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("log-group:{log_group_name}:*"))
}

/// `AccessDeniedException` for a request IAM does not allow.
///
/// The message is the refusal's own wording — the principal, the action and the resource
/// the decision was made about — so a client can tell "this caller may not do this" from
/// any other 4xx.
pub fn access_denied(denial: &authorization::Denied) -> AccessDeniedException {
    AccessDeniedException {
        message: denial.message(),
    }
}

/// `AccessDeniedException` for a write whose target this service cannot resolve, so there
/// is no resource to authorize it against.
///
/// A write addressed by the minted `AppMonitorId` (`PutRumEvents`, the data plane) is
/// authorized against the monitor that id names: the decision needs that monitor's ARN and
/// its resource-based policy. An id no stored monitor holds — never minted, or minted for a
/// monitor since deleted — leaves nothing to decide against, so the grant cannot be
/// established and the write is refused as an authorization failure rather than as a
/// missing resource. The body carries [`CALLER_LACKS_PERMISSIONS_MESSAGE`] alone: it names
/// neither the id nor the action, so a caller learns nothing about whether a monitor with
/// that id exists.
pub fn caller_lacks_permissions() -> AccessDeniedException {
    AccessDeniedException {
        message: CALLER_LACKS_PERMISSIONS_MESSAGE.to_string(),
    }
}

/// The whole message of [`caller_lacks_permissions`].
///
/// A CONSTANT rather than a format string: nothing of the request appears in it, which is
/// the point of this refusal — the resource it was asked about is exactly what it must not
/// describe.
pub const CALLER_LACKS_PERMISSIONS_MESSAGE: &str =
    "Caller principal does not have permissions for that resource.";

/// The wire error code, and the HTTP status, a request IAM does not allow is refused with.
///
/// One definition for the whole service, beside the body [`access_denied`] builds: the
/// code and the status are a property of the refusal rather than of the operation that
/// makes it, and this service's operations make the same one. The operations whose model
/// DECLARES `AccessDeniedException` take both from that shape's own schema and
/// `@httpError`; these two are for the ARN-addressed tagging operations, whose modelled
/// error lists do not declare it — `AccessDeniedException` is a common error rather than
/// one specific to an operation — and which therefore name the code and the status on
/// their own error channel.
pub const ACCESS_DENIED_ERROR_CODE: &str = "AccessDeniedException";
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// The whole message RUM's ARN-addressed tagging operations report for a resource they
/// cannot find.
///
/// Unlike the app-monitor operations' not-found message, which is built from the request
/// (it names the `Name` asked for), this one carries NOTHING from the request: the
/// recorded corpus unit `rum.untag-resource-unknown-resource-rejected` pins the message
/// for an unknown ARN as this exact sentence, and it is the same sentence whatever ARN the
/// request carried. So it is a constant here rather than a format string.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The HTTP status RUM's ARN-addressed tag WRITES answer on success: 204 No Content.
///
/// `TagResource` and `UntagResource` are the two operations of this service whose
/// modelled output declares no member at all, and both are served by the tagging
/// frontend: there is nothing for the answer to carry, and the status the service sends
/// says so. The AWS CloudWatch RUM API reference gives `HTTP/1.1 204` as the response
/// syntax of both, and every recorded response of either in the corpus is a 204 with no
/// content (eleven steps across five fixtures). The model's own `@http.code` is 200 for
/// both — defaulted on `TagResource`, declared on `UntagResource`
/// (`smithy_query(query="operation", operation="TagResource,UntagResource")`) — but a
/// status is what the service PUTS ON THE WIRE, and a client that distinguishes 204 from
/// 200 (empty-body handling, a cache, an assertion on the success class) sees the
/// difference, so the recorded behaviour is what the two handlers answer with.
///
/// `ListTagsForResource`, the frontend's third operation, reports a document and keeps
/// the 200 its own output needs; every other operation of this service is addressed by
/// `Name` or by `Id`, reports a document, and keeps its modelled status.
pub const TAGGING_NO_CONTENT_STATUS: u16 = 204;

/// `ResourceNotFoundException` for a `ResourceArn` that names no resource this service
/// holds — the not-found refusal every tagging operation answers (HTTP 404).
///
/// `message` is the service's own constant wording ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`])
/// and is the whole body: like every other not-found of this service, the refusal reports
/// no `resourceName` and no `resourceType` (see
/// [`crate::ops::types::ResourceNotFoundException`]). The ARN is therefore taken only to
/// decide that nothing matched it, which the caller already knows it sent.
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The wire error code RUM's ARN-addressed tagging frontend names a resource it cannot find
/// by, and the one it refuses an invalid request with.
///
/// The model declares `ResourceNotFoundException` and `ValidationException` on each of
/// those operations, and those shapes' `@httpError`s and members are what the responses
/// carry — but the codes this frontend puts in the body's `__type` are the shorter
/// `NotFoundException` and `BadRequestException`, which the model does not declare at all.
/// The recorded units pin both: `rum.untag-resource-unknown-resource-rejected` asserts
/// `err["Code"] == "NotFoundException"` for an ARN that resolves to nothing, and
/// `rum.tag-resource-invalid-key-rejected` / `rum.tag-resource-invalid-key-format-rejected`
/// assert `err["Code"] == "BadRequestException"` for a tag key the frontend refuses. They
/// are specific to the ARN-addressed tagging operations: RUM's app-monitor operations,
/// addressed by `Name`, do answer the modelled codes.
///
/// Here rather than on an operation, for the same reason as [`ACCESS_DENIED_ERROR_CODE`]:
/// the code is a property of the refusal and of the frontend that makes it, not of
/// whichever of the three operations was called. `crate::service` words a tag refusal the
/// BOUNDARY makes with the second one.
pub const TAGGING_NOT_FOUND_ERROR_CODE: &str = "NotFoundException";
pub const TAGGING_BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// A refusal RUM's ARN-addressed tagging frontend words itself, carrying the ONE mapping
/// from each refusal class to the wire code, the HTTP status and the body it answers with.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` are one frontend, so they
/// cannot word one condition two ways: an ARN naming no resource is
/// [`TAGGING_NOT_FOUND_ERROR_CODE`] carrying `ResourceNotFoundException`'s payload and its
/// 404, a request the frontend refuses is [`TAGGING_BAD_REQUEST_ERROR_CODE`] carrying
/// `ValidationException`'s payload and its 400, and a request IAM does not allow is
/// [`ACCESS_DENIED_ERROR_CODE`] carrying the modelled `AccessDeniedException` payload and
/// [`ACCESS_DENIED_STATUS`]. Each of the three operations' error channels wraps this type
/// and delegates every `smithy_server_runtime::OperationError` method to the methods below,
/// so the mapping is declared once here instead of once per operation — the move this
/// service already made for the page-size rule ([`page_size`]) and the destination-ARN rule
/// ([`declared_arn`]).
///
/// A channel whose handler reaches only some of these classes still carries the type
/// whole: which refusals an operation CONSTRUCTS is that handler's business, while what
/// each refusal looks like on the wire is the frontend's.
#[derive(Debug)]
pub enum TaggingRefusal {
    /// "That ARN names no resource" — [`tagged_resource_not_found`]'s body.
    NotFound(ResourceNotFoundException),
    /// "That request is not valid", worded by whichever rule refused it (a tag key outside
    /// the constraints its shape declares, a write past [`MAX_TAGS_PER_RESOURCE`]).
    BadRequest(ValidationException),
    /// "This caller may not perform this action on this resource" — [`access_denied`]'s
    /// body.
    AccessDenied(AccessDeniedException),
}

impl TaggingRefusal {
    /// The not-found refusal for a `ResourceArn` naming no resource this service holds.
    pub fn not_found() -> Self {
        Self::NotFound(tagged_resource_not_found())
    }

    /// The invalid-request refusal, stating `message` — the words the rule that refused the
    /// request produced.
    pub fn bad_request(message: String) -> Self {
        Self::BadRequest(ValidationException { message })
    }

    /// The authorization refusal for a request the policies in force do not allow.
    pub fn denied(denial: &authorization::Denied) -> Self {
        Self::AccessDenied(access_denied(denial))
    }

    /// The body this refusal serializes: the modelled shape itself, so each class's own
    /// schema decides which members the answer carries.
    pub fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::NotFound(e) => e,
            Self::BadRequest(e) => e,
            Self::AccessDenied(e) => e,
        }
    }

    /// The schema the answer is written against: the modelled shape's own.
    pub fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::NotFound(_) => &crate::ops::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
            Self::BadRequest(_) => &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            Self::AccessDenied(_) => &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
        }
    }

    /// The code on the wire, which the framework reads from an operation's `shape_name`
    /// rather than from the schema — the one respect in which these refusals differ from
    /// the modelled shapes they carry.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => TAGGING_NOT_FOUND_ERROR_CODE,
            Self::BadRequest(_) => TAGGING_BAD_REQUEST_ERROR_CODE,
            Self::AccessDenied(_) => ACCESS_DENIED_ERROR_CODE,
        }
    }

    /// The HTTP status: each carried shape's own `@httpError`, so the code on the wire
    /// changes and the status does not.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::BadRequest(_) => 400,
            Self::AccessDenied(_) => ACCESS_DENIED_STATUS,
        }
    }

    /// No `awsQueryError` code: restJson1 is this service's only protocol, and none of
    /// these shapes carries that trait to honour in any case.
    pub fn query_error_code(&self) -> Option<&'static str> {
        None
    }

    /// The sentence the body states, for the `Display` rendering below.
    fn message(&self) -> &str {
        match self {
            Self::NotFound(e) => &e.message,
            Self::BadRequest(e) => &e.message,
            Self::AccessDenied(e) => &e.message,
        }
    }
}

impl ::std::fmt::Display for TaggingRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.message())
    }
}

impl ::std::error::Error for TaggingRefusal {}

/// The most tags one RUM resource holds at once.
///
/// From the model: `TagResource` documents "You can associate as many as 50 tags with a
/// resource", `CreateAppMonitor$Tags` documents "You can associate as many as 50 tags with
/// an app monitor", and `UntagResource$TagKeys` carries the matching
/// `@length(min: 0, max: 50)`. It is also what bounds the heap one app monitor's tag map
/// can take: `TagMap` itself declares no length, so every operation that WRITES tags is
/// what keeps the map inside the documented ceiling.
///
/// The CEILING is documented; the wire code a request past it is refused with is not, and
/// no recorded unit exercises it, so each write path answers it with the refusal code its
/// own frontend uses for an invalid request.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Every constraint the modelled `TagKey` rejects these keys on, each at the dotted member
/// path `keyed` spells that key's position as — empty when every one of them satisfies
/// every constraint `TagKey` declares.
///
/// EVERY offending key yields its violation: a refusal then carries one clause per failing
/// (key, requirement) pair, in the order `keyed` yields them, so a caller sending several
/// bad keys is told about each of them in one answer instead of one per round trip. That is
/// the rule [`validation`] states for every constraint refusal this service words, and a
/// key carried by a map and a key carried by a list are the same kind of failure, so every
/// caller below collects its keys through here — what differs between them is the PATH a
/// clause reports a key at, and which frontend's words state it.
///
/// These checks belong in the handlers rather than at the boundary: the vendored boundary
/// validator does not validate a map KEY at all — it captures the key as the path segment
/// of the entry's VALUE and moves on (`write_leaf` in
/// `vendor/smithy-schema-server/src/constraints.rs`: "the key itself is not a constrained
/// leaf") — and `TagKey`'s `@pattern` would be unapplied even on a path that did validate
/// it, because it opens with a negative lookahead the validator's engine cannot compile.
/// So the constraints the model declares on a tag key would otherwise never be applied at
/// all.
///
/// The CHECK is [`validation`]'s, which is where this service applies a modelled `@length`
/// and `@pattern` itself. The violation is handed back STRUCTURALLY, unworded, because
/// which words state it is the answering frontend's choice and not this check's: an
/// ARN-addressed tag call keeps the framework's own sentence under that frontend's error
/// code ([`tag_key_refusal`]), and an app-monitor call states it in this service's one
/// constraint wording (`validation::SelfApplied::refusal`). The key is echoed into its
/// clause exactly as the caller sent it.
fn tag_key_violations<'a>(
    keyed: impl IntoIterator<Item = (String, &'a str)>,
) -> Vec<::aws_smithy_schema_server::constraints::Violation> {
    keyed
        .into_iter()
        .filter_map(|(path, key)| {
            validation::string_violation(validation::ConstrainedString::TagKey, &path, key)
        })
        // The bound the framework's own walk collects to
        // (`service_common::validation::MAX_VIOLATIONS`): a request past it is refused with
        // the violations found so far, so the message a refusal states is not sized by how
        // many keys a caller sent.
        .take(service_common::validation::MAX_VIOLATIONS)
        .collect()
}

/// [`tag_key_violations`] in the words RUM's ARN-addressed TAGGING frontend states a
/// refusal in: the framework's own validation envelope, which is what that frontend's
/// refusals are recorded carrying under its own error code (`crate::service`, and
/// [`TAGGING_BAD_REQUEST_ERROR_CODE`]). `None` when every key is admitted.
fn tag_key_refusal<'a>(keyed: impl IntoIterator<Item = (String, &'a str)>) -> Option<String> {
    // `envelope` answers `None` for an EMPTY violation list, which is exactly the case
    // where every key is admitted; a non-empty list always states its clauses.
    ::aws_smithy_schema_server::envelope(&tag_key_violations(keyed))
}

/// The dotted member path a violation on the tag MAP a request carries is reported at: the
/// lowerCamel spelling of `TagResource$Tags` and `CreateAppMonitor$Tags`, which is the
/// member every tag map this service WRITES is carried by.
const TAG_MAP_PATH: &str = "tags";

/// The tag keys of a request carried by the tag MAP, each paired with the dotted member
/// path a clause reports that entry at, in the order a refusal states them.
///
/// The path is the map member followed by the key the CALLER chose and nothing else: that
/// is the segment the framework's walk spells a TERMINAL map value with
/// (`begin_collection_child` in `vendor/smithy-schema-server/src/constraints.rs`, whose
/// `member` step is written only when DESCENDING into a structure value), and a `TagMap`
/// value is a scalar. It is the same spelling the sibling `DimensionKeysMap` entries are
/// reported at ([`dimension_keys_violations`]), so two map-entry violations of one kind are
/// not reported at two path shapes. The keys are judged in SORTED order, so the order a
/// multi-key refusal states its clauses in is fixed by the request rather than by a
/// `HashMap`'s iteration order.
fn tag_map_keyed<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<(String, &'a str)> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.into_iter()
        .map(|key| (format!("{TAG_MAP_PATH}.{key}"), key))
        .collect()
}

/// Why these tag keys cannot be written by a call of the ARN-addressed TAGGING frontend, or
/// `None` when every one of them satisfies the constraints the modelled `TagKey` declares.
///
/// The constraints are the map's, declared once on `TagMap`'s key shape and not the
/// property of whichever operation happens to carry it, so they are collected by the one
/// [`tag_key_violations`] every tag-key check of this service goes through — this is the
/// half that chooses that frontend's WORDS for them. The app-monitor create that seeds a
/// monitor's map applies the same constraints at the same paths and states them in this
/// service's own constraint wording ([`tag_map_key_self_applied`] carries the violations and
/// `validation::SelfApplied::refusal` words them), because that call is
/// answered by the app-monitor frontend. A tag key carried by a LIST rather than by the map
/// — the keys a removal NAMES — is judged by [`tag_key_list_refusal`], which applies these
/// same constraints at the path a list element is reported at.
pub fn tag_keys_refusal<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    tag_key_refusal(tag_map_keyed(keys))
}

/// The violations these tag keys commit against the modelled `TagKey`, unworded, for the
/// two callers that word them differently.
///
/// Same constraints, same paths and same completeness as [`tag_keys_refusal`] — both go
/// through [`tag_key_violations`] over [`tag_map_keyed`], the constraints being `TagMap`'s
/// key shape's rather than an operation's. What differs is the WORDS: the app-monitor
/// frontend states them through the one place a constraint refusal of this service is worded
/// ([`validation`], reached by `validation::SelfApplied::refusal`), so a create refused on a
/// tag key states its clause the way the same call's refusal on any other member does — the
/// modelled `@pattern` without the `^`/`$` pair the model writes around it, under the
/// modelled `ValidationException` an app-monitor call answers with.
///
/// Handed back unworded because the GATE in front of every operation reads it too
/// (`crate::service`): a request whose tag map carries a key the modelled `TagKey` rejects
/// AND breaks a constraint the framework's boundary walk applies is stated in ONE envelope,
/// which the handler it would have been refused by never gets to do.
#[must_use]
pub fn tag_map_key_self_applied<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> validation::SelfApplied {
    self_applied(tag_key_violations(tag_map_keyed(keys)))
}

/// The dotted member path a violation on the tag-key LIST a request carries is reported
/// at: the lowerCamel spelling of `UntagResource$TagKeys`, which is the member every
/// tag-key list of this service is carried by.
const TAG_KEY_LIST_PATH: &str = "tagKeys";

/// Why these tag keys cannot be NAMED, or `None` when every one of them satisfies the
/// constraints the modelled `TagKey` declares.
///
/// The constraints are the same ones [`tag_keys_refusal`] applies — they are declared once,
/// on the shape every tag key of this service targets, so they hold for a key a request
/// names as well as for one it writes — and the `@pattern` half is unapplied at the
/// boundary for the same reason: the validator's engine cannot compile a pattern opening
/// with a negative lookahead and treats one it cannot compile as matching.
///
/// What differs from the map form is only WHERE a clause reports the offending key: a key
/// carried by a LIST is reported at the position it holds in that list, which is the path
/// the framework's walk spells a list element as (`<list>.<1-based position>.member`), and
/// the keys are judged in the order the request listed them. EVERY offending key is stated
/// here as it is on the map side, because every tag-key check of this service collects
/// through the one [`tag_key_violations`] — so a caller is told about each key it has to
/// fix. The removal is the ARN-addressed tagging frontend's own operation, so its clauses
/// keep that frontend's words ([`tag_key_refusal`]).
pub fn tag_key_list_refusal<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    tag_key_refusal(tag_key_list_keyed(keys))
}

/// The same violations, unworded, for the gate in front of the operation
/// ([`tag_map_key_self_applied`] states why).
#[must_use]
pub fn tag_key_list_self_applied<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> validation::SelfApplied {
    self_applied(tag_key_violations(tag_key_list_keyed(keys)))
}

/// The tag keys a request NAMES in a list, each paired with the dotted member path a clause
/// reports that key at: the path the framework's walk spells a list element as, over the
/// keys in the order the request listed them. The sibling of [`tag_map_keyed`], so the two
/// forms of one check differ in nothing but that path.
fn tag_key_list_keyed<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<(String, &'a str)> {
    keys.into_iter()
        .enumerate()
        .map(|(position, key)| {
            let path = format!(
                "{TAG_KEY_LIST_PATH}.{}",
                ::aws_smithy_schema_server::constraints::path::element(position + 1)
            );
            (path, key)
        })
        .collect()
}

/// Why a tag write that would leave `resulting` tags on the resource `resource` names
/// cannot be applied, or `None` when it stays inside [`MAX_TAGS_PER_RESOURCE`].
///
/// `resulting` is counted on the map the write would LEAVE BEHIND, so a key already on the
/// resource replaces a value rather than adding an entry. Shared for the same reason as
/// [`tag_keys_refusal`]: the ceiling is the resource's, so a create that seeds the map and
/// a tag call that merges onto it are bounded by the one number.
pub fn tag_count_refusal(resulting: usize, resource: &str) -> Option<String> {
    (resulting > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "These tags would leave {resulting} tags on {resource}, more than the \
             {MAX_TAGS_PER_RESOURCE} tags a resource can be associated with."
        )
    })
}

/// The stored form of a `PolicyDocument`: the submitted JSON text with every
/// insignificant whitespace character removed.
///
/// The service parses the document it is given (it refuses one that is not a well-formed
/// policy, see [`policy_document_refusal`]) and keeps the parse's canonical text, so what
/// a later read reports is the policy's content and not the caller's layout: whitespace
/// between JSON tokens carries no meaning. Nothing INSIDE the document is touched —
/// whitespace in a string literal is content, element order, numbers and escapes are kept
/// exactly as written — so the document still holds the same statements, in the same
/// order, as the caller wrote them.
pub fn canonical_policy_document(document: &str) -> String {
    let mut canonical = String::with_capacity(document.len());
    // Whether the scanner is inside a JSON string literal, and whether the previous
    // character opened an escape sequence inside one (so an escaped quote does not end
    // the literal).
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            canonical.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            canonical.push(character);
        } else if !character.is_whitespace() {
            canonical.push(character);
        }
    }
    canonical
}

/// Why this `PolicyDocument` is not a resource policy this service can store, or `None`
/// when it is one.
///
/// `PolicyDocument` is modelled as a bare `String`, so NOTHING about its contents is
/// refused before a handler runs; `MalformedPolicyDocumentException` — "The policy
/// document that you specified is not formatted correctly" — is the modelled refusal for
/// a document that is not a well-formed IAM JSON policy. The grammar checked here is the
/// IAM JSON policy language's own: the document parses as JSON (by `serde_json`, this
/// crate's JSON parser) into an object whose only elements are
/// [`POLICY_DOCUMENT_ELEMENTS`], it carries at least one statement, and every statement
/// element has the type and the value set that language gives it. Semantics beyond the
/// grammar (whether an action exists, whether a principal resolves) are NOT checked:
/// they are not what "formatted correctly" means.
///
/// The reason is what DECIDES the refusal; it is not what the refusal reports. RUM answers
/// this error class with the code alone ([`malformed_policy_document`]), so the sentences
/// below state the grammar for a reader of this crate rather than for a caller.
pub fn policy_document_refusal(document: &str) -> Option<String> {
    let parsed = match ::serde_json::from_str::<::serde_json::Value>(document) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!(
                "The policy document is not valid JSON: {error}."
            ))
        }
    };
    let Some(elements) = parsed.as_object() else {
        return Some("The policy document must be a JSON object.".to_string());
    };
    if let Some(unknown) = elements
        .keys()
        .find(|key| !POLICY_DOCUMENT_ELEMENTS.contains(&key.as_str()))
    {
        return Some(format!(
            "The policy document carries the unknown element {unknown}; a policy \
             document's elements are {}.",
            POLICY_DOCUMENT_ELEMENTS.join(", ")
        ));
    }
    if let Some(version) = elements.get("Version") {
        let declared = version.as_str();
        if !declared.is_some_and(|value| POLICY_DOCUMENT_VERSIONS.contains(&value)) {
            return Some(format!(
                "The policy document's Version must be one of {}.",
                POLICY_DOCUMENT_VERSIONS.join(", ")
            ));
        }
    }
    if elements.get("Id").is_some_and(|id| !id.is_string()) {
        return Some("The policy document's Id must be a string.".to_string());
    }
    let statements = match elements.get("Statement") {
        None => {
            return Some("The policy document must carry a Statement element.".to_string())
        }
        // A single statement may be given as the object itself rather than as a
        // one-element list.
        Some(::serde_json::Value::Object(statement)) => {
            vec![::serde_json::Value::Object(statement.clone())]
        }
        Some(::serde_json::Value::Array(statements)) if !statements.is_empty() => {
            statements.clone()
        }
        Some(_) => {
            return Some(
                "The policy document's Statement must be a statement object or a \
                 non-empty list of them."
                    .to_string(),
            )
        }
    };
    statements.iter().find_map(policy_statement_refusal)
}

/// Why one element of a policy document's `Statement` is not a well-formed statement, or
/// `None` when it is one.
fn policy_statement_refusal(statement: &::serde_json::Value) -> Option<String> {
    let Some(members) = statement.as_object() else {
        return Some("Each policy statement must be a JSON object.".to_string());
    };
    match members.get("Effect").and_then(::serde_json::Value::as_str) {
        Some(effect) if POLICY_STATEMENT_EFFECTS.contains(&effect) => {}
        _ => {
            return Some(format!(
                "Each policy statement must carry an Effect of {}.",
                POLICY_STATEMENT_EFFECTS.join(" or ")
            ))
        }
    }
    // The IAM policy language takes the action either way round, but never both and
    // never neither: a statement with no action element applies to nothing.
    if members.contains_key("Action") == members.contains_key("NotAction") {
        return Some(
            "Each policy statement must carry exactly one of Action and NotAction.".to_string(),
        );
    }
    for element in ["Action", "NotAction", "Resource", "NotResource"] {
        if let Some(value) = members.get(element) {
            if !is_string_or_non_empty_string_list(value) {
                return Some(format!(
                    "A policy statement's {element} must be a string or a non-empty list \
                     of strings."
                ));
            }
        }
    }
    for element in ["Principal", "NotPrincipal"] {
        if let Some(value) = members.get(element) {
            if !is_policy_principal(value) {
                return Some(format!(
                    "A policy statement's {element} must be a string or a map of \
                     principal type to one or more principals."
                ));
            }
        }
    }
    if members.get("Sid").is_some_and(|sid| !sid.is_string()) {
        return Some("A policy statement's Sid must be a string.".to_string());
    }
    if members
        .get("Condition")
        .is_some_and(|condition| !is_policy_condition(condition))
    {
        return Some(
            "A policy statement's Condition must be a map of condition operator to a map \
             of condition key to its values."
                .to_string(),
        );
    }
    None
}

/// Whether `value` is how the IAM policy language writes one or more strings: the string
/// itself, or a non-empty list of strings.
fn is_string_or_non_empty_string_list(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(::serde_json::Value::is_string)
        }
        _ => false,
    }
}

/// Whether `value` is a `Principal`/`NotPrincipal` element: the wildcard string, or a map
/// of principal type (`AWS`, `Service`, `Federated`, `CanonicalUser`) to one or more
/// principals.
fn is_policy_principal(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Object(by_type) => {
            !by_type.is_empty()
                && by_type
                    .values()
                    .all(is_string_or_non_empty_string_list)
        }
        _ => false,
    }
}

/// Whether `value` is a `Condition` element: a map of condition operator to a map of
/// condition key to the value(s) it is compared against.
fn is_policy_condition(value: &::serde_json::Value) -> bool {
    value.as_object().is_some_and(|by_operator| {
        by_operator.values().all(|by_key| {
            by_key
                .as_object()
                .is_some_and(|keys| keys.values().all(is_policy_condition_value))
        })
    })
}

/// Whether `value` is a condition key's right-hand side: a string, a boolean, a number,
/// or a non-empty list of those.
fn is_policy_condition_value(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(_)
        | ::serde_json::Value::Bool(_)
        | ::serde_json::Value::Number(_) => true,
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(is_policy_condition_value)
        }
        _ => false,
    }
}

/// `MalformedPolicyDocumentException` as RUM answers it: the error CLASS alone.
///
/// Recorded behaviour: the body carries no `message` member at all — the model declares
/// one, but the service sends none for this class, so a caller that submitted a document
/// the policy grammar refuses learns the class and nothing more. Which rule of the grammar
/// the document broke ([`policy_document_refusal`]) therefore stays inside the service, and
/// no diagnostic of this crate's JSON parser reaches the wire.
pub fn malformed_policy_document() -> MalformedPolicyDocumentException {
    MalformedPolicyDocumentException {}
}

/// The message RUM answers `PolicyNotFoundException` with.
///
/// Recorded behaviour: the message is this CONSTANT, carrying no part of the request —
/// the monitor the caller asked about is already the one it addressed, so the error says
/// only that the resource policy is not there. (The shape models `message` and nothing
/// else, so the whole body is this one member.)
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Resource policy does not exist";

/// `PolicyNotFoundException` for an app monitor that exists but carries no resource
/// policy.
pub fn policy_not_found() -> PolicyNotFoundException {
    PolicyNotFoundException {
        message: POLICY_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The message `PutResourcePolicy` answers `InvalidPolicyRevisionIdException` with, for
/// EVERY request of that operation its optimistic-lock check refuses.
///
/// Recorded behaviour: the message is this CONSTANT — wholly fixed text, carrying no part
/// of the request, so neither the id the request named nor the one the policy currently
/// holds appears in it. A caller that holds the wrong revision id has to re-read the
/// policy to learn the current one, and nothing in the refusal tells it more than that its
/// write did not apply.
pub const PUT_POLICY_REVISION_MISMATCH_MESSAGE: &str =
    "If policyRevisionID is provided, then it must match the existing revision.";

/// The message `DeleteResourcePolicy` answers `InvalidPolicyRevisionIdException` with.
///
/// Recorded behaviour: this CONSTANT, with no trailing period and no part of the request
/// in it — and deliberately NOT the sentence `PutResourcePolicy` answers with
/// ([`PUT_POLICY_REVISION_MISMATCH_MESSAGE`]), so a caller reconciling puts and deletes
/// can tell which of the two optimistic-lock checks refused it.
pub const DELETE_POLICY_REVISION_MISMATCH_MESSAGE: &str =
    "PolicyRevisionID must match the existing revision";

/// `InvalidPolicyRevisionIdException` as `PutResourcePolicy` words it: its own fixed
/// sentence ([`PUT_POLICY_REVISION_MISMATCH_MESSAGE`]).
pub fn put_policy_revision_mismatch() -> InvalidPolicyRevisionIdException {
    InvalidPolicyRevisionIdException {
        message: PUT_POLICY_REVISION_MISMATCH_MESSAGE.to_string(),
    }
}

/// `InvalidPolicyRevisionIdException` as `DeleteResourcePolicy` words it: its own fixed
/// sentence ([`DELETE_POLICY_REVISION_MISMATCH_MESSAGE`]).
pub fn delete_policy_revision_mismatch() -> InvalidPolicyRevisionIdException {
    InvalidPolicyRevisionIdException {
        message: DELETE_POLICY_REVISION_MISMATCH_MESSAGE.to_string(),
    }
}

/// `ConflictException` for a create whose `Name` a stored app monitor already holds.
///
/// Like the not-found message, this one is built from the request: the literal `Resource `,
/// the requested name verbatim, and the literal ` already exists`, with no trailing
/// period. Unlike it, the body also reports the resource — `resourceName` is the name the
/// request asked for and `resourceType` is [`APP_MONITOR_RESOURCE_TYPE`] — because RUM
/// does serialize both members on this error.
pub fn app_monitor_already_exists(name: &str) -> ConflictException {
    ConflictException {
        message: format!("{CONFLICT_MESSAGE_PREFIX}{name}{CONFLICT_MESSAGE_SUFFIX}"),
        resource_name: name.to_string(),
        resource_type: Some(APP_MONITOR_RESOURCE_TYPE.to_string()),
    }
}

/// `ConflictException` for a write that would leave one extended-metrics destination
/// holding a SECOND definition of a metric it already has a definition for, naming the
/// definition that already exists by the id it is addressable under
/// (`metric_definition_id`).
///
/// The same error class, and so the same wording, as the duplicate-name conflict above:
/// the resource the request asked the service to store is already there, under the
/// identity reported here, and that existing resource is what the caller has to address
/// instead. No `resourceType` is reported, because the only recorded conflict of this
/// service reports the app-monitor noun ([`APP_MONITOR_RESOURCE_TYPE`]) and nothing
/// records the noun RUM would use for a metric definition; the member is optional, and
/// every other refusal of this service leaves an unrecorded member off the wire.
pub fn metric_definition_already_exists(metric_definition_id: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "{CONFLICT_MESSAGE_PREFIX}{metric_definition_id}{CONFLICT_MESSAGE_SUFFIX}"
        ),
        resource_name: metric_definition_id.to_string(),
        resource_type: None,
    }
}

/// The literals the duplicate-name conflict message brackets the requested name with.
pub const CONFLICT_MESSAGE_PREFIX: &str = "Resource ";
/// See [`CONFLICT_MESSAGE_PREFIX`].
pub const CONFLICT_MESSAGE_SUFFIX: &str = " already exists";

/// The message a BATCH create reports per definition the addressed destination already
/// holds — the per-item conflict, so what it names is the definition the request asked for
/// rather than an id.
///
/// The requested definition is printed the way the service prints a modelled structure: the
/// shape's name over its members in the order the model declares them, each member as
/// `name=value`, with the literal `null` for a member the request left out — so a caller
/// reading the message sees exactly the definition that was refused, including which of its
/// optional members carried nothing. A `DimensionKeys` map prints as its entries in KEY
/// order, which is the one order one request's map can be printed in twice: the map reaches
/// a handler as the built input's own unordered map, so the order the request spelled its
/// entries in is not recoverable here.
pub fn metric_definition_already_defined(definition: &MetricDefinitionRequest) -> String {
    let dimension_keys = definition
        .dimension_keys
        .as_ref()
        .map(|keys| {
            let entries: BTreeMap<&String, &String> = keys.iter().collect();
            let rendered: Vec<String> = entries
                .into_iter()
                .map(|(field, dimension)| format!("{field}={dimension}"))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        })
        .unwrap_or_else(|| PRINTED_NULL.to_string());
    format!(
        "Metric definition already exists: MetricDefinitionRequest(name={}, valueKey={}, \
         unitLabel={}, dimensionKeys={dimension_keys}, eventPattern={}, namespace={})",
        definition.name,
        printed(definition.value_key.as_deref()),
        printed(definition.unit_label.as_deref()),
        printed(definition.event_pattern.as_deref()),
        printed(definition.namespace.as_deref()),
    )
}

/// How a printed structure spells a member the request left out.
const PRINTED_NULL: &str = "null";

/// An optional member as a printed structure spells it: the value the request carried, or
/// [`PRINTED_NULL`] for a member it left out. An empty value is a value, so it prints as
/// itself.
fn printed(value: Option<&str>) -> &str {
    value.unwrap_or(PRINTED_NULL)
}

/// `ServiceQuotaExceededException` for a create that would take the store past an
/// app-monitor ceiling, naming the ceiling it reached.
///
/// ONE wording for both ceilings a create is subject to — the per-region allowance
/// ([`MAX_APP_MONITORS`]) and, for a create that would open a bucket past
/// [`MAX_REGIONS`], the store's total — so the two refusals cannot drift apart. The
/// shape models `message` and nothing else, so the whole body is this one member.
pub fn app_monitor_quota_exceeded(ceiling: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!("The maximum number of app monitors ({ceiling}) has already been created."),
    }
}

/// The media type RUM's app-monitor, resource-policy and metrics frontends announce for the
/// JSON body of an answer to a request that carried no payload, and so declared no media
/// type of its own.
///
/// RECORDED BEHAVIOUR: the model declares `restJson1`, whose serializer stamps
/// [`JSON_CONTENT_TYPE`] on a JSON body, but a recorded real-RUM answer to a request with
/// no payload — `GetAppMonitor`, `ListAppMonitors`, `DeleteAppMonitor`, the resource-policy
/// reads, `ListRumMetricsDestinations`, the metric-definition reads, their 404s included —
/// carries `application/x-amz-json-1.1`: the frontend answers in the media type the
/// request's payload declared, and a payload-less request declared none, so it falls back
/// to RUM's own JSON type. An answer to a request that DID carry a payload
/// (`CreateAppMonitor`, `UpdateAppMonitor`, `PutResourcePolicy`, `PutRumEvents`,
/// `BatchCreateRumMetricDefinitions`, their 4xxs included) is recorded in
/// [`JSON_CONTENT_TYPE`], the type that payload declared. The body is the same JSON
/// either way, so nothing but the announced type differs.
///
/// It is NOT what the ARN-addressed TAGGING frontend announces: a recorded
/// `ListTagsForResource` 200 and a recorded `UntagResource` 204 — both answers to a
/// payload-less request — carry [`JSON_CONTENT_TYPE`], as do that frontend's recorded 400
/// (a `ResourceArn` that is not an ARN) and 404 (one naming no resource). That frontend has
/// its own error vocabulary too ([`TAGGING_BAD_REQUEST_ERROR_CODE`]); the media type is the
/// same split, so which frontend answers the call is what settles the header
/// (`crate::service::serves_the_tagging_frontend`).
///
/// How the payload half is known: a replay of the recorded fixture
/// `rum-multi-platform-monitors` with `application/x-amz-json-1.1` announced on EVERY
/// answer reports `rum|CreateAppMonitor|2xx->2xx|$headers.content-type` as a divergence on
/// a step whose status and body both match, and the same replay passes end to end once a
/// payload-carrying request is answered in [`JSON_CONTENT_TYPE`]. Before the blanket
/// re-announce existed this service answered `application/json` everywhere and no sweep
/// ever filed a `CreateAppMonitor` content-type cluster, which agrees: the create's
/// recorded header was matching then.
///
/// Applied in [`crate::service::EmulatedService`], the one place every response this
/// service produces passes through.
pub const RESPONSE_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the `restJson1` serializer stamps on a JSON body — the one value
/// [`RESPONSE_CONTENT_TYPE`] replaces, the one a request's JSON payload declares, and the
/// one the ARN-addressed tagging frontend announces on every body it answers with. A
/// response whose body is NOT that JSON envelope (a `@httpPayload` blob's
/// `application/octet-stream`, a string payload's `text/plain`) announces the type its own
/// target shape maps to and is left alone.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// What separates the components of a continuation token's resume key: a control
/// character none of the components that precede the last one can contain (an
/// `AppMonitorName` matches `^(?!\.)[\.\-_#A-Za-z0-9]+$` and a `MetricDestination` is
/// one of two words). The LAST component needs no such guarantee, because
/// [`decode_pagination_token`] splits at most `components - 1` times and hands whatever
/// follows back whole — which matters for a `DestinationArn`, whose
/// `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*` pattern ends in an unconstrained `.*`. The region the
/// token carries ahead of the key needs no such guarantee either, because the decoder
/// knows which region is asking and matches that whole string as a literal prefix.
const PAGINATION_TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque continuation token for the last entry a page returned: `marker`, the region
/// the issuing request was signed for, then the components of that entry's key.
///
/// ONE codec for every paginated operation of this service (`ListAppMonitors`,
/// `ListRumMetricsDestinations`, `BatchGetRumMetricDefinitions`, `GetAppMonitorData`), so
/// their token formats cannot drift. Hex keeps the token printable and opaque to a
/// client — `NextToken` is an unconstrained `String`, so a client must pass back what it
/// was handed rather than construct one — while staying decodable by the next call.
/// `marker` is the issuing operation's own (see [`decode_pagination_token`]).
///
/// `region` is in every token because the collection a token indexes is a REGION's: this
/// service's resources belong to the region the request that created them was signed for
/// ([`AppMonitorRecord::region`]), so the same name denotes an independent resource at
/// each regional endpoint and two of them hold keys in common.
pub fn encode_pagination_token(marker: &str, region: &str, resume_after: &[&str]) -> String {
    let key = resume_after.join(&PAGINATION_TOKEN_SEPARATOR.to_string());
    hex::encode(format!("{marker}{region}{PAGINATION_TOKEN_SEPARATOR}{key}"))
}

/// The `components` of the key a token issued by [`encode_pagination_token`] under
/// `marker` for `region` resumes after, or `None` for any value that operation never
/// issued at that region's endpoint.
///
/// The marker is what makes a token the ISSUING operation's: without it any
/// hex-decodable string would be read as a continuation, and one list's token would
/// resume another's. The region is what makes it the issuing ENDPOINT's: a listing walks
/// the resources of the region its request was signed for, so a token presented at
/// another region's endpoint names a position in a collection that listing never walked.
/// A listing scoped to a parent resource puts that parent's name in the key as well, so
/// the caller can also check a token was issued for the resource it is resuming. The
/// value is read in the exact spelling [`encode_pagination_token`] writes and no other: a
/// token is opaque, so a client passes back what it was handed rather than a value of its
/// own that happens to carry the same bytes.
pub fn decode_pagination_token(
    marker: &str,
    region: &str,
    token: &str,
    components: usize,
) -> Option<Vec<String>> {
    // A token is honoured only when it is the value a page actually handed out, character
    // for character. The hex layer is where that can be lost: decoding admits `A`-`F` as
    // well as the `a`-`f` [`encode_pagination_token`] writes, so without this check every
    // re-spelling of an issued token's digits — a client upper-casing it, an intermediary
    // normalising its case — would decode to the same bytes and be read as the same resume
    // point, although no page of this service ever issued that value. Re-encoding the
    // bytes gives the one spelling the encoder would have written for them, so a token
    // that is not identical to it names no resume point.
    let bytes = hex::decode(token).ok()?;
    (hex::encode(&bytes) == token).then_some(())?;
    let decoded = String::from_utf8(bytes).ok()?;
    // The region is stripped as ONE literal prefix rather than split off, so a region
    // string carrying the separator itself is still matched whole instead of being read
    // as the first component of another region's key.
    let scoped = decoded.strip_prefix(marker)?;
    let resume_after =
        scoped.strip_prefix(&format!("{region}{PAGINATION_TOKEN_SEPARATOR}"))?;
    // `splitn` so only the components BEFORE the last one are delimited: the last
    // component keeps any separator it contains, rather than inflating the arity and
    // making this refuse a token the service itself issued.
    let key: Vec<String> = resume_after
        .splitn(components, PAGINATION_TOKEN_SEPARATOR)
        .map(str::to_string)
        .collect();
    // The first component identifies the entry to resume after, so a token carrying
    // none — or a key of the wrong arity for this operation — names no resume point.
    (key.len() == components && !key[0].is_empty()).then_some(key)
}

/// The page a `MaxResultsInteger`-paged operation of this service returns when the
/// request names no `MaxResults`.
///
/// From the model: every one of those members documents "The maximum number of results to
/// return in one operation. The default is 50. The maximum that you can specify is 100."
const DEFAULT_PAGE_SIZE: usize = 50;

/// The largest page one of those operations returns: `MaxResultsInteger`'s modelled
/// maximum (`@range(min: 1, max: 100)`).
const MAX_PAGE_SIZE: usize = 100;

/// How many entries a page of a `MaxResultsInteger`-paged operation holds, for a request
/// that asked for `requested` of them (`None` when it named no `MaxResults`).
///
/// ONE page-size rule for every such operation of this service (`ListAppMonitors`,
/// `ListRumMetricsDestinations`, `BatchGetRumMetricDefinitions`), beside the token codec
/// those same operations share, so their page sizes cannot drift. `MaxResultsInteger` is
/// modelled `@range(min: 1, max: 100)`, so the boundary validator has already refused
/// anything outside it; an absent value pages at the documented default
/// ([`DEFAULT_PAGE_SIZE`]), and the result is held inside the modelled range so no
/// arithmetic here can hand a caller an empty or an unbounded page.
///
/// NOT every paginated operation of this service: `GetAppMonitorData` pages on
/// `MaxQueryResults`, a different member with a different range and its own `@default`,
/// and keeps its own rule.
pub fn page_size(requested: Option<i32>) -> usize {
    requested
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRumRumState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRumRumState>)
}
