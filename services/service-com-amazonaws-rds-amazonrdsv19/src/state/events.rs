// SPDX-License-Identifier: CC0-1.0

//! The event log `DescribeEvents` reports.
//!
//! Amazon RDS records an event for each change it makes to a resource that is an event
//! source — the create that provisions it, the delete it accepts, the restart or
//! stop/start it performs — and reports those records through `DescribeEvents`. What this
//! module holds is that log for THIS service: one record per lifecycle change the service
//! actually performed, timestamped when it performed it. A record appears only because a
//! handler did the thing it records, so nothing here is invented: an account that created
//! nothing has no events, exactly as a fresh AWS account does.
//!
//! The log is per REGION, like every other store of this service: an event belongs to the
//! region of the resource it is about, so a `DescribeEvents` signed for one region never
//! reports another's. It is bounded on both factors — [`MAX_EVENTS_PER_REGION`] records
//! per region, and [`super::MAX_REGIONS`] regions, which
//! [`super::ComAmazonawsRdsAmazonrdsv19State::has_room_for_region`] counts this map into —
//! and each record carries only values this service already bounds (an identifier, an ARN
//! it computed, and one of the fixed messages below).
//!
//! An event also has a published lifetime: `DescribeEvents` returns events "for the past
//! 14 days", so a record past [`EVENT_RETENTION_SECONDS`] is dropped by
//! [`EventLog::prune`], which runs where every other change the clock has already made due
//! is materialised ([`super::ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`]).

use crate::ops::types::Event;

/// How long a recorded event stays readable: `DescribeEvents` "returns events related to
/// DB instances, DB clusters, DB parameter groups, DB security groups, DB snapshots, DB
/// cluster snapshots, and RDS Proxies for the past 14 days", in seconds.
pub const EVENT_RETENTION_SECONDS: f64 = 14.0 * 24.0 * 60.0 * 60.0;

/// The window `DescribeEvents` reports when the request bounds none: "By default, RDS
/// returns events that were generated in the past hour", which is also
/// `DescribeEventsMessage$Duration`'s "The number of minutes to retrieve events for.
/// Default: 60".
pub const DEFAULT_EVENT_WINDOW_MINUTES: i64 = 60;

/// How many events one region's log keeps.
///
/// AWS publishes no event-count quota — it publishes a retention window instead — and a
/// caller can drive lifecycle changes without limit, so the log is a ring like a DB
/// instance's own log ([`super::engine_log::MAX_LOG_ENTRIES`]): past this many records the
/// oldest goes. Generous next to the per-region resource quotas this service enforces, so
/// a region's whole resource lifecycle is readable.
pub const MAX_EVENTS_PER_REGION: usize = 512;

/// The source types `DescribeEventsMessage$SourceType` accepts, from the model's
/// `SourceType` enum (`smithy_query(query="shape",shape="SourceType")`), in the order it
/// declares them.
///
/// WIDER than the source types [`super::EVENT_CATEGORIES`] publishes a category map for
/// and than the ones a subscription may name ([`super::EVENT_SUBSCRIPTION_SOURCE_TYPES`]):
/// this is the set a READ may narrow by, and a value outside it names no source at all.
pub const DESCRIBE_EVENTS_SOURCE_TYPES: [&str; 11] = [
    "db-instance",
    "db-parameter-group",
    "db-security-group",
    "db-snapshot",
    "db-cluster",
    "db-cluster-snapshot",
    "custom-engine-version",
    "db-proxy",
    "blue-green-deployment",
    "db-shard-group",
    "zero-etl",
];

/// Refuse a `SourceType` that is not one of the modelled event source types, naming the
/// value rejected and the set it is not in — the wording this service already refuses an
/// unpublished event source type with (`DescribeEventCategories`).
pub fn check_event_source_type(source_type: &str) -> Result<(), String> {
    if DESCRIBE_EVENTS_SOURCE_TYPES.contains(&source_type) {
        return Ok(());
    }
    Err(format!(
        "Invalid event source type: {source_type}. Valid values are: {}",
        DESCRIBE_EVENTS_SOURCE_TYPES.join(", ")
    ))
}

/// One row of the published event table: the source type the change happens to, the
/// identifier RDS publishes that change under, the category RDS reports it under, and the
/// message it reports.
///
/// Source: the per-source-type event tables of "Amazon RDS event categories and event
/// messages" (Amazon RDS User Guide), each row of which states exactly these four fields.
/// Only the rows this service can actually PERFORM are here — a handler records a row when
/// it makes that change — and every row's category is one its source type publishes in
/// [`super::EVENT_CATEGORIES`], which [`categories_are_published`] holds to.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleEvent {
    /// The `Event$SourceType` the record carries.
    pub source_type: &'static str,
    /// The event identifier of this row in the published table (`RDS-EVENT-nnnn`), which
    /// is what the `detail.EventID` of the published envelope carries
    /// ([`super::bus_events`]). It is NOT a member of the modelled `Event` shape
    /// (`smithy_query(query="shape",shape="Event")` declares `SourceIdentifier`,
    /// `SourceType`, `Message`, `EventCategories`, `Date` and `SourceArn` and no id), so
    /// nothing in `DescribeEvents` reports it.
    pub event_id: &'static str,
    /// The `Event$EventCategories` entry the record carries.
    pub category: &'static str,
    /// The `Event$Message` the record carries.
    pub message: &'static str,
}

