// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metrics a workgroup publishes about the queries it RAN.
//!
//! # The rule this module implements
//!
//! A workgroup's configuration carries one switch that says whether Amazon CloudWatch metrics
//! are enabled for it (`WorkGroupConfiguration$PublishCloudWatchMetricsEnabled`), and a
//! workgroup it is ON for publishes the query metric family for every query that RAN in it:
//! one datapoint per series per query, as soon as that query has reached a terminal state,
//! into the [`METRIC_NAMESPACE`] namespace. A workgroup the
//! switch is OFF for — including one whose configuration never set it, which is the reading
//! that member's own documentation gives it ([`super::WORK_GROUP_SWITCH_UNSET`]) — publishes
//! nothing at all, so an account whose workgroups all have metrics disabled holds no series in
//! that namespace.
//!
//! The switch decides the queries RUN while it is on, so each query carries the decision taken
//! when it was submitted ([`QueryExecutionRecord::publishes_metrics`]): turning the metrics on
//! does not meter the queries that ran while they were off, and turning them off does not
//! withhold the metrics of a query that was already running.
//!
//! Each datapoint goes into one series per DIMENSION SET the query is reported under
//! ([`QueryMetrics::dimension_sets`]): the workgroup alone ([`WORK_GROUP_DIMENSION`], the name
//! the workgroup was created under), and the finer set that names the state the query settled
//! in ([`QUERY_STATE_DIMENSION`]) and the kind of statement it carried
//! ([`QUERY_TYPE_DIMENSION`]) beside it — so the same measurement is readable both for a
//! workgroup's queries as a whole and for the failures, or the definition statements, among
//! them.
//!
//! The series are the measures the query's own statistics report, each under the
//! `StandardUnit` its quantity is expressed in ([`QUERY_METRICS`]): the bytes the run
//! processed, and the phases the execution time partitions into. So a client that sizes or
//! bills queries off the namespace, and an alarm over one of those series, reads the same
//! numbers `GetQueryExecution` reports for that execution — there is no second measurement
//! here, only the projection of the stored one onto the series it belongs to.
//!
//! A datapoint is stamped at the instant the query COMPLETED, not at the instant the
//! publication happened to be made, so the period a query is counted in is the period it
//! completed in and a read of the execution and a read of the series report one timeline.
//!
//! # How a completion becomes a datapoint
//!
//! Nothing here decides WHEN a query completes: the instants its own record holds do
//! ([`QueryExecutionRecord::state_at`]), and the metrics of a query are owed from the instant
//! it became terminal — whether or not anybody looked. This module only compares the queries
//! that have become terminal against those whose metrics have already been published, so every
//! query publishes its family exactly once, whichever call or wake-up observed the completion;
//! a query that is still queued or running publishes nothing, and so does one already
//! published.
//!
//! WHO publishes: the same two things that announce a query's state changes
//! ([`super::query_events`]) — the call that reports a query's state, once it has released
//! this service's store, and the in-process clock pump that waits for the instants a query's
//! own record holds. The pump is what makes the series appear for a client that polls nothing
//! after submitting.
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own them
//! ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It goes
//! into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read, through
//! CloudWatch's own bounded append, so one metric and its reader cannot disagree — and an
//! alarm watching the series is made due by the append, exactly as an ingested datapoint makes
//! it due.
//!
//! Publication is best effort: an emulator with no CloudWatch mounted has nowhere to publish,
//! and a store at capacity keeps what it holds. A query reports its own outcome either way,
//! because a metric is published off the answer and so can never be what withholds one; the
//! queries of an append that did not go through are given back
//! ([`ComAmazonawsAthenaAmazonathenaState::forget_published_query_metrics`]) so the next call
//! or wake-up publishes them rather than losing them.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::*;

/// The CloudWatch store a query's own metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what a query publishes is what `ListMetrics` and
/// `GetMetricStatistics` report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The namespace Athena publishes its query metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/Athena";

/// The dimension every one of them is reported per: the workgroup whose query the work was,
/// named as the create that registered it named it.
pub const WORK_GROUP_DIMENSION: &str = "WorkGroup";

