// SPDX-License-Identifier: CC0-1.0

//! The account's FLEET METRICS: the record one holds, the aggregation it is
//! configured with, and what every reader of one reports.
//!
//! A fleet metric is a periodic AGGREGATION over the things a fleet-index query
//! selects — "Creates a fleet metric" (`CreateFleetMetric`) — so a record is the
//! query, the aggregation applied to it, the field aggregated and the period between
//! emissions. The name is the metric's identity: the model binds it as the
//! `@httpLabel` of `/fleet-metric/{metricName}` on every single-metric operation
//! (`smithy_query(query="operation",operation="CreateFleetMetric")`), which is why
//! the store is a `BTreeMap` keyed by it — the map IS the registry, and
//! `ListFleetMetrics` is `@paginated`, so a page boundary is only meaningful over a
//! stable order.
//!
//! One record carries everything a reader hands back, so `DescribeFleetMetric` after
//! `CreateFleetMetric` reports the created metric member for member: the reader
//! clones what the writer stored and the two cannot drift.
//!
//! A metric aggregates over a fleet INDEX, so it can only exist while that index
//! does and only once the index has finished building — see
//! [`FleetIndexAvailability`], which is what makes the create's
//! `ResourceNotFoundException` / `IndexNotReadyException` the index's own state and
//! not a guess.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AggregationType, DescribeFleetMetricOutput, AGGREGATION_TYPE_NAME_VALUES,
    FLEET_METRIC_UNIT_VALUES,
};

/// How many fleet metrics this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's fleet metrics; the exact figure is
/// not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`super::MAX_POLICIES`] exists — and `CreateFleetMetric`
/// past it refuses with a modelled error rather than growing without bound.
pub const MAX_FLEET_METRICS: usize = 100;

/// How many tags one fleet metric may carry: the same documented IoT tagging limit
/// [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_FLEET_METRIC_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `version` a freshly created fleet metric carries.
///
/// `DescribeFleetMetric$version` is "The version of the fleet metric" and is what a
/// caller passes as `expectedVersion` to `UpdateFleetMetric` / `DeleteFleetMetric`. A
/// record that exists has had one write (its create), so its version is 1 — the same
/// rule [`super::FIRST_THING_GROUP_VERSION`] states for a thing group.
pub const FIRST_FLEET_METRIC_VERSION: i64 = 1;

/// The granularity of `CreateFleetMetric$period`, in seconds.
///
/// The member documents it: "The time in seconds between fleet metric emissions.
/// Range [60(1 min), 86400(1 day)] and must be multiple of 60."
pub const FLEET_METRIC_PERIOD_GRANULARITY: i32 = 60;

/// The ARN of a fleet metric:
/// `arn:aws:iot:<region>:<account>:fleetmetric/<metricName>`.
///
/// `fleetmetric/` is the resource type AWS IoT's fleet-metric ARNs use — it is what
/// `CreateFleetMetric$metricArn` / `DescribeFleetMetric$metricArn` report and what an
/// `iot:DescribeFleetMetric` policy statement names a metric by; the namespace and
/// account segments are the ones [`super::thing_arn`] documents.
pub fn fleet_metric_arn(region: &str, metric_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("fleetmetric/{metric_name}"))
}

/// How this service refuses work over a fleet index whose build has not finished.
///
/// `IndexNotReadyException` is "The index is not ready"; the fixed text states that
/// and the variable parts are the index the request named and the status it is in
/// ([`crate::ops::types::INDEX_STATUS_VALUES`]).
pub fn index_not_ready_message(index_name: &str, index_status: &str) -> String {
    format!("Index {index_name} is not ready: it is in status {index_status}")
}

/// How a request that would aggregate over an index the account does not have is
/// refused.
///
/// The fixed text is the class's — the index is missing, and the call that brings one
/// into existence is `UpdateIndexingConfiguration` — and the variable part is the
/// index the request named, never a name from any one call's recording. It is a
/// LONGER sentence than the one a reader of an index states
/// ([`super::missing_fleet_index_message`]), because the caller of an aggregation is
/// told what to do about it.
pub fn missing_fleet_index_for_aggregation_message(index_name: &str) -> String {
    format!(
        "Index {index_name} does not exist. Please enable index by calling \
         UpdateIndexingConfiguration"
    )
}

