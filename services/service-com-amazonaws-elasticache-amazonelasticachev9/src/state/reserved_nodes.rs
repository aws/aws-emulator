// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The reservation terms this service publishes offerings for: the year count
/// `Duration` takes as shorthand, and the term in seconds.
///
/// From `DescribeReservedCacheNodesOfferings$Duration`: "Duration filter value,
/// specified in years or seconds. Valid Values: 1 | 3 | 31536000 | 94608000",
/// so a one-year term is 31536000 seconds and a three-year term 94608000.
pub const OFFERING_TERMS: [(i32, i32); 2] = [(1, 31_536_000), (3, 94_608_000)];

/// The payment options ElastiCache sells reservations under today.
pub const OFFERING_TYPE_ALL_UPFRONT: &str = "All Upfront";
/// See [`OFFERING_TYPE_ALL_UPFRONT`].
pub const OFFERING_TYPE_PARTIAL_UPFRONT: &str = "Partial Upfront";
/// See [`OFFERING_TYPE_ALL_UPFRONT`].
pub const OFFERING_TYPE_NO_UPFRONT: &str = "No Upfront";

/// Every value `OfferingType` documents: "Valid Values: "Light
/// Utilization"|"Medium Utilization"|"Heavy Utilization"|"All Upfront"|"Partial
/// Upfront"|"No Upfront"" — the set a filter value is checked against.
pub const OFFERING_TYPES: [&str; 6] = [
    "Light Utilization",
    "Medium Utilization",
    "Heavy Utilization",
    OFFERING_TYPE_ALL_UPFRONT,
    OFFERING_TYPE_PARTIAL_UPFRONT,
    OFFERING_TYPE_NO_UPFRONT,
];

/// The offering types the catalog is STOCKED with: the three payment options
/// above. The three legacy utilization types are documented filter values all
/// the same, so a request naming one is served rather than refused, and matches
/// no offering — exactly as it does against a region that sells none.
pub const STOCKED_OFFERING_TYPES: [&str; 3] = [
    OFFERING_TYPE_ALL_UPFRONT,
    OFFERING_TYPE_PARTIAL_UPFRONT,
    OFFERING_TYPE_NO_UPFRONT,
];

/// The `RecurringChargeFrequency` of a reservation's recurring charge: the
/// hourly charge a partial- or no-upfront reservation is billed at.
pub const RECURRING_CHARGE_HOURLY: &str = "Hourly";

/// Seconds in an hour, which is what turns a term into the hours a fixed price
/// covers.
pub(super) const SECONDS_PER_HOUR: f64 = 3_600.0;

/// The share of a partial-upfront reservation's cost that is paid up front; the
/// rest is the recurring hourly charge.
pub(super) const PARTIAL_UPFRONT_SHARE: f64 = 0.5;

/// The cache node types the offering catalog covers, each with the hourly
/// on-demand rate its reservations are priced from.
///
/// The node types are from `ReservedCacheNodesOffering$CacheNodeType`'s own list
/// (`smithy_query(query="doc", shape="ReservedCacheNodesOffering",
/// member="CacheNodeType")`), spanning the current-generation general purpose
/// (T4g/T3, M7g/M6g/M5) and memory optimized (R7g/R6g/R5) families and the
/// previous generation (M3, R3, T2) that list also names.
///
/// The rates are the EMULATOR's own, not a claim about AWS's prices: AWS
/// publishes its price list outside this API and no ElastiCache operation
/// reports it. They are a deterministic scale — a family rate times the node
/// size — so the prices this catalog reports are consistent between node types
/// and stable across reads.
pub const OFFERING_NODE_TYPES: [(&str, f64); 19] = [
    ("cache.m3.medium", 0.08),
    ("cache.m5.2xlarge", 0.64),
    ("cache.m5.large", 0.16),
    ("cache.m5.xlarge", 0.32),
    ("cache.m6g.large", 0.16),
    ("cache.m6g.xlarge", 0.32),
    ("cache.m7g.large", 0.16),
    ("cache.m7g.xlarge", 0.32),
    ("cache.r3.large", 0.24),
    ("cache.r5.2xlarge", 0.96),
    ("cache.r5.large", 0.24),
    ("cache.r5.xlarge", 0.48),
    ("cache.r6g.large", 0.24),
    ("cache.r7g.large", 0.24),
    ("cache.t2.micro", 0.02),
    ("cache.t3.medium", 0.08),
    ("cache.t3.micro", 0.02),
    ("cache.t3.small", 0.04),
    ("cache.t4g.micro", 0.02),
];

