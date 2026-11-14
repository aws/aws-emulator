// SPDX-License-Identifier: CC0-1.0

//! # Instance event windows
//!
//! [`ComAmazonawsEc2Amazonec2State::instance_event_windows`] holds the account's event
//! windows as [`InstanceEventWindowRecord`]s: the modelled [`InstanceEventWindow`] every
//! operation that reports one answers with, plus the instant the clock owes the window its
//! `creating` -> `active` move — the one member of the view that is a fact of the window's
//! age rather than of the request, so [`InstanceEventWindowRecord::view_at`] derives it on
//! read instead of letting two writers store two different words for it, exactly as
//! [`VolumeRecord`] does for a volume. Keyed by `InstanceEventWindowId` in a
//! [`std::collections::BTreeMap`], the same stable order the VPC, subnet and volume stores
//! are kept in, so a pagination token can name the id the next page starts at.
//!
//! What a window may say about WHEN it opens is a rule rather than a stored shape, and the
//! create and the modify of a window share it, so it lives here:
//! [`validate_event_window_cron_expression`] and [`validate_event_window_time_ranges`] are
//! the one reading of the constraints `CreateInstanceEventWindowRequest$CronExpression`
//! documents.
//!
//! What a window is TARGETED at is a rule the association and the disassociation of one
//! share, so it lives here too: [`require_one_event_window_target_kind`] is the one reading
//! of "only one type of target ... can be associated with an event window",
//! [`event_window_association_target`] the one constructor of a stored target, and
//! [`ComAmazonawsEc2Amazonec2State::set_instance_event_window_association`] the one writer
//! of one — so neither call can keep a target, charge its bytes or report the state the
//! clock owes it differently.
//!
//! A child module of [`crate::state`] rather than more lines in it: everything here belongs
//! to one resource and reaches the rest of the store through `use super::*`, so the state
//! module keeps only the field the windows live in and the writers that mint one.

use std::time::Instant;

use crate::ops::types::{
    InstanceEventWindow, InstanceEventWindowAssociationTarget, InstanceEventWindowTimeRange,
    InstanceEventWindowTimeRangeRequest,
};

use super::*;

/// The id prefix an event window is minted with.
///
/// The model publishes no pattern for `InstanceEventWindow$InstanceEventWindowId` (it is a
/// plain `String` — `smithy_query(query="shape",shape="InstanceEventWindowId")`), so the id
/// takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with the `iew` prefix EC2 gives an event
/// window. It is minted by the store and never taken from a request.
pub const INSTANCE_EVENT_WINDOW_ID_PREFIX: &str = "iew";

/// The state an event window is created in: the `InstanceEventWindowState` member
/// `creating` (`crate::ops::types::INSTANCE_EVENT_WINDOW_STATE_VALUES`).
pub const INSTANCE_EVENT_WINDOW_STATE_CREATING: &str = "creating";

/// The state the clock moves a newly created event window to: the
/// `InstanceEventWindowState` member `active`.
pub const INSTANCE_EVENT_WINDOW_STATE_ACTIVE: &str = "active";

/// The state a window reports while targets it was asked to take on are being attached to
/// it, from the call that asks until the clock has finished the work.
///
/// EC2 answers `AssociateInstanceEventWindow` with the window already carrying the target
/// and this word for its state, then reaches
/// [`INSTANCE_EVENT_WINDOW_STATE_ACTIVE`] without anybody asking — the same shape the
/// `creating` -> `active` move has. The model's `InstanceEventWindowState` enum lists the
/// four states a window RESTS in (`crate::ops::types::INSTANCE_EVENT_WINDOW_STATE_VALUES`)
/// and names neither in-flight word, so the two live here beside the moves that report
/// them.
pub const INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING: &str = "associating";

/// The state a window reports while targets it was asked to let go of are being detached
/// from it — the other direction of [`INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING`], reported
/// by `DisassociateInstanceEventWindow` until the clock has finished the work.
pub const INSTANCE_EVENT_WINDOW_STATE_DISASSOCIATING: &str = "disassociating";

/// `instance-event-window`, the `ResourceType` member that names an event window — the one
/// resource `CreateInstanceEventWindow`'s `TagSpecifications` may tag.
pub const RESOURCE_TYPE_INSTANCE_EVENT_WINDOW: &str = "instance-event-window";

/// How many event windows this store holds: "You can create up to 200 event windows per
/// Amazon Web Services Region" (`CreateInstanceEventWindow`).
pub const MAX_INSTANCE_EVENT_WINDOWS: usize = 200;

/// EC2's code for a create that would take the account past
/// [`MAX_INSTANCE_EVENT_WINDOWS`], in the `<Resource>LimitExceeded` form every other EC2
/// quota code takes ([`VOLUME_LIMIT_EXCEEDED`], [`LAUNCH_TEMPLATE_LIMIT_EXCEEDED`]).
pub const INSTANCE_EVENT_WINDOW_LIMIT_EXCEEDED: &str = "InstanceEventWindowLimitExceeded";

/// EC2's code for a request that named an event window id the account holds no window for.
pub const INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND: &str =
    "InvalidInstanceEventWindowId.NotFound";

/// EC2's code for a request that named something that is not an event window id at all.
pub const INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED: &str =
    "InvalidInstanceEventWindowId.Malformed";

