// SPDX-License-Identifier: CC0-1.0

//! The EVENT BUS target stage: what a pipe does with the events it is delivering when its
//! `Target` names an EventBridge event bus.
//!
//! "The parameters for using an EventBridge event bus as a target"
//! (`PipeTargetEventBridgeEventBusParameters`) are the members of the event the bus is
//! handed — its `Source`, its `DetailType`, the `Resources` it primarily concerns and the
//! `Time` it carries — so a bus target takes each event as ONE `PutEvents` entry whose
//! `Detail` is what the target stage was handed for that event: the record envelope, or the
//! payload the pipe's `InputTemplate` rendered from it. Everything a rule on that bus can
//! read comes from those members, which is why a rule matching the configured source fires
//! for the records the pipe moved.
//!
//! How many CALLS a batch comes to is this target's own, as it is for every other kind
//! ([`super::deliver_batch`]): `PutEvents` takes a list of entries whose length is
//! `@length(min: 1, max: 10)` (`PutEventsRequest$Entries` of
//! `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json`),
//! so the events of a batch are published [`MAX_ENTRIES_PER_CALL`] at a time rather than in
//! one call of any length.
//!
//! A value the pipe configured is used AS CONFIGURED unless it is a JSON path into the event
//! ([`super::resolved_parameter`]), which every member here admits — `Source`'s own modelled
//! pattern spells the path form out and `Time` is modelled as a `JsonPath` — so one pipe can
//! publish each record under the source, detail type, resources or timestamp the record
//! itself carries.
//!
//! An entry the bus did not take is the target stage FAILING, exactly as a refused
//! `SendMessage` is: the records stay on their source and the pipe hands them over again. A
//! batch longer than one call's entries is published chunk by chunk, so a chunk the bus
//! refused leaves the chunks before it published and their records still on the source —
//! delivery is at-least-once, as it is for every other target, and never atomic per batch.
//!
//! The publication is made over the in-process callback into THIS emulator
//! ([`super::Clients`]), so the only bus a pipe can reach is one this emulator itself holds:
//! no socket is opened and no name is resolved for the ARN a pipe carries.

use ::serde_json::Value;

use super::{resolved_parameter, target_body, CallFailure, Clients, Delivery};
use crate::ops::types::PipeTargetEventBridgeEventBusParameters;

/// The resource type an event bus ARN names: `arn:…:events:…:event-bus/<name>`.
pub(super) const EVENT_BUS_RESOURCE: &str = "event-bus";

/// The IAM action the delivery to a bus is authorized under: the API call that PUTS the
/// events on it, in EventBridge's own namespace.
pub(super) const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The most entries ONE `PutEvents` carries: `PutEventsRequest$Entries` is
/// `@length(min: 1, max: 10)`, so a batch longer than this is published over several calls.
const MAX_ENTRIES_PER_CALL: usize = 10;

/// The members of `PipeTargetEventBridgeEventBusParameters` resolved per event, named as the
/// model names them — what a resolution failure of one of them is reported by.
const SOURCE: &str = "Source";
/// See [`SOURCE`].
const DETAIL_TYPE: &str = "DetailType";
/// See [`SOURCE`].
const RESOURCES: &str = "Resources";
/// See [`SOURCE`].
const TIME: &str = "Time";

/// The event bus a pipe's events are published to: the bus itself, and the members of the
/// event its `EventBridgeEventBusParameters` spell out.
///
/// Each member is held AS THE PIPE CONFIGURED IT, because a value may be a JSON path into
/// the event and is then resolved for every delivery rather than once for the pipe.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Bus {
    /// The bus's ARN as the pipe configured it: what the entries name their bus by, and the
    /// resource the publication is authorized on.
    arn: String,
    /// The Region the ARN names — an event is published on the bus in its own Region.
    region: String,
    /// `DetailType`: "used to decide what fields to expect in the event detail".
    detail_type: Option<String>,
    /// `Source`: "the source of the event", which is what a rule on the bus matches events
    /// of this pipe by.
    source: Option<String>,
    /// `Resources`: the resources "which the event primarily concerns. Any number,
    /// including zero, may be present".
    resources: Vec<String>,
    /// `Time`: "the time stamp of the event, per RFC3339", absent for an event timestamped
    /// as the bus ingests it.
    time: Option<String>,
    /// `EndpointId`: "the URL subdomain of the endpoint", carried on the publication
    /// itself (`PutEventsRequest$EndpointId`) rather than on its entries.
    endpoint_id: Option<String>,
}