/// The statistics a `Statistics` aggregation may ask for.
///
/// The catalog is the modelled `Statistics` structure's own members
/// (`smithy_query(query="shape",shape="Statistics")`: `count`, `average`, `sum`,
/// `minimum`, `maximum`, `sumOfSquares`, `variance`, `stdDeviation`) — the values
/// `GetStatistics` reports — so what a fleet metric may aggregate is what the service
/// can report for it, and `AggregationType$values` naming anything else asks for a
/// statistic no reader has.
pub const AGGREGATION_STATISTICS: &[&str] = &[
    "count",
    "average",
    "sum",
    "minimum",
    "maximum",
    "sumOfSquares",
    "variance",
    "stdDeviation",
];

/// The `AggregationTypeName` of the aggregation that reports statistics over the
/// field, whose `values` name which of [`AGGREGATION_STATISTICS`] it reports.
pub const AGGREGATION_STATISTICS_NAME: &str = "Statistics";

/// The `AggregationTypeName` of the aggregation that reports percentiles of the
/// field, whose `values` name the percentiles it reports.
pub const AGGREGATION_PERCENTILES_NAME: &str = "Percentiles";

/// The `AggregationTypeName` of the aggregation that counts the distinct values of
/// the field, which has nothing to parameterise and so takes no `values`.
pub const AGGREGATION_CARDINALITY_NAME: &str = "Cardinality";

/// The largest percentile a `Percentiles` aggregation may ask for.
///
/// A percentile is a position in the distribution, so it runs up to the whole of it;
/// `AggregationTypeValue` is modelled `pattern ^[a-zA-Z0-9]+$`
/// (`smithy_query(query="shape",shape="AggregationTypeValue")`), which admits no
/// decimal point, so the percentiles a fleet metric can name are whole numbers.
pub const MAX_PERCENTILE: u32 = 100;

/// Why an aggregation a request asked for is not one this service can compute.
///
/// Each variant is a DIFFERENT refusal, so the caller is told which rule the request
/// broke rather than one fixed sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregationRejection {
    /// `AggregationType$name` is no aggregation the service has; carries what was
    /// asked for.
    UnknownName(String),
    /// A `Cardinality` aggregation was given `values`, which it has nothing to apply
    /// them to.
    ValuesNotTaken(String),
    /// A `Statistics` aggregation asked for a statistic that is not one of
    /// [`AGGREGATION_STATISTICS`]; carries what was asked for.
    UnknownStatistic(String),
    /// A `Percentiles` aggregation asked for something that is not a percentile;
    /// carries what was asked for.
    InvalidPercentile(String),
    /// `period` is not a whole number of minutes; carries what was asked for.
    PeriodNotWholeMinutes(i32),
    /// `unit` is no `FleetMetricUnit`; carries what was asked for.
    UnknownUnit(String),
}

impl AggregationRejection {
    /// What to tell the caller: the rule the request broke, built from the request
    /// that broke it.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownName(name) => format!(
                "aggregationType.name {name} is not a supported aggregation type; the supported \
                 types are {}",
                AGGREGATION_TYPE_NAME_VALUES.join(", ")
            ),
            Self::ValuesNotTaken(name) => {
                format!("the {name} aggregation type takes no aggregationType.values")
            }
            Self::UnknownStatistic(value) => format!(
                "aggregationType.values {value} is not a supported statistic; the supported \
                 statistics are {}",
                AGGREGATION_STATISTICS.join(", ")
            ),
            Self::InvalidPercentile(value) => format!(
                "aggregationType.values {value} is not a percentile; a percentile is a whole \
                 number between 1 and {MAX_PERCENTILE}"
            ),
            Self::PeriodNotWholeMinutes(period) => format!(
                "period must be a multiple of {FLEET_METRIC_PERIOD_GRANULARITY}; this one is \
                 {period}"
            ),
            Self::UnknownUnit(unit) => {
                format!("unit {unit} is not a supported fleet metric unit")
            }
        }
    }
}