/// The dimension naming the state a query SETTLED in, which the finer dimension set reports
/// beside the workgroup: a `QueryExecutionState` value
/// (`smithy_query(query="shape",shape="QueryExecutionState")`), and the terminal one this
/// query reached.
pub const QUERY_STATE_DIMENSION: &str = "QueryState";

/// The dimension naming the KIND of statement the query carried, the other member of that
/// finer set: a `StatementType` value
/// (`smithy_query(query="shape",shape="StatementType")`: DDL, DML, UTILITY).
pub const QUERY_TYPE_DIMENSION: &str = "QueryType";

/// The `StandardUnit` a quantity of data carries: `Bytes`, the member for a size.
pub const BYTES_UNIT: &str = "Bytes";

/// The `StandardUnit` a span of time carries here: `Milliseconds`, the member for the
/// resolution every one of Athena's own spans is measured in
/// (`QueryExecutionStatistics$TotalExecutionTimeInMillis` and its siblings).
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The metric reporting the bytes one query processed.
pub const PROCESSED_BYTES_METRIC: &str = "ProcessedBytes";

/// The metric reporting what the engine spent running one query.
pub const ENGINE_EXECUTION_TIME_METRIC: &str = "EngineExecutionTime";

/// The metric reporting what Athena spent planning one query.
pub const QUERY_PLANNING_TIME_METRIC: &str = "QueryPlanningTime";

/// The metric reporting what one query spent in the queue waiting for resources.
pub const QUERY_QUEUE_TIME_METRIC: &str = "QueryQueueTime";

/// The metric reporting what Athena spent finalising and publishing one query's results.
pub const SERVICE_PROCESSING_TIME_METRIC: &str = "ServiceProcessingTime";

/// The metric reporting what one query took in total.
pub const TOTAL_EXECUTION_TIME_METRIC: &str = "TotalExecutionTime";

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How many observations ONE datapoint of a query metric stands for: the one query it is
/// about.
const ONE_QUERY: f64 = 1.0;

/// Milliseconds in one second, for the epoch seconds a stored datapoint is stamped in.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// How many queries ONE call records and publishes the metrics of.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: it bounds the work one request can set off,
/// exactly as the transition ceiling of [`super::query_events`] does. A query past it stays
/// unpublished and is published by the next call or wake-up, so nothing is lost.
const MAX_RECORDED_QUERIES: usize = 64;

/// ONE series of the family a terminal query publishes: its metric name, the measure of the
/// query's own statistics it reports, and the `StandardUnit` that measure is expressed in.
///
/// The measure is read off the `QueryExecutionStatistics` the execution itself reports, so a
/// series carries the number `GetQueryExecution` carries for the same query rather than a
/// second measurement taken here.
pub struct QueryMetric {
    /// The metric name of the series.
    pub metric_name: &'static str,
    /// The `StandardUnit` the value is expressed in.
    pub unit: &'static str,
    /// The measure of the query's statistics this series reports.
    measure: fn(&QueryExecutionStatistics) -> Option<i64>,
}

/// The whole family a terminal query publishes: the data it processed, and the phases its
/// execution time partitions into.
///
/// The measures are the members of `QueryExecutionStatistics`
/// (`smithy_query(query="shape",shape="QueryExecutionStatistics")`), each named by the series
/// Athena reports it as.
pub const QUERY_METRICS: &[QueryMetric] = &[
    QueryMetric {
        metric_name: PROCESSED_BYTES_METRIC,
        unit: BYTES_UNIT,
        // "The number of bytes in the data that was queried."
        measure: |statistics| statistics.data_scanned_in_bytes,
    },
    QueryMetric {
        metric_name: ENGINE_EXECUTION_TIME_METRIC,
        unit: MILLISECONDS_UNIT,
        measure: |statistics| statistics.engine_execution_time_in_millis,
    },
    QueryMetric {
        metric_name: QUERY_PLANNING_TIME_METRIC,
        unit: MILLISECONDS_UNIT,
        measure: |statistics| statistics.query_planning_time_in_millis,
    },
    QueryMetric {
        metric_name: QUERY_QUEUE_TIME_METRIC,
        unit: MILLISECONDS_UNIT,
        measure: |statistics| statistics.query_queue_time_in_millis,
    },
    QueryMetric {
        metric_name: SERVICE_PROCESSING_TIME_METRIC,
        unit: MILLISECONDS_UNIT,
        measure: |statistics| statistics.service_processing_time_in_millis,
    },
    QueryMetric {
        metric_name: TOTAL_EXECUTION_TIME_METRIC,
        unit: MILLISECONDS_UNIT,
        measure: |statistics| statistics.total_execution_time_in_millis,
    },
];

