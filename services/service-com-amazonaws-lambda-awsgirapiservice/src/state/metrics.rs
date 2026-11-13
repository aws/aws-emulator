// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch metrics Lambda publishes ABOUT ITS OWN work on a function.
//!
//! A batch an event source mapping reads and hands to a function is moved with no client
//! action at all ([`super::event_source_poller`]), so the only way a client can tell "the
//! poller read an aged backlog" from "nothing was read" is the series Lambda publishes
//! about itself. It goes to the [`METRIC_NAMESPACE`] namespace, dimensioned on the
//! function the batch was handed to ([`FUNCTION_NAME_DIMENSION`]).
//!
//! A mapping's own work is reported per MAPPING rather than per function: how many events
//! its poller took out of the source, how many its filter criteria rejected, how many
//! reached the function and how many it deleted from the source are quantities of that one
//! mapping ([`EVENT_SOURCE_MAPPING_UUID_DIMENSION`]), and a client asks for them with the
//! mapping's own `MetricsConfig` ([`publishes_event_counts`]). Not every value that member
//! takes is selectable on every mapping: two of them are documented for the Kafka event
//! sources alone, so what a given source can be asked to produce is read here
//! ([`supported_metrics`]) and refused by the operations that take a `MetricsConfig`.
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read,
//! through CloudWatch's own bounded append, so one metric and its reader cannot disagree.
//!
//! One observation is published per BATCH READ, carrying what that read measured, at the
//! regular storage resolution — so a reader laying one-minute periods over the series sees
//! the batches read in each minute, and `Maximum` over a minute is the oldest data any
//! batch of that minute carried.
//!
//! What a series is identified by and what one observation of it is are this module's
//! ([`series`], [`observation`], [`append`]), so every metric this service publishes —
//! the per-function ones here, the invocations Lambda admits to a function
//! ([`publish_invocation`]), what each of those invocations came to once it had run
//! ([`InvocationMetrics`]), the requests Lambda throttled ([`publish_throttle`]), the
//! invocations Lambda's recursive-loop guardrail drops
//! ([`super::recursion`]) and the account-level concurrency metering in
//! [`super::concurrency_metering`] — reaches the store the same way and carries the same
//! resolution. An account-level series carries NO dimension: "a metric is uniquely defined
//! by a name, a namespace, and zero or more dimensions", and the quantity such a series
//! measures belongs to the Region rather than to one function.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The CloudWatch store Lambda's own metrics live in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what Lambda publishes is what `ListMetrics` and
/// `GetMetricStatistics` report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine. `None` means the engine carries no
/// such store — a deployment with no CloudWatch to publish to.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The namespace Lambda publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/Lambda";

/// The dimension they are reported per: the function the work was done for.
pub const FUNCTION_NAME_DIMENSION: &str = "FunctionName";

/// The dimension that names the RESOURCE an invocation addressed, beside the function it
/// belongs to: the function itself for an unqualified invocation, and the alias or version
/// the caller named for a qualified one ([`invoked_resource`]).
///
/// It is what tells two targets of one function apart in the same series — an invocation of
/// an alias and one of the function at large are both invocations of that function, so they
/// share the per-function series and differ in this one.
pub const RESOURCE_DIMENSION: &str = "Resource";

/// The dimension a series about ONE EVENT SOURCE MAPPING's own work is reported per: that
/// mapping's `UUID`, which is "The identifier of the event source mapping". The work such a
/// series measures belongs to the mapping rather than to the function it invokes — several
/// mappings can invoke one function — so the mapping is what identifies it.
pub const EVENT_SOURCE_MAPPING_UUID_DIMENSION: &str = "EventSourceMappingUUID";

/// The `EventSourceMappingMetric` a mapping's `MetricsConfig` turns the event-count family
/// on with: "`EventCount` to receive metrics related to the number of events processed by
/// your event source mapping" (`EventSourceMappingMetricsConfig$Metrics`).
pub const EVENT_COUNT_METRICS_SELECTOR: &str = "EventCount";

/// The `EventSourceMappingMetric` that same member documents as "`ErrorCount` (Amazon MSK
/// and self-managed Apache Kafka) to receive metrics related to the number of errors in your
/// event source mapping processing".
pub const ERROR_COUNT_METRICS_SELECTOR: &str = "ErrorCount";

