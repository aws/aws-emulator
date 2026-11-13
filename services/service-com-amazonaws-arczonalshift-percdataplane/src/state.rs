// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-arczonalshift-percdataplane service, plus
//! the domain rules more than one operation applies to it.
//!
//! ARC Zonal Shift models no operation that creates a managed resource —
//! "Resources are automatically registered with ARC by Amazon Web Services
//! services" (the `StartZonalShift` documentation in this service's Smithy
//! model) — so a managed resource reaches this store one of two ways: the
//! resources this emulator starts with are seeded here, at state init, and the
//! ones another service registers by turning zonal-shift integration on for them
//! are read, per call, from the shared registry that service writes
//! ([`published_resources`]). Everything a read reports is then computed from the
//! two: the zonal shifts a caller starts, and every value derived from them
//! (`appliedWeights`, `appliedStatus`, `practiceRunStatus`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessDeniedException, AutoshiftInResource, PracticeRunConfiguration, ZonalShiftInResource,
};

/// The managed resources the services that OWN them have registered with ARC,
/// read from the shared registry they write.
mod published_resources;

pub use published_resources::{managed_resource_registry, published_resources};

/// The practice run configuration ARC holds for a resource, and the rules every
/// operation that writes one applies to it.
mod practice_runs;

pub use practice_runs::{
    practice_run_configuration_deletion_conflict, requested_practice_run_configuration,
    updated_practice_run_configuration, PracticeRunConfigurationRefused, AUTOSHIFT_ENABLED_MESSAGE,
    AUTOSHIFT_ENABLED_REASON, PRACTICE_CONFIGURATION_EXISTS_MESSAGE,
    PRACTICE_CONFIGURATION_MISSING_MESSAGE, PRACTICE_CONFIGURATION_MISSING_REASON,
};

/// The zonal autoshift status ARC holds for a resource, and the rule the operation
/// that writes one applies to it.
mod zonal_autoshift;

pub use zonal_autoshift::{
    zonal_autoshift_conflict, PRACTICE_CONFIGURATION_REQUIRED_MESSAGE,
    ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON,
};

/// The Amazon Web Services Region the seeded managed resources live in. The
/// Availability Zone IDs they are deployed in (`use1-az*`) are this Region's, so
/// the two are changed together.
const SEED_REGION: &str = "us-east-1";

/// The longest a zonal shift may be set to be active for.
///
/// From the `expiresIn` documentation of `StartZonalShiftRequest` and
/// `UpdateZonalShiftRequest`: "You can set a zonal shift to be active initially
/// for up to three days (72 hours)."
pub const MAX_EXPIRES_IN: Duration = Duration::from_secs(72 * 60 * 60);

/// The number of items a list operation returns when the caller names no
/// `maxResults`: the largest page the model allows (`MaxResults`, range 1..100),
/// so an unpaginated caller sees everything in one answer.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How many COMPLETED (`CANCELED` or `EXPIRED`) zonal shifts one store keeps.
///
/// ARC documents no retention window for completed shifts, so this is a bound on
/// the emulator's own memory rather than an emulated one. `ListZonalShifts`
/// reports completed shifts as well as active ones, so without a cap a caller
/// repeating start/cancel grows the store — and the per-request expiry sweep over
/// it — without limit. The cap is far above the number of shifts a caller reads
/// back, and the oldest are dropped first, so what a caller can still observe is
/// what it keeps.
pub const MAX_COMPLETED_SHIFTS_RETAINED: usize = 1_000;

/// The message ARC answers with when the target of a zonal-shift call is not one
/// it holds for the caller.
///
/// It names no identifier, which is the point: the refusal discloses nothing about
/// what does or does not exist.
pub const TARGET_UNAVAILABLE_MESSAGE: &str = "Requested resource is not available.";

/// The ONE refusal every zonal-shift mutation keyed by a `resourceIdentifier` answers
/// when the resource it names is not one this account's ARC holds for the caller.
///
/// A mutation keyed by a `zonalShiftId` states no reason at all
/// ([`identifier_not_attributable`]): this message is about the RESOURCE a call named, so
/// it is answered only where a call named one.
///
/// ARC decides the caller's authorization ON THE TARGET before the target is
/// resolved, so an identifier it cannot attribute to the caller is answered
/// `AccessDeniedException` — "You do not have sufficient access to perform this
/// action.", `@error client` with `@httpError 403` and a service-common error of
/// `PercDataPlane` — and never `ResourceNotFoundException` (404), which would
/// disclose that the identifier names nothing. Built here, once, so the handlers
/// cannot drift apart on which branch they take.
#[must_use]
pub fn target_unavailable() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(TARGET_UNAVAILABLE_MESSAGE.to_string()),
    }
}

