// SPDX-License-Identifier: CC0-1.0

//! The Reserved Instance offering catalog, split out of [`crate::state`] unchanged: the
//! offerings this service publishes and the prices each of them is published at.

use super::*;

/// `ReservedInstancePaymentOption`'s `ALL_UPFRONT` value: the whole term is paid as
/// one upfront charge, so the reservation's recurring charge is a zero amount.
pub const PAYMENT_OPTION_ALL_UPFRONT: &str = "ALL_UPFRONT";

/// `ReservedInstancePaymentOption`'s `PARTIAL_UPFRONT` value: part of the term is
/// paid upfront and the rest as a recurring charge.
pub const PAYMENT_OPTION_PARTIAL_UPFRONT: &str = "PARTIAL_UPFRONT";

/// `ReservedInstancePaymentOption`'s `NO_UPFRONT` value: nothing is paid upfront and
/// the whole term is billed as a recurring charge.
pub const PAYMENT_OPTION_NO_UPFRONT: &str = "NO_UPFRONT";

/// `ReservedInstanceOffering$Duration` for a one-year term, in seconds — the unit
/// that member is documented in ("the duration, in seconds, for which the offering
/// will reserve the OpenSearch instance").
pub const TERM_ONE_YEAR: i32 = 365 * 24 * 60 * 60;

/// `ReservedInstanceOffering$Duration` for a three-year term, in seconds.
pub const TERM_THREE_YEARS: i32 = 3 * TERM_ONE_YEAR;

/// The terms the offering catalog publishes, shortest first: OpenSearch Service
/// reserves an instance for one year or for three.
pub const RESERVED_INSTANCE_TERMS: [i32; 2] = [TERM_ONE_YEAR, TERM_THREE_YEARS];

/// How many offerings ONE page of `DescribeReservedInstanceOfferings` carries when the
/// request names no `MaxResults`.
///
/// `MaxResults` is `@default(0)`: a request that names no page size has asked for no
/// PARTICULAR one, and the service answers with its own default page rather than with
/// the whole catalog — which is why the recorded first answer of this catalog carries a
/// continuation token. The model publishes no default, so this is the emulator's choice
/// of it, and it is the largest page a caller can ask for (`MaxResults` is
/// `@range(max: 100)`): a service does not answer an unbounded request with more than it
/// will answer a bounded one with. The catalog spans every instance type times every
/// term times every payment option, so it is far larger than this and the listing really
/// does hand out tokens.
pub const RESERVED_INSTANCE_OFFERING_PAGE: usize = 100;

/// `ReservedInstanceOffering$CurrencyCode`: every price in the catalog is quoted in
/// US dollars, the currency AWS publishes its price list in.
pub const RESERVED_INSTANCE_CURRENCY_CODE: &str = "USD";

/// `RecurringCharge$RecurringChargeFrequency` for a reservation's recurring charge:
/// the charge is billed per hour of the term.
pub const RECURRING_CHARGE_HOURLY: &str = "Hourly";

/// How many seconds make an hour, for turning a term's `Duration` into the hours a
/// recurring charge is billed over.
pub(super) const SECONDS_PER_HOUR: f64 = 3600.0;

/// AWS's published instance-size normalization factors, keyed by the size token of an
/// `OpenSearchPartitionInstanceType` (which is `<family>.<size>.search`). They are
/// the factors AWS publishes for Reserved Instance size flexibility — one `large` is
/// four units, one `xlarge` eight, and so on — and they are what makes a bigger size
/// of one family cost proportionally more in [`on_demand_hourly_price`].
pub(super) const SIZE_NORMALIZATION_FACTORS: [(&str, f64); 17] = [
    ("nano", 0.25),
    ("micro", 0.5),
    ("small", 1.0),
    ("medium", 2.0),
    ("large", 4.0),
    ("xlarge", 8.0),
    ("2xlarge", 16.0),
    ("3xlarge", 24.0),
    ("4xlarge", 32.0),
    ("6xlarge", 48.0),
    ("8xlarge", 64.0),
    ("9xlarge", 72.0),
    ("10xlarge", 80.0),
    ("12xlarge", 96.0),
    ("16xlarge", 128.0),
    ("18xlarge", 144.0),
    ("24xlarge", 192.0),
];

