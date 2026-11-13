// SPDX-License-Identifier: CC0-1.0

//! The per-table settings that each carry a status of their own.
//!
//! Continuous backups and point-in-time recovery, contributor insights, time to live,
//! and the Kinesis destinations a table streams to: each one a record with its own
//! status vocabulary, its own transitions and its own refusals. They live beside
//! [`super`] rather than inside it because each is a SETTING of a table with a
//! lifecycle of its own, while the parent module is the table and item store itself;
//! `super` re-exports them, so every caller still names them `crate::state::…`.

use super::*;

/// The `ContinuousBackupsStatus` a table reports: "Continuous backups are ENABLED on
/// all tables at table creation" (the `UpdateContinuousBackups` doc), and nothing in
/// this API turns them off — `PointInTimeRecoverySpecification` carries only the
/// point-in-time recovery flag — so every table reports the same one of the two values
/// the model declares for that member
/// ([`crate::ops::types::CONTINUOUS_BACKUPS_STATUS_VALUES`]).
pub const CONTINUOUS_BACKUPS_STATUS: &str = "ENABLED";

/// The two values `PointInTimeRecoveryStatus` takes: "ENABLED - Point in time recovery
/// is enabled. DISABLED - Point in time recovery is disabled"
/// (`PointInTimeRecoveryDescription$PointInTimeRecoveryStatus`), which are the model's
/// own value table ([`crate::ops::types::POINT_IN_TIME_RECOVERY_STATUS_VALUES`]) named
/// here one per meaning, so a reader of this store sees which one a setting reports.
pub const POINT_IN_TIME_RECOVERY_ENABLED: &str = "ENABLED";
pub const POINT_IN_TIME_RECOVERY_DISABLED: &str = "DISABLED";

/// The recovery period an enable that names none settles on: "This parameter is
/// optional. If no value is provided, the value will default to 35"
/// (`PointInTimeRecoverySpecification$RecoveryPeriodInDays`).
pub const DEFAULT_RECOVERY_PERIOD_IN_DAYS: i32 = 35;

/// How far behind the present the newest restorable point sits:
/// "`LatestRestorableDateTime` is typically 5 minutes before the current time" (the
/// `UpdateContinuousBackups` doc and
/// `PointInTimeRecoveryDescription$LatestRestorableDateTime`).
pub const RESTORE_WINDOW_LAG: Duration = Duration::from_secs(5 * 60);

/// One day, the unit `RecoveryPeriodInDays` counts the recovery window in.
pub(super) const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// This emulator's own ceiling on how many item versions ONE table's recovery journal
/// keeps, and how many bytes of them.
///
/// Not an AWS quota: DynamoDB states none, because its continuous backups are not held in
/// memory. A journal entry is a copy of one item as it stood before a write, so without a
/// ceiling the versions of one hot key would grow with the writes a caller makes. Reaching
/// either one drops the OLDEST versions and moves the journal's horizon up with them
/// ([`RecoveryJournal::horizon`]), so the window a restore is offered never reaches
/// further back than the versions still held.
///
/// These bound ONE table. What bounds the PROCESS is that the bytes every journal retains
/// are part of the store-wide figure admission control measures against
/// ([`ComAmazonawsDynamodbDynamodb20120810State::stored_bytes`], against
/// [`MAX_STORE_SIZE_BYTES`]): a journal holds item data, so it is counted with the item
/// data it is a copy of rather than beside it, and a caller cannot grow the two together
/// past that one ceiling however many tables have continuous backups turned on.
pub const MAX_RECOVERED_ITEM_VERSIONS: usize = 20_000;
pub const MAX_RECOVERED_ITEM_BYTES: i64 = 64 * 1024 * 1024;

/// What ONE retained version costs beyond the item it holds: the PRIMARY KEY it is
/// addressed by, which a version carries whether it holds an item or not.
///
/// Charged as the documented maximum a key may carry ([`MAX_PARTITION_KEY_BYTES`] plus
/// [`MAX_SORT_KEY_BYTES`]) rather than measured per key: that is the upper bound on every
/// key this store admits, so counting it keeps the ceilings above an upper bound on what a
/// journal really holds — the versions whose `before` is `None` included, which retain a
/// key and no item.
pub const RECOVERED_VERSION_OVERHEAD_BYTES: i64 = MAX_PARTITION_KEY_BYTES + MAX_SORT_KEY_BYTES;

/// One item as it stood BEFORE one write landed on it, and when that write landed.
///
/// `before` is `None` for a write that created the item, which is what undoing it removes.
#[derive(Debug, Clone)]
struct ItemVersion {
    at: SystemTime,
    key: ItemKey,
    before: Option<Item>,
}

/// What a table's continuous backups HOLD, so a restore to a point in time can hand back
/// the table as it stood then: every item version a write replaced since recovery was
/// turned on, in the order the writes landed.
///
/// An UNDO log rather than a snapshot per instant: the table's current items are the one
/// copy of the data, and the state at an instant is reached by putting back every version
/// a later write replaced ([`Self::items_at`]). So the data a restore materialises is the
/// table's own, and a table nobody wrote to since recovery was turned on costs nothing.
#[derive(Debug, Clone, Default)]
pub struct RecoveryJournal {
    /// The versions, oldest first — the order the writes that replaced them landed in,
    /// which is the order [`Self::items_at`] walks back through.
    versions: VecDeque<ItemVersion>,
    /// The bytes of item data and retained keys the versions above carry, against
    /// [`MAX_RECOVERED_ITEM_BYTES`] and, together with the table's own items, against the
    /// store-wide ceiling.
    bytes: i64,
    /// The oldest instant this journal can still reproduce the table at: `None` while it
    /// holds every version since recovery was turned on, and the instant of the oldest
    /// version it had to DROP once it dropped one. A restorable window is held at or after
    /// this instant, so no restore is offered a point whose data is no longer here.
    horizon: Option<SystemTime>,
}

impl RecoveryJournal {
    /// What one version costs this journal: the item it holds, plus the key it is addressed
    /// by ([`RECOVERED_VERSION_OVERHEAD_BYTES`]).
    fn cost(before: Option<&Item>) -> i64 {
        RECOVERED_VERSION_OVERHEAD_BYTES.saturating_add(before.map_or(0, item_size))
    }

    /// The bytes this journal retains, which are part of the store's own total.
    fn retained_bytes(&self) -> i64 {
        self.bytes
    }

