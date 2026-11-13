// SPDX-License-Identifier: CC0-1.0

//! Reserved Instances: the purchases the account holds, and the modifications of them.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! [`super::client_vpn`] gives: the two stores, the form of their ids, the refusals they
//! share and the clock-owed `processing` -> `fulfilled` move of a modification are one
//! subject, and keeping them together leaves `state.rs` readable — and leaves this fix out
//! of the end of that file, which is where every concurrently landing EC2 fix inserts.
//! Everything here is re-exported by `crate::state`, so a handler reaches it as
//! `crate::state::insert_reserved_instances` exactly as it reaches the helpers that live in
//! the parent module.
//!
//! The Reserved Instance MARKETPLACE listings of those purchases are the same subject and
//! live at the end of this file: a listing names the purchase it sells, is created only for
//! one the account holds in effect, and `CancelReservedInstancesListing` withdraws it.
//!
//! The stores themselves are fields of [`ComAmazonawsEc2Amazonec2State`]
//! (`reserved_instances`, `reserved_instances_modifications` and the client-token map
//! beside it), because the state struct is one struct; it is only the code that reads and
//! writes them that lives here.
//!
//! ## What a modification does to the purchases
//!
//! `ModifyReservedInstances` does not edit a Reserved Instance in place. It records a
//! modification REQUEST — `processing` at first — and, when that request is fulfilled,
//! "the Reserved Instances created as part of the modification request"
//! (`DescribeReservedInstancesModificationsRequest$Filters`, the
//! `modification-result.reserved-instances-id` entry, whose "ID is only available when the
//! status of the modification is `fulfilled`") take the place of the ones it named, which
//! are left in the modelled `ReservedInstanceState` enum's `retired`. That is the ONE
//! writer of a Reserved Instance a caller did not buy, and it runs from
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`], so the new purchases
//! exist from the instant the fulfilment fell due whether or not anybody called again.

use super::*;

use crate::ops::types::{
    InstanceCount, PriceSchedule, PriceScheduleSpecification, ReservedInstances,
    ReservedInstancesConfiguration, ReservedInstancesId, ReservedInstancesListing,
    ReservedInstancesModification, ReservedInstancesModificationResult,
};

// ---------------------------------------------------------------------------
// The purchases the account holds
// ---------------------------------------------------------------------------

/// A Reserved Instance purchase the account has scheduled for a future date and that has
/// not been paid for yet: the `ReservedInstanceState` enum's `queued` member
/// (`smithy_query(query="shape",shape="ReservedInstanceState")`). It is the ONE state a
/// queued purchase can be deleted from.
pub const RESERVED_INSTANCE_STATE_QUEUED: &str = "queued";

/// A queued purchase that has been deleted: the `ReservedInstanceState` enum's
/// `queued-deleted` member. The enum carrying this value is why a delete moves the record
/// rather than dropping it — the purchase is still reportable after it is cancelled.
pub const RESERVED_INSTANCE_STATE_QUEUED_DELETED: &str = "queued-deleted";

/// A Reserved Instance that is paid for and in effect: the `ReservedInstanceState` enum's
/// `active` member.
pub const RESERVED_INSTANCE_STATE_ACTIVE: &str = "active";

/// A Reserved Instance whose term this account no longer holds: the
/// `ReservedInstanceState` enum's `retired` member. Where a fulfilled modification leaves
/// the purchases it was asked to change
/// ([`ComAmazonawsEc2Amazonec2State::fulfill_due_reserved_instances_modifications`]).
pub const RESERVED_INSTANCE_STATE_RETIRED: &str = "retired";

/// The currency "specified using ISO 4217 standard currency codes. At this time, the only
/// supported currency is `USD`" (`ReservedInstances$CurrencyCode`), which is also the one
/// member of the modelled `CurrencyCodeValues` enum.
pub const RESERVED_INSTANCE_CURRENCY_CODE: &str = "USD";

/// The scope of a Reserved Instance bought in one Availability Zone: the `scope` enum's
/// `Availability Zone` member (`smithy_query(query="shape",shape="scope")`).
pub const RESERVED_INSTANCE_SCOPE_ZONE: &str = "Availability Zone";

/// The scope of a Reserved Instance bought for a whole Region: the `scope` enum's `Region`
/// member. Such a Reserved Instance names no Availability Zone.
pub const RESERVED_INSTANCE_SCOPE_REGION: &str = "Region";

/// How many characters the hyphenated form of a UUID has: `8-4-4-4-12` hexadecimal digits
/// plus the four hyphens. With [`UUID_HYPHEN_POSITIONS`] it is the whole of that form, and
/// so of [`reserved_instances_id_is_well_formed`].
const UUID_HYPHENATED_CHARS: usize = 36;

/// Where the four hyphens of the hyphenated UUID form sit, counting characters from the
/// start: the `8-4-4-4-12` grouping of its 32 hexadecimal digits.
const UUID_HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];

/// The first four fields of an id
/// [`ComAmazonawsEc2Amazonec2State::mint_reserved_instances_id`] lays out, leaving the
/// 48-bit node field for the store's own counter.
///
/// A UUID's version nibble is the first of its third field and its variant nibble the first
/// of its fourth: `8` and `8` here, which is RFC 9562's version 8 — the version reserved
/// for a UUID whose bits an implementation lays out itself — under the RFC 4122 variant. So
/// a minted id is a canonical UUID that says how it was made, rather than claiming the
/// randomness of a version 4 or the name hash of a version 5.
const RESERVED_INSTANCES_ID_LAYOUT_PREFIX: &str = "ec200000-0000-8000-8000";

/// How many Reserved Instances this emulator stores before a purchase is refused.
///
/// CHOSEN, like [`MAX_STORED_DEDICATED_HOSTS`] and for the same reason: nothing in this
/// emulator reclaims a stored Reserved Instance — a delete moves the record to
/// [`RESERVED_INSTANCE_STATE_QUEUED_DELETED`] and a fulfilled modification moves it to
/// [`RESERVED_INSTANCE_STATE_RETIRED`] rather than dropping it, so a purchase that is over
/// still occupies the ceiling — so the store needs a ceiling that is not the process's
/// memory. It is well above any published per-Region Reserved Instance quota a test would
/// exercise.
pub const MAX_STORED_RESERVED_INSTANCES: usize = 4096;

/// What one Reserved Instance is stored with: the facts of the purchase itself, everything
/// the service derives being left to
/// [`ComAmazonawsEc2Amazonec2State::insert_reserved_instances`].
#[derive(Debug, Clone)]
pub struct ReservedInstancesSpec {
    /// "The instance type on which the Reserved Instance can be used"
    /// (`ReservedInstances$InstanceType`): one of `INSTANCE_TYPE_VALUES`.
    pub instance_type: String,
    /// "The Availability Zone in which the Reserved Instance can be used", for a zonal
    /// purchase; `None` for one of Regional scope.
    pub availability_zone: Option<String>,
    /// The Region the purchase was made in, which is where its Availability Zone's id is
    /// read from ([`availability_zones_in_region`]).
    pub region: String,
    /// "The number of reservations purchased" (`ReservedInstances$InstanceCount`).
    pub instance_count: i32,
    /// "The duration of the Reserved Instance, in seconds"
    /// (`ReservedInstances$Duration`).
    pub duration_seconds: i64,
    /// "The offering class of the Reserved Instance": one of `OFFERING_CLASS_TYPE_VALUES`.
    pub offering_class: String,
    /// "The Reserved Instance offering type": one of `OFFERING_TYPE_VALUES_VALUES`.
    pub offering_type: String,
    /// "The Reserved Instance product platform description": one of
    /// `R_I_PRODUCT_DESCRIPTION_VALUES`.
    pub product_description: String,
    /// "The state of the Reserved Instance purchase": one of
    /// `RESERVED_INSTANCE_STATE_VALUES`, [`RESERVED_INSTANCE_STATE_QUEUED`] for a queued
    /// purchase.
    pub state: String,
    /// "The date and time the Reserved Instance started" — for a queued purchase, the
    /// future date the purchase is scheduled for.
    pub start: SystemTime,
    /// "The purchase price of the Reserved Instance" (`ReservedInstances$FixedPrice`).
    pub fixed_price: f32,
    /// "The usage price of the Reserved Instance, per hour"
    /// (`ReservedInstances$UsagePrice`).
    pub usage_price: f32,
}

/// Whether `id` is a well-formed Reserved Instances id.
///
/// A Reserved Instance's id is the hyphenated form of a UUID (see
/// [`ComAmazonawsEc2Amazonec2State::mint_reserved_instances_id`]), so anything else is a
/// value EC2 cannot read as an id at all and refuses with `InvalidParameterValue` BEFORE
/// it looks anything up — which is what leaves the modelled per-item
/// `reserved-instances-id-invalid` channel of `DeleteQueuedReservedInstances` for ids that
/// ARE well formed and name nothing.
///
/// Reading a caller's value NEVER panics and never allocates: the hyphenated form is 36
/// ASCII characters ([`UUID_HYPHENATED_CHARS`]) — a hyphen at each of
/// [`UUID_HYPHEN_POSITIONS`] and a hexadecimal digit everywhere else — so the check walks
/// the bytes once. Any UUID version passes, because an id a caller names may have been
/// minted by AWS rather than by this store; only the FORM is this service's to read.
pub fn reserved_instances_id_is_well_formed(id: &str) -> bool {
    id.len() == UUID_HYPHENATED_CHARS
        && id.bytes().enumerate().all(|(position, byte)| {
            if UUID_HYPHEN_POSITIONS.contains(&position) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// Refuse a request that named something that is not a Reserved Instances id: HTTP 400
/// `InvalidParameterValue`, naming the member and the value it could not read, the way
/// EC2 words a value it rejects.
pub fn invalid_reserved_instances_id<E>(reserved_instances_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Invalid value '{reserved_instances_id}' for reservedInstancesId. \
         A Reserved Instances ID is a UUID"
    ))
}

/// Refuse a `ReservedInstancesIds` entry of the REPORT of this account's purchases that is not
/// a Reserved Instances id: HTTP 400 `InvalidParameterValue`, naming the member as the request
/// spells it and the value it could not read, and nothing after it.
///
/// Deliberately not [`invalid_reserved_instances_id`]: that is the sentence
/// `DeleteQueuedReservedInstances` was recorded answering for its own `reservedInstancesId`
/// member, and `DescribeReservedInstances` was recorded wording its member's refusal this way
/// instead. Only the first [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters of the value are quoted
/// ([`echoed_resource_id`]).
pub fn reported_reserved_instances_id_unreadable<E>(reserved_instances_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Invalid value '{id}' for ReservedInstancesId.",
        id = echoed_resource_id(reserved_instances_id)
    ))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Reserved Instances fit inside the store's ceiling
    /// ([`MAX_STORED_RESERVED_INSTANCES`]).
    ///
    /// A purchase, or a modification that replaces purchases with new ones, must ask this
    /// BEFORE inserting anything and refuse when it answers `false`, the way an allocation
    /// asks [`Self::has_dedicated_host_capacity`]: one call can create more than one, so
    /// the whole batch is gated rather than each record as it is written.
    /// [`Self::insert_reserved_instances`] refuses on its own too, so no path can grow the
    /// store past the ceiling even if it forgets to ask.
    pub fn has_reserved_instances_capacity(&self, count: usize) -> bool {
        self.reserved_instances.len().saturating_add(count) <= MAX_STORED_RESERVED_INSTANCES
    }

    /// A fresh, unused Reserved Instances id.
    ///
    /// A Reserved Instance's id is a UUID — the ids AWS's own Reserved Instance examples
    /// carry read like `b847fa93-e282-4f55-b59a-1342f5bd7c02`, not like the
    /// `<prefix>-<hex>` ids [`Self::next_id`] mints — and the model's
    /// `ReservedInstances$ReservedInstancesId` is a plain `String` that pins no pattern,
    /// so the form is decided here, in the one place an id is minted.
    ///
    /// The 128 bits are laid out here rather than hashed or drawn from randomness: the
    /// store's own monotonic counter fills the node field of
    /// [`RESERVED_INSTANCES_ID_LAYOUT_PREFIX`], whose version nibble says exactly that (RFC
    /// 9562's version 8). So the id is a canonical UUID, unique within a run, the same
    /// across runs, and never taken from the request. The counter is `u64` and the node
    /// field holds 48 bits, so the store's ceiling ([`MAX_STORED_RESERVED_INSTANCES`]) is
    /// reached long before the layout could repeat an id.
    pub fn mint_reserved_instances_id(&mut self) -> String {
        self.mint_uuid()
    }

    /// A fresh, unused UUID in the layout this module gives every id of the Reserved
    /// Instances domain: the ONE place those 128 bits are laid out, so an id of any of its
    /// kinds — a purchase's, a modification's, an exchange's — draws on the same counter and
    /// no two can collide.
    fn mint_uuid(&mut self) -> String {
        format!(
            "{RESERVED_INSTANCES_ID_LAYOUT_PREFIX}-{node:012x}",
            node = self.next_uuid_node()
        )
    }

    /// The 48-bit node field the next id this module lays out carries: the store's own
    /// monotonic counter, so no two ids of either kind can collide.
    fn next_uuid_node(&mut self) -> u64 {
        self.id_counter = self.id_counter.saturating_add(1);
        self.id_counter & 0xffff_ffff_ffff
    }

    /// Store a Reserved Instance, and answer the `ReservedInstancesId` it was given, or
    /// `None` when the store is already at its ceiling
    /// ([`Self::has_reserved_instances_capacity`]).
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id, the currency, the tenancy, the scope (a Reserved Instance that
    /// names an Availability Zone is zonal, one that names none is Regional), that zone's
    /// id, and the instant the term ends, which is its start plus its duration — so two
    /// writers cannot report the same Reserved Instance differently. Nothing on the record
    /// is caller-chosen free text (a purchase names an offering, a count and a zone), so
    /// there is nothing to charge against the store's byte budget the way
    /// [`Self::insert_address`] charges an address's tags; the count ceiling is what bounds
    /// the store, because nothing reclaims a record.
    pub fn insert_reserved_instances(&mut self, spec: ReservedInstancesSpec) -> Option<String> {
        if !self.has_reserved_instances_capacity(1) {
            return None;
        }
        let reserved_instances_id = self.mint_reserved_instances_id();
        let availability_zone_id = spec.availability_zone.as_deref().and_then(|zone| {
            availability_zones_in_region(&spec.region)
                .into_iter()
                .find(|known| known.zone_name == zone)
                .map(|known| known.zone_id)
        });
        let scope = if spec.availability_zone.is_some() {
            RESERVED_INSTANCE_SCOPE_ZONE
        } else {
            RESERVED_INSTANCE_SCOPE_REGION
        };
        let duration = spec.duration_seconds.max(0);
        let end = spec
            .start
            .checked_add(Duration::from_secs(u64::try_from(duration).unwrap_or(0)))
            .unwrap_or(spec.start);
        let reserved_instances = ReservedInstances {
            currency_code: Some(RESERVED_INSTANCE_CURRENCY_CODE.to_string()),
            instance_tenancy: Some(DEFAULT_INSTANCE_TENANCY.to_string()),
            offering_class: Some(spec.offering_class),
            offering_type: Some(spec.offering_type),
            // "The recurring charge tag assigned to the resource"
            // (`ReservedInstances$RecurringCharges`): an upfront-priced term has none.
            recurring_charges: Some(Vec::new()),
            scope: Some(scope.to_string()),
            tags: Some(Vec::new()),
            availability_zone_id,
            reserved_instances_id: Some(reserved_instances_id.clone()),
            instance_type: Some(spec.instance_type),
            availability_zone: spec.availability_zone,
            start: Some(system_time_to_date_time(spec.start)),
            end: Some(system_time_to_date_time(end)),
            duration: Some(duration),
            usage_price: Some(spec.usage_price),
            fixed_price: Some(spec.fixed_price),
            instance_count: Some(spec.instance_count),
            product_description: Some(spec.product_description),
            state: Some(spec.state),
        };
        self.reserved_instances
            .insert(reserved_instances_id.clone(), reserved_instances);
        Some(reserved_instances_id)
    }
}

// ---------------------------------------------------------------------------
// The modifications of those purchases
// ---------------------------------------------------------------------------

/// The id prefix a Reserved Instances modification carries: `rimod-`, the form AWS's own
/// `ReservedInstancesModificationId` values read in
/// (`rimod-<uuid>`). The model puts no pattern on
/// `ReservedInstancesModificationId` (a plain `String`), so the form is decided in
/// [`ComAmazonawsEc2Amazonec2State::mint_reserved_instances_modification_id`], the one
/// place such an id is minted.
pub const RESERVED_INSTANCES_MODIFICATION_ID_PREFIX: &str = "rimod";

/// "processing" — the status a modification request carries while EC2 is still working on
/// it, the first of the three the `status` filter documents: "The status of the Reserved
/// Instances modification request (`processing` | `fulfilled` | `failed`)"
/// (`DescribeReservedInstancesModificationsRequest$Filters`).
pub const RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING: &str = "processing";

/// "fulfilled" — the status of a modification request EC2 has carried out: the new Reserved
/// Instances exist and the ones the request named are
/// [`RESERVED_INSTANCE_STATE_RETIRED`]. The move a `processing` request owes the clock
/// ([`TRANSITION_DELAY`] later, through
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED: &str = "fulfilled";

/// "failed" — the status of a modification request EC2 could not carry out, which carries
/// the reason in `ReservedInstancesModification$StatusMessage` ("The reason for the
/// status").
pub const RESERVED_INSTANCES_MODIFICATION_STATUS_FAILED: &str = "failed";

/// How many modification requests this emulator stores before one is refused.
///
/// CHOSEN, for the reason [`MAX_STORED_RECORDS_PER_STORE`] gives: nothing here reclaims a
/// modification record — a fulfilled request is still reportable — so without a count
/// ceiling repeated calls would grow this process's memory without bound.
pub const MAX_STORED_RESERVED_INSTANCES_MODIFICATIONS: usize = MAX_STORED_RECORDS_PER_STORE;

/// How many target configurations one modification request may carry.
///
/// CHOSEN: the model declares no `@length` on `ReservedInstancesConfigurationList`, and each
/// entry is caller-chosen text and a count that the stored record keeps for as long as the
/// modification lives, so the handler is the only place the size of that list can be
/// bounded. At the scale of the 100-entry ceiling the model does put on the comparable
/// Reserved-Instances id list of `DeleteQueuedReservedInstances`, and far above the number
/// of configurations one purchase can usefully be split into.
pub const MAX_RESERVED_INSTANCES_TARGET_CONFIGURATIONS: usize = 100;

/// `InvalidInput` (HTTP 400): a member of the request carries a value this operation cannot
/// act on — the code `ModifyReservedInstances` answers for a `reservedInstancesIds` entry
/// it cannot resolve, as its recorded refusal of an unknown Reserved Instance shows.
pub const INVALID_INPUT: &str = "InvalidInput";

/// `ReservedInstancesLimitExceeded` (HTTP 400): the call would take the account past the
/// number of Reserved Instances it may hold — EC2's `<Resource>LimitExceeded` form. The
/// ceiling a refusal reports is this store's ([`MAX_STORED_RESERVED_INSTANCES`] /
/// [`MAX_STORED_RESERVED_INSTANCES_MODIFICATIONS`]), not a published AWS quota.
pub const RESERVED_INSTANCES_LIMIT_EXCEEDED: &str = "ReservedInstancesLimitExceeded";

/// Refuse a value this operation cannot act on: HTTP 400 [`INVALID_INPUT`].
///
/// `message` is built the way the service words this code — `Invalid value for
/// '<member>'.` and then what was wrong with it — so the caller learns which member to fix
/// from the text alone. Reach it through one of the constructors below rather than wording
/// a member's complaint twice.
pub fn invalid_input<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_INPUT, 400, message)
}