/// The days of the week in CALENDAR order, each as the model's `WeekDay` enum spells it
/// (`crate::ops::types::WEEK_DAY_VALUES`, which lists the same seven members
/// alphabetically) paired with the three-letter alternative a cron day-of-week value may
/// use: "you can specify either integers 0 through 6, or alternative single values SUN
/// through SAT" (`CreateInstanceEventWindowRequest$CronExpression`).
///
/// A day's POSITION here is its cron day-of-week integer, and the hour-of-the-week
/// arithmetic a time range's span is measured in counts from the same position, so one
/// ordering serves both.
pub const WEEK_DAYS_IN_ORDER: [(&str, &str); 7] = [
    ("sunday", "SUN"),
    ("monday", "MON"),
    ("tuesday", "TUE"),
    ("wednesday", "WED"),
    ("thursday", "THU"),
    ("friday", "FRI"),
    ("saturday", "SAT"),
];

/// Hours in a day, the unit a time range's week day and hour are combined into.
pub const HOURS_PER_DAY: i64 = 24;

/// Hours in a week: a time range may run from one week day to another, including across
/// the end of the week, so its span is measured modulo this.
pub const HOURS_PER_WEEK: i64 = 7 * HOURS_PER_DAY;

/// The smallest span one range of an event window may cover: "Each hour range must be >= 2
/// hours, for example, 0-2 or 20-23" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const MIN_EVENT_WINDOW_RANGE_HOURS: i64 = 2;

/// The smallest span an event window's ranges may cover between them: "The event window
/// must be >= 4 hours. The combined total time ranges in the event window must be >= 4
/// hours" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const MIN_EVENT_WINDOW_TOTAL_HOURS: i64 = 4;

/// How many time ranges one event window may carry.
///
/// A STORE-PROTECTION ceiling this emulator chose, not a modelled `@length` and not an
/// AWS-published limit: the model declares no length on
/// `InstanceEventWindowTimeRangeRequestSet`, so without this one request could hand the
/// store a list as long as the request body allows and keep it for the process's lifetime.
/// A window's ranges live in ONE week and each covers at least
/// [`MIN_EVENT_WINDOW_RANGE_HOURS`], so the ceiling is how many ranges of the smallest span
/// a week holds — past it a range can only repeat a time another already covers (ranges may
/// overlap, so that is an argument about usefulness, not about validity).
/// [`validate_event_window_time_ranges`] refuses a longer list before it allocates
/// anything, the way the describe operations refuse more ids than [`MAX_RESOURCE_IDS`].
pub const MAX_EVENT_WINDOW_TIME_RANGES: usize = (HOURS_PER_WEEK / MIN_EVENT_WINDOW_RANGE_HOURS)
    as usize;

/// How long a cron expression this service reads may be.
///
/// The same kind of store-protection ceiling as [`MAX_EVENT_WINDOW_TIME_RANGES`], and
/// refused BEFORE the expression is split into fields, so neither the work nor the
/// allocation of reading it is proportional to a length the caller chose freely. The
/// longest expression the documented grammar admits is far shorter: the hour field holds at
/// most [`HOURS_PER_DAY`] / [`MIN_EVENT_WINDOW_RANGE_HOURS`] ranges of five characters, the
/// day-of-the-week field at most seven three-character days, and the other three fields are
/// one character each.
pub const MAX_EVENT_WINDOW_CRON_BYTES: usize = 256;

/// How many fields a cron expression carries: minute, hour, day of the month, month and
/// day of the week — as the example `* 0-4,20-23 * * 1,5`
/// (`CreateInstanceEventWindowRequest$CronExpression`) has them.
pub const CRON_FIELDS: usize = 5;

/// One stored instance event window: the modelled [`InstanceEventWindow`] every operation
/// that reports one answers with, plus the instant the clock owes it its move to `active`
/// and the word it reports until then.
///
/// The view holds no `State`: the state is a fact of the window's age, so
/// [`InstanceEventWindowRecord::view_at`] derives it rather than letting two writers store
/// two different words for it — the same shape [`VolumeRecord`] has.
#[derive(Debug, Clone)]
pub struct InstanceEventWindowRecord {
    /// The window as the model shapes it, with `State` left for `view_at` to fill.
    pub view: InstanceEventWindow,
    /// The instant from which this window's move to `active` is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and
    /// never reported, so a wall-clock adjustment cannot move a window back to `creating`.
    pub active_at: Instant,
    /// The `InstanceEventWindowState` this window reports until [`Self::active_at`]: the
    /// word for the work the last call to change it started — `creating` for the window a
    /// create minted, [`INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING`] or
    /// [`INSTANCE_EVENT_WINDOW_STATE_DISASSOCIATING`] for a change of its targets. Every
    /// one of those calls hands the window back BEFORE the clock has finished the work, so
    /// which word is owed is a fact of what was asked for and not of the window's age,
    /// while WHEN it gives way to `active` is a fact of the clock alone.
    pub in_flight_state: &'static str,
}

impl InstanceEventWindowRecord {
    /// This window as of `now`: the stored view with the `State` the clock has reached. A
    /// read observes the transition the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> InstanceEventWindow {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The window's `InstanceEventWindowState` at `now`: the word the call that last
    /// changed it left owed, until the clock reaches the instant that call made the move
    /// due, and `active` from then on.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.active_at {
            self.in_flight_state
        } else {
            INSTANCE_EVENT_WINDOW_STATE_ACTIVE
        }
    }
}