/// ONE datapoint a terminal query publishes: the series it belongs to, what it measured and
/// the unit of that measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// The metric name of the series this datapoint goes into.
    pub metric_name: &'static str,
    /// What the datapoint measured.
    pub value: f64,
    /// The `StandardUnit` the value is expressed in.
    pub unit: &'static str,
}

/// The metrics of ONE query that has reached a terminal state, waiting to be published.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryMetrics {
    /// The execution the datapoints are about, so a publication that failed can be given
    /// back against the record it was recorded on.
    pub query_execution_id: String,
    /// The workgroup the query ran in, which every dimension set its series are reported
    /// under names.
    pub work_group: String,
    /// The `QueryExecutionState` the query SETTLED in, which the finer dimension set reports.
    pub query_state: String,
    /// The `StatementType` the submitted SQL parses as, the other member of that finer set,
    /// and `None` for a submission that parses as no statement at all — there is then no kind
    /// to name.
    pub query_type: Option<String>,
    /// WHEN the query completed — the instant its own record puts the completion at, which is
    /// what every datapoint here is stamped with.
    pub at: SystemTime,
    /// One datapoint per series of the family.
    pub observations: Vec<Observation>,
}

impl QueryMetrics {
    /// The dimension SETS this query's datapoints are published under.
    ///
    /// Each set is a series of its own — "a metric is uniquely defined by a name, a namespace,
    /// and zero or more dimensions" — so the family is published both under the workgroup
    /// alone, which is what a client watching a workgroup's queries reads, and under the
    /// finer set that names the state the query settled in and the kind of statement it
    /// carried beside it, which is what a client watching only the failures, or only the
    /// definition statements, of that workgroup reads. A query that parses as no statement at
    /// all has no kind to name, so only the workgroup's own set is published for it rather
    /// than one carrying a guess.
    #[must_use]
    pub fn dimension_sets(&self) -> Vec<cloudwatch::DimensionSet> {
        let work_group = (
            WORK_GROUP_DIMENSION.to_string(),
            self.work_group.clone(),
        );
        let mut sets = vec![[work_group.clone()].into_iter().collect()];
        if let Some(query_type) = &self.query_type {
            sets.push(
                [
                    (
                        QUERY_STATE_DIMENSION.to_string(),
                        self.query_state.clone(),
                    ),
                    (QUERY_TYPE_DIMENSION.to_string(), query_type.clone()),
                    work_group,
                ]
                .into_iter()
                .collect(),
            );
        }
        sets
    }
}

impl QueryExecutionRecord {
    /// The metrics this query publishes at `now`, or `None` while it has not completed.
    ///
    /// Read off the same statistics a `GetQueryExecution` of this instant reports
    /// ([`QueryExecutionRecord::statistics_at`]): a member the statistics do not carry names
    /// no observation, so nothing is published for a measure the execution does not report
    /// rather than a zero standing in for it. The state and the statement kind the finer
    /// dimension set names come off that same record, so a series and a read of the execution
    /// cannot disagree about either.
    fn query_metrics(&self, now: SystemTime) -> Option<QueryMetrics> {
        let at = self.completion_at(now)?;
        let statistics = self.statistics_at(now);
        Some(QueryMetrics {
            query_execution_id: self.id.clone(),
            work_group: self.work_group.clone(),
            query_state: self.state_at(now).to_string(),
            query_type: self.statement_type.clone(),
            at,
            observations: QUERY_METRICS
                .iter()
                .filter_map(|metric| {
                    Some(Observation {
                        metric_name: metric.metric_name,
                        value: (metric.measure)(&statistics)? as f64,
                        unit: metric.unit,
                    })
                })
                .collect(),
        })
    }