/// The `EventSourceMappingMetric` that same member documents as "`KafkaMetrics` (Amazon MSK
/// and self-managed Apache Kafka) to receive metrics related to the Kafka consumers from your
/// event source mapping".
pub const KAFKA_METRICS_SELECTOR: &str = "KafkaMetrics";

/// The `EventSourceMappingMetric` values an event source can be asked for, in the order the
/// model's enum declares them (`EventCount`, `ErrorCount`, `KafkaMetrics`).
///
/// `EventSourceMappingMetricsConfig$Metrics` scopes two of the three to the KAFKA event
/// sources, each by the parenthesis its own entry opens with — "`ErrorCount` (Amazon MSK and
/// self-managed Apache Kafka)" and "`KafkaMetrics` (Amazon MSK and self-managed Apache
/// Kafka)" — so those two are selectable only there. `EventCount` carries no such
/// parenthesis, which makes it the one value every event source produces.
///
/// `kafka_source` says whether the resolved event source is one of the two the parentheses
/// name.
#[must_use]
pub fn supported_metrics(kafka_source: bool) -> &'static [&'static str] {
    if kafka_source {
        &[
            EVENT_COUNT_METRICS_SELECTOR,
            ERROR_COUNT_METRICS_SELECTOR,
            KAFKA_METRICS_SELECTOR,
        ]
    } else {
        &[EVENT_COUNT_METRICS_SELECTOR]
    }
}

/// What a `Metrics` selection that names one value twice is refused with.
///
/// The sentence is FIXED: the fault is the repetition, which is a property of the list on its
/// own, so neither the value that repeats nor the event source the mapping polls enters it.
pub const DUPLICATE_METRICS_REFUSAL: &str = "MetricsConfig contains duplicate metrics.";

/// Whether `metrics` names any value twice, and if so, what it is refused with.
///
/// A selection is a SET of the values the enum declares — which is why the member's own
/// length stops at the number of values there are — so a value named twice asks for one
/// metric twice and leaves the selection short of being a set. That is a fault of the LIST
/// ALONE, decided without resolving the mapping's event source, so it is read before the
/// per-source vocabulary ([`unsupported_metrics_refusal`]) and answers its own fixed
/// sentence, [`DUPLICATE_METRICS_REFUSAL`].
#[must_use]
pub fn duplicate_metrics_refusal(metrics: &[String]) -> Option<String> {
    metrics
        .iter()
        .enumerate()
        .any(|(index, metric)| metrics[..index].contains(metric))
        .then(|| DUPLICATE_METRICS_REFUSAL.to_string())
}

/// Whether every value in `metrics` is one the event source supports, and if not, what the
/// selection is refused with: the values that source expects instead.
///
/// The judgement is per VALUE, against the ones the source supports
/// ([`supported_metrics`]): a value that source cannot produce is refused with the values it
/// does expect, whether it is stated once or more. How many times a value is named is the
/// separate reading of [`duplicate_metrics_refusal`]. The empty selection asks for nothing
/// and names no unsupported value.
///
/// The sentence is the one AWS states for this refusal, with the values it names taken from
/// the source the request resolved.
#[must_use]
pub fn unsupported_metrics_refusal(metrics: &[String], kafka_source: bool) -> Option<String> {
    let supported = supported_metrics(kafka_source);
    let unsupported = metrics
        .iter()
        .any(|metric| !supported.contains(&metric.as_str()));
    unsupported.then(|| {
        format!(
            "Invalid EventSourceMapping metrics. Expected values: [{}]",
            supported.join(", ")
        )
    })
}

/// How many events a mapping's poller took out of its event source.
pub const POLLED_EVENT_COUNT_METRIC: &str = "PolledEventCount";

/// How many of the polled events the mapping's `FilterCriteria` rejected — "the filter
/// criteria that determine whether Lambda should process an event".
pub const FILTERED_OUT_EVENT_COUNT_METRIC: &str = "FilteredOutEventCount";

/// How many of the polled events were handed to the function in an invocation it accepted.
pub const INVOKED_EVENT_COUNT_METRIC: &str = "InvokedEventCount";

/// How many events the mapping's poller deleted from its event source.
pub const DELETED_EVENT_COUNT_METRIC: &str = "DeletedEventCount";