/// Check that the aggregation `aggregation` over `period` reported in `unit` is one
/// this service can compute, naming the first rule it breaks.
///
/// What makes an aggregation computable is the pair of its NAME and its VALUES: the
/// name selects which aggregation the service runs, and the values parameterise that
/// one aggregation — the statistics a `Statistics` aggregation reports, the
/// percentiles a `Percentiles` one reports, and nothing at all for `Cardinality`,
/// which counts distinct values and has nothing to parameterise.
///
/// Shared by every operation that configures a fleet metric, so `CreateFleetMetric`
/// and `UpdateFleetMetric` cannot drift apart on which aggregations they accept.
pub fn validate_aggregation(
    aggregation: &AggregationType,
    period: i32,
    unit: Option<&str>,
) -> Result<(), AggregationRejection> {
    if period % FLEET_METRIC_PERIOD_GRANULARITY != 0 {
        return Err(AggregationRejection::PeriodNotWholeMinutes(period));
    }
    if let Some(unit) = unit {
        if !FLEET_METRIC_UNIT_VALUES.contains(&unit) {
            return Err(AggregationRejection::UnknownUnit(unit.to_string()));
        }
    }
    let values = aggregation.values.clone().unwrap_or_default();
    match aggregation.name.as_str() {
        AGGREGATION_STATISTICS_NAME => {
            for value in &values {
                if !AGGREGATION_STATISTICS.contains(&value.as_str()) {
                    return Err(AggregationRejection::UnknownStatistic(value.clone()));
                }
            }
            Ok(())
        }
        AGGREGATION_PERCENTILES_NAME => {
            for value in &values {
                let percentile = value.parse::<u32>().ok().filter(|percentile| {
                    (1..=MAX_PERCENTILE).contains(percentile)
                });
                if percentile.is_none() {
                    return Err(AggregationRejection::InvalidPercentile(value.clone()));
                }
            }
            Ok(())
        }
        AGGREGATION_CARDINALITY_NAME => {
            if values.is_empty() {
                Ok(())
            } else {
                Err(AggregationRejection::ValuesNotTaken(
                    aggregation.name.clone(),
                ))
            }
        }
        unknown => Err(AggregationRejection::UnknownName(unknown.to_string())),
    }
}

/// Where a fleet index stands for an operation that needs to aggregate over it.
///
/// A fleet metric reads an index, so the index decides whether the metric can be
/// created at all: an account with no such index has nothing to aggregate
/// (`ResourceNotFoundException`), and one whose index is still building has the index
/// but not yet its contents (`IndexNotReadyException`, "The index is not ready").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetIndexAvailability {
    /// The account has no index under that name — it is no fleet index at all, or the
    /// configuration section that drives it is off.
    Missing,
    /// The index exists and its build has not finished.
    Building,
    /// The index exists and is `ACTIVE`.
    Ready,
}

/// The `FieldType` of a field holding text: one of
/// [`crate::ops::types::FIELD_TYPE_VALUES`].
pub const FIELD_TYPE_STRING: &str = "String";

/// The `FieldType` of a field holding a number.
pub const FIELD_TYPE_NUMBER: &str = "Number";

/// The `FieldType` of a field holding a flag.
pub const FIELD_TYPE_BOOLEAN: &str = "Boolean";

/// The fields of the THING index every account has without declaring them, and the
/// `FieldType` each carries.
///
/// These are fleet indexing's MANAGED fields — "Contains fields that are indexed and
/// whose types are already known by the Fleet Indexing service"
/// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="managedFields")`),
/// which is also why a caller's own `managedFields` list is dropped on an update: the
/// service decides them. They are the registry's own members every thing has, so an
/// aggregation may name one without the account declaring anything; any OTHER field —
/// a thing attribute, a shadow member — is aggregatable only once the account declares
/// it as a custom field of the index.
pub const MANAGED_THING_FIELDS: &[(&str, &str)] = &[
    ("thingId", FIELD_TYPE_STRING),
    ("thingName", FIELD_TYPE_STRING),
    ("thingTypeName", FIELD_TYPE_STRING),
    ("registry.version", FIELD_TYPE_NUMBER),
    ("connectivity.connected", FIELD_TYPE_BOOLEAN),
    ("connectivity.timestamp", FIELD_TYPE_NUMBER),
    ("connectivity.disconnectReason", FIELD_TYPE_STRING),
];