    /// The instant this query's metrics fall due at, or `None` when nothing is owed at an
    /// instant; see [`ComAmazonawsAthenaAmazonathenaState::next_query_metric_due_at`].
    fn metric_due_at(&self) -> Option<SystemTime> {
        if let Some(cancelled_at) = self.cancelled_at {
            return Some(cancelled_at);
        }
        self.outcome.as_ref().map(|_| self.completion_due_at)
    }
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// Whether the workgroup `name` publishes the metrics of a query submitted to it NOW.
    ///
    /// Answered from the configuration that workgroup HOLDS, through the one reading of its
    /// switches every read of the workgroup reports
    /// ([`super::effective_work_group_configuration`]), so what the metric path acts on is
    /// exactly what `GetWorkGroup` reports for the same workgroup. A workgroup this account
    /// does not hold publishes nothing — there is no configuration to read — and the
    /// account's default workgroup, which no create wrote, is read off the record built for
    /// it ([`super::default_work_group_record`]).
    ///
    /// Asked by the SUBMISSION, which stores the answer on the query it mints
    /// ([`QueryExecutionRecord::publishes_metrics`]): the switch decides the queries run while
    /// it is on, so a query carries the decision taken when it was submitted rather than one
    /// re-read when its metrics come to be published.
    pub fn work_group_publishes_metrics(&self, name: &str) -> bool {
        let stored = match self.work_groups.get(name) {
            Some(record) => record.configuration.clone(),
            None if name == DEFAULT_WORK_GROUP => {
                default_work_group_record(self.opened_at).configuration
            }
            None => return false,
        };
        effective_work_group_configuration(&stored, self.system_tables.enabled())
            .publish_cloud_watch_metrics_enabled
            .unwrap_or(WORK_GROUP_SWITCH_UNSET)
    }

    /// The metrics of every query that is METERED, has completed and has not been published
    /// yet, recorded as published, oldest completion first.
    ///
    /// Whether a query is metered was decided when it was SUBMITTED, by the switch its
    /// workgroup held then ([`Self::work_group_publishes_metrics`]), and is stored on the
    /// query: so an update of that switch neither meters a query that ran while the metrics
    /// were off nor withholds those of one that ran while they were on.
    ///
    /// Called with the store held, and answers the metrics to publish once it is released. At
    /// most [`MAX_RECORDED_QUERIES`] queries are taken; the rest stay unpublished, so the
    /// next call or wake-up records them.
    pub fn record_query_metrics(&mut self, now: SystemTime) -> Vec<QueryMetrics> {
        let mut due: Vec<QueryMetrics> = self
            .query_executions
            .values()
            .filter(|record| record.publishes_metrics && !record.metrics_published)
            .filter_map(|record| record.query_metrics(now))
            .collect();
        // The execution map walks in no particular order, so the publications are ordered by
        // WHEN the queries they report completed BEFORE the ceiling is applied: a batch over
        // the ceiling then takes the oldest completions and leaves the rest owed, rather than
        // an arbitrary subset of them.
        due.sort_by_key(|metrics| metrics.at);
        due.truncate(MAX_RECORDED_QUERIES);
        for metrics in &due {
            if let Some(record) = self.query_executions.get_mut(&metrics.query_execution_id) {
                record.metrics_published = true;
            }
        }
        due
    }

    /// Give back the record of publication for `published`: they were recorded as published
    /// and the append did not go through, so the next call or wake-up publishes them again
    /// rather than losing them.
    pub fn forget_published_query_metrics(&mut self, published: &[QueryMetrics]) {
        for metrics in published {
            if let Some(record) = self.query_executions.get_mut(&metrics.query_execution_id) {
                record.metrics_published = false;
            }
        }
    }