/// How long the newest record of a batch had been sitting in the shard when the event
/// source mapping read that batch.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `IteratorAge`,
/// "the age of the last record in the event" for an event source mapping that reads from a
/// stream. It is NOT the name the stream service uses for the same quantity in its own
/// namespace — a series is identified by its namespace, its name and its dimensions
/// together, so a client reading Lambda's backlog age asks for Lambda's name and finds
/// nothing under any other.
pub const ITERATOR_AGE_METRIC: &str = "IteratorAge";

/// The `StandardUnit` a span of time carries: `Milliseconds`.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The unit that age carries: it is a span of milliseconds, which CloudWatch's
/// `StandardUnit` spells `Milliseconds`.
pub const ITERATOR_AGE_UNIT: &str = MILLISECONDS_UNIT;

/// How many invocations of a function Lambda performed: the series a client reads to learn
/// that a function is being called at all.
///
/// It counts the invocation REQUESTS Lambda admitted to the function — the ones that got
/// past resolution, the function's lifecycle state, the payload limits and the
/// recursive-loop guardrail — whatever the execution each of them started then came to. A
/// request Lambda refuses before the function is reached is no invocation of it and is not
/// counted here: an unresolvable name, a function that cannot be invoked yet, a payload over
/// the limit or not JSON, a dry run, which "validate[s] parameter values" and invokes
/// nothing, and an invocation the guardrail stopped, which is metered as one
/// [`RECURSIVE_INVOCATIONS_DROPPED_METRIC`] datapoint instead.
///
/// One observation of 1 is published per admitted invocation, in [`COUNT_UNIT`], so a reader
/// summing a window over the series gets the invocations of that window and a function
/// nobody invoked has no datapoint at all rather than a zero.
pub const INVOCATIONS_METRIC: &str = "Invocations";

/// How long one invocation's own run took: the series a client reads to learn how long the
/// function spends on an event.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `Duration`,
/// "the amount of time that your function code spends processing an event", measured in
/// [`MILLISECONDS_UNIT`] — so one observation is published per invocation that RAN, carrying
/// the time that invocation's own run took, and a reader laying periods over the series gets
/// the average, the slowest and the fastest run of each period. An invocation that ran
/// nothing — one the service refused before the function was reached — measured no time and
/// publishes no datapoint.
pub const DURATION_METRIC: &str = "Duration";

/// How many invocations came to a FUNCTION ERROR: the series a client builds an error-rate
/// alarm on.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `Errors`, "the
/// number of invocations that result in a function error", which are "exceptions thrown by
/// your code and exceptions thrown by the Lambda runtime" — so one observation of 1 is
/// published per invocation whose run reported a `FunctionError`, in [`COUNT_UNIT`], and an
/// invocation that returned normally publishes none. A request Lambda refused before the
/// function was reached is no invocation of it and is counted here no more than it is in
/// [`INVOCATIONS_METRIC`].
pub const ERRORS_METRIC: &str = "Errors";

/// How many invocation REQUESTS were throttled: the series a client reads to learn that a
/// function is being called faster than its concurrency allows.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `Throttles`, "the
/// number of invocation requests that are throttled" — the requests answered
/// `TooManyRequestsException` because "running the function would cause you to exceed a
/// concurrency limit at either the account level […] or function level" (`Invoke`). One
/// observation of 1 is published per throttled request, in [`COUNT_UNIT`]; a throttled
/// request never reached the function, so it is counted here INSTEAD of in
/// [`INVOCATIONS_METRIC`].
pub const THROTTLES_METRIC: &str = "Throttles";

/// How many invocation REQUESTS Lambda dropped because it detected the function being
/// invoked as part of a recursive loop and the function's recursive loop detection
/// configuration told it to stop the chain.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]:
/// `RecursiveInvocationsDropped`, dimensioned on the function whose invocation was
/// stopped ([`FUNCTION_NAME_DIMENSION`]) and counted in [`COUNT_UNIT`]. It is the only way
/// a client learns the guardrail fired, so one datapoint is published per invocation
/// stopped and none at all for a function whose loops are allowed — see
/// [`super::recursion`].
pub const RECURSIVE_INVOCATIONS_DROPPED_METRIC: &str = "RecursiveInvocationsDropped";