/// The prefix every Amazon Resource Name carries, which is the whole of what
/// `ResourceArn`'s `@pattern` requires of one: `^arn:.*$`
/// (`smithy_query(query="shape", shape="ResourceArn")`). `ResourceIdentifier` — the
/// member a caller names a resource by — declares no pattern of its own, and its
/// documentation says "The identifier is the Amazon Resource Name (ARN) for the
/// resource", so this is the shape an identifier has to take to name one at all.
pub const RESOURCE_ARN_PREFIX: &str = "arn:";

/// Whether `identifier` is shaped like the resource ARN a `resourceIdentifier` names.
#[must_use]
pub fn names_a_resource_arn(identifier: &str) -> bool {
    identifier.starts_with(RESOURCE_ARN_PREFIX)
}

/// The refusal a zonal-shift operation answers for a target it cannot attribute to the
/// caller at all: a `resourceIdentifier` that is not shaped like a resource ARN
/// ([`names_a_resource_arn`]), and a `zonalShiftId` naming no shift ARC holds for this
/// account ([`ComAmazonawsArczonalshiftPercdataplaneState::authorized_shift_mut`]).
///
/// An IAM policy grants an ARC action on RESOURCE ARNS, and the caller's authorization
/// on the named target is decided before the service resolves it, so a target that names
/// no resource such a policy could have granted the action on is denied — 403
/// `AccessDeniedException` — rather than looked up and reported missing. It states NO
/// REASON: `AccessDeniedException$message` is optional in the model, and a denial taken
/// on an identifier before any target was resolved has nothing to report about it (ARC's
/// recorded denials on both kinds of identifier carry an empty message). The denial that
/// does state a reason is the one taken on a RESOURCE the caller named, which is the
/// subject the refusal can speak about ([`target_unavailable`]); an ARN-shaped
/// identifier a reporting operation resolves and ARC holds no record of is that
/// operation's own `ResourceNotFoundException` (404).
#[must_use]
pub fn identifier_not_attributable() -> AccessDeniedException {
    AccessDeniedException { message: None }
}

/// A zonal shift's lifecycle status.
///
/// The model's `ZonalShiftStatus` enum (`ACTIVE`, `EXPIRED`, `CANCELED`):
/// "ACTIVE: The zonal shift has been started and is active. EXPIRED: The zonal
/// shift has expired (the expiry time was exceeded). CANCELED: The zonal shift
/// was canceled."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonalShiftStatus {
    Active,
    Expired,
    Canceled,
}

impl ZonalShiftStatus {
    /// The value this status takes on the wire.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Expired => "EXPIRED",
            Self::Canceled => "CANCELED",
        }
    }
}

/// How a traffic shift was started.
///
/// The model's `ShiftType` enum (`FIS_EXPERIMENT`, `PRACTICE_RUN`,
/// `ZONAL_AUTOSHIFT`, `ZONAL_SHIFT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftType {
    ZonalShift,
    FisExperiment,
    ZonalAutoshift,
    PracticeRun,
}

impl ShiftType {
    /// The value this shift type takes on the wire.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ZonalShift => "ZONAL_SHIFT",
            Self::FisExperiment => "FIS_EXPERIMENT",
            Self::ZonalAutoshift => "ZONAL_AUTOSHIFT",
            Self::PracticeRun => "PRACTICE_RUN",
        }
    }

    /// Which of several shifts in progress for one resource is the one in
    /// effect: the lowest rank wins.
    ///
    /// From the `appliedStatus` documentation of `ZonalShiftInResource`: "zonal
    /// shifts that you start as a customer take precedence [over] autoshifts,
    /// which take precedence over practice runs. That is, customer-initiated
    /// zonal shifts > autoshifts > practice run zonal shifts." A shift an FIS
    /// experiment drives is not customer-initiated, so it ranks below one the
    /// customer started and above an autoshift.
    #[must_use]
    pub fn precedence(self) -> u8 {
        match self {
            Self::ZonalShift => 0,
            Self::FisExperiment => 1,
            Self::ZonalAutoshift => 2,
            Self::PracticeRun => 3,
        }
    }
}