/// A DB instance was created (`CreateDBInstance`).
pub const DB_INSTANCE_CREATED: LifecycleEvent = LifecycleEvent {
    source_type: "db-instance",
    event_id: "RDS-EVENT-0005",
    category: "creation",
    message: "DB instance created",
};

/// A DB instance was deleted (`DeleteDBInstance`).
pub const DB_INSTANCE_DELETED: LifecycleEvent = LifecycleEvent {
    source_type: "db-instance",
    event_id: "RDS-EVENT-0003",
    category: "deletion",
    message: "DB instance deleted",
};

/// A DB instance was restarted (`RebootDBInstance`).
pub const DB_INSTANCE_RESTARTED: LifecycleEvent = LifecycleEvent {
    source_type: "db-instance",
    event_id: "RDS-EVENT-0006",
    category: "availability",
    message: "DB instance restarted",
};

/// A stopped DB instance was started again (`StartDBInstance`).
pub const DB_INSTANCE_STARTED: LifecycleEvent = LifecycleEvent {
    source_type: "db-instance",
    event_id: "RDS-EVENT-0088",
    category: "notification",
    message: "DB instance started",
};

/// A DB instance was stopped (`StopDBInstance`).
pub const DB_INSTANCE_STOPPED: LifecycleEvent = LifecycleEvent {
    source_type: "db-instance",
    event_id: "RDS-EVENT-0087",
    category: "notification",
    message: "DB instance stopped",
};

/// A manual DB snapshot is being taken: `CreateDBSnapshot`, and the final snapshot a
/// delete or a stop takes of its instance. The event is recorded when the snapshot starts,
/// which is the status the snapshot itself reports.
pub const DB_SNAPSHOT_CREATING: LifecycleEvent = LifecycleEvent {
    source_type: "db-snapshot",
    event_id: "RDS-EVENT-0040",
    category: "creation",
    message: "Creating manual snapshot",
};

/// A manual DB snapshot was deleted (`DeleteDBSnapshot`), which is the published table's
/// deletion row for this source type.
pub const DB_SNAPSHOT_DELETED: LifecycleEvent = LifecycleEvent {
    source_type: "db-snapshot",
    event_id: "RDS-EVENT-0041",
    category: "deletion",
    message: "Manual snapshot deleted",
};

/// A DB cluster was created (`CreateDBCluster`).
pub const DB_CLUSTER_CREATED: LifecycleEvent = LifecycleEvent {
    source_type: "db-cluster",
    event_id: "RDS-EVENT-0170",
    category: "creation",
    message: "DB cluster created",
};

/// A DB cluster was deleted (`DeleteDBCluster`).
pub const DB_CLUSTER_DELETED: LifecycleEvent = LifecycleEvent {
    source_type: "db-cluster",
    event_id: "RDS-EVENT-0171",
    category: "deletion",
    message: "DB cluster deleted",
};

/// Whether every row above reports a category its source type publishes, so a client that
/// built an `EventCategories` argument out of `DescribeEventCategories` can narrow a read
/// to the events this service records. Held to by this module's own test.
#[cfg(test)]
fn categories_are_published(rows: &[LifecycleEvent]) -> bool {
    rows.iter().all(|row| {
        super::event_categories_of(row.source_type)
            .is_some_and(|published| published.contains(&row.category))
    })
}

/// One recorded event: which resource changed, what the change was, and when this service
/// made it.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// The position of this record in its region's log, which is also the `Marker` a page
    /// boundary at this record resumes from.
    pub sequence: u64,
    /// `Event$SourceIdentifier`: the identifier of the resource the event is about.
    pub source_identifier: String,
    /// `Event$SourceType`.
    pub source_type: &'static str,
    /// The event identifier of the published table row this record is, which the published
    /// envelope carries as `detail.EventID` ([`LifecycleEvent::event_id`]).
    pub event_id: &'static str,
    /// The one `Event$EventCategories` entry this record carries.
    pub category: &'static str,
    /// `Event$Message`.
    pub message: String,
    /// `Event$SourceArn`: the ARN of the resource the event is about, as the resource's
    /// own record computes it.
    pub source_arn: String,
    /// `Event$Date`: when this service recorded the change.
    pub date: ::aws_smithy_types::DateTime,
}

impl EventRecord {
    /// The pagination key of this record: its sequence, zero-padded so the keys sort the
    /// way the log is ordered.
    pub fn marker(&self) -> String {
        format!("{:020}", self.sequence)
    }