    /// Keep the version `at` replaced, and drop what is no longer reachable.
    fn record(&mut self, at: SystemTime, key: &ItemKey, before: Option<Item>, earliest: SystemTime) {
        self.bytes = self.bytes.saturating_add(Self::cost(before.as_ref()));
        self.versions.push_back(ItemVersion {
            at,
            key: key.clone(),
            before,
        });
        // A version replaced at or before the oldest restorable point is never put back
        // again — every point a restore may name is at or after it — so it is dropped
        // without moving the horizon: nothing became unreachable.
        while self
            .versions
            .front()
            .is_some_and(|version| version.at <= earliest)
        {
            self.drop_oldest();
        }
        // This emulator's own ceilings, which DO make the versions below them unreachable:
        // the horizon moves up to the oldest version still held.
        while self.versions.len() > MAX_RECOVERED_ITEM_VERSIONS
            || (self.bytes > MAX_RECOVERED_ITEM_BYTES && self.versions.len() > 1)
        {
            let dropped = self.drop_oldest();
            self.horizon = dropped.max(self.horizon);
        }
    }

    /// Drop the oldest version held, giving its bytes back to the budget.
    fn drop_oldest(&mut self) -> Option<SystemTime> {
        let dropped = self.versions.pop_front()?;
        self.bytes = self
            .bytes
            .saturating_sub(Self::cost(dropped.before.as_ref()));
        Some(dropped.at)
    }

    /// The oldest instant this journal can reproduce a table at, where it cannot reach
    /// back to the instant recovery was turned on.
    fn horizon(&self) -> Option<SystemTime> {
        self.horizon
    }

    /// `current` as it stood at `point`: every version a write landed AFTER that instant
    /// replaced, put back in reverse order, so the newest write is undone first and an
    /// item written several times lands on the version it held at `point`.
    fn items_at(
        &self,
        current: &BTreeMap<ItemKey, Item>,
        point: SystemTime,
    ) -> BTreeMap<ItemKey, Item> {
        let mut items = current.clone();
        for version in self.versions.iter().rev().filter(|held| held.at > point) {
            match &version.before {
                Some(before) => items.insert(version.key.clone(), before.clone()),
                None => items.remove(&version.key),
            };
        }
        items
    }
}

/// A table's point-in-time recovery settings: what `UpdateContinuousBackups` writes and
/// what both it and `DescribeContinuousBackups` report.
///
/// Only the SETTINGS are stored. The restorable window is DERIVED from them at every
/// read, because it moves with the clock rather than with any call: its newest point is
/// one [`RESTORE_WINDOW_LAG`] behind now and its oldest the configured period behind
/// that, bounded below by the instant recovery started — before which there is nothing
/// to restore from.
///
/// What a point inside that window ANSWERS WITH is the journal beside the settings
/// ([`RecoveryJournal`]): recovery being on is what makes an item version worth keeping,
/// so the versions live with the setting that asked for them and a disable drops them.
#[derive(Debug, Clone, Default)]
pub struct PointInTimeRecovery {
    /// When point-in-time recovery was last turned on, or `None` while it is off (which
    /// is how every table starts: this API is the only way to turn it on, and
    /// `CreateTable` carries no `PointInTimeRecoverySpecification`).
    pub(super) enabled_at: Option<SystemTime>,
    /// The recovery period in force while recovery is on, in days.
    pub(super) recovery_period_in_days: i32,
    /// Every item version a write has replaced since recovery was turned on, which is
    /// what a restore to a point inside the window is materialised from.
    journal: RecoveryJournal,
}

impl PointInTimeRecovery {
    /// Apply one `PointInTimeRecoverySpecification`.
    ///
    /// Turning recovery on while it is ALREADY on does not restart the window — the
    /// recoverable period is as old as the recovery that has been running — so only the
    /// period is re-stated; turning it off forgets where the window started, so a later
    /// enable starts a new one.
    pub fn apply(&mut self, enabled: bool, recovery_period_in_days: Option<i32>, now: SystemTime) {
        if enabled {
            self.enabled_at = Some(self.enabled_at.unwrap_or(now));
            self.recovery_period_in_days =
                recovery_period_in_days.unwrap_or(DEFAULT_RECOVERY_PERIOD_IN_DAYS);
        } else {
            self.enabled_at = None;
            self.recovery_period_in_days = 0;
            // Turning recovery off forgets where the window started, so the versions that
            // window was answered from are no longer anything's data: a later enable
            // starts a new window, and nothing can be restored to a point before it.
            self.journal = RecoveryJournal::default();
        }
    }

    /// Whether point-in-time recovery is on, which is what a restore from a point in
    /// time is served against ([`Self::restorable_window`] derives the window such a
    /// restore is held to, and answers `None` for exactly the tables this answers
    /// `false` for).
    pub fn enabled(&self) -> bool {
        self.enabled_at.is_some()
    }

    /// The restorable window at `now` — `(earliest, latest)` — while recovery is on,
    /// and `None` while it is off, because a table with recovery off has no point in
    /// time to restore to.
    ///
    /// DERIVED at every read rather than stored, because it moves with the clock: the
    /// newest point is one [`RESTORE_WINDOW_LAG`] behind now ("LatestRestorableDateTime
    /// is typically 5 minutes before the current time") and the oldest the configured
    /// period behind now, each held at or after the instant recovery started — before
    /// which there is nothing to restore from. The bounds are ordered by construction,
    /// so the window is never empty.
    pub fn restorable_window(&self, now: SystemTime) -> Option<(SystemTime, SystemTime)> {
        let enabled_at = self.enabled_at?;
        let latest = now
            .checked_sub(RESTORE_WINDOW_LAG)
            .unwrap_or(now)
            .max(enabled_at);
        let period = u32::try_from(self.recovery_period_in_days).unwrap_or_default();
        let earliest = now
            .checked_sub(DAY * period)
            .unwrap_or(now)
            .clamp(enabled_at, latest);
        // A window is only offered over the item versions the journal still holds: where
        // it had to drop the oldest of them, its horizon is the oldest point the table can
        // still be reproduced at, so that is where the window starts.
        let earliest = match self.journal.horizon() {
            Some(horizon) if horizon > earliest => horizon.min(latest),
            _ => earliest,
        };
        // Reported at the precision a timestamp carries on the wire, so each bound a
        // client reads back is exactly the instant it can send in again.
        Some((
            at_timestamp_precision(earliest),
            at_timestamp_precision(latest),
        ))
    }