/// The hourly on-demand rate, in US dollars, of ONE normalization unit of an instance
/// class, keyed by the first letter of the instance family (`c5`, `m6g`, `or1`,
/// `ultrawarm1`, …).
///
/// This is the emulator's OWN price list: AWS's price list is not in the Smithy model
/// and is not published as part of the API, so there is nothing to read it from. What
/// the catalog therefore guarantees is the RELATION between its prices, not an AWS
/// price: a compute-optimized unit costs less than a memory-optimized one, a burstable
/// one less than either, and the size factor scales each. Every price the catalog
/// reports is derived from this table, so the same offering costs the same on every
/// call and in every run.
pub(super) const CLASS_HOURLY_RATE_PER_UNIT: [(char, f64); 8] = [
    // Burstable (`t2`, `t3`, `t4g`).
    ('t', 0.018),
    // Compute optimized (`c4`, `c5`, `c6g`).
    ('c', 0.034),
    // General purpose (`m3`, `m4`, `m5`, `m6g`).
    ('m', 0.036),
    // OpenSearch optimized (`or1`).
    ('o', 0.040),
    // Memory optimized (`r3`, `r4`, `r5`, `r6g`, `r6gd`).
    ('r', 0.047),
    // Dense storage (`d2`).
    ('d', 0.052),
    // Storage optimized (`i2`, `i3`).
    ('i', 0.055),
    // UltraWarm (`ultrawarm1`).
    ('u', 0.060),
];

/// The fraction of the on-demand rate a ONE-year reservation costs, before the
/// payment-option discount. The emulator's own figure, like
/// [`CLASS_HOURLY_RATE_PER_UNIT`]; what it fixes is that reserving is cheaper than
/// paying on demand.
pub(super) const ONE_YEAR_RATE: f64 = 0.78;

/// The same fraction for a THREE-year reservation: a longer commitment costs less per
/// hour than a shorter one, so this is below [`ONE_YEAR_RATE`].
pub(super) const THREE_YEAR_RATE: f64 = 0.52;

/// The further discount for paying the whole term upfront — the largest of the three,
/// so `ALL_UPFRONT` is the cheapest payment option of a term.
pub(super) const ALL_UPFRONT_DISCOUNT: f64 = 0.92;

/// The further discount for paying part of the term upfront: between
/// [`ALL_UPFRONT_DISCOUNT`] and [`NO_UPFRONT_DISCOUNT`].
pub(super) const PARTIAL_UPFRONT_DISCOUNT: f64 = 0.96;

/// Paying nothing upfront earns no further discount, so this payment option is the
/// most expensive of a term.
pub(super) const NO_UPFRONT_DISCOUNT: f64 = 1.0;

/// The hourly on-demand rate of one instance type: its class's rate per normalization
/// unit times its size's factor.
///
/// `None` for a value that is not `<family>.<size>.search` with a known size token or
/// instance class — which no value of `OpenSearchPartitionInstanceType` is, so the
/// catalog spans the whole enum.
pub(super) fn on_demand_hourly_price(instance_type: &str) -> Option<f64> {
    let factor = size_normalization_factor(instance_type)?;
    let class = instance_family(instance_type)?.chars().next()?;
    let rate = CLASS_HOURLY_RATE_PER_UNIT
        .iter()
        .find(|(letter, _)| *letter == class)
        .map(|(_, rate)| *rate)?;
    Some(rate * factor)
}

/// The size normalization factor of one instance type, from
/// [`SIZE_NORMALIZATION_FACTORS`]: how many units of its instance class one instance of
/// that size is. It is the ONE scale this crate expresses a per-size quantity against,
/// so a bigger size of a family is bigger by the same factor wherever the size matters.
///
/// `None` for a value that is not `<family>.<size>.search` with a published size token,
/// which no value of `OpenSearchPartitionInstanceType` is.
pub fn size_normalization_factor(instance_type: &str) -> Option<f64> {
    let size = instance_type.split('.').nth(1)?;
    SIZE_NORMALIZATION_FACTORS
        .iter()
        .find(|(token, _)| *token == size)
        .map(|(_, factor)| *factor)
}