/// Whether zonal autoshift, or a practice run configuration, is in place for a
/// resource. The model's `ZonalAutoshiftStatus` enum (`DISABLED`, `ENABLED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonalAutoshiftStatus {
    Disabled,
    Enabled,
}

impl ZonalAutoshiftStatus {
    /// The value this status takes on the wire.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::Enabled => "ENABLED",
        }
    }

    /// The status a wire value names, or `None` when it is not one of the two the
    /// model declares (`ZONAL_AUTOSHIFT_STATUS_VALUES`).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "DISABLED" => Some(Self::Disabled),
            "ENABLED" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// Whether ARC notifies the account, through Amazon EventBridge, about EVERY
/// autoshift or only about the autoshifts that include one of its resources.
///
/// The model's `AutoshiftObserverNotificationStatus` enum (`ENABLED`,
/// `DISABLED`): "If the status is ENABLED, ARC includes all autoshift events
/// when you use the Amazon EventBridge pattern Autoshift In Progress. When the
/// status is DISABLED, ARC includes only autoshift events for autoshifts when
/// one or more of your resources is included in the autoshift." It is an
/// account-wide setting — neither operation that reads or writes it names a
/// resource — so the store holds exactly one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoshiftObserverNotificationStatus {
    Disabled,
    Enabled,
}

impl AutoshiftObserverNotificationStatus {
    /// The value this status takes on the wire.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::Enabled => "ENABLED",
        }
    }

    /// The status a wire value names, or `None` when it is not one of the two
    /// the model declares (`AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES`).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "DISABLED" => Some(Self::Disabled),
            "ENABLED" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// The autoshift observer notification status of an account that has never set
/// one: `DISABLED`, the narrower of the two.
///
/// `ENABLED` is what widens the EventBridge `Autoshift In Progress` pattern from
/// the autoshifts that include one of the account's resources to ALL autoshifts
/// (the `UpdateAutoshiftObserverNotificationStatus` documentation), so it is the
/// setting a caller opts in to; until a caller does, the store reports the
/// un-opted-in value.
pub const DEFAULT_AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS: AutoshiftObserverNotificationStatus =
    AutoshiftObserverNotificationStatus::Disabled;

/// A resource Amazon Web Services registered with ARC for zonal shift.
#[derive(Debug, Clone)]
pub struct ManagedResource {
    /// The resource's ARN, which is also its `resourceIdentifier`.
    pub arn: String,
    pub name: String,
    /// The Availability Zones the resource is deployed in; the key set of
    /// `appliedWeights` and the only values `awayFrom` may name.
    pub availability_zones: Vec<String>,
    /// Autoshifts ARC has run for this resource. Empty until the autoshift
    /// operations are implemented, which is what fills it.
    pub autoshifts: Vec<AutoshiftInResource>,
    /// The resource's practice run configuration, when one exists, and the value
    /// `practiceRunStatus` is read from.
    ///
    /// Filled for the length of one call by the accessors that answer about a
    /// managed resource ([`ComAmazonawsArczonalshiftPercdataplaneState::managed_resource`]
    /// and [`ComAmazonawsArczonalshiftPercdataplaneState::managed_resource_inventory`])
    /// from the one place the store keeps it
    /// ([`ComAmazonawsArczonalshiftPercdataplaneState::practice_run_configurations`]):
    /// a resource ARC did not create carries a configuration just as one this store
    /// was seeded with does, and the registry its owner writes holds no ARC
    /// configuration at all.
    pub practice_run_configuration: Option<PracticeRunConfiguration>,
    /// Whether the account has authorized ARC to shift traffic away from an
    /// Availability Zone for this resource on its behalf.
    ///
    /// Filled, like [`Self::practice_run_configuration`], for the length of one call
    /// by the accessors that answer about a managed resource from the one place the
    /// store keeps it
    /// ([`ComAmazonawsArczonalshiftPercdataplaneState::zonal_autoshift_statuses`]):
    /// the status is ARC's own record about a resource ARC did not create, so a
    /// resource another service registered carries the status a caller set for it
    /// just as one this store was seeded with does. A resource no caller has set one
    /// for carries the value here, `DISABLED`.
    pub zonal_autoshift_status: ZonalAutoshiftStatus,
}