    /// Keep the version of one item a write landed at `now` replaced, where recovery is on
    /// — what a restore to a point inside the window then hands back
    /// ([`Self::items_at`]).
    ///
    /// A table whose recovery is OFF has no restorable window at all, so nothing it holds
    /// could be read as of a past instant and no version is kept for it: `before` is
    /// borrowed and copied only where it is kept, so a write to such a table pays nothing.
    pub fn record_item_version(&mut self, now: SystemTime, key: &ItemKey, before: Option<&Item>) {
        if self.enabled_at.is_none() {
            return;
        }
        // The oldest point a restore may name, which is what makes an older version
        // unreachable — read before this version is added, since it does not move it.
        let Some((earliest, _)) = self.restorable_window(now) else {
            return;
        };
        self.journal.record(now, key, before.cloned(), earliest);
    }

    /// The bytes the versions this recovery kept retain, which the store counts with the
    /// item data they are copies of ([`RecoveryJournal`]), so one ceiling bounds both.
    pub fn retained_bytes(&self) -> i64 {
        self.journal.retained_bytes()
    }

    /// The items of a table that HOLDS `current`, as that table stood at `point`: what a
    /// restore to that point in time materialises, and what an export taken at it renders.
    ///
    /// Computed from the versions this recovery kept ([`RecoveryJournal::items_at`]), so an
    /// item written after `point` is absent from it and one written before it carries the
    /// value it held then — rather than the live table being handed over as a copy.
    pub fn items_at(
        &self,
        current: &BTreeMap<ItemKey, Item>,
        point: SystemTime,
    ) -> BTreeMap<ItemKey, Item> {
        self.journal.items_at(current, point)
    }

    /// The continuous-backups settings as the two continuous-backups operations report
    /// them: the table's continuous-backups status, and the point-in-time recovery
    /// description — the window only while recovery is on, since a table with recovery
    /// off has no point in time to restore to.
    pub fn describe(&self, now: SystemTime) -> ContinuousBackupsDescription {
        let recovery = match self.restorable_window(now) {
            Some((earliest, latest)) => PointInTimeRecoveryDescription {
                point_in_time_recovery_status: Some(POINT_IN_TIME_RECOVERY_ENABLED.to_string()),
                recovery_period_in_days: Some(self.recovery_period_in_days),
                earliest_restorable_date_time: Some(super::reported_date(earliest)),
                latest_restorable_date_time: Some(super::reported_date(latest)),
            },
            None => PointInTimeRecoveryDescription {
                point_in_time_recovery_status: Some(POINT_IN_TIME_RECOVERY_DISABLED.to_string()),
                ..PointInTimeRecoveryDescription::default()
            },
        };
        ContinuousBackupsDescription {
            continuous_backups_status: CONTINUOUS_BACKUPS_STATUS.to_string(),
            point_in_time_recovery_description: Some(recovery),
        }
    }
}

// =============================================================================
// CONTRIBUTOR INSIGHTS
//
// A table and each of its global secondary indexes carry their own setting: what
// `UpdateContributorInsights` writes, and what `DescribeContributorInsights` and
// `ListContributorInsights` report back.
// =============================================================================

/// The statuses a contributor insights setting is reported in, named one per meaning out
/// of the model's own value table
/// ([`crate::ops::types::CONTRIBUTOR_INSIGHTS_STATUS_VALUES`]).
///
/// `FAILED` is not among them: that status stands for a failure of the CloudWatch rules
/// behind the feature ("the most common exceptions for a FAILED status are:
/// LimitExceededException ... AccessDeniedException ..." — the `FailureException` doc),
/// and this store runs none, so no setting is ever reported in it.
pub const CONTRIBUTOR_INSIGHTS_ENABLING: &str = "ENABLING";
pub const CONTRIBUTOR_INSIGHTS_ENABLED: &str = "ENABLED";
pub const CONTRIBUTOR_INSIGHTS_DISABLING: &str = "DISABLING";
pub const CONTRIBUTOR_INSIGHTS_DISABLED: &str = "DISABLED";

/// The two values `ContributorInsightsAction` declares
/// ([`crate::ops::types::CONTRIBUTOR_INSIGHTS_ACTION_VALUES`]), named one per meaning.
pub const CONTRIBUTOR_INSIGHTS_ENABLE: &str = "ENABLE";
pub const CONTRIBUTOR_INSIGHTS_DISABLE: &str = "DISABLE";

/// The mode an enable that names none settles on: the wider of the two the model
/// declares, which tracks "all access and throttled events" rather than "throttled
/// events only" (`UpdateContributorInsights$ContributorInsightsMode`) — an enable that
/// asked for no narrowing asked for neither half to be dropped.
pub const DEFAULT_CONTRIBUTOR_INSIGHTS_MODE: &str = "ACCESSED_AND_THROTTLED_KEYS";

/// The narrower of the two modes the model declares
/// ([`crate::ops::types::CONTRIBUTOR_INSIGHTS_MODE_VALUES`]): "throttled events only for the
/// DynamoDB table or index" (`UpdateContributorInsights$ContributorInsightsMode`), which is
/// what makes it track one half of the key activity the wider mode does
/// ([`crate::state::contributor_insights`]).
pub const CONTRIBUTOR_INSIGHTS_MODE_THROTTLED_KEYS: &str = "THROTTLED_KEYS";

/// The separator between the table name and the index name in the key one contributor
/// insights summary pages by ([`contributor_insights_key`]).
///
/// A table name and an index name are `[a-zA-Z0-9_.-]` only (the `TableName` and
/// `IndexName` patterns), so this character can occur in neither: it cannot be mistaken
/// for part of a name, and it sorts below every character one can carry, which keeps a
/// table's own summary immediately ahead of its indexes' in the one ascending order
/// [`page_by_name`] walks.
pub(super) const CONTRIBUTOR_INSIGHTS_KEY_SEPARATOR: char = '!';

/// The name one contributor insights summary is ordered and resumed by: the table it
/// belongs to, and the index after it where the summary is an index's.
pub fn contributor_insights_key(table: &str, index: Option<&str>) -> String {
    match index {
        Some(index) => format!("{table}{CONTRIBUTOR_INSIGHTS_KEY_SEPARATOR}{index}"),
        None => table.to_string(),
    }
}