/// How many INVOCATION RECORDS of completed asynchronous invocations Lambda could not
/// deliver to the destination the function's asynchronous-invocation configuration names.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]:
/// `DestinationDeliveryFailures`, counted in [`COUNT_UNIT`]. One observation of 1 is
/// published per record the destination did NOT accept, and none at all for one it did — so a
/// datapoint on this series means a record really was lost, and an empty window means every
/// record of it arrived (see [`super::async_completion_delivery`]).
pub const DESTINATION_DELIVERY_FAILURES_METRIC: &str = "DestinationDeliveryFailures";

/// How many EVENTS Lambda could not write to the dead-letter target the function's
/// `DeadLetterConfig` names.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]:
/// `DeadLetterErrors`, counted in [`COUNT_UNIT`]. It counts the failures of the DELIVERY and
/// not the events that were dead-lettered: one observation of 1 per event the target did not
/// accept, and none at all for an event it took — so a function whose dead-letter queue is
/// working has no such series at all, however many events it drops there.
pub const DEAD_LETTER_ERRORS_METRIC: &str = "DeadLetterErrors";

/// How long an asynchronous event sat in the invocation queue before the function was
/// invoked with it.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]: `AsyncEventAge`,
/// measured in [`MILLISECONDS_UNIT`]. One observation is published per event the function
/// was invoked with, so a reader laying periods over the series gets the average and the
/// worst queueing delay of each period; an event nothing was invoked for measured no age and
/// publishes no datapoint.
pub const ASYNC_EVENT_AGE_METRIC: &str = "AsyncEventAge";

/// How many asynchronous EVENTS Lambda dropped without the function having processed them.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]:
/// `AsyncEventsDropped`, counted in [`COUNT_UNIT`]. An event is dropped once its deliveries
/// to the function are over and the last of them did not succeed — the attempts it was
/// allowed are spent, or its function could not be run at all — so one observation of 1 is
/// published per such event and none at all for an event a handler processed. It counts the
/// EVENT and not the attempts it had, which is what makes a window's sum the number of events
/// lost in it rather than the number of failures they produced
/// (see [`super::async_completion_delivery`]).
pub const ASYNC_EVENTS_DROPPED_METRIC: &str = "AsyncEventsDropped";

/// How many asynchronous EVENTS Lambda accepted onto the invocation queue.
///
/// The name is the one Lambda's own series carries in [`METRIC_NAMESPACE`]:
/// `AsyncEventsReceived`, counted in [`COUNT_UNIT`]. It counts the events Lambda successfully
/// QUEUED — "a success response (202 Accepted) indicates that the request is queued for
/// invocation" — so one observation of 1 is published per accepted event, whatever its
/// deliveries then come to, and a request Lambda refused before queueing anything is counted
/// here no more than it is in [`INVOCATIONS_METRIC`].
pub const ASYNC_EVENTS_RECEIVED_METRIC: &str = "AsyncEventsReceived";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
pub const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The `StandardUnit` a bare count of things carries: `Count`.
pub const COUNT_UNIT: &str = "Count";

/// The series one of Lambda's own metrics lives in: the [`METRIC_NAMESPACE`] namespace, the
/// metric's name, and the dimension set that identifies it — empty for a series measuring
/// something that belongs to the Region rather than to one function.
#[must_use]
pub fn series(metric: &str, dimensions: cloudwatch::DimensionSet) -> cloudwatch::MetricKey {
    cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions,
    }
}

/// The dimension set of a series reported per function: the function the work was done for.
#[must_use]
pub fn function_dimensions(function_name: &str) -> cloudwatch::DimensionSet {
    [(
        FUNCTION_NAME_DIMENSION.to_string(),
        function_name.to_string(),
    )]
    .into_iter()
    .collect()
}

/// The dimension set of a series reported per INVOKED RESOURCE: the function, and the
/// resource of it the invocation addressed ([`RESOURCE_DIMENSION`]).
#[must_use]
pub fn resource_dimensions(function_name: &str, resource: &str) -> cloudwatch::DimensionSet {
    [
        (
            FUNCTION_NAME_DIMENSION.to_string(),
            function_name.to_string(),
        ),
        (RESOURCE_DIMENSION.to_string(), resource.to_string()),
    ]
    .into_iter()
    .collect()
}

