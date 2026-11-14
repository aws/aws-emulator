// SPDX-License-Identifier: CC0-1.0

//! The EventBridge envelope each recorded RDS event is also PUBLISHED as.
//!
//! Amazon RDS does not only keep a lifecycle change for a reader to poll
//! ([`super::events`]): every change it records for an event source is also published onto
//! the account's DEFAULT event bus, with no opt-in and no subscription, under the `source`
//! this service owns ([`EVENT_SOURCE`]) and the `detail-type` of the SOURCE TYPE the change
//! happened to ([`published_detail_type`]). The envelope names the changed resource's ARN as
//! the single entry of `resources`, so a rule whose pattern pins `resources` — or
//! `detail.SourceArn` — to one resource is delivered that resource's events and no other's;
//! `detail` carries the published event identifier, the message, the category, the source
//! type, the source identifier, the source ARN and the date, which is what a rule's pattern
//! matches on and what its targets receive. A client driving RDS automation — restart
//! auditing, failover alerting, remediation — therefore observes a DB instance changing
//! state without polling the control plane again.
//!
//! One envelope per RECORD: [`BusEvent`] is rendered from the [`super::EventRecord`] the
//! change was written into, so the event a consumer receives and the row `DescribeEvents`
//! reports are the same record down to its `Date`, and no change can be published that the
//! log does not hold. A record exists only because a handler performed the change it
//! records, so a refused call publishes nothing.
//!
//! Publication is handed over by the ONE seam every request of this front door passes
//! through (`crate::service::query_error_envelope`), AFTER the response has been rendered
//! and this service's store released — never from a handler holding that store. A
//! publication reaches EventBridge's own handler, which matches the event against the
//! account's rules and delivers it to each of their targets, and a target may call back
//! into this service; holding the store across it would deadlock the very call the event
//! came from. EventBridge is reached the way every emulated service reaches another — the
//! AWS SDK over the framework's in-process callback, through the ONE client construction
//! [`service_common::eventbridge_client`] — so nothing here opens a socket, and the only
//! thing the address is built from is the Region the changed resource lives in, so no part
//! of a caller's request is a destination.
//!
//! Publication is BEST EFFORT: an emulator with no EventBridge mounted has nowhere to
//! publish, so a failure is traced and the operation's own answer stands.

use super::*;

/// The `source` of every event Amazon RDS publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry (`smithy_query(query="service_info")`:
/// `arnNamespace` `rds`).
pub const EVENT_SOURCE: &str = "aws.rds";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `detail-type` the events of `source_type` are published under, or `None` for a
/// source type no envelope is published for.
///
/// Amazon RDS publishes one detail-type per event SOURCE TYPE, spelling it `RDS <the source
/// type, in words> Event` — the form `RDS DB Instance Event` is the one the DB instance
/// events of this table carry. Every source type this service RECORDS a change for
/// (`db-instance`, `db-snapshot`, `db-cluster` — see [`super::events`]) is here, beside the
/// sibling resource types of the same family; a source type this emulator records nothing
/// for publishes nothing rather than an invented detail-type, which would be a rule no
/// pattern could be written against.
#[must_use]
pub fn published_detail_type(source_type: &str) -> Option<&'static str> {
    Some(match source_type {
        "db-instance" => "RDS DB Instance Event",
        "db-cluster" => "RDS DB Cluster Event",
        "db-snapshot" => "RDS DB Snapshot Event",
        "db-cluster-snapshot" => "RDS DB Cluster Snapshot Event",
        "db-parameter-group" => "RDS DB Parameter Group Event",
        "db-security-group" => "RDS DB Security Group Event",
        "db-proxy" => "RDS DB Proxy Event",
        _ => return None,
    })
}

/// The `detail.SourceType` of a published envelope: the modelled source type
/// (`smithy_query(query="shape",shape="SourceType")`, e.g. `db-instance`) as the event
/// detail spells it — upper case, with the hyphens that separate its words as underscores
/// (`DB_INSTANCE`).
#[must_use]
pub fn detail_source_type(source_type: &str) -> String {
    source_type.to_ascii_uppercase().replace('-', "_")
}

/// How many publications one store holds before the oldest is dropped.
///
/// The queue is drained by the request that filled it, so in practice it holds the one or
/// two events of a single call. The bound is what keeps it a queue rather than a leak when
/// there is nothing to drain it (a change recorded outside a served request): past this
/// many the oldest goes, exactly as the event log itself is a ring.
pub const MAX_PENDING_BUS_EVENTS: usize = 64;

/// One recorded change, rendered from the stored record and waiting to be published.
#[derive(Debug, Clone)]
#[must_use = "a recorded RDS event is published onto the default event bus"]
pub struct BusEvent {
    /// The Region the event is published in: the Region of the resource it is about, which
    /// is the Region its log belongs to.
    pub region: String,
    /// The record the envelope is rendered from — the very row `DescribeEvents` reports.
    pub record: EventRecord,
}