/// `value` rounded to `places` decimal places, so a reported price is a money
/// amount rather than the full binary expansion of the product behind it.
pub(super) fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The share of the on-demand rate a reservation's effective hourly rate is,
/// per payment option and per TERM POSITION in [`OFFERING_TERMS`].
///
/// The EMULATOR's own scale (see [`OFFERING_NODE_TYPES`]), fixed so that it
/// keeps the ordering a reservation is bought for: a longer term discounts more
/// than a shorter one, and more money up front discounts more than less.
/// Indexing by the term's position means a term added to `OFFERING_TERMS` has
/// to be priced here too, instead of silently taking the shortest term's share.
pub(super) const OFFERING_RATE_SHARES: [(&str, [f64; OFFERING_TERMS.len()]); STOCKED_OFFERING_TYPES.len()] = [
    (OFFERING_TYPE_ALL_UPFRONT, [0.60, 0.45]),
    (OFFERING_TYPE_PARTIAL_UPFRONT, [0.65, 0.50]),
    (OFFERING_TYPE_NO_UPFRONT, [0.75, 0.60]),
];

/// The entry of [`OFFERING_RATE_SHARES`] this payment option and term are
/// priced by.
///
/// Both lookups are over the catalog's own constants — the only caller builds
/// the catalog out of [`OFFERING_TERMS`] and [`STOCKED_OFFERING_TYPES`] — so a
/// miss would mean the pricing table lost a row the catalog still stocks, which
/// is a bug in this module rather than anything a request can ask for.
pub(super) fn offering_rate_share(offering_type: &str, duration_seconds: i32) -> f64 {
    let term = OFFERING_TERMS
        .iter()
        .position(|(_, seconds)| *seconds == duration_seconds)
        .expect("a stocked offering's term is one of OFFERING_TERMS");
    OFFERING_RATE_SHARES
        .iter()
        .find(|(option, _)| *option == offering_type)
        .map(|(_, shares)| shares[term])
        .expect("a stocked offering's payment option is priced by OFFERING_RATE_SHARES")
}

/// The `ReservedCacheNodesOfferingId` of the offering with these attributes.
///
/// "A unique identifier for the reserved cache node offering", in the form the
/// member's own example gives (`438012d3-4052-4cc7-b2e3-8d3372e0e706`): five
/// hyphen-separated hex groups. It is DERIVED from the attributes that make the
/// offering, so it is unique per offering and identical on every read, and the
/// catalog needs no counter to mint it.
pub fn offering_id(
    product_description: &str,
    cache_node_type: &str,
    duration_seconds: i32,
    offering_type: &str,
) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(
        format!(
            "{ARN_NAMESPACE}|{product_description}|{cache_node_type}|{duration_seconds}|{offering_type}"
        )
        .as_bytes(),
    );
    let digest = ::hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// The key an offering is held under, which is also the total order
/// `DescribeReservedCacheNodesOfferings` pages through and the position its
/// `Marker` resumes from.
///
/// The term is zero-padded so the key orders by term rather than by the digits
/// of the second count.
pub(super) fn offering_key(
    product_description: &str,
    cache_node_type: &str,
    duration_seconds: i32,
    offering_type: &str,
) -> String {
    format!("{product_description}|{cache_node_type}|{duration_seconds:010}|{offering_type}")
}