/// The resource an invocation of `function_name` addressed, as [`RESOURCE_DIMENSION`] names
/// it: the function itself for an invocation that resolved the unpublished version the
/// function is addressed by with no qualifier at all, and `name:qualifier` for one that named
/// an alias or a published version — the same distinction the unqualified and `:`-suffixed
/// forms of a function ARN draw ([`super::FunctionRecord::versioned_arn`]).
#[must_use]
pub fn invoked_resource(function_name: &str, qualifier: &str) -> String {
    if qualifier == crate::state::VERSION_LATEST {
        return function_name.to_string();
    }
    format!("{function_name}:{qualifier}")
}

/// The dimension set of a series reported per event source mapping: the mapping whose
/// poller did the work.
#[must_use]
pub fn mapping_dimensions(uuid: &str) -> cloudwatch::DimensionSet {
    [(
        EVENT_SOURCE_MAPPING_UUID_DIMENSION.to_string(),
        uuid.to_string(),
    )]
    .into_iter()
    .collect()
}

/// What ONE pass of a mapping's poller did with the events it took out of the event
/// source — each count a tally of that pass's own work, never a running total kept here:
/// the totals a client reads are the SUM CloudWatch computes over the datapoints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EventCounts {
    /// How many events the pass took out of the event source.
    pub polled: u32,
    /// How many of them the mapping's `FilterCriteria` rejected.
    pub filtered_out: u32,
    /// How many of them an invocation of the mapping's function accepted.
    pub invoked: u32,
    /// How many of them the pass deleted from the event source.
    pub deleted: u32,
}

/// Whether `metrics_config` selects the event-count family, which is the documented switch
/// for it: "the metrics you want your event source mapping to produce". A mapping carrying
/// no `MetricsConfig`, or one whose `Metrics` list does not hold
/// [`EVENT_COUNT_METRICS_SELECTOR`] — an emptied list included — produces none of them.
#[must_use]
pub fn publishes_event_counts(
    metrics_config: Option<&crate::ops::types::EventSourceMappingMetricsConfig>,
) -> bool {
    metrics_config
        .and_then(|config| config.metrics.as_ref())
        .is_some_and(|metrics| {
            metrics
                .iter()
                .any(|metric| metric == EVENT_COUNT_METRICS_SELECTOR)
        })
}

/// ONE measurement of a series, taken at `at_secs` (epoch seconds), published at the
/// regular storage resolution so it is aggregated in the minute it was taken in.
#[must_use]
pub fn observation(value: f64, unit: &str, at_secs: f64) -> cloudwatch::StoredDatum {
    cloudwatch::StoredDatum {
        timestamp_secs: at_secs,
        sample_count: 1.0,
        sum: value,
        minimum: value,
        maximum: value,
        unit: Some(unit.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    }
}

/// Store one observation of the series `key` names, and make every alarm watching that
/// series due for evaluation — exactly as an ingested `PutMetricData` datapoint does, so an
/// alarm cannot tell a service's own metric from a client's.
///
/// Answers whether it was stored: a store already holding its full complement of series
/// drops the observation rather than failing the work it is about, because a metric is
/// published off the response path.
pub fn append(
    store: &::engine::ServiceState<CloudWatchState>,
    key: cloudwatch::MetricKey,
    datum: cloudwatch::StoredDatum,
) -> bool {
    let watched = cloudwatch::WrittenSeries {
        key: key.clone(),
        units: vec![datum.unit().to_string()],
    };
    let at_secs = datum.timestamp_secs;
    let mut store = store.lock();
    if store.append_datapoints(key, vec![datum]).is_err() {
        return false;
    }
    store.make_alarms_due(&[watched], at_secs);
    true
}

/// Publish the ONE datapoint an invocation Lambda admitted to a function is metered by: a
/// count of 1 on the [`INVOCATIONS_METRIC`] series of the function `function_name` names and
/// on that of the resource `resource` addresses, so a reader summing a window over either
/// gets the invocations of that window.
///
/// It goes into the CloudWatch store of the SAME engine, which is the store
/// `GetMetricStatistics`, `GetMetricData` and `ListMetrics` answer from. An engine carrying
/// no CloudWatch has nowhere to publish to, so nothing is. Call it with no Lambda store lock
/// held: it takes the CloudWatch store's own.
pub fn publish_invocation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    resource: &str,
) {
    FunctionMetrics::new(cloudwatch_state(engine)).publish_invocation(function_name, resource);
}