/// The fraction of the on-demand rate a reservation of this term and payment option
/// costs: the term's rate times the payment option's discount.
///
/// `None` for a payment option outside `ReservedInstancePaymentOption`, so a value
/// added to that enum is left out of the catalog rather than silently priced as one
/// of the three it does know.
pub(super) fn reserved_rate(duration: i32, payment_option: &str) -> Option<f64> {
    let term = if duration >= TERM_THREE_YEARS {
        THREE_YEAR_RATE
    } else {
        ONE_YEAR_RATE
    };
    let upfront = match payment_option {
        PAYMENT_OPTION_ALL_UPFRONT => ALL_UPFRONT_DISCOUNT,
        PAYMENT_OPTION_PARTIAL_UPFRONT => PARTIAL_UPFRONT_DISCOUNT,
        PAYMENT_OPTION_NO_UPFRONT => NO_UPFRONT_DISCOUNT,
        _ => return None,
    };
    Some(term * upfront)
}

/// `value` rounded to whole cents, the precision an upfront charge is quoted at.
pub(super) fn cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `value` rounded to the precision AWS quotes an hourly rate at — thousandths of a
/// cent, so a small instance's hourly charge does not round away to zero.
pub(super) fn hourly(value: f64) -> f64 {
    (value * 100_000.0).round() / 100_000.0
}

/// The `ReservedInstanceOfferingId` of one catalog entry.
///
/// DERIVED from what the entry IS (its instance type, term and payment option) as a
/// name-based v5 UUID, so it satisfies `GUID`'s
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, two
/// entries can never share an id, and the id a caller read in one call is the id the
/// next call reports — which a freshly generated id would not be.
pub(super) fn offering_id(instance_type: &str, duration: i32, payment_option: &str) -> String {
    ::uuid::Uuid::new_v5(
        &::uuid::Uuid::NAMESPACE_OID,
        format!("opensearch/reservedInstanceOffering/{instance_type}/{duration}/{payment_option}")
            .as_bytes(),
    )
    .to_string()
}

/// One catalog entry: what one instance type costs to reserve for one term under one
/// payment option.
///
/// The three price members hold together the way a reservation is billed: the upfront
/// charge is `FixedPrice`, the rest of the term is a recurring hourly charge, and
/// `UsagePrice` — the rate charged for USING the reserved capacity — is zero, because
/// the reservation has already paid for the hours. So `ALL_UPFRONT` carries the whole
/// term as a fixed price and nothing hourly, `NO_UPFRONT` everything hourly and no
/// fixed price, and `PARTIAL_UPFRONT` half the term each way.
///
/// `RecurringCharges` carries ONE entry whatever the payment option: "the recurring
/// charge to your account" is billed per hour of the term for every offering, so an
/// offering whose term is paid up front publishes that charge with a ZERO amount — the
/// charge exists and is nothing — rather than publishing no charge at all, which would
/// leave a caller summing an empty list to tell a prepaid offering from one whose
/// hourly rate the answer simply did not report.
pub(super) fn reserved_instance_offering(
    instance_type: &str,
    duration: i32,
    payment_option: &str,
) -> Option<ReservedInstanceOffering> {
    let on_demand = on_demand_hourly_price(instance_type)?;
    let effective_hourly = on_demand * reserved_rate(duration, payment_option)?;
    let term_hours = f64::from(duration) / SECONDS_PER_HOUR;
    let (fixed_price, recurring_hourly) = match payment_option {
        PAYMENT_OPTION_ALL_UPFRONT => (effective_hourly * term_hours, 0.0),
        PAYMENT_OPTION_PARTIAL_UPFRONT => {
            (effective_hourly * term_hours / 2.0, effective_hourly / 2.0)
        }
        PAYMENT_OPTION_NO_UPFRONT => (0.0, effective_hourly),
        _ => return None,
    };
    let recurring_charges = vec![RecurringCharge {
        recurring_charge_amount: Some(hourly(recurring_hourly)),
        recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
    }];
    Some(ReservedInstanceOffering {
        reserved_instance_offering_id: Some(offering_id(instance_type, duration, payment_option)),
        instance_type: Some(instance_type.to_string()),
        duration,
        fixed_price: Some(cents(fixed_price)),
        usage_price: Some(0.0),
        currency_code: Some(RESERVED_INSTANCE_CURRENCY_CODE.to_string()),
        payment_option: Some(payment_option.to_string()),
        recurring_charges: Some(recurring_charges),
    })
}