/// Refuse a `reservedInstancesIds` entry the account holds no Reserved Instance for: HTTP
/// 400 [`INVALID_INPUT`], naming the member and the id the request asked about.
pub fn reserved_instances_not_found<E>(reserved_instances_id: &str) -> Scaffolded<E> {
    invalid_input(format!(
        "Invalid value for 'reservedInstancesIds'. \
         Reserved Instance {reserved_instances_id} is not found."
    ))
}

/// Refuse a `reservedInstancesIds` entry whose Reserved Instance is not in effect: HTTP 400
/// [`INVALID_INPUT`], naming the member, the id and the state the store holds it in.
///
/// Only an [`RESERVED_INSTANCE_STATE_ACTIVE`] Reserved Instance can be modified — a queued
/// purchase has not been made yet, and a retired one is over — so the state is what the
/// caller has to act on, and it comes from this store rather than from the request.
pub fn reserved_instances_not_active<E>(reserved_instances_id: &str, state: &str) -> Scaffolded<E> {
    invalid_input(format!(
        "Invalid value for 'reservedInstancesIds'. \
         Reserved Instance {reserved_instances_id} is in the '{state}' state."
    ))
}

/// Refuse a call that would take a Reserved Instances store past its ceiling: HTTP 400
/// [`RESERVED_INSTANCES_LIMIT_EXCEEDED`].
pub fn reserved_instances_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(RESERVED_INSTANCES_LIMIT_EXCEEDED, 400, message)
}

