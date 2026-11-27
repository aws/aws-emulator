// SPDX-License-Identifier: CC0-1.0

//! What a cluster's BROKERS write about themselves, and WHERE those lines go.
//!
//! A cluster's `LoggingInfo` is not a value a client reads back: it is an instruction.
//! `BrokerLogs` carries the three destination kinds a cluster may be told to send its
//! brokers' Apache Kafka logs to — a CloudWatch Logs `LogGroup`, an Amazon S3
//! `Bucket`/`Prefix` and a Firehose `DeliveryStream` — and each of them is gated by its own
//! required `Enabled`. The rule this module implements: every line the cluster's brokers
//! write is delivered to EXACTLY the destinations its own `BrokerLogs` enables and names,
//! and to nothing else — so a destination that is not enabled, one that is enabled without
//! naming where to write, and a cluster carrying no `LoggingInfo` at all each receive
//! nothing rather than having the lines diverted somewhere nobody named.
//!
//! The lines themselves are the BROKER's, in Apache Kafka's own log4j layout
//! (`[%d] %p %m (%c)%n`, the `ConversionPattern` its shipped `log4j.properties` writes a
//! broker log with): the instant, the level, the message and the logger that wrote it. What
//! is written is what the brokers of this emulator really did — each node coming up when
//! the clock brings the cluster to `ACTIVE`, and the log of each partition of a topic being
//! created when a topic is registered on it — so a consumer of a destination reads a record
//! of this cluster's own history rather than filler.
//!
//! WHEN a line is written is the broker's business and WHEN it is delivered is a call's:
//! the write happens under this service's state guard (a cluster settling, a topic being
//! registered) and delivery AWAITS, so the lines are held on the cluster record until a
//! caller can hand them over ([`deliver_broker_logs`]). Every operation that reads or drives
//! a cluster delivers what is held once it has released that guard, and so does the broker
//! endpoint itself after each frame it answers — which is what gets a line written by the
//! data plane to its destination without a control-plane call. The records are taken out of
//! the store before the first hand-over, so no line is delivered twice.
//!
//! Each destination kind is written through the ONE shared path for it: the log group
//! through CloudWatch Logs' own ingest ([`::service_common::cloudwatch_logs`]) and the
//! bucket and the delivery stream through the shared vended-log writes
//! ([`::service_common::vended_logs`]). The group, the bucket and the stream are the
//! ACCOUNT's, named by a caller that created them and pointed the cluster at them, so a
//! destination that is not there loses the lines and is not created — which is what makes a
//! misconfigured destination observable. Delivery is BEST EFFORT, as every vended-logs write
//! is: the caller has already been answered, so a refused write is reported in tracing
//! rather than failing anything.

use super::*;

/// How many broker log lines ONE cluster holds waiting for delivery.
///
/// CHOSEN as a process-safety bound, not an Amazon MSK limit: the lines are held in memory
/// until a call can hand them over, and how much a client drives a cluster is the client's
/// choice. Sized well past what one bring-up and a client's topics write; past it the OLDEST
/// line is dropped, which is what a log whose consumer cannot keep up loses.
pub const MAX_HELD_BROKER_LOG_LINES: usize = 1024;

/// The most PARTITIONS of one created topic whose logs are written about.
///
/// CHOSEN for the same reason: a topic's partition count comes from the request, and one
/// line per partition would let one create size the held records. A topic with more
/// partitions than this has the first [`MAX_LOGGED_PARTITIONS`] of them reported, which is
/// what a bounded log keeps.
const MAX_LOGGED_PARTITIONS: i32 = 64;

/// The log4j level a broker's own lifecycle and log-management lines carry: Apache Kafka
/// writes them at INFO, which is the level its shipped `log4j.properties` sets the root
/// logger to.
const LEVEL_INFO: &str = "INFO";

/// The logger a broker's own lifecycle lines carry.
const LOGGER_KAFKA_SERVER: &str = "kafka.server.KafkaServer";

/// The logger the broker's network layer writes under.
const LOGGER_SOCKET_SERVER: &str = "kafka.network.SocketServer";

/// The logger the broker's log subsystem writes a partition's log under.
const LOGGER_LOG_MANAGER: &str = "kafka.log.LogManager";

