// SPDX-License-Identifier: CC0-1.0

//! The management-event FAN-OUT: a served control-plane call published onto the account's
//! default EventBridge bus, as the event CloudTrail delivers for it.
//!
//! An account has three readers of its management events. One is CloudTrail's own
//! `LookupEvents`, which reads the log [`crate::management_events`] holds. The second is
//! EventBridge: while a trail of the Region is logging management events, each call that
//! trail logs AND THAT CHANGED THE ACCOUNT is also published onto the account's default
//! event bus, under the `detail-type` [`API_CALL_DETAIL_TYPE`] with `source`
//! `aws.<service>` and the CloudTrail record of the call as the event's `detail`. That is
//! the only path a control-plane call of a service with no event source of its own has to
//! an event-driven consumer, and it is what a rule like
//! `{"source":["aws.cloudfront"],"detail-type":["AWS API Call via CloudTrail"]}` matches.
//! The third reads the trail's own bucket, which is [`crate::trail_log_files`]' leg of the
//! same delivery and travels with the event here.
//!
//! A call that only REPORTED state reaches the first and the third reader and not this one:
//! a read is recorded in event history and written into each logging trail's bucket, and no
//! event is published for it ([`DeliveredApiCall::on_default_bus`]). So a consumer wired to
//! this channel sees the calls that changed the account, never its own polling.
//!
//! The publication therefore hangs off the SAME seam the recording does
//! ([`crate::audited_operation`]): the service that served the call records it, the log
//! answers whether the account's CloudTrail is delivering that Region's management events
//! ([`crate::management_events::ManagementEventLog::record_delivered`]), and what comes
//! back is published here. No service states the channel for itself.
//!
//! EventBridge is reached the way any emulated service reaches another: through the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), with the
//! client this crate's one construction of it builds
//! ([`crate::eventbridge_client`]), so this module names no service crate. Publication is
//! BEST EFFORT — an emulator with no EventBridge mounted has nowhere to publish, and a
//! caller is never failed because the fan-out could not be made.

use smithy_server_runtime::EmbeddedEmulator;

/// The `detail-type` CloudTrail's management-event fan-out carries, which is what a rule
/// written for this channel matches on.
pub const API_CALL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The bus the fan-out publishes onto: "AWS events are published to the default event bus"
/// — a rule on any other bus sees none of them.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `source` prefix every AWS-published event carries, ahead of the service's own
/// endpoint prefix (`aws.cloudfront`).
pub const AWS_SOURCE_PREFIX: &str = "aws.";

/// The suffix a CloudTrail `eventSource` carries after the service's endpoint prefix
/// (`cloudfront.amazonaws.com`), which is what is stripped to build the event `source`.
const EVENT_SOURCE_SUFFIX: &str = ".amazonaws.com";

/// The `source` of the event published for a call recorded under `event_source`.
///
/// CloudTrail names the serving service as `<endpoint-prefix>.amazonaws.com` and
/// EventBridge names the same service as `aws.<endpoint-prefix>`, so the one is derived
/// from the other rather than stated twice per service. A value that does not carry the
/// suffix is taken whole, which is the honest reading of a source this rule does not
/// recognise.
#[must_use]
pub fn event_source_of(event_source: &str) -> String {
    let prefix = event_source
        .strip_suffix(EVENT_SOURCE_SUFFIX)
        .unwrap_or(event_source);
    format!("{AWS_SOURCE_PREFIX}{prefix}")
}

/// One recorded call, as what the account's CloudTrail delivers for it.
///
/// Built by the log that recorded the call — so both the `detail` and each log file's record
/// are the very document `LookupEvents` reports and none of them can disagree — and
/// delivered by [`DeliveredApiCall::deliver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredApiCall {
    /// The published event's `source`: `aws.<endpoint-prefix>` of the serving service.
    pub source: String,
    /// The Region the event is published in, which is the Region the call was recorded in.
    pub region: String,
    /// The instant the call was recorded, as whole milliseconds since the epoch: the
    /// published event's `time`, which is WHEN the call was served and not when the
    /// publication was made.
    pub time_millis: i64,
    /// The published event's `detail`: the CloudTrail record of the call
    /// ([`crate::management_events::ManagementEvent::to_cloudtrail_event_json`]).
    pub detail: serde_json::Value,
    /// The log files the account's logging trails deliver this record as — one per trail
    /// that logs the call, into the bucket that trail was created with
    /// ([`crate::trail_log_files`]). Empty when no logging trail of the account has a
    /// destination bucket.
    pub log_files: Vec<crate::trail_log_files::DeliveredLogFile>,
    /// Whether this record is also PUBLISHED onto the account's default event bus.
    ///
    /// The fan-out carries the management calls that CHANGED the account. A call that only
    /// reported state (`readOnly`) is logged and delivered to every logging trail's bucket
    /// like any other, and no `AWS API Call via CloudTrail` event is published for it — so a
    /// rule over a service's API-call events is driven by the writes made on its resources
    /// and not by the reads a client or a console made while watching them.
    pub on_default_bus: bool,
}

impl DeliveredApiCall {
    /// Deliver this record everywhere the account's CloudTrail delivers it: into the bucket
    /// of each trail that logs the call, and — for a call that changed the account
    /// ([`Self::on_default_bus`]) — onto the account's default event bus.
    ///
    /// Nothing about the call that produced it changes: a failure is traced and the
    /// operation's own answer stands, for the reason the module docs give.
    pub async fn deliver(self, callback: &EmbeddedEmulator) {
        for log_file in self.log_files {
            log_file.deliver(callback).await;
        }
        if !self.on_default_bus {
            // A reporting call is recorded and logged, and published nowhere: see
            // [`Self::on_default_bus`].
            return;
        }
        let client = crate::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(&self.source)
            .detail_type(API_CALL_DETAIL_TYPE)
            // "If no time stamp is provided, the time stamp of the PutEvents call is
            // used" — the call's own instant is provided, so the event is stamped when
            // the call was served.
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.time_millis,
            ))
            .detail(self.detail.to_string())
            .build();
        let answered = client.put_events().entries(entry).send().await;
        match answered {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    source = %self.source,
                    region = %self.region,
                    "the management-event fan-out was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    source = %self.source,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the management-event fan-out could not be published"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_source_becomes_the_bus_source_of_the_same_service() {
        for (event_source, expected) in [
            ("cloudfront.amazonaws.com", "aws.cloudfront"),
            ("iam.amazonaws.com", "aws.iam"),
            ("elasticloadbalancing.amazonaws.com", "aws.elasticloadbalancing"),
            // A value that does not carry the suffix is taken whole rather than
            // truncated at a guess.
            ("cloudfront", "aws.cloudfront"),
        ] {
            let actual = event_source_of(event_source);
            assert_eq!(
                actual, expected,
                "case {event_source:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