/// The facts of a purchase that a modification may NOT change, as a value two purchases of
/// one request are compared by: "The Reserved Instances to be modified must be identical,
/// except for Availability Zone, network platform, and instance type"
/// (`ModifyReservedInstances`).
///
/// So the term and the offering are here, and the three members that sentence excepts are
/// not. The instant a purchase STARTED is not here either: two purchases of the same term
/// bought at different moments are the same offering held from different instants, which is
/// what the exception leaves room for, and comparing them by it would refuse every batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedInstancesIdentity {
    /// "The duration of the Reserved Instance, in seconds"
    /// (`ReservedInstances$Duration`).
    pub duration: Option<i64>,
    /// "The offering class of the Reserved Instance"
    /// (`ReservedInstances$OfferingClass`).
    pub offering_class: Option<String>,
    /// "The Reserved Instance offering type" (`ReservedInstances$OfferingType`).
    pub offering_type: Option<String>,
    /// "The Reserved Instance product platform description"
    /// (`ReservedInstances$ProductDescription`).
    pub product_description: Option<String>,
}

/// What a modification may not change about `reserved_instances`.
pub fn reserved_instances_identity(
    reserved_instances: &ReservedInstances,
) -> ReservedInstancesIdentity {
    ReservedInstancesIdentity {
        duration: reserved_instances.duration,
        offering_class: reserved_instances.offering_class.clone(),
        offering_type: reserved_instances.offering_type.clone(),
        product_description: reserved_instances.product_description.clone(),
    }
}

/// Refuse a request whose Reserved Instances are not interchangeable: HTTP 400
/// [`INVALID_INPUT`], in the operation's own words for the rule
/// ([`reserved_instances_identity`] is what they are compared by).
pub fn reserved_instances_not_identical<E>() -> Scaffolded<E> {
    invalid_input(
        "Invalid value for 'reservedInstancesIds'. The Reserved Instances to be modified \
         must be identical, except for Availability Zone, network platform, and instance \
         type.",
    )
}

/// One stored modification request: the modelled [`ReservedInstancesModification`] every
/// reader of it reports, plus the move the clock owes it and the Region it was made in.
///
/// The view is the modelled shape itself rather than a parallel record, for the reason
/// [`CapacityReservationRecord`] keeps its own that way: every member a writer sets
/// round-trips through a reader without a field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct ReservedInstancesModificationRecord {
    /// The modification as the model shapes it.
    pub view: ReservedInstancesModification,
    /// The `processing` -> `fulfilled` move the clock owes this request, while one is owed.
    pub pending: Option<ClockTransition<&'static str>>,
    /// The Region the modification was asked for in, which is where the Availability Zone
    /// of a new Reserved Instance is resolved against
    /// ([`availability_zones_in_region`]). The modelled shape carries no member for it.
    pub region: String,
}

/// What ONE modification request is stored with, beyond everything the service derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_reserved_instances_modification`] mints the id,
/// stamps the dates and sets the status).
#[derive(Debug, Clone)]
pub struct ReservedInstancesModificationSpec {
    /// "The IDs of the Reserved Instances to modify"
    /// (`ModifyReservedInstancesRequest$ReservedInstancesIds`), already resolved against
    /// the store by the caller.
    pub reserved_instances_ids: Vec<String>,
    /// "The configuration settings for the Reserved Instances to modify"
    /// (`ModifyReservedInstancesRequest$TargetConfigurations`), as the request gave them.
    pub target_configurations: Vec<ReservedInstancesConfiguration>,
    /// "A unique, case-sensitive token you provide to ensure idempotency of your
    /// modification request" (`ModifyReservedInstancesRequest$ClientToken`), if the request
    /// carried one.
    pub client_token: Option<String>,
    /// The Region the request was signed for.
    pub region: String,
    /// When the request was made: the create, effective and update dates of a modification
    /// that has just been recorded.
    pub requested_at: SystemTime,
}

