// SPDX-License-Identifier: CC0-1.0

//! @op DescribeEvents
//!
//! Returns events related to clusters, cache security groups, and cache parameter groups.
//! You can obtain events specific to a particular cluster, cache security group, or cache
//! parameter group by providing the name as a parameter. By default, only the events
//! occurring within the last hour are returned; however, you can retrieve up to 14 days'
//! worth of events if necessary.
//!
//! ## Input
//!
//! * `Duration` (IntegerOptional) — The number of minutes worth of events to retrieve.
//! * `EndTime` (TStamp) — The end of the time interval for which to retrieve events,
//!   specified in ISO 8601 format. Example: 2017-03-30T07:03:49.555Z
//! * `Marker` (String) — An optional marker returned from a prior request. Use this marker
//!   for pagination of results from this operation. If this parameter is specified, the
//!   response includes only records beyond the marker, up to the value specified by
//!   MaxRecords.
//! * `MaxRecords` (IntegerOptional) — The maximum number of records to include in the
//!   response. If more records exist than the specified MaxRecords value, a marker is
//!   included in the response so that the remaining results can be retrieved. Default: 100
//!   Constraints: minimum 20; maximum 100.
//! * `SourceIdentifier` (String) — The identifier of the event source for which events are
//!   returned. If not specified, all sources are included in the response.
//! * `SourceType` (SourceType, one of: cache-cluster | cache-parameter-group |
//!   cache-security-group | cache-subnet-group | replication-group | serverless-cache |
//!   serverless-cache-snapshot | user | user-group) — The event source to retrieve events
//!   for. If no value is specified, all events are returned.
//! * `StartTime` (TStamp) — The beginning of the time interval to retrieve events for,
//!   specified in ISO 8601 format. Example: 2017-03-30T07:03:49.555Z
//!
//! ## Output
//!
//! * `Events` (EventList) — A list of events. Each element in the list contains detailed
//!   information about one event.
//! * `Marker` (String) — Provides an identifier to allow retrieval of paginated results.
//!
//! ## Errors
//!
//! * `InvalidParameterCombinationException` (HTTP 400) — Two or more incompatible parameters
//!   were specified.
//! * `InvalidParameterValueException` (HTTP 400) — The value for a parameter is invalid.
//!
//! ## How this is served
//!
//! This is a REPORTING operation over the event log in [`crate::state`]: every change
//! this service makes to one of its resources records an event there (a cluster created,
//! modified or deleted, a snapshot taken, a cache subnet group or a serverless cache
//! created, and every status transition the clock materialises), and this handler selects
//! from that log. The only write it makes is materialising what the clock already
//! owed — the status transitions that fell due (`settle_all`, which records their events)
//! and the expiry of events past [`crate::state::EVENT_RETENTION`] — both due at this
//! instant whether or not anybody called.
//!
//! The window reported is `[StartTime, EndTime]`, where `EndTime` defaults to now and
//! `StartTime` to `Duration` minutes before the end of the window, or to
//! [`crate::state::DEFAULT_EVENT_WINDOW`] before it when neither is given ("By default,
//! only the events occurring within the last hour are returned"). `Duration` is the
//! relative spelling of the start of the window, so an explicit `StartTime` is what
//! bounds it when both are sent.
//!
//! Within that window the list is strictly REVERSE-chronological — the most recent
//! `Event$Date` first — and `Marker` pages through it in the same order, so the first
//! page a client reads is what just happened rather than the oldest events retained.