/// What one stored event window is created with, beside the id the store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for a subnet: the
/// operation that creates a window fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_instance_event_window`] turns it into the
/// modelled [`InstanceEventWindow`] the describe reports, so the id, the stored key and the
/// byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct InstanceEventWindowSpec {
    /// "The name of the event window" (`CreateInstanceEventWindowRequest$Name`), or `None`
    /// for a window created without one.
    pub name: Option<String>,
    /// The cron expression the window opens on, for a window defined by one.
    pub cron_expression: Option<String>,
    /// The time ranges the window opens in, for a window defined by them.
    pub time_ranges: Option<Vec<InstanceEventWindowTimeRange>>,
    /// The tags on the window.
    pub tags: Vec<Tag>,
    /// When the window's `creating` -> `active` move falls due.
    pub active_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another event window fits inside the store's ceiling
    /// ([`MAX_INSTANCE_EVENT_WINDOWS`]).
    ///
    /// A create must ask this BEFORE inserting and refuse with
    /// [`instance_event_window_limit_exceeded`] when it answers `false`, the way
    /// [`Self::has_vpc_capacity`] gates a VPC.
    pub fn has_instance_event_window_capacity(&self) -> bool {
        self.instance_event_windows.len() < MAX_INSTANCE_EVENT_WINDOWS
    }

    /// Store one newly created event window and answer the `InstanceEventWindowId` it was
    /// given, or `None` when the account already holds [`MAX_INSTANCE_EVENT_WINDOWS`] of
    /// them and NOTHING was stored.
    ///
    /// The id is minted here rather than taken from `spec`, the way [`Self::insert_subnet`]
    /// mints a `SubnetId`, and the caller-chosen bytes the record holds are charged against
    /// the store's byte budget here, so no path can store them uncharged. The count ceiling
    /// is asked HERE as well as by the create handler (which answers
    /// [`instance_event_window_limit_exceeded`] for it), so a writer that forgets the gate
    /// cannot grow the store either — the shape
    /// [`Self::insert_vpc_endpoint_connection`] has.
    pub fn insert_instance_event_window(
        &mut self,
        spec: InstanceEventWindowSpec,
    ) -> Option<String> {
        if !self.has_instance_event_window_capacity() {
            return None;
        }
        let instance_event_window_id = self.next_id(INSTANCE_EVENT_WINDOW_ID_PREFIX);
        let view = InstanceEventWindow {
            instance_event_window_id: Some(instance_event_window_id.clone()),
            name: spec.name,
            cron_expression: spec.cron_expression,
            time_ranges: spec.time_ranges,
            // "When you create the event window, targets (instance IDs, Dedicated Host IDs,
            // or tags) are not yet associated with it" (`CreateInstanceEventWindow`), so a
            // fresh window holds an EMPTY set of each of the three kinds of target;
            // `AssociateInstanceEventWindow` is the operation that puts one in.
            association_target: Some(event_window_association_target(
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )),
            // Derived from `active_at` on every read.
            state: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(instance_event_window_payload_bytes(&view));
        self.instance_event_windows.insert(
            instance_event_window_id.clone(),
            InstanceEventWindowRecord {
                view,
                active_at: spec.active_at,
                in_flight_state: INSTANCE_EVENT_WINDOW_STATE_CREATING,
            },
        );
        Some(instance_event_window_id)
    }

    /// Write one stored window's association target, leave it reporting `in_flight_state`
    /// until `active_at`, and answer whether a window of that id was there to write.
    ///
    /// The ONE writer of a window's targets, shared by the association and the
    /// disassociation of one, so the two cannot keep the target or the byte charge
    /// differently. The bytes the record held are given back and the bytes it now holds are
    /// charged, by the one definition of that charge
    /// ([`instance_event_window_payload_bytes`]) — a caller that could GROW the charge asks
    /// [`Self::would_exceed_stored_bytes_after_refund`] about the net change first, the way
    /// every other replacement path of this store does.
    pub fn set_instance_event_window_association(
        &mut self,
        instance_event_window_id: &str,
        association_target: InstanceEventWindowAssociationTarget,
        in_flight_state: &'static str,
        active_at: Instant,
    ) -> bool {
        let Some(record) = self.instance_event_windows.get_mut(instance_event_window_id) else {
            return false;
        };
        let refund = instance_event_window_payload_bytes(&record.view);
        record.view.association_target = Some(association_target);
        record.in_flight_state = in_flight_state;
        record.active_at = active_at;
        let charge = instance_event_window_payload_bytes(&record.view);
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        true
    }
}

/// The caller-chosen bytes one stored event window holds — its tags, its name, the cron
/// expression the request named, the time ranges it named and the targets it is associated
/// with — plus the fixed cost of the record itself
/// ([`INSTANCE_EVENT_WINDOW_RECORD_BYTES`]), so no window is free. The id and the state are
/// derived by this module.
///
/// A time range is a fixed-size element, but the LIST of them is as long as the caller made
/// it, so each one is charged — the bytes of the two week-day words it stores plus the
/// fixed cost of the element itself ([`EVENT_WINDOW_TIME_RANGE_BYTES`]) — the way
/// [`vpc_endpoint_connection_payload_bytes`] charges each DNS entry and each load balancer
/// ARN of a connection. A count ceiling ([`MAX_EVENT_WINDOW_TIME_RANGES`]) bounds the list
/// too, and the two bounds are independent: neither is the other.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: a
/// writer charges exactly it, and a removal path must give back the same number.
pub fn instance_event_window_payload_bytes(window: &InstanceEventWindow) -> usize {
    let time_range_bytes: usize = window
        .time_ranges
        .iter()
        .flatten()
        .map(|range| {
            EVENT_WINDOW_TIME_RANGE_BYTES
                .saturating_add(range.start_week_day.as_deref().map_or(0, str::len))
                .saturating_add(range.end_week_day.as_deref().map_or(0, str::len))
        })
        .sum();
    INSTANCE_EVENT_WINDOW_RECORD_BYTES
        .saturating_add(tag_list_bytes(
            window.tags.as_deref().unwrap_or_default(),
        ))
        .saturating_add(window.name.as_deref().map_or(0, str::len))
        .saturating_add(window.cron_expression.as_deref().map_or(0, str::len))
        .saturating_add(time_range_bytes)
        .saturating_add(
            window
                .association_target
                .as_ref()
                .map_or(0, event_window_association_target_bytes),
        )
}