/// One catalog entry, priced from the node type's hourly rate.
///
/// `FixedPrice` is "the fixed price charged for this offering" — the whole term
/// for an all-upfront reservation, half of it for a partial-upfront one, nothing
/// for a no-upfront one — and whatever is not paid up front is the hourly
/// `RecurringCharges` entry. EVERY offering publishes that entry, so
/// `RecurringCharges` is always a `RecurringCharge` structure a client can read
/// the frequency and the amount off: an all-upfront offering's amount is 0,
/// because its whole term is already in the fixed price, which is the one
/// combination that pairs with a non-zero `FixedPrice`. `UsagePrice`, "the
/// hourly price charged for this offering", is 0 for all three: an upfront-type
/// reservation's hourly charge is the recurring charge, not a usage price.
pub(super) fn stock_offering(
    product_description: &str,
    cache_node_type: &str,
    base_hourly: f64,
    duration_seconds: i32,
    offering_type: &'static str,
) -> ReservedCacheNodesOffering {
    let hours = f64::from(duration_seconds) / SECONDS_PER_HOUR;
    let effective_hourly = base_hourly * offering_rate_share(offering_type, duration_seconds);
    let (fixed_price, recurring_hourly) = if offering_type == OFFERING_TYPE_ALL_UPFRONT {
        (effective_hourly * hours, 0.0)
    } else if offering_type == OFFERING_TYPE_PARTIAL_UPFRONT {
        (
            effective_hourly * hours * PARTIAL_UPFRONT_SHARE,
            effective_hourly * (1.0 - PARTIAL_UPFRONT_SHARE),
        )
    } else {
        (0.0, effective_hourly)
    };
    let recurring_charges = vec![RecurringCharge {
        recurring_charge_amount: Some(round_to(recurring_hourly, 4)),
        recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
    }];
    ReservedCacheNodesOffering {
        reserved_cache_nodes_offering_id: Some(offering_id(
            product_description,
            cache_node_type,
            duration_seconds,
            offering_type,
        )),
        cache_node_type: Some(cache_node_type.to_string()),
        duration: Some(duration_seconds),
        fixed_price: Some(round_to(fixed_price, 2)),
        usage_price: Some(0.0),
        product_description: Some(product_description.to_string()),
        offering_type: Some(offering_type.to_string()),
        recurring_charges: Some(recurring_charges),
    }
}

/// The whole published catalog: one offering per cache engine, node type, term
/// and stocked payment option, keyed by [`offering_key`].
///
/// A `BTreeMap` is what gives the describe the stable total order its `Marker`
/// indexes into, the same way the cluster and subnet-group maps do.
pub fn offering_catalog() -> BTreeMap<String, ReservedCacheNodesOffering> {
    let mut catalog = BTreeMap::new();
    // `ProductDescription` is "the cache engine used by the offering", so the
    // catalog carries one offering per engine this service runs.
    for product_description in ENGINES {
        for (cache_node_type, base_hourly) in OFFERING_NODE_TYPES {
            for (_, duration_seconds) in OFFERING_TERMS {
                for offering_type in STOCKED_OFFERING_TYPES {
                    catalog.insert(
                        offering_key(
                            product_description,
                            cache_node_type,
                            duration_seconds,
                            offering_type,
                        ),
                        stock_offering(
                            product_description,
                            cache_node_type,
                            base_hourly,
                            duration_seconds,
                            offering_type,
                        ),
                    );
                }
            }
        }
    }
    catalog
}

/// The term in SECONDS a `Duration` filter value names, or the reason it names
/// none.
///
/// `Duration` is a plain string with no modelled constraint, so the documented
/// "Valid Values: 1 | 3 | 31536000 | 94608000" is enforced here; a year count
/// and the matching second count select the same offerings.
pub fn parse_offering_duration(value: &str) -> Result<i32, String> {
    OFFERING_TERMS
        .iter()
        .find(|(years, seconds)| value == years.to_string() || value == seconds.to_string())
        .map(|(_, seconds)| *seconds)
        .ok_or_else(|| {
            let valid = OFFERING_TERMS
                .iter()
                .flat_map(|(years, seconds)| [years.to_string(), seconds.to_string()])
                .collect::<Vec<_>>()
                .join(" | ");
            format!("Invalid value {value} for Duration: must be one of {valid}.")
        })
}

/// The documented `OfferingType` `value` names, or the reason it names none.
///
/// Matched ignoring ASCII case so the answer does not turn on how the caller
/// capitalised a documented value, and the canonical spelling is what any
/// comparison then uses.
pub fn canonical_offering_type(value: &str) -> Result<&'static str, String> {
    OFFERING_TYPES
        .into_iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(value))
        .ok_or_else(|| {
            format!(
                "Invalid value {value} for OfferingType: must be one of {}.",
                OFFERING_TYPES.join(" | ")
            )
        })
}