    /// The earliest instant at which this store owes a query's own metrics, if any — what the
    /// clock pump waits for beside the state changes it publishes.
    ///
    /// A query whose metrics are unpublished owes them from the instant it completes, which
    /// is the instant its own record holds; one whose run has not reported yet owes nothing
    /// at an instant, because what moves it is that report rather than the clock.
    pub(crate) fn next_query_metric_due_at(&self) -> Option<SystemTime> {
        self.query_executions
            .values()
            .filter(|record| record.publishes_metrics && !record.metrics_published)
            .filter_map(QueryExecutionRecord::metric_due_at)
            .min()
    }
}

/// The CloudWatch store in the SAME shared engine. `None` means the engine carries no such
/// store — a deployment with no CloudWatch to publish to.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// Record and publish the metrics this store owes at this instant, with no Athena lock held
/// across the append, and answer whether everything it recorded went through.
///
/// `false` says an append FAILED — the queries it recorded were given back and are still owed
/// — which is what tells the clock pump to slow down rather than to go on asking at the rate a
/// live completion is published at. An emulator with no CloudWatch mounted records nothing at
/// all, so a store mounted later still publishes the queries that completed before it.
pub fn publish_due_query_metrics(
    state: &::engine::ServiceState<ComAmazonawsAthenaAmazonathenaState>,
    cloudwatch: Option<&::engine::ServiceState<CloudWatchState>>,
) -> bool {
    let Some(store) = cloudwatch else {
        return true;
    };
    // The guard is taken and dropped inside this statement.
    let published = state.lock().record_query_metrics(SystemTime::now());
    if published.is_empty() {
        return true;
    }
    let mut unpublished: Vec<QueryMetrics> = Vec::new();
    for metrics in published {
        if !append(store, &metrics) {
            unpublished.push(metrics);
        }
    }
    if unpublished.is_empty() {
        return true;
    }
    state.lock().forget_published_query_metrics(&unpublished);
    false
}

/// Store every datapoint of `metrics` into each series it belongs to, and make every alarm
/// watching one of those series due for evaluation — exactly as an ingested `PutMetricData`
/// datapoint does, so an alarm cannot tell Athena's own metric from a client's.
///
/// One observation goes into one series per dimension set the query is reported under
/// ([`QueryMetrics::dimension_sets`]), which is what makes the same measurement readable both
/// per workgroup and per state and statement kind within it.
///
/// `false` when the CloudWatch store is at capacity and could take none of them.
fn append(store: &::engine::ServiceState<CloudWatchState>, metrics: &QueryMetrics) -> bool {
    let at_secs = epoch_secs(metrics.at);
    let dimension_sets = metrics.dimension_sets();
    let mut watched: Vec<cloudwatch::WrittenSeries> = Vec::new();
    let mut store = store.lock();
    for observation in &metrics.observations {
        for dimensions in &dimension_sets {
            let key = cloudwatch::MetricKey {
                namespace: METRIC_NAMESPACE.to_string(),
                metric_name: observation.metric_name.to_string(),
                dimensions: dimensions.clone(),
            };
            let datum = cloudwatch::StoredDatum {
                timestamp_secs: at_secs,
                sample_count: ONE_QUERY,
                sum: observation.value,
                minimum: observation.value,
                maximum: observation.value,
                unit: Some(observation.unit.to_string()),
                storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
            };
            if store.append_datapoints(key.clone(), vec![datum]).is_err() {
                ::tracing::warn!(
                    work_group = %metrics.work_group,
                    metric = observation.metric_name,
                    "the CloudWatch store is at capacity; a query's own metric could not be \
                     published"
                );
                return false;
            }
            watched.push(cloudwatch::WrittenSeries {
                key,
                units: vec![observation.unit.to_string()],
            });
        }
    }
    store.make_alarms_due(&watched, at_secs);
    true
}

/// `at` as the epoch seconds a stored datapoint is stamped in.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
        / MILLIS_PER_SECOND
}