/// The caller-chosen bytes one stored association target holds: every id and every tag the
/// calls that built it named, each of which the store keeps for as long as the window does.
///
/// Part of [`instance_event_window_payload_bytes`] rather than a charge of its own, so a
/// window is charged once wherever its target came from; the count of each set is bounded
/// independently ([`MAX_RESOURCE_IDS`], [`MAX_EVENT_WINDOW_TARGET_TAGS`]), and neither bound
/// is the other.
pub fn event_window_association_target_bytes(
    target: &InstanceEventWindowAssociationTarget,
) -> usize {
    let id_bytes = |ids: Option<&Vec<String>>| -> usize {
        ids.into_iter().flatten().map(String::len).sum()
    };
    id_bytes(target.instance_ids.as_ref())
        .saturating_add(id_bytes(target.dedicated_host_ids.as_ref()))
        .saturating_add(tag_list_bytes(target.tags.as_deref().unwrap_or_default()))
}

/// The fixed overhead one stored event window costs whatever it carries: the modelled
/// record itself, its minted id and the map entry holding it, as a CHOSEN constant in the
/// same spirit as [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] — the point is that an untagged
/// window is not free against [`MAX_STORED_BYTES`], not that it is byte-exact.
pub const INSTANCE_EVENT_WINDOW_RECORD_BYTES: usize = 512;

/// The fixed bytes one stored time range holds beside its two week-day words: the two hours
/// the model bounds to 0..=23, as the four-byte integers the record keeps them in.
pub const EVENT_WINDOW_TIME_RANGE_BYTES: usize = 2 * ::std::mem::size_of::<i32>();

/// Whether `id` is a well-formed event window id: [`INSTANCE_EVENT_WINDOW_ID_PREFIX`]
/// followed by 8 or 17 lowercase hexadecimal digits — the short and the long id form every
/// EC2 resource id takes, as the model's own `^iew-([0-9a-f]{8}|[0-9a-f]{17})$` style id
/// patterns read. The one definition of that form lives in
/// [`prefixed_id_is_well_formed`], which this names the prefix for.
pub fn instance_event_window_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(INSTANCE_EVENT_WINDOW_ID_PREFIX, id)
}

/// Refuse a request that named an event window id the account holds no such window for:
/// HTTP 400 `InvalidInstanceEventWindowId.NotFound`.
///
/// Worded by [`id_not_found`], the one template this service refuses an id it holds no
/// resource for with, and naming the resource the way EC2's own `ResourceType` member for it
/// does ([`RESOURCE_TYPE_INSTANCE_EVENT_WINDOW`]) — the spelling the refusals of the other
/// resource families whose ids carry a type token take
/// ([`invalid_capacity_reservation_id_not_found`]).
pub fn invalid_instance_event_window_id_not_found<E>(
    instance_event_window_id: &str,
) -> Scaffolded<E> {
    id_not_found(
        INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_INSTANCE_EVENT_WINDOW} ID"),
        instance_event_window_id,
    )
}

/// Refuse an event window id this service cannot resolve: a malformed one is answered as
/// malformed and a well-formed one as not found — the ordering [`unresolvable_host_id`] and
/// [`require_known_volume`] keep.
pub fn unresolvable_instance_event_window_id<E>(instance_event_window_id: &str) -> Scaffolded<E> {
    if instance_event_window_id_is_well_formed(instance_event_window_id) {
        return invalid_instance_event_window_id_not_found(instance_event_window_id);
    }
    Scaffolded::service_error(
        INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED,
        400,
        invalid_id_message(instance_event_window_id),
    )
}

/// Refuse a create that would take the store past [`MAX_INSTANCE_EVENT_WINDOWS`] or its
/// byte budget: HTTP 400 `InstanceEventWindowLimitExceeded`.
pub fn instance_event_window_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INSTANCE_EVENT_WINDOW_LIMIT_EXCEEDED, 400, message)
}

/// How many instance tags one event window's association target may name.
///
/// A STORE-PROTECTION ceiling this emulator chose, not a modelled `@length`: the model
/// declares none on `TagList`, so without it one request could hand the store a list as long
/// as the request body allows and keep it for the process's lifetime. A target's tags select
/// instances the way a resource's own tags are selected, so it takes the same count EC2
/// documents for those ([`MAX_TAGS_PER_RESOURCE`]); the ids of a target are bounded by
/// [`MAX_RESOURCE_IDS`], the ceiling every id list of this service takes.
pub const MAX_EVENT_WINDOW_TARGET_TAGS: usize = MAX_TAGS_PER_RESOURCE;