/// The bytes one stored modification request will hold: the `ClientToken`, the text of each
/// target configuration and the Reserved Instances ids the record keeps, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. The id, the dates and the
/// status are derived, so they are fixed-size; the ids are charged because the record holds
/// one entry per id for as long as the modification lives, even though each was resolved
/// against this store rather than taken on trust from the caller.
///
/// The ONE definition of that charge, the way [`flow_log_payload_bytes`] is for a flow log:
/// the handler asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_reserved_instances_modification`] charges exactly
/// it.
pub fn reserved_instances_modification_payload_bytes(
    spec: &ReservedInstancesModificationSpec,
) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    spec.target_configurations
        .iter()
        .map(|configuration| {
            text(configuration.availability_zone.as_deref())
                .saturating_add(text(configuration.availability_zone_id.as_deref()))
                .saturating_add(text(configuration.instance_type.as_deref()))
                .saturating_add(text(configuration.platform.as_deref()))
                .saturating_add(text(configuration.scope.as_deref()))
        })
        .fold(STORED_RECORD_OVERHEAD_BYTES, usize::saturating_add)
        .saturating_add(text(spec.client_token.as_deref()))
        .saturating_add(
            spec.reserved_instances_ids
                .iter()
                .map(String::len)
                .sum::<usize>(),
        )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another modification request fits inside the store's ceiling
    /// ([`MAX_STORED_RESERVED_INSTANCES_MODIFICATIONS`]).
    ///
    /// The same contract as [`Self::has_reserved_instances_capacity`]: the handler asks
    /// this BEFORE storing anything and refuses with
    /// [`reserved_instances_limit_exceeded`] when it answers `false`.
    pub fn has_reserved_instances_modification_capacity(&self) -> bool {
        self.reserved_instances_modifications.len() < MAX_STORED_RESERVED_INSTANCES_MODIFICATIONS
    }

    /// A fresh, unused Reserved Instances modification id: `rimod-<uuid>`, the form AWS's
    /// own modification ids read in.
    ///
    /// The UUID part is laid out by the same counter and layout as a Reserved Instances id
    /// ([`Self::mint_reserved_instances_id`]), so it is unique within a run and the same
    /// across runs, and never taken from the request.
    pub fn mint_reserved_instances_modification_id(&mut self) -> String {
        format!(
            "{RESERVED_INSTANCES_MODIFICATION_ID_PREFIX}-{uuid}",
            uuid = self.mint_uuid()
        )
    }

    /// The modification id a `ClientToken` has already minted, if it has:
    /// "A unique, case-sensitive token you provide to ensure idempotency of your
    /// modification request" (`ModifyReservedInstancesRequest$ClientToken`), so the repeat
    /// of a request answers with the modification the first one made instead of making a
    /// second.
    pub fn reserved_instances_modification_for_token(&self, client_token: &str) -> Option<&str> {
        self.reserved_instances_modification_client_tokens
            .get(client_token)
            .map(String::as_str)
    }

    /// Record a modification request, and answer the `ReservedInstancesModificationId` it
    /// was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id, the status the request starts in
    /// ([`RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING`]), the create, effective and
    /// update dates, and the `processing` -> `fulfilled` move the clock owes it — so two
    /// writers cannot report one modification differently. The caller-chosen bytes the
    /// record retains ([`reserved_instances_modification_payload_bytes`]) are charged
    /// against the store's byte budget here, so a writer cannot store them uncharged.
    ///
    /// The modification results carry the target configurations and NO
    /// `ReservedInstancesId` yet: that id "is only available when the status of the
    /// modification is `fulfilled`"
    /// (`DescribeReservedInstancesModificationsRequest$Filters`), which is what
    /// [`Self::fulfill_due_reserved_instances_modifications`] fills in.
    pub fn insert_reserved_instances_modification(
        &mut self,
        spec: ReservedInstancesModificationSpec,
    ) -> String {
        let payload_bytes = reserved_instances_modification_payload_bytes(&spec);
        let modification_id = self.mint_reserved_instances_modification_id();
        let requested_at = system_time_to_date_time(spec.requested_at);
        let view = ReservedInstancesModification {
            client_token: spec.client_token.clone(),
            create_date: Some(requested_at),
            // "The time for the modification to become effective"
            // (`ReservedInstancesModification$EffectiveDate`): a modification of a
            // Reserved Instance that is already in effect takes effect from the request.
            effective_date: Some(requested_at),
            update_date: Some(requested_at),
            modification_results: Some(
                spec.target_configurations
                    .into_iter()
                    .map(|target_configuration| ReservedInstancesModificationResult {
                        reserved_instances_id: None,
                        target_configuration: Some(target_configuration),
                    })
                    .collect(),
            ),
            reserved_instances_ids: Some(
                spec.reserved_instances_ids
                    .iter()
                    .map(|reserved_instances_id| ReservedInstancesId {
                        reserved_instances_id: Some(reserved_instances_id.clone()),
                    })
                    .collect(),
            ),
            reserved_instances_modification_id: Some(modification_id.clone()),
            status: Some(RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING.to_string()),
            // "The reason for the status": a request that is still being worked on has
            // none to give.
            status_message: None,
        };
        self.add_stored_bytes(payload_bytes);
        if let Some(token) = spec.client_token {
            self.reserved_instances_modification_client_tokens
                .insert(token, modification_id.clone());
        }
        self.reserved_instances_modifications.insert(
            modification_id.clone(),
            ReservedInstancesModificationRecord {
                view,
                pending: Some(ClockTransition::starting_now(
                    RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED,
                )),
                region: spec.region,
            },
        );
        modification_id
    }

    /// Carry out every modification request whose fulfilment the clock has made due.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// may call: the work was owed at the instant the request's
    /// [`ClockTransition::due_at`] named, whether or not anybody asked again.
    ///
    /// Fulfilling one request mints the Reserved Instances its target configurations
    /// describe — "the Reserved Instances created as part of the modification request",
    /// whose id "is only available when the status of the modification is `fulfilled`"
    /// (`DescribeReservedInstancesModificationsRequest$Filters`) — and leaves the ones the
    /// request named [`RESERVED_INSTANCE_STATE_RETIRED`], since the account holds the new
    /// purchases in their place. Each new purchase keeps the term of the purchase it
    /// replaces (its start, its duration and so its end, its offering and its prices), the
    /// target configuration deciding only what the modification can change: the instance
    /// type, the Availability Zone and so the scope, and the count.
    ///
    /// A request the store has no room for the new purchases of, one whose named purchases
    /// the store no longer holds, and one whose results carry no configuration to read,
    /// each reach [`RESERVED_INSTANCES_MODIFICATION_STATUS_FAILED`] with their own reason in
    /// `StatusMessage`, and mint nothing: a modification is all of its target
    /// configurations or none of them.
    pub fn fulfill_due_reserved_instances_modifications(&mut self, now: Instant) {
        let due: Vec<String> = self
            .reserved_instances_modifications
            .iter()
            .filter(|(_, record)| {
                record
                    .pending
                    .as_ref()
                    .is_some_and(|pending| now >= pending.due_at)
            })
            .map(|(modification_id, _)| modification_id.clone())
            .collect();
        for modification_id in due {
            // The record is taken OUT of the store while the new purchases are minted,
            // because minting one needs the whole state; it goes back, fulfilled, below.
            let Some(mut record) = self.reserved_instances_modifications.remove(&modification_id)
            else {
                continue;
            };
            // The move is applied once: taking it off the record is what stops a later call
            // fulfilling the same request again, and its wall-clock instant is when the
            // fulfilment fell due.
            let due_at = record
                .pending
                .take()
                .map_or_else(SystemTime::now, |pending| pending.at);
            let mut results = record.view.modification_results.take().unwrap_or_default();
            let modified: Vec<String> = record
                .view
                .reserved_instances_ids
                .iter()
                .flatten()
                .filter_map(|named| named.reserved_instances_id.clone())
                .collect();
            // The purchases the request named are identical except for the members a
            // modification may change, so the first of them carries the term every new
            // purchase keeps.
            let template = modified
                .iter()
                .find_map(|reserved_instances_id| self.reserved_instances.get(reserved_instances_id))
                .cloned();
            // The purchase each target configuration asks for, or the reason the fulfilment
            // cannot go ahead — each reason reported as itself, so a `failed` modification
            // says what actually blocked it: the purchases it named are no longer held, one
            // of its results carries no configuration to read, or the store has no room for
            // the purchases it would mint. A modification is all of its target
            // configurations or none of them, so nothing is minted on any of those paths.
            let planned: Result<Vec<ReservedInstancesSpec>, String> = match template {
                None => Err("The Reserved Instances this modification named are no longer \
                             held by this account"
                    .to_string()),
                Some(_) if !self.has_reserved_instances_capacity(results.len()) => {
                    Err(format!(
                        "This emulator holds {MAX_STORED_RESERVED_INSTANCES} Reserved \
                         Instances, which is as many as it stores"
                    ))
                }
                Some(template) => results
                    .iter()
                    .map(|result| {
                        result
                            .target_configuration
                            .as_ref()
                            .map(|target| {
                                reserved_instances_spec_for_target(
                                    &template,
                                    target,
                                    &record.region,
                                )
                            })
                            // Unreachable: every stored result is written FROM a target
                            // configuration by `insert_reserved_instances_modification`.
                            // Reported rather than skipped, so no result of a `fulfilled`
                            // modification can be left without the purchase it describes.
                            .ok_or_else(|| {
                                "A target configuration of this modification could not be \
                                 read"
                                    .to_string()
                            })
                    })
                    .collect(),
            };
            // A fulfilment is all of its target configurations or none of them, so a batch
            // that cannot be minted whole is rolled back and the modification reports the
            // reason: the store is left exactly as this call found it, and no result of a
            // `fulfilled` modification is left without the purchase it describes.
            let outcome = match planned {
                Err(reason) => Err(reason),
                Ok(specs) => {
                    let wanted = specs.len();
                    let mut minted: Vec<String> = Vec::with_capacity(wanted);
                    for spec in specs {
                        match self.insert_reserved_instances(spec) {
                            Some(new_id) => minted.push(new_id),
                            // Unreachable: the capacity check above covered the whole batch.
                            None => break,
                        }
                    }
                    if minted.len() == wanted {
                        Ok(minted)
                    } else {
                        for new_id in &minted {
                            self.reserved_instances.remove(new_id);
                        }
                        Err(format!(
                            "This emulator holds {MAX_STORED_RESERVED_INSTANCES} Reserved \
                             Instances, which is as many as it stores"
                        ))
                    }
                }
            };
            match outcome {
                Ok(minted) => {
                    for (result, new_id) in results.iter_mut().zip(minted) {
                        result.reserved_instances_id = Some(new_id);
                    }
                    for reserved_instances_id in &modified {
                        if let Some(replaced) = self.reserved_instances.get_mut(reserved_instances_id)
                        {
                            replaced.state = Some(RESERVED_INSTANCE_STATE_RETIRED.to_string());
                        }
                    }
                    record.view.status =
                        Some(RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED.to_string());
                }
                Err(reason) => {
                    record.view.status =
                        Some(RESERVED_INSTANCES_MODIFICATION_STATUS_FAILED.to_string());
                    record.view.status_message = Some(reason);
                }
            }
            // "The time when the modification request was last updated"
            // (`ReservedInstancesModification$UpdateDate`): the wall-clock instant the
            // fulfilment fell due, which `ClockTransition::at` recorded when the request was
            // made — not the instant somebody next asked about it.
            record.view.update_date = Some(system_time_to_date_time(due_at));
            record.view.modification_results = Some(results);
            self.reserved_instances_modifications
                .insert(modification_id, record);
        }
    }
}