/// Where an Apache Kafka broker keeps its partition logs (`log.dirs`), which a line about a
/// partition's log names: Amazon MSK's brokers run with the data volume mounted there.
const BROKER_LOG_DIR: &str = "/var/lib/kafka/data";

/// The destinations one cluster's `BrokerLogs` ENABLES and NAMES, resolved off the stored
/// record so the write and the `LoggingInfo` a reader reports cannot disagree.
///
/// A destination appears here only when its own `Enabled` is true AND it names where to
/// write: "whether broker logs get sent to the specified CloudWatch Logs destination" is
/// what decides it, and a destination enabled without a group, bucket or stream names no
/// destination at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrokerLogDestinations {
    /// The CloudWatch Logs log group the lines are ingested into.
    pub log_group: Option<String>,
    /// The Amazon S3 bucket the lines are written as an object in, and the key prefix the
    /// object sits under where the destination names one.
    pub bucket: Option<String>,
    /// See [`Self::bucket`].
    pub prefix: Option<String>,
    /// The Firehose delivery stream the lines are put on.
    pub delivery_stream: Option<String>,
}

impl BrokerLogDestinations {
    /// Whether this names any destination at all: a cluster whose `BrokerLogs` enables none
    /// writes nothing, so nothing is held for it either.
    #[must_use]
    pub fn none_named(&self) -> bool {
        self.log_group.is_none()
            && self.bucket.is_none()
            && self.delivery_stream.is_none()
    }
}

/// One line a broker wrote, with the broker that wrote it and the instant it reports.
#[derive(Debug, Clone)]
pub struct BrokerLogLine {
    /// WHICH broker node of the cluster wrote it, which is what tells the lines of one
    /// broker apart from another's in the destination.
    pub broker_id: i32,
    /// The instant the line reports, which is the instant it is ingested at.
    pub at: SystemTime,
    /// The line itself, in Apache Kafka's log4j layout.
    pub line: String,
}

/// Everything ONE delivery hands over: the lines a cluster's brokers wrote, and the
/// destinations that cluster's own `BrokerLogs` names for them.
#[derive(Debug, Clone)]
pub struct BrokerLogDelivery {
    /// The Region the destinations live in: the cluster's own.
    pub region: String,
    /// The cluster's name, which the log stream and the object key are named after.
    pub cluster_name: String,
    /// The opaque cluster id its endpoints sit under, which goes in the same two names so no
    /// two clusters of one name share a stream or an object.
    pub cluster_id: String,
    /// WHICH hand-over of this cluster's lines this is, counted from one: it is what keeps
    /// the object one delivery writes apart from the next's, which two deliveries falling in
    /// the same millisecond would otherwise share.
    pub ordinal: u64,
    /// Where the lines go.
    pub destinations: BrokerLogDestinations,
    /// The lines, oldest first.
    pub lines: Vec<BrokerLogLine>,
}

/// One instant as a broker log line writes it: log4j's `%d` default,
/// `yyyy-MM-dd HH:mm:ss,SSS` in UTC.
#[must_use]
pub fn log4j_instant(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format("%Y-%m-%d %H:%M:%S,%3f")
        .to_string()
}

/// One broker log line: the instant, the level, the message and the logger that wrote it —
/// Apache Kafka's own layout, so a consumer reads what it reads off a broker's log file.
#[must_use]
pub fn broker_log_line(at: SystemTime, level: &str, message: &str, logger: &str) -> String {
    format!("[{}] {level} {message} ({logger})", log4j_instant(at))
}

impl Cluster {
    /// The destinations this cluster's `BrokerLogs` enables and names.
    #[must_use]
    pub fn broker_log_destinations(&self) -> BrokerLogDestinations {
        let Some(logs) = self.logging_info.as_ref().map(|info| &info.broker_logs) else {
            return BrokerLogDestinations::default();
        };
        BrokerLogDestinations {
            log_group: logs
                .cloud_watch_logs
                .as_ref()
                .filter(|destination| destination.enabled)
                .and_then(|destination| named(destination.log_group.as_deref())),
            bucket: logs
                .s3
                .as_ref()
                .filter(|destination| destination.enabled)
                .and_then(|destination| named(destination.bucket.as_deref())),
            prefix: logs
                .s3
                .as_ref()
                .filter(|destination| destination.enabled)
                .and_then(|destination| named(destination.prefix.as_deref())),
            delivery_stream: logs
                .firehose
                .as_ref()
                .filter(|destination| destination.enabled)
                .and_then(|destination| named(destination.delivery_stream.as_deref())),
        }
    }

