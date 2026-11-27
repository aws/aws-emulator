// SPDX-License-Identifier: CC0-1.0

//! The topic reassignment operations a cluster holds: the record
//! `StartTopicReassignment` files, the clock-owed statuses it walks, the progress
//! derived from those instants, and the projections its readers report.
//!
//! A reassignment is a sub-resource of exactly one cluster — the model addresses
//! it as `/v1/clusters/{ClusterArn}/topic-reassignments/{TopicReassignmentOperationArn}`
//! — so it is resolved only through that cluster and disappears with it, the way a
//! cluster operation does.
//!
//! Its STATUS and its PROGRESS are pure functions of the instants the record holds
//! and the reading instant: nothing is slept on, and nothing a reader does moves
//! the operation along. The topics and partitions it counts are the cluster's own,
//! selected when the operation was filed by the patterns the request named
//! ([`TopicFilter`], the same selection a replication flow's topic patterns go
//! through).
//!
//! Its own file so the parent state module keeps to the cluster registry.

use super::*;

/// How long after `StartTopicReassignment` the operation leaves `PENDING` and the
/// partitions start moving.
///
/// CHOSEN, not documented: the real service queues the reassignment before its
/// brokers begin. It is the span every clock-owed transition of this service takes
/// ([`CLUSTER_ACTIVATION_DELAY`]), so a caller that reads the operation
/// immediately still observes `PENDING`.
pub const TOPIC_REASSIGNMENT_START_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// How long the partitions take to move once the operation is `IN_PROGRESS`:
/// the window its reported progress is spread over, chosen as
/// [`TOPIC_REASSIGNMENT_START_DELAY`] is.
pub const TOPIC_REASSIGNMENT_MIGRATION_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// How long after `StopTopicReassignment` the operation finishes stopping,
/// chosen as the spans above are.
pub const TOPIC_REASSIGNMENT_STOP_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// The `TopicReassignmentStatus` values, as the model's enum spells them.
pub const TOPIC_REASSIGNMENT_STATUS_PENDING: &str = "PENDING";
/// See [`TOPIC_REASSIGNMENT_STATUS_PENDING`].
pub const TOPIC_REASSIGNMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`TOPIC_REASSIGNMENT_STATUS_PENDING`].
pub const TOPIC_REASSIGNMENT_STATUS_STOPPING: &str = "STOPPING";
/// See [`TOPIC_REASSIGNMENT_STATUS_PENDING`].
pub const TOPIC_REASSIGNMENT_STATUS_COMPLETE: &str = "COMPLETE";
/// See [`TOPIC_REASSIGNMENT_STATUS_PENDING`].
pub const TOPIC_REASSIGNMENT_STATUS_STOPPED: &str = "STOPPED";

/// How many topic-selection patterns one reassignment may name:
/// "Regex patterns for topics to reassign. Use ['.*'] for all topics. Max 10
/// patterns." (`StartTopicReassignmentRequest$TopicsToInclude`).
pub const MAX_TOPIC_REASSIGNMENT_PATTERNS: usize = 10;

/// How long a `ClientToken` makes `StartTopicReassignment` idempotent for: "if you
/// retry a request with the same client token within 24 hours, the API returns the
/// original response without creating a duplicate operation"
/// (`StartTopicReassignmentRequest$ClientToken`).
pub const CLIENT_TOKEN_REUSE_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How many topic reassignment operations one cluster's registry keeps, past which
/// its oldest is forgotten.
///
/// The same bound a cluster's operation history is kept under
/// ([`MAX_OPERATIONS_PER_CLUSTER`]): a reassignment is an operation record of the
/// same kind, so one cluster cannot grow either history without bound.
pub const MAX_TOPIC_REASSIGNMENTS_PER_CLUSTER: usize = MAX_OPERATIONS_PER_CLUSTER;