#[allow(unused_imports)]
use super::types::*;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeEventsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeEventsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.elasticache#DescribeEvents")]
#[derive(Debug)]
pub enum DescribeEventsErrorModeled {
    InvalidParameterCombinationException(InvalidParameterCombinationException),
    InvalidParameterValueException(InvalidParameterValueException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_events::DescribeEventsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeEventsError = crate::scaffolded::Scaffolded<DescribeEventsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeEventsErrorModeled> for DescribeEventsError {
    fn from(modeled: DescribeEventsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeEvents` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeEvents<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeEvents<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeEvents<Eng>
{
    type Input = DescribeEventsInput;
    type Output = DescribeEventsOutput;
    type Error = DescribeEventsError;

    const NAME: &'static str = "DescribeEvents";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_EVENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_EVENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A reporting operation over the event log; see the module docs for the
        // two writes the clock owes it.
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::STORE_UNAVAILABLE,
            ));
        };

        // `MaxRecords` carries no modelled `@range`, so the bounds its own
        // documentation states ("Default: 100 Constraints: minimum 20; maximum
        // 100") are enforced here — the same ones every paginated describe in
        // this service shares.
        let page_size =
            crate::state::page_size(input.max_records).map_err(invalid_parameter_value)?;
        // A `Marker` only ever travels back unchanged, so one this service did
        // not mint is a bad parameter value rather than an empty page.
        let resume_from = match input.marker.as_deref() {
            Some(marker) => match crate::state::decode_marker(marker) {
                Some(key) => Some(key),
                None => {
                    return Err(invalid_parameter_value(format!(
                        "Invalid value {marker} for Marker."
                    )))
                }
            },
            None => None,
        };

        // A report answers with the state the clock owes at the instant it
        // answers, so a change already scheduled to fall due within a transition
        // span is waited for here rather than reported as still outstanding: the
        // first poll a client issues after the call that started the work
        // converges (`crate::state::await_settled`).
        crate::state::await_settled(&state).await;
        let now = SystemTime::now();
        // The end of the window: `EndTime` when the request bounds it, else the
        // instant of the call.
        let end = match input.end_time {
            Some(at) => instant("EndTime", at)?,
            None => now,
        };
        // `Duration` is "the number of minutes worth of events to retrieve" — a
        // span, so it has to be at least one minute. The member is an
        // `IntegerOptional` with no modelled `@range`, so any `i32` reaches
        // here.
        let span = match input.duration {
            Some(minutes) => {
                let Ok(minutes) = u64::try_from(minutes) else {
                    return Err(duration_refusal(minutes));
                };
                if minutes == 0 {
                    return Err(duration_refusal(0));
                }
                Some(Duration::from_secs(minutes * 60))
            }
            None => None,
        };
        // The start of the window: `StartTime` names it outright, `Duration`
        // names it relative to the end, and with neither it is
        // `DEFAULT_EVENT_WINDOW` before the end ("By default, only the events
        // occurring within the last hour are returned"). An explicit
        // `StartTime` is what bounds the window when both are sent, because
        // `Duration` is only the relative spelling of the same bound.
        let start = match (input.start_time, span) {
            (Some(at), _) => instant("StartTime", at)?,
            (None, Some(span)) => end.checked_sub(span).unwrap_or(UNIX_EPOCH),
            (None, None) => end
                .checked_sub(crate::state::DEFAULT_EVENT_WINDOW)
                .unwrap_or(UNIX_EPOCH),
        };
        if start > end {
            return Err(invalid_parameter_combination(
                "StartTime must not be later than EndTime: the two bound one time interval."
                    .to_string(),
            ));
        }

        // `SourceIdentifier`: "the identifier of the event source for which
        // events are returned. If not specified, all sources are included." A
        // resource's identity in this API is its lowercased name, so the filter
        // resolves the same way a describe of that resource does.
        let source_identifier = input.source_identifier.as_deref();
        // `SourceType`: "the event source to retrieve events for. If no value is
        // specified, all events are returned." Its member is a modelled enum,
        // so a value outside the enum is already refused by the boundary
        // validator and cannot reach here.
        let source_type = input.source_type.as_deref();

        let mut store = state.lock();
        // The two writes the clock owes: the status transitions that have fallen
        // due (each recording its own event) and the expiry of events past the
        // retention window. Both were owed at this instant whether or not
        // anybody called.
        store.settle_all(now);

        let mut page: Vec<Event> = Vec::new();
        let mut marker = None;
        // Newest FIRST: the event list a client reads is strictly
        // reverse-chronological, so the first page of "what just happened" is the
        // most recent events rather than the oldest ones. The log is keyed by the
        // instant each event happened (`crate::state::event_key`), so reading the
        // map backwards IS that order, and the `Marker` indexes into the same
        // order — the cursor a page hands back resumes at the next OLDER event,
        // which is why the resume bound runs the other way round too.
        for (key, event) in store.events.iter().rev().filter(|(key, event)| {
            resume_from.as_ref().is_none_or(|from| *key <= from)
                && event.at >= start
                && event.at <= end
                && source_identifier
                    .is_none_or(|id| event.source_identifier.eq_ignore_ascii_case(id))
                && source_type.is_none_or(|kind| event.source_type == kind)
        }) {
            if page.len() == page_size {
                // More records exist than `MaxRecords`, so the response carries
                // the marker the next call resumes from — the key of the first
                // event this page did not return.
                marker = Some(crate::state::encode_marker(key));
                break;
            }
            page.push(event.to_modeled());
        }

        Ok(DescribeEventsOutput {
            marker,
            events: Some(page),
        })
    }
}

/// `message` as this operation's modelled `InvalidParameterValueException`.
fn invalid_parameter_value(message: String) -> DescribeEventsError {
    DescribeEventsErrorModeled::InvalidParameterValueException(InvalidParameterValueException {
        message: Some(message),
    })
    .into()
}

/// `message` as this operation's modelled
/// `InvalidParameterCombinationException`, which is what "two or more
/// incompatible parameters were specified" is refused with.
fn invalid_parameter_combination(message: String) -> DescribeEventsError {
    DescribeEventsErrorModeled::InvalidParameterCombinationException(
        InvalidParameterCombinationException {
            message: Some(message),
        },
    )
    .into()
}

/// How a `Duration` that is not a span of minutes is refused.
fn duration_refusal(minutes: i32) -> DescribeEventsError {
    invalid_parameter_value(format!(
        "Invalid value {minutes} for Duration: a number of minutes worth of events must be at \
         least 1."
    ))
}

/// The instant a modelled timestamp names, or the refusal for one this service
/// cannot read as an instant.
fn instant(
    member: &str,
    at: ::aws_smithy_types::DateTime,
) -> Result<SystemTime, DescribeEventsError> {
    SystemTime::try_from(at).map_err(|_| {
        invalid_parameter_value(format!(
            "Invalid value {} for {member}: not an instant this service can read.",
            at.secs()
        ))
    })
}