    /// Whether this cluster's `BrokerLogs` names anywhere to deliver a line to at all.
    ///
    /// The ONE reading of that question is [`Self::broker_log_destinations`], so a
    /// destination kind added there is admitted here too rather than silently dropped by a
    /// second copy of the predicate.
    #[must_use]
    pub fn writes_broker_logs(&self) -> bool {
        !self.broker_log_destinations().none_named()
    }

    /// Hold `line`, written by broker `broker_id` at `at`, for delivery — or drop it where
    /// this cluster's `BrokerLogs` names no destination to deliver it to.
    ///
    /// A line is held only for a cluster that HAS somewhere to send it, so a cluster whose
    /// logging is off holds nothing at all; past [`MAX_HELD_BROKER_LOG_LINES`] the oldest
    /// held line goes first.
    pub(super) fn write_broker_log(&mut self, broker_id: i32, at: SystemTime, line: String) {
        if !self.writes_broker_logs() {
            return;
        }
        self.held_broker_logs.push_back(BrokerLogLine {
            broker_id,
            at,
            line,
        });
        while self.held_broker_logs.len() > MAX_HELD_BROKER_LOG_LINES {
            self.held_broker_logs.pop_front();
        }
    }

    /// The lines each of this cluster's brokers writes as it COMES UP, which the clock owes
    /// the cluster when it reaches `ACTIVE`: Apache Kafka's own startup lines, one set per
    /// broker node of the cluster in node-id order.
    ///
    /// Written once per cluster — a cluster that is already up is not starting again — and
    /// at the instant the transition was due rather than when anybody looked, so the lines
    /// report when the brokers really came up.
    pub(super) fn write_startup_broker_logs(&mut self, at: SystemTime) {
        if self.brokers_started {
            return;
        }
        self.brokers_started = true;
        let nodes: Vec<i32> = self.brokers.iter().map(|broker| broker.id).collect();
        for id in nodes {
            for line in [
                broker_log_line(
                    at,
                    LEVEL_INFO,
                    &format!("[KafkaServer id={id}] starting"),
                    LOGGER_KAFKA_SERVER,
                ),
                broker_log_line(
                    at,
                    LEVEL_INFO,
                    &format!(
                        "[SocketServer brokerId={id}] Started socket server acceptors and \
                         processors"
                    ),
                    LOGGER_SOCKET_SERVER,
                ),
                broker_log_line(
                    at,
                    LEVEL_INFO,
                    &format!("[KafkaServer id={id}] started"),
                    LOGGER_KAFKA_SERVER,
                ),
            ] {
                self.write_broker_log(id, at, line);
            }
        }
    }

    /// The lines the broker holding a new topic's partitions writes when that topic is
    /// created on this cluster: one per partition log it created, as Apache Kafka's log
    /// subsystem reports each.
    ///
    /// The partitions of a topic here are served by the cluster's first broker node, which
    /// is the node every access method's socket answers as, so that is the broker the lines
    /// are attributed to.
    pub(super) fn write_topic_broker_logs(
        &mut self,
        topic: &str,
        partitions: i32,
        at: SystemTime,
    ) {
        let broker_id = self
            .brokers
            .first()
            .map_or(broker_plane::BROKER_NODE_ID, |broker| broker.id);
        for index in 0..partitions.clamp(0, MAX_LOGGED_PARTITIONS) {
            let line = broker_log_line(
                at,
                LEVEL_INFO,
                &format!(
                    "Created log for partition {topic}-{index} in {BROKER_LOG_DIR} with \
                     properties {{}}"
                ),
                LOGGER_LOG_MANAGER,
            );
            self.write_broker_log(broker_id, at, line);
        }
    }
}

/// `value` where it names something, and `None` for a destination that names nothing: an
/// absent member and an empty one are both a destination with nowhere to write.
fn named(value: Option<&str>) -> Option<String> {
    value
        .filter(|named| !named.is_empty())
        .map(str::to_string)
}