/// The `TopicReassignmentMode` / `TopicReassignment` values, as the model's enums
/// spell them: `AUTO` has Amazon MSK reassign the topics itself, `MANUAL` leaves
/// them to the caller's own `StartTopicReassignment`.
pub const TOPIC_REASSIGNMENT_MODE_AUTO: &str = "AUTO";
/// See [`TOPIC_REASSIGNMENT_MODE_AUTO`]. It is the documented default of
/// `UpdateBrokerTypeRequest$TopicReassignment`: "when set to MANUAL (default),
/// customers manage topic reassignment using the StartTopicReassignment API".
pub const TOPIC_REASSIGNMENT_MODE_MANUAL: &str = "MANUAL";

/// The topic-selection patterns that name every topic of a cluster, which is what
/// an `UpdateBrokerType` whose `TopicReassignment` is `AUTO` reassigns: "MSK
/// automatically reassigns ALL topics to the target broker type"
/// (`UpdateBrokerTypeRequest$TopicReassignment`), and `['.*']` is the pattern list
/// the model gives for all topics.
pub fn all_topics_patterns() -> Vec<String> {
    vec![".*".to_string()]
}

/// One stored topic reassignment operation: what it was asked to move, how much of
/// the cluster that came to, and the instants its status is derived from.
#[derive(Debug, Clone)]
pub struct TopicReassignment {
    pub arn: String,
    /// The ARN of the cluster whose partitions are moving — half of every lookup,
    /// since a reassignment is addressed under its cluster.
    pub cluster_arn: String,
    /// The region the operation lives in: the signing region of the call that
    /// started it, and the region segment of its ARN.
    pub region: String,
    /// The broker instance type the topics are moving onto, as the request named
    /// it.
    pub target_instance_type: String,
    /// The patterns that selected the topics, kept so the readers report what the
    /// operation was asked for rather than what it resolved to.
    pub topics_to_include: Vec<String>,
    pub topics_to_exclude: Vec<String>,
    /// The `ClientToken` the accepted request carried, if any: a repeat of that
    /// request within [`CLIENT_TOKEN_REUSE_WINDOW`] is answered from this record
    /// instead of filing a second one.
    pub client_token: Option<String>,
    pub creation_time: SystemTime,
    /// The instant the clock owes this operation its `PENDING` -> `IN_PROGRESS`
    /// transition. Stored, never slept on.
    pub in_progress_due: SystemTime,
    /// The instant the clock owes it its completion, and the end of the window its
    /// progress is spread over.
    pub complete_due: SystemTime,
    /// When `StopTopicReassignment` was accepted, if it was: the instant the
    /// reported progress freezes at.
    pub stop_requested_at: Option<SystemTime>,
    /// The instant the clock then owes the operation its `STOPPING` -> `STOPPED`
    /// transition.
    pub stopped_due: Option<SystemTime>,
    /// How many topics of the cluster the patterns selected when the operation was
    /// filed.
    pub total_topics: i32,
    /// How many partitions those topics hold between them.
    pub total_partitions: i32,
}