/// The purchase one target configuration asks for, read against the purchase it replaces.
///
/// The term is the template's — its start, its duration and so its end, its offering class
/// and type, its product description and its prices — because a modification changes none
/// of them: "The Reserved Instances to be modified must be identical, except for
/// Availability Zone, network platform, and instance type" (`ModifyReservedInstances`). The
/// target configuration decides the instance type, the Availability Zone (and so the scope,
/// which [`ComAmazonawsEc2Amazonec2State::insert_reserved_instances`] derives from it) and
/// the count, falling back to the template's for a member it leaves out.
///
/// A configuration's `Platform` — "The network platform of the modified Reserved Instances"
/// — is kept ON the modification result, where a reader of the modification reports it back,
/// and leaves the new purchase's `ProductDescription` the one it replaces: that member names
/// the product a Reserved Instance was bought for, and no `RIProductDescription` member
/// distinguishes a network platform that this store would otherwise have to invent.
fn reserved_instances_spec_for_target(
    template: &ReservedInstances,
    target: &ReservedInstancesConfiguration,
    region: &str,
) -> ReservedInstancesSpec {
    // A configuration may name its zone by id instead of by name, so the name the purchase
    // is stored under is resolved from whichever the request gave.
    let availability_zone = target.availability_zone.clone().or_else(|| {
        target.availability_zone_id.as_deref().and_then(|zone_id| {
            availability_zones_in_region(region)
                .into_iter()
                .find(|known| known.zone_id == zone_id)
                .map(|known| known.zone_name)
        })
    });
    ReservedInstancesSpec {
        instance_type: target
            .instance_type
            .clone()
            .or_else(|| template.instance_type.clone())
            .unwrap_or_default(),
        availability_zone,
        region: region.to_string(),
        instance_count: target
            .instance_count
            .or(template.instance_count)
            .unwrap_or_default(),
        duration_seconds: template.duration.unwrap_or_default(),
        offering_class: template.offering_class.clone().unwrap_or_default(),
        offering_type: template.offering_type.clone().unwrap_or_default(),
        product_description: template.product_description.clone().unwrap_or_default(),
        // The new purchase takes the place of one that is in effect, so it is in effect
        // too.
        state: RESERVED_INSTANCE_STATE_ACTIVE.to_string(),
        start: template
            .start
            .map_or_else(SystemTime::now, date_time_to_system_time),
        fixed_price: template.fixed_price.unwrap_or_default(),
        usage_price: template.usage_price.unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Exchanging Convertible purchases for other offerings
// ---------------------------------------------------------------------------

/// The offering class of a Reserved Instance that may be exchanged for another: the
/// `OfferingClassType` enum's `convertible` member
/// (`smithy_query(query="shape",shape="OfferingClassType")`).
///
/// An exchange reads it on BOTH sides, because both sides of the sentence name it: "the IDs
/// of the Convertible Reserved Instances to exchange for another Convertible Reserved
/// Instance of the same or higher value"
/// (`AcceptReservedInstancesExchangeQuoteRequest$ReservedInstanceIds`).
pub const RESERVED_INSTANCE_OFFERING_CLASS_CONVERTIBLE: &str = "convertible";

/// `InvalidReservedInstancesID.NotFound` (HTTP 400): EC2's code for a reserved-instances id
/// that resolves to no purchase of this account, whichever operation read it.
///
/// ONE code for both halves of that — a value that is not a Reserved Instances id at all, and
/// a well-formed id the account holds nothing under — because that is the code the recorded
/// refusal carries for a value of the FIRST kind, which only a code covering both can do; the
/// two are told apart by the message. It is deliberately not the
/// [`invalid_reserved_instances_id`] of this module's other operations: that one is what
/// `DeleteQueuedReservedInstances` was recorded answering for its own `reservedInstancesId`
/// member, and `DescribeReservedInstances` words its own member's refusal the way this one
/// does. The EXCHANGE API carries this same code for a value outside the id form
/// ([`reserved_instances_id_malformed`]) and its own spelling for a well-formed id it cannot
/// resolve ([`INVALID_RESERVED_INSTANCE_ID`]).
pub const INVALID_RESERVED_INSTANCES_ID_NOT_FOUND: &str = "InvalidReservedInstancesID.NotFound";

/// `InvalidReservedInstanceId` (HTTP 400): EC2's code for a WELL-FORMED `reservedInstanceId`
/// the EXCHANGE API cannot resolve to a Reserved Instance of this account.
///
/// The exchange API's own spelling of that refusal, which is neither
/// [`INVALID_RESERVED_INSTANCES_ID_NOT_FOUND`] nor the `InvalidParameterValue` of
/// `DeleteQueuedReservedInstances`: EC2's per-operation validators do not word the Reserved
/// Instances id refusal the same way, and the recorded contract of
/// `GetReservedInstancesExchangeQuote` and `AcceptReservedInstancesExchangeQuote` carries
/// this one — which is why both of them answer it through
/// [`invalid_reserved_instance_id`] rather than each keeping a spelling of its own. A value
/// outside the id form is answered [`INVALID_RESERVED_INSTANCES_ID_NOT_FOUND`] instead, which
/// is what those same recordings show for a value of that kind.
pub const INVALID_RESERVED_INSTANCE_ID: &str = "InvalidReservedInstanceId";

/// Refuse a WELL-FORMED `reservedInstanceId` the exchange API cannot resolve to a Reserved
/// Instance of this account: HTTP 400 [`INVALID_RESERVED_INSTANCE_ID`], in the SENTENCE the
/// recorded refusals of the two exchange operations carry — a space, then the id the request
/// named in square brackets, and nothing else.
///
/// That sentence has almost no fixed text, which is unusual for an EC2 message and is why it
/// is worded in one place: it is what the recorded refusals show. Two independent recorded
/// units carry it — the quote of a stale exchange
/// (`ec2.accept-reserved-instances-exchange-quote-stale-quote-rejected`, whose recorded
/// message is ` [<the reservedInstanceId the request sent>]`) and the quote operation's own
/// contract unit (`ec2.op-contract-get-reserved-instances-exchange-quote`, the same sentence
/// around its own id) — and the second of those also pins that the ACCEPT answers the very
/// same code and message for the same id
/// (`ec2.op-contract-acceptReservedInstancesExchangeQuote-b`). The only part built from the
/// request is the id, so nothing of one recording's values is written here.
///
/// This is the refusal of a value that IS a Reserved Instances id
/// ([`reserved_instances_id_is_well_formed`]) and names no purchase of this account: a value
/// outside that form is not an id to look up at all, and is answered
/// [`reserved_instances_id_malformed`] BEFORE any lookup, the order and the pair of answers
/// every id member of this service carries. Only the first
/// [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters of the value are named
/// ([`echoed_resource_id`]).
pub fn invalid_reserved_instance_id<E>(reserved_instance_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_RESERVED_INSTANCE_ID,
        400,
        format!(" [{id}]", id = echoed_resource_id(reserved_instance_id)),
    )
}

/// The resource type a Reserved Instances purchase is named by, as the tag API spells it
/// (`ResourceType`'s `reserved-instances` member) — and as every refusal that names the
/// RESOURCE rather than a member spells it.
pub const RESOURCE_TYPE_RESERVED_INSTANCES: &str = "reserved-instances";

/// Refuse a `reservedInstanceId` that is not a Reserved Instances id at all: HTTP 400
/// [`INVALID_RESERVED_INSTANCES_ID_NOT_FOUND`], in the sentence EC2 words a malformed id of
/// this resource with — the fixed text `The <resource type> ID '` … `' is malformed`, around
/// the value the REQUEST sent ([`echoed_resource_id`] bounds the echo).
///
/// A value outside the id form names nothing in ANY account, so it is refused before there is
/// anything to look up — which is what tells it apart from a well-formed id this account
/// holds no purchase under ([`invalid_reserved_instance_id`]), the other half of the pair.
pub fn reserved_instances_id_malformed<E>(reserved_instances_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
        400,
        format!(
            "The {RESOURCE_TYPE_RESERVED_INSTANCES} ID '{id}' is malformed",
            id = echoed_resource_id(reserved_instances_id)
        ),
    )
}

/// `InvalidReservedInstancesOfferingId` (HTTP 400): the `offeringId` a target configuration
/// named is not an offering this service publishes — "the specified Reserved Instances
/// offering does not exist", as EC2's error reference puts it.
pub const INVALID_RESERVED_INSTANCES_OFFERING_ID: &str = "InvalidReservedInstancesOfferingId";

/// Refuse a well-formed `reservedInstanceId` the account holds no Reserved Instance under:
/// HTTP 400 [`INVALID_RESERVED_INSTANCES_ID_NOT_FOUND`], the other half of that code.
pub fn reserved_instances_id_does_not_exist<E>(reserved_instance_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_RESERVED_INSTANCES_ID_NOT_FOUND,
        400,
        format!(
            "The {RESOURCE_TYPE_RESERVED_INSTANCES} ID '{id}' does not exist",
            id = echoed_resource_id(reserved_instance_id)
        ),
    )
}

/// Refuse a `reservedInstanceId` whose Reserved Instance is not in effect: HTTP 400
/// `InvalidParameterValue`, naming the member, the id and the state the store holds it in.
///
/// Only a purchase that is in effect has a term left to carry onto another offering — a
/// queued one has not been made yet, a retired one is over, and one already exchanged is
/// retired by the exchange that replaced it — so the state is what the caller has to act on,
/// and it comes from this store rather than from the request.
pub fn reserved_instances_not_exchangeable<E>(
    reserved_instance_id: &str,
    state: &str,
) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Invalid value '{id}' for reservedInstanceId. The Reserved Instance is in the \
         '{state}' state",
        id = echoed_resource_id(reserved_instance_id)
    ))
}

/// Refuse a `reservedInstanceId` whose Reserved Instance is not Convertible: HTTP 400
/// `InvalidParameterValue`. Only a Convertible Reserved Instance can be exchanged
/// ([`RESERVED_INSTANCE_OFFERING_CLASS_CONVERTIBLE`]).
pub fn reserved_instances_not_convertible<E>(reserved_instance_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Invalid value '{id}' for reservedInstanceId. Only Convertible Reserved Instances \
         can be exchanged",
        id = echoed_resource_id(reserved_instance_id)
    ))
}

/// Refuse an `offeringId` that is not an offering this service publishes: HTTP 400
/// [`INVALID_RESERVED_INSTANCES_OFFERING_ID`].
///
/// Every offering id carries its own terms, this service's marker and a checksum over both
/// ([`encode_reserved_instances_offering_id`]), so a value
/// [`decode_reserved_instances_offering_id`] cannot read is not an id this service ever
/// reported an offering under: there is no offering for it to name, which is what "does not
/// exist" says. There is no second, malformed half to tell it from — an offering is a search
/// result rather than a stored resource, so an id that decodes names the offering it carries
/// and one that does not names nothing.
pub fn invalid_reserved_instances_offering_id<E>(offering_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_RESERVED_INSTANCES_OFFERING_ID,
        400,
        format!(
            "The Reserved Instances offering ID '{id}' does not exist",
            id = echoed_resource_id(offering_id)
        ),
    )
}

/// The id prefix a Reserved Instances offering id carries, in the `<prefix>-<payload>` form
/// this service's derived offering ids take ([`CAPACITY_BLOCK_OFFERING_ID_PREFIX`] is the
/// same construction for a Capacity Block offering). The model puts no pattern on
/// `ReservedInstancesOfferingId` (a plain `String`), so the form is decided in
/// [`encode_reserved_instances_offering_id`], the one place such an id is minted.
pub const RESERVED_INSTANCES_OFFERING_ID_PREFIX: &str = "rio";

/// The marker an offering id carries ahead of its checksum and terms, so an id a caller
/// assembled is not read as one this service minted, and an id of another derived kind is not
/// read as this one ([`encode_derived_id`]).
const RESERVED_INSTANCES_OFFERING_ID_VERSION: &str = "ec2-rio-1";

/// How many fields a Reserved Instances offering id's payload carries.
const RESERVED_INSTANCES_OFFERING_FIELDS: usize = 8;

/// What one Reserved Instances offering offers: the terms a purchase of it, or an exchange
/// into it, turns into a [`ReservedInstancesSpec`].
///
/// An offering is NOT a stored resource, for the reason the Capacity Block offering of
/// [`CapacityBlockOfferingTerms`] is not: nothing in the EC2 API creates or withdraws one, so
/// the offerings of a Region are facts of that Region and its published catalogs rather than
/// rows a caller wrote. The terms therefore travel INSIDE the offering id
/// ([`encode_reserved_instances_offering_id`]) instead of in a map keyed by it — which is
/// what lets a search report offerings without writing anything a later caller could see, and
/// lets an exchange mint a purchase of exactly the terms the offering was reported with.
#[derive(Debug, Clone, PartialEq)]
pub struct ReservedInstancesOfferingTerms {
    /// "The instance type on which the Reserved Instance can be used"
    /// (`ReservedInstancesOffering$InstanceType`): one of `INSTANCE_TYPE_VALUES`.
    pub instance_type: String,
    /// "The Availability Zone in which the Reserved Instance can be used" for an offering of
    /// zonal scope; `None` for one of Regional scope.
    pub availability_zone: Option<String>,
    /// "The Reserved Instance product platform description"
    /// (`ReservedInstancesOffering$ProductDescription`): one of
    /// `R_I_PRODUCT_DESCRIPTION_VALUES`.
    pub product_description: String,
    /// "If `convertible` it can be exchanged for Reserved Instances of the same or higher
    /// monetary value" (`ReservedInstancesOffering$OfferingClass`): one of
    /// `OFFERING_CLASS_TYPE_VALUES`.
    pub offering_class: String,
    /// "The Reserved Instance offering type" (`ReservedInstancesOffering$OfferingType`): one
    /// of `OFFERING_TYPE_VALUES_VALUES`.
    pub offering_type: String,
    /// "The duration of the Reserved Instance, in seconds"
    /// (`ReservedInstancesOffering$Duration`).
    pub duration_seconds: i64,
    /// "The purchase price of the Reserved Instance"
    /// (`ReservedInstancesOffering$FixedPrice`), per reservation.
    pub fixed_price: f32,
    /// "The usage price of the Reserved Instance, per hour"
    /// (`ReservedInstancesOffering$UsagePrice`).
    pub usage_price: f32,
}