/// The contributor insights setting of ONE table or index: what the last accepted
/// `UpdateContributorInsights` asked for, and when.
///
/// Only the REQUEST is stored; the status is derived at every read, because turning
/// insights on or off is asynchronous — the call leaves the setting `ENABLING` /
/// `DISABLING` and the clock owes it the move to `ENABLED` / `DISABLED` one
/// [`STATUS_TRANSITION`] later. A resource no request has ever named is `DISABLED` with
/// no timestamp at all: this API is the only way to turn insights on, and `CreateTable`
/// carries no contributor insights member.
#[derive(Debug, Clone, Default)]
pub struct ContributorInsights {
    /// Whether the last accepted action was `ENABLE`.
    pub(super) enabled: bool,
    /// When that action was accepted, or `None` while no request has named this
    /// resource.
    pub(super) requested_at: Option<SystemTime>,
    /// When the clock owes the move out of the transient status.
    pub(super) settles_at: Option<SystemTime>,
    /// The mode in force while insights are on, as the request named it (or
    /// [`DEFAULT_CONTRIBUTOR_INSIGHTS_MODE`] where it named none). A `DISABLE` clears
    /// it: a setting that tracks nothing is tracking neither kind of event.
    pub(super) mode: Option<String>,
}

impl ContributorInsights {
    /// Apply one `ContributorInsightsAction`, with the mode an enable names.
    ///
    /// The action is accepted whether or not it changes anything — re-stating `ENABLE`
    /// re-states the mode and starts the span again — so what a later read reports is
    /// always the last request's.
    pub fn apply(&mut self, enable: bool, mode: Option<&str>, now: SystemTime) {
        self.enabled = enable;
        self.requested_at = Some(now);
        self.settles_at = Some(now + STATUS_TRANSITION);
        self.mode = enable.then(|| {
            mode.unwrap_or(DEFAULT_CONTRIBUTOR_INSIGHTS_MODE)
                .to_string()
        });
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        let settled = self.settles_at.is_none_or(|due| now >= due);
        match (self.enabled, settled) {
            (true, true) => CONTRIBUTOR_INSIGHTS_ENABLED,
            (true, false) => CONTRIBUTOR_INSIGHTS_ENABLING,
            (false, true) => CONTRIBUTOR_INSIGHTS_DISABLED,
            (false, false) => CONTRIBUTOR_INSIGHTS_DISABLING,
        }
    }

    /// "Timestamp of the last time the status was changed"
    /// (`DescribeContributorInsightsOutput$LastUpdateDateTime`): the instant the status
    /// this read reports was reached — the call's own instant while the move is still
    /// owed, and the instant it fell due once it has. `None` for a resource no request
    /// has named, whose status has never changed.
    pub fn last_update(&self, now: SystemTime) -> Option<SystemTime> {
        match self.settles_at {
            Some(due) if now >= due => Some(due),
            _ => self.requested_at,
        }
    }

    /// The mode in force, which a setting that is off carries none of.
    pub fn mode(&self) -> Option<String> {
        self.mode.clone()
    }

    /// Whether this resource is COLLECTING key activity: the feature's CloudWatch rules exist
    /// from the enable that minted them ([`crate::state::contributor_insights`]), so an access
    /// contributes to them from that instant on and stops contributing when a disable deletes
    /// them — which is the last accepted action either way, whether or not the status it leaves
    /// has settled.
    pub fn collecting(&self) -> bool {
        self.enabled
    }

    /// Whether this setting is one `ListContributorInsights` reports a summary for.
    ///
    /// The listing enumerates the contributor insights CONFIGURATIONS that exist — the
    /// resources insights are being turned on for, are on for, or are still being turned
    /// off for. A resource whose status has settled `DISABLED` has no configuration, so
    /// it carries no summary: it drops out of the listing once a `DISABLE` settles, and a
    /// resource no update has ever named was never in it. `DescribeContributorInsights`
    /// is what answers for a named resource either way, and reports `DISABLED` for it.
    pub fn listed(&self, now: SystemTime) -> bool {
        self.status(now) != CONTRIBUTOR_INSIGHTS_DISABLED
    }
}

// =============================================================================
// TIME TO LIVE
//
// One setting per table: whether items expire, and the attribute whose epoch-seconds
// value says when. `UpdateTimeToLive` writes it, `DescribeTimeToLive` reports it, and
// the store's own sweep ([`Table::apply_due_expirations`]) removes the items whose
// deletion the clock has already made due.
// =============================================================================

/// The four values `TimeToLiveStatus` declares
/// ([`crate::ops::types::TIME_TO_LIVE_STATUS_VALUES`]), named one per meaning: turning
/// a table's TTL on or off is asynchronous ("It can take up to one hour for the change
/// to fully process" — the `UpdateTimeToLive` doc), so the setting is reported in a
/// transient status until the clock has settled it.
pub const TIME_TO_LIVE_ENABLING: &str = "ENABLING";
pub const TIME_TO_LIVE_ENABLED: &str = "ENABLED";
pub const TIME_TO_LIVE_DISABLING: &str = "DISABLING";
pub const TIME_TO_LIVE_DISABLED: &str = "DISABLED";

/// How long after an item has expired its deletion falls due, and the shortest span
/// between two sweeps of one table.
///
/// DynamoDB states no fixed figure for this one: "DynamoDB deletes expired items on a
/// best-effort basis ... DynamoDB typically deletes expired items within two days of
/// expiration. The exact duration within which an item gets deleted after expiration is
/// specific to the nature of the workload" (the `UpdateTimeToLive` doc), which is an
/// upper bound on a best-effort sweep rather than a duration to keep. This emulator
/// therefore CHOOSES 500 ms: non-zero, so an item that has just expired is still served
/// to a reader ("Items that have expired and not been deleted will still show up in
/// reads, queries, and scans" — the same doc), and short enough that a client that waits
/// sees the deletion. It doubles as the sweep interval, so the walk one request can
/// trigger over a table's items runs at most once per span however many requests arrive.
pub const TTL_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How a call that turns TTL on while it is already on is refused: AWS's FIXED sentence
/// for that case, which names neither the table nor the attribute.
///
/// This is the refusal for a call that asks for the setting the table ALREADY has — the
/// same attribute, enabled again. A call naming another attribute is a different
/// refusal, which reports the attribute in force
/// ([`time_to_live_attribute_mismatch`]).
pub const TIME_TO_LIVE_ALREADY_ENABLED_MESSAGE: &str = "TimeToLive is already enabled";

/// The same for a call that turns TTL off while it is already off.
pub const TIME_TO_LIVE_ALREADY_DISABLED_MESSAGE: &str = "TimeToLive is already disabled";

/// How a specification that names an attribute OTHER than the one TTL is in force for is
/// refused: AWS's sentence for that case, whose variable part is the attribute this
/// table's items are currently expired by, read from the table's stored setting.
///
/// The TTL attribute of a table whose TTL is on is not something this API moves: a
/// specification naming another attribute asks for a TTL over an attribute that is not
/// the one in force, so it is refused and the stored one is reported back — whether the
/// specification was enabling TTL or disabling it, because either way the attribute it
/// names is not this table's.
pub fn time_to_live_attribute_mismatch(attribute_in_force: &str) -> String {
    format!(
        "TimeToLive is active on a different AttributeName: current AttributeName is \
         {attribute_in_force}"
    )
}