/// Which of the three kinds of target one request named for an event window.
///
/// "Only one *type* of target (instance ID, instance tag, or Dedicated Host ID) can be
/// associated with an event window" (`InstanceEventWindowAssociationRequest`), so a request
/// names exactly one of them — this is which, as
/// [`require_one_event_window_target_kind`] reads it off the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventWindowTargetKind {
    /// `InstanceIds`: "The IDs of the instances".
    InstanceIds,
    /// `InstanceTags`: "The instance tags".
    InstanceTags,
    /// `DedicatedHostIds`: "The IDs of the Dedicated Hosts".
    DedicatedHostIds,
}

impl EventWindowTargetKind {
    /// The request member this kind of target is named by, for a refusal that has to say
    /// which one it means.
    pub fn member_name(self) -> &'static str {
        match self {
            Self::InstanceIds => "InstanceIds",
            Self::InstanceTags => "InstanceTags",
            Self::DedicatedHostIds => "DedicatedHostIds",
        }
    }

    /// The kinds of target `target` currently holds something of — none for a window
    /// nothing is associated with, and (by the rule above) one for every other window.
    pub fn held_by(target: &InstanceEventWindowAssociationTarget) -> Vec<Self> {
        let mut held = Vec::new();
        if target.instance_ids.as_deref().is_some_and(|ids| !ids.is_empty()) {
            held.push(Self::InstanceIds);
        }
        if target.tags.as_deref().is_some_and(|tags| !tags.is_empty()) {
            held.push(Self::InstanceTags);
        }
        if target
            .dedicated_host_ids
            .as_deref()
            .is_some_and(|ids| !ids.is_empty())
        {
            held.push(Self::DedicatedHostIds);
        }
        held
    }
}

/// The ONE reading of which targets a request named for an event window, shared by the
/// association and the disassociation of a window's targets: the kinds of target it named,
/// refusing one that named none at all.
///
/// A target is a required member of both requests, so a request reaching here carries the
/// structure — but each of its three sets is optional, and an EMPTY set names no target, so
/// an empty one counts as unspecified rather than as a kind the request asked about.
pub fn require_event_window_targets_named<E>(
    instance_ids: &[String],
    instance_tags: &[Tag],
    dedicated_host_ids: &[String],
) -> Result<Vec<EventWindowTargetKind>, Scaffolded<E>> {
    let named: Vec<EventWindowTargetKind> = [
        (!instance_ids.is_empty()).then_some(EventWindowTargetKind::InstanceIds),
        (!instance_tags.is_empty()).then_some(EventWindowTargetKind::InstanceTags),
        (!dedicated_host_ids.is_empty()).then_some(EventWindowTargetKind::DedicatedHostIds),
    ]
    .into_iter()
    .flatten()
    .collect();
    if named.is_empty() {
        return Err(Scaffolded::missing_parameter(
            "One of InstanceIds, InstanceTags or DedicatedHostIds must be specified",
        ));
    }
    Ok(named)
}

/// The sentence EC2 refuses a target that is not of one kind with: the three kinds of target
/// a target association may be composed of, and nothing of the request — the ids and tags a
/// caller sent are not part of it.
///
/// One wording, whether the request named two kinds at once or named one the window cannot
/// hold beside the kind it already holds, because the rule broken is the same: "Only one
/// *type* of target (instance ID, instance tag, or Dedicated Host ID) can be associated with
/// an event window" (`InstanceEventWindowAssociationRequest`).
pub const ONE_TARGET_TYPE_MESSAGE: &str = "Target Associations must be composed entirely of \
                                           Instance IDs, Instance Tags, or Dedicated Host \
                                           IDs.";

/// The one kind of target a request named, for the call the rule "Only one type of target
/// (instance ID, instance tag, or Dedicated Host ID) can be associated with an event window"
/// (`InstanceEventWindowAssociationRequest`) is stated for: a request naming more than one
/// kind is refused, as is one naming none ([`require_event_window_targets_named`]).
///
/// The kinds of target are members of ONE request structure
/// (`InstanceEventWindowAssociationRequest`), so a target composed of more than one kind is
/// an inadmissible VALUE of that structure rather than a pair of independent request
/// parameters that cannot be named together: the refusal is
/// [`Scaffolded::invalid_parameter_value`] ([`ONE_TARGET_TYPE_MESSAGE`]), the class a client
/// repairs by fixing the field it names.
pub fn require_one_event_window_target_kind<E>(
    instance_ids: &[String],
    instance_tags: &[Tag],
    dedicated_host_ids: &[String],
) -> Result<EventWindowTargetKind, Scaffolded<E>> {
    let named =
        require_event_window_targets_named(instance_ids, instance_tags, dedicated_host_ids)?;
    match named.as_slice() {
        [one] => Ok(*one),
        _ => Err(one_event_window_target_kind_required()),
    }
}

/// The ONE refusal of a target that is not composed of a single kind: HTTP 400
/// `InvalidParameterValue` carrying [`ONE_TARGET_TYPE_MESSAGE`].
///
/// Shared by the two readings of the rule — the target one request names
/// ([`require_one_event_window_target_kind`]) and the target that request would leave the
/// window holding — so the same broken rule cannot be answered with two classes.
pub fn one_event_window_target_kind_required<E>() -> Scaffolded<E> {
    Scaffolded::invalid_parameter_value(ONE_TARGET_TYPE_MESSAGE)
}