impl TopicReassignment {
    /// The operation's status at `now`: the point its stored instants put it at,
    /// on the stopping path once a stop has been accepted and on the migrating
    /// path otherwise.
    ///
    /// A pure function of the record and the reading instant, so every reader of
    /// one operation reports one status and no read moves it along.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let (Some(_), Some(stopped_due)) = (self.stop_requested_at, self.stopped_due) {
            return match now >= stopped_due {
                true => TOPIC_REASSIGNMENT_STATUS_STOPPED,
                false => TOPIC_REASSIGNMENT_STATUS_STOPPING,
            };
        }
        if now >= self.complete_due {
            return TOPIC_REASSIGNMENT_STATUS_COMPLETE;
        }
        if now >= self.in_progress_due {
            return TOPIC_REASSIGNMENT_STATUS_IN_PROGRESS;
        }
        TOPIC_REASSIGNMENT_STATUS_PENDING
    }

    /// Whether this operation is still moving partitions at `now`, which is what
    /// makes a second reassignment of the same cluster a conflict and a stop of
    /// this one something there is still to stop.
    pub fn is_in_flight(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            TOPIC_REASSIGNMENT_STATUS_PENDING
                | TOPIC_REASSIGNMENT_STATUS_IN_PROGRESS
                | TOPIC_REASSIGNMENT_STATUS_STOPPING
        )
    }

    /// When the operation last changed status: the instant the transition it is
    /// now past was owed at, and the instant it was filed for one that has not
    /// transitioned yet.
    ///
    /// Derived from the same instants [`Self::status`] is, so the time a reader
    /// reports and the status it reports cannot disagree.
    pub fn last_update_time(&self, now: SystemTime) -> SystemTime {
        match self.status(now) {
            TOPIC_REASSIGNMENT_STATUS_STOPPED => self.stopped_due.unwrap_or(self.creation_time),
            TOPIC_REASSIGNMENT_STATUS_STOPPING => {
                self.stop_requested_at.unwrap_or(self.creation_time)
            }
            TOPIC_REASSIGNMENT_STATUS_COMPLETE => self.complete_due,
            TOPIC_REASSIGNMENT_STATUS_IN_PROGRESS => self.in_progress_due,
            _ => self.creation_time,
        }
    }

    /// How far through the migration window `now` is, as a fraction in 0..=1: 0
    /// before the partitions start moving, 1 once the operation is complete, and
    /// the elapsed share of the window in between — frozen at the instant a stop
    /// was accepted, since nothing moved after it.
    fn migrated_fraction(&self, now: SystemTime) -> f64 {
        let at = match self.stop_requested_at {
            Some(stopped_at) => stopped_at.min(now),
            None => now,
        };
        if at >= self.complete_due {
            return 1.0;
        }
        let Ok(elapsed) = at.duration_since(self.in_progress_due) else {
            return 0.0;
        };
        let Ok(window) = self.complete_due.duration_since(self.in_progress_due) else {
            return 1.0;
        };
        if window.is_zero() {
            return 1.0;
        }
        (elapsed.as_secs_f64() / window.as_secs_f64()).clamp(0.0, 1.0)
    }

    /// `total` scaled by how far through the migration window `now` is, rounded
    /// down: a count of what has moved, never more than the total the operation
    /// was filed with.
    fn migrated_share(&self, total: i32, now: SystemTime) -> i32 {
        let share = f64::from(total.max(0)) * self.migrated_fraction(now);
        // Floor: a topic counts as moved once its partitions are all across, so a
        // partly moved topic is not yet one of the completed ones.
        let moved = share.floor();
        if moved.is_finite() {
            // Bounded by the total it was scaled from, so the cast cannot
            // saturate differently from the clamp above.
            (moved as i32).clamp(0, total.max(0))
        } else {
            0
        }
    }

    /// How many of the selected topics have moved by `now`.
    pub fn completed_topics(&self, now: SystemTime) -> i32 {
        self.migrated_share(self.total_topics, now)
    }

    /// How many of their partitions have moved by `now`.
    pub fn migrated_partitions(&self, now: SystemTime) -> i32 {
        self.migrated_share(self.total_partitions, now)
    }

    /// Accept this operation's stop at `now`: the partitions stop where they are,
    /// and the operation is `STOPPING` until the clock owes it `STOPPED`.
    ///
    /// Idempotent — an operation already stopping keeps the instants it was given,
    /// so repeating the call cannot move the progress it froze or postpone the
    /// stop.
    pub fn begin_stop(&mut self, now: SystemTime) {
        if self.stop_requested_at.is_some() {
            return;
        }
        self.stop_requested_at = Some(now);
        self.stopped_due = Some(now + TOPIC_REASSIGNMENT_STOP_DELAY);
    }

    /// The key a cluster's reassignments are listed by: the instant the operation
    /// was filed, latest first, with its own ARN breaking a tie so the order is
    /// TOTAL — the same key shape a cluster's operation history is ordered by
    /// ([`ClusterOperation::history_key`]), and the key the listing's page token
    /// carries.
    pub fn listing_key(&self) -> (std::cmp::Reverse<u128>, &str) {
        (
            std::cmp::Reverse(
                self.creation_time
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_millis()),
            ),
            self.arn.as_str(),
        )
    }

    /// The operation as `TopicReassignmentInfo`, the shape
    /// `ListTopicReassignments` pages.
    pub fn to_reassignment_info(&self, now: SystemTime) -> TopicReassignmentInfo {
        TopicReassignmentInfo {
            completed_topics: Some(self.completed_topics(now)),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            status: Some(self.status(now).to_string()),
            target_instance_type: Some(self.target_instance_type.clone()),
            topic_reassignment_operation_arn: Some(self.arn.clone()),
            total_topics: Some(self.total_topics),
        }
    }

    /// The same stored record as `DescribeTopicReassignment` reports it, at the
    /// same reading instant: the summary the listing carries, plus the selection it
    /// was filed with and the partition counts it is moving.
    ///
    /// `ErrorInfo` is absent because nothing fails an emulated reassignment: there
    /// is no error to describe.
    pub fn to_describe_output(&self, now: SystemTime) -> DescribeTopicReassignmentOutput {
        DescribeTopicReassignmentOutput {
            cluster_arn: Some(self.cluster_arn.clone()),
            completed_topics: Some(self.completed_topics(now)),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            error_info: None,
            last_update_time: Some(::aws_smithy_types::DateTime::from(
                self.last_update_time(now),
            )),
            migrated_partitions: Some(self.migrated_partitions(now)),
            status: Some(self.status(now).to_string()),
            target_instance_type: Some(self.target_instance_type.clone()),
            topic_reassignment_operation_arn: Some(self.arn.clone()),
            topics_to_exclude: Some(self.topics_to_exclude.clone()),
            topics_to_include: Some(self.topics_to_include.clone()),
            total_partitions: Some(self.total_partitions),
            total_topics: Some(self.total_topics),
        }
    }
}

