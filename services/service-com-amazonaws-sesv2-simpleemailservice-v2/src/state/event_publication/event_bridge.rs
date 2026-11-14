// SPDX-License-Identifier: CC0-1.0

//! The Amazon EventBridge destination of an event destination: the event bus a
//! configuration set publishes its email events onto, and the event one notification is
//! published as.
//!
//! `EventBridgeDestination$EventBusArn` is "the ARN of the Amazon EventBridge bus to
//! publish email events to", so a destination of this kind receives each matching email
//! event as ONE event put on that bus. The bus is reached the way any emulated service
//! reaches another — the AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — so nothing here opens a
//! socket or leaves the host, and the publication is the same best-effort hand-over every
//! other destination kind gets ([`super::notification`]): the message was accepted before
//! any event of it existed, so a bus that takes nothing changes nothing about the send.
//!
//! What the event carries is what the SES event-publishing data format states for a bus:
//! the `source` is this service's own event source ([`EVENT_SOURCE`]), the `detail` is the
//! very notification document every other destination kind of the same event destination
//! receives, and the `detail-type` names the email event the detail is of
//! ([`super::notification::Notification::detail_type`]) — so a rule matching this service's
//! source, or one of its event types, sees the whole envelope the published document is
//! wrapped in.

/// The ARN namespace of Amazon EventBridge, which the Region of the bus a destination
/// names is read out of — the Region the in-process publication is addressed at.
const EVENTS_ARN_NAMESPACE: &str = "events";

/// The `source` of every email event this service publishes onto an event bus: the
/// reserved event source of Amazon SES, which is what an EventBridge rule over this
/// service's events matches on.
pub const EVENT_SOURCE: &str = "aws.ses";

/// Put `document` on the event bus `destination` names, as one event of `detail_type`.
///
/// A bus this call cannot reach — one of another account, one nothing created, or one whose
/// ARN names no Region — takes nothing, which is logged and changes nothing about the send.
pub async fn put_event(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    destination: &crate::ops::types::EventBridgeDestination,
    detail_type: &str,
    document: &::serde_json::Value,
) {
    let bus_arn = destination.event_bus_arn.as_str();
    // The Region out of the bus's own ARN, under the Region grammar every model states for
    // the field: it is the only part of the destination that reaches the endpoint the
    // in-process call is addressed at, so a value that is no Region label names no bus this
    // can reach. Nothing else the caller supplied becomes an address — the endpoint is built
    // from that label alone ([`::service_common::eventbridge_client`]) and the transport is
    // the callback's own, inside this process.
    let Some(region) = ::service_common::arn::region_of(bus_arn, EVENTS_ARN_NAMESPACE) else {
        ::tracing::debug!(
            bus_arn,
            "an event destination's event bus ARN names no Region, so no event is published to it"
        );
        return;
    };
    // The entry addresses the bus by the ARN the destination named: `EventBusNameOrArn`
    // admits either spelling, and the ARN is what this service holds for the destination.
    let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
        .event_bus_name(bus_arn)
        .source(EVENT_SOURCE)
        .detail_type(detail_type)
        .detail(document.to_string())
        .build();
    let answered = ::service_common::eventbridge_client::eventbridge_client(callback, region)
        .put_events()
        .entries(entry)
        .send()
        .await;
    match answered {
        Ok(answer) if answer.failed_entry_count() == 0 => {}
        Ok(answer) => ::tracing::debug!(
            bus_arn,
            detail_type,
            failed_entry_count = answer.failed_entry_count(),
            "an event destination's event bus refused a published email event"
        ),
        Err(error) => ::tracing::debug!(
            bus_arn,
            detail_type,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "an event destination's event bus did not take a published email event"
        ),
    }
}