/// The `ReservedInstancesOfferingId` for `terms`: `rio-<hex>` over the terms, this service's
/// offering-id marker and a checksum ([`encode_derived_id`] is that construction, shared with
/// every other derived id this service reports).
///
/// Deriving the id FROM the terms is what makes it stable — two searches that find the same
/// offering report the same id, so a read changes nothing a later caller sees — and
/// resolvable, since [`decode_reserved_instances_offering_id`] reads the terms back out
/// without a store.
pub fn encode_reserved_instances_offering_id(terms: &ReservedInstancesOfferingTerms) -> String {
    encode_derived_id(
        RESERVED_INSTANCES_OFFERING_ID_PREFIX,
        RESERVED_INSTANCES_OFFERING_ID_VERSION,
        &reserved_instances_offering_payload(terms),
    )
}

/// The terms an id minted by [`encode_reserved_instances_offering_id`] carries; `None` for
/// anything else — an id that does not carry this service's prefix or marker, is not hex,
/// whose checksum does not match its terms, or whose terms are not the fields an offering
/// has.
pub fn decode_reserved_instances_offering_id(
    offering_id: &str,
) -> Option<ReservedInstancesOfferingTerms> {
    let fields = decode_derived_id(
        RESERVED_INSTANCES_OFFERING_ID_PREFIX,
        RESERVED_INSTANCES_OFFERING_ID_VERSION,
        offering_id,
        RESERVED_INSTANCES_OFFERING_FIELDS,
    )?;
    Some(ReservedInstancesOfferingTerms {
        duration_seconds: fields[0].parse().ok()?,
        fixed_price: fields[1].parse().ok()?,
        usage_price: fields[2].parse().ok()?,
        instance_type: hex_field(&fields[3])?,
        availability_zone: optional_hex_field(&fields[4])?,
        product_description: hex_field(&fields[5])?,
        offering_class: hex_field(&fields[6])?,
        offering_type: hex_field(&fields[7])?,
    })
}

/// The terms of one offering as the fields its id carries them as: the term and the prices as
/// decimal, the text through the shared field writer.
fn reserved_instances_offering_payload(terms: &ReservedInstancesOfferingTerms) -> Vec<String> {
    vec![
        terms.duration_seconds.to_string(),
        terms.fixed_price.to_string(),
        terms.usage_price.to_string(),
        derived_id_text_field(Some(terms.instance_type.as_str())),
        derived_id_text_field(terms.availability_zone.as_deref()),
        derived_id_text_field(Some(terms.product_description.as_str())),
        derived_id_text_field(Some(terms.offering_class.as_str())),
        derived_id_text_field(Some(terms.offering_type.as_str())),
    ]
}

/// The purchase an exchange into `terms` mints, holding `instance_count` reservations.
///
/// Everything about the new Reserved Instance comes from the OFFERING — its instance type,
/// its Availability Zone and so its scope, its product description, its offering class and
/// type, its term and its prices — because an exchange replaces a purchase with one of
/// another offering rather than editing it; only how many reservations it holds is carried
/// over from the purchases being exchanged. Its term starts when the exchange is accepted,
/// and it is in effect at once ([`RESERVED_INSTANCE_STATE_ACTIVE`]): the exchanged purchases
/// are gone from that instant, so what replaces them cannot be pending.
pub fn reserved_instances_spec_for_offering(
    terms: &ReservedInstancesOfferingTerms,
    region: &str,
    instance_count: i32,
    accepted_at: SystemTime,
) -> ReservedInstancesSpec {
    ReservedInstancesSpec {
        instance_type: terms.instance_type.clone(),
        availability_zone: terms.availability_zone.clone(),
        region: region.to_string(),
        instance_count,
        duration_seconds: terms.duration_seconds,
        offering_class: terms.offering_class.clone(),
        offering_type: terms.offering_type.clone(),
        product_description: terms.product_description.clone(),
        state: RESERVED_INSTANCE_STATE_ACTIVE.to_string(),
        start: accepted_at,
        fixed_price: terms.fixed_price,
        usage_price: terms.usage_price,
    }
}

/// How the reservations the exchanged purchases hold are divided over the target
/// configurations of ONE exchange: one count per configuration, in the order the request gave
/// them.
///
/// `TargetConfigurationRequest$InstanceCount` "is reserved and cannot be specified in a
/// request", so the caller never says how many reservations a target configuration takes and
/// the service divides them — equally, the remainder going to the earlier configurations,
/// which is the one division that keeps the total exactly and depends on nothing but how many
/// configurations there are.
///
/// The ONE definition of that division, because `GetReservedInstancesExchangeQuote` quotes the
/// configurations `AcceptReservedInstancesExchangeQuote` would then mint: a quote and the
/// exchange it describes cannot divide one purchase's reservations two ways. A request
/// carrying more configurations than there are reservations is refused by each of those
/// operations before this is reached, so no count it answers is zero.
pub fn reserved_instances_exchange_shares(total: i64, configurations: usize) -> Vec<i32> {
    let configurations = i64::try_from(configurations).unwrap_or(i64::MAX).max(1);
    let share = total / configurations;
    let remainder = total % configurations;
    (0..configurations)
        .map(|position| {
            let count = share.saturating_add(i64::from(position < remainder));
            i32::try_from(count).unwrap_or(i32::MAX)
        })
        .collect()
}

/// What one Reserved Instance, or one side of a whole exchange, is WORTH: the purchase price
/// of each reservation times how many of them there are.
///
/// The ONE definition of that number, because an exchange is only allowed "for another
/// Convertible Reserved Instance of the same or higher value"
/// (`AcceptReservedInstancesExchangeQuoteRequest$ReservedInstanceIds`) and the two sides of
/// that comparison have to be measured the same way. It is the upfront value —
/// `FixedPrice` is "the purchase price of the Reserved Instance" and `UsagePrice` is charged
/// per hour as the reservation is used — so the comparison is of what the account has already
/// paid for against what it is being given.
pub fn reserved_instances_value(fixed_price: f32, instance_count: i32) -> f64 {
    f64::from(fixed_price) * f64::from(instance_count)
}

/// How close two Reserved Instances values count as the SAME value: one cent, the smallest
/// amount [`RESERVED_INSTANCE_CURRENCY_CODE`] is quoted in.
///
/// An exchange is allowed "for another Convertible Reserved Instance of the same or higher
/// value", so one of equal value must not be refused because a sum of the single-precision
/// prices the modelled shapes carry rounded a fraction of a cent the wrong way.
pub const RESERVED_INSTANCES_VALUE_TOLERANCE: f64 = 0.01;

/// One Reserved Instances value as a refusal names it: the amount to the cent, with the
/// currency it is quoted in ([`RESERVED_INSTANCE_CURRENCY_CODE`]).
pub fn reserved_instances_value_text(value: f64) -> String {
    format!("{value:.2} {RESERVED_INSTANCE_CURRENCY_CODE}")
}

impl ComAmazonawsEc2Amazonec2State {
    /// A fresh, unused `ExchangeId`: "the ID of the successful exchange"
    /// (`AcceptReservedInstancesExchangeQuoteResult$ExchangeId`).
    ///
    /// A UUID, laid out by the same counter and layout as every other id of the Reserved
    /// Instances domain ([`Self::mint_reserved_instances_id`]), so it is unique within a run,
    /// the same across runs, and never taken from the request. The model pins no pattern on
    /// `ExchangeId` (a plain `String`), so the form is decided here, in the one place one is
    /// minted.
    pub fn mint_reserved_instances_exchange_id(&mut self) -> String {
        self.mint_uuid()
    }
}

// ---------------------------------------------------------------------------
// The Marketplace listings those purchases are offered for sale through
// ---------------------------------------------------------------------------

/// "active" — the status of a listing that is on sale: the `ListingStatus` enum's `active`
/// member (`smithy_query(query="shape",shape="ListingStatus")`), and the status a listing
/// starts in, since `CreateReservedInstancesListing` answers the listing it has just put up
/// for sale rather than one waiting to be put up.
pub const RESERVED_INSTANCES_LISTING_STATUS_ACTIVE: &str = "active";

/// "cancelled" — the status of a listing the seller has withdrawn: the `ListingStatus`
/// enum's `cancelled` member, which is where `CancelReservedInstancesListing` leaves one.
pub const RESERVED_INSTANCES_LISTING_STATUS_CANCELLED: &str = "cancelled";

/// "available" — listed reservations that are still for sale: the `ListingState` enum's
/// `available` member (`smithy_query(query="shape",shape="ListingState")`), which is the
/// state every reservation a new listing offers is in.
pub const RESERVED_INSTANCES_LISTING_STATE_AVAILABLE: &str = "available";

/// "sold" — listed reservations a buyer has bought: the `ListingState` enum's `sold`
/// member. Nothing in this API buys a listing, so no reservation of this store reaches it;
/// the count is reported because a listing reports one per member of the catalog.
pub const RESERVED_INSTANCES_LISTING_STATE_SOLD: &str = "sold";

/// "cancelled" — listed reservations the seller has withdrawn: the `ListingState` enum's
/// `cancelled` member, where cancelling a listing leaves the ones that were available.
pub const RESERVED_INSTANCES_LISTING_STATE_CANCELLED: &str = "cancelled";

/// "pending" — listed reservations whose sale is in flight: the `ListingState` enum's
/// `pending` member. Nothing in this API bids on a listing, so no reservation of this store
/// reaches it either.
pub const RESERVED_INSTANCES_LISTING_STATE_PENDING: &str = "pending";