impl ComAmazonawsKafkaKafkaState {
    /// Take the broker log lines the cluster `cluster_arn` is holding, with the destinations
    /// its own `BrokerLogs` names for them — after materialising the lines the clock already
    /// owes it.
    ///
    /// Scoped to the ONE cluster being driven, as every other read of this registry is: no
    /// other cluster's log is this call's business. `None` for a cluster this Region holds
    /// no record of, for one that has written nothing, and for one whose `BrokerLogs` names
    /// no destination — in each case there is nothing to hand over.
    pub fn take_broker_logs(
        &mut self,
        region: &str,
        cluster_arn: &str,
        now: SystemTime,
    ) -> Option<BrokerLogDelivery> {
        let cluster = self.settled_cluster_in_mut(region, cluster_arn, now)?;
        let destinations = cluster.broker_log_destinations();
        if destinations.none_named() {
            return None;
        }
        let lines: Vec<BrokerLogLine> = cluster.held_broker_logs.drain(..).collect();
        if lines.is_empty() {
            return None;
        }
        cluster.broker_log_deliveries = cluster.broker_log_deliveries.saturating_add(1);
        Some(BrokerLogDelivery {
            region: cluster.region.clone(),
            cluster_name: cluster.name.clone(),
            cluster_id: cluster_log_id(&cluster.dns_suffix),
            ordinal: cluster.broker_log_deliveries,
            destinations,
            lines,
        })
    }
}

/// The opaque cluster id a stream and an object key carry: the first label of the cluster's
/// endpoint suffix, which is the id Amazon MSK gives every endpoint of that cluster.
#[must_use]
fn cluster_log_id(dns_suffix: &str) -> String {
    dns_suffix
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Deliver every broker log line the cluster `cluster_arn` is holding, from an operation.
///
/// Call it at the END of a handler, with NO state guard held: this awaits, and the lines are
/// taken out of the store before the first hand-over so a concurrent call delivers none of
/// them twice.
pub async fn deliver_broker_logs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_arn: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    deliver_broker_logs_from(&state, ctx.callback(), region, cluster_arn).await;
}

/// [`deliver_broker_logs`] for a deliverer that holds this service's store and the
/// in-process transport ITSELF rather than a request's context: the cluster's own broker
/// endpoint, which is served on a core-owned socket and has no `OperationContext` at all.
///
/// The same delivery, so a line written by the data plane and one written by the control
/// plane cannot land differently.
pub async fn deliver_broker_logs_from(
    state: &::engine::ServiceState<ComAmazonawsKafkaKafkaState>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    cluster_arn: &str,
) {
    let held = state
        .lock()
        .take_broker_logs(region, cluster_arn, SystemTime::now());
    let Some(delivery) = held else {
        return;
    };
    delivered(callback, &delivery).await;
}

/// Hand `delivery`'s lines to each destination it names, through the one shared write per
/// destination kind.
async fn delivered(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    delivery: &BrokerLogDelivery,
) {
    if let Some(log_group) = delivery.destinations.log_group.as_deref() {
        // ONE STREAM PER BROKER: the lines of a broker are that broker's, so a group a
        // client pointed several clusters at holds each broker's log on its own without two
        // of them sharing a stream. Each line is ingested at the instant it REPORTS, so a
        // consumer of the group reads it in the period the broker wrote it in rather than in
        // the period it happened to be handed over in.
        for broker_id in broker_ids(delivery) {
            let events: Vec<(i64, String)> = delivery
                .lines
                .iter()
                .filter(|line| line.broker_id == broker_id)
                .map(|line| (broker_plane::epoch_millis(line.at), line.line.clone()))
                .collect();
            ::service_common::cloudwatch_logs::delivered_stamped_into_held_group_over(
                callback,
                &delivery.region,
                log_group,
                &broker_log_stream(delivery, broker_id),
                &events,
            )
            .await;
        }
    }
    let lines: Vec<String> = delivery
        .lines
        .iter()
        .map(|line| line.line.clone())
        .collect();
    if let Some(bucket) = delivery.destinations.bucket.as_deref() {
        ::service_common::vended_logs::delivered_object_over(
            callback,
            &delivery.region,
            bucket,
            &broker_log_object_key(delivery),
            &lines,
        )
        .await;
    }
    if let Some(delivery_stream) = delivery.destinations.delivery_stream.as_deref() {
        ::service_common::vended_logs::delivered_to_stream_over(
            callback,
            &delivery.region,
            delivery_stream,
            &lines,
        )
        .await;
    }
}