/// Publish the ONE datapoint an invocation request Lambda THROTTLED is metered by: a count of
/// 1 on the [`THROTTLES_METRIC`] series of the function and of the resource the refused
/// request addressed.
///
/// It reaches the same store, under the same conditions, as [`publish_invocation`].
pub fn publish_throttle<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    resource: &str,
) {
    FunctionMetrics::new(cloudwatch_state(engine)).publish_throttle(function_name, resource);
}

/// Publish the ONE datapoint an asynchronous event Lambda ACCEPTED onto the invocation queue
/// is metered by: a count of 1 on the [`ASYNC_EVENTS_RECEIVED_METRIC`] series of the function
/// and of the resource the accepting call addressed.
///
/// It reaches the same store, under the same conditions, as [`publish_invocation`].
pub fn publish_async_event_received<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    resource: &str,
) {
    FunctionMetrics::new(cloudwatch_state(engine)).publish_both(
        ASYNC_EVENTS_RECEIVED_METRIC,
        function_name,
        resource,
        1.0,
        COUNT_UNIT,
    );
}

/// The AWS/Lambda series ONE invocation is metered on once it has RUN, held by the invocation
/// itself so that every path that runs a handler meters it the same way — a synchronous call,
/// a queued event delivered off the response path, a response-streaming call and a function
/// URL request alike.
///
/// It holds no measurement of its own: what it carries is where the datapoints go and which
/// series they identify, and every observation is a datapoint in the CloudWatch store.
#[derive(Clone, Debug)]
pub struct InvocationMetrics {
    /// Where the datapoints go.
    publisher: FunctionMetrics,
    /// The function the invocation ran on.
    function_name: String,
    /// The resource the invocation addressed, as [`RESOURCE_DIMENSION`] names it.
    resource: String,
}

impl InvocationMetrics {
    /// The series an invocation of `function_name` addressed at `resource`
    /// ([`invoked_resource`]) is metered on, publishing into the CloudWatch store of the
    /// SAME engine.
    #[must_use]
    pub fn new<Eng: Send + Sync + 'static>(
        engine: &Eng,
        function_name: &str,
        resource: &str,
    ) -> Self {
        Self {
            publisher: FunctionMetrics::new(cloudwatch_state(engine)),
            function_name: function_name.to_string(),
            resource: resource.to_string(),
        }
    }

    /// Publish what this invocation's finished run came to: the time it took
    /// ([`DURATION_METRIC`]), and — for a run that reported a `FunctionError` — the one
    /// failure it came to ([`ERRORS_METRIC`]).
    ///
    /// Both are published per invocation rather than accumulated here, so a reader laying
    /// periods over either series gets the runs of each period and a period in which the
    /// function failed nothing holds no error datapoint at all.
    pub fn publish_outcome(&self, duration: ::std::time::Duration, errored: bool) {
        self.publisher.publish_duration(
            &self.function_name,
            &self.resource,
            duration.as_secs_f64() * 1000.0,
        );
        if errored {
            self.publisher
                .publish_error(&self.function_name, &self.resource);
        }
    }

    /// Publish that this invocation request was THROTTLED ([`THROTTLES_METRIC`]).
    pub fn publish_throttle(&self) {
        self.publisher
            .publish_throttle(&self.function_name, &self.resource);
    }

    /// Publish that the invocation RECORD of this completed asynchronous invocation did not
    /// reach its destination ([`DESTINATION_DELIVERY_FAILURES_METRIC`]).
    pub fn publish_destination_delivery_failure(&self) {
        self.publisher
            .publish_both(
                DESTINATION_DELIVERY_FAILURES_METRIC,
                &self.function_name,
                &self.resource,
                1.0,
                COUNT_UNIT,
            );
    }

    /// Publish that this invocation's event did not reach the function's dead-letter target
    /// ([`DEAD_LETTER_ERRORS_METRIC`]).
    pub fn publish_dead_letter_error(&self) {
        self.publisher.publish_both(
            DEAD_LETTER_ERRORS_METRIC,
            &self.function_name,
            &self.resource,
            1.0,
            COUNT_UNIT,
        );
    }

    /// Publish how long this invocation's event waited in the asynchronous queue before the
    /// function was invoked with it ([`ASYNC_EVENT_AGE_METRIC`]).
    pub fn publish_async_event_age(&self, waited: ::std::time::Duration) {
        self.publisher.publish_both(
            ASYNC_EVENT_AGE_METRIC,
            &self.function_name,
            &self.resource,
            waited.as_secs_f64() * 1000.0,
            MILLISECONDS_UNIT,
        );
    }

    /// Publish that this invocation's event was DROPPED without the function having
    /// processed it ([`ASYNC_EVENTS_DROPPED_METRIC`]).
    pub fn publish_async_event_dropped(&self) {
        self.publisher.publish_both(
            ASYNC_EVENTS_DROPPED_METRIC,
            &self.function_name,
            &self.resource,
            1.0,
            COUNT_UNIT,
        );
    }

    /// Publish that the function code was invoked once more for this invocation
    /// ([`INVOCATIONS_METRIC`]) — a RETRY of a queued asynchronous event.
    ///
    /// `Invocations` is "the number of invocations", counting every time the function code is
    /// invoked, so a queued event the retry policy delivers again is counted again: the
    /// accepting call metered the one attempt it admitted, and each further attempt is
    /// metered here, where that attempt is made.
    pub fn publish_retry_invocation(&self) {
        self.publisher
            .publish_invocation(&self.function_name, &self.resource);
    }
}