/// Why the topic-selection patterns of one reassignment cannot be used, naming the
/// member at fault as the wire spells it, or `None` when they are admissible.
///
/// The bound on the include list is the operation's own documented one
/// ([`MAX_TOPIC_REASSIGNMENT_PATTERNS`]); each pattern is admitted through the one
/// pattern reading of this service ([`topic_pattern_error`]), and the two lists
/// through the one matcher construction a reader selects topics with, so a list
/// this service could not select topics by is refused rather than stored and
/// selecting nothing.
pub fn reassignment_patterns_violation(
    topics_to_include: &[String],
    topics_to_exclude: &[String],
) -> Option<TopicPatternsViolation> {
    for (member, patterns) in [
        ("topicsToInclude", topics_to_include),
        ("topicsToExclude", topics_to_exclude),
    ] {
        if patterns.len() > MAX_TOPIC_REASSIGNMENT_PATTERNS {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} may hold at most {MAX_TOPIC_REASSIGNMENT_PATTERNS} patterns, not \
                     {}.",
                    patterns.len()
                ),
            });
        }
        if let Some(message) = patterns.iter().find_map(|pattern| topic_pattern_error(pattern)) {
            return Some(TopicPatternsViolation { member, message });
        }
        if topic_matcher(patterns).is_none() {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} holds patterns too large to be used together as a topic filter."
                ),
            });
        }
    }
    None
}

/// The work one `StartTopicReassignment` asks for: the broker type the topics are
/// to move onto, the patterns that select them, and the `ClientToken` the request
/// carried if any.
///
/// One value rather than four parameters, because it is one submitted REQUEST: the
/// idempotency match below compares the whole of it, so what identifies a repeat
/// and what the record is filed from cannot drift apart.
#[derive(Debug, Clone)]
pub struct TopicReassignmentRequest {
    /// `StartTopicReassignmentRequest$TargetInstanceType`.
    pub target_instance_type: String,
    /// `StartTopicReassignmentRequest$TopicsToInclude`.
    pub topics_to_include: Vec<String>,
    /// `StartTopicReassignmentRequest$TopicsToExclude`, empty when the request
    /// named none.
    pub topics_to_exclude: Vec<String>,
    /// `StartTopicReassignmentRequest$ClientToken`.
    pub client_token: Option<String>,
}