/// The broker nodes `delivery` carries lines for, in node-id order and each once.
fn broker_ids(delivery: &BrokerLogDelivery) -> Vec<i32> {
    let mut ids: Vec<i32> = delivery
        .lines
        .iter()
        .map(|line| line.broker_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The log stream one broker's lines are ingested into.
///
/// CHOSEN, like every other identity this emulator mints for a destination it writes into:
/// it names the CLUSTER and the BROKER whose lines it holds, so one log group holds the logs
/// of as many clusters and brokers as a client points at it and no two of them share a
/// stream — including two clusters that carried the same name, which the cluster id
/// separates.
#[must_use]
fn broker_log_stream(delivery: &BrokerLogDelivery, broker_id: i32) -> String {
    format!(
        "{}-{}-broker-{broker_id}",
        delivery.cluster_name, delivery.cluster_id
    )
}

/// The key of the object one delivery writes, under the destination's own prefix where it
/// names one: the `AWSLogs/<account-id>/<service segment>/<region>/` layout every delivered
/// log file in this tree sits under ([`::service_common::trail_log_files`] states the same
/// reading for a trail's files), this service's own segment, the cluster the lines belong
/// to, the day they were written and a name no two deliveries share.
#[must_use]
fn broker_log_object_key(delivery: &BrokerLogDelivery) -> String {
    let at = delivery
        .lines
        .first()
        .map_or_else(SystemTime::now, |line| line.at);
    let written = ::chrono::DateTime::<::chrono::Utc>::from(at);
    let prefix = match delivery.destinations.prefix.as_deref() {
        Some(prefix) => format!("{}/", prefix.trim_end_matches('/')),
        None => String::new(),
    };
    format!(
        "{prefix}{DELIVERED_LOGS_KEY_SEGMENT}/{}/{BROKER_LOGS_KEY_SEGMENT}/{}/{}-{}/{}-{}.log.gz",
        ::engine::emulator_account_id(),
        delivery.region,
        delivery.cluster_name,
        delivery.cluster_id,
        written.format("%Y-%m-%d-%H-%M-%S"),
        delivery.ordinal,
    )
}

/// The root segment every delivered log file of an AWS service sits under in a bucket, which
/// this delivery writes under too ("AWSLogs as a subfolder name", as the destinations of the
/// sibling vended-log deliveries in this tree state it).
const DELIVERED_LOGS_KEY_SEGMENT: &str = "AWSLogs";

/// This delivery's own segment under that root: the broker logs of an Amazon MSK cluster.
const BROKER_LOGS_KEY_SEGMENT: &str = "KafkaBrokerLogs";


#[cfg(test)]
mod tests {
    use crate::ops::types::{BrokerLogs, CloudWatchLogs, Firehose, S3};

    use super::*;

    /// An instant with a known UTC rendering: 2026-11-20T09:27:10.500Z.
    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_795_166_830_500)
    }

    /// RULE: a broker log line is Apache Kafka's own log4j layout — the instant, the level,
    /// the message and the logger — so a consumer reads what a broker's log file holds.
    #[test]
    fn a_broker_log_line_carries_the_level_and_the_logger_that_wrote_it() {
        // The rows span the loggers this module writes under and the messages each carries.
        let rows: &[(&str, &str, &str)] = &[
            (
                "a broker coming up",
                "[KafkaServer id=1] started",
                LOGGER_KAFKA_SERVER,
            ),
            (
                "the network layer",
                "[SocketServer brokerId=2] Started socket server acceptors and processors",
                LOGGER_SOCKET_SERVER,
            ),
            (
                "a partition's log",
                "Created log for partition orders-0 in /var/lib/kafka/data with properties {}",
                LOGGER_LOG_MANAGER,
            ),
        ];
        for (case, message, logger) in rows {
            let line = broker_log_line(at(), LEVEL_INFO, message, logger);
            assert_eq!(
                line,
                format!("[2026-11-20 09:27:10,500] INFO {message} ({logger})"),
                "case {case:?}: expected Apache Kafka's log4j layout, got {line:?}"
            );
            assert!(
                line.contains(LEVEL_INFO) && line.contains("kafka."),
                "case {case:?}: a broker log line carries a level token and a kafka logger \
                 name, got {line:?}"
            );
        }
    }

    /// RULE: a destination is a destination only when its own `Enabled` is true AND it names
    /// where to write, so every other combination names nothing — for every destination kind
    /// `BrokerLogs` carries.
    #[test]
    fn only_an_enabled_destination_that_names_a_place_is_one() {
        // The rows span the three destination KINDS crossed with the combinations of the
        // required `Enabled` and the member that says where to write.
        let places: &[(&str, Option<&str>, Option<&str>)] = &[
            ("a named place", Some("wired"), Some("wired")),
            ("an empty name", Some(""), None),
            ("nothing named", None, None),
        ];
        let kinds: &[(&str, Kind)] = &[
            ("the log group", Kind::CloudWatchLogs),
            ("the bucket", Kind::S3),
            ("the delivery stream", Kind::Firehose),
        ];
        for (kind_case, kind) in kinds {
            for (case, named, expected) in places {
                let case = format!("{kind_case} with {case}");
                let destinations = clustered(*kind, true, *named).broker_log_destinations();
                assert_eq!(
                    kind.named_in(&destinations).as_deref(),
                    *expected,
                    "case {case:?}: expected the destination {expected:?}, got {destinations:?}"
                );
                // And the same destination with `Enabled` false names nothing, whatever it
                // says, so nothing of this kind is ever written for it.
                let disabled = clustered(*kind, false, *named).broker_log_destinations();
                assert!(
                    kind.named_in(&disabled).is_none() && disabled.none_named(),
                    "case {case:?}: a destination that is not enabled names nothing, got \
                     {disabled:?}"
                );
            }
        }
        // A cluster carrying no `LoggingInfo` at all names nothing either.
        let mut bare = clustered(Kind::CloudWatchLogs, true, Some("wired"));
        bare.logging_info = None;
        assert!(
            bare.broker_log_destinations().none_named() && !bare.writes_broker_logs(),
            "a cluster with no LoggingInfo names no destination"
        );
        // ... and holds no line, so nothing is kept for a cluster nothing would be
        // delivered for.
        bare.write_broker_log(1, at(), "a line nobody asked for".to_string());
        assert!(
            bare.held_broker_logs.is_empty(),
            "a cluster with nowhere to deliver holds no line, got {:?}",
            bare.held_broker_logs
        );
    }

    /// Which destination kind of `BrokerLogs` a row is about.
    #[derive(Debug, Clone, Copy)]
    enum Kind {
        CloudWatchLogs,
        S3,
        Firehose,
    }

    impl Kind {
        /// What `destinations` names for this kind.
        fn named_in(self, destinations: &BrokerLogDestinations) -> Option<String> {
            match self {
                Self::CloudWatchLogs => destinations.log_group.clone(),
                Self::S3 => destinations.bucket.clone(),
                Self::Firehose => destinations.delivery_stream.clone(),
            }
        }
    }

    /// A provisioned cluster whose `BrokerLogs` carries exactly the destination `kind`, with
    /// that destination's own `Enabled` and the member that says where it writes.
    fn clustered(kind: Kind, enabled: bool, named: Option<&str>) -> Cluster {
        let named = named.map(str::to_string);
        let mut cluster = Cluster::provisioned(
            "us-west-2",
            0,
            at(),
            CreateClusterInput {
                cluster_name: "broker-logs".to_string(),
                kafka_version: "3.9.x".to_string(),
                number_of_broker_nodes: 2,
                broker_node_group_info: BrokerNodeGroupInfo {
                    instance_type: "kafka.m5.large".to_string(),
                    client_subnets: vec!["subnet-a".to_string(), "subnet-b".to_string()],
                    ..BrokerNodeGroupInfo::default()
                },
                ..CreateClusterInput::default()
            },
        );
        let broker_logs = match kind {
            Kind::CloudWatchLogs => BrokerLogs {
                cloud_watch_logs: Some(CloudWatchLogs {
                    enabled,
                    log_group: named,
                }),
                firehose: None,
                s3: None,
            },
            Kind::S3 => BrokerLogs {
                cloud_watch_logs: None,
                firehose: None,
                s3: Some(S3 {
                    enabled,
                    bucket: named,
                    prefix: Some("broker-logs".to_string()),
                }),
            },
            Kind::Firehose => BrokerLogs {
                cloud_watch_logs: None,
                firehose: Some(Firehose {
                    enabled,
                    delivery_stream: named,
                }),
                s3: None,
            },
        };
        cluster.logging_info = Some(LoggingInfo {
            authorizer_logs: None,
            broker_logs,
        });
        cluster
    }
}