/// Where one offering sits in the catalog the service PUBLISHES: the instance type it
/// reserves, in the order the published instance-type catalog lists them
/// ([`published_instance_types`]), then the term shortest first, then the payment
/// option in the order `ReservedInstancePaymentOption` declares them.
///
/// A price list is published per instance type, so that is the key a page of it is read
/// along: a caller walking the catalog sees one instance type's terms and payment options
/// together, and the same order on every call. The id an entry is STORED under is a digest
/// of what the entry is, which orders the catalog by nothing a caller can read.
pub(super) fn offering_catalog_rank(offering: &ReservedInstanceOffering) -> (usize, i32, usize) {
    let position = |values: &[&str], value: Option<&str>| {
        values
            .iter()
            .position(|candidate| Some(*candidate) == value)
            .unwrap_or(values.len())
    };
    (
        position(
            published_instance_type_order(),
            offering.instance_type.as_deref(),
        ),
        offering.duration,
        position(
            RESERVED_INSTANCE_PAYMENT_OPTION_VALUES,
            offering.payment_option.as_deref(),
        ),
    )
}

/// The published instance-type order ([`published_instance_types`]), derived ONCE.
///
/// The order is a function of the catalog table alone, so it is the same on every call:
/// ranking an offering is a lookup in it rather than a fresh walk of the catalog, which
/// is what keeps ordering a page of a catalog spanning every instance type, term and
/// payment option proportional to the page rather than to the catalog per entry.
fn published_instance_type_order() -> &'static [&'static str] {
    static ORDER: ::std::sync::OnceLock<Vec<&'static str>> = ::std::sync::OnceLock::new();
    ORDER.get_or_init(published_instance_types)
}

/// The whole offering catalog, keyed by `ReservedInstanceOfferingId`.
///
/// Reserved Instance offerings are published BY the service for a Region, not created
/// by a caller, so the catalog is seeded with the state rather than grown by an
/// operation: it is what `DescribeReservedInstanceOfferings` reports and what a
/// purchase resolves an offering id against. It spans one entry per instance type the
/// service PUBLISHES ([`published_instance_types`] — the one record of which instance
/// types exist, which carries the current generations the checked-in
/// `OpenSearchPartitionInstanceType` value set, an older snapshot of the same catalog,
/// does not list) times each term in [`RESERVED_INSTANCE_TERMS`] and each value of
/// `ReservedInstancePaymentOption`, so it is bounded and complete: an instance type a
/// client can RUN is one it can reserve, and the two catalogs cannot disagree about
/// which those are.
pub(super) fn seed_reserved_instance_offerings() -> BTreeMap<String, ReservedInstanceOffering> {
    let mut catalog = BTreeMap::new();
    for instance_type in published_instance_types() {
        for duration in RESERVED_INSTANCE_TERMS {
            for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
                let Some(offering) =
                    reserved_instance_offering(instance_type, duration, payment_option)
                else {
                    continue;
                };
                // The id the record CARRIES is the key it is stored under, so the id
                // has one producer per entry and no stored entry can report an id
                // other than the one a caller resolves it by — which is also the id
                // paging resumes from.
                let Some(id) = offering.reserved_instance_offering_id.clone() else {
                    continue;
                };
                catalog.insert(id, offering);
            }
        }
    }
    catalog
}