/// The five filter members `DescribeReservedCacheNodesOfferings` narrows the
/// catalog with (and `DescribeReservedCacheNodes` documents the same five for
/// the reservations bought from it).
///
/// A member the request omits filters nothing; one it sets keeps only the
/// offerings whose value matches it, so several set together intersect.
#[derive(Debug, Default, Clone)]
pub struct OfferingFilter {
    /// `ReservedCacheNodesOfferingId` — "show only the available offering that
    /// matches the specified reservation identifier". Matched exactly: it is an
    /// opaque identifier.
    pub offering_id: Option<String>,
    /// `CacheNodeType` — "show only the available offerings matching the
    /// specified cache node type".
    pub cache_node_type: Option<String>,
    /// `Duration`, as the term in seconds that
    /// [`parse_offering_duration`] resolved it to.
    pub duration_seconds: Option<i32>,
    /// `ProductDescription` — "show only the available offerings matching the
    /// specified product description".
    pub product_description: Option<String>,
    /// `OfferingType`, in the canonical spelling
    /// [`canonical_offering_type`] resolved.
    pub offering_type: Option<&'static str>,
}

impl OfferingFilter {
    /// True when a record carrying these five attributes satisfies every member
    /// this filter sets.
    ///
    /// The offering id is compared EXACTLY — it is an opaque identifier, so one
    /// that differs anywhere, case included, names a different offering, which
    /// is what lets the caller be told it does not exist rather than handed an
    /// empty page. The node type, the engine and the offering type are compared
    /// ignoring ASCII case: a node type and an engine name are both stored
    /// lowercase, and an offering type is a documented display spelling, so case
    /// is part of none of their identities.
    ///
    /// Taking the attributes rather than a shape is what lets a published
    /// offering and a reservation bought from one be narrowed by ONE
    /// implementation of the five comparisons, so the two describes cannot
    /// disagree about what a filter value selects.
    pub(super) fn matches_attributes(
        &self,
        offering_id: Option<&str>,
        cache_node_type: Option<&str>,
        duration_seconds: Option<i32>,
        product_description: Option<&str>,
        offering_type: Option<&str>,
    ) -> bool {
        let same = |filter: Option<&str>, value: Option<&str>| {
            filter.is_none_or(|wanted| {
                value.is_some_and(|actual| actual.eq_ignore_ascii_case(wanted))
            })
        };
        self.offering_id
            .as_deref()
            .is_none_or(|wanted| offering_id.is_some_and(|actual| actual == wanted))
            && same(self.cache_node_type.as_deref(), cache_node_type)
            && same(self.product_description.as_deref(), product_description)
            && self
                .duration_seconds
                .is_none_or(|wanted| duration_seconds == Some(wanted))
            && same(self.offering_type, offering_type)
    }

    /// True when `offering` satisfies every member this filter sets.
    pub fn matches(&self, offering: &ReservedCacheNodesOffering) -> bool {
        self.matches_attributes(
            offering.reserved_cache_nodes_offering_id.as_deref(),
            offering.cache_node_type.as_deref(),
            offering.duration,
            offering.product_description.as_deref(),
            offering.offering_type.as_deref(),
        )
    }

    /// True when `reservation` satisfies every member this filter sets.
    ///
    /// `DescribeReservedCacheNodes` documents the same filter members over the
    /// reservations bought from the catalog as
    /// `DescribeReservedCacheNodesOfferings` documents over the catalog itself,
    /// so the comparison is the same one, made against the reservation's own
    /// copy of the offering's attributes.
    pub fn matches_reservation(&self, reservation: &ReservedCacheNodeRecord) -> bool {
        self.matches_attributes(
            Some(&reservation.reserved_cache_nodes_offering_id),
            reservation.cache_node_type.as_deref(),
            reservation.duration_seconds,
            reservation.product_description.as_deref(),
            reservation.offering_type.as_deref(),
        )
    }
}

// ---------------------------------------------------------------------------
// Reserved cache nodes
//
// A reserved cache node — a reservation — IS an account resource, unlike the
// offering it is bought from: `PurchaseReservedCacheNodesOffering` mints one
// from a catalog entry above and `DescribeReservedCacheNodes` reports it. The
// record below is the one copy both read, so the two cannot disagree about
// what was bought.
// ---------------------------------------------------------------------------