/// Refuse an id list longer than this service serves, naming the member it came from —
/// the ceiling [`MAX_RESOURCE_IDS`] every id list of this service takes, read in one place
/// for both of the calls that change a window's targets.
pub fn require_event_window_target_id_count<E>(
    member: &str,
    ids: &[String],
) -> Result<(), Scaffolded<E>> {
    if ids.len() > MAX_RESOURCE_IDS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "{member} carries {} ids, above the maximum of {MAX_RESOURCE_IDS}",
            ids.len()
        )));
    }
    Ok(())
}

/// Refuse a target this store will not HOLD: more ids than [`MAX_RESOURCE_IDS`] or more
/// tags than [`MAX_EVENT_WINDOW_TARGET_TAGS`] on one window.
///
/// The per-request bounds above cap ONE call, which is not the same thing: an association
/// adds to the targets a window already carries, so without this, repeated calls would grow
/// a stored target — and the work of merging into it — past the ceiling one call is held to.
/// Asked of the target an association WOULD leave on the window, before anything is written.
pub fn require_event_window_target_capacity<E>(
    target: &InstanceEventWindowAssociationTarget,
) -> Result<(), Scaffolded<E>> {
    let ids = |held: Option<&Vec<String>>| held.map_or(0, Vec::len);
    for (member, count) in [
        ("instance", ids(target.instance_ids.as_ref())),
        ("Dedicated Host", ids(target.dedicated_host_ids.as_ref())),
    ] {
        if count > MAX_RESOURCE_IDS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The event window would be associated with {count} {member} ids, above the \
                 maximum of {MAX_RESOURCE_IDS}"
            )));
        }
    }
    let tags = target.tags.as_ref().map_or(0, Vec::len);
    if tags > MAX_EVENT_WINDOW_TARGET_TAGS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "The event window would be associated with {tags} instance tags, above the \
             maximum of {MAX_EVENT_WINDOW_TARGET_TAGS}"
        )));
    }
    Ok(())
}

/// Refuse a target tag list this service will not store: more tags than
/// [`MAX_EVENT_WINDOW_TARGET_TAGS`], or one whose key or value is longer than EC2 documents
/// ([`validate_tag_size`]) — the bound asked BEFORE the per-tag one, so the work is
/// proportional to a length this service serves rather than to one the caller chose.
pub fn validate_event_window_target_tags<E>(tags: &[Tag]) -> Result<(), Scaffolded<E>> {
    if tags.len() > MAX_EVENT_WINDOW_TARGET_TAGS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "InstanceTags carries {} tags, above the maximum of \
             {MAX_EVENT_WINDOW_TARGET_TAGS}",
            tags.len()
        )));
    }
    for tag in tags {
        validate_tag_size(
            tag.key.as_deref().unwrap_or_default(),
            tag.value.as_deref().unwrap_or_default(),
        )?;
    }
    Ok(())
}

/// One stored association target, with each of its three sets PRESENT: an empty one for a
/// kind of target nothing is associated with.
///
/// A window that has been asked to take on or let go of targets reports the structure
/// whatever is left in it, so a caller reads the set it asked about rather than inferring
/// from an absent element whether the window has targets of that kind. The ONE constructor
/// of a stored target, so the association and the disassociation of a window cannot report
/// its sets differently.
pub fn event_window_association_target(
    instance_ids: Vec<String>,
    instance_tags: Vec<Tag>,
    dedicated_host_ids: Vec<String>,
) -> InstanceEventWindowAssociationTarget {
    InstanceEventWindowAssociationTarget {
        instance_ids: Some(instance_ids),
        tags: Some(instance_tags),
        dedicated_host_ids: Some(dedicated_host_ids),
    }
}

/// `held` with every id of `added` it does not already carry appended, in the order the
/// request named them.
///
/// Associating a target a window already carries changes nothing: a target is a SET of
/// resources, so one id is one target of the window however many calls named it.
pub fn with_target_ids(held: &[String], added: &[String]) -> Vec<String> {
    let mut union: Vec<String> = held.to_vec();
    for id in added {
        if !union.iter().any(|kept| kept == id) {
            union.push(id.clone());
        }
    }
    union
}

/// `held` without any id `removed` names.
///
/// An id the window does not carry removes nothing rather than failing: the request asks for
/// the id not to be a target of the window, which is what it is afterwards either way.
pub fn without_target_ids(held: &[String], removed: &[String]) -> Vec<String> {
    held.iter()
        .filter(|id| !removed.iter().any(|dropped| dropped == *id))
        .cloned()
        .collect()
}

/// A tag target as the pair it selects instances by: its key and its value, either of which
/// a request may leave unset.
fn tag_target(tag: &Tag) -> (Option<&str>, Option<&str>) {
    (tag.key.as_deref(), tag.value.as_deref())
}

/// `held` with every tag of `added` it does not already carry appended, compared as the
/// key/value pair each one selects instances by — the tag equivalent of
/// [`with_target_ids`].
pub fn with_target_tags(held: &[Tag], added: &[Tag]) -> Vec<Tag> {
    let mut union: Vec<Tag> = held.to_vec();
    for tag in added {
        if !union.iter().any(|kept| tag_target(kept) == tag_target(tag)) {
            union.push(tag.clone());
        }
    }
    union
}