/// How a call that arrives while the last TTL change is still rate-limited is refused:
/// "Any additional UpdateTimeToLive calls for the same table during this one hour
/// duration result in a ValidationException" (the `UpdateTimeToLive` doc).
pub const TIME_TO_LIVE_MODIFIED_TOO_OFTEN_MESSAGE: &str =
    "Time to live has been modified multiple times within a fixed interval";

/// How long after one accepted `UpdateTimeToLive` the NEXT one is refused.
///
/// A documented duration, kept as documented: "It can take up to one hour for the change
/// to fully process. Any additional UpdateTimeToLive calls for the same table during this
/// one hour duration result in a ValidationException" (the `UpdateTimeToLive` doc). It is
/// NOT the span the setting's transient status is reported for ([`STATUS_TRANSITION`]):
/// the status settles on its own clock — a table reports `ENABLED` well before the hour is
/// out — while a second change stays refused for the whole interval.
pub const TTL_MODIFICATION_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// One table's Time to Live setting: what the last accepted `UpdateTimeToLive` asked
/// for, when it was accepted, and when this table's items were last swept.
///
/// Only the REQUEST is stored; the status is derived at every read, for the reason
/// [`ContributorInsights`]'s is — the change is asynchronous, so the call leaves the
/// setting `ENABLING` / `DISABLING` and the clock owes it the move to `ENABLED` /
/// `DISABLED` one [`STATUS_TRANSITION`] later. A table no `UpdateTimeToLive` has named
/// is `DISABLED` with no attribute at all: this API is the only way to turn TTL on, and
/// `CreateTable` carries no TTL member.
#[derive(Debug, Clone, Default)]
pub struct TimeToLive {
    /// Whether the last accepted specification turned TTL on.
    pub(super) enabled: bool,
    /// The attribute that specification named — the one whose value items expire by
    /// while TTL is on, and the one being withdrawn while it is going off. `None` until
    /// a request has named one.
    pub(super) attribute_name: Option<String>,
    /// When the clock owes the move out of the transient status.
    pub(super) settles_at: Option<SystemTime>,
    /// When the last accepted change was made, or `None` while no request has named this
    /// table — the instant the [`TTL_MODIFICATION_INTERVAL`] a further change is refused
    /// inside is counted from.
    pub(super) changed_at: Option<SystemTime>,
    /// When this table's items were last swept for expiries, or `None` while they never
    /// have been.
    pub(super) swept_at: Option<SystemTime>,
}

impl TimeToLive {
    /// Apply one `TimeToLiveSpecification`: the attribute it names comes into force (or
    /// starts being withdrawn) and the transient span starts again.
    pub fn apply(&mut self, enabled: bool, attribute_name: &str, now: SystemTime) {
        self.enabled = enabled;
        self.attribute_name = Some(attribute_name.to_string());
        self.settles_at = Some(now + STATUS_TRANSITION);
        self.changed_at = Some(now);
    }

    /// Whether this table's TTL setting may be changed at `now`: a change is accepted only
    /// once the [`TTL_MODIFICATION_INTERVAL`] since the last accepted one has passed, and a
    /// table no change has ever been made to is always modifiable.
    pub fn modifiable(&self, now: SystemTime) -> bool {
        self.changed_at
            .is_none_or(|changed| now >= changed + TTL_MODIFICATION_INTERVAL)
    }

    /// Whether the clock has already settled the last change this setting was given.
    pub fn settled(&self, now: SystemTime) -> bool {
        self.settles_at.is_none_or(|due| now >= due)
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match (self.enabled, self.settled(now)) {
            (true, true) => TIME_TO_LIVE_ENABLED,
            (true, false) => TIME_TO_LIVE_ENABLING,
            (false, true) => TIME_TO_LIVE_DISABLED,
            (false, false) => TIME_TO_LIVE_DISABLING,
        }
    }

    /// The attribute this table's items are expired by RIGHT NOW: the one the settled
    /// enable named, and `None` for a table whose TTL is off or still settling.
    pub fn attribute_in_force(&self, now: SystemTime) -> Option<&str> {
        match self.status(now) {
            TIME_TO_LIVE_ENABLED => self.attribute_name.as_deref(),
            _ => None,
        }
    }

    /// Project this setting into the `TimeToLiveDescription` the TTL reader reports:
    /// the status, and the attribute it is in force for.
    ///
    /// A setting that has settled OFF reports no attribute: nothing expires items on
    /// that table, so there is no TTL attribute for it to name — which is also what a
    /// table that has never had TTL reports.
    pub fn describe(&self, now: SystemTime) -> TimeToLiveDescription {
        let status = self.status(now);
        TimeToLiveDescription {
            time_to_live_status: Some(status.to_string()),
            attribute_name: match status {
                TIME_TO_LIVE_DISABLED => None,
                _ => self.attribute_name.clone(),
            },
        }
    }

    /// The attribute a sweep that is DUE at `now` expires items by, or `None` when this
    /// table has nothing to sweep yet.
    ///
    /// A sweep is due only while TTL is in force — a change that is still settling
    /// expires nothing, because the attribute it names is not the one items are being
    /// deleted by yet — and at most once per [`TTL_DELETION_DELAY`].
    pub(super) fn due_sweep_attribute(&self, now: SystemTime) -> Option<String> {
        let in_force = self.status(now) == TIME_TO_LIVE_ENABLED;
        let due = self
            .swept_at
            .is_none_or(|last| now >= last + TTL_DELETION_DELAY);
        (in_force && due)
            .then(|| self.attribute_name.clone())
            .flatten()
    }

    /// Record that this table's items have just been swept.
    pub(super) fn mark_swept(&mut self, now: SystemTime) {
        self.swept_at = Some(now);
    }
}

/// The epoch-seconds instant an item's TTL attribute must not be LATER than for its
/// deletion to be due at `now`: one [`TTL_DELETION_DELAY`] back, as an `N` value's text,
/// so the comparison runs through the same number parser a stored `N` was canonicalized
/// by ([`NumberKey`]).
pub(super) fn expiry_cutoff(now: SystemTime) -> Option<NumberKey> {
    let delay = i128::try_from(TTL_DELETION_DELAY.as_millis()).unwrap_or(i128::MAX);
    let millis = epoch_millis(now).saturating_sub(delay);
    let sign = if millis < 0 { "-" } else { "" };
    let magnitude = millis.unsigned_abs();
    NumberKey::parse(&format!(
        "{sign}{}.{:03}",
        magnitude / 1_000,
        magnitude % 1_000
    ))
}