impl Bus {
    /// The ARN the pipe named this bus by: the resource the publication is authorized on, and
    /// what the entries name their bus by.
    pub(super) fn arn(&self) -> &str {
        &self.arn
    }
}

/// The bus the `Target` ARN `arn` names, carrying the event members `parameters` configure.
pub(super) fn resolve(
    arn: &str,
    region: &str,
    parameters: Option<&PipeTargetEventBridgeEventBusParameters>,
) -> Bus {
    // An empty member configures nothing: it is the absence of a value, not a value that is
    // the empty string — the same reading every other target parameter is resolved under.
    let configured = |value: Option<&String>| {
        value
            .filter(|text| !text.is_empty())
            .map(String::to_string)
    };
    Bus {
        arn: arn.to_string(),
        region: region.to_string(),
        detail_type: configured(parameters.and_then(|p| p.detail_type.as_ref())),
        source: configured(parameters.and_then(|p| p.source.as_ref())),
        resources: parameters
            .and_then(|p| p.resources.as_ref())
            .into_iter()
            .flatten()
            .filter(|resource| !resource.is_empty())
            .cloned()
            .collect(),
        time: configured(parameters.and_then(|p| p.time.as_ref())),
        endpoint_id: configured(parameters.and_then(|p| p.endpoint_id.as_ref())),
    }
}