/// `held` without any tag `removed` names, compared as the key/value pair each one selects
/// instances by — the tag equivalent of [`without_target_ids`].
pub fn without_target_tags(held: &[Tag], removed: &[Tag]) -> Vec<Tag> {
    held.iter()
        .filter(|tag| {
            !removed
                .iter()
                .any(|dropped| tag_target(dropped) == tag_target(tag))
        })
        .cloned()
        .collect()
}

/// The calendar index of the `WeekDay` member `day` — `sunday` is 0 and `saturday` is 6 —
/// or `None` for a word that is not one of [`WEEK_DAYS_IN_ORDER`].
pub fn week_day_index(day: &str) -> Option<i64> {
    WEEK_DAYS_IN_ORDER
        .iter()
        .position(|(modelled, _)| *modelled == day)
        .and_then(|index| i64::try_from(index).ok())
}

/// How many hours a range running from the hour-of-the-week `start` to `end` covers.
///
/// Measured modulo [`HOURS_PER_WEEK`], because a time range may run across the end of the
/// week (`saturday` 20:00 to `sunday` 04:00 is eight hours, not minus 160). A range whose
/// start and end are the same hour covers none.
pub fn event_window_span_hours(start: i64, end: i64) -> i64 {
    (end - start).rem_euclid(HOURS_PER_WEEK)
}

/// Validate the `TimeRanges` of an event window, answering the modelled time ranges to
/// store.
///
/// How MANY ranges a window may carry is bounded first ([`MAX_EVENT_WINDOW_TIME_RANGES`]),
/// before this allocates anything, because the model declares no `@length` on the list.
/// The rules for each one are the ones `CreateInstanceEventWindowRequest$CronExpression` states for the
/// equivalent cron expression, and that "Define event windows for scheduled events" in the
/// Amazon EC2 User Guide — the guide `CreateInstanceEventWindow` points at — states for an
/// event window however it is defined: every range covers at least
/// [`MIN_EVENT_WINDOW_RANGE_HOURS`] and they cover at least
/// [`MIN_EVENT_WINDOW_TOTAL_HOURS`] between them. Each range names a week day and an hour
/// at both ends; the model bounds the hours (`Hour` is `@range(min: 0, max: 23)`), so an
/// out-of-range one is refused before a handler runs, and an unknown week day is refused
/// here.
///
/// Shared by the create and the modify of a window, which validate the same member the same
/// way.
pub fn validate_event_window_time_ranges<E>(
    ranges: &[InstanceEventWindowTimeRangeRequest],
) -> Result<Vec<InstanceEventWindowTimeRange>, Scaffolded<E>> {
    // Refused BEFORE anything is allocated or validated, so neither the work nor the
    // allocation this call makes is proportional to a number the caller chose freely.
    if ranges.len() > MAX_EVENT_WINDOW_TIME_RANGES {
        return Err(invalid_parameter_value(format!(
            "timeRanges carries {} time ranges, above the maximum of \
             {MAX_EVENT_WINDOW_TIME_RANGES}",
            ranges.len()
        )));
    }
    let mut stored: Vec<InstanceEventWindowTimeRange> = Vec::with_capacity(ranges.len());
    let mut total_hours: i64 = 0;
    for range in ranges {
        let (Some(start_week_day), Some(end_week_day)) = (
            range.start_week_day.as_deref(),
            range.end_week_day.as_deref(),
        ) else {
            return Err(missing_parameter(
                "A time range must name a startWeekDay and an endWeekDay",
            ));
        };
        let (Some(start_hour), Some(end_hour)) = (range.start_hour, range.end_hour) else {
            return Err(missing_parameter(
                "A time range must name a startHour and an endHour",
            ));
        };
        let (named_start_day, named_end_day) =
            (week_day_index(start_week_day), week_day_index(end_week_day));
        let (Some(start_day), Some(end_day)) = (named_start_day, named_end_day) else {
            let unknown = if named_start_day.is_none() {
                start_week_day
            } else {
                end_week_day
            };
            return Err(invalid_parameter_value(format!(
                "Invalid value '{unknown}' for a time range week day. \
                 A week day is one of {days}",
                days = WEEK_DAYS_IN_ORDER
                    .iter()
                    .map(|(modelled, _)| *modelled)
                    .collect::<Vec<_>>()
                    .join(", "),
            )));
        };
        let start = start_day * HOURS_PER_DAY + i64::from(start_hour);
        let end = end_day * HOURS_PER_DAY + i64::from(end_hour);
        let hours = event_window_span_hours(start, end);
        if hours < MIN_EVENT_WINDOW_RANGE_HOURS {
            return Err(invalid_parameter_value(format!(
                "The time range from {start_week_day} {start_hour} to {end_week_day} \
                 {end_hour} covers {hours} hours; each time range must cover at least \
                 {MIN_EVENT_WINDOW_RANGE_HOURS} hours"
            )));
        }
        total_hours = total_hours.saturating_add(hours);
        stored.push(InstanceEventWindowTimeRange {
            start_week_day: Some(start_week_day.to_string()),
            start_hour: Some(start_hour),
            end_week_day: Some(end_week_day.to_string()),
            end_hour: Some(end_hour),
        });
    }
    if total_hours < MIN_EVENT_WINDOW_TOTAL_HOURS {
        return Err(invalid_parameter_value(format!(
            "The time ranges cover {total_hours} hours between them; the combined total \
             must be at least {MIN_EVENT_WINDOW_TOTAL_HOURS} hours"
        )));
    }
    Ok(stored)
}