impl ManagedResource {
    /// Whether a practice run configuration exists for this resource.
    ///
    /// From `ManagedResourceSummary$practiceRunStatus`: "This status tracks
    /// whether a practice run configuration exists for a resource. ... ARC
    /// updates this status; you can't set a practice run status" — so it is
    /// derived here rather than stored.
    #[must_use]
    pub fn practice_run_status(&self) -> ZonalAutoshiftStatus {
        if self.practice_run_configuration.is_some() {
            ZonalAutoshiftStatus::Enabled
        } else {
            ZonalAutoshiftStatus::Disabled
        }
    }
}

/// One zonal shift, as this service stores it.
#[derive(Debug, Clone)]
pub struct ZonalShift {
    pub zonal_shift_id: String,
    /// The `resourceIdentifier` (ARN) of the resource traffic is shifted for.
    pub resource_identifier: String,
    pub away_from: String,
    pub start_time: SystemTime,
    pub expiry_time: SystemTime,
    pub status: ZonalShiftStatus,
    pub comment: String,
    pub shift_type: ShiftType,
    /// Set only for a practice run; `None` for a customer-initiated shift.
    pub practice_run_outcome: Option<String>,
}

/// Per-service state for com-amazonaws-arczonalshift-percdataplane. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsArczonalshiftPercdataplaneState {
    /// Managed resources, keyed by `resourceIdentifier` (their ARN).
    pub managed_resources: BTreeMap<String, ManagedResource>,
    /// Every zonal shift ever started, keyed by `zonalShiftId`. A completed
    /// shift stays here: `ListZonalShifts` "lists all active and completed
    /// zonal shifts".
    pub zonal_shifts: BTreeMap<String, ZonalShift>,
    /// The source of unique `zonalShiftId`s; see [`Self::next_zonal_shift_id`].
    pub next_shift_seq: u64,
    /// The account-wide autoshift observer notification setting:
    /// `UpdateAutoshiftObserverNotificationStatus` writes it and
    /// `GetAutoshiftObserverNotificationStatus` reports it.
    pub autoshift_observer_notification_status: AutoshiftObserverNotificationStatus,
    /// The practice run configuration ARC holds for a resource, keyed by
    /// `resourceIdentifier` (its ARN).
    ///
    /// Kept BESIDE the resources rather than inside one, because a configuration is
    /// ARC's own record about a resource ARC did not create: a resource reaches ARC
    /// either seeded into [`Self::managed_resources`] or published by the service
    /// that owns it ([`published_resources`]), and a configuration written for
    /// either is held here, so the shared registry stays the sole authority on what
    /// it published.
    pub practice_run_configurations: BTreeMap<String, PracticeRunConfiguration>,
    /// The zonal autoshift status ARC holds for a resource, keyed by
    /// `resourceIdentifier` (its ARN), for the resources a caller has set one for.
    ///
    /// Kept BESIDE the resources for the same reason a practice run configuration is
    /// ([`Self::practice_run_configurations`]): the status is ARC's own record about a
    /// resource ARC did not create, and a resource reaches ARC either seeded into
    /// [`Self::managed_resources`] or published by the service that owns it, which
    /// records no ARC status for it at all.
    pub zonal_autoshift_statuses: BTreeMap<String, ZonalAutoshiftStatus>,
}