/// How many Marketplace listings this emulator stores before one is refused.
///
/// CHOSEN, for the reason [`MAX_STORED_RESERVED_INSTANCES_MODIFICATIONS`] gives: nothing
/// here reclaims a listing — a cancelled one is still reportable, because `cancelled` is a
/// member of the modelled `ListingStatus` enum — so without a count ceiling repeated calls
/// would grow this process's memory without bound.
pub const MAX_STORED_RESERVED_INSTANCES_LISTINGS: usize = MAX_STORED_RECORDS_PER_STORE;

/// How many price schedules one listing may carry.
///
/// CHOSEN: the model declares no `@length` on `PriceScheduleSpecificationList`, and the
/// record keeps every entry for as long as the listing lives, so the handler is the only
/// place the size of that list can be bounded. A schedule prices "each month remaining in
/// the Reserved Instance term" (`CreateReservedInstancesListingRequest$PriceSchedules`) and
/// the longest term AWS sells is three years, so this is well above the number of months a
/// listing can usefully price — at the scale of the comparable ceiling
/// [`MAX_RESERVED_INSTANCES_TARGET_CONFIGURATIONS`] puts on a modification's list.
pub const MAX_RESERVED_INSTANCES_PRICE_SCHEDULES: usize = 100;

/// The seconds in one of the months a price schedule's `Term` counts: a twelfth of the
/// 365-day year a one-year Reserved Instance term is `Duration` seconds of
/// ("the duration of the Reserved Instance, in seconds" — `ReservedInstances$Duration`), so
/// the months a term is read in and the seconds a term is stored in are the same year.
///
/// A month of a term is therefore the same length wherever in the calendar it falls, which
/// leaves up to a day of slack against a calendar month at a month boundary. That is the
/// right trade here: the term this is measured against is itself held as a number of seconds
/// rather than as a count of calendar months.
const RESERVED_INSTANCES_MONTH_SECONDS: u64 = 365 * 24 * 60 * 60 / 12;

/// The `ReservedInstancesListingId` member, as the Reserved Instance Marketplace's own
/// validator spells it in a refusal: `listingId`, the name EC2 was recorded answering for a
/// value it could not resolve to a listing.
pub const LISTING_ID_MEMBER: &str = "listingId";

/// The `ReservedInstancesId` member of the Marketplace operations, as a refusal names it.
pub const LISTING_RESERVED_INSTANCES_ID_MEMBER: &str = "reservedInstancesId";

/// The `InstanceCount` member of `CreateReservedInstancesListing`, as a refusal names it.
pub const LISTING_INSTANCE_COUNT_MEMBER: &str = "instanceCount";

/// Refuse a Marketplace request member whose value this service cannot act on: HTTP 400
/// [`INVALID_INPUT`], `Invalid value for '<member>'` and nothing after it.
///
/// That is the whole sentence EC2 answers when the value names nothing it can act on and
/// there is no further reason to give — an id that resolves to no listing of this account
/// has nothing to say about itself beyond being unusable. When the service CAN say what was
/// wrong with a value it read, [`invalid_marketplace_value_because`] puts that reason after
/// the same opening.
pub fn invalid_marketplace_value<E>(member: &str) -> Scaffolded<E> {
    invalid_input(format!("Invalid value for '{member}'"))
}

/// Refuse a Marketplace request member the service read but cannot act on: the same HTTP
/// 400 [`INVALID_INPUT`] opening as [`invalid_marketplace_value`], with the reason after it
/// — the `. <reason>` half every other [`INVALID_INPUT`] refusal of this module carries
/// ([`reserved_instances_not_found`]).
pub fn invalid_marketplace_value_because<E>(member: &str, reason: impl AsRef<str>) -> Scaffolded<E> {
    invalid_input(format!(
        "Invalid value for '{member}'. {reason}",
        reason = reason.as_ref()
    ))
}

/// One stored Marketplace listing: the modelled [`ReservedInstancesListing`] every reader
/// of it reports, beside the one fact that shape carries no member for — when the term of
/// the Reserved Instance it sells ends, which is what the ACTIVE price schedule is read
/// against ([`reserved_instances_listing_price_schedules`]).
///
/// The view is the modelled shape itself rather than a parallel record, for the reason
/// [`ReservedInstancesModificationRecord`] keeps its own that way: every member a writer
/// sets round-trips through a reader without a field-by-field copy that can silently drop
/// one. Its price schedules carry NO `Active` flag, because which schedule is in effect is
/// a fact of the clock rather than of the listing: a reader derives it.
#[derive(Debug, Clone)]
pub struct ReservedInstancesListingRecord {
    /// The listing as the model shapes it.
    pub view: ReservedInstancesListing,
    /// When the term of the Reserved Instance this listing sells ends
    /// (`ReservedInstances$End`), held as the instant the months remaining are measured
    /// back from.
    pub term_end: SystemTime,
}

/// What ONE listing is stored with, beyond everything the service derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_reserved_instances_listing`] mints the id,
/// stamps the dates, sets the status and lays out the instance counts).
#[derive(Debug, Clone)]
pub struct ReservedInstancesListingSpec {
    /// "The ID of the active Standard Reserved Instance"
    /// (`CreateReservedInstancesListingRequest$ReservedInstancesId`), already resolved
    /// against the store by the caller.
    pub reserved_instances_id: String,
    /// "The number of instances that are a part of a Reserved Instance account to be listed
    /// in the Reserved Instance Marketplace"
    /// (`CreateReservedInstancesListingRequest$InstanceCount`).
    pub instance_count: i32,
    /// "A list specifying the price of the Standard Reserved Instance for each month
    /// remaining in the Reserved Instance term"
    /// (`CreateReservedInstancesListingRequest$PriceSchedules`), as the request gave them.
    pub price_schedules: Vec<PriceScheduleSpecification>,
    /// "A unique, case-sensitive key supplied by the client to ensure that the request is
    /// idempotent" (`CreateReservedInstancesListingRequest$ClientToken`), which this
    /// operation requires.
    pub client_token: String,
    /// When the term of the Reserved Instance being listed ends.
    pub term_end: SystemTime,
    /// When the listing was created: its create and update dates.
    pub created_at: SystemTime,
}

/// The bytes one stored listing will hold: the `ClientToken`, the Reserved Instances id and
/// the currency each price schedule is stored with — the one the request named or the default
/// the store fills in, since the record keeps that text either way — on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. The id, the dates, the status
/// and the instance counts are derived, and a schedule's term and price are fixed-size
/// numbers whose COUNT is bounded by [`MAX_RESERVED_INSTANCES_PRICE_SCHEDULES`], so the
/// stored TEXT is what is charged here.
///
/// The ONE definition of that charge, the way
/// [`reserved_instances_modification_payload_bytes`] is for a modification: the handler asks
/// it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_reserved_instances_listing`] charges exactly it.
pub fn reserved_instances_listing_payload_bytes(spec: &ReservedInstancesListingSpec) -> usize {
    spec.price_schedules
        .iter()
        .map(|schedule| {
            schedule
                .currency_code
                .as_deref()
                .unwrap_or(RESERVED_INSTANCE_CURRENCY_CODE)
                .len()
        })
        .fold(STORED_RECORD_OVERHEAD_BYTES, usize::saturating_add)
        .saturating_add(spec.client_token.len())
        .saturating_add(spec.reserved_instances_id.len())
}

/// The `InstanceCounts` of a listing: one entry per member of the modelled `ListingState`
/// catalog, in the order that enum declares them, so two reports of one listing cannot
/// disagree about which states it accounts for.
///
/// "The number of instances in this state" (`ReservedInstancesListing$InstanceCounts`), so
/// every member of the catalog carries a number — a state none of the listing's
/// reservations are in carries zero rather than being left out.
pub fn reserved_instances_listing_counts(
    available: i32,
    sold: i32,
    cancelled: i32,
    pending: i32,
) -> Vec<InstanceCount> {
    [
        (RESERVED_INSTANCES_LISTING_STATE_AVAILABLE, available),
        (RESERVED_INSTANCES_LISTING_STATE_SOLD, sold),
        (RESERVED_INSTANCES_LISTING_STATE_CANCELLED, cancelled),
        (RESERVED_INSTANCES_LISTING_STATE_PENDING, pending),
    ]
    .into_iter()
    .map(|(state, instance_count)| InstanceCount {
        instance_count: Some(instance_count),
        state: Some(state.to_string()),
    })
    .collect()
}

/// How many of a listing's reservations are in `state`: the entry of its `InstanceCounts`
/// for that member of the `ListingState` catalog, and zero when it holds none.
pub fn reserved_instances_listing_count(counts: &[InstanceCount], state: &str) -> i32 {
    counts
        .iter()
        .find(|entry| entry.state.as_deref() == Some(state))
        .and_then(|entry| entry.instance_count)
        .unwrap_or(0)
}

/// How many reservations a listing LISTED, whatever state they are in now: the sum of its
/// `InstanceCounts`.
///
/// Withdrawing a listing moves reservations between states without changing that total, so
/// this is the `InstanceCount` the listing was created with, read back off the record rather
/// than stored a second time beside it.
pub fn reserved_instances_listing_listed_count(counts: &[InstanceCount]) -> i32 {
    counts
        .iter()
        .filter_map(|entry| entry.instance_count)
        .fold(0, i32::saturating_add)
}

/// How many months of a term that ends at `term_end` are left at `now`.
///
/// A price schedule's `Term` is "the number of months remaining in the reservation"
/// (`PriceSchedule$Term`), counted in whole months, so a part month still counts as one and
/// the answer is never below one: "a specific price schedule is always in effect"
/// (`PriceSchedule$Active`), which a remainder of zero months would leave nothing to be in
/// effect for.
fn months_remaining_in_term(term_end: SystemTime, now: SystemTime) -> i64 {
    let remaining = term_end.duration_since(now).unwrap_or_default().as_secs();
    i64::try_from(remaining.div_ceil(RESERVED_INSTANCES_MONTH_SECONDS))
        .unwrap_or(i64::MAX)
        .max(1)
}