/// Whether one item's TTL attribute value has already made the item's deletion due.
///
/// "TTL compares the current time in epoch time format to the time stored in the TTL
/// attribute of an item. If the epoch time value stored in the attribute is less than
/// the current time, the item is marked as expired" (the `UpdateTimeToLive` doc): the
/// value is a NUMBER of seconds since the epoch, so an attribute of any other type
/// expires nothing — a string, a set or a boolean states no epoch time to compare.
pub(super) fn expiry_due(value: &AttributeValue, cutoff: &NumberKey) -> bool {
    match value {
        AttributeValue::N(text) => {
            NumberKey::parse(text).is_some_and(|expires_at| expires_at <= *cutoff)
        }
        _ => false,
    }
}

// =============================================================================
// KINESIS DATA STREAM DESTINATIONS
//
// A table replicates its item changes to a Kinesis data stream through a
// DESTINATION record it carries: the enable command registers one, the update
// command changes the timestamp precision of a registered one, the disable
// command stops it, and the describing operation reports every one the table
// holds. Each record below is what those operations read and report; no value
// they answer with is canned.
// =============================================================================

/// The `DestinationStatus` values a destination record moves between
/// ([`crate::ops::types::DESTINATION_STATUS_VALUES`]), named one per meaning.
///
/// Registering, updating and stopping replication are all asynchronous, so a
/// destination is reported in a transient status (`ENABLING`, `UPDATING`,
/// `DISABLING`) for a span and in the settled one (`ACTIVE`, `DISABLED`) after it —
/// the same clock-owed move every other status in this store makes
/// ([`STATUS_TRANSITION`]).
pub const DESTINATION_STATUS_ENABLING: &str = "ENABLING";
pub const DESTINATION_STATUS_ACTIVE: &str = "ACTIVE";
pub const DESTINATION_STATUS_UPDATING: &str = "UPDATING";
pub const DESTINATION_STATUS_DISABLING: &str = "DISABLING";
pub const DESTINATION_STATUS_DISABLED: &str = "DISABLED";
/// The status an enable that cannot be completed settles in — "ENABLE_FAILED" exists for
/// a replication the service could not start, which is what a `StreamArn` naming a stream
/// that does not exist asks for ([`kinesis_stream_exists`]).
pub const DESTINATION_STATUS_ENABLE_FAILED: &str = "ENABLE_FAILED";

/// The two units `ApproximateCreationDateTimePrecision` declares
/// ([`crate::ops::types::APPROXIMATE_CREATION_DATE_TIME_PRECISION_VALUES`]) — "the precision
/// of the Kinesis data stream timestamp. The values are either MILLISECOND or MICROSECOND"
/// (`KinesisDataStreamDestination$ApproximateCreationDateTimePrecision`) — named one per
/// meaning, because the unit of the creation time a delivered record carries is this value
/// ([`super::kinesis_delivery`]).
pub const DESTINATION_PRECISION_MILLISECOND: &str = "MILLISECOND";
pub const DESTINATION_PRECISION_MICROSECOND: &str = "MICROSECOND";

/// How DynamoDB opens its refusal of a command against a table's Kinesis streaming
/// destination that the table's state cannot serve; the reason follows the colon.
pub(super) const KINESIS_DESTINATION_INVALID_STATE: &str =
    "Table is not in a valid state to enable Kinesis Streaming Destination";

/// How a command that addresses ONE of a table's Kinesis streaming destinations is
/// refused when the table holds NO destination record for that stream at all: the stream
/// ARN the request named and the table it named it against, under the invalid-state
/// sentence above.
///
/// This is the lookup miss proper — the table has no record of ever replicating to that
/// stream. A record that EXISTS but is not `ACTIVE` is a state refusal instead
/// ([`kinesis_destination_not_active_to_update`]), because the two are different causes
/// for the caller: one is terminal, the other is waited out or re-enabled.
pub fn kinesis_destination_not_found(table: &str, stream_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: No streaming destination with streamArn: \
         {stream_arn} found for table with tableName: {table}"
    ))
}

/// How the command that STOPS replication is refused when the destination it addresses is
/// not `ACTIVE`: replication can only be stopped once it is running, and the refusal is
/// stated over that required status alone — it names neither the table nor the stream.
///
/// The same sentence answers every status that is not `ACTIVE`, a table that holds no
/// record for the stream included: in each case there is no running replication to stop.
pub fn kinesis_destination_not_active_to_disable() -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: KinesisStreamingDestination must be ACTIVE to \
         perform DISABLE operation."
    ))
}

/// How the command that CHANGES a destination's configuration is refused when the
/// destination it addresses exists but is not `ACTIVE`: the status the change requires,
/// over the table name and the Kinesis stream ARN the request addressed the destination
/// by (`kdsArn`).
pub fn kinesis_destination_not_active_to_update(table: &str, stream_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: Kinesis streaming is not in ACTIVE state. Updates \
         are only allowed in ACTIVE state. TableName: {table}, kdsArn: {stream_arn}"
    ))
}

/// Why an enable could not be completed, which is what the failed destination's
/// `DestinationStatusDescription` reports — "the human-readable string that corresponds to
/// the replica status".
///
/// A destination IS a Kinesis data stream, so every reason an enable settles
/// `ENABLE_FAILED` is a reason the `StreamArn` resolves to no stream this table could
/// replicate to, and each is told apart for the caller: a value that is not a stream ARN at
/// all, an ARN of a stream in another Region, and an ARN no stream of this Region carries.
#[derive(Debug, Clone)]
pub enum KinesisEnableFailure {
    /// The `StreamArn` does not name a Kinesis data stream
    /// ([`parse_kinesis_stream_arn`]).
    NotAStreamArn,
    /// It names a stream of another Region, which no table of this one replicates to.
    AnotherRegion(String),
    /// It is the ARN of a Kinesis data stream, and this Region's Kinesis endpoint holds no
    /// stream under it.
    NoSuchStream,
}

impl KinesisEnableFailure {
    /// The human-readable description of this failure, over the `StreamArn` the enable
    /// named: what the destination reports for as long as it carries the failed status.
    pub fn description(&self, stream_arn: &str) -> String {
        let reason = match self {
            Self::NotAStreamArn => {
                format!("{stream_arn} is not the ARN of a Kinesis data stream")
            }
            Self::AnotherRegion(region) => format!(
                "the Kinesis data stream must be in the same Region as the table, and \
                 {stream_arn} is in {region}"
            ),
            Self::NoSuchStream => {
                format!("there is no Kinesis data stream with ARN {stream_arn}")
            }
        };
        format!("Failed to enable Kinesis streaming destination: {reason}")
    }
}