impl Default for ComAmazonawsArczonalshiftPercdataplaneState {
    fn default() -> Self {
        let mut managed_resources = BTreeMap::new();
        for resource in seed_managed_resources() {
            managed_resources.insert(resource.arn.clone(), resource);
        }
        Self {
            managed_resources,
            zonal_shifts: BTreeMap::new(),
            next_shift_seq: 0,
            autoshift_observer_notification_status:
                DEFAULT_AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS,
            practice_run_configurations: BTreeMap::new(),
            zonal_autoshift_statuses: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsArczonalshiftPercdataplaneState {
    /// Mint the next `zonalShiftId`.
    ///
    /// The model constrains `ZonalShiftId` to `^[A-Za-z0-9-]+$`, length 6..36;
    /// every id this mints satisfies both and no two are equal.
    pub fn next_zonal_shift_id(&mut self) -> String {
        self.next_shift_seq += 1;
        format!("zs-{:010}", self.next_shift_seq)
    }

    /// Materialise the status change the clock has already made due: an `ACTIVE`
    /// shift whose `expiryTime` is in the past is `EXPIRED`, whether or not
    /// anybody called.
    ///
    /// From `ZonalShiftStatus`: "EXPIRED: The zonal shift has expired (the
    /// expiry time was exceeded)."
    ///
    /// The sweep is over the whole map, which [`prune_completed_shifts`] keeps
    /// bounded, so the work one request does under the state lock is bounded too.
    ///
    /// [`prune_completed_shifts`]: Self::prune_completed_shifts
    pub fn expire_due_shifts(&mut self, now: SystemTime) {
        for shift in self.zonal_shifts.values_mut() {
            if shift.status == ZonalShiftStatus::Active && shift.expiry_time <= now {
                shift.status = ZonalShiftStatus::Expired;
            }
        }
    }

    /// The managed resource a zonal-shift mutation may act on, or the refusal it
    /// answers instead.
    ///
    /// This is the ordering every such operation shares: the authorization
    /// decision on the named target is taken FIRST, so a `resourceIdentifier`
    /// ARC holds no resource for is refused with [`target_unavailable`]
    /// rather than reported as not found.
    ///
    /// `published` is what the services that own their resources have registered
    /// ([`published_resources`]), so a resource registered through one of them is as
    /// much a target of a mutation as one this store was seeded with.
    pub fn authorized_resource(
        &self,
        published: &[::service_common::arc_managed_resources::RegisteredResource],
        resource_identifier: &str,
    ) -> Result<ManagedResource, AccessDeniedException> {
        self.managed_resource(published, resource_identifier)
            .ok_or_else(target_unavailable)
    }

    /// The stored zonal shift a mutation keyed by `zonalShiftId` may act on, or
    /// the refusal it answers instead — the same ordering as
    /// [`Self::authorized_resource`], for the operations whose target is a shift.
    ///
    /// The refusal states no reason ([`identifier_not_attributable`]): a shift id is not
    /// a resource an IAM policy grants an action on, so the denial is taken on the
    /// identifier itself, with no resolved target to report anything about.
    pub fn authorized_shift_mut(
        &mut self,
        zonal_shift_id: &str,
    ) -> Result<&mut ZonalShift, AccessDeniedException> {
        self.zonal_shifts
            .get_mut(zonal_shift_id)
            .ok_or_else(identifier_not_attributable)
    }

    /// Bound the store: drop the oldest completed shifts once it holds more than
    /// [`MAX_COMPLETED_SHIFTS_RETAINED`] of them.
    ///
    /// Call this where a shift is ADDED — never on a read path, so a read never
    /// changes what a later caller sees. The shifts still in progress are already
    /// bounded (at most one customer-initiated shift per managed resource), so the
    /// completed ones are the only part of the store that grows.
    pub fn prune_completed_shifts(&mut self) {
        // Count before allocating: the store is below the cap on all but the rare
        // call that crosses it, and that call is the only one that has work to do.
        let completed_count = self
            .zonal_shifts
            .values()
            .filter(|shift| shift.status != ZonalShiftStatus::Active)
            .count();
        let Some(excess) = completed_count
            .checked_sub(MAX_COMPLETED_SHIFTS_RETAINED)
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let mut completed: Vec<(SystemTime, String)> = self
            .zonal_shifts
            .values()
            .filter(|shift| shift.status != ZonalShiftStatus::Active)
            .map(|shift| (shift.start_time, shift.zonal_shift_id.clone()))
            .collect();
        // Oldest first, so the shifts a caller is still likely to read back are the
        // ones that stay.
        completed.sort();
        for (_, zonal_shift_id) in completed.into_iter().take(excess) {
            self.zonal_shifts.remove(&zonal_shift_id);
        }
    }

    /// The shifts in progress for one resource: every shift whose status is
    /// `ACTIVE`, in `zonalShiftId` order.
    pub fn active_shifts_for_resource(&self, resource_identifier: &str) -> Vec<&ZonalShift> {
        self.zonal_shifts
            .values()
            .filter(|shift| {
                shift.resource_identifier == resource_identifier
                    && shift.status == ZonalShiftStatus::Active
            })
            .collect()
    }

    /// The one `ACTIVE` shift of a given type for a resource, if any. A second
    /// customer-initiated shift for a resource that already has one is a
    /// conflict, which is what this answers.
    pub fn active_shift_of_type(
        &self,
        resource_identifier: &str,
        shift_type: ShiftType,
    ) -> Option<&ZonalShift> {
        self.active_shifts_for_resource(resource_identifier)
            .into_iter()
            .find(|shift| shift.shift_type == shift_type)
    }

    /// The shift currently IN EFFECT for a resource: of the shifts in progress,
    /// the one whose [`ShiftType::precedence`] rank is lowest (ties broken by the
    /// earlier start, then by id, so the answer is stable).
    pub fn applied_shift(&self, resource_identifier: &str) -> Option<&ZonalShift> {
        self.active_shifts_for_resource(resource_identifier)
            .into_iter()
            .min_by(|a, b| {
                a.shift_type
                    .precedence()
                    .cmp(&b.shift_type.precedence())
                    .then(a.start_time.cmp(&b.start_time))
                    .then(a.zonal_shift_id.cmp(&b.zonal_shift_id))
            })
    }

    /// `appliedWeights` for a resource deployed in `availability_zones`: 0 for the
    /// Availability Zone the shift in effect moves traffic away from, 1 for every
    /// other zone it is deployed in.
    ///
    /// From `ManagedResourceSummary$appliedWeights`: "A collection of key-value
    /// pairs that indicate whether resources are active in Availability Zones or
    /// not. The key name is the Availability Zone where the resource is
    /// deployed. The value is 1 or 0." Derived from the shifts in progress, so a
    /// shift that is canceled or has expired no longer moves any weight.
    ///
    /// The zones are the caller's, read off the managed resource ARC holds —
    /// whether this store was seeded with it or the service that owns it registered
    /// it ([`Self::managed_resource_inventory`]) — so one reading of the weights
    /// serves both.
    pub fn applied_weights(
        &self,
        resource_identifier: &str,
        availability_zones: &[String],
    ) -> HashMap<String, f32> {
        let away_from = self
            .applied_shift(resource_identifier)
            .map(|shift| shift.away_from.clone());
        availability_zones
            .iter()
            .map(|az| {
                let weight = if Some(az.as_str()) == away_from.as_deref() {
                    0.0
                } else {
                    1.0
                };
                (az.clone(), weight)
            })
            .collect()
    }

    /// The `zonalShifts` member of a managed resource: the shifts in progress
    /// for it, each flagged `APPLIED` when it is the one in effect and
    /// `NOT_APPLIED` otherwise.
    pub fn zonal_shifts_in_resource(&self, resource_identifier: &str) -> Vec<ZonalShiftInResource> {
        let applied = self
            .applied_shift(resource_identifier)
            .map(|shift| shift.zonal_shift_id.clone());
        self.active_shifts_for_resource(resource_identifier)
            .into_iter()
            .map(|shift| ZonalShiftInResource {
                applied_status: if Some(shift.zonal_shift_id.as_str()) == applied.as_deref() {
                    "APPLIED".to_string()
                } else {
                    "NOT_APPLIED".to_string()
                },
                zonal_shift_id: shift.zonal_shift_id.clone(),
                resource_identifier: shift.resource_identifier.clone(),
                away_from: shift.away_from.clone(),
                expiry_time: as_date_time(shift.expiry_time),
                start_time: as_date_time(shift.start_time),
                comment: shift.comment.clone(),
                shift_type: Some(shift.shift_type.as_wire().to_string()),
                practice_run_outcome: shift.practice_run_outcome.clone(),
            })
            .collect()
    }
}

/// A stored instant as the wire timestamp the model declares.
#[must_use]
pub fn as_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Parse an `expiresIn` into the length of time it names.
///
/// From the `expiresIn` documentation: "specify a whole number, and then one of
/// the following, with no space: A lowercase letter m: ... minutes. A lowercase
/// letter h: ... hours." `None` when the value is not of that form, or names a
/// length of time past [`MAX_EXPIRES_IN`].
///
/// The unit is taken with `strip_suffix`, which splits on the character itself:
/// a request-supplied value ending in a multi-byte character is answered `None`
/// rather than indexed into on a byte offset that is not a character boundary.
#[must_use]
pub fn parse_expires_in(expires_in: &str) -> Option<Duration> {
    let (digits, unit_secs) = if let Some(digits) = expires_in.strip_suffix('m') {
        (digits, 60_u64)
    } else {
        // The other unit the documentation names; a value ending in neither is
        // not of that form at all.
        let digits = expires_in.strip_suffix('h')?;
        (digits, 60 * 60)
    };
    let count: u64 = digits.parse().ok()?;
    if count == 0 {
        return None;
    }
    let length = Duration::from_secs(count.checked_mul(unit_secs)?);
    (length <= MAX_EXPIRES_IN).then_some(length)
}

/// Marks a token as one THIS service issued, so a token it did not issue is
/// refused on its prefix rather than on whether its bytes happen to decode.
const PAGE_TOKEN_PREFIX: &str = "arc-zs-1:";

/// A pagination token for a list operation: the last key the previous page
/// returned, encoded with `aws_smithy_types::base64` — the codec the wire
/// protocols already carry, so no separate dependency encodes it — which keeps
/// the token a caller echoes back opaque.
#[must_use]
pub fn encode_page_token(last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}

/// The key [`encode_page_token`] encoded, or `None` when the token is not one
/// this service issued.
#[must_use]
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .map(str::to_string)
}

/// What a list operation's `nextToken` and `maxResults` ask for: the key to
/// resume strictly after, and how many items this page may hold.
#[derive(Debug)]
pub struct PageRequest {
    pub after: Option<String>,
    pub page_size: usize,
}

/// Why a list operation refuses its paging parameters: the
/// `ValidationExceptionReason` and the message, which the calling operation
/// lifts into its own modelled `ValidationException`.
#[derive(Debug)]
pub struct PageRequestRefused {
    pub reason: &'static str,
    pub message: String,
}

/// Read a list operation's paging parameters, so every paginated operation of
/// this service pages the same way.
///
/// The modelled `MaxResults` range (1..100) is refused at the boundary before a
/// handler runs, so the clamp here never changes a request the framework
/// accepted; it is there so a page size is always a usable count. An absent
/// `maxResults` takes [`DEFAULT_PAGE_SIZE`]. A `nextToken` this service did not
/// issue names no page, so it is refused rather than served as "from the
/// beginning".
pub fn page_request(
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<PageRequest, PageRequestRefused> {
    let after = match next_token {
        None => None,
        Some(token) => Some(decode_page_token(token).ok_or_else(|| PageRequestRefused {
            reason: "InvalidToken",
            message: "nextToken is not a token this service issued".to_string(),
        })?),
    };
    Ok(PageRequest {
        after,
        page_size: max_results.map_or(DEFAULT_PAGE_SIZE, |max| max.clamp(1, 100) as usize),
    })
}

/// The managed resources every emulator of this service starts with: the
/// resource types ARC registers for zonal shift — "Amazon EC2 Auto Scaling
/// groups[,] Amazon Elastic Kubernetes Service[,] Application Load Balancer[,]
/// Network Load Balancer" (the `resourceIdentifier` documentation) — in one
/// Region.
fn seed_managed_resources() -> Vec<ManagedResource> {
    vec![
        ManagedResource {
            arn: ::engine::emulator_arn(
                "elasticloadbalancing",
                SEED_REGION,
                "loadbalancer/app/zonal-shift-alb/73e2d6bc24d8a067",
            ),
            name: "zonal-shift-alb".to_string(),
            availability_zones: vec![
                "use1-az1".to_string(),
                "use1-az2".to_string(),
                "use1-az4".to_string(),
            ],
            autoshifts: Vec::new(),
            practice_run_configuration: None,
            zonal_autoshift_status: ZonalAutoshiftStatus::Disabled,
        },
        ManagedResource {
            arn: ::engine::emulator_arn(
                "elasticloadbalancing",
                SEED_REGION,
                "loadbalancer/net/zonal-shift-nlb/1a2b3c4d5e6f7890",
            ),
            name: "zonal-shift-nlb".to_string(),
            availability_zones: vec!["use1-az2".to_string(), "use1-az6".to_string()],
            autoshifts: Vec::new(),
            practice_run_configuration: None,
            zonal_autoshift_status: ZonalAutoshiftStatus::Disabled,
        },
        ManagedResource {
            arn: ::engine::emulator_arn(
                "autoscaling",
                SEED_REGION,
                "autoScalingGroup:4e2b6e5a-9c1d-4f3a-8b7e-0d6c5a4b3e21:autoScalingGroupName/zonal-shift-asg",
            ),
            name: "zonal-shift-asg".to_string(),
            availability_zones: vec![
                "use1-az1".to_string(),
                "use1-az2".to_string(),
                "use1-az4".to_string(),
                "use1-az6".to_string(),
            ],
            autoshifts: Vec::new(),
            practice_run_configuration: None,
            zonal_autoshift_status: ZonalAutoshiftStatus::Disabled,
        },
    ]
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsArczonalshiftPercdataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsArczonalshiftPercdataplaneState>)
}