/// The number of nodes a purchase reserves when the request names none, as
/// `PurchaseReservedCacheNodesOffering$CacheNodeCount` documents it: "The
/// number of cache node instances to reserve. Default: 1".
pub const DEFAULT_CACHE_NODE_COUNT: i32 = 1;

/// The states a reservation is reported in.
///
/// `ReservedCacheNode$State` is documented only as "the state of the reserved
/// cache node" and the model declares no enum for it, so these are the states
/// ElastiCache reports a reservation in: a purchase is `payment-pending` until
/// its payment is taken, `active` for the term it was bought for, and `retired`
/// once that term has elapsed.
pub const RESERVATION_STATE_PAYMENT_PENDING: &str = "payment-pending";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_ACTIVE: &str = "active";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_RETIRED: &str = "retired";

/// How long a reservation stays `payment-pending` before the clock owes it
/// `active`.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: AWS takes as long as the
/// payment takes, the emulator chooses a span, and it is deliberately NOT zero
/// so the purchase itself answers with the transient state. The due instant is
/// stored on the record and materialised by whichever later call reads it; a
/// report issued inside the span is answered once it has fallen due.
pub const RESERVATION_TRANSITION: Duration = Duration::from_millis(500);

/// How many reserved cache nodes the store holds before a purchase is refused
/// with the modelled `ReservedCacheNodeQuotaExceededFault` — "the request
/// cannot be processed because it would exceed the user's cache node quota".
///
/// See [`CLUSTER_QUOTA`]: the emulator's OWN bound on the store rather than a
/// claim about any account's limit, set at the same count as the cluster quota
/// because both bound cache nodes.
pub const RESERVED_CACHE_NODE_QUOTA: i32 = 300;

/// The ARN of a reserved cache node, in the form
/// `ReservedCacheNode$ReservationARN`'s own example gives
/// (`arn:aws:elasticache:us-east-1:123456789012:reserved-instance:ri-2017-03-27-08-33-25-582`).
pub fn reservation_arn(region: &str, reserved_cache_node_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("reserved-instance:{reserved_cache_node_id}"),
    )
}

/// The identifier a reservation the request named none carries:
/// `PurchaseReservedCacheNodesOffering$ReservedCacheNodeId` documents "if this
/// parameter is not specified, ElastiCache automatically generates an
/// identifier for the reservation".
///
/// The generated form is the one `ReservationARN`'s example carries — `ri-`
/// followed by the purchase instant with every field hyphen-separated
/// (`ri-2017-03-27-08-33-25-582`) — built from the RFC-3339 rendering of the
/// instant, so the service's one timestamp implementation is also what names a
/// reservation.
pub fn generated_reservation_id(at: SystemTime) -> String {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let stamp: String = rendered
        .trim_end_matches('Z')
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { '-' })
        .collect();
    format!("ri-{stamp}")
}