/// How the command that STARTS replication is refused when the table's streaming state
/// cannot serve it.
///
/// A table replicates to AT MOST ONE Kinesis data stream, so an enable is served only
/// while the table has no LIVE destination: whether the request names the stream the table
/// already replicates to or another one, the table's own streaming state is the same, and
/// so is the refusal. It is stated over the statuses an enable REQUIRES, the way the stop
/// command's refusal is stated over the status it requires
/// ([`kinesis_destination_not_active_to_disable`]): it names neither the table nor the
/// stream. `ENABLE_FAILED` is named beside `DISABLED` because AWS states both as the
/// statuses an enable may start from, and this store reaches both: a destination whose
/// stream could not be resolved settles `ENABLE_FAILED`, and an enable may be retried
/// from there.
pub fn kinesis_destination_not_enableable() -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: EnableKinesisStreamingDestination must be \
         DISABLED or ENABLE_FAILED to perform ENABLE operation."
    ))
}

/// How many Kinesis data stream destination records ONE table keeps.
///
/// The list grows by one per DISTINCT stream a command registers — a stream the table
/// already holds a record for is re-registered in place — and the stopped records are
/// the replication history the describing operation reports. AWS publishes no figure for
/// that history, so this store CHOOSES 64: room for a table's replication history to be
/// reported in full, and a bound, so no sequence of commands grows one table's record
/// list without end.
pub const MAX_KINESIS_DESTINATIONS_PER_TABLE: usize = 64;

/// The form a Kinesis data stream's ARN takes
/// (`arn:<partition>:kinesis:<region>:<account>:stream/<name>`), answering the Region the
/// stream it names belongs to and the stream's own name.
///
/// `None` for a value that names no Kinesis data stream. The `StreamArn` member is
/// modelled as a length-bounded string only, so a value that clears that bound is ACCEPTED
/// by every command that carries one: what it resolves to is answered here and against the
/// Kinesis service's own registry ([`kinesis_stream_exists`]), rather than refused as
/// unreadable input — the enable settles `ENABLE_FAILED` for a `StreamArn` that resolves to
/// no stream, and the commands that address an EXISTING destination answer their own
/// lookup miss.
pub fn parse_kinesis_stream_arn(stream_arn: &str) -> Option<(String, String)> {
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = stream_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return None,
    };
    if partition.is_empty() || service != "kinesis" || region.is_empty() {
        return None;
    }
    let name = resource.strip_prefix("stream/")?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some((region.to_string(), name.to_string()))
}

/// One Kinesis data stream destination of a table: the stream it replicates to, where
/// that replication is in its lifecycle, and the timestamp precision in force.
///
/// Only the last accepted command is stored; the status is derived at every read from
/// the clock, exactly as [`ContributorInsights`] derives its own.
#[derive(Debug, Clone)]
pub struct KinesisDestination {
    /// "The ARN for a specific Kinesis data stream"
    /// (`KinesisDataStreamDestination$StreamArn`) — the identity a command addresses
    /// this record by.
    pub stream_arn: String,
    /// The status this record settles on once the span has elapsed.
    pub(super) settled_status: &'static str,
    /// The status it reports until then.
    pub(super) transient_status: &'static str,
    /// When the clock owes the move from the transient status to the settled one.
    pub(super) settles_at: SystemTime,
    /// "The precision of the Kinesis data stream timestamp"
    /// (`KinesisDataStreamDestination$ApproximateCreationDateTimePrecision`), or `None`
    /// while no command has named one: the member is absent until one does.
    pub(super) precision: Option<String>,
    /// Why the enable this record came from could not be completed, for a registration
    /// headed for `ENABLE_FAILED` — what the failed destination reports as its
    /// `DestinationStatusDescription` ([`KinesisEnableFailure::description`]). `None` for a
    /// registration that replicates: there is no failure to describe.
    pub(super) status_description: Option<String>,
}

impl KinesisDestination {
    /// Register one destination, as the command that starts replication leaves it:
    /// `ENABLING` for the span, then `ACTIVE`.
    pub fn enabling(stream_arn: &str, precision: Option<&str>, now: SystemTime) -> Self {
        Self {
            stream_arn: stream_arn.to_string(),
            settled_status: DESTINATION_STATUS_ACTIVE,
            transient_status: DESTINATION_STATUS_ENABLING,
            settles_at: now + STATUS_TRANSITION,
            precision: precision.map(str::to_string),
            status_description: None,
        }
    }

    /// The same registration for an enable that CANNOT be completed: the record is
    /// `ENABLING` for the span, like any other, and settles in `ENABLE_FAILED` instead of
    /// `ACTIVE`, carrying the human-readable description of `failure`.
    ///
    /// Nothing replicates to a stream that does not resolve, so the destination never
    /// reaches `ACTIVE`; the status that says so is the one the model declares for it, the
    /// description is what tells the caller WHY, and a later enable may start again from it
    /// ([`kinesis_destination_not_enableable`]).
    pub fn enable_failing(
        stream_arn: &str,
        precision: Option<&str>,
        failure: &KinesisEnableFailure,
        now: SystemTime,
    ) -> Self {
        Self {
            settled_status: DESTINATION_STATUS_ENABLE_FAILED,
            status_description: Some(failure.description(stream_arn)),
            ..Self::enabling(stream_arn, precision, now)
        }
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.settles_at {
            self.transient_status
        } else {
            self.settled_status
        }
    }

    /// Whether this record still REPLICATES — the destination a command that addresses
    /// a table's live replication to this stream acts on. A record whose replication has
    /// stopped, or is stopping, is history: it is still reported, and no longer
    /// addressable. A record whose enable FAILED never replicated at all, so it is not
    /// live either, and an enable may start again from it.
    pub fn replicating(&self, now: SystemTime) -> bool {
        let status = self.status(now);
        status != DESTINATION_STATUS_DISABLED
            && status != DESTINATION_STATUS_DISABLING
            && status != DESTINATION_STATUS_ENABLE_FAILED
    }

    /// Whether this record is `ACTIVE` at `now` — replication running, with no
    /// asynchronous change of its own still owed.
    ///
    /// This is the status a command that CHANGES the destination requires: while a
    /// registration, a configuration change or a stop is still in flight the destination
    /// is in a transient status, and the change is refused on that status.
    pub fn active(&self, now: SystemTime) -> bool {
        self.status(now) == DESTINATION_STATUS_ACTIVE
    }