/// The publisher of the metrics Lambda publishes about its own work — on the function a
/// batch is handed to and on the event source mapping that polled it — held by the poller
/// doing that work.
///
/// It holds no counts of its own: every observation is a datapoint in the CloudWatch
/// store, never a second copy kept here.
#[derive(Clone)]
pub struct FunctionMetrics {
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
}

impl ::std::fmt::Debug for FunctionMetrics {
    /// Whether this publisher has a store to append to, which is all of it that is not
    /// that store (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("FunctionMetrics")
            .field("published", &self.store.is_some())
            .finish()
    }
}

impl FunctionMetrics {
    /// The publisher appending to `store`.
    #[must_use]
    pub fn new(store: Option<::engine::ServiceState<CloudWatchState>>) -> Self {
        Self { store }
    }

    /// Publish the iterator age ONE batch read for the function named `function_name`
    /// measured: `age_millis` is how long the newest record of that batch had been in the
    /// shard when it was read.
    pub fn publish_iterator_age(&self, function_name: &str, age_millis: f64) {
        self.publish(ITERATOR_AGE_METRIC, function_name, age_millis, ITERATOR_AGE_UNIT);
    }

    /// Publish the ONE invocation of `function_name` that was admitted to the function,
    /// addressed as `resource` ([`invoked_resource`]): a single observation of
    /// [`INVOCATIONS_METRIC`], counting that one invocation.
    ///
    /// It is published TWICE, once in each of the dimension sets the invocation belongs to —
    /// the function's own ([`function_dimensions`]) and the invoked resource's
    /// ([`resource_dimensions`]) — because "a metric is uniquely defined by a name, a
    /// namespace, and zero or more dimensions", so a reader asking for the function's
    /// invocations and one asking for an alias's are asking for two different series and
    /// each must carry the invocations it is of. Both observations are taken at the same
    /// instant, so a window sums the same count over either.
    pub fn publish_invocation(&self, function_name: &str, resource: &str) {
        self.publish_both(
            INVOCATIONS_METRIC,
            function_name,
            resource,
            1.0,
            COUNT_UNIT,
        );
    }

    /// Publish how long ONE finished run of `function_name`, addressed as `resource`, took:
    /// a single observation of [`DURATION_METRIC`] in [`MILLISECONDS_UNIT`].
    ///
    /// Published on both of the dimension sets the invocation belongs to, for the reason
    /// [`Self::publish_invocation`] is: a reader asking for the function's durations and one
    /// asking for an alias's are asking for two different series.
    pub fn publish_duration(&self, function_name: &str, resource: &str, millis: f64) {
        self.publish_both(
            DURATION_METRIC,
            function_name,
            resource,
            millis,
            MILLISECONDS_UNIT,
        );
    }

    /// Publish that ONE invocation of `function_name`, addressed as `resource`, came to a
    /// function error: a single observation of [`ERRORS_METRIC`].
    pub fn publish_error(&self, function_name: &str, resource: &str) {
        self.publish_both(ERRORS_METRIC, function_name, resource, 1.0, COUNT_UNIT);
    }