/// The values a new reservation carries that do not come from the offering it
/// is bought from — one argument so
/// [`ReservedCacheNodeRecord::from_offering`] does not take a row of positional
/// values.
#[derive(Debug, Clone)]
pub struct NewReservation {
    /// The reservation's identity: the `ReservedCacheNodeId` the request named,
    /// or the one [`generated_reservation_id`] minted for it.
    pub reserved_cache_node_id: String,
    /// `CacheNodeCount` — "the number of cache node instances to reserve".
    pub cache_node_count: i32,
    /// The Region of the request that bought it, which its ARN names.
    pub region: String,
    /// The tags the purchase was given. The ONE copy of this reservation's tags:
    /// `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// One reserved cache node, as this emulator persists it.
///
/// Every priced member is COPIED out of the catalog entry the reservation was
/// bought from, which is what makes a reservation a record of what was bought
/// rather than a second view of the live catalog.
#[derive(Debug, Clone)]
pub struct ReservedCacheNodeRecord {
    /// "The unique identifier for the reservation", which is this record's
    /// identity in the store.
    pub reserved_cache_node_id: String,
    /// The offering it was bought from.
    pub reserved_cache_nodes_offering_id: String,
    pub cache_node_type: Option<String>,
    /// "The time the reservation started" — the instant of the purchase.
    pub start_time: SystemTime,
    /// "The duration of the reservation in seconds", from the offering's term.
    pub duration_seconds: Option<i32>,
    pub fixed_price: Option<f64>,
    pub usage_price: Option<f64>,
    /// "The number of cache nodes that have been reserved."
    pub cache_node_count: i32,
    pub product_description: Option<String>,
    pub offering_type: Option<String>,
    /// One of [`RESERVATION_STATE_PAYMENT_PENDING`],
    /// [`RESERVATION_STATE_ACTIVE`] or [`RESERVATION_STATE_RETIRED`].
    pub state: String,
    /// The state change the clock owes this reservation, if any.
    pub pending_state: Option<PendingTransition>,
    /// "The recurring price charged to run this reserved cache node", from the
    /// offering.
    pub recurring_charges: Vec<RecurringCharge>,
    pub arn: String,
    /// See [`NewReservation::tags`].
    pub tags: Vec<(String, String)>,
}

impl ReservedCacheNodeRecord {
    /// The reservation `new` buys from `offering`.
    pub fn from_offering(offering: &ReservedCacheNodesOffering, new: NewReservation) -> Self {
        Self {
            arn: reservation_arn(&new.region, &new.reserved_cache_node_id),
            reserved_cache_node_id: new.reserved_cache_node_id,
            reserved_cache_nodes_offering_id: offering
                .reserved_cache_nodes_offering_id
                .clone()
                .unwrap_or_default(),
            cache_node_type: offering.cache_node_type.clone(),
            start_time: new.now,
            duration_seconds: offering.duration,
            // The offering's prices are per reserved cache node — "the fixed
            // price charged for this offering" and "the hourly price charged
            // for this offering" — so a multi-node reservation reports the same
            // per-node prices beside its `CacheNodeCount` rather than a total
            // no member of this shape documents.
            fixed_price: offering.fixed_price,
            usage_price: offering.usage_price,
            cache_node_count: new.cache_node_count,
            product_description: offering.product_description.clone(),
            offering_type: offering.offering_type.clone(),
            state: RESERVATION_STATE_PAYMENT_PENDING.to_string(),
            pending_state: Some(PendingTransition {
                due: new.now + RESERVATION_TRANSITION,
                status: RESERVATION_STATE_ACTIVE.to_string(),
            }),
            recurring_charges: offering.recurring_charges.clone().unwrap_or_default(),
            tags: new.tags,
        }
    }

    /// Materialise every state change the clock owes this reservation. The one
    /// write a reporting operation may make; see [`SnapshotRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_payment(now);
        self.settle_term(now);
    }

    /// `payment-pending` becomes `active` once the payment's due instant has
    /// passed.
    pub(super) fn settle_payment(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_state.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.status.clone();
        self.pending_state = None;
    }

    /// An `active` reservation becomes `retired` once the term it was bought
    /// for has elapsed: the reservation runs for its offering's `Duration`
    /// ("the duration of the reservation in seconds") from its `StartTime`, and
    /// that span is the offering's own rather than one the emulator chose.
    pub(super) fn settle_term(&mut self, now: SystemTime) {
        if self.state != RESERVATION_STATE_ACTIVE {
            return;
        }
        let Some(seconds) = self
            .duration_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
        else {
            return;
        };
        if now < self.start_time + Duration::from_secs(seconds) {
            return;
        }
        self.state = RESERVATION_STATE_RETIRED.to_string();
    }

    /// This record as the modelled `ReservedCacheNode`.
    pub fn to_modeled(&self) -> ReservedCacheNode {
        ReservedCacheNode {
            reserved_cache_node_id: Some(self.reserved_cache_node_id.clone()),
            reserved_cache_nodes_offering_id: Some(self.reserved_cache_nodes_offering_id.clone()),
            cache_node_type: self.cache_node_type.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            duration: self.duration_seconds,
            fixed_price: self.fixed_price,
            usage_price: self.usage_price,
            cache_node_count: Some(self.cache_node_count),
            product_description: self.product_description.clone(),
            offering_type: self.offering_type.clone(),
            state: Some(self.state.clone()),
            recurring_charges: Some(self.recurring_charges.clone()),
            reservation_arn: Some(self.arn.clone()),
        }
    }
}