/// The fields of the THING GROUP index every account has without declaring them, and
/// the `FieldType` each carries — the group index's half of
/// [`MANAGED_THING_FIELDS`].
pub const MANAGED_THING_GROUP_FIELDS: &[(&str, &str)] = &[
    ("thingGroupId", FIELD_TYPE_STRING),
    ("thingGroupName", FIELD_TYPE_STRING),
    ("parentGroupName", FIELD_TYPE_STRING),
];

/// The one statistic a `Statistics` aggregation can report over a field of ANY type.
///
/// `Statistics$count` is "The count of things that match the query string criteria and
/// contain a valid aggregation field value" — counting the things that have the field
/// needs no arithmetic on its values, while every other member of the modelled
/// `Statistics` structure is an arithmetic summary (`average`, `sum`, `minimum`,
/// `maximum`, `sumOfSquares`, `variance`, `stdDeviation`) and so needs a numeric field.
pub const COUNT_STATISTIC: &str = "count";

/// Why the field a request asked to aggregate cannot be aggregated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregationFieldRejection {
    /// The index carries no such field: it is neither one of the index's managed
    /// fields nor a custom field the account declared. Carries the field and the index.
    NotIndexed {
        /// The field the request named.
        aggregation_field: String,
        /// The index the request aggregates over.
        index_name: String,
    },
    /// The field is indexed, and the aggregation asked for some VALUES its type cannot
    /// be summarised by — the arithmetic statistics of a field that holds no numbers.
    UnsupportedValues {
        /// The aggregation the request asked for.
        aggregation_name: String,
        /// The `FieldType` the index carries the field as.
        field_type: String,
        /// The values asked for that a field of that type cannot be summarised by.
        invalid_values: Vec<String>,
        /// The values a field of that type CAN be summarised by.
        supported_values: Vec<String>,
    },
    /// The field is indexed, and the aggregation itself cannot be computed over a
    /// field of its type at all.
    UnsupportedFieldType {
        /// The aggregation the request asked for.
        aggregation_name: String,
        /// The `FieldType` the index carries the field as.
        field_type: String,
    },
}