    /// Publish that ONE invocation request for `function_name`, addressed at `resource`, was
    /// throttled: a single observation of [`THROTTLES_METRIC`].
    pub fn publish_throttle(&self, function_name: &str, resource: &str) {
        self.publish_both(THROTTLES_METRIC, function_name, resource, 1.0, COUNT_UNIT);
    }

    /// Publish that ONE invocation of the function `function_name` names was dropped by
    /// the recursive-loop guardrail: a single observation of
    /// [`RECURSIVE_INVOCATIONS_DROPPED_METRIC`], counting that one dropped invocation.
    ///
    /// Each drop is its own datapoint rather than a running total, so a reader summing a
    /// window over the series gets the invocations dropped in it, and a window in which
    /// the guardrail dropped nothing holds no datapoint at all.
    pub fn publish_recursive_invocations_dropped(&self, function_name: &str) {
        self.publish(
            RECURSIVE_INVOCATIONS_DROPPED_METRIC,
            function_name,
            1.0,
            COUNT_UNIT,
        );
    }

    /// Publish what ONE pass of the event source mapping `uuid` names did with the events
    /// it polled, at `at_secs` (epoch seconds): the whole event-count family, one
    /// observation of each, so a reader summing a period over any one of them gets the
    /// events of that period.
    ///
    /// Every count is published, a zero included: "the number of events" a pass filtered
    /// out, invoked the function with or deleted is as much a measurement when it is none
    /// as when it is many, and a series that skipped its zeros would leave a client unable
    /// to tell a poller that did nothing from one that is not publishing at all.
    pub fn publish_event_counts(&self, uuid: &str, counts: &EventCounts, at_secs: f64) {
        for (metric, value) in [
            (POLLED_EVENT_COUNT_METRIC, counts.polled),
            (FILTERED_OUT_EVENT_COUNT_METRIC, counts.filtered_out),
            (INVOKED_EVENT_COUNT_METRIC, counts.invoked),
            (DELETED_EVENT_COUNT_METRIC, counts.deleted),
        ] {
            self.publish_series(
                metric,
                mapping_dimensions(uuid),
                f64::from(value),
                COUNT_UNIT,
                at_secs,
                uuid,
            );
        }
    }

    /// Publish that the mapping `uuid` names polled NOTHING: one 0-valued
    /// [`POLLED_EVENT_COUNT_METRIC`] observation at `at_secs` (epoch seconds).
    ///
    /// This is what makes an empty window mean "nothing was there to poll" rather than
    /// "the poller is not publishing": a client reading a zero knows the mapping polled and
    /// found its source empty.
    pub fn publish_idle_poll(&self, uuid: &str, at_secs: f64) {
        self.publish_series(
            POLLED_EVENT_COUNT_METRIC,
            mapping_dimensions(uuid),
            0.0,
            COUNT_UNIT,
            at_secs,
            uuid,
        );
    }

    /// Append one observation of `metric` for one function.
    fn publish(&self, metric: &str, function_name: &str, value: f64, unit: &str) {
        self.publish_series(
            metric,
            function_dimensions(function_name),
            value,
            unit,
            cloudwatch::now_secs(),
            function_name,
        );
    }

    /// Append one observation of `metric` on BOTH of the dimension sets an invocation
    /// belongs to — the function's own and the invoked resource's — at one instant, so a
    /// window reads the same measurement over either.
    fn publish_both(
        &self,
        metric: &str,
        function_name: &str,
        resource: &str,
        value: f64,
        unit: &str,
    ) {
        let at_secs = cloudwatch::now_secs();
        for dimensions in [
            function_dimensions(function_name),
            resource_dimensions(function_name, resource),
        ] {
            self.publish_series(metric, dimensions, value, unit, at_secs, function_name);
        }
    }

    /// Append one observation of the series `metric` and `dimensions` identify, taken at
    /// `at_secs`. `about` is the resource the observation is of, named in the one log line
    /// a store at capacity produces.
    fn publish_series(
        &self,
        metric: &str,
        dimensions: cloudwatch::DimensionSet,
        value: f64,
        unit: &str,
        at_secs: f64,
        about: &str,
    ) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if !append(store, series(metric, dimensions), observation(value, unit, at_secs)) {
            ::tracing::warn!(
                resource = %about,
                metric = %metric,
                "the CloudWatch store is at capacity; a Lambda metric could not be published"
            );
        }
    }
}