impl TopicReassignmentRequest {
    /// Whether `filed` is the operation THIS request filed: the same client token,
    /// and the same work asked of it.
    ///
    /// A token identifies one submitted request, so a token reused with a
    /// different target broker type or different patterns names no operation this
    /// registry filed.
    fn filed_as(&self, filed: &TopicReassignment) -> bool {
        self.client_token.is_some()
            && filed.client_token == self.client_token
            && filed.target_instance_type == self.target_instance_type
            && filed.topics_to_include == self.topics_to_include
            && filed.topics_to_exclude == self.topics_to_exclude
    }
}

impl ComAmazonawsKafkaKafkaState {
    /// File a topic reassignment of the cluster `cluster_arn` names at `now`,
    /// handing back the record the caller must now see.
    ///
    /// The topics it moves are the cluster's own, selected by `requested`'s include
    /// patterns minus its exclude patterns through [`TopicFilter`] — the same
    /// selection a replication flow's patterns go through — and the counts it
    /// reports are theirs, so an operation over a cluster holding no matching topic
    /// reports no topics rather than a number nothing backs. `None` when this
    /// region holds no such cluster.
    ///
    /// The per-cluster history is bounded here, by forgetting that cluster's
    /// oldest record past [`MAX_TOPIC_REASSIGNMENTS_PER_CLUSTER`], so one cluster
    /// cannot grow this registry without bound.
    pub fn start_topic_reassignment(
        &mut self,
        region: &str,
        cluster_arn: &str,
        now: SystemTime,
        requested: TopicReassignmentRequest,
    ) -> Option<&TopicReassignment> {
        let TopicReassignmentRequest {
            target_instance_type,
            topics_to_include,
            topics_to_exclude,
            client_token,
        } = requested;
        let cluster = self.cluster_in(region, cluster_arn)?;
        let cluster_name = cluster.name.clone();
        let cluster_region = cluster.region.clone();
        let cluster_arn = cluster.arn.clone();
        // The cluster's own resource id, so the operation's ARN names the cluster
        // it belongs to the way every other MSK sub-resource ARN does.
        let cluster_id = cluster_arn.split('/').next_back().unwrap_or_default();
        let filter = TopicFilter::from_patterns(&topics_to_include, &topics_to_exclude);
        let selected: Vec<i32> = self
            .topics_in(&cluster_arn)
            .filter(|topic| topic.is_present(now) && filter.covers(&topic.name))
            .map(|topic| topic.partition_count)
            .collect();
        let total_topics = i32::try_from(selected.len()).unwrap_or(i32::MAX);
        let total_partitions = selected.iter().fold(0i32, |sum, count| {
            sum.saturating_add((*count).max(0))
        });
        let ordinal = self.next_topic_reassignment_suffix + 1;
        let operation_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{cluster_arn}:topic-reassignment:{ordinal}",
            ::engine::emulator_account_id()
        )));
        let arn = ::engine::emulator_arn(
            "kafka",
            &cluster_region,
            &format!("topic-reassignment/{cluster_name}/{cluster_id}/{operation_id}"),
        );
        let reassignment = TopicReassignment {
            arn: arn.clone(),
            cluster_arn: cluster_arn.clone(),
            region: cluster_region,
            target_instance_type,
            topics_to_include,
            topics_to_exclude,
            client_token,
            creation_time: now,
            in_progress_due: now + TOPIC_REASSIGNMENT_START_DELAY,
            complete_due: now + TOPIC_REASSIGNMENT_START_DELAY + TOPIC_REASSIGNMENT_MIGRATION_DELAY,
            stop_requested_at: None,
            stopped_due: None,
            total_topics,
            total_partitions,
        };
        self.next_topic_reassignment_suffix = self.next_topic_reassignment_suffix.saturating_add(1);
        self.topic_reassignments.insert(arn.clone(), reassignment);
        self.bound_cluster_topic_reassignments(&cluster_arn);
        self.topic_reassignments.get(&arn)
    }

    /// Forget the oldest reassignments of `cluster_arn` past
    /// [`MAX_TOPIC_REASSIGNMENTS_PER_CLUSTER`].
    fn bound_cluster_topic_reassignments(&mut self, cluster_arn: &str) {
        let mut held: Vec<(u128, String)> = self
            .topic_reassignments
            .values()
            .filter(|reassignment| reassignment.cluster_arn == cluster_arn)
            .map(|reassignment| {
                let (std::cmp::Reverse(millis), arn) = reassignment.listing_key();
                (millis, arn.to_string())
            })
            .collect();
        // Oldest first, so the excess taken off the front is this cluster's oldest.
        held.sort_unstable();
        let excess = held
            .len()
            .saturating_sub(MAX_TOPIC_REASSIGNMENTS_PER_CLUSTER);
        for (_, arn) in held.into_iter().take(excess) {
            self.topic_reassignments.remove(&arn);
        }
    }

    /// The reassignment `operation_arn` names under `cluster_arn`, as an endpoint
    /// for `region` sees it. The ONE lookup every reassignment reader answers
    /// from, so a listing and a describe of one operation cannot disagree.
    ///
    /// A pure read: the operation's status and progress are functions of the
    /// instants it holds and the reading instant, so nothing has to be settled for
    /// the answer to be right.
    pub fn topic_reassignment_in(
        &self,
        region: &str,
        cluster_arn: &str,
        operation_arn: &str,
    ) -> Option<&TopicReassignment> {
        self.topic_reassignments
            .get(operation_arn)
            .filter(|reassignment| {
                reassignment.region == region && reassignment.cluster_arn == cluster_arn
            })
    }

    /// The same lookup for a call that WRITES to the record.
    pub fn topic_reassignment_mut_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        operation_arn: &str,
    ) -> Option<&mut TopicReassignment> {
        self.topic_reassignments
            .get_mut(operation_arn)
            .filter(|reassignment| {
                reassignment.region == region && reassignment.cluster_arn == cluster_arn
            })
    }

    /// The reassignments `cluster_arn` holds, as an endpoint for `region` sees
    /// them, latest first ([`TopicReassignment::listing_key`]).
    pub fn topic_reassignments_in(
        &self,
        region: &str,
        cluster_arn: &str,
    ) -> Vec<&TopicReassignment> {
        let mut held: Vec<&TopicReassignment> = self
            .topic_reassignments
            .values()
            .filter(|reassignment| {
                reassignment.cluster_arn == cluster_arn && reassignment.region == region
            })
            .collect();
        held.sort_by(|left, right| left.listing_key().cmp(&right.listing_key()));
        held
    }

    /// Whether the cluster `cluster_arn` has a reassignment still moving
    /// partitions at `now`: one at a time, which is what makes a second
    /// `StartTopicReassignment` a conflict.
    pub fn topic_reassignment_in_flight(&self, cluster_arn: &str, now: SystemTime) -> bool {
        self.topic_reassignments.values().any(|reassignment| {
            reassignment.cluster_arn == cluster_arn && reassignment.is_in_flight(now)
        })
    }

    /// The reassignment of `cluster_arn` an accepted request identical to
    /// `requested` already filed, if one was filed within
    /// [`CLIENT_TOKEN_REUSE_WINDOW`] of `now` — which is what a repeat of that
    /// request is answered from instead of filing a second operation.
    ///
    /// What counts as the same request is [`TopicReassignmentRequest::filed_as`],
    /// so a call carrying no token matches nothing and a token reused for other
    /// work is not answered from the operation the first call filed.
    pub fn topic_reassignment_for_client_token(
        &self,
        cluster_arn: &str,
        requested: &TopicReassignmentRequest,
        now: SystemTime,
    ) -> Option<&TopicReassignment> {
        self.topic_reassignments.values().find(|reassignment| {
            reassignment.cluster_arn == cluster_arn
                && requested.filed_as(reassignment)
                && now
                    .duration_since(reassignment.creation_time)
                    .is_ok_and(|age| age < CLIENT_TOKEN_REUSE_WINDOW)
        })
    }

    /// Drop every topic reassignment of the cluster `cluster_arn`, which is what
    /// makes one disappear with the cluster whose partitions it was moving.
    pub fn drop_cluster_topic_reassignments(&mut self, cluster_arn: &str) {
        self.topic_reassignments
            .retain(|_, reassignment| reassignment.cluster_arn != cluster_arn);
    }
}