impl AggregationFieldRejection {
    /// What to tell the caller: the rule the request broke, built from the request
    /// that broke it and the account's own indexing configuration.
    ///
    /// The values refusal states which of the asked-for values the field's type cannot
    /// be summarised by and which it can, both as the service lists them.
    pub fn message(&self) -> String {
        match self {
            Self::NotIndexed {
                aggregation_field,
                index_name,
            } => format!(
                "Aggregation field {aggregation_field} is not indexed in index {index_name}; add \
                 it as a custom field by calling UpdateIndexingConfiguration"
            ),
            Self::UnsupportedValues {
                aggregation_name,
                field_type,
                invalid_values,
                supported_values,
            } => format!(
                "Invalid values [{}] for {aggregation_name} aggregation on {field_type} field. \
                 Supported values are [{}]",
                invalid_values.join(", "),
                supported_values.join(", ")
            ),
            Self::UnsupportedFieldType {
                aggregation_name,
                field_type,
            } => format!(
                "{aggregation_name} aggregation is not supported on {field_type} field"
            ),
        }
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// The `FieldType` the index `index_name` carries the field `aggregation_field`
    /// as, or `None` for a field the index does not carry.
    ///
    /// A field is carried either because the service manages it
    /// ([`MANAGED_THING_FIELDS`]) or because the account declared it as a custom field
    /// of that index's configuration section — "Contains custom field names and their
    /// data type" — so this is read from the stored configuration and recorded nowhere
    /// beside it.
    pub fn indexed_field_type(&self, index_name: &str, aggregation_field: &str) -> Option<String> {
        let (managed, custom) = match index_name {
            super::THING_INDEX_NAME => (
                MANAGED_THING_FIELDS,
                self.indexing_configuration.thing.custom_fields.as_ref(),
            ),
            super::THING_GROUP_INDEX_NAME => (
                MANAGED_THING_GROUP_FIELDS,
                self.indexing_configuration
                    .thing_group
                    .custom_fields
                    .as_ref(),
            ),
            _ => return None,
        };
        if let Some((_name, field_type)) = managed
            .iter()
            .find(|(name, _field_type)| *name == aggregation_field)
        {
            return Some((*field_type).to_string());
        }
        custom?
            .iter()
            .find(|field| field.name.as_deref() == Some(aggregation_field))
            .and_then(|field| field.r#type.clone())
    }

    /// Check that `aggregation` can be computed over `aggregation_field` of the index
    /// `index_name`, naming the first rule the request breaks.
    ///
    /// An aggregation reads the VALUES of one field, so the field has to be one the
    /// index holds, and the aggregation has to be one its type admits: the arithmetic
    /// statistics and every percentile are summaries OF NUMBERS, so over a field that
    /// holds no numbers the only statistic there is is the count of the things that
    /// carry it ([`COUNT_STATISTIC`]), and the distinct values of a field
    /// (`Cardinality`) can be counted whatever values it holds.
    pub fn validate_aggregation_field(
        &self,
        index_name: &str,
        aggregation_field: &str,
        aggregation: &AggregationType,
    ) -> Result<(), AggregationFieldRejection> {
        let Some(field_type) = self.indexed_field_type(index_name, aggregation_field) else {
            return Err(AggregationFieldRejection::NotIndexed {
                aggregation_field: aggregation_field.to_string(),
                index_name: index_name.to_string(),
            });
        };
        if field_type == FIELD_TYPE_NUMBER {
            // Every aggregation there is summarises numbers, so a numeric field admits
            // whichever one the request asked for.
            return Ok(());
        }
        // A `Statistics` aggregation over a field that holds no numbers can report the
        // COUNT and nothing else, so the refusal lists the values it asked for that are
        // not that one. A request naming no values asks for every statistic there is,
        // which is why those are what such a request is told are invalid.
        if aggregation.name == AGGREGATION_STATISTICS_NAME {
            let supported_values = vec![COUNT_STATISTIC.to_string()];
            let asked_for = aggregation.values.clone().unwrap_or_else(|| {
                AGGREGATION_STATISTICS
                    .iter()
                    .map(|statistic| (*statistic).to_string())
                    .collect()
            });
            let invalid_values: Vec<String> = asked_for
                .into_iter()
                .filter(|value| !supported_values.contains(value))
                .collect();
            if invalid_values.is_empty() {
                return Ok(());
            }
            return Err(AggregationFieldRejection::UnsupportedValues {
                aggregation_name: aggregation.name.clone(),
                field_type,
                invalid_values,
                supported_values,
            });
        }
        // A percentile is a position in a distribution of numbers, so there is no
        // percentile of a field that holds none.
        if aggregation.name == AGGREGATION_PERCENTILES_NAME {
            return Err(AggregationFieldRejection::UnsupportedFieldType {
                aggregation_name: aggregation.name.clone(),
                field_type,
            });
        }
        // Counting the DISTINCT values of a field is over the values themselves, so a
        // flag — which is not a value a cardinality is asked about — is refused while
        // text is not.
        if aggregation.name == AGGREGATION_CARDINALITY_NAME && field_type == FIELD_TYPE_BOOLEAN {
            return Err(AggregationFieldRejection::UnsupportedFieldType {
                aggregation_name: aggregation.name.clone(),
                field_type,
            });
        }
        Ok(())
    }
}

/// Everything about a fleet metric that is NOT its name: what it aggregates, over
/// which query and index, how often, and in which unit.
///
/// It is a value of its own so a reader hands back exactly what the writer stored,
/// and so an update replaces one value rather than a scatter of fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetMetricConfiguration {
    /// `CreateFleetMetric$queryString` — the search query whose things are
    /// aggregated, verbatim as the writer supplied it.
    pub query_string: String,
    /// `AggregationType$name` — which aggregation is computed.
    pub aggregation_name: String,
    /// `AggregationType$values` — what parameterises that aggregation.
    pub aggregation_values: Vec<String>,
    /// `CreateFleetMetric$period` — "The time in seconds between fleet metric
    /// emissions".
    pub period: i32,
    /// `CreateFleetMetric$aggregationField` — "The field to aggregate".
    pub aggregation_field: String,
    /// `CreateFleetMetric$description`, when the request carried one.
    pub description: Option<String>,
    /// `CreateFleetMetric$queryVersion`, when the request carried one.
    pub query_version: Option<String>,
    /// The index the metric aggregates over — the one the request named, or
    /// [`super::THING_INDEX_NAME`] for a request that named none.
    pub index_name: String,
    /// `CreateFleetMetric$unit`, when the request carried one: "Used to support unit
    /// transformation such as milliseconds to seconds ... Default to null".
    pub unit: Option<String>,
}

/// One fleet metric in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredFleetMetric {
    /// The name the metric was created under; also its key in the registry.
    pub metric_name: String,
    /// `arn:aws:iot:<region>:<account>:fleetmetric/<metricName>`, minted on create.
    pub metric_arn: String,
    /// What the metric is configured with; see [`FleetMetricConfiguration`].
    pub configuration: FleetMetricConfiguration,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags are
    /// "metadata, which can be used to manage the fleet metric" rather than part of
    /// what it computes, so they are not part of its configuration.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the metric was created — `DescribeFleetMetric$creationDate`.
    pub creation_date: SystemTime,
    /// When the metric last changed — `DescribeFleetMetric$lastModifiedDate`. Equal
    /// to [`RegisteredFleetMetric::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// The registry version of this record, which `expectedVersion` is checked
    /// against.
    pub version: i64,
}