    /// Whether a change made at `now` is DELIVERED to this destination's stream.
    ///
    /// Replication runs once the registration has settled, and keeps running while a
    /// configuration change of its own is still owed, so a record that is `ACTIVE` or
    /// `UPDATING` takes the item changes its table's writes make. A registration that has
    /// not settled yet delivers nothing — replication starts "at a timestamp chosen during
    /// the enable workflow" (the `EnableKinesisStreamingDestination` doc) — and neither
    /// does one that failed, or one whose replication has been stopped.
    pub fn delivers(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            DESTINATION_STATUS_ACTIVE | DESTINATION_STATUS_UPDATING
        )
    }

    /// The unit the creation time of a delivered record is stated in: the precision in
    /// force, and — for a destination no command has named one for — the service's own
    /// default, which is the enum's `MILLISECOND`.
    pub fn precision_in_force(&self) -> &str {
        self.precision
            .as_deref()
            .unwrap_or(DESTINATION_PRECISION_MILLISECOND)
    }

    /// Apply one update of this destination's configuration: the record is `UPDATING`
    /// for the span and `ACTIVE` after it, carrying the precision the command named.
    ///
    /// A command that names no precision changes none — it asks for no configuration
    /// change — and still restarts the span, because the service accepted the command.
    pub fn apply_update(&mut self, precision: Option<&str>, now: SystemTime) {
        if let Some(precision) = precision {
            self.precision = Some(precision.to_string());
        }
        self.settled_status = DESTINATION_STATUS_ACTIVE;
        self.transient_status = DESTINATION_STATUS_UPDATING;
        self.settles_at = now + STATUS_TRANSITION;
    }

    /// The timestamp precision in force, which a destination no command has named one
    /// for carries none of.
    pub fn precision(&self) -> Option<String> {
        self.precision.clone()
    }

    /// Apply the command that STOPS replication: the record is `DISABLING` for the span
    /// and `DISABLED` after it, keeping the stream it replicated to and the precision it
    /// replicated under — a stopped destination is still reported, as this table's
    /// replication history.
    pub fn apply_disable(&mut self, now: SystemTime) {
        self.settled_status = DESTINATION_STATUS_DISABLED;
        self.transient_status = DESTINATION_STATUS_DISABLING;
        self.settles_at = now + STATUS_TRANSITION;
    }

    /// Project this record into the `KinesisDataStreamDestination` the describing
    /// operation reports.
    ///
    /// `DestinationStatusDescription` is "the human-readable string that corresponds to
    /// the replica status" — a description of a FAILURE, which a record that replicates
    /// has none of, so the member is reported only while the record carries the failed
    /// status and stays absent for every other one.
    pub fn describe(&self, now: SystemTime) -> KinesisDataStreamDestination {
        let status = self.status(now);
        KinesisDataStreamDestination {
            stream_arn: Some(self.stream_arn.clone()),
            destination_status: Some(status.to_string()),
            destination_status_description: self
                .status_description
                .clone()
                .filter(|_| status == DESTINATION_STATUS_ENABLE_FAILED),
            approximate_creation_date_time_precision: self.precision(),
        }
    }
}

/// The settings' own clock-driven transitions, asserted over instants a WIRE test cannot
/// reach: the TTL modification interval is an hour, so the state a second change is served
/// from — and the statuses a change that is served leaves behind — are stated here, where
/// the instant one was accepted at can be put in the past.
#[cfg(test)]
mod tests {
    use super::*;

    /// One instant the cases below are stated about, so no case depends on the clock
    /// advancing while it runs.
    fn at(seconds_ago: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 - seconds_ago)
    }

    #[test]
    fn a_change_is_served_only_once_the_modification_interval_has_passed() {
        let now = at(0);
        let mut setting = TimeToLive::default();
        // A table no change has ever been made to takes one.
        assert!(
            setting.modifiable(now),
            "a setting no change has been made to must be modifiable, got {setting:?}"
        );

        setting.apply(true, "expires_at", now);
        // Rows: the instant of the change, every instant inside the interval, the instant
        // the interval falls due, and one past it.
        for (case, asked, modifiable) in [
            ("at the change itself", now, false),
            ("one second in", now + Duration::from_secs(1), false),
            (
                "one second before the interval is out",
                now + TTL_MODIFICATION_INTERVAL - Duration::from_secs(1),
                false,
            ),
            ("as the interval falls due", now + TTL_MODIFICATION_INTERVAL, true),
            (
                "past the interval",
                now + TTL_MODIFICATION_INTERVAL + Duration::from_secs(1),
                true,
            ),
        ] {
            assert_eq!(
                setting.modifiable(asked),
                modifiable,
                "case {case:?}: expected modifiable={modifiable}, got {setting:?}"
            );
        }
    }

    #[test]
    fn a_disable_settles_the_setting_off_on_the_status_clock() {
        // TTL turned on an interval ago, so the disable below is a change the setting takes.
        let enabled_at = at(TTL_MODIFICATION_INTERVAL.as_secs());
        let now = at(0);
        let mut setting = TimeToLive::default();
        setting.apply(true, "expires_at", enabled_at);
        assert_eq!(
            (setting.status(now), setting.attribute_in_force(now)),
            (TIME_TO_LIVE_ENABLED, Some("expires_at")),
            "case \"the settled enable\": expected ENABLED on its attribute, got {setting:?}"
        );

        setting.apply(false, "expires_at", now);
        // The status clock is the transient span, not the modification interval: the
        // setting is DISABLING for that span and DISABLED after it, and a setting that has
        // settled off expires nothing and reports no attribute.
        for (case, asked, status, in_force, attribute) in [
            (
                "while the disable is processing",
                now,
                TIME_TO_LIVE_DISABLING,
                None,
                Some("expires_at"),
            ),
            (
                "as the transient span falls due",
                now + STATUS_TRANSITION,
                TIME_TO_LIVE_DISABLED,
                None,
                None,
            ),
        ] {
            let described = setting.describe(asked);
            assert_eq!(
                (
                    setting.status(asked),
                    setting.attribute_in_force(asked),
                    described.attribute_name.as_deref(),
                ),
                (status, in_force, attribute),
                "case {case:?}: expected {status} on {attribute:?}, got {described:?}"
            );
        }

        // And the disable itself starts the interval again: the setting takes no further
        // change until it is out.
        assert!(
            !setting.modifiable(now + STATUS_TRANSITION),
            "case \"after the disable settled\": the interval runs from the change, got \
             {setting:?}"
        );
    }
}