/// Publish every event of the batch on the bus, or why the bus did not take them.
///
/// Each entry carries ONE event's payload as its `Detail`, under the members the pipe
/// configured as that event resolves them. A call the bus refused, and an entry it reported
/// it did not take, are both the whole attempt failing: nothing of the batch is finished
/// with its source, so the records are handed over again.
pub(super) async fn publish(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    bus: &Bus,
    payloads: &[Delivery<'_>],
) -> Result<(), CallFailure> {
    // Cloned once: the client is reached through the store of clients, and the entries of a
    // call are built while the request that carries them is held.
    let client = clients.eventbridge(callback, &bus.region).clone();
    for chunk in payloads.chunks(MAX_ENTRIES_PER_CALL) {
        let mut request = client.put_events();
        if let Some(endpoint) = &bus.endpoint_id {
            request = request.endpoint_id(endpoint);
        }
        for (payload, event) in chunk {
            request = request.entries(entry(bus, payload.as_ref(), event.as_ref())?);
        }
        let answered = request.send().await.map_err(|error| {
            format!(
                "the target event bus {} did not take the events ({})",
                bus.arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        // "The number of failed entries" (`PutEventsResponse$FailedEntryCount`): an entry
        // the bus refused is reported on the entry itself rather than as a refusal of the
        // call, so what it says about that entry is what this stage failed with.
        if answered.failed_entry_count() > 0 {
            let refused = answered
                .entries()
                .iter()
                .find_map(|entry| {
                    entry.error_code().map(|code| {
                        format!("{code}: {}", entry.error_message().unwrap_or_default())
                    })
                })
                .unwrap_or_else(|| "the bus named no reason".to_string());
            return Err(CallFailure::from(format!(
                "the target event bus {} refused an event ({refused})",
                bus.arn
            )));
        }
    }
    Ok(())
}

/// ONE event as the entry that publishes it: the payload the target stage was handed as its
/// `Detail`, under the members the pipe configured as THIS event resolves them.
fn entry(
    bus: &Bus,
    payload: &Value,
    event: &Value,
) -> Result<::aws_sdk_eventbridge::types::PutEventsRequestEntry, String> {
    let mut built = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
        .event_bus_name(bus.arn())
        .detail(target_body(payload));
    if let Some(source) = resolved_parameter(SOURCE, bus.source.as_deref(), event)? {
        built = built.source(source);
    }
    if let Some(detail_type) = resolved_parameter(DETAIL_TYPE, bus.detail_type.as_deref(), event)? {
        built = built.detail_type(detail_type);
    }
    for resource in &bus.resources {
        if let Some(resource) = resolved_parameter(RESOURCES, Some(resource), event)? {
            built = built.resources(resource);
        }
    }
    if let Some(at) = resolved_parameter(TIME, bus.time.as_deref(), event)? {
        built = built.time(instant(&at)?);
    }
    Ok(built.build())
}

/// The instant the text `at` spells, read as the timestamp an event carries: "the time stamp
/// of the event, per RFC3339" (`PipeTargetEventBridgeEventBusParameters$Time`).
///
/// Text spelling no such instant is a FAILED delivery rather than an event timestamped with
/// something else — the record stays on the source, exactly as for a target parameter whose
/// path named no value.
fn instant(at: &str) -> Result<::aws_smithy_types::DateTime, String> {
    ::aws_smithy_types::DateTime::from_str(at, ::aws_smithy_types::date_time::Format::DateTime)
        .map_err(|error| format!("the target parameter {TIME} is no RFC3339 time stamp ({error})"))
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::{entry, resolve, Bus};
    use crate::ops::types::PipeTargetEventBridgeEventBusParameters;

    /// The bus every row below publishes on.
    const BUS: &str = "arn:aws:events:us-east-1:000000000000:event-bus/orders";

    /// The event one record comes to, as the target stage reads it: a queue record whose
    /// body spells a JSON document.
    fn event() -> ::serde_json::Value {
        json!({
            "messageId": "a2f1c0e4-0000-4000-8000-000000000000",
            "body": { "kind": "alpha", "at": "2026-01-01T00:00:00Z" },
            "eventSourceARN": "arn:aws:sqs:us-east-1:000000000000:orders",
        })
    }

    /// The parameters a pipe configures, from the members the model carries.
    fn parameters(
        source: Option<&str>,
        detail_type: Option<&str>,
        resources: Option<Vec<&str>>,
        time: Option<&str>,
    ) -> PipeTargetEventBridgeEventBusParameters {
        PipeTargetEventBridgeEventBusParameters {
            endpoint_id: None,
            detail_type: detail_type.map(str::to_string),
            source: source.map(str::to_string),
            resources: resources
                .map(|list| list.into_iter().map(str::to_string).collect()),
            time: time.map(str::to_string),
        }
    }

    /// One row of the table: the parameters the pipe configured, and the members the entry
    /// built for one record must carry.
    struct Row {
        case: &'static str,
        configured: PipeTargetEventBridgeEventBusParameters,
        source: Option<&'static str>,
        detail_type: Option<&'static str>,
        resources: Vec<&'static str>,
        time: Option<&'static str>,
    }

    /// RULE: a bus target publishes each event the stage was handed as one entry whose
    /// `Detail` is that payload, under the `Source`, `DetailType`, `Resources` and `Time`
    /// the pipe configured — each used as configured unless it is a JSON path into the
    /// event, which is resolved against the event being published.
    ///
    /// The rows differ along which members the pipe configures and whether each is a
    /// literal value or a path, which is what the rule generalises over.
    #[test]
    fn an_entry_carries_the_event_under_the_configured_members() {
        let rows: [Row; 5] = [
            Row {
                case: "the source and detail type as configured",
                configured: parameters(Some("sim2real.pipes"), Some("OrderPlaced"), None, None),
                source: Some("sim2real.pipes"),
                detail_type: Some("OrderPlaced"),
                resources: Vec::new(),
                time: None,
            },
            Row {
                case: "no parameters at all",
                configured: parameters(None, None, None, None),
                source: None,
                detail_type: None,
                resources: Vec::new(),
                time: None,
            },
            Row {
                case: "the detail type taken from the record",
                configured: parameters(Some("sim2real.pipes"), Some("$.body.kind"), None, None),
                source: Some("sim2real.pipes"),
                detail_type: Some("alpha"),
                resources: Vec::new(),
                time: None,
            },
            Row {
                case: "the resources the event concerns",
                configured: parameters(
                    Some("sim2real.pipes"),
                    Some("OrderPlaced"),
                    Some(vec!["arn:aws:s3:::orders", "$.eventSourceARN"]),
                    None,
                ),
                source: Some("sim2real.pipes"),
                detail_type: Some("OrderPlaced"),
                resources: vec![
                    "arn:aws:s3:::orders",
                    "arn:aws:sqs:us-east-1:000000000000:orders",
                ],
                time: None,
            },
            Row {
                case: "the time stamp taken from the record",
                configured: parameters(
                    Some("sim2real.pipes"),
                    Some("OrderPlaced"),
                    None,
                    Some("$.body.at"),
                ),
                source: Some("sim2real.pipes"),
                detail_type: Some("OrderPlaced"),
                resources: Vec::new(),
                time: Some("2026-01-01T00:00:00Z"),
            },
        ];
        for Row {
            case,
            configured,
            source,
            detail_type,
            resources,
            time,
        } in rows
        {
            let bus = resolve(BUS, "us-east-1", Some(&configured));
            let event = event();
            let built = entry(&bus, &event, &event)
                .unwrap_or_else(|reason| panic!("case {case:?}: no entry was built ({reason})"));
            assert_eq!(
                built.event_bus_name(),
                Some(bus.arn()),
                "case {case:?}: expected the entry on the configured bus, got {built:?}"
            );
            assert_eq!(
                built.detail(),
                Some(event.to_string().as_str()),
                "case {case:?}: expected the event as the entry's detail, got {built:?}"
            );
            assert_eq!(
                built.source(),
                source,
                "case {case:?}: expected the source {source:?}, got {:?}",
                built.source()
            );
            assert_eq!(
                built.detail_type(),
                detail_type,
                "case {case:?}: expected the detail type {detail_type:?}, got {:?}",
                built.detail_type()
            );
            assert_eq!(
                built.resources(),
                resources.as_slice(),
                "case {case:?}: expected the resources {resources:?}, got {:?}",
                built.resources()
            );
            let carried = built.time().map(|at| {
                at.fmt(::aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_default()
            });
            assert_eq!(
                carried.as_deref(),
                time,
                "case {case:?}: expected the time stamp {time:?}, got {carried:?}"
            );
        }
    }

    /// RULE: a member whose value is a path that names no one value of the event is a
    /// FAILED delivery, not an entry carrying the path itself — and a `Time` that spells no
    /// RFC3339 instant is one too.
    #[test]
    fn a_member_that_cannot_be_resolved_fails_the_delivery() {
        // (case, the parameters the pipe configured)
        let rows: [(&str, PipeTargetEventBridgeEventBusParameters); 3] = [
            (
                "a source path naming nothing of the event",
                parameters(Some("$.body.missing"), Some("OrderPlaced"), None, None),
            ),
            (
                "a detail type path naming an object",
                parameters(Some("sim2real.pipes"), Some("$.body"), None, None),
            ),
            (
                "a time that is no instant",
                parameters(
                    Some("sim2real.pipes"),
                    Some("OrderPlaced"),
                    None,
                    Some("$.body.kind"),
                ),
            ),
        ];
        for (case, configured) in rows {
            let bus: Bus = resolve(BUS, "us-east-1", Some(&configured));
            let event = event();
            let built = entry(&bus, &event, &event);
            assert!(
                built.is_err(),
                "case {case:?}: expected the delivery to fail, got {built:?}"
            );
        }
    }
}