impl RegisteredFleetMetric {
    /// A newly created fleet metric: the minted identity plus the requested
    /// configuration and tags, at [`FIRST_FLEET_METRIC_VERSION`].
    pub fn new(
        metric_name: String,
        metric_arn: String,
        configuration: FleetMetricConfiguration,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            metric_name,
            metric_arn,
            configuration,
            tags,
            creation_date: created_at,
            last_modified_date: created_at,
            version: FIRST_FLEET_METRIC_VERSION,
        }
    }

    /// The record as `DescribeFleetMetric` reports it — every member of the stored
    /// record, so the read and the create cannot drift.
    pub fn to_description(&self) -> DescribeFleetMetricOutput {
        DescribeFleetMetricOutput {
            metric_name: Some(self.metric_name.clone()),
            query_string: Some(self.configuration.query_string.clone()),
            aggregation_type: Some(AggregationType {
                name: self.configuration.aggregation_name.clone(),
                values: Some(self.configuration.aggregation_values.clone()),
            }),
            period: Some(self.configuration.period),
            aggregation_field: Some(self.configuration.aggregation_field.clone()),
            description: self.configuration.description.clone(),
            query_version: self.configuration.query_version.clone(),
            index_name: Some(self.configuration.index_name.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            unit: self.configuration.unit.clone(),
            version: self.version,
            metric_arn: Some(self.metric_arn.clone()),
        }
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Where the fleet index `index_name` stands at `now` for an operation that has
    /// to aggregate over it.
    ///
    /// A pure read of the stored configuration, the recorded build and the clock:
    /// nothing is written, so asking leaves every later caller seeing what this one
    /// saw.
    pub fn fleet_index_availability(
        &self,
        index_name: &str,
        now: SystemTime,
    ) -> FleetIndexAvailability {
        if !self.fleet_index_enabled(index_name) {
            return FleetIndexAvailability::Missing;
        }
        if self.fleet_index_status(index_name, now) == super::INDEX_STATUS_ACTIVE {
            FleetIndexAvailability::Ready
        } else {
            FleetIndexAvailability::Building
        }
    }
}