/// Which `Term` of `terms` is in effect with `months_remaining` months left; `None` when the
/// listing prices no term at all.
///
/// "The current price schedule, as determined by the term remaining for the Reserved
/// Instance in the listing. A specific price schedule is always in effect, but only one
/// price schedule can be active at any time. Take, for example, a Reserved Instance listing
/// that has five months remaining in its term. When you specify price schedules for five
/// months and two months, this means that schedule 1, covering the first three months of the
/// remaining term, will be active during months 5, 4, and 3. Then schedule 2, covering the
/// last two months of the term, will be active for months 2 and 1" (`PriceSchedule$Active`).
///
/// So the schedule in effect is the one with the SMALLEST term that still covers what is
/// left — the five-month schedule covers months 5, 4 and 3 because the two-month one does
/// not reach them — and, when no schedule reaches that far, the one with the largest term,
/// since one is always in effect.
fn active_price_schedule_term(terms: &[i64], months_remaining: i64) -> Option<i64> {
    terms
        .iter()
        .copied()
        .filter(|term| *term >= months_remaining)
        .min()
        .or_else(|| terms.iter().copied().max())
}

/// The price schedules of `record` as a report of it carries them: the terms and prices the
/// listing was created with, with `Active` set on the one the term remaining puts in effect
/// ([`active_price_schedule_term`]) and cleared on every other.
///
/// Derived on every read rather than stored, because which schedule is in effect changes
/// with the CLOCK and not with the listing: two reads a month apart report the same listing
/// with a different schedule active, and neither read writes anything.
pub fn reserved_instances_listing_price_schedules(
    record: &ReservedInstancesListingRecord,
    now: SystemTime,
) -> Vec<PriceSchedule> {
    let stored: &[PriceSchedule] = record.view.price_schedules.as_deref().unwrap_or_default();
    let terms: Vec<i64> = stored.iter().filter_map(|schedule| schedule.term).collect();
    let active = active_price_schedule_term(&terms, months_remaining_in_term(record.term_end, now));
    stored
        .iter()
        .map(|schedule| PriceSchedule {
            active: Some(schedule.term.is_some() && schedule.term == active),
            ..schedule.clone()
        })
        .collect()
}

/// One stored listing as every reader of it reports it: what the record holds, with the
/// price schedule the term remaining puts in effect marked active.
pub fn reserved_instances_listing_view(
    record: &ReservedInstancesListingRecord,
    now: SystemTime,
) -> ReservedInstancesListing {
    ReservedInstancesListing {
        price_schedules: Some(reserved_instances_listing_price_schedules(record, now)),
        ..record.view.clone()
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another listing fits inside the store's ceiling
    /// ([`MAX_STORED_RESERVED_INSTANCES_LISTINGS`]).
    ///
    /// The same contract as [`Self::has_reserved_instances_modification_capacity`]: the
    /// handler asks this BEFORE storing anything and refuses instead of growing the store
    /// past its ceiling.
    pub fn has_reserved_instances_listing_capacity(&self) -> bool {
        self.reserved_instances_listings.len() < MAX_STORED_RESERVED_INSTANCES_LISTINGS
    }

    /// A fresh, unused `ReservedInstancesListingId`.
    ///
    /// A UUID, the form AWS's own listing ids read in, laid out by the same counter and
    /// layout as every other id of the Reserved Instances domain
    /// ([`Self::mint_reserved_instances_id`]), so it is unique within a run, the same across
    /// runs, and never taken from the request. The model pins no pattern on
    /// `ReservedInstancesListingId` (a plain `String`), so the form is decided here, in the
    /// one place one is minted.
    pub fn mint_reserved_instances_listing_id(&mut self) -> String {
        self.mint_uuid()
    }

    /// The listing id a `ClientToken` has already minted, if it has: "a unique,
    /// case-sensitive key supplied by the client to ensure that the request is idempotent.
    /// … This helps avoid duplicate listings"
    /// (`CreateReservedInstancesListingRequest$ClientToken`), so the repeat of a request
    /// answers with the listing the first one created instead of creating a second.
    pub fn reserved_instances_listing_for_token(&self, client_token: &str) -> Option<&str> {
        self.reserved_instances_listing_client_tokens
            .get(client_token)
            .map(String::as_str)
    }

    /// The id of the listing currently offering `reserved_instances_id` for sale, if one is.
    ///
    /// "You can submit one Standard Reserved Instance listing at a time"
    /// (`CreateReservedInstancesListing`), so a purchase that is already on sale cannot be
    /// listed a second time while that listing is active — a withdrawn or sold-out listing
    /// leaves it free to be listed again.
    pub fn active_reserved_instances_listing_of(&self, reserved_instances_id: &str) -> Option<&str> {
        self.reserved_instances_listings
            .values()
            .find(|record| {
                record.view.reserved_instances_id.as_deref() == Some(reserved_instances_id)
                    && record.view.status.as_deref()
                        == Some(RESERVED_INSTANCES_LISTING_STATUS_ACTIVE)
            })
            .and_then(|record| record.view.reserved_instances_listing_id.as_deref())
    }

    /// Put a Reserved Instance up for sale, and answer the `ReservedInstancesListingId` the
    /// listing was given, or `None` when the store is already at its ceiling
    /// ([`Self::has_reserved_instances_listing_capacity`]).
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller —
    /// the id, the create and update dates, the status a new listing starts in
    /// ([`RESERVED_INSTANCES_LISTING_STATUS_ACTIVE`]), the reason it reports for that status,
    /// and the instance counts, which start with every listed reservation available and none
    /// sold, cancelled or pending — so two readers cannot report one listing differently. The
    /// caller-chosen bytes the record retains
    /// ([`reserved_instances_listing_payload_bytes`]) are charged against the store's byte
    /// budget here, so a writer cannot store them uncharged.
    pub fn insert_reserved_instances_listing(
        &mut self,
        spec: ReservedInstancesListingSpec,
    ) -> Option<String> {
        if !self.has_reserved_instances_listing_capacity() {
            return None;
        }
        let payload_bytes = reserved_instances_listing_payload_bytes(&spec);
        let listing_id = self.mint_reserved_instances_listing_id();
        let created_at = system_time_to_date_time(spec.created_at);
        let view = ReservedInstancesListing {
            client_token: Some(spec.client_token.clone()),
            create_date: Some(created_at),
            instance_counts: Some(reserved_instances_listing_counts(spec.instance_count, 0, 0, 0)),
            price_schedules: Some(
                spec.price_schedules
                    .iter()
                    .map(|schedule| PriceSchedule {
                        // Which schedule is in effect is derived on every report
                        // ([`reserved_instances_listing_price_schedules`]), so the record
                        // keeps no answer of its own that a later read could contradict.
                        active: None,
                        currency_code: Some(
                            schedule
                                .currency_code
                                .clone()
                                .unwrap_or_else(|| RESERVED_INSTANCE_CURRENCY_CODE.to_string()),
                        ),
                        price: schedule.price,
                        term: schedule.term,
                    })
                    .collect(),
            ),
            reserved_instances_id: Some(spec.reserved_instances_id),
            reserved_instances_listing_id: Some(listing_id.clone()),
            status: Some(RESERVED_INSTANCES_LISTING_STATUS_ACTIVE.to_string()),
            // "The reason for the current status of the Reserved Instance listing"
            // (`ReservedInstancesListing$StatusMessage`): a listing that is simply on sale
            // reports that status as its own reason, the way AWS's own listing examples do.
            status_message: Some(RESERVED_INSTANCES_LISTING_STATUS_ACTIVE.to_string()),
            tags: Some(Vec::new()),
            update_date: Some(created_at),
        };
        self.add_stored_bytes(payload_bytes);
        self.reserved_instances_listing_client_tokens
            .insert(spec.client_token, listing_id.clone());
        self.reserved_instances_listings.insert(
            listing_id.clone(),
            ReservedInstancesListingRecord {
                view,
                term_end: spec.term_end,
            },
        );
        Some(listing_id)
    }

    /// Withdraw a listing from the Reserved Instance Marketplace at `at`, and answer it as a
    /// report of it carries it; `None` when the account holds no listing under `listing_id`.
    ///
    /// The reservations that were still for sale become
    /// [`RESERVED_INSTANCES_LISTING_STATE_CANCELLED`] and the listing's own status follows
    /// them to [`RESERVED_INSTANCES_LISTING_STATUS_CANCELLED`]: cancelling a listing
    /// withdraws what is still on offer, so the reservations a buyer has already taken are
    /// left where they are rather than un-sold. The record is kept rather than dropped —
    /// `cancelled` is a member of the modelled `ListingStatus` enum, so a describe still
    /// reports the withdrawn listing.
    pub fn cancel_reserved_instances_listing(
        &mut self,
        listing_id: &str,
        at: SystemTime,
    ) -> Option<ReservedInstancesListing> {
        let record = self.reserved_instances_listings.get_mut(listing_id)?;
        let counts: &[InstanceCount] = record.view.instance_counts.as_deref().unwrap_or_default();
        let available =
            reserved_instances_listing_count(counts, RESERVED_INSTANCES_LISTING_STATE_AVAILABLE);
        let sold = reserved_instances_listing_count(counts, RESERVED_INSTANCES_LISTING_STATE_SOLD);
        let cancelled =
            reserved_instances_listing_count(counts, RESERVED_INSTANCES_LISTING_STATE_CANCELLED);
        let pending =
            reserved_instances_listing_count(counts, RESERVED_INSTANCES_LISTING_STATE_PENDING);
        record.view.instance_counts = Some(reserved_instances_listing_counts(
            0,
            sold,
            cancelled.saturating_add(available),
            pending,
        ));
        record.view.status = Some(RESERVED_INSTANCES_LISTING_STATUS_CANCELLED.to_string());
        record.view.status_message =
            Some(RESERVED_INSTANCES_LISTING_STATUS_CANCELLED.to_string());
        // "The last modified timestamp of the listing"
        // (`ReservedInstancesListing$UpdateDate`): the instant this withdrawal was made.
        record.view.update_date = Some(system_time_to_date_time(at));
        Some(reserved_instances_listing_view(record, at))
    }
}