    /// Whether this record was written in `[start, end]`, the window a read asks for.
    pub fn within(&self, start: f64, end: f64) -> bool {
        let at = self.date.as_secs_f64();
        at >= start && at <= end
    }

    /// Project the stored record onto the modelled `Event` output shape.
    pub fn to_wire(&self) -> Event {
        Event {
            source_identifier: Some(self.source_identifier.clone()),
            source_type: Some(self.source_type.to_string()),
            message: Some(self.message.clone()),
            event_categories: Some(vec![self.category.to_string()]),
            date: Some(self.date),
            source_arn: Some(self.source_arn.clone()),
        }
    }
}

/// One region's event log: the lifecycle records this service has written there, oldest
/// first — the order `DescribeEvents` reports and pages them in.
#[derive(Debug, Default)]
pub struct EventLog {
    /// The records, oldest first, at most [`MAX_EVENTS_PER_REGION`] of them.
    pub entries: Vec<EventRecord>,
    /// The sequence the next record takes. Monotone and never reused, so a `Marker` a page
    /// issued cannot come to name a different record.
    next_sequence: u64,
}

impl EventLog {
    /// Record `event` for the resource `source_identifier` / `source_arn`, timestamped
    /// now, dropping the oldest record when the log is full, and hand back the record that
    /// was written.
    ///
    /// The record is handed back so the PUBLICATION of the same change is rendered from it
    /// ([`super::bus_events`]): the envelope on the account's default event bus and the row
    /// `DescribeEvents` reports are then one record, down to its `Date`.
    pub fn record(
        &mut self,
        event: LifecycleEvent,
        source_identifier: &str,
        source_arn: String,
    ) -> EventRecord {
        self.next_sequence += 1;
        let record = EventRecord {
            sequence: self.next_sequence,
            source_identifier: source_identifier.to_string(),
            source_type: event.source_type,
            event_id: event.event_id,
            category: event.category,
            message: event.message.to_string(),
            source_arn,
            date: super::now_as_wire_timestamp(),
        };
        self.entries.push(record.clone());
        if self.entries.len() > MAX_EVENTS_PER_REGION {
            self.entries.remove(0);
        }
        record
    }

    /// Drop the records past the published retention window, `now_seconds` being the
    /// current instant in seconds since the epoch.
    ///
    /// An expiry the clock has ALREADY made due: a record older than
    /// [`EVENT_RETENTION_SECONDS`] is one `DescribeEvents` does not report whether or not
    /// anybody called, so materialising it changes nothing a later caller could see.
    pub fn prune(&mut self, now_seconds: f64) {
        self.entries
            .retain(|record| now_seconds - record.date.as_secs_f64() <= EVENT_RETENTION_SECONDS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row of the table above reports a category its source type publishes.
    #[test]
    fn every_recorded_event_carries_a_published_category() {
        let rows = [
            DB_INSTANCE_CREATED,
            DB_INSTANCE_DELETED,
            DB_INSTANCE_RESTARTED,
            DB_INSTANCE_STARTED,
            DB_INSTANCE_STOPPED,
            DB_SNAPSHOT_CREATING,
            DB_SNAPSHOT_DELETED,
            DB_CLUSTER_CREATED,
            DB_CLUSTER_DELETED,
        ];
        assert!(
            categories_are_published(&rows),
            "every recorded event's category must be one its source type publishes, got {rows:?}"
        );
    }

    /// Every row carries the published form of an RDS event identifier, and no two rows
    /// share one: the identifier is what a consumer's rule matches `detail.EventID` on, so
    /// two different changes publishing the same id would be indistinguishable.
    #[test]
    fn every_recorded_event_carries_its_own_published_identifier() {
        let rows = [
            DB_INSTANCE_CREATED,
            DB_INSTANCE_DELETED,
            DB_INSTANCE_RESTARTED,
            DB_INSTANCE_STARTED,
            DB_INSTANCE_STOPPED,
            DB_SNAPSHOT_CREATING,
            DB_SNAPSHOT_DELETED,
            DB_CLUSTER_CREATED,
            DB_CLUSTER_DELETED,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for row in rows {
            let digits = row.event_id.strip_prefix("RDS-EVENT-").unwrap_or_default();
            assert!(
                digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit()),
                "case {:?}: an event identifier is RDS-EVENT-nnnn, got {:?}",
                row.message,
                row.event_id
            );
            assert!(
                !seen.contains(&row.event_id),
                "case {:?}: the identifier {:?} is already published for another change",
                row.message,
                row.event_id
            );
            seen.push(row.event_id);
        }
    }

    /// Every source type the table records for is one a read may narrow by.
    #[test]
    fn every_recorded_source_type_is_modelled() {
        for row in [DB_INSTANCE_CREATED, DB_SNAPSHOT_CREATING, DB_CLUSTER_CREATED] {
            assert!(
                check_event_source_type(row.source_type).is_ok(),
                "source type {:?} must be one DescribeEvents accepts",
                row.source_type
            );
        }
    }
}
