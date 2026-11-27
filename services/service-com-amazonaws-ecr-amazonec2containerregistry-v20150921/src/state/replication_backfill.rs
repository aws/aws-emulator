// SPDX-License-Identifier: CC0-1.0

//! The backfills that replicate a registry's EXISTING images: the records
//! `StartReplicationBackfill` creates, the statuses the clock moves them through, and the
//! per-image results `DescribeReplicationBackfillResults` and `ListReplicationBackfills`
//! report.
//!
//! # The rule this module implements
//!
//! The registry's replication configuration copies an image as it ARRIVES (see
//! [`super::replication`]): a rule stored after a push does not reach back for an image the
//! registry already holds. A replication backfill is what does reach back —
//! `StartReplicationBackfill` "replicates a registry's existing images that match its
//! replication configuration to the specified destination" — so the set of copies ONE
//! backfill makes is decided from the store as it stands when the backfill prepares: every
//! image of every repository of the source registry that a replication rule naming that
//! destination covers, counted "as a single replication" per image AND TAG combination
//! (`ReplicationBackfillResult`'s own documentation).
//!
//! The destination must already be a destination of the registry's configuration, which is
//! what makes a backfill a re-run of the configuration rather than a copy a caller can
//! point anywhere; a registry runs at most one backfill per destination Region at a time
//! and no more than [`MAX_BACKFILLS_IN_PROGRESS`] at once.
//!
//! # What the clock owes
//!
//! A backfill is not instantaneous and its status enum says so: it is `PREPARING` while it
//! works out the set of images to replicate, `IN_PROGRESS` while that set is being
//! replicated, and `COMPLETE` once it is. Both transitions are the CLOCK's
//! ([`BACKFILL_PREPARE_DELAY`], [`BACKFILL_REPLICATION_DELAY`]): the instants are stored on
//! the record and nothing sleeps for them, so the progression is owed whether or not
//! anybody calls again. It is made by the registry's own delivery task
//! ([`super::replication::deliver_when_due`], which settles the copies a push owes and the
//! ones a backfill owes under one slot) and, for a caller that reads before that task has
//! run, by the read itself — a read materialises only what the clock had already made due,
//! so reading twice reports the same thing.
//!
//! The copies a backfill makes are the SAME copies a push makes
//! ([`super::ComAmazonawsEcrAmazonec2containerregistryV20150921State::copy_image_into`]):
//! the destination registry ends up holding the manifest bytes as they were pushed, the
//! blobs that manifest references and the tags pointing at it, so a backfill this module
//! reports `COMPLETE` is one a read of the destination Region can see the images of.

use ::std::collections::{BTreeMap, BTreeSet};
use ::std::time::{Duration, SystemTime};

use super::ComAmazonawsEcrAmazonec2containerregistryV20150921State as State;
use crate::ops::types::{
    ImageIdentifier, ReplicationBackfillProgress, ReplicationBackfillResult,
    ReplicationBackfillSummary, ReplicationDestination,
};

/// The `ReplicationBackfillStatus` of a backfill that is working out which images it has to
/// replicate: the status a freshly started backfill is returned with ("A newly started
/// replication backfill is returned with a status of PREPARING").
pub const BACKFILL_STATUS_PREPARING: &str = "PREPARING";