/// Validate the `CronExpression` of an event window.
///
/// The constraints are the ones the member documents: a length this service reads at all
/// ([`MAX_EVENT_WINDOW_CRON_BYTES`], refused first, before the expression is split), then
/// five cron fields, of which only the hour and the day of the week carry values — "the
/// minute, month, and year must be
/// specified by `*`" — the hour field being "one or a multiple range, for example, 0-4 or
/// 0-4,20-23" with every range covering at least [`MIN_EVENT_WINDOW_RANGE_HOURS`] and the
/// ranges covering at least [`MIN_EVENT_WINDOW_TOTAL_HOURS`] between them, and a day of the
/// week being "either integers 0 through 6, or alternative single values SUN through SAT"
/// (`CreateInstanceEventWindowRequest$CronExpression`).
///
/// Shared by the create and the modify of a window, which validate the same member the same
/// way.
pub fn validate_event_window_cron_expression<E>(expression: &str) -> Result<(), Scaffolded<E>> {
    // Refused BEFORE the expression is split, so the pointer vector below is never sized
    // from a length the caller chose freely.
    if expression.len() > MAX_EVENT_WINDOW_CRON_BYTES {
        return Err(invalid_parameter_value(format!(
            "cronExpression carries {} bytes, above the maximum of \
             {MAX_EVENT_WINDOW_CRON_BYTES}",
            expression.len()
        )));
    }
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{expression}' for cronExpression. A cron expression carries \
             {CRON_FIELDS} fields, not {}",
            fields.len()
        )));
    }
    // Only the hour (the second field) and the day of the week (the fifth) carry values;
    // the minute, the day of the month and the month must each be `*`.
    for (position, name) in [(0, "minute"), (2, "day of the month"), (3, "month")] {
        let field = fields.get(position).copied().unwrap_or_default();
        if field != CRON_WILDCARD {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{field}' for the {name} field of cronExpression. \
                 It must be '{CRON_WILDCARD}'"
            )));
        }
    }

    let hour_field = fields.get(1).copied().unwrap_or_default();
    let mut total_hours: i64 = 0;
    for element in hour_field.split(',') {
        // "The hour value must be one or a multiple range" — a bare hour names no range, so
        // it is refused along with anything that is not two hours around one `-`.
        let Some((start, end)) = element.split_once('-') else {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour value is one or more ranges, for example 0-4 or 0-4,20-23"
            )));
        };
        let (Ok(start), Ok(end)) = (start.parse::<i64>(), end.parse::<i64>()) else {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour range runs between two hours, for example 0-4"
            )));
        };
        if !(0..HOURS_PER_DAY).contains(&start) || !(0..HOURS_PER_DAY).contains(&end) {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour is from 0 to {last}",
                last = HOURS_PER_DAY - 1
            )));
        }
        // A cron hour range does not wrap within a day — the day-of-the-week field is what
        // carries a window across days — so a reversed range is refused as reversed rather
        // than reported as covering a negative number of hours.
        if end <= start {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour range runs forwards, from its earlier hour to its later one"
            )));
        }
        let hours = end - start;
        if hours < MIN_EVENT_WINDOW_RANGE_HOURS {
            return Err(invalid_parameter_value(format!(
                "The hour range '{element}' of cronExpression covers {hours} hours; \
                 each hour range must cover at least {MIN_EVENT_WINDOW_RANGE_HOURS} hours"
            )));
        }
        total_hours = total_hours.saturating_add(hours);
    }
    if total_hours < MIN_EVENT_WINDOW_TOTAL_HOURS {
        return Err(invalid_parameter_value(format!(
            "The hour ranges of cronExpression cover {total_hours} hours between them; \
             the combined total must be at least {MIN_EVENT_WINDOW_TOTAL_HOURS} hours"
        )));
    }

    let day_field = fields.get(4).copied().unwrap_or_default();
    // The day-of-the-week field MAY be the wildcard — a window that opens in its hours on
    // every day of the week — while the hour field may not: "the hour value must be one or a
    // multiple range" names no wildcard, and a window open every hour of every day would
    // not be a window. The asymmetry is the member's, not an oversight here.
    if day_field != CRON_WILDCARD {
        for element in day_field.split(',') {
            if !cron_day_of_week_is_known(element) {
                return Err(invalid_parameter_value(format!(
                    "Invalid value '{element}' for the day of the week field of \
                     cronExpression. A day of the week is an integer from 0 to 6, or one \
                     of {days}",
                    days = WEEK_DAYS_IN_ORDER
                        .iter()
                        .map(|(_, alternative)| *alternative)
                        .collect::<Vec<_>>()
                        .join(", "),
                )));
            }
        }
    }
    Ok(())
}

/// The cron field that names every value of its position: "the minute, month, and year must
/// be specified by `*`" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const CRON_WILDCARD: &str = "*";

/// Whether `value` names a day of the week a cron expression may carry: "either integers 0
/// through 6, or alternative single values SUN through SAT"
/// (`CreateInstanceEventWindowRequest$CronExpression`), the three-letter alternatives of
/// [`WEEK_DAYS_IN_ORDER`] in either case, cron naming them case-insensitively.
fn cron_day_of_week_is_known(value: &str) -> bool {
    if let Ok(day) = value.parse::<i64>() {
        let days = i64::try_from(WEEK_DAYS_IN_ORDER.len()).unwrap_or_default();
        return (0..days).contains(&day);
    }
    WEEK_DAYS_IN_ORDER
        .iter()
        .any(|(_, alternative)| alternative.eq_ignore_ascii_case(value))
}