impl BusEvent {
    /// The envelope for `record`, recorded in `region`.
    pub fn of(region: &str, record: EventRecord) -> Self {
        Self {
            region: region.to_string(),
            record,
        }
    }

    /// The `detail-type` this event is published under, or `None` for a source type nothing
    /// is published for.
    #[must_use]
    pub fn detail_type(&self) -> Option<&'static str> {
        published_detail_type(self.record.source_type)
    }

    /// The rendered `detail` of the envelope: the members Amazon RDS carries for a DB event,
    /// every one of them read off the stored record.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "EventID": self.record.event_id,
            "Message": self.record.message,
            "EventCategories": [self.record.category],
            "SourceType": detail_source_type(self.record.source_type),
            "SourceIdentifier": self.record.source_identifier,
            "SourceArn": self.record.source_arn,
            "Date": self.date_text(),
        })
    }

    /// `detail.Date`: the instant the change was recorded, as the RFC 3339 UTC text an
    /// event's date is carried as.
    #[must_use]
    pub fn date_text(&self) -> String {
        self.record
            .date
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default()
    }

    /// The same instant in epoch milliseconds, which is what the envelope's own `time`
    /// carries.
    fn at_millis(&self) -> i64 {
        self.record.date.to_millis().unwrap_or_default()
    }

    /// Publish this event onto the account's default event bus in its own Region.
    ///
    /// Call it with NO store guard held, for the reason the module docs give. Nothing about
    /// the outcome changes what the caller of the operation that recorded the change is
    /// told.
    pub async fn publish(self, callback: &::smithy_server_runtime::EmbeddedEmulator) {
        let Some(detail_type) = self.detail_type() else {
            return;
        };
        let client =
            ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EVENT_SOURCE)
            .detail_type(detail_type)
            // The event's `time` is WHEN the change happened, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If
            // no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis(),
            ))
            // The one resource the event is about, which is what a rule filtering on
            // `resources` reads.
            .resources(&self.record.source_arn)
            .detail(self.detail().to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    source = %self.record.source_arn,
                    "an Amazon RDS event was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    source = %self.record.source_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an Amazon RDS event could not be published to the default event bus"
                );
            }
        }
    }
}

/// Publish every change this service has recorded and not published yet, in the order the
/// changes were recorded.
///
/// The ONE handover from a served call to the bus: the queue is taken out of the store under
/// its own lock and published with nothing held, so this is safe to await after the response
/// the caller gets has been rendered. An engine this service has no store in has nothing
/// recorded and so nothing to publish.
pub async fn publish_recorded_events<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard is BOUND to this block, so it is released before the first publication is
    // awaited.
    let pending = {
        let mut store = state.lock();
        store.take_pending_bus_events()
    };
    for event in pending {
        event.publish(callback).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::events::{
        DB_CLUSTER_CREATED, DB_CLUSTER_DELETED, DB_INSTANCE_CREATED, DB_INSTANCE_DELETED,
        DB_INSTANCE_RESTARTED, DB_INSTANCE_STARTED, DB_INSTANCE_STOPPED, DB_SNAPSHOT_CREATING,
        DB_SNAPSHOT_DELETED,
    };

    /// RULE: the detail's source type is the modelled one in upper case with its word
    /// separators as underscores. The rows span the modelled `SourceType` values' shapes:
    /// one word pair, three words, and a value carrying no hyphen at all.
    #[test]
    fn the_detail_source_type_is_the_modelled_one_upper_cased() {
        let rows: [(&str, &str); 5] = [
            ("db-instance", "DB_INSTANCE"),
            ("db-cluster", "DB_CLUSTER"),
            ("db-cluster-snapshot", "DB_CLUSTER_SNAPSHOT"),
            ("blue-green-deployment", "BLUE_GREEN_DEPLOYMENT"),
            ("zeroetl", "ZEROETL"),
        ];
        for (source_type, expected) in rows {
            let rendered = detail_source_type(source_type);
            assert_eq!(
                rendered, expected,
                "case {source_type:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// Every source type this service records a change for publishes a detail-type, and a
    /// source type outside the published set publishes none.
    #[test]
    fn every_recorded_source_type_has_a_published_detail_type() {
        for row in [
            DB_INSTANCE_RESTARTED,
            DB_INSTANCE_CREATED,
            DB_INSTANCE_DELETED,
            DB_INSTANCE_STARTED,
            DB_INSTANCE_STOPPED,
            DB_SNAPSHOT_CREATING,
            DB_SNAPSHOT_DELETED,
            DB_CLUSTER_CREATED,
            DB_CLUSTER_DELETED,
        ] {
            let detail_type = published_detail_type(row.source_type);
            assert!(
                detail_type.is_some_and(|text| text.starts_with("RDS ") && text.ends_with(" Event")),
                "case {:?}: a recorded source type publishes an `RDS … Event` detail-type, \
                 got {detail_type:?}",
                row.source_type
            );
        }
        assert_eq!(
            published_detail_type("db-widget"),
            None,
            "a source type nothing is published for publishes no detail-type"
        );
    }
}