/// The `ReplicationBackfillStatus` of a backfill whose set of images is settled and is
/// being replicated.
pub const BACKFILL_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ReplicationBackfillStatus` of a backfill that has replicated its set — a terminal
/// status.
pub const BACKFILL_STATUS_COMPLETE: &str = "COMPLETE";

/// The `ReplicationBackfillStatus` of a backfill a `StopReplicationBackfill` asked to stop
/// while replications of it were still in progress: those "continue to completion", so the
/// backfill is stopping rather than stopped.
pub const BACKFILL_STATUS_STOPPING: &str = "STOPPING";

/// The `ReplicationBackfillStatus` of a stopped backfill — a terminal status.
pub const BACKFILL_STATUS_STOPPED: &str = "STOPPED";

/// The `ReplicationBackfillResultStatus` of one replication the destination took.
pub const BACKFILL_RESULT_COMPLETE: &str = "COMPLETE";

/// The `ReplicationBackfillResultStatus` of one replication the destination did not take.
pub const BACKFILL_RESULT_FAILED: &str = "FAILED";

/// How long after the `StartReplicationBackfill` call that created it the clock owes a
/// backfill its `PREPARING` -> `IN_PROGRESS` transition.
///
/// AWS publishes no duration for the preparation of a backfill, so this is a CHOSEN value
/// inside the 250–750 ms band this project fixes for a clock-owed transition, like
/// [`super::IMAGE_REPLICATION_DELAY`] beside it. It is never zero, so a caller that reads a
/// backfill straight after starting it still meets `PREPARING` and the `null` progress that
/// status carries, and it is stored as a due instant and never slept for.
pub const BACKFILL_PREPARE_DELAY: Duration = Duration::from_millis(400);

/// How long after it finishes preparing (see [`BACKFILL_PREPARE_DELAY`]) the clock owes a
/// backfill its `IN_PROGRESS` -> terminal transition.
///
/// A CHOSEN value for the same reason and inside the same band, and never zero either: a
/// caller that reads a backfill once it has begun replicating still meets `IN_PROGRESS`, and
/// a stop made then still meets the `STOPPING` state that status leads to.
pub const BACKFILL_REPLICATION_DELAY: Duration = Duration::from_millis(400);

/// How many backfills of one registry may be in progress at once: "a registry can have at
/// most 10 replication backfills in progress at once. Starting a replication backfill that
/// would exceed this limit fails with a LimitExceededException"
/// (`StartReplicationBackfill`'s own documentation).
pub const MAX_BACKFILLS_IN_PROGRESS: usize = 10;

/// How long a backfill that has reached a terminal status is still reported for:
/// "replication backfills that have reached a terminal status are retained for 90 days after
/// they end, after which they are no longer returned" (`ListReplicationBackfills`'s own
/// documentation).
pub const BACKFILL_RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// See [`super::MAX_REPOSITORIES`]: the same reasoning for the backfill table, which
/// `StartReplicationBackfill` grows by one entry per call.
///
/// At most [`MAX_BACKFILLS_IN_PROGRESS`] of the entries can be in progress, so the rest have
/// ended and are held only for the [`BACKFILL_RETENTION`] window a read still reports them
/// in; a start that meets this bound therefore drops the backfill that ended FIRST to make
/// room ([`State::reclaim_replication_backfills`]) rather than refusing, so a registry that
/// is backfilled again and again keeps its most recent history and never grows without
/// bound.
pub const MAX_REPLICATION_BACKFILLS: usize = 100;

/// The most replications ONE backfill plans.
///
/// The plan is one entry per image AND TAG of every covered repository, so its size is
/// decided by what the registry holds rather than by the request; this is the emulator's own
/// bound on it, far past the images any registry this store holds in practice
/// ([`super::MAX_REGISTRY_CALLER_BYTES`] bounds the manifests it can hold at all). A
/// registry holding more covered images than this is backfilled for this many of them, in
/// the one deterministic order the plan is built in, and the progress it reports is the plan
/// it made.
pub const MAX_BACKFILL_REPLICATIONS: usize = 10_000;

/// A failure one of the backfill operations reports, mapped by each handler onto the
/// modelled error its own model declares — the same division of labour as [`super::Fault`],
/// which carries the failures the repository and image surface share.
#[derive(Debug, Clone)]
pub enum BackfillFault {
    /// No backfill of that id in this registry — `ReplicationBackfillNotFoundException`.
    NotFound(String),
    /// The backfill exists but its status does not admit the operation —
    /// `ReplicationBackfillInvalidStatusException`.
    InvalidStatus(String),
    /// A backfill for the destination Region asked for is already running —
    /// `ReplicationBackfillInProgressException`.
    InProgress(String),
    /// The registry already runs as many backfills as it may — `LimitExceededException`.
    LimitExceeded(String),
    /// The request names a destination the registry's replication configuration does not —
    /// `InvalidParameterException`.
    InvalidParameter(String),
}

impl BackfillFault {
    /// The message this failure renders with.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(m)
            | Self::InvalidStatus(m)
            | Self::InProgress(m)
            | Self::InvalidParameter(m)
            | Self::LimitExceeded(m) => m,
        }
    }
}

/// One replication the plan of a backfill names: one image, under one of its tags, of one
/// repository.
///
/// It NAMES the copy rather than carrying it — the image is read out of the store when the
/// copy is made — so a backfill never delivers content that was decided anywhere but on the
/// stored image.
#[derive(Debug, Clone)]
struct PlannedReplication {
    /// The repository holding the image, whose name the destination's repository shares.
    repository_name: String,
    /// The content digest the image is addressed by, in both registries.
    digest: String,
    /// The tag this replication is of, or `None` for an image no tag points at.
    tag: Option<String>,
}

impl PlannedReplication {
    /// The image identifier this replication is reported under: the digest it is addressed
    /// by, with the tag it was planned for when it has one.
    fn image_id(&self) -> ImageIdentifier {
        ImageIdentifier {
            image_digest: Some(self.digest.clone()),
            image_tag: self.tag.clone(),
        }
    }

    /// The key a paginated read of the results both ORDERS and pages by: the repository, the
    /// image and the tag, which is the order the plan is built in and is total, so a page
    /// boundary means the same thing on the next call.
    fn result_key(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{}",
            self.repository_name,
            self.digest,
            self.tag.as_deref().unwrap_or_default()
        )
    }
}

/// One replication backfill of this registry.
#[derive(Debug, Clone)]
pub struct ReplicationBackfill {
    /// The id `StartReplicationBackfill` answered with, which the describe and the stop
    /// address this record by.
    pub id: String,
    /// The Region of the registry whose existing images this backfill replicates: the Region
    /// the start call was addressed to.
    pub source_region: String,
    /// Where the images are replicated to, as the start call named it.
    pub destination: ReplicationDestination,
    /// When the backfill was started — `startedAt`.
    pub started_at: SystemTime,
    /// When the `PREPARING` -> `IN_PROGRESS` transition falls due.
    pub prepare_due_at: SystemTime,
    /// When the `IN_PROGRESS` -> terminal transition falls due.
    pub replicate_due_at: SystemTime,
    /// The backfill's `ReplicationBackfillStatus` as the clock has carried it so far.
    pub status: String,
    /// When the backfill ended, or `None` while it has not — "this value is null while the
    /// replication backfill is still in progress".
    pub ended_at: Option<SystemTime>,
    /// `progress`: "null until the replication backfill finishes preparing the set of images
    /// to replicate […] and remains null if the replication backfill is stopped before
    /// preparation completes".
    pub progress: Option<ReplicationBackfillProgress>,
    /// The per-image results, which are reported "only after the replication backfill
    /// reaches a terminal status" — empty until then.
    pub results: Vec<ReplicationBackfillResult>,
    /// The plan the preparation settled: what the replication step copies, kept so the
    /// results report the plan that was made rather than the store as it stands when the
    /// copies are made.
    plan: Vec<PlannedReplication>,
}

impl ReplicationBackfill {
    /// Whether this backfill is still running: one that has not reached a terminal status,
    /// which is what the one-per-destination-Region rule and
    /// [`MAX_BACKFILLS_IN_PROGRESS`] count.
    pub fn running(&self) -> bool {
        self.ended_at.is_none()
    }

    /// This backfill as a listing reports it.
    pub fn to_summary(&self) -> ReplicationBackfillSummary {
        ReplicationBackfillSummary {
            replication_backfill_id: self.id.clone(),
            destination: self.destination.clone(),
            status: self.status.clone(),
            // No backfill of this registry reports one: see `failure_code` on the describe
            // handler.
            failure_code: None,
            progress: self.progress.clone(),
            started_at: super::reported_instant(self.started_at),
            ended_at: self.ended_at.map(super::reported_instant),
        }
    }

    /// The key a paginated listing both ORDERS and pages by: the instant the backfill was
    /// started, made total by its own id.
    pub fn listing_key(&self) -> String {
        super::chronological_key(self.started_at, &self.id)
    }
}

impl State {
    /// Start a backfill of the registry in `source_region` into `destination` at `now`,
    /// keep it, and answer the record as it was stored.
    ///
    /// Refused where the registry's replication configuration does not name `destination`
    /// (the backfill re-runs that configuration, so it cannot copy anywhere the
    /// configuration does not send this registry's images), where a backfill for that
    /// destination Region is already running, and where the registry already runs
    /// [`MAX_BACKFILLS_IN_PROGRESS`] of them.
    pub fn start_replication_backfill(
        &mut self,
        source_region: &str,
        destination: ReplicationDestination,
        now: SystemTime,
    ) -> Result<ReplicationBackfill, BackfillFault> {
        // A status the clock already owes is written down before the request is judged
        // against the backfills in flight: a backfill whose completion fell due is not one
        // this start contends with.
        self.settle_replication_backfills(now);

        if !self.configured_destination(&destination) {
            return Err(BackfillFault::InvalidParameter(super::parameter_constraint(
                BACKFILL_DESTINATION_PARAMETER,
                UNCONFIGURED_DESTINATION_CONSTRAINT,
            )));
        }
        if let Some(running) = self.backfill_running_into(&destination.region) {
            return Err(BackfillFault::InProgress(
                backfill_in_progress_message(&running.destination.region),
            ));
        }
        if self.replication_backfills_running() >= MAX_BACKFILLS_IN_PROGRESS {
            return Err(BackfillFault::LimitExceeded(backfill_limit_message()));
        }

        self.reclaim_replication_backfills(now);
        let backfill = ReplicationBackfill {
            id: mint_backfill_id(),
            source_region: source_region.to_string(),
            destination,
            started_at: now,
            prepare_due_at: now + BACKFILL_PREPARE_DELAY,
            replicate_due_at: now + BACKFILL_PREPARE_DELAY + BACKFILL_REPLICATION_DELAY,
            status: BACKFILL_STATUS_PREPARING.to_string(),
            ended_at: None,
            progress: None,
            results: Vec::new(),
            plan: Vec::new(),
        };
        self.replication_backfills
            .insert(backfill.id.clone(), backfill.clone());
        Ok(backfill)
    }

    /// Stop the backfill `id` addresses at `now` and answer it as it now stands.
    ///
    /// A backfill still preparing has no replication in flight, so it stops at once; one
    /// that is replicating is `STOPPING` until the replications already in progress
    /// "continue to completion", which the clock owes at the instant they were due anyway.
    /// A backfill that has already reached a terminal status, or is already stopping, is
    /// refused: there is nothing left for a stop to do to it.
    pub fn stop_replication_backfill(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Result<ReplicationBackfill, BackfillFault> {
        // The stop is judged against the status the clock owes, not the one last written: a
        // backfill whose completion fell due is already terminal.
        self.settle_replication_backfills(now);
        let backfill = self
            .replication_backfills
            .get_mut(id)
            .ok_or_else(|| BackfillFault::NotFound(backfill_not_found_message(id)))?;
        match backfill.status.as_str() {
            BACKFILL_STATUS_PREPARING => {
                // Nothing of it is replicating yet, so it stops here — and its progress
                // stays `null`, which is what a backfill stopped before preparation
                // completed reports.
                backfill.status = BACKFILL_STATUS_STOPPED.to_string();
                backfill.ended_at = Some(now);
            }
            BACKFILL_STATUS_IN_PROGRESS => {
                backfill.status = BACKFILL_STATUS_STOPPING.to_string();
            }
            _ => {
                return Err(BackfillFault::InvalidStatus(
                    backfill_invalid_status_message(id, &backfill.status),
                ))
            }
        }
        Ok(backfill.clone())
    }

    /// The backfill `id` addresses, or [`BackfillFault::NotFound`].
    pub fn replication_backfill(&self, id: &str) -> Result<&ReplicationBackfill, BackfillFault> {
        self.replication_backfills
            .get(id)
            .ok_or_else(|| BackfillFault::NotFound(backfill_not_found_message(id)))
    }

    /// How many of this registry's backfills are still running.
    fn replication_backfills_running(&self) -> usize {
        self.replication_backfills
            .values()
            .filter(|backfill| backfill.running())
            .count()
    }

    /// The backfill still running into `region`, if any: the one the
    /// "only one replication backfill can be in progress for a given destination Region at a
    /// time" rule refuses a second start for.
    fn backfill_running_into(&self, region: &str) -> Option<&ReplicationBackfill> {
        self.replication_backfills
            .values()
            .find(|backfill| backfill.running() && backfill.destination.region == region)
    }

    /// Whether the registry's replication configuration names `destination` — the
    /// destination a backfill of it may be started for.
    fn configured_destination(&self, destination: &ReplicationDestination) -> bool {
        self.replication_rules
            .iter()
            .flat_map(|rule| rule.destinations.iter())
            .any(|configured| {
                configured.region == destination.region
                    && configured.registry_id == destination.registry_id
            })
    }

    /// Drop the backfills a read no longer reports: every ended one past
    /// [`BACKFILL_RETENTION`] as of `now`, and then, while the table is at
    /// [`MAX_REPLICATION_BACKFILLS`], the one that ended first.
    ///
    /// Expiry is the clock's, not a caller's: a backfill past its retention window is
    /// reported by nothing whether or not anybody looks, so dropping it changes nothing a
    /// later caller can observe.
    fn reclaim_replication_backfills(&mut self, now: SystemTime) {
        self.replication_backfills.retain(|_, backfill| {
            backfill.ended_at.is_none_or(|ended| {
                // An instant `now` has not reached is no age at all, so a backfill that
                // ended at one is inside its window rather than past it.
                now.duration_since(ended).unwrap_or_default() < BACKFILL_RETENTION
            })
        });
        while self.replication_backfills.len() >= MAX_REPLICATION_BACKFILLS {
            let Some(oldest) = self
                .replication_backfills
                .values()
                .filter(|backfill| !backfill.running())
                .min_by_key(|backfill| backfill.ended_at)
                .map(|backfill| backfill.id.clone())
            else {
                return;
            };
            self.replication_backfills.remove(&oldest);
        }
    }

    /// When the earliest transition this registry's backfills still owe falls due, or `None`
    /// when they owe none.
    ///
    /// Reported whether the instant is still to come or already past, for the same reason
    /// [`State::next_replication_due`] reports an overdue copy: a transition whose instant
    /// passed before anything could make it is owed MORE urgently, not less.
    pub fn next_replication_backfill_due(&self) -> Option<SystemTime> {
        self.replication_backfills
            .values()
            .filter_map(|backfill| match backfill.status.as_str() {
                BACKFILL_STATUS_PREPARING => Some(backfill.prepare_due_at),
                BACKFILL_STATUS_IN_PROGRESS | BACKFILL_STATUS_STOPPING => {
                    Some(backfill.replicate_due_at)
                }
                _ => None,
            })
            .min()
    }

    /// Make every backfill transition the clock has already made due as of `now`, and answer
    /// HOW MANY it made.
    ///
    /// Idempotent: a transition that has been made is not made again, so settling twice
    /// reports the same backfills. Both transitions of one backfill are made in one pass
    /// when both are due, so a caller that arrives late sees the terminal status rather than
    /// an intermediate one it was never owed.
    pub fn settle_replication_backfills(&mut self, now: SystemTime) -> usize {
        let mut made = 0usize;
        while let Some(id) = self.next_due_replication_backfill(now) {
            self.settle_one_replication_backfill(&id);
            made = made.saturating_add(1);
        }
        made
    }

    /// The id of a backfill with a transition due as of `now`, or `None` when none has one.
    fn next_due_replication_backfill(&self, now: SystemTime) -> Option<String> {
        self.replication_backfills
            .values()
            .find(|backfill| match backfill.status.as_str() {
                BACKFILL_STATUS_PREPARING => backfill.prepare_due_at <= now,
                BACKFILL_STATUS_IN_PROGRESS | BACKFILL_STATUS_STOPPING => {
                    backfill.replicate_due_at <= now
                }
                _ => false,
            })
            .map(|backfill| backfill.id.clone())
    }

    /// Make the one transition the backfill `id` owes — whichever its stored status says is
    /// next, which is what [`Self::next_due_replication_backfill`] established was due.
    fn settle_one_replication_backfill(&mut self, id: &str) {
        let Some(backfill) = self.replication_backfills.get(id) else {
            return;
        };
        match backfill.status.as_str() {
            BACKFILL_STATUS_PREPARING => {
                let source_region = backfill.source_region.clone();
                let destination = backfill.destination.clone();
                // The set of images is worked out from the store as it stands at the
                // instant the preparation was due, which is what the backfill then
                // replicates and reports a total for.
                let plan = self.planned_backfill(&source_region, &destination);
                if let Some(backfill) = self.replication_backfills.get_mut(id) {
                    backfill.progress = Some(prepared_progress(plan.len()));
                    backfill.plan = plan;
                    backfill.status = BACKFILL_STATUS_IN_PROGRESS.to_string();
                }
            }
            BACKFILL_STATUS_IN_PROGRESS | BACKFILL_STATUS_STOPPING => {
                self.replicate_backfill_plan(id);
            }
            _ => {}
        }
    }

    /// Make the copies the backfill `id` planned, record one result per planned replication
    /// and carry the backfill to its terminal status.
    ///
    /// A backfill that was asked to stop ends `STOPPED` rather than `COMPLETE`: the
    /// replications already in progress "continue to completion", and they are these, so
    /// what it stops is any further work rather than the copies it had started.
    fn replicate_backfill_plan(&mut self, id: &str) {
        let Some(backfill) = self.replication_backfills.get(id) else {
            return;
        };
        let plan = backfill.plan.clone();
        let destination_region = backfill.destination.region.clone();
        let destination_registry_id = backfill.destination.registry_id.clone();
        let source_region = backfill.source_region.clone();
        let stopping = backfill.status == BACKFILL_STATUS_STOPPING;
        let due_at = backfill.replicate_due_at;
        // Whether the destination is a registry this emulator holds at all. One is not:
        // another account's, because this emulator serves exactly one account
        // ([`super::registry_id`]) and holds no registry of anybody else's to copy into — so
        // a replication of it is reported by its status alone, exactly as the copies a push
        // starts to such a destination are (see [`super::replication`]).
        let delivers_here = super::replication::delivers_into(
            &source_region,
            &destination_region,
            &destination_registry_id,
        );

        // One copy per image, carrying every tag that points at it, so an image under two
        // tags is two replications of one copy. The copy is made at the instant the clock
        // owed it, so the destination's record of the image is not dated by whoever happened
        // to settle it.
        let mut copied: BTreeMap<(String, String), bool> = BTreeMap::new();
        let mut results = Vec::with_capacity(plan.len());
        for replication in &plan {
            let key = (
                replication.repository_name.clone(),
                replication.digest.clone(),
            );
            let made = match copied.get(&key) {
                Some(made) => *made,
                None => {
                    let made = !delivers_here
                        || self.copy_image_into(
                            &source_region,
                            &replication.repository_name,
                            &replication.digest,
                            &destination_region,
                            due_at,
                        );
                    copied.insert(key, made);
                    made
                }
            };
            results.push(ReplicationBackfillResult {
                repository_name: replication.repository_name.clone(),
                image_id: replication.image_id(),
                status: if made {
                    BACKFILL_RESULT_COMPLETE.to_string()
                } else {
                    BACKFILL_RESULT_FAILED.to_string()
                },
                // No failure code: this registry has no published `ReplicationError` value
                // to state for a copy its destination would not take, and a code AWS does
                // not send would be worse than none.
                failure_code: None,
                sub_image_failure_codes: None,
            });
        }

        let completed = results
            .iter()
            .filter(|result| result.status == BACKFILL_RESULT_COMPLETE)
            .count();
        let failed = results.len().saturating_sub(completed);
        let Some(backfill) = self.replication_backfills.get_mut(id) else {
            return;
        };
        backfill.progress = Some(ReplicationBackfillProgress {
            total_replication_count: count_of(results.len()),
            completed_replication_count: count_of(completed),
            failed_replication_count: count_of(failed),
            // The distinct failure codes of the failed replications, which are none: see
            // `failure_code` above.
            failure_codes: Vec::new(),
        });
        backfill.results = results;
        backfill.status = if stopping {
            BACKFILL_STATUS_STOPPED.to_string()
        } else {
            BACKFILL_STATUS_COMPLETE.to_string()
        };
        // The backfill ended when the clock made its replications due, not when something
        // got round to settling them, so a late settle reports the instant that was owed
        // rather than the instant somebody looked.
        backfill.ended_at = Some(due_at);
    }

    /// The plan a backfill of the registry in `source_region` into `destination` makes: one
    /// replication per image AND tag of every repository a replication rule naming that
    /// destination covers, in repository, image and tag order.
    ///
    /// An image no tag points at is one replication of its own: it is an image the
    /// destination has to end up holding, and the digest addresses it.
    fn planned_backfill(
        &self,
        source_region: &str,
        destination: &ReplicationDestination,
    ) -> Vec<PlannedReplication> {
        let covered: BTreeSet<&str> = self
            .replication_rules
            .iter()
            .filter(|rule| {
                rule.destinations.iter().any(|configured| {
                    configured.region == destination.region
                        && configured.registry_id == destination.registry_id
                })
            })
            .flat_map(|rule| {
                self.repositories_in(source_region)
                    .filter(move |repo| super::replication_rule_covers(rule, &repo.name))
                    .map(|repo| repo.name.as_str())
            })
            .collect();

        let mut plan = Vec::new();
        for name in covered {
            let Ok(repo) = self.repository(source_region, name) else {
                continue;
            };
            for image in repo.images.values() {
                if image.tags.is_empty() {
                    plan.push(PlannedReplication {
                        repository_name: repo.name.clone(),
                        digest: image.digest.clone(),
                        tag: None,
                    });
                }
                for tag in &image.tags {
                    plan.push(PlannedReplication {
                        repository_name: repo.name.clone(),
                        digest: image.digest.clone(),
                        tag: Some(tag.clone()),
                    });
                }
            }
        }
        plan.truncate(MAX_BACKFILL_REPLICATIONS);
        plan
    }

    /// The results of the backfill `id` that `status_filter` admits, each with the key a
    /// paginated read pages by.
    ///
    /// `status_filter` is `resultStatusFilter`: "if you omit this parameter, results with
    /// both statuses are returned".
    pub fn replication_backfill_results(
        &self,
        id: &str,
        status_filter: Option<&str>,
    ) -> Result<Vec<(String, ReplicationBackfillResult)>, BackfillFault> {
        let backfill = self.replication_backfill(id)?;
        Ok(backfill
            .plan
            .iter()
            .zip(backfill.results.iter())
            .filter(|(_, result)| status_filter.is_none_or(|wanted| result.status == wanted))
            .map(|(replication, result)| (replication.result_key(), result.clone()))
            .collect())
    }
}

/// The progress of a backfill that has just finished preparing: the total it will perform,
/// and nothing done yet.
fn prepared_progress(total: usize) -> ReplicationBackfillProgress {
    ReplicationBackfillProgress {
        total_replication_count: count_of(total),
        completed_replication_count: 0,
        failed_replication_count: 0,
        failure_codes: Vec::new(),
    }
}

/// A count as a `NonNegativeInteger` reports it. The counts are bounded by
/// [`MAX_BACKFILL_REPLICATIONS`], so the conversion cannot be reached with a figure outside
/// the type; a figure that somehow is states the largest one the member can carry rather
/// than wrapping to a negative count.
fn count_of(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}

/// The API label of the input member a refused destination is stated against.
const BACKFILL_DESTINATION_PARAMETER: &str = "destination";

/// The constraint a start fails when the registry's replication configuration does not name
/// the destination it asks for. The phrase states the constraint itself and carries no value
/// from the request: which Region and registry were named does not change what the caller
/// has to do about it.
const UNCONFIGURED_DESTINATION_CONSTRAINT: &str =
    "The destination must be configured in the registry's replication configuration";

/// The wording ECR uses when a backfill for a destination Region is already running — the
/// state `ReplicationBackfillInProgressException` documents, phrased from the request: the
/// destination Region that was asked for, and what to do about it.
fn backfill_in_progress_message(region: &str) -> String {
    format!(
        "A replication backfill is already in progress for the destination Region \
         '{region}' in the registry with id '{}'. Wait for the in progress backfill to \
         reach a terminal status, or stop it with the StopReplicationBackfill operation, \
         before starting another one.",
        super::registry_id()
    )
}

/// The wording ECR uses when a start would carry the registry past the number of backfills
/// it may run at once — `LimitExceededException`. Built like
/// [`super::pull_time_update_exclusion_limit_message`], from the registry the request
/// addressed and the bound it met.
fn backfill_limit_message() -> String {
    format!(
        "The registry with id '{}' has the maximum number of replication backfills in \
         progress allowed, which is {MAX_BACKFILLS_IN_PROGRESS}.",
        super::registry_id()
    )
}

/// The wording ECR uses when no backfill of this registry answers to an id: the kind of
/// thing that is missing, the id the request named it by, and the registry it was looked for
/// in — the construction [`super::repository_not_found_message`] follows.
fn backfill_not_found_message(id: &str) -> String {
    format!(
        "The replication backfill with ID '{id}' does not exist in the registry with id \
         '{}'",
        super::registry_id()
    )
}

/// The wording ECR uses when a backfill's status does not admit the operation asked of it —
/// the state `ReplicationBackfillInvalidStatusException` documents, phrased from the
/// request: the backfill that was addressed, the status it is in, and where to read that
/// status.
fn backfill_invalid_status_message(id: &str, status: &str) -> String {
    format!(
        "The replication backfill with ID '{id}' is in status '{status}' and cannot be \
         stopped. Use the DescribeReplicationBackfillResults operation to check the \
         current status before retrying."
    )
}

/// A freshly minted `ReplicationBackfillId`: the UUID the modelled
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$` spells,
/// from the `uuid` crate's version-4 mint rather than a layout of this service's own.
///
/// Nothing in it is derived from the request: a backfill id is a server-generated handle, so
/// two starts of the same destination are two backfills.
fn mint_backfill_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}
